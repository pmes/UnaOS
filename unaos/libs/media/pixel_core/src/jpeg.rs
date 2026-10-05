// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! JPEG decoder — ITU-T T.81 (ISO/IEC 10918-1), JFIF 1.02, EXIF 2.3 orientation.
//!
//! Covered: the marker syntax (B.1); DQT 8- and 16-bit tables (B.2.4.1); DHT (B.2.4.2) with the
//! canonical code generation of Annex C; SOF0 baseline, SOF1 extended-sequential Huffman (8-bit
//! precision) and SOF2 progressive Huffman (B.2.2); SOS (B.2.3) interleaved and non-interleaved, with
//! the MCU / block ordering of A.2; DRI + RSTn restart intervals (B.2.4.4, F.1.2.3); sequential
//! Huffman decoding of DC/AC (F.2.2); progressive spectral selection AND successive approximation —
//! DC first/refine, AC first/refine with EOB runs (G.1.2); any sampling factors 1..4 (A.1.1).
//!
//! Reconstruction follows the IJG reference so a browser built on libjpeg-turbo is matched closely:
//!   * IDCT: the IJG "islow" integer algorithm (Loeffler–Ligtenberg–Moschytz, 13-bit constants, 2 extra
//!     bits through pass 1, final descale by 18 bits with rounding, clamp to 0..255). Its precision is
//!     that of IEEE 1180-1990's accuracy test, which islow passes; it is bit-exact with libjpeg-turbo's
//!     C and SIMD islow paths.
//!   * Chroma upsampling: libjpeg's "fancy" triangle filters for h2v1, h2v2 and h1v2; other ratios are
//!     replicated (box). Edge samples replicate (libjpeg's context rows).
//!   * Colour (JFIF §7 / T.871): YCbCr → RGB with libjpeg's 16-bit fixed-point tables. 1 component =
//!     greyscale. 3 components with an Adobe APP14 transform=0, or ids 'R','G','B' = RGB untouched.
//!     4 components = Adobe CMYK (transform 0) or YCCK (transform 2), stored inverted as Photoshop
//!     writes it, converted naively `R = C·K/255` (no ICC).
//!   * EXIF orientation (APP1 "Exif", TIFF IFD0 tag 0x0112) is READ into `Image::orientation`; it is
//!     applied only when the caller asks ([`crate::Image::apply_orientation`]).
//!
//! NOT decoded (refused by name): arithmetic coding (SOF9..11, SOF13..15), lossless (SOF3, SOF7, SOF11,
//! SOF15), hierarchical (SOF5..7, DHP/EXP), 12-bit precision, DNL-defined height. No ICC profile is
//! applied. A truncated scan is refused rather than shown partially.

use alloc::vec;
use alloc::vec::Vec;

use crate::{Error, Image};

/// Zig-zag index → natural (row-major) position (Figure A.6).
const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7, 14, 21,
    28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61,
    54, 47, 55, 62, 63,
];

const LOOKUP_BITS: u32 = 9;

// Annex K.3 typical Huffman tables — installed in slots 0 (luminance) and 1 (chrominance) before any
// DHT, as the IJG library does, so a Motion-JPEG frame (which omits DHT, AVI1) decodes.
const K3_DC_LUM_BITS: [u8; 16] = [0, 1, 5, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0, 0, 0];
const K3_DC_CHR_BITS: [u8; 16] = [0, 3, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0];
const K3_DC_VALS: [u8; 12] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11];
const K3_AC_LUM_BITS: [u8; 16] = [0, 2, 1, 3, 3, 2, 4, 3, 5, 5, 4, 4, 0, 0, 1, 0x7d];
const K3_AC_LUM_VALS: [u8; 162] = [
    0x01, 0x02, 0x03, 0x00, 0x04, 0x11, 0x05, 0x12, 0x21, 0x31, 0x41, 0x06, 0x13, 0x51, 0x61, 0x07, 0x22, 0x71,
    0x14, 0x32, 0x81, 0x91, 0xa1, 0x08, 0x23, 0x42, 0xb1, 0xc1, 0x15, 0x52, 0xd1, 0xf0, 0x24, 0x33, 0x62, 0x72,
    0x82, 0x09, 0x0a, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x25, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x34, 0x35, 0x36, 0x37,
    0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58, 0x59,
    0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a, 0x83,
    0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0xa2, 0xa3,
    0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba, 0xc2, 0xc3,
    0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda, 0xe1, 0xe2,
    0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
];
const K3_AC_CHR_BITS: [u8; 16] = [0, 2, 1, 2, 4, 4, 3, 4, 7, 5, 4, 4, 0, 1, 2, 0x77];
const K3_AC_CHR_VALS: [u8; 162] = [
    0x00, 0x01, 0x02, 0x03, 0x11, 0x04, 0x05, 0x21, 0x31, 0x06, 0x12, 0x41, 0x51, 0x07, 0x61, 0x71, 0x13, 0x22,
    0x32, 0x81, 0x08, 0x14, 0x42, 0x91, 0xa1, 0xb1, 0xc1, 0x09, 0x23, 0x33, 0x52, 0xf0, 0x15, 0x62, 0x72, 0xd1,
    0x0a, 0x16, 0x24, 0x34, 0xe1, 0x25, 0xf1, 0x17, 0x18, 0x19, 0x1a, 0x26, 0x27, 0x28, 0x29, 0x2a, 0x35, 0x36,
    0x37, 0x38, 0x39, 0x3a, 0x43, 0x44, 0x45, 0x46, 0x47, 0x48, 0x49, 0x4a, 0x53, 0x54, 0x55, 0x56, 0x57, 0x58,
    0x59, 0x5a, 0x63, 0x64, 0x65, 0x66, 0x67, 0x68, 0x69, 0x6a, 0x73, 0x74, 0x75, 0x76, 0x77, 0x78, 0x79, 0x7a,
    0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a,
    0xa2, 0xa3, 0xa4, 0xa5, 0xa6, 0xa7, 0xa8, 0xa9, 0xaa, 0xb2, 0xb3, 0xb4, 0xb5, 0xb6, 0xb7, 0xb8, 0xb9, 0xba,
    0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7, 0xc8, 0xc9, 0xca, 0xd2, 0xd3, 0xd4, 0xd5, 0xd6, 0xd7, 0xd8, 0xd9, 0xda,
    0xe2, 0xe3, 0xe4, 0xe5, 0xe6, 0xe7, 0xe8, 0xe9, 0xea, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa,
];

