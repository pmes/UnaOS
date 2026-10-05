//! Properties: the cascade (presentation attributes < `<style>` rules by specificity and order < the `style`
//! attribute; `!important` above both — CSS Cascade 4 §6 as SVG 2 §6.3 applies it), inheritance, and the
//! computed values the renderer reads. Lengths (SVG 1.1 §7.10 units, CSS percentages) are resolved by the
//! renderer against the viewport it knows.

use crate::color::{self, Color, ParsedColor};
use crate::css::{self, Declaration, Stylesheet};
use crate::geom::number;
use crate::raster::FillRule;
use crate::stroke::{Cap, Join};
use crate::xml::{Document, Kind, Ns};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

/// Every property that is also a presentation attribute (SVG 2 §6.6 table) plus the few geometry properties.
pub const PRESENTATION: &[&str] = &[
    "alignment-baseline", "baseline-shift", "clip", "clip-path", "clip-rule", "color", "color-interpolation",
    "color-interpolation-filters", "color-rendering", "cursor", "direction", "display", "dominant-baseline",
    "fill", "fill-opacity", "fill-rule", "filter", "flood-color", "flood-opacity", "font-family", "font-size",
    "font-size-adjust", "font-stretch", "font-style", "font-variant", "font-weight", "glyph-orientation-horizontal",
    "glyph-orientation-vertical", "image-rendering", "isolation", "letter-spacing", "lighting-color", "marker-end",
    "marker-mid", "marker-start", "mask", "mask-type", "mix-blend-mode", "opacity", "overflow", "paint-order",
    "pointer-events", "shape-rendering", "stop-color", "stop-opacity", "stroke", "stroke-dasharray",
    "stroke-dashoffset", "stroke-linecap", "stroke-linejoin", "stroke-miterlimit", "stroke-opacity",
    "stroke-width", "text-anchor", "text-decoration", "text-rendering", "transform-origin", "unicode-bidi",
    "visibility", "word-spacing", "writing-mode", "font-kerning", "text-decoration-line",
];

/// Properties that may be set from CSS but are not presentation attributes (geometry properties etc.).
const CSS_ONLY_OK: &[&str] = &["font", "marker", "transform", "x", "y", "cx", "cy", "r", "rx", "ry", "width", "height", "d"];

pub type Props = Vec<(String, String)>;

fn set(p: &mut Props, n: &str, v: &str) {
    if let Some(e) = p.iter_mut().find(|e| e.0 == n) {
        e.1 = v.to_string();
    } else {
        p.push((n.to_string(), v.to_string()));
    }
}

fn apply_decl(p: &mut Props, d: &Declaration) {
    match d.name.as_str() {
        "font" => {
            // font: [style] [variant] [weight] size[/line-height] family
            let v = d.value.trim();
            let mut rest = v;
            let mut style = "normal";
            let mut weight = "normal";
            loop {
                let t = rest.split_whitespace().next().unwrap_or("");
                match t {
                    "italic" | "oblique" => style = if t == "italic" { "italic" } else { "oblique" },
                    "bold" | "bolder" | "lighter" | "100" | "200" | "300" | "400" | "500" | "600" | "700" | "800"
                    | "900" => weight = t,
                    "normal" | "small-caps" => {}
                    _ => break,
                }
                rest = rest.trim_start()[t.len()..].trim_start();
            }
            let Some((size, fam)) = rest.split_once(char::is_whitespace) else { return };
            let size = size.split('/').next().unwrap_or(size);
            set(p, "font-style", style);
            set(p, "font-weight", weight);
            set(p, "font-size", size);
            set(p, "font-family", fam.trim());
        }
        "marker" => {
            for n in ["marker-start", "marker-mid", "marker-end"] {
                set(p, n, &d.value);
            }
        }
        n => {
            if PRESENTATION.contains(&n) || CSS_ONLY_OK.contains(&n) {
                set(p, n, &d.value);
            }
        }
    }
}

