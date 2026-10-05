// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! LINUXABI (rung 1 of self-hosting, ROADMAP §1c SH-5) — run a STATIC Linux x86_64 ELF as a ring-3 program.
//!
//! One process at a time. `linux <path> [args…]` reads the file, builds a PRIVATE address space (own PML4;
//! the kernel half is shared exactly like a slot's), maps the `PT_LOAD` segments at their fixed vaddrs
//! (Linux static binaries link at 0x400000, which the 16 KiB slot window cannot hold, so this layer owns
//! its page tables), builds the System V initial stack, and spawns a pinned preemptible ring-3 task named
//! [`TASK_NAME`]. The SYSCALL stub is unchanged in behaviour: `syscall_dispatch` asks
//! [`is_linux_task`] first and routes to [`dispatch`], which speaks the LINUX numbers.
//!
//! Layout (all private to the process): ELF at its own vaddrs (< 16 MiB, checked to be free RAM, never the
//! kernel image/heap); brk at [`BRK_BASE`]; anonymous/file mmaps bump up from [`MMAP_BASE`]; stack top at
//! [`STACK_TOP`] — everything but the ELF lives under PML4[2], which is NEVER inherited from the kernel.
//!
//! Memory safety: every user pointer is resolved by a SOFTWARE walk of the process's own tables
//! ([`AddrSpace::copy_in`]/[`AddrSpace::copy_out`]): the page must be present+USER (+WRITABLE for a write)
//! and the kernel touches the page through its identity-mapped frame, never through the user VA. A bad
//! pointer is `-EFAULT`, not a kernel fault.
//!
//! LINUXABI2 (rung 2): a PROCESS TABLE (`proc.rs`), fork (eager page copy), execve, wait4, pipes, dup, getdents64, stdin from the shell
//! window, files writable under `/home/<user>/` (`fd.rs`, `sys.rs`). Blocking syscalls are RETRY loops in [`dispatch`].
//!
//! Known limits: no threads/signals delivery (kill = end the task); fork copies eagerly (no CoW fault hook); FS_BASE is per-core, so every
//! process is PINNED to one core (the scheduler hook re-asserts it per switch-in); files are slurped whole at open; x87/SSE state is
//! saved per process (LINUXABI3, `fpu.rs`: eager FXSAVE, CR4.OSFXSR only while a Linux task runs; no AVX); the cloned low-memory page-table copy does not see kernel mapping
//! edits made while the process runs.

pub mod elf;
pub mod fd;
pub mod fpu;
pub mod proc;
pub mod selfbuild;
pub mod sys;
pub mod sys2;

use alloc::collections::BTreeSet;
use alloc::sync::Arc;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

use fd::FdEnt;

use super::memory;

/// Task name — `syscall_dispatch` and `record_ring3_kill` match on it.
pub const TASK_NAME: &str = "linuxabi";
pub const PAGE: u64 = 4096;
pub const BRK_BASE: u64 = 0x0000_0100_0100_0000;
pub const BRK_MAX: u64 = 64 << 20;
pub const MMAP_BASE: u64 = 0x0000_0100_4000_0000;
pub const MMAP_LIMIT: u64 = 0x0000_0100_7000_0000;
pub const STACK_TOP: u64 = 0x0000_0100_8000_0000;
pub const STACK_PAGES: u64 = 128;
/// Total data pages one process may hold (96 MiB) — fail-closed `-ENOMEM`.
pub const MAX_PAGES: usize = 24576;

const P: u64 = 1;
const W: u64 = 2;
const U: u64 = 4;
const PS: u64 = 1 << 7;
const NX: u64 = 1 << 63;
const ADDR: u64 = 0x000F_FFFF_FFFF_F000;
const CANON_MAX: u64 = 0x0000_8000_0000_0000;

// ---------------------------------------------------------------------------------------------
// Address space
// ---------------------------------------------------------------------------------------------

/// A private x86_64 address space built over the kernel's. Owns every table it created/cloned and every
/// data frame; [`AddrSpace::release`] frees them all.
pub struct AddrSpace {
    pub pml4: u64,
    tables: Vec<u64>,
    frames: BTreeSet<u64>,
}

fn dealloc_frame(f: u64) {
    if let Ok(l) = core::alloc::Layout::from_size_align(4096, 4096) {
        unsafe { alloc::alloc::dealloc(f as *mut u8, l) };
    }
}

impl AddrSpace {
    pub fn new() -> Self {
        let pml4 = memory::alloc_page_frame();
        unsafe {
            let k = (memory::kernel_cr3() & ADDR) as *const u64;
            let p = pml4 as *mut u64;
            for i in 0..512 {
                *p.add(i) = *k.add(i);
            }
            // PML4[2] is the shared U1a window / the slots' private window: never inherit it.
            *p.add(2) = 0;
        }
        let mut tables = Vec::new();
        tables.push(pml4);
        AddrSpace { pml4, tables, frames: BTreeSet::new() }
    }

    pub fn pages(&self) -> usize {
        self.frames.len()
    }

    fn new_table(&mut self) -> u64 {
        let f = memory::alloc_page_frame();
        self.tables.push(f);
        f
    }

    fn clone_table(&mut self, src: u64) -> u64 {
        let f = memory::alloc_page_frame();
        unsafe { core::ptr::copy_nonoverlapping(src as *const u64, f as *mut u64, 512) };
        self.tables.push(f);
        f
    }

    /// Replace a huge leaf (`shift` 30 = 1 GiB, 21 = 2 MiB) by an equivalent table one level down.
    fn split_huge(&mut self, e: u64, shift: u32) -> u64 {
        let f = self.new_table();
        let t = f as *mut u64;
        unsafe {
            if shift == 30 {
                let mask = 0x000F_FFFF_C000_0000u64;
                let (base, flags) = (e & mask, e & !mask);
                for i in 0..512u64 {
                    *t.add(i as usize) = (base + i * (2 << 20)) | flags;
                }
            } else {
                let mask = 0x000F_FFFF_FFE0_0000u64;
                let (base, all) = (e & mask, e & !mask);
                let mut fl = all & !PS & !(1 << 12);
                if all & (1 << 12) != 0 {
                    fl |= PS; // the 2M PAT bit (12) is the 4K PAT bit (7)
                }
                for i in 0..512u64 {
                    *t.add(i as usize) = (base + i * 4096) | fl;
                }
            }
        }
        f
    }

