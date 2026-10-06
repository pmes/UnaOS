// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! CHARTER: Kernel — wm
//!
//! APPMENU2 (B393, MACPARITY rows 2, 4, 13, 14) — **the WM's default menu set and the system chords.**
//!
//! Every window the bar names gets, from the WINDOW MANAGER and with no app code: the app menu (About,
//! Settings..., Hide, Hide Others, Show All, Quit), File (Close Window live; New/Open/Save greyed until
//! the app publishes its own File), Edit (Cut/Copy/Paste/Select All routed to the focused consumer;
//! Undo/Redo greyed), Window (`winlist`) and Help. The TREES live in `winmenu.rs` beside the renderer
//! (one registry, box kinds); this file holds their ACTIONS, the chord router [`key`] the x86 input
//! router asks beside `winlist::key`, and the `tests appmenu` fixture. Our names, our glyphs — only the
//! chords and the layout are the Mac's (MACPARITY's legal line).
//!
//! Every act is an existing verb: Quit on a USER window is the close box's own metal-proven path
//! (`syscall::app_quit_owner` → `wc_close_click`: the owner's rows close, then `bg_kill`), so a stuck
//! app is quittable; Quit on kernel furniture is `winmenu`'s QUITLEAK chain. Hide is `wm::minimise`,
//! Show All `wm::raise_one`, Settings `settings::request_open`, Force Quit posts `activity` to the shell
//! (the dock's own never-perform-in-the-router rule), Edit pushes the `pal` action a Cmd-C would.

use super::keymap::Action;
use super::{winmenu, wm};
use core::sync::atomic::{AtomicU64, Ordering};

static PICKS: AtomicU64 = AtomicU64::new(0);
static CHORDS: AtomicU64 = AtomicU64::new(0);

fn set_active(owner: u64) {
    #[cfg(target_arch = "x86_64")]
    crate::arch::x86_64::syscall::user_input_set_active(owner);
    #[cfg(not(target_arch = "x86_64"))]
    let _ = owner;
}

/// A USER owner (a process slot), as opposed to kernel furniture or the shared window.
fn is_user(owner: u64) -> bool {
    owner != 0 && owner < wm::KERNEL_OWNER_BASE
}

/// Focus the next window in cycle order (or hand the keyboard back to the shell).
fn focus_next() {
    let mut ord: alloc::vec::Vec<wm::WinId> = alloc::vec::Vec::new();
    if wm::cycle_order(&mut ord) > 0 && wm::wl_focus(ord[0]) {
        if let Some((_, o)) = wm::wl_focused() {
            set_active(o);
        }
    } else {
        set_active(0);
    }
}

/// The window the bar names (the frontmost focused row), falling back to the WM's focused app window.
fn front() -> Option<(wm::WinId, u64)> {
    let w = winmenu::app_window();
    if w != wm::WIN_NONE {
        if let Some(o) = wm::owner_of(w) {
            return Some((w, o));
        }
    }
    wm::wl_focused()
}

/// **Quit a USER window's app** through the close box's path. `true` when `win` belonged to a user
/// owner and was handled here; `false` leaves kernel furniture to `winmenu`'s QUITLEAK chain.
pub fn quit_user(win: wm::WinId, route: &str) -> bool {
    let Some(owner) = wm::owner_of(win) else { return false };
    if !is_user(owner) {
        return false;
    }
    quit_user_owner(win, owner, route)
}

/// M6 — a CLOSE REQUEST first (`una_abi::INPUT_EV_CLOSE_REQ`; the app may save and close itself), the
/// kill after `CLOSE_REQ_BOUND_MS`; with no live process to ask, the close box's close-then-kill now.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
fn quit_user_owner(win: wm::WinId, owner: u64, route: &str) -> bool {
    if quit_request(win, owner, route).is_some() {
        return true;
    }
    winmenu::clear(win);
    let settle = crate::arch::x86_64::syscall::app_quit_owner(win, owner);
    serial_println!("[sysmenu] quit owner={:#x} win={} route={} answer=killed-after-ms=0 settle={}", owner, win, route, settle);
    true
}

