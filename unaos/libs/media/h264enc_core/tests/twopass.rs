// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! H264ENCTWOPASS (LEDGER SR698): two-pass rate control. (a) `rc::pass2_qps` on a synthetic first pass: a more complex P picture
//! gets the higher QP and still more predicted bits (x264's qscale ~ complexity^0.4), the predicted total within 2% of the
//! target, every QP within floor..=51; (b) tests/rc.rs's scene (30 frames, one GOP) coded CBR, then TwoPass planned from the CBR
//! run's `Encoder::stats` at the same rate: the two-pass total within 2% of the target, its P pictures' bits no more spread than
//! CBR's, and both decode through h264_core to the reconstruction byte for byte; (c) `Encoder::stats` holds one record per coded
//! picture with the access unit's bits and type, under B pictures and constant QP too; a plan needs `RcMode::TwoPass`.
//! H264ENCTWOPASSFILE (LEDGER SR719): (d) the records round-trip byte-exact through `rc::write_stats` / `read_stats` (I, P and B);
//! (e) a second pass planned from the read-back records codes the same bytes as one planned from the records in memory; (f) a
//! truncated text, a wrong count, a wrong version, a bad kind and a tampered complexity refuse by name.
//! H264ENCTWOPASSRESCALE (LEDGER SR727): (g) `rc::rescale_stats` 30 -> 15 means each pair, keeps the first I and the record invariant;
//! (h) a 15-frame pass 2 planned from the 30-frame pass 1 lands within 3% of its target, equal counts byte-identical; (i)
//! `Encoder::stats` leaves the HRD's filler NAL units out of a picture's bits, as the rc_update line does.
//! H264ENCTWOPASSBLUR (LEDGER SR736): (a) runs on `RAW` (SR698's rule: no blur, the next-P I rule); (j) `rc::blur_complexity` on an
//! impulse spreads it over its neighbours and stops at an IDR (the falling-from-the-peak check at cplxblur 2: at 20 the window
//! reaches the IDR at 0 too, a measured correction), `rc::exp_neg_q16` within 1 of round(65536 e^-t); (k) `RAW` plans SR698's exact
//! QPs (literals captured before the change); (l) the default (x264's blurs) 30-frame second pass within 2% of its target, its
//! PLANNED P QP range (`pass2_qps` at the export's target) no wider than RAW's; the bit spread (the flatter QP curve widens it) and
//! the coded QP range (`pass2_pick`'s runtime budget correction and the bucket's step move each picture off its plan) are printed,
//! not asserted, measured corrections of the design's bit-spread check; (m) qcomp 1 one QP for every P, qcomp 0 equal bits (at
//! floor 0: at QP_MIN the decade of complexity clamps, a measured correction), an invalid qcomp refused; (n) an I picture planned
//! from its GOP's P pictures less 3, one with no P after it from its own complexity.
//! H264ENCTWOPASSBLURINTRA (LEDGER SR751): (o) the coder's intra counts (an IDR all of its mbs, a P / B none); (p) x264's chain weight
//! 1 - (intra / mbs)^2 cuts the blur at a mostly-intra P (a record: the coder has no intra-in-P decision); (q) the v2 text round-trips
//! the counts, a v1 text and a bad count refuse by name. Test f's tampered line keeps the v2 count token.

use h264_core::Decoder;
use h264enc_core::rc::{Pass2Opts, PicStat, QP_MAX, QP_MIN, QP_STEP, STATS_VERSION, StatsError, blur_complexity, exp_neg_q16, intra_keep_q16, pass2_qps, qpstep_clamp, qpstep_limit, read_stats, rescale_stats, step16, write_stats};
use h264enc_core::{Access, Config, Encoder, Error, Pic, RateControl, Yuv420};

const W: usize = 128;
const H: usize = 96;
const FPS: u32 = 30;
const N: usize = 30;

/// tests/rc.rs's scene (copied: integration tests share no module): a textured gradient, a square moving 3 samples per frame
/// right (wrapping) and a patterned block moving 2 per frame down.
fn scene(t: usize) -> [Vec<u8>; 3] {
    let mut p = [vec![0u8; W * H], vec![0u8; W * H / 4], vec![0u8; W * H / 4]];
    let (ax, ay) = ((3 * t) % (W - 16), 20);
    let (bx, by) = (80, (2 * t) % (H - 24));
    for y in 0..H {
        for x in 0..W {
            let tex = ((x * 7 + y * 13) % 23) as i32 - 11;
            let bg = (40 + x * 100 / W + y * 60 / H) as i32 + tex;
            let a = x >= ax && x < ax + 16 && y >= ay && y < ay + 16;
            let b = x >= bx && x < bx + 24 && y >= by && y < by + 24;
            p[0][y * W + x] = if a { 220 } else if b { (60 + ((x + y) % 8) * 12) as u8 } else { bg.clamp(0, 255) as u8 };
        }
    }
    for y in 0..H / 2 {
        for x in 0..W / 2 {
            let a = 2 * x >= ax && 2 * x < ax + 16 && 2 * y >= ay && 2 * y < ay + 16;
            p[1][y * (W / 2) + x] = if a { 90 } else { (100 + x / 2) as u8 };
            p[2][y * (W / 2) + x] = if a { 200 } else { (150 - y / 2) as u8 };
        }
    }
    p
}

fn frame(p: &[Vec<u8>; 3]) -> Yuv420<'_> {
    Yuv420 { width: W as u32, height: H as u32, y: &p[0], u: &p[1], v: &p[2], y_stride: W, uv_stride: W / 2 }
}

fn base(rate: Option<RateControl>) -> Config {
    Config { width: W as u32, height: H as u32, qp: 26, gop: 30, rate, ..Default::default() }
}

fn au_bits(a: &Access) -> u64 {
    a.nals.iter().map(|n| n.len() as u64 * 8).sum()
}

fn predicted(stats: &[PicStat], qps: &[i32]) -> u64 {
    stats.iter().zip(qps).map(|(p, &q)| p.complexity / step16(q)).sum()
}