    /// Walk to `va`'s leaf slot, creating / cloning / splitting private tables on the way.
    fn leaf_slot(&mut self, va: u64) -> *mut u64 {
        let mut tbl = self.pml4 as *mut u64;
        for shift in [39u32, 30, 21] {
            let idx = ((va >> shift) & 511) as usize;
            let ep = unsafe { tbl.add(idx) };
            let e = unsafe { *ep };
            let next = if e & P == 0 {
                self.new_table()
            } else if e & PS != 0 && shift != 39 {
                self.split_huge(e, shift)
            } else {
                let t = e & ADDR;
                if self.tables.contains(&t) { t } else { self.clone_table(t) }
            };
            unsafe { *ep = next | P | W | U };
            tbl = next as *mut u64;
        }
        unsafe { tbl.add(((va >> 12) & 511) as usize) }
    }

    /// Software walk (no allocation): the leaf PTE for `va`, if present.
    pub fn pte(&self, va: u64) -> Option<u64> {
        if va >= CANON_MAX {
            return None;
        }
        let mut tbl = self.pml4 as *const u64;
        for shift in [39u32, 30, 21, 12] {
            let e = unsafe { *tbl.add(((va >> shift) & 511) as usize) };
            if e & P == 0 {
                return None;
            }
            if shift == 12 || (e & PS != 0 && shift != 39) {
                return Some(e);
            }
            tbl = (e & ADDR) as *const u64;
        }
        None
    }

    /// A USER page of this process: `(frame, pte)`. Kernel (non-USER) leaves answer `None`.
    fn user_page(&self, va: u64) -> Option<(u64, u64)> {
        let e = self.pte(va & !(PAGE - 1))?;
        if e & U == 0 || e & PS != 0 {
            return None;
        }
        Some((e & ADDR, e))
    }

    pub fn is_mapped(&self, va: u64) -> bool {
        self.user_page(va).is_some()
    }

    fn leaf_bits(w: bool, x: bool) -> u64 {
        P | U | if w { W } else { 0 } | if x { 0 } else { NX }
    }

    /// Map one fresh zeroed page at `va`. `false` = already mapped, page cap hit, or W+X asked.
    pub fn map_new(&mut self, va: u64, w: bool, x: bool) -> bool {
        if (w && x) || va & (PAGE - 1) != 0 || va >= CANON_MAX || self.frames.len() >= MAX_PAGES {
            return false;
        }
        if self.is_mapped(va) {
            return false;
        }
        let f = memory::alloc_page_frame();
        self.frames.insert(f);
        let slot = self.leaf_slot(va);
        unsafe { *slot = (f & ADDR) | Self::leaf_bits(w, x) };
        true
    }

    /// Change the permissions of an already-mapped page.
    pub fn set_perms(&mut self, va: u64, w: bool, x: bool) -> bool {
        if w && x {
            return false;
        }
        let Some((f, _)) = self.user_page(va) else { return false };
        let slot = self.leaf_slot(va & !(PAGE - 1));
        unsafe { *slot = (f & ADDR) | Self::leaf_bits(w, x) };
        true
    }

    /// Unmap `va`'s page and free its frame. `false` if it was not mapped.
    pub fn unmap(&mut self, va: u64) -> bool {
        let Some((f, _)) = self.user_page(va) else { return false };
        let slot = self.leaf_slot(va & !(PAGE - 1));
        unsafe { *slot = 0 };
        if self.frames.remove(&f) {
            dealloc_frame(f);
        }
        true
    }

    /// Copy `out.len()` bytes from user `va`. `false` = some page unmapped / not USER.
    pub fn copy_in(&self, va: u64, out: &mut [u8]) -> bool {
        let Some(end) = va.checked_add(out.len() as u64) else { return false };
        if end > CANON_MAX {
            return false;
        }
        let (mut a, mut done) = (va, 0usize);
        while done < out.len() {
            let Some((f, _)) = self.user_page(a) else { return false };
            let off = (a & (PAGE - 1)) as usize;
            let n = core::cmp::min(4096 - off, out.len() - done);
            unsafe { core::ptr::copy_nonoverlapping((f as *const u8).add(off), out.as_mut_ptr().add(done), n) };
            done += n;
            a += n as u64;
        }
        true
    }

    /// Copy into user `va`. `force` (the loader only) ignores the WRITABLE bit; ring 3 never gets it.
    pub fn copy_out(&self, va: u64, data: &[u8], force: bool) -> bool {
        let Some(end) = va.checked_add(data.len() as u64) else { return false };
        if end > CANON_MAX {
            return false;
        }
        // Validate the whole range first so a bad pointer writes nothing.
        let mut a = va & !(PAGE - 1);
        while a < end {
            match self.user_page(a) {
                Some((_, e)) if force || e & W != 0 => {}
                _ => return false,
            }
            a += PAGE;
        }
        let (mut a, mut done) = (va, 0usize);
        while done < data.len() {
            let Some((f, _)) = self.user_page(a) else { return false };
            let off = (a & (PAGE - 1)) as usize;
            let n = core::cmp::min(4096 - off, data.len() - done);
            unsafe { core::ptr::copy_nonoverlapping(data.as_ptr().add(done), (f as *mut u8).add(off), n) };
            done += n;
            a += n as u64;
        }
        true
    }

