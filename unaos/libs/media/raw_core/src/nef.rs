// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Nikon NEF's raw strip (RAWCOLOR, rmbp-ledger B724). The raw SubIFD is a CFA strip; three layouts:
//!
//! * `Compression 1`, 16-bit containers (12- or 14-bit samples) — [`crate::decode`]'s plain rows;
//! * `Compression 1`, PACKED 12/14-bit (the strip is exactly `w * h * bits / 8` bytes) — MSB-first rows;
//! * `Compression 34713` — Nikon's Huffman coding, as dcraw's `nikon_load_raw` reads it: a row-sequential bit
//!   stream (no byte stuffing); each sample is a Huffman symbol (low nibble = the difference's bit length, high
//!   nibble = a left shift for the lossy trees) then that many bits; columns 0 and 1 predict from the same column
//!   two rows up (`vpred[row & 1][col]`), the others from two columns left (`hpred[col & 1]`); the predicted value
//!   indexes the LINEARISATION CURVE to the sample.
//!
//! The curve, the predictors' seeds and the Huffman tree are in the maker note (EXIF 0x927C, a "Nikon\0" type-3
//! note: its own TIFF header at +10, offsets relative to it), tag 0x96 (`NEFLinearizationTable`): version bytes
//! (`0x46` = lossless; `0x44 0x20` = a sparse curve interpolated, with a SPLIT row where the tree changes), four
//! SHORTs `vpred`, the curve's size, the curve. Tag 0x3d is the black level (four SHORTs).

use alloc::vec::Vec;

use crate::decode::{BitState, Bits, Huff, Source};
use crate::tiff::Reader;
use crate::Error;

/// dcraw's `nikon_tree`: sixteen code-length counts, then the symbols (zero-padded to 32).
pub const TREES: [[u8; 32]; 6] = [
    // 12-bit lossy
    [0, 1, 5, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 5, 4, 3, 6, 2, 7, 1, 0, 8, 9, 11, 10, 12, 0, 0, 0],
    // 12-bit lossy after split
    [0, 1, 5, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 0x39, 0x5a, 0x38, 0x27, 0x16, 5, 4, 3, 2, 1, 0, 11, 12, 12, 0, 0],
    // 12-bit lossless
    [0, 1, 4, 2, 3, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 5, 4, 6, 3, 7, 2, 8, 1, 9, 0, 10, 11, 12, 0, 0, 0],
    // 14-bit lossy
    [0, 1, 4, 3, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 0, 5, 6, 4, 7, 8, 3, 9, 2, 1, 0, 10, 11, 12, 13, 14, 0],
    // 14-bit lossy after split
    [0, 1, 5, 1, 1, 1, 1, 1, 1, 1, 2, 0, 0, 0, 0, 0, 8, 0x5c, 0x4b, 0x3a, 0x29, 7, 6, 5, 4, 3, 2, 1, 0, 13, 14, 0],
    // 14-bit lossless
    [0, 1, 4, 2, 2, 3, 1, 2, 0, 0, 0, 0, 0, 0, 0, 0, 7, 6, 8, 5, 9, 4, 10, 3, 11, 12, 2, 0, 1, 13, 14, 0],
];

/// The Huffman table of tree `t`.
pub fn tree(t: usize) -> Result<Huff, Error> {
    let tr = TREES.get(t).ok_or(Error::Malformed("NEF tree"))?;
    let counts: [u8; 16] = tr[..16].try_into().map_err(|_| Error::Malformed("NEF tree"))?;
    let n: usize = counts.iter().map(|&c| c as usize).sum();
    Huff::new(&counts, &tr[16..16 + n])
}

/// EXIF's MakerNote.
pub const TAG_MAKER_NOTE: u16 = 0x927C;
/// Nikon: NEFLinearizationTable.
pub const TAG_LINEARIZATION: u16 = 0x96;
/// Nikon: BlackLevel.
pub const TAG_BLACK: u16 = 0x3d;

/// What the maker note tells the decoder.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    /// dcraw's tree index (0 12-bit lossy, 2 12-bit lossless, 3 14-bit lossy, 5 14-bit lossless).
    pub tree: u8,
    pub vpred: [u16; 4],
    /// The row from which the tree after it (`tree + 1`) codes the rest (0 = none).
    pub split: u16,
    /// The linearisation curve, 0x4000 entries (the identity where the note names none).
    pub curve: Vec<u16>,
    /// The largest sample the curve produces.
    pub white: u16,
}

