// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — wm
//!
//! POINTER CAPTURE (PREFSUI, rmbp-ledger B389, R93: "prefs sliders don't slide you just have to click to set" /
//! "grab-n-slide is super sluggish" / "grab and slide broken"). Flight 24's wire showed a drag as separate PRESSES
//! a second apart (`[backlight] slider click pos=16%` 13:12:14, `pos=35%` 13:12:16 …): a kernel window's controls
//! saw only the press edge, never a motion sample or the release.
//!
//! The seam: a press that lands on a draggable control CAPTURES the pointer ([`begin`] with the holder's motion
//! and release functions). While captured, every pointer report the drain routes reaches the holder at the live
//! cursor ([`motion`], fed from the x86 drain's tail `wc_route_tail` beside TERMSEL2's held-press arm) and the
//! primary release ends it ([`release`], fed from the release edge beside `termsel::pointer_release`). One capture
//! at a time; a second [`begin`] replaces the first (its release runs first, so a holder always sees its end).
//! Idle cost: one atomic load per pointer report. Any future drag (MACPARITY row 18) is another holder.
//!
//! Owed: aarch64's routers are byte-identity bound (`kernel8.img`); nothing feeds this module there yet, so a Pi
//! slider stays click-to-set.

use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// A holder's callback, in PANEL coordinates.
pub type PointerFn = fn(i32, i32);

static HELD: AtomicBool = AtomicBool::new(false);
static ON_MOTION: AtomicUsize = AtomicUsize::new(0);
static ON_RELEASE: AtomicUsize = AtomicUsize::new(0);

fn call(slot: &AtomicUsize, x: i32, y: i32) {
    let f = slot.load(Ordering::Acquire);
    if f != 0 {
        // SAFETY: only `begin` stores into these slots, always a valid `PointerFn` (`fn(i32, i32)`).
        let f: PointerFn = unsafe { core::mem::transmute::<usize, PointerFn>(f) };
        f(x, y);
    }
}

/// Capture the pointer for a pressed control: `motion` gets every report while the button is held, `release`
/// the release. Called from the holder's press arm (inside the click router).
pub fn begin(motion: PointerFn, release: PointerFn) {
    if HELD.swap(false, Ordering::AcqRel) {
        let (x, y) = cursor();
        call(&ON_RELEASE, x, y);
    }
    ON_MOTION.store(motion as usize, Ordering::Release);
    ON_RELEASE.store(release as usize, Ordering::Release);
    HELD.store(true, Ordering::Release);
}

/// Is a press held by a capture? One relaxed load.
#[inline]
pub fn held() -> bool {
    HELD.load(Ordering::Relaxed)
}

/// A pointer report while captured: the holder's motion at the live cursor `(x, y)`.
pub fn motion(x: i32, y: i32) {
    if held() {
        call(&ON_MOTION, x, y);
    }
}

/// The primary release: ends the capture and hands the holder its release. Any other release is one swap.
pub fn release(x: i32, y: i32) {
    if HELD.swap(false, Ordering::AcqRel) {
        call(&ON_RELEASE, x, y);
    }
}

/// Drop a capture without its release (the holder's window closed under the press).
pub fn cancel() {
    HELD.store(false, Ordering::Release);
}

fn cursor() -> (i32, i32) {
    match crate::video::panel_info_nonblocking() {
        Some(i) => crate::pal::cursor::pos(i.width as i32, i.height as i32),
        None => (0, 0),
    }
}

/// DROPTYPES (rmbp-ledger B477) — aarch64's feed: the focused-app drain in `main.rs` hands every event here (after
/// it moved the cursor), and the shell path's `render_service` its motion arms (after the cursor moved there), so a
/// capture held on the Pi (a drag, a slider) sees motion at the live cursor; the shell drain's release goes through
/// [`feed_release`]. One relaxed load when nothing is held.
pub fn feed(ev: crate::pal::Event) {
    if !held() {
        return;
    }
    match ev {
        crate::pal::Event::Mouse { .. } | crate::pal::Event::MouseAbsolute { .. } => {
            let (x, y) = cursor();
            motion(x, y);
        }
        crate::pal::Event::Button(mask) if mask & 0x01 == 0 => {
            let (x, y) = cursor();
            release(x, y);
        }
        _ => {}
    }
}

/// The shell-focus drain's half: the primary release only (its motion reaches [`feed`] in `render_service`).
pub fn feed_release(ev: crate::pal::Event) {
    if held() && matches!(ev, crate::pal::Event::Button(m) if m & 0x01 == 0) {
        let (x, y) = cursor();
        release(x, y);
    }
}
