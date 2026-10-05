// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: shared-core (media)
//!
//! SVGCORE (LEDGER SR52): UnaOS renders SVG from the specifications. `no_std` + `alloc`, no `unsafe`; the one
//! dependency is UnaOS's own font_core (glyph outlines, shaping, and the exact-area rasterizer paths are
//! filled with).
//!
//! | area | spec | module |
//! |---|---|---|
//! | XML 1.0 + Namespaces + DTD entities | W3C XML 1.0 5th ed., Namespaces in XML 1.0 | [`xml`] |
//! | `style` / `<style>` (the CSSCORE SR47 seam) | CSS 2.1 syntax, Selectors 3 subset, Cascade 4 | [`css`], [`style`] |
//! | colours | CSS Color 3/4 | [`color`] |
//! | path data, arcs, transforms, viewBox | SVG 1.1 §8.3, F.6, §7.6–7.8 | [`geom`] |
//! | fill, stroke, dashes, joins, caps | SVG 2 §13 | [`raster`], [`stroke`] |
//! | gradients, patterns | SVG 2 §14 | [`paint`], [`render`] |
//! | clipPath, mask | CSS Masking 1 | [`render`] |
//! | markers | SVG 2 §11.6 | [`marker`] |
//! | text | SVG 1.1 §10 / SVG 2 §11 | [`text`], [`fonts`] |
//! | `<image>` data: URLs | RFC 2397, RFC 4648 | [`image`] |
//!
//! ```ignore
//! let svg = svg_core::Svg::parse(bytes)?;
//! let (w, h) = svg.size();
//! let rgba = svg.render_rgba(w, h, &svg_core::Options::default());
//! ```

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod color;
pub mod css;
pub mod fmath;
pub mod fonts;
pub mod geom;
pub mod image;
pub mod marker;
pub mod paint;
pub mod raster;
pub mod render;
pub mod stroke;
pub mod style;
pub mod text;
pub mod xml;

use alloc::vec::Vec;
pub use fonts::FontSet;
pub use geom::{Par, Rect, Transform};
pub use raster::Pixmap;

/// A raster image handed back by the caller's decoder (straight RGBA).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedImage {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

/// Decodes the bytes of an `<image>` `data:` URL (PNG, JPEG, GIF, WebP…). pixel_core supplies one.
pub type ImageDecoder = fn(&[u8]) -> Option<DecodedImage>;

/// Rendering options.
#[derive(Clone, Debug, Default)]
pub struct Options<'a> {
    /// Fonts for `<text>`; without any, text is not drawn.
    pub fonts: Option<&'a FontSet>,
    /// Raster decoder for `<image>`; without one, raster images are skipped (SVG images still render).
    pub image_decoder: Option<ImageDecoder>,
    /// User languages for `systemLanguage` (BCP 47), e.g. `["en"]`.
    pub languages: Vec<&'a str>,
}

impl Options<'_> {
    /// The style the root element inherits (a browser's initial values: 16px, black, the default family).
    pub fn root_style(&self) -> style::Style {
        let mut s = style::Style::default();
        if let Some(f) = self.fonts {
            s.font_family = f.default_family.clone();
        }
        s
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Xml(xml::Error),
    /// The root element is not `<svg>` in the SVG namespace.
    NotSvg,
    /// A requested or intrinsic size of zero or beyond the limits.
    BadSize,
}

/// Largest rendered width or height.
pub const MAX_DIM: u32 = 1 << 14;

/// `true` when the bytes look like SVG: optional BOM/whitespace/comments/doctype, then `<svg` or `<?xml`.
pub fn sniff_svg(bytes: &[u8]) -> bool {
    let b = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    let mut i = 0;
    let n = b.len().min(4096);
    while i < n {
        while i < n && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let r = &b[i..];
        if r.starts_with(b"<svg") || r.starts_with(b"<?xml") {
            return r.starts_with(b"<svg") || find(&b[i..n], b"<svg").is_some();
        }
        if r.starts_with(b"<!--") {
            match find(r, b"-->") {
                Some(e) => i += e + 3,
                None => return false,
            }
        } else if r.starts_with(b"<!DOCTYPE") || r.starts_with(b"<!doctype") {
            return find(&b[i..n], b"<svg").is_some();
        } else {
            return false;
        }
    }
    false
}

fn find(h: &[u8], n: &[u8]) -> Option<usize> {
    h.windows(n.len()).position(|w| w == n)
}

/// A parsed SVG document, ready to render at any size.
#[derive(Clone, Debug)]
pub struct Svg {
    pub doc: xml::Document,
    pub props: Vec<style::Props>,
    pub root: usize,
    pub view_box: Option<Rect>,
    pub par: Par,
    width: f64,
    height: f64,
    /// Width and height were given in absolute units (else the viewport is the render size).
    sized: bool,
}

