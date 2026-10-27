// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Intra prediction: the luma mode (8.4.2: the three most probable modes), the chroma mode (8.4.3, Table 8-2), and the
//! sample prediction of 8.4.4.2: reference sample availability and substitution (8.4.4.2.2), filtering with strong
//! intra smoothing (8.4.4.2.3), INTRA_PLANAR (8.4.4.2.5), INTRA_DC (8.4.4.2.6 of the 2013 numbering: the DC with its
//! luma edge filter) and INTRA_ANGULAR 2..34 (8.4.4.2.6) with the mode 10 / 26 boundary filters.

use alloc::vec;
use alloc::vec::Vec;

use crate::picture::Picture;
use crate::ps::Geometry;
use crate::tables::{INTRA_HOR_VER_DIST_THRES, INTRA_PRED_ANGLE, INV_ANGLE};

pub const PLANAR: u32 = 0;
pub const DC: u32 = 1;

/// 8.4.2: IntraPredModeY from the left (A) and above (B) candidates (DC when unavailable, not intra or pcm; B is DC
/// when it lies in the CTB row above), prev_intra_luma_pred_flag, mpm_idx and rem_intra_luma_pred_mode.
pub fn luma_mode(cand_a: u32, cand_b: u32, prev_flag: bool, mpm_idx: u32, rem: u32) -> u32 {
    let mut list = if cand_a == cand_b {
        if cand_a < 2 { [PLANAR, DC, 26] } else { [cand_a, 2 + ((cand_a + 29) % 32), 2 + ((cand_a - 2 + 1) % 32)] }
    } else {
        let third = if cand_a != PLANAR && cand_b != PLANAR {
            PLANAR
        } else if cand_a != DC && cand_b != DC {
            DC
        } else {
            26
        };
        [cand_a, cand_b, third]
    };
    if prev_flag {
        return list[mpm_idx as usize];
    }
    list.sort_unstable();
    let mut m = rem;
    for &c in list.iter() {
        if m >= c {
            m += 1;
        }
    }
    m
}

/// 8.4.3, Table 8-2 (ChromaArrayType != 2): IntraPredModeC from intra_chroma_pred_mode and the luma mode X.
pub fn chroma_mode(intra_chroma_pred_mode: u32, luma: u32) -> u32 {
    let m = match intra_chroma_pred_mode {
        0 => PLANAR,
        1 => 26,
        2 => 10,
        3 => DC,
        _ => return luma,
    };
    if m == luma { 34 } else { m }
}

