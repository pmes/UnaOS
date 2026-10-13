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
    let prev = LAST_MS.swap(now, Relaxed); let pass_gap = PASS_GAP_MS.swap(0, Relaxed); // PUMPHOLE (B541): the longest hid-pass gap since the previous report, taken at EVERY counted report
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
    let k = GAPS.fetch_add(1, Relaxed) as usize; note_class(k, gap, pass_gap, who); // PUMPHOLE (B541): the gap classed starved (the hid pass itself was late or held) or silent (the pass ran on cadence and no frame came)
    if let (Some(a), Some(w)) = (GAP_AT.get(k), GAP_WHO.get(k)) {
        a.store(now << 24 | gap.min(0xFFFF) << 8 | (step & 0xff), Relaxed);
        w.store(if who == usize::MAX { 0 } else { (who as u64 + 1) << 32 } | held.min(0xFFFF_FFFF), Relaxed);
    }
    if gap > GAP_MAX_MS.load(Relaxed) {
        GAP_MAX_MS.store(gap, Relaxed);
        GAP_MAX_WHO.store(who, Relaxed);
        GAP_MAX_STEP.store(step as u32, Relaxed); GAP_MAX_PASS.store(pass_gap, Relaxed); // PUMPHOLE (B541): the worst gap's own pass gap
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

/// The PASS rule (PUMPHOLE B541: `gaps` is the STARVED count — a silent gap is the pad's, said apart). Pure.
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
    GAP_MAX_STEP.store(0, Relaxed); reset_class(); // PUMPHOLE (B541)
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
        ":: POINTERFEEL: reports={} rate_hz={}.{} gaps={} gap_max_ms={} worst_holder={} route={} {} {} -> {} :: bound_ms={} window_ms={} mt={} legacy={} reassert={} reassert_latched={} fr={} fr_worst_ms={} login_gap_ms={} login_gap_holder={} starved={} starved_max_ms={} silent={} silent_max_ms={} pass_max_ms={} pass_bound_ms={} via=tests",
        n, rate_x10 / 10, rate_x10 % 10, gaps, gmax, Classed(silent_worst(gaps), WorstWord(worst)), if mt > leg { "mt" } else { "legacy" }, FineWord, RateWord(rate_x10), // POINTERSCALE2 (B554): `expected_hz=` from the endpoint's bInterval beside `rate_hz=` (LINE-NEUTRAL, code first)
        if verdict(n, STARVED.load(Relaxed), mt, leg) && rate_ok(rate_x10, expected_hz()) { "PASS" } else { "FAIL" }, BOUND_MS, WINDOW_MS, mt, leg, reasserts(),
        REASSERT_LATCHED.load(Relaxed), fr, fr_worst, LOGIN_GAP_MS.load(Relaxed), LoginGapHolder, STARVED.load(Relaxed), STARVED_MAX_MS.load(Relaxed), SILENT.load(Relaxed), SILENT_MAX_MS.load(Relaxed), PASS_MAX_MS.load(Relaxed), BOUND_MS
    );
    let start = START_MS.load(Relaxed);
    for k in 0..(gaps as usize).min(GAP_LOG) {
        let (a, w) = (GAP_AT[k].load(Relaxed), GAP_WHO[k].load(Relaxed));
        let who = if w >> 32 == 0 { usize::MAX } else { (w >> 32) as usize - 1 };
        serial_println!(
            "[ptrfeel] gap n={} at_ms={} ms={} holder={} (POINTERFEEL: a finger-down report stream silent over {} ms){}",
            k + 1, (a >> 24).saturating_sub(start), (a >> 8) & 0xFFFF, Holder(who, w & 0xFFFF_FFFF, (a & 0xff) as u8), BOUND_MS, GapClass(k)
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

// ── PUMPHOLE (rmbp-ledger B541) — a report gap is classed by the hid pass's OWN cadence over it. FILE-TAIL append. ────────
//
// THE WIRE (flight 32 boot 1, `tests pointerfeel` with a finger moving): `gaps=11 gap_max_ms=2016
// worst_holder=unheld/step=bootlog`. Read against the same second: `[lag] stall … hid_gap_ms=8 … pump=bootlog pump_ms=53.2
// masked_ms=0 holder=-` — the hid pass ran at most 8 ms apart while the usb-pump sat 53 ms in its `bootlog` step, and the
// census (`[hid] stall census … holders=5`) names NO hold in step `bootlog` all session. The two pumps are separate
// PRIO_NORMAL tasks on one core and the scheduler preempts at `QUANTUM_TICKS` (4 ticks, arch/x86_64/sched.rs:174), so an
// UNMASKED step cannot hold the hid pass past one quantum; `step=` was a sample of where the usb-pump happened to be when
// the next report landed (its longest non-nap step), never a holder. What that step does is `bootpace::service_dump()`
// (main.rs:6026): `clock_x1_poll`, `fb_wc_witness`, `ftdi::late_verdict` (latched one-liners), `video::dimidle::service`,
// `video::wm::paygo_service` and the BPACE ledger's early-return snapshot — 17–53 ms per pass on the wire, unmasked.
//
// So the fixture now measures the half it could not see: [`note_pass`] (the hid pass, after it takes `EHCI_HID`, so a pass
// that waited on the lock is charged its wait) keeps the longest pass-to-pass gap since the last report; each report gap
// over [`BOUND_MS`] is STARVED when that pass gap also exceeded the bound or the pump core was held over it (the kernel's
// loss — the verdict's count), else SILENT (the pass polled on cadence and the pad sent no frame — the device's, said
// apart: `silent=<n> silent_max_ms=<n>`). The bound the verdict holds: every finger-down report gap the KERNEL causes is
// one with a hid-pass gap over 16 ms, and PASS says there was none in the window (`pass_max_ms=` is the window's worst pass
// gap, `pass_bound_ms=16`). `gaps=`/`gap_max_ms=` stay the raw stream, so the longitudinal reading does not move.

/// The hid pass's last entry in the window (0 = none yet) and the longest pass gap since the previous report.
static PASS_LAST_MS: AtomicU64 = AtomicU64::new(0);
static PASS_GAP_MS: AtomicU64 = AtomicU64::new(0);
/// The window's longest pass gap.
static PASS_MAX_MS: AtomicU64 = AtomicU64::new(0);
static STARVED: AtomicU32 = AtomicU32::new(0);
static STARVED_MAX_MS: AtomicU64 = AtomicU64::new(0);
static SILENT: AtomicU32 = AtomicU32::new(0);
static SILENT_MAX_MS: AtomicU64 = AtomicU64::new(0);
/// The worst gap's pass gap; the first [`GAP_LOG`] gaps' pass gaps (`u64::MAX` = not classed).
static GAP_MAX_PASS: AtomicU64 = AtomicU64::new(0);
static GAP_PASS: [AtomicU64; GAP_LOG] = [const { AtomicU64::new(u64::MAX) }; GAP_LOG];

/// The hid pass ran at `now` (`drivers/ehci/mod.rs` `service_ehci_hid`, under `EHCI_HID`; hid-pump only). One relaxed
/// load when the window is shut.
#[inline]
pub fn note_pass(now: u64) {
    if !ARMED.load(Relaxed) {
        return;
    }
    let prev = PASS_LAST_MS.swap(now, Relaxed);
    if prev != 0 && now > prev {
        PASS_GAP_MS.fetch_max(now - prev, Relaxed);
        PASS_MAX_MS.fetch_max(now - prev, Relaxed);
    }
}

/// Is a report gap the kernel's? The pass itself was late over it, or the pump core was held over it. Pure.
pub fn is_starved(pass_gap_ms: u64, held_who: usize) -> bool {
    pass_gap_ms > BOUND_MS || held_who != usize::MAX
}

fn note_class(k: usize, gap: u64, pass_gap: u64, who: usize) {
    if let Some(a) = GAP_PASS.get(k) {
        a.store(pass_gap, Relaxed);
    }
    if is_starved(pass_gap, who) {
        STARVED.fetch_add(1, Relaxed);
        STARVED_MAX_MS.fetch_max(gap, Relaxed);
    } else {
        SILENT.fetch_add(1, Relaxed);
        SILENT_MAX_MS.fetch_max(gap, Relaxed);
    }
}

fn reset_class() {
    for a in GAP_PASS.iter() {
        a.store(u64::MAX, Relaxed);
    }
    for a in [&PASS_LAST_MS, &PASS_GAP_MS, &PASS_MAX_MS, &STARVED_MAX_MS, &SILENT_MAX_MS, &GAP_MAX_PASS] {
        a.store(0, Relaxed);
    }
    STARVED.store(0, Relaxed);
    SILENT.store(0, Relaxed);
}

/// The worst gap was SILENT (unheld, the pass on cadence over it).
fn silent_worst(gaps: u32) -> bool {
    gaps > 0 && !is_starved(GAP_MAX_PASS.load(Relaxed), GAP_MAX_WHO.load(Relaxed))
}

/// `worst_holder=`: `silent/pass_max_ms=<n>` for a silent worst gap (the pad's), else the holder word as before.
struct Classed<T: core::fmt::Display>(bool, T);
impl<T: core::fmt::Display> core::fmt::Display for Classed<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.0 { write!(f, "silent/pass_max_ms={}", GAP_MAX_PASS.load(Relaxed)) } else { self.1.fmt(f) }
    }
}

