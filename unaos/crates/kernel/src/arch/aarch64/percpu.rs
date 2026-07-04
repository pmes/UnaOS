// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Per-CPU data, reached through the EL's thread-pointer register (TPIDR_EL1 or TPIDR_EL2) — the aarch64
// analogue of x86's GS-base per-CPU block.
//
// Each core owns a `PerCpuData` block; that block's address is written to its TPIDR, so any core can find
// *its own* block in one instruction (`mrs x, TPIDR_ELn`) with no MPIDR lookup. This is the mechanism a
// scheduler builds on (current task, run queue, preempt state); for now it carries per-core IPI and timer
// counters so we can prove each core answers its own SGIs and ticks.
//
// The blocks live in a static array (no heap — set up on each core before it enables IRQs, so the
// IRQ/SGI handlers can always resolve `this_cpu()`). WHICH TPIDR is used depends on the EL the core runs
// at: TPIDR_EL1 on the baremetal build (each core drops to EL1 at boot); on the UEFI/QEMU-virt + tegra
// build it is chosen at runtime from CurrentEL (EL2 -> TPIDR_EL2, EL1 -> TPIDR_EL1), because the `virt`
// boot core drops EL2 -> EL1 mid-boot (JC3) while tegra stays at EL2 — see `read_tpidr`/`write_tpidr`.

use core::sync::atomic::{AtomicU64, Ordering};

// The per-CPU thread pointer register selection.
//
//   * Baremetal (Pi): each core drops to EL1 at boot (`boot::drop_to_el1`), so it ALWAYS uses TPIDR_EL1
//     — a fixed, compile-time choice (no CurrentEL read), keeping the Pi hot path byte-identical.
//   * Not-baremetal (QEMU `virt` + Jetson tegra): a single binary must serve two EL histories. The boot
//     core comes up at EL2 (early bring-up, the JC2 SMP proof) and uses TPIDR_EL2; the `virt` path (JC3)
//     then drops the boot core to EL1, after which it MUST use TPIDR_EL1 (TPIDR_EL2 would trap at EL1);
//     tegra stays at EL2 throughout. So the register is chosen at RUNTIME from CurrentEL: EL2 ->
//     TPIDR_EL2, else -> TPIDR_EL1. On tegra CurrentEL is always 2, so this is behaviour-identical to the
//     old fixed TPIDR_EL2 (one extra `mrs CurrentEL` + branch, no semantic change); on `virt` it flips to
//     TPIDR_EL1 the moment the boot core is at EL1.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
#[inline(always)]
unsafe fn write_tpidr(p: u64) {
    unsafe {
        core::arch::asm!("msr TPIDR_EL1, {}", in(reg) p, options(nomem, nostack, preserves_flags));
    }
}
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
#[inline(always)]
fn read_tpidr() -> u64 {
    let p: u64;
    unsafe {
        core::arch::asm!("mrs {}, TPIDR_EL1", out(reg) p, options(nomem, nostack, preserves_flags));
    }
    p
}
#[cfg(not(all(target_arch = "aarch64", feature = "baremetal")))]
#[inline(always)]
unsafe fn write_tpidr(p: u64) {
    let el: u64;
    unsafe {
        core::arch::asm!("mrs {}, CurrentEL", out(reg) el, options(nomem, nostack, preserves_flags));
        if (el >> 2) & 0b11 == 2 {
            core::arch::asm!("msr TPIDR_EL2, {}", in(reg) p, options(nomem, nostack, preserves_flags));
        } else {
            core::arch::asm!("msr TPIDR_EL1, {}", in(reg) p, options(nomem, nostack, preserves_flags));
        }
    }
}
#[cfg(not(all(target_arch = "aarch64", feature = "baremetal")))]
#[inline(always)]
fn read_tpidr() -> u64 {
    let (el, p): (u64, u64);
    unsafe {
        core::arch::asm!("mrs {}, CurrentEL", out(reg) el, options(nomem, nostack, preserves_flags));
        if (el >> 2) & 0b11 == 2 {
            core::arch::asm!("mrs {}, TPIDR_EL2", out(reg) p, options(nomem, nostack, preserves_flags));
        } else {
            core::arch::asm!("mrs {}, TPIDR_EL1", out(reg) p, options(nomem, nostack, preserves_flags));
        }
    }
    p
}

