// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// aarch64 SMP bring-up on the QEMU `virt` / UEFI path via **PSCI CPU_ON** (Arc JC2).
//
// This is the sibling of `smp.rs` (the Pi 4 bare-metal spin-table path). The two differ in how a
// secondary is *released* and in the EL/MMU state it inherits:
//
//   * Pi bare-metal (`smp.rs`, `baremetal`-gated): the GPU firmware parks cores 1-3 in a spin-table;
//     we write our entry into their release slots and `SEV`. Each core arrives at EL2, MMU off, and
//     re-runs the same `boot::drop_to_el1` + `boot::enable_mmu` the BSP built its own tables with.
//   * virt / UEFI (this file, `not(feature = "pi")`): UEFI starts only the BSP; the other 3 cores sit
//     in PSCI-off state. We start them with the standardized **PSCI `CPU_ON`** SMC (the portable
//     mechanism the Jetson Orin also uses). A PSCI-woken core comes up through a CPU *reset* — MMU
//     off, caches off, DAIF masked, at **EL2** (matching the BSP; QEMU `virt,virtualization=on`), with
//     only its entry point and context id defined (PSCI DEN0022). It does NOT inherit the BSP's live
//     system registers. But UEFI already built a full identity map (VA == PA) that the BSP runs on, so
//     rather than build our own tables we **capture the BSP's live EL2 translation + FP-enable state**
//     (`SEC_CTX`) and each secondary *replays* it to join the same address space (`enable_mmu_virt`).
//
// The scheduler is NOT part of this arc: the aarch64 scheduler is `baremetal`-gated and EL1-coupled,
// while virt runs at EL2 (see the JC2 brief / STATUS). So the secondaries here do their per-core GICv3
// bring-up and then **park in a WFI loop with IRQs unmasked** — able to receive SGIs, nothing more.
// The arc's verdict is cross-core SGI delivery (BSP -> each AP, and one AP -> BSP), not CAPSTONE.
//
// Everything here is QEMU `virt` (`gic-version=3`) only; the whole module is `#[cfg(not(feature =
// "pi"))]` (baremetal implies pi, so it is compiled out of every Pi image), and the kick-off in
// `main.rs` is additionally gated at *runtime* on `gic::is_v3()`, so the plain GICv2 `virt` run stays
// single-core and byte-identical to baseline. Orin metal (real GICR base, cluster affinities, the
// PSCI conduit re-confirmed against the board's own DTB) is a later, operator-attended arc.

use core::sync::atomic::{AtomicBool, Ordering};

use super::{cache, exceptions, gic, percpu, timer};

/// QEMU `virt` with `-smp 4` is a single cluster of 4 cores (MPIDR Aff0 = 0..3, higher affinities 0);
/// cap the secondary loop and the static arrays to this (= `percpu::NUM_CPUS`). Core 0 is the BSP.
const NUM_CORES: usize = 4;

/// SGI 0 — the inter-processor channel used for the cross-core delivery proof (the same INTID the Pi
/// path reserves as `smp::IPI_RESCHED`; there is no scheduler here, so it is only ever a proof ping).
const IPI_SGI: u32 = 0;

/// PSCI `CPU_ON` (64-bit) function identifier (Arm DEN0022, PSCI 1.0 — the QEMU `virt` `/psci` node
/// advertises `compatible = "arm,psci-1.0"`). x1 = target MPIDR (affinity form), x2 = entry point PA,
/// x3 = context id (delivered in the secondary's x0). Returns 0 = SUCCESS, else a negative error
/// (-2 INVALID_PARAMS, -4 ALREADY_ON, ...).
const PSCI_CPU_ON: u64 = 0xC400_0003;

/// Set true by each secondary once its MMU + vectors + per-CPU + GICv3 CPU interface + IPI SGI are up
/// and IRQs are unmasked. The BSP waits on this (Acquire) before it sends any BSP -> AP ping, so a
/// ping can never be lost to a not-yet-receptive core. Release on the AP pairs with the BSP's Acquire.
static CORE_READY: [AtomicBool; NUM_CORES] = [const { AtomicBool::new(false) }; NUM_CORES];

