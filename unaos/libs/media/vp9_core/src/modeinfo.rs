// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Per-block mode info (spec §6.4.5 intra_frame_mode_info, §6.4.11 inter_frame_mode_info) and the
//! contexts it is read with (§9.3.1: skip, tx_size, is_inter, comp_mode, comp_ref, single_ref_p1/p2,
//! interp_filter, segment-id prediction, the inter-mode context), plus motion vector prediction
//! (§6.4.22 find_mv_refs / append_sub8x8_mvs: the eight neighbour positions per block size, the
//! previous frame's co-located vector, sign-bias inversion, clamping to the frame + 16 samples).

use crate::consts::*;
use crate::frame::{Mv, MvRef};

#[derive(Clone, Copy, Debug)]
pub struct ModeInfo {
    pub sb_type: u8,
    /// Luma mode (for sub-8x8 blocks the last sub-block's mode).
    pub mode: u8,
    pub uv_mode: u8,
    pub sub_modes: [u8; 4],
    pub ref_frame: [i8; 2],
    pub mv: [Mv; 2],
    pub sub_mvs: [[Mv; 2]; 4],
    pub interp_filter: u8,
    pub tx_size: u8,
    pub skip: bool,
    pub segment_id: u8,
    pub seg_id_predicted: bool,
}

impl Default for ModeInfo {
    fn default() -> Self {
        ModeInfo {
            sb_type: BLOCK_8X8,
            mode: DC_PRED,
            uv_mode: DC_PRED,
            sub_modes: [DC_PRED; 4],
            ref_frame: [INTRA_FRAME, NONE_FRAME],
            mv: [Mv::ZERO; 2],
            sub_mvs: [[Mv::ZERO; 2]; 4],
            interp_filter: 3,
            tx_size: TX_4X4,
            skip: false,
            segment_id: 0,
            seg_id_predicted: false,
        }
    }
}

impl ModeInfo {
    #[inline]
    pub fn is_inter(&self) -> bool {
        self.ref_frame[0] > INTRA_FRAME
    }
    #[inline]
    pub fn has_second(&self) -> bool {
        self.ref_frame[1] > INTRA_FRAME
    }
    /// get_y_mode: the sub-block's mode for sub-8x8 blocks, else the block mode.
    pub fn y_mode(&self, b: usize) -> u8 {
        if self.sb_type < BLOCK_8X8 { self.sub_modes[b] } else { self.mode }
    }
}

/// The neighbours a context is read from (None = outside the frame / tile).
#[derive(Clone, Copy)]
pub struct Neighbours<'a> {
    pub above: Option<&'a ModeInfo>,
    pub left: Option<&'a ModeInfo>,
}

