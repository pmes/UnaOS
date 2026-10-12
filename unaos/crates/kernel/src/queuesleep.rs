// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — kernel-by-ruling (B512 QUEUESLEEP, R103 §2: the input path wakes on input, measured on an idle desktop)
//!
//! QUEUESLEEP (rmbp-ledger B512) — flight 28, Peter at an idle desktop: "what's going on. there's been a long stall";
//! the wire read `stage=pump stage_ms=28440`, `stage=queue stage_ms=3090` and `35413`. `video::lag`'s QUEUESLEEP section
//! stamps every wait on the input path; this fixture reads those stamps over an idle window and then times one key.
//!
//! `tests queuesleep` (R80: only when asked; under `tests-at-boot` the window runs on its own task once the desktop is
//! up, so the registering context is never held):
//!
//! `:: QUEUESLEEP: idle_s=30 wake_ms=<n> pump_max_ms=<n> queue_max_ms=<n> pump_wait_worst=<word> -> PASS|FAIL ::
//!  bound_ms=50 pump_what_worst=<step> xhci_poll_gap_max_ms=<n> claim_busy=<n> nap_max_ms=<n> queue_what=<..>
//!  queue_wait=<..> via=<tests|boot>`
//!
//! * the window: [`IDLE_S`] seconds with NO synthetic traffic — `pump_max_ms` is the longest `usb-pump` interval (a step,
//!   or the loop's nap past its tick), `queue_max_ms` the longest a key or press sat in the queue (finished events and
//!   any still in flight, read directly so a router that never runs cannot hide one);
//! * then ONE key through the synthetic door every fixture uses (`pal::push_event`): a `KeyUp(lag::PROBE_KEY)` — a key
//!   edge no keyboard sends and every consumer ignores — and `wake_ms` is its push → the router taking it (`lag::route`),
//!   the whole input path: the ring, the `input` task, the GUI channel, the render task;
//! * PASS iff `wake_ms`, `pump_max_ms` and `queue_max_ms` are all ≤ [`BOUND_MS`]. `pump_wait_worst` names what the input
//!   was waiting on during the worst interval (`run` = nothing: the step held only the loop).

use core::sync::atomic::{AtomicBool, Ordering};

use crate::video::lag;

/// The idle window, seconds.
pub const IDLE_S: u64 = 30;
/// Every bound the verdict reads, ms (INPUTSTALL's 50).
pub const BOUND_MS: u64 = 50;
/// How long the probe key may take before it is a timeout.
const WAKE_WAIT_MS: u64 = 3_000;
/// Under `tests-at-boot`: after the desktop phase, let the boot's own one-shots finish before the window opens.
const SETTLE_MS: u64 = 5_000;
/// Under `tests-at-boot`: the longest the task waits for the desktop phase.
const DESK_WAIT_MS: u64 = 180_000;

static RUNNING: AtomicBool = AtomicBool::new(false);

/// `tests queuesleep`.
pub fn selftest() {
    if !lag::qs_on() {
        serial_println!(":: QUEUESLEEP: -> SKIP :: reason=no-wc (the stamps live on the x86 wc input path) bound_ms={}", BOUND_MS);
        return;
    }
    if cfg!(feature = "tests-at-boot") {
        spawn_window();
        return;
    }
    run("tests");
}

#[cfg(target_arch = "x86_64")]
fn spawn_window() {
    let cpu = crate::arch::smp::worker_cpu(0).unwrap_or_else(crate::arch::sched::meter_current_cpu);
    crate::arch::sched::spawn("queuesleep", boot_window, 0, cpu, crate::arch::sched::PRIO_NORMAL);
    serial_println!(":: QUEUESLEEP: armed idle_s={} on=task cpu={} (tests-at-boot: the window opens {} ms after phase=desktop) ::", IDLE_S, cpu, SETTLE_MS);
}

#[cfg(not(target_arch = "x86_64"))]
fn spawn_window() {}

