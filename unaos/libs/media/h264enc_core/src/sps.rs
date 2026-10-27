// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Parameter sets and slice headers, written: seq_parameter_set_rbsp (7.3.2.1.1) with
//! vui_parameters (E.1.1), pic_parameter_set_rbsp (7.3.2.2) and slice_header (7.3.3) — the exact
//! subset h264_core's `ps::parse_sps`, `ps::parse_pps` and `ps::parse_slice_header` read back.
//!
//! The stream is Constrained Baseline (A.2.1.1: profile_idc 66 with constraint_set0_flag and
//! constraint_set1_flag), or Main (A.2.2: profile_idc 77, constraint_set1_flag) under CABAC
//! (SR310): frames only, CAVLC / CABAC, one slice per picture, one reference frame, POC type
//! 2 (output order = decoding order) and a VUI bitstream_restriction of max_num_reorder_frames 0 /
//! max_dec_frame_buffering 1, so a decoder outputs each picture as its access unit ends.
//!
//! H264ENCBFRAMES (SR333): with B pictures ([`SeqParams::reorder`]) the stream is Main (B slices are not
//! Baseline; CAVLC keeps entropy_coding_mode_flag 0), POC type 0 (pic_order_cnt_lsb per slice), two
//! reference frames and a VUI of max_num_reorder_frames 1 / max_dec_frame_buffering 2; a B slice header
//! carries direct_spatial_mv_pred_flag 1, both list-modification flags 0 and, as a non-reference
//! picture, no dec_ref_pic_marking.
//!
//! H264ENCPYRAMID (SR349): under the pyramid ([`SeqParams::pyramid`], bframes >= 2) three reference frames, VUI
//! max_num_reorder_frames [`SeqParams::num_reorder`] (2 at bframes >= 3) and max_dec_frame_buffering 4; per slice a
//! [`SliceExtra`] carries the reference B's marking, the anchor's list-0 modification and MMCO 1, temporal direct and the
//! explicit pred_weight_table; the PPS names weighted_bipred_idc.
//!
//! H264ENCCOLOURHOST (SR377): with [`SeqParams::colour`] the VUI states video_signal_type (video_format 5, the range flag)
//! and its colour_description (primaries, transfer, matrix), the codes h264_core's `ps::vui` reads back; None = the flag 0.

use alloc::vec::Vec;

use crate::nal::BitWriter;
use crate::Pic;

/// profile_idc of Baseline (A.2.1).
pub const PROFILE_BASELINE: u8 = 66;
/// constraint_set0_flag | constraint_set1_flag: Constrained Baseline (A.2.1.1).
pub const CONSTRAINT_FLAGS: u8 = 0xC0;
/// profile_idc of Main (A.2.2): the CABAC stream (H264ENCCABAC, SR310).
pub const PROFILE_MAIN: u8 = 77;
/// constraint_set1_flag alone: the CABAC stream conforms to Main (A.2.2), not to Baseline.
pub const CONSTRAINT_MAIN: u8 = 0x40;
/// profile_idc of High (A.2.4): the 8x8 transform's stream (H264ENC8X8, SR322), constraint flags 0.
pub const PROFILE_HIGH: u8 = 100;

/// The sequence-level facts the writers need.
#[derive(Clone, Copy, Debug)]
pub struct SeqParams {
    pub width: u32,
    pub height: u32,
    pub mb_w: u32,
    pub mb_h: u32,
    pub log2_max_frame_num: u32,
    pub qp: i32,
    /// CABAC (entropy_coding_mode_flag 1): Main profile instead of Constrained Baseline.
    pub cabac: bool,
    /// The 8x8 transform (SR322): High profile, with the SPS's High fields.
    pub high: bool,
    /// B pictures (SR333): Main profile (unless High), POC type 0, two reference frames, VUI reorder 1.
    pub reorder: bool,
    /// log2_max_pic_order_cnt_lsb (written under `reorder` only).
    pub log2_max_poc_lsb: u32,
    /// The B pyramid (SR349): max_num_ref_frames 3 and max_dec_frame_buffering 4.
    pub pyramid: bool,
    /// VUI max_num_reorder_frames: 0 without B, 1, or 2 under a pyramid of bframes >= 3 (SR349).
    pub num_reorder: u32,
    /// H264ENCCOLOURHOST (SR377): VUI video_signal_type (primaries, transfer, matrix, full range); None writes the flag 0.
    pub colour: Option<(u8, u8, u8, bool)>,
}

