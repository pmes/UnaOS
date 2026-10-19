// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Canon CR2's raw strip (RAWCOLOR, rmbp-ledger B724): IFD3's `Compression 6` strip is a LOSSLESS JPEG
//! (ITU-T T.81 Annex H, "ljpeg92"): SOI, DHT, SOF3 (precision, height, width, 2 or 4 components), SOS (the
//! predictor, Canon's is 1 = the left neighbour), then Huffman-coded differences with `FF 00` stuffing.
//!
//! The JPEG frame is `wide x high` pixels of `clrs` interleaved components; read as samples it is a `wide * clrs`
//! by `high` raster (dcraw's `lossless_jpeg_load_raw`). Bodies since the 5D Mark II cut the SENSOR into vertical
//! SLICES (tag 0xC640, three SHORTs `[n, sw, slast]`): the stream holds slice 0 (columns `0..sw`, every row), then
//! slice 1, …, `n` slices of `sw` columns and a last one of `slast` — so mosaic row 0 is spread over the whole
//! stream. To hand the develop rows IN ORDER without holding the mosaic, the stream is walked once to the start of
//! each slice and the decoder state there is kept (the bit position and the per-component predictors: Canon's
//! predictor 1 needs nothing from the previous row but the first column's running value); then one cursor per
//! slice reads its `sw` samples of each row in turn. Memory: one row and one fetch window per slice.
//!
//! Predictors other than 1, restart intervals and Canon's sRAW (3 components) are refused by name.

use alloc::vec::Vec;

use crate::decode::{BitState, Bits, Huff, Source};
use crate::{Error, MAX_DIM, MAX_PIXELS};

/// How much of the strip's head the markers are read from.
pub const HEAD: usize = 16 << 10;

/// A lossless JPEG frame's header.
#[derive(Clone, Debug)]
pub struct Frame {
    pub bits: u8,
    pub high: u16,
    pub wide: u16,
    pub clrs: u8,
    pub psv: u8,
    /// The Huffman table of each component (in SOF order).
    huff: Vec<Huff>,
    /// Byte offset of the entropy-coded data within the strip.
    pub data: u64,
}

fn be16(b: &[u8], i: usize) -> Result<usize, Error> {
    let s = b.get(i..i + 2).ok_or(Error::Truncated)?;
    Ok(u16::from_be_bytes([s[0], s[1]]) as usize)
}

