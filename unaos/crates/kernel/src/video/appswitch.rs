// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — wm
//!
//! APPSWITCH (rmbp-ledger B428, MACPARITY row 10) — **Cmd-Tab cycles APPS on one strip of icons.**
//!
//! * THE APPS — the live window table grouped by `owner_asid` through WINDOWLIST's readers
//!   (`wm::cycle_order`, visible rows z-descending = most recent activation first; `wm::wl_rows`, every app
//!   row with its minimised bit). Apps whose every window is minimised (DOCK2) follow the visible ones.
//! * THE STRIP — a chromeless overlay row (`wm::overlay_open`, LAUNCHER's pattern), centred: one APPRES icon
//!   per app (`appres::blit_key_icon`, generic fallback), the selected one on an accent tile with its name
//!   under it. Tokens only (`theme::*()`).
//! * THE DOORS (R88: edit and latch only) — [`key_door`] (after `launcher::key_door` in x86's
//!   `wc_route_event`): `CycleWindow` (Cmd-Tab) builds the list on the first press and moves the selection on
//!   the next (Shift held: backwards); fewer than two apps is a consumed no-op; while up every key is its own
//!   and Esc cancels. [`hid_edge`] (from `xhci::hid_screenshot_chord_edge`, which both HID decoders call per
//!   report): the Cmd role's falling edge latches the commit, an Esc usage edge the cancel.
//! * THE PASS — [`service`] (chained after `launcher::service`): cancel, commit (activate + close), show, paint.
//!   A quick tap commits before the show is served, so nothing flashes.
//! * ACTIVATION — `wm::focus_changed(owner)` (every row of the owner gets a fresh z: minimised rows come
//!   back), then `wm::raise_one` back-to-front so the app keeps its own stack and its frontmost window is on top.
//!
//! Wire: `[appswitch] show apps=<n> sel=<name>`, `[appswitch] activate app=<name> windows=<n> restored=<n>`,
//! `[appswitch] cancel`, `[appswitch] one-app`; `tests appswitch` →
//! `:: APPSWITCH: apps=<n> order=mru strip=<ok|skip-oneapp> activate=<ok|skip-oneapp> -> PASS ::`.
//! Design: `docs/dev/evidence/rmbp-1005/appswitch.md`.

use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::{menubar, theme, wm};

// ── The pure core: group rows into apps, most recent first ──────────────────────────────────────

/// One app on the strip.
#[derive(Clone)]
pub struct App {
    pub owner: u64,
    /// Display name (the armed program's stem, else the frontmost title's first word).
    pub name: String,
    /// APPRES icon key.
    pub key: String,
    /// The app's windows back-to-front: minimised first, then visible ones by z ascending; the last is the front.
    pub wins: Vec<wm::WinId>,
    /// How many of `wins` are minimised.
    pub minimised: usize,
}

/// Group: `visible` = `(id, owner)` z-descending; `all` = `(id, owner, minimised)` in table order. Returns
/// `(owner, wins back-to-front, minimised count)` per app, most recently activated first, apps with only
/// minimised windows last (table order).
pub fn group(visible: &[(wm::WinId, u64)], all: &[(wm::WinId, u64, bool)]) -> Vec<(u64, Vec<wm::WinId>, usize)> {
    let mut out: Vec<(u64, Vec<wm::WinId>, usize)> = Vec::new();
    let mut add = |owner: u64| {
        if owner != 0 && !out.iter().any(|a| a.0 == owner) {
            out.push((owner, Vec::new(), 0));
        }
    };
    for &(_, o) in visible {
        add(o);
    }
    for &(_, o, m) in all {
        if m {
            add(o);
        }
    }
    for a in out.iter_mut() {
        for &(id, o, m) in all {
            if o == a.0 && m {
                a.1.push(id);
                a.2 += 1;
            }
        }
        for &(id, o) in visible.iter().rev() {
            if o == a.0 && !a.1.contains(&id) {
                a.1.push(id);
            }
        }
    }
    out.retain(|a| !a.1.is_empty());
    out
}

