// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Scaling and transformation (8.6): the scaling process for transform coefficients (8.6.2 / 8.6.3), the
//! two-stage inverse transform (8.6.4.2: the 4x4 DST-VII for intra luma 4x4, the 4/8/16/32-point DCTs of transMatrix,
//! the intermediate clip to 16 bits) and the transform skip and transquant bypass residuals. Coefficient and residual
//! arrays are nTbS x nTbS, indexed [y * nTbS + x] (x the column).

use alloc::vec;
use alloc::vec::Vec;

use crate::tables::{DST_MATRIX, LEVEL_SCALE, TRANS_MATRIX};

/// 8.6.2 / 8.6.3: TransCoeffLevel -> d (in place), for qP (Qp'Y or Qp'C), `factor` the ScalingFactor (None: m = 16).
pub fn scale(coeff: &mut [i32], log2: u32, qp: i32, bit_depth: u32, factor: Option<&[u8]>) {
    let bd_shift = bit_depth as i32 + log2 as i32 - 5;
    let ls = (LEVEL_SCALE[(qp % 6) as usize] as i64) << (qp / 6);
    let add = 1i64 << (bd_shift - 1);
    for (i, c) in coeff.iter_mut().enumerate() {
        if *c == 0 {
            continue;
        }
        let m = factor.map_or(16, |f| f[i] as i64);
        let v = ((*c as i64) * m * ls + add) >> bd_shift;
        *c = v.clamp(-32768, 32767) as i32;
    }
}

/// The basis function k of the nTbS-point inverse transform at sample n.
#[inline]
fn basis(dst: bool, log2: u32, k: usize, n: usize) -> i32 {
    if dst { DST_MATRIX[k][n] } else { TRANS_MATRIX[k << (5 - log2)][n] as i32 }
}

/// 8.6.4.2: the residual of the scaled coefficients `d` (DST when `dst`: intra luma 4x4).
pub fn inverse_transform(d: &[i32], log2: u32, dst: bool, bit_depth: u32) -> Vec<i32> {
    let n = 1usize << log2;
    // Stage 1: each column (vertical 1-D transform), then (e + 64) >> 7 clipped to 16 bits.
    let mut g = vec![0i32; n * n];
    for x in 0..n {
        let mut nz = false;
        for k in 0..n {
            if d[k * n + x] != 0 {
                nz = true;
                break;
            }
        }
        if !nz {
            continue;
        }
        for y in 0..n {
            let mut e: i64 = 0;
            for k in 0..n {
                let c = d[k * n + x];
                if c != 0 {
                    e += basis(dst, log2, k, y) as i64 * c as i64;
                }
            }
            g[y * n + x] = ((e + 64) >> 7).clamp(-32768, 32767) as i32;
        }
    }
    // Stage 2: each row (horizontal 1-D transform), then the bdShift rounding.
    let bd_shift = 20 - bit_depth as i64;
    let add = 1i64 << (bd_shift - 1);
    let mut r = vec![0i32; n * n];
    for y in 0..n {
        for x in 0..n {
            let mut v: i64 = 0;
            for k in 0..n {
                let c = g[y * n + k];
                if c != 0 {
                    v += basis(dst, log2, k, x) as i64 * c as i64;
                }
            }
            r[y * n + x] = ((v + add) >> bd_shift) as i32;
        }
    }
    r
}

/// 8.6.4.2 with transform_skip_flag: r = d << tsShift (tsShift = 5 + Log2(nTbS)), then the bdShift rounding.
pub fn transform_skip(d: &[i32], log2: u32, bit_depth: u32) -> Vec<i32> {
    let ts_shift = 5 + log2;
    let bd_shift = 20 - bit_depth as i64;
    let add = 1i64 << (bd_shift - 1);
    d.iter().map(|&c| ((((c as i64) << ts_shift) + add) >> bd_shift) as i32).collect()
}

/// Add a residual to the prediction in `plane` at (x0, y0), clipping to the bit depth (8.6.7 picture construction).
pub fn add_residual(plane: &mut crate::picture::Plane, x0: usize, y0: usize, log2: u32, res: &[i32], bit_depth: u32) {
    let n = 1usize << log2;
    let max = (1i32 << bit_depth) - 1;
    for y in 0..n {
        for x in 0..n {
            let v = plane.at(x0 + x, y0 + y) as i32 + res[y * n + x];
            plane.set(x0 + x, y0 + y, v.clamp(0, max) as u16);
        }
    }
}
