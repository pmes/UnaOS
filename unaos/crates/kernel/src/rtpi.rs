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
//! ## The instruments (all per ~5 s rollup span, resets each line)
//! 1. **`inherits`** — donation EVENTS this span: each time a holder's effective priority was raised
//!    by a blocker (the fast path where the holder was already at/above the blocker does NOT count).
//!    Reads `0` honestly when no inversion occurred — a machine with no contention prints
//!    `inherits=0`, not a fabricated number.
//! 2. **`max_jump`** — the largest single priority JUMP donated this span, in levels
//!    (`to − from`). The tail: the worst inversion inheritance had to correct. `--` when
//!    `inherits==0`.
//! 3. **`chain_max`** — the deepest TRANSITIVE chain walked this span (1 = direct holder, 2 = the
//!    holder was itself blocked on a second lock, …). Proves transitive inheritance is live and how
//!    deep it reached. `--` when `inherits==0`.
//! 4. **`active`** — a LIVE gauge (NOT per-span): how many tasks currently carry a non-zero donated
//!    priority right now. It is the leak witness: at idle, with no lock held under contention, it
//!    MUST read `0`. A persistent non-zero `active` with `inherits=0` for many spans is a priority
//!    leak and the instrument is built to make exactly that visible.
//!
//! Plus a rate-limited per-event trace `[rtpi] inherit c{from}->c{to} depth={d}` for the first
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

    /// Donation events (a holder's effective priority actually raised) this span.
    static INHERITS: AtomicU64 = AtomicU64::new(0);
    /// Largest single priority jump (`to - from`, in levels) donated this span.
    static MAX_JUMP: AtomicU64 = AtomicU64::new(0);
    /// Deepest transitive chain walked this span (1 = direct holder).
    static CHAIN_MAX: AtomicU64 = AtomicU64::new(0);
    /// LIVE gauge: tasks currently carrying a non-zero donated priority. The leak witness; persists
    /// across spans. Signed so an accounting bug reads negative (visible) rather than wrapping huge.
    static ACTIVE: AtomicI64 = AtomicI64::new(0);
    /// Per-span trace budget for the `[rtpi] inherit …` event lines.
    const TRACE_MAX: u32 = 32;
    static TRACE_COUNT: AtomicU32 = AtomicU32::new(0);

    /// Record one donation event: a holder at effective level `from` was raised to `to` (levels),
    /// `depth` hops down the transitive holder chain (1 = the lock's direct holder). `newly_active`
    /// is true iff this raise took the holder from unboosted (effective == base) to boosted — used
    /// to keep the `active` leak gauge a clean count of currently-boosted tasks.
    #[inline]
    pub fn note_inherit(from: u8, to: u8, depth: u32, newly_active: bool) {
        INHERITS.fetch_add(1, Relaxed);
        MAX_JUMP.fetch_max(to.saturating_sub(from) as u64, Relaxed);
        CHAIN_MAX.fetch_max(depth as u64, Relaxed);
        if newly_active {
            ACTIVE.fetch_add(1, Relaxed);
        }
        if TRACE_COUNT.fetch_add(1, Relaxed) < TRACE_MAX {
            serial_println!("[rtpi] inherit c{}->c{} depth={}", from, to, depth);
        }
    }

    /// Record that a task's donated priority was reverted to base (it released its last PI lock, so
    /// it no longer carries a boost). Decrements the live `active` gauge. Called once per boosted
    /// task that fully reverts.
    #[inline]
    pub fn note_revert() {
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
    pub fn note_inherit(_from: u8, _to: u8, _depth: u32, _newly_active: bool) {}
    #[inline(always)]
    pub fn note_revert() {}
    #[inline(always)]
    pub fn rollup() {}
}

pub use imp::{note_inherit, note_revert, rollup};