#[test]
fn a_pass2_qps_spends_where_the_complexity_is() {
    // 60 pictures, an IDR every 30, P complexities spread over a decade (bits x step16 at QP 26).
    let stats: Vec<PicStat> = (0..60u64)
        .map(|i| {
            let kind = if i % 30 == 0 { Pic::Idr } else { Pic::P };
            let bits = if kind == Pic::Idr { 60_000 } else { 2_000 + (i * 7919 % 61) * 300 };
            PicStat { kind, bits, qp: 26, complexity: bits * step16(26), intra_mbs: if kind == Pic::Idr { 48 } else { 0 }, mbs: 48 }
        })
        .collect();
    let first: u64 = stats.iter().map(|p| p.bits).sum();
    for target in [first / 2, first, first * 2] {
        let qps = pass2_qps(&stats, target, QP_MIN, &RAW);
        assert_eq!(qps.len(), stats.len());
        assert!(qps.iter().all(|q| (QP_MIN..=QP_MAX).contains(q)));
        let got = predicted(&stats, &qps);
        println!("twopass plan target={target} predicted={got} ({:+.2}%) qps={qps:?}", (got as f64 / target as f64 - 1.0) * 100.0);
        assert!(got <= target && got as f64 >= target as f64 * 0.98, "plan {got} vs target {target}");
        // The most and least complex P pictures: the textured one at the higher QP, still spending more bits.
        let ps: Vec<usize> = (0..stats.len()).filter(|&i| stats[i].kind == Pic::P).collect();
        let hi = *ps.iter().max_by_key(|&&i| stats[i].complexity).unwrap();
        let lo = *ps.iter().min_by_key(|&&i| stats[i].complexity).unwrap();
        assert!(qps[hi] > qps[lo], "textured QP {} vs flat {}", qps[hi], qps[lo]);
        assert!(stats[hi].complexity / step16(qps[hi]) > stats[lo].complexity / step16(qps[lo]));
    }
    // Unreachable targets: every QP at 51, or every QP at the floor.
    assert!(pass2_qps(&stats, 1, QP_MIN, &RAW).iter().all(|&q| q == QP_MAX));
    assert!(pass2_qps(&stats, u64::MAX / 2, 20, &RAW).iter().all(|&q| q == 20));
}

/// Encode N frames under `cfg` (planned from `plan`), decoding each through h264_core against the reconstruction.
fn run(cfg: Config, plan: &[PicStat]) -> (Vec<u64>, Vec<PicStat>) {
    let mut enc = Encoder::new(cfg).unwrap().planned(plan).unwrap();
    let mut dec = Decoder::new();
    let mut sizes = Vec::new();
    for t in 0..N {
        let p = scene(t);
        let au = enc.encode(&frame(&p)).unwrap();
        dec.decode_annexb(&au.annexb()).unwrap();
        dec.end_access_unit();
        let mut f = dec.take_frames();
        assert_eq!(f.len(), 1);
        let f = f.pop().unwrap();
        let rec = enc.reconstruction();
        for c in 0..3 {
            assert_eq!(f.planes[c].as_slice(), rec[c], "{:?} frame {t} plane {c}: decode differs from the reconstruction", cfg.rate);
        }
        sizes.push(au_bits(&au));
    }
    (sizes, enc.stats().to_vec())
}

fn spread(sizes: &[u64]) -> f64 {
    let p = &sizes[1..];
    *p.iter().max().unwrap() as f64 / *p.iter().min().unwrap().max(&1) as f64
}

#[test]
fn b_two_pass_lands_within_two_percent() {
    for kbps in [60u32, 100] {
        let (cbr, stats) = run(base(Some(RateControl::cbr(kbps, (FPS, 1)))), &[]);
        let (two, _) = run(base(Some(RateControl::two_pass(kbps, (FPS, 1)))), &stats);
        let target = kbps as f64 * 1000.0 * N as f64 / FPS as f64;
        let (c, t) = (cbr.iter().sum::<u64>() as f64, two.iter().sum::<u64>() as f64);
        println!(
            "twopass kbps={kbps} cbr={:.2} two={:.2} ({:+.2}%) spread cbr={:.2} two={:.2}",
            c * FPS as f64 / N as f64 / 1000.0,
            t * FPS as f64 / N as f64 / 1000.0,
            (t / target - 1.0) * 100.0,
            spread(&cbr),
            spread(&two)
        );
        println!("  cbr bits {cbr:?}");
        println!("  two bits {two:?}");
        assert!((t - target).abs() <= target * 0.02, "two-pass {t} bits vs target {target}");
        assert!(spread(&two) <= spread(&cbr), "two-pass P spread {:.2} wider than CBR's {:.2}", spread(&two), spread(&cbr));
    }
}

#[test]
fn c_stats_one_record_per_picture() {
    // Constant QP and CBR through encode.
    for rate in [None, Some(RateControl::cbr(60, (FPS, 1)))] {
        let mut enc = Encoder::new(base(rate)).unwrap();
        let mut aus = Vec::new();
        for t in 0..8 {
            aus.push(enc.encode(&frame(&scene(t))).unwrap());
        }
        let s = enc.stats();
        assert_eq!(s.len(), aus.len());
        for (r, a) in s.iter().zip(&aus) {
            assert_eq!(r.bits, au_bits(a));
            assert_eq!(r.kind == Pic::Idr, a.keyframe);
            assert_eq!(r.complexity, r.bits * step16(r.qp));
        }
    }
    // B pictures through push / flush: one record per unit, in coding order.
    let mut enc = Encoder::new(Config { bframes: 2, gop: 8, ..base(None) }).unwrap();
    let mut aus = Vec::new();
    for t in 0..10 {
        aus.extend(enc.push(&frame(&scene(t))).unwrap());
    }
    aus.extend(enc.flush().unwrap());
    let s = enc.stats();
    assert_eq!(s.len(), aus.len());
    assert!(s.iter().any(|r| r.kind == Pic::B));
    for (r, a) in s.iter().zip(&aus) {
        assert_eq!(r.bits, au_bits(a));
        assert_eq!(r.kind == Pic::Idr, a.keyframe);
    }
    // A plan needs TwoPass; an empty plan changes nothing (TwoPass without a plan is CBR byte for byte).
    let one = [PicStat { kind: Pic::Idr, bits: 1000, qp: 26, complexity: 1000 * step16(26), intra_mbs: 48, mbs: 48 }];
    assert!(matches!(Encoder::new(base(Some(RateControl::cbr(60, (FPS, 1))))).unwrap().planned(&one), Err(Error::Rate)));
    let bytes = |rate| {
        let mut enc = Encoder::new(base(Some(rate))).unwrap().planned(&[]).unwrap();
        (0..6).flat_map(|t| enc.encode(&frame(&scene(t))).unwrap().annexb()).collect::<Vec<u8>>()
    };
    assert_eq!(bytes(RateControl::two_pass(60, (FPS, 1))), bytes(RateControl::cbr(60, (FPS, 1))));
}

// ---- H264ENCTWOPASSFILE (LEDGER SR719): the first pass's records as text (named tail block) ----

/// The CBR first pass of tests (b) at `kbps`: its records.
fn first_pass(kbps: u32) -> Vec<PicStat> {
    run(base(Some(RateControl::cbr(kbps, (FPS, 1)))), &[]).1
}

fn text_of(pics: &[PicStat]) -> String {
    let mut s = String::new();
    write_stats(pics, (FPS, 1), &mut s).unwrap();
    s
}

