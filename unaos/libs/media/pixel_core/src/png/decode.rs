// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PNG decoder — ISO/IEC 15948:2004 / W3C PNG Specification (Third Edition).
//!
//! Covered: the signature (§5.2); chunk layout with CRC-32 (§5.3) — a bad CRC on a CRITICAL chunk is
//! refused, a bad CRC on an ancillary chunk drops that chunk (§13.2's "ancillary chunk errors may be
//! ignored"); IHDR (§11.2.2) with every legal colour-type/bit-depth pair of Table 11.1 — greyscale
//! 1/2/4/8/16, truecolour 8/16, indexed 1/2/4/8, greyscale+alpha 8/16, truecolour+alpha 8/16; PLTE
//! (§11.2.3); IDAT concatenation + zlib (§10, RFC 1950/1951, through [`crate::inflate`]); all five
//! filter types with the Paeth predictor (§9); Adam7 interlacing (§8.2); tRNS for all three colour
//! types that allow it (§11.3.2.1).
//!
//! Sample scaling to 8 bits (§13.12): sub-8-bit samples are scaled up exactly (`v * 255 / (2^d - 1)`,
//! i.e. bit replication); 16-bit samples are reduced by `(v * 255 + 32895) >> 16`, the correctly
//! rounded `v / 257`.
//!
//! NOT applied: gAMA, cHRM, sRGB, iCCP (colour management, §12) — the samples are returned as stored.
//! A browser that colour-manages will show a file carrying gAMA != 1/2.2 differently; the oracle
//! compares with those chunks stripped. sBIT is ignored (it is advisory). APNG (acTL/fcTL/fdAT) is
//! OWED: an APNG decodes as its default image, which is what the spec requires of a non-APNG decoder.

use alloc::vec;
use alloc::vec::Vec;

use crate::{Error, Image, be32, crc};

const SIG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Adam7 passes: (x start, y start, x step, y step) — §8.2, Table 8.1.
const ADAM7: [(usize, usize, usize, usize); 7] = [
    (0, 0, 8, 8),
    (4, 0, 8, 8),
    (0, 4, 4, 8),
    (2, 0, 4, 4),
    (0, 2, 2, 4),
    (1, 0, 2, 2),
    (0, 1, 1, 2),
];

struct Header {
    width: usize,
    height: usize,
    depth: u8,
    ctype: u8,
    interlace: bool,
}

impl Header {
    /// Samples per pixel (Table 11.1).
    fn channels(&self) -> usize {
        match self.ctype {
            0 | 3 => 1,
            2 => 3,
            4 => 2,
            _ => 4,
        }
    }
    fn bits_per_pixel(&self) -> usize {
        self.channels() * self.depth as usize
    }
    /// Bytes in one filtered scanline of `w` pixels, excluding the filter-type byte.
    fn row_bytes(&self, w: usize) -> usize {
        (w * self.bits_per_pixel()).div_ceil(8)
    }
    /// The filter's `bpp` (§9.2): bytes per complete pixel, at least 1.
    fn filter_bpp(&self) -> usize {
        self.bits_per_pixel().div_ceil(8).max(1)
    }
}

/// Transparency as tRNS states it, in the image's own sample space.
enum Trns {
    None,
    Gray(u16),
    Rgb(u16, u16, u16),
}

