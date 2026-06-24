// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Local APIC driver — xAPIC (MMIO) and x2APIC (MSR), chosen at runtime.
//
// Modern CPUs expose the local APIC two ways. The legacy xAPIC interface is a 4 KiB MMIO window
// at the architectural default 0xFEE00000 (identity-mapped by UEFI, like the xHCI BAR and the
// framebuffer — physical_memory_offset 0). The x2APIC interface replaces that window with a bank
// of MSRs: no MMIO, 32-bit APIC ids (so >255 CPUs), and a single-write ICR (no hi/lo split, no
// delivery-status poll) — what a real multi-core machine wants. We detect x2APIC via CPUID and
// drive whichever the CPU has behind one API (`init`/`eoi`/`apic_id`/`init_timer`/`send_ipi`),
// keeping the xAPIC path as a fallback for older hardware.
//
// Register *layouts* (SVR, the LVT entries, the timer) are identical across the two; only the
// access *method* differs, so `lapic_read`/`lapic_write` take both the MMIO offset and the MSR
// index and pick at runtime. EOI and the ICR are the only places the semantics differ.

use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use x86_64::registers::model_specific::Msr;

/// Physical (== virtual, identity-mapped) base of the xAPIC register block.
const LAPIC_BASE: usize = 0xFEE0_0000;

// xAPIC MMIO register offsets (Intel SDM Vol.3, "Local APIC Register Address Map").
const REG_ID: usize = 0x020; // Local APIC ID (id in bits 31:24)
const REG_EOI: usize = 0x0B0; // End Of Interrupt (write 0)
const REG_SVR: usize = 0x0F0; // Spurious Interrupt Vector Register
const REG_ICR_LOW: usize = 0x300; // Interrupt Command Register, low dword (writing it sends)
const REG_ICR_HIGH: usize = 0x310; // Interrupt Command Register, high dword (destination)
const REG_LVT_TIMER: usize = 0x320; // LVT Timer entry
const REG_LVT_LINT0: usize = 0x350; // LVT LINT0 entry
const REG_LVT_LINT1: usize = 0x360; // LVT LINT1 entry
const REG_TIMER_INITCNT: usize = 0x380; // Timer Initial Count
const REG_TIMER_DIVIDE: usize = 0x3E0; // Timer Divide Configuration

// x2APIC MSR indices (the same registers, addressed as MSRs).
const IA32_APIC_BASE: u32 = 0x1B; // bit 10 = x2APIC enable (EXTD), bit 11 = global enable (EN)
const APIC_BASE_ENABLE: u64 = 1 << 11;
const APIC_BASE_X2APIC: u64 = 1 << 10;
const X2_ID: u32 = 0x802; // full 32-bit APIC id (no >>24)
const X2_EOI: u32 = 0x80B;
const X2_SVR: u32 = 0x80F;
const X2_ICR: u32 = 0x830; // single 64-bit write: dest in bits 63:32, command in bits 31:0
const X2_LVT_TIMER: u32 = 0x832;
const X2_LVT_LINT0: u32 = 0x835;
const X2_LVT_LINT1: u32 = 0x836;
const X2_TIMER_INITCNT: u32 = 0x838;
const X2_TIMER_DIVIDE: u32 = 0x83E;

/// True once any CPU has switched to x2APIC. Set on the BSP in `init`; the APs read it (in their
/// own `init`) to enable x2APIC in their own MSR. A `Relaxed` `AtomicBool` is lock-free and safe
/// from interrupt context (the hot path `eoi` reads it).
static X2APIC: AtomicBool = AtomicBool::new(false);

/// Monotonic count of local-APIC timer ticks since boot. A periodic heartbeat / hlt wake source
/// (and the basis for a future scheduler tick / uptime).
pub static APIC_TICKS: AtomicU64 = AtomicU64::new(0);

#[inline]
fn x2apic() -> bool {
    X2APIC.load(Ordering::Relaxed)
}

#[inline]
unsafe fn mmio_read(reg: usize) -> u32 {
    core::ptr::read_volatile((LAPIC_BASE + reg) as *const u32)
}

#[inline]
unsafe fn mmio_write(reg: usize, val: u32) {
    core::ptr::write_volatile((LAPIC_BASE + reg) as *mut u32, val);
}

/// Read a local-APIC register through whichever interface is active.
#[inline]
unsafe fn lapic_read(mmio_off: usize, msr: u32) -> u32 {
    if x2apic() {
        Msr::new(msr).read() as u32
    } else {
        mmio_read(mmio_off)
    }
}

/// Write a local-APIC register through whichever interface is active.
#[inline]
unsafe fn lapic_write(mmio_off: usize, msr: u32, val: u32) {
    if x2apic() {
        Msr::new(msr).write(val as u64);
    } else {
        mmio_write(mmio_off, val);
    }
}

/// Detect x2APIC (CPUID.01H:ECX bit 21) and, if present, switch this CPU's local APIC into
/// x2APIC mode via IA32_APIC_BASE. Sets the global `X2APIC` flag. Idempotent and per-CPU: the
/// BSP calls it first (latching the mode), each AP calls it to flip its own MSR.
fn enable_x2apic_if_supported() {
    let supported = core::arch::x86_64::__cpuid(1).ecx & (1 << 21) != 0;
    if !supported {
        return; // old hardware: stay on the xAPIC MMIO path
    }
    unsafe {
        // Legal transition is xAPIC(EN=1) -> x2APIC(EN=1,EXTD=1); you cannot jump straight from
        // APIC-disabled to x2APIC. QEMU boots with EN=1, but enable it first to be safe.
        let mut base = Msr::new(IA32_APIC_BASE).read();
        if base & APIC_BASE_ENABLE == 0 {
            base |= APIC_BASE_ENABLE;
            Msr::new(IA32_APIC_BASE).write(base);
        }
        base |= APIC_BASE_X2APIC;
        Msr::new(IA32_APIC_BASE).write(base);
    }
    X2APIC.store(true, Ordering::Relaxed);
}

