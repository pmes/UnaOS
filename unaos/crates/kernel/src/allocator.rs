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
            #[cfg(target_arch = "x86_64")] let hg = heapgap::enter(); let mut heap = self.inner.lock(); #[cfg(target_arch = "x86_64")] heappoison::check(&self.inner, &mut heap); // HEAPREGION (B854): a torn verdict re-initialises the heap onto its reserve (the guard passed in). POISON2 (B846): a death-released heap lock is CHECKED at its next allocation (body at the tail). LINE-NEUTRAL append
            match { #[cfg(target_arch = "x86_64")] heappoison::op(heappoison::OP_ALLOC); if heap_dead() { Err(()) } else { heap.allocate_first_fit(layout) } } {
                Ok(ptr) => { result = ptr.as_ptr(); #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))] high_note(heap.used()); #[cfg(all(target_arch = "x86_64", feature = "wc"))] census::on_alloc(result as usize, layout.size()); #[cfg(target_arch = "x86_64")] heappoison::op(0); } // MEMLINE (B677): the window's high-water, under the heap lock. LINE-NEUTRAL append, code first; MEMLINE2 (B821): the owner census, same lock
                Err(_) => { #[cfg(target_arch = "x86_64")] heappoison::op(0); }
            }
        #[cfg(target_arch = "x86_64")] heapgap::leave_after(heap, hg); });
        #[cfg(target_arch = "x86_64")] heappoison::say(); result
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        arch::without_interrupts(|| {
            #[cfg(target_arch = "x86_64")] let hg = heapgap::enter(); let mut heap = self.inner.lock(); #[cfg(target_arch = "x86_64")] heappoison::check(&self.inner, &mut heap); // HEAPREGION (B854) HEAPFREEFIRST: a free that is the first take after a death runs the SAME check (and may re-init). LINE-NEUTRAL append
            unsafe {
                #[cfg(target_arch = "x86_64")] heappoison::op(heappoison::OP_FREE); if heap_free_ok(ptr as usize) { heap.deallocate(core::ptr::NonNull::new(ptr).unwrap(), layout); } #[cfg(all(target_arch = "x86_64", feature = "wc"))] census::on_free(ptr as usize); #[cfg(target_arch = "x86_64")] heappoison::op(0); // MEMLINE2 (B821): a tracked block leaves the owner census under the heap lock, whoever frees it. LINE-NEUTRAL append, code first. HEAPREGION (B854): a free into the abandoned range is DROPPED (`heap_free_ok`); the census still forgets it
            }
        #[cfg(target_arch = "x86_64")] heapgap::leave_after(heap, hg); }); #[cfg(target_arch = "x86_64")] heappoison::say(); // HEAPREGION (B854) HEAPFREEFIRST: the free's check is said after the lock drops, as alloc's tail does. LINE-NEUTRAL append
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
        let mut heap = ALLOCATOR.inner.lock(); #[cfg(target_arch = "x86_64")] heappoison::census_check(&ALLOCATOR.inner, &mut heap); #[cfg(target_arch = "x86_64")] heappoison::op(heappoison::OP_CENSUS); // BINDLIVE (B873) HEAPCENSUSCHECK: the census take runs the SAME poison check as alloc and free. POISON2 (B846): the trial loop mutates the hole list
        c.used = heap.used();
        c.free = heap.free();
        let step = align.max(1);
        let mut lo = 0usize;
        let mut hi = if heap_dead() { 0 } else { c.free / step * step }; // BINDLIVE (B873): after a fatal verdict the trial loop walks nothing
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
        c.largest_run = lo; #[cfg(target_arch = "x86_64")] heappoison::op(0);
    });
    #[cfg(target_arch = "x86_64")] heappoison::say(); c // BINDLIVE (B873): the census's check is said after the lock drops, as alloc's tail does
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
    use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::{AcqRel, Acquire, Relaxed, Release}};

    pub const STAGE: u8 = 1;
    pub const SHADOW: u8 = 2;
    pub const WALLPAPER: u8 = 3;
    pub const HELP: u8 = 4;
    /// MEMLINE3 (B845): wallpaper's RETIRED picture ([`retag`] at its retire), appres's decoded icons (PIX), the
    /// faces `video::text` holds resident.
    pub const WALLPAPER_RETIRED: u8 = 5;
    pub const APPRES_PIX: u8 = 6;
    pub const TEXT_RESIDENT: u8 = 7;
    /// FIXTURERETAIN (B1017): the GEN7 pages held for the boot by design (GEN7TLB, gen7.md §2.6: r11's HELD, r8's
    /// `reclaim=held` surfaces, gen7blit's sp/dp), named by [`retag`] at each hold site; desktopbuild counts it once.
    pub const GEN7_HELD: u8 = 8;
    /// WINDOWSURFTAG (B1030): a window's surface, renamed at `wm::create_inner` / `wm::shell_remint` by [`retag_tracked`]
    /// (the block the owner allocated; one static name, the row's sum and block count carry the windows).
    pub const WINDOW_SURFACE: u8 = 9;
    /// SLOTRECTAG (B1030): the WINDOWCAP3 slot record (`memory::slot_rec_ensure`), never freed: desktopbuild counts it once.
    pub const SLOT_RECORD: u8 = 10; pub const LOGIN_SURFACE: u8 = 11; // LOGINFURNITUREONCE (B1041): the login furniture's grow-once surface (login card, dialog, instgui), tagged at its allocation by `video::metrics::login_surf`; desktopbuild counts it once. LINE-NEUTRAL append, code first
    /// Tag -> its census name (0 = no tag).
    pub const NAMES: [&str; 12] = ["-", "stage", "shadow", "wallpaper", "help", "wallpaper-retired", "appres-pix", "text-resident", "gen7-held", "window-surface", "slot-record", "login-surface"];
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
    /// MEMLINE3 (B845): a slot whose word is unchanged across two census reads this far apart is reclaimed (a task
    /// killed inside a [`Scope`] never drops it; no Scope lives this long — one grow, one generation).
    const STALE_MS: u64 = 30_000;
    /// MEMLINE3 (B845): the word each slot held at the last census read, and when it was first seen.
    static SEEN: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
    static SEEN_MS: [AtomicU64; SLOTS] = [const { AtomicU64::new(0) }; SLOTS];
    /// MEMLINE3 (B845): slots reclaimed this boot.
    static RECLAIMED: AtomicU64 = AtomicU64::new(0);

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
    /// MEMLINE3 (B845): `own` is the word this Scope armed; the drop restores only while the slot still holds it, so a
    /// slot [`reap_stale`] reclaimed (and perhaps re-issued) is never clobbered.
    pub struct Scope {
        slot: usize,
        prev: u64,
        own: u64,
    }

    impl Drop for Scope {
        fn drop(&mut self) {
            if self.slot < SLOTS {
                let _ = ARMED[self.slot].compare_exchange(self.own, self.prev, AcqRel, Relaxed);
            }
        }
    }

    /// Tag the calling task's allocations `NAMES[tag]` until the returned [`Scope`] drops. Inert when the census is
    /// not armed, outside a task, or with all slots taken.
    pub fn enter(tag: u8) -> Scope {
        let inert = Scope { slot: SLOTS, prev: 0, own: 0 };
        if !ON.load(Acquire) || tag == 0 || tag as usize >= NAMES.len() {
            return inert;
        }
        let Some(tid) = crate::arch::sched::current_id() else { return inert };
        claim(tid, (tid << 8) | tag as u64)
    }

    /// Arm `packed` for task `tid`: its own slot when it has one (the drop restores the word it held), else a free
    /// one. FIXTURERETAIN (B1017): [`enter`]'s loop, shared with [`fx_enter`]. Inert with all slots taken.
    fn claim(tid: u64, packed: u64) -> Scope {
        for (i, s) in ARMED.iter().enumerate() {
            let v = s.load(Acquire);
            if v != 0 && v >> 8 == tid {
                s.store(packed, Release);
                return Scope { slot: i, prev: v, own: packed };
            }
        }
        for (i, s) in ARMED.iter().enumerate() {
            if s.compare_exchange(0, packed, AcqRel, Relaxed).is_ok() {
                return Scope { slot: i, prev: 0, own: packed };
            }
        }
        Scope { slot: SLOTS, prev: 0, own: 0 }
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
        let (name, tagged): (&'static str, bool) = if tag & FX != 0 {
            // FIXTURERETAIN (B1017): a fixture's word names only the blocks a task name would (>= BIG), as `fx:<verb>`.
            match fx_name(tag & !FX) {
                Some(n) if size >= BIG => (n, true),
                _ => return,
            }
        } else if tag != 0 && (tag as usize) < NAMES.len() {
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
        /// MEMLINE3 (B845): census slots reclaimed this boot ([`reap_stale`]).
        pub reclaimed: u64,
    }

    impl Snapshot {
        /// The bytes `name` held (0 = no row).
        pub fn bytes_of(&self, name: &str, tagged: bool) -> u64 {
            self.rows.iter().flatten().find(|r| r.tagged == tagged && r.name == name).map_or(0, |r| r.bytes)
        }
    }

    /// The census now, by owner. Takes the heap lock itself: never call it from inside an allocation.
    pub fn snapshot() -> Snapshot {
        reap_stale();
        let mut s = Snapshot { used: 0, tracked: 0, dropped: 0, rows: [None; ROWS], reclaimed: RECLAIMED.load(Relaxed) };
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

    /// MEMLINE3 (B845) — a census read reclaims a slot whose word has not changed for [`STALE_MS`] (its task died
    /// inside the Scope: x86 sched has no tid-liveness query to ask). Atomics only, before the heap lock. A false
    /// reclaim of a live Scope only leaves its remaining grows untagged (its drop's CAS misses).
    fn reap_stale() {
        let now = crate::arch::ms();
        for i in 0..SLOTS {
            let v = ARMED[i].load(Acquire);
            if v == 0 || SEEN[i].swap(v, AcqRel) != v {
                SEEN_MS[i].store(now, Relaxed);
                continue;
            }
            let stale = if v as u8 & FX != 0 { STALE_FX_MS } else { STALE_MS }; // FIXTURERETAIN (B1017): a fixture outlives 30 s
            if now.saturating_sub(SEEN_MS[i].load(Relaxed)) >= stale && ARMED[i].compare_exchange(v, 0, AcqRel, Relaxed).is_ok() {
                RECLAIMED.fetch_add(1, Relaxed);
                SEEN[i].store(0, Relaxed);
            }
        }
    }

    /// MEMLINE3 (B845) — name the block at `addr` (`size` bytes, the allocation's own length) `NAMES[tag]`: a block
    /// that changes owner without a new allocation (wallpaper's WALL -> RETIRED). Renames a tracked block or starts
    /// tracking an untracked one. Inert while unarmed. Takes the heap lock itself: never call it from an allocation.
    pub fn retag(addr: usize, size: usize, tag: u8) {
        if !ON.load(Acquire) || addr == 0 || size == 0 || tag == 0 || tag as usize >= NAMES.len() {
            return;
        }
        crate::arch::without_interrupts(|| {
            let heap = super::ALLOCATOR.lock();
            insert(addr, size, NAMES[tag as usize], true);
            drop(heap);
        });
    }

    /// WINDOWSURFTAG (B1030) — [`retag`]'s RENAME-ONLY form: name the block STARTING at `addr` `NAMES[tag]` only when the
    /// census already tracks it (the entry keeps its own size); anything else — a static, an interior pointer (a ring-3
    /// window's surface inside its slot record), an untracked small block — is left alone, so a seam that sees every
    /// window's `surf` inserts no phantom entry. `true` = renamed. Inert while unarmed. Takes the heap lock itself: never
    /// call it from an allocation.
    pub fn retag_tracked(addr: usize, tag: u8) -> bool {
        if !ON.load(Acquire) || addr == 0 || tag == 0 || tag as usize >= NAMES.len() {
            return false;
        }
        let name = NAMES[tag as usize];
        crate::arch::without_interrupts(|| {
            let heap = super::ALLOCATOR.lock();
            // SAFETY: the heap lock is held (`heap`), the table's contract.
            let e = unsafe { &mut *TABLE.e.get() };
            let mut i = home(addr);
            let mut hit = false;
            while e[i].addr != 0 {
                if e[i].addr == addr {
                    e[i].nptr = name.as_ptr() as usize;
                    e[i].nlen = name.len();
                    e[i].tagged = true;
                    hit = true;
                    break;
                }
                i = (i + 1) & (CAP - 1);
            }
            drop(heap);
            hit
        })
    }

    // FIXTURERETAIN (rmbp-ledger B1017, BP-cores-22) — PER-FIXTURE ATTRIBUTION. Flight 38's census charged +18387 KiB
    // to `task:bootlist-fx`: every list child carries that task name, so the row was the SUM over sixteen fixtures.
    // [`fx_enter`] (folded on `tests::run`'s fixture line, around `f()`, with the fixture's registered name) arms the
    // task with an FX word, `(tid << 8) | FX | idx`: `idx` names an interned `fx:<verb>` string in [`FXP`]/[`FXL`],
    // so the Entry's existing name field carries the fixture and nesting is exact (a `stage` enter inside a fixture
    // restores the FX word on its drop). [`note`] tracks such a task's block only when >= [`BIG`] (as a task name
    // would), `tagged`, so memcensus prints it bare: `[memline] census tag=fx:gen7`. The strings live in a static
    // arena under [`FXN`], a leaf lock taken only here, never with the heap lock held, never allocating; a name is
    // published (Release) before its idx is handed out, and [`note`] reads it lock-free (Acquire). Full = inert.

    /// The FX bit of a word's tag byte; the low 7 bits index the intern table.
    const FX: u8 = 0x80;
    const FX_MAX: usize = 128;
    const FX_ARENA: usize = 4096;
    /// A fixture runs up to the list's longest budget (`bootlist::BUDGET_MAX_S`) before its word is reaped.
    const STALE_FX_MS: u64 = 3_600_000;
    static FXP: [AtomicUsize; FX_MAX] = [const { AtomicUsize::new(0) }; FX_MAX];
    static FXL: [AtomicUsize; FX_MAX] = [const { AtomicUsize::new(0) }; FX_MAX];

    struct FxArena {
        buf: [u8; FX_ARENA],
        used: usize,
        /// `(source name ptr, source len)` per interned idx, `n` of them.
        src: [(usize, usize); FX_MAX],
        n: usize,
    }
    static FXN: crate::sync::Mutex<FxArena> = crate::sync::Mutex::new(FxArena { buf: [0; FX_ARENA], used: 0, src: [(0, 0); FX_MAX], n: 0 });

    /// The interned `fx:<verb>` for `idx` (lock-free; `None` = not published).
    fn fx_name(idx: u8) -> Option<&'static str> {
        let i = idx as usize;
        if i >= FX_MAX {
            return None;
        }
        let p = FXP[i].load(Acquire);
        if p == 0 {
            return None;
        }
        // SAFETY: `p`/`FXL[i]` were published by [`fx_intern`] for bytes of the static arena that are never rewritten
        // once published (the arena only grows), copied from a `&str` behind the literal ASCII `fx:` prefix.
        Some(unsafe { core::str::from_utf8_unchecked(core::slice::from_raw_parts(p as *const u8, FXL[i].load(Relaxed))) })
    }

    /// `name`'s idx, interning `fx:<name>` on its first use (`None` = the table or the arena is full).
    fn fx_intern(name: &'static str) -> Option<u8> {
        let mut a = FXN.lock();
        let key = (name.as_ptr() as usize, name.len());
        if let Some(i) = a.src[..a.n].iter().position(|&k| k == key) {
            return Some(i as u8);
        }
        let len = 3 + name.len();
        if a.n >= FX_MAX || a.used + len > FX_ARENA {
            return None;
        }
        let (at, i) = (a.used, a.n);
        a.buf[at..at + 3].copy_from_slice(b"fx:");
        a.buf[at + 3..at + len].copy_from_slice(name.as_bytes());
        a.used += len;
        a.src[i] = key;
        a.n += 1;
        let p = a.buf[at..].as_ptr() as usize;
        FXL[i].store(len, Relaxed);
        FXP[i].store(p, Release);
        Some(i as u8)
    }

    /// Name the calling task's >= [`BIG`] blocks `fx:<name>` until the returned [`Scope`] drops (`name` = a fixture's
    /// registered name). Inert when the census is not armed, outside a task, or with the table or the slots full.
    pub fn fx_enter(name: &'static str) -> Scope {
        let inert = Scope { slot: SLOTS, prev: 0, own: 0 };
        if !ON.load(Acquire) {
            return inert;
        }
        let Some(tid) = crate::arch::sched::current_id() else { return inert };
        let Some(idx) = fx_intern(name) else { return inert };
        claim(tid, (tid << 8) | (FX | idx) as u64)
    }

    /// The census is counting (armed at the first desktop build's ask).
    pub fn armed() -> bool {
        ON.load(Acquire)
    }
}

// POISON2 (rmbp-ledger B846, BP-cores-14) — THE HEAP'S DEATH MARK, CHECKED. STACKGUARD3 (B832) gave the heap lock back on
// its holder's death through `sync::release_task` and POISONED it, but nothing read the mark: `linked_list_allocator`
// 0.10.6 keeps its hole list private (`Heap.holes`; `HoleList::first_hole` is `cfg(test)`, `Heap::debug` is
// `cfg(fuzzing)`), so the list cannot be walked. The rule the crate permits: an in-op marker [`OP`] written ONLY with the
// heap lock held says whether a holder is inside the crate's list mutation (an alloc's split, a free's insert/merge, the
// census's trial loop). The next ALLOCATION's take runs [`check`] (`Mutex::take_poison`: one load while nothing is
// poisoned; a hit clears the slot): OP clear = the dead holder was outside any mutation, the list is whole -> released;
// OP set = it died inside one, the list may be half-applied -> said `torn=1` and KEPT (re-initialising needs a fresh
// region from the memory map: owed HEAPREGION). The line is said after the lock drops ([`say`] on alloc's tail), never
// under it. At the tail: no line above moves. x86 only (the registry and the poison table are x86's).
#[cfg(target_arch = "x86_64")]
pub mod heappoison {
    //! CHARTER: Kernel — kernel-by-ruling (B414 LOCKREG): the heap lock's poison check (POISON2 B846).
    use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering::{AcqRel, Acquire, Relaxed, Release}};

    use super::{Heap, Mutex, ALLOCATOR};

    pub const OP_ALLOC: u8 = 1;
    pub const OP_FREE: u8 = 2;
    pub const OP_CENSUS: u8 = 3;

    /// The crate mutation in flight (0 = none). Written only with the heap lock held.
    static OP: AtomicU8 = AtomicU8::new(0);
    /// Checks made (a poisoned take found), and how many read the list torn.
    static CHECKS: AtomicU64 = AtomicU64::new(0);
    static TORN: AtomicU64 = AtomicU64::new(0);
    static LAST_TORN: AtomicBool = AtomicBool::new(false);
    /// What to say after the lock drops, a bit set written only with the heap lock held: `0x80` a check (low nibble = the
    /// torn op, 0 = whole), `0x40` it re-initialised onto the reserve, `0x20` it was FATAL (torn twice), `0x10` a dropped-free
    /// report is due (HEAPREGION B854).
    static PENDING: AtomicU8 = AtomicU8::new(0);

    /// Mark (`kind`) or clear (0) the crate mutation in flight. Caller holds the heap lock.
    #[inline(always)]
    pub(super) fn op(kind: u8) {
        OP.store(kind, Relaxed);
    }

    /// The take's check (caller holds `lock`, whose guard is `heap`): one load while nothing is poisoned.
    #[inline(always)]
    pub(super) fn check(lock: &Mutex<Heap>, heap: &mut Heap) {
        if lock.take_poison() {
            checked(heap);
        }
    }

    /// HEAPREGION (B854): a torn verdict with the reserve unspent RE-INITIALISES the heap onto it (`regrow`); torn again
    /// after that is FATAL (said after the lock drops, then a panic: no allocation or free walks the list again); torn
    /// with no reserve carved is kept, as POISON2 kept it.
    #[cold]
    #[inline(never)]
    fn checked(heap: &mut Heap) {
        let k = OP.swap(0, Relaxed);
        CHECKS.fetch_add(1, AcqRel);
        let mut r = 0x80 | k;
        if k != 0 {
            TORN.fetch_add(1, Relaxed);
            if REINITS.load(Relaxed) != 0 {
                GATE.store(GATE_DEAD, Release);
                r |= 0x20;
            } else {
                let len = RESERVE_LEN.swap(0, Relaxed);
                if len != 0 {
                    regrow(heap, RESERVE_LO.load(Relaxed), len);
                    r |= 0x40;
                }
            }
        }
        LAST_TORN.store(k != 0, Release);
        PENDING.store(r | (PENDING.load(Relaxed) & 0x10), Release);
    }

    /// Say a pending check's line (alloc's tail, the heap lock released). One load while nothing is pending.
    #[inline(always)]
    pub(super) fn say() {
        if PENDING.load(Relaxed) != 0 {
            said();
        }
    }

    #[cold]
    #[inline(never)]
    fn said() {
        let r = PENDING.swap(0, AcqRel);
        if r == 0 {
            return;
        }
        let checks = CHECKS.load(Relaxed);
        if r & 0x80 != 0 {
            let op = match r & 0x0F {
                OP_ALLOC => "alloc",
                OP_FREE => "free",
                OP_CENSUS => "census",
                _ => "?",
            };
            if r & 0x0F == 0 {
                serial_println!("[heap] poison checked torn=0 -> released checks={} (POISON2 B846: the dead holder was outside the hole-list mutation)", checks);
            } else if r & 0x20 != 0 {
                serial_println!("[heap] poison checked torn=1 op={} -> fatal (HEAPREGION B854: torn twice, the reserve is spent) checks={} torn_total={}", op, checks, TORN.load(Relaxed));
            } else if r & 0x40 != 0 {
                serial_println!("[heap] poison checked torn=1 op={} -> reinit reserve={} checks={} torn_total={}", op, REGREW_LEN.load(Relaxed) >> 20, checks, TORN.load(Relaxed));
            } else {
                serial_println!("[heap] poison checked torn=1 op={} -> kept reserve=none (HEAPREGION B854: no reserve was carved; linked_list_allocator 0.10.6 has no hole walk) checks={} torn_total={}", op, checks, TORN.load(Relaxed));
            }
        }
        if r & 0x50 != 0 {
            serial_println!("[heap] reinit reserve={} dropped_frees={} (HEAPREGION B854: frees into the abandoned range are dropped, never walked)", REGREW_LEN.load(Relaxed) >> 20, DROPPED.load(Relaxed));
        }
        if r & 0x20 != 0 {
            panic!("HEAPREGION (B854): the heap's hole list tore twice; the reserve is spent");
        }
    }

    // ── `tests lockreg`'s heap leg (sync.rs `heap_leg`). STATE: 0 armed, 1 held, 2 claimed by the tester (it releases
    // through `release_task`), 3 the holder's bound passed first (it unlocked itself, unpoisoned). ──
    static STATE: AtomicU8 = AtomicU8::new(0);
    static HOLDER: AtomicU64 = AtomicU64::new(0);

    pub fn arm() {
        HOLDER.store(0, Relaxed);
        STATE.store(0, Release);
    }
    pub fn checks() -> u64 {
        CHECKS.load(Acquire)
    }
    pub fn last_torn() -> bool {
        LAST_TORN.load(Acquire)
    }
    pub fn held() -> bool {
        STATE.load(Acquire) == 1
    }
    pub fn holder() -> u64 {
        HOLDER.load(Acquire)
    }
    /// The tester takes the hold over (1 -> 2); `false` = the holder's bound passed first.
    pub fn claim() -> bool {
        STATE.compare_exchange(1, 2, AcqRel, Acquire).is_ok()
    }

    /// `lockreg-heap`: take the heap lock masked, end the hold without a release (the guard forgotten: a death the
    /// guard's Drop never sees), and wait — allocating nothing — until the tester claims it, or 500 ms (TSC) pass.
    pub fn hold_task(_: usize) {
        crate::arch::without_interrupts(|| {
            let g = ALLOCATOR.inner.lock();
            HOLDER.store(crate::sync::here_tid(), Release);
            core::mem::forget(g);
            STATE.store(1, Release);
            let hz = crate::arch::apic::tsc_hz().max(1_000_000);
            // SAFETY: `rdtsc` has no preconditions.
            let t0 = unsafe { core::arch::x86_64::_rdtsc() };
            while STATE.load(Acquire) == 1 {
                // SAFETY: as above.
                let now = unsafe { core::arch::x86_64::_rdtsc() };
                if now.wrapping_sub(t0) > hz / 2 && STATE.compare_exchange(1, 3, AcqRel, Acquire).is_ok() {
                    // SAFETY: this task took the lock above and forgot its guard; nobody claimed it (1 -> 3), so no other
                    // release can run.
                    unsafe { ALLOCATOR.inner.force_unlock() };
                    return;
                }
                core::hint::spin_loop();
            }
        });
    }

    /// The tester claimed the hold (state 2) but `release_task` released nothing: end it, unpoisoned.
    pub fn unlock_claimed() {
        if STATE.load(Acquire) == 2 {
            // SAFETY: state 2 = the holder forgot its guard and handed the hold to the claimer, who calls this once.
            unsafe { ALLOCATOR.inner.force_unlock() };
        }
    }

    /// One allocation (the take that runs [`check`]) and its free.
    pub fn probe() {
        let l = core::alloc::Layout::new::<u64>();
        // SAFETY: a non-zero-size layout; the block is freed with the same layout.
        unsafe {
            let p = core::alloc::GlobalAlloc::alloc(&ALLOCATOR, l);
            if !p.is_null() {
                core::alloc::GlobalAlloc::dealloc(&ALLOCATOR, p, l);
            }
        }
    }

    // ── HEAPREGION (rmbp-ledger B854): the reserve, the one re-init, the dropped frees. Every store below except
    // `set_reserve` (boot, before SMP) is made with the heap lock held, as `OP` is. ──

    const GATE_OPEN: u8 = 0;
    const GATE_MOVED: u8 = 1;
    const GATE_DEAD: u8 = 2;
    /// The free path's one load: 0 no re-init yet, 1 re-initialised (a free into `[OLD_LO, OLD_HI)` is dropped), 2 fatal.
    static GATE: AtomicU8 = AtomicU8::new(GATE_OPEN);
    /// The reserve (`arch::x86_64::memory::heap_reserve_carve`); `RESERVE_LEN` swaps to 0 when spent.
    static RESERVE_LO: AtomicUsize = AtomicUsize::new(0);
    static RESERVE_LEN: AtomicUsize = AtomicUsize::new(0);
    /// The abandoned range and the reserve's length once spent.
    static OLD_LO: AtomicUsize = AtomicUsize::new(0);
    static OLD_HI: AtomicUsize = AtomicUsize::new(0);
    static REGREW_LEN: AtomicUsize = AtomicUsize::new(0);
    static REINITS: AtomicU64 = AtomicU64::new(0);
    static DROPPED: AtomicU64 = AtomicU64::new(0);

    /// Boot (`memory::init`, before SMP): the span a torn heap re-initialises onto, adjacent to the heap's top.
    pub fn set_reserve(lo: usize, len: usize) {
        RESERVE_LO.store(lo, Relaxed);
        RESERVE_LEN.store(len, Release);
    }

    /// The re-init itself, on any `Heap` (the global one under its lock; `reinit_leg`'s scratch one): `Heap::init` resets
    /// the hole list to the new span WITHOUT walking the old one (linked_list_allocator 0.10.6), so it is sound on a torn
    /// list. Returns the abandoned `[lo, hi)`.
    fn regrow_onto(heap: &mut Heap, lo: usize, len: usize) -> (usize, usize) {
        let old = (heap.bottom() as usize, heap.top() as usize);
        // SAFETY: `[lo, lo + len)` is RAM nothing else owns (the carved reserve, or the leg's own buffer), at least a
        // hole's size; the old span's live blocks are never handed out again (frees into it are dropped).
        unsafe { heap.init(lo as *mut u8, len) };
        old
    }

    /// A free of `p` must not reach the new list: it lies in the abandoned range.
    #[inline(always)]
    fn in_old(p: usize, lo: usize, hi: usize) -> bool {
        p >= lo && p < hi
    }

    /// The global re-init (caller holds the heap lock): the abandoned range recorded, `heap_bounds()` widened to the
    /// reserve's OWN bounds (BINDLIVE B873: adjacent or not, `[lo, hi)` still covers every live block), the free gate moved.
    fn regrow(heap: &mut Heap, lo: usize, len: usize) {
        let (olo, ohi) = regrow_onto(heap, lo, len);
        OLD_LO.store(olo, Relaxed);
        OLD_HI.store(ohi, Relaxed);
        REGREW_LEN.store(len, Relaxed);
        super::HEAP_HI.fetch_max(lo + len, Relaxed); super::HEAP_LO.fetch_min(lo, Relaxed); // BINDLIVE (B873) HEAPRESERVE2: a reserve below the heap lowers LO
        REINITS.fetch_add(1, Relaxed);
        GATE.store(GATE_MOVED, Release);
    }

    /// The free gate (caller holds the heap lock): one load while no re-init happened. After one, a free into the abandoned
    /// range is DROPPED and counted (a report pended at 1, 2, 4, 8, … drops); after a fatal verdict nothing is freed.
    #[inline(always)]
    pub(super) fn free_ok(p: usize) -> bool {
        match GATE.load(Relaxed) {
            GATE_OPEN => true,
            g => free_moved(p, g),
        }
    }

    #[cold]
    #[inline(never)]
    fn free_moved(p: usize, g: u8) -> bool {
        if g == GATE_DEAD {
            return false;
        }
        if !in_old(p, OLD_LO.load(Relaxed), OLD_HI.load(Relaxed)) {
            return true;
        }
        let n = DROPPED.fetch_add(1, Relaxed) + 1;
        if n.is_power_of_two() {
            PENDING.fetch_or(0x10, Release);
        }
        false
    }

    /// After a fatal verdict no allocation walks the list (the caller holds the heap lock).
    #[inline(always)]
    pub(super) fn dead() -> bool {
        GATE.load(Relaxed) == GATE_DEAD
    }

    /// `tests lockreg`'s `heap_reinit=` leg: a scratch `Heap` on a 32 KiB buffer, one block, the SAME `regrow_onto` onto a
    /// second buffer, the SAME `in_old` test on the block, and a new allocation that must land in the second span. The
    /// global heap is never touched (the two buffers are ordinary allocations).
    pub fn reinit_leg() -> &'static str {
        const SPAN: usize = 32 * 1024;
        let mut a: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        let mut b: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
        if a.try_reserve_exact(SPAN).is_err() || b.try_reserve_exact(SPAN).is_err() {
            return "skip(scratch-alloc)";
        }
        let l = core::alloc::Layout::from_size_align(64, 8).unwrap();
        let (alo, blo) = (a.as_mut_ptr() as usize, b.as_mut_ptr() as usize);
        // SAFETY: `a`'s capacity is SPAN bytes this function owns; the scratch heap dies before `a` does.
        let mut h = unsafe { Heap::new(alo as *mut u8, SPAN) };
        let Ok(blk) = h.allocate_first_fit(l) else { return "skip(scratch-alloc)" };
        let (olo, ohi) = regrow_onto(&mut h, blo, SPAN);
        if (olo, ohi) != (alo, alo + SPAN) || h.bottom() as usize != blo {
            return "not-reserve";
        }
        if !in_old(blk.as_ptr() as usize, olo, ohi) {
            return "free-kept";
        }
        match h.allocate_first_fit(l) {
            Ok(p) if in_old(p.as_ptr() as usize, blo, blo + SPAN) => "ok",
            _ => "not-reserve",
        }
    }

    // ── BINDLIVE (rmbp-ledger B873) HEAPCENSUSCHECK: `heap_census`'s take runs [`check`] through [`census_check`], which
    // also counts the checks made at a census take (the `census_check=` leg reads it rise). ──
    static CENSUS_CHECKS: AtomicU64 = AtomicU64::new(0);

    /// [`check`] at the census take (caller holds `lock`, whose guard is `heap`): one load while nothing is poisoned.
    #[inline(always)]
    pub(super) fn census_check(lock: &Mutex<Heap>, heap: &mut Heap) {
        if lock.take_poison() {
            CENSUS_CHECKS.fetch_add(1, AcqRel);
            checked(heap);
        }
    }

    /// `tests lockreg`'s `census_check=` leg: the heap lock marked as a dead holder's release marks it (under this task's
    /// own hold, `fs::unafs::poison_leg`'s shape), then one `heap_census`. `ok` = the census take ran the check and read
    /// the list whole; another core's allocation may take the mark first (its own check, also whole), so up to 8 tries.
    pub fn census_leg() -> &'static str {
        let c0 = CENSUS_CHECKS.load(Acquire);
        for _ in 0..8 {
            let marked = crate::arch::without_interrupts(|| {
                let g = ALLOCATOR.inner.lock();
                let m = ALLOCATOR.inner.mark_poison();
                drop(g);
                m
            });
            if !marked {
                return "table-full";
            }
            let _ = super::heap_census(4096);
            if CENSUS_CHECKS.load(Acquire) > c0 {
                return if LAST_TORN.load(Acquire) { "torn" } else { "ok" };
            }
        }
        "lost-race"
    }
}

