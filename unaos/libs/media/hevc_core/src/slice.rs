// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The slice segment header (7.3.6.1) in full — the reference picture set, long-term pictures, the SAO / QP /
//! deblocking overrides, ref_pic_lists_modification (7.3.6.2) and pred_weight_table (7.3.6.3), so that P and B headers
//! parse — the entry points, the header extension and byte_alignment(). The slice data starts at
//! [`SliceHeader::data_offset`] (a byte offset into the RBSP).

use alloc::vec::Vec;

use h264_core::bits::BitReader;

use crate::nal::{NalHeader, IDR_N_LP, IDR_W_RADL};
use crate::ps::{ceil_log2, parse_st_rps, Pps, Sps, StRps};
use crate::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SliceType {
    B,
    P,
    #[default]
    I,
}

/// One list's explicit weights (7.3.6.3), per reference index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WeightList {
    /// (LumaWeight, luma_offset) per reference index.
    pub luma: Vec<(i32, i32)>,
    /// [(ChromaWeight, delta_chroma_offset)] for Cb and Cr per reference index.
    pub chroma: Vec<[(i32, i32); 2]>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PredWeights {
    pub luma_log2_weight_denom: u32,
    pub chroma_log2_weight_denom: u32,
    pub l0: WeightList,
    pub l1: WeightList,
}

/// One long-term entry of the header: (PocLsbLt, UsedByCurrPicLt, delta_poc_msb_cycle_lt if present).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LongTerm {
    pub poc_lsb: u32,
    pub used: bool,
    pub delta_poc_msb_cycle: Option<u32>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SliceHeader {
    pub first_slice_segment_in_pic_flag: bool,
    pub no_output_of_prior_pics_flag: bool,
    pub pps_id: u32,
    pub dependent_slice_segment_flag: bool,
    pub segment_address: u32,
    pub slice_type: SliceType,
    pub pic_output_flag: bool,
    pub colour_plane_id: u8,
    pub pic_order_cnt_lsb: u32,
    pub short_term_ref_pic_set_sps_flag: bool,
    pub short_term_ref_pic_set_idx: u32,
    /// The slice's own set when !short_term_ref_pic_set_sps_flag.
    pub st_rps: Option<StRps>,
    pub long_term: Vec<LongTerm>,
    pub temporal_mvp_enabled_flag: bool,
    pub sao_luma_flag: bool,
    pub sao_chroma_flag: bool,
    pub num_ref_idx_l0_active: u32,
    pub num_ref_idx_l1_active: u32,
    /// list_entry_l0 / list_entry_l1 when modified.
    pub list_entry_l0: Option<Vec<u32>>,
    pub list_entry_l1: Option<Vec<u32>>,
    pub mvd_l1_zero_flag: bool,
    pub cabac_init_flag: bool,
    pub collocated_from_l0_flag: bool,
    pub collocated_ref_idx: u32,
    pub pred_weights: Option<PredWeights>,
    pub max_num_merge_cand: u32,
    pub qp_delta: i32,
    pub cb_qp_offset: i32,
    pub cr_qp_offset: i32,
    pub cu_chroma_qp_offset_enabled_flag: bool,
    pub deblocking_filter_disabled_flag: bool,
    pub beta_offset_div2: i32,
    pub tc_offset_div2: i32,
    pub loop_filter_across_slices_enabled_flag: bool,
    pub entry_point_offsets: Vec<u32>,
    /// Byte offset of slice_segment_data() in the RBSP.
    pub data_offset: usize,
}

impl SliceHeader {
    /// SliceQpY (7-54).
    pub fn slice_qp(&self, pps: &Pps) -> i32 {
        pps.init_qp + self.qp_delta
    }

    /// The short-term RPS the slice uses.
    pub fn rps<'a>(&'a self, sps: &'a Sps) -> Option<&'a StRps> {
        if self.short_term_ref_pic_set_sps_flag { sps.st_rps.get(self.short_term_ref_pic_set_idx as usize) } else { self.st_rps.as_ref() }
    }

    /// NumPicTotalCurr (7-55) without the screen-content current-picture reference.
    pub fn num_pic_total_curr(&self, sps: &Sps) -> u32 {
        let st = self.rps(sps).map_or(0, |r| r.num_used());
        (st + self.long_term.iter().filter(|l| l.used).count()) as u32
    }

