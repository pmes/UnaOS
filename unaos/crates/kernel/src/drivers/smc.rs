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
    /// The payload names the step (0 = command, 1 = key arg, 2 = length/lookup, 3 = data byte).
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
/// reaches the DIAG, because unclaimed ports read `0xff`, so `REV `'s step-0 `NEW_CMD|ACK` wait
/// times out to `Stuck(0)` and dumps a flat-`ff` timeline. `SmcError::Absent` never was that path —
/// it requires the status low nibble to read exactly `0x00`, which a wedge cannot produce.
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
fn settle_before_command() {
    let start = crate::arch::now_cycles();
    while read_status() & (ST_DATA_READY | ST_BUSY) != 0 {
        if read_status() & ST_DATA_READY != 0 {
            let _ = read_data(); // drain a stale value byte to unstick the previous transaction
        }
        if crate::arch::now_cycles().wrapping_sub(start) >= SMC_WAIT_CYCLES {
            break; // bounded — never spin forever; step-0's NEW_CMD|ACK wait still guards
        }
        poll_pause();
    }
}

/// Read the value of a 4-character key into `out`, returning the number of bytes read (up to
/// `out.len()`). The classic Apple SMC READ handshake: command 0x10, four key bytes, one length
/// byte, then value bytes while `DATA_READY` holds.
pub fn read_key(key: &[u8; 4], out: &mut [u8]) -> Result<usize, SmcError> {
    let r = read_key_inner(key, out);
    // SMC-DIAG dispatch (KEY-SHAPE, see above). The boot's FIRST genuinely failing key read — from
    // whatever path (scout, battery sweep, enumeration) — dumps the raw status timeline, once. A
    // clean `Absent` for a key that has never answered is a *learned fact about this machine's key
    // set*, not a failure, and must not consume that one shot.
    match r {
        Ok(_) => note_shape(key, SHAPE_PRESENT),
        Err(SmcError::Absent) => {
            if shape_of(key) == SHAPE_PRESENT {
                // It answered earlier this boot (or is the protocol-required `REV `) and now does
                // not. A key cannot stop existing — this is a real fault worth the dump.
                dump_first_failure(key, SmcError::Absent);
            } else {
                note_shape(key, SHAPE_ABSENT);
            }
        }
        Err(e @ SmcError::Stuck(_)) => dump_first_failure(key, e),
    }
    r
}