/// A Huffman table in decoding form (Annex C + F.2.2.3), with a `LOOKUP_BITS` fast table.
#[derive(Clone)]
struct Huffman {
    /// `(code length, value)` for every `LOOKUP_BITS`-bit prefix; length 0 = take the slow path.
    fast: Vec<(u8, u8)>,
    maxcode: [i32; 18],
    valptr: [i32; 17],
    mincode: [i32; 17],
    vals: Vec<u8>,
}

impl Huffman {
    fn new(bits: &[u8; 16], vals: &[u8]) -> Result<Self, Error> {
        let mut fast = vec![(0u8, 0u8); 1 << LOOKUP_BITS];
        let mut maxcode = [-1i32; 18];
        let mut valptr = [0i32; 17];
        let mut mincode = [0i32; 17];
        let mut code = 0i32;
        let mut k = 0usize;
        for l in 1..=16usize {
            let n = bits[l - 1] as usize;
            if n > 0 {
                valptr[l] = k as i32;
                mincode[l] = code;
                for _ in 0..n {
                    if l as u32 <= LOOKUP_BITS {
                        let shift = LOOKUP_BITS - l as u32;
                        let base = (code as usize) << shift;
                        for j in 0..(1usize << shift) {
                            if let Some(e) = fast.get_mut(base + j) {
                                *e = (l as u8, *vals.get(k).ok_or(Error::Malformed("DHT"))?);
                            }
                        }
                    }
                    code += 1;
                    k += 1;
                }
                maxcode[l] = code - 1;
                if code > (1 << l) {
                    return Err(Error::Malformed("DHT over-subscribed"));
                }
            }
            code <<= 1;
        }
        maxcode[17] = i32::MAX;
        if k > vals.len() {
            return Err(Error::Malformed("DHT"));
        }
        Ok(Huffman { fast, maxcode, valptr, mincode, vals: vals[..k].to_vec() })
    }
}

/// The entropy-coded-segment bit reader: removes 0xFF00 stuffing, stops at a marker and then feeds
/// zero bits (F.2.2.5's behaviour at a marker).
struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    buf: u64,
    n: u32,
    at_marker: bool,
}

impl<'a> Bits<'a> {
    fn new(d: &'a [u8], pos: usize) -> Self {
        Bits { d, pos, buf: 0, n: 0, at_marker: false }
    }

    #[inline]
    fn fill(&mut self) {
        while self.n <= 56 {
            let mut b = 0u8;
            if !self.at_marker {
                if self.pos >= self.d.len() {
                    self.at_marker = true;
                } else {
                    b = self.d[self.pos];
                    if b == 0xFF {
                        let nx = self.d.get(self.pos + 1).copied().unwrap_or(0xD9);
                        if nx == 0x00 {
                            self.pos += 2;
                        } else {
                            self.at_marker = true;
                            b = 0;
                        }
                    } else {
                        self.pos += 1;
                    }
                }
            }
            self.buf |= (b as u64) << (56 - self.n);
            self.n += 8;
        }
    }

    #[inline]
    fn bits(&mut self, k: u32) -> u32 {
        if k == 0 {
            return 0;
        }
        if self.n < k {
            self.fill();
        }
        let v = (self.buf >> (64 - k)) as u32;
        self.buf <<= k;
        self.n -= k;
        v
    }

    #[inline]
    fn bit(&mut self) -> u32 {
        self.bits(1)
    }

    /// Receive `s` bits and sign-extend them (F.2.2.1 EXTEND).
    #[inline]
    fn receive_extend(&mut self, s: u32) -> i32 {
        if s == 0 {
            return 0;
        }
        let v = self.bits(s) as i32;
        if v < (1 << (s - 1)) { v - (1 << s) + 1 } else { v }
    }

    #[inline]
    fn decode(&mut self, h: &Huffman) -> Result<u8, Error> {
        if self.n < 16 {
            self.fill();
        }
        let peek = (self.buf >> (64 - LOOKUP_BITS)) as usize;
        let (l, v) = h.fast[peek];
        if l != 0 {
            self.buf <<= l;
            self.n -= l as u32;
            return Ok(v);
        }
        let p16 = (self.buf >> 48) as i32;
        for l in (LOOKUP_BITS as usize + 1)..=16 {
            let code = p16 >> (16 - l);
            if code <= h.maxcode[l] {
                self.buf <<= l;
                self.n -= l as u32;
                let i = h.valptr[l] + code - h.mincode[l];
                return h.vals.get(i as usize).copied().ok_or(Error::Malformed("huffman code"));
            }
        }
        Err(Error::Malformed("huffman code"))
    }

