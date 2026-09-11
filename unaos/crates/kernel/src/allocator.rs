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
// x86_64: 256 MiB. The 48 MiB the heap held through GR26 was sized for the video back buffer
// (~28 MiB at Retina 2880x1800 stride 4096) "plus margin" — and the desktop consumed the margin:
// 8 per-core WCPAR STAGE pools grow lazily toward MAX_STAGE_BYTES each, the shell window holds a
// ~5 MiB surface, wc-d wants a surface-sized verify snapshot. GR27 Boot A showed the end state of
// 48 MiB on metal: the heap pinned at zero (`[wc-d] verify win=2 -> SKIP (no memory ...)`), every
// stage `try_reserve` declining (`DECL_ALLOC`, 7k+ per boot), every present falling to the
// UNCLIPPED direct path — windows visibly bleeding through their occluders. The machine has GiBs;
// the heap was the artificial famine. 256 MiB restores the designed regime (staged presents,
// declines rare) with ~5x headroom over today's peak consumers, and stays comfortably inside the
// QEMU test configs' -m 1G. The x86 heap is carved from the first >=16 MiB Usable region that can
// hold it (arch/x86_64/memory.rs) on the identity map — no page-table cost to the raise.
//
// aarch64 keeps 48 MiB: its heap region is HAND-PLACED at 32 MiB, 64 MiB long
// (arch/aarch64/boot.rs), so a shared raise past 64 MiB would leave the Pi with NO heap at all.
// Widening that region belongs to the aarch64 lane; this split keeps its build byte-identical.
#[cfg(target_arch = "x86_64")]
pub const HEAP_SIZE: usize = 256 * 1024 * 1024; // 256 MiB
#[cfg(not(target_arch = "x86_64"))]
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

// VUGRAS: record the heap's identity-mapped span at init so the RAS localizer can name
// [heap_lo, heap_hi) as a candidate range for a decoded fault ADDR (and bound its DC CIVAC
// sweep) without reaching into the allocator internals. Read-only diagnostic surface.
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

// EL0MEM (orin 26) — the heap census. Until this arc the kernel had NO heap used/free counter on any
// arch, so a refused allocation on metal (`TEGRA-EL0: slot 4 backing allocation FAILED`, `PRTSCR:
// encoder declined (OutOfMemory)`, `[facet] refuse … alloc(2304000)`, all in one render12 sitting)
// could not be told apart as EXHAUSTION (free < need) or FIRST-FIT FRAGMENTATION (free >= need but no
// single run holds it). `used`/`free` are the allocator's own counters. `largest_run` is MEASURED,
// not derived: `linked_list_allocator` keeps its hole list private, so the largest 4 KiB-aligned run
// is found by trial — a binary search over `allocate_first_fit` at `align` with an immediate
// `deallocate` of every hit. That leaves the heap exactly as found (the hole list is address-ordered
// and re-merges on free; a miss does not mutate it) and costs ~log2(heap/4 KiB) walks, so it is a
// launch-path instrument, not a hot-path one. It takes the heap lock itself: never call it from
// inside an allocation.
#[derive(Clone, Copy)]
pub struct HeapCensus {
    pub used: usize,
    pub free: usize,
    /// Largest single `align`-aligned run a first-fit request could be granted right now.
    pub largest_run: usize,
}

pub fn heap_census(align: usize) -> HeapCensus {
    let mut c = HeapCensus { used: 0, free: 0, largest_run: 0 };
    arch::without_interrupts(|| {
        let mut heap = ALLOCATOR.lock();
        c.used = heap.used();
        c.free = heap.free();
        let step = align.max(1);
        let mut lo = 0usize;
        let mut hi = c.free / step * step;
        while lo < hi {
            // Round the midpoint UP to a step so the search always makes progress toward `hi`.
            let mid = (lo + (hi - lo + step) / 2 + step - 1) / step * step;
            let fits = match Layout::from_size_align(mid, step) {
                Ok(l) => match heap.allocate_first_fit(l) {
                    Ok(p) => {
                        unsafe { heap.deallocate(p, l) };
                        true
                    }
                    Err(_) => false,
                },
                Err(_) => false,
            };
            if fits {
                lo = mid;
            } else {
                hi = mid - step;
            }
        }
        c.largest_run = lo;
    });
    c
}
