use crate::layout::LayoutTree;
use font_kit::canvas::{Canvas, Format, RasterizationOptions};
use font_kit::family_name::FamilyName;
use font_kit::font::Font;
use font_kit::hinting::HintingOptions;
use font_kit::properties::Properties;
use pathfinder_geometry::transform2d::Transform2F;
use std::sync::Arc;
use taffy::prelude::*;

/// Inherited paint state carried down the box tree.
#[derive(Clone, Copy)]
struct Inherited {
    color: (u8, u8, u8),
    font_size: f32,
    bold: bool,
    italic: bool,
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
    family: u8, // 0 sans, 1 serif, 2 mono
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

/// Rasterized-glyph cache: font-kit rasterization dominated scroll
/// repaints (every damage strip re-rendered every glyph). Keyed by
/// (bold, glyph id, quarter-px font size); holds the coverage bitmap and
/// its raster-bounds origin. Cleared implicitly by process lifetime —
/// glyphs are font-global, not page-scoped.
type GlyphKey = (u8, u32, u32); // (fonts::face_key, glyph, quarter-px size)
struct CachedGlyph {
    origin: (i32, i32),
    w: i32,
    h: i32,
    cov: Vec<u8>,
}
thread_local! {
    static GLYPHS: std::cell::RefCell<std::collections::HashMap<GlyphKey, Option<CachedGlyph>>> =
        std::cell::RefCell::new(std::collections::HashMap::new());
}

fn rasterize_glyph_cached(
    font: &Font,
    font_key: u8,
    glyph_id: u32,
    font_size: f32,
) -> Option<(i32, i32, i32, i32)> {
    let key = (font_key, glyph_id, (font_size * 4.0) as u32);
    GLYPHS.with(|g| {
        if !g.borrow().contains_key(&key) {
            let computed = (|| {
                let bounds = font
                    .raster_bounds(
                        glyph_id,
                        font_size,
                        Transform2F::default(),
                        HintingOptions::None,
                        RasterizationOptions::GrayscaleAa,
                    )
                    .ok()?;
                if bounds.size().x() <= 0 || bounds.size().y() <= 0 {
                    return None;
                }
                let mut canvas = Canvas::new(bounds.size(), Format::A8);
                font.rasterize_glyph(
                    &mut canvas,
                    glyph_id,
                    font_size,
                    Transform2F::from_translation(-bounds.origin().to_f32()),
                    HintingOptions::None,
                    RasterizationOptions::GrayscaleAa,
                )
                .ok()?;
                Some(CachedGlyph {
                    origin: (bounds.origin().x(), bounds.origin().y()),
                    w: bounds.size().x(),
                    h: bounds.size().y(),
                    cov: canvas.pixels,
                })
            })();
            g.borrow_mut().insert(key, computed);
        }
        g.borrow()
            .get(&key)
            .and_then(|o| o.as_ref())
            .map(|c| (c.origin.0, c.origin.1, c.w, c.h))
    })
}

/// Blends one cached glyph's coverage at (px_x baseline-relative already applied by caller).
#[allow(clippy::too_many_arguments)]
fn blit_cached_glyph(
    font_key: u8,
    glyph_id: u32,
    font_size: f32,
    origin_x: f32,
    baseline_y: f32,
    color: (u8, u8, u8),
    surface: &mut [u8],
    width: u32,
    height: u32,
    damage_rects: &[(u32, u32, u32, u32)],
    clip: Clip,
) {
    let key = (font_key, glyph_id, (font_size * 4.0) as u32);
    GLYPHS.with(|g| {
        let g = g.borrow();
        let Some(Some(c)) = g.get(&key) else { return };
        for row in 0..c.h {
            for col in 0..c.w {
                let cov = c.cov[(row * c.w + col) as usize];
                if cov == 0 {
                    continue;
                }
                let dst_x = origin_x + (c.origin.0 + col) as f32;
                let dst_y = baseline_y + (c.origin.1 + row) as f32;
                if dst_x < 0.0 || dst_y < 0.0 {
                    continue;
                }
                let (px, py) = (dst_x as u32, dst_y as u32);
                if px < width && py < height && in_damage(px, py, damage_rects) && in_clip(px, py, clip) {
                    blend_px(surface, width, px, py, color, cov);
                }
            }
        }
    });
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
    font: &Font,
    font_key: u8,
    font_size: f32,
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
        text, origin_x, origin_y, max_width, font, font_key, font_size, line_height_mult, color,
        &crate::fonts::lines::TextMode::default(), deco, surface, width, height, damage_rects, clip,
    );
}

