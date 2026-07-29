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

use lazy_static::lazy_static;
use spin::Mutex;
use uart_16550::backend::PioBackend;
use uart_16550::Uart16550Tty;

lazy_static! {
    // `None` when there is no 16550 at 0x3F8. `new_port` runs init() + a loopback self-test and
    // returns Err on real laptops / the Pi (no UART there) — we must treat that as "no serial",
    // NEVER panic. The original bug: `.unwrap()` here panicked on metal, then the panic handler
    // called serial_println! which re-ran this initializer and recursed — a red screen with no
    // message and a freeze. fbcon mirrors all output to the framebuffer regardless.
    pub static ref SERIAL1: Mutex<Option<Uart16550Tty<PioBackend>>> = {
        let serial_port = unsafe { Uart16550Tty::new_port(0x3F8, uart_16550::Config::default()).ok() };
        Mutex::new(serial_port)
    };
}

/// Tri-state memory of whether this machine actually has a 16550 at 0x3F8, learned the first time the
/// lazy `SERIAL1` initializer runs. 0 = not yet known, 1 = present, 2 = absent.
///
/// Load-bearing for the staging ring: on a machine with NO serial port (a real laptop, the Pi) nothing
/// will ever drain the ring, so staging a contended line there would silently fill it and then produce
/// a stream of bogus `[serial] dropped N lines` markers about output that was never lost — fbcon
/// mirrors every one of those lines to the framebuffer regardless. On a serial-less machine the UART
/// path stays exactly what it always was: a no-op.
static UART_STATE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

#[doc(hidden)]
pub fn _print(args: ::core::fmt::Arguments) {
    use core::fmt::Write;
    use core::sync::atomic::Ordering;
    use x86_64::instructions::interrupts;

    crate::serial_ring::note_submitted();

    // PANIC ESCAPE HATCH. Past `enter_panic_mode` the Mutex is not touched at all: the machine is
    // dying, this may well be the very core that owns `SERIAL1` (the historical self-deadlock the
    // `try_lock` was introduced to survive — at the cost of eating the panic message outright), and a
    // last-words path that can lose its words is not a last-words path. `raw_write_str` is the same
    // lock-free bounded-poll sequence WEDGE-2 uses, so it acquires nothing and cannot deadlock; it is
    // synchronous, so the bytes are on the wire before the next statement runs. Drain the staged
    // backlog first so lines queued just before the fault are not buried with the machine.
    if crate::serial_ring::in_panic_mode() {
        crate::serial_ring::drain(raw_write_str);
        let mut raw = RawUart;
        let _ = raw.write_fmt(args);
        crate::serial_ring::note_emitted();
        crate::video::fbcon::_print(args);
        crate::drivers::xhci::ftdi::mirror(args);
        crate::selftest::capture(args);
        crate::flight_recorder::capture(args);
        return;
    }

    // Never-panic, and now never-SILENT: take the UART if it is free, otherwise DEFER the whole line
    // into the lock-free staging ring for the next holder to emit intact. The `try_lock` (never
    // `lock`) is still the rule — a print from an IRQ-masked or fault context must not be able to
    // block on a console another core owns — but its failure branch is no longer a silent `drop`,
    // which is what let arbitrary verdict lines evaporate under load. See `crate::serial_ring`.
    interrupts::without_interrupts(|| {
        if let Some(mut guard) = SERIAL1.try_lock() {
            UART_STATE.store(if guard.is_some() { 1 } else { 2 }, Ordering::Relaxed);
            if let Some(uart) = guard.as_mut() {
                // Emit anyone else's deferred lines BEFORE our own, so deferral never reorders the
                // wire (a line staged at t0 precedes a line written directly at t1 > t0), and so the
                // ring is kept shallow — a ring that is drained on every uncontended print is a ring
                // that essentially never reaches the full-and-must-drop state.
                {
                    let mut sink = |s: &str| {
                        #[cfg(feature = "logts")]
                        {
                            let _ = crate::logts::PrefixWriter { inner: uart }.write_str(s);
                        }
                        #[cfg(not(feature = "logts"))]
                        {
                            let _ = uart.write_str(s);
                        }
                    };
                    crate::serial_ring::drain(&mut sink);
                }
                // CLOCK-2: with `logts`, prefix each serial LINE with a compact timestamp (monotonic
                // ms → UTC after a civil anchor exists). Only the UART byte-stream is touched; the
                // fbcon + capture-ring mirrors below still receive the raw `args`. OFF => identical.
                #[cfg(feature = "logts")]
                {
                    let _ = crate::logts::PrefixWriter { inner: uart }.write_fmt(args);
                }
                #[cfg(not(feature = "logts"))]
                {
                    let _ = uart.write_fmt(args);
                }
                crate::serial_ring::note_emitted();
            } else {
                // No 16550 on this machine. Nothing was written, nothing was lost that fbcon does not
                // already carry — and there is no reason to keep a backlog nobody will ever drain.
                crate::serial_ring::drain(|_| {});
            }
        } else if UART_STATE.load(Ordering::Relaxed) != 2 {
            // Contended. Defer rather than drop; if the ring is full the loss is COUNTED and the next
            // drain announces it on the wire as `[serial] dropped N lines`.
            crate::serial_ring::stage(args);
        }
    });
    // Mirror to the framebuffer console so diagnostics/panics are visible on hardware that has
    // no serial port. `Arguments` is Copy; fbcon self-guards (try_lock + interrupts off).
    crate::video::fbcon::_print(args);
    // U2.5: mirror into the FTDI console boot-capture ring — ALWAYS, from the very first print, so
    // when the USB-serial console comes up mid-boot the whole early log replays out the cable. The
    // ring self-guards (try_lock only, never blocks, drop-oldest on overflow); it never takes the
    // XHCI_CONTROLLER lock or allocates, so this is safe from any print context.
    crate::drivers::xhci::ftdi::mirror(args);
    // TSTE-1 M2b: capture boot-fixture verdict lines (`-> PASS`/`-> FAIL`) into the selftest ring so
    // `tste` can replay them. Additive, alloc-free, `try_lock` only; safe from this IRQ-masked
    // context; zero change to what is printed above.
    crate::selftest::capture(args);
    // FLIGHT-RECORDER: capture the exact serial line bytes into the boot-log ring so `service()` can
    // later flush the whole boot log to UNAOS.LOG on the FAT volume. Same discipline as the taps
    // above — additive, alloc-free, `try_lock` only, drop-on-full; zero change to what is printed.
    crate::flight_recorder::capture(args);
}

