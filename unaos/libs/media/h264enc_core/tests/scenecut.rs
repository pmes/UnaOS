// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! H264ENCSCENECUT (LEDGER SR680): scene-cut IDRs (`Config::scenecut`). A flat scene A, a textured scene B and B lifted by
//! 16 levels (C) at 64x64, qp 26, gop 30: with Some(40) the hard cut A -> B is an IDR and the next scheduled IDR moves a GOP
//! past it; None keeps the GOP grid; the min_keyint guard (gop / 4) holds a B -> C step 3 frames after the cut as a P while
//! the same step far from an IDR is one; under `bframes` 1 the cut closes the open group (the held frame becomes a P anchor)
//! and is the IDR; every stream decodes through h264_core to the encoder's reconstruction; on tests/rc.rs's moving textured
//! scene Some(40) finds no cut and keeps the None stream's pinned bytes.

use h264_core::Decoder;
use h264enc_core::{Access, Config, Encoder, Yuv420};

const W: usize = 64;
const H: usize = 64;

fn flat(v: u8) -> [Vec<u8>; 3] {
    [vec![v; W * H], vec![128; W * H / 4], vec![128; W * H / 4]]
}

/// A fixed LCG texture, luma 60..=140 (+ `lift`).
fn texture(lift: u8) -> [Vec<u8>; 3] {
    let mut s = 0x2545_f491_4f6c_dd1du64;
    let mut p = flat(0);
    for v in p[0].iter_mut() {
        s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *v = (60 + (s >> 33) % 81) as u8 + lift;
    }
    p
}

fn yuv(w: usize, h: usize, p: &[Vec<u8>; 3]) -> Yuv420<'_> {
    Yuv420 { width: w as u32, height: h as u32, y: &p[0], u: &p[1], v: &p[2], y_stride: w, uv_stride: w / 2 }
}

fn cfg(scenecut: Option<u8>, bframes: u8) -> Config {
    Config { width: W as u32, height: H as u32, qp: 26, gop: 30, bframes, scenecut, ..Default::default() }
}

/// Push `n` frames and flush: the access units in decoding order and each one's reconstruction.
fn encode_all(cfg: Config, n: usize, src: &dyn Fn(usize) -> [Vec<u8>; 3]) -> (Vec<Access>, Vec<[Vec<u8>; 3]>) {
    let (w, h) = (cfg.width as usize, cfg.height as usize);
    let mut enc = Encoder::new(cfg).unwrap();
    let (mut aus, mut recons) = (Vec::new(), Vec::new());
    for t in 0..n {
        let p = src(t);
        aus.extend(enc.push(&yuv(w, h, &p)).unwrap());
        recons.extend(enc.reconstructions().iter().cloned());
    }
    aus.extend(enc.flush().unwrap());
    recons.extend(enc.reconstructions().iter().cloned());
    assert_eq!(aus.len(), n, "one access unit per frame");
    assert_eq!(recons.len(), n, "one reconstruction per access unit");
    (aus, recons)
}

/// h264_core decodes the units (decoding order); its output-order pictures equal the reconstructions sorted by pts.
fn decodes_to_recon(name: &str, aus: &[Access], recons: &[[Vec<u8>; 3]]) {
    let mut dec = Decoder::new();
    let mut frames = Vec::new();
    for (i, a) in aus.iter().enumerate() {
        dec.decode_annexb(&a.annexb()).unwrap_or_else(|e| panic!("{name}: AU {i}: {e:?}"));
        dec.end_access_unit();
        frames.extend(dec.take_frames());
    }
    dec.flush();
    frames.extend(dec.take_frames());
    assert_eq!(frames.len(), aus.len(), "{name}: decoded pictures");
    let mut order: Vec<usize> = (0..aus.len()).collect();
    order.sort_by_key(|&i| aus[i].pts);
    for (k, &i) in order.iter().enumerate() {
        for c in 0..3 {
            assert_eq!(frames[k].planes[c], recons[i][c], "{name}: output picture {k} plane {c}: decode differs from the reconstruction");
        }
    }
}

/// Display indices of the IDR access units (pts - `depth`), checked against the NAL unit type (5 IDR, else 1).
fn keys(aus: &[Access], depth: u64) -> Vec<u64> {
    let mut k: Vec<u64> = aus.iter().filter(|a| a.keyframe).map(|a| a.pts - depth).collect();
    for a in aus {
        assert_eq!(a.nals.last().unwrap()[0] & 0x1f, if a.keyframe { 5 } else { 1 }, "pts {}: NAL type", a.pts);
    }
    k.sort();
    k
}

#[test]
fn a_hard_cut_is_an_idr_and_moves_the_next_scheduled_one() {
    let src = |t: usize| if t < 10 { flat(50) } else { texture(0) };
    let (aus, recons) = encode_all(cfg(Some(40), 0), 45, &src);
    let got = keys(&aus, 0);
    println!("scenecut a: Some(40) keys {got:?}");
    assert_eq!(got, vec![0, 10, 40], "Some(40): IDR at the cut, then a GOP after it");
    decodes_to_recon("Some(40)", &aus, &recons);
    let (aus, recons) = encode_all(cfg(None, 0), 45, &src);
    assert_eq!(keys(&aus, 0), vec![0, 30], "None: the GOP grid");
    decodes_to_recon("None", &aus, &recons);
}

