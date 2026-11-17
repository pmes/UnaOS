// SPDX-License-Identifier: LGPL-3.0-or-later
//! HEVCINTER (SR216) KATs: P and B pictures hand-assembled through the test CABAC encoder over 32x32 pictures (CTB 16,
//! minimum CB 8) whose I pictures are pcm coding units (any content; pcm_loop_filter_disabled_flag keeps them
//! unfiltered), each decoded picture compared byte for byte with a picture built by hand:
//! M1 the reference picture lists of I0 P1 P2 P3 (8.3.4, with ref_pic_lists_modification) and of a B picture;
//! (a) skip-only P pictures equal their reference; (b) a 16x16 AMVP prediction unit with an integer mv, the other CTBs
//! merged from it, is the reference shifted with edge clamping; (c) half-sample and quarter-sample (2-D) mvs on a
//! gradient equal the 8-tap / 4-tap equations of 8.5.3.3.3 (and, inside, the linear ramp's closed form); (d) a B
//! picture bi-predicting I0 and I8 is (a + b + 1) >> 1; (e) TMVP: a skip CU takes the collocated mv scaled by the POC
//! distances (td 1, tb 2: x2); (f) a PU edge with mvs 0 and 4 samples apart is deblocked at bS 1 (only p0 / q0 move,
//! by tC 1) and the bS 0 edges and chroma are untouched; (g) explicit weighted prediction; (h) the inter residual.

mod common;

use common::*;
use hevc_core::tables::{CHROMA_FILTER, LUMA_FILTER};
use hevc_core::{Decoder, Frame};

const W: usize = 32;

#[derive(Clone)]
struct Img {
    y: Vec<u8>,
    cb: Vec<u8>,
    cr: Vec<u8>,
}

impl Img {
    fn new(fy: impl Fn(usize, usize) -> u8, fcb: impl Fn(usize, usize) -> u8, fcr: impl Fn(usize, usize) -> u8) -> Img {
        let plane = |n: usize, f: &dyn Fn(usize, usize) -> u8| (0..n * n).map(|i| f(i % n, i / n)).collect::<Vec<u8>>();
        Img { y: plane(W, &fy), cb: plane(W / 2, &fcb), cr: plane(W / 2, &fcr) }
    }
    /// The samples of the 16x16 luma block at CTB (cx, cy) and its 8x8 chroma blocks.
    fn ctb(&self, cx: usize, cy: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let blk = |p: &[u8], n: usize, s: usize, x0: usize, y0: usize| {
            (0..s * s).map(|i| p[(y0 + i / s) * n + x0 + i % s]).collect::<Vec<u8>>()
        };
        (blk(&self.y, W, 16, cx * 16, cy * 16), blk(&self.cb, W / 2, 8, cx * 8, cy * 8), blk(&self.cr, W / 2, 8, cx * 8, cy * 8))
    }
    fn planes(&self) -> [&Vec<u8>; 3] {
        [&self.y, &self.cb, &self.cr]
    }
}

fn gradient() -> Img {
    Img::new(|x, y| (x * 5 + y * 3) as u8, |x, y| (40 + x * 3 + y) as u8, |x, y| (200 - x - 2 * y) as u8)
}

fn sps_cfg() -> SpsCfg {
    SpsCfg { width: 32, height: 32, log2_ctb: 4, log2_max_tb: 4, pcm: Some((8, 8, 3, 4, true)), ..Default::default() }
}

fn inter_sps(tmvp: bool) -> InterSps {
    InterSps { max_dec_pic_buffering_minus1: 4, temporal_mvp: tmvp, ..Default::default() }
}

fn headers(tmvp: bool, deblocking_disabled: bool, pi: &PpsInter) -> Vec<u8> {
    let mut s = vps(false);
    s.extend(nal(33, &sps_rbsp_ext(&sps_cfg(), &inter_sps(tmvp))));
    s.extend(nal(34, &pps_rbsp_ext(&PpsCfg { init_qp: 26, deblocking_disabled, ..Default::default() }, pi)));
    s
}

/// The slice data of a pcm picture: the four CTBs, each one 16x16 pcm coding unit.
fn pcm_picture(mut e: CabacEnc, img: &Img) -> Vec<u8> {
    for k in 0..4 {
        let (y, cb, cr) = img.ctb(k % 2, k / 2);
        e.pcm_cu(0, 4, &y, &cb, &cr);
        if k < 3 {
            e.terminate(0);
        }
    }
    e.finish().out
}

/// An IDR pcm picture (POC 0).
fn idr(img: &Img, deblocking_disabled: bool) -> Vec<u8> {
    let e = CabacEnc::new(idr_slice_header(false, false, false, deblocking_disabled), 0, 26);
    nal(19, &pcm_picture(e, img))
}

/// initType of a slice_type with cabac_init_flag 0: P 1, B 2.
fn init_type(slice_type: u32) -> usize {
    if slice_type == 1 { 1 } else { 2 }
}

fn decode_all(stream: &[u8]) -> Vec<Frame> {
    let mut d = Decoder::new();
    d.decode_annexb(stream).expect("decode");
    d.flush();
    d.take_frames()
}

fn by_poc(f: &[Frame], poc: i32) -> &Frame {
    f.iter().find(|f| f.poc == poc).expect("frame of that POC")
}

fn assert_frame(f: &Frame, want: &Img, what: &str) {
    for (c, (got, want)) in f.planes.iter().zip(want.planes()).enumerate() {
        let n = if c == 0 { W } else { W / 2 };
        for i in 0..n * n {
            assert_eq!(got[i], want[i], "{what}: plane {c} sample ({}, {})", i % n, i / n);
        }
    }
}

/// The four CTBs as skip coding units with MaxNumMergeCand 1 (cu_skip_flag ctxInc 0, 1, 1, 2: the left / above
/// neighbours were skipped).
fn skip_picture(mut e: CabacEnc) -> Vec<u8> {
    for (k, inc) in [0usize, 1, 1, 2].into_iter().enumerate() {
        e.split_cu(0, false);
        e.cu_skip(inc, true);
        if k < 3 {
            e.terminate(0);
        }
    }
    e.finish().out
}

/// CTB 0 a 2Nx2N AMVP prediction unit (no neighbours, no TMVP: mvp 0, so mv = mvd), no residual; CTBs 1..3 skip
/// coding units (MaxNumMergeCand 1) whose first merge candidate is CTB 0's motion (A1, B1, then CTB 2 by A1); the
/// whole picture moves by `mvd`. cu_skip_flag ctxInc: 0 (CTB 0), 0 (CTB 1: CTB 0 not skipped), 0 (CTB 2), 2.
fn amvp_then_merge(mut e: CabacEnc, mvd: [i32; 2]) -> Vec<u8> {
    e.split_cu(0, false);
    e.cu_skip(0, false);
    e.pred_inter();
    e.part_mode_2nx2n();
    e.merge_flag(false);
    e.mvd(mvd);
    e.mvp_flag(0);
    e.rqt_root_cbf(false);
    for (inc, last) in [(0usize, false), (0, false), (2, true)] {
        e.terminate(0);
        e.split_cu(0, false);
        e.cu_skip(inc, true);
        if last {
            break;
        }
    }
    e.finish().out
}

fn p_slice(poc: u32, neg: Vec<(i32, bool)>) -> SliceCfg {
    SliceCfg { nal_type: 1, slice_type: 1, poc_lsb: poc, neg, five_minus_max_num_merge_cand: 4, deblocking_disabled: true, ..Default::default() }
}

fn picture(cfg: &SliceCfg, body: impl FnOnce(CabacEnc) -> Vec<u8>) -> Vec<u8> {
    nal(cfg.nal_type, &body(CabacEnc::new(slice_header(cfg), init_type(cfg.slice_type), 26)))
}

/// The reference shifted by a full-sample luma vector (dx, dy) (chroma by (dx / 2, dy / 2)), edges clamped.
fn shifted(r: &Img, dx: i32, dy: i32) -> Img {
    let at = |p: &[u8], n: usize, x: usize, y: usize, dx: i32, dy: i32| {
        let xs = (x as i32 + dx).clamp(0, n as i32 - 1) as usize;
        let ys = (y as i32 + dy).clamp(0, n as i32 - 1) as usize;
        p[ys * n + xs]
    };
    Img::new(
        |x, y| at(&r.y, W, x, y, dx, dy),
        |x, y| at(&r.cb, W / 2, x, y, dx / 2, dy / 2),
        |x, y| at(&r.cr, W / 2, x, y, dx / 2, dy / 2),
    )
}

