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
use hevc_core::tables::LUMA_FILTER;
use hevc_core::{Decoder, Frame};

const W: usize = 32;

#[derive(Clone)]
struct Img {
    y: Vec<u16>,
    cb: Vec<u16>,
    cr: Vec<u16>,
}

impl Img {
    fn new(fy: impl Fn(usize, usize) -> u16, fcb: impl Fn(usize, usize) -> u16, fcr: impl Fn(usize, usize) -> u16) -> Img {
        let plane = |n: usize, f: &dyn Fn(usize, usize) -> u16| (0..n * n).map(|i| f(i % n, i / n)).collect::<Vec<u16>>();
        Img { y: plane(W, &fy), cb: plane(W / 2, &fcb), cr: plane(W / 2, &fcr) }
    }
    /// The samples of the 16x16 luma block at CTB (cx, cy) and its 8x8 chroma blocks.
    fn ctb(&self, cx: usize, cy: usize) -> (Vec<u16>, Vec<u16>, Vec<u16>) {
        let blk = |p: &[u16], n: usize, s: usize, x0: usize, y0: usize| {
            (0..s * s).map(|i| p[(y0 + i / s) * n + x0 + i % s]).collect::<Vec<u16>>()
        };
        (blk(&self.y, W, 16, cx * 16, cy * 16), blk(&self.cb, W / 2, 8, cx * 8, cy * 8), blk(&self.cr, W / 2, 8, cx * 8, cy * 8))
    }
    fn planes(&self) -> [&Vec<u16>; 3] {
        [&self.y, &self.cb, &self.cr]
    }
}

fn gradient() -> Img {
    Img::new(|x, y| (x * 5 + y * 3) as u16, |x, y| (40 + x * 3 + y) as u16, |x, y| (200 - x - 2 * y) as u16)
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
            assert_eq!(got[i] as u16, want[i], "{what}: plane {c} sample ({}, {})", i % n, i / n);
        }
    }
}

/// The reference shifted by a full-sample luma vector (dx, dy) (chroma by (dx / 2, dy / 2)), edges clamped.
fn shifted(r: &Img, dx: i32, dy: i32) -> Img {
    let at = |p: &[u16], n: usize, x: usize, y: usize, dx: i32, dy: i32| {
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
fn hand_luma(r: &[u16], x: i32, y: i32, mv: [i32; 2]) -> i32 {
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
fn hand_chroma(r: &[u16], x: i32, y: i32, mv: [i32; 2]) -> i32 {
    hand_chroma_wh(r, 16, 16, x, y, mv)
}

/// Uni-prediction with default weights (8.5.3.3.4.2: (predSamples + 32) >> 6, clipped).
fn hand_mc(r: &Img, mv: [i32; 2]) -> Img {
    let uni = |v: i32| ((v + 32) >> 6).clamp(0, 255) as u16;
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
    let b = Img::new(|x, y| (250 - x * 2 - y * 5) as u16, |x, y| (10 + x * 7 + y * 2) as u16, |x, y| (90 + x + y * 4) as u16);
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
    // CTB 0: PRED_BI, ref_idx 0 in both lists, zero mvds (mvp 0); CTBs 1..3 merge its bi-prediction.
    d.decode_annexb(&picture(&bcfg, |e| bi_body(e, 2, [0, 0], [0, 0]))).unwrap();
    let lists = d.slice_ref_lists();
    assert_eq!((lists[0][0].poc.clone(), lists[0][1].poc.clone()), (vec![0, 8], vec![8, 0]));
    d.flush();
    let f = d.take_frames();
    // 8.5.3.3.4.2: (a << 6 + b << 6 + 64) >> 7 = (a + b + 1) >> 1.
    let avg = |p: &[u16], q: &[u16], i: usize| ((p[i] as u32 + q[i] as u32 + 1) >> 1) as u16;
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
        y: i0.y.iter().map(|&v| ((v as u32 + 1) / 2 + 10) as u16).collect(),
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
            want.y[y * W + x] = (i0.y[y * W + x] as u32 + 13).min(255) as u16;
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
        if !l1_only {
            return bi_body(e, 2, [32, 16], [-64, 0]);
        }
        e.split_cu(0, false);
        e.cu_skip(0, false);
        e.pred_inter();
        e.part_mode_2nx2n();
        e.merge_flag(false);
        // inter_pred_idc PRED_L1: bin 0 (ctxInc CtDepth 0) "0", then bin 1 (ctxInc 4) "1".
        e.decision(hevc_core::tables::INTER_PRED_IDC, 0);
        e.decision(hevc_core::tables::INTER_PRED_IDC + 4, 1);
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
    let w = |p: &[u8]| p.iter().map(|&v| v as u16).collect::<Vec<u16>>(); Img { y: w(&f.planes[0]), cb: w(&f.planes[1]), cr: w(&f.planes[2]) }
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
    let avg = |a: &[u16], b: &[u16]| a.iter().zip(b.iter()).map(|(&x, &y)| ((x as u32 + y as u32 + 1) >> 1) as u16).collect::<Vec<u16>>();
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
    assert_ne!(img_of(by_poc(&f, 4)).y, shifted(&p2, 1, 0).y, "not SR710's whole-sample rounding of (3, 1)");
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
/// SR1056 (HEVCCONCEALWEIGHTEDDISTANCE): c - s = 1 against s - r = 2, so the concealment scales P2's (32, 10) by the ratio
/// 1/2 (dpb.rs `scale_weight`: gain 0.5^(1/2) in the log domain, offset 10 / 2) to (45, 5); P5 carries that scaled table.
#[test]
fn conceal_weighted_applies_the_source_weights() {
    let i0 = gradient();
    let fade = Some((6, vec![Some((-32, 10))]));
    let mut s = headers(false, true, &PpsInter { weighted_pred: true, ..Default::default() });
    s.extend(idr(&i0, true));
    s.extend(picture(&SliceCfg { weights: fade.clone(), ..p_slice(2, vec![(-2, true)]) }, |e| amvp_then_merge(e, [32, 16])));
    s.extend(picture(&SliceCfg { weights: Some((6, vec![None])), ..p_slice(4, vec![(-1, true), (-2, false)]) }, skip_picture));
    s.extend(picture(&SliceCfg { weights: /* SR1056 (was fade (32, 10)) */ Some((6, vec![Some((-19, 5))])), ..p_slice(5, vec![(-3, true)]) }, |e| amvp_then_merge(e, [16, 8])));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![3]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4, 5]);
    let faded = |r: &Img| Img { y: r.y.iter().map(|&v| ((v as u32 + 1) / 2 + 10) as u16).collect(), cb: r.cb.clone(), cr: r.cr.clone() };
    assert_frame(by_poc(&f, 2), &faded(&shifted(&i0, 8, 4)), "P2: the gradient moved by (8, 4) and weighted (32, 10)");
    let scaled = |r: &Img| Img { y: r.y.iter().map(|&v| (((45 * v as u32 + 32) >> 6) + 5) as u16).collect(), cb: r.cb.clone(), cr: r.cr.clone() };
    let p2 = img_of(by_poc(&f, 2));
    let p5 = img_of(by_poc(&f, 5));
    assert_frame(by_poc(&f, 4), &p5, "P from the concealed picture equals the decoder's own weighted MC of P2 at (16, 8)");
    assert_frame(by_poc(&f, 4), &scaled(&shifted(&p2, 4, 2)), "SR1056 (was ((v + 1) >> 1) + 10): P from the concealed picture follows 8.5.3.3.4.3 with P2's weights scaled to (45, 5)");
    assert_ne!(img_of(by_poc(&f, 4)).y, faded(&shifted(&p2, 4, 2)).y, "SR1056: not P2's weights applied unscaled");
    assert_ne!(img_of(by_poc(&f, 4)).y, shifted(&p2, 4, 2).y, "not the unit-weight projection");
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
    assert_ne!(img_of(by_poc(&f, 6)).y, p5.y, "not the source projection alone");
}

// ---- HEVCCONCEALWEIGHTEDBIMIXED (LEDGER SR768): a bi source's two lists weighted with their own weights and one log2WD (named tail block) ----

/// B2's pred_weight_table (B7 carries its first entries) (luma_log2_weight_denom 6): L0[0] LumaWeight 64 - 16 = 48, offset 5; L1[0]
/// 64 - 48 = 16, offset -3 (the second entries default; chroma weights default).
fn bi_mixed_weights() -> (Option<(u32, Vec<Option<(i32, i32)>>)>, Option<Vec<Option<(i32, i32)>>>) {
    (Some((6, vec![Some((-16, 5)), None])), Some(vec![Some((-48, -3)), None]))
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
/// SR1056: L0 (to POC 0) scales by 1/2 and L1 (to POC 4) by -1/2: L0 (48, 5) -> (55, 3), L1 (16, -3) -> (128, 2) (the inverse
/// gain 4^(1/2)), so the blend is (55a + 128b + 384) >> 7, clipped; B7 carries those scaled entries.
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
    let b7 = SliceCfg { poc_lsb: 7, neg: vec![(-5, true)], pos: vec![], num_ref: Some((1, 1)), weights: /* SR1056 (was (-16, 5)) */ Some((6, vec![Some((-9, 3))])), weights_l1: /* SR1056 (was (-48, -3)) */ Some(vec![Some((64, 2))]), ..b2.clone() };
    s.extend(picture(&b7, |e| bi_body(e, 1, [16, 8], [32, 0])));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![3]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 16, forward_blocks: 16 }]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 4, 2, 6, 7], "output in decoding order (no reordering in the SPS)");
    let blend = |p: &Img, q: &Img, w0: u32, w1: u32| {
        let luma = p.y.iter().zip(q.y.iter()).map(|(&a, &b)| ((w0 * a as u32 + w1 * b as u32 + 12) >> 3) as u16).collect();
        let avg = |a: &[u16], b: &[u16]| a.iter().zip(b.iter()).map(|(&x, &y)| ((x as u32 + y as u32 + 1) >> 1) as u16).collect::<Vec<u16>>();
        Img { y: luma, cb: avg(&p.cb, &q.cb), cr: avg(&p.cr, &q.cr) }
    };
    assert_frame(by_poc(&f, 2), &blend(&shifted(&i0, 8, 4), &shifted(&i0, -16, 0), 3, 1), "B2: L0 weighted (48, 5), L1 (16, -3)");
    let src = img_of(by_poc(&f, 2));
    let (p, q) = (shifted(&src, 4, 2), shifted(&src, 8, 0));
    assert_ne!(p.y, q.y, "the fixture's two projections must differ");
    let b7f = img_of(by_poc(&f, 7));
    assert_frame(by_poc(&f, 6), &b7f, "P from the concealed picture equals the decoder's own weighted bi MC of B2 at the projected vectors");
    let scaled = |p: &Img, q: &Img| {
        let luma = p.y.iter().zip(q.y.iter()).map(|(&a, &b)| ((55 * a as u32 + 128 * b as u32 + 384) >> 7).min(255) as u16).collect();
        let avg = |a: &[u16], b: &[u16]| a.iter().zip(b.iter()).map(|(&x, &y)| ((x as u32 + y as u32 + 1) >> 1) as u16).collect::<Vec<u16>>();
        Img { y: luma, cb: avg(&p.cb, &q.cb), cr: avg(&p.cr, &q.cr) }
    };
    assert_frame(by_poc(&f, 6), &scaled(&p, &q), "SR1056 (was (3a + b + 12) >> 3): P from the concealed picture follows 8.5.3.3.4.3 with each list's weights scaled to its own ratio");
    assert_ne!(img_of(by_poc(&f, 6)).y, blend(&p, &q, 3, 1).y, "SR1056: not the lists' weights applied unscaled");
    let unweighted = p.y.iter().zip(q.y.iter()).map(|(&a, &b)| ((a as u32 + b as u32 + 1) >> 1) as u16).collect::<Vec<u16>>();
    assert_ne!(img_of(by_poc(&f, 6)).y, unweighted, "not the unweighted 8-262 average");
    assert_ne!(img_of(by_poc(&f, 6)).y, blend(&p, &q, 1, 3).y, "not the blend with the lists' weights swapped");
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
/// SR1056: at the ratio 1/2 the concealment scales luma (32, 10) to (45, 5), Cb (96, +20) to (111, +10) (coded (-17, -7)) and
/// Cr (160, -30) to (143, -15) (coded (15, 0)); P5 carries those scaled tables.
#[test]
fn conceal_weighted_chroma_applies_the_source_chroma_weights() {
    let i0 = gradient();
    let fade = Some((6, vec![Some((-32, 10))]));
    let chroma = Some((1, vec![Some([(-32, -12), (32, 2)])]));
    let mut s = headers(false, true, &PpsInter { weighted_pred: true, ..Default::default() });
    s.extend(idr(&i0, true));
    s.extend(picture(&SliceCfg { weights: fade.clone(), chroma_weights: chroma.clone(), ..p_slice(2, vec![(-2, true)]) }, |e| amvp_then_merge(e, [32, 16])));
    s.extend(picture(&SliceCfg { weights: Some((6, vec![None])), ..p_slice(4, vec![(-1, true), (-2, false)]) }, skip_picture));
    s.extend(picture(&SliceCfg { weights: /* SR1056 (was fade) */ Some((6, vec![Some((-19, 5))])), chroma_weights: /* SR1056 (was chroma) */ Some((1, vec![Some([(-17, -7), (15, 0)])])), ..p_slice(5, vec![(-3, true)]) }, |e| amvp_then_merge(e, [16, 8])));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![3]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4, 5]);
    let weighted = |r: &Img| Img {
        y: r.y.iter().map(|&v| ((v as i32 + 1) / 2 + 10).clamp(0, 255) as u16).collect(),
        cb: r.cb.iter().map(|&v| (((3 * v as i32 + 2) >> 2) + 20).clamp(0, 255) as u16).collect(),
        cr: r.cr.iter().map(|&v| (((5 * v as i32 + 2) >> 2) - 30).clamp(0, 255) as u16).collect(),
    };
    assert_frame(by_poc(&f, 2), &weighted(&shifted(&i0, 8, 4)), "P2: the gradient moved by (8, 4), luma (32, 10), Cb (96, +20), Cr (160, -30)");
    let p2 = img_of(by_poc(&f, 2));
    let scaled = |r: &Img| Img {
        y: r.y.iter().map(|&v| (((45 * v as i32 + 32) >> 6) + 5).clamp(0, 255) as u16).collect(),
        cb: r.cb.iter().map(|&v| (((111 * v as i32 + 64) >> 7) + 10).clamp(0, 255) as u16).collect(),
        cr: r.cr.iter().map(|&v| (((143 * v as i32 + 64) >> 7) - 15).clamp(0, 255) as u16).collect(),
    };
    let p5 = img_of(by_poc(&f, 5));
    assert_frame(by_poc(&f, 4), &p5, "P from the concealed picture equals the decoder's own weighted MC of P2 at (16, 8) on every plane");
    assert_frame(by_poc(&f, 4), &scaled(&shifted(&p2, 4, 2)), "SR1056 (was P2's own luma and chroma weights): P from the concealed picture follows 8.5.3.3.4.3 with them scaled to the ratio 1/2");
    assert_ne!(img_of(by_poc(&f, 4)).cb, weighted(&shifted(&p2, 4, 2)).cb, "SR1056: Cb not P2's chroma weights applied unscaled");
    let unit = shifted(&p2, 4, 2);
    assert_ne!(img_of(by_poc(&f, 4)).cb, unit.cb, "Cb is not the default-chroma-weight projection");
    assert_ne!(img_of(by_poc(&f, 4)).cr, unit.cr, "Cr is not the default-chroma-weight projection");
}

// ---- HEVCCONCEALWEIGHTEDBIDIRECT (LEDGER SR774): a bi source block with one long-term list conceals uni with the surviving list's weights (named tail block) ----

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
/// SR1056: the surviving list references POC 4 (s - r = -2, c - s = 1: ratio -1/2), so its weights scale: L1 (16, -3) ->
/// (128, 2), L0 (48, 5) -> (74, -3); P7 carries the scaled entry.
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
        let (live, dead) = if l1_lt { ((74, -3), wo[1]) } else { ((128, 2), wo[0]) }; // SR1056 (was wo[0] / wo[1]): the surviving list scaled by -1/2
        let was = if l1_lt { wo[0] } else { wo[1] };
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
        let uni = |r: &Img, (w, o): (i32, i32)| r.y.iter().map(|&a| (((64 * a as i32 * w + 2048) >> 12) + o).clamp(0, 255) as u16).collect::<Vec<u16>>();
        let want = Img { y: uni(&q, live), cb: q.cb.clone(), cr: q.cr.clone() };
        assert_ne!(uni(&q, live), uni(&q, dead), "{what}: the fixture's two lists' weights must differ");
        let p7 = img_of(by_poc(&f, 7));
        assert_frame(by_poc(&f, 6), &p7, &format!("{what}: P from the concealed picture equals the decoder's own uni weighted MC of B2 at (32, 0)"));
        assert_frame(by_poc(&f, 6), &want, &format!("{what}: P from the concealed picture follows 8-252 with the surviving list's weights"));
        assert_ne!(img_of(by_poc(&f, 6)).y, uni(&q, dead), "{what}: not uni through the other list's weights");
        assert_ne!(img_of(by_poc(&f, 6)).y, uni(&q, was), "{what}: SR1056: not the surviving list's weights applied unscaled");
        // 8.5.3.3.4.3 with the long-term list projected as if usable (td 2, tb 1: (16, 8) = (4, 2) samples), in list order.
        let (a0, a1) = if l1_lt { (&q, &p) } else { (&p, &q) };
        let blend = a0.y.iter().zip(a1.y.iter()).map(|(&x, &y)| ((64 * x as i32 * wo[0].0 + 64 * y as i32 * wo[1].0 + ((wo[0].1 + wo[1].1 + 1) << 12)) >> 13).clamp(0, 255) as u16).collect::<Vec<u16>>();
        assert_ne!(img_of(by_poc(&f, 6)).y, blend, "{what}: not the bi blend");
    }
}

