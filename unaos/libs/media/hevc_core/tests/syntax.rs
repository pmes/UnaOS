// SPDX-License-Identifier: LGPL-3.0-or-later
//! HEVCPARSE M1 KATs over hand-assembled bytes, one per syntax structure, each cited to its clause: the NAL unit
//! header (7.3.1.2) and emulation prevention (7.4.2), hvcC (ISO/IEC 14496-15 8.3.3.1) agreeing with the Annex B path,
//! the VPS with timing and HRD (7.3.2.1, E.2.2), the SPS with scaling_list_data (7.3.4, 7.4.5), two st_ref_pic_sets
//! (7.3.7; the second inter-predicted, 7.4.8), long-term pictures and a VUI with HRD (E.2.1), the PPS with uniform and
//! explicit tiles (6.5.1) and deblocking control, I and P slice segment headers (7.3.6.1, pred_weight_table 7.3.6.3,
//! entry points), every context table of 9.3.2.2 against the initValue counts and hand-computed states, the arithmetic
//! engine and the binarizations round-tripping through the test encoder, and the scans (6.5.3 .. 6.5.5).

mod common;

use common::*;
use hevc_core::cabac::{initial_contexts, Engine};
use hevc_core::nal::{self, parse_header, parse_hvcc};
use hevc_core::ps::{parse_pps, parse_sps, parse_vps, Geometry, SCAN_ORDER};
use hevc_core::slice::{parse_slice_header, SliceType};
use hevc_core::tables::*;
use hevc_core::{Decoder, Error};

#[test]
fn nal_header_and_emulation_prevention() {
    let h = parse_header(&[0x40, 0x01]).unwrap();
    assert_eq!((h.nal_type, h.layer_id, h.temporal_id), (nal::VPS_NUT, 0, 0));
    let h = parse_header(&[0x26, 0x01]).unwrap();
    assert!(h.is_idr() && h.is_irap() && h.is_vcl());
    let h = parse_header(&[0x2a, 0x0b]).unwrap(); // CRA, layer 1, tid 2
    assert_eq!((h.nal_type, h.layer_id, h.temporal_id), (nal::CRA_NUT, 1, 2));
    assert_eq!(parse_header(&[0x80, 0x01]), Err(Error::Invalid("forbidden_zero_bit set")));
    assert_eq!(parse_header(&[0x40, 0x00]), Err(Error::Invalid("nuh_temporal_id_plus1 is 0")));
    let mut out = Vec::new();
    nal::unescape(&[0, 0, 3, 1, 0, 0, 3, 0], &mut out);
    assert_eq!(out, [0, 0, 1, 0, 0, 0]);
    assert!(parse_header(&[0x10, 0x01]).unwrap().is_rasl()); // RASL_N (8)
    assert!(parse_header(&[0x00, 0x01]).unwrap().is_slnr()); // TRAIL_N
}

fn unit_payload(annexb_unit: &[u8]) -> Vec<u8> {
    annexb_unit[4..].to_vec()
}

