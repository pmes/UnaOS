// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The loop filter (spec §8.8): per-segment / reference / mode filter levels (§8.8.1 with the
//! deltas scaled by 1 << (level >> 5)), sharpness limits, and the edge loop over each 64x64
//! superblock in raster order — for every plane, all vertical edges (left to right within each 8-row
//! band), then all horizontal edges. An edge is filtered with the 16-wide, 8-wide or 4-wide filter
//! by transform size (32x32 uses 16, 4x4 edges on 32-sample boundaries use 8), never on the frame's
//! left / top edge, and not inside a skipped inter block except on its prediction-block edges; the
//! last half-size chroma row / column of an odd-sized frame drops to the 8-wide filter and loses its
//! inner 4x4 edge. Filters (§8.8.2): filter_mask, hev, flat / flat2 at 1 << (bd-8), the 4-tap,
//! 7-tap [1,1,1,2,1,1,1] and 15-tap filters, with every threshold shifted by bd-8.
//!
//! This is the reference decoder's generic per-plane path (vp9_filter_block_plane_non420), which it
//! uses for 4:4:0 / 4:2:2 and which builds the same edges as its 4:2:0 / 4:4:4 mask paths.

use crate::consts::*;
use crate::frame::FrameBuf;
use crate::header::{LoopFilterParams, Segmentation};
use crate::tables::UV_TXSIZE_LOOKUP;

/// What the loop filter needs from one 8x8 mode-info unit.
#[derive(Clone, Copy, Debug, Default)]
pub struct LfCell {
    pub level: u8,
    pub sb_type: u8,
    pub tx_size: u8,
    /// skip && is_inter: inner transform edges are not filtered.
    pub skip_inter: bool,
}

/// lvl[segment][ref_frame][mode_delta_index] (vp9_loop_filter_frame_init).
pub fn levels(lf: &LoopFilterParams, seg: &Segmentation) -> [[[u8; 2]; 4]; 8] {
    let mut out = [[[0u8; 2]; 4]; 8];
    let base = lf.level as i32;
    let scale = 1 << (base >> 5);
    for s in 0..8 {
        let mut lvl_seg = base;
        if seg.active(s, SEG_LVL_ALT_L) {
            let d = seg.feature_data[s][SEG_LVL_ALT_L] as i32;
            lvl_seg = (if seg.abs_delta { d } else { base + d }).clamp(0, MAX_LOOP_FILTER);
        }
        if !lf.delta_enabled {
            out[s] = [[lvl_seg as u8; 2]; 4];
        } else {
            let intra = lvl_seg + lf.ref_deltas[0] as i32 * scale;
            out[s][0][0] = intra.clamp(0, MAX_LOOP_FILTER) as u8;
            out[s][0][1] = out[s][0][0];
            for r in 1..4 {
                for m in 0..2 {
                    let v = lvl_seg + lf.ref_deltas[r] as i32 * scale + lf.mode_deltas[m] as i32 * scale;
                    out[s][r][m] = v.clamp(0, MAX_LOOP_FILTER) as u8;
                }
            }
        }
    }
    out
}

/// mode_lf_lut: ZEROMV and the intra modes use mode delta 0, the other inter modes delta 1.
pub fn mode_delta_index(mode: u8) -> usize {
    (mode >= NEARESTMV && mode != ZEROMV) as usize
}

#[derive(Clone, Copy)]
struct Thresh {
    mblim: i32,
    lim: i32,
    hev: i32,
}

fn thresholds(sharpness: u8) -> [Thresh; 64] {
    let mut t = [Thresh { mblim: 0, lim: 0, hev: 0 }; 64];
    for lvl in 0..64i32 {
        let s = sharpness as i32;
        let mut lim = lvl >> ((s > 0) as i32 + (s > 4) as i32);
        if s > 0 && lim > 9 - s {
            lim = 9 - s;
        }
        if lim < 1 {
            lim = 1;
        }
        t[lvl as usize] = Thresh { mblim: 2 * (lvl + 2) + lim, lim, hev: lvl >> 4 };
    }
    t
}

