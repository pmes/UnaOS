//! M1 KATs — OBU framing, sequence header (every field), frame header (KEY / INTRA_ONLY),
//! tile info, and the AVIF container, from hand-built bitstreams and public files.

mod common;
use av1_core::avif::avif_payload;
use av1_core::obu::*;
use common::{fetch, BitWriter};

fn obu(t: u8, payload: &[u8], with_size: bool) -> Vec<u8> {
    let mut v = vec![(t << 3) | ((with_size as u8) << 1)];
    if with_size {
        let mut n = payload.len();
        loop {
            let b = (n & 0x7f) as u8;
            n >>= 7;
            if n == 0 {
                v.push(b);
                break;
            }
            v.push(b | 0x80);
        }
    }
    v.extend_from_slice(payload);
    v
}

/// A full (non-reduced) sequence header exercising timing info, the decoder model, two operating
/// points, frame ids, 128x128 superblocks, order hints, screen content selection and a 10-bit
/// 4:2:0 colour config with chroma sample position.
fn build_full_seq() -> Vec<u8> {
    let mut w = BitWriter::default();
    w.put(3, 0); // seq_profile
    w.flag(false); // still_picture
    w.flag(false); // reduced_still_picture_header
    w.flag(true); // timing_info_present_flag
    w.put(32, 1001); // num_units_in_display_tick
    w.put(32, 60000); // time_scale
    w.flag(true); // equal_picture_interval
    w.uvlc(0); // num_ticks_per_picture_minus_1
    w.flag(true); // decoder_model_info_present_flag
    w.put(5, 9); // buffer_delay_length_minus_1
    w.put(32, 90000); // num_units_in_decoding_tick
    w.put(5, 4); // buffer_removal_time_length_minus_1
    w.put(5, 6); // frame_presentation_time_length_minus_1
    w.flag(true); // initial_display_delay_present_flag
    w.put(5, 1); // operating_points_cnt_minus_1 -> 2 points
    // op 0
    w.put(12, 0x103); // operating_point_idc
    w.put(5, 9); // seq_level_idx (> 7 -> tier)
    w.put(1, 1); // seq_tier
    w.flag(true); // decoder_model_present_for_this_op
    w.put(10, 300); // decoder_buffer_delay
    w.put(10, 200); // encoder_buffer_delay
    w.flag(true); // low_delay_mode_flag
    w.flag(true); // initial_display_delay_present_for_this_op
    w.put(4, 5); // initial_display_delay_minus_1
    // op 1
    w.put(12, 0x101);
    w.put(5, 4); // no tier
    w.flag(false); // decoder model not present
    w.flag(false); // no initial display delay
    w.put(4, 10); // frame_width_bits_minus_1
    w.put(4, 10); // frame_height_bits_minus_1
    w.put(11, 1919); // max_frame_width_minus_1
    w.put(11, 1079); // max_frame_height_minus_1
    w.flag(true); // frame_id_numbers_present_flag
    w.put(4, 5); // delta_frame_id_length_minus_2
    w.put(3, 2); // additional_frame_id_length_minus_1
    w.flag(true); // use_128x128_superblock
    w.flag(true); // enable_filter_intra
    w.flag(false); // enable_intra_edge_filter
    w.flag(true); // enable_interintra_compound
    w.flag(false); // enable_masked_compound
    w.flag(true); // enable_warped_motion
    w.flag(false); // enable_dual_filter
    w.flag(true); // enable_order_hint
    w.flag(true); // enable_jnt_comp
    w.flag(false); // enable_ref_frame_mvs
    w.flag(false); // seq_choose_screen_content_tools
    w.put(1, 1); // seq_force_screen_content_tools
    w.flag(false); // seq_choose_integer_mv
    w.put(1, 0); // seq_force_integer_mv
    w.put(3, 6); // order_hint_bits_minus_1
    w.flag(true); // enable_superres
    w.flag(true); // enable_cdef
    w.flag(false); // enable_restoration
    // color_config
    w.flag(true); // high_bitdepth -> 10 bit
    w.flag(false); // mono_chrome
    w.flag(true); // color_description_present_flag
    w.put(8, 9); // color_primaries BT.2020
    w.put(8, 16); // transfer PQ
    w.put(8, 9); // matrix BT.2020 NCL
    w.flag(false); // color_range
    w.put(2, 1); // chroma_sample_position (vertical)
    w.flag(true); // separate_uv_delta_q
    w.flag(true); // film_grain_params_present
    w.trailing();
    w.bytes
}