#[test]
fn hvcc_record_and_length_prefixed_samples_decode_like_annexb() {
    let cfg = SpsCfg::default();
    let (v, s, p) = (vps(false), sps(&cfg), pps(&PpsCfg { init_qp: 26, ..Default::default() }));
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, false), 0, 26);
    for (inc, mpm) in [(0, 1); 6] {
        e.split_cu(inc, false);
        e.dc_cu(mpm);
    }
    let slice = nal(19, &e.finish().out);
    let mut rec = vec![1u8, 0x01, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 60, 0xf0, 0x00, 0xfc, 0xfd, 0xf8, 0xf8, 0, 0, 0x0f, 3];
    for (ty, u) in [(32u8, &v), (33, &s), (34, &p)] {
        let b = unit_payload(u);
        rec.extend([0x80 | ty, 0, 1, (b.len() >> 8) as u8, b.len() as u8]);
        rec.extend(&b);
    }
    let c = parse_hvcc(&rec).unwrap();
    assert_eq!((c.general_profile_idc, c.general_level_idc, c.chroma_format_idc, c.bit_depth_luma), (1, 60, 1, 8));
    assert_eq!((c.nal_length_size, c.num_temporal_layers, c.temporal_id_nested), (4, 1, true));
    assert_eq!(c.general_profile_compatibility_flags, 0x6000_0000);
    assert_eq!(c.arrays.len(), 3);
    assert!(c.arrays.iter().all(|a| a.array_completeness && a.units.len() == 1));
    assert_eq!(c.arrays[1].nal_type, 33);
    let body = unit_payload(&slice);
    let mut sample = (body.len() as u32).to_be_bytes().to_vec();
    sample.extend(&body);
    let mut d = Decoder::new();
    d.configure_hvcc(&rec).unwrap();
    d.decode_sample(&sample).unwrap();
    d.flush();
    let a = d.take_frames();
    let mut stream = v.clone();
    stream.extend(&s);
    stream.extend(&p);
    stream.extend(&slice);
    let mut d = Decoder::new();
    d.decode_annexb(&stream).unwrap();
    d.flush();
    assert_eq!(a, d.take_frames());
    assert_eq!(a.len(), 1);
    assert_eq!(parse_hvcc(&rec[..20]), Err(Error::Truncated));
}

fn rbsp_of(annexb_unit: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    nal::unescape(&annexb_unit[6..], &mut out);
    out
}

#[test]
fn vps_with_timing_and_hrd() {
    let v = parse_vps(&rbsp_of(&vps(true))).unwrap();
    assert_eq!((v.id, v.max_sub_layers_minus1, v.temporal_id_nesting_flag), (0, 0, true));
    assert_eq!(v.ptl.general_profile_idc, 1);
    assert_eq!(v.ptl.general_level_idc, 60);
    assert!(v.ptl.general_progressive_source_flag && v.ptl.general_frame_only_constraint_flag);
    assert_eq!(v.ordering[0].max_dec_pic_buffering_minus1, 1);
    assert_eq!(v.timing, Some((1001, 30000)));
    assert_eq!(v.hrd.len(), 1);
    assert!(v.hrd[0].nal_hrd_parameters_present_flag && !v.hrd[0].vcl_hrd_parameters_present_flag);
    assert_eq!(v.hrd[0].cpb_cnt_minus1, vec![0]);
    assert!(!v.extension_flag);
}