/// Parse tag 0x96's bytes (`b` runs from its first byte to the end of what is held; dcraw reads past the tag's own
/// count for the split row and the 2110-byte skip) for a `bps`-bit strip.
pub fn meta(b: &[u8], le: bool, bps: u16) -> Option<Meta> {
    if !(bps == 12 || bps == 14) {
        return None;
    }
    let g16 = |p: usize| b.get(p..p + 2).map(|s| if le { u16::from_le_bytes([s[0], s[1]]) } else { u16::from_be_bytes([s[0], s[1]]) });
    let (ver0, ver1) = (*b.first()?, *b.get(1)?);
    let mut p = 2usize;
    if ver0 == 0x49 || ver1 == 0x58 {
        p += 2110;
    }
    let tree = if ver0 == 0x46 { 2 } else { 0 } + if bps == 14 { 3 } else { 0 };
    let vpred = [g16(p)?, g16(p + 2)?, g16(p + 4)?, g16(p + 6)?];
    p += 8;
    let mut max = (1usize << bps) & 0x7fff;
    let csize = g16(p)? as usize;
    p += 2;
    let step = if csize > 1 { max / (csize - 1) } else { 0 };
    let mut curve: Vec<u16> = Vec::new();
    curve.try_reserve_exact(0x10000).ok()?;
    curve.extend((0..0x10000u32).map(|i| i as u16));
    let mut split = 0u16;
    let mut read = false;
    if ver0 == 0x44 && ver1 == 0x20 && step > 0 {
        for i in 0..csize {
            if i * step < curve.len() {
                curve[i * step] = g16(p + 2 * i)?;
            }
        }
        for i in 0..max {
            let a = i - i % step;
            let hi = curve.get(a + step).copied().unwrap_or(curve[a]) as u32;
            curve[i] = ((curve[a] as u32 * (step - i % step) as u32 + hi * (i % step) as u32) / step as u32) as u16;
        }
        split = g16(562).unwrap_or(0);
        read = true;
    } else if ver0 != 0x46 && csize <= 0x4001 && csize > 0 {
        for i in 0..csize {
            curve[i] = g16(p + 2 * i)?;
        }
        max = csize;
        read = true;
    }
    while max >= 2 && curve[max - 2] == curve[max - 1] {
        max -= 1;
    }
    let white = if read && max >= 1 { curve[max - 1] } else { ((1u32 << bps) - 1) as u16 };
    curve.truncate(0x4000);
    Some(Meta { tree, vpred, split, curve, white })
}

/// The maker note of the EXIF IFD at `exif`: its Meta (tag 0x96, `None` when absent) and black level (tag 0x3d).
pub fn maker(r: &Reader, exif: usize, bps: u16) -> (Option<Meta>, Option<u16>) {
    let none = (None, None);
    let Ok((ents, _)) = r.ifd(exif) else { return none };
    let Some(e) = ents.iter().find(|e| e.tag == TAG_MAKER_NOTE) else { return none };
    let Some((s, l)) = r.value_span(e) else { return none };
    if l < 18 || r.b.get(s..s + 6) != Some(b"Nikon\0") {
        return none;
    }
    let Some(inner) = r.b.get(s + 10..) else { return none };
    if !crate::is_tiff(inner) {
        return none;
    }
    let ir = Reader { b: inner, le: inner[0] == b'I' };
    let Some(off) = ir.u32(4) else { return none };
    let Ok((ie, _)) = ir.ifd(off as usize) else { return none };
    let mut m = None;
    let mut black = None;
    for e in &ie {
        match e.tag {
            TAG_LINEARIZATION => {
                if let Some((vs, _)) = ir.value_span(e) {
                    m = meta(&inner[vs..], ir.le, bps);
                }
            }
            TAG_BLACK if e.count >= 4 => {
                let v: Vec<u32> = (0..4).filter_map(|i| ir.uint(e, i)).collect();
                if v.len() == 4 {
                    black = Some((v.iter().sum::<u32>() / 4) as u16);
                }
            }
            _ => {}
        }
    }
    (m, black)
}

/// The compressed strip's decoder.
#[derive(Clone, Debug)]
pub struct Plan {
    pub w: usize,
    pub h: usize,
    pub meta: Meta,
    huff: [Huff; 2],
    len: u64,
}

impl Plan {
    pub fn new(w: usize, h: usize, meta: &Meta, len: u64) -> Result<Plan, Error> {
        if meta.curve.len() < 0x4000 {
            return Err(Error::Malformed("NEF curve"));
        }
        let t = meta.tree as usize;
        let h0 = tree(t)?;
        // A split only exists for the lossy trees (0, 3): their "after split" twin is the next one.
        let h1 = if meta.split > 0 && (t == 0 || t == 3) { tree(t + 1)? } else { h0.clone() };
        Ok(Plan { w, h, meta: meta.clone(), huff: [h0, h1], len })
    }

    /// Hand rows `0..rows` to `f`, in order (the stream is row-sequential).
    pub fn stream<S: Source>(&self, src: &mut S, rows: usize, mut f: impl FnMut(usize, &[u16])) -> Result<(), Error> {
        let m = &self.meta;
        let mut b = Bits::new(BitState::at(0), self.len, false);
        let mut vpred = [[m.vpred[0], m.vpred[1]], [m.vpred[2], m.vpred[3]]];
        let mut hpred = [0u16; 2];
        let mut row = crate::zeroed::<u16>(self.w)?;
        let mut huff = &self.huff[0];
        for y in 0..rows.min(self.h) {
            if m.split > 0 && y == m.split as usize {
                huff = &self.huff[1];
            }
            for x in 0..self.w {
                let i = huff.decode(&mut b, src)? as u32;
                let (len, shl) = (i & 15, i >> 4);
                let diff: i32 = if len == 0 {
                    0
                } else {
                    if shl > len {
                        return Err(Error::Malformed("NEF difference"));
                    }
                    let v = b.get(src, len - shl)? as i32;
                    let mut d = (((v << 1) + 1) << shl) >> 1;
                    if d & (1 << (len - 1)) == 0 {
                        d -= (1 << len) - (shl == 0) as i32;
                    }
                    d
                };
                if x < 2 {
                    vpred[y & 1][x] = vpred[y & 1][x].wrapping_add(diff as u16);
                    hpred[x] = vpred[y & 1][x];
                } else {
                    hpred[x & 1] = hpred[x & 1].wrapping_add(diff as u16);
                }
                let idx = (hpred[x & 1] as i16 as i32).clamp(0, 0x3fff) as usize;
                row[x] = m.curve[idx];
            }
            f(y, &row);
        }
        Ok(())
    }
}
