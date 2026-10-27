// CHARTER: Facet (Images) — host face over the shared cores (RASTERFACE, LEDGER SR234)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The canvas: the view transform ([`View`]: zoom in powers of two about a point, pan by a delta) and the paint of
//! the document's composite through it, over a checkerboard where it is transparent.

use raster_core::Mask;

/// The smallest and largest zoom exponents: scale `2^z` from 1/8 to 32.
pub const ZOOM_MIN: i32 = -3;
pub const ZOOM_MAX: i32 = 5;
/// The checkerboard's square, in screen pixels.
pub const CHECK: u32 = 8;
/// The grey outside the document.
pub const BACKDROP: u8 = 64;

/// `screen = origin + canvas * 2^zoom`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct View {
    pub zoom: i32,
    pub origin: (f64, f64),
}

impl View {
    /// The largest power of two (at most 1) that shows a `dw x dh` document in a `vw x vh` region, centred.
    pub fn fit(dw: u32, dh: u32, vw: u32, vh: u32) -> View {
        let mut zoom = 0;
        while zoom > ZOOM_MIN && (scale_of(zoom) * dw as f64 > vw as f64 || scale_of(zoom) * dh as f64 > vh as f64) {
            zoom -= 1;
        }
        let s = scale_of(zoom);
        let origin = (((vw as f64 - dw as f64 * s) / 2.0).floor(), ((vh as f64 - dh as f64 * s) / 2.0).floor());
        View { zoom, origin }
    }

    pub fn scale(&self) -> f64 {
        scale_of(self.zoom)
    }

    /// The scale as the witness prints it: `2`, `1`, `1/4`.
    pub fn scale_word(&self) -> String {
        if self.zoom >= 0 { format!("{}", 1u32 << self.zoom) } else { format!("1/{}", 1u32 << -self.zoom) }
    }

    /// The canvas point (fractional) under a screen point.
    pub fn to_canvas(&self, sx: f64, sy: f64) -> (f64, f64) {
        let s = self.scale();
        ((sx - self.origin.0) / s, (sy - self.origin.1) / s)
    }

    /// The canvas cell under a screen point.
    pub fn cell(&self, sx: f64, sy: f64) -> (i64, i64) {
        let (x, y) = self.to_canvas(sx, sy);
        (x.floor() as i64, y.floor() as i64)
    }

    /// Zoom by `steps` powers of two about the screen point `(cx, cy)`: the canvas point under it stays put.
    /// False (and no change) when it would pass a bound.
    pub fn zoom_at(&mut self, steps: i32, cx: f64, cy: f64) -> bool {
        let z = self.zoom + steps;
        if !(ZOOM_MIN..=ZOOM_MAX).contains(&z) {
            return false;
        }
        let (px, py) = self.to_canvas(cx, cy);
        self.zoom = z;
        let s = self.scale();
        self.origin = (cx - px * s, cy - py * s);
        true
    }

    pub fn pan(&mut self, dx: f64, dy: f64) {
        self.origin = (self.origin.0 + dx, self.origin.1 + dy);
    }
}

fn scale_of(zoom: i32) -> f64 {
    if zoom >= 0 { (1u64 << zoom) as f64 } else { 1.0 / (1u64 << -zoom) as f64 }
}

/// The checkerboard grey at screen `(x, y)`.
pub fn check(x: u32, y: u32) -> u8 {
    if ((x / CHECK) + (y / CHECK)) % 2 == 0 { 204 } else { 255 }
}

/// Straight RGBA `c` over an opaque grey `g`.
pub fn over_grey(c: [u8; 4], g: u8) -> [u8; 3] {
    let a = c[3] as u32;
    let mix = |v: u8| ((v as u32 * a + g as u32 * (255 - a) + 127) / 255) as u8;
    [mix(c[0]), mix(c[1]), mix(c[2])]
}

/// Paint the canvas region `rw x rh` of a `stride`-px-wide RGBA frame: `comp` (the `dw x dh` composite) through
/// `view`, checkerboard under it, selected cells tinted a quarter toward blue, the backdrop elsewhere.
pub fn paint(frame: &mut [u8], stride: u32, rw: u32, rh: u32, comp: &[u8], dw: u32, dh: u32, view: &View, sel: Option<&Mask>) {
    for sy in 0..rh {
        for sx in 0..rw {
            let (cx, cy) = view.cell(sx as f64 + 0.5, sy as f64 + 0.5);
            let o = ((sy * stride + sx) * 4) as usize;
            let rgb = if cx >= 0 && cy >= 0 && cx < dw as i64 && cy < dh as i64 {
                let i = ((cy as u32 * dw + cx as u32) * 4) as usize;
                let mut rgb = over_grey([comp[i], comp[i + 1], comp[i + 2], comp[i + 3]], check(sx, sy));
                if sel.is_some_and(|m| m.at(cx, cy) > 0) {
                    rgb = [rgb[0] - rgb[0] / 4, rgb[1] - rgb[1] / 4, rgb[2] + (255 - rgb[2]) / 4];
                }
                rgb
            } else {
                [BACKDROP; 3]
            };
            frame[o..o + 4].copy_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
    }
}