/// Software-enable this CPU's local APIC and configure its LVT entries, then arm the timer.
///
/// Detect/enable x2APIC first (so every subsequent access uses the right interface). SVR: set
/// bit 8 (APIC Software Enable) and the spurious vector to 0xFF (matches the `spurious_handler`
/// in the IDT). Pure local-APIC system, no 8259, so LINT0 is masked (no legacy ExtINT virtual
/// wire); LINT1 stays wired as NMI (a real hardware signal, not legacy ISA). Then the timer.
///
/// Callable by both the BSP (early `arch::init`) and each AP (in `ap_main`) — each CPU has its
/// own local APIC, so this configures the calling CPU's.
pub fn init() {
    enable_x2apic_if_supported();
    unsafe {
        let svr = lapic_read(REG_SVR, X2_SVR);
        lapic_write(REG_SVR, X2_SVR, svr | (1 << 8) | 0xFF);
        lapic_write(REG_LVT_LINT0, X2_LVT_LINT0, 1 << 16); // masked
        lapic_write(REG_LVT_LINT1, X2_LVT_LINT1, 0x400); // delivery mode 100b = NMI, unmasked

        serial_println!(
            "APIC: {} software-enabled (id={}, SVR={:#x}, LINT0=masked, LINT1=NMI).",
            if x2apic() { "x2APIC" } else { "xAPIC" },
            apic_id_u32(),
            lapic_read(REG_SVR, X2_SVR)
        );
    }

    init_timer();
}

/// Start the local APIC timer in periodic mode at the IDT timer vector, as the system heartbeat
/// / hlt wake source (replacing the retired 8254 PIT).
///
/// The initial count is a fixed empirical value: the APIC timer counts at the (unknown,
/// per-machine) core-crystal frequency, so this gives some convenient rate on QEMU but NOT a
/// precise tick. Exact timing is not needed yet (the tick is only a wake source). Real hardware
/// will need calibration (CPUID leaf 0x15 / TSC-deadline) — deferred.
pub fn init_timer() {
    let vector = crate::arch::interrupts::TIMER_VECTOR as u32;
    unsafe {
        // Divide the input clock by 16 (encoding 0b0011).
        lapic_write(REG_TIMER_DIVIDE, X2_TIMER_DIVIDE, 0x3);
        // LVT Timer: vector, bit 17 = periodic mode, bit 16 (mask) = 0.
        lapic_write(REG_LVT_TIMER, X2_LVT_TIMER, vector | (1 << 17));
        // Writing the initial count (last) arms the countdown; it reloads each period.
        lapic_write(REG_TIMER_INITCNT, X2_TIMER_INITCNT, 50_000);
    }
    serial_println!("APIC: timer armed (vector {:#x}, periodic, ÷16).", vector);
}

/// Number of local-APIC timer ticks since boot.
#[inline]
pub fn ticks() -> u64 {
    APIC_TICKS.load(Ordering::Relaxed)
}

/// Signal End-Of-Interrupt to the local APIC. Called from every APIC-delivered handler (the
/// timer and the xHCI MSI-X interrupter) — but NOT from the spurious handler. Lock-free: a
/// single MMIO/MSR write, safe from interrupt context.
#[inline]
pub fn eoi() {
    unsafe { lapic_write(REG_EOI, X2_EOI, 0) };
}

/// Local APIC ID of the current CPU, truncated to 8 bits. Used as the MSI-X destination field
/// (the standard MSI address format only carries an 8-bit id, and we target the BSP, id 0).
pub fn apic_id() -> u8 {
    apic_id_u32() as u8
}

/// Full 32-bit local APIC ID of the current CPU (x2APIC ids can exceed 8 bits). Used for IPI
/// destinations and per-CPU identification.
pub fn apic_id_u32() -> u32 {
    unsafe {
        if x2apic() {
            Msr::new(X2_ID).read() as u32
        } else {
            mmio_read(REG_ID) >> 24
        }
    }
}

/// Send an inter-processor interrupt. `dest` is the destination APIC id; `icr_low` is the ICR
/// low-dword command (delivery mode, level, trigger, vector). Used for AP startup
/// (INIT-SIPI-SIPI) and, later, scheduler/TLB-shootdown IPIs.
///
/// x2APIC issues the whole thing as one 64-bit MSR write (no delivery-status poll exists). xAPIC
/// writes the destination into ICR-high, then the command into ICR-low (which triggers the
/// send), then spins on the delivery-status bit.
#[allow(dead_code)] // used from Phase D (AP startup) onward
pub fn send_ipi(dest: u32, icr_low: u32) {
    unsafe {
        if x2apic() {
            Msr::new(X2_ICR).write(((dest as u64) << 32) | icr_low as u64);
        } else {
            mmio_write(REG_ICR_HIGH, dest << 24);
            mmio_write(REG_ICR_LOW, icr_low);
            while mmio_read(REG_ICR_LOW) & (1 << 12) != 0 {
                core::hint::spin_loop();
            }
        }
    }
}