/// Decode a PNG file to straight RGBA8.
pub fn decode(bytes: &[u8]) -> Result<Image, Error> {
    if bytes.len() < 8 || bytes[..8] != SIG {
        return Err(Error::Malformed("png signature"));
    }
    let mut pos = 8usize;
    let mut hdr: Option<Header> = None;
    let mut palette: Vec<[u8; 4]> = Vec::new();
    let mut trns = Trns::None;
    let mut idat: Vec<u8> = Vec::new();
    let mut seen_idat = false;
    let mut idat_done = false;
    let mut seen_iend = false;

    while pos + 12 <= bytes.len() {
        let len = be32(bytes, pos) as usize;
        if len > 0x7FFF_FFFF || pos + 12 + len > bytes.len() {
            return Err(Error::Truncated);
        }
        let kind = &bytes[pos + 4..pos + 8];
        let data = &bytes[pos + 8..pos + 8 + len];
        let want = be32(bytes, pos + 8 + len);
        let got = crc::crc32(&bytes[pos + 4..pos + 8 + len]);
        let critical = kind[0] & 0x20 == 0;
        pos += 12 + len;
        if got != want {
            if critical {
                return Err(Error::Checksum("png chunk crc"));
            }
            continue;
        }
        if seen_idat && kind != b"IDAT" {
            idat_done = true;
        }
        match kind {
            b"IHDR" => {
                if hdr.is_some() || len != 13 {
                    return Err(Error::Malformed("IHDR"));
                }
                let width = be32(data, 0);
                let height = be32(data, 4);
                let (depth, ctype) = (data[8], data[9]);
                let ok = matches!(
                    (ctype, depth),
                    (0, 1 | 2 | 4 | 8 | 16) | (2, 8 | 16) | (3, 1 | 2 | 4 | 8) | (4, 8 | 16) | (6, 8 | 16)
                );
                if !ok {
                    return Err(Error::Malformed("IHDR colour type / bit depth"));
                }
                if data[10] != 0 || data[11] != 0 {
                    return Err(Error::Malformed("IHDR compression/filter method"));
                }
                if data[12] > 1 {
                    return Err(Error::Malformed("IHDR interlace method"));
                }
                if width > 0x7FFF_FFFF || height > 0x7FFF_FFFF {
                    return Err(Error::Malformed("IHDR dimension above 2^31-1"));
                }
                crate::rgba_len(width, height)?;
                hdr = Some(Header {
                    width: width as usize,
                    height: height as usize,
                    depth,
                    ctype,
                    interlace: data[12] == 1,
                });
            }
            b"PLTE" => {
                if hdr.is_none() || len % 3 != 0 || len == 0 || len > 768 || seen_idat {
                    return Err(Error::Malformed("PLTE"));
                }
                palette = data.chunks_exact(3).map(|c| [c[0], c[1], c[2], 255]).collect();
            }
            b"tRNS" => {
                let h = hdr.as_ref().ok_or(Error::Malformed("tRNS before IHDR"))?;
                match h.ctype {
                    3 => {
                        // Entries beyond the palette are an error per §11.3.2.1; be lenient and
                        // apply only the ones that land on a palette slot.
                        for (i, &a) in data.iter().enumerate() {
                            if let Some(p) = palette.get_mut(i) {
                                p[3] = a;
                            }
                        }
                    }
                    0 if len >= 2 => trns = Trns::Gray(u16::from_be_bytes([data[0], data[1]])),
                    2 if len >= 6 => {
                        trns = Trns::Rgb(
                            u16::from_be_bytes([data[0], data[1]]),
                            u16::from_be_bytes([data[2], data[3]]),
                            u16::from_be_bytes([data[4], data[5]]),
                        )
                    }
                    _ => {} // tRNS is prohibited for types 4 and 6: ignored as ancillary.
                }
            }
            b"IDAT" => {
                if hdr.is_none() {
                    return Err(Error::Malformed("IDAT before IHDR"));
                }
                if idat_done {
                    return Err(Error::Malformed("IDAT chunks not consecutive"));
                }
                seen_idat = true;
                idat.extend_from_slice(data);
            }
            b"IEND" => {
                seen_iend = true;
                break;
            }
            _ => {
                if critical {
                    return Err(Error::Unsupported("unknown critical png chunk"));
                }
            }
        }
    }
    let h = hdr.ok_or(Error::Malformed("no IHDR"))?;
    if !seen_idat {
        return Err(Error::Malformed("no IDAT"));
    }
    let _ = seen_iend; // a missing IEND is tolerated: every pixel was already delivered.
    if h.ctype == 3 && palette.is_empty() {
        return Err(Error::Malformed("indexed image without PLTE"));
    }

    // The exact filtered-stream size the header implies.
    let mut raw_len = 0usize;
    if h.interlace {
        for &(x0, y0, dx, dy) in &ADAM7 {
            let (pw, ph) = pass_dims(h.width, h.height, x0, y0, dx, dy);
            if pw > 0 && ph > 0 {
                raw_len += ph * (1 + h.row_bytes(pw));
            }
        }
    } else {
        raw_len = h.height * (1 + h.row_bytes(h.width));
    }
    let raw = crate::zlib_decompress(&idat, raw_len)?;
    if raw.len() < raw_len {
        return Err(Error::Truncated);
    }

    let mut rgba = crate::zeroed(h.width * h.height * 4)?;
    if h.interlace {
        let mut off = 0usize;
        for &(x0, y0, dx, dy) in &ADAM7 {
            let (pw, ph) = pass_dims(h.width, h.height, x0, y0, dx, dy);
            if pw == 0 || ph == 0 {
                continue;
            }
            let n = ph * (1 + h.row_bytes(pw));
            let mut sub = raw[off..off + n].to_vec();
            off += n;
            unfilter(&h, pw, ph, &mut sub)?;
            let mut line = vec![0u8; pw * 4];
            let rb = 1 + h.row_bytes(pw);
            for py in 0..ph {
                expand_row(&h, &palette, &trns, &sub[py * rb + 1..(py + 1) * rb], pw, &mut line);
                let y = y0 + py * dy;
                for px in 0..pw {
                    let x = x0 + px * dx;
                    let d = (y * h.width + x) * 4;
                    rgba[d..d + 4].copy_from_slice(&line[px * 4..px * 4 + 4]);
                }
            }
        }
    } else {
        let mut raw = raw;
        unfilter(&h, h.width, h.height, &mut raw)?;
        let rb = 1 + h.row_bytes(h.width);
        for y in 0..h.height {
            let out = &mut rgba[y * h.width * 4..(y + 1) * h.width * 4];
            expand_row(&h, &palette, &trns, &raw[y * rb + 1..(y + 1) * rb], h.width, out);
        }
    }
    Ok(Image::still(h.width as u32, h.height as u32, rgba))
}

