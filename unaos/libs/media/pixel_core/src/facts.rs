// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! ATTRCOLUMNS (rmbp-ledger B402) — an image's FACTS from its header, without decoding: the pixel size and whether
//! it animates. The kernel writes them as typed attributes (`media:width`, `media:height`, `image:animated`) beside
//! `una:type`, the way BeOS's sniffers wrote `Media:Width`; any ring-3 caller reads the same function. Every walk is
//! bounded by the slice it is given and checked before each read (the bytes are attacker-shaped). Pure.

/// What [`facts_of`] learned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageFacts {
    pub width: u32,
    pub height: u32,
    /// More than one frame: an APNG with `acTL` frames > 1, a GIF with more than one image, a WebP with `ANIM`.
    pub animated: bool,
}

fn be16(b: &[u8], i: usize) -> Option<u32> {
    Some(u16::from_be_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32)
}
fn be32(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_be_bytes([*b.get(i)?, *b.get(i + 1)?, *b.get(i + 2)?, *b.get(i + 3)?]))
}
fn le16(b: &[u8], i: usize) -> Option<u32> {
    Some(u16::from_le_bytes([*b.get(i)?, *b.get(i + 1)?]) as u32)
}
fn le24(b: &[u8], i: usize) -> Option<u32> {
    Some(*b.get(i)? as u32 | (*b.get(i + 1)? as u32) << 8 | (*b.get(i + 2)? as u32) << 16)
}
fn le32(b: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes([*b.get(i)?, *b.get(i + 1)?, *b.get(i + 2)?, *b.get(i + 3)?]))
}

/// The facts of an image file (`None` when the bytes are no image this core knows, or the header is cut short).
/// SVG is read from the root element's `width`/`height` (else its `viewBox`) whether or not the `svg` renderer is
/// built: the size is a fact of the markup.
pub fn facts_of(b: &[u8]) -> Option<ImageFacts> {
    if raw_core::is_tiff(b) {
        // RAWCORE (B444): the sensor size of a camera raw (or a plain TIFF's IFD0 size), from the head's IFDs.
        let x = crate::raw::facts(b)?;
        return Some(ImageFacts { width: x.width?, height: x.height?, animated: false }).filter(|f| f.width <= crate::MAX_DIM && f.height <= crate::MAX_DIM);
    }
    let f = |w: u32, h: u32, animated: bool| if w > 0 && h > 0 && w <= crate::MAX_DIM && h <= crate::MAX_DIM { Some(ImageFacts { width: w, height: h, animated }) } else { None };
    if b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        // IHDR is the first chunk (PNG §11.2.2); an `acTL` before the first IDAT makes it an APNG.
        let (w, h) = (be32(b, 16)?, be32(b, 20)?);
        let mut at = 8usize;
        let mut frames = 0u32;
        for _ in 0..64 {
            let (Some(len), Some(ty)) = (be32(b, at), b.get(at + 4..at + 8)) else { break };
            if ty == b"IDAT" {
                break;
            }
            if ty == b"acTL" {
                frames = be32(b, at + 8).unwrap_or(0);
            }
            at = at.checked_add(12 + len as usize)?;
        }
        return f(w, h, frames > 1);
    }
    if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        // Walk the marker segments to the first SOFn (ITU T.81 B.2.2): FF Cn Lh Ll P Yh Yl Xh Xl.
        let mut i = 2usize;
        for _ in 0..512 {
            if *b.get(i)? != 0xFF {
                return None;
            }
            let mut m = *b.get(i + 1)?;
            while m == 0xFF {
                i += 1;
                m = *b.get(i + 1)?;
            }
            if matches!(m, 0xD0..=0xD9 | 0x01) {
                i += 2;
                continue;
            }
            if matches!(m, 0xC0..=0xCF) && !matches!(m, 0xC4 | 0xC8 | 0xCC) {
                return f(be16(b, i + 7)?, be16(b, i + 5)?, false);
            }
            i = i.checked_add(2 + be16(b, i + 2)? as usize)?;
        }
        return None;
    }
    if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        let (w, h) = (le16(b, 6)?, le16(b, 8)?);
        let flags = *b.get(10)?;
        let mut i = 13 + if flags & 0x80 != 0 { 3 << ((flags & 7) + 1) } else { 0 };
        let mut images = 0u32;
        let skip_sub = |mut i: usize| -> Option<usize> {
            loop {
                let n = *b.get(i)? as usize;
                i += 1 + n;
                if n == 0 {
                    return Some(i);
                }
            }
        };
        for _ in 0..100_000 {
            match b.get(i) {
                Some(0x21) => i = match skip_sub(i + 2) { Some(n) => n, None => break },
                Some(0x2C) => {
                    images += 1;
                    if images > 1 {
                        break;
                    }
                    let lf = match b.get(i + 9) { Some(v) => *v, None => break };
                    i += 10 + if lf & 0x80 != 0 { 3 << ((lf & 7) + 1) } else { 0 };
                    i = match skip_sub(i + 1) { Some(n) => n, None => break };
                }
                _ => break,
            }
        }
        return f(w, h, images > 1);
    }
    if b.starts_with(b"BM") {
        if crate::mime_of(b) != Some("image/bmp") {
            return None; // OPENERS' BMP rule: reserved words zero and a DIB size the format defines
        }
        let dib = le32(b, 14)?;
        if dib == 12 {
            return f(le16(b, 18)?, le16(b, 20)?, false);
        }
        let w = le32(b, 18)? as i32;
        let h = le32(b, 22)? as i32;
        return f(w.unsigned_abs(), h.unsigned_abs(), false);
    }
    if b.starts_with(b"qoif") {
        return f(be32(b, 4)?, be32(b, 8)?, false);
    }
    if b.len() >= 12 && &b[0..4] == b"RIFF" && &b[8..12] == b"WEBP" {
        // The first chunk names the layout (WebP container spec): VP8X carries the canvas and the ANIM flag.
        return match b.get(12..16)? {
            b"VP8X" => f(le24(b, 24)? + 1, le24(b, 27)? + 1, *b.get(20)? & 0x02 != 0),
            b"VP8L" => {
                if *b.get(20)? != 0x2F {
                    return None;
                }
                let v = le32(b, 21)?;
                f((v & 0x3FFF) + 1, ((v >> 14) & 0x3FFF) + 1, false)
            }
            b"VP8 " => {
                if b.get(23..26)? != [0x9D, 0x01, 0x2A] {
                    return None;
                }
                f(le16(b, 26)? & 0x3FFF, le16(b, 28)? & 0x3FFF, false)
            }
            _ => None,
        };
    }
    svg_facts(b)
}

