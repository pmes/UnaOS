// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Intra prediction sample processes (8.3): Intra_4x4 (8.3.1.2.1 .. 8.3.1.2.9), Intra_8x8 with the
//! reference sample filtering of 8.3.2.2.1 (8.3.2.2.2 .. 8.3.2.2.10), Intra_16x16 (8.3.3) and
//! chroma for ChromaArrayType 1 (8.3.4).
//!
//! The caller gathers the neighbouring samples p[x, -1], p[-1, y] and p[-1, -1] with their
//! availability (8.3.1.2: outside the picture, in another slice, not yet decoded, or inter under
//! constrained_intra_pred_flag) and performs the top-right substitution (p[3, -1] / p[7, -1] copied
//! when the samples to its right are not available).

/// Neighbouring samples of an n x n block: `top[x]` = p[x, -1] for x in 0..2n, `left[y]` =
/// p[-1, y] for y in 0..n, `tl` = p[-1, -1].
pub struct Edges {
    pub top: [i32; 16],
    pub left: [i32; 16],
    pub tl: i32,
    pub has_top: bool,
    pub has_left: bool,
    pub has_tl: bool,
}

impl Edges {
    pub fn new() -> Edges {
        Edges { top: [128; 16], left: [128; 16], tl: 128, has_top: false, has_left: false, has_tl: false }
    }

    /// p[x, y] with x or y equal to -1.
    #[inline]
    fn p(&self, x: i32, y: i32) -> i32 {
        if y < 0 {
            if x < 0 { self.tl } else { self.top[x as usize] }
        } else {
            self.left[y as usize]
        }
    }
}

impl Default for Edges {
    fn default() -> Self {
        Edges::new()
    }
}

/// 8.3.2.2.1: the filtered reference samples p' of an Intra_8x8 block (top has 16 entries, the
/// right half already substituted when not available).
pub fn filter_8x8_edges(e: &Edges) -> Edges {
    let mut f = Edges { top: e.top, left: e.left, tl: e.tl, has_top: e.has_top, has_left: e.has_left, has_tl: e.has_tl };
    if e.has_top {
        f.top[0] = if e.has_tl { (e.tl + 2 * e.top[0] + e.top[1] + 2) >> 2 } else { (3 * e.top[0] + e.top[1] + 2) >> 2 };
        for x in 1..15 {
            f.top[x] = (e.top[x - 1] + 2 * e.top[x] + e.top[x + 1] + 2) >> 2;
        }
        f.top[15] = (e.top[14] + 3 * e.top[15] + 2) >> 2;
    }
    if e.has_tl {
        f.tl = match (e.has_top, e.has_left) {
            (true, true) => (e.top[0] + 2 * e.tl + e.left[0] + 2) >> 2,
            (true, false) => (3 * e.tl + e.top[0] + 2) >> 2,
            (false, true) => (3 * e.tl + e.left[0] + 2) >> 2,
            (false, false) => e.tl,
        };
    }
    if e.has_left {
        f.left[0] = if e.has_tl { (e.tl + 2 * e.left[0] + e.left[1] + 2) >> 2 } else { (3 * e.left[0] + e.left[1] + 2) >> 2 };
        for y in 1..7 {
            f.left[y] = (e.left[y - 1] + 2 * e.left[y] + e.left[y + 1] + 2) >> 2;
        }
        f.left[7] = (e.left[6] + 3 * e.left[7] + 2) >> 2;
    }
    f
}

