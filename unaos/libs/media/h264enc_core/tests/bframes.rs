// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! H264ENCBFRAMES (LEDGER SR333): B pictures. Three scenes (the moving square at 64x48, the textured
//! scene at 128x96, a 50x34 crop) with `bframes` 1 and 2 under CAVLC and CABAC, deblocking off and on,
//! plus an 8x8-transform and a rate-controlled stream: h264_core decodes the access units in decoding
//! order and outputs every picture IN OUTPUT ORDER equal to the encoder's reconstruction of it; dts
//! counts the decoding order, pts the display order + 1, dts <= pts; the SPS's VUI reorder depth is 1
//! (0 without B); `bframes` 0 through `push` keeps the pinned bytes; the GOP-30 scene is smaller with B.
//! H264ENCPYRAMID (SR349): bframes 3 / 4 (the pyramid: a reference B, VUI reorder 2, pts = display + 2), temporal
//! direct, implicit and explicit weighted bi-prediction each decode to the reconstruction in output order.

use h264_core::ps::avcc_reorder_depth;
use h264_core::Decoder;
use h264enc_core::{Access, BiPred, Config, Encoder, Entropy, Error, RateControl, Yuv420};

/// The moving square over a gradient (tests/pframes.rs's pattern) at any size.
fn pattern(w: usize, h: usize, t: usize) -> [Vec<u8>; 3] {
    let mut p = [vec![0u8; w * h], vec![0u8; w * h / 4], vec![0u8; w * h / 4]];
    let (sx, sy) = ((6 + 3 * t) % (w - 16), 14 % (h - 16));
    for y in 0..h {
        for x in 0..w {
            let inside = x >= sx && x < sx + 16 && y >= sy && y < sy + 16;
            p[0][y * w + x] = if inside { 220 } else { (30 + x * 120 / w + y * 40 / h) as u8 };
        }
    }
    for y in 0..h / 2 {
        for x in 0..w / 2 {
            let inside = 2 * x >= sx && 2 * x < sx + 16 && 2 * y >= sy && 2 * y < sy + 16;
            p[1][y * (w / 2) + x] = if inside { 90 } else { (110 + x) as u8 };
            p[2][y * (w / 2) + x] = if inside { 200 } else { (140 - y) as u8 };
        }
    }
    p
}

/// The textured scene of tests/rc.rs (a square moving right, a patterned block moving down) at any size.
fn scene(w: usize, h: usize, t: usize) -> [Vec<u8>; 3] {
    let mut p = [vec![0u8; w * h], vec![0u8; w * h / 4], vec![0u8; w * h / 4]];
    let (ax, ay) = ((3 * t) % (w - 16), 20 % (h - 16));
    let (bx, by) = (w * 5 / 8, (2 * t) % (h - 24));
    for y in 0..h {
        for x in 0..w {
            let tex = ((x * 7 + y * 13) % 23) as i32 - 11;
            let bg = (40 + x * 100 / w + y * 60 / h) as i32 + tex;
            let a = x >= ax && x < ax + 16 && y >= ay && y < ay + 16;
            let b = x >= bx && x < bx + 24 && y >= by && y < by + 24;
            p[0][y * w + x] = if a { 220 } else if b { (60 + ((x + y) % 8) * 12) as u8 } else { bg.clamp(0, 255) as u8 };
        }
    }
    for y in 0..h / 2 {
        for x in 0..w / 2 {
            let a = 2 * x >= ax && 2 * x < ax + 16 && 2 * y >= ay && 2 * y < ay + 16;
            p[1][y * (w / 2) + x] = if a { 90 } else { (100 + x / 2) as u8 };
            p[2][y * (w / 2) + x] = if a { 200 } else { (150 - y / 2) as u8 };
        }
    }
    p
}

fn yuv(w: usize, h: usize, p: &[Vec<u8>; 3]) -> Yuv420<'_> {
    Yuv420 { width: w as u32, height: h as u32, y: &p[0], u: &p[1], v: &p[2], y_stride: w, uv_stride: w / 2 }
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let se: f64 = a.iter().zip(b).map(|(&x, &y)| ((x as f64) - (y as f64)).powi(2)).sum();
    if se == 0.0 {
        return f64::INFINITY;
    }
    10.0 * (255.0f64 * 255.0 / (se / a.len() as f64)).log10()
}