#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => {
        $crate::arch::serial::_print(format_args!($($arg)*));
    };
}

#[macro_export]
macro_rules! serial_println {
    () => ($crate::arch::serial::_print(format_args!("\n")));
    ($($arg:tt)*) => ($crate::arch::serial::_print(format_args!("{}\n", format_args!($($arg)*))));
}

/// One byte at the 16550, **taking no lock**: a bounded poll of `LSR` bit 5 (transmitter holding
/// register empty) at `0x3F8 + 5`, then one `out` to the THR at `0x3F8`.
///
/// This is the tree's single lock-free UART write primitive, shared by two callers with the same hard
/// requirement — that they cannot block on anything:
///   * WEDGE-2/WEDGE-4 breadcrumbs (see [`wedge2_raw_byte`] and `crate::wedge2`), which must survive a
///     core dying with IRQs masked while holding any of `SERIAL1`/`FBCON`/`WRITER`/the allocator;
///   * the panic escape hatch in [`_print`], which must emit synchronously even when the panicking
///     core is itself the owner of `SERIAL1`.
///
/// It acquires NOTHING and allocates nothing. The spin is bounded so a machine with no 16550 degrades
/// (bytes into the void) instead of hanging — the same bound the aarch64 twin carries.
#[inline(never)]
pub fn raw_byte(byte: u8) {
    use x86_64::instructions::port::Port;
    unsafe {
        let mut lsr: Port<u8> = Port::new(0x3F8 + 5);
        let mut thr: Port<u8> = Port::new(0x3F8);
        let mut spins: u32 = 0;
        while (lsr.read() & (1 << 5)) == 0 {
            spins += 1;
            if spins > 1_000_000 {
                break;
            }
            core::hint::spin_loop();
        }
        thr.write(byte);
    }
}

/// A `core::fmt::Write` over [`raw_byte`] — lock-free formatting straight at the UART, for the panic
/// path only. Deliberately NOT used on any ordinary print: because it takes no lock, its bytes can
/// interleave with another core's in-progress line, which is the right trade for last words and the
/// wrong one for a `PASS` tally.
pub struct RawUart;

impl core::fmt::Write for RawUart {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for b in s.bytes() {
            raw_byte(b);
        }
        Ok(())
    }
}

/// Free-function form of [`RawUart`]'s writer, shaped for `serial_ring::drain`'s `FnMut(&str)` sink.
pub fn raw_write_str(s: &str) {
    for b in s.bytes() {
        raw_byte(b);
    }
}

/// WEDGE-2 — the x86_64 half of the breadcrumb seam: one byte at the UART, **taking no lock**.
///
/// Deliberately NOT `SERIAL1`: that is a `Mutex<Option<Uart16550Tty<_>>>`, and a breadcrumb whose job
/// is to survive a core dying with IRQs masked must never be able to block. This is the bare 16550
/// sequence instead — a bounded poll of `LSR` bit 5 (transmitter holding register empty) at
/// `0x3F8 + 5`, then one `out` to the THR at `0x3F8`. It acquires nothing: not `SERIAL1`, not `FBCON`,
/// not the video locks, not the allocator — all of which are reachable from the focus chain WEDGE-2
/// instruments. The spin is bounded so a machine with no 16550 degrades instead of hanging.
///
/// s44 (the x86 reproduction, capture truncated mid-word) is why this exists on this arch at all: the
/// mechanism is arch-neutral, so the instrumentation has to be portable and only this function is not.
/// See `crate::wedge2` for the token table and the interleaving trade-off.
///
/// SERWIT-1 note: the body moved verbatim into [`raw_byte`] so the panic escape hatch could share the
/// one audited lock-free sequence. Nothing about WEDGE-2's contract changed — same bounded LSR poll,
/// same single `out`, still no lock of any kind, and the staging ring this file gained is invisible
/// from here (breadcrumbs never enter `serial_ring`, so there is no new lock they could contend for).
#[cfg(feature = "wedge2")]
#[inline(never)]
pub fn wedge2_raw_byte(byte: u8) {
    raw_byte(byte);
}
