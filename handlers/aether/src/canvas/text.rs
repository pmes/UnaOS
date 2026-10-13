//! Canvas text (HTML §4.12.5.1.4 and §4.12.5.1.12): the `font` attribute's CSS shorthand parse and its
//! serialization (line-height dropped), the text preparation algorithm (ASCII whitespace → U+0020,
//! alignment, baselines, `maxWidth` squeeze), glyph outlines from font_core turned into canvas paths (so
//! `fillText`/`strokeText` go through the same fill/stroke pipeline as every other shape), and TextMetrics.
//! Shaping is Aether's own (`fonts::shape`, font_core's GSUB/GPOS shaper over the face stack).

use super::geom::{Matrix, Path};
use crate::fonts::{self, FontSel};
use font_core::PathCmd;

#[derive(Clone, Debug, PartialEq)]
pub struct FontSpec {
    /// 0 normal, 1 italic, 2 oblique.
    pub style: u8,
    pub small_caps: bool,
    pub weight: u16,
    /// font-stretch percent.
    pub stretch: u16,
    pub size: f64,
    pub families: Vec<fonts::Family>,
}

impl Default for FontSpec {
    fn default() -> Self {
        FontSpec { style: 0, small_caps: false, weight: 400, stretch: 100, size: 10.0, families: vec![fonts::Family::Generic(fonts::Generic::SansSerif)] }
    }
}

const STRETCH: [(&str, u16); 9] = [
    ("ultra-condensed", 50),
    ("extra-condensed", 62),
    ("condensed", 75),
    ("semi-condensed", 87),
    ("normal", 100),
    ("semi-expanded", 112),
    ("expanded", 125),
    ("extra-expanded", 150),
    ("ultra-expanded", 200),
];

pub fn stretch_name(p: u16) -> &'static str {
    STRETCH.iter().find(|(_, v)| *v == p).map(|(n, _)| *n).unwrap_or("normal")
}

pub fn stretch_value(name: &str) -> Option<u16> {
    STRETCH.iter().find(|(n, _)| *n == name).map(|(_, v)| *v)
}

/// The size a `<font-size>` token names, in px (relative units against `base`, the canvas's font size —
/// the UA default 16px for a canvas whose element style is not consulted).
fn font_size(tok: &str, base: f64) -> Option<f64> {
    let t = tok.to_ascii_lowercase();
    let kw = match t.as_str() {
        "xx-small" => Some(9.0),
        "x-small" => Some(10.0),
        "small" => Some(13.0),
        "medium" => Some(16.0),
        "large" => Some(18.0),
        "x-large" => Some(24.0),
        "xx-large" => Some(32.0),
        "xxx-large" => Some(48.0),
        "larger" => Some(base * 1.2),
        "smaller" => Some(base / 1.2),
        _ => None,
    };
    if kw.is_some() {
        return kw;
    }
    let split = t.find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '+' || c == '-' || c == 'e')).unwrap_or(t.len());
    // "1e3px": the exponent is part of the number only when followed by a digit; keep it simple.
    let (num, unit) = t.split_at(split);
    let v: f64 = num.parse().ok()?;
    if !v.is_finite() || v < 0.0 {
        return None;
    }
    let px = match unit {
        "px" => v,
        "pt" => v * 4.0 / 3.0,
        "pc" => v * 16.0,
        "in" => v * 96.0,
        "cm" => v * 96.0 / 2.54,
        "mm" => v * 96.0 / 25.4,
        "q" => v * 96.0 / 101.6,
        "em" | "rem" => v * base,
        "ex" => v * base * 0.5,
        "ch" => v * base * 0.5,
        "%" => v * base / 100.0,
        "" if v == 0.0 => 0.0,
        _ => return None,
    };
    Some(px)
}