/// An SPS (7.3.2.2) with scaling_list_data, two short-term RPSs (the second predicted from the first), one long-term
/// candidate and a VUI with timing, HRD and bitstream restriction. 128x64, CTB 16.
fn rich_sps() -> Vec<u8> {
    let mut b = BitWriter::default();
    b.u(0, 4);
    b.u(0, 3);
    b.bit(1);
    ptl(&mut b, 0);
    b.ue(0);
    b.ue(1);
    b.ue(128);
    b.ue(64);
    b.bit(0);
    b.ue(0);
    b.ue(0);
    b.ue(4);
    b.bit(1);
    b.ue(4);
    b.ue(2);
    b.ue(0);
    b.ue(0); // log2_min_luma_coding_block_size_minus3
    b.ue(1); // CTB 16
    b.ue(0);
    b.ue(2); // TB 4..16
    b.ue(1);
    b.ue(1);
    b.bit(1); // scaling_list_enabled_flag
    b.bit(1); // sps_scaling_list_data_present_flag
    // scaling_list_data (7.3.4)
    for size_id in 0..4 {
        let step = if size_id == 3 { 3 } else { 1 };
        let mut m = 0;
        while m < 6 {
            match (size_id, m) {
                (0, 0) => {
                    b.bit(1);
                    b.se(0); // 8
                    for _ in 1..16 {
                        b.se(1); // 9 .. 23
                    }
                }
                (2, 0) => {
                    b.bit(1);
                    b.se(12); // DC 20
                    for _ in 0..64 {
                        b.se(0); // all 20
                    }
                }
                (1, _) | (3, 0) => {
                    b.bit(0);
                    b.ue(0); // the default list
                }
                _ => {
                    b.bit(0);
                    b.ue(1); // the previous matrix (refMatrixId = matrixId - 1 * step)
                }
            }
            m += step;
        }
    }
    b.bit(0); // amp
    b.bit(1); // sao
    b.bit(0); // pcm
    b.ue(2); // num_short_term_ref_pic_sets
    // st_ref_pic_set(0): S0 = -1 (used), -3 (unused); S1 = +2 (used).
    b.ue(2);
    b.ue(1);
    b.ue(0);
    b.bit(1);
    b.ue(1);
    b.bit(0);
    b.ue(1);
    b.bit(1);
    // st_ref_pic_set(1): inter_ref_pic_set_prediction_flag, deltaRps = -1, every used_by_curr_pic_flag 1.
    b.bit(1);
    b.bit(1);
    b.ue(0);
    for _ in 0..4 {
        b.bit(1);
    }
    b.bit(1); // long_term_ref_pics_present_flag
    b.ue(1);
    b.u(5, 8);
    b.bit(1);
    b.bit(0); // sps_temporal_mvp_enabled_flag
    b.bit(1); // strong_intra_smoothing_enabled_flag
    b.bit(1); // vui_parameters_present_flag
    b.bit(1);
    b.u(255, 8);
    b.u(4, 16);
    b.u(3, 16);
    b.bit(0);
    b.bit(1);
    b.u(5, 3);
    b.bit(1); // video_full_range_flag
    b.bit(1);
    b.u(1, 8);
    b.u(1, 8);
    b.u(1, 8);
    b.bit(0);
    b.bit(0);
    b.bit(0);
    b.bit(0);
    b.bit(0); // default_display_window_flag
    b.bit(1); // vui_timing_info_present_flag
    b.u(1001, 32);
    b.u(60000, 32);
    b.bit(0);
    b.bit(1); // vui_hrd_parameters_present_flag
    hrd(&mut b);
    b.bit(1); // bitstream_restriction_flag
    b.u(0, 3);
    b.ue(0);
    b.ue(2);
    b.ue(1);
    b.ue(15);
    b.ue(15);
    b.bit(0); // sps_extension_present_flag
    b.trailing();
    b.out
}

#[test]
fn sps_with_scaling_lists_rps_long_term_and_vui() {
    let s = parse_sps(&rich_sps()).unwrap();
    assert_eq!((s.width, s.height, s.log2_ctb, s.log2_min_cb, s.log2_min_tb, s.log2_max_tb), (128, 64, 4, 3, 2, 4));
    assert_eq!((s.width_in_ctbs(), s.height_in_ctbs()), (8, 4));
    assert_eq!(s.log2_max_poc_lsb, 8);
    assert_eq!(s.ordering[0].max_dec_pic_buffering_minus1, 4);
    let sl = s.scaling_list.as_ref().unwrap();
    assert_eq!(&sl.list[0][0][..16], &(8u8..24).collect::<Vec<_>>()[..]);
    assert_eq!(sl.list[0][5], sl.list[0][0]);
    // ScalingFactor (7.4.5) through the 4x4 up-right diagonal scan: i = 1 is (0,1), i = 2 is (1,0).
    assert_eq!((sl.factor[0][0][0], sl.factor[0][0][4], sl.factor[0][0][1]), (8, 9, 10));
    assert_eq!(sl.factor[1][4][63], 91);
    assert_eq!(sl.factor[2][0][0], 20);
    assert!(sl.factor[2][3].iter().all(|&v| v == 20));
    // sizeId 3, matrixId 3 predicted from refMatrixId 3 - 1 * 3 = 0: the default intra list, DC 16.
    assert_eq!(sl.factor[3][3], sl.factor[3][0]);
    assert_eq!((sl.factor[3][3][0], sl.factor[3][3][31 * 32 + 31], sl.factor[3][3][1]), (16, 115, 16));
    assert_eq!(s.st_rps.len(), 2);
    assert_eq!((s.st_rps[0].delta_poc_s0.clone(), s.st_rps[0].used_s0.clone()), (vec![-1, -3], vec![true, false]));
    assert_eq!(s.st_rps[0].delta_poc_s1, vec![2]);
    // 7.4.8 with deltaRps -1: S0 = {-1 (the deltaRps entry), -2, -4}, S1 = {+1}.
    assert_eq!(s.st_rps[1].delta_poc_s0, vec![-1, -2, -4]);
    assert_eq!(s.st_rps[1].delta_poc_s1, vec![1]);
    assert_eq!(s.st_rps[1].num_used(), 4);
    assert_eq!((s.lt_ref_pic_poc_lsb_sps.clone(), s.used_by_curr_pic_lt_sps_flag.clone()), (vec![5], vec![true]));
    assert!(s.sample_adaptive_offset_enabled_flag && s.strong_intra_smoothing_enabled_flag);
    let v = s.vui.as_ref().unwrap();
    assert_eq!((v.aspect_ratio_idc, v.sar), (255, (4, 3)));
    assert!(v.video_full_range_flag);
    assert_eq!(v.colour, Some((1, 1, 1)));
    assert_eq!(v.timing, Some((1001, 60000)));
    assert_eq!(v.hrd.as_ref().unwrap().cpb_cnt_minus1, vec![0]);
    assert!(v.bitstream_restriction_flag);
    assert_eq!((v.max_bytes_per_pic_denom, v.max_bits_per_min_cu_denom), (2, 1));
}

