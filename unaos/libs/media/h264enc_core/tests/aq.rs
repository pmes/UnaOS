// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! H264ENCAQ (LEDGER SR693): adaptive quantisation per macroblock (`Config::aq`, x264's aq-mode 1). A 128x96 scene, its left
//! half a smooth gradient (a flat sky) and its right half a moving LCG texture: (a) `aq_offsets` is negative on the flat
//! macroblocks, positive on the textured ones, within +-6, mean zero within 1, and all zero at strength 0; (b) under Some(10) at
//! constant QP 26 (CAVLC and CABAC, deblocking on) h264_core decodes every frame to the encoder's reconstruction byte for byte
//! and every macroblock's QPY is 26 + its offset when it carries mb_qp_delta, else its predecessor's (QPY,pred), with offset
//! macroblocks seen at 26 + offset; the same stream under CBR decodes to its reconstruction; (c) Some(0) is the None stream's
//! bytes (constant QP and CBR); (d) the flat half's PSNR of the decoded IDR, None against Some(10), is printed (not asserted).
//! The decoder's own per-MB QP has no accessor (H264CORE-MBQP owed): `Encoder::mb_qps` is the encoder's record of it by the same
//! QPY,pred rule, and the byte-exact decode shows the decoder dequantised every coded macroblock at that QP.
//! H264ENCAQVARIANCEMODE (LEDGER SR1072, `Config::aq_mode`, x264's aq-mode 2 / 3 as recalled, pinned as implemented): (e) aq_mode
//! 0 and 1 are the same offsets and the same Some(10) stream bytes; (f) mode 2 on the scene: flat negative, textured positive, within
//! +-6, mean zero within 1, strength 0 all zero; (g) on a low-energy frame mode 2 spreads less than mode 1; (h) mode 3 on a half
//! flat-dark frame lowers the dark macroblocks below mode 2; (i) `root8_q8` within 1% of (e + 1)^(1/8) x 256 over 0..2^20; (j) modes
//! 2 and 3 (CAVLC, CABAC; CBR for mode 2) decode to the reconstruction with every MB QPY its row plus offset or QPY,pred.

use h264_core::Decoder;
use h264enc_core::{aq_offsets, root8_q8, Config, Encoder, Entropy, RateControl, Yuv420};

const W: usize = 128;
const H: usize = 96;
const N: usize = 12;

/// Left half: a smooth gradient; right half: an LCG texture (luma 60..=140) moving 2 samples per frame left.
fn scene(t: usize) -> [Vec<u8>; 3] {
    let mut p = [vec![0u8; W * H], vec![128u8; W * H / 4], vec![128u8; W * H / 4]];
    for y in 0..H {
        for x in 0..W {
            p[0][y * W + x] = if x < W / 2 {
                (90 + y / 8 + x / 16) as u8
            } else {
                let i = (y * 977 + x + 2 * t) as u64;
                let s = i.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                (60 + (s >> 33) % 81) as u8
            };
        }
    }
    p
}

fn cfg(aq: Option<u8>, entropy: Entropy) -> Config {
    Config { width: W as u32, height: H as u32, qp: 26, gop: 8, deblock: true, entropy, aq, ..Default::default() }
}