/// Parse the canvas `font` attribute (CSS `font` shorthand; CSS-wide keywords and system fonts refused).
pub fn parse_font(s: &str) -> Option<FontSpec> {
    let s = s.trim_matches(|c: char| c.is_ascii_whitespace());
    // Tokens with their byte ranges.
    let mut toks: Vec<(usize, usize)> = Vec::new();
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let st = i;
        while i < b.len() && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i > st {
            toks.push((st, i));
        }
    }
    let mut spec = FontSpec::default();
    let (mut have_style, mut have_variant, mut have_weight, mut have_stretch) = (false, false, false, false);
    let mut k = 0;
    let mut pre = 0;
    let size_tok;
    loop {
        let (a, e) = *toks.get(k)?;
        let t = &s[a..e];
        let l = t.to_ascii_lowercase();
        // The size token, possibly carrying "/line-height".
        let size_part = l.split('/').next().unwrap_or("");
        if let Some(px) = font_size(size_part, 16.0) {
            spec.size = px;
            size_tok = k;
            break;
        }
        if pre >= 4 {
            return None;
        }
        pre += 1;
        match l.as_str() {
            "normal" => {}
            "italic" if !have_style => {
                spec.style = 1;
                have_style = true;
            }
            "oblique" if !have_style => {
                spec.style = 2;
                have_style = true;
            }
            "small-caps" if !have_variant => {
                spec.small_caps = true;
                have_variant = true;
            }
            "bold" if !have_weight => {
                spec.weight = 700;
                have_weight = true;
            }
            "bolder" if !have_weight => {
                spec.weight = 700;
                have_weight = true;
            }
            "lighter" if !have_weight => {
                spec.weight = 100;
                have_weight = true;
            }
            _ => {
                if let (false, Ok(n)) = (have_weight, l.parse::<f64>()) {
                    if !(1.0..=1000.0).contains(&n) {
                        return None;
                    }
                    spec.weight = n.round() as u16;
                    have_weight = true;
                } else if let (false, Some(v)) = (have_stretch, stretch_value(&l)) {
                    spec.stretch = v;
                    have_stretch = true;
                } else {
                    return None;
                }
            }
        }
        k += 1;
    }
    // Skip a line-height: "12px/2", "12px / 2", "12px /2".
    let (_, size_end) = toks[size_tok];
    let mut rest_start = size_end;
    let size_text = &s[toks[size_tok].0..size_end];
    let mut next = size_tok + 1;
    if let Some(slash) = size_text.find('/') {
        if slash + 1 == size_text.len() {
            // "12px/ 2"
            next += 1;
            rest_start = toks.get(size_tok + 1).map(|t| t.1)?;
        }
    } else if let Some(&(a, e)) = toks.get(next) {
        let t = &s[a..e];
        if t == "/" {
            rest_start = toks.get(next + 1).map(|t| t.1)?;
            next += 2;
        } else if t.starts_with('/') {
            rest_start = e;
            next += 1;
        }
    }
    let _ = next;
    let family = s[rest_start..].trim();
    if family.is_empty() {
        return None;
    }
    spec.families = fonts::parse_family_list(family)?;
    Some(spec)
}

fn fmt_px(v: f64) -> String {
    // CSS serializes numbers with up to six significant digits.
    let mut s = format!("{:.6}", v);
    if v.abs() >= 1.0 {
        let digits = (v.abs().log10().floor() as i32 + 1).max(1);
        let decimals = (6 - digits).max(0) as usize;
        s = format!("{:.*}", decimals, v);
    }
    if s.contains('.') {
        s = s.trim_end_matches('0').trim_end_matches('.').to_string();
    }
    s
}

/// The serialized form of a font (CSS shorthand, line-height omitted), as Chromium returns `ctx.font`.
pub fn serialize_font(f: &FontSpec) -> String {
    let mut parts: Vec<String> = Vec::new();
    match f.style {
        1 => parts.push("italic".into()),
        2 => parts.push("oblique".into()),
        _ => {}
    }
    if f.small_caps {
        parts.push("small-caps".into());
    }
    match f.weight {
        400 => {}
        700 => parts.push("bold".into()),
        w => parts.push(w.to_string()),
    }
    if f.stretch != 100 {
        parts.push(stretch_name(f.stretch).into());
    }
    parts.push(format!("{}px", fmt_px(f.size)));
    parts.push(fonts::serialize_family_list(&f.families));
    parts.join(" ")
}