    /// Read a NUL-terminated string (≤ `max` bytes) from user memory.
    pub fn read_cstr(&self, va: u64, max: usize) -> Option<Vec<u8>> {
        let mut v = Vec::new();
        let mut a = va;
        while v.len() < max {
            let mut b = [0u8; 1];
            if !self.copy_in(a, &mut b) {
                return None;
            }
            if b[0] == 0 {
                return Some(v);
            }
            v.push(b[0]);
            a += 1;
        }
        None
    }

    /// LINUXABI2: is `[va, va+len)` entirely mapped USER+WRITABLE? (validate BEFORE a read consumes pipe/stdin bytes)
    pub fn writable_range(&self, va: u64, len: usize) -> bool {
        let Some(end) = va.checked_add(len as u64) else { return false };
        if end > CANON_MAX {
            return false;
        }
        let mut a = va & !(PAGE - 1);
        while a < end {
            match self.user_page(a) {
                Some((_, e)) if e & W != 0 => {}
                _ => return false,
            }
            a += PAGE;
        }
        true
    }

    /// LINUXABI2 fork: a new address space holding a byte-for-byte EAGER copy of every private data page (same perms).
    /// No copy-on-write: this kernel's page-fault path has no hook to resolve a write fault on a shared read-only page.
    pub fn fork_copy(&self) -> Option<AddrSpace> {
        let mut list: Vec<(u64, u64, u64)> = Vec::new();
        let priv_tbl = |e: u64| -> Option<*const u64> {
            if e & P == 0 || e & PS != 0 || !self.tables.contains(&(e & ADDR)) {
                None
            } else {
                Some((e & ADDR) as *const u64)
            }
        };
        unsafe {
            let t4 = self.pml4 as *const u64;
            for i4 in 0..512u64 {
                let Some(t3) = priv_tbl(*t4.add(i4 as usize)) else { continue };
                for i3 in 0..512u64 {
                    let Some(t2) = priv_tbl(*t3.add(i3 as usize)) else { continue };
                    for i2 in 0..512u64 {
                        let Some(t1) = priv_tbl(*t2.add(i2 as usize)) else { continue };
                        for i1 in 0..512u64 {
                            let e = *t1.add(i1 as usize);
                            if e & P != 0 && e & U != 0 && self.frames.contains(&(e & ADDR)) {
                                list.push(((i4 << 39) | (i3 << 30) | (i2 << 21) | (i1 << 12), e & ADDR, e));
                            }
                        }
                    }
                }
            }
        }
        let mut c = AddrSpace::new();
        for (va, f, e) in list {
            if !c.map_new(va, e & W != 0, e & NX == 0) {
                c.free_frames();
                return None;
            }
            let Some((cf, _)) = c.user_page(va) else {
                c.free_frames();
                return None;
            };
            unsafe { core::ptr::copy_nonoverlapping(f as *const u8, cf as *mut u8, 4096) };
        }
        Some(c)
    }

    /// LINUXABI2 execve: drop every user mapping but KEEP this PML4 (the task's `user_cr3` must stay valid). The kernel half
    /// is restored from the kernel's PML4, the TLB is flushed (caller is running on this space), THEN the old tables/frames are freed.
    pub fn reset(&mut self) {
        unsafe {
            let k = (memory::kernel_cr3() & ADDR) as *const u64;
            let p = self.pml4 as *mut u64;
            for i in 0..512 {
                *p.add(i) = *k.add(i);
            }
            *p.add(2) = 0;
            memory::load_cr3(memory::current_cr3());
        }
        for f in core::mem::take(&mut self.frames) {
            dealloc_frame(f);
        }
        let pml4 = self.pml4;
        for t in core::mem::take(&mut self.tables) {
            if t != pml4 {
                dealloc_frame(t);
            }
        }
        self.tables.push(pml4);
    }

    /// Free every frame and table. Call only when no core can be running on `pml4`.
    pub fn free_frames(&mut self) {
        for f in core::mem::take(&mut self.frames) {
            dealloc_frame(f);
        }
        for t in core::mem::take(&mut self.tables) {
            dealloc_frame(t);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Process state
// ---------------------------------------------------------------------------------------------

pub struct LinuxProc {
    pub asp: AddrSpace,
    pub brk: u64,
    pub brk_mapped: u64,
    pub mmap_next: u64,
    pub fds: Vec<Option<FdEnt>>,
    pub fs_base: u64,
    pub cwd: String,
    pub umask: u32,
    /// nanosleep/poll deadline (ms) while a RETRY loop is waiting.
    pub sleep_until: Option<u64>,
    /// SELFBUILD1: the image path as the VFS resolved it — what `readlink("/proc/self/exe")` answers.
    pub exe: String,
}

static ACTIVE: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);
static VIA_GROUP: AtomicBool = AtomicBool::new(false);
static EXIT_CODE: AtomicI64 = AtomicI64::new(-1);
/// `vec + 1` of a ring-3 fault that killed the ROOT process, 0 = none.
static FAULT: AtomicU64 = AtomicU64::new(0);
static FAULT_CR2: AtomicU64 = AtomicU64::new(0);
static NSYS: AtomicU64 = AtomicU64::new(0);
static ENOSYS_LIST: spin::Mutex<Vec<u32>> = spin::Mutex::new(Vec::new());

pub fn note_enosys(nr: u64) {
    let n = nr as u32;
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut l = ENOSYS_LIST.lock();
        if !l.contains(&n) {
            l.push(n);
            serial_println!("[linuxabi] enosys nr={}", nr);
        }
    });
}

pub fn root_exited(code: u64, group: bool) {
    VIA_GROUP.store(group, Ordering::Release);
    EXIT_CODE.store((code & 0xff) as i64, Ordering::Release);
    DONE.store(true, Ordering::Release);
}

// ---- FS_BASE per process (lock-free: read at the scheduler dispatch site) ----
static FS_TAB: [(AtomicU64, AtomicU64); 32] = [const { (AtomicU64::new(0), AtomicU64::new(0)) }; 32];

