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
//! 0-3 with their AT pixels and TPGDON, or MMR; of unknown length (7.2.7) when MMR is 0), the symbol dictionary (6.5)
//! and the text region (6.4), arithmetic (the integer decoders of Annex A) or Huffman (`huffman`: the standard tables
//! of Annex B and table segments), each with refinement and aggregation (`refine`, 6.3, also the refinement region
//! segment), and pattern dictionaries with halftone regions (`halftone`, 6.6-6.7, Annex C.5) — PDFFAX SR331,
//! JBIG2HUFF SR336. Intermediate regions are kept for the refinement that refers to them. Refused by name: the extended
//! generic template, a symbol dictionary reusing a previous one's contexts, a segment of unknown length other than an
//! arithmetic immediate generic region, MMR grey-scale planes beyond one ("JBIG2 halftone MMR") and the colour
//! extension.

use alloc::collections::BTreeMap;
use alloc::vec;
use alloc::vec::Vec;

use super::ccitt;
use super::ccitt::Bits;
use super::mq::MqDecoder;
use crate::{Error, MAX_DECODED};

mod halftone;
pub mod huffman;
mod refine;

use huffman::Table;
use refine::Refine;

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

    /// The columns `x0..x0 + w` (a symbol or pattern cut from a collective bitmap).
    fn columns(&self, x0: usize, w: usize) -> Result<Bitmap, Error> {
        if x0 + w > self.w {
            return Err(BAD);
        }
        let mut out = Bitmap::new(w, self.h, 0)?;
        for y in 0..self.h {
            out.data[y * w..(y + 1) * w].copy_from_slice(&self.data[y * self.w + x0..y * self.w + x0 + w]);
        }
        Ok(out)
    }

    /// The `w` x `h` area at (x, y), 0 outside (a refinement region's reference taken from the page, 7.4.7.5).
    fn area(&self, x: i64, y: i64, w: usize, h: usize) -> Result<Bitmap, Error> {
        let mut out = Bitmap::new(w, h, 0)?;
        for yy in 0..h {
            for xx in 0..w {
                out.data[yy * w + xx] = self.get((x + xx as i64) as isize, (y + yy as i64) as isize) as u8;
            }
        }
        Ok(out)
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
    /// The row count closing an immediate generic region of unknown length (7.2.7).
    rows: Option<u32>,
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
        let (end, rows) = if len == u32::MAX {
            let (e, r) = unknown_end(b, i, ty).ok_or(Error::Unsupported("JBIG2 unknown segment length"))?;
            (e, Some(r))
        } else {
            (i.saturating_add(len as usize).min(b.len()), None)
        };
        out.push(Segment { num, ty, refs, data: &b[i.min(end)..end], rows });
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
    /// The AT pixels (A1-A4 for template 0; A1 alone for 1-3); a pattern dictionary's A1 is (-HDPW, 0).
    at: [(i32, i32); 4],
}

/// TPGDON's pseudo-pixel context, per template (6.2.5.7 Figures 8-11).
const SLTP: [usize; 4] = [0x9B25, 0x0795, 0x00E5, 0x0195];

