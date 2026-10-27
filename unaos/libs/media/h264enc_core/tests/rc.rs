// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! H264ENCRC (LEDGER SR218): rate control. A 60-frame moving scene (128x96, 30 fps, GOP 30):
//! (a) CBR lands within +-10% of its target over the sequence and a one-second leaky bucket never
//! overflows; (b) VBR lets a complex (noise) picture spend more than CBR at the same mean, with the
//! mean held and the max bucket never exceeded; (c) h264_core decodes every rate-controlled frame
//! to the encoder's reconstruction byte for byte (QPY,pred tracked through slice_qp_delta and
//! mb_qp_delta, deblocking on and off); (d) with `rate: None` the streams are the bytes measured at
//! 1d99c4ae9, before rate control existed.

use h264_core::Decoder;
use h264enc_core::{Access, Config, Encoder, RateControl, RcMode, Yuv420};

const W: usize = 128;
const H: usize = 96;
const FPS: u32 = 30;
const N: usize = 60;

/// The scene: a textured gradient, a square moving 3 samples per frame right (wrapping) and a
/// patterned block moving 2 per frame down.
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

/// The scene with pseudo-random noise (+-16) over its luma at frame `at`: one complex picture.
fn spiked(at: usize) -> impl Fn(usize) -> [Vec<u8>; 3] {
    move |t| {
        let mut p = scene(t);
        if t == at {
            for (i, v) in p[0].iter_mut().enumerate() {
                let h = (i as u32 ^ (t as u32).wrapping_mul(0x9e37_79b1)).wrapping_mul(0x85eb_ca77);
                *v = (*v as i32 + (h >> 27) as i32 - 16).clamp(0, 255) as u8;
            }
        }
        p
    }
}

fn base(qp: u8) -> Config {
    Config { width: W as u32, height: H as u32, qp, gop: 30, ..Default::default() }
}

fn cbr(kbps: u32) -> Option<RateControl> {
    Some(RateControl { bits_per_second: kbps * 1000, fps_num: FPS, fps_den: 1, mode: RcMode::Cbr })
}

/// The bytes of an access unit as the rate controller counts them: every NAL unit.
fn au_bytes(a: &Access) -> usize {
    a.nals.iter().map(|n| n.len()).sum()
}

struct Run {
    sizes: Vec<usize>,
    qps: Vec<u8>,
}

/// Encode `n` frames of `src` under `cfg`; with `decode`, h264_core decodes each and it must equal
/// the encoder's reconstruction byte for byte.
fn run(cfg: Config, src: &dyn Fn(usize) -> [Vec<u8>; 3], decode: bool) -> Run {
    let mut enc = Encoder::new(cfg).unwrap();
    let mut dec = Decoder::new();
    let mut r = Run { sizes: Vec::new(), qps: Vec::new() };
    for t in 0..N {
        let p = src(t);
        let au = enc.encode(&Yuv420 { width: W as u32, height: H as u32, y: &p[0], u: &p[1], v: &p[2], y_stride: W, uv_stride: W / 2 }).unwrap();
        if decode {
            dec.decode_annexb(&au.annexb()).unwrap();
            dec.end_access_unit();
            let mut f = dec.take_frames();
            assert_eq!(f.len(), 1);
            let f = f.pop().unwrap();
            let rec = enc.reconstruction();
            for c in 0..3 {
                assert_eq!(f.planes[c].as_slice(), rec[c], "{:?} frame {t} plane {c}: decode differs from the reconstruction", cfg.rate);
            }
        }
        r.sizes.push(au_bytes(&au));
        r.qps.push(enc.picture_qp());
    }
    r
}

/// The one-second leaky bucket at `bps` (starts half full, drains R per picture, floor 0): its
/// highest fullness in bits.
fn bucket_peak(sizes: &[usize], bps: u32) -> i64 {
    let r = (bps / FPS) as i64;
    let mut f = bps as i64 / 2;
    let mut peak = f;
    for &s in sizes {
        f = (f + s as i64 * 8 - r).max(0);
        peak = peak.max(f);
    }
    peak
}

fn kbps(sizes: &[usize]) -> f64 {
    sizes.iter().sum::<usize>() as f64 * 8.0 * FPS as f64 / sizes.len() as f64 / 1000.0
}