pub fn fs_tab_set(cr3: u64, fs: u64) {
    for (k, v) in FS_TAB.iter() {
        let cur = k.load(Ordering::Acquire);
        if cur == cr3 || (cur == 0 && k.compare_exchange(0, cr3, Ordering::AcqRel, Ordering::Acquire).is_ok()) {
            v.store(fs, Ordering::Release);
            return;
        }
    }
}

pub fn fs_tab_clear(cr3: u64) {
    for (k, v) in FS_TAB.iter() {
        if k.load(Ordering::Acquire) == cr3 {
            v.store(0, Ordering::Release);
            k.store(0, Ordering::Release);
        }
    }
}

/// Scheduler dispatch hook: the task about to run is in address space `cr3` — if it is one of ours, restore its FS_BASE.
pub fn on_dispatch(cr3: u64) {
    if !ACTIVE.load(Ordering::Acquire) {
        return;
    }
    for (k, v) in FS_TAB.iter() {
        if k.load(Ordering::Acquire) == cr3 {
            let fs = v.load(Ordering::Acquire);
            if fs != 0 {
                if let Ok(va) = x86_64::VirtAddr::try_new(fs) {
                    x86_64::registers::model_specific::FsBase::write(va);
                }
            }
            return;
        }
    }
}

// ---- fork child register files, keyed by the child's PML4 ----
static FORK_REGS: spin::Mutex<Vec<(u64, [u64; 6])>> = spin::Mutex::new(Vec::new());

pub fn push_fork_regs(cr3: u64, r: [u64; 6]) {
    x86_64::instructions::interrupts::without_interrupts(|| FORK_REGS.lock().push((cr3, r)));
}

/// Called by `user_task_trampoline` (IF masked) for every first ring-3 entry: `Some` only for a fork child.
pub fn take_fork_regs(cr3: u64) -> Option<[u64; 6]> {
    if cr3 == 0 {
        return None;
    }
    let mut g = FORK_REGS.lock();
    let i = g.iter().position(|(c, _)| *c == cr3)?;
    Some(g.swap_remove(i).1)
}

/// Read user r8 (Linux arg 4) from the per-CPU scratch the SYSCALL stub parks it in (`linuxabi_r8!`).
/// MUST be the first thing `syscall_dispatch` does: the slot is a scratch, valid until IF opens.
#[inline(always)]
pub fn take_user_r8() -> u64 {
    let v: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[{o}]", out(reg) v,
            o = const super::percpu::USER_RSP_OFFSET, options(nostack, readonly, preserves_flags));
    }
    v
}

/// This task's kernel-stack top (the SYSCALL stub's frame anchor). Read BEFORE anything can block/yield.
#[inline(always)]
fn take_ktop() -> u64 {
    let v: u64;
    unsafe {
        core::arch::asm!("mov {}, gs:[{o}]", out(reg) v,
            o = const super::percpu::KERNEL_RSP_OFFSET, options(nostack, readonly, preserves_flags));
    }
    v
}

pub fn is_linux_task() -> bool {
    ACTIVE.load(Ordering::Acquire) && crate::arch::sched::current_name() == Some(TASK_NAME)
}

/// Called from `record_ring3_kill` when a Linux task faults.
pub fn note_fault(vec: u8, _err: u64, cr2: u64) {
    if let Some(i) = proc::cur_info() {
        if i.pid == proc::ROOT_PID.load(Ordering::Acquire) {
            FAULT_CR2.store(cr2, Ordering::Release);
            FAULT.store(vec as u64 + 1, Ordering::Release);
        }
        i.finish(11);
    }
}

