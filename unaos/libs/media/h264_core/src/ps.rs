// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Parameter sets and the slice header: seq_parameter_set_rbsp (7.3.2.1.1) with scaling lists
//! (7.3.2.1.1.1) and the VUI fields the DPB needs (Annex E.1.1, E.1.2), pic_parameter_set_rbsp
//! (7.3.2.2), and slice_header (7.3.3) with ref_pic_list_modification (7.3.3.1),
//! pred_weight_table (7.3.3.2) and dec_ref_pic_marking (7.3.3.3).
//!
//! Streams outside the decoded set are refused here, by name, before any slice data is touched.

use alloc::vec::Vec;

use crate::bits::BitReader;
use crate::tables::{DEFAULT_4X4_INTER, DEFAULT_4X4_INTRA, DEFAULT_8X8_INTER, DEFAULT_8X8_INTRA};
use crate::{Error, Result};

/// A scaling list as transmitted (7.3.2.1.1.1): absent, "use the default", or explicit (scan order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ListSyntax {
    Absent,
    Default,
    List4([u8; 16]),
    List8([u8; 64]),
}

/// The scaling matrices in effect (scan order): 4x4 lists Y/Cb/Cr intra then Y/Cb/Cr inter
/// (i = 0..5), 8x8 lists Y intra, Y inter (i = 6, 7).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScalingMatrices {
    pub l4: [[u8; 16]; 6],
    pub l8: [[u8; 64]; 2],
}

impl ScalingMatrices {
    /// Flat_4x4_16 / Flat_8x8_16.
    pub const FLAT: ScalingMatrices = ScalingMatrices { l4: [[16; 16]; 6], l8: [[16; 64]; 2] };
}

fn scaling_list(r: &mut BitReader, size: usize) -> ListSyntax {
    let mut list = [0u8; 64];
    let (mut last, mut next) = (8i32, 8i32);
    for j in 0..size {
        if next != 0 {
            let delta = r.se();
            next = (last + delta + 256).rem_euclid(256);
            if j == 0 && next == 0 {
                return ListSyntax::Default;
            }
        }
        list[j] = if next == 0 { last as u8 } else { next as u8 };
        last = list[j] as i32;
    }
    if size == 16 {
        let mut l = [0u8; 16];
        l.copy_from_slice(&list[..16]);
        ListSyntax::List4(l)
    } else {
        ListSyntax::List8(list)
    }
}

fn default4(i: usize) -> [u8; 16] {
    if i < 3 { DEFAULT_4X4_INTRA } else { DEFAULT_4X4_INTER }
}

fn default8(i: usize) -> [u8; 64] {
    if i == 0 { DEFAULT_8X8_INTRA } else { DEFAULT_8X8_INTER }
}

