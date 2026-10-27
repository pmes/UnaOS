// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Matrix — kernel-by-ruling R50
//!
//! QUARRY3 (rmbp-ledger B413, MACPARITY row 27) — Quarry's SIDEBAR. Quarry is the Finder by ruling (R50).
//! Above the tree, two sections:
//!
//! * **Favorites** — Home, Desktop, Documents, Downloads, Applications (`/apps`), Trash. Home and the Trash are
//!   `fs::trash`'s own answers (`home_base`, `trash_dir`); a favorite that does not exist on this volume is drawn
//!   dim and says so when pressed — never a silent dead row.
//! * **Locations** — the volumes, read from `/volumes` through Quarry's one listing seam (`collect`, the R89
//!   published view). A removable volume (`fs::removable`, USBSTOR) carries an EJECT glyph at the row's right
//!   edge; a press on it is `fs::removable::eject` — the detach path's unmount, the disk parked until replug.
//!
//! No store of its own (R79): the rows are re-derived when the volume generation moves (Quarry's `volume_gen`),
//! on the listing pass (`after_show`), never in the painter. Lock order: `MODEL` then `SIDE`.
//!
//! Witness at open: `[quarry3] sidebar favorites=<n>/6 locations=<names> removable=<names|->`.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use super::{fill, text, theme, Act, Geom, Model, Pane, Rect, PAD};

/// One sidebar row.
#[derive(Clone)]
pub(super) struct Ent {
    pub label: String,
    /// Empty for a section header.
    pub path: String,
    pub removable: bool,
    /// The path resolves on the live table.
    pub present: bool,
}

static SIDE: crate::sync::Mutex<Vec<Ent>> = crate::sync::Mutex::new(Vec::new());
/// Rows the sidebar holds (headers included), read by the GEOMETRY (`Geom::tree_pane`) without a lock.
static ROWS: AtomicUsize = AtomicUsize::new(10);
/// The volume generation the rows were derived against. `u64::MAX` = never.
static GEN: AtomicU64 = AtomicU64::new(u64::MAX);
/// QUARRYHOVER (B647): the first row shown — the sidebar scrolls when its rows outgrow its height (`h` is capped at
/// half the panes'). Clamped at every read ([`scroll_of`]), so a re-derive needs no reset. SIDEBARWHEEL (B792): the
/// offset store of the sidebar's pane on wm's one model ([`view`]); the step applies it, nothing else computes it.
static SCROLL: AtomicUsize = AtomicUsize::new(0);

/// Rows the sidebar can show at this geometry (the painter's own cut-off: a row must fit whole).
pub(super) fn vis(g: &Geom) -> usize {
    vis_for(g, ROWS.load(Ordering::Relaxed))
}

/// [`vis`] for a sidebar of `n` rows.
fn vis_for(g: &Geom, n: usize) -> usize {
    h_for(g, n).saturating_sub(g.ts) / g.row_h().max(1)
}

/// The first row shown, clamped so the last page is full.
fn scroll_of(g: &Geom) -> usize {
    SCROLL.load(Ordering::Relaxed).min(ROWS.load(Ordering::Relaxed).saturating_sub(vis(g)))
}

/// The raw offset (the fixture saves it) and its restore.
pub(super) fn scroll() -> usize {
    SCROLL.load(Ordering::Relaxed)
}
pub(super) fn set_scroll(s: usize) {
    SCROLL.store(s, Ordering::Relaxed);
}

/// QUARRYHOVER (B647): the row under surface `(sx, sy)` — a real row (never a header), by the press's arithmetic.
pub(super) fn row_at(g: &Geom, sx: usize, sy: usize) -> Option<usize> {
    let r = rect(g);
    if !r.contains(sx, sy) || sy < r.y + g.ts {
        return None;
    }
    let k = (sy - r.y - g.ts) / g.row_h().max(1);
    if k >= vis(g) {
        return None;
    }
    let i = k + scroll_of(g);
    SIDE.lock().get(i).filter(|e| !e.path.is_empty()).map(|_| i)
}

