//! CHARTER: Kernel — wm (FBDIRECT, rmbp-ledger B536: the registry of direct-framebuffer paints the compositor must learn about)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//!
//! FBDIRECT (rmbp-ledger B536, flight 31 REDBAND + SPLASHWORD). A few painters write straight onto the panel because
//! the thing they report may be holding the compositor (SESSIONWEDGE's PUMPWEDGE band, TESTWATCH's band, the
//! RESOLVEHANG2 OVER line) or because they own the glass before any screen exists (the held splash's stage word).
//! Before this file each of them "cleared" by FILLING its rectangle again, and the compositor's scene never learned
//! those pixels were stale: after flight 31's 2.2 s dongle wedge the red band stayed until the cursor and the app bar
//! overpainted it piecemeal, and boot 2's "Resolving the session" stayed under the login window.
//!
//! One rule, here: a direct paint registers its rect ([`note`]); its END hands the rect back to the compositor
//! ([`clear`]) — `wm::damage_intersecting` (the window half) and `screen::request_present_rect` (the backdrop half),
//! the pair `strip::restore_vacated` / `cursor::repair` use — never a fill; and a screen open hands back every rect
//! still registered ([`expose_all`], `login::open_as` and the desktop build). The panic screen stays direct.
//!
//! The table is one `AtomicU64` per painter (x, y, w, h packed 16 bits each; 0 = nothing on the glass): no lock, no
//! heap, so the watchdog painters keep their "takes nothing" contract on the PAINT. Only the hand-back takes the
//! compositor's two short locks (`TABLE`, `PRESENT_RECTS`), and it runs after the thing that wedged has moved again.

use core::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
use core::sync::atomic::{AtomicU32, AtomicU64};

/// SESSIONWEDGE's PUMPWEDGE band (`sessionwedge.rs` `paint`).
pub const PUMP_WEDGE: usize = 0;
/// TESTWATCH's band (`tests_watch.rs` `paint`).
pub const TEST_WATCH: usize = 1;
/// The held splash's stage word and the OVER band under it (`splash::step_label`).
pub const SPLASH_WORD: usize = 2;
/// The boot watchdog's OVER line straight on the panel (`splash::over_panel`, fed by `fs::bootstep::poll`).
pub const BOOT_OVER: usize = 3;
const SLOTS: usize = 4;
const NAMES: [&str; SLOTS] = ["pumpwedge", "testwatch", "splash-word", "boot-over"];

static RECT: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
/// Rects handed back to the compositor this boot (every [`clear`] and every rect an [`expose_all`] carried).
static HANDED: AtomicU32 = AtomicU32::new(0);

fn pack(x: usize, y: usize, w: usize, h: usize) -> u64 {
    let c = |v: usize| (v.min(0xffff)) as u64;
    (c(x) << 48) | (c(y) << 32) | (c(w) << 16) | c(h)
}

fn unpack(r: u64) -> (usize, usize, usize, usize) {
    (((r >> 48) & 0xffff) as usize, ((r >> 32) & 0xffff) as usize, ((r >> 16) & 0xffff) as usize, (r & 0xffff) as usize)
}

/// A direct paint put `(x, y, w, h)` on the panel for `slot`. Atomics only — safe from a watchdog that may not wait.
#[inline]
pub fn note(slot: usize, x: usize, y: usize, w: usize, h: usize) {
    if slot >= SLOTS || w == 0 || h == 0 {
        return;
    }
    RECT[slot].store(pack(x, y, w, h), Release);
}

/// The compositor repaints its scene over `r`: the windows it meets, then the backdrop. Returns the rect's area.
fn hand(r: u64) -> usize {
    let (x, y, w, h) = unpack(r);
    let _ = crate::video::wm::damage_intersecting(x, y, w, h);
    crate::video::screen::request_present_rect(x, y, w, h);
    HANDED.fetch_add(1, Relaxed);
    w * h
}

/// `slot`'s paint ENDED: its rect goes back to the compositor (a damage, never a fill). Returns whether one was
/// registered. `[fbdirect] clear painter=<name> rect=<x>,<y>,<w>x<h> -> damage`.
pub fn clear(slot: usize) -> bool {
    if slot >= SLOTS {
        return false;
    }
    let r = RECT[slot].swap(0, AcqRel);
    if r == 0 {
        return false;
    }
    let (x, y, w, h) = unpack(r);
    hand(r);
    serial_println!("[fbdirect] clear painter={} rect={},{},{}x{} -> damage handed={}", NAMES[slot], x, y, w, h, HANDED.load(Relaxed));
    true
}

/// A screen opened (`login::open_as`, the desktop build): every rect still registered is handed back once, so the
/// compositor's next pass repaints what any direct painter left on the glass. Returns how many.
/// `[fbdirect] expose why=<why> rects=<n> px=<n> handed=<n>`.
pub fn expose_all(why: &str) -> usize {
    let (mut n, mut px) = (0usize, 0usize);
    for slot in RECT.iter() {
        let r = slot.swap(0, AcqRel);
        if r != 0 {
            px += hand(r);
            n += 1;
        }
    }
    serial_println!("[fbdirect] expose why={} rects={} px={} handed={}", why, n, px, HANDED.load(Relaxed));
    n
}

/// Rects handed back to the compositor this boot — the fixtures' `rects=`.
pub fn handed() -> u32 {
    HANDED.load(Relaxed)
}

/// Whether `slot` has a rect on the glass the compositor has not been handed.
pub fn owed(slot: usize) -> bool {
    slot < SLOTS && RECT[slot].load(Acquire) != 0
}

/// How many of the splash's slots (the stage word, the boot OVER line) are still owed — `tests splash`'s `owed=`.
pub fn owed_splash() -> usize {
    owed(SPLASH_WORD) as usize + owed(BOOT_OVER) as usize
}
