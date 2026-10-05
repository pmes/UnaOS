//! §7.15 CDEF: per-8x8 direction search, variance-adjusted primary strength, and the constrained
//! directional filter. Reads CurrFrame (deblocked), writes CdefFrame.

use crate::bits::floor_log2;
use crate::decode::{FrameState, Plane, CDEF_NONE};
use crate::obu::FrameHeader;
use crate::tables::*;

pub fn cdef_frame(fs: &FrameState, h: &FrameHeader) -> [Plane; 3] {
    let mut out = [fs.planes[0].clone(), fs.planes[1].clone(), fs.planes[2].clone()];
    let step4 = NUM_4X4_BLOCKS_WIDE[BLOCK_8X8] as usize;
    let cdef_size4 = NUM_4X4_BLOCKS_WIDE[BLOCK_64X64] as usize;
    let cdef_mask4 = !(cdef_size4 - 1);
    let mut r = 0;
    while r < fs.mi_rows {
        let mut c = 0;
        while c < fs.mi_cols {
            let base_r = r & cdef_mask4;
            let base_c = c & cdef_mask4;
            let idx = fs.cdef_idx[fs.mi(base_r, base_c)];
            cdef_block(fs, h, &mut out, r, c, idx);
            c += step4;
        }
        r += step4;
    }
    out
}

fn cdef_block(fs: &FrameState, h: &FrameHeader, out: &mut [Plane; 3], r: usize, c: usize, idx: i8) {
    // (the copy into CdefFrame happened wholesale in cdef_frame)
    if idx == CDEF_NONE {
        return;
    }
    let idx = idx as usize;
    let coeff_shift = fs.bit_depth - 8;
    let skip = fs.skips[fs.mi(r, c)] != 0 && fs.skips[fs.mi(r + 1, c)] != 0 && fs.skips[fs.mi(r, c + 1)] != 0 && fs.skips[fs.mi(r + 1, c + 1)] != 0;
    if skip {
        return;
    }
    let (y_dir, var) = cdef_direction(fs, r, c);
    let mut pri_str = (h.cdef_y_pri_strength[idx] << coeff_shift) as i32;
    let mut sec_str = (h.cdef_y_sec_strength[idx] << coeff_shift) as i32;
    let mut dir = if pri_str == 0 { 0 } else { y_dir };
    let var_str = if (var >> 6) != 0 { floor_log2((var >> 6) as u32).min(12) as i32 } else { 0 };
    pri_str = if var != 0 { (pri_str * (4 + var_str) + 8) >> 4 } else { 0 };
    let mut damping = h.cdef_damping as i32 + coeff_shift as i32;
    cdef_filter(fs, out, 0, r, c, pri_str, sec_str, damping, dir);
    if fs.num_planes == 1 {
        return;
    }
    pri_str = (h.cdef_uv_pri_strength[idx] << coeff_shift) as i32;
    sec_str = (h.cdef_uv_sec_strength[idx] << coeff_shift) as i32;
    dir = if pri_str == 0 { 0 } else { CDEF_UV_DIR[fs.ss_x][fs.ss_y][y_dir] as usize };
    damping = h.cdef_damping as i32 + coeff_shift as i32 - 1;
    cdef_filter(fs, out, 1, r, c, pri_str, sec_str, damping, dir);
    cdef_filter(fs, out, 2, r, c, pri_str, sec_str, damping, dir);
}

