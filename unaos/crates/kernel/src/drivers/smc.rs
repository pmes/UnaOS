// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! BATMON-1 — Apple SMC (System Management Controller) polled key/value driver (UNAOS_SMC=1).
//!
//! Peter's end goal: run the 2012 MacBook Pro off its battery with an on-screen battery monitor.
//! The Apple SMC speaks a **polled** key/value protocol over two legacy ISA I/O ports — data at
//! `0x300`, command/status at `0x304` — with 4-character keys and typed values. There are no
//! interrupts, no DMA, and no ACPI interpreter involved: the driver drives the ports directly and
//! waits (bounded) on the status byte.
//!
//! ## Write surface (tripwire-grade, brief §Write surface)
//! Every port access here is to `0x300` (data) or `0x304` (command/status) and ONLY under the
//! `smc` feature (`UNAOS_SMC=1`). During a READ transaction the driver writes the 4 key-name bytes
//! and one length byte to the *data* port — those are the read protocol's arguments, NOT a
//! state-changing SMC write. The value-mutating `WRITE_CMD` (0x11) is **never issued** by this
//! module (writing an SMC key changes machine state — fan speed, LEDs — and is out of scope; any
//! need for it is a STOP-and-report per the brief). The error/interrupt port (`0x31e`) is
//! deliberately **not** touched: absent-vs-stuck is disambiguated from the `0x304` status byte
//! alone, keeping the surface exactly `{0x300, 0x304}`.
//!
//! ## Bounded handshakes (never forced)
//! Every status wait is bounded by an `rdtsc` deadline. A handshake that does not settle inside the
//! budget returns `SmcError::Stuck` and the caller emits a traced STOP-NOTE line — the driver never
//! spins forever and never forces a transaction through a wedged status bit.
//!
//! ## QEMU vs metal (honest by construction)
//! QEMU's `isa-applesmc` answers `READ_CMD` (0x10) over a tiny key set (`REV`, `OSK0/1`, and a few
//! status keys) and carries **no battery keys**; the build gated here also implements **neither**
//! `GET_KEY_BY_INDEX` (0x12) nor a `#KEY` count (empirically: the M1 scout reports both unavailable
//! on QEMU). The driver does not depend on that — it handles the success, `Absent`, and bounded
//! `Stuck` outcomes identically whatever the model supports. So:
//!   * the QEMU gate is a *known-key read* (`REV`) + a *probe-by-name sweep* that reports which
//!     curated keys the emulated SMC exposes;
//!   * key **enumeration** (`#KEY` + `GET_KEY_BY_INDEX`) and every **battery** key are metal-first
//!     by construction — on QEMU they report cleanly as absent/unsupported (bounded, never a hang),
//!     and the first attended 2012 sitting yields the machine's real battery-key inventory.
//!
//! x86_64 only; the whole module is unlinked when the knob is off (media byte-identical).

use spin::Mutex;
use x86_64::instructions::port::Port;

/// Data port (key bytes out, value bytes in). Brief write surface.
const SMC_DATA_PORT: u16 = 0x300;
/// Command/status port (command byte out, status byte in). Brief write surface.
const SMC_CMD_PORT: u16 = 0x304;

/// Command bytes (Apple SMC / QEMU `isa-applesmc`).
const CMD_READ: u8 = 0x10;
const CMD_GET_KEY_BY_INDEX: u8 = 0x12;

/// Status-byte bits (low nibble is the live handshake state; QEMU masks with 0x0f).
const ST_MASK: u8 = 0x0f;
const ST_CMD_DONE: u8 = 0x00;
const ST_DATA_READY: u8 = 0x01;
/// BUSY (0x02): the real Apple SMC raises this while it shifts the next value byte into the 0x300
/// data register — DATA_READY momentarily de-asserts under it. The inter-byte drain waits for BUSY
/// to clear before re-inspecting DATA_READY (GAP-1 fix). QEMU's `isa-applesmc` never sets it, so the
/// wait is a no-op there and the emulated drain is byte-identical.
const ST_BUSY: u8 = 0x02;
const ST_ACK: u8 = 0x04;
const ST_NEW_CMD: u8 = 0x08;
/// Expected status after a command byte: NEW_CMD|ACK.
const ST_AFTER_CMD: u8 = ST_NEW_CMD | ST_ACK; // 0x0c
/// Expected status after a key-name byte: ACK.
const ST_AFTER_ARG: u8 = ST_ACK; // 0x04

/// Bounded handshake budget in `rdtsc` cycles. SMC handshakes are µs-scale; this is ≈0.1 s at a
/// 2.3 GHz Ivy Bridge and ≈0.25 s under QEMU/TCG — long enough that a healthy SMC never trips it,
/// short enough that a wedged status bit (or an absent device) fails fast on a serial-less laptop.
const SMC_WAIT_CYCLES: u64 = 250_000_000;

/// Failure modes of a single key read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SmcError {
    /// The key does not exist on this SMC (status settled to CMD_DONE with no DATA_READY). Clean.
    Absent,
    /// A status handshake did not settle inside the bounded budget. Traced STOP-NOTE; never forced.
    /// The payload names the step (0 = command, 1 = key arg, 2 = length/lookup, 3 = data byte,
    /// 4 = pre-command residue drain — see `STEP_RESIDUE`; that one fails *before* the transaction
    /// starts, and its `pre=` on the DIAG line is the residue that defeated the drain).
    Stuck(u8),
}

/// KEY-SHAPE — what this boot has learned about whether a key exists on THIS SMC.
///
/// **SMC-DIAG honesty (GR17).** `SmcError::Absent` is the read protocol's *successful negative
/// answer*: the SMC looked the key up and settled to `CMD_DONE` — "no such key". It is not a
/// failure. Treating it as one made the one-shot `SMC-DIAG` line fire on `AC-W` — a key this
/// machine is **known** not to carry (`battery::AcDerived`; doc Caveat 1, metal-confirmed
/// 2026-07-25) — on every boot, ~4 ms into the scout. Two separate things were wrong with that:
///
///   * **It published proof of health under a failure headline.** The metal timeline reads
///     `[40 ×16]`; `40 & ST_MASK` is `0x00` = `ST_CMD_DONE`, and it is byte-identical to the cold
///     `SMC-DIAG: pre-touch … raw status=0x40` the same boot prints two lines earlier — i.e. the
///     "evidence" was the idle status of a healthy controller. The dump's own rubric only names
///     dead-flat `00`/`ff`, busy-wedged and oscillating; flat-at-idle is not a fault shape.
///   * **It consumed the diagnostic slot.** `dump_first_failure` fires ONCE per boot, and `AC-W` is
///     the first key in `PROBE_KEYS` order to return any `Err`, deterministically. So a documented
///     non-event spent the shot before any real failure could claim it: in the s73 capture boot 1
///     fired the DIAG on `AC-W absent` at 3252 ms and the first battery sweep dropped out entirely
///     98 ms later (`present=false … retries=11/11`) with nothing left to record the wire truth.
///
/// So absence is now **learned**, not alarmed at. A key that answers is `Present`; a key that
/// cleanly answers "no" is `Absent`; and the DIAG treats absence as a failure only when the key had
/// already proven it exists this boot — a genuine regression — or is `REV `, which the protocol
/// itself requires and which is therefore seeded `Present` below. `Stuck` still fires the DIAG
/// unconditionally from any key: a wedged handshake, not a clean lookup miss, is what the
/// instrument was built to catch. **Nothing is weakened by this**: a missing/undecoded SMC still
/// reaches the DIAG, and so does a bus stuck at any constant.
///
/// The reason is the *sequence*, not any single byte (review finding 2 — an earlier draft argued
/// "a wedge cannot produce low-nibble `0x00`", which is plainly false: the healthy idle byte `0x40`
/// on this very machine IS low-nibble `0x00`). Reaching `Absent` at all means the transaction got
/// as far as the length step, and that requires **passing two different waits first**:
/// `wait_status(ST_AFTER_CMD = 0x0c, step 0)` after the command byte, then
/// `wait_status(ST_AFTER_ARG = 0x04, step 1)` after each of the four key bytes. No constant value
/// satisfies both `0x0c` and `0x04` — so a bus stuck at *anything*, `0x00` and `0xff` and `0x40`
/// alike, times out at step 0 and yields `Stuck(0)`, which fires the DIAG unconditionally. `Absent`
/// is only reachable from a controller that actively handshook its way through six exchanges and
/// then answered "no such key", which is the definition of a working SMC.
const SHAPE_UNSEEN: u8 = 0;
const SHAPE_PRESENT: u8 = 1;
const SHAPE_ABSENT: u8 = 2;

/// Slots in the learned key-shape table. `PROBE_KEYS` (19) plus the sweep's own keys, with room to
/// spare; a full table simply stops learning (reads still work, the DIAG just stays conservative).
const SHAPE_SLOTS: usize = 32;

struct KeyShapes {
    n: usize,
    keys: [[u8; 4]; SHAPE_SLOTS],
    shape: [u8; SHAPE_SLOTS],
}

/// Seeded with `REV ` = `Present`: the driver's own presence test reads it, and an SMC that answers
/// the ports but denies `REV ` is broken, not merely differently-equipped — that absence SHOULD
/// still reach the DIAG on the very first read, with nothing prior to compare against.
static SHAPES: Mutex<KeyShapes> = Mutex::new(KeyShapes {
    n: 1,
    keys: {
        let mut k = [[0u8; 4]; SHAPE_SLOTS];
        k[0] = *b"REV ";
        k
    },
    shape: {
        let mut s = [SHAPE_UNSEEN; SHAPE_SLOTS];
        s[0] = SHAPE_PRESENT;
        s
    },
});

/// What this boot knows about `key` so far (`SHAPE_UNSEEN` if it has never been read).
fn shape_of(key: &[u8; 4]) -> u8 {
    let t = SHAPES.lock();
    for i in 0..t.n {
        if t.keys[i] == *key {
            return t.shape[i];
        }
    }
    SHAPE_UNSEEN
}

/// Record `key`'s observed shape. A key already known `Present` is never demoted to `Absent`: the
/// fact that it once answered is exactly what makes a later absence a reportable regression, so it
/// must survive the regression rather than be overwritten by it.
fn note_shape(key: &[u8; 4], shape: u8) {
    let mut t = SHAPES.lock();
    for i in 0..t.n {
        if t.keys[i] == *key {
            if !(t.shape[i] == SHAPE_PRESENT && shape == SHAPE_ABSENT) {
                t.shape[i] = shape;
            }
            return;
        }
    }
    if t.n < SHAPE_SLOTS {
        let n = t.n;
        t.keys[n] = *key;
        t.shape[n] = shape;
        t.n = n + 1;
    }
}

