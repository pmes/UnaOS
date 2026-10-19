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
//! task left in it is still there when the scheduler runs. FPU (rmbp-ledger B666) made the mechanism ONE for every ring-3 task:
//! the primitives (the 512-byte image, `fxsave64`/`fxrstor64`, the per-core CR0/CR4 init and the CR4.OSFXSR shadow) live in
//! `sched::fpu`, whose `dispatch`/`switch_out` are the scheduler's hooks; a UnaOS ring-3 task keeps its image in its own
//! `Task::fpu`, and this module keeps only what is Linux's:
//! - EAGER. [`dispatch`] (called by `sched::fpu::dispatch`) restores the incoming Linux process's FXSAVE image with `fxrstor64`
//!   after setting CR4.OSFXSR; [`switch_out`] (called by `sched::fpu::switch_out`, the first statement after any task returns to
//!   the scheduler) saves it with `fxsave64` and then loads the initial image, so no x87/XMM residue reaches the next task. A
//!   syscall that does not block needs nothing (softfloat kernel); one that blocks goes through `switch_out`.
//! - FXSAVE, not XSAVE: XCR0 would be x87|SSE (nothing here saves YMM), which is exactly what FXSAVE covers; CR4.OSXSAVE stays clear,
//!   so CPUID reports OSXSAVE=0, AVX/VEX is a clean `#UD` and libc dispatchers pick their SSE2 paths. Lazy CR0.TS/#NM is not used:
//!   the U2.5 first-entry `fninit` scrub in `user_task_trampoline` runs at CPL 0 and would itself trap with TS set.
//! - One slot per process in a lock-free static table keyed by PML4 (the `FS_TAB` shape) — `fork` copies the parent's state into
//!   it before the child task exists, which is why a Linux process does not use `Task::fpu`. A process with no slot (table full)
//!   runs with OSFXSR clear: its first SSE instruction is `#UD` → SIGSEGV zombie. Fail closed.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use super::super::gdt::MAX_CPUS;
use crate::arch::sched::fpu::{core_init, fxrstor, fxsave, set_osfxsr, FpuArea as Area, INIT};
pub use crate::arch::sched::fpu::{supported, CR4_FLIPS, MXCSR_INIT};

const SLOTS: usize = 32;

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

pub static SAVES: AtomicU64 = AtomicU64::new(0);
pub static RESTORES: AtomicU64 = AtomicU64::new(0);
pub static FORK_COPIES: AtomicU64 = AtomicU64::new(0);

/// `AT_HWCAP` for the auxv: on x86 Linux it is CPUID.1:EDX.
pub fn hwcap() -> u64 {
    core::arch::x86_64::__cpuid(1).edx as u64
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

/// `sched::fpu::dispatch`'s Linux arm (IF=0, before the switch into the task). `true` = `name` is a Linux task and was handled
/// here: its slot restored with CR4.OSFXSR set, or — no slot — OSFXSR cleared (fail closed). `false` = not a Linux task.
pub fn dispatch(cpu: usize, cr3: u64, name: &str) -> bool {
    if cpu >= MAX_CPUS || name != super::TASK_NAME {
        return false;
    }
    let cr3 = super::thread::key_for(cr3); // SELFBUILD2: one slot per THREAD key
    if cr3 != 0 && super::ACTIVE.load(Ordering::Acquire) && supported() {
        if let Some(i) = claim(cr3, &INIT) {
            core_init(cpu);
            set_osfxsr(cpu, true);
            unsafe { fxrstor(TAB[i].area.get()) };
            LIVE_IDX[cpu].store(i, Ordering::Relaxed);
            LIVE_KEY[cpu].store(cr3, Ordering::Relaxed);
            RESTORES.fetch_add(1, Ordering::Relaxed);
            return true;
        }
    }
    core_init(cpu); // makes the CR4 shadow exact, so the clear below is a real write
    set_osfxsr(cpu, false);
    true
}

/// `sched::fpu::switch_out`'s Linux arm (IF=0, the task just returned to the scheduler). One relaxed load when no Linux state is live.
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

/// SELFBUILD2 (signal delivery): the CURRENT Linux task's x87/SSE file — live in the registers while it runs in the kernel — as
/// an FXSAVE image. `false` = no Linux FP state is live on this core.
pub fn save_live(out: &mut [u8; 512]) -> bool {
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    if cpu >= MAX_CPUS || LIVE_KEY[cpu].load(Ordering::Relaxed) == 0 {
        return false;
    }
    let mut a = Area([0; 512]);
    unsafe { fxsave(&mut a) };
    out.copy_from_slice(&a.0);
    true
}

/// SELFBUILD2 (`rt_sigreturn`): load a user-supplied FXSAVE image into the current Linux task's registers. MXCSR is masked to
/// what this CPU accepts first — a reserved MXCSR bit makes `fxrstor` #GP at CPL 0.
pub fn restore_live(img: &[u8; 512]) -> bool {
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    if cpu >= MAX_CPUS || LIVE_KEY[cpu].load(Ordering::Relaxed) == 0 {
        return false;
    }
    let mut cur = Area([0; 512]);
    unsafe { fxsave(&mut cur) };
    let mut mask = u32::from_le_bytes([cur.0[28], cur.0[29], cur.0[30], cur.0[31]]);
    if mask == 0 {
        mask = 0xFFBF; // the architectural default MXCSR_MASK
    }
    let mut a = Area(*img);
    let mx = u32::from_le_bytes([a.0[24], a.0[25], a.0[26], a.0[27]]) & mask;
    a.0[24..28].copy_from_slice(&mx.to_le_bytes());
    a.0[28..32].copy_from_slice(&[0; 4]);
    unsafe { fxrstor(&a) };
    true
}
