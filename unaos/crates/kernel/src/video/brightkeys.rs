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
//! takes the pending flag, writes the gmux brightness register (only when `gmux_igd` is compiled
//! in; otherwise `gmux_written=0`) and prints the witness.
//!
//! `:: BRIGHTKEYS: key=<up|down> level=<n>/16 gmux_written=<0|1> indicator=1 -> PASS ::`
//! `indicator=1` is measured (`status::bright_item()` answers `Some(level)` right after the key),
//! `gmux_written=1` only when the register write completed.

use crate::video::keymap::Action;
use crate::video::status;
use core::sync::atomic::{AtomicU8, Ordering};

/// Backlight steps (`0..=16`). The gmux register is 16-bit; a step is `level * 0xFFFF / 16`.
pub const STEPS: u8 = 16;
/// Level assumed before the first key (the register is not read back at boot).
const DEFAULT_LEVEL: u8 = 12;

static LEVEL: AtomicU8 = AtomicU8::new(DEFAULT_LEVEL);
/// 0 = nothing pending, 1 = pending `down`, 2 = pending `up` (last key wins).
static PENDING: AtomicU8 = AtomicU8::new(0);

/// The pure step rule: one notch, clamped to `0..=STEPS`.
pub const fn step(level: u8, up: bool) -> u8 {
    if up { if level >= STEPS { STEPS } else { level + 1 } } else if level == 0 { 0 } else { level - 1 }
}

/// Register value for a level.
pub const fn raw_for(level: u8) -> u16 {
    ((level as u32 * 0xFFFF) / STEPS as u32) as u16
}

/// Called by `pal::push_event` for a brightness action. Non-blocking, no port I/O.
pub fn key(act: Action) {
    let up = matches!(act, Action::BrightnessUp);
    let lv = step(LEVEL.load(Ordering::Relaxed), up);
    LEVEL.store(lv, Ordering::Relaxed);
    PENDING.store(if up { 2 } else { 1 }, Ordering::Release);
    status::bright_show(lv);
}

static SELFTESTED: AtomicU8 = AtomicU8::new(0);

/// Desktop service pass: apply a pending step to the gmux and witness it. The FIRST call also runs
/// the boot fixture (an up step and a down step through the real [`key`] path, no gmux write, the
/// indicator cleared afterwards) so a board with no operator hands — QEMU, a bench replay — still
/// states the witness; a real key press prints its own with `gmux_written` measured.
pub fn service() {
    if SELFTESTED.swap(1, Ordering::AcqRel) == 0 {
        for act in [Action::BrightnessUp, Action::BrightnessDown] {
            key(act);
            apply(false);
        }
        status::bright_clear();
    }
    apply(true);
}

fn apply(write: bool) {
    let p = PENDING.swap(0, Ordering::AcqRel);
    if p == 0 {
        return;
    }
    let lv = LEVEL.load(Ordering::Relaxed);
    let written = write && write_gmux(raw_for(lv));
    let indicator = status::bright_item() == Some(lv);
    serial_println!(
        ":: BRIGHTKEYS: key={} level={}/{} gmux_written={} indicator={} -> {} ::",
        if p == 2 { "up" } else { "down" }, lv, STEPS, written as u8, indicator as u8,
        if indicator { "PASS" } else { "FAIL" }
    );
}

#[cfg(all(feature = "gmux_igd", feature = "intel-ivb"))]
fn write_gmux(raw: u16) -> bool {
    crate::drivers::gpu::igpu::gmux_set_brightness(raw)
}
#[cfg(not(all(feature = "gmux_igd", feature = "intel-ivb")))]
fn write_gmux(_raw: u16) -> bool {
    false
}
