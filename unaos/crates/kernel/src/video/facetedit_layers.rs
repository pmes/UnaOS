// CHARTER: Facet — shared-core face (Images; FACETEDIT, rmbp-ledger B900)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! FACETEDIT — the paint of the editor window and the ONE hit table it records: the canvas region (the document's
//! composite through the [`View`](super::facetedit_tools::View), a checkerboard under transparency, the selection
//! tinted toward the accent, `MEDIA_BG` outside the document), the layers strip (rows top-down from the TOP layer:
//! eye, name, opacity, blend; the active row raised; the layer buttons under the rows), the tool rail (a segmented
//! control and the zoom / undo buttons), the adjust / file row and the fields row. Every control is the glass kit's
//! (`video/draw.rs`: Settings' row widgets); every colour a `theme` token (BP-video-1). The press reads the same
//! table the paint wrote, so the painted control and the pressed control are one geometry.

use alloc::format;
use alloc::vec::Vec;

use super::draw::{self, ControlState, Rect, Surf};
use super::facetedit_tools::{Editor, ADJUSTS, FIELDS, Tool};
use super::metrics;
use super::text::{self, Face};
use super::theme;

/// The window, in logical px (clamped to the panel by the caller).
pub const WIN_W: usize = 960;
pub const WIN_H: usize = 640;
/// The rail row, the two bottom rows, the strip's width and one strip row, logical px.
const RAIL_H: usize = 40;
const ROW_BAR_H: usize = 40;
const STRIP_W: usize = 200;
const STRIP_ROW: usize = 28;
/// The checkerboard's square, device px.
const CHECK: i32 = 8;

/// What a press on a recorded rect means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// The tool rail's segmented control (the segment is read from the x).
    Rail,
    /// A layer / view verb (`Editor::verb`'s words).
    Verb(&'static str),
    /// Save / Open (the window runs them: VFS).
    File(&'static str),
    /// An adjust button (`ADJUSTS` index).
    Adjust(usize),
    /// A strip row / its eye box (layer index).
    Row(usize),
    Eye(usize),
    /// A field slider (`FIELDS` index; the value is read from the x).
    Field(usize),
}

/// The window's regions, device px.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    pub w: usize,
    pub h: usize,
    /// The canvas region: `(x, y, w, h)`.
    pub canvas: Rect,
    pub strip: Rect,
    rail: Rect,
    adjust: Rect,
    fields: Rect,
}

impl Layout {
    /// The regions of a `w x h` device-px surface.
    pub fn of(w: usize, h: usize) -> Layout {
        let (rail_h, bar_h, strip_w) = (metrics::size(RAIL_H) as i32, metrics::size(ROW_BAR_H) as i32, metrics::size(STRIP_W) as i32);
        let (w32, h32) = (w as i32, h as i32);
        let body_h = (h32 - rail_h - 2 * bar_h).max(1);
        let cw = (w32 - strip_w).max(1);
        Layout {
            w,
            h,
            rail: Rect::new(0, 0, w32, rail_h),
            canvas: Rect::new(0, rail_h, cw, body_h),
            strip: Rect::new(cw, rail_h, w32 - cw, body_h),
            adjust: Rect::new(0, rail_h + body_h, w32, bar_h),
            fields: Rect::new(0, rail_h + body_h + bar_h, w32, bar_h),
        }
    }
}

fn inside(r: Rect, x: i32, y: i32) -> bool {
    x >= r.x && y >= r.y && x < r.right() && y < r.bottom()
}

/// The recorded control under the surface point `(x, y)`, with the point.
pub fn hit_at(hits: &[(Rect, Hit)], x: i32, y: i32) -> Option<(Rect, Hit)> {
    hits.iter().rev().find(|(r, _)| inside(*r, x, y)).copied()
}

/// The rail segment under `x` (the segmented control's `x / (w / n)` rule).
pub fn rail_tool(r: Rect, x: i32) -> Tool {
    let n = Tool::ALL.len() as i32;
    let i = ((x - r.x).max(0) * n / r.w.max(1)).clamp(0, n - 1);
    Tool::ALL[i as usize]
}

/// A slider's value under `x` (its knob is `r.h` wide; `max` the field's).
pub fn slider_value(r: Rect, x: i32, max: u32) -> u32 {
    let span = (r.w - r.h).max(1) as i64;
    ((x - r.x - r.h / 2) as i64 * max as i64 / span).clamp(0, max as i64) as u32
}