    /// Byte position of the first byte not yet handed to the bit buffer — at a marker, the 0xFF.
    fn reset(&mut self) {
        self.buf = 0;
        self.n = 0;
        self.at_marker = false;
    }
}

#[derive(Clone, Default)]
struct Component {
    id: u8,
    h: usize,
    v: usize,
    tq: usize,
    /// Blocks per line / column covering the component (A.2.2's non-interleaved count).
    bw: usize,
    bh: usize,
    /// Blocks per line / column padded to whole MCUs (storage).
    pbw: usize,
    pbh: usize,
    /// Downsampled dimensions in samples (A.1.1).
    cw: usize,
    ch: usize,
    coefs: Vec<i16>,
    dc_pred: i32,
    td: usize,
    ta: usize,
}

struct Frame {
    width: usize,
    height: usize,
    progressive: bool,
    comps: Vec<Component>,
    hmax: usize,
    vmax: usize,
    mcux: usize,
    mcuy: usize,
}

fn be16(b: &[u8], i: usize) -> Result<usize, Error> {
    if i + 2 > b.len() {
        return Err(Error::Truncated);
    }
    Ok(((b[i] as usize) << 8) | b[i + 1] as usize)
}

/// Decode a JPEG file to RGBA8.
pub fn decode(data: &[u8]) -> Result<Image, Error> {
    if data.len() < 4 || data[0] != 0xFF || data[1] != 0xD8 {
        return Err(Error::Malformed("jpeg SOI"));
    }
    let mut qt = [[0u16; 64]; 4];
    let mut dc_tabs: [Option<Huffman>; 4] = [
        Some(Huffman::new(&K3_DC_LUM_BITS, &K3_DC_VALS)?),
        Some(Huffman::new(&K3_DC_CHR_BITS, &K3_DC_VALS)?),
        None,
        None,
    ];
    let mut ac_tabs: [Option<Huffman>; 4] = [
        Some(Huffman::new(&K3_AC_LUM_BITS, &K3_AC_LUM_VALS)?),
        Some(Huffman::new(&K3_AC_CHR_BITS, &K3_AC_CHR_VALS)?),
        None,
        None,
    ];
    let mut frame: Option<Frame> = None;
    let mut restart = 0usize;
    let mut adobe: Option<u8> = None;
    let mut jfif = false;
    let mut orientation = 1u8;
    let mut pos = 2usize;
    let mut eoi = false;

    loop {
        // Find the next marker (fill bytes 0xFF are legal padding, B.1.1.2).
        while pos < data.len() && data[pos] != 0xFF {
            pos += 1;
        }
        while pos < data.len() && data[pos] == 0xFF {
            pos += 1;
        }
        if pos >= data.len() {
            break;
        }
        let m = data[pos];
        pos += 1;
        match m {
            0xD8 | 0x01 | 0xD0..=0xD7 => continue,
            0xD9 => {
                eoi = true;
                break;
            }
            _ => {}
        }
        let len = be16(data, pos)?;
        if len < 2 || pos + len > data.len() {
            return Err(Error::Truncated);
        }
        let seg = &data[pos + 2..pos + len];
        pos += len;
        match m {
            0xDB => {
                let mut i = 0;
                while i < seg.len() {
                    let pq = seg[i] >> 4;
                    let tq = (seg[i] & 15) as usize;
                    if tq > 3 {
                        return Err(Error::Malformed("DQT id"));
                    }
                    i += 1;
                    for k in 0..64 {
                        let v = if pq == 0 {
                            *seg.get(i + k).ok_or(Error::Truncated)? as u16
                        } else {
                            be16(seg, i + 2 * k)? as u16
                        };
                        qt[tq][ZIGZAG[k]] = v;
                    }
                    i += if pq == 0 { 64 } else { 128 };
                }
            }
            0xC4 => {
                let mut i = 0;
                while i < seg.len() {
                    if i + 17 > seg.len() {
                        return Err(Error::Truncated);
                    }
                    let tc = seg[i] >> 4;
                    let th = (seg[i] & 15) as usize;
                    if th > 3 || tc > 1 {
                        return Err(Error::Malformed("DHT id"));
                    }
                    let mut bits = [0u8; 16];
                    bits.copy_from_slice(&seg[i + 1..i + 17]);
                    let n: usize = bits.iter().map(|&b| b as usize).sum();
                    if i + 17 + n > seg.len() {
                        return Err(Error::Truncated);
                    }
                    let t = Huffman::new(&bits, &seg[i + 17..i + 17 + n])?;
                    if tc == 0 {
                        dc_tabs[th] = Some(t);
                    } else {
                        ac_tabs[th] = Some(t);
                    }
                    i += 17 + n;
                }
            }
            0xDD => restart = be16(seg, 0)?,
            0xE0 => {
                if seg.starts_with(b"JFIF\0") {
                    jfif = true;
                }
            }
            0xE1 => {
                if seg.starts_with(b"Exif\0\0") {
                    if let Some(o) = exif_orientation(&seg[6..]) {
                        orientation = o;
                    }
                }
            }
            0xEE => {
                if seg.len() >= 12 && seg.starts_with(b"Adobe") {
                    adobe = Some(seg[11]);
                }
            }
            0xC0 | 0xC1 | 0xC2 => {
                if frame.is_some() {
                    return Err(Error::Malformed("second SOF"));
                }
                frame = Some(parse_sof(seg, m == 0xC2)?);
            }
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                return Err(Error::Unsupported(match m {
                    0xC3 | 0xC7 | 0xCB | 0xCF => "jpeg lossless",
                    0xC9..=0xCB | 0xCD..=0xCF => "jpeg arithmetic coding",
                    _ => "jpeg hierarchical",
                }));
            }
            0xDA => {
                let f = frame.as_mut().ok_or(Error::Malformed("SOS before SOF"))?;
                pos = decode_scan(data, pos, seg, f, &dc_tabs, &ac_tabs, restart)?;
            }
            0xDC => return Err(Error::Unsupported("jpeg DNL")),
            _ => {} // APPn, COM, JPGn: skipped.
        }
    }
    let _ = eoi; // a missing EOI is tolerated once every scan has been read.
    let f = frame.ok_or(Error::Malformed("no SOF"))?;
    let mut img = reconstruct(&f, &qt, adobe, jfif)?;
    img.orientation = orientation;
    Ok(img)
}

