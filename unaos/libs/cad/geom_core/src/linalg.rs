// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: the dense kernels the CAD cores share — the symmetric Jacobi eigen, the Cholesky solve and splitmix64
//! (VUG-GEOM-LINALG, LEDGER SR520); sketch_core, photo_core and scan_core carry no private copy.
//!
//! * [`jacobi_sym`]: cyclic Jacobi rotations (Numerical Recipes §11.1) on a small symmetric matrix (n <= 9 is the use:
//!   scan_core's 3 x 3 covariances, photo_core's 9 x 9 DLT AᵀA), the eigenpairs sorted ascending by value.
//! * [`cholesky_solve`]: `a x = b` for symmetric positive-definite `a` (sketch_core's damped normal equations, scan_core
//!   pose.rs's 6x6 ICP step, graph.rs's dense pose graph and separator system).
//! * [`cholesky_profile`] + [`cholesky_substitute`]: the same Cholesky over an envelope (row `i`'s first structurally
//!   non-zero column), scan_core graph.rs's submap interiors (VUGGEOMLINALGSCAN, LEDGER SR540: VUGSCANSUBMAP SR522's
//!   `factor` / `substitute` moved here). The dense solve IS the all-zero profile: one loop, the same flops in the
//!   same order.
//! * [`SplitMix64`]: Steele, Lea & Flood's splitmix64 (OOPSLA 2014; Vigna's splitmix64.c), the deterministic sampler and
//!   noise source; each caller owns its seed, so a measurement is bit-reproducible.
//!
//! Slice-shaped, row-major, pure; never panics on a short slice (a named empty or `false` answer). Mat3 / Mat4 stay the
//! fixed-size types.

use alloc::vec::Vec;

/// Eigen-decomposition of the symmetric n x n `a` (row-major; read, not changed) by cyclic Jacobi rotations: the
/// eigenvalues ascending and the eigenvectors as the COLUMNS of the returned row-major n x n matrix (column j belongs
/// to value j). The order is a stable sort on strict `<`, so column 0 is the first strict minimum of the unsorted
/// diagonal. At most 100 sweeps; a sweep starts only while the off-diagonal mass exceeds `norm · ε²`. A slice shorter
/// than `n · n` yields two empty vectors.
pub fn jacobi_sym(a: &[f64], n: usize) -> (Vec<f64>, Vec<f64>) {
    if a.len() < n * n {
        return (Vec::new(), Vec::new());
    }
    let mut a: Vec<f64> = a[..n * n].to_vec();
    let mut v: Vec<f64> = (0..n * n).map(|k| if k / n == k % n { 1.0 } else { 0.0 }).collect();
    for _ in 0..100 {
        let mut off = 0.0;
        let mut norm = 0.0;
        for p in 0..n {
            for q in 0..n {
                let x = a[p * n + q];
                norm += x * x;
                if p < q {
                    off += x * x;
                }
            }
        }
        if !(off > norm * (f64::EPSILON * f64::EPSILON)) {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                let apq = a[p * n + q];
                if apq == 0.0 {
                    continue;
                }
                let theta = (a[q * n + q] - a[p * n + p]) / (2.0 * apq);
                let t = if theta >= 0.0 {
                    1.0 / (theta + libm::sqrt(theta * theta + 1.0))
                } else {
                    -1.0 / (-theta + libm::sqrt(theta * theta + 1.0))
                };
                let c = 1.0 / libm::sqrt(t * t + 1.0);
                let s = t * c;
                for k in 0..n {
                    let akp = a[k * n + p];
                    let akq = a[k * n + q];
                    a[k * n + p] = c * akp - s * akq;
                    a[k * n + q] = s * akp + c * akq;
                }
                for k in 0..n {
                    let apk = a[p * n + k];
                    let aqk = a[q * n + k];
                    a[p * n + k] = c * apk - s * aqk;
                    a[q * n + k] = s * apk + c * aqk;
                }
                for k in 0..n {
                    let vkp = v[k * n + p];
                    let vkq = v[k * n + q];
                    v[k * n + p] = c * vkp - s * vkq;
                    v[k * n + q] = s * vkp + c * vkq;
                }
            }
        }
    }
    let vals: Vec<f64> = (0..n).map(|i| a[i * n + i]).collect();
    // Stable insertion sort of the column order on strict `<` (an element moves left only past a strictly larger one).
    let mut order: Vec<usize> = (0..n).collect();
    for i in 1..n {
        let mut j = i;
        while j > 0 && vals[order[j]] < vals[order[j - 1]] {
            order.swap(j, j - 1);
            j -= 1;
        }
    }
    let sorted: Vec<f64> = order.iter().map(|&k| vals[k]).collect();
    let mut vecs = Vec::with_capacity(n * n);
    for r in 0..n {
        for &k in &order {
            vecs.push(v[r * n + k]);
        }
    }
    (sorted, vecs)
}

