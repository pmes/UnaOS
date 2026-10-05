// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The ring-3 heap (VEINTLS, SR36): tls_core is `no_std` + `alloc` (records, the handshake transcript, the
//! parsed trust store), so a program that links the TLS transport needs a `#[global_allocator]` that FREES —
//! a TLS stream allocates per record for as long as an answer streams, and BIG.ELF's bump allocator would
//! walk off the 4 MiB ELF window inside one long reply.
//!
//! Shape: power-of-two size classes (16 B .. 8 MiB), one intrusive free list per class, carved from a bump
//! region that grows through [`Grow`] (SYS_SBRK in ring 3, a static arena in the host test). Freed blocks
//! go back to their class and are reused; memory is never handed back to the kernel (a program's peak is
//! its footprint, which is the honest budget for a 4 MiB window). O(1) alloc and free; `realloc` keeps the
//! block when the new size fits its class (a `Vec` doubling inside one class costs no copy). A block of
//! class 2^k is aligned to min(2^k, 4096), so any layout with align <= 4096 is served; larger alignments
//! are refused (null). One spin flag serialises callers (a ring-3 program may run a second thread).

use core::alloc::{GlobalAlloc, Layout};
use core::cell::UnsafeCell;
use core::ptr::null_mut;
use core::sync::atomic::{AtomicBool, Ordering};

const MIN_SHIFT: u32 = 4;
const MAX_SHIFT: u32 = 23;
const CLASSES: usize = (MAX_SHIFT - MIN_SHIFT + 1) as usize;
const MAX_ALIGN: usize = 4096;
/// The least the region grows by (fewer SYS_SBRK calls).
const GROW_MIN: usize = 64 * 1024;

/// Where fresh memory comes from: `grow(n)` returns the start of `n` new bytes, or null.
pub trait Grow {
    fn grow(&self, bytes: usize) -> *mut u8;
}

/// SYS_SBRK: the kernel extends the program's heap inside its ELF window.
pub struct Sbrk;

impl Grow for Sbrk {
    fn grow(&self, bytes: usize) -> *mut u8 {
        let r = crate::sys::sys(una_abi::SYS_SBRK, bytes as u64, 0, 0, 0);
        if r < 0 { null_mut() } else { r as usize as *mut u8 }
    }
}

struct Inner {
    free: [*mut u8; CLASSES],
    top: usize,
    end: usize,
    /// Bytes currently handed out (by class size), and the peak — for the program's wire line.
    live: usize,
    peak: usize,
}

pub struct SizeClassHeap<G: Grow> {
    lock: AtomicBool,
    inner: UnsafeCell<Inner>,
    grow: G,
}

// SAFETY: every access to `inner` happens under `lock`.
unsafe impl<G: Grow + Sync> Sync for SizeClassHeap<G> {}

/// The ring-3 heap: `#[global_allocator] static HEAP: vein_ring3::heap::Heap = vein_ring3::heap::Heap::sbrk();`
pub type Heap = SizeClassHeap<Sbrk>;

impl SizeClassHeap<Sbrk> {
    pub const fn sbrk() -> Self {
        Self::new(Sbrk)
    }
}

fn class_of(l: &Layout) -> Option<usize> {
    if l.align() > MAX_ALIGN {
        return None;
    }
    let need = l.size().max(l.align()).max(1 << MIN_SHIFT);
    let size = need.checked_next_power_of_two()?;
    let shift = size.trailing_zeros();
    if shift > MAX_SHIFT {
        return None;
    }
    Some((shift - MIN_SHIFT) as usize)
}

const fn class_size(c: usize) -> usize {
    1 << (c as u32 + MIN_SHIFT)
}

impl<G: Grow> SizeClassHeap<G> {
    pub const fn new(grow: G) -> Self {
        SizeClassHeap { lock: AtomicBool::new(false), inner: UnsafeCell::new(Inner { free: [null_mut(); CLASSES], top: 0, end: 0, live: 0, peak: 0 }), grow }
    }

    fn with<R>(&self, f: impl FnOnce(&mut Inner, &G) -> R) -> R {
        while self.lock.compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed).is_err() {
            core::hint::spin_loop();
        }
        // SAFETY: the flag gives exclusive access.
        let r = f(unsafe { &mut *self.inner.get() }, &self.grow);
        self.lock.store(false, Ordering::Release);
        r
    }

    /// (live bytes, peak bytes) by class size.
    pub fn usage(&self) -> (usize, usize) {
        self.with(|i, _| (i.live, i.peak))
    }

    fn take(i: &mut Inner, g: &G, c: usize) -> *mut u8 {
        let head = i.free[c];
        if !head.is_null() {
            // SAFETY: a free block's first word is the next pointer we stored in `put`.
            i.free[c] = unsafe { *(head as *mut *mut u8) };
            return head;
        }
        let size = class_size(c);
        let align = size.min(MAX_ALIGN);
        let mut start = (i.top + align - 1) & !(align - 1);
        if i.end == 0 || start + size > i.end {
            let want = (size + align).max(GROW_MIN);
            let p = g.grow(want) as usize;
            if p == 0 {
                return null_mut();
            }
            if p != i.end {
                // Not contiguous with the old region: start a fresh one (the old tail is abandoned).
                i.top = p;
            }
            i.end = p + want;
            start = (i.top + align - 1) & !(align - 1);
            if start + size > i.end {
                return null_mut();
            }
        }
        i.top = start + size;
        start as *mut u8
    }

    fn put(i: &mut Inner, c: usize, p: *mut u8) {
        // SAFETY: the block is at least 16 bytes and pointer-aligned (class >= 16, aligned >= 16).
        unsafe { *(p as *mut *mut u8) = i.free[c] };
        i.free[c] = p;
    }
}