/// Intra_4x4 (n = 4) or Intra_8x8 (n = 8, filtered edges) prediction of `mode` (Table 8-2 /
/// 8-3: 0 Vertical, 1 Horizontal, 2 DC, 3 Diagonal_Down_Left, 4 Diagonal_Down_Right,
/// 5 Vertical_Right, 6 Horizontal_Down, 7 Vertical_Left, 8 Horizontal_Up) into `out` (n x n).
pub fn pred_nxn(n: usize, mode: u8, e: &Edges, out: &mut [i32]) {
    let ni = n as i32;
    let log2n = if n == 4 { 2 } else { 3 };
    for y in 0..ni {
        for x in 0..ni {
            let p = |xx: i32, yy: i32| e.p(xx, yy);
            let v = match mode {
                0 => p(x, -1),
                1 => p(-1, y),
                2 => {
                    let st: i32 = (0..ni).map(|i| p(i, -1)).sum();
                    let sl: i32 = (0..ni).map(|i| p(-1, i)).sum();
                    match (e.has_top, e.has_left) {
                        (true, true) => (st + sl + ni) >> (log2n + 1),
                        (false, true) => (sl + (ni >> 1)) >> log2n,
                        (true, false) => (st + (ni >> 1)) >> log2n,
                        (false, false) => 128,
                    }
                }
                3 => {
                    if x == ni - 1 && y == ni - 1 {
                        (p(2 * ni - 2, -1) + 3 * p(2 * ni - 1, -1) + 2) >> 2
                    } else {
                        (p(x + y, -1) + 2 * p(x + y + 1, -1) + p(x + y + 2, -1) + 2) >> 2
                    }
                }
                4 => {
                    if x > y {
                        (p(x - y - 2, -1) + 2 * p(x - y - 1, -1) + p(x - y, -1) + 2) >> 2
                    } else if x < y {
                        (p(-1, y - x - 2) + 2 * p(-1, y - x - 1) + p(-1, y - x) + 2) >> 2
                    } else {
                        (p(0, -1) + 2 * p(-1, -1) + p(-1, 0) + 2) >> 2
                    }
                }
                5 => {
                    let z = 2 * x - y;
                    if z >= 0 && z & 1 == 0 {
                        (p(x - (y >> 1) - 1, -1) + p(x - (y >> 1), -1) + 1) >> 1
                    } else if z >= 0 {
                        (p(x - (y >> 1) - 2, -1) + 2 * p(x - (y >> 1) - 1, -1) + p(x - (y >> 1), -1) + 2) >> 2
                    } else if z == -1 {
                        (p(-1, 0) + 2 * p(-1, -1) + p(0, -1) + 2) >> 2
                    } else {
                        (p(-1, y - 2 * x - 1) + 2 * p(-1, y - 2 * x - 2) + p(-1, y - 2 * x - 3) + 2) >> 2
                    }
                }
                6 => {
                    let z = 2 * y - x;
                    if z >= 0 && z & 1 == 0 {
                        (p(-1, y - (x >> 1) - 1) + p(-1, y - (x >> 1)) + 1) >> 1
                    } else if z >= 0 {
                        (p(-1, y - (x >> 1) - 2) + 2 * p(-1, y - (x >> 1) - 1) + p(-1, y - (x >> 1)) + 2) >> 2
                    } else if z == -1 {
                        (p(-1, 0) + 2 * p(-1, -1) + p(0, -1) + 2) >> 2
                    } else {
                        (p(x - 2 * y - 1, -1) + 2 * p(x - 2 * y - 2, -1) + p(x - 2 * y - 3, -1) + 2) >> 2
                    }
                }
                7 => {
                    if y & 1 == 0 {
                        (p(x + (y >> 1), -1) + p(x + (y >> 1) + 1, -1) + 1) >> 1
                    } else {
                        (p(x + (y >> 1), -1) + 2 * p(x + (y >> 1) + 1, -1) + p(x + (y >> 1) + 2, -1) + 2) >> 2
                    }
                }
                _ => {
                    let z = x + 2 * y;
                    let zmax = 2 * ni - 3;
                    if z < zmax && z & 1 == 0 {
                        (p(-1, y + (x >> 1)) + p(-1, y + (x >> 1) + 1) + 1) >> 1
                    } else if z < zmax {
                        (p(-1, y + (x >> 1)) + 2 * p(-1, y + (x >> 1) + 1) + p(-1, y + (x >> 1) + 2) + 2) >> 2
                    } else if z == zmax {
                        (p(-1, ni - 2) + 3 * p(-1, ni - 1) + 2) >> 2
                    } else {
                        p(-1, ni - 1)
                    }
                }
            };
            out[(y * ni + x) as usize] = v;
        }
    }
}