/// The inherited paint state of an element's subtree: font, colour,
/// decorations, white-space, list style, super/sub shift (CSS 2.2 §6.2,
/// with the html.css UA defaults by tag).
fn inherit_element(inherited: &mut Inherited, tag: &str, spec: &crate::layout::PaintStyle, dom_node: &kuchiki::NodeRef) {
    let parent_font_size = inherited.font_size;
    let own_family = spec
        .family
        .unwrap_or_else(|| crate::layout::default_family(tag, inherited.family));
    inherited.font_size = spec.font_size.unwrap_or_else(|| {
        crate::layout::ua_font_size(tag, inherited.font_size, inherited.family, own_family)
    });
    inherited.bold = spec.bold.unwrap_or_else(|| crate::layout::default_bold(tag, inherited.bold));
    inherited.italic =
        spec.italic.unwrap_or_else(|| crate::layout::default_italic(tag, inherited.italic));
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
/// inline formatting context's text fragment (layout::inline).
#[allow(clippy::too_many_arguments)]
fn draw_glyph_run(
    text: &str,
    origin_x: f32,
    baseline_y: f32,
    font: &Font,
    font_key: u8,
    font_size: f32,
    letter_spacing: f32,
    color: (u8, u8, u8),
    deco: Deco,
    surface: &mut [u8],
    width: u32,
    height: u32,
    damage_rects: &[(u32, u32, u32, u32)],
    clip: Clip,
) {
    let adv = crate::fonts::lines::Advancer::new(font, font_key, font_size, letter_spacing);
    let mut pen_x = 0.0f32;
    for c in text.chars() {
        let advance = adv.char(c);
        if c != ' ' {
            if let Some(glyph_id) = font.glyph_for_char(c) {
                if rasterize_glyph_cached(font, font_key, glyph_id, font_size).is_some() {
                    blit_cached_glyph(
                        font_key, glyph_id, font_size, origin_x + pen_x, baseline_y, color, surface, width, height,
                        damage_rects, clip,
                    );
                }
            }
        }
        pen_x += advance;
    }
    if !(deco.underline || deco.line_through) {
        return;
    }
    let (a_px, _, _) = crate::fonts::line_metrics(font, font_size);
    let thick = (font_size / 16.0).round().max(1.0) as i32;
    // The fragment's spaces are inside its inline box (the line's hanging
    // spaces were already removed by layout), so the decoration spans them.
    let (x0, x1) = (origin_x, origin_x + pen_x);
    let mut hline = |y: f32| {
        let yi = y.round() as i32;
        for dy in 0..thick {
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
    if deco.underline && x1 > x0 {
        hline(baseline_y + (font_size / 9.0).max(1.0));
    }
    if deco.line_through && x1 > x0 {
        hline(baseline_y - a_px * 0.3);
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
    font: &Font,
    font_key: u8,
    font_size: f32,
    line_height_mult: f32,
    color: (u8, u8, u8),
    mode: &crate::fonts::lines::TextMode,
    deco: Deco,
    surface: &mut [u8],
    width: u32,
    height: u32,
    damage_rects: &[(u32, u32, u32, u32)],
    clip: Clip,
) {
    let ascent = crate::fonts::baseline_offset(font, font_size, line_height_mult);
    let line_height = crate::fonts::line_height(font, font_size, line_height_mult);
    let adv = crate::fonts::lines::Advancer::new(font, font_key, font_size, mode.letter_spacing);
    let lines = crate::fonts::lines::break_lines(&adv, text, mode, max_width);
    let (a_px, _, _) = crate::fonts::line_metrics(font, font_size);
    let hline = |x0: f32, x1: f32, y: f32, thick: u32, surface: &mut [u8]| {
        let yi = y.round() as i32;
        for dy in 0..thick as i32 {
            let yy = yi + dy;
            if yy < 0 || yy as u32 >= height {
                continue;
            }
            let x0 = x0.max(0.0).round() as u32;
            let x1 = (x1.max(0.0).round() as u32).min(width);
            for x in x0..x1 {
                if in_damage(x, yy as u32, damage_rects) && in_clip(x, yy as u32, clip) {
                    put_px(surface, width, x, yy as u32, color);
                }
            }
        }
    };
    let thick = (font_size / 16.0).round().max(1.0) as u32;
    for (i, line) in lines.iter().enumerate() {
        let baseline_y = origin_y + i as f32 * line_height + ascent;
        let mut pen_x = 0.0f32;
        for c in line.text.chars() {
            let advance = adv.char(c);
            if c != ' ' {
                if let Some(glyph_id) = font.glyph_for_char(c) {
                    if rasterize_glyph_cached(font, font_key, glyph_id, font_size).is_some() {
                        blit_cached_glyph(
                            font_key, glyph_id, font_size,
                            origin_x + pen_x, baseline_y,
                            color, surface, width, height, damage_rects, clip,
                        );
                    }
                }
            }
            pen_x += advance;
        }
        // Decorations span the line's ink extent (leading/trailing
        // collapsible spaces excluded).
        let lead_w: f32 = line.text.chars().take_while(|c| *c == ' ').map(|c| adv.char(c)).sum();
        let trail_w: f32 = line.text.chars().rev().take_while(|c| *c == ' ').map(|c| adv.char(c)).sum();
        let (x0, x1) = (origin_x + lead_w, origin_x + (line.width - trail_w).max(lead_w));
        if deco.underline && x1 > x0 {
            hline(x0, x1, baseline_y + (font_size / 9.0).max(1.0), thick, surface);
        }
        if deco.line_through && x1 > x0 {
            hline(x0, x1, baseline_y - a_px * 0.3, thick, surface);
        }
    }
}

/// The ordinal of a list item: its `value`, else the list's `start` (1)
/// plus the number of preceding items (`reversed` counts down).
fn list_ordinal(li: &kuchiki::NodeRef) -> i64 {
    let attr_num = |n: &kuchiki::NodeRef, a: &str| {
        n.as_element().and_then(|e| e.attributes.borrow().get(a).and_then(|v| v.trim().parse::<i64>().ok()))
    };
    if let Some(v) = attr_num(li, "value") {
        return v;
    }
    let is_li = |n: &kuchiki::NodeRef| n.as_element().is_some_and(|e| e.name.local.as_ref() == "li");
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
    li: &kuchiki::NodeRef, style: u8, content_x: f32, baseline: f32, font: &Font, key: u8, fs: f32,
    color: (u8, u8, u8), surface: &mut [u8], width: u32, height: u32,
    damage_rects: &[(u32, u32, u32, u32)], clip: Clip,
) {
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
    let adv = crate::fonts::lines::Advancer::new(font, key, fs, 0.0);
    let w = adv.str(&text);
    let mode = crate::fonts::lines::TextMode { white_space: 2, ..Default::default() };
    let top = baseline - crate::fonts::baseline_offset(font, fs, 0.0);
    draw_lines(&text, content_x - w, top, f32::MAX, font, key, fs, 0.0, color, &mode, Deco::default(), surface, width, height, damage_rects, clip);
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
/// lays it out (per-glyph advances, one space-glyph advance between
/// words). Centering a control label with any other measurement drifts.
fn measure_text_width(text: &str, font: &Font, font_size: f32) -> f32 {
    let metrics = font.metrics();
    let scale = font_size / metrics.units_per_em as f32;
    let space = crate::fonts::space_advance(font, font_size);
    let mut total = 0.0f32;
    let mut words = 0u32;
    for word in text.split_whitespace() {
        total += word
            .chars()
            .filter_map(|c| font.glyph_for_char(c))
            .filter_map(|g| font.advance(g).ok())
            .map(|a| a.x() * scale)
            .sum::<f32>();
        words += 1;
    }
    total + space * words.saturating_sub(1) as f32
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
    let bg = |id: Option<NodeId>| id.and_then(|i| layout.paint_map.get(&i)).and_then(|p| p.background);
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
    font: &Option<Arc<Font>>,
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
                font, 0, fs, 1.2, (255, 255, 255), false, surface, width, height, damage_rects,
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

    if media.controls {
        // The strip: 32px (an <audio> box is all strip), translucent black.
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
            let tw = measure_text_width(&label, font, fs);
            if cw > tw + 80.0 {
                draw_text(
                    &label, track_x0, centered_line_origin_y(font, fs, mid), tw + 2.0, font, 0, fs, 1.2,
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

    let font_engine = crate::fonts::FontEngine::new();
    let font = font_engine.load_font(&[FamilyName::SansSerif], &Properties::new());
    let font_bold = font_engine
        .load_font(
            &[FamilyName::SansSerif],
            Properties::new().weight(font_kit::properties::Weight::BOLD),
        )
        .or_else(|| font.clone());

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
        font: &Option<Arc<Font>>,
        font_bold: &Option<Arc<Font>>,
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
                    inherit_element(&mut cinh, el.name.local.as_ref(), &spec, n);
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
                    inherit_element(&mut inh, el.name.local.as_ref(), &spec, dn);
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
        font: &Option<Arc<Font>>,
        font_bold: &Option<Arc<Font>>,
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
                inherit_element(&mut inherited, tag, &spec, dom_node);

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
                if fancy && mask.is_none() && spec.background.is_some() {
                    boxpaint::paint(
                        box_sx, box_sy, bw, bh, spec.radius.unwrap_or([0.0; 4]),
                        [None; 4], spec.background, width, height, &mut blend_at,
                    );
                } else if let Some(bg) = spec.background {
                    for y in y_start..end_y {
                        for x in x_start..end_x {
                            if !in_damage(x, y, damage_rects) || !in_clip(x, y, clip) {
                                continue;
                            }
                            match mask_alpha(x, y) {
                                0 => {}
                                255 => put_px(surface, width, x, y, bg),
                                a => blend_px(surface, width, x, y, bg, a),
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
                    if let Some(f) = crate::fonts::face(inherited.family, inherited.bold, inherited.italic) {
                        let mode = crate::fonts::lines::TextMode { white_space: 3, ..Default::default() };
                        draw_lines(
                            text.trim_start_matches('\n'), content_x, content_y, content_w.max(1.0), &f,
                            crate::fonts::face_key(inherited.family, inherited.bold, inherited.italic),
                            fs, 0.0, spec.color.unwrap_or((0, 0, 0)), &mode, Deco::default(),
                            surface, width, height, damage_rects, clip,
                        );
                    }
                }
                if is_control && tag != "button" && !checkable && tag != "textarea" {
                    let face = crate::fonts::face(inherited.family, inherited.bold, inherited.italic);
                    if let Some(font) = face.as_ref().or(font.as_ref()) {
                        let attrs = el.attributes.borrow();
                        // A <select> paints the SELECTED OPTION'S TEXT, not
                        // its submit value: layout publishes the visible label
                        // as data-aether-label, so "English" shows where the
                        // value attribute would only say "en".
                        let shown = (tag == "select")
                            .then(|| attrs.get("data-aether-label"))
                            .flatten()
                            .filter(|v| !v.is_empty())
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
                            let text_w = measure_text_width(&value, font, fs);
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
                                font,
                                crate::fonts::face_key(inherited.family, inherited.bold, inherited.italic),
                                fs,
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
                    let mut stroke = |x0: f32, y0: f32, x1: f32, y1: f32, color: (u8, u8, u8)| {
                        let x0 = x0.max(0.0) as u32;
                        let y0 = y0.max(0.0) as u32;
                        let x1 = (x1.max(0.0) as u32).min(width);
                        let y1 = (y1.max(0.0) as u32).min(height);
                        for y in y0..y1 {
                            for x in x0..x1 {
                                if in_damage(x, y, damage_rects) && in_clip(x, y, clip) {
                                    put_px(surface, width, x, y, color);
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
                    if let Some(f) = crate::fonts::face(inherited.family, inherited.bold, inherited.italic) {
                        let fs = inherited.font_size;
                        let base = match layout.inline.get(&node_id).and_then(|l| l.first_baseline) {
                            Some(b) => content_y + b,
                            None => content_y + crate::fonts::baseline_offset(&f, fs, inherited.line_height),
                        };
                        paint_marker(
                            dom_node, inherited.list_style, content_x, base, &f,
                            crate::fonts::face_key(inherited.family, inherited.bold, inherited.italic),
                            fs, inherited.color, surface, width, height, damage_rects, clip,
                        );
                    }
                }
            } else if dom_node.as_text().is_some() && !inherited.text_hidden {
                // The same face the measurer wrapped this run with
                // (fonts::face): family x weight x style. The preloaded sans
                // pair is only the fallback when no face loads at all.
                let face = crate::fonts::face(inherited.family, inherited.bold, inherited.italic);
                let font = if face.is_some() {
                    &face
                } else if inherited.bold {
                    font_bold
                } else {
                    font
                };
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
                            lead: spec_t.and_then(|p| p.ws_lead).unwrap_or(false),
                            trail: spec_t.and_then(|p| p.ws_trail).unwrap_or(false),
                        };
                        if inherited.nowrap && mode.white_space == 0 {
                            mode.white_space = 1;
                        }
                        // A <button>'s label is an ordinary text child, so it
                        // is centred here against the ancestor control's
                        // content box — but only when the measured run fits on
                        // one line inside it, so a button wrapping rich or
                        // overflowing content keeps normal flow painting.
                        let fkey = crate::fonts::face_key(inherited.family, inherited.bold, inherited.italic);
                        let centered = inherited.center_box.and_then(|(cx, cy, cw, ch)| {
                            let adv = crate::fonts::lines::Advancer::new(font, fkey, inherited.font_size, mode.letter_spacing);
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
                            font,
                            fkey,
                            inherited.font_size,
                            inherited.line_height,
                            inherited.color,
                            &mode,
                            Deco { underline: inherited.underline, line_through: inherited.line_through },
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
            return; // fully clipped out — nothing below can paint
        }
        if let Some(ifc) = ifc {
            // An inline formatting context: its line-box fragments, in tree
            // order (CSS 2.2 Appendix E step 7: each inline box's background
            // and borders, then its text; atomic inlines as whole boxes).
            let content_x = current_x + layout_box.border.left + layout_box.padding.left;
            let content_y = current_y + layout_box.border.top + layout_box.padding.top;
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
                                inherit_element(&mut i, tag, &spec, n);
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
                        let mut blend_at = |x: u32, y: u32, c: (u8, u8, u8), a: f32| {
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
                        let Some(face) = crate::fonts::face(i.family, i.bold, i.italic) else { continue };
                        let key = crate::fonts::face_key(i.family, i.bold, i.italic);
                        draw_glyph_run(
                            text,
                            content_x + x - sx as f32,
                            (exact_content_y + baseline).round() - sy as f32,
                            &face,
                            key,
                            i.font_size,
                            i.letter_spacing,
                            i.color,
                            Deco { underline: i.underline, line_through: i.line_through },
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
    }

    let root_inherited = Inherited {
        color: (0, 0, 0),
        font_size: 16.0,
        bold: false,
        italic: false,
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
        family: 0,
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
