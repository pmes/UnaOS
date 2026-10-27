// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Parameter sets: profile_tier_level (7.3.3), the video / sequence / picture parameter sets (7.3.2.1-3) with
//! scaling_list_data (7.3.4, the defaults of Tables 7-5 / 7-6 and ScalingFactor 7.4.5), st_ref_pic_set (7.3.7 with the
//! inter-RPS prediction of 7.4.8), the VUI and HRD parameters (E.2.1-3), the scan orders (6.5.3-6.5.5) and the picture
//! geometry: the tile scan (6.5.1) and the z-scan order of 4x4 blocks (6.5.2).

use alloc::vec;
use alloc::vec::Vec;

use h264_core::bits::BitReader;

use crate::tables::{DEFAULT_8X8_INTER, DEFAULT_8X8_INTRA};
use crate::{Error, Result};

fn done<T>(r: &BitReader, v: T) -> Result<T> {
    if r.overran() { Err(Error::Truncated) } else { Ok(v) }
}

fn ue_max(r: &mut BitReader, max: u32, what: &'static str) -> Result<u32> {
    let v = r.ue();
    if v > max {
        return Err(Error::Invalid(what));
    }
    Ok(v)
}

/// Ceil(Log2(n)) for n >= 1.
pub fn ceil_log2(n: u32) -> u32 {
    if n <= 1 { 0 } else { 32 - (n - 1).leading_zeros() }
}

// ---------------------------------------------------------------- scans (6.5.3 .. 6.5.5)

const fn build_scans() -> [[[(u8, u8); 64]; 3]; 4] {
    let mut s = [[[(0u8, 0u8); 64]; 3]; 4];
    let mut log2 = 0;
    while log2 < 4 {
        let blk: i32 = 1 << log2;
        // 6.5.3 up-right diagonal.
        let mut i = 0usize;
        let mut x: i32 = 0;
        let mut y: i32 = 0;
        let mut stop = false;
        while !stop {
            while y >= 0 {
                if x < blk && y < blk {
                    s[log2][0][i] = (x as u8, y as u8);
                    i += 1;
                }
                y -= 1;
                x += 1;
            }
            y = x;
            x = 0;
            if i >= (blk * blk) as usize {
                stop = true;
            }
        }
        // 6.5.4 horizontal, 6.5.5 vertical.
        let mut i = 0usize;
        let mut a = 0;
        while a < blk {
            let mut b = 0;
            while b < blk {
                s[log2][1][i] = (b as u8, a as u8);
                s[log2][2][i] = (a as u8, b as u8);
                i += 1;
                b += 1;
            }
            a += 1;
        }
        log2 += 1;
    }
    s
}

/// ScanOrder[log2BlockSize][scanIdx][sPos] = (x, y) for log2BlockSize 0..=3 and scanIdx 0 (up-right diagonal),
/// 1 (horizontal), 2 (vertical).
pub const SCAN_ORDER: [[[(u8, u8); 64]; 3]; 4] = build_scans();

// ---------------------------------------------------------------- profile_tier_level (7.3.3)

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProfileTierLevel {
    pub general_profile_space: u8,
    pub general_tier_flag: bool,
    pub general_profile_idc: u8,
    pub general_profile_compatibility_flags: u32,
    pub general_progressive_source_flag: bool,
    pub general_interlaced_source_flag: bool,
    pub general_non_packed_constraint_flag: bool,
    pub general_frame_only_constraint_flag: bool,
    pub general_level_idc: u8,
    /// sub_layer_level_idc per sub-layer, when present.
    pub sub_layer_level_idc: Vec<Option<u8>>,
}

pub fn parse_ptl(r: &mut BitReader, profile_present: bool, max_sub_layers_minus1: u32) -> ProfileTierLevel {
    let mut p = ProfileTierLevel::default();
    if profile_present {
        p.general_profile_space = r.u(2) as u8;
        p.general_tier_flag = r.flag();
        p.general_profile_idc = r.u(5) as u8;
        p.general_profile_compatibility_flags = r.u(32);
        p.general_progressive_source_flag = r.flag();
        p.general_interlaced_source_flag = r.flag();
        p.general_non_packed_constraint_flag = r.flag();
        p.general_frame_only_constraint_flag = r.flag();
        r.skip(44); // the 43 constraint / reserved bits and general_inbld_flag / reserved
    }
    p.general_level_idc = r.u(8) as u8;
    let n = max_sub_layers_minus1 as usize;
    let mut prof = [false; 8];
    let mut lvl = [false; 8];
    for i in 0..n {
        prof[i] = r.flag();
        lvl[i] = r.flag();
    }
    if n > 0 {
        for _ in n..8 {
            r.skip(2); // reserved_zero_2bits
        }
    }
    for i in 0..n {
        if prof[i] {
            r.skip(88);
        }
        p.sub_layer_level_idc.push(if lvl[i] { Some(r.u(8) as u8) } else { None });
    }
    p
}

// ---------------------------------------------------------------- HRD (E.2.2 / E.2.3)

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Hrd {
    pub nal_hrd_parameters_present_flag: bool,
    pub vcl_hrd_parameters_present_flag: bool,
    pub sub_pic_hrd_params_present_flag: bool,
    /// cpb_cnt_minus1 per sub-layer.
    pub cpb_cnt_minus1: Vec<u32>,
}