/// A PPS (7.3.2.3): tiles (uniform or explicit 2x2), WPP, weighted prediction, deblocking control with offsets.
fn tiles_pps(uniform: bool) -> Vec<u8> {
    let mut b = BitWriter::default();
    b.ue(3);
    b.ue(0);
    b.bit(1); // dependent_slice_segments_enabled_flag
    b.bit(0);
    b.u(0, 3);
    b.bit(1); // sign_data_hiding_enabled_flag
    b.bit(0);
    b.ue(1);
    b.ue(0);
    b.se(-4); // init_qp 22
    b.bit(0);
    b.bit(1); // transform_skip_enabled_flag
    b.bit(1); // cu_qp_delta_enabled_flag
    b.ue(1);
    b.se(-2);
    b.se(3);
    b.bit(0);
    b.bit(1); // weighted_pred_flag
    b.bit(0);
    b.bit(0);
    b.bit(1); // tiles_enabled_flag
    b.bit(1); // entropy_coding_sync_enabled_flag
    b.ue(1);
    b.ue(1);
    b.flag(uniform);
    if !uniform {
        b.ue(2); // column 0: 3 CTBs
        b.ue(0); // row 0: 1 CTB
    }
    b.bit(0); // loop_filter_across_tiles_enabled_flag
    b.bit(1);
    b.bit(1); // deblocking_filter_control_present_flag
    b.bit(1); // override enabled
    b.bit(0);
    b.se(-2);
    b.se(3);
    b.bit(0);
    b.bit(0);
    b.ue(0);
    b.bit(0);
    b.bit(0);
    b.trailing();
    b.out
}

