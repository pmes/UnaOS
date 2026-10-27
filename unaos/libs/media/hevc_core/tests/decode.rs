// SPDX-License-Identifier: LGPL-3.0-or-later
//! HEVCPARSE M2 KATs: intra decoding over hand-assembled all-intra streams (no reference encoder exists on the build
//! host and the ITU-T H.265.1 conformance bitstreams are not embedded — HEVCCONF), each checkable by construction:
//! (a) a 64x48 picture of INTRA_DC coding units with no residual is 128 everywhere (8.4.4.2.6 DC with no neighbours is
//! 1 << (BitDepth - 1); deblocking a flat picture changes nothing); (b) the same with SAO band offset (band 16, +3) is
//! 131 in luma; (c) one DC coefficient in the last 8x8 luma TU, deblocking off, makes that TU 128 + d with d computed
//! from 8.6.2 / 8.6.4; (d) a pcm coding unit above an INTRA_ANGULAR26 coding unit: the latter copies the pcm block's
//! bottom row (8.4.4.2.6 with the mode 26 edge filter over substituted left samples); (e) unit KATs of the
//! transforms, the intra predictors, the deblocking filter and SAO against hand computation; (f) the refusals by name.

mod common;

use common::*;
use hevc_core::deblock::deblock_picture;
use hevc_core::intra::predict;
use hevc_core::picture::{Picture, SaoParams, SliceInfo, F_EDGE_V};
use hevc_core::ps::{Geometry, Ordering, Pps, Sps};
use hevc_core::sao::sao_picture;
use hevc_core::tables::*;
use hevc_core::transform::{inverse_transform, scale};
use hevc_core::{Decoder, Error, Frame};

fn decode(stream: &[u8]) -> Vec<Frame> {
    let mut d = Decoder::new();
    d.decode_annexb(stream).expect("decode");
    d.flush();
    d.take_frames()
}

fn headers(cfg: &SpsCfg, p: &PpsCfg) -> Vec<u8> {
    let mut s = vps(false);
    s.extend(sps(cfg));
    s.extend(pps(p));
    s
}

/// The coding units of the 64x48 CTB (7.3.8.4): the 64x64 CTB and its two lower 32x32 quadrants split implicitly
/// (they cross the bottom edge); (0,0) and (32,0) are 32x32 CUs, the lower ones 16x16 (their lower halves lie outside).
/// `last` codes the CU at (48,32): split into four 8x8 CUs when true.
fn all_dc_ctu(e: &mut CabacEnc, residual_in_last: bool) {
    e.split_cu(0, false); // (0,0): no left / above
    e.dc_cu(1); // MPM {planar, DC, 26}: DC is mpm_idx 1
    e.split_cu(0, false); // (32,0): left depth 1 is not > 1
    e.dc_cu(1);
    e.split_cu(0, false); // (0,32)
    e.dc_cu(1);
    e.split_cu(0, false); // (16,32)
    e.dc_cu(1);
    e.split_cu(0, false); // (32,32)
    e.dc_cu(1);
    if !residual_in_last {
        e.split_cu(0, false); // (48,32)
        e.dc_cu(1);
        return;
    }
    e.split_cu(0, true);
    for k in 0..4 {
        e.part_mode_2nx2n();
        e.prev_intra(true);
        e.mpm_idx(1);
        e.chroma_dm();
        e.cbf_chroma(0, false);
        e.cbf_chroma(0, false);
        e.cbf_luma(0, k == 3);
        if k == 3 {
            // residual_coding of an 8x8 luma TB holding one level 16 at (0,0): last position (0,0) (both prefixes 0,
            // ctxOffset 3), greater1 1 (ctxSet 0, greater1Ctx 1), greater2 1, sign 0, remaining 16 - 3 = 13.
            e.decision(LAST_X + 3, 0);
            e.decision(LAST_Y + 3, 0);
            e.decision(GT1 + 1, 1);
            e.decision(GT2, 1);
            e.bypass(0);
            e.remaining(13, 0);
        }
    }
}

fn all_dc_stream(sao_band: bool, conf_win: Option<[u32; 4]>) -> Vec<u8> {
    let cfg = SpsCfg { sao: sao_band, conf_win, ..Default::default() };
    let mut s = headers(&cfg, &PpsCfg { init_qp: 26, ..Default::default() });
    let mut e = CabacEnc::new(idr_slice_header(sao_band, false, sao_band, false), 0, 26);
    if sao_band {
        // sao(0, 0): no merge candidates; luma band offset (sao_type_idx 1: "10"), offsets 3 0 0 0, sign +, band 16.
        e.decision(SAO_TYPE, 1);
        e.bypass(0);
        e.bypass_tu(3, 7);
        for _ in 0..3 {
            e.bypass_tu(0, 7);
        }
        e.bypass(0);
        e.bypass_bits(16, 5);
    }
    all_dc_ctu(&mut e, false);
    s.extend(nal(19, &e.finish().out));
    s
}