// HEAPREGION (rmbp-ledger B854): the alloc and free paths' gates, one name on every arch (x86's `heappoison`; elsewhere
// there is no poison check, so nothing is ever dropped and the heap never dies). At the tail: no line above moves.
#[inline(always)]
fn heap_dead() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        heappoison::dead()
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

#[inline(always)]
#[allow(unused_variables)]
fn heap_free_ok(p: usize) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        heappoison::free_ok(p)
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        true
    }
}

// PUMPDEATHREHOME (rmbp-ledger B1016, BP-cores-14) — THE HEAP'S IN-SECTION MARKER. Flight 38 boot 1: the usb-pump died by
// its guard inside prtscr's masked UnaFS save and the heap lock stayed held 124 s by a holder the registry could not name
// (`[lock] spin-wait name=allocator.rs:51 … on=untracked`): `sync::Mutex::lock` takes the RAW lock before `reg::acquire`
// writes the tag, so a holder that dies in that window (calls pushing onto a stack already in its absorber) holds the
// heap with no tag and `release_task`'s walk cannot see it. [`IN`] names the task inside alloc's or dealloc's MASKED
// section per core (stored before the take, cleared after the guard drops: masked, the task neither migrates nor is
// preempted there); [`release_dead`] — called by `release_task` before its walk — releases such a hold, mark first,
// exactly as the walk does, so the next take runs `heappoison::check` (OP is 0 in the gap: `torn=0 -> released`; a
// holder that died inside a list mutation still reads `torn=1` and takes the HEAPREGION path). At the tail: no line
// above moves but the two lock lines and the two closing lines (same-line prefixes). x86 only (the registry is x86's).
#[cfg(target_arch = "x86_64")]
pub mod heapgap {
    //! CHARTER: Kernel — kernel-by-ruling (B414 LOCKREG): a heap hold the registry could not tag, released at its holder's death (PUMPDEATHREHOME B1016).
    use core::sync::atomic::{AtomicU64, Ordering::{AcqRel, Acquire, Relaxed, Release}};

