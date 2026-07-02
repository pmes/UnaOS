// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// GIC driver — the ARM Generic Interrupt Controller, the analogue of the x86 local APIC + I/O
// APIC. This one file speaks BOTH major architectures behind a single public API:
//   * GICv2 — the QEMU `virt` board default (GIC-400-compatible, gic-version=2) and the real
//     Raspberry Pi 4 (BCM2711 GIC-400). Distributor (GICD) + memory-mapped CPU interface (GICC).
//   * GICv3 — QEMU `virt` with gic-version=3 (the JC1 arc; the Jetson Orin Nano's Tegra234 has a
//     GICv3 too, but Orin metal is a later operator-attended arc — everything here is QEMU-only).
//     Distributor (GICD, affinity-routed) + per-core Redistributor (GICR) + a *system-register*
//     CPU interface (ICC_*_EL1), not MMIO.
//
// The version is picked at `init` by reading ID_AA64PFR0_EL1.GIC (0 => v2, non-zero => v3 CPU
// interface) — a system-register probe, because the v3 PIDR2 MMIO offset (0xFFE8) is unmapped on a
// v2 distributor and faults there. Every public entry point
// (`enable_ppi`/`enable_sgi`/`enable_spi`/`send_sgi`/`handle_irq`/`ppi_pending`) then dispatches on
// it. The `pi` build hard-pins v2 at COMPILE time (the GIC-400 is never v3), so the
// v3 machinery and the detection are `#[cfg(not(feature = "pi"))]` — the Pi image is byte-identical
// to the pre-v3 driver, and every v3 addition is additive + dispatch-gated (this file is shared
// with the pi lane).
//
// A GICv2 has two register banks:
//   * the Distributor (GICD) — global: routes/prioritises interrupts, holds the enable/group bits.
//   * the CPU interface (GICC) — per-core: acknowledge (IAR) and end-of-interrupt (EOIR), plus the
//     running-priority mask (PMR).
// A GICv3 replaces the memory-mapped GICC with system registers (ICC_*_EL1) and moves the banked
// SGI/PPI state (enable/group/priority/pending) out of the GICD into each core's Redistributor.
//
// Interrupt IDs (both versions): SGIs 0-15 (software), PPIs 16-31 (per-core private — the generic
// timer lives here, INTID 30), SPIs 32+ (shared peripherals). The timer tick is a PPI, so it needs
// no SPI routing, only a per-core enable (in ISENABLER0 — GICD's on v2, the Redistributor's on v3).

// --- MMIO bases (QEMU `virt` vs BCM2711 GIC-400), selected at build time like the UART. ---
#[cfg(not(feature = "pi"))]
const GICD_BASE: usize = 0x0800_0000; // QEMU virt distributor (VIRT_GIC_DIST)
#[cfg(not(feature = "pi"))]
const GICC_BASE: usize = 0x0801_0000; // QEMU virt CPU interface (VIRT_GIC_CPU)

#[cfg(feature = "pi")]
const GICD_BASE: usize = 0xFF84_1000; // BCM2711 GIC-400 distributor
#[cfg(feature = "pi")]
const GICC_BASE: usize = 0xFF84_2000; // BCM2711 GIC-400 CPU interface

// --- GICv3 redistributor (QEMU `virt` only; the Pi GIC-400 is v2 and never reaches this code). ---
// The GICD lives at the same 0x0800_0000 on virt for both versions; v3 adds the per-core GICR bank.
#[cfg(not(feature = "pi"))]
const GICR_BASE: usize = 0x080A_0000; // QEMU virt redistributor base (VIRT_GIC_REDIST)
#[cfg(not(feature = "pi"))]
const GICR_STRIDE: usize = 0x2_0000; // per-CPU: the RD frame (0) + the SGI frame (0x1_0000)
#[cfg(not(feature = "pi"))]
const GICR_SGI_OFFSET: usize = 0x1_0000; // the SGI/PPI register frame within one redistributor

