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

//! CHARTER: Kernel — kernel-by-ruling (R88 FLIGHTRING B400: the console as the log viewer — scroll-back over `boot_ring`, `tests flightring`)
//!
//! FLIGHTRING (rmbp-ledger B400). The console window reads the boot's text from the ONE ring (`boot_ring`, R79):
//! the prefill (`loginfurn::console_prefill`) paints the live tail; this module scrolls the window back through the
//! whole ring — the wheel over the console, and Shift+PgUp/PgDn, Cmd/Ctrl+Home/End while it holds focus — by
//! re-rendering the window from the ring (clear + replay under one present). No second store: the view is an offset
//! in lines from the ring's newest line. `[console] scroll back=<n> lines=<l> top=<0|1>`.
//!
//! `tests flightring` (R80: on demand, never at boot) writes a heap ring of the LIVE shape (the 64 KiB pinned head
//! and the live rolling size) past its capacity — the boot's own log is not overwritten with filler — and checks
//! the pinned head, the marker and the newest line; then prints a token and reads it back as the live ring's tail.
//! `:: FLIGHTRING: rolling_kib=<n> pinned_kib=64 tail_live=1 prefill_lines=<n> wrapped=1 -> PASS ::`

use core::sync::atomic::{AtomicUsize, Ordering::Relaxed};

/// The console grid's rows (from the prefill).
static ROWS: AtomicUsize = AtomicUsize::new(0);
/// The view's offset in lines back from the newest line (0 = the live tail).
static BACK: AtomicUsize = AtomicUsize::new(0);
/// The last prefill's painted lines (`usize::MAX` = the console was not opened this boot).
static PREFILL_LINES: AtomicUsize = AtomicUsize::new(usize::MAX);

/// `loginfurn::console_prefill`: the window's grid height; a fresh console is at the tail.
pub fn note_rows(rows: usize) {
    ROWS.store(rows, Relaxed);
    BACK.store(0, Relaxed);
}

/// `loginfurn::console_prefill`: what it painted.
pub fn note_prefill(lines: usize) {
    PREFILL_LINES.store(lines, Relaxed);
}

/// Lines per wheel detent.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
const WHEEL_LINES: isize = 3;

/// The console's scroll door, asked from `quarry::key_route`'s door chain on both arches. `true` when consumed:
/// a wheel detent over the console window, or a scroll action while the console holds focus.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub fn console_route(ev: crate::pal::Event) -> bool {
    use crate::video::{fbcon, keymap::Action, wm};
    let id = fbcon::console_win();
    if id == wm::WIN_NONE {
        return false;
    }
    let rows = ROWS.load(Relaxed).max(1);
    let page = rows.saturating_sub(1).max(1) as isize;
    let delta: isize = match ev {
        crate::pal::Event::Wheel(d) => {
            if d == 0 {
                return false;
            }
            // Scroll-under-pointer, as Quarry's wheel: the hit test says whose detent it is.
            let Some(i) = crate::video::panel_info_nonblocking() else { return false };
            let (x, y) = crate::pal::cursor::pos(i.width as i32, i.height as i32);
            match wm::hit_test(x, y) {
                Some((w, _, _)) if w == id => {}
                _ => return false,
            }
            d as isize * WHEEL_LINES // positive = wheel away = further back
        }
        crate::pal::Event::Action(a) => {
            if wm::focus_asid() != wm::KERNEL_OWNER_CONSOLE {
                return false;
            }
            match a {
                Action::ScrollPageUp => page,
                Action::ScrollPageDown => -page,
                Action::ScrollTop => isize::MAX / 4,
                Action::ScrollBottom => isize::MIN / 4,
                _ => return false,
            }
        }
        _ => return false,
    };
    scroll_by(delta, rows);
    true
}

/// Move the view `delta` lines (positive = back) and re-render the window from the ring.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
fn scroll_by(delta: isize, rows: usize) {
    use crate::video::fbcon;
    let old = BACK.load(Relaxed);
    let mut want = (old as isize).saturating_add(delta).max(0) as usize;
    let Some(mut t) = crate::boot_ring::tail(rows, want) else { return };
    // At the top, keep the window full: the first screen is the view's first `rows` lines.
    if t.at_top && t.lines < rows && t.back > 0 {
        want = t.back.saturating_sub(rows - t.lines);
        match crate::boot_ring::tail(rows, want) {
            Some(t2) => t = t2,
            None => return,
        }
    }
    let back = t.back;
    if back == old {
        return; // a detent spent on a bound repaints nothing
    }
    BACK.store(back, Relaxed);
    let was = fbcon::console_present_suspended();
    fbcon::console_present_suspend(true);
    fbcon::clear();
    let (lines, _painted) = crate::loginfurn::console_replay(&t.bytes);
    if !was {
        fbcon::console_present_suspend(false);
    }
    serial_println!(
        "[console] scroll back={} lines={} top={} ring_bytes={} wrapped={} (FLIGHTRING: the window re-rendered from the ring; back=0 is the live tail)",
        back, lines, t.at_top as u32, t.total, t.wrapped as u32
    );
}

