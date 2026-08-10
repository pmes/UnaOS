// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// TERM_RING — the bounded in-kernel terminal output channel (MIDDEN_CONVERGENCE §3, M2).
//
// Until now the framebuffer console WAS the output buffer: `Console::println` pushed an
// `alloc::String` straight into the view's `history` Vec, so the only producer that could ever emit
// a console line was whoever already held `&mut Console` — the render task. There was no stream, no
// second producer, and nothing between "a line exists" and "a window is painting it".
//
// This module is the seam between those two facts. It is a TRANSPORT: producers stage a formatted
// record here, and the task that owns the view drains it into the view's own store.
//
// ── Three rulings this module implements, and why ───────────────────────────────────────────────
//
//  1. **TRANSPORT, NOT SCROLLBACK.** The ring carries lines toward the renderer; the view's
//     `Console::history` remains the display store. §3 mandates drop-NEWEST for a producer that may
//     never block, and drop-newest is exactly right for a transport (the backlog is a symptom; the
//     newest line is the one the producer could not afford to wait for). It is exactly WRONG for a
//     scrollback: a scrollback that drops newest stops showing the present. Keeping the two roles in
//     two objects lets each take the policy that suits it — drop-newest here, drop-oldest there.
//
//  2. **`LineRing`, NOT `arch::sched::Channel` — a deliberate divergence from §3's sketch.** §3
//     names `Channel<TerminalMsg>` because `GUI_CHANNEL_X86` is the kernel's existing 64-slot
//     bounded channel. It cannot be used here, for reasons §3 itself supplies without drawing the
//     conclusion: `Channel` has no `try_send`, its buffer is a sleeping `Mutex<VecDeque>`, and its
//     `send`/`recv` assert they run on a scheduled task. Every one of those is fatal to the producer
//     contexts §3 names — IRQ-masked code and code holding the print lock may not sleep, may not
//     allocate, and may not be on a task at all — and a blocking `send` from inside `dispatch_command`
//     would push onto a queue only the blocked render task can drain, which is the deadlock §3 warns
//     about one paragraph earlier. [`LineRing`] (serial_ring.rs) is the same 64-slot bound with none
//     of that: three atomics, no Mutex, no allocation, no reentrancy into `serial_println!`, and a
//     [`Staged::Full`] return that hands the caller a counted refusal instead of a wait.
//
//  3. **OVERFLOW IS COUNTED, AND SAID OUT LOUD.** A truncated session must be VISIBLY truncated.
//     [`TERM_TAP`] is a [`TapCounters`] — the same ledger the four SERWIT-2 mirror taps keep, under
//     the same conservation law
//
//         submitted == absorbed + dropped + suppressed + in_flight
//
//     and [`service`] announces un-announced loss on the wire, on change only. TERM_TAP is
//     deliberately NOT enrolled in `serial_ring::taps()`: that array is the census of taps on the
//     SERIAL wire, and this ring is not one of them.
//
// ── Drop-newest costs ORDER here, not content ───────────────────────────────────────────────────
//
// [`crate::console::Console::println`] stages through this ring and then drains it, because on the
// surfaces that exist today the producer IS the consumer's task. When the ring refuses a record the
// console falls back to pushing the line straight into `history`, so a transport drop never costs
// the operator a line of output — it costs the ring's ORDERING guarantee for that line, and it is
// counted as `dropped` regardless. Counting it is what keeps the ledger honest about a transport
// that could not carry the traffic offered to it; the fallback is what keeps the panel correct while
// it is.
//
// ── Arch neutrality ─────────────────────────────────────────────────────────────────────────────
//
// Nothing here is arch-gated. The DRAIN SITE is: `main.rs`'s `handle_key` drains immediately before
// the post-command `console.draw(pal)` — the point §3 identifies, where `dispatch_command` has
// returned and the render task holds the view again. That site is shared by the x86 render service
// and the aarch64 BSP GUI loop, and draining an empty ring is a no-op, so aarch64 behaviour is
// unchanged by construction.

