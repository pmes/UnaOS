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
    let rows = ROWS.load(Ordering::Relaxed);
    let want = rows * g.row_h() + 2 * g.ts;
    want.min(g.h.saturating_sub(g.bar_h()) / 2)
}

/// The sidebar's rectangle (left column, between the toolbar and the tree).
pub(super) fn rect(g: &Geom) -> Rect {
    Rect { x: 0, y: g.bar_h(), w: g.tree_w(), h: h(g) }
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

/// Paint the sidebar over its rectangle (after the panes are drawn).
pub(super) fn paint(m: &Model, px: &mut [u32]) {
    let g = &m.geom;
    let r = rect(g);
    if r.h == 0 {
        return;
    }
    fill(px, g, r.x, r.y, r.w, r.h, theme::chrome_face());
    fill(px, g, r.x, r.y + r.h - 1, r.w, 1, theme::frame_line());
    let row_h = g.row_h();
    let side = SIDE.lock();
    for (i, e) in side.iter().enumerate() {
        let y = r.y + g.ts + i * row_h;
        if y + row_h > r.y + r.h {
            break;
        }
        if e.path.is_empty() {
            text(px, g, r.x + PAD(), y + g.ts, e.label.as_bytes(), r.x + r.w, theme::title_text_inactive());
            continue;
        }
        let here = m.cwd == e.path;
        if here {
            fill(px, g, r.x + PAD() / 2, y, r.w - PAD(), row_h, theme::accent());
        }
        let ink = if here {
            theme::chrome_face()
        } else if e.present {
            theme::content_text()
        } else {
            theme::title_text_inactive()
        };
        let ej = if e.removable { row_h } else { 0 };
        text(px, g, r.x + 3 * PAD(), y + g.ts, e.label.as_bytes(), r.x + r.w - ej, ink);
        if e.removable {
            let s = row_h.saturating_sub(4 * g.ts).max(4);
            eject_glyph(px, g, r.x + r.w - row_h, y + (row_h - s) / 2, s, ink);
        }
    }
}

/// A press inside the sidebar: navigate to the row's place, or eject a removable. `None` = not ours.
pub(super) fn press(m: &mut Model, sx: usize, sy: usize) -> Option<Act> {
    let g = m.geom;
    let r = rect(&g);
    if !r.contains(sx, sy) {
        return None;
    }
    let i = (sy.saturating_sub(r.y + g.ts)) / g.row_h();
    let Some(e) = SIDE.lock().get(i).cloned() else { return Some(Act::None) };
    if e.path.is_empty() {
        return Some(Act::None);
    }
    if e.removable && sx >= r.x + r.w - g.row_h() {
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
    m.navigate(&e.path);
    m.focus = Pane::List;
    m.status = None;
    serial_println!("[quarry3] sidebar press row={} -> {}", e.label, e.path);
    Some(Act::None)
}
