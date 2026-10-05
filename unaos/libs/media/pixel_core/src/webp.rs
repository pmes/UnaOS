// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! WebP decoder — RFC 9649 ("WebP Image Format", 2024): the §2 RIFF container, LOSSY images (a VP8
//! key frame, §2.5 / RFC 6386, decoded by `vp8_core`) with or without an `ALPH` alpha chunk (§2.7:
//! raw or VP8L-compressed alpha, the four filtering methods), and LOSSLESS images (§3–§7, the VP8L
//! bitstream, decoded here).
//!
//! Lossless covers: the VP8L header (§3.2); all four transforms (§4) — predictor (all 14 modes, the
//! border rules, the rightmost-column TR rule), colour (the signed 3.5 fixed-point deltas),
//! subtract-green, colour indexing (palette delta coding and 1/2/4-bit pixel bundling); the colour
//! cache (§5.2.3, hash 0x1e35a7bd); meta prefix codes / the entropy image (§6.2.2); simple and normal
//! prefix codes with the code-length code and `max_symbol` (§6.2.1); LZ77 backward references with
//! the 120-entry distance map and the length/distance prefix coding (§5.2.2).
//!
//! Lossy: the VP8 frame decodes to I420, which converts to RGB the way libwebp presents it (BT.601
//! limited range, "fancy" 9-3-3-1 chroma upsampling; `vp8_core::yuv`). The ALPH chunk's
//! pre-processing (level reduction) bits are informational and need no decoder action.
//!
//! Animation (ANIMWEBP, §2.7.1.1 "Animation"): the `ANIM` chunk (background colour, loop count) and
//! every `ANMF` frame (offset, size, duration, blending method, disposal method; frame data = an
//! optional `ALPH` + `VP8 `, or a `VP8L`; unknown sub-chunks skipped) composited onto the canvas into
//! [`crate::Image::frames`]. Composition follows libwebp's `anim_decode.c` and Blink's
//! `WEBPImageDecoder`, which agree: the canvas starts TRANSPARENT and disposal clears to transparent
//! (the ANIM background colour is a hint the spec lets a decoder ignore, and both ignore it); a KEY
//! frame — the first, a full-canvas frame that is opaque or not blended, or one whose predecessor was
//! disposed and was itself full-canvas or a key frame — starts from a transparent canvas and is
//! copied, not blended; any other frame with the blend bit is alpha-blended over the canvas with the
//! integer non-premultiplied `src-over` both of them use ([`crate::blend_nonpremult`]) — EXCEPT inside
//! the rectangle the previous frame disposed, where both copy (their `FindBlendRangeAtRow`); a pixel
//! with alpha 255 is copied (the blend is skipped for it, which is not the same as blending it: the
//! integer formula would round 255-alpha colours down by one).
//!
//! NOT decoded: no ICC profile is applied.

use alloc::vec;
use alloc::vec::Vec;

use crate::{Error, Frame, Image, le32};

/// Decode a WebP file: lossless (VP8L) or lossy (VP8, with optional ALPH alpha).
pub fn decode(b: &[u8]) -> Result<Image, Error> {
    if b.len() < 20 || &b[0..4] != b"RIFF" || &b[8..12] != b"WEBP" {
        return Err(Error::Malformed("webp RIFF header"));
    }
    let end = (8 + le32(b, 4) as usize).min(b.len());
    let mut p = 12usize;
    let mut canvas: Option<(u32, u32)> = None;
    let mut alph: Option<&[u8]> = None;
    while p + 8 <= end {
        let fourcc = &b[p..p + 4];
        let size = le32(b, p + 4) as usize;
        let data = b.get(p + 8..p + 8 + size).ok_or(Error::Truncated)?;
        match fourcc {
            b"VP8L" => return decode_vp8l(data),
            b"VP8 " => return decode_lossy(data, alph, canvas),
            b"VP8X" => {
                if data.len() < 10 {
                    return Err(Error::Truncated);
                }
                let w = (data[4] as u32 | (data[5] as u32) << 8 | (data[6] as u32) << 16) + 1;
                let h = (data[7] as u32 | (data[8] as u32) << 8 | (data[9] as u32) << 16) + 1;
                canvas = Some((w, h));
                // §2.7: the Animation flag (bit 1 of the flags byte) — the rest of the file is ANIM + ANMF.
                if data[0] & 0x02 != 0 {
                    return decode_animated(b, p + 8 + size + (size & 1), end, w, h);
                }
            }
            b"ALPH" => alph = Some(data),
            b"ANIM" | b"ANMF" => return Err(Error::Malformed("webp ANIM/ANMF without the VP8X animation flag")),
            _ => {} // ICCP, EXIF, XMP: keep looking for the image chunk
        }
        p += 8 + size + (size & 1);
    }
    Err(Error::Malformed("webp without an image chunk"))
}

