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
}

use crate::layout::default_font_size;

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

/// Draws `text` starting at (origin_x, origin_y), wrapping at `max_width`.
#[allow(clippy::too_many_arguments)]
fn draw_text(
    text: &str,
    origin_x: f32,
    origin_y: f32,
    max_width: f32,
    font: &Font,
    font_size: f32,
    color: (u8, u8, u8),
    surface: &mut [u8],
    width: u32,
    height: u32,
    damage_rects: &[(u32, u32, u32, u32)],
) {
    let metrics = font.metrics();
    let scale = font_size / metrics.units_per_em as f32;
    let ascent = metrics.ascent * scale;
    let line_height = (metrics.ascent - metrics.descent + metrics.line_gap) * scale;

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

            if let Ok(bounds) = font.raster_bounds(
                glyph_id,
                font_size,
                Transform2F::default(),
                HintingOptions::None,
                RasterizationOptions::GrayscaleAa,
            ) {
                if bounds.size().x() > 0 && bounds.size().y() > 0 {
                    let mut canvas = Canvas::new(bounds.size(), Format::A8);
                    if font
                        .rasterize_glyph(
                            &mut canvas,
                            glyph_id,
                            font_size,
                            Transform2F::from_translation(-bounds.origin().to_f32()),
                            HintingOptions::None,
                            RasterizationOptions::GrayscaleAa,
                        )
                        .is_ok()
                    {
                        for row in 0..bounds.size().y() {
                            for col in 0..bounds.size().x() {
                                let cov = canvas.pixels[(row * canvas.stride as i32 + col) as usize];
                                if cov == 0 {
                                    continue;
                                }
                                let dst_x = origin_x + pen_x + (bounds.origin().x() + col) as f32;
                                let dst_y = baseline_y + (bounds.origin().y() + row) as f32;
                                if dst_x < 0.0 || dst_y < 0.0 {
                                    continue;
                                }
                                let (px, py) = (dst_x as u32, dst_y as u32);
                                if px < width && py < height && in_damage(px, py, damage_rects) {
                                    blend_px(surface, width, px, py, color, cov);
                                }
                            }
                        }
                    }
                }
            }
            pen_x += advance;
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
    ) {
        let Ok(layout_box) = layout.taffy.layout(node_id) else { return };
        let current_x = abs_x + layout_box.location.x;
        let current_y = abs_y + layout_box.location.y;

        let mut inherited = inherited;

        if let Some(dom_node) = layout.node_map.get(&node_id) {
            let spec = layout.paint_map.get(&node_id).copied().unwrap_or_default();

            if let Some(el) = dom_node.as_element() {
                let tag = el.name.local.as_ref();
                inherited.font_size =
                    spec.font_size.unwrap_or_else(|| default_font_size(tag, inherited.font_size));
                inherited.bold = spec.bold.unwrap_or(
                    inherited.bold
                        || matches!(tag, "b" | "strong" | "h1" | "h2" | "h3" | "h4" | "h5" | "h6" | "th"),
                );
                inherited.color = spec.color.unwrap_or(if tag == "a" {
                    (0, 0, 238) // UA default link blue
                } else {
                    inherited.color
                });

                if let Some(bg) = spec.background {
                    let x_start = ((current_x as i32) - sx).max(0) as u32;
                    let y_start = ((current_y as i32) - sy).max(0) as u32;
                    let end_x = x_start
                        .saturating_add(layout_box.size.width.max(0.0) as u32)
                        .min(width);
                    let end_y = y_start
                        .saturating_add(layout_box.size.height.max(0.0) as u32)
                        .min(height);
                    for y in y_start..end_y {
                        for x in x_start..end_x {
                            if in_damage(x, y, damage_rects) {
                                put_px(surface, width, x, y, bg);
                            }
                        }
                    }
                }
            } else if dom_node.as_text().is_some() {
                let font = if inherited.bold { font_bold } else { font };
                if let Some(font) = font {
                    let text = dom_node.text_contents();
                    let text = text.trim();
                    if !text.is_empty() {
                        draw_text(
                            text,
                            current_x - sx as f32,
                            current_y - sy as f32,
                            layout_box.size.width.max(1.0),
                            font,
                            inherited.font_size,
                            inherited.color,
                            surface,
                            width,
                            height,
                            damage_rects,
                        );
                    }
                }
            }
        }

        if let Ok(children) = layout.taffy.children(node_id) {
            for child in children {
                draw_node(
                    child, current_x, current_y, inherited, layout, font, font_bold, surface,
                    width, height, sx, sy, damage_rects,
                );
            }
        }
    }

    let root_inherited = Inherited {
        color: (0, 0, 0),
        font_size: 16.0,
        bold: false,
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
    );
}