pub fn parse_hrd(r: &mut BitReader, common_inf: bool, max_sub_layers_minus1: u32) -> Hrd {
    let mut h = Hrd::default();
    if common_inf {
        h.nal_hrd_parameters_present_flag = r.flag();
        h.vcl_hrd_parameters_present_flag = r.flag();
        if h.nal_hrd_parameters_present_flag || h.vcl_hrd_parameters_present_flag {
            h.sub_pic_hrd_params_present_flag = r.flag();
            if h.sub_pic_hrd_params_present_flag {
                r.skip(8 + 5 + 1 + 5); // tick_divisor_minus2, du_cpb_removal_delay_increment_length_minus1, sub_pic_cpb_params_in_pic_timing_sei_flag, dpb_output_delay_du_length_minus1
            }
            r.skip(4 + 4); // bit_rate_scale, cpb_size_scale
            if h.sub_pic_hrd_params_present_flag {
                r.skip(4); // cpb_size_du_scale
            }
            r.skip(5 + 5 + 5); // initial_cpb_removal_delay_length_minus1, au_cpb_removal_delay_length_minus1, dpb_output_delay_length_minus1
        }
    }
    for _ in 0..=max_sub_layers_minus1 {
        let fixed_general = r.flag();
        let fixed_cvs = if fixed_general { true } else { r.flag() };
        let mut low_delay = false;
        if fixed_cvs {
            r.ue(); // elemental_duration_in_tc_minus1
        } else {
            low_delay = r.flag();
        }
        let cpb_cnt = if low_delay { 0 } else { r.ue().min(31) };
        h.cpb_cnt_minus1.push(cpb_cnt);
        for present in [h.nal_hrd_parameters_present_flag, h.vcl_hrd_parameters_present_flag] {
            if present {
                // sub_layer_hrd_parameters (E.2.3)
                for _ in 0..=cpb_cnt {
                    r.ue(); // bit_rate_value_minus1
                    r.ue(); // cpb_size_value_minus1
                    if h.sub_pic_hrd_params_present_flag {
                        r.ue(); // cpb_size_du_value_minus1
                        r.ue(); // bit_rate_du_value_minus1
                    }
                    r.skip(1); // cbr_flag
                }
            }
        }
    }
    h
}

// ---------------------------------------------------------------- VPS (7.3.2.1)

/// The DPB size, reorder depth and latency of one sub-layer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ordering {
    pub max_dec_pic_buffering_minus1: u32,
    pub max_num_reorder_pics: u32,
    pub max_latency_increase_plus1: u32,
}

fn parse_ordering(r: &mut BitReader, max_sub_layers_minus1: u32) -> Vec<Ordering> {
    let present = r.flag();
    let n = max_sub_layers_minus1 as usize + 1;
    let mut v = vec![Ordering::default(); n];
    let first = if present { 0 } else { n - 1 };
    for o in v.iter_mut().skip(first) {
        o.max_dec_pic_buffering_minus1 = r.ue();
        o.max_num_reorder_pics = r.ue();
        o.max_latency_increase_plus1 = r.ue();
    }
    if !present {
        let last = v[n - 1];
        for o in v.iter_mut() {
            *o = last;
        }
    }
    v
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Vps {
    pub id: u8,
    pub base_layer_internal_flag: bool,
    pub base_layer_available_flag: bool,
    pub max_layers_minus1: u8,
    pub max_sub_layers_minus1: u8,
    pub temporal_id_nesting_flag: bool,
    pub ptl: ProfileTierLevel,
    pub ordering: Vec<Ordering>,
    pub max_layer_id: u8,
    pub num_layer_sets_minus1: u32,
    /// (num_units_in_tick, time_scale) when vps_timing_info_present_flag.
    pub timing: Option<(u32, u32)>,
    pub hrd: Vec<Hrd>,
    pub extension_flag: bool,
}

pub fn parse_vps(rbsp: &[u8]) -> Result<Vps> {
    let mut r = BitReader::new(rbsp);
    let mut v = Vps {
        id: r.u(4) as u8,
        base_layer_internal_flag: r.flag(),
        base_layer_available_flag: r.flag(),
        max_layers_minus1: r.u(6) as u8,
        max_sub_layers_minus1: r.u(3) as u8,
        temporal_id_nesting_flag: r.flag(),
        ..Default::default()
    };
    if v.max_sub_layers_minus1 > 6 {
        return Err(Error::Invalid("vps_max_sub_layers_minus1 above 6"));
    }
    if r.u(16) != 0xffff {
        return Err(Error::Invalid("vps_reserved_0xffff_16bits"));
    }
    v.ptl = parse_ptl(&mut r, true, v.max_sub_layers_minus1 as u32);
    v.ordering = parse_ordering(&mut r, v.max_sub_layers_minus1 as u32);
    v.max_layer_id = r.u(6) as u8;
    v.num_layer_sets_minus1 = ue_max(&mut r, 1023, "vps_num_layer_sets_minus1")?;
    for _ in 1..=v.num_layer_sets_minus1 {
        r.skip(v.max_layer_id as usize + 1); // layer_id_included_flag
    }
    if r.flag() {
        let tick = r.u(32);
        let scale = r.u(32);
        v.timing = Some((tick, scale));
        if r.flag() {
            r.ue(); // vps_num_ticks_poc_diff_one_minus1
        }
        let n = ue_max(&mut r, 1024, "vps_num_hrd_parameters")?;
        for i in 0..n {
            r.ue(); // hrd_layer_set_idx
            let cprms = if i > 0 { r.flag() } else { true };
            let h = parse_hrd(&mut r, cprms, v.max_sub_layers_minus1 as u32);
            v.hrd.push(h);
        }
    }
    v.extension_flag = r.flag();
    done(&r, v)
}

// ---------------------------------------------------------------- scaling lists (7.3.4, 7.4.5)

/// ScalingFactor (7.4.5) for every size and matrixId: `factor[sizeId][matrixId]` is nTbS x nTbS, indexed [y * nTbS + x]
/// (sizeId 0..3 = 4x4..32x32; matrixId 0..2 intra Y/Cb/Cr, 3..5 inter).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScalingList {
    /// ScalingList[sizeId][matrixId][i] (coefficients in up-right diagonal order; 16 used for sizeId 0).
    pub list: [[[u8; 64]; 6]; 4],
    /// scaling_list_dc_coef_minus8 + 8 for sizeId 2, 3 (16 when not signalled).
    pub dc: [[u8; 6]; 4],
    pub factor: [[Vec<u8>; 6]; 4],
}

