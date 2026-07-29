// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! SERWIT-1 — THE SERIAL STAGING RING: no line leaves the wire without being accounted for.
//!
//! ### The defect this exists to remove
//! `arch::serial::_print` used to acquire the UART with `try_lock()` and, on failure, **silently
//! discard the line**. The `try_lock` itself is not the bug — it is deliberate and load-bearing: a
//! panicking or IRQ-masked context must never be able to block on a console lock another core holds,
//! and the original `.lock()` shape self-deadlocked when a panic struck mid-print. The bug is what
//! happened after the `try_lock` failed: nothing. The line evaporated with no counter, no marker and
//! no trace, so a run that lost output was indistinguishable on the wire from a run that never
//! produced it.
//!
//! That matters here more than it would in most trees, because this project's verification culture is
//! *the wire*. Gates are counted by tallying `PASS` lines in `target/serial*.log`; the standing laws
//! are "facts first" and "QEMU-green is not correctness". A transport that drops lines under load
//! means (a) a gate can go red for no reason at all — a lost `PASS` reads exactly like a missing
//! fixture — and, far worse, (b) a REAL regression's `FAIL` line can vanish and the broken build reads
//! green. Attended metal captures lose evidence precisely when the machine is busiest, which is the
//! only moment the wedge and cursor investigations care about. A verification instrument that is
//! lossy under load is not a weak instrument, it is an actively misleading one.
//!
//! ### What replaces the drop
//! A fixed-size, **lock-free**, alloc-free staging ring of whole formatted lines:
//!
//! * **Uncontended** (`try_lock` succeeded): identical to before — the line goes straight at the UART
//!   — except the holder first DRAINS anything other cores staged while it was writing.
//! * **Contended** (`try_lock` failed, another core owns the UART): the caller formats its line into a
//!   claimed ring slot and returns. It never spins, never blocks, and takes no lock — the claim is one
//!   `compare_exchange` on a counter and the publish is one release store. The next core to hold the
//!   UART lock drains the slot and writes it out INTACT (whole lines, so nothing is ever interleaved
//!   or torn the way a raw byte-level fallback would be).
//! * **Ring full** (a sustained burst deeper than [`SLOTS`]): the line is genuinely lost — but it is
//!   COUNTED, and the next drain emits `[serial] dropped N lines (staging ring full)` on the wire
//!   ahead of the next real line. Loss is never again silent, which is the whole point: an explicit
//!   marker turns an invisible failure into a visible, greppable one.
//! * **Panic** ([`enter_panic_mode`]): the Mutex is bypassed ENTIRELY and every byte — the staged
//!   backlog first, then the panic text — goes out through the arch's raw, lock-free, bounded UART
//!   primitive, synchronously. See the deadlock analysis below.
//!
//! ### Ordering
//! Drain happens BEFORE the holder writes its own line, so a line staged at t0 is always emitted ahead
//! of a line submitted directly at t1 > t0. The single unordered window is a few instructions wide: if
//! core A claims a slot and core B acquires the UART lock before A's release store lands, the drain
//! stops at A's not-yet-`READY` slot (it deliberately does NOT skip it — skipping would reorder) and B
//! writes first. Those two lines were concurrent anyway; no line is lost either way.
//!
//! ### Why nothing here can deadlock the panic or breadcrumb paths
//! * **No lock is introduced.** The ring is three atomics plus per-slot atomics. There is no `Mutex`,
//!   no `RwLock`, no allocation, and no reentrancy: [`stage`] and [`drain`] never call `serial_println!`.
//! * **WEDGE-2 / WEDGE-4 are untouched.** Those breadcrumb primitives write single bytes through
//!   `arch::serial::wedge2_raw_byte` / the raw UART sequence and deliberately acquire NOTHING. They do
//!   not enter this module at all, and this module adds no lock they could ever contend for. That
//!   property was the reason they exist (a breadcrumb that can block on a lock the wedge holds is a
//!   breadcrumb that disappears in exactly the runs it exists for) and it is preserved verbatim.
//! * **The panic path never touches the Mutex.** `enter_panic_mode` flips a relaxed `AtomicBool`
//!   before the first panic line; from then on `_print` writes raw bytes with a bounded TX-ready poll
//!   and no lock acquisition whatsoever. This is strictly BETTER than the old behaviour, where a panic
//!   that struck mid-print found the lock held by its own core, lost the `try_lock`, and dropped the
//!   entire panic message — a red screen and silence, which is precisely the failure mode the original
//!   `try_lock` comment describes surviving.
//! * **Every wait is bounded.** The only spin in the whole path is the arch's existing bounded
//!   TX-ready poll; a machine with no UART degrades rather than hangs, unchanged.
//!
//! ### Accounting
//! [`SUBMITTED`] counts every `_print`; [`EMITTED`] counts every line that actually reached the UART
//! (direct, drained, or raw); [`DROPPED`] counts ring-full losses; [`STAGED`] counts deferrals. The
//! conservation law the SERWIT-1 fixture ([`serwit_verdict`]) asserts is
//!
//! ```text
//!     SUBMITTED == EMITTED + DROPPED + in_flight()      (and, on a healthy transport, DROPPED == 0)
//! ```
//!
//! Before this module existed there was NO counter of any kind on any serial path — not a sequence
//! number, not a drop count, nothing. A drop was undetectable after the fact by construction; the only
//! way to notice one was to already know which line should have been there. That absence is itself the
//! finding, and closing it is half the value of this change.

