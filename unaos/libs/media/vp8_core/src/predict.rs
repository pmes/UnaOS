// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Prediction: intra (RFC 6386 §12) and inter (§18).
//!
//! Intra prediction runs in a per-macroblock workspace whose row 0 holds the pixels above the
//! macroblock (plus four above-right) and whose column 0 holds the pixels to its left, with the
//! frame-edge conventions of §12.2: above the top row every pixel is 127, left of the left column
//! every pixel is 129, and the above-left corner is 127 on the top row and 129 elsewhere in the
//! left column. Above-right of the rightmost macroblock repeats the last pixel of the row above.

use crate::consts::*;

/// Luma workspace: 1 border row + 16 rows, 1 border column + 16 + 4 above-right columns.
pub const WS: usize = 21;
/// Chroma workspace stride: 1 + 8.
pub const CWS: usize = 9;

#[inline]
fn avg3(a: u8, b: u8, c: u8) -> u8 {
    ((a as u32 + 2 * b as u32 + c as u32 + 2) >> 2) as u8
}
#[inline]
fn avg2(a: u8, b: u8) -> u8 {
    ((a as u32 + b as u32 + 1) >> 1) as u8
}
#[inline]
pub fn clamp255(v: i32) -> u8 {
    v.clamp(0, 255) as u8
}

/// 16×16 (n = 16, stride WS) or 8×8 chroma (n = 8, stride CWS) whole-block prediction into the
/// workspace interior. `have_above` / `have_left` only matter for DC (§12.2).
pub fn predict_mb(ws: &mut [u8], stride: usize, n: usize, mode: u8, have_above: bool, have_left: bool) {
    match mode {
        DC_PRED => {
            let mut sum = 0u32;
            let mut shift = if n == 16 { 3 } else { 2 };
            if have_above {
                for c in 0..n {
                    sum += ws[1 + c] as u32;
                }
                shift += 1;
            }
            if have_left {
                for r in 0..n {
                    sum += ws[(r + 1) * stride] as u32;
                }
                shift += 1;
            }
            let dc = if have_above || have_left { ((sum + (1 << (shift - 1))) >> shift) as u8 } else { 128 };
            for r in 0..n {
                ws[(r + 1) * stride + 1..(r + 1) * stride + 1 + n].fill(dc);
            }
        }
        V_PRED => {
            for r in 0..n {
                ws.copy_within(1..1 + n, (r + 1) * stride + 1);
            }
        }
        H_PRED => {
            for r in 0..n {
                let l = ws[(r + 1) * stride];
                ws[(r + 1) * stride + 1..(r + 1) * stride + 1 + n].fill(l);
            }
        }
        _ => {
            // TM_PRED
            let p = ws[0] as i32;
            for r in 0..n {
                let l = ws[(r + 1) * stride] as i32 - p;
                for c in 0..n {
                    ws[(r + 1) * stride + 1 + c] = clamp255(l + ws[1 + c] as i32);
                }
            }
        }
    }
}

