// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! H264ENCCABAC (LEDGER SR310): the CABAC writer against h264_core's CABAC reader. (a) The
//! arithmetic encoder: thousands of pseudo-random decisions over every context, bypass bins and
//! terminate bins read back bin for bin by `h264_core::cabac::Cabac`; (b) residual_block_cabac
//! for every ctxBlockCat < 5 and Exp-Golomb suffixes (levels up to 4000) read back by the
//! decoder's own `residual_block` / `exp_golomb_bypass`; (c) the parameter sets of a CABAC stream
//! read back: profile_idc 77 with constraint_set1, entropy_coding_mode_flag 1, cabac_init_idc 0 in
//! the P slice. The stream KATs (decode == reconstruction for every scene) sit beside their
//! scenes: tests/intra.rs, pframes.rs and rc.rs (`cabac_*`, `e_` / `f_` / `g_`).

use h264_core::bits::BitReader;
use h264_core::cabac::{Cabac, CAT_CHROMA_AC, CAT_CHROMA_DC, CAT_LUMA_4X4, CAT_LUMA_AC, CAT_LUMA_DC, NUM_CTX};
use h264_core::nal;
use h264_core::ps::{self, SliceType};
use h264enc_core::cabac::Writer;
use h264enc_core::nal::BitWriter;
use h264enc_core::{Config, Encoder, Entropy, Yuv420};

struct Rng(u32);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
}

/// One symbol of the engine KAT.
#[derive(Clone, Copy, Debug)]
enum Sym {
    Decision(usize, u32),
    Bypass(u32),
    Terminate0,
    Golomb(u32, u32),
}

/// Write `syms` then a terminating 1 and the alignment, as the encoder ends a slice.
fn write(column: usize, qp: i32, syms: &[Sym]) -> Vec<u8> {
    let mut w = BitWriter::new();
    let mut c = Writer::new(&mut w, column, qp, 1);
    for &s in syms {
        match s {
            Sym::Decision(ctx, b) => c.decision(&mut w, ctx, b),
            Sym::Bypass(b) => c.bypass(&mut w, b),
            Sym::Terminate0 => c.terminate(&mut w, false),
            Sym::Golomb(v, k) => c.exp_golomb_bypass(&mut w, v, k),
        }
    }
    c.terminate(&mut w, true);
    w.u((8 - w.bits() as u32 % 8) % 8, 0);
    w.finish()
}

#[test]
fn a_engine_round_trips_through_the_decoders_engine() {
    let mut rng = Rng(0x2545_f491);
    for (column, qp) in [(0usize, 16i32), (1, 26), (2, 40), (3, 51), (0, 0)] {
        let mut syms = Vec::new();
        // A skewed source per context, so states run to both ends of the table.
        for i in 0..20_000 {
            let r = rng.next();
            let s = match r % 16 {
                0 => Sym::Bypass((r >> 8) & 1),
                1 if i % 7 == 0 => Sym::Terminate0,
                2 => Sym::Golomb((r >> 8) % 3000, (r >> 4) % 4),
                _ => {
                    let ctx = ((r >> 4) as usize) % (NUM_CTX - 1);
                    let ctx = if ctx == 276 { 275 } else { ctx }; // 276 is the terminate context
                    let p1 = (ctx * 37) % 100; // the context's chance of a 1, in percent
                    Sym::Decision(ctx, ((r >> 16) % 100 < p1 as u32) as u32)
                }
            };
            syms.push(s);
        }
        let data = write(column, qp, &syms);
        let mut d = Cabac::new(&data, 0);
        d.init_contexts(column, qp);
        for (i, &s) in syms.iter().enumerate() {
            match s {
                Sym::Decision(ctx, b) => assert_eq!(d.decision(ctx), b, "column {column} qp {qp} symbol {i}: {s:?}"),
                Sym::Bypass(b) => assert_eq!(d.bypass(), b, "symbol {i}: {s:?}"),
                Sym::Terminate0 => assert_eq!(d.terminate(), 0, "symbol {i}: {s:?}"),
                Sym::Golomb(v, k) => assert_eq!(d.exp_golomb_bypass(k), v, "symbol {i}: {s:?}"),
            }
        }
        assert_eq!(d.terminate(), 1, "column {column} qp {qp}: end_of_slice_flag");
        assert!(d.pos() <= data.len() * 8, "the decoder read past the flushed code word");
        println!("engine column {column} qp {qp}: {} symbols in {} bytes", syms.len(), data.len());
    }
}

