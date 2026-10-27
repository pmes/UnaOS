// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! JBIG2Decode (ISO 32000-1 §7.4.7; ITU-T T.88) for the embedded-stream profile PDF uses — PDFFAX SR331.
//!
//! CHARTER: Aether — shared-core. pdf_core's JBIG2 decoder: segments in sequential order with no file header
//! (Annex D.3), the /JBIG2Globals segments read first. It decodes through the ONE MQ coder (`mq.rs`) and the ONE
//! fax decoder (`ccitt.rs`, for MMR generic regions).
//!
//! Built: the segment header (7.2), page information and end of stripe (7.4.8-7.4.10), the region segment
//! information field and its external combination operators (7.4.1), the generic region (6.2: arithmetic templates
//! 0-3 with their AT pixels and TPGDON, or MMR), the symbol dictionary (6.5) and the text region (6.4), both
//! arithmetic, with the integer decoders of Annex A. Refused by name: Huffman coding ("JBIG2 Huffman"), refinement
//! ("JBIG2 refinement"), halftone regions and pattern dictionaries ("JBIG2 halftone region"), the extended generic
//! template, a symbol dictionary reusing a previous one's contexts, and a segment of unknown length. Intermediate
//! regions are skipped: only a refinement (refused) composes them onto the page.

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use super::ccitt;
use super::mq::MqDecoder;
use crate::{Error, MAX_DECODED};

/// A decoded bitmap, one byte per pixel, 1 = black (JBIG2's sense).
#[derive(Debug, Clone, PartialEq)]
pub struct Bitmap {
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>,
}

impl Bitmap {
    fn new(w: usize, h: usize, fill: u8) -> Result<Bitmap, Error> {
        if w > 1 << 20 || h > 1 << 20 || w.saturating_mul(h) > MAX_DECODED {
            return Err(Error::TooLarge);
        }
        Ok(Bitmap { w, h, data: vec![fill; w * h] })
    }

    #[inline]
    fn get(&self, x: isize, y: isize) -> u32 {
        if x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return 0;
        }
        self.data[y as usize * self.w + x as usize] as u32
    }

    /// Rows packed MSB-first, each padded to a byte; with `invert` black is written as 0 (the PDF sample sense).
    pub fn pack(&self, invert: bool) -> Vec<u8> {
        let row = self.w.div_ceil(8);
        let mut out = vec![if invert { 0xFF } else { 0 }; row * self.h];
        for y in 0..self.h {
            for x in 0..self.w {
                if self.data[y * self.w + x] != 0 {
                    let m = 0x80u8 >> (x % 8);
                    if invert {
                        out[y * row + x / 8] &= !m;
                    } else {
                        out[y * row + x / 8] |= m;
                    }
                }
            }
        }
        out
    }

    /// Draw `src` at (x, y) with combination operator `op` (0 OR, 1 AND, 2 XOR, 3 XNOR, 4 REPLACE).
    fn compose(&mut self, src: &Bitmap, x: i64, y: i64, op: u8) {
        for sy in 0..src.h {
            let dy = y + sy as i64;
            if dy < 0 || dy >= self.h as i64 {
                continue;
            }
            for sx in 0..src.w {
                let dx = x + sx as i64;
                if dx < 0 || dx >= self.w as i64 {
                    continue;
                }
                let d = &mut self.data[dy as usize * self.w + dx as usize];
                let s = src.data[sy * src.w + sx];
                *d = match op {
                    0 => *d | s,
                    1 => *d & s,
                    2 => *d ^ s,
                    3 => 1 ^ (*d ^ s),
                    _ => s,
                };
            }
        }
    }
}

const BAD: Error = Error::Syntax("JBIG2Decode segment", 0);

fn be32(b: &[u8], i: usize) -> Result<u32, Error> {
    b.get(i..i + 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]])).ok_or(BAD)
}

fn be16(b: &[u8], i: usize) -> Result<u16, Error> {
    b.get(i..i + 2).map(|s| u16::from_be_bytes([s[0], s[1]])).ok_or(BAD)
}

fn byte(b: &[u8], i: usize) -> Result<u8, Error> {
    b.get(i).copied().ok_or(BAD)
}

/// One segment (7.2): its number, type, referred-to segments and data.
struct Segment<'a> {
    num: u32,
    ty: u8,
    refs: Vec<u32>,
    data: &'a [u8],
}

