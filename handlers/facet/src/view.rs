// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The view model: how a picture is framed in a viewport. Display-only — a view is never baked
//! into an export. The render rules, so the eyes suite and Chromium can reproduce them:
//!
//! 1. The view's flips apply first (`flip_h` mirrors left/right, `flip_v` top/bottom), then its
//!    clockwise quarter turns.
//! 2. Scale `s`: `Fit` = `min(vw / iw, vh / ih, 1)` (shrink to fit, never enlarge — the kernel
//!    viewer's rule); `Actual` = 1; `Percent(p)` = `p / 100`, `p` clamped to 1..=6400.
//! 3. The displayed size is `round(iw * s) x round(ih * s)` (at least 1x1). Its top-left corner sits
//!    at `floor((vw - dw) / 2) + pan` on each axis; on an axis where the picture is LARGER than the
//!    viewport the pan is clamped so no gap opens (`vw - dw <= x <= 0`); on an axis where it fits
//!    the picture stays centred and the pan is ignored.
//! 4. Sampling: `s < 1` resamples the whole picture to the displayed size with the documented
//!    triangle filter ([`Raster::resized`]); `s >= 1` is nearest-neighbour — viewport pixel `x`
//!    shows source column `floor((x - left + 0.5) / s)` — so 200 % is exact 2x2 pixel blocks (CSS
//!    `image-rendering: pixelated`).
//! 5. The picture is composited "source-over" onto the field colour in sRGB-encoded space, each
//!    term rounded separately as Skia's 8-bit premultiplied pipeline does (measured against
//!    Chromium, pixel-exact): `out = round(c * a / 255) + round(field * (255 - a) / 255)`, `a` the
//!    8-bit alpha; the result is opaque.

use crate::raster::Raster;
use bandy::signals::{FacetView, FacetZoom};

/// The field behind the picture: UnaOS Moonstone `#2D2B55` (the quartzite image view's `FIELD`).
pub const FIELD: [u8; 3] = [0x2D, 0x2B, 0x55];

/// The scale factor `zoom` gives an `iw x ih` picture in a `vw x vh` viewport.
pub fn scale(zoom: FacetZoom, iw: u32, ih: u32, vw: u32, vh: u32) -> f64 {
    match zoom {
        FacetZoom::Fit => (vw as f64 / iw.max(1) as f64).min(vh as f64 / ih.max(1) as f64).min(1.0),
        FacetZoom::Actual => 1.0,
        FacetZoom::Percent(p) => p.clamp(1, 6400) as f64 / 100.0,
    }
}

/// The picture after the view's flips and turns (rule 1).
pub fn transformed(img: &Raster, view: &FacetView) -> Raster {
    let mut r = if view.flip_h { img.flipped(true) } else { img.clone() };
    if view.flip_v {
        r = r.flipped(false);
    }
    r.rotated(view.quarter_turns)
}

/// Where the displayed picture lands: `(left, top, displayed_w, displayed_h, scale)`.
pub fn placement(iw: u32, ih: u32, vw: u32, vh: u32, view: &FacetView) -> (i64, i64, u32, u32, f64) {
    let s = scale(view.zoom, iw, ih, vw, vh);
    let dw = ((iw as f64 * s).round() as u32).max(1);
    let dh = ((ih as f64 * s).round() as u32).max(1);
    let axis = |v: u32, d: u32, pan: i32| -> i64 {
        let centred = (v as i64 - d as i64).div_euclid(2);
        if d > v { (centred + pan as i64).clamp(v as i64 - d as i64, 0) } else { centred }
    };
    (axis(vw, dw, view.pan_x), axis(vh, dh, view.pan_y), dw, dh, s)
}

