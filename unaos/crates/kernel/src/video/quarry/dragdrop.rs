// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Matrix — kernel-by-ruling R50
//!
//! DRAGDROP (rmbp-ledger B440, MACPARITY row 18) — Quarry as the drag session's first participant (`video::dnd`
//! owns the session). Quarry is the Finder by ruling (R50). A child module of `live` like `sidebar`/`ops`, so it
//! reads the model without widening it.
//!
//! * SOURCE — a press on a list row or an icon that selected it (not a double press) arms a drag of that item
//!   ([`arm_after_press`]); the session starts only past its threshold, so a click stays a click.
//! * `drop_ok` ([`drop_ok`]) — over Quarry's window: a FOLDER row / icon / tree row is a folder target; the list's
//!   empty area is the shown folder; the sidebar's Favorites section takes a FOLDER (adds a favorite); a Locations
//!   row takes anything (copies onto that volume). Never the item itself, its own folder, or into itself.
//! * `drop` ([`drop`]) — within a volume a MOVE (`ops::op_rename`), across volumes a COPY (`ops::op_copy`); Option
//!   forces the copy. The ops are the menu's own bodies (DIRNS-confined; a refusal is `ok=0` with its reason).
//! * Favorites the user adds are Principia's (R79): `system.quarry.favorites`, comma-joined paths, read by the
//!   sidebar's refresh ([`user_favorites`]) and written through the PrefSet bus.
//! * The hover highlight ([`paint`]) — an accent outline on the hovered row, icon, sidebar row or pane.
//!
//! `tests dragdrop` ([`selftest`]) drives the session through the capture seam with explicit points and a fixture
//! resolver over real ops on a folder under the user's home:
//! `:: DRAGDROP: session=ok move=ok copy=ok trash=ok cancel=ok -> PASS ::`.

use alloc::string::String;
use alloc::vec::Vec;

use super::{fill, join, leaf, sidebar, theme, toolbar, wm, Model, NodeKind, Pane, Rect, MODEL, WIN};
use crate::video::dnd::{self, Action, Kind, Payload, Target};
use core::sync::atomic::Ordering;

// ── the source ──────────────────────────────────────────────────────────────────────────────────

/// The list index at source point `(sx, sy)` in the list pane (either view), or `None`.
fn list_index_at(m: &Model, sx: usize, sy: usize) -> Option<usize> {
    let g = m.geom;
    let li = g.list_pane().inner();
    if !li.contains(sx, sy) {
        return None;
    }
    if toolbar::view() == toolbar::View::Icons {
        return super::iconview::hit(&g, li, m.list.len(), m.list_scroll, sx, sy);
    }
    let body_y = li.y + g.row_h();
    if sy < body_y {
        return None;
    }
    let r = (sy - body_y) / g.row_h();
    let i = m.list_scroll + r;
    (r < m.list_visible() && i < m.list.len()).then_some(i)
}

/// After Quarry handled a primary press at panel `(x, y)` = source `(sx, sy)`: when it selected a list row or icon
/// (a single press — a double press opened it), arm a drag of that item.
pub(super) fn arm_after_press(x: i32, y: i32, sx: usize, sy: usize) {
    let p = {
        let g = MODEL.lock();
        let Some(m) = g.as_ref() else { return };
        if m.focus != Pane::List || m.click_ms == 0 || m.click_pane != Pane::List {
            return;
        }
        let Some(i) = list_index_at(m, sx, sy) else { return };
        if i != m.list_sel {
            return;
        }
        let e = &m.list[i];
        if e.name == "." || e.name == ".." {
            return;
        }
        Payload { kind: Kind::File, paths: alloc::vec![join(&m.cwd, &e.name)], dir: matches!(e.kind, NodeKind::Dir), from: WIN.load(Ordering::Relaxed), label: e.name.clone() }
    };
    dnd::arm(p, x, y);
}

// ── drop_ok ─────────────────────────────────────────────────────────────────────────────────────

fn parent(p: &str) -> &str {
    match p.rfind('/') {
        Some(0) => "/",
        Some(i) => &p[..i],
        None => "/",
    }
}