fn display_name(owner: u64, front_title: &[u8]) -> String {
    let mut nm = [0u8; wm::MAX_TITLE];
    let l = wm::app_name_of(owner, &mut nm);
    let raw: &[u8] = if l != 0 {
        &nm[..l]
    } else {
        let end = front_title.iter().position(|&c| c == b' ' || c == b':' || c == 0).unwrap_or(front_title.len());
        &front_title[..end]
    };
    let s: String = raw.iter().map(|&b| if (0x20..0x7f).contains(&b) { b as char } else { '?' }).collect();
    let stem = crate::fs::appres::key_of_path(&s);
    if stem.is_empty() {
        return String::from("app");
    }
    stem
}

/// The live apps, most recently activated first.
pub fn apps() -> Vec<App> {
    let mut ord: Vec<wm::WinId> = Vec::new();
    let n = wm::cycle_order(&mut ord);
    let mut rows: Vec<wm::WlRow> = Vec::new();
    let live = wm::wl_rows(&mut rows);
    let owner_of = |id: wm::WinId| rows[..live].iter().find(|r| r.id == id).map(|r| r.owner).unwrap_or(0);
    let visible: Vec<(wm::WinId, u64)> = ord[..n].iter().map(|&id| (id, owner_of(id))).collect();
    let all: Vec<(wm::WinId, u64, bool)> = rows[..live].iter().map(|r| (r.id, r.owner, r.minimised)).collect();
    group(&visible, &all)
        .into_iter()
        .map(|(owner, wins, minimised)| {
            let front = *wins.last().unwrap_or(&wm::WIN_NONE);
            let title = rows[..live].iter().find(|r| r.id == front).map(|r| r.title[..r.len].to_vec()).unwrap_or_default();
            let name = display_name(owner, &title);
            let key = crate::fs::appres::key_of_title(&title).unwrap_or_else(|| name.clone());
            App { owner, name, key, wins, minimised }
        })
        .collect()
}

// ── State ───────────────────────────────────────────────────────────────────────────────────────

struct St {
    apps: Vec<App>,
    sel: usize,
    w: usize,
    h: usize,
    buf: Vec<u32>,
}

static ACTIVE: AtomicBool = AtomicBool::new(false);
static WIN: AtomicU32 = AtomicU32::new(0);
static ST: spin::Mutex<Option<St>> = spin::Mutex::new(None);
static SHOW_OWED: AtomicBool = AtomicBool::new(false);
static PAINT_OWED: AtomicBool = AtomicBool::new(false);
static COMMIT_OWED: AtomicBool = AtomicBool::new(false);
static CANCEL_OWED: AtomicBool = AtomicBool::new(false);
static CMD_PREV: AtomicBool = AtomicBool::new(false);
/// The fixture's stand-in for a held Cmd key (the HID byte is the operator's).
static FIXTURE_HELD: AtomicBool = AtomicBool::new(false);

/// Is the switcher up (between the first Cmd-Tab and the release)?
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

fn cmd_held() -> bool {
    CMD_PREV.load(Ordering::Acquire) || FIXTURE_HELD.load(Ordering::Acquire)
}

// ── Paint ───────────────────────────────────────────────────────────────────────────────────────

fn fill(buf: &mut [u32], w: usize, h: usize, x: usize, y: usize, rw: usize, rh: usize, c: u32) {
    for yy in y..(y + rh).min(h) {
        for xx in x..(x + rw).min(w) {
            buf[yy * w + xx] = c;
        }
    }
}

fn text(buf: &mut [u32], w: usize, h: usize, s: &str, x: usize, top: usize, ink: u32, bold: bool) {
    let ch = menubar::BAR_CELL_H();
    let b = s.as_bytes();
    for sy in 0..ch {
        let y = top + sy;
        if y < h {
            super::text::draw_row(&mut buf[y * w..(y + 1) * w], w, b, x, sy, ink, bold, menubar::BAR_FACE);
        }
    }
}