/// The segments of an embedded stream, in order (a short last segment keeps the bytes there are).
fn segments(b: &[u8]) -> Result<Vec<Segment<'_>>, Error> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 11 <= b.len() {
        let num = be32(b, i)?;
        let flags = byte(b, i + 4)?;
        let ty = flags & 0x3F;
        let page4 = flags & 0x40 != 0;
        i += 5;
        let r = byte(b, i)?;
        let count = if r >> 5 == 7 {
            let c = (be32(b, i)? & 0x1FFF_FFFF) as usize;
            i += 4 + (c + 1).div_ceil(8);
            c
        } else {
            i += 1;
            (r >> 5) as usize
        };
        let size = if num <= 256 { 1 } else if num <= 65536 { 2 } else { 4 };
        if count.saturating_mul(size) > b.len().saturating_sub(i) {
            return Err(BAD);
        }
        let mut refs = Vec::with_capacity(count);
        for _ in 0..count {
            refs.push(match size {
                1 => byte(b, i)? as u32,
                2 => be16(b, i)? as u32,
                _ => be32(b, i)?,
            });
            i += size;
        }
        i += if page4 { 4 } else { 1 };
        let len = be32(b, i)?;
        i += 4;
        if len == u32::MAX {
            return Err(Error::Unsupported("JBIG2 unknown segment length"));
        }
        let end = i.saturating_add(len as usize).min(b.len());
        out.push(Segment { num, ty, refs, data: &b[i.min(end)..end] });
        i = end;
        if ty == 51 {
            break;
        }
    }
    Ok(out)
}

/// The generic region decoding parameters (6.2.2) for arithmetic coding.
#[derive(Clone, Copy)]
struct Generic {
    template: u8,
    tpgdon: bool,
    /// The AT pixels (A1-A4 for template 0; A1 alone for 1-3).
    at: [(i8, i8); 4],
}

/// TPGDON's pseudo-pixel context, per template (6.2.5.7 Figures 8-11).
const SLTP: [usize; 4] = [0x9B25, 0x0795, 0x00E5, 0x0195];

/// 6.2.5.7, arithmetic: the context of every pixel in the standard bit order (6.2.5.3 Figures 3-6).
fn generic(dec: &mut MqDecoder, cx: &mut [u8], w: usize, h: usize, g: &Generic) -> Result<Bitmap, Error> {
    let mut bm = Bitmap::new(w, h, 0)?;
    let at = g.at.map(|(x, y)| (x as isize, y as isize));
    let mut ltp = 0u8;
    for y in 0..h {
        let yi = y as isize;
        if g.tpgdon {
            ltp ^= dec.decode(&mut cx[SLTP[g.template as usize & 3]]);
            if ltp == 1 {
                if y > 0 {
                    bm.data.copy_within((y - 1) * w..y * w, y * w);
                }
                continue;
            }
        }
        for x in 0..w {
            let xi = x as isize;
            let p = |dx: isize, dy: isize| bm.get(xi + dx, yi + dy);
            let c = match g.template {
                0 => {
                    p(-1, 0)
                        | p(-2, 0) << 1
                        | p(-3, 0) << 2
                        | p(-4, 0) << 3
                        | p(at[0].0, at[0].1) << 4
                        | p(2, -1) << 5
                        | p(1, -1) << 6
                        | p(0, -1) << 7
                        | p(-1, -1) << 8
                        | p(-2, -1) << 9
                        | p(at[1].0, at[1].1) << 10
                        | p(at[2].0, at[2].1) << 11
                        | p(1, -2) << 12
                        | p(0, -2) << 13
                        | p(-1, -2) << 14
                        | p(at[3].0, at[3].1) << 15
                }
                1 => {
                    p(-1, 0)
                        | p(-2, 0) << 1
                        | p(-3, 0) << 2
                        | p(at[0].0, at[0].1) << 3
                        | p(2, -1) << 4
                        | p(1, -1) << 5
                        | p(0, -1) << 6
                        | p(-1, -1) << 7
                        | p(-2, -1) << 8
                        | p(2, -2) << 9
                        | p(1, -2) << 10
                        | p(0, -2) << 11
                        | p(-1, -2) << 12
                }
                2 => {
                    p(-1, 0)
                        | p(-2, 0) << 1
                        | p(at[0].0, at[0].1) << 2
                        | p(1, -1) << 3
                        | p(0, -1) << 4
                        | p(-1, -1) << 5
                        | p(-2, -1) << 6
                        | p(1, -2) << 7
                        | p(0, -2) << 8
                        | p(-1, -2) << 9
                }
                _ => {
                    p(-1, 0)
                        | p(-2, 0) << 1
                        | p(-3, 0) << 2
                        | p(-4, 0) << 3
                        | p(at[0].0, at[0].1) << 4
                        | p(1, -1) << 5
                        | p(0, -1) << 6
                        | p(-1, -1) << 7
                        | p(-2, -1) << 8
                        | p(-3, -1) << 9
                }
            };
            let v = dec.decode(&mut cx[c as usize]);
            bm.data[y * w + x] = v;
        }
    }
    Ok(bm)
}