/// Push `n` frames of `src` and flush: the access units in decoding order and each one's reconstruction.
fn encode_all(cfg: Config, n: usize, src: &dyn Fn(usize) -> [Vec<u8>; 3]) -> (Vec<Access>, Vec<[Vec<u8>; 3]>, Encoder) {
    let (w, h) = (cfg.width as usize, cfg.height as usize);
    let mut enc = Encoder::new(cfg).unwrap();
    let (mut aus, mut recons) = (Vec::new(), Vec::new());
    for t in 0..n {
        let p = src(t);
        let out = enc.push(&yuv(w, h, &p)).unwrap();
        assert_eq!(out.len(), enc.reconstructions().len());
        recons.extend(enc.reconstructions().iter().cloned());
        aus.extend(out);
    }
    let out = enc.flush().unwrap();
    recons.extend(enc.reconstructions().iter().cloned());
    aus.extend(out);
    (aus, recons, enc)
}

/// The VUI reorder depth (and pts delay) of a `bframes` stream: 2 under a pyramid of 3 or 4 Bs, else 1 (SR349).
fn depth(cfg: &Config) -> u64 {
    if cfg.bframes >= 3 { 2 } else { 1 }
}

/// Encode under `cfg` (bframes > 0) and decode with h264_core: the timing rules hold, and the decoder's
/// output-order pictures equal the reconstructions sorted by pts, each within `floor` dB of its source.
/// Returns the stream's bytes (every NAL unit).
fn check(name: &str, cfg: Config, n: usize, src: &dyn Fn(usize) -> [Vec<u8>; 3], floor: f64) -> usize {
    let (aus, recons, enc) = encode_all(cfg, n, src);
    assert_eq!(aus.len(), n, "{name}: one access unit per frame");
    let dp = depth(&cfg);
    assert_eq!(avcc_reorder_depth(&enc.avcc()), Some(dp as u32), "{name}: VUI max_num_reorder_frames");
    for (i, a) in aus.iter().enumerate() {
        assert_eq!(a.dts, i as u64, "{name}: dts is the decoding index");
        assert!(a.dts <= a.pts, "{name}: AU {i} dts {} > pts {}", a.dts, a.pts);
        assert_eq!(a.keyframe, (a.pts - dp) % cfg.gop as u64 == 0, "{name}: AU {i} (pts {}) sync flag", a.pts);
    }
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by_key(|&i| aus[i].pts);
    assert_eq!(order.iter().map(|&i| aus[i].pts).collect::<Vec<_>>(), (dp..n as u64 + dp).collect::<Vec<_>>(), "{name}: pts = display + depth");
    assert!(aus.iter().any(|a| a.pts != a.dts + dp), "{name}: nothing was reordered");

    let mut dec = Decoder::new();
    let mut frames = Vec::new();
    for (i, a) in aus.iter().enumerate() {
        dec.decode_annexb(&a.annexb()).unwrap_or_else(|e| panic!("{name}: AU {i}: {e:?}"));
        dec.end_access_unit();
        frames.extend(dec.take_frames());
    }
    dec.flush();
    frames.extend(dec.take_frames());
    assert_eq!(frames.len(), n, "{name}: decoded pictures");
    for (k, &i) in order.iter().enumerate() {
        let p = src(k);
        for c in 0..3 {
            assert_eq!(frames[k].planes[c], recons[i][c], "{name}: output picture {k} plane {c}: decode differs from the reconstruction");
            let db = psnr(&frames[k].planes[c], &p[c]);
            assert!(db >= floor, "{name}: picture {k} plane {c}: {db:.2} dB < {floor}");
        }
    }
    aus.iter().map(|a| a.nals.iter().map(|x| x.len()).sum::<usize>()).sum()
}

#[test]
fn a_every_scene_with_b_decodes_to_the_reconstruction_in_output_order() {
    let scenes: [(&str, usize, usize, fn(usize, usize, usize) -> [Vec<u8>; 3]); 3] =
        [("pattern 64x48", 64, 48, pattern), ("scene 128x96", 128, 96, scene), ("crop 50x34", 50, 34, scene)];
    for (name, w, h, f) in scenes {
        for bframes in [1u8, 2, 3, 4] {
            for entropy in [Entropy::Cavlc, Entropy::Cabac] {
                for deblock in [false, true] {
                    let cfg = Config { width: w as u32, height: h as u32, qp: 26, gop: 10, subpel: 2, deblock, entropy, bframes, ..Default::default() };
                    let label = format!("{name} b{bframes} {entropy:?} deblock {deblock}");
                    let bytes = check(&label, cfg, 24, &|t| f(w, h, t), 30.0);
                    println!("bframes {label}: {bytes} bytes over 24 frames");
                }
            }
        }
    }
}

#[test]
fn a_transform8x8_and_rate_control_with_b_decode_to_the_reconstruction() {
    let (w, h) = (128usize, 96usize);
    let src = |t| scene(w, h, t);
    let base = Config { width: w as u32, height: h as u32, qp: 26, gop: 30, bframes: 2, deblock: true, subpel: 2, ..Default::default() };
    for entropy in [Entropy::Cavlc, Entropy::Cabac] {
        check(&format!("8x8 {entropy:?}"), Config { transform8x8: true, entropy, ..base }, 40, &src, 30.0);
        let rate = Some(RateControl::cbr(60, (30, 1)));
        let bytes = check(&format!("cbr60 {entropy:?}"), Config { rate, entropy, ..base }, 60, &src, 20.0);
        println!("bframes cbr60 {entropy:?}: {:.2} kbit/s", bytes as f64 * 8.0 * 30.0 / 60.0 / 1000.0);
    }
}