/// The `<svg …>` root's size: `width`/`height` (a number, `px` or unitless), else the `viewBox`'s last two numbers.
pub fn svg_facts(b: &[u8]) -> Option<ImageFacts> {
    let head = &b[..b.len().min(4096)];
    let s = core::str::from_utf8(head).ok().or_else(|| core::str::from_utf8(&head[..head.len().saturating_sub(3)]).ok())?;
    let start = s.find("<svg")?;
    let tag = &s[start..start + s[start..].find('>')?];
    let attr = |name: &str| -> Option<&str> {
        let mut from = 0;
        while let Some(p) = tag[from..].find(name) {
            let at = from + p;
            let before = tag.as_bytes().get(at.wrapping_sub(1)).copied().unwrap_or(b' ');
            let rest = tag[at + name.len()..].trim_start();
            if before.is_ascii_whitespace() && rest.starts_with('=') {
                let rest = rest[1..].trim_start();
                let q = rest.chars().next()?;
                if q == '"' || q == '\'' {
                    let body = &rest[1..];
                    return Some(&body[..body.find(q)?]);
                }
            }
            from = at + name.len();
        }
        None
    };
    let num = |v: &str| -> Option<u32> {
        let v = v.trim().trim_end_matches("px");
        let int = v.split('.').next()?;
        if int.is_empty() || !int.bytes().all(|c| c.is_ascii_digit()) {
            return None;
        }
        int.parse::<u32>().ok()
    };
    let (w, h) = match (attr("width").and_then(num), attr("height").and_then(num)) {
        (Some(w), Some(h)) => (w, h),
        _ => {
            let vb = attr("viewBox")?;
            let mut it = vb.split(|c: char| c == ',' || c.is_ascii_whitespace()).filter(|t| !t.is_empty()).skip(2);
            (num(it.next()?)?, num(it.next()?)?)
        }
    };
    if w > 0 && h > 0 { Some(ImageFacts { width: w, height: h, animated: false }) } else { None }
}