/// The AT pixel bytes after a flags field: 8 bytes for template 0, 2 for the others.
fn at_pixels(b: &[u8], i: usize, template: u8) -> Result<([(i8, i8); 4], usize), Error> {
    let n = if template == 0 { 4 } else { 1 };
    let mut at = [(0i8, 0i8); 4];
    for (k, a) in at.iter_mut().enumerate().take(n) {
        *a = (byte(b, i + 2 * k)? as i8, byte(b, i + 2 * k + 1)? as i8);
    }
    Ok((at, i + 2 * n))
}

/// Annex A.2: an integer arithmetic decoder's 512 contexts.
struct Ia(Vec<u8>);

impl Ia {
    fn new() -> Ia {
        Ia(vec![0; 512])
    }

    fn bit(&mut self, dec: &mut MqDecoder, prev: &mut usize) -> u32 {
        let d = dec.decode(&mut self.0[*prev]) as usize;
        *prev = if *prev < 256 { (*prev << 1) | d } else { (((*prev << 1) | d) & 511) | 256 };
        d as u32
    }

    fn bits(&mut self, dec: &mut MqDecoder, prev: &mut usize, n: u32) -> i64 {
        let mut v = 0i64;
        for _ in 0..n {
            v = (v << 1) | self.bit(dec, prev) as i64;
        }
        v
    }

    /// One value; `None` is OOB.
    fn int(&mut self, dec: &mut MqDecoder) -> Option<i64> {
        let mut prev = 1usize;
        let s = self.bit(dec, &mut prev);
        let v = if self.bit(dec, &mut prev) == 0 {
            self.bits(dec, &mut prev, 2)
        } else if self.bit(dec, &mut prev) == 0 {
            self.bits(dec, &mut prev, 4) + 4
        } else if self.bit(dec, &mut prev) == 0 {
            self.bits(dec, &mut prev, 6) + 20
        } else if self.bit(dec, &mut prev) == 0 {
            self.bits(dec, &mut prev, 8) + 84
        } else if self.bit(dec, &mut prev) == 0 {
            self.bits(dec, &mut prev, 12) + 340
        } else {
            self.bits(dec, &mut prev, 32) + 4436
        };
        match (s, v) {
            (1, 0) => None,
            (1, v) => Some(-v),
            (_, v) => Some(v),
        }
    }
}

/// Annex A.3: the symbol ID decoder (`len` bits, contexts indexed by the bits so far behind a leading 1).
fn iaid(dec: &mut MqDecoder, cx: &mut [u8], len: u32) -> usize {
    let mut prev = 1usize;
    for _ in 0..len {
        prev = (prev << 1) | dec.decode(&mut cx[prev]) as usize;
    }
    prev - (1 << len)
}

/// The region segment information field (7.4.1): width, height, x, y, external combination operator.
fn region_info(b: &[u8]) -> Result<(usize, usize, i64, i64, u8), Error> {
    Ok((be32(b, 0)? as usize, be32(b, 4)? as usize, be32(b, 8)? as i64, be32(b, 12)? as i64, byte(b, 16)? & 7))
}

const MAX_SYMBOLS: usize = 1 << 16;

