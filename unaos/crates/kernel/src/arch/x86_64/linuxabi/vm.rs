// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Kernel — driver
//!
//! SELFBUILD3 (B353) — lazy memory for the Linux ABI shim: mapped RANGES (VMAs) instead of eagerly allocated pages.
//!
//! * `mmap` records a [`Vma`] and allocates nothing. A ring-3 touch of an unbacked page takes the #PF path
//!   (`interrupts.rs::page_fault_handler` → [`fault`]), which allocates ONE frame, fills it (zero for anonymous memory; the
//!   file's bytes at `foff` for a file mapping, read through the VFS — UnaFS or FAT, and so through the block cache under
//!   them) and installs the PTE with the VMA's permissions. A kernel-side `copy_in`/`copy_out` to an unbacked page populates
//!   it the same way ([`AddrSpace::fault_in`]), so a `read()` into a fresh anonymous buffer works.
//! * `MAP_SHARED` file pages are written back (dirty PTEs only, clamped to the file's size) on `msync`, `munmap`, `execve`
//!   and process exit (`AddrSpace::free_frames`). `MAP_PRIVATE` file pages are private copies, never written back.
//! * `PROT_NONE` is real: an unbacked page in a PROT_NONE VMA is refused at fault time, a resident page is kept as a
//!   NOT-present leaf carrying its frame and [`SWN`] (an OS-available bit) so its contents survive `mprotect` back.
//! * `brk` is a lazy anonymous VMA from `BRK_BASE` (up to `MMAP_BASE`, 1008 MiB); the main stack is 128 eager pages under
//!   `STACK_TOP` plus a lazy VMA down to [`STACK_LAZY_LO`] (8 MiB in all); `mmap` is first-fit over two windows — the
//!   original [`super::MMAP_BASE`]..[`super::MMAP_LIMIT`] (768 MiB) and [`MMAP2_BASE`]..[`MMAP2_LIMIT`] (448 GiB, above
//!   the stack and the signal trampoline) — so `munmap`'d address space is reused and nothing collides with the ELF
//!   window (which stays below 16 MiB in PML4[0], never handed to `mmap`).
//! * The per-process cap is a budget on RESIDENT pages: [`HEAP_SHARE`] (the old 96 MiB cap, served from the kernel heap as
//!   before) plus every frame of the machine's Usable RAM above 16 MiB outside the heap window that the identity map
//!   reaches (the user frame POOL, [`pool_alloc`]). A refusal prints `[linuxabi] resident limit: …` with both numbers.
//!
//! What this is NOT (owed): a shared page cache (a MAP_SHARED page and `read()` of the same file are coherent only after
//! `msync`/`munmap`); MAP_SHARED across `fork` (the child gets a copy, as LINUXABI2's eager fork always did); SIGBUS past
//! EOF (pages wholly beyond EOF read as zeroes); lazy `execve` images (the ELF is still read whole and mapped eagerly).

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;
use core::cell::Cell;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::{memory, AddrSpace, LinuxProc, ADDR, BRK_BASE, CANON_MAX, MMAP_BASE, MMAP_LIMIT, NX, P, PAGE, PS, STACK_PAGES, STACK_TOP, U, W};

/// Second mmap window: above the stack (`STACK_TOP`) and the signal trampoline page, inside PML4[2] (never inherited).
pub const MMAP2_BASE: u64 = 0x0000_0101_0000_0000;
pub const MMAP2_LIMIT: u64 = 0x0000_017F_0000_0000;
/// The main stack: 8 MiB, of which the top `STACK_PAGES` are mapped eagerly by the loader and the rest fault in.
pub const STACK_LAZY_LO: u64 = STACK_TOP - (8 << 20);
/// Heap-backed resident pages per process (the pre-SELFBUILD3 `MAX_PAGES`, 96 MiB); past it frames come from the pool.
pub const HEAP_SHARE: usize = 24576;
/// OS-available PTE bit 9: "resident but PROT_NONE" (the leaf is NOT present; its frame address is kept).
pub const SWN: u64 = 1 << 9;
const DIRTY: u64 = 1 << 6;

const ENOMEM: i64 = 12;
const EBADF: i64 = 9;
const EACCES: i64 = 13;
const EFAULT: i64 = 14;
const EEXIST: i64 = 17;
const EINVAL: i64 = 22;
const ENODEV: i64 = 19;

