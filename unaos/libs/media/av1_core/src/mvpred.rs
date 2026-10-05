//! §7.9 motion field estimation and §7.10 motion vector prediction: the reference MV stack
//! (spatial row / column / point scans with weights, the temporal scan over the projected motion
//! field, sorting, the extra search, context derivation and clamping), the global-motion MV,
//! has_overlappable_candidates and find_warp_samples.

use crate::decode::{block_height, block_width, Dec, FrameState};
use crate::modeinfo::has_newmv;
use crate::obu::{get_relative_dist, FrameHeader, SequenceHeader};
use crate::refs::{Mv, RefStore};
use crate::tables::*;
use alloc::vec;

const INVALID_MV: i32 = -(1 << 15);
const INTRA: i32 = INTRA_FRAME as i32;

pub fn round2signed(x: i64, n: u32) -> i64 {
    if n == 0 {
        return x;
    }
    if x >= 0 {
        (x + (1i64 << (n - 1))) >> n
    } else {
        -((-x + (1i64 << (n - 1))) >> n)
    }
}

/// lower_mv_precision( candMv ) (§7.10.2.10)
pub fn lower_mv_precision(h: &FrameHeader, mv: &mut Mv) {
    if h.allow_high_precision_mv {
        return;
    }
    for c in mv.iter_mut() {
        if h.force_integer_mv {
            let a = c.abs();
            let a_int = (a + 3) >> 3;
            *c = if *c > 0 { a_int << 3 } else { -(a_int << 3) };
        } else if *c & 1 != 0 {
            *c += if *c > 0 { -1 } else { 1 };
        }
    }
}

/// get_mv_projection( mv, numerator, denominator ) (§7.9.3)
fn get_mv_projection(mv: Mv, numerator: i32, denominator: i32) -> Mv {
    let clipped_denominator = denominator.min(MAX_FRAME_DISTANCE as i32);
    let clipped_numerator = numerator.clamp(-(MAX_FRAME_DISTANCE as i32), MAX_FRAME_DISTANCE as i32);
    let mut out = [0; 2];
    for i in 0..2 {
        let scaled = round2signed(mv[i] as i64 * clipped_numerator as i64 * DIV_MULT[clipped_denominator as usize] as i64, 14);
        out[i] = (scaled as i32).clamp(-(1 << 14) + 1, (1 << 14) - 1);
    }
    out
}

fn project(v8: i32, delta: i32, dst_sign: i32, max8: i32, max_off8: i32, valid: &mut bool) -> i32 {
    let base8 = (v8 >> 3) << 3;
    let offset8 = if delta >= 0 { delta >> (3 + 1 + MI_SIZE_LOG2) } else { -((-delta) >> (3 + 1 + MI_SIZE_LOG2)) };
    let v = v8 + dst_sign * offset8;
    if v < 0 || v >= max8 || v < base8 - max_off8 || v >= base8 + 8 + max_off8 {
        *valid = false;
    }
    v
}

/// The projection process (§7.9.2). Returns whether the source frame could be used.
fn projection(seq: &SequenceHeader, h: &FrameHeader, refs: &RefStore, fs: &mut FrameState, src: usize, dst_sign: i32) -> bool {
    let src_idx = h.ref_frame_idx[src - LAST_FRAME];
    let w8 = (fs.mi_cols >> 1) as i32;
    let h8 = (fs.mi_rows >> 1) as i32;
    let f = match &refs.frames[src_idx] {
        Some(f) => f.clone(),
        None => return false,
    };
    if f.mi_rows as usize != fs.mi_rows || f.mi_cols as usize != fs.mi_cols || f.frame_type == INTRA_ONLY_FRAME as u8 || f.frame_type == KEY_FRAME as u8 {
        return false;
    }
    for y8 in 0..h8 {
        for x8 in 0..w8 {
            let row = (2 * y8 + 1) as usize;
            let col = (2 * x8 + 1) as usize;
            let k = row * fs.mi_cols + col;
            let src_ref = f.mf_ref_frames[k] as i32;
            if src_ref > INTRA {
                let ref_to_cur = get_relative_dist(seq, h.order_hints[src], h.order_hint);
                let ref_offset = get_relative_dist(seq, h.order_hints[src], f.saved_order_hints[src_ref as usize]);
                let pos_valid = ref_to_cur.abs() <= MAX_FRAME_DISTANCE as i32 && ref_offset.abs() <= MAX_FRAME_DISTANCE as i32 && ref_offset > 0;
                if pos_valid {
                    let mv = f.mf_mvs[k];
                    let proj_mv = get_mv_projection(mv, ref_to_cur * dst_sign, ref_offset);
                    // get_block_position
                    let mut valid = true;
                    let pos_y8 = project(y8, proj_mv[0], dst_sign, h8, MAX_OFFSET_HEIGHT as i32, &mut valid);
                    let pos_x8 = project(x8, proj_mv[1], dst_sign, w8, MAX_OFFSET_WIDTH as i32, &mut valid);
                    if valid {
                        for dst in LAST_FRAME..=ALTREF_FRAME {
                            let ref_to_dst = get_relative_dist(seq, h.order_hint, h.order_hints[dst]);
                            let proj_mv = get_mv_projection(mv, ref_to_dst, ref_offset);
                            fs.motion_field_mvs[dst][(pos_y8 * w8 + pos_x8) as usize] = proj_mv;
                        }
                    }
                }
            }
        }
    }
    true
}

