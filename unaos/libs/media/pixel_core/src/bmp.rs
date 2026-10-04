// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! BMP decoder — the Windows DIB family as Microsoft documents it (BITMAPFILEHEADER; BITMAPCOREHEADER,
//! BITMAPINFOHEADER, BITMAPV2/V3INFOHEADER, BITMAPV4HEADER, BITMAPV5HEADER; OS/2 2.x 64-byte header).
//!
//! Covered: 1/4/8-bit palettes (RGBTRIPLE for core headers, RGBQUAD otherwise; `biClrUsed` honoured),
//! 16-bit (BI_RGB = X1R5G5B5; BI_BITFIELDS any masks), 24-bit BGR, 32-bit (BI_RGB = BGRX, opaque;
//! BI_BITFIELDS / BI_ALPHABITFIELDS with an alpha mask from a V3+ header or the mask block), bottom-up
//! (positive height) and top-down (negative height) row order, 4-byte row padding, BI_RLE8 and BI_RLE4
//! (pixels skipped by delta / end-of-line / end-of-bitmap are transparent, as browsers draw them), and
//! BI_PNG / BI_JPEG (the embedded stream is handed to this crate's own PNG / JPEG decoder).
//! Mask channels narrower than 8 bits are widened by `round(v·255 / (2^n − 1))`.
//!
//! NOT decoded: 2-bit and 64-bit DIBs, OS/2 Huffman 1D and RLE24, embedded ICC profiles (V5).

use crate::{Error, Image, le16, le32};

/// Decode a BMP file to RGBA8.
pub fn decode(b: &[u8]) -> Result<Image, Error> {
    if b.len() < 26 || &b[0..2] != b"BM" {
        return Err(Error::Malformed("bmp header"));
    }
    let off = le32(b, 10) as usize;
    let hs = le32(b, 14) as usize;
    let h0 = 14 + hs;
    if b.len() < h0 {
        return Err(Error::Truncated);
    }
    let (width, height, bpp, comp, clr_used);
    if hs == 12 {
        width = le16(b, 18) as i32;
        height = le16(b, 20) as i16 as i32;
        bpp = le16(b, 24);
        comp = 0;
        clr_used = 0;
    } else if hs >= 40 {
        width = le32(b, 18) as i32;
        height = le32(b, 22) as i32;
        bpp = le16(b, 28);
        comp = le32(b, 30);
        clr_used = le32(b, 46) as usize;
    } else if hs == 16 || hs == 64 {
        // OS/2 2.x: the first 16 bytes match BITMAPINFOHEADER.
        width = le32(b, 18) as i32;
        height = le32(b, 22) as i32;
        bpp = le16(b, 28);
        comp = if hs >= 20 { le32(b, 30) } else { 0 };
        clr_used = if hs >= 36 { le32(b, 46) as usize } else { 0 };
    } else {
        return Err(Error::Unsupported("bmp header size"));
    }
    if width <= 0 || height == 0 || height == i32::MIN {
        return Err(Error::Malformed("bmp dimensions"));
    }
    let top_down = height < 0;
    let (w, h) = (width as u32, height.unsigned_abs());
    // BI_JPEG / BI_PNG: the pixel array IS a JPEG / PNG stream.
    if comp == 4 || comp == 5 {
        let data = b.get(off..).ok_or(Error::Truncated)?;
        return if comp == 5 { crate::png::decode(data) } else { crate::jpeg::decode(data) };
    }
    let len = crate::rgba_len(w, h)?;
    let (w, h) = (w as usize, h as usize);

    // Masks: inside a V2+ header (offset 40), or a 12/16-byte block after a 40-byte header.
    let mut masks = [0u32; 4];
    let bitfields = comp == 3 || comp == 6;
    if bitfields {
        let m = if hs >= 52 { 14 + 40 } else { h0 };
        let n = if hs >= 56 || comp == 6 { 4 } else { 3 };
        if b.len() < m + 4 * n {
            return Err(Error::Truncated);
        }
        for (i, mask) in masks.iter_mut().enumerate().take(n) {
            *mask = le32(b, m + 4 * i);
        }
    } else if bpp == 16 {
        masks = [0x7C00, 0x03E0, 0x001F, 0];
    } else if bpp == 32 {
        masks = [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0];
    }
    if bitfields && hs >= 56 && hs != 64 {
        masks[3] = le32(b, 14 + 52);
    }

    // Palette.
    let mut pal: [[u8; 4]; 256] = [[0, 0, 0, 255]; 256];
    if bpp <= 8 {
        let entry = if hs == 12 { 3 } else { 4 };
        let pstart = h0 + if bitfields && hs == 40 { 12 } else { 0 };
        let max = 1usize << bpp;
        let n = if clr_used == 0 || clr_used > max { max } else { clr_used };
        for (i, p) in pal.iter_mut().enumerate().take(n) {
            let q = pstart + i * entry;
            if q + 3 > b.len() {
                break;
            }
            *p = [b[q + 2], b[q + 1], b[q], 255];
        }
    }

    let mut out = crate::zeroed(len)?;
    let pix = b.get(off..).ok_or(Error::Truncated)?;
    // Row `r` of the file is display row `dy(r)`.
    let dy = |r: usize| if top_down { r } else { h - 1 - r };

    match (comp, bpp) {
        (1, 8) | (2, 4) => {
            rle(pix, w, h, bpp, &pal, &mut out, dy)?;
        }
        (0 | 3 | 6, 1 | 4 | 8 | 16 | 24 | 32) => {
            if bpp <= 8 && bitfields {
                return Err(Error::Malformed("bmp bitfields on a palette image"));
            }
            let stride = (w * bpp as usize).div_ceil(32) * 4;
            let ch: [(u32, u32); 4] = core::array::from_fn(|i| field(masks[i]));
            for r in 0..h {
                let row = match pix.get(r * stride..r * stride + stride) {
                    Some(x) => x,
                    None if r * stride < pix.len() => &pix[r * stride..],
                    None => break, // short pixel array: remaining rows stay transparent
                };
                let o = dy(r) * w * 4;
                for x in 0..w {
                    let px: [u8; 4] = match bpp {
                        1 | 4 | 8 => {
                            let bit = x * bpp as usize;
                            let Some(&byte) = row.get(bit / 8) else { break };
                            let i = (byte >> (8 - bpp as usize - bit % 8)) & ((1u16 << bpp) - 1) as u8;
                            pal[i as usize]
                        }
                        24 => {
                            let Some(p) = row.get(x * 3..x * 3 + 3) else { break };
                            [p[2], p[1], p[0], 255]
                        }
                        _ => {
                            let v = if bpp == 16 {
                                let Some(p) = row.get(x * 2..x * 2 + 2) else { break };
                                u16::from_le_bytes([p[0], p[1]]) as u32
                            } else {
                                let Some(p) = row.get(x * 4..x * 4 + 4) else { break };
                                u32::from_le_bytes([p[0], p[1], p[2], p[3]])
                            };
                            let a = if masks[3] == 0 { 255 } else { widen(v, ch[3]) };
                            [widen(v, ch[0]), widen(v, ch[1]), widen(v, ch[2]), a]
                        }
                    };
                    out[o + x * 4..o + x * 4 + 4].copy_from_slice(&px);
                }
            }
        }
        _ => return Err(Error::Unsupported("bmp compression / bit depth")),
    }
    Ok(Image::still(w as u32, h as u32, out))
}

