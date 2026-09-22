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

//! FTDI FT232 USB-serial console (U2.5).
//!
//! The 2012 rMBP has no 16550, so every metal verdict so far has been a photo of the framebuffer.
//! A USB-serial console retires that loop. QEMU's `-device usb-serial` presents an FTDI FT232
//! (VID 0x0403, PID 0x6001, one vendor-specific interface, bulk IN 0x81 + bulk OUT 0x02, MPS 64,
//! full-speed); Peter's physical FTDI cable behaves the same, so the driver lands QEMU-green now
//! and metal-verifies on cable day.
//!
//! This module is deliberately ARCH-NEUTRAL (it compiles under both the x86_64 and aarch64 kernel
//! targets, since `drivers::xhci` is not arch-gated): it holds only the FT232 protocol constants
//! and the boot-capture TX ring. The x86-only `_print` hook (arch/x86_64/serial.rs) is what feeds
//! [`mirror`]; the xHCI enumeration + bulk-OUT drain live in `drivers::xhci::mod`.
//!
//! SCOPE (this arc): TX only. FTDI bulk-OUT takes RAW bytes with NO header — we push the console
//! bytes straight out. FTDI bulk-IN (RX) prepends TWO modem-status bytes to every packet; stripping
//! those to give the kernel a real input console is a STUB deferred to a future arc. Enumeration is
//! ROOT-PORT only this arc (the hub-downstream walk is HID-only); FTDI behind a hub is a future arc.

use core::fmt;
use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use spin::Mutex;

/// FT232 identity as emulated by QEMU `usb-serial` (and the real FT232R cable).
pub const FTDI_VID: u16 = 0x0403;
pub const FTDI_PID: u16 = 0x6001;

// FTDI vendor control requests — all bmRequestType 0x40 (host-to-device | vendor | device),
// wIndex 0 (port A), no data stage. (Linux `drivers/usb/serial/ftdi_sio.h`.)
/// Reset the FT232's SIO engine. wValue 0 = reset both the RX and TX buffers.
pub const FTDI_SIO_RESET: u8 = 0x00;
/// Set hardware/software flow control. wValue 0 = no flow control.
pub const FTDI_SIO_SET_FLOW_CTRL: u8 = 0x02;
/// Set the baud-rate divisor (see [`FTDI_BAUD_115200`]).
pub const FTDI_SIO_SET_BAUDRATE: u8 = 0x03;
/// Set frame format (data bits / parity / stop bits) — see [`FTDI_DATA_8N1`].
pub const FTDI_SIO_SET_DATA: u8 = 0x04;

/// Baud divisor wValue for 115200 baud. The FT232 baud generator runs off a 3 MHz reference
/// (48 MHz / 16); the divisor is `3_000_000 / 115_200 = 26.04`. The closest encodable divisor is
/// 26 with fractional bits 15:14 = 0, i.e. wValue 0x001A → 115_385 baud (+0.16% error, well inside
/// a UART's tolerance). QEMU's model accepts the request and ignores baud for a file chardev; the
/// real cable honours it — metal truth on cable day.
pub const FTDI_BAUD_115200: u16 = 0x001A;

/// Frame-format wValue for 8 data bits, no parity, 1 stop bit (bits 10:8 parity = 0, bits 13:11
/// stop = 0, bits 7:0 data = 8).
pub const FTDI_DATA_8N1: u16 = 0x0008;

/// Boot-capture ring capacity, so that when the console comes up mid-boot the entire early log
/// replays out the cable.
///
/// **256 KiB, measured — not guessed.** The 64 KiB this started at stopped being enough, and the
/// bench captures say so with an exact number: `drain_ftdi`'s
/// `:: U2.5: FTDI TX mirror -> PASS (N boot bytes replayed) ::` pegged at 65731 — i.e. `CAP` + 195 —
/// on five of the six boots in `capture/rmbp-gr15-s70` and `capture/rmbp-s66-cand444`. A replay total
/// that EXCEEDS the ring can only mean the ring was already full when the console came up, i.e.
/// drop-oldest had eaten the head of the boot. The one boot that came in under (57598) is also the
/// only one whose capture still begins at `:: x86 fb-wc: retyped … ::`; every saturated boot begins
/// mid-line, tens of KiB later, with both `fb-wc` (the framebuffer's memory type — the line a panel
/// regression would have been convicted by) and `:: X86_64 Memory Init ::` gone.
///
/// The console does not come up until ~28.9 s (`ftdi:console-up`), so 4x is headroom against the
/// pre-console log growing further, not against the present measurement.
///
/// **1 MiB since PHASE31WIT (rmbp B112/B114) — 256 KiB was not enough either, and the paragraph
/// above predicted the exact shape of the failure.** It said a saturated ring begins "mid-line, tens
/// of KiB later, with both `fb-wc` … and `:: X86_64 Memory Init ::` gone". That is precisely what
/// the 2026-09-16 flight-8/flight-9 capture shows at 256 KiB: replay spans of 260 958 bytes
/// (flight 8) and 258 584 bytes (flight 9) against a 262 144-byte `CAP` — the ring pinned at
/// capacity on BOTH boots independently, each replay starting mid-token. What it threw away was what
/// a whole flight had been staked on: `:: x86 bar1exp: UC arm ARMED` (the UC experiment's only
/// witness), the baseline's own `:: x86 fb-wc: retyped`, and the unconditional `:: video: WRITER
/// seeded` — all three at ZERO hits across 11 265 captured lines.
///
/// 1 MiB is ~4x the measured pre-console volume: the margin becomes ~786 KiB (~4.0x) where 256 KiB
/// delivered ~1 KiB (1.004x) — i.e. it was not a margin at all. The accounting below is unchanged in
/// kind, only in magnitude, and the replay-time argument still holds: the ring replays what the boot
/// actually printed, so no boot that was not already losing bytes drains any longer than it does
/// today. A boot that genuinely filled 1 MiB would spend ~91 s draining at 115200 baud — but such a
/// boot is, at 256 KiB, one that silently discards three quarters of itself, and between
/// [`emit_verdict`] and the late `:: FTDI-CAP:` line that state is now loud on both channels rather
/// than inferred from a line nobody was looking for.
///
/// Cost: 1 MiB of `.bss`. It is a zero-initialised `static`, so it adds nothing to either kernel
/// image (`.bss (NOLOAD)` in `pi-baremetal.ld`, NOBITS in the x86 ELF) and nothing to the allocator,
/// which does not exist yet when the first `_print` reaches [`mirror`].
///
/// It does NOT cost 4x the replay time. The ring only ever holds what the boot actually printed, so
/// the replay is (real pre-console volume) / 11.5 KB/s at 115200 baud; raising `CAP` lengthens it only
/// by the bytes currently being thrown away. The worst case is bounded and worth stating plainly: a
/// boot that genuinely fills all 256 KiB spends ~23 s draining it out the cable (against ~5.7 s for a
/// full 64 KiB today), and `drain_ftdi` owns the main loop for that stretch. Per-transfer risk is
/// unchanged — more 512-byte bulk-OUTs, each with the same budget the bench measures at
/// `:: FTDI: tx pump budget=5387698040 used=91103400 … result=OK ::`, ~59x headroom, and no capture in
/// the tree has ever logged `FTDI TX disabled`.
const CAP: usize = 1024 * 1024;

