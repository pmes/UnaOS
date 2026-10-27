// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PDF4-THUMBS (LEDGER SR360): the left strip of a PDF tab, and since PDF4-OUTLINE / PDF4-THUMBSCROLL (SR374) its two
//! modes ([`Side`]). F4 (Acrobat's navigation-pane key, through `pdf_view_key`) cycles off -> thumbnails -> outline ->
//! off (the outline skipped when the file has none); the strip is [`STRIP_W`] device px wide at the frame's left and the
//! page lane starts after it (`PdfDocument::left`, its CSS width at the zoom). The file keeps its SR360 name: both modes
//! ARE this one strip — its geometry, `in_strip`, its open / close, its stored scroll and its click door are shared.
//!
//! Thumbnails: each page's is PDF2's `Page::render` at [`THUMB_DPI`] (96/8) through the SAME render cache as the pages
//! (`PdfDocument::pixmap`, keyed (page, dpi)), rendered lazily (only those inside the strip's window, which `present`
//! keeps cached while the strip is open), its page number in a [`LABEL_H`] band below it. Outline: pdf_core's
//! `Document::outlines_with` (the one destination resolver) as an indented tree, one [`ROW_H`] row per item shown,
//! [`INDENT`] px per level, a triangle toggle on a parent (its state `PdfDocument::outline_open`), the current page's
//! item on a band. Labels go through the engine's one glyph pipeline (`fonts::shape::Advancer` + `fonts::raster`).
//!
//! The strip is painted into the same frame by `present` (no second Surface). Its scroll is STORED
//! (`PdfDocument::strip_off`): a wheel over it moves it (`pdf_wheel`, the face's wheel door), and a page change
//! re-centres it only when the current page's thumbnail or row is outside the window ([`sync`]). A click on a thumbnail
//! scrolls to its page through the one `Event::Scroll` clamp; on an outline row it follows the item through forms.rs's
//! `pdf_follow` (the one follower a /Link click uses), on a toggle it opens or closes the item.
//!
//! CHARTER: Aether owns the browser and the PDF line (CODEX §2 Web); this is inside the live handler.

use pdf_core::annots::LinkAction;

use super::PdfDocument;

/// The strip's width in device px.
pub const STRIP_W: f64 = 120.0;
/// The thumbnails' dpi (100 % zoom / 8).
pub const THUMB_DPI: f64 = 12.0;
/// Device px between thumbnails and above the first.
pub const THUMB_GAP: f64 = 8.0;
/// PDF4-THUMBSCROLL (SR374): the page-number band under each thumbnail, in device px.
pub const LABEL_H: f64 = 14.0;
/// The labels' size in device px.
pub const LABEL_PX: f32 = 11.0;
/// The labels' and the toggles' ink.
pub const LABEL_INK: (u8, u8, u8) = (232, 234, 237);
/// The strip's backdrop (Chromium's PDF sidebar).
pub const STRIP_BG: (u8, u8, u8) = (50, 54, 57);
/// The frame around the current page's thumbnail.
pub const THUMB_ACTIVE: (u8, u8, u8) = (138, 180, 248);
/// The frame's width in device px.
const FRAME: i64 = 2;

/// PDF4-OUTLINE (SR374): what the open strip shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Side {
    #[default]
    Thumbs,
    Outline,
}

impl PdfDocument {
    /// Whether viewport CSS `x` falls in the open strip.
    pub fn in_strip(&self, x: f64) -> bool {
        self.thumbs && x < self.left
    }
}

/// Each page's thumbnail `(top, w, h)` in the strip's device px, unscrolled: its size in points at [`THUMB_DPI`],
/// its label band below it, stacked [`THUMB_GAP`] apart.
pub fn layout(pdf: &PdfDocument) -> Vec<(f64, f64, f64)> {
    let s = THUMB_DPI / 72.0;
    let mut y = THUMB_GAP;
    pdf.sizes
        .iter()
        .map(|&(w, h)| {
            let t = (y, (w * s).ceil(), (h * s).ceil());
            y += t.2 + LABEL_H + THUMB_GAP;
            t
        })
        .collect()
}

