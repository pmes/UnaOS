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
        core::hint::spin_loop();
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
        core::hint::spin_loop();
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
        core::hint::spin_loop();
    }
}

/// Read the value of a 4-character key into `out`, returning the number of bytes read (up to
/// `out.len()`). The classic Apple SMC READ handshake: command 0x10, four key bytes, one length
/// byte, then value bytes while `DATA_READY` holds.
pub fn read_key(key: &[u8; 4], out: &mut [u8]) -> Result<usize, SmcError> {
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
        core::hint::spin_loop();
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
        core::hint::spin_loop();
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
                serial_println!(":: SMC-SCOUT: key {} absent ({}) ::", name, desc);
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
    use spin::Mutex;

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
        /// AC adapter present (`AC-W` answered with a non-zero payload).
        pub ac_present: Option<bool>,
    }

    static CACHE: Mutex<BatterySnapshot> = Mutex::new(BatterySnapshot {
        present: false,
        soc_pct: None,
        volt_mv: None,
        amp_ma: None,
        full_mah: None,
        rem_mah: None,
        ac_present: None,
    });
    /// Last refresh time (ms); 0 = never. Throttles the port I/O off the per-frame path.
    static LAST_MS: Mutex<u64> = Mutex::new(0);
    /// Whether the boot witness line has fired (once, when the first real reading lands).
    static WITNESSED: Mutex<bool> = Mutex::new(false);

    const REFRESH_MS: u64 = 1000;

    fn read_u16(key: &[u8; 4]) -> Option<u16> {
        let mut b = [0u8; 2];
        match read_key(key, &mut b) {
            Ok(2) => Some(((b[0] as u16) << 8) | b[1] as u16),
            _ => None,
        }
    }

    fn read_i16(key: &[u8; 4]) -> Option<i16> {
        read_u16(key).map(|u| u as i16)
    }

    /// Read all battery keys and return a fresh snapshot (unthrottled). Pure reads.
    pub fn snapshot() -> BatterySnapshot {
        let mut s = BatterySnapshot::default();
        s.soc_pct = read_u16(b"BRSC");
        s.volt_mv = read_u16(b"B0AV");
        s.amp_ma = read_i16(b"B0AC");
        s.full_mah = read_u16(b"B0FC");
        s.rem_mah = read_u16(b"B0RM");
        // AC presence: any non-zero AC-W payload => adapter attached.
        let mut acw = [0u8; 2];
        s.ac_present = match read_key(b"AC-W", &mut acw) {
            Ok(n) if n >= 1 => Some(acw[..n].iter().any(|&x| x != 0)),
            Err(SmcError::Absent) => None,
            _ => None,
        };
        s.present = s.soc_pct.is_some() || s.volt_mv.is_some() || s.rem_mah.is_some();
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
    pub fn refresh_if_due() {
        let now = crate::arch::ms();
        {
            let mut last = LAST_MS.lock();
            if *last != 0 && now.wrapping_sub(*last) < REFRESH_MS {
                return;
            }
            *last = now;
        }
        let s = snapshot();
        *CACHE.lock() = s;

        // Fire the witness on the FIRST refresh (proves the M2 read path ran — the honest
        // `present=false` line on QEMU / a battery-less machine), then on the ~1 s cadence whenever
        // a battery is present so a sitting can watch discharge/charge track. Throttled above.
        let mut w = WITNESSED.lock();
        if !*w || s.present {
            *w = true;
            // Absent keys print the "-" sentinel — never a number a reader could mistake for a real
            // value (0 mA is a plausible amperage, so `None` must NOT read as 0). snapshot() itself
            // keeps the honest `None`; only this human-facing witness applies the sentinel.
            let fu = |o: Option<u16>| o.map(|v| alloc::format!("{}", v)).unwrap_or_else(|| "-".into());
            let fi = |o: Option<i16>| o.map(|v| alloc::format!("{}", v)).unwrap_or_else(|| "-".into());
            serial_println!(
                ":: SMC-BATT: present={} soc={}% volt={}mV amp={}mA full={}mAh rem={}mAh ac={} == witness ::",
                s.present,
                fu(s.soc_pct),
                fu(s.volt_mv),
                fi(s.amp_ma),
                fu(s.full_mah),
                fu(s.rem_mah),
                match s.ac_present { Some(true) => "yes", Some(false) => "no", None => "?" },
            );
        }
    }

    /// The last cached snapshot, for the vug meter hook (cheap; no port I/O).
    pub fn cached() -> BatterySnapshot {
        *CACHE.lock()
    }
}
