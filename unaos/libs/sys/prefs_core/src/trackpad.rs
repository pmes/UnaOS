// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! TRACKPADPANE (rmbp-ledger B412, MACPARITY row 16) — the Trackpad pane's RULES, one definition both rings link.
//!
//! The keys are Principia's (`system.trackpad.*`, declared in [`crate::schema`]); the kernel's Settings pane edits
//! them and hands the values to the Wellspring gesture stage (`drivers/ehci/tpgest.rs`), which applies them live.
//!
//! - **speed** (1..=10, default 5): ONE gain on the TPSPEED curve's output (`tp_scale`, two-slope 8/3@24, approved
//!   on flight 19). `gain8(5) == 8 == GAIN_DEN`, so the default reproduces today's pixels exactly; the curve's knee
//!   and the ratio of its slopes never move — only its scale. [`gain_step`] carries the sub-pixel remainder so a
//!   slow speed still moves on small deltas.
//! - **natural scroll** (default on): the wheel detent's sign follows the fingers ([`wheel`]).
//! - **tap to click** (default off), **secondary click** (`two-finger` | `off`, default `two-finger`), **three-finger
//!   drag** (default off): booleans the gesture stage reads.

/// The key names (namespace `system`).
pub const KEY_SPEED: &str = "trackpad.speed";
pub const KEY_TAP: &str = "trackpad.tap_to_click";
pub const KEY_NATURAL: &str = "trackpad.natural_scroll";
pub const KEY_SECONDARY: &str = "trackpad.secondary_click";
pub const KEY_THREE_DRAG: &str = "trackpad.three_finger_drag";

pub const SPEED_MIN: u8 = 1;
pub const SPEED_MAX: u8 = 10;
/// The speed whose gain is exactly 1 (today's curve).
pub const SPEED_DEFAULT: u8 = 5;
pub const TAP_DEFAULT: bool = false;
pub const NATURAL_DEFAULT: bool = true;
pub const THREE_DRAG_DEFAULT: bool = false;
/// `system.trackpad.secondary_click`'s spellings; the first is the default.
pub const SECONDARY_CHOICES: &[&str] = &["two-finger", "off"];

/// The gain's denominator: `px = curve * gain8(speed) / GAIN_DEN`.
pub const GAIN_DEN: i32 = 8;
/// Gain numerators for speeds 1..=10 (x0.25 .. x2.5); index 4 (speed 5) is `GAIN_DEN`.
const GAIN8: [i32; 10] = [2, 3, 4, 6, 8, 10, 12, 14, 16, 20];

/// Clamp a stored speed into 1..=10.
pub fn clamp_speed(n: i64) -> u8 {
    n.clamp(SPEED_MIN as i64, SPEED_MAX as i64) as u8
}

/// The gain numerator for `speed` (clamped).
pub fn gain8(speed: u8) -> i32 {
    GAIN8[(clamp_speed(speed as i64) - SPEED_MIN) as usize]
}

/// One axis: the curve's pixels `d` through the speed's gain, carrying the remainder in `res` (toward zero, the
/// remainder keeps its sign). At the default speed `res` stays 0 and the result is `d` exactly. Pure.
pub fn gain_step(d: i32, speed: u8, res: &mut i32) -> i32 {
    let a = d * gain8(speed) + *res;
    let px = a / GAIN_DEN;
    *res = a - px * GAIN_DEN;
    px
}

/// The R75 3-step `system.pointer.speed` (0 slow, 1 normal, 2 fast) as a trackpad speed — read only while
/// `system.trackpad.speed` is unset (its divisors 12/4 and 5/2 were ~x0.7 and ~x1.5 of the curve).
pub fn speed_from_legacy(pointer: i64) -> u8 {
    match pointer { 0 => 4, 2 => 7, _ => SPEED_DEFAULT }
}

/// Wheel detents for a two-finger move of `det` detents in POINTER orientation (positive = fingers moved down).
/// `Event::Wheel` positive = up (toward the top of the content): natural scrolling moves the content WITH the
/// fingers, so fingers down = wheel up; off, the sign flips (fingers down = toward the end). Pure.
pub fn wheel(det: i32, natural: bool) -> i32 {
    if natural { det } else { -det }
}

/// `system.trackpad.secondary_click` as the gesture stage's flag.
pub fn secondary_on(v: &str) -> bool {
    v != "off"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_speed_is_the_flown_curve() {
        let mut r = 0;
        for d in -40..=40 {
            assert_eq!(gain_step(d, SPEED_DEFAULT, &mut r), d);
            assert_eq!(r, 0);
        }
    }

    #[test]
    fn slow_speed_keeps_small_motion_by_the_remainder() {
        let mut r = 0;
        let moved: i32 = (0..8).map(|_| gain_step(1, 1, &mut r)).sum();
        assert_eq!(moved, 2); // 8 px of curve at x0.25
        let mut r = 0;
        let back: i32 = (0..8).map(|_| gain_step(-1, 1, &mut r)).sum();
        assert_eq!(back, -2);
    }

    #[test]
    fn gain_is_monotone_and_clamped() {
        for s in SPEED_MIN..SPEED_MAX {
            assert!(gain8(s) < gain8(s + 1));
        }
        assert_eq!(gain8(0), gain8(1));
        assert_eq!(gain8(99), gain8(10));
        assert_eq!(gain8(SPEED_DEFAULT), GAIN_DEN);
    }

    #[test]
    fn natural_flips_the_wheel() {
        assert_eq!(wheel(2, true), 2);
        assert_eq!(wheel(2, false), -2);
        assert_eq!(speed_from_legacy(1), SPEED_DEFAULT);
        assert!(secondary_on("two-finger") && !secondary_on("off"));
    }
}
