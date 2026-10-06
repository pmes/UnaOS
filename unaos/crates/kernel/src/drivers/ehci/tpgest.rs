// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — driver (the Wellspring report path's gesture stage; the preferences are Principia's
//! `system.trackpad.*`, their rules `prefs_core::trackpad`, edited by the Settings Trackpad tab).
//!
//! TRACKPADPANE (rmbp-ledger B412, MACPARITY row 16). [`shape`] runs once per decoded TYPE2 frame, AFTER
//! `TpCensus::mt_step` turned it into the curve's `(buttons, dx, dy)`, and returns the triple the vendor arm then
//! installs exactly as before (`note_buttons` + one `push_pointer_report`). What it adds, each live from an atomic
//! the Settings pane sets:
//!
//! - **speed** — one gain on the curve's pixels with the remainder carried while the finger stays down
//!   (`prefs_core::trackpad::gain_step`); speed 5 is the flown curve, pixel for pixel.
//! - **two-finger scroll** — two fingers down and no click: the mover's curve dy becomes `Event::Wheel` detents
//!   ([`DETENT_PX`] per detent), sign by `natural_scroll`; no pointer motion.
//! - **secondary click** — the pad's click (`ibt`) whose PRESS edge sees two fingers is `Button(0x02)` until its
//!   release. Latched at the press: a second finger landing on a click already held stays primary — that is
//!   TPDRAG's click-and-hold-with-one, drag-with-the-other (the mover drives the pointer, the click stays down).
//! - **tap to click** — down -> up within [`TAP_MS`], at most [`TAP_TRAVEL_PX`] of travel, no click: one
//!   click (two fingers: secondary).
//! - **three-finger drag** — three fingers down hold the primary button; motion from the mover; any finger lifting
//!   below three releases it.
//!
//! Witness: `[tp] gesture kind=<drag2|scroll|secondary|tap|three-drag> fingers=<n> len=<n> …`, the first
//! [`WIT_MAX`] of each kind per boot (a hand at the glass, never at boot — R80).

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering::Relaxed};

use super::Wsp2Frame;
use crate::pal::Event;
use prefs_core::trackpad as tp;

/// Curve pixels of two-finger travel per wheel detent. [ONE-SOURCE: glass guess — the TPSCALE witness reads
/// 16 px/frame on a brisk stroke, so a brisk scroll is ~2-3 detents per frame; refuted by Peter's hand.]
pub const DETENT_PX: i32 = 6;
/// A tap's longest touch. [ONE-SOURCE: glass guess, the order of the common 150-200 ms tap window.]
pub const TAP_MS: u64 = 180;
/// A tap's most travel, curve pixels summed over both axes. [ONE-SOURCE: glass guess.]
pub const TAP_TRAVEL_PX: i32 = 3;
/// Witness lines per gesture kind per boot.
const WIT_MAX: u8 = 4;

static SPEED: AtomicU8 = AtomicU8::new(tp::SPEED_DEFAULT);
static TAP: AtomicBool = AtomicBool::new(tp::TAP_DEFAULT);
static NATURAL: AtomicBool = AtomicBool::new(tp::NATURAL_DEFAULT);
static SECONDARY: AtomicBool = AtomicBool::new(true);
static THREE: AtomicBool = AtomicBool::new(tp::THREE_DRAG_DEFAULT);

/// The live values: `(speed, tap, natural, secondary, three_drag)`.
pub fn get() -> (u8, bool, bool, bool, bool) {
    (SPEED.load(Relaxed), TAP.load(Relaxed), NATURAL.load(Relaxed), SECONDARY.load(Relaxed), THREE.load(Relaxed))
}
pub fn set_speed(n: u8) { SPEED.store(tp::clamp_speed(n as i64), Relaxed); }
pub fn set_tap(on: bool) { TAP.store(on, Relaxed); }
pub fn set_natural(on: bool) { NATURAL.store(on, Relaxed); }
pub fn set_secondary(on: bool) { SECONDARY.store(on, Relaxed); }
pub fn set_three_drag(on: bool) { THREE.store(on, Relaxed); }

#[derive(Clone, Copy, PartialEq)]
enum Held { None, Primary, Secondary }

struct St {
    touching: bool,
    down_ms: u64,
    travel: i32,
    max_f: u8,
    clicked: bool,
    btn_prev: bool,
    held: Held,
    three_held: bool,
    res: (i32, i32),
    wheel_acc: i32,
    /// Witness lines emitted per kind: drag2, scroll, secondary, tap, three-drag.
    wit: [u8; 5],
}

static ST: crate::sync::Mutex<St> = crate::sync::Mutex::new(St {
    touching: false, down_ms: 0, travel: 0, max_f: 0, clicked: false, btn_prev: false, held: Held::None,
    three_held: false, res: (0, 0), wheel_acc: 0, wit: [0; 5],
});

