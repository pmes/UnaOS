// SPDX-License-Identifier: LGPL-3.0-or-later
//! HEVCREXTSCC (LEDGER SR357) KATs: screen content coding pictures assembled by hand with the test kit's CABAC encoder
//! (tests/common: the palette encoder mirrors this crate's reading of H.265 v4, so these pin the decoder to hand
//! arithmetic over that reading — self-consistent, unconformed until the bench's HEVCCONF-SCC run), each decoded
//! picture compared with a picture built by hand: (a) the adaptive colour transform of a lossless intra coding unit is
//! the inverse YCgCo-R of its three DC levels; (b) a lossy one scales its luma level at qP + PpsActQpOffsetY +
//! SliceActQpOffsetY and spreads it into all three components; (c) intra block copy from the left with integer vectors
//! (motion_vector_resolution_control_idc 1) copies a pcm block; (d) a vector into the undecoded region is refused by
//! name; (e) a 4:4:4 palette coding unit with two entries, index and copy-above runs and one escape at QP 4 (levelScale
//! 64: the escape is its value), then a second unit reusing predictor entry 1 (the predictor update); (f) the same map at
//! 4:2:0 (chroma from the even luma positions, the chroma escape at an even position); (g) the palette predictor
//! synchronised across a wavefront row; plus every SCC flag present and off decodes as the stream without them.

mod common;

use common::*;
use hevc_core::tables::*;
use hevc_core::{Decoder, Error, Frame};

fn decode(stream: &[u8]) -> Result<Vec<Frame>, Error> {
    let mut d = Decoder::new();
    d.decode_annexb(stream)?;
    d.flush();
    Ok(d.take_frames())
}

fn hdr(cfg: &SpsCfg, x: &InterSps, s: &SpsScc, p: &PpsCfg, r: &PpsRext, ps: &PpsScc) -> Vec<u8> {
    let mut v = vps(false);
    v.extend(nal(33, &sps_rbsp_scc(cfg, x, Some(s))));
    v.extend(nal(34, &pps_rbsp_scc(p, &PpsInter::default(), r, Some(ps))));
    v
}

fn pps_qp(init_qp: i32) -> PpsCfg {
    PpsCfg { init_qp, deblocking_disabled: true, ..Default::default() }
}

/// One DC level `level` (|level| >= 3) of a residual_coding() whose last position is (0, 0) at last ctxOffset `off`:
/// greater1 1, greater2 1, the sign, then coeff_abs_level_remaining |level| - 3.
fn dc(e: &mut CabacEnc, off: usize, chroma: bool, level: i32) {
    e.decision(LAST_X + off, 0);
    e.decision(LAST_Y + off, 0);
    e.decision(GT1 + if chroma { 16 } else { 0 } + 1, 1);
    e.decision(GT2 + if chroma { 4 } else { 0 }, 1);
    e.bypass((level < 0) as u32);
    e.remaining(level.unsigned_abs() - 3, 0);
}

/// 8.6.2 then the 8x8 DCT (8.6.4.2) of one DC level at qP `qp`, flat scaling, 8 bits: the flat residual.
fn dc_residual_8x8(level: i64, qp: i64) -> i32 {
    let bd_shift = 8 + 3 - 5;
    let d = ((level * 16 * ((LEVEL_SCALE[(qp % 6) as usize] as i64) << (qp / 6)) + (1 << (bd_shift - 1))) >> bd_shift).clamp(-32768, 32767);
    let g = ((64 * d + 64) >> 7).clamp(-32768, 32767);
    ((64 * g + 2048) >> 12) as i32
}

/// The 8x8 4:4:4 intra coding unit's head: (cu_transquant_bypass_flag), 2Nx2N, INTRA_DC by mpm_idx 1, chroma DM.
fn act_cu_head(e: &mut CabacEnc, bypass: bool) {
    if bypass {
        e.decision(TRANSQUANT_BYPASS, 1);
    }
    e.part_mode_2nx2n();
    e.prev_intra(true);
    e.mpm_idx(1);
    e.chroma_dm();
}

fn cfg444(w: u32, h: u32) -> SpsCfg {
    SpsCfg { width: w, height: h, log2_ctb: 4, log2_max_tb: 3, chroma_format_idc: 3, ..Default::default() }
}