/// motion_field_estimation() (§7.9.1)
pub fn motion_field_estimation(seq: &SequenceHeader, h: &FrameHeader, refs: &RefStore, fs: &mut FrameState) {
    let w8 = fs.mi_cols >> 1;
    let h8 = fs.mi_rows >> 1;
    for rf in LAST_FRAME..=ALTREF_FRAME {
        fs.motion_field_mvs[rf] = vec![[INVALID_MV, INVALID_MV]; w8 * h8];
    }
    let last_idx = h.ref_frame_idx[0];
    let cur_gold_order_hint = h.order_hints[GOLDEN_FRAME];
    let last_alt_order_hint = refs.frames[last_idx].as_ref().map(|f| f.saved_order_hints[ALTREF_FRAME]).unwrap_or(0);
    let use_last = last_alt_order_hint != cur_gold_order_hint;
    if use_last {
        projection(seq, h, refs, fs, LAST_FRAME, -1);
    }
    let mut ref_stamp = MFMV_STACK_SIZE as i32 - 2;
    if get_relative_dist(seq, h.order_hints[BWDREF_FRAME], h.order_hint) > 0 && projection(seq, h, refs, fs, BWDREF_FRAME, 1) {
        ref_stamp -= 1;
    }
    if get_relative_dist(seq, h.order_hints[ALTREF2_FRAME], h.order_hint) > 0 && projection(seq, h, refs, fs, ALTREF2_FRAME, 1) {
        ref_stamp -= 1;
    }
    if get_relative_dist(seq, h.order_hints[ALTREF_FRAME], h.order_hint) > 0 && ref_stamp >= 0 && projection(seq, h, refs, fs, ALTREF_FRAME, 1) {
        ref_stamp -= 1;
    }
    if ref_stamp >= 0 {
        projection(seq, h, refs, fs, LAST2_FRAME, -1);
    }
}

impl<'a, 'f> Dec<'a, 'f> {
    /// find_mv_stack( isCompound ) (§7.10.2)
    pub(crate) fn find_mv_stack(&mut self, is_compound: bool) {
        let bw4 = NUM_4X4_BLOCKS_WIDE[self.mi_size] as usize;
        let bh4 = NUM_4X4_BLOCKS_HIGH[self.mi_size] as usize;
        self.num_mv_found = 0;
        self.new_mv_count = 0;
        self.weight_stack = [0; 10];
        self.global_mvs[0] = self.setup_global_mv(0);
        if is_compound {
            self.global_mvs[1] = self.setup_global_mv(1);
        }
        self.found_match = false;
        self.scan_row(-1, is_compound);
        let mut found_above_match = self.found_match;
        self.found_match = false;
        self.scan_col(-1, is_compound);
        let mut found_left_match = self.found_match;
        self.found_match = false;
        if bw4.max(bh4) <= 16 {
            self.scan_point(-1, bw4 as isize, is_compound);
        }
        if self.found_match {
            found_above_match = true;
        }
        self.close_matches = found_above_match as usize + found_left_match as usize;
        let num_nearest = self.num_mv_found;
        let num_new = self.new_mv_count;
        if num_nearest > 0 {
            for idx in 0..num_nearest {
                self.weight_stack[idx] += REF_CAT_LEVEL as u32;
            }
        }
        self.zero_mv_context = 0;
        if self.hdr.use_ref_frame_mvs {
            self.temporal_scan(is_compound);
        }
        self.scan_point(-1, -1, is_compound);
        if self.found_match {
            found_above_match = true;
        }
        self.found_match = false;
        self.scan_row(-3, is_compound);
        if self.found_match {
            found_above_match = true;
        }
        self.found_match = false;
        self.scan_col(-3, is_compound);
        if self.found_match {
            found_left_match = true;
        }
        self.found_match = false;
        if bh4 > 1 {
            self.scan_row(-5, is_compound);
        }
        if self.found_match {
            found_above_match = true;
        }
        self.found_match = false;
        if bw4 > 1 {
            self.scan_col(-5, is_compound);
        }
        if self.found_match {
            found_left_match = true;
        }
        self.total_matches = found_above_match as usize + found_left_match as usize;
        self.sort_stack(0, num_nearest, is_compound);
        self.sort_stack(num_nearest, self.num_mv_found, is_compound);
        if self.num_mv_found < 2 {
            self.extra_search(is_compound);
        }
        self.context_and_clamping(is_compound, num_new);
    }