const KINDS: [&str; 5] = ["drag2", "scroll", "secondary", "tap", "three-drag"];

fn wit(s: &mut St, k: usize, fingers: u8, len: usize, detail: core::fmt::Arguments) {
    if s.wit[k] >= WIT_MAX { return; }
    s.wit[k] += 1;
    serial_println!("[tp] gesture kind={} fingers={} len={} {} (TRACKPADPANE)", KINDS[k], fingers, len, detail);
}

/// One decoded frame after the curve: returns the `(buttons, dx, dy)` the vendor arm installs; pushes the
/// secondary / tap / wheel events itself.
pub(super) fn shape(f: Wsp2Frame, buttons: u8, dx: i32, dy: i32, len: usize, _idx: usize) -> (u8, i32, i32) {
    let now = crate::arch::ms();
    let (speed, tap_on, natural, sec_on, three_on) = get();
    let touching = f.fingers > 0 && f.touch0 != 0;
    let n = if touching { f.fingers } else { 0 };
    let phys = buttons & 0x01 != 0;
    // Up to: a secondary edge, a tap's press + release, one wheel.
    let mut evs: [Option<Event>; 4] = [None; 4];
    let mut ne = 0usize;
    let mut s = ST.lock();
    if touching && !s.touching {
        s.down_ms = now; s.travel = 0; s.max_f = 0; s.clicked = false; s.res = (0, 0); s.wheel_acc = 0;
    }
    if touching {
        s.max_f = s.max_f.max(n);
        s.travel = s.travel.saturating_add(dx.abs() + dy.abs());
    }
    if phys { s.clicked = true; }
    // The pad's click: its kind is decided at the PRESS edge and kept to the release.
    if phys && !s.btn_prev {
        s.held = if sec_on && n >= 2 { Held::Secondary } else { Held::Primary };
        if s.held == Held::Secondary {
            evs[ne] = Some(Event::Button(0x02)); ne += 1;
            wit(&mut s, 2, n, len, format_args!("press=two-finger"));
        }
    } else if !phys && s.btn_prev {
        if s.held == Held::Secondary { evs[ne] = Some(Event::Button(0x00)); ne += 1; }
        s.held = Held::None;
    }
    s.btn_prev = phys;
    let three = three_on && n >= 3 && !phys;
    if three && !s.three_held { wit(&mut s, 4, n, len, format_args!("hold=primary")); }
    s.three_held = three;
    let out_btn = if s.held == Held::Primary || three { 0x01 } else { 0x00 };
    let (mut ox, mut oy) = (0, 0);
    if n >= 2 && !phys && !three && s.held == Held::None {
        // Two-finger scroll: the curve's dy in detents, no pointer motion.
        s.wheel_acc += dy;
        let det = s.wheel_acc / DETENT_PX;
        if det != 0 {
            s.wheel_acc -= det * DETENT_PX;
            let w = tp::wheel(det, natural).clamp(-127, 127);
            evs[ne] = Some(Event::Wheel(w as i8)); ne += 1;
            wit(&mut s, 1, n, len, format_args!("dy={} wheel={} natural={}", dy, w, natural as u8));
        }
    } else {
        let (mut rx, mut ry) = s.res;
        ox = tp::gain_step(dx, speed, &mut rx);
        oy = tp::gain_step(dy, speed, &mut ry);
        s.res = (rx, ry);
        if n >= 2 && s.held == Held::Primary && (dx != 0 || dy != 0) {
            wit(&mut s, 0, n, len, format_args!("click=held dx={} dy={} speed={}", ox, oy, speed));
        }
    }
    if !touching && s.touching {
        let ms = now.saturating_sub(s.down_ms);
        if tap_on && !s.clicked && ms <= TAP_MS && s.travel <= TAP_TRAVEL_PX {
            let b = if sec_on && s.max_f >= 2 { 0x02 } else { 0x01 };
            evs[ne] = Some(Event::Button(b)); ne += 1;
            evs[ne] = Some(Event::Button(0x00)); ne += 1;
            let mf = s.max_f;
            let tr = s.travel;
            wit(&mut s, 3, mf, len, format_args!("ms={} travel={} button={}", ms, tr, b));
        }
    }
    if !touching { s.res = (0, 0); s.wheel_acc = 0; }
    s.touching = touching;
    drop(s);
    for e in evs.iter().take(ne).flatten() {
        match e {
            Event::Wheel(_) => crate::pal::push_event(*e),
            _ => crate::pal::push_pointer_report(None, Some(*e)),
        }
    }
    (out_btn, ox, oy)
}