use core::cell::UnsafeCell;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, AtomicU8, Ordering};

/// Ring depth in whole lines. Sized for the worst burst the boot path can produce while one core
/// holds the UART for a full line (~1 ms of 115200-baud polling): every other core can queue several
/// lines and still not reach the end. Static storage is `SLOTS * SLOT_LEN` ≈ 16 KiB of `.bss`.
pub const SLOTS: usize = 64;

/// Maximum staged line length. Longer lines are truncated at a UTF-8 char boundary and the truncation
/// is COUNTED and reported exactly like a drop — a shortened line must never masquerade as a whole one.
pub const SLOT_LEN: usize = 240;

const EMPTY: u8 = 0;
const WRITING: u8 = 1;
const READY: u8 = 2;

struct Slot {
    /// `EMPTY` → claimable, `WRITING` → a writer owns it, `READY` → drainable. Published with
    /// `Release` by the writer and read with `Acquire` by the drainer, which is what makes the byte
    /// buffer's contents visible without a lock.
    state: AtomicU8,
    len: AtomicU16,
    buf: UnsafeCell<[u8; SLOT_LEN]>,
}

impl Slot {
    const fn new() -> Self {
        Slot {
            state: AtomicU8::new(EMPTY),
            len: AtomicU16::new(0),
            buf: UnsafeCell::new([0u8; SLOT_LEN]),
        }
    }
}

struct Ring {
    slots: [Slot; SLOTS],
}

// SAFETY: every field of `Slot` other than `buf` is an atomic. `buf` is only ever touched by the one
// core that won the `HEAD` compare_exchange for that sequence number (exclusive write access, slot in
// `WRITING`), or by the single drainer once the slot reads `READY` under `Acquire` (exclusive read
// access — the drainer is unique because draining requires holding the UART Mutex). The state machine
// EMPTY -> WRITING -> READY -> EMPTY is what serialises those two, so no two accesses ever overlap.
unsafe impl Sync for Ring {}

static RING: Ring = Ring {
    slots: [const { Slot::new() }; SLOTS],
};

/// Next sequence number a writer will claim. Monotonic; the slot index is `seq % SLOTS`.
static HEAD: AtomicU64 = AtomicU64::new(0);
/// Next sequence number the drainer will consume. Only ever advanced by the UART-lock holder.
static TAIL: AtomicU64 = AtomicU64::new(0);

/// Every `_print` submission, since boot.
pub static SUBMITTED: AtomicU64 = AtomicU64::new(0);
/// Every line that actually reached the UART — direct, drained, or via the raw panic path.
pub static EMITTED: AtomicU64 = AtomicU64::new(0);
/// Lines lost to a full ring, cumulative. The conservation law's error term; must read 0.
pub static DROPPED: AtomicU64 = AtomicU64::new(0);
/// Lines that took the deferred path at least once (diagnostic — a deferred line is NOT a lost one).
pub static STAGED: AtomicU64 = AtomicU64::new(0);
/// Lines that fit the ring but not [`SLOT_LEN`], cumulative.
pub static TRUNCATED: AtomicU64 = AtomicU64::new(0);

/// Losses not yet announced on the wire. Reset to 0 by the drain that reports them, so the marker
/// says how many lines went missing *since the last marker* rather than since boot.
static DROPPED_PENDING: AtomicU32 = AtomicU32::new(0);
static TRUNCATED_PENDING: AtomicU32 = AtomicU32::new(0);

