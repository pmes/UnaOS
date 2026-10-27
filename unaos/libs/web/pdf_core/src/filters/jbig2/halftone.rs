// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Aether — shared-core. JBIG2 pattern dictionaries (T.88 6.7, 7.4.4) and halftone regions (6.6, 7.4.5) with
//! the grey-scale image of Annex C.5 — JBIG2HUFF SR336. Every bitmap goes through the one generic decoder (`generic`)
//! or the one MMR path (`mmr_bitmap` over `ccitt::mmr`); patterns are placed by `Bitmap::compose`.

use alloc::vec;
use alloc::vec::Vec;

use super::super::mq::MqDecoder;
use super::{be16, be32, byte, generic, mmr_bitmap, region_info, Bitmap, Generic, BAD, MAX_SYMBOLS};
use crate::{Error, MAX_DECODED};

/// 7.4.4 / 6.7.5: a pattern dictionary's patterns, cut from one collective bitmap of (GRAYMAX + 1) x HDPW columns.
pub(super) fn pattern_dict(b: &[u8]) -> Result<Vec<Bitmap>, Error> {
    let f = byte(b, 0)?;
    let template = (f >> 1) & 3;
    let (pw, ph) = (byte(b, 1)? as usize, byte(b, 2)? as usize);
    let n = be32(b, 3)? as usize + 1;
    if pw == 0 || ph == 0 || n > MAX_SYMBOLS {
        return Err(BAD);
    }
    let data = b.get(7..).unwrap_or(&[]);
    let all = if f & 1 != 0 {
        mmr_bitmap(data, n * pw, ph)?
    } else {
        let mut dec = MqDecoder::new(data);
        let mut cx = vec![0u8; 1 << 16];
        let at = [(-(pw as i32), 0), (-3, -1), (2, -2), (-2, -2)];
        generic(&mut dec, &mut cx, n * pw, ph, &Generic { template, tpgdon: false, at }, None)?
    };
    (0..n).map(|k| all.columns(k * pw, pw)).collect()
}

/// Annex C.5: the grey-scale image, `bpp` bit planes decoded most significant first on one decoder and one context array
/// (or one MMR plane), Gray-decoded; the values row by row.
fn gray_image(
    data: &[u8],
    mmr: bool,
    w: usize,
    h: usize,
    bpp: u32,
    template: u8,
    skip: Option<&Bitmap>,
) -> Result<Vec<u32>, Error> {
    let mut vals = vec![0u32; w * h];
    if bpp == 0 {
        return Ok(vals);
    }
    let mut planes: Vec<Bitmap> = Vec::with_capacity(bpp as usize);
    if mmr {
        planes.push(mmr_bitmap(data, w, h)?);
    } else {
        let mut dec = MqDecoder::new(data);
        let mut cx = vec![0u8; 1 << 16];
        let at = [(if template <= 1 { 3 } else { 2 }, -1), (-3, -1), (2, -2), (-2, -2)];
        let g = Generic { template, tpgdon: false, at };
        for _ in 0..bpp {
            planes.push(generic(&mut dec, &mut cx, w, h, &g, skip)?);
        }
        // Decoded as GSPLANES[bpp - 1] first: index 0 here is the most significant plane.
        planes.reverse();
    }
    for j in (0..planes.len().saturating_sub(1)).rev() {
        let (lo, hi) = planes.split_at_mut(j + 1);
        for (a, b) in lo[j].data.iter_mut().zip(&hi[0].data) {
            *a ^= *b;
        }
    }
    for (j, p) in planes.iter().enumerate() {
        for (v, &bit) in vals.iter_mut().zip(&p.data) {
            *v |= (bit as u32) << j;
        }
    }
    Ok(vals)
}

/// 7.4.5 / 6.6.5: a halftone region over the referred pattern dictionary `pats`.
pub(super) fn region(b: &[u8], pats: &[Bitmap]) -> Result<Bitmap, Error> {
    let (w, h, ..) = region_info(b)?;
    let f = byte(b, 17)?;
    let mmr = f & 1 != 0;
    let template = (f >> 1) & 3;
    let combop = (f >> 4) & 7;
    let def = (f >> 7) & 1;
    let (gw, gh) = (be32(b, 18)? as usize, be32(b, 22)? as usize);
    let (gx, gy) = (be32(b, 26)? as i32 as i64, be32(b, 30)? as i32 as i64);
    let (rx, ry) = (be16(b, 34)? as i64, be16(b, 36)? as i64);
    let first = pats.first().ok_or(BAD)?;
    let (pw, ph) = (first.w as i64, first.h as i64);
    let mut bpp = 0u32;
    while (1usize << bpp) < pats.len() {
        bpp += 1;
    }
    if mmr && bpp > 1 {
        return Err(Error::Unsupported("JBIG2 halftone MMR"));
    }
    if gw.saturating_mul(gh).saturating_mul(first.w.saturating_mul(first.h)) > MAX_DECODED {
        return Err(Error::TooLarge);
    }
    let mut out = Bitmap::new(w, h, def)?;
    let at = |mg: i64, ng: i64| ((gx + mg * ry + ng * rx) >> 8, (gy + mg * rx - ng * ry) >> 8);
    let skip = if f & 8 != 0 {
        let mut s = Bitmap::new(gw, gh, 0)?;
        for mg in 0..gh {
            for ng in 0..gw {
                let (x, y) = at(mg as i64, ng as i64);
                if x + pw <= 0 || x >= w as i64 || y + ph <= 0 || y >= h as i64 {
                    s.data[mg * gw + ng] = 1;
                }
            }
        }
        Some(s)
    } else {
        None
    };
    let vals = gray_image(b.get(38..).unwrap_or(&[]), mmr, gw, gh, bpp, template, skip.as_ref())?;
    for mg in 0..gh {
        for ng in 0..gw {
            let (x, y) = at(mg as i64, ng as i64);
            let gi = (vals[mg * gw + ng] as usize).min(pats.len() - 1);
            out.compose(&pats[gi], x, y, combop);
        }
    }
    Ok(out)
}
