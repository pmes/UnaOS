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
#[cfg(all(not(feature = "pi"), not(feature = "tegra")))]
const UART0_ADDR: usize = 0x0900_0000;

// ── Jetson Orin Nano / Tegra234 serial (`tegra` feature) ────────────────────────────────────
// With `tegra`, SerialPort bypasses the PL011 register code in this file and drives a Tegra234
// NS16550-style UART instead (32-bit regs, reg-shift = 2). Mutually exclusive with `pi` (one
// board UART per build; `baremetal` implies `pi`, so the M5c PL011 RX-interrupt machinery below
// can never coexist with `tegra`).
#[cfg(all(feature = "pi", feature = "tegra"))]
compile_error!("kernel features `pi` and `tegra` are mutually exclusive — pick one board UART");

// ── Jetson Orin Nano / Tegra234: NS16550-style UART ──────────────────────────────────────────
#[cfg(feature = "tegra")]
mod tegra {
    // Tegra UART base. *** TO VERIFY ON THE BOARD. *** Default = UARTC @ 0x0C28_0000: on the Orin
    // Nano dev kit the debug header the brief targets (the button-header USB-TTL — pin 3 RXD /
    // pin 4 TXD, 115200 8N1) is physically driven by UARTC. It is the port the SPE streams the
    // Tegra Combined UART (TCU) console onto, i.e. where bytes actually reach the adapter. (This
    // overrides the brief's UART_A guess of 0x0310_0000 — that pin group is the 40-pin header, is
    // `status = disabled` in the device tree, and is not a UEFI console.) Caveats from research:
    //   • UARTC lives in the always-on (AON) cluster and is normally OWNED by the SPE for the TCU.
    //     If the SPE keeps muxing the TCU after we take over, our writes and its may interleave
    //     (garbled output). By UEFI handoff UARTC is clocked + out-of-reset, so polled OUTPUT
    //     usually works untouched — but this is the #1 thing to confirm on the board.
    //   • Diagnosing silence on the board: LSR reading 0x00 (or a quiescent value) means the AON
    //     UART is likely held in reset / clock-gated → it needs a BPMP IVC call to deassert reset +
    //     ungate the clock. LSR reading an 0xDEAD.... fill is instead the SoC's decode/access-error
    //     sentinel → suspect a wrong BASE or a firewall denying CCPLEX access; try a fallback UART
    //     rather than ungating. (Neither is done here; output-only inherits whatever firmware/SPE
    //     left — we deliberately do NOT reprogram the baud divisor, whose clock is BPMP-managed and
    //     not a fixed PC value.)
    // Fallbacks if UARTC is silent or garbled (edit BASE and rebuild — `./arroyo check` is ~25s):
    //   0x0310_0000  UARTA — clean CCPLEX-owned 16550, but `status = disabled`, NOT routed to the
    //                debug header (it's on the 40-pin header), and needs clock/reset/pinmux + baud
    //                setup we don't do here. 0x0314_0000 UARTE is the next candidate, same caveats.
    //   If output is garbled (wrong line control), set LCR (0x0C) = 0x03 for 8N1 and program the
    //   divisor — but verify the UART clock on the board first.
    // Do NOT point this at the TCU mailbox (HSP doorbell @ ~0x0C16_8000): that is a different,
    // non-16550 protocol; a polled LSR/THR driver cannot drive it.
    const BASE: usize = 0x0C28_0000;

    // 16550 registers with Tegra reg-shift = 2 → each logical register is 4 bytes apart and
    // accessed as a 32-bit word (meaningful data in the low 8 bits). THR/RBR at +0x00, LSR at
    // register index 5 → byte offset 5 << 2 = 0x14.
    const LSR: usize = 5 << 2;
    const LSR_THRE: u32 = 1 << 5; // transmit holding register empty (ok to write)
    const LSR_DR: u32 = 1 << 0; // receive data ready

    pub fn write_byte(byte: u8) {
        unsafe {
            let thr = BASE as *mut u32;
            let lsr = (BASE + LSR) as *const u32;
            // Bounded THRE wait: if BASE is wrong, or the AON UART is held in reset (LSR reads 0x00
            // so THRE never sets), we must NOT hang boot — give up after the bound; fbcon still
            // carries the byte. This bound is load-bearing precisely because BASE is unverified.
            let mut spins: u32 = 0;
            while (core::ptr::read_volatile(lsr) & LSR_THRE) == 0 {
                spins += 1;
                if spins > 1_000_000 {
                    break;
                }
                core::hint::spin_loop();
            }
            core::ptr::write_volatile(thr, byte as u32);
        }
    }

    pub fn read_byte() -> Option<u8> {
        unsafe {
            let rbr = BASE as *const u32;
            let lsr = (BASE + LSR) as *const u32;
            let status = core::ptr::read_volatile(lsr);
            // Open-bus guard — the read-side counterpart to write_byte's bounded TX wait, and as
            // load-bearing here because BASE is unverified. If BASE is wrong, or the AON UART is
            // held in reset / firewalled off CCPLEX, MMIO reads return all-ones (0xFFFF_FFFF).
            // LSR_DR is bit 0, so an unguarded test would see "data ready" forever and inject
            // phantom 0xFF bytes into the shell on every poll. Treat all-ones as "no UART, no data".
            // (Telltale on the board: spurious/garbage serial input ⇒ suspect a wrong BASE.)
            if status == 0xFFFF_FFFF {
                return None;
            }
            if (status & LSR_DR) != 0 {
                Some((core::ptr::read_volatile(rbr) & 0xFF) as u8)
            } else {
                None
            }
        }
    }
}