#[test]
fn m1_reference_lists_and_skip_only_p_pictures() {
    let i0 = gradient();
    let mut s = headers(false, true, &PpsInter { lists_modification: true, ..Default::default() });
    s.extend(idr(&i0, true));
    let p1 = p_slice(1, vec![(-1, true)]);
    let p2 = SliceCfg { num_ref: Some((2, 1)), lists_mod: true, ..p_slice(2, vec![(-1, true), (-2, true)]) };
    let p3 = SliceCfg { num_ref: Some((2, 1)), lists_mod: true, list_entry_l0: Some(vec![1, 0]), ..p_slice(3, vec![(-1, true), (-2, true)]) };
    let mut d = Decoder::new();
    d.decode_annexb(&s).unwrap();
    let mut want_l0 = Vec::new();
    for (cfg, l0) in [(p1, vec![0]), (p2, vec![1, 0]), (p3, vec![1, 2])] {
        d.decode_annexb(&picture(&cfg, skip_picture)).unwrap();
        let lists = d.slice_ref_lists();
        assert_eq!(lists.len(), 1);
        assert_eq!(lists[0][0].poc, l0, "RefPicList0 of POC {}", cfg.poc_lsb);
        assert!(lists[0][1].is_empty());
        assert!(lists[0][0].long_term.iter().all(|&l| !l));
        want_l0.push(l0);
    }
    d.flush();
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 1, 2, 3]);
    // (a) every skip-only picture is its reference (zero merge candidate, refIdx 0, mv 0), byte for byte.
    for fr in &f {
        assert_frame(fr, &i0, "skip-only P");
    }
}

#[test]
fn integer_mv_prediction_unit_copies_the_shifted_block() {
    let i0 = gradient();
    let mut s = headers(false, true, &PpsInter::default());
    s.extend(idr(&i0, true));
    // mvd (32, 16) quarter samples = (8, 4) luma samples, (4, 2) chroma samples.
    s.extend(picture(&p_slice(1, vec![(-1, true)]), |e| amvp_then_merge(e, [32, 16])));
    let f = decode_all(&s);
    assert_frame(by_poc(&f, 1), &shifted(&i0, 8, 4), "integer mv");
}

/// 8.5.3.3.3.1 written out for one luma sample of the reference (8-bit: shift1 0, shift2 6, shift3 6).
fn hand_luma(r: &[u8], x: i32, y: i32, mv: [i32; 2]) -> i32 {
    let s = |x: i32, y: i32| r[(y.clamp(0, 31) * 32 + x.clamp(0, 31)) as usize] as i32;
    let (xf, yf) = ((mv[0] & 3) as usize, (mv[1] & 3) as usize);
    let (xi, yi) = (x + (mv[0] >> 2), y + (mv[1] >> 2));
    let h = |yy: i32| (0..8).map(|i| LUMA_FILTER[xf][i] * s(xi + i as i32 - 3, yy)).sum::<i32>();
    if yf == 0 {
        if xf == 0 { s(xi, yi) << 6 } else { h(yi) }
    } else if xf == 0 {
        (0..8).map(|i| LUMA_FILTER[yf][i] * s(xi, yi + i as i32 - 3)).sum()
    } else {
        (0..8).map(|n| LUMA_FILTER[yf][n] * h(yi + n as i32 - 3)).sum::<i32>() >> 6
    }
}

/// 8.5.3.3.3.2 for one chroma sample (eighth-sample mv).
fn hand_chroma(r: &[u8], x: i32, y: i32, mv: [i32; 2]) -> i32 {
    let s = |x: i32, y: i32| r[(y.clamp(0, 15) * 16 + x.clamp(0, 15)) as usize] as i32;
    let (xf, yf) = ((mv[0] & 7) as usize, (mv[1] & 7) as usize);
    let (xi, yi) = (x + (mv[0] >> 3), y + (mv[1] >> 3));
    let h = |yy: i32| (0..4).map(|i| CHROMA_FILTER[xf][i] * s(xi + i as i32 - 1, yy)).sum::<i32>();
    if yf == 0 {
        if xf == 0 { s(xi, yi) << 6 } else { h(yi) }
    } else if xf == 0 {
        (0..4).map(|i| CHROMA_FILTER[yf][i] * s(xi, yi + i as i32 - 1)).sum()
    } else {
        (0..4).map(|n| CHROMA_FILTER[yf][n] * h(yi + n as i32 - 1)).sum::<i32>() >> 6
    }
}

/// Uni-prediction with default weights (8.5.3.3.4.2: (predSamples + 32) >> 6, clipped).
fn hand_mc(r: &Img, mv: [i32; 2]) -> Img {
    let uni = |v: i32| ((v + 32) >> 6).clamp(0, 255) as u8;
    Img::new(
        |x, y| uni(hand_luma(&r.y, x as i32, y as i32, mv)),
        |x, y| uni(hand_chroma(&r.cb, x as i32, y as i32, mv)),
        |x, y| uni(hand_chroma(&r.cr, x as i32, y as i32, mv)),
    )
}

#[test]
fn fractional_mvs_follow_the_8_tap_and_4_tap_filters() {
    let i0 = gradient();
    let mut s = headers(false, true, &PpsInter::default());
    s.extend(idr(&i0, true));
    // P1: the half-sample mv (2, 0); P2 (its RPS keeps I0 only): the quarter-sample 2-D mv (1, 3).
    s.extend(picture(&p_slice(1, vec![(-1, true)]), |e| amvp_then_merge(e, [2, 0])));
    s.extend(picture(&p_slice(2, vec![(-2, true)]), |e| amvp_then_merge(e, [1, 3])));
    let f = decode_all(&s);
    let half = by_poc(&f, 1);
    assert_frame(half, &hand_mc(&i0, [2, 0]), "half-sample mv");
    // Inside the picture the half-sample position of the ramp 5x + 3y is (64 f + 5 * 32 + 32) >> 6 = f + 3; of Cb
    // (slope 3, eighth-sample phase 2: fC sum of i * c = 16) (64 v + 48 + 32) >> 6 = v + 1; of Cr (slope -1) v.
    for y in 0..32 {
        for x in 3..=27 {
            assert_eq!(half.planes[0][y * 32 + x] as usize, 5 * x + 3 * y + 3, "ramp ({x},{y})");
        }
    }
    for y in 0..16 {
        for x in 1..=13 {
            assert_eq!(half.planes[1][y * 16 + x] as usize, 40 + 3 * x + y + 1);
            assert_eq!(half.planes[2][y * 16 + x] as usize, 200 - x - 2 * y);
        }
    }
    assert_frame(by_poc(&f, 2), &hand_mc(&i0, [1, 3]), "quarter-sample 2-D mv");
}

#[test]
fn b_picture_bi_predicts_the_rounded_average() {
    let a = gradient();
    let b = Img::new(|x, y| (250 - x * 2 - y * 5) as u8, |x, y| (10 + x * 7 + y * 2) as u8, |x, y| (90 + x + y * 4) as u8);
    let mut s = headers(false, true, &PpsInter::default());
    s.extend(idr(&a, true));
    // I8: a TRAIL_R I picture keeping I0 (used_by_curr_pic 0).
    let i8cfg = SliceCfg { nal_type: 1, slice_type: 2, poc_lsb: 8, neg: vec![(-8, false)], deblocking_disabled: true, ..Default::default() };
    s.extend(nal(1, &pcm_picture(CabacEnc::new(slice_header(&i8cfg), 0, 26), &b)));
    // B4: StCurrBefore {0}, StCurrAfter {8}, two active references per list.
    let bcfg = SliceCfg {
        nal_type: 1,
        slice_type: 0,
        poc_lsb: 4,
        neg: vec![(-4, true)],
        pos: vec![(4, true)],
        num_ref: Some((2, 2)),
        five_minus_max_num_merge_cand: 4,
        deblocking_disabled: true,
        ..Default::default()
    };
    let mut d = Decoder::new();
    d.decode_annexb(&s).unwrap();
    d.decode_annexb(&picture(&bcfg, |mut e| {
        // CTB 0: PRED_BI, ref_idx 0 in both lists, zero mvds (mvp 0); CTBs 1..3 merge its bi-prediction.
        e.split_cu(0, false);
        e.cu_skip(0, false);
        e.pred_inter();
        e.part_mode_2nx2n();
        e.merge_flag(false);
        e.pred_bi(0);
        for _ in 0..2 {
            e.ref_idx(0, 2);
            e.mvd([0, 0]);
            e.mvp_flag(0);
        }
        e.rqt_root_cbf(false);
        for (inc, last) in [(0usize, false), (0, false), (2, true)] {
            e.terminate(0);
            e.split_cu(0, false);
            e.cu_skip(inc, true);
            if last {
                break;
            }
        }
        e.finish().out
    }))
    .unwrap();
    let lists = d.slice_ref_lists();
    assert_eq!((lists[0][0].poc.clone(), lists[0][1].poc.clone()), (vec![0, 8], vec![8, 0]));
    d.flush();
    let f = d.take_frames();
    // 8.5.3.3.4.2: (a << 6 + b << 6 + 64) >> 7 = (a + b + 1) >> 1.
    let avg = |p: &[u8], q: &[u8], i: usize| ((p[i] as u32 + q[i] as u32 + 1) >> 1) as u8;
    let want = Img {
        y: (0..W * W).map(|i| avg(&a.y, &b.y, i)).collect(),
        cb: (0..W * W / 4).map(|i| avg(&a.cb, &b.cb, i)).collect(),
        cr: (0..W * W / 4).map(|i| avg(&a.cr, &b.cr, i)).collect(),
    };
    assert_frame(by_poc(&f, 4), &want, "bi-prediction");
}