#[test]
fn sequence_header_every_field() {
    let s = SequenceHeader::parse(&build_full_seq()).unwrap();
    assert_eq!(s.seq_profile, 0);
    assert!(!s.still_picture && !s.reduced_still_picture_header);
    assert!(s.timing_info_present_flag);
    assert_eq!(s.timing_info, TimingInfo { num_units_in_display_tick: 1001, time_scale: 60000, equal_picture_interval: true, num_ticks_per_picture_minus_1: 0 });
    assert!(s.decoder_model_info_present_flag);
    assert_eq!(
        s.decoder_model_info,
        DecoderModelInfo { buffer_delay_length_minus_1: 9, num_units_in_decoding_tick: 90000, buffer_removal_time_length_minus_1: 4, frame_presentation_time_length_minus_1: 6 }
    );
    assert!(s.initial_display_delay_present_flag);
    assert_eq!(s.operating_points.len(), 2);
    assert_eq!(
        s.operating_points[0],
        OperatingPoint {
            idc: 0x103,
            seq_level_idx: 9,
            seq_tier: 1,
            decoder_model_present_for_this_op: true,
            decoder_buffer_delay: 300,
            encoder_buffer_delay: 200,
            low_delay_mode_flag: true,
            initial_display_delay_present_for_this_op: true,
            initial_display_delay_minus_1: 5
        }
    );
    assert_eq!(s.operating_points[1], OperatingPoint { idc: 0x101, seq_level_idx: 4, ..Default::default() });
    assert_eq!(s.operating_point_idc, 0x103);
    assert_eq!((s.frame_width_bits_minus_1, s.frame_height_bits_minus_1), (10, 10));
    assert_eq!((s.max_frame_width_minus_1, s.max_frame_height_minus_1), (1919, 1079));
    assert!(s.frame_id_numbers_present_flag);
    assert_eq!((s.delta_frame_id_length_minus_2, s.additional_frame_id_length_minus_1), (5, 2));
    assert!(s.use_128x128_superblock && s.enable_filter_intra && !s.enable_intra_edge_filter);
    assert!(s.enable_interintra_compound && !s.enable_masked_compound && s.enable_warped_motion && !s.enable_dual_filter);
    assert!(s.enable_order_hint && s.enable_jnt_comp && !s.enable_ref_frame_mvs);
    assert!(!s.seq_choose_screen_content_tools);
    assert_eq!(s.seq_force_screen_content_tools, 1);
    assert_eq!(s.seq_force_integer_mv, 0);
    assert_eq!(s.order_hint_bits, 7);
    assert!(s.enable_superres && s.enable_cdef && !s.enable_restoration);
    let c = &s.color_config;
    assert_eq!(c.bit_depth, 10);
    assert!(c.high_bitdepth && !c.twelve_bit && !c.mono_chrome);
    assert_eq!(c.num_planes, 3);
    assert_eq!((c.color_primaries, c.transfer_characteristics, c.matrix_coefficients), (9, 16, 9));
    assert!(!c.color_range);
    assert_eq!((c.subsampling_x, c.subsampling_y), (1, 1));
    assert_eq!(c.chroma_sample_position, 1);
    assert!(c.separate_uv_delta_q);
    assert!(s.film_grain_params_present);
}

/// reduced_still_picture_header + profile 1 (4:4:4) with the sRGB/identity shortcut, and profile 2
/// 12-bit 4:2:2 paths of color_config.
fn reduced_seq(profile: u8, colour: &dyn Fn(&mut BitWriter)) -> Vec<u8> {
    let mut w = BitWriter::default();
    w.put(3, profile as u64);
    w.flag(true); // still_picture
    w.flag(true); // reduced_still_picture_header
    w.put(5, 8); // seq_level_idx[0]
    w.put(4, 7); // frame_width_bits_minus_1
    w.put(4, 6); // frame_height_bits_minus_1
    w.put(8, 199); // 200 wide
    w.put(7, 99); // 100 high
    w.flag(false); // use_128x128_superblock
    w.flag(false); // enable_filter_intra
    w.flag(true); // enable_intra_edge_filter
    w.flag(false); // enable_superres
    w.flag(true); // enable_cdef
    w.flag(true); // enable_restoration
    colour(&mut w);
    w.flag(false); // film_grain_params_present
    w.trailing();
    w.bytes
}

