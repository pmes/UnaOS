// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Local APIC (xAPIC) driver.
//
// The local APIC MMIO window lives at the architectural default 0xFEE00000 and is
// identity-mapped by UEFI (like the xHCI BAR and the framebuffer — the bootloader uses
// physical_memory_offset 0), so every access here is a plain `read_volatile` /
// `write_volatile`: no page-table work and no MSRs. QEMU hardware-enables the APIC at
// reset; we only need the software-enable (SVR bit 8). x2APIC / IA32_APIC_BASE MSR work is
// deferred to SMP bring-up.

use core::sync::atomic::{AtomicU64, Ordering};

/// Physical (== virtual, identity-mapped) base of the local APIC register block.
const LAPIC_BASE: usize = 0xFEE0_0000;

// Register offsets (Intel SDM Vol.3, Table "Local APIC Register Address Map").
const REG_ID: usize = 0x020; // Local APIC ID (id in bits 31:24)
const REG_EOI: usize = 0x0B0; // End Of Interrupt (write 0)
const REG_SVR: usize = 0x0F0; // Spurious Interrupt Vector Register
const REG_LVT_TIMER: usize = 0x320; // LVT Timer entry
const REG_LVT_LINT0: usize = 0x350; // LVT LINT0 entry
const REG_LVT_LINT1: usize = 0x360; // LVT LINT1 entry
const REG_TIMER_INITCNT: usize = 0x380; // Timer Initial Count
const REG_TIMER_DIVIDE: usize = 0x3E0; // Timer Divide Configuration

/// Monotonic count of local-APIC timer ticks taken since boot. A periodic heartbeat / hlt
/// wake source (and the basis for a future scheduler tick / uptime).
pub static APIC_TICKS: AtomicU64 = AtomicU64::new(0);

#[inline]
unsafe fn read(reg: usize) -> u32 {
    core::ptr::read_volatile((LAPIC_BASE + reg) as *const u32)
}

#[inline]
unsafe fn write(reg: usize, val: u32) {
    core::ptr::write_volatile((LAPIC_BASE + reg) as *mut u32, val);
}

/// Software-enable the local APIC and configure its LVT entries, then arm the timer.
///
/// SVR: set bit 8 (APIC Software Enable) and the spurious vector to 0xFF (matches the
/// `spurious_handler` registered in the IDT). This is a pure local-APIC system with no 8259
/// PIC, so LINT0 is masked — it would only matter for legacy "virtual wire" ExtINT delivery,
/// which we don't use. LINT1 stays wired as NMI (a legitimate hardware signal, not legacy
/// ISA). The APIC timer is then armed as the system heartbeat.
pub fn init() {
    unsafe {
        let svr = read(REG_SVR);
        write(REG_SVR, svr | (1 << 8) | 0xFF);

        write(REG_LVT_LINT0, 1 << 16); // masked (no legacy PIC, so no ExtINT virtual wire)
        write(REG_LVT_LINT1, 0x400);   // delivery mode 100b = NMI, unmasked

        serial_println!(
            "APIC: Local APIC software-enabled (id={}, SVR={:#x}, LINT0=masked, LINT1=NMI).",
            apic_id(),
            read(REG_SVR)
        );
    }

    init_timer();
}

/// Start the local APIC timer in periodic mode at the IDT timer vector, as the system
/// heartbeat / hlt wake source (replacing the retired 8254 PIT). The legacy PIC is fully
/// masked in `interrupts::disable_legacy_pic`, so this vector is driven solely by the local
/// APIC and acknowledged with an APIC EOI.
///
/// The initial count is a fixed empirical value: the APIC timer counts at the (unknown,
/// per-machine) core-crystal frequency, so this gives some convenient rate on QEMU but NOT a
/// precise 100 Hz. Exact timing is not needed yet (the tick is only a wake source). Real
/// hardware will need calibration (CPUID leaf 0x15 / TSC-deadline) — deferred to the SMP/
/// scheduler work.
pub fn init_timer() {
    let vector = crate::arch::interrupts::TIMER_VECTOR as u32;
    unsafe {
        // Divide the input clock by 16 (encoding 0b0011).
        write(REG_TIMER_DIVIDE, 0x3);
        // LVT Timer: vector = TIMER_VECTOR, bit 17 = periodic mode, bit 16 (mask) = 0.
        write(REG_LVT_TIMER, vector | (1 << 17));
        // Writing the initial count (last) arms the countdown; it reloads each period.
        write(REG_TIMER_INITCNT, 50_000);
    }
    serial_println!("APIC: timer armed (vector {:#x}, periodic, ÷16).", vector);
}

/// Number of local-APIC timer ticks since boot.
#[inline]
pub fn ticks() -> u64 {
    APIC_TICKS.load(Ordering::Relaxed)
}

/// Signal End-Of-Interrupt to the local APIC. Called from every APIC-delivered handler
/// (the timer and the xHCI MSI-X interrupter) — but NOT from the spurious handler. Lock-free:
/// a single MMIO write, safe from interrupt context.
#[inline]
pub fn eoi() {
    unsafe { write(REG_EOI, 0) };
}

/// Local APIC ID of the current (boot) processor. Used as the MSI-X destination field.
pub fn apic_id() -> u8 {
    unsafe { (read(REG_ID) >> 24) as u8 }
}
