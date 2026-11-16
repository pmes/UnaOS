// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! H264ENCHRD (LEDGER SR699): the rate controller's bucket signalled as the NAL HRD. Over tests/rc.rs's scene (128x96, 30 fps),
//! 12 frames at CBR 400 kbit/s: (a) the SPS VUI carries timing_info (1 / 60 ticks) and nal_hrd_parameters whose BitRate and
//! CpbSize are the bucket's (400000 bits/s, a one-second CPB), cbr_flag 1, the delay fields 24 bits, the level raised to hold
//! them (1.2); VBR signals its max bucket with cbr_flag 0; CRF / hrd off write neither; (b) an exact-rational replay of the
//! decoder's CPB at the signalled rate, from the first buffering period's initial_cpb_removal_delay and each pic_timing's
//! cpb_removal_delay chain, never underflows (each access unit has arrived by its removal) and never overflows (the CPB stays
//! within CpbSize before each removal; the filler NAL units counted), with and without B pictures, and at 2000 kbit/s over 60
//! frames, a bucket the scene cannot fill, where the filler holds the drained bucket (and the stream still decodes); (c) every access unit carries
//! one pic_timing SEI and every IDR (only an IDR) one buffering_period, and h264_core decodes each to the encoder's
//! reconstruction byte for byte (SEI and filler skipped); (d) `hrd: true` without a bucket (constant QP: tests/rc.rs's pin, CRF)
//! changes no byte. H264COREHRD (SR708): the SPS is read by h264_core's `ps::parse_sps` (`Sps::timing_info` / `nal_hrd`) and the
//! SEIs by its decoder (`Decoder::take_hrd_notes`); no bit walker of its own.

use h264_core::decoder::HrdKind;
use h264_core::nal::{unescape, NAL_FILLER, NAL_IDR, NAL_PPS, NAL_SEI, NAL_SPS};
use h264_core::ps::{parse_sps, Sps};
use h264_core::Decoder;
use h264enc_core::{Access, Config, Encoder, RateControl, Yuv420};

const W: usize = 128;
const H: usize = 96;
const FPS: u32 = 30;
const N: usize = 12;

/// tests/rc.rs's scene: a textured gradient, a square moving 3 samples per frame right and a patterned block moving 2 down.
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

fn cbr400(gop: u32) -> Config {
    Config { width: W as u32, height: H as u32, qp: 26, gop, rate: Some(RateControl::cbr(400, (FPS, 1))), hrd: true, ..Default::default() }
}

/// Encode `n` frames of the scene (push / flush, so B pictures come out in decoding order); with `decode`, h264_core decodes
/// every access unit (bframes 0) and it must equal the reconstruction.
fn run(cfg: Config, n: usize, decode: bool) -> Vec<Access> {
    let mut enc = Encoder::new(cfg).unwrap();
    let mut dec = Decoder::new();
    let mut out = Vec::new();
    for t in 0..n {
        let p = scene(t);
        let aus = enc.push(&Yuv420 { width: W as u32, height: H as u32, y: &p[0], u: &p[1], v: &p[2], y_stride: W, uv_stride: W / 2 }).unwrap();
        for (au, rec) in aus.iter().zip(enc.reconstructions()) {
            if decode {
                dec.decode_annexb(&au.annexb()).unwrap();
                dec.end_access_unit();
                let mut f = dec.take_frames();
                assert_eq!(f.len(), 1, "frame {t}: one picture out");
                let f = f.pop().unwrap();
                for c in 0..3 {
                    assert_eq!(f.planes[c].as_slice(), rec[c].as_slice(), "frame {t} plane {c}: decode differs from the reconstruction");
                }
            }
        }
        out.extend(aus);
    }
    out.extend(enc.flush().unwrap());
    out
}

fn rbsp(nal: &[u8]) -> Vec<u8> {
    let mut v = Vec::new();
    unescape(&nal[1..], &mut v);
    v
}

/// The SPS as h264_core reads it (level_idc, timing_info, nal_hrd, vcl_hrd, low_delay_hrd).
fn sps_of(nal: &[u8]) -> Sps {
    assert_eq!(nal[0] & 0x1f, NAL_SPS);
    parse_sps(&rbsp(nal)).unwrap()
}

/// The SEI messages of one access unit: (buffering_period initial_cpb_removal_delay, pic_timing (cpb_removal_delay,
/// dpb_output_delay)) per SEI NAL, with the counts of each.
#[derive(Debug, Default)]
struct Sei {
    bp: Vec<u32>,
    pt: Vec<(u32, u32)>,
}