fn read_key_inner(key: &[u8; 4], out: &mut [u8]) -> Result<usize, SmcError> {
    // 0) settle any residue from a prior (interrupted) transaction so step-0 starts clean (M2
    //    idle-guard). No-op on an idle SMC — byte-identical on QEMU.
    settle_before_command();

    // 1) command byte -> expect NEW_CMD|ACK.
    write_cmd(CMD_READ);
    wait_status(ST_AFTER_CMD, 0)?;

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
    serial_println!(
        ":: SMC-DIAG: FIRST FAILURE key {} kind {} step {} t={}ms — raw status timeline [{}] (16 reads, ~15us apart) == evidence ::",
        name,
        kind,
        step,
        crate::arch::ms(),
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
fn read_key_by_index(index: u32, name: &mut [u8; 4]) -> Result<(), SmcError> {
    // 0) settle any residue from a prior transaction (M2 idle-guard; no-op on an idle SMC).
    settle_before_command();

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
                // Absence at scout time is the INVENTORY RESULT, not a fault: the SMC looked the
                // key up and answered "no such key". Said plainly on the line, because this is the
                // reading that used to arrive dressed as `SMC-DIAG: FIRST FAILURE … == evidence`.
                serial_println!(
                    ":: SMC-SCOUT: key {} absent — this SMC does not carry it (clean negative answer, not a fault) ({}) ::",
                    name, desc
                );
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
                    Err(_) => {
                        serial_println!(
                            ":: SMC-SCOUT: index enumeration stopped at idx {} (GET_KEY_BY_INDEX unsupported or stuck) ::",
                            idx
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

    /// AC-W PROBE-ONCE re-probe period. A key set is fixed by the SMC's firmware, so a learned
    /// absence cannot go stale in practice — this exists so the learning stays **falsifiable**
    /// rather than becoming an article of faith. One transaction a minute instead of one a second.
    const ACW_REPROBE_MS: u64 = 60_000;
    /// When `AC-W` was last actually probed (0 = never; the boot scout's probe is separate).
    static ACW_LAST_PROBE_MS: Mutex<u64> = Mutex::new(0);
    /// Whether the "AC presence is unknown on this machine" statement has been made (once a boot).
    static ACW_NOTED: Mutex<bool> = Mutex::new(false);

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
    /// SWEEP-ABORT (metal perf fix, 2026-07-18): if the FIRST key's handshake comes back Stuck —
    /// the SMC is not answering at all, not merely lacking that key — the remaining keys are
    /// skipped (they would each burn the same bounded multi-attempt timeout). On the sitting-1
    /// metal GUI builds (SMC unresponsive every boot) an un-aborted sweep cost up to ~16 stuck
    /// handshakes x the 0.1 s budget on the vug meter cadence — the "cursor really slows vug down"
    /// stall was THIS, not the sprite. A clean `Absent` (QEMU) still sweeps every key, unchanged.
    pub fn snapshot() -> BatterySnapshot {
        let mut s = BatterySnapshot::default();
        RETRIES_SWEEP.store(0, Ordering::Relaxed); // per-sweep retry counter (IVY-RETRY)
        
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
        s.ac_present = 'acw: {
            // AC-W PROBE-ONCE (GR17). On a machine whose SMC has no `AC-W` — the 2012 rMBP, learned
            // by the boot scout and re-learned by the first sweep — re-running the full transaction
            // every second cannot change the answer; it just spends an SMC transaction per second
            // rediscovering a fixed property of the firmware's key set. Once the key is known
            // absent, the read is skipped and `ac_present` stays the honest `None`.
            //
            // NOT a cached claim: the learning is re-tested every `ACW_REPROBE_MS`, so if this ever
            // ran on a machine that does carry `AC-W` — or if a clean `Absent` were ever produced by
            // something other than a real lookup miss — the skip self-corrects within a minute and
            // `ac_present` resolves to the direct answer. Falsifiable, not assumed.
            if super::shape_of(b"AC-W") == super::SHAPE_ABSENT {
                let now = crate::arch::ms();
                let mut last = ACW_LAST_PROBE_MS.lock();
                if *last != 0 && now.wrapping_sub(*last) < ACW_REPROBE_MS {
                    break 'acw None; // known absent, not due for re-probe: ac_present stays unknown
                }
                *last = now;
                // Say it ONCE, plainly, in place of the every-boot `FIRST FAILURE` line this
                // replaces: the machine has no AC-W, so AC presence is genuinely UNKNOWN and the
                // `ac=derived:…` field is an inference from the B0AC sign — never a measurement.
                let mut noted = ACW_NOTED.lock();
                if !*noted {
                    *noted = true;
                    serial_println!(
                        ":: SMC-BATT: AC-W is absent on this SMC (clean negative answer, not a fault) — AC presence is UNKNOWN; ac=derived:* is inferred from the B0AC sign, and the key is re-probed every {} ms == witness ::",
                        ACW_REPROBE_MS
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
            s.ac_stuck = true; // budget exhausted on a non-Absent failure: wedged, not absent
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
    /// Concurrency: the SMC key/value transaction is not internally serialized. Every caller on the
    /// x86 rMBP path runs on the BSP (boot `pci::init`, the main-loop poll, the vug cadence — all
    /// single-threaded), so the throttle-then-transact sequence never interleaves in practice. If a
    /// future path ever drives the SMC from multiple cores, wrap the transaction in a lock (a
    /// garbled read is bounded — `Stuck` — never a hang, so this is a correctness-of-reading concern,
    /// not a safety one).
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
                ":: SMC-BATT: present={} soc={}% volt={}mV amp={}mA full={}mAh rem={}mAh ac={} retries={}/{} == witness ::",
                s.present,
                fu(s.soc_pct),
                fu(s.volt_mv),
                fi(s.amp_ma),
                fu(s.full_mah),
                fu(s.rem_mah),
                ac,
                rsweep,
                rtotal,
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
                serial_println!(
                    ":: SMC-BATT: retry rollup — {} retries in the last {} ms (total {}) == rollup ::",
                    unseen,
                    window,
                    rtotal
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
