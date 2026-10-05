//! Filter Effects Module Level 1: the `filter` property, the `<filter>` element, its region, the primitive
//! subregions, the filter graph (`in` / `in2` / `result`), `color-interpolation-filters`, and the CSS filter
//! functions (§13), which are mapped onto the same primitives ([`crate::fe`] holds the pixel operations).
//!
//! **Working space.** A filter runs on one device-aligned raster. When the element's user→device transform is
//! a scale + translation, that raster *is* device space; otherwise the transform is decomposed into a scale
//! (applied while filtering) and a remainder (rotation/skew, applied when the result is drawn) — the
//! decomposition Skia makes for image filters, so blurs and morphology stay axis-aligned. The raster covers
//! the filter region (rounded out), limited to the canvas plus one canvas of margin on every side, so
//! offsets and blurs can still pull content in from outside the visible canvas.
//!
//! **What Chromium does that the spec leaves open** (each measured against the oracle):
//! * an unresolvable `url()` is skipped (alone: the element renders unfiltered); a filter with no primitives,
//!   or a region with zero/negative size, renders nothing;
//! * an `in` naming an unknown result means "the previous result" (SourceGraphic for the first primitive);
//! * `feImage` referencing an element renders it translated to the subregion origin (or, for an element with
//!   relative lengths, with the viewport mapped onto the subregion);
//! * CSS filter functions run in sRGB with an unbounded region; `blur()` needs a unit, `drop-shadow()` takes
//!   unitless lengths (the presentation attribute's quirk); an invalid function voids the whole list.

use crate::color::{self, ParsedColor};
use crate::fe::{self, BlendMode, CompositeOp, EdgeMode, IRect, Light, Lighting, TransferFn};
use crate::fmath::{ceil, floor, sin_cos, sqrt};
use crate::geom::{Path, Rect, Transform, number_list, parse_par, view_box_transform};
use crate::paint::Shader;
use crate::raster::{FillRule, Pixmap, fill_coverage};
use crate::render::{Ctx, Renderer};
use crate::style::{self, Axis, Unit, parse_length};
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;

/// What filtering an element produced.
pub enum Outcome {
    /// No valid filter: render the element as if `filter` were `none`.
    Unfiltered,
    /// The element renders nothing.
    Nothing,
    /// The filtered element as a canvas-sized premultiplied layer (clip-path, mask and opacity still apply).
    Layer(Pixmap),
}

/// One entry of the `filter` property's value.
#[derive(Clone, Debug, PartialEq)]
pub enum FilterFn {
    Url(String),
    /// Standard deviation in user units.
    Blur(f64),
    DropShadow { color: [u8; 3], alpha: f32, dx: f64, dy: f64, sd: f64 },
    Matrix([f32; 20]),
    /// R, G, B, A transfer functions.
    Transfer([TransferFn; 4]),
}

// ---------------------------------------------------------------- the `filter` property (Filter Effects 1 §12–13)

/// Parse a `filter` value: `none` → empty; any invalid entry voids the whole value (`None`).
/// `font_size` and the viewport resolve the lengths of `blur()` / `drop-shadow()`; `current` is `currentColor`.
pub fn parse_filter(v: &str, font_size: f64, vw: f64, vh: f64, current: color::Color) -> Option<Vec<FilterFn>> {
    let v = v.trim();
    if v == "none" {
        return Some(Vec::new());
    }
    let mut out = Vec::new();
    let b = v.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let s = i;
        while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'-') {
            i += 1;
        }
        let name = v[s..i].to_ascii_lowercase();
        if i >= b.len() || b[i] != b'(' || name.is_empty() {
            return None;
        }
        // Find the matching parenthesis (one nesting level for colour functions inside drop-shadow).
        let mut depth = 0;
        let a0 = i + 1;
        let mut end = None;
        while i < b.len() {
            match b[i] {
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        end = Some(i);
                        break;
                    }
                }
                _ => {}
            }
            i += 1;
        }
        let e = end?;
        let args = v[a0..e].trim();
        i = e + 1;
        out.push(parse_fn(&name, args, font_size, vw, vh, current)?);
    }
    if out.is_empty() { None } else { Some(out) }
}

/// `<number> | <percentage>`, non-negative; empty → `def`.
fn amount(args: &str, def: f64) -> Option<f64> {
    if args.is_empty() {
        return Some(def);
    }
    let v = if let Some(p) = args.strip_suffix('%') { p.trim().parse::<f64>().ok()? / 100.0 } else { args.parse::<f64>().ok()? };
    if !(v >= 0.0) || !v.is_finite() {
        return None;
    }
    Some(v)
}

/// A CSS length (no percentages); `unitless` admits plain numbers (the SVG attribute quirk).
fn css_len(s: &str, unitless: bool, font_size: f64, vw: f64, vh: f64) -> Option<f64> {
    let l = parse_length(s)?;
    match l.unit {
        Unit::Percent => None,
        Unit::None if !unitless && l.v != 0.0 => None,
        _ => Some(style::resolve(l, Axis::X, vw, vh, font_size)),
    }
}

fn parse_fn(name: &str, args: &str, font_size: f64, vw: f64, vh: f64, current: color::Color) -> Option<FilterFn> {
    let table = |a: f64, b: f64| TransferFn::Table(vec![a, b]);
    Some(match name {
        "url" => FilterFn::Url(style::parse_func_iri(&alloc::format!("url({args})"))?),
        "blur" => {
            let sd = if args.is_empty() { 0.0 } else { css_len(args, false, font_size, vw, vh)? };
            if sd < 0.0 {
                return None;
            }
            FilterFn::Blur(sd)
        }
        "brightness" => {
            let a = amount(args, 1.0)?;
            let f = TransferFn::Linear { slope: a, intercept: 0.0 };
            FilterFn::Transfer([f.clone(), f.clone(), f, TransferFn::Identity])
        }
        "contrast" => {
            let a = amount(args, 1.0)?;
            let f = TransferFn::Linear { slope: a, intercept: -0.5 * a + 0.5 };
            FilterFn::Transfer([f.clone(), f.clone(), f, TransferFn::Identity])
        }
        "invert" => {
            let a = amount(args, 1.0)?.min(1.0);
            let f = table(a, 1.0 - a);
            FilterFn::Transfer([f.clone(), f.clone(), f, TransferFn::Identity])
        }
        "opacity" => {
            let a = amount(args, 1.0)?.min(1.0);
            FilterFn::Transfer([TransferFn::Identity, TransferFn::Identity, TransferFn::Identity, table(0.0, a)])
        }
        "grayscale" => {
            let a = 1.0 - amount(args, 1.0)?.min(1.0) as f32;
            FilterFn::Matrix([
                0.2126 + 0.7874 * a, 0.7152 - 0.7152 * a, 0.0722 - 0.0722 * a, 0., 0.,
                0.2126 - 0.2126 * a, 0.7152 + 0.2848 * a, 0.0722 - 0.0722 * a, 0., 0.,
                0.2126 - 0.2126 * a, 0.7152 - 0.7152 * a, 0.0722 + 0.9278 * a, 0., 0.,
                0., 0., 0., 1., 0.,
            ])
        }
        "sepia" => {
            let a = 1.0 - amount(args, 1.0)?.min(1.0) as f32;
            FilterFn::Matrix([
                0.393 + 0.607 * a, 0.769 - 0.769 * a, 0.189 - 0.189 * a, 0., 0.,
                0.349 - 0.349 * a, 0.686 + 0.314 * a, 0.168 - 0.168 * a, 0., 0.,
                0.272 - 0.272 * a, 0.534 - 0.534 * a, 0.131 + 0.869 * a, 0., 0.,
                0., 0., 0., 1., 0.,
            ])
        }
        "saturate" => FilterFn::Matrix(fe::saturate_matrix(amount(args, 1.0)? as f32)),
        "hue-rotate" => {
            let deg = if args.is_empty() { 0.0 } else { angle(args)? };
            FilterFn::Matrix(fe::hue_rotate_matrix(deg))
        }
        "drop-shadow" => {
            // [<color>] <length>{2,3} [<color>], space separated.
            let toks = tokens(args);
            let mut col: Option<color::Color> = None;
            let mut lens = Vec::new();
            for (k, t) in toks.iter().enumerate() {
                if let Some(l) = css_len(t, true, font_size, vw, vh) {
                    if col.is_some() && k != 0 && lens.is_empty() {
                        // colour first, then lengths: fine
                    }
                    lens.push(l);
                } else {
                    let c = match color::parse(t)? {
                        ParsedColor::Color(c) => c,
                        ParsedColor::CurrentColor => current,
                    };
                    // The colour may come only first or last.
                    if col.is_some() || !(k == 0 || k == toks.len() - 1) {
                        return None;
                    }
                    col = Some(c);
                }
            }
            if !(lens.len() == 2 || lens.len() == 3) {
                return None;
            }
            let sd = lens.get(2).copied().unwrap_or(0.0);
            if sd < 0.0 {
                return None;
            }
            let c = col.unwrap_or(current);
            FilterFn::DropShadow { color: [c.r, c.g, c.b], alpha: c.a, dx: lens[0], dy: lens[1], sd }
        }
        _ => return None,
    })
}

