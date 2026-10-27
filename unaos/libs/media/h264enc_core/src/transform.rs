// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The forward side of 8.5: the 4x4 forward core transform (the integer transform whose inverse
//! is 8.5.12.2), the 4x4 luma DC and 2x2 chroma DC forward Hadamards, and the forward quantiser
//! with the multiplication factors MF (the encoder-side counterpart of the Recommendation's
//! normAdjust4, Table 8-12 / 8.5.9: MF(m) * normAdjust4(m) * 16 ~= 2^21 for each position class).
//!
//! Reconstruction never re-implements the inverse: [`recon4`], [`recon_i16`] and [`recon_chroma`]
//! call h264_core's `transform::{scale4, luma_dc, chroma_dc, idct4_add}` in the order its
//! decoder's `reconstruct` calls them, so the encoder's reconstruction is the decoder's output.
//!
//! Coefficient arrays are raster within their block, c[i * n + j] with i the row (vertical
//! frequency) and j the column, as h264_core indexes them.

use h264_core::picture::{BLK_X, BLK_Y};
use h264_core::transform::{chroma_dc, idct4_add, luma_dc, scale4, LevelScale};

/// MF(qP % 6, class): class 0 for positions with i and j both even, 1 for both odd, 2 otherwise.
const MF: [[i32; 3]; 6] = [[13107, 5243, 8066], [11916, 4660, 7490], [10082, 4194, 6554], [9362, 3647, 5825], [8192, 3355, 5243], [7282, 2893, 4559]];

#[inline]
fn mf(q: usize, pos: usize) -> i64 {
    let (i, j) = (pos / 4, pos % 4);
    let class = if i % 2 == 0 && j % 2 == 0 {
        0
    } else if i % 2 == 1 && j % 2 == 1 {
        1
    } else {
        2
    };
    MF[q][class] as i64
}

/// The rounding offset f of the quantiser: 2^qbits / 3 for intra blocks, / 6 for inter blocks.
#[inline]
fn offset(qbits: i32, intra: bool) -> i64 {
    (1i64 << qbits) / if intra { 3 } else { 6 }
}

/// Forward 4x4 core transform Y = Cf X CfT, Cf = [[1,1,1,1],[2,1,-1,-2],[1,-1,-1,1],[1,-2,2,-1]].
pub fn fdct4(x: &[i32; 16]) -> [i32; 16] {
    let mut t = [0i32; 16];
    for i in 0..4 {
        let r = &x[i * 4..i * 4 + 4];
        let (s03, d03, s12, d12) = (r[0] + r[3], r[0] - r[3], r[1] + r[2], r[1] - r[2]);
        t[i * 4] = s03 + s12;
        t[i * 4 + 1] = 2 * d03 + d12;
        t[i * 4 + 2] = s03 - s12;
        t[i * 4 + 3] = d03 - 2 * d12;
    }
    let mut y = [0i32; 16];
    for j in 0..4 {
        let (a, b, c, d) = (t[j], t[4 + j], t[8 + j], t[12 + j]);
        let (s03, d03, s12, d12) = (a + d, a - d, b + c, b - c);
        y[j] = s03 + s12;
        y[4 + j] = 2 * d03 + d12;
        y[8 + j] = s03 - s12;
        y[12 + j] = d03 - 2 * d12;
    }
    y
}

/// The forward luma DC transform of Intra_16x16: (H W H) / 2 over the 4x4 array of block DCs
/// (W[i * 4 + j] = DC of the block in row i, column j), H the 4x4 Hadamard h264_core's `luma_dc`
/// inverts.
pub fn hadamard4(w: &[i32; 16]) -> [i32; 16] {
    let mut t = [0i32; 16];
    for i in 0..4 {
        let (a, b, c, d) = (w[i * 4], w[i * 4 + 1], w[i * 4 + 2], w[i * 4 + 3]);
        t[i * 4] = a + b + c + d;
        t[i * 4 + 1] = a + b - c - d;
        t[i * 4 + 2] = a - b - c + d;
        t[i * 4 + 3] = a - b + c - d;
    }
    let mut y = [0i32; 16];
    for j in 0..4 {
        let (a, b, c, d) = (t[j], t[4 + j], t[8 + j], t[12 + j]);
        for (k, v) in [a + b + c + d, a + b - c - d, a - b - c + d, a - b + c - d].into_iter().enumerate() {
            y[k * 4 + j] = v.signum() * ((v.abs() + 1) >> 1);
        }
    }
    y
}

