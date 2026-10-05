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
// WINDOWCAP-2: no `CAP` — the rows and their label bytes are growable (one row per live app window).

struct Store {
    rows: UnsafeCell<alloc::vec::Vec<MenuItem>>,
    labels: UnsafeCell<alloc::vec::Vec<[u8; MENU_LABEL_MAX]>>,
}
// SAFETY: written only by `rebuild` from the bar's open path (task context); every reader runs on that same
// input path after it, so a read never overlaps a write.
unsafe impl Sync for Store {}
const BLANK: MenuItem = MenuItem { id: 0, label: "", flags: 0 };
static STORE: Store = Store { rows: UnsafeCell::new(alloc::vec::Vec::new()), labels: UnsafeCell::new(alloc::vec::Vec::new()) }; // WINDOWCAP-2: growable; `BLANK` kept for the type's zero row
#[allow(dead_code)] const _BLANK_KEPT: MenuItem = BLANK;
static COUNT: AtomicUsize = AtomicUsize::new(0);

/// Show Desktop is holding windows down.
static SD_ACTIVE: AtomicBool = AtomicBool::new(false);
/// Bit `id-1` = window `id` was minimised by Show Desktop.
static SD_MASK: spin::Mutex<alloc::vec::Vec<wm::WinId>> = spin::Mutex::new(alloc::vec::Vec::new()); // WINDOWCAP-2: the ids Show Desktop moved (was a u64 bit per id)
/// The owner that held focus when Show Desktop went down.
static SD_FOCUS: AtomicU64 = AtomicU64::new(0);

/// Is Show Desktop holding windows down? (The bar keeps the Window box up while it is.)
pub fn desktop_hidden() -> bool {
    SD_ACTIVE.load(Ordering::Relaxed)
}

