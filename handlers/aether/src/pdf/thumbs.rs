// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PDF4-THUMBS (LEDGER SR360): the left strip of a PDF tab, and since PDF4-OUTLINE / PDF4-THUMBSCROLL (SR374) its two
//! modes ([`Side`]). F4 (Acrobat's navigation-pane key, through `pdf_view_key`) cycles off -> thumbnails -> outline ->
//! off (the outline skipped when the file has none); the strip is [`STRIP_W`] logical px wide at the frame's left (device
//! px at the engine's scale through [`strip_px`], PDF4-STRIP-HIDPI SR387) and the page lane starts after it
//! (`PdfDocument::left` = [`strip_css`]). The file keeps its SR360 name: both modes
//! ARE this one strip — its geometry, `in_strip`, its open / close, its stored scroll and its click door are shared.
//!
//! Thumbnails: each page's is PDF2's `Page::render` at [`THUMB_DPI`] (96/8) through the SAME render cache as the pages
//! (`PdfDocument::pixmap`, keyed (page, dpi)), rendered lazily (only those inside the strip's window, which `present`
//! keeps cached while the strip is open), its page number in a [`LABEL_H`] band below it. Outline: pdf_core's
//! `Document::outlines_with` (the one destination resolver) as an indented tree, one [`ROW_H`] row per item shown,
//! [`INDENT`] px per level, a triangle toggle on a parent (its state `PdfDocument::outline_open`), the current page's
//! item on a band; a refused action's row is muted and its click named on the console line, /C and /F style the title,
//! a long title is ellipsised (SR387). Labels go through the engine's one glyph pipeline (`fonts::shape::Advancer` + `fonts::raster`).
//!
//! The strip is painted into the same frame by `present` (no second Surface). Its scroll is STORED
//! (`PdfDocument::strip_off`): a wheel over it moves it (`pdf_wheel`, the face's wheel door), and a page change
//! re-centres it only when the current page's thumbnail or row is outside the window ([`sync`]). A click on a thumbnail
//! scrolls to its page through the one `Event::Scroll` clamp; on an outline row it follows the item through forms.rs's
//! `pdf_follow` (the one follower a /Link click uses), on a toggle it opens or closes the item.
//!
//! CHARTER: Aether owns the browser and the PDF line (CODEX §2 Web); this is inside the live handler.

use std::borrow::Cow;

use pdf_core::annots::LinkAction;

use super::PdfDocument;

/// The strip's width in logical px (every strip length below is logical: [`strip_px`] scales it to device px).
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
/// its label band below it, stacked [`THUMB_GAP`] apart; every length at the engine's scale `z` ([`strip_px`]).
pub fn layout(pdf: &PdfDocument, z: f64) -> Vec<(f64, f64, f64)> {
    let s = THUMB_DPI * z / 72.0;
    let mut y = strip_px(THUMB_GAP, z);
    pdf.sizes
        .iter()
        .map(|&(w, h)| {
            let t = (y, (w * s).ceil(), (h * s).ceil());
            y += t.2 + strip_px(LABEL_H, z) + strip_px(THUMB_GAP, z);
            t
        })
        .collect()
}