/// Whitespace-separated tokens, keeping parenthesised groups (colour functions) whole.
fn tokens(s: &str) -> Vec<&str> {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let st = i;
        let mut depth = 0;
        while i < b.len() && (depth > 0 || !b[i].is_ascii_whitespace()) {
            if b[i] == b'(' {
                depth += 1;
            } else if b[i] == b')' {
                depth -= 1;
            }
            i += 1;
        }
        out.push(&s[st..i]);
    }
    out
}

/// A CSS `<angle>` in degrees; a unitless value is valid only when zero.
fn angle(s: &str) -> Option<f64> {
    let s = s.trim();
    let split = s.find(|c: char| c.is_ascii_alphabetic()).unwrap_or(s.len());
    let v: f64 = s[..split].parse().ok()?;
    Some(match &s[split..] {
        "deg" => v,
        "grad" => v * 0.9,
        "rad" => v * 180.0 / core::f64::consts::PI,
        "turn" => v * 360.0,
        "" if v == 0.0 => 0.0,
        _ => return None,
    })
}

// ---------------------------------------------------------------- rendering an element through its filter

fn round_out(r: &Rect) -> (i64, i64, i64, i64) {
    const EPS: f64 = 1e-3;
    (floor(r.x + EPS) as i64, floor(r.y + EPS) as i64, ceil(r.right() - EPS) as i64, ceil(r.bottom() - EPS) as i64)
}

/// Render `node`'s content (`f`) through the filter `value`. `pctx`: the parent's context (for the bbox);
/// `ectx`: the element's own context (its user→device transform and style).
pub fn render_filtered<'a, F>(r: &mut Renderer<'a>, node: usize, pctx: &Ctx, ectx: &Ctx, value: &str, f: F) -> Outcome
where
    F: FnOnce(&mut Renderer<'a>, &Ctx, &mut Pixmap),
{
    let bbox = r.bbox(node, pctx, &Transform::IDENTITY);
    render_filtered_bbox(r, node, bbox, ectx, value, f)
}

/// [`render_filtered`] with the element's object bounding box given (e.g. a `<tspan>`'s glyphs).
pub fn render_filtered_bbox<'a, F>(r: &mut Renderer<'a>, node: usize, bbox: Option<Rect>, ectx: &Ctx, value: &str, f: F) -> Outcome
where
    F: FnOnce(&mut Renderer<'a>, &Ctx, &mut Pixmap),
{
    if r.filtering.contains(&node) {
        return Outcome::Unfiltered;
    }
    let st = &ectx.style;
    let Some(ops) = parse_filter(value, st.font_size, ectx.vw, ectx.vh, st.color) else { return Outcome::Unfiltered };
    // Unresolvable references are skipped.
    let ops: Vec<FilterFn> = ops
        .into_iter()
        .filter(|o| match o {
            FilterFn::Url(id) => r.doc.by_id(id).map(|f| r.doc.nodes[f].is_svg("filter")).unwrap_or(false),
            _ => true,
        })
        .collect();
    if ops.is_empty() {
        return Outcome::Unfiltered;
    }
    let t = ectx.ts;
    let (cw, ch) = (r.w as f64, r.h as f64);
    // Working space: device space for scale+translate, else the scale part (remainder applied when drawing).
    let axis = t.b == 0.0 && t.c == 0.0;
    let (ft, post) = if axis {
        (t, Transform::IDENTITY)
    } else {
        let sx = crate::fmath::hypot(t.a, t.b);
        let sy = crate::fmath::hypot(t.c, t.d);
        if sx == 0.0 || sy == 0.0 {
            return Outcome::Nothing;
        }
        let s = Transform::scale(sx, sy);
        (s, t.mul(&Transform::scale(1.0 / sx, 1.0 / sy)))
    };
    let m = cw.max(ch);
    let limit_dev = Rect::new(-m, -m, cw + 2.0 * m, ch + 2.0 * m);
    let limit = match post.invert() {
        Some(inv) => limit_dev.transform(&inv),
        None => return Outcome::Nothing,
    };
    // The raster: the canvas plus a margin (content outside a filter region still feeds its primitives).
    let wr = round_out(&limit);
    if let [FilterFn::Url(id)] = ops.as_slice() {
        if filter_region(r, r.doc.by_id(id).unwrap(), bbox, ectx).is_none() {
            return Outcome::Nothing;
        }
    }
    if wr.2 <= wr.0 || wr.3 <= wr.1 {
        return Outcome::Nothing;
    }
    let (ww, wh) = ((wr.2 - wr.0) as usize, (wr.3 - wr.1) as usize);
    let wft = Transform::translate(-wr.0 as f64, -wr.1 as f64).mul(&ft);
    // SourceGraphic.
    let mut src = Pixmap::new(ww, wh);
    {
        let mut sub = r.sub(ww, wh);
        let cctx = Ctx { ts: wft, ..ectx.clone() };
        f(&mut sub, &cctx, &mut src);
    }
    r.filtering.push(node);
    let mut cur = src;
    for op in &ops {
        let next = match op {
            FilterFn::Url(id) => {
                let fnode = r.doc.by_id(id).unwrap();
                run_filter(r, fnode, bbox, ectx, wft, ww, wh, cur)
            }
            _ => Some(run_function(op, wft, cur)),
        };
        match next {
            Some(n) => cur = n,
            None => {
                r.filtering.pop();
                return Outcome::Nothing;
            }
        }
    }
    r.filtering.pop();
    // Into device space.
    let mut layer = Pixmap::new(r.w, r.h);
    if axis {
        for y in 0..wh {
            let dy = y as i64 + wr.1;
            if dy < 0 || dy >= r.h as i64 {
                continue;
            }
            for x in 0..ww {
                let dx = x as i64 + wr.0;
                if dx < 0 || dx >= r.w as i64 {
                    continue;
                }
                let si = (y * ww + x) * 4;
                let di = (dy as usize * r.w + dx as usize) * 4;
                layer.data[di..di + 4].copy_from_slice(&cur.data[si..si + 4]);
            }
        }
    } else {
        let place = post.mul(&Transform::translate(wr.0 as f64, wr.1 as f64));
        let Some(inv) = place.invert() else { return Outcome::Nothing };
        let mut q = Path::new();
        q.move_to(0.0, 0.0);
        q.line_to(ww as f64, 0.0);
        q.line_to(ww as f64, wh as f64);
        q.line_to(0.0, wh as f64);
        q.close();
        let sh = Shader::Image { inv, pix: cur, smooth: true, repeat: false, cubic: None };
        if let Some(c) = fill_coverage(&q.transform(&place), FillRule::NonZero, r.w, r.h, true) {
            r.paint_coverage(&mut layer, &c, &sh, 1.0);
        }
    }
    Outcome::Layer(layer)
}

/// How a CSS `filter` value resolves on a layer that is not part of an SVG document (Aether's boxes).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CssFilterContext {
    /// Layer pixels per CSS px (device pixel ratio × zoom).
    pub scale: f64,
    /// The element's computed `font-size` (for `em` in `blur()` / `drop-shadow()`).
    pub font_size: f64,
    /// The viewport in CSS px (for `vw` / `vh`).
    pub viewport: (f64, f64),
    /// The element's `color` (`currentColor`, and drop-shadow's default colour).
    pub current_color: color::Color,
}