/// PDF4-THUMBSCROLL (SR374): the strip's content height in device px in its current mode.
pub fn extent(pdf: &PdfDocument) -> f64 {
    match pdf.side {
        Side::Thumbs => layout(pdf).last().map_or(0.0, |l| l.0 + l.2 + LABEL_H + THUMB_GAP),
        Side::Outline => 2.0 * THUMB_GAP + rows(pdf).len() as f64 * ROW_H,
    }
}

/// PDF4-THUMBSCROLL (SR374): the strip's stored scroll for a window `dh` device px tall: when the current page `cur`
/// changed since the last call and its thumbnail or row (`top`, `h`) is outside the window, re-centred on it; then
/// clamped to the content `extent`. The wheel moves the same offset (`pdf_wheel`).
pub fn sync(pdf: &PdfDocument, extent: f64, cur: usize, (top, h): (f64, f64), dh: f64) -> f64 {
    let mut off = pdf.strip_off.get();
    if pdf.strip_cur.get() != Some(cur) {
        pdf.strip_cur.set(Some(cur));
        if top < off || top + h > off + dh {
            off = top + h / 2.0 - dh / 2.0;
        }
    }
    let off = off.clamp(0.0, (extent - dh).max(0.0));
    pdf.strip_off.set(off);
    off
}

/// The current page (under the viewport's vertical centre) for a frame `dh` device px tall at `zoom`.
fn current(pdf: &PdfDocument, dh: f64, zoom: f64, scroll_y: f64) -> usize {
    pdf.page_at(scroll_y + dh / zoom / 2.0)
}

/// The thumbnail mode's offset, synced on the current page's thumbnail.
fn thumbs_off(pdf: &PdfDocument, lay: &[(f64, f64, f64)], cur: usize, dh: f64) -> f64 {
    let ext = lay.last().map_or(0.0, |l| l.0 + l.2 + LABEL_H + THUMB_GAP);
    let rect = lay.get(cur).map_or((0.0, 0.0), |l| (l.0, l.2));
    sync(pdf, ext, cur, rect, dh)
}

/// Fill device rect [x0, x1) x [y0, y1) of a `dw` x `dh` frame with `c`, clipped to the strip's `sw` columns.
#[allow(clippy::too_many_arguments)]
fn fill(out: &mut [u8], (dw, dh): (usize, usize), sw: usize, x0: i64, y0: i64, x1: i64, y1: i64, c: (u8, u8, u8)) {
    for y in y0.max(0)..y1.min(dh as i64) {
        for x in x0.max(0)..x1.min(sw as i64) {
            let o = (y as usize * dw + x as usize) * 4;
            out[o..o + 4].copy_from_slice(&[c.0, c.1, c.2, 255]);
        }
    }
}