#[inline]
fn read_status() -> u8 {
    // SAFETY: reading the SMC status at 0x304 is a pure port read (ring 0, no CR4.TSD). It has no
    // side effect on the transaction state in the QEMU model or on real Apple SMC hardware.
    unsafe { Port::<u8>::new(SMC_CMD_PORT).read() }
}

#[inline]
fn write_cmd(v: u8) {
    // SAFETY: 0x304 command write, brief write surface. Only READ_CMD / GET_KEY_BY_INDEX are ever
    // passed here (never the value-mutating WRITE_CMD).
    unsafe { Port::<u8>::new(SMC_CMD_PORT).write(v) }
}

#[inline]
fn write_data(v: u8) {
    // SAFETY: 0x300 data write, brief write surface — a key-name or length argument of a READ.
    unsafe { Port::<u8>::new(SMC_DATA_PORT).write(v) }
}

#[inline]
fn read_data() -> u8 {
    // SAFETY: 0x300 data read — one value byte. Advances the SMC's data cursor (that is the read
    // protocol), no other side effect.
    unsafe { Port::<u8>::new(SMC_DATA_PORT).read() }
}

/// Bounded inter-poll pause (~15 µs at 2.3 GHz) between status reads (BATMON-HOLD hardening).
/// The real Apple SMC misbehaves when the host hammers the status port back-to-back — the known
/// applesmc timing discipline is to space the polls out (Linux paces them ≥16 µs apart). The first
/// status check in every wait happens BEFORE any pause, so QEMU's instantly-ready model sees the
/// identical port-access sequence; only a not-yet-ready metal handshake gets the pacing. Pure
/// cycle-bounded spin — no timer, no sleep, and it does not extend the outer `SMC_WAIT_CYCLES`
/// deadline.
const SMC_POLL_PAUSE_CYCLES: u64 = 35_000;

fn poll_pause() {
    let start = crate::arch::now_cycles();
    while crate::arch::now_cycles().wrapping_sub(start) < SMC_POLL_PAUSE_CYCLES {
        core::hint::spin_loop();
    }
}

/// Poll the status port until its low nibble equals `want`, bounded by the cycle budget.
fn wait_status(want: u8, step: u8) -> Result<(), SmcError> {
    let start = crate::arch::now_cycles();
    loop {
        if read_status() & ST_MASK == want {
            return Ok(());
        }
        if crate::arch::now_cycles().wrapping_sub(start) >= SMC_WAIT_CYCLES {
            return Err(SmcError::Stuck(step));
        }
        poll_pause();
    }
}

/// Wait (bounded) until BUSY clears between value bytes (GAP-1 fix). On the real Apple SMC the
/// controller raises BUSY (0x02) while it shifts the next value byte into 0x300, momentarily
/// de-asserting DATA_READY; the inter-byte drain must let BUSY settle before it re-reads DATA_READY,
/// otherwise it mistakes the shift gap for end-of-value and truncates. QEMU never sets BUSY, so this
/// returns on the first status read there — the emulated drain is byte-identical. Bounded by the
/// same `rdtsc` budget as every other handshake; a genuine per-byte wedge yields `Stuck(step)`.
fn wait_busy_clear(step: u8) -> Result<(), SmcError> {
    let start = crate::arch::now_cycles();
    loop {
        if read_status() & ST_BUSY == 0 {
            return Ok(());
        }
        if crate::arch::now_cycles().wrapping_sub(start) >= SMC_WAIT_CYCLES {
            return Err(SmcError::Stuck(step));
        }
        poll_pause();
    }
}

/// Belt-and-suspenders idle-guard (GAP-2 fix, M2). M1's full inter-byte drain already leaves the SMC
/// idle (status = CMD_DONE) between transactions, so a fresh command's step-0 `NEW_CMD|ACK` wait
/// starts clean. As defence-in-depth against any residue left by an interrupted or truncated prior
/// read, this runs before each command byte: while the status still shows DATA_READY or BUSY set (a
/// stale partial read), it drains one leftover data byte at a time — a read of 0x300, the read
/// protocol's own cursor-advance, not a new write surface — under the bounded `rdtsc` budget, then
/// returns. On an idle SMC — always the case on QEMU between transactions — the very first status
/// read shows neither bit set, the loop body never runs, and no data byte is read: byte-behaviour-
/// identical (no port write, no extra data read). The deadline keeps it finite even if a wedged SMC
/// never settled; the command's own step-0 wait remains the real guard past that.
/// Step code for a pre-command residue drain that could not reach idle. Steps 0..=3 are the
/// transaction's own handshakes; this one happens *before* the command byte is written.
const STEP_RESIDUE: u8 = 4;

/// Drain cap for the pre-command residue clear (s73 Boot S). A *legitimate* stale value can never
/// exceed the largest buffer any caller passes — 32 bytes, the scout's — so 64 is 2x the maximum
/// residue the protocol can produce and cannot truncate a real drain. What it does cut short is a
/// data phase that will not terminate: the old loop drained for the full `SMC_WAIT_CYCLES`, which at
/// the ~15 us pacing quantum is on the order of 6600 reads, and Boot S proves that did not clear it.
/// Failing at 64 reads (~1 ms) instead of ~109 ms turns a silent, expensive loss into a fast,
/// attributed one; nothing that used to succeed can stop succeeding.
const MAX_RESIDUE_DRAIN: u32 = 64;

fn settle_before_command() -> Result<(), SmcError> {
    let start = crate::arch::now_cycles();
    let mut drained: u32 = 0;
    loop {
        let st = read_status();
        // Idle is the whole test: the controller is ready for a command exactly when its low nibble
        // is CMD_DONE. Anything else is residue — this generalizes the old `DATA_READY|BUSY` bit
        // list and so also covers a lingering `NEW_CMD`/`ACK` (command-phase residue), which the old
        // test could not see at all. Note the observed s73 residue is `DATA_READY|ACK`, i.e. inside
        // the old mask already; the NEW_CMD arm is defence-in-depth, not the fix for that.
        if st & ST_MASK == ST_CMD_DONE {
            return Ok(());
        }
        if st & ST_DATA_READY != 0 && drained < MAX_RESIDUE_DRAIN {
            let _ = read_data(); // drain a stale value byte to unstick the previous transaction
            drained += 1;
        } else if drained >= MAX_RESIDUE_DRAIN {
            return Err(residue_fail(st, drained)); // draining is not working; stop paying for it
        }
        if crate::arch::now_cycles().wrapping_sub(start) >= SMC_WAIT_CYCLES {
            return Err(residue_fail(st, drained));
        }
        poll_pause();
    }
}

/// The residue clear gave up. **This used to `break` in silence** — the single most consequential
/// line in this driver's history of quiet instruments. Boot S caught it: `pre=0x45` is
/// `DATA_READY|ACK`, and `DATA_READY` was *already in the old loop's mask*, so the guard had seen
/// the residue, drained against it for its full budget, failed, said nothing, and let the command go
/// out anyway — after which step 0 timed out and the DIAG reported a `stuck step 0` whose real cause
/// had happened 109 ms earlier and left no trace. A guard that can lose must be able to say so.
fn residue_fail(status: u8, drained: u32) -> SmcError {
    use core::sync::atomic::{AtomicBool, Ordering};
    static NOTED: AtomicBool = AtomicBool::new(false);
    RESIDUE_FAILS.fetch_add(1, Ordering::Relaxed);
    // `pre=` on the DIAG line reports the residue that defeated the drain, not a status read after
    // it — so a `Stuck(4)` names the condition that actually stopped the transaction.
    LAST_PRE_CMD_STATUS.store(status, Ordering::Relaxed);
    if !NOTED.swap(true, Ordering::Relaxed) {
        serial_println!(
            ":: SMC-DIAG: STOP-NOTE residue drain lost — status {:#04x} (low nibble {:#03x}) still not CMD_DONE after {} drained bytes; command NOT issued into it (bounded, not forced, noted once) == evidence ::",
            status,
            status & ST_MASK,
            drained
        );
    }
    SmcError::Stuck(STEP_RESIDUE)
}

/// Read the value of a 4-character key into `out`, returning the number of bytes read (up to
/// `out.len()`). The classic Apple SMC READ handshake: command 0x10, four key bytes, one length
/// byte, then value bytes while `DATA_READY` holds.
pub fn read_key(key: &[u8; 4], out: &mut [u8]) -> Result<usize, SmcError> {
    let r = read_txn(key, out);
    // SMC-DIAG dispatch (KEY-SHAPE, see above). The boot's FIRST genuinely failing key read — from
    // the scout or the battery sweep — dumps the raw status timeline, once. A clean `Absent` for a
    // key that has never answered is a *learned fact about this machine's key set*, not a failure,
    // and must not consume that one shot.
    match r {
        Ok(_) => {
            note_shape(key, SHAPE_PRESENT);
            r
        }
        Err(SmcError::Absent) if shape_of(key) == SHAPE_PRESENT => {
            // CORROBORATE (review finding 1). A key that already answered this boot cannot have
            // stopped existing, so this is either a real fault or a bad sample — and the sweep
            // retries a `Stuck` three times before believing it, so believing a *single* `Absent`
            // here was the weaker standard of the two. One re-read decides it. That matters because
            // firing spends the boot's only DIAG latch: the very failure mode this arc exists to
            // stop. `read_txn` serializes transactions so a concurrent reader can no longer drain
            // our data bytes and manufacture the `CMD_DONE` that reads as `Absent`; this is the
            // second line of defence, against any single-sample anomaly the lock does not cover
            // (a transient EC hiccup, a truncated prior transaction the idle-guard half-cleared).
            match read_txn(key, out) {
                Ok(n) => {
                    // The first read was the anomaly. The key is there, and we now hold its value:
                    // return it rather than propagating a hole the caller would have to re-read.
                    note_shape(key, SHAPE_PRESENT);
                    Ok(n)
                }
                Err(SmcError::Absent) => {
                    dump_first_failure(key, SmcError::Absent); // repeated: a real regression
                    Err(SmcError::Absent)
                }
                Err(e) => {
                    dump_first_failure(key, e); // wedged on the re-read: report what actually broke
                    Err(e)
                }
            }
        }
        Err(SmcError::Absent) => {
            note_shape(key, SHAPE_ABSENT);
            r
        }
        Err(e @ SmcError::Stuck(_)) => {
            dump_first_failure(key, e);
            r
        }
    }
}

