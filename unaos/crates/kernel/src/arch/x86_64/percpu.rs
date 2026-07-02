// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// Per-CPU data, reached through the GS base.
//
// Each CPU owns a `PerCpuData` block; that block's address is loaded into IA32_GS_BASE, and its
// first field is a pointer to itself. So any CPU can find *its own* block in one instruction —
// `mov reg, gs:[0]` — with no lookup keyed on APIC id. This is the standard per-CPU mechanism a
// scheduler builds on (current task, run queue, preempt state); for now it carries per-CPU timer
// and IPI counters so we can prove each core's local APIC timer ticks and answers IPIs
// independently.
//
// The blocks live in a static array (no heap — set up before the heap on the BSP, and APs never
// touch the allocator). GS base must be programmed on each CPU *before* it enables interrupts, so
// the timer/IPI handlers can always resolve `this_cpu()`.

use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::registers::model_specific::Msr;

use crate::arch::gdt::MAX_CPUS;

const IA32_GS_BASE: u32 = 0xC000_0101;

/// One CPU's private data. `self_ptr` MUST stay first (offset 0): `gs:[0]` reads it to recover
/// the block's address. `#[repr(C)]` pins that layout.
///
/// `syscall_kernel_rsp` / `syscall_user_rsp` are the SYSCALL/SYSRET stack-switch anchors (U1a).
/// SYSCALL does not switch stacks itself, so the `unaos_syscall_entry` stub (see `syscall.rs`)
/// `swapgs`es to bring THIS block into GS, saves the user rsp into `syscall_user_rsp`, and loads
/// the per-task kernel-stack top from `syscall_kernel_rsp`. They are reached from the naked stub
/// by fixed gs-relative offsets (`{KERNEL,USER}_RSP_OFFSET`), asserted below — do not reorder the
/// fields above them without updating the offsets. Both are plain `u64` (single-CPU, IRQ-masked
/// syscall path — no cross-CPU access), written by `set_syscall_kernel_rsp` and the stub.
#[repr(C)]
pub struct PerCpuData {
    self_ptr: u64,
    pub cpu_index: u32,
    pub apic_id: u32,
    /// Local-APIC timer ticks taken on THIS CPU.
    pub ticks: AtomicU64,
    /// IPIs handled on THIS CPU.
    pub ipis: AtomicU64,
    /// U1a: per-task kernel-stack top the SYSCALL stub switches to on entry from ring 3.
    syscall_kernel_rsp: u64,
    /// U1a: scratch slot the SYSCALL stub saves the user rsp into across a syscall.
    syscall_user_rsp: u64,
}

/// gs-relative offset of `syscall_kernel_rsp`, baked into the naked SYSCALL stub.
pub const KERNEL_RSP_OFFSET: usize = core::mem::offset_of!(PerCpuData, syscall_kernel_rsp);
/// gs-relative offset of `syscall_user_rsp`, baked into the naked SYSCALL stub.
pub const USER_RSP_OFFSET: usize = core::mem::offset_of!(PerCpuData, syscall_user_rsp);

impl PerCpuData {
    const fn new() -> Self {
        PerCpuData {
            self_ptr: 0,
            cpu_index: 0,
            apic_id: 0,
            ticks: AtomicU64::new(0),
            ipis: AtomicU64::new(0),
            syscall_kernel_rsp: 0,
            syscall_user_rsp: 0,
        }
    }
}

static mut PERCPU: [PerCpuData; MAX_CPUS] = [const { PerCpuData::new() }; MAX_CPUS];

/// Initialise this CPU's per-CPU block and point IA32_GS_BASE at it. Called once per CPU, on that
/// CPU, before it enables interrupts (BSP: in `arch::init`; AP: in `ap_entry`).
pub fn init_cpu(index: usize, apic_id: u32) {
    assert!(index < MAX_CPUS, "percpu::init_cpu: index out of range");
    unsafe {
        let p = &raw mut PERCPU[index];
        (*p).self_ptr = p as u64;
        (*p).cpu_index = index as u32;
        (*p).apic_id = apic_id;
        // gs:[0] now yields self_ptr; gs-relative access reaches this CPU's fields.
        Msr::new(IA32_GS_BASE).write(p as u64);
    }
}

/// This CPU's per-CPU block, via `gs:[0]`. Only valid after `init_cpu` has run on this CPU
/// (always true in interrupt context, since GS base is set before `sti`).
#[inline]
pub fn this_cpu() -> &'static PerCpuData {
    let ptr: *const PerCpuData;
    unsafe {
        core::arch::asm!("mov {}, gs:[0]", out(reg) ptr, options(nostack, preserves_flags));
        &*ptr
    }
}

/// U1a: record THIS CPU's per-task kernel-stack top for the SYSCALL entry stub. Called by
/// `user_task_trampoline` (IRQ-masked, on the CPU that is about to drop to ring 3) just before the
/// `swapgs`/`iretq`, so the stub's `mov rsp, gs:[KERNEL_RSP_OFFSET]` lands on this task's kernel
/// stack. Written through the self-pointer (`gs:[0]`); no other CPU touches this field.
#[inline]
pub fn set_syscall_kernel_rsp(rsp: u64) {
    let ptr: *mut PerCpuData;
    unsafe {
        core::arch::asm!("mov {}, gs:[0]", out(reg) ptr, options(nostack, preserves_flags));
        (*ptr).syscall_kernel_rsp = rsp;
    }
}

/// Another CPU's per-CPU block by logical index (for the BSP to inspect APs). The counters are
/// atomics, so a shared reference is safe to read across CPUs.
pub fn cpu(index: usize) -> Option<&'static PerCpuData> {
    if index >= MAX_CPUS {
        return None;
    }
    unsafe { Some(&*(&raw const PERCPU[index])) }
}

/// Record a timer tick on the current CPU. Lock-free; called from the timer interrupt.
#[inline]
pub fn note_tick() {
    this_cpu().ticks.fetch_add(1, Ordering::Relaxed);
}

/// Record a handled IPI on the current CPU. Lock-free; called from the IPI interrupt.
#[inline]
pub fn note_ipi() {
    this_cpu().ipis.fetch_add(1, Ordering::Relaxed);
}