/// 6.2.5.7, arithmetic: the context of every pixel in the standard bit order (6.2.5.3 Figures 3-6). A pixel set in
/// `skip` (USESKIP, the halftone grey planes) is 0 and not decoded.
fn generic(dec: &mut MqDecoder, cx: &mut [u8], w: usize, h: usize, g: &Generic, skip: Option<&Bitmap>) -> Result<Bitmap, Error> {
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
            if skip.is_some_and(|s| s.data[y * w + x] != 0) {
                continue;
            }
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
fn at_pixels(b: &[u8], i: usize, template: u8) -> Result<([(i32, i32); 4], usize), Error> {
    let n = if template == 0 { 4 } else { 1 };
    let mut at = [(0i32, 0i32); 4];
    for (k, a) in at.iter_mut().enumerate().take(n) {
        *a = (byte(b, i + 2 * k)? as i8 as i32, byte(b, i + 2 * k + 1)? as i8 as i32);
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

/// 6.5: the exported symbols of a symbol dictionary segment — arithmetic (Annex A) or Huffman (SDHUFF: the selected
/// tables, one collective bitmap per height class, 6.5.9), each symbol a generic bitmap or, with SDREFAGG, a refinement
/// of one symbol or an aggregate text region of several (6.5.8.2).
fn symbol_dict(b: &[u8], inputs: Vec<Bitmap>, tables: &[Table]) -> Result<Vec<Bitmap>, Error> {
    let f = be16(b, 0)?;
    if f & 0x100 != 0 {
        return Err(Error::Unsupported("JBIG2 retained contexts"));
    }
    let huff = f & 1 != 0;
    let refagg = f & 2 != 0;
    let template = ((f >> 10) & 3) as u8;
    let rtemplate = ((f >> 12) & 1) as u8;
    let (at, mut i) = if huff { ([(0, 0); 4], 2) } else { at_pixels(b, 2, template)? };
    let rat = if refagg && rtemplate == 0 {
        i += 4;
        refine::at_pixels(b, i - 4)?
    } else {
        [(0, 0); 2]
    };
    let _num_ex = be32(b, i)? as usize;
    let num_new = be32(b, i + 4)? as usize;
    if num_new > MAX_SYMBOLS || inputs.len() + num_new > MAX_SYMBOLS {
        return Err(Error::TooLarge);
    }
    i += 8;
    let n_in = inputs.len();
    let mut all = inputs;
    let mut idlen = 0u32;
    while (1usize << idlen) < n_in + num_new {
        idlen += 1;
    }
    let rt = Refine { template: rtemplate, tpgron: false, at: rat };
    let mut grcx = vec![0u8; if refagg { refine::CONTEXTS } else { 0 }];
    // Arithmetic: one MQ decoder over the data; Huffman: the fax bit reader and the tables of 7.4.2.1.6.
    let mut dec = MqDecoder::new(b.get(i..).unwrap_or(&[]));
    let mut gb = vec![0u8; 1 << 16];
    let (mut iadh, mut iadw, mut iaex, mut iaai) = (Ia::new(), Ia::new(), Ia::new(), Ia::new());
    let mut ta = TextArith::new(idlen);
    let mut bits = Bits { data: b, pos: i * 8 };
    let mut custom = tables.iter();
    let dh_t = select(&mut custom, (f >> 2) & 3, &[&huffman::B4, &huffman::B5])?;
    let dw_t = select(&mut custom, (f >> 4) & 3, &[&huffman::B2, &huffman::B3])?;
    let bm_t = select(&mut custom, if f & 0x40 != 0 { 3 } else { 0 }, &[&huffman::B1])?;
    let agg_t = select(&mut custom, if f & 0x80 != 0 { 3 } else { 0 }, &[&huffman::B1])?;
    let (b1, b15) = (Table::from_std(&huffman::B1), Table::from_std(&huffman::B15));
    let agg_huff = TextHuff {
        fs: Table::from_std(&huffman::B6),
        ds: Table::from_std(&huffman::B8),
        dt: Table::from_std(&huffman::B11),
        rdw: b15.clone(),
        rdh: b15.clone(),
        rdx: b15.clone(),
        rdy: b15.clone(),
        rsize: b1.clone(),
        ids: if idlen == 0 || !(huff && refagg) { None } else { Some(Table::fixed(n_in + num_new, idlen)?) },
    };
    let g = Generic { template, tpgdon: false, at };
    let mut height = 0i64;
    let mut pixels = 0usize;
    let mut classes = 0usize;
    while all.len() - n_in < num_new {
        classes += 1;
        if classes > num_new + 1 {
            return Err(BAD);
        }
        let dh = if huff { dh_t.decode(&mut bits)? } else { iadh.int(&mut dec) };
        height += dh.ok_or(BAD)?;
        if !(1..=1 << 16).contains(&height) {
            return Err(BAD);
        }
        let mut width = 0i64;
        let mut widths: Vec<usize> = Vec::new();
        loop {
            let dw = if huff { dw_t.decode(&mut bits)? } else { iadw.int(&mut dec) };
            let Some(dw) = dw else { break };
            width += dw;
            if !(0..=1 << 16).contains(&width) || all.len() - n_in + widths.len() >= num_new {
                return Err(BAD);
            }
            pixels += width as usize * height as usize;
            if pixels > MAX_DECODED {
                return Err(Error::TooLarge);
            }
            let (wu, hu) = (width as usize, height as usize);
            if huff && !refagg {
                widths.push(wu);
                continue;
            }
            let sym = if !refagg {
                generic(&mut dec, &mut gb, wu, hu, &g, None)?
            } else {
                let n = if huff { agg_t.decode(&mut bits)? } else { iaai.int(&mut dec) }.ok_or(BAD)?;
                if !(1..=MAX_SYMBOLS as i64).contains(&n) {
                    return Err(BAD);
                }
                if n == 1 {
                    // 6.5.8.2.2: one symbol refined by (RDX, RDY).
                    if huff {
                        let id = huffman::read(&mut bits, idlen)? as usize;
                        let rdx = b15.decode(&mut bits)?.ok_or(BAD)?;
                        let rdy = b15.decode(&mut bits)?.ok_or(BAD)?;
                        let size = usize::try_from(b1.decode(&mut bits)?.ok_or(BAD)?).map_err(|_| BAD)?;
                        bits.align();
                        let start = bits.pos / 8;
                        let data = b.get(start..start.saturating_add(size)).ok_or(BAD)?;
                        let mut rdec = MqDecoder::new(data);
                        let r = all.get(id).ok_or(BAD)?;
                        let s = refine::refine(&mut rdec, &mut grcx, wu, hu, r, rdx, rdy, &rt)?;
                        bits.pos = (start + size) * 8;
                        s
                    } else {
                        let id = iaid(&mut dec, &mut ta.id, idlen);
                        let rdx = ta.rdx.int(&mut dec).ok_or(BAD)?;
                        let rdy = ta.rdy.int(&mut dec).ok_or(BAD)?;
                        let r = all.get(id).ok_or(BAD)?;
                        refine::refine(&mut dec, &mut grcx, wu, hu, r, rdx, rdy, &rt)?
                    }
                } else {
                    // 6.5.8.2.1: an aggregate — a text region over the symbols so far (Table 17).
                    let p = TextParams {
                        w: wu,
                        h: hu,
                        logstrips: 0,
                        corner: 1,
                        transposed: false,
                        combop: 0,
                        defpixel: 0,
                        ds: 0,
                        refine: true,
                        rt,
                        num_inst: n as usize,
                        idlen,
                    };
                    if huff {
                        text_decode(&mut Coder::Huff(&mut bits, &agg_huff), &mut grcx, &p, &all)?
                    } else {
                        text_decode(&mut Coder::Arith(&mut dec, &mut ta), &mut grcx, &p, &all)?
                    }
                }
            };
            all.push(sym);
        }
        if huff && !refagg {
            // 6.5.9: BMSIZE, the byte boundary, then the collective bitmap (uncompressed when BMSIZE is 0, else MMR).
            let size = usize::try_from(bm_t.decode(&mut bits)?.ok_or(BAD)?).map_err(|_| BAD)?;
            bits.align();
            let start = bits.pos / 8;
            let total: usize = widths.iter().sum();
            let hu = height as usize;
            let (coll, used) = if size == 0 {
                let stride = total.div_ceil(8);
                let data = b.get(start..start.saturating_add(stride.saturating_mul(hu))).ok_or(BAD)?;
                (unpack(data, total, hu)?, stride * hu)
            } else {
                (mmr_bitmap(b.get(start..start.saturating_add(size)).ok_or(BAD)?, total, hu)?, size)
            };
            bits.pos = (start + used) * 8;
            let mut x0 = 0usize;
            for &wd in &widths {
                all.push(coll.columns(x0, wd)?);
                x0 += wd;
            }
        }
    }
    let mut flags = vec![false; all.len()];
    let (mut at_i, mut cur) = (0usize, false);
    let mut runs = 0usize;
    while at_i < all.len() {
        runs += 1;
        let run = if huff { b1.decode(&mut bits)? } else { iaex.int(&mut dec) }.ok_or(BAD)?;
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

/// A Huffman table selection (7.4.2.1.6, 7.4.3.1.6): a standard table by index, 3 = the next referred table segment.
fn select(custom: &mut core::slice::Iter<'_, Table>, sel: u16, std: &[&huffman::Std]) -> Result<Table, Error> {
    if sel == 3 {
        custom.next().cloned().ok_or(BAD)
    } else {
        std.get(sel as usize).map(|t| Table::from_std(t)).ok_or(BAD)
    }
}

/// The arithmetic decoders a text region reads through (Table 17: shared by a symbol dictionary's aggregates).
struct TextArith {
    dt: Ia,
    fs: Ia,
    ds: Ia,
    it: Ia,
    ri: Ia,
    rdw: Ia,
    rdh: Ia,
    rdx: Ia,
    rdy: Ia,
    id: Vec<u8>,
}

impl TextArith {
    fn new(idlen: u32) -> TextArith {
        let (dt, fs, ds, it, ri) = (Ia::new(), Ia::new(), Ia::new(), Ia::new(), Ia::new());
        let (rdw, rdh, rdx, rdy) = (Ia::new(), Ia::new(), Ia::new(), Ia::new());
        TextArith { dt, fs, ds, it, ri, rdw, rdh, rdx, rdy, id: vec![0; 1 << idlen] }
    }
}

/// A text region's Huffman tables (7.4.3.1.6) and its symbol ID codes (`None`: one symbol, no bits).
struct TextHuff {
    fs: Table,
    ds: Table,
    dt: Table,
    rdw: Table,
    rdh: Table,
    rdx: Table,
    rdy: Table,
    rsize: Table,
    ids: Option<Table>,
}

/// Where a text region's values come from: the MQ decoder and Annex A's decoders, or the bit reader and the tables.
enum Coder<'c, 'd> {
    Arith(&'c mut MqDecoder<'d>, &'c mut TextArith),
    Huff(&'c mut Bits<'d>, &'c TextHuff),
}

impl Coder<'_, '_> {
    /// One integer through the decoder `pick` names or the table `tab` names; `None` is OOB.
    fn int(&mut self, pick: fn(&mut TextArith) -> &mut Ia, tab: fn(&TextHuff) -> &Table) -> Result<Option<i64>, Error> {
        match self {
            Coder::Arith(dec, a) => Ok(pick(a).int(dec)),
            Coder::Huff(b, t) => tab(t).decode(b),
        }
    }

    /// CURT: IAIT, or LOGSBSTRIPS bits under Huffman.
    fn it(&mut self, logstrips: u32) -> Result<i64, Error> {
        match self {
            Coder::Arith(dec, a) => a.it.int(dec).ok_or(BAD),
            Coder::Huff(b, _) => Ok(huffman::read(b, logstrips)? as i64),
        }
    }

    fn id(&mut self, len: u32) -> Result<usize, Error> {
        match self {
            Coder::Arith(dec, a) => Ok(iaid(dec, &mut a.id, len)),
            Coder::Huff(b, t) => match &t.ids {
                Some(ids) => Ok(usize::try_from(ids.decode(b)?.ok_or(BAD)?).map_err(|_| BAD)?),
                None => Ok(0),
            },
        }
    }

    /// RI: IARI, or one bit under Huffman.
    fn ri(&mut self) -> Result<bool, Error> {
        match self {
            Coder::Arith(dec, a) => Ok(a.ri.int(dec).ok_or(BAD)? != 0),
            Coder::Huff(b, _) => Ok(huffman::read(b, 1)? != 0),
        }
    }

    /// 6.4.11: the refined instance of `base` — RDW, RDH, RDX, RDY, then the refinement (under Huffman: RSIZE, the byte
    /// boundary and RSIZE bytes of MQ data with the shared refinement contexts).
    fn refined(&mut self, grcx: &mut [u8], base: &Bitmap, rt: &Refine) -> Result<Bitmap, Error> {
        let rdw = self.int(|a| &mut a.rdw, |t| &t.rdw)?.ok_or(BAD)?;
        let rdh = self.int(|a| &mut a.rdh, |t| &t.rdh)?.ok_or(BAD)?;
        let rdx = self.int(|a| &mut a.rdx, |t| &t.rdx)?.ok_or(BAD)?;
        let rdy = self.int(|a| &mut a.rdy, |t| &t.rdy)?.ok_or(BAD)?;
        let w = usize::try_from(base.w as i64 + rdw).map_err(|_| BAD)?;
        let h = usize::try_from(base.h as i64 + rdh).map_err(|_| BAD)?;
        let (dx, dy) = ((rdw >> 1) + rdx, (rdh >> 1) + rdy);
        match self {
            Coder::Arith(dec, _) => refine::refine(dec, grcx, w, h, base, dx, dy, rt),
            Coder::Huff(b, t) => {
                let size = usize::try_from(t.rsize.decode(b)?.ok_or(BAD)?).map_err(|_| BAD)?;
                b.align();
                let start = b.pos / 8;
                let data = b.data.get(start..start.saturating_add(size)).ok_or(BAD)?;
                let mut dec = MqDecoder::new(data);
                let bm = refine::refine(&mut dec, grcx, w, h, base, dx, dy, rt)?;
                b.pos = (start + size) * 8;
                Ok(bm)
            }
        }
    }
}

/// A text region's decoding parameters (6.4.2).
struct TextParams {
    w: usize,
    h: usize,
    logstrips: u32,
    corner: u8,
    transposed: bool,
    combop: u8,
    defpixel: u8,
    ds: i64,
    refine: bool,
    rt: Refine,
    num_inst: usize,
    idlen: u32,
}

/// 6.4: a text region segment's header (7.4.3.1: flags, Huffman flags, refinement AT, SBNUMINSTANCES, the symbol ID
/// table) and its bitmap.
fn text_region(b: &[u8], syms: &[Bitmap], tables: &[Table]) -> Result<Bitmap, Error> {
    let (w, h, ..) = region_info(b)?;
    let f = be16(b, 17)?;
    let huff = f & 1 != 0;
    let refine = f & 2 != 0;
    let mut i = 19;
    let hf = if huff {
        i += 2;
        be16(b, 19)?
    } else {
        0
    };
    let rtemplate = (f >> 15) as u8;
    let rat = if refine && rtemplate == 0 {
        i += 4;
        refine::at_pixels(b, i - 4)?
    } else {
        [(0, 0); 2]
    };
    let num_inst = be32(b, i)? as usize;
    i += 4;
    let mut idlen = 0u32;
    while (1usize << idlen) < syms.len() {
        idlen += 1;
    }
    let p = TextParams {
        w,
        h,
        logstrips: ((f >> 2) & 3) as u32,
        corner: ((f >> 4) & 3) as u8,
        transposed: f & 0x40 != 0,
        combop: ((f >> 7) & 3) as u8,
        defpixel: ((f >> 9) & 1) as u8,
        ds: {
            let v = ((f >> 10) & 0x1F) as i64;
            if v & 0x10 != 0 { v - 32 } else { v }
        },
        refine,
        rt: Refine { template: rtemplate, tpgron: false, at: rat },
        num_inst,
        idlen,
    };
    if num_inst == 0 {
        return Bitmap::new(w, h, p.defpixel);
    }
    let mut grcx = vec![0u8; if refine { refine::CONTEXTS } else { 0 }];
    if !huff {
        let mut dec = MqDecoder::new(b.get(i..).unwrap_or(&[]));
        let mut ta = TextArith::new(idlen);
        return text_decode(&mut Coder::Arith(&mut dec, &mut ta), &mut grcx, &p, syms);
    }
    let mut custom = tables.iter();
    let rd = [&huffman::B14, &huffman::B15];
    let fs = select(&mut custom, hf & 3, &[&huffman::B6, &huffman::B7])?;
    let ds = select(&mut custom, (hf >> 2) & 3, &[&huffman::B8, &huffman::B9, &huffman::B10])?;
    let dt = select(&mut custom, (hf >> 4) & 3, &[&huffman::B11, &huffman::B12, &huffman::B13])?;
    let rdw = select(&mut custom, (hf >> 6) & 3, &rd)?;
    let rdh = select(&mut custom, (hf >> 8) & 3, &rd)?;
    let rdx = select(&mut custom, (hf >> 10) & 3, &rd)?;
    let rdy = select(&mut custom, (hf >> 12) & 3, &rd)?;
    let rsize = select(&mut custom, if hf & 0x4000 != 0 { 3 } else { 0 }, &[&huffman::B1])?;
    let mut bits = Bits { data: b, pos: i * 8 };
    let ids = Some(huffman::symbol_id_table(&mut bits, syms.len())?);
    let th = TextHuff { fs, ds, dt, rdw, rdh, rdx, rdy, rsize, ids };
    text_decode(&mut Coder::Huff(&mut bits, &th), &mut grcx, &p, syms)
}

/// 6.4.5: the text region decoding procedure, its values through `c`.
fn text_decode(c: &mut Coder, grcx: &mut [u8], p: &TextParams, syms: &[Bitmap]) -> Result<Bitmap, Error> {
    let strips = 1i64 << p.logstrips;
    let mut sb = Bitmap::new(p.w, p.h, p.defpixel)?;
    if p.num_inst == 0 {
        return Ok(sb);
    }
    if syms.is_empty() {
        return Err(BAD);
    }
    let right = p.corner & 2 != 0;
    let bottom = p.corner & 1 == 0;
    let mut strip_t = -(c.int(|a| &mut a.dt, |t| &t.dt)?.ok_or(BAD)? * strips);
    let mut first_s = 0i64;
    let mut n = 0usize;
    while n < p.num_inst {
        strip_t += c.int(|a| &mut a.dt, |t| &t.dt)?.ok_or(BAD)? * strips;
        first_s += c.int(|a| &mut a.fs, |t| &t.fs)?.ok_or(BAD)?;
        let mut cur_s = first_s;
        loop {
            let cur_t = if strips == 1 { 0 } else { c.it(p.logstrips)? };
            let t = strip_t + cur_t;
            let base = syms.get(c.id(p.idlen)?).ok_or(BAD)?;
            let refined;
            let sym = if p.refine && c.ri()? {
                refined = c.refined(grcx, base, &p.rt)?;
                &refined
            } else {
                base
            };
            let (wi, hi) = (sym.w as i64, sym.h as i64);
            // 6.4.5 3c x: the reference corner's side advances S before the symbol is placed ...
            if !p.transposed && right {
                cur_s += wi - 1;
            } else if p.transposed && bottom {
                cur_s += hi - 1;
            }
            let (x, y) = if p.transposed { (t, cur_s) } else { (cur_s, t) };
            let x = if right { x - wi + 1 } else { x };
            let y = if bottom { y - hi + 1 } else { y };
            sb.compose(sym, x, y, p.combop);
            // ... and the other side after.
            if !p.transposed && !right {
                cur_s += wi - 1;
            } else if p.transposed && !bottom {
                cur_s += hi - 1;
            }
            n += 1;
            let Some(d) = c.int(|a| &mut a.ds, |t| &t.ds)? else { break };
            if n >= p.num_inst {
                break;
            }
            cur_s += d + p.ds;
        }
    }
    Ok(sb)
}

/// Packed MSB-first rows (each padded to a byte) as a bitmap.
fn unpack(bits: &[u8], w: usize, h: usize) -> Result<Bitmap, Error> {
    let mut bm = Bitmap::new(w, h, 0)?;
    let row = w.div_ceil(8);
    for y in 0..h {
        for x in 0..w {
            bm.data[y * w + x] = (bits[y * row + x / 8] >> (7 - x % 8)) & 1;
        }
    }
    Ok(bm)
}

/// An MMR bitmap (6.2.6) through the one fax decoder.
fn mmr_bitmap(data: &[u8], w: usize, h: usize) -> Result<Bitmap, Error> {
    unpack(&ccitt::mmr(data, w, h)?, w, h)
}

/// A generic region segment's bitmap (7.4.6); `rows` is the row count closing a region of unknown length (7.2.7).
fn generic_region(b: &[u8], rows: Option<u32>) -> Result<Bitmap, Error> {
    let (w, h, ..) = region_info(b)?;
    let (h, b) = match rows {
        Some(r) => (r as usize, &b[..b.len().saturating_sub(4)]),
        None => (h, b),
    };
    let f = byte(b, 17)?;
    if f & 0x10 != 0 {
        return Err(Error::Unsupported("JBIG2 extended template"));
    }
    let template = (f >> 1) & 3;
    if f & 1 != 0 {
        return mmr_bitmap(b.get(18..).unwrap_or(&[]), w, h);
    }
    let (at, i) = at_pixels(b, 18, template)?;
    let mut dec = MqDecoder::new(b.get(i..).unwrap_or(&[]));
    let mut cx = vec![0u8; 1 << 16];
    generic(&mut dec, &mut cx, w, h, &Generic { template, tpgdon: f & 8 != 0, at }, None)
}

/// Undo JBIG2Decode: the page bitmap (1 = black). `globals` is the decoded /JBIG2Globals stream.
pub fn decode(data: &[u8], globals: Option<&[u8]>) -> Result<Bitmap, Error> {
    let mut segs = match globals {
        Some(g) => segments(g)?,
        None => Vec::new(),
    };
    segs.extend(segments(data)?);
    let mut dicts: BTreeMap<u32, Vec<Bitmap>> = BTreeMap::new();
    let mut tables: BTreeMap<u32, Table> = BTreeMap::new();
    let mut patterns: BTreeMap<u32, Vec<Bitmap>> = BTreeMap::new();
    let mut inter: BTreeMap<u32, Bitmap> = BTreeMap::new();
    let mut page: Option<Bitmap> = None;
    let mut striped = false;
    let mut page_def = 0u8;
    for s in &segs {
        let referred = || -> Vec<Bitmap> { s.refs.iter().filter_map(|r| dicts.get(r)).flatten().cloned().collect() };
        let reftables = || -> Vec<Table> { s.refs.iter().filter_map(|r| tables.get(r)).cloned().collect() };
        match s.ty {
            0 => {
                let ex = symbol_dict(s.data, referred(), &reftables())?;
                dicts.insert(s.num, ex);
            }
            4 | 6 | 7 | 20 | 22 | 23 | 36 | 38 | 39 | 40 | 42 | 43 => {
                let (w, h, x, y, op) = region_info(s.data)?;
                if byte(s.data, 16)? & 8 != 0 {
                    return Err(Error::Unsupported("JBIG2 colour extension"));
                }
                let bm = match s.ty {
                    4 | 6 | 7 => text_region(s.data, &referred(), &reftables())?,
                    20 | 22 | 23 => halftone::region(s.data, s.refs.iter().find_map(|r| patterns.get(r)).ok_or(BAD)?)?,
                    36 | 38 | 39 => generic_region(s.data, s.rows)?,
                    _ => {
                        // 7.4.7.5: the referred intermediate region, else the page's area under the region.
                        let reference = match s.refs.iter().find_map(|r| inter.remove(r)) {
                            Some(r) => r,
                            None => page.as_ref().ok_or(Error::Syntax("JBIG2 region before page information", 0))?.area(x, y, w, h)?,
                        };
                        refine::region(s.data, &reference)?
                    }
                };
                if matches!(s.ty, 4 | 20 | 36 | 40) {
                    inter.insert(s.num, bm);
                    continue;
                }
                let pg = page.as_mut().ok_or(Error::Syntax("JBIG2 region before page information", 0))?;
                if striped && y as usize + bm.h > pg.h {
                    grow(pg, y as usize + bm.h, page_def)?;
                }
                pg.compose(&bm, x, y, op);
            }
            16 => {
                patterns.insert(s.num, halftone::pattern_dict(s.data)?);
            }
            53 => {
                tables.insert(s.num, Table::parse(s.data)?);
            }
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

/// 7.2.7: an immediate generic region (MMR 0) of unknown length ends at the first 0xFF 0xAC after its header, followed
/// by its 4-byte row count: the segment's end and that count.
fn unknown_end(b: &[u8], i: usize, ty: u8) -> Option<(usize, u32)> {
    if ty != 38 && ty != 39 {
        return None;
    }
    let f = *b.get(i + 17)?;
    if f & 1 != 0 {
        return None;
    }
    let mut j = i + 18 + if (f >> 1) & 3 == 0 { 8 } else { 2 };
    while j + 6 <= b.len() {
        if b[j] == 0xFF && b[j + 1] == 0xAC {
            return Some((j + 6, u32::from_be_bytes([b[j + 2], b[j + 3], b[j + 4], b[j + 5]])));
        }
        j += 1;
    }
    None
}
