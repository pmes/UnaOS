// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Compositing a stamp onto an RGBA8 buffer. There is no compositor here: every pixel goes through
//! `raster_core::composite::blend_pixel`, the call raster_core's own layer compositor makes — Normal is pixel_core's
//! `blend_nonpremult` behind raster_core's two exact identities (an opaque source, a transparent destination), the
//! separable modes choose the colour first, and `Erase` is destination-out with the colour kept.

use raster_core::BlendMode;
use raster_core::composite::blend_pixel;

use crate::brush::Brush;
use crate::mul255;
use crate::stamp::{Stamp, stamp};
use crate::stroke::{Stroke, dabs};

/// Composite `rgba` (straight RGBA8) through `stamp` onto `pixels` (`w x h`, RGBA8 straight, row-major) under
/// `mode`. Per pixel `a = round(rgba.a * coverage * stamp.flow / 255²)`. Stamp pixels off the canvas are skipped; a
/// buffer whose length is not `w * h * 4` (or a stamp whose coverage is not `w * h`) is left untouched.
pub fn apply(pixels: &mut [u8], w: u32, h: u32, stamp: &Stamp, rgba: [u8; 4], mode: BlendMode) {
    let (cw, ch) = (w as i64, h as i64);
    if pixels.len() as u64 != w as u64 * h as u64 * 4 {
        return;
    }
    if stamp.coverage.len() as u64 != stamp.w as u64 * stamp.h as u64 {
        return;
    }
    let (sx, sy) = (stamp.x0 as i64, stamp.y0 as i64);
    let x0 = sx.max(0);
    let y0 = sy.max(0);
    let x1 = (sx + stamp.w as i64).min(cw);
    let y1 = (sy + stamp.h as i64).min(ch);
    let src_a = rgba[3] as u32;
    let flow = stamp.flow as u32;
    for y in y0..y1 {
        for x in x0..x1 {
            let c = stamp.coverage[((y - sy) * stamp.w as i64 + (x - sx)) as usize] as u32;
            let a = (src_a * c * flow + 65025 / 2) / 65025;
            if a == 0 {
                continue;
            }
            let i = ((y * cw + x) * 4) as usize;
            let d = [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]];
            pixels[i..i + 4].copy_from_slice(&blend_pixel(mode, rgba, a as u8, d));
        }
    }
}

/// Paint `stroke` with `brush` in colour `rgba` onto `pixels` (`w x h` RGBA8): `rgba.a` scaled by the brush's
/// opacity, then [`dabs`] → [`stamp`] → [`apply`] in stroke order under `brush.mode`.
pub fn paint(pixels: &mut [u8], w: u32, h: u32, brush: &Brush, stroke: &Stroke, rgba: [u8; 4]) {
    let colour = [rgba[0], rgba[1], rgba[2], mul255(rgba[3] as u32, brush.opacity as u32) as u8];
    for dab in dabs(brush, stroke) {
        apply(pixels, w, h, &stamp(&dab), colour, brush.mode);
    }
}