// ---- HEVCCONCEALBIDIRECTFORWARD (LEDGER SR778): a weighted bi referencer with one long-term list refines the lost picture through the surviving list's inverse weights (named tail block) ----

/// The SPS carries long_term_ref_pics_present_flag; the PPS weighted_pred_flag and weighted_bipred_flag. IDR0 (the
/// gradient), P2 (references POC 0 with mv (6, 2), default table), C = B4 (StCurrBefore {3} with POC 2 kept unused, LtCurr
/// {POC 0 by PocLsbLt 0}: L0 = L1 = [the lost POC 3, POC 0 long-term], two references per list; luma_log2_weight_denom 6,
/// every L0 entry (48, 5), every L1 entry (16, -3); PRED_BI with one list to the long-term POC 0 (ref_idx 1, mvd (-64, 0))
/// and the other to POC 3 (ref_idx 0, mvd (32, 16))), P5 (skip-only, references POC 3 alone, default table). POC 3 is
/// generated and concealed from POC 2; when C ends, SR754's forward road re-projects it from C: `project_lists` drops the
/// long-term list, `forward_only` keeps the list to POC 3, -mv = (-8, -4) samples fetched from C, and HEVCCONCEALBIDIRECTFORWARD
/// applies the INVERSE of THAT list's (w, o) (C coded pred = w * L + o): p' = round half away (4096 (q - o) / w), L = (p' + 32)
/// >> 6, clipped; chroma (default weights) the shifted samples. So P5 (the refined picture's skip copy) is that hand inverse
/// of C moved back on every plane; it is neither the unweighted copy (SR754 before SR778) nor the inverse through the OTHER list.
#[test]
fn conceal_bi_direct_forward_refines_through_the_surviving_lists_inverse_weights() {
    let i0 = gradient();
    let (lt, st) = ([-64, 0], [32, 16]);
    // (w, o) of each list: L0 (64 - 16, 5), L1 (64 - 48, -3).
    let wo = [(48, 5), (16, -3)];
    let inv = |r: &[u16], (w, o): (i32, i32)| {
        r.iter()
            .map(|&q| {
                let n = 4096 * (q as i32 - o);
                let (n, d) = if w < 0 { (-n, -w) } else { (n, w) };
                let p = if n >= 0 { (n + d / 2) / d } else { -((-n + d / 2) / d) };
                ((p + 32) >> 6).clamp(0, 255) as u16
            })
            .collect::<Vec<u16>>()
    };
    for l1_lt in [false, true] {
        let mut s = vps(false);
        s.extend(nal(33, &sps_rbsp_ext(&sps_cfg(), &InterSps { long_term_refs: true, ..inter_sps(false) })));
        s.extend(nal(34, &pps_rbsp_ext(&PpsCfg { init_qp: 26, deblocking_disabled: true, ..Default::default() }, &PpsInter { weighted_pred: true, weighted_bipred: true, ..Default::default() })));
        s.extend(idr(&i0, true));
        s.extend(picture(&SliceCfg { weights: Some((6, vec![None])), lt_pocs: Some(vec![]), ..p_slice(2, vec![(-2, true)]) }, |e| amvp_then_merge(e, [6, 2])));
        let c = SliceCfg {
            nal_type: 1,
            slice_type: 0,
            poc_lsb: 4,
            neg: vec![(-1, true), (-2, false)],
            pos: vec![],
            lt_pocs: Some(vec![0]),
            num_ref: Some((2, 2)),
            weights: Some((6, vec![Some((-16, 5)), Some((-16, 5))])),
            weights_l1: Some(vec![Some((-48, -3)), Some((-48, -3))]),
            five_minus_max_num_merge_cand: 4,
            deblocking_disabled: true,
            ..Default::default()
        };
        let (refs, m0, m1) = if l1_lt { ([0, 1], st, lt) } else { ([1, 0], lt, st) };
        s.extend(picture(&c, |e| bi_ref_body(e, 2, refs, m0, m1)));
        s.extend(picture(&SliceCfg { weights: Some((6, vec![None])), lt_pocs: Some(vec![]), ..p_slice(5, vec![(-2, true)]) }, skip_picture));
        let mut d = Decoder::new();
        d.decode_annexb(&s).expect("decode past the lost picture");
        d.flush();
        let what = if l1_lt { "L1 long-term, L0 survives" } else { "L0 long-term, L1 survives" };
        let (live, dead) = if l1_lt { (wo[0], wo[1]) } else { (wo[1], wo[0]) };
        assert_eq!(d.take_generated_refs(), vec![3], "{what}");
        assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }], "{what}: every block of C refines");
        let f = d.take_frames();
        assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4, 5], "{what}: output in decoding order");
        let back = shifted(&img_of(by_poc(&f, 4)), -8, -4);
        let want = Img { y: inv(&back.y[..], live), cb: back.cb.clone(), cr: back.cr.clone() };
        assert_ne!(inv(&back.y[..], live), inv(&back.y[..], dead), "{what}: the fixture's two lists' inverses must differ");
        assert_ne!(want.y, back.y, "{what}: the fixture's inverse must differ from the copy");
        assert_frame(by_poc(&f, 5), &want, &format!("{what}: P from the picture refined from C through the surviving list's inverse weights"));
        assert_ne!(img_of(by_poc(&f, 5)).y, back.y, "{what}: not the unweighted copy of C");
        assert_ne!(img_of(by_poc(&f, 5)).y, inv(&back.y[..], dead), "{what}: not the inverse through the other list's weights");
    }
}

// ---- HEVCCONCEALFORWARDBIINVERSE (LEDGER SR784): a weighted bi referencer with both lists on the lost picture refines it through the joint inverse of 8-264 (named tail block) ----

/// The PPS carries weighted_bipred_flag (the P pictures carry no table). IDR0 (the gradient), P2 (references POC 0 with mv
/// (6, 2)), C = B4 (StCurrBefore {3} with POC 2 kept unused: L0 = L1 = [the lost POC 3], one reference per list;
/// luma_log2_weight_denom 6, L0 (48, 5), L1 (16, -3); PRED_BI with BOTH lists to POC 3 at the EQUAL vector (32, 16)), P5
/// (skip-only, references POC 3 alone). POC 3 is generated and concealed from POC 2; when C ends, SR754's forward road
/// re-projects it from C with both candidates kept (`forward_only`: both lists reference POC 3), the two fetches of C at
/// -mv = (-8, -4) samples are equal, and HEVCCONCEALFORWARDBIINVERSE applies the JOINT inverse of both lists' weights (C
/// coded pred = (64L * 48 + 64L * 16 + ((5 - 3 + 1) << 12)) >> 13): p' = round half away (4096 (2q - 2) / 64), L = (p' + 32)
/// >> 6, clipped; chroma (default weights) the shifted samples. So P5 (the refined picture's skip copy) is that hand joint
/// inverse of C moved back on every plane; it is neither the unweighted 8-262 average (the copy of C) nor either list's
/// uni inverse.
#[test]
fn conceal_forward_bi_inverse_joins_both_lists_weights() {
    let i0 = gradient();
    let st = [32, 16];
    // (w, o) of each list: L0 (64 - 16, 5), L1 (64 - 48, -3).
    let wo = [(48, 5), (16, -3)];
    let round = |n: i32, d: i32| {
        let (n, d) = if d < 0 { (-n, -d) } else { (n, d) };
        if n >= 0 { (n + d / 2) / d } else { -((-n + d / 2) / d) }
    };
    let joint = |r: &[u16]| r.iter().map(|&q| ((round(4096 * (2 * q as i32 - (wo[0].1 + wo[1].1)), wo[0].0 + wo[1].0) + 32) >> 6).clamp(0, 255) as u16).collect::<Vec<u16>>();
    let uni_inv = |r: &[u16], (w, o): (i32, i32)| r.iter().map(|&q| ((round(4096 * (q as i32 - o), w) + 32) >> 6).clamp(0, 255) as u16).collect::<Vec<u16>>();
    let mut s = headers(false, true, &PpsInter { weighted_bipred: true, ..Default::default() });
    s.extend(idr(&i0, true));
    s.extend(picture(&p_slice(2, vec![(-2, true)]), |e| amvp_then_merge(e, [6, 2])));
    let c = SliceCfg {
        nal_type: 1,
        slice_type: 0,
        poc_lsb: 4,
        neg: vec![(-1, true), (-2, false)],
        pos: vec![],
        num_ref: Some((1, 1)),
        weights: Some((6, vec![Some((-16, 5))])),
        weights_l1: Some(vec![Some((-48, -3))]),
        five_minus_max_num_merge_cand: 4,
        deblocking_disabled: true,
        ..Default::default()
    };
    s.extend(picture(&c, |e| bi_body(e, 1, st, st)));
    s.extend(picture(&p_slice(5, vec![(-2, true)]), skip_picture));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![3]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }], "every block of C refines");
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4, 5], "output in decoding order");
    let back = shifted(&img_of(by_poc(&f, 4)), -8, -4);
    let want = Img { y: joint(&back.y[..]), cb: back.cb.clone(), cr: back.cr.clone() };
    assert_ne!(want.y, back.y, "the fixture's joint inverse must differ from the copy");
    assert_ne!(want.y, uni_inv(&back.y[..], wo[0]), "the fixture's joint inverse must differ from L0's uni inverse");
    assert_ne!(want.y, uni_inv(&back.y[..], wo[1]), "the fixture's joint inverse must differ from L1's uni inverse");
    assert_frame(by_poc(&f, 5), &want, "P from the picture refined from C through the joint inverse of both lists' weights");
    assert_ne!(img_of(by_poc(&f, 5)).y, back.y, "not the unweighted 8-262 average (the copy of C)");
    assert_ne!(img_of(by_poc(&f, 5)).y, uni_inv(&back.y[..], wo[0]), "not L0's uni inverse");
    assert_ne!(img_of(by_poc(&f, 5)).y, uni_inv(&back.y[..], wo[1]), "not L1's uni inverse");
}

// ---- HEVCCONCEALWEIGHTEDCHROMAL1 (LEDGER SR793): L1 chroma weights through the surviving list and the joint inverse (named tail block) ----

/// SR774's fixture with explicit CHROMA weights on both lists of B2 (the kit's `chroma_weights` / `chroma_weights_l1`):
/// delta_chroma_log2_weight_denom 1 (ChromaLog2WeightDenom 7, log2WD 7 + 6 = 13), every L0 entry Cb (-32, -12) -> (96, +20),
/// Cr (32, 2) -> (160, -30), every L1 entry Cb (-64, -80) -> (64, -16), Cr (-16, 6) -> (112, +22) (7-56: o = 128 + doff - w).
/// The long-term list is dropped by `project_lists`, so POC 3 is concealed from POC 2 along the surviving list alone at
/// (8, 0) samples (chroma (4, 0)), and `conceal_fetch` weights planes 1 / 2 by THAT list's chroma (w, o) through
/// `explicit_weight_of` (c = 1 / 2, l = the surviving list): Cb / Cr = ((64a * w + 4096) >> 13) + o. P7 carries the
/// surviving list's luma and chroma weights, so P6 (the concealed picture's skip copy) is exactly P7 on every plane and
/// the hand equations; its chroma is neither the default-weight projection nor through the other list's chroma weights.
/// With L0 long-term (l1_lt false) this is the L1 chroma road.
#[test]
fn conceal_weighted_chroma_l1_through_the_surviving_list() {
    let i0 = gradient();
    let (lt, st) = ([32, 16], [-64, 0]);
    // (w, o) of each list: L0 (64 - 16, 5), L1 (64 - 48, -3).
    let wo = [(48, 5), (16, -3)];
    // Per list [(delta_chroma_weight, delta_chroma_offset); Cb, Cr] and the 7-56 (w, o) they give.
    let cd = [[(-32, -12), (32, 2)], [(-64, -80), (-16, 6)]];
    let cwo = [[(96, 20), (160, -30)], [(64, -16), (112, 22)]];
    // SR1056: the surviving list (to POC 4: ratio -1/2) scaled by dpb.rs `scale_weight`, coded for P7 and its 7-56 (w, o).
    let scd = [[(20, 10), (-14, 1)], [(53, 61), (9, -2)]];
    let scwo = [[(148, -10), (114, 15)], [(181, 8), (137, -11)]];
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
            chroma_weights: Some((1, vec![Some(cd[0]), Some(cd[0])])),
            chroma_weights_l1: Some(vec![Some(cd[1]), Some(cd[1])]),
            five_minus_max_num_merge_cand: 4,
            deblocking_disabled: true,
            ..Default::default()
        };
        let (refs, m0, m1) = if l1_lt { ([0, 1], st, lt) } else { ([1, 0], lt, st) };
        s.extend(picture(&b2, |e| bi_ref_body(e, 2, refs, m0, m1)));
        s.extend(picture(&SliceCfg { weights: Some((6, vec![None])), lt_pocs: Some(vec![]), ..p_slice(6, vec![(-3, true), (-4, false)]) }, skip_picture));
        let (sl, dl) = if l1_lt { (0, 1) } else { (1, 0) };
        let live = [(74, -3), (128, 2)][sl]; // SR1056 (was wo[sl])
        s.extend(picture(&SliceCfg { weights: Some((6, vec![Some((live.0 - 64, live.1))])), chroma_weights: Some((1, vec![Some(scd[sl])])) /* SR1056 (was cd[sl]) */, lt_pocs: Some(vec![]), ..p_slice(7, vec![(-5, true)]) }, |e| amvp_then_merge(e, [32, 0])));
        let mut d = Decoder::new();
        d.decode_annexb(&s).expect("decode past the lost picture");
        d.flush();
        let what = if l1_lt { "L1 long-term, L0 survives" } else { "L0 long-term, L1 survives" };
        assert_eq!(d.take_generated_refs(), vec![3], "{what}");
        assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }], "{what}: every block uni");
        let f = d.take_frames();
        assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 4, 2, 6, 7], "{what}: output in decoding order");
        let q = shifted(&img_of(by_poc(&f, 2)), 8, 0);
        // 8-252: luma log2WD 6 + 6 = 12, chroma 7 + 6 = 13, clipped.
        let uni = |r: &[u16], (w, o): (i32, i32)| r.iter().map(|&a| (((64 * a as i32 * w + 2048) >> 12) + o).clamp(0, 255) as u16).collect::<Vec<u16>>();
        let uni_c = |r: &[u16], (w, o): (i32, i32)| r.iter().map(|&a| (((64 * a as i32 * w + 4096) >> 13) + o).clamp(0, 255) as u16).collect::<Vec<u16>>();
        let want = Img { y: uni(&q.y[..], live), cb: uni_c(&q.cb[..], scwo[sl][0]), cr: uni_c(&q.cr[..], scwo[sl][1]) };
        assert_ne!(want.cb, uni_c(&q.cb[..], cwo[dl][0]), "{what}: the fixture's two lists' Cb weights must differ");
        assert_ne!(want.cr, uni_c(&q.cr[..], cwo[dl][1]), "{what}: the fixture's two lists' Cr weights must differ");
        let p7 = img_of(by_poc(&f, 7));
        assert_frame(by_poc(&f, 6), &p7, &format!("{what}: P from the concealed picture equals the decoder's own uni weighted MC of B2 at (32, 0) on every plane"));
        assert_frame(by_poc(&f, 6), &want, &format!("{what}: P from the concealed picture follows 8-252 with the surviving list's luma and chroma weights"));
        assert_ne!(img_of(by_poc(&f, 6)).y, uni(&q.y[..], wo[sl]), "{what}: SR1056: not the surviving list's luma weights applied unscaled");
        assert_ne!(img_of(by_poc(&f, 6)).cb, q.cb, "{what}: Cb is not the default-chroma-weight projection");
        assert_ne!(img_of(by_poc(&f, 6)).cr, q.cr, "{what}: Cr is not the default-chroma-weight projection");
        assert_ne!(img_of(by_poc(&f, 6)).cb, uni_c(&q.cb[..], cwo[dl][0]), "{what}: Cb not through the other list's chroma weights");
        assert_ne!(img_of(by_poc(&f, 6)).cr, uni_c(&q.cr[..], cwo[dl][1]), "{what}: Cr not through the other list's chroma weights");
    }
}

