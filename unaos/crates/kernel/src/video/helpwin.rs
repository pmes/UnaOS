//! CHARTER: Kernel — wm (HELPSEARCH B729: the Help window, a kernel app on the glass; no CODEX handler owns help)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! HELPSEARCH (rmbp-ledger B729; BP-bar-2/3, research C §8) — **the Help window** and the third search source.
//! The module `help::window` (declared from `help.rs` by `#[path]`; it lives in `video/` with the painters, so the
//! appearance gate scans it), x86 `wc` only (its doors ride the Launcher's, which are x86's): no `lib.rs` or `video/mod.rs` line.
//!
//! * THE WINDOW — a native kernel window (`wm::create_at_native`, the FILEVIEW shape, its own owner slot) titled with
//!   the page's title: the title, the body word-wrapped at the window's columns, then a "Settings for this page"
//!   group with one `Open the setting: <row label>` link per key the page names (the label is `settings::MAP`'s). A
//!   verb opens here too: its `help::render` text, no links.
//! * THE DOORS (R88: edit and latch only) — [`key_route`] while this window holds focus: Up/Down scroll a row, Space /
//!   `b` a page, Tab moves to the next link, Return follows it, Escape closes; the wheel scrolls. [`press_route`]: the
//!   close box closes, a press on a link follows it, any press in the content focuses the window. Both are chained
//!   from the Launcher's doors (`video::launcher::key_door` / `press_at`, the router's first door and arm) when the
//!   Launcher is not up. Every paint, open, close and link runs in [`service`] (chained from `launcher::service`, the
//!   desktop pass).
//! * A LINK is `settings::request_open_at(key)` — Settings opens on that row (SETTINGSMAP's one table, BP-video-16).
//! * THE SETTINGS SOURCE — [`search_rows`] ranks `settings::MAP` by its painted labels for the Launcher's Settings group
//!   and the fixture: the whole query in the label, else every query word starting a label word.
//! * F1 — [`request_for_focus`] latches the focused program's page (`app:` in the corpus: Settings, Quarry, the shell,
//!   else the desktop's). The fn-row router (`video/fnrow.rs`) is outside this arc; its call is owed (see B729).
//!
//! Wire: `[help] open page=<slug> win=<id> rows=<n> links=<n>`, `[help] link key=system.<k> tab=<t> control=<c>`,
//! `[help] closed win=<id>`, `[help] f1 owner=<asid> app=<a> page=<slug>`.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::video::{settings, theme, wm};

/// Kernel-furniture owner slot (`+ 0x29`: no other kernel row uses it).
pub const OWNER: u64 = wm::KERNEL_OWNER_BASE + 0x29;
const _: () = assert!(OWNER != settings::OWNER && OWNER != crate::video::fileview::OWNER && OWNER != wm::KERNEL_OWNER_CONSOLE && OWNER != wm::KERNEL_OWNER_DESKTOP);

#[allow(non_snake_case)]
#[inline]
fn WIN_W() -> usize {
    crate::ui::px(640)
}
#[allow(non_snake_case)]
#[inline]
fn WIN_H() -> usize {
    crate::ui::px(460)
}
#[allow(non_snake_case)]
#[inline]
fn PAD() -> usize {
    crate::ui::px(14)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Title,
    Body,
    Head,
    Link(usize),
}

struct Row {
    text: String,
    kind: Kind,
}

struct State {
    slug: String,
    rows: Vec<Row>,
    /// The links' Principia keys (`system.` stripped), in row order.
    links: Vec<String>,
    top: usize,
    vis: usize,
    sel: Option<usize>,
    w: usize,
    h: usize,
    surf: Vec<u32>,
}

static WIN: AtomicU32 = AtomicU32::new(wm::WIN_NONE);
static STATE: crate::sync::Mutex<Option<State>> = crate::sync::Mutex::new(None);
/// Latched by the doors, drained by [`service`].
static OPEN_OWED: crate::sync::Mutex<Option<String>> = crate::sync::Mutex::new(None);
static OPEN_POSTED: AtomicBool = AtomicBool::new(false);
static PAINT_OWED: AtomicBool = AtomicBool::new(false);
static CLOSE_OWED: AtomicBool = AtomicBool::new(false);
static LINK_OWED: AtomicU32 = AtomicU32::new(u32::MAX);
static F1_OWED: AtomicBool = AtomicBool::new(false);