/// One secondary's boot/idle stack (64 KiB). AArch64 SP must stay 16-aligned; `align(16)` + a
/// power-of-two size guarantee it. Lives in BSS (zeroed by the UEFI loader). Slot 0 is unused (the BSP
/// has UEFI's stack); the 3 secondaries take slots 1-3, computed by `_secondary_start_virt`.
const SEC_STACK_SIZE: usize = 0x1_0000; // 64 KiB
#[repr(C, align(16))]
struct SecStack([u8; SEC_STACK_SIZE]);
static mut SECONDARY_STACKS: [SecStack; NUM_CORES] =
    [const { SecStack([0; SEC_STACK_SIZE]) }; NUM_CORES];

/// The BSP's live EL2 state a secondary must replay to run the same identity map with FP enabled.
/// Captured once by `capture_secondary_ctx`, cleaned to the Point of Coherency, then read by each
/// secondary with its **MMU off** (i.e. non-cacheable, straight from PoC — hence the clean). Plain
/// `u64` fields read/written volatile; no locks/atomics (the secondary reads this before it can touch
/// either). `align(64)` keeps it inside one cache line so a single `clean_range` publishes it.
///
/// CPTR_EL2 is captured alongside the four translation registers because a PSCI-reset core does not
/// inherit the BSP's FP-enable state: the kernel is built `+neon`, so ordinary Rust (fmt in the AP's
/// first `serial_println!`, memcpy) autovectorizes, and CPTR_EL2.TFP (bit 10) traps EL2 FP/SIMD to
/// EL2 — with the AP's VBAR_EL2 still UEFI's, that trap would kill the AP before it prints. TFP resets
/// UNKNOWN (the baremetal `drop_to_el1` clears it for exactly this reason); replaying the BSP's
/// known-good CPTR_EL2 removes the reliance on the reset value (QEMU zeroes it; other firmware/silicon
/// need not).
#[repr(C, align(64))]
#[derive(Clone, Copy)]
struct SecondaryCtx {
    mair: u64,
    tcr: u64,
    ttbr0: u64,
    sctlr: u64,
    cptr: u64,
}
static mut SEC_CTX: SecondaryCtx =
    SecondaryCtx { mair: 0, tcr: 0, ttbr0: 0, sctlr: 0, cptr: 0 };

// The secondary entry stub. Runs at EL2 with the MMU OFF (x0 = context id, unused — we take the core
// id from MPIDR to stay authoritative). Sets SP to this core's stack top, then tail-calls the Rust
// entry. Absolute symbol references (adrp/add, `sym`) resolve correctly because UEFI identity-maps the
// kernel (VA == PA). Same shape as the baremetal `_secondary_start`; the MMU is enabled in Rust
// (`enable_mmu_virt`), not here, so no captured-register offsets are baked into asm.
core::arch::global_asm!(
    r#"
    .globl _secondary_start_virt
    _secondary_start_virt:
        mrs   x0, mpidr_el1
        and   x0, x0, #0xff          // x0 = core id (Aff0); virt is a single cluster
        adrp  x1, {stacks}
        add   x1, x1, #:lo12:{stacks}
        mov   x2, #({size} >> 12)
        lsl   x2, x2, #12            // x2 = SEC_STACK_SIZE
        madd  x3, x0, x2, x1         // x3 = &SECONDARY_STACKS + core*size
        add   x3, x3, x2             // + size  => top of this core's stack
        mov   sp, x3
        bl    {entry}               // __secondary_rust_virt(core) — never returns
    1:  wfe
        b     1b
    "#,
    stacks = sym SECONDARY_STACKS,
    entry = sym __secondary_rust_virt,
    size = const SEC_STACK_SIZE,
);

unsafe extern "C" {
    fn _secondary_start_virt();
}

