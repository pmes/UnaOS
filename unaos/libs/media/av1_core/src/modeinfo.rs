//! §5.11.18–§5.11.33: inter_frame_mode_info and everything under it — inter segment id with the
//! temporal segmentation map, skip mode, is_inter, intra blocks of inter frames, reference frames
//! (single, bidirectional and unidirectional compound), the inter modes and the dynamic reference
//! list, MV residuals, inter-intra, motion mode (OBMC / local warp), compound type (average,
//! distance, wedge, difference-weighted) and the interpolation filters — plus §8.3.2, the context
//! of every one of those symbols.

use crate::decode::{block_height, block_width, Dec};
use crate::refs::Mv;
use crate::tables::*;
use crate::Result;

macro_rules! sym {
    ($s:ident, $cdf:expr) => {
        $s.sd.read_symbol(&mut $cdf)
    };
}

const INTRA: i32 = INTRA_FRAME as i32;

fn check_backward(ref_frame: i32) -> bool {
    ref_frame >= BWDREF_FRAME as i32 && ref_frame <= ALTREF_FRAME as i32
}
fn is_samedir_ref_pair(ref0: i32, ref1: i32) -> bool {
    (ref0 >= BWDREF_FRAME as i32) == (ref1 >= BWDREF_FRAME as i32)
}
fn ref_count_ctx(c0: usize, c1: usize) -> usize {
    if c0 < c1 {
        0
    } else if c0 == c1 {
        1
    } else {
        2
    }
}

/// has_newmv( mode )
pub fn has_newmv(mode: usize) -> bool {
    matches!(mode, NEWMV | NEW_NEWMV | NEAR_NEWMV | NEW_NEARMV | NEAREST_NEWMV | NEW_NEARESTMV)
}

impl<'a, 'f> Dec<'a, 'f> {
    /// inter_frame_mode_info() (§5.11.18)
    pub(crate) fn inter_frame_mode_info(&mut self) -> Result<()> {
        self.use_intrabc = false;
        let (r, c) = (self.mi_row, self.mi_col);
        let left = if self.avail_l { self.fs.ref_frames[self.fs.mi(r, c - 1)] } else { [INTRA_FRAME as i8, -1] };
        let above = if self.avail_u { self.fs.ref_frames[self.fs.mi(r - 1, c)] } else { [INTRA_FRAME as i8, -1] };
        self.left_ref_frame = [left[0] as i32, left[1] as i32];
        self.above_ref_frame = [above[0] as i32, above[1] as i32];
        self.left_intra = self.left_ref_frame[0] <= INTRA;
        self.above_intra = self.above_ref_frame[0] <= INTRA;
        self.left_single = self.left_ref_frame[1] <= INTRA;
        self.above_single = self.above_ref_frame[1] <= INTRA;
        self.skip = false;
        self.inter_segment_id(true);
        self.read_skip_mode();
        if self.skip_mode {
            self.skip = true;
        } else {
            self.read_skip();
        }
        if !self.hdr.seg_id_pre_skip {
            self.inter_segment_id(false);
        }
        self.lossless = self.hdr.lossless_array[self.segment_id];
        self.read_cdef();
        self.read_delta_qindex();
        self.read_delta_lf();
        self.read_deltas = false;
        self.read_is_inter();
        if self.is_inter {
            self.inter_block_mode_info();
        } else {
            self.intra_block_mode_info();
        }
        Ok(())
    }

    fn seg_active(&self, feature: usize) -> bool {
        self.hdr.seg_feature_active_idx(self.segment_id, feature)
    }

    /// get_segment_id() (§5.11.21): the smallest predicted id under the block.
    fn get_segment_id(&self) -> usize {
        let bw4 = NUM_4X4_BLOCKS_WIDE[self.mi_size] as usize;
        let bh4 = NUM_4X4_BLOCKS_HIGH[self.mi_size] as usize;
        let x_mis = (self.fs.mi_cols - self.mi_col).min(bw4);
        let y_mis = (self.fs.mi_rows - self.mi_row).min(bh4);
        let mut seg = 7;
        for y in 0..y_mis {
            for x in 0..x_mis {
                seg = seg.min(self.fs.prev_segment_ids[self.fs.mi(self.mi_row + y, self.mi_col + x)] as usize);
            }
        }
        seg
    }

    fn set_seg_pred_context(&mut self, v: u8) {
        let bw4 = NUM_4X4_BLOCKS_WIDE[self.mi_size] as usize;
        let bh4 = NUM_4X4_BLOCKS_HIGH[self.mi_size] as usize;
        for i in 0..bw4 {
            self.above_seg_pred_context[self.mi_col + i] = v;
        }
        for i in 0..bh4 {
            self.left_seg_pred_context[self.mi_row + i] = v;
        }
    }

