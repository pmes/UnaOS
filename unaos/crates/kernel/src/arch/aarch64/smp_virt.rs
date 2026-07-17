// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// aarch64 SMP bring-up via **PSCI CPU_ON** — QEMU `virt`/UEFI (Arc JC2) and Jetson Orin Nano /
// Tegra234 silicon (Arc JM5). The sibling of `smp.rs` (the Pi 4 bare-metal spin-table path); the two
// differ in how a secondary is *released* and the EL/MMU state it inherits:
//
//   * Pi bare-metal (`smp.rs`, `baremetal`-gated): the GPU firmware parks cores 1-3 in a spin-table;
//     we write our entry into their release slots and `SEV`. Each core arrives at EL2, MMU off, and
//     re-runs the same `boot::drop_to_el1` + `boot::enable_mmu` the BSP built its own tables with.
//   * virt / UEFI and Orin metal (this file, `not(feature = "pi")`): firmware starts only the BSP; the
//     other cores sit in PSCI-off state. We start them with the standardized **PSCI `CPU_ON`** SMC
//     (Orin's boot chain is ATF/BL31 + OP-TEE, so SMC reaches its PSCI too). A PSCI-woken core comes up
//     through a CPU *reset* — MMU off, caches off, DAIF masked, at **EL2** — with only its entry point
//     and context id defined (DEN0022). It does NOT inherit the BSP's live system registers. But the
//     firmware (UEFI, or JM3's `mmu_tegra`) already built a full identity map the BSP runs on, so rather
//     than build our own tables we **capture the BSP's live EL2 regime** (`SEC_CTX`) and each secondary
//     *replays* it to join the same address space (`enable_mmu_virt`).
//
// ## Core identity: linear index vs MPIDR affinity (the JM5 generalization)
//
// QEMU `virt` is a single cluster (MPIDR Aff0 = core index, Aff1..3 = 0). **Tegra234 is multi-cluster**
// (3 clusters × 4 Cortex-A78AE): the cluster is in Aff2/Aff1 and **Aff0 is always 0**, so Aff0 is *not*
// a usable core id. So each core is given a **linear index** 0..N-1 (BSP = 0, secondaries 1..) that is
// dense and board-independent; the linear index is what selects a core's stack / per-CPU block /
// `CORE_READY` slot, and it is handed to the woken core as the PSCI **context id** (delivered in x0).
// The core's **MPIDR affinity** (packed {Aff3,Aff2,Aff1,Aff0}) is a *separate* value, used only as the
// PSCI `CPU_ON` target and as the `gic::send_sgi` target so an SGI reaches the right core across
// clusters. On `virt` the two coincide (affinity == index), so the path is byte-compatible there.
//
// The present cores (and their real affinities) are discovered at runtime by enumerating the GIC
// redistributor frames (`gic::enumerate_redistributor_affinities`) — metal truth read straight from the
// silicon, so the fused Orin core set is found without the firmware DTB (whose `fdt` parse is blocked,
// task cde963a7). This is also the first code to walk a *non-first* redistributor frame on Orin, i.e.
// the first metal exercise of JM4's VLPIS-derived stride.
//
// The scheduler is NOT part of this arc (it is `baremetal`-gated + EL1-coupled, while this path runs at
// EL2 — see JC2/JC3). So the secondaries do their per-core GICv3 bring-up and **park in a WFI loop with
// IRQs unmasked** — able to receive SGIs, nothing more. The verdict is cross-core SGI (BSP → each AP,
// and each AP → BSP), not CAPSTONE.
//
// The whole module is `#[cfg(not(feature = "pi"))]` (baremetal implies pi, so it is compiled out of
// every Pi image). The `virt` kick-off in `main.rs` is additionally runtime-gated on `gic::is_v3()`, so
// the plain GICv2 `virt` run stays single-core and byte-identical to baseline; the tegra kick-off is in
// `tegra_early_stop` (after the JM4 GIC/timer bring-up).

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use super::{cache, exceptions, gic, percpu, sched, timer};

/// Per-core slot cap for the static arrays (stacks, `CORE_READY`, the enumeration buffer). The Orin
/// build sizes it to 8 (covers the Nano's 6 cores + headroom, matching `percpu::NUM_CPUS`); QEMU `virt`
/// keeps 4 (its `-smp 4`). It is `tegra`-gated rather than a flat 8 so the `virt` binary — and thus the
/// virt-GICv2 serial log — stays **byte-identical** to baseline (the 64 KiB-per-slot `SECONDARY_STACKS`
/// array would otherwise grow the BSS by 256 KiB and shift every address printed after it). The number
/// of cores actually brought up is discovered at runtime (redistributor enumeration) and is ≤ this;
/// unused slots are zeroed BSS. Core 0 is always the BSP.
#[cfg(feature = "tegra")]
const MAX_CORES: usize = 8;
#[cfg(not(feature = "tegra"))]
const MAX_CORES: usize = 4;

/// SGI 0 — the inter-processor channel used for the cross-core delivery proof (the same INTID the Pi
/// path reserves as `smp::IPI_RESCHED`; there is no scheduler here, so it is only ever a proof ping).
/// Per-AP distinct SGI INTIDs (attributable AP→BSP delivery, delta-list item) are deferred to a
/// follow-up — the BSP→AP direction is already per-core attributable via each AP's own IPI counter.
const IPI_SGI: u32 = 0;

/// PSCI `CPU_ON` (64-bit) function identifier (Arm DEN0022, PSCI 1.0). x1 = target MPIDR (affinity
/// form), x2 = entry point PA, x3 = context id (delivered in the secondary's x0). Returns 0 = SUCCESS,
/// else a negative error (-2 INVALID_PARAMS, -4 ALREADY_ON, ...).
const PSCI_CPU_ON: u64 = 0xC400_0003;

/// PSCI `AFFINITY_INFO` (64-bit) function identifier (DEN0022). x1 = target MPIDR (affinity form), x2 =
/// lowest affinity level (0 = core). Returns 0 = ON, 1 = OFF, 2 = ON_PENDING for a core the firmware
/// knows, or a negative error (-2 INVALID_PARAMS) for an affinity that is not a valid, present PE. It is
/// a pure query of the firmware's topology table — it does NOT touch the core's power/reset hardware —
/// so it is safe to probe every enumerated redistributor frame with it. **Load-bearing on Orin metal**
/// (JM5 attempt 1): the Tegra234 GIC-600 exposes redistributor frames for the whole die's core slots
/// (8), but the Nano is a 6-core part — a `CPU_ON` to a fuse-disabled phantom core is a *fatal firmware
/// RAS Uncorrectable Error* ("CBB Interface Error / Error response from slave" → the core powers off).
/// So each enumerated core is gated behind `AFFINITY_INFO` and only started if it reports valid.
const PSCI_AFFINITY_INFO: u64 = 0xC400_0004;

/// Set true by each secondary once its MMU + vectors + per-CPU + GICv3 CPU interface + IPI SGI are up
/// and IRQs are unmasked. Indexed by **linear core index**. The BSP waits on this (Acquire) before it
/// sends any BSP → AP ping, so a ping can never be lost to a not-yet-receptive core. Release on the AP
/// pairs with the BSP's Acquire.
static CORE_READY: [AtomicBool; MAX_CORES] = [const { AtomicBool::new(false) }; MAX_CORES];