/// level_idc for a frame of `mb_w` x `mb_h` macroblocks: the lowest level of Table A-1 whose
/// MaxFS holds the frame and whose sqrt(8 * MaxFS) bound holds each side (A.3.1 items b, f). The
/// bit rate is not a constraint here (QP is constant; rate control is owed).
pub fn level_idc(mb_w: u32, mb_h: u32) -> u8 {
    const LEVELS: [(u8, u32); 11] = [(10, 99), (20, 396), (21, 792), (30, 1620), (31, 3600), (32, 5120), (40, 8192), (42, 8704), (50, 22080), (51, 36864), (60, 139264)];
    let fs = mb_w * mb_h;
    for &(l, max_fs) in &LEVELS {
        if fs <= max_fs && mb_w * mb_w <= 8 * max_fs && mb_h * mb_h <= 8 * max_fs {
            return l;
        }
    }
    62
}

/// seq_parameter_set_rbsp (7.3.2.1.1), the RBSP after the NAL header byte.
pub fn write_sps(p: &SeqParams) -> Vec<u8> {
    let mut w = BitWriter::new();
    let (profile, constraint) = if p.high { (PROFILE_HIGH, 0) } else if p.cabac || p.reorder { (PROFILE_MAIN, CONSTRAINT_MAIN) } else { (PROFILE_BASELINE, CONSTRAINT_FLAGS) };
    w.u(8, profile as u32);
    w.u(8, constraint as u32); // constraint_set0..5 flags + reserved_zero_2bits
    w.u(8, level_idc(p.mb_w, p.mb_h) as u32);
    w.ue(0); // seq_parameter_set_id
    if p.high {
        w.ue(1); // chroma_format_idc: 4:2:0
        w.ue(0); // bit_depth_luma_minus8
        w.ue(0); // bit_depth_chroma_minus8
        w.flag(false); // qpprime_y_zero_transform_bypass_flag
        w.flag(false); // seq_scaling_matrix_present_flag: flat (scaling matrices owed)
    }
    w.ue(p.log2_max_frame_num - 4);
    if p.reorder {
        w.ue(0); // pic_order_cnt_type 0: pic_order_cnt_lsb in every slice header (SR333)
        w.ue(p.log2_max_poc_lsb - 4); // log2_max_pic_order_cnt_lsb_minus4
        w.ue(if p.pyramid { 3 } else { 2 }); // max_num_ref_frames: the past and the future anchor (+ the pyramid's middle B, SR349)
    } else {
        w.ue(2); // pic_order_cnt_type: POC follows frame_num, output order = decoding order
        w.ue(1); // max_num_ref_frames
    }
    w.flag(false); // gaps_in_frame_num_value_allowed_flag
    w.ue(p.mb_w - 1);
    w.ue(p.mb_h - 1); // pic_height_in_map_units_minus1
    w.flag(true); // frame_mbs_only_flag
    w.flag(true); // direct_8x8_inference_flag
    let (cr, cb) = ((p.mb_w * 16 - p.width) / 2, (p.mb_h * 16 - p.height) / 2);
    w.flag(cr != 0 || cb != 0); // frame_cropping_flag (CropUnitX = CropUnitY = 2)
    if cr != 0 || cb != 0 {
        w.ue(0);
        w.ue(cr);
        w.ue(0);
        w.ue(cb);
    }
    w.flag(true); // vui_parameters_present_flag
    // vui_parameters (E.1.1): the bitstream restriction, and the colour when stated (SR377).
    w.flag(false); // aspect_ratio_info_present_flag
    w.flag(false); // overscan_info_present_flag
    w.flag(p.colour.is_some()); // video_signal_type_present_flag (H264ENCCOLOURHOST SR377: the file's colour, else 0)
    if let Some((primaries, transfer, matrix, full)) = p.colour {
        w.u(3, 5); // video_format: unspecified
        w.flag(full); // video_full_range_flag
        w.flag(true); // colour_description_present_flag
        w.u(8, primaries as u32); // colour_primaries (Table E-3 = H.273)
        w.u(8, transfer as u32); // transfer_characteristics (Table E-4)
        w.u(8, matrix as u32); // matrix_coefficients (Table E-5)
    }
    w.flag(false); // chroma_loc_info_present_flag
    w.flag(false); // timing_info_present_flag
    w.flag(false); // nal_hrd_parameters_present_flag
    w.flag(false); // vcl_hrd_parameters_present_flag
    w.flag(false); // pic_struct_present_flag
    w.flag(true); // bitstream_restriction_flag
    w.flag(true); // motion_vectors_over_pic_boundaries_flag
    w.ue(2); // max_bytes_per_pic_denom
    w.ue(1); // max_bits_per_mb_denom
    w.ue(16); // log2_max_mv_length_horizontal
    w.ue(16); // log2_max_mv_length_vertical
    w.ue(p.num_reorder); // max_num_reorder_frames: 1 with B pictures (the anchor waits for them), 2 under a deep pyramid
    w.ue(if p.pyramid { 4 } else { 1 + p.reorder as u32 }); // max_dec_frame_buffering: 3 references + 1 waiting B under the pyramid
    w.trailing();
    w.finish()
}

