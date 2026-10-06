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
//! CHARTER: Kernel — kernel-by-ruling (B443 PERFREVIEW: the budget line; reads the instruments that exist, owns no store)
//!
//! PERFREVIEW (rmbp-ledger B443). The review's witness: ONE line, once, when the services start at login —
//! `[perf] frame_us=<n> key_us=<n> svc_idle_us=<n> boot_s=<n> passes=<n> keys=<n> svcs=<n>` — so the next review reads
//! numbers, not code. Budgets (Mac): a frame 16667 us at 60 Hz, a key 1000 us (R86), boot as flown (24.6–29.9 s).
//! * `frame_us`    — the MEAN compositor pass since boot (`lag::pass` guard; x86 `wc` only, 0 elsewhere).
//! * `key_us`      — the WORST key→echo since boot (`lag::finish`, keys that completed; the setter's and the login's).
//! * `svc_idle_us` — the MEAN render-loop service chain (`lag::seg(S_CONSOLE)`: `console_launch_drain` → the
//!   `quarry::service` desktop chain — what every render pass pays when nothing happened).
//! * `boot_s`      — `:: BOOT: total=` in whole seconds (`bootpace::boot_line`).
//! Nothing here allocates, locks or tests (R80); the hooks are relaxed atomics.
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

static FRAME_SUM_US: AtomicU64 = AtomicU64::new(0);
static FRAME_N: AtomicU64 = AtomicU64::new(0);
static KEY_MAX_US: AtomicU64 = AtomicU64::new(0);
static KEY_N: AtomicU64 = AtomicU64::new(0);
static SVC_SUM_US: AtomicU64 = AtomicU64::new(0);
static SVC_N: AtomicU64 = AtomicU64::new(0);
static BOOT_MS: AtomicU64 = AtomicU64::new(0);
static SAID: AtomicBool = AtomicBool::new(false);

/// `lag::sec_pass`: one compositor pass took `us`.
#[inline]
pub fn note_frame(us: u64) {
    FRAME_SUM_US.fetch_add(us, Relaxed);
    FRAME_N.fetch_add(1, Relaxed);
}

/// `lag::finish`: a key reached the glass `us` after it was queued.
#[inline]
pub fn note_key(us: u64) {
    KEY_MAX_US.fetch_max(us, Relaxed);
    KEY_N.fetch_add(1, Relaxed);
}

/// `lag::seg(S_CONSOLE)`: the render loop's service chain took `us`.
#[inline]
pub fn note_svc(us: u64) {
    SVC_SUM_US.fetch_add(us, Relaxed);
    SVC_N.fetch_add(1, Relaxed);
}

/// `bootpace::boot_line`: the boot's total (counter ticks at `hz`).
pub fn note_boot(raw: u64, hz: u64) {
    if hz != 0 {
        BOOT_MS.store(((raw as u128) * 1000 / hz as u128) as u64, Relaxed);
    }
}

/// The budget line, once (`boot::services_line`, the login's first service pass).
pub fn say() {
    if SAID.swap(true, Relaxed) {
        return;
    }
    let mean = |s: &AtomicU64, n: &AtomicU64| s.load(Relaxed).checked_div(n.load(Relaxed)).unwrap_or(0);
    serial_println!(
        "[perf] frame_us={} key_us={} svc_idle_us={} boot_s={} passes={} keys={} svcs={} (B443: budgets frame 16667 key 1000)",
        mean(&FRAME_SUM_US, &FRAME_N),
        KEY_MAX_US.load(Relaxed),
        mean(&SVC_SUM_US, &SVC_N),
        BOOT_MS.load(Relaxed) / 1000,
        FRAME_N.load(Relaxed),
        KEY_N.load(Relaxed),
        SVC_N.load(Relaxed)
    );
}
