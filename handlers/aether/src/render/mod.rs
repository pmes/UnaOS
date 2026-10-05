use crate::fonts::lines::Advancer;
use crate::fonts::{Face as Font, FontSel};
use crate::layout::LayoutTree;
use taffy::prelude::*;

/// Inherited paint state carried down the box tree.
#[derive(Clone, Copy)]
struct Inherited {
    color: (u8, u8, u8),
    font_size: f32,
    /// Computed font-weight.
    weight: u16,
    italic: bool,
    /// font-stretch, percent.
    stretch: u16,
    word_spacing: f32,
    /// direction: rtl
    rtl: bool,
    /// The box whose `text-shadow` is in effect (inherited; `PaintStyle::text_shadows` holds the layers).
    text_shadow: Option<NodeId>,
    line_height: f32, // multiplier of font size
    underline: bool,
    nowrap: bool,
    white_space: u8,
    word_break: u8,
    overflow_wrap: u8,
    letter_spacing: f32,
    line_through: bool,
    /// vertical-align super/sub: paint-time baseline shift in px.
    shift_y: f32,
    /// list-style-type in effect (layout::PaintStyle::list_style codes).
    list_style: u8,
    family: u16, // fonts::family_list id
    /// The image-replacement idiom: this subtree's text is off-box, but
    /// its boxes and backgrounds still paint.
    text_hidden: bool,
    text_transform: u8, // 0 = none, 1 = uppercase, 2 = lowercase, 3 = capitalize
    /// Content box (screen space: x, y, w, h) of an enclosing button-like
    /// control. A `<button>`'s label is an ordinary text child, so the box it
    /// must be centred in is only known from the ancestor.
    center_box: Option<(f32, f32, f32, f32)>,
}

use crate::layout::default_font_size;

impl Inherited {
    fn sel(&self) -> FontSel {
        FontSel { family: self.family, weight: self.weight, italic: self.italic, stretch: self.stretch }
    }
    /// The measurer/shaper of this text: face stack, size, spacing, direction.
    fn advancer(&self) -> Advancer {
        Advancer::new(self.sel(), self.font_size, self.letter_spacing).with_word_spacing(self.word_spacing).with_dir(self.rtl)
    }
    fn shadows<'a>(&self, layout: &'a LayoutTree) -> &'a [effects::Shadow] {
        self.text_shadow
            .and_then(|n| layout.paint_map.get(&n))
            .and_then(|p| p.text_shadows.as_deref())
            .unwrap_or(&[])
    }
}

/// The sans-serif selection UI chrome text (media controls) is drawn in.
fn ui_sel() -> FontSel {
    FontSel::new(crate::fonts::SANS, 400, false)
}

/// Screen-space clip rect (x0, y0, x1, y1), already scroll-adjusted.
type Clip = (f32, f32, f32, f32);

mod boxpaint;
pub(crate) mod effects;

fn in_clip(x: u32, y: u32, clip: Clip) -> bool {
    let (x0, y0, x1, y1) = clip;
    (x as f32) >= x0 && (x as f32) < x1 && (y as f32) >= y0 && (y as f32) < y1
}

fn in_damage(x: u32, y: u32, damage_rects: &[(u32, u32, u32, u32)]) -> bool {
    damage_rects
        .iter()
        .any(|&(dx, dy, dw, dh)| x >= dx && x < dx + dw && y >= dy && y < dy + dh)
}

fn put_px(surface: &mut [u8], width: u32, x: u32, y: u32, (r, g, b): (u8, u8, u8)) {
    let idx = ((y * width + x) * 4) as usize;
    if idx + 3 < surface.len() {
        surface[idx] = b;
        surface[idx + 1] = g;
        surface[idx + 2] = r;
        surface[idx + 3] = 255;
    }
}

fn blend_px(surface: &mut [u8], width: u32, x: u32, y: u32, (r, g, b): (u8, u8, u8), alpha: u8) {
    let idx = ((y * width + x) * 4) as usize;
    if idx + 3 < surface.len() {
        let a = alpha as u32;
        let inv = 255 - a;
        surface[idx] = ((b as u32 * a + surface[idx] as u32 * inv) / 255) as u8;
        surface[idx + 1] = ((g as u32 * a + surface[idx + 1] as u32 * inv) / 255) as u8;
        surface[idx + 2] = ((r as u32 * a + surface[idx + 2] as u32 * inv) / 255) as u8;
        surface[idx + 3] = 255;
    }
}

/// Draws `text` shaped (font_core: bidi, fallback, kerning, ligatures) with its pen at
/// (`origin_x`, `baseline_y`), glyphs at quarter-pixel x phases through Skia's A8 pre-blend for `color`
/// (fonts::raster). Returns the advance drawn.
#[allow(clippy::too_many_arguments)]
fn draw_shaped(
    text: &str, adv: &Advancer, origin_x: f32, baseline_y: f32, color: (u8, u8, u8),
    surface: &mut [u8], width: u32, height: u32, damage_rects: &[(u32, u32, u32, u32)], clip: Clip,
) -> f32 {
    let lut = crate::fonts::raster::preblend(color);
    let placed = adv.place(text);
    let mut end = 0.0f32;
    for (g, x, dy) in &placed {
        end = end.max(*x - g.dx * adv.size + g.adv * adv.size);
        crate::fonts::raster::draw_glyph(g.face, g.gid, adv.size, origin_x + x, baseline_y + dy, &lut, &mut |px, py, a| {
            if px < 0 || py < 0 {
                return;
            }
            let (px, py) = (px as u32, py as u32);
            if px < width && py < height && in_damage(px, py, damage_rects) && in_clip(px, py, clip) {
                blend_px(surface, width, px, py, color, a);
            }
        });
    }
    adv.str(text).max(end)
}

/// css-text-decor-3 §4: each shadow layer is the text's glyph coverage offset by (dx, dy), blurred by a
/// Gaussian of σ = blur/2 (Blink's `BlurRadiusToStdDev`), painted in the shadow colour beneath the text,
/// the first layer topmost.
#[allow(clippy::too_many_arguments)]
fn draw_text_shadows(
    text: &str, adv: &Advancer, origin_x: f32, baseline_y: f32, shadows: &[effects::Shadow],
    surface: &mut [u8], width: u32, height: u32, damage_rects: &[(u32, u32, u32, u32)], clip: Clip,
) {
    if shadows.is_empty() {
        return;
    }
    let placed = adv.place(text);
    for sh in shadows.iter().rev() {
        let sigma = (sh.blur / 2.0).max(0.0);
        let pad = (sigma * 3.0).ceil() as i32 + 1;
        // the coverage mask of the run, in a box around its ink
        let lw = adv.str(text);
        let x0 = (origin_x + sh.dx).floor() as i32 - pad - 2;
        let y0 = (baseline_y + sh.dy - adv.size * 1.5).floor() as i32 - pad;
        let mw = (lw + adv.size).ceil() as i32 + 2 * pad + 4;
        let mh = (adv.size * 2.5).ceil() as i32 + 2 * pad;
        if mw <= 0 || mh <= 0 || mw as i64 * mh as i64 > 1 << 24 {
            continue;
        }
        let mut mask = vec![0f32; (mw * mh) as usize];
        let ident: [u8; 256] = std::array::from_fn(|i| i as u8);
        for (g, x, dy) in &placed {
            crate::fonts::raster::draw_glyph(g.face, g.gid, adv.size, origin_x + sh.dx + x, baseline_y + sh.dy + dy, &ident, &mut |px, py, a| {
                let (mx, my) = (px - x0, py - y0);
                if mx >= 0 && my >= 0 && mx < mw && my < mh {
                    let m = &mut mask[(my * mw + mx) as usize];
                    *m = (*m + a as f32 / 255.0).min(1.0);
                }
            });
        }
        if sigma > 0.0 {
            effects::gaussian_blur(&mut mask, mw as usize, mh as usize, sigma);
        }
        let c = sh.color.0;
        for my in 0..mh {
            for mx in 0..mw {
                let a = mask[(my * mw + mx) as usize] * sh.color.1;
                if a <= 0.0 {
                    continue;
                }
                let (px, py) = (x0 + mx, y0 + my);
                if px < 0 || py < 0 {
                    continue;
                }
                let (px, py) = (px as u32, py as u32);
                if px < width && py < height && in_damage(px, py, damage_rects) && in_clip(px, py, clip) {
                    blend_px(surface, width, px, py, c, (a * 255.0).round().min(255.0) as u8);
                }
            }
        }
    }
}

/// Text decoration lines of one painted span from `x0` to `x1` at `baseline` (css-text-decor-3 §2, the
/// face's `post` metrics as Blink uses them: fonts::decoration_metrics).
#[allow(clippy::too_many_arguments)]
fn draw_decorations(
    face: &Font, size: f32, deco: Deco, x0: f32, x1: f32, baseline: f32, color: (u8, u8, u8),
    surface: &mut [u8], width: u32, height: u32, damage_rects: &[(u32, u32, u32, u32)], clip: Clip,
) {
    if !(deco.underline || deco.line_through) || x1 <= x0 {
        return;
    }
    let m = crate::fonts::decoration_metrics(face, size);
    let mut hline = |top: f32, thick: f32| {
        let yi = top.round() as i32;
        for dy in 0..thick.round().max(1.0) as i32 {
            let yy = yi + dy;
            if yy < 0 || yy as u32 >= height {
                continue;
            }
            let xa = x0.max(0.0).round() as u32;
            let xb = (x1.max(0.0).round() as u32).min(width);
            for x in xa..xb {
                if in_damage(x, yy as u32, damage_rects) && in_clip(x, yy as u32, clip) {
                    put_px(surface, width, x, yy as u32, color);
                }
            }
        }
    };
    if deco.underline {
        hline(baseline + m.underline_offset, m.thickness);
    }
    if deco.line_through {
        hline(baseline - m.line_through_offset, m.thickness);
    }
}

/// How one background (or mask) layer maps onto a box: the painted image
/// rectangle in box-local coordinates plus the repeat mode.
#[derive(Clone, Copy)]
struct BgGeometry {
    off_x: f32,
    off_y: f32,
    w: f32,
    h: f32,
    repeat: u8, // 0 repeat, 1 no-repeat, 2 repeat-x, 3 repeat-y
}

impl BgGeometry {
    /// Maps a box-local pixel to image-space UV, honouring the repeat mode.
    /// None = this pixel is outside every tile of the layer.
    fn sample(&self, lx: f32, ly: f32, iw: u32, ih: u32) -> Option<(u32, u32)> {
        let (rx, ry) = (matches!(self.repeat, 0 | 2), matches!(self.repeat, 0 | 3));
        let mut u = lx - self.off_x;
        let mut v = ly - self.off_y;
        if rx {
            u = u.rem_euclid(self.w);
        } else if u < 0.0 || u >= self.w {
            return None;
        }
        if ry {
            v = v.rem_euclid(self.h);
        } else if v < 0.0 || v >= self.h {
            return None;
        }
        Some((
            ((u / self.w * iw as f32) as u32).min(iw.saturating_sub(1)),
            ((v / self.h * ih as f32) as u32).min(ih.saturating_sub(1)),
        ))
    }
}

/// Resolves one length/percentage component against a reference length.
/// Returns None for `auto` and unparsed values.
fn bg_length(token: &str, reference: f32) -> Option<f32> {
    let t = token.trim();
    // calc()/min()/max()/clamp() — component CSS states icon metrics this way.
    if t.contains('(') {
        return crate::css::eval_length(t, Some(reference));
    }
    if let Some(p) = t.strip_suffix('%') {
        return p.parse::<f32>().ok().map(|p| p / 100.0 * reference);
    }
    for unit in ["px", "pt", "rem", "em"] {
        if let Some(n) = t.strip_suffix(unit) {
            let n: f32 = n.trim().parse().ok()?;
            return Some(match unit {
                "px" => n,
                "pt" => n * 4.0 / 3.0,
                _ => n * default_font_size("", 16.0),
            });
        }
    }
    t.parse::<f32>().ok().filter(|n| *n == 0.0)
}

