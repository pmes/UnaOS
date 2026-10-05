// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Kernel — driver
//!
//! SELFBUILD4 (B356) — copy-on-write fork for the Linux ABI shim (the compatibility box, as SELFBUILD3's `vm.rs`).
//!
//! LINUXABI2's fork copied every resident page of the parent eagerly (bounded at 64 MiB). A toolchain pipeline forks
//! a big process to exec a small one (`sh -c "a | b"`, make, cargo, the rustc driver), so almost every copied page is
//! thrown away by the child's `execve`. Here a fork SHARES every resident frame:
//!
//! * a private page (the ELF image, the eager stack, a MAP_PRIVATE VMA, brk) becomes read-only in BOTH spaces with
//!   [`COW`] (OS bit 10) set and its logical writability kept in [`CW`] (OS bit 11);
//! * a MAP_SHARED page (anonymous or file) stays writable in both — the frame IS the sharing (Linux semantics; before
//!   this arc the child got a copy). A MAP_SHARED VMA's pages are faulted in before the fork (bounded, [`SHARED_PREFAULT`])
//!   so a page first touched after the fork is shared too;
//! * a write fault on a [`COW`] leaf whose [`CW`] is set copies the frame if another space still holds it, else takes the
//!   frame back writable ([`AddrSpace::cow_break`]); a kernel store (`copy_out`, `writable_range`) breaks it the same way;
//! * [`REFS`] counts the spaces holding a frame (only frames held by two or more); `vm::free_frame` asks [`unshare`]
//!   first, so a frame goes back to the heap or the pool only when its last holder drops it.
//!
//! All Linux tasks of a session run on one core (LINUXABI2), so a CR3 reload after the parent's PTEs are downgraded is
//! the whole TLB shoot-down.

use super::{vm, AddrSpace, ADDR, CANON_MAX, NX, P, PAGE, U, W};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicU64, Ordering};

/// OS-available PTE bit 10: "shared copy-on-write" (the leaf is read-only until a write breaks it).
pub const COW: u64 = 1 << 10;
/// OS-available PTE bit 11: the COW leaf is logically writable (the W bit the share took away).
pub const CW: u64 = 1 << 11;
/// A MAP_SHARED VMA with at most this many pages is faulted in whole before a fork (so the child shares every page).
pub const SHARED_PREFAULT: u64 = 16384;

/// Frames held by two or more address spaces: frame -> holders.
static REFS: spin::Mutex<BTreeMap<u64, u32>> = spin::Mutex::new(BTreeMap::new());

// ---- counters (the `tests selfbuild4` kernel line) ----
pub static COW_FORKS: AtomicU64 = AtomicU64::new(0);
/// Pages a fork shared instead of copying (private + shared).
pub static COW_SHARED: AtomicU64 = AtomicU64::new(0);
/// Write faults / kernel stores that copied a still-shared frame.
pub static COW_COPIES: AtomicU64 = AtomicU64::new(0);
/// Write faults / kernel stores that found the frame no longer shared and took it back writable without a copy.
pub static COW_REUSES: AtomicU64 = AtomicU64::new(0);

fn share(f: u64) {
    x86_64::instructions::interrupts::without_interrupts(|| *REFS.lock().entry(f).or_insert(1) += 1);
}

/// One holder lets go of `f`: `true` = another space still holds it (do NOT free it).
pub fn unshare(f: u64) -> bool {
    x86_64::instructions::interrupts::without_interrupts(|| {
        let mut r = REFS.lock();
        match r.get_mut(&f) {
            Some(c) => {
                *c -= 1;
                if *c <= 1 {
                    r.remove(&f);
                }
                true
            }
            None => false,
        }
    })
}

fn holders(f: u64) -> u32 {
    x86_64::instructions::interrupts::without_interrupts(|| REFS.lock().get(&f).copied().unwrap_or(1))
}