fn icon_size() -> usize {
    (menubar::BAR_CELL_H() * 3).max(32)
}

const PAD: usize = 16;
const GAP: usize = 12;

fn geometry(n: usize, pw: usize) -> (usize, usize, usize) {
    let s = icon_size();
    let slot = s + GAP;
    let max_slots = (pw.saturating_sub(40 + 2 * PAD) / slot).max(1);
    let shown = n.min(max_slots);
    let w = (2 * PAD + shown * slot - GAP).max(menubar::BAR_CELL_W() * 20);
    let h = PAD + s + 8 + menubar::BAR_CELL_H() + PAD;
    (w, h, shown)
}

fn paint(st: &mut St) {
    let (w, h) = (st.w, st.h);
    let s = icon_size();
    let slot = s + GAP;
    let ch = menubar::BAR_CELL_H();
    let cw = menubar::BAR_CELL_W().max(1);
    let n = st.apps.len();
    let shown = ((w.saturating_sub(2 * PAD) + GAP) / slot).min(n).max(1);
    let first = if st.sel >= shown { st.sel + 1 - shown } else { 0 };
    let buf = &mut st.buf;
    fill(buf, w, h, 0, 0, w, h, theme::chrome_face());
    fill(buf, w, h, 0, 0, w, 1, theme::frame_line());
    fill(buf, w, h, 0, h - 1, w, 1, theme::frame_line());
    fill(buf, w, h, 0, 0, 1, h, theme::frame_line());
    fill(buf, w, h, w - 1, 0, 1, h, theme::frame_line());
    let row_w = shown * slot - GAP;
    let x0 = (w - row_w) / 2;
    for (k, a) in st.apps.iter().enumerate().skip(first).take(shown) {
        let x = x0 + (k - first) * slot;
        let on = k == st.sel;
        if on {
            fill(buf, w, h, x - GAP / 2, PAD - GAP / 2, s + GAP, s + GAP, theme::selection());
        }
        if !crate::fs::appres::blit_key_icon(buf, w, h, x, PAD, s, &a.key) {
            fill(buf, w, h, x, PAD, s, s, theme::frame_line());
            let mark = &a.name[..a.name.len().min(1)];
            text(buf, w, h, &mark.to_ascii_uppercase(), x + (s - cw) / 2, PAD + (s - ch) / 2, theme::bevel_light(), true);
        }
        if on {
            let name = &a.name[..a.name.len().min(w / cw - 2)];
            let tw = name.len() * cw;
            let tx = (x + s / 2).saturating_sub(tw / 2).clamp(PAD / 2, w.saturating_sub(tw + PAD / 2));
            text(buf, w, h, name, tx, PAD + s + 8, theme::content_text(), true);
        }
    }
}

fn repaint() {
    let id = WIN.load(Ordering::Acquire);
    if id == 0 {
        return;
    }
    {
        let mut g = ST.lock();
        let Some(st) = g.as_mut() else { return };
        paint(st);
    }
    let _ = wm::present(id);
}

fn open_strip() -> bool {
    if WIN.load(Ordering::Acquire) != 0 {
        return true;
    }
    let (pw, ph) = {
        let fb = *super::WRITER.lock();
        if !fb.is_ready() {
            return false;
        }
        let i = fb.info();
        (i.width, i.height)
    };
    let mut g = ST.lock();
    let Some(st) = g.as_mut() else { return false };
    let (w, h, _) = geometry(st.apps.len(), pw);
    let (w, h) = (w.min(pw.saturating_sub(8)).max(32), h.min(ph.saturating_sub(8)).max(32));
    let mut buf: Vec<u32> = Vec::new();
    if buf.try_reserve_exact(w * h).is_err() {
        return false;
    }
    buf.resize(w * h, theme::chrome_face());
    st.w = w;
    st.h = h;
    st.buf = buf;
    paint(st);
    let (addr, len) = (st.buf.as_ptr() as usize, st.buf.len() * 4);
    let (ox, oy) = (pw.saturating_sub(w) / 2, ph.saturating_sub(h) / 2);
    let sel = st.apps.get(st.sel).map(|a| a.name.clone()).unwrap_or_default();
    let n = st.apps.len();
    drop(g);
    let id = wm::overlay_open(addr, len, w, h, ox, oy);
    if id == wm::WIN_NONE {
        serial_println!("[appswitch] show REFUSED reason=overlay");
        return false;
    }
    WIN.store(id, Ordering::Release);
    serial_println!("[appswitch] show apps={} sel={}", n, sel);
    true
}