/// One 4×4 sub-block (§12.3) at workspace offset `o` (its top-left pixel); the row above starts at
/// `o - stride - 1` (the corner), the left column at `o - 1`.
pub fn predict_sub(ws: &mut [u8], stride: usize, o: usize, mode: u8) {
    let p = ws[o - stride - 1];
    let mut a = [0u8; 8];
    a.copy_from_slice(&ws[o - stride..o - stride + 8]);
    let l = [ws[o - 1], ws[o + stride - 1], ws[o + 2 * stride - 1], ws[o + 3 * stride - 1]];
    let mut b = [[0u8; 4]; 4];
    match mode {
        B_DC_PRED => {
            let mut s = 4u32;
            for i in 0..4 {
                s += a[i] as u32 + l[i] as u32;
            }
            b = [[(s >> 3) as u8; 4]; 4];
        }
        B_TM_PRED => {
            for r in 0..4 {
                for c in 0..4 {
                    b[r][c] = clamp255(l[r] as i32 + a[c] as i32 - p as i32);
                }
            }
        }
        B_VE_PRED => {
            let row = [avg3(p, a[0], a[1]), avg3(a[0], a[1], a[2]), avg3(a[1], a[2], a[3]), avg3(a[2], a[3], a[4])];
            b = [row; 4];
        }
        B_HE_PRED => {
            let col = [avg3(p, l[0], l[1]), avg3(l[0], l[1], l[2]), avg3(l[1], l[2], l[3]), avg3(l[2], l[3], l[3])];
            for r in 0..4 {
                b[r] = [col[r]; 4];
            }
        }
        B_LD_PRED => {
            for r in 0..4 {
                for c in 0..4 {
                    let i = r + c;
                    b[r][c] = if i == 6 { avg3(a[6], a[7], a[7]) } else { avg3(a[i], a[i + 1], a[i + 2]) };
                }
            }
        }
        B_RD_PRED | B_VR_PRED | B_HD_PRED => {
            let e = [l[3], l[2], l[1], l[0], p, a[0], a[1], a[2], a[3]];
            match mode {
                B_RD_PRED => {
                    for r in 0..4 {
                        for c in 0..4 {
                            let i = 3 - r + c;
                            b[r][c] = avg3(e[i], e[i + 1], e[i + 2]);
                        }
                    }
                }
                B_VR_PRED => {
                    b[3][0] = avg3(e[1], e[2], e[3]);
                    b[2][0] = avg3(e[2], e[3], e[4]);
                    b[3][1] = avg3(e[3], e[4], e[5]);
                    b[1][0] = b[3][1];
                    b[2][1] = avg2(e[4], e[5]);
                    b[0][0] = b[2][1];
                    b[3][2] = avg3(e[4], e[5], e[6]);
                    b[1][1] = b[3][2];
                    b[2][2] = avg2(e[5], e[6]);
                    b[0][1] = b[2][2];
                    b[3][3] = avg3(e[5], e[6], e[7]);
                    b[1][2] = b[3][3];
                    b[2][3] = avg2(e[6], e[7]);
                    b[0][2] = b[2][3];
                    b[1][3] = avg3(e[6], e[7], e[8]);
                    b[0][3] = avg2(e[7], e[8]);
                }
                _ => {
                    // B_HD_PRED
                    b[3][0] = avg2(e[0], e[1]);
                    b[3][1] = avg3(e[0], e[1], e[2]);
                    b[2][0] = avg2(e[1], e[2]);
                    b[3][2] = b[2][0];
                    b[2][1] = avg3(e[1], e[2], e[3]);
                    b[3][3] = b[2][1];
                    b[2][2] = avg2(e[2], e[3]);
                    b[1][0] = b[2][2];
                    b[2][3] = avg3(e[2], e[3], e[4]);
                    b[1][1] = b[2][3];
                    b[1][2] = avg2(e[3], e[4]);
                    b[0][0] = b[1][2];
                    b[1][3] = avg3(e[3], e[4], e[5]);
                    b[0][1] = b[1][3];
                    b[0][2] = avg3(e[4], e[5], e[6]);
                    b[0][3] = avg3(e[5], e[6], e[7]);
                }
            }
        }
        B_VL_PRED => {
            b[0][0] = avg2(a[0], a[1]);
            b[1][0] = avg3(a[0], a[1], a[2]);
            b[2][0] = avg2(a[1], a[2]);
            b[0][1] = b[2][0];
            b[1][1] = avg3(a[1], a[2], a[3]);
            b[3][0] = b[1][1];
            b[2][1] = avg2(a[2], a[3]);
            b[0][2] = b[2][1];
            b[3][1] = avg3(a[2], a[3], a[4]);
            b[1][2] = b[3][1];
            b[2][2] = avg2(a[3], a[4]);
            b[0][3] = b[2][2];
            b[3][2] = avg3(a[3], a[4], a[5]);
            b[1][3] = b[3][2];
            b[2][3] = avg3(a[4], a[5], a[6]);
            b[3][3] = avg3(a[5], a[6], a[7]);
        }
        _ => {
            // B_HU_PRED
            b[0][0] = avg2(l[0], l[1]);
            b[0][1] = avg3(l[0], l[1], l[2]);
            b[0][2] = avg2(l[1], l[2]);
            b[1][0] = b[0][2];
            b[0][3] = avg3(l[1], l[2], l[3]);
            b[1][1] = b[0][3];
            b[1][2] = avg2(l[2], l[3]);
            b[2][0] = b[1][2];
            b[1][3] = avg3(l[2], l[3], l[3]);
            b[2][1] = b[1][3];
            b[2][2] = l[3];
            b[2][3] = l[3];
            b[3] = [l[3]; 4];
        }
    }
    for r in 0..4 {
        ws[o + r * stride..o + r * stride + 4].copy_from_slice(&b[r]);
    }
}

