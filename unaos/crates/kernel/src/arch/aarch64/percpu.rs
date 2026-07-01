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
// IRQ/SGI handlers can always resolve `this_cpu()`). TPIDR_EL2 (not EL1/EL0) because the kernel
// runs at EL2 on this platform; `msr TPIDR_EL2` and `mrs …, TPIDR_EL2` are the EL2 thread pointer.

use core::sync::atomic::{AtomicU64, Ordering};

/// The Pi 4 is 4× Cortex-A72. Index by MPIDR Aff0 (0..3).
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
        core::arch::asm!("msr TPIDR_EL2, {}", in(reg) p as u64, options(nomem, nostack, preserves_flags));
    }
}

/// This core's block, via TPIDR_EL2. Only valid after `init` has run on this core (which it has by
/// the time IRQs are unmasked — the SGI/timer handlers call this).
#[inline]
pub fn this_cpu() -> &'static PerCpuData {
    let p: u64;
    unsafe {
        core::arch::asm!("mrs {}, TPIDR_EL2", out(reg) p, options(nomem, nostack, preserves_flags));
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