/// Is the Help window up (its id names a live row of ours)?
pub fn is_open() -> bool {
    let id = WIN.load(Ordering::Acquire);
    id != wm::WIN_NONE && wm::owner_of(id) == Some(OWNER)
}

/// The page on the glass (empty = none).
pub fn shown() -> String {
    STATE.lock().as_ref().map(|s| s.slug.clone()).unwrap_or_default()
}

/// The link count of the page on the glass.
pub fn links_shown() -> usize {
    STATE.lock().as_ref().map(|s| s.links.len()).unwrap_or(0)
}

// ── The settings source ─────────────────────────────────────────────────────────────────────────

/// `settings::MAP` rows for `q`, best first: `(row, score)` — the whole query at the label's start (100), at a word's
/// start (80), inside it (60); else every query word starting a word of the label (40). One row per first key. Pure.
pub fn search_rows(q: &str) -> Vec<(usize, i32)> {
    let q = q.trim();
    let qw = super::index::words_of(q);
    let mut out: Vec<(usize, i32)> = Vec::new();
    if q.is_empty() {
        return out;
    }
    for (i, r) in settings::MAP.iter().enumerate() {
        if r.tab >= settings::TABS {
            continue;
        }
        let l = r.label;
        let lb = l.as_bytes();
        let at = |k: usize| lb.len() >= k + q.len() && lb[k..k + q.len()].eq_ignore_ascii_case(q.as_bytes());
        let s = if at(0) {
            100
        } else if (1..lb.len()).any(|k| !lb[k - 1].is_ascii_alphanumeric() && at(k)) {
            80
        } else if super::index::holds_ci(l, q) {
            60
        } else if !qw.is_empty() && qw.iter().all(|w| super::index::words_of(l).iter().any(|lw| lw.starts_with(w.as_str()))) {
            40
        } else {
            continue;
        };
        let key = r.keys.first().copied().unwrap_or("");
        if !out.iter().any(|(j, _)| settings::MAP[*j].keys.first().copied() == Some(key)) {
            out.push((i, s));
        }
    }
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// The painted label of the Settings row that edits `key` (`None` = no row names it).
pub fn row_label(key: &str) -> Option<&'static str> {
    settings::MAP.iter().find(|r| r.tab < settings::TABS && r.keys.contains(&key)).map(|r| r.label)
}

// ── Layout and paint ────────────────────────────────────────────────────────────────────────────

/// Word-wrap `line` at `cols` (a `- ` item's continuation indents two). Pure.
fn wrap(line: &str, cols: usize, kind: Kind, out: &mut Vec<Row>) {
    let cols = cols.max(8);
    let indent = if line.starts_with("- ") { 2 } else if line.len() > 2 && line.as_bytes()[0].is_ascii_digit() && &line[1..3] == ". " { 3 } else { 0 };
    let mut cur = String::new();
    for w in line.split(' ') {
        let lead = if cur.is_empty() { 0 } else { 1 };
        if !cur.is_empty() && cur.len() + lead + w.len() > cols {
            out.push(Row { text: core::mem::take(&mut cur), kind });
            for _ in 0..indent {
                cur.push(' ');
            }
        } else if lead == 1 {
            cur.push(' ');
        }
        let mut w = w;
        while w.len() > cols.saturating_sub(cur.len()) && cur.trim().is_empty() {
            let cut = cols.saturating_sub(cur.len()).max(1).min(w.len());
            cur.push_str(&w[..cut]);
            out.push(Row { text: core::mem::take(&mut cur), kind });
            w = &w[cut..];
        }
        cur.push_str(w);
    }
    out.push(Row { text: cur, kind });
}

/// The rows of a page: the title, a blank, the body wrapped, then the links' group. Pure.
fn rows_of(title: &str, body: &str, keys: &[String], cols: usize) -> (Vec<Row>, Vec<String>) {
    let mut rows = Vec::new();
    wrap(title, cols, Kind::Title, &mut rows);
    rows.push(Row { text: String::new(), kind: Kind::Body });
    for l in body.lines() {
        if l.is_empty() {
            rows.push(Row { text: String::new(), kind: Kind::Body });
        } else {
            wrap(l, cols, Kind::Body, &mut rows);
        }
    }
    let mut links = Vec::new();
    if !keys.is_empty() {
        rows.push(Row { text: String::new(), kind: Kind::Body });
        rows.push(Row { text: String::from("Settings for this page"), kind: Kind::Head });
        for k in keys {
            let label = row_label(k).unwrap_or(k.as_str());
            rows.push(Row { text: alloc::format!("> Open the setting: {}", label), kind: Kind::Link(links.len()) });
            links.push(k.clone());
        }
    }
    (rows, links)
}

