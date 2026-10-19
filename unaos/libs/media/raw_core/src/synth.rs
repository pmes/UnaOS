// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! A SYNTHETIC Sony ARW, written the way the format lays one out (TIFF header; IFD0 = Make/Model/Orientation,
//! the full-size preview's JPEGInterchangeFormat, SubIFDs -> the raw IFD, the EXIF IFD; IFD1 = the thumbnail;
//! the raw IFD = CFA photometric, CFAPattern RGGB, Sony black/white, the strip). It is the known-answer file of
//! the host KATs and of `tests rawcore` until a real ARW from Peter's card replaces it — no number proven on it
//! is a claim about a real camera's bytes. The preview is lux's 632-byte `tiny_red.jpg`.

use alloc::vec;
use alloc::vec::Vec;

/// The embedded preview (a baseline JPEG).
pub const PREVIEW_JPEG: &[u8] = include_bytes!("synth_preview.jpg");

pub const MAKE: &str = "SONY";
pub const MODEL: &str = "ILCE-7M3";
pub const LENS: &str = "FE 28-70mm F3.5-5.6 OSS";
pub const TAKEN: &str = "2026:10:05 14:30:00";
/// `TAKEN` as unix seconds.
pub const TAKEN_UNIX: i64 = 1_791_210_600;
pub const ISO: u32 = 400;
pub const BLACK: u16 = 512;
pub const WHITE: u16 = 16383;

/// How the synthetic strip is coded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Coding {
    /// `Compression 1`, 14-bit samples in 16-bit little-endian containers.
    Plain14,
    /// `Compression 32767`, `BitsPerSample 8` — Sony cRAW of 11-bit codes (no curve tag: dcraw's default).
    Craw,
}

struct E {
    tag: u16,
    typ: u16,
    count: u32,
    data: Vec<u8>,
}

fn e16(tag: u16, v: &[u16]) -> E {
    E { tag, typ: 3, count: v.len() as u32, data: v.iter().flat_map(|x| x.to_le_bytes()).collect() }
}
fn e32(tag: u16, v: u32) -> E {
    E { tag, typ: 4, count: 1, data: v.to_le_bytes().to_vec() }
}
fn eascii(tag: u16, s: &str) -> E {
    let mut d = s.as_bytes().to_vec();
    d.push(0);
    E { tag, typ: 2, count: d.len() as u32, data: d }
}
fn erat(tag: u16, n: u32, d: u32) -> E {
    let mut b = n.to_le_bytes().to_vec();
    b.extend_from_slice(&d.to_le_bytes());
    E { tag, typ: 5, count: 1, data: b }
}
fn ebytes(tag: u16, v: &[u8]) -> E {
    E { tag, typ: 1, count: v.len() as u32, data: v.to_vec() }
}

/// Serialise IFDs (with their next pointers and out-of-line values) at `base`. Returns bytes and each IFD's offset.
fn emit(ifds: &mut [Vec<E>], next: &[Option<usize>], base: usize) -> (Vec<u8>, Vec<u32>) {
    let mut offs = Vec::new();
    let mut at = base;
    for i in ifds.iter_mut() {
        i.sort_by_key(|e| e.tag);
        offs.push(at as u32);
        at += 2 + 12 * i.len() + 4;
    }
    let mut ext: Vec<u8> = Vec::new();
    let ext_base = at;
    let mut out = Vec::new();
    for (k, i) in ifds.iter().enumerate() {
        out.extend_from_slice(&(i.len() as u16).to_le_bytes());
        for e in i {
            out.extend_from_slice(&e.tag.to_le_bytes());
            out.extend_from_slice(&e.typ.to_le_bytes());
            out.extend_from_slice(&e.count.to_le_bytes());
            if e.data.len() <= 4 {
                let mut v = e.data.clone();
                v.resize(4, 0);
                out.extend_from_slice(&v);
            } else {
                out.extend_from_slice(&((ext_base + ext.len()) as u32).to_le_bytes());
                ext.extend_from_slice(&e.data);
                if ext.len() % 2 == 1 {
                    ext.push(0);
                }
            }
        }
        out.extend_from_slice(&(next[k].map(|n| offs[n]).unwrap_or(0)).to_le_bytes());
    }
    out.extend_from_slice(&ext);
    (out, offs)
}