impl Neighbours<'_> {
    pub fn skip_ctx(&self) -> usize {
        self.above.map_or(0, |m| m.skip as usize) + self.left.map_or(0, |m| m.skip as usize)
    }
    pub fn seg_pred_ctx(&self) -> usize {
        self.above.map_or(0, |m| m.seg_id_predicted as usize) + self.left.map_or(0, |m| m.seg_id_predicted as usize)
    }
    pub fn tx_ctx(&self, max_tx: u8) -> usize {
        let mut a = match self.above {
            Some(m) if !m.skip => m.tx_size,
            _ => max_tx,
        };
        let mut l = match self.left {
            Some(m) if !m.skip => m.tx_size,
            _ => max_tx,
        };
        if self.left.is_none() {
            l = a;
        }
        if self.above.is_none() {
            a = l;
        }
        ((a + l) > max_tx) as usize
    }
    pub fn intra_inter_ctx(&self) -> usize {
        match (self.above, self.left) {
            (Some(a), Some(l)) => {
                let (ai, li) = (!a.is_inter(), !l.is_inter());
                if ai && li { 3 } else { (ai || li) as usize }
            }
            (Some(e), None) | (None, Some(e)) => 2 * (!e.is_inter()) as usize,
            (None, None) => 0,
        }
    }
    pub fn interp_ctx(&self) -> usize {
        let l = self.left.map_or(3, |m| m.interp_filter);
        let a = self.above.map_or(3, |m| m.interp_filter);
        if l == a {
            l as usize
        } else if l == 3 {
            a as usize
        } else if a == 3 {
            l as usize
        } else {
            3
        }
    }
    pub fn comp_mode_ctx(&self, fixed: i8) -> usize {
        match (self.above, self.left) {
            (Some(a), Some(l)) => {
                if !a.has_second() && !l.has_second() {
                    ((a.ref_frame[0] == fixed) ^ (l.ref_frame[0] == fixed)) as usize
                } else if !a.has_second() {
                    2 + (a.ref_frame[0] == fixed || !a.is_inter()) as usize
                } else if !l.has_second() {
                    2 + (l.ref_frame[0] == fixed || !l.is_inter()) as usize
                } else {
                    4
                }
            }
            (Some(e), None) | (None, Some(e)) => {
                if !e.has_second() { (e.ref_frame[0] == fixed) as usize } else { 3 }
            }
            (None, None) => 1,
        }
    }
    pub fn comp_ref_ctx(&self, fixed: i8, var: [i8; 2], sign_bias: &[bool; 4]) -> usize {
        let fix_ref_idx = sign_bias[fixed as usize] as usize;
        let var_ref_idx = 1 - fix_ref_idx;
        match (self.above, self.left) {
            (Some(a), Some(l)) => {
                let (ai, li) = (!a.is_inter(), !l.is_inter());
                if ai && li {
                    2
                } else if ai || li {
                    let e = if ai { l } else { a };
                    if !e.has_second() {
                        1 + 2 * (e.ref_frame[0] != var[1]) as usize
                    } else {
                        1 + 2 * (e.ref_frame[var_ref_idx] != var[1]) as usize
                    }
                } else {
                    let l_sg = !l.has_second();
                    let a_sg = !a.has_second();
                    let vrfa = if a_sg { a.ref_frame[0] } else { a.ref_frame[var_ref_idx] };
                    let vrfl = if l_sg { l.ref_frame[0] } else { l.ref_frame[var_ref_idx] };
                    if vrfa == vrfl && var[1] == vrfa {
                        0
                    } else if l_sg && a_sg {
                        if (vrfa == fixed && vrfl == var[0]) || (vrfl == fixed && vrfa == var[0]) {
                            4
                        } else if vrfa == vrfl {
                            3
                        } else {
                            1
                        }
                    } else if l_sg || a_sg {
                        let vrfc = if l_sg { vrfa } else { vrfl };
                        let rfs = if a_sg { vrfa } else { vrfl };
                        if vrfc == var[1] && rfs != var[1] {
                            1
                        } else if rfs == var[1] && vrfc != var[1] {
                            2
                        } else {
                            4
                        }
                    } else if vrfa == vrfl {
                        4
                    } else {
                        2
                    }
                }
            }
            (Some(e), None) | (None, Some(e)) => {
                if !e.is_inter() {
                    2
                } else if e.has_second() {
                    4 * (e.ref_frame[var_ref_idx] != var[1]) as usize
                } else {
                    3 * (e.ref_frame[0] != var[1]) as usize
                }
            }
            (None, None) => 2,
        }
    }
    pub fn single_ref_p1_ctx(&self) -> usize {
        match (self.above, self.left) {
            (Some(a), Some(l)) => {
                let (ai, li) = (!a.is_inter(), !l.is_inter());
                if ai && li {
                    2
                } else if ai || li {
                    let e = if ai { l } else { a };
                    if !e.has_second() {
                        4 * (e.ref_frame[0] == LAST_FRAME) as usize
                    } else {
                        1 + (e.ref_frame[0] == LAST_FRAME || e.ref_frame[1] == LAST_FRAME) as usize
                    }
                } else {
                    let (a2, l2) = (a.has_second(), l.has_second());
                    let (a0, a1, l0, l1) = (a.ref_frame[0], a.ref_frame[1], l.ref_frame[0], l.ref_frame[1]);
                    if a2 && l2 {
                        1 + (a0 == LAST_FRAME || a1 == LAST_FRAME || l0 == LAST_FRAME || l1 == LAST_FRAME) as usize
                    } else if a2 || l2 {
                        let rfs = if !a2 { a0 } else { l0 };
                        let crf1 = if a2 { a0 } else { l0 };
                        let crf2 = if a2 { a1 } else { l1 };
                        if rfs == LAST_FRAME {
                            3 + (crf1 == LAST_FRAME || crf2 == LAST_FRAME) as usize
                        } else {
                            (crf1 == LAST_FRAME || crf2 == LAST_FRAME) as usize
                        }
                    } else {
                        2 * (a0 == LAST_FRAME) as usize + 2 * (l0 == LAST_FRAME) as usize
                    }
                }
            }
            (Some(e), None) | (None, Some(e)) => {
                if !e.is_inter() {
                    2
                } else if !e.has_second() {
                    4 * (e.ref_frame[0] == LAST_FRAME) as usize
                } else {
                    1 + (e.ref_frame[0] == LAST_FRAME || e.ref_frame[1] == LAST_FRAME) as usize
                }
            }
            (None, None) => 2,
        }
    }
    pub fn single_ref_p2_ctx(&self) -> usize {
        match (self.above, self.left) {
            (Some(a), Some(l)) => {
                let (ai, li) = (!a.is_inter(), !l.is_inter());
                if ai && li {
                    2
                } else if ai || li {
                    let e = if ai { l } else { a };
                    if !e.has_second() {
                        if e.ref_frame[0] == LAST_FRAME { 3 } else { 4 * (e.ref_frame[0] == GOLDEN_FRAME) as usize }
                    } else {
                        1 + 2 * (e.ref_frame[0] == GOLDEN_FRAME || e.ref_frame[1] == GOLDEN_FRAME) as usize
                    }
                } else {
                    let (a2, l2) = (a.has_second(), l.has_second());
                    let (a0, a1, l0, l1) = (a.ref_frame[0], a.ref_frame[1], l.ref_frame[0], l.ref_frame[1]);
                    if a2 && l2 {
                        if a0 == l0 && a1 == l1 {
                            3 * (a0 == GOLDEN_FRAME || a1 == GOLDEN_FRAME || l0 == GOLDEN_FRAME || l1 == GOLDEN_FRAME) as usize
                        } else {
                            2
                        }
                    } else if a2 || l2 {
                        let rfs = if !a2 { a0 } else { l0 };
                        let crf1 = if a2 { a0 } else { l0 };
                        let crf2 = if a2 { a1 } else { l1 };
                        if rfs == GOLDEN_FRAME {
                            3 + (crf1 == GOLDEN_FRAME || crf2 == GOLDEN_FRAME) as usize
                        } else if rfs == ALTREF_FRAME {
                            (crf1 == GOLDEN_FRAME || crf2 == GOLDEN_FRAME) as usize
                        } else {
                            1 + 2 * (crf1 == GOLDEN_FRAME || crf2 == GOLDEN_FRAME) as usize
                        }
                    } else if a0 == LAST_FRAME && l0 == LAST_FRAME {
                        3
                    } else if a0 == LAST_FRAME || l0 == LAST_FRAME {
                        let edge0 = if a0 == LAST_FRAME { l0 } else { a0 };
                        4 * (edge0 == GOLDEN_FRAME) as usize
                    } else {
                        2 * (a0 == GOLDEN_FRAME) as usize + 2 * (l0 == GOLDEN_FRAME) as usize
                    }
                }
            }
            (Some(e), None) | (None, Some(e)) => {
                if !e.is_inter() || (e.ref_frame[0] == LAST_FRAME && !e.has_second()) {
                    2
                } else if !e.has_second() {
                    4 * (e.ref_frame[0] == GOLDEN_FRAME) as usize
                } else {
                    3 * (e.ref_frame[0] == GOLDEN_FRAME || e.ref_frame[1] == GOLDEN_FRAME) as usize
                }
            }
            (None, None) => 2,
        }
    }
}

