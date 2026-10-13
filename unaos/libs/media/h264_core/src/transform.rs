// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Scaling and transformation (8.5): LevelScale4x4 / LevelScale8x8 from the scaling matrices
//! (8.5.9), the Intra_16x16 luma DC Hadamard and its scaling (8.5.10), the chroma DC 2x2 transform
//! and scaling for ChromaArrayType 1 (8.5.11), 4x4 residual scaling and the 4x4 inverse transform
//! (8.5.12), 8x8 scaling and the 8x8 inverse transform (8.5.13), and the picture construction of
//! 8.5.14 (prediction + residual, clipped to 8 bits).
//!
//! Every coefficient array here is raster order within its block, c[i * n + j] = c_ij with i the
//! row and j the column, as the equations of 8.5.12.2 index it.

use crate::ps::ScalingMatrices;
use crate::tables::{NORM_ADJUST4, NORM_ADJUST8, ZIGZAG4, ZIGZAG8};

/// LevelScale4x4(m, i, j) for the six 4x4 lists and LevelScale8x8(m, i, j) for the two 8x8 lists
/// (8.5.9): weightScale (the list through the inverse zig-zag) times normAdjust.
pub struct LevelScale {
    pub l4: [[[i32; 16]; 6]; 6],
    pub l8: [[[i32; 64]; 6]; 2],
}

impl LevelScale {
    pub fn new(m: &ScalingMatrices) -> LevelScale {
        let mut s = LevelScale { l4: [[[0; 16]; 6]; 6], l8: [[[0; 64]; 6]; 2] };
        for list in 0..6 {
            let mut w = [0i32; 16];
            for k in 0..16 {
                w[ZIGZAG4[k] as usize] = m.l4[list][k] as i32;
            }
            for q in 0..6 {
                for p in 0..16 {
                    s.l4[list][q][p] = w[p] * NORM_ADJUST4[q][p] as i32;
                }
            }
        }
        for list in 0..2 {
            let mut w = [0i32; 64];
            for k in 0..64 {
                w[ZIGZAG8[k] as usize] = m.l8[list][k] as i32;
            }
            for q in 0..6 {
                for p in 0..64 {
                    s.l8[list][q][p] = w[p] * NORM_ADJUST8[q][p] as i32;
                }
            }
        }
        s
    }
}

/// 8.5.12.1: scale the 4x4 block `c` in place with LevelScale4x4 `ls` (the list for qP % 6).
/// With `skip_dc` the c_00 entry is left alone (Intra_16x16 luma and chroma blocks, whose DC was
/// produced by 8.5.10 / 8.5.11).
pub fn scale4(c: &mut [i32; 16], ls: &[i32; 16], qp: i32, skip_dc: bool) {
    let q6 = qp / 6;
    for i in (skip_dc as usize)..16 {
        if c[i] == 0 {
            continue;
        }
        c[i] = if qp >= 24 { (c[i] * ls[i]) << (q6 - 4) } else { (c[i] * ls[i] + (1 << (3 - q6))) >> (4 - q6) };
    }
}

/// 8.5.13.1: scale an 8x8 block in place.
pub fn scale8(c: &mut [i32; 64], ls: &[i32; 64], qp: i32) {
    let q6 = qp / 6;
    for i in 0..64 {
        if c[i] == 0 {
            continue;
        }
        c[i] = if qp >= 36 { (c[i] * ls[i]) << (q6 - 6) } else { (c[i] * ls[i] + (1 << (5 - q6))) >> (6 - q6) };
    }
}

/// 8.5.10: the Intra_16x16 DC coefficients c (raster 4x4) -> dcY, the DC of each 4x4 block, in
/// the block's spatial position (dcY_ij belongs to the block in row i, column j). `ls00` is
/// LevelScale4x4(qP % 6, 0, 0) of the intra luma list.
pub fn luma_dc(c: &[i32; 16], ls00: i32, qp: i32) -> [i32; 16] {
    let mut t = [0i32; 16];
    // f = A c A, A = [[1,1,1,1],[1,1,-1,-1],[1,-1,-1,1],[1,-1,1,-1]] (8-320); rows first.
    for i in 0..4 {
        let (a, b, cc, d) = (c[i * 4], c[i * 4 + 1], c[i * 4 + 2], c[i * 4 + 3]);
        t[i * 4] = a + b + cc + d;
        t[i * 4 + 1] = a + b - cc - d;
        t[i * 4 + 2] = a - b - cc + d;
        t[i * 4 + 3] = a - b + cc - d;
    }
    let mut f = [0i32; 16];
    for j in 0..4 {
        let (a, b, cc, d) = (t[j], t[4 + j], t[8 + j], t[12 + j]);
        f[j] = a + b + cc + d;
        f[4 + j] = a + b - cc - d;
        f[8 + j] = a - b - cc + d;
        f[12 + j] = a - b + cc - d;
    }
    let q6 = qp / 6;
    for v in f.iter_mut() {
        *v = if qp >= 36 { (*v * ls00) << (q6 - 6) } else { (*v * ls00 + (1 << (5 - q6))) >> (6 - q6) };
    }
    f
}