    /// inter_segment_id( preSkip ) (§5.11.19)
    fn inter_segment_id(&mut self, pre_skip: bool) {
        if !self.hdr.segmentation_enabled {
            self.segment_id = 0;
            return;
        }
        let predicted_segment_id = self.get_segment_id();
        if self.hdr.segmentation_update_map {
            if pre_skip && !self.hdr.seg_id_pre_skip {
                self.segment_id = 0;
                return;
            }
            if !pre_skip && self.skip {
                self.set_seg_pred_context(0);
                self.read_segment_id();
                return;
            }
            if self.hdr.segmentation_temporal_update {
                let ctx = self.left_seg_pred_context[self.mi_row] as usize + self.above_seg_pred_context[self.mi_col] as usize;
                let seg_id_predicted = sym!(self, self.cdf.segment_id_predicted[ctx]);
                if seg_id_predicted != 0 {
                    self.segment_id = predicted_segment_id;
                } else {
                    self.read_segment_id();
                }
                self.set_seg_pred_context(seg_id_predicted as u8);
            } else {
                self.read_segment_id();
            }
        } else {
            self.segment_id = predicted_segment_id;
        }
    }

    /// read_skip_mode() (§5.11.10)
    fn read_skip_mode(&mut self) {
        if self.seg_active(SEG_LVL_SKIP)
            || self.seg_active(SEG_LVL_REF_FRAME)
            || self.seg_active(SEG_LVL_GLOBALMV)
            || !self.hdr.skip_mode_present
            || block_width(self.mi_size) < 8
            || block_height(self.mi_size) < 8
        {
            self.skip_mode = false;
        } else {
            let mut ctx = 0;
            if self.avail_u {
                ctx += self.fs.skip_modes[self.fs.mi(self.mi_row - 1, self.mi_col)] as usize;
            }
            if self.avail_l {
                ctx += self.fs.skip_modes[self.fs.mi(self.mi_row, self.mi_col - 1)] as usize;
            }
            self.skip_mode = sym!(self, self.cdf.skip_mode[ctx]) != 0;
        }
    }

    /// read_is_inter() (§5.11.20)
    fn read_is_inter(&mut self) {
        if self.skip_mode {
            self.is_inter = true;
        } else if self.seg_active(SEG_LVL_REF_FRAME) {
            self.is_inter = self.hdr.feature_data[self.segment_id][SEG_LVL_REF_FRAME] != INTRA;
        } else if self.seg_active(SEG_LVL_GLOBALMV) {
            self.is_inter = true;
        } else {
            let ctx = if self.avail_u && self.avail_l {
                if self.left_intra && self.above_intra {
                    3
                } else {
                    (self.left_intra || self.above_intra) as usize
                }
            } else if self.avail_u || self.avail_l {
                2 * (if self.avail_u { self.above_intra } else { self.left_intra }) as usize
            } else {
                0
            };
            self.is_inter = sym!(self, self.cdf.is_inter[ctx]) != 0;
        }
    }

    /// intra_block_mode_info() (§5.11.22)
    fn intra_block_mode_info(&mut self) {
        self.ref_frame = [INTRA, NONE as i32];
        let ctx = SIZE_GROUP[self.mi_size] as usize;
        self.y_mode = sym!(self, self.cdf.y_mode[ctx]);
        self.intra_angle_info_y();
        if self.has_chroma {
            let cfl_allowed = if self.lossless && self.get_plane_residual_size(self.mi_size, 1) == BLOCK_4X4 {
                true
            } else {
                !self.lossless && block_width(self.mi_size).max(block_height(self.mi_size)) <= 32
            };
            let ym = self.y_mode;
            self.uv_mode = if cfl_allowed {
                sym!(self, self.cdf.uv_mode_cfl_allowed[ym])
            } else {
                sym!(self, self.cdf.uv_mode_cfl_not_allowed[ym])
            };
            if self.uv_mode == UV_CFL_PRED {
                self.read_cfl_alphas();
            }
            self.intra_angle_info_uv();
        } else {
            self.uv_mode = DC_PRED;
        }
        self.palette_size_y = 0;
        self.palette_size_uv = 0;
        if self.mi_size >= BLOCK_8X8
            && block_width(self.mi_size) <= 64
            && block_height(self.mi_size) <= 64
            && self.hdr.allow_screen_content_tools
        {
            self.palette_mode_info();
        }
        self.filter_intra_mode_info();
    }

