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
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
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

/// Boot-capture ring capacity. 64 KiB comfortably holds the whole pre-FTDI boot log so that when
/// the console comes up mid-boot the entire early log replays out the cable.
const CAP: usize = 64 * 1024;

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

/// Lines staged for the cable but not yet copied into the capture ring.
pub fn staged_in_flight() -> u64 {
    STAGE.in_flight()
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
        let _ = fmt::write(&mut *ring, args);
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
        let _ = fmt::write(&mut *ring, args);
        tap.absorb();
        return;
    }
    tap.drop_line();
    UNANNOUNCED.fetch_add(1, Ordering::Relaxed);
}

/// Copy every staged line into the capture ring, in order, and put any un-announced loss into the
/// stream as a real line.
///
/// **Caller must hold `RING`** — that is what makes the drainer unique. No print, no allocation and no
/// second lock happens in here, so it is safe from the same IRQ-masked print context `mirror` is.
fn drain_staged_into(ring: &mut Ring) {
    let n = STAGE.drain(|s| {
        for &b in s.as_bytes() {
            ring.push_byte(b);
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
    }
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
    let n = max.min(ring.len);
    for i in 0..n {
        let idx = (ring.head + i) % CAP;
        *dst.add(i) = ring.buf[idx];
    }
    ring.head = (ring.head + n) % CAP;
    ring.len -= n;
    n
}
