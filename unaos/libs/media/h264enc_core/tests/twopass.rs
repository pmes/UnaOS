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

use h264_core::Decoder;
use h264enc_core::rc::{Pass2Opts, PicStat, QP_MAX, QP_MIN, STATS_VERSION, StatsError, blur_complexity, exp_neg_q16, pass2_qps, read_stats, rescale_stats, step16, write_stats};
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
            PicStat { kind, bits, qp: 26, complexity: bits * step16(26) }
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
    let one = [PicStat { kind: Pic::Idr, bits: 1000, qp: 26, complexity: 1000 * step16(26) }];
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
    let f: Vec<u64> = c[2].split(' ').skip(1).map(|x| x.parse().unwrap()).collect();
    c[2] = format!("{} {} {} {}", &owned[2][..1], f[0], f[1], f[2] + 1);
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
const RAW: Pass2Opts = Pass2Opts { qcomp: (3, 5), cplxblur: 0, qblur_q8: 0, i_from_gop: false };

/// Test (a)'s first pass: 60 pictures, an IDR every 30, P complexities spread over a decade.
fn synthetic() -> Vec<PicStat> {
    (0..60u64)
        .map(|i| {
            let kind = if i % 30 == 0 { Pic::Idr } else { Pic::P };
            let bits = if kind == Pic::Idr { 60_000 } else { 2_000 + (i * 7919 % 61) * 300 };
            PicStat { kind, bits, qp: 26, complexity: bits * step16(26) }
        })
        .collect()
}

fn pic(kind: Pic, bits: u64) -> PicStat {
    PicStat { kind, bits, qp: 26, complexity: bits * step16(26) }
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