/// The `[ptrfeel] gap` line's tail: ` pass_max_ms=<n> cause=<starved|silent>`.
struct GapClass(usize);
impl core::fmt::Display for GapClass {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let p = GAP_PASS.get(self.0).map(|a| a.load(Relaxed)).unwrap_or(u64::MAX);
        if p == u64::MAX {
            return Ok(());
        }
        let held = GAP_WHO.get(self.0).map(|w| w.load(Relaxed) >> 32 != 0).unwrap_or(false);
        write!(f, " pass_max_ms={} cause={}", p, if p > BOUND_MS || held { "starved" } else { "silent" })
    }
}

// ── POINTERSCALE2 (rmbp-ledger B554) — the report RATE against what the endpoint promises. FILE-TAIL append. ──────────────
//
// THE WIRE (flight 33 boot 2, a finger moving the whole window): `reports=244 rate_hz=48.8 … silent=3 silent_max_ms=1001
// pass_max_ms=2 -> PASS` — the fixture passed a stream that delivered under half the pad's frames, because PUMPHOLE's
// rule only charges the kernel a gap its own pass caused and a "silent" gap is called the pad's. The rate itself is the
// reading Peter feels ("mouse even less responsive than ever!"), so it is now IN the verdict: the vendor endpoint's
// bInterval (USB 2.0 §9.6.6: FS/LS frames, HS 2^(b-1) microframes; recorded at arm time by `drivers/ehci/mod.rs`'
// `configure_report_pointer` through [`note_ep`]) gives `expected_hz=` — capped at 1000 because the periodic QH is armed
// S-mask 0x01, one transaction per 1 ms frame — and the window FAILs when `rate_hz` is below [`RATE_PCT_MIN`] % of it.
// Wire: ` expected_hz=<n> binterval=<b> speed=<fs|hs> rate_pct=<n> rate=<ok|LOW>` on the verdict line (`expected_hz=-
// rate=unrated` when no vendor endpoint was armed this boot — the legacy route already fails the verdict).