#[test]
fn a_act_lossless_is_the_inverse_ycgco_r_of_the_three_levels() {
    // DC levels (rY, rCg, rCo) = (10, 6, -4) in a transquant-bypass CU: tmp = 10 - (6 >> 1) = 7; G = 7 + 6 = 13;
    // B = 7 - (-4 >> 1) = 9; R = 9 + (-4) = 5 at (0, 0) of Y, Cb, Cr over the DC prediction 128.
    let r = PpsRext { transquant_bypass: true, ..Default::default() };
    let mut s = hdr(&cfg444(8, 8), &InterSps::default(), &SpsScc::default(), &pps_qp(26), &r, &PpsScc { act: Some((false, [0; 3])), ..Default::default() });
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 26);
    act_cu_head(&mut e, true);
    e.cbf_chroma(0, true);
    e.cbf_chroma(0, true);
    e.cbf_luma(0, true);
    e.decision(TU_ACT, 1);
    dc(&mut e, 3, false, 10);
    dc(&mut e, 15, true, 6);
    dc(&mut e, 15, true, -4);
    s.extend(nal(19, &e.finish().out));
    let f = &decode(&s).expect("decode")[0];
    for (c, want) in [(0, 141), (1, 137), (2, 133)] {
        for i in 0..64 {
            assert_eq!(f.planes[c][i], if i == 0 { want } else { 128 }, "plane {c} sample {i}");
        }
    }
}

#[test]
fn b_act_lossy_adds_the_pps_and_slice_qp_offsets_to_the_luma_scale() {
    // PpsActQpOffsetY 3 + slice_act_y_qp_offset 3: the luma level 3 is scaled at qP 32 (r 10; at 26 it would be 5); with
    // rCg = rCo = 0 the inverse YCgCo-R puts r into G, B and R alike.
    let ps = PpsScc { act: Some((true, [3, 0, 0])), ..Default::default() };
    let mut s = hdr(&cfg444(8, 8), &InterSps::default(), &SpsScc::default(), &pps_qp(26), &PpsRext::default(), &ps);
    let sl = SliceCfg { nal_type: 19, slice_type: 2, deblocking_disabled: true, ..Default::default() };
    let mut e = CabacEnc::new(slice_header_scc(&sl, &SliceScc { act_qp_offset: Some([3, 0, 0]), ..Default::default() }), 0, 26);
    act_cu_head(&mut e, false);
    e.cbf_chroma(0, false);
    e.cbf_chroma(0, false);
    e.cbf_luma(0, true);
    e.decision(TU_ACT, 1);
    dc(&mut e, 3, false, 3);
    s.extend(nal(19, &e.finish().out));
    let r = dc_residual_8x8(3, 32);
    assert_eq!((r, dc_residual_8x8(3, 26)), (10, 5));
    let f = &decode(&s).expect("decode")[0];
    for c in 0..3 {
        assert!(f.planes[c].iter().all(|&v| v as i32 == 128 + r), "plane {c}: {:?}", &f.planes[c][..8]);
    }
}