/// h264_core's decoder fed every access unit's SPS / PPS / SEI NAL units: its HRD notes, per access unit.
fn seis(aus: &[Access]) -> Vec<Sei> {
    let mut dec = Decoder::new();
    aus.iter()
        .map(|au| {
            for n in au.nals.iter().filter(|n| matches!(n[0] & 0x1f, NAL_SPS | NAL_PPS | NAL_SEI)) {
                dec.decode_nal(n).unwrap();
            }
            let mut s = Sei::default();
            for note in dec.take_hrd_notes() {
                match note.kind {
                    HrdKind::BufferingPeriod { sps_id, nal, vcl } => {
                        assert_eq!(sps_id, 0, "seq_parameter_set_id");
                        assert!(vcl.is_empty(), "no VCL HRD");
                        assert_eq!(nal.len(), 1, "one CPB");
                        assert_eq!(nal[0].1, 0, "initial_cpb_removal_delay_offset");
                        s.bp.push(nal[0].0);
                    }
                    HrdKind::PicTiming { cpb_removal_delay, dpb_output_delay } => s.pt.push((cpb_removal_delay, dpb_output_delay)),
                }
            }
            s
        })
        .collect()
}

fn bits(au: &Access) -> i128 {
    au.nals.iter().map(|n| n.len() as i128 * 8).sum()
}

/// The decoder's CPB replayed exactly (C.1, CBR arrival from t = 0 at BitRate): returns (the filler NAL units, the smallest
/// arrival margin in bits at a removal, the largest fullness before a removal). Time is counted in 1 / (90000 x time_scale) s.
fn replay(aus: &[Access], sps: &Sps) -> (usize, i128, i128) {
    let (nut, ts, _) = sps.timing_info.expect("timing_info");
    let h = sps.nal_hrd.expect("nal_hrd_parameters");
    assert!(h.cbr, "the replay is the CBR one");
    let (nut, ts, br, cpb) = (nut as i128, ts as i128, h.bit_rate as i128, h.cpb_size as i128);
    let notes = seis(aus);
    let unit = 90_000 * ts;
    let tick = nut * 90_000;
    let (mut anchor, mut cum, mut fillers) = (None::<i128>, 0i128, 0usize);
    let (mut margin, mut peak) = (i128::MAX, 0i128);
    for (n, au) in aus.iter().enumerate() {
        let s = &notes[n];
        let (cpb_delay, _) = s.pt[0];
        let t = match (anchor, s.bp.first()) {
            (None, Some(&d)) => d as i128 * ts,
            (Some(a), _) => a + cpb_delay as i128 * tick,
            (None, None) => panic!("the stream does not start with a buffering period"),
        };
        if anchor.is_none() || !s.bp.is_empty() {
            anchor = Some(t);
        }
        assert_eq!(t, aus_first(&notes, ts) + 2 * n as i128 * tick, "access unit {n}: removal times are one frame apart");
        let before = br * t - cum * unit; // CPB fullness x unit just before the removal (bits arrived - bits removed)
        cum += bits(au);
        let arrived = br * t - cum * unit; // >= 0: all of the access unit has arrived
        margin = margin.min(arrived / unit);
        peak = peak.max(before / unit);
        assert!(arrived >= 0, "access unit {n}: CPB underflow ({} bits short)", -arrived / unit);
        assert!(before <= cpb * unit, "access unit {n}: CPB overflow ({} bits over {cpb})", (before - cpb * unit) / unit);
        fillers += au.nals.iter().filter(|x| x[0] & 0x1f == NAL_FILLER).count();
    }
    (fillers, margin, peak)
}

fn aus_first(notes: &[Sei], ts: i128) -> i128 {
    notes[0].bp[0] as i128 * ts
}

#[test]
fn a_sps_signals_the_bucket() {
    let aus = run(cbr400(6), 2, false);
    let v = sps_of(&aus[0].nals[0]);
    println!("hrd sps cbr 400: {v:?}");
    assert_eq!(v.timing_info, Some((1, 2 * FPS, true)), "two ticks per frame");
    let lens = |h: h264_core::ps::Hrd| [h.initial_cpb_removal_delay_len, h.cpb_removal_delay_len, h.dpb_output_delay_len, h.time_offset_len];
    assert_eq!(v.nal_hrd.map(|h| (h.cpb_cnt, h.bit_rate, h.cpb_size, h.cbr, lens(h))), Some((1, 400_000, 400_000, true, [24, 24, 24, 24])), "BitRate / CpbSize are the bucket's");
    assert!(v.vcl_hrd.is_none());
    assert!(!v.low_delay_hrd);
    assert_eq!(v.level_idc, 12, "level 1.2: 400 kbit/s past level 1.1's 192 x 1200");
    // VBR: the max bucket, cbr_flag 0; a rate not a multiple of 64 is rounded down to the signalled one.
    let vbr = Config { rate: Some(RateControl::vbr(300, 601, (FPS, 1))), ..cbr400(6) };
    let v = sps_of(&run(vbr, 1, false)[0].nals[0]);
    assert_eq!(v.nal_hrd.map(|h| (h.bit_rate, h.cpb_size, h.cbr)), Some((600_960, 600_960, false)), "VBR: the max bucket, 601000 rounded to 64");
    // hrd off, CRF: neither flag.
    for cfg in [Config { hrd: false, ..cbr400(6) }, Config { rate: Some(RateControl::crf(26, (FPS, 1))), ..cbr400(6) }] {
        let v = sps_of(&run(cfg, 1, false)[0].nals[0]);
        assert_eq!((v.timing_info, v.nal_hrd, v.low_delay_hrd), (None, None, false), "{:?} hrd {}", cfg.rate, cfg.hrd);
        assert_eq!(v.level_idc, 10);
    }
}