#[test]
fn d_stats_text_round_trips() {
    // I and P from the CBR first pass; I, P and B from a reordered constant-QP stream.
    let mut enc = Encoder::new(Config { bframes: 2, gop: 8, ..base(None) }).unwrap();
    for t in 0..10 {
        enc.push(&frame(&scene(t))).unwrap();
    }
    enc.flush().unwrap();
    let with_b = enc.stats().to_vec();
    assert!(with_b.iter().any(|p| p.kind == Pic::B));
    for pics in [first_pass(60), with_b] {
        let text = text_of(&pics);
        assert!(text.starts_with(&format!("h264enc-stats v{STATS_VERSION} frames={} fps={FPS}/1\n", pics.len())), "{text}");
        assert_eq!(text.lines().count(), pics.len() + 1);
        let back = read_stats(&text).unwrap();
        assert_eq!(back.fps, (FPS, 1));
        assert_eq!(back.pics, pics);
        assert_eq!(text_of(&back.pics), text, "write(read(text)) is the text byte for byte");
    }
}

#[test]
fn e_file_fed_pass2_equals_in_process() {
    let bytes = |plan: &[PicStat]| {
        let mut enc = Encoder::new(base(Some(RateControl::two_pass(60, (FPS, 1))))).unwrap().planned(plan).unwrap();
        (0..N).flat_map(|t| enc.encode(&frame(&scene(t))).unwrap().annexb()).collect::<Vec<u8>>()
    };
    let stats = first_pass(60);
    let read = read_stats(&text_of(&stats)).unwrap().pics;
    let (mem, file) = (bytes(&stats), bytes(&read));
    println!("twopass file pass2 bytes={} in-process bytes={}", file.len(), mem.len());
    assert_eq!(file, mem, "a pass 2 from the stats text codes the in-process pass 2's bytes");
}

#[test]
fn f_stats_refusals_by_name() {
    let pics = first_pass(60);
    let text = text_of(&pics);
    let n = pics.len();
    let lines: Vec<&str> = text.lines().collect();
    let join = |l: &[String]| l.iter().map(|s| format!("{s}\n")).collect::<String>();
    let owned: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
    // Truncated at a line end: the count; mid-line: that line.
    assert_eq!(read_stats(&join(&owned[..n])), Err(StatsError::Count { header: n, lines: n - 1 }));
    assert_eq!(read_stats(&text[..text.len() - 3]), Err(StatsError::Line(n + 1)));
    // The header's count, version, shape.
    let mut h = owned.clone();
    h[0] = format!("h264enc-stats v{STATS_VERSION} frames={} fps={FPS}/1", n - 1);
    assert_eq!(read_stats(&join(&h)), Err(StatsError::Count { header: n - 1, lines: n }));
    h[0] = format!("h264enc-stats v{} frames={n} fps={FPS}/1", STATS_VERSION + 1);
    assert_eq!(read_stats(&join(&h)), Err(StatsError::Header));
    h[0] = format!("h264enc-stats v{STATS_VERSION} frames={n} fps={FPS}/0");
    assert_eq!(read_stats(&join(&h)), Err(StatsError::Header));
    assert_eq!(read_stats(""), Err(StatsError::Header));
    // A kind other than I / P / B, a tampered complexity, a fifth field.
    let mut k = owned.clone();
    k[1].replace_range(0..1, "X");
    let e = read_stats(&join(&k)).unwrap_err();
    assert_eq!(e, StatsError::Line(2));
    assert!(e.to_string().contains("bad line 2"), "{e}");
    let mut c = owned.clone();
    let f: Vec<u64> = c[2].split(' ').skip(1).take(3).map(|x| x.parse().unwrap()).collect();
    c[2] = format!("{} {} {} {} {}", &owned[2][..1], f[0], f[1], f[2] + 1, owned[2].split(' ').nth(4).unwrap());
    assert_eq!(read_stats(&join(&c)), Err(StatsError::Line(3)));
    let mut x = owned.clone();
    x[3].push_str(" 7");
    assert_eq!(read_stats(&join(&x)), Err(StatsError::Line(4)));
}

// ---- H264ENCTWOPASSRESCALE (LEDGER SR727): a first pass rescaled to another frame count (named tail block) ----

#[test]
fn g_rescale_30_to_15_means_pairs() {
    let s = first_pass(60);
    assert_eq!(s.len(), N);
    let r = rescale_stats(&s, 15);
    assert_eq!(r.len(), 15);
    assert_eq!(r[0].kind, Pic::Idr, "the first I kept");
    for (j, p) in r.iter().enumerate() {
        let mean = (s[2 * j].complexity + s[2 * j + 1].complexity) / 2;
        let step = step16(p.qp);
        assert!(p.complexity <= mean && p.complexity + step > mean, "record {j}: complexity {} vs pair mean {mean}", p.complexity);
        assert_eq!((p.kind, p.qp), (s[2 * j].kind, s[2 * j].qp), "record {j}: the lower-middle record's kind and qp");
        assert_eq!(p.complexity, p.bits * step);
    }
    assert_eq!(read_stats(&text_of(&r)).unwrap().pics, r, "a rescaled vector round-trips the stats text");
    assert_eq!(rescale_stats(&s, N), s, "equal counts: the records unchanged");
    assert!(rescale_stats(&s, 0).is_empty());
    assert!(rescale_stats(&[], 7).is_empty());
    // Upsampled 15 -> 30: each picture derived from record j / 2.
    let u = rescale_stats(&r, 30);
    assert_eq!(u.len(), 30);
    assert_eq!(u[0].kind, Pic::Idr);
    for (j, p) in u.iter().enumerate() {
        let q = r[j / 2];
        assert_eq!((p.kind, p.qp, p.complexity), (q.kind, q.qp, q.complexity), "upsampled {j}");
    }
}

#[test]
fn h_range_planned_from_whole_pass_lands_within_3_percent() {
    const M: usize = 15;
    let encode = |kbps: u32, plan: &[PicStat], n: usize| -> (u64, Vec<u8>) {
        let mut enc = Encoder::new(base(Some(RateControl::two_pass(kbps, (FPS, 1))))).unwrap().planned(plan).unwrap();
        let mut dec = Decoder::new();
        let (mut bits, mut bytes) = (0u64, Vec::new());
        for t in 0..n {
            let p = scene(t);
            let au = enc.encode(&frame(&p)).unwrap();
            dec.decode_annexb(&au.annexb()).unwrap();
            dec.end_access_unit();
            let f = dec.take_frames().pop().unwrap();
            let rec = enc.reconstruction();
            for c in 0..3 {
                assert_eq!(f.planes[c].as_slice(), rec[c], "kbps {kbps} frame {t} plane {c}: decode differs from the reconstruction");
            }
            bits += au_bits(&au);
            bytes.extend(au.annexb());
        }
        (bits, bytes)
    };
    for kbps in [60u32, 100] {
        let stats = first_pass(kbps);
        let (bits, _) = encode(kbps, &rescale_stats(&stats, M), M);
        let target = kbps as f64 * 1000.0 * M as f64 / FPS as f64;
        println!("twopass rescale 30->15 kbps={kbps} bits={bits} target={target:.0} ({:+.2}%)", (bits as f64 / target - 1.0) * 100.0);
        assert!((bits as f64 - target).abs() <= target * 0.03, "rescaled pass 2 {bits} bits vs target {target}");
        assert_eq!(encode(kbps, &rescale_stats(&stats, N), N).1, encode(kbps, &stats, N).1, "equal counts: the same bytes");
    }
}

