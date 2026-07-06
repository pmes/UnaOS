// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Jetson Orin Nano (Tegra234) EL2 -> EL1 drop (Arc JM6). This is the **tegra analogue** of the `virt`
// JC3 drop in `boot_virt.rs`: the reviewed model. It exists as a SEPARATE module because `boot_virt.rs`
// is `virt`-only — its own doc pins it *"MUST NOT be called on the Pi/tegra builds"* (it hardcodes the
// QEMU-`virt` peripheral window and *builds a fresh* EL1 table). The tegra build differs in one decisive
// way that drives a different (and simpler) sequence:
//
//   * virt: `boot_virt::build_l1` builds a FRESH EL1&0 identity table, then arms the EL1 regime at it.
//   * tegra (here): `mmu_tegra::init` has ALREADY built the identity map — `L1[0]` = the Tegra Device
//     window (UARTC `0x0C28_0000` + GIC-600 `0x0F40_0000`), each firmware-declared RAM GiB = Normal-WB
//     — and is running the **EL2** regime on it. It also built the **EL1-precise twin** `L1_EL1`
//     (`MmuInfo::ttbr0_el1`), same GiB set with the EL1&0 leaf recipe; the EL1 arm points
//     `TTBR0_EL1`/`TCR_EL1`/`MAIR_EL1`/`SCTLR_EL1` at THAT twin. The map is IDENTITY (VA==PA), so
//     enabling EL1 translation moves neither PC nor SP.
//
// Why the live EL2 `L1` itself must NOT be the EL1 table — the JM6 metal lesson (5 dark boots; the
// original arc did exactly that and hung dark at the eret). EL2 leaf descriptors set AP[1] (bit 6,
// RES1 in the single-privilege EL2 regime). Reinterpreted under EL1&0, bit 6 is AP[2:1]=0b01 =
// "EL0 read-write" — and the VMSA forces PXN=1 for ANY region writable at EL0 (Arm ARM DDI 0487,
// stage-1 instruction access permissions), regardless of the descriptor's PXN bit and of
// SCTLR_EL1.WXN. "There is no EL0 yet" does not matter: the rule is unconditional. Every RAM GiB was
// therefore privileged-execute-never at EL1: the first post-eret fetch took a permission-fault
// instruction abort, and the VBAR_EL1 vector — in the same unexecutable RAM — could not even fetch its
// handler (recursive abort, dark). QEMU never caught it because no Tegra234 machine model exists; the
// virt twin was green because `boot_virt` builds its table with the EL1 recipe (AP[2:1]=0b00) from the
// start. `L1_EL1` maps RAM AP[2:1]=0b00 (EL1 RW, no EL0, EL1-executable) and Device UXN|PXN|nGnRE.
//
// The load-bearing invariant (same as virt, `boot_virt.rs:167-192`): we arm the EL1 regime with
// **`SCTLR_EL1.M=1` while STILL at EL2** — where it is DORMANT (`SCTLR_EL2` governs EL2 translation). It
// becomes live the instant the drop's `eret` lands at EL1, so **EL1 never runs a single instruction with
// its MMU off** — no atomic ever executes on Device-typed memory. (The Pi cold-reset path tolerates a
// brief MMU-off EL1 window only because it runs no atomics there; we avoid the window entirely.)
//
// Scope: the boot core only. Single-core, cooperative CAPSTONE (`sched::run_capstone_boot_core`). JM5's
// Orin SMP (PSCI `CPU_ON`) is PARKED — metal-blocked on an external Tegra BL31/MCE RAS fault — and is
// deliberately NOT part of this path (see `main.rs` `tegra_early_stop`); JM6 needs no SMP, so it sidesteps
// that wall entirely.

