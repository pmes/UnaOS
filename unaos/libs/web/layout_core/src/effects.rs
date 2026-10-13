//! AETHERSTYLE3 (SR103): `filter` (Filter Effects 1 §5, the filter functions of §13) and `backdrop-filter`
//! (Filter Effects 2 §3) on HTML boxes, through SVGCORE's hook [`svg_core::filter::apply_css_filter`] — the same
//! primitives an SVG `<filter>` runs (blur, the colour matrices of brightness / contrast / grayscale / sepia /
//! saturate / hue-rotate / invert / opacity, drop-shadow), in sRGB with an unbounded region.
//!
//! The painter of each ring draws a filtered box as a GROUP (the opacity group's shape): the subtree is drawn,
//! the pixels it changed become a straight-alpha RGBA layer with [`filter_margin`] px of transparent margin, the
//! layer goes through [`filter_rgba`], and the result composites source-over onto the backdrop. A backdrop
//! filter runs [`filter_rgba`] on the backdrop under the box's border box before the box paints. A value
//! svg_core does not parse leaves the declaration invalid (it is ledgered, not stored).
//!
//! AETHERSTYLE3B (SR115): a `url(#id)` entry references an SVG `<filter>` element of the page (inline SVG,
//! Filter Effects 1 §5). [`filter_rgba_refs`] runs the list in order: the filter functions between references
//! through [`filter_rgba`], each reference through svg_core's own `<filter>` machinery
//! (`svg_core::filter::render_filtered_bbox`) — the element is serialized ([`filter_markup`], over
//! [`crate::DomNode::attrs`]) into a one-filter SVG document whose source graphic is the layer and whose bounding
//! box is the element's border box, so `filterUnits`, `primitiveUnits`, the region and the whole primitive graph
//! are svg_core's. A reference that does not resolve to a `<filter>` is skipped (svg_core's rule for SVG
//! content: alone, the box renders unfiltered); a reference to another document is skipped and ledgered.

use crate::prelude::*;
use crate::DomNode;
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

// ---- AETHERSTYLE3B (SR115): `url()` references to SVG `<filter>` elements ----------------------------------------

/// One top-level entry of a filter list: (function name lowercased, its argument text).
fn entries(value: &str) -> Vec<(String, &str)> {
    let mut out = Vec::new();
    let b = value.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let s = i;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-') {
            i += 1;
        }
        if i >= b.len() || b[i] != b'(' {
            break;
        }
        let name = value[s..i].to_ascii_lowercase();
        let a0 = i + 1;
        let mut depth = 0i32;
        while i < b.len() {
            match b[i] {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        let a1 = i.min(b.len());
        out.push((name, &value[a0..a1]));
        i = a1 + 1;
    }
    out
}

/// The fragment id a `url()` entry's argument names (`#id`, quoted or not); None for another document.
fn url_id(args: &str) -> Option<String> {
    let a = args.trim().trim_matches(|c| c == '"' || c == '\'');
    a.strip_prefix('#').filter(|id| !id.is_empty()).map(String::from)
}

/// The fragment ids the list's `url()` entries reference, in order.
pub fn filter_urls(value: &str) -> Vec<String> {
    entries(value).into_iter().filter(|(n, _)| n == "url").filter_map(|(_, a)| url_id(a)).collect()
}

/// The element of `root`'s tree whose `id` is `id` (the first in tree order).
pub fn find_by_id<N: DomNode>(root: &N, id: &str) -> Option<N> {
    let mut top = root.clone();
    while let Some(p) = top.parent() {
        top = p;
    }
    top.descendants().into_iter().find(|n| n.is_element() && n.attr("id").as_deref() == Some(id))
}

fn escape(s: &str, out: &mut String) {
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            c => out.push(c),
        }
    }
}

/// An element's subtree as XML markup for svg_core: elements with their attributes ([`DomNode::attrs`]) and
/// text; comments and the rest dropped.
pub fn filter_markup<N: DomNode>(el: &N) -> String {
    fn walk<N: DomNode>(n: &N, out: &mut String, depth: usize) {
        if depth > 64 {
            return;
        }
        if n.is_text() {
            escape(&n.text(), out);
            return;
        }
        if !n.is_element() {
            return;
        }
        let tag = n.tag().unwrap_or_default();
        out.push('<');
        out.push_str(&tag);
        for (k, v) in n.attrs() {
            if k.contains(':') && !k.starts_with("xlink:") || k == "xmlns" {
                continue;
            }
            out.push(' ');
            out.push_str(&k);
            out.push_str("=\"");
            escape(&v, out);
            out.push('"');
        }
        out.push('>');
        for c in n.children() {
            walk(&c, out, depth + 1);
        }
        out.push_str("</");
        out.push_str(&tag);
        out.push('>');
    }
    let mut out = String::new();
    walk(el, &mut out, 0);
    out
}

/// The `<filter>` markup `id` names in `root`'s document (None: no such element, or it is not a `<filter>`).
pub fn resolve_filter<N: DomNode>(root: &N, id: &str) -> Option<String> {
    let f = find_by_id(root, id)?;
    (f.tag().as_deref() == Some("filter")).then(|| filter_markup(&f))
}

