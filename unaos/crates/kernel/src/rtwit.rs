// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! `[rtwit]` — the WORST-CASE RULER (R0 of the real-time ladder).
//!
//! A pure measurement instrument. It changes NO scheduling, locking, or present behaviour: every
//! hook is a `now_cycles()` read plus a `fetch_max` (and, for `in2present`, one histogram bump).
//! It can therefore never regress a behaviour — it only makes the tail visible.
//!
//! ## Why it exists
//! Every hot-path number on the machine today is a MEAN — `[wcn] rate=`, `[comp2] compose_us=`,
//! `[wpace] rate=`, `[schedx86] load c=%`. Real-time lives in the TAIL: the single longest
//! input→present, the single longest lock hold, the single longest interrupt-mask span. You cannot
//! bound what you only measure on average. This module reports MAXes and a p99-ish high-water,
//! never a mean.
//!
//! ## The three instruments
//! 1. **`in2present`** — input→present latency, measured as the tightest HONEST PROXY:
//!    `note_input_enqueued()` stamps the TSC of the FIRST input to land after the previous present
//!    (a single "oldest un-presented input" slot); `note_present_composited()` reads the delta when
//!    a frame actually composites and resets the slot. Reported as `in2present_max_us` /
//!    `in2present_p99_us`.
//!    - IT CAPTURES: the age of the oldest input awaiting a composited frame — the enqueue→present
//!      wait, the front half of the input→photon chain.
//!    - IT DOES NOT CAPTURE: (a) that the composited frame actually *rendered* that specific input
//!      (we assume the next composite reflects it — it may over- or under-attribute); (b) any
//!      photon/scanout latency AFTER the present syscall returns; (c) a per-event distribution —
//!      only the oldest input per present batch is timed.
//!    - `in2present_p99bkt_us` is the UPPER BOUND of a coarse power-of-2 µs bucket (not an exact
//!      percentile); `in2present_max_us` is the exact tail so no worst case is lost.
//! 2. **per-lock max hold** — `hold(Lock)` returns an RAII timer whose `Drop` folds the critical
//!    section's TSC delta into a per-lock `fetch_max`. Instrumented locks: STAGE, TABLE,
//!    EVENT_QUEUE, WINDOWS. Reported as `stage_max_us` / `table_max_us` / `evq_max_us` /
//!    `windows_max_us`. A lock whose max-hold scales with work is the RT enemy; this makes it
//!    visible without changing acquire order or blocking semantics.
//! 3. **max interrupt-mask span** — `note_mask_span(cycles)` is called from the OUTERMOST transition
//!    (enabled→disabled→enabled) of every SOFTWARE mask primitive on x86: `without_interrupts` (which
//!    now masks via `IrqMask` so a panicking closure still restores IF), the `IrqMask` RAII, AND the
//!    syscall layer's `IrqGuard` RAII. Nested masks are not double-counted. Reported as
//!    `irqmask_max_us`. This is the floor under every RT bound.
//!    - ⚠ SFMASK GAP — NAMED so nobody tunes RT off an under-count: a ring-3 SYSCALL runs its ENTIRE
//!      body IF-masked by the CPU's syscall-entry SFMASK (not by any guard above), so the present
//!      path's own masked span is the syscall body, which this field does NOT include (on a syscall
//!      path `IrqGuard`/`IrqMask` find IF already 0 and time nothing). Capturing that honestly needs
//!      a syscall entry/exit instrument that discounts the context switches a blocking syscall makes
//!      (e.g. the present path's `sleep_ms`) — otherwise a blocked syscall reads as a false
//!      millisecond-scale "mask". That is a separate later rung; `irqmask_max_us` here is the max of
//!      the software-primitive mask holds only.
//!
//! ## Honesty
//! - Every field is a MAX or high-water, never a mean.
//! - A field that never populated in a span reads the sentinel `--`, never `0` masquerading as
//!   "0µs".
//! - The ruler measures its OWN cost (`ruler_ns=`, the back-to-back `now_cycles()` delta) so a
//!   reader can subtract the perturbation the instrument injects into each span it times.
//! - Every max is PER ROLLUP SPAN: read-and-reset at each `[rtwit]` line, so the value is the worst
//!   case seen in that ~5 s window (a concurrent record between read and reset can lose one sample —
//!   acceptable for an instrument, noted here).
//!
//! ## Gating — zero cost when off
//! The whole battery lives behind `feature = "rtwit"` (`UNAOS_RTWIT=1`) AND `target_arch =
//! "x86_64"`. When off, every entry point degrades to an empty `#[inline(always)]` shim and
//! `HoldTimer` is a zero-sized no-op — so the module is always declared and the call sites in
//! `video/wm.rs`, `pal.rs`, `arch/x86_64/syscall.rs` and `arch/x86_64/mod.rs` stay `#[cfg]`-free.
//! Follows the `wedge2` pattern.