/// The Linux syscall entry (called from `syscall_dispatch` in the task's context, IF masked by SFMASK).
pub fn dispatch(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> i64 {
    let ktop = take_ktop();
    let Some(info) = proc::cur_info() else { return -38 };
    NSYS.fetch_add(1, Ordering::AcqRel);
    let traced = TRACE_N.fetch_add(1, Ordering::AcqRel) < TRACE_MAX; // LINUXABI3 M5: the first TRACE_MAX syscalls of a session
    if nr == 60 || nr == 231 {
        if traced {
            serial_println!("[linux] sys={} {} pid={} a0={:#x} -> (exit)", nr, sys_name(nr), info.pid, a0);
        }
        proc::exit_current(&info, a0, nr == 231); // never returns
    }
    let args = [a0, a1, a2, a3, a4, a5];
    let lp = info.lp.clone();
    let mut blocked = false;
    let rc = loop {
        let rc = {
            let mut p = fd::lk(&lp);
            sys::handle(&mut p, &info, ktop, nr, args)
        };
        if rc != sys::RETRY {
            break rc;
        }
        if !blocked {
            // LINUXABI3 M5: name every syscall that BLOCKS (always, not capped), and publish it so a timeout can say which.
            blocked = true;
            BLOCKED.store(nr + 1, Ordering::Release);
            BLOCKED_A0.store(a0, Ordering::Release);
            BLOCKED_T.store(crate::arch::ms(), Ordering::Release);
            serial_println!("[linux] sys={} {} pid={} a0={:#x} a1={:#x} a2={:#x} -> blocks", nr, sys_name(nr), info.pid, a0, a1, a2);
        }
        // would block: no lock is held now. Let the others (pipe peer, children, the verb's key pump) run.
        proc::gc();
        crate::arch::sched::kill_check_current();
        crate::arch::sched::yield_now();
    };
    if blocked {
        let _ = BLOCKED.compare_exchange(nr + 1, 0, Ordering::AcqRel, Ordering::Acquire);
    }
    if traced || blocked {
        serial_println!("[linux] sys={} {} pid={} a0={:#x} a1={:#x} a2={:#x} -> {}", nr, sys_name(nr), info.pid, a0, a1, a2, rc);
    }
    crate::arch::sched::kill_check_current();
    rc
}

// ---- LINUXABI3 M5: syscall trace + "which syscall is it stuck in" ----
const TRACE_MAX: u64 = 48;
static TRACE_N: AtomicU64 = AtomicU64::new(0);
/// `nr + 1` of the syscall a process of this session is blocked in right now (0 = none), its first arg, and since when (ms).
static BLOCKED: AtomicU64 = AtomicU64::new(0);
static BLOCKED_A0: AtomicU64 = AtomicU64::new(0);
static BLOCKED_T: AtomicU64 = AtomicU64::new(0);

/// Linux x86_64 syscall names for the trace and the FAIL line (the ones this layer answers, plus the usual libc start-up set).
pub fn sys_name(nr: u64) -> &'static str {
    match nr {
        0 => "read", 1 => "write", 2 => "open", 3 => "close", 4 => "stat", 5 => "fstat", 6 => "lstat", 7 => "poll",
        8 => "lseek", 9 => "mmap", 10 => "mprotect", 11 => "munmap", 12 => "brk", 13 => "rt_sigaction",
        14 => "rt_sigprocmask", 15 => "rt_sigreturn", 16 => "ioctl", 17 => "pread64", 19 => "readv", 20 => "writev",
        21 => "access", 22 => "pipe", 23 => "select", 24 => "sched_yield", 32 => "dup", 33 => "dup2", 35 => "nanosleep",
        39 => "getpid", 56 => "clone", 57 => "fork", 58 => "vfork", 59 => "execve", 60 => "exit", 61 => "wait4",
        62 => "kill", 63 => "uname", 72 => "fcntl", 78 => "getdents", 79 => "getcwd", 80 => "chdir", 82 => "rename",
        83 => "mkdir", 87 => "unlink", 89 => "readlink", 96 => "gettimeofday", 97 => "getrlimit", 102 => "getuid",
        104 => "getgid", 107 => "geteuid", 108 => "getegid", 109 => "setpgid", 110 => "getppid", 111 => "getpgrp",
        121 => "getpgid", 158 => "arch_prctl", 186 => "gettid", 200 => "tkill", 202 => "futex", 217 => "getdents64",
        218 => "set_tid_address", 228 => "clock_gettime", 230 => "clock_nanosleep", 231 => "exit_group",
        234 => "tgkill", 247 => "waitid", 257 => "openat", 262 => "newfstatat", 267 => "readlinkat", 269 => "faccessat",
        273 => "set_robust_list", 293 => "pipe2", 302 => "prlimit64", 318 => "getrandom", 332 => "statx", 334 => "rseq",
        131 => "sigaltstack", 157 => "prctl", 204 => "sched_getaffinity", 292 => "dup3", 52 => "getpeername", 435 => "clone3", // SELFBUILD1
        _ => "?",
    }
}

/// The blocked-syscall tag for a FAIL line: `read(fd=0)`, `wait4`, … (empty when nothing is blocked).
fn blocked_tag() -> String {
    let b = BLOCKED.load(Ordering::Acquire);
    if b == 0 {
        return String::new();
    }
    let nr = b - 1;
    match nr {
        0 | 1 | 3 | 5 | 16 | 17 | 72 | 217 => alloc::format!("{}(fd={})", sys_name(nr), BLOCKED_A0.load(Ordering::Acquire) as i64),
        _ => alloc::format!("{}({})", sys_name(nr), nr),
    }
}

// ---------------------------------------------------------------------------------------------
// Run one program (a SESSION: the root process and everything it forks)
// ---------------------------------------------------------------------------------------------

pub struct Report {
    pub pass: bool,
    pub exit: String,
    pub nsys: u64,
    pub enosys: Vec<u32>,
    pub ms: u64,
    pub forks: u64,
    pub child_ok: u64,
    /// LINUXABI3 M5: the syscall a process was blocked in when the session timed out / was interrupted (empty otherwise).
    pub blocked: String,
}

impl Report {
    pub fn witness(&self, path: &str) -> String {
        let mut e = String::new();
        for (i, n) in self.enosys.iter().enumerate() {
            if i > 0 {
                e.push(',');
            }
            e.push_str(&alloc::format!("{}", n));
        }
        let blk = if self.blocked.is_empty() { String::new() } else { alloc::format!(" blocked={}", self.blocked) };
        alloc::format!(
            ":: LINUXABI: path={} exit={} syscalls={} enosys=[{}] ms={}{} -> {} ::",
            path, self.exit, self.nsys, e, self.ms, blk, if self.pass { "PASS" } else { "FAIL" }
        )
    }
}

pub(crate) fn read_image(path: &str) -> Result<(String, Vec<u8>), String> {
    use crate::fs::vfs::NodeKind;
    let full = crate::shell::vfs_path(path);
    let mt = crate::shell::vfs_mount_table();
    let st = mt.stat(&full).map_err(|_| alloc::format!("{}: no such file (-ENOENT)", full))?;
    if matches!(st.kind, NodeKind::Dir) {
        return Err(alloc::format!("{}: is a directory (-EISDIR)", full));
    }
    if st.size == 0 || st.size > (128 << 20) {
        return Err(alloc::format!("{}: size {} out of range (-E2BIG)", full, st.size));
    }
    let bytes = mt.read(&full, 0, st.size as usize).map_err(|_| alloc::format!("{}: read failed (-EIO)", full))?;
    if bytes.len() as u64 != st.size {
        return Err(alloc::format!("{}: short read {} of {} (-EIO)", full, bytes.len(), st.size));
    }
    Ok((full, bytes))
}

/// Load `path` as a static Linux ELF and run its session to completion (or `deadline_ms`). `interactive` pumps the keyboard into
/// the session's stdin; `out` receives each completed line of the session's stdout/stderr.
pub fn run_path(path: &str, argv: &[&str], deadline_ms: u64, interactive: bool, out: &mut dyn FnMut(&str)) -> Result<Report, String> {
    if ACTIVE.swap(true, Ordering::AcqRel) {
        return Err(String::from("another Linux program is running (one session at a time)"));
    }
    let r = run_inner(path, argv, deadline_ms, interactive, out);
    ACTIVE.store(false, Ordering::Release);
    r
}