#[test]
fn tmvp_takes_the_collocated_mv_scaled_by_poc_distance() {
    let i0 = gradient();
    let mut s = headers(true, true, &PpsInter { lists_modification: true, ..Default::default() });
    s.extend(idr(&i0, true));
    // P1: mv M = (8, -8) everywhere (AMVP then merge; whole luma and chroma samples), slice_temporal_mvp_enabled_flag 0.
    s.extend(picture(&SliceCfg { sps_tmvp: true, ..p_slice(1, vec![(-1, true)]) }, |e| amvp_then_merge(e, [8, -8])));
    // P2: StCurrBefore {1, 0}; list_entry_l0 [1, 0] makes RefPicList0 = [POC 0, POC 1]; the collocated picture is
    // RefPicList0[1] = P1. The merge Col candidate (refIdx 0 = POC 0): the centre block of CTB 0 ((8, 8) -> (0, 0);
    // the bottom-right lies in the next CTB row) holds M towards POC 0: colPocDiff 1, currPocDiff 2, td 1, tb 2,
    // tx 16384, distScaleFactor (2 * 16384 + 32) >> 6 = 512, mv = (512 * M + 127) >> 8 = 2M = (16, -16).
    let p2 = SliceCfg {
        sps_tmvp: true,
        tmvp: true,
        collocated_ref_idx: 1,
        num_ref: Some((2, 1)),
        lists_mod: true,
        list_entry_l0: Some(vec![1, 0]),
        ..p_slice(2, vec![(-1, true), (-2, true)])
    };
    s.extend(picture(&p2, skip_picture));
    let f = decode_all(&s);
    assert_frame(by_poc(&f, 1), &shifted(&i0, 2, -2), "P1 (mv M)");
    assert_frame(by_poc(&f, 2), &shifted(&i0, 4, -4), "P2 (collocated 2M)");
}

#[test]
fn pu_edge_with_different_mvs_is_deblocked_at_bs_1() {
    // Luma 100 left of x = 20, 110 from it; chroma flat.
    let i0 = Img::new(|x, _| if x < 20 { 100 } else { 110 }, |_, _| 128, |_, _| 128);
    let mut s = headers(false, false, &PpsInter::default());
    s.extend(idr(&i0, false));
    let cfg = SliceCfg { five_minus_max_num_merge_cand: 3, deblocking_disabled: false, ..p_slice(1, vec![(-1, true)]) };
    s.extend(picture(&cfg, |mut e| {
        // CTB 0: skip, merge_idx 0 of {zero, zero}: mv 0.
        e.split_cu(0, false);
        e.cu_skip(0, true);
        e.merge_idx(0, 2);
        e.terminate(0);
        // CTB 1 (left neighbour skipped: ctxInc 1): AMVP, mvp = A1 (CTB 0) = 0, mvd (16, 0): mv 4 samples right.
        e.split_cu(0, false);
        e.cu_skip(1, false);
        e.pred_inter();
        e.part_mode_2nx2n();
        e.merge_flag(false);
        e.mvd([16, 0]);
        e.mvp_flag(0);
        e.rqt_root_cbf(false);
        e.terminate(0);
        // CTB 2 (above skipped): skip, candidates {B1 = CTB 0 (mv 0), B0 = CTB 1}: merge_idx 0, mv 0.
        e.split_cu(0, false);
        e.cu_skip(1, true);
        e.merge_idx(0, 2);
        e.terminate(0);
        // CTB 3 (left skipped, above not): skip, candidates {A1 = CTB 2, B1 = CTB 1}: merge_idx 1, mv (16, 0).
        e.split_cu(0, false);
        e.cu_skip(1, true);
        e.merge_idx(1, 2);
        e.finish().out
    }));
    let f = decode_all(&s);
    // Before the filter: x < 16 reads x (100), x >= 16 reads x + 4 >= 20 (110). The vertical edge x = 16 separates
    // mv 0 from mv (16, 0) (one mv each, same picture, |dx| 16 >= 4, no coefficients): bS 1. QpY 26: beta 16,
    // tC = tC'[26 + 2 * (bS - 1)] = 1 (bS 2 would give tC'[28] = 2). d = 0 < beta; |p0 - q0| = 10 >= (5 tC + 1) >> 1:
    // normal filter, delta = (9 * 10 - 3 * 10 + 8) >> 4 = 4 -> Clip3(-1, 1) = 1: p0 101, q0 109; dEp / dEq hold but
    // Clip3(-(tC >> 1), tC >> 1) = 0 leaves p1 / q1. The horizontal edge y = 16 joins equal motion (bS 0); chroma is
    // filtered only at bS 2.
    let want = Img::new(
        |x, _| match x {
            15 => 101,
            16 => 109,
            _ if x < 16 => 100,
            _ => 110,
        },
        |_, _| 128,
        |_, _| 128,
    );
    assert_frame(by_poc(&f, 1), &want, "bS 1 edge");
}

#[test]
fn explicit_weighted_prediction() {
    let i0 = gradient();
    let mut s = headers(false, true, &PpsInter { weighted_pred: true, ..Default::default() });
    s.extend(idr(&i0, true));
    // luma_log2_weight_denom 6, LumaWeightL0[0] = 64 - 32 = 32, luma_offset_l0 10; chroma weights default.
    let cfg = SliceCfg { weights: Some((6, vec![Some((-32, 10))])), ..p_slice(1, vec![(-1, true)]) };
    s.extend(picture(&cfg, skip_picture));
    let f = decode_all(&s);
    // 8.5.3.3.4.3: log2WD 6 + 6 = 12: ((v << 6) * 32 + 2^11) >> 12 + 10 = ((v + 1) >> 1) + 10; chroma: w 64, o 0: v.
    let want = Img {
        y: i0.y.iter().map(|&v| ((v as u32 + 1) / 2 + 10) as u8).collect(),
        cb: i0.cb.clone(),
        cr: i0.cr.clone(),
    };
    assert_frame(by_poc(&f, 1), &want, "explicit weights");
}

