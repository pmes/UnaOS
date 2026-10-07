// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — driver (the Wellspring vendor lane's sub-pixel curve stage; the gain after it is Principia's
//! `system.trackpad.speed`, applied by `tpgest`).
//!
//! FINEMOTION (rmbp-ledger B496, flight 27: "once i try to move with accuracy it basically will not move").
//! `tp_scale` turned each frame's raw sensor delta into pixels by dividing TOWARD ZERO with no remainder, so a
//! slow, accurate finger — 1..7 Wellspring units per frame — produced 0 px on every frame, forever. [`step`]
//! evaluates the SAME curve (TPSCALE's divisor below TPSPEED's knee, its steeper divisor above) in Q8 sub-pixels
//! and carries the remainder per axis while the finger stays down: every delta moves the pointer by the finger,
//! whole pixels out as soon as the travel adds up to one.
//!
//! Witness: `[ptr] delta route=… in=… scaled=… moved=… why=…` for the first [`WIT_MAX`] deltas of a boot, then one
//! `[ptr] fine …` summary per second while deltas flow (driven by a hand on the pad, never at boot — R80).
//! `tests finemotion` drives the lane: `:: FINEMOTION: one_count_moved=… ten_count_moved=… floor=… stall_s=… ::`.

use core::sync::atomic::{AtomicU32, AtomicU64, Ordering::Relaxed};

/// Sub-pixel resolution of the carried remainder (Q8).
pub(super) const Q: i32 = 256;
/// Per-delta lines a boot prints before the per-second summary takes over.
const WIT_MAX: u32 = 20;
/// The summary's period.
const SUM_MS: u64 = 1000;

/// The curve in Q8 pixels for one raw (clamped) sensor delta, sign preserved. Same shape as `tp_scale`.
pub(super) fn curve_q8(raw: i32) -> i32 {
    let a = raw.abs();
    let k = super::TP_MT_CURVE_KNEE;
    let q = if a <= k { a * Q / super::tp_div_low() } else { k * Q / super::tp_div_low() + (a - k) * Q / super::tp_div_high() };
    if raw < 0 { -q } else { q }
}

/// One axis: the delta's Q8 pixels plus the carried remainder; whole pixels out, `|res| < Q` kept.
pub(super) fn step(raw: i32, res: &mut i32) -> i32 {
    let t = *res + curve_q8(raw);
    let px = t / Q;
    *res = t - px * Q;
    px
}

static WIT_N: AtomicU32 = AtomicU32::new(0);
static N: AtomicU32 = AtomicU32::new(0);
static MOVED: AtomicU32 = AtomicU32::new(0);
static CARRIED: AtomicU32 = AtomicU32::new(0);
static LOST: AtomicU32 = AtomicU32::new(0);
static REL: AtomicU32 = AtomicU32::new(0);
static VENDOR: AtomicU32 = AtomicU32::new(0);
static LOGGED: AtomicU32 = AtomicU32::new(0);
static LAST_MS: AtomicU64 = AtomicU64::new(0);

/// Q8 as a signed decimal with two places (`-0.37`).
struct Q8(i32);
impl core::fmt::Display for Q8 {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let a = self.0.unsigned_abs();
        write!(f, "{}{}.{:02}", if self.0 < 0 { "-" } else { "" }, a / Q as u32, (a % Q as u32) * 100 / Q as u32)
    }
}

/// Charge one decoded report on the pointer lane. `inp` is the report's raw delta (sensor units on the vendor lane,
/// counts on the legacy one), `q` the Q8 pixels it added, `out` the whole pixels installed, `fingers` the frame's
/// count (two fingers is scroll, not pointer motion).
pub(super) fn note(idx: usize, vendor: bool, inp: (i32, i32), q: (i32, i32), out: (i32, i32), fingers: u8) {
    if vendor { VENDOR.fetch_add(1, Relaxed); } else { REL.fetch_add(1, Relaxed); }
    if inp == (0, 0) {
        return;
    }
    let moved = out != (0, 0);
    let why = if moved {
        "px"
    } else if fingers >= 2 {
        "two-finger"
    } else if q != (0, 0) {
        "carried"
    } else {
        "lost"
    };
    N.fetch_add(1, Relaxed);
    match why {
        "px" => { MOVED.fetch_add(1, Relaxed); }
        "carried" => { CARRIED.fetch_add(1, Relaxed); }
        "lost" => { LOST.fetch_add(1, Relaxed); }
        _ => {}
    }
    if WIT_N.load(Relaxed) < WIT_MAX {
        WIT_N.fetch_add(1, Relaxed);
        serial_println!(
            "[ptr] delta route={} in={},{} scaled={},{} moved={} why={} (FINEMOTION: every delta moves the pointer; sub-pixel travel is carried, never floored)",
            if vendor { "vendor" } else { "legacy" }, inp.0, inp.1, Q8(q.0), Q8(q.1), moved as u8, why
        );
    }
    summary_tick(idx);
}