/// SR784's fixture with explicit CHROMA weights on both lists of C = B4 (the chroma tables of the test above: L0 Cb (96, +20),
/// Cr (160, -30); L1 Cb (64, -16), Cr (112, +22); ChromaLog2WeightDenom 7). Both lists reference the lost POC 3 at the equal
/// vector, so the forward road fetches C's chroma twice at -mv (whole-sample chroma (-4, -2)) and `conceal_fetch` applies
/// `bi_explicit_inv` to planes 1 / 2 with their own (w0, o0, 13) / (w1, o1, 13) from `explicit_weight_of` (the luma arm):
/// p = 64q, p' = round half away ((p << 8) - ((o0 + o1) << 13), w0 + w1) = round (8192 (2q - (o0 + o1)), w0 + w1), then
/// (p' + 32) >> 6, clipped. So P5 (the refined picture's skip copy) is that hand joint inverse of C moved back on every
/// plane; its chroma is neither the copy of C nor either list's chroma uni inverse.
#[test]
fn conceal_forward_bi_inverse_chroma_joins_both_lists() {
    let i0 = gradient();
    let st = [32, 16];
    // (w, o) of each list: L0 (64 - 16, 5), L1 (64 - 48, -3).
    let wo = [(48, 5), (16, -3)];
    // Per list [(delta_chroma_weight, delta_chroma_offset); Cb, Cr] and the 7-56 (w, o) they give.
    let cd = [[(-32, -12), (32, 2)], [(-64, -80), (-16, 6)]];
    let cwo = [[(96, 20), (160, -30)], [(64, -16), (112, 22)]];
    let round = |n: i32, d: i32| {
        let (n, d) = if d < 0 { (-n, -d) } else { (n, d) };
        if n >= 0 { (n + d / 2) / d } else { -((-n + d / 2) / d) }
    };
    let joint = |r: &[u16]| r.iter().map(|&q| ((round(4096 * (2 * q as i32 - (wo[0].1 + wo[1].1)), wo[0].0 + wo[1].0) + 32) >> 6).clamp(0, 255) as u16).collect::<Vec<u16>>();
    let joint_c = |r: &[u16], k: usize| r.iter().map(|&q| ((round(8192 * (2 * q as i32 - (cwo[0][k].1 + cwo[1][k].1)), cwo[0][k].0 + cwo[1][k].0) + 32) >> 6).clamp(0, 255) as u16).collect::<Vec<u16>>();
    let uni_inv_c = |r: &[u16], (w, o): (i32, i32)| r.iter().map(|&q| ((round(8192 * (q as i32 - o), w) + 32) >> 6).clamp(0, 255) as u16).collect::<Vec<u16>>();
    let mut s = headers(false, true, &PpsInter { weighted_bipred: true, ..Default::default() });
    s.extend(idr(&i0, true));
    s.extend(picture(&p_slice(2, vec![(-2, true)]), |e| amvp_then_merge(e, [6, 2])));
    let c = SliceCfg {
        nal_type: 1,
        slice_type: 0,
        poc_lsb: 4,
        neg: vec![(-1, true), (-2, false)],
        pos: vec![],
        num_ref: Some((1, 1)),
        weights: Some((6, vec![Some((-16, 5))])),
        weights_l1: Some(vec![Some((-48, -3))]),
        chroma_weights: Some((1, vec![Some(cd[0])])),
        chroma_weights_l1: Some(vec![Some(cd[1])]),
        five_minus_max_num_merge_cand: 4,
        deblocking_disabled: true,
        ..Default::default()
    };
    s.extend(picture(&c, |e| bi_body(e, 1, st, st)));
    s.extend(picture(&p_slice(5, vec![(-2, true)]), skip_picture));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![3]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }], "every block of C refines");
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4, 5], "output in decoding order");
    let back = shifted(&img_of(by_poc(&f, 4)), -8, -4);
    let want = Img { y: joint(&back.y[..]), cb: joint_c(&back.cb[..], 0), cr: joint_c(&back.cr[..], 1) };
    for (k, (got, copy)) in [(&want.cb, &back.cb), (&want.cr, &back.cr)].into_iter().enumerate() {
        assert_ne!(got, copy, "plane {}: the fixture's joint chroma inverse must differ from the copy", k + 1);
        assert_ne!(*got, uni_inv_c(&copy[..], cwo[0][k]), "plane {}: the fixture's joint chroma inverse must differ from L0's uni inverse", k + 1);
        assert_ne!(*got, uni_inv_c(&copy[..], cwo[1][k]), "plane {}: the fixture's joint chroma inverse must differ from L1's uni inverse", k + 1);
    }
    assert_frame(by_poc(&f, 5), &want, "P from the picture refined from C through the joint inverse of both lists' luma and chroma weights");
    assert_ne!(img_of(by_poc(&f, 5)).cb, back.cb, "Cb is not the copy of C");
    assert_ne!(img_of(by_poc(&f, 5)).cr, back.cr, "Cr is not the copy of C");
    assert_ne!(img_of(by_poc(&f, 5)).cb, uni_inv_c(&back.cb[..], cwo[0][0]), "Cb is not L0's chroma uni inverse");
    assert_ne!(img_of(by_poc(&f, 5)).cb, uni_inv_c(&back.cb[..], cwo[1][0]), "Cb is not L1's chroma uni inverse");
    assert_ne!(img_of(by_poc(&f, 5)).cr, uni_inv_c(&back.cr[..], cwo[0][1]), "Cr is not L0's chroma uni inverse");
    assert_ne!(img_of(by_poc(&f, 5)).cr, uni_inv_c(&back.cr[..], cwo[1][1]), "Cr is not L1's chroma uni inverse");
}

// ---- HEVCCONCEALWEIGHTEDCHROMA422 (LEDGER SR798): explicit chroma weights through the 4:2:2 concealment (named tail block) ----

/// SR798: every sample of the w x h plane `r` of component `c` fetched at `mv` (luma quarter-sample through [`hand_luma`],
/// chroma eighth-sample through the kit's `hand_chroma_wh`), its 14-bit intermediate mapped by `f`, clipped to 8 bits.
fn fetch_map(r: &[u8], c: usize, w: i32, h: i32, mv: [i32; 2], f: impl Fn(i32) -> i32) -> Vec<u8> {
    let r16: Vec<u16> = r.iter().map(|&v| v as u16).collect();
    (0..w * h)
        .map(|i| {
            let (x, y) = (i % w, i / w);
            let p = if c == 0 { hand_luma(&r16, x, y, mv) } else { hand_chroma_wh(&r16, w, h, x, y, mv) };
            f(p).clamp(0, 255) as u8
        })
        .collect()
}

/// SR798: n / d rounded half away from zero (inter.rs `uni_explicit_inv` / `bi_explicit_inv`'s division).
fn round_half_away(n: i64, d: i64) -> i64 {
    let (n, d) = if d < 0 { (-n, -d) } else { (n, d) };
    if n >= 0 { (n + d / 2) / d } else { -((-n + d / 2) / d) }
}

/// SR776's P shape at chroma_format_idc 2 (4:2:2: 16x32 chroma) with SR758's fractional vectors. The PPS carries
/// weighted_pred_flag; IDR0 ([`head_fmt`]), P2 (references POC 0 with mv (6, 2)), P4 (skip-only, references the lost POC 3,
/// keeps POC 2 unused; its one entry defaults: a copy), P5 (references POC 2 with mv (3, 1)). P2 and P5 carry SR741's luma
/// fade (denom 6, LumaWeightL0[0] 32, offset 10) and SR776's chroma weights (ChromaLog2WeightDenom 7, Cb (96, +20), Cr
/// (160, -30)). POC 3 is concealed from POC 2 along (6, 2) rescaled by td 2, tb 1 to (3, 1); dpb.rs `conceal_fetch` scales
/// it to the chroma mvC (3 * 2 / 2, 1 * 2 / 1) = (3, 2) over 4x8 chroma blocks and weights planes 1 / 2 by their own
/// (w, o, log2WD 13) through `explicit_weight_of`. So P4 (the concealed picture's copy) is exactly P5 on every plane and the
/// hand 8-252 ((p * w + 4096) >> 13) + o over 8.5.3.3.3.2 at (3, 2); its chroma is neither the default weights nor the
/// 4:2:0 arm's mvC (3, 1).
#[test]
fn conceal_weighted_chroma_422_l0() {
    conceal_weighted_chroma_l0_at(2);
}

/// SR802: the body of [`conceal_weighted_chroma_422_l0`] at chroma_format_idc `idc` (2 or 3; geometry and mvC from [`fmt_geom`] / [`mvc_at`]).
fn conceal_weighted_chroma_l0_at(idc: u32) {
    let (sub, (cw, ch), prev) = fmt_geom(idc);
    let (mvc, mvp) = (mvc_at([3, 1], sub), mvc_at([3, 1], prev));
    let fade = Some((6, vec![Some((-32, 10))]));
    let chroma = Some((1, vec![Some([(-32, -12), (32, 2)])]));
    let mut s = head_fmt(idc, &inter_sps(false), &PpsInter { weighted_pred: true, ..Default::default() });
    s.extend(picture(&SliceCfg { weights: fade.clone(), chroma_weights: chroma.clone(), ..p_slice(2, vec![(-2, true)]) }, |e| amvp_then_merge(e, [6, 2])));
    s.extend(picture(&SliceCfg { weights: Some((6, vec![None])), ..p_slice(4, vec![(-1, true), (-2, false)]) }, skip_picture));
    s.extend(picture(&SliceCfg { weights: /* SR1056 (was fade) */ Some((6, vec![Some((-19, 5))])), chroma_weights: /* SR1056 (was chroma) */ Some((1, vec![Some([(-17, -7), (15, 0)])])), ..p_slice(5, vec![(-3, true)]) }, |e| amvp_then_merge(e, [3, 1])));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![3]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4, 5]);
    let (p2, p4, p5) = (by_poc(&f, 2), by_poc(&f, 4), by_poc(&f, 5));
    for c in 0..3 {
        assert_eq!(p4.planes[c].len(), if c == 0 { 32 * 32 } else { (cw * ch) as usize }, "idc {idc} plane {c}: the format's sample count");
        assert_eq!(p4.planes[c], p5.planes[c], "plane {c}: the concealed picture equals the decoder's own weighted MC of P2 at (3, 1)");
    }
    assert_eq!(p4.planes[0], fetch_map(&p2.planes[0], 0, 32, 32, [3, 1], |p| ((p * 45 + 2048) >> 12) + 5), "SR1056 (was (32, 10)): luma: 8-252 with P2's fade scaled to (45, 5) at (3, 1)");
    for (k, (w, o)) in /* SR1056 (was [(96, 20), (160, -30)]) */ [(111, 10), (143, -15)].into_iter().enumerate() {
        let (c, r) = (k + 1, &p2.planes[k + 1][..]);
        let wt = move |p: i32| ((p * w + 4096) >> 13) + o;
        assert_eq!(p4.planes[c], fetch_map(r, c, cw, ch, mvc, wt), "idc {idc} plane {c}: 8-252 with P2's chroma weights over 8.5.3.3.3.2 at mvC {mvc:?}");
        assert_ne!(p4.planes[c], fetch_map(r, c, cw, ch, mvc, |p| (p + 32) >> 6), "idc {idc} plane {c}: not the default-chroma-weight projection");
        assert_ne!(p4.planes[c], fetch_map(r, c, cw, ch, mvp, wt), "idc {idc} plane {c}: not the previous format's mvC {mvp:?}");
        if idc == 3 {
            assert_ne!(p4.planes[c], fetch_map(r, 0, 32, 32, [3, 1], wt), "4:4:4 plane {c}: not the luma 8-tap at quarter units");
        }
    }
}

/// SR793's surviving-list fixture at chroma_format_idc 2: B2 (L0 = L1 = [POC 4, POC 0 long-term], every L0 entry luma
/// (48, 5) Cb (96, +20) Cr (160, -30), every L1 entry luma (16, -3) Cb (64, -16) Cr (112, +22), ChromaLog2WeightDenom 7);
/// PRED_BI with one list to the long-term POC 0 (mvd (32, 16)) and the other to POC 4 at (-6, -2) — SR793's (-64, 0) is
/// horizontal only, its chroma (4, 0) at 4:2:0 and 4:2:2 alike. `project_lists` drops the long-term list, so POC 3 is
/// concealed along the surviving list alone: (-6, -2) rescaled by td -2, tb 1 to (3, 1), chroma mvC (3, 2), weighted uni by
/// THAT list's luma and chroma (w, o). P7 references POC 2 at (3, 1) with the surviving list's weights, so P6 (the concealed
/// picture's skip copy) is exactly P7 on every plane and the hand 8-252; its chroma is neither the default weights, nor the
/// other list's chroma, nor the 4:2:0 arm's mvC (3, 1). With L0 long-term (l1_lt false) this is the L1 chroma road.
#[test]
fn conceal_weighted_chroma_422_l1_surviving() {
    conceal_weighted_chroma_l1_surviving_at(2);
}

/// SR802: the body of [`conceal_weighted_chroma_422_l1_surviving`] at chroma_format_idc `idc` (2 or 3).
fn conceal_weighted_chroma_l1_surviving_at(idc: u32) {
    let (sub, (cw, ch), prev) = fmt_geom(idc);
    let (mvc, mvp) = (mvc_at([3, 1], sub), mvc_at([3, 1], prev));
    let (lt, st) = ([32, 16], [-6, -2]);
    // (w, o) of each list: L0 (64 - 16, 5), L1 (64 - 48, -3).
    let wo = [(48, 5), (16, -3)];
    // Per list [(delta_chroma_weight, delta_chroma_offset); Cb, Cr] and the 7-56 (w, o) they give.
    let cd = [[(-32, -12), (32, 2)], [(-64, -80), (-16, 6)]];
    let cwo = [[(96, 20), (160, -30)], [(64, -16), (112, 22)]];
    // SR1056: the surviving list (to POC 4: ratio -1/2) scaled by dpb.rs `scale_weight`, coded for P7 and its 7-56 (w, o).
    let scd = [[(20, 10), (-14, 1)], [(53, 61), (9, -2)]];
    let scwo = [[(148, -10), (114, 15)], [(181, 8), (137, -11)]];
    let wt = |(w, o): (i32, i32)| move |p: i32| ((p * w + 4096) >> 13) + o;
    for l1_lt in [false, true] {
        let mut s = head_fmt(idc, &InterSps { long_term_refs: true, ..inter_sps(false) }, &PpsInter { weighted_pred: true, weighted_bipred: true, ..Default::default() });
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
            chroma_weights: Some((1, vec![Some(cd[0]), Some(cd[0])])),
            chroma_weights_l1: Some(vec![Some(cd[1]), Some(cd[1])]),
            five_minus_max_num_merge_cand: 4,
            deblocking_disabled: true,
            ..Default::default()
        };
        let (refs, m0, m1) = if l1_lt { ([0, 1], st, lt) } else { ([1, 0], lt, st) };
        s.extend(picture(&b2, |e| bi_ref_body(e, 2, refs, m0, m1)));
        s.extend(picture(&SliceCfg { weights: Some((6, vec![None])), lt_pocs: Some(vec![]), ..p_slice(6, vec![(-3, true), (-4, false)]) }, skip_picture));
        let (sl, dl) = if l1_lt { (0, 1) } else { (1, 0) };
        let live = [(74, -3), (128, 2)][sl]; // SR1056 (was wo[sl])
        s.extend(picture(&SliceCfg { weights: Some((6, vec![Some((live.0 - 64, live.1))])), chroma_weights: Some((1, vec![Some(scd[sl])])) /* SR1056 (was cd[sl]) */, lt_pocs: Some(vec![]), ..p_slice(7, vec![(-5, true)]) }, |e| amvp_then_merge(e, [3, 1])));
        let mut d = Decoder::new();
        d.decode_annexb(&s).expect("decode past the lost picture");
        d.flush();
        let what = if l1_lt { "L1 long-term, L0 survives" } else { "L0 long-term, L1 survives" };
        assert_eq!(d.take_generated_refs(), vec![3], "{what}");
        assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }], "{what}: every block uni");
        let f = d.take_frames();
        assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 4, 2, 6, 7], "{what}: output in decoding order");
        let (p2, p6, p7) = (by_poc(&f, 2), by_poc(&f, 6), by_poc(&f, 7));
        for c in 0..3 {
            assert_eq!(p6.planes[c].len(), if c == 0 { 32 * 32 } else { (cw * ch) as usize }, "{what}: idc {idc} plane {c}: the format's sample count");
            assert_eq!(p6.planes[c], p7.planes[c], "{what}: plane {c}: the concealed picture equals the decoder's own uni weighted MC of B2 at (3, 1)");
        }
        assert_eq!(p6.planes[0], fetch_map(&p2.planes[0], 0, 32, 32, [3, 1], |p| ((p * live.0 + 2048) >> 12) + live.1), "{what}: luma 8-252 with the surviving list's weights");
        assert_ne!(p6.planes[0], fetch_map(&p2.planes[0], 0, 32, 32, [3, 1], |p| ((p * wo[sl].0 + 2048) >> 12) + wo[sl].1), "{what}: SR1056: not the surviving list's luma weights applied unscaled");
        for k in 0..2 {
            let (c, r) = (k + 1, &p2.planes[k + 1][..]);
            assert_eq!(p6.planes[c], fetch_map(r, c, cw, ch, mvc, wt(scwo[sl][k])), "{what}: idc {idc} plane {c}: 8-252 with the surviving list's chroma weights at mvC {mvc:?}");
            assert_ne!(p6.planes[c], fetch_map(r, c, cw, ch, mvc, |p| (p + 32) >> 6), "{what}: idc {idc} plane {c}: not the default-chroma-weight projection");
            assert_ne!(p6.planes[c], fetch_map(r, c, cw, ch, mvc, wt(cwo[dl][k])), "{what}: idc {idc} plane {c}: not through the other list's chroma weights");
            assert_ne!(p6.planes[c], fetch_map(r, c, cw, ch, mvp, wt(scwo[sl][k])), "{what}: idc {idc} plane {c}: not the previous format's mvC {mvp:?}");
            if idc == 3 {
                assert_ne!(p6.planes[c], fetch_map(r, 0, 32, 32, [3, 1], wt(scwo[sl][k])), "{what}: 4:4:4 plane {c}: not the luma 8-tap at quarter units");
            }
        }
    }
}