/// A label in the UI face at `(x, y)` (its line top), clipped to the surface.
fn label(s: &mut Surf, x: i32, y: i32, t: &[u8], ink: u32) -> i32 {
    if x < 0 || y < 0 {
        return x;
    }
    let (w, h) = (s.w, s.h);
    text::draw_text(s.px, w, w, h, x as usize, y as usize, t, ink, false, Face::Ui) as i32
}

/// The label top that centres one UI line in `r`.
fn mid(r: Rect) -> i32 {
    r.y + (r.h - Face::Ui.cell_h() as i32) / 2
}

/// A row of push buttons from `x`, each `bw` wide, centred in `row`; each recorded. Returns the pen x.
fn buttons(s: &mut Surf, hits: &mut Vec<(Rect, Hit)>, row: Rect, mut x: i32, bw: i32, items: &[(&[u8], Hit)]) -> i32 {
    let gap = metrics::size(6) as i32;
    let bh = metrics::size(draw::button_h()) as i32;
    let y = row.y + (row.h - bh) / 2;
    for (t, h) in items {
        let r = draw::push_button(s, x, y, bw, t, false, ControlState::NORMAL);
        hits.push((r, *h));
        x = r.right() + gap;
    }
    x
}

/// Paint the whole window into `px` (`lay.w x lay.h`) and return the hit table.
pub fn paint(ed: &mut Editor, px: &mut [u32], lay: &Layout) -> Vec<(Rect, Hit)> {
    let mut hits: Vec<(Rect, Hit)> = Vec::new();
    let mut s = Surf::new(px, lay.w);
    draw::fill(&mut s, Rect::new(0, 0, lay.w as i32, lay.h as i32), theme::opaque(theme::surface_window()));
    paint_canvas(ed, &mut s, lay.canvas);
    paint_strip(ed, &mut s, &mut hits, lay.strip);
    // The rail: the tools as one segmented control, then the view verbs.
    let pad = metrics::size(8) as i32;
    let seg_w = (lay.rail.w * 3 / 5).max(metrics::size(320) as i32);
    let sh = metrics::size(draw::seg_h()) as i32;
    let r = draw::segmented_w(&mut s, pad, lay.rail.y + (lay.rail.h - sh) / 2, seg_w, &Tool::LABELS, ed.tool.index(), ControlState::NORMAL);
    hits.push((r, Hit::Rail));
    let bw = metrics::size(56) as i32;
    buttons(&mut s, &mut hits, lay.rail, r.right() + pad, bw, &[(&b"-"[..], Hit::Verb("zoom-")), (&b"+"[..], Hit::Verb("zoom+")), (&b"Undo"[..], Hit::Verb("undo")), (&b"Redo"[..], Hit::Verb("redo"))]);
    // The adjust / file row.
    let mut items: Vec<(&[u8], Hit)> = ADJUSTS.iter().enumerate().map(|(i, (_, l))| (*l, Hit::Adjust(i))).collect();
    items.push((&b"Save"[..], Hit::File("save")));
    items.push((&b"Open"[..], Hit::File("open")));
    let n = items.len() as i32;
    let bw = ((lay.adjust.w - 2 * pad) / n - metrics::size(6) as i32).max(metrics::size(40) as i32);
    buttons(&mut s, &mut hits, lay.adjust, pad, bw, &items);
    // The fields row: a label and a slider per field, then the colour swatch.
    let f = lay.fields;
    let cell = ((f.w - 2 * pad - metrics::size(40) as i32) / FIELDS.len() as i32).max(metrics::size(60) as i32);
    let mut x = pad;
    for (i, (key, lbl, max)) in FIELDS.iter().enumerate() {
        let v = match *key {
            "diameter" => ed.diameter,
            "hardness" => ed.hardness as u32,
            "flow" => ed.flow as u32,
            "red" => ed.colour[0] as u32,
            "green" => ed.colour[1] as u32,
            "blue" => ed.colour[2] as u32,
            _ => ed.colour[3] as u32,
        };
        let lx = label(&mut s, x, mid(f), lbl, theme::text_secondary());
        let sx = lx + metrics::size(4) as i32;
        let kd = metrics::size(draw::row_h()) as i32 / 2;
        let r = draw::slider(&mut s, sx, f.y + (f.h - kd) / 2, (x + cell - sx - pad).max(kd * 2), v, *max, ControlState::NORMAL);
        hits.push((r, Hit::Field(i)));
        x += cell;
    }
    let sw = metrics::size(28) as i32;
    let swatch = Rect::new(x, f.y + (f.h - sw) / 2, sw, sw);
    let c = ed.colour;
    for yy in 0..sw {
        for xx in 0..sw {
            let chk = checker(xx, yy);
            let p = draw::blend(chk, ((c[0] as u32) << 16) | ((c[1] as u32) << 8) | c[2] as u32, c[3] as u32);
            put(&mut s, swatch.x + xx, swatch.y + yy, p);
        }
    }
    draw::stroke_rrect(&mut s, swatch, 0, 1, theme::opaque(theme::line_strong()));
    hits
}