fn under(path: &str, dir: &str) -> bool {
    path.len() > dir.len() && path.as_bytes()[..dir.len()].eq_ignore_ascii_case(dir.as_bytes()) && (dir.ends_with('/') || path.as_bytes()[dir.len()] == b'/')
}

/// May `p` drop INTO folder `dir`? Never onto itself, into itself, or back into its own folder.
fn folder_ok(p: &Payload, dir: &str) -> bool {
    p.paths.iter().all(|s| !s.eq_ignore_ascii_case(dir) && !under(dir, s) && !parent(s).eq_ignore_ascii_case(dir))
}

/// Panel point -> Quarry source point, when Quarry's window is the top-most at it.
fn to_source(x: i32, y: i32) -> Option<(wm::WinId, usize, usize)> {
    let id = WIN.load(Ordering::Relaxed);
    if id == wm::WIN_NONE || x < 0 || y < 0 {
        return None;
    }
    match wm::hit_test(x, y) {
        Some((w, _, _)) if w == id => {}
        _ => return None,
    }
    let info = wm::info(id)?;
    let scale = info.scale.max(1);
    if (x as usize) < info.x || (y as usize) < info.y {
        return None;
    }
    let (sx, sy) = ((x as usize - info.x) / scale, (y as usize - info.y) / scale);
    (sx < info.w && sy < info.h).then_some((id, sx, sy))
}

/// The sidebar row index at a source point (headers included), or `None` outside the sidebar.
fn sidebar_row_at(m: &Model, sx: usize, sy: usize) -> Option<usize> {
    let g = m.geom;
    let r = sidebar::rect(&g);
    if !r.contains(sx, sy) || sy < r.y + g.ts {
        return None;
    }
    Some((sy - r.y - g.ts) / g.row_h())
}

/// The target at source `(sx, sy)` of Quarry's model, if `p` may drop there.
fn target_in(m: &Model, win: wm::WinId, sx: usize, sy: usize, p: &Payload) -> Option<Target> {
    let g = m.geom;
    if let Some(i) = sidebar_row_at(m, sx, sy) {
        let rows = sidebar::rows();
        let e = rows.get(i)?;
        let vol = e.path.starts_with(crate::fs::bootdisk::VOLUMES);
        let in_favs = !vol && rows[..=i].iter().rev().find(|r| r.path.is_empty()).map(|h| h.label == "Favorites").unwrap_or(false);
        if in_favs || (e.path.is_empty() && e.label == "Favorites") {
            return p.dir.then_some(Target::Favorites);
        }
        return (vol && folder_ok(p, &e.path)).then(|| Target::Volume { dir: e.path.clone() });
    }
    let tp = g.tree_pane();
    if tp.contains(sx, sy) {
        let ti = tp.inner();
        if sy < ti.y {
            return None;
        }
        let i = m.tree_scroll + (sy - ti.y) / g.row_h();
        let dir = m.tree.get(i)?.path.clone();
        return folder_ok(p, &dir).then_some(Target::Folder { win, dir });
    }
    if g.list_pane().inner().contains(sx, sy) {
        let dir = match list_index_at(m, sx, sy) {
            Some(i) if matches!(m.list[i].kind, NodeKind::Dir) => join(&m.cwd, &m.list[i].name),
            Some(_) => return None,
            None => m.cwd.clone(),
        };
        return folder_ok(p, &dir).then_some(Target::Folder { win, dir });
    }
    None
}

/// `drop_ok(kind)` at panel `(x, y)` — the session's question to Quarry.
pub fn drop_ok(x: i32, y: i32, p: &Payload) -> Option<Target> {
    if p.kind != Kind::File {
        return None;
    }
    let (win, sx, sy) = to_source(x, y)?;
    let g = MODEL.lock();
    target_in(g.as_ref()?, win, sx, sy, p)
}

// ── drop ────────────────────────────────────────────────────────────────────────────────────────

/// The mount a path lives on: the longest mount prefix of the live table that contains it.
fn volume_of(mounts: &[String], path: &str) -> String {
    mounts.iter().filter(|m| m.as_str() == "/" || path.eq_ignore_ascii_case(m) || under(path, m)).max_by_key(|m| m.len()).cloned().unwrap_or_else(|| String::from("/"))
}