/// Filter the whole frame. `cells` is the mi_rows x mi_cols grid.
pub fn filter_frame(fb: &mut FrameBuf, cells: &[LfCell], sharpness: u8) {
    let th = thresholds(sharpness);
    let bd = fb.bit_depth;
    let (mi_rows, mi_cols) = (fb.mi_rows, fb.mi_cols);
    for mi_row in (0..mi_rows).step_by(8) {
        for mi_col in (0..mi_cols).step_by(8) {
            for plane in 0..3 {
                let (ssx, ssy) = if plane == 0 { (0, 0) } else { (fb.ss_x as usize, fb.ss_y as usize) };
                let p = &mut fb.planes[plane];
                filter_block_plane(&mut p.data, p.stride, cells, mi_rows, mi_cols, mi_row, mi_col, ssx, ssy, &th, bd);
            }
        }
    }
}

fn filter_block_plane(buf: &mut [u16], stride: usize, cells: &[LfCell], mi_rows: usize, mi_cols: usize, mi_row: usize, mi_col: usize, ssx: usize, ssy: usize, th: &[Thresh; 64], bd: u8) {
    let row_step = 1 << ssy;
    let col_step = 1 << ssx;
    let mut mask16 = [0u32; 8];
    let mut mask8 = [0u32; 8];
    let mut mask4 = [0u32; 8];
    let mut mask4int = [0u32; 8];
    let mut lfl = [0u8; 64];
    let x_base = (mi_col * 8) >> ssx;
    let y_base = (mi_row * 8) >> ssy;

    // Vertical edges, one 8-sample band (r) at a time.
    let mut r = 0;
    while r < 8 && mi_row + r < mi_rows {
        let (mut m16c, mut m8c, mut m4c) = (0u32, 0u32, 0u32);
        let mut c = 0;
        while c < 8 && mi_col + c < mi_cols {
            let cell = &cells[(mi_row + r) * mi_cols + mi_col + c];
            let sb = cell.sb_type as usize;
            let skip_this = cell.skip_inter;
            let block_edge_left = if NUM_4X4_W[sb] > 1 { c & (NUM_8X8_W[sb] as usize - 1) == 0 } else { true };
            let skip_this_c = skip_this && !block_edge_left;
            let block_edge_above = if NUM_4X4_H[sb] > 1 { r & (NUM_8X8_H[sb] as usize - 1) == 0 } else { true };
            let skip_this_r = skip_this && !block_edge_above;
            let tx = if ssx == 0 && ssy == 0 { cell.tx_size } else { UV_TXSIZE_LOOKUP[sb][cell.tx_size as usize][ssx][ssy] };
            let skip_border_4x4_c = ssx == 1 && mi_col + c == mi_cols - 1;
            let skip_border_4x4_r = ssy == 1 && mi_row + r == mi_rows - 1;
            let bit = 1u32 << (c >> ssx);
            lfl[(r << 3) + (c >> ssx)] = cell.level;
            if cell.level == 0 {
                c += col_step;
                continue;
            }
            if tx == TX_32X32 {
                if !skip_this_c && ((c >> ssx) & 3) == 0 {
                    if !skip_border_4x4_c { m16c |= bit } else { m8c |= bit }
                }
                if !skip_this_r && ((r >> ssy) & 3) == 0 {
                    if !skip_border_4x4_r { mask16[r] |= bit } else { mask8[r] |= bit }
                }
            } else if tx == TX_16X16 {
                if !skip_this_c && ((c >> ssx) & 1) == 0 {
                    if !skip_border_4x4_c { m16c |= bit } else { m8c |= bit }
                }
                if !skip_this_r && ((r >> ssy) & 1) == 0 {
                    if !skip_border_4x4_r { mask16[r] |= bit } else { mask8[r] |= bit }
                }
            } else {
                if !skip_this_c {
                    if tx == TX_8X8 || ((c >> ssx) & 3) == 0 { m8c |= bit } else { m4c |= bit }
                }
                if !skip_this_r {
                    if tx == TX_8X8 || ((r >> ssy) & 3) == 0 { mask8[r] |= bit } else { mask4[r] |= bit }
                }
                if !skip_this && tx < TX_8X8 && !skip_border_4x4_c {
                    mask4int[r] |= bit;
                }
            }
            c += col_step;
        }
        let border = if mi_col == 0 { !1u32 } else { !0u32 };
        let y = y_base + ((r * 8) >> ssy);
        filter_vert(buf, stride, x_base, y, m16c & border, m8c & border, m4c & border, mask4int[r], th, &lfl[r << 3..], bd);
        r += row_step;
    }

    // Horizontal edges.
    let mut r = 0;
    while r < 8 && mi_row + r < mi_rows {
        let skip_border_4x4_r = ssy == 1 && mi_row + r == mi_rows - 1;
        let m4int = if skip_border_4x4_r { 0 } else { mask4int[r] };
        let (m16, m8, m4) = if mi_row + r == 0 { (0, 0, 0) } else { (mask16[r], mask8[r], mask4[r]) };
        let y = y_base + ((r * 8) >> ssy);
        filter_horiz(buf, stride, x_base, y, m16, m8, m4, m4int, th, &lfl[r << 3..], bd);
        r += row_step;
    }
}