// Distributor register offsets (GICv2 spec).
const GICD_CTLR: usize = 0x000; // control (enable groups)
const GICD_TYPER: usize = 0x004; // type: ITLinesNumber etc.
#[cfg_attr(not(feature = "pi"), allow(dead_code))] // group reassignment is Pi-only (see enable_ppi)
const GICD_IGROUPR: usize = 0x080; // group select, 1 bit / INTID (32 per word)
const GICD_ISENABLER: usize = 0x100; // set-enable, 1 bit / INTID (32 per word)
const GICD_IPRIORITYR: usize = 0x400; // priority, 1 byte / INTID
const GICD_ITARGETSR: usize = 0x800; // SPI CPU-interface routing, 1 byte / INTID (SPIs only)
const GICD_ICFGR: usize = 0xC00; // config (edge/level), 2 bits / INTID
const GICD_SGIR: usize = 0xF00; // software-generated-interrupt trigger (write-only)

// GICv3-only distributor register (QEMU virt). Version detection uses ID_AA64PFR0_EL1.GIC, not an
// MMIO PIDR2 read — the v3 PIDR2 offset 0xFFE8 is unmapped on a v2 GICD and would fault there.
#[cfg(not(feature = "pi"))]
const GICD_IROUTER: usize = 0x6000; // per-SPI affinity routing (64-bit, 8 bytes/INTID; replaces ITARGETSR under ARE=1)

// GICv3 redistributor register offsets. RD-frame registers are relative to a core's RD base; the
// SGI-frame registers (banked SGI/PPI state) are relative to RD base + GICR_SGI_OFFSET.
#[cfg(not(feature = "pi"))]
const GICR_TYPER: usize = 0x0008; // 64-bit: Affinity_Value in bits[63:32], Last in bit4
#[cfg(not(feature = "pi"))]
const GICR_WAKER: usize = 0x0014; // ProcessorSleep = bit1, ChildrenAsleep = bit2
#[cfg(not(feature = "pi"))]
const GICR_IGROUPR0: usize = 0x0080; // (SGI frame) group select for SGIs/PPIs, 1 bit / INTID
#[cfg(not(feature = "pi"))]
const GICR_ISENABLER0: usize = 0x0100; // (SGI frame) set-enable for SGIs/PPIs
#[cfg(not(feature = "pi"))]
const GICR_ISPENDR0: usize = 0x0200; // (SGI frame) set-pending for SGIs/PPIs (diagnostic read)
#[cfg(not(feature = "pi"))]
const GICR_IPRIORITYR: usize = 0x0400; // (SGI frame) priority, 1 byte / INTID

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

// --- GICv3 version state + redistributor MMIO helpers (QEMU virt only; absent on the pi build). ---

/// Latched at `init` from GICD_PIDR2.ArchRev. `is_v3()` reads this on non-pi builds; the pi build
/// hard-pins v2 at compile time (`is_v3()` folds to `false`, eliminating every v3 branch).
#[cfg(not(feature = "pi"))]
static GIC_IS_V3: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Whether the runtime GIC is a GICv3. Compile-time `false` under `pi` (the GIC-400 is always v2),
/// so all v3 dispatch arms in this file are dead-code-eliminated from the Pi image (where every
/// caller of this helper is itself `#[cfg(not(feature = "pi"))]`, hence the pi-only dead_code allow).
#[inline]
#[cfg_attr(feature = "pi", allow(dead_code))]
fn is_v3() -> bool {
    #[cfg(feature = "pi")]
    {
        false
    }
    #[cfg(not(feature = "pi"))]
    {
        GIC_IS_V3.load(core::sync::atomic::Ordering::Relaxed)
    }
}

#[cfg(not(feature = "pi"))]
#[inline]
unsafe fn gicr_read(base: usize, off: usize) -> u32 {
    core::ptr::read_volatile((base + off) as *const u32)
}
#[cfg(not(feature = "pi"))]
#[inline]
unsafe fn gicr_write(base: usize, off: usize, val: u32) {
    core::ptr::write_volatile((base + off) as *mut u32, val);
}
#[cfg(not(feature = "pi"))]
#[inline]
unsafe fn gicr_read64(base: usize, off: usize) -> u64 {
    core::ptr::read_volatile((base + off) as *const u64)
}

