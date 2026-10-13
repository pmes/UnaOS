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
//!
//! POINTERSCALE (rmbp-ledger B540, flight 32: "the harder i try to make the mouse move where i want the less it does
//! so"). Three defects on the vendor lane, read from `[ptr]`/`[tp] mt`: (1) the two-slope curve ran PER AXIS, so a
//! diagonal's longer leg crossed the knee alone and the stroke bent toward it (`in=-18,-51 scaled=-2.25,-12.00` is
//! not x÷8/y÷4 but x below the knee, y above it) — [`step2`] now evaluates the curve ONCE on the frame's vector
//! magnitude and applies that one gain to both legs: isotropic, the stroke's direction kept; (2) the scale was a
//! guessed constant (8 units/px) — it is now derived from the pad's RANGE ([`range_note`]: the measured x extent,
//! seeded with flight 32's own `-3243..4069`, widened by what the live pad reports, capped at twice the seed) as one
//! slow pad-width stroke = [`PAD_PX`] pixels, so slow, deliberate travel lands instead of sitting in the carry;
//! (3) mod.rs' `TP_MT_MAX_STEP` clamp (128 units per frame per axis) put a ceiling of curve(128) = 37.67 px on every
//! frame — the `[tp] mt … dx=±37/38` saturation — so a faster finger moved the pointer no faster, and a burst after a
//! stream hole or a re-touch with no lift frame was clamped into a fixed diagonal hop. The clamp is gone; a frame
//! whose step is a JUMP ([`is_jump`]: more than a quarter of the range, or the first after a [`JUMP_GAP_MS`] silence)
//! re-baselines instead (counted `rebase=`). Wire: `[ptr] delta … div=<units per px> range=<span>`, `[ptr] fine …
//! rebase=<n> div=… range=x<min>..<max>,y<min>..<max>`, and `tests pointerfeel`'s `scale=` / `carried=`.

use core::sync::atomic::{AtomicI32, AtomicU32, AtomicU64, Ordering::Relaxed};

/// Sub-pixel resolution of the carried remainder (Q8).
pub(super) const Q: i32 = 256;
/// Per-delta lines a boot prints before the per-second summary takes over.
const WIT_MAX: u32 = 20;
/// The summary's period.
const SUM_MS: u64 = 1000;

/// POINTERSCALE: pixels one slow (below-knee) pad-width stroke moves before the Principia gain — the panel's
/// logical width (2880 px glass at 2x), so one unhurried stroke across the pad crosses the screen once. TPSCALE's
/// flown 8 units/px over flight 32's 7312-unit x span was 914 px: every slow frame of 1..7 units sat in the carry.
pub(super) const PAD_PX: i32 = 1440;
/// POINTERSCALE: the seed range — flight 32's own `[tp] mt` extent (f32-boot1: x -3243 at 04:23:59, 4069 at
/// 04:32:46; y -141 at 04:22:40, 6165 at 04:25:02). The live pad's reports only WIDEN it ([`range_note`]).
const SEED_X: (i32, i32) = (-3243, 4069);
const SEED_Y: (i32, i32) = (-141, 6165);
/// A coordinate beyond this magnitude is garbled and never widens the range.
const COORD_SANE: i32 = 16_384;
/// POINTERSCALE: a frame after this much stream silence re-baselines (a hole or an unannounced re-touch, not motion).
pub(super) const JUMP_GAP_MS: u64 = 250;

static X_MIN: AtomicI32 = AtomicI32::new(SEED_X.0);
static X_MAX: AtomicI32 = AtomicI32::new(SEED_X.1);
static Y_MIN: AtomicI32 = AtomicI32::new(SEED_Y.0);
static Y_MAX: AtomicI32 = AtomicI32::new(SEED_Y.1);
static REBASE: AtomicU32 = AtomicU32::new(0);

/// POINTERSCALE: one touching frame's raw sensor position into the measured range (live endpoint only — the
/// fixtures' censuses never call it). Widen-only.
pub(super) fn range_note(x: i32, y: i32) {
    if x.abs() > COORD_SANE || y.abs() > COORD_SANE {
        return;
    }
    X_MIN.fetch_min(x, Relaxed);
    X_MAX.fetch_max(x, Relaxed);
    Y_MIN.fetch_min(y, Relaxed);
    Y_MAX.fetch_max(y, Relaxed);
}

/// The span the scale is derived from: the measured x extent, never below the seed's, capped at twice it.
pub(super) fn span() -> i32 {
    let seed = SEED_X.1 - SEED_X.0;
    (X_MAX.load(Relaxed) - X_MIN.load(Relaxed)).clamp(seed, 2 * seed)
}

/// Sensor units per pixel below the knee, Q8 — ONE number for both axes.
pub(super) fn div_q8() -> i32 {
    span() * Q / PAD_PX
}

/// Q8 pixels for a delta of MAGNITUDE `m` (>= 0): `m / div` below the knee, the excess above it at TPSPEED's flown
/// steeper ratio (`TP_MT_DIV_LOW / TP_MT_DIV_HIGH` = 8/3 times the low gain). Continuous at the knee.
fn mag_q8(m: i64) -> i64 {
    let (d, k) = (div_q8().max(1) as i64, super::TP_MT_CURVE_KNEE as i64);
    let (lo, hi) = (m.min(k), (m - k).max(0));
    let q2 = (Q as i64) * (Q as i64);
    lo * q2 / d + hi * q2 * super::tp_div_low() as i64 / (d * super::tp_div_high() as i64)
}

/// The curve in Q8 pixels for one raw sensor delta on ONE axis, sign preserved — exactly what [`step2`] gives a
/// frame that moves along that axis alone.
pub(super) fn curve_q8(raw: i32) -> i32 {
    let q = mag_q8(raw.unsigned_abs() as i64) as i32;
    if raw < 0 { -q } else { q }
}