#[test]
fn b_residual_blocks_round_trip_through_the_decoders_residual_block() {
    let mut rng = Rng(0x1234_5678);
    let cats = [(CAT_LUMA_DC, 16usize), (CAT_LUMA_AC, 15), (CAT_LUMA_4X4, 16), (CAT_CHROMA_DC, 4), (CAT_CHROMA_AC, 15)];
    let mut blocks = Vec::new();
    for n in 0..600 {
        let (cat, max) = cats[n % cats.len()];
        let mut l = vec![0i32; max];
        let density = 1 + rng.next() % 4;
        for v in l.iter_mut() {
            let r = rng.next();
            if r % 4 < density {
                let mag = match (r >> 8) % 10 {
                    0..=5 => 1,
                    6..=8 => 2 + (r >> 12) % 14,
                    _ => 15 + (r >> 12) % 4000,
                };
                *v = if (r >> 30) & 1 == 1 { -(mag as i32) } else { mag as i32 };
            }
        }
        if l.iter().all(|&v| v == 0) {
            l[max - 1] = -3; // the last position alone: the inferred-last path
        }
        blocks.push((cat, max, l));
    }
    let mut w = BitWriter::new();
    let mut c = Writer::new(&mut w, 1, 30, 1);
    for (cat, max, l) in &blocks {
        c.residual_block(&mut w, *cat, *max, l);
    }
    c.terminate(&mut w, true);
    w.u((8 - w.bits() as u32 % 8) % 8, 0);
    let data = w.finish();
    let mut d = Cabac::new(&data, 0);
    d.init_contexts(1, 30);
    let mut buf = [0i32; 64];
    for (n, (cat, max, l)) in blocks.iter().enumerate() {
        let nz = d.residual_block(*cat, *max, &mut buf);
        assert_eq!(&buf[..*max], l.as_slice(), "block {n} cat {cat}");
        assert_eq!(nz as usize, l.iter().filter(|&&v| v != 0).count());
    }
    assert_eq!(d.terminate(), 1);
}

fn frame(t: usize) -> [Vec<u8>; 3] {
    let (w, h) = (48usize, 32usize);
    let mut p = [vec![0u8; w * h], vec![128u8; w * h / 4], vec![128u8; w * h / 4]];
    for y in 0..h {
        for x in 0..w {
            p[0][y * w + x] = ((x * 5 + y * 3 + t * 7) % 200 + 20) as u8;
        }
    }
    p
}

fn yuv(p: &[Vec<u8>; 3]) -> Yuv420<'_> {
    Yuv420 { width: 48, height: 32, y: &p[0], u: &p[1], v: &p[2], y_stride: 48, uv_stride: 24 }
}

#[test]
fn c_parameter_sets_read_back_main_cabac() {
    let mut enc = Encoder::new(Config { width: 48, height: 32, qp: 28, gop: 4, entropy: Entropy::Cabac, ..Default::default() }).unwrap();
    let f0 = frame(0);
    let idr = enc.encode(&yuv(&f0)).unwrap();
    let f1 = frame(1);
    let p = enc.encode(&yuv(&f1)).unwrap();
    let mut sps_rbsp = Vec::new();
    nal::unescape(&idr.nals[0][1..], &mut sps_rbsp);
    assert_eq!(sps_rbsp[1] & 0xC0, 0x40, "constraint_set1_flag alone (Main)");
    let sps = ps::parse_sps(&sps_rbsp).unwrap();
    assert_eq!(sps.profile_idc, 77);
    let mut pps_rbsp = Vec::new();
    nal::unescape(&idr.nals[1][1..], &mut pps_rbsp);
    let pps = ps::parse_pps(&pps_rbsp, |_| 1).unwrap();
    assert!(pps.cabac, "entropy_coding_mode_flag");
    assert!(!pps.transform_8x8_mode);
    let c = nal::parse_avcc(&enc.avcc()).unwrap();
    assert_eq!(c.profile_idc, 77);
    for (au, ty, st) in [(&idr, nal::NAL_IDR, SliceType::I), (&p, nal::NAL_SLICE, SliceType::P)] {
        let slice = au.nals.last().unwrap();
        let h = nal::parse_header(slice[0]).unwrap();
        assert_eq!(h.nal_type, ty);
        let mut rbsp = Vec::new();
        nal::unescape(&slice[1..], &mut rbsp);
        let mut r = BitReader::new(&rbsp);
        let sh = ps::parse_slice_header(&mut r, h.nal_type, h.ref_idc, |_| Some(&pps), |_| Some(&sps)).unwrap();
        assert_eq!((sh.slice_type, sh.cabac_init_idc, sh.qp), (st, 0, 28));
        // cabac_alignment_one_bit up to the byte boundary.
        for b in sh.data_bit..sh.data_bit.div_ceil(8) * 8 {
            assert_eq!((rbsp[b / 8] >> (7 - b % 8)) & 1, 1, "cabac_alignment_one_bit at bit {b}");
        }
    }
}