    /// The setup global mv process (§7.10.2.1).
    fn setup_global_mv(&self, ref_list: usize) -> Mv {
        let rf = self.ref_frame[ref_list];
        let mut mv: Mv;
        let typ = if rf != INTRA { self.hdr.gm_type[rf as usize] as usize } else { IDENTITY };
        if rf == INTRA || typ == IDENTITY {
            mv = [0, 0];
        } else if typ == TRANSLATION {
            let gm = &self.hdr.gm_params[rf as usize];
            mv = [gm[0] >> (WARPEDMODEL_PREC_BITS - 3), gm[1] >> (WARPEDMODEL_PREC_BITS - 3)];
        } else {
            let gm = &self.hdr.gm_params[rf as usize];
            let bw = block_width(self.mi_size) as i64;
            let bh = block_height(self.mi_size) as i64;
            let x = (self.mi_col * MI_SIZE) as i64 + bw / 2 - 1;
            let y = (self.mi_row * MI_SIZE) as i64 + bh / 2 - 1;
            let xc = (gm[2] as i64 - (1 << WARPEDMODEL_PREC_BITS)) * x + gm[3] as i64 * y + gm[0] as i64;
            let yc = gm[4] as i64 * x + (gm[5] as i64 - (1 << WARPEDMODEL_PREC_BITS)) * y + gm[1] as i64;
            if self.hdr.allow_high_precision_mv {
                mv = [round2signed(yc, WARPEDMODEL_PREC_BITS as u32 - 3) as i32, round2signed(xc, WARPEDMODEL_PREC_BITS as u32 - 3) as i32];
            } else {
                mv = [
                    round2signed(yc, WARPEDMODEL_PREC_BITS as u32 - 2) as i32 * 2,
                    round2signed(xc, WARPEDMODEL_PREC_BITS as u32 - 2) as i32 * 2,
                ];
            }
        }
        lower_mv_precision(self.hdr, &mut mv);
        mv
    }

    fn inside(&self, r: isize, c: isize) -> bool {
        c >= self.mi_col_start as isize && c < self.mi_col_end as isize && r >= self.mi_row_start as isize && r < self.mi_row_end as isize
    }

    /// The scan row process (§7.10.2.2).
    fn scan_row(&mut self, delta_row: isize, is_compound: bool) {
        let bw4 = NUM_4X4_BLOCKS_WIDE[self.mi_size] as usize;
        let end4 = bw4.min(self.fs.mi_cols - self.mi_col).min(16);
        let mut delta_col: isize = 0;
        let use_step16 = bw4 >= 16;
        let mut delta_row = delta_row;
        if delta_row.abs() > 1 {
            delta_row += (self.mi_row & 1) as isize;
            delta_col = 1 - (self.mi_col & 1) as isize;
        }
        let mut i = 0usize;
        while i < end4 {
            let mv_row = self.mi_row as isize + delta_row;
            let mv_col = self.mi_col as isize + delta_col + i as isize;
            if !self.inside(mv_row, mv_col) {
                break;
            }
            let cand_size = self.fs.mi_sizes[self.fs.mi(mv_row as usize, mv_col as usize)] as usize;
            let mut len = bw4.min(NUM_4X4_BLOCKS_WIDE[cand_size] as usize);
            if delta_row.abs() > 1 {
                len = len.max(2);
            }
            if use_step16 {
                len = len.max(4);
            }
            let weight = len as u32 * 2;
            self.add_ref_mv_candidate(mv_row as usize, mv_col as usize, is_compound, weight);
            i += len;
        }
    }

    /// The scan col process (§7.10.2.3).
    fn scan_col(&mut self, delta_col: isize, is_compound: bool) {
        let bh4 = NUM_4X4_BLOCKS_HIGH[self.mi_size] as usize;
        let end4 = bh4.min(self.fs.mi_rows - self.mi_row).min(16);
        let mut delta_row: isize = 0;
        let use_step16 = bh4 >= 16;
        let mut delta_col = delta_col;
        if delta_col.abs() > 1 {
            delta_row = 1 - (self.mi_row & 1) as isize;
            delta_col += (self.mi_col & 1) as isize;
        }
        let mut i = 0usize;
        while i < end4 {
            let mv_row = self.mi_row as isize + delta_row + i as isize;
            let mv_col = self.mi_col as isize + delta_col;
            if !self.inside(mv_row, mv_col) {
                break;
            }
            let cand_size = self.fs.mi_sizes[self.fs.mi(mv_row as usize, mv_col as usize)] as usize;
            let mut len = bh4.min(NUM_4X4_BLOCKS_HIGH[cand_size] as usize);
            if delta_col.abs() > 1 {
                len = len.max(2);
            }
            if use_step16 {
                len = len.max(4);
            }
            let weight = len as u32 * 2;
            self.add_ref_mv_candidate(mv_row as usize, mv_col as usize, is_compound, weight);
            i += len;
        }
    }

