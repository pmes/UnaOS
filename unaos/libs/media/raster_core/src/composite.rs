// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The one compositor. Every src-over in this crate is pixel_core's [`blend_nonpremult`]; the separable blend modes
//! only choose the source COLOUR (W3C Compositing 1 §5.8: `Cs' = (1 - ab) Cs + ab B(Cb, Cs)`) before that call, and
//! Erase is destination-out (it removes alpha and composites no colour).
//!
//! Two exact identities of src-over are taken before the call: an opaque source IS the result, and over a fully
//! transparent destination the source IS the result. `blend_nonpremult` returns `src - 1` per channel for an opaque
//! source (`floor(2^24 / 255) * 255 = 2^24 - 1`); libwebp's own caller skips opaque sources for the same reason.

use alloc::vec;
use alloc::vec::Vec;

use pixel_core::blend_nonpremult;

use crate::document::{BlendMode, Document, Layer};

/// `round(a * b / 255)` for bytes.
#[inline]
pub(crate) fn mul255(a: u32, b: u32) -> u32 {
    (a * b + 127) / 255
}

/// The separable blend function `B(b, s)` of one channel (b = backdrop, s = source), 0..=255.
pub fn blend_channel(mode: BlendMode, b: u8, s: u8) -> u8 {
    let (b, s) = (b as u32, s as u32);
    (match mode {
        BlendMode::Normal | BlendMode::Erase => s,
        BlendMode::Multiply => mul255(b, s),
        BlendMode::Screen => b + s - mul255(b, s),
        BlendMode::Overlay => {
            if b < 128 {
                (2 * b * s + 127) / 255
            } else {
                255 - (2 * (255 - b) * (255 - s) + 127) / 255
            }
        }
        BlendMode::Darken => b.min(s),
        BlendMode::Lighten => b.max(s),
        BlendMode::Difference => b.abs_diff(s),
    }) as u8
}

/// Src-over of `src` (straight RGBA) onto `dst`: the two exact identities, else pixel_core's `blend_nonpremult`.
#[inline]
pub(crate) fn over(src: [u8; 4], dst: [u8; 4]) -> [u8; 4] {
    if src[3] == 255 || dst[3] == 0 {
        if src[3] == 0 {
            return dst;
        }
        return src;
    }
    blend_nonpremult(src, dst)
}

/// One layer pixel `src` (alpha already scaled by opacity and mask to `a`) meeting `dst` under `mode`.
pub fn blend_pixel(mode: BlendMode, src: [u8; 4], a: u8, dst: [u8; 4]) -> [u8; 4] {
    if a == 0 {
        return dst;
    }
    match mode {
        BlendMode::Erase => [dst[0], dst[1], dst[2], (mul255(dst[3] as u32, 255 - a as u32)) as u8],
        BlendMode::Normal => over([src[0], src[1], src[2], a], dst),
        m => {
            let ab = dst[3] as u32;
            let mix = |i: usize| {
                let bl = blend_channel(m, dst[i], src[i]) as u32;
                (((255 - ab) * src[i] as u32 + ab * bl + 127) / 255) as u8
            };
            over([mix(0), mix(1), mix(2), a], dst)
        }
    }
}

/// Composite one layer onto the canvas buffer `out` (`cw x ch`, RGBA8).
fn layer_onto(l: &Layer, out: &mut [u8], cw: i64, ch: i64) {
    let (ox, oy) = (l.offset.0 as i64, l.offset.1 as i64);
    let x0 = ox.max(0);
    let y0 = oy.max(0);
    let x1 = (ox + l.width as i64).min(cw);
    let y1 = (oy + l.height as i64).min(ch);
    for cy in y0..y1 {
        for cx in x0..x1 {
            let (lx, ly) = (cx - ox, cy - oy);
            let Some(p) = l.pixel(lx, ly) else { continue };
            let a = mul255(mul255(p[3] as u32, l.opacity as u32), l.mask_at(lx, ly) as u32) as u8;
            let i = ((cy * cw + cx) * 4) as usize;
            let d = [out[i], out[i + 1], out[i + 2], out[i + 3]];
            out[i..i + 4].copy_from_slice(&blend_pixel(l.blend, p, a, d));
        }
    }
}

impl Document {
    /// The flattened canvas: RGBA8, `width * height * 4`, starting transparent; layers bottom first, hidden layers
    /// skipped, each through its mask and opacity and its blend mode.
    pub fn composite(&self) -> Vec<u8> {
        let (cw, ch) = (self.width as i64, self.height as i64);
        let mut out = vec![0u8; (cw * ch * 4) as usize];
        for l in self.layers.iter().filter(|l| l.visible) {
            layer_onto(l, &mut out, cw, ch);
        }
        out
    }
}
