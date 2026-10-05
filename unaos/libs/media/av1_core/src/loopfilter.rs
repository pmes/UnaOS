//! §7.14 the deblocking loop filter: edge loop (vertical edges of all planes, then horizontal),
//! filter size, adaptive strength (segment / delta-lf / ref & mode deltas), masks, the narrow
//! 4-tap filter and the wide 6/8/16 filters.

use crate::decode::{round2, FrameState};
use crate::obu::FrameHeader;
use crate::tables::*;

pub fn loop_filter_frame(fs: &mut FrameState, h: &FrameHeader) {
    for plane in 0..fs.num_planes {
        if plane == 0 || h.loop_filter_level[1 + plane] != 0 {
            for pass in 0..2 {
                let row_step = if plane == 0 { 1 } else { 1 << fs.ss_y };
                let col_step = if plane == 0 { 1 } else { 1 << fs.ss_x };
                let mut row = 0;
                while row < fs.mi_rows {
                    let mut col = 0;
                    while col < fs.mi_cols {
                        edge(fs, h, plane, pass, row, col);
                        col += col_step;
                    }
                    row += row_step;
                }
            }
        }
    }
}

fn edge(fs: &mut FrameState, h: &FrameHeader, plane: usize, pass: usize, row: usize, col: usize) {
    let (sub_x, sub_y) = if plane == 0 { (0, 0) } else { (fs.ss_x, fs.ss_y) };
    let (dx, dy) = if pass == 0 { (1isize, 0isize) } else { (0, 1) };
    let x = col * MI_SIZE;
    let y = row * MI_SIZE;
    let row = row | sub_y;
    let col = col | sub_x;
    let on_screen = !(x >= h.frame_width as usize || y >= h.frame_height as usize || (pass == 0 && x == 0) || (pass == 1 && y == 0));
    if !on_screen {
        return;
    }
    let xp = x >> sub_x;
    let yp = y >> sub_y;
    let prev_row = row - ((dy as usize) << sub_y);
    let prev_col = col - ((dx as usize) << sub_x);
    let mi = fs.mi(row, col);
    let mi_size = fs.mi_sizes[mi] as usize;
    let tx_sz = fs.lf_tx_sizes[plane][(row >> sub_y) * fs.mi_cols + (col >> sub_x)] as usize;
    let plane_size = SUBSAMPLED_SIZE[mi_size][sub_x][sub_y] as usize;
    let skip = fs.skips[mi] != 0;
    let is_intra = fs.ref_frames[mi][0] as i32 <= INTRA_FRAME as i32;
    let prev_tx_sz = fs.lf_tx_sizes[plane][(prev_row >> sub_y) * fs.mi_cols + (prev_col >> sub_x)] as usize;
    let is_block_edge = if pass == 0 {
        xp % (4 * NUM_4X4_BLOCKS_WIDE[plane_size] as usize) == 0
    } else {
        yp % (4 * NUM_4X4_BLOCKS_HIGH[plane_size] as usize) == 0
    };
    let is_tx_edge = if pass == 0 { xp % TX_WIDTH[tx_sz] as usize == 0 } else { yp % TX_HEIGHT[tx_sz] as usize == 0 };
    let apply_filter = if !is_tx_edge { false } else { is_block_edge || !skip || is_intra };
    // filter size (§7.14.3)
    let base_size = if pass == 0 {
        (TX_WIDTH[prev_tx_sz] as usize).min(TX_WIDTH[tx_sz] as usize)
    } else {
        (TX_HEIGHT[prev_tx_sz] as usize).min(TX_HEIGHT[tx_sz] as usize)
    };
    let filter_size = if plane == 0 { base_size.min(16) } else { base_size.min(8) };
    let (mut lvl, mut limit, mut blimit, mut thresh) = strength(fs, h, row, col, plane, pass);
    if lvl == 0 {
        (lvl, limit, blimit, thresh) = strength(fs, h, prev_row, prev_col, plane, pass);
    }
    for i in 0..MI_SIZE {
        if apply_filter && lvl > 0 {
            sample_filter(
                fs,
                xp as isize + dy * i as isize,
                yp as isize + dx * i as isize,
                plane,
                limit,
                blimit,
                thresh,
                dx,
                dy,
                filter_size,
            );
        }
    }
}