/// A fixed, heap-free circular byte buffer. The very first `_print`s predate the allocator, so the
/// ring must be a `static` with an inline array — never `Vec`. Drop-oldest on overflow.
struct Ring {
    buf: [u8; CAP],
    /// Index of the oldest buffered byte.
    head: usize,
    /// Number of valid bytes currently buffered (0..=CAP).
    len: usize,
    /// Count of bytes dropped on overflow (oldest-first), for diagnostics.
    dropped: u64,
}

impl Ring {
    const fn new() -> Self {
        Ring { buf: [0u8; CAP], head: 0, len: 0, dropped: 0 }
    }

    /// Append one byte, dropping the oldest (and counting it) when the ring is full.
    fn push_byte(&mut self, b: u8) {
        if self.len == CAP {
            self.head = (self.head + 1) % CAP;
            self.len -= 1;
            self.dropped = self.dropped.wrapping_add(1);
        }
        let tail = (self.head + self.len) % CAP;
        self.buf[tail] = b;
        self.len += 1;
    }
}

/// `fmt::Write` so a `_print`'s `Arguments` can be formatted straight into the ring with no heap and
/// no intermediate buffer (each `write_str` fragment is pushed byte-by-byte under the same lock).
impl fmt::Write for Ring {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for &b in s.as_bytes() {
            self.push_byte(b);
        }
        Ok(())
    }
}

static RING: Mutex<Ring> = Mutex::new(Ring::new());

/// True once the FTDI console is fully brought up and TX may drain. Cleared permanently if the
/// bulk-OUT drain ever times out or errors — the kernel must never wedge on console TX.
static LIVE: AtomicBool = AtomicBool::new(false);

/// SERWIT-2 staging ring for the FTDI mirror. See [`mirror`].
///
/// 64 slots × 240 bytes ≈ 15 KiB of `.bss`, sized to the same worst burst as the primary wire's ring:
/// every other core can queue several whole lines while one core holds `RING` for a memcpy and still
/// not reach the end. It is a `LineRing` and not a second byte ring on purpose — deferral must preserve
/// WHOLE LINES, or a contended capture reads as interleaved garbage rather than as an ordered log.
static STAGE: crate::serial_ring::LineRing<64, 240> = crate::serial_ring::LineRing::new();

/// Losses not yet announced IN THE CAPTURE STREAM. Deliberately separate from the tap ledger's own
/// pending counter: the wire's announcement (`serial_ring::mirror_service`) and the cable's must not
/// consume each other's news — a bench sitting reads the cable and never sees the wire, and a log
/// reader sees the wire and never sees the cable.
static UNANNOUNCED: AtomicU32 = AtomicU32::new(0);

/// CAPWIT — whether the capture still owes the wire its opening integrity verdict. Spent on the first
/// drain after the console goes live, which is the first moment anything can be said out loud at all.
static VERDICT_DUE: AtomicBool = AtomicBool::new(true);

/// CAPWIT — ring-overflow BYTES already announced on the cable.
///
/// Deliberately distinct from [`UNANNOUNCED`], which counts whole LINES lost to lock contention. These
/// are different failures with different fixes: contention loses a line while the ring has room,
/// overflow loses the head of the boot because the ring ran out of it. Folding them together is how a
/// full ring would get to hide behind a quiet mutex.
static ANNOUNCED_DROPS: AtomicU64 = AtomicU64::new(0);

/// PHASE31WIT — bytes [`drain_into`] has handed to the cable since boot, i.e. the size of the replay
/// a bench operator actually received. Compared against [`CAP`] it is the saturation test the
/// flight-8/flight-9 capture had to be reconstructed by hand to perform.
static REPLAYED: AtomicU64 = AtomicU64::new(0);

/// PHASE31WIT — spent by the first late `:: FTDI-CAP:` line, so the verdict is exactly one line.
static LATE_VERDICT_DUE: AtomicBool = AtomicBool::new(true);

/// Lines staged for the cable but not yet copied into the capture ring.
pub fn staged_in_flight() -> u64 {
    STAGE.in_flight()
}

/// Bytes the capture ring has evicted (oldest-first) because it was full — i.e. how much of the boot
/// log the cable can no longer replay.
///
/// `None` means the ring was busy and the count could not be read. That is NOT the same as zero and is
/// not reported as zero: a drop counter whose contended path reads as "nothing was lost" is an
/// instrument that cannot falsify the thing it exists to catch.
pub fn dropped_bytes() -> Option<u64> {
    RING.try_lock().map(|r| r.dropped)
}