/// Join the BSP's EL2 translation regime, MMU still OFF on entry. Reads the captured `SEC_CTX` (which
/// the BSP cleaned to PoC — this read is non-cacheable, straight from RAM), then programs this core's
/// EL2 registers and turns the MMU on. Called as the very first thing in `__secondary_rust_virt`,
/// before any lock/atomic (a spinlock's `ldxr/stxr` is CONSTRAINED UNPREDICTABLE with the MMU off) and
/// before any FP/SIMD (CPTR_EL2 is set here first). System-register moves only; no atomics/locks.
///
/// Order: CPTR_EL2 (un-trap FP) first, then MAIR/TCR/TTBR0, a `tlbi alle2` to drop any cold-reset TLB
/// state, `dsb sy; isb`, then SCTLR_EL2 (whose captured value has M=1 → MMU on) and a final `isb`. The
/// map is identity, so enabling translation moves neither PC nor SP. This is the EL2 analogue of the
/// proven baremetal `boot::enable_mmu` (EL1).
unsafe fn enable_mmu_virt() {
    let c = unsafe { core::ptr::read_volatile(&raw const SEC_CTX) };
    unsafe {
        core::arch::asm!(
            "msr CPTR_EL2, {cptr}",   // un-trap FP/SIMD BEFORE any +neon-autovectorized code below
            "msr MAIR_EL2, {mair}",
            "msr TCR_EL2,  {tcr}",
            "msr TTBR0_EL2, {ttbr0}",
            "tlbi alle2",             // drop cold-reset EL2 TLB state before translation is enabled
            "dsb sy",
            "isb",
            "msr SCTLR_EL2, {sctlr}", // captured M=1/C/I (+ SA/EE/RES1) → MMU + caches on
            "isb",                    // also synchronizes the CPTR write before the first FP op
            cptr = in(reg) c.cptr,
            mair = in(reg) c.mair,
            tcr = in(reg) c.tcr,
            ttbr0 = in(reg) c.ttbr0,
            sctlr = in(reg) c.sctlr,
            options(nostack, preserves_flags),
        );
    }
}

/// Rust entry for a PSCI-started secondary. Called from `_secondary_start_virt` with the MMU still
/// OFF and this core's stack set (x0 = core id). Brings the core up to parity with the BSP's per-core
/// state, then parks in WFI able to service SGIs. Never returns.
#[unsafe(no_mangle)]
extern "C" fn __secondary_rust_virt(core_raw: u64) -> ! {
    let core = core_raw as usize;
    // MMU + FP on FIRST (replaying the BSP's EL2 regime), before any lock/atomic/FP. `serial_println!`
    // below takes a spinlock, so nothing may print before this.
    unsafe { enable_mmu_virt() };
    // Per-core VBAR_EL2 (+ HCR_EL2 IMO/FMO/AMO so a physical IRQ taken at EL2 targets EL2) — matches
    // the BSP; installed before IRQs are unmasked.
    exceptions::install();
    // Per-CPU block reachable via TPIDR_EL2 (the IPI handler resolves its counter through this).
    percpu::init(core);
    // This core's GICv3 redistributor (wake + banked Group 1) and its system-register CPU interface —
    // both banked, so the BSP's JC1 setup does not cover them. Then enable the IPI SGI (banked in this
    // core's redistributor) and unmask IRQ.
    gic::init_secondary_v3();
    gic::enable_sgi(IPI_SGI);
    exceptions::enable_irq();

    serial_println!(":: AARCH64 SMP: AP {} online ::", core);
    // Publish readiness AFTER everything above (Release pairs with the BSP's Acquire): the BSP only
    // sends a BSP -> AP ping once this core can actually take it.
    CORE_READY[core].store(true, Ordering::Release);

    // AP -> BSP proof: ping the BSP (core 0) once. The BSP enabled SGI 0 and unmasked IRQ before any
    // CPU_ON, so it is already receptive; it counts these to confirm the reverse direction.
    gic::send_sgi(0, IPI_SGI);

    // Park: IRQs unmasked, so a BSP -> AP SGI wakes this WFI, is serviced (handle_irq_v3 counts it),
    // and the core re-parks. No scheduler on the virt path (see the module header).
    loop {
        unsafe { core::arch::asm!("wfi", options(nomem, nostack, preserves_flags)) };
    }
}

