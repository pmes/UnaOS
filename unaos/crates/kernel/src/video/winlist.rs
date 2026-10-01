// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// WINDOWLIST (R75) — the menubar's WINDOW menu, ⌘M and ⌘`.
//
// The bar (`winmenu`) lays a `Window` box LAST on the bar whenever a window is named on it (or while Show
// Desktop holds every window down, so the way back stays reachable). Its rows are kernel-authored and
// REBUILT FROM THE LIVE TABLE at every open ([`rebuild`], task context): Minimize, Zoom, Snap Left/Right
// (x86 `winsnap`), Bring All to Front, Show Desktop, a keyline, then one row per live app window with the
// focused one checked. A row's label is `&'static str` by the registry's design, so the rows and their label
// bytes live in static storage written ONLY by `rebuild` (open time) and read by the painter / press router
// on the same single input path — never concurrently with a write.
//
// Every act is an existing wm verb: `wm::minimise`, `wm::zoom`, `winsnap::key`, `wm::raise_one`,
// `wm::cycle_commit` (focus + raise). Show Desktop minimises each live app window by the same flag and
// remembers which it moved in [`SD_MASK`]; the second pick raises exactly those back and re-focuses the
// window that had focus.

use super::keymap::Action;
use super::winmenu::{MenuItem, FLAG_CHECKED, FLAG_DISABLED, FLAG_SEPARATOR, MENU_LABEL_MAX};
use super::wm;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

/// The bar title.
pub const LABEL: &str = "Window";

pub const ID_MINIMIZE: u32 = 0x100;
pub const ID_ZOOM: u32 = 0x101;
pub const ID_SNAP_LEFT: u32 = 0x102;
pub const ID_SNAP_RIGHT: u32 = 0x103;
pub const ID_FRONT: u32 = 0x104;
pub const ID_SHOW_DESKTOP: u32 = 0x105;
/// A window row's id is `ID_WIN_BASE + WinId`.
pub const ID_WIN_BASE: u32 = 0x200;

/// Fixed rows (incl. the keyline) ahead of the per-window rows.
#[cfg(target_arch = "x86_64")]
pub const FIXED_ROWS: usize = 7;
#[cfg(not(target_arch = "x86_64"))]
pub const FIXED_ROWS: usize = 5;
const CAP: usize = FIXED_ROWS + wm::MAX_WINDOWS;

struct Store {
    rows: UnsafeCell<[MenuItem; CAP]>,
    labels: UnsafeCell<[[u8; MENU_LABEL_MAX]; wm::MAX_WINDOWS]>,
}
// SAFETY: written only by `rebuild` from the bar's open path (task context); every reader runs on that same
// input path after it, so a read never overlaps a write.
unsafe impl Sync for Store {}
const BLANK: MenuItem = MenuItem { id: 0, label: "", flags: 0 };
static STORE: Store = Store { rows: UnsafeCell::new([BLANK; CAP]), labels: UnsafeCell::new([[0; MENU_LABEL_MAX]; wm::MAX_WINDOWS]) };
static COUNT: AtomicUsize = AtomicUsize::new(0);

/// Show Desktop is holding windows down.
static SD_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Bit `id-1` = window `id` was minimised by Show Desktop.
static SD_MASK: AtomicU64 = AtomicU64::new(0);
/// The owner that held focus when Show Desktop went down.
static SD_FOCUS: AtomicU64 = AtomicU64::new(0);

/// Is Show Desktop holding windows down? (The bar keeps the Window box up while it is.)
pub fn desktop_hidden() -> bool {
    SD_ACTIVE.load(Ordering::Relaxed)
}

/// The rows the Window dropdown shows (as of the last [`rebuild`]).
pub fn rows() -> &'static [MenuItem] {
    let n = COUNT.load(Ordering::Acquire).min(CAP);
    // SAFETY: see `Store`.
    unsafe { &(*STORE.rows.get())[..n] }
}