fn filter_vert(buf: &mut [u16], stride: usize, x: usize, y: usize, mut m16: u32, mut m8: u32, mut m4: u32, mut m4i: u32, th: &[Thresh; 64], lfl: &[u8], bd: u8) {
    let mut col = 0usize;
    while (m16 | m8 | m4 | m4i) != 0 {
        let t = th[lfl[col] as usize];
        let at = y * stride + x + col * 8;
        if (m16 | m8 | m4) & 1 != 0 {
            if m16 & 1 != 0 {
                edge(buf, at, 1, stride, 8, 16, t, bd);
            } else if m8 & 1 != 0 {
                edge(buf, at, 1, stride, 8, 8, t, bd);
            } else {
                edge(buf, at, 1, stride, 8, 4, t, bd);
            }
        }
        if m4i & 1 != 0 {
            edge(buf, at + 4, 1, stride, 8, 4, t, bd);
        }
        col += 1;
        m16 >>= 1;
        m8 >>= 1;
        m4 >>= 1;
        m4i >>= 1;
    }
}

fn filter_horiz(buf: &mut [u16], stride: usize, x: usize, y: usize, mut m16: u32, mut m8: u32, mut m4: u32, mut m4i: u32, th: &[Thresh; 64], lfl: &[u8], bd: u8) {
    let mut col = 0usize;
    while (m16 | m8 | m4 | m4i) != 0 {
        let t = th[lfl[col] as usize];
        let at = y * stride + x + col * 8;
        if m16 & 1 != 0 {
            edge(buf, at, stride, 1, 8, 16, t, bd);
        } else if m8 & 1 != 0 {
            edge(buf, at, stride, 1, 8, 8, t, bd);
            if m4i & 1 != 0 {
                edge(buf, at + 4 * stride, stride, 1, 8, 4, t, bd);
            }
        } else if m4 & 1 != 0 {
            edge(buf, at, stride, 1, 8, 4, t, bd);
            if m4i & 1 != 0 {
                edge(buf, at + 4 * stride, stride, 1, 8, 4, t, bd);
            }
        } else if m4i & 1 != 0 {
            edge(buf, at + 4 * stride, stride, 1, 8, 4, t, bd);
        }
        col += 1;
        m16 >>= 1;
        m8 >>= 1;
        m4 >>= 1;
        m4i >>= 1;
    }
}