fn parse_sof(seg: &[u8], progressive: bool) -> Result<Frame, Error> {
    if seg.len() < 6 {
        return Err(Error::Truncated);
    }
    if seg[0] != 8 {
        return Err(Error::Unsupported("jpeg sample precision other than 8"));
    }
    let height = be16(seg, 1)?;
    let width = be16(seg, 3)?;
    let nc = seg[5] as usize;
    if height == 0 {
        return Err(Error::Unsupported("jpeg DNL height"));
    }
    if !(nc == 1 || nc == 3 || nc == 4) || seg.len() < 6 + 3 * nc {
        return Err(Error::Malformed("SOF components"));
    }
    crate::rgba_len(width as u32, height as u32)?;
    let mut comps = Vec::new();
    for i in 0..nc {
        let b = &seg[6 + 3 * i..9 + 3 * i];
        let (h, v) = ((b[1] >> 4) as usize, (b[1] & 15) as usize);
        if !(1..=4).contains(&h) || !(1..=4).contains(&v) || b[2] > 3 {
            return Err(Error::Malformed("SOF sampling/table"));
        }
        comps.push(Component { id: b[0], h, v, tq: b[2] as usize, ..Default::default() });
    }
    let hmax = comps.iter().map(|c| c.h).max().unwrap();
    let vmax = comps.iter().map(|c| c.v).max().unwrap();
    let mcux = width.div_ceil(8 * hmax);
    let mcuy = height.div_ceil(8 * vmax);
    for c in comps.iter_mut() {
        c.cw = (width * c.h).div_ceil(hmax);
        c.ch = (height * c.v).div_ceil(vmax);
        c.bw = c.cw.div_ceil(8);
        c.bh = c.ch.div_ceil(8);
        c.pbw = mcux * c.h;
        c.pbh = mcuy * c.v;
        let n = c.pbw * c.pbh * 64;
        let mut v = Vec::new();
        v.try_reserve_exact(n).map_err(|_| Error::OutOfMemory)?;
        v.resize(n, 0i16);
        c.coefs = v;
    }
    Ok(Frame { width, height, progressive, comps, hmax, vmax, mcux, mcuy })
}

