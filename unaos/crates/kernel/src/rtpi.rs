// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! `[rtpi]` — the PRIORITY-INHERITANCE witness (R1 of the real-time ladder).
//!
//! The companion to `[rtwit]` (R0). Where `rtwit` is a pure MEASUREMENT ruler that changes no
//! behaviour, this module is the observability half of a BEHAVIOUR change: priority inheritance on
//! the x86 sleeping `Mutex` (`arch::x86_64::sched`). The mechanism lives in `sched.rs`; this module
//! only COUNTS what it does, so the counters and the mechanism gate on the same feature and the
//! call sites stay `#[cfg]`-free.
//!
//! ## What priority inheritance fixes
//! x86's scheduler has strict priority + round-robin + anti-starvation aging, but NO priority
//! inheritance. A low-priority task holding a sleeping `Mutex` that a high-priority task needs can
//! be preempted by any number of MID-priority tasks, so the high task waits for the low task PLUS
//! every mid task that runs in front of it — unbounded priority inversion, the classic real-time
//! killer. Inheritance bounds it: while a strictly-higher task blocks on a `Mutex`, the holder
//! INHERITS the blocker's priority (transitively down a chain of holders), so mid-priority tasks
//! can no longer preempt the critical section; the inversion is bounded to ONE hold.
//!
//! ## Where the donation lives (reclamation safety)
//! The inherited floor is stored on the LONG-LIVED lock (a per-`Mutex` `boost`), never on the
//! holder's `Task` (which `run()` frees the instant it exits). A donor therefore only touches lock
//! control blocks, never a `Task` — closing the cross-CPU use-after-free a Task-targeted design has.
//! The holder's effective priority folds in the `boost` of every lock it holds.
//!
//! ## The instruments (all per ~5 s rollup span, resets each line)
//! 1. **`inherits`** — donation EVENTS this span: each time a blocked task raised a lock's inherited
//!    `boost` (the fast path where the lock already carried a `>=` boost does NOT count). Reads `0`
//!    honestly when no inversion occurred — a machine with no contention prints `inherits=0`.
//! 2. **`max_jump`** — the largest single priority JUMP a lock's boost took this span, in levels
//!    (`new − old`). The tail: the worst inversion inheritance had to correct. `--` when
//!    `inherits==0`.
//! 3. **`chain_max`** — the deepest TRANSITIVE chain walked this span (1 = the lock the blocker waits
//!    on directly, 2 = its holder was itself blocked on a second lock, …). Proves transitive
//!    inheritance is live and how deep it reached. `--` when `inherits==0`.
//! 4. **`active`** — a LIVE gauge (NOT per-span): how many LOCKS currently carry a non-zero `boost`
//!    (are contended-and-boosted) right now. It is the leak witness: because a lock's boost is reset
//!    the instant its last blocked waiter leaves (`nwait == 0`), at idle it MUST read `0`. A
//!    persistent non-zero `active` with `inherits=0` for many spans is the leak the gauge exposes.
//!    One BENIGN residue is possible: a donor following a momentarily stale `owner_waits` uplink can
//!    boost a lock that has already gone idle, stranding that boost (and one `active` count) until
//!    the lock's NEXT waiterless acquire clears it — so a small `active > 0` with `inherits=0` that
//!    drains on subsequent lock traffic is that residue, not a leak. The leak signature is `active`
//!    that never drains.
//!
//! Plus a rate-limited per-event trace `[rtpi] inherit c{old}->c{new} depth={d}` for the first
//! `TRACE_MAX` donations of each span, so a boot shows the actual inheritance events, not only the
//! rollup totals.
//!
//! ## Honesty
//! - `inherits` is an exact count; `max_jump` / `chain_max` are tails (MAXes), never means.
//! - A span with no donation prints `inherits=0 max_jump=-- chain_max=-- active=<gauge>` — the
//!   `--` sentinels distinguish "no inversion happened" from "a 0-level jump", and `active` reports
//!   the live leak gauge regardless.
//!
//! ## Gating — zero cost when off
//! The whole battery lives behind `feature = "rtpi"` (`UNAOS_RTPI=1`) AND `target_arch = "x86_64"`.
//! When off, every entry point degrades to an empty `#[inline(always)]` shim, and — crucially —
//! the `sched.rs` MECHANISM it witnesses is itself `#[cfg(feature = "rtpi")]`-gated (the `Task` /
//! `Mutex` PI fields do not exist and `Mutex::lock` takes its original single-`wait()` path), so a
//! knob-off build is byte-identical to the pre-arc scheduler. Follows the `rtwit` pattern.

