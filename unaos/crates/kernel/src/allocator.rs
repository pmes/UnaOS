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

pub unsafe fn init_heap_raw(heap_start: *mut u8, heap_size: usize) {
    unsafe { ALLOCATOR.lock().init(heap_start, heap_size) };
}