#[test]
fn inter_residual_through_rqt_root_cbf() {
    let i0 = gradient();
    let mut s = headers(false, true, &PpsInter::default());
    s.extend(idr(&i0, true));
    s.extend(picture(&p_slice(1, vec![(-1, true)]), |mut e| {
        // CTB 0: a merged 2Nx2N CU (rqt_root_cbf inferred 1), one 16x16 TU (no split: MaxTrafoDepth 0, 2Nx2N),
        // cbf_cb 0, cbf_cr 0, cbf_luma inferred 1, residual_coding with one level 16 at (0, 0): last prefixes 0
        // (ctxOffset 6), greater1 1 (ctxSet 0, greater1Ctx 1), greater2 1, sign 0, remaining 13.
        e.split_cu(0, false);
        e.cu_skip(0, false);
        e.pred_inter();
        e.part_mode_2nx2n();
        e.merge_flag(true);
        e.cbf_chroma(0, false);
        e.cbf_chroma(0, false);
        e.decision(hevc_core::tables::LAST_X + 6, 0);
        e.decision(hevc_core::tables::LAST_Y + 6, 0);
        e.decision(hevc_core::tables::GT1 + 1, 1);
        e.decision(hevc_core::tables::GT2, 1);
        e.bypass(0);
        e.remaining(13, 0);
        for (inc, last) in [(0usize, false), (0, false), (2, true)] {
            e.terminate(0);
            e.split_cu(0, false);
            e.cu_skip(inc, true);
            if last {
                break;
            }
        }
        e.finish().out
    }));
    let f = decode_all(&s);
    // 8.6.2 at Qp'Y 26, m 16, 16x16 (bdShift 7): (16 * 16 * (51 << 4) + 64) >> 7 = 1632; 8.6.4.2 (DCT, not DST):
    // (64 * 1632 + 64) >> 7 = 816, (64 * 816 + 2048) >> 12 = 13 at every position.
    assert_eq!((16 * 16 * (51 << 4) + 64) >> 7, 1632);
    assert_eq!((64 * 1632 + 64) >> 7, 816);
    assert_eq!((64 * 816 + 2048) >> 12, 13);
    let mut want = i0.clone();
    for y in 0..16 {
        for x in 0..16 {
            want.y[y * W + x] = (i0.y[y * W + x] as u32 + 13).min(255) as u8;
        }
    }
    assert_frame(by_poc(&f, 1), &want, "inter residual");
}

// ---- HEVCPERF (LEDGER SR687): fast MC equals the clamped oracle (named tail block) ----

/// The crate's deterministic generator (tests/syntax.rs: the same LCG constants), 16 bits per draw.
fn lcg(x: &mut u32) -> u32 {
    *x = x.wrapping_mul(1103515245).wrapping_add(12345);
    *x >> 16
}

/// A w x h plane of random `bd`-bit samples.
fn rand_plane(w: usize, h: usize, bd: u32, seed: u32) -> hevc_core::picture::Plane {
    let mut s = seed;
    let mut p = hevc_core::picture::Plane::new(w, h, 0);
    for v in p.data.iter_mut() {
        *v = (lcg(&mut s) & ((1u32 << bd) - 1)) as u16;
    }
    p
}

/// Integer target positions of a block in a pw x ph plane: one interior, the four corners pushed past the picture,
/// and the four edges (the clamp cases of the padded window).
fn hevcperf_positions(pw: i32, ph: i32) -> [(i32, i32); 9] {
    [(pw / 2 - 4, ph / 2 - 4), (-6, -6), (pw - 3, -6), (-6, ph - 3), (pw - 3, ph - 3), (pw / 2, -5), (-5, ph / 2), (pw - 2, ph / 2), (pw / 2, ph - 2)]
}

#[test]
fn hevcperf_fast_mc_equals_the_clamped_oracle() {
    use hevc_core::inter::{chroma_mc, chroma_mc_ref, luma_mc, luma_mc_ref};
    for bd in [8u32, 10, 12, 16] {
        let ry = rand_plane(40, 24, bd, 0x5eed ^ bd);
        let rc = rand_plane(20, 12, bd, 0xc0de ^ bd);
        for (w, h) in [(8, 8), (16, 4), (4, 8), (64, 64)] {
            for (px, py) in hevcperf_positions(40, 24) {
                for xf in 0..4 {
                    for yf in 0..4 {
                        let mv = [(px - 8) * 4 + xf, (py - 8) * 4 + yf];
                        let (mut fast, mut slow) = (vec![0i32; (w * h) as usize], vec![0i32; (w * h) as usize]);
                        luma_mc(&ry, 8, 8, mv, w, h, bd, &mut fast);
                        luma_mc_ref(&ry, 8, 8, mv, w, h, bd, &mut slow);
                        assert_eq!(fast, slow, "luma bd {bd} {w}x{h} at ({px},{py}) phase ({xf},{yf})");
                    }
                }
            }
        }
        for (w, h) in [(4, 4), (8, 2), (2, 8), (32, 32)] {
            for (px, py) in hevcperf_positions(20, 12) {
                for xf in 0..8 {
                    for yf in 0..8 {
                        let mv = [(px - 4) * 8 + xf, (py - 4) * 8 + yf];
                        let (mut fast, mut slow) = (vec![0i32; (w * h) as usize], vec![0i32; (w * h) as usize]);
                        chroma_mc(&rc, 4, 4, mv, w, h, bd, &mut fast);
                        chroma_mc_ref(&rc, 4, 4, mv, w, h, bd, &mut slow);
                        assert_eq!(fast, slow, "chroma bd {bd} {w}x{h} at ({px},{py}) phase ({xf},{yf})");
                    }
                }
            }
        }
    }
}

/// The measurement (printed, not asserted): a 416x240 8-bit picture (the class-D conformance size; the conformance
/// vectors SKIP offline) predicted as 16x16 luma / 8x8 chroma blocks over every phase, the border blocks pushed past
/// the picture, through the clamped oracle and then the fast path.
#[test]
fn hevcperf_timing() {
    use hevc_core::inter::{chroma_mc, chroma_mc_ref, luma_mc, luma_mc_ref};
    use std::time::Instant;
    let ry = rand_plane(416, 240, 8, 0x1234);
    let rc = rand_plane(208, 120, 8, 0x4321);
    // (plane, block size, frac mask, frac bits, the mc fn) for luma then chroma.
    type Mc = fn(&hevc_core::picture::Plane, i32, i32, [i32; 2], i32, i32, u32, &mut [i32]);
    let run = |luma: Mc, chroma: Mc| -> (f64, Vec<i32>) {
        let mut all = Vec::new();
        let t = Instant::now();
        for (r, n, mask, bits, mc) in [(&ry, 16i32, 3i32, 2i32, luma), (&rc, 8, 7, 3, chroma)] {
            let (bw, bh) = (r.w as i32 / n, r.h as i32 / n);
            let mut out = vec![0i32; (n * n) as usize];
            let mut k = 0i32;
            for _round in 0..8 {
                for by in 0..bh {
                    for bx in 0..bw {
                        let d = |b: i32, last: i32| if b == 0 { -10 } else if b == last { 10 } else { k % 7 - 3 };
                        let mv = [(d(bx, bw - 1) << bits) + (k & mask), (d(by, bh - 1) << bits) + ((k >> bits) & mask)];
                        mc(r, bx * n, by * n, mv, n, n, 8, &mut out);
                        all.extend_from_slice(&out);
                        k += 1;
                    }
                }
            }
        }
        (t.elapsed().as_secs_f64() * 1000.0, all)
    };
    let (before, slow) = run(luma_mc_ref, chroma_mc_ref);
    let (after, fast) = run(luma_mc, chroma_mc);
    assert!(slow == fast, "fast path differs from the oracle on the timing picture");
    println!("hevcperf: inter vectors {before:.1} -> {after:.1}");
}

// ---- HEVCINTERCIP (LEDGER SR688): a missing reference picture is generated (named tail block) ----

/// 8.3.3.2: a stream whose first picture is the skip-only P1 (its IDR lost) names POC 0, which the DPB does not hold:
/// POC 0 is generated (grey 128, intra), P1 copies it, P2 (references POC 1 and POC 0) finds both without a second
/// generation, and only P1 and P2 are output.
#[test]
fn missing_reference_is_generated_and_decoding_continues() {
    let mut s = headers(false, true, &PpsInter::default());
    s.extend(picture(&p_slice(1, vec![(-1, true)]), skip_picture));
    s.extend(picture(&p_slice(2, vec![(-1, true), (-2, true)]), skip_picture));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the missing reference");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![0]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![1, 2]);
    let grey = Img::new(|_, _| 128, |_, _| 128, |_, _| 128);
    for fr in &f {
        assert_frame(fr, &grey, "P from the generated reference");
    }
}

// ---- HEVCERRORCONCEAL (LEDGER SR700): a generated reference concealed from the nearest held picture (named tail block) ----

