// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Per-CPU data, reached through TPIDR_EL2 — the aarch64 analogue of x86's GS-base per-CPU block.
//
// Each core owns a `PerCpuData` block; that block's address is written to TPIDR_EL2, so any core
// can find *its own* block in one instruction (`mrs x, TPIDR_EL2`) with no MPIDR lookup. This is
// the mechanism a scheduler builds on (current task, run queue, preempt state); for now it carries
// per-core IPI and timer counters so we can prove each core answers its own SGIs and ticks.
//
// The blocks live in a static array (no heap — set up on each core before it enables IRQs, so the
// IRQ/SGI handlers can always resolve `this_cpu()`). The thread-pointer register is TPIDR_EL1 on the
// baremetal build (each core drops to EL1 at boot) and TPIDR_EL2 on the UEFI/QEMU-virt build (which
// stays at EL2) — see the `tpidr_reg!` selector below.

use core::sync::atomic::{AtomicU64, Ordering};

// The per-CPU thread pointer register. Baremetal drops to EL1 (see `boot::drop_to_el1`), so it uses
// TPIDR_EL1; the UEFI/QEMU-virt build stays at EL2 and uses TPIDR_EL2. A single `asm!` template can't
// `#[cfg]` a register name, so select the register here and splice it in via `concat!`.
#[cfg(all(target_arch = "aarch64", feature = "baremetal"))]
macro_rules! tpidr_reg {
    () => { "TPIDR_EL1" };
}
#[cfg(not(all(target_arch = "aarch64", feature = "baremetal")))]
macro_rules! tpidr_reg {
    () => { "TPIDR_EL2" };
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

/// Point this core's TPIDR_EL2 at its `PerCpuData`. Call once per core (BSP + each AP) after the
/// MMU is on (the blocks are in cacheable RAM). `cpu_index` = MPIDR Aff0.
pub fn init(cpu_index: usize) {
    debug_assert!(cpu_index < NUM_CPUS);
    unsafe {
        let p = &raw mut PERCPU[cpu_index];
        (*p).cpu_index = cpu_index as u32;
        core::arch::asm!(concat!("msr ", tpidr_reg!(), ", {}"), in(reg) p as u64, options(nomem, nostack, preserves_flags));
    }
}

/// This core's block, via TPIDR_EL2. Only valid after `init` has run on this core (which it has by
/// the time IRQs are unmasked — the SGI/timer handlers call this).
#[inline]
pub fn this_cpu() -> &'static PerCpuData {
    let p: u64;
    unsafe {
        core::arch::asm!(concat!("mrs {}, ", tpidr_reg!()), out(reg) p, options(nomem, nostack, preserves_flags));
        &*(p as *const PerCpuData)
    }
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
