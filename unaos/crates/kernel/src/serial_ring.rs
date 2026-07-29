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
/// lines and still not reach the end.
pub const SLOTS: usize = 64;

/// Maximum staged line length. Longer lines are truncated at a UTF-8 char boundary, SEALED with a
/// visible marker, and COUNTED — a shortened line must never masquerade as a whole one.
///
/// ### SERWIT-2W — why 1536, with the arithmetic
///
/// The original 240 was a guess, and it was wrong. The aarch64 seat gated SERWIT-1 into their tree and
/// immediately saw `truncated 1`: their F3/K1 witness lines run past 340 characters. **A truncated
/// REQUIRE or verdict line breaks an `awk` tally exactly as badly as a lost one**, so on that tree the
/// law "the wire may not lose lines" was not actually held — the transport was still corrupting
/// evidence under contention, just more quietly than before.
///
/// So the width was measured rather than guessed. Every `serial_print!`/`serial_println!` format string
/// in the kernel (2786 of them, both arches) was reconstructed — `\`-continuations joined, placeholders
/// charged a pessimistic 20 bytes each, the width every 16-bit register field can actually print:
///
/// ```text
///     > 240 chars: 264 format strings   ← 9.5% of the tree truncated at the old width
///     > 340 chars:  97                  ← the aarch64 seat's measured floor is NOT the maximum
///     > 512 chars:  21
///     > 768 chars:   6
///     > 896 chars:   1
///     measured maximum: 1291 chars      (`arch/aarch64/v3d.rs`, the v3d59 mainline-audit note —
///                                        a pure literal, so 1291 is exact, not an estimate)
/// ```
///
/// 1291 + the trailing newline = **1292 bytes of true worst case**. 1536 gives 244 bytes (19%) of
/// headroom over it and 655 bytes over the entire rest of the tree, and is a clean power-of-two
/// multiple of the page allocator's granularity.
///
/// **Why not 1024** (which would have covered 2785 of 2786): because it would leave exactly one line in
/// the tree truncating on every boot that enables it. A truncation counter that is permanently non-zero
/// for a known-benign reason is a counter people learn to ignore, and this arc is entirely about not
/// having instruments people learn to ignore. At 1536 the counter reads 0, so any non-zero reading is
/// news.
///
/// **Why one width and not per-arch** (which was offered): the >340 population is spread across BOTH
/// arches — `v3d.rs` and `rtl8168_tegra.rs` on aarch64, `video/wcf.rs`'s `[wc-f]` scanout rollups and
/// `arch/x86_64/syscall.rs`'s S9 verdicts on x86. Two per-arch widths would be two numbers each sized
/// against one seat's lines and silently wrong for the other's — which is the precise failure mode being
/// closed here. Both seats read one number.
///
/// **Cost, stated plainly.** `SLOTS * SLOT_LEN` = 64 × 1536 = 96 KiB of `.bss` per staging ring, and
/// there are three (the primary wire, the FTDI mirror, the flight recorder) = 288 KiB, uniform on both
/// arches and present on metal. That is the price of a transport that does not corrupt evidence, paid
/// once in BSS on machines with gigabytes.
pub const SLOT_LEN: usize = 1536;

/// Scratch width for the fixed-form loss/truncation markers. Deliberately NOT [`SLOT_LEN`]: the marker
/// buffer is a stack array inside [`drain`], which runs in the UART-locked, IRQ-masked region of every
/// print, and a 1.5 KiB frame there would be a real cost for a line that is never more than ~80 bytes.
const MARKER_LEN: usize = 128;

/// Sealed onto the end of a line that did not fit, so a human reading a capture cannot mistake a cut
/// line for a complete one. The counter says HOW MANY were cut; this says WHICH, and exactly where.
pub const TRUNCATION_MARK: &str = "…⟨SERWIT-2W: line truncated here⟩\n";

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

