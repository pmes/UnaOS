use core::fmt::{self, Write};
use spin::Mutex;
use lazy_static::lazy_static;

// PL011 UART base. QEMU `virt` puts the PL011 at 0x09000000; the real Pi 4 (BCM2711, low-peripheral
// mode) puts UART0/PL011 at 0xFE201000 — identical register layout, different address. The Pi
// firmware (config.txt `enable_uart=1`, with the PL011 routed to GPIO14/15 via the `miniuart-bt`
// overlay) leaves it initialised at 115200 8N1, and UEFI uses it as its console, so we INHERIT that
// setup and just push bytes through the data register — no baud/line reprogramming. With the
// official Debug Probe on GPIO14/15 this is the Pi's real serial console (replacing the earlier
// `pi`=fbcon-only path, which only existed because the address was hardcoded to the QEMU one).
#[cfg(feature = "pi")]
const UART0_ADDR: usize = 0xFE20_1000;
#[cfg(not(feature = "pi"))]
const UART0_ADDR: usize = 0x0900_0000;

pub struct SerialPort;

impl SerialPort {
    pub fn new() -> SerialPort {
        SerialPort
    }

    pub fn write_byte(&self, byte: u8) {
        unsafe {
            let uart = UART0_ADDR as *mut u8;
            // Bounded TXFF wait (FR bit 5 = TX FIFO full): never spin forever if the UART is
            // absent/misaddressed — fbcon still carries output, so a wrong base degrades, not hangs.
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
        unsafe {
            let uart = UART0_ADDR as *mut u8;
            // RXFE (FR bit 4) clear => a byte is waiting. On the real PL011 (Pi 0xFE201000 or QEMU
            // 0x09000000) this is a true RX-empty flag, so it gives a serial-console keyboard rather
            // than the phantom keystrokes the old hardcoded-QEMU-address `pi` path had to avoid.
            if (core::ptr::read_volatile(uart.add(0x18)) & (1 << 4)) == 0 {
                Some(core::ptr::read_volatile(uart))
            } else {
                None
            }
        }
    }
}

// ---- M5c: PL011 RX interrupt → wake the scheduled input task (bare-metal Pi only) ----------------
//
// Instead of the input task polling the UART, the PL011 raises an interrupt when a byte arrives; the
// GIC delivers it as SPI 153 to the input core, the ISR (`on_rx_interrupt`) masks the source and
// posts `RX_READY`, and the input task wakes to drain the FIFO. Metal-only: QEMU raspi4b delivers no
// Group-1 IRQ, so `timer::is_live()` is false there and the input task keeps polling (this stays
// unused). All the interrupt work is on the PL011's own registers + a scheduler Semaphore — none of
// it touches the `SERIAL_PORT` spin lock (poll_input holds that IRQ-unmasked, so an ISR that took it
// would self-deadlock same-core).

#[cfg(feature = "baremetal")]
const UART_IMSC: usize = 0x38; // interrupt mask set/clear
#[cfg(feature = "baremetal")]
const UART_ICR: usize = 0x44; // interrupt clear
#[cfg(feature = "baremetal")]
const UART_RXIM: u32 = 1 << 4; // receive interrupt mask (fires at the FIFO trigger level)
#[cfg(feature = "baremetal")]
const UART_RTIM: u32 = 1 << 6; // receive-timeout mask (fires for a lone byte below the trigger level)

/// BCM2711 GIC-400 SPI for UART0/PL011: device-tree `GIC_SPI 121` => INTID 121 + 32 = 153, in GIC
/// mode (config.txt `enable_gic=1`). Only referenced on the bare-metal interrupt-driven-input path.
#[cfg(feature = "baremetal")]
pub const PL011_RX_INTID: u32 = 153;

/// Posted by the RX ISR; the scheduled input task blocks on it. Counting, so a post that lands before
/// the task waits is not lost. Must be `init()`ed on the BSP before the input task blocks (capacity).
#[cfg(feature = "baremetal")]
pub static RX_READY: crate::arch::sched::Semaphore = crate::arch::sched::Semaphore::new(0);

/// Set true the first time the RX ISR runs. The input task logs it once AFTER waking (never from the
/// ISR — a `serial_println!` there would deadlock on `SERIAL_PORT`); it distinguishes a real
/// interrupt wake from a backstop poll, confirming the SPI actually delivers on this board.
#[cfg(feature = "baremetal")]
pub static RX_IRQ_SEEN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Arm the PL011 RX + RX-timeout interrupts (clearing any stale latched state first). Enable BOTH:
/// single keystrokes sit below the RX FIFO trigger level, so the RX-TIMEOUT interrupt is what fires
/// for them. Call once when arming interrupt-driven input.
#[cfg(feature = "baremetal")]
pub fn enable_rx_interrupt() {
    unsafe {
        core::ptr::write_volatile((UART0_ADDR + UART_ICR) as *mut u32, UART_RXIM | UART_RTIM);
        core::ptr::write_volatile((UART0_ADDR + UART_IMSC) as *mut u32, UART_RXIM | UART_RTIM);
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

/// Re-arm the RX interrupts after the input task drained the FIFO. Deliberately does NOT write ICR:
/// a byte that slipped into the FIFO during the drain still has its receive-timeout pending, and
/// clearing it (ICR) would drop the level that re-triggers the ISR. Unmasking IMSC alone re-arms.
#[cfg(feature = "baremetal")]
pub fn rearm_rx_interrupt() {
    unsafe {
        core::ptr::write_volatile((UART0_ADDR + UART_IMSC) as *mut u32, UART_RXIM | UART_RTIM);
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

/// True if a received byte is waiting in the RX FIFO (FR.RXFE clear). Closes the tiny window between
/// the input task's last drain read and its IMSC re-arm.
#[cfg(feature = "baremetal")]
pub fn rx_pending() -> bool {
    unsafe { (core::ptr::read_volatile((UART0_ADDR + 0x18) as *const u32) & (1 << 4)) == 0 }
}

/// PL011 RX interrupt service — called from `gic::handle_irq` (INTID 153) with IRQ masked. Masks the
/// RX interrupts (writing IMSC=0 forces MIS = RIS & IMSC = 0, deasserting the line to the GIC) with a
/// `dsb` so that store is globally visible BEFORE the handler's EOI deactivates the SPI — otherwise
/// the level-sensitive line would still read high at EOI and the GIC would re-pend a phantom 153.
/// Then wakes the input task. Does NOT read the FIFO (the task drains it) and NEVER logs.
#[cfg(feature = "baremetal")]
pub fn on_rx_interrupt() {
    unsafe {
        core::ptr::write_volatile((UART0_ADDR + UART_IMSC) as *mut u32, 0);
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
    RX_IRQ_SEEN.store(true, core::sync::atomic::Ordering::Relaxed);
    RX_READY.post();
}