/// One `ANMF` frame's header (§2.7.1.1).
struct AnmfHeader {
    x: usize,
    y: usize,
    w: usize,
    h: usize,
    duration: u32,
    blend: bool,
    dispose_bg: bool,
}

/// Decode an animated WebP: `p` is just past the VP8X chunk, `end` the RIFF payload end, `(cw, ch)`
/// the canvas from VP8X.
fn decode_animated(b: &[u8], mut p: usize, end: usize, cw: u32, ch: u32) -> Result<Image, Error> {
    let len = crate::rgba_len(cw, ch)?;
    let (cwu, chu) = (cw as usize, ch as usize);
    // Pass 1, the demuxer (libwebp `demux.c`): the chunk list. A chunk cut off by the end of the
    // file ends the list there (the partial frame is dropped, as an incremental demuxer drops it);
    // a complete frame whose rectangle leaves the canvas refuses the whole file, as libwebp's
    // `IsValidExtendedFormat` does before any frame is decoded.
    let mut loop_raw: Option<u16> = None;
    let mut list: Vec<(AnmfHeader, &[u8])> = Vec::new();
    while p + 8 <= end {
        let fourcc = &b[p..p + 4];
        let size = le32(b, p + 4) as usize;
        let Some(data) = b.get(p + 8..p + 8 + size) else {
            break;
        };
        p += 8 + size + (size & 1);
        match fourcc {
            b"ANIM" => {
                if data.len() < 6 {
                    return Err(Error::Truncated);
                }
                // Bytes 0..4: background colour (B, G, R, A) — ignored, as libwebp and Blink do.
                loop_raw = Some(u16::from_le_bytes([data[4], data[5]]));
            }
            b"ANMF" => {
                if loop_raw.is_none() {
                    return Err(Error::Malformed("webp ANMF before ANIM"));
                }
                if data.len() < 16 {
                    break;
                }
                let u24 = |i: usize| data[i] as u32 | (data[i + 1] as u32) << 8 | (data[i + 2] as u32) << 16;
                let hd = AnmfHeader {
                    x: 2 * u24(0) as usize,
                    y: 2 * u24(3) as usize,
                    w: u24(6) as usize + 1,
                    h: u24(9) as usize + 1,
                    duration: u24(12),
                    blend: data[15] & 0x02 == 0,
                    dispose_bg: data[15] & 0x01 != 0,
                };
                if hd.x + hd.w > cwu || hd.y + hd.h > chu {
                    return Err(Error::Malformed("webp ANMF frame outside the canvas"));
                }
                list.push((hd, &data[16..]));
            }
            _ => {} // ICCP, EXIF, XMP and unknown chunks
        }
    }
    if list.is_empty() {
        return Err(Error::Malformed("webp animation without an ANMF frame"));
    }
    // Pass 2: decode and composite. A frame whose bitstream fails ends the animation at the frames
    // already composited (Blink fails that frame and keeps the earlier ones); a failing FIRST frame
    // refuses the file.
    let mut canvas = crate::zeroed(len)?;
    let mut frames: Vec<Frame> = Vec::new();
    // The previous frame's rectangle, disposal, and key-ness (libwebp `IsKeyFrame`).
    let mut prev: Option<(AnmfHeader, bool)> = None;
    for (hd, fd) in list {
        let decoded = decode_frame_data(fd).and_then(|(fw, fh, px, a)| {
            if (fw as usize, fh as usize) != (hd.w, hd.h) {
                Err(Error::Malformed("webp ANMF frame size differs from its bitstream"))
            } else {
                Ok((px, a))
            }
        });
        let (px, has_alpha) = match decoded {
            Ok(v) => v,
            Err(e) if frames.is_empty() => return Err(e),
            Err(_) => break,
        };
        let full = hd.w == cwu && hd.h == chu;
        // Inside the rectangle the previous frame disposed, the starting pixel is transparent
        // and libwebp/Blink do NOT blend there (`FindBlendRangeAtRow`): the frame is copied.
        let disposed = match &prev {
            Some((ph, _)) if ph.dispose_bg => Some((ph.x, ph.y, ph.x + ph.w, ph.y + ph.h)),
            _ => None,
        };
        let key = match &prev {
            None => true,
            Some((ph, pkey)) => {
                ((!has_alpha || !hd.blend) && full) || (ph.dispose_bg && ((ph.w == cwu && ph.h == chu) || *pkey))
            }
        };
        // Bring the canvas to this frame's starting state: the previous frame's disposal.
        if let Some((ph, _)) = &prev {
            if ph.dispose_bg {
                clear_rect(&mut canvas, cwu, ph.x, ph.y, ph.w, ph.h);
            }
        }
        if key {
            canvas.fill(0);
        }
        for row in 0..hd.h {
            for col in 0..hd.w {
                let s = (row * hd.w + col) * 4;
                let d = ((hd.y + row) * cwu + hd.x + col) * 4;
                let src = [px[s], px[s + 1], px[s + 2], px[s + 3]];
                let (cx, cy) = (hd.x + col, hd.y + row);
                let in_disposed = disposed.is_some_and(|(x0, y0, x1, y1)| cx >= x0 && cx < x1 && cy >= y0 && cy < y1);
                let out = if key || !hd.blend || src[3] == 255 || in_disposed {
                    src
                } else {
                    crate::blend_nonpremult(src, [canvas[d], canvas[d + 1], canvas[d + 2], canvas[d + 3]])
                };
                canvas[d..d + 4].copy_from_slice(&out);
            }
        }
        let mut snap = Vec::new();
        snap.try_reserve_exact(len).map_err(|_| Error::OutOfMemory)?;
        snap.extend_from_slice(&canvas);
        frames.push(Frame { delay_ms: hd.duration, rgba: snap });
        prev = Some((hd, key));
    }
    let mut img = Image::still(cw, ch, frames[0].rgba.clone());
    if frames.len() > 1 {
        img.frames = Some(frames);
        img.loop_count = crate::loop_from_plays(loop_raw.unwrap_or(0) as u32);
    }
    Ok(img)
}