/// SR793's joint-inverse fixture at chroma_format_idc 2: C = B4 (L0 = L1 = [the lost POC 3], one reference per list; luma
/// L0 (48, 5) L1 (16, -3) at denom 6; chroma L0 Cb (96, +20) Cr (160, -30), L1 Cb (64, -16) Cr (112, +22) at
/// ChromaLog2WeightDenom 7) PRED_BI with BOTH lists to POC 3 at the equal fractional vector (6, 2). When C ends, the forward
/// road refines POC 3 from C at -mv = (-6, -2) (chroma mvC (-6, -4) over 4x8 blocks), both fetches equal, through
/// `bi_explicit_inv` per plane with that plane's two (w, o) from `explicit_weight_of`: p' = round half away
/// (((p << (denom + 1)) - ((o0 + o1) << log2WD)) / (w0 + w1)), then (p' + 32) >> 6, clipped, on the 14-bit 8.5.3.3.3.2 fetch p.
/// So P5 (the refined picture's skip copy) is that hand joint inverse on every plane; its chroma is neither the copy of C,
/// nor either list's chroma uni inverse, nor the 4:2:0 arm's mvC (-6, -2).
#[test]
fn conceal_forward_bi_inverse_chroma_422_joins_both_lists() {
    conceal_forward_bi_inverse_chroma_joins_both_lists_at(2);
}

/// SR802: the body of [`conceal_forward_bi_inverse_chroma_422_joins_both_lists`] at chroma_format_idc `idc` (2 or 3).
fn conceal_forward_bi_inverse_chroma_joins_both_lists_at(idc: u32) {
    let (sub, (cw, ch), prev) = fmt_geom(idc);
    let (mvc, mvp) = (mvc_at([-6, -2], sub), mvc_at([-6, -2], prev));
    let st = [6, 2];
    // (w, o) of each list: L0 (64 - 16, 5), L1 (64 - 48, -3).
    let wo = [(48, 5), (16, -3)];
    // Per list [(delta_chroma_weight, delta_chroma_offset); Cb, Cr] and the 7-56 (w, o) they give.
    let cd = [[(-32, -12), (32, 2)], [(-64, -80), (-16, 6)]];
    let cwo = [[(96, 20), (160, -30)], [(64, -16), (112, 22)]];
    // `bi_explicit_inv` / `uni_explicit_inv` at 8 bits (shift1 6, log2WD = denom + 6) on the 14-bit fetch p.
    let joint = |denom: u32, a: (i32, i32), b: (i32, i32)| move |p: i32| (round_half_away(((p as i64) << (denom + 1)) - (((a.1 + b.1) as i64) << (denom + 6)), (a.0 + b.0) as i64) as i32 + 32) >> 6;
    let uni_inv = |denom: u32, (w, o): (i32, i32)| move |p: i32| (round_half_away(((p as i64) - ((o as i64) << 6)) << denom, w as i64) as i32 + 32) >> 6;
    let mut s = head_fmt(idc, &inter_sps(false), &PpsInter { weighted_bipred: true, ..Default::default() });
    s.extend(picture(&p_slice(2, vec![(-2, true)]), |e| amvp_then_merge(e, [6, 2])));
    let c4 = SliceCfg {
        nal_type: 1,
        slice_type: 0,
        poc_lsb: 4,
        neg: vec![(-1, true), (-2, false)],
        pos: vec![],
        num_ref: Some((1, 1)),
        weights: Some((6, vec![Some((-16, 5))])),
        weights_l1: Some(vec![Some((-48, -3))]),
        chroma_weights: Some((1, vec![Some(cd[0])])),
        chroma_weights_l1: Some(vec![Some(cd[1])]),
        five_minus_max_num_merge_cand: 4,
        deblocking_disabled: true,
        ..Default::default()
    };
    s.extend(picture(&c4, |e| bi_body(e, 1, st, st)));
    s.extend(picture(&p_slice(5, vec![(-2, true)]), skip_picture));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![3]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }], "every block of C refines");
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4, 5], "output in decoding order");
    let (c, p5) = (by_poc(&f, 4), by_poc(&f, 5));
    assert_eq!(p5.planes[1].len(), (cw * ch) as usize, "idc {idc}: the format's chroma sample count");
    assert_eq!(p5.planes[0], fetch_map(&c.planes[0], 0, 32, 32, [-6, -2], joint(6, wo[0], wo[1])), "luma: the joint inverse of both lists' weights of C at -mv");
    for k in 0..2 {
        let (pl, r) = (k + 1, &c.planes[k + 1][..]);
        let want = fetch_map(r, pl, cw, ch, mvc, joint(7, cwo[0][k], cwo[1][k]));
        assert_eq!(p5.planes[pl], want, "idc {idc} plane {pl}: the joint inverse of both lists' chroma weights of C at mvC {mvc:?}");
        assert_ne!(p5.planes[pl], fetch_map(r, pl, cw, ch, mvc, |p| (p + 32) >> 6), "idc {idc} plane {pl}: not the copy of C");
        assert_ne!(p5.planes[pl], fetch_map(r, pl, cw, ch, mvc, uni_inv(7, cwo[0][k])), "idc {idc} plane {pl}: not L0's chroma uni inverse");
        assert_ne!(p5.planes[pl], fetch_map(r, pl, cw, ch, mvc, uni_inv(7, cwo[1][k])), "idc {idc} plane {pl}: not L1's chroma uni inverse");
        assert_ne!(p5.planes[pl], fetch_map(r, pl, cw, ch, mvp, joint(7, cwo[0][k], cwo[1][k])), "idc {idc} plane {pl}: not the previous format's mvC {mvp:?}");
        if idc == 3 {
            assert_ne!(p5.planes[pl], fetch_map(r, 0, 32, 32, [-6, -2], joint(7, cwo[0][k], cwo[1][k])), "4:4:4 plane {pl}: not the luma 8-tap at quarter units");
        }
    }
}

// ---- HEVCCONCEALWEIGHTEDCHROMA444 (LEDGER SR802): the three weighted chroma concealment pins at chroma_format_idc 3 (named tail block) ----

/// SR802: of chroma_format_idc `idc` (2 or 3) on the 32x32 fixtures: (SubWidthC, SubHeightC), the chroma plane (w, h), and the
/// PREVIOUS format's (SubWidthC, SubHeightC) whose mvC each pin must not match (2: 4:2:0's (2, 2); 3: 4:2:2's (2, 1)).
fn fmt_geom(idc: u32) -> ((i32, i32), (i32, i32), (i32, i32)) {
    let (sub, prev) = if idc == 2 { ((2, 1), (2, 2)) } else { ((1, 1), (2, 1)) };
    (sub, (32 / sub.0, 32 / sub.1), prev)
}

/// SR802: the chroma mvC of the quarter-sample luma vector `mv` at (SubWidthC, SubHeightC) `sub`: dpb.rs `conceal_fetch`'s
/// `mv * 2 / sub` (eighth units, truncating like the decoder). At 4:4:4 it is mv x 2: even eighths through the 4-tap chroma filter.
fn mvc_at(mv: [i32; 2], sub: (i32, i32)) -> [i32; 2] {
    [mv[0] * 2 / sub.0, mv[1] * 2 / sub.1]
}

/// SR802: SR798's L0 weighted chroma fixture at chroma_format_idc 3 (separate_colour_plane_flag 0): 32x32 chroma, the concealed
/// block 8x8 on every plane (`conceal_motion`'s (x / 1, y / 1, bw, bh)), mvC (6, 2) through `chroma_mc`'s 4-tap filter. Not 4:2:2's
/// mvC (3, 2): the discriminating component is the HORIZONTAL one (4:2:2 already has full vertical resolution); not the luma 8-tap.
#[test]
fn conceal_weighted_chroma_444_l0() {
    assert!(!sps_cfg().separate_colour_plane, "the idc-3 SPS writes separate_colour_plane_flag 0");
    conceal_weighted_chroma_l0_at(3);
}

/// SR802: SR798's surviving-list B fixture at chroma_format_idc 3: the short-term (-6, -2) projects to (3, 1), mvC (6, 2), weighted
/// uni by the surviving list's chroma; not the other list, not 4:2:2's mvC (3, 2), not the luma 8-tap.
#[test]
fn conceal_weighted_chroma_444_l1_surviving() {
    assert!(!sps_cfg().separate_colour_plane, "the idc-3 SPS writes separate_colour_plane_flag 0");
    conceal_weighted_chroma_l1_surviving_at(3);
}

/// SR802: SR798's equal-vector forward referencer at chroma_format_idc 3: the joint `bi_explicit_inv` over the 4:4:4 fetch of C at
/// mvC (-12, -4); not either list's uni inverse, not 4:2:2's mvC (-6, -4), not the luma 8-tap.
#[test]
fn conceal_forward_bi_inverse_chroma_444_joins_both_lists() {
    assert!(!sps_cfg().separate_colour_plane, "the idc-3 SPS writes separate_colour_plane_flag 0");
    conceal_forward_bi_inverse_chroma_joins_both_lists_at(3);
}

// ---- HEVCTESTKITBBODY (LEDGER SR833): the kit's B body equals the inline B CUs it replaced (named tail block) ----

/// SR833: the two B coding units inter.rs wrote inline (`b_picture_bi_predicts_the_rounded_average`'s B4 with zero mvds,
/// `bidir_conceal_stream`'s PRED_BI B2 with L0 (32, 16), L1 (-64, 0)) were captured as the bytes those tests fed the
/// decoder before they moved onto the kit's [`bi_body`]; the kit's writer must reproduce them exactly.
#[test]
fn kit_b_body_equals_the_inline_b_cus() {
    const PIN_A: &[u8] = &[0, 0, 0, 1, 2, 1, 224, 132, 137, 38, 144, 184, 48, 252, 18, 128];
    const PIN_B: &[u8] = &[0, 0, 0, 1, 2, 1, 224, 68, 149, 105, 11, 128, 43, 156, 132, 62, 204, 100, 169, 24];
    for (poc, m0, m1, pin) in [(4u32, [0, 0], [0, 0], PIN_A), (2, [32, 16], [-64, 0], PIN_B)] {
        let cfg = SliceCfg {
            nal_type: 1,
            slice_type: 0,
            poc_lsb: poc,
            neg: vec![(-(poc as i32), true)],
            pos: vec![(poc as i32, true)],
            num_ref: Some((2, 2)),
            five_minus_max_num_merge_cand: 4,
            deblocking_disabled: true,
            ..Default::default()
        };
        assert_eq!(picture(&cfg, |e| bi_body(e, 2, m0, m1)), pin, "B POC {poc}");
    }
}

// ---- HEVCCONCEALBI444 (LEDGER SR845): bi concealment averaged on every chroma sample at chroma_format_idc 1 / 2 / 3 (named tail block) ----

/// SR845: [`bidir_conceal_stream`]'s PRED_BI shape over the kit's [`head_fmt`] at chroma_format_idc `idc` (the non-linear
/// pcm pattern): IDR0, P4 (skip-only: I0), B2 (StCurrBefore {0}, StCurrAfter {4}, two references per list; CTB 0 PRED_BI
/// through the kit's `bi_body` with L0 mvd (32, 16) to POC 0 and L1 mvd (-64, 0) to POC 4, CTBs 1..3 skip-merge it),
/// then P6 (skip-only, references only the lost POC 3). Returns the stream and the byte range of B2's NAL.
fn bidir_conceal_stream_fmt(idc: u32) -> (Vec<u8>, std::ops::Range<usize>) {
    let mut s = head_fmt(idc, &inter_sps(false), &PpsInter::default());
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
    let start = s.len();
    s.extend(picture(&bcfg, |e| bi_body(e, 2, [32, 16], [-64, 0])));
    let b2 = start..s.len();
    s.extend(picture(&p_slice(6, vec![(-3, true)]), skip_picture));
    (s, b2)
}

/// SR845: [`conceal_bidir_averages_both_projections`] at chroma_format_idc `idc`. POC 3 is concealed from B2 along both
/// lists: L0 (32, 16) rescaled by td 2, tb 1 to (16, 8) quarter samples, L1 (-64, 0) by td -2, tb 1 to (32, 0); dpb.rs
/// `conceal_fetch` takes chroma mvC = mv * 2 / (SubWidthC, SubHeightC) ([`mvc_at`]): whole chroma samples on every format
/// (4:2:0 (2, 1) / (4, 0), 4:2:2 (2, 2) / (4, 0), 4:4:4 (4, 2) / (8, 0)), so each 14-bit fetch is s << 6 and 8-262's
/// `bi_default` is (s + t + 1) >> 1 exactly. P6 (the concealed picture's skip copy) equals that average of the two fetches
/// of B2 on EVERY sample of every plane (the 4:4:4 concealed chroma block is 8x8 = 64: `second` filled whole), and differs
/// from either fetch alone and (idc 2 / 3) from the previous format's mvC.
fn conceal_bidir_averages_both_projections_at(idc: u32) {
    let (sub, (cw, ch), prev) = if idc == 1 { ((2, 2), (16, 16), (2, 2)) } else { fmt_geom(idc) };
    let (s, _) = bidir_conceal_stream_fmt(idc);
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![3], "idc {idc}");
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 16, forward_blocks: 16 }], "idc {idc}: every block bi");
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 4, 2, 6], "idc {idc}: output in decoding order");
    let (b2, p6) = (by_poc(&f, 2), by_poc(&f, 6));
    let avg = |a: &[u8], b: &[u8]| a.iter().zip(b.iter()).map(|(&x, &y)| ((x as u32 + y as u32 + 1) >> 1) as u8).collect::<Vec<u8>>();
    let whole = |p: i32| (p + 32) >> 6;
    let (pl, ql) = (fetch_map(&b2.planes[0], 0, 32, 32, [16, 8], whole), fetch_map(&b2.planes[0], 0, 32, 32, [32, 0], whole));
    assert_eq!(p6.planes[0], avg(&pl, &ql), "idc {idc}: luma, the 8-262 average of (4, 2) and (8, 0)");
    assert_ne!(p6.planes[0], pl, "idc {idc}: luma not the L0 projection alone");
    assert_ne!(p6.planes[0], ql, "idc {idc}: luma not the L1 projection alone");
    let (m0, m1) = (mvc_at([16, 8], sub), mvc_at([32, 0], sub));
    for c in 1..3 {
        let r = &b2.planes[c][..];
        assert_eq!(p6.planes[c].len(), (cw * ch) as usize, "idc {idc} plane {c}: the format's sample count");
        let (p, q) = (fetch_map(r, c, cw, ch, m0, whole), fetch_map(r, c, cw, ch, m1, whole));
        let want = avg(&p, &q);
        for i in 0..want.len() {
            assert_eq!(p6.planes[c][i], want[i], "idc {idc} plane {c} sample ({}, {}): the average of mvC {m0:?} and {m1:?}", i as i32 % cw, i as i32 / cw);
        }
        assert_ne!(p6.planes[c], p, "idc {idc} plane {c}: not the L0 projection alone");
        assert_ne!(p6.planes[c], q, "idc {idc} plane {c}: not the L1 projection alone");
        if idc != 1 {
            let (n0, n1) = (mvc_at([16, 8], prev), mvc_at([32, 0], prev));
            assert_ne!(p6.planes[c], avg(&fetch_map(r, c, cw, ch, n0, whole), &fetch_map(r, c, cw, ch, n1, whole)), "idc {idc} plane {c}: not the previous format's mvC {n0:?} / {n1:?}");
        }
    }
}

/// SR845: the bi concealment at 4:2:0 over the kit's non-linear pattern (the gradient pin above unchanged).
#[test]
fn conceal_bidir_averages_both_projections_420_pattern() {
    conceal_bidir_averages_both_projections_at(1);
}