/// 6.5, arithmetic: the exported symbols of a symbol dictionary segment.
fn symbol_dict(b: &[u8], inputs: Vec<Bitmap>) -> Result<Vec<Bitmap>, Error> {
    let f = be16(b, 0)?;
    if f & 1 != 0 {
        return Err(Error::Unsupported("JBIG2 Huffman"));
    }
    if f & 2 != 0 {
        return Err(Error::Unsupported("JBIG2 refinement"));
    }
    if f & 0x100 != 0 {
        return Err(Error::Unsupported("JBIG2 retained contexts"));
    }
    let template = ((f >> 10) & 3) as u8;
    let (at, i) = at_pixels(b, 2, template)?;
    let _num_ex = be32(b, i)? as usize;
    let num_new = be32(b, i + 4)? as usize;
    if num_new > MAX_SYMBOLS || inputs.len() + num_new > MAX_SYMBOLS {
        return Err(Error::TooLarge);
    }
    let mut dec = MqDecoder::new(b.get(i + 8..).unwrap_or(&[]));
    let mut gb = vec![0u8; 1 << 16];
    let (mut iadh, mut iadw, mut iaex) = (Ia::new(), Ia::new(), Ia::new());
    let g = Generic { template, tpgdon: false, at };
    let mut new: Vec<Bitmap> = Vec::with_capacity(num_new);
    let mut height = 0i64;
    let mut pixels = 0usize;
    let mut classes = 0usize;
    while new.len() < num_new {
        classes += 1;
        if classes > num_new + 1 {
            return Err(BAD);
        }
        height += iadh.int(&mut dec).ok_or(BAD)?;
        if !(1..=1 << 16).contains(&height) {
            return Err(BAD);
        }
        let mut width = 0i64;
        while let Some(dw) = iadw.int(&mut dec) {
            width += dw;
            if !(0..=1 << 16).contains(&width) || new.len() >= num_new {
                return Err(BAD);
            }
            pixels += width as usize * height as usize;
            if pixels > MAX_DECODED {
                return Err(Error::TooLarge);
            }
            new.push(generic(&mut dec, &mut gb, width as usize, height as usize, &g)?);
        }
    }
    let mut all = inputs;
    all.extend(new);
    let mut flags = vec![false; all.len()];
    let (mut at_i, mut cur) = (0usize, false);
    let mut runs = 0usize;
    while at_i < all.len() {
        runs += 1;
        let run = iaex.int(&mut dec).ok_or(BAD)?;
        if run < 0 || at_i + run as usize > all.len() || runs > 2 * all.len() + 2 {
            return Err(BAD);
        }
        for fl in &mut flags[at_i..at_i + run as usize] {
            *fl = cur;
        }
        at_i += run as usize;
        cur = !cur;
    }
    Ok(all.into_iter().zip(flags).filter(|(_, f)| *f).map(|(s, _)| s).collect())
}

/// 6.4, arithmetic: a text region's bitmap.
fn text_region(b: &[u8], syms: &[Bitmap]) -> Result<Bitmap, Error> {
    let (w, h, ..) = region_info(b)?;
    let f = be16(b, 17)?;
    if f & 1 != 0 {
        return Err(Error::Unsupported("JBIG2 Huffman"));
    }
    if f & 2 != 0 {
        return Err(Error::Unsupported("JBIG2 refinement"));
    }
    let strips = 1i64 << ((f >> 2) & 3);
    let corner = ((f >> 4) & 3) as u8;
    let transposed = f & 0x40 != 0;
    let combop = ((f >> 7) & 3) as u8;
    let defpixel = ((f >> 9) & 1) as u8;
    let ds = {
        let v = ((f >> 10) & 0x1F) as i64;
        if v & 0x10 != 0 { v - 32 } else { v }
    };
    let num_inst = be32(b, 19)? as usize;
    let mut sb = Bitmap::new(w, h, defpixel)?;
    if num_inst == 0 {
        return Ok(sb);
    }
    if syms.is_empty() {
        return Err(BAD);
    }
    let mut len = 0u32;
    while (1usize << len) < syms.len() {
        len += 1;
    }
    let mut dec = MqDecoder::new(b.get(23..).unwrap_or(&[]));
    let (mut iadt, mut iafs, mut iads, mut iait) = (Ia::new(), Ia::new(), Ia::new(), Ia::new());
    let mut idcx = vec![0u8; 1 << len];
    let right = corner & 2 != 0;
    let bottom = corner & 1 == 0;
    let mut strip_t = -(iadt.int(&mut dec).ok_or(BAD)? * strips);
    let mut first_s = 0i64;
    let mut n = 0usize;
    while n < num_inst {
        strip_t += iadt.int(&mut dec).ok_or(BAD)? * strips;
        first_s += iafs.int(&mut dec).ok_or(BAD)?;
        let mut cur_s = first_s;
        loop {
            let cur_t = if strips == 1 { 0 } else { iait.int(&mut dec).ok_or(BAD)? };
            let t = strip_t + cur_t;
            let sym = syms.get(iaid(&mut dec, &mut idcx, len)).ok_or(BAD)?;
            let (wi, hi) = (sym.w as i64, sym.h as i64);
            // 6.4.5 3c x: the reference corner's side advances S before the symbol is placed ...
            if !transposed && right {
                cur_s += wi - 1;
            } else if transposed && bottom {
                cur_s += hi - 1;
            }
            let (x, y) = if transposed { (t, cur_s) } else { (cur_s, t) };
            let x = if right { x - wi + 1 } else { x };
            let y = if bottom { y - hi + 1 } else { y };
            sb.compose(sym, x, y, combop);
            // ... and the other side after.
            if !transposed && !right {
                cur_s += wi - 1;
            } else if transposed && !bottom {
                cur_s += hi - 1;
            }
            n += 1;
            let Some(d) = iads.int(&mut dec) else { break };
            if n >= num_inst {
                break;
            }
            cur_s += d + ds;
        }
    }
    Ok(sb)
}