#[test]
fn i_note_stat_skips_filler() {
    let mut enc = Encoder::new(Config { hrd: true, ..base(Some(RateControl::cbr(2000, (FPS, 1)))) }).unwrap();
    let aus: Vec<Access> = (0..60).map(|t| enc.encode(&frame(&scene(t))).unwrap()).collect();
    let filler = |a: &Access| -> u64 { a.nals.iter().filter(|n| n[0] & 0x1f == h264_core::nal::NAL_FILLER).map(|n| n.len() as u64 * 8).sum() };
    assert!(aus.iter().any(|a| filler(a) > 0), "the filler path was not exercised");
    let s = enc.stats();
    assert_eq!(s.len(), aus.len());
    for (r, a) in s.iter().zip(&aus) {
        assert_eq!(r.bits, au_bits(a) - filler(a), "a record's bits leave the filler out");
        assert_eq!(r.complexity, r.bits * step16(r.qp));
    }
}

// ---- H264ENCTWOPASSBLUR (LEDGER SR736): x264's complexity blur, qblur and GOP-mean I rule, the qcomp knob (named tail block) ----

/// SR698's plan: no complexity blur, no QP blur, an I picture from its next P's complexity.
const RAW: Pass2Opts = Pass2Opts { qcomp: (3, 5), cplxblur: 0, qblur_q8: 0, i_from_gop: false, qpstep: 0 };

/// Test (a)'s first pass: 60 pictures, an IDR every 30, P complexities spread over a decade.
fn synthetic() -> Vec<PicStat> {
    (0..60u64)
        .map(|i| {
            let kind = if i % 30 == 0 { Pic::Idr } else { Pic::P };
            let bits = if kind == Pic::Idr { 60_000 } else { 2_000 + (i * 7919 % 61) * 300 };
            PicStat { kind, bits, qp: 26, complexity: bits * step16(26), intra_mbs: if kind == Pic::Idr { 48 } else { 0 }, mbs: 48 }
        })
        .collect()
}

fn pic(kind: Pic, bits: u64) -> PicStat {
    PicStat { kind, bits, qp: 26, complexity: bits * step16(26), intra_mbs: if kind == Pic::Idr { 48 } else { 0 }, mbs: 48 }
}

#[test]
fn j_blur_complexity_spreads_an_impulse_and_stops_at_an_idr() {
    assert_eq!(exp_neg_q16(0), 65536);
    assert_eq!(exp_neg_q16(12 << 16), 0);
    for t in [327u64, 1 << 15, 65536, 4 << 16, 8 << 16, (11 << 16) + 5000] {
        let e = (65536.0 * (-(t as f64) / 65536.0).exp()).round() as i64;
        let got = exp_neg_q16(t) as i64;
        assert!((got - e).abs() <= 1, "exp_neg_q16({t}) = {got} vs {e}");
    }
    // An IDR at 0 and 45, P pictures of complexity c, one 10c P at 20.
    let c = 1000 * step16(26);
    let stats: Vec<PicStat> = (0..60).map(|i| if i % 45 == 0 { pic(Pic::Idr, 30_000) } else if i == 20 { pic(Pic::P, 10_000) } else { pic(Pic::P, 1000) }).collect();
    assert_eq!(blur_complexity(&stats, 0), stats.iter().map(|p| p.complexity).collect::<Vec<_>>(), "cplxblur 0: each its own");
    for blur in [2u32, 20] {
        let b = blur_complexity(&stats, blur);
        println!("twopassblur impulse cplxblur={blur} 16..=24 {:?} 43..=46 {:?}", &b[16..=24], &b[43..=46]);
        assert!(b[20] < 10 * c && b[20] > c, "the peak spread");
        assert!(b[19] > c && b[21] > c, "its neighbours raised");
    }
    // cplxblur 2 (at 20 the window reaches the IDR at 0 too, so 16..=24 fall with the distance from it).
    let b = blur_complexity(&stats, 2);
    assert!(b[18] <= b[19] && b[22] <= b[21] && b[19] <= b[20] && b[21] <= b[20], "falling away from the peak");
    // cplxblur 2: past j = 0..=4, future j = 1..=3; the impulse at 20 reaches 17..=24 only, and the picture before the IDR at 45
    // does not see past it (44: its own and four before, all c).
    assert_eq!(b[16], c);
    assert_eq!(b[25], c);
    assert_eq!(b[44], c, "the future walk stops before the next IDR");
    assert!(b[17] > c && b[24] > c);
    assert_eq!(&b[16..=24], &J_IMPULSE_2[..], "cplxblur 2 pinned");
}

/// blur_complexity(cplxblur 2) of test (j)'s impulse, pictures 16..=24 (measured on this tree).
const J_IMPULSE_2: [u64; 9] = [208000, 437863, 443685, 447248, 448444, 447248, 443685, 437863, 429960];

#[test]
fn k_raw_reproduces_sr698() {
    let stats = synthetic();
    let first: u64 = stats.iter().map(|p| p.bits).sum();
    for (target, want) in [(first / 2, &K_SR698[0]), (first, &K_SR698[1]), (first * 2, &K_SR698[2])] {
        assert_eq!(&pass2_qps(&stats, target, QP_MIN, &RAW)[..], &want[..], "RAW at target {target} is SR698's plan");
    }
}

/// SR698's plans of `synthetic()` at first / 2, first, first x 2 (test (a)'s printed `qps=`, captured on the tree before SR736).
const K_SR698: [[i32; 60]; 3] = [
    [31, 34, 33, 32, 31, 29, 34, 34, 33, 32, 30, 27, 34, 33, 32, 31, 29, 34, 34, 33, 32, 30, 27, 34, 33, 32, 31, 29, 34, 34, 29, 32, 30, 28, 34, 33, 33, 31, 30, 34, 34, 33, 32, 31, 28, 34, 34, 33, 31, 30, 35, 34, 33, 32, 31, 28, 34, 34, 33, 32],
    [25, 28, 27, 26, 25, 23, 28, 28, 27, 26, 24, 21, 28, 27, 26, 25, 23, 28, 28, 27, 26, 24, 21, 28, 27, 26, 25, 23, 28, 28, 23, 26, 24, 22, 28, 27, 27, 25, 24, 28, 28, 27, 26, 25, 22, 28, 28, 27, 25, 24, 29, 28, 27, 26, 25, 22, 28, 28, 27, 26],
    [19, 22, 21, 20, 19, 17, 22, 22, 21, 20, 18, 16, 22, 21, 20, 19, 17, 22, 22, 21, 20, 18, 16, 22, 21, 20, 19, 17, 22, 22, 17, 20, 18, 16, 22, 21, 21, 19, 18, 22, 22, 21, 20, 19, 16, 22, 22, 21, 19, 18, 23, 22, 21, 20, 19, 16, 22, 22, 21, 20],
];