/// Copy the most recently buffered bytes into `dst` WITHOUT consuming them, newest-aligned to the
/// end of the ring: on success `dst[..n]` holds the last `n` bytes the capture received, in order.
///
/// `None` means the ring was busy — not "the capture is empty". The two readings are different facts
/// and a caller that folded them together would report a contended lock as an absent log.
///
/// **`try_lock` only, no allocation, no mutation.** This is a read path onto the sink that the
/// primary `_print` also feeds, so it must obey `mirror`'s discipline exactly: blocking here would let
/// a reader stall the wire, and consuming here would eat bytes the cable still owes the bench.
/// Deliberately does NOT fold [`STAGE`] in — a staged line has not reached the ring yet, and a peek
/// that quietly drained staging would change what the next `drain_into` replays.
pub fn peek_recent(dst: &mut [u8]) -> Option<usize> {
    let ring = RING.try_lock()?;
    let n = dst.len().min(ring.len);
    let start = (ring.head + (ring.len - n)) % CAP;
    for (i, slot) in dst[..n].iter_mut().enumerate() {
        *slot = ring.buf[(start + i) % CAP];
    }
    Some(n)
}

/// Append a formatted `_print` to the boot-capture ring.
///
/// **try_lock only on the sink, never blocks** — the discipline of the SERIAL1 `_print` path, and
/// non-negotiable: this runs OUTSIDE `SERIAL1` and OUTSIDE the interrupt mask, so every core reaches
/// it at once, and a mirror that could block here would be able to stall the primary wire. That
/// inversion would be worse than any drop. The bounded turn below holds NOTHING while it waits and
/// re-tries the sink on every turn, so it is back-pressure and not a block.
///
/// **SERWIT-2 — what changed.** The failure branch used to be nothing: on contention the whole
/// formatted line evaporated, with no counter anywhere. THIS IS THE BENCH'S OWN CAPTURE PATH — the
/// 2012 rMBP has no 16550, so on an attended metal sitting the FTDI cable is not a mirror of the
/// evidence, it IS the evidence — and it lost lines precisely when the machine was busiest.
///
/// **SERWIT-2 BACKPRESSURE (FIXTURE_FLAKES §2a, rmbp-ledger B159).** One free retry was not enough:
/// `staged` hit this ring's own 64 slots exactly on the FAILs (`dropped=7`, `dropped=13`) against
/// `staged=10..30 dropped=0` on the greens — DEPTH EXHAUSTION under contention. Now: take the sink if
/// free (draining anyone else's staged lines first, so deferral never reorders the capture); else defer
/// the whole line into [`STAGE`]; else GO ROUND under the primary wire's own contended-producer policy
/// ([`crate::serial_ring::defer_policy`], rows compiler-checked). Only when the BOUND expires is a line
/// lost, and a lost line is COUNTED and announced IN THE CAPTURE STREAM (see [`drain_staged_into`]).
pub fn mirror(args: fmt::Arguments) {
    let tap = &crate::serial_ring::TAP_FTDI;
    tap.submit();
    // SERWIT-2 BACKPRESSURE. The bound is the PRIMARY WIRE'S OWN, so this transport has one checkable magnitude and not a second one nobody can check (see `BACKPRESSURE_SPINS`). In panic mode it collapses to 1 — which IS the old single free retry, turn for turn — because a dying machine must not spend a bounded wait per line on a holder that may never release.
    let bound = if crate::serial_ring::in_panic_mode() { 1 } else { crate::serial_ring::BACKPRESSURE_SPINS };
    let mut spins: u32 = 0;
    loop {
        if let Some(mut ring) = RING.try_lock() {
            drain_staged_into(&mut ring);
            let _ = ring_write(&mut ring, args);
            tap.absorb();
            return;
        }
        match STAGE.stage(args) {
            crate::serial_ring::Staged::Whole => { tap.note_staged(); return; }
            crate::serial_ring::Staged::Truncated => {
                // Sealed with a visible marker in the capture; counted and announced like a loss.
                tap.note_staged();
                tap.tear();
                UNANNOUNCED.fetch_add(1, Ordering::Relaxed);
                return;
            }
            crate::serial_ring::Staged::Full => {}
        }
        // Sink busy AND staging full — the one branch that was still lossy. Go round: each turn re-tries the SINK first (winning it drains STAGE and writes this line intact), so the wait bears progress, and room can also arrive from ANOTHER core's drain. Nothing is held across the turn.
        match crate::serial_ring::defer_policy(false, spins, bound) {
            crate::serial_ring::Defer::Retry => { spins += 1; core::hint::spin_loop(); }
            _ => break,
        }
    }
    tap.drop_line();
    UNANNOUNCED.fetch_add(1, Ordering::Relaxed);
}

/// This sink's line-start flag — mutated only while `RING` is held (by the prefixing writer and by
/// [`drain_staged_into`]'s bare-byte correction).
#[cfg(feature = "logts")]
static LINE_START: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// CLOCK-2b: the direct (lock-held) write into the capture ring, timestamp-prefixed under `logts`.
/// This ring IS the bench's evidence on a machine with no 16550, and the UART-side prefix never
/// reaches it. Staged lines are folded in bare by [`drain_staged_into`] on purpose — a prefix
/// rendered at drain time would stamp the drain, not the emission. Under `logts` the ring's
/// `dropped` byte count includes prefix bytes that were never on the UART wire.
fn ring_write(ring: &mut Ring, args: fmt::Arguments) -> fmt::Result {
    #[cfg(feature = "logts")]
    {
        use core::fmt::Write;
        crate::logts::TapPrefixWriter { inner: ring, state: &LINE_START }.write_fmt(args)
    }
    #[cfg(not(feature = "logts"))]
    fmt::write(ring, args)
}

