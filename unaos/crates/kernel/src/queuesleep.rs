// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — kernel-by-ruling (B512 QUEUESLEEP, R103 §2: the input path wakes on input, measured on an idle desktop)
//!
//! QUEUESLEEP (rmbp-ledger B512) — flight 28, Peter at an idle desktop: "what's going on. there's been a long stall";
//! the wire read `stage=pump stage_ms=28440`, `stage=queue stage_ms=3090` and `35413`. `video::lag`'s QUEUESLEEP section
//! stamps every wait on the input path; this fixture reads those stamps over an idle window and then times one key.
//!
//! `tests queuesleep` (R80: only when asked). QUEUEWEDGE (B532): the window ALWAYS runs on its own `queuesleep` task
//! — typed, it opens at once; under `tests-at-boot`, once the desktop is up — so the verb returns in microseconds.
//! Flight 30 ran the 30 s window ON the `shell-job` task inside `tests::run`: for 33 s it held the shell (the job's
//! queue `busy`, so `tests loginrender` queued behind it) and the `tests` table (`tests::RUNNING`, every other `tests`
//! refused). It held no lock and nothing the compositor takes — the `stage=comp` seconds were the headless door's own
//! keys, routed to a console that never draws, charged `comp` until the 3 s timeout on a desktop with no pass. The typed
//! verb prints `:: QUEUESLEEP: armed idle_s=30 on=task cpu=<n> via=tests -> SKIP reason=verdict-follows ::` and the
//! verdict line below follows from the window task; TESTWATCH (`tests_watch.rs`) watches that task, stamped every pass.
//!
//! `:: QUEUESLEEP: idle_s=30 wake_ms=<n> pump_max_ms=<n> queue_max_ms=<n> pump_wait_worst=<word> -> PASS|FAIL ::
//!  bound_ms=50 pump_what_worst=<step> xhci_poll_gap_max_ms=<n> claim_busy=<n> nap_max_ms=<n> queue_what=<..>
//!  queue_wait=<..> via=<tests|boot>`
//!
//! * the window: [`IDLE_S`] seconds with NO synthetic traffic — `pump_max_ms` is the longest `usb-pump` interval (a step,
//!   or the loop's nap past its tick), `queue_max_ms` the longest a key or press sat in the queue (finished events and
//!   any still in flight, read directly so a router that never runs cannot hide one);
//! * then ONE key through the door a real key takes (`pal::push_event`, the funnel the xHCI decoder pushes its own
//!   `KeyUp` through, drivers/xhci/mod.rs): a `KeyUp(lag::PROBE_KEY)` — a key edge no keyboard sends and every consumer
//!   ignores — and `wake_ms` is its push → the router taking it (`lag::route`), the whole input path: the ring, the
//!   `input` task, the GUI channel, the render task. The push is one masked ring-lock section; the wait for the route
//!   is a bounded poll on the window task ([`WAKE_WAIT_MS`]) — nothing else ever waits on it;
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
    if RUNNING.load(Ordering::Acquire) {
        serial_println!(":: QUEUESLEEP: -> SKIP :: reason=already-running bound_ms={}", BOUND_MS);
        return;
    }
    spawn_window(cfg!(feature = "tests-at-boot"));
}

/// QUEUEWEDGE (B532): the window's own task, typed or at boot — the caller (the `shell-job` task inside `tests::run`)
/// returns at once and holds nothing for the window's 30 s.
#[cfg(target_arch = "x86_64")]
fn spawn_window(boot: bool) {
    let cpu = crate::arch::smp::worker_cpu(0).unwrap_or_else(crate::arch::sched::meter_current_cpu);
    crate::arch::sched::spawn("queuesleep", window_task, boot as usize, cpu, crate::arch::sched::PRIO_NORMAL);
    if boot {
        serial_println!(":: QUEUESLEEP: armed idle_s={} on=task cpu={} (tests-at-boot: the window opens {} ms after phase=desktop) ::", IDLE_S, cpu, SETTLE_MS);
    } else {
        serial_println!(
            ":: QUEUESLEEP: armed idle_s={} on=task cpu={} via=tests -> SKIP reason=verdict-follows :: (the window runs on its own task; the shell and the tests table are free; the verdict line follows in ~{} s)",
            IDLE_S, cpu, IDLE_S + WAKE_WAIT_MS / 1000
        );
    }
}