/// SMC-TXN serialization (review finding 1). One key read is a multi-step conversation with a
/// stateful controller — command, four key bytes, a length byte, then N data reads that each
/// *advance the SMC's own cursor*. Two interleaved conversations do not merely race for a value:
/// one drains the other's data bytes, so the victim re-reads the status, sees the transaction it
/// never finished has completed (`ST_CMD_DONE`), and reports a **clean `Absent` for a key that is
/// present**. That is indistinguishable at the call site from the real thing, and under the
/// KEY-SHAPE rule above it would fire the one-shot DIAG on a non-fault — reintroducing this arc's
/// disease by a different door.
///
/// Interleaving is reachable today: the `batmon` shell verb calls `snapshot()` unthrottled, the
/// service-task and vug-cadence callers both call `refresh_if_due()`, and a sweep against a
/// wedging SMC runs well past the 1000 ms throttle, so two callers can both find it due.
///
/// **The lock is safe on every caller.** Every entry point — `pci::init` (boot), the `main.rs`
/// service-loop bodies, the vug meter cadence, the `batmon` shell verb, and `bench_ride`'s probes —
/// is ordinary task context; **no interrupt handler touches the SMC**, so an ISR cannot arrive and
/// spin on a lock its own interruptee holds. Re-entrancy is impossible too: `read_key_inner` calls
/// nothing but port I/O and `now_cycles`. Under a cooperative scheduler the holder never yields
/// mid-transaction, so the lock is never contended across a yield; under a preemptive one a spinner
/// burns its slice and the holder resumes. Hold time is one transaction, itself bounded by
/// `SMC_WAIT_CYCLES` per step — the same bound that already capped every caller's own wait.
static TXN: Mutex<()> = Mutex::new(());

/// One serialized SMC transaction. Every path that talks to the controller goes through here.
fn read_txn(key: &[u8; 4], out: &mut [u8]) -> Result<usize, SmcError> {
    let _guard = TXN.lock();
    read_key_inner(key, out)
}