/// The BSP's own MPIDR affinity (packed {Aff3,Aff2,Aff1,Aff0}), published before the first `CPU_ON` so a
/// woken AP can target the BSP by affinity for its AP → BSP ping. On `virt` this is 0 (Aff0=0), so the
/// AP → BSP path is byte-identical to the old hardcoded `send_sgi(0, …)`.
static BSP_AFFINITY: AtomicU32 = AtomicU32::new(0);

/// The packed MPIDR affinity of each **linear core index** (`AFF_BY_INDEX[k]` = the affinity of linear
/// core `k`; index 0 = the BSP), published by the BSP before the first `CPU_ON`. A woken secondary uses
/// it to recover its OWN linear index by matching its live `MPIDR_EL1` affinity — the structural fix for
/// the CORE3-class hazard: the PSCI context id delivered in x0 is spilled to the stack with the MMU OFF
/// and can be reloaded cacheable-stale after `enable_mmu_virt` on real silicon (QEMU-invisible; see
/// `__secondary_rust_virt`). Re-deriving the index MMU-ON deletes that stale-line window. Aff0 is NOT a
/// core id on multi-cluster Tegra234 (Aff0=0 on every core, the cluster is in Aff2/Aff1), so the match is
/// on the FULL packed affinity — never a bare Aff0 mask. `N_CORES_PUB`'s Release publishes the whole
/// table; a secondary's Acquire load of it orders the affinity reads after the BSP's stores.
static AFF_BY_INDEX: [AtomicU32; MAX_CORES] = [const { AtomicU32::new(0) }; MAX_CORES];
static N_CORES_PUB: AtomicU32 = AtomicU32::new(0);

/// One secondary's boot/idle stack (64 KiB). AArch64 SP must stay 16-aligned; `align(16)` + a
/// power-of-two size guarantee it. Lives in BSS (zeroed by the loader). Slot 0 is unused (the BSP has
/// the firmware's stack); each secondary takes the slot for its **linear index**, computed by
/// `_secondary_start_virt` from the PSCI context id in x0.
const SEC_STACK_SIZE: usize = 0x1_0000; // 64 KiB
#[repr(C, align(16))]
struct SecStack([u8; SEC_STACK_SIZE]);
static mut SECONDARY_STACKS: [SecStack; MAX_CORES] =
    [const { SecStack([0; SEC_STACK_SIZE]) }; MAX_CORES];

/// The BSP's live EL2 state a secondary must replay to run the same identity map with FP enabled and
/// under the same translation regime. Captured once by `capture_secondary_ctx`, cleaned to the Point of
/// Coherency, then read by each secondary with its **MMU off** (non-cacheable, straight from PoC —
/// hence the clean). Plain `u64` fields read/written volatile; no locks/atomics (the secondary reads
/// this before it can touch either). `align(64)` keeps it inside one cache line so a single
/// `clean_range` publishes it.
///
/// * `hcr` (JM5, Part B): a PSCI-reset core resets with an UNKNOWN `HCR_EL2.E2H`/`TGE`; if it differed
///   from the BSP's, the AP would interpret the replayed `TCR`/`TTBR`/`SCTLR_EL2` under the wrong
///   translation regime (QEMU-invisible; JM3 confirmed the Orin BSP is E2H=0). The AP forces `HCR_EL2`
///   to the BSP's value **first**, in the entry stub, before any of the translation registers.
/// * `cptr` (JC2): the FP-enable a PSCI-reset core does not inherit — the kernel is `+neon`, so ordinary
///   Rust (fmt in the AP's first `serial_println!`, memcpy) autovectorizes, and `CPTR_EL2.TFP` traps
///   EL2 FP to EL2 with the AP's VBAR still the firmware's → the trap would kill the AP before it prints.
///   JM5 replays it in the entry stub (Part C), before any compiler-generated code, not by compiler luck.
#[repr(C, align(64))]
#[derive(Clone, Copy)]
struct SecondaryCtx {
    mair: u64,
    tcr: u64,
    ttbr0: u64,
    sctlr: u64,
    cptr: u64,
    hcr: u64,
}
static mut SEC_CTX: SecondaryCtx =
    SecondaryCtx { mair: 0, tcr: 0, ttbr0: 0, sctlr: 0, cptr: 0, hcr: 0 };