#[test]
fn pps_tiles_and_the_tile_scan() {
    let sps = parse_sps(&rich_sps()).unwrap(); // 8 x 4 CTBs
    let p = parse_pps(&tiles_pps(true)).unwrap();
    assert_eq!((p.id, p.init_qp, p.diff_cu_qp_delta_depth, p.cb_qp_offset, p.cr_qp_offset), (3, 22, 1, -2, 3));
    assert!(p.sign_data_hiding_enabled_flag && p.transform_skip_enabled_flag && p.weighted_pred_flag);
    assert!(p.tiles_enabled_flag && p.entropy_coding_sync_enabled_flag && !p.loop_filter_across_tiles_enabled_flag);
    assert!(p.deblocking_filter_override_enabled_flag && !p.pps_deblocking_filter_disabled_flag);
    assert_eq!((p.beta_offset_div2, p.tc_offset_div2), (-2, 3));
    let g = Geometry::new(&sps, &p).unwrap();
    // Uniform 2x2: columns 4 + 4, rows 2 + 2 (6.5.1).
    assert_eq!((g.col_bd.clone(), g.row_bd.clone()), (vec![0, 4, 8], vec![0, 2, 4]));
    assert_eq!((g.rs_to_ts[4], g.rs_to_ts[8], g.rs_to_ts[12], g.rs_to_ts[16]), (8, 4, 12, 16));
    assert_eq!((g.tile_id[g.rs_to_ts[4] as usize], g.tile_id[g.rs_to_ts[31] as usize]), (1, 3));
    for rs in 0..32 {
        assert_eq!(g.ts_to_rs[g.rs_to_ts[rs] as usize], rs as u32);
    }
    let p = parse_pps(&tiles_pps(false)).unwrap();
    assert_eq!((p.column_widths.clone(), p.row_heights.clone()), (vec![3], vec![1]));
    let g = Geometry::new(&sps, &p).unwrap();
    assert_eq!((g.col_bd.clone(), g.row_bd.clone()), (vec![0, 3, 8], vec![0, 1, 4]));
    // rs 11 = (3, 1) lies in tile (1, 1): the 3 x 3 CTBs of tile (0, 1) and the 8 x 1 of tile row 0 precede it.
    assert_eq!((g.rs_to_ts[3], g.rs_to_ts[8], g.rs_to_ts[11]), (3, 8, 3 * 3 + 8));
    // z-scan (6.5.2) at 4x4 granularity inside the first CTB (16x16 = 4x4 blocks).
    assert_eq!((g.zs[0], g.zs[1], g.zs[g.w4], g.zs[g.w4 + 1], g.zs[2]), (0, 1, 2, 3, 4));
}