    use crate::arch::gdt::MAX_CPUS;

    use super::ALLOCATOR;

    /// Per core: the task id inside alloc's or dealloc's masked section (0 = none). Written only by that core, masked.
    static IN: [AtomicU64; MAX_CPUS] = [const { AtomicU64::new(0) }; MAX_CPUS];
    /// Gap holds released at a death since boot.
    static RELEASED: AtomicU64 = AtomicU64::new(0);
    /// `tests pumpdeath`'s deliberate gap hold is in place (1) — its own bound may end it (`fixture_rescue`).
    static FIX: AtomicU64 = AtomicU64::new(0);

    /// The section begins (caller masked, before `self.inner.lock()`): `cpu + 1`, or 0 when the registry cannot name
    /// the caller (unarmed, a foreign GS, the scheduler's own context) — then nothing is stored.
    #[inline(always)]
    pub fn enter() -> usize {
        match crate::sync::here() {
            Some((tid, cpu)) if tid != 0 && cpu < MAX_CPUS => {
                IN[cpu].store(tid, Release);
                cpu + 1
            }
            _ => 0,
        }
    }

    /// The section ends: the heap guard `g` drops FIRST, then the marker clears (caller still masked).
    #[inline(always)]
    pub fn leave_after<G>(g: G, c: usize) {
        drop(g);
        leave(c);
    }