/// Resolve transmitted lists with Table 7-2's fall-back rules. `fallback` is `None` for rule A
/// (defaults) and the sequence-level matrices for rule B.
fn resolve(l4: &[ListSyntax; 6], l8: &[ListSyntax; 2], fallback: Option<&ScalingMatrices>) -> ScalingMatrices {
    let mut m = ScalingMatrices::FLAT;
    for i in 0..6 {
        m.l4[i] = match l4[i] {
            ListSyntax::List4(l) => l,
            ListSyntax::Default => default4(i),
            _ => match (i, fallback) {
                (0 | 3, None) => default4(i),
                (0 | 3, Some(f)) => f.l4[i],
                _ => m.l4[i - 1],
            },
        };
    }
    for i in 0..2 {
        m.l8[i] = match l8[i] {
            ListSyntax::List8(l) => l,
            ListSyntax::Default => default8(i),
            _ => match fallback {
                None => default8(i),
                Some(f) => f.l8[i],
            },
        };
    }
    m
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sps {
    pub profile_idc: u8,
    pub constraint_set3: bool,
    pub level_idc: u8,
    pub id: u32,
    pub chroma_format_idc: u32,
    pub bit_depth_luma: u32,
    pub bit_depth_chroma: u32,
    pub transform_bypass: bool,
    pub seq_scaling_present: bool,
    pub scaling: ScalingMatrices,
    pub log2_max_frame_num: u32,
    pub poc_type: u32,
    pub log2_max_poc_lsb: u32,
    pub delta_pic_order_always_zero: bool,
    pub offset_for_non_ref_pic: i32,
    pub offset_for_top_to_bottom_field: i32,
    pub offset_for_ref_frame: Vec<i32>,
    pub max_num_ref_frames: u32,
    pub gaps_allowed: bool,
    pub width_mbs: u32,
    pub height_map_units: u32,
    pub frame_mbs_only: bool,
    pub mbaff: bool,
    pub direct_8x8_inference: bool,
    /// frame_crop_{left,right,top,bottom}_offset.
    pub crop: [u32; 4],
    /// VUI bitstream_restriction: max_num_reorder_frames, max_dec_frame_buffering.
    pub max_num_reorder: Option<u32>,
    pub max_dec_frame_buffering: Option<u32>,
    /// VUI video_full_range_flag and matrix_coefficients (Table E-5), when signalled.
    pub full_range: bool,
    pub matrix_coefficients: u8,
}

impl Sps {
    pub fn height_mbs(&self) -> u32 {
        (2 - self.frame_mbs_only as u32) * self.height_map_units
    }

    /// The cropped frame rectangle (7.4.2.1.1, CropUnitX = 2, CropUnitY = 2 * (2 - frame_mbs_only)
    /// for 4:2:0): (x, y, width, height) in luma samples.
    pub fn crop_rect(&self) -> (u32, u32, u32, u32) {
        let (w, h) = (self.width_mbs * 16, self.height_mbs() * 16);
        let cux = 2;
        let cuy = 2 * (2 - self.frame_mbs_only as u32);
        let x0 = (cux * self.crop[0]).min(w.saturating_sub(2));
        let x1 = w.saturating_sub(cux * self.crop[1]).max(x0 + 2);
        let y0 = (cuy * self.crop[2]).min(h.saturating_sub(2));
        let y1 = h.saturating_sub(cuy * self.crop[3]).max(y0 + 2);
        (x0, y0, x1 - x0, y1 - y0)
    }

    /// MaxDpbFrames (A.3.1 item h, Table A-1 MaxDpbMbs), raised to max_num_ref_frames and to
    /// max_dec_frame_buffering when the VUI states it.
    pub fn dpb_frames(&self) -> usize {
        let level_1b = self.level_idc == 9 || (self.level_idc == 11 && self.constraint_set3 && !matches!(self.profile_idc, 100 | 110 | 122 | 244 | 44));
        let max_dpb_mbs: u32 = if level_1b {
            396
        } else {
            match self.level_idc {
                0..=10 => 396,
                11 => 900,
                12 | 13 | 20 => 2376,
                21 => 4752,
                22 | 30 => 8100,
                31 => 18000,
                32 => 20480,
                40 | 41 => 32768,
                42 => 34816,
                50 => 110400,
                51 | 52 => 184320,
                _ => 696320,
            }
        };
        let frame_mbs = (self.width_mbs * self.height_mbs()).max(1);
        let mut n = (max_dpb_mbs / frame_mbs).clamp(1, 16) as usize;
        if let Some(b) = self.max_dec_frame_buffering {
            n = (b as usize).clamp(1, 16);
        }
        n.max(self.max_num_ref_frames as usize).clamp(1, 16)
    }
}

const HIGH_PROFILES: [u8; 13] = [100, 110, 122, 244, 44, 83, 86, 118, 128, 138, 139, 134, 135];

fn hrd(r: &mut BitReader) {
    let cpb_cnt = r.ue() + 1;
    r.u(4);
    r.u(4);
    for _ in 0..cpb_cnt.min(32) {
        r.ue();
        r.ue();
        r.bit();
    }
    r.u(5);
    r.u(5);
    r.u(5);
    r.u(5);
}

/// seq_parameter_set_rbsp (7.3.2.1.1), the RBSP after the NAL header byte.
pub fn parse_sps(rbsp: &[u8]) -> Result<Sps> {
    let mut r = BitReader::new(rbsp);
    let profile_idc = r.u(8) as u8;
    let flags = r.u(8);
    let level_idc = r.u(8) as u8;
    let id = r.ue();
    if id > 31 {
        return Err(Error::Invalid("seq_parameter_set_id > 31"));
    }
    let mut s = Sps {
        profile_idc,
        constraint_set3: flags & 0x10 != 0,
        level_idc,
        id,
        chroma_format_idc: 1,
        bit_depth_luma: 8,
        bit_depth_chroma: 8,
        transform_bypass: false,
        seq_scaling_present: false,
        scaling: ScalingMatrices::FLAT,
        log2_max_frame_num: 4,
        poc_type: 0,
        log2_max_poc_lsb: 4,
        delta_pic_order_always_zero: false,
        offset_for_non_ref_pic: 0,
        offset_for_top_to_bottom_field: 0,
        offset_for_ref_frame: Vec::new(),
        max_num_ref_frames: 0,
        gaps_allowed: false,
        width_mbs: 0,
        height_map_units: 0,
        frame_mbs_only: true,
        mbaff: false,
        direct_8x8_inference: false,
        crop: [0; 4],
        max_num_reorder: None,
        max_dec_frame_buffering: None,
        full_range: false,
        matrix_coefficients: 2,
    };
    if HIGH_PROFILES.contains(&profile_idc) {
        s.chroma_format_idc = r.ue();
        if s.chroma_format_idc > 3 {
            return Err(Error::Invalid("chroma_format_idc > 3"));
        }
        if s.chroma_format_idc == 3 {
            r.bit(); // separate_colour_plane_flag
        }
        s.bit_depth_luma = r.ue() + 8;
        s.bit_depth_chroma = r.ue() + 8;
        s.transform_bypass = r.flag();
        s.seq_scaling_present = r.flag();
        if s.seq_scaling_present {
            let mut l4 = [ListSyntax::Absent; 6];
            let mut l8 = [ListSyntax::Absent; 2];
            let n = if s.chroma_format_idc != 3 { 8 } else { 12 };
            for i in 0..n {
                if r.flag() {
                    let l = scaling_list(&mut r, if i < 6 { 16 } else { 64 });
                    if i < 6 {
                        l4[i] = l;
                    } else if i < 8 {
                        l8[i - 6] = l;
                    }
                }
            }
            s.scaling = resolve(&l4, &l8, None);
        }
    }
    s.log2_max_frame_num = r.ue() + 4;
    if s.log2_max_frame_num > 16 {
        return Err(Error::Invalid("log2_max_frame_num_minus4 > 12"));
    }
    s.poc_type = r.ue();
    match s.poc_type {
        0 => {
            s.log2_max_poc_lsb = r.ue() + 4;
            if s.log2_max_poc_lsb > 16 {
                return Err(Error::Invalid("log2_max_pic_order_cnt_lsb_minus4 > 12"));
            }
        }
        1 => {
            s.delta_pic_order_always_zero = r.flag();
            s.offset_for_non_ref_pic = r.se();
            s.offset_for_top_to_bottom_field = r.se();
            let n = r.ue();
            if n > 255 {
                return Err(Error::Invalid("num_ref_frames_in_pic_order_cnt_cycle > 255"));
            }
            for _ in 0..n {
                s.offset_for_ref_frame.push(r.se());
            }
        }
        2 => {}
        _ => return Err(Error::Invalid("pic_order_cnt_type > 2")),
    }
    s.max_num_ref_frames = r.ue();
    s.gaps_allowed = r.flag();
    s.width_mbs = r.ue() + 1;
    s.height_map_units = r.ue() + 1;
    s.frame_mbs_only = r.flag();
    if !s.frame_mbs_only {
        s.mbaff = r.flag();
    }
    s.direct_8x8_inference = r.flag();
    if r.flag() {
        for c in s.crop.iter_mut() {
            *c = r.ue();
        }
    }
    if r.overran() {
        return Err(Error::Truncated);
    }
    if s.width_mbs > 1024 || s.height_mbs() > 1024 {
        return Err(Error::Invalid("picture larger than 16384 samples on a side"));
    }
    if s.max_num_ref_frames > 16 {
        return Err(Error::Invalid("max_num_ref_frames > 16"));
    }
    if r.flag() {
        // vui_parameters (E.1.1). A damaged VUI loses only the reorder hints.
        let save = s.clone();
        vui(&mut r, &mut s);
        if r.overran() {
            s = save;
        }
    }
    Ok(s)
}

fn vui(r: &mut BitReader, s: &mut Sps) {
    if r.flag() {
        if r.u(8) == 255 {
            r.u(16);
            r.u(16);
        }
    }
    if r.flag() {
        r.bit();
    }
    if r.flag() {
        r.u(3);
        s.full_range = r.flag();
        if r.flag() {
            r.u(8);
            r.u(8);
            s.matrix_coefficients = r.u(8) as u8;
        }
    }
    if r.flag() {
        r.ue();
        r.ue();
    }
    if r.flag() {
        r.u(32);
        r.u(32);
        r.bit();
    }
    let nal_hrd = r.flag();
    if nal_hrd {
        hrd(r);
    }
    let vcl_hrd = r.flag();
    if vcl_hrd {
        hrd(r);
    }
    if nal_hrd || vcl_hrd {
        r.bit();
    }
    r.bit(); // pic_struct_present_flag
    if r.flag() {
        r.bit();
        r.ue();
        r.ue();
        r.ue();
        r.ue();
        s.max_num_reorder = Some(r.ue());
        s.max_dec_frame_buffering = Some(r.ue());
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pps {
    pub id: u32,
    pub sps_id: u32,
    pub cabac: bool,
    pub bottom_field_pic_order_in_frame_present: bool,
    pub num_slice_groups: u32,
    pub num_ref_idx_default: [u32; 2],
    pub weighted_pred: bool,
    pub weighted_bipred_idc: u32,
    pub pic_init_qp: i32,
    pub chroma_qp_offset: [i32; 2],
    pub deblocking_filter_control_present: bool,
    pub constrained_intra_pred: bool,
    pub redundant_pic_cnt_present: bool,
    pub transform_8x8_mode: bool,
    pub pic_scaling_present: bool,
    pub l4: [ListSyntax; 6],
    pub l8: [ListSyntax; 2],
}

impl Pps {
    /// The scaling matrices of a picture using this PPS with `sps` (Table 7-2).
    pub fn scaling(&self, sps: &Sps) -> ScalingMatrices {
        if !self.pic_scaling_present {
            return sps.scaling;
        }
        resolve(&self.l4, &self.l8, if sps.seq_scaling_present { Some(&sps.scaling) } else { None })
    }
}

/// pic_parameter_set_rbsp (7.3.2.2). `chroma_format_idc` of the referenced SPS sizes the list of
/// 8x8 scaling lists (1 when the SPS has not been seen).
pub fn parse_pps(rbsp: &[u8], chroma_format_idc: impl Fn(u32) -> u32) -> Result<Pps> {
    let mut r = BitReader::new(rbsp);
    let id = r.ue();
    let sps_id = r.ue();
    if id > 255 || sps_id > 31 {
        return Err(Error::Invalid("parameter set id out of range"));
    }
    let cabac = r.flag();
    let bottom = r.flag();
    let num_slice_groups = r.ue() + 1;
    if num_slice_groups > 8 {
        return Err(Error::Invalid("num_slice_groups_minus1 > 7"));
    }
    if num_slice_groups > 1 {
        // Parsed only to reach the fields after it; a slice using this PPS is refused (FMO).
        let map_type = r.ue();
        match map_type {
            0 => {
                for _ in 0..num_slice_groups {
                    r.ue();
                }
            }
            2 => {
                for _ in 0..num_slice_groups - 1 {
                    r.ue();
                    r.ue();
                }
            }
            3..=5 => {
                r.bit();
                r.ue();
            }
            6 => {
                let n = r.ue() + 1;
                let bits = 32 - (num_slice_groups - 1).leading_zeros();
                for _ in 0..n.min(1 << 20) {
                    r.u(bits);
                }
            }
            _ => {}
        }
    }
    let l0 = r.ue() + 1;
    let l1 = r.ue() + 1;
    if l0 > 32 || l1 > 32 {
        return Err(Error::Invalid("num_ref_idx_default_active_minus1 > 31"));
    }
    let weighted_pred = r.flag();
    let weighted_bipred_idc = r.u(2);
    let pic_init_qp = 26 + r.se();
    r.se(); // pic_init_qs_minus26
    let cqo = r.se();
    let mut p = Pps {
        id,
        sps_id,
        cabac,
        bottom_field_pic_order_in_frame_present: bottom,
        num_slice_groups,
        num_ref_idx_default: [l0, l1],
        weighted_pred,
        weighted_bipred_idc,
        pic_init_qp,
        chroma_qp_offset: [cqo, cqo],
        deblocking_filter_control_present: r.flag(),
        constrained_intra_pred: r.flag(),
        redundant_pic_cnt_present: r.flag(),
        transform_8x8_mode: false,
        pic_scaling_present: false,
        l4: [ListSyntax::Absent; 6],
        l8: [ListSyntax::Absent; 2],
    };
    if r.more_rbsp_data() {
        p.transform_8x8_mode = r.flag();
        p.pic_scaling_present = r.flag();
        if p.pic_scaling_present {
            let n8 = if chroma_format_idc(sps_id) != 3 { 2 } else { 6 };
            for i in 0..6 + if p.transform_8x8_mode { n8 } else { 0 } {
                if r.flag() {
                    let l = scaling_list(&mut r, if i < 6 { 16 } else { 64 });
                    if i < 6 {
                        p.l4[i] = l;
                    } else if i < 8 {
                        p.l8[i - 6] = l;
                    }
                }
            }
        }
        p.chroma_qp_offset[1] = r.se();
    }
    if r.overran() {
        return Err(Error::Truncated);
    }
    if !(-12..=12).contains(&p.chroma_qp_offset[0]) || !(-12..=12).contains(&p.chroma_qp_offset[1]) {
        return Err(Error::Invalid("chroma_qp_index_offset outside -12..12"));
    }
    if !(0..=51).contains(&p.pic_init_qp) {
        return Err(Error::Invalid("pic_init_qp_minus26 out of range"));
    }
    Ok(p)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SliceType {
    P,
    B,
    I,
}

/// One memory_management_control_operation (7.3.3.3) with its operands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mmco {
    /// 1: difference_of_pic_nums_minus1
    ForgetShort(u32),
    /// 2: long_term_pic_num
    ForgetLong(u32),
    /// 3: difference_of_pic_nums_minus1, long_term_frame_idx
    ShortToLong(u32, u32),
    /// 4: max_long_term_frame_idx_plus1
    MaxLongIdx(u32),
    /// 5
    Reset,
    /// 6: long_term_frame_idx
    CurrentToLong(u32),
}

/// pred_weight_table (7.3.3.2) with the inferred defaults filled in: weight 2^denom, offset 0.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PredWeights {
    pub luma_log2: u32,
    pub chroma_log2: u32,
    /// [list][refIdx] = (weight, offset)
    pub luma: [[(i32, i32); 32]; 2],
    /// [list][refIdx][Cb/Cr] = (weight, offset)
    pub chroma: [[[(i32, i32); 2]; 32]; 2],
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SliceHeader {
    pub nal_type: u8,
    pub nal_ref_idc: u8,
    pub idr: bool,
    pub first_mb: u32,
    pub slice_type: SliceType,
    pub pps_id: u32,
    pub frame_num: u32,
    pub idr_pic_id: u32,
    pub poc_lsb: u32,
    pub delta_poc_bottom: i32,
    pub delta_poc: [i32; 2],
    pub redundant_pic_cnt: u32,
    pub direct_spatial: bool,
    pub num_ref_idx_active: [u32; 2],
    /// ref_pic_list_modification: (modification_of_pic_nums_idc, value) per list.
    pub rplm: [Vec<(u32, u32)>; 2],
    pub weights: Option<PredWeights>,
    pub no_output_of_prior_pics: bool,
    pub long_term_reference: bool,
    pub adaptive_marking: bool,
    pub mmcos: Vec<Mmco>,
    pub cabac_init_idc: u32,
    pub qp: i32,
    pub disable_deblocking: u32,
    pub alpha_offset: i32,
    pub beta_offset: i32,
    /// Bit position of slice_data() in the RBSP.
    pub data_bit: usize,
}

impl SliceHeader {
    pub fn has_mmco5(&self) -> bool {
        self.mmcos.iter().any(|m| *m == Mmco::Reset)
    }
}

fn parse_weights(r: &mut BitReader, n: [u32; 2], lists: usize) -> PredWeights {
    let luma_log2 = r.ue().min(7);
    let chroma_log2 = r.ue().min(7);
    let mut w = PredWeights { luma_log2, chroma_log2, luma: [[(1 << luma_log2, 0); 32]; 2], chroma: [[[(1 << chroma_log2, 0); 2]; 32]; 2] };
    for l in 0..lists {
        for i in 0..n[l] as usize {
            if r.flag() {
                w.luma[l][i] = (r.se(), r.se());
            }
            if r.flag() {
                for c in 0..2 {
                    w.chroma[l][i][c] = (r.se(), r.se());
                }
            }
        }
    }
    w
}

/// slice_header (7.3.3). `sps_of` / `pps_of` look the parameter sets up by id.
pub fn parse_slice_header<'a>(
    r: &mut BitReader,
    nal_type: u8,
    nal_ref_idc: u8,
    pps_of: impl Fn(u32) -> Option<&'a Pps>,
    sps_of: impl Fn(u32) -> Option<&'a Sps>,
) -> Result<SliceHeader> {
    let idr = nal_type == 5;
    let first_mb = r.ue();
    let st = r.ue();
    let slice_type = match st % 5 {
        0 => SliceType::P,
        1 => SliceType::B,
        2 => SliceType::I,
        3 | 4 => return Err(Error::Unsupported("SP/SI slices (Extended profile)")),
        _ => return Err(Error::Invalid("slice_type > 9")),
    };
    let pps_id = r.ue();
    let pps = pps_of(pps_id).ok_or(Error::Invalid("slice refers to a missing PPS"))?;
    let sps = sps_of(pps.sps_id).ok_or(Error::Invalid("PPS refers to a missing SPS"))?;
    check_sps(sps)?;
    if pps.num_slice_groups > 1 {
        return Err(Error::Unsupported("FMO (slice groups)"));
    }
    if idr && slice_type != SliceType::I {
        return Err(Error::Invalid("IDR picture with a P or B slice"));
    }
    let frame_num = r.u(sps.log2_max_frame_num);
    if !sps.frame_mbs_only {
        if r.flag() {
            return Err(Error::Unsupported("interlace (field pictures)"));
        }
        if sps.mbaff {
            return Err(Error::Unsupported("interlace (MBAFF)"));
        }
    }
    let mut h = SliceHeader {
        nal_type,
        nal_ref_idc,
        idr,
        first_mb,
        slice_type,
        pps_id,
        frame_num,
        idr_pic_id: 0,
        poc_lsb: 0,
        delta_poc_bottom: 0,
        delta_poc: [0; 2],
        redundant_pic_cnt: 0,
        direct_spatial: false,
        num_ref_idx_active: pps.num_ref_idx_default,
        rplm: [Vec::new(), Vec::new()],
        weights: None,
        no_output_of_prior_pics: false,
        long_term_reference: false,
        adaptive_marking: false,
        mmcos: Vec::new(),
        cabac_init_idc: 0,
        qp: 0,
        disable_deblocking: 0,
        alpha_offset: 0,
        beta_offset: 0,
        data_bit: 0,
    };
    if idr {
        h.idr_pic_id = r.ue();
    }
    if sps.poc_type == 0 {
        h.poc_lsb = r.u(sps.log2_max_poc_lsb);
        if pps.bottom_field_pic_order_in_frame_present {
            h.delta_poc_bottom = r.se();
        }
    }
    if sps.poc_type == 1 && !sps.delta_pic_order_always_zero {
        h.delta_poc[0] = r.se();
        if pps.bottom_field_pic_order_in_frame_present {
            h.delta_poc[1] = r.se();
        }
    }
    if pps.redundant_pic_cnt_present {
        h.redundant_pic_cnt = r.ue();
    }
    if slice_type == SliceType::B {
        h.direct_spatial = r.flag();
    }
    if slice_type != SliceType::I {
        if r.flag() {
            h.num_ref_idx_active[0] = r.ue() + 1;
            if slice_type == SliceType::B {
                h.num_ref_idx_active[1] = r.ue() + 1;
            }
        }
        // Frame coding: at most 16 references per list (7.4.3); 32 is the field limit.
        if h.num_ref_idx_active[0] > 32 || h.num_ref_idx_active[1] > 32 {
            return Err(Error::Invalid("num_ref_idx_active_minus1 > 31"));
        }
    }
    if slice_type != SliceType::B {
        h.num_ref_idx_active[1] = 0;
    }
    if slice_type == SliceType::I {
        h.num_ref_idx_active[0] = 0;
    }
    // ref_pic_list_modification (7.3.3.1)
    for l in 0..2 {
        let active = (l == 0 && slice_type != SliceType::I) || (l == 1 && slice_type == SliceType::B);
        if active && r.flag() {
            loop {
                let idc = r.ue();
                if idc == 3 {
                    break;
                }
                if idc > 5 || r.overran() || h.rplm[l].len() > 66 {
                    return Err(Error::Invalid("ref_pic_list_modification"));
                }
                let v = r.ue();
                h.rplm[l].push((idc, v));
            }
        }
    }
    if (pps.weighted_pred && slice_type == SliceType::P) || (pps.weighted_bipred_idc == 1 && slice_type == SliceType::B) {
        let lists = if slice_type == SliceType::B { 2 } else { 1 };
        h.weights = Some(parse_weights(r, h.num_ref_idx_active, lists));
    }
    if nal_ref_idc != 0 {
        // dec_ref_pic_marking (7.3.3.3)
        if idr {
            h.no_output_of_prior_pics = r.flag();
            h.long_term_reference = r.flag();
        } else {
            h.adaptive_marking = r.flag();
            if h.adaptive_marking {
                loop {
                    let op = r.ue();
                    let m = match op {
                        0 => break,
                        1 => Mmco::ForgetShort(r.ue()),
                        2 => Mmco::ForgetLong(r.ue()),
                        3 => {
                            let d = r.ue();
                            Mmco::ShortToLong(d, r.ue())
                        }
                        4 => Mmco::MaxLongIdx(r.ue()),
                        5 => Mmco::Reset,
                        6 => Mmco::CurrentToLong(r.ue()),
                        _ => return Err(Error::Invalid("memory_management_control_operation > 6")),
                    };
                    if r.overran() || h.mmcos.len() > 66 {
                        return Err(Error::Invalid("dec_ref_pic_marking"));
                    }
                    h.mmcos.push(m);
                }
            }
        }
    }
    if pps.cabac && slice_type != SliceType::I {
        h.cabac_init_idc = r.ue();
        if h.cabac_init_idc > 2 {
            return Err(Error::Invalid("cabac_init_idc > 2"));
        }
    }
    h.qp = pps.pic_init_qp + r.se();
    if !(0..=51).contains(&h.qp) {
        return Err(Error::Invalid("SliceQPY outside 0..51"));
    }
    if pps.deblocking_filter_control_present {
        h.disable_deblocking = r.ue();
        if h.disable_deblocking > 2 {
            return Err(Error::Invalid("disable_deblocking_filter_idc > 2"));
        }
        if h.disable_deblocking != 1 {
            h.alpha_offset = r.se() * 2;
            h.beta_offset = r.se() * 2;
            if !(-12..=12).contains(&h.alpha_offset) || !(-12..=12).contains(&h.beta_offset) {
                return Err(Error::Invalid("slice_alpha_c0 / beta offset outside -6..6"));
            }
        }
    }
    if r.overran() {
        return Err(Error::Truncated);
    }
    let mbs = sps.width_mbs * sps.height_mbs();
    if h.first_mb >= mbs {
        return Err(Error::Invalid("first_mb_in_slice beyond the picture"));
    }
    h.data_bit = r.pos();
    Ok(h)
}

/// The refusals an SPS can trigger, by name.
pub fn check_sps(sps: &Sps) -> Result<()> {
    match sps.chroma_format_idc {
        1 => {}
        0 => return Err(Error::Unsupported("4:0:0 monochrome")),
        2 => return Err(Error::Unsupported("4:2:2 chroma")),
        _ => return Err(Error::Unsupported("4:4:4 chroma")),
    }
    if sps.bit_depth_luma != 8 || sps.bit_depth_chroma != 8 {
        return Err(Error::Unsupported("bit depth above 8"));
    }
    if sps.transform_bypass {
        return Err(Error::Unsupported("qpprime_y_zero_transform_bypass (lossless)"));
    }
    Ok(())
}
