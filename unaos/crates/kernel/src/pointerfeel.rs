//! CHARTER: Kernel — kernel-by-ruling (B535 POINTERFEEL, R103 §2: the pointer measured the way the person feels it)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! POINTERFEEL (rmbp-ledger B535) — flight 31, Peter at the glass: "mouse completely fucking terrible in nearly every
//! way  can only just get it to work", on a boot where HIDSTALL, FINEMOTION 200/200 and INPUTSTALL all PASSED. Those
//! fixtures inject deltas or read the kernel's own passes; none of them counts the trackpad's REAL report stream under
//! a finger. This file is the instrument that does, plus the two readings the row names beside it:
//!
//! * `tests pointerfeel` (R80: only when asked) — on its own task (QUEUEWEDGE's shape, so the verb returns at once),
//!   the person is asked on the wire and the glass to move one finger on the trackpad for [`WINDOW_MS`]. The window
//!   opens at the FIRST finger-down report (waits up to [`TOUCH_WAIT_MS`]); every trackpad report the EHCI decode
//!   installs is counted ([`note`], both routes), and every gap over [`BOUND_MS`] between two finger-down reports is
//!   named with the hid pump core's holder over that gap (HIDSTALL2's census ring, [`crate::hidstall::holder_between`]):
//!   `:: POINTERFEEL: reports=<n> rate_hz=<f> gaps=<n> gap_max_ms=<n> worst_holder=<site> route=<legacy|mt> -> PASS|FAIL
//!   :: bound_ms=16 …`, then one `[ptrfeel] gap` line per named gap (at most [`GAP_LOG`]).
//!   PASS iff reports > 0, gaps == 0 and the route is `mt`. The route is in the verdict on purpose: on the legacy
//!   (id 0x02, HID-mode) stream the pad reports only on motion, so a resting finger and a lost report read the same —
//!   the legacy lane cannot pass a fixture whose question is "did every report while the finger was down arrive";
//!   the vendor stream reports every frame while a finger is down, so there a gap IS a gap.
//! * the mode RE-ASSERT counter ([`reassert_due`] / [`note_reassert`]) — `drivers/ehci/mod.rs`' tail `tp_mode_reassert`
//!   (the route fix) is bounded here: at most [`REASSERT_MAX`] per boot, [`REASSERT_GAP_MS`] apart.
//! * the login screen's `hid_gap_max_ms` NAMED ([`note_login_gap`], from `boot::hid_pass`): the holder of the pump core
//!   over the longest login gap and the `usb-pump` step in flight when it ended (`hid_gap_holder=` on the `[login] key
//!   latency` line, [`LoginGapHolder`]).
//!
//! * POINTERSCALE (rmbp-ledger B540): the vendor lane's ONE scale and the window's CARRIED share ([`note_fine`], from
//!   `drivers/ehci/finemotion.rs`) — `scale=<units per px>u/px range=<span> carried=<reports that moved nothing>/<reports
//!   with motion>` on the verdict line, so the glass reading of "slow motion is swallowed" is a number.
//!
//! Writers: [`note`], [`note_fine`] and [`note_login_gap`] run on the `hid-pump` only (one writer), so every field is a relaxed atomic.

use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering::Relaxed};

/// The measuring window, from the first finger-down report (the row's 5 s).
pub const WINDOW_MS: u64 = 5_000;
/// A gap between two finger-down reports longer than this is a gap (one 60 Hz frame — HIDSTALL's `HID_GAP_STALL_MS`).
pub const BOUND_MS: u64 = crate::hidstall::HID_GAP_STALL_MS;
/// How long the fixture waits for the first touch before it says so.
const TOUCH_WAIT_MS: u64 = 20_000;
/// Gaps named one per line after the verdict.
const GAP_LOG: usize = 8;
/// Mode re-asserts per boot, and the least time between two.
pub const REASSERT_MAX: u32 = 2;
pub const REASSERT_GAP_MS: u64 = 2_000;