/// The CSS `filter` property on an arbitrary premultiplied layer: the library API Aether calls for HTML
/// boxes (Filter Effects 1 §13 — the same primitives an SVG filter runs, in sRGB, with an unbounded region).
/// The caller renders the box into `layer` with enough transparent margin for blurs and shadows to spread.
/// `url()` entries are skipped (there is no SVG document to resolve them in). `None` when the value is
/// invalid (the declaration is ignored) or `none`.
pub fn apply_css_filter(layer: &Pixmap, value: &str, cx: &CssFilterContext) -> Option<Pixmap> {
    let ops = parse_filter(value, cx.font_size, cx.viewport.0, cx.viewport.1, cx.current_color)?;
    let ops: Vec<FilterFn> = ops.into_iter().filter(|o| !matches!(o, FilterFn::Url(_))).collect();
    if ops.is_empty() {
        return None;
    }
    let t = Transform::scale(cx.scale, cx.scale);
    let mut cur = layer.clone();
    for op in &ops {
        cur = run_function(op, t, cur);
    }
    Some(cur)
}

/// A CSS filter function on the whole working raster, in sRGB.
fn run_function(op: &FilterFn, wft: Transform, src: Pixmap) -> Pixmap {
    let b = IRect { x0: 0, y0: 0, x1: src.w, y1: src.h };
    let (kx, ky) = (wft.a.abs(), wft.d.abs());
    match op {
        FilterFn::Blur(sd) => {
            let mut o = src;
            fe::blur(&mut o, &b, fe::box_size(sd * kx), fe::box_size(sd * ky));
            o
        }
        FilterFn::DropShadow { color, alpha, dx, dy, sd } => drop_shadow(&src, &b, &b, *sd * kx, *sd * ky, dx * wft.a, dy * wft.d, fe::color_in_space(*color, *alpha, false)),
        FilterFn::Matrix(m) => fe::color_matrix(&src, &b, m),
        FilterFn::Transfer(t) => fe::component_transfer(&src, &b, &[t[0].table(), t[1].table(), t[2].table(), t[3].table()]),
        FilterFn::Url(_) => src,
    }
}

#[allow(clippy::too_many_arguments)]
fn drop_shadow(src: &Pixmap, in_b: &IRect, b: &IRect, sx: f64, sy: f64, dx: f64, dy: f64, c: [f32; 4]) -> Pixmap {
    let mut a = fe::alpha_only(src);
    let all = IRect { x0: 0, y0: 0, x1: src.w, y1: src.h };
    fe::clip_to(&mut a, in_b);
    fe::blur(&mut a, &all, fe::box_size(sx), fe::box_size(sy));
    let a = fe::offset(&a, &all, dx, dy);
    let mut out = fe::colorize(&a, c);
    fe::over_into(&mut out, src, &all);
    fe::clip_to(&mut out, b);
    out
}

/// The `<filter>` element's attribute sources. Chromium does not follow `href` on `<filter>` (Filter Effects 1
/// dropped it): neither the region attributes nor the primitives inherit, so the chain is the element alone.
fn filter_chain(_r: &Renderer, f: usize) -> Vec<usize> {
    vec![f]
}

fn chain_attr<'d>(r: &'d Renderer, chain: &[usize], a: &str) -> Option<&'d str> {
    chain.iter().find_map(|&n| r.doc.nodes[n].attr(a))
}

/// A coordinate in objectBoundingBox units: a number or a percentage, as a fraction.
fn obb_frac(s: &str) -> Option<f64> {
    let l = parse_length(s)?;
    Some(if l.unit == Unit::Percent { l.v / 100.0 } else { l.v })
}

/// The filter region in the element's user space; `None` when the filter renders nothing.
fn filter_region(r: &Renderer, f: usize, bbox: Option<Rect>, ectx: &Ctx) -> Option<Rect> {
    let chain = filter_chain(r, f);
    let obb = chain_attr(r, &chain, "filterUnits").map(|v| v.trim() != "userSpaceOnUse").unwrap_or(true);
    let st = r.doc_style(f);
    let get_v = |a: &str, def: &str, axis: Axis| -> Option<f64> {
        let v = chain_attr(r, &chain, a).filter(|v| parse_length(v).is_some()).unwrap_or(def);
        if obb {
            obb_frac(v)
        } else {
            Some(r.len(parse_length(v)?, axis, ectx, &st))
        }
    };
    let (x, y, w, h) = (get_v("x", "-10%", Axis::X)?, get_v("y", "-10%", Axis::Y)?, get_v("width", "120%", Axis::X)?, get_v("height", "120%", Axis::Y)?);
    let reg = if obb {
        let b = bbox.filter(|b| b.w > 0.0 && b.h > 0.0)?;
        Rect::new(b.x + x * b.w, b.y + y * b.h, w * b.w, h * b.h)
    } else {
        Rect::new(x, y, w, h)
    };
    if !(reg.w > 0.0 && reg.h > 0.0) {
        return None;
    }
    Some(reg)
}

/// A number list as Chromium keeps it after a syntax error: the numbers before the error.
fn number_list_partial(s: &str) -> Vec<f64> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let Some(v) = crate::geom::number(b, &mut i) else { break };
        out.push(v);
        // Garbage after a number ends the list (the number itself is kept).
        if i < b.len() && !(b[i].is_ascii_whitespace() || b[i] == b',') {
            break;
        }
        crate::geom::comma_wsp(b, &mut i);
    }
    out
}

