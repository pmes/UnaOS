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
//! SEIs by its decoder (`Decoder::take_hrd_notes`); no bit walker of its own. H264ENCHRDVBRKAT (SR1034): the VBR twins (e-h, the file tail) through
//! the same replay, its cbr_flag 0 arm (arrival at the max rate stops while the CPB is full).

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
fn run_src(cfg: Config, n: usize, decode: bool, src: fn(usize) -> [Vec<u8>; 3]) -> Vec<Access> {
    let mut enc = Encoder::new(cfg).unwrap();
    let mut dec = Decoder::new();
    let mut out = Vec::new();
    for t in 0..n {
        let p = src(t);
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
    /// The VCL HRD's initial_cpb_removal_delay per buffering_period (H264ENCHRDVCL SR1060; empty without `Config::hrd_vcl`).
    vbp: Vec<u32>,
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
                        assert!(vcl.is_empty() || vcl == nal, "the VCL delay set mirrors the NAL set (SR1060)"); s.vbp.extend(vcl.iter().map(|d| d.0));
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
    let h = sps.nal_hrd.or(sps.vcl_hrd).expect("nal_hrd_parameters"); // the NAL HRD (Type II, every NAL unit), else the VCL HRD (Type I: VCL + filler, SR1060)
    let cbr = h.cbr; // H264ENCHRDVBRKAT (SR1034): cbr_flag 0 is the VBR replay (arrival at BitRate, the max, stops while the CPB is full: no overflow under VBR); the CBR arm unchanged
    // cbr_flag 1: continuous arrival from t = 0 at BitRate; cbr_flag 0 (the VCL HRD, SR1060): on demand, `vbr_road` (C.1.2).
    let (nut, ts, br, cpb) = (nut as i128, ts as i128, h.bit_rate as i128, h.cpb_size as i128);
    let notes = seis(aus);
    let unit = 90_000 * ts;
    let tick = nut * 90_000;
    let (mut anchor, mut cum, mut fillers) = (None::<i128>, 0i128, 0usize); let mut road: Vec<(i128, i128, i128)> = Vec::new();
    let (mut margin, mut peak) = (i128::MAX, 0i128);
    let (mut fill, mut last) = (0i128, 0i128); // SR1034: the VBR CPB fullness x unit after the last removal, and its time
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
        let before = if cbr { br * t - cum * unit } else { (fill + br * (t - last)).min(cpb * unit) }; // CPB fullness x unit just before the removal (bits arrived - bits removed)
        let b = if sps.nal_hrd.is_some() { bits(au) } else { vcl_bits(au) }; cum += b; road.push((t, b, s.bp.first().map_or(road.last().map_or(0, |r| r.2), |&d| d as i128 * ts)));
        let arrived = if cbr { br * t - cum * unit } else { before - b * unit }; // >= 0: all of the access unit has arrived
        (fill, last) = (arrived, t); // SR1034
        if let (false, Some(&d)) = (cbr, s.bp.first()) {
            assert!(d as i128 * br * ts <= before, "access unit {n}: VBR initial_cpb_removal_delay {d} claims more than the CPB holds"); // SR1034: the delay is the fullness at the max rate (H264ENC-HRD-VBR-INITIAL-DELAY's rule)
        }
        margin = margin.min(arrived / unit);
        peak = peak.max(before / unit);
        assert!(arrived >= 0, "access unit {n}: CPB underflow ({} bits short)", -arrived / unit);
        assert!(!h.cbr || before <= cpb * unit, "access unit {n}: CPB overflow ({} bits over {cpb})", (before - cpb * unit) / unit);
        fillers += au.nals.iter().filter(|x| x[0] & 0x1f == NAL_FILLER).count();
    }
    // SR1060 fold: the NAL HRD's cbr_flag 0 keeps SR1034's inline VBR replay (its KATs pin that road); the VCL HRD alone takes `vbr_road` (C.1.2).
    if h.cbr || sps.nal_hrd.is_some() { (fillers, margin, peak) } else { vbr_road(&road, br, cpb, unit, fillers) }
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

// ---- H264ENCHRDVBRKAT (LEDGER SR1034): VBR twins of the HRD KATs (named tail block) ----
// `Config::hrd` under a VBR rate signals rc.rs's max bucket (`Rc::hrd_params`: BitRate = CpbSize = the max, cbr_flag 0) and each
// buffering period's initial_cpb_removal_delay as the max bucket's fullness at the max rate (`Encoder::hrd_au`, x264's rule). The
// twins replay it through `replay`'s cbr_flag 0 arm: bits arrive at the max rate and stop while the CPB is full, a picture never
// leaves it short, and no delay claims more than the CPB holds.

/// Encode `n` frames of tests/rc.rs's scene (the KATs above): `run_src` over [`scene`].
fn run(cfg: Config, n: usize, decode: bool) -> Vec<Access> {
    run_src(cfg, n, decode, scene)
}

/// VBR mean 300 / max `max` kbit/s at 30 fps (under the HRD both round down to 64: the mean to 299968, max 600 stays 600000, max 900 becomes 899968).
fn vbr(max: u32, gop: u32) -> Config {
    Config { rate: Some(RateControl::vbr(300, max, (FPS, 1))), ..cbr400(gop) }
}

/// A new scene: a dense hashed texture drifting one sample per frame right (an IDR here costs far more than [`scene`]'s).
fn busy(t: usize) -> [Vec<u8>; 3] {
    let hash = |x: usize, y: usize, k: u32| {
        let mut h = (x as u32).wrapping_mul(0x9E37_79B1) ^ (y as u32).wrapping_mul(0x85EB_CA77) ^ k;
        h ^= h >> 15;
        h = h.wrapping_mul(0x2C1B_3C6D);
        (h ^ (h >> 12)) >> 24
    };
    let mut p = [vec![0u8; W * H], vec![0u8; W * H / 4], vec![0u8; W * H / 4]];
    for y in 0..H {
        for x in 0..W {
            p[0][y * W + x] = (32 + hash(x + 512 - t, y, 0) * 3 / 4) as u8;
        }
    }
    for y in 0..H / 2 {
        for x in 0..W / 2 {
            p[1][y * (W / 2) + x] = (96 + hash(2 * x + 512 - t, y, 1) / 4) as u8;
            p[2][y * (W / 2) + x] = (96 + hash(2 * x + 512 - t, y, 2) / 4) as u8;
        }
    }
    p
}

#[test]
fn e_vbr_sps_signals_the_max_bucket() {
    for max in [600u32, 900] {
        let v = sps_of(&run(vbr(max, 6), 1, false)[0].nals[0]);
        println!("hrd sps vbr 300 / {max}: {v:?}");
        assert_eq!(v.timing_info, Some((1, 2 * FPS, true)), "two ticks per frame");
        let h = v.nal_hrd.expect("nal_hrd_parameters");
        let lens = [h.initial_cpb_removal_delay_len, h.cpb_removal_delay_len, h.dpb_output_delay_len, h.time_offset_len];
        assert_eq!((h.cpb_cnt, h.bit_rate, h.cpb_size, h.cbr, lens), (1, (max * 1000 & !63) as u64, (max * 1000 & !63) as u64, false, [24, 24, 24, 24]), "VBR {max}: BitRate / CpbSize the max bucket rounded to 64 (600000, 899968; not the mean 299968), cbr_flag 0");
        assert!(v.vcl_hrd.is_none());
        assert!(!v.low_delay_hrd);
        assert_eq!(v.level_idc, 13, "level 1.3: the max past level 1.2's 384 x 1200 (the mean would fit 1.2)");
    }
}

#[test]
fn f_vbr_cpb_replay_never_underflows() {
    for bframes in [0u8, 2] {
        let aus = run(Config { bframes, ..vbr(600, 6) }, N, false);
        assert_eq!(aus.len(), N);
        let v = sps_of(&aus[0].nals[0]);
        let (fillers, margin, peak) = replay(&aus, &v);
        let total: i128 = aus.iter().map(bits).sum();
        println!(
            "hrd vbr 600 kbit/s bframes {bframes}: {N} access units, {total} bits ({:.1} kbit/s), arrival margin min {margin} bits, cpb peak {peak} / 600000 bits",
            total as f64 * FPS as f64 / N as f64 / 1000.0
        );
        println!("  bytes {:?}", aus.iter().map(|a| bits(a) / 8).collect::<Vec<_>>());
        assert_eq!(fillers, 0, "VBR pads nothing");
        assert_eq!(peak, 600_000, "the CPB fills to CpbSize and stops (cbr_flag 0)");
        assert!(margin >= 0);
    }
}

#[test]
fn g_vbr_every_picture_times_and_decodes() {
    let aus = run(vbr(600, 6), N, true);
    let v = sps_of(&aus[0].nals[0]);
    replay(&aus, &v);
    let notes = seis(&aus);
    let mut idrs = 0;
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
        assert_eq!(au.nals.iter().position(|x| x[0] & 0x1f == NAL_SEI), Some(if idr { 2 } else { 0 }));
        assert!(au.nals.iter().all(|x| x[0] & 0x1f != NAL_FILLER), "access unit {n}: VBR writes no filler");
    }
    assert_eq!(idrs, 2);
    let aus = run(Config { bframes: 2, ..vbr(600, 6) }, N, false);
    for (au, s) in aus.iter().zip(seis(&aus)) {
        assert_eq!(s.pt[0].1 as u64, 2 * (au.pts - au.dts));
    }
}

