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

/// Decode a GIF to RGBA8 (first frame) plus every composited frame when animated. Built on
/// [`Stepper`]: the very compositor [`crate::Animation`] streams one frame at a time.
pub fn decode(b: &[u8]) -> Result<Image, Error> {
    let mut s = Stepper::new(b)?;
    let mut frames: Vec<Frame> = Vec::new();
    while let Some(r) = s.step(b) {
        let delay_ms = r?;
        let mut snap = Vec::new();
        snap.try_reserve_exact(s.canvas.len()).map_err(|_| Error::OutOfMemory)?;
        snap.extend_from_slice(&s.canvas);
        frames.push(Frame { delay_ms, rgba: snap });
    }
    if frames.is_empty() {
        return Err(Error::Malformed("gif without an image"));
    }
    let rgba = frames[0].rgba.clone();
    let mut img = Image::still(s.cw, s.ch, rgba);
    if frames.len() > 1 {
        img.frames = Some(frames);
        img.loop_count = s.loop_count;
    }
    Ok(img)
}

/// The block walk both the compositor and [`scan`] use: one event per Extension or Image Descriptor.
enum Event {
    /// Graphic Control Extension: (disposal, transparent index, delay in ms).
    Gce(u8, Option<u8>, u32),
    /// NETSCAPE2.0 / ANIMEXTS1.0 loop count.
    Loop(u16),
    /// An Image Descriptor: rectangle, packed byte, local colour table position (start, entries),
    /// LZW minimum code size, and the position of its first data sub-block.
    Image { fx: usize, fy: usize, fw: usize, fh: usize, ip: u8, lct: Option<(usize, usize)>, min_code: u8, data: usize },
    /// Trailer, garbage after the last frame, or the end of the bytes.
    End,
}

/// Header + Logical Screen Descriptor + Global Color Table: (width, height, gct, first block position).
fn header(b: &[u8]) -> Result<(u32, u32, Vec<[u8; 3]>, usize), Error> {
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
    Ok((w, h, gct, p))
}

/// The next event at `*p`, advancing `*p` past it (an image's data sub-blocks included).
fn next_event(b: &[u8], p: &mut usize) -> Result<Event, Error> {
    while *p < b.len() {
        match b[*p] {
            0x3B => return Ok(Event::End),
            0x21 => {
                let label = *b.get(*p + 1).ok_or(Error::Truncated)?;
                *p += 2;
                let mut blocks = Vec::new();
                *p = sub_blocks(b, *p, &mut blocks)?;
                match label {
                    0xF9 if blocks.len() >= 4 => {
                        let transparent = if blocks[0] & 1 != 0 { Some(blocks[3]) } else { None };
                        return Ok(Event::Gce((blocks[0] >> 2) & 7, transparent, le16(&blocks, 1) as u32 * 10));
                    }
                    0xFF => {
                        // Application identifier (8) + auth code (3), then the first data sub-block.
                        if blocks.len() >= 14
                            && (blocks.starts_with(b"NETSCAPE2.0") || blocks.starts_with(b"ANIMEXTS1.0"))
                            && blocks[11] == 1
                        {
                            return Ok(Event::Loop(le16(&blocks, 12)));
                        }
                    }
                    _ => {}
                }
            }
            0x2C => {
                if *p + 10 > b.len() {
                    return Err(Error::Truncated);
                }
                let (fx, fy) = (le16(b, *p + 1) as usize, le16(b, *p + 3) as usize);
                let (fw, fh) = (le16(b, *p + 5) as usize, le16(b, *p + 7) as usize);
                let ip = b[*p + 9];
                *p += 10;
                let mut lct = None;
                if ip & 0x80 != 0 {
                    let n = 2usize << (ip & 7);
                    if *p + 3 * n > b.len() {
                        return Err(Error::Truncated);
                    }
                    lct = Some((*p, n));
                    *p += 3 * n;
                }
                let min_code = *b.get(*p).ok_or(Error::Truncated)?;
                *p += 1;
                let data = *p;
                *p = skip_sub_blocks(b, *p);
                return Ok(Event::Image { fx, fy, fw, fh, ip, lct, min_code, data });
            }
            0x00 => *p += 1, // stray block terminator; tolerated
            _ => return Ok(Event::End), // garbage after the last frame: stop as browsers do
        }
    }
    Ok(Event::End)
}

/// Count the frames and read the loop count without decoding a pixel (FACETANIM: the viewer's
/// `frame i/n` and its loop policy before the first frame is shown). A structural error ends the
/// count at the frames before it (the compositor then reports the error at that frame).
pub fn scan(b: &[u8]) -> (usize, Option<u16>) {
    let Ok((_, _, _, mut p)) = header(b) else { return (0, None) };
    let (mut n, mut lp) = (0usize, None);
    loop {
        match next_event(b, &mut p) {
            Ok(Event::Image { .. }) => n += 1,
            Ok(Event::Loop(c)) => lp = Some(c),
            Ok(Event::Gce(..)) => {}
            Ok(Event::End) | Err(_) => return (n, lp),
        }
    }
}

/// The GIF compositor, one frame per [`Stepper::step`]: the canvas plus, only while a frame with
/// disposal 3 is up, the canvas from before it (FACETANIM: at most one extra canvas-sized buffer).
/// The file's bytes are passed to every call (the stepper borrows nothing, so an owner can hold both).
pub struct Stepper {
    gct: Vec<[u8; 3]>,
    start: usize,
    p: usize,
    pub(crate) cw: u32,
    pub(crate) ch: u32,
    pub(crate) canvas: Vec<u8>,
    /// The disposal the frame on the canvas asks for: (method, x, y, w, h, the canvas before it).
    pending: Option<(u8, usize, usize, usize, usize, Option<Vec<u8>>)>,
    pub(crate) loop_count: Option<u16>,
    first: bool,
    done: bool,
}

