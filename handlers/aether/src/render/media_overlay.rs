//! AETHERMEDIA2 (LEDGER SR134): what paints over a `<video>`'s picture, called from `paint_media` (the
//! media element's one paint hook): the UA's default caption box for the showing text tracks' active
//! cues, and the play affordance of an element whose autoplay / `play()` was refused for want of user
//! activation.
//!
//! The caption box is Chromium's default rendering of WebVTT cues (its `::cue` UA style): white
//! sans-serif text at 5 % of the video box's height (≥ 12 px) on rgba(0, 0, 0, 0.8), one box per
//! line, centred, stacked upward from just above the bottom edge (above the controls strip when
//! `controls` is on). The play affordance is Chromium's overlay play button: a 56 px dark disc with a
//! white triangle, centred in the box.

use super::{Advancer, Clip, Font, blend_px, centered_line_origin_y, draw_text, in_damage, measure_text_width, ui_sel};

#[allow(clippy::too_many_arguments)]
pub(super) fn paint(
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
    if cw <= 0.0 || ch <= 0.0 || media.kind != crate::media::Kind::Video {
        return;
    }
    let fill = |surface: &mut [u8], x0: f32, y0: f32, x1: f32, y1: f32, test: &dyn Fn(f32, f32) -> bool, color: (u8, u8, u8), alpha: u8| {
        let x0 = x0.max(cx).max(clip.0).max(0.0).round() as u32;
        let y0 = y0.max(cy).max(clip.1).max(0.0).round() as u32;
        let x1 = x1.min(cx + cw).min(clip.2).min(width as f32).round().max(0.0) as u32;
        let y1 = y1.min(cy + ch).min(clip.3).min(height as f32).round().max(0.0) as u32;
        for y in y0..y1 {
            for x in x0..x1 {
                if in_damage(x, y, damage_rects) && test(x as f32 + 0.5, y as f32 + 0.5) {
                    blend_px(surface, width, x, y, color, alpha);
                }
            }
        }
    };

    if media.blocked {
        let (mx, my) = (cx + cw / 2.0, cy + ch / 2.0);
        let r = 28.0f32.min(cw / 2.0).min(ch / 2.0);
        fill(surface, mx - r, my - r, mx + r, my + r, &|x, y| (x - mx).powi(2) + (y - my).powi(2) <= r * r, (0, 0, 0), 170);
        let (tx, th) = (mx - r * 0.3, r * 0.45);
        fill(surface, tx, my - th, tx + th * 1.6, my + th, &|x, y| (y - my).abs() <= th * (1.0 - (x - tx) / (th * 1.6)), (255, 255, 255), 255);
    }

    let Some(font) = font else { return };
    if media.captions.is_empty() {
        return;
    }
    let fs = (ch * 0.05).max(12.0);
    let adv = Advancer::new(ui_sel(), fs, 0.0);
    let line_h = fs * 1.2;
    let pad = fs * 0.25;
    let lines: Vec<&str> = media.captions.iter().flat_map(|c| c.split('\n')).filter(|l| !l.trim().is_empty()).collect();
    let bottom = cy + ch - if media.controls { 32.0 } else { 0.0 } - fs * 0.5;
    let mut y = bottom - lines.len() as f32 * line_h;
    for line in lines {
        let tw = measure_text_width(line, &adv).min(cw - 2.0 * pad);
        let x0 = cx + (cw - tw) / 2.0 - pad;
        fill(surface, x0, y, x0 + tw + 2.0 * pad, y + line_h, &|_, _| true, (0, 0, 0), 204);
        draw_text(
            line, x0 + pad, centered_line_origin_y(font, fs, y + line_h / 2.0), tw + 1.0, &adv, 1.2, (255, 255, 255), false, surface, width, height,
            damage_rects, clip,
        );
        y += line_h;
    }
}