static RUNNING: AtomicBool = AtomicBool::new(false);
/// The window is open for [`note`] (set by the fixture; the decode's cost when clear is one relaxed load).
static ARMED: AtomicBool = AtomicBool::new(false);
/// `arch::ms()` of the first finger-down report in the window (0 = still waiting for the touch).
static START_MS: AtomicU64 = AtomicU64::new(0);
static LAST_MS: AtomicU64 = AtomicU64::new(0);
static PREV_DOWN: AtomicBool = AtomicBool::new(false);
static REPORTS: AtomicU32 = AtomicU32::new(0);
static MT: AtomicU32 = AtomicU32::new(0);
static LEGACY: AtomicU32 = AtomicU32::new(0);
static GAPS: AtomicU32 = AtomicU32::new(0);
static GAP_MAX_MS: AtomicU64 = AtomicU64::new(0);
/// The worst gap's holder (`hidstall` holder index, `usize::MAX` = the pump core was not held over it).
static GAP_MAX_WHO: AtomicUsize = AtomicUsize::new(usize::MAX);
/// The worst gap's `usb-pump` step at its end (a [`crate::video::lag::PUMP_NAMES`] id).
static GAP_MAX_STEP: AtomicU32 = AtomicU32::new(0);
/// The first [`GAP_LOG`] gaps: `at_ms << 24 | gap_ms (≤ 2^16-1) << 8 | step`, and their holder `(ix + 1) << 32 | held_ms`.
static GAP_AT: [AtomicU64; GAP_LOG] = [const { AtomicU64::new(0) }; GAP_LOG];
static GAP_WHO: [AtomicU64; GAP_LOG] = [const { AtomicU64::new(0) }; GAP_LOG];

/// POINTERSCALE: the window's motion reports, those whose travel stayed in the sub-pixel carry, and the scale.
static FINE_N: AtomicU32 = AtomicU32::new(0);
static FINE_CARRIED: AtomicU32 = AtomicU32::new(0);
static FINE_DIV_Q8: AtomicU32 = AtomicU32::new(0);
static FINE_SPAN: AtomicU32 = AtomicU32::new(0);

static REASSERTS: AtomicU32 = AtomicU32::new(0);
static REASSERT_LATCHED: AtomicU32 = AtomicU32::new(0);
static REASSERT_LAST_MS: AtomicU64 = AtomicU64::new(0);

static LOGIN_GAP_MS: AtomicU64 = AtomicU64::new(0);
static LOGIN_GAP_WHO: AtomicUsize = AtomicUsize::new(usize::MAX);
static LOGIN_GAP_HELD: AtomicU64 = AtomicU64::new(0);
static LOGIN_GAP_STEP: AtomicU32 = AtomicU32::new(0);

/// Register `tests pointerfeel` once (`tests.rs`' shellux ensure, LINE-NEUTRAL append).
pub fn ensure() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if !DONE.swap(true, core::sync::atomic::Ordering::AcqRel) {
        crate::tests::register("pointerfeel", selftest);
    }
}

// ── the decode's half (hid-pump) ───────────────────────────────────────────────────────────────────────────────────

/// One trackpad report the EHCI decode installed, at the pass clock `now` (`drivers/ehci/mod.rs`, both arms).
/// `vendor` = the multi-touch (TYPE2 frame) route; `down` = a finger is on the pad (vendor: `fingers > 0`; legacy: the
/// report moved or holds a button — the HID-mode stream cannot see a resting finger).
#[inline]
pub fn note(now: u64, vendor: bool, down: bool) {
    if !ARMED.load(Relaxed) {
        return;
    }
    let start = START_MS.load(Relaxed);
    if start == 0 {
        if !down {
            return;
        }
        START_MS.store(now.max(1), Relaxed);
    } else if now >= start.saturating_add(WINDOW_MS) {
        return;
    }
    REPORTS.fetch_add(1, Relaxed);
    if vendor { MT.fetch_add(1, Relaxed); } else { LEGACY.fetch_add(1, Relaxed); }
    let prev = LAST_MS.swap(now, Relaxed);
    let prev_down = PREV_DOWN.swap(down, Relaxed);
    if prev == 0 || !prev_down || !down {
        return;
    }
    let gap = now.saturating_sub(prev);
    if gap <= BOUND_MS {
        return;
    }
    let (held, who) = crate::hidstall::holder_between(prev, now);
    let step = crate::video::lag::pump_step_now() as u64;
    let k = GAPS.fetch_add(1, Relaxed) as usize;
    if let (Some(a), Some(w)) = (GAP_AT.get(k), GAP_WHO.get(k)) {
        a.store(now << 24 | gap.min(0xFFFF) << 8 | (step & 0xff), Relaxed);
        w.store(if who == usize::MAX { 0 } else { (who as u64 + 1) << 32 } | held.min(0xFFFF_FFFF), Relaxed);
    }
    if gap > GAP_MAX_MS.load(Relaxed) {
        GAP_MAX_MS.store(gap, Relaxed);
        GAP_MAX_WHO.store(who, Relaxed);
        GAP_MAX_STEP.store(step as u32, Relaxed);
    }
}