#[test]
fn i_and_p_slice_segment_headers() {
    let sps = parse_sps(&rich_sps()).unwrap();
    let pps = parse_pps(&tiles_pps(true)).unwrap();
    // A CRA I slice header, not the first segment: slice_segment_address u(5) (32 CTBs).
    let mut b = BitWriter::default();
    b.bit(0);
    b.bit(1); // no_output_of_prior_pics_flag
    b.ue(3);
    b.bit(0); // dependent_slice_segment_flag
    b.u(9, 5);
    b.ue(2);
    b.u(17, 8); // slice_pic_order_cnt_lsb
    b.bit(1); // short_term_ref_pic_set_sps_flag
    b.u(0, 1);
    b.ue(0); // num_long_term_sps
    b.ue(0); // num_long_term_pics
    b.bit(1);
    b.bit(0); // slice_sao_luma / chroma
    b.se(2);
    b.bit(1); // deblocking_filter_override_flag
    b.bit(0);
    b.se(1);
    b.se(-1);
    b.bit(0); // slice_loop_filter_across_slices_enabled_flag
    b.ue(1); // num_entry_point_offsets
    b.ue(15);
    b.u(299, 16);
    b.trailing();
    let hdr_len = b.out.len();
    b.u(0xab, 8);
    let cra = hevc_core::nal::parse_header(&[(nal::CRA_NUT << 1), 1]).unwrap();
    let h = parse_slice_header(&b.out, &cra, &sps, &pps, None).unwrap();
    assert!(!h.first_slice_segment_in_pic_flag && h.no_output_of_prior_pics_flag);
    assert_eq!((h.segment_address, h.slice_type, h.pic_order_cnt_lsb), (9, SliceType::I, 17));
    assert!(h.short_term_ref_pic_set_sps_flag && h.sao_luma_flag && !h.sao_chroma_flag);
    assert_eq!((h.slice_qp(&pps), h.beta_offset_div2, h.tc_offset_div2), (24, 1, -1));
    assert!(!h.loop_filter_across_slices_enabled_flag);
    assert_eq!(h.entry_point_offsets, vec![300]);
    assert_eq!(h.data_offset, hdr_len);

    // A TRAIL_R P slice header: RPS 1 from the SPS, a long-term picture from the SPS and one in the header,
    // two active references, pred_weight_table, merge candidates, entry points.
    let mut b = BitWriter::default();
    b.bit(1);
    b.ue(3);
    b.ue(1); // P
    b.u(7, 8);
    b.bit(1);
    b.u(1, 1); // short_term_ref_pic_set_idx
    b.ue(1); // num_long_term_sps
    b.ue(1); // num_long_term_pics
    b.bit(0); // delta_poc_msb_present_flag[0]
    b.u(9, 8); // poc_lsb_lt[1]
    b.bit(0);
    b.bit(1);
    b.ue(2); // delta_poc_msb_cycle_lt[1]
    b.bit(0);
    b.bit(0); // sao
    b.bit(1); // num_ref_idx_active_override_flag
    b.ue(1);
    b.ue(6); // luma_log2_weight_denom
    b.se(-1); // chroma denom 5
    b.bit(1);
    b.bit(0);
    b.bit(0);
    b.bit(1);
    b.se(3);
    b.se(-2);
    for _ in 0..2 {
        b.se(-1);
        b.se(4);
    }
    b.ue(2); // five_minus_max_num_merge_cand
    b.se(-3);
    b.bit(0); // deblocking_filter_override_flag
    b.bit(0); // slice_loop_filter_across_slices_enabled_flag
    b.ue(2);
    b.ue(7);
    b.u(9, 8);
    b.u(19, 8);
    b.trailing();
    let trail = hevc_core::nal::parse_header(&[(nal::TRAIL_R << 1), 1]).unwrap();
    let h = parse_slice_header(&b.out, &trail, &sps, &pps, None).unwrap();
    assert_eq!((h.slice_type, h.pic_order_cnt_lsb, h.short_term_ref_pic_set_idx), (SliceType::P, 7, 1));
    assert_eq!(h.long_term.len(), 2);
    assert_eq!((h.long_term[0].poc_lsb, h.long_term[0].used), (5, true));
    assert_eq!((h.long_term[1].poc_lsb, h.long_term[1].used, h.long_term[1].delta_poc_msb_cycle), (9, false, Some(2)));
    assert_eq!(h.num_pic_total_curr(&sps), 5);
    assert_eq!(h.num_ref_idx_l0_active, 2);
    let w = h.pred_weights.as_ref().unwrap();
    assert_eq!((w.luma_log2_weight_denom, w.chroma_log2_weight_denom), (6, 5));
    assert_eq!(w.l0.luma, vec![(67, -2), (64, 0)]);
    assert_eq!(w.l0.chroma[0], [(32, 0), (32, 0)]);
    assert_eq!(w.l0.chroma[1], [(31, 4), (31, 4)]);
    assert_eq!((h.max_num_merge_cand, h.slice_qp(&pps)), (3, 19));
    assert_eq!(h.entry_point_offsets, vec![10, 20]);
}