/// Zero (transparent black) the `w x h` rectangle at `(x, y)` of a `cw`-wide RGBA canvas.
pub(crate) fn clear_rect(canvas: &mut [u8], cw: usize, x: usize, y: usize, w: usize, h: usize) {
    for row in y..y + h {
        canvas[(row * cw + x) * 4..(row * cw + x + w) * 4].fill(0);
    }
}

/// One ANMF frame's Frame Data: `ALPH`? + `VP8 `, or `VP8L`, unknown sub-chunks skipped. Returns the
/// frame's size, its straight RGBA, and whether it carries alpha (an ALPH chunk, or the VP8L header's
/// `alpha_is_used` bit — the `has_alpha` libwebp's demuxer reports and the key-frame rule reads).
fn decode_frame_data(d: &[u8]) -> Result<(u32, u32, Vec<u8>, bool), Error> {
    let mut p = 0usize;
    let mut alph: Option<&[u8]> = None;
    while p + 8 <= d.len() {
        let fourcc = &d[p..p + 4];
        let size = le32(d, p + 4) as usize;
        let data = d.get(p + 8..p + 8 + size).ok_or(Error::Truncated)?;
        match fourcc {
            b"VP8L" => {
                let has_alpha = data.len() >= 5 && (le32(data, 1) >> 28) & 1 == 1;
                let img = decode_vp8l(data)?;
                return Ok((img.width, img.height, img.rgba, has_alpha));
            }
            b"VP8 " => {
                let img = decode_lossy(data, alph, None)?;
                return Ok((img.width, img.height, img.rgba, alph.is_some()));
            }
            b"ALPH" => alph = Some(data),
            _ => {}
        }
        p += 8 + size + (size & 1);
    }
    Err(Error::Malformed("webp ANMF without an image bitstream"))
}

/// A lossy image: the VP8 key frame, plus the ALPH plane when the extended header carried one.
fn decode_lossy(vp8: &[u8], alph: Option<&[u8]>, canvas: Option<(u32, u32)>) -> Result<Image, Error> {
    let tag = vp8_core::parse_tag(vp8).map_err(vp8_err)?;
    if !tag.key_frame {
        return Err(Error::Malformed("webp VP8 chunk is not a key frame"));
    }
    let (w, h) = (tag.width as u32, tag.height as u32);
    if let Some((cw, ch)) = canvas {
        if (cw, ch) != (w, h) {
            return Err(Error::Malformed("webp VP8 frame size differs from the VP8X canvas"));
        }
    }
    crate::rgba_len(w, h)?;
    let yuv = vp8_core::decode_key_frame_strict(vp8).map_err(vp8_err)?;
    let mut rgba = vp8_core::yuv::to_rgba(&yuv);
    if let Some(a) = alph {
        let alpha = decode_alpha(a, w as usize, h as usize)?;
        for (px, &a) in rgba.chunks_exact_mut(4).zip(alpha.iter()) {
            px[3] = a;
        }
    }
    Ok(Image::still(w, h, rgba))
}

fn vp8_err(e: vp8_core::Error) -> Error {
    match e {
        vp8_core::Error::Truncated => Error::Truncated,
        vp8_core::Error::Malformed(m) => Error::Malformed(m),
    }
}