/// Per-CPU block count. The Pi 4 is 4× Cortex-A72 (indexed by MPIDR Aff0 0..3); the QEMU `virt`
/// SMP run is 4 cores; the Jetson Orin Nano is a **6-core** Cortex-A78AE SoC. On multi-cluster silicon
/// MPIDR Aff0 is *not* a dense core id (Tegra234 encodes the cluster in Aff1/Aff2 with Aff0=0), so the
/// SMP bring-up assigns each core a **linear index** 0..N-1 (BSP=0) and uses that as the block index —
/// see `smp_virt.rs`. The Orin build sizes this to 8 (covers 6 + headroom, matching `smp_virt`'s
/// `MAX_CORES`); pi/virt keep 4. It is `tegra`-gated rather than a flat bump specifically so the
/// **pi and virt binaries — and thus their serial logs — stay byte-identical** (a larger array would
/// shift the BSS layout and every address printed after it).
///
/// NOTE (JM5 lane extension, Peter-approved 2026-07-04): `percpu.rs` is a shared aarch64 core file not
/// named in the JM5 brief's lane; this `tegra`-gated size is the only out-of-lane change and is flagged
/// for Fable's review. It changes no logic — only the block count, and only on the Orin build.
#[cfg(feature = "tegra")]
pub const NUM_CPUS: usize = 8;
#[cfg(not(feature = "tegra"))]
pub const NUM_CPUS: usize = 4;

/// One core's private data. `#[repr(C, align(64))]` puts each core's block on its own cache line so
/// the atomic counters don't false-share across cores.
#[repr(C, align(64))]
pub struct PerCpuData {
    pub cpu_index: u32,
    /// SGIs (inter-processor interrupts) handled on THIS core.
    pub ipis: AtomicU64,
    /// Timer ticks taken on THIS core (used once each core arms its own timer).
    pub ticks: AtomicU64,
}

impl PerCpuData {
    const fn new() -> Self {
        PerCpuData { cpu_index: 0, ipis: AtomicU64::new(0), ticks: AtomicU64::new(0) }
    }
}

static mut PERCPU: [PerCpuData; NUM_CPUS] = [const { PerCpuData::new() }; NUM_CPUS];

/// Point this core's thread-pointer register (TPIDR_EL1 or TPIDR_EL2 — see `write_tpidr`) at its
/// `PerCpuData`. Call once per core (BSP + each AP) after the MMU is on (the blocks are in cacheable
/// RAM). On the `virt` path (JC3) the boot core calls this a SECOND time after it drops to EL1, so its
/// TPIDR_EL1 is seeded (the pre-drop call seeded TPIDR_EL2). `cpu_index` = linear core index.
pub fn init(cpu_index: usize) {
    debug_assert!(cpu_index < NUM_CPUS);
    unsafe {
        let p = &raw mut PERCPU[cpu_index];
        (*p).cpu_index = cpu_index as u32;
        write_tpidr(p as u64);
    }
}

/// This core's block, via the thread-pointer register (TPIDR_EL1/EL2 per CurrentEL — see `read_tpidr`).
/// Only valid after `init` has run on this core (which it has by the time IRQs are unmasked — the
/// SGI/timer handlers call this).
#[inline]
pub fn this_cpu() -> &'static PerCpuData {
    unsafe { &*(read_tpidr() as *const PerCpuData) }
}

/// Another core's block, by index — for the BSP to read an AP's counters (e.g. the IPI smoke test).
/// Cross-core reads go through the atomic fields; the struct itself is written once, at `init`.
pub fn cpu(index: usize) -> &'static PerCpuData {
    debug_assert!(index < NUM_CPUS);
    unsafe { &*(&raw const PERCPU[index]) }
}

/// Convenience: bump this core's IPI counter (called from the SGI path in `gic::handle_irq`).
#[inline]
pub fn count_ipi() {
    this_cpu().ipis.fetch_add(1, Ordering::Relaxed);
}
