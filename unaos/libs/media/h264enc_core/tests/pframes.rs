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
    round_trip_cfg(Config { width: W as u32, height: H as u32, qp: 26, gop, ..Default::default() }, n, src, 35.0).0
}

/// Encode `n` frames of `src(t)` under `cfg` and decode them back: every decoded frame equals the
/// encoder's reconstruction byte for byte and sits at least `floor` dB from the source. Returns the
/// access units and the mean luma PSNR.
fn round_trip_cfg(cfg: Config, n: usize, src: impl Fn(usize) -> [Vec<u8>; 3], floor: f64) -> (Vec<Access>, f64) {
    let gop = cfg.gop;
    let mut enc = Encoder::new(cfg).unwrap();
    let mut dec = Decoder::new();
    let mut aus = Vec::new();
    let mut sum_db = 0.0;
    for t in 0..n {
        let p = src(t);
        let au = enc.encode(&yuv(&p)).unwrap();
        let f = decode_one(&mut dec, &au);
        let rec = enc.reconstruction();
        for c in 0..3 {
            assert_eq!(f.planes[c].as_slice(), rec[c], "{cfg:?} frame {t} plane {c}: decode differs from the reconstruction");
            let db = psnr(&f.planes[c], &p[c]);
            assert!(db >= floor, "{cfg:?} frame {t} plane {c}: {db:.2} dB < {floor}");
        }
        let y_db = psnr(&f.planes[0], &p[0]);
        sum_db += y_db;
        println!("gop {gop} frame {t}: {} {} bytes, Y {:.2} dB", if au.keyframe { "IDR" } else { "P  " }, au.annexb().len(), y_db);
        aus.push(au);
    }
    (aus, sum_db / n as f64)
}