/// Solve `a x = b` for symmetric positive-definite `a` (m x m, row-major, its lower triangle read and overwritten by its
/// Cholesky factor); `b` becomes `x`. False when a pivot is not positive (the caller raises its damping) or a slice is
/// shorter than `m · m` / `m`. The all-zero profile of [`cholesky_profile`] + [`cholesky_substitute`] (SR540).
pub fn cholesky_solve(a: &mut [f64], m: usize, b: &mut [f64]) -> bool {
    if a.len() < m * m || b.len() < m {
        return false;
    }
    if !factor_by(a, m, |_| 0) {
        return false;
    }
    substitute_by(a, m, |_| 0, b);
    true
}

/// The Cholesky factor `L` of the symmetric positive-definite `a` (m x m, row-major, lower triangle read) in place,
/// over the envelope `first[row]` (the row's first structurally non-zero column, `first[i] <= i`; `L` keeps the
/// envelope, so nothing left of it is read or written). False on a pivot that is not positive or a slice shorter
/// than `m · m` / `m`. VUGGEOMLINALGSCAN (SR540): VUGSCANSUBMAP's (SR522) `factor`, moved from scan_core graph.rs.
pub fn cholesky_profile(a: &mut [f64], m: usize, first: &[usize]) -> bool {
    if a.len() < m * m || first.len() < m {
        return false;
    }
    factor_by(a, m, |i| first[i])
}

/// `b` <- `(L·Lᵀ)⁻¹ b` for [`cholesky_profile`]'s `L` over the same envelope (row-form substitutions). False (and `b`
/// untouched) when a slice is shorter than `m · m` / `m`. VUGGEOMLINALGSCAN (SR540): SR522's `substitute`, moved.
pub fn cholesky_substitute(a: &[f64], m: usize, first: &[usize], b: &mut [f64]) -> bool {
    if a.len() < m * m || first.len() < m || b.len() < m {
        return false;
    }
    substitute_by(a, m, |i| first[i], b);
    true
}

/// The one Cholesky loop (SR522's `factor`, the profile read through `first`): the dense call passes `|_| 0`, which
/// skips nothing and starts every sum at column 0 (SR520's dense loop, flop for flop).
fn factor_by(a: &mut [f64], m: usize, first: impl Fn(usize) -> usize) -> bool {
    for j in 0..m {
        let mut d = a[j * m + j];
        for k in first(j)..j {
            d -= a[j * m + k] * a[j * m + k];
        }
        if !(d > 0.0) || !d.is_finite() {
            return false;
        }
        let l = libm::sqrt(d);
        a[j * m + j] = l;
        for i in j + 1..m {
            if first(i) > j {
                continue;
            }
            let mut s = a[i * m + j];
            for k in first(i).max(first(j))..j {
                s -= a[i * m + k] * a[j * m + k];
            }
            a[i * m + j] = s / l;
        }
    }
    true
}

/// The two triangular solves over `factor_by`'s `L` (SR522's `substitute`, the profile read through `first`).
fn substitute_by(a: &[f64], m: usize, first: impl Fn(usize) -> usize, b: &mut [f64]) {
    for i in 0..m {
        let mut s = b[i];
        for k in first(i)..i {
            s -= a[i * m + k] * b[k];
        }
        b[i] = s / a[i * m + i];
    }
    for i in (0..m).rev() {
        let mut s = b[i];
        for k in i + 1..m {
            if first(k) <= i {
                s -= a[k * m + i] * b[k];
            }
        }
        b[i] = s / a[i * m + i];
    }
}

/// splitmix64 (Steele, Lea & Flood 2014): the deterministic sampler and noise source of the CAD cores.
#[derive(Clone, Debug)]
pub struct SplitMix64(u64);

impl SplitMix64 {
    pub fn new(seed: u64) -> SplitMix64 {
        SplitMix64(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1) (the top 53 bits).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// `next_u64() % n`; 0 without drawing when `n == 0`.
    pub fn below(&mut self, n: usize) -> usize {
        if n == 0 {
            return 0;
        }
        (self.next_u64() % n as u64) as usize
    }
}