/// The mount prefixes as of Quarry's last `reload_roots` (the model's own copy; no second table read).
fn mounts() -> Vec<String> {
    MODEL.lock().as_ref().map(|m| m.mounts.clone()).unwrap_or_default()
}

/// Re-read the shown folder and the sidebar after a drop; repaint.
fn refresh_after() {
    if let Some(m) = MODEL.lock().as_mut() {
        let c = m.cwd.clone();
        m.invalidate();
        m.show(&c);
        m.settle();
    }
    sidebar::refresh(true);
    super::repaint();
}

/// `drop(kind, payload)` on a Quarry target. Returns what it did and whether it landed.
pub fn drop(p: &Payload, t: &Target, option: bool) -> (Action, bool) {
    let r = match t {
        Target::Favorites => {
            let mut ok = p.dir;
            for s in p.paths.iter() {
                ok &= add_favorite(s);
            }
            (Action::Favorite, ok)
        }
        Target::Folder { dir, .. } | Target::Volume { dir } => {
            let ms = mounts();
            let mut ok = true;
            let mut act = Action::Move;
            for s in p.paths.iter() {
                let same = volume_of(&ms, s).eq_ignore_ascii_case(&volume_of(&ms, dir));
                let r = if same && !option && !matches!(t, Target::Volume { .. }) {
                    super::ops::op_rename(s, &join(dir, &leaf(s)))
                } else {
                    act = Action::Copy;
                    super::ops::op_copy(s, dir).map(|_| ())
                };
                if let Err(e) = r {
                    serial_println!("[dnd] {} src={} dst={} reason={}", if act == Action::Copy { "copy" } else { "move" }, s, dir, e);
                    ok = false;
                }
            }
            (act, ok)
        }
        Target::Trash => (Action::Trash, false), // the dock's, never routed here
    };
    refresh_after();
    r
}

// ── favorites (Principia's, R79) ────────────────────────────────────────────────────────────────

static FAVS: spin::Mutex<Option<Vec<String>>> = spin::Mutex::new(None);

/// The favorites the user added, in order (`system.quarry.favorites`; read once, then kept with every add).
pub(super) fn user_favorites() -> Vec<String> {
    let mut g = FAVS.lock();
    if g.is_none() {
        let t = crate::prefs_client::sys_text(crate::prefs::key::QUARRY_FAVORITES).unwrap_or_default();
        *g = Some(t.split(',').map(|s| s.trim()).filter(|s| s.starts_with('/')).map(String::from).collect());
    }
    g.clone().unwrap_or_default()
}

/// Add a folder to the sidebar's Favorites; store it. `false` = already there, or the list would not fit.
fn add_favorite(path: &str) -> bool {
    let mut v = user_favorites();
    if v.iter().any(|f| f.eq_ignore_ascii_case(path)) || sidebar::favorites().iter().any(|(_, f)| f.eq_ignore_ascii_case(path)) {
        return false;
    }
    v.push(String::from(path));
    let joined = v.join(",");
    if joined.len() > FAVS_MAX {
        return false;
    }
    *FAVS.lock() = Some(v);
    crate::prefs_client::sys_set(crate::prefs::key::QUARRY_FAVORITES, crate::prefs::PrefValue::Str(joined));
    true
}

/// The schema row's length bound (`prefs_core::schema`, `system.quarry.favorites`).
const FAVS_MAX: usize = 512;

// ── the hover highlight ─────────────────────────────────────────────────────────────────────────

/// The session's hover moved: repaint Quarry when it is open (its painter reads [`dnd::hover`]).
pub fn hover_changed() {
    if super::is_open() {
        super::repaint();
    }
}

fn outline(px: &mut [u32], m: &Model, r: Rect, c: u32) {
    let g = &m.geom;
    if r.w < 4 || r.h < 4 {
        return;
    }
    fill(px, g, r.x, r.y, r.w, 2, c);
    fill(px, g, r.x, r.y + r.h - 2, r.w, 2, c);
    fill(px, g, r.x, r.y, 2, r.h, c);
    fill(px, g, r.x + r.w - 2, r.y, 2, r.h, c);
}