fn close() {
    let id = WIN.swap(0, Ordering::AcqRel);
    if id != 0 {
        wm::close(id);
    }
    *ST.lock() = None; // after the row is gone
    ACTIVE.store(false, Ordering::Release);
    SHOW_OWED.store(false, Ordering::Release);
    PAINT_OWED.store(false, Ordering::Release);
}

/// Activate `a`: focus to its owner (every row raised, minimised ones restored), then its windows back-to-front
/// so the app's own stack is kept and its frontmost window ends on top. Returns the windows raised.
fn activate(a: &App) -> usize {
    #[cfg(target_arch = "x86_64")]
    crate::arch::x86_64::syscall::user_input_set_active(a.owner);
    if wm::drag_active() != wm::WIN_NONE {
        wm::drag_cancel("appswitch");
    }
    wm::focus_changed(a.owner);
    let mut raised = 0usize;
    for &id in a.wins.iter() {
        if wm::raise_one(id) {
            raised += 1;
        }
    }
    serial_println!("[appswitch] activate app={} windows={} restored={}", a.name, raised, a.minimised);
    raised
}

fn owe() {
    #[cfg(not(feature = "quarry"))]
    service();
}

// ── The doors ───────────────────────────────────────────────────────────────────────────────────

/// **The key door** — asked right after `launcher::key_door` by x86's `wc_route_event`. `true` when consumed.
pub fn key_door(ev: crate::pal::Event) -> bool {
    use crate::pal::Event;
    use super::keymap::Action;
    if let Event::Action(Action::CycleWindow) = ev {
        let back = super::keymap::shift_held();
        if is_active() {
            if let Some(st) = ST.lock().as_mut() {
                let n = st.apps.len().max(1);
                st.sel = if back { (st.sel + n - 1) % n } else { (st.sel + 1) % n };
            }
            PAINT_OWED.store(true, Ordering::Release);
        } else {
            let apps = apps();
            if apps.len() < 2 {
                serial_println!("[appswitch] one-app apps={}", apps.len());
                return true;
            }
            let sel = if back { apps.len() - 1 } else { 1 };
            *ST.lock() = Some(St { apps, sel, w: 0, h: 0, buf: Vec::new() });
            ACTIVE.store(true, Ordering::Release);
            SHOW_OWED.store(true, Ordering::Release);
            if !cmd_held() {
                COMMIT_OWED.store(true, Ordering::Release); // released before the press was routed: a tap
            }
        }
        owe();
        return true;
    }
    if !is_active() {
        return false;
    }
    match ev {
        Event::Key(0x1B) | Event::Action(Action::Deselect) => {
            CANCEL_OWED.store(true, Ordering::Release);
            owe();
            true
        }
        Event::Key(_) | Event::KeyUp(_) | Event::Action(_) => true,
        _ => false,
    }
}

/// **The HID edge** — every keyboard report, from `xhci::hid_screenshot_chord_edge` (both decoders). Atomics only.
pub fn hid_edge(cur_keys: &[u8; 6], prev_keys: &[u8; 6], modifiers: u8) {
    let held = modifiers & super::keymap::active().cmd_role != 0;
    let was = CMD_PREV.swap(held, Ordering::AcqRel);
    if !is_active() {
        return;
    }
    if was && !held {
        COMMIT_OWED.store(true, Ordering::Release);
    }
    const ESC: u8 = 0x29;
    if cur_keys.contains(&ESC) && !prev_keys.contains(&ESC) {
        CANCEL_OWED.store(true, Ordering::Release);
    }
}

