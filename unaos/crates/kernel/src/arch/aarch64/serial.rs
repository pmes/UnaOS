use core::fmt::{self, Write};
use spin::Mutex;
use lazy_static::lazy_static;

const UART0_ADDR: usize = 0x0900_0000;

pub struct SerialPort;

impl SerialPort {
    pub fn new() -> SerialPort {
        SerialPort
    }

    pub fn write_byte(&self, byte: u8) {
        let uart = UART0_ADDR as *mut u8;
        unsafe {
            // Wait until UART is ready to transmit
            while (core::ptr::read_volatile(uart.add(0x18)) & (1 << 5)) != 0 {}
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
    SERIAL_PORT.lock().write_fmt(args).unwrap();
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
        let uart = UART0_ADDR as *mut u8;
        unsafe {
            if (core::ptr::read_volatile(uart.add(0x18)) & (1 << 4)) == 0 {
                Some(core::ptr::read_volatile(uart))
            } else {
                None
            }
        }
    }
}