/// M6 — post the close request for a USER `owner` (the dock tile's Quit asks here too). `Some` when the
/// request is in flight; `None` when there is no live process to ask (the caller closes/kills itself).
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub fn quit_request(win: wm::WinId, owner: u64, route: &str) -> Option<&'static str> {
    if !is_user(owner) {
        return None;
    }
    if crate::arch::x86_64::syscall::app_quit_request(win, owner) {
        serial_println!("[sysmenu] quit owner={:#x} win={} route={} -> close-request", owner, win, route);
        Some("close-requested")
    } else {
        None
    }
}
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
pub fn quit_request(_win: wm::WinId, _owner: u64, _route: &str) -> Option<&'static str> {
    None
}
/// aarch64: no user-owner kill path from the WM yet; the Quit arm's `wm::close` stands (owed).
#[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
fn quit_user_owner(_win: wm::WinId, _owner: u64, _route: &str) -> bool {
    false
}

/// Quit the app that owns `win`: a user app by the close box's close-then-kill, kernel furniture by
/// `winmenu`'s Quit arm (pulse / quarry / instgui / bare `wm::close`).
pub fn quit_app(win: wm::WinId, route: &str) {
    if !quit_user(win, route) {
        winmenu::pick_app_row(win, winmenu::APP_ITEM_QUIT);
    }
}

/// Close one window (File > Close Window, Cmd-W). The LAST window of an app quits it, as on the Mac.
pub fn close_window(win: wm::WinId, route: &'static str) {
    let Some(owner) = wm::owner_of(win) else { return };
    let mut wr: alloc::vec::Vec<wm::WlRow> = alloc::vec::Vec::new();
    let live = wm::wl_rows(&mut wr);
    let mine = wr[..live].iter().filter(|r| r.owner == owner).count();
    if is_user(owner) && mine > 1 {
        winmenu::clear(win);
        let ok = wm::close(win);
        wm::focus_after_close(win, owner, route);
        serial_println!("[sysmenu] close-window win={} owner={:#x} siblings={} closed={}", win, owner, mine - 1, ok);
    } else {
        serial_println!("[sysmenu] close-window win={} owner={:#x} last=true -> quit", win, owner);
        quit_app(win, route);
    }
}

/// Hide every window of `owner` (Hide <app>, Cmd-H). Returns how many went down.
pub fn hide_owner(win: wm::WinId, owner: u64, route: &str) -> usize {
    let mut wr: alloc::vec::Vec<wm::WlRow> = alloc::vec::Vec::new();
    let live = wm::wl_rows(&mut wr);
    let mut n = 0usize;
    for r in wr[..live].iter().filter(|r| r.owner == owner && !r.minimised) {
        if wm::minimise(r.id).starts_with("parked") {
            n += 1;
        }
    }
    if n == 0 && wm::minimise(win).starts_with("parked") {
        n = 1; // kernel furniture (the console) is not in the app rows; the named window itself goes
    }
    if n > 0 {
        focus_next();
    }
    serial_println!("[sysmenu] hide owner={:#x} win={} hidden={} route={}", owner, win, n, route);
    n
}

/// Hide every OTHER app's windows (Hide Others).
fn hide_others(win: wm::WinId, owner: u64) -> usize {
    let mut wr: alloc::vec::Vec<wm::WlRow> = alloc::vec::Vec::new();
    let live = wm::wl_rows(&mut wr);
    let mut n = 0usize;
    for r in wr[..live].iter().filter(|r| r.owner != owner && !r.minimised) {
        if wm::minimise(r.id).starts_with("parked") {
            n += 1;
        }
    }
    if wm::wl_focus(win) {
        set_active(owner);
    }
    serial_println!("[sysmenu] hide-others owner={:#x} hidden={}", owner, n);
    n
}