#[test]
fn every_context_table_kat() {
    // (name, contexts per initType as Table 9-4 assigns them, the first initValue of initType 0 / 1 / 2).
    let want: &[(&str, usize, [u8; 3])] = &[
        ("sao_merge_flag", 1, [153, 153, 153]),
        ("sao_type_idx", 1, [200, 185, 160]),
        ("split_cu_flag", 3, [139, 107, 107]),
        ("cu_transquant_bypass_flag", 1, [154, 154, 154]),
        ("cu_skip_flag", 3, [0, 197, 197]),
        ("pred_mode_flag", 1, [0, 149, 134]),
        ("part_mode", 4, [184, 154, 154]),
        ("prev_intra_luma_pred_flag", 1, [184, 154, 183]),
        ("intra_chroma_pred_mode", 1, [63, 152, 152]),
        ("rqt_root_cbf", 1, [0, 79, 79]),
        ("merge_flag", 1, [0, 110, 154]),
        ("merge_idx", 1, [0, 122, 137]),
        ("inter_pred_idc", 5, [0, 95, 95]),
        ("ref_idx", 2, [0, 153, 153]),
        ("mvp_flag", 1, [0, 168, 168]),
        ("split_transform_flag", 3, [153, 124, 224]),
        ("cbf_luma", 2, [111, 153, 153]),
        ("cbf_chroma", 5, [94, 149, 149]),
        ("abs_mvd_greater0_flag", 1, [0, 140, 169]),
        ("abs_mvd_greater1_flag", 1, [0, 198, 198]),
        ("cu_qp_delta_abs", 2, [154, 154, 154]),
        ("transform_skip_flag", 2, [139, 139, 139]),
        ("last_sig_coeff_x_prefix", 18, [110, 125, 125]),
        ("last_sig_coeff_y_prefix", 18, [110, 125, 125]),
        ("coded_sub_block_flag", 4, [91, 121, 121]),
        ("sig_coeff_flag", 44, [111, 155, 170]),
        ("coeff_abs_level_greater1_flag", 24, [140, 154, 154]),
        ("coeff_abs_level_greater2_flag", 6, [138, 107, 107]),
    ];
    assert_eq!(CTX_TABLES.len(), want.len());
    let mut next = 0;
    for (t, &(name, n, first)) in CTX_TABLES.iter().zip(want) {
        assert_eq!(t.name, name);
        assert_eq!(t.slot, next, "{name}: slots are contiguous in Table 9-4 order");
        assert_eq!(t.len(), n, "{name}");
        for it in 0..3 {
            let row = t.init[it];
            // part_mode has one context in I slices (Table 9-11: ctxIdx 0 for initType 0), four otherwise.
            let short_ok = name == "part_mode" && it == 0 && row.len() == 1;
            assert!(row.is_empty() || row.len() == n || short_ok, "{name} initType {it}");
            assert_eq!(row.first().copied().unwrap_or(0), first[it], "{name} initType {it}");
        }
        next += n;
    }
    assert_eq!(next, CTX_END);
    assert!(CTX_END <= h264_core::cabac::NUM_CTX);
    // 9.3.2.2 by hand at SliceQpY 26: initValue -> (pStateIdx, valMps).
    let hand: &[(u8, u8, u8)] = &[(139, 0, 0), (154, 0, 1), (63, 8, 0), (200, 8, 1), (110, 7, 1), (227, 15, 0), (74, 40, 0), (31, 24, 0), (224, 39, 0)];
    for &(v, p, mps) in hand {
        assert_eq!(init_state(v, 26), (p << 1) | mps, "initValue {v}");
    }
    assert_eq!(init_state(154, 60), init_state(154, 51));
    // The engine holds exactly those states after init.
    for it in 0..3 {
        let mut e = Engine::new(&[0u8; 4], 0);
        e.init_contexts(it, 26);
        for t in CTX_TABLES {
            for i in 0..t.len() {
                let v = t.init[it].get(i).copied().unwrap_or(CNU);
                assert_eq!(e.state(t.slot + i), init_state(v, 26));
            }
        }
        assert_eq!(e.save(), initial_contexts(it, 26));
    }
    assert_eq!(hevc_core::cabac::init_type(SliceType::P, true), 2);
    assert_eq!(hevc_core::cabac::init_type(SliceType::B, true), 1);
}