/// SR845: the bi concealment at 4:2:2: 4x8 chroma blocks, both fetches averaged on every chroma sample.
#[test]
fn conceal_bidir_averages_both_projections_422() {
    conceal_bidir_averages_both_projections_at(2);
}

/// SR845: the bi concealment at 4:4:4: 8x8 chroma blocks, `conceal_fetch`'s 64-sample `second` filled whole.
#[test]
fn conceal_bidir_averages_both_projections_444() {
    assert!(!sps_cfg().separate_colour_plane, "the idc-3 SPS writes separate_colour_plane_flag 0");
    conceal_bidir_averages_both_projections_at(3);
}

/// SR845: the fuzz-shape pin on the 4:4:4 bi concealment road: each byte of B2's slice NAL after its start code and
/// two-byte header, xor-ed with 0xff and with 0x01 in turn, decodes (Ok or Err), flushes and yields its frames without
/// a panic.
#[test]
fn conceal_bidir_444_damaged_b_decodes_without_panic() {
    let (s, b2) = bidir_conceal_stream_fmt(3);
    for i in b2.start + 6..b2.end {
        for mask in [0xffu8, 0x01] {
            let mut t = s.clone();
            t[i] ^= mask;
            let mut d = Decoder::new();
            let _ = d.decode_annexb(&t);
            d.flush();
            let _ = d.take_frames();
        }
    }
}

// ---- HEVCCONCEALMAIN10PIN (LEDGER SR1027): the concealment roads at 10 bits (named tail block) ----

/// SR1027: a Main10 gradient whose samples all sit above 255 (an 8-bit clamp would flatten every one) with a corner
/// saturated at 1023 (luma and Cb): luma min(1023, 300 + 19x + 11y), Cb min(1023, 600 + 30x + 10y), Cr 900 - 7x - 13y.
fn gradient10() -> Img {
    Img::new(|x, y| (300 + x * 19 + y * 11).min(1023) as u16, |x, y| (600 + x * 30 + y * 10).min(1023) as u16, |x, y| (900 - x * 7 - y * 13) as u16)
}

/// SR1027: [`headers`] at Main10 through the kit's own knob (`SpsCfg.bit_depth` 10, pcm (10, 10, 3, 4)), no TMVP,
/// deblocking off.
fn headers10(pi: &PpsInter) -> Vec<u8> {
    let mut s = vps(false);
    s.extend(nal(33, &sps_rbsp_ext(&SpsCfg { bit_depth: 10, pcm: Some((10, 10, 3, 4, true)), ..sps_cfg() }, &inter_sps(false))));
    s.extend(nal(34, &pps_rbsp_ext(&PpsCfg { init_qp: 26, deblocking_disabled: true, ..Default::default() }, pi)));
    s
}

/// SR1027: [`idr`] at 10 bits: the four CTBs as 16x16 pcm coding units through the kit's `pcm_cu_bits` (10, 10).
fn idr10(img: &Img) -> Vec<u8> {
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 26);
    for k in 0..4 {
        let (y, cb, cr) = img.ctb(k % 2, k / 2);
        e.pcm_cu_bits(0, 4, 10, 10, &y, &cb, &cr);
        if k < 3 {
            e.terminate(0);
        }
    }
    nal(19, &e.finish().out)
}

/// SR1027: a Main10 frame's 10-bit samples (`planes16`) as an [`Img`].
fn img16_of(f: &Frame) -> Img {
    let p = f.planes16.as_ref().expect("planes16 at 10 bits");
    Img { y: p[0].clone(), cb: p[1].clone(), cr: p[2].clone() }
}

/// SR1027: [`assert_frame`] at 10 bits: bit_depth 10, `planes16` equal to `want`, and the 8-bit `planes` its >> 2.
fn assert_frame16(f: &Frame, want: &Img, what: &str) {
    assert_eq!(f.bit_depth, 10, "{what}: a Main10 frame");
    let got = img16_of(f);
    for (c, (g, w)) in got.planes().into_iter().zip(want.planes()).enumerate() {
        let n = if c == 0 { W } else { W / 2 };
        for i in 0..n * n {
            assert_eq!(g[i], w[i], "{what}: plane {c} sample ({}, {})", i % n, i / n);
            assert_eq!(f.planes[c][i], (w[i] >> 2) as u8, "{what}: plane {c} narrowed ({}, {})", i % n, i / n);
        }
    }
}

/// SR1027: [`conceal_projection_scales_by_poc_distance`] at 10 bits: POC 3 is concealed from P2 along (32, 16) rescaled
/// to (16, 8) = (4, 2) samples through `conceal_fetch` at bd 10, so P4 is P2 advanced by (4, 2) on the 10-bit planes:
/// every sample stays above 255 and the saturated corner reads 1023 (the clamp (1 << 10) - 1), not 255.
#[test]
fn conceal_main10_projection_keeps_high_samples() {
    let i0 = gradient10();
    let mut s = headers10(&PpsInter::default());
    s.extend(idr10(&i0));
    s.extend(picture(&p_slice(2, vec![(-2, true)]), |e| amvp_then_merge(e, [32, 16])));
    s.extend(picture(&p_slice(4, vec![(-1, true)]), skip_picture));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![3]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4]);
    assert_frame16(by_poc(&f, 0), &i0, "IDR0 at 10 bits");
    assert_frame16(by_poc(&f, 2), &shifted(&i0, 8, 4), "P2 moved");
    assert_frame16(by_poc(&f, 4), &shifted(&shifted(&i0, 8, 4), 4, 2), "P from the picture projected from POC 2 at 10 bits");
    let p4 = img16_of(by_poc(&f, 4));
    assert_eq!(p4.y.iter().copied().max(), Some(1023), "the saturated corner survives at (1 << 10) - 1");
    assert!(p4.y.iter().chain(p4.cb.iter()).chain(p4.cr.iter()).all(|&v| v > 255), "no sample flattened to 8 bits");
}

/// SR1027: [`conceal_tie_takes_the_lower_poc`] at 10 bits: POC 1 ties POC 0 / POC 2 and is the still copy of the intra
/// IDR0, so P4 is the 10-bit gradient.
#[test]
fn conceal_main10_tie_takes_the_lower_poc() {
    let i0 = gradient10();
    let mut s = headers10(&PpsInter::default());
    s.extend(idr10(&i0));
    s.extend(picture(&p_slice(2, vec![(-2, true)]), |e| amvp_then_merge(e, [32, 16])));
    s.extend(picture(&p_slice(4, vec![(-3, true)]), skip_picture));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![1]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 1, src_poc: 0, projected: false, bidir_blocks: 0, forward_blocks: 16 }]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4]);
    assert_frame16(by_poc(&f, 2), &shifted(&i0, 8, 4), "P2 moved");
    assert_frame16(by_poc(&f, 4), &i0, "P from the picture concealed from POC 0 at 10 bits");
}

/// SR1027: [`conceal_bidir_averages_both_projections`] at 10 bits: the 10-bit head, then [`bidir_conceal_stream_fmt`]'s
/// P4 / B2 / P6 bytes reused as they are (no bit-depth syntax in those slices). POC 3 is concealed from B2 along both
/// lists ((4, 2) and (8, 0) samples) and `bi_default` at bd 10 (shift2 5 over s << 4) is (a + b + 1) >> 1 exactly, so P6
/// is that average of B2's two fetches on the 10-bit planes.
#[test]
fn conceal_main10_bidir_averages_at_10_bits() {
    let (full, _) = bidir_conceal_stream_fmt(1);
    let head8 = head_fmt(1, &inter_sps(false), &PpsInter::default()).len();
    let mut s = headers10(&PpsInter::default());
    s.extend(idr10(&gradient10()));
    s.extend_from_slice(&full[head8..]);
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![3]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 16, forward_blocks: 16 }]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 4, 2, 6], "output in decoding order");
    let b2 = img16_of(by_poc(&f, 2));
    let (p, q) = (shifted(&b2, 4, 2), shifted(&b2, 8, 0));
    let avg = |a: &[u16], b: &[u16]| a.iter().zip(b.iter()).map(|(&x, &y)| ((x as u32 + y as u32 + 1) >> 1) as u16).collect::<Vec<u16>>();
    let want = Img { y: avg(&p.y, &q.y), cb: avg(&p.cb, &q.cb), cr: avg(&p.cr, &q.cr) };
    assert_ne!(want.y, p.y, "the fixture's two projections must differ");
    assert_ne!(want.y, q.y, "the fixture's two projections must differ");
    assert!(want.y.iter().all(|&v| v > 255), "the average stays above 8 bits");
    assert_frame16(by_poc(&f, 6), &want, "P from the picture concealed along both lists of B2 at 10 bits");
}

/// SR1027: [`conceal_weighted_applies_the_source_weights`] at 10 bits with luma weight 64 under denom 6 (unit) and
/// luma_offset 10: 8.5.3.3.4.3 scales the offset by WpOffsetBdShiftY = BitDepthY - 8 (inter.rs `explicit_weight_of`
/// `o << off_shift`), so P2 is the moved gradient + 40 and the concealed POC 3 (P4, its copy) is P2 advanced by (4, 2) +
/// 40, clipped at (1 << 10) - 1: equal to P5 (the decoder's own weighted MC of P2 at (16, 8)); the fixture's fetches
/// cross 1023 (the clamp is armed), and it is neither the unscaled + 10 nor the unit-weight projection.
/// SR1056: at the ratio 1/2 the unit weight stays 64 and the shifted offset 40 halves to 20 (P5 coded offset 5).
#[test]
fn conceal_main10_weighted_offset_scales_by_bd() {
    let i0 = gradient10();
    let lift = Some((6, vec![Some((0, 10))]));
    let mut s = headers10(&PpsInter { weighted_pred: true, ..Default::default() });
    s.extend(idr10(&i0));
    s.extend(picture(&SliceCfg { weights: lift.clone(), ..p_slice(2, vec![(-2, true)]) }, |e| amvp_then_merge(e, [32, 16])));
    s.extend(picture(&SliceCfg { weights: Some((6, vec![None])), ..p_slice(4, vec![(-1, true), (-2, false)]) }, skip_picture));
    s.extend(picture(&SliceCfg { weights: /* SR1056 (was lift (0, 10)) */ Some((6, vec![Some((0, 5))])), ..p_slice(5, vec![(-3, true)]) }, |e| amvp_then_merge(e, [16, 8])));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![3]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4, 5]);
    let off = |r: &Img, o: u32| Img { y: r.y.iter().map(|&v| (v as u32 + o).min(1023) as u16).collect(), cb: r.cb.clone(), cr: r.cr.clone() };
    assert_frame16(by_poc(&f, 2), &off(&shifted(&i0, 8, 4), 40), "P2: the gradient moved by (8, 4), offset 10 << 2");
    let p2 = img16_of(by_poc(&f, 2));
    let fetch = shifted(&p2, 4, 2);
    assert!(fetch.y.iter().any(|&v| v < 1023 && v + 20 > 1023), "SR1056 (was + 40): the fixture reaches the clamp at (1 << 10) - 1");
    assert_frame16(by_poc(&f, 4), &img16_of(by_poc(&f, 5)), "P from the concealed picture equals the decoder's own weighted MC of P2 at (16, 8)");
    assert_frame16(by_poc(&f, 4), &off(&fetch, 20), "SR1056 (was 40): P from the concealed picture adds (10 << (10 - 8)) / 2 and clips at 1023");
    assert_ne!(img16_of(by_poc(&f, 4)).y, off(&fetch, 40).y, "SR1056: not the offset applied unscaled");
    let p4 = img16_of(by_poc(&f, 4));
    assert_ne!(p4.y, off(&fetch, 10).y, "not the unscaled offset");
    assert_ne!(p4.y, fetch.y, "not the unit-weight projection");
}

/// SR1027: [`empty_dpb_keeps_the_generated_picture_grey`] at 10 bits: with the IDR lost POC 0 is generated at
/// 1 << (10 - 1) = 512 on every plane (dpb.rs `generate_missing`), nothing is concealed, and P1 (its copy) reads 512
/// (8-bit view 128).
#[test]
fn conceal_main10_grey_fill_is_512() {
    let mut s = headers10(&PpsInter::default());
    s.extend(picture(&p_slice(1, vec![(-1, true)]), skip_picture));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the missing reference");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![0]);
    assert!(d.take_concealed().is_empty(), "an empty DPB conceals nothing");
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![1]);
    assert_frame16(&f[0], &Img::new(|_, _| 512, |_, _| 512, |_, _| 512), "P from the 10-bit grey generated reference");
}

// ---- HEVCCONCEALMAIN10HIGHPRECISION (LEDGER SR1035): the concealment's weighted roads under high_precision_offsets_enabled_flag at 10 bits (named tail block) ----

/// SR1035: sps_range_extension() carrying only high_precision_offsets_enabled_flag (the kit's `SpsCfg.range_ext` in syntax
/// order: index 6, ps.rs reads it as such).
const HIGH_PRECISION: [bool; 9] = [false, false, false, false, false, false, true, false, false];

/// SR1035: [`headers10`] with the SPS range extension written when `high` (high_precision_offsets_enabled_flag 1);
/// `headers10_hp(pi, false)` is [`headers10`]'s bytes (asserted in `conceal_main10_high_precision_offset_unshifted`).
fn headers10_hp(pi: &PpsInter, high: bool) -> Vec<u8> {
    let mut s = vps(false);
    s.extend(nal(33, &sps_rbsp_ext(&SpsCfg { bit_depth: 10, pcm: Some((10, 10, 3, 4, true)), range_ext: high.then_some(HIGH_PRECISION), ..sps_cfg() }, &inter_sps(false))));
    s.extend(nal(34, &pps_rbsp_ext(&PpsCfg { init_qp: 26, deblocking_disabled: true, ..Default::default() }, pi)));
    s
}

/// SR1035: [`conceal_main10_weighted_offset_scales_by_bd`] with the flag off and on. luma_offset 10 under unit weight
/// (64, denom 6): WpOffsetBdShiftY is BitDepthY - 8 = 2 without the flag (+ 40, SR1027's numbers: the armed contrast) and
/// 0 with it (inter.rs `explicit_weight_of` `off_shift`), so with the flag P2 is the moved gradient + 10 and the concealed
/// POC 3 (P4, its copy) is P2 advanced by (4, 2) + 10, clipped at 1023 (a fetch sample sits in 1014..1022: the clamp is
/// armed), equal to P5 (the decoder's own weighted MC of P2 at (16, 8), which reads the same flag at ctu.rs `predict_pu`).
/// SR1056: the concealment halves the offset at the ratio 1/2 (10 -> 5 with the flag, 40 -> 20 without); P5 coded offset 5 is both.
#[test]
fn conceal_main10_high_precision_offset_unshifted() {
    let pi = PpsInter { weighted_pred: true, ..Default::default() };
    assert_eq!(headers10_hp(&pi, false), headers10(&pi), "the flag-off head is SR1027's head byte for byte");
    let i0 = gradient10();
    let off = |r: &Img, o: u32| Img { y: r.y.iter().map(|&v| (v as u32 + o).min(1023) as u16).collect(), cb: r.cb.clone(), cr: r.cr.clone() };
    let mut p4s = Vec::new();
    for high in [false, true] {
        let lift = Some((6, vec![Some((0, 10))]));
        let mut s = headers10_hp(&pi, high);
        s.extend(idr10(&i0));
        s.extend(picture(&SliceCfg { weights: lift.clone(), ..p_slice(2, vec![(-2, true)]) }, |e| amvp_then_merge(e, [32, 16])));
        s.extend(picture(&SliceCfg { weights: Some((6, vec![None])), ..p_slice(4, vec![(-1, true), (-2, false)]) }, skip_picture));
        s.extend(picture(&SliceCfg { weights: /* SR1056 (was lift (0, 10)) */ Some((6, vec![Some((0, 5))])), ..p_slice(5, vec![(-3, true)]) }, |e| amvp_then_merge(e, [16, 8])));
        let mut d = Decoder::new();
        d.decode_annexb(&s).expect("decode past the lost picture");
        d.flush();
        assert_eq!(d.take_generated_refs(), vec![3], "high {high}");
        assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }], "high {high}");
        let f = d.take_frames();
        assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4, 5], "high {high}");
        let o = if high { 10 } else { 40 };
        assert_frame16(by_poc(&f, 2), &off(&shifted(&i0, 8, 4), o), &format!("high {high}: P2, the gradient moved by (8, 4) + {o}"));
        let fetch = shifted(&img16_of(by_poc(&f, 2)), 4, 2);
        let o = o / 2; // SR1056: the concealment's offset at the ratio 1/2 (P2 keeps its own)
        assert!(fetch.y.iter().any(|&v| v < 1023 && v as u32 + o > 1023), "high {high}: the fixture reaches the clamp at (1 << 10) - 1");
        assert_frame16(by_poc(&f, 4), &img16_of(by_poc(&f, 5)), &format!("high {high}: P from the concealed picture equals the decoder's own weighted MC of P2 at (16, 8)"));
        assert_frame16(by_poc(&f, 4), &off(&fetch, o), &format!("high {high}: P from the concealed picture adds {o} and clips at 1023"));
        p4s.push(img16_of(by_poc(&f, 4)).y);
    }
    assert_ne!(p4s[0], p4s[1], "the flag changes the concealed picture: + 5, not + 20 (SR1056; was + 10, not + 40)");
}

