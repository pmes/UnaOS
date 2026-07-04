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

#[doc(hidden)]
pub fn _print(args: ::core::fmt::Arguments) {
    use core::fmt::Write;
    use x86_64::instructions::interrupts;

    // Best-effort, never-panic: skip the UART if absent (None) or if its lock is already held
    // (a panic mid-print would otherwise self-deadlock here). Output is dropped, not fatal.
    interrupts::without_interrupts(|| {
        if let Some(mut guard) = SERIAL1.try_lock() {
            if let Some(uart) = guard.as_mut() {
                let _ = uart.write_fmt(args);
            }
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