#[test]
fn all_intra_dc_64x48_is_flat_128() {
    let f = decode(&all_dc_stream(false, None));
    assert_eq!(f.len(), 1);
    assert_eq!((f[0].width, f[0].height, f[0].poc), (64, 48, 0));
    assert!(f[0].planes[0].iter().all(|&v| v == 128));
    assert!(f[0].planes[1].iter().chain(f[0].planes[2].iter()).all(|&v| v == 128));
    assert_eq!(f[0].planes[1].len(), 32 * 24);
}

#[test]
fn conformance_window_crops_the_frame() {
    // conf_win_right_offset 4, bottom 4 (chroma units, 4:2:0): 64x48 -> 56x40.
    let f = decode(&all_dc_stream(false, Some([0, 4, 0, 4])));
    assert_eq!((f[0].width, f[0].height), (56, 40));
    assert_eq!(f[0].planes[0].len(), 56 * 40);
    assert_eq!(f[0].planes[2].len(), 28 * 20);
}

#[test]
fn sao_band_offset_lifts_band_16_by_3() {
    let f = decode(&all_dc_stream(true, None));
    // 8.7.3: bandShift 3, 128 >> 3 = band 16 = bandTable index 1 -> SaoOffsetVal[1] = 3.
    assert!(f[0].planes[0].iter().all(|&v| v == 131));
    assert!(f[0].planes[1].iter().all(|&v| v == 128));
}

/// 8.6.2 then 8.6.4.2 for one DC level in an 8x8 block at Qp'Y 26 with flat scaling (m = 16), computed directly.
fn dc_residual(level: i64) -> i32 {
    let bd_shift = 8 + 3 - 5;
    let d = (level * 16 * ((LEVEL_SCALE[26 % 6] as i64) << (26 / 6)) + (1 << (bd_shift - 1))) >> bd_shift;
    let g = ((64 * d + 64) >> 7).clamp(-32768, 32767);
    ((64 * g + (1 << 11)) >> 12) as i32
}

#[test]
fn one_dc_coefficient_in_the_last_tu() {
    let cfg = SpsCfg::default();
    let mut s = headers(&cfg, &PpsCfg { init_qp: 26, deblocking_disabled: true, ..Default::default() });
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 26);
    all_dc_ctu(&mut e, true);
    s.extend(nal(19, &e.finish().out));
    let f = decode(&s);
    let d = dc_residual(16);
    assert_eq!(d, 26);
    let y = &f[0].planes[0];
    for row in 0..48 {
        for col in 0..64 {
            let want = if (56..64).contains(&col) && (40..48).contains(&row) { 128 + d } else { 128 };
            assert_eq!(y[row * 64 + col] as i32, want, "luma ({col},{row})");
        }
    }
    assert!(f[0].planes[1].iter().all(|&v| v == 128));
}

fn pcm_y(x: usize, y: usize) -> u8 {
    (x * 7 + y * 3) as u8
}
fn pcm_cb(x: usize, y: usize) -> u8 {
    (40 + x * 3 + y) as u8
}
fn pcm_cr(x: usize, y: usize) -> u8 {
    (200 - x - 2 * y) as u8
}

