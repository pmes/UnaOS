// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The raw strip, one row at a time. Two families:
//!
//! ROW-ADDRESSABLE (a row is a fixed number of bytes, so a caller can stream a 50 MB strip through a few rows):
//!
//! * `Compression 1` — 16-bit sample containers in the file's byte order (`BitsPerSample` 12, 14 or 16), or —
//!   when the strip is exactly `w * h * bits / 8` bytes — PACKED 10/12/14-bit samples, MSB first (Nikon's
//!   uncompressed NEF, RAWCOLOR B724).
//! * `Compression 32767`, `BitsPerSample 8` — Sony's compressed raw ("cRAW", ARW 2.x), as dcraw's
//!   `sony_arw2_load_raw` reads it: a row is `width` bytes of 16-byte blocks; a block carries sixteen pixels of
//!   ONE colour (every other column of a 32-column span — the first block the even columns, the next the odd):
//!   bits 0..11 max, 11..22 min, 22..26 the index of the max pixel, 26..30 the index of the min pixel, then the
//!   other fourteen pixels as 7-bit deltas above min, shifted left by `sh` (the smallest of 0..=3 with
//!   `0x80 << sh > max - min`), clamped to 11 bits. Each 11-bit value goes through the tone curve
//!   (`curve[v << 1] >> 2`) to the 14-bit sample.
//!
//! SEQUENTIAL (a Huffman bit stream; RAWCOLOR B724): `Compression 6` — Canon CR2's lossless JPEG ([`crate::cr2`]);
//! `Compression 34713` — Nikon NEF's Huffman coding ([`crate::nef`]). These are read through a [`Source`] by
//! [`RowDecoder::stream`], which hands rows out IN ORDER for every coding; [`RowDecoder::row`] is the
//! row-addressable families' only.
//!
//! The bit reader ([`Bits`]) pulls `WIN`-byte windows from the [`Source`], so a sequential strip is streamed too.

use alloc::vec::Vec;

use crate::tiff::Strip;
use crate::Error;

/// Where a strip's bytes come from: `fetch(off, len)` = `len` bytes at `off` WITHIN THE STRIP (a file in memory, a
/// VFS read — the kernel's Facet streams through one).
pub trait Source {
    fn fetch(&mut self, off: u64, len: usize) -> Result<Vec<u8>, Error>;
}

/// A strip held in memory.
pub struct SliceSource<'a>(pub &'a [u8]);

impl Source for SliceSource<'_> {
    fn fetch(&mut self, off: u64, len: usize) -> Result<Vec<u8>, Error> {
        let s = usize::try_from(off).map_err(|_| Error::Truncated)?;
        let b = self.0.get(s..s.checked_add(len).ok_or(Error::Truncated)?).ok_or(Error::Truncated)?;
        let mut v = Vec::new();
        v.try_reserve_exact(len).map_err(|_| Error::OutOfMemory)?;
        v.extend_from_slice(b);
        Ok(v)
    }
}

/// The bytes one fetch pulls for a bit stream.
pub const WIN: usize = 64 << 10;
/// The bytes one fetch pulls for row-addressable rows (at least one row).
pub const RUN: usize = 256 << 10;

/// A bit reader's position: the next byte to pull and the bits already pulled. Copy: a snapshot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BitState {
    pos: u64,
    acc: u64,
    n: u32,
    eos: bool,
}

impl BitState {
    /// Before the byte at strip offset `pos`.
    pub fn at(pos: u64) -> BitState {
        BitState { pos, ..Default::default() }
    }
}

/// MSB-first bits from a [`Source`], one window held. `jpeg`: an `FF 00` is one `FF`; any other `FF xx` is a marker,
/// which ends the data (zeros follow, as dcraw's `getbits` reads them).
#[derive(Clone, Debug)]
pub struct Bits {
    st: BitState,
    win: Vec<u8>,
    at: u64,
    len: u64,
    jpeg: bool,
}