/// Decode one scan's entropy-coded data. Returns the byte position of the marker that ends it.
#[allow(clippy::too_many_arguments)]
fn decode_scan(
    data: &[u8],
    start: usize,
    seg: &[u8],
    f: &mut Frame,
    dc_tabs: &[Option<Huffman>; 4],
    ac_tabs: &[Option<Huffman>; 4],
    restart: usize,
) -> Result<usize, Error> {
    let ns = *seg.first().ok_or(Error::Truncated)? as usize;
    if ns == 0 || ns > 4 || seg.len() < 1 + 2 * ns + 3 {
        return Err(Error::Malformed("SOS"));
    }
    let mut sc: Vec<usize> = Vec::new();
    for i in 0..ns {
        let cid = seg[1 + 2 * i];
        let t = seg[2 + 2 * i];
        let ci = f.comps.iter().position(|c| c.id == cid).ok_or(Error::Malformed("SOS component"))?;
        f.comps[ci].td = (t >> 4) as usize & 3;
        f.comps[ci].ta = (t & 15) as usize & 3;
        sc.push(ci);
    }
    let ss = seg[1 + 2 * ns] as usize;
    let se = seg[2 + 2 * ns] as usize;
    let ah = (seg[3 + 2 * ns] >> 4) as u32;
    let al = (seg[3 + 2 * ns] & 15) as u32;
    if f.progressive {
        if se > 63 || ss > se || (ss == 0 && se != 0) || (ss > 0 && ns != 1) || al > 13 {
            return Err(Error::Malformed("SOS progression"));
        }
    } else if ss != 0 || se != 63 || ah != 0 || al != 0 {
        // Sequential: the parameters are fixed (B.2.3); be lenient about Se as libjpeg is.
    }
    let (ss, se) = if f.progressive { (ss, se) } else { (0, 63) };
    for &ci in &sc {
        let c = &f.comps[ci];
        let need_dc = ss == 0 && ah == 0;
        let need_ac = ss > 0 || !f.progressive;
        if need_dc && dc_tabs[c.td].is_none() || need_ac && ac_tabs[c.ta].is_none() {
            return Err(Error::Malformed("missing huffman table"));
        }
    }
    for c in f.comps.iter_mut() {
        c.dc_pred = 0;
    }

    let mut br = Bits::new(data, start);
    let mut eobrun = 0u32;
    let single = ns == 1;
    let (units_x, units_y) = if single {
        let c = &f.comps[sc[0]];
        (c.bw, c.bh)
    } else {
        (f.mcux, f.mcuy)
    };
    let total = units_x * units_y;
    let empty = Huffman { fast: Vec::new(), maxcode: [0; 18], valptr: [0; 17], mincode: [0; 17], vals: Vec::new() };

    let decode_block = |c: &mut Component, br: &mut Bits, eobrun: &mut u32, bx: usize, by: usize| -> Result<(), Error> {
        let off = (by * c.pbw + bx) * 64;
        let blk = &mut c.coefs[off..off + 64];
        let dct = dc_tabs[c.td].as_ref().unwrap_or(&empty);
        let act = ac_tabs[c.ta].as_ref().unwrap_or(&empty);
        if !f.progressive {
            // F.2.2.1 / F.2.2.2
            let t = br.decode(dct)? as u32;
            if t > 11 {
                return Err(Error::Malformed("DC magnitude"));
            }
            c.dc_pred += br.receive_extend(t);
            blk[0] = c.dc_pred as i16;
            let mut k = 1;
            while k < 64 {
                let rs = br.decode(act)?;
                let (r, s) = ((rs >> 4) as usize, (rs & 15) as u32);
                if s == 0 {
                    if r == 15 {
                        k += 16;
                        continue;
                    }
                    break;
                }
                k += r;
                if k > 63 {
                    return Err(Error::Malformed("AC run past 63"));
                }
                blk[ZIGZAG[k]] = br.receive_extend(s) as i16;
                k += 1;
            }
            return Ok(());
        }
        if ss == 0 {
            // G.1.2.1 DC
            if ah == 0 {
                let t = br.decode(dct)? as u32;
                if t > 11 {
                    return Err(Error::Malformed("DC magnitude"));
                }
                c.dc_pred += br.receive_extend(t);
                blk[0] = (c.dc_pred << al) as i16;
            } else if br.bit() != 0 {
                blk[0] |= (1 << al) as i16;
            }
            return Ok(());
        }
        if ah == 0 {
            // G.1.2.2 AC first
            if *eobrun > 0 {
                *eobrun -= 1;
                return Ok(());
            }
            let mut k = ss;
            while k <= se {
                let rs = br.decode(act)?;
                let (r, s) = ((rs >> 4) as usize, (rs & 15) as u32);
                if s != 0 {
                    k += r;
                    if k > 63 {
                        return Err(Error::Malformed("AC run past 63"));
                    }
                    blk[ZIGZAG[k]] = (br.receive_extend(s) * (1 << al)) as i16;
                } else if r == 15 {
                    k += 15;
                } else {
                    *eobrun = (1u32 << r) - 1;
                    if r > 0 {
                        *eobrun += br.bits(r as u32);
                    }
                    break;
                }
                k += 1;
            }
            return Ok(());
        }
        // G.1.2.3 AC refinement (the IJG procedure).
        let p1: i16 = 1 << al;
        let m1: i16 = -1i16 << al;
        let mut k = ss;
        if *eobrun == 0 {
            while k <= se {
                let rs = br.decode(act)?;
                let mut r = (rs >> 4) as i32;
                let s = (rs & 15) as u32;
                let mut val: i16 = 0;
                if s != 0 {
                    if s != 1 {
                        return Err(Error::Malformed("AC refine magnitude"));
                    }
                    val = if br.bit() != 0 { p1 } else { m1 };
                } else if r != 15 {
                    *eobrun = 1u32 << r;
                    if r > 0 {
                        *eobrun += br.bits(r as u32);
                    }
                    break;
                }
                while k <= se {
                    let z = ZIGZAG[k];
                    if blk[z] != 0 {
                        if br.bit() != 0 && (blk[z] & p1) == 0 {
                            blk[z] += if blk[z] >= 0 { p1 } else { m1 };
                        }
                    } else {
                        r -= 1;
                        if r < 0 {
                            break;
                        }
                    }
                    k += 1;
                }
                if val != 0 && k <= se {
                    blk[ZIGZAG[k]] = val;
                }
                k += 1;
            }
        }
        if *eobrun > 0 {
            while k <= se {
                let z = ZIGZAG[k];
                if blk[z] != 0 && br.bit() != 0 && (blk[z] & p1) == 0 {
                    blk[z] += if blk[z] >= 0 { p1 } else { m1 };
                }
                k += 1;
            }
            *eobrun -= 1;
        }
        Ok(())
    };

    let mut done = 0usize;
    while done < total {
        if restart > 0 && done > 0 && done % restart == 0 {
            // F.1.2.3: byte-align, consume RSTn, reset predictors and the EOB run.
            br.reset();
            let mut p = br.pos;
            while p + 1 < data.len() && !(data[p] == 0xFF && (0xD0..=0xD7).contains(&data[p + 1])) {
                if data[p] == 0xFF && data[p + 1] != 0 && data[p + 1] != 0xFF {
                    break; // some other marker: the stream is short; carry on with zeros.
                }
                p += 1;
            }
            if p + 1 < data.len() && data[p] == 0xFF && (0xD0..=0xD7).contains(&data[p + 1]) {
                p += 2;
            }
            br.pos = p;
            eobrun = 0;
            for c in f.comps.iter_mut() {
                c.dc_pred = 0;
            }
        }
        let (ux, uy) = (done % units_x, done / units_x);
        if single {
            decode_block(&mut f.comps[sc[0]], &mut br, &mut eobrun, ux, uy)?;
        } else {
            for &ci in &sc {
                let (h, v) = (f.comps[ci].h, f.comps[ci].v);
                for y in 0..v {
                    for x in 0..h {
                        decode_block(&mut f.comps[ci], &mut br, &mut eobrun, ux * h + x, uy * v + y)?;
                    }
                }
            }
        }
        done += 1;
    }
    // Find the marker that ends the scan (skip any RSTn the last interval left behind).
    let mut p = br.pos;
    while p + 1 < data.len() {
        if data[p] == 0xFF && data[p + 1] != 0 && data[p + 1] != 0xFF && !(0xD0..=0xD7).contains(&data[p + 1]) {
            break;
        }
        p += 1;
    }
    if p + 1 >= data.len() && br.at_marker && br.pos >= data.len() {
        // Ran off the end of the file inside the scan.
        return Err(Error::Truncated);
    }
    Ok(p)
}