/// Read the markers up to the scan. `b` = the strip's first bytes.
pub fn frame(b: &[u8]) -> Result<Frame, Error> {
    if b.get(0..2) != Some(&[0xFF, 0xD8]) {
        return Err(Error::Malformed("ljpeg SOI"));
    }
    let mut tables: [Option<Huff>; 4] = [None, None, None, None];
    let mut sof: Option<(u8, u16, u16, Vec<u8>)> = None;
    let mut p = 2usize;
    loop {
        // A marker: one or more FF, then its code.
        while b.get(p) == Some(&0xFF) && b.get(p + 1) == Some(&0xFF) {
            p += 1;
        }
        if b.get(p) != Some(&0xFF) {
            return Err(if p >= b.len() { Error::Truncated } else { Error::Malformed("ljpeg marker") });
        }
        let m = *b.get(p + 1).ok_or(Error::Truncated)?;
        let len = be16(b, p + 2)?;
        if len < 2 {
            return Err(Error::Malformed("ljpeg segment length"));
        }
        let seg = b.get(p + 4..p + 2 + len).ok_or(Error::Truncated)?;
        match m {
            0xC4 => {
                let mut i = 0;
                while i < seg.len() {
                    let id = (seg[i] & 3) as usize;
                    let counts: [u8; 16] = seg.get(i + 1..i + 17).ok_or(Error::Truncated)?.try_into().map_err(|_| Error::Truncated)?;
                    let n: usize = counts.iter().map(|&c| c as usize).sum();
                    let syms = seg.get(i + 17..i + 17 + n).ok_or(Error::Truncated)?;
                    tables[id] = Some(Huff::new(&counts, syms)?);
                    i += 17 + n;
                }
            }
            0xC3 => {
                if seg.len() < 6 {
                    return Err(Error::Truncated);
                }
                let nf = seg[5] as usize;
                let ids: Vec<u8> = (0..nf).filter_map(|k| seg.get(6 + 3 * k).copied()).collect();
                if ids.len() != nf {
                    return Err(Error::Truncated);
                }
                sof = Some((seg[0], be16(seg, 1)? as u16, be16(seg, 3)? as u16, ids));
            }
            0xC0..=0xC2 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => return Err(Error::Unsupported("JPEG raw that is not lossless (SOF3)")),
            0xDD => {
                if be16(seg, 0)? != 0 {
                    return Err(Error::Unsupported("ljpeg restart interval"));
                }
            }
            0xDA => {
                let (bits, high, wide, ids) = sof.ok_or(Error::Malformed("ljpeg SOS before SOF3"))?;
                let ns = *seg.first().ok_or(Error::Truncated)? as usize;
                let clrs = ids.len();
                if ns != clrs {
                    return Err(Error::Unsupported("ljpeg scan of a component subset"));
                }
                if clrs == 3 {
                    return Err(Error::Unsupported("Canon sRAW"));
                }
                if !(1..=4).contains(&clrs) || !(2..=16).contains(&bits) || high == 0 || wide == 0 {
                    return Err(Error::Malformed("ljpeg frame"));
                }
                let mut huff = Vec::new();
                for k in 0..clrs {
                    // The scan's selector for SOF component k; a selector naming no table falls back to dcraw's rule
                    // (component k takes table k, else the nearest lower one).
                    let sel = (0..ns).find(|&j| seg.get(1 + 2 * j) == Some(&ids[k])).and_then(|j| seg.get(2 + 2 * j)).map(|&t| (t >> 4) as usize & 3);
                    let t = sel.and_then(|t| tables[t].clone()).or_else(|| (0..=k.min(3)).rev().find_map(|t| tables[t].clone())).ok_or(Error::Malformed("ljpeg Huffman table"))?;
                    huff.push(t);
                }
                let psv = *seg.get(1 + 2 * ns).ok_or(Error::Truncated)?;
                if psv != 1 {
                    return Err(Error::Unsupported("ljpeg predictor other than 1"));
                }
                return Ok(Frame { bits, high, wide, clrs: clrs as u8, psv, huff, data: (p + 2 + len) as u64 });
            }
            0xD9 => return Err(Error::Malformed("ljpeg EOI before the scan")),
            _ => {}
        }
        p += 2 + len;
    }
}

impl Frame {
    /// The mosaic this frame fills (dcraw: `wide * clrs` by `high`, halved and doubled when that is wider than four
    /// times its height with an even component count).
    pub fn dims(&self) -> (u32, u32) {
        let (mut w, mut h) = (self.wide as u32 * self.clrs as u32, self.high as u32);
        if w > 4 * h && self.clrs % 2 == 0 {
            w /= 2;
            h *= 2;
        }
        (w, h)
    }
}

/// Where each slice starts in the stream, the decoder state there, and the mosaic geometry.
#[derive(Clone, Debug)]
pub struct Plan {
    pub f: Frame,
    pub w: usize,
    pub h: usize,
    /// Each slice's width; one entry when the strip is not sliced.
    seg: Vec<usize>,
    /// The strip's length.
    len: u64,
}

/// One cursor's snapshot.
#[derive(Clone, Copy, Debug)]
struct Snap {
    bits: BitState,
    vpred: [i32; 4],
    left: [i32; 4],
    s: u64,
}