#[test]
fn h_vbr_scene_change_stays_in_bucket() {
    let aus = run_src(vbr(600, 6), N, true, |t| if t < 6 { scene(t) } else { busy(t) });
    let v = sps_of(&aus[0].nals[0]);
    let idr: Vec<i128> = aus.iter().filter(|a| a.nals.iter().any(|x| x[0] & 0x1f == NAL_IDR)).map(bits).collect();
    let (fillers, margin, peak) = replay(&aus, &v);
    let bp: Vec<u32> = seis(&aus).iter().flat_map(|s| s.bp.clone()).collect();
    println!("hrd vbr scene change: idr {} -> {} bits, initial_cpb_removal_delay {bp:?}, arrival margin min {margin} bits, cpb peak {peak} / 600000 bits", idr[0], idr[1]);
    println!("  bytes {:?}", aus.iter().map(|a| bits(a) / 8).collect::<Vec<_>>());
    assert_eq!(idr.len(), 2);
    assert!(idr[1] >= 2 * idr[0], "the scene change's IDR ({}) is not twice the first ({})", idr[1], idr[0]);
    assert_eq!((fillers, bp.len()), (0, 2));
    assert!(margin >= 0);
}

// ---- H264ENCHRDVBRUNDERFLOW (LEDGER SR1050): the VBR row-QP room term at the knife edge (named tail block) ----
// A VBR picture whose bits pass the max bucket's room at its start (bmax - fmax: the CPB `hrd_au` signals is the bucket's mirror)
// underflows the CPB. The knife edge: the stream's first IDR has no fitted model, so `Rc::picture` codes it at start_qp (cfg.qp 16
// here) whatever the target, and only `row_qp` can move it. While the target cap is idle its size does not depend on the max, so
// the run at max 2000 (room x 15/16 far above it: the term cannot fire) IS the old code's IDR at the knife max: the armed contrast
// is measured at runtime. rc.rs `Rc::room_step` (SR1050) raises the rows' QP until the projected picture fits the room.