/// IDR0 (the gradient), P1 (the gradient moved by (8, 4)), P2 lost, P3 (skip-only, references POC 2 alone): POC 2 is
/// generated and concealed from the nearest held picture, POC 1 (not POC 0, not grey), so P3 is P1's samples.
#[test]
fn lost_picture_is_concealed_from_the_nearest_held_picture() {
    let i0 = gradient();
    let mut s = headers(false, true, &PpsInter::default());
    s.extend(idr(&i0, true));
    s.extend(picture(&p_slice(1, vec![(-1, true)]), |e| amvp_then_merge(e, [32, 16])));
    s.extend(picture(&p_slice(3, vec![(-1, true)]), skip_picture));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![2]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 2, src_poc: 1, projected: true, bidir_blocks: 0, forward_blocks: 16 }]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 1, 3]);
    assert_frame(by_poc(&f, 3), &shifted(&shifted(&i0, 8, 4), 8, 4), "P from the picture concealed from POC 1, advanced by its motion");
}

/// IDR0 (the gradient), P2 (the gradient moved by (8, 4), references POC 0), P4 (skip-only, references the lost POC 1):
/// POC 0 and POC 2 are both one away from POC 1, and the lower POC conceals it, so P4 is the gradient.
#[test]
fn conceal_tie_takes_the_lower_poc() {
    let i0 = gradient();
    let mut s = headers(false, true, &PpsInter::default());
    s.extend(idr(&i0, true));
    s.extend(picture(&p_slice(2, vec![(-2, true)]), |e| amvp_then_merge(e, [32, 16])));
    s.extend(picture(&p_slice(4, vec![(-3, true)]), skip_picture));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![1]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 1, src_poc: 0, projected: false, bidir_blocks: 0, forward_blocks: 16 }]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4]);
    assert_frame(by_poc(&f, 2), &shifted(&i0, 8, 4), "P2 moved");
    assert_frame(by_poc(&f, 4), &i0, "P from the picture concealed from POC 0");
}

/// With the DPB empty (the stream starts at P1, its IDR lost) there is nothing to conceal from: POC 0 is generated grey
/// and nothing is noted as concealed.
#[test]
fn empty_dpb_keeps_the_generated_picture_grey() {
    let mut s = headers(false, true, &PpsInter::default());
    s.extend(picture(&p_slice(1, vec![(-1, true)]), skip_picture));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the missing reference");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![0]);
    assert!(d.take_concealed().is_empty(), "an empty DPB conceals nothing");
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![1]);
    assert_frame(&f[0], &Img::new(|_, _| 128, |_, _| 128, |_, _| 128), "P from the grey generated reference");
}

// ---- HEVCERRORCONCEALMOTION (LEDGER SR710): the concealed picture projected along its source's motion (named tail block) ----

/// IDR0 (the gradient), P2 (the gradient moved by (8, 4), references POC 0: the motion spans two POCs), P4 (skip-only,
/// references the lost POC 3): POC 3 is concealed from the nearest held picture, POC 2, whose vector (32, 16) is
/// rescaled by scale_mv to one POC (td 2, tb 1: (16, 8) quarter samples, (4, 2) samples, chroma (2, 1)), so P4 is P2
/// advanced by (4, 2) and the note says projected. (SR700's first test is the one-POC case; its tie test the intra
/// source's still copy.)
#[test]
fn conceal_projection_scales_by_poc_distance() {
    let i0 = gradient();
    let mut s = headers(false, true, &PpsInter::default());
    s.extend(idr(&i0, true));
    s.extend(picture(&p_slice(2, vec![(-2, true)]), |e| amvp_then_merge(e, [32, 16])));
    s.extend(picture(&p_slice(4, vec![(-1, true)]), skip_picture));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![3]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4]);
    assert_frame(by_poc(&f, 2), &shifted(&i0, 8, 4), "P2 moved");
    assert_frame(by_poc(&f, 4), &shifted(&shifted(&i0, 8, 4), 4, 2), "P from the picture projected from POC 2 by half its vector");
}

// ---- HEVCCONCEALBIDIR (LEDGER SR721): a bi-predicted source block projected along both lists (named tail block) ----

/// IDR0 (the gradient), P4 (skip-only: I0), B2 (StCurrBefore {0}, StCurrAfter {4}, two references per list: CTB 0 a
/// 2Nx2N AMVP unit, PRED_BI with L0 mvd (32, 16) to POC 0 and L1 mvd (-64, 0) to POC 4, or PRED_L1 alone when `l1_only`;
/// mvp 0 so mv = mvd; CTBs 1..3 skip-merge its motion), then P6 (skip-only, references only the lost POC 3). POC 3 is
/// generated and concealed from POC 2 (POC 2 and POC 4 tie at one away; the lower conceals). Returns the decoder after
/// the flush.
fn bidir_conceal_stream(l1_only: bool) -> hevc_core::Decoder {
    let mut s = headers(false, true, &PpsInter::default());
    s.extend(idr(&gradient(), true));
    s.extend(picture(&p_slice(4, vec![(-4, true)]), skip_picture));
    let bcfg = SliceCfg {
        nal_type: 1,
        slice_type: 0,
        poc_lsb: 2,
        neg: vec![(-2, true)],
        pos: vec![(2, true)],
        num_ref: Some((2, 2)),
        five_minus_max_num_merge_cand: 4,
        deblocking_disabled: true,
        ..Default::default()
    };
    s.extend(picture(&bcfg, |mut e| {
        e.split_cu(0, false);
        e.cu_skip(0, false);
        e.pred_inter();
        e.part_mode_2nx2n();
        e.merge_flag(false);
        if l1_only {
            // inter_pred_idc PRED_L1: bin 0 (ctxInc CtDepth 0) "0", then bin 1 (ctxInc 4) "1".
            e.decision(hevc_core::tables::INTER_PRED_IDC, 0);
            e.decision(hevc_core::tables::INTER_PRED_IDC + 4, 1);
        } else {
            e.pred_bi(0);
            e.ref_idx(0, 2);
            e.mvd([32, 16]);
            e.mvp_flag(0);
        }
        e.ref_idx(0, 2);
        e.mvd([-64, 0]);
        e.mvp_flag(0);
        e.rqt_root_cbf(false);
        for (inc, last) in [(0usize, false), (0, false), (2, true)] {
            e.terminate(0);
            e.split_cu(0, false);
            e.cu_skip(inc, true);
            if last {
                break;
            }
        }
        e.finish().out
    }));
    s.extend(picture(&p_slice(6, vec![(-3, true)]), skip_picture));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    d
}

fn img_of(f: &Frame) -> Img {
    Img { y: f.planes[0].clone(), cb: f.planes[1].clone(), cr: f.planes[2].clone() }
}

/// A bi-predicted source (B2) conceals the lost POC 3 along BOTH lists: L0 (32, 16) to POC 0 rescaled by td 2, tb 1 to
/// (16, 8) quarter samples = (4, 2) samples; L1 (-64, 0) to POC 4 rescaled by td -2, tb 1 to (32, 0) = (8, 0) samples;
/// the concealed picture (read through P6, its skip copy) is the 8-262 average (a + b + 1) >> 1 of the two whole-sample
/// fetches from B2, every one of the sixteen 8x8 blocks is bi-directional, and it differs from either fetch alone.
#[test]
fn conceal_bidir_averages_both_projections() {
    let mut d = bidir_conceal_stream(false);
    assert_eq!(d.take_generated_refs(), vec![3]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 16, forward_blocks: 16 }]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 4, 2, 6], "output in decoding order (no reordering in the SPS)");
    let b2 = img_of(by_poc(&f, 2));
    let (p, q) = (shifted(&b2, 4, 2), shifted(&b2, 8, 0));
    let avg = |a: &[u8], b: &[u8]| a.iter().zip(b.iter()).map(|(&x, &y)| ((x as u32 + y as u32 + 1) >> 1) as u8).collect::<Vec<u8>>();
    let want = Img { y: avg(&p.y, &q.y), cb: avg(&p.cb, &q.cb), cr: avg(&p.cr, &q.cr) };
    assert_ne!(want.y, p.y, "the fixture's two projections must differ");
    assert_ne!(want.y, q.y, "the fixture's two projections must differ");
    assert_frame(by_poc(&f, 6), &want, "P from the picture concealed along both lists of B2");
}

/// A source block predicted from L1 alone (PRED_L1) projects along L1 only: (8, 0) samples, no bi-directional block.
#[test]
fn conceal_bidir_one_list_falls_back() {
    let mut d = bidir_conceal_stream(true);
    assert_eq!(d.take_generated_refs(), vec![3]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 4, 2, 6], "output in decoding order (no reordering in the SPS)");
    assert_frame(by_poc(&f, 6), &shifted(&img_of(by_poc(&f, 2)), 8, 0), "P from the picture concealed along L1 of B2");
}

