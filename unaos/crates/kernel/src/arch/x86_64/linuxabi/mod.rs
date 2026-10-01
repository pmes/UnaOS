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
//! Known limits (rung 1): no threads/fork/execve/signals; files are read-only and slurped whole at open;
//! the cloned low-memory page-table copy does not see kernel mapping edits made while the process runs.

pub mod elf;
pub mod sys;

use alloc::collections::BTreeSet;
use alloc::string::String;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

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

    /// Free every frame and table. Call only when no core can be running on `pml4`.
    pub fn release(&mut self) {
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

pub enum Fd {
    Console,
    File { data: Vec<u8>, pos: u64, dir: bool },
}

pub struct LinuxProc {
    pub asp: AddrSpace,
    pub brk: u64,
    pub brk_mapped: u64,
    pub mmap_next: u64,
    pub fds: Vec<Option<Fd>>,
    pub fs_base: u64,
    pub nsys: u64,
    pub enosys: Vec<u32>,
    pub cwd: String,
}

static LP: spin::Mutex<Option<LinuxProc>> = spin::Mutex::new(None);
static ACTIVE: AtomicBool = AtomicBool::new(false);
static DONE: AtomicBool = AtomicBool::new(false);
static VIA_GROUP: AtomicBool = AtomicBool::new(false);
static EXIT_CODE: AtomicI64 = AtomicI64::new(-1);
/// `vec + 1` of a ring-3 fault that killed the task, 0 = none.
static FAULT: AtomicU64 = AtomicU64::new(0);
static FAULT_CR2: AtomicU64 = AtomicU64::new(0);

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

pub fn is_linux_task() -> bool {
    ACTIVE.load(Ordering::Acquire) && crate::arch::sched::current_name() == Some(TASK_NAME)
}

/// Called from `record_ring3_kill` when the Linux task faults.
pub fn note_fault(vec: u8, _err: u64, cr2: u64) {
    FAULT_CR2.store(cr2, Ordering::Release);
    FAULT.store(vec as u64 + 1, Ordering::Release);
}

/// The Linux syscall entry (called from `syscall_dispatch` in the task's context, IF possibly open).
pub fn dispatch(nr: u64, a0: u64, a1: u64, a2: u64, a3: u64, a4: u64, a5: u64) -> i64 {
    let args = [a0, a1, a2, a3, a4, a5];
    if nr == 60 || nr == 231 {
        {
            let mut g = LP.lock();
            if let Some(p) = g.as_mut() {
                p.nsys += 1;
            }
        }
        VIA_GROUP.store(nr == 231, Ordering::Release);
        EXIT_CODE.store((a0 & 0xff) as i64, Ordering::Release);
        DONE.store(true, Ordering::Release);
        crate::arch::sched::exit(); // never returns; restores the kernel CR3 and retires the task
    }
    let rc = {
        let mut g = LP.lock();
        match g.as_mut() {
            Some(p) => {
                p.nsys += 1;
                if p.fs_base != 0 {
                    // TLS survives preemption/migration only if re-asserted; the task is also pinned.
                    if let Ok(v) = x86_64::VirtAddr::try_new(p.fs_base) {
                        x86_64::registers::model_specific::FsBase::write(v);
                    }
                }
                sys::handle(p, nr, args)
            }
            None => -38,
        }
    };
    crate::arch::sched::kill_check_current();
    rc
}

// ---------------------------------------------------------------------------------------------
// Run one program
// ---------------------------------------------------------------------------------------------

pub struct Report {
    pub pass: bool,
    pub exit: String,
    pub nsys: u64,
    pub enosys: Vec<u32>,
    pub ms: u64,
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
        alloc::format!(
            ":: LINUXABI: path={} exit={} syscalls={} enosys=[{}] ms={} -> {} ::",
            path, self.exit, self.nsys, e, self.ms, if self.pass { "PASS" } else { "FAIL" }
        )
    }
}