fn fnv(aus: &[Vec<u8>]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in aus.iter().flatten() {
        h = (h ^ *b as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// Encode N frames of the scene; h264_core decodes each and it must equal the reconstruction byte for byte. `each` sees the
/// encoder after every frame. Returns the samples and the decoded frames' luma.
fn run(cfg: Config, each: &mut dyn FnMut(usize, &Encoder)) -> (Vec<Vec<u8>>, Vec<Vec<u8>>) {
    let mut enc = Encoder::new(cfg).unwrap();
    let mut dec = Decoder::new();
    let (mut samples, mut lumas) = (Vec::new(), Vec::new());
    for t in 0..N {
        let p = scene(t);
        let au = enc.encode(&Yuv420 { width: W as u32, height: H as u32, y: &p[0], u: &p[1], v: &p[2], y_stride: W, uv_stride: W / 2 }).unwrap();
        dec.decode_annexb(&au.annexb()).unwrap();
        dec.end_access_unit();
        let mut f = dec.take_frames();
        assert_eq!(f.len(), 1);
        let f = f.pop().unwrap();
        let rec = enc.reconstruction();
        for c in 0..3 {
            assert_eq!(f.planes[c].as_slice(), rec[c], "{:?} frame {t} plane {c}: decode differs from the reconstruction", cfg.aq);
        }
        each(t, &enc);
        samples.push(au.avcc_sample());
        lumas.push(f.planes[0].clone());
    }
    (samples, lumas)
}

#[test]
fn aq_a_offsets_follow_the_energy() {
    let p = scene(0);
    let (mb_w, mb_h) = (W / 16, H / 16);
    let off = aq_offsets(&p[0], W, mb_w, mb_h, 10, 1);
    assert_eq!(off.len(), mb_w * mb_h);
    println!("aq offsets (strength 10): {off:?}");
    for (i, &o) in off.iter().enumerate() {
        assert!((-6..=6).contains(&o), "MB {i}: offset {o} outside +-6");
        if i % mb_w < mb_w / 2 {
            assert!(o < 0, "flat MB {i}: offset {o} is not negative");
        } else {
            assert!(o > 0, "textured MB {i}: offset {o} is not positive");
        }
    }
    let sum: i32 = off.iter().map(|&o| o as i32).sum();
    assert!(sum.unsigned_abs() as usize <= off.len(), "mean offset {sum}/{} not within 1 of zero", off.len());
    assert!(aq_offsets(&p[0], W, mb_w, mb_h, 0, 1).iter().all(|&o| o == 0), "strength 0 moved an offset");
}

#[test]
fn aq_b_decodes_and_each_mb_is_its_row_plus_offset() {
    for entropy in [Entropy::Cavlc, Entropy::Cabac] {
        let mut hits = 0usize;
        run(cfg(Some(10), entropy), &mut |t: usize, enc: &Encoder| {
            let (off, qps) = (enc.aq_picture(), enc.mb_qps());
            assert_eq!(off.len(), qps.len(), "{entropy:?} frame {t}: one offset per MB");
            assert!(off.iter().any(|&o| o != 0), "{entropy:?} frame {t}: no MB offset");
            // Constant QP: the row QP is SliceQPY 26 on every row; QPY,pred starts at SliceQPY.
            let mut pred = 26i32;
            for (i, (&o, &q)) in off.iter().zip(qps.iter()).enumerate() {
                let want = (26 + o as i32).clamp(16, 51);
                assert!(q as i32 == want || q as i32 == pred, "{entropy:?} frame {t} MB {i}: QPY {q} is neither 26 + {o} nor QPY,pred {pred}");
                if o != 0 && q as i32 == want {
                    hits += 1;
                }
                pred = q as i32;
            }
        });
        println!("aq {entropy:?}: {hits} offset MBs coded at 26 + offset over {N} frames");
        assert!(hits > 0, "{entropy:?}: no MB was coded at its offset QP");
    }
    let rate = Some(RateControl::cbr(60, (30, 1)));
    run(Config { rate, ..cfg(Some(10), Entropy::Cavlc) }, &mut |_: usize, _: &Encoder| {});
}

#[test]
fn aq_c_strength_zero_is_the_none_stream() {
    let rate = Some(RateControl::cbr(60, (30, 1)));
    for (name, base) in [("qp26", cfg(None, Entropy::Cavlc)), ("cbr60", Config { rate, ..cfg(None, Entropy::Cavlc) }), ("qp26 cabac", cfg(None, Entropy::Cabac))] {
        let (none, _) = run(base, &mut |_: usize, _: &Encoder| {});
        let (zero, _) = run(Config { aq: Some(0), ..base }, &mut |_: usize, _: &Encoder| {});
        assert_eq!(fnv(&zero), fnv(&none), "{name}: aq Some(0) moved the None stream's bytes");
    }
}

#[test]
fn aq_d_flat_psnr_printed() {
    let src = scene(0);
    let psnr = |l: &[u8]| {
        let (mut se, mut n) = (0u64, 0u64);
        for y in 0..H {
            for x in 0..W / 2 {
                let d = l[y * W + x] as i64 - src[0][y * W + x] as i64;
                se += (d * d) as u64;
                n += 1;
            }
        }
        if se == 0 { f64::INFINITY } else { 10.0 * (255.0f64 * 255.0 * n as f64 / se as f64).log10() }
    };
    let (none_s, none_l) = run(cfg(None, Entropy::Cavlc), &mut |_: usize, _: &Encoder| {});
    let (aq_s, aq_l) = run(cfg(Some(10), Entropy::Cavlc), &mut |_: usize, _: &Encoder| {});
    let bytes = |s: &[Vec<u8>]| s.iter().map(Vec::len).sum::<usize>();
    println!("aq: flat psnr none={:.2} aq={:.2} (IDR, left half); bytes none={} aq={}", psnr(&none_l[0]), psnr(&aq_l[0]), bytes(&none_s), bytes(&aq_s));
}

/// One luma plane: left half `left(x, y)`, right half an LCG texture `lo..lo + span`.
fn half(left: u8, lo: u64, span: u64) -> Vec<u8> {
    let mut y = vec![left; W * H];
    for r in 0..H {
        for x in W / 2..W {
            let s = ((r * 977 + x) as u64).wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            y[r * W + x] = (lo + (s >> 33) % span) as u8;
        }
    }
    y
}

#[test]
fn aq_e_mode_one_is_mode_zero() {
    let p = scene(0);
    let (mb_w, mb_h) = (W / 16, H / 16);
    assert_eq!(aq_offsets(&p[0], W, mb_w, mb_h, 10, 0), aq_offsets(&p[0], W, mb_w, mb_h, 10, 1), "aq_mode 1 moved mode 0's offsets");
    for entropy in [Entropy::Cavlc, Entropy::Cabac] {
        let (zero, _) = run(cfg(Some(10), entropy), &mut |_: usize, _: &Encoder| {});
        let (one, _) = run(Config { aq_mode: 1, ..cfg(Some(10), entropy) }, &mut |_: usize, _: &Encoder| {});
        assert_eq!(fnv(&one), fnv(&zero), "{entropy:?}: aq_mode 1 moved the aq_mode 0 stream's bytes");
    }
}

#[test]
fn aq_f_mode_two_follows_the_energy() {
    let p = scene(0);
    let (mb_w, mb_h) = (W / 16, H / 16);
    let off = aq_offsets(&p[0], W, mb_w, mb_h, 10, 2);
    println!("aq mode2 offsets (strength 10): {off:?}");
    for (i, &o) in off.iter().enumerate() {
        assert!((-6..=6).contains(&o), "MB {i}: offset {o} outside +-6");
        if i % mb_w < mb_w / 2 {
            assert!(o < 0, "flat MB {i}: mode 2 offset {o} is not negative");
        } else {
            assert!(o > 0, "textured MB {i}: mode 2 offset {o} is not positive");
        }
    }
    let sum: i32 = off.iter().map(|&o| o as i32).sum();
    assert!(sum.unsigned_abs() as usize <= off.len(), "mode 2 mean offset {sum}/{} not within 1 of zero", off.len());
    assert!(aq_offsets(&p[0], W, mb_w, mb_h, 0, 2).iter().all(|&o| o == 0), "mode 2 strength 0 moved an offset");
}

#[test]
fn aq_g_mode_two_adapts_less_on_a_low_energy_frame() {
    let y = half(100, 100, 4);
    let (mb_w, mb_h) = (W / 16, H / 16);
    let spread = |o: &[i8]| *o.iter().max().unwrap() as i32 - *o.iter().min().unwrap() as i32;
    let (m1, m2) = (aq_offsets(&y, W, mb_w, mb_h, 10, 1), aq_offsets(&y, W, mb_w, mb_h, 10, 2));
    println!("aq mode2 spread {} < mode1 spread {} (low-energy frame)", spread(&m2), spread(&m1));
    assert!(spread(&m2) < spread(&m1), "mode 2 spread {} not below mode 1's {}: {m2:?} vs {m1:?}", spread(&m2), spread(&m1));
}

#[test]
fn aq_h_mode_three_lowers_the_dark_macroblocks() {
    let y = half(16, 60, 81);
    let (mb_w, mb_h) = (W / 16, H / 16);
    let (m2, m3) = (aq_offsets(&y, W, mb_w, mb_h, 10, 2), aq_offsets(&y, W, mb_w, mb_h, 10, 3));
    println!("aq mode3 offsets {m3:?} vs mode2 {m2:?}");
    let (mut s2, mut s3) = (0i32, 0i32);
    for i in 0..m2.len() {
        assert!((-6..=6).contains(&m3[i]), "MB {i}: mode 3 offset {} outside +-6", m3[i]);
        if i % mb_w < mb_w / 2 {
            assert!(m3[i] <= m2[i], "dark MB {i}: mode 3 offset {} above mode 2's {}", m3[i], m2[i]);
            s2 += m2[i] as i32;
            s3 += m3[i] as i32;
        }
    }
    assert!(s3 < s2, "mode 3 did not lower the dark half: {s3} vs {s2}");
}

#[test]
fn aq_i_root8_within_one_percent() {
    let mut es: Vec<u32> = (0..4096).collect();
    es.extend((4096..1u32 << 20).step_by(997));
    es.extend((12..=20).map(|k| (1u32 << k) - 1));
    let mut worst = 0f64;
    for e in es {
        let want = ((e as f64) + 1.0).powf(0.125) * 256.0;
        let got = root8_q8(e) as f64;
        let rel = (got - want).abs() / want;
        assert!(rel <= 0.01, "root8_q8({e}) = {got}, want {want:.3}");
        worst = worst.max(rel);
    }
    println!("aq root8 max rel err {worst:.5}");
}

#[test]
fn aq_j_variance_modes_decode_with_each_mb_its_row_plus_offset() {
    for mode in [2u8, 3] {
        for entropy in [Entropy::Cavlc, Entropy::Cabac] {
            run(Config { aq_mode: mode, ..cfg(Some(10), entropy) }, &mut |t: usize, enc: &Encoder| {
                let (off, qps) = (enc.aq_picture(), enc.mb_qps());
                assert_eq!(off.len(), qps.len(), "mode {mode} {entropy:?} frame {t}: one offset per MB");
                let mut pred = 26i32;
                for (i, (&o, &q)) in off.iter().zip(qps.iter()).enumerate() {
                    assert!((16..=51).contains(&q), "mode {mode} {entropy:?} frame {t} MB {i}: QPY {q} out of range");
                    let want = (26 + o as i32).clamp(16, 51);
                    assert!(q as i32 == want || q as i32 == pred, "mode {mode} {entropy:?} frame {t} MB {i}: QPY {q} is neither 26 + {o} nor {pred}");
                    pred = q as i32;
                }
            });
        }
    }
    let rate = Some(RateControl::cbr(60, (30, 1)));
    run(Config { rate, aq_mode: 2, ..cfg(Some(10), Entropy::Cavlc) }, &mut |_: usize, _: &Encoder| {});
}