fn face() -> crate::video::text::Face {
    crate::video::text::Face::Grid
}

fn paint(st: &mut State) {
    let (w, h) = (st.w, st.h);
    let ch = face().cell_h();
    for p in st.surf.iter_mut() {
        *p = theme::surface_content();
    }
    let (pad, sb) = (PAD(), crate::ui::px(6));
    for r in 0..st.vis {
        let Some(row) = st.rows.get(st.top + r) else { break };
        let y = pad + r * ch;
        let (ink, bold) = match row.kind {
            Kind::Title => (theme::text_primary(), true),
            Kind::Body => (theme::text_primary(), false),
            Kind::Head => (theme::text_secondary(), true),
            Kind::Link(i) if st.sel == Some(i) => {
                for yy in y..(y + ch).min(h) {
                    for xx in pad / 2..w.saturating_sub(sb + pad / 2) {
                        st.surf[yy * w + xx] = theme::accent();
                    }
                }
                (theme::surface_raised(), true)
            }
            Kind::Link(_) => (theme::accent(), false),
        };
        crate::video::text::draw_text(&mut st.surf, w, w - sb, h, pad, y, row.text.as_bytes(), ink, bold, face());
    }
    // The scroll thumb (proportional; full height when everything fits).
    let total = st.rows.len().max(1);
    let th = if total <= st.vis { h } else { (h * st.vis / total).max(crate::ui::px(8)) };
    let ty = if total <= st.vis { 0 } else { (h - th) * st.top / (total - st.vis) };
    for y in 0..h {
        let c = if y >= ty && y < ty + th { theme::line_strong() } else { theme::surface_sunken() };
        for x in w - crate::ui::px(5)..w - crate::ui::px(1) {
            st.surf[y * w + x] = c;
        }
    }
}

fn repaint() {
    let id = WIN.load(Ordering::Acquire);
    if !is_open() {
        return;
    }
    {
        let mut g = STATE.lock();
        let Some(st) = g.as_mut() else { return };
        paint(st);
    }
    let _ = wm::present(id);
}

// ── Open / close (the service pass) ─────────────────────────────────────────────────────────────

/// Open a document in the Help window (replacing the one shown). `slug` names it on the wire.
fn open_doc(slug: &str, title: &str, body: &str, keys: &[String]) -> Result<usize, String> {
    let pi = crate::video::panel_info_nonblocking().ok_or_else(|| String::from("panel busy"))?;
    let (pw, ph) = (pi.width, pi.height);
    let w = WIN_W().min(pw.saturating_sub(2 * wm::BORDER()).max(1));
    let h = WIN_H().min(ph.saturating_sub(wm::TITLE_H() + 2 * wm::BORDER()).max(1));
    let (cw, ch) = (face().cell_w().max(1), face().cell_h().max(1));
    let cols = w.saturating_sub(2 * PAD() + crate::ui::px(6)) / cw;
    let vis = h.saturating_sub(2 * PAD()) / ch;
    if cols < 16 || vis < 4 {
        return Err(String::from("window below floor"));
    }
    let (rows, links) = rows_of(title, body, keys, cols);
    let mut surf: Vec<u32> = Vec::new();
    if surf.try_reserve_exact(w * h).is_err() {
        return Err(String::from("out of memory"));
    }
    surf.resize(w * h, theme::surface_content());
    close_now();
    let (_s, ow, oh) = wm::spawn_geometry_native(w, h).ok_or_else(|| String::from("geometry unavailable"))?;
    let wtop = crate::ui_status::top_chrome_h(pw, ph);
    let ox = pw.saturating_sub(ow) / 2 + crate::ui::px(24);
    let oy = wtop + ph.saturating_sub(wtop).saturating_sub(crate::ui_status::chrome_h(ph)).saturating_sub(oh) / 3;
    let n_rows = rows.len();
    let n_links = links.len();
    let mut st = State { slug: String::from(slug), rows, links, top: 0, vis, sel: None, w, h, surf };
    paint(&mut st);
    let base = st.surf.as_ptr() as usize;
    // The Vec<u32> lives in STATE for the window's life; `close_now` drops the row before the buffer.
    *STATE.lock() = Some(st);
    let wt = alloc::format!("Help - {}", title);
    let id = wm::create_at_native(OWNER, base, w * h * 4, w as u32, h as u32, (w * 4) as u32, wt.as_bytes(), ox + wm::BORDER(), oy + wm::TITLE_H() + wm::BORDER());
    if id == wm::WIN_NONE {
        *STATE.lock() = None;
        return Err(String::from("window create failed"));
    }
    WIN.store(id, Ordering::Release);
    wm::winid_register_holder(&WIN, "help");
    wm::focus_changed(OWNER);
    let _ = wm::present(id);
    serial_println!("[help] open page={} win={} rows={} links={} cols={} vis={}", slug, id, n_rows, n_links, cols, vis);
    Ok(id as usize)
}