// ---- HEVCCONCEALSUBPEL (LEDGER SR733): the concealed picture projected through the interpolation filters (named tail block) ----

/// IDR0 (the gradient), P2 (references POC 0 with the quarter-sample mv (6, 2)), P4 (skip-only, references the lost POC
/// 3 and keeps POC 2 unused), P5 (references POC 2 with mv (3, 1)): POC 3 is concealed from POC 2, whose vector (6, 2)
/// rescales by td 2, tb 1 to (3, 1) quarter samples = (0.75, 0.25), kept fractional and fetched through the 8-tap /
/// 4-tap filters, so P4 (the concealed picture's skip copy) is exactly P5, the decoder's own uni-predicted PU of P2 with
/// the same vector, and the hand-written 8.5.3.3.3 equations; it is not SR710's whole-sample projection (1, 0).
#[test]
fn conceal_subpel_projects_through_the_interpolation() {
    let i0 = gradient();
    let mut s = headers(false, true, &PpsInter::default());
    s.extend(idr(&i0, true));
    s.extend(picture(&p_slice(2, vec![(-2, true)]), |e| amvp_then_merge(e, [6, 2])));
    s.extend(picture(&p_slice(4, vec![(-1, true), (-2, false)]), skip_picture));
    s.extend(picture(&p_slice(5, vec![(-3, true)]), |e| amvp_then_merge(e, [3, 1])));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![3]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4, 5]);
    assert_frame(by_poc(&f, 2), &hand_mc(&i0, [6, 2]), "P2 at the quarter-sample mv (6, 2)");
    let p2 = img_of(by_poc(&f, 2));
    let p5 = img_of(by_poc(&f, 5));
    assert_frame(by_poc(&f, 4), &p5, "P from the concealed picture equals the decoder's own MC of P2 at (3, 1)");
    assert_frame(by_poc(&f, 4), &hand_mc(&p2, [3, 1]), "P from the concealed picture follows 8.5.3.3.3 at (3, 1)");
    assert_ne!(by_poc(&f, 4).planes[0], shifted(&p2, 1, 0).y, "not SR710's whole-sample rounding of (3, 1)");
}

// ---- HEVCCONCEALWEIGHTED (LEDGER SR741): the concealed picture weighted as its source's PUs were (named tail block) ----

/// The PPS carries weighted_pred_flag; IDR0 (the gradient), P2 (references POC 0 with mv (32, 16), pred_weight_table
/// luma_log2_weight_denom 6, LumaWeightL0[0] 32, luma_offset 10: a fade halving luma and lifting it by 10; chroma
/// weights default), P4 (skip-only, references the lost POC 3 and keeps POC 2 unused; its table's one entry has the
/// default weight: a copy), P5 (references POC 2 with mv (16, 8) and P2's weights). POC 3 is concealed from POC 2,
/// whose vector rescales by td 2, tb 1 to (16, 8) = (4, 2) samples and whose block was weighted (32, 10) against its
/// reference: the projection is weighted the same way through 8.5.3.3.4.3, so P4 (the concealed picture's copy) is
/// exactly P5, the decoder's own weighted MC of P2 with the same vector and weights, and the hand equation
/// ((P2(x + 4, y + 2) + 1) >> 1) + 10 (chroma P2's shifted samples); it is not the unit-weight projection.
#[test]
fn conceal_weighted_applies_the_source_weights() {
    let i0 = gradient();
    let fade = Some((6, vec![Some((-32, 10))]));
    let mut s = headers(false, true, &PpsInter { weighted_pred: true, ..Default::default() });
    s.extend(idr(&i0, true));
    s.extend(picture(&SliceCfg { weights: fade.clone(), ..p_slice(2, vec![(-2, true)]) }, |e| amvp_then_merge(e, [32, 16])));
    s.extend(picture(&SliceCfg { weights: Some((6, vec![None])), ..p_slice(4, vec![(-1, true), (-2, false)]) }, skip_picture));
    s.extend(picture(&SliceCfg { weights: fade, ..p_slice(5, vec![(-3, true)]) }, |e| amvp_then_merge(e, [16, 8])));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![3]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4, 5]);
    let faded = |r: &Img| Img { y: r.y.iter().map(|&v| ((v as u32 + 1) / 2 + 10) as u8).collect(), cb: r.cb.clone(), cr: r.cr.clone() };
    assert_frame(by_poc(&f, 2), &faded(&shifted(&i0, 8, 4)), "P2: the gradient moved by (8, 4) and weighted (32, 10)");
    let p2 = img_of(by_poc(&f, 2));
    let p5 = img_of(by_poc(&f, 5));
    assert_frame(by_poc(&f, 4), &p5, "P from the concealed picture equals the decoder's own weighted MC of P2 at (16, 8)");
    assert_frame(by_poc(&f, 4), &faded(&shifted(&p2, 4, 2)), "P from the concealed picture follows 8.5.3.3.4.3 with P2's weights");
    assert_ne!(by_poc(&f, 4).planes[0], shifted(&p2, 4, 2).y, "not the unit-weight projection");
}

// ---- HEVCCONCEALFORWARD (LEDGER SR754): the concealed picture refined from its first referencer's motion (named tail block) ----

/// IDR0 (the gradient), P2 (references POC 0 with mv (6, 2)), P4 (references the lost POC 3 with mv (32, 16) on every
/// block, keeps POC 2 unused), P5 (references POC 2 with mv (3, 1), keeps POC 3 unused), P6 (skip-only, references POC 3
/// alone). POC 3 is concealed from POC 2 (SR733: P5 is that projection); P4 reads it moved by (8, 4); when P4 ends, POC 3
/// is re-projected from P4's motion (td 1, tb -1: mv' = -mv = (-32, -16), (-8, -4) samples, edges clamped), so P6 (its
/// skip copy) is P4 moved back, not the source projection, while P4 and P5 keep their samples (C is never written).
#[test]
fn conceal_forward_refines_from_the_first_referencer() {
    let i0 = gradient();
    let mut s = headers(false, true, &PpsInter::default());
    s.extend(idr(&i0, true));
    s.extend(picture(&p_slice(2, vec![(-2, true)]), |e| amvp_then_merge(e, [6, 2])));
    s.extend(picture(&p_slice(4, vec![(-1, true), (-2, false)]), |e| amvp_then_merge(e, [32, 16])));
    s.extend(picture(&p_slice(5, vec![(-2, false), (-3, true)]), |e| amvp_then_merge(e, [3, 1])));
    s.extend(picture(&p_slice(6, vec![(-3, true)]), skip_picture));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![3]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4, 5, 6]);
    let p2 = img_of(by_poc(&f, 2));
    let p5 = img_of(by_poc(&f, 5));
    assert_frame(by_poc(&f, 5), &hand_mc(&p2, [3, 1]), "P5 reads P2, not the refined picture");
    assert_frame(by_poc(&f, 4), &shifted(&p5, 8, 4), "P4 read the source projection of POC 3 and is never rewritten");
    let p4 = img_of(by_poc(&f, 4));
    assert_frame(by_poc(&f, 6), &shifted(&p4, -8, -4), "P from the picture refined from P4 at -mv");
    assert_ne!(by_poc(&f, 6).planes[0], p5.y, "not the source projection alone");
}

// ---- HEVCCONCEALWEIGHTEDBIMIXED (LEDGER SR768): a bi source's two lists weighted with their own weights and one log2WD (named tail block) ----

/// B2's pred_weight_table (B7 carries its first entries) (luma_log2_weight_denom 6): L0[0] LumaWeight 64 - 16 = 48, offset 5; L1[0]
/// 64 - 48 = 16, offset -3 (the second entries default; chroma weights default).
fn bi_mixed_weights() -> (Option<(u32, Vec<Option<(i32, i32)>>)>, Option<Vec<Option<(i32, i32)>>>) {
    (Some((6, vec![Some((-16, 5)), None])), Some(vec![Some((-48, -3)), None]))
}