/// The rows the Window dropdown shows (as of the last [`rebuild`]).
pub fn rows() -> &'static [MenuItem] {
    // SAFETY: see `Store`.
    let r = unsafe { &*STORE.rows.get() };
    let n = COUNT.load(Ordering::Acquire).min(r.len());
    &r[..n]
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
    let mut wr: alloc::vec::Vec<wm::WlRow> = alloc::vec::Vec::new(); // WINDOWCAP-2
    let live = wm::wl_rows(&mut wr);
    COUNT.store(0, Ordering::Release); rows.clear(); rows.reserve(FIXED_ROWS + live); labels.clear(); labels.resize(live, [0; MENU_LABEL_MAX]); // WINDOWCAP-2: sized BEFORE any label pointer is taken, so no row can point into a buffer a later push reallocated
    let foc = wm::wl_focused();
    let has_focus = foc.is_some();
    let dis = if has_focus { 0 } else { FLAG_DISABLED };
    let mut n = 0usize;
    let mut push = |id: u32, label: &'static str, flags: u32| {
        rows.push(MenuItem { id, label, flags });
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
        let mut ord: alloc::vec::Vec<wm::WinId> = alloc::vec::Vec::new(); // WINDOWCAP-2
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
    let mut wr: alloc::vec::Vec<wm::WlRow> = alloc::vec::Vec::new(); // WINDOWCAP-2
    let live = wm::wl_rows(&mut wr);
    let mut moved = 0usize;
    if !SD_ACTIVE.load(Ordering::Relaxed) {
        let mut mask: alloc::vec::Vec<wm::WinId> = alloc::vec::Vec::new();
        for r in wr[..live].iter().filter(|r| !r.minimised) {
            if wm::minimise(r.id).starts_with("parked") {
                mask.push(r.id);
                moved += 1;
            }
        }
        if moved > 0 {
            SD_FOCUS.store(wm::focus_asid(), Ordering::Relaxed);
            *SD_MASK.lock() = mask;
            SD_ACTIVE.store(true, Ordering::Release);
            set_active(0);
        }
        serial_println!("[winlist] show-desktop hide moved={} live={}", moved, live);
    } else {
        let mask = core::mem::take(&mut *SD_MASK.lock());
        SD_ACTIVE.store(false, Ordering::Release);
        let focus = SD_FOCUS.load(Ordering::Relaxed);
        let mut top = wm::WIN_NONE;
        for r in wr[..live].iter() {
            if mask.contains(&r.id) && r.minimised && wm::raise_one(r.id) {
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
            SD_MASK.lock().clear();
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

/// WINDOWLIST fixture (`tests windowlist`): two minted windows; the Window menu is opened through the bar's press
/// router (`winmenu::press_at`, the seam every desktop click takes first), the SECOND window's row is picked and focus
/// asserted; Show Desktop is picked and both windows asserted minimised; picked again, both asserted restored.
#[cfg(all(feature = "witness", target_arch = "x86_64", feature = "wc"))]
pub fn selftest() {
    use super::winmenu;
    const A: u64 = 0xD51;
    const B: u64 = 0xD52;
    let ida = wm::wl_fixture_mint(A, b"wl-a");
    let idb = wm::wl_fixture_mint(B, b"wl-b");
    let done = |ids: bool| {
        if ids {
            wm::close_owner(A);
            wm::close_owner(B);
        }
    };
    if ida == wm::WIN_NONE || idb == wm::WIN_NONE {
        serial_println!(":: WINDOWLIST: SKIP (window table full) ::");
        done(true);
        return;
    }
    // Focus ends on A (raised last), so picking B's row is a real change of focus.
    wm::wl_focus(idb);
    wm::wl_focus(ida);
    winmenu::set_app_window(ida, b"wl-a");
    // Open the menu by pressing the Window box.
    let Some((bx, by)) = winmenu::wl_box_center() else {
        serial_println!(":: WINDOWLIST: SKIP (no bar / panel) ::");
        done(true);
        return;
    };
    let opened = winmenu::press_at(bx, by) && winmenu::is_open();
    let nrows = rows().len();
    let mut wr: alloc::vec::Vec<wm::WlRow> = alloc::vec::Vec::new(); // WINDOWCAP-2
    let live = wm::wl_rows(&mut wr);
    let rows_ok = nrows == FIXED_ROWS + live && live >= 2;
    let pick_second = match winmenu::wl_row_center(ID_WIN_BASE + idb) {
        Some((x, y)) => winmenu::press_at(x, y),
        None => false,
    };
    let focused_id = wm::wl_focused().map(|f| f.0).unwrap_or(0);
    let focus_ok = opened && pick_second && focused_id == idb && wm::focus_asid() == B && !winmenu::is_open();
    // Show Desktop, through the menu.
    let open_and_pick = |id: u32| -> bool {
        let Some((x, y)) = winmenu::wl_box_center() else { return false };
        if !winmenu::press_at(x, y) {
            return false;
        }
        match winmenu::wl_row_center(id) {
            Some((rx, ry)) => winmenu::press_at(rx, ry),
            None => false,
        }
    };
    let hid = open_and_pick(ID_SHOW_DESKTOP);
    let live_now = wm::wl_rows(&mut wr);
    let minimised = wr[..live_now].iter().filter(|r| r.minimised).count();
    let all_down = hid && live_now >= 2 && minimised == live_now && desktop_hidden();
    let back = open_and_pick(ID_SHOW_DESKTOP);
    let live_after = wm::wl_rows(&mut wr);
    let restored = back && wr[..live_after].iter().filter(|r| r.id == ida || r.id == idb).all(|r| !r.minimised) && !desktop_hidden();
    let show_desktop_ok = all_down && restored;
    let ok = rows_ok && focus_ok && show_desktop_ok;
    serial_println!(
        ":: WINDOWLIST: rows={} live={} focused={} minimised={} show_desktop_ok={} -> {} ::",
        nrows, live, focused_id, minimised, show_desktop_ok, if ok { "PASS" } else { "FAIL" }
    );
    winmenu::set_app_window(wm::WIN_NONE, b"");
    done(true);
}