/// SR1035: the chroma offset of 7-56 under the flag at 10 bits. P2 and P5 carry unit luma (no luma entry) and chroma
/// weights with ChromaLog2WeightDenom 6: Cb ChromaWeight 32 (delta -32), delta_chroma_offset 511 (SR1043: was 600, outside
/// the flag-off delta range -512..511 that `pred_weight_list` now refuses; the results are unchanged); Cr ChromaWeight 64,
/// delta_chroma_offset -300. With the flag WpOffsetHalfRangeC is 512 and the shift 0: Cb Clip3(-512, 511, 512 + 511 - 256
/// = 767) = +511 (the upper edge), Cr -300 (outside the flag-off range); without it the half range is 128 and the shift 2: Cb
/// Clip3(-128, 127, 128 + 511 - 64 = 575) = 127, << 2 = 508, Cr -128 << 2 = -512 (the lower edge -half << shift is -512 at both flag values, so it carries no
/// contrast). Whole-sample vectors at log2WD 6 + 4 = 10 read Cb ((v + 1) >> 1) + o, Cr v + o, clipped to 0..1023. The
/// concealed POC 3 (P4, its copy) equals P5 (the decoder's own MC) and the hand equation on P2 advanced by (4, 2).
/// SR1056: at the ratio 1/2 Cb (32, 511 / 508) scales to (45, 256 / 254) and Cr (64, -300 / -512) to (64, -150 / -256); P5 carries
/// the scaled table with the flag (coded Cb (-19, 104), Cr (0, -150)); without it Cb 254 is no multiple of 4, so no table
/// codes it: the flag-off concealment is pinned by the hand equation alone.
#[test]
fn conceal_main10_high_precision_chroma_half_range() {
    let i0 = gradient10();
    let pi = PpsInter { weighted_pred: true, ..Default::default() };
    let chroma = Some((0, vec![Some([(-32, /* SR1043 */ 511), (0, -300)])]));
    let mut p4s = Vec::new();
    for high in [false, true] {
        let (ocb, ocr) = if high { (511, -300) } else { (508, -512) };
        let weighted = |r: &Img| Img {
            y: r.y.clone(),
            cb: r.cb.iter().map(|&v| (((v as i32 + 1) >> 1) + ocb).clamp(0, 1023) as u16).collect(),
            cr: r.cr.iter().map(|&v| (v as i32 + ocr).clamp(0, 1023) as u16).collect(),
        };
        let (scb, scr) = if high { (256, -150) } else { (254, -256) }; // SR1056: the concealment's scaled offsets
        let scaled = |r: &Img| Img {
            y: r.y.clone(),
            cb: r.cb.iter().map(|&v| (((45 * v as i32 + 32) >> 6) + scb).clamp(0, 1023) as u16).collect(),
            cr: r.cr.iter().map(|&v| (v as i32 + scr).clamp(0, 1023) as u16).collect(),
        };
        let mut s = headers10_hp(&pi, high);
        s.extend(idr10(&i0));
        s.extend(picture(&SliceCfg { weights: Some((6, vec![None])), chroma_weights: chroma.clone(), ..p_slice(2, vec![(-2, true)]) }, |e| amvp_then_merge(e, [32, 16])));
        s.extend(picture(&SliceCfg { weights: Some((6, vec![None])), ..p_slice(4, vec![(-1, true), (-2, false)]) }, skip_picture));
        s.extend(picture(&SliceCfg { weights: Some((6, vec![None])), chroma_weights: /* SR1056 (was chroma) */ if high { Some((0, vec![Some([(-19, 104), (0, -150)])])) } else { chroma.clone() }, ..p_slice(5, vec![(-3, true)]) }, |e| amvp_then_merge(e, [16, 8])));
        let mut d = Decoder::new();
        d.decode_annexb(&s).expect("decode past the lost picture");
        d.flush();
        assert_eq!(d.take_generated_refs(), vec![3], "high {high}");
        assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }], "high {high}");
        let f = d.take_frames();
        assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4, 5], "high {high}");
        assert_frame16(by_poc(&f, 2), &weighted(&shifted(&i0, 8, 4)), &format!("high {high}: P2, Cb (32, {ocb}), Cr (64, {ocr})"));
        let p2 = img16_of(by_poc(&f, 2));
        if high {
            assert_frame16(by_poc(&f, 4), &img16_of(by_poc(&f, 5)), &format!("high {high}: P from the concealed picture equals the decoder's own weighted MC on every plane"));
        }
        assert_frame16(by_poc(&f, 4), &scaled(&shifted(&p2, 4, 2)), &format!("high {high}: SR1056 (was Cb ({ocb}), Cr ({ocr})): P from the concealed picture follows 7-56 / 8.5.3.3.4.3 scaled to Cb (45, {scb}), Cr (64, {scr})"));
        p4s.push(img16_of(by_poc(&f, 4)));
    }
    assert_ne!(p4s[0].cb, p4s[1].cb, "Cb: + 256 with the flag, not + 254 (SR1056; was + 511, not + 508)");
    assert_ne!(p4s[0].cr, p4s[1].cr, "Cr: - 150 with the flag, not - 256 (SR1056; was - 300, not - 512)");
}

/// SR1035: [`conceal_weighted_bi_mixed_lists_use_one_log2wd`] (SR768's stream: B2's mixed table, P6 skip, B7) at 10 bits
/// with the flag off and on. 8.5.3.3.4.3 bi with p = s << 4 and log2WD 6 + 4 = 10: L0 (48, 5), L1 (16, -3) give
/// (768a + 256b + ((o0 + o1 + 1) << 10)) >> 11, which is (3a + b + 12) >> 3 with the flag (offsets unshifted) and
/// (3a + b + 36) >> 3 without it (offsets 20 / -12); chroma (default weights, offset 0 at both) (a + b + 1) >> 1. P6 (the
/// concealed POC 3's copy) is B7 (the decoder's own weighted bi MC of B2) and the equation on B2's two projections.
/// SR1056: the concealment scales L0 by 1/2 and L1 by -1/2: (55, 3) / (128, 2) with the flag, (55, 10) / (128, 6) without, so P6
/// reads (55a + 128b + 384) >> 7 / (55a + 128b + 1088) >> 7, clipped; B7 carries the scaled table with the flag; without it
/// the L0 offset 10 is no multiple of 4, so P6 is pinned by the hand equation alone.
#[test]
fn conceal_main10_high_precision_bidir() {
    let i0 = gradient10();
    let (w0, w1) = bi_mixed_weights();
    let pi = PpsInter { weighted_bipred: true, ..Default::default() };
    let mut p6s = Vec::new();
    for high in [false, true] {
        let r = if high { 12 } else { 36 };
        let blend = |p: &Img, q: &Img| {
            let luma = p.y.iter().zip(q.y.iter()).map(|(&a, &b)| ((3 * a as u32 + b as u32 + r) >> 3) as u16).collect();
            let avg = |a: &[u16], b: &[u16]| a.iter().zip(b.iter()).map(|(&x, &y)| ((x as u32 + y as u32 + 1) >> 1) as u16).collect::<Vec<u16>>();
            Img { y: luma, cb: avg(&p.cb, &q.cb), cr: avg(&p.cr, &q.cr) }
        };
        let sr = if high { 384 } else { 1088 }; // SR1056: ((o0 + o1 + 1) << 10) >> 4 of the scaled offsets
        let scaled = |p: &Img, q: &Img| {
            let luma = p.y.iter().zip(q.y.iter()).map(|(&a, &b)| ((55 * a as u32 + 128 * b as u32 + sr) >> 7).min(1023) as u16).collect();
            let avg = |a: &[u16], b: &[u16]| a.iter().zip(b.iter()).map(|(&x, &y)| ((x as u32 + y as u32 + 1) >> 1) as u16).collect::<Vec<u16>>();
            Img { y: luma, cb: avg(&p.cb, &q.cb), cr: avg(&p.cr, &q.cr) }
        };
        let mut s = headers10_hp(&pi, high);
        s.extend(idr10(&i0));
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
        let b7 = SliceCfg { poc_lsb: 7, neg: vec![(-5, true)], pos: vec![], num_ref: Some((1, 1)), weights: /* SR1056 (was (-16, 5)) */ Some((6, vec![Some(if high { (-9, 3) } else { (-16, 5) })])), weights_l1: /* SR1056 (was (-48, -3)) */ Some(vec![Some(if high { (64, 2) } else { (-48, -3) })]), ..b2.clone() };
        s.extend(picture(&b7, |e| bi_body(e, 1, [16, 8], [32, 0])));
        let mut d = Decoder::new();
        d.decode_annexb(&s).expect("decode past the lost picture");
        d.flush();
        assert_eq!(d.take_generated_refs(), vec![3], "high {high}");
        assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 16, forward_blocks: 16 }], "high {high}");
        let f = d.take_frames();
        assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 4, 2, 6, 7], "high {high}: output in decoding order");
        assert_frame16(by_poc(&f, 2), &blend(&shifted(&i0, 8, 4), &shifted(&i0, -16, 0)), &format!("high {high}: B2, L0 (48, 5), L1 (16, -3)"));
        let src = img16_of(by_poc(&f, 2));
        let (p, q) = (shifted(&src, 4, 2), shifted(&src, 8, 0));
        assert_ne!(p.y, q.y, "the fixture's two projections must differ");
        if high {
            assert_frame16(by_poc(&f, 6), &img16_of(by_poc(&f, 7)), &format!("high {high}: P from the concealed picture equals the decoder's own weighted bi MC of B2"));
        }
        assert_frame16(by_poc(&f, 6), &scaled(&p, &q), &format!("high {high}: SR1056 (was (3a + b + {r}) >> 3): P from the concealed picture follows 8.5.3.3.4.3 with each list's weights scaled"));
        p6s.push(img16_of(by_poc(&f, 6)).y);
    }
    assert_ne!(p6s[0], p6s[1], "the flag changes the weighted bi concealment: + 384 rounding, not + 1088 (SR1056; was + 12, not + 36)");
}

// ---- HEVCWPOFFSETRANGE (LEDGER SR1043): pred_weight_table offsets outside 7.4.7.3's ranges refuse the slice (named tail block) ----

/// SR1043: decode a weighted head (8-bit [`headers`] + [`idr`], or SR1035's 10-bit [`headers10_hp`] + [`idr10`] with
/// high_precision_offsets_enabled_flag `high`), then return the decode of ONE skip-only P1 whose one-entry table carries
/// `luma` as luma_offset_l0 (delta_luma_weight 0) and `cb` / `cr` as delta_chroma_offset_l0 (delta_chroma_weight 0; the
/// other component 0). Before SR1043 every value parsed (the armed contrast is the pre-arc decoder's Ok).
fn wp_probe(depth10: bool, high: bool, luma: Option<i32>, cb: Option<i32>, cr: Option<i32>) -> hevc_core::Result<()> {
    let pi = PpsInter { weighted_pred: true, ..Default::default() };
    let mut s = if depth10 { headers10_hp(&pi, high) } else { headers(false, true, &pi) };
    s.extend(if depth10 { idr10(&gradient10()) } else { idr(&gradient(), true) });
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("the weighted head and the IDR decode");
    let chroma = (cb.is_some() || cr.is_some()).then(|| (0, vec![Some([(0, cb.unwrap_or(0)), (0, cr.unwrap_or(0))])]));
    let cfg = SliceCfg { weights: Some((6, vec![luma.map(|o| (0, o))])), chroma_weights: chroma, ..p_slice(1, vec![(-1, true)]) };
    d.decode_annexb(&picture(&cfg, skip_picture))
}

const LUMA_OFF: hevc_core::Error = hevc_core::Error::Invalid("luma_offset");
const CHROMA_OFF: hevc_core::Error = hevc_core::Error::Invalid("delta_chroma_offset");

/// SR1043: at 8 bits WpOffsetHalfRangeY is 128: luma_offset_l0 127 and -128 parse, 128 and -129 refuse the slice by name.
#[test]
fn wp_offset_range_8bit_luma() {
    for o in [127, -128, 0] {
        assert_eq!(wp_probe(false, false, Some(o), None, None), Ok(()), "luma_offset {o}");
    }
    for o in [128, -129, 300, -300] {
        assert_eq!(wp_probe(false, false, Some(o), None, None), Err(LUMA_OFF), "luma_offset {o}");
    }
}

/// SR1043: at 8 bits WpOffsetHalfRangeC is 128, so delta_chroma_offset_l0 runs -512..511: 511 and -512 parse, 512 and
/// -513 refuse by name, on Cb and on Cr.
#[test]
fn wp_offset_range_8bit_chroma_delta() {
    for o in [511, -512] {
        assert_eq!(wp_probe(false, false, None, Some(o), None), Ok(()), "Cb delta_chroma_offset {o}");
        assert_eq!(wp_probe(false, false, None, None, Some(o)), Ok(()), "Cr delta_chroma_offset {o}");
    }
    for o in [512, -513] {
        assert_eq!(wp_probe(false, false, None, Some(o), None), Err(CHROMA_OFF), "Cb delta_chroma_offset {o}");
        assert_eq!(wp_probe(false, false, None, None, Some(o)), Err(CHROMA_OFF), "Cr delta_chroma_offset {o}");
    }
}

/// SR1043: at 10 bits WITHOUT high_precision_offsets_enabled_flag the half ranges stay 1 << 7 whatever the depth: the
/// 8-bit ranges hold (luma 127 / 128, chroma delta 511 / 512).
#[test]
fn wp_offset_range_10bit_without_flag() {
    assert_eq!(wp_probe(true, false, Some(127), None, None), Ok(()));
    assert_eq!(wp_probe(true, false, Some(-128), None, None), Ok(()));
    assert_eq!(wp_probe(true, false, Some(128), None, None), Err(LUMA_OFF));
    assert_eq!(wp_probe(true, false, Some(-129), None, None), Err(LUMA_OFF));
    assert_eq!(wp_probe(true, false, None, Some(511), None), Ok(()));
    assert_eq!(wp_probe(true, false, None, Some(512), None), Err(CHROMA_OFF));
}

/// SR1043: at 10 bits WITH the flag (SR1035's head) WpOffsetHalfRangeY / C are 1 << 9 = 512: luma_offset 511 and -512
/// parse, 512 and -513 refuse; delta_chroma_offset 2047 and -2048 parse, 2048 and -2049 refuse.
#[test]
fn wp_offset_range_10bit_high_precision() {
    for o in [511, -512, 128, -300] {
        assert_eq!(wp_probe(true, true, Some(o), None, None), Ok(()), "luma_offset {o}");
    }
    for o in [512, -513] {
        assert_eq!(wp_probe(true, true, Some(o), None, None), Err(LUMA_OFF), "luma_offset {o}");
    }
    for o in [2047, -2048, 600] {
        assert_eq!(wp_probe(true, true, None, Some(o), None), Ok(()), "Cb delta_chroma_offset {o}");
        assert_eq!(wp_probe(true, true, None, None, Some(o)), Ok(()), "Cr delta_chroma_offset {o}");
    }
    for o in [2048, -2049] {
        assert_eq!(wp_probe(true, true, None, Some(o), None), Err(CHROMA_OFF), "Cb delta_chroma_offset {o}");
        assert_eq!(wp_probe(true, true, None, None, Some(o)), Err(CHROMA_OFF), "Cr delta_chroma_offset {o}");
    }
}

/// SR1043: a refused slice is a lost slice. IDR0 (the gradient), P1 (the gradient moved by (8, 4), a unit table), P2
/// (luma_offset_l0 128 at 8 bits: refused by name before its picture starts), P3 (skip-only, references POC 2 alone):
/// POC 2 is generated and concealed from POC 1 through the existing lost-picture road, and every output frame equals
/// the same stream with P2 dropped.
#[test]
fn wp_offset_refused_slice_conceals_as_lost() {
    let i0 = gradient();
    let unit = Some((6, vec![None]));
    let head = {
        let mut s = headers(false, true, &PpsInter { weighted_pred: true, ..Default::default() });
        s.extend(idr(&i0, true));
        s.extend(picture(&SliceCfg { weights: unit.clone(), ..p_slice(1, vec![(-1, true)]) }, |e| amvp_then_merge(e, [32, 16])));
        s
    };
    let bad = picture(&SliceCfg { weights: Some((6, vec![Some((0, 128))])), ..p_slice(2, vec![(-1, true)]) }, skip_picture);
    let p3 = picture(&SliceCfg { weights: unit, ..p_slice(3, vec![(-1, true)]) }, skip_picture);
    let mut runs = Vec::new();
    for with_bad in [true, false] {
        let mut d = Decoder::new();
        d.decode_annexb(&head).expect("the head, the IDR and P1 decode");
        if with_bad {
            assert_eq!(d.decode_annexb(&bad), Err(LUMA_OFF), "P2's out-of-range luma_offset refuses the slice");
        }
        d.decode_annexb(&p3).expect("decode past the refused picture");
        d.flush();
        assert_eq!(d.take_generated_refs(), vec![2], "with P2 {with_bad}");
        assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 2, src_poc: 1, projected: true, bidir_blocks: 0, forward_blocks: 16 }], "with P2 {with_bad}");
        let f = d.take_frames();
        assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 1, 3], "with P2 {with_bad}");
        assert_frame(by_poc(&f, 3), &shifted(&shifted(&i0, 8, 4), 8, 4), "P from the picture concealed from POC 1, advanced by its motion");
        runs.push(f.iter().map(|f| f.planes.clone()).collect::<Vec<_>>());
    }
    assert_eq!(runs[0], runs[1], "the refused slice decodes as the lost one");
}