/// SERWIT-2W — overwrite the tail of a truncated line with [`TRUNCATION_MARK`], in place, at a UTF-8
/// char boundary. `n` is the current length and is updated to the sealed length.
///
/// Backing up rather than appending is deliberate: the buffer is already full, and a marker that only
/// appeared when there happened to be room would be absent exactly on the longest lines — the ones that
/// most need it.
///
/// # Safety
/// `buf` must be valid for `cap` bytes and `*n <= cap`.
fn seal_truncation(buf: *mut u8, cap: usize, n: &mut usize) {
    let mark = TRUNCATION_MARK.as_bytes();
    if cap < mark.len() {
        return;
    }
    let mut at = (*n).min(cap - mark.len());
    // Do not cut a multi-byte char in half while backing up.
    // SAFETY: `at < cap` and the buffer holds `*n >= at` initialised bytes.
    while at > 0 && (unsafe { *buf.add(at) } & 0xC0) == 0x80 {
        at -= 1;
    }
    // SAFETY: `at + mark.len() <= cap` by the `min` above.
    unsafe { core::ptr::copy_nonoverlapping(mark.as_ptr(), buf.add(at), mark.len()) };
    *n = at + mark.len();
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
        // SERWIT-2W: seal the cut so the wire itself says the line is short. A counter alone leaves the
        // reader of a capture unable to tell WHICH line was clipped.
        seal_truncation(w.buf, SLOT_LEN, &mut w.n);
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
    // SERWIT-2W: a fixed-form marker needs marker-sized scratch, not slot-sized. See [`MARKER_LEN`].
    let mut buf = [0u8; MARKER_LEN];
    let mut w = BoundedWriter {
        buf: buf.as_mut_ptr(),
        cap: MARKER_LEN,
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

// ── SERWIT-3: the WIDE-LINE leg ──────────────────────────────────────────────────────────────────
//
// The 240-byte slot was a guess, and it cost the aarch64 seat a `truncated 1` on their very first
// gated run: their F3/K1 witnesses run past 340 characters. A truncated verdict line breaks an `awk`
// tally exactly as badly as a lost one, so a transport that silently clips wide lines has not actually
// stopped lying — it has only changed how. This leg exists so the width can never quietly regress:
// every worker also emits ONE line at the widest realistic size, THROUGH THE CONTENDED PATH, and the
// verdict asserts that not one of them was clipped.
//
// The line is END-SENTINELLED on purpose. The in-kernel assertion (`TRUNCATED` delta == 0) and the
// on-the-wire check are independent: `awk '/SERWIT3-END/' target/serial.log | wc -l` must equal the
// worker count, and a clipped line loses its sentinel by construction (truncation takes the TAIL). A
// counter that only ever agreed with itself would prove nothing — the same reasoning SERWIT-1 used to
// sequence-number its burst.

/// 1248 characters of ballast (78 × 16). Emitted, the line is
/// `"[serwit3] c=N width-probe " + PAD + " SERWIT3-END\n"` = 1248 + 39 = **1287 bytes**, which sits
/// within five bytes of the tree's measured worst case (`arch/aarch64/v3d.rs`'s 1291-char audit note).
/// So this leg exercises the ACTUAL maximum a boot can emit, not a comfortable sample of it — a probe
/// sized well under the real maximum would pass on a width that still clips real lines, which is
/// precisely the failure being closed.
const SERWIT_PAD: &str = concat!(
    "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF", // 1
    "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF", // 2
    "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF", // 3
    "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF", // 4
    "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF", // 5
    "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF", // 6
    "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF", // 7
    "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF", // 8
    "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF", // 9
    "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF", // 10
    "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF", // 11
    "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF", // 12
    "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF", // 13
    "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF", // 14
    "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF", // 15
    "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF", // 16
    "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF", // 17
    "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF", // 18
    "0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF", // 19
    "0123456789ABCDEF0123456789ABCDEF",                                 // 19.5 -> 1248
);

/// `TRUNCATED` at the moment the gate opened, so the verdict asserts on the WINDOW's delta.
static SERWIT_TRUNC_BASE: AtomicU64 = AtomicU64::new(0);

/// One worker's burst. Runs as an ordinary scheduled kernel task on its own core.
pub fn serwit_worker(cpu: usize) {
    while !SERWIT_GO.load(Ordering::Acquire) {
        core::hint::spin_loop();
    }
    for n in 0..SERWIT_BURST {
        serial_println!("[serwit] c={} n={}", cpu, n);
        SERWIT_SENT.fetch_add(1, Ordering::Relaxed);
    }
    // SERWIT-3: the wide line, emitted from inside the same contended window as the burst above.
    serial_println!(
        "[serwit3] c={} width-probe {} SERWIT3-END",
        cpu,
        SERWIT_PAD
    );
    SERWIT_SENT.fetch_add(1, Ordering::Relaxed);
    SERWIT_DONE.fetch_add(1, Ordering::Release);
}

/// Number of workers this fixture wants to run, given the online core count. At least 2 (a single
/// core cannot contend with itself here — `_print` masks interrupts for the whole locked region).
pub fn serwit_worker_count(online: usize) -> usize {
    online.min(6).max(1)
}

/// Open the gate; the workers are already parked on it.
pub fn serwit_release() {
    // SERWIT-3 baselines the truncation counter HERE rather than through `serwit_snapshot`, whose
    // signature belongs to a call site outside this arc's lane.
    SERWIT_TRUNC_BASE.store(TRUNCATED.load(Ordering::Relaxed), Ordering::Relaxed);
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
    // +1: SERWIT-3's wide-line probe, one per worker.
    let want = workers as u64 * (SERWIT_BURST + 1);
    let truncated = TRUNCATED.load(Ordering::Relaxed) - SERWIT_TRUNC_BASE.load(Ordering::Relaxed);
    let submitted = SUBMITTED.load(Ordering::Relaxed) - base_submitted;
    let emitted = EMITTED.load(Ordering::Relaxed) - base_emitted;
    let dropped = DROPPED.load(Ordering::Relaxed) - base_dropped;
    let inflight = in_flight();
    let staged = STAGED.load(Ordering::Relaxed);
    // `submitted` counts this verdict's own prints too, and `emitted` lags by however many lines are
    // still queued behind this one; the law is stated with both slacks made explicit rather than
    // fudged, so `balanced` is a real equality and not a tolerance.
    let balanced = submitted == emitted + dropped + inflight;
    if sent == want && dropped == 0 && truncated == 0 && balanced {
        serial_println!(
            ":: SERWIT-1: contended serial — {} lines sent (incl. {} wide-line probes at ~{}B), {} \
             deferred to the staging ring, 0 dropped, 0 truncated, accounting balanced \
             (submitted={} emitted={} inflight={}) -> PASS ::",
            sent,
            workers,
            SERWIT_PAD.len() + 39,
            staged,
            submitted,
            emitted,
            inflight
        );
    } else {
        serial_println!(
            ":: SERWIT-1: FAIL — sent={} (want {}) dropped={} truncated={} submitted={} emitted={} \
             inflight={} balanced={} ::",
            sent,
            want,
            dropped,
            truncated,
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

// ═════════════════════════════════════════════════════════════════════════════════════════════════
// SERWIT-2 — THE MIRROR TAPS: the same law, applied to the four taps SERWIT-1 left alone.
// ═════════════════════════════════════════════════════════════════════════════════════════════════
//
// `_print` does not write to one sink, it writes to five. The PRIMARY wire (the 16550) is what
// SERWIT-1 fixed; hanging off the same seam are four MIRRORS, each of which had the identical
// pre-SERWIT shape — `try_lock()`, and on failure the whole formatted line evaporates with no
// counter anywhere:
//
//   * `video::fbcon::_print`            — the on-screen console.
//   * `drivers::xhci::ftdi::mirror`     — the FTDI cable, i.e. the BENCH'S OWN CAPTURE PATH.
//   * `selftest::capture`               — the `tste` boot-verdict replay ring.
//   * `flight_recorder::capture`        — `UNAOS.LOG`.
//
// Crucially those four run OUTSIDE `SERIAL1` and OUTSIDE the interrupt mask (`arch::serial::_print`
// calls them after the locked region), so they are contended by every core at once — including on the
// contended lines the primary wire is busy deferring. They are not a rarer version of the same defect;
// under a multi-core burst they are a WORSE one.
//
// ### Not every tap owes the same thing
//
// A mirror must never be able to stall the primary wire — an inversion where the panel makes the UART
// wait would be worse than the drop it replaces — so `try_lock`-and-never-block stays the rule
// everywhere. What changes is the failure branch, and the right answer differs by tap:
//
//   * **Sinks whose content is evidence** (`ftdi`, `flightrec`, `tste`) must not lose lines at all.
//     Their sinks are cheap (a memcpy into a byte ring, a 42-byte record), so the SERWIT-1 discipline
//     transplants directly: defer into a lock-free [`LineRing`] and let the next holder drain it.
//     `tste`'s ring goes one better and drops its Mutex entirely — a fixed array of fixed records
//     needs only an atomic index claim, so there is no lock left to lose.
//   * **Sinks that are a VIEW** (`fbcon`) stay legitimately lossy. Painting a deferred backlog means
//     glyph work inside the masked, locked critical section that PANEL-DEFER exists to keep short, and
//     the line is on the wire regardless — the panel is the one sink whose loss costs no evidence. So
//     the obligation there is the weaker half of the law: every miss is COUNTED, and announced both on
//     the panel (a `[fbcon] N line(s) missed` marker) and on the wire (see [`mirror_service`]).
//
// ### Why none of this can deadlock, stall or contend a breadcrumb
//
// [`LineRing`] is [`RING`]'s machinery made reusable: three atomics plus per-slot atomics, no Mutex, no
// allocation, no reentrancy into `serial_println!`, every wait bounded. WEDGE-2 / WEDGE-4 write single
// bytes through the arch's raw lock-free UART primitive and enter none of this; SERWIT-2 adds no lock
// they could contend for and REMOVES one (`selftest`'s). The panic path is likewise untouched: every
// tap is still `try_lock`-only, and staging is wait-free apart from a CAS retry.

/// One slot of a [`LineRing`]. Same state machine as [`Slot`], generic over the line length so a tap
/// can size its staging area to its own traffic.
struct MirrorSlot<const LEN: usize> {
    state: AtomicU8,
    len: AtomicU16,
    buf: UnsafeCell<[u8; LEN]>,
}

impl<const LEN: usize> MirrorSlot<LEN> {
    const fn new() -> Self {
        MirrorSlot {
            state: AtomicU8::new(EMPTY),
            len: AtomicU16::new(0),
            buf: UnsafeCell::new([0u8; LEN]),
        }
    }
}

/// A reusable, lock-free, alloc-free staging ring of whole formatted lines — [`RING`]'s shape, exposed
/// so a mirror tap can have its own without a second copy of the reasoning.
///
/// **Caller contract for [`drain`](LineRing::drain):** the caller must hold the tap's sink exclusively,
/// which is what makes the drainer unique. Every tap here drains only while holding the very lock whose
/// contention caused the staging in the first place.
pub struct LineRing<const SLOTS: usize, const LEN: usize> {
    slots: [MirrorSlot<LEN>; SLOTS],
    head: AtomicU64,
    tail: AtomicU64,
}

// SAFETY: identical argument to `Ring`'s — `buf` is touched only by the writer that won the `head`
// claim (slot in `WRITING`) or by the unique drainer once the slot reads `READY` under `Acquire`.
unsafe impl<const SLOTS: usize, const LEN: usize> Sync for LineRing<SLOTS, LEN> {}

impl<const SLOTS: usize, const LEN: usize> LineRing<SLOTS, LEN> {
    pub const fn new() -> Self {
        LineRing {
            slots: [const { MirrorSlot::<LEN>::new() }; SLOTS],
            head: AtomicU64::new(0),
            tail: AtomicU64::new(0),
        }
    }

    /// Lines staged but not yet drained.
    #[inline]
    pub fn in_flight(&self) -> u64 {
        self.head
            .load(Ordering::Acquire)
            .wrapping_sub(self.tail.load(Ordering::Acquire))
    }

    /// Defer one line, reporting the outcome so the caller can charge its own ledger.
    pub fn stage(&self, args: fmt::Arguments) -> Staged {
        let seq = loop {
            let head = self.head.load(Ordering::Acquire);
            let tail = self.tail.load(Ordering::Acquire);
            if head.wrapping_sub(tail) >= SLOTS as u64 {
                return Staged::Full;
            }
            if self
                .head
                .compare_exchange_weak(head, head + 1, Ordering::AcqRel, Ordering::Relaxed)
                .is_ok()
            {
                break head;
            }
            core::hint::spin_loop();
        };
        let slot = &self.slots[(seq % SLOTS as u64) as usize];
        slot.state.store(WRITING, Ordering::Relaxed);
        let mut w = BoundedWriter {
            buf: slot.buf.get() as *mut u8,
            cap: LEN,
            n: 0,
            truncated: false,
        };
        let _ = w.write_fmt(args);
        if w.truncated {
            seal_truncation(w.buf, LEN, &mut w.n);
        }
        slot.len.store(w.n as u16, Ordering::Relaxed);
        slot.state.store(READY, Ordering::Release);
        if w.truncated { Staged::Truncated } else { Staged::Whole }
    }

    /// Emit every staged line, in order, through `emit`. Returns how many lines were emitted. Stops at
    /// the first claimed-but-unpublished slot rather than skipping it, so deferral never reorders the
    /// sink; bounded by `SLOTS` iterations, so a drain can never lengthen a print unboundedly.
    pub fn drain<F: FnMut(&str)>(&self, mut emit: F) -> u64 {
        let mut out = 0u64;
        let mut guard = 0usize;
        loop {
            if guard > SLOTS {
                break;
            }
            guard += 1;
            let tail = self.tail.load(Ordering::Relaxed);
            if tail == self.head.load(Ordering::Acquire) {
                break;
            }
            let slot = &self.slots[(tail % SLOTS as u64) as usize];
            if slot.state.load(Ordering::Acquire) != READY {
                break;
            }
            let n = slot.len.load(Ordering::Relaxed) as usize;
            // SAFETY: the slot is READY, so its writer is finished and released; the caller's sink lock
            // makes this the only reader.
            let full: &[u8; LEN] = unsafe { &*slot.buf.get() };
            let bytes = &full[..n.min(LEN)];
            if let Ok(s) = core::str::from_utf8(bytes) {
                emit(s);
            }
            out += 1;
            slot.state.store(EMPTY, Ordering::Release);
            self.tail.store(tail.wrapping_add(1), Ordering::Release);
        }
        out
    }
}

/// What [`LineRing::stage`] did with a line.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Staged {
    /// Deferred intact.
    Whole,
    /// Deferred, but it did not fit `LEN` — sealed with [`TRUNCATION_MARK`]. The tap counts it as a
    /// tear, which is announced like a drop: a short line is a corrupted line, not a delivered one.
    Truncated,
    /// The ring was full; the line is genuinely lost and the caller must count it.
    Full,
}

/// [`SlotWriter`]'s length-generic twin: format into a caller-owned buffer, truncating at a UTF-8 char
/// boundary rather than splitting a multi-byte sequence.
pub struct BoundedWriter {
    pub buf: *mut u8,
    pub cap: usize,
    pub n: usize,
    pub truncated: bool,
}

impl Write for BoundedWriter {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let bytes = s.as_bytes();
        let room = self.cap - self.n;
        let take = if bytes.len() <= room {
            bytes.len()
        } else {
            self.truncated = true;
            let mut k = room;
            while k > 0 && (bytes[k] & 0xC0) == 0x80 {
                k -= 1;
            }
            k
        };
        if take > 0 {
            // SAFETY: the caller owns `buf` for `cap` bytes and `self.n + take <= cap` by construction.
            unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), self.buf.add(self.n), take) };
            self.n += take;
        }
        Ok(())
    }
}