#[test]
fn sequence_header_reduced_and_colour_paths() {
    // profile 1, 8-bit, sRGB primaries/transfer + identity matrix -> 4:4:4 full range, no range bit
    let b = reduced_seq(1, &|w| {
        w.flag(false); // high_bitdepth
        w.flag(true); // color_description_present_flag
        w.put(8, 1);
        w.put(8, 13);
        w.put(8, 0);
        w.flag(false); // separate_uv_delta_q
    });
    let s = SequenceHeader::parse(&b).unwrap();
    assert!(s.reduced_still_picture_header && s.still_picture);
    assert_eq!(s.operating_points[0].seq_level_idx, 8);
    assert_eq!((s.max_frame_width_minus_1 + 1, s.max_frame_height_minus_1 + 1), (200, 100));
    assert_eq!(s.seq_force_screen_content_tools, 2);
    assert_eq!(s.seq_force_integer_mv, 2);
    assert_eq!(s.order_hint_bits, 0);
    let c = &s.color_config;
    assert!(c.color_range);
    assert_eq!((c.subsampling_x, c.subsampling_y, c.bit_depth), (0, 0, 8));
    // profile 2, 12-bit, 4:2:2 (subsampling_x = 1, subsampling_y = 0)
    let b = reduced_seq(2, &|w| {
        w.flag(true); // high_bitdepth
        w.flag(true); // twelve_bit
        w.flag(false); // mono_chrome
        w.flag(false); // no description
        w.flag(true); // color_range
        w.put(1, 1); // subsampling_x
        w.put(1, 0); // subsampling_y
        w.flag(false); // separate_uv_delta_q
    });
    let s = SequenceHeader::parse(&b).unwrap();
    let c = &s.color_config;
    assert_eq!((c.bit_depth, c.subsampling_x, c.subsampling_y), (12, 1, 0));
    assert_eq!((c.color_primaries, c.transfer_characteristics, c.matrix_coefficients), (2, 2, 2));
    // monochrome
    let b = reduced_seq(0, &|w| {
        w.flag(false);
        w.flag(true); // mono_chrome
        w.flag(false);
        w.flag(false); // color_range
    });
    let s = SequenceHeader::parse(&b).unwrap();
    assert!(s.color_config.mono_chrome);
    assert_eq!(s.color_config.num_planes, 1);
}

