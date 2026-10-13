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
//!
//! POINTERSCALE2 (rmbp-ledger B554, flight 33: "mouse even less responsive than ever!"). Two of B540's three repairs
//! made it worse with use: (1) the range-derived scale DRIFTED — [`range_note`] widened the extent the gain came from,
//! so every new extreme lowered the gain for the rest of the session (f33-boot2: `div=5.07u/px range=x-3243..4069` at
//! 06:00:03, `div=5.55u/px range=x-3243..4751` at 06:00:54, `div=5.84u/px range=x-3243..5182` at 06:02:25 — monotone,
//! 15 % slower in two and a half minutes). The scale is now FIXED: [`PAD_RANGE_X`] is a named constant, never
//! re-derived; the live extent is only PRINTED (`range_seen=`) and changes nothing. (2) a fast stroke was DISCARDED —
//! a step over a quarter of the range re-baselined (`rebase=8` in two minutes), throwing the stroke away. It is now
//! CLAMPED ([`clamp`]): magnitude capped at [`MAX_STEP`], direction kept, the pointer moves; `rebase=` counts only a
//! true re-touch, the first frame after [`JUMP_GAP_MS`] of silence with no lift frame. Wire: `[ptr] fine … rebase=<n>
//! clamped=<n> div=… range=x<min>..<max>,y<min>..<max> range_seen=x<min>..<max>,y<min>..<max>`.

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
/// POINTERSCALE2: THIS PAD'S RANGE, FIXED — the OBSERVED extent of the rMBP's Wellspring sensor over two flights, not a
/// declared one (the vendor interface's report descriptor carries no logical range for the finger fields). x: -3243
/// (f32-boot1 04:23:59, and f33-boot2's own minimum) .. 4751 (f33-boot2 06:00:54, `range=x-3243..4751`); y: -141
/// (f32-boot1 04:22:40) .. 6165 (f32-boot1 04:25:02; f33 never exceeded it). The same f33 boot later read x 5182
/// (06:02:25): a reading beyond this constant is printed as `range_seen=` and moves NOTHING — the gain is one number for
/// the life of the boot, so the pointer feels the same at minute 1 and minute 100.
pub(super) const PAD_RANGE_X: (i32, i32) = (-3243, 4751);
pub(super) const PAD_RANGE_Y: (i32, i32) = (-141, 6165);
/// The x span the one scale is derived from: 7994 units = [`PAD_PX`] px below the knee, 5.55 units/px.
const PAD_SPAN: i32 = PAD_RANGE_X.1 - PAD_RANGE_X.0;
/// POINTERSCALE2: the largest raw step one frame may carry, as a vector MAGNITUDE — an eighth of the pad, i.e. a full
/// pad width in 80 ms at the pad's ~100 Hz frame rate, faster than any deliberate stroke. A bigger step (a burst after
/// a stream hole, a flick) is CLAMPED to this length with its direction kept: the pointer moves, fast, the way the
/// finger went — never discarded (B540's quarter-range rebase threw it away) and never bent (B540's per-axis clamp did).
pub(super) const MAX_STEP: i32 = PAD_SPAN / 8;
/// A coordinate beyond this magnitude is garbled and never enters `range_seen=`.
const COORD_SANE: i32 = 16_384;
/// POINTERSCALE: a frame after this much stream silence (with no lift frame between) is a RE-TOUCH and re-baselines.
/// POINTERSCALE2: this is now the ONLY rebase (`gap >= 250 ms`); a fast step inside it is clamped, not rebased.
pub(super) const JUMP_GAP_MS: u64 = 250;

/// POINTERSCALE2: the live extent, PRINTED as `range_seen=` and read by nothing else (seeded empty).
static X_MIN: AtomicI32 = AtomicI32::new(i32::MAX);
static X_MAX: AtomicI32 = AtomicI32::new(i32::MIN);
static Y_MIN: AtomicI32 = AtomicI32::new(i32::MAX);
static Y_MAX: AtomicI32 = AtomicI32::new(i32::MIN);
static REBASE: AtomicU32 = AtomicU32::new(0);
static CLAMPED: AtomicU32 = AtomicU32::new(0);

/// One touching frame's raw sensor position into `range_seen=` (live endpoint only — the fixtures' censuses never call
/// it). POINTERSCALE2: a witness only; the scale never reads it.
pub(super) fn range_note(x: i32, y: i32) {
    if x.abs() > COORD_SANE || y.abs() > COORD_SANE {
        return;
    }
    X_MIN.fetch_min(x, Relaxed);
    X_MAX.fetch_max(x, Relaxed);
    Y_MIN.fetch_min(y, Relaxed);
    Y_MAX.fetch_max(y, Relaxed);
}

/// The span the scale is derived from — POINTERSCALE2: the fixed [`PAD_RANGE_X`] extent, never the live one.
pub(super) fn span() -> i32 {
    PAD_SPAN
}

/// POINTERSCALE2: one frame's raw step with its MAGNITUDE capped at [`MAX_STEP`], direction kept (both legs scaled by
/// the same factor). Pure; `true` when it cut.
fn cap(r: (i32, i32)) -> ((i32, i32), bool) {
    let (rx, ry) = (r.0 as i64, r.1 as i64);
    let m2 = rx * rx + ry * ry;
    let lim = MAX_STEP as i64;
    if m2 <= lim * lim {
        return (r, false);
    }
    let m = isqrt(m2).max(1);
    (((rx * lim / m) as i32, (ry * lim / m) as i32), true)
}

