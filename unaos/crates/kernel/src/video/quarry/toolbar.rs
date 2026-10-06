// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Matrix — kernel-by-ruling R50
//!
//! QUARRY3 (rmbp-ledger B413, MACPARITY row 27) — Quarry's TOOLBAR and PATH BAR. Quarry is the Finder by ruling (R50).
//!
//! * **Back / Forward** — a [`History`] of the places the list pane showed (every `show` that changed the
//!   directory; a travel through the history does not record itself). `<` / `>` keys too.
//! * **The view switcher** — List | Icons (`v` toggles). The icon view is `iconview.rs`.
//! * **Search** — a field at the right of the toolbar (a press, or `/`, focuses it); every keystroke re-runs a
//!   NAME match through UnaFS's name tree (`UnaFS::find_names`, the shared core both rings link — LAUNCHER row 36
//!   reuses it), else a bounded walk of the namespace on a FAT root. The hits replace the list (their paths as
//!   names, under `/`), Return keeps them, Esc (or emptying the field) puts the directory back.
//! * **The path bar** — under the toolbar, the current place as segments `/ > volumes > UnaOS`; a press on a
//!   segment goes there. The last activation's result (the only feedback a launch has) still rides it.
//!
//! Window state, not a preference (no store, R79). Lock order: `MODEL` then `TB`.
//! Witness: `[quarry3] search q=<q> hits=<n> src=<unafs-names|walk> dirs=<n> ms=<n>` per keystroke.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use super::{fill, text, theme, Act, DirEnt, Geom, Model, NodeKind, Pane, Rect, MODEL, PAD};

/// History depth (each way).
const HIST_MAX: usize = 32;
/// Characters the search field takes.
const QUERY_MAX: usize = 48;
/// Hits a search shows, and directories one keystroke may read.
const HITS_MAX: usize = 256;
const DIRS_MAX: usize = 512;

/// The list pane's view.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum View {
    List,
    Icons,
}

/// Back/forward over visited places. Pure: the fixture drives its own instance.
#[derive(Default)]
pub(super) struct History {
    back: Vec<String>,
    fwd: Vec<String>,
    cur: String,
}

impl History {
    /// The list pane now shows `path`: a NEW place pushes the old one on Back and clears Forward.
    pub fn visit(&mut self, path: &str) {
        if self.cur == path {
            return;
        }
        if !self.cur.is_empty() {
            if self.back.len() >= HIST_MAX {
                self.back.remove(0);
            }
            self.back.push(core::mem::take(&mut self.cur));
        }
        self.fwd.clear();
        self.cur = String::from(path);
    }
    /// Step back: the place to show, the current one moving to Forward.
    pub fn back(&mut self) -> Option<String> {
        let p = self.back.pop()?;
        self.fwd.push(core::mem::replace(&mut self.cur, p.clone()));
        Some(p)
    }
    pub fn forward(&mut self) -> Option<String> {
        let p = self.fwd.pop()?;
        self.back.push(core::mem::replace(&mut self.cur, p.clone()));
        Some(p)
    }
    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }
    pub fn can_forward(&self) -> bool {
        !self.fwd.is_empty()
    }
}

struct Search {
    focused: bool,
    active: bool,
    q: String,
    /// The directory the list showed before the search took it.
    saved: String,
    hits: usize,
    src: &'static str,
}

struct Tb {
    hist: History,
    view: View,
    search: Search,
}

static TB: crate::sync::Mutex<Tb> = crate::sync::Mutex::new(Tb {
    hist: History { back: Vec::new(), fwd: Vec::new(), cur: String::new() },
    view: View::List,
    search: Search { focused: false, active: false, q: String::new(), saved: String::new(), hits: 0, src: "-" },
});
/// A Back/Forward/segment travel is in flight: its `show` must not record itself.
static TRAVEL: AtomicBool = AtomicBool::new(false);
/// The search's own `show`s (restoring the directory) must not end the search they belong to.
static SEARCHING: AtomicBool = AtomicBool::new(false);

pub(super) fn view() -> View {
    TB.lock().view
}

// ── Layout (pure: the painter and the router read the same numbers) ─────────────────────────────

/// The toolbar's controls, in surface px.
pub(super) struct Layout {
    pub back: Rect,
    pub fwd: Rect,
    pub list: Rect,
    pub icons: Rect,
    pub search: Rect,
    /// The path bar row.
    pub path: Rect,
}