// ── Translation attributes (ARM ARM DDI0487), EL1&0 regime. ──────────────────────────────────────────
// MAIR: AttrIdx 0 = Normal Inner/Outer Write-Back non-transient (0xFF); AttrIdx 1 = **Device-nGnRE**
// (0x04) — the SAME value `mmu_tegra` programmed into MAIR_EL2, so the reused `L1`'s AttrIdx-1 device
// blocks keep the exact memory type they had at EL2 (nGnRE, Tegra's early-write-ack-tolerant type — NOT
// the Pi/virt nGnRnE). Regime-independent layout, so this one value serves MAIR_EL1 too.
const MAIR_VAL: u64 = 0x04FF;

// TCR_EL1: T0SZ=25 (39-bit VA -> L1 top level, 1 GiB blocks), IRGN0=ORGN0=WB, SH0=inner-shareable,
// TG0=4 KiB, the TTBR1 (high) half disabled via EPD1=1, IPS=36-bit at [34:32]. Byte-for-byte the
// `boot.rs`/`boot_virt`/`mmu_tegra` EL1 value (at EL1 PS is *IPS* at [34:32], NOT the TCR_EL2 short
// format). Covers Orin RAM (~10 GiB) inside the 64 GiB IPS ceiling.
const TCR_EL1_VAL: u64 = 25            // T0SZ  [5:0]
    | (0b01 << 8)                      // IRGN0 = WB
    | (0b01 << 10)                     // ORGN0 = WB
    | (0b11 << 12)                     // SH0   = inner shareable
    | (0b00 << 14)                     // TG0   = 4 KiB
    | (25 << 16)                       // T1SZ  [21:16] (TTBR1 unused; legal value)
    | (1 << 23)                        // EPD1  = disable the TTBR1 table walk
    | (0b10 << 30)                     // TG1   = 4 KiB (legal encoding; TTBR1 unused)
    | (0b001 << 32);                   // IPS   = 36-bit / 64 GiB, at [34:32]

// SCTLR_EL1 as an ABSOLUTE value (NOT an RMW). On tegra the firmware (NVIDIA UEFI) runs the kernel at
// EL2 and never initialises SCTLR_EL1, which resets architecturally UNKNOWN — an RMW could read the RES1
// bits as 0 and leave them cleared (CONSTRAINED UNPREDICTABLE). So OR the Armv8.0 SCTLR_EL1 RES1 mask
// (0x30D0_0800 = bits 29,28,23,22,20,11) with M (MMU), C (data cache), I (instruction cache) — the exact
// `boot.rs`/`boot_virt` value.
//
// A78AE note (Orin is Cortex-A78AE = Armv8.2, NOT the A72 that QEMU `virt` models): every bit in
// 0x30D0_0800 is, on A78AE, either still RES1 (11, 20, 22) or a defined control whose 1-value is benign
// for a kernel-only core (23=SPAN: leave PSTATE.PAN unchanged on exception; 28=nTLSMD, 29=LSMAOE: the
// AArch32 LDM/STM defaults, moot for an AArch64 kernel). None is RES0 on A78AE, so the A72 mask is a
// safe superset here. C|I are set because the reused `L1` is cacheable (mmu_tegra cleaned it to PoC and
// the EL1 walker is WB per TCR IRGN0/ORGN0).
const SCTLR_EL1_VAL: u64 = 0x30D0_0800 | (1 << 0) | (1 << 2) | (1 << 12);

