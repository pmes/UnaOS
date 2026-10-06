// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Matrix — kernel-by-ruling R50
//!
//! QUARRY3 (rmbp-ledger B413, MACPARITY row 27) — Quarry's ICON VIEW. Quarry is the Finder by ruling (R50).
//!
//! The list pane as a grid: one cell per entry, a 64 px (logical) icon with the name centred below it. A program
//! draws its own icon through APPRES (`fs::appres::blit_path_icon`, the sighted icon else the generic one); a
//! folder and a document draw OUR glyphs — a tabbed folder, a page with a folded corner and the type's short tag
//! (the extension) across it. The selection is the model's `list_sel` (the same row the list view selects, so
//! switching views keeps it); `list_scroll` is read as the first ITEM shown and kept on a row boundary here.
//!
//! Press: select; a double press opens (the list's `is_double` + `activate_row`, no second gesture). Arrows: left /
//! right step one cell, up / down one row.

use alloc::string::String;
use alloc::vec::Vec;

use super::{fill, text, theme, Act, Geom, Model, NodeKind, Pane, Rect, MODEL, PAD};

/// The icon side, physical px.
pub(super) fn icon_px() -> usize {
    crate::ui::px(64)
}

/// One grid cell's `(w, h)` for this geometry: the icon plus air, and two name rows below it.
pub(super) fn cell(g: &Geom) -> (usize, usize) {
    let w = (icon_px() + 4 * PAD()).max(12 * g.cell_w());
    (w, icon_px() + 2 * PAD() + g.cell_h() + g.ts)
}

/// `(columns, visible rows)` of the grid inside `li`.
pub(super) fn grid(g: &Geom, li: Rect) -> (usize, usize) {
    let (cw, ch) = cell(g);
    ((li.w / cw).max(1), (li.h / ch).max(1))
}

/// The first row shown, from `list_scroll` (an item index), clamped so the last page is full.
fn top_row(len: usize, scroll: usize, cols: usize, vis: usize) -> usize {
    let rows = len.div_ceil(cols);
    (scroll / cols).min(rows.saturating_sub(vis))
}

/// The item under `(sx, sy)` in the grid inside `li`, or `None`.
pub(super) fn hit(g: &Geom, li: Rect, len: usize, scroll: usize, sx: usize, sy: usize) -> Option<usize> {
    if !li.contains(sx, sy) {
        return None;
    }
    let (cw, ch) = cell(g);
    let (cols, vis) = grid(g, li);
    let (c, r) = ((sx - li.x) / cw, (sy - li.y) / ch);
    if c >= cols || r >= vis {
        return None;
    }
    let i = (top_row(len, scroll, cols, vis) + r) * cols + c;
    (i < len).then_some(i)
}

/// The centre of item `i`'s cell (the fixture's press point), or `None` when it is not on screen.
pub(super) fn centre_of(g: &Geom, li: Rect, len: usize, scroll: usize, i: usize) -> Option<(usize, usize)> {
    let (cw, ch) = cell(g);
    let (cols, vis) = grid(g, li);
    let t = top_row(len, scroll, cols, vis);
    let r = (i / cols).checked_sub(t)?;
    (r < vis).then(|| (li.x + (i % cols) * cw + cw / 2, li.y + r * ch + ch / 2))
}

fn fill_raw(px: &mut [u32], stride: usize, h: usize, x: usize, y: usize, w: usize, hh: usize, c: u32) {
    for r in y..(y + hh).min(h) {
        for col in x..(x + w).min(stride) {
            px[r * stride + col] = c;
        }
    }
}

/// The short type tag a document glyph carries: its extension, upper-cased, at most four characters.
pub(super) fn tag_of(name: &str) -> String {
    match name.rfind('.') {
        Some(i) if i > 0 && i + 1 < name.len() => name[i + 1..].chars().take(4).collect::<String>().to_ascii_uppercase(),
        _ => String::new(),
    }
}

/// Draw the icon for `path` (`dir` = a folder) `s` px square at `(x, y)` into a `stride`-wide surface of `h`
/// rows. Shared with Quick Look's card. Returns the source: `appres`, `folder` or `document`.
pub(super) fn draw_icon(px: &mut [u32], stride: usize, h: usize, x: usize, y: usize, s: usize, path: &str, dir: bool, face: crate::video::text::Face) -> &'static str {
    if !dir && crate::fs::appres::blit_path_icon(px, stride, h, x, y, s, path) {
        return "appres";
    }
    let u = (s / 16).max(1);
    if dir {
        // A tabbed folder: the tab, then the body.
        fill_raw(px, stride, h, x + u, y + 3 * u, 6 * u, 2 * u, theme::control_mid());
        fill_raw(px, stride, h, x + u, y + 4 * u, 14 * u, 10 * u, theme::control_mid());
        fill_raw(px, stride, h, x + u, y + 6 * u, 14 * u, 8 * u, theme::control_zoom());
        return "folder";
    }
    // A page with a folded corner and the type tag.
    let (px0, pw, ph) = (x + 3 * u, 10 * u, 14 * u);
    fill_raw(px, stride, h, px0, y + u, pw, ph, theme::frame_line());
    fill_raw(px, stride, h, px0 + 1, y + u + 1, pw.saturating_sub(2), ph.saturating_sub(2), theme::bevel_light());
    for i in 0..3 * u {
        fill_raw(px, stride, h, px0 + pw - 3 * u + i, y + u + i, 3 * u - i, 1, theme::content_fill());
    }
    fill_raw(px, stride, h, px0 + pw - 3 * u, y + 4 * u, 3 * u, 1, theme::frame_line());
    let leaf = path.rsplit('/').next().unwrap_or(path);
    let tag = tag_of(leaf);
    if !tag.is_empty() {
        let tw = tag.len() * face.cell_w();
        let bx = px0 + pw / 2 - (tw / 2).min(pw / 2);
        fill_raw(px, stride, h, px0 + 1, y + 9 * u, pw.saturating_sub(2), face.cell_h().min(4 * u), theme::accent());
        crate::video::text::draw_text(px, stride, (px0 + pw).min(stride), h, bx, y + 9 * u, tag.as_bytes(), theme::chrome_face(), true, face);
    }
    "document"
}