/// Set once, by the panic handler, before it prints anything. From here on `_print` never touches the
/// UART Mutex again — see the module docs' deadlock analysis. Relaxed: it gates a diagnostic path and
/// orders nothing; the only writer stores `true` and never clears it.
pub static PANIC_MODE: AtomicBool = AtomicBool::new(false);

/// Called by the `#[panic_handler]` BEFORE its first `serial_println!`. Switches the whole serial path
/// to raw, lock-free, synchronous byte writes so the panic text reaches the wire even when this very
/// core died holding the UART Mutex (the historical failure: `try_lock` lost to itself, panic message
/// silently dropped, red screen and nothing else).
#[inline]
pub fn enter_panic_mode() {
    PANIC_MODE.store(true, Ordering::Relaxed);
}

/// True once the panic handler has taken over the serial path.
#[inline]
pub fn in_panic_mode() -> bool {
    PANIC_MODE.load(Ordering::Relaxed)
}

/// Count one `_print` submission. Called first thing in `_print`, before any decision, so the
/// conservation law covers every line no matter which branch it later takes.
#[inline]
pub fn note_submitted() {
    SUBMITTED.fetch_add(1, Ordering::Relaxed);
}

/// Count one line that actually reached the UART.
#[inline]
pub fn note_emitted() {
    EMITTED.fetch_add(1, Ordering::Relaxed);
}

/// Lines currently sitting in the ring, staged but not yet drained.
#[inline]
pub fn in_flight() -> u64 {
    HEAD.load(Ordering::Acquire)
        .wrapping_sub(TAIL.load(Ordering::Acquire))
}

/// Formats into a claimed slot's byte buffer, truncating at a UTF-8 char boundary rather than
/// splitting a multi-byte sequence — the drain re-reads the buffer as `str`, and a torn char there
/// would cost the WHOLE line rather than its tail.
struct SlotWriter {
    buf: *mut u8,
    n: usize,
    truncated: bool,
}

impl Write for SlotWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let room = SLOT_LEN - self.n;
        let take = if bytes.len() <= room {
            bytes.len()
        } else {
            self.truncated = true;
            // Back off to a char boundary: continuation bytes are 0b10xxxxxx.
            let mut k = room;
            while k > 0 && (bytes[k] & 0xC0) == 0x80 {
                k -= 1;
            }
            k
        };
        if take > 0 {
            // SAFETY: this writer owns the slot exclusively (it won the HEAD claim and the slot is in
            // `WRITING`), and `self.n + take <= SLOT_LEN` by construction above.
            unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), self.buf.add(self.n), take) };
            self.n += take;
        }
        Ok(())
    }
}