/// The checkerboard's colour at region point `(x, y)`.
fn checker(x: i32, y: i32) -> u32 {
    if ((x / CHECK) + (y / CHECK)) % 2 == 0 { theme::surface_raised() } else { theme::surface_sunken() }
}

fn put(s: &mut Surf, x: i32, y: i32, rgb: u32) {
    if x >= 0 && y >= 0 && (x as usize) < s.w && (y as usize) < s.h {
        s.px[y as usize * s.w + x as usize] = theme::opaque(rgb);
    }
}

/// The canvas region: each pixel through the view; inside the document the composite over the checkerboard (the
/// selected cells a quarter toward the accent), outside `MEDIA_BG`.
fn paint_canvas(ed: &mut Editor, s: &mut Surf, r: Rect) {
    let view = ed.view;
    let (dw, dh) = (ed.doc.width as i64, ed.doc.height as i64);
    let accent = theme::accent();
    let outside = theme::opaque(theme::MEDIA_BG);
    let (comp, sel) = ed.composite();
    for sy in 0..r.h {
        let row = (r.y + sy) as usize;
        if row >= s.h {
            break;
        }
        for sx in 0..r.w {
            let col = (r.x + sx) as usize;
            if col >= s.w {
                break;
            }
            let (cx, cy) = view.cell(sx, sy);
            let p = if cx >= 0 && cy >= 0 && cx < dw && cy < dh {
                let i = ((cy * dw + cx) * 4) as usize;
                let src = ((comp[i] as u32) << 16) | ((comp[i + 1] as u32) << 8) | comp[i + 2] as u32;
                let mut p = draw::blend(checker(sx, sy), src, comp[i + 3] as u32);
                if sel.as_ref().is_some_and(|m| m.at(cx, cy) > 0) {
                    p = draw::blend(p, accent, 64);
                }
                theme::opaque(p)
            } else {
                outside
            };
            s.px[row * s.w + col] = p;
        }
    }
}

/// The layers strip: rows top-down from the top layer, then the layer buttons.
fn paint_strip(ed: &mut Editor, s: &mut Surf, hits: &mut Vec<(Rect, Hit)>, r: Rect) {
    draw::fill(s, r, theme::opaque(theme::surface_sidebar()));
    draw::hairline_v(s, r.x, r.y, r.h, theme::opaque(theme::line_subtle()));
    let rh = metrics::size(STRIP_ROW) as i32;
    let pad = metrics::size(6) as i32;
    let n = ed.doc.layers.len();
    for (i, l) in ed.doc.layers.iter().enumerate() {
        let y = r.y + (n - 1 - i) as i32 * rh;
        let row = Rect::new(r.x, y, r.w, rh);
        if row.bottom() > r.bottom() {
            continue;
        }
        if i == ed.active {
            draw::fill(s, row, theme::opaque(theme::surface_raised()));
        }
        let cb = metrics::size(16) as i32;
        let eye = draw::checkbox(s, r.x + pad, y + (rh - cb) / 2, l.visible, ControlState::NORMAL);
        let name = format!("{}  {} {}", l.name, l.opacity, super::facetedit_tools::blend_word(l.blend));
        let ink = if i == ed.active { theme::text_primary() } else { theme::text_secondary() };
        label(s, eye.right() + pad, mid(row), name.as_bytes(), ink);
        hits.push((row, Hit::Row(i)));
        hits.push((eye, Hit::Eye(i)));
    }
    // The layer verbs, two rows of three under the rows.
    let bh = metrics::size(draw::button_h()) as i32 + pad;
    let bw = (r.w - 4 * pad) / 3;
    let top = r.y + n as i32 * rh + pad;
    let rows: [[(&[u8], Hit); 3]; 2] = [
        [(&b"+"[..], Hit::Verb("layer-add")), (&b"-"[..], Hit::Verb("layer-remove")), (&b"Eye"[..], Hit::Verb("layer-eye"))],
        [(&b"Up"[..], Hit::Verb("layer-up")), (&b"Down"[..], Hit::Verb("layer-down")), (&b"Blend"[..], Hit::Verb("layer-blend"))],
    ];
    for (k, items) in rows.iter().enumerate() {
        let row = Rect::new(r.x, top + k as i32 * bh, r.w, bh);
        if row.bottom() <= r.bottom() {
            buttons(s, hits, row, r.x + pad, bw, items);
        }
    }
}