pub struct SerialPort;

impl SerialPort {
    pub fn new() -> SerialPort {
        SerialPort
    }

    #[cfg(not(feature = "tegra"))]
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

    #[cfg(feature = "tegra")]
    pub fn write_byte(&self, byte: u8) {
        tegra::write_byte(byte);
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

/// WEDGE-2 — the aarch64 half of the breadcrumb seam: one byte at the UART, **taking no lock**.
///
/// `SerialPort::write_byte` is already exactly that — a bounded volatile poll of the PL011 `FR`
/// TX-full bit (or the Tegra NS16550 `LSR`) followed by one volatile store to the data register. It
/// is a method on a unit struct, so calling it does NOT require `SERIAL_PORT`, and nothing in
/// `crate::wedge2`'s call chain acquires `SERIAL_PORT`, `FBCON`, `WRITER`, `TABLE`, `SPRITE` or the
/// allocator. That is the whole property WEDGE-2 needs: every one of those locks is reachable from
/// the focus chain being instrumented, so a breadcrumb that could block on one would be missing in
/// precisely the runs it exists for. The cost is that a token may interleave with another core's
/// in-progress `serial_println!` line; see `crate::wedge2` for why that is the right trade for a
/// last-words instrument.
///
/// The x86 tree inherits WEDGE-2 by porting this one function (`arch/x86_64/serial.rs`) — everything
/// above it is arch-neutral.
#[cfg(feature = "wedge2")]
#[inline(never)]
pub fn wedge2_raw_byte(byte: u8) {
    SerialPort.write_byte(byte);
}

/// Free-function raw writer for `serial_ring::drain`'s sink — the PL011/Tegra counterpart of the x86
/// `raw_write_str`. `SerialPort::write_byte` is a method on a unit struct, so this acquires NOTHING
/// (not `SERIAL_PORT`, not `FBCON`, not the allocator) and its TX wait is bounded.
pub fn raw_write_str(s: &str) {
    for b in s.bytes() {
        SerialPort.write_byte(b);
    }
}

#[doc(hidden)]
pub fn _print(args: fmt::Arguments) {
    crate::serial_ring::note_submitted();

    // PANIC ESCAPE HATCH — the aarch64 half, identical in shape to x86. This path is the reason the
    // blocking `.lock()` below had to go: this arch did NOT share x86's silent-drop defect (a blocking
    // acquire loses nothing), but it carried the complementary one — a panic or abort that struck
    // mid-print, on the core already holding `SERIAL_PORT`, would spin on that lock FOREVER and the
    // machine would die with no message at all. Silence either way, and the same cure: past
    // `enter_panic_mode` no lock is touched, the staged backlog and then the panic text go straight
    // at the UART through the bounded lock-free primitive, synchronously.
    if crate::serial_ring::in_panic_mode() {
        crate::serial_ring::drain(raw_write_str);
        let mut raw = SerialPort;
        let _ = raw.write_fmt(args);
        crate::serial_ring::note_emitted();
        crate::video::fbcon::_print(args);
        crate::selftest::capture(args);
        return;
    }

    // Guard the serial lock with interrupts masked (matching x86) so an interrupt handler that
    // logs can't deadlock against an in-progress print holding the same lock.
    crate::arch::without_interrupts(|| {
        // Best-effort, never panic (write_byte is a no-op on the `pi` build). `try_lock` + defer
        // rather than the old blocking `lock()`: same shared discipline as x86 so there is ONE serial
        // transport to reason about across both arches, and — the part that matters on this arch — a
        // fault handler or an exception-level print can no longer stall behind another core's line.
        // Nothing is dropped: a contended line goes into the lock-free staging ring and the next
        // holder emits it intact, in order. See `crate::serial_ring`.
        if let Some(mut guard) = SERIAL_PORT.try_lock() {
            {
                let mut sink = |s: &str| {
                    #[cfg(feature = "logts")]
                    {
                        let _ = crate::logts::PrefixWriter { inner: &mut *guard }.write_str(s);
                    }
                    #[cfg(not(feature = "logts"))]
                    {
                        let _ = guard.write_str(s);
                    }
                };
                crate::serial_ring::drain(&mut sink);
            }
            // CLOCK-2: with `logts`, prefix each serial LINE with a compact timestamp (monotonic ms →
            // UTC after a civil anchor exists). Only the UART byte-stream is touched; the fbcon +
            // capture-ring mirrors below still receive the raw `args`. Feature OFF => byte-identical.
            #[cfg(feature = "logts")]
            {
                let _ = crate::logts::PrefixWriter { inner: &mut *guard }.write_fmt(args);
            }
            #[cfg(not(feature = "logts"))]
            {
                let _ = guard.write_fmt(args);
            }
            crate::serial_ring::note_emitted();
        } else {
            crate::serial_ring::stage(args);
        }
    });
    // Mirror to the framebuffer console (visible without a serial port). fbcon self-guards.
    crate::video::fbcon::_print(args);
    // TSTE-1 M2b: capture boot-fixture verdict lines (`-> PASS`/`-> FAIL`) into the selftest ring so
    // `tste` can replay them. Additive, alloc-free, `try_lock` only; safe from this IRQ-masked
    // context; zero change to what is printed above.
    crate::selftest::capture(args);
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
    #[cfg(not(feature = "tegra"))]
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

    #[cfg(feature = "tegra")]
    pub fn read_byte(&self) -> Option<u8> {
        tegra::read_byte()
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