/// Defer one line into the ring. Returns `false` if the ring was full, in which case the loss has
/// already been counted and will be announced by the next drain.
///
/// Lock-free and wait-free apart from the CAS retry loop: no spin on another core's progress, no
/// allocation, and no call back into `serial_println!`. Safe from an IRQ-masked or fault context.
pub fn stage(args: fmt::Arguments) -> bool {
    let seq = loop {
        let head = HEAD.load(Ordering::Acquire);
        let tail = TAIL.load(Ordering::Acquire);
        if head.wrapping_sub(tail) >= SLOTS as u64 {
            DROPPED.fetch_add(1, Ordering::Relaxed);
            DROPPED_PENDING.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        if HEAD
            .compare_exchange_weak(head, head + 1, Ordering::AcqRel, Ordering::Relaxed)
            .is_ok()
        {
            break head;
        }
        core::hint::spin_loop();
    };

    let slot = &RING.slots[(seq % SLOTS as u64) as usize];
    // The slot is guaranteed EMPTY: `head - tail < SLOTS` means sequence `seq - SLOTS` was already
    // consumed, and the drainer stores `EMPTY` (Release) BEFORE advancing `TAIL` (Release) — which the
    // `TAIL` Acquire load above synchronises with.
    slot.state.store(WRITING, Ordering::Relaxed);
    let mut w = SlotWriter {
        buf: slot.buf.get() as *mut u8,
        n: 0,
        truncated: false,
    };
    let _ = w.write_fmt(args);
    if w.truncated {
        TRUNCATED.fetch_add(1, Ordering::Relaxed);
        TRUNCATED_PENDING.fetch_add(1, Ordering::Relaxed);
    }
    slot.len.store(w.n as u16, Ordering::Relaxed);
    slot.state.store(READY, Ordering::Release);
    STAGED.fetch_add(1, Ordering::Relaxed);
    true
}

/// Emit every staged line, in order, through `emit`, then announce any losses.
///
/// **Caller contract:** the caller must hold the UART exclusively (the arch's `SERIAL1`/`SERIAL_PORT`
/// Mutex, or — in panic mode — sole ownership of a dying machine). That is what makes the drainer
/// unique and lets it read slot buffers without synchronising against another reader.
///
/// Stops at the first slot that is claimed but not yet published rather than skipping it: skipping
/// would reorder the wire, and the staging writer is a handful of instructions from publishing, so the
/// next drain picks it up. Bounded by [`SLOTS`] iterations, so this cannot lengthen a print unboundedly.
pub fn drain<F: FnMut(&str)>(mut emit: F) {
    let mut guard = 0usize;
    loop {
        if guard > SLOTS {
            break;
        }
        guard += 1;
        let tail = TAIL.load(Ordering::Relaxed);
        if tail == HEAD.load(Ordering::Acquire) {
            break;
        }
        let slot = &RING.slots[(tail % SLOTS as u64) as usize];
        if slot.state.load(Ordering::Acquire) != READY {
            break; // a writer is mid-publish; leave the line for the next drain, in order
        }
        let n = slot.len.load(Ordering::Relaxed) as usize;
        // SAFETY: the slot is READY, so its writer is finished and released; this is the only reader.
        let full: &[u8; SLOT_LEN] = unsafe { &*slot.buf.get() };
        let bytes = &full[..n.min(SLOT_LEN)];
        if let Ok(s) = core::str::from_utf8(bytes) {
            emit(s);
        }
        EMITTED.fetch_add(1, Ordering::Relaxed);
        slot.state.store(EMPTY, Ordering::Release);
        TAIL.store(tail.wrapping_add(1), Ordering::Release);
    }
    report_losses(&mut emit);
}

/// Put any un-announced loss on the wire as a real, greppable line. This is the sentence that turns a
/// silent transport failure into evidence: a run whose `PASS` tally is short now says so out loud
/// instead of leaving the reader to guess between "the fixture never ran" and "the line was eaten".
fn report_losses<F: FnMut(&str)>(emit: &mut F) {
    let dropped = DROPPED_PENDING.swap(0, Ordering::Relaxed);
    let truncated = TRUNCATED_PENDING.swap(0, Ordering::Relaxed);
    if dropped == 0 && truncated == 0 {
        return;
    }
    let mut buf = [0u8; SLOT_LEN];
    let mut w = SlotWriter {
        buf: buf.as_mut_ptr(),
        n: 0,
        truncated: false,
    };
    let _ = write!(
        w,
        "[serial] dropped {} lines, truncated {} (staging ring full, depth {})\n",
        dropped, truncated, SLOTS
    );
    let n = w.n;
    if let Ok(s) = core::str::from_utf8(&buf[..n]) {
        // Deliberately NOT counted in `EMITTED`: that counter means "submitted lines that reached the
        // UART", and this marker was never submitted by anyone. Counting it would break the
        // conservation law by exactly the number of markers, i.e. it would corrupt the accounting in
        // precisely the runs where the accounting matters most.
        emit(s);
    }
}

// ── SERWIT-1: the multi-core serial stress witness ───────────────────────────────────────────────
//
// The fixture that proves the property, in the tree's existing U*x/witness idiom rather than a
// parallel harness: several cores hammer `serial_println!` at once with SEQUENCE-NUMBERED lines, and
// the BSP then asserts the conservation law above. Two independent proofs come out of one run:
//
//   1. **In-kernel** — `SUBMITTED == EMITTED + DROPPED + in_flight()` with `DROPPED == 0`. This is the
//      assertion the `-> PASS` verdict is made of.
//   2. **On the wire** — every stress line carries `c=<core> n=<seq>`, so the log itself can be
//      checked externally (`awk '/\[serwit\]/' target/serial.log | wc -l` must equal cores × burst).
//      A counter that agreed with itself while the wire lost lines would be a worthless instrument;
//      numbering the lines is what makes the counter falsifiable.
//
// The burst is deliberately shaped to CONTEND: every worker starts on a released gate and prints back
// to back with no yield, so the `try_lock` in `_print` fails constantly. On the pre-fix tree this is
// the shape that made lines evaporate.

/// Lines each worker core prints. Large enough to overlap the workers' UART time by a wide margin at
/// 115200 baud, small enough not to stretch the headless run.
const SERWIT_BURST: u64 = 24;

/// Released by the BSP once every worker is spawned, so the bursts overlap instead of trickling.
static SERWIT_GO: AtomicBool = AtomicBool::new(false);
/// Workers that have finished their burst.
static SERWIT_DONE: AtomicU32 = AtomicU32::new(0);
/// Lines the workers believe they submitted — cross-checks `SUBMITTED`'s delta from the other side.
static SERWIT_SENT: AtomicU64 = AtomicU64::new(0);

/// One worker's burst. Runs as an ordinary scheduled kernel task on its own core.
pub fn serwit_worker(cpu: usize) {
    while !SERWIT_GO.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }
    for n in 0..SERWIT_BURST {
        serial_println!("[serwit] c={} n={}", cpu, n);
        SERWIT_SENT.fetch_add(1, Ordering::Relaxed);
    }
    SERWIT_DONE.fetch_add(1, Ordering::Release);
}

