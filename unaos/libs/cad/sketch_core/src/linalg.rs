// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The small dense kernels the solver needs: a row-major matrix and the ORDERED modified Gram-Schmidt rank with
//! re-orthogonalisation (MGS2, "twice is enough") that names a dependent row by insertion order, the solver's own
//! structure. The Cholesky solve of the damped symmetric positive-definite system is geom_core's ONE
//! (`geom_core::linalg::cholesky_solve`, VUG-GEOM-LINALG SR520), re-exported here. Sketch scale (tens to a few hundred
//! parameters) is what dense is for.

use alloc::vec;
use alloc::vec::Vec;

/// A dependent row is one whose re-orthogonalised remainder, after the row was scaled to unit length, is below
/// this. Not a geometric tolerance (BP-vug-2 governs those): it is a relative threshold on unit vectors, ~1e7 ε,
/// well above MGS2's O(ε) loss of orthogonality and well below any real independent direction at sketch scale.
pub const RANK_TOL: f64 = 1e-9;

/// A row is a participant in a dependency when its coefficient in the combination is above this (unit rows).
pub const PARTICIPATION: f64 = 1e-6;

/// A row-major dense matrix.
#[derive(Clone, Debug)]
pub struct Mat {
    pub rows: usize,
    pub cols: usize,
    pub a: Vec<f64>,
}

impl Mat {
    pub fn row(&self, i: usize) -> &[f64] {
        &self.a[i * self.cols..(i + 1) * self.cols]
    }
}

pub fn dot(a: &[f64], b: &[f64]) -> f64 {
    let mut s = 0.0;
    for i in 0..a.len() {
        s += a[i] * b[i];
    }
    s
}

pub fn norm(a: &[f64]) -> f64 {
    libm::sqrt(dot(a, a))
}

/// The rank of a set of rows taken in order.
#[derive(Clone, Debug, Default)]
pub struct Rank {
    /// Orthonormal basis of the row space, one vector per independent row.
    pub q: Vec<Vec<f64>>,
    /// The indices of the independent rows, in order.
    pub indep: Vec<usize>,
    /// The dependent rows: (row index, coefficients over `indep` of the unit-scaled row).
    pub dep: Vec<(usize, Vec<f64>)>,
}

/// Ordered MGS2 over the rows of `m`: each row is scaled to unit length, projected off the basis twice, and is
/// dependent when the remainder is below [`RANK_TOL`]. A zero row is dependent with no participants. Because the
/// order is the rows' order, a dependent set names its LAST member.
pub fn rank(m: &Mat) -> Rank {
    let n = m.cols;
    let mut out = Rank::default();
    // t[j]: q_j expressed over the unit-scaled independent rows (length grows with indep).
    let mut t: Vec<Vec<f64>> = Vec::new();
    for i in 0..m.rows {
        let row = m.row(i);
        let len = norm(row);
        if !(len > 0.0) || !len.is_finite() {
            out.dep.push((i, vec![0.0; out.indep.len()]));
            continue;
        }
        let mut v: Vec<f64> = row.iter().map(|x| x / len).collect();
        let mut c = vec![0.0; out.q.len()];
        for _pass in 0..2 {
            for (j, q) in out.q.iter().enumerate() {
                let p = dot(q, &v);
                c[j] += p;
                for k in 0..n {
                    v[k] -= p * q[k];
                }
            }
        }
        let rho = norm(&v);
        if rho < RANK_TOL {
            // unit row = Σ_j c_j q_j = Σ_j c_j Σ_i t[j][i] row_i
            let mut coef = vec![0.0; out.indep.len()];
            for (j, cj) in c.iter().enumerate() {
                for (ii, tji) in t[j].iter().enumerate() {
                    coef[ii] += cj * tji;
                }
            }
            out.dep.push((i, coef));
        } else {
            // q_new = (row_new - Σ_j c_j q_j) / rho
            let k = out.indep.len();
            let mut tn = vec![0.0; k + 1];
            tn[k] = 1.0 / rho;
            for (j, cj) in c.iter().enumerate() {
                for (ii, tji) in t[j].iter().enumerate() {
                    tn[ii] -= cj * tji / rho;
                }
            }
            for tj in t.iter_mut() {
                tj.push(0.0);
            }
            t.push(tn);
            for x in v.iter_mut() {
                *x /= rho;
            }
            out.q.push(v);
            out.indep.push(i);
        }
    }
    out
}

/// The damped normal-equations solve: geom_core's (VUG-GEOM-LINALG SR520).
pub use geom_core::linalg::cholesky_solve;