/// Paint the hover outline over the finished surface (from `q3_paint`).
pub(super) fn paint(m: &Model, px: &mut [u32]) {
    let Some(t) = dnd::hover() else { return };
    let g = m.geom;
    let row_h = g.row_h();
    let c = theme::accent();
    let sr = sidebar::rect(&g);
    match t {
        Target::Folder { win, dir } if win == WIN.load(Ordering::Relaxed) => {
            let li = g.list_pane().inner();
            if dir == m.cwd {
                outline(px, m, li, c);
                return;
            }
            if let Some(i) = m.list.iter().position(|e| join(&m.cwd, &e.name) == dir) {
                if toolbar::view() == toolbar::View::Icons {
                    if let Some((cx, cy)) = super::iconview::centre_of(&g, li, m.list.len(), m.list_scroll, i) {
                        let (cw, ch) = super::iconview::cell(&g);
                        outline(px, m, Rect { x: cx - cw / 2, y: cy - ch / 2, w: cw, h: ch }, c);
                    }
                } else if i >= m.list_scroll && i - m.list_scroll < m.list_visible() {
                    outline(px, m, Rect { x: li.x, y: li.y + row_h + (i - m.list_scroll) * row_h, w: li.w, h: row_h }, c);
                }
                return;
            }
            let ti = g.tree_pane().inner();
            if let Some(i) = m.tree.iter().position(|r| r.path == dir) {
                if i >= m.tree_scroll && i - m.tree_scroll < m.tree_visible() {
                    outline(px, m, Rect { x: ti.x, y: ti.y + (i - m.tree_scroll) * row_h, w: ti.w, h: row_h }, c);
                }
            }
        }
        Target::Favorites => {
            let rows = sidebar::rows();
            let n = rows.iter().take_while(|e| !(e.path.is_empty() && e.label != "Favorites")).count();
            outline(px, m, Rect { x: sr.x, y: sr.y + g.ts, w: sr.w, h: (n * row_h).min(sr.h.saturating_sub(g.ts)) }, c);
        }
        Target::Volume { dir } => {
            if let Some(i) = sidebar::rows().iter().position(|e| e.path == dir) {
                let y = sr.y + g.ts + i * row_h;
                if y + row_h <= sr.y + sr.h {
                    outline(px, m, Rect { x: sr.x, y, w: sr.w, h: row_h }, c);
                }
            }
        }
        _ => {}
    }
}

// ── `tests dragdrop` ────────────────────────────────────────────────────────────────────────────

static FIX_T: spin::Mutex<Option<Target>> = spin::Mutex::new(None);

/// The fixture's resolver: right of x=100 the fixture target, left of it nothing.
fn fix_resolve(x: i32, _y: i32, _p: &Payload) -> Option<Target> {
    if x >= 100 { FIX_T.lock().clone() } else { None }
}

fn exists(p: &str) -> bool {
    crate::shell::vfs_mount_table().stat(p).is_ok()
}

fn fix_payload(path: &str) -> Payload {
    Payload { kind: Kind::File, paths: alloc::vec![String::from(path)], dir: false, from: wm::WIN_NONE, label: leaf(path) }
}

/// One fixture drag: arm at (10,10), a sub-threshold nudge, travel to (150,10) over `t`, then `end` (release or Esc).
fn drag(path: &str, t: Target, esc: bool) -> (bool, Option<(Action, bool)>) {
    *FIX_T.lock() = Some(t.clone());
    dnd::arm_with(fix_payload(path), 10, 10, fix_resolve, false);
    crate::video::capture::motion(11, 10);
    let quiet = !dnd::active();
    crate::video::capture::motion(150, 10);
    let live = dnd::active() && dnd::hover() == Some(t) && crate::video::capture::held();
    if esc {
        let took = dnd::key(crate::pal::Event::Key(0x1b));
        crate::video::capture::release(150, 10);
        return (quiet && live && took && !dnd::armed() && !crate::video::capture::held(), dnd::take_last());
    }
    crate::video::capture::release(150, 10);
    (quiet && live && !dnd::armed() && dnd::hover().is_none(), dnd::take_last())
}