/// The ALPH chunk (RFC 9649 §2.7): a header byte (reserved:2, pre-processing:2, filtering:2,
/// compression:2), then the `w × h` alpha plane — raw, or a VP8L image-stream with implicit
/// dimensions whose GREEN channel carries alpha — then the inverse of the spatial filter.
fn decode_alpha(d: &[u8], w: usize, h: usize) -> Result<Vec<u8>, Error> {
    let hdr = *d.first().ok_or(Error::Truncated)?;
    let compression = hdr & 3;
    let filtering = (hdr >> 2) & 3;
    let mut a = match compression {
        0 => {
            let raw = d.get(1..1 + w * h).ok_or(Error::Truncated)?;
            raw.to_vec()
        }
        1 => {
            let mut br = Bits::new(&d[1..]);
            let px = decode_vp8l_stream(&mut br, w, h)?;
            px.iter().map(|p| (p >> 8) as u8).collect()
        }
        _ => return Err(Error::Malformed("webp ALPH compression method")),
    };
    // Unfiltering (§2.7, "Filtering method"): each value is a delta from its predictor, mod 256.
    // Row 0 predicts from the left (pixel (0,0) from 0); column 0 predicts from above.
    let pred_at = |a: &[u8], x: usize, y: usize| -> u8 {
        let l = |a: &[u8]| a[y * w + x - 1];
        let t = |a: &[u8]| a[(y - 1) * w + x];
        match (x, y) {
            (0, 0) => 0,
            (_, 0) => l(a),
            (0, _) => t(a),
            _ => match filtering {
                1 => l(a),
                2 => t(a),
                _ => {
                    let g = l(a) as i32 + t(a) as i32 - a[(y - 1) * w + x - 1] as i32;
                    g.clamp(0, 255) as u8
                }
            },
        }
    };
    if filtering != 0 {
        for y in 0..h {
            for x in 0..w {
                let p = pred_at(&a, x, y);
                a[y * w + x] = a[y * w + x].wrapping_add(p);
            }
        }
    }
    Ok(a)
}

/// LSB-first bit reader (§3.1).
struct Bits<'a> {
    d: &'a [u8],
    pos: usize,
    buf: u64,
    n: u32,
    overrun: bool,
}

impl<'a> Bits<'a> {
    fn new(d: &'a [u8]) -> Self {
        Bits { d, pos: 0, buf: 0, n: 0, overrun: false }
    }
    #[inline]
    fn fill(&mut self) {
        while self.n <= 56 {
            let b = match self.d.get(self.pos) {
                Some(&b) => b,
                None => {
                    if self.pos >= self.d.len() + 8 {
                        self.overrun = true;
                    }
                    0
                }
            };
            self.pos += 1;
            self.buf |= (b as u64) << self.n;
            self.n += 8;
        }
    }
    #[inline]
    fn read(&mut self, k: u32) -> u32 {
        if k == 0 {
            return 0;
        }
        if self.n < k {
            self.fill();
        }
        let v = (self.buf & ((1u64 << k) - 1)) as u32;
        self.buf >>= k;
        self.n -= k;
        v
    }
}

/// A canonical prefix code (§6.2.1): an 8-bit first-level table plus a count/symbol slow path.
struct Code {
    /// One-symbol code: reading consumes no bits.
    single: Option<u16>,
    fast: Vec<(u8, u16)>,
    counts: [u16; 16],
    symbols: Vec<u16>,
}

const FAST_BITS: u32 = 8;

impl Code {
    fn from_lengths(lengths: &[u8]) -> Result<Code, Error> {
        let used: Vec<usize> = (0..lengths.len()).filter(|&i| lengths[i] != 0).collect();
        if used.is_empty() {
            return Err(Error::Malformed("webp empty prefix code"));
        }
        if used.len() == 1 {
            return Ok(Code { single: Some(used[0] as u16), fast: Vec::new(), counts: [0; 16], symbols: Vec::new() });
        }
        let mut counts = [0u16; 16];
        for &l in lengths {
            if l > 15 {
                return Err(Error::Malformed("webp code length"));
            }
            counts[l as usize] += 1;
        }
        counts[0] = 0;
        let mut left = 1i32;
        for c in counts.iter().skip(1) {
            left = (left << 1) - *c as i32;
            if left < 0 {
                return Err(Error::Malformed("webp prefix code over-subscribed"));
            }
        }
        if left != 0 {
            return Err(Error::Malformed("webp prefix code incomplete"));
        }
        let mut offs = [0u16; 16];
        for l in 1..15 {
            offs[l + 1] = offs[l] + counts[l];
        }
        let mut symbols = vec![0u16; used.len()];
        let mut next_code = [0u32; 16];
        {
            let mut code = 0u32;
            for l in 1..16 {
                code = (code + counts[l - 1] as u32) << 1;
                next_code[l] = code;
            }
        }
        let mut fast = vec![(0u8, 0u16); 1 << FAST_BITS];
        for (sym, &l) in lengths.iter().enumerate() {
            if l == 0 {
                continue;
            }
            symbols[offs[l as usize] as usize] = sym as u16;
            offs[l as usize] += 1;
            let code = next_code[l as usize];
            next_code[l as usize] += 1;
            if l as u32 <= FAST_BITS {
                // Bits arrive code-MSB first in an LSB-first stream: index by the reversed code.
                let rev = (code.reverse_bits() >> (32 - l as u32)) as usize;
                let step = 1usize << l;
                let mut i = rev;
                while i < fast.len() {
                    fast[i] = (l, sym as u16);
                    i += step;
                }
            }
        }
        Ok(Code { single: None, fast, counts, symbols })
    }