// ── Reconstruction ──────────────────────────────────────────────────────────────────────────────

const CONST_BITS: i32 = 13;
const PASS1_BITS: i32 = 2;
const FIX_0_298631336: i32 = 2446;
const FIX_0_390180644: i32 = 3196;
const FIX_0_541196100: i32 = 4433;
const FIX_0_765366865: i32 = 6270;
const FIX_0_899976223: i32 = 7373;
const FIX_1_175875602: i32 = 9633;
const FIX_1_501321110: i32 = 12299;
const FIX_1_847759065: i32 = 15137;
const FIX_1_961570560: i32 = 16069;
const FIX_2_053119869: i32 = 16819;
const FIX_2_562915447: i32 = 20995;
const FIX_3_072711026: i32 = 25172;

#[inline]
fn descale(x: i32, n: i32) -> i32 {
    (x + (1 << (n - 1))) >> n
}

/// The IJG islow inverse DCT of one dequantized block, writing 8x8 samples at `out[stride]`.
fn idct_islow(coef: &[i16], q: &[u16; 64], out: &mut [u8], stride: usize) {
    let mut ws = [0i32; 64];
    for col in 0..8 {
        let dq = |r: usize| coef[r * 8 + col] as i32 * q[r * 8 + col] as i32;
        if (1..8).all(|r| coef[r * 8 + col] == 0) {
            let dc = dq(0) << PASS1_BITS;
            for r in 0..8 {
                ws[r * 8 + col] = dc;
            }
            continue;
        }
        let (z2, z3) = (dq(2), dq(6));
        let z1 = (z2 + z3) * FIX_0_541196100;
        let tmp2 = z1 + z3 * -FIX_1_847759065;
        let tmp3 = z1 + z2 * FIX_0_765366865;
        let (z2, z3) = (dq(0), dq(4));
        let tmp0 = (z2 + z3) << CONST_BITS;
        let tmp1 = (z2 - z3) << CONST_BITS;
        let (tmp10, tmp13, tmp11, tmp12) = (tmp0 + tmp3, tmp0 - tmp3, tmp1 + tmp2, tmp1 - tmp2);
        let (t0, t1, t2, t3) = odd(dq(7), dq(5), dq(3), dq(1));
        let n = CONST_BITS - PASS1_BITS;
        ws[col] = descale(tmp10 + t3, n);
        ws[7 * 8 + col] = descale(tmp10 - t3, n);
        ws[8 + col] = descale(tmp11 + t2, n);
        ws[6 * 8 + col] = descale(tmp11 - t2, n);
        ws[2 * 8 + col] = descale(tmp12 + t1, n);
        ws[5 * 8 + col] = descale(tmp12 - t1, n);
        ws[3 * 8 + col] = descale(tmp13 + t0, n);
        ws[4 * 8 + col] = descale(tmp13 - t0, n);
    }
    let clamp = |v: i32| -> u8 { (v + 128).clamp(0, 255) as u8 };
    for row in 0..8 {
        let w = &ws[row * 8..row * 8 + 8];
        let o = &mut out[row * stride..row * stride + 8];
        let n = CONST_BITS + PASS1_BITS + 3;
        if w[1..].iter().all(|&v| v == 0) {
            let v = clamp(descale(w[0], PASS1_BITS + 3));
            o.fill(v);
            continue;
        }
        let (z2, z3) = (w[2], w[6]);
        let z1 = (z2 + z3) * FIX_0_541196100;
        let tmp2 = z1 + z3 * -FIX_1_847759065;
        let tmp3 = z1 + z2 * FIX_0_765366865;
        let tmp0 = (w[0] + w[4]) << CONST_BITS;
        let tmp1 = (w[0] - w[4]) << CONST_BITS;
        let (tmp10, tmp13, tmp11, tmp12) = (tmp0 + tmp3, tmp0 - tmp3, tmp1 + tmp2, tmp1 - tmp2);
        let (t0, t1, t2, t3) = odd(w[7], w[5], w[3], w[1]);
        o[0] = clamp(descale(tmp10 + t3, n));
        o[7] = clamp(descale(tmp10 - t3, n));
        o[1] = clamp(descale(tmp11 + t2, n));
        o[6] = clamp(descale(tmp11 - t2, n));
        o[2] = clamp(descale(tmp12 + t1, n));
        o[5] = clamp(descale(tmp12 - t1, n));
        o[3] = clamp(descale(tmp13 + t0, n));
        o[4] = clamp(descale(tmp13 - t0, n));
    }
}