/// Open the corpus page `slug` (the index loaded on demand). The service pass and the fixture call this.
pub fn open_page(slug: &str) -> Result<usize, String> {
    super::index::ensure_loaded();
    let doc = super::index::with(|ix| ix.page(slug).map(|p| (p.title.clone(), p.body.clone(), p.keys.clone())));
    match doc {
        Some(Some((t, b, k))) => open_doc(slug, &t, &b, &k),
        Some(None) => Err(alloc::format!("no page {}", slug)),
        None => Err(String::from("no help index")),
    }
}

/// Open shell verb `name`'s doc (`help::render`) in the Help window.
pub fn open_verb(name: &str) -> Result<usize, String> {
    let d = super::find(name).ok_or_else(|| alloc::format!("no verb {}", name))?;
    let text = super::render(d);
    let body = alloc::format!("{}\n\nType it in the shell window. \"help\" there lists every command.", text.trim_end());
    open_doc(&alloc::format!("verb:{}", d.name), &alloc::format!("The {} command", d.name), &body, &[])
}

/// Close the window now (the service pass); the surface is freed after the row stops naming it.
fn close_now() {
    let id = WIN.swap(wm::WIN_NONE, Ordering::AcqRel);
    if id != wm::WIN_NONE && wm::owner_of(id) == Some(OWNER) {
        wm::close(id);
        serial_println!("[help] closed win={}", id);
    }
    *STATE.lock() = None;
}

/// Close the window (the fixture; the service pass).
pub fn close() {
    close_now();
}

/// Latch page `slug` for [`service`] (input-router safe).
pub fn request_open(slug: &str) {
    *OPEN_OWED.lock() = Some(String::from(slug));
    OPEN_POSTED.store(true, Ordering::Release);
}

/// F1: latch the focused program's page for [`service`] (the fn-row router's call; router safe).
pub fn request_for_focus() {
    F1_OWED.store(true, Ordering::Release);
}

/// The program the window with focus belongs to, as the corpus's `app:` names it.
pub fn app_of(owner: u64) -> &'static str {
    if owner == settings::OWNER {
        return "settings";
    }
    #[cfg(feature = "quarry")]
    if owner == crate::video::quarry::live::OWNER {
        return "quarry";
    }
    if owner == wm::KERNEL_OWNER_CONSOLE {
        return "shell";
    }
    "desktop"
}

/// The page F1 opens for `app`: the page whose slug is the app's name, else the first page marked for it, else `help`.
pub fn page_for_app(app: &str) -> String {
    super::index::ensure_loaded();
    super::index::with(|ix| {
        ix.page(app).filter(|p| p.app == app).or_else(|| ix.pages.iter().find(|p| p.app == app)).map(|p| p.slug.clone())
    })
    .flatten()
    .unwrap_or_else(|| String::from("help"))
}

/// **The pass** — chained from `launcher::service`. Idle: five atomic loads.
pub fn service() {
    if F1_OWED.swap(false, Ordering::AcqRel) {
        let owner = wm::focus_asid();
        let app = app_of(owner);
        let slug = page_for_app(app);
        serial_println!("[help] f1 owner={:#x} app={} page={}", owner, app, slug);
        request_open(&slug);
    }
    if OPEN_POSTED.swap(false, Ordering::AcqRel) {
        let want = OPEN_OWED.lock().take();
        if let Some(s) = want {
            let r = match s.strip_prefix("verb:") {
                Some(v) => open_verb(v),
                None => open_page(&s),
            };
            if let Err(e) = r {
                serial_println!("[help] open REFUSED page={} reason={}", s, e);
            }
        }
    }
    if CLOSE_OWED.swap(false, Ordering::AcqRel) {
        close_now();
    }
    let link = LINK_OWED.swap(u32::MAX, Ordering::AcqRel);
    if link != u32::MAX {
        let key = STATE.lock().as_ref().and_then(|s| s.links.get(link as usize).cloned());
        if let Some(k) = key {
            let _ = follow(&k);
        }
    }
    if PAINT_OWED.swap(false, Ordering::AcqRel) {
        repaint();
    }
}