/// `(shift, bits)` of a contiguous channel mask.
fn field(mask: u32) -> (u32, u32) {
    if mask == 0 {
        return (0, 0);
    }
    let shift = mask.trailing_zeros();
    let bits = (mask >> shift).trailing_ones().min(32 - shift);
    (shift, bits)
}

/// Extract a mask channel and widen it to 8 bits: `round(v * 255 / (2^bits - 1))` below 8 bits (the
/// table Blink's BMP reader uses), the top 8 bits at 8 or more.
fn widen(v: u32, (shift, bits): (u32, u32)) -> u8 {
    if bits == 0 {
        return 0;
    }
    let x = (v >> shift) & ((1u64 << bits) - 1) as u32;
    if bits >= 8 {
        return (x >> (bits - 8)) as u8;
    }
    let max = (1u32 << bits) - 1;
    ((x * 255 + max / 2) / max) as u8
}

/// BI_RLE8 / BI_RLE4 (the run-length encodings of the DIB spec). Unwritten pixels stay transparent.
fn rle(
    p: &[u8],
    w: usize,
    h: usize,
    bpp: u16,
    pal: &[[u8; 4]; 256],
    out: &mut [u8],
    dy: impl Fn(usize) -> usize,
) -> Result<(), Error> {
    let (mut x, mut r, mut i) = (0usize, 0usize, 0usize);
    let mut put = |x: usize, r: usize, idx: u8| {
        if x < w && r < h {
            let o = (dy(r) * w + x) * 4;
            out[o..o + 4].copy_from_slice(&pal[idx as usize]);
        }
    };
    while i + 1 < p.len() && r < h {
        let (n, c) = (p[i] as usize, p[i + 1]);
        i += 2;
        if n > 0 {
            // Encoded run: `n` pixels of `c` (RLE4 alternates its two nibbles).
            for k in 0..n {
                let idx = if bpp == 8 { c } else if k % 2 == 0 { c >> 4 } else { c & 15 };
                put(x, r, idx);
                x += 1;
            }
            continue;
        }
        match c {
            0 => {
                x = 0;
                r += 1;
            }
            1 => break,
            2 => {
                if i + 1 >= p.len() {
                    break;
                }
                x += p[i] as usize;
                r += p[i + 1] as usize;
                i += 2;
            }
            n => {
                // Absolute mode: `n` literal pixels, padded to a 16-bit boundary.
                let n = n as usize;
                let bytes = if bpp == 8 { n } else { n.div_ceil(2) };
                for k in 0..n {
                    let Some(&byte) = p.get(i + if bpp == 8 { k } else { k / 2 }) else { break };
                    let idx = if bpp == 8 { byte } else if k % 2 == 0 { byte >> 4 } else { byte & 15 };
                    put(x, r, idx);
                    x += 1;
                }
                i += bytes + (bytes & 1);
            }
        }
    }
    Ok(())
}