/// POINTERSCALE (B540): one vendor-lane report WITH motion (`drivers/ehci/finemotion.rs`' `note`) — `carried` = it put
/// no whole pixel out, its travel stayed in the sub-pixel remainder; `div_q8` / `span` = the scale it ran at.
/// Counted only inside an open window (after the first touch), like [`note`].
#[inline]
pub fn note_fine(carried: bool, div_q8: i32, span: i32) {
    if !ARMED.load(Relaxed) || START_MS.load(Relaxed) == 0 {
        return;
    }
    FINE_N.fetch_add(1, Relaxed);
    if carried {
        FINE_CARRIED.fetch_add(1, Relaxed);
    }
    FINE_DIV_Q8.store(div_q8.max(0) as u32, Relaxed);
    FINE_SPAN.store(span.max(0) as u32, Relaxed);
}

/// `scale=<u/px>u/px range=<span> carried=<c>/<n>` — the window's scale and carried share (`-` before any motion).
struct FineWord;
impl core::fmt::Display for FineWord {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let (n, c, d) = (FINE_N.load(Relaxed), FINE_CARRIED.load(Relaxed), FINE_DIV_Q8.load(Relaxed));
        if n == 0 {
            return f.write_str("scale=- carried=0/0");
        }
        write!(f, "scale={}.{:02}u/px range={} carried={}/{} carried_pct={}", d / 256, (d % 256) * 100 / 256, FINE_SPAN.load(Relaxed), c, n, c * 100 / n)
    }
}

/// The stream contradicted a latched readback (`[tp] mode-mismatch readback=vendor stream=legacy`): may the decode ask
/// for a mode re-assert now? Claims the slot when it says yes.
pub fn reassert_due() -> bool {
    let now = crate::arch::ms().max(1);
    let last = REASSERT_LAST_MS.load(Relaxed);
    if REASSERTS.load(Relaxed) >= REASSERT_MAX || (last != 0 && now.saturating_sub(last) < REASSERT_GAP_MS) {
        return false;
    }
    REASSERT_LAST_MS.store(now, Relaxed);
    REASSERTS.fetch_add(1, Relaxed);
    true
}

/// A re-assert ran; `latched` = the readback after it said vendor.
pub fn note_reassert(latched: bool) {
    if latched {
        REASSERT_LATCHED.fetch_add(1, Relaxed);
    }
}

/// Re-asserts so far this boot.
pub fn reasserts() -> u32 {
    REASSERTS.load(Relaxed)
}

/// `boot::hid_pass`: a NEW longest login-screen gap `[last, now]` — name the pump core's holder over it and the
/// `usb-pump` step in flight when it ended.
pub fn note_login_gap(last: u64, now: u64) {
    let (held, who) = crate::hidstall::holder_between(last, now);
    LOGIN_GAP_MS.store(now.saturating_sub(last), Relaxed);
    LOGIN_GAP_WHO.store(who, Relaxed);
    LOGIN_GAP_HELD.store(held, Relaxed);
    LOGIN_GAP_STEP.store(crate::video::lag::pump_step_now() as u32, Relaxed);
}

