// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

use core::alloc::{GlobalAlloc, Layout};
use core::ptr::null_mut;
use linked_list_allocator::Heap;
use spin::Mutex;
use crate::arch;

pub const HEAP_START: usize = 0x_4444_4444_0000;
// 48 MiB. The video back buffer (double-buffering) is heap-allocated and sized to the chosen
// framebuffer mode, so the heap must hold it plus the xHCI/console/block allocations. The
// bootloader drives the panel at its native (EDID) resolution: a Retina panel is 2880x1800x4 ~=
// 20 MiB, and Apple GOP often pads the stride, so the back buffer can approach ~30 MiB. 48 MiB
// covers that with margin. The bootloader caps mode selection so the back buffer can never exceed
// what this heap can hold (see MAX_BACKBUF_BYTES in crates/bootloader).
pub const HEAP_SIZE: usize = 48 * 1024 * 1024; // 48 MiB

pub struct Locked<A> {
    inner: Mutex<A>,
}

impl<A> Locked<A> {
    pub const fn new(inner: A) -> Self {
        Locked {
            inner: Mutex::new(inner),
        }
    }

    pub fn lock(&self) -> impl core::ops::DerefMut<Target = A> + '_ {
        self.inner.lock()
    }
}

unsafe impl GlobalAlloc for Locked<Heap> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut result = null_mut();
        arch::without_interrupts(|| {
            let mut heap = self.inner.lock();
            match heap.allocate_first_fit(layout) {
                Ok(ptr) => result = ptr.as_ptr(),
                Err(_) => {}
            }
        });
        result
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        arch::without_interrupts(|| {
            let mut heap = self.inner.lock();
            unsafe {
                heap.deallocate(core::ptr::NonNull::new(ptr).unwrap(), layout);
            }
        });
    }
}

#[global_allocator]
static ALLOCATOR: Locked<Heap> = Locked::new(Heap::empty());

// XCARVE-4 diagnostic surface (mirrors hw-jetson): expose the heap's [lo, hi) so shared
// consumers (xhci scratchpad bounds guard) can sanity-check placements. (0, 0) pre-init.
use core::sync::atomic::{AtomicUsize, Ordering};
static HEAP_LO: AtomicUsize = AtomicUsize::new(0);
static HEAP_HI: AtomicUsize = AtomicUsize::new(0);

/// The heap's `[lo, hi)` byte span (identity-mapped PA==VA on the metal targets). `(0, 0)` before
/// `init_heap_raw` has run.
pub fn heap_bounds() -> (usize, usize) {
    (HEAP_LO.load(Ordering::Relaxed), HEAP_HI.load(Ordering::Relaxed))
}

pub unsafe fn init_heap_raw(heap_start: *mut u8, heap_size: usize) {
    HEAP_LO.store(heap_start as usize, Ordering::Relaxed);
    HEAP_HI.store(heap_start as usize + heap_size, Ordering::Relaxed);
    unsafe { ALLOCATOR.lock().init(heap_start, heap_size) };
}