/// PDF4-THUMBSCROLL (SR374): the strip's content height in device px in its current mode, at the engine's scale `z`.
pub fn extent(pdf: &PdfDocument, z: f64) -> f64 {
    match pdf.side {
        Side::Thumbs => layout(pdf, z).last().map_or(0.0, |l| l.0 + l.2 + strip_px(LABEL_H, z) + strip_px(THUMB_GAP, z)),
        Side::Outline => 2.0 * strip_px(THUMB_GAP, z) + rows(pdf).len() as f64 * strip_px(ROW_H, z),
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
fn thumbs_off(pdf: &PdfDocument, lay: &[(f64, f64, f64)], cur: usize, dh: f64, z: f64) -> f64 {
    let ext = lay.last().map_or(0.0, |l| l.0 + l.2 + strip_px(LABEL_H, z) + strip_px(THUMB_GAP, z));
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
    let (strip_w, lh) = (strip_px(STRIP_W, zoom), strip_px(LABEL_H, zoom));
    let sw = (strip_w.round() as usize).min(dw);
    fill(out, (dw, dh), sw, 0, 0, sw as i64, dh as i64, STRIP_BG);
    let cur = current(pdf, dh as f64, zoom, scroll_y);
    if pdf.side == Side::Outline {
        outline_paint(pdf, out, (dw, dh), sw, cur, zoom);
        return None;
    }
    let lay = layout(pdf, zoom);
    let off = thumbs_off(pdf, &lay, cur, dh as f64, zoom);
    let vis: Vec<usize> = (0..lay.len()).filter(|&i| lay[i].0 + lay[i].2 + lh > off && lay[i].0 < off + dh as f64).collect();
    let frame = strip_px(FRAME as f64, zoom).round() as i64;
    for &i in &vis {
        let (top, w, h) = lay[i];
        let pm = pdf.pixmap(i, THUMB_DPI * zoom);
        let (pw, ph) = if pm.w > 0 { (pm.w, pm.h) } else { (w as usize, h as usize) };
        let x0 = ((strip_w - pw as f64) / 2.0).round() as i64;
        let y0 = (top - off).round() as i64;
        if i == cur {
            fill(out, (dw, dh), sw, x0 - frame, y0 - frame, x0 + pw as i64 + frame, y0 + ph as i64 + frame, THUMB_ACTIVE);
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
        let lx = (strip_w - label_w(&n, zoom)) / 2.0;
        label(out, (dw, dh), sw, lx, top - off + h + lh - strip_px(3.0, zoom), &n, &advancer(zoom, 400, false), LABEL_INK);
    }
    Some((*vis.first()?, *vis.last()?))
}

/// The page whose thumbnail (its label band, or the gap above it) is at strip device row `y`.
pub fn at(pdf: &PdfDocument, y: f64, dh: f64, zoom: f64, scroll_y: f64) -> Option<usize> {
    let lay = layout(pdf, zoom);
    let off = thumbs_off(pdf, &lay, current(pdf, dh, zoom, scroll_y), dh, zoom);
    let y = y + off;
    let (gap, lh) = (strip_px(THUMB_GAP, zoom), strip_px(LABEL_H, zoom));
    lay.iter().position(|&(top, _, h)| y >= top - gap && y < top + h + lh)
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
        pdf.left = if pdf.thumbs { strip_css(z) } else { 0.0 };
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
fn outline_off(pdf: &PdfDocument, rows: &[usize], cur: usize, dh: f64, z: f64) -> f64 {
    let (gap, rh) = (strip_px(THUMB_GAP, z), strip_px(ROW_H, z));
    let ext = 2.0 * gap + rows.len() as f64 * rh;
    let rect = current_row(pdf, rows, cur).map_or((pdf.strip_off.get(), 0.0), |r| (gap + r as f64 * rh, rh));
    sync(pdf, ext, cur, rect, dh)
}

/// A parent's toggle at (`x`, `cy`): right-pointing when closed, down-pointing when open, `n` device px (8 at scale 1).
#[allow(clippy::too_many_arguments)]
fn toggle(out: &mut [u8], size: (usize, usize), sw: usize, x: i64, cy: i64, open: bool, n: i64) {
    if open {
        for k in 0..n / 2 {
            fill(out, size, sw, x + k, cy - n / 4 + k, x + n - k, cy - n / 4 + 1 + k, LABEL_INK);
        }
    } else {
        for k in 0..n {
            let w = 2 * (k.min(n - 1 - k) + 1);
            fill(out, size, sw, x, cy - n / 2 + k, x + w, cy - n / 2 + 1 + k, LABEL_INK);
        }
    }
}

/// Paint the outline rows (the backdrop is down): the current page's row band, each parent's toggle at its level's
/// indent, the title after it (SR387: in the item's ink and /F style, ellipsised at the strip's text edge), every length
/// at the engine's scale `z`.
fn outline_paint(pdf: &PdfDocument, out: &mut [u8], (dw, dh): (usize, usize), sw: usize, cur: usize, z: f64) {
    let rows = rows(pdf);
    let off = outline_off(pdf, &rows, cur, dh as f64, z);
    let act = current_row(pdf, &rows, cur);
    let (gap, rh) = (strip_px(THUMB_GAP, z), strip_px(ROW_H, z));
    for (r, &i) in rows.iter().enumerate() {
        let top = gap + r as f64 * rh - off;
        if top + rh <= 0.0 || top >= dh as f64 {
            continue;
        }
        let it = &pdf.outline[i];
        let y0 = top.round() as i64;
        if Some(r) == act {
            fill(out, (dw, dh), sw, 0, y0, sw as i64, y0 + rh.round() as i64, ROW_ACTIVE);
        }
        let x = strip_px(row_x(it.level), z);
        let cy = y0 + (rh / 2.0) as i64;
        if it.kids > 0 {
            let open = pdf.outline_open.get(i).copied().unwrap_or(false);
            toggle(out, (dw, dh), sw, x.round() as i64, cy, open, strip_px(8.0, z).round() as i64);
        }
        let adv = advancer(z, if it.bold { 700 } else { 400 }, it.italic);
        let tx = x + strip_px(TOGGLE, z);
        let title = ellipsize(&adv, &it.title, sw as f64 - tx - strip_px(PAD, z));
        label(out, (dw, dh), sw, tx, cy as f64 + (LABEL_PX * z as f32 * 0.35) as f64, &title, &adv, row_ink(it));
    }
}

/// The outline item at strip device (`x`, `y`), and whether the point is on its toggle (a parent's toggle column).
pub fn outline_at(pdf: &PdfDocument, (x, y): (f64, f64), dh: f64, zoom: f64, scroll_y: f64) -> Option<(usize, bool)> {
    let rows = rows(pdf);
    let off = outline_off(pdf, &rows, current(pdf, dh, zoom, scroll_y), dh, zoom);
    let r = (y + off - strip_px(THUMB_GAP, zoom)) / strip_px(ROW_H, zoom);
    if r < 0.0 {
        return None;
    }
    let i = *rows.get(r as usize)?;
    let it = &pdf.outline[i];
    let tx = strip_px(row_x(it.level), zoom);
    Some((i, it.kids > 0 && x >= tx - strip_px(2.0, zoom) && x < tx + strip_px(TOGGLE, zoom)))
}

/// The labels' shaper: the engine's sans-serif at [`LABEL_PX`] at the engine's scale `z`, in `weight` / `italic` (an
/// outline item's /F through the font pipeline's own selection).
pub fn advancer(z: f64, weight: u16, italic: bool) -> crate::fonts::shape::Advancer {
    crate::fonts::shape::Advancer::new(crate::fonts::FontSel::new(crate::fonts::SANS, weight, italic), LABEL_PX * z as f32, 0.0)
}

/// A page label's advance in device px at scale `z`.
fn label_w(text: &str, z: f64) -> f64 {
    advancer(z, 400, false).str(text) as f64
}

/// Draw `text` with its pen at device (`x`, `baseline`) through `adv` in `ink`, clipped to the strip's `sw` columns: the
/// engine's one glyph pipeline (`Advancer::place` + `raster::preblend` / `raster::draw_glyph`, as render's
/// `draw_shaped` composes them), plotted over the straight-RGBA frame.
#[allow(clippy::too_many_arguments)]
fn label(
    out: &mut [u8],
    (dw, dh): (usize, usize),
    sw: usize,
    x: f64,
    baseline: f64,
    text: &str,
    adv: &crate::fonts::shape::Advancer,
    ink: (u8, u8, u8),
) {
    use crate::fonts::raster;
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
        if let Some(kind) = refused(&a) {
            // PDF4-OUTLINE-ACTIONS (SR387): a refused kind is named on the engine's console line; nothing navigates.
            crate::js::host::console_out(1, &format!("[pdf] outline action refused: {kind}"));
            return;
        }
        self.pdf_follow(a);
    }
}

// =================================================================================================
// PDF4-OUTLINE-ACTIONS / -STYLE / -ELLIPSIS / PDF4-STRIP-HIDPI (LEDGER SR387): the refused kinds, the row's ink, the
// ellipsis and the ONE scale helper every strip length goes through.
// =================================================================================================

/// A refused outline row's title ink (Chromium's secondary text grey).
pub const MUTED: (u8, u8, u8) = (154, 160, 166);
/// The gap between an outline title's ellipsis and the strip's right edge, logical px.
pub const PAD: f64 = 6.0;

/// A strip length (the constants above are LOGICAL px) in device px at the engine's scale `z` (`AetherEngine::zoom`,
/// which is also devicePixelRatio): the strip, its rows, indents, gaps and label band all scale through here.
pub fn strip_px(logical: f64, z: f64) -> f64 {
    logical * z
}

/// The open strip's width in CSS px at scale `z` (the page lane starts after it: `PdfDocument::left`).
pub fn strip_css(z: f64) -> f64 {
    strip_px(STRIP_W, z) / z
}

/// The kind an outline click refuses: pdf_core's refused action by its own name (`LinkAction::Refused`: GoToR,
/// Launch, JavaScript, ...), or "Named" for a /Named outside the standard four (forms.rs's one table, `named_page`).
pub fn refused(a: &LinkAction) -> Option<&'static str> {
    match a {
        LinkAction::Refused(k) => Some(*k),
        LinkAction::Named(n) if super::forms::named_page(n, 0, 1).is_none() => Some("Named"),
        _ => None,
    }
}

/// An outline row's title ink: [`MUTED`] for a refused action, else its /C (black, the spec's default and invisible on
/// the strip, keeps [`LABEL_INK`]), else [`LABEL_INK`].
pub fn row_ink(it: &pdf_core::OutlineItem) -> (u8, u8, u8) {
    if it.action.as_ref().and_then(refused).is_some() {
        return MUTED;
    }
    match it.color {
        Some(c) if c != [0.0; 3] => {
            let b = |v: f64| (v * 255.0).round() as u8;
            (b(c[0]), b(c[1]), b(c[2]))
        }
        _ => LABEL_INK,
    }
}

/// `text` as it fits `avail` device px through `adv`: whole when it fits, else cut at the glyph cluster where the
/// Advancer's per-glyph widths pass `avail` less the ellipsis's width, with `…` after it.
pub fn ellipsize<'a>(adv: &crate::fonts::shape::Advancer, text: &'a str, avail: f64) -> Cow<'a, str> {
    if adv.str(text) as f64 <= avail {
        return Cow::Borrowed(text);
    }
    let room = avail - adv.str("\u{2026}") as f64;
    let mut cut = text.len();
    let mut pen = 0.0f64;
    for g in adv.shape(text).glyphs.iter() {
        pen += (g.adv * adv.size) as f64;
        if pen > room {
            cut = g.cluster.min(text.len());
            break;
        }
    }
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    Cow::Owned(format!("{}\u{2026}", text[..cut].trim_end()))
}