#[test]
fn l_default_blur_lands_within_two_percent() {
    // (bits per picture, SliceQPY per picture) of a second pass planned with `opts`.
    let encode = |kbps: u32, plan: &[PicStat], opts: &Pass2Opts| -> (Vec<u64>, Vec<i32>) {
        let mut enc = Encoder::new(base(Some(RateControl::two_pass(kbps, (FPS, 1))))).unwrap().planned_with(plan, opts).unwrap();
        let bits = (0..N).map(|t| au_bits(&enc.encode(&frame(&scene(t))).unwrap())).collect();
        (bits, enc.stats().iter().map(|p| p.qp).collect())
    };
    // The P pictures' QP range (the first picture is the IDR).
    let qrange = |q: &[i32]| q[1..].iter().max().unwrap() - q[1..].iter().min().unwrap();
    for kbps in [60u32, 100] {
        let stats = first_pass(kbps);
        let ((raw, rq), (blur, bq)) = (encode(kbps, &stats, &RAW), encode(kbps, &stats, &Pass2Opts::default()));
        let target = kbps as f64 * 1000.0 * N as f64 / FPS as f64;
        // The plans themselves at the export's target (`Encoder::planned_with`'s target, floor QP_MIN).
        let tb = kbps as u64 * 1000 * N as u64 / FPS as u64;
        let (pr, pb) = (pass2_qps(&stats, tb, QP_MIN, &RAW), pass2_qps(&stats, tb, QP_MIN, &Pass2Opts::default()));
        let (r, b) = (raw.iter().sum::<u64>() as f64, blur.iter().sum::<u64>() as f64);
        println!(
            "twopassblur kbps={kbps} raw={:+.2}% blur={:+.2}% planned P qp range raw={} blur={}; coded P qp range raw={} blur={}; bit spread raw={:.2} blur={:.2}",
            (r / target - 1.0) * 100.0,
            (b / target - 1.0) * 100.0,
            qrange(&pr),
            qrange(&pb),
            qrange(&rq),
            qrange(&bq),
            spread(&raw),
            spread(&blur)
        );
        println!("  planned raw {pr:?}\n  planned blur {pb:?}\n  coded raw {rq:?}\n  coded blur {bq:?}");
        assert!((b - target).abs() <= target * 0.02, "blurred two-pass {b} bits vs target {target}");
        assert!(qrange(&pb) <= qrange(&pr), "blurred planned P QP range {} wider than RAW's {}", qrange(&pb), qrange(&pr));
        let mut via_planned = Encoder::new(base(Some(RateControl::two_pass(kbps, (FPS, 1))))).unwrap().planned(&stats).unwrap();
        let p: Vec<u64> = (0..N).map(|t| au_bits(&via_planned.encode(&frame(&scene(t))).unwrap())).collect();
        assert_eq!(p, blur, "planned is planned_with(Default)");
    }
}

#[test]
fn m_qcomp_bounds_and_refusals() {
    let stats = synthetic();
    let first: u64 = stats.iter().map(|p| p.bits).sum();
    let ps: Vec<usize> = (0..stats.len()).filter(|&i| stats[i].kind == Pic::P).collect();
    // qcomp 1: one QP for every P (the default blurs too).
    for opts in [Pass2Opts { qcomp: (1, 1), ..RAW }, Pass2Opts { qcomp: (1, 1), ..Pass2Opts::default() }] {
        let q = pass2_qps(&stats, first, QP_MIN, &opts);
        assert!(ps.iter().all(|&i| q[i] == q[ps[0]]), "qcomp 1 {opts:?}: {q:?}");
    }
    // qcomp 0: qscale ~ complexity, every P's predicted bits within the step16 rounding of equal (one QP step); floor 0 so the
    // decade of complexity (20 QP) is not clamped at QP_MIN.
    let bits = |opts: &Pass2Opts| -> f64 {
        let q = pass2_qps(&stats, first, 0, opts);
        let b: Vec<u64> = ps.iter().map(|&i| stats[i].complexity / step16(q[i])).collect();
        *b.iter().max().unwrap() as f64 / *b.iter().min().unwrap() as f64
    };
    let (flat, raw) = (bits(&Pass2Opts { qcomp: (0, 1), ..RAW }), bits(&RAW));
    println!("twopassblur qcomp=0 P bits max/min={flat:.3} qcomp=0.6 {raw:.3}");
    assert!(flat <= 1.25 && flat < raw, "qcomp 0 P bits spread {flat:.3} (qcomp 0.6: {raw:.3})");
    // Invalid qcomp: refused by name.
    let plan = first_pass(60);
    for qcomp in [(6, 5), (1, 0), (-1, 5)] {
        let e = Encoder::new(base(Some(RateControl::two_pass(60, (FPS, 1))))).unwrap().planned_with(&plan, &Pass2Opts { qcomp, ..Pass2Opts::default() });
        assert!(matches!(e, Err(Error::Rate)), "qcomp {qcomp:?}");
    }
}

#[test]
fn n_i_picture_from_its_gop() {
    // GOP 1: equal P pictures; GOP 2: varied; a last IDR with no P after it, its complexity the P at 13's.
    let mut stats: Vec<PicStat> = Vec::new();
    stats.push(pic(Pic::Idr, 40_000));
    stats.extend((1..10).map(|_| pic(Pic::P, 3000)));
    stats.push(pic(Pic::Idr, 40_000));
    stats.extend((11..20u64).map(|i| pic(Pic::P, 1000 + i * 400)));
    stats.push(pic(Pic::Idr, 1000 + 13 * 400));
    let gop = Pass2Opts { cplxblur: 0, qblur_q8: 0, i_from_gop: true, ..RAW };
    let total: u64 = stats.iter().map(|p| p.bits).sum();
    let q = pass2_qps(&stats, total, QP_MIN, &gop);
    println!("twopassblur i rule qps={q:?}");
    assert_eq!(q[0], q[1] - 3, "an I at its GOP's (equal) P QP less 3");
    let mean = q[11..20].iter().sum::<i32>() as f64 / 9.0;
    assert!((q[10] as f64 - (mean - 3.0)).abs() <= 1.0, "an I {} at its GOP's mean P QP {mean:.2} less 3", q[10]);
    assert_eq!(q[20], q[13], "an I with no P after it: its own complexity, no offset");
    assert_ne!(pass2_qps(&stats, total, QP_MIN, &RAW)[20], q[20], "RAW takes the previous P's less 3");
}