// The secondary entry stub. Runs at EL2 with the MMU OFF. x0 = PSCI context id = **linear core index**,
// used HERE only to pick this core's stack (MPIDR Aff0 is not a core id on multi-cluster silicon, so we
// do NOT read MPIDR in the stub). NOTE: x0 is ADVISORY past the MMU turn-on — `__secondary_rust_virt`
// re-derives the index from MPIDR affinity once the MMU is on, because the compiler spills this MMU-off
// argument to the stack and a cacheable reload of it can hit a stale line on >1 MiB images (the
// CORE3-SMP hazard; see `__secondary_rust_virt` + arch_arm64.md §ORIN-SMP). The asm is unchanged.
// It sets SP to this core's stack top, then replays — *before any compiler-generated code* — the two
// EL2 regime bits a reset core does not inherit and that gate everything after: `HCR_EL2` (Part B, so
// the Rust translation-register replay is interpreted under the BSP's E2H/TGE) then `CPTR_EL2` (Part C,
// so the first `+neon` instruction the compiler may emit does not trap). Both come from `SEC_CTX`, which
// the BSP cleaned to PoC — this MMU-off read is straight from RAM. The MMU itself is enabled in Rust
// (`enable_mmu_virt`). Absolute symbol references resolve because the map is identity (VA == PA).
core::arch::global_asm!(
    r#"
    .globl _secondary_start_virt
    _secondary_start_virt:
        mov   x19, x0                // x19 = linear core index (PSCI context id in x0)
        adrp  x1, {stacks}
        add   x1, x1, #:lo12:{stacks}
        mov   x2, #({size} >> 12)
        lsl   x2, x2, #12            // x2 = SEC_STACK_SIZE
        madd  x3, x19, x2, x1        // x3 = &SECONDARY_STACKS + index*size
        add   x3, x3, x2             //   + size  => top of this core's stack
        mov   sp, x3
        // The whole replay below is EL2 (the BSP's regime; SEC_CTX holds *_EL2 registers). PSCI wakes a
        // core at the caller's EL, so we expect EL2 — but if the firmware monitor instead drops this AP
        // to EL1, `msr hcr_el2`/etc would UNDEF and could wedge the board. Guard it: at any EL != 2, park
        // in WFE without touching an EL2 register, which the BSP observes as a clean CORE_READY timeout.
        mrs   x7, CurrentEL
        cmp   x7, #(2 << 2)
        b.ne  1f
        adrp  x4, {sec_ctx}
        add   x4, x4, #:lo12:{sec_ctx}
        ldr   x5, [x4, #{hcr_off}]   // HCR_EL2 first: fix E2H/TGE before the translation-register replay
        msr   hcr_el2, x5
        isb
        ldr   x6, [x4, #{cptr_off}]  // CPTR_EL2: un-trap FP/SIMD before any +neon-autovectorized code
        msr   cptr_el2, x6
        isb
        mov   x0, x19               // arg0 = linear core index
        bl    {entry}               // __secondary_rust_virt(core) — never returns
    1:  wfe
        b     1b
    "#,
    stacks = sym SECONDARY_STACKS,
    sec_ctx = sym SEC_CTX,
    entry = sym __secondary_rust_virt,
    size = const SEC_STACK_SIZE,
    hcr_off = const core::mem::offset_of!(SecondaryCtx, hcr),
    cptr_off = const core::mem::offset_of!(SecondaryCtx, cptr),
);

unsafe extern "C" {
    fn _secondary_start_virt();
}

/// Join the BSP's EL2 translation regime, MMU still OFF on entry. `HCR_EL2` and `CPTR_EL2` are already
/// replayed by the entry stub (before any compiler code); this reads the captured `SEC_CTX` (non-cacheable,
/// straight from RAM — the BSP cleaned it to PoC) and programs the translation registers, then turns the
/// MMU on. Called as the very first thing in `__secondary_rust_virt`, before any lock/atomic (a spinlock's
/// `ldxr/stxr` is CONSTRAINED UNPREDICTABLE with the MMU off). System-register moves only.
///
/// Order: MAIR/TCR/TTBR0, a `tlbi alle2` to drop cold-reset TLB state, `dsb sy; isb`, then SCTLR_EL2
/// (whose captured value has M=1 → MMU on) and a final `isb`. The map is identity, so enabling
/// translation moves neither PC nor SP. This is the EL2 analogue of the proven baremetal
/// `boot::enable_mmu` (EL1).
unsafe fn enable_mmu_virt() {
    let c = unsafe { core::ptr::read_volatile(&raw const SEC_CTX) };
    unsafe {
        core::arch::asm!(
            "msr MAIR_EL2, {mair}",
            "msr TCR_EL2,  {tcr}",
            "msr TTBR0_EL2, {ttbr0}",
            "tlbi alle2",             // drop cold-reset EL2 TLB state before translation is enabled
            "dsb sy",
            "isb",
            "msr SCTLR_EL2, {sctlr}", // captured M=1/C/I (+ SA/EE/RES1) → MMU + caches on
            "isb",
            mair = in(reg) c.mair,
            tcr = in(reg) c.tcr,
            ttbr0 = in(reg) c.ttbr0,
            sctlr = in(reg) c.sctlr,
            options(nostack, preserves_flags),
        );
    }
}

/// Rust entry for a PSCI-started secondary. Called from `_secondary_start_virt` with the MMU still OFF
/// and this core's stack set. The incoming `_advisory` argument (x0 = the PSCI context id the stub used
/// to select this core's stack) is **advisory only past the MMU turn-on** and deliberately ignored for
/// everything after: on real silicon the compiler spills that MMU-off argument to the stack
/// (`str x0, [sp]`) and reloads it CACHEABLE after `enable_mmu_virt` (for the `serial_println!` — which
/// takes it by-reference — and the `CORE_READY` index), so it can hit a stale line the way the Pi
/// CORE3-SMP regression did (mismatched-attributes coherency; QEMU-invisible, image-layout-deterministic;
/// see smp.rs / arch_arm64.md §CORE3-SMP + §ORIN-SMP). We re-derive this core's linear index FRESH from
/// `MPIDR_EL1` AFTER the MMU is on, matching the full packed affinity against the BSP-published
/// `AFF_BY_INDEX` (Aff0 alone is not a core id on multi-cluster Tegra234), so every store/load of the id
/// is cacheable-coherent. Brings the core up to parity with the BSP's per-core state, then parks in WFI
/// able to service SGIs. Never returns.
#[unsafe(no_mangle)]
extern "C" fn __secondary_rust_virt(_advisory: u64) -> ! {
    // MMU on FIRST (replaying the BSP's EL2 regime), before any lock/atomic/FP. `serial_println!` below
    // takes a spinlock, so nothing may print before this. (HCR_EL2/CPTR_EL2 were replayed in the stub.)
    unsafe { enable_mmu_virt() };
    // Re-derive this core's linear index from MPIDR affinity with the MMU ON — the structural fix for the
    // CORE3-class stale-line hazard. `this_affinity()` reads MPIDR_EL1 live (at EL2 it returns the
    // physical affinity — VMPIDR only redirects EL1 reads), so this and every use of the derived index
    // execute cacheable-coherent, with no MMU-off store / MMU-on reload for a stale line to poison. The
    // match is on the full packed affinity, never a bare Aff0 mask (Aff0=0 on every Tegra234 core).
    let aff = gic::this_affinity();
    let n = N_CORES_PUB.load(Ordering::Acquire) as usize;
    let mut derived = usize::MAX;
    let mut i = 1; // a secondary is never index 0 (the BSP); start the search past it
    while i < n && i < MAX_CORES {
        if AFF_BY_INDEX[i].load(Ordering::Relaxed) == aff {
            derived = i;
            break;
        }
        i += 1;
    }
    // Unknown/garbage affinity (a core the BSP never published, or index 0/out of range): park in a
    // low-power WFE loop rather than index CORE_READY/percpu out of bounds. A parked core is exactly the
    // graceful failure the BSP times out on (WARNING … did not come online), never worse. No lock or
    // shared state is touched before this point (only the MMU replay + the affinity reads).
    if derived == 0 || derived >= MAX_CORES {
        loop {
            unsafe { core::arch::asm!("wfe", options(nomem, nostack, preserves_flags)) };
        }
    }
    let core = derived; // linear index; indexes the stack / per-CPU block / CORE_READY slot
    // Per-core VBAR_EL2 (+ HCR_EL2 IMO/FMO/AMO so a physical IRQ taken at EL2 targets EL2) — matches the
    // BSP; installed before IRQs are unmasked.
    exceptions::install();
    // Per-CPU block reachable via TPIDR_EL2, indexed by the linear core id (the IPI handler resolves its
    // counter through this).
    percpu::init(core);
    // This core's GICv3 redistributor (wake + banked Group 1) and its system-register CPU interface —
    // both banked, so the BSP's setup does not cover them. `init_secondary_v3` resolves THIS core's
    // redistributor by matching its MPIDR affinity against GICR_TYPER (the Tegra234 GICR base + stride
    // come from `gic.rs`). Then enable the IPI SGI (banked in this core's redistributor) and unmask IRQ.
    gic::init_secondary_v3();
    gic::enable_sgi(IPI_SGI);
    exceptions::enable_irq();

    serial_println!(
        ":: AARCH64 SMP: AP {} online (aff={:#010x}) ::",
        core,
        gic::this_affinity()
    );
    // Publish readiness AFTER everything above (Release pairs with the BSP's Acquire): the BSP only
    // sends a BSP → AP ping once this core can actually take it.
    CORE_READY[core].store(true, Ordering::Release);

    // AP → BSP proof: ping the BSP once, targeting it by its published affinity (0 on `virt`). The BSP
    // enabled SGI 0 and unmasked IRQ before any CPU_ON, so it is already receptive; it counts these to
    // confirm the reverse direction.
    gic::send_sgi(BSP_AFFINITY.load(Ordering::Acquire) as usize, IPI_SGI);

    // SCHED-NEXT busy-heartbeat: before parking idle, run ONE bounded cooperative pass over a queue the
    // BSP staged for this core (released via `secondary_work_go`). This publishes honest BUSY telemetry
    // (`CPU_BUSY[core] > 0`) — the other half of the idle-heartbeat below, which alone only proves a
    // parked core reads idle. Cooperative (yield/exit tasks, no timer, no WFI) so it is safe on this
    // EL2 secondary; the pass drains a finite pre-staged queue and returns, then the core parks as
    // before. The spin-for-release inside waits with IRQ unmasked, so the BSP→AP ping still lands.
    sched::run_secondary_work(core);

    // Honest idle heartbeat (VUG-1 M3b): this core is online but parks WITHOUT running the scheduler,
    // so it never calls `dispatch_next` and its CPU-pulse counters would stay (0,0) — a pinned/undefined
    // meter bar for a demonstrably-online-idle core. Register it as idle so the bar reads honest 0% busy.
    // Bump once at park entry AND on every WFI wake — the wake bump is the load-bearing one (the BSP's
    // witness reads busy+idle>0, which each re-park guarantees; the entry bump just seeds it). IRQs are
    // already unmasked, so the BSP→AP ping is itself such a wake. Introspection-only, lock-free relaxed
    // — no scheduling-path effect.
    sched::note_core_idle(core);
    // Park: IRQs unmasked, so a BSP → AP SGI wakes this WFI, is serviced (handle_irq_v3 counts it), and
    // the core re-parks. No scheduler on this path (see the module header).
    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack, preserves_flags)) };
        sched::note_core_idle(core);
    }
}

