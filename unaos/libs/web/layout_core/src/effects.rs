//! AETHERSTYLE3 (SR103): `filter` (Filter Effects 1 §5, the filter functions of §13) and `backdrop-filter`
//! (Filter Effects 2 §3) on HTML boxes, through SVGCORE's hook [`svg_core::filter::apply_css_filter`] — the same
//! primitives an SVG `<filter>` runs (blur, the colour matrices of brightness / contrast / grayscale / sepia /
//! saturate / hue-rotate / invert / opacity, drop-shadow), in sRGB with an unbounded region.
//!
//! The painter of each ring draws a filtered box as a GROUP (the opacity group's shape): the subtree is drawn,
//! the pixels it changed become a straight-alpha RGBA layer with [`filter_margin`] px of transparent margin, the
//! layer goes through [`filter_rgba`], and the result composites source-over onto the backdrop. A backdrop
//! filter runs [`filter_rgba`] on the backdrop under the box's border box before the box paints. A `url()`
//! reference has no SVG document to resolve in and is dropped (svg_core's rule); a value svg_core does not
//! parse leaves the declaration invalid (it is ledgered, not stored).

use crate::prelude::*;
use svg_core::color::Color;
use svg_core::filter::{apply_css_filter, parse_filter, CssFilterContext, FilterFn};
use svg_core::raster::Pixmap;

/// What a filter's lengths and `currentColor` resolve against.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FilterCx {
    /// Layer pixels per CSS px (device pixel ratio x zoom).
    pub scale: f32,
    /// The element's computed font-size (em in `blur()` / `drop-shadow()`).
    pub font_size: f32,
    /// The viewport in CSS px (vw / vh).
    pub viewport: (f32, f32),
    /// The element's `color`.
    pub color: (u8, u8, u8),
}

impl Default for FilterCx {
    fn default() -> Self {
        FilterCx { scale: 1.0, font_size: 16.0, viewport: (800.0, 600.0), color: (0, 0, 0) }
    }
}

fn ops(value: &str, cx: &FilterCx) -> Option<Vec<FilterFn>> {
    let c = Color { r: cx.color.0, g: cx.color.1, b: cx.color.2, a: 1.0 };
    parse_filter(value, cx.font_size as f64, cx.viewport.0 as f64, cx.viewport.1 as f64, c)
}

/// The declaration parses as a `<filter-value-list>` or `none` (Filter Effects 1 §5): the cascade stores it.
pub fn filter_valid(value: &str) -> bool {
    ops(value, &FilterCx::default()).is_some()
}

/// How far (in layer px) the list can move content beyond its source: 3σ of every blur, a drop shadow's
/// offset plus 3σ (Filter Effects 1 §13: the Gaussian's visible extent).
pub fn filter_margin(value: &str, cx: &FilterCx) -> f32 {
    let Some(list) = ops(value, cx) else { return 0.0 };
    let mut m = 0.0f64;
    for op in &list {
        match op {
            FilterFn::Blur(sd) => m += 3.0 * sd.abs(),
            FilterFn::DropShadow { dx, dy, sd, .. } => m += 3.0 * sd.abs() + dx.abs().max(dy.abs()),
            _ => {}
        }
    }
    ((m * cx.scale as f64) as f32).ceil()
}

/// Runs `value` over a straight-alpha RGBA layer of `w` x `h` in place. `false` = nothing to do (`none`, an
/// invalid list, or only `url()` entries): the layer is untouched.
pub fn filter_rgba(layer: &mut [u8], w: usize, h: usize, value: &str, cx: &FilterCx) -> bool {
    if w == 0 || h == 0 || layer.len() < w * h * 4 {
        return false;
    }
    // premultiply into svg_core's canvas
    let mut pm = Pixmap::new(w, h);
    for (d, s) in pm.data.chunks_exact_mut(4).zip(layer.chunks_exact(4)) {
        let a = s[3] as u32;
        d[0] = ((s[0] as u32 * a + 127) / 255) as u8;
        d[1] = ((s[1] as u32 * a + 127) / 255) as u8;
        d[2] = ((s[2] as u32 * a + 127) / 255) as u8;
        d[3] = s[3];
    }
    let c = Color { r: cx.color.0, g: cx.color.1, b: cx.color.2, a: 1.0 };
    let fcx = CssFilterContext {
        scale: cx.scale as f64,
        font_size: cx.font_size as f64,
        viewport: (cx.viewport.0 as f64, cx.viewport.1 as f64),
        current_color: c,
    };
    let Some(out) = apply_css_filter(&pm, value, &fcx) else { return false };
    if out.w != w || out.h != h {
        return false;
    }
    for (d, s) in layer.chunks_exact_mut(4).zip(out.data.chunks_exact(4)) {
        let a = s[3] as u32;
        if a == 0 {
            d.copy_from_slice(&[0, 0, 0, 0]);
            continue;
        }
        for k in 0..3 {
            d[k] = ((s[k] as u32 * 255 + a / 2) / a).min(255) as u8;
        }
        d[3] = s[3];
    }
    true
}

/// Source-over of a straight-alpha RGBA layer pixel onto an opaque (r, g, b) backdrop.
#[inline]
pub fn over(dst: (u8, u8, u8), src: [u8; 4]) -> (u8, u8, u8) {
    let a = src[3] as u32;
    let mix = |d: u8, s: u8| ((s as u32 * a + d as u32 * (255 - a) + 127) / 255) as u8;
    (mix(dst.0, src[0]), mix(dst.1, src[1]), mix(dst.2, src[2]))
}