    /// inter_block_mode_info() (§5.11.23)
    fn inter_block_mode_info(&mut self) {
        self.palette_size_y = 0;
        self.palette_size_uv = 0;
        self.read_ref_frames();
        let is_compound = self.ref_frame[1] > INTRA;
        self.find_mv_stack(is_compound);
        if self.skip_mode {
            self.y_mode = NEAREST_NEARESTMV;
        } else if self.seg_active(SEG_LVL_SKIP) || self.seg_active(SEG_LVL_GLOBALMV) {
            self.y_mode = GLOBALMV;
        } else if is_compound {
            let ctx = COMPOUND_MODE_CTX_MAP[self.ref_mv_context >> 1][self.new_mv_context.min(COMP_NEWMV_CTXS - 1)] as usize;
            let compound_mode = sym!(self, self.cdf.compound_mode[ctx]);
            self.y_mode = NEAREST_NEARESTMV + compound_mode;
        } else {
            let ctx = self.new_mv_context;
            let new_mv = sym!(self, self.cdf.new_mv[ctx]);
            if new_mv == 0 {
                self.y_mode = NEWMV;
            } else {
                let ctx = self.zero_mv_context;
                let zero_mv = sym!(self, self.cdf.zero_mv[ctx]);
                if zero_mv == 0 {
                    self.y_mode = GLOBALMV;
                } else {
                    let ctx = self.ref_mv_context;
                    let ref_mv = sym!(self, self.cdf.ref_mv[ctx]);
                    self.y_mode = if ref_mv == 0 { NEARESTMV } else { NEARMV };
                }
            }
        }
        self.ref_mv_idx = 0;
        if self.y_mode == NEWMV || self.y_mode == NEW_NEWMV {
            for idx in 0..2 {
                if self.num_mv_found > idx + 1 {
                    let ctx = self.drl_ctx_stack[idx];
                    let drl_mode = sym!(self, self.cdf.drl_mode[ctx]);
                    if drl_mode == 0 {
                        self.ref_mv_idx = idx;
                        break;
                    }
                    self.ref_mv_idx = idx + 1;
                }
            }
        } else if self.has_nearmv() {
            self.ref_mv_idx = 1;
            for idx in 1..3 {
                if self.num_mv_found > idx + 1 {
                    let ctx = self.drl_ctx_stack[idx];
                    let drl_mode = sym!(self, self.cdf.drl_mode[ctx]);
                    if drl_mode == 0 {
                        self.ref_mv_idx = idx;
                        break;
                    }
                    self.ref_mv_idx = idx + 1;
                }
            }
        }
        self.assign_mv(is_compound);
        self.read_interintra_mode(is_compound);
        self.read_motion_mode(is_compound);
        self.read_compound_type(is_compound);
        if self.hdr.interpolation_filter == SWITCHABLE as u8 {
            let dirs = if self.seq.enable_dual_filter { 2 } else { 1 };
            for dir in 0..dirs {
                if self.needs_interp_filter() {
                    let ctx = self.interp_filter_ctx(dir);
                    self.interp_filter[dir] = sym!(self, self.cdf.interp_filter[ctx]) as u8;
                } else {
                    self.interp_filter[dir] = EIGHTTAP as u8;
                }
            }
            if !self.seq.enable_dual_filter {
                self.interp_filter[1] = self.interp_filter[0];
            }
        } else {
            self.interp_filter = [self.hdr.interpolation_filter; 2];
        }
    }

    fn has_nearmv(&self) -> bool {
        matches!(self.y_mode, NEARMV | NEAR_NEARMV | NEAR_NEWMV | NEW_NEARMV)
    }

    fn needs_interp_filter(&self) -> bool {
        let large = block_width(self.mi_size).min(block_height(self.mi_size)) >= 8;
        if self.skip_mode || self.motion_mode == LOCALWARP {
            false
        } else if large && self.y_mode == GLOBALMV {
            self.hdr.gm_type[self.ref_frame[0] as usize] == TRANSLATION as u8
        } else if large && self.y_mode == GLOBAL_GLOBALMV {
            self.hdr.gm_type[self.ref_frame[0] as usize] == TRANSLATION as u8
                || self.hdr.gm_type[self.ref_frame[1] as usize] == TRANSLATION as u8
        } else {
            true
        }
    }