/// A 32x16 4:2:0 IDR picture of one P slice whose only reference is the current picture: a 16x16 pcm block at x = 0 (luma
/// (x * 13 + y * 7) & 255, chroma patterns), then a 2Nx2N AMVP coding unit at x = 16 with MvdL0 `mvd` in integer samples
/// (motion_vector_resolution_control_idc 1) and no residual.
fn ibc_stream(mvd: [i32; 2]) -> (Vec<u8>, [Vec<u8>; 3]) {
    let cfg = SpsCfg { width: 32, height: 16, log2_ctb: 4, log2_max_tb: 4, pcm: Some((8, 8, 3, 4, true)), ..Default::default() };
    let scc = SpsScc { curr_pic_ref: true, mv_res_idc: 1, ..Default::default() };
    let mut s = hdr(&cfg, &InterSps { max_dec_pic_buffering_minus1: 1, ..Default::default() }, &scc, &pps_qp(26), &PpsRext::default(), &PpsScc { curr_pic_ref: true, ..Default::default() });
    let y: Vec<u16> = (0..256).map(|i| (((i % 16) * 13 + (i / 16) * 7) & 255) as u16).collect();
    let cb: Vec<u16> = (0..64).map(|i| (40 + (i % 8) * 9 + (i / 8) * 3) as u16).collect();
    let cr: Vec<u16> = (0..64).map(|i| (200 - (i % 8) * 5 - (i / 8) * 11) as u16).collect();
    let sl = SliceCfg { nal_type: 19, slice_type: 1, five_minus_max_num_merge_cand: 4, deblocking_disabled: true, ..Default::default() };
    let mut e = CabacEnc::new(slice_header(&sl), 1, 26);
    e.split_cu(0, false);
    e.cu_skip(0, false);
    e.decision(PRED_MODE, 1);
    e.pcm_samples_fmt(4, 8, 8, &y, &cb, &cr, (2, 2));
    e.terminate(0);
    e.split_cu(0, false);
    e.cu_skip(0, false);
    e.pred_inter();
    e.part_mode_2nx2n();
    e.merge_flag(false);
    e.mvd(mvd);
    e.mvp_flag(0);
    e.rqt_root_cbf(false);
    s.extend(nal(19, &e.finish().out));
    // The picture the copy from the left makes.
    let mut want = [vec![0u8; 32 * 16], vec![0u8; 16 * 8], vec![0u8; 16 * 8]];
    for (c, (src, n)) in [(&y, 16usize), (&cb, 8), (&cr, 8)].into_iter().enumerate() {
        for yy in 0..n {
            for xx in 0..n {
                let v = src[yy * n + xx] as u8;
                want[c][yy * 2 * n + xx] = v;
                want[c][yy * 2 * n + n + xx] = v;
            }
        }
    }
    (s, want)
}

#[test]
fn c_intra_block_copy_from_the_left_with_integer_vectors() {
    // MvdL0 (-16, 0) in integer samples: mvL0 (-64, 0) quarter samples (no neighbour vector, mvp 0); the chroma vector
    // (-64, 0) eighth samples at 4:2:0 is the whole 8x8 chroma block on the left.
    let (s, want) = ibc_stream([-16, 0]);
    let f = &decode(&s).expect("decode")[0];
    for c in 0..3 {
        assert_eq!(f.planes[c], want[c], "plane {c}");
    }
}

#[test]
fn d_intra_block_copy_into_the_undecoded_region_is_refused_by_name() {
    // mvL0 (0, 0): the reference block is the coding unit itself, whose bottom-right sample is not decoded yet.
    let (s, _) = ibc_stream([0, 0]);
    match decode(&s) {
        Err(Error::Unsupported(m)) => assert!(m.contains("HEVCREXT-SCC"), "{m}"),
        other => panic!("expected a refusal by name, got {:?}", other.map(|f| f.len())),
    }
}

const E0: [u16; 3] = [10, 20, 30];
const E1: [u16; 3] = [200, 100, 50];
const ESC: [u32; 3] = [77, 88, 99];

/// The 8x8 index map of (e) / (f): rows 0..3 index 0, rows 4..7 index 1, the escape (index 2) at (ex, 6).
fn pal_map(ex: usize) -> Vec<u8> {
    (0..64).map(|i| if i == 6 * 8 + ex { 2 } else if i / 8 < 4 { 0 } else { 1 }).collect()
}

fn run(copy: bool, len: usize) -> PalRun {
    PalRun { copy, len }
}

fn pal_sps(w: u32, h: u32, idc: u32, log2_max_tb: u32) -> (SpsCfg, SpsScc) {
    let cfg = SpsCfg { width: w, height: h, log2_ctb: 4, log2_max_tb, chroma_format_idc: idc, ..Default::default() };
    (cfg, SpsScc { palette: Some((4, 4, None)), ..Default::default() })
}