impl Svg {
    pub fn parse(bytes: &[u8]) -> Result<Svg, Error> {
        let doc = xml::Document::parse(bytes).map_err(Error::Xml)?;
        let root = doc.root_element().ok_or(Error::NotSvg)?;
        if !doc.nodes[root].is_svg("svg") {
            return Err(Error::NotSvg);
        }
        let props = style::cascade(&doc);
        let n = &doc.nodes[root];
        let view_box = n.attr("viewBox").and_then(geom::parse_view_box).filter(|v| v.w > 0.0 && v.h > 0.0);
        let par = n.attr("preserveAspectRatio").map(geom::parse_par).unwrap_or_default();
        let st = style::Style::compute(&style::Style::default(), &props[root]);
        let dim = |a: &str| -> Option<f64> {
            let l = style::get(&props[root], a).or(n.attr(a)).and_then(style::parse_length)?;
            if l.unit == style::Unit::Percent {
                return None;
            }
            // Zero or negative sizes are treated as auto (Chromium renders such an image unscaled).
            Some(style::resolve(l, style::Axis::X, 0.0, 0.0, st.font_size)).filter(|&v| v > 0.0)
        };
        let (w, h) = (dim("width"), dim("height"));
        // Intrinsic size (SVG 2 §8.2 / CSS replaced elements): explicit width/height; a missing one from the
        // viewBox aspect ratio; else the viewBox size; else 100 × 100.
        let (width, height) = match (w, h, view_box) {
            (Some(w), Some(h), _) => (w, h),
            (Some(w), None, Some(v)) => (w, w * v.h / v.w),
            (None, Some(h), Some(v)) => (h * v.w / v.h, h),
            (None, None, Some(v)) => (v.w, v.h),
            (Some(w), None, None) => (w, 100.0),
            (None, Some(h), None) => (100.0, h),
            (None, None, None) => (100.0, 100.0),
        };
        let sized = w.is_some() && h.is_some();
        Ok(Svg { doc, props, root, view_box, par, width, height, sized })
    }

    /// Intrinsic size in CSS px (fractional).
    pub fn size_f(&self) -> (f64, f64) {
        (self.width, self.height)
    }

    /// Intrinsic size rounded to whole pixels (at least 1).
    pub fn size(&self) -> (u32, u32) {
        let r = |v: f64| (fmath::round(v).max(1.0) as u32).min(MAX_DIM);
        (r(self.width), r(self.height))
    }

    /// Render onto a new `w × h` premultiplied canvas. The viewBox (or, without one, the intrinsic size) is
    /// mapped onto the canvas with the root's `preserveAspectRatio`.
    pub fn render(&self, w: u32, h: u32, opts: &Options) -> Result<Pixmap, Error> {
        if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM {
            return Err(Error::BadSize);
        }
        let (w, h) = (w as usize, h as usize);
        let mut canvas = Pixmap::new(w, h);
        // Without a viewBox: an absolutely sized SVG scales its intrinsic size onto the canvas; one without a
        // size (width/height auto or %) takes the canvas as its viewport, unscaled (Chromium's SVG-in-<img>).
        let vb = match self.view_box {
            Some(v) => v,
            None if self.sized => Rect::new(0.0, 0.0, self.width, self.height),
            None => Rect::new(0.0, 0.0, w as f64, h as f64),
        };
        let par = if self.view_box.is_some() { self.par } else { Par { align: None, slice: false } };
        let t = geom::view_box_transform(&vb, &par, 0.0, 0.0, w as f64, h as f64);
        let mut r = render::Renderer::new(&self.doc, &self.props, opts, w, h);
        self.render_into(&mut r, t, vb, &mut canvas);
        Ok(canvas)
    }

    /// Render with an explicit user→device transform (for nesting, e.g. `<image href=x.svg>`).
    pub fn render_into(&self, r: &mut render::Renderer, t: Transform, vb: Rect, canvas: &mut Pixmap) {
        let ctx = render::Ctx { ts: t, vw: vb.w, vh: vb.h, style: r.opts.root_style(), ctx_fill: None, ctx_stroke: None, ctx_elem: None };
        r.render_root(self.root, &ctx, canvas);
    }

    /// Render to straight (non-premultiplied) RGBA bytes.
    pub fn render_rgba(&self, w: u32, h: u32, opts: &Options) -> Result<Vec<u8>, Error> {
        Ok(self.render(w, h, opts)?.to_straight())
    }
}