/// A KEY_FRAME header for the reduced still-picture sequence above (200x100, 64x64 SBs):
/// screen content, uniform tiles with one column increment, delta-coded quantizers, qmatrix,
/// segmentation with two features, delta q/lf, deblocking with ref/mode delta updates, CDEF with
/// two strength sets, Wiener luma / sgrproj chroma restoration, TX_MODE_SELECT, reduced_tx_set.
#[test]
fn key_frame_header_every_intra_field() {
    let seq = SequenceHeader::parse(&reduced_seq(0, &|w| {
        w.flag(false);
        w.flag(false);
        w.flag(false);
        w.flag(true); // color_range
        w.put(2, 0); // chroma_sample_position
        w.flag(true); // separate_uv_delta_q
    }))
    .unwrap();
    let mut w = BitWriter::default();
    // reduced still picture: no show_existing_frame / frame_type / show_frame bits
    w.flag(false); // disable_cdf_update
    w.flag(true); // allow_screen_content_tools (seq says SELECT)
    w.flag(false); // force_integer_mv (overridden to 1 for intra)
    // frame_size_override_flag = 0 (reduced), order_hint 0 bits, refresh = all (key+show)
    // frame_size(): no override -> 200x100; superres disabled in seq; render_size:
    w.flag(false); // render_and_frame_size_different
    w.flag(true); // allow_intrabc
    // disable_frame_end_update_cdf = 1 (reduced) -> nothing read
    // tile_info: sbCols = 4, sbRows = 2
    w.flag(true); // uniform_tile_spacing_flag
    w.flag(true); // increment_tile_cols_log2 -> 1
    w.flag(false); // stop
    w.flag(false); // increment_tile_rows_log2: stop at 0
    w.put(1, 1); // context_update_tile_id (TileColsLog2 + TileRowsLog2 = 1 bit)
    w.put(2, 3); // tile_size_bytes_minus_1 -> 4
    // quantization_params
    w.put(8, 100); // base_q_idx
    w.flag(true);
    w.su(7, -3); // DeltaQYDc
    w.flag(true); // diff_uv_delta (separate_uv_delta_q)
    w.flag(true);
    w.su(7, 5); // DeltaQUDc
    w.flag(false); // DeltaQUAc = 0
    w.flag(true);
    w.su(7, -7); // DeltaQVDc
    w.flag(true);
    w.su(7, 2); // DeltaQVAc
    w.flag(true); // using_qmatrix
    w.put(4, 3); // qm_y
    w.put(4, 4); // qm_u
    w.put(4, 5); // qm_v (separate)
    // segmentation_params
    w.flag(true); // segmentation_enabled; primary_ref_frame NONE -> update_data = 1
    for i in 0..8 {
        for j in 0..8 {
            if i == 1 && j == 0 {
                w.flag(true);
                w.su(9, -20); // ALT_Q
            } else if i == 2 && j == 6 {
                w.flag(true); // SKIP (0 bits)
            } else {
                w.flag(false);
            }
        }
    }
    // delta_q_params
    w.flag(true); // delta_q_present
    w.put(2, 2); // delta_q_res
    // delta_lf_params: allow_intrabc = 1 -> delta_lf_present not read (0)
    // loop_filter_params: allow_intrabc -> levels 0, nothing read
    // cdef_params: allow_intrabc -> nothing read
    // lr_params: allow_intrabc -> nothing read
    w.flag(true); // tx_mode_select
    w.flag(true); // reduced_tx_set
    // film grain not present
    w.trailing();
    let h = parse_frame_header(&w.bytes, &seq, 0, 0).unwrap();
    assert_eq!(h.frame_type, 0);
    assert!(h.frame_is_intra && h.show_frame && !h.showable_frame && h.error_resilient_mode == false);
    assert!(h.allow_screen_content_tools && h.force_integer_mv && h.allow_intrabc);
    assert!(h.disable_frame_end_update_cdf);
    assert_eq!(h.refresh_frame_flags, 0xff);
    assert_eq!((h.frame_width, h.frame_height, h.upscaled_width), (200, 100, 200));
    assert_eq!((h.render_width, h.render_height), (200, 100));
    assert_eq!((h.mi_cols, h.mi_rows), (50, 26));
    let t = &h.tile_info;
    assert_eq!((t.tile_cols_log2, t.tile_rows_log2, t.tile_cols, t.tile_rows), (1, 0, 2, 1));
    assert_eq!(t.mi_col_starts, vec![0, 32, 50]);
    assert_eq!(t.mi_row_starts, vec![0, 26]);
    assert_eq!((t.context_update_tile_id, t.tile_size_bytes), (1, 4));
    assert_eq!(h.base_q_idx, 100);
    assert_eq!((h.delta_q_y_dc, h.delta_q_u_dc, h.delta_q_u_ac, h.delta_q_v_dc, h.delta_q_v_ac), (-3, 5, 0, -7, 2));
    assert!(h.using_qmatrix);
    assert_eq!((h.qm_y, h.qm_u, h.qm_v), (3, 4, 5));
    assert!(h.segmentation_enabled && h.segmentation_update_map && h.segmentation_update_data);
    assert!(h.feature_enabled[1][0] && h.feature_enabled[2][6]);
    assert_eq!(h.feature_data[1][0], -20);
    assert!(h.seg_id_pre_skip);
    assert_eq!(h.last_active_seg_id, 2);
    assert!(h.delta_q_present && !h.delta_lf_present);
    assert_eq!(h.delta_q_res, 2);
    assert!(!h.coded_lossless);
    assert_eq!(h.seg_qm_level[0][1], 3);
    assert_eq!(h.loop_filter_level, [0, 0, 0, 0]);
    assert_eq!(h.cdef_bits, 0);
    assert_eq!(h.cdef_damping, 3);
    assert!(!h.uses_lr);
    assert_eq!(h.tx_mode, 2);
    assert!(h.reduced_tx_set);
    assert_eq!(h.get_qindex(true, 1, 0), 80);
}