/// Register `tests flightring` once (folded into `loginfurn::ensure_tests`).
pub fn ensure_tests() {
    static DONE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
    if !DONE.swap(true, core::sync::atomic::Ordering::AcqRel) {
        crate::tests::register("flightring", flightring_selftest);
    }
}

/// `tests flightring`.
pub fn flightring_selftest() {
    use crate::boot_ring::{HeapRing, PINNED_CAP};
    let kib = crate::boot_ring::rolling_kib();
    let cap = kib * 1024;
    let Some(mut r) = HeapRing::new(PINNED_CAP, cap) else {
        serial_println!(":: FLIGHTRING: rolling_kib={} pinned_kib={} -> SKIP reason=heap (a test ring of the live shape could not be allocated) ::", kib, PINNED_CAP / 1024);
        return;
    };
    // Write past N: the pinned head fills, the rolling part wraps at least once.
    let mut i = 0u64;
    while r.core.total() <= (cap + PINNED_CAP + 4096) as u64 {
        let line = alloc::format!("flightring-test line {} ................................................\n", i);
        r.append(line.as_bytes());
        i += 1;
    }
    let newest = r.tail(1, 0);
    let newest_ok = core::str::from_utf8(&newest.bytes).map_or(false, |s| s.starts_with(&alloc::format!("flightring-test line {} ", i - 1)));
    let whole = r.tail(usize::MAX, 0);
    let head_ok = whole.bytes.starts_with(b"flightring-test line 0 ") && whole.at_top;
    let marker_ok = !whole.joined
        && whole.bytes.windows(26).any(|w| w == b":: FLIGHTRING: ---- pinned");
    let wrapped = r.core.wrapped(cap);
    drop(whole);
    drop(r);
    // The live ring: a token printed now is its newest line.
    let tok = crate::arch::ms();
    serial_println!("[flightring] live token={}", tok);
    let needle = alloc::format!("[flightring] live token={}", tok);
    let mut tail_live = false;
    for _ in 0..8 {
        if let Some(t) = crate::boot_ring::tail(1, 0) {
            if t.bytes.windows(needle.len()).any(|w| w == needle.as_bytes()) {
                tail_live = true;
                break;
            }
        }
        core::hint::spin_loop();
    }
    let rows = match ROWS.load(Relaxed) {
        0 => 16,
        n => n,
    };
    let prefill = match PREFILL_LINES.load(Relaxed) {
        usize::MAX => crate::boot_ring::tail(rows, 0).map_or(0, |t| t.lines),
        n => n,
    };
    let live_wrapped = crate::boot_ring::tail(0, 0).map_or(false, |t| t.wrapped);
    let pass = kib >= crate::boot_ring::FLOOR_ROLL / 1024 && wrapped && newest_ok && head_ok && marker_ok && tail_live && prefill > 0;
    if !pass {
        serial_println!(
            ":: FLIGHTRING: reason=floor={} wrapped={} newest={} head={} marker={} tail_live={} prefill={} ::",
            kib >= crate::boot_ring::FLOOR_ROLL / 1024, wrapped, newest_ok, head_ok, marker_ok, tail_live, prefill
        );
    }
    serial_println!(
        ":: FLIGHTRING: rolling_kib={} pinned_kib={} tail_live={} prefill_lines={} wrapped={} live_wrapped={} test_lines={} -> {} ::",
        kib,
        PINNED_CAP / 1024,
        tail_live as u32,
        prefill,
        wrapped as u32,
        live_wrapped as u32,
        i,
        if pass { "PASS" } else { "FAIL" }
    );
}

/// WIREDIET (rmbp-ledger B461, PERFREVIEW F5): the wire's diet, once, at the desktop (`boot::ignite`) —
/// every `_print` so far (`serial_ring::SUBMITTED`) over the boot's uptime, so the next review reads the rate
/// in one line instead of counting a capture. `[flightring] diet lines=<n> up_ms=<ms> per_min=<n>`.
pub fn diet_line() {
    let lines = crate::serial_ring::SUBMITTED.load(core::sync::atomic::Ordering::Relaxed);
    let up_ms = crate::arch::ms().max(1);
    serial_println!("[flightring] diet lines={} up_ms={} per_min={}", lines, up_ms, lines.saturating_mul(60_000) / up_ms);
}