// ---- HEVCCONCEALWEIGHTEDDISTANCE (LEDGER SR1056): the source weights scaled by the POC distance ratio (named tail block) ----

/// SR1056: a fade fixture with the lost picture at a chosen distance. The PPS carries weighted_pred_flag; IDR0 (the
/// gradient), P1 (references POC 0 at distance 1 with mv (16, 8), luma_log2_weight_denom 6, LumaWeightL0[0] 80 (gain 1.25),
/// luma_offset 8; chroma weights default), P(lost + 1) (skip-only, references the lost POC `lost` and keeps POC 1 unused, a
/// unit table), P(lost + 2) (the verifier: references POC 1 at P1's vector projected by (lost - 1) / 1 with the luma table
/// `verify`). POC `lost` is concealed from POC 1 (the nearest held picture). Returns the decoder after the flush.
fn fade_conceal_stream(lost: i32, verify: (i32, i32)) -> Decoder {
    let k = lost - 1;
    let mut s = headers(false, true, &PpsInter { weighted_pred: true, ..Default::default() });
    s.extend(idr(&gradient(), true));
    s.extend(picture(&SliceCfg { weights: Some((6, vec![Some((16, 8))])), ..p_slice(1, vec![(-1, true)]) }, |e| amvp_then_merge(e, [16, 8])));
    s.extend(picture(&SliceCfg { weights: Some((6, vec![None])), ..p_slice((lost + 1) as u32, vec![(-1, true), (-lost, false)]) }, skip_picture));
    s.extend(picture(&SliceCfg { weights: Some((6, vec![Some(verify)])), ..p_slice((lost + 2) as u32, vec![(-(lost + 1), true)]) }, |e| amvp_then_merge(e, [16 * k, 8 * k])));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    d
}

/// SR1056: luma of `r` through 8-252 with (w, o) at log2WD 12 (denom 6, 8 bits): ((64v * w + 2048) >> 12) + o, clipped;
/// chroma (default weights) unchanged.
fn weigh6(r: &Img, (w, o): (i32, i32)) -> Img {
    Img { y: r.y.iter().map(|&v| (((64 * v as i32 * w + 2048) >> 12) + o).clamp(0, 255) as u16).collect(), cb: r.cb.clone(), cr: r.cr.clone() }
}

/// SR1056: s = 1 predicted from r = 0 with (80, 8) (gain 1.25, offset 8 per picture), c = 3 lost: c - s = 2 against
/// s - r = 1, so dpb.rs `scale_weight` takes the gain to 1.25^2 in the log domain (Q16 102234: w' = 100) and the offset
/// to 16, and the vector to (32, 16) = (8, 4) samples. P4 (the concealed picture's copy) is P1 advanced by (8, 4) through
/// ((100v + 32) >> 6) + 16, equal to P5 (the decoder's own weighted MC of P1 coded (100, 16)); before SR1056 it read P1's
/// unscaled ((5v + 2) >> 2) + 8 (the armed contrast).
#[test]
fn conceal_weighted_distance_fade_two() {
    let mut d = fade_conceal_stream(3, (100 - 64, 16));
    assert_eq!(d.take_generated_refs(), vec![3]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 1, projected: true, bidir_blocks: 0, forward_blocks: 16 }]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 1, 4, 5]);
    assert_frame(by_poc(&f, 1), &weigh6(&shifted(&gradient(), 4, 2), (80, 8)), "P1: the gradient moved by (4, 2) and weighted (80, 8)");
    let fetch = shifted(&img_of(by_poc(&f, 1)), 8, 4);
    assert_frame(by_poc(&f, 4), &img_of(by_poc(&f, 5)), "P from the concealed picture equals the decoder's own MC of P1 weighted (100, 16)");
    assert_frame(by_poc(&f, 4), &weigh6(&fetch, (100, 16)), "P from the concealed picture: the gain 1.25^2 and the offset 2 x 8");
    assert_ne!(img_of(by_poc(&f, 4)).y, weigh6(&fetch, (80, 8)).y, "not P1's weights applied unscaled (SR741's road)");
}

/// SR1056: the same fade with c = 2 lost: c - s = 1 == s - r, so `scale_weight` returns P1's (80, 8) unchanged and P3 (the
/// concealed picture's copy) is P1 advanced by (4, 2) through ((5v + 2) >> 2) + 8, equal to P4 coded with P1's own table.
#[test]
fn conceal_weighted_distance_equal_is_unscaled() {
    let mut d = fade_conceal_stream(2, (16, 8));
    assert_eq!(d.take_generated_refs(), vec![2]);
    assert_eq!(d.take_concealed(), vec![hevc_core::dpb::Concealed { poc: 2, src_poc: 1, projected: true, bidir_blocks: 0, forward_blocks: 16 }]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 1, 3, 4]);
    let fetch = shifted(&img_of(by_poc(&f, 1)), 4, 2);
    assert_frame(by_poc(&f, 3), &img_of(by_poc(&f, 4)), "P from the concealed picture equals the decoder's own MC of P1 with P1's table");
    assert_frame(by_poc(&f, 3), &weigh6(&fetch, (80, 8)), "equal distances: P1's weights unchanged");
    assert_eq!(hevc_core::dpb::scale_weight((80, 8, 12), (1, 1), 8, false), (80, 8, 12), "num == den is the identity");
    assert_eq!(hevc_core::dpb::scale_weight((80, 8, 12), (2, 0), 8, false), (80, 8, 12), "den == 0 is the identity");
}

/// SR1056: the scaled offset holds 7.4.7.3's half range (HEVCWPOFFSETRANGE SR1043) in `explicit_weight_of`'s shifted domain
/// and the scaled weight its delta range [2^denom - 128, 2^denom + 127]: 8 bits [-128, 127]; 10 bits without the flag
/// [-128 << 2, 127 << 2] = [-512, 508]; with high_precision_offsets_enabled_flag [-512, 511]; weight 191 at denom 6.
#[test]
fn conceal_weighted_distance_clamp_at_the_offset_edge() {
    use hevc_core::dpb::scale_weight;
    assert_eq!(scale_weight((64, 100, 12), (3, 1), 8, false), (64, 127, 12), "8 bits: 300 clamps to 127");
    assert_eq!(scale_weight((64, -100, 12), (3, 1), 8, false), (64, -128, 12), "8 bits: -300 clamps to -128");
    assert_eq!(scale_weight((64, 400, 10), (3, 1), 10, false), (64, 508, 10), "10 bits, flag off: 1200 clamps to 127 << 2");
    assert_eq!(scale_weight((64, -400, 10), (3, 1), 10, false), (64, -512, 10), "10 bits, flag off: -1200 clamps to -128 << 2");
    assert_eq!(scale_weight((64, 400, 10), (3, 1), 10, true), (64, 511, 10), "10 bits, flag on: 1200 clamps to 511");
    assert_eq!(scale_weight((64, -400, 10), (3, 1), 10, true), (64, -512, 10), "10 bits, flag on: -1200 clamps to -512");
    assert_eq!(scale_weight((191, 0, 12), (2, 1), 8, false), (191, 0, 12), "the weight edge 64 + 127 holds");
    assert_eq!(scale_weight((128, 0, 12), (3, 1), 8, false), (191, 0, 12), "gain 2^3 clamps to the weight edge");
    assert_eq!(scale_weight((64, 10, 12), (1, -2), 8, false), (64, -5, 12), "a negative ratio flips the offset (rounded half away)");
}

/// SR1056: the round trip of dpb.rs `log2_q16` / `exp2_q16` over g in {0.5, 0.8, 1.25, 2} x r in {1/2, 2, 3} (the doc table
/// on `scale_weight`): the Q16 gain is pinned, within 0.2% of the true g^r, and `scale_weight`'s denom-6 weight (g = w / 64:
/// w 32, 51, 80, 128) is pinned (191 is the weight edge).
#[test]
fn conceal_weighted_distance_log_error_table() {
    use hevc_core::dpb::{exp2_q16, log2_q16, scale_weight};
    let ratios = [(1, 2), (2, 1), (3, 1)];
    let table: [(u32, i32, [u32; 3], [i32; 3]); 4] = [
        (32768, 32, [46431, 16384, 8192], [45, 16, 8]),
        (52429, 51, [58530, 41973, 33571], [57, 41, 32]),
        (81920, 80, [73356, 102234, 127932], [72, 100, 125]),
        (131072, 128, [92862, 262144, 524288], [91, 191, 191]),
    ];
    for (x, w, gq, w6) in table {
        for (k, &(a, b)) in ratios.iter().enumerate() {
            let got = exp2_q16(round_half_away(log2_q16(x) * a, b));
            assert_eq!(got, gq[k], "g {x} r {a}/{b}: the Q16 gain");
            let truth = (x as f64 / 65536.0).powf(a as f64 / b as f64);
            let err = (got as f64 / 65536.0 / truth - 1.0).abs();
            assert!(err <= 0.002, "g {x} r {a}/{b}: {got} is {err} off the true {truth}");
            assert_eq!(scale_weight((w, 0, 12), (a as i32, b as i32), 8, false).0, w6[k], "g {w}/64 r {a}/{b}: the scaled weight");
        }
    }
    assert_eq!(log2_q16(65536), 0, "log2 1 = 0");
    assert_eq!(exp2_q16(0), 65536, "2^0 = 1");
}

// ---- HEVCCONCEALWEIGHTEDSLICES (LEDGER SR1067): each slice segment's pred_weight_table conceals its own blocks (named tail block) ----

/// A one-entry luma pred_weight_table (luma_log2_weight_denom 6): delta_luma_weight `dw` (LumaWeightL0 64 + dw), offset `o`.
fn slices_wp(dw: i32, o: i32) -> Option<(u32, Vec<Option<(i32, i32)>>)> {
    Some((6, vec![Some((dw, o))]))
}

/// 8.5.3.3.4.3 at 8 bits for luma weight `w` (denom 6) and offset `o`: ((w v + 32) >> 6) + o (before the clip).
fn slices_weighted(w: i32, o: i32) -> impl Fn(i32) -> i32 {
    move |v| ((w * v + 32) >> 6) + o
}

/// `r` with luma rows 0-15 (CTBs 0-1, the first segment) through `top` and rows 16-31 (CTBs 2-3) through `bottom`, clipped to 8
/// bits; chroma unchanged (the tables carry default chroma weights).
fn slices_halves(r: &Img, top: impl Fn(i32) -> i32, bottom: impl Fn(i32) -> i32) -> Img {
    let y = r.y.iter().enumerate().map(|(i, &v)| (if i / W < 16 { top(v as i32) } else { bottom(v as i32) }).clamp(0, 255) as u16).collect();
    Img { y, cb: r.cb.clone(), cr: r.cr.clone() }
}

/// IDR0 (the gradient), the P2 slice data `p2` (references POC 0), P5 (skip-only, references the lost POC 4 and keeps POC 2
/// unused; its table's one entry has the default weight: a copy), then `p6` when given. c - s = s - r = 2: the concealment
/// ratio is 1, so the projected vector is P2's own and the weights stay unscaled (dpb.rs `scale_weight` num == den).
fn slices_ratio_one(pi: &PpsInter, p2: Vec<u8>, p6: Option<Vec<u8>>) -> (Vec<Frame>, Vec<i32>, Vec<hevc_core::dpb::Concealed>) {
    let mut s = headers(false, true, pi);
    s.extend(idr(&gradient(), true));
    s.extend(p2);
    s.extend(picture(&SliceCfg { weights: Some((6, vec![None])), ..p_slice(5, vec![(-1, true), (-3, false)]) }, skip_picture));
    if let Some(p6) = p6 {
        s.extend(p6);
    }
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    (d.take_frames(), d.take_generated_refs(), d.take_concealed())
}

/// The PPS carries weighted_pred_flag. P2 is coded as TWO independent slice segments (the kit's `two_segments`), both moving by
/// mv (32, 16) = (8, 4) samples from POC 0: the top (CTBs 0-1) with table A (LumaWeightL0 80 = 1.25, offset 8), the bottom
/// (CTBs 2-3) with table B (48 = 0.75, offset -8). POC 4 is concealed from P2 at ratio 1; P5 is its copy. Each 8x8 block is
/// weighted by the table of the slice that coded its source block (picture.rs `slice_weights`), so P5 equals P6, the decoder's
/// own two-slice weighted MC of P2 with tables A / B, and the hand equation per half. Before SR1067 the bottom half took table A
/// (the first segment's): ((80 v + 32) >> 6) + 8 on rows 16-31, refused below.
#[test]
fn conceal_weighted_slices_each_half_keeps_its_table() {
    let pi = PpsInter { weighted_pred: true, ..Default::default() };
    let p2 = |poc: u32, neg: Vec<(i32, bool)>| {
        two_segments(&SliceCfg { weights: slices_wp(16, 8), ..p_slice(poc, neg.clone()) }, &SliceCfg { weights: slices_wp(-16, -8), ..p_slice(poc, neg) }, None, [32, 16])
    };
    let (f, generated, concealed) = slices_ratio_one(&pi, p2(2, vec![(-2, true)]), Some(p2(6, vec![(-4, true)])));
    assert_eq!(generated, vec![4]);
    assert_eq!(concealed, vec![hevc_core::dpb::Concealed { poc: 4, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }]);
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 5, 6]);
    let (a, b) = (slices_weighted(80, 8), slices_weighted(48, -8));
    assert_frame(by_poc(&f, 2), &slices_halves(&shifted(&gradient(), 8, 4), &a, &b), "P2: the top segment weighted by A, the bottom by B");
    let p2 = img_of(by_poc(&f, 2));
    let p6 = img_of(by_poc(&f, 6));
    assert_frame(by_poc(&f, 5), &p6, "the concealed copy equals the decoder's own two-slice weighted MC of P2 at (32, 16)");
    assert_frame(by_poc(&f, 5), &slices_halves(&shifted(&p2, 8, 4), &a, &b), "the top half by table A, the bottom half by table B");
    assert_ne!(img_of(by_poc(&f, 5)).y, slices_halves(&shifted(&p2, 8, 4), &a, &a).y, "not the first segment's table on the bottom half (pre-SR1067)");
}

/// Two segments carrying the SAME table (A) conceal byte for byte as P2 coded as one slice with that table (the SR741 road):
/// one table per slice changes nothing when the slices agree.
#[test]
fn conceal_weighted_slices_identical_tables_match_one_slice() {
    let pi = PpsInter { weighted_pred: true, ..Default::default() };
    let cfg = SliceCfg { weights: slices_wp(16, 8), ..p_slice(2, vec![(-2, true)]) };
    let (two, g2, c2) = slices_ratio_one(&pi, two_segments(&cfg, &cfg, None, [32, 16]), None);
    let (one, g1, c1) = slices_ratio_one(&pi, picture(&cfg, |e| amvp_then_merge(e, [32, 16])), None);
    assert_eq!((g2, c2), (g1, c1));
    assert_eq!(two.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 5]);
    assert_eq!(by_poc(&two, 2).planes, by_poc(&one, 2).planes, "P2 two segments == one slice");
    assert_eq!(by_poc(&two, 5).planes, by_poc(&one, 5).planes, "the concealed copy: two segments == one slice");
}

/// The PPS enables dependent slice segments; P2's second segment is DEPENDENT (its header stops at slice_segment_address and
/// carries no table: 7.3.6.1 copies the independent segment's), so it belongs to the first slice and every block of the
/// concealed POC 4 is weighted by table A.
#[test]
fn conceal_weighted_slices_dependent_segment_inherits() {
    let pi = PpsInter { weighted_pred: true, dependent_slices: true, ..Default::default() };
    let cfg = SliceCfg { weights: slices_wp(16, 8), ..p_slice(2, vec![(-2, true)]) };
    let (f, generated, concealed) = slices_ratio_one(&pi, two_segments(&cfg, &cfg, Some(true), [32, 16]), None);
    assert_eq!(generated, vec![4]);
    assert_eq!(concealed.iter().map(|c| (c.poc, c.src_poc, c.projected)).collect::<Vec<_>>(), vec![(4, 2, true)]);
    let a = slices_weighted(80, 8);
    assert_frame(by_poc(&f, 2), &slices_halves(&shifted(&gradient(), 8, 4), &a, &a), "P2: both segments weighted by A");
    let p2 = img_of(by_poc(&f, 2));
    assert_frame(by_poc(&f, 5), &slices_halves(&shifted(&p2, 8, 4), &a, &a), "the dependent segment's blocks conceal with its slice's table");
}