/// One mirror tap's ledger. The conservation law each tap must satisfy is
///
/// ```text
///     submitted == absorbed + dropped + suppressed + in_flight
/// ```
///
/// where `suppressed` is a line the tap DECLINED by policy (the GUI owns the panel, quiet-panel is in
/// force, the console is not up yet). Separating "declined on purpose" from "lost" is the whole point:
/// lumping them would make a silently-lossy tap indistinguishable from a correctly-quiet one, which is
/// the exact ambiguity SERWIT-1 removed from the primary wire.
pub struct TapCounters {
    /// Lines handed to this tap.
    pub submitted: AtomicU64,
    /// Lines that reached the tap's sink (directly or drained from its staging ring).
    pub absorbed: AtomicU64,
    /// Lines that took the deferred path (diagnostic — a deferred line is not a lost one).
    pub staged: AtomicU64,
    /// Lines genuinely lost. Must read 0 on an evidence tap.
    pub dropped: AtomicU64,
    /// Lines the tap declined by policy. Not loss.
    pub suppressed: AtomicU64,
    /// Lines that reached the sink only in part (a mid-line contention tear on the panel).
    pub torn: AtomicU64,
    /// Loss not yet announced on the wire; swapped to 0 by the announcement.
    pending: AtomicU32,
}

