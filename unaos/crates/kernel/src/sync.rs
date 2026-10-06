// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Kernel — kernel-by-ruling
//! LOCKREG (rmbp-ledger B414) — ONE spin lock type for the kernel. Design: docs/dev/evidence/rmbp-1005/lockreg.md.
//!
//! [`Mutex<T>`] is spin's mutex with the same API (`new` const, `lock`, `try_lock`, `is_locked`, `force_unlock`,
//! `get_mut`, `into_inner`), built as a `spin::Mutex<()>` raw lock beside an `UnsafeCell<T>` so the raw lock can
//! be released WITHOUT knowing `T` (a dead task's locks, by address). `new` is `#[track_caller]`: the
//! construction site is the lock's NAME (its class). Every kernel `spin::Mutex` is this type (the sweep).

use core::cell::UnsafeCell;
use core::marker::PhantomData;
use core::ops::{Deref, DerefMut};
use core::panic::Location;

/// The kernel's spin lock. Same API as `spin::Mutex`.
pub struct Mutex<T: ?Sized> {
    raw: spin::Mutex<()>,
    site: &'static Location<'static>,
    /// The order class (1-based index of `site` in the registry's class table; 0 = not yet seen).
    #[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))] // aarch64: the wrapper only (no registry)
    class: core::sync::atomic::AtomicU16,
    data: UnsafeCell<T>,
}

unsafe impl<T: ?Sized + Send> Sync for Mutex<T> {}
unsafe impl<T: ?Sized + Send> Send for Mutex<T> {}

/// The held lock. `R` only mirrors spin's relax parameter so `MutexGuard<'_, T, spin::relax::Spin>` reads as before.
pub struct MutexGuard<'a, T: ?Sized + 'a, R = spin::relax::Spin> {
    /// The registry entry this hold occupies (`u16::MAX` = untracked). Released in `Drop`, BEFORE `_raw`.
    #[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))] // aarch64: the wrapper only (no registry)
    slot: u16,
    _raw: spin::MutexGuard<'a, ()>,
    data: *mut T,
    _p: PhantomData<(&'a mut T, R)>,
}

unsafe impl<T: ?Sized + Sync, R> Sync for MutexGuard<'_, T, R> {}

impl<T> Mutex<T> {
    #[track_caller]
    #[inline]
    pub const fn new(value: T) -> Self {
        Mutex { raw: spin::Mutex::new(()), site: Location::caller(), class: core::sync::atomic::AtomicU16::new(0), data: UnsafeCell::new(value) }
    }

    #[inline]
    pub fn into_inner(self) -> T {
        self.data.into_inner()
    }
}

impl<T: ?Sized> Mutex<T> {
    #[inline]
    fn guard<'a>(&'a self, raw: spin::MutexGuard<'a, ()>, slot: u16) -> MutexGuard<'a, T> {
        MutexGuard { slot, _raw: raw, data: self.data.get(), _p: PhantomData }
    }

    #[cfg_attr(not(target_arch = "x86_64"), allow(dead_code))] // aarch64: the wrapper only (no registry)
    #[inline]
    fn addr(&self) -> usize {
        &self.raw as *const spin::Mutex<()> as usize
    }

    #[track_caller]
    #[inline]
    pub fn lock(&self) -> MutexGuard<'_, T> {
        #[cfg(target_arch = "x86_64")]
        {
            let at = Location::caller();
            if let Some(g) = self.raw.try_lock() {
                return self.guard(g, reg::acquire(self.addr(), &self.class, self.site, at));
            }
            // The slow path: say who waits on what, so the 50 ms scan can name a spin past 10 ms.
            let w = reg::wait_begin(self.addr(), self.site, at);
            let g = loop {
                while self.raw.is_locked() {
                    core::hint::spin_loop();
                }
                if let Some(g) = self.raw.try_lock() {
                    break g;
                }
            };
            reg::wait_end(w);
            self.guard(g, reg::acquire(self.addr(), &self.class, self.site, at))
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            self.guard(self.raw.lock(), u16::MAX)
        }
    }