/// Paint the strip into `out` (`dw` x `dh` device px, straight RGBA): the backdrop, then the mode's content. In
/// thumbnail mode: the visible thumbnails centred and clipped to the strip, the current page's framed, each page's
/// number below it; returns the first and last page whose thumbnail it drew (the outline mode keeps none: None).
pub fn paint(pdf: &PdfDocument, out: &mut [u8], (dw, dh): (usize, usize), zoom: f64, scroll_y: f64) -> Option<(usize, usize)> {
    let sw = (STRIP_W as usize).min(dw);
    fill(out, (dw, dh), sw, 0, 0, sw as i64, dh as i64, STRIP_BG);
    let cur = current(pdf, dh as f64, zoom, scroll_y);
    if pdf.side == Side::Outline {
        outline_paint(pdf, out, (dw, dh), sw, cur);
        return None;
    }
    let lay = layout(pdf);
    let off = thumbs_off(pdf, &lay, cur, dh as f64);
    let vis: Vec<usize> = (0..lay.len()).filter(|&i| lay[i].0 + lay[i].2 + LABEL_H > off && lay[i].0 < off + dh as f64).collect();
    for &i in &vis {
        let (top, w, h) = lay[i];
        let pm = pdf.pixmap(i, THUMB_DPI);
        let (pw, ph) = if pm.w > 0 { (pm.w, pm.h) } else { (w as usize, h as usize) };
        let x0 = ((STRIP_W - pw as f64) / 2.0).round() as i64;
        let y0 = (top - off).round() as i64;
        if i == cur {
            fill(out, (dw, dh), sw, x0 - FRAME, y0 - FRAME, x0 + pw as i64 + FRAME, y0 + ph as i64 + FRAME, THUMB_ACTIVE);
        }
        for py in 0..ph {
            let dy = y0 + py as i64;
            if dy < 0 || dy >= dh as i64 {
                continue;
            }
            for px in 0..pw {
                let dx = x0 + px as i64;
                if dx < 0 || dx >= sw as i64 {
                    continue;
                }
                let o = (dy as usize * dw + dx as usize) * 4;
                if pm.w > 0 {
                    let s = (py * pm.w + px) * 4;
                    out[o..o + 4].copy_from_slice(&pm.data[s..s + 4]);
                } else {
                    out[o..o + 4].fill(255);
                }
            }
        }
        // PDF4-THUMBSCROLL (SR374): the page number, centred in the band below the thumbnail.
        let n = (i + 1).to_string();
        let lx = (STRIP_W - label_w(&n)) / 2.0;
        label(out, (dw, dh), sw, lx, top - off + h + LABEL_H - 3.0, &n, LABEL_INK);
    }
    Some((*vis.first()?, *vis.last()?))
}

/// The page whose thumbnail (its label band, or the gap above it) is at strip device row `y`.
pub fn at(pdf: &PdfDocument, y: f64, dh: f64, zoom: f64, scroll_y: f64) -> Option<usize> {
    let lay = layout(pdf);
    let off = thumbs_off(pdf, &lay, current(pdf, dh, zoom, scroll_y), dh);
    let y = y + off;
    lay.iter().position(|&(top, _, h)| y >= top - THUMB_GAP && y < top + h + LABEL_H)
}

impl crate::AetherEngine {
    /// F4 on a PDF: cycle the strip off -> thumbnails -> outline (when the file has one) -> off; the page lane moves,
    /// `scroll_x` re-clamps, and the strip's stored scroll starts over.
    pub(crate) fn pdf_strip_cycle(&mut self) {
        let z = self.zoom();
        let Some(pdf) = self.pdf.as_mut() else { return };
        let (open, side) = match (pdf.thumbs, pdf.side) {
            (false, _) => (true, Side::Thumbs),
            (true, Side::Thumbs) if !pdf.outline.is_empty() => (true, Side::Outline),
            _ => (false, Side::Thumbs),
        };
        pdf.thumbs = open;
        pdf.side = side;
        pdf.strip_off.set(0.0);
        pdf.strip_cur.set(None);
        pdf.left = if pdf.thumbs { STRIP_W / z } else { 0.0 };
        self.pdf_scroll_x(0.0);
        self.pdf_dirty();
    }

    /// A press in the open strip at viewport CSS (`x`, `y`): in thumbnail mode scroll to the thumbnail's page top (as a
    /// /Named page link does); in outline mode the row's toggle or its destination (`pdf_outline_click`).
    pub(crate) fn pdf_thumb_click(&mut self, x: f64, y: f64) {
        let (z, dh, sy, vw) = (self.zoom(), self.device.1 as f64, self.scroll_y, self.width as f64);
        let Some(pdf) = self.pdf.as_ref() else { return };
        if pdf.side == Side::Outline {
            return self.pdf_outline_click(x * z, y * z);
        }
        let Some(i) = at(pdf, y * z, dh, z, sy) else { return };
        let top = pdf.page_rect(i, vw).1;
        self.handle_event(crate::api::events::Event::Scroll(0.0, top - self.scroll_y));
        self.pdf_dirty();
    }
}