/// Encode one row of 11-bit codes as Sony cRAW (the inverse of [`crate::decode`]'s block reader). Exact when
/// each block's span is below 128 (`sh == 0`); otherwise deltas are truncated, as the camera's are.
pub fn craw_row(codes: &[u16]) -> Vec<u8> {
    let w = codes.len();
    let mut out = vec![0u8; w];
    let mut col = 0usize;
    let mut dp = 0usize;
    while col + 30 < w && dp + 16 <= w {
        let idx: Vec<usize> = (0..16).map(|i| col + 2 * i).collect();
        let px: Vec<u32> = idx.iter().map(|&c| codes.get(c).copied().unwrap_or(0) as u32 & 0x7ff).collect();
        let (mut imax, mut imin) = (0usize, 0usize);
        for i in 0..16 {
            if px[i] > px[imax] {
                imax = i;
            }
            if px[i] < px[imin] {
                imin = i;
            }
        }
        if imin == imax {
            imin = (imax + 1) % 16;
        }
        let (max, min) = (px[imax], px[imin]);
        let mut sh = 0u32;
        while sh < 4 && (0x80u32 << sh) <= max - min {
            sh += 1;
        }
        let mut bits: u128 = (max | min << 11 | (imax as u32) << 22 | (imin as u32) << 26) as u128;
        let mut bit = 30;
        for i in 0..16 {
            if i == imax || i == imin {
                continue;
            }
            let d = ((px[i] - min) >> sh) & 0x7f;
            bits |= (d as u128) << bit;
            bit += 7;
        }
        out[dp..dp + 16].copy_from_slice(&bits.to_le_bytes());
        // The decoder's walk: even columns of a span, then its odd columns.
        col += 32;
        col -= if col & 1 == 1 { 1 } else { 31 };
        dp += 16;
    }
    out
}

/// What a synthetic ARW carries beyond its strip (RAWVIEW, B711).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opts {
    /// The embedded JPEG (IFD0's and IFD1's 513/514). `false` = a raw with NO preview: Quick Look's half path.
    pub preview: bool,
    /// Sony's as-shot levels, R G G B (`WB_RGGBLevels`, 0x7313), in an SR2SubIFD encrypted with `key` and named
    /// by an SR2Private IFD that IFD0's DNGPrivateData (50740) points at.
    pub wb_rggb: Option<[u16; 4]>,
    pub key: u32,
}

/// The SR2 key the synthetic files use.
pub const SR2_KEY: u32 = 0x5EED_0A11;

impl Default for Opts {
    fn default() -> Self {
        Opts { preview: true, wb_rggb: None, key: SR2_KEY }
    }
}

/// A synthetic ARW of `w x h` (w a multiple of 32 for `Craw`) holding `samples` (14-bit for `Plain14`, 11-bit
/// codes for `Craw`), row-major — with the preview, no white balance ([`arw_with`] and [`Opts::default`]).
pub fn arw(w: u32, h: u32, samples: &[u16], coding: Coding) -> Vec<u8> {
    arw_with(w, h, samples, coding, Opts::default())
}

/// The plaintext SR2SubIFD at file offset `at`: one entry, `WB_RGGBLevels` (four SHORTs, out of line at `at + 18`).
fn sr2_block(at: u32, levels: [u16; 4]) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&crate::wb::tag::WB_RGGB.to_le_bytes());
    b.extend_from_slice(&3u16.to_le_bytes());
    b.extend_from_slice(&4u32.to_le_bytes());
    b.extend_from_slice(&(at + 18).to_le_bytes());
    b.extend_from_slice(&0u32.to_le_bytes());
    for l in levels {
        b.extend_from_slice(&l.to_le_bytes());
    }
    b.resize(28, 0);
    b
}