/// A reference plane: `w × h` samples (macroblock-aligned), `stride` apart. Reads outside are
/// clamped to the nearest edge sample — the reference decoder's border extension, made infinite
/// (equivalent, because every motion vector that reaches past the 32-pixel border is clamped to
/// one that sees only replicated samples, §5 / §18).
pub struct RefPlane<'a> {
    pub data: &'a [u8],
    pub stride: usize,
    pub w: i32,
    pub h: i32,
}

/// Predict a `bw × bh` block whose top-left is at (`x`, `y`) in the plane, displaced by the
/// motion vector (`mvx`, `mvy`) in eighth-sample units of this plane (luma vectors are quarter-pel
/// and arrive doubled), into `dst` at `dst_stride`. Six-tap (§18.3) or bilinear (version 1/2).
#[allow(clippy::too_many_arguments)]
pub fn predict_inter(
    r: &RefPlane,
    x: i32,
    y: i32,
    bw: usize,
    bh: usize,
    mvx: i32,
    mvy: i32,
    bilinear: bool,
    dst: &mut [u8],
    dst_stride: usize,
) {
    let x0 = x + (mvx >> 3);
    let y0 = y + (mvy >> 3);
    let fx = (mvx & 7) as usize;
    let fy = (mvy & 7) as usize;
    // Gather a (bw + 5) × (bh + 5) window starting 2 samples up/left, edge-clamped.
    let ww = bw + 5;
    let wh = bh + 5;
    let mut win = [0u8; 21 * 21];
    let inside = x0 - 2 >= 0 && y0 - 2 >= 0 && x0 + bw as i32 + 3 <= r.w && y0 + bh as i32 + 3 <= r.h;
    for j in 0..wh {
        let sy = y0 - 2 + j as i32;
        if inside {
            let o = sy as usize * r.stride + (x0 - 2) as usize;
            win[j * ww..j * ww + ww].copy_from_slice(&r.data[o..o + ww]);
        } else {
            let row = sy.clamp(0, r.h - 1) as usize * r.stride;
            for i in 0..ww {
                let sx = (x0 - 2 + i as i32).clamp(0, r.w - 1) as usize;
                win[j * ww + i] = r.data[row + sx];
            }
        }
    }
    if fx == 0 && fy == 0 {
        for j in 0..bh {
            dst[j * dst_stride..j * dst_stride + bw].copy_from_slice(&win[(j + 2) * ww + 2..(j + 2) * ww + 2 + bw]);
        }
        return;
    }
    if !bilinear {
        // First pass: horizontal over all bh + 5 rows (§18.3: rounded, clamped to 8 bits).
        let hf = &SIXTAP[fx];
        let mut tmp = [0u8; 16 * 21];
        for j in 0..wh {
            for i in 0..bw {
                let s = &win[j * ww + i..j * ww + i + 6];
                let mut v = 64;
                for t in 0..6 {
                    v += s[t] as i32 * hf[t];
                }
                tmp[j * bw + i] = clamp255(v >> 7);
            }
        }
        let vf = &SIXTAP[fy];
        for j in 0..bh {
            for i in 0..bw {
                let mut v = 64;
                for t in 0..6 {
                    v += tmp[(j + t) * bw + i] as i32 * vf[t];
                }
                dst[j * dst_stride + i] = clamp255(v >> 7);
            }
        }
    } else {
        let hf = &BILINEAR[fx];
        let mut tmp = [0u8; 16 * 17];
        for j in 0..bh + 1 {
            for i in 0..bw {
                let s = (j + 2) * ww + i + 2;
                tmp[j * bw + i] = ((win[s] as i32 * hf[0] + win[s + 1] as i32 * hf[1] + 64) >> 7) as u8;
            }
        }
        let vf = &BILINEAR[fy];
        for j in 0..bh {
            for i in 0..bw {
                dst[j * dst_stride + i] =
                    ((tmp[j * bw + i] as i32 * vf[0] + tmp[(j + 1) * bw + i] as i32 * vf[1] + 64) >> 7) as u8;
            }
        }
    }
}