// =================================================================================================
// PDF4-OUTLINE (SR374): the outline mode — rows, the painter, the hit test and the click.
// =================================================================================================

/// An outline row's height in device px.
pub const ROW_H: f64 = 22.0;
/// The indent per outline level in device px.
pub const INDENT: f64 = 16.0;
/// The toggle column's width in device px (the title starts after it).
pub const TOGGLE: f64 = 12.0;
/// The current page's row band.
pub const ROW_ACTIVE: (u8, u8, u8) = (74, 80, 86);

/// The left edge of a level's toggle column in device px (the title at `row_x + TOGGLE`).
pub fn row_x(level: usize) -> f64 {
    6.0 + INDENT * level as f64
}

/// The outline items shown, as indices into `PdfDocument::outline`, in pre-order: the descendants of a closed item
/// are skipped.
pub fn rows(pdf: &PdfDocument) -> Vec<usize> {
    let mut v = Vec::new();
    let mut closed: Option<usize> = None;
    for (i, it) in pdf.outline.iter().enumerate() {
        if let Some(l) = closed {
            if it.level > l {
                continue;
            }
            closed = None;
        }
        v.push(i);
        if it.kids > 0 && !pdf.outline_open.get(i).copied().unwrap_or(false) {
            closed = Some(it.level);
        }
    }
    v
}

/// The page an item's destination names (a /GoTo); None for /URI, /Named and refused actions.
fn item_page(it: &pdf_core::OutlineItem) -> Option<usize> {
    match &it.action {
        Some(LinkAction::GoTo(d)) => Some(d.page),
        _ => None,
    }
}

/// The row (an index into `rows`) of the current page's item: the last shown row whose destination page is at or
/// before page `cur`.
pub fn current_row(pdf: &PdfDocument, rows: &[usize], cur: usize) -> Option<usize> {
    rows.iter().rposition(|&i| item_page(&pdf.outline[i]).is_some_and(|p| p <= cur))
}

/// The outline mode's offset, synced on the current page's row.
fn outline_off(pdf: &PdfDocument, rows: &[usize], cur: usize, dh: f64) -> f64 {
    let ext = 2.0 * THUMB_GAP + rows.len() as f64 * ROW_H;
    let rect = current_row(pdf, rows, cur).map_or((pdf.strip_off.get(), 0.0), |r| (THUMB_GAP + r as f64 * ROW_H, ROW_H));
    sync(pdf, ext, cur, rect, dh)
}

/// A parent's toggle at (`x`, `cy`): right-pointing when closed, down-pointing when open, 8 device px.
fn toggle(out: &mut [u8], size: (usize, usize), sw: usize, x: i64, cy: i64, open: bool) {
    if open {
        for k in 0..4 {
            fill(out, size, sw, x + k, cy - 2 + k, x + 8 - k, cy - 1 + k, LABEL_INK);
        }
    } else {
        for k in 0..8 {
            let w = 2 * (k.min(7 - k) + 1);
            fill(out, size, sw, x, cy - 4 + k, x + w, cy - 3 + k, LABEL_INK);
        }
    }
}

/// Paint the outline rows (the backdrop is down): the current page's row band, each parent's toggle at its level's
/// indent, the title after it, clipped to the strip.
fn outline_paint(pdf: &PdfDocument, out: &mut [u8], (dw, dh): (usize, usize), sw: usize, cur: usize) {
    let rows = rows(pdf);
    let off = outline_off(pdf, &rows, cur, dh as f64);
    let act = current_row(pdf, &rows, cur);
    for (r, &i) in rows.iter().enumerate() {
        let top = THUMB_GAP + r as f64 * ROW_H - off;
        if top + ROW_H <= 0.0 || top >= dh as f64 {
            continue;
        }
        let it = &pdf.outline[i];
        let y0 = top.round() as i64;
        if Some(r) == act {
            fill(out, (dw, dh), sw, 0, y0, sw as i64, y0 + ROW_H as i64, ROW_ACTIVE);
        }
        let x = row_x(it.level);
        let cy = y0 + (ROW_H / 2.0) as i64;
        if it.kids > 0 {
            toggle(out, (dw, dh), sw, x.round() as i64, cy, pdf.outline_open.get(i).copied().unwrap_or(false));
        }
        label(out, (dw, dh), sw, x + TOGGLE, cy as f64 + (LABEL_PX * 0.35) as f64, &it.title, LABEL_INK);
    }
}