/// A generic region segment's bitmap (7.4.6).
fn generic_region(b: &[u8]) -> Result<Bitmap, Error> {
    let (w, h, ..) = region_info(b)?;
    let f = byte(b, 17)?;
    if f & 0x10 != 0 {
        return Err(Error::Unsupported("JBIG2 extended template"));
    }
    let template = (f >> 1) & 3;
    if f & 1 != 0 {
        let bits = ccitt::mmr(b.get(18..).unwrap_or(&[]), w, h)?;
        let mut bm = Bitmap::new(w, h, 0)?;
        let row = w.div_ceil(8);
        for y in 0..h {
            for x in 0..w {
                bm.data[y * w + x] = (bits[y * row + x / 8] >> (7 - x % 8)) & 1;
            }
        }
        return Ok(bm);
    }
    let (at, i) = at_pixels(b, 18, template)?;
    let mut dec = MqDecoder::new(b.get(i..).unwrap_or(&[]));
    let mut cx = vec![0u8; 1 << 16];
    generic(&mut dec, &mut cx, w, h, &Generic { template, tpgdon: f & 8 != 0, at })
}

/// Undo JBIG2Decode: the page bitmap (1 = black). `globals` is the decoded /JBIG2Globals stream.
pub fn decode(data: &[u8], globals: Option<&[u8]>) -> Result<Bitmap, Error> {
    let mut segs = match globals {
        Some(g) => segments(g)?,
        None => Vec::new(),
    };
    segs.extend(segments(data)?);
    let mut dicts: BTreeMap<u32, Vec<Bitmap>> = BTreeMap::new();
    let mut page: Option<Bitmap> = None;
    let mut striped = false;
    let mut page_def = 0u8;
    for s in &segs {
        let referred = || -> Vec<Bitmap> { s.refs.iter().filter_map(|r| dicts.get(r)).flatten().cloned().collect() };
        match s.ty {
            0 => {
                let ex = symbol_dict(s.data, referred())?;
                dicts.insert(s.num, ex);
            }
            6 | 7 | 38 | 39 => {
                let (_, _, x, y, op) = region_info(s.data)?;
                let bm = if s.ty < 38 { text_region(s.data, &referred())? } else { generic_region(s.data)? };
                let pg = page.as_mut().ok_or(Error::Syntax("JBIG2 region before page information", 0))?;
                if striped && y as usize + bm.h > pg.h {
                    grow(pg, y as usize + bm.h, page_def)?;
                }
                pg.compose(&bm, x, y, op);
            }
            4 | 36 => {}
            16 | 20 | 22 | 23 => return Err(Error::Unsupported("JBIG2 halftone region")),
            40 | 42 | 43 => return Err(Error::Unsupported("JBIG2 refinement")),
            48 => {
                let w = be32(s.data, 0)? as usize;
                let h = be32(s.data, 4)?;
                let def = (byte(s.data, 16)? >> 2) & 1;
                striped = h == u32::MAX;
                page_def = def;
                page = Some(Bitmap::new(w, if striped { 0 } else { h as usize }, def)?);
            }
            50 => {
                if let (Some(pg), true) = (page.as_mut(), striped) {
                    let nh = be32(s.data, 0)? as usize + 1;
                    if nh > pg.h {
                        grow(pg, nh, page_def)?;
                    }
                }
            }
            49 | 51 => break,
            _ => {}
        }
    }
    page.ok_or(Error::Syntax("JBIG2 page information missing", 0))
}

/// A striped page (height 0xFFFFFFFF) grows to `h` rows of its default pixel.
fn grow(pg: &mut Bitmap, h: usize, def: u8) -> Result<(), Error> {
    if h > 1 << 20 || pg.w.saturating_mul(h) > MAX_DECODED {
        return Err(Error::TooLarge);
    }
    pg.data.resize(pg.w * h, def);
    pg.h = h;
    Ok(())
}