    /// The scan point process (§7.10.2.4).
    fn scan_point(&mut self, delta_row: isize, delta_col: isize, is_compound: bool) {
        let mv_row = self.mi_row as isize + delta_row;
        let mv_col = self.mi_col as isize + delta_col;
        if self.inside(mv_row, mv_col) && self.fs.ref_frames[self.fs.mi(mv_row as usize, mv_col as usize)][0] != crate::decode::REF_UNWRITTEN {
            self.add_ref_mv_candidate(mv_row as usize, mv_col as usize, is_compound, 4);
        }
    }

    /// The temporal scan process (§7.10.2.5).
    fn temporal_scan(&mut self, is_compound: bool) {
        let bw4 = NUM_4X4_BLOCKS_WIDE[self.mi_size] as isize;
        let bh4 = NUM_4X4_BLOCKS_HIGH[self.mi_size] as isize;
        let step_w4 = if bw4 >= 16 { 4 } else { 2 };
        let step_h4 = if bh4 >= 16 { 4 } else { 2 };
        let mut delta_row = 0;
        while delta_row < bh4.min(16) {
            let mut delta_col = 0;
            while delta_col < bw4.min(16) {
                self.add_tpl_ref_mv(delta_row, delta_col, is_compound);
                delta_col += step_w4;
            }
            delta_row += step_h4;
        }
        let allow_extension = bh4 >= NUM_4X4_BLOCKS_HIGH[BLOCK_8X8] as isize
            && bh4 < NUM_4X4_BLOCKS_HIGH[BLOCK_64X64] as isize
            && bw4 >= NUM_4X4_BLOCKS_WIDE[BLOCK_8X8] as isize
            && bw4 < NUM_4X4_BLOCKS_WIDE[BLOCK_64X64] as isize;
        if allow_extension {
            let pos = [[bh4, -2], [bh4, bw4], [bh4 - 2, bw4]];
            for p in pos {
                let row = (self.mi_row & 15) as isize + p[0];
                let col = (self.mi_col & 15) as isize + p[1];
                if row >= 0 && row < 16 && col >= 0 && col < 16 {
                    self.add_tpl_ref_mv(p[0], p[1], is_compound);
                }
            }
        }
    }

    /// The temporal sample process (§7.10.2.6).
    fn add_tpl_ref_mv(&mut self, delta_row: isize, delta_col: isize, is_compound: bool) {
        let mv_row = (self.mi_row as isize + delta_row) | 1;
        let mv_col = (self.mi_col as isize + delta_col) | 1;
        if !self.inside(mv_row, mv_col) {
            return;
        }
        let x8 = (mv_col >> 1) as usize;
        let y8 = (mv_row >> 1) as usize;
        let w8 = self.fs.mi_cols >> 1;
        let k = y8 * w8 + x8;
        if delta_row == 0 && delta_col == 0 {
            self.zero_mv_context = 1;
        }
        self.fs.stats.temporal_mvs = true;
        if !is_compound {
            let mut cand_mv = self.fs.motion_field_mvs[self.ref_frame[0] as usize][k];
            if cand_mv[0] == INVALID_MV {
                return;
            }
            lower_mv_precision(self.hdr, &mut cand_mv);
            if delta_row == 0 && delta_col == 0 {
                if (cand_mv[0] - self.global_mvs[0][0]).abs() >= 16 || (cand_mv[1] - self.global_mvs[0][1]).abs() >= 16 {
                    self.zero_mv_context = 1;
                } else {
                    self.zero_mv_context = 0;
                }
            }
            let mut idx = 0;
            while idx < self.num_mv_found {
                if cand_mv == self.ref_stack_mv[idx][0] {
                    break;
                }
                idx += 1;
            }
            if idx < self.num_mv_found {
                self.weight_stack[idx] += 2;
            } else if self.num_mv_found < MAX_REF_MV_STACK_SIZE {
                self.ref_stack_mv[self.num_mv_found][0] = cand_mv;
                self.weight_stack[self.num_mv_found] = 2;
                self.num_mv_found += 1;
            }
        } else {
            let mut cand_mv0 = self.fs.motion_field_mvs[self.ref_frame[0] as usize][k];
            if cand_mv0[0] == INVALID_MV {
                return;
            }
            let mut cand_mv1 = self.fs.motion_field_mvs[self.ref_frame[1] as usize][k];
            if cand_mv1[0] == INVALID_MV {
                return;
            }
            lower_mv_precision(self.hdr, &mut cand_mv0);
            lower_mv_precision(self.hdr, &mut cand_mv1);
            if delta_row == 0 && delta_col == 0 {
                if (cand_mv0[0] - self.global_mvs[0][0]).abs() >= 16
                    || (cand_mv0[1] - self.global_mvs[0][1]).abs() >= 16
                    || (cand_mv1[0] - self.global_mvs[1][0]).abs() >= 16
                    || (cand_mv1[1] - self.global_mvs[1][1]).abs() >= 16
                {
                    self.zero_mv_context = 1;
                } else {
                    self.zero_mv_context = 0;
                }
            }
            let mut idx = 0;
            while idx < self.num_mv_found {
                if cand_mv0 == self.ref_stack_mv[idx][0] && cand_mv1 == self.ref_stack_mv[idx][1] {
                    break;
                }
                idx += 1;
            }
            if idx < self.num_mv_found {
                self.weight_stack[idx] += 2;
            } else if self.num_mv_found < MAX_REF_MV_STACK_SIZE {
                self.ref_stack_mv[self.num_mv_found] = [cand_mv0, cand_mv1];
                self.weight_stack[self.num_mv_found] = 2;
                self.num_mv_found += 1;
            }
        }
    }