impl Bits {
    /// From `st` in a strip of `len` bytes.
    pub fn new(st: BitState, len: u64, jpeg: bool) -> Bits {
        Bits { st, win: Vec::new(), at: 0, len, jpeg }
    }

    pub fn state(&self) -> BitState {
        self.st
    }

    fn byte_at<S: Source>(&mut self, src: &mut S, pos: u64) -> Result<Option<u8>, Error> {
        if pos >= self.len {
            return Ok(None);
        }
        if pos < self.at || pos >= self.at + self.win.len() as u64 {
            let n = core::cmp::min(WIN as u64, self.len - pos) as usize;
            self.win = src.fetch(pos, n)?;
            if self.win.len() < n {
                return Err(Error::Truncated);
            }
            self.at = pos;
        }
        Ok(Some(self.win[(pos - self.at) as usize]))
    }

    fn fill<S: Source>(&mut self, src: &mut S, need: u32) -> Result<(), Error> {
        while self.st.n < need {
            let mut b = 0u8;
            if !self.st.eos {
                match self.byte_at(src, self.st.pos)? {
                    None => self.st.eos = true,
                    Some(0xFF) if self.jpeg => match self.byte_at(src, self.st.pos + 1)? {
                        Some(0) => {
                            self.st.pos += 2;
                            b = 0xFF;
                        }
                        _ => self.st.eos = true,
                    },
                    Some(x) => {
                        self.st.pos += 1;
                        b = x;
                    }
                }
            }
            self.st.acc = (self.st.acc << 8) | b as u64;
            self.st.n += 8;
        }
        Ok(())
    }

    /// The next 16 bits, not consumed.
    pub fn peek16<S: Source>(&mut self, src: &mut S) -> Result<u32, Error> {
        self.fill(src, 16)?;
        Ok(((self.st.acc >> (self.st.n - 16)) & 0xFFFF) as u32)
    }

    pub fn skip(&mut self, k: u32) {
        self.st.n -= k.min(self.st.n);
        self.st.acc &= (1u64 << self.st.n) - 1;
    }

    /// `k` bits (0..=16).
    pub fn get<S: Source>(&mut self, src: &mut S, k: u32) -> Result<u32, Error> {
        if k == 0 {
            return Ok(0);
        }
        self.fill(src, k)?;
        let v = ((self.st.acc >> (self.st.n - k)) & ((1u64 << k) - 1)) as u32;
        self.skip(k);
        Ok(v)
    }
}

/// A canonical Huffman table (JPEG's: sixteen code-length counts, then the symbols in code order).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Huff {
    maxcode: [i32; 17],
    mincode: [i32; 17],
    valptr: [usize; 17],
    vals: Vec<u8>,
}

impl Huff {
    pub fn new(counts: &[u8; 16], syms: &[u8]) -> Result<Huff, Error> {
        let (mut maxcode, mut mincode, mut valptr) = ([-1i32; 17], [0i32; 17], [0usize; 17]);
        let (mut code, mut k) = (0i32, 0usize);
        for l in 1..=16 {
            let c = counts[l - 1] as i32;
            valptr[l] = k;
            mincode[l] = code;
            code += c;
            k += c as usize;
            if c > 0 {
                maxcode[l] = code - 1;
            }
            if code > 1 << l {
                return Err(Error::Malformed("Huffman table over-full"));
            }
            code <<= 1;
        }
        if k == 0 || k > syms.len() {
            return Err(Error::Malformed("Huffman table symbols"));
        }
        Ok(Huff { maxcode, mincode, valptr, vals: syms[..k].to_vec() })
    }

    /// One symbol.
    pub fn decode<S: Source>(&self, b: &mut Bits, src: &mut S) -> Result<u8, Error> {
        let p = b.peek16(src)?;
        for l in 1..=16usize {
            let c = (p >> (16 - l)) as i32;
            if c <= self.maxcode[l] {
                b.skip(l as u32);
                return Ok(self.vals[self.valptr[l] + (c - self.mincode[l]) as usize]);
            }
        }
        Err(Error::Malformed("Huffman code"))
    }