/// The longest login gap's name: `<holder>/held_ms=<n>` when the pump core was held over it, else
/// `unheld/step=<usb-pump step>` (the gap was the hid pump not being run, not a masked hold).
pub struct LoginGapHolder;
impl core::fmt::Display for LoginGapHolder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if LOGIN_GAP_MS.load(Relaxed) == 0 {
            return f.write_str("-");
        }
        Holder(LOGIN_GAP_WHO.load(Relaxed), LOGIN_GAP_HELD.load(Relaxed), LOGIN_GAP_STEP.load(Relaxed) as u8).fmt(f)
    }
}

/// `(holder index, held ms, usb-pump step)` as one word on the wire.
struct Holder(usize, u64, u8);
impl core::fmt::Display for Holder {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.0 == usize::MAX {
            write!(f, "unheld/step={}", crate::video::lag::PUMP_NAMES.get(self.2 as usize).copied().unwrap_or("-"))
        } else {
            write!(f, "{}/held_ms={}", crate::hidstall::HolderName(self.0), self.1)
        }
    }
}

// ── the fixture ────────────────────────────────────────────────────────────────────────────────────────────────────

/// The PASS rule. Pure.
pub fn verdict(reports: u32, gaps: u32, mt: u32, legacy: u32) -> bool {
    reports > 0 && gaps == 0 && mt > legacy
}

/// `tests pointerfeel`.
pub fn selftest() {
    if RUNNING.load(Relaxed) {
        serial_println!(":: POINTERFEEL: -> SKIP :: reason=already-running bound_ms={}", BOUND_MS);
        return;
    }
    spawn_window();
}

#[cfg(target_arch = "x86_64")]
fn spawn_window() {
    let cpu = crate::arch::smp::worker_cpu(0).unwrap_or_else(crate::arch::sched::meter_current_cpu);
    crate::arch::sched::spawn("pointerfeel", window_task, 0, cpu, crate::arch::sched::PRIO_NORMAL);
    serial_println!(
        ":: POINTERFEEL: armed window_ms={} on=task cpu={} via=tests -> SKIP reason=verdict-follows :: (move one finger on the trackpad for {} s; the verdict follows)",
        WINDOW_MS, cpu, WINDOW_MS / 1000
    );
}

#[cfg(not(target_arch = "x86_64"))]
fn spawn_window() {
    serial_println!(":: POINTERFEEL: -> SKIP :: reason=no-ehci-trackpad (the bcm5974 decode is x86's) bound_ms={}", BOUND_MS);
}

#[cfg(target_arch = "x86_64")]
fn window_task(_: usize) {
    run();
    crate::arch::sched::exit();
}

#[cfg(target_arch = "x86_64")]
fn run() {
    if RUNNING.swap(true, core::sync::atomic::Ordering::AcqRel) {
        serial_println!(":: POINTERFEEL: -> SKIP :: reason=already-running bound_ms={}", BOUND_MS);
        return;
    }
    // Reset, then open. The decode only reads ARMED; it writes the rest, and nothing writes them while ARMED is clear.
    for a in GAP_AT.iter().chain(GAP_WHO.iter()) {
        a.store(0, Relaxed);
    }
    START_MS.store(0, Relaxed);
    LAST_MS.store(0, Relaxed);
    PREV_DOWN.store(false, Relaxed);
    for c in [&REPORTS, &MT, &LEGACY, &GAPS, &FINE_N, &FINE_CARRIED, &FINE_DIV_Q8, &FINE_SPAN] {
        c.store(0, Relaxed);
    }
    GAP_MAX_MS.store(0, Relaxed);
    GAP_MAX_WHO.store(usize::MAX, Relaxed);
    GAP_MAX_STEP.store(0, Relaxed);
    ARMED.store(true, core::sync::atomic::Ordering::Release);
    serial_println!("[ptrfeel] move one finger on the trackpad for {} s (waiting up to {} s for the first touch)", WINDOW_MS / 1000, TOUCH_WAIT_MS / 1000);
    let _ = crate::video::notify(b"pointerfeel", b"Trackpad test", b"Move one finger on the trackpad for 5 seconds", b"", 0, b"");
    let t0 = crate::arch::ms();
    loop {
        let s = START_MS.load(Relaxed);
        let now = crate::arch::ms();
        if s != 0 && now >= s.saturating_add(WINDOW_MS) {
            break;
        }
        if s == 0 && now.saturating_sub(t0) >= TOUCH_WAIT_MS {
            break;
        }
        crate::arch::sched::sleep_ms(50);
    }
    ARMED.store(false, core::sync::atomic::Ordering::Release);
    crate::arch::sched::sleep_ms(2); // a decode already past the ARMED load finishes its note
    report();
    RUNNING.store(false, core::sync::atomic::Ordering::Release);
}