#[test]
fn a_cbr_lands_within_ten_percent_and_never_overflows() {
    for (target, start) in [(60u32, 26u8), (30, 30), (100, 34)] {
        let r = run(Config { rate: cbr(target), ..base(start) }, &scene, false);
        let got = kbps(&r.sizes);
        let peak = bucket_peak(&r.sizes, target * 1000);
        println!("rc cbr {target} kbit/s from QP {start}: {got:.2} kbit/s ({:+.1}%), bucket peak {peak} / {} bits", (got / target as f64 - 1.0) * 100.0, target * 1000);
        println!("  bytes {:?}", r.sizes);
        println!("  qp    {:?}", r.qps);
        assert!((got - target as f64).abs() <= target as f64 * 0.10, "CBR {target}: {got:.2} kbit/s outside +-10%");
        assert!(peak <= (target * 1000) as i64, "CBR {target}: the bucket overflowed ({peak} bits)");
    }
}

#[test]
fn b_vbr_spends_on_the_complex_picture_and_holds_the_mean() {
    let at = 40;
    let src = spiked(at);
    let target = 60u32;
    let vbr = Some(RateControl { bits_per_second: target * 1000, fps_num: FPS, fps_den: 1, mode: RcMode::Vbr { max_bits_per_second: 2 * target * 1000 } });
    let c = run(Config { rate: cbr(target), ..base(26) }, &src, false);
    let v = run(Config { rate: vbr, ..base(26) }, &src, false);
    let (gc, gv) = (kbps(&c.sizes), kbps(&v.sizes));
    let peak = bucket_peak(&v.sizes, 2 * target * 1000);
    println!("rc vbr {target} max {}: {gv:.2} kbit/s; cbr {gc:.2} kbit/s", 2 * target);
    println!("  noise picture {at}: vbr {} bytes qp {}, cbr {} bytes qp {}; vbr max bucket peak {peak} / {} bits", v.sizes[at], v.qps[at], c.sizes[at], c.qps[at], 2 * target * 1000);
    println!("  vbr bytes {:?}", v.sizes);
    println!("  vbr qp {:?}\n  cbr qp {:?}\n  cbr bytes {:?}", v.qps, c.qps, c.sizes);
    assert!(v.sizes[at] > c.sizes[at], "VBR spent {} bytes on the complex picture, CBR {}", v.sizes[at], c.sizes[at]);
    assert!((gv - target as f64).abs() <= target as f64 * 0.10, "VBR mean {gv:.2} kbit/s outside +-10%");
    assert!(peak <= (2 * target * 1000) as i64, "VBR max bucket overflowed ({peak} bits)");
}

#[test]
fn c_rate_controlled_decode_is_the_reconstruction() {
    let vbr = Some(RateControl { bits_per_second: 45_000, fps_num: FPS, fps_den: 1, mode: RcMode::Vbr { max_bits_per_second: 90_000 } });
    for rate in [cbr(60), cbr(25), vbr] {
        for deblock in [false, true] {
            let r = run(Config { rate, deblock, subpel: 2, ..base(26) }, &spiked(40), true);
            let lo = r.qps.iter().min().unwrap();
            let hi = r.qps.iter().max().unwrap();
            println!("rc decode == recon {rate:?} deblock {deblock}: picture QP {lo}..={hi}, {:.2} kbit/s", kbps(&r.sizes));
            assert!(lo != hi, "the picture QP never moved (slice_qp_delta untested)");
        }
    }
}

