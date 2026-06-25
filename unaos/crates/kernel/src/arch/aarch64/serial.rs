use core::fmt::{self, Write};
use spin::Mutex;
use lazy_static::lazy_static;

// QEMU `virt` PL011 base. The real Pi 4 (BCM2711) PL011 is at 0xFE201000 — a different address —
// so the `pi` build does NOT touch this UART at all (writing here would land in RAM and the TXFF
// wait could spin forever); it relies on fbcon for on-screen output, exactly like the Mac.
#[cfg_attr(feature = "pi", allow(dead_code))]
const UART0_ADDR: usize = 0x0900_0000;

pub struct SerialPort;

impl SerialPort {
    pub fn new() -> SerialPort {
        SerialPort
    }

    pub fn write_byte(&self, byte: u8) {
        #[cfg(feature = "pi")]
        let _ = byte; // no UART on the Pi build — fbcon carries output
        #[cfg(not(feature = "pi"))]
        unsafe {
            let uart = UART0_ADDR as *mut u8;
            // Bounded TXFF wait: never spin forever if the UART is absent/misaddressed.
            let mut spins: u32 = 0;
            while (core::ptr::read_volatile(uart.add(0x18)) & (1 << 5)) != 0 {
                spins += 1;
                if spins > 1_000_000 {
                    break;
                }
                core::hint::spin_loop();
            }
            core::ptr::write_volatile(uart, byte);
        }
    }
}

impl Write for SerialPort {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for byte in s.bytes() {
            self.write_byte(byte);
        }
        Ok(())
    }
}

lazy_static! {
    pub static ref SERIAL_PORT: Mutex<SerialPort> = Mutex::new(SerialPort::new());
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    // Guard the serial lock with interrupts masked (matching x86) so an interrupt handler that
    // logs can't deadlock against an in-progress print holding the same lock.
    crate::arch::without_interrupts(|| {
        // Best-effort, never panic (write_byte is a no-op on the `pi` build).
        let _ = SERIAL_PORT.lock().write_fmt(args);
    });
    // Mirror to the framebuffer console (visible without a serial port). fbcon self-guards.
    crate::video::fbcon::_print(args);
}

// Expression-style (parentheses, no trailing semicolon) so the macros work in both
// statement and expression position — matching the x86_64 serial macros.
#[macro_export]
macro_rules! serial_print {
    ($($arg:tt)*) => (
        $crate::arch::aarch64::serial::_print(format_args!($($arg)*))
    );
}

#[macro_export]
macro_rules! serial_println {
    () => (
        $crate::arch::aarch64::serial::_print(format_args!("\n"))
    );
    ($fmt:expr) => (
        $crate::arch::aarch64::serial::_print(format_args!(concat!($fmt, "\n")))
    );
    ($fmt:expr, $($arg:tt)*) => (
        $crate::arch::aarch64::serial::_print(format_args!(concat!($fmt, "\n"), $($arg)*))
    );
}

impl SerialPort {
    pub fn read_byte(&self) -> Option<u8> {
        // No UART input on the Pi build — reading the QEMU address would be RAM and inject phantom
        // keystrokes. Video-only; there's no serial console on the Pi anyway.
        #[cfg(feature = "pi")]
        {
            None
        }
        #[cfg(not(feature = "pi"))]
        unsafe {
            let uart = UART0_ADDR as *mut u8;
            if (core::ptr::read_volatile(uart.add(0x18)) & (1 << 4)) == 0 {
                Some(core::ptr::read_volatile(uart))
            } else {
                None
            }
        }
    }
}