// ---- counters (the `tests selfbuild3` kernel line) ----
pub static PEAK_RESIDENT: AtomicU64 = AtomicU64::new(0);
pub static FAULTS_ANON: AtomicU64 = AtomicU64::new(0);
pub static FAULTS_FILE: AtomicU64 = AtomicU64::new(0);
pub static WRITEBACK_PAGES: AtomicU64 = AtomicU64::new(0);
pub static POOL_PAGES_USED: AtomicU64 = AtomicU64::new(0);
/// High-water mark of [`POOL_PAGES_USED`] (the `tests selfbuild3` big group drives a process past the heap share).
pub static POOL_PEAK: AtomicU64 = AtomicU64::new(0);
pub static REFUSALS: AtomicU64 = AtomicU64::new(0);

/// One mapped range `[start, end)` (page-aligned). `file` = the VFS path of a file mapping; `foff` = the file offset of `start`.
#[derive(Clone)]
pub struct Vma {
    pub start: u64,
    pub end: u64,
    pub r: bool,
    pub w: bool,
    pub x: bool,
    pub shared: bool,
    pub file: Option<Arc<String>>,
    pub foff: u64,
}

impl Vma {
    pub fn anon(start: u64, end: u64, w: bool) -> Vma {
        Vma { start, end, r: true, w, x: false, shared: false, file: None, foff: 0 }
    }
    fn none(&self) -> bool {
        !self.r && !self.w && !self.x
    }
}

/// The VMA list of one address space, sorted by `start`, never overlapping.
#[derive(Clone, Default)]
pub struct Vm {
    pub vmas: Vec<Vma>,
    /// The resident-limit refusal line is printed once per address space.
    refused: Cell<bool>,
}

impl Vm {
    pub fn find(&self, va: u64) -> Option<&Vma> {
        let i = self.vmas.partition_point(|v| v.end <= va);
        self.vmas.get(i).filter(|v| v.start <= va)
    }

    /// Cut the VMA that strictly contains `a` into `[start, a)` and `[a, end)`.
    fn split(&mut self, a: u64) {
        let i = self.vmas.partition_point(|v| v.end <= a);
        if let Some(v) = self.vmas.get(i) {
            if v.start < a && a < v.end {
                let mut right = v.clone();
                right.foff += a - v.start;
                right.start = a;
                self.vmas[i].end = a;
                self.vmas.insert(i + 1, right);
            }
        }
    }

    pub fn remove(&mut self, lo: u64, hi: u64) {
        self.split(lo);
        self.split(hi);
        self.vmas.retain(|v| v.end <= lo || v.start >= hi);
    }

    pub fn insert(&mut self, v: Vma) {
        let i = self.vmas.partition_point(|x| x.start < v.start);
        self.vmas.insert(i, v);
    }

    pub fn overlaps(&self, lo: u64, hi: u64) -> bool {
        self.vmas.iter().any(|v| v.start < hi && v.end > lo)
    }

    /// First fit of `len` bytes in `[lo, hi)`.
    pub(super) fn gap(&self, len: u64, lo: u64, hi: u64) -> Option<u64> {
        let mut cur = lo;
        for v in self.vmas.iter().filter(|v| v.end > lo && v.start < hi) {
            if v.start >= cur && v.start - cur >= len {
                return Some(cur);
            }
            cur = cur.max(v.end);
        }
        if cur.checked_add(len).is_some_and(|e| e <= hi) { Some(cur) } else { None }
    }