/// Turn leaf `bits` into its copy-on-write form: W moves into [`CW`].
pub fn cowify(bits: u64) -> u64 {
    let b = bits | COW;
    if b & W != 0 { (b & !W) | CW } else { b & !CW }
}

impl AddrSpace {
    /// SELFBUILD4 fork: a new address space SHARING every resident frame (see the module note) with the same VMA list.
    pub fn fork_cow(&self) -> Option<AddrSpace> {
        // MAP_SHARED VMAs: back them whole first (bounded), so the child shares every page, not only the touched ones.
        let shared: Vec<(u64, u64)> = self.vm.vmas.iter().filter(|v| v.shared && v.r).map(|v| (v.start, v.end)).collect();
        for (lo, hi) in shared {
            if (hi - lo) / PAGE <= SHARED_PREFAULT {
                let mut a = lo;
                while a < hi && self.populate(a, false, false).is_ok() {
                    a += PAGE;
                }
            }
        }
        let list: Vec<(u64, u64)> = {
            let frames = &self.m.borrow().frames;
            self.leaf_range(0, CANON_MAX).into_iter().filter(|(_, e)| frames.contains(&(e & ADDR))).collect()
        };
        let mut c = AddrSpace::new();
        c.vm = self.vm.clone();
        let keep = P | W | U | NX | vm::SWN | COW | CW;
        let mut n = 0u64;
        for (va, e) in list {
            let f = e & ADDR;
            let in_shared = self.vm.find(va).is_some_and(|v| v.shared);
            let bits = if in_shared { e & keep & !(COW | CW) } else if e & COW != 0 { e & keep } else { cowify(e & keep) };
            if !in_shared {
                let slot = self.leaf_slot(va);
                unsafe { *slot = f | (e & (1 << 6)) | bits }; // the parent keeps its dirty bit (MAP_SHARED write-back)
            }
            share(f);
            c.install(va, f, bits);
            n += 1;
        }
        unsafe { super::memory::load_cr3(super::memory::current_cr3()) };
        COW_FORKS.fetch_add(1, Ordering::Relaxed);
        COW_SHARED.fetch_add(n, Ordering::Relaxed);
        Some(c)
    }

    /// Break the share of `va`'s page for a write: `Some((frame, pte))` writable now, `None` = not a COW leaf, or a COW
    /// leaf that is not logically writable, or no frame for the copy (the budget line is printed by `take_frame`).
    pub(super) fn cow_break(&self, va: u64) -> Option<(u64, u64)> {
        let va = va & !(PAGE - 1);
        let e = self.raw_leaf(va)?;
        if e & COW == 0 || e & U == 0 || e & P == 0 || e & CW == 0 {
            return None;
        }
        let f = e & ADDR;
        let bits = (e & !ADDR & !(COW | CW)) | W;
        let nf = if holders(f) > 1 {
            let nf = self.take_frame(va).ok()?;
            unsafe { core::ptr::copy_nonoverlapping(f as *const u8, nf as *mut u8, PAGE as usize) };
            unshare(f);
            self.m.borrow_mut().frames.remove(&f);
            self.m.borrow_mut().frames.insert(nf);
            COW_COPIES.fetch_add(1, Ordering::Relaxed);
            nf
        } else {
            COW_REUSES.fetch_add(1, Ordering::Relaxed);
            f
        };
        let slot = self.leaf_slot(va);
        unsafe {
            *slot = nf | bits;
            core::arch::asm!("invlpg [{}]", in(reg) va, options(nostack, preserves_flags));
        }
        Some((nf, nf | bits))
    }

    /// A user page for a kernel STORE: an existing writable page, a COW page broken for the write, or a lazy page backed.
    pub(super) fn page_for_store(&self, va: u64) -> Option<(u64, u64)> {
        match self.user_page(va) {
            Some((_, e)) if e & COW != 0 => self.cow_break(va).or(Some((e & ADDR, e))),
            Some(pe) => Some(pe),
            None => self.fault_in(va, true),
        }
    }
}