/// One B picture's slice data: CTB 0 a 2Nx2N AMVP unit, PRED_BI, ref_idx 0 in both lists of `n` active references, L0 mvd
/// `m0`, L1 mvd `m1` (mvp 0, so mv = mvd); CTBs 1..3 skip-merge its motion.
fn bi_body(mut e: CabacEnc, n: u32, m0: [i32; 2], m1: [i32; 2]) -> Vec<u8> {
    e.split_cu(0, false);
    e.cu_skip(0, false);
    e.pred_inter();
    e.part_mode_2nx2n();
    e.merge_flag(false);
    e.pred_bi(0);
    for mv in [m0, m1] {
        e.ref_idx(0, n);
        e.mvd(mv);
        e.mvp_flag(0);
    }
    e.rqt_root_cbf(false);
    for (inc, last) in [(0usize, false), (0, false), (2, true)] {
        e.terminate(0);
        e.split_cu(0, false);
        e.cu_skip(inc, true);
        if last {
            break;
        }
    }
    e.finish().out
}

/// The PPS carries weighted_bipred_flag (not weighted_pred_flag: the P pictures carry no table). IDR0 (the gradient), P4
/// (skip-only: I0), B2 (StCurrBefore {0}, StCurrAfter {4}, two references per list, the mixed table of
/// [`bi_mixed_weights`]; PRED_BI L0 (32, 16) to POC 0, L1 (-64, 0) to POC 4), P6 (skip-only, references the lost POC 3 and
/// keeps POC 2 unused), B7 (both lists [POC 2], one reference each, the first entries of that table; PRED_BI L0 (16, 8), L1 (32, 0): B2's
/// vectors rescaled to POC 3 by td 2, tb 1 and td -2, tb 1). POC 3 is concealed from POC 2 along both lists, each fetch
/// weighted by its own list's (w, o) and the ONE log2WD 6 + 6 = 12 (8.5.3.3.4.3): (64a * 48 + 64b * 16 + ((5 - 3 + 1) << 12))
/// >> 13 = (3a + b + 12) >> 3; chroma (default weights) (a + b + 1) >> 1. So P6 (the concealed picture's skip copy) is
/// exactly B7, the decoder's own weighted bi MC of B2 at the projected vectors, and the hand equation; it is neither the
/// unweighted average nor the blend with the lists' weights swapped.
#[test]
fn conceal_weighted_bi_mixed_lists_use_one_log2wd() {
    let i0 = gradient();
    let (w0, w1) = bi_mixed_weights();
    let mut s = headers(false, true, &PpsInter { weighted_bipred: true, ..Default::default() });
    s.extend(idr(&i0, true));
    s.extend(picture(&p_slice(4, vec![(-4, true)]), skip_picture));
    let b2 = SliceCfg {
        nal_type: 1,
        slice_type: 0,
        poc_lsb: 2,
        neg: vec![(-2, true)],
        pos: vec![(2, true)],
        num_ref: Some((2, 2)),
        weights: w0.clone(),
        weights_l1: w1.clone(),
        five_minus_max_num_merge_cand: 4,
        deblocking_disabled: true,
        ..Default::default()
    };
    s.extend(picture(&b2, |e| bi_body(e, 2, [32, 16], [-64, 0])));
    s.extend(picture(&p_slice(6, vec![(-3, true), (-4, false)]), skip_picture));
    // B7's one reference per list carries the first entry of each table (7.3.6.3 writes num_ref_idx_lX_active entries).
    let b7 = SliceCfg { poc_lsb: 7, neg: vec![(-5, true)], pos: vec![], num_ref: Some((1, 1)), weights: Some((6, vec![Some((-16, 5))])), weights_l1: Some(vec![Some((-48, -3))]), ..b2.clone() };
    s.extend(picture(&b7, |e| bi_body(e, 1, [16, 8], [32, 0])));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![3]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 16, forward_blocks: 16 }]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 4, 2, 6, 7], "output in decoding order (no reordering in the SPS)");
    let blend = |p: &Img, q: &Img, w0: u32, w1: u32| {
        let luma = p.y.iter().zip(q.y.iter()).map(|(&a, &b)| ((w0 * a as u32 + w1 * b as u32 + 12) >> 3) as u8).collect();
        let avg = |a: &[u8], b: &[u8]| a.iter().zip(b.iter()).map(|(&x, &y)| ((x as u32 + y as u32 + 1) >> 1) as u8).collect::<Vec<u8>>();
        Img { y: luma, cb: avg(&p.cb, &q.cb), cr: avg(&p.cr, &q.cr) }
    };
    assert_frame(by_poc(&f, 2), &blend(&shifted(&i0, 8, 4), &shifted(&i0, -16, 0), 3, 1), "B2: L0 weighted (48, 5), L1 (16, -3)");
    let src = img_of(by_poc(&f, 2));
    let (p, q) = (shifted(&src, 4, 2), shifted(&src, 8, 0));
    assert_ne!(p.y, q.y, "the fixture's two projections must differ");
    let b7f = img_of(by_poc(&f, 7));
    assert_frame(by_poc(&f, 6), &b7f, "P from the concealed picture equals the decoder's own weighted bi MC of B2 at the projected vectors");
    assert_frame(by_poc(&f, 6), &blend(&p, &q, 3, 1), "P from the concealed picture follows 8.5.3.3.4.3 with each list's weights");
    let unweighted = p.y.iter().zip(q.y.iter()).map(|(&a, &b)| ((a as u32 + b as u32 + 1) >> 1) as u8).collect::<Vec<u8>>();
    assert_ne!(by_poc(&f, 6).planes[0], unweighted, "not the unweighted 8-262 average");
    assert_ne!(by_poc(&f, 6).planes[0], blend(&p, &q, 1, 3).y, "not the blend with the lists' weights swapped");
}

// ---- HEVCCONCEALWEIGHTEDCHROMA (LEDGER SR776): the concealed picture weighted on its chroma planes as its source's PUs were (named tail block) ----

/// SR741's P shape with explicit chroma weights. The PPS carries weighted_pred_flag; IDR0 (the gradient), P2 (references
/// POC 0 with mv (32, 16)), P4 (skip-only, references the lost POC 3 and keeps POC 2 unused; its one entry defaults: a
/// copy), P5 (references POC 2 with mv (16, 8)). P2 and P5 carry SR741's luma fade (denom 6, LumaWeightL0[0] 32, offset
/// 10) and chroma weights: delta_chroma_log2_weight_denom 1 (ChromaLog2WeightDenom 7), Cb delta_chroma_weight -32
/// (ChromaWeight 96) delta_chroma_offset -12 (7-56: 128 - 12 - ((128 * 96) >> 7) = +20), Cr delta 32 (160), delta_chroma_offset 2
/// (128 + 2 - 160 = -30). Whole-sample vectors, so 8.5.3.3.4.3 at log2WD 7 + 6 = 13 reads: Cb ((3s + 2) >> 2) + 20, Cr
/// ((5s + 2) >> 2) - 30, luma ((s + 1) >> 1) + 10. POC 3 is concealed from POC 2 through `conceal_fetch` with planes 1
/// and 2 weighted by their own (w, o, log2WD), so P4 (the concealed picture's copy) is exactly P5 and the hand equations
/// on ALL THREE planes, and its chroma planes are not the default-chroma-weight projection.
#[test]
fn conceal_weighted_chroma_applies_the_source_chroma_weights() {
    let i0 = gradient();
    let fade = Some((6, vec![Some((-32, 10))]));
    let chroma = Some((1, vec![Some([(-32, -12), (32, 2)])]));
    let mut s = headers(false, true, &PpsInter { weighted_pred: true, ..Default::default() });
    s.extend(idr(&i0, true));
    s.extend(picture(&SliceCfg { weights: fade.clone(), chroma_weights: chroma.clone(), ..p_slice(2, vec![(-2, true)]) }, |e| amvp_then_merge(e, [32, 16])));
    s.extend(picture(&SliceCfg { weights: Some((6, vec![None])), ..p_slice(4, vec![(-1, true), (-2, false)]) }, skip_picture));
    s.extend(picture(&SliceCfg { weights: fade, chroma_weights: chroma, ..p_slice(5, vec![(-3, true)]) }, |e| amvp_then_merge(e, [16, 8])));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![3]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4, 5]);
    let weighted = |r: &Img| Img {
        y: r.y.iter().map(|&v| ((v as i32 + 1) / 2 + 10).clamp(0, 255) as u8).collect(),
        cb: r.cb.iter().map(|&v| (((3 * v as i32 + 2) >> 2) + 20).clamp(0, 255) as u8).collect(),
        cr: r.cr.iter().map(|&v| (((5 * v as i32 + 2) >> 2) - 30).clamp(0, 255) as u8).collect(),
    };
    assert_frame(by_poc(&f, 2), &weighted(&shifted(&i0, 8, 4)), "P2: the gradient moved by (8, 4), luma (32, 10), Cb (96, +20), Cr (160, -30)");
    let p2 = img_of(by_poc(&f, 2));
    let p5 = img_of(by_poc(&f, 5));
    assert_frame(by_poc(&f, 4), &p5, "P from the concealed picture equals the decoder's own weighted MC of P2 at (16, 8) on every plane");
    assert_frame(by_poc(&f, 4), &weighted(&shifted(&p2, 4, 2)), "P from the concealed picture follows 8.5.3.3.4.3 with P2's luma and chroma weights");
    let unit = shifted(&p2, 4, 2);
    assert_ne!(by_poc(&f, 4).planes[1], unit.cb, "Cb is not the default-chroma-weight projection");
    assert_ne!(by_poc(&f, 4).planes[2], unit.cr, "Cr is not the default-chroma-weight projection");
}