fn read_image(path: &str) -> Result<(String, Vec<u8>), String> {
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

/// Load `path` as a static Linux ELF, run it to completion (or `deadline_ms`), and report.
pub fn run_path(path: &str, argv: &[&str], deadline_ms: u64) -> Result<Report, String> {
    if ACTIVE.swap(true, Ordering::AcqRel) {
        return Err(String::from("another Linux program is running (one at a time)"));
    }
    let r = run_inner(path, argv, deadline_ms);
    ACTIVE.store(false, Ordering::Release);
    r
}

fn run_inner(path: &str, argv: &[&str], deadline_ms: u64) -> Result<Report, String> {
    let (full, bytes) = read_image(path)?;
    let plan = elf::parse(&bytes).map_err(String::from)?;
    let mut asp = AddrSpace::new();
    if let Err(e) = elf::load(&mut asp, &bytes, &plan) {
        asp.release();
        return Err(String::from(e));
    }
    let envp = ["PATH=/apps:/", "HOME=/", "TERM=linux"];
    let sp = match elf::build_stack(&mut asp, &plan, &full, argv, &envp) {
        Ok(s) => s,
        Err(e) => {
            asp.release();
            return Err(String::from(e));
        }
    };
    serial_println!(
        "[linuxabi] load path={} segs={} entry={:#x} stack={:#x} brk={:#x}",
        full, plan.segs.len(), plan.entry, sp, BRK_BASE
    );
    let pml4 = asp.pml4;
    let mut fds = Vec::new();
    fds.push(Some(Fd::Console));
    fds.push(Some(Fd::Console));
    fds.push(Some(Fd::Console));
    DONE.store(false, Ordering::Release);
    VIA_GROUP.store(false, Ordering::Release);
    EXIT_CODE.store(-1, Ordering::Release);
    FAULT.store(0, Ordering::Release);
    *LP.lock() = Some(LinuxProc {
        asp,
        brk: BRK_BASE,
        brk_mapped: BRK_BASE,
        mmap_next: MMAP_BASE,
        fds,
        fs_base: 0,
        nsys: 0,
        enosys: Vec::new(),
        cwd: String::from("/"),
    });
    let t0 = crate::arch::ms();
    let kill = alloc::sync::Arc::new(crate::arch::sched::KillSwitch::new());
    // Pinned to THIS core (not CPU_AUTO): FS_BASE is per-core state the scheduler does not switch, so the
    // task must not migrate. Preemptible, so a busy program still lets the desktop run.
    let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
    let _pid = crate::arch::sched::spawn_user_preemptible(TASK_NAME, plan.entry, sp, cpu, pml4, kill.clone());
    let deadline = crate::arch::ticks() + deadline_ms;
    while !DONE.load(Ordering::Acquire) && FAULT.load(Ordering::Acquire) == 0 && crate::arch::ticks() < deadline {
        crate::arch::sched::yield_now();
    }
    let done = DONE.load(Ordering::Acquire);
    let fault = FAULT.load(Ordering::Acquire);
    let mut exit = String::new();
    let mut safe_to_free = true;
    if done || fault != 0 {
        // Let the exiting task finish `sched::exit` (restore kernel CR3) before its tables are freed.
        let settle = crate::arch::ticks() + 50;
        while crate::arch::ticks() < settle {
            crate::arch::sched::yield_now();
        }
        if done {
            exit = alloc::format!("{}", EXIT_CODE.load(Ordering::Acquire));
        } else {
            exit = alloc::format!("FAULT(vec={} cr2={:#x})", fault - 1, FAULT_CR2.load(Ordering::Acquire));
        }
    } else {
        kill.request();
        let kd = crate::arch::ticks() + 500;
        while !kill.is_reaped() && crate::arch::ticks() < kd {
            crate::arch::sched::yield_now();
        }
        safe_to_free = kill.is_reaped();
        exit = String::from("TIMEOUT");
    }
    let ms = crate::arch::ms().saturating_sub(t0);
    let mut g = LP.lock();
    let (nsys, enosys) = match g.as_ref() {
        Some(p) => (p.nsys, p.enosys.clone()),
        None => (0, Vec::new()),
    };
    if let Some(mut p) = g.take() {
        if safe_to_free {
            p.asp.release();
        } // else: leak the space rather than free tables a still-live task may be running on
    }
    drop(g);
    Ok(Report { pass: done && VIA_GROUP.load(Ordering::Acquire), exit, nsys, enosys, ms })
}

// ---------------------------------------------------------------------------------------------
// Shell verb + `tests linuxabi`
// ---------------------------------------------------------------------------------------------

/// `linux <path> [args…]`
pub fn shell_verb(args: &[&str], console: &mut crate::console::Console) {
    let Some(&path) = args.first() else {
        console.println("usage: linux <path> [args...]   (run a static Linux x86_64 ELF)");
        return;
    };
    report(path, args, console);
}

fn report(path: &str, argv: &[&str], console: &mut crate::console::Console) {
    match run_path(path, argv, 20_000) {
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
    match run_path(P, &[P], 10_000) {
        Ok(r) => serial_println!("{}", r.witness(P)),
        Err(e) if e.contains("-ENOENT") => {
            serial_println!(":: LINUXABI: path={} exit=? syscalls=0 enosys=[] ms=0 -> SKIP (fixture not staged) ::", P)
        }
        Err(e) => serial_println!(":: LINUXABI: path={} exit=? syscalls=0 enosys=[] ms=0 -> FAIL ({}) ::", P, e),
    }
}
