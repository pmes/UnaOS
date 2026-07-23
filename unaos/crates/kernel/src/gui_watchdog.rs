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

//! GUI-CARRY — the app-input watchdog and mode-transition witnesses.
//!
//! ## Why this exists
//!
//! When a full-screen app (`vug`, `pulse`, …) owns the screen, the Pi input router stops forwarding
//! pointer/key events into `GUI_CHANNEL` and leaves them in `pal::EVENT_QUEUE` for the app's own
//! `pump_and_poll` drain (the GUI-CLICK-2 `SCREEN_APP_ACTIVE` gate in `main.rs`). That gate is
//! correct while the app is *live*, but it has no escape hatch: if the app wedges (its drain loop
//! stops making progress) the keyboard is trapped inside a dead app forever — the shell can never be
//! reached again short of a reboot.
//!
//! This module is the escape hatch, and it is deliberately a **self-contained state machine** so it
//! carries no dependency on the input-router internals that live in the off-limits `main.rs` /
//! `pal.rs`. It observes three things through its narrow API — app *enter*, app *exit*, and a
//! liveness *heartbeat* (`note_progress`, bumped by the app's drain each pass) — and answers one
//! question on a cadence: has the active app stopped making progress for longer than the timeout? If
//! so, `poll()` returns `true` and the caller returns input to the shell.
//!
//! ## Witnesses (serial, `[gui]` family)
//!
//! * `[gui] app-enter t=<s>s` — a full-screen app took the screen.
//! * `[gui] app-exit t=<s>s dur=<s>s wedged=<bool>` — it gave the screen back; `dur` is how long it
//!   held it and `wedged` records whether the watchdog fired during the session.
//! * `[gui] watchdog app wedged <n>s … — returning input to shell` — the escape hatch fired
//!   (latched, so it prints once per wedge, not once per poll).
//!
//! All timestamps are monotonic uptime **seconds** from the shared `crate::clock::uptime_secs()`
//! seam — the only public monotonic accessor that needs no cargo feature and works on both arches
//! (aarch64 `CNTPCT`/`CNTFRQ`; x86 invariant TSC once calibrated). Whole-second granularity is ample
//! for a multi-second wedge watchdog and keeps the module off the private `monotonic()` freq seam.
//!
//! ## Concurrency
//!
//! State is three lock-free atomics touched with `Relaxed` ordering. `on_app_enter` /
//! `on_app_exit` run on the input core (around `dispatch_command`); `note_progress` runs on whatever
//! core drains the app; `poll` runs on the status/pump cadence. None of these coordinate any memory
//! beyond the atomics themselves, so `Relaxed` is sufficient and no lock is introduced — safe to call
//! from IRQ-masked contexts.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// How long an active app may make no drain progress before the watchdog returns input to the shell.
/// Whole seconds, matched to the `uptime_secs()` granularity. Five seconds is long enough that a
/// briefly busy-but-live app (a heavy frame, a blocking readback) is never falsely reclaimed, yet
/// short enough that a truly dead app does not strand the keyboard for an unbounded time.
pub const WATCHDOG_TIMEOUT_SECS: u64 = 5;

/// Sentinel stored in `APP_ENTER_SECS` while no full-screen app owns the screen. `u64::MAX` can never
/// be a real uptime reading within any plausible run, so it doubles as the "inactive" flag.
const INACTIVE: u64 = u64::MAX;

/// Uptime seconds captured when the current app took the screen, or `INACTIVE` when the shell owns it.
static APP_ENTER_SECS: AtomicU64 = AtomicU64::new(INACTIVE);

/// Uptime seconds of the most recent `note_progress` heartbeat from the active app's drain loop. The
/// watchdog measures staleness as `now - LAST_PROGRESS_SECS`.
static LAST_PROGRESS_SECS: AtomicU64 = AtomicU64::new(0);

/// Latches `true` the first time the watchdog fires for the current app session, so the wedge witness
/// prints exactly once (not once per poll cadence). Cleared on the next `on_app_enter`.
static WEDGE_LATCHED: AtomicBool = AtomicBool::new(false);

/// Monotonic uptime seconds, or `None` where the arch counter is not yet trustworthy (an
/// uncalibrated x86 TSC early in boot). A `None` reading makes the watchdog no-op rather than guess.
fn now_secs() -> Option<u64> {
    crate::clock::uptime_secs()
}