/// Paint the grid over the list pane's interior `li` (after the list painted, when the view is Icons).
pub(super) fn paint(m: &Model, px: &mut [u32], li: Rect) -> usize {
    let g = &m.geom;
    fill(px, g, li.x, li.y, li.w, li.h, theme::content_fill());
    if let Some(e) = &m.err {
        text(px, g, li.x + PAD(), li.y + PAD(), e.as_bytes(), li.x + li.w, theme::content_text());
        return 0;
    }
    let (cw, ch) = cell(g);
    let (cols, vis) = grid(g, li);
    let t = top_row(m.list.len(), m.list_scroll, cols, vis);
    let s = icon_px();
    let mut drawn = 0usize;
    for r in 0..vis {
        for c in 0..cols {
            let i = (t + r) * cols + c;
            let Some(e) = m.list.get(i) else { return drawn };
            let (x, y) = (li.x + c * cw, li.y + r * ch);
            let sel = i == m.list_sel;
            if sel {
                let c2 = if m.focus == Pane::List { theme::accent() } else { theme::scroll_thumb() };
                fill(px, g, x + PAD() / 2, y + PAD() / 2, cw - PAD(), ch - PAD(), c2);
            }
            let path = super::join(&m.cwd, &e.name);
            draw_icon(px, g.w, g.h, x + (cw - s) / 2, y + PAD(), s, &path, matches!(e.kind, NodeKind::Dir), g.face);
            // The name, centred under the icon; a name wider than the cell ends in `..`.
            let fit = (cw.saturating_sub(PAD()) / g.cell_w().max(1)).max(3);
            let leaf = e.name.rsplit('/').next().unwrap_or(&e.name);
            let mut nm: Vec<u8> = Vec::from(leaf.as_bytes());
            if nm.len() > fit {
                nm.truncate(fit - 2);
                nm.extend_from_slice(b"..");
            }
            let tw = nm.len() * g.cell_w();
            let ink = if sel && m.focus == Pane::List { theme::chrome_face() } else { theme::content_text() };
            text(px, g, x + cw.saturating_sub(tw) / 2, y + PAD() + s + PAD(), &nm, x + cw, ink);
            drawn += 1;
        }
    }
    drawn
}

/// A press in the list pane while the view is Icons. `None` = not ours.
pub(super) fn press(m: &mut Model, sx: usize, sy: usize) -> Option<Act> {
    let li = m.geom.list_pane().inner();
    if !li.contains(sx, sy) {
        return None;
    }
    m.focus = Pane::List;
    let Some(i) = hit(&m.geom, li, m.list.len(), m.list_scroll, sx, sy) else {
        m.click_ms = 0;
        return Some(Act::None);
    };
    let now = crate::arch::ms();
    let dbl = super::is_double(m.click_ms, now, m.click_row, i, m.click_pane == Pane::List);
    m.list_sel = i;
    m.click_row = i;
    m.click_pane = Pane::List;
    m.click_ms = if dbl { 0 } else { now };
    if dbl {
        return Some(m.activate_row(i));
    }
    Some(Act::None)
}

/// Keep `list_sel` on screen: `list_scroll` moves by whole rows.
fn follow(m: &mut Model) {
    let li = m.geom.list_pane().inner();
    let (cols, vis) = grid(&m.geom, li);
    let row = m.list_sel / cols;
    let t = top_row(m.list.len(), m.list_scroll, cols, vis);
    let t = if row < t { row } else if row >= t + vis { row + 1 - vis } else { t };
    m.list_scroll = t * cols;
}

/// Arrow keys while the view is Icons and the list has the keyboard. `true` = consumed.
pub(super) fn key(c: u8) -> bool {
    if !matches!(c, 0x1C..=0x1F) {
        return false;
    }
    let mut guard = MODEL.lock();
    let Some(m) = guard.as_mut() else { return false };
    if m.focus != Pane::List || m.list.is_empty() {
        return false;
    }
    let cols = grid(&m.geom, m.geom.list_pane().inner()).0;
    let n = m.list.len();
    let s = m.list_sel;
    m.list_sel = match c {
        0x1D => s.saturating_sub(1),
        0x1C => (s + 1).min(n - 1),
        0x1F => s.saturating_sub(cols),
        _ => (s + cols).min(n - 1),
    };
    follow(m);
    true
}