/// `tests dragdrop` registration (rides `quarry3_tests`).
pub fn tests() {
    crate::tests::register("dragdrop", selftest);
}

/// `tests dragdrop` — the session over the model; the real ops on `<home>/DragDropTest`.
pub fn selftest() {
    let mt = crate::shell::vfs_mount_table();
    let base = alloc::format!("{}/DragDropTest", crate::fs::trash::home_base());
    let (a, b) = (alloc::format!("{}/A", base), alloc::format!("{}/B", base));
    let _ = super::ops::op_delete(&base);
    let made = super::ops::op_mkdir(&base).is_ok() && super::ops::op_mkdir(&a).is_ok() && super::ops::op_mkdir(&b).is_ok();
    let f0 = alloc::format!("{}/dnd.txt", base);
    let wrote = made && mt.create(&f0, NodeKind::File, crate::fs::vfs::KERNEL_PRINCIPAL).is_ok() && mt.write(&f0, 0, b"dragdrop\n", crate::fs::vfs::KERNEL_PRINCIPAL).is_ok();
    if !wrote {
        serial_println!(":: DRAGDROP: fixture={} -> SKIP :: reason=no-home-folder ::", base);
        return;
    }
    let (fa, fb) = (alloc::format!("{}/dnd.txt", a), alloc::format!("{}/dnd.txt", b));
    // session + move: base/dnd.txt onto folder A (same volume, no Option) = a move.
    let (s1, l1) = drag(&f0, Target::Folder { win: wm::WIN_NONE, dir: a.clone() }, false);
    let move_ok = l1 == Some((Action::Move, true)) && exists(&fa) && !exists(&f0);
    // copy: Option forces a copy of A/dnd.txt into B; the source stays.
    dnd::force_option(true);
    let (s2, l2) = drag(&fa, Target::Folder { win: wm::WIN_NONE, dir: b.clone() }, false);
    dnd::force_option(false);
    let copy_ok = l2 == Some((Action::Copy, true)) && exists(&fa) && exists(&fb);
    // cancel: A/dnd.txt over the base folder, Esc — nothing moves, the capture is released.
    let (s3, l3) = drag(&fa, Target::Folder { win: wm::WIN_NONE, dir: base.clone() }, true);
    let cancel_ok = s3 && l3.is_none() && exists(&fa) && !exists(&f0);
    // trash: B/dnd.txt onto the Trash tile — DOCK2's path; restored afterwards.
    *dnd::LAST_TRASHED.lock() = None;
    let (s4, l4) = drag(&fb, Target::Trash, false);
    let trashed = !exists(&fb);
    let name = dnd::LAST_TRASHED.lock().take();
    let restored = name.as_deref().map(|n| crate::fs::trash::restore(n).is_ok()).unwrap_or(false);
    let trash_ok = l4 == Some((Action::Trash, true)) && trashed && restored && exists(&fb);
    // drop_ok rules: a file never becomes a favorite; a folder never drops into itself.
    let fold = Payload { kind: Kind::File, paths: alloc::vec![a.clone()], dir: true, from: wm::WIN_NONE, label: String::from("A") };
    let rules_ok = !folder_ok(&fold, &a) && !folder_ok(&fold, &alloc::format!("{}/x", a)) && !folder_ok(&fold, &base) && folder_ok(&fold, &b);
    let session_ok = s1 && s2 && s4 && rules_ok;
    let _ = super::ops::op_delete(&base);
    let w = |b: bool| if b { "ok" } else { "FAIL" };
    let pass = session_ok && move_ok && copy_ok && trash_ok && cancel_ok;
    serial_println!(
        ":: DRAGDROP: session={} move={} copy={} trash={} cancel={} -> {} :: fixture={} threshold={} ::",
        w(session_ok),
        w(move_ok),
        w(copy_ok),
        w(trash_ok),
        w(cancel_ok),
        if pass { "PASS" } else { "FAIL" },
        base,
        dnd::threshold()
    );
}