/// The favorites, in the Finder's order: (label, path).
pub(super) fn favorites() -> [(&'static str, String); 6] {
    let home = crate::fs::trash::home_base();
    [
        ("Home", home.clone()),
        ("Desktop", alloc::format!("{}/Desktop", home)),
        ("Documents", alloc::format!("{}/Documents", home)),
        ("Downloads", alloc::format!("{}/Downloads", home)),
        ("Applications", String::from("/apps")),
        ("Trash", crate::fs::trash::trash_dir()),
    ]
}

/// The sidebar's height in surface px: its rows plus a little air, never more than half the panes' height.
pub(super) fn h(g: &Geom) -> usize {
    h_for(g, ROWS.load(Ordering::Relaxed))
}

/// [`h`] for a sidebar of `n` rows.
fn h_for(g: &Geom, n: usize) -> usize {
    let want = n * g.row_h() + 2 * g.ts;
    want.min(g.h.saturating_sub(g.bar_h()) / 2)
}

/// The sidebar's rectangle (left column, between the toolbar and the tree).
pub(super) fn rect(g: &Geom) -> Rect {
    rect_for(g, ROWS.load(Ordering::Relaxed))
}

/// [`rect`] for a sidebar of `n` rows.
fn rect_for(g: &Geom, n: usize) -> Rect {
    Rect { x: 0, y: g.bar_h(), w: g.tree_w(), h: h_for(g, n) }
}

/// Re-derive the rows when the volume generation moved (or never ran). Called on the listing pass.
pub(super) fn refresh(force: bool) {
    let vg = super::volume_gen();
    if !force && GEN.load(Ordering::Relaxed) == vg {
        return;
    }
    GEN.store(vg, Ordering::Relaxed);
    let mt = crate::shell::vfs_mount_table();
    let mut rows: Vec<Ent> = Vec::new();
    rows.push(Ent { label: String::from("Favorites"), path: String::new(), removable: false, present: false });
    for (l, p) in favorites().iter() {
        let present = matches!(mt.stat(p), Ok(s) if matches!(s.kind, crate::fs::vfs::NodeKind::Dir));
        rows.push(Ent { label: String::from(*l), path: p.clone(), removable: false, present });
    }
    for p in super::dragdrop::user_favorites() { // DRAGDROP (B440): the folders the user dropped on Favorites (Principia's `system.quarry.favorites`)
        let present = matches!(mt.stat(&p), Ok(s) if matches!(s.kind, crate::fs::vfs::NodeKind::Dir));
        rows.push(Ent { label: String::from(p.rsplit('/').next().unwrap_or(&p)), path: p, removable: false, present });
    }
    rows.push(Ent { label: String::from("Locations"), path: String::new(), removable: false, present: false });
    let removable = crate::fs::removable::mounted_names();
    if let Ok((true, vols)) = super::collect(crate::fs::bootdisk::VOLUMES) {
        for v in vols.iter() {
            let path = alloc::format!("{}/{}", crate::fs::bootdisk::VOLUMES, v.name);
            rows.push(Ent { label: v.name.clone(), path, removable: removable.iter().any(|r| *r == v.name), present: true });
        }
    }
    ROWS.store(rows.len(), Ordering::Relaxed);
    *SIDE.lock() = rows;
}

/// The rows now (a copy, for the fixture and the witness).
pub(super) fn rows() -> Vec<Ent> {
    SIDE.lock().clone()
}

/// `[quarry3] sidebar …` — once per open.
pub(super) fn witness() {
    let r = rows();
    let favs = r.iter().filter(|e| !e.path.is_empty() && !e.path.starts_with(crate::fs::bootdisk::VOLUMES)).count();
    let fav_ok = r.iter().filter(|e| !e.path.is_empty() && !e.path.starts_with(crate::fs::bootdisk::VOLUMES) && e.present).count();
    let locs: Vec<&str> = r.iter().filter(|e| e.path.starts_with(crate::fs::bootdisk::VOLUMES)).map(|e| e.label.as_str()).collect();
    let rem: Vec<&str> = r.iter().filter(|e| e.removable).map(|e| e.label.as_str()).collect();
    serial_println!(
        "[quarry3] sidebar favorites={}/{} locations={} removable={}",
        fav_ok,
        favs,
        if locs.is_empty() { String::from("-") } else { locs.join(",") },
        if rem.is_empty() { String::from("-") } else { rem.join(",") }
    );
}

/// The eject glyph: a triangle over a bar, our drawing, `s` px square at `(x, y)`.
fn eject_glyph(px: &mut [u32], g: &Geom, x: usize, y: usize, s: usize, c: u32) {
    let tri = s * 3 / 5;
    for i in 0..tri {
        let half = (i * s) / (2 * tri.max(1));
        fill(px, g, x + s / 2 - half, y + i, 2 * half + 1, 1, c);
    }
    fill(px, g, x, y + tri + s / 6, s, (s / 6).max(1), c);
}

/// Paint the sidebar over its rectangle (after the panes are drawn): the ground, the keyline, then the rows through
/// the one row painter ([`rows_paint`]). The bar is wm's (`live::panes_register`).
pub(super) fn paint(m: &Model, px: &mut [u32]) {
    let g = &m.geom;
    let r = rect(g);
    if r.h == 0 {
        return;
    }
    fill(px, g, r.x, r.y, r.w, r.h, theme::surface_window());
    fill(px, g, r.x, r.y + r.h - 1, r.w, 1, theme::line_subtle());
    let _ = rows_paint(m, px, 0, vis(g));
}

/// A press inside the sidebar: navigate to the row's place, or eject a removable. `None` = not ours.
pub(super) fn press(m: &mut Model, sx: usize, sy: usize) -> Option<Act> {
    let g = m.geom;
    let r = rect(&g);
    if !r.contains(sx, sy) {
        return None;
    }
    let i = (sy.saturating_sub(r.y + g.ts)) / g.row_h() + scroll_of(&g); // QUARRYHOVER (B647): past the scrolled-off rows
    let Some(e) = SIDE.lock().get(i).cloned() else { return Some(Act::None) };
    if e.path.is_empty() {
        return Some(Act::None);
    }
    let w = r.w.saturating_sub(if view(&g).is_some() { super::SBW() } else { 0 }); // SIDEBARWHEEL (B792): the glyph sits left of the gutter
    if e.removable && sx >= r.x + w - g.row_h() && sx < r.x + w {
        let ok = crate::fs::removable::eject(&e.label);
        serial_println!("[quarry3] eject volume={} -> {}", e.label, if ok { "ok" } else { "not-mounted" });
        m.invalidate();
        m.reload_roots();
        if m.cwd.starts_with(&e.path) {
            m.navigate(crate::fs::bootdisk::VOLUMES);
        }
        refresh(true);
        m.status = Some(alloc::format!("ejected {}", e.label));
        return Some(Act::None);
    }
    if !e.present {
        m.status = Some(alloc::format!("{} is not on this volume ({})", e.label, e.path));
        return Some(Act::None);
    }
    m.status = None; // NAVTRAVEL (B841): cleared BEFORE the navigate, so a deferred one leaves `opening <leaf>` showing
    m.navigate(&e.path);
    m.focus = Pane::List;
    serial_println!("[quarry3] sidebar press row={} -> {}", e.label, e.path);
    Some(Act::None)
}

// ── SIDEBARWHEEL (rmbp-ledger B792) — the sidebar is a pane on wm's one scroll model — TAIL-APPENDED ───────────────
// SCROLLPANES (B777) owed item 1: the sidebar kept its own wheel. Now it registers a `wm::ScrollView` like the tree
// ([`view`]: viewport = its rows left of the `SBW()` gutter, line = the row pitch, offset = [`SCROLL`] x pitch) from
// `live::panes_register`; the wheel, the gutter press and the thumb drag are wm's, and the step (`live::side_scrolled`
// -> `live::pane_step`) runs [`step_in`]: `wm::scroll_blit` and the strip rows through THE row painter
// ([`rows_paint_in`]). Lock order MODEL -> SURF -> SIDE (repaint's). No arithmetic of its own.

/// The sidebar's model for `n` rows with `top` the first shown: `(view, gutter_w)`, `None` when they fit.
pub(super) fn view_for(g: &Geom, n: usize, top: usize) -> Option<(crate::video::wm::ScrollView, usize)> {
    let (rh, vis) = (g.row_h(), vis_for(g, n));
    if rh == 0 || vis == 0 || n <= vis {
        return None;
    }
    let r = rect_for(g, n);
    let vp = (r.x, r.y + g.ts, r.w.saturating_sub(super::SBW()), vis * rh);
    Some((crate::video::wm::ScrollView { offset: top.min(n - vis) * rh, extent: n * rh, viewport: vp, line: rh }, super::SBW()))
}

/// The live sidebar's model (its rows, its offset).
pub(super) fn view(g: &Geom) -> Option<(crate::video::wm::ScrollView, usize)> {
    view_for(g, ROWS.load(Ordering::Relaxed), SCROLL.load(Ordering::Relaxed))
}

/// Rows `[r0, r1)` of the viewport (view-relative) exactly as the full paint draws them, `top` the first row shown: the
/// ground left of the gutter (the keyline put back when the band covers it), then each row — a header, the here
/// accent or the hover wash, the label, the eject glyph. THE sidebar row painter. Returns the pixels painted.
pub(super) fn rows_paint_in(m: &Model, px: &mut [u32], rows: &[Ent], top: usize, r0: usize, r1: usize) -> usize {
    let g = &m.geom;
    let n = rows.len();
    let r = rect_for(g, n);
    let (row_h, vis) = (g.row_h(), vis_for(g, n));
    let (r0, r1) = (r0.min(vis), r1.min(vis));
    if r.h == 0 || row_h == 0 || r0 >= r1 {
        return 0;
    }
    let w = r.w.saturating_sub(if n > vis { super::SBW() } else { 0 });
    let (y0, y1) = (r.y + g.ts + r0 * row_h, r.y + g.ts + r1 * row_h);
    fill(px, g, r.x, y0, w, y1 - y0, theme::surface_window());
    let line = r.y + r.h - 1;
    if line >= y0 && line < y1 {
        fill(px, g, r.x, line, w, 1, theme::line_subtle());
    }
    for k in r0..r1 {
        let i = top + k;
        let Some(e) = rows.get(i) else { break };
        let y = r.y + g.ts + k * row_h;
        if e.path.is_empty() {
            text(px, g, r.x + PAD(), y + g.ts, e.label.as_bytes(), r.x + w, theme::text_secondary());
            continue;
        }
        let here = m.cwd == e.path;
        if here {
            fill(px, g, r.x + PAD() / 2, y, w - PAD(), row_h, theme::accent());
        } else if m.hover == super::Hover::Side(i) {
            super::hover_wash(px, g, r.x + PAD() / 2, y, w - PAD(), row_h); // QUARRYHOVER (B647): the kit's hover wash under the pointer
        }
        let ink = if here {
            theme::surface_window()
        } else if e.present {
            theme::text_primary()
        } else {
            theme::text_secondary()
        };
        let ej = if e.removable { row_h } else { 0 };
        text(px, g, r.x + 3 * PAD(), y + g.ts, e.label.as_bytes(), r.x + w - ej, ink);
        if e.removable {
            let s = row_h.saturating_sub(4 * g.ts).max(4);
            eject_glyph(px, g, r.x + w - row_h, y + (row_h - s) / 2, s, ink);
        }
    }
    (y1 - y0) * w
}

/// [`rows_paint_in`] over the live rows at the live offset.
pub(super) fn rows_paint(m: &Model, px: &mut [u32], r0: usize, r1: usize) -> usize {
    let top = scroll_of(&m.geom);
    let side = SIDE.lock();
    rows_paint_in(m, px, &side, top, r0, r1)
}

/// The step on the surface: the first row shown after it, and the pixels painted (the one rect-scoped blit, then the
/// strip rows through [`rows_paint_in`]). The sidebar has no selection to pull. The probe drives this same body.
pub(super) fn step_in(m: &Model, px: &mut [u32], rows: &[Ent], st: &crate::video::wm::ScrollStep) -> (usize, usize) {
    let rh = m.geom.row_h().max(1);
    let top = st.off / rh;
    if st.moved == 0 {
        return (top, 0);
    }
    let _ = crate::video::wm::scroll_blit(px, m.geom.w, st);
    let y0 = st.viewport.1;
    let (r0, r1) = ((st.strip.0 - y0) / rh, (st.strip.1 - y0).div_ceil(rh));
    (top, rows_paint_in(m, px, rows, top, r0, r1))
}

/// The live step ([`step_in`] over the live rows), its offset stored.
pub(super) fn step(m: &Model, px: &mut [u32], st: &crate::video::wm::ScrollStep) -> usize {
    let (top, n) = {
        let side = SIDE.lock();
        step_in(m, px, &side, st)
    };
    SCROLL.store(top, Ordering::Relaxed);
    n
}