/// Which load-bearing lock a [`HoldTimer`] is timing. Named in both the real and the shim build so
/// the call sites are `#[cfg]`-free.
#[derive(Clone, Copy)]
pub enum Lock {
    Stage,
    Table,
    Evq,
    Windows,
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// REAL IMPLEMENTATION — x86 with the knob armed.
// ─────────────────────────────────────────────────────────────────────────────────────────────
#[cfg(all(feature = "rtwit", target_arch = "x86_64"))]
mod imp {
    use super::Lock;
    use crate::arch::now_cycles;
    use core::sync::atomic::{AtomicU64, Ordering::Relaxed};

    /// The "never populated" marker for the pending-input slot. `u64::MAX` cannot be a real cycle
    /// delta within a rollup span, so a slot still holding it means no sample landed.
    const EMPTY: u64 = u64::MAX;

    /// A worst-case slot: the running MAX (in TSC cycles) and the number of samples folded into it,
    /// both PER SPAN. `count == 0` at rollup means "never populated" and prints the `--` sentinel —
    /// distinguishing a genuine 0-cycle reading from silence.
    struct MaxSlot {
        max: AtomicU64,
        count: AtomicU64,
    }
    impl MaxSlot {
        const fn new() -> Self {
            Self { max: AtomicU64::new(0), count: AtomicU64::new(0) }
        }
        #[inline]
        fn record(&self, cycles: u64) {
            self.count.fetch_add(1, Relaxed);
            self.max.fetch_max(cycles, Relaxed);
        }
        /// Read-and-reset. Returns `Some(max_cycles)` if any sample landed this span, else `None`.
        fn take(&self) -> Option<u64> {
            let n = self.count.swap(0, Relaxed);
            let m = self.max.swap(0, Relaxed);
            if n == 0 {
                None
            } else {
                Some(m)
            }
        }
    }

    // ── instrument 1: input→present proxy ──────────────────────────────────────────────────────
    /// TSC of the oldest input awaiting a composited frame, or `EMPTY` when none is pending.
    static PENDING_INPUT: AtomicU64 = AtomicU64::new(EMPTY);
    static IN2PRESENT: MaxSlot = MaxSlot::new();
    /// Fixed-bucket histogram of `in2present` samples (in µs), for a cheap p99-ish high-water. Bucket
    /// `i` counts samples with `us <= IN2P_BOUNDS[i]`; the trailing bucket is the overflow.
    const IN2P_BOUNDS: [u64; 13] = [1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096];
    static IN2P_HIST: [AtomicU64; 14] = [const { AtomicU64::new(0) }; 14];

    // ── instrument 2: per-lock max hold ────────────────────────────────────────────────────────
    static STAGE_HOLD: MaxSlot = MaxSlot::new();
    static TABLE_HOLD: MaxSlot = MaxSlot::new();
    static EVQ_HOLD: MaxSlot = MaxSlot::new();
    static WINDOWS_HOLD: MaxSlot = MaxSlot::new();

    // ── instrument 3: max interrupt-mask span ──────────────────────────────────────────────────
    static IRQMASK: MaxSlot = MaxSlot::new();

    #[inline]
    fn slot(lock: Lock) -> &'static MaxSlot {
        match lock {
            Lock::Stage => &STAGE_HOLD,
            Lock::Table => &TABLE_HOLD,
            Lock::Evq => &EVQ_HOLD,
            Lock::Windows => &WINDOWS_HOLD,
        }
    }

    /// RAII lock-hold timer. Constructed just after a lock is acquired; its `Drop` (which, by Rust's
    /// reverse-declaration drop order, runs just BEFORE the lock guard's own `Drop` when declared
    /// after it) folds the acquire→release delta into the lock's per-span MAX. It changes nothing
    /// about the lock: same acquire order, same blocking, one extra `now_cycles()` at drop.
    pub struct HoldTimer {
        lock: Lock,
        t0: u64,
    }
    impl Drop for HoldTimer {
        #[inline]
        fn drop(&mut self) {
            let d = now_cycles().wrapping_sub(self.t0);
            slot(self.lock).record(d);
        }
    }

    #[inline]
    pub fn hold(lock: Lock) -> HoldTimer {
        HoldTimer { lock, t0: now_cycles() }
    }