fn fnv(aus: &[Vec<u8>]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in aus.iter().flatten() {
        h = (h ^ *b as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

#[test]
fn d_rate_none_keeps_the_constant_qp_bytes() {
    for (name, cfg, want) in [
        ("qp26 gop30", base(26), 0x1290_fbb5_41d1_0774u64),
        ("qp30 gop10 deblock subpel2", Config { gop: 10, deblock: true, subpel: 2, ..base(30) }, 0x20e7_2796_bb58_a08b),
        ("qp36 gop1", Config { gop: 1, ..base(36) }, 0x7785_4898_5213_d0db),
    ] {
        let mut enc = Encoder::new(cfg).unwrap();
        let mut samples = Vec::new();
        for t in 0..N {
            let p = scene(t);
            let au = enc.encode(&Yuv420 { width: W as u32, height: H as u32, y: &p[0], u: &p[1], v: &p[2], y_stride: W, uv_stride: W / 2 }).unwrap();
            assert_eq!(enc.picture_qp(), cfg.qp);
            samples.push(au.avcc_sample());
        }
        assert_eq!(fnv(&samples), want, "{name}: rate None moved the bytes measured at 1d99c4ae9");
    }
}

/// H264ENCCABAC (LEDGER SR310): (e) under CABAC the rate-controlled streams (CBR 60 / 25 kbit/s,
/// VBR 45 / 90, deblocking on and off) decode to the reconstruction byte for byte; (f) the three
/// pinned constant-QP streams are unchanged with `entropy` named Cavlc; (g) the CABAC stream of the
/// GOP-30 scene is smaller than the CAVLC one (both printed).
#[test]
fn e_cabac_rate_controlled_decode_is_the_reconstruction() {
    let vbr = Some(RateControl { bits_per_second: 45_000, fps_num: FPS, fps_den: 1, mode: RcMode::Vbr { max_bits_per_second: 90_000 } });
    for rate in [cbr(60), cbr(25), vbr] {
        for deblock in [false, true] {
            let r = run(Config { rate, deblock, subpel: 2, entropy: h264enc_core::Entropy::Cabac, ..base(26) }, &spiked(40), true);
            let lo = r.qps.iter().min().unwrap();
            let hi = r.qps.iter().max().unwrap();
            println!("cabac rc decode == recon {rate:?} deblock {deblock}: picture QP {lo}..={hi}, {:.2} kbit/s", kbps(&r.sizes));
        }
    }
}

#[test]
fn f_cavlc_named_keeps_the_pinned_bytes() {
    let cavlc = |c: Config| Config { entropy: h264enc_core::Entropy::Cavlc, ..c };
    for (name, cfg, want) in [
        ("qp26 gop30", cavlc(base(26)), 0x1290_fbb5_41d1_0774u64),
        ("qp30 gop10 deblock subpel2", cavlc(Config { gop: 10, deblock: true, subpel: 2, ..base(30) }), 0x20e7_2796_bb58_a08b),
        ("qp36 gop1", cavlc(Config { gop: 1, ..base(36) }), 0x7785_4898_5213_d0db),
    ] {
        let mut enc = Encoder::new(cfg).unwrap();
        let mut samples = Vec::new();
        for t in 0..N {
            let p = scene(t);
            samples.push(enc.encode(&Yuv420 { width: W as u32, height: H as u32, y: &p[0], u: &p[1], v: &p[2], y_stride: W, uv_stride: W / 2 }).unwrap().avcc_sample());
        }
        assert_eq!(fnv(&samples), want, "{name}: Entropy::Cavlc moved the pinned bytes");
    }
}

#[test]
fn g_cabac_is_smaller_on_the_gop30_scene() {
    let cavlc = run(base(26), &scene, true);
    let cabac = run(Config { entropy: h264enc_core::Entropy::Cabac, ..base(26) }, &scene, true);
    let (a, b) = (cavlc.sizes.iter().sum::<usize>(), cabac.sizes.iter().sum::<usize>());
    println!("cabac gop30: cavlc={a} cabac={b} bytes ({:.1}%)", 100.0 * b as f64 / a as f64);
    assert!(b < a, "the CABAC stream ({b} bytes) is not smaller than the CAVLC one ({a})");
}

/// H264ENC8X8 (LEDGER SR322): (h) with `transform8x8` the GOP-30 scene (High, Intra_8x8 and the 8x8
/// transform chosen per macroblock) decodes to the reconstruction byte for byte under both entropy
/// coders, and is no larger than the same coder's stream without it (both printed); (i) the three
/// pinned constant-QP streams are unchanged with `transform8x8: false` named.
#[test]
fn h_transform8x8_gop30_decodes_and_is_not_larger() {
    for entropy in [h264enc_core::Entropy::Cavlc, h264enc_core::Entropy::Cabac] {
        let off = run(Config { entropy, ..base(26) }, &scene, true);
        let on = run(Config { entropy, transform8x8: true, ..base(26) }, &scene, true);
        let (a, b) = (off.sizes.iter().sum::<usize>(), on.sizes.iter().sum::<usize>());
        println!("8x8 gop30 {entropy:?}: off={a} on={b} bytes ({:.1}%)", 100.0 * b as f64 / a as f64);
        assert!(b <= a, "{entropy:?}: the 8x8 stream ({b} bytes) is larger than the 4x4 one ({a})");
    }
}

#[test]
fn i_transform8x8_off_keeps_the_pinned_bytes() {
    let off = |c: Config| Config { transform8x8: false, ..c };
    for (name, cfg, want) in [
        ("qp26 gop30", off(base(26)), 0x1290_fbb5_41d1_0774u64),
        ("qp30 gop10 deblock subpel2", off(Config { gop: 10, deblock: true, subpel: 2, ..base(30) }), 0x20e7_2796_bb58_a08b),
        ("qp36 gop1", off(Config { gop: 1, ..base(36) }), 0x7785_4898_5213_d0db),
    ] {
        let mut enc = Encoder::new(cfg).unwrap();
        let mut samples = Vec::new();
        for t in 0..N {
            let p = scene(t);
            samples.push(enc.encode(&Yuv420 { width: W as u32, height: H as u32, y: &p[0], u: &p[1], v: &p[2], y_stride: W, uv_stride: W / 2 }).unwrap().avcc_sample());
        }
        assert_eq!(fnv(&samples), want, "{name}: transform8x8 false moved the pinned bytes");
    }
}