    /// The sub-ranges of `[lo, hi)` no VMA covers.
    fn holes(&self, lo: u64, hi: u64) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        let mut cur = lo;
        for v in self.vmas.iter().filter(|v| v.end > lo && v.start < hi) {
            if v.start > cur {
                out.push((cur, v.start));
            }
            cur = cur.max(v.end);
        }
        if cur < hi {
            out.push((cur, hi));
        }
        out
    }

    fn protect(&mut self, lo: u64, hi: u64, r: bool, w: bool, x: bool) {
        self.split(lo);
        self.split(hi);
        for v in self.vmas.iter_mut().filter(|v| v.start >= lo && v.end <= hi) {
            v.r = r;
            v.w = w;
            v.x = x;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The user frame pool: Usable RAM above 16 MiB, outside the kernel heap, reachable through the identity map.
// ---------------------------------------------------------------------------------------------

struct Pool {
    regs: [(u64, u64); 32],
    n: usize,
    cur: usize,
    bump: u64,
    /// Head of the intrusive free list (the next pointer lives in the free frame itself), 0 = empty.
    free: u64,
    total: u64,
    ready: bool,
}

static POOL: spin::Mutex<Pool> = spin::Mutex::new(Pool { regs: [(0, 0); 32], n: 0, cur: 0, bump: 0, free: 0, total: 0, ready: false });
static POOL_LOGGED: AtomicBool = AtomicBool::new(false);
const MIB2: u64 = 2 << 20;
/// Physical addresses at/above PML4[2] (1 TiB) alias the process's private window in its own CR3: never pooled.
const POOL_CEIL: u64 = 0x0000_0100_0000_0000;

fn pool_init(p: &mut Pool) {
    p.ready = true;
    let (hs, hl) = memory::selfbuild3_heap_window();
    let mut cut: Vec<(u64, u64)> = Vec::new();
    for (s, e) in memory::selfbuild3_usable_ram() {
        let s = ((s.max(16 << 20)) + PAGE - 1) & !(PAGE - 1);
        let e = e.min(POOL_CEIL) & !(PAGE - 1);
        if s >= e {
            continue;
        }
        // Subtract the heap window.
        let (h0, h1) = (hs, hs + hl);
        let parts = if hl == 0 || h1 <= s || h0 >= e {
            [(s, e), (0, 0)]
        } else {
            [(s, h0.max(s)), (h1.min(e), e)]
        };
        for (a, b) in parts {
            if a >= b {
                continue;
            }
            // Keep the prefix the identity map really reaches (VA == PA), probed every 2 MiB and at the last page.
            let mut end = a;
            let mut probe = a;
            while probe < b {
                if memory::translate(probe) != Some(probe) {
                    break;
                }
                end = ((probe & !(MIB2 - 1)) + MIB2).min(b);
                probe = end;
            }
            if end > a && memory::translate(end - PAGE) != Some(end - PAGE) {
                end = a;
            }
            if end > a {
                cut.push((a, end));
            }
        }
    }
    for (a, b) in cut.into_iter().take(32) {
        p.regs[p.n] = (a, b);
        p.n += 1;
        p.total += (b - a) / PAGE;
    }
    p.bump = if p.n > 0 { p.regs[0].0 } else { 0 };
}

fn pool_contains(p: &Pool, f: u64) -> bool {
    p.regs[..p.n].iter().any(|&(a, b)| f >= a && f < b)
}

/// Pages the pool can ever hand out (initialises it on first use).
pub fn pool_total() -> u64 {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut p = POOL.lock();
        if !p.ready {
            pool_init(&mut p);
        }
        p.total
    })
}

/// One zeroed frame from the pool, or `None` when it is exhausted (or the machine has no RAM past the heap).
pub fn pool_alloc() -> Option<u64> {
    let f = x86_64::instructions::interrupts::without_interrupts(|| {
        let mut p = POOL.lock();
        if !p.ready {
            pool_init(&mut p);
        }
        if p.free != 0 {
            let f = p.free;
            p.free = unsafe { core::ptr::read_volatile(f as *const u64) };
            return Some(f);
        }
        while p.cur < p.n {
            let (a, b) = p.regs[p.cur];
            if p.bump < a {
                p.bump = a;
            }
            if p.bump + PAGE <= b {
                let f = p.bump;
                p.bump += PAGE;
                return Some(f);
            }
            p.cur += 1;
        }
        None
    })?;
    unsafe { core::ptr::write_bytes(f as *mut u8, 0, PAGE as usize) };
    let used = POOL_PAGES_USED.fetch_add(1, Ordering::Relaxed) + 1;
    POOL_PEAK.fetch_max(used, Ordering::Relaxed);
    if !POOL_LOGGED.swap(true, Ordering::AcqRel) {
        serial_println!("[linuxabi] frame pool: first frame {:#x} (pool {} pages = {} MiB past the heap)", f, pool_total(), pool_total() / 256);
    }
    Some(f)
}

/// A zeroed data frame: from the kernel heap while `heap_ok` (the first [`HEAP_SHARE`] pages of a process), else the pool.
pub fn alloc_frame(heap_ok: bool) -> Option<u64> {
    if heap_ok {
        if let Ok(l) = core::alloc::Layout::from_size_align(4096, 4096) {
            let p = unsafe { alloc::alloc::alloc_zeroed(l) };
            if !p.is_null() {
                return Some(p as u64);
            }
        }
    }
    pool_alloc()
}

/// Give a data frame back to whichever allocator it came from.
pub fn free_frame(f: u64) {
    if super::cow::unshare(f) {
        return; // SELFBUILD4: another address space still holds this frame (copy-on-write fork)
    }
    let pooled = x86_64::instructions::interrupts::without_interrupts(|| {
        let mut p = POOL.lock();
        if p.ready && pool_contains(&p, f) {
            unsafe { core::ptr::write_volatile(f as *mut u64, p.free) };
            p.free = f;
            true
        } else {
            false
        }
    });
    if pooled {
        POOL_PAGES_USED.fetch_sub(1, Ordering::Relaxed);
    } else if let Ok(l) = core::alloc::Layout::from_size_align(4096, 4096) {
        unsafe { alloc::alloc::dealloc(f as *mut u8, l) };
    }
}