    /// The add reference motion vector process (§7.10.2.7).
    fn add_ref_mv_candidate(&mut self, mv_row: usize, mv_col: usize, is_compound: bool, weight: u32) {
        let i = self.fs.mi(mv_row, mv_col);
        if self.fs.is_inters[i] == 0 {
            return;
        }
        let rf = self.fs.ref_frames[i];
        if !is_compound {
            for cand_list in 0..2 {
                if rf[cand_list] as i32 == self.ref_frame[0] {
                    self.search_stack(mv_row, mv_col, cand_list, weight);
                }
            }
        } else if rf[0] as i32 == self.ref_frame[0] && rf[1] as i32 == self.ref_frame[1] {
            self.compound_search_stack(mv_row, mv_col, weight);
        }
    }

    /// The search stack process (§7.10.2.8).
    fn search_stack(&mut self, mv_row: usize, mv_col: usize, cand_list: usize, weight: u32) {
        let i = self.fs.mi(mv_row, mv_col);
        let cand_mode = self.fs.y_modes[i] as usize;
        let cand_size = self.fs.mi_sizes[i] as usize;
        let large = block_width(cand_size).min(block_height(cand_size)) >= 8;
        let mut cand_mv = if (cand_mode == GLOBALMV || cand_mode == GLOBAL_GLOBALMV)
            && self.hdr.gm_type[self.ref_frame[0] as usize] > TRANSLATION as u8
            && large
        {
            self.global_mvs[0]
        } else {
            self.fs.mvs[i][cand_list]
        };
        lower_mv_precision(self.hdr, &mut cand_mv);
        if has_newmv(cand_mode) {
            self.new_mv_count += 1;
        }
        self.found_match = true;
        let mut idx = 0;
        while idx < self.num_mv_found {
            if cand_mv == self.ref_stack_mv[idx][0] {
                break;
            }
            idx += 1;
        }
        if idx < self.num_mv_found {
            self.weight_stack[idx] += weight;
        } else if self.num_mv_found < MAX_REF_MV_STACK_SIZE {
            self.ref_stack_mv[self.num_mv_found][0] = cand_mv;
            self.weight_stack[self.num_mv_found] = weight;
            self.num_mv_found += 1;
        }
    }

    /// The compound search stack process (§7.10.2.9).
    fn compound_search_stack(&mut self, mv_row: usize, mv_col: usize, weight: u32) {
        let i = self.fs.mi(mv_row, mv_col);
        let mut cand_mvs = self.fs.mvs[i];
        let cand_mode = self.fs.y_modes[i] as usize;
        let cand_size = self.fs.mi_sizes[i] as usize;
        if cand_mode == GLOBAL_GLOBALMV {
            for ref_list in 0..2 {
                if self.hdr.gm_type[self.ref_frame[ref_list] as usize] > TRANSLATION as u8 {
                    cand_mvs[ref_list] = self.global_mvs[ref_list];
                }
            }
        }
        let _ = cand_size;
        for m in cand_mvs.iter_mut() {
            lower_mv_precision(self.hdr, m);
        }
        self.found_match = true;
        let mut idx = 0;
        while idx < self.num_mv_found {
            if cand_mvs[0] == self.ref_stack_mv[idx][0] && cand_mvs[1] == self.ref_stack_mv[idx][1] {
                break;
            }
            idx += 1;
        }
        if idx < self.num_mv_found {
            self.weight_stack[idx] += weight;
        } else if self.num_mv_found < MAX_REF_MV_STACK_SIZE {
            self.ref_stack_mv[self.num_mv_found] = cand_mvs;
            self.weight_stack[self.num_mv_found] = weight;
            self.num_mv_found += 1;
        }
        if has_newmv(cand_mode) {
            self.new_mv_count += 1;
        }
    }