/// Bring every hidden window back (Show All), keeping `win` in front.
fn show_all(win: wm::WinId, owner: u64) -> usize {
    let mut wr: alloc::vec::Vec<wm::WlRow> = alloc::vec::Vec::new();
    let live = wm::wl_rows(&mut wr);
    let mut n = 0usize;
    for r in wr[..live].iter().filter(|r| r.minimised) {
        if wm::raise_one(r.id) {
            n += 1;
        }
    }
    if win != wm::WIN_NONE && wm::wl_focus(win) {
        set_active(owner);
    }
    serial_println!("[sysmenu] show-all shown={}", n);
    n
}

/// Settings (Cmd-,): the settings window's latch, drained by the desktop's service pass.
fn open_settings(route: &str) {
    super::settings::request_open();
    serial_println!("[sysmenu] settings requested route={}", route);
}

/// Force Quit (Cmd-Option-Esc, the crystal's Force Quit...): Activity, POSTED to the shell — never
/// performed inside a router (the dock's rule).
pub fn force_quit(route: &str) {
    let posted = super::dock::post_line_launch("activity");
    serial_println!("[sysmenu] force-quit -> activity posted={} route={}", posted, route);
}

/// Edit: the action a chord would have produced, through the same queue, to the focused consumer.
fn edit(a: Action) {
    crate::pal::push_event(crate::pal::Event::Action(a));
    serial_println!("[sysmenu] edit action={} -> focused", a.name());
}

/// **A pick from one of the WM's own rows** (`winmenu::app_pick`'s fall-through).
pub fn pick(win: wm::WinId, id: u32) {
    PICKS.fetch_add(1, Ordering::Relaxed);
    let owner = wm::owner_of(win).unwrap_or(0);
    match id {
        winmenu::APP_ITEM_SETTINGS => open_settings("menu"),
        winmenu::APP_ITEM_HIDE => {
            hide_owner(win, owner, "menu");
        }
        winmenu::APP_ITEM_HIDE_OTHERS => {
            hide_others(win, owner);
        }
        winmenu::APP_ITEM_SHOW_ALL => {
            show_all(win, owner);
        }
        winmenu::FILE_ITEM_CLOSE => close_window(win, "route=file-close"),
        winmenu::EDIT_ITEM_CUT => edit(Action::Cut),
        winmenu::EDIT_ITEM_COPY => edit(Action::Copy),
        winmenu::EDIT_ITEM_PASTE => edit(Action::Paste),
        winmenu::EDIT_ITEM_SELECT_ALL => edit(Action::SelectAll),
        other => serial_println!("[sysmenu] pick REFUSE win={} id={:#x} reason=unbound (greyed rows are the app's to bind)", win, other),
    }
}

/// **The system chords**, asked by the x86 input router beside `winlist::key`. `true` when consumed.
/// Cmd-M is `winlist`'s and Ctrl-Cmd-Q / Cmd-L are the router's `LockScreen` arm; these are the rest.
pub fn key(a: Action) -> bool {
    let route = "chord";
    match a {
        Action::QuitApp | Action::CloseWindow | Action::HideApp => {
            CHORDS.fetch_add(1, Ordering::Relaxed);
            let Some((win, owner)) = front() else {
                serial_println!("[sysmenu] chord action={} win=none -> no-op", a.name());
                return true;
            };
            serial_println!("[sysmenu] chord action={} win={} owner={:#x}", a.name(), win, owner);
            match a {
                Action::QuitApp => quit_app(win, route),
                Action::CloseWindow => close_window(win, "route=cmd-w"),
                _ => {
                    hide_owner(win, owner, route);
                }
            }
            true
        }
        Action::OpenSettings => {
            CHORDS.fetch_add(1, Ordering::Relaxed);
            open_settings(route);
            true
        }
        Action::ForceQuit => {
            CHORDS.fetch_add(1, Ordering::Relaxed);
            force_quit(route);
            true
        }
        _ => false,
    }
}