/// [`cap`] on the decode path (`TpCensus::mt_step`), counted (`clamped=` on `[ptr] fine`) when it cuts.
pub(super) fn clamp(r: (i32, i32)) -> (i32, i32) {
    let (out, cut) = cap(r);
    if cut {
        CLAMPED.fetch_add(1, Relaxed);
    }
    out
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

/// POINTERSCALE: is this frame a RE-TOUCH to re-baseline on rather than motion to move by? POINTERSCALE2: ONLY the
/// first frame after [`JUMP_GAP_MS`] or more of silence — the size of the step no longer decides it (a big step is
/// [`clamp`]ed and moves). `_r` is kept so the call site's shape is unchanged. Counts it when it says yes.
pub(super) fn is_jump(_r: (i32, i32), gap_ms: u64) -> bool {
    let jump = gap_ms >= JUMP_GAP_MS;
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

/// The fixed range and the live one as two words: `range=x<min>..<max>,y<min>..<max> range_seen=x<min>..<max>,y<min>..<max>`
/// (`range_seen=-` before the first touching frame). POINTERSCALE2: only `range=` scales anything.
struct Extent;
impl core::fmt::Display for Extent {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "range=x{}..{},y{}..{} range_seen=", PAD_RANGE_X.0, PAD_RANGE_X.1, PAD_RANGE_Y.0, PAD_RANGE_Y.1)?;
        let (x0, x1) = (X_MIN.load(Relaxed), X_MAX.load(Relaxed));
        if x0 > x1 {
            return f.write_str("-");
        }
        write!(f, "x{}..{},y{}..{}", x0, x1, Y_MIN.load(Relaxed), Y_MAX.load(Relaxed))
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
        "[ptr] fine ep={} n={} moved={} carried={} lost={} floor={} rel={} vendor={} rebase={} clamped={} div={}u/px {} -> {} (route evidence: reports by stream since boot; POINTERSCALE2: one FIXED scale from the pad's observed range, range_seen= changes nothing; a fast step clamped with its direction kept, rebase= only a re-touch after 250 ms silence)",
        idx, n, MOVED.load(Relaxed), CARRIED.load(Relaxed), lost, if lost == 0 { "none" } else { "LOST" },
        REL.load(Relaxed), VENDOR.load(Relaxed), REBASE.load(Relaxed), CLAMPED.load(Relaxed), Q8(div_q8()), Extent, if lost == 0 { "OK" } else { "FAIL" }
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
    // unit). POINTERSCALE2 (B554): a quarter-range step inside the silence rule is CLAMPED, not rebased — it moves the
    // pointer by exactly curve(MAX_STEP) along its own direction (`jump=clamped`); a frame after JUMP_GAP_MS of silence
    // with no lift re-baselines (no pixels, remainder dropped) and the next frame moves again (`retouch=rebased`).
    let (_, qd) = step2((-18, -51), &mut (0, 0));
    let iso_ok = qd.0 < 0 && qd.1 < 0 && ((qd.0 as i64) * 51 - (qd.1 as i64) * 18).abs() <= 51;
    let _ = c.mt_step(fr(0, 0));
    let _ = c.mt_step(fr(0, 1));
    let jump_x = span() / 4 + 1;
    let (_, jx, _, _) = c.mt_step(fr(jump_x, 1));
    let jump_ok = jx >= 1 && c.mt_last.0 == MAX_STEP && c.mt_last.2 == curve_q8(MAX_STEP) && cap((jump_x, 0)) == ((MAX_STEP, 0), true);
    let ((cx, cy), _) = cap((-3 * MAX_STEP, -4 * MAX_STEP)); // a 3-4-5 diagonal: clamped to length MAX_STEP, the 3:4 kept
    let dir_ok = cx < 0 && cy < 0 && ((cx as i64) * 4 - (cy as i64) * 3).abs() <= 4 && (cx * cx + cy * cy) as i64 <= (MAX_STEP as i64) * (MAX_STEP as i64);
    let now = crate::arch::ms();
    let retouch_ok = if now > 2 * JUMP_GAP_MS {
        c.mt_ms = now - JUMP_GAP_MS - 10; // the stream went silent 260 ms ago with the finger never lifted
        let (_, rx, _, _) = c.mt_step(fr(jump_x + 50, 1));
        let rebased = rx == 0 && c.mt_res == (0, 0) && c.mt_last == (0, 0, 0, 0);
        let (_, nx, _, _) = c.mt_step(fr(jump_x + 150, 1));
        rebased && nx >= 1
    } else {
        true // a boot under half a second old cannot fake a 260 ms silence; never the case at `tests finemotion`
    };
    let stall_s = crate::arch::ms().saturating_sub(t0) / 1000;
    let pass = one == 200 && ten == 20 && floor_ok && legacy_ok && iso_ok && jump_ok && dir_ok && retouch_ok && stall_s == 0;
    serial_println!(
        ":: FINEMOTION: one_count_moved={}/200 ten_count_moved={}/20 floor={} stall_s={} iso={} jump={} retouch={} max_step={} {} -> {} ::",
        one, ten, if floor_ok && legacy_ok { "none" } else { "FLOORED" }, stall_s, if iso_ok { "ok" } else { "BENT" },
        if jump_ok && dir_ok { "clamped" } else { "DISCARDED" }, if retouch_ok { "rebased" } else { "MOVED" }, MAX_STEP, Scale, if pass { "PASS" } else { "FAIL" }
    );
}