use core::fmt;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::serial_ring::{LineRing, Staged, TapCounters};

/// Records the ring holds. 64, matching `GUI_CHANNEL_X86` per §3: the consumer is the same render
/// task, on the same core, at the same tempo, so a deeper ring only buys latency before the same drop.
pub const TERM_SLOTS: usize = 64;

/// Bytes per record. Fixed and inline — no `alloc::String` — because producers include IRQ-masked
/// contexts and contexts holding the print lock, where an allocation is not permitted. Comfortably
/// wider than a panel row at any scale this kernel drives (1920 px at the scale-2 cell is 120 cells);
/// a longer line is sealed with `serial_ring::TRUNCATION_MARK` and counted as a tear, never silently
/// shortened.
pub const TERM_LINE: usize = 240;

static TERM_RING: LineRing<TERM_SLOTS, TERM_LINE> = LineRing::new();

/// This ring's ledger. Public so a witness can read the conservation law off it.
pub static TERM_TAP: TapCounters = TapCounters::new();

/// The fixture hold. While set, [`console_out`] declines every record (counted as `suppressed`, which
/// is what that counter is for — "declined by policy", not lost) and [`drain`] yields nothing. The
/// selftest raises it so its own staged records are the ONLY records in the ring: a foreign console
/// line arriving mid-fixture would otherwise interleave and make the order assertion flaky, which is
/// a gate that fails for a reason unrelated to the property it tests.
static HOLD: AtomicBool = AtomicBool::new(false);

/// Stage one record, bypassing the hold. The fixture's own producer.
fn stage_raw(args: fmt::Arguments) -> Staged {
    TERM_TAP.submit();
    let outcome = TERM_RING.stage(args);
    match outcome {
        Staged::Whole => TERM_TAP.note_staged(),
        Staged::Truncated => {
            TERM_TAP.note_staged();
            TERM_TAP.tear();
        }
        Staged::Full => TERM_TAP.drop_line(),
    }
    outcome
}

/// Offer one formatted line to the terminal transport. Returns `true` iff the record is now IN the
/// ring (whole or sealed-truncated) and will reach the view at the next drain; `false` means the
/// caller still owns the line and must place it itself.
///
/// Wait-free apart from a CAS retry, allocation-free, and it never reenters `Console`. Safe from an
/// IRQ-masked context and from inside the print lock.
pub fn console_out(args: fmt::Arguments) -> bool {
    if HOLD.load(Ordering::Acquire) {
        TERM_TAP.submit();
        TERM_TAP.suppress();
        return false;
    }
    !matches!(stage_raw(args), Staged::Full)
}

/// [`console_out`] for a line that is already a `&str`.
#[inline]
pub fn console_out_str(text: &str) -> bool {
    console_out(format_args!("{}", text))
}

/// Emit every staged record, in order, into `emit`, and return how many.
///
/// **Caller contract**, inherited from [`LineRing::drain`]: the caller must hold the view
/// exclusively, which is what makes the drainer unique. Today that is `&mut Console`.
pub fn drain<F: FnMut(&str)>(emit: F) -> u64 {
    if HOLD.load(Ordering::Acquire) {
        return 0;
    }
    drain_raw(emit)
}

fn drain_raw<F: FnMut(&str)>(emit: F) -> u64 {
    let n = TERM_RING.drain(emit);
    TERM_TAP.absorb_n(n);
    n
}

/// Records staged but not yet drained.
#[inline]
pub fn in_flight() -> u64 {
    TERM_RING.in_flight()
}