    fn interp_filter_ctx(&self, dir: usize) -> usize {
        let mut ctx = ((dir & 1) * 2 + (self.ref_frame[1] > INTRA) as usize) * 4;
        let mut left_type = 3usize;
        let mut above_type = 3usize;
        if self.avail_l {
            let i = self.fs.mi(self.mi_row, self.mi_col - 1);
            let rf = self.fs.ref_frames[i];
            if rf[0] as i32 == self.ref_frame[0] || rf[1] as i32 == self.ref_frame[0] {
                left_type = self.fs.interp_filters[i][dir] as usize;
            }
        }
        if self.avail_u {
            let i = self.fs.mi(self.mi_row - 1, self.mi_col);
            let rf = self.fs.ref_frames[i];
            if rf[0] as i32 == self.ref_frame[0] || rf[1] as i32 == self.ref_frame[0] {
                above_type = self.fs.interp_filters[i][dir] as usize;
            }
        }
        if left_type == above_type {
            ctx += left_type;
        } else if left_type == 3 {
            ctx += above_type;
        } else if above_type == 3 {
            ctx += left_type;
        } else {
            ctx += 3;
        }
        ctx
    }

    fn count_refs(&self, frame_type: usize) -> usize {
        let ft = frame_type as i32;
        let mut c = 0;
        if self.avail_u {
            c += (self.above_ref_frame[0] == ft) as usize + (self.above_ref_frame[1] == ft) as usize;
        }
        if self.avail_l {
            c += (self.left_ref_frame[0] == ft) as usize + (self.left_ref_frame[1] == ft) as usize;
        }
        c
    }

    fn comp_ref_ctx(&self) -> usize {
        let last12 = self.count_refs(LAST_FRAME) + self.count_refs(LAST2_FRAME);
        let last3_gold = self.count_refs(LAST3_FRAME) + self.count_refs(GOLDEN_FRAME);
        ref_count_ctx(last12, last3_gold)
    }
    fn comp_ref_p1_ctx(&self) -> usize {
        ref_count_ctx(self.count_refs(LAST_FRAME), self.count_refs(LAST2_FRAME))
    }
    fn comp_ref_p2_ctx(&self) -> usize {
        ref_count_ctx(self.count_refs(LAST3_FRAME), self.count_refs(GOLDEN_FRAME))
    }
    fn comp_bwdref_ctx(&self) -> usize {
        let brfarf2 = self.count_refs(BWDREF_FRAME) + self.count_refs(ALTREF2_FRAME);
        ref_count_ctx(brfarf2, self.count_refs(ALTREF_FRAME))
    }
    fn comp_bwdref_p1_ctx(&self) -> usize {
        ref_count_ctx(self.count_refs(BWDREF_FRAME), self.count_refs(ALTREF2_FRAME))
    }
    fn single_ref_p1_ctx(&self) -> usize {
        let fwd = self.count_refs(LAST_FRAME) + self.count_refs(LAST2_FRAME) + self.count_refs(LAST3_FRAME) + self.count_refs(GOLDEN_FRAME);
        let bwd = self.count_refs(BWDREF_FRAME) + self.count_refs(ALTREF2_FRAME) + self.count_refs(ALTREF_FRAME);
        ref_count_ctx(fwd, bwd)
    }
    fn uni_comp_ref_p1_ctx(&self) -> usize {
        let last2 = self.count_refs(LAST2_FRAME);
        let last3_gold = self.count_refs(LAST3_FRAME) + self.count_refs(GOLDEN_FRAME);
        ref_count_ctx(last2, last3_gold)
    }

    fn comp_mode_ctx(&self) -> usize {
        if self.avail_u && self.avail_l {
            if self.above_single && self.left_single {
                (check_backward(self.above_ref_frame[0]) ^ check_backward(self.left_ref_frame[0])) as usize
            } else if self.above_single {
                2 + (check_backward(self.above_ref_frame[0]) || self.above_intra) as usize
            } else if self.left_single {
                2 + (check_backward(self.left_ref_frame[0]) || self.left_intra) as usize
            } else {
                4
            }
        } else if self.avail_u {
            if self.above_single {
                check_backward(self.above_ref_frame[0]) as usize
            } else {
                3
            }
        } else if self.avail_l {
            if self.left_single {
                check_backward(self.left_ref_frame[0]) as usize
            } else {
                3
            }
        } else {
            1
        }
    }