/// The forward chroma DC transform (4:2:0): H2 W H2 over [[c00, c01], [c10, c11]] in the order
/// h264_core's `chroma_dc` inverts (c0 c1 c2 c3 = top-left, top-right, bottom-left, bottom-right).
pub fn hadamard2(c: &[i32; 4]) -> [i32; 4] {
    [c[0] + c[1] + c[2] + c[3], c[0] - c[1] + c[2] - c[3], c[0] + c[1] - c[2] - c[3], c[0] - c[1] - c[2] + c[3]]
}

/// Quantise a transformed 4x4 block in place: Z = sign(Y) * ((|Y| * MF + f) >> (15 + qP / 6)).
/// With `skip_dc` the DC position is cleared (Intra_16x16 and chroma blocks code it separately).
pub fn quant4(c: &mut [i32; 16], qp: i32, intra: bool, skip_dc: bool) {
    let qbits = 15 + qp / 6;
    let f = offset(qbits, intra);
    let q = (qp % 6) as usize;
    if skip_dc {
        c[0] = 0;
    }
    for p in (skip_dc as usize)..16 {
        let v = c[p] as i64;
        let z = ((v.abs() * mf(q, p) + f) >> qbits) as i32;
        c[p] = if v < 0 { -z } else { z };
    }
}

/// Quantise transformed DC coefficients (the 16 luma DCs or the 4 chroma DCs) in place:
/// Z = sign(Y) * ((|Y| * MF(qP % 6, 0) + 2f) >> (16 + qP / 6)).
pub fn quant_dc(c: &mut [i32], qp: i32, intra: bool) {
    let qbits = 15 + qp / 6;
    let f = offset(qbits, intra);
    let m = mf((qp % 6) as usize, 0);
    for v in c.iter_mut() {
        let a = *v as i64;
        let z = ((a.abs() * m + 2 * f) >> (qbits + 1)) as i32;
        *v = if a < 0 { -z } else { z };
    }
}

/// The 16 forward-transformed 4x4 blocks of a 16x16 residual (row stride 16), by luma4x4BlkIdx.
pub fn fdct16(res: &[i32; 256]) -> [[i32; 16]; 16] {
    let mut out = [[0i32; 16]; 16];
    for blk in 0..16 {
        let (bx, by) = (BLK_X[blk] * 4, BLK_Y[blk] * 4);
        let mut x = [0i32; 16];
        for i in 0..4 {
            for j in 0..4 {
                x[i * 4 + j] = res[(by + i) * 16 + bx + j];
            }
        }
        out[blk] = fdct4(&x);
    }
    out
}

/// Code one 8x8 chroma residual (row stride 8): the four 4x4 transforms (chroma4x4BlkIdx raster),
/// their DCs through [`hadamard2`] and [`quant_dc`], the AC through [`quant4`]. Returns the DC
/// levels (c0..c3) and the AC levels (raster, [0] = 0) of the four blocks.
pub fn code_chroma(res: &[i32; 64], qpc: i32, intra: bool) -> ([i32; 4], [[i32; 16]; 4]) {
    let mut ac = [[0i32; 16]; 4];
    let mut w = [0i32; 4];
    for blk in 0..4 {
        let (bx, by) = ((blk % 2) * 4, (blk / 2) * 4);
        let mut x = [0i32; 16];
        for i in 0..4 {
            for j in 0..4 {
                x[i * 4 + j] = res[(by + i) * 8 + bx + j];
            }
        }
        ac[blk] = fdct4(&x);
        w[blk] = ac[blk][0];
    }
    let mut dc = hadamard2(&w);
    quant_dc(&mut dc, qpc, intra);
    for b in ac.iter_mut() {
        quant4(b, qpc, intra, true);
    }
    (dc, ac)
}