/// The per-process resident budget in pages: the heap share plus the whole pool.
pub fn limit_pages() -> usize {
    HEAP_SHARE + pool_total() as usize
}

// ---------------------------------------------------------------------------------------------
// Address-space side (lazy population, leaf walks, write-back)
// ---------------------------------------------------------------------------------------------

/// Why a page could not be populated.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refuse {
    NoVma,
    Prot,
    Budget,
    /// SELFBUILD4: a file page wholly past EOF (SIGBUS).
    Bus,
}

impl AddrSpace {
    /// The raw leaf for `va` (present OR not), provided every upper level is a present non-huge entry.
    pub(super) fn raw_leaf(&self, va: u64) -> Option<u64> {
        if va >= CANON_MAX {
            return None;
        }
        let mut tbl = self.pml4 as *const u64;
        for shift in [39u32, 30, 21] {
            let e = unsafe { *tbl.add(((va >> shift) & 511) as usize) };
            if e & P == 0 || (e & PS != 0 && shift != 39) {
                return None;
            }
            tbl = (e & ADDR) as *const u64;
        }
        Some(unsafe { *tbl.add(((va >> 12) & 511) as usize) })
    }

    /// A resident PROT_NONE page (`SWN` leaf)?
    pub(super) fn raw_none(&self, va: u64) -> bool {
        self.raw_leaf(va & !(PAGE - 1)).is_some_and(|e| e & P == 0 && e & SWN != 0 && e & U != 0)
    }

    /// Leaf bits for a VMA protection (`SWN` + not-present for PROT_NONE).
    pub(super) fn prot_bits(r: bool, w: bool, x: bool) -> u64 {
        if !r && !w && !x {
            U | NX | SWN
        } else {
            P | U | if w { W } else { 0 } | if x { 0 } else { NX }
        }
    }

    /// Budget check + one frame for this space. Prints the limit (once per space) on refusal.
    pub(super) fn take_frame(&self, va: u64) -> Result<u64, Refuse> {
        let have = self.m.borrow().frames.len();
        let limit = limit_pages();
        let f = if have >= limit { None } else { alloc_frame(have < HEAP_SHARE) };
        match f {
            Some(f) => Ok(f),
            None => {
                REFUSALS.fetch_add(1, Ordering::Relaxed);
                if !self.vm.refused.replace(true) {
                    serial_println!(
                        "[linuxabi] resident limit: resident={} pages limit={} pages ({} MiB = heap share {} + pool {}; pool in use {}) va={:#x} -> refused",
                        have, limit, limit / 256, HEAP_SHARE, limit - HEAP_SHARE, POOL_PAGES_USED.load(Ordering::Relaxed), va
                    );
                }
                Err(Refuse::Budget)
            }
        }
    }

    /// Install frame `f` (already owned and filled) at `va` with leaf `bits`.
    pub(super) fn install(&self, va: u64, f: u64, bits: u64) {
        let n = {
            let mut m = self.m.borrow_mut();
            m.frames.insert(f);
            m.frames.len() as u64
        };
        let slot = self.leaf_slot(va);
        unsafe { *slot = (f & ADDR) | bits };
        PEAK_RESIDENT.fetch_max(n, Ordering::Relaxed);
    }