impl FontSpec {
    pub fn sel(&self, stretch_override: Option<u16>) -> FontSel {
        FontSel {
            family: fonts::intern_family_list(self.families.clone()),
            weight: self.weight,
            italic: self.style != 0,
            stretch: stretch_override.unwrap_or(self.stretch),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    End,
    Left,
    Right,
    Center,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Baseline {
    Top,
    Hanging,
    Middle,
    Alphabetic,
    Ideographic,
    Bottom,
}

/// Options of one text operation, from the drawing state.
pub struct TextOpts<'a> {
    pub font: &'a FontSpec,
    pub align: Align,
    pub baseline: Baseline,
    pub rtl: bool,
    pub letter_spacing: f64,
    pub word_spacing: f64,
    pub kerning: bool,
    pub stretch: Option<u16>,
}

/// One positioned glyph: face, glyph id, pen x and y offset in px (y down), relative to the text's start.
struct Placed {
    face: &'static fonts::Face,
    gid: u16,
    x: f64,
    y: f64,
}

pub struct Laid {
    glyphs: Vec<Placed>,
    pub width: f64,
    /// Font ascent/descent of the primary face in px (descent positive).
    pub ascent: f64,
    pub descent: f64,
    size: f64,
    synth_oblique: bool,
}

/// HTML "text preparation algorithm" steps 1–5: whitespace replacement, shaping, spacing.
pub fn lay_out(text: &str, o: &TextOpts) -> Laid {
    let prepared: String = text.chars().map(|c| if matches!(c, '\t' | '\n' | '\x0c' | '\r') { ' ' } else { c }).collect();
    let sel = o.font.sel(o.stretch);
    let size = o.font.size;
    let dir = if o.rtl { font_core::Direction::Rtl } else { font_core::Direction::Ltr };
    let ligatures = o.letter_spacing == 0.0;
    let shaped = fonts::shape::shape(&sel, &prepared, dir, ligatures);
    let mut glyphs = Vec::with_capacity(shaped.glyphs.len());
    let mut pen = 0.0f64;
    let bytes = prepared.as_bytes();
    let mut synth_oblique = false;
    for g in &shaped.glyphs {
        glyphs.push(Placed { face: g.face, gid: g.gid, x: pen + g.dx as f64 * size, y: -(g.dy as f64) * size });
        synth_oblique |= g.face.synth_oblique;
        pen += g.adv as f64 * size;
        if o.letter_spacing != 0.0 {
            pen += o.letter_spacing;
        }
        if o.word_spacing != 0.0 && bytes.get(g.cluster) == Some(&b' ') {
            pen += o.word_spacing;
        }
    }
    let _ = o.kerning;
    let (ascent, descent) = match fonts::face(&sel) {
        Some(f) => {
            let m = f.metrics();
            let s = size / m.units_per_em.max(1) as f64;
            (m.ascent as f64 * s, -(m.descent as f64) * s)
        }
        None => (size * 0.8, size * 0.2),
    };
    Laid { glyphs, width: pen, ascent, descent, size, synth_oblique }
}

/// The y-down offset from the line `textBaseline` names to the alphabetic baseline.
pub fn baseline_offset(b: Baseline, ascent: f64, descent: f64) -> f64 {
    match b {
        Baseline::Alphabetic => 0.0,
        Baseline::Top => ascent,
        Baseline::Hanging => ascent * 0.8,
        Baseline::Middle => (ascent - descent) / 2.0,
        Baseline::Ideographic | Baseline::Bottom => -descent,
    }
}

/// The x of the text's start relative to the anchor, before any maxWidth squeeze.
fn align_offset(align: Align, rtl: bool, width: f64) -> f64 {
    let align = match (align, rtl) {
        (Align::Start, false) | (Align::End, true) => Align::Left,
        (Align::Start, true) | (Align::End, false) => Align::Right,
        (a, _) => a,
    };
    match align {
        Align::Left => 0.0,
        Align::Right => -width,
        _ => -width / 2.0,
    }
}

/// The glyph outlines of a laid-out text as one path in the space `ctm` maps to (device space for drawing).
/// `None` when `maxWidth` forbids drawing (≤ 0 or NaN).
pub fn text_path(l: &Laid, o: &TextOpts, x: f64, y: f64, max_width: Option<f64>, ctm: &Matrix) -> Option<Path> {
    let mut sx = 1.0;
    if let Some(mw) = max_width {
        if mw.is_nan() || mw <= 0.0 {
            return None;
        }
        if mw.is_finite() && l.width > mw {
            sx = mw / l.width;
        }
    }
    let start = align_offset(o.align, o.rtl, l.width);
    let by = baseline_offset(o.baseline, l.ascent, l.descent);
    // anchor → squeeze → start of text on the baseline
    let base = ctm.mul(&Matrix::translate(x, y)).mul(&Matrix::scale(sx, 1.0)).mul(&Matrix::translate(start, by));
    let mut path = Path::new();
    for g in &l.glyphs {
        let Some(gp) = g.face.font.glyph_path(g.gid) else { continue };
        if gp.cmds.is_empty() {
            continue;
        }
        let upem = g.face.font.units_per_em.max(1) as f64;
        let s = l.size / upem;
        let mut m = base.mul(&Matrix::translate(g.x, g.y)).mul(&Matrix::scale(s, -s));
        if l.synth_oblique && g.face.synth_oblique {
            m = m.mul(&Matrix::new(1.0, 0.0, 0.25, 1.0, 0.0, 0.0));
        }
        for c in &gp.cmds {
            match *c {
                PathCmd::MoveTo(a, b) => path.move_to(&m, a as f64, b as f64),
                PathCmd::LineTo(a, b) => path.line_to(&m, a as f64, b as f64),
                PathCmd::QuadTo(a, b, c2, d) => path.quadratic_curve_to(&m, a as f64, b as f64, c2 as f64, d as f64),
                PathCmd::CubicTo(a, b, c2, d, e, f) => {
                    path.bezier_curve_to(&m, a as f64, b as f64, c2 as f64, d as f64, e as f64, f as f64)
                }
                PathCmd::Close => path.close_path(),
            }
        }
    }
    Some(path)
}

/// TextMetrics (HTML §4.12.5.1.12): every member, in CSS px, relative to the alignment point and the line
/// `textBaseline` names.
#[derive(Clone, Copy, Debug, Default)]
pub struct Metrics {
    pub width: f64,
    pub actual_left: f64,
    pub actual_right: f64,
    pub font_ascent: f64,
    pub font_descent: f64,
    pub actual_ascent: f64,
    pub actual_descent: f64,
    pub em_ascent: f64,
    pub em_descent: f64,
    pub hanging: f64,
    pub alphabetic: f64,
    pub ideographic: f64,
}

pub fn measure(text: &str, o: &TextOpts) -> Metrics {
    let l = lay_out(text, o);
    let start = align_offset(o.align, o.rtl, l.width);
    let by = baseline_offset(o.baseline, l.ascent, l.descent);
    let (mut x0, mut x1, mut y0, mut y1) = (f64::INFINITY, f64::NEG_INFINITY, f64::INFINITY, f64::NEG_INFINITY);
    for g in &l.glyphs {
        let Some(gp) = g.face.font.glyph_path(g.gid) else { continue };
        if gp.cmds.is_empty() {
            continue;
        }
        let s = l.size / g.face.font.units_per_em.max(1) as f64;
        x0 = x0.min(start + g.x + gp.x_min as f64 * s);
        x1 = x1.max(start + g.x + gp.x_max as f64 * s);
        // y down, relative to the alphabetic baseline
        y0 = y0.min(g.y - gp.y_max as f64 * s);
        y1 = y1.max(g.y - gp.y_min as f64 * s);
    }
    let (al, ar, aa, ad) = if x0.is_finite() {
        (-x0, x1, -(y0 + by), y1 + by)
    } else {
        (-start, start, 0.0, 0.0)
    };
    let off = |b: Baseline| baseline_offset(b, l.ascent, l.descent);
    Metrics {
        width: l.width,
        actual_left: al,
        actual_right: ar,
        font_ascent: l.ascent - by,
        font_descent: l.descent + by,
        actual_ascent: aa,
        actual_descent: ad,
        em_ascent: l.ascent - by,
        em_descent: l.descent + by,
        hanging: off(Baseline::Hanging) - by,
        alphabetic: off(Baseline::Alphabetic) - by,
        ideographic: off(Baseline::Ideographic) - by,
    }
}