fn cdef_direction(fs: &FrameState, r: usize, c: usize) -> (usize, i64) {
    let mut cost = [0i64; 8];
    let mut partial = [[0i64; 15]; 8];
    let x0 = c << MI_SIZE_LOG2;
    let y0 = r << MI_SIZE_LOG2;
    let p = &fs.planes[0];
    for i in 0..8 {
        for j in 0..8 {
            let x = ((p.get(x0 + j, y0 + i) as i64) >> (fs.bit_depth - 8)) - 128;
            partial[0][i + j] += x;
            partial[1][i + j / 2] += x;
            partial[2][i] += x;
            partial[3][3 + i - j / 2] += x;
            partial[4][7 + i - j] += x;
            partial[5][3 - i / 2 + j] += x;
            partial[6][j] += x;
            partial[7][i / 2 + j] += x;
        }
    }
    let dt = |i: usize| DIV_TABLE[i] as i64;
    for i in 0..8 {
        cost[2] += partial[2][i] * partial[2][i];
        cost[6] += partial[6][i] * partial[6][i];
    }
    cost[2] *= dt(8);
    cost[6] *= dt(8);
    for i in 0..7 {
        cost[0] += (partial[0][i] * partial[0][i] + partial[0][14 - i] * partial[0][14 - i]) * dt(i + 1);
        cost[4] += (partial[4][i] * partial[4][i] + partial[4][14 - i] * partial[4][14 - i]) * dt(i + 1);
    }
    cost[0] += partial[0][7] * partial[0][7] * dt(8);
    cost[4] += partial[4][7] * partial[4][7] * dt(8);
    let mut i = 1;
    while i < 8 {
        for j in 0..5 {
            cost[i] += partial[i][3 + j] * partial[i][3 + j];
        }
        cost[i] *= dt(8);
        for j in 0..3 {
            cost[i] += (partial[i][j] * partial[i][j] + partial[i][10 - j] * partial[i][10 - j]) * dt(2 * j + 2);
        }
        i += 2;
    }
    let mut best_cost = 0;
    let mut y_dir = 0;
    for i in 0..8 {
        if cost[i] > best_cost {
            best_cost = cost[i];
            y_dir = i;
        }
    }
    let var = (best_cost - cost[(y_dir + 4) & 7]) >> 10;
    (y_dir, var)
}

fn constrain(diff: i32, threshold: i32, damping: i32) -> i32 {
    if threshold == 0 {
        return 0;
    }
    let damping_adj = (damping - floor_log2(threshold as u32) as i32).max(0);
    let sign = if diff < 0 { -1 } else { 1 };
    sign * (threshold - (diff.abs() >> damping_adj)).clamp(0, diff.abs())
}

#[allow(clippy::too_many_arguments)]
fn cdef_filter(fs: &FrameState, out: &mut [Plane; 3], plane: usize, r: usize, c: usize, pri_str: i32, sec_str: i32, damping: i32, dir: usize) {
    let coeff_shift = fs.bit_depth - 8;
    let (sub_x, sub_y) = if plane > 0 { (fs.ss_x, fs.ss_y) } else { (0, 0) };
    let x0 = (c * MI_SIZE) >> sub_x;
    let y0 = (r * MI_SIZE) >> sub_y;
    let w = 8 >> sub_x;
    let h = 8 >> sub_y;
    let cur = &fs.planes[plane];
    let (mi_rows, mi_cols) = (fs.mi_rows as isize, fs.mi_cols as isize);
    let get = |i: usize, j: usize, dir: usize, k: usize, sign: isize| -> Option<i32> {
        let y = y0 as isize + i as isize + sign * CDEF_DIRECTIONS[dir][k][0] as isize;
        let x = x0 as isize + j as isize + sign * CDEF_DIRECTIONS[dir][k][1] as isize;
        let cand_r = (y << sub_y) >> MI_SIZE_LOG2;
        let cand_c = (x << sub_x) >> MI_SIZE_LOG2;
        // is_inside_filter_region
        if cand_c >= 0 && cand_c < mi_cols && cand_r >= 0 && cand_r < mi_rows {
            Some(cur.get(x as usize, y as usize) as i32)
        } else {
            None
        }
    };
    let pt = (pri_str >> coeff_shift) & 1;
    for i in 0..h {
        for j in 0..w {
            let mut sum = 0i32;
            let x = cur.get(x0 + j, y0 + i) as i32;
            let mut max = x;
            let mut min = x;
            for k in 0..2 {
                for sign in [-1isize, 1] {
                    if let Some(p) = get(i, j, dir, k, sign) {
                        sum += CDEF_PRI_TAPS[pt as usize][k] as i32 * constrain(p - x, pri_str, damping);
                        max = max.max(p);
                        min = min.min(p);
                    }
                    for dir_off in [-2isize, 2] {
                        let d2 = ((dir as isize + dir_off) & 7) as usize;
                        if let Some(s) = get(i, j, d2, k, sign) {
                            sum += CDEF_SEC_TAPS[pt as usize][k] as i32 * constrain(s - x, sec_str, damping);
                            max = max.max(s);
                            min = min.min(s);
                        }
                    }
                }
            }
            let v = x + ((8 + sum - (sum < 0) as i32) >> 4);
            out[plane].set(x0 + j, y0 + i, v.clamp(min, max) as u16);
        }
    }
}