/// [`busy`]'s first picture frozen for 6 frames at VBR mean 64 / max `max` kbit/s, gop 12, cfg.qp 16, deblocking on (so
/// `Encoder::mb_qps` holds the coded QPs): the access units and the IDR's QP per macroblock row (the row's largest).
fn knife(max: u32) -> (Vec<Access>, Vec<u8>) {
    let cfg = Config { width: W as u32, height: H as u32, qp: 16, gop: 12, deblock: true, hrd: true, rate: Some(RateControl::vbr(64, max, (FPS, 1))), ..Default::default() };
    let mut enc = Encoder::new(cfg).unwrap();
    let p = busy(0);
    let (mut aus, mut rows) = (Vec::new(), Vec::new());
    for t in 0..6 {
        aus.push(enc.encode(&Yuv420 { width: W as u32, height: H as u32, y: &p[0], u: &p[1], v: &p[2], y_stride: W, uv_stride: W / 2 }).unwrap());
        if t == 0 {
            rows = enc.mb_qps().chunks(W / 16).map(|r| *r.iter().max().unwrap()).collect();
        }
    }
    (aus, rows)
}

/// The idle run (max 2000) and the knife run (max = 15/16 of the idle IDR's bits): (idle IDR bits, idle rows, knife units, knife rows).
fn knife_pair() -> (i128, Vec<u8>, Vec<Access>, Vec<u8>) {
    let (idle, idle_rows) = knife(2000);
    let x = bits(&idle[0]);
    let (aus, rows) = knife((x * 15 / 16 / 1000) as u32);
    (x, idle_rows, aus, rows)
}