/// STEP0-PRE: the status byte read immediately before the last command write (see
/// `read_key_inner`). Reported by the DIAG so a step-0 stall can be attributed.
static LAST_PRE_CMD_STATUS: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// STEP0-STALL CENSUS. The DIAG is one-shot by design, so it reports the boot's FIRST wedge and
/// says nothing about whether that wedge was a one-off or the first of hundreds — which is exactly
/// the transient-vs-standing question a fix would have to turn on. Counting them costs no output
/// (the total rides the existing retry-rollup line, which fires at most once per `RETRY_ROLLUP_MS`)
/// and makes the distinction measurable across a whole boot.
static STEP0_STALLS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn note_step0_stall() {
    STEP0_STALLS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

/// Pre-command residue drains that could not reach idle (`Stuck(STEP_RESIDUE)`).
static RESIDUE_FAILS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// `(step-0 stalls, residue-drain failures)` since boot.
///
/// CENSUS PLACEMENT (s73 Boot S). These rode the retry-ROLLUP line when they were added — and that
/// line fired **zero times in the entire s73 capture**, six boots and ~24 minutes of uptime. The
/// rollup only prints when the witness did NOT, and it resets its own clock every time the witness
/// fires; on this SMC the pack state flaps often enough that `fire` is true well inside the 300 s
/// period, so the rollup is starved permanently. A counter on a line that never prints is exactly
/// the disease this arc exists to treat, so the counts now ride the `SMC-BATT` witness itself.
pub fn stall_counts() -> (u32, u32) {
    use core::sync::atomic::Ordering;
    (STEP0_STALLS.load(Ordering::Relaxed), RESIDUE_FAILS.load(Ordering::Relaxed))
}

fn read_key_inner(key: &[u8; 4], out: &mut [u8]) -> Result<usize, SmcError> {
    // 0) settle any residue from a prior (interrupted) transaction so step-0 starts clean (M2
    //    idle-guard). No-op on an idle SMC — byte-identical on QEMU. Now FALLIBLE: if the residue
    //    cannot be cleared, the command is not issued into it and the caller gets `Stuck(4)` naming
    //    that, rather than a `Stuck(0)` 109 ms later that blames the wrong step.
    settle_before_command()?;

    // 1) command byte -> expect NEW_CMD|ACK.
    //
    // STEP0-PRE (s73, 2026-08-06): latch the status the instant BEFORE the command write. The
    // discriminator was added to decide whether a `stuck step 0` meant the SMC stalled on OUR
    // command (`pre` idle) or we wrote into residue (`pre` not idle). **Boot S answered it, and
    // neither of the two predicted values came back**: `pre=0x45` = idle-high | `ST_ACK` |
    // `ST_DATA_READY`, low nibble `0x05`.
    //
    // `ST_DATA_READY` was ALREADY in the old settle loop's mask. Since `pre` is sampled *after*
    // `settle_before_command`, that byte proves the guard ran, drained against the residue for its
    // entire budget, failed to reach idle, and exited through a silent `break` — then the command
    // went out into a controller still mid-data-phase and step 0 timed out 109 ms later. The `0x48`
    // the DIAG timeline shows is the *consequence* (our command latching `NEW_CMD` over the mess),
    // not the cause. The cause is upstream and now returns `Stuck(STEP_RESIDUE)` instead of nothing.
    LAST_PRE_CMD_STATUS.store(read_status(), core::sync::atomic::Ordering::Relaxed);
    write_cmd(CMD_READ);
    if let Err(e) = wait_status(ST_AFTER_CMD, 0) {
        note_step0_stall();
        return Err(e);
    }

    // 2) four key-name bytes -> each acks.
    for &b in key.iter() {
        write_data(b);
        wait_status(ST_AFTER_ARG, 1)?;
    }

    // 3) length byte. The SMC now looks the key up: found => ACK|DATA_READY; missing => CMD_DONE.
    write_data(out.len() as u8);
    let start = crate::arch::now_cycles();
    loop {
        let s = read_status() & ST_MASK;
        if s & ST_DATA_READY != 0 {
            break; // value ready
        }
        if s == ST_CMD_DONE {
            return Err(SmcError::Absent); // clean "no such key" — the QEMU path for #KEY/battery
        }
        if crate::arch::now_cycles().wrapping_sub(start) >= SMC_WAIT_CYCLES {
            return Err(SmcError::Stuck(2));
        }
        poll_pause();
    }

    // 4) drain value bytes (GAP-1 fix). Between bytes the real SMC momentarily de-asserts
    //    DATA_READY and raises BUSY while it shifts the next byte into 0x300, so per byte we first
    //    wait for BUSY to clear, then inspect DATA_READY: set => one more value byte; clear => the
    //    SMC has signalled end-of-value. Termination comes from the SMC's done-signal, NOT
    //    `out.len()`, so an oversized buffer (REV into `present`'s buf[8], the 32-byte scout buffer)
    //    still returns the true length (6 for REV) with no spurious Stuck. `out.len()` is only the
    //    safety cap that prevents writing past the caller's buffer. QEMU never raises BUSY and holds
    //    DATA_READY across all `len` bytes, so this drains byte-identically there.
    let mut n = 0;
    while n < out.len() {
        wait_busy_clear(3)?;
        if read_status() & ST_DATA_READY == 0 {
            break; // end-of-value signalled by the SMC (real key length < out.len())
        }
        out[n] = read_data();
        n += 1;
    }
    Ok(n)
}

/// SMC-DIAG (metal directive, 2026-07-18): one-shot bounded raw-handshake evidence dump on the
/// FIRST failed key read of the boot — usbdebug-independent (plain serial), fires exactly once,
/// regardless of any root-cause theory. Captures what four consecutive GUI-build sittings could
/// not: the actual wire behaviour at the moment the handshake fails. Dumps the failing key, the
/// failing step, uptime, the RAW (unmasked) status byte at failure, then a 16-sample status-byte
/// timeline (~15 µs apart — the applesmc pacing quantum) so the next sitting can read whether the
/// status is dead-flat (0x00/0xFF: device absent / decoded nowhere), busy-wedged, or oscillating.
/// Read-only: only 0x304 status reads — no new write surface.
///
/// GR17: this is now reached only for a `Stuck` handshake or an UNEXPECTED absence (KEY-SHAPE — a
/// key that had already answered this boot). It is no longer reachable from an ordinary "this
/// machine does not carry that key" answer, which is what burned the one shot on `AC-W` every boot
/// and left the instrument permanently spent before the first real fault of the boot.
fn dump_first_failure(key: &[u8; 4], err: SmcError) {
    use core::sync::atomic::{AtomicBool, Ordering};
    static FIRED: AtomicBool = AtomicBool::new(false);
    if FIRED.swap(true, Ordering::Relaxed) {
        return;
    }
    // `Absent` carries no step — the lookup completed, it just answered "no". The old code printed
    // the 0xFF sentinel straight into the numeric `step` field, so the line read `step 255`: a step
    // that does not exist (the real ones are 0..=3). It renders `n/a` now.
    let (kind, step) = match err {
        SmcError::Absent => ("absent-unexpected", alloc::string::String::from("n/a")),
        SmcError::Stuck(s) => ("stuck", alloc::format!("{}", s)),
    };
    let mut samples = [0u8; 16];
    for s in samples.iter_mut() {
        *s = read_status();
        poll_pause();
    }
    let name = core::str::from_utf8(&key[..]).unwrap_or("????");
    // `pre=` is the status immediately before the command write (STEP0-PRE). On a `stuck step 0` it
    // is the whole ballgame: `pre` idle means the SMC stalled on OUR command; `pre` already showing
    // NEW_CMD means we wrote into residue the idle-guard does not detect. On any other step/kind it
    // is merely the transaction's starting condition, still worth having.
    serial_println!(
        ":: SMC-DIAG: FIRST FAILURE key {} kind {} step {} t={}ms pre={:#04x} — raw status timeline [{}] (16 reads, ~15us apart) == evidence ::",
        name,
        kind,
        step,
        crate::arch::ms(),
        LAST_PRE_CMD_STATUS.load(Ordering::Relaxed),
        fmt_hex(&samples)
    );
}

/// True if an SMC answers on the ports — detected by reading the always-present `REV ` key.
pub fn present() -> bool {
    let mut buf = [0u8; 8];
    matches!(read_key(b"REV ", &mut buf), Ok(n) if n >= 1)
}

/// Read one key at `index` via GET_KEY_BY_INDEX (0x12). Metal-only: QEMU does not implement 0x12,
/// so this returns `Stuck`/`Absent` there (bounded) and the scout reports enumeration unavailable.
///
/// **Does NOT route to `dump_first_failure`, deliberately** (review finding 5 — the over-claim that
/// the DIAG covered "scout, battery sweep, enumeration" has been dropped from `read_key`, because
/// this path never reached it). Routing it would be actively wrong here: Caveat 3 records that
/// `#KEY` enumeration is a **standing** bounded wedge on this machine, so wiring it to the one-shot
/// latch would re-spend the DIAG on a known condition every boot — precisely the disease this arc
/// removed from `AC-W`. The information is not lost: the scout's enumeration handler now reports
/// `Absent` and `Stuck(step)` as the distinct outcomes they are, instead of collapsing both into
/// "unsupported or stuck". If enumeration ever stops being a standing wedge, that line is the
/// evidence for routing it.
///
/// Serialized on `TXN` like every other transaction: it drives the same stateful cursor, so it can
/// both corrupt and be corrupted by an interleaved `read_key`.
fn read_key_by_index(index: u32, name: &mut [u8; 4]) -> Result<(), SmcError> {
    let _guard = TXN.lock();
    // 0) settle any residue from a prior transaction (M2 idle-guard; no-op on an idle SMC).
    settle_before_command()?;

    write_cmd(CMD_GET_KEY_BY_INDEX);
    wait_status(ST_AFTER_CMD, 0)?;
    for b in index.to_be_bytes() {
        write_data(b);
        wait_status(ST_AFTER_ARG, 1)?;
    }
    // The name is 4 bytes; the SMC readies them like a value read.
    write_data(4);
    let start = crate::arch::now_cycles();
    loop {
        let s = read_status() & ST_MASK;
        if s & ST_DATA_READY != 0 {
            break;
        }
        if s == ST_CMD_DONE {
            return Err(SmcError::Absent);
        }
        if crate::arch::now_cycles().wrapping_sub(start) >= SMC_WAIT_CYCLES {
            return Err(SmcError::Stuck(2));
        }
        poll_pause();
    }
    // A key name is exactly 4 bytes; mirror read_key's per-byte BUSY-then-DATA_READY handshake
    // (GAP-1 fix). Unlike a value read an early DATA_READY-clear here IS an error (a 4-byte name
    // must fully drain), so it stays Stuck(3) rather than a clean stop.
    for slot in name.iter_mut() {
        wait_busy_clear(3)?;
        if read_status() & ST_DATA_READY == 0 {
            return Err(SmcError::Stuck(3));
        }
        *slot = read_data();
    }
    Ok(())
}

/// Curated key inventory the scout probes by name. On QEMU only `REV `/`OSK0` (+ the status keys)
/// answer; on the 2012 rMBP the battery block resolves and the sitting log records exactly which of
/// these keys — and their byte payloads — the real SMC carries. NO key here is *assumed* present:
/// the scout reports each one present-or-absent. The battery + per-cell keys are the standard Apple
/// SMC names for that era; the metal inventory confirms the exact set and decides M2's fork.
const PROBE_KEYS: &[(&[u8; 4], &str)] = &[
    (b"REV ", "SMC firmware revision"),
    (b"OSK0", "OSK part 0 (QEMU sanity key)"),
    (b"#KEY", "key count (metal-only; enables index enumeration)"),
    (b"BNum", "battery count"),
    (b"BSIn", "battery status/info bitfield"),
    (b"BRSC", "relative state of charge (%)"),
    (b"B0AC", "battery 0 amperage (mA, signed)"),
    (b"B0AV", "battery 0 voltage (mV)"),
    (b"B0FC", "battery 0 full charge capacity (mAh)"),
    (b"B0RM", "battery 0 remaining capacity (mAh)"),
    (b"B0St", "battery 0 status bits"),
    (b"B0TF", "battery 0 time-to-full (min)"),
    // INVENTORY HOLE closed (GR17): `battery::snapshot()` reads `B0Pr` on every sweep to decide
    // `present`, but the scout never probed it — so its shape on this machine was undocumented, and
    // whichever way it answers was being discovered 1 Hz at a time inside the sweep instead of once
    // at boot. Scouting it also lets KEY-SHAPE learn it before the first sweep asks.
    (b"B0Pr", "battery 0 present flag (the sweep's presence key)"),
    (b"CHBI", "charger battery current"),
    (b"CHBV", "charger battery voltage"),
    (b"AC-W", "AC adapter wattage / presence"),
    (b"BC1V", "cell 1 voltage (mV) — per-cell fork probe"),
    (b"BC2V", "cell 2 voltage (mV) — per-cell fork probe"),
    (b"BC3V", "cell 3 voltage (mV) — per-cell fork probe"),
];

/// Upper bound on the index enumeration walk (metal `#KEY` is a few hundred; cap keeps the bounded
/// walk finite even on a misbehaving SMC).
const MAX_ENUM_KEYS: u32 = 512;

fn fmt_hex(bytes: &[u8]) -> alloc::string::String {
    use core::fmt::Write;
    let mut s = alloc::string::String::new();
    for (i, b) in bytes.iter().enumerate() {
        if i > 0 {
            let _ = s.write_char(' ');
        }
        let _ = write!(s, "{:02x}", b);
    }
    s
}

/// M1 scout — probe the SMC key surface and dump it to serial, so the first attended 2012 sitting
/// yields the machine's real battery-key inventory. Read-only w.r.t. SMC state (no WRITE_CMD).
/// Fires once at boot from `pci::init` under the `smc` feature.
pub fn scout() {
    serial_println!(":: SMC-SCOUT: begin (ports data={:#x} cmd={:#x}) ::", SMC_DATA_PORT, SMC_CMD_PORT);
    // SMC-DIAG: timestamp + raw pre-touch status byte BEFORE the first transaction — lets a
    // sitting compare when each build first touches the SMC (the GUI's quiet boot reaches this
    // point much earlier than the fbcon-heavy usbdebug boot) and what the status port reads cold.
    serial_println!(
        ":: SMC-DIAG: pre-touch t={}ms raw status={:#04x} ::",
        crate::arch::ms(),
        read_status()
    );

    if !present() {
        serial_println!(":: SMC-SCOUT: end (present=N — no SMC answered REV; metal-first battery keys) ::");
        return;
    }

    let mut probed = 0u32;
    let mut found = 0u32;
    for (key, desc) in PROBE_KEYS.iter() {
        probed += 1;
        let mut buf = [0u8; 32];
        match read_key(key, &mut buf) {
            Ok(n) => {
                found += 1;
                let name = core::str::from_utf8(&key[..]).unwrap_or("????");
                serial_println!(
                    ":: SMC-SCOUT: key {} present len={} bytes=[{}] ({}) ::",
                    name, n, fmt_hex(&buf[..n]), desc
                );
            }
            Err(SmcError::Absent) => {
                let name = core::str::from_utf8(&key[..]).unwrap_or("????");
                // CORROBORATE (review finding 4). This line is a *confident claim about the
                // machine* — "this SMC does not carry it" — and it seeds KEY-SHAPE for the rest of
                // the boot, so it must not rest on one un-retried observation when the sweep next
                // door retries a `Stuck` three times before believing it. Read it again; report
                // only what reproduces.
                let mut again = [0u8; 32];
                match read_txn(key, &mut again) {
                    Ok(n) => {
                        // The first read was the anomaly, not the key set. Record it as present.
                        found += 1;
                        note_shape(key, SHAPE_PRESENT);
                        serial_println!(
                            ":: SMC-SCOUT: key {} present len={} bytes=[{}] — first read said absent, second disagreed (bad sample, not an inventory fact) ({}) ::",
                            name, n, fmt_hex(&again[..n]), desc
                        );
                    }
                    Err(SmcError::Absent) => {
                        // Absence at scout time is the INVENTORY RESULT, not a fault: the SMC
                        // looked the key up, twice, and answered "no such key". Said plainly,
                        // because this is the reading that used to arrive dressed as
                        // `SMC-DIAG: FIRST FAILURE … == evidence`.
                        serial_println!(
                            ":: SMC-SCOUT: key {} absent (x2) — this SMC does not carry it (clean negative answer, not a fault) ({}) ::",
                            name, desc
                        );
                    }
                    Err(SmcError::Stuck(step)) => {
                        // Absent then Stuck is not an inventory fact at all — say so rather than
                        // banking the absence.
                        serial_println!(
                            ":: SMC-SCOUT: key {} STOP-NOTE inconsistent — first read absent, re-read wedged at step {} (no inventory claim made) ({}) ::",
                            name, step, desc
                        );
                    }
                }
            }
            Err(SmcError::Stuck(step)) => {
                let name = core::str::from_utf8(&key[..]).unwrap_or("????");
                // STOP-NOTE: a handshake wedged. Never forced; reported and skipped.
                serial_println!(
                    ":: SMC-SCOUT: key {} STOP-NOTE handshake stuck at step {} (bounded, not forced) ({}) ::",
                    name, step, desc
                );
            }
        }
    }

    // Index enumeration (metal-only). #KEY is a ui32 count; walk it via GET_KEY_BY_INDEX. On QEMU
    // #KEY is absent and 0x12 is unimplemented, so this reports "unavailable" and moves on.
    let mut keycount = [0u8; 4];
    match read_key(b"#KEY", &mut keycount) {
        Ok(n) if n >= 1 => {
            let mut count: u32 = 0;
            for b in &keycount[..n] {
                count = (count << 8) | (*b as u32);
            }
            serial_println!(":: SMC-SCOUT: #KEY count={} — walking index list ::", count);
            let cap = count.min(MAX_ENUM_KEYS);
            let mut walked = 0u32;
            for idx in 0..cap {
                let mut name = [0u8; 4];
                match read_key_by_index(idx, &mut name) {
                    Ok(()) => {
                        walked += 1;
                        let ns = core::str::from_utf8(&name).unwrap_or("????");
                        serial_println!(":: SMC-SCOUT: idx {} = {} ::", idx, ns);
                    }
                    // Absent and Stuck are different facts and no longer share a line (review
                    // finding 5): "the SMC has no key at this index" ends an enumeration normally,
                    // while a wedged handshake is a fault that happens to end it too. This path
                    // deliberately does not touch the one-shot DIAG — see `read_key_by_index`.
                    Err(SmcError::Absent) => {
                        serial_println!(
                            ":: SMC-SCOUT: index enumeration ended at idx {} — GET_KEY_BY_INDEX answered no-such-index (clean stop) ::",
                            idx
                        );
                        break;
                    }
                    Err(SmcError::Stuck(step)) => {
                        serial_println!(
                            ":: SMC-SCOUT: index enumeration STOP-NOTE at idx {} — handshake wedged at step {} (bounded, not forced; Caveat 3) ::",
                            idx, step
                        );
                        break;
                    }
                }
            }
            serial_println!(":: SMC-SCOUT: index walk done ({} of {} names) ::", walked, count);
        }
        _ => {
            serial_println!(":: SMC-SCOUT: index enumeration unavailable (no #KEY — QEMU/limited SMC; metal yields the full list) ::");
        }
    }

    // MASS-ABSENCE FLOOR (review finding 4). Every absent key now prints a confident, individually
    // reasonable "this SMC does not carry it" line — so an EC that acks `REV ` and then refuses
    // everything else would emit a page of calm inventory lines and NO diagnostic at all, because
    // no single read failed in a way the DIAG recognises. Mass absence is the silent path, and it
    // is the shape a half-dead controller takes. It cannot be judged per key; only in aggregate.
    //
    // The floor is what an SMC answering at all must manage: `REV ` (the presence probe itself) and
    // `OSK0`, both of which even QEMU's minimal model carries. Falling below it does not mean the
    // key set differs from our guesses — it means the controller is not really answering.
    const SCOUT_FOUND_FLOOR: u32 = 2;
    if found < SCOUT_FOUND_FLOOR {
        serial_println!(
            ":: SMC-SCOUT: STOP-NOTE mass absence — only {} of {} keys answered, below the floor of {} (REV + OSK0). This reads like a controller that acks the presence probe and refuses the rest, NOT a machine with a different key set; treat the absent lines above as unproven ::",
            found, probed, SCOUT_FOUND_FLOOR
        );
    }

    serial_println!(
        ":: SMC-SCOUT: end (present=Y probed={} found={}) == witness ::",
        probed, found
    );
}

/// M2 — the battery monitor. Reads the standard Apple SMC charge/voltage/amperage keys and caches a
/// snapshot for the on-screen meter (vug hook) + a throttled serial witness. PROVISIONAL key set
/// (the documented 2012-era names); the M1 metal inventory confirms/refines it and decides the
/// per-cell fork. On QEMU (no battery keys) the snapshot reads `present=false` and the meter renders
/// an honest empty state — never fabricated numbers.
pub mod battery {
    use super::{read_key, SmcError};
    use core::sync::atomic::{AtomicU32, Ordering};
    use spin::Mutex;

    /// IVY-AC — charge state *inferred* from the `B0AC` amperage sign, for machines where the
    /// `AC-W` key is ABSENT (metal fact: the 2012 rMBP has no AC-W, so `ac_present` can never
    /// resolve there and the witness printed a bare `?` forever).
    ///
    /// Inference and its limits, stated plainly:
    ///   * `B0AC` is a signed mA reading. Metal-confirmed on the 2012 rMBP that the sign flips
    ///     correctly with the adapter: **negative = discharging** (battery sourcing the machine),
    ///     **positive = charging** (adapter sourcing charge into the pack).
    ///   * Therefore `Discharging` implies the adapter is NOT carrying the load; `Charging`
    ///     implies it IS. That is an inference about the *adapter*, derived from the *battery*.
    ///   * **Ambiguity around 0 mA.** A pack that is full while the adapter is attached settles at
    ///     ~0 mA — indistinguishable, from amperage alone, from a machine idling on a battery that
    ///     happens to be drawing under the noise floor. Both land in `Idle`, which asserts nothing
    ///     about AC presence. `Idle` is a refusal to guess, not a claim.
    ///   * The deadband exists because the reading dithers by a few mA at rest; without it the
    ///     state would flap between charging and discharging on sensor noise.
    /// This never overrides a real `AC-W` answer — on a machine that carries the key, `ac_present`
    /// remains the truth and the derived state is only supplementary.
    #[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
    pub enum AcDerived {
        /// No `B0AC` reading this sweep — nothing to infer from.
        #[default]
        Unknown,
        /// `B0AC` positive beyond the deadband: charge flowing INTO the pack ⇒ adapter present.
        Charging,
        /// `B0AC` negative beyond the deadband: pack sourcing the machine ⇒ adapter not carrying.
        Discharging,
        /// `|B0AC|` within the deadband: full-on-adapter or resting-on-battery — indistinguishable.
        Idle,
    }

    impl AcDerived {
        pub fn as_str(self) -> &'static str {
            match self {
                AcDerived::Unknown => "unknown",
                AcDerived::Charging => "charging",
                AcDerived::Discharging => "discharging",
                AcDerived::Idle => "idle",
            }
        }
    }

    /// Deadband (mA) around zero inside which the amperage sign carries no information. Chosen
    /// above the observed at-rest dither of the 2012 pack and far below any real charge/discharge
    /// current (which runs hundreds to thousands of mA).
    const AC_IDLE_DEADBAND_MA: i16 = 32;

    fn derive_ac(amp_ma: Option<i16>) -> AcDerived {
        match amp_ma {
            None => AcDerived::Unknown,
            Some(ma) if ma > AC_IDLE_DEADBAND_MA => AcDerived::Charging,
            Some(ma) if ma < -AC_IDLE_DEADBAND_MA => AcDerived::Discharging,
            Some(_) => AcDerived::Idle,
        }
    }

    /// IVY-RETRY — re-reads consumed by the sweep currently in flight (reset at sweep start), and
    /// the cumulative count since boot. Both appear on the SMC-BATT witness as `retries=SWEEP/TOTAL`
    /// so a metal sitting can read *how often* the per-read drop-out caveat actually bites, rather
    /// than only seeing the holes it leaves behind. Counted, never acted on: the retry budget itself
    /// is unchanged and bounded.
    static RETRIES_SWEEP: AtomicU32 = AtomicU32::new(0);
    static RETRIES_TOTAL: AtomicU32 = AtomicU32::new(0);
    /// RETRIES-LATCH: `RETRIES_SWEEP` is zeroed at the top of every sweep, but the witness does not
    /// fire on every sweep (see `refresh_if_due`) — so a sweep whose count was never printed used to
    /// vanish when the next sweep zeroed the counter, and the worst sweeps (total drop-out,
    /// `present=false`) were exactly the silent ones. Every sweep now latches its final count here
    /// before returning, and the witness reads the latch, so the number reported is the last sweep
    /// that actually ran rather than whatever the current one happens to have reached.
    static RETRIES_LAST: AtomicU32 = AtomicU32::new(0);

    fn note_retry() {
        RETRIES_SWEEP.fetch_add(1, Ordering::Relaxed);
        RETRIES_TOTAL.fetch_add(1, Ordering::Relaxed);
    }

    /// Latch the in-flight sweep's retry count so it survives the next sweep's reset.
    fn latch_retries() {
        RETRIES_LAST.store(RETRIES_SWEEP.load(Ordering::Relaxed), Ordering::Relaxed);
    }

    /// `(retries consumed by the last completed sweep, retries since boot)`.
    pub fn retry_counts() -> (u32, u32) {
        (RETRIES_LAST.load(Ordering::Relaxed), RETRIES_TOTAL.load(Ordering::Relaxed))
    }

    /// A decoded battery reading. Every field is `Option` — a key the SMC lacks stays `None`
    /// (honest absence), never a placeholder value.
    #[derive(Clone, Copy, Default)]
    pub struct BatterySnapshot {
        pub present: bool,
        /// Relative state of charge, percent (`BRSC`).
        pub soc_pct: Option<u16>,
        /// Terminal voltage, millivolts (`B0AV`).
        pub volt_mv: Option<u16>,
        /// Instantaneous current, milliamps, signed (`B0AC`; +charging / −discharging convention
        /// is SMC-dependent and confirmed at the metal sitting).
        pub amp_ma: Option<i16>,
        /// Full-charge capacity, mAh (`B0FC`).
        pub full_mah: Option<u16>,
        /// Remaining capacity, mAh (`B0RM`).
        pub rem_mah: Option<u16>,
        /// AC adapter present (`AC-W` answered with a non-zero payload). Stays `None` on machines
        /// that lack the key entirely — the 2012 rMBP among them; see `ac_derived`.
        pub ac_present: Option<bool>,
        /// `AC-W` was present-but-wedged: the bounded retry budget was exhausted on non-`Absent`
        /// failures, so `ac_present` is `None` because of an SMC fault rather than because the key
        /// does not exist. Distinguishes a live fault from the rMBP's stable key-less shape.
        pub ac_stuck: bool,
        /// Charge state INFERRED from the `B0AC` sign when `AC-W` cannot answer (IVY-AC). Never a
        /// substitute for `ac_present` where that resolves; see [`AcDerived`] for the ambiguity.
        pub ac_derived: AcDerived,
    }

    static CACHE: Mutex<BatterySnapshot> = Mutex::new(BatterySnapshot {
        present: false,
        soc_pct: None,
        volt_mv: None,
        amp_ma: None,
        full_mah: None,
        rem_mah: None,
        ac_present: None,
        ac_stuck: false,
        ac_derived: AcDerived::Unknown,
    });
    /// Last refresh time (ms); 0 = never. Throttles the port I/O off the per-frame path.
    static LAST_MS: Mutex<u64> = Mutex::new(0);
    /// Time (ms) of the last GOOD reading (`present=true`); 0 = never. `CACHE` holds that reading
    /// until a newer good one lands (BATMON-HOLD), so the widget's staleness is `now - GOOD_MS`.
    static GOOD_MS: Mutex<u64> = Mutex::new(0);
    /// Whether the boot witness line has fired (once, when the first real reading lands).
    static WITNESSED: Mutex<bool> = Mutex::new(false);
    /// QUIET-WITNESS state key: the fields whose change makes a re-print worth a reader's attention.
    /// `present`, the charge percentage and the AC shape are the *state* of the pack; `volt/amp/rem`
    /// jitter every sweep on real hardware, so keying on them would re-print at 1 Hz forever and
    /// scroll the panel (`PANEL_CONSOLE` mirrors serial to the glass) — the exact thing the quiet-boot
    /// witness policy forbids. `None` = nothing witnessed yet.
    static LAST_STATE: Mutex<Option<(bool, Option<u16>, Option<bool>, bool, AcDerived)>> = Mutex::new(None);
    /// One-per-transition serial note for a failed refresh while a good reading is held
    /// (BATMON-HOLD evidence line; cleared when a good reading returns).
    static HOLDING: Mutex<bool> = Mutex::new(false);
    /// When the CURRENT hold began (ms); 0 = not holding. Drives `HOLD_NOTE_MS`.
    static HOLD_SINCE: Mutex<u64> = Mutex::new(0);
    /// Whether the CURRENT hold has printed its evidence line (so the release line pairs with it
    /// and never appears alone).
    static HOLD_NOTED: Mutex<bool> = Mutex::new(false);
    /// Whether ANY hold has ever been announced. The first one always prints — that is the fact a
    /// reader needs (this SMC drops sweeps) — and every later one has to earn it by lasting.
    static HOLD_EVER: Mutex<bool> = Mutex::new(false);
    /// Retry ROLLUP bookkeeping: the cumulative retry total a reader has already been shown, and
    /// when the rollup last reported (0 = clock not started).
    static ROLLED_TOTAL: Mutex<u32> = Mutex::new(0);
    static ROLLED_MS: Mutex<u64> = Mutex::new(0);

    // PWR ROLLUP bookkeeping
    struct PwrRollupState {
        samples: u32,
        unknown: u32,
        sum: i64,
        min: i64,
        max: i64,
        state: AcDerived,
        start_ms: u64,
        total_samples: u32,
        total_sum: i64,
        total_ms: u64,
    }
    static PWR_ROLLUP: Mutex<PwrRollupState> = Mutex::new(PwrRollupState {
        samples: 0,
        unknown: 0,
        sum: 0,
        min: 0,
        max: 0,
        state: AcDerived::Unknown,
        start_ms: 0,
        total_samples: 0,
        total_sum: 0,
        total_ms: 0,
    });
    const PWR_ROLLUP_MS: u64 = 10_000;

    const REFRESH_MS: u64 = 1000;
    /// QUIET-HOLD threshold. On the 2012 rMBP's flaky SMC a ONE-SWEEP drop-out is the normal case,
    /// not an event: the `holding` / `hold released` pair fired every second on s39 metal. After the
    /// first hold has been announced, a hold only earns a line once it has PERSISTED this long —
    /// i.e. it is no longer a blip but an outage. A hold that never reaches it stays silent on both
    /// ends (`HOLD_NOTED` keeps the release paired with its hold).
    const HOLD_NOTE_MS: u64 = 5_000;
    /// Retry ROLLUP period. `retries > 0` is NOT an event on this machine — s39 metal showed
    /// `retries=2/2, 2/4, 6/14, 3/33, 4/44 …`, i.e. virtually every sweep consumes some, so keying
    /// the witness on it re-printed at 1 Hz forever. Retry counts stay on the once-per-boot witness
    /// line and on every state-change line; anything the reader has NOT already been shown is
    /// reported by one rollup line, at most this often.
    const RETRY_ROLLUP_MS: u64 = if cfg!(feature = "bootlog") { 60_000 } else { 300_000 };
    /// Per-key bounded retry budget (BATMON-HOLD hardening). Metal evidence (2026-07-18 sitting:
    /// Boot A `present=true soc=51%`, Boot B minutes later `present=false full=9962mAh`) shows
    /// individual key reads failing intermittently on the real SMC while others succeed in the same
    /// sweep. Each attempt is already deadline-bounded (`SMC_WAIT_CYCLES`), so retrying a Stuck /
    /// short read a couple of times is bounded total work — never a busy-loop. A clean `Absent`
    /// (the SMC looked the key up and said no) is NOT retried.
    const READ_ATTEMPTS: u32 = 3;

    /// PROBE-ONCE re-probe period. A key set is fixed by the SMC's firmware, so a learned absence
    /// cannot go stale in practice — this exists so the learning stays **falsifiable** rather than
    /// becoming an article of faith. One transaction a minute per absent key instead of one a
    /// second.
    const ABSENT_REPROBE_MS: u64 = 60_000;
    /// When the last re-probe window opened (0 = never; the boot scout's own probe is separate).
    static ABSENT_LAST_PROBE_MS: Mutex<u64> = Mutex::new(0);
    /// Whether THIS sweep is a re-probe window. Decided once at the top of `snapshot()` so that all
    /// of the sweep's absent keys re-probe together on the same minute rather than each consuming
    /// the window in turn (which would stretch any one key's re-probe to 60 s x however many absent
    /// keys there are, and make the cadence depend on `PROBE_KEYS` order).
    static REPROBE_WINDOW: Mutex<bool> = Mutex::new(false);
    /// Whether the "AC presence is unknown on this machine" statement has been made (once a boot).
    static ACW_NOTED: Mutex<bool> = Mutex::new(false);

    /// PROBE-ONCE (review finding 7). True when `key` is known absent on this SMC and this sweep is
    /// not a re-probe window — i.e. the transaction can be skipped, because the answer is already
    /// known and cannot have changed.
    ///
    /// Generalized from the AC-W-only form: the argument was never about `AC-W`. Any key learned
    /// absent is a fixed property of the controller's firmware, and re-asking at 1 Hz forever buys
    /// nothing. `B0Pr` is the concrete reason this had to generalize — the sweep reads it every
    /// pass to decide `present`, its shape on this machine is unknown until the next boot, and if
    /// it comes back absent the AC-W-only form would have left it re-probing at 1 Hz permanently:
    /// a second standing cost of exactly the kind this arc removed.
    fn probe_once_skip(key: &[u8; 4]) -> bool {
        super::shape_of(key) == super::SHAPE_ABSENT && !*REPROBE_WINDOW.lock()
    }

    /// One key read's sweep-relevant outcome: value, clean absence, or an unresponsive handshake.
    enum KeyRead {
        Val(u16),
        Absent,
        Stuck,
    }

    /// Read a 2-byte key with the bounded retry budget. Every attempt after the first is a RETRY
    /// and is counted (IVY-RETRY) so the witness can report drop-out frequency; the budget itself
    /// is unchanged — `READ_ATTEMPTS` attempts max, each individually deadline-bounded, so total
    /// work stays bounded and no re-read can turn into a spin.
    fn read_u16k(key: &[u8; 4]) -> KeyRead {
        // PROBE-ONCE: a key this SMC is known not to carry needs no transaction. Returns exactly
        // what the read would have returned, so every caller downstream is unchanged.
        if probe_once_skip(key) {
            return KeyRead::Absent;
        }
        for attempt in 0..READ_ATTEMPTS {
            if attempt > 0 {
                note_retry(); // this pass is a re-read of a key that failed the previous one
            }
            let mut b = [0u8; 2];
            match read_key(key, &mut b) {
                Ok(2) => return KeyRead::Val(((b[0] as u16) << 8) | b[1] as u16),
                Err(SmcError::Absent) => return KeyRead::Absent, // clean absence — no retry
                _ => {} // Stuck / short read: bounded retry (each attempt itself deadline-bounded)
            }
        }
        KeyRead::Stuck
    }

    fn opt(r: KeyRead) -> Option<u16> {
        match r {
            KeyRead::Val(v) => Some(v),
            _ => None,
        }
    }

    /// Read all battery keys and return a fresh snapshot (unthrottled). Pure reads.
    ///
    /// **There is no SWEEP-ABORT here, and that is deliberate** (review finding 6). A comment
    /// describing a first-key-Stuck early return outlived the code until GR17; the block was
    /// removed by `6b34e1f7`, and reading that commit shows the removal was the *point*, not
    /// collateral: "a stuck key no longer invalidates the keys that answered … Metal showed why:
    /// volt, full and rem dropped out while amp still read, one sweep before BRSC stuck aborted
    /// everything and latched present=false. Three unplugged boots produced zero PWR windows
    /// because of it." Restoring the abort would restore that: on this SMC individual keys drop out
    /// independently, so keying the whole sweep on `BRSC` throws away every other key's good
    /// reading and manufactures a `present=false` the pack never had.
    ///
    /// Its stated purpose — not burning ~16 bounded stuck-handshake budgets per second on the vug
    /// cadence when the SMC is unresponsive — is still served, by the `FAIL_STREAK` backoff in
    /// `refresh_if_due` (1 s → 32 s on consecutive failed sweeps, reset by any good one). That
    /// throttles the *frequency* of expensive sweeps without discarding the keys that answered,
    /// which is the correct axis. The worst-case single sweep is still long; `TXN` serialization
    /// (see `read_txn`) is what keeps that from corrupting a concurrent reader.
    pub fn snapshot() -> BatterySnapshot {
        let mut s = BatterySnapshot::default();
        RETRIES_SWEEP.store(0, Ordering::Relaxed); // per-sweep retry counter (IVY-RETRY)

        // PROBE-ONCE: open a re-probe window at most once per `ABSENT_REPROBE_MS`, decided here so
        // every absent key in this sweep re-probes on the same minute (see `REPROBE_WINDOW`).
        let now_ms = crate::arch::ms();
        {
            let mut last = *ABSENT_LAST_PROBE_MS.lock();
            let due = last == 0 || now_ms.wrapping_sub(last) >= ABSENT_REPROBE_MS;
            if due {
                last = now_ms;
                *ABSENT_LAST_PROBE_MS.lock() = last;
            }
            *REPROBE_WINDOW.lock() = due;
        }

        s.soc_pct = opt(read_u16k(b"BRSC"));
        s.volt_mv = opt(read_u16k(b"B0AV"));
        s.amp_ma = opt(read_u16k(b"B0AC")).map(|u| u as i16);
        s.full_mah = opt(read_u16k(b"B0FC"));
        s.rem_mah = opt(read_u16k(b"B0RM"));
        // AC presence: any non-zero AC-W payload => adapter attached. Same bounded, counted retry
        // discipline as the numeric keys — a Stuck handshake gets one more chance before the field
        // becomes a hole; a clean Absent (the 2012 rMBP's answer: this machine has no AC-W) stops
        // immediately, and the derived state below takes over.
        //
        // AC-STUCK: exhausting the budget is NOT the same fact as a clean `Absent`, even though both
        // leave `ac_present = None`. "This machine has no AC-W" (the 2012 rMBP) is a stable property
        // the derived state legitimately covers; "AC-W is there but the handshake wedged" is a live
        // SMC fault a sitting needs to see. `ac_stuck` records which of the two produced the hole.
        s.ac_stuck = false;
        // AC-W PROBE-ONCE (GR17, generalized in review finding 7 — the skip itself now lives in
        // `probe_once_skip`). On a machine whose SMC has no `AC-W` — the 2012 rMBP, learned by the
        // boot scout — re-running the full transaction every second cannot change the answer; it
        // just spends an SMC transaction per second rediscovering a fixed property of the
        // firmware's key set. Once known absent, the read is skipped and `ac_present` stays `None`.
        //
        // NOT a cached claim: it is re-tested every `ABSENT_REPROBE_MS`, so if this ran on a machine
        // that does carry `AC-W` the skip self-corrects within a minute and `ac_present` resolves to
        // the direct answer. Falsifiable, not assumed.
        //
        // AC-STUCK-FLAP (review finding 3): a re-probe of a known-absent key is a **liveness poll,
        // not the sweep's AC read**, so its failure must not be reported as the sweep's AC state.
        // Letting it set `ac_stuck` made that flag alternate true (re-probe minute) / false (the
        // other 59 sweeps) forever; `ac_stuck` is in the `LAST_STATE` key, so each flip earned a
        // witness line — two extra lines a minute, permanently, from an instrument whose entire
        // purpose is to print once. It also falsified this arc's own prediction 6 (`ac=stuck` must
        // not appear). A liveness poll that wedges leaves the sweep's AC fields exactly as a
        // skipped sweep would.
        let acw_known_absent = super::shape_of(b"AC-W") == super::SHAPE_ABSENT;
        s.ac_present = 'acw: {
            if acw_known_absent {
                if probe_once_skip(b"AC-W") {
                    break 'acw None; // known absent, not a re-probe minute: AC presence unknown
                }
                // Say it ONCE, plainly, in place of the every-boot `FIRST FAILURE` line this
                // replaces: the machine has no AC-W, so AC presence is genuinely UNKNOWN and the
                // `ac=derived:…` field is an inference from the B0AC sign — never a measurement.
                let mut noted = ACW_NOTED.lock();
                if !*noted {
                    *noted = true;
                    serial_println!(
                        ":: SMC-BATT: AC-W is absent on this SMC (clean negative answer, not a fault) — AC presence is UNKNOWN; ac=derived:* is inferred from the B0AC sign, and the key is re-probed every {} ms == witness ::",
                        ABSENT_REPROBE_MS
                    );
                }
            }
            for attempt in 0..READ_ATTEMPTS {
                if attempt > 0 {
                    note_retry();
                }
                let mut acw = [0u8; 2];
                match read_key(b"AC-W", &mut acw) {
                    Ok(n) if n >= 1 => break 'acw Some(acw[..n].iter().any(|&x| x != 0)),
                    Err(SmcError::Absent) => break 'acw None, // key does not exist here — no retry
                    _ => {}
                }
            }
            // Budget exhausted on a non-Absent failure: wedged, not absent — EXCEPT when this read
            // was the liveness re-probe of a key already known absent (AC-STUCK-FLAP above). There,
            // a wedge tells us only that the poll did not land; it is not evidence that a key this
            // machine does not have is "there but stuck", and reporting it as such flapped the
            // witness twice a minute forever. Leave the sweep's AC shape as the skipped case.
            if !acw_known_absent {
                s.ac_stuck = true;
            }
            None
        };
        // IVY-AC: infer charge state from the B0AC sign. Computed always (it is cheap and costs no
        // port I/O); it is only *reported in place of* ac_present when AC-W could not answer.
        s.ac_derived = derive_ac(s.amp_ma);
        
        let b0pr = read_u16k(b"B0Pr");
        s.present = match b0pr {
            KeyRead::Val(v) => v != 0,
            KeyRead::Stuck => CACHE.lock().present,
            KeyRead::Absent => s.soc_pct.is_some() || s.volt_mv.is_some() || s.rem_mah.is_some(),
        };
        latch_retries();
        s
    }

    /// Refresh the cached snapshot if the throttle interval elapsed. Call freely from the main
    /// loops / the vug meter cadence — the port I/O runs at most once per `REFRESH_MS`.
    ///
    /// Concurrency: the transaction IS now internally serialized — see `read_txn`'s `TXN` lock,
    /// which this note used to merely prescribe ("if a future path ever drives the SMC from
    /// multiple cores, wrap the transaction in a lock"). The prescription was overdue and the
    /// premise understated the risk on two counts. Interleaving did not need a second core: the
    /// `batmon` shell verb calls `snapshot()` unthrottled, two `refresh_if_due` callers can both
    /// find the throttle expired when a sweep against a wedging SMC overruns `REFRESH_MS`, and the
    /// service-task and vug-cadence sites are distinct call paths. And the consequence is worse than
    /// a garbled read: one reader draining the other's data bytes leaves the victim seeing
    /// `ST_CMD_DONE` and reporting a clean **`Absent` for a present key** — which under KEY-SHAPE
    /// spends the boot's one-shot DIAG latch on a phantom.
    ///
    /// The throttle-then-transact sequence outside the lock can still race (two callers may both
    /// decide a refresh is due and both sweep). That is a wasted sweep, not a wrong reading, and it
    /// is bounded by the same throttle; the reading itself is now atomic per key.
    /// Consecutive failed sweeps since the last good one — drives the failure BACKOFF: the
    /// refresh interval doubles per failed sweep (1 s → 2 s → … capped at 32 s), so a machine
    /// whose SMC never answers pays the bounded stuck-handshake cost a couple of times a minute
    /// instead of every second (the other half of the vug-stall fix; a good sweep resets it).
    static FAIL_STREAK: Mutex<u32> = Mutex::new(0);

    pub fn refresh_if_due() {
        let now = crate::arch::ms();
        {
            let streak = *FAIL_STREAK.lock();
            let interval = REFRESH_MS << streak.min(5); // 1 s .. 32 s
            let mut last = LAST_MS.lock();
            if *last != 0 && now.wrapping_sub(*last) < interval {
                return;
            }
            *last = now;
        }
        let s = snapshot();
        {
            let mut streak = FAIL_STREAK.lock();
            *streak = if s.present { 0 } else { streak.saturating_add(1) };
        }
        // BATMON-HOLD: a failed sweep (present=false) must NOT clobber a previous good reading —
        // the metal SMC read is intermittent (2026-07-18 sitting evidence), and a widget that goes
        // dark on one bad sweep is worse than one that holds the last good number with an honest
        // staleness age. A good sweep replaces the cache and stamps GOOD_MS; a bad sweep leaves the
        // cache alone (the widget reads it plus its age via `cached()`). If there has never been a
        // good reading the honest empty state stays cached.
        if s.present {
            *CACHE.lock() = s;
            *GOOD_MS.lock() = now;
            let mut h = HOLDING.lock();
            if *h {
                *h = false;
                *HOLD_SINCE.lock() = 0;
                // QUIET-HOLD: the release line exists to close the hold line. If the hold was never
                // announced (a blip under `HOLD_NOTE_MS`), there is nothing to close — stay silent,
                // so the pair can never appear half-printed.
                let mut noted = HOLD_NOTED.lock();
                if *noted {
                    *noted = false;
                    serial_println!(":: SMC-BATT: good reading returned — hold released ::");
                }
            }
        } else if *GOOD_MS.lock() != 0 {
            let mut h = HOLDING.lock();
            let mut since = HOLD_SINCE.lock();
            if !*h {
                *h = true;
                *since = now;
                *HOLD_NOTED.lock() = false;
            }
            // QUIET-HOLD: announce the FIRST hold of the boot immediately (that this SMC drops
            // sweeps at all is the fact worth having), and every later one only once it has lasted
            // `HOLD_NOTE_MS` — a single-sweep drop-out is this machine's normal, and printing it
            // scrolled the panel at 1 Hz on s39. Either way it prints at most once per hold.
            let mut noted = HOLD_NOTED.lock();
            let mut ever = HOLD_EVER.lock();
            let held = now.wrapping_sub(*since);
            if !*noted && (!*ever || held >= HOLD_NOTE_MS) {
                *noted = true;
                *ever = true;
                serial_println!(
                    ":: SMC-BATT: sweep failed (present=false) — holding last good reading (age {} ms, held {} ms) ::",
                    now.wrapping_sub(*GOOD_MS.lock()),
                    held
                );
            }
        } else {
            *CACHE.lock() = s;
        }

        // QUIET-WITNESS (panel-witness audit): a quiet attended boot must show this witness EXACTLY
        // ONCE — `PANEL_CONSOLE` mirrors every `serial_println!` to the panel at the takeover seam, so
        // an unconditional 1 Hz re-print scrolls the glass forever and ruins any photograph of the
        // boot. Fire on the FIRST refresh (proves the M2 read path ran — the honest `present=false`
        // line on QEMU / a battery-less machine), then only when the *state* changed (see
        // `LAST_STATE`) or when the sweep consumed retries (RETRIES-LATCH: those are precisely the
        // ones worth seeing, `present=false` drop-outs included). Information is preserved — every
        // state change and every retrying sweep still prints; only the identical-repeat is dropped.
        // Under the `bootlog` feature (`UNAOS_BOOTLOG=1`, the boot-log-on-screen sitting mode) the
        // old full ~1 s cadence is restored unchanged, so a sitting can still watch discharge track.
        let (rsweep, rtotal) = retry_counts();
        let (stall0, resid) = super::stall_counts();
        let mut w = WITNESSED.lock();
        let mut last = LAST_STATE.lock();
        // QUIET-AC: the state key must carry the SETTLED ac shape, never the instantaneous one. On
        // s39 metal a single dashed `B0AC` read makes `derive_ac(None)` return `Unknown` for exactly
        // one sweep, which flipped the key twice (derived:… -> Unknown -> derived:…) and re-printed
        // the witness at 1 Hz with a transient `ac=?`. A momentary hole is not a state change: when
        // the derivation has nothing to say, the key keeps the last shape it did have. `ac_present`
        // and `ac_stuck` are unaffected — those are answers, not inferences — and the printed line
        // still reports the honest instantaneous value.
        let ac_key = if matches!(s.ac_derived, AcDerived::Unknown) {
            last.map_or(AcDerived::Unknown, |l| l.4)
        } else {
            s.ac_derived
        };
        // QUIET-PRESENCE (s41): the state key must carry the HELD presence, never the instantaneous
        // one — same philosophy as QUIET-AC above. On s41 metal this SMC alternates
        // `present=true` / `present=false` sweep to sweep (retries=2/178, 2/180, 2/301 …), and with
        // the raw `present` in the key EVERY flap was a state change, so the witness re-printed every
        // few seconds. A single failed sweep is not a state change: while BATMON-HOLD is active and
        // the hold is still younger than `HOLD_NOTE_MS` (the same blip/outage threshold the hold
        // notes use), the key keeps the WHOLE shape it had — presence and the components that go
        // dark with it (`soc_pct` reads `None` on exactly those sweeps, so keying on the raw value
        // would re-print regardless). A real removal still prints once: the hold outlives
        // `HOLD_NOTE_MS`, or there was never a good reading to hold (no-battery machines / QEMU), and
        // in both cases the key resolves to the honest `present=false`. The printed line is
        // untouched — it always reports the instantaneous sweep.
        let blip = !s.present && {
            let h = *HOLDING.lock();
            let since = *HOLD_SINCE.lock();
            h && now.wrapping_sub(since) < HOLD_NOTE_MS
        };
        let state = match (blip, *last) {
            (true, Some(prev)) => prev,
            _ => (s.present, s.soc_pct, s.ac_present, s.ac_stuck, ac_key),
        };
        let changed = last.map_or(true, |l| l != state);
        // Two disjoint predicates, never OR-ed together, so the `bootlog` arm is *byte for byte* the
        // pre-audit condition and that mode's log is unchanged.
        // RETRIES-LATCH RETIRED from the quiet arm (s39 metal): `retries > 0` is not a fire
        // condition. `retries=2/2, 2/4, 6/14, 3/33, 4/44 …` — on this SMC virtually EVERY sweep
        // consumes retries, so that disjunct made the predicate fire every second and the witness
        // scrolled the glass exactly as before the quiet fix. The counts are not lost: they ride on
        // the once-per-boot line and on every state-change line, and the ROLLUP below reports any
        // the reader has not already been shown. The `bootlog` arm is untouched.
        let fire = if cfg!(feature = "bootlog") {
            !*w || s.present || rsweep > 0
        } else {
            !*w || changed
        };
        if fire {
            *w = true;
            *last = Some(state);
            // Absent keys print the "-" sentinel — never a number a reader could mistake for a real
            // value (0 mA is a plausible amperage, so `None` must NOT read as 0). snapshot() itself
            // keeps the honest `None`; only this human-facing witness applies the sentinel.
            let fu = |o: Option<u16>| o.map(|v| alloc::format!("{}", v)).unwrap_or_else(|| "-".into());
            let fi = |o: Option<i16>| o.map(|v| alloc::format!("{}", v)).unwrap_or_else(|| "-".into());
            // `ac=` reports the DIRECT reading when AC-W answered; otherwise the IVY-AC inference
            // from the B0AC sign, tagged `derived:` so no reader can mistake it for a direct one.
            // `ac=?` now only appears when there is neither an AC-W answer nor a B0AC reading.
            // A wedged AC-W reads `stuck` — a live SMC fault, distinct from `derived:…` (the key is
            // genuinely absent and the B0AC sign stands in) and from `?` (no answer, nothing to infer).
            let ac = match (s.ac_present, s.ac_stuck, s.ac_derived) {
                (Some(true), _, _) => alloc::string::String::from("yes"),
                (Some(false), _, _) => alloc::string::String::from("no"),
                (None, true, _) => alloc::string::String::from("stuck"),
                (None, false, AcDerived::Unknown) => alloc::string::String::from("?"),
                (None, false, d) => alloc::format!("derived:{}", d.as_str()),
            };
            serial_println!(
                // `stall0=` / `resid=` are the step-0 and residue-drain census (see
                // `super::stall_counts`) — they ride HERE, not the rollup, because the rollup line
                // has never once fired on this machine. They are cumulative since boot: flat across
                // a boot means the wedge was a blip, climbing means it is standing.
                ":: SMC-BATT: present={} soc={}% volt={}mV amp={}mA full={}mAh rem={}mAh ac={} retries={}/{} stall0={} resid={} == witness ::",
                s.present,
                fu(s.soc_pct),
                fu(s.volt_mv),
                fi(s.amp_ma),
                fu(s.full_mah),
                fu(s.rem_mah),
                ac,
                rsweep,
                rtotal,
                stall0,
                resid,
            );
        }

        // RETRY ROLLUP. The witness line above already carries `retries=SWEEP/TOTAL`, so whenever it
        // fires the reader is up to date — record that and say nothing. Between fires, retries keep
        // accruing invisibly; this reports the ones NOT yet shown, at most once per
        // `RETRY_ROLLUP_MS`. That preserves the information the retired fire condition used to carry
        // while costing one line per period instead of one per second.
        //
        // Under `bootlog` the arm above fires on every retrying sweep, so the delta is always
        // consumed there and this never has anything to report — that mode's log is unchanged.
        let mut rolled = ROLLED_TOTAL.lock();
        let mut rolled_ms = ROLLED_MS.lock();
        if fire {
            *rolled = rtotal;
            *rolled_ms = now;
        } else {
            if *rolled_ms == 0 {
                *rolled_ms = now;
            }
            let unseen = rtotal.saturating_sub(*rolled);
            if unseen > 0 && now.wrapping_sub(*rolled_ms) >= RETRY_ROLLUP_MS {
                let window = now.wrapping_sub(*rolled_ms);
                *rolled = rtotal;
                *rolled_ms = now;
                // STEP0-STALL CENSUS rides here rather than on its own line: a step-0 stall is a
                // command byte the SMC never acknowledged, and after the s73 wedge the open
                // question is its RATE, not its existence. `stalls=` climbing across rollups means
                // standing; one early stall and a flat count thereafter means transient — the
                // distinction the one-shot DIAG structurally cannot draw.
                serial_println!(
                    ":: SMC-BATT: retry rollup — {} retries in the last {} ms (total {}, stall0 {}, resid {}) == rollup ::",
                    unseen,
                    window,
                    rtotal,
                    stall0,
                    resid
                );
            }
        }

        // PWR ROLLUP (M1 power instrument)
        let mut pwr = PWR_ROLLUP.lock();
        let mut flush = false;
        let mut reason = "";
        let dropped_accumulator = !s.present && pwr.start_ms != 0 && (pwr.samples > 0 || pwr.unknown > 0);

        if s.present {
            if pwr.start_ms == 0 {
                pwr.start_ms = now;
                pwr.state = s.ac_derived;
            }

            if s.ac_derived != pwr.state && pwr.start_ms != 0 && (pwr.samples > 0 || pwr.unknown > 0) {
                flush = true;
                reason = "state change";
            } else if now.wrapping_sub(pwr.start_ms) >= PWR_ROLLUP_MS && (pwr.samples > 0 || pwr.unknown > 0) {
                flush = true;
                reason = "rollup";
            }
        } else if dropped_accumulator {
            flush = true;
            reason = "presence dropped";
        }

        if flush {
            let window_ms = now.wrapping_sub(pwr.start_ms);

            let state_str = match pwr.state {
                AcDerived::Charging => "plugged (charging)",
                AcDerived::Discharging => "unplugged (discharging)",
                AcDerived::Idle => "idle",
                AcDerived::Unknown => "unknown",
            };

            // ORDERING IS LOAD-BEARING — `(dropped)` MUST be tested first.
            //
            // These two arms partition the `samples == 0` space, and they partition it by WHY the
            // window ended, not by what was in it. Reaching this block at all requires
            // `samples > 0 || unknown > 0` (every one of the three `flush` conditions above demands
            // it), so `samples == 0` here implies `unknown > 0` — which means the `(unknown
            // dominated)` test is true for EVERY sample-less window, including the dropped ones.
            //
            // With the tests in the other order, `(dropped)` was unreachable: it required
            // `samples == 0` AND falling through `samples == 0 && unknown > 0`, i.e. `unknown == 0`,
            // while `dropped_accumulator` itself demands `samples > 0 || unknown > 0`. Mutually
            // exclusive. The branch could never print, so a presence drop was reported as ordinary
            // unknown-domination and the SMC dying mid-window looked like a quiet sensor.
            //
            // Nothing is lost by preferring `(dropped)`: `unknown=` is on both lines either way.
            if dropped_accumulator && pwr.samples == 0 {
                serial_println!(
                    ":: PWR: NO-WINDOW (dropped) window_ms={} state={} samples={} unknown={} == {} ::",
                    window_ms, state_str, pwr.samples, pwr.unknown, reason
                );
            } else if pwr.samples == 0 && pwr.unknown > 0 {
                serial_println!(
                    ":: PWR: NO-WINDOW (unknown dominated) window_ms={} state={} samples={} unknown={} == {} ::",
                    window_ms, state_str, pwr.samples, pwr.unknown, reason
                );
            } else if pwr.min < 0 && pwr.max > 0 {
                serial_println!(
                    ":: PWR: INADMISSIBLE (straddles zero, excluded from cumulative) window_ms={} state={} samples={} unknown={} sum={} min={} max={} == {} ::",
                    window_ms, state_str, pwr.samples, pwr.unknown, pwr.sum, pwr.min, pwr.max, reason
                );
            } else {
                pwr.total_ms += window_ms;
                pwr.total_sum += pwr.sum;
                pwr.total_samples += pwr.samples;

                serial_println!(
                    ":: PWR: window_ms={} state={} samples={} unknown={} sum={} min={} max={} (total: time={} sum={} samples={}) (ac_derived: inferred, no hardware key) == {} ::",
                    window_ms, state_str, pwr.samples, pwr.unknown, pwr.sum, pwr.min, pwr.max, pwr.total_ms, pwr.total_sum, pwr.total_samples, reason
                );
            }

            // Reset accumulator for the new window
            pwr.samples = 0;
            pwr.unknown = 0;
            pwr.sum = 0;
            pwr.min = 0;
            pwr.max = 0;
            if s.present {
                pwr.start_ms = now;
                pwr.state = s.ac_derived;
            } else {
                pwr.start_ms = 0;
                pwr.state = AcDerived::Unknown;
            }
        }

        if s.present {
            // Accumulate current sample
            if let (Some(volt), Some(amp)) = (s.volt_mv, s.amp_ma) {
                if matches!(s.ac_derived, AcDerived::Unknown) {
                    pwr.unknown += 1;
                } else {
                    let mw = (volt as i64 * amp as i64) / 1000;
                    if pwr.samples == 0 {
                        pwr.min = mw;
                        pwr.max = mw;
                    } else {
                        if mw < pwr.min { pwr.min = mw; }
                        if mw > pwr.max { pwr.max = mw; }
                    }
                    pwr.sum += mw;
                    pwr.samples += 1;
                }
            } else {
                // If either volt or amp is missing, it's a partial sweep and is unknown.
                pwr.unknown += 1;
            }
        }
    }

    /// The last cached snapshot plus its age in ms (0 when fresh-ish or never-good), for the vug
    /// meter hook (cheap; no port I/O). The age is `now - GOOD_MS` when the cache holds a good
    /// reading — the widget shows a staleness note once it grows past a few refresh periods
    /// (BATMON-HOLD), instead of going dark.
    pub fn cached() -> (BatterySnapshot, u64) {
        let s = *CACHE.lock();
        let good = *GOOD_MS.lock();
        let age = if s.present && good != 0 { crate::arch::ms().wrapping_sub(good) } else { 0 };
        (s, age)
    }
}