/// The same sequence, no intrabc: deblocking with delta updates, CDEF, loop restoration.
#[test]
fn key_frame_header_filters() {
    let seq = SequenceHeader::parse(&reduced_seq(0, &|w| {
        w.flag(false);
        w.flag(false);
        w.flag(false);
        w.flag(true);
        w.put(2, 0);
        w.flag(false);
    }))
    .unwrap();
    let mut w = BitWriter::default();
    w.flag(true); // disable_cdf_update
    w.flag(false); // allow_screen_content_tools
    w.flag(false); // render_and_frame_size_different
    w.flag(false); // uniform_tile_spacing_flag = 0: explicit sizes
    // sbCols = 4: width_in_sbs_minus_1 ns(4) -> value 3 (all four)  ns(4): w=3,m=4, f(2)=3 <4
    w.put(2, 3);
    // sbRows = 2, maxTileHeightSb = max(8/4,1)=2 -> ns(2): w=2, m=2, f(1)
    w.put(1, 1); // height_in_sbs_minus_1 = 1
    w.put(8, 40); // base_q_idx
    w.flag(false); // DeltaQYDc
    w.flag(false); // DeltaQUDc
    w.flag(false); // DeltaQUAc
    w.flag(false); // using_qmatrix
    w.flag(false); // segmentation_enabled
    w.flag(false); // delta_q_present
    // loop_filter_params
    w.put(6, 10);
    w.put(6, 12);
    w.put(6, 7);
    w.put(6, 8);
    w.put(3, 2); // sharpness
    w.flag(true); // delta_enabled
    w.flag(true); // delta_update
    for i in 0..8 {
        if i == 1 {
            w.flag(true);
            w.su(7, -5);
        } else {
            w.flag(false);
        }
    }
    w.flag(true);
    w.su(7, 3); // mode delta 0
    w.flag(false);
    // cdef_params
    w.put(2, 2); // damping 5
    w.put(2, 1); // 2 sets
    for (yp, ys, up, us) in [(5u64, 3u64, 2u64, 1u64), (9, 0, 4, 3)] {
        w.put(4, yp);
        w.put(2, ys);
        w.put(4, up);
        w.put(2, us);
    }
    // lr_params
    w.put(2, 2); // Y: WIENER
    w.put(2, 3); // U: SGRPROJ
    w.put(2, 1); // V: SWITCHABLE
    w.put(1, 1); // lr_unit_shift (64x64 sb) -> 1
    w.put(1, 0); // lr_unit_extra_shift
    w.put(1, 1); // lr_uv_shift
    w.flag(false); // tx_mode_select -> LARGEST
    w.flag(false); // reduced_tx_set
    w.trailing();
    let h = parse_frame_header(&w.bytes, &seq, 0, 0).unwrap();
    assert!(h.disable_cdf_update);
    let t = &h.tile_info;
    assert!(!t.uniform_tile_spacing_flag);
    assert_eq!((t.tile_cols, t.tile_rows), (1, 1));
    assert_eq!(t.mi_row_starts, vec![0, 26]);
    assert_eq!(h.loop_filter_level, [10, 12, 7, 8]);
    assert_eq!(h.loop_filter_sharpness, 2);
    assert_eq!(h.loop_filter_ref_deltas, [1, -5, 0, 0, -1, 0, -1, -1]);
    assert_eq!(h.loop_filter_mode_deltas, [3, 0]);
    assert_eq!(h.cdef_damping, 5);
    assert_eq!(h.cdef_bits, 1);
    assert_eq!(&h.cdef_y_pri_strength[..2], &[5, 9]);
    assert_eq!(&h.cdef_y_sec_strength[..2], &[4, 0]); // 3 -> 4
    assert_eq!(&h.cdef_uv_pri_strength[..2], &[2, 4]);
    assert_eq!(&h.cdef_uv_sec_strength[..2], &[1, 4]);
    assert_eq!(h.frame_restoration_type, [1, 2, 3]);
    assert!(h.uses_lr);
    assert_eq!(h.loop_restoration_size, [128, 64, 64]);
    assert_eq!(h.tx_mode, 1);
}