#[test]
fn b_vui_reorder_depth_and_encode_refused() {
    let base = Config { width: 64, height: 48, qp: 26, gop: 10, ..Default::default() };
    assert_eq!(avcc_reorder_depth(&Encoder::new(base).unwrap().avcc()), Some(0));
    let mut enc = Encoder::new(Config { bframes: 2, ..base }).unwrap();
    assert_eq!(avcc_reorder_depth(&enc.avcc()), Some(1));
    let p = pattern(64, 48, 0);
    assert_eq!(enc.encode(&yuv(64, 48, &p)), Err(Error::Reorder));
}

fn fnv(aus: &[Vec<u8>]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in aus.iter().flatten() {
        h = (h ^ *b as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

/// tests/rc.rs's three constant-QP pins, reproduced through `push` with `bframes` 0 named.
#[test]
fn c_bframes_zero_through_push_keeps_the_pinned_bytes() {
    let (w, h) = (128usize, 96usize);
    let base = |qp: u8| Config { width: w as u32, height: h as u32, qp, gop: 30, bframes: 0, ..Default::default() };
    for (name, cfg, want) in [
        ("qp26 gop30", base(26), 0x1290_fbb5_41d1_0774u64),
        ("qp30 gop10 deblock subpel2", Config { gop: 10, deblock: true, subpel: 2, ..base(30) }, 0x20e7_2796_bb58_a08b),
        ("qp36 gop1", Config { gop: 1, ..base(36) }, 0x7785_4898_5213_d0db),
    ] {
        let (aus, _, _) = encode_all(cfg, 60, &|t| scene(w, h, t));
        assert!(aus.iter().enumerate().all(|(i, a)| a.pts == i as u64 && a.dts == i as u64), "{name}: pts = dts = frame index");
        let samples: Vec<Vec<u8>> = aus.iter().map(Access::avcc_sample).collect();
        assert_eq!(fnv(&samples), want, "{name}: bframes 0 through push moved the pinned bytes");
    }
}

#[test]
fn d_gop30_scene_is_smaller_with_b() {
    let (w, h) = (128usize, 96usize);
    for entropy in [Entropy::Cavlc, Entropy::Cabac] {
        let cfg = |bframes: u8| Config { width: w as u32, height: h as u32, qp: 26, gop: 30, entropy, bframes, ..Default::default() };
        let (aus, _, _) = encode_all(cfg(0), 60, &|t| scene(w, h, t));
        let b0: usize = aus.iter().map(|a| a.nals.iter().map(|x| x.len()).sum::<usize>()).sum();
        let b1 = check(&format!("gop30 b1 {entropy:?}"), cfg(1), 60, &|t| scene(w, h, t), 30.0);
        let b2 = check(&format!("gop30 b2 {entropy:?}"), cfg(2), 60, &|t| scene(w, h, t), 30.0);
        let b3 = check(&format!("gop30 b3 {entropy:?}"), cfg(3), 60, &|t| scene(w, h, t), 30.0);
        let b4 = check(&format!("gop30 b4 {entropy:?}"), cfg(4), 60, &|t| scene(w, h, t), 30.0);
        let pc = |b: usize| 100.0 * b as f64 / b0 as f64;
        println!("bframes gop30 {entropy:?}: b0={b0} b1={b1} ({:.1}%) b2={b2} ({:.1}%) b3={b3} ({:.1}%) b4={b4} ({:.1}%) bytes (b2+ = the pyramid)", pc(b1), pc(b2), pc(b3), pc(b4));
        assert!(b1 < b0 && b2 < b0 && b3 < b0 && b4 < b0, "{entropy:?}: B pictures did not shrink the stream (b0 {b0} b1 {b1} b2 {b2} b3 {b3} b4 {b4})");
    }
}

/// The textured scene fading up: +3 luma per frame (the explicit weights' case, SR349).
fn fade(w: usize, h: usize, t: usize) -> [Vec<u8>; 3] {
    let mut p = scene(w, h, t);
    for v in p[0].iter_mut() {
        *v = (*v as usize + 3 * t).min(255) as u8;
    }
    p
}

/// The pyramid (SR349): bframes 3 writes VUI reorder 2 and codes a reference B (a non-IDR slice with nal_ref_idc 1)
/// per full group; bframes 2 keeps reorder 1 (its middle is the first B).
#[test]
fn e_pyramid_codes_a_reference_b_and_reorder_two() {
    let base = Config { width: 64, height: 48, qp: 26, gop: 10, ..Default::default() };
    assert_eq!(avcc_reorder_depth(&Encoder::new(Config { bframes: 2, ..base }).unwrap().avcc()), Some(1));
    for b in [3u8, 4] {
        let cfg = Config { bframes: b, ..base };
        assert_eq!(avcc_reorder_depth(&Encoder::new(cfg).unwrap().avcc()), Some(2), "b{b}: VUI reorder");
        let (aus, _, _) = encode_all(cfg, 20, &|t| pattern(64, 48, t));
        let refb = aus.iter().filter(|a| a.nals.iter().any(|n| n[0] & 0x1f == 1 && n[0] >> 5 == 1)).count();
        assert!(refb >= 2, "b{b}: {refb} reference B pictures");
    }
}

/// Temporal direct (SR349) on SR333's Bs (bframes 1) and the pyramid's middle (bframes 3): decodes bit-exact to the
/// reconstruction, and the stream differs from spatial direct's.
#[test]
fn f_temporal_direct_decodes_to_the_reconstruction() {
    let (w, h) = (128usize, 96usize);
    for bframes in [1u8, 3] {
        for entropy in [Entropy::Cavlc, Entropy::Cabac] {
            for deblock in [false, true] {
                let cfg = Config { width: w as u32, height: h as u32, qp: 26, gop: 15, subpel: 2, deblock, entropy, bframes, ..Default::default() };
                let label = format!("temporal b{bframes} {entropy:?} deblock {deblock}");
                let t = check(&label, Config { direct_temporal: true, ..cfg }, 30, &|t| scene(w, h, t), 30.0);
                let s = check(&format!("spatial b{bframes} {entropy:?} deblock {deblock}"), cfg, 30, &|t| scene(w, h, t), 30.0);
                println!("bframes {label}: {t} bytes (spatial {s})");
                assert_ne!(t, 0);
            }
        }
    }
    let cfg = Config { width: w as u32, height: h as u32, qp: 26, gop: 15, bframes: 1, ..Default::default() };
    let (a, _, _) = encode_all(Config { direct_temporal: true, ..cfg }, 30, &|t| scene(w, h, t));
    let (b, _, _) = encode_all(cfg, 30, &|t| scene(w, h, t));
    assert_ne!(a, b, "temporal direct wrote the spatial stream");
}

/// Weighted bi-prediction (SR349): implicit (Auto under the pyramid at bframes 4, where the middle sits at 2/5 and the
/// weights are not 32 / 32; Implicit at bframes 1) and explicit (the fading scene) decode bit-exact; the explicit stream
/// differs from the average's.
#[test]
fn g_weighted_bipred_decodes_to_the_reconstruction() {
    let (w, h) = (128usize, 96usize);
    for entropy in [Entropy::Cavlc, Entropy::Cabac] {
        let base = Config { width: w as u32, height: h as u32, qp: 26, gop: 15, subpel: 2, deblock: true, entropy, ..Default::default() };
        let auto = check(&format!("auto b4 {entropy:?}"), Config { bframes: 4, ..base }, 30, &|t| scene(w, h, t), 30.0);
        let avg = check(&format!("average b4 {entropy:?}"), Config { bframes: 4, bipred: BiPred::Average, ..base }, 30, &|t| scene(w, h, t), 30.0);
        let imp = check(&format!("implicit b1 {entropy:?}"), Config { bframes: 1, bipred: BiPred::Implicit, ..base }, 30, &|t| scene(w, h, t), 30.0);
        for b in [2u8, 3] {
            let exp = check(&format!("explicit fade b{b} {entropy:?}"), Config { bframes: b, bipred: BiPred::Explicit, ..base }, 30, &|t| fade(w, h, t), 28.0);
            let fav = check(&format!("average fade b{b} {entropy:?}"), Config { bframes: b, bipred: BiPred::Average, ..base }, 30, &|t| fade(w, h, t), 28.0);
            println!("bframes weighted {entropy:?} fade b{b}: explicit={exp} average={fav} bytes");
        }
        println!("bframes weighted {entropy:?}: b4 auto(implicit)={auto} average={avg}; b1 implicit={imp} bytes");
    }
    let cfg = Config { width: w as u32, height: h as u32, qp: 26, gop: 15, bframes: 2, ..Default::default() };
    let (a, _, _) = encode_all(Config { bipred: BiPred::Explicit, ..cfg }, 30, &|t| fade(w, h, t));
    let (b, _, _) = encode_all(Config { bipred: BiPred::Average, ..cfg }, 30, &|t| fade(w, h, t));
    assert_ne!(a, b, "explicit weights wrote the average's stream");
}