/// 8.5.11 for ChromaArrayType 1: c = [[c0, c1], [c2, c3]] -> dcC (same layout), with
/// dcC = ((f * LevelScale4x4(qP % 6, 0, 0)) << (qP / 6)) >> 5.
pub fn chroma_dc(c: &[i32; 4], ls00: i32, qp: i32) -> [i32; 4] {
    let f = [c[0] + c[1] + c[2] + c[3], c[0] - c[1] + c[2] - c[3], c[0] + c[1] - c[2] - c[3], c[0] - c[1] - c[2] + c[3]];
    let q6 = qp / 6;
    let mut d = [0i32; 4];
    for k in 0..4 {
        d[k] = ((f[k] * ls00) << q6) >> 5;
    }
    d
}

/// 8.5.12.2 + 8.5.14: inverse 4x4 transform of the scaled block `d` and add to the prediction
/// already in `dst` (rows of `stride` starting at `off`).
pub fn idct4_add(d: &[i32; 16], dst: &mut [u8], off: usize, stride: usize) {
    let mut f = [0i32; 16];
    for i in 0..4 {
        let r = &d[i * 4..i * 4 + 4];
        let e0 = r[0] + r[2];
        let e1 = r[0] - r[2];
        let e2 = (r[1] >> 1) - r[3];
        let e3 = r[1] + (r[3] >> 1);
        f[i * 4] = e0 + e3;
        f[i * 4 + 1] = e1 + e2;
        f[i * 4 + 2] = e1 - e2;
        f[i * 4 + 3] = e0 - e3;
    }
    for j in 0..4 {
        let g0 = f[j] + f[8 + j];
        let g1 = f[j] - f[8 + j];
        let g2 = (f[4 + j] >> 1) - f[12 + j];
        let g3 = f[4 + j] + (f[12 + j] >> 1);
        let h = [g0 + g3, g1 + g2, g1 - g2, g0 - g3];
        for i in 0..4 {
            let p = off + i * stride + j;
            dst[p] = (dst[p] as i32 + ((h[i] + 32) >> 6)).clamp(0, 255) as u8;
        }
    }
}

/// One 8-point pass of 8.5.13.2 (equations 8-338 .. 8-361).
fn idct8_1d(d: [i32; 8]) -> [i32; 8] {
    let e0 = d[0] + d[4];
    let e1 = -d[3] + d[5] - d[7] - (d[7] >> 1);
    let e2 = d[0] - d[4];
    let e3 = d[1] + d[7] - d[3] - (d[3] >> 1);
    let e4 = (d[2] >> 1) - d[6];
    let e5 = -d[1] + d[7] + d[5] + (d[5] >> 1);
    let e6 = d[2] + (d[6] >> 1);
    let e7 = d[3] + d[5] + d[1] + (d[1] >> 1);
    let f0 = e0 + e6;
    let f1 = e1 + (e7 >> 2);
    let f2 = e2 + e4;
    let f3 = e3 + (e5 >> 2);
    let f4 = e2 - e4;
    let f5 = (e3 >> 2) - e5;
    let f6 = e0 - e6;
    let f7 = e7 - (e1 >> 2);
    [f0 + f7, f2 + f5, f4 + f3, f6 + f1, f6 - f1, f4 - f3, f2 - f5, f0 - f7]
}

/// 8.5.13.2 + 8.5.14: inverse 8x8 transform and add.
pub fn idct8_add(d: &[i32; 64], dst: &mut [u8], off: usize, stride: usize) {
    let mut g = [0i32; 64];
    for i in 0..8 {
        let mut r = [0i32; 8];
        r.copy_from_slice(&d[i * 8..i * 8 + 8]);
        let o = idct8_1d(r);
        g[i * 8..i * 8 + 8].copy_from_slice(&o);
    }
    for j in 0..8 {
        let mut col = [0i32; 8];
        for i in 0..8 {
            col[i] = g[i * 8 + j];
        }
        let m = idct8_1d(col);
        for i in 0..8 {
            let p = off + i * stride + j;
            dst[p] = (dst[p] as i32 + ((m[i] + 32) >> 6)).clamp(0, 255) as u8;
        }
    }
}