/// The per-second summary, only while deltas flow (a census that has not moved says nothing).
fn summary_tick(idx: usize) {
    let now = crate::arch::ms();
    let n = N.load(Relaxed);
    if n == LOGGED.load(Relaxed) || now.wrapping_sub(LAST_MS.load(Relaxed)) < SUM_MS {
        return;
    }
    LAST_MS.store(now, Relaxed);
    LOGGED.store(n, Relaxed);
    let lost = LOST.load(Relaxed);
    serial_println!(
        "[ptr] fine ep={} n={} moved={} carried={} lost={} floor={} rel={} vendor={} -> {} (route evidence: reports by stream since boot)",
        idx, n, MOVED.load(Relaxed), CARRIED.load(Relaxed), lost, if lost == 0 { "none" } else { "LOST" },
        REL.load(Relaxed), VENDOR.load(Relaxed), if lost == 0 { "OK" } else { "FAIL" }
    );
}

/// `tests finemotion` — 200 one-count and 20 ten-count deltas through `TpCensus::mt_step` (the vendor lane, a
/// fixture census, not the live endpoint's), and a one-count id-0x02 report through `trackpad_dispatch` (the
/// legacy lane). A one-count delta "moved" when the lane's position (whole pixels out plus the carried
/// remainder) advanced by exactly its curve value; a ten-count delta when it put at least one whole pixel out.
/// `floor=none` when the whole pixels out plus the remainder equal the input's Q8 sum (nothing quantised away).
pub(super) fn selftest() {
    let t0 = crate::arch::ms();
    let mut c = super::TpCensus::EMPTY;
    c.latched = true;
    let fr = |x: i32, touch: i32| super::Wsp2Frame { fingers: if touch != 0 { 1 } else { 0 }, button: 0, x0: x, y0: 0, touch0: touch, x1: 0, y1: 0, touch1: 0 };
    let (mut one, mut ten, mut floor_ok) = (0u32, 0u32, true);
    // One-count: baseline, then 200 frames of +1 unit.
    let _ = c.mt_step(fr(0, 1));
    let (mut px_sum, mut want_q) = (0i32, 0i32);
    for k in 1..=200 {
        let before = px_sum * Q + c.mt_res.0;
        let (_, dx, _, _) = c.mt_step(fr(k, 1));
        px_sum += dx;
        if px_sum * Q + c.mt_res.0 - before == curve_q8(1) && curve_q8(1) > 0 { one += 1; }
        want_q += curve_q8(1);
    }
    floor_ok &= px_sum * Q + c.mt_res.0 == want_q;
    // Lift (the remainder resets), re-touch, then 20 frames of +10.
    let _ = c.mt_step(fr(0, 0));
    floor_ok &= c.mt_res == (0, 0);
    let _ = c.mt_step(fr(0, 1));
    let (mut px10, mut want10) = (0i32, 0i32);
    for k in 1..=20 {
        let (_, dx, _, _) = c.mt_step(fr(k * 10, 1));
        px10 += dx;
        want10 += curve_q8(10);
        if dx >= 1 { ten += 1; }
    }
    floor_ok &= px10 * Q + c.mt_res.0 == want10;
    // The legacy lane: one count in, one pixel out.
    let legacy_ok = super::trackpad_dispatch(&[super::TRACKPAD_REPORT_ID, 0, 1, 0, 0, 0, 0, 0])
        == super::TpRoute::Rel { buttons: 0, dx: 1, dy: 0 };
    let stall_s = crate::arch::ms().saturating_sub(t0) / 1000;
    let pass = one == 200 && ten == 20 && floor_ok && legacy_ok && stall_s == 0;
    serial_println!(
        ":: FINEMOTION: one_count_moved={}/200 ten_count_moved={}/20 floor={} stall_s={} -> {} ::",
        one, ten, if floor_ok && legacy_ok { "none" } else { "FLOORED" }, stall_s, if pass { "PASS" } else { "FAIL" }
    );
}