    fn comp_ref_type_ctx(&self) -> usize {
        let above0 = self.above_ref_frame[0];
        let above1 = self.above_ref_frame[1];
        let left0 = self.left_ref_frame[0];
        let left1 = self.left_ref_frame[1];
        let above_comp_inter = self.avail_u && !self.above_intra && !self.above_single;
        let left_comp_inter = self.avail_l && !self.left_intra && !self.left_single;
        let above_uni_comp = above_comp_inter && is_samedir_ref_pair(above0, above1);
        let left_uni_comp = left_comp_inter && is_samedir_ref_pair(left0, left1);
        if self.avail_u && !self.above_intra && self.avail_l && !self.left_intra {
            let samedir = is_samedir_ref_pair(above0, left0) as usize;
            if !above_comp_inter && !left_comp_inter {
                1 + 2 * samedir
            } else if !above_comp_inter {
                if !left_uni_comp {
                    1
                } else {
                    3 + samedir
                }
            } else if !left_comp_inter {
                if !above_uni_comp {
                    1
                } else {
                    3 + samedir
                }
            } else if !above_uni_comp && !left_uni_comp {
                0
            } else if !above_uni_comp || !left_uni_comp {
                2
            } else {
                3 + ((above0 == BWDREF_FRAME as i32) == (left0 == BWDREF_FRAME as i32)) as usize
            }
        } else if self.avail_u && self.avail_l {
            if above_comp_inter {
                1 + 2 * above_uni_comp as usize
            } else if left_comp_inter {
                1 + 2 * left_uni_comp as usize
            } else {
                2
            }
        } else if above_comp_inter {
            4 * above_uni_comp as usize
        } else if left_comp_inter {
            4 * left_uni_comp as usize
        } else {
            2
        }
    }

    /// read_ref_frames() (§5.11.25)
    fn read_ref_frames(&mut self) {
        if self.skip_mode {
            self.ref_frame = [self.hdr.skip_mode_frame[0] as i32, self.hdr.skip_mode_frame[1] as i32];
        } else if self.seg_active(SEG_LVL_REF_FRAME) {
            self.ref_frame = [self.hdr.feature_data[self.segment_id][SEG_LVL_REF_FRAME], NONE as i32];
        } else if self.seg_active(SEG_LVL_SKIP) || self.seg_active(SEG_LVL_GLOBALMV) {
            self.ref_frame = [LAST_FRAME as i32, NONE as i32];
        } else {
            let bw4 = NUM_4X4_BLOCKS_WIDE[self.mi_size] as usize;
            let bh4 = NUM_4X4_BLOCKS_HIGH[self.mi_size] as usize;
            let comp_mode = if self.hdr.reference_select && bw4.min(bh4) >= 2 {
                let ctx = self.comp_mode_ctx();
                sym!(self, self.cdf.comp_mode[ctx])
            } else {
                SINGLE_REFERENCE
            };
            if comp_mode == COMPOUND_REFERENCE {
                let ctx = self.comp_ref_type_ctx();
                let comp_ref_type = sym!(self, self.cdf.comp_ref_type[ctx]);
                if comp_ref_type == UNIDIR_COMP_REFERENCE {
                    let ctx = self.single_ref_p1_ctx();
                    let uni_comp_ref = sym!(self, self.cdf.uni_comp_ref[ctx][0]);
                    if uni_comp_ref != 0 {
                        self.ref_frame = [BWDREF_FRAME as i32, ALTREF_FRAME as i32];
                    } else {
                        let ctx = self.uni_comp_ref_p1_ctx();
                        let p1 = sym!(self, self.cdf.uni_comp_ref[ctx][1]);
                        if p1 != 0 {
                            let ctx = self.comp_ref_p2_ctx();
                            let p2 = sym!(self, self.cdf.uni_comp_ref[ctx][2]);
                            if p2 != 0 {
                                self.ref_frame = [LAST_FRAME as i32, GOLDEN_FRAME as i32];
                            } else {
                                self.ref_frame = [LAST_FRAME as i32, LAST3_FRAME as i32];
                            }
                        } else {
                            self.ref_frame = [LAST_FRAME as i32, LAST2_FRAME as i32];
                        }
                    }
                } else {
                    let ctx = self.comp_ref_ctx();
                    let comp_ref = sym!(self, self.cdf.comp_ref[ctx][0]);
                    if comp_ref == 0 {
                        let ctx = self.comp_ref_p1_ctx();
                        let p1 = sym!(self, self.cdf.comp_ref[ctx][1]);
                        self.ref_frame[0] = if p1 != 0 { LAST2_FRAME as i32 } else { LAST_FRAME as i32 };
                    } else {
                        let ctx = self.comp_ref_p2_ctx();
                        let p2 = sym!(self, self.cdf.comp_ref[ctx][2]);
                        self.ref_frame[0] = if p2 != 0 { GOLDEN_FRAME as i32 } else { LAST3_FRAME as i32 };
                    }
                    let ctx = self.comp_bwdref_ctx();
                    let comp_bwdref = sym!(self, self.cdf.comp_bwd_ref[ctx][0]);
                    if comp_bwdref == 0 {
                        let ctx = self.comp_bwdref_p1_ctx();
                        let p1 = sym!(self, self.cdf.comp_bwd_ref[ctx][1]);
                        self.ref_frame[1] = if p1 != 0 { ALTREF2_FRAME as i32 } else { BWDREF_FRAME as i32 };
                    } else {
                        self.ref_frame[1] = ALTREF_FRAME as i32;
                    }
                }
            } else {
                let ctx = self.single_ref_p1_ctx();
                let p1 = sym!(self, self.cdf.single_ref[ctx][0]);
                if p1 != 0 {
                    let ctx = self.comp_bwdref_ctx();
                    let p2 = sym!(self, self.cdf.single_ref[ctx][1]);
                    if p2 == 0 {
                        let ctx = self.comp_bwdref_p1_ctx();
                        let p6 = sym!(self, self.cdf.single_ref[ctx][5]);
                        self.ref_frame[0] = if p6 != 0 { ALTREF2_FRAME as i32 } else { BWDREF_FRAME as i32 };
                    } else {
                        self.ref_frame[0] = ALTREF_FRAME as i32;
                    }
                } else {
                    let ctx = self.comp_ref_ctx();
                    let p3 = sym!(self, self.cdf.single_ref[ctx][2]);
                    if p3 != 0 {
                        let ctx = self.comp_ref_p2_ctx();
                        let p5 = sym!(self, self.cdf.single_ref[ctx][4]);
                        self.ref_frame[0] = if p5 != 0 { GOLDEN_FRAME as i32 } else { LAST3_FRAME as i32 };
                    } else {
                        let ctx = self.comp_ref_p1_ctx();
                        let p4 = sym!(self, self.cdf.single_ref[ctx][3]);
                        self.ref_frame[0] = if p4 != 0 { LAST2_FRAME as i32 } else { LAST_FRAME as i32 };
                    }
                }
                self.ref_frame[1] = NONE as i32;
            }
        }
    }