    #[inline]
    fn read(&self, br: &mut Bits) -> Result<u16, Error> {
        if let Some(s) = self.single {
            return Ok(s);
        }
        if br.n < 16 {
            br.fill();
        }
        let (l, s) = self.fast[(br.buf & ((1 << FAST_BITS) - 1)) as usize];
        if l != 0 {
            br.buf >>= l;
            br.n -= l as u32;
            return Ok(s);
        }
        // Slow path: canonical decode bit by bit (the "puff" shape).
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        for len in 1..16 {
            code |= br.read(1) as i32;
            let count = self.counts[len] as i32;
            if code - count < first {
                return self.symbols.get((index + code - first) as usize).copied().ok_or(Error::Malformed("webp symbol"));
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(Error::Malformed("webp symbol"))
    }
}

const CODE_LENGTH_ORDER: [usize; 19] = [17, 18, 0, 1, 2, 3, 4, 5, 16, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15];

/// Read one prefix code over an alphabet of `size` symbols (§6.2.1).
fn read_code(br: &mut Bits, size: usize) -> Result<Code, Error> {
    let mut lengths = vec![0u8; size];
    if br.read(1) == 1 {
        // Simple code: one or two symbols.
        let n = br.read(1) + 1;
        let first8 = br.read(1);
        let s0 = br.read(1 + 7 * first8) as usize;
        if s0 >= size {
            return Err(Error::Malformed("webp simple code symbol"));
        }
        lengths[s0] = 1;
        if n == 2 {
            let s1 = br.read(8) as usize;
            if s1 >= size {
                return Err(Error::Malformed("webp simple code symbol"));
            }
            lengths[s1] = 1;
        }
        return Code::from_lengths(&lengths);
    }
    let ncl = 4 + br.read(4) as usize;
    if ncl > 19 {
        return Err(Error::Malformed("webp code length count"));
    }
    let mut cl = [0u8; 19];
    for &o in CODE_LENGTH_ORDER.iter().take(ncl) {
        cl[o] = br.read(3) as u8;
    }
    let clc = Code::from_lengths(&cl)?;
    let mut max_symbol = if br.read(1) == 1 {
        let nbits = 2 + 2 * br.read(3);
        let m = 2 + br.read(nbits) as usize;
        if m > size {
            return Err(Error::Malformed("webp max_symbol"));
        }
        m
    } else {
        size
    };
    let mut prev = 8u8;
    let mut i = 0usize;
    while i < size {
        if max_symbol == 0 {
            break;
        }
        max_symbol -= 1;
        let c = clc.read(br)?;
        match c {
            0..=15 => {
                lengths[i] = c as u8;
                if c != 0 {
                    prev = c as u8;
                }
                i += 1;
            }
            16 | 17 | 18 => {
                let (extra, base, val) = match c {
                    16 => (2, 3, prev),
                    17 => (3, 3, 0),
                    _ => (7, 11, 0),
                };
                let n = base + br.read(extra) as usize;
                if i + n > size {
                    return Err(Error::Malformed("webp code length repeat"));
                }
                lengths[i..i + n].fill(val);
                i += n;
            }
            _ => return Err(Error::Malformed("webp code length symbol")),
        }
    }
    Code::from_lengths(&lengths)
}

/// The five codes of one prefix-code group (§6.2.2).
struct Group {
    green: Code,
    red: Code,
    blue: Code,
    alpha: Code,
    dist: Code,
}

/// §5.2.2 length / distance prefix coding.
fn prefix_value(br: &mut Bits, sym: u32) -> u32 {
    if sym < 4 {
        return sym + 1;
    }
    let extra = (sym - 2) >> 1;
    let offset = (2 + (sym & 1)) << extra;
    offset + br.read(extra) + 1
}

/// The 120-entry distance map (§5.2.2), as (xi, yi).
const DIST_MAP: [(i8, i8); 120] = [
    (0, 1), (1, 0), (1, 1), (-1, 1), (0, 2), (2, 0), (1, 2), (-1, 2), (2, 1), (-2, 1), (2, 2), (-2, 2), (0, 3),
    (3, 0), (1, 3), (-1, 3), (3, 1), (-3, 1), (2, 3), (-2, 3), (3, 2), (-3, 2), (0, 4), (4, 0), (1, 4), (-1, 4),
    (4, 1), (-4, 1), (3, 3), (-3, 3), (2, 4), (-2, 4), (4, 2), (-4, 2), (0, 5), (3, 4), (-3, 4), (4, 3), (-4, 3),
    (5, 0), (1, 5), (-1, 5), (5, 1), (-5, 1), (2, 5), (-2, 5), (5, 2), (-5, 2), (4, 4), (-4, 4), (3, 5), (-3, 5),
    (5, 3), (-5, 3), (0, 6), (6, 0), (1, 6), (-1, 6), (6, 1), (-6, 1), (2, 6), (-2, 6), (6, 2), (-6, 2), (4, 5),
    (-4, 5), (5, 4), (-5, 4), (3, 6), (-3, 6), (6, 3), (-6, 3), (0, 7), (7, 0), (1, 7), (-1, 7), (5, 5), (-5, 5),
    (7, 1), (-7, 1), (4, 6), (-4, 6), (6, 4), (-6, 4), (2, 7), (-2, 7), (7, 2), (-7, 2), (3, 7), (-3, 7), (7, 3),
    (-7, 3), (5, 6), (-5, 6), (6, 5), (-6, 5), (8, 0), (4, 7), (-4, 7), (7, 4), (-7, 4), (8, 1), (8, 2), (6, 6),
    (-6, 6), (8, 3), (5, 7), (-5, 7), (7, 5), (-7, 5), (8, 4), (6, 7), (-6, 7), (7, 6), (-7, 6), (8, 5), (7, 7),
    (-7, 7), (8, 6), (8, 7),
];

/// Decode an entropy-coded image of `w x h` ARGB pixels (§5). `meta` = whether this is the main
/// (spatially coded) image, which may carry meta prefix codes.
fn decode_image(br: &mut Bits, w: usize, h: usize, meta: bool) -> Result<Vec<u32>, Error> {
    let cache_bits = if br.read(1) == 1 {
        let b = br.read(4);
        if !(1..=11).contains(&b) {
            return Err(Error::Malformed("webp colour cache bits"));
        }
        b
    } else {
        0
    };
    let cache_size = if cache_bits > 0 { 1usize << cache_bits } else { 0 };
    // Meta prefix codes: the entropy image maps each block to a group.
    let (mut prefix_bits, mut ent, mut ent_w) = (0u32, Vec::new(), 1usize);
    let mut ngroups = 1usize;
    if meta && br.read(1) == 1 {
        prefix_bits = br.read(3) + 2;
        ent_w = w.div_ceil(1 << prefix_bits);
        let ent_h = h.div_ceil(1 << prefix_bits);
        ent = decode_image(br, ent_w, ent_h, false)?;
        for px in ent.iter_mut() {
            *px = (*px >> 8) & 0xFFFF;
            ngroups = ngroups.max(*px as usize + 1);
        }
    }
    if ngroups > 4096 {
        return Err(Error::Malformed("webp too many prefix groups"));
    }
    let mut groups = Vec::with_capacity(ngroups);
    for _ in 0..ngroups {
        groups.push(Group {
            green: read_code(br, 256 + 24 + cache_size)?,
            red: read_code(br, 256)?,
            blue: read_code(br, 256)?,
            alpha: read_code(br, 256)?,
            dist: read_code(br, 40)?,
        });
        if br.overrun {
            return Err(Error::Truncated);
        }
    }
    let n = w * h;
    let mut out: Vec<u32> = Vec::new();
    out.try_reserve_exact(n).map_err(|_| Error::OutOfMemory)?;
    out.resize(n, 0);
    let mut cache = vec![0u32; cache_size];
    let mut cached = 0usize; // pixels already inserted into the cache
    let hash_shift = 32 - cache_bits;
    let mut pos = 0usize;
    while pos < n {
        let g = if ent.is_empty() {
            &groups[0]
        } else {
            let (x, y) = (pos % w, pos / w);
            &groups[ent[(y >> prefix_bits) * ent_w + (x >> prefix_bits)] as usize]
        };
        let s = g.green.read(br)? as usize;
        if s < 256 {
            let r = g.red.read(br)? as u32;
            let b = g.blue.read(br)? as u32;
            let a = g.alpha.read(br)? as u32;
            out[pos] = (a << 24) | (r << 16) | ((s as u32) << 8) | b;
            pos += 1;
        } else if s < 256 + 24 {
            let len = prefix_value(br, (s - 256) as u32) as usize;
            let ds = g.dist.read(br)? as u32;
            let dcode = prefix_value(br, ds) as usize;
            let dist = if dcode > 120 {
                dcode - 120
            } else {
                let (xi, yi) = DIST_MAP[dcode - 1];
                let d = xi as isize + yi as isize * w as isize;
                if d < 1 { 1 } else { d as usize }
            };
            if dist > pos || pos + len > n {
                return Err(Error::Malformed("webp backward reference"));
            }
            for k in 0..len {
                out[pos + k] = out[pos + k - dist];
            }
            pos += len;
        } else {
            let i = s - 280;
            if i >= cache_size {
                return Err(Error::Malformed("webp colour cache index"));
            }
            // The cache must hold every pixel before this one.
            while cached < pos {
                let px = out[cached];
                cache[(0x1e35_a7bd_u32.wrapping_mul(px) >> hash_shift) as usize] = px;
                cached += 1;
            }
            out[pos] = cache[i];
            pos += 1;
        }
        if cache_size > 0 {
            while cached < pos {
                let px = out[cached];
                cache[(0x1e35_a7bd_u32.wrapping_mul(px) >> hash_shift) as usize] = px;
                cached += 1;
            }
        }
        if br.overrun {
            return Err(Error::Truncated);
        }
    }
    Ok(out)
}

enum Transform {
    Predictor { bits: u32, img: Vec<u32> },
    Color { bits: u32, img: Vec<u32> },
    SubtractGreen,
    Index { table: Vec<u32>, bits: u32 },
}

fn decode_vp8l(d: &[u8]) -> Result<Image, Error> {
    if d.len() < 5 || d[0] != 0x2F {
        return Err(Error::Malformed("vp8l signature"));
    }
    let mut br = Bits::new(&d[1..]);
    let w = br.read(14) as usize + 1;
    let h = br.read(14) as usize + 1;
    let _alpha_hint = br.read(1);
    if br.read(3) != 0 {
        return Err(Error::Malformed("vp8l version"));
    }
    let len = crate::rgba_len(w as u32, h as u32)?;
    let px = decode_vp8l_stream(&mut br, w, h)?;
    let mut rgba = crate::zeroed(len)?;
    for (o, p) in rgba.chunks_exact_mut(4).zip(px.iter()) {
        o.copy_from_slice(&[(p >> 16) as u8, (p >> 8) as u8, *p as u8, (p >> 24) as u8]);
    }
    Ok(Image::still(w as u32, h as u32, rgba))
}

/// A VP8L image-stream of known dimensions (§3.2 onward, after the header): transforms, then the
/// entropy-coded image, then the inverse transforms. ARGB words. The ALPH chunk's lossless alpha
/// is exactly this, with implicit dimensions (RFC 9649 §2.7.1.? "ALPH").
fn decode_vp8l_stream(br: &mut Bits, w: usize, h: usize) -> Result<Vec<u32>, Error> {
    crate::rgba_len(w as u32, h as u32)?;
    let mut xsize = w;
    let mut transforms: Vec<Transform> = Vec::new();
    let mut seen = [false; 4];
    while br.read(1) == 1 {
        let t = br.read(2) as usize;
        if seen[t] {
            return Err(Error::Malformed("vp8l transform repeated"));
        }
        seen[t] = true;
        match t {
            0 | 1 => {
                let bits = br.read(3) + 2;
                let img = decode_image(br, xsize.div_ceil(1 << bits), h.div_ceil(1 << bits), false)?;
                transforms.push(if t == 0 { Transform::Predictor { bits, img } } else { Transform::Color { bits, img } });
            }
            2 => transforms.push(Transform::SubtractGreen),
            _ => {
                let n = br.read(8) as usize + 1;
                let mut table = decode_image(br, n, 1, false)?;
                for i in 1..n {
                    table[i] = add_pixels(table[i], table[i - 1]);
                }
                let bits = match n {
                    0..=2 => 3,
                    3..=4 => 2,
                    5..=16 => 1,
                    _ => 0,
                };
                xsize = xsize.div_ceil(1 << bits);
                transforms.push(Transform::Index { table, bits });
            }
        }
    }
    let mut px = decode_image(br, xsize, h, true)?;
    // Inverse transforms in reverse order (§4).
    for t in transforms.iter().rev() {
        match t {
            Transform::Predictor { bits, img } => predict(&mut px, xsize, h, *bits, img),
            Transform::Color { bits, img } => {
                let bw = xsize.div_ceil(1 << bits);
                for y in 0..h {
                    for x in 0..xsize {
                        let e = img[(y >> bits) * bw + (x >> bits)];
                        let (g2r, g2b, r2b) = (e as u8 as i8, (e >> 8) as u8 as i8, (e >> 16) as u8 as i8);
                        let p = px[y * xsize + x];
                        let green = (p >> 8) as u8 as i8;
                        let mut red = (p >> 16) as u8 as i32;
                        let mut blue = p as u8 as i32;
                        red += delta(g2r, green);
                        blue += delta(g2b, green);
                        blue += delta(r2b, red as u8 as i8);
                        px[y * xsize + x] = (p & 0xFF00_FF00) | (((red as u32) & 0xFF) << 16) | ((blue as u32) & 0xFF);
                    }
                }
            }
            Transform::SubtractGreen => {
                for p in px.iter_mut() {
                    let g = (*p >> 8) & 0xFF;
                    let r = (((*p >> 16) & 0xFF) + g) & 0xFF;
                    let b = ((*p & 0xFF) + g) & 0xFF;
                    *p = (*p & 0xFF00_FF00) | (r << 16) | b;
                }
            }
            Transform::Index { table, bits } => {
                let mut out = vec![0u32; w * h];
                let per = 1usize << bits;
                let bpp = 8 >> bits;
                let mask = (1u32 << bpp) - 1;
                for y in 0..h {
                    for x in 0..w {
                        let packed = (px[y * xsize + x / per] >> 8) & 0xFF;
                        let i = if *bits == 0 { packed } else { (packed >> (bpp * (x % per) as u32)) & mask };
                        out[y * w + x] = table.get(i as usize).copied().unwrap_or(0);
                    }
                }
                px = out;
                xsize = w;
            }
        }
    }
    let _ = xsize;
    Ok(px)
}

#[inline]
fn delta(t: i8, c: i8) -> i32 {
    (t as i32 * c as i32) >> 5
}

#[inline]
fn add_pixels(a: u32, b: u32) -> u32 {
    let ag = (a & 0xFF00_FF00).wrapping_add(b & 0xFF00_FF00) & 0xFF00_FF00;
    let rb = (a & 0x00FF_00FF).wrapping_add(b & 0x00FF_00FF) & 0x00FF_00FF;
    ag | rb
}

#[inline]
fn ch(p: u32, s: u32) -> i32 {
    ((p >> s) & 0xFF) as i32
}

#[inline]
fn avg2(a: u32, b: u32) -> u32 {
    (((a ^ b) & 0xFEFE_FEFE) >> 1) + (a & b)
}

fn map4(f: impl Fn(u32) -> i32) -> u32 {
    let mut out = 0u32;
    for s in [0u32, 8, 16, 24] {
        out |= (f(s).clamp(0, 255) as u32) << s;
    }
    out
}

fn select(l: u32, t: u32, tl: u32) -> u32 {
    let mut pl = 0;
    let mut pt = 0;
    for s in [0u32, 8, 16, 24] {
        let p = ch(l, s) + ch(t, s) - ch(tl, s);
        pl += (p - ch(l, s)).abs();
        pt += (p - ch(t, s)).abs();
    }
    if pl < pt { l } else { t }
}

/// The inverse predictor transform (§4.1), in place, in raster order.
fn predict(px: &mut [u32], w: usize, h: usize, bits: u32, img: &[u32]) {
    let bw = w.div_ceil(1 << bits);
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let pred = if y == 0 {
                if x == 0 { 0xFF00_0000 } else { px[i - 1] }
            } else if x == 0 {
                px[i - w]
            } else {
                let mode = (img[(y >> bits) * bw + (x >> bits)] >> 8) & 0xF;
                let (l, t, tl) = (px[i - 1], px[i - w], px[i - w - 1]);
                let tr = px[i - w + 1]; // rightmost column: the leftmost pixel of this row (§4.1)
                match mode {
                    0 => 0xFF00_0000,
                    1 => l,
                    2 => t,
                    3 => tr,
                    4 => tl,
                    5 => avg2(avg2(l, tr), t),
                    6 => avg2(l, tl),
                    7 => avg2(l, t),
                    8 => avg2(tl, t),
                    9 => avg2(t, tr),
                    10 => avg2(avg2(l, tl), avg2(t, tr)),
                    11 => select(l, t, tl),
                    12 => map4(|s| ch(l, s) + ch(t, s) - ch(tl, s)),
                    13 => {
                        let a = avg2(l, t);
                        map4(|s| ch(a, s) + (ch(a, s) - ch(tl, s)) / 2)
                    }
                    _ => 0xFF00_0000, // 14, 15: undefined; libwebp treats them as mode 0
                }
            };
            px[i] = add_pixels(px[i], pred);
        }
    }
}