/// Issue a PSCI fast call via the **SMC** conduit and return x0 (0/positive = result, else negative
/// error). SMC is the conduit that reaches the emulated/firmware PSCI from our EL2 kernel: QEMU
/// `virt,virtualization=on` advertises `method = "smc"` and intercepts the SMC in TCG regardless of EL3;
/// the Orin's ATF/BL31 monitor at EL3 services the SMC directly. (An `hvc` from EL2 would be taken to
/// our own EL2 vector instead.)
///
/// SMCCC (DEN0028): a fast call returns results in x0-x3 and preserves only x18-x30 + SP — so x0-x17 are
/// volatile. x1-x3 are marked clobbered outputs, x4-x17 clobbers. No `nomem`: PSCI calls have global
/// side effects (a `CPU_ON` makes the BSP's prior stores to SEC_CTX/stacks matter on another core), and
/// their ordering vs the `dsb sy`-terminated `clean_range`s must be preserved.
fn psci_call(func: u64, x1: u64, x2: u64, x3: u64) -> i64 {
    let mut x0 = func;
    unsafe {
        core::arch::asm!(
            "smc #0",
            inout("x0") x0,
            inout("x1") x1 => _,
            inout("x2") x2 => _,
            inout("x3") x3 => _,
            out("x4") _, out("x5") _, out("x6") _, out("x7") _,
            out("x8") _, out("x9") _, out("x10") _, out("x11") _,
            out("x12") _, out("x13") _, out("x14") _, out("x15") _,
            out("x16") _, out("x17") _,
            options(nostack),
        );
    }
    x0 as i64
}

/// PSCI `CPU_ON`: start the core at `target_mpidr` executing at `entry` with `context_id` in its x0.
fn psci_cpu_on(target_mpidr: u64, entry: u64, context_id: u64) -> i64 {
    psci_call(PSCI_CPU_ON, target_mpidr, entry, context_id)
}

/// PSCI `AFFINITY_INFO` at core level: is `target_mpidr` a valid, present PE? Returns 0 (ON) / 1 (OFF) /
/// 2 (ON_PENDING) for a known core, or a negative error (-2 INVALID_PARAMS) for an affinity the firmware
/// does not populate — the safe presence check that keeps a `CPU_ON` off a fuse-disabled phantom core.
fn psci_affinity_info(target_mpidr: u64) -> i64 {
    psci_call(PSCI_AFFINITY_INFO, target_mpidr, 0, 0)
}

/// Convert a packed GICR/`MPIDR`-contiguous affinity {Aff3[31:24],Aff2[23:16],Aff1[15:8],Aff0[7:0]} —
/// the form `gic::this_affinity()`/`enumerate_redistributor_affinities` return — into the **MPIDR/PSCI**
/// layout, which places Aff3 at bits[39:32] (Aff2..Aff0 stay in bits[23:0]). Identity for Aff3=0 (all of
/// QEMU `virt` and Tegra234, whose top cluster is Aff2=2), but correct-by-construction for Aff3≠0.
#[inline]
fn affinity_to_mpidr(packed: u32) -> u64 {
    let p = packed as u64;
    (p & 0x00FF_FFFF) | (((p >> 24) & 0xFF) << 32)
}

/// Capture the BSP's live EL2 translation + FP-enable + regime state into `SEC_CTX` for the secondaries
/// to replay. Read directly from the running BSP's system registers, so whatever the firmware/JM3
/// configured (TCR_EL2 RES1 bits, SCTLR_EL2 SA/EE, a permissive CPTR_EL2, HCR_EL2.E2H) is carried
/// faithfully — no hand-constructed bits. BSP-only, MMU on.
fn capture_secondary_ctx() {
    let (mair, tcr, ttbr0, sctlr, cptr, hcr): (u64, u64, u64, u64, u64, u64);
    unsafe {
        core::arch::asm!("mrs {}, MAIR_EL2", out(reg) mair, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, TCR_EL2", out(reg) tcr, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, TTBR0_EL2", out(reg) ttbr0, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, SCTLR_EL2", out(reg) sctlr, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, CPTR_EL2", out(reg) cptr, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, HCR_EL2", out(reg) hcr, options(nomem, nostack, preserves_flags));
        core::ptr::write_volatile(
            &raw mut SEC_CTX,
            SecondaryCtx { mair, tcr, ttbr0, sctlr, cptr, hcr },
        );
    }
}

/// Confirm + log the PSCI conduit. On the EL2 `virt`/Orin path the conduit is SMC regardless of what the
/// DTB `method` says for an EL1-guest view, so this is informational. `dtb_addr == 0` (the tegra caller)
/// skips the FDT parse entirely — the `fdt-0.1.5` parse of the real Orin DTB panics (task cde963a7), and
/// SMC is used unconditionally — printing only the assumed-SMC line.
fn report_conduit(dtb_addr: u64, dtb_size: usize) {
    if dtb_addr != 0 {
        unsafe {
            let slice = core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size);
            if let Ok(fdt) = fdt::Fdt::new(slice) {
                if let Some(node) = fdt.find_node("/psci") {
                    if let Some(p) = node.property("method") {
                        let m = core::str::from_utf8(p.value).unwrap_or("?").trim_end_matches('\0');
                        serial_println!(
                            ":: AARCH64 SMP: PSCI conduit=SMC (EL2); DTB /psci method=\"{}\" ::",
                            m
                        );
                        return;
                    }
                }
            }
        }
    }
    serial_println!(":: AARCH64 SMP: PSCI conduit=SMC (EL2; /psci method not parsed, assumed) ::");
}

