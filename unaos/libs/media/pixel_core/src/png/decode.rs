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
//! compares with those chunks stripped. sBIT is ignored (it is advisory).
//!
//! APNG (ANIMWEBP, the APNG specification — PNG 3rd edition §11.3.6 / the Mozilla APNG 1.0 text):
//! `acTL` (frame count, plays), `fcTL` (sequence number, frame rectangle, delay fraction, dispose_op,
//! blend_op), `fdAT` (sequence number + frame data, possibly split across chunks). The default image
//! is frame 0 when an `fcTL` precedes the first `IDAT`, else it is not shown and the animation is the
//! `fdAT` frames alone (Blink shows the first of those). Every frame is decoded with the IHDR's colour
//! type, depth, interlace, PLTE and tRNS at its own size, then composited onto a transparent canvas:
//! `APNG_BLEND_OP_SOURCE` copies the rectangle, `APNG_BLEND_OP_OVER` blends it with
//! [`crate::blend_nonpremult`] (a transparent destination takes the source as is, as Blink's
//! `BlendRGBARaw` does); dispose `NONE` keeps, `BACKGROUND` clears the rectangle to transparent black,
//! `PREVIOUS` restores the canvas as it was before the frame (on frame 0: `BACKGROUND`, per the spec).
//! Delays are `round(1000 * num / den)` ms with `den = 0` read as 100. A sequence-number break, a
//! frame outside the canvas, or a frame that fails to decode ends the animation at the frames before
//! it (a broken FIRST frame falls back to the default image).

use alloc::vec;
use alloc::vec::Vec;

use crate::{Error, Frame, Image, be32, crc};

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

#[derive(Clone)]
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
    // APNG state: acTL (frames, plays), the fcTL frames with their fdAT data, the next sequence number.
    let mut actl: Option<u32> = None;
    let mut afr: Vec<ApngFrame> = Vec::new();
    let mut default_is_frame = false;
    let mut next_seq = 0u32;
    let mut anim_broken = false;

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
            b"acTL" => {
                if len == 8 && actl.is_none() && !seen_idat && be32(data, 0) > 0 {
                    actl = Some(be32(data, 4));
                }
            }
            b"fcTL" => {
                let Some(h) = hdr.as_ref() else { continue };
                if anim_broken {
                    continue;
                }
                match parse_fctl(data, h, next_seq, !seen_idat && afr.is_empty()) {
                    Some(fr) => {
                        if !seen_idat {
                            default_is_frame = true;
                        }
                        afr.push(fr);
                        next_seq += 1;
                    }
                    None => anim_broken = true,
                }
            }
            b"fdAT" => {
                if anim_broken || len < 4 {
                    continue;
                }
                // fdAT belongs to the last fcTL, which must be a frame after the IDAT.
                let ok = be32(data, 0) == next_seq && seen_idat && !(default_is_frame && afr.len() == 1);
                match afr.last_mut() {
                    Some(fr) if ok => {
                        fr.data.extend_from_slice(&data[4..]);
                        next_seq += 1;
                    }
                    _ => anim_broken = true,
                }
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

    // An APNG: composite its frames; a still (or an APNG whose animation is unusable) is the IDAT.
    if let Some(plays) = actl {
        if let Some(img) = composite_apng(&h, &palette, &trns, &idat, default_is_frame, afr, plays)? {
            return Ok(img);
        }
    }
    let rgba = decode_pixels(&h, &palette, &trns, &idat)?;
    Ok(Image::still(h.width as u32, h.height as u32, rgba))
}

/// Decode one image's zlib stream (the IDAT, or one APNG frame's fdAT data) at the size `h` states,
/// to straight RGBA8.
fn decode_pixels(h: &Header, palette: &[[u8; 4]], trns: &Trns, idat: &[u8]) -> Result<Vec<u8>, Error> {
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
    Ok(rgba)
}


/// One APNG frame: its fcTL and (after the IDAT) its concatenated fdAT data.
struct ApngFrame {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    delay_ms: u32,
    dispose: u8,
    blend_over: bool,
    data: Vec<u8>,
}