/// vp9_above_block_mode / vp9_left_block_mode for key-frame sub-block mode contexts.
pub fn above_block_mode(cur: &ModeInfo, above: Option<&ModeInfo>, b: usize) -> u8 {
    if b == 0 || b == 1 {
        match above {
            Some(m) if !m.is_inter() => m.y_mode(b + 2),
            _ => DC_PRED,
        }
    } else {
        cur.sub_modes[b - 2]
    }
}

pub fn left_block_mode(cur: &ModeInfo, left: Option<&ModeInfo>, b: usize) -> u8 {
    if b == 0 || b == 2 {
        match left {
            Some(m) if !m.is_inter() => m.y_mode(b + 1),
            _ => DC_PRED,
        }
    } else {
        cur.sub_modes[b - 1]
    }
}

/// mv_ref_blocks: the candidate positions (row, col) in 8x8 units per block size.
pub const MV_REF_BLOCKS: [[(i32, i32); 8]; 13] = [
    [(-1, 0), (0, -1), (-1, -1), (-2, 0), (0, -2), (-2, -1), (-1, -2), (-2, -2)],
    [(-1, 0), (0, -1), (-1, -1), (-2, 0), (0, -2), (-2, -1), (-1, -2), (-2, -2)],
    [(-1, 0), (0, -1), (-1, -1), (-2, 0), (0, -2), (-2, -1), (-1, -2), (-2, -2)],
    [(-1, 0), (0, -1), (-1, -1), (-2, 0), (0, -2), (-2, -1), (-1, -2), (-2, -2)],
    [(0, -1), (-1, 0), (1, -1), (-1, -1), (0, -2), (-2, 0), (-2, -1), (-1, -2)],
    [(-1, 0), (0, -1), (-1, 1), (-1, -1), (-2, 0), (0, -2), (-1, -2), (-2, -1)],
    [(-1, 0), (0, -1), (-1, 1), (1, -1), (-1, -1), (-3, 0), (0, -3), (-3, -3)],
    [(0, -1), (-1, 0), (2, -1), (-1, -1), (-1, 1), (0, -3), (-3, 0), (-3, -3)],
    [(-1, 0), (0, -1), (-1, 2), (-1, -1), (1, -1), (-3, 0), (0, -3), (-3, -3)],
    [(-1, 1), (1, -1), (-1, 2), (2, -1), (-1, -1), (-3, 0), (0, -3), (-3, -3)],
    [(0, -1), (-1, 0), (4, -1), (-1, 2), (-1, -1), (0, -3), (-3, 0), (2, -1)],
    [(-1, 0), (0, -1), (-1, 4), (2, -1), (-1, -1), (-3, 0), (0, -3), (-1, 2)],
    [(-1, 3), (3, -1), (-1, 4), (4, -1), (-1, -1), (-1, 0), (0, -1), (-1, 6)],
];