    /// Copy the fields a dependent slice segment takes from its slice's independent segment (7.4.7.1).
    pub fn inherit(&mut self, from: &SliceHeader) {
        let keep = (self.first_slice_segment_in_pic_flag, self.no_output_of_prior_pics_flag, self.pps_id, self.segment_address);
        let entry = core::mem::take(&mut self.entry_point_offsets);
        let off = self.data_offset;
        *self = from.clone();
        (self.first_slice_segment_in_pic_flag, self.no_output_of_prior_pics_flag, self.pps_id, self.segment_address) = keep;
        self.dependent_slice_segment_flag = true;
        self.entry_point_offsets = entry;
        self.data_offset = off;
    }
}

/// The PPS id of a slice segment header (to find the parameter sets before parsing the rest).
pub fn peek_pps_id(rbsp: &[u8], nal: &NalHeader) -> Result<u32> {
    let mut r = BitReader::new(rbsp);
    r.skip(1);
    if nal.is_irap() {
        r.skip(1);
    }
    let id = r.ue();
    if r.overran() || id > 63 {
        return Err(Error::Invalid("slice_pic_parameter_set_id"));
    }
    Ok(id)
}

fn pred_weight_list(r: &mut BitReader, n: u32, chroma: bool, luma_denom: u32, chroma_denom: u32) -> Result<WeightList> {
    let n = n as usize;
    let mut lf = Vec::with_capacity(n);
    for _ in 0..n {
        lf.push(r.flag());
    }
    let mut cf = alloc::vec![false; n];
    if chroma {
        for f in cf.iter_mut() {
            *f = r.flag();
        }
    }
    let mut w = WeightList::default();
    for i in 0..n {
        let mut lw = (1i32 << luma_denom, 0i32);
        if lf[i] {
            let d = r.se();
            if !(-128..=127).contains(&d) {
                return Err(Error::Invalid("delta_luma_weight"));
            }
            lw = ((1 << luma_denom) + d, r.se());
        }
        w.luma.push(lw);
        let mut cw = [(1i32 << chroma_denom, 0i32); 2];
        if cf[i] {
            for c in cw.iter_mut() {
                let d = r.se();
                if !(-128..=127).contains(&d) {
                    return Err(Error::Invalid("delta_chroma_weight"));
                }
                *c = ((1 << chroma_denom) + d, r.se());
            }
        }
        w.chroma.push(cw);
    }
    Ok(w)
}