    /// Each symbol's (code, length), first occurrence (the synthetic encoders').
    pub fn codes(&self) -> Vec<Option<(u16, u8)>> {
        let mut out = alloc::vec![None; 256];
        for l in 1..=16usize {
            if self.maxcode[l] < 0 {
                continue;
            }
            for c in self.mincode[l]..=self.maxcode[l] {
                let s = self.vals[self.valptr[l] + (c - self.mincode[l]) as usize] as usize;
                if out[s].is_none() {
                    out[s] = Some((c as u16, l as u8));
                }
            }
        }
        out
    }
}

/// Decodes rows of one [`Strip`].
pub struct RowDecoder {
    pub width: usize,
    pub height: usize,
    kind: Kind,
    /// The largest sample value the coding can produce (the default white level).
    pub max_code: u16,
    /// The strip's byte length.
    len: u64,
}

enum Kind {
    Plain { le: bool },
    Packed { bits: u32 },
    Craw { curve: Vec<u16> },
    /// The plan is read from the strip's head when the parse could not (the strip lay past the head it was given).
    Ljpeg { plan: Option<crate::cr2::Plan>, slices: Option<[u16; 3]> },
    Nikon { plan: crate::nef::Plan },
}

/// dcraw's Sony curve: identity, then from the four knees (tag 28688, each `>> 2 & 0xfff`) the slope doubles
/// at every knee up to 4095.
pub fn sony_curve(knees: Option<[u16; 4]>) -> Vec<u16> {
    let mut c: Vec<u16> = (0..0x1_0000u32).map(|i| i as u16).collect();
    let k = knees.unwrap_or([0; 4]);
    let s = [0u16, k[0], k[1], k[2], k[3], 4095];
    for i in 0..5 {
        let mut j = s[i] as usize + 1;
        while j <= s[i + 1] as usize && j < c.len() {
            c[j] = c[j - 1].wrapping_add(1 << i);
            j += 1;
        }
    }
    c
}

impl RowDecoder {
    /// The decoder of `s`. A CR2 strip's geometry comes from its JPEG header: when [`parse`](crate::parse) was given
    /// the strip's bytes it read it; otherwise use [`open`](Self::open).
    pub fn new(s: &Strip) -> Result<Self, Error> {
        let (w, h) = (s.width as usize, s.height as usize);
        let packed = |bits: u16| matrix_len(w, h, bits as u64) == Some(s.len) && bits < 16 && (w * bits as usize) % 8 == 0;
        let (kind, max_code) = match (s.compression, s.bits) {
            (1, b @ (10 | 12 | 14)) if packed(b) => (Kind::Packed { bits: b as u32 }, ((1u32 << b) - 1) as u16),
            (1, 9..=16) => (Kind::Plain { le: s.le }, ((1u32 << s.bits) - 1) as u16),
            (1, _) => return Err(Error::Unsupported("uncompressed sample depth")),
            (32767, 8) => {
                let curve = sony_curve(s.curve);
                let top = curve[0x7ff << 1] >> 2;
                (Kind::Craw { curve }, top)
            }
            (32767, 12) => return Err(Error::Unsupported("Sony packed 12-bit (owed: a real file)")),
            (32767, _) => return Err(Error::Unsupported("Sony ARW1 (owed)")),
            (6, b) => (Kind::Ljpeg { plan: None, slices: s.slices }, ((1u32 << b.clamp(2, 16)) - 1) as u16),
            (34713, _) => {
                let m = s.nikon.as_ref().ok_or(Error::Unsupported("NEF without its linearisation table"))?;
                (Kind::Nikon { plan: crate::nef::Plan::new(w, h, m, s.len)? }, m.white)
            }
            (7, _) => return Err(Error::Unsupported("lossless JPEG raw (owed)")),
            _ => return Err(Error::Unsupported("raw compression")),
        };
        let d = RowDecoder { width: w, height: h, kind, max_code, len: s.len };
        if (d.row_bytes() as u64) * (h as u64) > s.len {
            return Err(Error::Truncated);
        }
        Ok(d)
    }