/// Copy every staged line into the capture ring, in order, and put any un-announced loss into the
/// stream as a real line.
///
/// **Caller must hold `RING`** — that is what makes the drainer unique. No print, no allocation and no
/// second lock happens in here, so it is safe from the same IRQ-masked print context `mirror` is.
fn drain_staged_into(ring: &mut Ring) {
    // CLOCK-2b: the drainer writes ring bytes WITHOUT going through the prefixing writer (staged
    // lines are deliberately bare — a drain-time prefix would stamp the drain, not the emission),
    // so it must keep the sink's line-start flag true to where the ring actually is. A staged
    // `serial_print!` FRAGMENT (no trailing `\n`) would otherwise leave the flag claiming
    // line-start and the next prefix would land mid-line, stamping text with a time it was not
    // emitted at.
    #[cfg(feature = "logts")]
    let mut last_byte: Option<u8> = None;
    let n = STAGE.drain(|s| {
        for &b in s.as_bytes() {
            ring.push_byte(b);
        }
        #[cfg(feature = "logts")]
        {
            last_byte = s.as_bytes().last().copied().or(last_byte);
        }
    });
    crate::serial_ring::TAP_FTDI.absorb_n(n);
    // The cable's own channel. A capture that is short must say so on the capture, not only in a
    // counter the reader of the capture will never see.
    let pending = UNANNOUNCED.swap(0, Ordering::Relaxed);
    if pending > 0 {
        let mut buf = [0u8; 96];
        let mut w = crate::serial_ring::BoundedWriter {
            buf: buf.as_mut_ptr(),
            cap: buf.len(),
            n: 0,
            truncated: false,
        };
        let _ = fmt::Write::write_fmt(
            &mut w,
            format_args!("\n[ftdi] {} console line(s) lost to contention\n", pending),
        );
        let len = w.n;
        for &b in &buf[..len] {
            ring.push_byte(b);
        }
        #[cfg(feature = "logts")]
        {
            last_byte = buf[..len].last().copied().or(last_byte);
        }
    }
    #[cfg(feature = "logts")]
    if let Some(b) = last_byte {
        LINE_START.store(b == b'\n', core::sync::atomic::Ordering::Relaxed);
    }
}

/// CAPWIT — write the capture's own integrity verdict into the cable's TX staging buffer.
///
/// Returns `Some(len)` when a verdict line was written to `dst` (the caller pushes exactly those bytes
/// and calls again), `None` when nothing is owed or it would not fit.
///
/// **The line goes straight into `dst` and never into the ring.** That is not a stylistic choice. This
/// runs with `RING` held, so a `serial_println!` here would re-enter [`mirror`] and, on the panic path,
/// `_print`'s other sinks — and worse, a verdict pushed into a FULL ring would evict the very early
/// bytes it is reporting on, so the act of reporting the loss would enlarge it. Writing into the DMA
/// staging buffer costs the ring nothing and puts the verdict AHEAD of the replay it describes.
///
/// No state is consumed unless the line is actually delivered, so a caller with a short `max` simply
/// retries on the next drain rather than silently burning the one announcement.
///
/// # Safety
/// `dst` must point to a writable region of at least `max` bytes.
unsafe fn emit_verdict(ring: &Ring, dst: *mut u8, max: usize) -> Option<usize> {
    let dropped = ring.dropped;
    let first = VERDICT_DUE.load(Ordering::Relaxed);
    let announced = ANNOUNCED_DROPS.load(Ordering::Relaxed);
    if !first && dropped <= announced {
        return None;
    }

    let mut buf = [0u8; 192];
    let mut w = crate::serial_ring::BoundedWriter {
        buf: buf.as_mut_ptr(),
        cap: buf.len(),
        n: 0,
        truncated: false,
    };
    // Three readings, and the instrument can produce all three. A witness that can only report success
    // is not a witness; this one names the failure and the number, and says so on the wire.
    let _ = if first && dropped == 0 {
        fmt::Write::write_fmt(
            &mut w,
            format_args!(
                "\n:: FTDI-CAP: early-boot capture INTACT -- 0 byte(s) dropped ({} of {} buffered) ::\n",
                ring.len, CAP
            ),
        )
    } else if first {
        fmt::Write::write_fmt(
            &mut w,
            format_args!(
                "\n:: FTDI-CAP: early-boot capture TRUNCATED -- {} byte(s) LOST off the head before this line (ring cap {}, {} survived) ::\n",
                dropped, CAP, ring.len
            ),
        )
    } else {
        fmt::Write::write_fmt(
            &mut w,
            format_args!(
                "\n:: FTDI-CAP: capture ring OVERFLOWED -- {} further byte(s) lost ({} total, ring cap {}) ::\n",
                dropped - announced, dropped, CAP
            ),
        )
    };

    let len = w.n;
    if len == 0 || len > max {
        return None;
    }
    core::ptr::copy_nonoverlapping(buf.as_ptr(), dst, len);
    VERDICT_DUE.store(false, Ordering::Relaxed);
    ANNOUNCED_DROPS.store(dropped, Ordering::Relaxed);
    Some(len)
}

/// Whether the FTDI TX sink is live (console up + not disabled).
#[inline]
pub fn is_live() -> bool {
    LIVE.load(Ordering::Relaxed)
}

/// Mark the sink live (`true`, after bring-up) or permanently off (`false`, on TX failure).
#[inline]
pub fn set_live(v: bool) {
    LIVE.store(v, Ordering::Relaxed);
}