/// Issue a PSCI `CPU_ON` via the **SMC** conduit and return x0 (0 = SUCCESS, else negative error).
///
/// SMC is the conduit that reaches QEMU's PSCI from our EL2 guest: `virt,virtualization=on` advertises
/// `method = "smc"` in the generated `/psci` node (confirmed by `qemu-system-aarch64 ... dumpdtb`),
/// and QEMU's TCG PSCI intercepts the SMC regardless of EL3 being absent. (An `hvc` from EL2 would be
/// taken to our own EL2 vector instead.) `report_conduit` re-confirms this against the live DTB.
///
/// SMCCC (DEN0028): for a fast call the implementation returns results in x0-x3 and preserves only
/// x18-x30 + SP — so x0-x17 are all volatile. x1-x3 are therefore marked clobbered outputs (not plain
/// inputs), and x4-x17 are listed as clobbers. No `nomem`: the call has a global memory side effect
/// (it makes the BSP's prior stores to SEC_CTX/stacks matter on another core), and the ordering vs the
/// `dsb sy`-terminated `clean_range`s above must be preserved.
fn psci_cpu_on(target_mpidr: u64, entry: u64, context_id: u64) -> i64 {
    let mut x0 = PSCI_CPU_ON;
    unsafe {
        core::arch::asm!(
            "smc #0",
            inout("x0") x0,
            inout("x1") target_mpidr => _,
            inout("x2") entry => _,
            inout("x3") context_id => _,
            out("x4") _, out("x5") _, out("x6") _, out("x7") _,
            out("x8") _, out("x9") _, out("x10") _, out("x11") _,
            out("x12") _, out("x13") _, out("x14") _, out("x15") _,
            out("x16") _, out("x17") _,
            options(nostack),
        );
    }
    x0 as i64
}

/// Capture the BSP's live EL2 translation + FP-enable state into `SEC_CTX` for the secondaries to
/// replay. Read directly from the running BSP's system registers, so whatever UEFI configured
/// (TCR_EL2 RES1 bits, SCTLR_EL2 SA/EE, a permissive CPTR_EL2) is carried faithfully — no
/// hand-constructed bits. BSP-only, MMU on.
fn capture_secondary_ctx() {
    let (mair, tcr, ttbr0, sctlr, cptr): (u64, u64, u64, u64, u64);
    unsafe {
        core::arch::asm!("mrs {}, MAIR_EL2", out(reg) mair, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, TCR_EL2", out(reg) tcr, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, TTBR0_EL2", out(reg) ttbr0, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, SCTLR_EL2", out(reg) sctlr, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mrs {}, CPTR_EL2", out(reg) cptr, options(nomem, nostack, preserves_flags));
        core::ptr::write_volatile(&raw mut SEC_CTX, SecondaryCtx { mair, tcr, ttbr0, sctlr, cptr });
    }
}