// ─────────────────────────────────────────────────────────────────────────────────────────────
// REAL IMPLEMENTATION — x86 with the knob armed.
// ─────────────────────────────────────────────────────────────────────────────────────────────
#[cfg(all(feature = "rtpi", target_arch = "x86_64"))]
mod imp {
    use core::sync::atomic::{AtomicI64, AtomicU32, AtomicU64, Ordering::Relaxed};

    /// Donation events (a lock's inherited `boost` actually raised) this span.
    static INHERITS: AtomicU64 = AtomicU64::new(0);
    /// Largest single priority jump (`new - old`, in levels) donated this span.
    static MAX_JUMP: AtomicU64 = AtomicU64::new(0);
    /// Deepest transitive chain walked this span (1 = direct holder).
    static CHAIN_MAX: AtomicU64 = AtomicU64::new(0);
    /// LIVE gauge: LOCKS currently carrying a non-zero inherited `boost` (a contended, boosted lock).
    /// The leak witness; persists across spans. Because the boost lives on the long-lived lock and is
    /// reset the instant its last blocked waiter leaves (`nwait == 0`), at idle this MUST read 0 — a
    /// persistent non-zero with `inherits=0` for many spans is the leak the gauge exists to expose.
    /// Signed so an accounting bug reads negative (visible) rather than wrapping huge.
    static ACTIVE: AtomicI64 = AtomicI64::new(0);
    /// Per-span trace budget for the `[rtpi] inherit …` event lines.
    const TRACE_MAX: u32 = 32;
    static TRACE_COUNT: AtomicU32 = AtomicU32::new(0);

    /// Record one donation event: a lock's inherited `boost` was raised from level `old` to `new`,
    /// `depth` hops down the transitive holder chain (1 = the lock the blocker waits on directly).
    #[inline]
    pub fn note_inherit(old: u8, new: u8, depth: u32) {
        INHERITS.fetch_add(1, Relaxed);
        MAX_JUMP.fetch_max(new.saturating_sub(old) as u64, Relaxed);
        CHAIN_MAX.fetch_max(depth as u64, Relaxed);
        if TRACE_COUNT.fetch_add(1, Relaxed) < TRACE_MAX {
            serial_println!("[rtpi] inherit c{}->c{} depth={}", old, new, depth);
        }
    }

    /// A lock's `boost` crossed 0 → positive (it acquired its first blocked waiter). Bumps the live
    /// `active` gauge of currently-boosted locks.
    #[inline]
    pub fn note_boost_begin() {
        ACTIVE.fetch_add(1, Relaxed);
    }

    /// A lock's `boost` was reset to 0 (its last blocked waiter left). Drops the live `active` gauge.
    #[inline]
    pub fn note_boost_end() {
        ACTIVE.fetch_sub(1, Relaxed);
    }

    /// Emit the `[rtpi]` rollup line and reset the per-span slots. `active` is NOT reset — it is a
    /// live gauge. Called from the ~5 s witness gate alongside `rtwit::rollup`.
    pub fn rollup() {
        let n = INHERITS.swap(0, Relaxed);
        let jump = MAX_JUMP.swap(0, Relaxed);
        let chain = CHAIN_MAX.swap(0, Relaxed);
        TRACE_COUNT.store(0, Relaxed);
        let active = ACTIVE.load(Relaxed);
        if n == 0 {
            // No inversion this span — read zero honestly; the tails are `--`, not a fabricated 0.
            serial_println!(
                "[rtpi] inherits=0 max_jump=-- chain_max=-- active={}",
                active
            );
        } else {
            serial_println!(
                "[rtpi] inherits={} max_jump={} chain_max={} active={}",
                n,
                jump,
                chain,
                active
            );
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// SHIM — feature off, or any non-x86 arch. Every entry point is an empty inline no-op, so the
// knob-off build is byte-inert (and the `sched.rs` mechanism is separately `#[cfg]`-gated off).
// ─────────────────────────────────────────────────────────────────────────────────────────────
#[cfg(not(all(feature = "rtpi", target_arch = "x86_64")))]
mod imp {
    #[inline(always)]
    pub fn note_inherit(_old: u8, _new: u8, _depth: u32) {}
    #[inline(always)]
    pub fn note_boost_begin() {}
    #[inline(always)]
    pub fn note_boost_end() {}
    #[inline(always)]
    pub fn rollup() {}
}

pub use imp::{note_boost_begin, note_boost_end, note_inherit, rollup};