    /// The sorting process (§7.10.2.11).
    fn sort_stack(&mut self, start: usize, end: usize, is_compound: bool) {
        let mut end = end;
        while end > start {
            let mut new_end = start;
            for idx in start + 1..end {
                if self.weight_stack[idx - 1] < self.weight_stack[idx] {
                    self.weight_stack.swap(idx - 1, idx);
                    for list in 0..1 + is_compound as usize {
                        let t = self.ref_stack_mv[idx - 1][list];
                        self.ref_stack_mv[idx - 1][list] = self.ref_stack_mv[idx][list];
                        self.ref_stack_mv[idx][list] = t;
                    }
                    new_end = idx;
                }
            }
            end = new_end;
        }
    }

    /// The extra search process (§7.10.2.12).
    fn extra_search(&mut self, is_compound: bool) {
        let mut ref_id_count = [0usize; 2];
        let mut ref_diff_count = [0usize; 2];
        let mut ref_id_mvs = [[[0i32; 2]; 2]; 2];
        let mut ref_diff_mvs = [[[0i32; 2]; 2]; 2];
        let mut w4 = (NUM_4X4_BLOCKS_WIDE[self.mi_size] as usize).min(16);
        let mut h4 = (NUM_4X4_BLOCKS_HIGH[self.mi_size] as usize).min(16);
        w4 = w4.min(self.fs.mi_cols - self.mi_col);
        h4 = h4.min(self.fs.mi_rows - self.mi_row);
        let num4x4 = w4.min(h4);
        for pass in 0..2 {
            let mut idx = 0;
            while idx < num4x4 && self.num_mv_found < 2 {
                let (mv_row, mv_col) = if pass == 0 {
                    (self.mi_row as isize - 1, (self.mi_col + idx) as isize)
                } else {
                    ((self.mi_row + idx) as isize, self.mi_col as isize - 1)
                };
                if !self.inside(mv_row, mv_col) {
                    break;
                }
                let (mr, mc) = (mv_row as usize, mv_col as usize);
                // add_extra_mv_candidate (§7.10.2.13)
                let i = self.fs.mi(mr, mc);
                let rf = self.fs.ref_frames[i];
                if is_compound {
                    for cand_list in 0..2 {
                        let cand_ref = rf[cand_list] as i32;
                        if cand_ref > INTRA {
                            for list in 0..2 {
                                let mut cand_mv = self.fs.mvs[i][cand_list];
                                if cand_ref == self.ref_frame[list] && ref_id_count[list] < 2 {
                                    ref_id_mvs[list][ref_id_count[list]] = cand_mv;
                                    ref_id_count[list] += 1;
                                } else if ref_diff_count[list] < 2 {
                                    if self.hdr.ref_frame_sign_bias[cand_ref as usize] != self.hdr.ref_frame_sign_bias[self.ref_frame[list] as usize] {
                                        cand_mv[0] *= -1;
                                        cand_mv[1] *= -1;
                                    }
                                    ref_diff_mvs[list][ref_diff_count[list]] = cand_mv;
                                    ref_diff_count[list] += 1;
                                }
                            }
                        }
                    }
                } else {
                    for cand_list in 0..2 {
                        let cand_ref = rf[cand_list] as i32;
                        if cand_ref > INTRA {
                            let mut cand_mv = self.fs.mvs[i][cand_list];
                            if self.hdr.ref_frame_sign_bias[cand_ref as usize] != self.hdr.ref_frame_sign_bias[self.ref_frame[0] as usize] {
                                cand_mv[0] *= -1;
                                cand_mv[1] *= -1;
                            }
                            let mut k = 0;
                            while k < self.num_mv_found {
                                if cand_mv == self.ref_stack_mv[k][0] {
                                    break;
                                }
                                k += 1;
                            }
                            if k == self.num_mv_found {
                                self.ref_stack_mv[k][0] = cand_mv;
                                self.weight_stack[k] = 2;
                                self.num_mv_found += 1;
                            }
                        }
                    }
                }
                if pass == 0 {
                    idx += NUM_4X4_BLOCKS_WIDE[self.fs.mi_sizes[i] as usize] as usize;
                } else {
                    idx += NUM_4X4_BLOCKS_HIGH[self.fs.mi_sizes[i] as usize] as usize;
                }
            }
        }
        if is_compound {
            let mut combined_mvs = [[[0i32; 2]; 2]; 2];
            for list in 0..2 {
                let mut comp_count = 0;
                for idx in 0..ref_id_count[list] {
                    combined_mvs[comp_count][list] = ref_id_mvs[list][idx];
                    comp_count += 1;
                }
                let mut idx = 0;
                while idx < ref_diff_count[list] && comp_count < 2 {
                    combined_mvs[comp_count][list] = ref_diff_mvs[list][idx];
                    comp_count += 1;
                    idx += 1;
                }
                while comp_count < 2 {
                    combined_mvs[comp_count][list] = self.global_mvs[list];
                    comp_count += 1;
                }
            }
            if self.num_mv_found == 1 {
                if combined_mvs[0][0] == self.ref_stack_mv[0][0] && combined_mvs[0][1] == self.ref_stack_mv[0][1] {
                    self.ref_stack_mv[self.num_mv_found] = combined_mvs[1];
                } else {
                    self.ref_stack_mv[self.num_mv_found] = combined_mvs[0];
                }
                self.weight_stack[self.num_mv_found] = 2;
                self.num_mv_found += 1;
            } else {
                for idx in 0..2 {
                    self.ref_stack_mv[self.num_mv_found] = combined_mvs[idx];
                    self.weight_stack[self.num_mv_found] = 2;
                    self.num_mv_found += 1;
                }
            }
        } else {
            for idx in self.num_mv_found..2 {
                self.ref_stack_mv[idx][0] = self.global_mvs[0];
            }
        }
    }