impl ScalingList {
    /// The default lists (Table 7-5: flat 16 for 4x4; Table 7-6 for 8x8 and up).
    pub fn default_lists() -> ScalingList {
        let mut list = [[[16u8; 64]; 6]; 4];
        for s in 1..4 {
            for m in 0..6 {
                list[s][m] = if m < 3 { DEFAULT_8X8_INTRA } else { DEFAULT_8X8_INTER };
            }
        }
        let mut sl = ScalingList { list, dc: [[16; 6]; 4], factor: Default::default() };
        sl.derive();
        sl
    }

    fn default_list(size_id: usize, matrix_id: usize) -> [u8; 64] {
        if size_id == 0 {
            [16; 64]
        } else if matrix_id < 3 {
            DEFAULT_8X8_INTRA
        } else {
            DEFAULT_8X8_INTER
        }
    }

    /// scaling_list_data() (7.3.4).
    pub fn parse(r: &mut BitReader) -> Result<ScalingList> {
        let mut sl = ScalingList::default_lists();
        for size_id in 0..4 {
            let step = if size_id == 3 { 3 } else { 1 };
            let mut matrix_id = 0;
            while matrix_id < 6 {
                if !r.flag() {
                    // scaling_list_pred_mode_flag 0: a default or an earlier list.
                    let delta = r.ue() as usize;
                    if delta * step > matrix_id {
                        return Err(Error::Invalid("scaling_list_pred_matrix_id_delta"));
                    }
                    if delta == 0 {
                        sl.list[size_id][matrix_id] = Self::default_list(size_id, matrix_id);
                        sl.dc[size_id][matrix_id] = 16;
                    } else {
                        let ref_id = matrix_id - delta * step;
                        sl.list[size_id][matrix_id] = sl.list[size_id][ref_id];
                        sl.dc[size_id][matrix_id] = sl.dc[size_id][ref_id];
                    }
                } else {
                    let mut next: i32 = 8;
                    let coef_num = core::cmp::min(64, 1 << (4 + (size_id << 1)));
                    if size_id > 1 {
                        let dc = r.se();
                        if !(-7..=247).contains(&dc) {
                            return Err(Error::Invalid("scaling_list_dc_coef_minus8"));
                        }
                        next = dc + 8;
                        sl.dc[size_id][matrix_id] = next as u8;
                    }
                    for i in 0..coef_num {
                        let d = r.se();
                        if !(-128..=127).contains(&d) {
                            return Err(Error::Invalid("scaling_list_delta_coef"));
                        }
                        next = (next + d + 256) % 256;
                        if next == 0 {
                            return Err(Error::Invalid("scaling list entry 0"));
                        }
                        sl.list[size_id][matrix_id][i] = next as u8;
                    }
                }
                matrix_id += step;
            }
        }
        // 7.3.4 (ChromaArrayType 3 only uses them): the 32x32 chroma lists follow the 16x16 ones.
        for m in [1, 2, 4, 5] {
            sl.list[3][m] = sl.list[2][m];
            sl.dc[3][m] = sl.dc[2][m];
        }
        sl.derive();
        done(r, sl)
    }

    /// ScalingFactor (7.4.5).
    fn derive(&mut self) {
        for size_id in 0..4 {
            let n = 4usize << size_id;
            for m in 0..6 {
                let mut f = vec![0u8; n * n];
                if size_id == 0 {
                    for i in 0..16 {
                        let (x, y) = SCAN_ORDER[2][0][i];
                        f[y as usize * 4 + x as usize] = self.list[0][m][i];
                    }
                } else {
                    let rep = n / 8;
                    for i in 0..64 {
                        let (x, y) = SCAN_ORDER[3][0][i];
                        for j in 0..rep {
                            for k in 0..rep {
                                f[(y as usize * rep + j) * n + x as usize * rep + k] = self.list[size_id][m][i];
                            }
                        }
                    }
                    if size_id > 1 {
                        f[0] = self.dc[size_id][m];
                    }
                }
                self.factor[size_id][m] = f;
            }
        }
    }
}

// ---------------------------------------------------------------- st_ref_pic_set (7.3.7, 7.4.8)

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StRps {
    /// DeltaPocS0 (negative, nearest first) and UsedByCurrPicS0.
    pub delta_poc_s0: Vec<i32>,
    pub used_s0: Vec<bool>,
    /// DeltaPocS1 (positive, nearest first) and UsedByCurrPicS1.
    pub delta_poc_s1: Vec<i32>,
    pub used_s1: Vec<bool>,
}

impl StRps {
    pub fn num_negative(&self) -> usize {
        self.delta_poc_s0.len()
    }
    pub fn num_positive(&self) -> usize {
        self.delta_poc_s1.len()
    }
    pub fn num_delta_pocs(&self) -> usize {
        self.num_negative() + self.num_positive()
    }
    /// The pictures of this set the current picture may reference (the RPS part of NumPicTotalCurr, 7.4.7.2).
    pub fn num_used(&self) -> usize {
        self.used_s0.iter().chain(self.used_s1.iter()).filter(|&&u| u).count()
    }
}