/// Busy-wait until `deadline` (a CNTPCT value) for `cond`, returning whether it held. Bounds every SMP
/// handshake so a wedged core can never hang boot (mirrors the mailbox/xHCI/smp.rs deadline pattern).
fn wait_until(deadline: u64, mut cond: impl FnMut() -> bool) -> bool {
    loop {
        if cond() {
            return true;
        }
        if timer::cntpct() >= deadline {
            return false;
        }
        core::hint::spin_loop();
    }
}

/// BSP: discover the present cores, bring every secondary online via PSCI `CPU_ON`, then prove the GICv3
/// cross-core IPI path in both directions. Called from `main.rs` (virt) / `tegra_early_stop` (Orin) only
/// when `gic::is_v3()`.
///
/// Publication before start: the secondaries read `SEC_CTX` with their MMU OFF (non-cacheable), so it is
/// cleaned to PoC; `SECONDARY_STACKS` is clean+invalidated so the loader's *cacheable* BSS-zero lines
/// can't later evict-clobber a secondary's MMU-off stack writes (a no-op in QEMU, load-bearing on metal).
/// Both complete (`dsb sy`) before the first `CPU_ON`.
pub fn start_secondaries(dtb_addr: u64, dtb_size: usize) {
    // Publish the BSP's affinity so a woken AP can target it, and enable SGI 0 on the BSP's own CPU
    // interface so it can RECEIVE the AP → BSP pings. Its IRQs are already unmasked (arch/gic init).
    let bsp_aff = gic::this_affinity();
    BSP_AFFINITY.store(bsp_aff, Ordering::Release);
    gic::enable_sgi(IPI_SGI);

    // Capture the EL2 regime, then publish it (and the secondary stacks) for MMU-off consumers.
    capture_secondary_ctx();
    cache::clean_range(&raw const SEC_CTX as usize, core::mem::size_of::<SecondaryCtx>());
    cache::clean_invalidate_range(
        &raw const SECONDARY_STACKS as usize,
        core::mem::size_of::<[SecStack; MAX_CORES]>(),
    );

    report_conduit(dtb_addr, dtb_size);

    // Discover the present cores from the GIC redistributors (metal truth). Assign dense linear indices:
    // BSP = 0, each *other* present core = 1,2,… in walk order. `aff_by_index[k]` = the affinity of
    // linear core k (its PSCI/SGI target); the BSP is index 0.
    let mut present = [0u32; MAX_CORES];
    let n_present = gic::enumerate_redistributor_affinities(&mut present);
    let mut aff_by_index = [0u32; MAX_CORES];
    aff_by_index[0] = bsp_aff;
    let mut n_cores = 1usize; // includes the BSP
    for &a in present.iter().take(n_present) {
        if a == bsp_aff {
            continue; // the BSP is already running (linear index 0)
        }
        if n_cores >= MAX_CORES {
            serial_println!(
                ":: AARCH64 SMP: NOTE more cores present than the {}-slot cap; extra ignored ::",
                MAX_CORES
            );
            break;
        }
        aff_by_index[n_cores] = a;
        n_cores += 1;
    }
    let n_enumerated_aps = n_cores - 1;
    // Dump every enumerated affinity up front — BEFORE any PSCI call — so the metal capture has the full
    // set even if a later probe/CPU_ON faults (JM5 attempt 1 lost them to a RAS fault).
    for idx in 0..n_cores {
        serial_println!(
            ":: AARCH64 SMP: enumerated core {} aff={:#010x}{} ::",
            idx,
            aff_by_index[idx],
            if idx == 0 { " (BSP)" } else { "" }
        );
    }

    // Publish the linear-index → affinity table BEFORE any CPU_ON, so a woken secondary can recover its
    // OWN linear index by matching its live MPIDR affinity (the CORE3-class fix — `__secondary_rust_virt`
    // no longer trusts the MMU-off-spilled context id). The per-slot Relaxed stores are ordered before a
    // secondary's reads by the `N_CORES_PUB` Release below pairing with the secondary's Acquire; the
    // stack/SEC_CTX cache maintenance already issued (`dsb sy`) also precedes the first CPU_ON.
    for idx in 0..n_cores {
        AFF_BY_INDEX[idx].store(aff_by_index[idx], Ordering::Relaxed);
    }
    N_CORES_PUB.store(n_cores as u32, Ordering::Release);

    // Presence gate (JM5 attempt-1 fix): the Tegra234 GIC-600 exposes redistributor frames for the whole
    // die's core slots, but the Nano is a 6-core part — a `CPU_ON` to a fuse-disabled phantom core is a
    // *fatal firmware RAS Uncorrectable Error*. `AFFINITY_INFO` is a safe topology query (it never touches
    // the core's power hardware) that returns a valid state (0/1/2) for a present core and a negative
    // error for a phantom. Only cores it reports present are startable. On QEMU `virt` all enumerated
    // cores are present, so this is a no-op there (still 3/3).
    let mut startable = [false; MAX_CORES];
    let mut n_startable = 0usize;
    for idx in 1..n_cores {
        let info = psci_affinity_info(affinity_to_mpidr(aff_by_index[idx]));
        startable[idx] = info >= 0; // 0=ON, 1=OFF, 2=ON_PENDING; negative = INVALID_PARAMS (absent)
        if startable[idx] {
            n_startable += 1;
        }
        serial_println!(
            ":: AARCH64 SMP: core {} (aff={:#010x}) AFFINITY_INFO={} -> {} ::",
            idx,
            aff_by_index[idx],
            info,
            if startable[idx] { "present" } else { "absent (phantom, skipped)" }
        );
    }
    serial_println!(
        ":: AARCH64 SMP: walk found {} core(s); BSP aff={:#010x}; {} enumerated secondary(ies), {} present -> starting ::",
        n_present,
        bsp_aff,
        n_enumerated_aps,
        n_startable
    );

    let freq = timer::cntfrq();
    let freq = if freq == 0 { 62_500_000 } else { freq };
    let bsp_ipi_before = percpu::cpu(0).ipis.load(Ordering::Acquire);

    // SCHED-NEXT busy-heartbeat: arm cooperative secondary work BEFORE any CPU_ON, so every secondary
    // that comes online observes it and waits (generously) for our staged-work release rather than
    // parking idle. This is the `virt`-only path; `start_secondaries_tegra` never arms, so real Orin
    // secondaries skip the wait entirely (see `sched::run_secondary_work`).
    sched::arm_secondary_work();

    // Start each PRESENT secondary. Target = its real MPIDR affinity; context id = its linear index
    // (delivered in x0, used by the entry stub for the stack + as the per-CPU index). Entry PA = the stub
    // symbol (identity-mapped). A CPU_ON error → log + skip, never hang.
    let entry = _secondary_start_virt as *const () as usize as u64;
    for idx in 1..n_cores {
        if !startable[idx] {
            continue;
        }
        let aff = aff_by_index[idx];
        let ret = psci_cpu_on(affinity_to_mpidr(aff), entry, idx as u64);
        if ret == 0 {
            serial_println!(
                ":: AARCH64 SMP: CPU_ON AP {} (aff={:#010x}) -> SUCCESS (entry={:#x}) ::",
                idx, aff, entry
            );
        } else {
            serial_println!(
                ":: AARCH64 SMP: CPU_ON AP {} (aff={:#010x}) -> ERROR {} (skipped) ::",
                idx, aff, ret
            );
        }
    }

    // Wait (≤ ~500 ms each) for every started secondary to publish readiness.
    for idx in 1..n_cores {
        if !startable[idx] {
            continue;
        }
        let deadline = timer::cntpct() + freq / 2;
        if !wait_until(deadline, || CORE_READY[idx].load(Ordering::Acquire)) {
            serial_println!(
                ":: AARCH64 SMP: WARNING AP {} (aff={:#010x}) did not come online ::",
                idx, aff_by_index[idx]
            );
        }
    }

    // BSP → AP proof: ping each online core with SGI 0 (targeted by its affinity) and confirm its
    // per-CPU counter ticks. This direction is individually attributable per core.
    for idx in 1..n_cores {
        if !CORE_READY[idx].load(Ordering::Acquire) {
            continue;
        }
        let before = percpu::cpu(idx).ipis.load(Ordering::Acquire);
        gic::send_sgi(aff_by_index[idx] as usize, IPI_SGI);
        let deadline = timer::cntpct() + freq / 10; // ~100 ms
        let ok = wait_until(deadline, || percpu::cpu(idx).ipis.load(Ordering::Acquire) > before);
        let after = percpu::cpu(idx).ipis.load(Ordering::Acquire);
        serial_println!(
            ":: AARCH64 SMP: BSP -> AP {} SGI {} (count {} -> {}) ::",
            idx,
            if ok { "OK" } else { "TIMEOUT" },
            before,
            after
        );
    }

    // AP → BSP proof: each online AP pinged the BSP once during its bring-up. The verdict is "at least
    // one landed", NOT an exact count: a GICv3 SGI is a single pending bit per (INTID, target), so
    // several APs racing SGI 0 at the BSP before it acknowledges the first coalesce into fewer distinct
    // IRQs. The v3 IAR carries no source CPU, but only APs send SGI 0 to the BSP (the BSP never
    // self-sends), so any growth of the BSP's counter is attributable to an AP. (Per-AP distinct INTIDs
    // for individually-attributable AP→BSP delivery are a deferred delta-list follow-up.)
    let online = (1..n_cores)
        .filter(|&c| startable[c] && CORE_READY[c].load(Ordering::Acquire))
        .count();
    let deadline = timer::cntpct() + freq / 10;
    let ok = wait_until(deadline, || {
        percpu::cpu(0).ipis.load(Ordering::Acquire) > bsp_ipi_before
    });
    let bsp_ipi_after = percpu::cpu(0).ipis.load(Ordering::Acquire);
    serial_println!(
        ":: AARCH64 SMP: AP -> BSP SGI {} ({} online APs pinged, {} delivered; BSP ipi {} -> {}) ::",
        if ok { "OK" } else { "TIMEOUT" },
        online,
        bsp_ipi_after - bsp_ipi_before,
        bsp_ipi_before,
        bsp_ipi_after
    );

    // Timer stretch: DEFERRED. Arming a secondary's periodic tick means its `on_tick` bumps the *shared*
    // global `TICKS`, which `arch::ticks()`/`ms()` feed to wall-clock timeout budgets — a second ticking
    // core would expire those ~2× early. Containing that needs a per-core-only tick path in timer.rs,
    // outside this arc's lane. So the APs park on SGIs alone; per-core preemptible ticks land with the
    // EL2→EL1 drop (JC3).
    serial_println!(
        ":: AARCH64 SMP: AP timer PPI stretch deferred (per-core arm would double-count the shared \
         tick clock; JC3) ::"
    );

    serial_println!(
        ":: AARCH64 SMP: {}/{} secondaries online via PSCI CPU_ON on the GICv3 path ::",
        online,
        n_startable
    );

    // SCHED-NEXT busy-heartbeat: stage a small cooperative queue for each online secondary and release
    // them. Each secondary is spinning in `sched::run_secondary_work` (reached right after it published
    // CORE_READY + pinged, so before its idle park); the release lets it drain the staged queue — real
    // task dispatch that bumps CPU_BUSY — then park idle. Staging happens AFTER the ping proofs above so
    // those observe the same responsive-spinning secondary as before (IRQ unmasked in the spin).
    const SECWORK_TASKS: usize = 2; // >= 2 so busy telemetry is unambiguous (each yields 3x)
    let mut expected = 0usize;
    for idx in 1..n_cores {
        if startable[idx] && CORE_READY[idx].load(Ordering::Acquire) {
            sched::stage_secondary_work(idx, SECWORK_TASKS);
            expected += 1;
        }
    }
    sched::secondary_work_go();
    // Wait for the ACTUAL completion COUNT to reach `expected` before reading the meter, so the busy
    // witness never races an un-run core. The ceiling is a GENEROUS finite backstop (~2 s), NOT a
    // timing bound: each secondary catches the release (armed before CPU_ON) and drains a handful of
    // yield/exit tasks in microseconds of guest time, so the count is reached near-instantly even
    // under heavy host load — the earlier tight per-core ceiling is what flaked the witness. Finite so
    // a genuinely stuck secondary (a bug) can never hang the boot; a shortfall fails the witness loud.
    let deadline = timer::cntpct() + 2 * freq;
    let all_done = wait_until(deadline, || {
        (1..n_cores)
            .filter(|&idx| startable[idx] && CORE_READY[idx].load(Ordering::Acquire))
            .filter(|&idx| sched::secondary_work_done(idx))
            .count()
            == expected
    });
    if !all_done {
        serial_println!(
            ":: AARCH64 SMP: WARNING — cooperative work pass incomplete ({} of {} secondaries done) ::",
            (1..n_cores)
                .filter(|&idx| startable[idx] && CORE_READY[idx].load(Ordering::Acquire))
                .filter(|&idx| sched::secondary_work_done(idx))
                .count(),
            expected
        );
    }

    // Per-core heartbeat witness (VUG-1 M3b honesty): each online secondary ran a cooperative work
    // pass (`run_secondary_work` → CPU_BUSY) and THEN parked idle (`note_core_idle` → CPU_IDLE). Read
    // both counters back: an online secondary must now read `busy > 0` (it ran real scheduled work)
    // AND `idle > 0` (it parked honestly) — the pinned `(0,0)` would render an undefined/frozen meter
    // bar and a bare `busy == 0` would be the parked-only state the idle-heartbeat proved. This is the
    // QEMU-provable half; the real Orin vug pixels (a live busy bar on a secondary) are the accruing
    // metal witness.
    let mut all_idle = true;
    let mut all_busy = true;
    for idx in 1..n_cores {
        if !(startable[idx] && CORE_READY[idx].load(Ordering::Acquire)) {
            continue;
        }
        let (busy, idle) = sched::meter_cpu_ticks(idx);
        if idle == 0 {
            all_idle = false;
        }
        if busy == 0 {
            all_busy = false;
        }
        serial_println!(
            ":: AARCH64 SMP: AP {} pulse (busy={}, idle={}) {} ::",
            idx,
            busy,
            idle,
            if busy > 0 && idle > 0 {
                "ran+idle"
            } else if busy + idle > 0 {
                "idle"
            } else {
                "PINNED"
            }
        );
    }
    serial_println!(
        ":: AARCH64 SMP: per-core idle heartbeat {} — {} online APs report idle (not pinned) ::",
        if all_idle { "PASS" } else { "FAIL" },
        online
    );
    serial_println!(
        ":: AARCH64 SMP: per-core busy heartbeat {} — {} online APs ran cooperative scheduled work ::",
        if all_busy { "PASS" } else { "FAIL" },
        online
    );
}