    /// Stamp the arrival of an input event at the EVENT_QUEUE enqueue funnel. Records only the FIRST
    /// input since the last composite (the "oldest un-presented input"); later inputs in the same
    /// batch do not overwrite it, so the reported latency is the age of the oldest one.
    #[inline]
    pub fn note_input_enqueued() {
        // Only claim the slot if it is empty. A relaxed CAS is enough: a lost race just means a
        // near-simultaneous input wins the slot, which is within the proxy's stated resolution.
        let _ = PENDING_INPUT.compare_exchange(EMPTY, now_cycles(), Relaxed, Relaxed);
    }

    /// A frame actually composited. If an input was pending, fold its enqueue→present age into the
    /// `in2present` MAX and histogram, and clear the slot for the next batch.
    #[inline]
    pub fn note_present_composited() {
        let t0 = PENDING_INPUT.swap(EMPTY, Relaxed);
        if t0 == EMPTY {
            return;
        }
        let d = now_cycles().wrapping_sub(t0);
        IN2PRESENT.record(d);
        // Histogram is keyed in µs; if the TSC is not yet calibrated we still bump the overflow
        // bucket so the count is honest, and the p99 line reports `tsc_uncal`.
        let us = cycles_to_us(d).unwrap_or(u64::MAX);
        let mut b = IN2P_BOUNDS.len(); // default: overflow bucket
        for (i, &bound) in IN2P_BOUNDS.iter().enumerate() {
            if us <= bound {
                b = i;
                break;
            }
        }
        IN2P_HIST[b].fetch_add(1, Relaxed);
    }

    /// Fold one OUTERMOST interrupt-mask span (enabled→disabled→enabled) into the per-span MAX.
    /// Called only on the outer transition; nested masks are not double-counted. Fed by every
    /// software mask primitive on x86: `arch::x86_64::IrqMask` (which also backs `without_interrupts`)
    /// and the syscall layer's `IrqGuard`. See the SFMASK-gap note in the module docs for what this
    /// deliberately does NOT cover.
    #[inline]
    pub fn note_mask_span(cycles: u64) {
        IRQMASK.record(cycles);
    }

    /// Convert a TSC-cycle delta to microseconds, or `None` if the TSC is not calibrated.
    fn cycles_to_us(cycles: u64) -> Option<u64> {
        let hz = crate::arch::x86_64::apic::tsc_hz();
        if hz == 0 {
            None
        } else {
            Some(((cycles as u128 * 1_000_000) / hz as u128) as u64)
        }
    }

    /// Read-and-reset the in2present histogram, returning the p99-ish high-water: the upper bound
    /// (µs) of the bucket in which the 99th-percentile sample falls. `None` if no samples landed.
    fn in2p_p99_us() -> Option<u64> {
        let mut counts = [0u64; 14];
        let mut total = 0u64;
        for (i, c) in IN2P_HIST.iter().enumerate() {
            let v = c.swap(0, Relaxed);
            counts[i] = v;
            total += v;
        }
        if total == 0 {
            return None;
        }
        // Smallest bucket whose cumulative count reaches 99% of the samples.
        let threshold = (total * 99).div_ceil(100);
        let mut cum = 0u64;
        for (i, &c) in counts.iter().enumerate() {
            cum += c;
            if cum >= threshold {
                return Some(if i < IN2P_BOUNDS.len() {
                    IN2P_BOUNDS[i]
                } else {
                    // overflow bucket: report the top finite bound as the floor.
                    IN2P_BOUNDS[IN2P_BOUNDS.len() - 1]
                });
            }
        }
        Some(IN2P_BOUNDS[IN2P_BOUNDS.len() - 1])
    }

    /// Measure the ruler's own per-span cost, in nanoseconds. Every timed span brackets its work with
    /// TWO `now_cycles()` reads (one at acquire/enqueue, one at release/present), so the perturbation
    /// injected into the reported value is ~TWO read latencies. We sample the minimum back-to-back
    /// single-read gap and return 2× it — the honest per-span two-read cost a reader subtracts to
    /// recover the true worst case (rather than the single-gap figure, which would understate it by
    /// half).
    fn ruler_ns() -> Option<u64> {
        let hz = crate::arch::x86_64::apic::tsc_hz();
        if hz == 0 {
            return None;
        }
        let mut min = u64::MAX;
        let mut prev = now_cycles();
        for _ in 0..32 {
            let now = now_cycles();
            let d = now.wrapping_sub(prev);
            if d < min {
                min = d;
            }
            prev = now;
        }
        // 2× the single-read gap = the two-read per-span cost.
        Some(((min as u128 * 2 * 1_000_000_000) / hz as u128) as u64)
    }