/// Program the EL1&0 translation regime from EL2 with the MMU ENABLED (M=1), pointing at `mmu_tegra`'s
/// already-built **EL1-precise** identity table (`l1_pa` = `MmuInfo::ttbr0_el1` — NOT the live EL2 `L1`;
/// see the module header for the AP[1]-forces-PXN lesson). Dormant while we remain at EL2 (SCTLR_EL2
/// governs EL2 translation); it becomes live the instant the drop's `eret` lands at EL1 — so EL1 never
/// runs a single instruction with its MMU off. One `asm!` block, no memory traffic inside; `tlbi vmalle1`
/// drops any stale EL1&0 TLB state before the regime is armed. The map is identity, so arming EL1
/// translation moves neither PC nor SP once we are at EL1. No table build / cache-clean here:
/// `mmu_tegra::init` already wrote `L1_EL1` and cleaned it to the Point of Coherency.
unsafe fn enable_el1_regime(l1_pa: u64) {
    unsafe {
        core::arch::asm!(
            "msr MAIR_EL1, {mair}",
            "msr TCR_EL1,  {tcr}",
            "msr TTBR0_EL1, {ttbr0}",
            "msr TTBR1_EL1, xzr",     // high half unused (EPD1=1 disables its walk); zero defensively
            "tlbi vmalle1",           // drop stale EL1&0 TLB entries before translation is armed
            "dsb sy",
            "isb",
            "msr SCTLR_EL1, {sctlr}", // absolute M|C|I|RES1 (SCTLR_EL1 resets UNKNOWN on tegra — no RMW)
            "isb",
            mair = in(reg) MAIR_VAL,
            tcr = in(reg) TCR_EL1_VAL,
            ttbr0 = in(reg) l1_pa,
            sctlr = in(reg) SCTLR_EL1_VAL,
            options(nostack, preserves_flags),
        );
    }
}

// The EL2 -> EL1 drop proper. MUST be naked asm: an ordinary Rust fn's prologue/epilogue would spill/
// reload x30 and adjust SP around the `eret`, and the eret skips the epilogue — corrupting the frame.
// Runs at EL2, no stack traffic, x30 untouched; `eret`s back to the caller now at EL1 (same SP/frame —
// the standard "return to x30" drop trick). By this point `enable_el1_regime` has already armed the EL1
// MMU, so the EL1 landing runs cached/coherent. Mirrors `boot_virt::drop_el2_to_el1_virt` exactly, with
// one tegra-specific addition: it MASKS DAIF up front.
//
// Why mask DAIF first (unlike virt): the tegra path reaches this drop with IRQs UNMASKED at EL2 — JM4
// (`main.rs`) called `exceptions::enable_irq()` and proved the timer PPI delivering at EL2 (`verify_live`)
// just before us. If a timer IRQ fired mid-drop it would be handled fine at EL2 (the shared `__vec_irq`
// stub banks EL2 state on this not-baremetal build), but masking first makes the sequence atomic and is
// exactly the state we eret into (`SPSR_EL2 = 0x3c5` lands EL1 with DAIF masked). CNTP is then disabled
// below so no timer IRQ is even pending, and CAPSTONE runs cooperatively — the shared `__vec_irq` would
// FAULT if an IRQ were taken at EL1 (it reads ELR_EL2/SPSR_EL2, inaccessible at EL1).
core::arch::global_asm!(
    r#"
    .globl drop_el2_to_el1_tegra
drop_el2_to_el1_tegra:
    // Mask D/A/I/F at EL2 so the drop is atomic and we eret into the same masked state. JM4 left IRQs
    // unmasked at EL2; CAPSTONE at EL1 needs none (cooperative), and an IRQ at EL1 would fault the stub.
    msr   daifset, #0xf
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
    // + asm comments — the shared IRQ stub banks EL2 state, and cooperative CAPSTONE needs no preemption.
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
    fn drop_el2_to_el1_tegra();
}

/// Drop the Orin boot core EL2 -> EL1 on the `tegra` path: arm the EL1&0 regime at `mmu_tegra`'s already-
/// built EL1-precise identity table (`l1_pa` = `MmuInfo::ttbr0_el1`) with the MMU on (dormant at EL2),
/// then eret to EL1.
/// Returns to the caller now running at EL1 with the MMU live and DAIF masked. The caller must (re-)init
/// per-CPU (`percpu::init`, now TPIDR_EL1) and the exception vectors (`exceptions::install`, now VBAR_EL1)
/// before running the scheduler. Call at EL2 AFTER JM4's GIC/timer bring-up (the drop disables the timer,
/// so IRQs are about to be irrelevant); `tegra`-only (this module mirrors `boot_virt`'s `virt`-only role).
pub unsafe fn drop_to_el1(l1_pa: u64) {
    unsafe {
        enable_el1_regime(l1_pa);
        drop_el2_to_el1_tegra();
    }
}