#[cfg(not(target_arch = "x86_64"))]
fn spawn_window(_boot: bool) {
    run("tests"); // unreachable in practice (`qs_on` is x86 + `wc`); keeps one caller on every arch
}

#[cfg(target_arch = "x86_64")]
fn window_task(boot: usize) {
    if boot != 0 {
        let t0 = crate::arch::ms();
        while crate::boot::phase() != crate::boot::Phase::Desktop && crate::arch::ms().saturating_sub(t0) < DESK_WAIT_MS {
            crate::arch::sched::sleep_ms(100);
        }
        crate::arch::sched::sleep_ms(SETTLE_MS);
    }
    watch_arm();
    run(if boot != 0 { "boot" } else { "tests" });
    watch_disarm();
    crate::arch::sched::exit();
}

/// TESTWATCH (B532): the window task is watched from a sibling core while it runs, and stamps every pass.
fn watch_arm() {
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    crate::tests_watch::arm("tests queuesleep");
}

fn watch_disarm() {
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    crate::tests_watch::disarm();
}

fn watch_stamp() {
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    crate::tests_watch::stamp();
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
    lag::qs_window_reset(); let busy0 = Busy::now(); let mut held: Option<(alloc::string::String, &'static str)> = None; // QUEUESLEEP2 (B557): the window's operator traffic, and the holder named AT the worst sample (same-line fold)
    let t0 = crate::arch::ms();
    let mut q_seen = 0u64;
    while crate::arch::ms().saturating_sub(t0) < IDLE_S * 1000 {
        sleep_ms(100);
        let s = lag::qs_queue_now_us(); if s > q_seen { q_seen = s; held = Some(lag::qs_queue_word(0)); } // QUEUESLEEP2 (B557): the word is read WHILE the key sits, not after the window
        watch_stamp();
    }
    let (pump, q_us, xgap_us, busy, nap_us) = lag::qs_window();
    let q_us = q_us.max(q_seen).max(lag::qs_queue_now_us());
    let (p_us, p_step, p_code) = lag::qs_unpack(pump);
    let (e_what, e_wait) = lag::qs_queue_word(0); let traffic = Busy::now().since(&busy0); let (q_what, q_wait) = held_word(&held, q_seen, q_us); // QUEUESLEEP2 (B557): `queue_end` is the old verdict-time reading (the render loop's idle end state)
    // One key through the synthetic door; the router's stamp says when the whole path took it.
    let t = lag::qs_probe_arm();
    crate::pal::push_event(crate::pal::Event::KeyUp(lag::PROBE_KEY));
    let w0 = crate::arch::ms();
    let mut done = 0u64;
    while done == 0 && crate::arch::ms().saturating_sub(w0) < WAKE_WAIT_MS {
        sleep_tick();
        done = lag::qs_probe_done();
        watch_stamp();
    }
    let wake_ms = if done == 0 { None } else { Some(done.saturating_sub(t) / 1000) };
    let (pump_ms, queue_ms) = (p_us / 1000, q_us / 1000);
    // The worst wait: the queue's word when the queue was the worse reading and over the bound, else the pump's.
    let worst = if queue_ms > pump_ms && queue_ms > BOUND_MS { q_wait } else { lag::wait_name(p_code) };
    let (tw_what, tw_wait) = if done == 0 { lag::qs_queue_word(0) } else { (alloc::string::String::from("-"), "-") };
    serial_println!(
        ":: QUEUESLEEP: idle_s={} wake_ms={} pump_max_ms={} queue_max_ms={} pump_wait_worst={} -> {} :: bound_ms={} pump_what_worst={} xhci_poll_gap_max_ms={} claim_busy={} nap_max_ms={} queue_what={} queue_wait={} timeout_what={} timeout_wait={} via={} queue_end={}/{} {}",
        IDLE_S, WakeWord(wake_ms), pump_ms, queue_ms, worst, if verdict(wake_ms, pump_ms, queue_ms) { "PASS" } else { "FAIL" },
        BOUND_MS, lag::qs_step_name(p_step), xgap_us / 1000, busy, nap_us / 1000, q_what, q_wait, tw_what, tw_wait, via, e_what, e_wait, traffic
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

// QUEUESLEEP2 (rmbp-ledger B557) — the measurement, made to say what it saw. FILE TAIL, so no line above moves.
// Flight 33 (FLIGHT33.md §3) read `queue_max_ms=73 queue_what=render@shell-present queue_wait=hlt pump_what_worst=prtscr`
// and the window was NOT idle: Cmd-Shift-3 at +9 s (`PRTSCR: chord=cmd-shift-3 -> capture armed`, the PRTSCR-VOL ladder
// and its refusal on the pump: the 23 ms `prtscr` step) and two dock presses at +26 s (Quarry's launch, `handler=console-launch
// handler_ms=496`, and the raise press behind it, `[lag] click→window-shown ms=121.5 queue=73.0`). `queue_what` was read
// ONCE, after the window, so it named the render loop's idle END state (parked after its last mark, `shell-present`), never
// the step that held the 73 ms. Now: the word is taken at the worst in-flight sample (`held`), `queue_end=` keeps the old
// reading, and `window=idle|busy` with the operator's own traffic over the 30 s says whether the window measured an idle
// desktop at all (R103 §2: this fixture's premise). The verdict rule is unchanged.

/// The window's operator traffic, as the counters already kept elsewhere read it.
struct Busy {
    keys: u64,
    ptr: u64,
    chords: u32,
    stalls: u32,
}

impl Busy {
    fn now() -> Busy {
        let (ptr, keys, _, _, _) = crate::pal::event_queue_stats();
        Busy { keys, ptr, chords: crate::video::prtscr::census().0, stalls: lag::handler_stalls() }
    }

    fn since(&self, t0: &Busy) -> Busy {
        Busy {
            keys: self.keys.wrapping_sub(t0.keys),
            ptr: self.ptr.wrapping_sub(t0.ptr),
            chords: self.chords.wrapping_sub(t0.chords),
            stalls: self.stalls.wrapping_sub(t0.stalls),
        }
    }

    /// No key, pointer report, capture chord or render-handler stall second in the window. Pure.
    fn idle(&self) -> bool {
        self.keys == 0 && self.ptr == 0 && self.chords == 0 && self.stalls == 0
    }
}

impl core::fmt::Display for Busy {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "window={} keys={} ptr={} capture_chords={} handler_stalls={}",
            if self.idle() { "idle" } else { "busy" },
            self.keys, self.ptr, self.chords, self.stalls
        )
    }
}

/// `(queue_what, queue_wait)`: the word read at the worst in-flight sample when that sample IS the window's queue max
/// (`seen_us >= max_us`); `unsampled` when the worst key finished between two 100 ms samples (its holder was never
/// observed in flight — `queue_end` and the `[lag] stall` line of that second name it); `-` for an empty window.
fn held_word(held: &Option<(alloc::string::String, &'static str)>, seen_us: u64, max_us: u64) -> (alloc::string::String, &'static str) {
    match held {
        Some((w, c)) if seen_us >= max_us => (w.clone(), c),
        _ if max_us == 0 => (alloc::string::String::from("-"), "-"),
        _ => (alloc::string::String::from("unsampled"), "-"),
    }
}
