// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// QEMU `virt` / UEFI EL2 -> EL1 drop (Arc JC3). The `virt` build boots via EDK2/AAVMF, which hands the
// kernel off at **EL2** with the MMU already on (its own identity map of RAM + the low-1-GiB peripheral
// window). A normal OS runs at EL1 — and, decisively, the aarch64 scheduler (`sched.rs`) is EL1-coupled
// (its context-switch, per-CPU access and preemption all assume the EL1&0 regime and EL1-banked
// exception state). So to un-gate the scheduler + CAPSTONE on the `virt` GICv3 path, the boot core must
// first drop EL2 -> EL1, exactly as the Pi bare-metal path does in `boot::drop_to_el1` / `boot::enable_mmu`.
//
// This module is the `virt` analogue of that Pi pair — a SEPARATE module because `boot.rs` is
// `#[cfg(feature = "baremetal")]` (its tables hardcode the Pi physical map and it runs from a cold-reset
// EL2 with the MMU OFF), while here we start from a live EL2 with the MMU already ON. The difference in
// starting state drives the difference in sequence:
//
//   * Pi: `drop_to_el1` erets to EL1 with the MMU OFF, then `enable_mmu` turns the EL1 MMU on. Sound only
//     because the post-eret window runs no atomics before `enable_mmu`.
//   * virt (here): we build the EL1&0 translation regime and program it with **M=1 while still at EL2**
//     (where it is dormant — SCTLR_EL2 governs EL2 translation), THEN eret to EL1. The EL1 MMU is
//     therefore live from the very first EL1 instruction — there is NO MMU-off window at EL1 at all, so
//     no atomic ever runs on Device-typed memory.
//
// The EL1 identity map mirrors `mmu_tegra`'s shape (which does the same job for the tegra EL2 regime):
// `L1[0]` = the low-1-GiB Device window (QEMU `virt` puts the GICv3 distributor at 0x0800_0000, the GICR
// at 0x080A_0000 and the PL011 UART at 0x0900_0000 all inside it), every RAM GiB the firmware memory map
// names = a Normal-WB block, the rest invalid. The RAM-GiB set is captured from `boot_info` BEFORE
// `memory::init` consumes it (see `ram_gib_mask`), plus a belt-and-braces re-mark of the executing GiB
// and the live stack GiB at drop time so the very first post-drop fetch / stack access is always backed.
//
// Scope: the boot core only (`drop only the BSP, leave APs parked` — the JC2 SMP secondaries stay at EL2
// in their WFI park, untouched). Single-core, cooperative CAPSTONE — see `sched::run_capstone_boot_core`.

use unaos_boot_info::{BootInfo, MemoryRegion, MemoryRegionKind};

/// A single Level-1 translation table: 512 entries x 8 bytes = one 4 KiB page. With a 4 KiB granule and
/// T0SZ=25 (39-bit VA) the top lookup level is L1 and each entry maps a **1 GiB block**, so these 512
/// entries cover the whole 512 GiB VA — we fill `L1[0]` (Device) and each RAM GiB (Normal), the rest
/// invalid. Lives in BSS (the kernel image sits in RAM, GiB 1 on `virt`, so the walker reaches it once
/// the map is live). Written entry-by-entry in `build_l1`, so it does not depend on the loader zeroing BSS.
#[repr(C, align(4096))]
struct PageTable([u64; 512]);
static mut L1: PageTable = PageTable([0; 512]);

// ── Translation attributes (ARM ARM DDI0487), EL1&0 regime — the proven `boot.rs`/`mmu_tegra` recipe. ──
// MAIR: AttrIdx 0 = Normal Inner/Outer Write-Back non-transient (0xFF); AttrIdx 1 = Device-nGnRnE (0x00),
// matching the Pi recipe (QEMU tolerates any Device sub-type for the PL011/GIC MMIO). Regime-independent.
const MAIR_VAL: u64 = 0xFF;

// TCR_EL1: T0SZ=25 (39-bit VA -> L1 top level, 1 GiB blocks), IRGN0=ORGN0=WB, SH0=inner-shareable,
// TG0=4 KiB, the TTBR1 (high) half disabled via EPD1=1, IPS=36-bit at [34:32]. Byte-for-byte the
// `boot.rs`/`mmu_tegra` EL1 value (PS is IPS at [34:32] at EL1, NOT the TCR_EL2 short-format layout).
const TCR_EL1_VAL: u64 = 25            // T0SZ  [5:0]
    | (0b01 << 8)                      // IRGN0 = WB
    | (0b01 << 10)                     // ORGN0 = WB
    | (0b11 << 12)                     // SH0   = inner shareable
    | (0b00 << 14)                     // TG0   = 4 KiB
    | (25 << 16)                       // T1SZ  [21:16] (TTBR1 unused; legal value)
    | (1 << 23)                        // EPD1  = disable the TTBR1 table walk
    | (0b10 << 30)                     // TG1   = 4 KiB (legal encoding; TTBR1 unused)
    | (0b001 << 32);                   // IPS   = 36-bit / 64 GiB, at [34:32]

