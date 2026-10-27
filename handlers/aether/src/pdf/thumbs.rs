// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PDF4-THUMBS (LEDGER SR360): the thumbnail strip of a PDF tab. F4 (Acrobat's navigation-pane key, through
//! `pdf_view_key`) toggles a strip [`STRIP_W`] device px wide at the frame's left; the page lane starts after it
//! (`PdfDocument::left`, its CSS width at the zoom). Each page's thumbnail is PDF2's `Page::render` at [`THUMB_DPI`]
//! (96/8) through the SAME render cache as the pages (`PdfDocument::pixmap`, keyed (page, dpi)), rendered lazily: only
//! the thumbnails inside the strip's window, which `present` keeps cached while the strip is open. The strip is painted
//! into the same frame by `present` (no second Surface); it follows the current page (the page under the viewport's
//! centre is framed and kept centred: a wheel over the strip is owed); a click scrolls to the thumbnail's page through
//! the one `Event::Scroll` clamp.
//!
//! CHARTER: Aether owns the browser and the PDF line (CODEX §2 Web); this is inside the live handler.

use super::PdfDocument;

/// The strip's width in device px.
pub const STRIP_W: f64 = 120.0;
/// The thumbnails' dpi (100 % zoom / 8).
pub const THUMB_DPI: f64 = 12.0;
/// Device px between thumbnails and above the first.
pub const THUMB_GAP: f64 = 8.0;
/// The strip's backdrop (Chromium's PDF sidebar).
pub const STRIP_BG: (u8, u8, u8) = (50, 54, 57);
/// The frame around the current page's thumbnail.
pub const THUMB_ACTIVE: (u8, u8, u8) = (138, 180, 248);
/// The frame's width in device px.
const FRAME: i64 = 2;

impl PdfDocument {
    /// Whether viewport CSS `x` falls in the open strip.
    pub fn in_strip(&self, x: f64) -> bool {
        self.thumbs && x < self.left
    }
}

/// Each page's thumbnail `(top, w, h)` in the strip's device px, unscrolled: its size in points at [`THUMB_DPI`],
/// stacked [`THUMB_GAP`] apart.
pub fn layout(pdf: &PdfDocument) -> Vec<(f64, f64, f64)> {
    let s = THUMB_DPI / 72.0;
    let mut y = THUMB_GAP;
    pdf.sizes
        .iter()
        .map(|&(w, h)| {
            let t = (y, (w * s).ceil(), (h * s).ceil());
            y += t.2 + THUMB_GAP;
            t
        })
        .collect()
}

/// The strip's scroll in device px: page `cur`'s thumbnail centred in a strip `dh` tall, clamped to the stack.
pub fn strip_scroll(lay: &[(f64, f64, f64)], cur: usize, dh: f64) -> f64 {
    let Some(&(top, _, h)) = lay.get(cur) else { return 0.0 };
    let extent = lay.last().map_or(0.0, |l| l.0 + l.2 + THUMB_GAP);
    (top + h / 2.0 - dh / 2.0).clamp(0.0, (extent - dh).max(0.0))
}

/// The current page (under the viewport's vertical centre) for a frame `dh` device px tall at `zoom`.
fn current(pdf: &PdfDocument, dh: f64, zoom: f64, scroll_y: f64) -> usize {
    pdf.page_at(scroll_y + dh / zoom / 2.0)
}

/// Paint the strip into `out` (`dw` x `dh` device px, straight RGBA): the backdrop, the visible thumbnails centred and
/// clipped to the strip, the current page's framed. Returns the first and last page whose thumbnail it drew.
pub fn paint(pdf: &PdfDocument, out: &mut [u8], (dw, dh): (usize, usize), zoom: f64, scroll_y: f64) -> Option<(usize, usize)> {
    let sw = (STRIP_W as usize).min(dw);
    let fill = |out: &mut [u8], x0: i64, y0: i64, x1: i64, y1: i64, c: (u8, u8, u8)| {
        for y in y0.max(0)..y1.min(dh as i64) {
            for x in x0.max(0)..x1.min(sw as i64) {
                let o = (y as usize * dw + x as usize) * 4;
                out[o..o + 4].copy_from_slice(&[c.0, c.1, c.2, 255]);
            }
        }
    };
    fill(out, 0, 0, sw as i64, dh as i64, STRIP_BG);
    let lay = layout(pdf);
    let cur = current(pdf, dh as f64, zoom, scroll_y);
    let off = strip_scroll(&lay, cur, dh as f64);
    let vis: Vec<usize> = (0..lay.len()).filter(|&i| lay[i].0 + lay[i].2 > off && lay[i].0 < off + dh as f64).collect();
    for &i in &vis {
        let (top, w, h) = lay[i];
        let pm = pdf.pixmap(i, THUMB_DPI);
        let (pw, ph) = if pm.w > 0 { (pm.w, pm.h) } else { (w as usize, h as usize) };
        let x0 = ((STRIP_W - pw as f64) / 2.0).round() as i64;
        let y0 = (top - off).round() as i64;
        if i == cur {
            fill(out, x0 - FRAME, y0 - FRAME, x0 + pw as i64 + FRAME, y0 + ph as i64 + FRAME, THUMB_ACTIVE);
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
    }
    Some((*vis.first()?, *vis.last()?))
}

/// The page whose thumbnail (or the gap below it) is at strip device row `y`.
pub fn at(pdf: &PdfDocument, y: f64, dh: f64, zoom: f64, scroll_y: f64) -> Option<usize> {
    let lay = layout(pdf);
    let off = strip_scroll(&lay, current(pdf, dh, zoom, scroll_y), dh);
    let y = y + off;
    lay.iter().position(|&(top, _, h)| y >= top - THUMB_GAP && y < top + h)
}

impl crate::AetherEngine {
    /// F4 on a PDF: open or close the strip; the page lane moves and `scroll_x` re-clamps.
    pub(crate) fn pdf_thumbs_toggle(&mut self) {
        let z = self.zoom();
        let Some(pdf) = self.pdf.as_mut() else { return };
        pdf.thumbs = !pdf.thumbs;
        pdf.left = if pdf.thumbs { STRIP_W / z } else { 0.0 };
        self.pdf_scroll_x(0.0);
        self.pdf_dirty();
    }

    /// A press in the open strip at viewport CSS `y`: scroll to the thumbnail's page top (as a /Named page link does).
    pub(crate) fn pdf_thumb_click(&mut self, y: f64) {
        let (z, dh, sy, vw) = (self.zoom(), self.device.1 as f64, self.scroll_y, self.width as f64);
        let Some(pdf) = self.pdf.as_ref() else { return };
        let Some(i) = at(pdf, y * z, dh, z, sy) else { return };
        let top = pdf.page_rect(i, vw).1;
        self.handle_event(crate::api::events::Event::Scroll(0.0, top - self.scroll_y));
        self.pdf_dirty();
    }
}