    /// get_mode( refList ) (§5.11.30)
    fn get_mode(&self, ref_list: usize) -> usize {
        let y = self.y_mode;
        if ref_list == 0 {
            if y < NEAREST_NEARESTMV {
                y
            } else if y == NEW_NEWMV || y == NEW_NEARESTMV || y == NEW_NEARMV {
                NEWMV
            } else if y == NEAREST_NEARESTMV || y == NEAREST_NEWMV {
                NEARESTMV
            } else if y == NEAR_NEARMV || y == NEAR_NEWMV {
                NEARMV
            } else {
                GLOBALMV
            }
        } else if y == NEW_NEWMV || y == NEAREST_NEWMV || y == NEAR_NEWMV {
            NEWMV
        } else if y == NEAREST_NEARESTMV || y == NEW_NEARESTMV {
            NEARESTMV
        } else if y == NEAR_NEARMV || y == NEW_NEARMV {
            NEARMV
        } else {
            GLOBALMV
        }
    }

    /// assign_mv( isCompound ) (§5.11.26)
    pub(crate) fn assign_mv(&mut self, is_compound: bool) {
        for i in 0..1 + is_compound as usize {
            let comp_mode = if self.use_intrabc { NEWMV } else { self.get_mode(i) };
            if self.use_intrabc {
                self.pred_mv[0] = self.ref_stack_mv[0][0];
                if self.pred_mv[0][0] == 0 && self.pred_mv[0][1] == 0 {
                    self.pred_mv[0] = self.ref_stack_mv[1][0];
                }
                if self.pred_mv[0][0] == 0 && self.pred_mv[0][1] == 0 {
                    let sb_size = if self.seq.use_128x128_superblock { BLOCK_128X128 } else { BLOCK_64X64 };
                    let sb_size4 = NUM_4X4_BLOCKS_HIGH[sb_size] as i32;
                    if (self.mi_row as i32) - sb_size4 < self.mi_row_start as i32 {
                        self.pred_mv[0][0] = 0;
                        self.pred_mv[0][1] = -(sb_size4 * MI_SIZE as i32 + INTRABC_DELAY_PIXELS as i32) * 8;
                    } else {
                        self.pred_mv[0][0] = -(sb_size4 * MI_SIZE as i32 * 8);
                        self.pred_mv[0][1] = 0;
                    }
                }
            } else if comp_mode == GLOBALMV {
                self.pred_mv[i] = self.global_mvs[i];
            } else {
                let mut pos = if comp_mode == NEARESTMV { 0 } else { self.ref_mv_idx };
                if comp_mode == NEWMV && self.num_mv_found <= 1 {
                    pos = 0;
                }
                self.pred_mv[i] = self.ref_stack_mv[pos][i];
            }
            if comp_mode == NEWMV {
                self.read_mv(i);
            } else {
                self.mv[i] = self.pred_mv[i];
            }
        }
    }