/// Resolve the cascade for every element; index = node id.
pub fn cascade(doc: &Document) -> Vec<Props> {
    let mut sheet = Stylesheet::default();
    for (i, n) in doc.nodes.iter().enumerate() {
        if n.is_svg("style") {
            let ty = n.attr("type").unwrap_or("text/css").trim();
            if !(ty.is_empty() || ty.eq_ignore_ascii_case("text/css")) {
                continue;
            }
            let mut text = String::new();
            for &c in &doc.nodes[i].children {
                if let Some(t) = doc.nodes[c].text() {
                    text.push_str(t);
                }
            }
            sheet.parse_into(&text);
        }
    }
    let mut rules: Vec<&css::Rule> = sheet.rules.iter().collect();
    rules.sort_by(|a, b| a.selector.specificity.cmp(&b.selector.specificity).then(a.order.cmp(&b.order)));
    let mut out = Vec::with_capacity(doc.nodes.len());
    for (i, n) in doc.nodes.iter().enumerate() {
        let mut p: Props = Vec::new();
        if !matches!(n.kind, Kind::Element { .. }) {
            out.push(p);
            continue;
        }
        for a in &n.attrs {
            if a.ns == Ns::None && PRESENTATION.contains(&a.name.as_str()) {
                set(&mut p, &a.name, a.value.trim());
            }
            if a.ns == Ns::Xml && a.name == "space" {
                set(&mut p, "xml:space", a.value.trim());
            }
        }
        let matched: Vec<&css::Rule> = rules.iter().copied().filter(|r| r.selector.matches(doc, i)).collect();
        let inline = n.attr("style").map(css::parse_declarations).unwrap_or_default();
        for r in &matched {
            for d in r.decls.iter().filter(|d| !d.important) {
                apply_decl(&mut p, d);
            }
        }
        for d in inline.iter().filter(|d| !d.important) {
            apply_decl(&mut p, d);
        }
        for r in &matched {
            for d in r.decls.iter().filter(|d| d.important) {
                apply_decl(&mut p, d);
            }
        }
        for d in inline.iter().filter(|d| d.important) {
            apply_decl(&mut p, d);
        }
        out.push(p);
    }
    out
}

pub fn get<'a>(p: &'a Props, n: &str) -> Option<&'a str> {
    p.iter().find(|e| e.0 == n).map(|e| e.1.as_str())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Unit {
    None,
    Px,
    Em,
    Ex,
    In,
    Cm,
    Mm,
    Pt,
    Pc,
    Percent,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Length {
    pub v: f64,
    pub unit: Unit,
}

impl Length {
    pub const ZERO: Length = Length { v: 0.0, unit: Unit::None };
    pub const fn px(v: f64) -> Length {
        Length { v, unit: Unit::None }
    }
    pub const fn pct(v: f64) -> Length {
        Length { v, unit: Unit::Percent }
    }
}

/// Parse one length, consuming from `s[*i..]`.
pub fn length_at(s: &[u8], i: &mut usize) -> Option<Length> {
    let v = number(s, i)?;
    let rest = &s[*i..];
    let (unit, n) = if rest.starts_with(b"%") {
        (Unit::Percent, 1)
    } else {
        let mut k = 0;
        while k < rest.len() && rest[k].is_ascii_alphabetic() {
            k += 1;
        }
        let u = core::str::from_utf8(&rest[..k]).ok()?.to_ascii_lowercase();
        let unit = match u.as_str() {
            "" => Unit::None,
            "px" => Unit::Px,
            "em" => Unit::Em,
            "ex" => Unit::Ex,
            "in" => Unit::In,
            "cm" => Unit::Cm,
            "mm" => Unit::Mm,
            "pt" => Unit::Pt,
            "pc" => Unit::Pc,
            _ => return None,
        };
        (unit, k)
    };
    *i += n;
    Some(Length { v, unit })
}

pub fn parse_length(s: &str) -> Option<Length> {
    let b = s.trim().as_bytes();
    let mut i = 0;
    let l = length_at(b, &mut i)?;
    if i != b.len() {
        return None;
    }
    Some(l)
}

pub fn parse_length_list(s: &str) -> Option<Vec<Length>> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    loop {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b',') {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        out.push(length_at(b, &mut i)?);
        if i < b.len() && !(b[i].is_ascii_whitespace() || b[i] == b',') {
            return None;
        }
    }
    Some(out)
}

/// Which viewport dimension a percentage refers to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Axis {
    X,
    Y,
    Diag,
}