    #[inline(always)]
    fn leave(c: usize) {
        if c != 0 && c <= MAX_CPUS {
            IN[c - 1].store(0, Release);
        }
    }

    /// `release_task`'s heap case (before its walk): `tid` died inside the section on some core, the heap is still
    /// locked, the registry names NO holder (a tracked hold is the walk's, or a live holder's) and `tid` was not
    /// spinning for it (`sync::waits_on`) — so `tid` holds it untagged. Mark, force-unlock, one line; 1, else 0.
    pub fn release_dead(tid: u64, why: &str) -> usize {
        if tid == 0 {
            return 0;
        }
        let Some(c) = (0..MAX_CPUS).find(|&c| IN[c].load(Acquire) == tid) else { return 0 };
        IN[c].store(0, Release);
        let m = &ALLOCATOR.inner;
        if !m.is_locked() || m.holder().is_some() || crate::sync::waits_on(tid as u32).is_some() {
            return 0;
        }
        let poisoned = m.mark_poison(); // BEFORE the force-unlock, so the next take sees it (`heappoison::check`)
        // SAFETY: `tid` never runs again (a death site calls `release_task`) and holds the raw heap lock with no guard
        // that could still be used: it took the lock inside this section and died before its tag (no tracked holder,
        // no wait record).
        unsafe { m.force_unlock() };
        RELEASED.fetch_add(1, AcqRel);
        let site = m.site();
        let file = site.file();
        serial_println!(
            "[lock] released-on-death name={}:{} at=heap-gap by={}@cpu{} why={} poisoned={}",
            file.rsplit('/').next().unwrap_or(file), site.line(), tid, c, why, poisoned as u8
        );
        1
    }