    fn clamp_mv_row(&self, mvec: i32, border: i32) -> i32 {
        let bh4 = NUM_4X4_BLOCKS_HIGH[self.mi_size] as i32;
        let mb_to_top_edge = -((self.mi_row as i32 * MI_SIZE as i32) * 8);
        let mb_to_bottom_edge = ((self.fs.mi_rows as i32 - bh4 - self.mi_row as i32) * MI_SIZE as i32) * 8;
        mvec.clamp(mb_to_top_edge - border, mb_to_bottom_edge + border)
    }
    fn clamp_mv_col(&self, mvec: i32, border: i32) -> i32 {
        let bw4 = NUM_4X4_BLOCKS_WIDE[self.mi_size] as i32;
        let mb_to_left_edge = -((self.mi_col as i32 * MI_SIZE as i32) * 8);
        let mb_to_right_edge = ((self.fs.mi_cols as i32 - bw4 - self.mi_col as i32) * MI_SIZE as i32) * 8;
        mvec.clamp(mb_to_left_edge - border, mb_to_right_edge + border)
    }

    /// The context and clamping process (§7.10.2.14).
    fn context_and_clamping(&mut self, is_compound: bool, num_new: usize) {
        let bw = block_width(self.mi_size) as i32;
        let bh = block_height(self.mi_size) as i32;
        let num_lists = if is_compound { 2 } else { 1 };
        for idx in 0..self.num_mv_found {
            let mut z = 0;
            if idx + 1 < self.num_mv_found {
                let w0 = self.weight_stack[idx];
                let w1 = self.weight_stack[idx + 1];
                if w0 >= REF_CAT_LEVEL as u32 {
                    if w1 < REF_CAT_LEVEL as u32 {
                        z = 1;
                    }
                } else {
                    z = 2;
                }
            }
            self.drl_ctx_stack[idx] = z;
        }
        for list in 0..num_lists {
            for idx in 0..self.num_mv_found {
                let mut ref_mv = self.ref_stack_mv[idx][list];
                ref_mv[0] = self.clamp_mv_row(ref_mv[0], MV_BORDER as i32 + bh * 8);
                ref_mv[1] = self.clamp_mv_col(ref_mv[1], MV_BORDER as i32 + bw * 8);
                self.ref_stack_mv[idx][list] = ref_mv;
            }
        }
        if self.close_matches == 0 {
            self.new_mv_context = self.total_matches.min(1);
            self.ref_mv_context = self.total_matches;
        } else if self.close_matches == 1 {
            self.new_mv_context = 3 - num_new.min(1);
            self.ref_mv_context = 2 + self.total_matches;
        } else {
            self.new_mv_context = 5 - num_new.min(1);
            self.ref_mv_context = 5;
        }
    }

    /// has_overlappable_candidates() (§7.10.3)
    pub(crate) fn has_overlappable_candidates(&self) -> bool {
        if self.avail_u {
            let w4 = NUM_4X4_BLOCKS_WIDE[self.mi_size] as usize;
            let mut x4 = self.mi_col;
            while x4 < self.fs.mi_cols.min(self.mi_col + w4) {
                let c = (x4 | 1).min(self.fs.mi_cols - 1);
                if self.fs.ref_frames[self.fs.mi(self.mi_row - 1, c)][0] as i32 > INTRA {
                    return true;
                }
                x4 += 2;
            }
        }
        if self.avail_l {
            let h4 = NUM_4X4_BLOCKS_HIGH[self.mi_size] as usize;
            let mut y4 = self.mi_row;
            while y4 < self.fs.mi_rows.min(self.mi_row + h4) {
                let r = (y4 | 1).min(self.fs.mi_rows - 1);
                if self.fs.ref_frames[self.fs.mi(r, self.mi_col - 1)][0] as i32 > INTRA {
                    return true;
                }
                y4 += 2;
            }
        }
        false
    }