// ---- HEVCCONCEALWEIGHTEDBIDIRECT (LEDGER SR774): a bi source block with one long-term list conceals uni with the surviving list's weights (named tail block) ----

/// [`bi_body`] with ref_idx `refs` (L0, L1) of `n` active references per list: CTB 0 a 2Nx2N AMVP unit, PRED_BI, L0 mvd
/// `m0`, L1 mvd `m1` (mvp 0, so mv = mvd); CTBs 1..3 skip-merge its motion.
fn bi_ref_body(mut e: CabacEnc, n: u32, refs: [u32; 2], m0: [i32; 2], m1: [i32; 2]) -> Vec<u8> {
    e.split_cu(0, false);
    e.cu_skip(0, false);
    e.pred_inter();
    e.part_mode_2nx2n();
    e.merge_flag(false);
    e.pred_bi(0);
    for (r, mv) in refs.into_iter().zip([m0, m1]) {
        e.ref_idx(r, n);
        e.mvd(mv);
        e.mvp_flag(0);
    }
    e.rqt_root_cbf(false);
    for (inc, last) in [(0usize, false), (0, false), (2, true)] {
        e.terminate(0);
        e.split_cu(0, false);
        e.cu_skip(inc, true);
        if last {
            break;
        }
    }
    e.finish().out
}

/// The SPS carries long_term_ref_pics_present_flag; the PPS weighted_pred_flag and weighted_bipred_flag. IDR0 (the
/// gradient), P4 (I0, default table: a copy), B2 (StCurrAfter {4}, LtCurr {POC 0 by PocLsbLt 0}: L0 = L1 = [POC 4,
/// POC 0 long-term], two references per list; luma_log2_weight_denom 6, every L0 entry (48, 5), every L1 entry (16, -3);
/// PRED_BI with one list to the long-term POC 0 (ref_idx 1, mvd (32, 16)) and the other to POC 4 (ref_idx 0, mvd
/// (-64, 0))), P6 (skip-only, references the lost POC 3, keeps POC 2 unused), P7 (references POC 2 with mv (32, 0) and
/// the SURVIVING list's weights). `project_lists` drops the long-term list (`ref_lt`), so POC 3 is concealed from POC 2
/// along the other list alone: (-64, 0) rescaled by td -2, tb 1 to (32, 0) = (8, 0) samples, weighted uni (8-252) by
/// THAT list's (w, o) through `conceal_fetch`'s `w[cand[0].is_none() as usize]`: (A) L0 long-term -> L1 (16, -3):
/// ((a + 2) >> 2) - 3; (B) L1 long-term -> L0 (48, 5): ((3a + 2) >> 2) + 5; chroma (default weights) the shifted
/// samples. So P6 (the concealed picture's skip copy) is exactly P7, the decoder's own uni weighted MC of B2, and the
/// hand equation; it is neither uni through the OTHER list's weights nor the bi blend with the long-term list
/// projected; every block is uni (bidir_blocks 0).
#[test]
fn conceal_weighted_bi_direct_falls_to_the_surviving_list() {
    let i0 = gradient();
    let (lt, st) = ([32, 16], [-64, 0]);
    // (w, o) of each list: L0 (64 - 16, 5), L1 (64 - 48, -3).
    let wo = [(48, 5), (16, -3)];
    for l1_lt in [false, true] {
        let mut s = vps(false);
        s.extend(nal(33, &sps_rbsp_ext(&sps_cfg(), &InterSps { long_term_refs: true, ..inter_sps(false) })));
        s.extend(nal(34, &pps_rbsp_ext(&PpsCfg { init_qp: 26, deblocking_disabled: true, ..Default::default() }, &PpsInter { weighted_pred: true, weighted_bipred: true, ..Default::default() })));
        s.extend(idr(&i0, true));
        s.extend(picture(&SliceCfg { weights: Some((6, vec![None])), lt_pocs: Some(vec![]), ..p_slice(4, vec![(-4, true)]) }, skip_picture));
        let b2 = SliceCfg {
            nal_type: 1,
            slice_type: 0,
            poc_lsb: 2,
            neg: vec![],
            pos: vec![(2, true)],
            lt_pocs: Some(vec![0]),
            num_ref: Some((2, 2)),
            weights: Some((6, vec![Some((-16, 5)), Some((-16, 5))])),
            weights_l1: Some(vec![Some((-48, -3)), Some((-48, -3))]),
            five_minus_max_num_merge_cand: 4,
            deblocking_disabled: true,
            ..Default::default()
        };
        let (refs, m0, m1) = if l1_lt { ([0, 1], st, lt) } else { ([1, 0], lt, st) };
        s.extend(picture(&b2, |e| bi_ref_body(e, 2, refs, m0, m1)));
        s.extend(picture(&SliceCfg { weights: Some((6, vec![None])), lt_pocs: Some(vec![]), ..p_slice(6, vec![(-3, true), (-4, false)]) }, skip_picture));
        let (live, dead) = if l1_lt { (wo[0], wo[1]) } else { (wo[1], wo[0]) };
        s.extend(picture(&SliceCfg { weights: Some((6, vec![Some((live.0 - 64, live.1))])), lt_pocs: Some(vec![]), ..p_slice(7, vec![(-5, true)]) }, |e| amvp_then_merge(e, [32, 0])));
        let mut d = Decoder::new();
        d.decode_annexb(&s).expect("decode past the lost picture");
        d.flush();
        let what = if l1_lt { "L1 long-term, L0 survives" } else { "L0 long-term, L1 survives" };
        assert_eq!(d.take_generated_refs(), vec![3], "{what}");
        assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }], "{what}: every block uni");
        let f = d.take_frames();
        assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 4, 2, 6, 7], "{what}: output in decoding order");
        let src = img_of(by_poc(&f, 2));
        let (p, q) = (shifted(&src, 4, 2), shifted(&src, 8, 0));
        // 8-252 with log2WD 6 + 6 = 12: ((64a * w + 2048) >> 12) + o, clipped.
        let uni = |r: &Img, (w, o): (i32, i32)| r.y.iter().map(|&a| (((64 * a as i32 * w + 2048) >> 12) + o).clamp(0, 255) as u8).collect::<Vec<u8>>();
        let want = Img { y: uni(&q, live), cb: q.cb.clone(), cr: q.cr.clone() };
        assert_ne!(uni(&q, live), uni(&q, dead), "{what}: the fixture's two lists' weights must differ");
        let p7 = img_of(by_poc(&f, 7));
        assert_frame(by_poc(&f, 6), &p7, &format!("{what}: P from the concealed picture equals the decoder's own uni weighted MC of B2 at (32, 0)"));
        assert_frame(by_poc(&f, 6), &want, &format!("{what}: P from the concealed picture follows 8-252 with the surviving list's weights"));
        assert_ne!(by_poc(&f, 6).planes[0], uni(&q, dead), "{what}: not uni through the other list's weights");
        // 8.5.3.3.4.3 with the long-term list projected as if usable (td 2, tb 1: (16, 8) = (4, 2) samples), in list order.
        let (a0, a1) = if l1_lt { (&q, &p) } else { (&p, &q) };
        let blend = a0.y.iter().zip(a1.y.iter()).map(|(&x, &y)| ((64 * x as i32 * wo[0].0 + 64 * y as i32 * wo[1].0 + ((wo[0].1 + wo[1].1 + 1) << 12)) >> 13).clamp(0, 255) as u8).collect::<Vec<u8>>();
        assert_ne!(by_poc(&f, 6).planes[0], blend, "{what}: not the bi blend");
    }
}