#[test]
fn engine_and_binarizations_round_trip() {
    let mut e = CabacEnc::new(BitWriter::default(), 0, 30);
    let mut seq = Vec::new();
    let mut x = 12345u32;
    for i in 0..2000u32 {
        x = x.wrapping_mul(1103515245).wrapping_add(12345);
        let ctx = (x >> 8) as usize % 20 + SIG;
        let bin = ((x >> 16) % 5 == 0) as u32 ^ (ctx & 1) as u32;
        seq.push((ctx, bin));
        e.decision(ctx, bin);
        if i % 7 == 0 {
            e.bypass(x >> 31);
        }
        if i % 11 == 0 {
            e.terminate(0);
        }
    }
    for v in 0..200u32 {
        e.remaining(v, v % 5);
    }
    for v in [0u32, 1, 2, 7, 100, 4095] {
        e.egk(v, 0);
    }
    e.bypass_bits(0x15, 5);
    let bw = e.finish();
    let mut d = Engine::new(&bw.out, 0);
    d.init_contexts(0, 30);
    let mut x = 12345u32;
    for (i, &(ctx, bin)) in seq.iter().enumerate() {
        x = x.wrapping_mul(1103515245).wrapping_add(12345);
        assert_eq!(d.decision(ctx), bin, "bin {i}");
        if i % 7 == 0 {
            assert_eq!(d.bypass(), x >> 31);
        }
        if i % 11 == 0 {
            assert_eq!(d.terminate(), 0);
        }
    }
    for v in 0..200u32 {
        assert_eq!(d.coeff_abs_level_remaining(v % 5), v);
    }
    for v in [0u32, 1, 2, 7, 100, 4095] {
        assert_eq!(d.bypass_egk(0), v);
    }
    assert_eq!(d.bypass_bits(5), 0x15);
    assert_eq!(d.terminate(), 1);
    assert_eq!(d.aligned_pos(), bw.out.len());
}

#[test]
fn scans_kat() {
    let d4: Vec<(u8, u8)> = SCAN_ORDER[2][0][..6].to_vec();
    assert_eq!(d4, [(0, 0), (0, 1), (1, 0), (0, 2), (1, 1), (2, 0)]);
    assert_eq!(SCAN_ORDER[2][0][15], (3, 3));
    assert_eq!(&SCAN_ORDER[1][0][..4], &[(0, 0), (0, 1), (1, 0), (1, 1)]);
    assert_eq!(&SCAN_ORDER[2][1][..5], &[(0, 0), (1, 0), (2, 0), (3, 0), (0, 1)]);
    assert_eq!(&SCAN_ORDER[2][2][..5], &[(0, 0), (0, 1), (0, 2), (0, 3), (1, 0)]);
    assert_eq!(SCAN_ORDER[3][0][63], (7, 7));
}

/// An hvcC (ISO/IEC 14496-15 8.3.3.1: Main, level 60, 4:2:0, 8-bit, 4-byte lengths) over the given (type, NAL unit)
/// arrays, one unit each.
fn hvcc_of(units: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut rec = vec![1u8, 0x01, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 60, 0xf0, 0x00, 0xfc, 0xfd, 0xf8, 0xf8, 0, 0, 0x0f];
    rec.push(units.len() as u8);
    for (ty, u) in units {
        rec.extend([0x80 | ty, 0, 1, (u.len() >> 8) as u8, u.len() as u8]);
        rec.extend(u);
    }
    rec
}

/// HEVCDEPTH-CORE (LEDGER SR304): `ps::hvcc_reorder_depth` reads the hvcC's first SPS's sps_max_num_reorder_pics at the
/// highest sub-layer through `parse_sps`; a record that does not parse, or carries no SPS, is `None`.
#[test]
fn hvcc_reorder_depth_reads_the_first_sps() {
    use hevc_core::ps::hvcc_reorder_depth;
    let cfg = SpsCfg::default();
    let deep = InterSps { max_dec_pic_buffering_minus1: 2, max_num_reorder_pics: 2, ..Default::default() };
    let pps_unit = nal_bytes(34, &pps_rbsp(&PpsCfg::default()));
    let rec = hvcc_of(&[(32, unit_payload(&vps(false))), (33, nal_bytes(33, &sps_rbsp_ext(&cfg, &deep))), (34, pps_unit.clone())]);
    assert_eq!(hvcc_reorder_depth(&rec), Some(2), "sps_max_num_reorder_pics");
    assert_eq!(hvcc_reorder_depth(&hvcc_of(&[(33, nal_bytes(33, &sps_rbsp(&cfg)))])), Some(0));
    assert_eq!(hvcc_reorder_depth(&rec[..20]), None, "a truncated record");
    assert_eq!(hvcc_reorder_depth(&hvcc_of(&[(34, pps_unit)])), None, "no SPS array");
}
