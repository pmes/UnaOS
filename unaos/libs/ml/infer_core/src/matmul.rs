// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The dense layer `y = x·Wᵀ + b` (`nn.Linear`: W is `[out, in]`), with a cache-blocked kernel.
//!
//! **Layout.** At load, W is packed into panels of [`NR`] output columns: panel `p` holds, for
//! every input index `k`, the [`NR`] weights `W[NR·p + c][k]` contiguously (zero-padded past
//! `out`), so the kernel streams a panel linearly. Weights are f32 or, on the f16 path, binary16
//! (half the memory; each `KC × NR` block is widened to f32 once into a scratch buffer and reused
//! for every row — widening is exact, so the f16 path computes exactly what the f32 path would on
//! the f16-rounded weights).
//!
//! **Blocking.** `k` in blocks of [`KC`] (a `KC × NR` weight block is 8 KB: it stays in L1), then
//! panels, then rows in groups of [`MR`]; the `MR × NR` accumulators live in registers.
//!
//! **Order of operations (reproducibility).** Every output element is
//! `((0 + x[i,0]·w[0]) + x[i,1]·w[1]) + … + x[i,in-1]·w[in-1]`, summed in f32 in ascending `k`,
//! then `+ b`. Blocking only changes WHEN each partial sum is computed, never the sequence of
//! roundings (the accumulator is stored and reloaded between `k` blocks), and Rust never fuses
//! `a·b + c` into an FMA — so the result is the same bits on every machine and for any `m`.

use alloc::vec;
use alloc::vec::Vec;

use crate::half::{f16_to_f32, f32_to_f16};
use crate::{Result, err};

/// Output columns per panel.
pub const NR: usize = 8;
/// Rows per register block.
pub const MR: usize = 4;
/// Input depth per cache block.
pub const KC: usize = 256;

#[derive(Debug, Clone)]
enum Panels {
    F32(Vec<f32>),
    F16(Vec<u16>),
}

#[derive(Debug, Clone)]
pub struct Linear {
    pub n_in: usize,
    pub n_out: usize,
    panels: Panels,
    bias: Vec<f32>,
}

impl Linear {
    /// From an `[n_out, n_in]` row-major weight and an `n_out` bias; `half` stores the weights
    /// as binary16 (rounded to nearest even).
    pub fn new(weight: &[f32], bias: &[f32], n_out: usize, n_in: usize, half: bool) -> Result<Self> {
        if n_in == 0 || n_out == 0 || Some(weight.len()) != n_out.checked_mul(n_in) || bias.len() != n_out {
            return Err(err(alloc::format!("linear: weight {} / bias {} do not fit [{n_out}, {n_in}]", weight.len(), bias.len())));
        }
        let np = n_out.div_ceil(NR);
        let mut packed = vec![0.0f32; np * n_in * NR];
        for j in 0..n_out {
            let (p, c) = (j / NR, j % NR);
            let row = &weight[j * n_in..(j + 1) * n_in];
            for (k, &w) in row.iter().enumerate() {
                packed[(p * n_in + k) * NR + c] = w;
            }
        }
        let panels = if half { Panels::F16(packed.iter().map(|&w| f32_to_f16(w)).collect()) } else { Panels::F32(packed) };
        Ok(Linear { n_in, n_out, panels, bias: bias.to_vec() })
    }

    /// Concatenate layers with the same input along the output axis (Q, K, V → one QKV layer).
    /// Each output column is computed exactly as it was alone.
    pub fn concat(parts: &[(&[f32], &[f32], usize)], n_in: usize, half: bool) -> Result<Self> {
        let mut w = Vec::new();
        let mut b = Vec::new();
        let mut n_out = 0;
        for (pw, pb, o) in parts {
            if Some(pw.len()) != o.checked_mul(n_in) || pb.len() != *o {
                return Err(err("linear: concat part does not fit"));
            }
            w.extend_from_slice(pw);
            b.extend_from_slice(pb);
            n_out += o;
        }
        Self::new(&w, &b, n_out, n_in, half)
    }