/// Confirm + log the PSCI conduit from the live DTB (the brief's preferred discovery path). On our EL2
/// virt config the conduit is SMC regardless of what the DTB `method` says for the EL1-guest view, so
/// this is informational: it proves the FDT is understood and records what QEMU advertised. (An EL1
/// PSCI host — Orin metal, later — would *honor* the method rather than fix it to SMC.)
fn report_conduit(dtb_addr: u64, dtb_size: usize) {
    if dtb_addr != 0 {
        unsafe {
            let slice = core::slice::from_raw_parts(dtb_addr as *const u8, dtb_size);
            if let Ok(fdt) = fdt::Fdt::new(slice) {
                if let Some(node) = fdt.find_node("/psci") {
                    if let Some(p) = node.property("method") {
                        let m = core::str::from_utf8(p.value).unwrap_or("?").trim_end_matches('\0');
                        serial_println!(
                            ":: AARCH64 SMP: PSCI conduit=SMC (EL2 virt); DTB /psci method=\"{}\" ::",
                            m
                        );
                        return;
                    }
                }
            }
        }
    }
    serial_println!(":: AARCH64 SMP: PSCI conduit=SMC (EL2 virt; /psci method not in FDT, assumed) ::");
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

/// BSP: bring the 3 secondaries online via PSCI `CPU_ON`, then prove the GICv3 cross-core IPI path in
/// both directions. Called from `main.rs` only when `gic::is_v3()` (GICv2 virt stays single-core).
///
/// Publication before start: the secondaries read `SEC_CTX` with their MMU OFF (non-cacheable), so it
/// is cleaned to PoC; `SECONDARY_STACKS` is clean+invalidated so the UEFI loader's *cacheable* BSS-zero
/// lines can't later evict-clobber a secondary's MMU-off stack writes (a no-op in QEMU, load-bearing on
/// metal — the baremetal path is immune only because its BSS zero is itself MMU-off). Both complete
/// (`dsb sy`) before the first `CPU_ON`.
pub fn start_secondaries(dtb_addr: u64, dtb_size: usize) {
    // The BSP must be able to RECEIVE the AP -> BSP pings: enable SGI 0 on its own CPU interface. Its
    // IRQs are already unmasked (arch::init ran enable_irq well before this).
    gic::enable_sgi(IPI_SGI);

    // Capture the EL2 regime, then publish it (and the secondary stacks) for MMU-off consumers.
    // Taking the raw address of a `static mut` (`&raw const … as usize`) and the `cache::` helpers are
    // all safe — there is no dereference here, so no `unsafe` block is needed.
    capture_secondary_ctx();
    cache::clean_range(&raw const SEC_CTX as usize, core::mem::size_of::<SecondaryCtx>());
    cache::clean_invalidate_range(
        &raw const SECONDARY_STACKS as usize,
        core::mem::size_of::<[SecStack; NUM_CORES]>(),
    );

    report_conduit(dtb_addr, dtb_size);

    let freq = timer::cntfrq();
    let freq = if freq == 0 { 62_500_000 } else { freq };
    let bsp_ipi_before = percpu::cpu(0).ipis.load(Ordering::Acquire);

    // Start each secondary. target MPIDR = Aff0 = core (virt single cluster, Aff1/2/3 = 0). Entry PA =
    // the stub symbol (identity-mapped). A CPU_ON error → log + skip, never hang.
    let entry = _secondary_start_virt as *const () as usize as u64;
    for core in 1..NUM_CORES {
        let ret = psci_cpu_on(core as u64, entry, core as u64);
        if ret == 0 {
            serial_println!(":: AARCH64 SMP: CPU_ON AP {} -> SUCCESS (entry={:#x}) ::", core, entry);
        } else {
            serial_println!(":: AARCH64 SMP: CPU_ON AP {} -> ERROR {} (skipped) ::", core, ret);
        }
    }

    // Wait (≤ ~500 ms each) for every secondary to publish readiness.
    for core in 1..NUM_CORES {
        let deadline = timer::cntpct() + freq / 2;
        if !wait_until(deadline, || CORE_READY[core].load(Ordering::Acquire)) {
            serial_println!(":: AARCH64 SMP: WARNING AP {} did not come online ::", core);
        }
    }

    // BSP -> AP proof: ping each online core with SGI 0 and confirm its per-CPU counter ticks.
    for core in 1..NUM_CORES {
        if !CORE_READY[core].load(Ordering::Acquire) {
            continue;
        }
        let before = percpu::cpu(core).ipis.load(Ordering::Acquire);
        gic::send_sgi(core, IPI_SGI);
        let deadline = timer::cntpct() + freq / 10; // ~100 ms
        let ok = wait_until(deadline, || percpu::cpu(core).ipis.load(Ordering::Acquire) > before);
        let after = percpu::cpu(core).ipis.load(Ordering::Acquire);
        serial_println!(
            ":: AARCH64 SMP: BSP -> AP {} SGI {} (count {} -> {}) ::",
            core,
            if ok { "OK" } else { "TIMEOUT" },
            before,
            after
        );
    }

    // AP -> BSP proof: each online AP pinged the BSP once during its bring-up. The verdict is "at
    // least one landed" (the brief's requirement), NOT an exact count: a GICv3 SGI is a single pending
    // bit per (INTID, target), so several APs racing SGI 0 at the BSP before it acknowledges the first
    // coalesce into fewer distinct IRQs. The v3 IAR carries no source CPU, but only APs send SGI 0 to
    // core 0 (the BSP never self-sends), so any growth of the BSP's counter is attributable to an AP.
    let online = (1..NUM_CORES).filter(|&c| CORE_READY[c].load(Ordering::Acquire)).count();
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

    // Timer stretch (brief §3): DEFERRED. Arming a secondary's periodic tick means its `on_tick` bumps
    // the *shared* global `TICKS`, which `arch::ticks()`/`ms()` feed to the xHCI and e1000 timeout
    // budgets on this same virt boot — a second ticking core would expire those wall-clock budgets ~2x
    // early. Containing that needs a per-core-only tick path in timer.rs, which is outside this arc's
    // lane. So the APs park on SGIs alone; per-core preemptible ticks land with the EL2->EL1 drop (JC3).
    serial_println!(
        ":: AARCH64 SMP: AP timer PPI stretch deferred (per-core arm would double-count the shared \
         tick clock read by xHCI/e1000 timeouts; JC3) ::"
    );

    serial_println!(
        ":: AARCH64 SMP: {}/{} secondaries online via PSCI CPU_ON on the GICv3 virt path ::",
        online,
        NUM_CORES - 1
    );
}