/// Parse an fcTL (26 bytes): `None` when the sequence number is out of order or the rectangle is
/// empty, leaves the canvas, or (for a default-image frame) is not the whole canvas at (0, 0).
fn parse_fctl(d: &[u8], h: &Header, seq: u32, is_default: bool) -> Option<ApngFrame> {
    if d.len() < 26 || be32(d, 0) != seq {
        return None;
    }
    let (w, ht, x, y) = (be32(d, 4) as usize, be32(d, 8) as usize, be32(d, 12) as usize, be32(d, 16) as usize);
    if w == 0 || ht == 0 || x.checked_add(w)? > h.width || y.checked_add(ht)? > h.height {
        return None;
    }
    if is_default && (x, y, w, ht) != (0, 0, h.width, h.height) {
        return None;
    }
    let num = u16::from_be_bytes([d[20], d[21]]) as u32;
    let den = match u16::from_be_bytes([d[22], d[23]]) as u32 {
        0 => 100,
        n => n,
    };
    let (dispose, blend) = (d[24], d[25]);
    if dispose > 2 || blend > 1 {
        return None;
    }
    Some(ApngFrame { x, y, w, h: ht, delay_ms: (num * 1000 + den / 2) / den, dispose, blend_over: blend == 1, data: Vec::new() })
}

/// Composite the APNG frames onto the canvas. `Ok(None)` = no usable frame: show the default image.
fn composite_apng(
    h: &Header,
    palette: &[[u8; 4]],
    trns: &Trns,
    idat: &[u8],
    default_is_frame: bool,
    afr: Vec<ApngFrame>,
    plays: u32,
) -> Result<Option<Image>, Error> {
    let (cw, ch) = (h.width, h.height);
    let len = cw * ch * 4;
    let mut canvas = crate::zeroed(len)?;
    let mut frames: Vec<Frame> = Vec::new();
    // The canvas before the previous frame was drawn (for dispose PREVIOUS), and that frame.
    let mut saved: Option<Vec<u8>> = None;
    let mut prev: Option<(usize, usize, usize, usize, u8)> = None;
    for (i, fr) in afr.iter().enumerate() {
        let sub = Header { width: fr.w, height: fr.h, ..h.clone() };
        let px = if i == 0 && default_is_frame {
            decode_pixels(&sub, palette, trns, idat)?
        } else {
            match decode_pixels(&sub, palette, trns, &fr.data) {
                Ok(px) => px,
                Err(e) if frames.is_empty() && !default_is_frame => {
                    let _ = e;
                    return Ok(None);
                }
                Err(_) => break,
            }
        };
        // The previous frame's disposal brings the canvas to this frame's starting state.
        if let Some((x, y, w, ht, dispose)) = prev {
            match dispose {
                1 => crate::webp::clear_rect(&mut canvas, cw, x, y, w, ht),
                2 => {
                    if let Some(s) = saved.take() {
                        canvas = s;
                    }
                }
                _ => {}
            }
        }
        // Dispose PREVIOUS on the first frame acts as BACKGROUND (the canvas before it is clear).
        let dispose = if frames.is_empty() && fr.dispose == 2 { 1 } else { fr.dispose };
        saved = if dispose == 2 { Some(canvas.clone()) } else { None };
        for row in 0..fr.h {
            for col in 0..fr.w {
                let s = (row * fr.w + col) * 4;
                let d = ((fr.y + row) * cw + fr.x + col) * 4;
                let src = [px[s], px[s + 1], px[s + 2], px[s + 3]];
                let out = if !fr.blend_over || src[3] == 255 || canvas[d + 3] == 0 {
                    src
                } else if src[3] == 0 {
                    [canvas[d], canvas[d + 1], canvas[d + 2], canvas[d + 3]]
                } else {
                    crate::blend_srcover_f32(src, [canvas[d], canvas[d + 1], canvas[d + 2], canvas[d + 3]])
                };
                canvas[d..d + 4].copy_from_slice(&out);
            }
        }
        let mut snap = Vec::new();
        snap.try_reserve_exact(len).map_err(|_| Error::OutOfMemory)?;
        snap.extend_from_slice(&canvas);
        frames.push(Frame { delay_ms: fr.delay_ms, rgba: snap });
        prev = Some((fr.x, fr.y, fr.w, fr.h, dispose));
    }
    if frames.is_empty() {
        return Ok(None);
    }
    let mut img = Image::still(cw as u32, ch as u32, frames[0].rgba.clone());
    if frames.len() > 1 {
        img.frames = Some(frames);
        img.loop_count = crate::loop_from_plays(plays);
    }
    Ok(Some(img))
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

