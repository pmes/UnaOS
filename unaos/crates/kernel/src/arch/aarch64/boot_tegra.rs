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

// ORIN-NET-3 (M1, `pcie3`): keep the post-drop EL1 regime's IPS in lock-step with the widened EL2
// output ceiling. `mmu_tegra::map_mmio_window` patches BOTH the live EL2 `L1` and the EL1-precise twin
// `L1_EL1` (twin-table discipline), so a controller-0 aperture mapped at ~184 GiB now survives the JM6
// EL2->EL1 drop; the recon itself runs at EL2 BEFORE the drop, but arming EL1 with a 36-bit IPS while
// the twin holds a 40-bit output descriptor would trap any FUTURE EL1 access to it. Widening IPS only
// expands the legal output range — every existing EL1 mapping (RAM <=10 GiB, device GiB-0/1) is
// unaffected. IPS is [34:32] in the EL1&0 format. Knob-off => byte-identical to the NET-2 literal.
#[cfg(feature = "pcie3")]
const TCR_EL1_ACTIVE: u64 = (TCR_EL1_VAL & !(0b111 << 32)) | (0b010 << 32);
#[cfg(not(feature = "pcie3"))]
const TCR_EL1_ACTIVE: u64 = TCR_EL1_VAL;

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
            tcr = in(reg) TCR_EL1_ACTIVE,
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
    // IRQEL-RT2 evidence latch. These four registers decide where a physical IRQ taken at EL1 goes
    // (HCR_EL2.IMO/TGE), whether EL1 may touch CNTP_*/CNTPCT at all (CNTHCTL_EL2), and whether an
    // ICC_*_EL1 access at EL1 is even defined (ICC_SRE_EL1) — and NONE of them is readable once we
    // eret: an `mrs` of an EL2 register from EL1 is UNDEFINED and would take a sync exception into
    // the very vectors under test. So snapshot them HERE, at the last instant they are readable and
    // in their FINAL post-programming values (both writes above are already retired), into RAM the
    // EL1 twin maps Normal-WB on the same core — no barrier needed for a same-core read-back.
    // x1/x2 are dead scratch at this point; x0 is reloaded immediately below and x30 is untouched,
    // so the "no stack traffic, x30 preserved" contract of this stub still holds.
    adrp  x1, {latch}
    add   x1, x1, :lo12:{latch}
    mrs   x2, hcr_el2
    str   x2, [x1]
    mrs   x2, cnthctl_el2
    str   x2, [x1, #8]
    mrs   x2, S3_0_C12_C12_5      // ICC_SRE_EL1 — gates EL1's ICC_* system-register CPU interface
    str   x2, [x1, #16]
    mrs   x2, S3_4_C12_C9_5       // ICC_SRE_EL2
    str   x2, [x1, #24]
    // Land at EL1h (SPx = SP_EL1) with DAIF masked; SP_EL1 = current SP so the stack is continuous;
    // ELR_EL2 = x30 so the eret returns to our caller now running at EL1.
    mov   x0, sp
    msr   sp_el1, x0
    mov   x0, #0x3c5
    msr   spsr_el2, x0
    msr   elr_el2, x30
    isb
    eret
"#,
    latch = sym JM6_EL2_LATCH,
);

/// IRQEL-RT2 — the four EL2-only registers the drop asm latches immediately before its `eret`, in
/// their final post-programming values: `[0]` HCR_EL2, `[1]` CNTHCTL_EL2, `[2]` ICC_SRE_EL1,
/// `[3]` ICC_SRE_EL2. Written once, by that asm, at EL2; read-only afterwards. `AtomicU64` (not
/// `static mut`) so the read-back needs no `unsafe` and no raw-reference lint exception — the layout
/// is a plain `u64` either way, which is what the `str x2, [x1, #N]` above writes.
pub static JM6_EL2_LATCH: [core::sync::atomic::AtomicU64; 4] = [
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
];

/// Read back the JM6 drop's EL2 latch as `(HCR_EL2, CNTHCTL_EL2, ICC_SRE_EL1, ICC_SRE_EL2)`. All
/// zeroes means the drop has not run on this core yet (the statics' initial value), which is itself
/// honest: HCR_EL2 is never legitimately 0 here — the drop always sets RW (bit 31).
pub fn jm6_el2_latch() -> (u64, u64, u64, u64) {
    use core::sync::atomic::Ordering;
    (
        JM6_EL2_LATCH[0].load(Ordering::Relaxed),
        JM6_EL2_LATCH[1].load(Ordering::Relaxed),
        JM6_EL2_LATCH[2].load(Ordering::Relaxed),
        JM6_EL2_LATCH[3].load(Ordering::Relaxed),
    )
}

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
    // ORIN-EL1AP: publish the root we are about to install, for the one AP that will replay this same
    // drop on its own core (see the ORIN-EL1AP block at the file tail). Placed BEFORE the drop rather
    // than after because there is no "after" on this side — `drop_el2_to_el1_tegra` returns to our
    // CALLER, not to us. Knob-off the statement does not exist.
    #[cfg(feature = "orinel1ap")]
    EL1_ROOT.store(l1_pa, core::sync::atomic::Ordering::Release);
    unsafe {
        enable_el1_regime(l1_pa);
        drop_el2_to_el1_tegra();
    }
}

// ══════════════════════════════════════════════════════════════════════════════════════════════════
// ORIN-EL1AP (baton orin-8 item 1, Candidate C) — drop ONE PSCI-woken AP from EL2 to EL1
// ══════════════════════════════════════════════════════════════════════════════════════════════════
//
// THE GAP THIS CLOSES. `sched.rs`'s EL0-EL1CORE filter admits an EL0 task only onto a core that has
// MEASURED itself at EL1 (`EL1_CORE_MASK`, stamped by `sched::mark_el1_core`). On the `smp_virt` path
// only the BSP ever drops, so the mask holds exactly `0x1` — and the BSP is the core running the
// shell, which `run` keeps and `bg` may not. The refusal is therefore correct and permanent: every
// `bg` answers `el0refuse=<n> el1cores=0x1`. Nothing was ever made ELIGIBLE. This makes exactly one
// AP eligible, so `el1cores` gains a second bit and a background EL0 tenant has a core to live on —
// one whose dispatch loop is the full `sched::run()` (`secondary_run`), which unlike the boot core's
// `run_capstone_boot_core` DOES drain due sleepers, so a tenant that sleeps is actually woken.
//
// EXACTLY ONE AP, BY CONSTRUCTION (`EL1AP_SEAT`). The minimum that closes the gap, and the minimum
// blast radius: every other AP stays at EL2 as a scheduler participant with byte-identical behaviour,
// so the SCHED-3/SPREAD-* balance for ordinary kernel tasks across the remaining cores is untouched.
// The seat is a one-shot claim, not a retry loop — a claimant that then fails does NOT hand the seat
// back, because a core that could not complete the drop is evidence about the platform, not a
// transient, and a retry storm across five APs is a worse failure than one refusal line.
//
// IT REUSES THE BSP'S DROP, IT DOES NOT CLONE IT. `enable_el1_regime` + `drop_el2_to_el1_tegra` are
// called verbatim — the same Rust fn, the same naked asm symbol. Every register that sequence writes
// is per-core banked (MAIR/TCR/TTBR0/TTBR1/SCTLR_EL1, CPACR_EL1, VMPIDR/VPIDR_EL2, CPTR_EL2,
// MDCR_EL2, CNTHCTL_EL2, CNTVOFF_EL2, CNTP_CTL_EL0, HCR_EL2, SP_EL1, SPSR_EL2, ELR_EL2), so running
// it on an AP programs THAT AP's copy and nothing else. Two of them are load-bearing here in a way
// they are not on the BSP and are worth naming:
//
//   * `mrs x0, mpidr_el1 ; msr vmpidr_el2, x0` — at EL1 an `MPIDR_EL1` read returns VMPIDR_EL2. The
//     AP seeds its OWN affinity because the asm runs on the AP, so `gic::this_affinity()` (and with
//     it `sgi_target_for_index`, the reschedule poke, `init_secondary_v3`'s redistributor match)
//     keeps answering the truth after the drop. A hand-written second drop is exactly where this
//     would have been got wrong.
//   * `mov x0, sp ; msr sp_el1, x0` — SP_EL1 becomes this AP's own 64 KiB `SECONDARY_STACKS` slot,
//     because that is the SP the AP is running on. No stack is shared and none is switched.
//
// WHAT THE BSP'S SEQUENCE DOES *NOT* PROGRAM, AND WHO DOES IT INSTEAD. `VBAR_EL1` — the caller must
// re-run `exceptions::install()` on the far side (it picks the EL from `CurrentEL` at runtime), and
// `percpu::init()` must re-seed `TPIDR_EL1` (the pre-drop `init` seeded TPIDR_EL2 and TPIDR_EL1 is
// RESET-UNKNOWN). Both are the BSP's own post-drop statements at `main.rs`'s tegra terminus; the AP
// call site repeats them for the same reasons. The drop also lands with DAIF MASKED and CNTP
// DISABLED, which is right for the BSP (cooperative CAPSTONE, no preemption) and wrong for an AP that
// must take its own PPI 30 tick and the reschedule SGI — so the call site re-arms the tick and
// unmasks IRQ. `__vec_irq`'s `irq_bank!`/`irq_unbank!` are RUNTIME `CurrentEL` branches on `tegra`
// (`exceptions.rs`), so an IRQ taken at EL1 on this AP banks ELR_EL1/SPSR_EL1 correctly.
//
// WHY IT WAITS FOR THE BSP INSTEAD OF DERIVING THE ROOT ITSELF. TTBR0_EL1 must be `mmu_tegra`'s
// EL1-PRECISE twin `L1_EL1`, never the live EL2 `L1` (the module header's AP[1]-forces-PXN lesson —
// five dark boots). The AP could not take it from `SEC_CTX`: that captures the EL2 regime, whose
// `ttbr0` is exactly the wrong table. It takes the value from the BSP's own `drop_to_el1` call
// instead, so the AP installs the table the BSP DEMONSTRABLY landed on rather than a second reference
// that could drift — the argument `mmu_tegra_el0::BOOT_ROOT` makes for reading the live register. It
// also means `mmu_tegra.rs` needs no edit at all, which matters beyond tidiness: a fully
// `#[cfg]`-gated append to that file was MEASURED to move the jetson media hash (arch_arm64.md
// §JETSON-EL0).
//
// THE COST OF THAT WAIT, STATED PLAINLY. The BSP publishes at `main.rs`'s drop, which is late — past
// the whole device-probe stretch. The claiming AP therefore stalls before `sched::secondary_run`, so
// for that window ONE core is not yet in `ONLINE_MASK` and is not a placement candidate. Only the
// claimant waits (the seat is taken BEFORE the wait, so every other AP falls straight through to
// today's path), the wait is bounded, and both ends of it are on the wire. Knob-off, none of it
// exists.
//
// FAIL-CLOSED AT EVERY EXIT. Wrong EL, seat already held, root never published, or an eret that
// somehow did not land at EL1 — each returns `false` having stamped NOTHING, so `EL1_CORE_MASK` keeps
// its pre-existing value and EL0 placement stays REFUSED exactly as it is today. There is no partial
// state: the drop is one `eret`, and the stamp happens strictly after it, in the caller. And every
// one of those exits PRINTS — silence must never be indistinguishable from "the code never ran".

/// ORIN-EL1AP — how long the claiming AP will wait for the BSP to publish its EL1 root, in seconds.
/// Generous on purpose: the BSP reaches its drop only after PCIe/USB/SD enumeration, and a budget
/// tight enough to expire during a slow probe would turn a healthy boot into a refusal. Bounded all
/// the same — a BSP that never drops must not park an AP forever.
#[cfg(feature = "orinel1ap")]
const EL1AP_ROOT_WAIT_S: u64 = 30;

/// ORIN-EL1AP — the EL1 root the BSP ACTUALLY installed (`mmu_tegra`'s `L1_EL1` PA), published by
/// [`drop_to_el1`] on its way through. Zero = the BSP has not dropped yet, which is why zero is the
/// value the AP refuses on rather than a sentinel it could mistake for a table at PA 0.
#[cfg(feature = "orinel1ap")]
static EL1_ROOT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

/// ORIN-EL1AP — the single AP-at-EL1 seat. `swap(true)` is the claim; there is no release.
#[cfg(feature = "orinel1ap")]
static EL1AP_SEAT: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// ORIN-EL1AP — the claiming AP's own copy of the four EL2 registers `drop_el2_to_el1_tegra` latches
/// (same order as [`JM6_EL2_LATCH`]: HCR_EL2, CNTHCTL_EL2, ICC_SRE_EL1, ICC_SRE_EL2).
///
/// It exists because the drop asm writes ONE global latch, and the AP runs that asm too. Left alone,
/// an AP drop would overwrite the BSP's IRQEL-RT2 evidence — which `timer.rs`'s `[irqel2a]` witness
/// reads back from RAM precisely BECAUSE those registers are unreadable at EL1. So the AP snapshots
/// the latch before its drop and restores it after, publishing its own four values here instead. The
/// restore is what keeps `[irqel2a]` honest; this array is the AP's half of the same evidence.
#[cfg(feature = "orinel1ap")]
pub static JM6_AP_LATCH: [core::sync::atomic::AtomicU64; 4] = [
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
    core::sync::atomic::AtomicU64::new(0),
];

/// ORIN-EL1AP — read back the claiming AP's EL2 latch as
/// `(HCR_EL2, CNTHCTL_EL2, ICC_SRE_EL1, ICC_SRE_EL2)`. All zeroes means no AP has dropped, which is
/// honest for the same reason [`jm6_el2_latch`]'s is: HCR_EL2 is never legitimately 0 after the drop
/// (it always sets RW, bit 31).
#[cfg(feature = "orinel1ap")]
pub fn jm6_ap_latch() -> (u64, u64, u64, u64) {
    use core::sync::atomic::Ordering;
    (
        JM6_AP_LATCH[0].load(Ordering::Relaxed),
        JM6_AP_LATCH[1].load(Ordering::Relaxed),
        JM6_AP_LATCH[2].load(Ordering::Relaxed),
        JM6_AP_LATCH[3].load(Ordering::Relaxed),
    )
}

/// ORIN-EL1AP — claim the single AP-at-EL1 seat and drop THIS core EL2 -> EL1, reusing the BSP's
/// `enable_el1_regime` + `drop_el2_to_el1_tegra` verbatim. `cpu` is the caller's MPIDR-derived linear
/// index (`smp_virt::__secondary_rust_virt` re-derives it from `gic::this_affinity()` with the MMU on,
/// so it NAMES this core rather than being asserted about it — which is exactly the property
/// `sched::mark_el1_core`'s doc demands of an AP-side stamp).
///
/// Returns `true` having landed at EL1 with DAIF masked and CNTP disabled — the caller MUST then
/// re-seed `percpu::init(cpu)` (TPIDR_EL1), re-run `exceptions::install()` (VBAR_EL1), stamp
/// `sched::mark_el1_core()`, re-arm the tick and unmask IRQ. Returns `false` having changed NOTHING
/// about this core's EL: it is still at EL2, still unstamped, and the caller carries on exactly as an
/// un-knobbed boot does.
///
/// Prints on every path, including every refusal.
#[cfg(feature = "orinel1ap")]
pub unsafe fn drop_ap_to_el1(cpu: usize) -> bool {
    use core::sync::atomic::Ordering;

    // (1) MEASURE the EL before touching the seat. An AP the firmware monitor started at some other
    // EL must not burn the one seat to discover it cannot use it. (`_secondary_start_virt` already
    // parks a non-EL2 core in WFE before it reaches any Rust, so this is belt-and-braces — but it is
    // a register read that cannot fault, and the alternative is an `msr` that UNDEFs.)
    let el: u64;
    unsafe {
        core::arch::asm!("mrs {}, CurrentEL", out(reg) el, options(nomem, nostack, preserves_flags));
    }
    let el = (el >> 2) & 0b11;
    if el != 2 {
        serial_println!(
            ":: tegra: [el1ap] REFUSED cpu={} — CurrentEL={}, not EL2; core stays at EL2 and out of el1cores ::",
            cpu,
            el
        );
        return false;
    }

    // (2) CLAIM BEFORE WAITING, so exactly ONE AP pays the wait. Every other AP is told so and falls
    // straight through to the unchanged path — no stall, no second candidate, no race at the drop.
    if EL1AP_SEAT.swap(true, Ordering::AcqRel) {
        serial_println!(
            ":: tegra: [el1ap] REFUSED cpu={} — the EL1 seat is already held; core stays at EL2 and out of el1cores ::",
            cpu
        );
        return false;
    }

    // (3) Wait (bounded) for the BSP's own drop to publish the root it installed. Off CNTPCT, which
    // free-runs regardless of the timer's enable state and is readable at EL2 without permission.
    let freq = super::timer::cntfrq();
    let freq = if freq == 0 { 62_500_000 } else { freq };
    let deadline = super::timer::cntpct() + freq.saturating_mul(EL1AP_ROOT_WAIT_S);
    serial_println!(
        ":: tegra: [el1ap] seat claimed by cpu={} — waiting up to {} s for the BSP EL1 root ::",
        cpu,
        EL1AP_ROOT_WAIT_S
    );
    let mut root = EL1_ROOT.load(Ordering::Acquire);
    while root == 0 && super::timer::cntpct() < deadline {
        core::hint::spin_loop();
        root = EL1_ROOT.load(Ordering::Acquire);
    }
    if root == 0 {
        serial_println!(
            ":: tegra: [el1ap] REFUSED cpu={} — the BSP EL1 root was not published within {} s (the BSP drop did not complete); core stays at EL2 and out of el1cores ::",
            cpu,
            EL1AP_ROOT_WAIT_S
        );
        return false;
    }

    // (4) ICC_SRE_EL2: set SRE (bit 0) and **Enable** (bit 3) before leaving EL2. `gic.rs`'s
    // `init_cpu_interface_v3` sets SRE only — enough while the core stays at EL2, where nothing traps
    // to itself. `Enable` is the bit that stops an EL1 access to ICC_SRE_EL1 trapping UP into EL2
    // vectors this core is about to abandon; setting it is purely permissive (it removes a trap and
    // grants nothing else), and it is done HERE, in the AP path only, rather than in the shared drop
    // asm, so the BSP's byte-for-byte sequence is untouched. RMW to preserve the rest of the field.
    unsafe {
        let sre2: u64;
        core::arch::asm!("mrs {}, S3_4_C12_C9_5", out(reg) sre2, options(nomem, nostack, preserves_flags));
        core::arch::asm!(
            "msr S3_4_C12_C9_5, {}",
            in(reg) sre2 | (1u64 << 3) | 1u64,
            options(nomem, nostack, preserves_flags)
        );
        core::arch::asm!("isb", options(nomem, nostack, preserves_flags));
    }

    // (5) Snapshot the shared drop latch — our asm is about to overwrite it (see `JM6_AP_LATCH`).
    let saved = [
        JM6_EL2_LATCH[0].load(Ordering::Relaxed),
        JM6_EL2_LATCH[1].load(Ordering::Relaxed),
        JM6_EL2_LATCH[2].load(Ordering::Relaxed),
        JM6_EL2_LATCH[3].load(Ordering::Relaxed),
    ];

    // The last line printable with this core's EL2 identity intact.
    serial_println!(
        ":: tegra: [el1ap] cpu={} dropping EL2 -> EL1 (TTBR0_EL1={:#x}, the BSP-installed L1_EL1) ::",
        cpu,
        root
    );

    // (6) THE DROP — the BSP's, reused. `enable_el1_regime` arms the EL1&0 regime with the MMU on
    // (dormant while we remain at EL2), then the naked asm erets to our caller now at EL1.
    unsafe {
        enable_el1_regime(root);
        drop_el2_to_el1_tegra();
    }

    // ── Everything below runs at EL1. TPIDR_EL1 is still UNKNOWN, so NOTHING here may resolve
    //    `percpu::this_cpu()`. `serial_println!` does not (the BSP prints its own JM6b landing proof
    //    before its `percpu::init(0)` for the same reason), and the latch move is plain atomics. ──

    // (7) Publish our four values and give the BSP its evidence back.
    for i in 0..4 {
        JM6_AP_LATCH[i].store(JM6_EL2_LATCH[i].load(Ordering::Relaxed), Ordering::Relaxed);
        JM6_EL2_LATCH[i].store(saved[i], Ordering::Relaxed);
    }

    // (8) MEASURE where we actually landed. Unreachable in practice — an `eret` to EL1h either lands
    // or the core is gone — but the stamp downstream is only sound if this is EL1, and a measured
    // answer costs one `mrs`.
    let el: u64;
    unsafe {
        core::arch::asm!("mrs {}, CurrentEL", out(reg) el, options(nomem, nostack, preserves_flags));
    }
    let el = (el >> 2) & 0b11;
    if el != 1 {
        serial_println!(
            ":: tegra: [el1ap] REFUSED cpu={} — post-eret CurrentEL={}, not EL1; core stays out of el1cores ::",
            cpu,
            el
        );
        return false;
    }
    true
}