/// Predict the nTbS x nTbS block (nTbS = 1 << log2) of component `c` whose top-left sample is (x0, y0) in that
/// component's plane, writing the prediction into the plane (8.4.4.2).
pub fn predict(pic: &mut Picture, geo: &Geometry, c: usize, x0: usize, y0: usize, log2: u32, mode: u32, strong_smoothing: bool, bit_depth: u32) {
    let n = 1usize << log2;
    let scale = if c == 0 { 1 } else { 2 };
    let (xl, yl) = ((x0 * scale) as i32, (y0 * scale) as i32);
    // refs[0..2n] = p[-1][2n-1 .. 0], refs[2n] = p[-1][-1], refs[2n+1 ..] = p[0 .. 2n-1][-1]: the order of 8.4.4.2.2.
    let total = 4 * n + 1;
    let mut refs = vec![0i32; total];
    let mut avail = vec![false; total];
    let pos = |i: usize| -> (i32, i32) {
        if i < 2 * n {
            (x0 as i32 - 1, y0 as i32 + (2 * n - 1 - i) as i32)
        } else if i == 2 * n {
            (x0 as i32 - 1, y0 as i32 - 1)
        } else {
            (x0 as i32 + (i - 2 * n - 1) as i32, y0 as i32 - 1)
        }
    };
    let mut any = false;
    {
        let plane = &pic.planes[c];
        for i in 0..total {
            let (xs, ys) = pos(i);
            if pic.available(geo, xl, yl, xs * scale as i32, ys * scale as i32) {
                avail[i] = true;
                refs[i] = plane.at(xs as usize, ys as usize) as i32;
                any = true;
            }
        }
    }
    if !any {
        for r in refs.iter_mut() {
            *r = 1 << (bit_depth - 1);
        }
    } else {
        if !avail[0] {
            let first = (1..total).find(|&i| avail[i]).unwrap_or(0);
            refs[0] = refs[first];
        }
        for i in 1..total {
            if !avail[i] {
                refs[i] = refs[i - 1];
            }
        }
    }
    let ni = n as i32;
    // 8.4.4.2.3 filtering (cIdx 0 for 4:2:0).
    if c == 0 && mode != DC && n != 4 {
        let min_dist = (mode as i32 - 26).abs().min((mode as i32 - 10).abs());
        if min_dist > INTRA_HOR_VER_DIST_THRES[(log2 - 3) as usize] {
            let left = |y: i32| refs[(2 * ni - 1 - y) as usize];
            let top = |x: i32| refs[(2 * ni + 1 + x) as usize];
            let corner = refs[2 * n];
            let thr = 1 << (bit_depth - 5);
            let bi = strong_smoothing
                && n == 32
                && (corner + top(2 * ni - 1) - 2 * top(ni - 1)).abs() < thr
                && (corner + left(2 * ni - 1) - 2 * left(ni - 1)).abs() < thr;
            let mut f = refs.clone();
            if bi {
                let (l63, t63) = (left(63), top(63));
                for y in 0..63 {
                    f[(2 * ni - 1 - y) as usize] = ((63 - y) * corner + (y + 1) * l63 + 32) >> 6;
                }
                for x in 0..63 {
                    f[(2 * ni + 1 + x) as usize] = ((63 - x) * corner + (x + 1) * t63 + 32) >> 6;
                }
            } else {
                // In refs order the filter is the [1 2 1] filter of neighbours, ends kept.
                for i in 1..total - 1 {
                    f[i] = (refs[i - 1] + 2 * refs[i] + refs[i + 1] + 2) >> 2;
                }
            }
            refs = f;
        }
    }
    let left = |y: i32| refs[(2 * ni - 1 - y) as usize];
    let top = |x: i32| refs[(2 * ni + 1 + x) as usize];
    let max = (1i32 << bit_depth) - 1;
    let mut pred = vec![0i32; n * n];
    match mode {
        PLANAR => {
            for y in 0..ni {
                for x in 0..ni {
                    pred[(y * ni + x) as usize] =
                        ((ni - 1 - x) * left(y) + (x + 1) * top(ni) + (ni - 1 - y) * top(x) + (y + 1) * left(ni) + ni) >> (log2 + 1);
                }
            }
        }
        DC => {
            let mut s = ni;
            for i in 0..ni {
                s += top(i) + left(i);
            }
            let dc = s >> (log2 + 1);
            for v in pred.iter_mut() {
                *v = dc;
            }
            if c == 0 && n < 32 {
                pred[0] = (left(0) + 2 * dc + top(0) + 2) >> 2;
                for x in 1..ni {
                    pred[x as usize] = (top(x) + 3 * dc + 2) >> 2;
                }
                for y in 1..ni {
                    pred[(y * ni) as usize] = (left(y) + 3 * dc + 2) >> 2;
                }
            }
        }
        _ => {
            let angle = INTRA_PRED_ANGLE[mode as usize];
            // ref[k] stored at r[k + n] for k in -n ..= 2n.
            let mut r: Vec<i32> = vec![0; 3 * n + 1];
            let vertical = mode >= 18;
            let main = |k: i32| if vertical { top(k) } else { left(k) };
            let side = |k: i32| if vertical { left(k) } else { top(k) };
            for k in 0..=ni {
                r[(k + ni) as usize] = main(k - 1);
            }
            if angle < 0 {
                let lo = (ni * angle) >> 5;
                if lo < -1 {
                    let inv = INV_ANGLE[(mode - 11) as usize];
                    for k in lo..=-1 {
                        r[(k + ni) as usize] = side(-1 + ((k * inv + 128) >> 8));
                    }
                }
            } else {
                for k in ni + 1..=2 * ni {
                    r[(k + ni) as usize] = main(k - 1);
                }
            }
            for j in 0..ni {
                let idx = ((j + 1) * angle) >> 5;
                let fact = ((j + 1) * angle) & 31;
                for i in 0..ni {
                    let a = r[(i + idx + 1 + ni) as usize];
                    let v = if fact != 0 { ((32 - fact) * a + fact * r[(i + idx + 2 + ni) as usize] + 16) >> 5 } else { a };
                    // vertical: j is the row (y), i the column (x); horizontal: the transpose.
                    let (x, y) = if vertical { (i, j) } else { (j, i) };
                    pred[(y * ni + x) as usize] = v;
                }
            }
            if c == 0 && n < 32 {
                if mode == 26 {
                    for y in 0..ni {
                        pred[(y * ni) as usize] = (top(0) + ((left(y) - left(-1)) >> 1)).clamp(0, max);
                    }
                } else if mode == 10 {
                    for x in 0..ni {
                        pred[x as usize] = (left(0) + ((top(x) - top(-1)) >> 1)).clamp(0, max);
                    }
                }
            }
        }
    }
    let plane = &mut pic.planes[c];
    for y in 0..n {
        for x in 0..n {
            plane.set(x0 + x, y0 + y, pred[y * n + x] as u8);
        }
    }
}