#[test]
fn e_palette_444_runs_escape_and_the_predictor_update() {
    // CU 0 (x 0): new entries E0, E1 and escape; runs in traverse scan: index 0 x 32, index 1 x 8 (row 4), copy-above x 11
    // (row 5 and row 6 x 0..2), the escape at (3, 6), index 1 to the end. QP 4: (v * 64 + 32) >> 6 = v. CU 1 (x 8):
    // palette_predictor_run 2 reuses predictor entry 1 (E1; the predictor after CU 0 is [E0, E1]), no new entry, no escape.
    let (cfg, scc) = pal_sps(16, 8, 3, 3);
    let mut s = hdr(&cfg, &InterSps::default(), &scc, &pps_qp(4), &PpsRext::default(), &PpsScc::default());
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 4);
    let map = pal_map(3);
    e.decision(PALETTE_MODE, 1);
    e.palette_coding(&PalCu {
        log2: 3,
        max_size: 4,
        pred_size: 0,
        reuse: &[],
        new: &[E0, E1],
        escape: true,
        map: &map,
        runs: &[run(false, 32), run(false, 8), run(true, 11), run(false, 1), run(false, 12)],
        transpose: false,
        esc: &[(6 * 8 + 3, ESC)],
        bypass_cu: false,
        sub: (1, 1),
        bit_depth: 8,
    });
    e.decision(PALETTE_MODE, 1);
    e.palette_coding(&PalCu {
        log2: 3,
        max_size: 4,
        pred_size: 2,
        reuse: &[1],
        new: &[],
        escape: false,
        map: &[0; 64],
        runs: &[run(false, 64)],
        transpose: false,
        esc: &[],
        bypass_cu: false,
        sub: (1, 1),
        bit_depth: 8,
    });
    s.extend(nal(19, &e.finish().out));
    let f = &decode(&s).expect("decode")[0];
    for c in 0..3 {
        for y in 0..8 {
            for x in 0..16 {
                let want = if x >= 8 {
                    E1[c]
                } else {
                    match map[y * 8 + x] {
                        0 => E0[c],
                        1 => E1[c],
                        _ => ESC[c] as u16,
                    }
                };
                assert_eq!(f.planes[c][y * 16 + x] as u16, want, "plane {c} ({x}, {y})");
            }
        }
    }
}

#[test]
fn f_palette_420_takes_chroma_from_the_even_luma_positions() {
    // The map of (e) with the escape at (2, 6): every chroma sample (x, y) takes the index at luma (2x, 2y), so chroma
    // (1, 3) is the escape and its Cb / Cr escape values are coded (x and y even).
    let (cfg, scc) = pal_sps(8, 8, 1, 3);
    let mut s = hdr(&cfg, &InterSps::default(), &scc, &pps_qp(4), &PpsRext::default(), &PpsScc::default());
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 4);
    let map = pal_map(2);
    e.decision(PALETTE_MODE, 1);
    e.palette_coding(&PalCu {
        log2: 3,
        max_size: 4,
        pred_size: 0,
        reuse: &[],
        new: &[E0, E1],
        escape: true,
        map: &map,
        runs: &[run(false, 32), run(false, 8), run(true, 10), run(false, 1), run(false, 13)],
        transpose: false,
        esc: &[(6 * 8 + 2, ESC)],
        bypass_cu: false,
        sub: (2, 2),
        bit_depth: 8,
    });
    s.extend(nal(19, &e.finish().out));
    let f = &decode(&s).expect("decode")[0];
    let pick = |c: usize, i: u8| match i {
        0 => E0[c],
        1 => E1[c],
        _ => ESC[c] as u16,
    };
    for i in 0..64 {
        assert_eq!(f.planes[0][i] as u16, pick(0, map[i]), "luma {i}");
    }
    for c in 1..3 {
        for y in 0..4 {
            for x in 0..4 {
                assert_eq!(f.planes[c][y * 4 + x] as u16, pick(c, map[2 * y * 8 + 2 * x]), "plane {c} ({x}, {y})");
            }
        }
    }
}