/// Integer square root (floor), for the frame's vector magnitude.
fn isqrt(n: i64) -> i64 {
    if n <= 0 {
        return 0;
    }
    let mut x = n;
    let mut y = (x + 1) / 2;
    while y < x {
        x = y;
        y = (x + n / x) / 2;
    }
    x
}

/// One frame, BOTH axes: the curve on the vector magnitude, that one gain on each leg (isotropic, direction kept),
/// plus the carried remainders; returns `(whole pixels out, Q8 pixels added)`, `|res| < Q` kept per axis.
pub(super) fn step2(r: (i32, i32), res: &mut (i32, i32)) -> ((i32, i32), (i32, i32)) {
    let (rx, ry) = (r.0 as i64, r.1 as i64);
    let m = if rx == 0 { ry.abs() } else if ry == 0 { rx.abs() } else { isqrt(rx * rx + ry * ry) };
    let q = if m == 0 { (0, 0) } else { let g = mag_q8(m); ((rx * g / m) as i32, (ry * g / m) as i32) };
    let (tx, ty) = (res.0 + q.0, res.1 + q.1);
    let (px, py) = (tx / Q, ty / Q);
    *res = (tx - px * Q, ty - py * Q);
    ((px, py), q)
}

/// POINTERSCALE: is this frame's step a JUMP to re-baseline on rather than motion to move by? More than a quarter of
/// the range in one frame, or the first frame after [`JUMP_GAP_MS`] of silence. Counts it when it says yes.
pub(super) fn is_jump(r: (i32, i32), gap_ms: u64) -> bool {
    let lim = (span() / 4) as i64;
    let (rx, ry) = (r.0 as i64, r.1 as i64);
    let jump = gap_ms > JUMP_GAP_MS || rx * rx + ry * ry > lim * lim;
    if jump {
        REBASE.fetch_add(1, Relaxed);
    }
    jump
}

/// The scale as one word: `div=<units per px>u/px range=<span>`.
pub(crate) struct Scale;
impl core::fmt::Display for Scale {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "div={}u/px range={}", Q8(div_q8()), span())
    }
}

/// The measured extent as one word: `x<min>..<max>,y<min>..<max>`.
struct Extent;
impl core::fmt::Display for Extent {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "x{}..{},y{}..{}", X_MIN.load(Relaxed), X_MAX.load(Relaxed), Y_MIN.load(Relaxed), Y_MAX.load(Relaxed))
    }
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
    if vendor {
        crate::pointerfeel::note_fine(why == "carried", div_q8(), span()); // POINTERSCALE (B540): the window's carried share and the scale, for `tests pointerfeel`
    }
    match why {
        "px" => { MOVED.fetch_add(1, Relaxed); }
        "carried" => { CARRIED.fetch_add(1, Relaxed); }
        "lost" => { LOST.fetch_add(1, Relaxed); }
        _ => {}
    }
    if WIT_N.load(Relaxed) < WIT_MAX {
        WIT_N.fetch_add(1, Relaxed);
        serial_println!(
            "[ptr] delta route={} in={},{} scaled={},{} moved={} why={} {} (FINEMOTION: every delta moves the pointer; sub-pixel travel is carried, never floored. POINTERSCALE: one gain on the vector, both axes)",
            if vendor { "vendor" } else { "legacy" }, inp.0, inp.1, Q8(q.0), Q8(q.1), moved as u8, why, Scale
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
        "[ptr] fine ep={} n={} moved={} carried={} lost={} floor={} rel={} vendor={} rebase={} div={}u/px range={} -> {} (route evidence: reports by stream since boot; POINTERSCALE: one scale from the pad's range, jumps re-baselined, no clamp)",
        idx, n, MOVED.load(Relaxed), CARRIED.load(Relaxed), lost, if lost == 0 { "none" } else { "LOST" },
        REL.load(Relaxed), VENDOR.load(Relaxed), REBASE.load(Relaxed), Q8(div_q8()), Extent, if lost == 0 { "OK" } else { "FAIL" }
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
    // POINTERSCALE (B540): flight 32's own diagonal frame gets ONE gain on both legs (qx/qy == rx/ry within a Q8
    // unit); a quarter-range step re-baselines (no pixels, remainder dropped) and the next frame moves again.
    let (_, qd) = step2((-18, -51), &mut (0, 0));
    let iso_ok = qd.0 < 0 && qd.1 < 0 && ((qd.0 as i64) * 51 - (qd.1 as i64) * 18).abs() <= 51;
    let _ = c.mt_step(fr(0, 0));
    let _ = c.mt_step(fr(0, 1));
    let jump_x = span() / 4 + 1;
    let (_, jx, _, _) = c.mt_step(fr(jump_x, 1));
    let jump_ok = jx == 0 && c.mt_res == (0, 0) && c.mt_last == (0, 0, 0, 0);
    let (_, nx, _, _) = c.mt_step(fr(jump_x + 100, 1));
    let jump_ok = jump_ok && nx >= 1;
    let stall_s = crate::arch::ms().saturating_sub(t0) / 1000;
    let pass = one == 200 && ten == 20 && floor_ok && legacy_ok && iso_ok && jump_ok && stall_s == 0;
    serial_println!(
        ":: FINEMOTION: one_count_moved={}/200 ten_count_moved={}/20 floor={} stall_s={} iso={} jump={} {} -> {} ::",
        one, ten, if floor_ok && legacy_ok { "none" } else { "FLOORED" }, stall_s, if iso_ok { "ok" } else { "BENT" },
        if jump_ok { "rebased" } else { "MOVED" }, Scale, if pass { "PASS" } else { "FAIL" }
    );
}