fn set_active(owner: u64) {
    #[cfg(target_arch = "x86_64")]
    crate::arch::x86_64::syscall::user_input_set_active(owner);
    #[cfg(not(target_arch = "x86_64"))]
    let _ = owner;
}

/// Rebuild the rows from the live window table.
pub fn rebuild() {
    // SAFETY: see `Store`.
    let rows = unsafe { &mut *STORE.rows.get() };
    let labels = unsafe { &mut *STORE.labels.get() };
    let mut wr = [wm::WlRow { id: 0, owner: 0, minimised: false, title: [0; wm::MAX_TITLE], len: 0 }; wm::MAX_WINDOWS];
    let live = wm::wl_rows(&mut wr);
    let foc = wm::wl_focused();
    let has_focus = foc.is_some();
    let dis = if has_focus { 0 } else { FLAG_DISABLED };
    let mut n = 0usize;
    let mut push = |id: u32, label: &'static str, flags: u32| {
        rows[n] = MenuItem { id, label, flags };
        n += 1;
    };
    push(ID_MINIMIZE, "Minimize", dis);
    push(ID_ZOOM, "Zoom", dis);
    #[cfg(target_arch = "x86_64")]
    {
        push(ID_SNAP_LEFT, "Snap Left", dis);
        push(ID_SNAP_RIGHT, "Snap Right", dis);
    }
    push(ID_FRONT, "Bring All to Front", if live == 0 { FLAG_DISABLED } else { 0 });
    push(ID_SHOW_DESKTOP, "Show Desktop", if desktop_hidden() { FLAG_CHECKED } else if live == 0 { FLAG_DISABLED } else { 0 });
    push(0, "", FLAG_SEPARATOR);
    for i in 0..live {
        let r = &wr[i];
        let mut len = 0usize;
        let lab = &mut labels[i];
        for &b in r.title[..r.len].iter().take(wm::MAX_TITLE) {
            lab[len] = if (0x20..0x7f).contains(&b) { b } else { b'?' };
            len += 1;
        }
        if len == 0 {
            for &b in b"untitled" {
                lab[len] = b;
                len += 1;
            }
        }
        if r.minimised {
            for &b in b" (min)" {
                if len < MENU_LABEL_MAX {
                    lab[len] = b;
                    len += 1;
                }
            }
        }
        // SAFETY: ASCII only (sanitised above); the bytes live in `STORE` for the program's life.
        let s: &'static str = unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(lab.as_ptr(), len)) };
        let checked = foc.map_or(false, |(fid, _)| fid == r.id);
        push(ID_WIN_BASE + r.id, s, if checked { FLAG_CHECKED } else { 0 });
    }
    COUNT.store(n, Ordering::Release);
}

/// Minimise the focused window and hand focus to the next visible one (or the shell).
pub fn minimise_focused(route: &str) -> bool {
    let Some((id, owner)) = wm::wl_focused() else { return false };
    let settle = wm::minimise(id);
    serial_println!("[wm-act] minimise win={} owner={:#x} route={} -> settle={}", id, owner, route, settle);
    let parked = settle.starts_with("parked");
    if parked {
        let mut ord = [wm::WIN_NONE; wm::MAX_WINDOWS];
        if wm::cycle_order(&mut ord) > 0 && wm::wl_focus(ord[0]) {
            if let Some((_, o)) = wm::wl_focused() {
                set_active(o);
            }
        } else {
            set_active(0);
        }
    }
    parked
}