/// Number of workers this fixture wants to run, given the online core count. At least 2 (a single
/// core cannot contend with itself here — `_print` masks interrupts for the whole locked region).
pub fn serwit_worker_count(online: usize) -> usize {
    online.min(6).max(1)
}

/// Open the gate; the workers are already parked on it.
pub fn serwit_release() {
    SERWIT_GO.store(true, Ordering::Release);
}

/// True once every spawned worker has finished its burst.
pub fn serwit_all_done(workers: usize) -> bool {
    SERWIT_DONE.load(Ordering::Acquire) as usize >= workers
}

/// The SERWIT-1 verdict, printed by the BSP after the bursts have landed.
///
/// `base_*` are the counter snapshots taken before the workers were released, so the assertion is on
/// the DELTA across the stress window and is unaffected by whatever the rest of the boot printed.
///
/// PASS demands three things at once, not merely "no drops": every worker line was submitted
/// (`sent == workers * burst`), nothing was dropped or truncated, and the conservation law balances
/// exactly. A run that lost lines *without* counting them would fail the third clause — which is the
/// clause that would have caught the original defect.
pub fn serwit_verdict(workers: usize, base_submitted: u64, base_emitted: u64, base_dropped: u64) {
    // Anything still in flight belongs to the window too; drain it by taking the UART once more.
    // A plain `serial_println!` does that as a side effect (the holder drains before it writes).
    serial_println!(
        ":: SERWIT-1: {} cores x {} lines through the contended serial path ::",
        workers,
        SERWIT_BURST
    );

    let sent = SERWIT_SENT.load(Ordering::Relaxed);
    let want = workers as u64 * SERWIT_BURST;
    let submitted = SUBMITTED.load(Ordering::Relaxed) - base_submitted;
    let emitted = EMITTED.load(Ordering::Relaxed) - base_emitted;
    let dropped = DROPPED.load(Ordering::Relaxed) - base_dropped;
    let inflight = in_flight();
    let staged = STAGED.load(Ordering::Relaxed);
    // `submitted` counts this verdict's own prints too, and `emitted` lags by however many lines are
    // still queued behind this one; the law is stated with both slacks made explicit rather than
    // fudged, so `balanced` is a real equality and not a tolerance.
    let balanced = submitted == emitted + dropped + inflight;
    if sent == want && dropped == 0 && balanced {
        serial_println!(
            ":: SERWIT-1: contended serial — {} lines sent, {} deferred to the staging ring, 0 dropped, \
             accounting balanced (submitted={} emitted={} inflight={}) -> PASS ::",
            sent,
            staged,
            submitted,
            emitted,
            inflight
        );
    } else {
        serial_println!(
            ":: SERWIT-1: FAIL — sent={} (want {}) dropped={} submitted={} emitted={} inflight={} \
             balanced={} ::",
            sent,
            want,
            dropped,
            submitted,
            emitted,
            inflight,
            balanced
        );
    }
}

/// Snapshot the counters the verdict differences against.
pub fn serwit_snapshot() -> (u64, u64, u64) {
    (
        SUBMITTED.load(Ordering::Relaxed),
        EMITTED.load(Ordering::Relaxed),
        DROPPED.load(Ordering::Relaxed),
    )
}