/// `tests appmenu` (R80: a fixture, never at boot). The witness:
/// `:: APPMENU2: default_set=5 chords=[Q,W,H,M,comma,optesc] crystal=[lock,forcequit] publishers=unbounded -> PASS ::`
pub fn selftest() {
    // Chords: the LIVE table's answers to synthetic modifier bytes (LGUI 0x08, LALT 0x04, LCTRL 0x01).
    const GUI: u8 = 0x08;
    const ALT: u8 = 0x04;
    const CTRL: u8 = 0x01;
    let t = super::keymap::active();
    let r = |m: u8, u: u8| super::keymap::resolve(t, m, u);
    let legs: [(&str, bool); 6] = [
        ("Q", r(GUI, 0x14) == Some(Action::QuitApp)),
        ("W", r(GUI, 0x1A) == Some(Action::CloseWindow)),
        ("H", r(GUI, 0x0B) == Some(Action::HideApp)),
        ("M", r(GUI, 0x10) == Some(Action::Minimize)),
        ("comma", r(GUI, 0x36) == Some(Action::OpenSettings)),
        ("optesc", r(GUI | ALT, 0x29) == Some(Action::ForceQuit)),
    ];
    let lock_chord = r(GUI | CTRL, 0x14) == Some(Action::LockScreen);
    let mut chords = alloc::string::String::new();
    for (name, ok) in legs.iter() {
        if *ok {
            if !chords.is_empty() {
                chords.push(',');
            }
            chords.push_str(name);
        }
    }
    let chords_ok = legs.iter().all(|l| l.1) && lock_chord;

    // Crystal: the system menu's rows, read from its own table.
    let lock = super::crystal::has_row("Lock Screen");
    let fq = super::crystal::has_row("Force Quit...");
    let mut crystal = alloc::string::String::new();
    if lock {
        crystal.push_str("lock");
    }
    if fq {
        if !crystal.is_empty() {
            crystal.push(',');
        }
        crystal.push_str("forcequit");
    }

    // Publishers: more than the old cap of four, on ids past the table (no live window is touched).
    let base = wm::slots() as u32 + 1;
    const N: u32 = 6;
    let mut published = 0u32;
    for i in 0..N {
        if winmenu::publish(base + i, FIXTURE_TREE, fixture_pick) && winmenu::has_tree(base + i) {
            published += 1;
        }
    }
    for i in 0..N {
        winmenu::clear(base + i);
    }
    let pubs_ok = published == N;

    // The default set, laid out for a named window.
    let default_set = default_set_count();
    let ok = default_set == 5 && chords_ok && lock && fq && pubs_ok;
    serial_println!(
        "[sysmenu] fixture default_set={} chords_ok={} lock_chord={} published={}/{} picks={} chord_hits={}",
        default_set, chords_ok, lock_chord, published, N, PICKS.load(Ordering::Relaxed), CHORDS.load(Ordering::Relaxed)
    );
    serial_println!(
        ":: APPMENU2: default_set={} chords=[{}] crystal=[{}] publishers={} -> {} ::",
        default_set, chords, crystal, if pubs_ok { "unbounded" } else { "capped" }, if ok { "PASS" } else { "FAIL" }
    );
}

const FIXTURE_ITEMS: &[winmenu::MenuItem] = &[winmenu::MenuItem { id: 1, label: "One", flags: 0 }];
const FIXTURE_TREE: &[winmenu::MenuTitle] = &[winmenu::MenuTitle { label: "View", items: FIXTURE_ITEMS }];
fn fixture_pick(_id: u32) {}

/// The WM boxes (app, File, Edit, Window, Help) the bar lays out for a minted, named fixture window.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc"))]
fn default_set_count() -> usize {
    const OWNER: u64 = 0xA392;
    let id = wm::wl_fixture_mint(OWNER, b"am2");
    if id == wm::WIN_NONE {
        serial_println!("[sysmenu] fixture mint=refused (window table full)");
        return 0;
    }
    wm::wl_focus(id);
    winmenu::set_app_window(id, b"am2");
    let n = winmenu::wm_box_count();
    wm::close_owner(OWNER);
    n
}
#[cfg(not(all(feature = "witness", target_arch = "x86_64", feature = "wc")))]
fn default_set_count() -> usize {
    0
}