/// ORIN-SMP-3 — the real 6-core Orin bring-up kick-off (`tegra` + `tegrasmp` gated). Called from
/// `tegra_early_stop` after JM4 (GIC-600 + generic timer + heap up) and BEFORE the JM6 EL2->EL1 drop,
/// so the BSP is still at **EL2**: the woken secondaries wake at the caller's EL (EL2) and replay the
/// BSP's live EL2 regime through the born-fixed `__secondary_rust_virt` path — exactly the state
/// `SEC_CTX`/`enable_mmu_virt` capture. (No EL1/`SEC_CTX` divergence is needed: the JM6b EL1-precise
/// twin table is for the boot core's post-drop regime, which this kick-off precedes.)
///
/// **Presence oracle (RIDER 1): the DTB `/cpus` node ALONE.** The target list is produced by
/// `fdt_tegra::cpu_affinities` from the FDT walk only — never `AFFINITY_INFO` (12 false-valid slots on
/// the 6-core Nano) and never the redistributor walk (8 frames). No code path here issues `CPU_ON` to
/// an affinity absent from that list; the fuse-disabled-`CPU_ON` question (JM5 attempt-1's fatal RAS)
/// is therefore never retested by accident. If `/cpus` yields nothing (unmapped/malformed DTB, or a
/// headless handoff with no DTB), this STOPs before any `CPU_ON` — single-core, exactly like the JM6
/// terminus, never a phantom start.
///
/// **Firmware precondition (RIDER 2).** The first serial line restates it: the bench card asserts the
/// UEFI build (`t23x_general 39.2.0-gcid-45755727` or newer, Peter-acknowledged) as the precondition
/// under which `CPU_ON` is known to work (the SMP-2 bench verdict). A downgraded firmware = the
/// operator STOPs at the bench before trusting the run.
#[cfg(feature = "tegrasmp")]
pub fn start_secondaries_tegra(dtb_addr: u64, dtb_size: usize, ram_gib_mask: u64) {
    // RIDER 2 — the firmware precondition, restated so the transcript self-documents.
    serial_println!(
        ":: AARCH64 SMP: ORIN-SMP-3 kick-off — PRECONDITION UEFI t23x_general 39.2.0-gcid-45755727 (or \
         newer); CPU_ON known-good on this firmware per the SMP-2 bench ::"
    );

    let bsp_aff = gic::this_affinity();

    // RIDER 1 — the ONLY presence oracle: the DTB `/cpus` enumeration. No AFFINITY_INFO, no GICR walk.
    let mut dtb_cores = [0u32; MAX_CORES];
    let n_dtb = super::fdt_tegra::cpu_affinities(dtb_addr, dtb_size, ram_gib_mask, &mut dtb_cores);
    if n_dtb == 0 {
        serial_println!(
            ":: AARCH64 SMP: ORIN-SMP-3 — DTB /cpus named no cores (dtb=@{:#x} size={:#x}); STOP, \
             staying single-core (no CPU_ON) ::",
            dtb_addr, dtb_size
        );
        return;
    }

    // Build the dense linear-index -> affinity table: BSP = index 0, each OTHER `/cpus` core = 1,2,…
    // in DTB order. The BSP is already running, so it is never a `CPU_ON` target.
    let mut aff_by_index = [0u32; MAX_CORES];
    aff_by_index[0] = bsp_aff;
    let mut n_cores = 1usize;
    let mut bsp_seen_in_dtb = false;
    for &a in dtb_cores.iter().take(n_dtb) {
        if a == bsp_aff {
            bsp_seen_in_dtb = true;
            continue; // the BSP holds linear index 0
        }
        if n_cores >= MAX_CORES {
            serial_println!(
                ":: AARCH64 SMP: ORIN-SMP-3 — more /cpus cores than the {}-slot cap; extra ignored ::",
                MAX_CORES
            );
            break;
        }
        aff_by_index[n_cores] = a;
        n_cores += 1;
    }
    let n_secondaries = n_cores - 1;

    // Dump every enumerated core up front — BEFORE any PSCI call — so the metal capture has the full
    // set even if a later CPU_ON faults (the JM5 attempt-1 lesson: a RAS fault ate the enumeration).
    for idx in 0..n_cores {
        serial_println!(
            ":: AARCH64 SMP: ORIN-SMP-3 enumerated core {} aff={:#010x}{} (source=DTB /cpus) ::",
            idx,
            aff_by_index[idx],
            if idx == 0 { " (BSP)" } else { "" }
        );
    }
    if !bsp_seen_in_dtb {
        // The running BSP's affinity was not among the DTB /cpus set — a firmware/enumeration
        // inconsistency worth a serial note (the BSP is still index 0 and startable secondaries are
        // unaffected; this only flags that /cpus and MPIDR disagree about the boot core).
        serial_println!(
            ":: AARCH64 SMP: ORIN-SMP-3 NOTE BSP aff={:#010x} not present in DTB /cpus set ::",
            bsp_aff
        );
    }

    // Publish the EL2 regime + secondary stacks for the MMU-off consumers, then the linear-index table
    // (the CORE3-class fix — a woken secondary recovers its index by matching its live MPIDR affinity,
    // never the MMU-off-spilled context id). Same publication protocol as the virt path.
    BSP_AFFINITY.store(bsp_aff, Ordering::Release);
    gic::enable_sgi(IPI_SGI);
    capture_secondary_ctx();
    cache::clean_range(&raw const SEC_CTX as usize, core::mem::size_of::<SecondaryCtx>());
    cache::clean_invalidate_range(
        &raw const SECONDARY_STACKS as usize,
        core::mem::size_of::<[SecStack; MAX_CORES]>(),
    );
    for idx in 0..n_cores {
        AFF_BY_INDEX[idx].store(aff_by_index[idx], Ordering::Relaxed);
    }
    N_CORES_PUB.store(n_cores as u32, Ordering::Release);

    let freq = timer::cntfrq();
    let freq = if freq == 0 { 62_500_000 } else { freq };
    let bsp_ipi_before = percpu::cpu(0).ipis.load(Ordering::Acquire);

    // Start each `/cpus`-named secondary. Target = its real MPIDR affinity; context id = its linear
    // index (advisory past the MMU turn-on). Entry PA = the identity-mapped stub. A CPU_ON error is
    // logged + skipped, never a hang. NO AFFINITY_INFO gate — the DTB already IS the presence gate.
    let entry = _secondary_start_virt as *const () as usize as u64;
    for idx in 1..n_cores {
        let aff = aff_by_index[idx];
        let ret = psci_cpu_on(affinity_to_mpidr(aff), entry, idx as u64);
        if ret == 0 {
            serial_println!(
                ":: AARCH64 SMP: ORIN-SMP-3 CPU_ON AP {} (aff={:#010x}) -> SUCCESS (entry={:#x}) ::",
                idx, aff, entry
            );
        } else {
            serial_println!(
                ":: AARCH64 SMP: ORIN-SMP-3 CPU_ON AP {} (aff={:#010x}) -> ERROR {} (skipped) ::",
                idx, aff, ret
            );
        }
    }

    // Bounded wait (≤ ~500 ms each) for every secondary to publish readiness. A miss = WARNING +
    // continue (the graceful pre-fix mode: a core that never checks in never hangs the boot).
    for idx in 1..n_cores {
        let deadline = timer::cntpct() + freq / 2;
        if !wait_until(deadline, || CORE_READY[idx].load(Ordering::Acquire)) {
            serial_println!(
                ":: AARCH64 SMP: ORIN-SMP-3 WARNING AP {} (aff={:#010x}) did not come online ::",
                idx, aff_by_index[idx]
            );
        }
    }

    // BSP -> AP proof: ping each online core and confirm its per-CPU IPI counter ticks.
    for idx in 1..n_cores {
        if !CORE_READY[idx].load(Ordering::Acquire) {
            continue;
        }
        let before = percpu::cpu(idx).ipis.load(Ordering::Acquire);
        gic::send_sgi(aff_by_index[idx] as usize, IPI_SGI);
        let deadline = timer::cntpct() + freq / 10; // ~100 ms
        let ok = wait_until(deadline, || percpu::cpu(idx).ipis.load(Ordering::Acquire) > before);
        let after = percpu::cpu(idx).ipis.load(Ordering::Acquire);
        serial_println!(
            ":: AARCH64 SMP: ORIN-SMP-3 BSP -> AP {} SGI {} (count {} -> {}) ::",
            idx,
            if ok { "OK" } else { "TIMEOUT" },
            before,
            after
        );
    }

    // AP -> BSP proof: each online AP pinged the BSP once during bring-up (see the virt-path note on
    // SGI coalescing — the verdict is "at least one landed", the BSP never self-sends SGI 0).
    let online = (1..n_cores)
        .filter(|&c| CORE_READY[c].load(Ordering::Acquire))
        .count();
    let deadline = timer::cntpct() + freq / 10;
    let ok = wait_until(deadline, || {
        percpu::cpu(0).ipis.load(Ordering::Acquire) > bsp_ipi_before
    });
    let bsp_ipi_after = percpu::cpu(0).ipis.load(Ordering::Acquire);
    serial_println!(
        ":: AARCH64 SMP: ORIN-SMP-3 AP -> BSP SGI {} ({} online APs pinged, {} delivered; BSP ipi {} -> {}) ::",
        if ok { "OK" } else { "TIMEOUT" },
        online,
        bsp_ipi_after - bsp_ipi_before,
        bsp_ipi_before,
        bsp_ipi_after
    );

    // AP periodic ticks stay DEFERRED here for the same reason as the virt path (a second core arming
    // the shared tick clock would double-count the wall-clock budgets; JC3). The APs park on SGIs, and
    // the boot core proceeds to the JM6 EL1 drop + CAPSTONE below.
    serial_println!(
        ":: AARCH64 SMP: ORIN-SMP-3 {}/{} secondaries online via PSCI CPU_ON (DTB /cpus oracle); AP \
         timer PPI stretch deferred (JC3) ::",
        online,
        n_secondaries
    );
}

