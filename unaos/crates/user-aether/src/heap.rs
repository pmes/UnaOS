// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// AETHERMETAL (SR66): the ring-3 heap — `vein_ring3::heap` (VEINTLS, SR36), COPIED verbatim (its unit tests stay
// in vein_ring3, which the host workspace runs) so AETHER.ELF does not link the TLS transport to get an allocator.
// The parser, the cascade, the layout tree and the glyph cache allocate and free on every relayout and repaint,
// so a bump allocator would walk off the window; this one reuses freed blocks by size class.
//
// Shape: power-of-two size classes (16 B .. 8 MiB), one intrusive free list per class, carved from a bump
// region that grows through SYS_SBRK. O(1) alloc and free; `realloc` keeps the block when the new size fits
// its class. Memory is never handed back to the kernel (the program's peak is its footprint).

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

/// The ring-3 heap: `#[global_allocator] static HEAP: heap::Heap = heap::Heap::sbrk();`
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