/// §7.14.4 + §7.14.5 -> (lvl, limit, blimit, thresh)
fn strength(fs: &FrameState, h: &FrameHeader, row: usize, col: usize, plane: usize, pass: usize) -> (i32, i32, i32, i32) {
    let mi = fs.mi(row, col);
    let segment = fs.segment_ids[mi] as usize;
    let rf = fs.ref_frames[mi][0] as i32;
    let mode = fs.y_modes[mi] as usize;
    let mode_type = if mode >= NEARESTMV && mode != GLOBALMV && mode != GLOBAL_GLOBALMV { 1 } else { 0 };
    let delta_lf = if !h.delta_lf_multi {
        fs.delta_lfs[mi][0] as i32
    } else {
        fs.delta_lfs[mi][if plane == 0 { pass } else { plane + 1 }] as i32
    };
    // §7.14.5
    let i = if plane == 0 { pass } else { plane + 1 };
    let base_filter_level = (delta_lf + h.loop_filter_level[i] as i32).clamp(0, MAX_LOOP_FILTER as i32);
    let mut lvl_seg = base_filter_level;
    let feature = SEG_LVL_ALT_LF_Y_V + i;
    if h.seg_feature_active_idx(segment, feature) {
        lvl_seg = (h.feature_data[segment][feature] + lvl_seg).clamp(0, MAX_LOOP_FILTER as i32);
    }
    if h.loop_filter_delta_enabled {
        let n_shift = lvl_seg >> 5;
        if rf <= INTRA_FRAME as i32 {
            lvl_seg += h.loop_filter_ref_deltas[INTRA_FRAME] << n_shift;
        } else {
            lvl_seg += (h.loop_filter_ref_deltas[rf as usize] << n_shift) + (h.loop_filter_mode_deltas[mode_type] << n_shift);
        }
        lvl_seg = lvl_seg.clamp(0, MAX_LOOP_FILTER as i32);
    }
    let lvl = lvl_seg;
    let shift = if h.loop_filter_sharpness > 4 {
        2
    } else if h.loop_filter_sharpness > 0 {
        1
    } else {
        0
    };
    let limit = if h.loop_filter_sharpness > 0 {
        (lvl >> shift).clamp(1, 9 - h.loop_filter_sharpness as i32)
    } else {
        (lvl >> shift).max(1)
    };
    let blimit = 2 * (lvl + 2) + limit;
    let thresh = lvl >> 4;
    (lvl, limit, blimit, thresh)
}

