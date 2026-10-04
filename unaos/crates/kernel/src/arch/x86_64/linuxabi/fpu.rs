// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! CHARTER: Kernel — driver
//!
//! LINUXABI3 — x87/SSE state for Linux-ABI tasks (the compatibility box, `docs/dev/OS/03_COMPATIBILITY_BOX`; ledger B309).
//!
//! The kernel target is `+soft-float` with MMX/SSE disabled, so no kernel code ever touches the x87/XMM file: whatever a ring-3
//! task left in it is still there when the scheduler runs. That is what makes this small:
//! - EAGER, Linux tasks only. [`dispatch`] (the scheduler's dispatch site, beside `on_dispatch`) restores the incoming Linux
//!   process's 512-byte FXSAVE image with `fxrstor64` after setting CR4.OSFXSR; [`switch_out`] (first statement after any task
//!   returns to the scheduler: yield, preempt, block, exit) saves it with `fxsave64` and then loads the Linux initial image, so no
//!   x87/XMM residue reaches the next task. A syscall that does not block needs nothing (softfloat kernel); one that blocks goes
//!   through `switch_out`.
//! - CR4.OSFXSR follows the dispatch: set for a Linux task, cleared when a RING-3 non-Linux task is dispatched, so the UnaOS ring-3
//!   programs (built `-sse`) keep their `#UD` fence on SSE exactly as before. Kernel tasks change nothing. CR4 is written only on
//!   Linux↔UnaOS-ring-3 transitions (per-core shadow).
//! - FXSAVE, not XSAVE: XCR0 would be x87|SSE (nothing here saves YMM), which is exactly what FXSAVE covers; CR4.OSXSAVE stays clear,
//!   so CPUID reports OSXSAVE=0, AVX/VEX is a clean `#UD` and libc dispatchers pick their SSE2 paths. Lazy CR0.TS/#NM is not used:
//!   the U2.5 first-entry `fninit` scrub in `user_task_trampoline` runs at CPL 0 and would itself trap with TS set.
//! - One slot per process in a lock-free static table keyed by PML4 (the `FS_TAB` shape). A process with no slot (table full) runs
//!   with OSFXSR clear: its first SSE instruction is `#UD` → SIGSEGV zombie. Fail closed.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};

use super::super::gdt::MAX_CPUS;

const SLOTS: usize = 32;
const CR0_MP: u64 = 1 << 1;
const CR0_EM: u64 = 1 << 2;
const CR0_TS: u64 = 1 << 3;
const CR0_NE: u64 = 1 << 5;
const CR4_OSFXSR: u64 = 1 << 9;
const CR4_OSXMMEXCPT: u64 = 1 << 10;
/// Linux initial MXCSR: every exception masked, round-to-nearest, no flags.
pub const MXCSR_INIT: u32 = 0x1F80;

#[repr(C, align(16))]
struct Area([u8; 512]);

/// The Linux initial FP state as an FXSAVE image: FCW 0x37F (all x87 exceptions masked, 64-bit precision), empty tags, MXCSR 0x1F80,
/// every register zero.
static INIT: Area = {
    let mut a = [0u8; 512];
    a[0] = 0x7F;
    a[1] = 0x03;
    a[24] = (MXCSR_INIT & 0xFF) as u8;
    a[25] = (MXCSR_INIT >> 8) as u8;
    Area(a)
};

struct Slot {
    key: AtomicU64,
    area: UnsafeCell<Area>,
}
// SAFETY: a slot's area is touched only by the core its (pinned) process runs on, at dispatch/switch-back with IF=0, or by the
// fork parent before the child exists; ownership is claimed by CAS on `key`.
unsafe impl Sync for Slot {}

static TAB: [Slot; SLOTS] = [const { Slot { key: AtomicU64::new(0), area: UnsafeCell::new(Area([0; 512])) } }; SLOTS];
/// Per core: PML4 of the Linux process whose FP state is LIVE in this core's registers (0 = none) and its slot index.
static LIVE_KEY: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
static LIVE_IDX: [AtomicUsize; MAX_CPUS] = [const { AtomicUsize::new(0) }; MAX_CPUS];
/// Per core: shadow of CR4.OSFXSR as this module last wrote it; one-time CR0/OSXMMEXCPT init done.
static CR4_ON: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];
static CORE_INIT: [AtomicBool; MAX_CPUS] = [const { AtomicBool::new(false) }; MAX_CPUS];

pub static SAVES: AtomicU64 = AtomicU64::new(0);
pub static RESTORES: AtomicU64 = AtomicU64::new(0);
pub static CR4_FLIPS: AtomicU64 = AtomicU64::new(0);
pub static FORK_COPIES: AtomicU64 = AtomicU64::new(0);

/// FXSR + SSE + SSE2 (CPUID.1:EDX bits 24/25/26) — every x86_64 part has them; checked anyway, fail closed.
pub fn supported() -> bool {
    // 0 = not probed, 1 = yes, 2 = no. Cached: CPUID is serialising (and a VM exit under a hypervisor) — not per dispatch.
    static PROBE: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
    match PROBE.load(Ordering::Relaxed) {
        1 => true,
        2 => false,
        _ => {
            let ok = core::arch::x86_64::__cpuid(1).edx & (7 << 24) == 7 << 24;
            PROBE.store(if ok { 1 } else { 2 }, Ordering::Relaxed);
            ok
        }
    }
}

