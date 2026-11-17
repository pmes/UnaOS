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
    /// H264ENCHRD (SR699): the rate controller's bucket as the VUI's timing_info + nal_hrd_parameters; None writes both flags 0.
    pub hrd: Option<Hrd>,
    /// H264ENCHRDVCL (SR1060): beside `hrd`, the same table as vcl_hrd_parameters with cbr_flag 0; unread when `hrd` is None.
    pub hrd_vcl: bool,
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
    w.u(8, hrd_level(level_idc(p.mb_w, p.mb_h), p.hrd, p.high) as u32);
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
    w.flag(p.hrd.is_some()); if let Some(h) = p.hrd { timing_info(&mut w, &h); } // timing_info_present_flag (H264ENCHRD SR699: the HRD's clock)
    w.flag(p.hrd.is_some()); if let Some(h) = p.hrd { hrd_parameters(&mut w, &h); } // nal_hrd_parameters_present_flag (H264ENCHRD SR699)
    let vcl = p.hrd.filter(|_| p.hrd_vcl).map(|h| Hrd { cbr: false, ..h }); w.flag(vcl.is_some()); if let Some(h) = vcl { hrd_parameters(&mut w, &h); } if p.hrd.is_some() { w.flag(false); } // vcl_hrd_parameters_present_flag (+ the NAL table with cbr_flag 0 under Config::hrd_vcl, H264ENCHRDVCL SR1060); low_delay_hrd_flag 0, written once (SR699)
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

// ---- H264ENCHRD (LEDGER SR699): HRD parameters, buffering-period / pic-timing SEI and filler writers (named tail block) ----

/// The hypothetical reference decoder the stream signals (E.1.2, one CPB): the rate controller's bucket (rc.rs `Rc::hrd_params`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Hrd {
    /// BitRate in bits/s (a multiple of 64: `RateControl::for_hrd`).
    pub bit_rate: u32,
    /// CpbSize in bits (a multiple of 64).
    pub cpb_size: u32,
    /// cbr_flag: the CBR bucket (filler pads it), else VBR's max-rate bucket.
    pub cbr: bool,
    /// num_units_in_tick / time_scale (E.2.1): fps_den / (2 fps_num), two ticks per frame.
    pub num_units_in_tick: u32,
    pub time_scale: u32,
}

/// timing_info (E.1.1): the tick and fixed_frame_rate_flag 1.
fn timing_info(w: &mut BitWriter, h: &Hrd) {
    w.u(32, h.num_units_in_tick); // num_units_in_tick
    w.u(32, h.time_scale); // time_scale
    w.flag(true); // fixed_frame_rate_flag
}

/// The largest scale s (0..=15) with `v` a whole multiple of 2^(base + s) (v > 0).
fn hrd_scale(v: u32, base: u32) -> u32 {
    let mut s = 0;
    while s < 15 && base + s + 1 < 32 && v.is_multiple_of(1u32 << (base + s + 1)) {
        s += 1;
    }
    s
}

/// hrd_parameters (E.1.2): one CPB (SchedSelIdx 0), the exact BitRate / CpbSize, the delay fields 24 bits long.
fn hrd_parameters(w: &mut BitWriter, h: &Hrd) {
    let (bs, cs) = (hrd_scale(h.bit_rate, 6), hrd_scale(h.cpb_size, 4));
    w.ue(0); // cpb_cnt_minus1
    w.u(4, bs); // bit_rate_scale
    w.u(4, cs); // cpb_size_scale
    w.ue((h.bit_rate >> (6 + bs)).max(1) - 1); // bit_rate_value_minus1[0]
    w.ue((h.cpb_size >> (4 + cs)).max(1) - 1); // cpb_size_value_minus1[0]
    w.flag(h.cbr); // cbr_flag[0]
    w.u(5, 23); // initial_cpb_removal_delay_length_minus1
    w.u(5, 23); // cpb_removal_delay_length_minus1
    w.u(5, 23); // dpb_output_delay_length_minus1
    w.u(5, 24); // time_offset_length
}