pub(super) fn layout(g: &Geom) -> Layout {
    let tb_h = g.bar_h() / 2;
    let bs = g.row_h();
    let y = (tb_h - bs) / 2;
    let back = Rect { x: PAD(), y, w: bs, h: bs };
    let fwd = Rect { x: back.x + bs + g.ts, y, w: bs, h: bs };
    let seg = 6 * g.cell_w();
    let list = Rect { x: fwd.x + bs + 4 * PAD(), y, w: seg, h: bs };
    let icons = Rect { x: list.x + seg, y, w: seg, h: bs };
    let sw = (g.w / 4).max(16 * g.cell_w()).min(g.w / 3);
    let search = Rect { x: g.w.saturating_sub(PAD() + sw), y, w: sw, h: bs };
    Layout { back, fwd, list, icons, search, path: Rect { x: 0, y: tb_h, w: g.w, h: g.bar_h() - tb_h } }
}

/// The path bar's segments: `(x0, x1, path)` for `cwd`, drawn from `x` at advance `cw`, `" > "` between.
pub(super) fn segments(cwd: &str, x: usize, cw: usize) -> Vec<(usize, usize, String, String)> {
    let mut out: Vec<(usize, usize, String, String)> = Vec::new();
    let mut px = x;
    out.push((px, px + cw, String::from("/"), String::from("/")));
    px += cw + 3 * cw;
    let mut acc = String::new();
    for part in cwd.split('/').filter(|s| !s.is_empty()) {
        acc.push('/');
        acc.push_str(part);
        let w = part.len() * cw;
        out.push((px, px + w, String::from(part), acc.clone()));
        px += w + 3 * cw;
    }
    out
}

// ── The listing hook ────────────────────────────────────────────────────────────────────────────

/// After every `show` (live.rs's listing pass): record the place, end a search the person navigated out of,
/// and let the sidebar re-derive its rows when the volumes moved.
pub(super) fn after_show(m: &mut Model) {
    super::sidebar::refresh(false);
    let mut t = TB.lock();
    if !SEARCHING.load(Ordering::Relaxed) && t.search.active {
        t.search.active = false;
        t.search.focused = false;
        t.search.q.clear();
    }
    if TRAVEL.load(Ordering::Relaxed) {
        t.hist.cur = m.cwd.clone();
    } else if !t.search.active {
        t.hist.visit(&m.cwd);
    }
}

fn travel(m: &mut Model, to: Option<String>) -> bool {
    let Some(p) = to else { return false };
    TRAVEL.store(true, Ordering::Relaxed);
    m.navigate(&p);
    TRAVEL.store(false, Ordering::Relaxed);
    m.focus = Pane::List;
    m.status = None;
    serial_println!("[quarry3] travel -> {}", p);
    true
}

fn go_back(m: &mut Model) -> bool {
    let to = TB.lock().hist.back();
    travel(m, to)
}

fn go_forward(m: &mut Model) -> bool {
    let to = TB.lock().hist.forward();
    travel(m, to)
}

fn set_view(v: View) {
    let mut t = TB.lock();
    if t.view != v {
        t.view = v;
        serial_println!("[quarry3] view={}", if v == View::Icons { "icons" } else { "list" });
        drop(t); super::folderview::set_mode(if v == View::Icons { "icons" } else { "list" }); // FOLDERVIEW (B424): the folder's mode, latched
    }
}

// ── Search ──────────────────────────────────────────────────────────────────────────────────────

/// Every name containing `q`: `(absolute path, is_dir)`, the source, and the directories read.
pub(super) fn search_names(q: &str, max: usize) -> (Vec<(String, bool)>, &'static str, usize) {
    #[cfg(any(target_arch = "aarch64", feature = "unafs"))]
    {
        let mt = crate::shell::vfs_mount_table();
        if mt.volume_name("/").map(|n| n == "native").unwrap_or(false) {
            if let Ok(Ok((hits, dirs))) = crate::fs::unafs::with_unafs(|fs| fs.find_names(q, max, DIRS_MAX)) {
                return (hits, "unafs-names", dirs);
            }
        }
    }
    // A FAT root: a bounded breadth-first walk of the namespace through the one listing seam.
    let ql = q.to_ascii_lowercase();
    let mut hits: Vec<(String, bool)> = Vec::new();
    let mut queue: alloc::collections::VecDeque<String> = alloc::collections::VecDeque::new();
    queue.push_back(String::from("/"));
    let mut dirs = 0usize;
    while let Some(d) = queue.pop_front() {
        if dirs >= DIRS_MAX || hits.len() >= max || q.is_empty() {
            break;
        }
        dirs += 1;
        let Ok((true, rows)) = super::collect(&d) else { continue };
        for e in rows.iter() {
            let p = super::join(&d, &e.name);
            let is_dir = matches!(e.kind, NodeKind::Dir);
            if e.name.to_ascii_lowercase().contains(ql.as_str()) {
                hits.push((p.clone(), is_dir));
                if hits.len() >= max {
                    break;
                }
            }
            if is_dir && !(d == "/" && e.name == "volumes") {
                queue.push_back(p);
            }
        }
    }
    (hits, "walk", dirs)
}

