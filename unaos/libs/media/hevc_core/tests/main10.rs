// SPDX-License-Identifier: LGPL-3.0-or-later
//! HEVCMAIN10 (LEDGER SR311) KATs: Main 10 pictures assembled by hand with the test kit's CABAC encoder, each checkable
//! by construction against a hand-built 16-bit picture: (a) a 32x32 INTRA_DC picture with no residual is
//! 1 << (BitDepth - 1) = 512 everywhere; (b) a 32x32 pcm coding unit with PcmBitDepthY 10 (samples above 255) and
//! PcmBitDepthC 9 (decoded << 1, 8.4.4.1); (c) one DC level in a 16x16 luma TU at Qp'Y = 26 + QpBdOffsetY 12 = 38
//! through 8.6.2 / 8.6.4.2 at bdShift 20 - 10; (d) an 8-bit Frame keeps bit_depth 8 and no planes16; (e) the refusals
//! by name above 10 bits and for unequal luma / chroma depths (HEVCREXT). `planes` is always planes16 >> (BitDepth - 8).

mod common;

use common::*;
use hevc_core::tables::*;
use hevc_core::{Decoder, Error, Frame};

fn decode(stream: &[u8]) -> Vec<Frame> {
    let mut d = Decoder::new();
    d.decode_annexb(stream).expect("decode");
    d.flush();
    d.take_frames()
}

fn headers(cfg: &SpsCfg, p: &PpsCfg) -> Vec<u8> {
    let mut s = vps(false);
    s.extend(sps(cfg));
    s.extend(pps(p));
    s
}

fn cfg32(bit_depth: u32) -> SpsCfg {
    SpsCfg { width: 32, height: 32, log2_ctb: 5, bit_depth, ..Default::default() }
}

/// One 32x32 CTB holding one INTRA_DC coding unit, no residual.
fn dc32(cfg: &SpsCfg) -> Vec<u8> {
    let mut s = headers(cfg, &PpsCfg { init_qp: 26, ..Default::default() });
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, false), 0, 26);
    e.split_cu(0, false);
    e.dc_cu(1); // MPM {planar, DC, 26}: DC is mpm_idx 1
    s.extend(nal(19, &e.finish().out));
    s
}

/// Every sample of `planes16` narrowed by `shift` equals `planes`.
fn narrow_matches(f: &Frame, shift: u32) {
    let p16 = f.planes16.as_ref().expect("planes16 above 8 bits");
    for c in 0..3 {
        assert_eq!(p16[c].len(), f.planes[c].len());
        assert!(p16[c].iter().zip(f.planes[c].iter()).all(|(&a, &b)| (a >> shift) as u8 == b), "plane {c}");
    }
}

#[test]
fn main10_all_dc_is_flat_512() {
    let f = decode(&dc32(&cfg32(10)));
    assert_eq!(f.len(), 1);
    assert_eq!((f[0].width, f[0].height, f[0].bit_depth), (32, 32, 10));
    let p16 = f[0].planes16.as_ref().unwrap();
    assert!(p16.iter().all(|p| p.iter().all(|&v| v == 512)));
    assert_eq!((p16[0].len(), p16[1].len()), (32 * 32, 16 * 16));
    assert!(f[0].planes.iter().all(|p| p.iter().all(|&v| v == 128)));
    narrow_matches(&f[0], 2);
}

fn y16(x: usize, y: usize) -> u16 {
    ((x * 37 + y * 29 + 300) % 1024) as u16
}
fn cb9(x: usize, y: usize) -> u16 {
    ((x * 13 + y * 7 + 100) % 512) as u16
}
fn cr9(x: usize, y: usize) -> u16 {
    (511 - x * 5 - y * 3) as u16
}

