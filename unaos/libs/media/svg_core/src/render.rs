//! The renderer: walks the document (SVG 2 §3 "Rendering Model"), computing styles as it descends, and paints
//! each graphics element in document order onto a premultiplied canvas. Group effects (`opacity`, `clip-path`,
//! `mask`) render into an offscreen layer that is clipped/masked and composited with source-over (§3.6).
//! `<use>` instantiates its target with the `use` element as the style parent (§5.6); `<symbol>` and nested
//! `<svg>` establish viewports (§8.2) with `overflow` clipping; `<switch>` picks its first passing child.

use crate::fmath::{ceil, sqrt};
use crate::geom::{self, Path, Rect, Transform, parse_par, parse_path, parse_transform, parse_view_box, view_box_transform};
use crate::paint::{Shader, Spread, Stop, premul};
use crate::raster::{Coverage, FillRule, Mask, Pixmap, fill_coverage, fill_polys};
use crate::stroke::{StrokeStyle, normalize_dashes, stroke_polys};
use crate::style::{self, Axis, Length, Paint, Props, Style, get, parse_func_iri, parse_length, resolve};
use crate::xml::{Document, Kind};
use crate::{Options, text};
use alloc::boxed::Box;
use alloc::vec::Vec;

const MAX_DEPTH: usize = 48;

/// Coordinate context: the user→device transform and the nearest viewport's size (for percentages).
#[derive(Clone, Debug)]
pub struct Ctx {
    pub ts: Transform,
    pub vw: f64,
    pub vh: f64,
    pub style: Style,
    /// Context element paints for `context-fill` / `context-stroke` (markers, use).
    pub ctx_fill: Option<Box<Paint>>,
    pub ctx_stroke: Option<Box<Paint>>,
}

pub struct Renderer<'a> {
    pub doc: &'a Document,
    pub props: &'a [Props],
    pub opts: &'a Options<'a>,
    pub w: usize,
    pub h: usize,
    depth: usize,
    /// Elements currently being instantiated (use targets, patterns, clip paths, masks, markers): cycle guard.
    active: Vec<usize>,
    /// Root element font size and line height (for `rem`/`rlh`), and the initial containing block in CSS px
    /// (for viewport units).
    pub root_font: f64,
    pub root_lh: f64,
    pub icb: (f64, f64),
}

pub enum ClipResult {
    Mask(Mask),
    /// The reference is invalid: render as if `clip-path`/`mask` were not specified.
    Ignore,
    /// Nothing is rendered (e.g. an objectBoundingBox clip on an element with an empty bbox).
    Nothing,
}

fn num_attr(n: &crate::xml::Node, a: &str) -> Option<Length> {
    n.attr(a).and_then(parse_length)
}

/// `true` when the conditional processing attributes (SVG 2 §5.8) pass.
fn conditions_pass(n: &crate::xml::Node, langs: &[&str]) -> bool {
    if let Some(e) = n.attr("requiredExtensions") {
        // No extensions are supported; an empty string is false too (SVG 2).
        let _ = e;
        return false;
    }
    if let Some(f) = n.attr("requiredFeatures") {
        // SVG 2 dropped requiredFeatures; browsers treat it as always true except an empty value.
        if f.trim().is_empty() {
            return false;
        }
    }
    if let Some(l) = n.attr("systemLanguage") {
        let ok = l.split(',').map(|s| s.trim()).any(|tag| {
            !tag.is_empty()
                && langs.iter().any(|ul| {
                    let p = tag.split('-').next().unwrap_or(tag);
                    ul.eq_ignore_ascii_case(tag) || ul.split('-').next().unwrap_or(ul).eq_ignore_ascii_case(p)
                })
        });
        if !ok {
            return false;
        }
    }
    true
}

/// CSS transform functions → the SVG transform grammar (units stripped, angles in degrees).
fn parse_css_transform(s: &str) -> Option<Transform> {
    let mut out = alloc::string::String::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_digit() || c == b'.' || ((c == b'-' || c == b'+') && i + 1 < b.len() && (b[i + 1].is_ascii_digit() || b[i + 1] == b'.')) {
            let mut j = i;
            let v = geom::number(b, &mut j)?;
            let mut k = j;
            while k < b.len() && b[k].is_ascii_alphabetic() {
                k += 1;
            }
            let unit = &s[j..k];
            let v = match unit {
                "" | "px" | "deg" => v,
                "rad" => v * 180.0 / core::f64::consts::PI,
                "grad" => v * 0.9,
                "turn" => v * 360.0,
                _ => return None,
            };
            out.push_str(&alloc::format!("{v}"));
            i = k;
        } else {
            out.push(c as char);
            i += 1;
        }
    }
    let out = out.replace("translateX(", "translate(").replace("scaleX(", "scale(");
    parse_transform(&out)
}

impl<'a> Renderer<'a> {
    pub fn new(doc: &'a Document, props: &'a [Props], opts: &'a Options<'a>, w: usize, h: usize) -> Self {
        Renderer { doc, props, opts, w, h, depth: 0, active: Vec::new(), root_font: 16.0, root_lh: 16.0 * 1.2, icb: (w as f64, h as f64) }
    }

