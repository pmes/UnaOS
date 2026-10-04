// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! QOI decoder — "The Quite OK Image Format, Specification Version 1.0" (qoiformat.org, 2022-01-05),
//! implemented whole: the 14-byte header, the six chunk types (QOI_OP_RGB, QOI_OP_RGBA, QOI_OP_INDEX,
//! QOI_OP_DIFF, QOI_OP_LUMA, QOI_OP_RUN) with the 8-bit tags taking precedence over the 2-bit ones,
//! the 64-entry index with hash `(r*3 + g*5 + b*7 + a*11) % 64`, the start pixel (0,0,0,255) and the
//! end marker. Lossless and exact: the decoded RGBA IS the image. `channels` and `colorspace` are
//! informative only (the spec says they do not change decoding); a 3-channel file keeps alpha 255.

use crate::{Error, Image, be32};

/// Decode a QOI file to RGBA8.
pub fn decode(b: &[u8]) -> Result<Image, Error> {
    if b.len() < 14 + 8 || &b[0..4] != b"qoif" {
        return Err(Error::Malformed("qoi header"));
    }
    let (w, h) = (be32(b, 4), be32(b, 8));
    if !(3..=4).contains(&b[12]) || b[13] > 1 {
        return Err(Error::Malformed("qoi channels/colorspace"));
    }
    let len = crate::rgba_len(w, h)?;
    let mut out = crate::zeroed(len)?;
    let mut index = [[0u8; 4]; 64];
    let mut px = [0u8, 0, 0, 255];
    let mut p = 14usize;
    let end = b.len() - 8;
    let mut o = 0usize;
    let mut run = 0u32;
    while o < len {
        if run > 0 {
            run -= 1;
        } else {
            if p >= end {
                return Err(Error::Truncated);
            }
            let t = b[p];
            p += 1;
            match t {
                0xFE => {
                    if p + 3 > end {
                        return Err(Error::Truncated);
                    }
                    px[..3].copy_from_slice(&b[p..p + 3]);
                    p += 3;
                }
                0xFF => {
                    if p + 4 > end {
                        return Err(Error::Truncated);
                    }
                    px.copy_from_slice(&b[p..p + 4]);
                    p += 4;
                }
                _ => match t >> 6 {
                    0 => px = index[(t & 63) as usize],
                    1 => {
                        px[0] = px[0].wrapping_add((t >> 4) & 3).wrapping_sub(2);
                        px[1] = px[1].wrapping_add((t >> 2) & 3).wrapping_sub(2);
                        px[2] = px[2].wrapping_add(t & 3).wrapping_sub(2);
                    }
                    2 => {
                        if p >= end {
                            return Err(Error::Truncated);
                        }
                        let dg = (t & 63).wrapping_sub(32);
                        let n = b[p];
                        p += 1;
                        px[0] = px[0].wrapping_add(dg).wrapping_add(n >> 4).wrapping_sub(8);
                        px[1] = px[1].wrapping_add(dg);
                        px[2] = px[2].wrapping_add(dg).wrapping_add(n & 15).wrapping_sub(8);
                    }
                    _ => run = (t & 63) as u32, // this pixel plus `run` more
                },
            }
            let hsh = (px[0] as usize * 3 + px[1] as usize * 5 + px[2] as usize * 7 + px[3] as usize * 11) % 64;
            index[hsh] = px;
        }
        out[o..o + 4].copy_from_slice(&px);
        o += 4;
    }
    Ok(Image::still(w, h, out))
}