/// Intra_16x16 (8.3.3; Table 8-4: 0 Vertical, 1 Horizontal, 2 DC, 3 Plane) into `out` (16x16).
pub fn pred_16x16(mode: u8, e: &Edges, out: &mut [i32; 256]) {
    match mode {
        0 => {
            for y in 0..16 {
                for x in 0..16 {
                    out[y * 16 + x] = e.top[x];
                }
            }
        }
        1 => {
            for y in 0..16 {
                for x in 0..16 {
                    out[y * 16 + x] = e.left[y];
                }
            }
        }
        2 => {
            let st: i32 = e.top.iter().sum();
            let sl: i32 = e.left.iter().sum();
            let v = match (e.has_top, e.has_left) {
                (true, true) => (st + sl + 16) >> 5,
                (false, true) => (sl + 8) >> 4,
                (true, false) => (st + 8) >> 4,
                (false, false) => 128,
            };
            out.fill(v);
        }
        _ => {
            let p = |x: i32, y: i32| e.p(x, y);
            let mut h = 0;
            let mut v = 0;
            for i in 0..8 {
                h += (i + 1) * (p(8 + i, -1) - p(6 - i, -1));
                v += (i + 1) * (p(-1, 8 + i) - p(-1, 6 - i));
            }
            let a = 16 * (p(-1, 15) + p(15, -1));
            let b = (5 * h + 32) >> 6;
            let c = (5 * v + 32) >> 6;
            for y in 0..16 {
                for x in 0..16 {
                    out[(y * 16 + x) as usize] = ((a + b * (x - 7) + c * (y - 7) + 16) >> 5).clamp(0, 255);
                }
            }
        }
    }
}

/// Chroma intra prediction for ChromaArrayType 1 (8.3.4; Table 8-5: 0 DC, 1 Horizontal,
/// 2 Vertical, 3 Plane) of one 8x8 chroma block into `out`.
pub fn pred_chroma(mode: u8, e: &Edges, out: &mut [i32; 64]) {
    match mode {
        0 => {
            // 8.3.4.1 .. 8.3.4.3: per 4x4 chroma block, by its position.
            for blk in 0..4 {
                let (xo, yo) = ((blk % 2) * 4, (blk / 2) * 4);
                let st: i32 = (0..4).map(|i| e.top[xo + i]).sum();
                let sl: i32 = (0..4).map(|i| e.left[yo + i]).sum();
                let (t, l) = (e.has_top, e.has_left);
                let v = if (xo == 0 && yo == 0) || (xo > 0 && yo > 0) {
                    match (t, l) {
                        (true, true) => (st + sl + 4) >> 3,
                        (false, true) => (sl + 2) >> 2,
                        (true, false) => (st + 2) >> 2,
                        _ => 128,
                    }
                } else if xo > 0 {
                    if t {
                        (st + 2) >> 2
                    } else if l {
                        (sl + 2) >> 2
                    } else {
                        128
                    }
                } else if l {
                    (sl + 2) >> 2
                } else if t {
                    (st + 2) >> 2
                } else {
                    128
                };
                for y in 0..4 {
                    for x in 0..4 {
                        out[(yo + y) * 8 + xo + x] = v;
                    }
                }
            }
        }
        1 => {
            for y in 0..8 {
                for x in 0..8 {
                    out[y * 8 + x] = e.left[y];
                }
            }
        }
        2 => {
            for y in 0..8 {
                for x in 0..8 {
                    out[y * 8 + x] = e.top[x];
                }
            }
        }
        _ => {
            let p = |x: i32, y: i32| e.p(x, y);
            let mut h = 0;
            let mut v = 0;
            for i in 0..4 {
                h += (i + 1) * (p(4 + i, -1) - p(2 - i, -1));
                v += (i + 1) * (p(-1, 4 + i) - p(-1, 2 - i));
            }
            let a = 16 * (p(-1, 7) + p(7, -1));
            let b = (34 * h + 32) >> 6;
            let c = (34 * v + 32) >> 6;
            for y in 0..8 {
                for x in 0..8 {
                    out[(y * 8 + x) as usize] = ((a + b * (x - 3) + c * (y - 3) + 16) >> 5).clamp(0, 255);
                }
            }
        }
    }
}