/// pic_parameter_set_rbsp (7.3.2.2): CAVLC or CABAC (`cabac`, SR310), one slice group, one active reference by default,
/// pic_init_qp = the stream's QP (so slice_qp_delta is 0), chroma_qp_index_offset 0,
/// deblocking_filter_control_present_flag 1 (each slice names its filter idc). With `t8x8` (SR322) the
/// High tail: transform_8x8_mode_flag 1, pic_scaling_matrix_present_flag 0, second_chroma_qp_index_offset 0.
pub fn write_pps(qp: i32, cabac: bool, t8x8: bool, bipred_idc: u32) -> Vec<u8> {
    let mut w = BitWriter::new();
    w.ue(0); // pic_parameter_set_id
    w.ue(0); // seq_parameter_set_id
    w.flag(cabac); // entropy_coding_mode_flag: 0 CAVLC, 1 CABAC
    w.flag(false); // bottom_field_pic_order_in_frame_present_flag
    w.ue(0); // num_slice_groups_minus1
    w.ue(0); // num_ref_idx_l0_default_active_minus1
    w.ue(0); // num_ref_idx_l1_default_active_minus1
    w.flag(false); // weighted_pred_flag
    w.u(2, bipred_idc); // weighted_bipred_idc: 0 default, 1 explicit, 2 implicit (SR349)
    w.se(qp - 26); // pic_init_qp_minus26
    w.se(0); // pic_init_qs_minus26
    w.se(0); // chroma_qp_index_offset
    w.flag(true); // deblocking_filter_control_present_flag
    w.flag(false); // constrained_intra_pred_flag
    w.flag(false); // redundant_pic_cnt_present_flag
    if t8x8 {
        w.flag(true); // transform_8x8_mode_flag
        w.flag(false); // pic_scaling_matrix_present_flag
        w.se(0); // second_chroma_qp_index_offset
    }
    w.trailing();
    w.finish()
}