/// [`filter_margin`] for a box `bw` x `bh` px whose list may reference `<filter>` elements: a reference's
/// default region reaches 10% past the bounding box on each side (Filter Effects 1 §7.1), plus room for the
/// primitives' blurs.
pub fn filter_margin_box(value: &str, cx: &FilterCx, bw: f32, bh: f32) -> f32 {
    let plain: Vec<String> = entries(value).into_iter().filter(|(n, _)| n != "url").map(|(n, a)| format!("{n}({a})")).collect();
    let mut m = if plain.is_empty() { 0.0 } else { filter_margin(&plain.join(" "), cx) };
    let refs = filter_urls(value).len() as f32;
    if refs > 0.0 {
        m += refs * ((bw.max(bh) * 0.1 + 24.0) * cx.scale).ceil();
    }
    m
}

const SOURCE_ID: &str = "aether-filter-source";

/// One referenced `<filter>` over the layer through svg_core. `Some(true)`: the layer is the result; `Some(false)`:
/// the reference did not resolve (skipped); `None`: the filter renders nothing (the layer is cleared).
fn run_svg_filter(layer: &mut [u8], w: usize, h: usize, markup: &str, id: &str, bbox: (f32, f32, f32, f32)) -> Option<bool> {
    use svg_core::filter::{render_filtered_bbox, Outcome};
    use svg_core::render::{Ctx, Renderer};
    use svg_core::{Rect, Svg, Transform};
    let doc = format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" xmlns:xlink=\"http://www.w3.org/1999/xlink\" width=\"{w}\" height=\"{h}\"><g id=\"{SOURCE_ID}\"/>{markup}</svg>"
    );
    let Ok(svg) = Svg::parse(doc.as_bytes()) else { return Some(false) };
    let Some(src) = svg.doc.by_id(SOURCE_ID) else { return Some(false) };
    let opts = svg_core::Options::default();
    let mut r = Renderer::new(&svg.doc, &svg.props, &opts, w, h);
    let ectx = Ctx { ts: Transform::IDENTITY, vw: w as f64, vh: h as f64, style: opts.root_style(), ctx_fill: None, ctx_stroke: None, ctx_elem: None };
    let bb = Rect::new(bbox.0 as f64, bbox.1 as f64, bbox.2 as f64, bbox.3 as f64);
    let value = format!("url(#{id})");
    let input: &[u8] = layer;
    let out = render_filtered_bbox(&mut r, src, Some(bb), &ectx, &value, |_, cctx, pm| {
        // the working raster is the layer translated by the context's offset (an identity scale here)
        let (ox, oy) = ((cctx.ts.e + 0.5) as i64, (cctx.ts.f + 0.5) as i64);
        for y in 0..h {
            let ty = y as i64 + oy;
            if ty < 0 || ty >= pm.h as i64 {
                continue;
            }
            for x in 0..w {
                let tx = x as i64 + ox;
                if tx < 0 || tx >= pm.w as i64 {
                    continue;
                }
                let s = &input[(y * w + x) * 4..(y * w + x) * 4 + 4];
                let a = s[3] as u32;
                let d = (ty as usize * pm.w + tx as usize) * 4;
                pm.data[d] = ((s[0] as u32 * a + 127) / 255) as u8;
                pm.data[d + 1] = ((s[1] as u32 * a + 127) / 255) as u8;
                pm.data[d + 2] = ((s[2] as u32 * a + 127) / 255) as u8;
                pm.data[d + 3] = s[3];
            }
        }
    });
    match out {
        Outcome::Unfiltered => Some(false),
        Outcome::Nothing => None,
        Outcome::Layer(pm) => {
            if pm.w != w || pm.h != h {
                return Some(false);
            }
            for (d, s) in layer.chunks_exact_mut(4).zip(pm.data.chunks_exact(4)) {
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
            Some(true)
        }
    }
}

/// [`filter_rgba`] for a list that may reference SVG `<filter>` elements: the entries run in order, filter
/// functions through svg_core's CSS hook, each `url(#id)` through the `<filter>` `resolve(id)` serializes (its
/// markup), with `bbox` (x, y, w, h in layer px) as the element's bounding box. `false` = nothing ran.
pub fn filter_rgba_refs(
    layer: &mut [u8],
    w: usize,
    h: usize,
    value: &str,
    cx: &FilterCx,
    bbox: (f32, f32, f32, f32),
    resolve: &dyn Fn(&str) -> Option<String>,
) -> bool {
    if !value.to_ascii_lowercase().contains("url(") {
        return filter_rgba(layer, w, h, value, cx);
    }
    if w == 0 || h == 0 || layer.len() < w * h * 4 {
        return false;
    }
    let mut ran = false;
    let mut pending: Vec<String> = Vec::new();
    let flush = |layer: &mut [u8], pending: &mut Vec<String>, ran: &mut bool| {
        if !pending.is_empty() {
            *ran |= filter_rgba(layer, w, h, &pending.join(" "), cx);
            pending.clear();
        }
    };
    for (name, args) in entries(value) {
        if name != "url" {
            pending.push(format!("{name}({args})"));
            continue;
        }
        flush(layer, &mut pending, &mut ran);
        let Some(id) = url_id(args) else { continue };
        let Some(markup) = resolve(&id) else { continue };
        match run_svg_filter(layer, w, h, &markup, &id, bbox) {
            Some(r) => ran |= r,
            None => {
                layer.iter_mut().for_each(|b| *b = 0);
                ran = true;
            }
        }
    }
    flush(layer, &mut pending, &mut ran);
    ran
}