    /// Back one page of a VMA. `Ok((frame, pte))`; a page already resident answers as it is.
    pub fn populate(&self, va: u64, write: bool, fetch: bool) -> Result<(u64, u64), Refuse> {
        let va = va & !(PAGE - 1);
        let Some(v) = self.vm.find(va) else { return Err(Refuse::NoVma) };
        if v.none() || (write && !v.w) || (fetch && !v.x) {
            return Err(Refuse::Prot);
        }
        if let Some(pe) = self.user_page(va) {
            return Ok(pe);
        }
        if self.raw_none(va) {
            return Err(Refuse::Prot);
        }
        let bits = Self::prot_bits(v.r, v.w, v.x);
        let file = v.file.clone().map(|p| (p, v.foff + (va - v.start)));
        if let Some((path, off)) = &file {
            // SELFBUILD4: a page wholly past EOF is SIGBUS (Linux), not zeroes; the page holding EOF reads zero past it.
            if crate::shell::vfs_mount_table().stat(path).is_ok_and(|s| *off >= s.size) {
                return Err(Refuse::Bus);
            }
        }
        let f = self.take_frame(va)?;
        if let Some((path, off)) = file {
            // Fill BEFORE the PTE goes live: no other thread of the process can see a half-read page.
            if let Ok(bytes) = crate::shell::vfs_mount_table().read(&path, off, PAGE as usize) {
                let n = bytes.len().min(PAGE as usize);
                unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), f as *mut u8, n) };
            }
            FAULTS_FILE.fetch_add(1, Ordering::Relaxed);
        } else {
            FAULTS_ANON.fetch_add(1, Ordering::Relaxed);
        }
        self.install(va, f, bits);
        Ok((f, (f & ADDR) | bits))
    }

    /// `copy_in`/`copy_out`'s hook: back an unbacked page the kernel is about to touch for ring 3.
    pub(super) fn fault_in(&self, va: u64, write: bool) -> Option<(u64, u64)> {
        if self.vm.vmas.is_empty() {
            return None;
        }
        self.populate(va, write, false).ok()
    }

    /// Set the hardware dirty bit after the kernel stored into a user page (MAP_SHARED write-back sees it).
    pub(super) fn mark_dirty(&self, va: u64) {
        if self.vm.vmas.iter().any(|v| v.shared && v.file.is_some()) {
            let slot = self.leaf_slot(va & !(PAGE - 1));
            unsafe { *slot |= DIRTY };
        }
    }

    fn walk(&self, tbl: u64, shift: u32, base: u64, lo: u64, hi: u64, out: &mut Vec<(u64, u64)>) {
        let span = 1u64 << shift;
        let first = if lo > base { ((lo - base) >> shift) as usize } else { 0 };
        let last = (((hi - base) + span - 1) >> shift).min(512) as usize;
        for i in first..last {
            let va = base + i as u64 * span;
            let e = unsafe { *(tbl as *const u64).add(i) };
            if shift == 12 {
                if e & U != 0 && e & (P | SWN) != 0 && va >= lo && va < hi {
                    out.push((va, e));
                }
                continue;
            }
            if e & P == 0 || e & PS != 0 {
                continue;
            }
            let t = e & ADDR;
            if !self.m.borrow().tables.contains(&t) {
                continue; // a kernel table: never ours to report
            }
            self.walk(t, shift - 9, va, lo, hi, out);
        }
    }

    /// Every USER leaf (present or PROT_NONE-resident) in `[lo, hi)`, in address order, skipping absent tables wholesale.
    pub fn leaf_range(&self, lo: u64, hi: u64) -> Vec<(u64, u64)> {
        let mut out = Vec::new();
        let hi = hi.min(CANON_MAX);
        if lo < hi {
            self.walk(self.pml4, 39, 0, lo, hi, &mut out);
        }
        out
    }

    /// Drop one page (present or PROT_NONE-resident) and free its frame. `false` = nothing there.
    pub(super) fn drop_page(&self, va: u64) -> bool {
        let va = va & !(PAGE - 1);
        let Some(e) = self.raw_leaf(va) else { return false };
        if e & U == 0 || e & (P | SWN) == 0 {
            return false;
        }
        let slot = self.leaf_slot(va);
        unsafe { *slot = 0 };
        let f = e & ADDR;
        if self.m.borrow_mut().frames.remove(&f) {
            free_frame(f);
        }
        true
    }

    /// Rewrite a resident leaf's protection (keeps its frame and dirty bit).
    fn reprotect(&self, va: u64, e: u64, r: bool, w: bool, x: bool) {
        let slot = self.leaf_slot(va);
        let mut b = Self::prot_bits(r, w, x);
        if e & super::cow::COW != 0 {
            b = super::cow::cowify(b); // SELFBUILD4: a shared page stays copy-on-write (W goes to CW)
        }
        unsafe { *slot = (e & ADDR) | (e & DIRTY) | b };
    }

    /// Write MAP_SHARED dirty pages in `[lo, hi)` back to their files. `clear` = clear the dirty bits (msync).
    pub fn writeback(&self, lo: u64, hi: u64, clear: bool) -> u64 {
        use crate::fs::vfs::KERNEL_PRINCIPAL;
        let mut n = 0u64;
        let shared: Vec<Vma> = self.vm.vmas.iter().filter(|v| v.shared && v.file.is_some() && v.end > lo && v.start < hi).cloned().collect();
        if shared.is_empty() {
            return 0;
        }
        let mt = crate::shell::vfs_mount_table();
        for v in shared {
            let Some(path) = v.file.as_ref() else { continue };
            let Ok(st) = mt.stat(path) else { continue };
            for (va, e) in self.leaf_range(lo.max(v.start), hi.min(v.end)) {
                if e & DIRTY == 0 {
                    continue;
                }
                let off = v.foff + (va - v.start);
                if off < st.size {
                    let len = (st.size - off).min(PAGE) as usize;
                    let bytes = unsafe { core::slice::from_raw_parts((e & ADDR) as *const u8, len) };
                    let mut done = 0usize;
                    while done < len {
                        match mt.write(path, off + done as u64, &bytes[done..], KERNEL_PRINCIPAL) {
                            Ok(w) if w > 0 => done += w,
                            _ => break,
                        }
                    }
                    n += 1;
                }
                if clear {
                    let slot = self.leaf_slot(va);
                    unsafe { *slot &= !DIRTY };
                }
            }
        }
        WRITEBACK_PAGES.fetch_add(n, Ordering::Relaxed);
        n
    }

    /// Exit / execve / free: every MAP_SHARED page goes home.
    pub(super) fn writeback_all(&self) {
        if self.vm.vmas.iter().any(|v| v.shared && v.file.is_some()) {
            self.writeback(0, CANON_MAX, false);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The #PF hook (interrupt context, IF=0, GS already swapped to the kernel's)
// ---------------------------------------------------------------------------------------------

/// A ring-3 page fault of a Linux task: `true` = resolved (or retry: another thread of the process holds its lock),
/// return to ring 3 and re-execute; `false` = a real fault, kill as before.
pub fn fault(cr2: u64, err: u64) -> bool {
    if !super::is_linux_task() {
        return false;
    }
    let Some(info) = super::proc::cur_info() else { return false };
    let Some(p) = info.lp.try_lock() else { return true };
    let va = cr2 & !(PAGE - 1);
    let (write, fetch) = (err & 2 != 0, err & 16 != 0);
    let mut low = false;
    if err & 1 != 0 {
        // Protection fault on a present page: legal only if the PTE already allows it (a stale TLB entry).
        if let Some((_, e)) = p.asp.user_page(va) {
            if write && e & super::cow::COW != 0 && e & super::cow::CW != 0 {
                return p.asp.cow_break(va).is_some(); // SELFBUILD4: copy-on-write
            }
            if (!write || e & W != 0) && (!fetch || e & NX == 0) {
                flush();
                return true;
            }
            return false;
        }
        // SELFBUILD4: present but not USER = the kernel's identity map under a lazy ELF page (PML4[0]): back it below.
        if p.asp.vm.find(va).is_none() {
            return false;
        }
        low = true;
    }
    match p.asp.populate(va, write, fetch) {
        Ok(_) => {
            if low {
                flush(); // the supervisor translation (and the split huge entry) may be cached
            }
            true
        }
        Err(r) => {
            if r == Refuse::Bus {
                FAULT_SIG.store(((info.pid as u64) << 8) | 7, Ordering::Release);
                serial_println!("[linuxabi] fault va={:#x} err={:#x} -> Bus (SIGBUS: file page past EOF)", cr2, err);
            } else if r != Refuse::Budget {
                serial_println!("[linuxabi] fault va={:#x} err={:#x} -> {:?} (SIGSEGV)", cr2, err, r);
            }
            false
        }
    }
}

/// SELFBUILD4: the signal a ring-3 fault ends the process with — `pid << 8 | sig`, set by [`fault`] for SIGBUS.
pub static FAULT_SIG: AtomicU64 = AtomicU64::new(0);

/// The wait status for process `pid`'s fatal fault: 7 (SIGBUS) when [`fault`] said so, else 11 (SIGSEGV).
pub fn take_fault_sig(pid: u32) -> i64 {
    let v = FAULT_SIG.load(Ordering::Acquire);
    if v != 0 && v >> 8 == pid as u64 {
        FAULT_SIG.store(0, Ordering::Release);
        return (v & 0xff) as i64;
    }
    11
}

pub(super) fn flush() {
    unsafe { memory::load_cr3(memory::current_cr3()) };
}

pub(super) fn page_up(v: u64) -> Option<u64> {
    v.checked_add(PAGE - 1).map(|x| x & !(PAGE - 1))
}

/// May a `MAP_FIXED` range live at `[lo, hi)`? (the two mmap windows; never the brk, the stack, the trampoline or PML4[0])
pub(super) fn fixed_ok(lo: u64, hi: u64) -> bool {
    // SELFBUILD4: the brk range too — musl's mallocng puts a PROT_NONE guard page at the brk start with MAP_FIXED.
    (lo >= MMAP_BASE && hi <= STACK_LAZY_LO) || (lo >= MMAP2_BASE && hi <= MMAP2_LIMIT) || (lo >= BRK_BASE && hi <= MMAP_BASE)
}

// ---------------------------------------------------------------------------------------------
// Syscalls (routed from sys.rs)
// ---------------------------------------------------------------------------------------------

/// `mmap(addr, len, prot, flags, fd, off)` (9).
pub fn mmap(p: &mut LinuxProc, addr: u64, len: u64, prot: u64, flags: u64, fd: u64, off: u64) -> i64 {
    use super::fd::Kind;
    if len == 0 {
        return -EINVAL;
    }
    let Some(len) = page_up(len) else { return -ENOMEM };
    let (r, w, x) = (prot & 1 != 0, prot & 2 != 0, prot & 4 != 0);
    if w && x {
        return -EACCES; // W^X
    }
    let anon = flags & 0x20 != 0;
    let shared = matches!(flags & 3, 1 | 3);
    if !anon && off & (PAGE - 1) != 0 {
        return -EINVAL;
    }
    let file = if anon {
        None
    } else {
        let Some(d) = super::sys::get(p, fd) else { return -EBADF };
        let k = super::fd::lk(&d.k);
        match &*k {
            Kind::File { path, read, write, .. } => {
                if !*read || (shared && w && !*write) {
                    return -EACCES;
                }
                Some(Arc::new(path.clone()))
            }
            Kind::Dir { .. } => return -ENODEV,
            _ => return -EACCES,
        }
    };
    let base = if flags & 0x10 != 0 || flags & 0x10_0000 != 0 {
        // MAP_FIXED / MAP_FIXED_NOREPLACE
        let Some(end) = addr.checked_add(len) else { return -ENOMEM };
        if addr & (PAGE - 1) != 0 || !fixed_ok(addr, end) {
            return -ENOMEM;
        }
        if flags & 0x10 == 0 && p.asp.vm.overlaps(addr, end) {
            return -EEXIST;
        }
        unmap_range(p, addr, end);
        addr
    } else {
        match p.asp.vm.gap(len, MMAP_BASE, MMAP_LIMIT).or_else(|| p.asp.vm.gap(len, MMAP2_BASE, MMAP2_LIMIT)) {
            Some(b) => b,
            None => return -ENOMEM,
        }
    };
    let r = r || w || x; // x86: write and execute imply read
    p.asp.vm.insert(Vma { start: base, end: base + len, r, w, x, shared, file, foff: off });
    if flags & 0x8000 != 0 {
        // MAP_POPULATE: back it now (best effort, within the budget).
        let mut a = base;
        while a < base + len && p.asp.populate(a, false, false).is_ok() {
            a += PAGE;
        }
    }
    base as i64
}

/// Write back, unmap and forget `[lo, hi)`.
pub(super) fn unmap_range(p: &mut LinuxProc, lo: u64, hi: u64) {
    p.asp.writeback(lo, hi, false);
    for (va, _) in p.asp.leaf_range(lo, hi) {
        p.asp.drop_page(va);
    }
    p.asp.vm.remove(lo, hi);
    flush();
}

/// `munmap(addr, len)` (11).
pub fn munmap(p: &mut LinuxProc, addr: u64, len: u64) -> i64 {
    if addr & (PAGE - 1) != 0 || len == 0 {
        return -EINVAL;
    }
    let Some(len) = page_up(len) else { return -EINVAL };
    let Some(end) = addr.checked_add(len) else { return -EINVAL };
    unmap_range(p, addr, end.min(CANON_MAX));
    0
}

/// Is every page of `[lo, hi)` either inside a VMA or resident (the ELF image, the eager stack)?
fn mapped_all(p: &LinuxProc, lo: u64, hi: u64) -> bool {
    p.asp.vm.holes(lo, hi).into_iter().all(|(a, b)| p.asp.leaf_range(a, b).len() as u64 == (b - a) / PAGE)
}

/// `mprotect(addr, len, prot)` (10).
pub fn mprotect(p: &mut LinuxProc, addr: u64, len: u64, prot: u64) -> i64 {
    if addr & (PAGE - 1) != 0 {
        return -EINVAL;
    }
    let Some(len) = page_up(len) else { return -ENOMEM };
    let Some(end) = addr.checked_add(len) else { return -ENOMEM };
    let (r, w, x) = (prot & 1 != 0, prot & 2 != 0, prot & 4 != 0);
    if w && x {
        return -EACCES;
    }
    if end > CANON_MAX || !mapped_all(p, addr, end) {
        return -ENOMEM;
    }
    let r = r || w || x;
    p.asp.vm.protect(addr, end, r, w, x);
    for (va, e) in p.asp.leaf_range(addr, end) {
        p.asp.reprotect(va, e, r, w, x);
    }
    flush();
    0
}

/// `madvise(addr, len, advice)` (28): DONTNEED / FREE drop the resident pages of VMAs (they fault back zeroed or re-read);
/// every other advice is accepted as a hint.
pub fn madvise(p: &mut LinuxProc, addr: u64, len: u64, advice: u64) -> i64 {
    if addr & (PAGE - 1) != 0 {
        return -EINVAL;
    }
    let Some(len) = page_up(len) else { return -EINVAL };
    let Some(end) = addr.checked_add(len) else { return -EINVAL };
    if advice == 4 || advice == 8 {
        p.asp.writeback(addr, end, true);
        let vmas: Vec<(u64, u64)> = p.asp.vm.vmas.iter().filter(|v| v.end > addr && v.start < end).map(|v| (v.start.max(addr), v.end.min(end))).collect();
        for (a, b) in vmas {
            for (va, _) in p.asp.leaf_range(a, b) {
                p.asp.drop_page(va);
            }
        }
        flush();
    }
    0
}

/// `msync(addr, len, flags)` (26).
pub fn msync(p: &mut LinuxProc, addr: u64, len: u64, _flags: u64) -> i64 {
    if addr & (PAGE - 1) != 0 {
        return -EINVAL;
    }
    let Some(len) = page_up(len) else { return -ENOMEM };
    let Some(end) = addr.checked_add(len) else { return -ENOMEM };
    if !p.asp.vm.holes(addr, end).is_empty() {
        return -ENOMEM;
    }
    p.asp.writeback(addr, end, true);
    flush();
    0
}

/// `mincore(addr, len, vec)` (27): one byte per page, bit 0 = resident.
pub fn mincore(p: &mut LinuxProc, addr: u64, len: u64, vec: u64) -> i64 {
    if addr & (PAGE - 1) != 0 {
        return -EINVAL;
    }
    let Some(len) = page_up(len) else { return -ENOMEM };
    let Some(end) = addr.checked_add(len) else { return -ENOMEM };
    let pages = (len / PAGE) as usize;
    if pages > (1 << 22) {
        return -ENOMEM; // 16 GiB per call is plenty
    }
    if end > CANON_MAX || !mapped_all(p, addr, end) {
        return -ENOMEM;
    }
    let mut v = alloc::vec![0u8; pages];
    for (va, _) in p.asp.leaf_range(addr, end) {
        v[((va - addr) / PAGE) as usize] = 1;
    }
    if p.asp.copy_out(vec, &v, false) { 0 } else { -EFAULT }
}

/// `brk(want)` (12): a lazy anonymous VMA `[BRK_BASE, page_up(brk))`.
pub fn brk(p: &mut LinuxProc, want: u64) -> i64 {
    if want < BRK_BASE || want > MMAP_BASE {
        return p.brk as i64;
    }
    let Some(end) = page_up(want) else { return p.brk as i64 };
    let cur_end = p.brk_mapped;
    if end > cur_end && p.asp.vm.overlaps(cur_end, end) {
        return p.brk as i64; // SELFBUILD4: a MAP_FIXED mapping sits where the heap would grow (Linux refuses too)
    }
    if end > cur_end {
        p.asp.vm.remove(BRK_BASE, cur_end);
        p.asp.vm.insert(Vma::anon(BRK_BASE, end, true));
    } else if end < cur_end {
        for (va, _) in p.asp.leaf_range(end, cur_end) {
            p.asp.drop_page(va);
        }
        p.asp.vm.remove(end, cur_end);
        flush();
    }
    p.brk_mapped = end;
    if want < p.brk && want < end {
        // Linux hands back zeroes when the heap regrows: scrub the partial page that stays.
        if let Some((f, _)) = p.asp.user_page(want) {
            let off = (want & (PAGE - 1)) as usize;
            unsafe { core::ptr::write_bytes((f as *mut u8).add(off), 0, PAGE as usize - off) };
        }
    }
    p.brk = want;
    want as i64
}

/// The loader's lazy part of the main stack (below the eager `STACK_PAGES`).
pub fn add_stack_vma(asp: &mut AddrSpace) {
    let eager_lo = STACK_TOP - STACK_PAGES * PAGE;
    if !asp.vm.overlaps(STACK_LAZY_LO, eager_lo) {
        asp.vm.insert(Vma::anon(STACK_LAZY_LO, eager_lo, true));
    }
}

/// `sysinfo` totals: (total bytes, free bytes) for a process with `resident` pages.
pub fn sysinfo_bytes(resident: usize) -> (u64, u64) {
    let total = limit_pages() as u64 * PAGE;
    (total, total.saturating_sub(resident as u64 * PAGE))
}