/// Show Desktop: minimise every live app window (remembering which), or — when it already did — raise those
/// back and give focus to the window that had it. Returns how many windows moved.
pub fn show_desktop_toggle() -> usize {
    let mut wr = [wm::WlRow { id: 0, owner: 0, minimised: false, title: [0; wm::MAX_TITLE], len: 0 }; wm::MAX_WINDOWS];
    let live = wm::wl_rows(&mut wr);
    let mut moved = 0usize;
    if !SD_ACTIVE.load(Ordering::Relaxed) {
        let mut mask = 0u64;
        for r in wr[..live].iter().filter(|r| !r.minimised) {
            if wm::minimise(r.id).starts_with("parked") {
                mask |= 1u64 << (r.id - 1);
                moved += 1;
            }
        }
        if moved > 0 {
            SD_FOCUS.store(wm::focus_asid(), Ordering::Relaxed);
            SD_MASK.store(mask, Ordering::Relaxed);
            SD_ACTIVE.store(true, Ordering::Release);
            set_active(0);
        }
        serial_println!("[winlist] show-desktop hide moved={} live={}", moved, live);
    } else {
        let mask = SD_MASK.swap(0, Ordering::Relaxed);
        SD_ACTIVE.store(false, Ordering::Release);
        let focus = SD_FOCUS.load(Ordering::Relaxed);
        let mut top = wm::WIN_NONE;
        for r in wr[..live].iter() {
            if mask & (1u64 << (r.id - 1)) != 0 && r.minimised && wm::raise_one(r.id) {
                moved += 1;
                if r.owner == focus || top == wm::WIN_NONE {
                    top = r.id;
                }
            }
        }
        if top != wm::WIN_NONE && wm::wl_focus(top) {
            if let Some((_, o)) = wm::wl_focused() {
                set_active(o);
            }
        }
        serial_println!("[winlist] show-desktop restore moved={} focus={:#x}", moved, focus);
    }
    moved
}

/// Deliver a pick from the Window menu (the bar already dismissed it).
pub fn pick(id: u32) {
    match id {
        ID_MINIMIZE => {
            let ok = minimise_focused("menu");
            serial_println!("[winlist] pick minimize ok={}", ok);
        }
        ID_ZOOM => {
            if let Some((win, _)) = wm::wl_focused() {
                let settle = wm::zoom(win);
                serial_println!("[winlist] pick zoom win={} -> {}", win, settle);
            }
        }
        #[cfg(target_arch = "x86_64")]
        ID_SNAP_LEFT => {
            let ok = wm::winsnap::key(Action::SnapLeft);
            serial_println!("[winlist] pick snap-left ok={}", ok);
        }
        #[cfg(target_arch = "x86_64")]
        ID_SNAP_RIGHT => {
            let ok = wm::winsnap::key(Action::SnapRight);
            serial_println!("[winlist] pick snap-right ok={}", ok);
        }
        ID_FRONT => {
            let focused = wm::wl_focused();
            let n = wm::wl_raise_all();
            if let Some((id, _)) = focused {
                wm::raise_one(id);
            }
            serial_println!("[winlist] pick bring-all-to-front raised={}", n);
        }
        ID_SHOW_DESKTOP => {
            show_desktop_toggle();
        }
        w if w > ID_WIN_BASE => {
            let win = w - ID_WIN_BASE;
            // Picking a window while Show Desktop holds the rest down leaves the rest down (macOS), but the
            // toggle is over: the next Show Desktop minimises again.
            SD_ACTIVE.store(false, Ordering::Release);
            SD_MASK.store(0, Ordering::Relaxed);
            let ok = wm::wl_focus(win);
            if let Some((_, o)) = wm::wl_focused() {
                set_active(o);
            }
            serial_println!("[winlist] pick window win={} focused={}", win, ok);
        }
        other => serial_println!("[winlist] pick REFUSE id={}", other),
    }
}

/// The router seam for the two window chords (`⌘M`, `⌘``). `true` when the action was ours.
pub fn key(a: Action) -> bool {
    match a {
        Action::Minimize => {
            minimise_focused("key");
            true
        }
        Action::CycleApp => {
            if let Some((_, owner)) = wm::wl_focused() {
                if let Some(id) = wm::wl_same_app_pick(owner) {
                    set_active(owner);
                    let ok = wm::cycle_commit(id, owner);
                    serial_println!("[winlist] cycle-app win={} owner={:#x} ok={}", id, owner, ok);
                }
            }
            true
        }
        _ => false,
    }
}