/// The bytes of the P access units.
fn p_bytes(aus: &[Access]) -> usize {
    aus.iter().filter(|a| !a.keyframe).map(|a| a.annexb().len()).sum()
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

/// A smooth textured scene panning by (1.25, 0.5) samples per frame: a quarter-sample motion the
/// integer search can only approximate (H264ENC2, SR211).
fn pan_frac(t: usize) -> [Vec<u8>; 3] {
    let (ox, oy) = (1.25 * t as f64, 0.5 * t as f64);
    let tex = |x: f64, y: f64| -> u8 { (128.0 + 50.0 * (x * 0.45).sin() * (y * 0.3).cos() + 30.0 * ((x + 2.0 * y) * 0.21).sin()).round().clamp(0.0, 255.0) as u8 };
    let mut p = [vec![0u8; W * H], vec![0u8; W * H / 4], vec![0u8; W * H / 4]];
    for y in 0..H {
        for x in 0..W {
            p[0][y * W + x] = tex(x as f64 + ox, y as f64 + oy);
        }
    }
    for y in 0..H / 2 {
        for x in 0..W / 2 {
            let (cx, cy) = (2.0 * x as f64 + ox, 2.0 * y as f64 + oy);
            p[1][y * (W / 2) + x] = (120.0 + 20.0 * (cx * 0.1).sin()).round() as u8;
            p[2][y * (W / 2) + x] = (130.0 + 20.0 * (cy * 0.12).cos()).round() as u8;
        }
    }
    p
}

fn cfg(qp: u8, gop: u32, deblock: bool, subpel: u8) -> Config {
    Config { width: W as u32, height: H as u32, qp, gop, deblock, subpel, rate: None, ..Default::default() }
}

/// H264ENC2 M1 (LEDGER SR211): with the in-loop filter on (idc 0) every I and P frame h264_core
/// decodes — and deblocks — equals the encoder's filtered reconstruction byte for byte at QP 26
/// and 40, and on the moving pattern the filter does not lower the mean luma PSNR of the idc 1
/// run. The textured pan's PSNRs are printed only: a smoothing filter over its genuine hard
/// checker edges may lose a fraction of a dB (measured -0.08 dB at QP 26).
#[test]
fn m1_deblock_qp26_qp40_decode_is_recon() {
    for (name, scene) in [("moving", frame as fn(usize) -> [Vec<u8>; 3]), ("pan", pan)] {
        for qp in [26u8, 40] {
            let floor = if qp <= 26 { 33.0 } else { 20.0 };
            let (_, off) = round_trip_cfg(cfg(qp, 4, false, 0), 10, scene, floor);
            let (_, on) = round_trip_cfg(cfg(qp, 4, true, 0), 10, scene, floor);
            println!("deblock {name} qp {qp}: mean Y {off:.3} dB (idc 1) -> {on:.3} dB (idc 0)");
            if name == "moving" {
                assert!(on >= off, "{name} qp {qp}: deblocked mean Y {on:.3} dB below the unfiltered {off:.3} dB");
            }
        }
    }
}

/// H264ENC2 M2 (LEDGER SR211): the half / quarter-sample refinement decodes to the reconstruction
/// and shrinks the P frames of a scene moving by fractions of a sample; the integer pan is quoted.
#[test]
fn m2_subpel_quarter_shrinks_p_frames() {
    let (int_aus, _) = round_trip_cfg(cfg(26, 10, false, 0), 6, pan_frac, 35.0);
    let (half_aus, _) = round_trip_cfg(cfg(26, 10, false, 1), 6, pan_frac, 35.0);
    let (q_aus, _) = round_trip_cfg(cfg(26, 10, false, 2), 6, pan_frac, 35.0);
    let (i, h, q) = (p_bytes(&int_aus), p_bytes(&half_aus), p_bytes(&q_aus));
    println!("pan_frac P bytes: subpel 0 {i}, subpel 1 {h}, subpel 2 {q}");
    assert!(q < i, "fractional pan: subpel 2 P frames {q} bytes not below subpel 0's {i}");
    let (pi, _) = round_trip_cfg(cfg(26, 10, false, 0), 6, pan, 35.0);
    let (pq, _) = round_trip_cfg(cfg(26, 10, false, 2), 6, pan, 35.0);
    println!("integer pan P bytes: subpel 0 {}, subpel 2 {}", p_bytes(&pi), p_bytes(&pq));
}

/// H264ENC2 (LEDGER SR211): both tools together still decode to the reconstruction.
#[test]
fn m2_deblock_and_subpel_together() {
    round_trip_cfg(cfg(30, 4, true, 2), 8, pan_frac, 30.0);
    round_trip_cfg(cfg(30, 4, true, 2), 8, frame, 30.0);
}

/// H264ENCCABAC (LEDGER SR310): every scene above under CABAC (Main profile) decodes, frame by
/// frame, to the encoder's reconstruction byte for byte: the moving pattern (GOP 10 and 4), the
/// static sequence (P_Skip flags), the integer and fractional pans, the filter and the refinement.
#[test]
fn cabac_every_p_scene_decodes_to_the_reconstruction() {
    let cabac = |c: Config| Config { entropy: h264enc_core::Entropy::Cabac, ..c };
    for gop in [10u32, 4] {
        round_trip_cfg(cabac(cfg(26, gop, false, 0)), 10, frame, 35.0);
    }
    let still = round_trip_cfg(cabac(cfg(26, 10, false, 0)), 5, |_| frame(0), 35.0).0;
    println!("cabac static P bytes: {:?}", still.iter().skip(1).map(|a| a.annexb().len()).collect::<Vec<_>>());
    for (name, scene) in [("moving", frame as fn(usize) -> [Vec<u8>; 3]), ("pan", pan), ("pan_frac", pan_frac)] {
        for (qp, deblock, subpel, floor) in [(16u8, false, 0u8, 35.0), (26, true, 2, 33.0), (40, false, 1, 20.0), (30, true, 2, 30.0)] {
            let (cv, _) = round_trip_cfg(cfg(qp, 4, deblock, subpel), 8, scene, floor);
            let (cb, _) = round_trip_cfg(cabac(cfg(qp, 4, deblock, subpel)), 8, scene, floor);
            let bytes = |aus: &[Access]| aus.iter().map(|a| a.annexb().len()).sum::<usize>();
            println!("cabac {name} qp {qp} deblock {deblock} subpel {subpel}: cavlc={} cabac={} bytes", bytes(&cv), bytes(&cb));
        }
    }
}

/// H264ENC8X8 (LEDGER SR322): every scene above with `transform8x8` (High: Intra_8x8 in the IDRs, the
/// P macroblocks on the 4x4 transform with transform_size_8x8_flag 0 written after a luma
/// coded_block_pattern), under both entropy coders, decodes frame by frame to the encoder's
/// reconstruction byte for byte: the moving pattern (GOP 10 and 4), the static sequence, the pans
/// with the filter and the refinement. Bytes printed against the knob off.
#[test]
fn t8x8_every_p_scene_decodes_to_the_reconstruction() {
    for entropy in [h264enc_core::Entropy::Cavlc, h264enc_core::Entropy::Cabac] {
        let on = |c: Config| Config { entropy, transform8x8: true, ..c };
        let off = |c: Config| Config { entropy, ..c };
        for gop in [10u32, 4] {
            round_trip_cfg(on(cfg(26, gop, false, 0)), 10, frame, 35.0);
        }
        round_trip_cfg(on(cfg(26, 10, false, 0)), 5, |_| frame(0), 35.0);
        for (name, scene) in [("moving", frame as fn(usize) -> [Vec<u8>; 3]), ("pan", pan), ("pan_frac", pan_frac)] {
            for (qp, deblock, subpel, floor) in [(16u8, false, 0u8, 35.0), (26, true, 2, 33.0), (40, false, 1, 20.0), (30, true, 2, 30.0)] {
                let (a, _) = round_trip_cfg(off(cfg(qp, 4, deblock, subpel)), 8, scene, floor);
                let (b, _) = round_trip_cfg(on(cfg(qp, 4, deblock, subpel)), 8, scene, floor);
                let bytes = |aus: &[Access]| aus.iter().map(|a| a.annexb().len()).sum::<usize>();
                println!("8x8 {entropy:?} {name} qp {qp} deblock {deblock} subpel {subpel}: off={} on={} bytes", bytes(&a), bytes(&b));
            }
        }
    }
}
