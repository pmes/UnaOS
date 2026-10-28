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