/// slice_header (7.3.3) of the one slice of a picture: first_mb_in_slice 0, slice_type 7 (I, all
/// slices of the picture) or 5 (P), the PPS, frame_num, idr_pic_id on an IDR; a P slice keeps the
/// default one active reference and no list modification; marking is the IDR's 0 / 0 or sliding
/// window; slice_qp_delta (SliceQPY - pic_init_qp: 0 at constant QP, the rate controller's picture QP
/// otherwise, SR218); disable_deblocking_filter_idc 0 with zero filter offsets when
/// `deblock` (the encoder filters its reconstruction as the decoder does, SR211), else 1 (the
/// reconstruction is unfiltered). Under CABAC a P slice carries cabac_init_idc 0 (SR310).
/// SR333: `pic` B writes slice_type 6, direct_spatial_mv_pred_flag 1, ref_pic_list_modification_flag_l1 0
/// and (nal_ref_idc 0) no marking; `poc` Some((lsb, log2_max_pic_order_cnt_lsb)) writes pic_order_cnt_lsb
/// (POC type 0), None keeps POC type 2's empty syntax.
pub fn write_slice_header(w: &mut BitWriter, pic: Pic, frame_num: u32, log2_max_frame_num: u32, idr_pic_id: u32, poc: Option<(u32, u32)>, deblock: bool, slice_qp_delta: i32, cabac: bool, x: &SliceExtra) {
    let idr = pic == Pic::Idr;
    w.ue(0); // first_mb_in_slice
    w.ue(match pic {
        Pic::Idr => 7,
        Pic::P => 5,
        Pic::B => 6,
    }); // slice_type (all slices of the picture)
    w.ue(0); // pic_parameter_set_id
    w.u(log2_max_frame_num, frame_num);
    if idr {
        w.ue(idr_pic_id);
    }
    if let Some((lsb, log2)) = poc {
        w.u(log2, lsb); // pic_order_cnt_lsb (POC type 0, SR333)
    }
    // pic_order_cnt_type 2: no POC syntax.
    if pic == Pic::B {
        w.flag(x.direct_spatial); // direct_spatial_mv_pred_flag (0 = temporal, SR349)
    }
    if !idr {
        w.flag(false); // num_ref_idx_active_override_flag
        w.flag(x.l0_reorder.is_some()); // ref_pic_list_modification_flag_l0
        if let Some(d) = x.l0_reorder {
            w.ue(0); // modification_of_pic_nums_idc 0: subtract
            w.ue(d); // abs_diff_pic_num_minus1
            w.ue(3); // end of the list's modifications
        }
    }
    if pic == Pic::B {
        w.flag(false); // ref_pic_list_modification_flag_l1
    }
    if let Some(o) = x.weights {
        pred_weight_table(w, o);
    }
    // nal_ref_idc != 0: dec_ref_pic_marking (7.3.3.3); a B picture is a reference only as the pyramid's middle (SR349).
    if idr {
        w.flag(false); // no_output_of_prior_pics_flag
        w.flag(false); // long_term_reference_flag
    } else if pic == Pic::P || x.reference {
        w.flag(x.forget.is_some()); // adaptive_ref_pic_marking_mode_flag: sliding window, or MMCO 1 (SR349)
        if let Some(d) = x.forget {
            w.ue(1); // memory_management_control_operation 1: a short-term picture unused
            w.ue(d); // difference_of_pic_nums_minus1
            w.ue(0); // end of the operations
        }
    }
    if cabac && !idr {
        w.ue(0); // cabac_init_idc (P / B slices; the I slices are the IDRs)
    }
    w.se(slice_qp_delta); // slice_qp_delta
    if deblock {
        w.ue(0); // disable_deblocking_filter_idc
        w.se(0); // slice_alpha_c0_offset_div2
        w.se(0); // slice_beta_offset_div2
    } else {
        w.ue(1); // disable_deblocking_filter_idc
    }
}

/// The per-slice syntax beyond SR333's (H264ENCPYRAMID, SR349); [`Default`] writes SR333's bytes for an anchor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SliceExtra {
    /// A B picture with nal_ref_idc != 0 (the pyramid's middle): it carries dec_ref_pic_marking.
    pub reference: bool,
    /// direct_spatial_mv_pred_flag of a B slice (false = temporal direct).
    pub direct_spatial: bool,
    /// The list-0 modification of a P slice: abs_diff_pic_num_minus1 (idc 0) so RefPicList0[0] is the past anchor.
    pub l0_reorder: Option<u32>,
    /// MMCO 1 difference_of_pic_nums_minus1: the previous middle B marked unused.
    pub forget: Option<u32>,
    /// The explicit pred_weight_table of a B slice (weighted_bipred_idc 1): the luma offsets of L0[0] and L1[0].
    pub weights: Option<[i32; 2]>,
}

impl Default for SliceExtra {
    fn default() -> SliceExtra {
        SliceExtra { reference: false, direct_spatial: true, l0_reorder: None, forget: None, weights: None }
    }
}

/// The luma_log2_weight_denom (and chroma's) of the explicit table: weight 32 is 1.0.
pub const WP_LOG2_DENOM: u32 = 5;

/// pred_weight_table (7.3.3.2) of a B slice with one active reference per list: denominators [`WP_LOG2_DENOM`], per list
/// a luma weight of 32 with the offset when it is not 0 (luma_weight_lX_flag), chroma at its defaults.
fn pred_weight_table(w: &mut BitWriter, offsets: [i32; 2]) {
    w.ue(WP_LOG2_DENOM); // luma_log2_weight_denom
    w.ue(WP_LOG2_DENOM); // chroma_log2_weight_denom
    for o in offsets {
        w.flag(o != 0); // luma_weight_lX_flag
        if o != 0 {
            w.se(1 << WP_LOG2_DENOM); // luma_weight_lX
            w.se(o); // luma_offset_lX
        }
        w.flag(false); // chroma_weight_lX_flag
    }
}