#[test]
fn g_the_palette_predictor_follows_the_wavefront_sync() {
    // 3x2 CTBs of 16 with entropy_coding_sync, one 16x16 palette coding unit each, no escape. Row 0: CTU 0 new A; CTU 1
    // no reuse, new B — the predictor stored after CTU 1 is [B, A]; CTU 2 no reuse, new C (the row ends on [C, B, A]).
    // Row 1, synchronised from the storage: CTU 3 reuses entry 0 (B; C if the row-0 predictor ran on, nothing if
    // initialised), CTU 4 entry 1 of [B, A] (A), CTU 5 entry 0 of [A, B] (A).
    const A: [u16; 3] = [40, 50, 60];
    const B: [u16; 3] = [220, 30, 140];
    const C: [u16; 3] = [90, 170, 15];
    let (cfg, scc) = pal_sps(48, 32, 3, 4);
    let mut s = hdr(&cfg, &InterSps::default(), &scc, &pps_qp(26), &PpsRext::default(), &PpsScc { wpp: true, ..Default::default() });
    let flat = [0u8; 256];
    let one = [run(false, 256)];
    let cu = |pred_size: usize, reuse: &'static [usize], new: &'static [[u16; 3]]| (pred_size, reuse, new);
    let ctus = [cu(0, &[], &[A]), cu(1, &[], &[B]), cu(2, &[], &[C]), cu(2, &[0], &[]), cu(2, &[1], &[]), cu(2, &[0], &[])];
    let mut e = CabacEnc::new(BitWriter::default(), 0, 26);
    let mut first_len = 0;
    let mut wpp_state = e.state;
    for (k, &(pred_size, reuse, new)) in ctus.iter().enumerate() {
        e.split_cu(0, false);
        e.decision(PALETTE_MODE, 1);
        e.palette_coding(&PalCu {
            log2: 4,
            max_size: 4,
            pred_size,
            reuse,
            new,
            escape: false,
            map: &flat,
            runs: &one,
            transpose: false,
            esc: &[],
            bypass_cu: false,
            sub: (1, 1),
            bit_depth: 8,
        });
        if k < 5 {
            e.terminate(0); // end_of_slice_segment_flag
        }
        if k == 1 {
            wpp_state = e.state; // 9.3.2.4: the contexts stored after the row's second CTU
        }
        if k == 2 {
            e.terminate(1); // end_of_subset_one_bit, then byte_alignment()
            e.zero_align();
            first_len = e.bw.out.len();
            e.restart();
            e.state = wpp_state; // row 1 starts from the stored contexts (9.3.1)
        }
    }
    let data = e.finish().out;
    let sl = SliceCfg { nal_type: 19, slice_type: 2, deblocking_disabled: true, ..Default::default() };
    let mut rbsp = slice_header_scc(&sl, &SliceScc { entry_points: Some((15, vec![first_len as u32 - 1])), ..Default::default() }).out;
    rbsp.extend(data);
    s.extend(nal(19, &rbsp));
    let f = &decode(&s).expect("decode")[0];
    for c in 0..3 {
        for y in 0..32 {
            for x in 0..48 {
                let want = match (x / 16, y / 16) {
                    (0, 0) | (1, 1) | (2, 1) => A[c],
                    (2, 0) => C[c],
                    _ => B[c],
                };
                assert_eq!(f.planes[c][y * 48 + x] as u16, want, "plane {c} ({x}, {y})");
            }
        }
    }
}

#[test]
fn every_scc_flag_present_and_off_decodes_as_the_stream_without_them() {
    // An SPS and a PPS carrying sps_scc_extension() / pps_scc_extension() with every tool off (the PPS with an empty
    // initializer list) decode one INTRA_DC picture as the same stream without the extensions does.
    let cfg = SpsCfg { width: 16, height: 16, log2_ctb: 4, log2_max_tb: 4, ..Default::default() };
    let body = || {
        let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 26);
        e.split_cu(0, false);
        e.dc_cu_fmt(1, 1);
        nal(19, &e.finish().out)
    };
    let mut plain = vps(false);
    plain.extend(nal(33, &sps_rbsp_ext(&cfg, &InterSps::default())));
    plain.extend(nal(34, &pps_rbsp_rext(&pps_qp(26), &PpsInter::default(), &PpsRext::default())));
    plain.extend(body());
    let mut scc = hdr(&cfg, &InterSps::default(), &SpsScc::default(), &pps_qp(26), &PpsRext::default(), &PpsScc { initializers: Some(vec![]), ..Default::default() });
    scc.extend(body());
    let (a, b) = (decode(&plain).expect("plain"), decode(&scc).expect("scc"));
    assert_eq!(a[0].planes, b[0].planes);
    assert!(b[0].planes.iter().all(|p| p.iter().all(|&v| v == 128)));
}