#[test]
fn pcm_block_and_vertical_prediction_below_it() {
    let cfg = SpsCfg { pcm: Some((8, 8, 3, 5, true)), ..Default::default() };
    let mut s = headers(&cfg, &PpsCfg { init_qp: 26, deblocking_disabled: true, ..Default::default() });
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 26);
    // (0,0) 32x32: pcm_flag 1, pcm_alignment_zero_bits, 32x32 luma + 2 x 16x16 chroma 8-bit samples, engine restart.
    e.split_cu(0, false);
    e.terminate(1);
    e.zero_align();
    for y in 0..32 {
        for x in 0..32 {
            e.bw.u(pcm_y(x, y) as u64, 8);
        }
    }
    for f in [pcm_cb as fn(usize, usize) -> u8, pcm_cr] {
        for y in 0..16 {
            for x in 0..16 {
                e.bw.u(f(x, y) as u64, 8);
            }
        }
    }
    e.restart();
    // (32,0): pcm_flag 0, DC (A is the pcm CU: DC).
    e.split_cu(0, false);
    e.terminate(0);
    e.dc_cu(1);
    // (0,32) 16x16: pcm_flag 0, INTRA_ANGULAR26 = mpm_idx 2 of {planar, DC, 26}; chroma DM (26).
    e.split_cu(0, false);
    e.terminate(0);
    e.prev_intra(true);
    e.mpm_idx(2);
    e.chroma_dm();
    e.cbf_chroma(0, false);
    e.cbf_chroma(0, false);
    e.cbf_luma(0, false);
    // (16,32): A = 26, B = DC (pcm) -> {26, DC, planar}: DC is mpm_idx 1. (32,32), (48,32): DC.
    for _ in 0..3 {
        e.split_cu(0, false);
        e.terminate(0);
        e.dc_cu(1);
    }
    s.extend(nal(19, &e.finish().out));
    let f = decode(&s);
    let (yp, cb, cr) = (&f[0].planes[0], &f[0].planes[1], &f[0].planes[2]);
    for y in 0..32 {
        for x in 0..32 {
            assert_eq!(yp[y * 64 + x], pcm_y(x, y));
        }
    }
    for y in 0..16 {
        for x in 0..16 {
            assert_eq!(cb[y * 32 + x], pcm_cb(x, y));
            assert_eq!(cr[y * 32 + x], pcm_cr(x, y));
        }
    }
    // The left column of the mode 26 block is substituted from p[0][-1], so its edge filter adds 0: a copy.
    for y in 32..48 {
        for x in 0..16 {
            assert_eq!(yp[y * 64 + x], pcm_y(x, 31), "luma ({x},{y})");
        }
    }
    for y in 16..24 {
        for x in 0..8 {
            assert_eq!(cb[y * 32 + x], pcm_cb(x, 15));
            assert_eq!(cr[y * 32 + x], pcm_cr(x, 15));
        }
    }
}

#[test]
fn inter_slices_high_bit_depth_and_422_are_refused_by_name() {
    // A TRAIL_R P slice header (its own st_ref_pic_set: one negative picture).
    let mut s = headers(&SpsCfg::default(), &PpsCfg { init_qp: 26, ..Default::default() });
    let mut b = BitWriter::default();
    b.bit(1);
    b.ue(0);
    b.ue(1); // P
    b.u(1, 8); // slice_pic_order_cnt_lsb
    b.bit(0); // short_term_ref_pic_set_sps_flag
    b.ue(1);
    b.ue(0);
    b.ue(0);
    b.bit(1);
    b.bit(0); // num_ref_idx_active_override_flag
    b.ue(0); // five_minus_max_num_merge_cand
    b.se(0);
    b.bit(1); // slice_loop_filter_across_slices_enabled_flag
    b.trailing();
    b.u(0xff, 8);
    s.extend(nal(1, &b.out));
    // HEVCINTER (SR216) decodes P slices: this one's RPS names POC 0, which was never decoded (8.3.2).
    let err = Decoder::new().decode_annexb(&s).unwrap_err();
    assert_eq!(err, Error::Invalid("reference picture set names a missing picture (8.3.2)"));

    for (cfg, want) in [
        (SpsCfg { bit_depth: 13, ..Default::default() }, "bit depth above 12 (HEVCREXT-TOOLS)"),
        (SpsCfg { chroma_format_idc: 0, ..Default::default() }, "monochrome 4:0:0 (HEVCREXT-MONO)"),
    ] {
        let mut s = headers(&cfg, &PpsCfg { init_qp: 26, ..Default::default() });
        let hdr = idr_slice_header(false, false, false, false);
        let mut e = CabacEnc::new(hdr, 0, 26);
        all_dc_ctu(&mut e, false);
        s.extend(nal(19, &e.finish().out));
        assert_eq!(Decoder::new().decode_annexb(&s).unwrap_err(), Error::Unsupported(want));
    }
}

// ---------------------------------------------------------------- unit KATs

fn unit(w: u32, h: u32, log2_ctb: u32) -> (Sps, Pps, Geometry, Picture) {
    let sps = Sps {
        width: w,
        height: h,
        log2_ctb,
        log2_min_cb: 3,
        log2_min_tb: 2,
        log2_max_tb: log2_ctb.min(5),
        chroma_format_idc: 1,
        bit_depth_luma: 8,
        bit_depth_chroma: 8,
        ordering: vec![Ordering::default()],
        ..Default::default()
    };
    let pps = Pps { num_tile_columns: 1, num_tile_rows: 1, uniform_spacing_flag: true, loop_filter_across_tiles_enabled_flag: true, ..Default::default() };
    let geo = Geometry::new(&sps, &pps).unwrap();
    let mut pic = Picture::new(w as usize, h as usize, &geo);
    pic.slices.push(SliceInfo::default());
    for s in pic.ctb_slice.iter_mut() {
        *s = 0;
    }
    (sps, pps, geo, pic)
}