/// `AT_HWCAP` for the auxv: on x86 Linux it is CPUID.1:EDX.
pub fn hwcap() -> u64 {
    core::arch::x86_64::__cpuid(1).edx as u64
}

#[inline(always)]
unsafe fn fxsave(a: *mut Area) {
    unsafe { core::arch::asm!("fxsave64 [{}]", in(reg) a, options(nostack, preserves_flags)) };
}

#[inline(always)]
unsafe fn fxrstor(a: *const Area) {
    unsafe { core::arch::asm!("fxrstor64 [{}]", in(reg) a, options(nostack, preserves_flags, readonly)) };
}

fn core_init(cpu: usize) {
    if CORE_INIT[cpu].load(Ordering::Relaxed) {
        return;
    }
    use x86_64::registers::control::{Cr0, Cr4};
    unsafe {
        let cr0 = Cr0::read_raw() as u64;
        Cr0::write_raw(((cr0 | CR0_MP | CR0_NE) & !(CR0_EM | CR0_TS)) as _);
        Cr4::write_raw((Cr4::read_raw() as u64 | CR4_OSXMMEXCPT) as _);
    }
    CORE_INIT[cpu].store(true, Ordering::Relaxed);
}

fn set_osfxsr(cpu: usize, on: bool) {
    if CR4_ON[cpu].load(Ordering::Relaxed) == on {
        return;
    }
    use x86_64::registers::control::Cr4;
    unsafe {
        let cr4 = Cr4::read_raw() as u64;
        Cr4::write_raw((if on { cr4 | CR4_OSFXSR } else { cr4 & !CR4_OSFXSR }) as _);
    }
    CR4_ON[cpu].store(on, Ordering::Relaxed);
    CR4_FLIPS.fetch_add(1, Ordering::Relaxed);
}

fn find(cr3: u64) -> Option<usize> {
    TAB.iter().position(|s| s.key.load(Ordering::Acquire) == cr3)
}

/// Claim a slot for `cr3` (or return its existing one), seeded with `seed` when newly claimed.
fn claim(cr3: u64, seed: *const Area) -> Option<usize> {
    if let Some(i) = find(cr3) {
        return Some(i);
    }
    for (i, s) in TAB.iter().enumerate() {
        if s.key.compare_exchange(0, cr3, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            unsafe { core::ptr::copy_nonoverlapping(seed, s.area.get(), 1) };
            return Some(i);
        }
    }
    None
}

/// Scheduler dispatch hook (IF=0, before the switch into the task). `ring3` = the task runs in ring 3 (`user_entry != 0`).
pub fn dispatch(cpu: usize, cr3: u64, name: &str, ring3: bool) {
    if cpu >= MAX_CPUS {
        return;
    }
    if cr3 != 0 && name == super::TASK_NAME && super::ACTIVE.load(Ordering::Acquire) && supported() {
        if let Some(i) = claim(cr3, &INIT) {
            core_init(cpu);
            set_osfxsr(cpu, true);
            unsafe { fxrstor(TAB[i].area.get()) };
            LIVE_IDX[cpu].store(i, Ordering::Relaxed);
            LIVE_KEY[cpu].store(cr3, Ordering::Relaxed);
            RESTORES.fetch_add(1, Ordering::Relaxed);
            return;
        }
    }
    if ring3 {
        set_osfxsr(cpu, false);
    }
}

/// Scheduler switch-back hook (IF=0, the task just returned to the scheduler). One relaxed load when no Linux state is live.
pub fn switch_out(cpu: usize) {
    if cpu >= MAX_CPUS {
        return;
    }
    let k = LIVE_KEY[cpu].load(Ordering::Relaxed);
    if k == 0 {
        return;
    }
    LIVE_KEY[cpu].store(0, Ordering::Relaxed);
    let i = LIVE_IDX[cpu].load(Ordering::Relaxed);
    if TAB[i].key.load(Ordering::Acquire) == k {
        unsafe { fxsave(TAB[i].area.get()) };
        SAVES.fetch_add(1, Ordering::Relaxed);
    }
    unsafe { fxrstor(&INIT) }; // the scrub: no Linux x87/XMM residue reaches the next task
}

/// `fork`: the parent is current in kernel context, so its live registers ARE its FP state — copy them into the child's slot.
/// `false` = no slot free (the caller fails the fork with -EAGAIN).
pub fn fork_into(child_cr3: u64) -> bool {
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    let Some(i) = claim(child_cr3, &INIT) else { return false };
    if cpu < MAX_CPUS && LIVE_KEY[cpu].load(Ordering::Relaxed) != 0 {
        unsafe { fxsave(TAB[i].area.get()) };
        FORK_COPIES.fetch_add(1, Ordering::Relaxed);
    }
    true
}

/// `execve`: the new image starts from the Linux initial FP state.
pub fn exec_reset() {
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    if cpu < MAX_CPUS && LIVE_KEY[cpu].load(Ordering::Relaxed) != 0 {
        unsafe { fxrstor(&INIT) };
    }
}

/// Free `cr3`'s slot (where `FS_TAB` is cleared: `gc`, session end).
pub fn release_slot(cr3: u64) {
    for s in TAB.iter() {
        if s.key.load(Ordering::Acquire) == cr3 {
            s.key.store(0, Ordering::Release);
        }
    }
}

/// Slots in use (the `tests linuxabi3` leak check reads it after the session).
pub fn in_use() -> usize {
    TAB.iter().filter(|s| s.key.load(Ordering::Acquire) != 0).count()
}