fn drain_out(pend: &mut String, out: &mut dyn FnMut(&str), flush: bool) {
    let b = fd::out_take();
    if !b.is_empty() {
        pend.push_str(&String::from_utf8_lossy(&b));
    }
    while let Some(i) = pend.find('\n') {
        let line: String = String::from(pend[..i].trim_end_matches('\r'));
        out(&line);
        pend.drain(..=i);
    }
    if flush && !pend.is_empty() {
        let l = core::mem::take(pend);
        out(&l);
    }
}

/// The verb's key pump (the `vug`/`pulse` seam): edits one line and, on Enter, hands it to the session's stdin. `true` = Ctrl-C.
fn pump_keys(edit: &mut String, pend: &mut String, out: &mut dyn FnMut(&str)) -> bool {
    for _ in 0..64 {
        let Some(ev) = crate::pal::pump_and_poll() else { return false };
        if let crate::pal::Event::Key(c) = ev {
            match c {
                3 => return true,
                4 => {
                    // LINUXABI3 M5: Ctrl-D — a partial line is sent as-is (no newline); on an empty line it is end of input.
                    if edit.is_empty() {
                        fd::stdin_eof_set();
                    } else {
                        pend.push_str(edit);
                        fd::stdin_push(core::mem::take(edit).as_bytes());
                    }
                }
                b'\n' | b'\r' => {
                    pend.push_str(edit);
                    pend.push('\n'); // the echo: prompt-so-far + what was typed
                    let mut l = core::mem::take(edit);
                    l.push('\n');
                    fd::stdin_push(l.as_bytes());
                    drain_out(pend, out, false);
                }
                8 | 0x7f => {
                    edit.pop();
                }
                0x20..=0x7e => {
                    if edit.len() < 1024 {
                        edit.push(c as char);
                    }
                }
                _ => {}
            }
        }
    }
    false
}

fn run_inner(path: &str, argv: &[&str], deadline_ms: u64, interactive: bool, out: &mut dyn FnMut(&str)) -> Result<Report, String> {
    let (full, bytes) = read_image(path)?;
    let plan = elf::parse(&bytes).map_err(String::from)?;
    let mut asp = AddrSpace::new();
    if let Err(e) = elf::load(&mut asp, &bytes, &plan) {
        asp.free_frames();
        return Err(String::from(e));
    }
    let envp = ["PATH=/apps:/", "HOME=/", "TERM=linux"];
    let sp = match elf::build_stack(&mut asp, &plan, &full, argv, &envp) {
        Ok(s) => s,
        Err(e) => {
            asp.free_frames();
            return Err(String::from(e));
        }
    };
    serial_println!(
        "[linuxabi] load path={} segs={} entry={:#x} stack={:#x} brk={:#x}",
        full, plan.segs.len(), plan.entry, sp, BRK_BASE
    );
    let con = fd::Desc::new(fd::Kind::Console, 0);
    let mut fds: Vec<Option<FdEnt>> = Vec::new();
    for _ in 0..3 {
        fds.push(Some(FdEnt { d: con.clone(), cloexec: false }));
    }
    drop(con);
    DONE.store(false, Ordering::Release);
    VIA_GROUP.store(false, Ordering::Release);
    EXIT_CODE.store(-1, Ordering::Release);
    FAULT.store(0, Ordering::Release);
    NSYS.store(0, Ordering::Release);
    TRACE_N.store(0, Ordering::Release);
    BLOCKED.store(0, Ordering::Release);
    let _ = fd::stdin_eof_take();
    ENOSYS_LIST.lock().clear();
    proc::reset_session();
    let _ = fd::out_take();
    let kill = Arc::new(crate::arch::sched::KillSwitch::new());
    let pid = proc::new_pid();
    proc::ROOT_PID.store(pid, Ordering::Release);
    let cwd = crate::shell::cwd_now();
    let root = proc::make_info(pid, 0, pid, asp, kill.clone(), |asp| LinuxProc {
        asp,
        brk: BRK_BASE,
        brk_mapped: BRK_BASE,
        mmap_next: MMAP_BASE,
        fds,
        fs_base: 0,
        cwd,
        umask: 0o022,
        sleep_until: None,
        exe: full.clone(),
    });
    let pml4 = root.pml4;
    proc::register(root.clone());
    let t0 = crate::arch::ms();
    // Pinned to THIS core (not CPU_AUTO): FS_BASE is per-core state; the dispatch hook re-asserts it per switch-in.
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    let _tid = crate::arch::sched::spawn_user_preemptible(TASK_NAME, plan.entry, sp, cpu, pml4, kill.clone());
    let deadline = crate::arch::ticks() + deadline_ms;
    let (mut pend, mut edit) = (String::new(), String::new());
    let mut ctrl_c = false;
    let mut hinted = false;
    while root.is_live() && crate::arch::ticks() < deadline && !ctrl_c {
        proc::gc();
        drain_out(&mut pend, out, false);
        if interactive {
            ctrl_c = pump_keys(&mut edit, &mut pend, out);
            // LINUXABI3 M5: a program waiting on stdin is not a hang — say so once, after 1 s, with any half-printed prompt shown.
            if !hinted && edit.is_empty() && BLOCKED.load(Ordering::Acquire) == 1 && BLOCKED_A0.load(Ordering::Acquire) == 0
                && !fd::stdin_has_data() && crate::arch::ms().saturating_sub(BLOCKED_T.load(Ordering::Acquire)) >= 1000
            {
                hinted = true;
                drain_out(&mut pend, out, true);
                out("(linux: the program is waiting for a line on stdin: type it + Enter; Ctrl-D = end of input, Ctrl-C = stop)");
            }
        }
        crate::arch::sched::yield_now();
    }
    let timed_out = root.is_live();
    let blocked = if timed_out { blocked_tag() } else { String::new() };
    // Let the finishing tasks leave their CR3 (sched::exit) before any table is freed.
    let settle = crate::arch::ticks() + 50;
    while crate::arch::ticks() < settle {
        drain_out(&mut pend, out, false);
        crate::arch::sched::yield_now();
    }
    // The session is the root's lifetime: end every survivor.
    let all = proc::snapshot();
    for i in all.iter().filter(|i| i.is_live()) {
        i.kill.request();
    }
    let kd = crate::arch::ticks() + 500;
    let mut safe_to_free = true;
    for i in all.iter() {
        let was_killed = !i.freed.load(Ordering::Acquire) && i.state.load(Ordering::Acquire) == -1;
        if was_killed {
            while !i.kill.is_reaped() && crate::arch::ticks() < kd {
                crate::arch::sched::yield_now();
            }
            if !i.kill.is_reaped() {
                safe_to_free = false;
            }
        }
    }
    let ms = crate::arch::ms().saturating_sub(t0);
    drain_out(&mut pend, out, true);
    let fault = FAULT.load(Ordering::Acquire);
    let st = root.state.load(Ordering::Acquire);
    let exit = if timed_out {
        String::from(if ctrl_c { "INTERRUPTED" } else { "TIMEOUT" })
    } else if fault != 0 {
        alloc::format!("FAULT(vec={} cr2={:#x})", fault - 1, FAULT_CR2.load(Ordering::Acquire))
    } else if st >= 256 || st == 0 {
        alloc::format!("{}", st >> 8)
    } else {
        alloc::format!("SIG{}", st)
    };
    let (forks, child_ok) = (proc::FORKS.load(Ordering::Acquire), proc::CHILD_EXIT_OK.load(Ordering::Acquire));
    let nsys = NSYS.load(Ordering::Acquire);
    let enosys = ENOSYS_LIST.lock().clone();
    for i in proc::snapshot() {
        if !i.freed.load(Ordering::Acquire) {
            if safe_to_free {
                let mut lp = i.lp.lock();
                lp.fds.clear();
                lp.asp.free_frames();
            } // else: leak the space rather than free tables a still-live task may be running on
            fs_tab_clear(i.pml4);
            fpu::release_slot(i.pml4); // LINUXABI3
        }
    }
    proc::reset_session();
    x86_64::instructions::interrupts::without_interrupts(|| FORK_REGS.lock().clear());
    x86_64::instructions::interrupts::without_interrupts(|| fd::STDIN.lock().clear());
    Ok(Report { pass: !timed_out && VIA_GROUP.load(Ordering::Acquire) && fault == 0, exit, nsys, enosys, ms, forks, child_ok, blocked })
}