#[test]
fn i_vbr_knife_edge_does_not_underflow() {
    let (x, idle_rows, aus, rows) = knife_pair();
    let v = sps_of(&aus[0].nals[0]);
    let cpb = v.nal_hrd.expect("nal_hrd_parameters").cpb_size as i128;
    let y = bits(&aus[0]);
    println!("hrd vbr knife edge: idle idr {x} bits rows {idle_rows:?}, cpb {cpb} (old margin {}), knife idr {y} bits rows {rows:?}", cpb - x);
    println!("  bytes {:?}", aus.iter().map(|a| bits(a) / 8).collect::<Vec<_>>());
    assert!(cpb < x, "the knife CPB ({cpb}) holds the old code's IDR ({x}): not a knife edge");
    let (fillers, margin, peak) = replay(&aus, &v);
    println!("  arrival margin min {margin} bits, cpb peak {peak} / {cpb} bits");
    assert_eq!(fillers, 0, "VBR pads nothing");
    assert!(margin >= 0);
    assert!(y <= cpb && y < x, "the knife IDR ({y}) shrank below the CPB ({cpb}) and the old IDR ({x})");
    // Measured (SR1050 EXCEPTION runs): at the old rc.rs the knife run's IDR was 111928 bits and `replay` panicked (CPB underflow,
    // old margin 104000 - 111936 = -7936); with the room term the IDR is 93800 bits and the smallest margin 5698.
    assert_eq!((x, cpb, y, margin), (111_936, 104_000, 93_800, 5_698), "idle IDR, knife CPB, knife IDR, arrival margin");
}

#[test]
fn j_vbr_row_qp_steps_are_bounded() {
    let (_, idle_rows, _, rows) = knife_pair();
    let term: Vec<i32> = rows.iter().zip(&idle_rows).map(|(&k, &i)| k as i32 - i as i32).collect();
    println!("hrd vbr knife row term {term:?} (idle rows {idle_rows:?}, knife rows {rows:?})");
    assert_eq!(term[0], 0, "row 0 never reaches rc_row");
    assert!(term.iter().all(|d| (0..=6).contains(d)), "the room term is bounded at +6");
    assert!(term.windows(2).all(|w| w[0] <= w[1]), "the room term never falls within the picture");
    assert!(term.iter().any(|&d| d > 0), "the room term never fired at the knife edge");
    // Measured (SR1050): idle rows [16, 19 x 5] (row_qp's +3), knife rows [16, 22, 24, 25, 25, 25]; at the old rc.rs the term was all 0.
    assert_eq!(term, [0, 3, 5, 6, 6, 6], "the room term per macroblock row");
}

// ---- H264ENCHRDVCL (LEDGER SR1060): the VCL HRD beside the NAL HRD, replayed as Annex C's Type I point (named tail block) ----

/// The bits of the access unit the Type I CPB receives (C.1): VCL NAL units (nal_unit_type 1..=5) and filler data.
fn vcl_bits(au: &Access) -> i128 {
    au.nals.iter().filter(|n| matches!(n[0] & 0x1f, 1..=5 | NAL_FILLER)).map(|n| n.len() as i128 * 8).sum()
}

/// `replay`'s cbr_flag 0 road (C.1.2): access unit k starts arriving at max(the previous one's final arrival, its removal less its
/// buffering period's initial_cpb_removal_delay) and arrives at BitRate. `road` holds (removal time, bits, initial delay) per
/// access unit in `replay`'s time unit; times are scaled by BitRate (bits x unit). Asserts no underflow (each access unit has
/// arrived by its removal) and no overflow (the fullness before each removal within CpbSize); returns (fillers, the smallest
/// arrival margin in bits, the largest fullness before a removal).
fn vbr_road(road: &[(i128, i128, i128)], br: i128, cpb: i128, unit: i128, fillers: usize) -> (usize, i128, i128) {
    let mut start = Vec::with_capacity(road.len());
    let mut end = i128::MIN;
    let (mut margin, mut peak) = (i128::MAX, 0i128);
    for (n, &(t, b, d)) in road.iter().enumerate() {
        let a = end.max((t - d) * br);
        end = a + b * unit;
        start.push(a);
        let m = t * br - end;
        assert!(m >= 0, "access unit {n}: Type I CPB underflow ({} bits short)", -m / unit);
        margin = margin.min(m / unit);
    }
    for (n, &(t, _, _)) in road.iter().enumerate() {
        let full: i128 = road[n..].iter().zip(&start[n..]).map(|(&(_, b, _), &a)| (t * br - a).clamp(0, b * unit)).sum();
        assert!(full <= cpb * unit, "access unit {n}: Type I CPB overflow ({} bits over {cpb})", (full - cpb * unit) / unit);
        peak = peak.max(full / unit);
    }
    (fillers, margin, peak)
}