/// SR1056's distance scaling per table: P2's top segment carries the default weight (identity), its bottom (32, 10); P4
/// (skip-only) references the lost POC 3, concealed from P2 at ratio (3 - 2) / (2 - 0) = 1/2 (mv' (16, 8) = (4, 2) samples):
/// the top stays the copy (identity scales to identity) and the bottom takes (32, 10) scaled to SR1056's pinned (45, 5).
#[test]
fn conceal_weighted_slices_distance_scales_per_table() {
    let mut s = headers(false, true, &PpsInter { weighted_pred: true, ..Default::default() });
    s.extend(idr(&gradient(), true));
    let neg = vec![(-2, true)];
    s.extend(two_segments(&SliceCfg { weights: Some((6, vec![None])), ..p_slice(2, neg.clone()) }, &SliceCfg { weights: slices_wp(-32, 10), ..p_slice(2, neg) }, None, [32, 16]));
    s.extend(picture(&SliceCfg { weights: Some((6, vec![None])), ..p_slice(4, vec![(-1, true), (-2, false)]) }, skip_picture));
    let mut d = Decoder::new();
    d.decode_annexb(&s).expect("decode past the lost picture");
    d.flush();
    assert_eq!(d.take_generated_refs(), vec![3]);
    let f = d.take_frames();
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4]);
    let p2 = img_of(by_poc(&f, 2));
    assert_frame(by_poc(&f, 4), &slices_halves(&shifted(&p2, 4, 2), |v| v, slices_weighted(45, 5)), "top: identity; bottom: (32, 10) scaled by 1/2 to (45, 5)");
    assert_ne!(img_of(by_poc(&f, 4)).y, slices_halves(&shifted(&p2, 4, 2), |v| v, slices_weighted(32, 10)).y, "not the bottom table unscaled");
}

// ---- HEVCCONCEALFORWARDDEPTH (LEDGER SR1073): a concealed picture refined by up to two referencers (named tail block) ----

/// Decode `s` and flush: the frames, the generated POCs and the concealment notes.
fn depth_decode(s: &[u8]) -> (Vec<Frame>, Vec<i32>, Vec<hevc_core::dpb::Concealed>) {
    let mut d = Decoder::new();
    d.decode_annexb(s).expect("decode past the lost picture");
    d.flush();
    (d.take_frames(), d.take_generated_refs(), d.take_concealed())
}

/// Luma rows 0-15 and chroma rows 0-7 (CTBs 0-1) of `top`, the rest of `bottom`.
fn depth_halves(top: &Img, bottom: &Img) -> Img {
    let pick = |t: &[u16], b: &[u16], n: usize| t.iter().zip(b).enumerate().map(|(i, (&t, &b))| if i / n < n / 2 { t } else { b }).collect::<Vec<u16>>();
    Img { y: pick(&top.y, &bottom.y, W), cb: pick(&top.cb, &bottom.cb, W / 2), cr: pick(&top.cr, &bottom.cr, W / 2) }
}

/// The equal-weight average (a + b + 1) >> 1 of `a` and `b`, sample by sample on every plane.
fn depth_avg(a: &Img, b: &Img) -> Img {
    let avg = |p: &[u16], q: &[u16]| p.iter().zip(q).map(|(&p, &q)| (p + q + 1) >> 1).collect::<Vec<u16>>();
    Img { y: avg(&a.y, &b.y), cb: avg(&a.cb, &b.cb), cr: avg(&a.cr, &b.cr) }
}

/// The PPS carries lists_modification_present_flag. IDR0 (the gradient), P2 (references POC 0 with mv (6, 2)), C1 = P4 and
/// C2 = P5 coded as TWO independent slice segments (the kit's `two_segments`) over StCurrBefore {POC 3 lost, the other}:
/// C1's top segment (CTBs 0-1) takes L0 = [POC 3], its bottom L0 = [POC 2] (list_entry_l0 1), both at mvd (32, 16); C2's top
/// L0 = [POC 4], its bottom L0 = [POC 3], both at mvd (-16, 8); P6 (skip-only, references POC 3 alone: its copy, and a
/// referencer at mv 0 whose refinement is idempotent). POC 3 is concealed from POC 2 (S projection: P2 at (3, 1)); C1 refines
/// its top half at -mv = (-8, -4) samples, C2 its bottom half at (4, -2): every block is refined once. Before SR1073 the
/// drained `forward` let C2 do nothing, so P6's bottom half stayed the S projection `hand_mc(P2, [3, 1])` (refused below).
#[test]
fn conceal_forward_depth_second_referencer_fills_the_uncovered_half() {
    let mut s = headers(false, true, &PpsInter { lists_modification: true, ..Default::default() });
    s.extend(idr(&gradient(), true));
    s.extend(picture(&p_slice(2, vec![(-2, true)]), |e| amvp_then_merge(e, [6, 2])));
    let half = |poc: u32, top: u32, bottom: u32, mvd: [i32; 2]| {
        let neg = vec![(-1, true), (-2, true)];
        let cfg = |e: u32| SliceCfg { lists_mod: true, list_entry_l0: Some(vec![e]), ..p_slice(poc, neg.clone()) };
        two_segments(&cfg(top), &cfg(bottom), None, mvd)
    };
    s.extend(half(4, 0, 1, [32, 16]));
    s.extend(half(5, 0, 1, [-16, 8]));
    s.extend(picture(&p_slice(6, vec![(-3, true)]), skip_picture));
    let (f, generated, concealed) = depth_decode(&s);
    assert_eq!(generated, vec![3]);
    assert_eq!(concealed, vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }], "C1's 8 blocks and C2's 8");
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4, 5, 6]);
    let (p2, p4, p5) = (img_of(by_poc(&f, 2)), img_of(by_poc(&f, 4)), img_of(by_poc(&f, 5)));
    let want = depth_halves(&shifted(&p4, -8, -4), &shifted(&p5, 4, -2));
    assert_frame(by_poc(&f, 6), &want, "top refined from C1 at -mv, bottom from C2 at -mv");
    let before = depth_halves(&shifted(&p4, -8, -4), &hand_mc(&p2, [3, 1]));
    assert_ne!(img_of(by_poc(&f, 6)).y, before.y, "not the pre-SR1073 bottom half (the S projection)");
}

/// IDR0, P2 (mv (6, 2) to POC 0), C1 = P4 (references the lost POC 3 at mvd (32, 16), keeps POC 2 unused), C2 = P5
/// (references POC 3 alone at mvd (-16, 8), keeps POC 4 unused), when `third` C3 = P6 (references POC 3 alone at mvd (8, -24),
/// keeps 4 / 5 unused), P7 (skip-only copy of POC 3, keeps 4 / 5 unused), B8 (L0 = L1 = [POC 5, POC 4]; PRED_BI L0 ref_idx 1
/// = P4 at (-32, -16), L1 ref_idx 0 = P5 at (16, -8)). Every block of POC 3 is refined by C1 (outright) then C2 (averaged):
/// (P4(x - (8, 4)) + P5(x + (4, -2)) + 1) >> 1 on every plane, which is exactly B8 (integer vectors: 8-262 on the 14-bit
/// intermediates is (a + b + 1) >> 1); C3 finds every count at CONCEAL_FORWARD_DEPTH and changes nothing.
fn depth_three(third: bool) -> (Vec<Frame>, Vec<i32>, Vec<hevc_core::dpb::Concealed>) {
    let mut s = headers(false, true, &PpsInter::default());
    s.extend(idr(&gradient(), true));
    s.extend(picture(&p_slice(2, vec![(-2, true)]), |e| amvp_then_merge(e, [6, 2])));
    s.extend(picture(&p_slice(4, vec![(-1, true), (-2, false)]), |e| amvp_then_merge(e, [32, 16])));
    s.extend(picture(&p_slice(5, vec![(-1, false), (-2, true)]), |e| amvp_then_merge(e, [-16, 8])));
    if third {
        s.extend(picture(&p_slice(6, vec![(-1, false), (-2, false), (-3, true)]), |e| amvp_then_merge(e, [8, -24])));
    }
    s.extend(picture(&p_slice(7, vec![(-2, false), (-3, false), (-4, true)]), skip_picture));
    let b8 = SliceCfg { nal_type: 1, slice_type: 0, poc_lsb: 8, neg: vec![(-3, true), (-4, true)], num_ref: Some((2, 2)), five_minus_max_num_merge_cand: 4, deblocking_disabled: true, ..Default::default() };
    s.extend(picture(&b8, |e| bi_ref_body(e, 2, [1, 0], [-32, -16], [16, -8])));
    depth_decode(&s)
}

#[test]
fn conceal_forward_depth_both_referencers_average_and_a_third_does_nothing() {
    let (f, generated, concealed) = depth_three(true);
    assert_eq!(generated, vec![3]);
    assert_eq!(concealed, vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }]);
    assert_eq!(f.iter().map(|f| f.poc).collect::<Vec<_>>(), vec![0, 2, 4, 5, 6, 7, 8]);
    let (p4, p5) = (img_of(by_poc(&f, 4)), img_of(by_poc(&f, 5)));
    let want = depth_avg(&shifted(&p4, -8, -4), &shifted(&p5, 4, -2));
    assert_frame(by_poc(&f, 7), &want, "C1's and C2's projections averaged per sample");
    assert_frame(by_poc(&f, 8), &want, "the decoder's own bi-prediction of P4 / P5 at the same vectors");
    assert_ne!(img_of(by_poc(&f, 7)).y, shifted(&p4, -8, -4).y, "not C1's projection alone (pre-SR1073)");
    let (g, _, c) = depth_three(false);
    assert_eq!(c, concealed);
    assert_eq!(by_poc(&g, 7).planes, by_poc(&f, 7).planes, "a third referencer (count 2 = depth) changes nothing");
}

/// The PPS carries weighted_pred_flag; every P slice a table (denom 6; one entry): P2 and P7 the default (a copy), C1 = P4
/// (48, 5), C2 = P5 (80, -8); the stream of [`depth_three`] without C3 and B8. Each referencer's projection is inverted by its
/// OWN weights (SR778 `uni_explicit_inv`: p' = round half away (4096 (q - o) / w), (p' + 32) >> 6, clipped) before the equal
/// average; chroma (default weights) averages the copies.
#[test]
fn conceal_forward_depth_weighted_referencers_each_invert_their_own() {
    let mut s = headers(false, true, &PpsInter { weighted_pred: true, ..Default::default() });
    s.extend(idr(&gradient(), true));
    let id = Some((6, vec![None]));
    s.extend(picture(&SliceCfg { weights: id.clone(), ..p_slice(2, vec![(-2, true)]) }, |e| amvp_then_merge(e, [6, 2])));
    s.extend(picture(&SliceCfg { weights: slices_wp(-16, 5), ..p_slice(4, vec![(-1, true), (-2, false)]) }, |e| amvp_then_merge(e, [32, 16])));
    s.extend(picture(&SliceCfg { weights: slices_wp(16, -8), ..p_slice(5, vec![(-1, false), (-2, true)]) }, |e| amvp_then_merge(e, [-16, 8])));
    s.extend(picture(&SliceCfg { weights: id, ..p_slice(7, vec![(-4, true)]) }, skip_picture));
    let (f, generated, _) = depth_decode(&s);
    assert_eq!(generated, vec![3]);
    let inv = |r: &Img, (w, o): (i64, i64)| Img { y: r.y.iter().map(|&q| ((round_half_away(4096 * (q as i64 - o), w) + 32) >> 6).clamp(0, 255) as u16).collect(), cb: r.cb.clone(), cr: r.cr.clone() };
    let (p4, p5) = (img_of(by_poc(&f, 4)), img_of(by_poc(&f, 5)));
    let want = depth_avg(&inv(&shifted(&p4, -8, -4), (48, 5)), &inv(&shifted(&p5, 4, -2), (80, -8)));
    assert_frame(by_poc(&f, 7), &want, "each referencer inverted by its own weights, then averaged");
}

/// The named SR1073 behaviour change: the PPS carries lists_modification_present_flag; P4 holds the lost POC 3 in StCurrBefore
/// (so it is generated and concealed from POC 2) but its L0 is [POC 2] alone (list_entry_l0 1, mvd (32, 16)): no block of P4
/// references POC 3. P5 references POC 3 alone at mvd (-16, 8); P6 is its skip copy. Before SR1073 P4's end drained POC 3
/// unrefined, so P6 was the S projection `hand_mc(P2, [3, 1])`; now POC 3 waits for its first real referencer, P5.
#[test]
fn conceal_forward_depth_waits_for_the_first_real_referencer() {
    let mut s = headers(false, true, &PpsInter { lists_modification: true, ..Default::default() });
    s.extend(idr(&gradient(), true));
    s.extend(picture(&p_slice(2, vec![(-2, true)]), |e| amvp_then_merge(e, [6, 2])));
    s.extend(picture(&SliceCfg { lists_mod: true, list_entry_l0: Some(vec![1]), ..p_slice(4, vec![(-1, true), (-2, true)]) }, |e| amvp_then_merge(e, [32, 16])));
    s.extend(picture(&p_slice(5, vec![(-1, false), (-2, true)]), |e| amvp_then_merge(e, [-16, 8])));
    s.extend(picture(&p_slice(6, vec![(-3, true)]), skip_picture));
    let (f, generated, concealed) = depth_decode(&s);
    assert_eq!(generated, vec![3]);
    assert_eq!(concealed, vec![hevc_core::dpb::Concealed { poc: 3, src_poc: 2, projected: true, bidir_blocks: 0, forward_blocks: 16 }], "refined by P5, not dropped at P4's end");
    let (p2, p5) = (img_of(by_poc(&f, 2)), img_of(by_poc(&f, 5)));
    assert_frame(by_poc(&f, 6), &shifted(&p5, 4, -2), "refined from P5, the first picture that references POC 3");
    assert_ne!(img_of(by_poc(&f, 6)).y, hand_mc(&p2, [3, 1]).y, "not the pre-SR1073 S projection");
}

// ---- HEVCWPTOSHIBALUMA (LEDGER SR1082): merge B0 prunes against the B1 neighbour, not the pruned candidate (named tail block) ----

/// 8.5.3.2.3: availableFlagB0 is 0 when availableB1 and the units covering B1 and B0 carry the same motion — the B1
/// NEIGHBOUR, whether or not B1 itself was pruned against A1. MaxNumMergeCand 2. CTB 0 an AMVP unit, mvd (8, 0) (mvp 0:
/// mv 2 luma samples right = M); CTBs 1 and 2 skip, merge_idx 0 (M by A1 / B1); CTB 3 split into four 8x8 skip CUs
/// (cu_skip_flag ctxInc 2: left and above skipped), merge_idx 0, 0, 1, 0. At z2 (16, 24): A1 = CTB 2 (M), B1 = z0 (M,
/// pruned: equal to A1), B0 = z1 (M, pruned against B1), A0 below the picture, B2 = CTB 2 (M, pruned: equal to A1), no
/// temporal candidate: the list is {M, zero}, merge_idx 1 = mv 0 refIdx 0; z3 merges A1 = z2 (mv 0). Before SR1082 B0
/// survived (compared with the pruned B1, None), the list was {M, M}, and the whole picture moved by M (WP_A_Toshiba_3
/// and WP_A_MAIN10_Toshiba_3: CTB 17's merge_idx 1, mv (2, 0) for (0, 0)).
#[test]
fn merge_b0_prunes_against_the_b1_neighbour_not_the_pruned_candidate() {
    let i0 = gradient();
    let mut s = headers(false, true, &PpsInter::default());
    s.extend(idr(&i0, true));
    let cfg = SliceCfg { five_minus_max_num_merge_cand: 3, ..p_slice(1, vec![(-1, true)]) };
    s.extend(picture(&cfg, |mut e| {
        e.split_cu(0, false);
        e.cu_skip(0, false);
        e.pred_inter();
        e.part_mode_2nx2n();
        e.merge_flag(false);
        e.mvd([8, 0]);
        e.mvp_flag(0);
        e.rqt_root_cbf(false);
        for _ in 0..2 {
            e.terminate(0);
            e.split_cu(0, false);
            e.cu_skip(0, true);
            e.merge_idx(0, 2);
        }
        e.terminate(0);
        e.split_cu(0, true);
        for idx in [0, 0, 1, 0] {
            e.cu_skip(2, true);
            e.merge_idx(idx, 2);
        }
        e.finish().out
    }));
    let f = decode_all(&s);
    let m = shifted(&i0, 2, 0);
    let want = Img::new(
        |x, y| if x >= 16 && y >= 24 { i0.y[y * W + x] } else { m.y[y * W + x] },
        |x, y| if x >= 8 && y >= 12 { i0.cb[y * (W / 2) + x] } else { m.cb[y * (W / 2) + x] },
        |x, y| if x >= 8 && y >= 12 { i0.cr[y * (W / 2) + x] } else { m.cr[y * (W / 2) + x] },
    );
    assert_ne!(img_of(by_poc(&f, 1)).y, m.y, "not the whole-picture shift of the duplicate B0 (pre-SR1082)");
    assert_frame(by_poc(&f, 1), &want, "z2 merge_idx 1 = the zero candidate; z3 merges it");
}
