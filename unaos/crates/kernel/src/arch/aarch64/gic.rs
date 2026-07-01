// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// GICv2 driver — the ARM Generic Interrupt Controller, the analogue of the x86 local APIC + I/O
// APIC. Both the QEMU `virt` board (GIC-400-compatible model, forced to gic-version=2) and the
// real Raspberry Pi 4 (BCM2711 GIC-400) implement GICv2, so one driver serves both; only the MMIO
// base addresses differ, selected by the `pi` build feature (the same switch the serial uses).
//
// A GICv2 has two register banks:
//   * the Distributor (GICD) — global: routes/prioritises interrupts, holds the enable/group bits.
//   * the CPU interface (GICC) — per-core: acknowledge (IAR) and end-of-interrupt (EOIR), plus the
//     running-priority mask (PMR).
//
// Interrupt IDs: SGIs 0-15 (software), PPIs 16-31 (per-core private — the generic timer lives here,
// INTID 30), SPIs 32+ (shared peripherals). The timer tick is a PPI, so it needs no SPI routing
// (ITARGETSR), only a per-core enable in ISENABLER0.

// --- MMIO bases (QEMU `virt` vs BCM2711 GIC-400), selected at build time like the UART. ---
#[cfg(not(feature = "pi"))]
const GICD_BASE: usize = 0x0800_0000; // QEMU virt distributor (VIRT_GIC_DIST)
#[cfg(not(feature = "pi"))]
const GICC_BASE: usize = 0x0801_0000; // QEMU virt CPU interface (VIRT_GIC_CPU)

#[cfg(feature = "pi")]
const GICD_BASE: usize = 0xFF84_1000; // BCM2711 GIC-400 distributor
#[cfg(feature = "pi")]
const GICC_BASE: usize = 0xFF84_2000; // BCM2711 GIC-400 CPU interface

// Distributor register offsets (GICv2 spec).
const GICD_CTLR: usize = 0x000; // control (enable groups)
const GICD_TYPER: usize = 0x004; // type: ITLinesNumber etc.
#[cfg_attr(not(feature = "pi"), allow(dead_code))] // group reassignment is Pi-only (see enable_ppi)
const GICD_IGROUPR: usize = 0x080; // group select, 1 bit / INTID (32 per word)
const GICD_ISENABLER: usize = 0x100; // set-enable, 1 bit / INTID (32 per word)
const GICD_IPRIORITYR: usize = 0x400; // priority, 1 byte / INTID
#[allow(dead_code)] // reserved for SPI edge/level config when shared peripherals are wired
const GICD_ICFGR: usize = 0xC00; // config (edge/level), 2 bits / INTID
const GICD_SGIR: usize = 0xF00; // software-generated-interrupt trigger (write-only)

// CPU interface register offsets.
const GICC_CTLR: usize = 0x00; // control (enable signalling)
const GICC_PMR: usize = 0x04; // priority mask (threshold)
const GICC_BPR: usize = 0x08; // binary point
const GICC_IAR: usize = 0x0C; // interrupt acknowledge (read => INTID)
const GICC_EOIR: usize = 0x10; // end of interrupt (write the acked value back)

/// Special INTIDs 1020-1023 (1023 = spurious) are never real interrupts and take no EOI.
const SPURIOUS_FLOOR: u32 = 1020;

#[inline]
unsafe fn gicd_read(off: usize) -> u32 {
    core::ptr::read_volatile((GICD_BASE + off) as *const u32)
}
#[inline]
unsafe fn gicd_write(off: usize, val: u32) {
    core::ptr::write_volatile((GICD_BASE + off) as *mut u32, val);
}
#[inline]
unsafe fn gicc_read(off: usize) -> u32 {
    core::ptr::read_volatile((GICC_BASE + off) as *const u32)
}
#[inline]
unsafe fn gicc_write(off: usize, val: u32) {
    core::ptr::write_volatile((GICC_BASE + off) as *mut u32, val);
}

/// Bring up the GICv2 on the boot core: the (global) distributor, then this core's CPU interface.
/// Per-interrupt enabling happens later (`enable_ppi`/`enable_sgi`), once each source is configured.
/// Secondary cores skip the distributor and call `init_cpu_interface` alone.
pub fn init() {
    let num_intids = init_distributor();
    init_cpu_interface();
    serial_println!(
        ":: AARCH64 GICv2 init (GICD={:#x}, GICC={:#x}, {} INTIDs) ::",
        GICD_BASE, GICC_BASE, num_intids
    );
}

/// Enable the distributor — GLOBAL state, done once by the BSP. Returns the max INTID range for the
/// boot log. The Security-Extensions group model is the QEMU-vs-Pi gotcha (see `enable_ppi`): either
/// way GICD_CTLR bit0 = enable (Group 0 on QEMU / EnableGrp1 in the Non-secure alias on the Pi).
fn init_distributor() -> u32 {
    unsafe {
        let typer = gicd_read(GICD_TYPER);
        // (ITLinesNumber+1)*32 = the max INTID range the distributor supports — informational only.
        let num_intids = ((typer & 0x1F) + 1) * 32;
        gicd_write(GICD_CTLR, 0);
        gicd_write(GICD_CTLR, 0x1);
        num_intids
    }
}