// ════════════════════════════════════════════════════════════════════════════════════════════════
// ORIN-SMP-6 probe support (lane amendment, Maestro-granted 2026-07-16)
// ════════════════════════════════════════════════════════════════════════════════════════════════
//
// The SMP-6 legs 21/23 wake cores into the REAL `_secondary_start_virt` entry (the last untested
// difference vs the acquitted `smpprobe.rs` replica). The real path consumes this module's private
// publication state (`SEC_CTX`, `SECONDARY_STACKS`, `AFF_BY_INDEX`/`N_CORES_PUB`, `BSP_AFFINITY`)
// and reports online via the private `CORE_READY` — so the probe needs EXACTLY two things from this
// module: (a) a publish-only entry that performs the same pre-`CPU_ON` publication
// `start_secondaries_tegra` does (ctx capture + table publication + cache maintenance — it issues
// NO `CPU_ON`), and (b) read visibility on the online signal. Both are `smpprobe`-gated (plus
// `tegra`), so the knob-off image — and every non-probe build — is byte-identical to baseline.

/// Publish everything a REAL-entry secondary consumes, exactly as `start_secondaries_tegra` does
/// before its `CPU_ON` loop, and return the real entry PA (`_secondary_start_virt`). Publish-only:
/// this function issues NO `CPU_ON` — the caller (`smpprobe.rs`) owns the wake and its record.
///
/// `aff_by_index[0]` must be the BSP's packed affinity; `aff_by_index[1..]` the `CPU_ON` targets in
/// linear-index order (the probe sources them from the DTB `/cpus` oracle, RIDER 5). Mirrors the
/// `start_secondaries_tegra` publication protocol line-for-line: BSP affinity + SGI-0 receive
/// enable, EL2 regime capture + clean to PoC, secondary-stack clean+invalidate, then the
/// linear-index table with the `N_CORES_PUB` Release fence.
#[cfg(all(feature = "tegra", feature = "smpprobe"))]
pub fn probe_publish_real_path(aff_by_index: &[u32]) -> u64 {
    let n_cores = aff_by_index.len().min(MAX_CORES);
    if n_cores > 0 {
        BSP_AFFINITY.store(aff_by_index[0], Ordering::Release);
    }
    gic::enable_sgi(IPI_SGI);
    capture_secondary_ctx();
    cache::clean_range(&raw const SEC_CTX as usize, core::mem::size_of::<SecondaryCtx>());
    cache::clean_invalidate_range(
        &raw const SECONDARY_STACKS as usize,
        core::mem::size_of::<[SecStack; MAX_CORES]>(),
    );
    for (idx, &aff) in aff_by_index.iter().take(n_cores).enumerate() {
        AFF_BY_INDEX[idx].store(aff, Ordering::Relaxed);
    }
    N_CORES_PUB.store(n_cores as u32, Ordering::Release);
    _secondary_start_virt as *const () as usize as u64
}

/// Read accessor for the real path's online signal: has the secondary at linear index `idx` run the
/// full `__secondary_rust_virt` bring-up and published `CORE_READY` (Acquire pairs with its
/// Release)? Out-of-range indices read false.
#[cfg(all(feature = "tegra", feature = "smpprobe"))]
pub fn probe_core_online(idx: usize) -> bool {
    idx < MAX_CORES && CORE_READY[idx].load(Ordering::Acquire)
}
