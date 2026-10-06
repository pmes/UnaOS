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

//! BRIGHTKEYS — the brightness keys (F1/F2) step the panel backlight through the gmux, and the
//! menu bar shows the level for 1.5 s.
//!
//! Two halves, split by context. [`key`] runs where the HID decoder pushes its `Action` (interrupt
//! or device-service context): it only moves the level, raises a pending flag and starts the bar's
//! indicator — atomics, no port I/O, no heap. [`service`] runs on the desktop service pass: it
//! takes the pending flag, writes the level through `backlight::set_level_via` (BRIGHTFLOOR: the one
//! writer — floor, gmux register, readback) and prints the witness.
//!
//! `:: BRIGHTKEYS: key=<up|down> level=<n>/16 gmux_written=<0|1> indicator=1 -> PASS ::`
//! `indicator=1` is measured (`status::bright_item()` answers `Some(level)` right after the key),
//! `gmux_written=1` only when the register READ BACK the value written (BRIGHTFLOOR).

use crate::video::keymap::Action;
use crate::video::status;
use core::sync::atomic::{AtomicU8, Ordering};

/// Backlight steps (`0..=16` in the step rule; the lit range is `backlight::FLOOR..=STEPS`).
pub const STEPS: u8 = crate::video::backlight::STEPS;

/// 0 = nothing pending, 1 = pending `down`, 2 = pending `up` (last key wins).
static PENDING: AtomicU8 = AtomicU8::new(0);

/// The pure step rule: one notch, clamped to `0..=STEPS`. (The FLOOR is applied by the caller through
/// `backlight::stage`/`set_level_via` — the rule stays the plain notch the aarch64 checks pin.)
pub const fn step(level: u8, up: bool) -> u8 {
    if up { if level >= STEPS { STEPS } else { level + 1 } } else if level == 0 { 0 } else { level - 1 }
}

/// Called by `pal::push_event` for a brightness action. Non-blocking, no port I/O: the level is STAGED
/// (BRIGHTFLOOR: clamped to the floor — Down at level 1 stays at 1) and the desktop pass writes it.
pub fn key(act: Action) {
    let up = matches!(act, Action::BrightnessUp);
    // GLASSLAG M3 (B370): the step is judged against the register's known value — a Down that would not
    // darken (the panel already at or below the floor) writes nothing; see `backlight::next_level`.
    crate::video::bezel::arm(crate::video::bezel::K_BRIGHT); // BEZEL (B405): the bezel shows on every press, at the limit too
    let Some(lv) = crate::video::backlight::stage_step(up) else {
        status::bright_show(crate::video::backlight::level());
        return;
    };
    PENDING.store(if up { 2 } else { 1 }, Ordering::Release);
    status::bright_show(lv);
}

/// Desktop service pass: apply a pending step through THE backlight writer and witness it. R80: the
/// boot fixture that used to run on the first call is now part of `tests brightfloor` ([`selftest`]).
pub fn service() {
    crate::video::backlight::seed_from_hw(); // GLASSLAG M3 (B370): once — the panel's own level, before the first step
    apply(true);
    crate::video::bezel::ensure_registered(); // BEZEL (B405): `tests bezel`, registered on the desktop pass (R80)
    crate::video::bezel::service(); // after the write: the bezel reads the register's readback
}

/// The BRIGHTKEYS key-path fixture (an up step and a down step through the real [`key`] path, no
/// register write, the indicator cleared afterwards). Called from `backlight::selftest`.
pub fn selftest() {
    let prev = crate::video::backlight::level();
    for act in [Action::BrightnessUp, Action::BrightnessDown] {
        key(act);
        apply(false);
    }
    status::bright_clear();
    crate::video::bezel::arm(crate::video::bezel::K_NONE); // BEZEL: the fixture's keys do not flash the bezel
    let _ = crate::video::backlight::stage(prev);
}

fn apply(write: bool) {
    let p = PENDING.swap(0, Ordering::AcqRel);
    if p == 0 {
        return;
    }
    let lv = crate::video::backlight::level();
    // BRIGHTFLOOR: `gmux_written` is the READBACK verdict now (the register holds the value written,
    // nonzero), not "the transaction completed".
    let written = write && crate::video::backlight::set_level_via(lv, "keys").on;
    let indicator = status::bright_item() == Some(lv);
    serial_println!(
        ":: BRIGHTKEYS: key={} level={}/{} gmux_written={} indicator={} -> {} ::",
        if p == 2 { "up" } else { "down" }, lv, STEPS, written as u8, indicator as u8,
        if indicator { "PASS" } else { "FAIL" }
    );
}

/// SETTINGS (R75): the current level (`backlight::FLOOR..=STEPS`).
pub fn level() -> u8 { crate::video::backlight::level() }