// ---- H264ENCTWOPASSBLURINTRA (LEDGER SR751): x264's intra weight of the complexity blur's chain (named tail block) ----

#[test]
fn o_intra_counts_from_the_coder() {
    // The CBR first pass (one IDR, 29 P) and a constant-QP stream with B pictures: an IDR codes all 48 macroblocks intra, a P / B
    // none (the coder has no intra-in-P decision: H264ENC-P-INTRA owed; the SR680 cut frame with scenecut off is such a P).
    let mut enc = Encoder::new(Config { bframes: 2, gop: 8, ..base(None) }).unwrap();
    for t in 0..10 {
        enc.push(&frame(&scene(t))).unwrap();
    }
    enc.flush().unwrap();
    for pics in [first_pass(60), enc.stats().to_vec()] {
        let counts: Vec<String> = pics.iter().map(|p| format!("{:?}:{}/{}", p.kind, p.intra_mbs, p.mbs)).collect();
        println!("twopassblurintra counts {}", counts.join(" "));
        for p in &pics {
            assert_eq!(p.mbs, ((W / 16) * (H / 16)) as u32);
            assert_eq!(p.intra_mbs, if p.kind == Pic::Idr { p.mbs } else { 0 }, "{p:?}");
        }
    }
}

#[test]
fn p_intra_weight_cuts_the_chain() {
    let marked = |kind: Pic, bits: u64, intra: u32| PicStat { intra_mbs: intra, ..pic(kind, bits) };
    let mostly = marked(Pic::P, 1000, 44);
    let keep = intra_keep_q16(&mostly);
    println!("twopassblurintra keep 44/48={keep} 48/48={} 0/48={}", intra_keep_q16(&marked(Pic::P, 1000, 48)), intra_keep_q16(&marked(Pic::P, 1000, 0)));
    assert_eq!(keep, 65536 - (44 * 44 * 65536) / (48 * 48));
    assert!(keep < 65536 / 4, "a 44/48 P's link weight {keep} not below 1/4");
    assert_eq!(intra_keep_q16(&marked(Pic::P, 1000, 48)), 0);
    assert_eq!(intra_keep_q16(&marked(Pic::P, 1000, 0)), 65536);
    assert_eq!(intra_keep_q16(&marked(Pic::P, 1000, 60)), 0, "intra clamped to mbs");
    assert_eq!(intra_keep_q16(&PicStat { mbs: 0, intra_mbs: 0, ..pic(Pic::P, 1000) }), 65536, "mbs 0 keeps the link");
    // Test (j)'s stream (IDR at 0 and 45, P pictures of complexity c) with a 10c P at 20 marked 0/48, 44/48, 48/48.
    let c = 1000 * step16(26);
    let stream = |intra: u32| -> Vec<PicStat> {
        (0..60).map(|i| if i % 45 == 0 { pic(Pic::Idr, 30_000) } else if i == 20 { marked(Pic::P, 10_000, intra) } else { pic(Pic::P, 1000) }).collect()
    };
    let (plain, part, full) = (blur_complexity(&stream(0), 2), blur_complexity(&stream(44), 2), blur_complexity(&stream(48), 2));
    println!("twopassblurintra picture 19 at cplxblur 2: 0/48={} 44/48={} 48/48={} c={c}", plain[19], part[19], full[19]);
    assert_eq!(plain, blur_complexity(&stream(0), 2));
    assert_eq!(&plain[16..=24], &J_IMPULSE_2[..], "intra 0: SR736's pins");
    assert!(part[19] < plain[19] && part[19] > c, "x264's partial cut: {} between c {c} and {}", part[19], plain[19]);
    assert_eq!(full[19], c, "an all-intra P cuts the forward walk as an IDR does");
}

#[test]
fn q_stats_v2_text_carries_the_counts() {
    let pics = vec![pic(Pic::Idr, 30_000), PicStat { intra_mbs: 44, ..pic(Pic::P, 1000) }, pic(Pic::P, 1200)];
    let text = text_of(&pics);
    println!("twopassblurintra text {text:?}");
    assert_eq!(STATS_VERSION, 2);
    assert!(text.starts_with("h264enc-stats v2 frames=3 "));
    assert!(text.lines().nth(2).unwrap().ends_with(" 44/48"), "{text}");
    assert_eq!(read_stats(&text).unwrap().pics, pics, "v2 round-trips the counts");
    // A v1 text (its header and four-token lines): refused as Header, as any other version is.
    let v1: String = text
        .lines()
        .enumerate()
        .map(|(i, l)| if i == 0 { l.replacen("v2", "v1", 1) + "\n" } else { l.rsplit_once(' ').unwrap().0.to_string() + "\n" })
        .collect();
    assert_eq!(read_stats(&v1), Err(StatsError::Header));
    // A count above mbs, and a line without the count: refused by line.
    assert_eq!(read_stats(&text.replace(" 44/48", " 49/48")), Err(StatsError::Line(3)));
    assert_eq!(read_stats(&text.replace(" 44/48", "")), Err(StatsError::Line(3)));
}

// ---- H264ENCTWOPASSQPSTEP (LEDGER SR763): x264's get_diff_limited_q qpstep clamp over the plan (named tail block) ----
// (r) a 10x P spike and a lasting 10x step under the default's qpstep 4 with the blurs off (the default's cplxblur 20 spreads a 10x
// spike to ~1.4x, leaving no 4-QP step to limit): every neighbouring P pair within 4 QP (qpstep 0: the spike steps 8), the ramp
// before the step, the I at its GOP's limited mean P QP less 3, the predicted total within 2%; (s) `rc::qpstep_limit` itself: a B
// untouched, no limit across an I, an I against the next I only with no P between nor after it, qpstep 0 the identity; (t) moved
// pins: none. `qpstep 0` on the default plans synthetic()'s pre-SR763 default literals, the default (qpstep 4) the same ones, and
// the scene's first pass plans the same at both qpsteps (tests b / e / h / l unmoved); RAW (qpstep 0) keeps test k's SR698 literals.

/// IDRs at 0 and 30, P pictures of 1000 bits, one 10000-bit P at 12 and 10000-bit P pictures from 45 on.
fn spiked() -> Vec<PicStat> {
    (0..60).map(|i| if i % 30 == 0 { pic(Pic::Idr, 30_000) } else if i == 12 || i >= 45 { pic(Pic::P, 10_000) } else { pic(Pic::P, 1000) }).collect()
}