/// Add the residual of one 4x4 block of levels `c` (raster) to the prediction already in `dst`:
/// 8.5.12.1 scaling with `ls` (the LevelScale4x4 list for qP % 6) and the inverse transform —
/// h264_core's `scale4` + `idct4_add`, skipped as the decoder skips a block with no coefficients.
pub fn recon4(c: &[i32; 16], ls: &[i32; 16], qp: i32, dst: &mut [u8], off: usize, stride: usize) {
    if c.iter().all(|&v| v == 0) {
        return;
    }
    let mut d = *c;
    scale4(&mut d, ls, qp, false);
    idct4_add(&d, dst, off, stride);
}

/// The Intra_16x16 residual (8.5.10 then 8.5.12) of DC levels `dc` (raster) and AC levels `ac`
/// (by luma4x4BlkIdx, raster, [0] unused) onto the 16x16 prediction at `off` — the decoder's
/// sequence: `luma_dc` when a DC level is non-zero, each block's DC placed, `scale4` without
/// the DC, `idct4_add`.
pub fn recon_i16(dc: &[i32; 16], ac: &[[i32; 16]; 16], ls: &LevelScale, qp: i32, dst: &mut [u8], off: usize, stride: usize) {
    let q6 = (qp % 6) as usize;
    let dcv = if dc.iter().any(|&v| v != 0) { luma_dc(dc, ls.l4[0][q6][0], qp) } else { [0; 16] };
    for blk in 0..16 {
        let (bx, by) = (BLK_X[blk], BLK_Y[blk]);
        let mut c = ac[blk];
        c[0] = dcv[by * 4 + bx];
        if c.iter().all(|&v| v == 0) {
            continue;
        }
        scale4(&mut c, &ls.l4[0][q6], qp, true);
        idct4_add(&c, dst, off + by * 4 * stride + bx * 4, stride);
    }
}

/// The chroma residual (8.5.11 then 8.5.12) of one component onto its 8x8 prediction at `off`,
/// with LevelScale list `list` (1 / 2 intra Cb / Cr, 4 / 5 inter) at chroma QP `qpc` — the
/// decoder's sequence; called only when CodedBlockPatternChroma is non-zero, as it is.
pub fn recon_chroma(dc: &[i32; 4], ac: &[[i32; 16]; 4], ls: &LevelScale, list: usize, qpc: i32, dst: &mut [u8], off: usize, stride: usize) {
    let q6 = (qpc % 6) as usize;
    let d = chroma_dc(dc, ls.l4[list][q6][0], qpc);
    for blk in 0..4 {
        let mut co = ac[blk];
        co[0] = d[blk];
        if co.iter().all(|&v| v == 0) {
            continue;
        }
        scale4(&mut co, &ls.l4[list][q6], qpc, true);
        let (bx, by) = ((blk % 2) * 4, (blk / 2) * 4);
        idct4_add(&co, dst, off + by * stride + bx, stride);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use h264_core::ps::ScalingMatrices;

    /// Forward transform + quantiser + h264_core's inverse lands within the quantiser's step.
    #[test]
    fn forward_then_shared_inverse_is_near_identity() {
        let ls = LevelScale::new(&ScalingMatrices::FLAT);
        let mut x = [0i32; 16];
        for (k, v) in x.iter_mut().enumerate() {
            *v = ((k * 37) % 23) as i32 - 11;
        }
        for qp in [16, 26, 40] {
            let mut c = fdct4(&x);
            quant4(&mut c, qp, true, false);
            let mut dst = [128u8; 16];
            recon4(&c, &ls.l4[0][(qp % 6) as usize], qp, &mut dst, 0, 4);
            let step = [4, 13, 52][[16, 26, 40].iter().position(|&q| q == qp).unwrap()];
            for k in 0..16 {
                let e = (dst[k] as i32 - 128 - x[k]).abs();
                assert!(e <= step, "qp {qp} k {k}: {e}");
            }
        }
    }
}