/// [`arw`] with [`Opts`].
pub fn arw_with(w: u32, h: u32, samples: &[u16], coding: Coding, o: Opts) -> Vec<u8> {
    let strip: Vec<u8> = match coding {
        Coding::Plain14 => samples.iter().flat_map(|v| v.to_le_bytes()).collect(),
        Coding::Craw => samples.chunks(w as usize).flat_map(craw_row).collect(),
    };
    let (bits, comp, black, white): (u16, u16, u16, u16) = match coding {
        Coding::Plain14 => (14, 1, BLACK, WHITE),
        Coding::Craw => (8, 32767, BLACK, WHITE),
    };
    let sr2_len = 28u32;
    let build = |jpeg_at: u32, strip_at: u32, sub: u32, exif: u32, sr2p: u32, sr2_at: u32| {
        let mut ifd0 = vec![e16(259, &[6]), eascii(271, MAKE), eascii(272, MODEL), e16(274, &[1]), e32(330, sub), e32(34665, exif)];
        let mut ifd1 = vec![e16(259, &[6])];
        if o.preview {
            ifd0.push(e32(513, jpeg_at));
            ifd0.push(e32(514, PREVIEW_JPEG.len() as u32));
            ifd1.push(e32(513, jpeg_at));
            ifd1.push(e32(514, PREVIEW_JPEG.len() as u32));
        }
        if o.wb_rggb.is_some() {
            ifd0.push(e32(crate::wb::tag::DNG_PRIVATE, sr2p));
        }
        let raw = vec![
            e32(256, w),
            e32(257, h),
            e16(258, &[bits]),
            e16(259, &[comp]),
            e16(262, &[32803]),
            e32(273, strip_at),
            e16(277, &[1]),
            e32(279, strip.len() as u32),
            e16(33421, &[2, 2]),
            ebytes(33422, &[0, 1, 1, 2]),
            e16(0x7310, &[black; 4]),
            e16(0x787f, &[white, white, white]),
        ];
        let exif = vec![
            erat(33434, 1, 250),
            erat(33437, 28, 10),
            e16(34855, &[ISO as u16]),
            eascii(36867, TAKEN),
            erat(37386, 35, 1),
            eascii(42036, LENS),
        ];
        if o.wb_rggb.is_some() {
            let sr2p = vec![e32(crate::wb::tag::SR2_OFFSET, sr2_at), e32(crate::wb::tag::SR2_LENGTH, sr2_len), e32(crate::wb::tag::SR2_KEY, o.key)];
            let mut ifds = [ifd0, ifd1, raw, exif, sr2p];
            emit(&mut ifds, &[Some(1), None, None, None, None], 8)
        } else {
            let mut ifds = [ifd0, ifd1, raw, exif];
            emit(&mut ifds, &[Some(1), None, None, None], 8)
        }
    };
    // Pass 1 sizes the head and places the IFDs; pass 2 writes the real offsets (sizes do not depend on values).
    let (head, offs) = build(0, 0, 0, 0, 0, 0);
    let jpeg_at = 8 + head.len() as u32;
    let after = jpeg_at + if o.preview { PREVIEW_JPEG.len() as u32 } else { 0 };
    let sr2_at = (after + 3) & !3;
    let tail = if o.wb_rggb.is_some() { sr2_at + sr2_len } else { after };
    let strip_at = (tail + 15) & !15;
    let (head2, _) = build(jpeg_at, strip_at, offs[2], offs[3], offs.get(4).copied().unwrap_or(0), sr2_at);
    debug_assert_eq!(head.len(), head2.len());
    let mut f = b"II\x2a\x00".to_vec();
    f.extend_from_slice(&8u32.to_le_bytes());
    f.extend_from_slice(&head2);
    if o.preview {
        f.extend_from_slice(PREVIEW_JPEG);
    }
    if let Some(levels) = o.wb_rggb {
        f.resize(sr2_at as usize, 0);
        let mut blk = sr2_block(sr2_at, levels);
        crate::wb::sony_decrypt(&mut blk, o.key);
        f.extend_from_slice(&blk);
    }
    f.resize(strip_at as usize, 0);
    f.extend_from_slice(&strip);
    f
}

/// A GREY scene under the light the levels name (R G G B, green = 1024 in [`crate::wb`]'s sense): every cell's
/// samples are `BLACK + GREY_G * 1024 / level`, so a correct white balance renders R = G = B. The KAT's levels
/// (2048, 1024, 1024, 1536) give R = 3000, G = 6000, B = 4000 above black: exact products after the gain.
pub fn grey(w: u32, h: u32, levels: [u16; 4]) -> Vec<u16> {
    let lv = [levels[0] as u32, ((levels[1] as u32 + levels[2] as u32) / 2).max(1), levels[3] as u32];
    let mut v = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let c = [0usize, 1, 1, 2][((y & 1) * 2 + (x & 1)) as usize];
            v.push(BLACK + (GREY_G * lv[1] / lv[c].max(1)) as u16);
        }
    }
    v
}

/// The grey scene's green sample above black.
pub const GREY_G: u32 = 6000;

/// The synthetic mosaic `tests rawcore` and the KATs use: a horizontal ramp per colour plane, 14-bit, above black.
pub fn ramp(w: u32, h: u32) -> Vec<u16> {
    let mut v = Vec::with_capacity((w * h) as usize);
    for y in 0..h {
        for x in 0..w {
            let c = [0u16, 1, 1, 2][((y & 1) * 2 + (x & 1)) as usize];
            v.push(BLACK + (x as u16) * 400 + c * 1000);
        }
    }
    v
}