unsafe impl<G: Grow> GlobalAlloc for SizeClassHeap<G> {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let Some(c) = class_of(&l) else { return null_mut() };
        self.with(|i, g| {
            let p = Self::take(i, g, c);
            if !p.is_null() {
                i.live += class_size(c);
                i.peak = i.peak.max(i.live);
            }
            p
        })
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        let Some(c) = class_of(&l) else { return };
        self.with(|i, _| {
            i.live -= class_size(c);
            Self::put(i, c, p)
        })
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, new_size: usize) -> *mut u8 {
        let Some(old) = class_of(&l) else { return null_mut() };
        let Ok(nl) = Layout::from_size_align(new_size, l.align()) else { return null_mut() };
        let Some(new) = class_of(&nl) else { return null_mut() };
        if new == old {
            return p;
        }
        // SAFETY: the GlobalAlloc contract — `p` is live with layout `l`.
        let q = unsafe { self.alloc(nl) };
        if !q.is_null() {
            unsafe {
                core::ptr::copy_nonoverlapping(p, q, l.size().min(new_size));
                self.dealloc(p, l);
            }
        }
        q
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use core::cell::Cell;

    /// A fixed arena as the "kernel": hands out consecutive pieces, refuses past its end.
    struct Arena {
        base: usize,
        len: usize,
        used: Cell<usize>,
        calls: Cell<usize>,
    }
    unsafe impl Sync for Arena {}
    impl Grow for Arena {
        fn grow(&self, bytes: usize) -> *mut u8 {
            self.calls.set(self.calls.get() + 1);
            if self.used.get() + bytes > self.len {
                return null_mut();
            }
            let p = self.base + self.used.get();
            self.used.set(self.used.get() + bytes);
            p as *mut u8
        }
    }

    fn arena(len: usize) -> SizeClassHeap<Arena> {
        let mem = std::vec![0u8; len + 4096].leak();
        let base = (mem.as_ptr() as usize + 4095) & !4095;
        SizeClassHeap::new(Arena { base, len, used: Cell::new(0), calls: Cell::new(0) })
    }

    #[test]
    fn classes_align_and_reuse() {
        let h = arena(1 << 20);
        unsafe {
            for (size, align) in [(1, 1), (16, 8), (17, 8), (100, 16), (4096, 4096), (5000, 8), (70_000, 64)] {
                let l = Layout::from_size_align(size, align).unwrap();
                let p = h.alloc(l);
                assert!(!p.is_null());
                assert_eq!(p as usize % align, 0);
                assert_eq!(p as usize % class_size(class_of(&l).unwrap()).min(MAX_ALIGN), 0);
                p.write_bytes(0xA5, size);
                h.dealloc(p, l);
                // The freed block is the next one handed out for the same class.
                assert_eq!(h.alloc(l), p);
                h.dealloc(p, l);
            }
            assert_eq!(h.usage().0, 0);
            assert!(h.alloc(Layout::from_size_align(8, 8192).unwrap()).is_null(), "align > 4096 refused");
        }
    }

    #[test]
    fn a_tls_shaped_stream_stays_bounded() {
        // A long answer: per record a 16 KiB plaintext Vec and a 17 KiB record Vec, freed in turn. A bump
        // allocator would need ~100 MB for 3000 records; this heap must stay at its first peak.
        let h = arena(1 << 20);
        unsafe {
            let (a, b) = (Layout::from_size_align(16_384, 1).unwrap(), Layout::from_size_align(16_384 + 256, 1).unwrap());
            for _ in 0..3000 {
                let p = h.alloc(a);
                let q = h.alloc(b);
                assert!(!p.is_null() && !q.is_null());
                h.dealloc(p, a);
                h.dealloc(q, b);
            }
            let (live, peak) = h.usage();
            assert_eq!(live, 0);
            assert_eq!(peak, 16_384 + 32_768);
            assert!(h.grow.used.get() <= 2 * GROW_MIN + 32_768, "used {}", h.grow.used.get());
        }
    }

    #[test]
    fn realloc_in_class_keeps_the_block_and_copies_across() {
        let h = arena(1 << 20);
        unsafe {
            let l = Layout::from_size_align(40, 8).unwrap();
            let p = h.alloc(l);
            for i in 0..40 {
                *p.add(i) = i as u8;
            }
            assert_eq!(h.realloc(p, l, 64), p, "40 -> 64 stays in the 64 B class");
            let l64 = Layout::from_size_align(64, 8).unwrap();
            let q = h.realloc(p, l64, 1000);
            assert_ne!(q, p);
            for i in 0..40 {
                assert_eq!(*q.add(i), i as u8);
            }
            h.dealloc(q, Layout::from_size_align(1000, 8).unwrap());
            assert_eq!(h.usage().0, 0);
        }
    }

    #[test]
    fn exhaustion_is_null_not_a_crash() {
        let h = arena(GROW_MIN * 2);
        unsafe {
            let l = Layout::from_size_align(GROW_MIN, 8).unwrap();
            assert!(!h.alloc(l).is_null());
            assert!(h.alloc(Layout::from_size_align(GROW_MIN * 4, 8).unwrap()).is_null());
        }
    }
}