impl Plan {
    /// The plan of a strip whose first bytes are `head`, `len` bytes long, sliced by `slices` (tag 0xC640).
    pub fn new(head: &[u8], len: u64, slices: Option<[u16; 3]>) -> Result<Plan, Error> {
        let f = frame(head)?;
        let (w, h) = f.dims();
        if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM || w as u64 * h as u64 > MAX_PIXELS {
            return Err(Error::TooLarge);
        }
        let seg = match slices {
            Some([n, sw, last]) if n > 0 => {
                let (n, sw, last) = (n as usize, sw as usize, last as usize);
                if sw == 0 || n.checked_mul(sw).and_then(|x| x.checked_add(last)) != Some(w as usize) {
                    return Err(Error::Malformed("CR2 slice table"));
                }
                let mut v = alloc::vec![sw; n];
                if last > 0 {
                    v.push(last);
                }
                v
            }
            _ => alloc::vec![w as usize],
        };
        Ok(Plan { f, w: w as usize, h: h as usize, seg, len })
    }

    /// Read the head from `src` and plan.
    pub fn read<S: Source>(src: &mut S, len: u64, slices: Option<[u16; 3]>) -> Result<Plan, Error> {
        let head = src.fetch(0, core::cmp::min(len, HEAD as u64) as usize)?;
        Plan::new(&head, len, slices)
    }

    fn start(&self) -> Snap {
        let v = 1i32 << (self.f.bits - 1);
        Snap { bits: BitState::at(self.f.data), vpred: [v; 4], left: [0; 4], s: 0 }
    }

    /// The next sample of the stream.
    #[inline]
    fn sample<S: Source>(&self, c: &mut Snap, b: &mut Bits, src: &mut S) -> Result<u16, Error> {
        let clrs = self.f.clrs as u64;
        let js = c.s % (self.f.wide as u64 * clrs);
        let (k, col) = ((js % clrs) as usize, js / clrs);
        let t = self.f.huff[k].decode(b, src)? as u32;
        let diff = match t {
            0 => 0,
            16 => -32768,
            1..=15 => {
                let x = b.get(src, t)? as i32;
                if x < 1 << (t - 1) { x - (1 << t) + 1 } else { x }
            }
            _ => return Err(Error::Malformed("ljpeg difference category")),
        };
        let pred = if col == 0 {
            let p = c.vpred[k];
            c.vpred[k] = p + diff;
            p
        } else {
            c.left[k]
        };
        let v = pred + diff;
        c.left[k] = v;
        c.s += 1;
        Ok(v.clamp(0, 0xFFFF) as u16)
    }

    /// Hand rows `0..rows` of the mosaic to `f`, in order.
    pub fn stream<S: Source>(&self, src: &mut S, rows: usize, mut f: impl FnMut(usize, &[u16])) -> Result<(), Error> {
        let rows = rows.min(self.h);
        // Each slice's start: the walk from the stream's start (only when sliced).
        let mut snaps: Vec<Snap> = Vec::new();
        snaps.try_reserve_exact(self.seg.len()).map_err(|_| Error::OutOfMemory)?;
        let mut c = self.start();
        snaps.push(c);
        if self.seg.len() > 1 && rows > 0 {
            let mut b = Bits::new(c.bits, self.len, true);
            for i in 1..self.seg.len() {
                let at = (i * self.seg[0] * self.h) as u64;
                while c.s < at {
                    self.sample(&mut c, &mut b, src)?;
                }
                c.bits = b.state();
                snaps.push(c);
            }
        }
        let mut cur: Vec<(Snap, Bits)> = Vec::new();
        cur.try_reserve_exact(snaps.len()).map_err(|_| Error::OutOfMemory)?;
        for s in &snaps {
            cur.push((*s, Bits::new(s.bits, self.len, true)));
        }
        let mut row = crate::zeroed::<u16>(self.w)?;
        for y in 0..rows {
            let mut x0 = 0;
            for (i, (c, b)) in cur.iter_mut().enumerate() {
                for x in 0..self.seg[i] {
                    row[x0 + x] = self.sample(c, b, src)?;
                }
                x0 += self.seg[i];
            }
            f(y, &row);
        }
        Ok(())
    }
}