// ---------------------------------------------------------------------------------------------
// Shell verb + `tests linuxabi` / `tests linuxabi2`
// ---------------------------------------------------------------------------------------------

/// `linux <path> [args…]` — runs interactively: the verb pumps the keyboard into the program's stdin (Ctrl-C ends the session).
pub fn shell_verb(args: &[&str], console: &mut crate::console::Console) {
    let Some(&path) = args.first() else {
        console.println("usage: linux <path> [args...]   (run a static Linux x86_64 ELF; Ctrl-C ends it)");
        return;
    };
    // LINUXABI3: busybox picks its applet from argv[0]'s basename and accepts only names starting with lowercase "busybox";
    // the FAT 8.3 name reads BUSYBOX.LNX, so hand it "busybox" (then `linux /apps/BUSYBOX.LNX ls /` runs the ls applet).
    let mut argv: Vec<&str> = args.to_vec();
    if is_busybox(path) {
        argv[0] = "busybox";
    }
    report(path, &argv, console);
}

fn is_busybox(path: &str) -> bool {
    let base = path.rsplit('/').next().unwrap_or(path);
    base.len() >= 7 && base.as_bytes()[..7].eq_ignore_ascii_case(b"busybox")
}

fn report(path: &str, argv: &[&str], console: &mut crate::console::Console) {
    let res = {
        let mut sink = |l: &str| console.println(l);
        run_path(path, argv, 600_000, true, &mut sink)
    };
    match res {
        Ok(r) => {
            let w = r.witness(path);
            console.println(&w);
            serial_println!("{}", w);
        }
        Err(e) => {
            console.println(&alloc::format!("linux: {}", e));
            serial_println!(":: LINUXABI: path={} exit=? syscalls=0 enosys=[] ms=0 -> FAIL ({}) ::", path, e);
        }
    }
}

/// `tests linuxabi` — run `/apps/HELLO.LNX`. Absent fixture = SKIP (staging is the arroyo/builder lane's).
pub fn selftest() {
    const P: &str = "/apps/HELLO.LNX";
    match run_path(P, &[P], 5_000, false, &mut |_| {}) {
        Ok(r) => serial_println!("{}", r.witness(P)),
        Err(e) if e.contains("-ENOENT") => {
            serial_println!(":: LINUXABI: path={} exit=? syscalls=0 enosys=[] ms=0 -> SKIP (fixture not staged) ::", P)
        }
        Err(e) => serial_println!(":: LINUXABI: path={} exit=? syscalls=0 enosys=[] ms=0 -> FAIL ({}) ::", P, e),
    }
    selfbuild::kat(); // SELFBUILD1 M2: the syscall known-answer tests (second witness, `:: LINUXABI-KAT:`)
}