/// st_ref_pic_set(stRpsIdx): `sets` holds the SPS's sets 0..stRpsIdx; `num_sets` is num_short_term_ref_pic_sets
/// (stRpsIdx == num_sets is the slice header's own set).
pub fn parse_st_rps(r: &mut BitReader, idx: usize, num_sets: usize, sets: &[StRps]) -> Result<StRps> {
    let inter = idx != 0 && r.flag();
    let mut s = StRps::default();
    if inter {
        let delta_idx = if idx == num_sets { r.ue() as usize + 1 } else { 1 };
        if delta_idx > idx {
            return Err(Error::Invalid("delta_idx_minus1"));
        }
        let rf = &sets[idx - delta_idx];
        let sign = r.flag();
        let abs = r.ue() as i32 + 1;
        if abs > 1 << 15 {
            return Err(Error::Invalid("abs_delta_rps_minus1"));
        }
        let delta_rps = if sign { -abs } else { abs };
        let n = rf.num_delta_pocs();
        let mut used = vec![false; n + 1];
        let mut use_delta = vec![true; n + 1];
        for j in 0..=n {
            used[j] = r.flag();
            if !used[j] {
                use_delta[j] = r.flag();
            }
        }
        let nn = rf.num_negative();
        let np = rf.num_positive();
        // 7-61
        for j in (0..np).rev() {
            let d = rf.delta_poc_s1[j] + delta_rps;
            if d < 0 && use_delta[nn + j] {
                s.delta_poc_s0.push(d);
                s.used_s0.push(used[nn + j]);
            }
        }
        if delta_rps < 0 && use_delta[n] {
            s.delta_poc_s0.push(delta_rps);
            s.used_s0.push(used[n]);
        }
        for j in 0..nn {
            let d = rf.delta_poc_s0[j] + delta_rps;
            if d < 0 && use_delta[j] {
                s.delta_poc_s0.push(d);
                s.used_s0.push(used[j]);
            }
        }
        // 7-62
        for j in (0..nn).rev() {
            let d = rf.delta_poc_s0[j] + delta_rps;
            if d > 0 && use_delta[j] {
                s.delta_poc_s1.push(d);
                s.used_s1.push(used[j]);
            }
        }
        if delta_rps > 0 && use_delta[n] {
            s.delta_poc_s1.push(delta_rps);
            s.used_s1.push(used[n]);
        }
        for j in 0..np {
            let d = rf.delta_poc_s1[j] + delta_rps;
            if d > 0 && use_delta[nn + j] {
                s.delta_poc_s1.push(d);
                s.used_s1.push(used[nn + j]);
            }
        }
    } else {
        let nn = ue_max(r, 16, "num_negative_pics")? as usize;
        let np = ue_max(r, 16, "num_positive_pics")? as usize;
        let mut poc = 0i32;
        for _ in 0..nn {
            poc -= r.ue().min(1 << 15) as i32 + 1;
            s.delta_poc_s0.push(poc);
            s.used_s0.push(r.flag());
        }
        poc = 0;
        for _ in 0..np {
            poc += r.ue().min(1 << 15) as i32 + 1;
            s.delta_poc_s1.push(poc);
            s.used_s1.push(r.flag());
        }
    }
    if s.num_delta_pocs() > 16 {
        return Err(Error::Invalid("st_ref_pic_set holds more than 16 pictures"));
    }
    done(r, s)
}

// ---------------------------------------------------------------- VUI (E.2.1)

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Vui {
    pub aspect_ratio_idc: u8,
    pub sar: (u16, u16),
    pub overscan_appropriate: Option<bool>,
    pub video_format: u8,
    pub video_full_range_flag: bool,
    /// (colour_primaries, transfer_characteristics, matrix_coeffs) when colour_description_present_flag.
    pub colour: Option<(u8, u8, u8)>,
    pub chroma_loc: Option<(u32, u32)>,
    pub neutral_chroma_indication_flag: bool,
    pub field_seq_flag: bool,
    pub frame_field_info_present_flag: bool,
    /// default display window offsets (left, right, top, bottom).
    pub default_display_window: Option<[u32; 4]>,
    /// (num_units_in_tick, time_scale).
    pub timing: Option<(u32, u32)>,
    pub hrd: Option<Hrd>,
    pub bitstream_restriction_flag: bool,
    pub max_bytes_per_pic_denom: u32,
    pub max_bits_per_min_cu_denom: u32,
}

pub fn parse_vui(r: &mut BitReader, max_sub_layers_minus1: u32) -> Vui {
    let mut v = Vui { video_format: 5, ..Default::default() };
    if r.flag() {
        v.aspect_ratio_idc = r.u(8) as u8;
        if v.aspect_ratio_idc == 255 {
            v.sar = (r.u(16) as u16, r.u(16) as u16);
        }
    }
    if r.flag() {
        v.overscan_appropriate = Some(r.flag());
    }
    if r.flag() {
        v.video_format = r.u(3) as u8;
        v.video_full_range_flag = r.flag();
        if r.flag() {
            v.colour = Some((r.u(8) as u8, r.u(8) as u8, r.u(8) as u8));
        }
    }
    if r.flag() {
        v.chroma_loc = Some((r.ue(), r.ue()));
    }
    v.neutral_chroma_indication_flag = r.flag();
    v.field_seq_flag = r.flag();
    v.frame_field_info_present_flag = r.flag();
    if r.flag() {
        v.default_display_window = Some([r.ue(), r.ue(), r.ue(), r.ue()]);
    }
    if r.flag() {
        let tick = r.u(32);
        let scale = r.u(32);
        v.timing = Some((tick, scale));
        if r.flag() {
            r.ue(); // vui_num_ticks_poc_diff_one_minus1
        }
        if r.flag() {
            v.hrd = Some(parse_hrd(r, true, max_sub_layers_minus1));
        }
    }
    v.bitstream_restriction_flag = r.flag();
    if v.bitstream_restriction_flag {
        r.skip(3); // tiles_fixed_structure_flag, motion_vectors_over_pic_boundaries_flag, restricted_ref_pic_lists_flag
        r.ue(); // min_spatial_segmentation_idc
        v.max_bytes_per_pic_denom = r.ue();
        v.max_bits_per_min_cu_denom = r.ue();
        r.ue(); // log2_max_mv_length_horizontal
        r.ue(); // log2_max_mv_length_vertical
    }
    v
}

