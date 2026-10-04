// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! GIF decoder — "Graphics Interchange Format, Version 89a" (CompuServe, 1990); 87a files decode too.
//!
//! Covered: Header + Logical Screen Descriptor (§17–18); Global and Local Color Tables (§19, §21);
//! Image Descriptor (§20) with interlaced row order (Appendix E); Table-Based Image Data — variable-
//! length-code LZW with Clear and End-of-Information codes, code size growth to 12 bits and the
//! deferred clear at a full table (§22, Appendix F); Graphic Control Extension (§23): disposal methods
//! 0–3, transparency index, delay time; the NETSCAPE2.0 / ANIMEXTS1.0 Application Extension loop count;
//! Comment / Plain Text / unknown extensions skipped (Plain Text is not rendered, as no browser does).
//!
//! Composition follows the browsers, not the 1990 text where they differ: the canvas starts fully
//! TRANSPARENT (the Background Color Index is ignored, as Chromium and Firefox ignore it), and
//! disposal 2 restores the frame rectangle to transparent. Each [`crate::Frame`] is the whole canvas
//! as it looks while that frame is displayed. A frame whose LZW data ends early keeps the pixels it
//! did deliver (browsers show partial frames); a file with no image at all is refused.

use alloc::vec;
use alloc::vec::Vec;

use crate::{Error, Frame, Image, le16};