impl Stepper {
    pub fn new(b: &[u8]) -> Result<Self, Error> {
        let (w, h, gct, p) = header(b)?;
        // A zero logical screen is legal-but-useless; browsers size it from the first frame.
        let canvas = crate::zeroed(crate::rgba_len(w.max(1), h.max(1))?)?;
        Ok(Stepper { gct, start: p, p, cw: w, ch: h, canvas, pending: None, loop_count: None, first: true, done: false })
    }

    /// A disposal-3 frame is up: the canvas from before it is held.
    pub fn holds_saved(&self) -> bool {
        matches!(self.pending, Some((_, _, _, _, _, Some(_))))
    }

    /// Back to before frame 0 (the canvas is cleared, nothing is re-read but the bytes).
    pub fn reset(&mut self) -> Result<(), Error> {
        // A canvas sized from the first frame (zero logical screen) keeps that size.
        self.canvas.fill(0);
        self.p = self.start;
        self.pending = None;
        self.first = true;
        self.done = false;
        Ok(())
    }

    /// Composite the next frame onto the canvas and return its delay; `None` at the end.
    pub fn step(&mut self, b: &[u8]) -> Option<Result<u32, Error>> {
        if self.done {
            return None;
        }
        let r = self.step_inner(b);
        if !matches!(r, Some(Ok(_))) {
            self.done = true;
        }
        r
    }

    fn step_inner(&mut self, b: &[u8]) -> Option<Result<u32, Error>> {
        // Apply the previous frame's disposal before this one is drawn (§23.c.iv).
        if let Some((disposal, fx, fy, fw, fh, saved)) = self.pending.take() {
            let (cw, ch) = (self.cw as usize, self.ch as usize);
            match disposal {
                2 => {
                    for y in fy..(fy + fh).min(ch) {
                        for x in fx..(fx + fw).min(cw) {
                            let o = (y * cw + x) * 4;
                            self.canvas[o..o + 4].fill(0);
                        }
                    }
                }
                3 => {
                    if let Some(s) = saved {
                        self.canvas = s;
                    }
                }
                _ => {}
            }
        }
        // Pending Graphic Control Extension state.
        let (mut disposal, mut transparent, mut delay) = (0u8, None, 0u32);
        loop {
            let ev = match next_event(b, &mut self.p) {
                Ok(ev) => ev,
                Err(e) => return Some(Err(e)),
            };
            match ev {
                Event::End => return None,
                Event::Loop(c) => self.loop_count = Some(c),
                Event::Gce(d, t, ms) => (disposal, transparent, delay) = (d, t, ms),
                Event::Image { fx, fy, fw, fh, ip, lct, min_code, data } => {
                    return Some(self.draw(b, fx, fy, fw, fh, ip, lct, min_code, data, disposal, transparent).map(|_| delay));
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw(
        &mut self,
        b: &[u8],
        fx: usize,
        fy: usize,
        fw: usize,
        fh: usize,
        ip: u8,
        lct: Option<(usize, usize)>,
        min_code: u8,
        data_at: usize,
        disposal: u8,
        transparent: Option<u8>,
    ) -> Result<(), Error> {
        let lct_v: Vec<[u8; 3]>;
        let table: &[[u8; 3]] = match lct {
            Some((at, n)) => {
                lct_v = b[at..at + 3 * n].chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect();
                &lct_v
            }
            None => &self.gct,
        };
        if self.first && (self.cw == 0 || self.ch == 0) {
            self.cw = (fx + fw) as u32;
            self.ch = (fy + fh) as u32;
            self.canvas = crate::zeroed(crate::rgba_len(self.cw, self.ch)?)?;
        }
        let mut data = Vec::new();
        sub_blocks(b, data_at, &mut data)?;
        if !(1..=11).contains(&min_code) {
            return Err(Error::Malformed("gif LZW minimum code size"));
        }
        if (fw as u64) * (fh as u64) > crate::MAX_PIXELS {
            return Err(Error::TooLarge);
        }
        let mut idx = vec![0u8; fw * fh];
        let got = lzw(&data, min_code as u32, &mut idx);
        // Disposal 3 needs the canvas as it was before this frame.
        let saved = if disposal == 3 {
            let mut s = Vec::new();
            s.try_reserve_exact(self.canvas.len()).map_err(|_| Error::OutOfMemory)?;
            s.extend_from_slice(&self.canvas);
            Some(s)
        } else {
            None
        };
        let (cw, ch) = (self.cw as usize, self.ch as usize);
        let interlaced = ip & 0x40 != 0;
        let rows = row_order(fh, interlaced);
        for (src_row, &dy) in rows.iter().enumerate() {
            let y = fy + dy;
            if y >= ch {
                continue;
            }
            for dx in 0..fw {
                let i = src_row * fw + dx;
                if i >= got {
                    break; // short LZW data: the rest of the frame is not drawn
                }
                let x = fx + dx;
                if x >= cw {
                    continue;
                }
                let ci = idx[i];
                if Some(ci) == transparent {
                    continue;
                }
                let c = table.get(ci as usize).copied().unwrap_or([0, 0, 0]);
                let o = (y * cw + x) * 4;
                self.canvas[o..o + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
            }
        }
        self.pending = Some((disposal, fx, fy, fw, fh, saved));
        self.first = false;
        Ok(())
    }
}

/// Past a data sub-block chain without copying it (the end of the bytes ends it).
fn skip_sub_blocks(b: &[u8], mut p: usize) -> usize {
    loop {
        let Some(&n) = b.get(p) else { return p };
        p += 1;
        if n == 0 {
            return p;
        }
        p = (p + n as usize).min(b.len());
    }
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