/// Copy up to `max` of the oldest buffered bytes into `dst`, advancing the ring past them. Returns
/// the number of bytes copied (0 = the ring is empty). The drain caller (the main-loop
/// `service_ftdi`) stages these into the FTDI slot's DMA buffer and pushes them out bulk-OUT.
///
/// Blocking `lock()` is used (not try_lock): this is called only from the single main loop, and the
/// only other holders of `RING` are `mirror`'s try_lock (which never blocks) and this function
/// itself — so no interrupt or other core can be holding it, and there is no deadlock. `dst` must be
/// valid for `max` bytes.
///
/// # Safety
/// `dst` must point to a writable region of at least `max` bytes.
pub unsafe fn drain_into(dst: *mut u8, max: usize) -> usize {
    let mut ring = RING.lock();
    // SERWIT-2: fold anything staged by a contended `mirror` into the capture BEFORE deciding what to
    // push out the cable, so a staged line reaches the wire on this pass rather than waiting for the
    // next print. A quiet machine that stopped printing must not leave its last lines in the ring.
    drain_staged_into(&mut ring);
    // CAPWIT: the capture states its own integrity BEFORE the replay it describes. A reader who is
    // handed a truncated boot log must be TOLD SO, on the wire, with the byte count — not left to
    // notice that a line they were not looking for is missing. That inference is what failed here for
    // two weeks, and the cable is the only channel an attended metal sitting has.
    if let Some(n) = emit_verdict(&ring, dst, max) {
        return n;
    }
    let n = max.min(ring.len);
    for i in 0..n {
        let idx = (ring.head + i) % CAP;
        *dst.add(i) = ring.buf[idx];
    }
    ring.head = (ring.head + n) % CAP;
    ring.len -= n;
    // PHASE31WIT: count what the cable actually received, so `late_verdict` can state the replay
    // size instead of leaving it to be reconstructed by measuring the capture file afterwards.
    REPLAYED.fetch_add(n as u64, Ordering::Relaxed);
    n
}

/// PHASE31WIT — the capture's integrity verdict, said AGAIN and LATE, as an ordinary console line.
///
/// [`emit_verdict`] already writes a `:: FTDI-CAP:` line, and it is the right design for what it
/// does: it goes straight into the DMA staging buffer, ahead of the replay, so it cannot itself
/// evict the bytes it is reporting on. But it is therefore the FIRST thing on the cable — emitted at
/// the instant the FTDI device comes up, which on a bench is before the operator's `cat
/// /dev/ttyUSB0` has the port open. The 2026-09-16 flight-8/flight-9 capture proves the gap
/// empirically: `FTDI-CAP` has ZERO hits across 11 265 lines and two boots, while the ring was
/// pinned at capacity on both — the one line that existed to announce the loss was itself lost, to
/// a different mechanism than the loss it was announcing.
///
/// So the verdict is said twice, on two channels with different failure modes: once ahead of the
/// replay where it cannot be evicted, and once here, well inside the live stream, where a late-
/// attaching operator and every log file will carry it. A diagnostic that can only be read by
/// someone who was already watching is not a diagnostic.
///
/// WAITS FOR THE REPLAY TO FINISH (`ring.len == 0`) before speaking, and that is not politeness:
/// fired on the first pass after the console goes live it would print `replayed=` at whatever the
/// first drain happened to have moved — a number that says nothing about saturation, which is the
/// one thing this line exists to report. An empty ring is the honest moment: everything the cable
/// was owed is on it. The main loop drains every pass, so this is reached almost immediately; the
/// verdict's own bytes re-fill the ring afterwards, which is harmless because the latch is spent.
///
/// Self-latched to one line; a relaxed load per pass thereafter. Takes no lock while printing — the
/// ring read is a `try_lock` dropped before the `serial_println!`, because that print re-enters
/// [`mirror`] and would otherwise meet a lock this function holds. A contended read simply leaves
/// the verdict owed and retries on the next pass. A failed `try_lock` here means "could not read",
/// never "nothing was lost" — the distinction [`dropped_bytes`] exists to preserve.
pub fn late_verdict() {
    if !LIVE.load(Ordering::Relaxed) || !LATE_VERDICT_DUE.load(Ordering::Relaxed) {
        return;
    }
    let Some((lost, pending)) = RING.try_lock().map(|r| (r.dropped, r.len)) else {
        return;
    };
    if pending != 0 {
        return; // the replay is still going out the cable — `replayed=` is not final yet
    }
    if !LATE_VERDICT_DUE.swap(false, Ordering::AcqRel) {
        return;
    }
    serial_println!(
        ":: FTDI-CAP: replayed={} cap={} lost={} head_cut={} ::",
        REPLAYED.load(Ordering::Relaxed),
        CAP,
        lost,
        if lost > 0 { "y" } else { "n" }
    );
}