/// The least share of the endpoint's bInterval rate a finger-down window must deliver.
pub const RATE_PCT_MIN: u64 = 80;
/// The vendor trackpad endpoint's bInterval (0 = none armed) and whether it is high-speed.
static EP_BINTERVAL: AtomicU32 = AtomicU32::new(0);
static EP_HS: AtomicBool = AtomicBool::new(false);

/// `configure_report_pointer`: the vendor-multitouch endpoint is armed with this `bInterval` at this speed.
pub fn note_ep(binterval: u8, hs: bool) {
    EP_HS.store(hs, Relaxed);
    EP_BINTERVAL.store(binterval as u32, Relaxed);
}

/// The pad's OWN frame cadence (seat, at the fold of B554): the bInterval is the host's POLL rate, not the pad's
/// report rate — a vendor-mode Apple pad answers NAK on every poll that has no new frame and produces frames at
/// about 125 Hz (flights 32/33 saw 49 Hz reach the kernel, which is the shortfall this verdict measures). A 500 Hz
/// bar from `bInterval=2` at full speed would fail every boot by construction; the bar is the pad's cadence.
pub const PAD_FRAME_HZ: u64 = 125;

/// Polls per second the endpoint's bInterval promises, at most one per 1 ms frame (the QH's S-mask). 0 = unknown.
pub fn poll_hz() -> u64 {
    let b = EP_BINTERVAL.load(Relaxed) as u64;
    if b == 0 {
        return 0;
    }
    let hz = if EP_HS.load(Relaxed) { 8000 >> (b.min(16) - 1) } else { 1000 / b };
    hz.clamp(1, 1000)
}

/// Reports per second a finger-down window is expected to deliver: the pad's cadence, never more than the poll rate. 0 = unknown.
pub fn expected_hz() -> u64 {
    let p = poll_hz();
    if p == 0 { 0 } else { p.min(PAD_FRAME_HZ) }
}

/// Is a window's rate (tenths of Hz) at least [`RATE_PCT_MIN`] % of `expected` Hz? An unknown expectation does not rate. Pure.
pub fn rate_ok(rate_x10: u64, expected: u64) -> bool {
    expected == 0 || rate_x10 * 100 >= expected * 10 * RATE_PCT_MIN
}

/// ` expected_hz=<n> binterval=<b> speed=<fs|hs> rate_pct=<n> rate=<ok|LOW>` for a window rate in tenths of Hz.
struct RateWord(u64);
impl core::fmt::Display for RateWord {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let e = expected_hz();
        if e == 0 {
            return f.write_str("expected_hz=- rate=unrated");
        }
        write!(
            f, "expected_hz={} poll_hz={} binterval={} speed={} rate_pct={} rate={}",
            e, poll_hz(), EP_BINTERVAL.load(Relaxed), if EP_HS.load(Relaxed) { "hs" } else { "fs" }, self.0 * 10 / e,
            if rate_ok(self.0, e) { "ok" } else { "LOW" }
        )
    }
}