    #[track_caller]
    #[inline]
    pub fn try_lock(&self) -> Option<MutexGuard<'_, T>> {
        #[cfg(target_arch = "x86_64")]
        {
            let at = Location::caller();
            self.raw.try_lock().map(|g| self.guard(g, reg::acquire(self.addr(), &self.class, self.site, at)))
        }
        #[cfg(not(target_arch = "x86_64"))]
        {
            self.raw.try_lock().map(|g| self.guard(g, u16::MAX))
        }
    }

    #[inline]
    pub fn is_locked(&self) -> bool {
        self.raw.is_locked()
    }

    /// # Safety
    /// As `spin::Mutex::force_unlock`: no live guard may still be used.
    #[inline]
    pub unsafe fn force_unlock(&self) {
        #[cfg(target_arch = "x86_64")]
        reg::forget(self.addr());
        unsafe { self.raw.force_unlock() }
    }

    #[inline]
    pub fn get_mut(&mut self) -> &mut T {
        self.data.get_mut()
    }

    /// The construction site — the lock's name.
    #[inline]
    pub fn site(&self) -> &'static Location<'static> {
        self.site
    }
}

impl<T: Default> Default for Mutex<T> {
    #[track_caller]
    fn default() -> Self {
        Mutex::new(T::default())
    }
}

impl<T: ?Sized + core::fmt::Debug> core::fmt::Debug for Mutex<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self.try_lock() {
            Some(g) => write!(f, "Mutex {{ data: {:?} }}", &*g),
            None => f.write_str("Mutex { <locked> }"),
        }
    }
}

impl<T: ?Sized, R> Drop for MutexGuard<'_, T, R> {
    #[inline]
    fn drop(&mut self) {
        #[cfg(target_arch = "x86_64")]
        reg::release(self.slot);
    }
}

impl<T: ?Sized, R> Deref for MutexGuard<'_, T, R> {
    type Target = T;
    #[inline]
    fn deref(&self) -> &T {
        unsafe { &*self.data }
    }
}

impl<T: ?Sized, R> DerefMut for MutexGuard<'_, T, R> {
    #[inline]
    fn deref_mut(&mut self) -> &mut T {
        unsafe { &mut *self.data }
    }
}

impl<T: ?Sized + core::fmt::Debug, R> core::fmt::Debug for MutexGuard<'_, T, R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(&**self, f)
    }
}

impl<T: ?Sized + core::fmt::Display, R> core::fmt::Display for MutexGuard<'_, T, R> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Display::fmt(&**self, f)
    }
}

// ══ LOCKREG M2 — the holder registry (x86_64) ═══════════════════════════════════════════════════════════
// Per-CPU partitions of held entries (occupancy bitmask claimed by CAS — ISR/NMI-safe, no heap), a per-CPU
// waiter record, the order-class table and the edge set. All printing is from `isr_tick`'s 50 ms scan (one
// core, CLOCKCORE's per-core tick) through `serial_println!` (try_lock sinks): never from inside `lock()`.

#[cfg(target_arch = "x86_64")]
pub use reg::{arm, ensure_tests, here_tid, isr_tick, name_task, release_task};