/// Splits a background-/mask- component list on whitespace that sits outside
/// any parentheses, so `max(calc(1rem + 4px), 10px)` stays one token.
fn split_components(value: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (i, c) in value.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c.is_whitespace() && depth <= 0 => {
                if i > start {
                    out.push(&value[start..i]);
                }
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    if start < value.len() {
        out.push(&value[start..]);
    }
    out
}

/// Resolves background-size / background-position / background-repeat into
/// the painted rectangle. `size`/`position` are the declared strings (None
/// = CSS initial: `auto` and `0% 0%`).
fn resolve_bg_geometry(
    size: Option<&str>,
    position: Option<&str>,
    repeat: Option<u8>,
    box_w: f32,
    box_h: f32,
    img_w: f32,
    img_h: f32,
) -> BgGeometry {
    // --- background-size ---
    let (mut w, mut h) = (img_w, img_h);
    match size.map(str::trim) {
        Some("cover") | Some("contain") => {
            let s = if size == Some("cover") {
                (box_w / img_w).max(box_h / img_h)
            } else {
                (box_w / img_w).min(box_h / img_h)
            };
            w = img_w * s;
            h = img_h * s;
        }
        Some(v) if !v.is_empty() && v != "auto" => {
            let mut it = split_components(v).into_iter();
            let a = it.next().unwrap_or("auto");
            let b = it.next();
            let rw = bg_length(a, box_w);
            let rh = b.and_then(|b| bg_length(b, box_h));
            match (rw, rh) {
                // One value (or an explicit `auto` partner): the other axis
                // keeps the intrinsic aspect ratio.
                (Some(rw), None) => {
                    w = rw;
                    h = img_h * (rw / img_w);
                }
                (None, Some(rh)) => {
                    h = rh;
                    w = img_w * (rh / img_h);
                }
                (Some(rw), Some(rh)) => {
                    w = rw;
                    h = rh;
                }
                (None, None) => {}
            }
        }
        _ => {}
    }
    let (w, h) = (w.max(0.01), h.max(0.01));

    // --- background-position ---
    let mut off_x = 0.0;
    let mut off_y = 0.0;
    if let Some(pos) = position {
        let tokens: Vec<&str> = split_components(pos);
        // Keyword tokens name their own axis; anything else fills
        // horizontal-then-vertical in source order.
        let mut horiz: Option<&str> = None;
        let mut vert: Option<&str> = None;
        for t in &tokens {
            match *t {
                "left" | "right" => horiz = Some(t),
                "top" | "bottom" => vert = Some(t),
                "center" => {
                    if horiz.is_none() && vert.is_some() {
                        horiz = Some("center");
                    } else if horiz.is_none() {
                        horiz = Some("center");
                    } else if vert.is_none() {
                        vert = Some("center");
                    }
                }
                other => {
                    if horiz.is_none() {
                        horiz = Some(other);
                    } else if vert.is_none() {
                        vert = Some(other);
                    }
                }
            }
        }
        // A lone horizontal keyword/value centres nothing vertically: the
        // vertical component defaults to `center` per the CSS grammar only
        // when a keyword was used; a bare length defaults to 0 for the
        // second component in the one-value form -> `center` per spec.
        let resolve = |tok: Option<&str>, free: f32, span: f32| -> f32 {
            match tok {
                None | Some("left") | Some("top") => 0.0,
                Some("right") | Some("bottom") => free,
                Some("center") => free / 2.0,
                Some(t) => {
                    if t.ends_with('%') {
                        bg_length(t, free).unwrap_or(0.0)
                    } else {
                        bg_length(t, span).unwrap_or(0.0)
                    }
                }
            }
        };
        off_x = resolve(horiz, box_w - w, box_w);
        off_y = resolve(
            if tokens.len() == 1 && vert.is_none() { Some("center") } else { vert },
            box_h - h,
            box_h,
        );
    }

    BgGeometry { off_x, off_y, w, h, repeat: repeat.unwrap_or(0) }
}

/// Test hook: the resolved background/mask rectangle as
/// `(off_x, off_y, w, h)`. Keeps `BgGeometry` private to the renderer.
#[cfg(test)]
pub(crate) fn test_bg_geometry(
    size: Option<&str>,
    position: Option<&str>,
    repeat: Option<u8>,
    box_w: f32,
    box_h: f32,
    img_w: f32,
    img_h: f32,
) -> (f32, f32, f32, f32) {
    let g = resolve_bg_geometry(size, position, repeat, box_w, box_h, img_w, img_h);
    (g.off_x, g.off_y, g.w, g.h)
}

/// Transforms text based on text-transform property: 0 = none, 1 = uppercase, 2 = lowercase, 3 = capitalize.
pub(crate) fn transform_text(text: &str, transform: u8) -> String {
    match transform {
        1 => text.to_uppercase(),
        2 => text.to_lowercase(),
        3 => {
            // capitalize: uppercase first letter of each word
            text.split_whitespace()
                .map(|word| {
                    let mut chars = word.chars();
                    match chars.next() {
                        None => String::new(),
                        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        }
        _ => text.to_string(),
    }
}

/// Draws `text` starting at (origin_x, origin_y), wrapping at `max_width`
/// (white-space: normal, no decoration beyond `underline`).
#[allow(clippy::too_many_arguments)]
fn draw_text(
    text: &str,
    origin_x: f32,
    origin_y: f32,
    max_width: f32,
    adv: &Advancer,
    line_height_mult: f32,
    color: (u8, u8, u8),
    underline: bool,
    surface: &mut [u8],
    width: u32,
    height: u32,
    damage_rects: &[(u32, u32, u32, u32)],
    clip: Clip,
) {
    let deco = Deco { underline, line_through: false };
    draw_lines(
        text, origin_x, origin_y, max_width, adv, line_height_mult, color,
        &crate::fonts::lines::TextMode::default(), deco, &[], surface, width, height, damage_rects, clip,
    );
}

/// The inherited paint state of an element's subtree: font, colour,
/// decorations, white-space, list style, super/sub shift (CSS 2.2 §6.2,
/// with the html.css UA defaults by tag).
fn inherit_element(inherited: &mut Inherited, tag: &str, spec: &crate::layout::PaintStyle, dom_node: &crate::dom::NodeRef, node: NodeId) {
    if spec.text_shadows.is_some() {
        inherited.text_shadow = Some(node);
    }
    let parent_font_size = inherited.font_size;
    let own_family = spec
        .family
        .unwrap_or_else(|| crate::layout::default_family(tag, inherited.family));
    inherited.font_size = spec.used_font_size.or(spec.font_size).unwrap_or_else(|| {
        crate::layout::ua_font_size(tag, inherited.font_size, inherited.family, own_family)
    });
    inherited.weight = spec
        .weight
        .map(|w| w.resolve(inherited.weight))
        .unwrap_or_else(|| crate::layout::default_weight(tag, inherited.weight));
    inherited.italic =
        spec.italic.unwrap_or_else(|| crate::layout::default_italic(tag, inherited.italic));
    if let Some(st) = spec.stretch {
        inherited.stretch = st;
    }
    if let Some(ws) = spec.word_spacing {
        inherited.word_spacing = ws;
    }
    inherited.rtl = spec.rtl.unwrap_or_else(|| crate::layout::default_rtl(dom_node, inherited.rtl));
    inherited.family = spec
        .family
        .unwrap_or_else(|| crate::layout::default_family(tag, inherited.family));
    if let Some(lh) = spec.line_height {
        inherited.line_height = lh;
    }
    if let Some(tt) = spec.text_transform {
        inherited.text_transform = tt;
    }

    if tag == "a" || tag == "u" {
        inherited.underline = true;
    }
    if let Some(u) = spec.underline {
        inherited.underline = u;
    }
    if let Some(nw) = spec.nowrap {
        inherited.nowrap = nw;
    }
    inherited.white_space = spec
        .white_space
        .unwrap_or_else(|| crate::layout::default_white_space(tag, inherited.white_space));
    if let Some(v) = spec.word_break { inherited.word_break = v; }
    if let Some(v) = spec.overflow_wrap { inherited.overflow_wrap = v; }
    if let Some(v) = spec.letter_spacing { inherited.letter_spacing = v; }
    inherited.line_through = spec
        .line_through
        .unwrap_or_else(|| crate::layout::default_line_through(tag, inherited.line_through));
    // html.css list styles: ul disc (circle one level down,
    // square deeper), ol decimal; inherited into the items.
    match tag {
        "ul" | "menu" | "dir" => {
            let depth = dom_node
                .ancestors()
                .filter(|a| a.as_element().is_some_and(|e| matches!(e.name.local.as_ref(), "ul" | "ol" | "menu" | "dir")))
                .count();
            inherited.list_style = [1, 2, 3][depth.min(2)];
        }
        "ol" => inherited.list_style = 4,
        _ => {}
    }
    if let Some(ls) = spec.list_style {
        inherited.list_style = ls;
    }
    // Blink: super raises by parent-size/3 + 1, sub lowers by
    // parent-size/5 + 1.
    match tag {
        "sup" => inherited.shift_y -= parent_font_size / 3.0 + 1.0,
        "sub" => inherited.shift_y += parent_font_size / 5.0 + 1.0,
        _ => {}
    }
    if spec.text_hidden == Some(true) {
        inherited.text_hidden = true;
    }
    inherited.color = spec.color.unwrap_or(if tag == "a" {
        (0, 0, 238) // UA default link blue
    } else {
        inherited.color
    });
}

/// Draws one line's worth of text with its baseline at `baseline_y` — an
/// inline formatting context's text fragment (layout::inline): shadows,
/// then the shaped glyphs, then decorations spanning the fragment.
#[allow(clippy::too_many_arguments)]
fn draw_glyph_run(
    text: &str,
    origin_x: f32,
    baseline_y: f32,
    adv: &Advancer,
    color: (u8, u8, u8),
    deco: Deco,
    shadows: &[effects::Shadow],
    surface: &mut [u8],
    width: u32,
    height: u32,
    damage_rects: &[(u32, u32, u32, u32)],
    clip: Clip,
) {
    draw_text_shadows(text, adv, origin_x, baseline_y, shadows, surface, width, height, damage_rects, clip);
    let w = draw_shaped(text, adv, origin_x, baseline_y, color, surface, width, height, damage_rects, clip);
    if let Some(face) = crate::fonts::face(&adv.sel) {
        // The fragment's spaces are inside its inline box (the line's hanging
        // spaces were already removed by layout), so the decoration spans them.
        draw_decorations(face, adv.size, deco, origin_x, origin_x + w, baseline_y, color, surface, width, height, damage_rects, clip);
    }
}

/// Positioned (relative/absolute/fixed/sticky) with an integer z-index,
/// fixed/sticky, a flex item with an integer z-index, or opacity < 1:
/// the box forms a stacking context (CSS 2.2 §9.9.1, css-position-3 §8,
/// css-flexbox-1 §4.3, css-color-4 §15).
pub(crate) fn is_stacking_context(layout: &LayoutTree, id: NodeId) -> bool {
    let Some(p) = layout.paint_map.get(&id) else { return false };
    let kind = p.position_kind.unwrap_or(0);
    let z = p.z_index.flatten();
    if matches!(kind, 3 | 4) || (kind != 0 && z.is_some()) {
        return true;
    }
    if p.opacity.is_some_and(|o| o < 1.0) {
        return true;
    }
    z.is_some()
        && layout
            .taffy
            .parent(id)
            .and_then(|par| layout.paint_map.get(&par))
            .and_then(|pp| pp.flex_container)
            == Some(true)
}

/// Painted as a layer of its stacking context rather than in normal flow.
fn is_layered(layout: &LayoutTree, id: NodeId) -> bool {
    layout.paint_map.get(&id).and_then(|p| p.position_kind).is_some_and(|k| k != 0)
        || is_stacking_context(layout, id)
}

/// The layer's z: its integer z-index, else 0 (auto paints at 0).
fn z_of(layout: &LayoutTree, id: NodeId) -> i32 {
    layout.paint_map.get(&id).and_then(|p| p.z_index).flatten().unwrap_or(0)
}

/// The vertical offset of a table cell's content (CSS 2.2 §17.5.4): middle
/// (the UA default) centres the line boxes in the content box, bottom puts
/// them at its end, top/baseline leave them at the top.
fn cell_valign_offset(layout: &LayoutTree, id: NodeId, content_h: f32) -> f32 {
    let is_cell = layout
        .node_map
        .get(&id)
        .and_then(|n| n.as_element().map(|e| matches!(e.name.local.as_ref(), "td" | "th")))
        .unwrap_or(false);
    if !is_cell {
        return 0.0;
    }
    let Ok(l) = layout.taffy.layout(id) else { return 0.0 };
    let inner = l.size.height - l.padding.top - l.padding.bottom - l.border.top - l.border.bottom;
    let free = (inner - content_h).max(0.0);
    match layout.paint_map.get(&id).and_then(|p| p.vertical_align).map(|v| v.0) {
        Some(0 | 6) => 0.0,
        Some(7) => free,
        _ => (free / 2.0).floor(),
    }
}

/// Text decoration lines of one run.
#[derive(Clone, Copy, Default)]
struct Deco {
    underline: bool,
    line_through: bool,
}

/// Draws a run on the lines fonts::lines breaks it into — the same breaker
/// the measurer used, so the run fills exactly its measured box.
#[allow(clippy::too_many_arguments)]
fn draw_lines(
    text: &str,
    origin_x: f32,
    origin_y: f32,
    max_width: f32,
    adv: &Advancer,
    line_height_mult: f32,
    color: (u8, u8, u8),
    mode: &crate::fonts::lines::TextMode,
    deco: Deco,
    shadows: &[effects::Shadow],
    surface: &mut [u8],
    width: u32,
    height: u32,
    damage_rects: &[(u32, u32, u32, u32)],
    clip: Clip,
) {
    let Some(font) = crate::fonts::face(&adv.sel) else { return };
    let size = adv.size;
    let ascent = crate::fonts::baseline_offset(font, size, line_height_mult);
    let line_height = crate::fonts::line_height(font, size, line_height_mult);
    let lines = crate::fonts::lines::break_lines(adv, text, mode, max_width);
    for (i, line) in lines.iter().enumerate() {
        let baseline_y = origin_y + i as f32 * line_height + ascent;
        draw_text_shadows(&line.text, adv, origin_x, baseline_y, shadows, surface, width, height, damage_rects, clip);
        draw_shaped(&line.text, adv, origin_x, baseline_y, color, surface, width, height, damage_rects, clip);
        // Decorations span the line's ink extent (leading/trailing
        // collapsible spaces excluded).
        let lead = &line.text[..line.text.len() - line.text.trim_start_matches(' ').len()];
        let trail = &line.text[line.text.trim_end_matches(' ').len()..];
        let (lead_w, trail_w) = (adv.str(lead), adv.str(trail));
        let (x0, x1) = (origin_x + lead_w, origin_x + (line.width - trail_w).max(lead_w));
        draw_decorations(font, size, deco, x0, x1, baseline_y, color, surface, width, height, damage_rects, clip);
    }
}

/// The ordinal of a list item: its `value`, else the list's `start` (1)
/// plus the number of preceding items (`reversed` counts down).
fn list_ordinal(li: &crate::dom::NodeRef) -> i64 {
    let attr_num = |n: &crate::dom::NodeRef, a: &str| {
        n.as_element().and_then(|e| e.attributes.borrow().get(a).and_then(|v| v.trim().parse::<i64>().ok()))
    };
    if let Some(v) = attr_num(li, "value") {
        return v;
    }
    let is_li = |n: &crate::dom::NodeRef| n.as_element().is_some_and(|e| e.name.local.as_ref() == "li");
    let before = li.preceding_siblings().filter(|n| is_li(n)).count() as i64;
    let parent = li.parent();
    let reversed = parent.as_ref().and_then(|p| p.as_element().map(|e| e.attributes.borrow().get("reversed").is_some())).unwrap_or(false);
    if reversed {
        let total = parent.as_ref().map(|p| p.children().filter(|n| is_li(n)).count() as i64).unwrap_or(1);
        parent.as_ref().and_then(|p| attr_num(p, "start")).unwrap_or(total) - before
    } else {
        parent.as_ref().and_then(|p| attr_num(p, "start")).unwrap_or(1) + before
    }
}

fn roman(mut n: i64) -> String {
    if n <= 0 || n >= 4000 {
        return n.to_string();
    }
    let table = [(1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"), (90, "xc"), (50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i")];
    let mut s = String::new();
    for (v, r) in table {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    s
}

fn alpha(n: i64) -> String {
    if n <= 0 {
        return n.to_string();
    }
    let mut n = n;
    let mut s = Vec::new();
    while n > 0 {
        n -= 1;
        s.push((b'a' + (n % 26) as u8) as char);
        n /= 26;
    }
    s.iter().rev().collect()
}

/// The marker text of a counter style (css-counter-styles-3 §6-7 for the
/// predefined styles): "3. ", "c. ", "iv. ", "03. ".
pub(crate) fn marker_text(style: u8, n: i64) -> String {
    let body = match style {
        5 => alpha(n),
        6 => alpha(n).to_uppercase(),
        7 => roman(n),
        8 => roman(n).to_uppercase(),
        9 if (0..10).contains(&n) => format!("0{n}"),
        _ => n.to_string(),
    };
    format!("{body}. ")
}

/// Paints an outside list marker. Bullets are Blink's shapes — a disc,
/// circle or square of about a third of the em, centred ~0.27em above the
/// baseline, its left edge one em before the content edge; counters are
/// the marker text in the item's font, ending at the content edge.
#[allow(clippy::too_many_arguments)]
fn paint_marker(
    li: &crate::dom::NodeRef, style: u8, content_x: f32, baseline: f32, adv: &Advancer,
    color: (u8, u8, u8), surface: &mut [u8], width: u32, height: u32,
    damage_rects: &[(u32, u32, u32, u32)], clip: Clip,
) {
    let fs = adv.size;
    if (1..=3).contains(&style) {
        let d = (fs * 0.31).round().max(3.0);
        let x = (content_x - fs).round();
        let cy = baseline - (fs * 0.27).round();
        let y = (cy - d / 2.0).round();
        let mut blend_at = |px: u32, py: u32, c: (u8, u8, u8), a: f32| {
            if !in_damage(px, py, damage_rects) || !in_clip(px, py, clip) {
                return;
            }
            if a >= 0.999 {
                put_px(surface, width, px, py, c);
            } else if a > 0.0 {
                blend_px(surface, width, px, py, c, (a * 255.0).round() as u8);
            }
        };
        match style {
            1 => boxpaint::paint(x, y, d, d, [d / 2.0; 4], [None; 4], Some(color), width, height, &mut blend_at),
            2 => boxpaint::paint(x, y, d, d, [d / 2.0; 4], [Some((1.0, color, 0)); 4], None, width, height, &mut blend_at),
            _ => boxpaint::paint(x, y, d, d, [0.0; 4], [None; 4], Some(color), width, height, &mut blend_at),
        }
        return;
    }
    let text = marker_text(style, list_ordinal(li));
    let w = adv.str(&text);
    draw_shaped(&text, adv, content_x - w, baseline, color, surface, width, height, damage_rects, clip);
}

/// A checkbox or radio at Chromium's control-theme look: 13x13, a 1px
/// #767676 frame (radius 2, or a circle) on white; checked, a #0075ff fill
/// with a white tick, or a #0075ff ring around a #0075ff dot.
#[allow(clippy::too_many_arguments)]
fn paint_checkable(
    x: f32, y: f32, w: f32, h: f32, radio: bool, checked: bool,
    surface: &mut [u8], width: u32, height: u32, damage_rects: &[(u32, u32, u32, u32)], clip: Clip,
) {
    let mut blend_at = |px: u32, py: u32, c: (u8, u8, u8), a: f32| {
        if !in_damage(px, py, damage_rects) || !in_clip(px, py, clip) {
            return;
        }
        if a >= 0.999 {
            put_px(surface, width, px, py, c);
        } else if a > 0.0 {
            blend_px(surface, width, px, py, c, (a * 255.0).round() as u8);
        }
    };
    let blue = (0, 117, 255);
    let grey = (118, 118, 118);
    let r = if radio { [w.min(h) / 2.0; 4] } else { [2.0; 4] };
    let frame = |c| [Some((1.0, c, 0u8)); 4];
    if radio {
        if checked {
            boxpaint::paint(x, y, w, h, r, frame(blue), Some((255, 255, 255)), width, height, &mut blend_at);
            let d = w.min(h) * 0.55;
            boxpaint::paint(x + (w - d) / 2.0, y + (h - d) / 2.0, d, d, [d / 2.0; 4], [None; 4], Some(blue), width, height, &mut blend_at);
        } else {
            boxpaint::paint(x, y, w, h, r, frame(grey), Some((255, 255, 255)), width, height, &mut blend_at);
        }
        return;
    }
    if !checked {
        boxpaint::paint(x, y, w, h, r, frame(grey), Some((255, 255, 255)), width, height, &mut blend_at);
        return;
    }
    boxpaint::paint(x, y, w, h, r, [None; 4], Some(blue), width, height, &mut blend_at);
    // The tick: (3, 6.5) -> (5.5, 9) -> (10, 4) in the 13px box, 1.5px thick.
    let seg = |p: (f32, f32), q: (f32, f32), blend: &mut dyn FnMut(u32, u32, (u8, u8, u8), f32)| {
        let (x0, y0, x1, y1) = (x + p.0 * w / 13.0, y + p.1 * h / 13.0, x + q.0 * w / 13.0, y + q.1 * h / 13.0);
        let (minx, maxx) = (x0.min(x1).floor() - 1.0, x0.max(x1).ceil() + 1.0);
        let (miny, maxy) = (y0.min(y1).floor() - 1.0, y0.max(y1).ceil() + 1.0);
        let (dx, dy) = (x1 - x0, y1 - y0);
        let len2 = dx * dx + dy * dy;
        let mut yy = miny;
        while yy <= maxy {
            let mut xx = minx;
            while xx <= maxx {
                let (cx, cy) = (xx + 0.5, yy + 0.5);
                let t = (((cx - x0) * dx + (cy - y0) * dy) / len2).clamp(0.0, 1.0);
                let (ex, ey) = (cx - (x0 + t * dx), cy - (y0 + t * dy));
                let d = (ex * ex + ey * ey).sqrt();
                let a = (1.25 - d).clamp(0.0, 1.0);
                if a > 0.0 && xx >= 0.0 && yy >= 0.0 {
                    blend(xx as u32, yy as u32, (255, 255, 255), a);
                }
                xx += 1.0;
            }
            yy += 1.0;
        }
    };
    seg((3.0, 6.5), (5.5, 9.0), &mut blend_at);
    seg((5.5, 9.0), (10.0, 4.0), &mut blend_at);
}

/// Single-line advance width of `text`, measured EXACTLY the way `draw_text`
/// lays it out (whitespace collapsed, shaped). Centering a control label
/// with any other measurement drifts.
fn measure_text_width(text: &str, adv: &Advancer) -> f32 {
    adv.str(&text.split_whitespace().collect::<Vec<_>>().join(" "))
}

/// (ascent, descent) in pixels at `font_size` — descent is positive-down, so
/// the inked extent of one line is `ascent + descent`.
fn text_extents(font: &Font, font_size: f32) -> (f32, f32) {
    let metrics = font.metrics();
    let scale = font_size / metrics.units_per_em as f32;
    (metrics.ascent * scale, -metrics.descent * scale)
}

/// `draw_text` origin_y that puts the FIRST line's inked extent centred on
/// `center_y`. `draw_text` adds the ascent to reach the baseline, so the
/// origin sits half an inked line above the centre.
fn centered_line_origin_y(font: &Font, font_size: f32, center_y: f32) -> f32 {
    let (ascent, descent) = text_extents(font, font_size);
    center_y - (ascent + descent) / 2.0
}

/// UNAOS_LAYOUTDUMP=<max-depth>: writes the computed box tree (tag, id/class,
/// rect, display, paint gates) to stderr. Diagnostic only — off by default.
pub fn dump_layout(layout: &LayoutTree) {
    let Ok(max_depth) = std::env::var("UNAOS_LAYOUTDUMP") else { return };
    let max_depth: usize = max_depth.trim().parse().unwrap_or(6);
    fn walk(layout: &LayoutTree, id: NodeId, x: f32, y: f32, depth: usize, max_depth: usize) {
        let Ok(b) = layout.taffy.layout(id) else { return };
        let (cx, cy) = (x + b.location.x, y + b.location.y);
        if depth <= max_depth {
            let st = layout.taffy.style(id);
            let disp = st.map(|s| format!("{:?}", s.display)).unwrap_or_default();
            let p = layout.paint_map.get(&id);
            let mut desc = String::new();
            if let Some(n) = layout.node_map.get(&id) {
                if let Some(el) = n.as_element() {
                    desc.push_str(el.name.local.as_ref());
                    let a = el.attributes.borrow();
                    if let Some(i) = a.get("id") {
                        desc.push_str(&format!("#{}", i));
                    }
                    if let Some(c) = a.get("class") {
                        desc.push_str(&format!(".{}", &c[..c.len().min(60)]));
                    }
                } else if n.as_text().is_some() {
                    let t = n.text_contents();
                    let t = t.trim();
                    desc = format!("\"{}\"", &t[..t.len().min(40)]);
                }
            }
            eprintln!(
                "{:indent$}{} @({:.0},{:.0}) {:.0}x{:.0} {} hid={:?} clip={:?}",
                "", desc, cx, cy, b.size.width, b.size.height, disp,
                p.and_then(|p| p.hidden), p.and_then(|p| p.clip),
                indent = depth * 2,
            );
            if let Some(il) = layout.inline.get(&id) {
                for (t, h, b) in &il.lines {
                    eprintln!("{:indent$}| line y={t:.2} h={h:.2} baseline={b:.2}", "", indent = depth * 2 + 2);
                }
                for f in &il.frags {
                    eprintln!("{:indent$}| {f:?}", "", indent = depth * 2 + 2);
                }
            }
        }
        if depth >= max_depth {
            return;
        }
        if let Ok(kids) = layout.taffy.children(id) {
            for k in kids {
                walk(layout, k, cx, cy, depth + 1, max_depth);
            }
        }
    }
    walk(layout, layout.root_node, 0.0, 0.0, 0, max_depth);
}

/// The colour that fills the whole canvas: `html`'s background-color, or,
/// when the root has none, `body`'s. None = no author background (white).
pub fn canvas_background(layout: &LayoutTree) -> Option<(u8, u8, u8)> {
    let find = |tag: &str| -> Option<NodeId> {
        // html and body sit within the top three levels of the box tree.
        let mut level = vec![layout.root_node];
        for _ in 0..4 {
            let mut next = Vec::new();
            for id in level {
                let is_tag = layout
                    .node_map
                    .get(&id)
                    .and_then(|n| n.as_element().map(|e| e.name.local.as_ref() == tag))
                    .unwrap_or(false);
                if is_tag {
                    return Some(id);
                }
                next.extend(layout.taffy.children(id).unwrap_or_default());
            }
            level = next;
        }
        None
    };
    let bg = |id: Option<NodeId>| {
        let p = id.and_then(|i| layout.paint_map.get(&i))?;
        let c = p.background?;
        let a = p.bg_alpha.unwrap_or(1.0);
        if a <= 0.0 {
            return None;
        }
        // Over the white the UA canvas starts as.
        let mix = |v: u8| (v as f32 * a + 255.0 * (1.0 - a)).round() as u8;
        Some((mix(c.0), mix(c.1), mix(c.2)))
    };
    bg(find("html")).or_else(|| bg(find("body")))
}

/// Paint a `<video>`/`<audio>` box (AETHERVIDEO, LEDGER SR39): the picture (poster or the
/// frame on glass) placed by `object-fit`/`object-position` inside the content box and clipped
/// to it, bilinear-sampled at pixel centres (exact at 1:1); Stria's error text when the stream
/// failed; and, with `controls`, a play/pause + progress + time strip along the bottom.
#[allow(clippy::too_many_arguments)]
fn paint_media(
    media: &crate::media::Paint,
    spec: &crate::layout::PaintStyle,
    content: (f32, f32, f32, f32),
    font: &Option<&'static Font>,
    surface: &mut [u8],
    width: u32,
    height: u32,
    damage_rects: &[(u32, u32, u32, u32)],
    clip: Clip,
) {
    let (cx, cy, cw, ch) = content;
    if cw <= 0.0 || ch <= 0.0 {
        return;
    }
    // The content box, in clamped screen pixels, intersected with the clip.
    let span = |x0: f32, y0: f32, x1: f32, y1: f32| {
        let x0 = x0.max(cx).max(clip.0).max(0.0);
        let y0 = y0.max(cy).max(clip.1).max(0.0);
        let x1 = x1.min(cx + cw).min(clip.2).min(width as f32);
        let y1 = y1.min(cy + ch).min(clip.3).min(height as f32);
        (x0.round() as u32, y0.round() as u32, (x1.round().max(0.0)) as u32, (y1.round().max(0.0)) as u32)
    };

    if let Some(err) = &media.error {
        let (x0, y0, x1, y1) = span(cx, cy, cx + cw, cy + ch);
        for y in y0..y1 {
            for x in x0..x1 {
                if in_damage(x, y, damage_rects) {
                    put_px(surface, width, x, y, (32, 32, 32));
                }
            }
        }
        if let Some(font) = font {
            let fs = 13.0;
            draw_text(
                err, cx + 8.0, centered_line_origin_y(font, fs, cy + ch.min(40.0) / 2.0), (cw - 16.0).max(1.0),
                &Advancer::new(ui_sel(), fs, 0.0), 1.2, (255, 255, 255), false, surface, width, height, damage_rects,
                (cx.max(clip.0), cy.max(clip.1), (cx + cw).min(clip.2), (cy + ch).min(clip.3)),
            );
        }
    } else if let Some(pic) = &media.picture {
        let (sw, sh) = (pic.width() as f32, pic.height() as f32);
        let fit = spec.object_fit.unwrap_or(1);
        let pos = spec.object_position.as_deref().map(crate::media::parse_object_position).unwrap_or((0.5, 0.5));
        let (rx, ry, rw, rh) = crate::media::object_fit_rect(fit, pos, cw, ch, sw, sh);
        let (dx, dy) = (cx + rx, cy + ry);
        let (x0, y0, x1, y1) = span(dx, dy, dx + rw, dy + rh);
        let (sx_scale, sy_scale) = (sw / rw, sh / rh);
        let (iw, ih) = (pic.width() as i32, pic.height() as i32);
        let raw = pic.as_raw();
        let at = |u: i32, v: i32| {
            let o = ((v.clamp(0, ih - 1) * iw + u.clamp(0, iw - 1)) * 4) as usize;
            [raw[o] as f32, raw[o + 1] as f32, raw[o + 2] as f32, raw[o + 3] as f32]
        };
        for y in y0..y1 {
            let fv = (y as f32 + 0.5 - dy) * sy_scale - 0.5;
            let v0 = fv.floor();
            let ty = fv - v0;
            for x in x0..x1 {
                if !in_damage(x, y, damage_rects) {
                    continue;
                }
                let fu = (x as f32 + 0.5 - dx) * sx_scale - 0.5;
                let u0 = fu.floor();
                let tx = fu - u0;
                let (u0, v0i) = (u0 as i32, v0 as i32);
                let (p00, p10, p01, p11) = (at(u0, v0i), at(u0 + 1, v0i), at(u0, v0i + 1), at(u0 + 1, v0i + 1));
                let mut c = [0u8; 4];
                for k in 0..4 {
                    let top = p00[k] + (p10[k] - p00[k]) * tx;
                    let bot = p01[k] + (p11[k] - p01[k]) * tx;
                    c[k] = (top + (bot - top) * ty).round().clamp(0.0, 255.0) as u8;
                }
                if c[3] == 255 {
                    put_px(surface, width, x, y, (c[0], c[1], c[2]));
                } else if c[3] > 0 {
                    blend_px(surface, width, x, y, (c[0], c[1], c[2]), c[3]);
                }
            }
        }
    }

    if media.controls && media.kind == crate::media::Kind::Audio {
        paint_audio_controls(media, content, font, surface, width, height, damage_rects, clip);
    } else if media.controls {
        // The strip: 32px, translucent black.
        let sh = if media.kind == crate::media::Kind::Audio { ch } else { ch.min(32.0) };
        let top = cy + ch - sh;
        let (x0, y0, x1, y1) = span(cx, top, cx + cw, cy + ch);
        for y in y0..y1 {
            for x in x0..x1 {
                if in_damage(x, y, damage_rects) {
                    blend_px(surface, width, x, y, (0, 0, 0), 160);
                }
            }
        }
        let mid = top + sh / 2.0;
        let white = (255, 255, 255);
        let fill = |surface: &mut [u8], fx0: f32, fy0: f32, fx1: f32, fy1: f32, test: &dyn Fn(f32, f32) -> bool, color: (u8, u8, u8)| {
            let (x0, y0, x1, y1) = span(fx0, fy0, fx1, fy1);
            for y in y0..y1 {
                for x in x0..x1 {
                    if in_damage(x, y, damage_rects) && test(x as f32 + 0.5, y as f32 + 0.5) {
                        put_px(surface, width, x, y, color);
                    }
                }
            }
        };
        let bx = cx + 10.0;
        if media.playing {
            // Pause: two bars.
            fill(surface, bx, mid - 7.0, bx + 4.0, mid + 7.0, &|_, _| true, white);
            fill(surface, bx + 8.0, mid - 7.0, bx + 12.0, mid + 7.0, &|_, _| true, white);
        } else {
            // Play: a right-pointing triangle.
            fill(surface, bx, mid - 7.0, bx + 12.0, mid + 7.0, &|x, y| (y - mid).abs() <= 7.0 * (1.0 - (x - bx) / 12.0), white);
        }
        // Time text and progress track.
        let label = format!(
            "{} / {}",
            crate::media::clock_text(media.pts_ns),
            crate::media::clock_text(media.duration_ns as i64)
        );
        let mut track_x0 = bx + 22.0;
        if let Some(font) = font {
            let fs = 12.0;
            let tw = measure_text_width(&label, &Advancer::new(ui_sel(), fs, 0.0));
            if cw > tw + 80.0 {
                draw_text(
                    &label, track_x0, centered_line_origin_y(font, fs, mid), tw + 2.0, &Advancer::new(ui_sel(), fs, 0.0), 1.2,
                    white, false, surface, width, height, damage_rects, clip,
                );
                track_x0 += tw + 10.0;
            }
        }
        let track_x1 = cx + cw - 10.0;
        if track_x1 > track_x0 + 4.0 {
            fill(surface, track_x0, mid - 2.0, track_x1, mid + 2.0, &|_, _| true, (110, 110, 110));
            if media.duration_ns > 0 {
                let f = (media.pts_ns.max(0) as f32 / media.duration_ns as f32).clamp(0.0, 1.0);
                fill(surface, track_x0, mid - 2.0, track_x0 + (track_x1 - track_x0) * f, mid + 2.0, &|_, _| true, white);
            }
        }
    }
}

/// `<audio controls>` as Chromium draws its default audio control (AUDIOTRACK, LEDGER SR45),
/// geometry measured from Chromium's own rendering of a 300×54 control: a pill of
/// rgb(241,243,244) with fully rounded ends; a play triangle (pause bars while playing) — the
/// Material icons at 20 px, at x+16; the `m:ss / m:ss` time at x+47, 13 px; the timeline
/// x+129 … right−91, 4 px tall with round caps, played part rgb(11,11,11), the rest
/// rgb(88,89,89); the speaker (Material `volume_up`, 24 px) at right−69; the overflow dots
/// (`more_vert`, 20 px) at right−37.5. Shapes are coverage-antialiased (4×4 samples per
/// pixel). Stria's meter frames move the time while it plays.
#[allow(clippy::too_many_arguments)]
fn paint_audio_controls(
    media: &crate::media::Paint,
    content: (f32, f32, f32, f32),
    font: &Option<&'static Font>,
    surface: &mut [u8],
    width: u32,
    height: u32,
    damage_rects: &[(u32, u32, u32, u32)],
    clip: Clip,
) {
    let (cx, cy, cw, ch) = content;
    let lim = (cx.max(clip.0).max(0.0), cy.max(clip.1).max(0.0), (cx + cw).min(clip.2).min(width as f32), (cy + ch).min(clip.3).min(height as f32));
    // Fill `inside` over the rectangle with 4×4-sample coverage.
    let cover = |surface: &mut [u8], r: (f32, f32, f32, f32), inside: &dyn Fn(f32, f32) -> bool, color: (u8, u8, u8)| {
        let x0 = r.0.max(lim.0).floor().max(0.0) as u32;
        let y0 = r.1.max(lim.1).floor().max(0.0) as u32;
        let x1 = r.2.min(lim.2).ceil().max(0.0) as u32;
        let y1 = r.3.min(lim.3).ceil().max(0.0) as u32;
        for y in y0..y1 {
            for x in x0..x1 {
                if !in_damage(x, y, damage_rects) {
                    continue;
                }
                let mut n = 0u32;
                for sy in 0..4 {
                    for sx in 0..4 {
                        if inside(x as f32 + (sx as f32 + 0.5) / 4.0, y as f32 + (sy as f32 + 0.5) / 4.0) {
                            n += 1;
                        }
                    }
                }
                match n {
                    0 => {}
                    16 => put_px(surface, width, x, y, color),
                    _ => blend_px(surface, width, x, y, color, (n * 255 / 16) as u8),
                }
            }
        }
    };
    let mid = cy + ch / 2.0;
    let right = cx + cw;
    // the pill
    let rad = (ch / 2.0).min(cw / 2.0);
    let pill = |x: f32, y: f32| {
        let qx = if x < cx + rad { cx + rad - x } else if x > right - rad { x - (right - rad) } else { 0.0 };
        let qy = ((y - mid).abs() - (ch / 2.0 - rad)).max(0.0);
        qx * qx + qy * qy <= rad * rad && y >= cy && y <= cy + ch
    };
    cover(surface, (cx, cy, right, cy + ch), &pill, (241, 243, 244));
    let black = (0, 0, 0);
    // play / pause: Material play_arrow / pause at 20 px (scale 5/6), origin (x+16⅓, mid−10)
    let (ox, oy, k) = (cx + 16.0 + 1.0 / 3.0, mid - 10.0, 20.0 / 24.0);
    if media.playing {
        let bars = move |x: f32, y: f32| {
            let (u, v) = ((x - ox) / k, (y - oy) / k);
            (5.0..=19.0).contains(&v) && ((6.0..=10.0).contains(&u) || (14.0..=18.0).contains(&u))
        };
        cover(surface, (ox, oy, ox + 20.0, oy + 20.0), &bars, black);
    } else {
        let tri = move |x: f32, y: f32| {
            let (u, v) = ((x - ox) / k, (y - oy) / k);
            u >= 8.0 && u <= 19.0 && (v - 12.0).abs() <= 7.0 * (19.0 - u) / 11.0
        };
        cover(surface, (ox, oy, ox + 20.0, oy + 20.0), &tri, black);
    }
    // the time
    let label = format!("{} / {}", crate::media::clock_text(media.pts_ns), crate::media::clock_text(media.duration_ns as i64));
    if let Some(font) = font {
        let fs = 13.0;
        let tw = measure_text_width(&label, &Advancer::new(ui_sel(), fs, 0.0));
        draw_text(&label, cx + 47.0, centered_line_origin_y(font, fs, mid), tw + 2.0, &Advancer::new(ui_sel(), fs, 0.0), 1.2, (31, 31, 31), false, surface, width, height, damage_rects, lim);
    }
    // the timeline
    let (t0, t1) = (cx + 129.0, right - 91.0);
    if t1 > t0 + 4.0 {
        let cap = move |x: f32, y: f32| {
            let dx = if x < t0 + 2.0 { t0 + 2.0 - x } else if x > t1 - 2.0 { x - (t1 - 2.0) } else { 0.0 };
            dx * dx + (y - mid) * (y - mid) <= 4.0 && x >= t0 && x <= t1
        };
        let f = if media.duration_ns > 0 { (media.pts_ns.max(0) as f32 / media.duration_ns as f32).clamp(0.0, 1.0) } else { 0.0 };
        let split = t0 + (t1 - t0) * f;
        cover(surface, (t0, mid - 2.0, t1, mid + 2.0), &move |x, y| cap(x, y) && x >= split, (88, 89, 89));
        if f > 0.0 {
            cover(surface, (t0, mid - 2.0, split, mid + 2.0), &move |x, y| cap(x, y) && x < split, (11, 11, 11));
        }
    }
    // the speaker: Material volume_up at 24 px, origin (right−69, mid−12)
    let (sx, sy) = (right - 69.0, mid - 12.0);
    if sx > t1 {
        let body = move |x: f32, y: f32| {
            let (u, v) = (x - sx, y - sy);
            let rect = (3.0..=7.0).contains(&u) && (9.0..=15.0).contains(&v);
            let cone = (7.0..=12.0).contains(&u) && (v - 12.0).abs() <= 3.0 + (u - 7.0);
            rect || cone
        };
        cover(surface, (sx, sy, sx + 24.0, sy + 24.0), &body, black);
        let waves = move |x: f32, y: f32| {
            let (u, v) = (x - sx, y - sy);
            let r = ((u - 12.0) * (u - 12.0) + (v - 12.0) * (v - 12.0)).sqrt();
            u >= 14.0 && (r <= 4.5 || (7.0..=9.0).contains(&r))
        };
        cover(surface, (sx, sy, sx + 24.0, sy + 24.0), &waves, black);
    }
    // the overflow menu: Material more_vert at 20 px, dots r 5/3 at (10, 5|10|15)
    let (mx, my) = (right - 37.5, mid - 10.5);
    if mx > t1 {
        let dots = move |x: f32, y: f32| {
            let (u, v) = (x - mx - 10.0, y - my);
            [5.0f32, 10.0, 15.0].iter().any(|c| u * u + (v - c) * (v - c) <= (5.0f32 / 3.0) * (5.0 / 3.0))
        };
        cover(surface, (mx, my, mx + 20.0, my + 20.0), &dots, black);
    }
}

pub fn render_frame(
    layout: &LayoutTree,
    surface: &mut [u8],
    width: u32,
    height: u32,
    scroll_x: f64,
    scroll_y: f64,
    damage_rects: &[(u32, u32, u32, u32)],
) {
    if damage_rects.is_empty() {
        return;
    }
    dump_layout(layout);

    // Clear damaged regions to the CANVAS background: the root element's
    // background, else body's (CSS Backgrounds §2.11.2 — the root/body
    // background propagates to the whole canvas, not just their boxes).
    let canvas = canvas_background(layout).unwrap_or((255, 255, 255));
    for &(dx, dy, dw, dh) in damage_rects {
        let ex = (dx + dw).min(width);
        let ey = (dy + dh).min(height);
        for y in dy..ey {
            for x in dx..ex {
                put_px(surface, width, x, y, canvas);
            }
        }
    }

    let font = crate::fonts::face(&FontSel::new(crate::fonts::SANS, 400, false));
    let font_bold = crate::fonts::face(&FontSel::new(crate::fonts::SANS, 700, false));

    let sy = scroll_y as i32;
    let sx = scroll_x as i32;

    /// css-color-4 / css-masking: `opacity` < 1 paints the subtree as a
    /// GROUP and composites it once. Painting the group straight onto the
    /// backdrop and then mixing with a snapshot of the backdrop by the
    /// opacity is exact: per pixel, `b(1-a) + a(g·ga + b(1-ga))` =
    /// `b(1 - a·ga) + a·ga·g`, which is source-over of the group at a·ga.
    #[allow(clippy::too_many_arguments)]
    fn draw_node(
        node_id: NodeId,
        abs_x: f32,
        abs_y: f32,
        inherited: Inherited,
        layout: &LayoutTree,
        font: &Option<&'static Font>,
        font_bold: &Option<&'static Font>,
        surface: &mut [u8],
        width: u32,
        height: u32,
        sx: i32,
        sy: i32,
        damage_rects: &[(u32, u32, u32, u32)],
        clip: Clip,
    ) {
        let op = layout.paint_map.get(&node_id).and_then(|p| p.opacity).unwrap_or(1.0);
        if op > 0.0 && op < 1.0 {
            let snap = surface.to_vec();
            draw_node_inner(
                node_id, abs_x, abs_y, inherited, layout, font, font_bold, surface, width, height, sx, sy,
                damage_rects, clip,
            );
            let a = (op * 256.0).round() as u32;
            for (o, b) in surface.chunks_exact_mut(4).zip(snap.chunks_exact(4)) {
                if o[..3] != b[..3] {
                    for i in 0..3 {
                        o[i] = ((b[i] as u32 * (256 - a) + o[i] as u32 * a) >> 8) as u8;
                    }
                }
            }
        } else {
            draw_node_inner(
                node_id, abs_x, abs_y, inherited, layout, font, font_bold, surface, width, height, sx, sy,
                damage_rects, clip,
            );
        }
    }

    /// One layer of a stacking context (CSS 2.2 Appendix E): a positioned or
    /// stacking-context-forming descendant, hoisted out of normal-flow
    /// painting with what `draw_node` needs to paint it later.
    #[derive(Clone, Copy)]
    struct Layer {
        z: i32,
        order: usize,
        node: NodeId,
        abs_x: f32,
        abs_y: f32,
        inh: Inherited,
        clip: Clip,
    }

    /// Collects the layers of the stacking context rooted at `node` (whose
    /// border box sits at `cur`): every positioned / z-indexed descendant
    /// reached through normal-flow boxes and through positioned z-index:auto
    /// boxes (whose positioned descendants belong to THIS context, §9.9.1),
    /// but not inside a nested stacking context (it paints its own).
    #[allow(clippy::too_many_arguments)]
    fn collect_layers(
        layout: &LayoutTree,
        node: NodeId,
        cur: (f32, f32),
        inh: Inherited,
        clip: Clip,
        sx: i32,
        sy: i32,
        out: &mut Vec<Layer>,
    ) {
        let Ok(lb) = layout.taffy.layout(node) else { return };
        // The children with the parent origin draw_node expects for each.
        let mut kids: Vec<(NodeId, f32, f32, Inherited)> = Vec::new();
        if let Some(il) = layout.inline.get(&node) {
            let cx = cur.0 + lb.border.left + lb.padding.left;
            let cy = cur.1 + lb.border.top + lb.padding.top;
            for f in &il.frags {
                if let crate::layout::inline::Frag::Atomic { node: a, x, y } = f {
                    let loc = layout.taffy.layout(*a).map(|l| l.location).unwrap_or(taffy::Point { x: 0.0, y: 0.0 });
                    kids.push((*a, cx + x - loc.x, cy + y - loc.y, inline_inherited(layout, node, *a, inh)));
                }
            }
            for c in layout.taffy.children(node).unwrap_or_default() {
                if matches!(layout.paint_map.get(&c).and_then(|p| p.position_kind), Some(2 | 3)) {
                    kids.push((c, cur.0, cur.1, inh));
                }
            }
        } else {
            for c in layout.taffy.children(node).unwrap_or_default() {
                kids.push((c, cur.0, cur.1, inh));
            }
        }
        for (c, ax, ay, ci) in kids {
            let p = layout.paint_map.get(&c);
            if p.and_then(|p| p.hidden).unwrap_or(false)
                || layout.taffy.style(c).map(|s| s.display == taffy::style::Display::None).unwrap_or(false)
            {
                continue;
            }
            let Ok(cl) = layout.taffy.layout(c) else { continue };
            let ccur = (ax + cl.location.x, ay + cl.location.y);
            let mut cinh = ci;
            if let Some(n) = layout.node_map.get(&c) {
                if let Some(el) = n.as_element() {
                    let spec = p.cloned().unwrap_or_default();
                    inherit_element(&mut cinh, el.name.local.as_ref(), &spec, n, c);
                }
            }
            let cclip = if p.and_then(|p| p.clip).unwrap_or(false) {
                let (bx0, by0) = (ccur.0 - sx as f32, ccur.1 - sy as f32);
                (clip.0.max(bx0), clip.1.max(by0), clip.2.min(bx0 + cl.size.width), clip.3.min(by0 + cl.size.height))
            } else {
                clip
            };
            if is_layered(layout, c) {
                out.push(Layer { z: z_of(layout, c), order: out.len(), node: c, abs_x: ax, abs_y: ay, inh: ci, clip });
                if is_stacking_context(layout, c) {
                    continue;
                }
            }
            collect_layers(layout, c, ccur, cinh, cclip, sx, sy, out);
        }
    }

    /// The inherited state at `child` (an atomic inline of the IFC rooted at
    /// `root`), applying the inline boxes between them.
    fn inline_inherited(layout: &LayoutTree, root: NodeId, child: NodeId, root_inh: Inherited) -> Inherited {
        let mut path = Vec::new();
        let mut n = layout.taffy.parent(child);
        while let Some(p) = n {
            if p == root {
                break;
            }
            path.push(p);
            n = layout.taffy.parent(p);
        }
        let mut inh = root_inh;
        for p in path.into_iter().rev() {
            if let Some(dn) = layout.node_map.get(&p) {
                if let Some(el) = dn.as_element() {
                    let spec = layout.paint_map.get(&p).cloned().unwrap_or_default();
                    inherit_element(&mut inh, el.name.local.as_ref(), &spec, dn, p);
                }
            }
        }
        inh.shift_y = 0.0;
        inh
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_node_inner(
        node_id: NodeId,
        abs_x: f32,
        abs_y: f32,
        inherited: Inherited,
        layout: &LayoutTree,
        font: &Option<&'static Font>,
        font_bold: &Option<&'static Font>,
        surface: &mut [u8],
        width: u32,
        height: u32,
        sx: i32,
        sy: i32,
        damage_rects: &[(u32, u32, u32, u32)],
        clip: Clip,
    ) {
        // display:none subtrees exist in the tree with zero-size boxes;
        // they must not paint (their text would smear at the parent origin).
        if layout
            .taffy
            .style(node_id)
            .map(|s| s.display == taffy::style::Display::None)
            .unwrap_or(false)
        {
            return;
        }
        // visibility:hidden / opacity:0 keep their space but paint nothing;
        // approximation: the whole subtree skips (no visibility:visible
        // re-reveal inside a hidden ancestor).
        if layout
            .paint_map
            .get(&node_id)
            .and_then(|p| p.hidden)
            .unwrap_or(false)
        {
            return;
        }
        let Ok(layout_box) = layout.taffy.layout(node_id) else { return };
        let mut current_x = abs_x + layout_box.location.x;
        let mut current_y = abs_y + layout_box.location.y;
        // position: fixed — the containing block is the viewport (CSS 2.2
        // §10.1): insets resolve against it and the box ignores scrolling.
        if layout.paint_map.get(&node_id).and_then(|p| p.position_kind) == Some(3) {
            if let Ok(st) = layout.taffy.style(node_id) {
                let len = |v: taffy::style::LengthPercentageAuto, basis: f32| -> Option<f32> {
                    let raw = v.into_raw();
                    match raw.tag() {
                        taffy::style::CompactLength::LENGTH_TAG => Some(raw.value()),
                        taffy::style::CompactLength::PERCENT_TAG => Some(raw.value() * basis),
                        _ => None,
                    }
                };
                let (vw, vh) = (width as f32, height as f32);
                let (w, h) = (layout_box.size.width, layout_box.size.height);
                if let Some(l) = len(st.inset.left, vw) {
                    current_x = sx as f32 + l;
                } else if let Some(r) = len(st.inset.right, vw) {
                    current_x = sx as f32 + vw - r - w;
                }
                if let Some(t) = len(st.inset.top, vh) {
                    current_y = sy as f32 + t;
                } else if let Some(b) = len(st.inset.bottom, vh) {
                    current_y = sy as f32 + vh - b - h;
                }
            }
        }

        let mut inherited = inherited;

        if let Some(dom_node) = layout.node_map.get(&node_id) {
            let spec = layout.paint_map.get(&node_id).cloned().unwrap_or_default();

            if let Some(el) = dom_node.as_element() {
                let tag = el.name.local.as_ref();
                inherit_element(&mut inherited, tag, &spec, dom_node, node_id);

                let bw = layout_box.size.width.max(0.0);
                let bh = layout_box.size.height.max(0.0);
                // Box-local origin in screen space (may be negative when
                // scrolled past; the painted span is clamped, the local
                // coordinate is not — that is what keeps a positioned
                // background aligned while scrolling).
                let box_sx = current_x - sx as f32;
                let box_sy = current_y - sy as f32;
                let x_start = (box_sx.max(0.0)) as u32;
                let y_start = (box_sy.max(0.0)) as u32;
                let end_x = ((box_sx + bw).max(0.0) as u32).min(width);
                let end_y = ((box_sy + bh).max(0.0) as u32).min(height);

                // mask-image is an alpha stencil over this box's background
                // paint: the fill only lands where the mask is opaque. A
                // declared-but-unresolvable mask suppresses the fill —
                // painting the raw box would show a solid blob where the
                // page means an icon glyph.
                let mask = spec.mask_image.as_ref().map(|url| {
                    crate::images::get(url).map(|img| {
                        let g = resolve_bg_geometry(
                            spec.mask_size.as_deref(),
                            spec.mask_position.as_deref(),
                            spec.mask_repeat,
                            bw,
                            bh,
                            img.width() as f32,
                            img.height() as f32,
                        );
                        (img, g)
                    })
                });
                let mask_alpha = |x: u32, y: u32| -> u8 {
                    match &mask {
                        None => 255,
                        Some(None) => 0,
                        Some(Some((img, g))) => {
                            let (lx, ly) = (x as f32 - box_sx, y as f32 - box_sy);
                            match g.sample(lx, ly, img.width(), img.height()) {
                                Some((u, v)) => img.get_pixel(u, v).0[3],
                                None => 0,
                            }
                        }
                    }
                };
                if matches!(mask, Some(None)) {
                    crate::ledger::record_css("mask-image-unresolved");
                }

                // Rounded corners or a patterned border: the shape painter
                // (render/boxpaint.rs) owns the background and the border.
                let fancy = spec.radius.is_some_and(|r| r.iter().any(|&v| v != 0.0))
                    || spec.border_style.is_some_and(|s| s.iter().any(|&v| v != 0));
                let mut blend_at = |x: u32, y: u32, c: (u8, u8, u8), a: f32| {
                    if !in_damage(x, y, damage_rects) || !in_clip(x, y, clip) {
                        return;
                    }
                    if a >= 0.999 {
                        put_px(surface, width, x, y, c);
                    } else if a > 0.0 {
                        blend_px(surface, width, x, y, c, (a * 255.0).round() as u8);
                    }
                };
                // Outer box shadows paint under the box, outside it.
                if let Some(sh) = spec.shadows.as_ref().filter(|s| !s.is_empty()) {
                    let radii = boxpaint::used_radii(spec.radius.unwrap_or([0.0; 4]), bw, bh);
                    effects::paint_shadows(sh, box_sx, box_sy, bw, bh, radii, width, height, &boxpaint::sdf, &mut blend_at);
                }
                // css-color-4: a translucent background composites source-over.
                let bga = spec.bg_alpha.unwrap_or(1.0).clamp(0.0, 1.0);
                let background = spec.background.filter(|_| bga > 0.0);
                if fancy && mask.is_none() && background.is_some() {
                    let mut blend_a = |x: u32, y: u32, c: (u8, u8, u8), a: f32| blend_at(x, y, c, a * bga);
                    boxpaint::paint(
                        box_sx, box_sy, bw, bh, spec.radius.unwrap_or([0.0; 4]),
                        [None; 4], background, width, height, &mut blend_a,
                    );
                } else if let Some(bg) = background {
                    let ba = (bga * 255.0).round() as u32;
                    for y in y_start..end_y {
                        for x in x_start..end_x {
                            if !in_damage(x, y, damage_rects) || !in_clip(x, y, clip) {
                                continue;
                            }
                            match mask_alpha(x, y) as u32 * ba / 255 {
                                0 => {}
                                255 => put_px(surface, width, x, y, bg),
                                a => blend_px(surface, width, x, y, bg, a as u8),
                            }
                        }
                    }
                }

                if let Some(Some(g)) = spec.bg_gradient.as_ref() {
                    let radii = boxpaint::used_radii(spec.radius.unwrap_or([0.0; 4]), bw, bh);
                    let (gx, gy) = (box_sx, box_sy);
                    let cover = move |fx: f32, fy: f32| (0.5 - boxpaint::sdf(fx, fy, gx, gy, gx + bw, gy + bh, radii)).clamp(0.0, 1.0);
                    let mut blend_g = |x: u32, y: u32, c: (u8, u8, u8), a: f32| {
                        if !in_damage(x, y, damage_rects) || !in_clip(x, y, clip) {
                            return;
                        }
                        if a >= 0.999 {
                            put_px(surface, width, x, y, c);
                        } else if a > 0.0 {
                            blend_px(surface, width, x, y, c, (a * 255.0).round() as u8);
                        }
                    };
                    effects::paint_gradient(g, box_sx, box_sy, bw, bh, width, height, &cover, &mut blend_g);
                }
                // background-image paints over the color, under content,
                // honouring background-size / -position / -repeat (the
                // sprite-sheet idiom is exactly a positioned no-repeat
                // layer of an intrinsically-sized sheet).
                if let Some(img) = spec.bg_image.as_deref().and_then(crate::images::get) {
                    let g = resolve_bg_geometry(
                        spec.bg_size.as_deref(),
                        spec.bg_position.as_deref(),
                        spec.bg_repeat,
                        bw,
                        bh,
                        img.width() as f32,
                        img.height() as f32,
                    );
                    for y in y_start..end_y {
                        for x in x_start..end_x {
                            if !in_damage(x, y, damage_rects) || !in_clip(x, y, clip) {
                                continue;
                            }
                            let Some((u, v)) =
                                g.sample(x as f32 - box_sx, y as f32 - box_sy, img.width(), img.height())
                            else {
                                continue;
                            };
                            let [r, gr, b, a] = img.get_pixel(u, v).0;
                            let a = (a as u32 * mask_alpha(x, y) as u32 / 255) as u8;
                            if a > 0 {
                                blend_px(surface, width, x, y, (r, gr, b), a);
                            }
                        }
                    }
                }

                // Inset box shadows (§7.1.3): over the background, under the
                // border and content, inside the padding box.
                if let Some(sh) = spec.shadows.as_ref().filter(|s| s.iter().any(|s| s.inset)) {
                    let (bl, bt) = (layout_box.border.left, layout_box.border.top);
                    let (br, bb) = (layout_box.border.right, layout_box.border.bottom);
                    let outer = boxpaint::used_radii(spec.radius.unwrap_or([0.0; 4]), bw, bh);
                    let inner = [
                        (outer[0] - bl.max(bt)).max(0.0),
                        (outer[1] - br.max(bt)).max(0.0),
                        (outer[2] - br.max(bb)).max(0.0),
                        (outer[3] - bl.max(bb)).max(0.0),
                    ];
                    let mut blend_i = |x: u32, y: u32, c: (u8, u8, u8), a: f32| {
                        if !in_damage(x, y, damage_rects) || !in_clip(x, y, clip) {
                            return;
                        }
                        if a >= 0.999 {
                            put_px(surface, width, x, y, c);
                        } else if a > 0.0 {
                            blend_px(surface, width, x, y, c, (a * 255.0).round() as u8);
                        }
                    };
                    effects::paint_inset_shadows(
                        sh, box_sx + bl, box_sy + bt, (bw - bl - br).max(0.0), (bh - bt - bb).max(0.0), inner,
                        width, height, &boxpaint::sdf, &mut blend_i,
                    );
                }

                if tag == "img" {
                    let src = crate::images::effective_img_src(&el.attributes.borrow());
                    if let Some(img) = src.as_deref().and_then(crate::images::get) {
                        // The painted SPAN clamps to the viewport; the box
                        // ORIGIN must not. Clamping the origin re-anchors a
                        // scrolled-off image to the viewport edge, so it
                        // stops moving with the page — and an incremental
                        // scroll (shift-and-repaint-the-strip) then leaves a
                        // train of copies down the viewport.
                        let bw = layout_box.size.width.max(1.0);
                        let bh = layout_box.size.height.max(1.0);
                        let x_start = box_sx.max(0.0) as u32;
                        let y_start = box_sy.max(0.0) as u32;
                        let end_x = ((box_sx + bw).max(0.0) as u32).min(width);
                        let end_y = ((box_sy + bh).max(0.0) as u32).min(height);
                        for y in y_start..end_y {
                            for x in x_start..end_x {
                                if !in_damage(x, y, damage_rects) || !in_clip(x, y, clip) {
                                    continue;
                                }
                                // Nearest-neighbor sample into the layout box.
                                let u = ((x as f32 - box_sx) / bw * img.width() as f32).max(0.0) as u32;
                                let v = ((y as f32 - box_sy) / bh * img.height() as f32).max(0.0) as u32;
                                let px = img.get_pixel(u.min(img.width() - 1), v.min(img.height() - 1));
                                let [r, g, b, a] = px.0;
                                if a > 0 {
                                    blend_px(surface, width, x, y, (r, g, b), a);
                                }
                            }
                        }
                    }
                }

                if tag == "video" || tag == "audio" {
                    if let Some(media) = crate::media::paint_for(dom_node) {
                        let (pad, bord) = (layout_box.padding, layout_box.border);
                        let content = (
                            box_sx + pad.left + bord.left,
                            box_sy + pad.top + bord.top,
                            (bw - pad.left - bord.left - pad.right - bord.right).max(0.0),
                            (bh - pad.top - bord.top - pad.bottom - bord.bottom).max(0.0),
                        );
                        paint_media(&media, &spec, content, font, surface, width, height, damage_rects, clip);
                    }
                }

                // Form controls get a UA border and their value text.
                let is_control = matches!(tag, "input" | "textarea" | "select" | "button");
                let kind = crate::layout::control_kind(dom_node);
                let checkable = matches!(kind, Some(crate::layout::Control::Checkbox | crate::layout::Control::Radio));
                // Chromium's control theme paints a 1px #767676 frame at the
                // outer edge of the (2px) UA border; checkboxes and radios
                // are drawn whole below.
                let mut border = spec.border.or(if is_control && !checkable {
                    Some([Some((1.0, (118, 118, 118))); 4])
                } else {
                    None
                });
                // Apply border-*-width overrides if present
                if let Some(sides) = &mut border {
                    if let Some(widths) = &spec.border_width {
                        for (i, width) in widths.iter().enumerate() {
                            if let Some(w) = width {
                                if let Some((_, c)) = sides[i] {
                                    sides[i] = Some((*w, c));
                                }
                            }
                        }
                    }
                }
                // Content box in screen space — the label/value area, inside
                // the UA border and any author padding.
                let (pad, bord) = (layout_box.padding, layout_box.border);
                let inset_l = pad.left + bord.left;
                let inset_r = pad.right + bord.right;
                let content_x = box_sx + inset_l;
                let content_y = box_sy + pad.top + bord.top;
                let content_w = (bw - inset_l - inset_r).max(0.0);
                let content_h = (bh - pad.top - bord.top - pad.bottom - bord.bottom).max(0.0);
                // A push button's label centres on both axes; a text field's
                // value stays left-aligned but rides the vertical middle,
                // which is what makes native fields look "in" their box.
                let is_push_button = tag == "button"
                    || (tag == "input"
                        && el
                            .attributes
                            .borrow()
                            .get("type")
                            .map(|t| {
                                let t = t.to_ascii_lowercase();
                                matches!(t.as_str(), "submit" | "button" | "reset")
                            })
                            .unwrap_or(false));
                if is_push_button {
                    inherited.center_box = Some((content_x, content_y, content_w, content_h));
                }

                if checkable {
                    let checked = el.attributes.borrow().get("checked").is_some();
                    let radio = kind == Some(crate::layout::Control::Radio);
                    paint_checkable(box_sx, box_sy, bw, bh, radio, checked, surface, width, height, damage_rects, clip);
                }
                if kind == Some(crate::layout::Control::Select) {
                    // The drop-down arrow: a 2px chevron 9px wide, centred
                    // vertically, its right end 8px inside the right edge.
                    let cx = box_sx + bw - 8.0 - 4.5;
                    let cy = box_sy + bh / 2.0;
                    for i in 0..=8 {
                        let dx = i as f32 - 4.0;
                        let y = cy - 2.0 + (4.0 - dx.abs());
                        for t in 0..2 {
                            let (x, yy) = ((cx + dx).round(), (y + t as f32).round());
                            if x >= 0.0 && yy >= 0.0 && (x as u32) < width && (yy as u32) < height {
                                let (xu, yu) = (x as u32, yy as u32);
                                if in_damage(xu, yu, damage_rects) && in_clip(xu, yu, clip) {
                                    put_px(surface, width, xu, yu, (0, 0, 0));
                                }
                            }
                        }
                    }
                }
                if tag == "textarea" {
                    // The value: the element's text, pre-wrap, from the
                    // content box's top-left, in the monospace control font.
                    let text = dom_node.text_contents();
                    let fs = inherited.font_size;
                    let _ = fs;
                    let mode = crate::fonts::lines::TextMode { white_space: 3, ..Default::default() };
                    draw_lines(
                        text.trim_start_matches('\n'), content_x, content_y, content_w.max(1.0), &inherited.advancer(),
                        0.0, spec.color.unwrap_or((0, 0, 0)), &mode, Deco::default(), &[],
                        surface, width, height, damage_rects, clip,
                    );
                }
                if is_control && tag != "button" && !checkable && tag != "textarea" {
                    let face = crate::fonts::face(&inherited.sel());
                    if let Some(font) = face.or(*font) {
                        let adv = inherited.advancer();
                        // A value script (or the user) set is the control's value (the dirty value
                        // flag, HTML §4.10.5.4); the attribute is only its default.
                        let dirty = crate::js::control_value(dom_node);
                        let select_label = (tag == "select")
                            .then(|| crate::layout::select_selected_option(dom_node).map(|(_, l)| l))
                            .flatten();
                        let attrs = el.attributes.borrow();
                        // A <select> paints the SELECTED OPTION'S TEXT, not
                        // its submit value: layout publishes the visible label
                        // as data-aether-label, so "English" shows where the
                        // value attribute would only say "en".
                        let shown = select_label
                            .as_deref()
                            .filter(|v| !v.is_empty())
                            .or_else(|| dirty.as_deref())
                            .or_else(|| attrs.get("value"))
                            .filter(|v| !v.is_empty());
                        let is_placeholder = shown.is_none();
                        let password = attrs.get("type").is_some_and(|t| t.trim().eq_ignore_ascii_case("password"));
                        let mut value = shown.or_else(|| attrs.get("placeholder")).unwrap_or("").to_string();
                        if password && !is_placeholder {
                            value = "\u{2022}".repeat(value.chars().count());
                        }
                        drop(attrs);
                        // Field text is black (FieldText), a placeholder
                        // #757575 (html.css ::placeholder darkGray).
                        let value_color = if is_placeholder { (117, 117, 117) } else { spec.color.unwrap_or((0, 0, 0)) };
                        if !value.is_empty() {
                            let fs = inherited.font_size;
                            let text_w = measure_text_width(&value, &adv);
                            // <input>/<select> are single-line by definition —
                            // their value rides the vertical middle however
                            // tall the author makes the box, which is what
                            // native fields do. A <textarea> is a multi-line
                            // edit surface: its text starts at the top.
                            let single_line = tag != "textarea" && content_h > 0.0;
                            let (tx, ty, max_w) = if is_push_button && text_w <= content_w {
                                (
                                    content_x + (content_w - text_w) / 2.0,
                                    centered_line_origin_y(font, fs, content_y + content_h / 2.0),
                                    content_w.max(1.0),
                                )
                            } else {
                                // The value's line box is centred in the
                                // content box (a single-line field).
                                let lh = crate::fonts::line_height(font, fs, 0.0);
                                let ty = if single_line {
                                    content_y + ((content_h - lh) / 2.0).floor()
                                } else {
                                    content_y
                                };
                                let left = if tag == "select" { content_x + 4.0 } else { content_x };
                                (left, ty, f32::MAX)
                            };
                            draw_text(
                                &value,
                                tx,
                                ty,
                                max_w,
                                &adv,
                                0.0,
                                value_color,
                                false,
                                surface,
                                width,
                                height,
                                damage_rects,
                                clip,
                            );
                        }
                    }
                }

                if let (true, Some(sides)) = (fancy, border) {
                    let styles = spec.border_style.unwrap_or([0; 4]);
                    let mut bs: [boxpaint::Side; 4] = [None; 4];
                    for i in 0..4 {
                        bs[i] = sides[i].filter(|(w, _)| *w > 0.0).map(|(w, c)| (w, c, styles[i]));
                    }
                    let bda = spec.border_alpha.unwrap_or(1.0);
                    let mut blend_at = |x: u32, y: u32, c: (u8, u8, u8), a: f32| {
                        let a = a * bda;
                        if !in_damage(x, y, damage_rects) || !in_clip(x, y, clip) {
                            return;
                        }
                        if a >= 0.999 {
                            put_px(surface, width, x, y, c);
                        } else if a > 0.0 {
                            blend_px(surface, width, x, y, c, (a * 255.0).round() as u8);
                        }
                    };
                    boxpaint::paint(
                        box_sx, box_sy, bw, bh, spec.radius.unwrap_or([0.0; 4]),
                        bs, None, width, height, &mut blend_at,
                    );
                } else if let Some(sides) = border {
                    // Same rule as the background/image spans: x_start..end_x
                    // is the CLAMPED screen span of the unclamped box origin
                    // (box_sx/box_sy). Re-deriving the end from a clamped
                    // start pins a scrolled-off box's borders to the viewport
                    // edge and repeats them on every scroll strip.
                    // [top, right, bottom, left], each side its own stroke.
                    // Sides are expressed in unclamped screen floats and the
                    // stroke clips them into the viewport, so a box whose
                    // bottom edge lies below the fold does not stamp its
                    // bottom border across the last visible row.
                    let (bx0, by0) = (box_sx, box_sy);
                    let (bx1, by1) = (box_sx + bw, box_sy + bh);
                    let bda = (spec.border_alpha.unwrap_or(1.0).clamp(0.0, 1.0) * 255.0).round() as u8;
                    let mut stroke = |x0: f32, y0: f32, x1: f32, y1: f32, color: (u8, u8, u8)| {
                        let x0 = x0.max(0.0) as u32;
                        let y0 = y0.max(0.0) as u32;
                        let x1 = (x1.max(0.0) as u32).min(width);
                        let y1 = (y1.max(0.0) as u32).min(height);
                        for y in y0..y1 {
                            for x in x0..x1 {
                                if in_damage(x, y, damage_rects) && in_clip(x, y, clip) {
                                    if bda == 255 {
                                        put_px(surface, width, x, y, color);
                                    } else if bda > 0 {
                                        blend_px(surface, width, x, y, color, bda);
                                    }
                                }
                            }
                        }
                    };
                    // A zero-width side draws NOTHING. `border-width: 0`
                    // with a style and colour still set is the single most
                    // common declaration on the web (every Tailwind/reset
                    // preflight opens with it); rounding it up to a hairline
                    // outlines every box on the page.
                    let px = |w: f32| if w > 0.0 { w.max(1.0) } else { 0.0 };
                    if let Some((w, c)) = sides[0].filter(|(w, _)| *w > 0.0) {
                        stroke(bx0, by0, bx1, by0 + px(w), c);
                    }
                    if let Some((w, c)) = sides[2].filter(|(w, _)| *w > 0.0) {
                        stroke(bx0, by1 - px(w), bx1, by1, c);
                    }
                    if let Some((w, c)) = sides[3].filter(|(w, _)| *w > 0.0) {
                        stroke(bx0, by0, bx0 + px(w), by1, c);
                    }
                    if let Some((w, c)) = sides[1].filter(|(w, _)| *w > 0.0) {
                        stroke(bx1 - px(w), by0, bx1, by1, c);
                    }
                }
                // css-lists-3 §3.1: an outside ::marker for a list item,
                // its first line's baseline, ending at the content edge.
                let is_item = spec.list_item.unwrap_or(tag == "li");
                if is_item && inherited.list_style != 0 && !inherited.text_hidden {
                    if let Some(f) = crate::fonts::face(&inherited.sel()) {
                        let fs = inherited.font_size;
                        let base = match layout.inline.get(&node_id).and_then(|l| l.first_baseline) {
                            Some(b) => content_y + b,
                            None => content_y + crate::fonts::baseline_offset(f, fs, inherited.line_height),
                        };
                        // ::marker text is unaffected by the item's letter/word spacing
                        let adv = Advancer::new(inherited.sel(), fs, 0.0).with_dir(inherited.rtl);
                        paint_marker(
                            dom_node, inherited.list_style, content_x, base.round(), &adv,
                            inherited.color, surface, width, height, damage_rects, clip,
                        );
                    }
                }
            } else if dom_node.as_text().is_some() && !inherited.text_hidden {
                // The same face the measurer wrapped this run with
                // (fonts::face): family x weight x style. The preloaded sans
                // pair is only the fallback when no face loads at all.
                let face = crate::fonts::face(&inherited.sel());
                let font = face.or(if inherited.weight >= 600 { *font_bold } else { *font });
                if let Some(font) = font {
                    let text = dom_node.text_contents();
                    if !text.trim().is_empty() || inherited.white_space >= 2 || layout.paint_map.get(&node_id).and_then(|p| p.ws_lead) == Some(true) {
                        let text = transform_text(&text, inherited.text_transform);
                        let spec_t = layout.paint_map.get(&node_id);
                        let mut mode = crate::fonts::lines::TextMode {
                            white_space: inherited.white_space,
                            word_break: inherited.word_break,
                            overflow_wrap: inherited.overflow_wrap,
                            letter_spacing: inherited.letter_spacing,
                            word_spacing: inherited.word_spacing,
                            rtl: inherited.rtl,
                            lead: spec_t.and_then(|p| p.ws_lead).unwrap_or(false),
                            trail: spec_t.and_then(|p| p.ws_trail).unwrap_or(false),
                        };
                        let adv = inherited.advancer();
                        if inherited.nowrap && mode.white_space == 0 {
                            mode.white_space = 1;
                        }
                        // A <button>'s label is an ordinary text child, so it
                        // is centred here against the ancestor control's
                        // content box — but only when the measured run fits on
                        // one line inside it, so a button wrapping rich or
                        // overflowing content keeps normal flow painting.
                        let centered = inherited.center_box.and_then(|(cx, cy, cw, ch)| {
                            let tw = crate::fonts::lines::break_lines(&adv, &text, &mode, f32::MAX)[0].width;
                            (tw <= cw && cw > 0.0).then(|| {
                                (
                                    cx + (cw - tw) / 2.0,
                                    centered_line_origin_y(font, inherited.font_size, cy + ch / 2.0),
                                    cw.max(1.0),
                                )
                            })
                        });
                        // Break at the UNROUNDED box width the measurer saw
                        // (the rounded one can be a fraction narrower).
                        let wrap_w = layout
                            .taffy
                            .unrounded_layout(node_id)
                            .size
                            .width
                            .max(layout_box.size.width)
                            + 0.01;
                        let (tx, ty, max_w) = centered.unwrap_or((
                            current_x - sx as f32,
                            current_y - sy as f32 + inherited.shift_y,
                            if inherited.nowrap { f32::MAX } else { wrap_w.max(1.0) },
                        ));
                        draw_lines(
                            &text,
                            tx,
                            ty,
                            max_w,
                            &adv,
                            inherited.line_height,
                            inherited.color,
                            &mode,
                            Deco { underline: inherited.underline, line_through: inherited.line_through },
                            inherited.shadows(layout),
                            surface,
                            width,
                            height,
                            damage_rects,
                            clip,
                        );
                    }
                }
            }
        }

        // overflow != visible: children clip to this box's screen rect.
        let ifc = layout.inline.get(&node_id);
        let child_clip = if layout
            .paint_map
            .get(&node_id)
            .and_then(|p| p.clip)
            .unwrap_or(false)
        {
            let bx0 = current_x - sx as f32;
            let by0 = current_y - sy as f32;
            (
                clip.0.max(bx0),
                clip.1.max(by0),
                clip.2.min(bx0 + layout_box.size.width.max(0.0)),
                clip.3.min(by0 + layout_box.size.height.max(0.0)),
            )
        } else {
            clip
        };
        // overflow clipping to a ROUNDED box (css-backgrounds-3 §5.3): the
        // children paint against the rectangular clip, then every pixel of
        // the box outside its rounded padding box returns to the backdrop
        // (mixed by coverage at the curve) — exact for the corners.
        let round_clip = layout
            .paint_map
            .get(&node_id)
            .filter(|p| p.clip.unwrap_or(false) && p.radius.is_some_and(|r| r.iter().any(|&v| v != 0.0)))
            .map(|p| {
                let (bw, bh) = (layout_box.size.width, layout_box.size.height);
                let outer = boxpaint::used_radii(p.radius.unwrap_or([0.0; 4]), bw, bh);
                let b = layout_box.border;
                let inner = [
                    (outer[0] - b.left.max(b.top)).max(0.0),
                    (outer[1] - b.right.max(b.top)).max(0.0),
                    (outer[2] - b.right.max(b.bottom)).max(0.0),
                    (outer[3] - b.left.max(b.bottom)).max(0.0),
                ];
                let (x0, y0) = (current_x - sx as f32 + b.left, current_y - sy as f32 + b.top);
                ((x0, y0, x0 + bw - b.left - b.right, y0 + bh - b.top - b.bottom), inner, surface.to_vec())
            });
        let finish_round_clip = |surface: &mut [u8]| {
            if let Some(((x0, y0, x1, y1), r, snap)) = &round_clip {
                let (ax, ay) = (x0.floor().max(0.0) as u32, y0.floor().max(0.0) as u32);
                let (bx, by) = ((x1.ceil().max(0.0) as u32).min(width), (y1.ceil().max(0.0) as u32).min(height));
                for y in ay..by {
                    for x in ax..bx {
                        let cov = (0.5 - boxpaint::sdf(x as f32 + 0.5, y as f32 + 0.5, *x0, *y0, *x1, *y1, *r)).clamp(0.0, 1.0);
                        if cov >= 1.0 {
                            continue;
                        }
                        let i = ((y * width + x) * 4) as usize;
                        for k in 0..3 {
                            surface[i + k] = (snap[i + k] as f32 * (1.0 - cov) + surface[i + k] as f32 * cov).round() as u8;
                        }
                    }
                }
            }
        };
        // A stacking context (Appendix E): its layers, collected through the
        // boxes it paints in normal flow; negative z-index under that flow.
        let mut layers: Vec<Layer> = Vec::new();
        if node_id == layout.root_node || is_stacking_context(layout, node_id) {
            collect_layers(layout, node_id, (current_x, current_y), inherited, child_clip, sx, sy, &mut layers);
            layers.sort_by_key(|l| (l.z, l.order));
        }
        let paint_layers = |layers: &[Layer], surface: &mut [u8]| {
            for l in layers {
                draw_node(
                    l.node, l.abs_x, l.abs_y, l.inh, layout, font, font_bold, surface, width, height, sx, sy,
                    damage_rects, l.clip,
                );
            }
        };
        let split = layers.iter().position(|l| l.z >= 0).unwrap_or(layers.len());
        paint_layers(&layers[..split], surface);
        if child_clip.2 <= child_clip.0 || child_clip.3 <= child_clip.1 {
            paint_layers(&layers[split..], surface);
            finish_round_clip(surface);
            return; // fully clipped out — nothing below can paint
        }
        if let Some(ifc) = ifc {
            // An inline formatting context: its line-box fragments, in tree
            // order (CSS 2.2 Appendix E step 7: each inline box's background
            // and borders, then its text; atomic inlines as whole boxes).
            let content_x = current_x + layout_box.border.left + layout_box.padding.left;
            // A table cell's vertical-align places its lines in the cell
            // (§17.5.4; html.css: middle).
            let valign_off = cell_valign_offset(layout, node_id, ifc.height);
            let content_y = current_y + layout_box.border.top + layout_box.padding.top + valign_off;
            // Text baselines snap to whole pixels from the EXACT (unrounded)
            // position, as Skia rounds a glyph run's fractional origin; the
            // box tree's rounded block positions can be up to half a pixel
            // off it. (Box decorations stay on the rounded geometry.)
            let exact_content_y = {
                // The rounding error along the box's ancestor chain.
                let mut err = 0.0f32;
                let mut n = node_id;
                loop {
                    err += layout.taffy.unrounded_layout(n).location.y
                        - layout.taffy.layout(n).map(|l| l.location.y).unwrap_or(0.0);
                    match layout.taffy.parent(n) {
                        Some(p) => n = p,
                        None => break,
                    }
                }
                let u = layout.taffy.unrounded_layout(node_id);
                content_y + err + (u.border.top + u.padding.top) - (layout_box.border.top + layout_box.padding.top)
            };
            // The inherited state of every inline box in the context.
            let mut inh: std::collections::HashMap<NodeId, Inherited> = std::collections::HashMap::new();
            let mut skip: std::collections::HashSet<NodeId> = std::collections::HashSet::new();
            fn walk_inh(
                layout: &LayoutTree,
                id: NodeId,
                parent: Inherited,
                hidden: bool,
                inh: &mut std::collections::HashMap<NodeId, Inherited>,
                skip: &mut std::collections::HashSet<NodeId>,
            ) {
                for k in layout.taffy.children(id).unwrap_or_default() {
                    let mut i = parent;
                    let spec = layout.paint_map.get(&k).cloned().unwrap_or_default();
                    let hid = hidden || spec.hidden.unwrap_or(false);
                    if hid {
                        skip.insert(k);
                    }
                    let mut recurse = false;
                    if let Some(n) = layout.node_map.get(&k) {
                        if let Some(el) = n.as_element() {
                            let tag = el.name.local.as_ref();
                            // Atomic boxes compute their own state in draw_node.
                            inh.insert(k, parent);
                            if !crate::layout::inline::is_atomic_pub(layout, k) {
                                inherit_element(&mut i, tag, &spec, n, k);
                                i.shift_y = 0.0;
                                inh.insert(k, i);
                                recurse = true;
                            }
                        } else {
                            inh.insert(k, parent);
                        }
                    } else {
                        inh.insert(k, parent);
                    }
                    if recurse {
                        walk_inh(layout, k, i, hid, inh, skip);
                    }
                }
            }
            let mut root_inh = inherited;
            root_inh.shift_y = 0.0;
            walk_inh(layout, node_id, root_inh, false, &mut inh, &mut skip);
            for frag in &ifc.frags {
                match frag {
                    crate::layout::inline::Frag::Box { node, x, y, w, h, first, last } => {
                        if skip.contains(node) {
                            continue;
                        }
                        let Some(spec) = layout.paint_map.get(node) else { continue };
                        let fx = content_x + x - sx as f32;
                        let fy = content_y + y - sy as f32;
                        let mut sides: [boxpaint::Side; 4] = [None; 4];
                        if let Some(b) = spec.border {
                            let styles = spec.border_style.unwrap_or([0; 4]);
                            for i in 0..4 {
                                let mut side = b[i];
                                if let (Some(ws), Some((_, c))) = (spec.border_width.as_ref(), side) {
                                    if let Some(w) = ws[i] {
                                        side = Some((w, c));
                                    }
                                }
                                sides[i] = side.filter(|(w, _)| *w > 0.0).map(|(w, c)| (w, c, styles[i]));
                            }
                        }
                        // box-decoration-break: slice — the inline-start edge
                        // only on the first fragment, the end on the last.
                        if !*first {
                            sides[3] = None;
                        }
                        if !*last {
                            sides[1] = None;
                        }
                        let mut radius = spec.radius.unwrap_or([0.0; 4]);
                        if !*first {
                            radius[0] = 0.0;
                            radius[3] = 0.0;
                        }
                        if !*last {
                            radius[1] = 0.0;
                            radius[2] = 0.0;
                        }
                        let rounded = radius.iter().any(|&r| r != 0.0)
                            || sides.iter().any(|s| s.is_some_and(|s| s.2 != 0));
                        let bga = spec.bg_alpha.unwrap_or(1.0);
                        let bda = spec.border_alpha.unwrap_or(1.0);
                        let bg_rgb = spec.background;
                        let mut blend_at = |x: u32, y: u32, c: (u8, u8, u8), a: f32| {
                            // Background and border colours composite at their alpha.
                            let a = a * if Some(c) == bg_rgb { bga } else { bda };
                            if !in_damage(x, y, damage_rects) || !in_clip(x, y, child_clip) {
                                return;
                            }
                            if a >= 0.999 {
                                put_px(surface, width, x, y, c);
                            } else if a > 0.0 {
                                blend_px(surface, width, x, y, c, (a * 255.0).round() as u8);
                            }
                        };
                        if rounded {
                            boxpaint::paint(fx, fy, *w, *h, radius, sides, spec.background, width, height, &mut blend_at);
                        } else {
                            // Pixel-snapped rectangles, as Blink snaps box decorations.
                            let (x0, y0) = (fx.round(), fy.round());
                            let (x1, y1) = ((fx + w).round(), (fy + h).round());
                            let mut fill = |ax: f32, ay: f32, bx: f32, by: f32, c: (u8, u8, u8)| {
                                let (ax, ay) = (ax.max(0.0) as u32, ay.max(0.0) as u32);
                                let (bx, by) = ((bx.max(0.0) as u32).min(width), (by.max(0.0) as u32).min(height));
                                for yy in ay..by {
                                    for xx in ax..bx {
                                        blend_at(xx, yy, c, 1.0);
                                    }
                                }
                            };
                            if let Some(bg) = spec.background {
                                fill(x0, y0, x1, y1, bg);
                            }
                            if let Some((bw, c, _)) = sides[0] {
                                fill(x0, y0, x1, y0 + bw.round().max(1.0), c);
                            }
                            if let Some((bw, c, _)) = sides[2] {
                                fill(x0, y1 - bw.round().max(1.0), x1, y1, c);
                            }
                            if let Some((bw, c, _)) = sides[3] {
                                fill(x0, y0, x0 + bw.round().max(1.0), y1, c);
                            }
                            if let Some((bw, c, _)) = sides[1] {
                                fill(x1 - bw.round().max(1.0), y0, x1, y1, c);
                            }
                        }
                    }
                    crate::layout::inline::Frag::Text { node, text, x, baseline, .. } => {
                        if skip.contains(node) {
                            continue;
                        }
                        let Some(i) = inh.get(node).copied() else { continue };
                        if i.text_hidden {
                            continue;
                        }
                        draw_glyph_run(
                            text,
                            content_x + x - sx as f32,
                            (exact_content_y + baseline).round() - sy as f32,
                            &i.advancer(),
                            i.color,
                            Deco { underline: i.underline, line_through: i.line_through },
                            i.shadows(layout),
                            surface,
                            width,
                            height,
                            damage_rects,
                            child_clip,
                        );
                    }
                    crate::layout::inline::Frag::Atomic { node, x, y } => {
                        if skip.contains(node) || is_layered(layout, *node) {
                            continue;
                        }
                        let i = inh.get(node).copied().unwrap_or(inherited);
                        let loc = layout.taffy.layout(*node).map(|l| l.location).unwrap_or(taffy::Point { x: 0.0, y: 0.0 });
                        draw_node(
                            *node, content_x + x - loc.x, content_y + y - loc.y, i, layout, font, font_bold, surface,
                            width, height, sx, sy, damage_rects, child_clip,
                        );
                    }
                }
            }
        }
        if ifc.is_none() {
            // Normal flow (Appendix E steps 3-5), tree order; positioned and
            // stacking-context boxes were hoisted into their context's layers.
            for child in layout.taffy.children(node_id).unwrap_or_default() {
                if is_layered(layout, child) {
                    continue;
                }
                draw_node(
                    child, current_x, current_y, inherited, layout, font, font_bold, surface,
                    width, height, sx, sy, damage_rects, child_clip,
                );
            }
        }
        // Steps 6-7: z-index auto/0 positioned boxes in tree order, then
        // positive z-index ascending.
        paint_layers(&layers[split..], surface);
        finish_round_clip(surface);
    }

    let root_inherited = Inherited {
        color: (0, 0, 0),
        font_size: 16.0,
        weight: 400,
        italic: false,
        stretch: 100,
        word_spacing: 0.0,
        rtl: false,
        text_shadow: None,
        line_height: 0.0, // natural
        underline: false,
        nowrap: false,
        white_space: 0,
        word_break: 0,
        overflow_wrap: 0,
        letter_spacing: 0.0,
        line_through: false,
        shift_y: 0.0,
        list_style: 1,
        family: crate::fonts::STANDARD, // the default standard font (Times New Roman)
        text_hidden: false,
        text_transform: 0,
        center_box: None,
    };
    draw_node(
        layout.root_node,
        0.0,
        0.0,
        root_inherited,
        layout,
        &font,
        &font_bold,
        surface,
        width,
        height,
        sx,
        sy,
        damage_rects,
        (0.0, 0.0, width as f32, height as f32),
    );
}
