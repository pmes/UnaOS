// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The raw strip, one row at a time: every supported coding is row-addressable (a row is a fixed number of
//! bytes), so a caller can stream a 50 MB strip through a few rows of memory (the kernel's Facet does).
//!
//! * `Compression 1` — 16-bit sample containers in the file's byte order (`BitsPerSample` 12, 14 or 16).
//! * `Compression 32767`, `BitsPerSample 8` — Sony's compressed raw ("cRAW", ARW 2.x), as dcraw's
//!   `sony_arw2_load_raw` reads it: a row is `width` bytes of 16-byte blocks; a block carries sixteen pixels of
//!   ONE colour (every other column of a 32-column span — the first block the even columns, the next the odd):
//!   bits 0..11 max, 11..22 min, 22..26 the index of the max pixel, 26..30 the index of the min pixel, then the
//!   other fourteen pixels as 7-bit deltas above min, shifted left by `sh` (the smallest of 0..=3 with
//!   `0x80 << sh > max - min`), clamped to 11 bits. Each 11-bit value goes through the tone curve
//!   (`curve[v << 1] >> 2`) to the 14-bit sample.

use alloc::vec::Vec;

use crate::tiff::Strip;
use crate::Error;

/// Decodes rows of one [`Strip`].
pub struct RowDecoder {
    pub width: usize,
    pub height: usize,
    kind: Kind,
    /// The largest sample value the coding can produce (the default white level).
    pub max_code: u16,
}

enum Kind {
    Plain { le: bool },
    Craw { curve: Vec<u16> },
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
    pub fn new(s: &Strip) -> Result<Self, Error> {
        let (w, h) = (s.width as usize, s.height as usize);
        let (kind, max_code) = match (s.compression, s.bits) {
            (1, 9..=16) => (Kind::Plain { le: s.le }, ((1u32 << s.bits) - 1) as u16),
            (1, _) => return Err(Error::Unsupported("uncompressed sample depth")),
            (32767, 8) => {
                let curve = sony_curve(s.curve);
                let top = curve[0x7ff << 1] >> 2;
                (Kind::Craw { curve }, top)
            }
            (32767, 12) => return Err(Error::Unsupported("Sony packed 12-bit (owed: a real file)")),
            (32767, _) => return Err(Error::Unsupported("Sony ARW1 (owed)")),
            (7, _) => return Err(Error::Unsupported("lossless JPEG raw (owed)")),
            _ => return Err(Error::Unsupported("raw compression")),
        };
        let d = RowDecoder { width: w, height: h, kind, max_code };
        if (d.row_bytes() as u64) * (h as u64) > s.len {
            return Err(Error::Truncated);
        }
        Ok(d)
    }

    /// Bytes one coded row occupies.
    pub fn row_bytes(&self) -> usize {
        match self.kind {
            Kind::Plain { .. } => self.width * 2,
            Kind::Craw { .. } => self.width,
        }
    }

    /// Decode one row (`src` = exactly [`row_bytes`](Self::row_bytes)) into `out` (`width` samples).
    pub fn row(&self, src: &[u8], out: &mut [u16]) -> Result<(), Error> {
        if src.len() < self.row_bytes() || out.len() < self.width {
            return Err(Error::Truncated);
        }
        match &self.kind {
            Kind::Plain { le } => {
                for (o, p) in out.iter_mut().zip(src.chunks_exact(2)) {
                    *o = if *le { u16::from_le_bytes([p[0], p[1]]) } else { u16::from_be_bytes([p[0], p[1]]) };
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
        }
        Ok(())
    }

    /// The whole mosaic from a strip held in memory (`strip` = the strip's bytes).
    pub fn all(&self, strip: &[u8]) -> Result<Vec<u16>, Error> {
        let n = self.width.checked_mul(self.height).ok_or(Error::TooLarge)?;
        let mut v = crate::zeroed::<u16>(n)?;
        let rb = self.row_bytes();
        for y in 0..self.height {
            let src = strip.get(y * rb..(y + 1) * rb).ok_or(Error::Truncated)?;
            self.row(src, &mut v[y * self.width..(y + 1) * self.width])?;
        }
        Ok(v)
    }
}