/// `tests linuxabi2` — `PIPE.LNX` (fork + pipe + wait4) and `LS.LNX` (stdin line + getdents64 over `/`).
pub fn selftest2() {
    const PIPE: &str = "/apps/PIPE.LNX";
    const LS: &str = "/apps/LS.LNX";
    let mut cap1 = String::new();
    let r1 = run_path(PIPE, &[PIPE], 5_000, false, &mut |l| {
        cap1.push_str(l);
        cap1.push('\n');
    });
    let mut cap2 = String::new();
    fd::stdin_push(b"hello-stdin\n");
    let r2 = run_path(LS, &[LS], 5_000, false, &mut |l| {
        cap2.push_str(l);
        cap2.push('\n');
    });
    fd::stdin_take_line(1 << 16);
    let missing = |r: &Result<Report, String>| matches!(r, Err(e) if e.contains("-ENOENT"));
    // LINUXABI3 M5: a non-passing run prints its own witness first, which names the syscall it was blocked in on a timeout.
    for (r, path) in [(&r1, PIPE), (&r2, LS)] {
        if let Ok(rep) = r {
            if !rep.pass {
                serial_println!("{}", rep.witness(path));
            }
        }
    }
    if missing(&r1) || missing(&r2) {
        serial_println!(":: LINUXABI2: fork_ok=0 pipe_ok=0 dents=0 stdin=0 -> SKIP (fixtures not staged) ::");
        return;
    }
    let (mut fork_ok, mut pipe_ok) = (false, false);
    match &r1 {
        Ok(r) => {
            fork_ok = r.forks >= 1 && r.child_ok >= 1;
            pipe_ok = r.pass && r.exit == "0" && cap1.contains("ping");
        }
        Err(e) => serial_println!("[linuxabi] PIPE.LNX: {}", e),
    }
    let (mut dents, mut stdin_ok) = (0usize, false);
    match &r2 {
        Ok(r) => {
            let mut it = cap2.lines();
            stdin_ok = it.next() == Some("hello-stdin");
            dents = it.filter(|l| !l.is_empty()).count();
            if !r.pass {
                dents = 0;
            }
        }
        Err(e) => serial_println!("[linuxabi] LS.LNX: {}", e),
    }
    let pass = fork_ok && pipe_ok && dents >= 3 && stdin_ok;
    serial_println!(
        ":: LINUXABI2: fork_ok={} pipe_ok={} dents={} stdin={} -> {} ::",
        fork_ok as u8, pipe_ok as u8, dents, stdin_ok as u8, if pass { "PASS" } else { "FAIL" }
    );
}

/// `tests linuxabi3` — `SSE.LNX` (SSE/SSE2 in ring 3, MXCSR, a 64-byte SSE memcpy, then two forks that verify inherited xmm8..15,
/// mutate them across context switches and re-verify; the parent re-verifies its own after reaping) and, when an operator staged one,
/// a static musl `BUSYBOX.LNX ls /`. Absent SSE.LNX = SKIP (staging is the arroyo/builder lane's); absent busybox = `busybox=skip`.
pub fn selftest3() {
    const SSE: &str = "/apps/SSE.LNX";
    const BB: &str = "/apps/BUSYBOX.LNX";
    let r0 = fpu::RESTORES.load(Ordering::Relaxed);
    let (s0, k0, f0) = (fpu::SAVES.load(Ordering::Relaxed), fpu::FORK_COPIES.load(Ordering::Relaxed), fpu::CR4_FLIPS.load(Ordering::Relaxed));
    let mut cap = String::new();
    let r = run_path(SSE, &[SSE], 5_000, false, &mut |l| {
        cap.push_str(l);
        cap.push('\n');
    });
    if matches!(&r, Err(e) if e.contains("-ENOENT")) {
        serial_println!(":: LINUXABI3: cr4=? save=fx sse_lnx=skip fork_fp=skip busybox=skip -> SKIP (fixture not staged) ::");
        return;
    }
    let (mut sse_ok, mut fork_fp) = (false, false);
    match &r {
        Ok(rep) => {
            let clean = rep.pass && rep.exit == "0";
            sse_ok = clean && cap.lines().any(|l| l == "sse ok");
            fork_fp = clean && rep.forks >= 2 && rep.child_ok >= 2;
            if !clean {
                // SSE.LNX exits with the id of the check that failed: 11-16 parent (11 paddd, 12/13 cvt*, 14 initial MXCSR,
                // 15 parent xmm8..15 after the children, 16 MXCSR after the switches, 17 fork, 18/19 wait4), 21/22 a child.
                serial_println!("[linuxabi] SSE.LNX exit={} forks={} child_ok={} blocked={}", rep.exit, rep.forks, rep.child_ok, rep.blocked);
            }
        }
        Err(e) => serial_println!("[linuxabi] SSE.LNX: {}", e),
    }
    let restores = fpu::RESTORES.load(Ordering::Relaxed).wrapping_sub(r0);
    let mut cap2 = String::new();
    let rb = run_path(BB, &["busybox", "ls", "/"], 5_000, false, &mut |l| {
        cap2.push_str(l);
        cap2.push('\n');
    });
    let busybox = match &rb {
        Err(e) if e.contains("-ENOENT") => "skip",
        Ok(rep) if rep.pass && rep.exit == "0" && cap2.lines().filter(|l| !l.is_empty()).count() >= 3 => "ok",
        Ok(rep) => {
            serial_println!("{}", rep.witness(BB));
            "fail"
        }
        Err(e) => {
            serial_println!("[linuxabi] BUSYBOX.LNX: {}", e);
            "fail"
        }
    };
    let slots = fpu::in_use();
    serial_println!(
        "[linuxabi] fpu restores={} saves={} fork_copies={} cr4_flips={} slots_in_use_after={}",
        restores,
        fpu::SAVES.load(Ordering::Relaxed).wrapping_sub(s0),
        fpu::FORK_COPIES.load(Ordering::Relaxed).wrapping_sub(k0),
        fpu::CR4_FLIPS.load(Ordering::Relaxed).wrapping_sub(f0),
        slots
    );
    let ok = |b: bool| if b { "ok" } else { "fail" };
    let pass = sse_ok && fork_fp && busybox != "fail" && restores > 0 && slots == 0;
    serial_println!(
        ":: LINUXABI3: cr4={} save=fx sse_lnx={} fork_fp={} busybox={} -> {} ::",
        if restores > 0 { "osfxsr" } else { "off" },
        ok(sse_ok),
        ok(fork_fp),
        busybox,
        if pass { "PASS" } else { "FAIL" }
    );
}