    /// [`new`](Self::new), the CR2 geometry read from the strip's head through `src` (strip-relative offsets).
    pub fn open<S: Source>(s: &Strip, src: &mut S) -> Result<Self, Error> {
        let mut d = RowDecoder::new(s)?;
        if let Kind::Ljpeg { plan, slices } = &mut d.kind {
            let p = crate::cr2::Plan::read(src, s.len, *slices)?;
            d.width = p.w;
            d.height = p.h;
            d.max_code = ((1u32 << p.f.bits) - 1) as u16;
            *plan = Some(p);
        }
        Ok(d)
    }

    /// The coding's name on the wire: `craw` (Sony ARW 2.x compressed), `plain` (16-bit containers), `packed`
    /// (MSB-first packed samples), `ljpeg` (Canon CR2's lossless JPEG), `nikon` (NEF's Huffman coding).
    pub fn coding(&self) -> &'static str {
        match self.kind {
            Kind::Plain { .. } => "plain",
            Kind::Packed { .. } => "packed",
            Kind::Craw { .. } => "craw",
            Kind::Ljpeg { .. } => "ljpeg",
            Kind::Nikon { .. } => "nikon",
        }
    }

    /// A bit stream that is read in order (no fixed row size): [`stream`](Self::stream) only.
    pub fn sequential(&self) -> bool {
        matches!(self.kind, Kind::Ljpeg { .. } | Kind::Nikon { .. })
    }

    /// Bytes one coded row occupies (0 for a sequential coding).
    pub fn row_bytes(&self) -> usize {
        match self.kind {
            Kind::Plain { .. } => self.width * 2,
            Kind::Packed { bits } => self.width * bits as usize / 8,
            Kind::Craw { .. } => self.width,
            Kind::Ljpeg { .. } | Kind::Nikon { .. } => 0,
        }
    }

    /// Decode one row (`src` = exactly [`row_bytes`](Self::row_bytes)) into `out` (`width` samples). Row-addressable
    /// codings only.
    pub fn row(&self, src: &[u8], out: &mut [u16]) -> Result<(), Error> {
        if self.sequential() {
            return Err(Error::Unsupported("a sequential coding has no row by itself"));
        }
        if src.len() < self.row_bytes() || out.len() < self.width {
            return Err(Error::Truncated);
        }
        match &self.kind {
            Kind::Plain { le } => {
                for (o, p) in out.iter_mut().zip(src.chunks_exact(2)) {
                    *o = if *le { u16::from_le_bytes([p[0], p[1]]) } else { u16::from_be_bytes([p[0], p[1]]) };
                }
            }
            Kind::Packed { bits } => {
                let (mut acc, mut n, mut i) = (0u32, 0u32, 0usize);
                for o in out[..self.width].iter_mut() {
                    while n < *bits {
                        acc = (acc << 8) | src[i] as u32;
                        i += 1;
                        n += 8;
                    }
                    *o = ((acc >> (n - bits)) & ((1 << bits) - 1)) as u16;
                    n -= bits;
                    acc &= (1 << n) - 1;
                }
            }
            Kind::Craw { curve } => {
                out[..self.width].fill(0);
                let w = self.width;
                let mut col = 0usize;
                let mut dp = 0usize;
                while col + 30 < w && dp + 16 <= src.len() {
                    let blk = &src[dp..dp + 16];
                    let val = u32::from_le_bytes([blk[0], blk[1], blk[2], blk[3]]);
                    let max = val & 0x7ff;
                    let min = (val >> 11) & 0x7ff;
                    let imax = ((val >> 22) & 0x0f) as usize;
                    let imin = ((val >> 26) & 0x0f) as usize;
                    let mut sh = 0u32;
                    while sh < 4 && (0x80u32 << sh) <= max.wrapping_sub(min) {
                        sh += 1;
                    }
                    let mut bit = 30usize;
                    for i in 0..16 {
                        let p = if i == imax {
                            max
                        } else if i == imin {
                            min
                        } else {
                            let at = bit >> 3;
                            let two = u16::from_le_bytes([blk[at], *blk.get(at + 1).unwrap_or(&0)]) as u32;
                            let v = (((two >> (bit & 7)) & 0x7f) << sh) + min;
                            bit += 7;
                            v.min(0x7ff)
                        };
                        if col < w {
                            out[col] = curve[(p as usize) << 1] >> 2;
                        }
                        col += 2;
                    }
                    col -= if col & 1 == 1 { 1 } else { 31 };
                    dp += 16;
                }
            }
            Kind::Ljpeg { .. } | Kind::Nikon { .. } => {}
        }
        Ok(())
    }

    /// Hand mosaic rows `0..rows` to `f` in order, reading the strip through `src` (strip-relative offsets): runs of
    /// rows for a row-addressable coding, the bit stream for a sequential one. One row is held, never the mosaic.
    pub fn stream<S: Source>(&self, src: &mut S, rows: usize, mut f: impl FnMut(usize, &[u16])) -> Result<(), Error> {
        match &self.kind {
            Kind::Ljpeg { plan: Some(p), .. } => p.stream(src, rows, f),
            Kind::Ljpeg { plan: None, slices } => crate::cr2::Plan::read(src, self.len, *slices)?.stream(src, rows, f),
            Kind::Nikon { plan } => plan.stream(src, rows, f),
            _ => {
                let rows = rows.min(self.height);
                let mut row = crate::zeroed::<u16>(self.width)?;
                let rb = self.row_bytes();
                let per = core::cmp::max(1, RUN / rb.max(1));
                let mut y = 0usize;
                while y < rows {
                    let n = core::cmp::min(per, rows - y);
                    let run = src.fetch((y * rb) as u64, n * rb)?;
                    if run.len() < n * rb {
                        return Err(Error::Truncated);
                    }
                    for i in 0..n {
                        self.row(&run[i * rb..(i + 1) * rb], &mut row)?;
                        f(y + i, &row);
                    }
                    y += n;
                }
                Ok(())
            }
        }
    }

    /// The whole mosaic from a strip held in memory (`strip` = the strip's bytes): `width * height` samples (for a
    /// CR2 decoder made by [`new`](Self::new) without its geometry, the frame's — read [`dims`](Self::dims)).
    pub fn all(&self, strip: &[u8]) -> Result<Vec<u16>, Error> {
        let mut src = SliceSource(strip);
        let owned;
        let (w, h, plan) = match &self.kind {
            Kind::Ljpeg { plan: None, slices } => {
                owned = crate::cr2::Plan::read(&mut src, self.len.min(strip.len() as u64), *slices)?;
                (owned.w, owned.h, Some(&owned))
            }
            _ => (self.width, self.height, None),
        };
        let n = w.checked_mul(h).ok_or(Error::TooLarge)?;
        let mut v = crate::zeroed::<u16>(n)?;
        let put = |y: usize, r: &[u16]| v[y * w..(y + 1) * w].copy_from_slice(&r[..w]);
        match plan {
            Some(p) => p.stream(&mut src, h, put)?,
            None => self.stream(&mut src, h, put)?,
        }
        Ok(v)
    }

    /// The mosaic's width and height as the coding knows them.
    pub fn dims(&self) -> (usize, usize) {
        (self.width, self.height)
    }
}

/// `w * h * bits / 8`, checked.
fn matrix_len(w: usize, h: usize, bits: u64) -> Option<u64> {
    (w as u64).checked_mul(h as u64)?.checked_mul(bits).map(|x| x / 8)
}