/// Open THIS core's CPU interface (GICC) — banked per-core, so every core (BSP and each AP) must run
/// it before it can take interrupts. Lowest priority threshold (0xFF passes everything), no
/// sub-priority grouping, then enable signalling as IRQ (FIQ-enable left 0).
pub fn init_cpu_interface() {
    unsafe {
        gicc_write(GICC_PMR, 0xFF);
        gicc_write(GICC_BPR, 0x0);
        gicc_write(GICC_CTLR, 0x1);
    }
}

/// Enable a banked interrupt (SGI 0-15 or PPI 16-31) on THIS core: (on the Pi) move it to Group 1,
/// give it the highest priority, and set its enable bit. INTIDs < 32 are banked per-core, so
/// ISENABLER0 / the low IGROUPR/IPRIORITYR words address *this* core's copy — each core must call
/// this for the sources it wants.
fn enable_banked(intid: u32) {
    unsafe {
        // On the Pi's security-extensions GIC accessed from Non-secure, the interrupt must be
        // Group 1 to be deliverable; on QEMU's no-security GIC it stays in its default Group 0
        // (touching IGROUPR there is unnecessary and, paired with a Group-0-only enable, would mask
        // it). So only the Pi build reassigns the group. SGIs and PPIs share IGROUPR0 (bits 0-31).
        #[cfg(feature = "pi")]
        {
            let grp = gicd_read(GICD_IGROUPR);
            gicd_write(GICD_IGROUPR, grp | (1 << intid));
        }
        // Highest priority (0x00). IPRIORITYR is byte-addressable, one byte per INTID.
        core::ptr::write_volatile((GICD_BASE + GICD_IPRIORITYR + intid as usize) as *mut u8, 0x00);
        // Set-enable: write-1-to-set, other bits unaffected.
        gicd_write(GICD_ISENABLER, 1 << intid);
    }
}

/// Enable a private peripheral interrupt (PPI, INTID 16-31) on this core — e.g. the generic timer.
pub fn enable_ppi(intid: u32) {
    debug_assert!((16..32).contains(&intid), "enable_ppi expects a PPI (16-31)");
    enable_banked(intid);
}

/// Enable a software-generated interrupt (SGI, INTID 0-15) on this core — the IPI channel. Every
/// core that should RECEIVE this SGI must enable it (banked).
pub fn enable_sgi(intid: u32) {
    debug_assert!(intid < 16, "enable_sgi expects an SGI (0-15)");
    enable_banked(intid);
}

/// Send SGI `intid` (0-15) to core `target_cpu` (its GIC CPU-interface number, which equals the
/// core index on the Pi 4). The `DSB` publishes any memory the target will read on wake before the
/// interrupt is raised. TargetListFilter=0b00 => use the CPUTargetList bitmask in bits[23:16].
pub fn send_sgi(target_cpu: usize, intid: u32) {
    unsafe {
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));
        let val = ((1u32 << target_cpu) << 16) | (intid & 0xF);
        gicd_write(GICD_SGIR, val);
    }
}

/// Read the set-pending bit for INTID 16-31 (GICD_ISPENDR0). Diagnostic only — tells whether the
/// distributor sees a PPI pending (separates "timer not asserting" from "GIC not delivering").
pub fn ppi_pending(intid: u32) -> bool {
    const GICD_ISPENDR: usize = 0x200;
    unsafe { gicd_read(GICD_ISPENDR) & (1 << intid) != 0 }
}

/// Acknowledge, dispatch, and end one interrupt at the CPU interface. Called from the IRQ vector
/// stub with IRQs masked. Reading IAR acknowledges (and returns the INTID); writing EOIR the same
/// value completes it. Spurious reads (INTID >= 1020) take no EOI.
pub fn handle_irq() {
    let iar = unsafe { gicc_read(GICC_IAR) };
    let intid = iar & 0x3FF;
    if intid >= SPURIOUS_FLOOR {
        return; // spurious / special — no EOI
    }

    if intid < 16 {
        // SGI (inter-processor interrupt). INTID = the IPI channel, bits[12:10] = source core.
        // For now the only IPI is a wake/reschedule ping, which just needs to have interrupted the
        // idle WFE — count it (per-core) as proof of cross-core delivery. M3 hangs the scheduler
        // reschedule off this.
        crate::arch::percpu::count_ipi();
    } else if intid == crate::arch::timer::TIMER_INTID {
        // The handler re-arms the timer (clearing its level-sensitive line) before we EOI.
        crate::arch::timer::on_tick();
    }

    // EOI with the full acked value (preserves the source-CPU field for SGIs; a no-op for PPIs).
    unsafe { gicc_write(GICC_EOIR, iar) };
}