/// Filter `count` samples along an edge. `q0` is the first sample past the edge; `step` moves
/// across the edge (p0 = q0 - step), `along` moves to the next sample along it.
fn edge(buf: &mut [u16], q0: usize, step: usize, along: usize, count: usize, size: u8, t: Thresh, bd: u8) {
    let shift = bd as i32 - 8;
    let lim = t.lim << shift;
    let mblim = t.mblim << shift;
    let hev_t = t.hev << shift;
    let one = 1 << shift;
    for i in 0..count {
        let o = q0 + i * along;
        let g = |k: isize| buf[(o as isize + k * step as isize) as usize] as i32;
        let (p3, p2, p1, p0, q0v, q1, q2, q3) = (g(-4), g(-3), g(-2), g(-1), g(0), g(1), g(2), g(3));
        let mask = (p3 - p2).abs() <= lim
            && (p2 - p1).abs() <= lim
            && (p1 - p0).abs() <= lim
            && (q1 - q0v).abs() <= lim
            && (q2 - q1).abs() <= lim
            && (q3 - q2).abs() <= lim
            && (p0 - q0v).abs() * 2 + (p1 - q1).abs() / 2 <= mblim;
        if !mask {
            continue;
        }
        let flat = size >= 8
            && (p1 - p0).abs() <= one
            && (q1 - q0v).abs() <= one
            && (p2 - p0).abs() <= one
            && (q2 - q0v).abs() <= one
            && (p3 - p0).abs() <= one
            && (q3 - q0v).abs() <= one;
        let set = |buf: &mut [u16], k: isize, v: i32| buf[(o as isize + k * step as isize) as usize] = v as u16;
        if size == 16 && flat {
            let (p7, p6, p5, p4, q4, q5, q6, q7) = (g(-8), g(-7), g(-6), g(-5), g(4), g(5), g(6), g(7));
            let flat2 = (p4 - p0).abs() <= one
                && (q4 - q0v).abs() <= one
                && (p5 - p0).abs() <= one
                && (q5 - q0v).abs() <= one
                && (p6 - p0).abs() <= one
                && (q6 - q0v).abs() <= one
                && (p7 - p0).abs() <= one
                && (q7 - q0v).abs() <= one;
            if flat2 {
                let px = [p7, p6, p5, p4, p3, p2, p1, p0, q0v, q1, q2, q3, q4, q5, q6, q7];
                // 15-tap: output k (1..=14) = Round2(sum of taps over px[k-7..=k+7] clamped to the
                // ends, with px[k] counted twice, 4).
                let mut out = [0i32; 16];
                for k in 1..15 {
                    let mut s = px[k];
                    for d in -7isize..=7 {
                        let idx = (k as isize + d).clamp(0, 15) as usize;
                        s += px[idx];
                    }
                    out[k] = (s + 8) >> 4;
                }
                for k in 1..15 {
                    set(buf, k as isize - 8, out[k]);
                }
                continue;
            }
        }
        if flat {
            set(buf, -3, (p3 + p3 + p3 + 2 * p2 + p1 + p0 + q0v + 4) >> 3);
            set(buf, -2, (p3 + p3 + p2 + 2 * p1 + p0 + q0v + q1 + 4) >> 3);
            set(buf, -1, (p3 + p2 + p1 + 2 * p0 + q0v + q1 + q2 + 4) >> 3);
            set(buf, 0, (p2 + p1 + p0 + 2 * q0v + q1 + q2 + q3 + 4) >> 3);
            set(buf, 1, (p1 + p0 + q0v + 2 * q1 + q2 + q3 + q3 + 4) >> 3);
            set(buf, 2, (p0 + q0v + q1 + 2 * q2 + q3 + q3 + q3 + 4) >> 3);
            continue;
        }
        // filter4
        let lo = -(128 << shift);
        let hi = (128 << shift) - 1;
        let c = |v: i32| v.clamp(lo, hi);
        let off = 0x80 << shift;
        let (ps1, ps0, qs0, qs1) = (p1 - off, p0 - off, q0v - off, q1 - off);
        let hev = (p1 - p0).abs() > hev_t || (q1 - q0v).abs() > hev_t;
        let mut filter = if hev { c(ps1 - qs1) } else { 0 };
        filter = c(filter + 3 * (qs0 - ps0));
        let f1 = c(filter + 4) >> 3;
        let f2 = c(filter + 3) >> 3;
        set(buf, 0, c(qs0 - f1) + off);
        set(buf, -1, c(ps0 + f2) + off);
        if !hev {
            let f = (f1 + 1) >> 1;
            set(buf, 1, c(qs1 - f) + off);
            set(buf, -2, c(ps1 + f) + off);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 15-tap closed form above equals the printed taps, e.g. op6 = (7*p7 + 2*p6 + p5..q0 + 8) >> 4.
    #[test]
    fn fifteen_tap_ends() {
        let px: [i32; 16] = [10, 20, 30, 40, 50, 60, 70, 80, 90, 100, 110, 120, 130, 140, 150, 160];
        let mut s = px[1];
        for d in -7isize..=7 {
            s += px[(1 + d).clamp(0, 15) as usize];
        }
        let want = (px[0] * 7 + px[1] * 2 + px[2] + px[3] + px[4] + px[5] + px[6] + px[7] + px[8] + 8) >> 4;
        assert_eq!((s + 8) >> 4, want);
        let mut s = px[14];
        for d in -7isize..=7 {
            s += px[(14 + d).clamp(0, 15) as usize];
        }
        let want = (px[7] + px[8] + px[9] + px[10] + px[11] + px[12] + px[13] + px[14] * 2 + px[15] * 7 + 8) >> 4;
        assert_eq!((s + 8) >> 4, want);
    }

    #[test]
    fn levels_with_deltas() {
        let lf = LoopFilterParams { level: 40, sharpness: 0, delta_enabled: true, ref_deltas: [1, 0, -1, -1], mode_deltas: [0, 0] };
        let l = levels(&lf, &Segmentation::default());
        // scale = 2 at level >= 32.
        assert_eq!(l[0][0][0], 42);
        assert_eq!(l[0][1][0], 40);
        assert_eq!(l[0][2][1], 38);
    }
}