/// The VCL HRD (cbr_flag 0) beside the NAL HRD, replayed on two roads: the cbr 1 road (continuous arrival) with the Type I bits
/// (VCL + filler), a subset of the NAL bits, so its arrival margin is at least the NAL one; and the cbr 0 road (on-demand arrival,
/// C.1.2) as signalled, which proves itself by `vbr_road`'s own no-underflow / no-overflow asserts (margins printed).
#[test]
fn k_vcl_hrd_mirrors_the_nal_bucket() {
    let aus = run(Config { hrd_vcl: true, ..cbr400(6) }, N, true);
    assert_eq!(aus.len(), N);
    let v = sps_of(&aus[0].nals[0]);
    let nal = v.nal_hrd.expect("nal_hrd_parameters");
    assert!(nal.cbr);
    assert_eq!(v.vcl_hrd, Some(h264_core::ps::Hrd { cbr: false, ..nal }), "the VCL table is the NAL table with cbr_flag 0");
    assert!(!v.low_delay_hrd, "low_delay_hrd_flag 0, written once");
    assert_eq!(v.timing_info, Some((1, 2 * FPS, true)));
    let notes = seis(&aus);
    let off = seis(&run(cbr400(6), N, false));
    let mut vbps = 0;
    for (n, (s, o)) in notes.iter().zip(&off).enumerate() {
        assert_eq!(s.vbp, s.bp, "access unit {n}: the VCL delay set is the NAL set");
        assert_eq!(s.pt, o.pt, "access unit {n}: pic_timing unchanged by the VCL HRD");
        vbps += s.vbp.len();
    }
    assert_eq!(vbps, 2, "a VCL delay in each IDR's buffering_period");
    let (f2, m2, p2) = replay(&aus, &v);
    let (f1, m1, p1) = replay(&aus, &Sps { nal_hrd: None, ..v.clone() });
    let (_, ms, ps) = replay(&aus, &Sps { nal_hrd: None, vcl_hrd: Some(h264_core::ps::Hrd { cbr: true, ..nal }), ..v.clone() });
    println!("hrd vcl cbr 400: type II (nal, cbr 1) fillers {f2} margin {m2} peak {p2}; type I bits on the cbr 1 road margin {ms} peak {ps}; type I (vcl, cbr 0 road) margin {m1} peak {p1} / {} bits", nal.cpb_size);
    assert_eq!(f1, f2);
    assert!(ms >= m2, "the Type I bits are a subset: their cbr 1 arrival margin {ms} below the NAL one {m2}");
    assert!(m1 >= 0 && p1 <= nal.cpb_size as i128, "the cbr 0 road: no underflow, no overflow");
}

#[test]
fn l_vcl_hrd_off_is_byte_identical() {
    // d's constant-QP pin with the VCL HRD asked for too: no bucket, no byte.
    let cfg = Config { width: W as u32, height: H as u32, qp: 26, gop: 30, hrd: true, hrd_vcl: true, ..Default::default() };
    let samples: Vec<Vec<u8>> = run(cfg, 60, false).iter().map(Access::avcc_sample).collect();
    assert_eq!(fnv(&samples), 0xd572_6ae5_9a9b_6d9c, "hrd_vcl without a bucket moved the constant-QP bytes");
    let bytes = |cfg: Config| -> Vec<Vec<u8>> { run(cfg, N, false).iter().map(Access::annexb).collect() };
    assert_eq!(bytes(Config { hrd: false, hrd_vcl: true, ..cbr400(30) }), bytes(Config { hrd: false, ..cbr400(30) }), "hrd_vcl without hrd moved bytes");
    assert_eq!(bytes(Config { hrd_vcl: false, ..cbr400(6) }), bytes(cbr400(6)), "hrd_vcl false is the default's bytes");
    assert_ne!(bytes(Config { hrd_vcl: true, ..cbr400(6) }), bytes(cbr400(6)), "hrd_vcl wrote nothing");
}