    /// read_mv( ref ) (§5.11.31)
    fn read_mv(&mut self, r: usize) {
        let mut diff_mv: Mv = [0, 0];
        let ctx = if self.use_intrabc { MV_INTRABC_CONTEXT } else { 0 };
        let mv_joint = sym!(self, self.cdf.mv_joint[ctx]);
        if mv_joint == MV_JOINT_HZVNZ || mv_joint == MV_JOINT_HNZVNZ {
            diff_mv[0] = self.read_mv_component(ctx, 0);
        }
        if mv_joint == MV_JOINT_HNZVZ || mv_joint == MV_JOINT_HNZVNZ {
            diff_mv[1] = self.read_mv_component(ctx, 1);
        }
        self.mv[r] = [self.pred_mv[r][0] + diff_mv[0], self.pred_mv[r][1] + diff_mv[1]];
    }

    /// read_mv_component( comp ) (§5.11.32)
    fn read_mv_component(&mut self, ctx: usize, comp: usize) -> i32 {
        let mv_sign = sym!(self, self.cdf.mv_sign[ctx][comp]);
        let mv_class = sym!(self, self.cdf.mv_class[ctx][comp]);
        let mag: i32;
        if mv_class == MV_CLASS_0 {
            let mv_class0_bit = sym!(self, self.cdf.mv_class0_bit[ctx][comp]) as i32;
            let fr = if self.hdr.force_integer_mv { 3 } else { sym!(self, self.cdf.mv_class0_fr[ctx][comp][mv_class0_bit as usize]) as i32 };
            let hp = if self.hdr.allow_high_precision_mv { sym!(self, self.cdf.mv_class0_hp[ctx][comp]) as i32 } else { 1 };
            mag = ((mv_class0_bit << 3) | (fr << 1) | hp) + 1;
        } else {
            let mut d = 0i32;
            for i in 0..mv_class {
                let mv_bit = sym!(self, self.cdf.mv_bit[ctx][comp][i]) as i32;
                d |= mv_bit << i;
            }
            let mut m = (CLASS0_SIZE as i32) << (mv_class + 2);
            let fr = if self.hdr.force_integer_mv { 3 } else { sym!(self, self.cdf.mv_fr[ctx][comp]) as i32 };
            let hp = if self.hdr.allow_high_precision_mv { sym!(self, self.cdf.mv_hp[ctx][comp]) as i32 } else { 1 };
            m += ((d << 3) | (fr << 1) | hp) + 1;
            mag = m;
        }
        if mv_sign != 0 { -mag } else { mag }
    }

    /// read_interintra_mode( isCompound ) (§5.11.28)
    fn read_interintra_mode(&mut self, is_compound: bool) {
        if !self.skip_mode && self.seq.enable_interintra_compound && !is_compound && self.mi_size >= BLOCK_8X8 && self.mi_size <= BLOCK_32X32 {
            let ctx = SIZE_GROUP[self.mi_size] as usize - 1;
            self.interintra = sym!(self, self.cdf.inter_intra[ctx]) != 0;
            if self.interintra {
                self.interintra_mode = sym!(self, self.cdf.inter_intra_mode[ctx]);
                self.ref_frame[1] = INTRA;
                self.angle_delta_y = 0;
                self.angle_delta_uv = 0;
                self.use_filter_intra = false;
                let ms = self.mi_size;
                self.wedge_interintra = sym!(self, self.cdf.wedge_inter_intra[ms]) != 0;
                if self.wedge_interintra {
                    self.wedge_index = sym!(self, self.cdf.wedge_index[ms]);
                    self.wedge_sign = 0;
                }
            }
        } else {
            self.interintra = false;
        }
    }

    /// is_scaled( refFrame ) (§5.11.27)
    pub(crate) fn is_scaled(&self, ref_frame: usize) -> bool {
        let f = match &self.refs.frames[self.hdr.ref_frame_idx[ref_frame - LAST_FRAME]] {
            Some(f) => f,
            None => return false,
        };
        let fw = self.hdr.frame_width;
        let fh = self.hdr.frame_height;
        let x_scale = ((f.upscaled_width << REF_SCALE_SHIFT) + (fw / 2)) / fw;
        let y_scale = ((f.frame_height << REF_SCALE_SHIFT) + (fh / 2)) / fh;
        let no_scale = 1 << REF_SCALE_SHIFT;
        x_scale != no_scale || y_scale != no_scale
    }