#[cfg(target_arch = "x86_64")]
mod reg {
    use core::panic::Location;
    use core::sync::atomic::Ordering::{AcqRel, Acquire, Relaxed, Release};
    use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU16, AtomicU32, AtomicU64, AtomicUsize};

    use crate::arch::gdt::MAX_CPUS;

    pub const NONE: u16 = u16::MAX;
    /// Held entries per CPU (one bit each in `Part::mask`).
    const PER: usize = 32;
    /// Order classes (construction sites) and recorded edges.
    const CLASSES: usize = 1024;
    const EDGES: usize = 4096;
    /// Thresholds (ms) and the scan period.
    const HELD_MS: u64 = 50;
    const WAIT_MS: u64 = 10;
    const SCAN_MS: u64 = 50;
    /// At most this many `[lock]` scan lines per boot (QUIETBOOT), once per site and kind.
    const LINE_CAP: u32 = 16;

    type Loc = Location<'static>;

    #[repr(align(64))]
    struct Part {
        mask: AtomicU32,
        /// `tid << 24 | class << 8 | (cpu + 1)` (0 = free).
        tag: [AtomicU64; PER],
        lock: [AtomicUsize; PER],
        at: [AtomicPtr<Loc>; PER],
        t0: [AtomicU64; PER],
        /// The one spin-waiter on this core (0 = none): lock address, its name, the acquire site, t0, tid.
        w_lock: AtomicUsize,
        w_name: AtomicPtr<Loc>,
        w_at: AtomicPtr<Loc>,
        w_t0: AtomicU64,
        w_tid: AtomicU64,
        acq: AtomicU64,
        tracked: AtomicU64,
        held_max: AtomicU64,
    }

    impl Part {
        const fn new() -> Self {
            Part {
                mask: AtomicU32::new(0),
                tag: [const { AtomicU64::new(0) }; PER],
                lock: [const { AtomicUsize::new(0) }; PER],
                at: [const { AtomicPtr::new(core::ptr::null_mut()) }; PER],
                t0: [const { AtomicU64::new(0) }; PER],
                w_lock: AtomicUsize::new(0),
                w_name: AtomicPtr::new(core::ptr::null_mut()),
                w_at: AtomicPtr::new(core::ptr::null_mut()),
                w_t0: AtomicU64::new(0),
                w_tid: AtomicU64::new(0),
                acq: AtomicU64::new(0),
                tracked: AtomicU64::new(0),
                held_max: AtomicU64::new(0),
            }
        }
    }

    static ON: AtomicBool = AtomicBool::new(false);
    static PARTS: [Part; MAX_CPUS] = [const { Part::new() }; MAX_CPUS];
    /// Acquires taken while the registry could not place them (before `arm`, GS not the kernel's).
    static EARLY: AtomicU64 = AtomicU64::new(0);
    static CLASS: [AtomicPtr<Loc>; CLASSES] = [const { AtomicPtr::new(core::ptr::null_mut()) }; CLASSES];
    static CLASS_N: AtomicU32 = AtomicU32::new(0);
    /// `a << 16 | b` = class a was held when class b was taken (0 = empty).
    static EDGE: [AtomicU32; EDGES] = [const { AtomicU32::new(0) }; EDGES];
    /// Pending inversions for the scan: `a << 48 | b << 32 | tid << 8 | cpu` (16 kept).
    static INV: [AtomicU64; 16] = [const { AtomicU64::new(0) }; 16];
    static INV_N: AtomicU32 = AtomicU32::new(0);
    static INV_SAID: AtomicU32 = AtomicU32::new(0);
    /// The scan: the next due ms, the lines said, the once-per-site record (site pointer | kind in bit 0..1).
    static NEXT_SCAN: AtomicU64 = AtomicU64::new(0);
    static LINES: AtomicU32 = AtomicU32::new(0);
    static SAID: [AtomicUsize; 64] = [const { AtomicUsize::new(0) }; 64];
    /// The last lock NAME the scan saw held past 50 ms (for `tests lockreg`), and the test's own name (never capped).
    static SEEN_LONG: AtomicPtr<Loc> = AtomicPtr::new(core::ptr::null_mut());
    static TEST_NAME: AtomicPtr<Loc> = AtomicPtr::new(core::ptr::null_mut());

    #[inline]
    fn rdtsc() -> u64 {
        unsafe { core::arch::x86_64::_rdtsc() }
    }

    /// (task id, cpu) of the caller — `None` until armed or when GS is not this kernel's per-CPU block
    /// (an NMI/#MC from ring 3, the syscall entry window): such a hold is simply not tracked.
    #[inline]
    fn who() -> Option<(u64, usize)> {
        if !ON.load(Relaxed) {
            return None;
        }
        let gs = unsafe { x86_64::registers::model_specific::Msr::new(0xC000_0101).read() };
        let (Some(c0), Some(cl)) = (crate::arch::percpu::cpu(0), crate::arch::percpu::cpu(MAX_CPUS - 1)) else { return None };
        if gs < c0 as *const _ as u64 || gs > cl as *const _ as u64 {
            return None;
        }
        let cpu = crate::arch::percpu::this_cpu().cpu_index as usize;
        if cpu >= MAX_CPUS {
            return None;
        }
        Some((crate::arch::sched::current_task_id(cpu).unwrap_or(0), cpu))
    }

    #[inline]
    fn tag_tid(t: u64) -> u64 {
        t >> 24
    }
    #[inline]
    fn tag_class(t: u64) -> u16 {
        ((t >> 8) & 0xFFFF) as u16
    }
    #[inline]
    fn tag_cpu(t: u64) -> usize {
        ((t & 0xFF) as usize).saturating_sub(1)
    }

    /// The task id of the caller (0 when untracked) — for a job that wants to be NAMED later (DECJOB).
    pub fn here_tid() -> u64 {
        who().map(|(t, _)| t).unwrap_or(0)
    }

    /// The class of construction site `site` (1-based; 0 = the table is full).
    pub fn class_of(site: &'static Loc) -> u16 {
        let p = site as *const Loc as *mut Loc;
        let h = ((p as usize >> 3).wrapping_mul(0x9E37_79B9_7F4A_7C15usize) >> 20) % CLASSES;
        for k in 0..CLASSES {
            let i = (h + k) % CLASSES;
            let cur = CLASS[i].load(Acquire);
            if cur == p {
                return (i + 1) as u16;
            }
            if cur.is_null() {
                match CLASS[i].compare_exchange(core::ptr::null_mut(), p, AcqRel, Acquire) {
                    Ok(_) => {
                        CLASS_N.fetch_add(1, Relaxed);
                        return (i + 1) as u16;
                    }
                    Err(now) if now == p => return (i + 1) as u16,
                    Err(_) => {}
                }
            }
        }
        0
    }

    fn class_site(c: u16) -> Option<&'static Loc> {
        if c == 0 || c as usize > CLASSES {
            return None;
        }
        let p = CLASS[c as usize - 1].load(Acquire);
        // SAFETY: every stored pointer is a `&'static Location` (from `#[track_caller]`).
        if p.is_null() { None } else { Some(unsafe { &*p }) }
    }

    /// Record edge a→b; the first time b→a was already recorded, queue an inversion for the scan.
    fn edge(a: u16, b: u16, tid: u64, cpu: usize) {
        let key = (a as u32) << 16 | b as u32;
        let rev = (b as u32) << 16 | a as u32;
        let slot = |k: u32| ((k.wrapping_mul(0x9E37_79B1) >> 16) as usize) % EDGES;
        let h = slot(key);
        for n in 0..64 {
            let i = (h + n) % EDGES;
            let cur = EDGE[i].load(Relaxed);
            if cur == key {
                return;
            }
            if cur == 0 {
                match EDGE[i].compare_exchange(0, key, AcqRel, Relaxed) {
                    Ok(_) => break,
                    Err(now) if now == key => return,
                    Err(_) => continue,
                }
            }
            if n == 63 {
                return; // full around here: the order is not recorded
            }
        }
        let h = slot(rev);
        for n in 0..64 {
            let cur = EDGE[(h + n) % EDGES].load(Relaxed);
            if cur == 0 {
                return;
            }
            if cur == rev {
                let k = INV_N.fetch_add(1, AcqRel) as usize;
                if k < INV.len() {
                    INV[k].store((a as u64) << 48 | (b as u64) << 32 | (tid & 0xFF_FFFF) << 8 | cpu as u64, Release);
                }
                return;
            }
        }
    }

    /// A hold begins: one CAS (the entry) + four stores; the order edges against this task's other holds on this core.
    #[inline]
    pub fn acquire(addr: usize, class: &AtomicU16, site: &'static Loc, at: &'static Loc) -> u16 {
        let Some((tid, cpu)) = who() else {
            EARLY.fetch_add(1, Relaxed);
            return NONE;
        };
        let p = &PARTS[cpu];
        p.acq.store(p.acq.load(Relaxed) + 1, Relaxed);
        let c = match class.load(Relaxed) {
            0 => {
                let c = class_of(site);
                class.store(c, Relaxed);
                c
            }
            c => c,
        };
        let mut m = p.mask.load(Acquire);
        if c != 0 {
            let mut bits = m;
            while bits != 0 {
                let i = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let t = p.tag[i].load(Relaxed);
                if t != 0 && tag_tid(t) == tid {
                    let a = tag_class(t);
                    if a != 0 && a != c {
                        edge(a, c, tid, cpu);
                    }
                }
            }
        }
        let i = loop {
            let free = !m;
            if free == 0 {
                return NONE;
            }
            let i = free.trailing_zeros();
            match p.mask.compare_exchange_weak(m, m | 1 << i, AcqRel, Acquire) {
                Ok(_) => break i as usize,
                Err(now) => m = now,
            }
        };
        p.lock[i].store(addr, Relaxed);
        p.at[i].store(at as *const Loc as *mut Loc, Relaxed);
        p.t0[i].store(rdtsc(), Relaxed);
        p.tag[i].store(tid << 24 | (c as u64) << 8 | (cpu as u64 + 1), Release);
        p.tracked.store(p.tracked.load(Relaxed) + 1, Relaxed);
        (cpu * PER + i) as u16
    }

    /// A hold ends (the guard's `Drop`, before the raw unlock): the held span into this core's max, the entry freed.
    #[inline]
    pub fn release(slot: u16) {
        if slot == NONE {
            return;
        }
        let (cpu, i) = (slot as usize / PER, slot as usize % PER);
        let p = &PARTS[cpu];
        let span = rdtsc().wrapping_sub(p.t0[i].load(Relaxed));
        if span < u64::MAX / 2 && span > p.held_max.load(Relaxed) {
            p.held_max.fetch_max(span, Relaxed);
        }
        p.tag[i].store(0, Release);
        p.mask.fetch_and(!(1u32 << i), Release);
    }

    /// A lock force-released by address (`Mutex::force_unlock`): its entries go with it.
    pub fn forget(addr: usize) {
        for p in PARTS.iter() {
            let mut bits = p.mask.load(Acquire);
            while bits != 0 {
                let i = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                if p.tag[i].load(Acquire) != 0 && p.lock[i].load(Relaxed) == addr {
                    p.tag[i].store(0, Release);
                    p.mask.fetch_and(!(1u32 << i), Release);
                }
            }
        }
    }

    /// The slow path begins on this core: (cpu) for `wait_end`, or `usize::MAX` when not recorded (untracked, or
    /// this core already has a waiter — an ISR spinning above a spinning task).
    #[inline]
    pub fn wait_begin(addr: usize, name: &'static Loc, at: &'static Loc) -> usize {
        let Some((tid, cpu)) = who() else { return usize::MAX };
        let p = &PARTS[cpu];
        if p.w_lock.load(Relaxed) != 0 {
            return usize::MAX;
        }
        p.w_name.store(name as *const Loc as *mut Loc, Relaxed);
        p.w_at.store(at as *const Loc as *mut Loc, Relaxed);
        p.w_t0.store(rdtsc(), Relaxed);
        p.w_tid.store(tid, Relaxed);
        p.w_lock.store(addr, Release);
        cpu
    }

    #[inline]
    pub fn wait_end(cpu: usize) {
        if cpu < MAX_CPUS {
            PARTS[cpu].w_lock.store(0, Release);
        }
    }

    /// Turn the registry on (the BSP, `stackguard::boot_line`, per-CPU blocks up) and say so once.
    pub fn arm() {
        if ON.swap(true, AcqRel) {
            return;
        }
        serial_println!("[lock] registry armed classes={} cap={} (LOCKREG)", CLASS_N.load(Relaxed), PER * MAX_CPUS);
    }

    /// `<file>:<line>` of a site, the file's last path component.
    pub struct Site(pub Option<&'static Loc>);
    impl core::fmt::Display for Site {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            match self.0 {
                Some(l) => {
                    let file = l.file();
                    write!(f, "{}:{}", file.rsplit('/').next().unwrap_or(file), l.line())
                }
                None => f.write_str("?"),
            }
        }
    }

    /// Once per (site, kind) and inside the line cap — or always for the test's own lock.
    fn say_ok(site: Option<&'static Loc>, kind: usize) -> bool {
        let Some(s) = site else { return false };
        let key = (s as *const Loc as usize) | kind;
        for slot in SAID.iter() {
            let cur = slot.load(Acquire);
            if cur == key {
                return false;
            }
            if cur == 0 {
                if slot.compare_exchange(0, key, AcqRel, Acquire).is_err() {
                    if slot.load(Acquire) == key {
                        return false;
                    }
                    continue;
                }
                let test = TEST_NAME.load(Relaxed) == s as *const Loc as *mut Loc;
                return test || LINES.fetch_add(1, Relaxed) < LINE_CAP;
            }
        }
        false
    }

    /// The holder of lock `addr`, as `<tid>@cpu<n>` (any core's partition).
    fn holder_of(addr: usize) -> Option<(u64, usize)> {
        for p in PARTS.iter() {
            let mut bits = p.mask.load(Acquire);
            while bits != 0 {
                let i = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let t = p.tag[i].load(Acquire);
                if t != 0 && p.lock[i].load(Relaxed) == addr {
                    return Some((tag_tid(t), tag_cpu(t)));
                }
            }
        }
        None
    }

    /// Timer ISR, every core (after CLOCKCORE's `isr_tick`): every 50 ms ONE core reads every partition.
    #[inline]
    pub fn isr_tick() {
        if !ON.load(Relaxed) {
            return;
        }
        let now = crate::arch::ms();
        let due = NEXT_SCAN.load(Relaxed);
        if now < due || NEXT_SCAN.compare_exchange(due, now + SCAN_MS, Relaxed, Relaxed).is_err() {
            return;
        }
        scan();
    }

    fn scan() {
        let hz = crate::arch::apic::tsc_hz();
        if hz == 0 {
            return;
        }
        let ms = |c: u64| c / (hz / 1000).max(1);
        let t = rdtsc();
        for p in PARTS.iter() {
            let mut bits = p.mask.load(Acquire);
            while bits != 0 {
                let i = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let tag = p.tag[i].load(Acquire);
                if tag == 0 {
                    continue;
                }
                let (t0, addr, at) = (p.t0[i].load(Relaxed), p.lock[i].load(Relaxed), p.at[i].load(Relaxed));
                let span = t.saturating_sub(t0);
                if span > (u64::MAX >> 1) || ms(span) <= HELD_MS || p.tag[i].load(Acquire) != tag {
                    continue;
                }
                p.held_max.fetch_max(span, Relaxed);
                let name = class_site(tag_class(tag));
                if let Some(n) = name {
                    SEEN_LONG.store(n as *const Loc as *mut Loc, Release);
                }
                if say_ok(name, 0) {
                    let waiters = PARTS.iter().filter(|q| q.w_lock.load(Relaxed) == addr).count();
                    // SAFETY: `at` is a `&'static Location` or null.
                    let at = if at.is_null() { None } else { Some(unsafe { &*at }) };
                    serial_println!("[lock] held-too-long name={} at={} by={}@cpu{} ms={} waiters={}", Site(name), Site(at), tag_tid(tag), tag_cpu(tag), ms(span), waiters);
                }
            }
        }
        for (c, p) in PARTS.iter().enumerate() {
            let addr = p.w_lock.load(Acquire);
            if addr == 0 {
                continue;
            }
            let (t0, tid, name, at) = (p.w_t0.load(Relaxed), p.w_tid.load(Relaxed), p.w_name.load(Relaxed), p.w_at.load(Relaxed));
            let span = t.saturating_sub(t0);
            if span > (u64::MAX >> 1) || ms(span) <= WAIT_MS || p.w_lock.load(Acquire) != addr {
                continue;
            }
            // SAFETY: both are `&'static Location`s or null.
            let (name, at) = (if name.is_null() { None } else { Some(unsafe { &*name }) }, if at.is_null() { None } else { Some(unsafe { &*at }) });
            if say_ok(at, 1) {
                match holder_of(addr) {
                    Some((ht, hc)) => serial_println!("[lock] spin-wait name={} at={} waiter={}@cpu{} on={}@cpu{} ms={}", Site(name), Site(at), tid, c, ht, hc, ms(span)),
                    None => serial_println!("[lock] spin-wait name={} at={} waiter={}@cpu{} on=untracked ms={}", Site(name), Site(at), tid, c, ms(span)),
                }
            }
        }
        let n = (INV_N.load(Acquire) as usize).min(INV.len());
        let said = INV_SAID.load(Relaxed) as usize;
        for k in said..n {
            let v = INV[k].load(Acquire);
            if v == 0 {
                break;
            }
            INV_SAID.store(k as u32 + 1, Relaxed);
            let (a, b) = ((v >> 48) as u16, ((v >> 32) & 0xFFFF) as u16);
            serial_println!("[lock] order-inversion a={} b={} by={}@cpu{}", Site(class_site(a)), Site(class_site(b)), (v >> 8) & 0xFF_FFFF, v & 0xFF);
        }
    }

    /// Up to 16 names for one line.
    struct Names {
        n: usize,
        more: usize,
        v: [Option<&'static Loc>; 16],
    }
    impl Names {
        fn new() -> Self {
            Names { n: 0, more: 0, v: [None; 16] }
        }
        fn push(&mut self, l: Option<&'static Loc>) {
            if self.n < self.v.len() {
                self.v[self.n] = l;
                self.n += 1;
            } else {
                self.more += 1;
            }
        }
    }
    impl core::fmt::Display for Names {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            for k in 0..self.n {
                if k > 0 {
                    f.write_str(",")?;
                }
                write!(f, "{}", Site(self.v[k]))?;
            }
            if self.more > 0 {
                write!(f, ",+{}", self.more)?;
            }
            Ok(())
        }
    }

    /// M3 — a task that will never run its guards' `Drop`s again (halted by STACKGUARD, a panic) gives back EVERY
    /// lock the registry holds in its name: force-unlocked by address, entry freed, all named on one line. Run AFTER
    /// the special releases (`lockowner`: the sink, the UART, the UnaFS mount — which must be leaked, not dropped).
    pub fn release_task(tid: u64, why: &str) -> usize {
        if tid == 0 || !ON.load(Relaxed) {
            return 0;
        }
        let mut names = Names::new();
        for p in PARTS.iter() {
            let mut bits = p.mask.load(Acquire);
            while bits != 0 {
                let i = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let t = p.tag[i].load(Acquire);
                if t == 0 || tag_tid(t) != tid {
                    continue;
                }
                let addr = p.lock[i].load(Relaxed);
                p.tag[i].store(0, Release);
                p.mask.fetch_and(!(1u32 << i), Release);
                if addr != 0 {
                    // SAFETY: `addr` is the raw `spin::Mutex<()>` of a `sync::Mutex` held by `tid`, which never runs
                    // again; the lock outlives the hold (a guard borrows it).
                    unsafe { (*(addr as *const spin::Mutex<()>)).force_unlock() };
                }
                names.push(class_site(tag_class(t)));
            }
        }
        serial_println!("[lock] task={} released=[{}] ({})", tid, names, why);
        names.n + names.more
    }

    /// A LIVE task the kernel has given up on (DECJOB's abort): its holds and its wait NAMED, never released —
    /// force-unlocking under a holder that may still run would corrupt what the lock guards.
    pub fn name_task(tid: u64, why: &str) {
        if tid == 0 || !ON.load(Relaxed) {
            return;
        }
        let mut names = Names::new();
        let mut waits: Option<&'static Loc> = None;
        let t = rdtsc();
        let mut wait_ms = 0;
        for p in PARTS.iter() {
            let mut bits = p.mask.load(Acquire);
            while bits != 0 {
                let i = bits.trailing_zeros() as usize;
                bits &= bits - 1;
                let tg = p.tag[i].load(Acquire);
                if tg != 0 && tag_tid(tg) == tid {
                    names.push(class_site(tag_class(tg)));
                }
            }
            if p.w_lock.load(Acquire) != 0 && p.w_tid.load(Relaxed) == tid {
                let n = p.w_name.load(Relaxed);
                // SAFETY: a `&'static Location` or null.
                waits = if n.is_null() { None } else { Some(unsafe { &*n }) };
                let hz = crate::arch::apic::tsc_hz().max(1000);
                wait_ms = t.saturating_sub(p.w_t0.load(Relaxed)) / (hz / 1000);
            }
        }
        match waits {
            Some(w) => serial_println!("[lock] task={} holds=[{}] waits={} ms={} ({})", tid, names, Site(Some(w)), wait_ms, why),
            None => serial_println!("[lock] task={} holds=[{}] waits=none ({})", tid, names, why),
        }
    }

    // ── M4: `tests lockreg` (typed only, R80) ──────────────────────────────────────────────────────────────

    static HOLD: super::Mutex<u32> = super::Mutex::new(0);
    static DIE: super::Mutex<u32> = super::Mutex::new(0);
    static HOLD_DONE: AtomicBool = AtomicBool::new(false);
    static DIE_TID: AtomicU64 = AtomicU64::new(0);
    static DIE_DONE: AtomicBool = AtomicBool::new(false);

    /// `tests lockreg` registration, once (never run at boot, R80).
    pub fn ensure_tests() {
        static DONE: AtomicBool = AtomicBool::new(false);
        if !DONE.swap(true, AcqRel) {
            crate::tests::register("lockreg", selftest);
        }
    }

    fn hold_task(_: usize) {
        let mut g = HOLD.lock();
        let t0 = crate::arch::ms();
        while crate::arch::ms().saturating_sub(t0) < 60 {
            core::hint::spin_loop();
        }
        *g += 1;
        drop(g);
        HOLD_DONE.store(true, Release);
    }

    fn die_task(_: usize) {
        let g = DIE.lock();
        DIE_TID.store(here_tid(), Release);
        core::mem::forget(g); // the task ends HOLDING it: a death the guard's Drop never sees
        DIE_DONE.store(true, Release);
    }

    fn wait_flag(f: &AtomicBool, ms: u64) -> bool {
        let t0 = crate::arch::ms();
        while !f.load(Acquire) {
            if crate::arch::ms().saturating_sub(t0) > ms {
                return false;
            }
            crate::arch::sched::sleep_ms(5);
        }
        true
    }

    /// `tests lockreg`: (1) a scratch task holds a scratch lock 60 ms and the 50 ms scan must name it;
    /// (2) a scratch task ends holding a lock and `release_task` must give it back; (3) the live registry's
    /// counts — classes, tracked/all acquires, the longest hold, the order inversions seen.
    fn selftest() {
        let on = ON.load(Acquire);
        let hold_name = HOLD.site() as *const Loc as *mut Loc;
        TEST_NAME.store(hold_name, Release);
        SEEN_LONG.store(core::ptr::null_mut(), Release);
        HOLD_DONE.store(false, Release);
        crate::arch::sched::spawn("lockreg-hold", hold_task, 0, crate::arch::sched::CPU_AUTO, crate::arch::sched::PRIO_NORMAL);
        let held_done = wait_flag(&HOLD_DONE, 1000);
        // The scan runs every 50 ms: a 60 ms hold is seen at least once while live.
        let named = SEEN_LONG.load(Acquire) == hold_name;
        DIE_DONE.store(false, Release);
        crate::arch::sched::spawn("lockreg-die", die_task, 0, crate::arch::sched::CPU_AUTO, crate::arch::sched::PRIO_NORMAL);
        let died = wait_flag(&DIE_DONE, 1000);
        crate::arch::sched::sleep_ms(10);
        let tid = DIE_TID.load(Acquire);
        let freed = if died && tid != 0 { release_task(tid, "tests lockreg") } else { 0 };
        let back = DIE.try_lock().is_some();
        let released = died && freed == 1 && back;
        let (mut acq, mut tracked, mut held_max) = (EARLY.load(Relaxed), 0u64, 0u64);
        for p in PARTS.iter() {
            acq += p.acq.load(Relaxed);
            tracked += p.tracked.load(Relaxed);
            held_max = held_max.max(p.held_max.load(Relaxed));
        }
        let hz = crate::arch::apic::tsc_hz().max(1000);
        let inv = INV_N.load(Acquire);
        let pass = on && held_done && named && released && inv == 0;
        serial_println!(
            ":: LOCKREG: locks={} wrapped={}/{} held_max_ms={} inversions={} released_on_death={} named={} -> {} ::",
            CLASS_N.load(Relaxed),
            tracked,
            acq,
            held_max / (hz / 1000),
            inv,
            if released { "ok" } else if !died { "no-task" } else if freed != 1 { "not-found" } else { "still-held" },
            if named { "ok" } else if !on { "unarmed" } else if !held_done { "no-task" } else { "missed" },
            if pass { "PASS" } else { "FAIL" }
        );
    }
}