#[cfg(target_arch = "x86_64")]
fn boot_window(_: usize) {
    let t0 = crate::arch::ms();
    while crate::boot::phase() != crate::boot::Phase::Desktop && crate::arch::ms().saturating_sub(t0) < DESK_WAIT_MS {
        crate::arch::sched::sleep_ms(100);
    }
    crate::arch::sched::sleep_ms(SETTLE_MS);
    run("boot");
    crate::arch::sched::exit();
}

fn sleep_ms(ms: u64) {
    #[cfg(target_arch = "x86_64")]
    crate::arch::sched::sleep_ms(ms);
    #[cfg(not(target_arch = "x86_64"))]
    let _ = ms;
}

fn sleep_tick() {
    #[cfg(target_arch = "x86_64")]
    crate::arch::sched::sleep_ticks(1);
}

/// The PASS rule over the three readings (ms). Pure.
pub fn verdict(wake_ms: Option<u64>, pump_max_ms: u64, queue_max_ms: u64) -> bool {
    matches!(wake_ms, Some(w) if w <= BOUND_MS) && pump_max_ms <= BOUND_MS && queue_max_ms <= BOUND_MS
}

fn run(via: &str) {
    if RUNNING.swap(true, Ordering::AcqRel) {
        serial_println!(":: QUEUESLEEP: -> SKIP :: reason=already-running bound_ms={}", BOUND_MS);
        return;
    }
    // The idle window: nothing synthetic, the stamps read what the machine does on its own.
    lag::qs_window_reset();
    let t0 = crate::arch::ms();
    let mut q_seen = 0u64;
    while crate::arch::ms().saturating_sub(t0) < IDLE_S * 1000 {
        sleep_ms(100);
        q_seen = q_seen.max(lag::qs_queue_now_us());
    }
    let (pump, q_us, xgap_us, busy, nap_us) = lag::qs_window();
    let q_us = q_us.max(q_seen).max(lag::qs_queue_now_us());
    let (p_us, p_step, p_code) = lag::qs_unpack(pump);
    let (q_what, q_wait) = lag::qs_queue_word(0);
    // One key through the synthetic door; the router's stamp says when the whole path took it.
    let t = lag::qs_probe_arm();
    crate::pal::push_event(crate::pal::Event::KeyUp(lag::PROBE_KEY));
    let w0 = crate::arch::ms();
    let mut done = 0u64;
    while done == 0 && crate::arch::ms().saturating_sub(w0) < WAKE_WAIT_MS {
        sleep_tick();
        done = lag::qs_probe_done();
    }
    let wake_ms = if done == 0 { None } else { Some(done.saturating_sub(t) / 1000) };
    let (pump_ms, queue_ms) = (p_us / 1000, q_us / 1000);
    // The worst wait: the queue's word when the queue was the worse reading and over the bound, else the pump's.
    let worst = if queue_ms > pump_ms && queue_ms > BOUND_MS { q_wait } else { lag::wait_name(p_code) };
    let (tw_what, tw_wait) = if done == 0 { lag::qs_queue_word(0) } else { (alloc::string::String::from("-"), "-") };
    serial_println!(
        ":: QUEUESLEEP: idle_s={} wake_ms={} pump_max_ms={} queue_max_ms={} pump_wait_worst={} -> {} :: bound_ms={} pump_what_worst={} xhci_poll_gap_max_ms={} claim_busy={} nap_max_ms={} queue_what={} queue_wait={} timeout_what={} timeout_wait={} via={}",
        IDLE_S, WakeWord(wake_ms), pump_ms, queue_ms, worst, if verdict(wake_ms, pump_ms, queue_ms) { "PASS" } else { "FAIL" },
        BOUND_MS, lag::qs_step_name(p_step), xgap_us / 1000, busy, nap_us / 1000, q_what, q_wait, tw_what, tw_wait, via
    );
    RUNNING.store(false, Ordering::Release);
}

struct WakeWord(Option<u64>);
impl core::fmt::Display for WakeWord {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.0 {
            Some(ms) => write!(f, "{}", ms),
            None => write!(f, "timeout"),
        }
    }
}