    /// find_warp_samples() (§7.10.4)
    pub(crate) fn find_warp_samples(&mut self) {
        self.num_samples = 0;
        self.num_samples_scanned = 0;
        let w4 = NUM_4X4_BLOCKS_WIDE[self.mi_size] as usize;
        let h4 = NUM_4X4_BLOCKS_HIGH[self.mi_size] as usize;
        let mut do_top_left = true;
        let mut do_top_right = true;
        if self.avail_u {
            let src_size = self.fs.mi_sizes[self.fs.mi(self.mi_row - 1, self.mi_col)] as usize;
            let src_w = NUM_4X4_BLOCKS_WIDE[src_size] as usize;
            if w4 <= src_w {
                let col_offset = -((self.mi_col & (src_w - 1)) as isize);
                if col_offset < 0 {
                    do_top_left = false;
                }
                if col_offset + src_w as isize > w4 as isize {
                    do_top_right = false;
                }
                self.add_sample(-1, 0);
            } else {
                let mut i = 0;
                while i < w4.min(self.fs.mi_cols - self.mi_col) {
                    let src_size = self.fs.mi_sizes[self.fs.mi(self.mi_row - 1, self.mi_col + i)] as usize;
                    let src_w = NUM_4X4_BLOCKS_WIDE[src_size] as usize;
                    let mi_step = w4.min(src_w);
                    self.add_sample(-1, i as isize);
                    i += mi_step;
                }
            }
        }
        if self.avail_l {
            let src_size = self.fs.mi_sizes[self.fs.mi(self.mi_row, self.mi_col - 1)] as usize;
            let src_h = NUM_4X4_BLOCKS_HIGH[src_size] as usize;
            if h4 <= src_h {
                let row_offset = -((self.mi_row & (src_h - 1)) as isize);
                if row_offset < 0 {
                    do_top_left = false;
                }
                self.add_sample(0, -1);
            } else {
                let mut i = 0;
                while i < h4.min(self.fs.mi_rows - self.mi_row) {
                    let src_size = self.fs.mi_sizes[self.fs.mi(self.mi_row + i, self.mi_col - 1)] as usize;
                    let src_h = NUM_4X4_BLOCKS_HIGH[src_size] as usize;
                    let mi_step = h4.min(src_h);
                    self.add_sample(i as isize, -1);
                    i += mi_step;
                }
            }
        }
        if do_top_left {
            self.add_sample(-1, -1);
        }
        if do_top_right && w4.max(h4) <= 16 {
            self.add_sample(-1, w4 as isize);
        }
        if self.num_samples == 0 && self.num_samples_scanned > 0 {
            self.num_samples = 1;
        }
    }

    /// The add sample process (§7.10.4.2).
    fn add_sample(&mut self, delta_row: isize, delta_col: isize) {
        if self.num_samples_scanned >= LEAST_SQUARES_SAMPLES_MAX {
            return;
        }
        let mv_row = self.mi_row as isize + delta_row;
        let mv_col = self.mi_col as isize + delta_col;
        if !self.inside(mv_row, mv_col) {
            return;
        }
        let i = self.fs.mi(mv_row as usize, mv_col as usize);
        let rf = self.fs.ref_frames[i];
        if rf[0] == crate::decode::REF_UNWRITTEN {
            return;
        }
        if rf[0] as i32 != self.ref_frame[0] {
            return;
        }
        if rf[1] as i32 != NONE as i32 {
            return;
        }
        let cand_sz = self.fs.mi_sizes[i] as usize;
        let cand_w4 = NUM_4X4_BLOCKS_WIDE[cand_sz] as isize;
        let cand_h4 = NUM_4X4_BLOCKS_HIGH[cand_sz] as isize;
        let cand_row = mv_row & !(cand_h4 - 1);
        let cand_col = mv_col & !(cand_w4 - 1);
        let mid_y = (cand_row * 4 + cand_h4 * 2 - 1) as i32;
        let mid_x = (cand_col * 4 + cand_w4 * 2 - 1) as i32;
        let threshold = (block_width(self.mi_size).max(block_height(self.mi_size)) as i32).clamp(16, 112);
        let cmv = self.fs.mvs[self.fs.mi(cand_row as usize, cand_col as usize)][0];
        let mv_diff_row = (cmv[0] - self.mv[0][0]).abs();
        let mv_diff_col = (cmv[1] - self.mv[0][1]).abs();
        let valid = mv_diff_row + mv_diff_col <= threshold;
        let cand = [mid_y * 8, mid_x * 8, mid_y * 8 + cmv[0], mid_x * 8 + cmv[1]];
        self.num_samples_scanned += 1;
        if !valid && self.num_samples_scanned > 1 {
            return;
        }
        self.cand_list[self.num_samples] = cand;
        if valid {
            self.num_samples += 1;
        }
    }
}