/// Re-run the search for the field's text (a keystroke), or put the directory back when it is empty.
fn run_search(m: &mut Model) {
    let (q, saved, was_active) = {
        let t = TB.lock();
        (t.search.q.clone(), t.search.saved.clone(), t.search.active)
    };
    if q.is_empty() {
        if was_active {
            end_search(m);
        }
        return;
    }
    if !was_active {
        let mut t = TB.lock();
        t.search.saved = m.cwd.clone();
        t.search.active = true;
    }
    let _ = saved;
    let t0 = crate::arch::ms();
    let (hits, src, dirs) = search_names(&q, HITS_MAX);
    let n = hits.len();
    m.list = hits
        .into_iter()
        .map(|(p, d)| DirEnt { name: String::from(p.trim_start_matches('/')), kind: if d { NodeKind::Dir } else { NodeKind::File }, size: 0, mtime: None })
        .collect();
    m.cwd = String::from("/");
    m.list_sel = 0;
    m.list_scroll = 0;
    m.list_truncated = false;
    m.err = None;
    super::columns::after_show(m);
    {
        let mut t = TB.lock();
        t.search.hits = n;
        t.search.src = src;
    }
    serial_println!("[quarry3] search q={} hits={} src={} dirs={} ms={}", q, n, src, dirs, crate::arch::ms().saturating_sub(t0));
}

fn end_search(m: &mut Model) {
    let saved = {
        let mut t = TB.lock();
        t.search.active = false;
        t.search.focused = false;
        t.search.q.clear();
        core::mem::take(&mut t.search.saved)
    };
    SEARCHING.store(true, Ordering::Relaxed);
    TRAVEL.store(true, Ordering::Relaxed);
    m.show(if saved.is_empty() { "/" } else { saved.as_str() });
    TRAVEL.store(false, Ordering::Relaxed);
    SEARCHING.store(false, Ordering::Relaxed);
}

// ── Input ───────────────────────────────────────────────────────────────────────────────────────

/// A key while the SEARCH FIELD holds the keyboard. `true` = consumed.
pub(super) fn key_search(c: u8) -> bool {
    if !TB.lock().search.focused {
        return false;
    }
    let mut guard = MODEL.lock();
    let Some(m) = guard.as_mut() else { return false };
    match c {
        0x1b => {
            if TB.lock().search.active {
                end_search(m);
            } else {
                TB.lock().search.focused = false;
            }
        }
        b'\r' | b'\n' => {
            TB.lock().search.focused = false;
            m.focus = Pane::List;
        }
        0x08 | 0x7f => {
            TB.lock().search.q.pop();
            SEARCHING.store(true, Ordering::Relaxed);
            run_search(m);
            SEARCHING.store(false, Ordering::Relaxed);
        }
        0x20..=0x7e => {
            {
                let mut t = TB.lock();
                if t.search.q.len() >= QUERY_MAX {
                    return true;
                }
                t.search.q.push(c as char);
            }
            SEARCHING.store(true, Ordering::Relaxed);
            run_search(m);
            SEARCHING.store(false, Ordering::Relaxed);
        }
        _ => return false, // arrows fall through to the list
    }
    m.settle();
    true
}

/// Toolbar keys with the list/tree focused: `<` back, `>` forward, `v` view, `/` search. `true` = consumed.
pub(super) fn key(c: u8) -> bool {
    match c {
        b'<' | b'>' => {
            let mut guard = MODEL.lock();
            let Some(m) = guard.as_mut() else { return false };
            let _ = if c == b'<' { go_back(m) } else { go_forward(m) };
            m.settle();
            true
        }
        b'v' | b'V' => {
            let v = if view() == View::List { View::Icons } else { View::List };
            set_view(v);
            true
        }
        b'/' => {
            TB.lock().search.focused = true;
            true
        }
        _ => false,
    }
}

/// A press on the toolbar or the path bar. `None` = not ours.
pub(super) fn press(m: &mut Model, sx: usize, sy: usize) -> Option<Act> {
    let g = m.geom;
    if sy >= g.bar_h() {
        // A press anywhere else takes the keyboard away from the field (its hits stay).
        TB.lock().search.focused = false;
        return None;
    }
    let l = layout(&g);
    if l.back.contains(sx, sy) {
        go_back(m);
    } else if l.fwd.contains(sx, sy) {
        go_forward(m);
    } else if l.list.contains(sx, sy) {
        set_view(View::List);
    } else if l.icons.contains(sx, sy) {
        set_view(View::Icons);
    } else if l.search.contains(sx, sy) {
        TB.lock().search.focused = true;
    } else if l.path.contains(sx, sy) && !TB.lock().search.active {
        let hit = segments(&m.cwd, PAD(), g.cell_w()).into_iter().find(|s| sx >= s.0 && sx < s.1);
        if let Some((_, _, _, p)) = hit {
            if p != m.cwd {
                m.navigate(&p);
                m.focus = Pane::List;
                serial_println!("[quarry3] pathbar -> {}", p);
            }
        }
    }
    m.settle();
    Some(Act::None)
}