/// **The pass** — chained after `launcher::service`. Idle: four atomic loads.
pub fn service() {
    if CANCEL_OWED.swap(false, Ordering::AcqRel) {
        COMMIT_OWED.store(false, Ordering::Release);
        if is_active() {
            close();
            serial_println!("[appswitch] cancel");
        }
        return;
    }
    if COMMIT_OWED.swap(false, Ordering::AcqRel) {
        let pick = ST.lock().as_ref().and_then(|st| st.apps.get(st.sel).cloned());
        close();
        if let Some(a) = pick {
            activate(&a);
        }
        return;
    }
    if SHOW_OWED.swap(false, Ordering::AcqRel) {
        PAINT_OWED.store(false, Ordering::Release);
        if is_active() {
            let _ = open_strip();
        }
    } else if PAINT_OWED.swap(false, Ordering::AcqRel) {
        repaint();
    }
}

// ── The fixture ─────────────────────────────────────────────────────────────────────────────────

/// `tests appswitch` (R80: typed, never at boot). The grouping core on a synthetic table (MRU order, one entry
/// per owner, minimised-only apps last, the front window last), then — with two or more live apps — the real
/// door: Cmd-Tab held (the fixture stands in for the Cmd byte), the strip shown by the pass, the release
/// committed by the pass, focus on the selected owner; a second round puts focus back where it was.
pub fn selftest() {
    // Synthetic: owner 7 has windows 3 (front) and 1; owner 9 has 2 visible; owner 5 has only 4, minimised.
    let vis = [(3, 7u64), (2, 9), (1, 7)];
    let all = [(1, 7u64, false), (2, 9, false), (3, 7, false), (4, 5, true)];
    let g = group(&vis, &all);
    let order_ok = g.len() == 3
        && g[0].0 == 7 && g[0].1 == [1, 3] && g[0].2 == 0
        && g[1].0 == 9 && g[1].1 == [2]
        && g[2].0 == 5 && g[2].1 == [4] && g[2].2 == 1
        && group(&[(1, 7)], &[(1, 7, false)]).len() == 1;
    let live = apps();
    let n = live.len();
    let (strip, act) = if n < 2 {
        ("skip-oneapp", "skip-oneapp")
    } else if is_active() {
        ("busy", "busy")
    } else {
        let before = wm::focus_asid();
        let want = live[1].owner;
        FIXTURE_HELD.store(true, Ordering::Release);
        key_door(crate::pal::Event::Action(super::keymap::Action::CycleWindow));
        service();
        let shown = WIN.load(Ordering::Acquire) != 0;
        let headless = !super::WRITER.lock().is_ready();
        FIXTURE_HELD.store(false, Ordering::Release);
        COMMIT_OWED.store(true, Ordering::Release);
        service();
        let landed = wm::focus_asid() == want && !is_active();
        // Put focus back: the previous app is now index 1.
        if before != 0 && before != want {
            key_door(crate::pal::Event::Action(super::keymap::Action::CycleWindow));
            service();
        }
        serial_println!("[appswitch] fixture shown={} headless={} want={} landed={} back={}", shown, headless, want, landed, wm::focus_asid() == before);
        (if shown { "ok" } else if headless { "skip-headless" } else { "FAIL" }, if landed { "ok" } else { "FAIL" })
    };
    let ok = order_ok && strip != "FAIL" && act != "FAIL";
    serial_println!(
        ":: APPSWITCH: apps={} order={} strip={} activate={} -> {} ::",
        n, if order_ok { "mru" } else { "FAIL" }, strip, act, if ok { "PASS" } else { "FAIL" }
    );
}

/// `tests appswitch` registration, once (rides `appres::ensure_tests`).
pub fn ensure_tests() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, Ordering::AcqRel) {
        crate::tests::register("appswitch", selftest);
    }
}