/// level_idc under the HRD: the lowest level of Table A-1, not below the frame-size `level`, whose MaxBR and MaxCPB x
/// cpbBrNalFactor (Table A-2: 1200, High 1500) hold the signalled BitRate and CpbSize (A.3.1 item h); None returns `level`.
fn hrd_level(level: u8, hrd: Option<Hrd>, high: bool) -> u8 {
    const LIMITS: [(u8, u64, u64); 16] = [
        (10, 64, 175), (11, 192, 500), (12, 384, 1000), (13, 768, 2000), (20, 2000, 2000), (21, 4000, 4000), (22, 4000, 4000), (30, 10000, 10000),
        (31, 14000, 14000), (32, 20000, 20000), (40, 20000, 25000), (41, 50000, 62500), (42, 50000, 62500), (50, 135000, 135000), (51, 240000, 240000), (52, 240000, 240000),
    ];
    let Some(h) = hrd else { return level };
    let f = if high { 1500 } else { 1200 };
    for &(l, br, cpb) in &LIMITS {
        if l >= level && h.bit_rate as u64 <= br * f && h.cpb_size as u64 <= cpb * f {
            return l;
        }
    }
    level.max(52)
}

/// One sei_message (7.3.2.3.1): payloadType and payloadSize as 0xFF runs plus the last byte, then the payload.
fn sei_message(out: &mut Vec<u8>, ptype: usize, payload: &[u8]) {
    for mut v in [ptype, payload.len()] {
        while v >= 255 {
            out.push(0xFF);
            v -= 255;
        }
        out.push(v as u8);
    }
    out.extend_from_slice(payload);
}

/// The payload bytes of `w`, closed by the payload alignment (D.1.1: bit_equal_to_one then zeros) when not byte aligned.
fn sei_payload(mut w: BitWriter) -> Vec<u8> {
    if !w.bits().is_multiple_of(8) {
        w.trailing();
    }
    w.finish()
}

/// sei_rbsp (7.3.2.3) of one access unit under the HRD: on an IDR (`bp` Some(initial_cpb_removal_delay in 90 kHz units)) a
/// buffering_period (D.1.2: seq_parameter_set_id 0, the delay and a zero offset for the one NAL CPB, and with `vcl` (SR1060) the same
/// pair for the one VCL CPB), then the pic_timing
/// (D.1.3: cpb_removal_delay and dpb_output_delay in ticks, pic_struct_present_flag 0); rbsp_trailing_bits.
pub fn sei_rbsp(bp: Option<u32>, vcl: bool, cpb_removal_delay: u32, dpb_output_delay: u32) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(delay) = bp {
        let mut w = BitWriter::new();
        w.ue(0); // seq_parameter_set_id
        w.u(24, delay); // initial_cpb_removal_delay[0]
        w.u(24, 0); if vcl { w.u(24, delay); w.u(24, 0); } // initial_cpb_removal_delay_offset[0]; the VCL HRD's pair under Config::hrd_vcl (H264ENCHRDVCL SR1060)
        sei_message(&mut out, 0, &sei_payload(w));
    }
    let mut w = BitWriter::new();
    w.u(24, cpb_removal_delay & 0xFF_FFFF); // cpb_removal_delay
    w.u(24, dpb_output_delay & 0xFF_FFFF); // dpb_output_delay
    sei_message(&mut out, 1, &sei_payload(w));
    out.push(0x80); // rbsp_trailing_bits
    out
}

/// filler_data_rbsp (7.3.2.12): `n` ff_bytes and rbsp_trailing_bits (the NAL unit is n + 2 bytes).
pub fn filler_rbsp(n: usize) -> Vec<u8> {
    let mut v = alloc::vec![0xFFu8; n];
    v.push(0x80);
    v
}