#[test]
fn b_cpb_replay_never_underflows_or_overflows() {
    for bframes in [0u8, 2] {
        let cfg = Config { bframes, ..cbr400(6) };
        let aus = run(cfg, N, false);
        assert_eq!(aus.len(), N);
        let v = sps_of(&aus[0].nals[0]);
        let (fillers, margin, peak) = replay(&aus, &v);
        let total: i128 = aus.iter().map(bits).sum();
        println!(
            "hrd cbr 400 kbit/s bframes {bframes}: {N} access units, {total} bits ({:.1} kbit/s), filler NAL units {fillers}, arrival margin min {margin} bits, cpb peak {peak} / 400000 bits",
            total as f64 * FPS as f64 / N as f64 / 1000.0
        );
        println!("  bytes {:?}", aus.iter().map(|a| bits(a) / 8).collect::<Vec<_>>());
    }
    // A bucket the scene cannot fill (2000 kbit/s, 60 frames: the QP floor 16 caps the pictures below the drain): the
    // half-full bucket drains to empty and the filler NAL units hold it there; the replay counts them.
    let cfg = Config { rate: Some(RateControl::cbr(2000, (FPS, 1))), ..cbr400(30) };
    let aus = run(cfg, 60, true);
    let v = sps_of(&aus[0].nals[0]);
    let (fillers, margin, peak) = replay(&aus, &v);
    println!("hrd cbr 2000 kbit/s: 60 access units, filler NAL units {fillers}, arrival margin min {margin} bits, cpb peak {peak} / {} bits", v.nal_hrd.unwrap().cpb_size);
    assert!(fillers > 0, "the filler path was not exercised");
}

#[test]
fn c_every_picture_times_and_decodes() {
    let aus = run(cbr400(6), N, true);
    let mut idrs = 0;
    let notes = seis(&aus);
    for (n, au) in aus.iter().enumerate() {
        let s = &notes[n];
        assert_eq!(s.pt.len(), 1, "access unit {n}: one pic_timing");
        let idr = au.nals.iter().any(|x| x[0] & 0x1f == NAL_IDR);
        assert_eq!(s.bp.len(), idr as usize, "access unit {n}: a buffering_period exactly on an IDR");
        assert_eq!(s.pt[0].1, 0, "no reorder: dpb_output_delay 0");
        let since = n % 6;
        let want = if since == 0 { if n == 0 { 0 } else { 12 } } else { 2 * since as u32 };
        assert_eq!(s.pt[0].0, want, "access unit {n}: cpb_removal_delay");
        idrs += idr as usize;
        // The SEI is ahead of the slice (after an IDR's SPS / PPS); filler, if any, last.
        let at = au.nals.iter().position(|x| x[0] & 0x1f == NAL_SEI).unwrap();
        assert_eq!(at, if idr { 2 } else { 0 });
        // The avcC sample carries the SEI (and filler) too.
        assert!(au.avcc_sample().len() > au.nals.iter().filter(|x| matches!(x[0] & 0x1f, 1 | 5)).map(|x| x.len() + 4).sum::<usize>());
    }
    assert_eq!(idrs, 2);
    // Under B pictures the output delay is the reorder: 2 x (pts - dts).
    let aus = run(Config { bframes: 2, ..cbr400(6) }, N, false);
    for (au, s) in aus.iter().zip(seis(&aus)) {
        assert_eq!(s.pt[0].1 as u64, 2 * (au.pts - au.dts));
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
fn d_no_bucket_no_bytes_moved() {
    // tests/rc.rs `d_rate_none_keeps_the_constant_qp_bytes`' first pin (60 frames, qp 26, gop 30), with hrd asked for.
    let cfg = Config { width: W as u32, height: H as u32, qp: 26, gop: 30, hrd: true, ..Default::default() };
    let samples: Vec<Vec<u8>> = run(cfg, 60, false).iter().map(Access::avcc_sample).collect();
    assert_eq!(fnv(&samples), 0xd572_6ae5_9a9b_6d9c, "hrd without a bucket moved the constant-QP bytes"); // SR771 (H264ENCPINTRACHROMA: the intra / inter compare pays the chroma): was 0x1fbc_56dc_6807_f2a8
    let crf = |hrd| Config { rate: Some(RateControl::crf(26, (FPS, 1))), hrd, ..cbr400(30) };
    let a: Vec<Vec<u8>> = run(crf(true), N, false).iter().map(Access::annexb).collect();
    let b: Vec<Vec<u8>> = run(crf(false), N, false).iter().map(Access::annexb).collect();
    assert_eq!(a, b, "hrd under CRF moved bytes");
    let off: Vec<Vec<u8>> = run(Config { hrd: false, ..cbr400(30) }, N, false).iter().map(Access::annexb).collect();
    assert!(off.iter().flatten().count() > 0);
    for au in run(Config { hrd: false, ..cbr400(30) }, N, false) {
        assert!(au.nals.iter().all(|x| !matches!(x[0] & 0x1f, NAL_SEI | NAL_FILLER)), "hrd off writes no SEI / filler");
    }
}