#[test]
fn inverse_transforms_kat() {
    // A DC level alone is flat at every size: d = 1024 -> g = (64 * 1024 + 64) >> 7 = 512 -> (64 * 512 + 2048) >> 12 = 8.
    for log2 in 2..=5 {
        let n = 1 << log2;
        let mut d = vec![0i32; n * n];
        d[0] = 1024;
        let r = inverse_transform(&d, log2, false, 8);
        assert!(r.iter().all(|&v| v == 8), "DCT {n}");
    }
    let mut d = vec![0i32; 16];
    d[0] = 64;
    assert!(inverse_transform(&d, 2, false, 8).iter().all(|&v| v == 1));
    // The DST basis 0 at 128: g = [29, 55, 74, 84] down column 0, then r[x][y] = (DST[0][x] * g[y] + 2048) >> 12.
    let mut d = vec![0i32; 16];
    d[0] = 128;
    let r = inverse_transform(&d, 2, true, 8);
    assert_eq!((r[0], r[3 * 4 + 3], r[3 * 4], r[3]), (0, 2, 1, 1));
    // The transMatrix rows the Recommendation prints.
    assert_eq!(&TRANS_MATRIX[1][..16], &[90, 90, 88, 85, 82, 78, 73, 67, 61, 54, 46, 38, 31, 22, 13, 4]);
    assert_eq!(TRANS_MATRIX[1][31], -90);
    assert_eq!(&TRANS_MATRIX[8][..4], &[83, 36, -36, -83]);
    assert_eq!(&TRANS_MATRIX[16][..4], &[64, -64, -64, 64]);
    assert_eq!(&TRANS_MATRIX[2][..8], &[90, 87, 80, 70, 57, 43, 25, 9]);
    assert_eq!(&TRANS_MATRIX[4][..4], &[89, 75, 50, 18]);
    // 8.6.2: 16 at Qp 26, nTbS 8: (16 * 16 * 51 << 4 + 32) >> 6 = 3264; with ScalingFactor 20: 4080.
    let mut c = vec![16i32; 64];
    scale(&mut c, 3, 26, 8, None);
    assert_eq!(c[0], 3264);
    let mut c = vec![16i32; 64];
    scale(&mut c, 3, 26, 8, Some(&[20u8; 64]));
    assert_eq!(c[0], 4080);
}

#[test]
fn intra_predictors_kat() {
    // A 4x4 block at (4,4) of a 16x16 picture (CTB 16): left p[-1][0..3] = 10 20 30 40, top p[0..3][-1] = 50 60 70 80,
    // corner 100; below-left and above-right are later in z-scan order, so they are substituted (40 and 80).
    let setup = || {
        let (_, _, geo, mut pic) = unit(16, 16, 4);
        for (i, v) in [10u16, 20, 30, 40].iter().enumerate() {
            pic.planes[0].set(3, 4 + i, *v);
        }
        for (i, v) in [50u16, 60, 70, 80].iter().enumerate() {
            pic.planes[0].set(4 + i, 3, *v);
        }
        pic.planes[0].set(3, 3, 100);
        (geo, pic)
    };
    let run = |mode: u32| {
        let (geo, mut pic) = setup();
        predict(&mut pic, &geo, 0, 4, 4, 2, mode, false, 8);
        move |x: usize, y: usize| pic.planes[0].at(4 + x, 4 + y)
    };
    let p = run(0); // planar
    assert_eq!((p(0, 0), p(3, 3), p(1, 2)), (38, 60, 50));
    let p = run(1); // DC 45 with the luma edge filter
    assert_eq!((p(0, 0), p(1, 0), p(3, 0), p(0, 3), p(2, 2)), (38, 49, 54, 44, 45));
    let p = run(18); // diagonal down-right through the projected left samples
    assert_eq!((p(0, 0), p(0, 1), p(1, 0), p(3, 3), p(0, 3)), (100, 10, 50, 100, 30));
    let p = run(2);
    assert_eq!((p(0, 0), p(3, 3), p(1, 0), p(0, 2)), (20, 40, 30, 40));
    let p = run(34);
    assert_eq!((p(0, 0), p(3, 3), p(1, 1), p(0, 1)), (60, 80, 80, 70));
    let p = run(26); // vertical with the column-0 edge filter
    assert_eq!((p(0, 0), p(0, 3), p(1, 0), p(3, 2)), (5, 20, 60, 80));
    // The mode derivations.
    assert_eq!(hevc_core::intra::luma_mode(1, 1, true, 2, 0), 26);
    assert_eq!(hevc_core::intra::luma_mode(10, 10, true, 1, 0), 2 + ((10 + 29) % 32));
    assert_eq!(hevc_core::intra::luma_mode(26, 1, false, 0, 5), 7); // sorted {0, 1, 26}: 5 -> 6 -> 7
    assert_eq!(hevc_core::intra::chroma_mode(1, 26), 34);
    assert_eq!(hevc_core::intra::chroma_mode(4, 7), 7);
}