/// Absolute units → px, font-relative against `font_size`, percentages against the viewport.
pub fn resolve(l: Length, axis: Axis, vw: f64, vh: f64, font_size: f64) -> f64 {
    match l.unit {
        Unit::None | Unit::Px => l.v,
        Unit::Em => l.v * font_size,
        Unit::Ex => l.v * font_size / 2.0,
        Unit::In => l.v * 96.0,
        Unit::Cm => l.v * 96.0 / 2.54,
        Unit::Mm => l.v * 96.0 / 25.4,
        Unit::Pt => l.v * 4.0 / 3.0,
        Unit::Pc => l.v * 16.0,
        Unit::Percent => {
            let base = match axis {
                Axis::X => vw,
                Axis::Y => vh,
                Axis::Diag => crate::fmath::sqrt((vw * vw + vh * vh) / 2.0),
            };
            l.v / 100.0 * base
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum Paint {
    None,
    Color(Color),
    CurrentColor,
    Url(String, Option<alloc::boxed::Box<Paint>>),
    ContextFill,
    ContextStroke,
}

pub fn parse_paint(s: &str) -> Option<Paint> {
    let s = s.trim();
    if s == "none" {
        return Some(Paint::None);
    }
    if s == "context-fill" {
        return Some(Paint::ContextFill);
    }
    if s == "context-stroke" {
        return Some(Paint::ContextStroke);
    }
    if let Some(r) = s.strip_prefix("url(") {
        let close = r.find(')')?;
        let inner = r[..close].trim().trim_matches(|c| c == '"' || c == '\'');
        let id = inner.strip_prefix('#')?.to_string();
        let rest = r[close + 1..].trim();
        let fb = if rest.is_empty() { None } else { Some(alloc::boxed::Box::new(parse_paint(rest)?)) };
        return Some(Paint::Url(id, fb));
    }
    match color::parse(s)? {
        ParsedColor::Color(c) => Some(Paint::Color(c)),
        ParsedColor::CurrentColor => Some(Paint::CurrentColor),
    }
}

/// `url(#id)` → `id`.
pub fn parse_func_iri(s: &str) -> Option<String> {
    let r = s.trim().strip_prefix("url(")?;
    let close = r.find(')')?;
    let inner = r[..close].trim().trim_matches(|c| c == '"' || c == '\'');
    Some(inner.strip_prefix('#')?.to_string())
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Anchor {
    Start,
    Middle,
    End,
}

/// Inherited properties (computed), plus the non-inherited ones the renderer reads from the element itself.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    pub fill: Paint,
    pub fill_opacity: f64,
    pub fill_rule: FillRule,
    pub stroke: Paint,
    pub stroke_width: Length,
    pub stroke_opacity: f64,
    pub cap: Cap,
    pub join: Join,
    pub miter_limit: f64,
    pub dasharray: Option<Vec<Length>>,
    pub dashoffset: Length,
    pub color: Color,
    pub visible: bool,
    pub clip_rule: FillRule,
    pub font_family: String,
    pub font_size: f64,
    pub font_weight: u16,
    pub italic: bool,
    pub anchor: Anchor,
    pub letter_spacing: f64,
    pub word_spacing: f64,
    pub aa: bool,
    pub text_aa: bool,
    pub stroke_first: bool,
    pub markers_first: bool,
    pub marker_start: Option<String>,
    pub marker_mid: Option<String>,
    pub marker_end: Option<String>,
    pub preserve_space: bool,
    pub kerning: bool,
    pub image_smooth: bool,
    pub linear_rgb_interp: bool,
    pub rtl: bool,
    // ---- non-inherited (reset per element) ----
    pub opacity: f64,
    pub display_none: bool,
    pub stop_color: Color,
    pub stop_opacity: f64,
    pub overflow_visible: bool,
    pub mask_alpha: bool,
    pub decoration: u8,
    pub baseline_shift: Option<String>,
    pub dominant_baseline: Option<String>,
}

pub const DECOR_UNDERLINE: u8 = 1;
pub const DECOR_OVERLINE: u8 = 2;
pub const DECOR_LINE_THROUGH: u8 = 4;

impl Default for Style {
    fn default() -> Self {
        Style {
            fill: Paint::Color(Color::BLACK),
            fill_opacity: 1.0,
            fill_rule: FillRule::NonZero,
            stroke: Paint::None,
            stroke_width: Length::px(1.0),
            stroke_opacity: 1.0,
            cap: Cap::Butt,
            join: Join::Miter,
            miter_limit: 4.0,
            dasharray: None,
            dashoffset: Length::ZERO,
            color: Color::BLACK,
            visible: true,
            clip_rule: FillRule::NonZero,
            font_family: String::from("serif"),
            font_size: 16.0,
            font_weight: 400,
            italic: false,
            anchor: Anchor::Start,
            letter_spacing: 0.0,
            word_spacing: 0.0,
            aa: true,
            text_aa: true,
            stroke_first: false,
            markers_first: false,
            marker_start: None,
            marker_mid: None,
            marker_end: None,
            preserve_space: false,
            kerning: true,
            image_smooth: true,
            linear_rgb_interp: false,
            rtl: false,
            opacity: 1.0,
            display_none: false,
            stop_color: Color::BLACK,
            stop_opacity: 1.0,
            overflow_visible: false,
            mask_alpha: false,
            decoration: 0,
            baseline_shift: None,
            dominant_baseline: None,
        }
    }
}

fn opacity_value(s: &str) -> Option<f64> {
    let s = s.trim();
    let v = if let Some(p) = s.strip_suffix('%') { p.trim().parse::<f64>().ok()? / 100.0 } else { s.parse::<f64>().ok()? };
    if !v.is_finite() {
        return None;
    }
    Some(v.clamp(0.0, 1.0))
}

fn font_size_value(s: &str, parent: f64) -> Option<f64> {
    let s = s.trim();
    let kw = match s {
        "xx-small" => Some(9.0),
        "x-small" => Some(10.0),
        "small" => Some(13.0),
        "medium" => Some(16.0),
        "large" => Some(18.0),
        "x-large" => Some(24.0),
        "xx-large" => Some(32.0),
        "xxx-large" => Some(48.0),
        "larger" => Some(parent * 1.2),
        "smaller" => Some(parent / 1.2),
        _ => None,
    };
    if kw.is_some() {
        return kw;
    }
    let l = parse_length(s)?;
    if l.v < 0.0 {
        return None;
    }
    Some(match l.unit {
        Unit::Percent => l.v / 100.0 * parent,
        Unit::Em => l.v * parent,
        Unit::Ex => l.v * parent / 2.0,
        _ => resolve(l, Axis::X, 0.0, 0.0, parent),
    })
}

impl Style {
    /// Compute the style of an element from its parent's and its own specified properties.
    pub fn compute(parent: &Style, p: &Props) -> Style {
        let mut s = parent.clone();
        // Reset the non-inherited properties.
        s.opacity = 1.0;
        s.display_none = false;
        s.stop_color = Color::BLACK;
        s.stop_opacity = 1.0;
        s.overflow_visible = false;
        s.mask_alpha = false;
        s.decoration = 0;
        s.baseline_shift = None;
        s.dominant_baseline = None;
        // `color` first: currentColor in other properties refers to this element's color.
        for (n, v) in p.iter() {
            let v = v.trim();
            let inherit = v == "inherit";
            match n.as_str() {
                "color" => {
                    if inherit {
                        s.color = parent.color;
                    } else if let Some(c) = color::parse_color(v) {
                        s.color = c;
                    } else if v == "currentColor" {
                        s.color = parent.color;
                    }
                }
                "font-size" => {
                    if inherit {
                        s.font_size = parent.font_size;
                    } else if let Some(f) = font_size_value(v, parent.font_size) {
                        s.font_size = f;
                    }
                }
                _ => {}
            }
        }
        for (n, v) in p.iter() {
            let v = v.trim();
            if v == "inherit" {
                s.inherit_one(n, parent);
                continue;
            }
            match n.as_str() {
                "fill" => {
                    if let Some(pt) = parse_paint(v) {
                        s.fill = pt;
                    }
                }
                "stroke" => {
                    if let Some(pt) = parse_paint(v) {
                        s.stroke = pt;
                    }
                }
                "fill-opacity" => {
                    if let Some(o) = opacity_value(v) {
                        s.fill_opacity = o;
                    }
                }
                "stroke-opacity" => {
                    if let Some(o) = opacity_value(v) {
                        s.stroke_opacity = o;
                    }
                }
                "opacity" => {
                    if let Some(o) = opacity_value(v) {
                        s.opacity = o;
                    }
                }
                "stop-opacity" => {
                    if let Some(o) = opacity_value(v) {
                        s.stop_opacity = o;
                    }
                }
                "stop-color" => {
                    match color::parse(v) {
                        Some(ParsedColor::Color(c)) => s.stop_color = c,
                        Some(ParsedColor::CurrentColor) => s.stop_color = s.color,
                        None => {}
                    }
                }
                "fill-rule" => match v {
                    "evenodd" => s.fill_rule = FillRule::EvenOdd,
                    "nonzero" => s.fill_rule = FillRule::NonZero,
                    _ => {}
                },
                "clip-rule" => match v {
                    "evenodd" => s.clip_rule = FillRule::EvenOdd,
                    "nonzero" => s.clip_rule = FillRule::NonZero,
                    _ => {}
                },
                "stroke-width" => {
                    if let Some(l) = parse_length(v) {
                        if l.v >= 0.0 {
                            s.stroke_width = l;
                        }
                    }
                }
                "stroke-linecap" => match v {
                    "butt" => s.cap = Cap::Butt,
                    "round" => s.cap = Cap::Round,
                    "square" => s.cap = Cap::Square,
                    _ => {}
                },
                "stroke-linejoin" => match v {
                    "miter" => s.join = Join::Miter,
                    "miter-clip" => s.join = Join::MiterClip,
                    "round" => s.join = Join::Round,
                    "bevel" => s.join = Join::Bevel,
                    "arcs" => s.join = Join::Arcs,
                    _ => {}
                },
                "stroke-miterlimit" => {
                    if let Ok(m) = v.parse::<f64>() {
                        if m >= 1.0 {
                            s.miter_limit = m;
                        }
                    }
                }
                "stroke-dasharray" => {
                    if v == "none" {
                        s.dasharray = None;
                    } else if let Some(l) = parse_length_list(v) {
                        s.dasharray = Some(l);
                    }
                }
                "stroke-dashoffset" => {
                    if let Some(l) = parse_length(v) {
                        s.dashoffset = l;
                    }
                }
                "visibility" => match v {
                    "visible" => s.visible = true,
                    "hidden" | "collapse" => s.visible = false,
                    _ => {}
                },
                "display" => s.display_none = v == "none",
                "overflow" => s.overflow_visible = v == "visible" || v == "auto",
                "mask-type" => s.mask_alpha = v == "alpha",
                "font-family" => s.font_family = v.to_string(),
                "font-weight" => {
                    s.font_weight = match v {
                        "normal" => 400,
                        "bold" => 700,
                        "bolder" => {
                            let w = parent.font_weight;
                            if w < 350 {
                                400
                            } else if w < 550 {
                                700
                            } else {
                                900
                            }
                        }
                        "lighter" => {
                            let w = parent.font_weight;
                            if w < 550 {
                                100
                            } else if w < 750 {
                                400
                            } else {
                                700
                            }
                        }
                        _ => v.parse::<u16>().ok().filter(|w| (1..=1000).contains(w)).unwrap_or(s.font_weight),
                    }
                }
                "font-style" => s.italic = v == "italic" || v.starts_with("oblique"),
                "text-anchor" => match v {
                    "start" => s.anchor = Anchor::Start,
                    "middle" => s.anchor = Anchor::Middle,
                    "end" => s.anchor = Anchor::End,
                    _ => {}
                },
                "letter-spacing" => {
                    if v == "normal" {
                        s.letter_spacing = 0.0;
                    } else if let Some(l) = parse_length(v) {
                        s.letter_spacing = resolve(l, Axis::X, 0.0, 0.0, s.font_size);
                    }
                }
                "word-spacing" => {
                    if v == "normal" {
                        s.word_spacing = 0.0;
                    } else if let Some(l) = parse_length(v) {
                        s.word_spacing = resolve(l, Axis::X, 0.0, 0.0, s.font_size);
                    }
                }
                "shape-rendering" => s.aa = !(v == "crispEdges" || v == "optimizeSpeed"),
                "text-rendering" => s.text_aa = v != "optimizeSpeed",
                "image-rendering" => s.image_smooth = !(v == "optimizeSpeed" || v == "pixelated" || v == "crisp-edges"),
                "color-interpolation" => s.linear_rgb_interp = v == "linearRGB",
                "paint-order" => {
                    let words: Vec<&str> = v.split_whitespace().collect();
                    if words == ["normal"] || words.is_empty() {
                        s.stroke_first = false;
                        s.markers_first = false;
                    } else {
                        // Complete the order with the missing keywords in their default order.
                        let mut order: Vec<&str> = Vec::new();
                        for w in &words {
                            if matches!(*w, "fill" | "stroke" | "markers") && !order.contains(w) {
                                order.push(w);
                            }
                        }
                        for w in ["fill", "stroke", "markers"] {
                            if !order.contains(&w) {
                                order.push(w);
                            }
                        }
                        let pos = |k: &str| order.iter().position(|w| *w == k).unwrap();
                        s.stroke_first = pos("stroke") < pos("fill");
                        s.markers_first = pos("markers") < pos("fill");
                    }
                }
                "marker-start" => s.marker_start = parse_func_iri(v),
                "marker-mid" => s.marker_mid = parse_func_iri(v),
                "marker-end" => s.marker_end = parse_func_iri(v),
                "xml:space" => s.preserve_space = v == "preserve",
                "font-kerning" => s.kerning = v != "none",
                "direction" => s.rtl = v == "rtl",
                "text-decoration" | "text-decoration-line" => {
                    let mut d = 0;
                    for w in v.split_whitespace() {
                        match w {
                            "underline" => d |= DECOR_UNDERLINE,
                            "overline" => d |= DECOR_OVERLINE,
                            "line-through" => d |= DECOR_LINE_THROUGH,
                            _ => {}
                        }
                    }
                    s.decoration = d;
                }
                "baseline-shift" => s.baseline_shift = Some(v.to_string()),
                "dominant-baseline" | "alignment-baseline" => s.dominant_baseline = Some(v.to_string()),
                _ => {}
            }
        }
        s
    }

    fn inherit_one(&mut self, n: &str, p: &Style) {
        match n {
            "fill" => self.fill = p.fill.clone(),
            "stroke" => self.stroke = p.stroke.clone(),
            "opacity" => self.opacity = p.opacity,
            "stop-color" => self.stop_color = p.stop_color,
            "stop-opacity" => self.stop_opacity = p.stop_opacity,
            "overflow" => self.overflow_visible = p.overflow_visible,
            "display" => self.display_none = p.display_none,
            _ => {} // inherited properties already hold the parent's value
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cascade_order() {
        let doc = Document::parse(
            br##"<svg xmlns="http://www.w3.org/2000/svg"><style>#r { fill: blue } rect { fill: green; stroke: red !important }</style>
            <rect id="r" fill="yellow" stroke="black" style="fill: purple; stroke: white"/><rect id="s" fill="yellow"/></svg>"##,
        )
        .unwrap();
        let p = cascade(&doc);
        let r = doc.by_id("r").unwrap();
        let s = doc.by_id("s").unwrap();
        assert_eq!(get(&p[r], "fill"), Some("purple"));
        assert_eq!(get(&p[r], "stroke"), Some("red"));
        assert_eq!(get(&p[s], "fill"), Some("green"));
    }

    #[test]
    fn computed_values() {
        let root = Style::default();
        let mut p: Props = Vec::new();
        set(&mut p, "font-size", "2em");
        set(&mut p, "fill", "url(#g) red");
        set(&mut p, "color", "lime");
        set(&mut p, "stroke", "currentColor");
        set(&mut p, "opacity", "50%");
        set(&mut p, "paint-order", "stroke");
        let s = Style::compute(&root, &p);
        assert_eq!(s.font_size, 32.0);
        assert_eq!(s.fill, Paint::Url("g".into(), Some(alloc::boxed::Box::new(Paint::Color(Color::rgb(255, 0, 0))))));
        assert_eq!(s.stroke, Paint::CurrentColor);
        assert_eq!(s.opacity, 0.5);
        assert!(s.stroke_first);
        let child = Style::compute(&s, &Vec::new());
        assert_eq!(child.opacity, 1.0);
        assert_eq!(child.font_size, 32.0);
        assert_eq!(parse_length("1in").map(|l| resolve(l, Axis::X, 0.0, 0.0, 16.0)), Some(96.0));
        assert_eq!(parse_length("10%").map(|l| resolve(l, Axis::Y, 50.0, 200.0, 16.0)), Some(20.0));
        assert!(parse_length("10q").is_none());
    }
}