/// Bring up the GIC on the boot core: detect the architecture version (v2 vs v3), run the matching
/// init sequence (distributor + this core's CPU interface, plus the redistributor on v3), then run
/// the self-SGI delivery smoke. Per-interrupt enabling happens later (`enable_ppi`/`enable_sgi`),
/// once each source is configured. Secondary cores skip the distributor and call `init_cpu_interface`
/// (v2) alone; the v3 per-core bring-up (redistributor wake + `init_cpu_interface_v3`) is factored so
/// JC2's APs can reuse it.
pub fn init() {
    // The Pi's GIC-400 is always v2, so its build skips detection entirely and runs the v2 path;
    // only the QEMU-virt build detects and can take the v3 path (and its self-SGI smoke).
    #[cfg(feature = "pi")]
    init_v2();

    #[cfg(not(feature = "pi"))]
    {
        detect_and_latch_version();
        if is_v3() {
            init_v3();
        } else {
            init_v2();
        }
        // Boot-time IPI exercise (v2 and v3): prove SGIs are delivered through the IRQ vector before
        // the timer is armed. The only IPI test available until JC2 brings SMP to the virt/Orin path.
        self_sgi_smoke();
    }
}

/// The GICv2 bring-up: the (global) distributor, then this core's memory-mapped CPU interface. This
/// is the exact sequence the driver has always run — unchanged so the Pi image stays byte-identical.
fn init_v2() {
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

// ============================ GICv3 (QEMU virt only) ============================
// Everything below to `enable_banked` is compiled out on the pi build.

/// Detect the GIC architecture version and latch v3. We probe ID_AA64PFR0_EL1.GIC (bits[27:24]) —
/// the ARM-canonical GICv3 CPU-interface check — NOT the GICD_PIDR2 register the brief first named:
/// the GICv2 distributor is a 4 KiB frame whose PIDR2 lives at 0xFE8, so the v3 PIDR2 offset 0xFFE8
/// exists only in the v3 64 KiB frame, and probing it by MMIO faults (external abort) on a v2 machine
/// (QEMU virt gic-version=2). A system-register read cannot fault, and a non-zero GIC field is exactly
/// the capability the v3 path relies on (the ICC_*_EL1 registers). 0b0000 => v2 (memory-mapped GICC);
/// 0b0001/0b0011 => GICv3/GICv4 system-register interface. Runs once on the BSP before any
/// version-specific access.
#[cfg(not(feature = "pi"))]
fn detect_and_latch_version() {
    let pfr0: u64;
    unsafe { core::arch::asm!("mrs {}, ID_AA64PFR0_EL1", out(reg) pfr0, options(nomem, nostack, preserves_flags)) };
    let gic_field = (pfr0 >> 24) & 0xF;
    GIC_IS_V3.store(gic_field != 0, core::sync::atomic::Ordering::Relaxed);
}

/// GICv3 boot-core bring-up: distributor (affinity routing + Group 1), this core's redistributor
/// (wake + all banked INTIDs to Group 1), then the system-register CPU interface.
#[cfg(not(feature = "pi"))]
fn init_v3() {
    let num_intids = unsafe {
        let typer = gicd_read(GICD_TYPER);
        ((typer & 0x1F) + 1) * 32
    };
    init_distributor_v3();
    let rd = init_redistributor_v3();
    init_cpu_interface_v3();
    serial_println!(
        ":: AARCH64 GICv3 init (GICD={:#x}, GICR={:#x}, {} INTIDs) ::",
        GICD_BASE, rd, num_intids
    );
}

/// Poll GICD_CTLR.RWP (bit 31) to 0 — a register write to the distributor is still propagating.
/// Bounded so a GIC that never clears it (or a model that doesn't implement RWP) can't wedge boot.
#[cfg(not(feature = "pi"))]
fn gicd_wait_rwp() {
    let mut budget = 1_000_000u32;
    while unsafe { gicd_read(GICD_CTLR) } & (1 << 31) != 0 {
        budget -= 1;
        if budget == 0 {
            break;
        }
        core::hint::spin_loop();
    }
}

/// Enable the GICv3 distributor for a single-security-state (QEMU virt DS=1) system. ARE (Affinity
/// Routing Enable, bit 4) MUST be set — and take effect — before the group enables, because it
/// switches the distributor to the affinity-routed model the redistributor/IROUTER paths assume;
/// then EnableGrp1 (bit 1), with EnableGrp0 (bit 0) harmless under DS=1 (mirrors the v2 bit0 enable).
#[cfg(not(feature = "pi"))]
fn init_distributor_v3() {
    unsafe {
        let ctlr = gicd_read(GICD_CTLR);
        gicd_write(GICD_CTLR, ctlr | (1 << 4)); // ARE
        gicd_wait_rwp();
        let ctlr = gicd_read(GICD_CTLR);
        gicd_write(GICD_CTLR, ctlr | (1 << 1) | (1 << 0)); // EnableGrp1 (+ Grp0)
        gicd_wait_rwp();
    }
}

/// This core's redistributor Affinity_Value, packed the way GICR_TYPER[63:32] reports it:
/// {Aff3,Aff2,Aff1,Aff0}, one byte each. MPIDR_EL1 keeps Aff0..Aff2 in bits[23:0] and Aff3 in
/// bits[39:32]; repack Aff3 up next to the rest.
#[cfg(not(feature = "pi"))]
fn mpidr_affinity() -> u32 {
    let mpidr: u64;
    unsafe { core::arch::asm!("mrs {}, MPIDR_EL1", out(reg) mpidr, options(nomem, nostack, preserves_flags)) };
    ((((mpidr >> 32) & 0xFF) << 24) | (mpidr & 0x00FF_FFFF)) as u32
}

/// Find THIS core's redistributor frame by walking the GICR frames and matching GICR_TYPER's
/// affinity against MPIDR_EL1. On QEMU virt's UEFI single-core path it's the first frame; the walk
/// is factored (not cached in a BSP-only static) so JC2's APs each resolve their own. Panics — a
/// loud STOP+report — if the `Last` frame is reached without a match (a malformed GIC / wrong base).
#[cfg(not(feature = "pi"))]
fn this_cpu_redistributor() -> usize {
    let target = mpidr_affinity();
    let mut frame = GICR_BASE;
    loop {
        let typer = unsafe { gicr_read64(frame, GICR_TYPER) };
        if (typer >> 32) as u32 == target {
            return frame;
        }
        if typer & (1 << 4) != 0 {
            panic!("GICv3: no redistributor frame for MPIDR affinity {:#010x}", target);
        }
        frame += GICR_STRIDE;
    }
}

/// Wake this core's redistributor and put all its banked INTIDs (SGIs + PPIs) into Group 1, then
/// return the RD base. Serves the BSP now and each AP in JC2 (call it once per core, before that
/// core's `init_cpu_interface_v3`). Per-INTID enable/priority happens later in `enable_banked` (v3).
#[cfg(not(feature = "pi"))]
fn init_redistributor_v3() -> usize {
    let rd = this_cpu_redistributor();
    unsafe {
        // Wake: clear ProcessorSleep (bit1), then wait for ChildrenAsleep (bit2) to clear — bounded,
        // matching the driver's other spin budgets, so a model that never clears it can't hang boot.
        let waker = gicr_read(rd, GICR_WAKER);
        gicr_write(rd, GICR_WAKER, waker & !(1 << 1));
        let mut budget = 1_000_000u32;
        while gicr_read(rd, GICR_WAKER) & (1 << 2) != 0 {
            budget -= 1;
            if budget == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        // All 32 banked INTIDs to Group 1 (Non-secure-deliverable), the v3 analogue of the Pi's v2
        // IGROUPR reassignment. The generic-timer PPI 30 and any IPI SGI are covered in one write.
        gicr_write(rd + GICR_SGI_OFFSET, GICR_IGROUPR0, 0xFFFF_FFFF);
    }
    rd
}

/// Open THIS core's GICv3 CPU interface via the system-register (ICC_*_EL1) path — the v3 analogue
/// of `init_cpu_interface`. Enable SRE, drop the priority mask to pass everything, EOImode=0 (a
/// single ICC_EOIR does priority-drop + deactivate, like the v2 GICC_EOIR), then enable Group 1.
///
/// SRE subtlety: on the QEMU virt/UEFI path the kernel runs at EL2 with IRQs routed there
/// (HCR_EL2.IMO), and EL2 accesses to the ICC_*_EL1 *physical* interface are gated by
/// ICC_SRE_EL2.SRE — so set that when at EL2. Also set ICC_SRE_EL1 (harmless here, and required for
/// a future EL1-hosted v3, e.g. Orin metal in a later arc).
#[cfg(not(feature = "pi"))]
fn init_cpu_interface_v3() {
    unsafe {
        // ICC_SRE_EL2 (S3_4_C12_C9_5).SRE — only reachable at EL2; needed there to use ICC_*_EL1.
        if super::exceptions::current_el() >= 2 {
            let sre: u64;
            core::arch::asm!("mrs {}, S3_4_C12_C9_5", out(reg) sre, options(nomem, nostack, preserves_flags));
            core::arch::asm!("msr S3_4_C12_C9_5, {}", in(reg) sre | 1, options(nomem, nostack, preserves_flags));
            core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
        }
        // ICC_SRE_EL1 (S3_0_C12_C12_5).SRE.
        let sre1: u64;
        core::arch::asm!("mrs {}, S3_0_C12_C12_5", out(reg) sre1, options(nomem, nostack, preserves_flags));
        core::arch::asm!("msr S3_0_C12_C12_5, {}", in(reg) sre1 | 1, options(nomem, nostack, preserves_flags));
        core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
        // ICC_PMR_EL1 (S3_0_C4_C6_0) = 0xFF: pass every priority (mirrors GICC_PMR=0xFF).
        core::arch::asm!("msr S3_0_C4_C6_0, {}", in(reg) 0xFFu64, options(nomem, nostack, preserves_flags));
        // ICC_CTLR_EL1 (S3_0_C12_C12_4): clear EOImode (bit1) so ICC_EOIR both drops priority and
        // deactivates, matching the v2 single-write EOI. RMW to preserve the other (RO/RES) fields.
        let ctlr: u64;
        core::arch::asm!("mrs {}, S3_0_C12_C12_4", out(reg) ctlr, options(nomem, nostack, preserves_flags));
        core::arch::asm!("msr S3_0_C12_C12_4, {}", in(reg) ctlr & !(1u64 << 1), options(nomem, nostack, preserves_flags));
        // ICC_IGRPEN1_EL1 (S3_0_C12_C12_7) = 1: enable Group 1 signalling (mirrors GICC_CTLR enable).
        core::arch::asm!("msr S3_0_C12_C12_7, {}", in(reg) 1u64, options(nomem, nostack, preserves_flags));
        core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
    }
}

/// Boot-time self-SGI smoke (QEMU virt path, both GIC versions). Sends a free SGI to this core and
/// confirms it is delivered through the IRQ vector (`handle_irq` -> `percpu::count_ipi`). It runs
/// inside `init`, i.e. BEFORE `exceptions::enable_irq`, so PSTATE.I is still masked — it therefore
/// makes the SGI pending, briefly unmasks IRQ itself, waits (bounded, off CNTPCT) for the per-core
/// IPI counter to move, then restores DAIF. Nothing else is enabled yet (the timer is armed after
/// `gic::init`), so the only interrupt that can land in the window is our SGI.
#[cfg(not(feature = "pi"))]
fn self_sgi_smoke() {
    // A free SGI INTID: the scheduler's IPI is SGI 0 (smp::IPI_RESCHED); 15 is unused everywhere.
    const SMOKE_SGI: u32 = 15;
    let self_cpu = (mpidr_affinity() & 0xF) as usize; // Aff0 = this core's target-list bit / GICC id
    enable_sgi(SMOKE_SGI);

    let before = super::percpu::this_cpu().ipis.load(core::sync::atomic::Ordering::Relaxed);
    let delivered;
    unsafe {
        let daif: u64;
        core::arch::asm!("mrs {}, DAIF", out(reg) daif, options(nomem, nostack, preserves_flags));
        // Make the SGI pending first, then unmask IRQ so it is taken immediately on the isb.
        send_sgi(self_cpu, SMOKE_SGI);
        core::arch::asm!("msr daifclr, #2", options(nomem, nostack, preserves_flags));
        core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
        // Bounded wait (~100 ms off the free-running CNTPCT, the same window verify_live uses).
        let freq = super::timer::cntfrq();
        let budget = (if freq == 0 { 62_500_000 } else { freq }) / 10;
        let start = super::timer::cntpct();
        let mut got = false;
        while super::timer::cntpct().wrapping_sub(start) < budget {
            if super::percpu::this_cpu().ipis.load(core::sync::atomic::Ordering::Relaxed) != before {
                got = true;
                break;
            }
            core::hint::spin_loop();
        }
        delivered = got;
        // Restore the entry DAIF (re-mask IRQ) — the boot sequence continues masked until enable_irq.
        core::arch::asm!("msr daif, {}", in(reg) daif, options(nomem, nostack, preserves_flags));
    }

    let ver = if is_v3() { "v3" } else { "v2" };
    if delivered {
        serial_println!(":: GIC self-SGI delivered ({}) ::", ver);
    } else {
        serial_println!(":: GIC self-SGI NOT delivered ({}) — check GIC routing ::", ver);
    }
}

/// Enable a banked interrupt (SGI 0-15 or PPI 16-31) on THIS core: (on the Pi) move it to Group 1,
/// give it the highest priority, and set its enable bit. INTIDs < 32 are banked per-core, so
/// ISENABLER0 / the low IGROUPR/IPRIORITYR words address *this* core's copy — each core must call
/// this for the sources it wants.
fn enable_banked(intid: u32) {
    // v3: banked SGIs/PPIs (including the generic-timer PPI 30) live in this core's redistributor,
    // not the distributor — dispatch there. Compiled out on the pi build (v2 only).
    #[cfg(not(feature = "pi"))]
    if is_v3() {
        enable_banked_v3(intid);
        return;
    }
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

/// GICv3 banked-interrupt enable: SGIs/PPIs are configured in THIS core's redistributor SGI frame,
/// not the distributor. The group is already set wholesale (`init_redistributor_v3` put all 32 into
/// Group 1), so here we just set the highest priority + the enable bit, then publish with a `dsb`.
#[cfg(not(feature = "pi"))]
fn enable_banked_v3(intid: u32) {
    let sgi = this_cpu_redistributor() + GICR_SGI_OFFSET;
    unsafe {
        core::ptr::write_volatile((sgi + GICR_IPRIORITYR + intid as usize) as *mut u8, 0x00);
        gicr_write(sgi, GICR_ISENABLER0, 1 << intid);
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
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

/// Enable a shared peripheral interrupt (SPI, INTID >= 32) and route it to `target_cpu`. Unlike the
/// banked SGI/PPI path, SPIs live in WORD-indexed distributor registers (the enable_banked single-word
/// `ISENABLER0 | (1<<intid)` form is only valid for INTID < 32) and need explicit CPU routing via
/// ITARGETSR. This is GLOBAL distributor state, so call it once on the BSP (the same place the timer
/// PPI is brought up). Used for the PL011 RX interrupt (scheduler-driven input); level-sensitive.
///
/// Register math (word = intid/32, bit = intid%32; e.g. INTID 153 -> word 4, bit 25):
///   * IGROUPR[word]    |= 1<<bit    — Group 1 (Non-secure deliverability on the Pi), RMW.
///   * ICFGR[intid/16]  clear the 2-bit field at (intid%16)*2 -> 0b00 = level-sensitive, RMW so the
///     other 15 INTIDs in that word keep their config (on GIC-400 bit0 of each field is RAO/WI, so
///     clearing the edge bit is what selects level).
///   * IPRIORITYR[intid] = 0x00      — highest priority (byte; same level as the timer PPI, and with
///     BPR=0 there is no preemption nesting — acceptable).
///   * ITARGETSR[intid]  = 1<<cpu    — CPU-interface bitmask (byte; on the Pi 4 the interface bit
///     equals the core index, as `send_sgi` documents).
///   * ISENABLER[word]   = 1<<bit    — set-enable (word-indexed).
/// A closing `dsb sy` publishes every write before the caller unmasks the peripheral's own interrupt.
pub fn enable_spi(intid: u32, target_cpu: usize) {
    debug_assert!(intid >= 32, "enable_spi expects an SPI (>= 32)");
    debug_assert!(target_cpu < 8, "ITARGETSR is an 8-bit CPU-interface bitmask");
    // v3: with ARE=1 the SPI is routed by MPIDR affinity via GICD_IROUTER, not the v2 ITARGETSR byte.
    // (Not exercised on the QEMU virt path in JC1 — the sole SPI, the PL011 RX, is baremetal/pi-only —
    // but implemented for correctness/JC2 reuse.) Compiled out on the pi build.
    #[cfg(not(feature = "pi"))]
    if is_v3() {
        enable_spi_v3(intid, target_cpu);
        return;
    }
    let word = (intid / 32) as usize;
    let bit = intid % 32;
    unsafe {
        #[cfg(feature = "pi")]
        {
            let goff = GICD_IGROUPR + word * 4;
            gicd_write(goff, gicd_read(goff) | (1 << bit));
        }
        let coff = GICD_ICFGR + (intid as usize / 16) * 4;
        let cshift = (intid % 16) * 2;
        gicd_write(coff, gicd_read(coff) & !(0b11 << cshift)); // level-sensitive, RMW
        core::ptr::write_volatile((GICD_BASE + GICD_IPRIORITYR + intid as usize) as *mut u8, 0x00);
        core::ptr::write_volatile((GICD_BASE + GICD_ITARGETSR + intid as usize) as *mut u8, 1 << target_cpu);
        gicd_write(GICD_ISENABLER + word * 4, 1 << bit);
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

/// GICv3 SPI enable. Group/config/priority/enable stay in the distributor (as v2), but CPU routing
/// under ARE=1 is by affinity through the 64-bit GICD_IROUTER<n> (0x6000 + 8*n) instead of the v2
/// ITARGETSR byte. `target_cpu` maps to Aff0 on QEMU virt (higher affinities 0); JC2 / Orin clusters
/// must widen this to the target's full MPIDR affinity.
#[cfg(not(feature = "pi"))]
fn enable_spi_v3(intid: u32, target_cpu: usize) {
    let word = (intid / 32) as usize;
    let bit = intid % 32;
    unsafe {
        let goff = GICD_IGROUPR + word * 4;
        gicd_write(goff, gicd_read(goff) | (1 << bit)); // Group 1 (deliverable), RMW
        let coff = GICD_ICFGR + (intid as usize / 16) * 4;
        let cshift = (intid % 16) * 2;
        gicd_write(coff, gicd_read(coff) & !(0b11 << cshift)); // level-sensitive, RMW
        core::ptr::write_volatile((GICD_BASE + GICD_IPRIORITYR + intid as usize) as *mut u8, 0x00);
        core::ptr::write_volatile((GICD_BASE + GICD_IROUTER + intid as usize * 8) as *mut u64, (target_cpu as u64) & 0xF);
        gicd_write(GICD_ISENABLER + word * 4, 1 << bit);
        core::arch::asm!("dsb sy", options(nostack, preserves_flags));
    }
}

/// Send SGI `intid` (0-15) to core `target_cpu` (its GIC CPU-interface number, which equals the
/// core index on the Pi 4). The `DSB` publishes any memory the target will read on wake before the
/// interrupt is raised. TargetListFilter=0b00 => use the CPUTargetList bitmask in bits[23:16].
pub fn send_sgi(target_cpu: usize, intid: u32) {
    // v3: SGIs are generated by writing the ICC_SGI1R_EL1 system register, not GICD_SGIR. Compiled
    // out on the pi build.
    #[cfg(not(feature = "pi"))]
    if is_v3() {
        send_sgi_v3(target_cpu, intid);
        return;
    }
    unsafe {
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));
        let val = ((1u32 << target_cpu) << 16) | (intid & 0xF);
        gicd_write(GICD_SGIR, val);
    }
}

/// GICv3 SGI generation via ICC_SGI1R_EL1 (S3_0_C12_C11_5): INTID in bits[27:24], TargetList in
/// bits[15:0] (one bit per Aff0 in the target's affinity group), Aff1/2/3 in [23:16]/[39:32]/[55:48].
/// QEMU virt (cortex-a72) keeps every core in one affinity group (Aff1..3 = 0, Aff0 = core index),
/// so `1 << target_cpu` addresses core `target_cpu`. JC2 / Orin clusters (non-zero Aff1) must fill
/// the affinity fields from the target's MPIDR. The `dsb ish` publishes memory the target reads on
/// wake before the SGI is raised (as the v2 path does).
#[cfg(not(feature = "pi"))]
fn send_sgi_v3(target_cpu: usize, intid: u32) {
    let sgi = ((intid as u64 & 0xF) << 24) | (1u64 << (target_cpu & 0xF));
    unsafe {
        core::arch::asm!("dsb ish", options(nostack, preserves_flags));
        core::arch::asm!("msr S3_0_C12_C11_5, {}", in(reg) sgi, options(nostack, preserves_flags));
        core::arch::asm!("isb", options(nostack, preserves_flags));
    }
}

/// Read the set-pending bit for a banked INTID 16-31 (the generic-timer PPI). Diagnostic only —
/// tells whether the GIC sees the PPI pending (separates "timer not asserting" from "GIC not
/// delivering"). v2 tracks banked pending in GICD_ISPENDR0; v3 moves it to this core's redistributor
/// SGI frame (GICR_ISPENDR0), so dispatch — otherwise the v3 probe would always read 0.
pub fn ppi_pending(intid: u32) -> bool {
    #[cfg(not(feature = "pi"))]
    if is_v3() {
        let sgi = this_cpu_redistributor() + GICR_SGI_OFFSET;
        return unsafe { gicr_read(sgi, GICR_ISPENDR0) } & (1 << intid) != 0;
    }
    const GICD_ISPENDR: usize = 0x200;
    unsafe { gicd_read(GICD_ISPENDR) & (1 << intid) != 0 }
}

/// Acknowledge, dispatch, and end one interrupt at the CPU interface. Called from the IRQ vector
/// stub with IRQs masked. Reading IAR acknowledges (and returns the INTID); writing EOIR the same
/// value completes it. Spurious reads (INTID >= 1020) take no EOI.
pub fn handle_irq() {
    // v3: acknowledge/EOI go through the ICC_*_EL1 system registers, not the memory-mapped GICC.
    // Compiled out on the pi build (which is always v2).
    #[cfg(not(feature = "pi"))]
    if is_v3() {
        handle_irq_v3();
        return;
    }
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
    } else {
        // Other SPIs. So far the only one is the PL011 RX interrupt (M5c scheduler-driven input,
        // bare-metal Pi only). Its handler MASKS the RX interrupt (deasserting the level to the GIC,
        // with a `dsb` so that is visible before the EOI below — else this level-sensitive SPI would
        // re-pend) and wakes the input task; it never reads the FIFO or logs (the task drains it).
        #[cfg(feature = "baremetal")]
        if intid == crate::arch::serial::PL011_RX_INTID {
            crate::arch::serial::on_rx_interrupt();
        }
    }

    // EOI with the full acked value (preserves the source-CPU field for SGIs; a no-op for PPIs).
    unsafe { gicc_write(GICC_EOIR, iar) };

    // Scheduler preemption runs AFTER EOI: timer_preempt may context-switch away, and doing so
    // before deactivating the interrupt would leave it active on this CPU interface across the
    // switch (blocking equal/lower-priority interrupts until this context is resumed and returns).
    // Ordering is on_tick (re-arm, deassert the level) -> EOI (deactivate) -> preempt (switch).
    #[cfg(feature = "baremetal")]
    if intid == crate::arch::timer::TIMER_INTID {
        crate::arch::sched::timer_preempt();
    }
}

/// GICv3 IRQ acknowledge/dispatch/EOI via the system-register CPU interface (ICC_IAR1_EL1 /
/// ICC_EOIR1_EL1). This is the QEMU virt/Orin (non-baremetal) path: the scheduler, the PL011 RX SPI
/// and timer preemption are all baremetal(pi)-only, so the v3 dispatch tree is just the SGI counter
/// and the generic timer. INTID 1020-1023 are spurious/special and take no EOI (as on v2).
#[cfg(not(feature = "pi"))]
fn handle_irq_v3() {
    let iar: u64;
    unsafe { core::arch::asm!("mrs {}, S3_0_C12_C12_0", out(reg) iar, options(nomem, nostack, preserves_flags)) };
    let intid = (iar & 0xFF_FFFF) as u32; // v3 INTID is 24-bit; source-CPU is not in the IAR
    if intid >= SPURIOUS_FLOOR {
        return; // spurious / special — no EOI
    }
    if intid < 16 {
        crate::arch::percpu::count_ipi();
    } else if intid == crate::arch::timer::TIMER_INTID {
        crate::arch::timer::on_tick();
    }
    // ICC_EOIR1_EL1: writing the acked value drops priority and (EOImode=0) deactivates.
    unsafe { core::arch::asm!("msr S3_0_C12_C12_1, {}", in(reg) iar, options(nomem, nostack, preserves_flags)) };
}
