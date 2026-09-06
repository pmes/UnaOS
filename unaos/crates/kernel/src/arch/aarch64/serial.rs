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
    const BASE: usize = 0x0C28_0000; #[cfg(feature = "orinrx")] pub(super) fn base() -> usize { BASE } // SERIALRX (ORINRX) — cfg-erased accessor for the tail mod witness, so the knob-off build keeps the const byte-for-byte as declared. ⚠ LINE-NEUTRAL append (and note the measured limit: any byte changed in this LIB-crate file, even a comment, renames ThinLTO `.llvm.<hash>` symbol suffixes in kernel.elf`s .symtab/.strtab; the loaded image stays identical — see the Cargo `orinrx` comment).

    // 16550 registers with Tegra reg-shift = 2 → each logical register is 4 bytes apart and
    // accessed as a 32-bit word (meaningful data in the low 8 bits). THR/RBR at +0x00, LSR at
    // register index 5 → byte offset 5 << 2 = 0x14.
    const LSR: usize = 5 << 2;
    const LSR_THRE: u32 = 1 << 5; // transmit holding register empty (ok to write)
    const LSR_DR: u32 = 1 << 0; // receive data ready

    pub fn write_byte(byte: u8) { if super::tegra_guard::drop_pre_map() { return; } // DARKWIN-GUARD (tail mod)
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

    pub fn read_byte() -> Option<u8> { if !super::tegra_guard::ready() { return None; } // DARKWIN-GUARD (tail mod)
        unsafe {
            let rbr = BASE as *const u32;
            let lsr = (BASE + LSR) as *const u32;
            let status = core::ptr::read_volatile(lsr); #[cfg(feature = "orinrx")] super::serialrx::note_lsr(status); // SERIALRX (ORINRX) — capture the RAW word BEFORE the guards below swallow it (store only: this runs under SERIAL_PORT; the print is off-lock in the tail mod). ⚠ LINE-NEUTRAL append.
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

// ---- SERFIX: the input poll DECLINES a held port instead of blocking on it ----------------------
//
// `arch::poll_input` is the one RX read in the kernel, and it runs from the preemptible input task
// with interrupts ENABLED. Until this arc it took `SERIAL_PORT` with a BLOCKING acquire, which is
// the same shape INWEDGE found on the panel lock and for the same reason: a preemptible task that
// blocks on a raw spinlock, unmasked, on the core that also hosts the kernel's IRQ-context printer.
// Its safety was never structural — it rested on one fact about one counterparty, that `sys_write`
// (syscall.rs) holds the port only for a bounded IRQ-masked byte loop. That is a property of today's
// callers, not of the lock, and it silently obliges every future `SERIAL_PORT` holder to stay
// bounded; the moment one does not, the input core wedges behind it with no diagnostic.
//
// So the acquisition below is NON-BLOCKING. A refused poll degrades to "no byte this pass", which
// costs nothing real: serial input is POLLED, not edge-delivered — the byte stays in the PL011 RX
// FIFO (16 deep, plus the RX-timeout interrupt on the metal path) and the next pump pass reads it.
// A `while let Some(b) = poll_input()` drain loop simply ends one iteration early and re-enters on
// the next pass. Nothing is lost; at worst one poll interval of latency is added under contention
// that previously would have been an unbounded stall.
//
// The refusals are COUNTED, and the census is reported EDGE-TRIGGERED once per contention episode
// (see `serfix_witness`), matching the `[inwedge]` discipline: silent on every boot that never
// contended — which is every automated gate — and loud exactly once when a real storm begins.

/// SERFIX — polls that found `SERIAL_PORT` held elsewhere and declined it. Nonzero means the window
/// that would have blocked the input core was entered on this boot, and was walked away from.
static SERFIX_REFUSED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// SERFIX — polls that acquired the port. The denominator, so a refusal rate is readable rather than
/// an unanchored count.
static SERFIX_READ: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// SERFIX — true while inside a contention episode (a refusal has been witnessed and no poll has
/// acquired the port since). Set by the witness, cleared by the next successful acquire; it is what
/// makes the `[serfix]` line episode-edged instead of per-refusal.
static SERFIX_IN_EPISODE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// SERFIX — `(read, refused)`.
pub fn serfix_census() -> (u32, u32) {
    use core::sync::atomic::Ordering;
    (SERFIX_READ.load(Ordering::Relaxed), SERFIX_REFUSED.load(Ordering::Relaxed))
}

/// SERFIX — the `[serfix]` recurrence witness, emitted at the START of a contention episode: the
/// previous poll acquired the port and this one did not. Episode-edged rather than per-refusal
/// because `poll_input` runs in a tight pump loop — a line per refusal would turn the storm this
/// exists to reveal into a flood that hides it. A sustained storm therefore prints once, with the
/// running totals; a second line means the port was released and contended again.
///
/// Printing from the refusal path is safe by construction: `_print` itself is `try_lock` + staging
/// ring (see `_print` above), so the witness for a held port cannot block on that same held port —
/// the line goes into the lock-free ring and the next holder emits it intact, in order.
fn serfix_note_refused() {
    use core::sync::atomic::Ordering;
    let refused = SERFIX_REFUSED.fetch_add(1, Ordering::Relaxed) + 1;
    if !SERFIX_IN_EPISODE.swap(true, Ordering::Relaxed) {
        let read = SERFIX_READ.load(Ordering::Relaxed);
        serial_println!(
            "[serfix] port read={} refused={} — poll_input declined a held SERIAL_PORT instead of blocking on it (input core survived)",
            read, refused
        );
    }
}

/// SERFIX — the ONE `SERIAL_PORT` acquisition the input poll makes: non-blocking and counted.
/// `None` means either "no byte waiting" or "the port was held elsewhere"; both are the same thing
/// to the caller — nothing to read on this pass, try again on the next one.
///
/// This is what `arch::poll_input` calls; it is the whole of the input side's contract with the
/// port lock, so no future `SERIAL_PORT` holder can wedge the input core by running long.
pub fn poll_input_nonblocking() -> Option<u8> {
    use core::sync::atomic::Ordering;
    match SERIAL_PORT.try_lock() {
        Some(port) => {
            SERFIX_READ.fetch_add(1, Ordering::Relaxed);
            // Episode closed: the next refusal is a new one and gets its own witness line.
            SERFIX_IN_EPISODE.store(false, Ordering::Relaxed);
            port.read_byte()
        }
        None => {
            serfix_note_refused();
            None
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
// would self-deadlock same-core — SERFIX made poll_input's ACQUIRE non-blocking, which removes the
// symmetric hazard of poll_input stalling behind a holder, but the section it holds is still
// IRQ-unmasked, so the rule for the ISR is unchanged: it takes no console lock).

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

// ── DARKWIN-GUARD (orin 1, 2026-08-18) — tail-defined per the Location-shift convention ──────
// Between ExitBootServices and `mmu_tegra::init`, the kernel runs under the UEFI-handoff
// translation tables, which map RAM but NOT the Tegra device window (JM2 R4: the kernel's first
// UARTC read faulted there, caught by UEFI's still-resident ArmCpuDxe vectors — whose post-EBS
// reporting path is gone, so the observed outcome on the box is a silent stop at the logo).
// The trunk merge proved the class on metal: `kernel_main` step 0a2 (`video::init_edid`,
// unconditional on every arch) printed its witness line before `tegra_early_stop`, and the
// board died at the NVIDIA logo on every boot of 2026-08-18. This latch closes the UARTC class:
// bytes offered before `mark_mmio_ready()` (armed by `tegra_early_stop` immediately after
// `mmu_tegra::init` returns) are DROPPED and COUNTED, never written; the count is witnessed on
// the wire once the window closes. Byte-granularity by design — the content of a pre-map line
// is gone (fbcon/selftest mirrors still carry it), the COUNT is what survives to the wire.
// Everything lives in this tail mod, with one-line calls appended to the pre-existing
// `write_byte`/`read_byte` opening lines, so no pre-existing line shifts and the non-tegra
// images keep their panic-Location bytes (the main.rs "35-line block" lesson).
#[cfg(feature = "tegra")]
mod tegra_guard {
    use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    static MMIO_READY: AtomicBool = AtomicBool::new(false);
    static DROPPED_PRE_MAP: AtomicU32 = AtomicU32::new(0);

    /// Arm the UART: the Tegra device window is mapped and UARTC MMIO is safe. Called exactly
    /// once, by `tegra_early_stop`, the moment `mmu_tegra::init` returns.
    pub fn mark_mmio_ready() {
        MMIO_READY.store(true, Ordering::Release);
    }

    /// True once the device window is mapped (read-side gate: LSR is unmapped before that).
    pub fn ready() -> bool {
        MMIO_READY.load(Ordering::Acquire)
    }

    /// Write-side gate: returns true (and counts the byte) while the window is still dark.
    pub fn drop_pre_map() -> bool {
        if ready() {
            return false;
        }
        DROPPED_PRE_MAP.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// How many bytes the guard dropped before the device window was mapped — nonzero means
    /// some caller printed before `mmu_tegra::init`, exactly the class that hung the merged
    /// base on metal.
    pub fn dropped_pre_map() -> u32 {
        DROPPED_PRE_MAP.load(Ordering::Relaxed)
    }
}
// The module surface `tegra_early_stop` (the sole armer) and its witness line reach:
// `arch::serial::{mark_mmio_ready, dropped_pre_map}`.
#[cfg(feature = "tegra")]
pub use tegra_guard::{dropped_pre_map, mark_mmio_ready};

// ---- SERIAL-FOCUS: the shell's SERIAL INBOX (bare-metal Pi only) ---------------------------------
//
// THE BLOCKER THIS EXISTS TO REMOVE. Before this module the Pi's serial RX had exactly one
// destination: `main::input_service` read a byte with `poll_input()` and posted it as an
// `Event::Key` straight into `GUI_CHANNEL`. That is the SHELL's channel, which sounds right and is
// not, for two reasons that compound:
//
//   (1) `GUI_CHANNEL`'s only consumer is `render_service`, and `render_service` PARKS inside
//       `handle_key -> shell::dispatch_command` for the entire life of a foreground command — which
//       includes `run <elf>`, the call that registers an EL0 window's ASID through
//       `user_input_set_active` and thereby gives that window the keyboard. So in exactly the state
//       the campaign cares about — a focused EL0 window — the channel's consumer is asleep. The
//       first 64 serial bytes queue where nothing will read them; the 65th blocks the input task
//       inside `Channel::send` (a `Semaphore::wait` with NO deadline). The serial wire is then dead
//       for the rest of the program's run, and the input task with it. That unbounded wait on a
//       parked consumer is the freeze family, reached from the UART instead of from the compositor.
//
//   (2) A byte that reaches `pal::EVENT_QUEUE` at all — `pal::pump_and_poll`'s aarch64 arm used to
//       put it there — is INDISTINGUISHABLE from a decoded USB HID key by the time it meets the
//       `[uvug9]` routing decision in `main::pump_usb_into_gui`. With `user_input_active() != 0`
//       that decision hands the event to `route_input_to_active_el0()`, i.e. to the focused
//       window's per-process ring. The ruling that a focused EL0 window owns the keyboard is
//       CORRECT and stands untouched — but it was silently annexing the serial console with it.
//
// THE SPLIT IS BY SOURCE, AND IT IS MADE BY CONSTRUCTION RATHER THAN BY A PREDICATE. There is no
// `source` tag threaded through `pal::Event`, and deliberately so: a tag is a field every future
// router has to remember to test, and the one that forgets is a regression nobody sees until the
// bench. Instead the serial byte NEVER ENTERS the focus-routed pipeline at all. It is consumed
// BEFORE the focus decision, into this ring, whose only consumer is the shell's key path in
// `render_service`. USB HID keeps `EVENT_QUEUE` and every line of its focus routing exactly as it
// is — this module adds zero lines inside `pump_usb_into_gui`'s routing branches — so "the focused
// EL0 window owns the USB keyboard" and "serial always reaches the shell" are now two statements
// about two disjoint carriers, and neither can be broken by editing the other.
//
// BOUNDED, AND THE PRODUCER NEVER WAITS. `offer` is total: it takes the byte or refuses it and says
// so, in O(1), under a spin lock held across a single array store. It has no blocking edge anywhere,
// which is the property (1) above was missing. A storm of serial input therefore cannot jam
// `GUI_CHANNEL` — the storm does not travel on `GUI_CHANNEL` at all; at most ONE coalesced wake
// token does (see `main::serial_to_shell`).
//
// CAPACITY, with the arithmetic. 512 bytes. The shell's line editor consumes a byte per keystroke
// and a command line is ~80 columns, so this holds ~6 full command lines pasted back to back, or
// ~44 ms of continuous 115200 8N1 traffic (11520 B/s) with the consumer completely asleep. The
// consumer is only ever asleep for the duration of one foreground command, and the paste case is
// what a storm test actually does, so six lines is the honest working set and 512 is a round
// number above it. This is a NEW ring and is NOT `serial_ring::SLOT_LEN` (1536): that one is the
// SERWIT-2 output staging slot whose size is a measured worst case and is not to be grown casually.
// The two share nothing but the word "serial" and travel in opposite directions.
//
// OVERFLOW POLICY: DROP THE NEWEST, and count it. The alternative (evict the oldest) keeps the ring
// full of the most recent bytes but silently deletes the FRONT of whatever line was being typed,
// which the line editor then submits as a mangled command. Dropping the newest leaves what has been
// accepted as an exact, contiguous, in-order PREFIX of the arrival stream, so ordering within the
// serial stream is preserved for everything that is delivered, and the loss is at the tail where
// the operator can see it did not echo. `dropped` is on the census line for the same reason.
#[cfg(feature = "baremetal")]
pub mod shell_inbox {
    use core::sync::atomic::{AtomicU64, Ordering};
    use spin::Mutex;

    /// Ring capacity in bytes. See the module header for the arithmetic behind 512.
    pub const CAP: usize = 512;

    struct Inbox {
        buf: [u8; CAP],
        head: usize, // next byte to hand the shell
        len: usize,  // bytes held
    }

    static INBOX: Mutex<Inbox> = Mutex::new(Inbox { buf: [0; CAP], head: 0, len: 0 });

    /// Bytes this ring took off the wire on the serial console's behalf.
    static ACCEPTED: AtomicU64 = AtomicU64::new(0);
    /// Bytes handed to the shell's key path (`render_service`'s drain). `accepted - delivered - held`
    /// is always 0 on a healthy boot — a gap means a consumer took bytes by some other door.
    static DELIVERED: AtomicU64 = AtomicU64::new(0);
    /// Bytes refused because the ring was full. Non-zero means the shell was parked longer than 512
    /// bytes of traffic; it is a capacity reading, not a fault, and it is never silent.
    static DROPPED: AtomicU64 = AtomicU64::new(0);
    /// High-water mark of `len`, so a boot that never dropped still reports how close it came.
    static HIGH: AtomicU64 = AtomicU64::new(0);

    /// PRODUCER. Take one serial byte for the shell. Never blocks, never allocates, returns `false`
    /// when the ring is full (the byte is dropped and counted). Callable from any context that may
    /// take a spin lock — it is held across a single array store and two `usize` updates.
    pub fn offer(byte: u8) -> bool {
        crate::arch::without_interrupts(|| {
            let mut q = INBOX.lock();
            if q.len == CAP {
                drop(q);
                DROPPED.fetch_add(1, Ordering::Relaxed);
                return false;
            }
            let tail = (q.head + q.len) % CAP;
            q.buf[tail] = byte;
            q.len += 1;
            let len = q.len as u64;
            drop(q);
            ACCEPTED.fetch_add(1, Ordering::Relaxed);
            HIGH.fetch_max(len, Ordering::Relaxed);
            true
        })
    }

    /// CONSUMER. The next serial byte for the shell, in arrival order, or `None`. The shell's key
    /// path in `render_service` is the sole production consumer; the QEMU fixture is the other.
    pub fn take() -> Option<u8> {
        let b = crate::arch::without_interrupts(|| {
            let mut q = INBOX.lock();
            if q.len == 0 {
                return None;
            }
            let b = q.buf[q.head];
            q.head = (q.head + 1) % CAP;
            q.len -= 1;
            Some(b)
        });
        if b.is_some() {
            DELIVERED.fetch_add(1, Ordering::Relaxed);
        }
        b
    }

    /// Bytes currently held (the shell has not drained them yet).
    pub fn held() -> usize {
        crate::arch::without_interrupts(|| INBOX.lock().len)
    }

    /// Census for the `[serfocus]` witness: `(accepted, delivered, dropped, high_water)`.
    pub fn census() -> (u64, u64, u64, u64) {
        (
            ACCEPTED.load(Ordering::Relaxed),
            DELIVERED.load(Ordering::Relaxed),
            DROPPED.load(Ordering::Relaxed),
            HIGH.load(Ordering::Relaxed),
        )
    }

    /// Fixture aid — empty the ring and zero the counters so the QEMU witness measures its own
    /// window rather than inheriting whatever the boot has already carried. Not on any boot path.
    #[cfg(feature = "witness")]
    pub fn test_reset() {
        crate::arch::without_interrupts(|| {
            let mut q = INBOX.lock();
            q.head = 0;
            q.len = 0;
        });
        ACCEPTED.store(0, Ordering::Relaxed);
        DELIVERED.store(0, Ordering::Relaxed);
        DROPPED.store(0, Ordering::Relaxed);
        HIGH.store(0, Ordering::Relaxed);
    }
}

// ---- SERIALRX (ORINRX): the Orin's serial console RECEIVE path — `orinrx`, DEFAULT OFF ----------
//
// THE GAP. `tegra::read_byte` above (LSR.DR + RBR — the whole polled-RX contract of a 16550; there is
// no RX-enable bit to find) is compiled into every jetson image and reachable through
// `arch::poll_input` -> `poll_input_nonblocking`, yet the console path never calls it:
// `jd2_console_pump` (both phases), its `supstate` twin `jd2_supstate_phase2` and the headless
// `kbd_pump_body` all drain `pal::next_event()`, which is `pop_event()` alone — no MMIO. The only
// tegra callers of the poll were `pal::pump_and_poll` (vug, the selftest pager), which the console
// pump never enters. So the Orin has been output-only for the cheapest possible reason: routing.
//
// THE FIX is one statement, `drain()` below, appended to the xHCI poll block of each of those FOUR
// pumps: poll the UART and push each byte as `Event::Key` onto the same queue the xHCI HID decoder
// feeds, so the drain that follows sees a serial byte exactly as it sees a keystroke. Everything
// downstream — `handle_key`, the `:: tegra: JD2 — KEY` echo, `shell::dispatch_command` — is already
// source-agnostic. Same shape as `pump_and_poll`'s aarch64 arm in pal.rs.
//
// WHY A KNOB, DEFAULT OFF. `BASE` is marked TO VERIFY ON THE BOARD (header above): observed TX does
// not establish RX. If BASE is wrong, or the AON UART is not ours, the failure mode is PHANTOM BYTES
// injected into the shell on every poll — `read_byte`'s all-ones guard catches only the open-bus
// case. The knob keeps the shipped image byte-identical until a board has answered.
//
// TWO DIAGNOSTICS, because `read_byte` SWALLOWS the one that matters: it returns `None` both on an
// all-ones LSR and on no-data, so a silent negative would be undiagnosable.
//   (1) `note_lsr` captures the RAW LSR word of the FIRST poll that reached the port. It is called
//       from `read_byte` UNDER `SERIAL_PORT`, so it only stores; `witness_once` prints it off-lock
//       from the pump, once, and classifies it by the driver header's table:
//         0x0000_0000   RX-ZERO     AON UART held in reset / clock-gated -> needs a BPMP reset+clock call
//         0xFFFF_FFFF   RX-OPENBUS  open bus / wrong BASE                -> try a fallback UART
//         0xDEAD_xxxx   RX-DEAD     SoC decode/access-error sentinel     -> wrong BASE or a CCPLEX firewall
//         anything else RX-LIVE     the UART answers (0x60 = THRE|TEMT is the idle 16550 signature)
//   (2) `census` counts bytes delivered and prints `[serialrx] rx=` on the pump's EXISTING sweep
//       cadence (~250 ms ticks; every 4th = ~1 s, the `[orinrender] census` rate). No new timer; the
//       headless `kbd_pump_body` has no sweep and so carries the drain + witness only.
//
// The witness token is subsystem-named (`[serialrx]`), never board-named (Peter, 2026-09-03).
// Tail module on purpose: knob-off it is `#[cfg]`-erased and nothing below it exists to shift, so
// the file's line numbering — and every panic `Location` in it — is untouched.
#[cfg(all(feature = "tegra", feature = "orinrx"))]
pub mod serialrx {
    use core::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};

    /// Raw LSR word of the first poll that reached the port; meaningful once `LSR_SEEN`.
    static LSR_FIRST: AtomicU32 = AtomicU32::new(0);
    static LSR_SEEN: AtomicBool = AtomicBool::new(false);
    static LSR_PRINTED: AtomicBool = AtomicBool::new(false);
    /// Bytes `drain` delivered to the PAL queue.
    static RX: AtomicU64 = AtomicU64::new(0);
    static RX_AT_LAST_CENSUS: AtomicU64 = AtomicU64::new(0);
    static CENSUS_ARMED: AtomicBool = AtomicBool::new(false);
    static CENSUS_TICK: AtomicU64 = AtomicU64::new(0);
    /// Sweep ticks per census line: the pump sweeps every ~250 ms, so 4 = ~1 s.
    const CENSUS_PERIOD: u64 = 4;

    // RXDISCRIM (A16): the three discriminators for the 3-of-5 byte loss of render3b. Overrun
    // between polls is refuted by the census (~325k polls/s vs 87 µs/byte); the live models are a
    // stall-time overrun (the pump's own `KEY` echo under the port lock) vs a competing reader
    // (UARTC is the SPE/TCU combined-UART port — two readers on one RBR). An overrun SETS LSR.OVRF;
    // a competitor never does. FIFO mode qualifies either (a 16-deep FIFO would not overrun in a
    // 2.3 ms echo at 87 µs/byte). Both are READ-ONLY witnesses; no FCR/IER/MCR write anywhere.
    /// POLLS that observed bit 1 (OVRF, the 16550/Tegra `UART_LSR_0` overrun flag) set — a poll
    /// count, not an event count: N overruns inside one stall (the pump's ~2.3 ms `KEY` echo) show
    /// as ONE poll with the flag up, so this is a lower bound on events; the A16 verdict table only
    /// asks `ovrf > 0`. Words with any of bits 31..16 set are excluded (all-ones = open bus,
    /// `0xdead....` = SoC decode error — both have bit 1 set and are not overruns; Tegra's LSR uses
    /// bits 9..0 only). PANEL4 L2, 2026-09-06.
    static OVRF: AtomicU64 = AtomicU64::new(0);
    /// Raw IIR word of the one-shot read taken on the witness line.
    static IIR_FIRST: AtomicU32 = AtomicU32::new(0);
    /// 16550 LSR bit 1 = OVRF (overrun error): the RBR was overwritten before it was read.
    const LSR_OVRF: u32 = 1 << 1;
    /// 16550 IIR = register index 2, at the tegra mod's reg-shift-2 stride (`LSR = 5 << 2`, above).
    /// Read-only: IIR[7:6] = 0b11 reports FIFOs enabled (FCR.FIFOE latched); 0b00 = 16450 mode.
    const IIR: usize = 2 << 2;
    const IIR_FIFO_SHIFT: u32 = 6;

    /// Called by `tegra::read_byte` UNDER `SERIAL_PORT` with the raw LSR word, ahead of its guards.
    /// Store only — a print here would deadlock on the port the caller holds. First word wins.
    pub fn note_lsr(status: u32) {
        if !LSR_SEEN.load(Ordering::Acquire) {
            LSR_FIRST.store(status, Ordering::Relaxed);
            LSR_SEEN.store(true, Ordering::Release);
        }
        if (status & 0xFFFF_0000) == 0 && (status & LSR_OVRF) != 0 {
            OVRF.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// IIR[7:6] decoded: `on` = 0b11 (FIFOs enabled), `off` = 0b00 (16450 mode), else the raw pair.
    fn fifo_state(iir: u32) -> &'static str {
        match (iir >> IIR_FIFO_SHIFT) & 0b11 {
            0b11 => "on",
            0b00 => "off",
            0b01 => "odd(01)",
            _ => "odd(10)",
        }
    }

    /// The driver header's three-state table, applied to a raw LSR word.
    pub fn classify(lsr: u32) -> &'static str {
        if lsr == 0 {
            "RX-ZERO (AON UART held in reset / clock-gated — needs a BPMP reset-deassert + clock-ungate)"
        } else if lsr == 0xFFFF_FFFF {
            "RX-OPENBUS (open bus / wrong BASE — try a fallback UART)"
        } else if (lsr >> 16) == 0xDEAD {
            "RX-DEAD (SoC decode/access-error sentinel — wrong BASE or a CCPLEX firewall)"
        } else {
            "RX-LIVE (the UART answers; 0x60 = THRE|TEMT is the idle 16550 signature)"
        }
    }

    /// THE DRAIN — the one statement the console path lacked. Appended to the xHCI poll block of
    /// `jd2_console_pump` (phase 1 and phase 2), `jd2_supstate_phase2` and `kbd_pump_body`.
    pub fn drain() {
        // RXMERGE (A37): the RBR read POPS the byte out of the RX FIFO the SPE reads too, so a
        // parked source must not read AT ALL — read-and-discard would destroy the byte instead of
        // leaving it for the mailbox. `uartc_owns_rbr` is the whole arbitration; see the RXMERGE
        // block at the foot of this module.
        if uartc_owns_rbr() {
            while let Some(b) = crate::arch::poll_input() {
                deliver(SRC_UARTC, b);
            }
        } else {
            PARKED.fetch_add(1, Ordering::Relaxed);
            #[cfg(feature = "tcurx")] seed_lsr_parked();
        }
        #[cfg(feature = "tcurx")] mbox_drain(); witness_once();
    }

    /// One-shot: the raw LSR word of the first poll, printed off-lock from the pump.
    fn witness_once() {
        if !LSR_SEEN.load(Ordering::Acquire) || LSR_PRINTED.swap(true, Ordering::Relaxed) {
            return;
        }
        let lsr = LSR_FIRST.load(Ordering::Relaxed);
        // RXDISCRIM (A16): one read of IIR, off-lock, after a poll has already reached the port (so
        // the MMIO window is up — `LSR_SEEN` is set only from inside `read_byte`, past its guard).
        // Reading IIR is NOT side-effect-free for the port's co-owner: a 16550 IIR read acknowledges a
        // pending THRE interrupt, and the SPE/TCU may have armed IER on this shared UART. It is read
        // ONCE per boot (this function is one-shot) and the line says whether the read could have
        // eaten anything: IIR bit 0 = 1 means "no interrupt pending" (render4: 0xc1 → pending=0).
        // PANEL4 L3, 2026-09-06. Same stride arithmetic as `read_byte`'s `BASE + LSR`.
        let iir = unsafe { core::ptr::read_volatile((super::tegra::base() + IIR) as *const u32) };
        IIR_FIRST.store(iir, Ordering::Relaxed);
        serial_println!(
            "[serialrx] lsr={:#010x} base={:#010x} iir={:#04x} fifo={} pending={} -> {} (0x00000000 = AON UART held in reset/clock-gated, needs BPMP; 0xffffffff = open bus / wrong BASE; 0xdead.... = SoC decode error; else live)",
            lsr,
            super::tegra::base(),
            iir & 0xFF,
            fifo_state(iir),
            (iir & 1 == 0) as u8,
            classify(lsr)
        );
    }

    /// `[serialrx] rx=` on the pump's sweep cadence (every `CENSUS_PERIOD` ticks). `polls`/`refused`
    /// are the SERFIX counters: `polls=0` means the drain never reached the port at all.
    pub fn census(tick: u64) {
        if CENSUS_ARMED.swap(true, Ordering::Relaxed)
            && tick.wrapping_sub(CENSUS_TICK.load(Ordering::Relaxed)) < CENSUS_PERIOD
        {
            return;
        }
        CENSUS_TICK.store(tick, Ordering::Relaxed);
        let rx = RX.load(Ordering::Relaxed);
        let delta = rx.wrapping_sub(RX_AT_LAST_CENSUS.swap(rx, Ordering::Relaxed));
        let (polls, refused) = super::serfix_census();
        let lsr0 = LSR_FIRST.load(Ordering::Relaxed);
        let verdict = if !LSR_SEEN.load(Ordering::Acquire) {
            "RX-UNPOLLED (no poll has reached the port: SERIAL_PORT held every pass, or the MMIO window still dark)"
        } else {
            classify(lsr0)
        };
        // RXDISCRIM (A16): `ovrf=` is the running count of polls that saw LSR.OVRF — an overrun
        // sets it, a competing reader never does. Cumulative like `rx=`, so the burst window is
        // scored by the delta across it.
        let ovrf = OVRF.load(Ordering::Relaxed); #[cfg(feature = "tcurx")] serial_println!("[serialrx] rx={} (+{}) polls={} refused={} ovrf={} lsr0={:#010x} mbox={} -> {} (TCURX2: `mbox=` = bytes TAKEN from the TCU RX mailbox and consumed by write-back; `rx=` totals BOTH sources, UARTC RBR + mailbox)", rx, delta, polls, refused, ovrf, lsr0, crate::arch::hsp_tegra::rx_mbox_took(), verdict); #[cfg(not(feature = "tcurx"))]
        serial_println!(
            "[serialrx] rx={} (+{}) polls={} refused={} ovrf={} lsr0={:#010x} -> {}",
            rx, delta, polls, refused, ovrf, lsr0, verdict
        );
        // RXMERGE (A37): the source split and the two rule counters, on the same cadence.
        #[cfg(feature = "tcurx")]
        rxmerge_census();
    }

    // TCURX2 (orin 15, `tcurx` = tegra+orinrx+tcuprobe+tcurx, DEFAULT OFF) — the SECOND RX SOURCE.
    //
    // Rung 1 (`tcuprobe`, render6) proved where the CCPLEX's console input actually is: the burst
    // `tste\r` left UARTC with `s`,`t`,`\r` while the TCU RX mailbox sat at `raw=0x82006574`
    // (full=1, nbytes=2, data=[74 65 …]) — the two bytes UARTC lost, parked and never consumed
    // because the probe deliberately never writes. R19 says a failed-under-conditions path stays
    // open, so the RBR poll above is UNTOUCHED and this is an ADDITION: `drain` now pulls from both
    // sources into the same `Event::Key` queue and the same `RX` counter, with `mbox=` in the
    // census saying how many of those bytes came from the mailbox.
    //
    // Everything from here to the module's closing brace is a TAIL APPEND inside the tail module:
    // knob-off it is `#[cfg]`-erased and no line above it moved, so every `panic::Location` in this
    // file — and the Pi's `kernel8.img`, which compiles `serial.rs` with `tegra` off — is untouched.
    /// Drain the TCU RX mailbox: take WHOLE WORDS until it reports empty (RXBURST, A16 / orin 17),
    /// then push each byte as the same `Event::Key` the RBR loop pushes. The consume-by-write-back
    /// protocol and the ONLY register write the knob adds live in `arch::aarch64::hsp_tegra`.
    /// Called from `drain` OFF the `SERIAL_PORT` lock (the RBR `while` above has already released
    /// it), which is what makes the `[tcurx] took=` per-byte witness safe to print from in there.
    /// THE PRINTS MUST STAY OUT HERE, AFTER THE SLOT IS CLEAR: render8 lost 2 of 5 burst bytes
    /// because the old per-byte take printed ~160 characters — ~14 ms of polled 115200 transmit —
    /// with bit 31 still asserted, and the SPE had nowhere to post `e` and CR (hsp_tegra §RXBURST).
    #[cfg(feature = "tcurx")]
    fn mbox_drain() {
        let burst = crate::arch::hsp_tegra::rx_mbox_drain();
        for i in 0..burst.len() { burst.witness(i); deliver(SRC_MBOX, burst.byte(i)); }
    }

    // ═══ RXMERGE (A37, orin 16) — ONE OWNER, ONE ORDERED STREAM ══════════════════════════════════
    //
    // THE DEFECT, from render7 2026-09-06 (`~/unaos-bench/scratch/orin16/render7-boot1.log`, the
    // injector's own log beside it proving five bytes per leg):
    //
    //   BURST `tste` + CR — UARTC's RBR delivered `s`,`t`,CR (:1347-1349) and the mailbox delivered
    //   `t`,`e` SIXTEEN LINES LATER (:1363-1366), after the shell had already run the CR. Five bytes
    //   in, five keys out — exactly-once held — but the ORDER was `s t CR t e`, so the shell saw
    //   `cmd="st"` and the late `te` fell into the next line.
    //
    //   PACED, same five bytes — the mailbox carried ALL FIVE in order (`t`,`s`,`t`,`e` at :1498-1507
    //   then CR at :1521) while UARTC ALSO delivered the CR (:1508, no `[tcurx] took=` ahead of it).
    //   `keys=6` for five injected: `rx=11 (+6) … mbox=7`. THE SAME BYTE CAME DOWN BOTH TRANSPORTS.
    //
    // MECHANISM. UARTC is the SPE/TCU's combined-UART port (A16): the SPE reads its RBR and forwards
    // console RX into the HSP shared mailbox, which is the CCPLEX's actual console-input contract.
    // Our direct RBR poll is a SECOND reader on that one FIFO. Each RBR read pops an entry, so the
    // two readers normally split the stream (the burst leg: 3 to us, 2 to the SPE) — but the pop is
    // not atomic across two masters, so an overlapping pair of reads can both retire the same entry
    // (the paced CR). The two transports then have wildly different latencies — the RBR is delivered
    // in the pass that reads it, the mailbox whenever the SPE gets round to posting and we next
    // drain — so their interleave at `Event::Key` is arbitrary. Two readers, no shared sequence.
    //
    // WHY NOT A MERGE QUEUE. A merge "keyed by arrival" is what the code already did: both sources
    // push into one PAL queue in observation order, and that produced `cmd="st"`. There is NO
    // ordering tag on the wire — the mailbox word carries a byte count, not a sequence number — so
    // no consumer can reconstruct the send order from two unsequenced transports. Nor can duplicates
    // be filtered by value: a human typing `tt` is indistinguishable from one `t` mirrored, and a
    // value filter would silently eat the second keystroke. **Exactly-once and in-order therefore
    // require exactly ONE reader.** That is this block.
    //
    // WHICH ONE — the mailbox, on the paced leg's evidence: it carried 5 of 5 in order, unaided. The
    // burst leg's mailbox share was short (2 of 5) precisely because our RBR poll stole the other
    // three first; remove the thief and the SPE has the whole stream to forward. The RBR poll is
    // KEPT, not deleted (R19): it is the source whenever the mailbox never armed (no DTB resolution,
    // or a board where the TCU is not the console), which is also the `tcurx`-off image's behaviour,
    // unchanged.
    //
    // THE RULE IS STATED AS `const fn`s AND CHECKED BY `./arroyo check`. `#[cfg(test)] mod tests` is
    // dead code in this crate by construction — nothing runs `cargo test` on a `no_std` kernel and
    // `check` cannot see it (the reason `gui_watchdog.rs` and `drivers/gpu/kepler.rs` both removed
    // theirs). The `const _` block below is const-evaluated on every `arm-tegra-tcurx` leg of
    // `./arroyo check`, so a regression in the ordering/dedup rule is a BUILD FAILURE, and QEMU
    // models no Tegra234 so there is nothing for `test-arm` to exercise anyway.

    /// Source tags. Plain `u8` so the whole rule below is const-evaluable.
    const SRC_UARTC: u8 = 0;
    #[cfg(feature = "tcurx")]
    const SRC_MBOX: u8 = 1;

    /// Both readers deliver — the render7 behaviour, and the state before the mailbox arms.
    pub const POLICY_BOTH: u8 = 0;
    /// The mailbox owns RX outright; the UARTC RBR is not read at all.
    #[cfg(feature = "tcurx")]
    pub const POLICY_MBOX_ONLY: u8 = 1;

    /// THE ARBITRATION. An armed mailbox (rung 1 resolved the word from the live DTB) means the SPE
    /// forward is available, and it takes the console outright.
    #[cfg(feature = "tcurx")]
    pub const fn policy_for(mbox_armed: bool) -> u8 {
        if mbox_armed { POLICY_MBOX_ONLY } else { POLICY_BOTH }
    }

    /// May the direct UARTC RBR poll run? Only under `BOTH`. This is a "do not READ" rule, never a
    /// "read and drop" one: the read is what pops the byte away from the SPE.
    pub const fn polls_uartc(policy: u8) -> bool {
        policy == POLICY_BOTH
    }

    /// A cross-source handoff: the delivered stream just switched transports. This is render7's
    /// REORDER signature (the burst's `s t CR` from UARTC then `t e` from the mailbox). Under
    /// `MBOX_ONLY` it can never be true — which is the claim this arc makes.
    pub const fn is_handoff(prev_src: u8, src: u8, have_prev: bool) -> bool {
        have_prev && prev_src != src
    }

    /// A cross-source value repeat — render7's paced double-CR. A DETECTOR, NEVER A FILTER: no byte
    /// is ever dropped on its account. `prev_src != src` is what separates a transport duplicate
    /// from a legitimate repeat, because a human typing `tt` sends both bytes down ONE transport.
    pub const fn is_xdup(prev_src: u8, prev_byte: u8, src: u8, byte: u8, have_prev: bool) -> bool {
        have_prev && prev_src != src && prev_byte == byte
    }

    /// Replay a recorded `(source, byte)` wire feed under a policy: returns
    /// `(delivered, handoffs, xdups)`. A byte offered by a parked source is never delivered — on
    /// metal it is never popped from the FIFO either, so the SPE forwards it instead.
    pub const fn replay<const N: usize>(policy: u8, feed: &[(u8, u8); N]) -> (u32, u32, u32) {
        let (mut delivered, mut handoffs, mut xdups) = (0u32, 0u32, 0u32);
        let (mut prev_src, mut prev_byte, mut have_prev) = (0u8, 0u8, false);
        let mut i = 0;
        while i < N {
            let (src, byte) = feed[i];
            i += 1;
            if src == SRC_UARTC && !polls_uartc(policy) {
                continue;
            }
            if is_handoff(prev_src, src, have_prev) {
                handoffs += 1;
            }
            if is_xdup(prev_src, prev_byte, src, byte, have_prev) {
                xdups += 1;
            }
            prev_src = src;
            prev_byte = byte;
            have_prev = true;
            delivered += 1;
        }
        (delivered, handoffs, xdups)
    }

    // THE RULE, checked at build time on the two render7 legs as they were actually recorded.
    #[cfg(feature = "tcurx")]
    const _: () = {
        // Parking is a read ban, not a drop rule.
        assert!(polls_uartc(POLICY_BOTH));
        assert!(!polls_uartc(POLICY_MBOX_ONLY));
        assert!(policy_for(true) == POLICY_MBOX_ONLY);
        assert!(policy_for(false) == POLICY_BOTH);

        // render7 BURST as flown: UARTC s,t,CR then the mailbox's t,e sixteen lines later.
        // Exactly-once held (5 in, 5 out) but ONE cross-source handoff put them out of order —
        // the shell read `st`. The counters must SEE that; a counter that cannot fire is no gate.
        const BURST: [(u8, u8); 5] =
            [(SRC_UARTC, b's'), (SRC_UARTC, b't'), (SRC_UARTC, 0x0d), (SRC_MBOX, b't'), (SRC_MBOX, b'e')];
        assert!(matches!(replay(POLICY_BOTH, &BURST), (5, 1, 0)));

        // render7 PACED as flown: the mailbox carried all five in order and UARTC ALSO delivered the
        // CR. Six keys for five bytes, one handoff into the stray CR and one back out, one xdup.
        const PACED: [(u8, u8); 6] = [
            (SRC_MBOX, b't'),
            (SRC_MBOX, b's'),
            (SRC_MBOX, b't'),
            (SRC_MBOX, b'e'),
            (SRC_UARTC, 0x0d),
            (SRC_MBOX, 0x0d),
        ];
        assert!(matches!(replay(POLICY_BOTH, &PACED), (6, 2, 1)));
        // …and under the fix the same wire delivers the five injected bytes, in order, once each.
        assert!(matches!(replay(POLICY_MBOX_ONLY, &PACED), (5, 0, 0)));

        // A legitimate same-source repeat is NOT a duplicate — `tt` keeps both bytes.
        const REPEAT: [(u8, u8); 2] = [(SRC_MBOX, b't'), (SRC_MBOX, b't')];
        assert!(matches!(replay(POLICY_MBOX_ONLY, &REPEAT), (2, 0, 0)));
        // The burst wire under the fix: the three bytes UARTC stole are never popped by us at
        // all, so they stay in the FIFO for the SPE — what is DELIVERED here is the mailbox's own
        // two, in order, with no handoff and no duplicate.
        assert!(matches!(replay(POLICY_MBOX_ONLY, &BURST), (2, 0, 0)));
    };

    /// The policy in force (`POLICY_*`). Starts at `BOTH` and flips once the mailbox arms.
    #[cfg(feature = "tcurx")]
    static POLICY: AtomicU32 = AtomicU32::new(POLICY_BOTH as u32);
    /// Drain passes in which the UARTC RBR poll was PARKED because the mailbox owns RX.
    static PARKED: AtomicU64 = AtomicU64::new(0);
    /// Monotonic delivery index — every byte handed to the key path gets one, from either source.
    #[cfg(feature = "tcurx")]
    static SEQ: AtomicU64 = AtomicU64::new(0);
    /// Per-source delivered counts.
    #[cfg(feature = "tcurx")]
    static N_UARTC: AtomicU64 = AtomicU64::new(0);
    #[cfg(feature = "tcurx")]
    static N_MBOX: AtomicU64 = AtomicU64::new(0);
    /// `dup=` / `reorder=` — the two rule violations, counted, never acted on.
    #[cfg(feature = "tcurx")]
    static XDUP: AtomicU64 = AtomicU64::new(0);
    #[cfg(feature = "tcurx")]
    static HANDOFF: AtomicU64 = AtomicU64::new(0);
    /// Previous delivery packed as `have<<16 | src<<8 | byte`, so one relaxed load carries the
    /// whole predecessor the two rules need.
    #[cfg(feature = "tcurx")]
    static PREV: AtomicU32 = AtomicU32::new(0);
    #[cfg(feature = "tcurx")]
    const PREV_HAVE: u32 = 1 << 16;

    #[cfg(feature = "tcurx")]
    fn src_name(src: u8) -> &'static str {
        if src == SRC_UARTC { "uartc" } else { "mbox" }
    }

    #[cfg(feature = "tcurx")]
    fn policy_name(policy: u8) -> &'static str {
        if polls_uartc(policy) { "both" } else { "mbox-only" }
    }

    /// Arbitrate, once per drain pass. Announces the ONE transition it can make, off-lock.
    #[cfg(feature = "tcurx")]
    fn uartc_owns_rbr() -> bool {
        let want = policy_for(crate::arch::hsp_tegra::rx_mbox_armed());
        if POLICY.swap(want as u32, Ordering::Relaxed) != want as u32 {
            serial_println!(
                "[rxmerge] policy={} armed={} uartc-rbr={} -> A37: one owner, one ordered stream (the parked reader is NOT read — an RBR read pops the byte away from the SPE; `dup=`/`reorder=` on the census stay 0 while this holds, and `[serialrx] polls=0` becomes the CORRECT reading of a parked port — the LSR witness is taken directly instead, which does not pop the RBR)",
                policy_name(want),
                crate::arch::hsp_tegra::rx_mbox_armed() as u8,
                if polls_uartc(want) { "polled" } else { "parked" }
            );
        }
        polls_uartc(want)
    }

    /// Knob-off there is no second source, so the RBR always owns the port.
    #[cfg(not(feature = "tcurx"))]
    fn uartc_owns_rbr() -> bool {
        true
    }

    /// 16550 LSR at the tegra mod's reg-shift-2 stride, same arithmetic as `read_byte`'s `BASE + LSR`.
    #[cfg(feature = "tcurx")]
    const LSR_REG: usize = 5 << 2;

    /// KEEP A16's WITNESS ALIVE ON A PARKED PORT. `note_lsr` is fed from inside `read_byte`, which
    /// is precisely the call parking stops making — so without this a parked boot would print
    /// `polls=0 -> RX-UNPOLLED` with no LSR word at all and A16's `lsr=`/`iir=`/`fifo=` evidence
    /// would SILENTLY stop appearing, which is the one failure mode the serial-transport law
    /// forbids. An LSR read returns the LINE STATE and does not pop the RBR, so unlike a data read
    /// it cannot steal a byte from the SPE; it is taken ONCE per boot, off-lock, the same one-shot
    /// discipline and the same accepted side-effect class as the IIR read PANEL4 L3 sanctioned.
    /// NOTE for the scorer: with the port parked, `ovrf=` is at most 1 — it counts POLLS that saw
    /// the flag, and there is now exactly one such poll per boot instead of millions.
    #[cfg(feature = "tcurx")]
    fn seed_lsr_parked() {
        // DARKWIN-GUARD, the same read-side gate `read_byte` opens with: LSR is UNMAPPED until
        // `mmu_tegra::init` returns, so this must never be the read that touches a dark window.
        if LSR_SEEN.load(Ordering::Acquire) || !super::tegra_guard::ready() {
            return;
        }
        note_lsr(unsafe { core::ptr::read_volatile((super::tegra::base() + LSR_REG) as *const u32) });
    }

    /// THE ONE INTAKE. Every RX byte from every source reaches the key path through here and
    /// nowhere else, which is what makes `seq=` a total order over the console stream.
    #[cfg(feature = "tcurx")]
    fn deliver(src: u8, b: u8) {
        crate::pal::push_event(crate::pal::Event::Key(b));
        RX.fetch_add(1, Ordering::Relaxed);
        let prev = PREV.load(Ordering::Relaxed);
        let (have, psrc, pbyte) = (prev & PREV_HAVE != 0, ((prev >> 8) & 0xff) as u8, (prev & 0xff) as u8);
        if is_handoff(psrc, src, have) {
            HANDOFF.fetch_add(1, Ordering::Relaxed);
        }
        if is_xdup(psrc, pbyte, src, b, have) {
            XDUP.fetch_add(1, Ordering::Relaxed);
        }
        PREV.store(PREV_HAVE | ((src as u32) << 8) | b as u32, Ordering::Relaxed);
        if src == SRC_UARTC { &N_UARTC } else { &N_MBOX }.fetch_add(1, Ordering::Relaxed);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        serial_println!(
            "[rxmerge] src={} seq={} byte={:#04x} '{}' policy={} dup={} reorder={}",
            src_name(src),
            seq,
            b,
            if (0x20u8..0x7f).contains(&b) { b as char } else { '.' },
            policy_name(POLICY.load(Ordering::Relaxed) as u8),
            XDUP.load(Ordering::Relaxed),
            HANDOFF.load(Ordering::Relaxed)
        );
    }

    /// Knob-off: the byte-for-byte pre-RXMERGE intake, one source, no witness.
    #[cfg(not(feature = "tcurx"))]
    fn deliver(_src: u8, b: u8) {
        crate::pal::push_event(crate::pal::Event::Key(b));
        RX.fetch_add(1, Ordering::Relaxed);
    }

    /// The scoreable rollup, printed on the census cadence beside `[serialrx] rx=`.
    #[cfg(feature = "tcurx")]
    fn rxmerge_census() {
        let policy = POLICY.load(Ordering::Relaxed) as u8;
        let (dup, reorder) = (XDUP.load(Ordering::Relaxed), HANDOFF.load(Ordering::Relaxed));
        serial_println!(
            "[rxmerge] census policy={} seq={} uartc={} mbox={} dup={} reorder={} parked={} -> {}",
            policy_name(policy),
            SEQ.load(Ordering::Relaxed),
            N_UARTC.load(Ordering::Relaxed),
            N_MBOX.load(Ordering::Relaxed),
            dup,
            reorder,
            PARKED.load(Ordering::Relaxed),
            if dup == 0 && reorder == 0 {
                "SINGLE-SOURCE (exactly-once, in-order: every byte came down one transport). EXPECTED SIDE EFFECTS under policy=mbox-only: `[serialrx] polls=0` beside this line is CORRECT, not a dead drain — the UARTC RBR is never read; `parked=` is the drain-liveness counter that replaces `polls=`. A16's `lsr=`/`iir=`/`fifo=` witness still prints (one direct LSR read, which does not pop the RBR) but `ovrf=` is now at most 1, because it counts POLLS that saw the flag and there is exactly one per boot"
            } else {
                "SPLIT-SOURCE (A37 live: two readers are still both delivering — dup>0 is the same byte down both transports, reorder>0 is a cross-transport handoff in the delivered stream)"
            }
        );
    }
}