// ── Paint ───────────────────────────────────────────────────────────────────────────────────────

fn button(px: &mut [u32], g: &Geom, r: Rect, label: &[u8], on: bool, live: bool) {
    fill(px, g, r.x, r.y, r.w, r.h, if on { theme::accent() } else { theme::button_face() });
    super::keyline(px, g, r, theme::frame_line());
    let ink = if on {
        theme::chrome_face()
    } else if live {
        theme::button_text()
    } else {
        theme::title_text_inactive()
    };
    let tw = label.len() * g.cell_w();
    let x = r.x + r.w.saturating_sub(tw) / 2;
    text(px, g, x, r.y + (r.h.saturating_sub(g.cell_h())) / 2, label, r.x + r.w, ink);
}

/// Paint the toolbar row and the path bar over the bar region (after live.rs drew it plain).
pub(super) fn paint(m: &Model, px: &mut [u32]) {
    let g = &m.geom;
    let l = layout(g);
    let t = TB.lock();
    fill(px, g, 0, 0, g.w, g.bar_h(), theme::chrome_face());
    fill(px, g, 0, l.path.y.saturating_sub(1), g.w, 1, theme::frame_line());
    fill(px, g, 0, g.bar_h() - 1, g.w, 1, theme::frame_line());
    button(px, g, l.back, b"<", false, t.hist.can_back());
    button(px, g, l.fwd, b">", false, t.hist.can_forward());
    button(px, g, l.list, b"List", t.view == View::List, true);
    button(px, g, l.icons, b"Icons", t.view == View::Icons, true);
    // The search field.
    let s = l.search;
    fill(px, g, s.x, s.y, s.w, s.h, theme::content_fill());
    super::keyline(px, g, s, if t.search.focused { theme::accent() } else { theme::frame_line() });
    let ty = s.y + s.h.saturating_sub(g.cell_h()) / 2;
    if t.search.q.is_empty() && !t.search.focused {
        text(px, g, s.x + PAD(), ty, b"Search", s.x + s.w - PAD(), theme::title_text_inactive());
    } else {
        let mut q: Vec<u8> = Vec::from(t.search.q.as_bytes());
        if t.search.focused {
            q.push(b'_');
        }
        // Keep the tail in view when the query is longer than the field.
        let fit = (s.w.saturating_sub(2 * PAD()) / g.cell_w().max(1)).max(1);
        let from = q.len().saturating_sub(fit);
        text(px, g, s.x + PAD(), ty, &q[from..], s.x + s.w - PAD(), theme::content_text());
    }
    // The path bar.
    let py = l.path.y + l.path.h.saturating_sub(g.cell_h()) / 2;
    if t.search.active {
        let line = alloc::format!("Search \"{}\": {} found ({})", t.search.q, t.search.hits, t.search.src);
        text(px, g, PAD(), py, line.as_bytes(), g.w - PAD(), theme::title_text_active());
        return;
    }
    let segs = segments(&m.cwd, PAD(), g.cell_w());
    let mut end = PAD();
    for (i, (x0, x1, label, _)) in segs.iter().enumerate() {
        if *x1 > g.w - PAD() {
            break;
        }
        if i > 0 {
            text(px, g, x0 - 3 * g.cell_w(), py, b" > ", *x0, theme::title_text_inactive());
        }
        let last = i + 1 == segs.len();
        text(px, g, *x0, py, label.as_bytes(), *x1 + g.cell_w(), if last { theme::title_text_active() } else { theme::accent() });
        end = *x1;
    }
    let mut tail: Vec<u8> = Vec::new();
    if m.list_truncated {
        tail.extend_from_slice(b"  (list truncated)");
    }
    if let Some(st) = &m.status {
        tail.extend_from_slice(b"  -  ");
        tail.extend_from_slice(st.as_bytes());
    }
    if !tail.is_empty() {
        text(px, g, end, py, &tail, g.w - PAD(), theme::title_text_active());
    }
}

/// FOLDERVIEW (B424): the folder just entered carries `mode` — show it, without latching a change.
pub(super) fn apply_mode(mode: &str) {
    TB.lock().view = if mode == "icons" { View::Icons } else { View::List };
}

/// FOLDERVIEW (B424): a search's results are not a folder — no view is resolved or saved for them.
pub(super) fn search_active() -> bool {
    TB.lock().search.active
}