/// Decode a GIF to RGBA8 (first frame) plus every composited frame when animated.
pub fn decode(b: &[u8]) -> Result<Image, Error> {
    if b.len() < 13 || !(b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a")) {
        return Err(Error::Malformed("gif header"));
    }
    let (w, h) = (le16(b, 6) as u32, le16(b, 8) as u32);
    let packed = b[10];
    let mut p = 13usize;
    let mut gct: Vec<[u8; 3]> = Vec::new();
    if packed & 0x80 != 0 {
        let n = 2usize << (packed & 7);
        if p + 3 * n > b.len() {
            return Err(Error::Truncated);
        }
        gct = b[p..p + 3 * n].chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
        p += 3 * n;
    }
    // A zero logical screen is legal-but-useless; browsers size it from the first frame.
    let (mut cw, mut ch) = (w, h);
    let len = crate::rgba_len(cw.max(1), ch.max(1))?;
    let mut canvas = crate::zeroed(len)?;
    let mut frames: Vec<Frame> = Vec::new();
    let mut loop_count: Option<u16> = None;
    // Pending Graphic Control Extension state.
    let mut disposal = 0u8;
    let mut transparent: Option<u8> = None;
    let mut delay = 0u32;
    let mut first = true;

    while p < b.len() {
        match b[p] {
            0x3B => break,
            0x21 => {
                let label = *b.get(p + 1).ok_or(Error::Truncated)?;
                p += 2;
                let mut blocks = Vec::new();
                p = sub_blocks(b, p, &mut blocks)?;
                match label {
                    0xF9 if blocks.len() >= 4 => {
                        disposal = (blocks[0] >> 2) & 7;
                        transparent = if blocks[0] & 1 != 0 { Some(blocks[3]) } else { None };
                        delay = le16(&blocks, 1) as u32 * 10;
                    }
                    0xFF => {
                        // Application identifier (8) + auth code (3), then the first data sub-block.
                        if blocks.len() >= 14
                            && (blocks.starts_with(b"NETSCAPE2.0") || blocks.starts_with(b"ANIMEXTS1.0"))
                            && blocks[11] == 1
                        {
                            loop_count = Some(le16(&blocks, 12));
                        }
                    }
                    _ => {}
                }
            }
            0x2C => {
                if p + 10 > b.len() {
                    return Err(Error::Truncated);
                }
                let (fx, fy) = (le16(b, p + 1) as usize, le16(b, p + 3) as usize);
                let (fw, fh) = (le16(b, p + 5) as usize, le16(b, p + 7) as usize);
                let ip = b[p + 9];
                p += 10;
                let lct: Vec<[u8; 3]>;
                let table: &[[u8; 3]] = if ip & 0x80 != 0 {
                    let n = 2usize << (ip & 7);
                    if p + 3 * n > b.len() {
                        return Err(Error::Truncated);
                    }
                    lct = b[p..p + 3 * n].chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
                    p += 3 * n;
                    &lct
                } else {
                    &gct
                };
                if first && (cw == 0 || ch == 0) {
                    cw = (fx + fw) as u32;
                    ch = (fy + fh) as u32;
                    canvas = crate::zeroed(crate::rgba_len(cw, ch)?)?;
                }
                let min_code = *b.get(p).ok_or(Error::Truncated)?;
                p += 1;
                let mut data = Vec::new();
                p = sub_blocks(b, p, &mut data)?;
                if !(1..=11).contains(&min_code) {
                    return Err(Error::Malformed("gif LZW minimum code size"));
                }
                if (fw as u64) * (fh as u64) > crate::MAX_PIXELS {
                    return Err(Error::TooLarge);
                }
                let mut idx = vec![0u8; fw * fh];
                let got = lzw(&data, min_code as u32, &mut idx);
                // Disposal 3 needs the canvas as it was before this frame.
                let saved = if disposal == 3 { Some(canvas.clone()) } else { None };
                let interlaced = ip & 0x40 != 0;
                let rows = row_order(fh, interlaced);
                for (src_row, &dy) in rows.iter().enumerate() {
                    let y = fy + dy;
                    if y >= ch as usize {
                        continue;
                    }
                    for dx in 0..fw {
                        let i = src_row * fw + dx;
                        if i >= got {
                            break; // short LZW data: the rest of the frame is not drawn
                        }
                        let x = fx + dx;
                        if x >= cw as usize {
                            continue;
                        }
                        let ci = idx[i];
                        if Some(ci) == transparent {
                            continue;
                        }
                        let c = table.get(ci as usize).copied().unwrap_or([0, 0, 0]);
                        let o = (y * cw as usize + x) * 4;
                        canvas[o..o + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
                    }
                }
                let mut snap = Vec::new();
                snap.try_reserve_exact(canvas.len()).map_err(|_| Error::OutOfMemory)?;
                snap.extend_from_slice(&canvas);
                frames.push(Frame { delay_ms: delay, rgba: snap });
                // Apply this frame's disposal before the next one is drawn (§23.c.iv).
                match disposal {
                    2 => {
                        for y in fy..(fy + fh).min(ch as usize) {
                            for x in fx..(fx + fw).min(cw as usize) {
                                let o = (y * cw as usize + x) * 4;
                                canvas[o..o + 4].fill(0);
                            }
                        }
                    }
                    3 => {
                        if let Some(s) = saved {
                            canvas = s;
                        }
                    }
                    _ => {}
                }
                disposal = 0;
                transparent = None;
                delay = 0;
                first = false;
            }
            0x00 => p += 1, // stray block terminator; tolerated
            _ => break,     // garbage after the last frame: stop as browsers do
        }
    }
    if frames.is_empty() {
        return Err(Error::Malformed("gif without an image"));
    }
    let rgba = frames[0].rgba.clone();
    let mut img = Image::still(cw, ch, rgba);
    if frames.len() > 1 {
        img.frames = Some(frames);
        img.loop_count = loop_count;
    }
    Ok(img)
}

/// Concatenate a data sub-block chain (§15) starting at `p`; returns the position after the
/// terminator. A chain cut off by the end of the file returns what it has.
fn sub_blocks(b: &[u8], mut p: usize, out: &mut Vec<u8>) -> Result<usize, Error> {
    loop {
        let Some(&n) = b.get(p) else { return Ok(p) };
        p += 1;
        if n == 0 {
            return Ok(p);
        }
        let end = (p + n as usize).min(b.len());
        out.extend_from_slice(&b[p..end]);
        p = end;
    }
}

/// Destination row for each stored row (Appendix E: passes 0/8, 4/8, 2/4, 1/2).
fn row_order(h: usize, interlaced: bool) -> Vec<usize> {
    if !interlaced {
        return (0..h).collect();
    }
    let mut v = Vec::with_capacity(h);
    for (start, step) in [(0, 8), (4, 8), (2, 4), (1, 2)] {
        let mut y = start;
        while y < h {
            v.push(y);
            y += step;
        }
    }
    v
}

/// Variable-length-code LZW (Appendix F). Writes indices into `out`; returns how many were written.
fn lzw(data: &[u8], min: u32, out: &mut [u8]) -> usize {
    let clear = 1u32 << min;
    let eoi = clear + 1;
    let mut prefix = [0u16; 4096];
    let mut suffix = [0u8; 4096];
    let mut first = [0u8; 4096];
    let mut length = [0u16; 4096];
    for i in 0..clear as usize {
        suffix[i] = i as u8;
        first[i] = i as u8;
        length[i] = 1;
    }
    let mut next = eoi + 1;
    let mut size = min + 1;
    let mut prev: Option<u32> = None;
    let (mut acc, mut nbits, mut pos, mut o) = (0u32, 0u32, 0usize, 0usize);
    while o < out.len() {
        while nbits < size {
            let Some(&byte) = data.get(pos) else { return o };
            acc |= (byte as u32) << nbits;
            nbits += 8;
            pos += 1;
        }
        let code = acc & ((1 << size) - 1);
        acc >>= size;
        nbits -= size;
        if code == clear {
            next = eoi + 1;
            size = min + 1;
            prev = None;
            continue;
        }
        if code == eoi {
            return o;
        }
        let Some(pc) = prev else {
            if code >= clear {
                return o; // the first code after a Clear must be a root
            }
            out[o] = code as u8;
            o += 1;
            prev = Some(code);
            continue;
        };
        // `code == next` is the KwKwK case: the string being defined right now.
        let (src, kwk) = if code < next {
            (code as usize, false)
        } else if code == next {
            (pc as usize, true)
        } else {
            return o; // a code not yet defined: corrupt
        };
        let fc = first[src];
        let slen = length[src] as usize;
        // Write the string back to front, clipped to the frame.
        let mut c = src;
        let mut k = slen;
        while k > 0 {
            k -= 1;
            if o + k < out.len() {
                out[o + k] = suffix[c];
            }
            c = prefix[c] as usize;
        }
        o = (o + slen).min(out.len());
        if kwk && o < out.len() {
            out[o] = fc;
            o += 1;
        }
        if next < 4096 {
            prefix[next as usize] = pc as u16;
            suffix[next as usize] = fc;
            first[next as usize] = first[pc as usize];
            length[next as usize] = length[pc as usize] + 1;
            next += 1;
            if next == (1 << size) && size < 12 {
                size += 1;
            }
        }
        prev = Some(code);
    }
    o
}