/// The largest QP step between neighbouring P pictures of `q`.
fn p_step(stats: &[PicStat], q: &[i32]) -> i32 {
    (1..q.len()).filter(|&i| stats[i - 1].kind == Pic::P && stats[i].kind == Pic::P).map(|i| (q[i] - q[i - 1]).abs()).max().unwrap_or(0)
}

#[test]
fn r_qpstep_limits_a_spike() {
    assert_eq!(Pass2Opts::default().qpstep, QP_STEP);
    assert_eq!(QP_STEP, 4, "x264's i_qp_step");
    let stats = spiked();
    let first: u64 = stats.iter().map(|p| p.bits).sum();
    let sharp = Pass2Opts { cplxblur: 0, qblur_q8: 0, ..Pass2Opts::default() };
    for target in [first / 2, first] {
        let free = pass2_qps(&stats, target, QP_MIN, &Pass2Opts { qpstep: 0, ..sharp });
        let held = pass2_qps(&stats, target, QP_MIN, &sharp);
        println!("twopassqpstep spike target={target} qpstep=0 step={} {free:?}", p_step(&stats, &free));
        println!("twopassqpstep spike target={target} qpstep=4 step={} {held:?}", p_step(&stats, &held));
        assert!(p_step(&stats, &free) > 4, "the fixture steps more than 4 QP without the limit");
        assert!(p_step(&stats, &held) <= 4, "every neighbouring P pair within 4 QP: {held:?}");
        assert!(held[44] >= held[45] - 4 && held[12] <= held[13] + 4, "the ramp before the step, the spike held to its next P");
        let mean = held[31..60].iter().sum::<i32>() as f64 / 29.0;
        assert!((held[30] as f64 - (mean - 3.0)).abs() <= 1.0, "the I {} at its GOP's limited mean P QP {mean:.2} less 3", held[30]);
        let got = predicted(&stats, &held);
        assert!(got <= target && got as f64 >= target as f64 * 0.98, "plan {got} vs target {target}");
    }
}

#[test]
fn s_qpstep_limit_rules() {
    let q = |v: &[i64]| -> Vec<i64> { v.iter().map(|&x| x << 16).collect() };
    // P pictures: a B untouched, the P at 3 held to the P at 4, the P at 1 to the (held) P at 3, the P at 4 not across the IDR at 5.
    let kinds = [Pic::Idr, Pic::P, Pic::B, Pic::P, Pic::P, Pic::Idr, Pic::P];
    let mut v = q(&[20, 35, 99, 20, 30, 40, 10]);
    qpstep_limit(&mut v, &kinds, 4, Pic::P);
    assert_eq!(v, q(&[20, 30, 99, 26, 30, 40, 10]));
    // I pictures: the I at 0 held to the I at 1 (no P between, none after 1 before 2); the I at 1 not (a P follows the I at 2); the
    // I at 2 not (a P is its next non-B).
    let kinds = [Pic::Idr, Pic::Idr, Pic::Idr, Pic::P, Pic::Idr];
    let mut v = q(&[10, 30, 50, 0, 90]);
    qpstep_limit(&mut v, &kinds, 4, Pic::Idr);
    assert_eq!(v, q(&[26, 30, 50, 0, 90]));
    // qpstep 0: the identity.
    let mut v = q(&[20, 35, 99, 20, 30, 40, 10]);
    qpstep_limit(&mut v, &[Pic::Idr, Pic::P, Pic::B, Pic::P, Pic::P, Pic::Idr, Pic::P], 0, Pic::P);
    assert_eq!(v, q(&[20, 35, 99, 20, 30, 40, 10]));
}

#[test]
fn t_qpstep_zero_is_the_pre_sr763_default() {
    let stats = synthetic();
    let first: u64 = stats.iter().map(|p| p.bits).sum();
    for (target, want) in [(first / 2, &T_DEFAULT_PRE[0]), (first, &T_DEFAULT_PRE[1]), (first * 2, &T_DEFAULT_PRE[2])] {
        let held = pass2_qps(&stats, target, QP_MIN, &Pass2Opts::default());
        println!("twopassqpstep synthetic default target={target} {held:?}");
        assert_eq!(&pass2_qps(&stats, target, QP_MIN, &Pass2Opts { qpstep: 0, ..Pass2Opts::default() })[..], &want[..], "qpstep 0 is the pre-SR763 default");
        assert_eq!(&held[..], &want[..], "no moved pin: synthetic()'s default plan steps at most 4 QP between P pictures");
    }
    for kbps in [60u32, 100] {
        let scene = first_pass(kbps);
        let tb = kbps as u64 * 1000 * N as u64 / FPS as u64;
        let held = pass2_qps(&scene, tb, QP_MIN, &Pass2Opts::default());
        assert_eq!(held, pass2_qps(&scene, tb, QP_MIN, &Pass2Opts { qpstep: 0, ..Pass2Opts::default() }), "kbps {kbps}: the scene's plan unmoved");
    }
}

/// pass2_qps(synthetic(), first / 2, first, first x 2, QP_MIN, Pass2Opts::default()) captured on the tree BEFORE SR763.
const T_DEFAULT_PRE: [[i32; 60]; 3] = [
    [30, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 32, 30, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33, 33],
    [24, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 24, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27],
    [18, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 18, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21],
];

// ---- H264ENCTWOPASSQPSTEPRUNTIME (LEDGER SR769): x264's lstep clamp on the pass-2 runtime pick (named tail block) ----

/// A 30-picture record that disagrees with scene(): an IDR, P pictures of `p` bits, a `k` x P at 12 and `k` x P from 22 on.
fn rt_plan(p: u64, k: u64) -> Vec<PicStat> {
    (0..N).map(|i| if i == 0 { pic(Pic::Idr, 30 * p) } else if i == 12 || i >= 22 { pic(Pic::P, k * p) } else { pic(Pic::P, p) }).collect()
}

/// Dense noise (an LCG per sample) for picture `t`: far costlier than scene().
fn noise(t: usize) -> [Vec<u8>; 3] {
    let mut s = (t as u32).wrapping_mul(2654435761) | 1;
    let mut next = || {
        s = s.wrapping_mul(1664525).wrapping_add(1013904223);
        (s >> 24) as u8
    };
    let y = (0..W * H).map(|_| next()).collect();
    let u = (0..W * H / 4).map(|_| next()).collect();
    let v = (0..W * H / 4).map(|_| next()).collect();
    [y, u, v]
}

