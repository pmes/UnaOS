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

use h264_core::Decoder;
use h264enc_core::rc::{PicStat, QP_MAX, QP_MIN, STATS_VERSION, StatsError, pass2_qps, read_stats, step16, write_stats};
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
        let qps = pass2_qps(&stats, target, QP_MIN);
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
    assert!(pass2_qps(&stats, 1, QP_MIN).iter().all(|&q| q == QP_MAX));
    assert!(pass2_qps(&stats, u64::MAX / 2, 20).iter().all(|&q| q == 20));
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