/// The islow odd part over inputs (x7, x5, x3, x1).
#[inline]
fn odd(t0: i32, t1: i32, t2: i32, t3: i32) -> (i32, i32, i32, i32) {
    let z1 = t0 + t3;
    let z2 = t1 + t2;
    let z3 = t0 + t2;
    let z4 = t1 + t3;
    let z5 = (z3 + z4) * FIX_1_175875602;
    let t0 = t0 * FIX_0_298631336;
    let t1 = t1 * FIX_2_053119869;
    let t2 = t2 * FIX_3_072711026;
    let t3 = t3 * FIX_1_501321110;
    let z1 = z1 * -FIX_0_899976223;
    let z2 = z2 * -FIX_2_562915447;
    let z3 = z3 * -FIX_1_961570560 + z5;
    let z4 = z4 * -FIX_0_390180644 + z5;
    (t0 + z1 + z3, t1 + z2 + z4, t2 + z2 + z3, t3 + z1 + z4)
}

/// One component's sample plane (padded to whole blocks), after the IDCT.
fn plane(c: &Component, q: &[u16; 64]) -> Result<Vec<u8>, Error> {
    let stride = c.pbw * 8;
    let mut out = crate::zeroed(stride * c.pbh * 8)?;
    for by in 0..c.bh {
        for bx in 0..c.bw {
            let off = (by * c.pbw + bx) * 64;
            idct_islow(&c.coefs[off..off + 64], q, &mut out[by * 8 * stride + bx * 8..], stride);
        }
    }
    Ok(out)
}

/// Upsample a component plane to full resolution (`width x height`), libjpeg-fancy where it has a
/// fancy filter, replicated otherwise.
fn upsample(p: &[u8], c: &Component, f: &Frame) -> Result<Vec<u8>, Error> {
    let (w, h) = (f.width, f.height);
    let stride = c.pbw * 8;
    let (cw, ch) = (c.cw, c.ch);
    let (fx, fy) = (f.hmax / c.h, f.vmax / c.v);
    let mut out = crate::zeroed(w * h)?;
    let at = |x: usize, y: usize| -> i32 { p[y.min(ch - 1) * stride + x.min(cw - 1)] as i32 };
    if f.hmax % c.h != 0 || f.vmax % c.v != 0 {
        // Non-integral ratios (legal but exotic): nearest sample.
        for y in 0..h {
            for x in 0..w {
                out[y * w + x] = at(x * c.h / f.hmax, y * c.v / f.vmax) as u8;
            }
        }
        return Ok(out);
    }
    match (fx, fy) {
        (1, 1) => {
            for y in 0..h {
                out[y * w..y * w + w].copy_from_slice(&p[y * stride..y * stride + w]);
            }
        }
        (2, 1) => {
            // h2v1_fancy_upsample
            let mut row = vec![0u8; cw * 2];
            for y in 0..h {
                h2_row(&mut row, cw, |x| at(x, y), |v, _| v);
                out[y * w..y * w + w].copy_from_slice(&row[..w]);
            }
        }
        (1, 2) => {
            // h1v2_fancy_upsample: bias 1 on the upper output row, 2 on the lower.
            for y in 0..h {
                let sy = y / 2;
                let ny = if y % 2 == 0 { sy.saturating_sub(1) } else { (sy + 1).min(ch - 1) };
                let bias = if y % 2 == 0 { 1 } else { 2 };
                for x in 0..w {
                    out[y * w + x] = ((at(x, sy) * 3 + at(x, ny) + bias) >> 2) as u8;
                }
            }
        }
        (2, 2) => {
            // h2v2_fancy_upsample: vertical 3:1 column sums, then horizontal 3:1 with biases 8/7.
            let mut sums = vec![0i32; cw];
            let mut row = vec![0u8; cw * 2];
            for y in 0..h {
                let sy = y / 2;
                let ny = if y % 2 == 0 { sy.saturating_sub(1) } else { (sy + 1).min(ch - 1) };
                for (x, s) in sums.iter_mut().enumerate() {
                    *s = at(x, sy) * 3 + at(x, ny);
                }
                h2v2_row(&mut row, &sums);
                out[y * w..y * w + w].copy_from_slice(&row[..w]);
            }
        }
        _ => {
            for y in 0..h {
                for x in 0..w {
                    out[y * w + x] = at(x / fx, y / fy) as u8;
                }
            }
        }
    }
    Ok(out)
}

/// libjpeg h2v1 fancy row: out[2i] = (3·in[i] + in[i-1] + 1) >> 2, out[2i+1] = (3·in[i] + in[i+1] + 2) >> 2,
/// with the end samples copied.
fn h2_row(row: &mut [u8], cw: usize, src: impl Fn(usize) -> i32, _m: impl Fn(i32, usize) -> i32) {
    if cw == 1 {
        let v = src(0) as u8;
        row[0] = v;
        row[1] = v;
        return;
    }
    let v = src(0);
    row[0] = v as u8;
    row[1] = ((v * 3 + src(1) + 2) >> 2) as u8;
    for i in 1..cw - 1 {
        let v = src(i) * 3;
        row[2 * i] = ((v + src(i - 1) + 1) >> 2) as u8;
        row[2 * i + 1] = ((v + src(i + 1) + 2) >> 2) as u8;
    }
    let i = cw - 1;
    let v = src(i);
    row[2 * i] = ((v * 3 + src(i - 1) + 1) >> 2) as u8;
    row[2 * i + 1] = v as u8;
}

