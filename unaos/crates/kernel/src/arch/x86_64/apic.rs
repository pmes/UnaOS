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

/// Physical (== virtual, identity-mapped) base of the local APIC register block.
const LAPIC_BASE: usize = 0xFEE0_0000;

// Register offsets (Intel SDM Vol.3, Table "Local APIC Register Address Map").
const REG_ID: usize = 0x020; // Local APIC ID (id in bits 31:24)
const REG_EOI: usize = 0x0B0; // End Of Interrupt (write 0)
const REG_SVR: usize = 0x0F0; // Spurious Interrupt Vector Register
const REG_LVT_LINT0: usize = 0x350; // LVT LINT0 entry
const REG_LVT_LINT1: usize = 0x360; // LVT LINT1 entry

#[inline]
unsafe fn read(reg: usize) -> u32 {
    core::ptr::read_volatile((LAPIC_BASE + reg) as *const u32)
}

#[inline]
unsafe fn write(reg: usize, val: u32) {
    core::ptr::write_volatile((LAPIC_BASE + reg) as *mut u32, val);
}

/// Software-enable the local APIC and wire up the legacy-transition LVT entries.
///
/// SVR: set bit 8 (APIC Software Enable) and the spurious vector to 0xFF (matches the
/// `spurious_handler` registered in the IDT). Once the APIC is software-enabled the CPU no
/// longer takes 8259 interrupts via the bare INTR pin — they must arrive through LINT0. So
/// during the legacy-retirement transition we program LINT0 = ExtINT (delivers the PIC's
/// INTR, "virtual wire" mode, keeps the PIT timer + PS/2 keyboard alive) and LINT1 = NMI.
/// Both are masked off in Phase 3 once USB-HID + the APIC timer replace them.
pub fn init() {
    unsafe {
        let svr = read(REG_SVR);
        write(REG_SVR, svr | (1 << 8) | 0xFF);

        write(REG_LVT_LINT0, 0x700); // delivery mode 111b = ExtINT, unmasked
        write(REG_LVT_LINT1, 0x400); // delivery mode 100b = NMI, unmasked

        serial_println!(
            "APIC: Local APIC software-enabled (id={}, SVR={:#x}, LINT0=ExtINT, LINT1=NMI).",
            apic_id(),
            read(REG_SVR)
        );
    }
}

/// Signal End-Of-Interrupt to the local APIC. Called from every APIC-delivered handler
/// (MSI-X, and later the APIC timer) — but NOT from the spurious handler. Lock-free: a
/// single MMIO write, safe from interrupt context.
#[inline]
pub fn eoi() {
    unsafe { write(REG_EOI, 0) };
}

/// Local APIC ID of the current (boot) processor. Used as the MSI-X destination field.
pub fn apic_id() -> u8 {
    unsafe { (read(REG_ID) >> 24) as u8 }
}