const MODE_2_COUNTER: [usize; 14] = [9, 9, 9, 9, 9, 9, 9, 9, 9, 9, 0, 0, 3, 1];
const COUNTER_TO_CONTEXT: [usize; 19] = [2, 3, 4, 1, 3, 9, 0, 9, 9, 5, 5, 9, 5, 9, 9, 9, 9, 9, 6];
const IDX_N_COLUMN_TO_SUBBLOCK: [[usize; 2]; 4] = [[1, 2], [1, 3], [3, 2], [3, 3]];

/// What the MV search needs about the current block and frame.
pub struct MvSearch<'a> {
    pub mis: &'a [ModeInfo],
    /// mi_rows x mi_cols grid of indices into `mis` (u32::MAX = not decoded).
    pub grid: &'a [u32],
    pub mi_rows: usize,
    pub mi_cols: usize,
    pub tile_col_start: usize,
    pub tile_col_end: usize,
    pub mi_row: usize,
    pub mi_col: usize,
    pub bsize: u8,
    /// The previous frame's co-located motion, when usable.
    pub prev: Option<MvRef>,
    pub sign_bias: [bool; 4],
    /// mb_to_*_edge in 1/8 samples.
    pub to_left: i32,
    pub to_right: i32,
    pub to_top: i32,
    pub to_bottom: i32,
}