impl TapCounters {
    pub const fn new() -> Self {
        TapCounters {
            submitted: AtomicU64::new(0),
            absorbed: AtomicU64::new(0),
            staged: AtomicU64::new(0),
            dropped: AtomicU64::new(0),
            suppressed: AtomicU64::new(0),
            torn: AtomicU64::new(0),
            pending: AtomicU32::new(0),
        }
    }
    #[inline]
    pub fn submit(&self) {
        self.submitted.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn absorb(&self) {
        self.absorbed.fetch_add(1, Ordering::Relaxed);
    }
    /// Account for a whole drained batch at once.
    #[inline]
    pub fn absorb_n(&self, n: u64) {
        if n > 0 {
            self.absorbed.fetch_add(n, Ordering::Relaxed);
        }
    }
    #[inline]
    pub fn note_staged(&self) {
        self.staged.fetch_add(1, Ordering::Relaxed);
    }
    #[inline]
    pub fn suppress(&self) {
        self.suppressed.fetch_add(1, Ordering::Relaxed);
    }
    /// One line lost. Counted twice on purpose: cumulatively, and as un-announced.
    #[inline]
    pub fn drop_line(&self) {
        self.dropped.fetch_add(1, Ordering::Relaxed);
        self.pending.fetch_add(1, Ordering::Relaxed);
    }
    /// One line that reached the sink truncated. Announced like a drop, counted apart from one.
    #[inline]
    pub fn tear(&self) {
        self.torn.fetch_add(1, Ordering::Relaxed);
        self.pending.fetch_add(1, Ordering::Relaxed);
    }
    /// Losses not yet announced, taken.
    #[inline]
    pub fn take_pending(&self) -> u32 {
        self.pending.swap(0, Ordering::Relaxed)
    }
    fn tally(&self) -> (u64, u64, u64, u64, u64, u64) {
        (
            self.submitted.load(Ordering::Relaxed),
            self.absorbed.load(Ordering::Relaxed),
            self.staged.load(Ordering::Relaxed),
            self.dropped.load(Ordering::Relaxed),
            self.suppressed.load(Ordering::Relaxed),
            self.torn.load(Ordering::Relaxed),
        )
    }
}

/// The on-screen console mirror (legitimately lossy — counted, never staged).
pub static TAP_FBCON: TapCounters = TapCounters::new();
/// The FTDI console capture — the bench's own capture path. Staged; must not lose lines.
pub static TAP_FTDI: TapCounters = TapCounters::new();
/// The `tste` boot-verdict replay ring. Lock-free; must not lose records.
pub static TAP_TSTE: TapCounters = TapCounters::new();
/// The `UNAOS.LOG` flight recorder. Staged; must not lose lines.
pub static TAP_FLIGHTREC: TapCounters = TapCounters::new();

/// Lines each tap has in flight, supplied by the tap (only it knows the shape of its staging).
fn tap_inflight(name: &str) -> u64 {
    match name {
        "ftdi" => crate::drivers::xhci::ftdi::staged_in_flight(),
        #[cfg(target_arch = "x86_64")]
        "flightrec" => crate::flight_recorder::staged_in_flight(),
        _ => 0,
    }
}

fn taps() -> [(&'static str, &'static TapCounters); 4] {
    [
        ("fbcon", &TAP_FBCON),
        ("ftdi", &TAP_FTDI),
        ("tste", &TAP_TSTE),
        ("flightrec", &TAP_FLIGHTREC),
    ]
}

/// SERWIT-2 — announce any un-announced mirror loss on the WIRE, and emit the verdict once.
///
/// **Call from an IF=1, unlocked, non-print context only** (it prints, so calling it from inside a tap
/// would recurse through `_print`). The mirrors' own channels announce independently — the FTDI ring
/// injects its marker into the cable stream, the panel paints one, `UNAOS.LOG` carries its byte-drop
/// note, `tste` prints its ring-full count — because a tap's own reader is the one who needs to know
/// its tap lied. This is the wire's copy of the same news, for the reader who only has the log.
///
/// Self-rate-limiting: `take_pending` clears the counter, so a tap that is not losing lines prints
/// nothing at all and a tap that is prints once per burst, never once per line.
pub fn mirror_service() {
    for (name, t) in taps() {
        if t.take_pending() > 0 {
            serial_println!(
                "[mirror] {}: {} line(s) dropped, {} truncated since boot (sink contended or full)",
                name,
                t.dropped.load(Ordering::Relaxed),
                t.torn.load(Ordering::Relaxed)
            );
        }
    }
    mirror_verdict_once();
}

/// One-shot: has the SERWIT-2 verdict been emitted yet?
static MIRROR_VERDICT_DONE: AtomicBool = AtomicBool::new(false);

/// THE SAMPLING WINDOW, stated rather than fudged.
///
/// The tap ledgers are six independent atomics; there is no lock-free way to read them as one instant,
/// and taking a seqlock over them would put a lock on the print path — the one thing this whole arc
/// forbids. So a snapshot taken while another core is between its `submit()` and its outcome shows that
/// line as neither absorbed nor dropped nor suppressed nor in flight. **At most one such line can exist
/// per core**, because a core is inside exactly one `_print` at a time and the taps do not recurse.
///
/// 64 is therefore not a tolerance, it is the ceiling on the number of cores this kernel brings up. The
/// distinction that matters: a sampling artefact is bounded by the core count and disappears on the next
/// sample; a genuine accounting hole — the defect this fixture exists to catch — grows without bound
/// with traffic and would blow straight past this ceiling on any real boot. A single silently-dropping
/// tap on the pre-fix tree lost 87.5% of a 72-line burst.
const MIRROR_WINDOW: u64 = 64;

/// The SERWIT-2 verdict, emitted once from the first [`mirror_service`] poll — i.e. on entry to the
/// main loop, after the boot fixtures (SERWIT-1's own multi-core burst included) have run through all
/// four taps under real contention.
///
/// PASS demands, for every tap, that the conservation law balances — no line is unaccounted for — AND
/// that the three EVIDENCE taps lost nothing. `fbcon`'s misses are reported but are not fatal: the
/// panel is a view, the line is on the wire regardless, and the property being proven for it is that a
/// miss is visible rather than that it never happens.
fn mirror_verdict_once() {
    if MIRROR_VERDICT_DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    // Snapshot BEFORE printing: the verdict's own lines go through these very taps.
    let mut balanced = true;
    let mut evidence_lost = 0u64;
    let mut snap: [(&'static str, (u64, u64, u64, u64, u64, u64), u64, u64); 4] =
        [("", (0, 0, 0, 0, 0, 0), 0, 0); 4];
    for (i, (name, t)) in taps().into_iter().enumerate() {
        let tally = t.tally();
        let inflight = tap_inflight(name);
        let (submitted, absorbed, _staged, dropped, suppressed, _torn) = tally;
        let accounted = absorbed + dropped + suppressed + inflight;
        // `accounted > submitted` means a line was counted twice — a real accounting bug, never a
        // sampling artefact, because `submit` runs strictly before any outcome for the same line.
        let gap = submitted.wrapping_sub(accounted);
        if accounted > submitted || gap > MIRROR_WINDOW {
            balanced = false;
        }
        snap[i] = (name, tally, inflight, gap);
        if name != "fbcon" {
            evidence_lost += dropped;
        }
    }
    for (name, (submitted, absorbed, staged, dropped, suppressed, torn), inflight, gap) in snap {
        serial_println!(
            ":: SERWIT-2 tap {}: submitted={} absorbed={} staged={} dropped={} suppressed={} torn={} inflight={} in_progress={} ::",
            name, submitted, absorbed, staged, dropped, suppressed, torn, inflight, gap
        );
    }
    if balanced && evidence_lost == 0 {
        serial_println!(
            ":: SERWIT-2: mirror taps — every line accounted for on all 4 taps, 0 lost on the 3 \
             evidence taps (ftdi/tste/flightrec) -> PASS ::"
        );
    } else {
        serial_println!(
            ":: SERWIT-2: FAIL — balanced={} evidence_lost={} ::",
            balanced,
            evidence_lost
        );
    }
}