/// Follow a link: Settings opens on the row `key` names. Returns `(tab, control)`.
pub fn follow(key: &str) -> (usize, usize) {
    let (tab, ctrl) = settings::request_open_at(key);
    serial_println!("[help] link key=system.{} tab={} control={}", key, tab, ctrl);
    (tab, ctrl)
}

// ── The doors (input router: edit and latch only) ───────────────────────────────────────────────

fn scroll_by(d: isize) {
    if let Some(st) = STATE.lock().as_mut() {
        let max = st.rows.len().saturating_sub(st.vis) as isize;
        st.top = (st.top as isize + d).clamp(0, max.max(0)) as usize;
    }
    PAINT_OWED.store(true, Ordering::Release);
}

/// Keys and wheel while the Help window holds focus. `true` when consumed.
pub fn key_route(ev: crate::pal::Event) -> bool {
    if !is_open() || wm::focus_asid() != OWNER {
        return false;
    }
    let vis = STATE.lock().as_ref().map(|s| s.vis).unwrap_or(1);
    let page = vis.saturating_sub(1).max(1) as isize;
    match ev {
        crate::pal::Event::Wheel(d) => scroll_by(-(d as isize) * 3),
        crate::pal::Event::Key(c) => match c {
            0x1F => scroll_by(-1),
            0x1E => scroll_by(1),
            b' ' => scroll_by(page),
            b'b' | b'B' => scroll_by(-page),
            0x09 => {
                if let Some(st) = STATE.lock().as_mut() {
                    let n = st.links.len();
                    if n > 0 {
                        let next = st.sel.map(|s| (s + 1) % n).unwrap_or(0);
                        st.sel = Some(next);
                        // Keep the selected link in view.
                        if let Some(r) = st.rows.iter().position(|r| r.kind == Kind::Link(next)) {
                            if r < st.top || r >= st.top + st.vis {
                                st.top = r.saturating_sub(st.vis.saturating_sub(1)).min(st.rows.len().saturating_sub(st.vis));
                            }
                        }
                    }
                }
                PAINT_OWED.store(true, Ordering::Release);
            }
            b'\r' | b'\n' => {
                if let Some(i) = STATE.lock().as_ref().and_then(|s| s.sel) {
                    LINK_OWED.store(i as u32, Ordering::Release);
                }
            }
            0x1B => CLOSE_OWED.store(true, Ordering::Release),
            _ => return false,
        },
        _ => return false,
    }
    true
}

/// A primary press: the close box closes, a link is followed, the content takes focus. `true` when consumed. A press
/// on the title bar (not the close box) is left to the window manager's drag.
pub fn press_route(x: i32, y: i32, mask: u8) -> bool {
    let id = WIN.load(Ordering::Acquire);
    if mask & 0x01 == 0 || !is_open() {
        return false;
    }
    match wm::hit_test(x, y) {
        Some((w, _, _)) if w == id => {}
        _ => return false,
    }
    if wm::close_box_hit(id, x, y) {
        CLOSE_OWED.store(true, Ordering::Release);
        return true;
    }
    let Some(info) = wm::info(id) else { return false };
    if x < info.x as i32 || y < info.y as i32 {
        return false;
    }
    let sc = info.scale.max(1);
    let (cx, cy) = ((x as usize - info.x) / sc, (y as usize - info.y) / sc);
    if cx >= info.w || cy >= info.h {
        return false;
    }
    wm::focus_changed(OWNER);
    let ch = face().cell_h().max(1);
    if let Some(st) = STATE.lock().as_mut() {
        if cy >= PAD() {
            let r = st.top + (cy - PAD()) / ch;
            if let Some(Kind::Link(i)) = st.rows.get(r).map(|r| r.kind) {
                st.sel = Some(i);
                LINK_OWED.store(i as u32, Ordering::Release);
                PAINT_OWED.store(true, Ordering::Release);
            }
        }
    }
    true
}