/// libjpeg h2v2 fancy row over precomputed column sums.
fn h2v2_row(row: &mut [u8], s: &[i32]) {
    let cw = s.len();
    if cw == 1 {
        row[0] = ((s[0] * 4 + 8) >> 4) as u8;
        row[1] = ((s[0] * 4 + 7) >> 4) as u8;
        return;
    }
    row[0] = ((s[0] * 4 + 8) >> 4) as u8;
    row[1] = ((s[0] * 3 + s[1] + 7) >> 4) as u8;
    for i in 1..cw - 1 {
        row[2 * i] = ((s[i] * 3 + s[i - 1] + 8) >> 4) as u8;
        row[2 * i + 1] = ((s[i] * 3 + s[i + 1] + 7) >> 4) as u8;
    }
    let i = cw - 1;
    row[2 * i] = ((s[i] * 3 + s[i - 1] + 8) >> 4) as u8;
    row[2 * i + 1] = ((s[i] * 4 + 7) >> 4) as u8;
}

/// FIX(x) at libjpeg's 16 SCALEBITS.
const fn fix16(x: f64) -> i32 {
    (x * 65536.0 + 0.5) as i32
}

fn reconstruct(f: &Frame, qt: &[[u16; 64]; 4], adobe: Option<u8>, jfif: bool) -> Result<Image, Error> {
    let mut planes = Vec::new();
    for c in &f.comps {
        let p = plane(c, &qt[c.tq])?;
        planes.push(upsample(&p, c, f)?);
    }
    let n = f.width * f.height;
    let mut rgba = crate::zeroed(n * 4)?;
    let nc = f.comps.len();
    // JFIF/T.871: three components are YCbCr unless an Adobe marker says transform 0 or the ids
    // spell 'R','G','B'.
    let rgb_ids = nc == 3 && f.comps[0].id == b'R' && f.comps[1].id == b'G' && f.comps[2].id == b'B';
    let ycc = nc == 3 && !rgb_ids && (jfif || adobe != Some(0));
    let half = 1 << 15;
    for i in 0..n {
        let o = &mut rgba[i * 4..i * 4 + 4];
        match nc {
            1 => {
                let g = planes[0][i];
                o.copy_from_slice(&[g, g, g, 255]);
            }
            3 if ycc => {
                let y = planes[0][i] as i32;
                let cb = planes[1][i] as i32 - 128;
                let cr = planes[2][i] as i32 - 128;
                let r = y + ((fix16(1.40200) * cr + half) >> 16);
                let g = y + ((-fix16(0.34414) * cb - fix16(0.71414) * cr + half) >> 16);
                let b = y + ((fix16(1.77200) * cb + half) >> 16);
                o.copy_from_slice(&[r.clamp(0, 255) as u8, g.clamp(0, 255) as u8, b.clamp(0, 255) as u8, 255]);
            }
            3 => o.copy_from_slice(&[planes[0][i], planes[1][i], planes[2][i], 255]),
            _ => {
                let (mut c, mut m, mut y) = (planes[0][i] as i32, planes[1][i] as i32, planes[2][i] as i32);
                let k = planes[3][i] as i32;
                if adobe == Some(2) {
                    // YCCK: the first three are YCbCr of the inverted CMY.
                    let (yy, cb, cr) = (c, m - 128, y - 128);
                    let r = yy + ((fix16(1.40200) * cr + half) >> 16);
                    let g = yy + ((-fix16(0.34414) * cb - fix16(0.71414) * cr + half) >> 16);
                    let b = yy + ((fix16(1.77200) * cb + half) >> 16);
                    c = 255 - r.clamp(0, 255);
                    m = 255 - g.clamp(0, 255);
                    y = 255 - b.clamp(0, 255);
                }
                if adobe.is_some() {
                    // Adobe stores CMYK inverted: the sample is 255 - ink. R = iC·iK/255, truncated
                    // as Blink's JPEG decoder does it.
                    o.copy_from_slice(&[
                        (c * k / 255) as u8,
                        (m * k / 255) as u8,
                        (y * k / 255) as u8,
                        255,
                    ]);
                } else {
                    o.copy_from_slice(&[
                        (((255 - c) * (255 - k) + 127) / 255) as u8,
                        (((255 - m) * (255 - k) + 127) / 255) as u8,
                        (((255 - y) * (255 - k) + 127) / 255) as u8,
                        255,
                    ]);
                }
            }
        }
    }
    Ok(Image::still(f.width as u32, f.height as u32, rgba))
}

/// The EXIF orientation tag (0x0112) from IFD0 of a TIFF structure (EXIF 2.3 §4.5.2, §4.6.4).
fn exif_orientation(t: &[u8]) -> Option<u8> {
    if t.len() < 8 {
        return None;
    }
    let le = match &t[0..2] {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let u16_at = |i: usize| -> Option<u16> {
        let b = t.get(i..i + 2)?;
        Some(if le { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) })
    };
    let u32_at = |i: usize| -> Option<u32> {
        let b = t.get(i..i + 4)?;
        Some(if le { u32::from_le_bytes([b[0], b[1], b[2], b[3]]) } else { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) })
    };
    if u16_at(2)? != 42 {
        return None;
    }
    let ifd = u32_at(4)? as usize;
    let n = u16_at(ifd)? as usize;
    for e in 0..n.min(512) {
        let p = ifd + 2 + e * 12;
        if u16_at(p)? == 0x0112 && u16_at(p + 2)? == 3 {
            let v = u16_at(p + 8)?;
            return if (1..=8).contains(&v) { Some(v as u8) } else { None };
        }
    }
    None
}