#[cfg(target_arch = "x86_64")]
fn report() {
    let (fr_cpu, fr_worst) = crate::flight_recorder::worker_state();
    let fr = FrWord(fr_cpu);
    if START_MS.load(Relaxed) == 0 {
        serial_println!(
            ":: POINTERFEEL: reports=0 rate_hz=0.0 gaps=0 gap_max_ms=0 worst_holder=- route=- -> SKIP :: reason=no-touch bound_ms={} window_ms={} touch_wait_ms={} reassert={} fr={} fr_worst_ms={} login_gap_ms={} login_gap_holder={} via=tests",
            BOUND_MS, WINDOW_MS, TOUCH_WAIT_MS, reasserts(), fr, fr_worst, LOGIN_GAP_MS.load(Relaxed), LoginGapHolder
        );
        return;
    }
    let (n, mt, leg, gaps) = (REPORTS.load(Relaxed), MT.load(Relaxed), LEGACY.load(Relaxed), GAPS.load(Relaxed));
    let rate_x10 = (n as u64) * 10_000 / WINDOW_MS;
    let gmax = GAP_MAX_MS.load(Relaxed);
    let worst = if gaps == 0 { None } else { Some(Holder(GAP_MAX_WHO.load(Relaxed), 0, GAP_MAX_STEP.load(Relaxed) as u8)) };
    serial_println!(
        ":: POINTERFEEL: reports={} rate_hz={}.{} gaps={} gap_max_ms={} worst_holder={} route={} {} -> {} :: bound_ms={} window_ms={} mt={} legacy={} reassert={} reassert_latched={} fr={} fr_worst_ms={} login_gap_ms={} login_gap_holder={} via=tests",
        n, rate_x10 / 10, rate_x10 % 10, gaps, gmax, WorstWord(worst), if mt > leg { "mt" } else { "legacy" }, FineWord,
        if verdict(n, gaps, mt, leg) { "PASS" } else { "FAIL" }, BOUND_MS, WINDOW_MS, mt, leg, reasserts(),
        REASSERT_LATCHED.load(Relaxed), fr, fr_worst, LOGIN_GAP_MS.load(Relaxed), LoginGapHolder
    );
    let start = START_MS.load(Relaxed);
    for k in 0..(gaps as usize).min(GAP_LOG) {
        let (a, w) = (GAP_AT[k].load(Relaxed), GAP_WHO[k].load(Relaxed));
        let who = if w >> 32 == 0 { usize::MAX } else { (w >> 32) as usize - 1 };
        serial_println!(
            "[ptrfeel] gap n={} at_ms={} ms={} holder={} (POINTERFEEL: a finger-down report stream silent over {} ms)",
            k + 1, (a >> 24).saturating_sub(start), (a >> 8) & 0xFFFF, Holder(who, w & 0xFFFF_FFFF, (a & 0xff) as u8), BOUND_MS
        );
    }
}

struct WorstWord(Option<Holder>);
impl core::fmt::Display for WorstWord {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match &self.0 {
            None => f.write_str("-"),
            // the worst gap's holder by NAME (its own held ms is on its `[ptrfeel] gap` line)
            Some(h) if h.0 == usize::MAX => h.fmt(f),
            Some(h) => write!(f, "{}", crate::hidstall::HolderName(h.0)),
        }
    }
}

/// The flight recorder's service: `worker:c<n>` once it runs off the pump, `inline` before (or with no worker core).
struct FrWord(usize);
impl core::fmt::Display for FrWord {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.0 == usize::MAX { f.write_str("inline") } else { write!(f, "worker:c{}", self.0) }
    }
}
