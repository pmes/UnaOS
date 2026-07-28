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
    family: u8, // 0 sans, 1 serif, 2 mono
    /// The image-replacement idiom: this subtree's text is off-box, but
    /// its boxes and backgrounds still paint.
    text_hidden: bool,
    text_transform: u8, // 0 = none, 1 = uppercase, 2 = lowercase, 3 = capitalize
}

use crate::layout::default_font_size;

/// Screen-space clip rect (x0, y0, x1, y1), already scroll-adjusted.
type Clip = (f32, f32, f32, f32);

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
type GlyphKey = (u8, u32, u32); // (family*2+bold, glyph, quarter-px size)
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
            let mut it = v.split_whitespace();
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
        let tokens: Vec<&str> = pos.split_whitespace().collect();
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

/// Transforms text based on text-transform property: 0 = none, 1 = uppercase, 2 = lowercase, 3 = capitalize.
fn transform_text(text: &str, transform: u8) -> String {
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

/// Draws `text` starting at (origin_x, origin_y), wrapping at `max_width`.
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
    let metrics = font.metrics();
    let scale = font_size / metrics.units_per_em as f32;
    let ascent = metrics.ascent * scale;
    let natural = (metrics.ascent - metrics.descent + metrics.line_gap) * scale;
    // 0.0 = "normal": use the font's natural line height.
    let line_height = if line_height_mult > 0.0 { font_size * line_height_mult } else { natural };

    let mut pen_x = 0.0f32;
    let mut line = 0u32;

    for word in text.split_whitespace() {
        let word_width: f32 = word
            .chars()
            .filter_map(|c| font.glyph_for_char(c))
            .filter_map(|g| font.advance(g).ok())
            .map(|a| a.x() * scale)
            .sum();
        let space = font_size * 0.3;

        if pen_x > 0.0 && pen_x + word_width > max_width {
            pen_x = 0.0;
            line += 1;
        }

        for c in word.chars() {
            let Some(glyph_id) = font.glyph_for_char(c) else { continue };
            let advance = font.advance(glyph_id).map(|a| a.x() * scale).unwrap_or(font_size * 0.5);
            let baseline_y = origin_y + line as f32 * line_height + ascent;
            if rasterize_glyph_cached(font, font_key, glyph_id, font_size).is_some() {
                blit_cached_glyph(
                    font_key, glyph_id, font_size,
                    origin_x + pen_x, baseline_y,
                    color, surface, width, height, damage_rects, clip,
                );
            }
            pen_x += advance;
        }
        if underline && word_width > 0.0 {
            let baseline_y = origin_y + line as f32 * line_height + ascent;
            let uy = (baseline_y + 2.0) as i32;
            if uy >= 0 {
                let x0 = (origin_x + pen_x - word_width).max(0.0) as u32;
                let x1 = ((origin_x + pen_x) as u32).min(width);
                let uy = uy as u32;
                if uy < height {
                    for x in x0..x1 {
                        if in_damage(x, uy, damage_rects) && in_clip(x, uy, clip) {
                            put_px(surface, width, x, uy, color);
                        }
                    }
                }
            }
        }
        pen_x += space;
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

    // Clear damaged regions to the page background.
    for &(dx, dy, dw, dh) in damage_rects {
        let ex = (dx + dw).min(width);
        let ey = (dy + dh).min(height);
        for y in dy..ey {
            for x in dx..ex {
                put_px(surface, width, x, y, (255, 255, 255));
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
        let current_x = abs_x + layout_box.location.x;
        let current_y = abs_y + layout_box.location.y;

        let mut inherited = inherited;

        if let Some(dom_node) = layout.node_map.get(&node_id) {
            let spec = layout.paint_map.get(&node_id).cloned().unwrap_or_default();

            if let Some(el) = dom_node.as_element() {
                let tag = el.name.local.as_ref();
                inherited.font_size =
                    spec.font_size.unwrap_or_else(|| default_font_size(tag, inherited.font_size));
                inherited.bold = spec.bold.unwrap_or(
                    inherited.bold
                        || matches!(tag, "b" | "strong" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "th"),
                );
                inherited.italic = spec.italic.unwrap_or(
                    inherited.italic || matches!(tag, "i" | "em"),
                );
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
                if spec.text_hidden == Some(true) {
                    inherited.text_hidden = true;
                }
                inherited.color = spec.color.unwrap_or(if tag == "a" {
                    (0, 0, 238) // UA default link blue
                } else {
                    inherited.color
                });

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

                if let Some(bg) = spec.background {
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
                        let x_start = ((current_x as i32) - sx).max(0) as u32;
                        let y_start = ((current_y as i32) - sy).max(0) as u32;
                        let bw = layout_box.size.width.max(1.0);
                        let bh = layout_box.size.height.max(1.0);
                        let end_x = x_start.saturating_add(bw as u32).min(width);
                        let end_y = y_start.saturating_add(bh as u32).min(height);
                        for y in y_start..end_y {
                            for x in x_start..end_x {
                                if !in_damage(x, y, damage_rects) {
                                    continue;
                                }
                                // Nearest-neighbor sample into the layout box.
                                let u = ((x - x_start) as f32 / bw * img.width() as f32) as u32;
                                let v = ((y - y_start) as f32 / bh * img.height() as f32) as u32;
                                let px = img.get_pixel(u.min(img.width() - 1), v.min(img.height() - 1));
                                let [r, g, b, a] = px.0;
                                if a > 0 {
                                    blend_px(surface, width, x, y, (r, g, b), a);
                                }
                            }
                        }
                    }
                }

                // Form controls get a UA border and their value text.
                let is_control = matches!(tag, "input" | "textarea" | "select" | "button");
                let mut border = spec.border.or(if is_control {
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
                if is_control && tag != "button" {
                    if let Some(font) = font {
                        let attrs = el.attributes.borrow();
                        let value = attrs
                            .get("value")
                            .filter(|v| !v.is_empty())
                            .or_else(|| attrs.get("placeholder"))
                            .unwrap_or("")
                            .to_string();
                        drop(attrs);
                        if !value.is_empty() {
                            draw_text(
                                &value,
                                current_x - sx as f32 + 4.0,
                                current_y - sy as f32 + 3.0,
                                layout_box.size.width.max(8.0) - 8.0,
                                font,
                                0,
                                inherited.font_size.min(14.0),
                                inherited.line_height,
                                (60, 60, 60),
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

                if let Some(sides) = border {
                    let x_start = ((current_x as i32) - sx).max(0) as u32;
                    let y_start = ((current_y as i32) - sy).max(0) as u32;
                    let end_x = x_start
                        .saturating_add(layout_box.size.width.max(0.0) as u32)
                        .min(width);
                    let end_y = y_start
                        .saturating_add(layout_box.size.height.max(0.0) as u32)
                        .min(height);
                    // [top, right, bottom, left], each side its own stroke.
                    let mut stroke = |x0: u32, y0: u32, x1: u32, y1: u32, color: (u8, u8, u8)| {
                        for y in y0..y1.min(height) {
                            for x in x0..x1.min(width) {
                                if in_damage(x, y, damage_rects) && in_clip(x, y, clip) {
                                    put_px(surface, width, x, y, color);
                                }
                            }
                        }
                    };
                    if let Some((w, c)) = sides[0] {
                        stroke(x_start, y_start, end_x, y_start.saturating_add(w.max(1.0) as u32), c);
                    }
                    if let Some((w, c)) = sides[2] {
                        stroke(x_start, end_y.saturating_sub(w.max(1.0) as u32), end_x, end_y, c);
                    }
                    if let Some((w, c)) = sides[3] {
                        stroke(x_start, y_start, x_start.saturating_add(w.max(1.0) as u32), end_y, c);
                    }
                    if let Some((w, c)) = sides[1] {
                        stroke(end_x.saturating_sub(w.max(1.0) as u32), y_start, end_x, end_y, c);
                    }
                }
            } else if dom_node.as_text().is_some() && !inherited.text_hidden {
                // Family font first (serif/mono), falling back to the
                // preloaded sans pair; bold uses the sans-bold face for
                // non-sans families (approximation).
                let fam_font = if inherited.family != 0 && !inherited.bold {
                    crate::layout::family_font(inherited.family)
                } else {
                    None
                };
                let font = if fam_font.is_some() {
                    &fam_font
                } else if inherited.bold {
                    font_bold
                } else {
                    font
                };
                if let Some(font) = font {
                    let text = dom_node.text_contents();
                    let text = text.trim();
                    if !text.is_empty() {
                        let text = transform_text(text, inherited.text_transform);
                        draw_text(
                            &text,
                            current_x - sx as f32,
                            current_y - sy as f32,
                            if inherited.nowrap { f32::MAX } else { layout_box.size.width.max(1.0) },
                            font,
                            inherited.family * 2 + if inherited.bold { 1 } else { 0 },
                            inherited.font_size,
                            inherited.line_height,
                            inherited.color,
                            inherited.underline,
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
        if child_clip.2 <= child_clip.0 || child_clip.3 <= child_clip.1 {
            return; // fully clipped out — nothing below can paint
        }
        if let Ok(children) = layout.taffy.children(node_id) {
            for child in children {
                draw_node(
                    child, current_x, current_y, inherited, layout, font, font_bold, surface,
                    width, height, sx, sy, damage_rects, child_clip,
                );
            }
        }
    }

    let root_inherited = Inherited {
        color: (0, 0, 0),
        font_size: 16.0,
        bold: false,
        italic: false,
        line_height: 0.0, // natural
        underline: false,
        nowrap: false,
        family: 0,
        text_hidden: false,
        text_transform: 0,
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