#[allow(clippy::too_many_arguments)]
fn sample_filter(fs: &mut FrameState, x: isize, y: isize, plane: usize, limit: i32, blimit: i32, thresh: i32, dx: isize, dy: isize, filter_size: usize) {
    let bd = fs.bit_depth;
    let p = &mut fs.planes[plane];
    let at = |p: &crate::decode::Plane, k: isize| -> i32 { p.get((x + dx * k) as usize, (y + dy * k) as usize) as i32 };
    let q0 = at(p, 0);
    let q1 = at(p, 1);
    let q2 = at(p, 2);
    let q3 = at(p, 3);
    let p0 = at(p, -1);
    let p1 = at(p, -2);
    let p2 = at(p, -3);
    let p3 = at(p, -4);
    // filter mask process (§7.14.6.2)
    let thresh_bd = thresh << (bd - 8);
    let hev_mask = (p1 - p0).abs() > thresh_bd || (q1 - q0).abs() > thresh_bd;
    let filter_len = if filter_size == 4 {
        4
    } else if plane != 0 {
        6
    } else if filter_size == 8 {
        8
    } else {
        16
    };
    let limit_bd = limit << (bd - 8);
    let blimit_bd = blimit << (bd - 8);
    let mut mask = false;
    mask |= (p1 - p0).abs() > limit_bd;
    mask |= (q1 - q0).abs() > limit_bd;
    mask |= (p0 - q0).abs() * 2 + (p1 - q1).abs() / 2 > blimit_bd;
    if filter_len >= 6 {
        mask |= (p2 - p1).abs() > limit_bd;
        mask |= (q2 - q1).abs() > limit_bd;
    }
    if filter_len >= 8 {
        mask |= (p3 - p2).abs() > limit_bd;
        mask |= (q3 - q2).abs() > limit_bd;
    }
    let filter_mask = !mask;
    let threshold_bd = 1 << (bd - 8);
    let mut flat_mask = false;
    if filter_size >= 8 {
        let mut m = false;
        m |= (p1 - p0).abs() > threshold_bd;
        m |= (q1 - q0).abs() > threshold_bd;
        m |= (p2 - p0).abs() > threshold_bd;
        m |= (q2 - q0).abs() > threshold_bd;
        if filter_len >= 8 {
            m |= (p3 - p0).abs() > threshold_bd;
            m |= (q3 - q0).abs() > threshold_bd;
        }
        flat_mask = !m;
    }
    let mut flat_mask2 = false;
    if filter_size >= 16 {
        let q4 = at(p, 4);
        let q5 = at(p, 5);
        let q6 = at(p, 6);
        let p4 = at(p, -5);
        let p5 = at(p, -6);
        let p6 = at(p, -7);
        let mut m = false;
        m |= (p6 - p0).abs() > threshold_bd;
        m |= (q6 - q0).abs() > threshold_bd;
        m |= (p5 - p0).abs() > threshold_bd;
        m |= (q5 - q0).abs() > threshold_bd;
        m |= (p4 - p0).abs() > threshold_bd;
        m |= (q4 - q0).abs() > threshold_bd;
        flat_mask2 = !m;
    }
    if !filter_mask {
        return;
    }
    let set = |p: &mut crate::decode::Plane, k: isize, v: i32| p.set((x + dx * k) as usize, (y + dy * k) as usize, v as u16);
    if filter_size == 4 || !flat_mask {
        // narrow filter (§7.14.6.3)
        let lo = -(1i32 << (bd - 1));
        let hi = (1i32 << (bd - 1)) - 1;
        let c = |v: i32| v.clamp(lo, hi);
        let off = 0x80 << (bd - 8);
        let ps1 = p1 - off;
        let ps0 = p0 - off;
        let qs0 = q0 - off;
        let qs1 = q1 - off;
        let mut filter = if hev_mask { c(ps1 - qs1) } else { 0 };
        filter = c(filter + 3 * (qs0 - ps0));
        let filter1 = c(filter + 4) >> 3;
        let filter2 = c(filter + 3) >> 3;
        set(p, 0, c(qs0 - filter1) + off);
        set(p, -1, c(ps0 + filter2) + off);
        if !hev_mask {
            let filter = round2(filter1 as i64, 1) as i32;
            set(p, 1, c(qs1 - filter) + off);
            set(p, -2, c(ps1 + filter) + off);
        }
    } else {
        let log2_size: u32 = if filter_size == 8 || !flat_mask2 { 3 } else { 4 };
        // wide filter (§7.14.6.4)
        let n: isize = if log2_size == 4 {
            6
        } else if plane == 0 {
            3
        } else {
            2
        };
        let n2: isize = if log2_size == 3 && plane == 0 { 0 } else { 1 };
        let mut f = [0i32; 16];
        for i in -n..n {
            let mut t = 0i32;
            for j in -n..=n {
                let pp = (i + j).clamp(-(n + 1), n);
                let tap = if j.abs() <= n2 { 2 } else { 1 };
                t += at(p, pp) * tap;
            }
            f[(i + n) as usize] = round2(t as i64, log2_size) as i32;
        }
        for i in -n..n {
            set(p, i, f[(i + n) as usize]);
        }
    }
}
