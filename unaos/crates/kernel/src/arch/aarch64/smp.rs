// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// aarch64 SMP bring-up — releasing the parked Cortex-A72 secondary cores on the Raspberry Pi 4.
//
// The GPU firmware (default armstub, no `armstub=` line) starts only core 0 at our kernel; cores
// 1-3 sit in the firmware's spin-table, each in `wfe; ldr xN,[release_addr]; cbz xN,loop; br xN`,
// watching a per-core 64-bit slot in the first page of RAM (core1=0xE0, core2=0xE8, core3=0xF0 —
// from the BCM2711 DTB `cpu-release-addr`). To release a core we write our entry point into its
// slot and `SEV`; it wakes, reads a now-non-zero slot, and branches to us.
//
// Two things make this correct on real silicon (both no-ops in QEMU, which QEMU raspi4b models with
// the *same* spin-table so this exact code is testable there):
//   * The parked core reads its slot with the **MMU off / non-cacheable**, while we write it with
//     the MMU on and RAM cacheable — so our store can sit in cache and never be seen. We must clean
//     the slot to the Point of Coherency (`DC CVAC` + `DSB`) before `SEV`.
//   * A released core arrives in the *same* raw state core 0 did: EL2, MMU OFF, caches off, no
//     stack. So each secondary re-runs the per-core setup (stack, `enable_mmu`, exception vectors)
//     before it can touch a lock or atomic. That's what `_secondary_start` + `__secondary_rust` do.

use core::sync::atomic::{AtomicBool, Ordering};

use super::{boot, cache, exceptions, gic, percpu, sched, timer};

/// The Pi 4 is a single cluster of 4 Cortex-A72s (MPIDR Aff0 = 0..3).
const NUM_CORES: usize = 4;

/// SGI 0 — the inter-processor wake/reschedule ping. M2 uses it only to prove cross-core delivery;
/// M3 will hang the scheduler reschedule off it (the handler is `gic::handle_irq`'s SGI branch).
pub const IPI_RESCHED: u32 = 0;

/// Per-core spin-table release slots (physical addresses in the first page), from the BCM2711 DTB
/// `cpu-release-addr`. Index by MPIDR Aff0; slot 0 (0xD8) is core 0's and unused (it's already up).
const RELEASE_ADDR: [usize; NUM_CORES] = [0xD8, 0xE0, 0xE8, 0xF0];

/// Set true by each secondary once its MMU + vectors + per-CPU + GIC + IPI are up and IRQs are
/// unmasked — the BSP waits on this before it sends any SGI (so the ping isn't lost pre-init).
static CORE_READY: [AtomicBool; NUM_CORES] = [const { AtomicBool::new(false) }; NUM_CORES];

/// One secondary's boot/idle stack (16 KiB… 64 KiB; AArch64 SP must stay 16-aligned, which the
/// type alignment + power-of-two size guarantee). Lives in BSS (zeroed by core 0's `_start`).
const SEC_STACK_SIZE: usize = 0x1_0000; // 64 KiB
#[repr(C, align(16))]
struct SecStack([u8; SEC_STACK_SIZE]);
/// One stack per core, indexed by Aff0. Slot 0 is unused (core 0 has the linker `__stack_top`); the
/// secondaries take slots 1-3. `_secondary_start` computes its own top: base + (core+1)*size.
static mut SECONDARY_STACKS: [SecStack; NUM_CORES] =
    [const { SecStack([0; SEC_STACK_SIZE]) }; NUM_CORES];

// The secondary entry stub. Runs with the MMU OFF at EL2 (x0-x3 = 0 per the spin-table protocol).
// It sets SP to this core's stack top, then tail-calls the Rust entry. Position-independent absolute
// symbol references (adrp/add, and `sym`) resolve to the correct address because the kernel is
// identity-mapped (VA == PA). Kept out of `.text.boot` on purpose — it's reached by address, not by
// being first in the image.
core::arch::global_asm!(
    r#"
    .globl _secondary_start
    _secondary_start:
        mrs   x0, mpidr_el1
        and   x0, x0, #0xff          // x0 = core id (Aff0)
        adrp  x1, {stacks}
        add   x1, x1, #:lo12:{stacks}
        mov   x2, #({size} >> 12)
        lsl   x2, x2, #12            // x2 = SEC_STACK_SIZE
        madd  x3, x0, x2, x1         // x3 = &SECONDARY_STACKS + core*size
        add   x3, x3, x2            // + size  => top of this core's stack
        mov   sp, x3
        bl    {entry}               // __secondary_rust(core)  — never returns
    1:  wfe
        b     1b
    "#,
    stacks = sym SECONDARY_STACKS,
    entry = sym __secondary_rust,
    size = const SEC_STACK_SIZE,
);

unsafe extern "C" {
    fn _secondary_start();
}