    fn sub(&self, w: usize, h: usize) -> Renderer<'a> {
        Renderer { doc: self.doc, props: self.props, opts: self.opts, w, h, depth: self.depth + 1, active: self.active.clone(), root_font: self.root_font, root_lh: self.root_lh, icb: self.icb }
    }

    /// The computed style of `node` as its document ancestors give it (for resources rendered out of place).
    pub fn doc_style(&self, node: usize) -> Style {
        let mut chain = Vec::new();
        let mut n = Some(node);
        while let Some(i) = n {
            if matches!(self.doc.nodes[i].kind, Kind::Element { .. }) {
                chain.push(i);
            }
            n = self.doc.nodes[i].parent;
        }
        let mut s = self.opts.root_style();
        for &i in chain.iter().rev() {
            s = Style::compute(&s, &self.props[i]);
        }
        s
    }

    /// A resource (paint server, clip path, mask) inside a `display: none` subtree, or under an element whose
    /// conditional attributes fail, has no rendering object in a browser: references to it are invalid.
    pub fn resource_hidden(&self, node: usize) -> bool {
        let mut n = Some(node);
        while let Some(i) = n {
            let nd = &self.doc.nodes[i];
            if matches!(nd.kind, Kind::Element { .. }) {
                if get(&self.props[i], "display").map(|v| v.trim() == "none").unwrap_or(false) {
                    return true;
                }
                if !conditions_pass(nd, &self.opts.languages) {
                    return true;
                }
            }
            n = nd.parent;
        }
        false
    }

    pub fn len(&self, l: Length, axis: Axis, ctx: &Ctx, st: &Style) -> f64 {
        use style::Unit;
        match l.unit {
            Unit::Ex | Unit::Ch | Unit::Ic | Unit::Lh => {
                let m = self.font_metrics(st);
                let k = match l.unit {
                    Unit::Ex => m.0,
                    Unit::Ch => m.1,
                    Unit::Ic => m.2,
                    _ => m.3,
                };
                l.v * k * st.font_size
            }
            Unit::Rem => l.v * self.root_font,
            Unit::Rlh => l.v * self.root_lh,
            Unit::Vw => l.v * self.icb.0 / 100.0,
            Unit::Vh => l.v * self.icb.1 / 100.0,
            Unit::Vmin => l.v * self.icb.0.min(self.icb.1) / 100.0,
            Unit::Vmax => l.v * self.icb.0.max(self.icb.1) / 100.0,
            _ => resolve(l, axis, ctx.vw, ctx.vh, st.font_size),
        }
    }

    /// (ex, ch, ic, normal line height) of the style's primary font, in ems (CSS Values 4 §6.1).
    pub fn font_metrics(&self, st: &Style) -> (f64, f64, f64, f64) {
        let fallback = (0.5, 0.5, 1.0, 1.2);
        let Some(fs) = self.opts.fonts else { return fallback };
        let Some(i) = fs.select(&st.font_family, st.font_weight, st.italic) else { return fallback };
        let Some(f) = fs.faces[i].font() else { return fallback };
        let u = f.units_per_em as f64;
        let ex = f.os2.and_then(|o| o.x_height).filter(|&v| v > 0).map(|v| v as f64 / u).unwrap_or(0.5);
        let adv = |c: char| {
            let g = f.glyph_index(c);
            if g == 0 { None } else { Some(f.advance(g) as f64 / u) }
        };
        let (a, d, g) = f.line_metrics();
        (ex, adv('0').unwrap_or(0.5), adv('\u{6C34}').unwrap_or(1.0), (a as f64 - d as f64 + g as f64) / u)
    }

    fn attr_len(&self, node: usize, a: &str, axis: Axis, ctx: &Ctx, st: &Style, def: f64) -> f64 {
        let n = &self.doc.nodes[node];
        let v = get(&self.props[node], a).and_then(parse_length).or_else(|| num_attr(n, a));
        v.map(|l| self.len(l, axis, ctx, st)).unwrap_or(def)
    }

    /// The `transform` presentation attribute, or the CSS `transform` property when a style sets it (CSS
    /// Transforms 1: lengths in px, angles in deg/rad/grad/turn; `transform-origin` defaults to 0 0 in SVG).
    pub fn element_transform(&self, node: usize) -> Transform {
        if let Some(css) = get(&self.props[node], "transform") {
            if css.trim() == "none" {
                return Transform::IDENTITY;
            }
            return parse_css_transform(css).filter(|t| t.is_finite()).unwrap_or(Transform::IDENTITY);
        }
        self.doc.nodes[node].attr("transform").and_then(parse_transform).filter(|t| t.is_finite()).unwrap_or(Transform::IDENTITY)
    }

    // ------------------------------------------------------------------ geometry

    /// The geometry of a basic shape or path in user space (SVG 2 §10), `None` when it does not render.
    pub fn shape_path(&self, node: usize, ctx: &Ctx, st: &Style) -> Option<Path> {
        let n = &self.doc.nodes[node];
        let mut p = Path::new();
        match n.name() {
            "path" => {
                let d = get(&self.props[node], "d").map(|v| v.trim_start_matches("path(").trim_end_matches(')').trim_matches('"').trim_matches('\''));
                p = parse_path(d.or(n.attr("d")).unwrap_or(""));
            }
            "rect" => {
                let x = self.attr_len(node, "x", Axis::X, ctx, st, 0.0);
                let y = self.attr_len(node, "y", Axis::Y, ctx, st, 0.0);
                let w = self.attr_len(node, "width", Axis::X, ctx, st, 0.0);
                let h = self.attr_len(node, "height", Axis::Y, ctx, st, 0.0);
                if !(w > 0.0 && h > 0.0) {
                    return None;
                }
                let rxa = self.rx_ry(node, "rx", Axis::X, ctx, st);
                let rya = self.rx_ry(node, "ry", Axis::Y, ctx, st);
                // SVG 2 §10.2: auto radii take the other one; clamp to half the size.
                let (mut rx, mut ry) = match (rxa, rya) {
                    (None, None) => (0.0, 0.0),
                    (Some(a), None) => (a, a),
                    (None, Some(b)) => (b, b),
                    (Some(a), Some(b)) => (a, b),
                };
                rx = rx.min(w / 2.0);
                ry = ry.min(h / 2.0);
                if rx > 0.0 && ry > 0.0 {
                    let k = 0.552_284_749_830_793_4;
                    let (kx, ky) = (rx * k, ry * k);
                    p.move_to(x + rx, y);
                    p.line_to(x + w - rx, y);
                    p.cubic_to((x + w - rx + kx, y), (x + w, y + ry - ky), (x + w, y + ry));
                    p.line_to(x + w, y + h - ry);
                    p.cubic_to((x + w, y + h - ry + ky), (x + w - rx + kx, y + h), (x + w - rx, y + h));
                    p.line_to(x + rx, y + h);
                    p.cubic_to((x + rx - kx, y + h), (x, y + h - ry + ky), (x, y + h - ry));
                    p.line_to(x, y + ry);
                    p.cubic_to((x, y + ry - ky), (x + rx - kx, y), (x + rx, y));
                    p.close();
                } else {
                    p.move_to(x, y);
                    p.line_to(x + w, y);
                    p.line_to(x + w, y + h);
                    p.line_to(x, y + h);
                    p.close();
                }
            }
            "circle" | "ellipse" => {
                let cx = self.attr_len(node, "cx", Axis::X, ctx, st, 0.0);
                let cy = self.attr_len(node, "cy", Axis::Y, ctx, st, 0.0);
                let (rx, ry) = if n.name() == "circle" {
                    let r = self.attr_len(node, "r", Axis::Diag, ctx, st, 0.0);
                    (r, r)
                } else {
                    let a = self.rx_ry(node, "rx", Axis::X, ctx, st);
                    let b = self.rx_ry(node, "ry", Axis::Y, ctx, st);
                    match (a, b) {
                        (Some(a), Some(b)) => (a, b),
                        (Some(a), None) => (a, a),
                        (None, Some(b)) => (b, b),
                        (None, None) => (0.0, 0.0),
                    }
                };
                if !(rx > 0.0 && ry > 0.0) {
                    return None;
                }
                let k = 0.552_284_749_830_793_4;
                let (kx, ky) = (rx * k, ry * k);
                p.move_to(cx + rx, cy);
                p.cubic_to((cx + rx, cy + ky), (cx + kx, cy + ry), (cx, cy + ry));
                p.cubic_to((cx - kx, cy + ry), (cx - rx, cy + ky), (cx - rx, cy));
                p.cubic_to((cx - rx, cy - ky), (cx - kx, cy - ry), (cx, cy - ry));
                p.cubic_to((cx + kx, cy - ry), (cx + rx, cy - ky), (cx + rx, cy));
                p.close();
            }
            "line" => {
                let x1 = self.attr_len(node, "x1", Axis::X, ctx, st, 0.0);
                let y1 = self.attr_len(node, "y1", Axis::Y, ctx, st, 0.0);
                let x2 = self.attr_len(node, "x2", Axis::X, ctx, st, 0.0);
                let y2 = self.attr_len(node, "y2", Axis::Y, ctx, st, 0.0);
                p.move_to(x1, y1);
                p.line_to(x2, y2);
            }
            "polyline" | "polygon" => {
                // Points: render up to the error; an odd count drops the last coordinate.
                let s = n.attr("points").unwrap_or("").as_bytes();
                let mut i = 0;
                let mut v = Vec::new();
                loop {
                    while i < s.len() && (s[i].is_ascii_whitespace() || s[i] == b',') {
                        i += 1;
                    }
                    if i >= s.len() {
                        break;
                    }
                    match geom::number(s, &mut i) {
                        Some(x) => v.push(x),
                        None => break,
                    }
                }
                if v.len() < 4 {
                    return None;
                }
                p.move_to(v[0], v[1]);
                let mut k = 2;
                while k + 1 < v.len() {
                    p.line_to(v[k], v[k + 1]);
                    k += 2;
                }
                if n.name() == "polygon" {
                    p.close();
                }
            }
            _ => return None,
        }
        if p.segs.len() < 2 {
            return None;
        }
        Some(p)
    }

    fn rx_ry(&self, node: usize, a: &str, axis: Axis, ctx: &Ctx, st: &Style) -> Option<f64> {
        let n = &self.doc.nodes[node];
        let v = get(&self.props[node], a).or(n.attr(a))?;
        if v.trim() == "auto" {
            return None;
        }
        let l = parse_length(v)?;
        let r = self.len(l, axis, ctx, st);
        if r < 0.0 { None } else { Some(r) }
    }

    /// The object bounding box of an element in the coordinate system `t` maps its user space to.
    pub fn bbox(&mut self, node: usize, ctx: &Ctx, t: &Transform) -> Option<Rect> {
        if self.depth > MAX_DEPTH {
            return None;
        }
        let n = &self.doc.nodes[node];
        if !n.is_svg_element() {
            return None;
        }
        let st = Style::compute(&ctx.style, &self.props[node]);
        if st.display_none {
            return None;
        }
        match n.name() {
            "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" => {
                let p = self.shape_path(node, ctx, &st)?;
                p.transform(t).bbox()
            }
            "g" | "a" | "switch" | "svg" => {
                let mut c = ctx.clone();
                c.style = st;
                let mut acc: Option<Rect> = None;
                let kids: Vec<usize> = self.doc.elements(node).collect();
                for k in kids {
                    let kt = t.mul(&self.element_transform(k));
                    if let Some(b) = self.bbox(k, &c, &kt) {
                        acc = Some(acc.map(|a| a.union(&b)).unwrap_or(b));
                    }
                }
                acc
            }
            "use" => {
                let target = n.href().and_then(|h| h.strip_prefix('#')).and_then(|id| self.doc.by_id(id))?;
                if self.active.contains(&target) {
                    return None;
                }
                let x = self.attr_len(node, "x", Axis::X, ctx, &st, 0.0);
                let y = self.attr_len(node, "y", Axis::Y, ctx, &st, 0.0);
                let mut c = ctx.clone();
                c.style = st;
                let tt = t.mul(&Transform::translate(x, y)).mul(&self.element_transform(target));
                self.active.push(target);
                self.depth += 1;
                let r = if self.doc.nodes[target].is_svg("symbol") {
                    let kids: Vec<usize> = self.doc.elements(target).collect();
                    let mut acc: Option<Rect> = None;
                    for k in kids {
                        let kt = tt.mul(&self.element_transform(k));
                        if let Some(b) = self.bbox(k, &c, &kt) {
                            acc = Some(acc.map(|a| a.union(&b)).unwrap_or(b));
                        }
                    }
                    acc
                } else {
                    self.bbox(target, &c, &tt)
                };
                self.depth -= 1;
                self.active.pop();
                r
            }
            "text" => text::text_bbox(self, node, ctx, &st, t),
            "image" => {
                let x = self.attr_len(node, "x", Axis::X, ctx, &st, 0.0);
                let y = self.attr_len(node, "y", Axis::Y, ctx, &st, 0.0);
                let w = self.attr_len(node, "width", Axis::X, ctx, &st, 0.0);
                let h = self.attr_len(node, "height", Axis::Y, ctx, &st, 0.0);
                Some(Rect::new(x, y, w, h).transform(t))
            }
            _ => None,
        }
    }

    // ------------------------------------------------------------------ painting

    /// Fill a coverage region with a shader.
    pub fn paint_coverage(&self, canvas: &mut Pixmap, cov: &Coverage, sh: &Shader, opacity: f32) {
        if opacity <= 0.0 {
            return;
        }
        if let Shader::Solid(c) = sh {
            for yy in 0..cov.h {
                for xx in 0..cov.w {
                    let v = cov.data[yy * cov.w + xx];
                    if v != 0 {
                        canvas.blend(cov.x + xx, cov.y + yy, *c, v as f32 / 255.0 * opacity);
                    }
                }
            }
            return;
        }
        for yy in 0..cov.h {
            for xx in 0..cov.w {
                let v = cov.data[yy * cov.w + xx];
                if v != 0 {
                    let (px, py) = (cov.x + xx, cov.y + yy);
                    let c = sh.at(px as f64 + 0.5, py as f64 + 0.5);
                    canvas.blend(px, py, c, v as f32 / 255.0 * opacity);
                }
            }
        }
    }

    fn resolve_paint<'p>(&self, p: &'p Paint, ctx: &'p Ctx) -> Option<&'p Paint> {
        match p {
            Paint::ContextFill => ctx.ctx_fill.as_deref(),
            Paint::ContextStroke => ctx.ctx_stroke.as_deref(),
            other => Some(other),
        }
    }

    /// A shader for a paint, with `bbox` the user-space object bounding box. `None` = paint nothing.
    pub fn shader(&mut self, p: &Paint, st: &Style, bbox: Option<Rect>, ctx: &Ctx) -> Option<Shader> {
        let p = self.resolve_paint(p, ctx)?.clone();
        match p {
            Paint::None | Paint::ContextFill | Paint::ContextStroke => None,
            Paint::Color(c) => Some(Shader::Solid(premul([c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0, c.a]))),
            Paint::CurrentColor => {
                let c = st.color;
                Some(Shader::Solid(premul([c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0, c.a])))
            }
            Paint::Url(id, fb) => {
                let target = self.doc.by_id(&id).filter(|&t| {
                    let n = &self.doc.nodes[t];
                    (n.is_svg("linearGradient") || n.is_svg("radialGradient") || n.is_svg("pattern")) && !self.resource_hidden(t)
                });
                match target {
                    Some(t) => {
                        if self.doc.nodes[t].is_svg("pattern") {
                            self.pattern_shader(t, bbox, ctx)
                        } else {
                            self.gradient_shader(t, bbox, ctx)
                        }
                    }
                    None => match fb {
                        Some(f) => self.shader(&f, st, bbox, ctx),
                        None => None,
                    },
                }
            }
        }
    }

    /// Follow `href` on gradients/patterns: the chain of elements (self first), cycle-safe.
    fn href_chain(&self, node: usize, kinds: &[&str]) -> Vec<usize> {
        let mut out = alloc::vec![node];
        let mut cur = node;
        while let Some(t) = self.doc.nodes[cur].href().and_then(|h| h.strip_prefix('#')).and_then(|id| self.doc.by_id(id)) {
            if out.contains(&t) || !kinds.iter().any(|k| self.doc.nodes[t].is_svg(k)) {
                break;
            }
            out.push(t);
            cur = t;
        }
        out
    }

    /// An attribute along an href chain, taken only from elements of the same kind as the first (Chromium
    /// inherits no attributes across linear/radial gradients — only stops).
    fn chain_attr<'d>(&'d self, chain: &[usize], a: &str) -> Option<&'d str> {
        let kind = self.doc.nodes[chain[0]].name();
        chain.iter().filter(|&&n| self.doc.nodes[n].name() == kind).find_map(|&n| self.doc.nodes[n].attr(a))
    }

    fn gradient_shader(&mut self, node: usize, bbox: Option<Rect>, ctx: &Ctx) -> Option<Shader> {
        let chain = self.href_chain(node, &["linearGradient", "radialGradient"]);
        // Stops: from the first element in the chain that has any.
        let stop_owner = chain.iter().copied().find(|&n| self.doc.elements(n).any(|c| self.doc.nodes[c].is_svg("stop")));
        let mut stops: Vec<Stop> = Vec::new();
        if let Some(o) = stop_owner {
            let kids: Vec<usize> = self.doc.elements(o).filter(|&c| self.doc.nodes[c].is_svg("stop")).collect();
            for k in kids {
                let ks = self.doc_style(k);
                let off = self.doc.nodes[k].attr("offset").map(|v| {
                    let v = v.trim();
                    if let Some(p) = v.strip_suffix('%') { p.trim().parse::<f64>().unwrap_or(0.0) / 100.0 } else { v.parse::<f64>().unwrap_or(0.0) }
                });
                let mut off = off.unwrap_or(0.0).clamp(0.0, 1.0);
                if let Some(l) = stops.last() {
                    off = off.max(l.offset);
                }
                let c = ks.stop_color;
                stops.push(Stop {
                    offset: off,
                    color: [c.r as f32 / 255.0, c.g as f32 / 255.0, c.b as f32 / 255.0, c.a * ks.stop_opacity as f32],
                });
            }
        }
        if stops.is_empty() {
            return None;
        }
        if stops.len() == 1 {
            return Some(Shader::Solid(premul(stops[0].color)));
        }
        let obb = self.chain_attr(&chain, "gradientUnits").map(|v| v.trim() != "userSpaceOnUse").unwrap_or(true);
        let gt = self.chain_attr(&chain, "gradientTransform").and_then(parse_transform).unwrap_or(Transform::IDENTITY);
        let spread = match self.chain_attr(&chain, "spreadMethod").map(|s| s.trim()) {
            Some("reflect") => Spread::Reflect,
            Some("repeat") => Spread::Repeat,
            _ => Spread::Pad,
        };
        let units = if obb {
            let b = bbox?;
            if b.w <= 0.0 || b.h <= 0.0 {
                return None;
            }
            Transform::new(b.w, 0.0, 0.0, b.h, b.x, b.y)
        } else {
            Transform::IDENTITY
        };
        let total = ctx.ts.mul(&units).mul(&gt);
        let inv = total.invert()?;
        let st = self.doc_style(node);
        let gl = |r: &Self, a: &str, axis: Axis, def: Length| -> f64 {
            let l = r.chain_attr(&chain, a).and_then(parse_length).unwrap_or(def);
            if obb {
                match l.unit {
                    style::Unit::Percent => l.v / 100.0,
                    _ => resolve(l, axis, 1.0, 1.0, st.font_size),
                }
            } else {
                resolve(l, axis, ctx.vw, ctx.vh, st.font_size)
            }
        };
        let is_linear = self.doc.nodes[node].is_svg("linearGradient");
        if is_linear {
            let x1 = gl(self, "x1", Axis::X, Length::pct(0.0));
            let y1 = gl(self, "y1", Axis::Y, Length::pct(0.0));
            let x2 = gl(self, "x2", Axis::X, Length::pct(100.0));
            let y2 = gl(self, "y2", Axis::Y, Length::pct(0.0));
            if x1 == x2 && y1 == y2 {
                // SVG 2 §14.3.2: the area is painted with the last stop's colour.
                return Some(Shader::Solid(premul(stops[stops.len() - 1].color)));
            }
            Some(Shader::Linear { inv, p1: (x1, y1), p2: (x2, y2), stops, spread })
        } else {
            let cx = gl(self, "cx", Axis::X, Length::pct(50.0));
            let cy = gl(self, "cy", Axis::Y, Length::pct(50.0));
            let r = gl(self, "r", Axis::Diag, Length::pct(50.0));
            let fx = if self.chain_attr(&chain, "fx").is_some() { gl(self, "fx", Axis::X, Length::pct(50.0)) } else { cx };
            let fy = if self.chain_attr(&chain, "fy").is_some() { gl(self, "fy", Axis::Y, Length::pct(50.0)) } else { cy };
            // A negative fr is treated as 0; r ≤ 0 paints the last stop (Chromium).
            let fr = gl(self, "fr", Axis::Diag, Length::pct(0.0)).max(0.0);
            if r <= 0.0 {
                return Some(Shader::Solid(premul(stops[stops.len() - 1].color)));
            }
            Some(Shader::Radial { inv, c: (cx, cy), r, f: (fx, fy), fr, stops, spread })
        }
    }

    fn pattern_shader(&mut self, node: usize, bbox: Option<Rect>, ctx: &Ctx) -> Option<Shader> {
        if self.active.contains(&node) || self.depth > MAX_DEPTH {
            return None;
        }
        let chain = self.href_chain(node, &["pattern"]);
        let content = chain.iter().copied().find(|&n| self.doc.elements(n).next().is_some());
        let obb = self.chain_attr(&chain, "patternUnits").map(|v| v.trim() != "userSpaceOnUse").unwrap_or(true);
        let cobb = self.chain_attr(&chain, "patternContentUnits").map(|v| v.trim() == "objectBoundingBox").unwrap_or(false);
        let pt = self.chain_attr(&chain, "patternTransform").and_then(parse_transform).unwrap_or(Transform::IDENTITY);
        let vb = self.chain_attr(&chain, "viewBox").and_then(parse_view_box);
        let par = self.chain_attr(&chain, "preserveAspectRatio").map(parse_par).unwrap_or_default();
        let st = self.doc_style(node);
        let gl = |r: &Self, a: &str, axis: Axis| -> f64 {
            let l = r.chain_attr(&chain, a).and_then(parse_length).unwrap_or(Length::ZERO);
            if obb {
                match l.unit {
                    style::Unit::Percent => l.v / 100.0,
                    _ => resolve(l, axis, 1.0, 1.0, st.font_size),
                }
            } else {
                resolve(l, axis, ctx.vw, ctx.vh, st.font_size)
            }
        };
        let (mut x, mut y, mut w, mut h) = (gl(self, "x", Axis::X), gl(self, "y", Axis::Y), gl(self, "width", Axis::X), gl(self, "height", Axis::Y));
        if obb || (cobb && vb.is_none()) {
            let b = bbox?;
            if obb {
                if b.w <= 0.0 || b.h <= 0.0 {
                    return None;
                }
                x = b.x + x * b.w;
                y = b.y + y * b.h;
                w *= b.w;
                h *= b.h;
            }
        }
        if !(w > 0.0 && h > 0.0) {
            return None;
        }
        let p = ctx.ts.mul(&pt);
        let sx = crate::fmath::hypot(p.a, p.b);
        let sy = crate::fmath::hypot(p.c, p.d);
        if !(sx > 0.0 && sy > 0.0) {
            return None;
        }
        let pw = (ceil(w * sx) as usize).clamp(1, 4096);
        let ph = (ceil(h * sy) as usize).clamp(1, 4096);
        let (kx, ky) = (pw as f64 / w, ph as f64 / h);
        let content_ts = if let Some(vb) = vb {
            if vb.w <= 0.0 || vb.h <= 0.0 {
                return None;
            }
            view_box_transform(&vb, &par, 0.0, 0.0, w, h)
        } else if cobb {
            let b = bbox?;
            Transform::new(b.w, 0.0, 0.0, b.h, 0.0, 0.0)
        } else {
            Transform::IDENTITY
        };
        let tile_ts = Transform::scale(kx, ky).mul(&content_ts);
        let mut tile = Pixmap::new(pw, ph);
        if let Some(c) = content {
            let mut sub = self.sub(pw, ph);
            sub.active.push(node);
            let cst = self.doc_style(c);
            let cctx = Ctx {
                ts: tile_ts,
                vw: vb.map(|v| v.w).unwrap_or(ctx.vw),
                vh: vb.map(|v| v.h).unwrap_or(ctx.vh),
                style: cst,
                ctx_fill: None,
                ctx_stroke: None,
            };
            let kids: Vec<usize> = self.doc.elements(c).collect();
            for k in kids {
                sub.render(k, &cctx, &mut tile);
            }
        }
        let inv_p = p.invert()?;
        let inv = Transform::scale(kx, ky).mul(&Transform::translate(-x, -y)).mul(&inv_p);
        Some(Shader::Image { inv, pix: tile, smooth: true, repeat: true })
    }

    // ------------------------------------------------------------------ clip & mask

    pub fn clip_mask(&mut self, id: &str, bbox: Option<Rect>, ctx: &Ctx) -> ClipResult {
        let Some(cp) = self.doc.by_id(id).filter(|&c| self.doc.nodes[c].is_svg("clipPath")) else {
            return ClipResult::Ignore;
        };
        if self.active.contains(&cp) || self.depth > MAX_DEPTH {
            // A self-referencing clip path is an error: the element is not rendered.
            return ClipResult::Nothing;
        }
        // Chromium treats a clipPath with `display: none` as an invalid reference (renders unclipped).
        if self.resource_hidden(cp) {
            return ClipResult::Ignore;
        }
        let cn = &self.doc.nodes[cp];
        let obb = cn.attr("clipPathUnits").map(|v| v.trim() == "objectBoundingBox").unwrap_or(false);
        let mut ts = ctx.ts.mul(&self.element_transform(cp));
        if obb {
            let Some(b) = bbox.filter(|b| b.w > 0.0 && b.h > 0.0) else { return ClipResult::Nothing };
            ts = ts.mul(&Transform::new(b.w, 0.0, 0.0, b.h, b.x, b.y));
        }
        let cst = self.doc_style(cp);
        let mut mask = Mask::new(self.w, self.h, 0);
        self.active.push(cp);
        self.depth += 1;
        let base = Ctx { ts, vw: ctx.vw, vh: ctx.vh, style: cst.clone(), ctx_fill: None, ctx_stroke: None };
        let kids: Vec<usize> = self.doc.elements(cp).collect();
        for k in kids {
            self.clip_child(k, &base, &mut mask);
        }
        // The clipPath's own clip-path intersects.
        let own = get(&self.props[cp], "clip-path").and_then(parse_func_iri);
        let mut result = ClipResult::Mask(mask);
        if let Some(oid) = own {
            match self.clip_mask(&oid, bbox, ctx) {
                ClipResult::Mask(m2) => {
                    if let ClipResult::Mask(m) = &mut result {
                        m.multiply(&m2);
                    }
                }
                ClipResult::Nothing => result = ClipResult::Nothing,
                ClipResult::Ignore => {}
            }
        }
        self.depth -= 1;
        self.active.pop();
        result
    }

    fn clip_child(&mut self, k: usize, base: &Ctx, mask: &mut Mask) {
        let n = &self.doc.nodes[k];
        if !n.is_svg_element() {
            return;
        }
        let st = Style::compute(&base.style, &self.props[k]);
        if st.display_none {
            return;
        }
        let name = n.name();
        let mut ts = base.ts.mul(&self.element_transform(k));
        let mut ctx = Ctx { ts, ..base.clone() };
        ctx.style = st.clone();
        let mut target = k;
        let mut tstyle = st.clone();
        if name == "use" {
            let Some(t) = n.href().and_then(|h| h.strip_prefix('#')).and_then(|id| self.doc.by_id(id)) else { return };
            let tn = &self.doc.nodes[t];
            if !matches!(tn.name(), "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" | "text") || !tn.is_svg_element() {
                return;
            }
            let x = self.attr_len(k, "x", Axis::X, &ctx, &st, 0.0);
            let y = self.attr_len(k, "y", Axis::Y, &ctx, &st, 0.0);
            ts = ts.mul(&Transform::translate(x, y)).mul(&self.element_transform(t));
            tstyle = Style::compute(&st, &self.props[t]);
            if tstyle.display_none {
                return;
            }
            ctx.ts = ts;
            ctx.style = tstyle.clone();
            target = t;
        } else if !matches!(name, "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" | "text") {
            return;
        }
        let mut local = Mask::new(self.w, self.h, 0);
        let tname = self.doc.nodes[target].name();
        if tname == "text" {
            let parent_ctx = Ctx { ts, ..base.clone() };
            let mut pc = parent_ctx;
            pc.style = if target == k { base.style.clone() } else { st.clone() };
            text::clip_text(self, target, &pc, &mut local);
        } else {
            if !tstyle.visible {
                return;
            }
            let Some(p) = self.shape_path(target, &ctx, &tstyle) else { return };
            if let Some(c) = fill_coverage(&p.transform(&ts), tstyle.clip_rule, self.w, self.h, tstyle.aa) {
                local.union_coverage(&c);
            }
        }
        // A child's own clip-path intersects that child only.
        if let Some(cid) = get(&self.props[k], "clip-path").and_then(parse_func_iri) {
            let bb = self.bbox(k, base, &Transform::IDENTITY);
            let cctx = Ctx { ts: base.ts.mul(&self.element_transform(k)), ..base.clone() };
            match self.clip_mask(&cid, bb.map(|b| b), &cctx) {
                ClipResult::Mask(m) => local.multiply(&m),
                ClipResult::Nothing => return,
                ClipResult::Ignore => {}
            }
        }
        for (a, b) in mask.data.iter_mut().zip(local.data.iter()) {
            let (x, y) = (*a as u32, *b as u32);
            *a = (x + y - crate::raster::div255(x * y)).min(255) as u8;
        }
    }

    pub fn mask_mask(&mut self, id: &str, bbox: Option<Rect>, ctx: &Ctx) -> ClipResult {
        let Some(mk) = self.doc.by_id(id).filter(|&c| self.doc.nodes[c].is_svg("mask") && !self.resource_hidden(c)) else {
            return ClipResult::Ignore;
        };
        if self.active.contains(&mk) || self.depth > MAX_DEPTH {
            return ClipResult::Nothing;
        }
        let mn = &self.doc.nodes[mk];
        let obb = mn.attr("maskUnits").map(|v| v.trim() != "userSpaceOnUse").unwrap_or(true);
        let cobb = mn.attr("maskContentUnits").map(|v| v.trim() == "objectBoundingBox").unwrap_or(false);
        let mst = self.doc_style(mk);
        let ml = |a: &str, axis: Axis, def: Length| -> f64 {
            let l = mn.attr(a).and_then(parse_length).unwrap_or(def);
            if obb {
                match l.unit {
                    style::Unit::Percent => l.v / 100.0,
                    _ => resolve(l, axis, 1.0, 1.0, mst.font_size),
                }
            } else {
                resolve(l, axis, ctx.vw, ctx.vh, mst.font_size)
            }
        };
        let (mut x, mut y, mut w, mut h) =
            (ml("x", Axis::X, Length::pct(-10.0)), ml("y", Axis::Y, Length::pct(-10.0)), ml("width", Axis::X, Length::pct(120.0)), ml("height", Axis::Y, Length::pct(120.0)));
        if obb || cobb {
            let Some(b) = bbox.filter(|b| b.w > 0.0 && b.h > 0.0) else { return ClipResult::Nothing };
            if obb {
                x = b.x + x * b.w;
                y = b.y + y * b.h;
                w *= b.w;
                h *= b.h;
            }
        }
        if !(w > 0.0 && h > 0.0) {
            return ClipResult::Nothing;
        }
        let content_ts = if cobb {
            let b = bbox.unwrap();
            ctx.ts.mul(&Transform::new(b.w, 0.0, 0.0, b.h, b.x, b.y))
        } else {
            ctx.ts
        };
        let mut layer = Pixmap::new(self.w, self.h);
        self.active.push(mk);
        self.depth += 1;
        let cctx = Ctx { ts: content_ts, vw: ctx.vw, vh: ctx.vh, style: mst.clone(), ctx_fill: None, ctx_stroke: None };
        let kids: Vec<usize> = self.doc.elements(mk).collect();
        for k in kids {
            self.render(k, &cctx, &mut layer);
        }
        let mut region = Path::new();
        region.move_to(x, y);
        region.line_to(x + w, y);
        region.line_to(x + w, y + h);
        region.line_to(x, y + h);
        region.close();
        let mut rmask = Mask::new(self.w, self.h, 0);
        if let Some(c) = fill_coverage(&region.transform(&ctx.ts), FillRule::NonZero, self.w, self.h, true) {
            rmask.union_coverage(&c);
        }
        let linear = mst.linear_rgb_interp;
        let mut m = Mask::new(self.w, self.h, 0);
        for p in 0..self.w * self.h {
            let px = &layer.data[p * 4..p * 4 + 4];
            let a = px[3] as f64 / 255.0;
            let v = if mst.mask_alpha {
                a
            } else if a == 0.0 {
                0.0
            } else {
                let lin = |c: u8| {
                    let s = c as f64 / 255.0 / a;
                    if linear {
                        if s <= 0.04045 { s / 12.92 } else { crate::fmath::powf((s + 0.055) / 1.055, 2.4) }
                    } else {
                        s
                    }
                };
                (0.2125 * lin(px[0]) + 0.7154 * lin(px[1]) + 0.0721 * lin(px[2])) * a
            };
            m.data[p] = ((v * 255.0 + 0.5) as u32).min(255) as u8;
        }
        m.multiply(&rmask);
        // `mask` on a <mask> element is not applied (Chromium ignores it; CSS Masking does not define it).
        let res = ClipResult::Mask(m);
        self.depth -= 1;
        self.active.pop();
        res
    }

    // ------------------------------------------------------------------ elements

    /// Enter a resource element (marker): `true` when it is already active (a cycle) or too deep.
    pub fn depth_guard(&mut self, n: usize) -> bool {
        if self.active.contains(&n) || self.depth > MAX_DEPTH {
            return true;
        }
        self.active.push(n);
        self.depth += 1;
        false
    }

    pub fn depth_release(&mut self) {
        self.active.pop();
        self.depth -= 1;
    }

    /// The root `<svg>`: its own group effects, then its children (the viewport transform is in `ctx`).
    pub fn render_root(&mut self, node: usize, ctx: &Ctx, canvas: &mut Pixmap) {
        // `display: none` on the root of an SVG image is ignored (Chromium renders it).
        let st = Style { display_none: false, ..Style::compute(&ctx.style, &self.props[node]) };
        self.root_font = st.font_size;
        self.root_lh = self.font_metrics(&st).3 * st.font_size;
        let ectx = Ctx { style: st.clone(), ..ctx.clone() };
        self.with_effects(node, ctx, &ectx, &st, canvas, |r, c, l| r.children(node, c, l));
    }

    /// Render one element (and its subtree) in context `ctx` (whose style is the parent's computed style).
    pub fn render(&mut self, node: usize, ctx: &Ctx, canvas: &mut Pixmap) {
        if self.depth > MAX_DEPTH {
            return;
        }
        let n = &self.doc.nodes[node];
        if !n.is_svg_element() {
            return;
        }
        let name = n.name();
        if !matches!(
            name,
            "g" | "a" | "switch" | "svg" | "use" | "path" | "rect" | "circle" | "ellipse" | "line" | "polyline" | "polygon" | "text" | "image"
        ) {
            return;
        }
        if !conditions_pass(n, &self.opts.languages) {
            return;
        }
        let st = Style::compute(&ctx.style, &self.props[node]);
        if st.display_none {
            return;
        }
        let ts = if name == "svg" { ctx.ts } else { ctx.ts.mul(&self.element_transform(node)) };
        if !ts.is_finite() || ts.det() == 0.0 && name != "svg" {
            return;
        }
        let ectx = Ctx { ts, vw: ctx.vw, vh: ctx.vh, style: st.clone(), ctx_fill: ctx.ctx_fill.clone(), ctx_stroke: ctx.ctx_stroke.clone() };
        let p = &self.props[node];
        let clip = get(p, "clip-path").filter(|v| v.trim() != "none").map(|v| parse_func_iri(v));
        let mask = get(p, "mask").filter(|v| v.trim() != "none").map(|v| parse_func_iri(v));
        let has_filter = get(p, "filter").map(|v| v.trim() != "none").unwrap_or(false);
        let _ = has_filter; // filters are out of scope (SR52 ceiling): content renders unfiltered
        let needs_layer = st.opacity < 1.0 || clip.is_some() || mask.is_some();
        if !needs_layer {
            self.depth += 1;
            self.content(node, &ectx, &st, &ctx.style, canvas);
            self.depth -= 1;
            return;
        }
        if st.opacity <= 0.0 {
            return;
        }
        let bbox = if clip.is_some() || mask.is_some() { self.bbox(node, ctx, &Transform::IDENTITY) } else { None };
        let mut layer = Pixmap::new(self.w, self.h);
        self.depth += 1;
        self.content(node, &ectx, &st, &ctx.style, &mut layer);
        if let Some(c) = clip {
            match c {
                // An unparsable value: ignored.
                None => {}
                Some(id) => match self.clip_mask(&id, bbox, &ectx) {
                    ClipResult::Mask(m) => layer.apply_mask(&m),
                    ClipResult::Ignore => {}
                    ClipResult::Nothing => {
                        self.depth -= 1;
                        return;
                    }
                },
            }
        }
        if let Some(Some(id)) = mask {
            match self.mask_mask(&id, bbox, &ectx) {
                ClipResult::Mask(m) => layer.apply_mask(&m),
                // A reference to a missing element is ignored (Chromium renders the element unmasked).
                ClipResult::Ignore => {}
                ClipResult::Nothing => {
                    self.depth -= 1;
                    return;
                }
            }
        }
        self.depth -= 1;
        canvas.draw_pixmap(&layer, st.opacity as f32, None);
    }

    fn children(&mut self, node: usize, ctx: &Ctx, canvas: &mut Pixmap) {
        let kids: Vec<usize> = self.doc.elements(node).collect();
        for k in kids {
            self.render(k, ctx, canvas);
        }
    }

    fn content(&mut self, node: usize, ctx: &Ctx, st: &Style, parent: &Style, canvas: &mut Pixmap) {
        let name = self.doc.nodes[node].name();
        match name {
            "g" | "a" => self.children(node, ctx, canvas),
            "switch" => {
                let kids: Vec<usize> = self.doc.elements(node).collect();
                for k in kids {
                    let kn = &self.doc.nodes[k];
                    if !kn.is_svg_element() || !conditions_pass(kn, &self.opts.languages) {
                        continue;
                    }
                    // The first child whose conditions pass is chosen even if it has display: none.
                    let _ = st;
                    self.render(k, ctx, canvas);
                    break;
                }
            }
            "svg" => self.nested_svg(node, None, ctx, st, canvas),
            "use" => self.use_element(node, ctx, st, canvas),
            "text" => text::render_text(self, node, ctx, parent, canvas),
            "image" => self.image(node, ctx, st, canvas),
            _ => {
                if let Some(path) = self.shape_path(node, ctx, st) {
                    self.draw_path(&path, node, ctx, st, canvas, true);
                }
            }
        }
    }

    /// Fill and stroke (in `paint-order`) a user-space path; `markers` adds marker rendering.
    pub fn draw_path(&mut self, path: &Path, node: usize, ctx: &Ctx, st: &Style, canvas: &mut Pixmap, markers: bool) {
        if !st.visible {
            return;
        }
        let bbox = path.bbox();
        let order: [u8; 3] = if st.markers_first {
            if st.stroke_first { [2, 1, 0] } else { [2, 0, 1] }
        } else if st.stroke_first {
            [1, 0, 2]
        } else {
            [0, 1, 2]
        };
        for o in order {
            match o {
                0 => self.fill_path(path, st, bbox, ctx, canvas),
                1 => self.stroke_path(path, st, bbox, ctx, canvas),
                _ => {
                    if markers {
                        crate::marker::render_markers(self, path, node, ctx, st, canvas);
                    }
                }
            }
        }
    }

    pub fn fill_path(&mut self, path: &Path, st: &Style, bbox: Option<Rect>, ctx: &Ctx, canvas: &mut Pixmap) {
        if matches!(st.fill, Paint::None) {
            return;
        }
        let Some(sh) = self.shader(&st.fill.clone(), st, bbox, ctx) else { return };
        if let Some(c) = fill_coverage(&path.transform(&ctx.ts), st.fill_rule, self.w, self.h, st.aa) {
            self.paint_coverage(canvas, &c, &sh, st.fill_opacity as f32);
        }
    }

    pub fn stroke_style(&self, st: &Style, ctx: &Ctx) -> Option<StrokeStyle> {
        let width = self.len(st.stroke_width, Axis::Diag, ctx, st);
        if !(width > 0.0) {
            return None;
        }
        let dash = st.dasharray.as_ref().and_then(|d| {
            let v: Vec<f64> = d.iter().map(|l| self.len(*l, Axis::Diag, ctx, st)).collect();
            normalize_dashes(v)
        });
        let off = self.len(st.dashoffset, Axis::Diag, ctx, st);
        Some(StrokeStyle { width, cap: st.cap, join: st.join, miter_limit: st.miter_limit, dash: dash.map(|d| (d, off)) })
    }

    pub fn stroke_path(&mut self, path: &Path, st: &Style, bbox: Option<Rect>, ctx: &Ctx, canvas: &mut Pixmap) {
        if matches!(st.stroke, Paint::None) {
            return;
        }
        let Some(ss) = self.stroke_style(st, ctx) else { return };
        let Some(sh) = self.shader(&st.stroke.clone(), st, bbox, ctx) else { return };
        let polys = stroke_polys(path, &ss, &ctx.ts);
        if let Some(c) = fill_polys(&polys, FillRule::NonZero, self.w, self.h, st.aa) {
            self.paint_coverage(canvas, &c, &sh, st.stroke_opacity as f32);
        }
    }

    /// A new viewport (nested `<svg>`, or the `<symbol>`/`<svg>` a `<use>` instantiates; SVG 2 §8.2).
    pub fn nested_svg(&mut self, node: usize, use_wh: Option<(Option<f64>, Option<f64>)>, ctx: &Ctx, st: &Style, canvas: &mut Pixmap) {
        let n = &self.doc.nodes[node];
        let is_symbol = n.is_svg("symbol");
        let x = self.attr_len(node, "x", Axis::X, ctx, st, 0.0);
        let y = self.attr_len(node, "y", Axis::Y, ctx, st, 0.0);
        let mut w = self.attr_len(node, "width", Axis::X, ctx, st, ctx.vw);
        let mut h = self.attr_len(node, "height", Axis::Y, ctx, st, ctx.vh);
        if n.attr("width").is_none() && get(&self.props[node], "width").is_none() {
            w = ctx.vw;
        }
        if n.attr("height").is_none() && get(&self.props[node], "height").is_none() {
            h = ctx.vh;
        }
        if let Some((uw, uh)) = use_wh {
            if let Some(v) = uw {
                w = v;
            }
            if let Some(v) = uh {
                h = v;
            }
        }
        if !(w > 0.0 && h > 0.0) {
            return;
        }
        let vb = n.attr("viewBox").and_then(parse_view_box);
        let par = n.attr("preserveAspectRatio").map(parse_par).unwrap_or_default();
        let (inner, vw, vh) = match vb {
            Some(v) if v.w > 0.0 && v.h > 0.0 => (view_box_transform(&v, &par, x, y, w, h), v.w, v.h),
            Some(_) => return,
            None => (Transform::translate(x, y), w, h),
        };
        let cctx = Ctx { ts: ctx.ts.mul(&inner), vw, vh, style: st.clone(), ctx_fill: ctx.ctx_fill.clone(), ctx_stroke: ctx.ctx_stroke.clone() };
        let clip = !st.overflow_visible;
        let _ = is_symbol;
        if clip {
            let mut layer = Pixmap::new(self.w, self.h);
            self.children(node, &cctx, &mut layer);
            let mut r = Path::new();
            r.move_to(x, y);
            r.line_to(x + w, y);
            r.line_to(x + w, y + h);
            r.line_to(x, y + h);
            r.close();
            let mut m = Mask::new(self.w, self.h, 0);
            if let Some(c) = fill_coverage(&r.transform(&ctx.ts), FillRule::NonZero, self.w, self.h, true) {
                m.union_coverage(&c);
            }
            canvas.draw_pixmap(&layer, 1.0, Some(&m));
        } else {
            self.children(node, &cctx, canvas);
        }
    }

    fn use_element(&mut self, node: usize, ctx: &Ctx, st: &Style, canvas: &mut Pixmap) {
        let n = &self.doc.nodes[node];
        let Some(target) = n.href().and_then(|h| h.strip_prefix('#')).and_then(|id| self.doc.by_id(id)) else { return };
        // Cycles: the target is being instantiated already, or it contains this use.
        if self.active.contains(&target) || target == node || self.doc.is_ancestor(target, node) {
            return;
        }
        let x = self.attr_len(node, "x", Axis::X, ctx, st, 0.0);
        let y = self.attr_len(node, "y", Axis::Y, ctx, st, 0.0);
        let mut cctx = ctx.clone();
        cctx.ts = ctx.ts.mul(&Transform::translate(x, y));
        cctx.style = st.clone();
        // The use element is the context element of its shadow tree (SVG 2 §13.3 context-fill/stroke).
        cctx.ctx_fill = self.resolve_paint(&st.fill, ctx).cloned().map(Box::new);
        cctx.ctx_stroke = self.resolve_paint(&st.stroke, ctx).cloned().map(Box::new);
        self.active.push(target);
        let tn = &self.doc.nodes[target];
        if tn.is_svg("symbol") || tn.is_svg("svg") {
            let uw = n.attr("width").and_then(parse_length).map(|l| self.len(l, Axis::X, ctx, st));
            let uh = n.attr("height").and_then(parse_length).map(|l| self.len(l, Axis::Y, ctx, st));
            let tst = Style::compute(st, &self.props[target]);
            if !tst.display_none || tn.is_svg("symbol") {
                // symbol is display:none by UA style only outside of use; its own display property applies.
                let tst = if tn.is_svg("symbol") { Style { display_none: false, ..tst } } else { tst };
                let tts = cctx.ts.mul(&self.element_transform(target));
                let tctx = Ctx { ts: tts, ..cctx.clone() };
                // Group effects on the symbol/svg itself.
                let tst2 = tst.clone();
                self.with_effects(target, &tctx, &tctx, &tst, canvas, |r, c, layer| r.nested_svg(target, Some((uw, uh)), c, &tst2, layer));
            }
        } else {
            self.render(target, &cctx, canvas);
        }
        self.active.pop();
    }

    /// Run `f` with the opacity / clip-path / mask of `node` applied.
    fn with_effects(&mut self, node: usize, pctx: &Ctx, ctx: &Ctx, st: &Style, canvas: &mut Pixmap, f: impl FnOnce(&mut Self, &Ctx, &mut Pixmap)) {
        let p = &self.props[node];
        let clip = get(p, "clip-path").and_then(parse_func_iri);
        let mask = get(p, "mask").and_then(parse_func_iri);
        if st.opacity >= 1.0 && clip.is_none() && mask.is_none() {
            f(self, ctx, canvas);
            return;
        }
        let bbox = self.bbox(node, pctx, &Transform::IDENTITY);
        let mut layer = Pixmap::new(self.w, self.h);
        f(self, ctx, &mut layer);
        if let Some(id) = clip {
            match self.clip_mask(&id, bbox, ctx) {
                ClipResult::Mask(m) => layer.apply_mask(&m),
                ClipResult::Ignore => {}
                ClipResult::Nothing => return,
            }
        }
        if let Some(id) = mask {
            match self.mask_mask(&id, bbox, ctx) {
                ClipResult::Mask(m) => layer.apply_mask(&m),
                ClipResult::Ignore => {}
                ClipResult::Nothing => return,
            }
        }
        canvas.draw_pixmap(&layer, st.opacity as f32, None);
    }

    fn image(&mut self, node: usize, ctx: &Ctx, st: &Style, canvas: &mut Pixmap) {
        if !st.visible {
            return;
        }
        let n = &self.doc.nodes[node];
        let Some(href) = n.href() else { return };
        let Some((mime, data)) = crate::image::parse_data_url(href) else { return };
        let x = self.attr_len(node, "x", Axis::X, ctx, st, 0.0);
        let y = self.attr_len(node, "y", Axis::Y, ctx, st, 0.0);
        let wa = get(&self.props[node], "width").or(n.attr("width")).filter(|v| v.trim() != "auto").and_then(parse_length);
        let ha = get(&self.props[node], "height").or(n.attr("height")).filter(|v| v.trim() != "auto").and_then(parse_length);
        let par = n.attr("preserveAspectRatio").map(parse_par).unwrap_or_default();
        let is_svg = mime.contains("svg") || crate::sniff_svg(&data);
        if is_svg {
            if self.depth > 8 {
                return;
            }
            let Ok(sub) = crate::Svg::parse(&data) else { return };
            let (iw, ih) = sub.size_f();
            let w = wa.map(|l| self.len(l, Axis::X, ctx, st)).unwrap_or(iw);
            let h = ha.map(|l| self.len(l, Axis::Y, ctx, st)).unwrap_or(ih);
            if !(w > 0.0 && h > 0.0) {
                return;
            }
            // The image's own viewBox (or its intrinsic size) maps into (x, y, w, h) with the image element's pAR.
            let vb = sub.view_box.unwrap_or(Rect::new(0.0, 0.0, iw, ih));
            let t = ctx.ts.mul(&view_box_transform(&vb, &par, x, y, w, h));
            let mut layer = Pixmap::new(self.w, self.h);
            let mut r = Renderer::new(&sub.doc, &sub.props, self.opts, self.w, self.h);
            r.depth = self.depth + 4;
            sub.render_into(&mut r, t, vb, &mut layer);
            let mut clip = Path::new();
            clip.move_to(x, y);
            clip.line_to(x + w, y);
            clip.line_to(x + w, y + h);
            clip.line_to(x, y + h);
            clip.close();
            let mut m = Mask::new(self.w, self.h, 0);
            if let Some(c) = fill_coverage(&clip.transform(&ctx.ts), FillRule::NonZero, self.w, self.h, true) {
                m.union_coverage(&c);
            }
            canvas.draw_pixmap(&layer, 1.0, Some(&m));
            return;
        }
        let Some(dec) = self.opts.image_decoder else { return };
        let Some(img) = dec(&data) else { return };
        if img.width == 0 || img.height == 0 || img.rgba.len() < img.width * img.height * 4 {
            return;
        }
        let (iw, ih) = (img.width as f64, img.height as f64);
        let w = wa.map(|l| self.len(l, Axis::X, ctx, st)).unwrap_or(iw);
        let h = ha.map(|l| self.len(l, Axis::Y, ctx, st)).unwrap_or(ih);
        if !(w > 0.0 && h > 0.0) {
            return;
        }
        let it = view_box_transform(&Rect::new(0.0, 0.0, iw, ih), &par, x, y, w, h);
        let Some(inv) = ctx.ts.mul(&it).invert() else { return };
        let mut pix = Pixmap::new(img.width, img.height);
        for (d, s) in pix.data.chunks_exact_mut(4).zip(img.rgba.chunks_exact(4)) {
            let a = s[3] as u32;
            d[0] = crate::raster::div255(s[0] as u32 * a) as u8;
            d[1] = crate::raster::div255(s[1] as u32 * a) as u8;
            d[2] = crate::raster::div255(s[2] as u32 * a) as u8;
            d[3] = s[3];
        }
        // Draw the intersection of the viewport and the placed image.
        let ib = Rect::new(it.e, it.f, iw * it.a, ih * it.d);
        let x0 = x.max(ib.x);
        let y0 = y.max(ib.y);
        let x1 = (x + w).min(ib.right());
        let y1 = (y + h).min(ib.bottom());
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        let mut r = Path::new();
        r.move_to(x0, y0);
        r.line_to(x1, y0);
        r.line_to(x1, y1);
        r.line_to(x0, y1);
        r.close();
        let sh = Shader::Image { inv, pix, smooth: st.image_smooth, repeat: false };
        if let Some(c) = fill_coverage(&r.transform(&ctx.ts), FillRule::NonZero, self.w, self.h, true) {
            self.paint_coverage(canvas, &c, &sh, 1.0);
        }
        let _ = sqrt(0.0);
    }
}