/// Pixel dimensions of one Adam7 pass (§8.2).
fn pass_dims(w: usize, h: usize, x0: usize, y0: usize, dx: usize, dy: usize) -> (usize, usize) {
    let pw = if w > x0 { (w - x0).div_ceil(dx) } else { 0 };
    let ph = if h > y0 { (h - y0).div_ceil(dy) } else { 0 };
    (pw, ph)
}

/// Reverse the per-scanline filters in place (§9.2–9.4). `data` is `rows` lines of
/// `1 + row_bytes(w)` bytes, each starting with its filter-type byte; the first line's "previous
/// line" is all zeros.
fn unfilter(h: &Header, w: usize, rows: usize, data: &mut [u8]) -> Result<(), Error> {
    let rb = h.row_bytes(w);
    let stride = rb + 1;
    let bpp = h.filter_bpp();
    for y in 0..rows {
        let (before, cur) = data.split_at_mut(y * stride);
        let prev: &[u8] = if y == 0 { &[] } else { &before[(y - 1) * stride + 1..y * stride] };
        let ft = cur[0];
        let line = &mut cur[1..stride];
        let up = |i: usize| -> u8 { if prev.is_empty() { 0 } else { prev[i] } };
        match ft {
            0 => {}
            1 => {
                for i in bpp..rb {
                    line[i] = line[i].wrapping_add(line[i - bpp]);
                }
            }
            2 => {
                if !prev.is_empty() {
                    for i in 0..rb {
                        line[i] = line[i].wrapping_add(prev[i]);
                    }
                }
            }
            3 => {
                for i in 0..rb {
                    let a = if i >= bpp { line[i - bpp] as u16 } else { 0 };
                    let b = up(i) as u16;
                    line[i] = line[i].wrapping_add(((a + b) >> 1) as u8);
                }
            }
            4 => {
                for i in 0..rb {
                    let a = if i >= bpp { line[i - bpp] } else { 0 };
                    let b = up(i);
                    let c = if i >= bpp { up(i - bpp) } else { 0 };
                    line[i] = line[i].wrapping_add(paeth(a, b, c));
                }
            }
            _ => return Err(Error::Malformed("png filter type")),
        }
    }
    Ok(())
}

/// The Paeth predictor (§9.4), exactly as the spec writes it — the tie order is part of the format.
#[inline]
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let pa = (p - a as i16).abs();
    let pb = (p - b as i16).abs();
    let pc = (p - c as i16).abs();
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// 16 → 8 bits, correctly rounded (`round(v * 255 / 65535)`).
#[inline]
fn s16(v: u16) -> u8 {
    ((v as u32 + 128) / 257) as u8
}

/// Convert one unfiltered scanline of `w` pixels to RGBA8.
fn expand_row(h: &Header, pal: &[[u8; 4]], trns: &Trns, line: &[u8], w: usize, out: &mut [u8]) {
    let d = h.depth as usize;
    // The `i`-th sample of the line at the image's bit depth (packed MSB-first, §7.2).
    let sample = |i: usize| -> u16 {
        match d {
            16 => u16::from_be_bytes([line[i * 2], line[i * 2 + 1]]),
            8 => line[i] as u16,
            _ => {
                let bit = i * d;
                let byte = line[bit / 8];
                let shift = 8 - d - (bit % 8);
                ((byte >> shift) as u16) & ((1u16 << d) - 1)
            }
        }
    };
    let to8 = |v: u16| -> u8 {
        match d {
            16 => s16(v),
            8 => v as u8,
            _ => (v as u32 * 255 / ((1u32 << d) - 1)) as u8,
        }
    };
    for x in 0..w {
        let o = &mut out[x * 4..x * 4 + 4];
        match h.ctype {
            0 => {
                let v = sample(x);
                let g = to8(v);
                let a = match trns {
                    Trns::Gray(t) if *t == v => 0,
                    _ => 255,
                };
                o.copy_from_slice(&[g, g, g, a]);
            }
            2 => {
                let (r, g, b) = (sample(x * 3), sample(x * 3 + 1), sample(x * 3 + 2));
                let a = match trns {
                    Trns::Rgb(tr, tg, tb) if (*tr, *tg, *tb) == (r, g, b) => 0,
                    _ => 255,
                };
                o.copy_from_slice(&[to8(r), to8(g), to8(b), a]);
            }
            3 => {
                let i = sample(x) as usize;
                // An index past the palette is an error per §11.2.3; render it opaque black as
                // libpng's "benign" mode does rather than refusing the whole image.
                o.copy_from_slice(&pal.get(i).copied().unwrap_or([0, 0, 0, 255]));
            }
            4 => {
                let g = to8(sample(x * 2));
                o.copy_from_slice(&[g, g, g, to8(sample(x * 2 + 1))]);
            }
            _ => {
                for c in 0..4 {
                    o[c] = to8(sample(x * 4 + c));
                }
            }
        }
    }
}