#[test]
fn obu_framing() {
    let seq = build_full_seq();
    let mut stream = obu(OBU_TEMPORAL_DELIMITER_T, &[], true);
    stream.extend(obu(OBU_SEQUENCE_HEADER_T, &seq, true));
    stream.extend(obu(OBU_PADDING_T, &[1, 2, 3], false)); // last OBU may omit obu_size
    let obus = split_obus(&stream).unwrap();
    assert_eq!(obus.len(), 3);
    assert_eq!(obus[0].obu_type, OBU_TEMPORAL_DELIMITER_T);
    assert!(obus[0].payload.is_empty());
    assert_eq!(obus[1].obu_type, OBU_SEQUENCE_HEADER_T);
    assert_eq!(obus[1].payload, &seq[..]);
    assert_eq!(obus[2].payload, &[1, 2, 3]);
    // forbidden bit
    assert!(split_obus(&[0x80]).is_err());
    // obu_size past the end
    assert!(split_obus(&[0x12, 0x05, 0x00]).is_err());
}

/// The three small public AVIF files: container facts and their sequence/frame headers.
#[test]
fn avif_container_public_vectors() {
    let cases: [(&str, u32, u32, u8, bool, u32, (u32, u32)); 3] = [
        // name, w, h, profile, 128 sb, bit depth, subsampling
        ("white_1x1", 1, 1, 1, false, 8, (0, 0)),
        ("kodim23_yuv420_8bpc", 768, 512, 0, true, 8, (1, 1)),
        ("colors_sdr_srgb", 200, 200, 1, false, 8, (0, 0)),
    ];
    for (name, w, h, prof, sb128, bd, ss) in cases {
        let Some(file) = fetch(name) else { continue };
        let item = avif_payload(&file).unwrap();
        assert_eq!((item.width, item.height), (w, h), "{name} ispe");
        assert_eq!(item.bits_per_channel, vec![8, 8, 8], "{name} pixi");
        let c = item.av1c.as_ref().unwrap();
        assert_eq!(c.seq_profile, prof, "{name} av1C");
        let n = item.nclx.unwrap();
        assert_eq!((n.colour_primaries, n.transfer_characteristics, n.matrix_coefficients, n.full_range), (1, 13, 6, true));
        let obus = item.obus().unwrap();
        let types: Vec<u8> = obus.iter().map(|o| o.obu_type).collect();
        assert!(types.contains(&OBU_SEQUENCE_HEADER_T) && types.contains(&OBU_FRAME_T), "{name}: {types:?}");
        let sh = obus.iter().find(|o| o.obu_type == OBU_SEQUENCE_HEADER_T).unwrap();
        let s = SequenceHeader::parse(sh.payload).unwrap();
        assert_eq!(s.seq_profile, prof);
        assert!(s.still_picture, "{name} still_picture");
        assert_eq!(s.use_128x128_superblock, sb128);
        assert_eq!(s.color_config.bit_depth, bd);
        assert_eq!((s.color_config.subsampling_x, s.color_config.subsampling_y), ss);
        assert_eq!((s.max_frame_width_minus_1 + 1, s.max_frame_height_minus_1 + 1), (w, h));
        // av1C's configOBUs (optional; these files leave them empty) must agree when present
        for o in split_obus(&c.config_obus).unwrap() {
            assert_eq!(SequenceHeader::parse(o.payload).unwrap(), s);
        }
        let fr = obus.iter().find(|o| o.obu_type == OBU_FRAME_T).unwrap();
        let fh = parse_frame_header(fr.payload, &s, 0, 0).unwrap();
        assert_eq!(fh.frame_type, 0, "{name} KEY_FRAME");
        assert!(fh.show_frame);
        assert_eq!((fh.frame_width, fh.frame_height), (w, h));
        assert_eq!((fh.tile_info.tile_cols, fh.tile_info.tile_rows), (1, 1));
    }
}

#[test]
fn avif_rejects_garbage() {
    assert!(avif_payload(&[]).is_err());
    assert!(avif_payload(&[0, 0, 0, 8, b'f', b't', b'y', b'p']).is_err());
}
