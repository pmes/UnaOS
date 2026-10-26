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
use crate::sync::Mutex;
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
                Ok(ptr) => { result = ptr.as_ptr(); #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))] high_note(heap.used()); #[cfg(all(target_arch = "x86_64", feature = "wc"))] census::on_alloc(result as usize, layout.size()); } // MEMLINE (B677): the window's high-water, under the heap lock. LINE-NEUTRAL append, code first; MEMLINE2 (B821): the owner census, same lock
                Err(_) => {}
            }
        });
        result
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        arch::without_interrupts(|| {
            let mut heap = self.inner.lock();
            unsafe {
                heap.deallocate(core::ptr::NonNull::new(ptr).unwrap(), layout); #[cfg(all(target_arch = "x86_64", feature = "wc"))] census::on_free(ptr as usize); // MEMLINE2 (B821): a tracked block leaves the owner census under the heap lock, whoever frees it. LINE-NEUTRAL append, code first
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

/// PANICSCREEN (B406): the heap lock is held right now (the panic log is written only when it is free — a
/// panic inside the allocator holds it for good).
pub fn heap_busy() -> bool {
    ALLOCATOR.inner.is_locked()
}

// MEMLINE (rmbp-ledger B677, BP-bar-24, BP-bar-9) — THE HEAP'S HIGH-WATER, OVER A WINDOW. EL0MEM's census reads `used` and
// `free` NOW; the bar's memory line needs the PEAK a span reached (a build that allocates and frees a transient buffer
// leaves `used` where it found it and the cost unseen). [`high_note`] runs on every granted allocation, inside the heap
// lock, so a plain load/compare/store is serialised by that lock (no locked RMW on the hot path); [`heap_high_reset`]
// takes the same lock to open a window at the current `used`. Frees do not lower it: it is a high-water, read by
// [`heap_high`]. The desktop family only (the one reader is `video::desktopbuild`): a build without the desktop links
// none of it, and the alloc line above compiles as it did. At the tail: no line above moves.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
static HEAP_HIGH: AtomicUsize = AtomicUsize::new(0);

/// A granted allocation left the heap at `used` bytes: raise the window's high-water. Caller holds the heap lock.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
#[inline(always)]
fn high_note(used: usize) {
    if used > HEAP_HIGH.load(Ordering::Relaxed) {
        HEAP_HIGH.store(used, Ordering::Relaxed);
    }
}

/// The heap's `used` bytes now (one field read under the lock; never call it from inside an allocation).
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub fn heap_used() -> usize {
    arch::without_interrupts(|| ALLOCATOR.lock().used())
}

/// Open a high-water window: the mark restarts at `used` now, which it returns.
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub fn heap_high_reset() -> usize {
    arch::without_interrupts(|| {
        let used = ALLOCATOR.lock().used();
        HEAP_HIGH.store(used, Ordering::Relaxed);
        used
    })
}

/// The most `used` bytes the heap held since the last [`heap_high_reset`] (since boot if none).
#[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
pub fn heap_high() -> usize {
    HEAP_HIGH.load(Ordering::Relaxed)
}

// MEMLINE2 (rmbp-ledger B821, BP-cores-22) — THE HEAP'S OWNERS. MEMLINE's line says HOW MUCH the heap holds; flight 37
// boot 1 (`:: MEMLINE: heap_kb=139000 … logout_delta_pct=43 -> FAIL :: src=rebuild`, the first build's high 96685) could
// not say WHO holds the 42 MiB a session added. The census is the allocator's own, at the tail so no line above moves:
// * [`enter`]`(tag)` arms `(task id, tag)` in one of [`SLOTS`] atomic words for the [`Scope`]'s life (a grow site in
//   wm.rs, wallpaper.rs, help/index.rs); it never allocates and never locks, and a nested enter restores on drop.
// * Every granted allocation ([`on_alloc`], folded on the grant line) is TRACKED when the census is armed ([`arm`], the
//   desktop build's ask) and either its task has a tag armed or it is >= [`BIG`] (named by its task, `task:<name>`).
// * A tracked block sits in a [`CAP`]-entry side table keyed by address, touched ONLY with the heap lock held (no new
//   lock): [`on_alloc`]/[`on_free`] run inside `GlobalAlloc::alloc`/`dealloc`, [`snapshot`] takes the lock itself. A
//   free removes the block whoever frees it. Past [`MAX_LIVE`] blocks an insert is refused and counted (`dropped`).
// * [`snapshot`] aggregates the table by owner into fixed rows (no allocation under the lock). The reader is
//   `video::desktopbuild`'s MEMLINE fixture. x86 desktop only: every other build links none of it.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub mod census {
    use core::cell::UnsafeCell;
    use core::sync::atomic::{AtomicBool, AtomicU64, Ordering::{AcqRel, Acquire, Relaxed, Release}};

    pub const STAGE: u8 = 1;
    pub const SHADOW: u8 = 2;
    pub const WALLPAPER: u8 = 3;
    pub const HELP: u8 = 4;
    /// Tag -> its census name (0 = no tag).
    pub const NAMES: [&str; 5] = ["-", "stage", "shadow", "wallpaper", "help"];
    /// An untagged block this large is tracked under its task's name.
    pub const BIG: usize = 64 * 1024;
    /// Owner rows a [`Snapshot`] carries (the last one becomes `other` on overflow).
    pub const ROWS: usize = 24;
    const SLOTS: usize = 16;
    const CAP: usize = 4096;
    const MAX_LIVE: usize = CAP / 4 * 3;

    static ON: AtomicBool = AtomicBool::new(false);
    /// `(tid << 8) | tag`, 0 = free.
    static ARMED: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];

    /// One tracked block. All-zero is empty (the table is zero-initialised: `.bss`, not image bytes).
    #[derive(Clone, Copy)]
    struct Entry {
        addr: usize,
        size: usize,
        nptr: usize,
        nlen: usize,
        tagged: bool,
    }
    const EMPTY: Entry = Entry { addr: 0, size: 0, nptr: 0, nlen: 0, tagged: false };

    struct Table {
        e: UnsafeCell<[Entry; CAP]>,
        live: UnsafeCell<usize>,
        tracked: UnsafeCell<usize>,
        dropped: UnsafeCell<u64>,
    }
    // SAFETY: every access is made with the global heap lock held — `on_alloc`/`on_free` are called only from inside
    // `GlobalAlloc::alloc`/`dealloc`'s critical section, and `snapshot` takes `ALLOCATOR`'s lock — so that lock
    // serialises the table.
    unsafe impl Sync for Table {}
    static TABLE: Table = Table {
        e: UnsafeCell::new([EMPTY; CAP]),
        live: UnsafeCell::new(0),
        tracked: UnsafeCell::new(0),
        dropped: UnsafeCell::new(0),
    };

    /// The census starts counting (the desktop build's ask). Never disarmed: a free must always find its block.
    pub fn arm() {
        ON.store(true, Release);
    }

    /// An armed tag; dropping it restores the task's previous tag (or frees the slot). `slot == SLOTS` = inert.
    pub struct Scope {
        slot: usize,
        prev: u64,
    }

    impl Drop for Scope {
        fn drop(&mut self) {
            if self.slot < SLOTS {
                ARMED[self.slot].store(self.prev, Release);
            }
        }
    }

    /// Tag the calling task's allocations `NAMES[tag]` until the returned [`Scope`] drops. Inert when the census is
    /// not armed, outside a task, or with all slots taken.
    pub fn enter(tag: u8) -> Scope {
        let inert = Scope { slot: SLOTS, prev: 0 };
        if !ON.load(Acquire) || tag == 0 || tag as usize >= NAMES.len() {
            return inert;
        }
        let Some(tid) = crate::arch::sched::current_id() else { return inert };
        let packed = (tid << 8) | tag as u64;
        for (i, s) in ARMED.iter().enumerate() {
            let v = s.load(Acquire);
            if v != 0 && v >> 8 == tid {
                s.store(packed, Release);
                return Scope { slot: i, prev: v };
            }
        }
        for (i, s) in ARMED.iter().enumerate() {
            if s.compare_exchange(0, packed, AcqRel, Relaxed).is_ok() {
                return Scope { slot: i, prev: 0 };
            }
        }
        inert
    }

    #[inline(always)]
    fn home(addr: usize) -> usize {
        ((addr >> 4).wrapping_mul(0x9E37_79B9_7F4A_7C15) >> 52) & (CAP - 1)
    }

    /// A granted allocation (caller holds the heap lock). One relaxed load while unarmed.
    #[inline(always)]
    pub(crate) fn on_alloc(addr: usize, size: usize) {
        if ON.load(Relaxed) {
            note(addr, size);
        }
    }

    #[inline(never)]
    fn note(addr: usize, size: usize) {
        // Both reads are lock-free (a published word, a same-core deref): legal under the heap lock (BP-cores-6).
        let Some(tid) = crate::arch::sched::current_id() else { return };
        let mut tag = 0u8;
        for s in ARMED.iter() {
            let v = s.load(Acquire);
            if v != 0 && v >> 8 == tid {
                tag = v as u8;
                break;
            }
        }
        let (name, tagged): (&'static str, bool) = if tag != 0 && (tag as usize) < NAMES.len() {
            (NAMES[tag as usize], true)
        } else if size >= BIG {
            (crate::arch::sched::current_name().unwrap_or("-"), false)
        } else {
            return;
        };
        insert(addr, size, name, tagged);
    }

    /// Caller holds the heap lock.
    fn insert(addr: usize, size: usize, name: &'static str, tagged: bool) {
        // SAFETY: the heap lock is held (the table's contract); the three cells are distinct.
        let (e, live, tracked) = unsafe { (&mut *TABLE.e.get(), &mut *TABLE.live.get(), &mut *TABLE.tracked.get()) };
        if *live >= MAX_LIVE {
            // SAFETY: as above.
            unsafe { *TABLE.dropped.get() += 1 };
            return;
        }
        let mut i = home(addr);
        while e[i].addr != 0 && e[i].addr != addr {
            i = (i + 1) & (CAP - 1);
        }
        if e[i].addr == addr {
            *tracked -= e[i].size;
        } else {
            *live += 1;
        }
        e[i] = Entry { addr, size, nptr: name.as_ptr() as usize, nlen: name.len(), tagged };
        *tracked += size;
    }

    /// A freed block (caller holds the heap lock): out of the table if it was tracked.
    #[inline(always)]
    pub(crate) fn on_free(addr: usize) {
        if ON.load(Relaxed) {
            remove(addr);
        }
    }

    /// Caller holds the heap lock. Linear probing with backward-shift deletion (no tombstones).
    #[inline(never)]
    fn remove(addr: usize) {
        // SAFETY: the heap lock is held (the table's contract); the cells are distinct.
        let (e, live, tracked) = unsafe { (&mut *TABLE.e.get(), &mut *TABLE.live.get(), &mut *TABLE.tracked.get()) };
        let mut i = home(addr);
        loop {
            if e[i].addr == 0 {
                return;
            }
            if e[i].addr == addr {
                break;
            }
            i = (i + 1) & (CAP - 1);
        }
        *tracked -= e[i].size;
        *live -= 1;
        let mut hole = i;
        let mut j = i;
        loop {
            j = (j + 1) & (CAP - 1);
            if e[j].addr == 0 {
                break;
            }
            let h = home(e[j].addr);
            // `e[j]` may fill the hole when the hole lies on its probe path, cyclically in [h, j).
            if (hole.wrapping_sub(h) & (CAP - 1)) < (j.wrapping_sub(h) & (CAP - 1)) {
                e[hole] = e[j];
                hole = j;
            }
        }
        e[hole] = EMPTY;
    }

    /// One owner's tracked bytes at a [`snapshot`].
    #[derive(Clone, Copy)]
    pub struct Row {
        pub name: &'static str,
        /// `true` = an armed tag (`stage`, …); `false` = a task name (printed `task:<name>`).
        pub tagged: bool,
        pub bytes: u64,
        pub blocks: u32,
    }

    /// The heap's `used`, the tracked total, the refused inserts and the owner rows, at one instant.
    #[derive(Clone, Copy)]
    pub struct Snapshot {
        pub used: u64,
        pub tracked: u64,
        pub dropped: u64,
        pub rows: [Option<Row>; ROWS],
    }

    impl Snapshot {
        /// The bytes `name` held (0 = no row).
        pub fn bytes_of(&self, name: &str, tagged: bool) -> u64 {
            self.rows.iter().flatten().find(|r| r.tagged == tagged && r.name == name).map_or(0, |r| r.bytes)
        }
    }

    /// The census now, by owner. Takes the heap lock itself: never call it from inside an allocation.
    pub fn snapshot() -> Snapshot {
        let mut s = Snapshot { used: 0, tracked: 0, dropped: 0, rows: [None; ROWS] };
        crate::arch::without_interrupts(|| {
            let heap = super::ALLOCATOR.lock();
            s.used = heap.used() as u64;
            // SAFETY: the heap lock is held (`heap`), the table's contract.
            let e = unsafe { &*TABLE.e.get() };
            // SAFETY: as above.
            unsafe {
                s.tracked = *TABLE.tracked.get() as u64;
                s.dropped = *TABLE.dropped.get();
            }
            for x in e.iter().filter(|x| x.addr != 0) {
                // SAFETY: `nptr`/`nlen` were taken from a `&'static str` at insert.
                let name: &'static str = unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(x.nptr as *const u8, x.nlen)) };
                let at = match s.rows.iter().position(|r| matches!(r, Some(r) if r.tagged == x.tagged && r.name == name)) {
                    Some(k) => k,
                    None => match s.rows.iter().position(|r| r.is_none()) {
                        Some(k) => {
                            s.rows[k] = Some(Row { name, tagged: x.tagged, bytes: 0, blocks: 0 });
                            k
                        }
                        None => {
                            let k = ROWS - 1;
                            if let Some(r) = s.rows[k].as_mut() {
                                r.name = "other";
                                r.tagged = false;
                            }
                            k
                        }
                    },
                };
                if let Some(r) = s.rows[at].as_mut() {
                    r.bytes += x.size as u64;
                    r.blocks += 1;
                }
            }
            drop(heap);
        });
        s
    }
}