/// Render `img` through `view` into a `vw x vh` opaque RGBA frame on `field`.
pub fn render(img: &Raster, view: &FacetView, vw: u32, vh: u32, field: [u8; 3]) -> Raster {
    let pic = transformed(img, view);
    let (left, top, dw, dh, s) = placement(pic.width, pic.height, vw, vh, view);
    let mut out = Raster::filled(vw, vh, [field[0], field[1], field[2], 255]);
    let small = if s < 1.0 { Some(pic.resized(dw, dh)) } else { None };
    // Source column/row for each viewport column/row (None = outside the picture).
    let map = |v: u32, origin: i64, d: u32, src: u32| -> Vec<Option<u32>> {
        (0..v as i64)
            .map(|x| {
                let rel = x - origin;
                if rel < 0 || rel >= d as i64 {
                    return None;
                }
                Some(if small.is_some() {
                    rel as u32
                } else {
                    (((rel as f64 + 0.5) / s).floor() as u32).min(src - 1)
                })
            })
            .collect()
    };
    let src = small.as_ref().unwrap_or(&pic);
    let cols = map(vw, left, dw, src.width);
    let rows = map(vh, top, dh, src.height);
    for (y, sy) in rows.iter().enumerate() {
        let Some(sy) = sy else { continue };
        for (x, sx) in cols.iter().enumerate() {
            let Some(sx) = sx else { continue };
            let p = src.px(*sx, *sy);
            let d = (y * vw as usize + x) * 4;
            if p[3] == 255 {
                out.rgba[d..d + 3].copy_from_slice(&p[..3]);
            } else {
                // Each term rounded on its own: Skia's 8-bit premultiplied source-over.
                let a = p[3] as u32;
                for c in 0..3 {
                    let src = (p[c] as u32 * a + 127) / 255;
                    let dst = (field[c] as u32 * (255 - a) + 127) / 255;
                    out.rgba[d + c] = (src + dst).min(255) as u8;
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(zoom: FacetZoom) -> FacetView {
        FacetView { zoom, ..FacetView::default() }
    }

    #[test]
    fn kat_fit_never_enlarges_and_centres() {
        let img = Raster::filled(4, 2, [255, 0, 0, 255]);
        let out = render(&img, &view(FacetZoom::Fit), 8, 6, FIELD);
        // 4x2 at s=1 centred in 8x6: left 2, top 2.
        assert_eq!(out.px(1, 2), [0x2D, 0x2B, 0x55, 255]);
        assert_eq!(out.px(2, 2), [255, 0, 0, 255]);
        assert_eq!(out.px(5, 3), [255, 0, 0, 255]);
        assert_eq!(out.px(6, 3), [0x2D, 0x2B, 0x55, 255]);
        assert_eq!(out.px(2, 4), [0x2D, 0x2B, 0x55, 255]);
        // Fit shrinks: 8x4 into 4x4 -> s=0.5 -> 4x2 at top 1.
        assert_eq!(placement(8, 4, 4, 4, &view(FacetZoom::Fit)), (0, 1, 4, 2, 0.5));
    }

    #[test]
    fn kat_zoom_200_is_pixel_blocks() {
        let img = Raster::new(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]);
        let out = render(&img, &view(FacetZoom::Percent(200)), 4, 2, FIELD);
        let reds: Vec<u8> = out.rgba.chunks_exact(4).map(|p| p[0]).collect();
        assert_eq!(reds, [255, 255, 0, 0, 255, 255, 0, 0]);
    }

    #[test]
    fn kat_pan_clamps_only_on_overflowing_axis() {
        // 10x2 picture at 100 % in a 4x4 viewport: x overflows (range -6..=0), y fits (centred 1).
        let v = FacetView { zoom: FacetZoom::Actual, pan_x: 100, pan_y: 100, ..FacetView::default() };
        assert_eq!(placement(10, 2, 4, 4, &v), (0, 1, 10, 2, 1.0));
        let v = FacetView { pan_x: -100, ..v };
        assert_eq!(placement(10, 2, 4, 4, &v).0, -6);
        let v = FacetView { pan_x: 0, ..v };
        assert_eq!(placement(10, 2, 4, 4, &v).0, -3);
    }

    #[test]
    fn kat_view_turn_and_flip_order() {
        // 2x1 [A B]; flip_h -> [B A]; then a quarter turn -> column B over A.
        let img = Raster::new(2, 1, vec![1, 0, 0, 255, 2, 0, 0, 255]);
        let v = FacetView { zoom: FacetZoom::Actual, quarter_turns: 1, flip_h: true, ..FacetView::default() };
        let t = transformed(&img, &v);
        assert_eq!((t.width, t.height, t.px(0, 0)[0], t.px(0, 1)[0]), (1, 2, 2, 1));
    }

    #[test]
    fn kat_alpha_composites_over_field() {
        let img = Raster::new(1, 1, vec![255, 255, 255, 128]);
        let out = render(&img, &view(FacetZoom::Actual), 1, 1, [0, 0, 0]);
        assert_eq!(out.px(0, 0), [128, 128, 128, 255]);
        // The Chromium-measured case: blue-quadrant (40,70,220) at alpha 128 over Moonstone.
        let img = Raster::new(1, 1, vec![40, 70, 220, 128]);
        assert_eq!(render(&img, &view(FacetZoom::Actual), 1, 1, FIELD).px(0, 0), [42, 56, 152, 255]);
    }
}