    /// A bounded, allocation-free line buffer for the `[rtwit]` rollup. `write_str` copies until
    /// full, then silently truncates — the field set is fixed and the cap is comfortably over its
    /// worst-case width, so truncation cannot occur in practice; the buffer just makes the witness
    /// path alloc-free like every other rollup on this wire.
    struct Buf {
        b: [u8; 256],
        n: usize,
    }
    impl Buf {
        fn new() -> Self {
            Self { b: [0; 256], n: 0 }
        }
        fn as_str(&self) -> &str {
            core::str::from_utf8(&self.b[..self.n]).unwrap_or("[rtwit] <utf8-error>")
        }
    }
    impl core::fmt::Write for Buf {
        fn write_str(&mut self, s: &str) -> core::fmt::Result {
            let bytes = s.as_bytes();
            let room = self.b.len() - self.n;
            let take = room.min(bytes.len());
            self.b[self.n..self.n + take].copy_from_slice(&bytes[..take]);
            self.n += take;
            Ok(())
        }
    }

    /// Format a per-span MAX slot as `<us>` or the `--` never-populated sentinel.
    fn fmt_slot(w: &mut impl core::fmt::Write, name: &str, s: &MaxSlot) {
        match s.take() {
            Some(cycles) => match cycles_to_us(cycles) {
                Some(us) => {
                    let _ = write!(w, " {}={}", name, us);
                }
                None => {
                    let _ = write!(w, " {}=tsc_uncal", name);
                }
            },
            None => {
                let _ = write!(w, " {}=--", name);
            }
        }
    }

    /// Emit the `[rtwit]` rollup line and reset every per-span slot. Called from the ~5 s witness
    /// gate in `x86_render_service`.
    pub fn rollup() {
        use core::fmt::Write;
        // A bounded stack buffer — no allocation on the witness path.
        let mut buf = Buf::new();
        let _ = write!(buf, "[rtwit]");

        // in2present max
        match IN2PRESENT.take() {
            Some(cycles) => match cycles_to_us(cycles) {
                Some(us) => {
                    let _ = write!(buf, " in2present_max_us={}", us);
                }
                None => {
                    let _ = write!(buf, " in2present_max_us=tsc_uncal");
                }
            },
            None => {
                let _ = write!(buf, " in2present_max_us=--");
            }
        }
        // in2present p99-ish high-water (also resets the histogram). Named `p99bkt` because it is the
        // UPPER BOUND of a coarse power-of-2 µs bucket, not an exact percentile — `in2present_max_us`
        // above is the exact tail, so no worst case is lost; this locates roughly where the 99th
        // percentile sits.
        match in2p_p99_us() {
            Some(us) => {
                let _ = write!(buf, " in2present_p99bkt_us={}", us);
            }
            None => {
                let _ = write!(buf, " in2present_p99bkt_us=--");
            }
        }

        // per-lock max holds
        fmt_slot(&mut buf, "stage_max_us", &STAGE_HOLD);
        fmt_slot(&mut buf, "table_max_us", &TABLE_HOLD);
        fmt_slot(&mut buf, "evq_max_us", &EVQ_HOLD);
        fmt_slot(&mut buf, "windows_max_us", &WINDOWS_HOLD);

        // interrupt-mask max span
        fmt_slot(&mut buf, "irqmask_max_us", &IRQMASK);

        // the ruler's own cost
        match ruler_ns() {
            Some(ns) => {
                let _ = write!(buf, " ruler_ns={}", ns);
            }
            None => {
                let _ = write!(buf, " ruler_ns=tsc_uncal");
            }
        }

        serial_println!("{}", buf.as_str());
    }
}

// ─────────────────────────────────────────────────────────────────────────────────────────────
// SHIM — feature off, or any non-x86 arch. Every entry point is an empty inline no-op and
// `HoldTimer` is zero-sized, so the knob-off build is byte-inert on the hot path.
// ─────────────────────────────────────────────────────────────────────────────────────────────
#[cfg(not(all(feature = "rtwit", target_arch = "x86_64")))]
mod imp {
    use super::Lock;

    /// Zero-sized no-op hold timer.
    pub struct HoldTimer;

    #[inline(always)]
    pub fn hold(_lock: Lock) -> HoldTimer {
        HoldTimer
    }
    #[inline(always)]
    pub fn note_input_enqueued() {}
    #[inline(always)]
    pub fn note_present_composited() {}
    #[inline(always)]
    pub fn note_mask_span(_cycles: u64) {}
    #[inline(always)]
    pub fn rollup() {}
}

pub use imp::{hold, note_input_enqueued, note_mask_span, note_present_composited, rollup, HoldTimer};