/// Rust entry for a released secondary core. Called from `_secondary_start` with the **MMU still
/// off** and this core's stack set. Turns the MMU on FIRST (before any lock/atomic — `serial_println`
/// takes a spinlock), then brings up this core's private state: exception vectors, per-CPU block
/// (TPIDR_EL2), GIC CPU interface, the IPI SGI, and IRQ unmask. It then idles in WFE, waking to
/// service inter-processor interrupts (M3 will replace the idle with the scheduler).
#[unsafe(no_mangle)]
extern "C" fn __secondary_rust(core_raw: u64) -> ! {
    let core = core_raw as usize;
    // Drop this core EL2 -> EL1 (matching the BSP) BEFORE the MMU, so its locks/atomics run at EL1.
    // Each core must do its own drop: VMPIDR/CNTHCTL/CPTR/CPACR/SP_EL1 are all banked per-core.
    unsafe { boot::drop_to_el1() };
    // MMU on, using the L1 table the BSP already built (`enable_mmu` touches no per-core state).
    unsafe { boot::enable_mmu() };
    // Per-core VBAR_EL2 so a fault on this core is caught rather than jumping to a stale vector.
    exceptions::install();
    // Per-CPU data reachable via TPIDR_EL2 (the IPI handler resolves its counter through this).
    percpu::init(core);
    // This core's GIC CPU interface + the IPI SGI (both banked per-core), then unmask IRQ.
    gic::init_cpu_interface();
    gic::enable_sgi(IPI_RESCHED);
    exceptions::enable_irq();
    // This core's own periodic tick, for scheduler preemption (`INTERVAL` was set by the BSP's
    // timer::init). Also wakes this core's WFI in the scheduler's wait/idle loops.
    timer::arm_this_core();

    serial_println!(":: AARCH64 SMP: core {} online (EL1, MMU on, GIC+IPI+timer ready) ::", core);
    // Publish readiness AFTER everything above (Release pairs with the BSP's Acquire), so the BSP
    // only sends an SGI once this core can actually take it.
    CORE_READY[core].store(true, Ordering::Release);

    // Enter the scheduler: wait for the BSP to turn scheduling on, then run this core's run queue
    // forever (idling in WFI when empty). Never returns.
    sched::wait_and_run(core)
}

/// The secondary cores that came online (MPIDR Aff0), for the BSP to place a scheduler workload on.
pub fn online_secondaries() -> alloc::vec::Vec<usize> {
    (1..NUM_CORES).filter(|&c| CORE_READY[c].load(Ordering::Acquire)).collect()
}

/// Busy-wait until `deadline` (a CNTPCT value) for `cond` to hold; returns whether it did. Bounds
/// the SMP handshakes so a wedged core can't hang boot (mirrors the mailbox/xHCI deadline pattern).
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

/// BSP: release cores 1-3 from the spin-table, wait for each to come online, then prove the IPI path
/// by sending each an SGI and confirming its handler ran. Write our entry into each slot, clean it
/// to RAM (the parked cores read it MMU-off), one `DSB` + broadcast `SEV`, each core re-checks its
/// own slot and only the one now non-zero proceeds.
pub fn start_secondaries() {
    // The BSP already has its GIC CPU interface (gic::init) and IRQs unmasked; enable the IPI SGI on
    // it too so it can RECEIVE inter-processor pings (M3 has cores wake each other). Harmless in M2.
    gic::enable_sgi(IPI_RESCHED);

    let entry = _secondary_start as usize as u64;
    unsafe {
        for core in 1..NUM_CORES {
            core::ptr::write_volatile(RELEASE_ADDR[core] as *mut u64, entry);
        }
        // All four slots (0xD8..0xF0) fall in one 64-byte cache line, so this single clean covers
        // cores 1-3; DSB completes it before the SEV makes the wake observable.
        let span = (RELEASE_ADDR[NUM_CORES - 1] + 8) - RELEASE_ADDR[1];
        cache::clean_range(RELEASE_ADDR[1], span);
        core::arch::asm!("dsb sy", "sev", options(nostack, preserves_flags));
    }
    serial_println!(":: AARCH64 SMP: released cores 1-3 via spin-table (0xE0/0xE8/0xF0) ::");

    let freq = timer::cntfrq();
    // Wait (≤ ~500 ms) for every secondary to publish readiness.
    for core in 1..NUM_CORES {
        let deadline = timer::cntpct() + freq / 2;
        if !wait_until(deadline, || CORE_READY[core].load(Ordering::Acquire)) {
            serial_println!(":: AARCH64 SMP: WARNING core {} did not come online ::", core);
        }
    }

    // IPI smoke test: ping each online core with SGI 0 and confirm its per-CPU counter ticks — proof
    // the GIC delivers a software interrupt from the BSP to another core's CPU interface.
    for core in 1..NUM_CORES {
        if !CORE_READY[core].load(Ordering::Acquire) {
            continue;
        }
        let before = percpu::cpu(core).ipis.load(Ordering::Acquire);
        gic::send_sgi(core, IPI_RESCHED);
        let deadline = timer::cntpct() + freq / 10; // ~100 ms
        let took = wait_until(deadline, || {
            percpu::cpu(core).ipis.load(Ordering::Acquire) > before
        });
        let after = percpu::cpu(core).ipis.load(Ordering::Acquire);
        serial_println!(
            ":: AARCH64 SMP: core {} IPI {} (count {} -> {}) ::",
            core,
            if took { "OK" } else { "TIMEOUT" },
            before,
            after
        );
    }

    // The SGI travels the same Group-1 IRQ path as the generic timer. QEMU raspi4b's GIC model does
    // not deliver Group-1 interrupts (it's why the timer takes the poll-spin fallback there but is
    // LIVE on the real Pi 4), so the IPIs above TIME OUT in QEMU and are expected to be OK on metal.
    // Key the caveat off the timer's measured liveness so the log is self-explanatory.
    if !timer::is_live() {
        serial_println!(
            ":: AARCH64 SMP: (IPI delivery shares the timer's Group-1 path — not modeled by QEMU \
             raspi4b; validate on metal, where the timer is LIVE) ::"
        );
    }
}
