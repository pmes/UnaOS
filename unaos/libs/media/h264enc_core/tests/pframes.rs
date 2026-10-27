// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! H264ENC M2 (LEDGER SR197): P frames. Ten frames of a moving test pattern (a bright square
//! moving 3 samples per frame across a gradient) through GOPs of 10 and 4: every frame h264_core
//! decodes equals the encoder's reconstruction of it, within the quantiser's bound of the source;
//! P access units are smaller than the IDR; a static sequence's P frames are P_Skip runs.

use h264_core::{Decoder, Frame};
use h264enc_core::{Access, Config, Encoder, Yuv420};

const W: usize = 64;
const H: usize = 48;

/// Frame `t` of the moving pattern: the gradient background and a 16x16 square at x = 6 + 3t.
fn frame(t: usize) -> [Vec<u8>; 3] {
    let mut p = [vec![0u8; W * H], vec![0u8; W * H / 4], vec![0u8; W * H / 4]];
    let (sx, sy) = (6 + 3 * t, 14);
    for y in 0..H {
        for x in 0..W {
            let inside = x >= sx && x < sx + 16 && y >= sy && y < sy + 16;
            p[0][y * W + x] = if inside { 220 } else { (30 + x * 120 / W + y * 40 / H) as u8 };
        }
    }
    for y in 0..H / 2 {
        for x in 0..W / 2 {
            let inside = 2 * x >= sx && 2 * x < sx + 16 && 2 * y >= sy && 2 * y < sy + 16;
            p[1][y * (W / 2) + x] = if inside { 90 } else { (110 + x) as u8 };
            p[2][y * (W / 2) + x] = if inside { 200 } else { (140 - y) as u8 };
        }
    }
    p
}

fn yuv(p: &[Vec<u8>; 3]) -> Yuv420<'_> {
    Yuv420 { width: W as u32, height: H as u32, y: &p[0], u: &p[1], v: &p[2], y_stride: W, uv_stride: W / 2 }
}

fn psnr(a: &[u8], b: &[u8]) -> f64 {
    let se: f64 = a.iter().zip(b).map(|(&x, &y)| ((x as f64) - (y as f64)).powi(2)).sum();
    if se == 0.0 {
        return f64::INFINITY;
    }
    10.0 * (255.0f64 * 255.0 / (se / a.len() as f64)).log10()
}

fn decode_one(dec: &mut Decoder, au: &Access) -> Frame {
    dec.decode_annexb(&au.annexb()).unwrap();
    dec.end_access_unit();
    let mut f = dec.take_frames();
    assert_eq!(f.len(), 1, "one frame per access unit (VUI max_num_reorder_frames 0)");
    f.pop().unwrap()
}

/// Encode `n` frames of `src(t)` with `gop` and decode them back; returns the access units.
fn round_trip(gop: u32, n: usize, src: impl Fn(usize) -> [Vec<u8>; 3]) -> Vec<Access> {
    let mut enc = Encoder::new(Config { width: W as u32, height: H as u32, qp: 26, gop }).unwrap();
    let mut dec = Decoder::new();
    let mut aus = Vec::new();
    for t in 0..n {
        let p = src(t);
        let au = enc.encode(&yuv(&p)).unwrap();
        let f = decode_one(&mut dec, &au);
        let rec = enc.reconstruction();
        for c in 0..3 {
            assert_eq!(f.planes[c].as_slice(), rec[c], "gop {gop} frame {t} plane {c}: decode differs from the reconstruction");
            let db = psnr(&f.planes[c], &p[c]);
            assert!(db >= 35.0, "gop {gop} frame {t} plane {c}: {db:.2} dB < 35");
        }
        println!("gop {gop} frame {t}: {} {} bytes, Y {:.2} dB", if au.keyframe { "IDR" } else { "P  " }, au.annexb().len(), psnr(&f.planes[0], &p[0]));
        aus.push(au);
    }
    aus
}

#[test]
fn m2_moving_pattern_gop10_every_frame_decodes_to_the_reconstruction() {
    let aus = round_trip(10, 10, frame);
    for (t, au) in aus.iter().enumerate() {
        assert_eq!(au.keyframe, t == 0, "frame {t}");
        assert_eq!(au.nals.len(), if t == 0 { 3 } else { 1 });
    }
    let idr = aus[0].annexb().len();
    for (t, au) in aus.iter().enumerate().skip(1) {
        assert!(au.annexb().len() < idr, "frame {t}: P {} bytes not below the IDR's {idr}", au.annexb().len());
    }
}

#[test]
fn m2_moving_pattern_gop4_idrs_at_0_4_8() {
    let aus = round_trip(4, 10, frame);
    let keys: Vec<usize> = aus.iter().enumerate().filter(|(_, a)| a.keyframe).map(|(t, _)| t).collect();
    assert_eq!(keys, vec![0, 4, 8]);
    for (t, au) in aus.iter().enumerate() {
        let nal_type = au.nals.last().unwrap()[0] & 0x1f;
        assert_eq!(nal_type, if t % 4 == 0 { 5 } else { 1 }, "frame {t}");
    }
}

#[test]
fn m2_static_sequence_p_frames_are_skip_runs() {
    let aus = round_trip(10, 5, |_| frame(0));
    let idr = aus[0].annexb().len();
    for (t, au) in aus.iter().enumerate().skip(1) {
        let n = au.annexb().len();
        // start code + header + slice header + one mb_skip_run of 12 + trailing bits.
        assert!(n <= 12, "static frame {t}: {n} bytes (IDR {idr})");
    }
}

/// A textured scene panning by (2, 1) samples per frame: only motion vectors keep its P frames
/// small (P_Skip with zero motion would leave the whole texture in the residual).
fn pan(t: usize) -> [Vec<u8>; 3] {
    let tex = |x: usize, y: usize| -> u8 {
        let h = (x as u32).wrapping_mul(0x9e37_79b1) ^ (y as u32).wrapping_mul(0x85eb_ca77);
        let edge = if (x / 6 + y / 4) % 2 == 0 { 180 } else { 60 };
        (edge + (h >> 28) as i32) as u8
    };
    let mut p = [vec![0u8; W * H], vec![0u8; W * H / 4], vec![0u8; W * H / 4]];
    for y in 0..H {
        for x in 0..W {
            p[0][y * W + x] = tex(x + 2 * t, y + t);
        }
    }
    for y in 0..H / 2 {
        for x in 0..W / 2 {
            p[1][y * (W / 2) + x] = 100 + ((2 * x + 2 * t) / 8 % 4) as u8 * 10;
            p[2][y * (W / 2) + x] = 150 - ((2 * y + t) / 8 % 4) as u8 * 10;
        }
    }
    p
}

#[test]
fn m2_panning_scene_is_followed_by_the_motion_search() {
    let aus = round_trip(10, 6, pan);
    let idr = aus[0].annexb().len();
    for (t, au) in aus.iter().enumerate().skip(1) {
        let n = au.annexb().len();
        assert!(n * 3 < idr, "pan frame {t}: P {n} bytes not under a third of the IDR's {idr}");
    }
}