    /// read_motion_mode( isCompound ) (§5.11.27)
    fn read_motion_mode(&mut self, is_compound: bool) {
        self.motion_mode = SIMPLE;
        if self.skip_mode || !self.hdr.is_motion_mode_switchable {
            return;
        }
        if block_width(self.mi_size).min(block_height(self.mi_size)) < 8 {
            return;
        }
        if !self.hdr.force_integer_mv && (self.y_mode == GLOBALMV || self.y_mode == GLOBAL_GLOBALMV) && self.hdr.gm_type[self.ref_frame[0] as usize] > TRANSLATION as u8 {
            return;
        }
        if is_compound || self.ref_frame[1] == INTRA || !self.has_overlappable_candidates() {
            return;
        }
        self.find_warp_samples();
        let ms = self.mi_size;
        if self.hdr.force_integer_mv || self.num_samples == 0 || !self.hdr.allow_warped_motion || self.is_scaled(self.ref_frame[0] as usize) {
            let use_obmc = sym!(self, self.cdf.use_obmc[ms]);
            self.motion_mode = if use_obmc != 0 { OBMC } else { SIMPLE };
        } else {
            self.motion_mode = sym!(self, self.cdf.motion_mode[ms]);
        }
    }

    /// read_compound_type( isCompound ) (§5.11.29)
    fn read_compound_type(&mut self, is_compound: bool) {
        self.comp_group_idx = 0;
        self.compound_idx = 1;
        if self.skip_mode {
            self.compound_type = COMPOUND_AVERAGE;
            return;
        }
        if is_compound {
            let n = WEDGE_BITS[self.mi_size];
            if self.seq.enable_masked_compound {
                let ctx = self.comp_group_idx_ctx();
                self.comp_group_idx = sym!(self, self.cdf.comp_group_idx[ctx]);
            }
            if self.comp_group_idx == 0 {
                if self.seq.enable_jnt_comp {
                    let ctx = self.compound_idx_ctx();
                    self.compound_idx = sym!(self, self.cdf.compound_idx[ctx]);
                    self.compound_type = if self.compound_idx != 0 { COMPOUND_AVERAGE } else { COMPOUND_DISTANCE };
                } else {
                    self.compound_type = COMPOUND_AVERAGE;
                }
            } else if n == 0 {
                self.compound_type = COMPOUND_DIFFWTD;
            } else {
                let ms = self.mi_size;
                self.compound_type = sym!(self, self.cdf.compound_type[ms]);
            }
            if self.compound_type == COMPOUND_WEDGE {
                let ms = self.mi_size;
                self.wedge_index = sym!(self, self.cdf.wedge_index[ms]);
                self.wedge_sign = self.sd.read_literal(1) as usize;
            } else if self.compound_type == COMPOUND_DIFFWTD {
                self.mask_type = self.sd.read_literal(1) as usize;
            }
        } else if self.interintra {
            self.compound_type = if self.wedge_interintra { COMPOUND_WEDGE } else { COMPOUND_INTRA };
        } else {
            self.compound_type = COMPOUND_AVERAGE;
        }
    }

    fn comp_group_idx_ctx(&self) -> usize {
        let mut ctx = 0;
        if self.avail_u {
            if !self.above_single {
                ctx += self.fs.comp_group_idxs[self.fs.mi(self.mi_row - 1, self.mi_col)] as usize;
            } else if self.above_ref_frame[0] == ALTREF_FRAME as i32 {
                ctx += 3;
            }
        }
        if self.avail_l {
            if !self.left_single {
                ctx += self.fs.comp_group_idxs[self.fs.mi(self.mi_row, self.mi_col - 1)] as usize;
            } else if self.left_ref_frame[0] == ALTREF_FRAME as i32 {
                ctx += 3;
            }
        }
        ctx.min(5)
    }

    fn compound_idx_ctx(&self) -> usize {
        let seq = self.seq;
        let h = self.hdr;
        let fwd = crate::obu::get_relative_dist(seq, h.order_hints[self.ref_frame[0] as usize], h.order_hint).abs();
        let bck = crate::obu::get_relative_dist(seq, h.order_hints[self.ref_frame[1] as usize], h.order_hint).abs();
        let mut ctx = if fwd == bck { 3 } else { 0 };
        if self.avail_u {
            if !self.above_single {
                ctx += self.fs.compound_idxs[self.fs.mi(self.mi_row - 1, self.mi_col)] as usize;
            } else if self.above_ref_frame[0] == ALTREF_FRAME as i32 {
                ctx += 1;
            }
        }
        if self.avail_l {
            if !self.left_single {
                ctx += self.fs.compound_idxs[self.fs.mi(self.mi_row, self.mi_col - 1)] as usize;
            } else if self.left_ref_frame[0] == ALTREF_FRAME as i32 {
                ctx += 1;
            }
        }
        ctx
    }
}
