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
/// Cost: 256 KiB of `.bss`. It is a zero-initialised `static`, so it adds nothing to either kernel
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
const CAP: usize = 256 * 1024;

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
/// **try_lock only, never blocks** — the discipline of the SERIAL1 `_print` path, and non-negotiable:
/// this runs OUTSIDE `SERIAL1` and OUTSIDE the interrupt mask, so every core reaches it at once, and a
/// mirror that could block here would be able to stall the primary wire. That inversion would be worse
/// than any drop.
///
/// **SERWIT-2 — what changed.** The failure branch used to be nothing: on contention the whole
/// formatted line evaporated, with no counter anywhere. That is the costliest instance of the defect in
/// the tree, because THIS IS THE BENCH'S OWN CAPTURE PATH — the 2012 rMBP has no 16550, so on an
/// attended metal sitting the FTDI cable is not a mirror of the evidence, it IS the evidence. It lost
/// lines precisely when the machine was busiest, which is the only moment the wedge and cursor
/// investigations care about, and it lost them invisibly.
///
/// Now: take the ring if it is free (draining anyone else's staged lines first, so deferral never
/// reorders the capture); otherwise defer the whole line into the lock-free [`STAGE`] ring; and if that
/// is full too, try the ring once more before giving up — the retry is free (`try_lock`) and turns most
/// of what would remain into a hit. Only after all three does a line count as lost, and a lost line is
/// COUNTED and announced IN THE CAPTURE STREAM ITSELF (see [`drain_staged_into`]), which is the only
/// channel a bench sitting has.
pub fn mirror(args: fmt::Arguments) {
    let tap = &crate::serial_ring::TAP_FTDI;
    tap.submit();
    if let Some(mut ring) = RING.try_lock() {
        drain_staged_into(&mut ring);
        let _ = ring_write(&mut ring, args);
        tap.absorb();
        return;
    }
    match STAGE.stage(args) {
        crate::serial_ring::Staged::Whole => {
            tap.note_staged();
            return;
        }
        crate::serial_ring::Staged::Truncated => {
            // Sealed with a visible marker in the capture; counted and announced like a loss.
            tap.note_staged();
            tap.tear();
            UNANNOUNCED.fetch_add(1, Ordering::Relaxed);
            return;
        }
        crate::serial_ring::Staged::Full => {}
    }
    // Staging ring full: one free retry at the sink before the line is declared lost.
    if let Some(mut ring) = RING.try_lock() {
        drain_staged_into(&mut ring);
        let _ = ring_write(&mut ring, args);
        tap.absorb();
        return;
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
    n
}