// ── FTDIRX — the FTDI console learns to RECEIVE (rmbp A9, the x86 half of LEDGER S29) ────────────
//
// The module header above says, in as many words, that bulk-IN RX was "a STUB deferred to a future
// arc". This is that arc. The x86 half of the kernel had exactly one console — the FT232 cable on
// xHCI — and it was write-only: a bench operator could READ a boot but could not TYPE into it, so
// every interactive verdict on the 2012 rMBP was still a photograph of the panel plus a USB
// keyboard. After this module a byte typed on the bench side of the cable reaches
// `pal::EVENT_QUEUE` exactly as a UART byte does on the Orin (`arch/aarch64/serial.rs`'s
// `serialrx::drain`, whose shape this copies), and `x86_input_service`'s `next_event` drain then
// forwards it to the shell like any keystroke.
//
// WHAT IS PROTOCOL AND WHAT IS CONTROLLER. This module holds only the FT232 side of the arc — the
// two-byte modem-status prefix, the counters and the single intake that reaches `push_event`. The
// TRB/doorbell half (arm one Normal TRB on bulk-IN, claim its completion, re-arm) lives beside the
// TX pump in `drivers::xhci::mod`, because that is where a transfer ring is. The split is the same
// one the header already draws for TX, and it is what keeps this file arch-neutral.
//
// ⚠ TAIL MODULE ON PURPOSE. Knob-off this whole block is `#[cfg]`-erased and there is nothing below
// it to shift, so every panic `Location` in this file — and therefore the knob-off loadable image —
// is untouched (LEDGER P7; `./arroyo knoboff ftdirx`). A statement added ABOVE would not be.
#[cfg(feature = "ftdirx")]
pub mod ftdirx {
    use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};

    /// **EVERY FTDI bulk-IN packet is prefixed by TWO modem-status bytes** (Linux
    /// `drivers/usb/serial/ftdi_sio.h`: byte 0 = the modem status register, byte 1 = the line
    /// status register). They are not data and must never reach the key path — pushed through, a
    /// typed `help\n` arrives as `\x01\x60help\n` and the shell sees garbage. Stripping them is the
    /// one thing this arc could get silently wrong, which is why the go-red mutation in the gate is
    /// "delete the strip".
    pub const STATUS_BYTES: usize = 2;

    /// Bytes per IN transfer: 64, the FT232's full-speed bulk max packet size — ONE wire packet per
    /// TRB, so a short packet always retires the TD and the completion's residue is exact.
    pub const CHUNK: usize = 64;

    /// Byte offset of the RX landing zone inside the FTDI slot's `scsi_data_buffer`.
    ///
    /// The FTDI slot never runs BOT, so that 32 KiB buffer is free — `drain_ftdi` already reuses its
    /// first `FTDI_TX_CHUNK` (64) bytes as the TX staging area. RX takes
    /// a DISJOINT window rather than a second allocation: 1 KiB in is far past anything TX touches,
    /// is 64-byte aligned, and the buffer is 64 KiB-ALIGNED and 32 KiB long, so `[1024, 1088)`
    /// cannot cross a 64 KiB boundary (xHCI 1.2 §4.11.7.1 — the same rule that sizes the buffer).
    pub const BUF_OFFSET: usize = 1024;

    /// FTDI vendor request `FTDI_SIO_SET_LATENCY_TIMER` (Linux `ftdi_sio.h`), bmRequestType 0x40,
    /// wValue = milliseconds. The FT232's default is 16 ms: with no data to send the chip still
    /// answers an IN token every latency tick with a bare 2-byte status packet, and with data it
    /// waits up to that long before shipping a short packet. NOT issued by this arc's bring-up —
    /// 16 ms is already under a typist's inter-key gap and changing it would move the TX-side
    /// vendor-setup sequence that the U2.5 witness covers. Declared here so the next arc that wants
    /// a faster cable has the number and its provenance rather than a magic 0x09.
    pub const FTDI_SIO_SET_LATENCY_TIMER: u8 = 0x09;

    /// Is a Normal TRB outstanding on the bulk-IN endpoint right now? Exactly one, ever — the
    /// re-arm happens only after its completion has been consumed, so the ring can never over-arm.
    static ARMED: AtomicBool = AtomicBool::new(false);
    /// The armed TRB's physical address; the completion is matched against it.
    static TRB_PHYS: AtomicU64 = AtomicU64::new(0);
    static SLOT: AtomicU8 = AtomicU8::new(0);
    static DCI: AtomicU8 = AtomicU8::new(0);
    /// Set by the event-ring dispatch, consumed by the main-loop service pass. The dispatch does
    /// NOTHING else — LOCKFIX: `push_event` takes the event-queue lock and must never be reached
    /// from inside `drain_event_ring_once`.
    static DONE: AtomicBool = AtomicBool::new(false);
    static CODE: AtomicU8 = AtomicU8::new(0);
    /// The completion's TRB Transfer Length field = bytes NOT transferred (the residue).
    static RESIDUE: AtomicU32 = AtomicU32::new(0);

    /// Bytes delivered to the PAL queue (the header's `rx=`).
    static RX: AtomicU64 = AtomicU64::new(0);
    /// IN completions consumed, of any size.
    static PACKETS: AtomicU64 = AtomicU64::new(0);
    /// Completions carrying ONLY the two status bytes (or fewer) — the FT232's idle latency-timer
    /// poll. Counted and dropped; a cable that is merely quiet must not look like a cable that is
    /// dead, and this is the number that tells them apart.
    static IDLE: AtomicU64 = AtomicU64::new(0);
    /// Completions with a non-success, non-short completion code.
    static ERRORS: AtomicU64 = AtomicU64::new(0);
    static FIRST_LOGGED: AtomicBool = AtomicBool::new(false);
    /// Last `rx=` a rollup reported; the next fires when `rx` has at least DOUBLED it. The
    /// log-scale throttle `note_ftdi_pump` uses, for the same reason: a console that is being typed
    /// into must not spend its own bandwidth narrating itself, while the LAST such line still
    /// carries the run's true totals.
    static REPORTED: AtomicU64 = AtomicU64::new(0);

    /// Whether a TRB is outstanding (the service pass arms one when this is false).
    #[inline]
    pub fn armed() -> bool {
        ARMED.load(Ordering::Relaxed)
    }

    /// Record the TRB the service pass has just pushed, BEFORE its doorbell.
    #[inline]
    pub fn arm(slot: u8, dci: u8, trb_phys: u64) {
        SLOT.store(slot, Ordering::Relaxed);
        DCI.store(dci, Ordering::Relaxed);
        TRB_PHYS.store(trb_phys, Ordering::Relaxed);
        DONE.store(false, Ordering::Relaxed);
        ARMED.store(true, Ordering::Relaxed);
    }

    /// Drop all RX state: the console's slot is gone (teardown) or the sink went dark. The next
    /// service pass with a live console arms a fresh TRB on whatever slot it then has.
    #[inline]
    pub fn reset() {
        ARMED.store(false, Ordering::Relaxed);
        DONE.store(false, Ordering::Relaxed);
        TRB_PHYS.store(0, Ordering::Relaxed);
        SLOT.store(0, Ordering::Relaxed);
        DCI.store(0, Ordering::Relaxed);
    }

    /// EVENT-RING DISPATCH HALF. Claim a Transfer Event for the armed bulk-IN TRB — matched by slot
    /// + endpoint + TRB address, or by any error on our endpoint (an error event can name a
    /// different TRB of the same TD). Returns true iff the event was ours, in which case the caller
    /// consumes it. `DONE` is a FIRST-WRITE LATCH: a duplicate Success for an already-recorded
    /// completion (the Panther Point `XHCI_SPURIOUS_SUCCESS` quirk this driver already defends
    /// against on EP0 and on the HID reads) is consumed without overwriting the residue.
    ///
    /// LOCKFIX: this runs inside `drain_event_ring_once` and takes NO lock, prints nothing and
    /// touches no ring — it stores four relaxed words. Everything else waits for the main loop.
    pub fn claim(slot_id: u8, endpoint_id: u8, param: u64, code: u8, transfer_len: u32) -> bool {
        if !ARMED.load(Ordering::Relaxed)
            || SLOT.load(Ordering::Relaxed) != slot_id
            || DCI.load(Ordering::Relaxed) != endpoint_id
        {
            return false;
        }
        let is_error = code != 1 && code != 13;
        if param != TRB_PHYS.load(Ordering::Relaxed) && !is_error {
            return false;
        }
        if !DONE.swap(true, Ordering::Relaxed) {
            CODE.store(code, Ordering::Relaxed);
            RESIDUE.store(transfer_len, Ordering::Relaxed);
        }
        true
    }

    /// MAIN-LOOP HALF. Take the completed transfer, if one has landed: `(completion code, residue)`.
    /// Disarms, so the caller re-arms after delivering the bytes.
    pub fn take_done() -> Option<(u8, u32)> {
        if !DONE.load(Ordering::Relaxed) {
            return None;
        }
        DONE.store(false, Ordering::Relaxed);
        ARMED.store(false, Ordering::Relaxed);
        Some((CODE.load(Ordering::Relaxed), RESIDUE.load(Ordering::Relaxed)))
    }

    /// Note a failed IN completion. The read is re-armed regardless (the TransferRing recycles and
    /// CErr lets the controller retry) — an FTDI console that stops receiving on one bad packet is
    /// worse than one that drops it, and the count is what says which happened.
    pub fn note_error(code: u8) {
        ERRORS.fetch_add(1, Ordering::Relaxed);
        PACKETS.fetch_add(1, Ordering::Relaxed);
        serial_println!(":: FTDIRX: IN completion code={} errors={} — re-arming ::",
            code, ERRORS.load(Ordering::Relaxed));
    }

    /// **THE ONE INTAKE.** `pkt` is one bulk-IN packet exactly as the FT232 sent it: two modem-status
    /// bytes, then zero or more data bytes. Strip the two, push the rest as `Event::Key`.
    ///
    /// A packet of [`STATUS_BYTES`] or fewer is the chip's idle latency-timer poll — it carries no
    /// data, and pushing its status bytes would type `\x01\x60` into the shell every 16 ms. Counted,
    /// not delivered.
    ///
    /// Called ONLY from the main-loop service pass (never the event dispatch), so `push_event`'s
    /// event-queue lock and this function's `serial_println!` are both taken in the context every
    /// other console print already uses.
    pub fn deliver(pkt: &[u8]) {
        PACKETS.fetch_add(1, Ordering::Relaxed);
        if pkt.len() <= STATUS_BYTES {
            IDLE.fetch_add(1, Ordering::Relaxed);
            return;
        }
        for &b in &pkt[STATUS_BYTES..] {
            note_origin(b); crate::pal::push_event(crate::pal::Event::Key(b)); // SERIALDOOR — **THE ORIGIN TAG, and it is recorded BEFORE the push, never after.** `push_event` takes the event-queue lock and the drain can be running on another core the instant it is released, so a tag written after the push is a tag the door may look for and not find — one serial byte per boot silently taking the keyboard path, which is the class of defect nobody reproduces. Tagging first can only ever be early, and an early tag is harmless: `claim_origin` also matches the BYTE, so a tag with no event behind it is simply never claimed and ages out of the ring. See `note_origin`/`claim_origin` at this module's tail for the FIFO and for the one state it cannot distinguish. ⚠ FOLDED onto the existing push, CODE BEFORE COMMENT (LEDGER P7).
            let n = RX.fetch_add(1, Ordering::Relaxed) + 1;
            if !FIRST_LOGGED.swap(true, Ordering::Relaxed) {
                // THE WITNESS. One line, the first real byte only: it splits "the cable never
                // received" from "the cable received and the shell ignored it", which is the exact
                // split an attended bench sitting cannot make from the panel.
                serial_println!(
                    ":: FTDIRX: first byte rx={} byte={:#04x} '{}' idle={} ::",
                    n, b,
                    if (0x20u8..0x7f).contains(&b) { b as char } else { '.' },
                    IDLE.load(Ordering::Relaxed)
                );
            }
        }
        rollup();
    }

    /// The scoreable rollup, on the same log-scale throttle `note_ftdi_pump` uses: printed when
    /// `rx` has at least doubled the last reported count, so a burst of typing costs a handful of
    /// lines over a whole session — O(log n) lines for n bytes, which is what makes it safe on a
    /// console whose own output shares the cable.
    fn rollup() {
        let rx = RX.load(Ordering::Relaxed);
        let reported = REPORTED.load(Ordering::Relaxed);
        if rx < reported.saturating_mul(2).max(1) {
            return;
        }
        REPORTED.store(rx, Ordering::Relaxed);
        serial_println!(
            ":: FTDIRX: rx={} packets={} idle={} errors={} result=OK ::",
            rx,
            PACKETS.load(Ordering::Relaxed),
            IDLE.load(Ordering::Relaxed),
            ERRORS.load(Ordering::Relaxed)
        );
    }

    // ── SERIALDOOR — the ORIGIN TAG ──────────────────────────────────────────────────────────────
    //
    // Peter's ruling, 2026-09-17: *the serial console is a console, not a keyboard.* A byte typed at
    // the cable must reach the SHELL whatever holds window focus; a byte typed on the keyboard keeps
    // today's behaviour, so a focused Quarry may still open its selection with Enter.
    //
    // FTDICR measured the defect on flight 10 (`docs/dev/OS/02_KERNEL_CORE/serial_transport.md`
    // §FTDICR): `deliver` pushes `Event::Key(b)` into the SAME `pal` queue the HID decoders push into,
    // so `wc_route_event`'s furniture key doors judge a wire byte exactly as they judge a keystroke —
    // `[quarry] key_route key=0x0d focus=1 took=1`, and `help\r` never ran. The transport was
    // blameless: `rx=5`, `errors=0`, and the same byte passed the moment focus left (`focus=0 took=0`,
    // `[midden] cmd=` 19 ms later).
    //
    // WHY A FIFO OF BYTES AND NOT A COUNTER. A counter ("the next N keys are serial") is wrong the
    // first time a keyboard report interleaves with the cable: the queue is one FIFO shared by both
    // producers, so a credit would be spent on whichever key came out next. This ring records WHAT was
    // pushed, in order, and [`claim_origin`] pops only when the byte at the head MATCHES the byte at
    // the door. The one state it cannot separate is stated rather than hidden: the same byte value
    // typed on the keyboard while a serial byte of that value is outstanding is claimed as serial, so
    // that keystroke reaches the shell instead of the focused window. It needs two people typing the
    // same character into two devices inside one drain pass; it costs one keystroke; and it fails
    // toward the shell, which is the safe direction (the operator can always get out).
    //
    // SPSC and lock-free BY CONSTRUCTION, which is what makes it legal here: the producer is the xHCI
    // main-loop service pass (`deliver`'s only caller) and the consumer is the key drain, one of each,
    // so the two indices need no CAS. `note_origin` is the only writer of `ORIGIN_W`, `claim_origin`
    // the only writer of `ORIGIN_R`.
    //
    // ⚠ TAIL OF THE TAIL MODULE. This block is appended BELOW everything in a module that is itself
    // `#[cfg]`-erased knob-off, so no panic `Location` in this file moves and the knob-off loadable
    // image is untouched — the rule the module header states, applied to the module's own tail.

    /// SERIALDOOR — outstanding tags. 128 is four full FT232 packets' worth of data bytes (`CHUNK`
    /// 64 minus `STATUS_BYTES`, so 62 each) and the drain empties the queue every pass, so the ring
    /// is sized to hold a burst the console can produce between two passes and not to hold a session.
    const ORIGIN_CAP: u64 = 128;
    static ORIGIN_RING: [AtomicU8; ORIGIN_CAP as usize] = [const { AtomicU8::new(0) }; ORIGIN_CAP as usize];
    /// Write and read cursors, monotonic and wrapping; `W - R` is the occupancy.
    static ORIGIN_W: AtomicU64 = AtomicU64::new(0);
    static ORIGIN_R: AtomicU64 = AtomicU64::new(0);
    /// Tags the door actually claimed — the numerator of the census line.
    static ORIGIN_CLAIMED: AtomicU64 = AtomicU64::new(0);
    /// Tags DROPPED because the ring was full. A dropped tag is not a lost byte: the byte still
    /// reaches the door, it is simply judged as a keystroke, which is exactly the pre-arc behaviour.
    /// Counted rather than swallowed so a capture can say whether the ring was ever the limit.
    static ORIGIN_OVERRUN: AtomicU64 = AtomicU64::new(0);

    /// Record that the byte about to be pushed came off the WIRE. Producer side; called only from
    /// [`deliver`], immediately before the push.
    fn note_origin(b: u8) {
        let w = ORIGIN_W.load(Ordering::Relaxed);
        if w.wrapping_sub(ORIGIN_R.load(Ordering::Acquire)) >= ORIGIN_CAP {
            ORIGIN_OVERRUN.fetch_add(1, Ordering::Relaxed);
            return;
        }
        ORIGIN_RING[(w % ORIGIN_CAP) as usize].store(b, Ordering::Relaxed);
        ORIGIN_W.store(w.wrapping_add(1), Ordering::Release);
    }

    /// **The door's question: is this `Key(b)` the next byte the wire owes?** Consumer side, called
    /// from `arch::x86_64::syscall::wc_route_event` at the TOP of the key door. Pops and answers
    /// `true` only when the ring is non-empty AND its head is this exact byte; otherwise the event is
    /// a keystroke and nothing is consumed.
    pub fn claim_origin(b: u8) -> bool {
        let r = ORIGIN_R.load(Ordering::Relaxed);
        if r == ORIGIN_W.load(Ordering::Acquire) {
            return false;
        }
        if ORIGIN_RING[(r % ORIGIN_CAP) as usize].load(Ordering::Relaxed) != b {
            return false;
        }
        ORIGIN_R.store(r.wrapping_add(1), Ordering::Release);
        ORIGIN_CLAIMED.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// SERIALDOOR — `(claimed, outstanding, overrun)`. Read by the fixture and by the rollup so a
    /// capture can say whether every tagged byte was claimed, rather than only that the door fired.
    pub fn origin_census() -> (u64, u64, u64) {
        (
            ORIGIN_CLAIMED.load(Ordering::Relaxed),
            ORIGIN_W.load(Ordering::Acquire).wrapping_sub(ORIGIN_R.load(Ordering::Acquire)),
            ORIGIN_OVERRUN.load(Ordering::Relaxed),
        )
    }

    /// SERIALDOOR — the fixture's producer seam: tag a byte and push it exactly as [`deliver`] does,
    /// without a controller. `#[cfg(feature = "witness")]` so no shipped image carries a way to
    /// synthesise wire bytes.
    #[cfg(feature = "witness")]
    pub fn inject_serial_byte(b: u8) {
        note_origin(b);
        crate::pal::push_event(crate::pal::Event::Key(b));
    }

    /// SERIALDOOR — tag a byte WITHOUT pushing it, for a fixture that wants to drive the router
    /// directly rather than through the queue. Same order as the live path: tag, then deliver.
    #[cfg(feature = "witness")]
    pub fn tag_serial_byte(b: u8) {
        note_origin(b);
    }
}