    /// PUMPWEDGE's `heap_holder=`: the registry's holder `<tid>@cpu<n>`, else a section task not spinning
    /// `gap:<tid>@cpu<n>`, else `untracked` while locked, else `none`.
    pub fn holder_word(out: &mut impl core::fmt::Write) {
        let m = &ALLOCATOR.inner;
        if let Some((t, c)) = m.holder() {
            let _ = write!(out, "{}@cpu{}", t, c);
            return;
        }
        if !m.is_locked() {
            let _ = out.write_str("none");
            return;
        }
        for (c, w) in IN.iter().enumerate() {
            let t = w.load(Acquire);
            if t != 0 && crate::sync::waits_on(t as u32).is_none() {
                let _ = write!(out, "gap:{}@cpu{}", t, c);
                return;
            }
        }
        let _ = out.write_str("untracked");
    }

    /// PUMPWEDGE's `heap_spin_ms=`: the oldest live spin on the heap lock (0 = nobody waits).
    pub fn spin_ms() -> u64 {
        ALLOCATOR.inner.wait_ms()
    }

    /// Gap holds released at a death since boot.
    pub fn released() -> u64 {
        RELEASED.load(Acquire)
    }

    /// The heap lock is held right now (anyone).
    pub fn locked() -> bool {
        ALLOCATOR.inner.is_locked()
    }