#[test]
fn main10_pcm_keeps_ten_bit_samples() {
    let cfg = SpsCfg { pcm: Some((10, 9, 3, 5, true)), ..cfg32(10) };
    let mut s = headers(&cfg, &PpsCfg { init_qp: 26, deblocking_disabled: true, ..Default::default() });
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 26);
    let plane = |f: fn(usize, usize) -> u16, n: usize| (0..n * n).map(|i| f(i % n, i / n)).collect::<Vec<u16>>();
    let (y, cb, cr) = (plane(y16, 32), plane(cb9, 16), plane(cr9, 16));
    e.pcm_cu_bits(0, 5, 10, 9, &y, &cb, &cr);
    s.extend(nal(19, &e.finish().out));
    let f = decode(&s);
    assert_eq!(f[0].bit_depth, 10);
    let p16 = f[0].planes16.as_ref().unwrap();
    assert!(y.iter().any(|&v| v > 255));
    assert_eq!(p16[0], y);
    // 8.4.4.1 (pcm): PcmBitDepthC 9 in a 10-bit picture is shifted left by BitDepthC - PcmBitDepthC = 1.
    assert_eq!(p16[1], cb.iter().map(|&v| v << 1).collect::<Vec<u16>>());
    assert_eq!(p16[2], cr.iter().map(|&v| v << 1).collect::<Vec<u16>>());
    narrow_matches(&f[0], 2);
    assert_eq!(f[0].planes[0][1], (y16(1, 0) >> 2) as u8);
}

/// 8.6.2 then 8.6.4.2 for one DC level in a 16x16 block at Qp'Y 38 (QpY 26 + QpBdOffsetY 12), flat scaling, 10 bits.
fn dc_residual_10(level: i64) -> i32 {
    let (bd, log2, qp) = (10i64, 4i64, 38i64);
    let bd_shift = bd + log2 - 5;
    let d = (level * 16 * ((LEVEL_SCALE[(qp % 6) as usize] as i64) << (qp / 6)) + (1 << (bd_shift - 1))) >> bd_shift;
    let g = ((64 * d + 64) >> 7).clamp(-32768, 32767);
    let bd_shift2 = 20 - bd;
    ((64 * g + (1 << (bd_shift2 - 1))) >> bd_shift2) as i32
}

#[test]
fn main10_one_dc_coefficient_scales_at_ten_bits() {
    let cfg = SpsCfg { width: 16, height: 16, log2_ctb: 4, log2_max_tb: 4, bit_depth: 10, ..Default::default() };
    let mut s = headers(&cfg, &PpsCfg { init_qp: 26, deblocking_disabled: true, ..Default::default() });
    let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 26);
    e.split_cu(0, false);
    e.prev_intra(true);
    e.mpm_idx(1); // DC
    e.chroma_dm();
    e.cbf_chroma(0, false);
    e.cbf_chroma(0, false);
    e.cbf_luma(0, true);
    // residual_coding of the 16x16 luma TB holding one level 16 at (0,0): last prefixes 0 (ctxOffset 3 * 2 + 0 = 6),
    // greater1 1 (ctxSet 0, greater1Ctx 1), greater2 1, sign 0, remaining 16 - 3 = 13.
    e.decision(LAST_X + 6, 0);
    e.decision(LAST_Y + 6, 0);
    e.decision(GT1 + 1, 1);
    e.decision(GT2, 1);
    e.bypass(0);
    e.remaining(13, 0);
    s.extend(nal(19, &e.finish().out));
    let f = decode(&s);
    let r = dc_residual_10(16);
    // d = (16 * 16 * (51 << 6) + 256) >> 9 = 1632, g = (64 * 1632 + 64) >> 7 = 816, r = (64 * 816 + 512) >> 10 = 51.
    assert_eq!(r, 51);
    let p16 = f[0].planes16.as_ref().unwrap();
    assert!(p16[0].iter().all(|&v| v as i32 == 512 + r), "{:?}", &p16[0][..8]);
    assert!(p16[1].iter().chain(p16[2].iter()).all(|&v| v == 512));
    assert!(f[0].planes[0].iter().all(|&v| v == ((512 + r) >> 2) as u8));
    narrow_matches(&f[0], 2);
}

#[test]
fn eight_bit_frames_carry_no_planes16() {
    let f = decode(&dc32(&cfg32(8)));
    assert_eq!((f[0].bit_depth, f[0].planes16.is_none()), (8, true));
    assert!(f[0].planes.iter().all(|p| p.iter().all(|&v| v == 128)));
}

#[test]
fn twelve_bit_and_unequal_depths_are_refused_by_name() {
    for (cfg, want) in [
        (cfg32(12), "bit depth above 10 (HEVCREXT)"),
        (SpsCfg { chroma_bit_depth: Some(8), ..cfg32(10) }, "luma and chroma bit depths differ (HEVCREXT)"),
    ] {
        assert_eq!(Decoder::new().decode_annexb(&dc32(&cfg)).unwrap_err(), Error::Unsupported(want));
    }
}
