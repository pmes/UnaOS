// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! I420 → RGBA as WebP decoders present it: BT.601 limited-range coefficients in 14-bit fixed
//! point and "fancy" (bilinear 9-3-3-1) chroma upsampling, evaluated in the same integer steps as
//! libwebp's `UpsampleRgbaLinePair` so a lossy WebP lands on the same RGB a browser shows.
//! (AV1/AVIF's float H.273 converter lives in av1_core; this one is the WebP-shaped twin.)

use alloc::vec;
use alloc::vec::Vec;

use crate::Yuv420;

const YUV_FIX2: i32 = 6;
const YUV_MASK2: i32 = (256 << YUV_FIX2) - 1;

#[inline]
fn mult_hi(v: i32, coeff: i32) -> i32 {
    (v * coeff) >> 8
}
#[inline]
fn clip8(v: i32) -> u8 {
    if v & !YUV_MASK2 == 0 { (v >> YUV_FIX2) as u8 } else if v < 0 { 0 } else { 255 }
}

/// One pixel, Y'CbCr (limited range) → R'G'B'.
#[inline]
pub fn yuv_to_rgb(y: u8, u: u8, v: u8) -> [u8; 3] {
    let (y, u, v) = (y as i32, u as i32, v as i32);
    [
        clip8(mult_hi(y, 19077) + mult_hi(v, 26149) - 14234),
        clip8(mult_hi(y, 19077) - mult_hi(u, 6419) - mult_hi(v, 13320) + 8708),
        clip8(mult_hi(y, 19077) + mult_hi(u, 33050) - 17685),
    ]
}

/// Upsample one or two luma rows sharing the chroma rows `top_uv` / `cur_uv` (libwebp's
/// line-pair kernel). `uv` values are packed `u | v << 16`.
#[allow(clippy::too_many_arguments)]
fn line_pair(top_y: &[u8], bot_y: Option<&[u8]>, tu: &[u8], tv: &[u8], cu: &[u8], cv: &[u8], top: &mut [u8], bot: Option<&mut [u8]>, len: usize) {
    let load = |u: &[u8], v: &[u8], i: usize| u[i] as u32 | (v[i] as u32) << 16;
    let put = |dst: &mut [u8], x: usize, y: u8, uv: u32| {
        let rgb = yuv_to_rgb(y, (uv & 0xff) as u8, (uv >> 16) as u8);
        dst[x * 4..x * 4 + 3].copy_from_slice(&rgb);
        dst[x * 4 + 3] = 255;
    };
    let last_pair = (len - 1) >> 1;
    let mut tl = load(tu, tv, 0);
    let mut l = load(cu, cv, 0);
    let mut bot = bot;
    put(top, 0, top_y[0], (3 * tl + l + 0x0002_0002) >> 2);
    if let (Some(by), Some(b)) = (bot_y, bot.as_deref_mut()) {
        put(b, 0, by[0], (3 * l + tl + 0x0002_0002) >> 2);
    }
    for x in 1..=last_pair {
        let t = load(tu, tv, x);
        let uv = load(cu, cv, x);
        let avg = tl + t + l + uv + 0x0008_0008;
        let diag_12 = (avg + 2 * (t + l)) >> 3;
        let diag_03 = (avg + 2 * (tl + uv)) >> 3;
        put(top, 2 * x - 1, top_y[2 * x - 1], ((diag_12 + tl) >> 1) & 0x00ff_00ff);
        put(top, 2 * x, top_y[2 * x], ((diag_03 + t) >> 1) & 0x00ff_00ff);
        if let (Some(by), Some(b)) = (bot_y, bot.as_deref_mut()) {
            put(b, 2 * x - 1, by[2 * x - 1], ((diag_03 + l) >> 1) & 0x00ff_00ff);
            put(b, 2 * x, by[2 * x], ((diag_12 + uv) >> 1) & 0x00ff_00ff);
        }
        tl = t;
        l = uv;
    }
    if len & 1 == 0 {
        put(top, len - 1, top_y[len - 1], (3 * tl + l + 0x0002_0002) >> 2);
        if let (Some(by), Some(b)) = (bot_y, bot.as_mut()) {
            put(b, len - 1, by[len - 1], (3 * l + tl + 0x0002_0002) >> 2);
        }
    }
}

/// The whole picture to straight RGBA (alpha 255), fancy-upsampled.
pub fn to_rgba(p: &Yuv420) -> Vec<u8> {
    let (w, h) = (p.width as usize, p.height as usize);
    let cw = p.chroma_width() as usize;
    let mut out = vec![0u8; w * h * 4];
    let yrow = |r: usize| &p.y[r * w..r * w + w];
    let urow = |r: usize| &p.u[r * cw..r * cw + cw];
    let vrow = |r: usize| &p.v[r * cw..r * cw + cw];
    // Row 0 alone, with its own chroma row on both sides.
    {
        let (first, _) = out.split_at_mut(w * 4);
        line_pair(yrow(0), None, urow(0), vrow(0), urow(0), vrow(0), first, None, w);
    }
    // Then rows (2j-1, 2j) between chroma rows j-1 and j.
    let mut y = 1;
    while y + 1 < h {
        let j = y.div_ceil(2);
        let (a, b) = out[y * w * 4..(y + 2) * w * 4].split_at_mut(w * 4);
        line_pair(yrow(y), Some(yrow(y + 1)), urow(j - 1), vrow(j - 1), urow(j), vrow(j), a, Some(b), w);
        y += 2;
    }
    // An even height leaves the last row, with the last chroma row on both sides.
    if h > 1 && h % 2 == 0 {
        let j = (h - 1) / 2;
        let last = &mut out[(h - 1) * w * 4..h * w * 4];
        line_pair(yrow(h - 1), None, urow(j), vrow(j), urow(j), vrow(j), last, None, w);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grey_and_primaries() {
        assert_eq!(yuv_to_rgb(16, 128, 128), [0, 0, 0]);
        assert_eq!(yuv_to_rgb(235, 128, 128), [255, 255, 255]);
        let r = yuv_to_rgb(81, 90, 240);
        assert!(r[0] >= 253 && r[1] <= 2 && r[2] <= 2, "{r:?}");
    }

    #[test]
    fn flat_picture_stays_flat() {
        for (w, h) in [(1, 1), (2, 2), (3, 5), (16, 9)] {
            let p = Yuv420 {
                width: w,
                height: h,
                y: vec![100; (w * h) as usize],
                u: vec![60; (w.div_ceil(2) * h.div_ceil(2)) as usize],
                v: vec![200; (w.div_ceil(2) * h.div_ceil(2)) as usize],
            };
            let px = to_rgba(&p);
            let want = yuv_to_rgb(100, 60, 200);
            for c in px.chunks_exact(4) {
                assert_eq!(&c[..3], &want);
            }
        }
    }
}