/// The subregion of a primitive without a crop.
const UNBOUNDED: Rect = Rect { x: -1e9, y: -1e9, w: 2e9, h: 2e9 };

/// A primitive's result.
struct Res {
    img: Pixmap,
    linear: bool,
    /// The subregion, working pixels (float) and rounded out within the filter region.
    sub: Rect,
    px: IRect,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum In {
    Source,
    SourceAlpha,
    Res(usize),
}

struct Run<'r, 'a> {
    r: &'r mut Renderer<'a>,
    w: usize,
    h: usize,
    /// user → working pixels (a scale + translation).
    ft: Transform,
    bbox: Option<Rect>,
    /// primitiveUnits = objectBoundingBox.
    obb: bool,
    region: Rect,
    region_px: IRect,
    vctx: Ctx,
    source: Pixmap,
    results: Vec<Res>,
    names: Vec<(String, usize)>,
}

fn irect(r: &Rect, clip: &IRect) -> IRect {
    let (x0, y0, x1, y1) = round_out(r);
    let c = |v: i64, lo: usize, hi: usize| v.clamp(lo as i64, hi as i64) as usize;
    let o = IRect { x0: c(x0, clip.x0, clip.x1), y0: c(y0, clip.y0, clip.y1), x1: c(x1, clip.x0, clip.x1), y1: c(y1, clip.y0, clip.y1) };
    if o.is_empty() { IRect { x0: 0, y0: 0, x1: 0, y1: 0 } } else { o }
}

fn num_attr(r: &Renderer, n: usize, a: &str, def: f64) -> f64 {
    r.doc.nodes[n].attr(a).and_then(|v| v.trim().parse::<f64>().ok()).filter(|v| v.is_finite()).unwrap_or(def)
}

/// "a [b]" → (a, b or a); `None` if malformed or more than two numbers.
fn num_pair(r: &Renderer, n: usize, a: &str) -> Option<Option<(f64, f64)>> {
    let Some(v) = r.doc.nodes[n].attr(a) else { return Some(None) };
    let l = number_list(v)?;
    match l.len() {
        1 => Some(Some((l[0], l[0]))),
        2 => Some(Some((l[0], l[1]))),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn run_filter(r: &mut Renderer, f: usize, bbox: Option<Rect>, ectx: &Ctx, ft: Transform, w: usize, h: usize, source: Pixmap) -> Option<Pixmap> {
    let region_user = filter_region(r, f, bbox, ectx)?;
    let chain = filter_chain(r, f);
    let obb = chain_attr(r, &chain, "primitiveUnits").map(|v| v.trim() == "objectBoundingBox").unwrap_or(false);
    // The primitives: this filter's own children.
    let prims: Vec<usize> = r.doc.elements(f).filter(|&c| {
        let nd = &r.doc.nodes[c];
        nd.is_svg_element() && nd.name().starts_with("fe")
    }).collect();
    let region = region_user.transform(&ft);
    let all = IRect { x0: 0, y0: 0, x1: w, y1: h };
    let region_px = irect(&region, &all);
    if obb && bbox.map(|b| b.w <= 0.0 || b.h <= 0.0).unwrap_or(true) {
        return None;
    }
    // SourceGraphic is not clipped to the region (a blur near the region's edge still sees the content
    // outside it); only the filter's result is.
    let mut run = Run { r, w, h, ft, bbox, obb, region, region_px, vctx: ectx.clone(), source, results: Vec::new(), names: Vec::new() };
    for &p in &prims {
        if !run.primitive(p) {
            // An unknown element name (fe*) is a filter error: the element renders nothing.
            return None;
        }
    }
    let last = run.results.pop()?;
    let mut img = last.img;
    if last.linear {
        fe::convert(&mut img, false);
    }
    // Every result is already cropped to its subregion within the region (or, uncropped, unbounded).
    Some(img)
}

impl Run<'_, '_> {
    fn input(&self, a: Option<&str>) -> In {
        let prev = if self.results.is_empty() { In::Source } else { In::Res(self.results.len() - 1) };
        match a.map(|s| s.trim()) {
            Some("SourceGraphic") => In::Source,
            Some("SourceAlpha") => In::SourceAlpha,
            Some(name) if !name.is_empty() => match self.names.iter().rev().find(|(n, _)| n == name) {
                Some(&(_, i)) => In::Res(i),
                None => prev,
            },
            _ => prev,
        }
    }

    fn input_of(&self, n: usize, a: &str) -> In {
        self.input(self.r.doc.nodes[n].attr(a))
    }

    /// The input image converted to the operating colour space, its pixel bounds and subregion.
    fn image(&self, i: In, linear: bool) -> (Pixmap, IRect, Rect) {
        let (mut img, lin, px, sub) = match i {
            In::Source => (self.source.clone(), false, self.region_px, self.region),
            In::SourceAlpha => (fe::alpha_only(&self.source), false, self.region_px, self.region),
            In::Res(k) => {
                let r = &self.results[k];
                (r.img.clone(), r.linear, r.px, r.sub)
            }
        };
        if lin != linear {
            fe::convert(&mut img, linear);
        }
        (img, px, sub)
    }

    fn sub_of(&self, i: In) -> Rect {
        match i {
            In::Res(k) => self.results[k].sub,
            _ => self.region,
        }
    }

    /// x / y / width / height of a primitive → its subregion (working pixels).
    fn subregion(&self, n: usize, inputs: &[In], tile: bool) -> Rect {
        let mut def = if inputs.is_empty() || tile || inputs.iter().any(|i| !matches!(i, In::Res(_))) {
            self.region
        } else {
            let mut u = self.sub_of(inputs[0]);
            for &i in &inputs[1..] {
                u = u.union(&self.sub_of(i));
            }
            u
        };
        let Some(inv) = self.ft.invert() else { return def };
        let du = def.transform(&inv);
        let nd = &self.r.doc.nodes[n];
        let st = self.r.doc_style(n);
        let val = |a: &str, axis: Axis| -> Option<f64> {
            let v = nd.attr(a)?;
            let l = parse_length(v)?;
            if self.obb {
                let b = self.bbox?;
                let f = if l.unit == Unit::Percent { l.v / 100.0 } else { l.v };
                Some(match (a, axis) {
                    ("x", _) => b.x + f * b.w,
                    ("y", _) => b.y + f * b.h,
                    (_, Axis::X) => f * b.w,
                    _ => f * b.h,
                })
            } else {
                Some(self.r.len(l, axis, &self.vctx, &st))
            }
        };
        let x = val("x", Axis::X).unwrap_or(du.x);
        let y = val("y", Axis::Y).unwrap_or(du.y);
        let ww = val("width", Axis::X).unwrap_or(du.w);
        let hh = val("height", Axis::Y).unwrap_or(du.h);
        if !(ww > 0.0 && hh > 0.0) {
            // An empty subregion means no crop at all (Chromium): the primitive covers the whole raster,
            // beyond the filter region too.
            return UNBOUNDED;
        }
        def = Rect::new(x, y, ww, hh).transform(&self.ft);
        def
    }

    /// The pixel bounds a primitive writes: its subregion within the filter region.
    fn px_of(&self, sub: &Rect) -> IRect {
        if sub.w >= UNBOUNDED.w {
            IRect { x0: 0, y0: 0, x1: self.w, y1: self.h }
        } else {
            irect(sub, &self.region_px)
        }
    }

    /// A number in primitiveUnits → working pixels along x (signed) / y.
    fn ux(&self, v: f64) -> f64 {
        v * if self.obb { self.bbox.map(|b| b.w).unwrap_or(0.0) } else { 1.0 } * self.ft.a
    }
    fn uy(&self, v: f64) -> f64 {
        v * if self.obb { self.bbox.map(|b| b.h).unwrap_or(0.0) } else { 1.0 } * self.ft.d
    }

    fn push(&mut self, n: usize, img: Pixmap, linear: bool, sub: Rect, px: IRect) {
        let mut img = img;
        fe::clip_to(&mut img, &px);
        self.results.push(Res { img, linear, sub, px });
        if let Some(name) = self.r.doc.nodes[n].attr("result").map(|s| s.trim()).filter(|s| !s.is_empty()) {
            self.names.push((name.to_string(), self.results.len() - 1));
        }
    }

    /// Evaluate one primitive; `false` for an unknown primitive element.
    fn primitive(&mut self, n: usize) -> bool {
        let name = self.r.doc.nodes[n].name().to_string();
        let st = self.r.doc_style(n);
        let linear = st.filters_linear;
        let all = IRect { x0: 0, y0: 0, x1: self.w, y1: self.h };
        let one = |s: &Self| [s.input_of(n, "in")];
        match name.as_str() {
            "feGaussianBlur" => {
                let ins = one(self);
                let sub = self.subregion(n, &ins, false);
                let px = self.px_of(&sub);
                let (mut img, _, _) = self.image(ins[0], linear);
                let sd = num_pair(self.r, n, "stdDeviation").unwrap_or(Some((0.0, 0.0))).unwrap_or((0.0, 0.0));
                if sd.0 < 0.0 || sd.1 < 0.0 || (sd.0 == 0.0 && sd.1 == 0.0) {
                    // Disabled: the result is the input.
                } else {
                    let (sx, sy) = (self.ux(sd.0).abs(), self.uy(sd.1).abs());
                    let (dx, dy) = (fe::box_size(sx), fe::box_size(sy));
                    // Blur only what can reach the subregion.
                    let reach = |b: &IRect| IRect { x0: b.x0.saturating_sub(3 * dx), y0: b.y0.saturating_sub(3 * dy), x1: b.x1 + 3 * dx, y1: b.y1 + 3 * dy }.intersect(&all);
                    fe::blur(&mut img, &reach(&px), dx, dy);
                }
                self.push(n, img, linear, sub, px);
            }
            "feOffset" => {
                let ins = one(self);
                let sub = self.subregion(n, &ins, false);
                let px = self.px_of(&sub);
                let (img, _, _) = self.image(ins[0], linear);
                let dx = self.ux(num_attr(self.r, n, "dx", 0.0));
                let dy = self.uy(num_attr(self.r, n, "dy", 0.0));
                let o = fe::offset(&img, &px, dx, dy);
                self.push(n, o, linear, sub, px);
            }
            "feFlood" => {
                let sub = self.subregion(n, &[], false);
                let px = self.px_of(&sub);
                let mut img = Pixmap::new(self.w, self.h);
                let c = st.flood_color;
                fe::flood(&mut img, &px, fe::color_in_space([c.r, c.g, c.b], c.a * st.flood_opacity as f32, linear));
                self.push(n, img, linear, sub, px);
            }
            "feMerge" => {
                let nodes: Vec<usize> = self.r.doc.elements(n).filter(|&k| self.r.doc.nodes[k].is_svg("feMergeNode")).collect();
                let ins: Vec<In> = nodes.iter().map(|&k| self.input_of(k, "in")).collect();
                let sub = self.subregion(n, &ins, false);
                let px = self.px_of(&sub);
                let mut acc = Pixmap::new(self.w, self.h);
                for &i in &ins {
                    let (img, _, _) = self.image(i, linear);
                    fe::over_into(&mut acc, &img, &px);
                }
                self.push(n, acc, linear, sub, px);
            }
            "feComposite" | "feBlend" => {
                let ins = [self.input_of(n, "in"), self.input_of(n, "in2")];
                let sub = self.subregion(n, &ins, false);
                let px = self.px_of(&sub);
                let (s, _, _) = self.image(ins[0], linear);
                let (d, _, _) = self.image(ins[1], linear);
                let nd = &self.r.doc.nodes[n];
                let o = if name == "feBlend" {
                    let m = nd.attr("mode").and_then(BlendMode::parse).unwrap_or(BlendMode::Normal);
                    fe::blend(&s, &d, &px, m)
                } else {
                    let op = match nd.attr("operator").map(|s| s.trim()) {
                        Some("in") => CompositeOp::In,
                        Some("out") => CompositeOp::Out,
                        Some("atop") => CompositeOp::Atop,
                        Some("xor") => CompositeOp::Xor,
                        Some("lighter") => CompositeOp::Lighter,
                        Some("arithmetic") => {
                            let k = |a: &str| num_attr(self.r, n, a, 0.0) as f32;
                            CompositeOp::Arithmetic([k("k1"), k("k2"), k("k3"), k("k4")])
                        }
                        _ => CompositeOp::Over,
                    };
                    fe::composite(&s, &d, &px, op)
                };
                self.push(n, o, linear, sub, px);
            }
            "feColorMatrix" => {
                let ins = one(self);
                let sub = self.subregion(n, &ins, false);
                let px = self.px_of(&sub);
                let (img, _, _) = self.image(ins[0], linear);
                let nd = &self.r.doc.nodes[n];
                let vals = nd.attr("values").and_then(number_list);
                let m = match nd.attr("type").map(|s| s.trim()).unwrap_or("matrix") {
                    "saturate" => match vals.as_deref() {
                        Some([s]) => fe::saturate_matrix(*s as f32),
                        Some([]) | None => fe::IDENTITY_MATRIX,
                        _ => fe::IDENTITY_MATRIX,
                    },
                    "hueRotate" => match vals.as_deref() {
                        Some([d]) => fe::hue_rotate_matrix(*d),
                        _ => fe::IDENTITY_MATRIX,
                    },
                    "luminanceToAlpha" => fe::LUMINANCE_TO_ALPHA,
                    // `matrix`, and an invalid type (the attribute's initial value applies).
                    _ => match vals.as_deref() {
                        Some(v) if v.len() == 20 => {
                            let mut m = [0f32; 20];
                            for (d, s) in m.iter_mut().zip(v) {
                                *d = *s as f32;
                            }
                            m
                        }
                        _ => fe::IDENTITY_MATRIX,
                    },
                };
                let o = fe::color_matrix(&img, &px, &m);
                self.push(n, o, linear, sub, px);
            }
            "feComponentTransfer" => {
                let ins = one(self);
                let sub = self.subregion(n, &ins, false);
                let px = self.px_of(&sub);
                let (img, _, _) = self.image(ins[0], linear);
                let mut fns = [TransferFn::Identity, TransferFn::Identity, TransferFn::Identity, TransferFn::Identity];
                let kids: Vec<usize> = self.r.doc.elements(n).collect();
                for k in kids {
                    let kn = &self.r.doc.nodes[k];
                    let slot = match kn.name() {
                        "feFuncR" => 0,
                        "feFuncG" => 1,
                        "feFuncB" => 2,
                        "feFuncA" => 3,
                        _ => continue,
                    };
                    if !kn.is_svg_element() {
                        continue;
                    }
                    let list = |a: &str| kn.attr(a).map(number_list_partial).unwrap_or_default();
                    fns[slot] = match kn.attr("type").map(|s| s.trim()) {
                        Some("table") => TransferFn::Table(list("tableValues")),
                        Some("discrete") => TransferFn::Discrete(list("tableValues")),
                        Some("linear") => TransferFn::Linear { slope: num_attr(self.r, k, "slope", 1.0), intercept: num_attr(self.r, k, "intercept", 0.0) },
                        Some("gamma") => TransferFn::Gamma {
                            amplitude: num_attr(self.r, k, "amplitude", 1.0),
                            exponent: num_attr(self.r, k, "exponent", 1.0),
                            offset: num_attr(self.r, k, "offset", 0.0),
                        },
                        _ => TransferFn::Identity,
                    };
                }
                let t = [fns[0].table(), fns[1].table(), fns[2].table(), fns[3].table()];
                let o = fe::component_transfer(&img, &px, &t);
                self.push(n, o, linear, sub, px);
            }
            "feMorphology" => {
                let ins = one(self);
                let sub = self.subregion(n, &ins, false);
                let px = self.px_of(&sub);
                let (img, ib, _) = self.image(ins[0], linear);
                let dilate = self.r.doc.nodes[n].attr("operator").map(|s| s.trim() == "dilate").unwrap_or(false);
                let rad = num_pair(self.r, n, "radius").unwrap_or(Some((0.0, 0.0))).unwrap_or((0.0, 0.0));
                let o = if rad.0 < 0.0 || rad.1 < 0.0 || (rad.0 == 0.0 && rad.1 == 0.0) {
                    img
                } else {
                    let rx = crate::fmath::round(self.ux(rad.0).abs()) as usize;
                    let ry = crate::fmath::round(self.uy(rad.1).abs()) as usize;
                    fe::morphology(&img, &ib, &px, rx.min(self.w), ry.min(self.h), dilate)
                };
                self.push(n, o, linear, sub, px);
            }
            "feTile" => {
                let ins = one(self);
                let sub = self.subregion(n, &ins, true);
                let px = self.px_of(&sub);
                let (img, ib, _) = self.image(ins[0], linear);
                let o = fe::tile(&img, &ib, &px);
                self.push(n, o, linear, sub, px);
            }
            "feDropShadow" => {
                let ins = one(self);
                let sub = self.subregion(n, &ins, false);
                let px = self.px_of(&sub);
                let (img, ib, _) = self.image(ins[0], linear);
                let sd = num_pair(self.r, n, "stdDeviation").unwrap_or(Some((2.0, 2.0))).unwrap_or((2.0, 2.0));
                let (sx, sy) = if sd.0 < 0.0 || sd.1 < 0.0 { (0.0, 0.0) } else { (self.ux(sd.0).abs(), self.uy(sd.1).abs()) };
                let dx = self.ux(num_attr(self.r, n, "dx", 2.0));
                let dy = self.uy(num_attr(self.r, n, "dy", 2.0));
                let c = st.flood_color;
                let col = fe::color_in_space([c.r, c.g, c.b], c.a * st.flood_opacity as f32, linear);
                let o = drop_shadow(&img, &ib, &px, sx, sy, dx, dy, col);
                self.push(n, o, linear, sub, px);
            }
            "feTurbulence" => {
                let sub = self.subregion(n, &[], false);
                let px = self.px_of(&sub);
                let mut img = Pixmap::new(self.w, self.h);
                let nd = &self.r.doc.nodes[n];
                let bf = match nd.attr("baseFrequency").map(number_list) {
                    None => Some((0.0, 0.0)),
                    Some(Some(l)) if l.len() == 1 => Some((l[0], l[0])),
                    Some(Some(l)) if l.len() == 2 => Some((l[0], l[1])),
                    _ => None,
                };
                let fractal = nd.attr("type").map(|s| s.trim() == "fractalNoise").unwrap_or(false);
                let stitch = nd.attr("stitchTiles").map(|s| s.trim() == "stitch").unwrap_or(false);
                let octaves = nd.attr("numOctaves").and_then(|v| v.trim().parse::<i64>().ok()).unwrap_or(1).max(0) as u32;
                let seed = num_attr(self.r, n, "seed", 0.0);
                if let Some((fx, fy)) = bf.filter(|f| f.0 >= 0.0 && f.1 >= 0.0) {
                    let inv = self.ft.invert().unwrap_or(Transform::IDENTITY);
                    // The noise is sampled at the pixel centre in user space, plus half a user unit (measured:
                    // Chromium's lattice sits half a unit off at every scale).
                    let map = |x: usize, y: usize| -> (f64, f64) {
                        let (u, v) = inv.apply(x as f64 + 0.5, y as f64 + 0.5);
                        (u + 0.5, v + 0.5)
                    };
                    let tile = if stitch {
                        let su = sub.transform(&inv);
                        Some((su.x, su.y, su.w, su.h))
                    } else {
                        None
                    };
                    // baseFrequency is not scaled by primitiveUnits (Chromium); the seed is truncated (spec).
                    fe::turbulence(&mut img, &px, &map, seed as i64, fx, fy, octaves.min(32), fractal, tile);
                }
                self.push(n, img, linear, sub, px);
            }
            "feDisplacementMap" => {
                let ins = [self.input_of(n, "in"), self.input_of(n, "in2")];
                let sub = self.subregion(n, &ins, false);
                let px = self.px_of(&sub);
                let (s, sb, _) = self.image(ins[0], linear);
                let (m, _, _) = self.image(ins[1], linear);
                let sel = |a: &str| match self.r.doc.nodes[n].attr(a).map(|s| s.trim()) {
                    Some("R") => 0,
                    Some("G") => 1,
                    Some("B") => 2,
                    _ => 3,
                };
                let scale = num_attr(self.r, n, "scale", 0.0);
                let o = fe::displacement(&s, &m, &px, &sb, self.ux(scale), self.uy(scale), sel("xChannelSelector"), sel("yChannelSelector"));
                self.push(n, o, linear, sub, px);
            }
            "feConvolveMatrix" => {
                let ins = one(self);
                let sub = self.subregion(n, &ins, false);
                let px = self.px_of(&sub);
                let (img, ib, _) = self.image(ins[0], linear);
                let o = self.convolve(n, &img, &ib, &px).unwrap_or_else(|| Pixmap::new(self.w, self.h));
                self.push(n, o, linear, sub, px);
            }
            "feDiffuseLighting" | "feSpecularLighting" => {
                let ins = one(self);
                let sub = self.subregion(n, &ins, false);
                let px = self.px_of(&sub);
                let (img, _, _) = self.image(ins[0], linear);
                let o = self.lighting(n, &name, &img, &px, &st, linear).unwrap_or_else(|| Pixmap::new(self.w, self.h));
                self.push(n, o, linear, sub, px);
            }
            "feImage" => {
                let sub = self.subregion(n, &[], false);
                let px = self.px_of(&sub);
                let img = self.fe_image(n, &sub).unwrap_or_else(|| Pixmap::new(self.w, self.h));
                // The image is sRGB.
                let mut img = img;
                if linear {
                    fe::convert(&mut img, true);
                }
                self.push(n, img, linear, sub, px);
            }
            _ => return false,
        }
        true
    }

    fn convolve(&self, n: usize, img: &Pixmap, ib: &IRect, px: &IRect) -> Option<Pixmap> {
        let nd = &self.r.doc.nodes[n];
        let order = match nd.attr("order") {
            None => (3usize, 3usize),
            Some(v) => {
                let toks: Vec<&str> = v.split(|c: char| c.is_ascii_whitespace() || c == ',').filter(|s| !s.is_empty()).collect();
                let p = |s: &str| s.parse::<i64>().ok().filter(|&o| o > 0).map(|o| o as usize);
                match toks.as_slice() {
                    [a] => {
                        let a = p(a)?;
                        (a, a)
                    }
                    [a, b] => (p(a)?, p(b)?),
                    _ => return None,
                }
            }
        };
        let kernel = nd.attr("kernelMatrix").and_then(number_list)?;
        if kernel.len() != order.0 * order.1 {
            return None;
        }
        let divisor = match nd.attr("divisor") {
            // divisor="0" is ignored (Chromium), like an absent one.
            Some(v) if v.trim().parse::<f64>().ok().filter(|d| *d != 0.0).is_some() => v.trim().parse::<f64>().unwrap(),
            _ => {
                // The kernel sum, or 1 when it is zero (up to float rounding: 8 × 0.1 − 0.8 is not 0.0).
                let s: f32 = kernel.iter().map(|&k| k as f32).sum();
                if s == 0.0 { 1.0 } else { s as f64 }
            }
        };
        let tgt = |a: &str, ord: usize| -> Option<usize> {
            match nd.attr(a) {
                None => Some(ord / 2),
                Some(v) => v.trim().parse::<i64>().ok().filter(|&t| t >= 0 && (t as usize) < ord).map(|t| t as usize),
            }
        };
        let target = (tgt("targetX", order.0)?, tgt("targetY", order.1)?);
        let edge = match nd.attr("edgeMode").map(|s| s.trim()) {
            Some("wrap") => EdgeMode::Wrap,
            Some("none") => EdgeMode::None,
            _ => EdgeMode::Duplicate,
        };
        let preserve_alpha = nd.attr("preserveAlpha").map(|s| s.trim() == "true").unwrap_or(false);
        let bias = num_attr(self.r, n, "bias", 0.0);
        let c = fe::Convolve { order, kernel: &kernel, divisor, bias, target, edge, preserve_alpha };
        Some(fe::convolve(img, ib, px, &c))
    }

    fn lighting(&self, n: usize, name: &str, img: &Pixmap, px: &IRect, st: &style::Style, linear: bool) -> Option<Pixmap> {
        let r = &*self.r;
        let light_node = r.doc.elements(n).find(|&k| matches!(r.doc.nodes[k].name(), "feDistantLight" | "fePointLight" | "feSpotLight"))?;
        let ln = &r.doc.nodes[light_node];
        let na = |a: &str, d: f64| num_attr(r, light_node, a, d);
        // Positions: user space → working pixels; z scales by the mean of the axis scales.
        let (bw, bh) = self.bbox.map(|b| (b.w, b.h)).unwrap_or((1.0, 1.0));
        let pos = |x: f64, y: f64, z: f64| -> [f64; 3] {
            let (x, y, z) = if self.obb {
                let b = self.bbox.unwrap_or(Rect::new(0.0, 0.0, 1.0, 1.0));
                (b.x + x * b.w, b.y + y * b.h, z * sqrt((bw * bw + bh * bh) / 2.0))
            } else {
                (x, y, z)
            };
            let (px_, py_) = self.ft.apply(x, y);
            [px_, py_, z * (self.ft.a.abs() + self.ft.d.abs()) / 2.0]
        };
        let light = match ln.name() {
            "feDistantLight" => {
                let az = na("azimuth", 0.0) * core::f64::consts::PI / 180.0;
                let el = na("elevation", 0.0) * core::f64::consts::PI / 180.0;
                let (sa, ca) = sin_cos(az);
                let (se, ce) = sin_cos(el);
                Light::Distant { dir: [ca * ce, sa * ce, se] }
            }
            "fePointLight" => Light::Point { pos: pos(na("x", 0.0), na("y", 0.0), na("z", 0.0)) },
            _ => {
                let p = pos(na("x", 0.0), na("y", 0.0), na("z", 0.0));
                let t = pos(na("pointsAtX", 0.0), na("pointsAtY", 0.0), na("pointsAtZ", 0.0));
                let d = [t[0] - p[0], t[1] - p[1], t[2] - p[2]];
                let l = sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]);
                let s = if l > 0.0 { [d[0] / l, d[1] / l, d[2] / l] } else { [0.0, 0.0, 0.0] };
                let exponent = na("specularExponent", 1.0).clamp(1.0, 128.0);
                let cone = ln.attr("limitingConeAngle").and_then(|v| v.trim().parse::<f64>().ok()).map(|a| a.abs().min(90.0)).filter(|a| *a != 0.0).unwrap_or(90.0);
                let cos_outer = crate::fmath::cos(cone * core::f64::consts::PI / 180.0);
                Light::Spot { pos: p, s, exponent, cos_outer }
            }
        };
        // The surface height is a z length: it maps into the working space like the lights' z (Skia).
        let surface = num_attr(r, n, "surfaceScale", 1.0) * (self.ft.a.abs() + self.ft.d.abs()) / 2.0;
        let kind = if name == "feDiffuseLighting" {
            Lighting::Diffuse { kd: num_attr(r, n, "diffuseConstant", 1.0) }
        } else {
            let ks = num_attr(r, n, "specularConstant", 1.0);
            // Chromium clamps the exponent into the spec's range 1..=128.
            let ex = num_attr(r, n, "specularExponent", 1.0).clamp(1.0, 128.0);
            Lighting::Specular { ks, exponent: ex }
        };
        let c = fe::color_in_space([st.lighting_color.r, st.lighting_color.g, st.lighting_color.b], 1.0, linear);
        let color = [c[0] as f64 * 255.0, c[1] as f64 * 255.0, c[2] as f64 * 255.0];
        Some(fe::lighting(img, px, &light, kind, surface, color))
    }

    /// feImage: an element (`#id`) or a `data:` image, placed in the subregion `sub` (working pixels).
    fn fe_image(&mut self, n: usize, sub: &Rect) -> Option<Pixmap> {
        let inv = self.ft.invert()?;
        let dst = sub.transform(&inv);
        let href = self.r.doc.nodes[n].href()?.trim().to_string();
        let mut out = Pixmap::new(self.w, self.h);
        if let Some(id) = href.strip_prefix('#') {
            let target = self.r.doc.by_id(id)?;
            let tn = &self.r.doc.nodes[target];
            if !tn.is_svg_element() || self.r.is_active(target) || self.r.depth > 24 {
                return None;
            }
            // Chromium: translate to the subregion origin, or map the viewport onto it for relative lengths.
            let rel = ["x", "y", "width", "height", "cx", "cy", "r", "rx", "ry", "x1", "y1", "x2", "y2"].iter().any(|a| tn.attr(a).map(|v| v.contains('%')).unwrap_or(false));
            let t = if rel && self.vctx.vw > 0.0 && self.vctx.vh > 0.0 {
                Transform::new(dst.w / self.vctx.vw, 0.0, 0.0, dst.h / self.vctx.vh, dst.x, dst.y)
            } else {
                Transform::translate(dst.x, dst.y)
            };
            let parent = tn.parent.unwrap_or(target);
            let style = self.r.doc_style(parent);
            let ctx = Ctx { ts: self.ft.mul(&t), vw: self.vctx.vw, vh: self.vctx.vh, style, ctx_fill: None, ctx_stroke: None, ctx_elem: None };
            let mut sub_r = self.r.sub(self.w, self.h);
            sub_r.render(target, &ctx, &mut out);
            return Some(out);
        }
        // A data: image (an external file is never fetched).
        let (mime, data) = crate::image::parse_data_url(&href)?;
        let par = self.r.doc.nodes[n].attr("preserveAspectRatio").map(parse_par).unwrap_or_default();
        if mime.contains("svg") {
            let svg = crate::Svg::parse(&data).ok()?;
            let (iw, ih) = svg.size_f();
            let vb = svg.view_box.unwrap_or(Rect::new(0.0, 0.0, iw, ih));
            let t = self.ft.mul(&view_box_transform(&Rect::new(0.0, 0.0, iw, ih), &par, dst.x, dst.y, dst.w, dst.h));
            let t = t.mul(&view_box_transform(&vb, &svg.par, 0.0, 0.0, iw, ih));
            let mut rr = Renderer::new(&svg.doc, &svg.props, self.r.opts, self.w, self.h);
            svg.render_into(&mut rr, t, vb, &mut out);
            return Some(out);
        }
        let dec = self.r.opts.image_decoder?;
        let img = dec(&data)?;
        if img.width == 0 || img.height == 0 || img.rgba.len() < img.width * img.height * 4 {
            return None;
        }
        let (iw, ih) = (img.width as f64, img.height as f64);
        let it = self.ft.mul(&view_box_transform(&Rect::new(0.0, 0.0, iw, ih), &par, dst.x, dst.y, dst.w, dst.h));
        let inv = it.invert()?;
        let mut pix = Pixmap::new(img.width, img.height);
        for (d, s) in pix.data.chunks_exact_mut(4).zip(img.rgba.chunks_exact(4)) {
            let a = s[3] as u32;
            for c in 0..3 {
                d[c] = crate::raster::div255(s[c] as u32 * a) as u8;
            }
            d[3] = s[3];
        }
        let mut q = Path::new();
        q.move_to(0.0, 0.0);
        q.line_to(iw, 0.0);
        q.line_to(iw, ih);
        q.line_to(0.0, ih);
        q.close();
        let sh = Shader::Image { inv, pix, smooth: true, repeat: false, cubic: None };
        if let Some(c) = fill_coverage(&q.transform(&it), FillRule::NonZero, self.w, self.h, true) {
            self.r.paint_coverage(&mut out, &c, &sh, 1.0);
        }
        Some(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filter_function_lists() {
        let black = color::Color::BLACK;
        let p = |s: &str| parse_filter(s, 16.0, 100.0, 100.0, black);
        assert_eq!(p("none"), Some(Vec::new()));
        assert_eq!(p("url(#a)"), Some(vec![FilterFn::Url("a".into())]));
        assert_eq!(p("blur(2px)"), Some(vec![FilterFn::Blur(2.0)]));
        assert_eq!(p("blur()"), Some(vec![FilterFn::Blur(0.0)]));
        // blur() needs a unit; negative and percentage values are invalid; so is one bad entry in a list.
        assert_eq!(p("blur(4)"), None);
        assert_eq!(p("blur(-5px)"), None);
        assert_eq!(p("blur(50%)"), None);
        assert_eq!(p("grayscale() hue-rotate(random) opacity(0.5)"), None);
        assert_eq!(p("hue-rotate(45)"), None);
        assert!(matches!(p("hue-rotate(0.25turn)").as_deref(), Some([FilterFn::Matrix(_)])));
        // drop-shadow: colour first or last, two or three lengths, unitless allowed.
        assert_eq!(p("drop-shadow(blue 4 5 6)"), Some(vec![FilterFn::DropShadow { color: [0, 0, 255], alpha: 1.0, dx: 4.0, dy: 5.0, sd: 6.0 }]));
        assert_eq!(p("drop-shadow(4 5 6 blue)"), p("drop-shadow(blue 4 5 6)"));
        assert_eq!(p("drop-shadow(10 15)"), Some(vec![FilterFn::DropShadow { color: [0, 0, 0], alpha: 1.0, dx: 10.0, dy: 15.0, sd: 0.0 }]));
        assert_eq!(p("drop-shadow(4)"), None);
        assert_eq!(p("drop-shadow(blue 4 5 6 7)"), None);
        assert_eq!(p("drop-shadow(red, 10, 15)"), None);
        assert_eq!(p("drop-shadow(blue 3% 4% 5%)"), None);
        assert_eq!(p("drop-shadow()"), None);
        assert_eq!(angle("45grad"), Some(40.5));
    }

    #[test]
    fn css_filter_api_on_a_layer() {
        let cx = CssFilterContext { scale: 2.0, font_size: 16.0, viewport: (100.0, 100.0), current_color: color::Color::BLACK };
        let mut layer = Pixmap::new(4, 1);
        layer.data.copy_from_slice(&[255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 0, 0, 0, 0]);
        // grayscale(1): luminance 0.2126 / 0.7152 / 0.0722 of red, green, blue.
        let g = apply_css_filter(&layer, "grayscale(1)", &cx).unwrap();
        assert_eq!(&g.data[0..4], &[54, 54, 54, 255]);
        assert_eq!(&g.data[4..8], &[182, 182, 182, 255]);
        assert_eq!(&g.data[8..12], &[18, 18, 18, 255]);
        assert_eq!(&g.data[12..16], &[0, 0, 0, 0]);
        // invert(1) then opacity(50%): cyan at half alpha (the table truncates 127.5); transparent stays.
        let i = apply_css_filter(&layer, "invert(1) opacity(50%)", &cx).unwrap();
        assert_eq!(&i.data[0..4], &[0, 127, 127, 127]);
        assert_eq!(&i.data[12..16], &[0, 0, 0, 0]);
        // Invalid or url-only values are ignored.
        assert!(apply_css_filter(&layer, "blur(4)", &cx).is_none());
        assert!(apply_css_filter(&layer, "url(#x)", &cx).is_none());
        // drop-shadow(2px 0 currentColor) at scale 2: shifted 4 layer pixels, under the source.
        let mut dot = Pixmap::new(8, 1);
        dot.data[3] = 255;
        let d = apply_css_filter(&dot, "drop-shadow(2px 0)", &cx).unwrap();
        assert_eq!(&d.data[16..20], &[0, 0, 0, 255]);
        assert_eq!(&d.data[0..4], &[0, 0, 0, 255]);
        assert_eq!(d.data[4 * 2 + 3], 0);
    }
}