// SCTLR_EL1 as an ABSOLUTE value (NOT an RMW): on `virt` the firmware runs the kernel at EL2 and never
// initialises SCTLR_EL1, which resets architecturally UNKNOWN — an RMW could read the RES1 bits as 0 and
// leave them cleared (CONSTRAINED UNPREDICTABLE). So OR the Armv8.0 Cortex-A72 SCTLR_EL1 RES1 mask
// (0x30D0_0800 = bits 29,28,23,22,20,11) with M (MMU), C (data cache), I (instruction cache). This is
// the exact `boot.rs` SCTLR_EL1_VAL; QEMU `virt` is `-cpu cortex-a72`, so the A72 RES1 mask is correct.
const SCTLR_EL1_VAL: u64 = 0x30D0_0800 | (1 << 0) | (1 << 2) | (1 << 12);

// L1 block descriptor bits. bits[1:0]=0b01 (block), AttrIndx[4:2], AF=bit10, SH[9:8]. AP[7:6]=0b00 in a
// RAM block => EL1 read-write, no EL0, executable at EL1 (PXN=0). A Device block sets UXN(54)+PXN(53) so
// peripheral memory is never executable at any EL.
const DESC_BLOCK: u64 = 0b01;
const DESC_AF: u64 = 1 << 10;
const SH_INNER: u64 = 0b11 << 8;
const ATTR_NORMAL: u64 = 0 << 2; // MAIR AttrIdx 0
const ATTR_DEVICE: u64 = 1 << 2; // MAIR AttrIdx 1
const DESC_UXN: u64 = 1 << 54;
const DESC_PXN: u64 = 1 << 53;

/// Normal, cacheable, inner-shareable, executable 1 GiB RAM block (kernel image / stack / heap / tables).
#[inline]
const fn ram_block(pa: u64) -> u64 {
    pa | DESC_AF | SH_INNER | ATTR_NORMAL | DESC_BLOCK
}
/// Device-nGnRnE, execute-never 1 GiB block (the low peripheral window: GICD/GICR/PL011/PCIe-ECAM-low).
#[inline]
const fn device_block(pa: u64) -> u64 {
    pa | DESC_UXN | DESC_PXN | DESC_AF | ATTR_DEVICE | DESC_BLOCK
}

/// Derive the RAM-GiB mask (bit `g` set ⇔ `g * 1 GiB` is firmware-declared RAM) from the boot memory
/// map. MUST be called BEFORE `memory::init` consumes `boot_info` (it moves the `&'static mut`), so the
/// caller captures this `u64` early and carries it into the drop. GiB 0 is never marked RAM (it stays the
/// Device window `L1[0]`). Mirrors `mmu_tegra::build_l1`'s Usable/Bootloader classification.
pub fn ram_gib_mask(boot_info: &BootInfo) -> u64 {
    let mut mask: u64 = 0;
    if boot_info.memory_regions_addr != 0 && boot_info.memory_regions_len != 0 {
        let regions = unsafe {
            core::slice::from_raw_parts(
                boot_info.memory_regions_addr as *const MemoryRegion,
                boot_info.memory_regions_len,
            )
        };
        for r in regions {
            let is_ram = matches!(r.kind, MemoryRegionKind::Usable | MemoryRegionKind::Bootloader);
            if !is_ram || r.page_count == 0 {
                continue;
            }
            let start = r.phys_start;
            let end = r.phys_start + r.page_count * 4096 - 1; // inclusive last byte
            let mut g = start >> 30;
            let g_hi = end >> 30;
            while g <= g_hi {
                if g >= 1 && g < 64 {
                    mask |= 1u64 << g;
                }
                g += 1;
            }
        }
    }
    mask
}

