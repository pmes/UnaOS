// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Kernel — driver
//!
//! SELFBUILD5 (B357, ROADMAP §1c SH-5) — the pieces of the Linux ABI shim a linker and a growing Rust program hit next:
//! `mremap`, the alternate signal stack honoured at delivery, `getcpu`.
//!
//! * `mremap(old, old_len, new_len, flags, new)` (25) over SELFBUILD3's VMAs. A shrink unmaps the tail. A grow stays IN PLACE
//!   when the range ends its VMA and the pages after it are free (the VMA's end moves; nothing is touched). Otherwise, with
//!   `MREMAP_MAYMOVE`, the range MOVES: a new VMA (same protection, same file and offset) is placed first-fit (or at `new` with
//!   `MREMAP_FIXED`, whatever was there unmapped) and every resident leaf is re-homed by rewriting PTEs — the frame, the
//!   copy-on-write bits and the dirty bit go with it, no byte is copied. `MREMAP_DONTUNMAP` (equal lengths) moves the pages
//!   and keeps the old VMA, which then faults zero (anonymous) or re-reads its file. musl's `realloc` past its mmap threshold
//!   (~128 KiB) is the caller (SELFBUILD4 M4 item 1); lld's output buffer is the other.
//! * `sigaltstack` (131) is per THREAD (keyed by the thread key, `thread::key_for`): install / disable / query with
//!   `SS_ONSTACK` while running on it (`EPERM` to change it then), `SS_AUTODISARM`. [`sig_stack`] is what `signal::deliver`
//!   asks: a handler installed with `SA_ONSTACK` gets its frame at the top of the alternate stack unless the thread is
//!   already on it, and the frame's `uc_stack` records the stack as Linux does.
//! * `getcpu` (309): CPU 0, node 0 — every Linux process is pinned to the verb's core.
//!
//! Limits (stated): delivery is still at the syscall boundary only, so a stack OVERFLOW handler (a fault on the guard page)
//! is not yet run on the alternate stack — the fault kills the group as before; that path needs fault-time delivery
//! (SELFBUILD4 M4 item 6).

use super::proc::ProcInfo;
use super::vm::{self, Vma};
use super::{LinuxProc, BRK_BASE, CANON_MAX, MMAP_BASE, MMAP_LIMIT, PAGE};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

const EPERM: i64 = 1;
const ENOMEM: i64 = 12;
const EFAULT: i64 = 14;
const EINVAL: i64 = 22;

const MREMAP_MAYMOVE: u64 = 1;
const MREMAP_FIXED: u64 = 2;
const MREMAP_DONTUNMAP: u64 = 4;

const SS_ONSTACK: u32 = 1;
const SS_DISABLE: u32 = 2;
const SS_AUTODISARM: u32 = 1 << 31;
const MINSIGSTKSZ: u64 = 2048;
pub const SA_ONSTACK: u64 = 0x0800_0000;

// ---- counters (the `tests selfbuild5` kernel line) ----
pub static MREMAP_CALLS: AtomicU64 = AtomicU64::new(0);
pub static MREMAP_INPLACE: AtomicU64 = AtomicU64::new(0);
pub static MREMAP_MOVED: AtomicU64 = AtomicU64::new(0);
/// Resident pages re-homed by a move (PTE rewrites, not copies).
pub static MREMAP_PAGES: AtomicU64 = AtomicU64::new(0);
pub static ALTSTACK_DELIVERIES: AtomicU64 = AtomicU64::new(0);