#[test]
fn deblocking_strong_and_normal_kat() {
    for (q, want) in [
        // |p0 - q0| = 10 < (5 tC + 1) >> 1 = 13 at QP 37 (beta 36, tC 5): the strong filter.
        (70u16, [(5usize, 61u16), (6, 63), (7, 64), (8, 66), (9, 68), (10, 69)]),
        // |p0 - q0| = 20: the normal filter, Delta 5 (clipped from 8), dEp / dEq: p1 + 2, q1 - 2.
        (80u16, [(5, 60), (6, 62), (7, 65), (8, 75), (9, 78), (10, 80)]),
    ] {
        let (sps, pps, geo, mut pic) = unit(16, 16, 4);
        for y in 0..16 {
            for x in 0..16 {
                pic.planes[0].set(x, y, if x < 8 { 60 } else { q });
            }
        }
        for v in pic.qp.iter_mut() {
            *v = 37;
        }
        for y in (0..16).step_by(4) {
            let i = pic.idx4(8, y);
            pic.flags[i] |= F_EDGE_V;
        }
        deblock_picture(&mut pic, &geo, &sps, &pps);
        for y in 0..16 {
            for &(x, v) in want.iter() {
                assert_eq!(pic.planes[0].at(x, y), v, "q={q} ({x},{y})");
            }
        }
    }
}

#[test]
fn sao_edge_offset_kat() {
    let (mut sps, pps, geo, mut pic) = unit(16, 16, 4);
    sps.sample_adaptive_offset_enabled_flag = true;
    pic.slices[0].sao_luma = true;
    pic.planes[0].data.iter_mut().for_each(|v| *v = 100);
    pic.planes[0].set(5, 5, 90); // a local minimum along SaoEoClass 0: edgeIdx 0 -> 1
    pic.planes[0].set(10, 10, 110); // a local maximum: edgeIdx 4
    pic.sao[0] = SaoParams { type_idx: [2, 0, 0], offset_val: [[0, 4, 0, 0, -2], [0; 5], [0; 5]], band_position: [0; 3], eo_class: [0; 3] };
    sao_picture(&mut pic, &geo, &sps, &pps);
    assert_eq!(pic.planes[0].at(5, 5), 94);
    assert_eq!(pic.planes[0].at(10, 10), 108);
    assert_eq!(pic.planes[0].at(4, 5), 100); // edgeIdx 3 (offset 0)
    assert_eq!(pic.planes[0].at(0, 5), 100); // the picture's left column: a neighbour outside, left as is
}

/// HEVCCONF (SR321): the decoded picture hash SEI (a suffix SEI after the picture's slice) is checked against the
/// decoded, filtered, uncropped picture: the flat-128 64x48 picture with its true CRCs is a Match, with a checksum one
/// off in luma a Mismatch(0). The hash functions themselves have KATs in tests/conformance.rs.
#[test]
fn picture_hash_sei_checks_the_decoded_picture() {
    use hevc_core::picture::Plane;
    use hevc_core::sei::{plane_checksum, plane_crc, HashCheck};
    let flat = [Plane::new(64, 48, 128), Plane::new(32, 24, 128), Plane::new(32, 24, 128)];
    let checks = |payload: Vec<u8>| {
        let mut s = all_dc_stream(false, None);
        let mut rbsp = vec![132, payload.len() as u8];
        rbsp.extend(payload);
        rbsp.push(0x80);
        s.extend(nal(40, &rbsp));
        let mut d = Decoder::new();
        d.decode_annexb(&s).expect("decode");
        d.flush();
        assert_eq!(d.take_frames().len(), 1);
        d.take_hash_checks()
    };
    let mut crc = vec![1u8];
    for p in &flat {
        crc.extend(plane_crc(p, 8).to_be_bytes());
    }
    assert_eq!(checks(crc), [(0, HashCheck::Match)]);
    let mut sum = vec![2u8];
    for (c, p) in flat.iter().enumerate() {
        sum.extend((plane_checksum(p, 8) + u32::from(c == 0)).to_be_bytes());
    }
    assert_eq!(checks(sum), [(0, HashCheck::Mismatch(0))]);
}
