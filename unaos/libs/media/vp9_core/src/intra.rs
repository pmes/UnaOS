// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Intra prediction (spec §8.5.1): edge construction — left column, above row, above-right — with
//! the base±1 fill when an edge is unavailable and replication past the decoded frame edge, then the
//! ten predictors (DC with its left-only / above-only / neither variants, V, H, the six directional
//! modes, TM). The 4x4 D45 and D63 predictors read the true above-right samples and differ from the
//! generic form, exactly as in the reference decoder.

use crate::consts::*;

const NEED_LEFT: u8 = 1;
const NEED_ABOVE: u8 = 2;
const NEED_ABOVERIGHT: u8 = 4;

const EXTEND_MODES: [u8; 10] = [
    NEED_ABOVE | NEED_LEFT, // DC
    NEED_ABOVE,             // V
    NEED_LEFT,              // H
    NEED_ABOVERIGHT,        // D45
    NEED_LEFT | NEED_ABOVE, // D135
    NEED_LEFT | NEED_ABOVE, // D117
    NEED_LEFT | NEED_ABOVE, // D153
    NEED_LEFT,              // D207
    NEED_ABOVERIGHT,        // D63
    NEED_LEFT | NEED_ABOVE, // TM
];

/// Where the block sits and what is available around it.
pub struct IntraEdge {
    /// Top-left sample of the transform block in the plane.
    pub x0: usize,
    pub y0: usize,
    pub have_top: bool,
    pub have_left: bool,
    pub have_right: bool,
    /// The plane's 8-aligned decoded size.
    pub frame_w: usize,
    pub frame_h: usize,
    /// The block extends past the right / bottom of the mode-info grid (mb_to_right_edge < 0).
    pub right_overhang: bool,
    pub bottom_overhang: bool,
}