/// The outline item at strip device (`x`, `y`), and whether the point is on its toggle (a parent's toggle column).
pub fn outline_at(pdf: &PdfDocument, (x, y): (f64, f64), dh: f64, zoom: f64, scroll_y: f64) -> Option<(usize, bool)> {
    let rows = rows(pdf);
    let off = outline_off(pdf, &rows, current(pdf, dh, zoom, scroll_y), dh);
    let r = (y + off - THUMB_GAP) / ROW_H;
    if r < 0.0 {
        return None;
    }
    let i = *rows.get(r as usize)?;
    let it = &pdf.outline[i];
    let tx = row_x(it.level);
    Some((i, it.kids > 0 && x >= tx - 2.0 && x < tx + TOGGLE))
}

/// The labels' shaper: the engine's sans-serif at [`LABEL_PX`].
fn advancer() -> crate::fonts::shape::Advancer {
    crate::fonts::shape::Advancer::new(crate::fonts::FontSel::new(crate::fonts::SANS, 400, false), LABEL_PX, 0.0)
}

/// A label's advance in device px.
fn label_w(text: &str) -> f64 {
    advancer().str(text) as f64
}

/// Draw `text` with its pen at device (`x`, `baseline`) in `ink`, clipped to the strip's `sw` columns: the engine's
/// one glyph pipeline (`Advancer::place` + `raster::preblend` / `raster::draw_glyph`, as render's `draw_shaped`
/// composes them), plotted over the straight-RGBA frame.
fn label(out: &mut [u8], (dw, dh): (usize, usize), sw: usize, x: f64, baseline: f64, text: &str, ink: (u8, u8, u8)) {
    use crate::fonts::raster;
    let adv = advancer();
    let lut = raster::preblend(ink);
    for (g, gx, dy) in adv.place(text) {
        raster::draw_glyph(g.face, g.gid, adv.size, x as f32 + gx, baseline as f32 + dy, &lut, &mut |px, py, a| {
            if px < 0 || py < 0 || px as usize >= sw || py as usize >= dh {
                return;
            }
            let o = (py as usize * dw + px as usize) * 4;
            let (a, inv) = (a as u32, 255 - a as u32);
            out[o] = ((ink.0 as u32 * a + out[o] as u32 * inv) / 255) as u8;
            out[o + 1] = ((ink.1 as u32 * a + out[o + 1] as u32 * inv) / 255) as u8;
            out[o + 2] = ((ink.2 as u32 * a + out[o + 2] as u32 * inv) / 255) as u8;
            out[o + 3] = 255;
        });
    }
}

impl crate::AetherEngine {
    /// A press on the outline at strip device (`x`, `y`): a parent's toggle opens or closes it; anywhere else on a row
    /// follows its action through `pdf_follow` (/GoTo to its page and position, /Named, /URI as a /Link click does).
    fn pdf_outline_click(&mut self, x: f64, y: f64) {
        let (z, dh, sy) = (self.zoom(), self.device.1 as f64, self.scroll_y);
        let Some(pdf) = self.pdf.as_mut() else { return };
        let Some((i, on_toggle)) = outline_at(pdf, (x, y), dh, z, sy) else { return };
        if on_toggle {
            if let Some(o) = pdf.outline_open.get_mut(i) {
                *o = !*o;
            }
            return self.pdf_dirty();
        }
        let Some(a) = pdf.outline[i].action.clone() else { return };
        self.pdf_follow(a);
    }
}