/// (bits, SliceQPY) per picture of a two-pass encode at `kbps` planned from `plan` with `opts`, picture t from `src(t)`.
fn rt_encode(kbps: u32, plan: &[PicStat], opts: &Pass2Opts, src: &dyn Fn(usize) -> [Vec<u8>; 3]) -> (Vec<u64>, Vec<i32>) {
    let mut enc = Encoder::new(base(Some(RateControl::two_pass(kbps, (FPS, 1))))).unwrap().planned_with(plan, opts).unwrap();
    let bits = (0..N).map(|t| au_bits(&enc.encode(&frame(&src(t))).unwrap())).collect();
    (bits, enc.stats().iter().map(|p| p.qp).collect())
}

/// The largest same-type coded QP step between neighbouring P pictures of `q` (the plan's kinds).
fn rt_step(q: &[i32]) -> i32 {
    (2..q.len()).map(|i| (q[i] - q[i - 1]).abs()).max().unwrap_or(0)
}

#[test]
fn u_qpstep_clamp_rules() {
    // No last picture of the type, or qpstep 0: unchanged.
    assert_eq!(qpstep_clamp(40, None, 4, (false, false)), 40);
    assert_eq!(qpstep_clamp(40, Some(20), 0, (true, true)), 40);
    // Within +-4 of the last coded: held at either bound, untouched inside.
    assert_eq!(qpstep_clamp(40, Some(30), 4, (false, false)), 34);
    assert_eq!(qpstep_clamp(20, Some(30), 4, (false, false)), 26);
    assert_eq!(qpstep_clamp(32, Some(30), 4, (false, false)), 32);
    // x264's asymmetric widening: over budget the high bound a further step up, under budget the low bound a further step down.
    assert_eq!(qpstep_clamp(40, Some(30), 4, (false, true)), 38);
    assert_eq!(qpstep_clamp(20, Some(30), 4, (false, true)), 26);
    assert_eq!(qpstep_clamp(20, Some(30), 4, (true, false)), 22);
    assert_eq!(qpstep_clamp(40, Some(30), 4, (true, false)), 34);
}

// (v) / (w) fixture, tuned and named: rt_plan(1000, 10) coded from scene() at 60 kbps (the measured premise: qpstep 0 steps 9 QP
// between coded neighbouring P pictures; the pre-SR769 tree at qpstep 4 stepped 5, the plan held within 4 and the runtime `off`
// adding the rest; 30 kbps and k = 30 step the same way, 13 / 6).
const RT_KBPS: u32 = 60;

#[test]
fn v_runtime_qpstep_holds_the_coded_steps() {
    let sharp = Pass2Opts { cplxblur: 0, qblur_q8: 0, ..Pass2Opts::default() };
    let plan = rt_plan(1000, 10);
    let (fb, free) = rt_encode(RT_KBPS, &plan, &Pass2Opts { qpstep: 0, ..sharp }, &scene);
    let (hb, held) = rt_encode(RT_KBPS, &plan, &sharp, &scene);
    let target = RT_KBPS as f64 * 1000.0 * N as f64 / FPS as f64;
    let (f, h) = (fb.iter().sum::<u64>() as f64, hb.iter().sum::<u64>() as f64);
    println!("twopassqpsteprt spike qpstep=0 step={} {:+.2}% {free:?}", rt_step(&free), (f / target - 1.0) * 100.0);
    println!("twopassqpsteprt spike qpstep=4 step={} {:+.2}% {held:?}", rt_step(&held), (h / target - 1.0) * 100.0);
    assert!(rt_step(&free) > 4, "the fixture steps more than 4 QP between coded P pictures without the clamps: {free:?}");
    // X: within qpstep of the last coded P, a further qpstep where `off` says over / under budget (x264's asymmetric lstep).
    assert!(rt_step(&held) <= 2 * 4, "every neighbouring coded P pair within 2 x qpstep: {held:?}");
    assert!(rt_step(&held) < rt_step(&free), "the clamp narrows the coded steps: {held:?} vs {free:?}");
}

#[test]
fn w_qpstep_zero_codes_the_pre_sr769_stream() {
    let raw = Pass2Opts { cplxblur: 0, qblur_q8: 0, qpstep: 0, ..Pass2Opts::default() };
    let (bits, qps) = rt_encode(RT_KBPS, &rt_plan(1000, 10), &raw, &scene);
    assert_eq!(bits, W_PRE_BITS, "qpstep 0: the runtime clamp is off, the coded bytes are the pre-SR769 tree's");
    assert_eq!(qps, W_PRE_QPS);
}

/// rt_encode(RT_KBPS, rt_plan(1000, 10), qpstep 0, scene) captured on the tree BEFORE SR769 (bits, SliceQPY per picture).
const W_PRE_BITS: [u64; 30] = [15104, 1200, 944, 968, 1056, 1224, 1240, 1312, 1184, 1240, 1064, 1064, 360, 2112, 1176, 1192, 1136, 1056, 1264, 1048, 1160, 1288, 520, 560, 536, 1272, 1240, 1440, 1176, 1504];
const W_PRE_QPS: [i32; 30] = [29, 28, 28, 28, 28, 28, 28, 28, 28, 29, 29, 29, 37, 28, 29, 29, 29, 29, 29, 29, 29, 29, 38, 37, 36, 34, 33, 32, 32, 32];

// (x) the bucket wins, measured and named: scene() then dense noise from picture 15 under the scene's own first pass (the plan
// claims them flat). The bucket raise lifts the QP 29 -> 51 in one picture, past any clamp bound (2 x qpstep), at qpstep 4 as at
// 0. Noise costs ~23 kbit a picture at QP 51 against 2 kbit at 60 kbps, so the replayed bucket overflows whatever the QP (the
// first noise picture alone, coded at its pre-noise QP, is ~80 kbit > the 60 kbit bucket): printed, not asserted bounded.
#[test]
fn x_the_bucket_raise_wins_over_the_clamp() {
    let plan = first_pass(RT_KBPS);
    let src = |t: usize| if t < 15 { scene(t) } else { noise(t) };
    let (fb, free) = rt_encode(RT_KBPS, &plan, &Pass2Opts { qpstep: 0, ..Pass2Opts::default() }, &src);
    let (hb, held) = rt_encode(RT_KBPS, &plan, &Pass2Opts::default(), &src);
    let (r, buf) = (RT_KBPS as i64 * 1000 / FPS as i64, RT_KBPS as i64 * 1000);
    let mut full = buf / 2;
    let full_max = hb.iter().map(|&b| { full = (full + b as i64 - r).max(0); full }).max().unwrap();
    println!("twopassqpsteprt bucket step={} full_max={full_max} buf={buf} {held:?}", rt_step(&held));
    assert!(rt_step(&held) > 2 * 4, "the bucket raise steps past any clamp bound: {held:?}");
    assert!(held[16..].iter().all(|&q| q == QP_MAX), "the raise reaches QP_MAX under the clamp: {held:?}");
    assert_eq!(held[16..], free[16..], "the clamp never holds the raise back");
    assert_eq!(hb[16..], fb[16..]);
}