    /// `out[m × n_out] = x[m × n_in] · Wᵀ + b`.
    pub fn forward(&self, x: &[f32], m: usize, out: &mut [f32]) {
        let (n_in, n_out) = (self.n_in, self.n_out);
        debug_assert!(x.len() >= m * n_in && out.len() >= m * n_out);
        out[..m * n_out].iter_mut().for_each(|v| *v = 0.0);
        let np = n_out.div_ceil(NR);
        let mut scratch = match self.panels {
            Panels::F16(_) => vec![0.0f32; KC * NR],
            Panels::F32(_) => Vec::new(),
        };
        let mut k0 = 0;
        while k0 < n_in {
            let k1 = (k0 + KC).min(n_in);
            for p in 0..np {
                let block: &[f32] = match &self.panels {
                    Panels::F32(w) => &w[(p * n_in + k0) * NR..(p * n_in + k1) * NR],
                    Panels::F16(h) => {
                        let src = &h[(p * n_in + k0) * NR..(p * n_in + k1) * NR];
                        for (d, &s) in scratch.iter_mut().zip(src) {
                            *d = f16_to_f32(s);
                        }
                        &scratch[..src.len()]
                    }
                };
                let j0 = p * NR;
                let cols = NR.min(n_out - j0);
                let mut i = 0;
                while i < m {
                    let rows = MR.min(m - i);
                    let mut acc = [[0.0f32; NR]; MR];
                    for r in 0..rows {
                        acc[r][..cols].copy_from_slice(&out[(i + r) * n_out + j0..(i + r) * n_out + j0 + cols]);
                    }
                    let (wb, _) = block.as_chunks::<NR>();
                    if rows == MR {
                        let row = |r: usize| &x[(i + r) * n_in + k0..(i + r) * n_in + k1];
                        let (x0, x1, x2, x3) = (row(0), row(1), row(2), row(3));
                        let [mut c0, mut c1, mut c2, mut c3] = acc;
                        for ((((w, &a0), &a1), &a2), &a3) in wb.iter().zip(x0).zip(x1).zip(x2).zip(x3) {
                            for c in 0..NR {
                                c0[c] += a0 * w[c];
                                c1[c] += a1 * w[c];
                                c2[c] += a2 * w[c];
                                c3[c] += a3 * w[c];
                            }
                        }
                        acc = [c0, c1, c2, c3];
                    } else {
                        for (r, accr) in acc.iter_mut().enumerate().take(rows) {
                            let xr = &x[(i + r) * n_in + k0..(i + r) * n_in + k1];
                            for (w, &a) in wb.iter().zip(xr) {
                                for c in 0..NR {
                                    accr[c] += a * w[c];
                                }
                            }
                        }
                    }
                    for r in 0..rows {
                        out[(i + r) * n_out + j0..(i + r) * n_out + j0 + cols].copy_from_slice(&acc[r][..cols]);
                    }
                    i += rows;
                }
            }
            k0 = k1;
        }
        for row in out[..m * n_out].chunks_exact_mut(n_out) {
            for (v, b) in row.iter_mut().zip(&self.bias) {
                *v += b;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The documented order, written naively.
    fn naive(x: &[f32], w: &[f32], b: &[f32], m: usize, n_out: usize, n_in: usize) -> Vec<f32> {
        let mut y = vec![0.0f32; m * n_out];
        for i in 0..m {
            for j in 0..n_out {
                let mut acc = 0.0f32;
                for k in 0..n_in {
                    acc += x[i * n_in + k] * w[j * n_in + k];
                }
                y[i * n_out + j] = acc + b[j];
            }
        }
        y
    }

    fn lcg(seed: &mut u64) -> f32 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((*seed >> 40) as f32 / (1u64 << 24) as f32) - 0.5
    }

    #[test]
    fn blocked_kernel_is_bit_identical_to_the_documented_order() {
        let mut s = 7;
        for &(m, n_out, n_in) in &[(1, 1, 1), (3, 5, 7), (4, 8, 256), (9, 13, 300), (17, 1152, 384), (5, 384, 1536)] {
            let x: Vec<f32> = (0..m * n_in).map(|_| lcg(&mut s)).collect();
            let w: Vec<f32> = (0..n_out * n_in).map(|_| lcg(&mut s)).collect();
            let b: Vec<f32> = (0..n_out).map(|_| lcg(&mut s)).collect();
            let want = naive(&x, &w, &b, m, n_out, n_in);
            let mut got = vec![0.0; m * n_out];
            Linear::new(&w, &b, n_out, n_in, false).unwrap().forward(&x, m, &mut got);
            assert!(got.iter().zip(&want).all(|(a, b)| a.to_bits() == b.to_bits()), "f32 {m}x{n_out}x{n_in}");
            // f16 path = the f32 path on f16-rounded weights, bit for bit.
            let wh: Vec<f32> = w.iter().map(|&v| f16_to_f32(f32_to_f16(v))).collect();
            let want = naive(&x, &wh, &b, m, n_out, n_in);
            Linear::new(&w, &b, n_out, n_in, true).unwrap().forward(&x, m, &mut got);
            assert!(got.iter().zip(&want).all(|(a, b)| a.to_bits() == b.to_bits()), "f16 {m}x{n_out}x{n_in}");
        }
        assert!(Linear::new(&[1.0; 6], &[0.0; 2], 2, 2, false).is_err());
    }
}