/// Populate `L1`: `L1[0]` = the low-1-GiB Device window, every RAM GiB in `mask` = a Normal-WB block, the
/// rest invalid (a stray access there faults rather than silently succeeding). Adds a belt-and-braces
/// re-mark of the GiB we execute from and the GiB the live SP points into, so the first post-drop fetch /
/// stack access is backed even if the firmware map omitted them. Runs at EL2, MMU on; volatile writes only.
unsafe fn build_l1(mask: u64) {
    // The executing PC and live SP (identity VA==PA under the firmware map): guarantee their GiBs are RAM.
    let (code_va, sp_va): (u64, u64);
    unsafe {
        core::arch::asm!("adr {}, .", out(reg) code_va, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mov {}, sp", out(reg) sp_va, options(nomem, nostack, preserves_flags));
    }
    let mut mask = mask;
    for va in [code_va, sp_va] {
        let g = va >> 30;
        if g >= 1 && g < 64 {
            mask |= 1u64 << g;
        }
    }
    let l1 = &raw mut L1 as *mut u64;
    unsafe {
        l1.add(0).write_volatile(device_block(0));
        for i in 1..512usize {
            let desc = if i < 64 && (mask >> i) & 1 != 0 { ram_block((i as u64) << 30) } else { 0 };
            l1.add(i).write_volatile(desc);
        }
    }
    unsafe { clean_l1_to_poc() };
}

/// Clean the whole L1 page to the Point of Coherency (`dc cvac` per 64-byte line, then `dsb sy`). The
/// descriptors were written through the firmware's cacheable EL2 mapping; the EL1 table walker (cacheable
/// per TCR IRGN0/ORGN0=WB) reads them coherently by PA on this PIPT-cache A72, but the explicit clean +
/// barrier orders the stores before the regime is armed — belt and braces, matching `mmu_tegra`.
unsafe fn clean_l1_to_poc() {
    let base = &raw const L1 as u64;
    let mut off: u64 = 0;
    while off < 4096 {
        unsafe { core::arch::asm!("dc cvac, {}", in(reg) base + off, options(nostack, preserves_flags)) };
        off += 64;
    }
    unsafe { core::arch::asm!("dsb sy", options(nostack, preserves_flags)) };
}

/// Program the EL1&0 translation regime from EL2 with the MMU ENABLED (M=1). Dormant while we remain at
/// EL2 (SCTLR_EL2 governs EL2 translation), it becomes live the instant the drop's `eret` lands at EL1 —
/// so EL1 never runs a single instruction with its MMU off. One `asm!` block, no memory traffic inside;
/// the `tlbi vmalle1` drops any stale EL1&0 TLB state before the regime is armed. The map is identity, so
/// enabling EL1 translation moves neither PC nor SP once we are at EL1.
unsafe fn enable_el1_regime() {
    let ttbr0 = &raw const L1 as u64;
    unsafe {
        core::arch::asm!(
            "msr MAIR_EL1, {mair}",
            "msr TCR_EL1,  {tcr}",
            "msr TTBR0_EL1, {ttbr0}",
            "msr TTBR1_EL1, xzr",     // high half unused (EPD1=1 disables its walk); zero defensively
            "tlbi vmalle1",           // drop stale EL1&0 TLB entries before translation is armed
            "dsb sy",
            "isb",
            "msr SCTLR_EL1, {sctlr}", // absolute M|C|I|RES1 (SCTLR_EL1 resets UNKNOWN on virt — no RMW)
            "isb",
            mair = in(reg) MAIR_VAL,
            tcr = in(reg) TCR_EL1_VAL,
            ttbr0 = in(reg) ttbr0,
            sctlr = in(reg) SCTLR_EL1_VAL,
            options(nostack, preserves_flags),
        );
    }
}

// The EL2 -> EL1 drop proper. MUST be naked asm: an ordinary Rust fn's prologue/epilogue would spill/
// reload x30 and adjust SP around the `eret`, and the eret skips the epilogue — corrupting the frame.
// Runs at EL2, no stack traffic, x30 untouched; `eret`s back to the caller now at EL1 (same SP/frame —
// the standard "return to x30" drop trick). By this point `enable_el1_regime` has already armed the EL1
// MMU, so the EL1 landing runs cached/coherent. This mirrors the Pi `boot::drop_to_el1` sequence, with
// two `virt`-specific additions: it programs no MMU (already done) and it DISABLES the physical timer.
//
// Why disable the timer: the shared IRQ vector stub (`exceptions::__vec_irq`) banks ELR_EL2/SPSR_EL2 on
// this not-baremetal/not-tegra build (the fixed `irq_bank!` EL2 arm), which would fault if an IRQ were taken
// at EL1. CAPSTONE on `virt` runs COOPERATIVELY (no preemption needed — the same way the Pi CAPSTONE runs
// in QEMU with no Group-1 delivery), so we kill the only periodic IRQ source (CNTP) before the drop; the
// JC2 SMP SGIs are already quiescent (their proof completed before this drop). CNTPCT (the free-running
// counter `busy_delay_ms` reads) is unaffected — only the timer *condition* (CNTP_CTL) is disabled.
core::arch::global_asm!(
    r#"
    .globl drop_el2_to_el1_virt
drop_el2_to_el1_virt:
    // MPIDR_EL1/MIDR_EL1 read at EL1 return VMPIDR_EL2/VPIDR_EL2 — seed them with the real values.
    mrs   x0, mpidr_el1
    msr   vmpidr_el2, x0
    mrs   x0, midr_el1
    msr   vpidr_el2, x0
    // CPTR_EL2 = 0x33ff: clear TFP (bit 10) so EL1/EL0 FP/SIMD does NOT trap to EL2 (the kernel is +neon;
    // fmt/memcpy autovectorize). CPTR_EL2.TFP takes precedence over CPACR_EL1.FPEN and resets UNKNOWN, so
    // set it explicitly; 0x33ff keeps the non-VHE RES1 bits set (do NOT 'msr cptr_el2, xzr').
    mov   x0, #0x33ff
    msr   cptr_el2, x0
    // MDCR_EL2 = 0: don't route EL1 debug/PMU exceptions to the (now abandoned) EL2 vectors.
    msr   mdcr_el2, xzr
    // CNTHCTL_EL2 EL1PCTEN+EL1PCEN=1: let EL1 read CNTPCT / use CNTP_* without trapping to EL2
    // (busy_delay_ms reads CNTPCT). CNTVOFF_EL2=0 so CNTVCT shares the physical timebase.
    mrs   x0, cnthctl_el2
    orr   x0, x0, #0x3
    msr   cnthctl_el2, x0
    msr   cntvoff_el2, xzr
    // Disable the physical timer condition (ENABLE=0) so no timer IRQ is delivered at EL1. See the module
    // comment — the shared IRQ stub banks EL2 state, and cooperative CAPSTONE needs no preemption.
    msr   cntp_ctl_el0, xzr
    // CPACR_EL1.FPEN=0b11 (bits [21:20]): the EL1-side FP/SIMD enable.
    mov   x0, #(0b11 << 20)
    msr   cpacr_el1, x0
    // HCR_EL2 = RW only (bit 31): EL1 executes AArch64; IMO/FMO/AMO cleared so a physical IRQ taken at EL1
    // targets EL1 natively (no EL2 routing). Bare write (this replaces the IMO/FMO/AMO set by
    // exceptions::install at EL2 — the boot core no longer routes to its abandoned EL2 vectors).
    mov   x0, #(1 << 31)
    msr   hcr_el2, x0
    // Land at EL1h (SPx = SP_EL1) with DAIF masked; SP_EL1 = current SP so the stack is continuous;
    // ELR_EL2 = x30 so the eret returns to our caller now running at EL1.
    mov   x0, sp
    msr   sp_el1, x0
    mov   x0, #0x3c5
    msr   spsr_el2, x0
    msr   elr_el2, x30
    isb
    eret
"#
);

unsafe extern "C" {
    /// Drop this core EL2 -> EL1 and return to the caller now executing at EL1. Call at EL2 AFTER
    /// `enable_el1_regime` has armed the EL1 MMU. See the naked asm above for the full sequence.
    fn drop_el2_to_el1_virt();
}

/// Drop the boot core EL2 -> EL1 on the `virt` path: build the EL1 identity map (from the pre-captured
/// `ram_gib_mask` + the live code/SP GiBs), arm the EL1&0 regime with the MMU on (dormant at EL2), then
/// eret to EL1. Returns to the caller now running at EL1 with the MMU live and DAIF masked. The caller
/// must (re-)init per-CPU (`percpu::init`, now TPIDR_EL1) and the exception vectors (`exceptions::install`,
/// now VBAR_EL1) before running the scheduler. Call at EL2 with IRQs about to be irrelevant (the drop
/// disables the timer); MUST NOT be called on the Pi/tegra builds (this module is `virt`-only).
pub unsafe fn drop_to_el1(ram_gib_mask: u64) {
    unsafe {
        build_l1(ram_gib_mask);
        enable_el1_regime();
        drop_el2_to_el1_virt();
    }
}