pub fn reset() {
    for c in [&MREMAP_CALLS, &MREMAP_INPLACE, &MREMAP_MOVED, &MREMAP_PAGES, &ALTSTACK_DELIVERIES] {
        c.store(0, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------------------------
// mremap
// ---------------------------------------------------------------------------------------------

/// `mremap(old, old_len, new_len, flags, new_addr)` (25).
pub fn mremap(p: &mut LinuxProc, old: u64, old_len: u64, new_len: u64, flags: u64, new_addr: u64) -> i64 {
    MREMAP_CALLS.fetch_add(1, Ordering::Relaxed);
    if flags & !(MREMAP_MAYMOVE | MREMAP_FIXED | MREMAP_DONTUNMAP) != 0 || old & (PAGE - 1) != 0 {
        return -EINVAL;
    }
    if flags & (MREMAP_FIXED | MREMAP_DONTUNMAP) != 0 && flags & MREMAP_MAYMOVE == 0 {
        return -EINVAL;
    }
    let (Some(old_len), Some(new_len)) = (vm::page_up(old_len), vm::page_up(new_len)) else { return -EINVAL };
    if new_len == 0 || old_len == 0 {
        return -EINVAL; // old_len 0 = "duplicate a shared mapping": not answered
    }
    if flags & MREMAP_DONTUNMAP != 0 && old_len != new_len {
        return -EINVAL;
    }
    let Some(old_end) = old.checked_add(old_len).filter(|e| *e <= CANON_MAX) else { return -EINVAL };
    let Some(v) = p.asp.vm.find(old).cloned() else { return -EFAULT };
    if old_end > v.end {
        return -EFAULT; // Linux: the old range must lie in ONE mapping
    }
    if old < p.brk_mapped && old_end > BRK_BASE && v.start == BRK_BASE {
        return -EINVAL; // the brk heap itself is `brk`'s to size
    }
    let fixed = flags & MREMAP_FIXED != 0;
    let dontunmap = flags & MREMAP_DONTUNMAP != 0;
    if !fixed && !dontunmap {
        if new_len <= old_len {
            if new_len < old_len {
                vm::unmap_range(p, old + new_len, old_end);
            }
            return old as i64;
        }
        // Grow in place: the range ends its VMA and the next pages are free (and legal).
        if let Some(want_end) = old.checked_add(new_len) {
            if old_end == v.end && vm::fixed_ok(old_end, want_end) && !p.asp.vm.overlaps(old_end, want_end) {
                if let Some(i) = p.asp.vm.vmas.iter().position(|x| x.end == old_end && x.start <= old) {
                    p.asp.vm.vmas[i].end = want_end;
                    MREMAP_INPLACE.fetch_add(1, Ordering::Relaxed);
                    return old as i64;
                }
            }
        }
        if flags & MREMAP_MAYMOVE == 0 {
            return -ENOMEM;
        }
    }
    // MOVE.
    let mut old_len = old_len;
    let dest = if fixed {
        let Some(dest_end) = new_addr.checked_add(new_len) else { return -EINVAL };
        if new_addr & (PAGE - 1) != 0 || !vm::fixed_ok(new_addr, dest_end) {
            return -EINVAL;
        }
        if new_addr < old_end && dest_end > old {
            return -EINVAL; // Linux: the ranges may not overlap
        }
        vm::unmap_range(p, new_addr, dest_end);
        if new_len < old_len {
            vm::unmap_range(p, old + new_len, old_end);
            old_len = new_len;
        }
        new_addr
    } else {
        match p.asp.vm.gap(new_len, MMAP_BASE, MMAP_LIMIT).or_else(|| p.asp.vm.gap(new_len, vm::MMAP2_BASE, vm::MMAP2_LIMIT)) {
            Some(b) => b,
            None => return -ENOMEM,
        }
    };
    let moved = old_len.min(new_len);
    let leaves: Vec<(u64, u64)> = p.asp.leaf_range(old, old + moved);
    for &(va, e) in leaves.iter() {
        let from = p.asp.leaf_slot(va);
        unsafe { *from = 0 };
        let to = p.asp.leaf_slot(dest + (va - old));
        unsafe { *to = e };
    }
    let nv = Vma { start: dest, end: dest + new_len, foff: v.foff + (old - v.start), ..v };
    if !dontunmap {
        p.asp.vm.remove(old, old + old_len);
    }
    p.asp.vm.insert(nv);
    vm::flush();
    MREMAP_MOVED.fetch_add(1, Ordering::Relaxed);
    MREMAP_PAGES.fetch_add(leaves.len() as u64, Ordering::Relaxed);
    dest as i64
}

// ---------------------------------------------------------------------------------------------
// sigaltstack
// ---------------------------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Alt {
    pid: u32,
    key: u64,
    sp: u64,
    size: u64,
    /// `SS_AUTODISARM` as installed (kept while disarmed, so `rt_sigreturn` can re-arm).
    autodisarm: bool,
    armed: bool,
}

static ALTS: spin::Mutex<Vec<Alt>> = spin::Mutex::new(Vec::new());

fn alt_of(key: u64) -> Option<Alt> {
    x86_64::instructions::interrupts::without_interrupts(|| ALTS.lock().iter().find(|a| a.key == key).copied())
}

fn alt_set(a: Alt) {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut g = ALTS.lock();
        g.retain(|x| x.key != a.key);
        g.push(a);
    });
}

/// A new thread starts with no alternate stack (its key may be a dead thread's).
pub fn forget_key(key: u64) {
    x86_64::instructions::interrupts::without_interrupts(|| ALTS.lock().retain(|x| x.key != key));
}

/// exit / execve: the process's alternate stacks are gone.
pub fn forget_pid(pid: u32) {
    x86_64::instructions::interrupts::without_interrupts(|| ALTS.lock().retain(|x| x.pid != pid));
}

/// A new session: nothing survives.
pub fn forget_all() {
    x86_64::instructions::interrupts::without_interrupts(|| ALTS.lock().clear());
}

/// fork: the child's leader (key = its PML4) inherits the forking thread's alternate stack.
pub fn fork_copy(parent: &ProcInfo, child_pid: u32, child_key: u64) {
    forget_key(child_key);
    if let Some(a) = alt_of(super::thread::key_for(parent.pml4)) {
        alt_set(Alt { pid: child_pid, key: child_key, ..a });
    }
}

/// Linux's `on_sig_stack`: `sp` is inside `(ss_sp, ss_sp + ss_size]`.
fn on(a: &Alt, sp: u64) -> bool {
    a.armed && sp > a.sp && sp - a.sp <= a.size
}