/// Predict a `4 << tx_size` square into `buf` at (e.x0, e.y0).
pub fn predict(buf: &mut [u16], stride: usize, e: &IntraEdge, tx_size: u8, mode: u8, bit_depth: u8) {
    let bs = 4usize << tx_size;
    let base: i32 = 128 << (bit_depth - 8);
    let at = |x: usize, y: usize| y * stride + x;
    let mut left = [0i32; 32];
    // above[16] is above_row[0]; above[15] is the top-left sample.
    let mut above = [0i32; 64 + 16];
    const A: usize = 16;
    let need = EXTEND_MODES[mode as usize];
    let (x0, y0) = (e.x0, e.y0);

    if need & NEED_LEFT != 0 {
        if e.have_left {
            let rows = if e.bottom_overhang && y0 + bs > e.frame_h { e.frame_h.saturating_sub(y0).max(1) } else { bs };
            for i in 0..bs {
                let r = i.min(rows - 1);
                left[i] = buf[at(x0 - 1, y0 + r)] as i32;
            }
        } else {
            left[..bs].fill(base + 1);
        }
    }
    let top_left = |buf: &[u16]| if e.have_left { buf[at(x0 - 1, y0 - 1)] as i32 } else { base + 1 };
    if need & NEED_ABOVE != 0 {
        if e.have_top {
            let n = if e.right_overhang && x0 + bs > e.frame_w { e.frame_w - x0 } else { bs };
            for i in 0..bs {
                above[A + i] = buf[at(x0 + i.min(n - 1), y0 - 1)] as i32;
            }
            above[A - 1] = top_left(buf);
        } else {
            above[A..A + bs].fill(base - 1);
            above[A - 1] = base - 1;
        }
    }
    if need & NEED_ABOVERIGHT != 0 {
        if e.have_top {
            // How many real samples the above row offers: 2*bs only for a 4x4 with its right
            // neighbour decoded; never past the decoded frame edge.
            let mut n = if bs == 4 && e.have_right { 2 * bs } else { bs };
            if e.right_overhang {
                if x0 + 2 * bs <= e.frame_w {
                    // n stays
                } else if x0 + bs <= e.frame_w {
                    if bs == 4 && e.have_right {
                        n = e.frame_w - x0;
                    }
                } else {
                    n = e.frame_w - x0;
                }
            }
            for i in 0..2 * bs {
                above[A + i] = buf[at(x0 + i.min(n - 1), y0 - 1)] as i32;
            }
            above[A - 1] = top_left(buf);
        } else {
            above[A..A + 2 * bs].fill(base - 1);
            above[A - 1] = base - 1;
        }
    }

    let max = (1i32 << bit_depth) - 1;
    let ab = |i: isize| above[(A as isize + i) as usize];
    let avg2 = |a: i32, b: i32| (a + b + 1) >> 1;
    let avg3 = |a: i32, b: i32, c: i32| (a + 2 * b + c + 2) >> 2;
    let mut put = |x: usize, y: usize, v: i32| buf[at(x0 + x, y0 + y)] = v as u16;

    match mode {
        DC_PRED => {
            let v = match (e.have_left, e.have_top) {
                (true, true) => {
                    let s: i32 = left[..bs].iter().sum::<i32>() + above[A..A + bs].iter().sum::<i32>();
                    (s + bs as i32) / (2 * bs as i32)
                }
                (true, false) => (left[..bs].iter().sum::<i32>() + (bs as i32 >> 1)) / bs as i32,
                (false, true) => (above[A..A + bs].iter().sum::<i32>() + (bs as i32 >> 1)) / bs as i32,
                (false, false) => base,
            };
            for y in 0..bs {
                for x in 0..bs {
                    put(x, y, v);
                }
            }
        }
        V_PRED => {
            for y in 0..bs {
                for x in 0..bs {
                    put(x, y, above[A + x]);
                }
            }
        }
        H_PRED => {
            for y in 0..bs {
                for x in 0..bs {
                    put(x, y, left[y]);
                }
            }
        }
        TM_PRED => {
            let tl = above[A - 1];
            for y in 0..bs {
                for x in 0..bs {
                    put(x, y, (left[y] + above[A + x] - tl).clamp(0, max));
                }
            }
        }
        D45_PRED => {
            if bs == 4 {
                let a = |i: usize| above[A + i];
                let row = [
                    avg3(a(0), a(1), a(2)),
                    avg3(a(1), a(2), a(3)),
                    avg3(a(2), a(3), a(4)),
                    avg3(a(3), a(4), a(5)),
                    avg3(a(4), a(5), a(6)),
                    avg3(a(5), a(6), a(7)),
                    a(7),
                ];
                for y in 0..4 {
                    for x in 0..4 {
                        put(x, y, row[x + y]);
                    }
                }
            } else {
                let ar = above[A + bs - 1];
                let mut row0 = [0i32; 32];
                for x in 0..bs - 1 {
                    row0[x] = avg3(above[A + x], above[A + x + 1], above[A + x + 2]);
                }
                row0[bs - 1] = ar;
                for y in 0..bs {
                    for x in 0..bs {
                        let v = if x + y < bs { row0[x + y] } else { ar };
                        put(x, y, v);
                    }
                }
            }
        }
        D63_PRED => {
            if bs == 4 {
                let a = |i: usize| above[A + i];
                let r0 = [avg2(a(0), a(1)), avg2(a(1), a(2)), avg2(a(2), a(3)), avg2(a(3), a(4))];
                let r1 = [avg3(a(0), a(1), a(2)), avg3(a(1), a(2), a(3)), avg3(a(2), a(3), a(4)), avg3(a(3), a(4), a(5))];
                for x in 0..4 {
                    put(x, 0, r0[x]);
                    put(x, 1, r1[x]);
                }
                put(0, 2, r0[1]);
                put(1, 2, r0[2]);
                put(2, 2, r0[3]);
                put(3, 2, avg2(a(4), a(5)));
                put(0, 3, r1[1]);
                put(1, 3, r1[2]);
                put(2, 3, r1[3]);
                put(3, 3, avg3(a(4), a(5), a(6)));
            } else {
                let mut r0 = [0i32; 32];
                let mut r1 = [0i32; 32];
                for c in 0..bs {
                    r0[c] = avg2(above[A + c], above[A + c + 1]);
                    r1[c] = avg3(above[A + c], above[A + c + 1], above[A + c + 2]);
                }
                let fill = above[A + bs - 1];
                for c in 0..bs {
                    put(c, 0, r0[c]);
                    put(c, 1, r1[c]);
                }
                let mut size = bs - 2;
                let mut r = 2;
                while r < bs {
                    for c in 0..bs {
                        let (v0, v1) = if c < size { (r0[(r >> 1) + c], r1[(r >> 1) + c]) } else { (fill, fill) };
                        put(c, r, v0);
                        put(c, r + 1, v1);
                    }
                    r += 2;
                    size -= 1;
                }
            }
        }
        D207_PRED => {
            // Built in place column by column as the reference does.
            let mut d = [[0i32; 32]; 32];
            for r in 0..bs - 1 {
                d[r][0] = avg2(left[r], left[r + 1]);
            }
            d[bs - 1][0] = left[bs - 1];
            for r in 0..bs - 2 {
                d[r][1] = avg3(left[r], left[r + 1], left[r + 2]);
            }
            d[bs - 2][1] = avg3(left[bs - 2], left[bs - 1], left[bs - 1]);
            d[bs - 1][1] = left[bs - 1];
            for c in 0..bs - 2 {
                d[bs - 1][2 + c] = left[bs - 1];
            }
            for r in (0..bs - 1).rev() {
                for c in 0..bs - 2 {
                    d[r][2 + c] = d[r + 1][c];
                }
            }
            for y in 0..bs {
                for x in 0..bs {
                    put(x, y, d[y][x]);
                }
            }
        }
        D117_PRED => {
            let mut d = [[0i32; 32]; 32];
            for c in 0..bs {
                d[0][c] = avg2(ab(c as isize - 1), ab(c as isize));
            }
            d[1][0] = avg3(left[0], ab(-1), ab(0));
            for c in 1..bs {
                d[1][c] = avg3(ab(c as isize - 2), ab(c as isize - 1), ab(c as isize));
            }
            d[2][0] = avg3(ab(-1), left[0], left[1]);
            for r in 3..bs {
                d[r][0] = avg3(left[r - 3], left[r - 2], left[r - 1]);
            }
            for r in 2..bs {
                for c in 1..bs {
                    d[r][c] = d[r - 2][c - 1];
                }
            }
            for y in 0..bs {
                for x in 0..bs {
                    put(x, y, d[y][x]);
                }
            }
        }
        D135_PRED => {
            let mut border = [0i32; 64];
            for i in 0..bs - 2 {
                border[i] = avg3(left[bs - 3 - i], left[bs - 2 - i], left[bs - 1 - i]);
            }
            border[bs - 2] = avg3(ab(-1), left[0], left[1]);
            border[bs - 1] = avg3(left[0], ab(-1), ab(0));
            border[bs] = avg3(ab(-1), ab(0), ab(1));
            for i in 0..bs - 2 {
                border[bs + 1 + i] = avg3(above[A + i], above[A + i + 1], above[A + i + 2]);
            }
            for y in 0..bs {
                for x in 0..bs {
                    put(x, y, border[bs - 1 - y + x]);
                }
            }
        }
        D153_PRED => {
            let mut d = [[0i32; 32]; 32];
            d[0][0] = avg2(ab(-1), left[0]);
            for r in 1..bs {
                d[r][0] = avg2(left[r - 1], left[r]);
            }
            d[0][1] = avg3(left[0], ab(-1), ab(0));
            d[1][1] = avg3(ab(-1), left[0], left[1]);
            for r in 2..bs {
                d[r][1] = avg3(left[r - 2], left[r - 1], left[r]);
            }
            for c in 0..bs - 2 {
                d[0][2 + c] = avg3(ab(c as isize - 1), ab(c as isize), ab(c as isize + 1));
            }
            for r in 1..bs {
                for c in 0..bs - 2 {
                    d[r][2 + c] = d[r - 1][c];
                }
            }
            for y in 0..bs {
                for x in 0..bs {
                    put(x, y, d[y][x]);
                }
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn edge(x0: usize, y0: usize, top: bool, left: bool) -> IntraEdge {
        IntraEdge { x0, y0, have_top: top, have_left: left, have_right: false, frame_w: 64, frame_h: 64, right_overhang: false, bottom_overhang: false }
    }

    #[test]
    fn no_edges_is_mid_grey_and_borders_are_base_pm_one() {
        let mut b = vec![0u16; 64 * 64];
        predict(&mut b, 64, &edge(0, 0, false, false), TX_8X8, DC_PRED, 8);
        assert!(b[..8].iter().all(|&v| v == 128));
        // V with no top row copies 127; H with no left column copies 129.
        predict(&mut b, 64, &edge(8, 0, false, false), TX_4X4, V_PRED, 8);
        assert_eq!(b[8], 127);
        predict(&mut b, 64, &edge(16, 0, false, false), TX_4X4, H_PRED, 8);
        assert_eq!(b[16], 129);
        // 10-bit: 512 / 511 / 513.
        predict(&mut b, 64, &edge(24, 0, false, false), TX_4X4, DC_PRED, 10);
        assert_eq!(b[24], 512);
    }

    #[test]
    fn tm_from_a_gradient() {
        let mut b = vec![0u16; 16 * 16];
        for x in 0..16 {
            b[x] = 10 + x as u16; // row 0
        }
        for y in 0..16 {
            b[y * 16] = 10 + y as u16; // column 0
        }
        predict(&mut b, 16, &edge(1, 1, true, true), TX_4X4, TM_PRED, 8);
        // left[y] + above[x] - topleft = (11+y) + (11+x) - 10
        assert_eq!(b[16 + 1], 12);
        assert_eq!(b[4 * 16 + 4], 18);
    }
}