// ---------------------------------------------------------------- SPS (7.3.2.2)

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pcm {
    pub bit_depth_luma: u32,
    pub bit_depth_chroma: u32,
    pub log2_min_cb: u32,
    pub log2_max_cb: u32,
    pub loop_filter_disabled: bool,
}

/// sps_range_extension() (7.3.2.2.2) flags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpsRangeExt {
    pub transform_skip_rotation_enabled_flag: bool,
    pub transform_skip_context_enabled_flag: bool,
    pub implicit_rdpcm_enabled_flag: bool,
    pub explicit_rdpcm_enabled_flag: bool,
    pub extended_precision_processing_flag: bool,
    pub intra_smoothing_disabled_flag: bool,
    pub high_precision_offsets_enabled_flag: bool,
    pub persistent_rice_adaptation_enabled_flag: bool,
    pub cabac_bypass_alignment_enabled_flag: bool,
}

impl SpsRangeExt {
    pub fn any(&self) -> bool {
        *self != SpsRangeExt::default()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Sps {
    pub vps_id: u8,
    pub max_sub_layers_minus1: u8,
    pub temporal_id_nesting_flag: bool,
    pub ptl: ProfileTierLevel,
    pub id: u32,
    pub chroma_format_idc: u32,
    pub separate_colour_plane_flag: bool,
    pub width: u32,
    pub height: u32,
    /// conf_win_{left,right,top,bottom}_offset (in chroma sample units for 4:2:0).
    pub conf_win: [u32; 4],
    pub bit_depth_luma: u32,
    pub bit_depth_chroma: u32,
    pub log2_max_poc_lsb: u32,
    pub ordering: Vec<Ordering>,
    pub log2_min_cb: u32,
    pub log2_ctb: u32,
    pub log2_min_tb: u32,
    pub log2_max_tb: u32,
    pub max_transform_hierarchy_depth_inter: u32,
    pub max_transform_hierarchy_depth_intra: u32,
    pub scaling_list_enabled_flag: bool,
    /// The SPS's lists (signalled, else the defaults) when scaling_list_enabled_flag.
    pub scaling_list: Option<ScalingList>,
    pub amp_enabled_flag: bool,
    pub sample_adaptive_offset_enabled_flag: bool,
    pub pcm: Option<Pcm>,
    pub st_rps: Vec<StRps>,
    pub long_term_ref_pics_present_flag: bool,
    pub lt_ref_pic_poc_lsb_sps: Vec<u32>,
    pub used_by_curr_pic_lt_sps_flag: Vec<bool>,
    pub temporal_mvp_enabled_flag: bool,
    pub strong_intra_smoothing_enabled_flag: bool,
    pub vui: Option<Vui>,
    pub range_ext: SpsRangeExt,
    pub multilayer_extension_flag: bool,
    pub extension_3d_flag: bool,
    pub scc_extension_flag: bool,
}

impl Sps {
    pub fn ctb_size(&self) -> u32 {
        1 << self.log2_ctb
    }
    pub fn width_in_ctbs(&self) -> u32 {
        self.width.div_ceil(self.ctb_size())
    }
    pub fn height_in_ctbs(&self) -> u32 {
        self.height.div_ceil(self.ctb_size())
    }
    pub fn size_in_ctbs(&self) -> u32 {
        self.width_in_ctbs() * self.height_in_ctbs()
    }
    /// ChromaArrayType (7.4.3.2.1).
    pub fn chroma_array_type(&self) -> u32 {
        if self.separate_colour_plane_flag { 0 } else { self.chroma_format_idc }
    }
    /// The ordering of the highest sub-layer (HighestTid = sps_max_sub_layers_minus1).
    pub fn highest_ordering(&self) -> Ordering {
        self.ordering.last().copied().unwrap_or_default()
    }
}

pub fn parse_sps(rbsp: &[u8]) -> Result<Sps> {
    let mut r = BitReader::new(rbsp);
    let mut s = Sps { vps_id: r.u(4) as u8, max_sub_layers_minus1: r.u(3) as u8, temporal_id_nesting_flag: r.flag(), ..Default::default() };
    if s.max_sub_layers_minus1 > 6 {
        return Err(Error::Invalid("sps_max_sub_layers_minus1 above 6"));
    }
    s.ptl = parse_ptl(&mut r, true, s.max_sub_layers_minus1 as u32);
    s.id = ue_max(&mut r, 15, "sps_seq_parameter_set_id")?;
    s.chroma_format_idc = ue_max(&mut r, 3, "chroma_format_idc")?;
    if s.chroma_format_idc == 3 {
        s.separate_colour_plane_flag = r.flag();
    }
    s.width = r.ue();
    s.height = r.ue();
    if r.flag() {
        s.conf_win = [r.ue(), r.ue(), r.ue(), r.ue()];
    }
    s.bit_depth_luma = ue_max(&mut r, 8, "bit_depth_luma_minus8")? + 8;
    s.bit_depth_chroma = ue_max(&mut r, 8, "bit_depth_chroma_minus8")? + 8;
    s.log2_max_poc_lsb = ue_max(&mut r, 12, "log2_max_pic_order_cnt_lsb_minus4")? + 4;
    s.ordering = parse_ordering(&mut r, s.max_sub_layers_minus1 as u32);
    s.log2_min_cb = ue_max(&mut r, 3, "log2_min_luma_coding_block_size_minus3")? + 3;
    s.log2_ctb = s.log2_min_cb + ue_max(&mut r, 3, "log2_diff_max_min_luma_coding_block_size")?;
    s.log2_min_tb = ue_max(&mut r, 3, "log2_min_luma_transform_block_size_minus2")? + 2;
    s.log2_max_tb = s.log2_min_tb + ue_max(&mut r, 3, "log2_diff_max_min_luma_transform_block_size")?;
    s.max_transform_hierarchy_depth_inter = ue_max(&mut r, 4, "max_transform_hierarchy_depth_inter")?;
    s.max_transform_hierarchy_depth_intra = ue_max(&mut r, 4, "max_transform_hierarchy_depth_intra")?;
    if !(4..=6).contains(&s.log2_ctb) {
        return Err(Error::Invalid("CtbLog2SizeY outside 4..6"));
    }
    if s.log2_min_tb >= s.log2_min_cb || s.log2_max_tb > s.log2_ctb.min(5) {
        return Err(Error::Invalid("transform block sizes"));
    }
    if s.width == 0 || s.height == 0 || s.width % (1 << s.log2_min_cb) != 0 || s.height % (1 << s.log2_min_cb) != 0 {
        return Err(Error::Invalid("picture size not a multiple of MinCbSizeY"));
    }
    if s.width > 16888 || s.height > 16888 {
        return Err(Error::Unsupported("picture larger than level 6.2"));
    }
    s.scaling_list_enabled_flag = r.flag();
    if s.scaling_list_enabled_flag {
        s.scaling_list = Some(if r.flag() { ScalingList::parse(&mut r)? } else { ScalingList::default_lists() });
    }
    s.amp_enabled_flag = r.flag();
    s.sample_adaptive_offset_enabled_flag = r.flag();
    if r.flag() {
        let bl = r.u(4) + 1;
        let bc = r.u(4) + 1;
        let lmin = r.ue() + 3;
        let lmax = lmin + r.ue();
        s.pcm = Some(Pcm { bit_depth_luma: bl, bit_depth_chroma: bc, log2_min_cb: lmin, log2_max_cb: lmax, loop_filter_disabled: r.flag() });
        if bl > s.bit_depth_luma || bc > s.bit_depth_chroma || lmax > 5.min(s.log2_ctb) {
            return Err(Error::Invalid("pcm parameters"));
        }
    }
    let n = ue_max(&mut r, 64, "num_short_term_ref_pic_sets")? as usize;
    for i in 0..n {
        let st = parse_st_rps(&mut r, i, n, &s.st_rps)?;
        s.st_rps.push(st);
    }
    s.long_term_ref_pics_present_flag = r.flag();
    if s.long_term_ref_pics_present_flag {
        let n = ue_max(&mut r, 32, "num_long_term_ref_pics_sps")?;
        for _ in 0..n {
            s.lt_ref_pic_poc_lsb_sps.push(r.u(s.log2_max_poc_lsb));
            s.used_by_curr_pic_lt_sps_flag.push(r.flag());
        }
    }
    s.temporal_mvp_enabled_flag = r.flag();
    s.strong_intra_smoothing_enabled_flag = r.flag();
    if r.flag() {
        s.vui = Some(parse_vui(&mut r, s.max_sub_layers_minus1 as u32));
    }
    if r.flag() {
        let range = r.flag();
        s.multilayer_extension_flag = r.flag();
        s.extension_3d_flag = r.flag();
        s.scc_extension_flag = r.flag();
        r.skip(4); // sps_extension_4bits
        if range {
            let mut f = [false; 9];
            for b in f.iter_mut() {
                *b = r.flag();
            }
            s.range_ext = SpsRangeExt {
                transform_skip_rotation_enabled_flag: f[0],
                transform_skip_context_enabled_flag: f[1],
                implicit_rdpcm_enabled_flag: f[2],
                explicit_rdpcm_enabled_flag: f[3],
                extended_precision_processing_flag: f[4],
                intra_smoothing_disabled_flag: f[5],
                high_precision_offsets_enabled_flag: f[6],
                persistent_rice_adaptation_enabled_flag: f[7],
                cabac_bypass_alignment_enabled_flag: f[8],
            };
        }
        // The multilayer / 3D / SCC extension bodies and sps_extension_data_flag follow; the base layer needs none
        // of them (SCC is refused at activation).
        return done(&r, s);
    }
    done(&r, s)
}

// ---------------------------------------------------------------- PPS (7.3.2.3)

/// pps_range_extension() (7.3.2.3.2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PpsRangeExt {
    pub log2_max_transform_skip_block_size_minus2: u32,
    pub cross_component_prediction_enabled_flag: bool,
    pub chroma_qp_offset_list_enabled_flag: bool,
    pub diff_cu_chroma_qp_offset_depth: u32,
    pub cb_qp_offset_list: Vec<i32>,
    pub cr_qp_offset_list: Vec<i32>,
    pub log2_sao_offset_scale_luma: u32,
    pub log2_sao_offset_scale_chroma: u32,
}

impl PpsRangeExt {
    pub fn any(&self) -> bool {
        *self != PpsRangeExt::default()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Pps {
    pub id: u32,
    pub sps_id: u32,
    pub dependent_slice_segments_enabled_flag: bool,
    pub output_flag_present_flag: bool,
    pub num_extra_slice_header_bits: u32,
    pub sign_data_hiding_enabled_flag: bool,
    pub cabac_init_present_flag: bool,
    pub num_ref_idx_l0_default_active: u32,
    pub num_ref_idx_l1_default_active: u32,
    pub init_qp: i32,
    pub constrained_intra_pred_flag: bool,
    pub transform_skip_enabled_flag: bool,
    pub cu_qp_delta_enabled_flag: bool,
    pub diff_cu_qp_delta_depth: u32,
    pub cb_qp_offset: i32,
    pub cr_qp_offset: i32,
    pub slice_chroma_qp_offsets_present_flag: bool,
    pub weighted_pred_flag: bool,
    pub weighted_bipred_flag: bool,
    pub transquant_bypass_enabled_flag: bool,
    pub tiles_enabled_flag: bool,
    pub entropy_coding_sync_enabled_flag: bool,
    pub num_tile_columns: u32,
    pub num_tile_rows: u32,
    pub uniform_spacing_flag: bool,
    /// column_width_minus1 + 1 / row_height_minus1 + 1 (all but the last) when not uniform.
    pub column_widths: Vec<u32>,
    pub row_heights: Vec<u32>,
    pub loop_filter_across_tiles_enabled_flag: bool,
    pub loop_filter_across_slices_enabled_flag: bool,
    pub deblocking_filter_control_present_flag: bool,
    pub deblocking_filter_override_enabled_flag: bool,
    pub pps_deblocking_filter_disabled_flag: bool,
    pub beta_offset_div2: i32,
    pub tc_offset_div2: i32,
    pub scaling_list: Option<ScalingList>,
    pub lists_modification_present_flag: bool,
    pub log2_parallel_merge_level: u32,
    pub slice_segment_header_extension_present_flag: bool,
    pub range_ext: PpsRangeExt,
    pub multilayer_extension_flag: bool,
    pub extension_3d_flag: bool,
    pub scc_extension_flag: bool,
}

pub fn parse_pps(rbsp: &[u8]) -> Result<Pps> {
    let mut r = BitReader::new(rbsp);
    let mut p = Pps { id: ue_max(&mut r, 63, "pps_pic_parameter_set_id")?, sps_id: ue_max(&mut r, 15, "pps_seq_parameter_set_id")?, ..Default::default() };
    p.dependent_slice_segments_enabled_flag = r.flag();
    p.output_flag_present_flag = r.flag();
    p.num_extra_slice_header_bits = r.u(3);
    p.sign_data_hiding_enabled_flag = r.flag();
    p.cabac_init_present_flag = r.flag();
    p.num_ref_idx_l0_default_active = ue_max(&mut r, 14, "num_ref_idx_l0_default_active_minus1")? + 1;
    p.num_ref_idx_l1_default_active = ue_max(&mut r, 14, "num_ref_idx_l1_default_active_minus1")? + 1;
    p.init_qp = 26 + r.se();
    p.constrained_intra_pred_flag = r.flag();
    p.transform_skip_enabled_flag = r.flag();
    p.cu_qp_delta_enabled_flag = r.flag();
    if p.cu_qp_delta_enabled_flag {
        p.diff_cu_qp_delta_depth = ue_max(&mut r, 3, "diff_cu_qp_delta_depth")?;
    }
    p.cb_qp_offset = r.se();
    p.cr_qp_offset = r.se();
    if !(-12..=12).contains(&p.cb_qp_offset) || !(-12..=12).contains(&p.cr_qp_offset) {
        return Err(Error::Invalid("pps chroma qp offset"));
    }
    p.slice_chroma_qp_offsets_present_flag = r.flag();
    p.weighted_pred_flag = r.flag();
    p.weighted_bipred_flag = r.flag();
    p.transquant_bypass_enabled_flag = r.flag();
    p.tiles_enabled_flag = r.flag();
    p.entropy_coding_sync_enabled_flag = r.flag();
    p.num_tile_columns = 1;
    p.num_tile_rows = 1;
    p.uniform_spacing_flag = true;
    p.loop_filter_across_tiles_enabled_flag = true;
    if p.tiles_enabled_flag {
        p.num_tile_columns = ue_max(&mut r, 19, "num_tile_columns_minus1")? + 1;
        p.num_tile_rows = ue_max(&mut r, 21, "num_tile_rows_minus1")? + 1;
        p.uniform_spacing_flag = r.flag();
        if !p.uniform_spacing_flag {
            for _ in 1..p.num_tile_columns {
                p.column_widths.push(r.ue() + 1);
            }
            for _ in 1..p.num_tile_rows {
                p.row_heights.push(r.ue() + 1);
            }
        }
        p.loop_filter_across_tiles_enabled_flag = r.flag();
    }
    p.loop_filter_across_slices_enabled_flag = r.flag();
    p.deblocking_filter_control_present_flag = r.flag();
    if p.deblocking_filter_control_present_flag {
        p.deblocking_filter_override_enabled_flag = r.flag();
        p.pps_deblocking_filter_disabled_flag = r.flag();
        if !p.pps_deblocking_filter_disabled_flag {
            p.beta_offset_div2 = r.se();
            p.tc_offset_div2 = r.se();
            if !(-6..=6).contains(&p.beta_offset_div2) || !(-6..=6).contains(&p.tc_offset_div2) {
                return Err(Error::Invalid("pps deblocking offsets"));
            }
        }
    }
    if r.flag() {
        p.scaling_list = Some(ScalingList::parse(&mut r)?);
    }
    p.lists_modification_present_flag = r.flag();
    p.log2_parallel_merge_level = r.ue() + 2;
    p.slice_segment_header_extension_present_flag = r.flag();
    if r.flag() {
        let range = r.flag();
        p.multilayer_extension_flag = r.flag();
        p.extension_3d_flag = r.flag();
        p.scc_extension_flag = r.flag();
        r.skip(4); // pps_extension_4bits
        if range {
            let e = &mut p.range_ext;
            if p.transform_skip_enabled_flag {
                e.log2_max_transform_skip_block_size_minus2 = r.ue();
            }
            e.cross_component_prediction_enabled_flag = r.flag();
            e.chroma_qp_offset_list_enabled_flag = r.flag();
            if e.chroma_qp_offset_list_enabled_flag {
                e.diff_cu_chroma_qp_offset_depth = r.ue();
                let n = ue_max(&mut r, 5, "chroma_qp_offset_list_len_minus1")? + 1;
                for _ in 0..n {
                    e.cb_qp_offset_list.push(r.se());
                    e.cr_qp_offset_list.push(r.se());
                }
            }
            e.log2_sao_offset_scale_luma = r.ue();
            e.log2_sao_offset_scale_chroma = r.ue();
        }
    }
    done(&r, p)
}

// ---------------------------------------------------------------- picture geometry (6.5.1, 6.5.2)

/// The scans of one (SPS, PPS) pair: the tile scan (6.5.1) and the z-scan order of the 4x4 blocks (6.5.2, computed
/// at 4x4 granularity, which orders any two blocks of different minimum transform blocks as MinTbAddrZs does).
#[derive(Clone, Debug, Default)]
pub struct Geometry {
    pub width_ctbs: u32,
    pub height_ctbs: u32,
    pub log2_ctb: u32,
    /// colBd / rowBd (in CTBs, num + 1 entries).
    pub col_bd: Vec<u32>,
    pub row_bd: Vec<u32>,
    pub rs_to_ts: Vec<u32>,
    pub ts_to_rs: Vec<u32>,
    /// TileId[ctbAddrTs].
    pub tile_id: Vec<u32>,
    /// Width of the 4x4 grid (whole CTBs).
    pub w4: usize,
    pub h4: usize,
    /// The z-scan address of each 4x4 block, [y4 * w4 + x4].
    pub zs: Vec<u32>,
}

impl Geometry {
    pub fn new(sps: &Sps, pps: &Pps) -> Result<Geometry> {
        let w = sps.width_in_ctbs();
        let h = sps.height_in_ctbs();
        let cols = pps.num_tile_columns;
        let rows = pps.num_tile_rows;
        if cols > w || rows > h {
            return Err(Error::Invalid("more tiles than CTBs"));
        }
        let split = |n: u32, total: u32, explicit: &[u32]| -> Result<Vec<u32>> {
            let mut v = Vec::with_capacity(n as usize);
            if pps.uniform_spacing_flag {
                for i in 0..n {
                    v.push(((i + 1) * total) / n - (i * total) / n);
                }
            } else {
                let sum: u32 = explicit.iter().sum();
                if sum >= total {
                    return Err(Error::Invalid("tile sizes exceed the picture"));
                }
                v.extend_from_slice(explicit);
                v.push(total - sum);
            }
            Ok(v)
        };
        let colw = split(cols, w, &pps.column_widths)?;
        let rowh = split(rows, h, &pps.row_heights)?;
        let mut col_bd = vec![0u32; cols as usize + 1];
        for i in 0..cols as usize {
            col_bd[i + 1] = col_bd[i] + colw[i];
        }
        let mut row_bd = vec![0u32; rows as usize + 1];
        for j in 0..rows as usize {
            row_bd[j + 1] = row_bd[j] + rowh[j];
        }
        let n = (w * h) as usize;
        let mut rs_to_ts = vec![0u32; n];
        for rs in 0..n as u32 {
            let (tbx, tby) = (rs % w, rs / w);
            let tx = (0..cols as usize).rev().find(|&i| tbx >= col_bd[i]).unwrap_or(0);
            let ty = (0..rows as usize).rev().find(|&j| tby >= row_bd[j]).unwrap_or(0);
            let mut v = 0;
            for i in 0..tx {
                v += rowh[ty] * colw[i];
            }
            for j in 0..ty {
                v += w * rowh[j];
            }
            v += (tby - row_bd[ty]) * colw[tx] + tbx - col_bd[tx];
            rs_to_ts[rs as usize] = v;
        }
        let mut ts_to_rs = vec![0u32; n];
        for rs in 0..n {
            ts_to_rs[rs_to_ts[rs] as usize] = rs as u32;
        }
        let mut tile_id = vec![0u32; n];
        let mut tid = 0;
        for j in 0..rows as usize {
            for i in 0..cols as usize {
                for y in row_bd[j]..row_bd[j + 1] {
                    for x in col_bd[i]..col_bd[i + 1] {
                        tile_id[rs_to_ts[(y * w + x) as usize] as usize] = tid;
                    }
                }
                tid += 1;
            }
        }
        let log2_ctb = sps.log2_ctb;
        let sh = (log2_ctb - 2) as usize;
        let w4 = (w as usize) << sh;
        let h4 = (h as usize) << sh;
        let mut zs = vec![0u32; w4 * h4];
        for y in 0..h4 {
            for x in 0..w4 {
                let ctb = (y >> sh) * w as usize + (x >> sh);
                let mut v = rs_to_ts[ctb] << (2 * sh);
                for i in 0..sh {
                    let m = 1usize << i;
                    if x & m != 0 {
                        v += (m * m) as u32;
                    }
                    if y & m != 0 {
                        v += (2 * m * m) as u32;
                    }
                }
                zs[y * w4 + x] = v;
            }
        }
        Ok(Geometry { width_ctbs: w, height_ctbs: h, log2_ctb, col_bd, row_bd, rs_to_ts, ts_to_rs, tile_id, w4, h4, zs })
    }

    /// The tile of the CTB at raster address `rs`.
    pub fn tile_of_rs(&self, rs: u32) -> u32 {
        self.tile_id[self.rs_to_ts[rs as usize] as usize]
    }

    /// The raster CTB address holding luma sample (x, y).
    pub fn ctb_rs(&self, x: u32, y: u32) -> u32 {
        (y >> self.log2_ctb) * self.width_ctbs + (x >> self.log2_ctb)
    }
}