/// Announce transport loss on the WIRE, on change only.
///
/// Self-rate-limiting on `TapCounters::take_pending`: a ring that is not losing records prints
/// nothing at all, and one that is prints once per burst rather than once per record. Call from an
/// IF=1, unlocked, non-print context — it prints, and the drain site is exactly such a context.
pub fn service() {
    if TERM_TAP.take_pending() > 0 {
        serial_println!(
            ":: termring: {} record(s) dropped, {} truncated since boot (transport full — {} slots x {} B; the console's direct-push fallback kept the text, the ring lost the ordering) == witness ::",
            TERM_TAP.dropped.load(Ordering::Relaxed),
            TERM_TAP.torn.load(Ordering::Relaxed),
            TERM_SLOTS,
            TERM_LINE,
        );
    }
}

/// The conservation-law tuple: `(submitted, absorbed, staged, dropped, suppressed, torn, in_flight)`.
/// The law is `submitted == absorbed + dropped + suppressed + in_flight`; `staged` and `torn` are
/// diagnostics that cut ACROSS the partition (a sealed-truncated record is still absorbed) and are
/// deliberately not terms in it.
pub fn ledger() -> (u64, u64, u64, u64, u64, u64, u64) {
    (
        TERM_TAP.submitted.load(Ordering::Relaxed),
        TERM_TAP.absorbed.load(Ordering::Relaxed),
        TERM_TAP.staged.load(Ordering::Relaxed),
        TERM_TAP.dropped.load(Ordering::Relaxed),
        TERM_TAP.suppressed.load(Ordering::Relaxed),
        TERM_TAP.torn.load(Ordering::Relaxed),
        TERM_RING.in_flight(),
    )
}

/// The fixture's record body for sequence `i`, formatted into `buf`; returns the byte length. One
/// function, used BOTH to produce the records and to re-derive what the drain must hand back, so the
/// round-trip assertion compares against a recomputation rather than against a remembered value.
#[cfg(feature = "witness")]
fn fixture_line(i: usize, buf: &mut [u8; TERM_LINE]) -> usize {
    use core::fmt::Write;
    let mut w = crate::serial_ring::BoundedWriter {
        buf: buf.as_mut_ptr(),
        cap: TERM_LINE,
        n: 0,
        truncated: false,
    };
    let _ = write!(w, "TR-{:03} abcdefghijklmnopqrstuvwxyz", i);
    w.n
}