/// The pure wedge decision, factored out of `poll` so it can be reasoned about (and unit-tested)
/// without touching any global state or clock. Returns `true` iff an app is active and its last
/// progress heartbeat is at least `timeout` seconds old. `saturating_sub` keeps a backwards clock
/// step (should never happen on a monotonic counter) from producing a false wedge.
pub fn wedge_decision(app_active: bool, now: u64, last_progress: u64, timeout: u64) -> bool {
    app_active && now.saturating_sub(last_progress) >= timeout
}

/// Record that a full-screen app has taken the screen. Call this where `SCREEN_APP_ACTIVE` is set
/// `true` (around `dispatch_command` in `handle_key`). Resets the heartbeat and the wedge latch, then
/// emits the `[gui] app-enter` witness.
pub fn on_app_enter() {
    let t = now_secs().unwrap_or(0);
    APP_ENTER_SECS.store(t, Ordering::Relaxed);
    LAST_PROGRESS_SECS.store(t, Ordering::Relaxed);
    WEDGE_LATCHED.store(false, Ordering::Relaxed);
    serial_println!("[gui] app-enter t={}s", t);
}

/// Record that the app gave the screen back. Call this where `SCREEN_APP_ACTIVE` is set `false`.
/// Emits the `[gui] app-exit` witness with the session duration and whether the watchdog fired, then
/// returns the machine to the inactive state.
pub fn on_app_exit() {
    let enter = APP_ENTER_SECS.swap(INACTIVE, Ordering::Relaxed);
    let wedged = WEDGE_LATCHED.swap(false, Ordering::Relaxed);
    let now = now_secs().unwrap_or(0);
    let dur = if enter == INACTIVE { 0 } else { now.saturating_sub(enter) };
    serial_println!("[gui] app-exit t={}s dur={}s wedged={}", now, dur, wedged);
}

/// Liveness heartbeat: the active app's drain loop calls this each pass to prove it is still making
/// progress. Cheap and idempotent; a no-op when no app is active, so it is safe to wire into a shared
/// drain path (`pal::pump_and_poll`) that also runs with no app on screen.
pub fn note_progress() {
    if APP_ENTER_SECS.load(Ordering::Relaxed) == INACTIVE {
        return;
    }
    if let Some(s) = now_secs() {
        LAST_PROGRESS_SECS.store(s, Ordering::Relaxed);
    }
}

/// `true` while a full-screen app owns the screen (mirrors `SCREEN_APP_ACTIVE`, but derived from this
/// module's own state so the watchdog is self-contained).
pub fn is_app_active() -> bool {
    APP_ENTER_SECS.load(Ordering::Relaxed) != INACTIVE
}

/// The watchdog tick. Call on a cadence (the status timer / USB pump). Returns `true` iff the active
/// app has made no drain progress for at least `WATCHDOG_TIMEOUT_SECS` — the caller's cue to return
/// input to the shell (clear `SCREEN_APP_ACTIVE`, so the router resumes forwarding into `GUI_CHANNEL`).
/// The wedge witness is latched to one print per wedge; the returned verdict is not latched, so the
/// caller may keep reclaiming input for as long as the app stays wedged.
pub fn poll() -> bool {
    let enter = APP_ENTER_SECS.load(Ordering::Relaxed);
    if enter == INACTIVE {
        return false;
    }
    let now = match now_secs() {
        Some(s) => s,
        None => return false,
    };
    let last = LAST_PROGRESS_SECS.load(Ordering::Relaxed);
    if wedge_decision(true, now, last, WATCHDOG_TIMEOUT_SECS) {
        if !WEDGE_LATCHED.swap(true, Ordering::Relaxed) {
            serial_println!(
                "[gui] watchdog app wedged {}s (no drain since t={}s, now t={}s) — returning input to shell",
                now.saturating_sub(last),
                last,
                now
            );
        }
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn live_app_is_not_wedged() {
        // progress 1s ago, timeout 5s -> healthy
        assert!(!wedge_decision(true, 100, 99, WATCHDOG_TIMEOUT_SECS));
    }

    #[test]
    fn stalled_app_wedges_at_timeout() {
        // exactly at the timeout boundary -> wedged
        assert!(wedge_decision(true, 105, 100, WATCHDOG_TIMEOUT_SECS));
        // past it -> wedged
        assert!(wedge_decision(true, 200, 100, WATCHDOG_TIMEOUT_SECS));
    }

    #[test]
    fn inactive_never_wedges() {
        assert!(!wedge_decision(false, 1_000, 0, WATCHDOG_TIMEOUT_SECS));
    }

    #[test]
    fn backwards_clock_does_not_false_wedge() {
        // now < last (should be impossible on a monotonic counter) -> saturates to 0, healthy
        assert!(!wedge_decision(true, 90, 100, WATCHDOG_TIMEOUT_SECS));
    }
}