    /// `tests pumpdeath`'s deliberate gap hold is in place.
    pub fn fixture_taken() -> bool {
        FIX.load(Acquire) == 1
    }

    /// `pumpdeath-die`: masked, enter the section, take the heap, forget the guard and drop the registry's tag (the gap,
    /// made on purpose), then recurse into this task's guard page — a REAL guard death (`overflow_exit` ->
    /// `release_task`). Allocates nothing. Returns only if the guard never fired (the marker cleared; the hold stays
    /// for the fixture's bound).
    pub fn fixture_die() -> u64 {
        crate::arch::without_interrupts(|| {
            let c = enter();
            core::mem::forget(ALLOCATOR.inner.lock());
            ALLOCATOR.inner.untrack();
            FIX.store(1, Release);
            let n = crate::arch::stackguard::scratch_deep(0);
            leave(c);
            n
        })
    }

    /// The fixture's bound: its own hold still in place (no release came) — end it, unpoisoned, as `heappoison::hold_task`'s
    /// bound does. `true` = it rescued the heap.
    pub fn fixture_rescue() -> bool {
        if FIX.swap(0, AcqRel) != 1 {
            return false;
        }
        let m = &ALLOCATOR.inner;
        if m.is_locked() && m.holder().is_none() {
            // SAFETY: the fixture task took the lock untagged and is dead or returned without its guard; no release ran.
            unsafe { m.force_unlock() };
            return true;
        }
        false
    }

    /// The fixture is over (released or rescued): the next run starts clean.
    pub fn fixture_clear() {
        FIX.store(0, Relaxed);
    }
}