/// slice_segment_header() (7.3.6.1). `prev` is the slice's independent segment header, needed by a dependent
/// slice segment (whose omitted fields it copies).
pub fn parse_slice_header(rbsp: &[u8], nal: &NalHeader, sps: &Sps, pps: &Pps, prev: Option<&SliceHeader>) -> Result<SliceHeader> {
    let mut r = BitReader::new(rbsp);
    let mut h = SliceHeader { first_slice_segment_in_pic_flag: r.flag(), ..Default::default() };
    if nal.is_irap() {
        h.no_output_of_prior_pics_flag = r.flag();
    }
    h.pps_id = r.ue();
    if !h.first_slice_segment_in_pic_flag {
        if pps.dependent_slice_segments_enabled_flag {
            h.dependent_slice_segment_flag = r.flag();
        }
        h.segment_address = r.u(ceil_log2(sps.size_in_ctbs()));
        if h.segment_address >= sps.size_in_ctbs() {
            return Err(Error::Invalid("slice_segment_address"));
        }
    }
    if h.dependent_slice_segment_flag {
        let Some(p) = prev else {
            return Err(Error::Invalid("dependent slice segment without its slice"));
        };
        h.inherit(p);
    } else {
        r.skip(pps.num_extra_slice_header_bits as usize);
        h.slice_type = match r.ue() {
            0 => SliceType::B,
            1 => SliceType::P,
            2 => SliceType::I,
            _ => return Err(Error::Invalid("slice_type")),
        };
        h.pic_output_flag = if pps.output_flag_present_flag { r.flag() } else { true };
        if sps.separate_colour_plane_flag {
            h.colour_plane_id = r.u(2) as u8;
        }
        if nal.nal_type != IDR_W_RADL && nal.nal_type != IDR_N_LP {
            h.pic_order_cnt_lsb = r.u(sps.log2_max_poc_lsb);
            h.short_term_ref_pic_set_sps_flag = r.flag();
            let n = sps.st_rps.len();
            if !h.short_term_ref_pic_set_sps_flag {
                h.st_rps = Some(parse_st_rps(&mut r, n, n, &sps.st_rps)?);
            } else {
                if n == 0 {
                    return Err(Error::Invalid("short_term_ref_pic_set_sps_flag with no SPS sets"));
                }
                if n > 1 {
                    h.short_term_ref_pic_set_idx = r.u(ceil_log2(n as u32));
                }
                if h.short_term_ref_pic_set_idx as usize >= n {
                    return Err(Error::Invalid("short_term_ref_pic_set_idx"));
                }
            }
            if sps.long_term_ref_pics_present_flag {
                let nsps = sps.lt_ref_pic_poc_lsb_sps.len() as u32;
                let num_lt_sps = if nsps > 0 { r.ue() } else { 0 };
                if num_lt_sps > nsps {
                    return Err(Error::Invalid("num_long_term_sps"));
                }
                let num_lt_pics = r.ue();
                if num_lt_sps + num_lt_pics > 32 {
                    return Err(Error::Invalid("num_long_term_pics"));
                }
                for i in 0..num_lt_sps + num_lt_pics {
                    let mut lt = LongTerm::default();
                    if i < num_lt_sps {
                        let idx = if nsps > 1 { r.u(ceil_log2(nsps)) } else { 0 };
                        if idx >= nsps {
                            return Err(Error::Invalid("lt_idx_sps"));
                        }
                        lt.poc_lsb = sps.lt_ref_pic_poc_lsb_sps[idx as usize];
                        lt.used = sps.used_by_curr_pic_lt_sps_flag[idx as usize];
                    } else {
                        lt.poc_lsb = r.u(sps.log2_max_poc_lsb);
                        lt.used = r.flag();
                    }
                    if r.flag() {
                        lt.delta_poc_msb_cycle = Some(r.ue());
                    }
                    h.long_term.push(lt);
                }
            }
            if sps.temporal_mvp_enabled_flag {
                h.temporal_mvp_enabled_flag = r.flag();
            }
        }
        if sps.sample_adaptive_offset_enabled_flag {
            h.sao_luma_flag = r.flag();
            if sps.chroma_array_type() != 0 {
                h.sao_chroma_flag = r.flag();
            }
        }
        if h.slice_type != SliceType::I {
            h.num_ref_idx_l0_active = pps.num_ref_idx_l0_default_active;
            if h.slice_type == SliceType::B {
                h.num_ref_idx_l1_active = pps.num_ref_idx_l1_default_active;
            }
            if r.flag() {
                h.num_ref_idx_l0_active = r.ue() + 1;
                if h.slice_type == SliceType::B {
                    h.num_ref_idx_l1_active = r.ue() + 1;
                }
            }
            if h.num_ref_idx_l0_active > 15 || h.num_ref_idx_l1_active > 15 {
                return Err(Error::Invalid("num_ref_idx_active_minus1 above 14"));
            }
            let total = h.num_pic_total_curr(sps);
            if pps.lists_modification_present_flag && total > 1 {
                let bits = ceil_log2(total);
                if r.flag() {
                    h.list_entry_l0 = Some((0..h.num_ref_idx_l0_active).map(|_| r.u(bits)).collect());
                }
                if h.slice_type == SliceType::B && r.flag() {
                    h.list_entry_l1 = Some((0..h.num_ref_idx_l1_active).map(|_| r.u(bits)).collect());
                }
            }
            if h.slice_type == SliceType::B {
                h.mvd_l1_zero_flag = r.flag();
            }
            if pps.cabac_init_present_flag {
                h.cabac_init_flag = r.flag();
            }
            if h.temporal_mvp_enabled_flag {
                h.collocated_from_l0_flag = true;
                if h.slice_type == SliceType::B {
                    h.collocated_from_l0_flag = r.flag();
                }
                if (h.collocated_from_l0_flag && h.num_ref_idx_l0_active > 1) || (!h.collocated_from_l0_flag && h.num_ref_idx_l1_active > 1) {
                    h.collocated_ref_idx = r.ue();
                }
            }
            if (pps.weighted_pred_flag && h.slice_type == SliceType::P) || (pps.weighted_bipred_flag && h.slice_type == SliceType::B) {
                let chroma = sps.chroma_array_type() != 0;
                let ld = r.ue();
                if ld > 7 {
                    return Err(Error::Invalid("luma_log2_weight_denom"));
                }
                let cd = if chroma { ld as i32 + r.se() } else { 0 };
                if !(0..=7).contains(&cd) {
                    return Err(Error::Invalid("delta_chroma_log2_weight_denom"));
                }
                let mut w = PredWeights { luma_log2_weight_denom: ld, chroma_log2_weight_denom: cd as u32, ..Default::default() };
                w.l0 = pred_weight_list(&mut r, h.num_ref_idx_l0_active, chroma, ld, cd as u32)?;
                if h.slice_type == SliceType::B {
                    w.l1 = pred_weight_list(&mut r, h.num_ref_idx_l1_active, chroma, ld, cd as u32)?;
                }
                h.pred_weights = Some(w);
            }
            let five_minus = r.ue();
            if five_minus > 4 {
                return Err(Error::Invalid("five_minus_max_num_merge_cand"));
            }
            h.max_num_merge_cand = 5 - five_minus;
        }
        h.qp_delta = r.se();
        let qp = pps.init_qp + h.qp_delta;
        let off = 6 * (sps.bit_depth_luma as i32 - 8);
        if qp < -off || qp > 51 {
            return Err(Error::Invalid("SliceQpY out of range"));
        }
        if pps.slice_chroma_qp_offsets_present_flag {
            h.cb_qp_offset = r.se();
            h.cr_qp_offset = r.se();
            if !(-12..=12).contains(&h.cb_qp_offset) || !(-12..=12).contains(&h.cr_qp_offset) {
                return Err(Error::Invalid("slice chroma qp offset"));
            }
        }
        if pps.range_ext.chroma_qp_offset_list_enabled_flag {
            h.cu_chroma_qp_offset_enabled_flag = r.flag();
        }
        let override_flag = pps.deblocking_filter_override_enabled_flag && r.flag();
        h.deblocking_filter_disabled_flag = pps.pps_deblocking_filter_disabled_flag;
        h.beta_offset_div2 = pps.beta_offset_div2;
        h.tc_offset_div2 = pps.tc_offset_div2;
        if override_flag {
            h.deblocking_filter_disabled_flag = r.flag();
            if !h.deblocking_filter_disabled_flag {
                h.beta_offset_div2 = r.se();
                h.tc_offset_div2 = r.se();
                if !(-6..=6).contains(&h.beta_offset_div2) || !(-6..=6).contains(&h.tc_offset_div2) {
                    return Err(Error::Invalid("slice deblocking offsets"));
                }
            }
        }
        h.loop_filter_across_slices_enabled_flag = pps.loop_filter_across_slices_enabled_flag;
        if pps.loop_filter_across_slices_enabled_flag && (h.sao_luma_flag || h.sao_chroma_flag || !h.deblocking_filter_disabled_flag) {
            h.loop_filter_across_slices_enabled_flag = r.flag();
        }
    }
    if pps.tiles_enabled_flag || pps.entropy_coding_sync_enabled_flag {
        let n = r.ue();
        let max = if pps.tiles_enabled_flag && pps.entropy_coding_sync_enabled_flag {
            pps.num_tile_columns * sps.height_in_ctbs() - 1
        } else if pps.tiles_enabled_flag {
            pps.num_tile_columns * pps.num_tile_rows - 1
        } else {
            sps.height_in_ctbs() - 1
        };
        if n > max {
            return Err(Error::Invalid("num_entry_point_offsets"));
        }
        if n > 0 {
            let len = r.ue() + 1;
            if len > 32 {
                return Err(Error::Invalid("offset_len_minus1"));
            }
            for _ in 0..n {
                h.entry_point_offsets.push(r.u(len) + 1);
            }
        }
    }
    if pps.slice_segment_header_extension_present_flag {
        let len = r.ue();
        if len > 256 {
            return Err(Error::Invalid("slice_segment_header_extension_length"));
        }
        r.skip(8 * len as usize);
    }
    // byte_alignment(): alignment_bit_equal_to_one, then zero bits.
    if r.bit() != 1 {
        return Err(Error::Invalid("alignment_bit_equal_to_one"));
    }
    while !r.byte_aligned() {
        r.skip(1);
    }
    if r.overran() {
        return Err(Error::Truncated);
    }
    h.data_offset = r.pos() / 8;
    Ok(h)
}