#[test]
fn b_min_keyint_holds_a_step_near_the_last_idr() {
    let near = |t: usize| if t < 10 { flat(50) } else if t < 13 { texture(0) } else { texture(16) };
    let (aus, recons) = encode_all(cfg(Some(40), 0), 45, &near);
    let got = keys(&aus, 0);
    println!("scenecut b: B -> C at 13 (3 after the cut) keys {got:?}");
    assert_eq!(got, vec![0, 10, 40], "the B -> C step within gop / 4 of the cut stays a P");
    decodes_to_recon("near", &aus, &recons);
    let far = |t: usize| if t < 20 { texture(0) } else { texture(16) };
    let (aus, recons) = encode_all(cfg(Some(40), 0), 30, &far);
    let got = keys(&aus, 0);
    println!("scenecut b: B -> C at 20 keys {got:?}");
    assert_eq!(got, vec![0, 20], "the same step beyond gop / 4 is a cut: the guard is what held frame 13");
    decodes_to_recon("far", &aus, &recons);
}

#[test]
fn c_bframes_cut_closes_the_open_group() {
    let src = |t: usize| if t < 12 { flat(50) } else { texture(0) };
    let (aus, recons) = encode_all(cfg(Some(40), 1), 45, &src);
    let got = keys(&aus, 1);
    println!("scenecut c: bframes 1 keys {got:?}");
    assert_eq!(got, vec![0, 12, 42], "bframes 1: the cut at 12 is the IDR, the next a GOP later");
    let held = aus.iter().position(|a| a.pts == 11 + 1).unwrap();
    let cut = aus.iter().position(|a| a.pts == 12 + 1).unwrap();
    let nal = aus[held].nals.last().unwrap()[0];
    assert_eq!((nal & 0x1f, nal >> 5 & 3), (1, 3), "the held frame 11 closes the group as a P anchor (nal_ref_idc 3), not a B");
    assert_eq!(cut, held + 1, "the IDR follows the group it closed in decoding order");
    decodes_to_recon("bframes 1", &aus, &recons);
}

/// tests/rc.rs's textured scene (a square moving right, a patterned block moving down) at 128x96.
fn scene(t: usize) -> [Vec<u8>; 3] {
    let (w, h) = (128usize, 96usize);
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

fn fnv(aus: &[Vec<u8>]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for b in aus.iter().flatten() {
        h = (h ^ *b as u64).wrapping_mul(0x100_0000_01b3);
    }
    h
}

#[test]
fn d_motion_is_not_a_cut_and_keeps_the_pinned_bytes() {
    let mut enc = Encoder::new(Config { width: 128, height: 96, qp: 26, gop: 30, scenecut: Some(40), ..Default::default() }).unwrap();
    let mut samples = Vec::new();
    for t in 0..60 {
        let p = scene(t);
        let au = enc.encode(&yuv(128, 96, &p)).unwrap();
        assert_eq!(au.keyframe, t % 30 == 0, "frame {t}: a moving scene is not a cut");
        samples.push(au.avcc_sample());
    }
    assert_eq!(fnv(&samples), 0x1fbc_56dc_6807_f2a8, "Some(40) on the moving scene: the None pin (tests/rc.rs qp26 gop30)");
}

// ---- H264ENCSCENECUTFADE (LEDGER SR709): the gradual rule (named tail block) ----
// A picture is a cut only if its test passes AND the previous picture's un-guarded threshold test did not: a run of changing
// pictures (a fade, a dissolve, a brightness ramp) is ONE IDR at its first firing picture. The fixture is scene B's texture scaled
// (a smooth gradient's intra cost is ~0, so any brightness step would fire the threshold and the slow-ramp case would say nothing).

/// `texture(0)` with its luma scaled by `pct` / 100.
fn scaled(pct: u32) -> [Vec<u8>; 3] {
    let mut p = texture(0);
    for v in p[0].iter_mut() {
        *v = (*v as u32 * pct / 100) as u8;
    }
    p
}

#[test]
fn e_a_fade_is_one_idr() {
    let fade = |t: usize| if t < 10 { scaled(100) } else if t < 22 { scaled(100 - (t as u32 - 9) * 100 / 12) } else { scaled(0) };
    let (aus, recons) = encode_all(cfg(Some(40), 0), 50, &fade);
    let got = keys(&aus, 0);
    let inside: Vec<u64> = got.iter().copied().filter(|d| (10..22).contains(d)).collect();
    println!("scenecut e: fade 10..=21 to black keys {got:?} ({} inside the fade)", inside.len());
    assert_eq!(inside.len(), 1, "a fade is ONE IDR at its first firing picture, not one per frame");
    let c = inside[0];
    assert_eq!(got, vec![0, c, c + 30], "the next scheduled IDR a GOP after the fade's");
    decodes_to_recon("fade", &aus, &recons);
}

#[test]
fn f_two_hard_cuts_three_apart_are_two_idrs() {
    let src = |t: usize| if t < 10 { flat(50) } else if t < 13 { texture(0) } else { flat(200) };
    let (aus, recons) = encode_all(cfg(Some(40), 0), 45, &src);
    let got = keys(&aus, 0);
    println!("scenecut f: cuts at 10 and 13 keys {got:?}");
    assert_eq!(got, vec![0, 10, 13, 43], "a second hard cut after a static gap is a cut");
    decodes_to_recon("two cuts", &aus, &recons);
}

#[test]
fn g_slow_ramp_is_not_a_cut() {
    let ramp = |t: usize| scaled(100 - 2 * t as u32);
    let (aus, recons) = encode_all(cfg(Some(40), 0), 30, &ramp);
    let got = keys(&aus, 0);
    println!("scenecut g: 2 percent per frame keys {got:?}");
    assert_eq!(got, vec![0], "a slow ramp below the threshold is no cut");
    decodes_to_recon("ramp", &aus, &recons);
}