impl MvSearch<'_> {
    fn candidate(&self, pos: (i32, i32)) -> Option<&ModeInfo> {
        let r = self.mi_row as i32 + pos.0;
        let c = self.mi_col as i32 + pos.1;
        if r < 0 || c < self.tile_col_start as i32 || r >= self.mi_rows as i32 || c >= self.tile_col_end as i32 {
            return None;
        }
        let idx = self.grid[r as usize * self.mi_cols + c as usize];
        if idx == u32::MAX { None } else { Some(&self.mis[idx as usize]) }
    }

    /// get_mode_context: the inter-mode context from the first two neighbours.
    pub fn mode_context(&self) -> usize {
        let mut counter = 0;
        for i in 0..2 {
            if let Some(c) = self.candidate(MV_REF_BLOCKS[self.bsize as usize][i]) {
                counter += MODE_2_COUNTER[c.mode as usize];
            }
        }
        COUNTER_TO_CONTEXT[counter]
    }

    fn clamp(&self, mv: Mv) -> Mv {
        const B: i32 = 16 << 3;
        Mv::new(
            (mv.row as i32).clamp(self.to_top - B, self.to_bottom + B),
            (mv.col as i32).clamp(self.to_left - B, self.to_right + B),
        )
    }

    /// dec_find_mv_refs: up to two distinct candidates for `ref_frame`; returns the list and how
    /// many entries are meaningful (1 for NEARESTMV / NEWMV, 2 for NEARMV), each clamped.
    pub fn find_mv_refs(&self, mode: u8, ref_frame: i8, block: i32) -> ([Mv; 2], usize) {
        let mut list = [Mv::ZERO; 2];
        let mut count = 0usize;
        let early_break = mode != NEARMV;
        let mut different_ref_found = false;
        let positions = &MV_REF_BLOCKS[self.bsize as usize];
        let sign_bias = &self.sign_bias;
        // Returns true when the search is done.
        let add = |mv: Mv, list: &mut [Mv; 2], count: &mut usize| -> bool {
            if *count > 0 {
                if mv != list[0] {
                    list[1] = mv;
                    *count = 2;
                    return true;
                }
                false
            } else {
                list[0] = mv;
                *count = 1;
                early_break
            }
        };
        let scale = |m: &ModeInfo, which: usize| -> Mv {
            let mv = m.mv[which];
            if sign_bias[m.ref_frame[which] as usize] != sign_bias[ref_frame as usize] { Mv::new(-(mv.row as i32), -(mv.col as i32)) } else { mv }
        };
        let done = 'search: {
            let mut i = 0;
            if block >= 0 {
                while i < 2 {
                    let pos = positions[i];
                    if let Some(c) = self.candidate(pos) {
                        different_ref_found = true;
                        let sub = |which: usize| {
                            if c.sb_type < BLOCK_8X8 {
                                c.sub_mvs[IDX_N_COLUMN_TO_SUBBLOCK[block as usize][(pos.1 == 0) as usize]][which]
                            } else {
                                c.mv[which]
                            }
                        };
                        if c.ref_frame[0] == ref_frame {
                            if add(sub(0), &mut list, &mut count) {
                                break 'search true;
                            }
                        } else if c.ref_frame[1] == ref_frame && add(sub(1), &mut list, &mut count) {
                            break 'search true;
                        }
                    }
                    i += 1;
                }
            }
            while i < 8 {
                if let Some(c) = self.candidate(positions[i]) {
                    different_ref_found = true;
                    if c.ref_frame[0] == ref_frame {
                        if add(c.mv[0], &mut list, &mut count) {
                            break 'search true;
                        }
                    } else if c.ref_frame[1] == ref_frame && add(c.mv[1], &mut list, &mut count) {
                        break 'search true;
                    }
                }
                i += 1;
            }
            if let Some(p) = self.prev {
                if p.ref_frame[0] == ref_frame {
                    if add(p.mv[0], &mut list, &mut count) {
                        break 'search true;
                    }
                } else if p.ref_frame[1] == ref_frame && add(p.mv[1], &mut list, &mut count) {
                    break 'search true;
                }
            }
            if different_ref_found {
                for i in 0..8 {
                    if let Some(c) = self.candidate(positions[i]) {
                        if c.is_inter() {
                            if c.ref_frame[0] != ref_frame && add(scale(c, 0), &mut list, &mut count) {
                                break 'search true;
                            }
                            if c.has_second() && c.ref_frame[1] != ref_frame && c.mv[1] != c.mv[0] && add(scale(c, 1), &mut list, &mut count) {
                                break 'search true;
                            }
                        }
                    }
                }
            }
            if let Some(p) = self.prev {
                if p.ref_frame[0] != ref_frame && p.ref_frame[0] > INTRA_FRAME {
                    let mut mv = p.mv[0];
                    if sign_bias[p.ref_frame[0] as usize] != sign_bias[ref_frame as usize] {
                        mv = Mv::new(-(mv.row as i32), -(mv.col as i32));
                    }
                    if add(mv, &mut list, &mut count) {
                        break 'search true;
                    }
                }
                if p.ref_frame[1] > INTRA_FRAME && p.ref_frame[1] != ref_frame && p.mv[1] != p.mv[0] {
                    let mut mv = p.mv[1];
                    if sign_bias[p.ref_frame[1] as usize] != sign_bias[ref_frame as usize] {
                        mv = Mv::new(-(mv.row as i32), -(mv.col as i32));
                    }
                    if add(mv, &mut list, &mut count) {
                        break 'search true;
                    }
                }
            }
            false
        };
        if !done {
            count = if mode == NEARMV { 2 } else { 1 };
        }
        for k in 0..count {
            list[k] = self.clamp(list[k]);
        }
        (list, count)
    }
}

/// use_mv_hp: high precision only for small reference vectors.
pub fn use_mv_hp(mv: Mv) -> bool {
    (mv.row as i32).abs() < 64 && (mv.col as i32).abs() < 64
}

/// lower_mv_precision.
pub fn lower_mv_precision(mv: Mv, allow_hp: bool) -> Mv {
    if allow_hp && use_mv_hp(mv) {
        return mv;
    }
    let f = |v: i16| -> i16 {
        if v & 1 != 0 { if v > 0 { v - 1 } else { v + 1 } } else { v }
    };
    Mv { row: f(mv.row), col: f(mv.col) }
}