fn stack_t(sp: u64, flags: u32, size: u64) -> [u8; 24] {
    let mut o = [0u8; 24];
    o[0..8].copy_from_slice(&sp.to_le_bytes());
    o[8..12].copy_from_slice(&flags.to_le_bytes());
    o[16..24].copy_from_slice(&size.to_le_bytes());
    o
}

/// The `ss_flags` Linux reports for thread state `a` at user sp `usp`.
fn flags_of(a: Option<&Alt>, usp: u64) -> u32 {
    match a {
        Some(a) if a.armed => (if on(a, usp) { SS_ONSTACK } else { 0 }) | if a.autodisarm { SS_AUTODISARM } else { 0 },
        _ => SS_DISABLE,
    }
}

/// `sigaltstack(ss, oss)` (131): stack_t { ss_sp: u64, ss_flags: i32, ss_size: u64 } = 24 bytes.
pub fn sigaltstack(p: &mut LinuxProc, info: &ProcInfo, ktop: u64, ss: u64, oss: u64) -> i64 {
    let key = super::thread::key_for(info.pml4);
    let usp = unsafe { *((ktop - 16) as *const u64) };
    let cur = alt_of(key);
    if oss != 0 {
        let o = match cur {
            Some(a) if a.armed => stack_t(a.sp, flags_of(Some(&a), usp), a.size),
            _ => stack_t(0, SS_DISABLE, 0),
        };
        if !p.asp.copy_out(oss, &o, false) {
            return -EFAULT;
        }
    }
    if ss == 0 {
        return 0;
    }
    let mut b = [0u8; 24];
    if !p.asp.copy_in(ss, &mut b) {
        return -EFAULT;
    }
    if cur.is_some_and(|a| on(&a, usp) && !a.autodisarm) {
        return -EPERM;
    }
    let sp = u64::from_le_bytes(b[0..8].try_into().unwrap_or([0; 8]));
    let fl = u32::from_le_bytes(b[8..12].try_into().unwrap_or([0; 4]));
    let size = u64::from_le_bytes(b[16..24].try_into().unwrap_or([0; 8]));
    let mode = fl & !SS_AUTODISARM;
    if mode != 0 && mode != SS_DISABLE && mode != SS_ONSTACK {
        return -EINVAL;
    }
    if mode == SS_DISABLE {
        alt_set(Alt { pid: info.pid, key, sp: 0, size: 0, autodisarm: false, armed: false });
        return 0;
    }
    if size < MINSIGSTKSZ {
        return -ENOMEM;
    }
    alt_set(Alt { pid: info.pid, key, sp, size, autodisarm: fl & SS_AUTODISARM != 0, armed: true });
    0
}

/// `signal::deliver`'s question: where does the frame go for a handler with `act_flags`, interrupted at `usp`?
/// Returns `(top, uc_stack)`: `top` is the highest byte the frame may use (the red zone already skipped on the normal stack),
/// `uc_stack` the 24 bytes the frame's `ucontext.uc_stack` carries.
pub fn sig_stack(info: &ProcInfo, usp: u64, act_flags: u64) -> (u64, [u8; 24]) {
    let key = super::thread::key_for(info.pml4);
    let cur = alt_of(key);
    let uc = match cur {
        Some(a) if a.armed => stack_t(a.sp, flags_of(Some(&a), usp), a.size),
        _ => stack_t(0, SS_DISABLE, 0),
    };
    match cur {
        Some(a) if act_flags & SA_ONSTACK != 0 && a.armed && !on(&a, usp) => {
            if a.autodisarm {
                alt_set(Alt { armed: false, ..a });
            }
            ALTSTACK_DELIVERIES.fetch_add(1, Ordering::Relaxed);
            (a.sp + a.size, uc)
        }
        _ => (usp.wrapping_sub(128), uc),
    }
}

/// `rt_sigreturn`: an `SS_AUTODISARM` stack disarmed at delivery is re-armed from the frame's `uc_stack` (Linux's
/// `restore_altstack`; without AUTODISARM the thread is on the stack and the restore is a refused no-op there too).
pub fn sigreturn_restore(info: &ProcInfo, uc_stack: &[u8]) {
    if uc_stack.len() < 24 {
        return;
    }
    let fl = u32::from_le_bytes(uc_stack[8..12].try_into().unwrap_or([0; 4]));
    if fl & SS_AUTODISARM == 0 {
        return;
    }
    let key = super::thread::key_for(info.pml4);
    let sp = u64::from_le_bytes(uc_stack[0..8].try_into().unwrap_or([0; 8]));
    let size = u64::from_le_bytes(uc_stack[16..24].try_into().unwrap_or([0; 8]));
    if size >= MINSIGSTKSZ {
        alt_set(Alt { pid: info.pid, key, sp, size, autodisarm: true, armed: true });
    }
}

// ---------------------------------------------------------------------------------------------
// getcpu
// ---------------------------------------------------------------------------------------------

/// `getcpu(cpu, node, cache)` (309).
pub fn getcpu(p: &mut LinuxProc, cpu: u64, node: u64) -> i64 {
    for va in [cpu, node] {
        if va != 0 && !p.asp.copy_out(va, &0u32.to_le_bytes(), false) {
            return -EFAULT;
        }
    }
    0
}