/// TERMRING — the transport's own witness: **does a producer that outruns the consumer lose exactly
/// the records the drop-newest policy says it should, keep the ones it kept in order and byte-exact,
/// and account for every record it was handed?**
///
/// Four properties, each able to fail on its own:
///
///  1. **Bound + refusal.** With the consumer parked, `TERM_SLOTS + 16` records are offered. Exactly
///     `TERM_SLOTS` may be accepted and exactly 16 refused, and the ring must report `TERM_SLOTS`
///     in flight. A ring that quietly grew, or that overwrote instead of refusing, fails here.
///  2. **Drop-NEWEST, order, and bytes.** The survivors must be sequences `0..TERM_SLOTS`, drained in
///     that order, each byte-identical to a freshly recomputed [`fixture_line`]. Drop-OLDEST would
///     hand back `16..TERM_SLOTS+16` and fail the very first comparison; a reordering drain fails the
///     sequence check; a mangled slot fails the byte check.
///  3. **Truncation is SEALED, not silent.** A record longer than [`TERM_LINE`] must come back at
///     most `TERM_LINE` bytes, ending in `serial_ring::TRUNCATION_MARK`, with the tear counted.
///  4. **The conservation law.** `submitted == absorbed + dropped + suppressed + in_flight` over the
///     whole fixture. This is the property that catches an accounting hole rather than a mechanism
///     one: any path that consumes a record without charging the ledger breaks it.
///
/// One-shot, in-RAM, and self-cleaning: it raises [`HOLD`] so no foreign line can enter, leaves the
/// ring empty, and restores the counters' *invariants* (not their values — a ledger that could be
/// rewound would not be a ledger). `pre=` on the verdict line reports anything it had to clear out
/// before it started, so a nonzero is visible rather than absorbed into the result.
#[cfg(feature = "witness")]
pub fn termring_selftest() {
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }

    const EXTRA: usize = 16;
    const N: usize = TERM_SLOTS + EXTRA;

    HOLD.store(true, Ordering::Release);

    // Anything the ring happened to be holding is drained (and charged) first, so leg 1 starts from a
    // known depth. On a quiet boot this is 0.
    let pre = drain_raw(|_| {});

    // ── LEG 1: the bound and the refusal ────────────────────────────────────────────────────────
    let mut accepted = 0usize;
    let mut refused = 0usize;
    for i in 0..N {
        let mut buf = [0u8; TERM_LINE];
        let n = fixture_line(i, &mut buf);
        let body = core::str::from_utf8(&buf[..n]).unwrap_or("");
        match stage_raw(format_args!("{}", body)) {
            Staged::Full => refused += 1,
            _ => accepted += 1,
        }
    }
    let bound_ok = accepted == TERM_SLOTS && refused == EXTRA && in_flight() == TERM_SLOTS as u64;

    // ── LEG 2: drop-newest, order, bytes ────────────────────────────────────────────────────────
    let mut seq = 0usize;
    let mut order_ok = true;
    let mut bytes_ok = true;
    let drained = drain_raw(|line| {
        let mut buf = [0u8; TERM_LINE];
        let n = fixture_line(seq, &mut buf);
        let want = core::str::from_utf8(&buf[..n]).unwrap_or("");
        if line != want {
            bytes_ok = false;
            // Separate "wrong record" from "right record, wrong bytes": the 6-byte `TR-nnn` head is
            // the sequence, so a head mismatch is an ORDER (or drop-policy) failure specifically.
            if line.len() < 6 || want.len() < 6 || line.as_bytes()[..6] != want.as_bytes()[..6] {
                order_ok = false;
            }
        }
        seq += 1;
    });
    let drain_ok = drained == TERM_SLOTS as u64 && in_flight() == 0;

    // ── LEG 3: truncation is sealed and counted ─────────────────────────────────────────────────
    let torn_before = TERM_TAP.torn.load(Ordering::Relaxed);
    // 26 x 12 = 312 bytes of body, comfortably past TERM_LINE.
    let long_staged = stage_raw(format_args!(
        "TR-LONG {}",
        "abcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyzabcdefghijklmnopqrstuvwxyz"
    )) == Staged::Truncated;
    let mut trunc_len = 0usize;
    let mut trunc_marked = false;
    let trunc_drained = drain_raw(|line| {
        trunc_len = line.len();
        trunc_marked = line.ends_with(crate::serial_ring::TRUNCATION_MARK);
    });
    let trunc_ok = long_staged
        && trunc_drained == 1
        && trunc_len <= TERM_LINE
        && trunc_marked
        && TERM_TAP.torn.load(Ordering::Relaxed) == torn_before + 1;

    HOLD.store(false, Ordering::Release);

    // ── LEG 4: the conservation law ─────────────────────────────────────────────────────────────
    let (sub, abs, stg, drp, sup, torn, inflt) = ledger();
    let law_ok = sub == abs + drp + sup + inflt;

    let ok = bound_ok && order_ok && bytes_ok && drain_ok && trunc_ok && law_ok;
    serial_println!(
        ":: TERMRING: transport ring slots={} len={} pre={} offered={} accepted={} refused={} drained={} bound={} order={} bytes={} trunc={}(len={} sealed={}) law={} ledger[sub={} abs={} stg={} drp={} sup={} torn={} inflight={}] :: {} ::",
        TERM_SLOTS,
        TERM_LINE,
        pre,
        N,
        accepted,
        refused,
        drained,
        bound_ok,
        order_ok,
        bytes_ok,
        trunc_ok,
        trunc_len,
        trunc_marked,
        law_ok,
        sub,
        abs,
        stg,
        drp,
        sup,
        torn,
        inflt,
        if ok { "PASS" } else { "FAIL" }
    );
}
