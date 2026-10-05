// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — wm
//!
//! WINDOWCAP-2 (rmbp-ledger B378, R90 "no hardcoding!!!") — GROWABLE PER-ROW STORAGE FOR THE WINDOW TABLE.
//!
//! `video::wm` used to keep each row's side state (pace stamps, shadow buffers, title source, native bit,
//! generation, witness counters …) in `[_; MAX_WINDOWS]` statics and `u32` slot masks, which made the
//! table's width a compile-time constant. [`SegVec`] replaces every one of them: a lock-free, append-only
//! segmented vector whose segments are allocated on first touch and never move, so a `&T` handed out for
//! slot `i` stays valid for the life of the kernel and readers need no lock.
//!
//! Segment `k` holds `BASE << k` elements, so [`SEGS`] segments cover more slots than a `WinId` (u32) can
//! name — the bound is the id's TYPE, not a policy. A slot nobody has written costs nothing: [`SegVec::peek`]
//! answers `None` (read as the zero state) without allocating, [`SegVec::get`] allocates the segment
//! (one heap allocation per doubling, never on a steady-state path).

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::sync::atomic::{AtomicPtr, AtomicUsize, Ordering};

/// Elements in segment 0.
const BASE: usize = 16;
/// Segments: `BASE * (2^SEGS - 1)` exceeds `u32::MAX`, i.e. every `WinId` has a home.
pub const SEGS: usize = 29;
const _: () = assert!((BASE as u128) * ((1u128 << SEGS) - 1) > u32::MAX as u128);

/// An append-only, never-moving, segment-doubling vector of `T`.
pub struct SegVec<T: 'static> {
    segs: [AtomicPtr<T>; SEGS],
    init: fn() -> T,
    /// One past the highest index ever [`get`](SegVec::get)-touched — the bound every "all slots" scan uses.
    hwm: AtomicUsize,
}

// SAFETY: elements are only shared as `&T`; `T: Sync` makes that sound across cores. Segments are
// installed by CAS and never freed or moved while the vector lives (it lives in a `static`).
unsafe impl<T: Sync + Send> Sync for SegVec<T> {}

#[inline]
fn locate(i: usize) -> (usize, usize) {
    let q = i / BASE + 1;
    let k = (usize::BITS - 1 - q.leading_zeros()) as usize;
    (k, i - BASE * ((1usize << k) - 1))
}

impl<T: 'static> SegVec<T> {
    /// `init` builds one element's zero state.
    pub const fn new(init: fn() -> T) -> SegVec<T> {
        SegVec { segs: [const { AtomicPtr::new(core::ptr::null_mut()) }; SEGS], init, hwm: AtomicUsize::new(0) }
    }

    /// Element `i` if its segment exists — never allocates (safe from an ISR or under the heap lock).
    #[inline]
    pub fn peek(&self, i: usize) -> Option<&T> {
        let (k, off) = locate(i);
        if k >= SEGS {
            return None;
        }
        let p = self.segs[k].load(Ordering::Acquire);
        if p.is_null() {
            return None;
        }
        // SAFETY: segment `k` holds `BASE << k` initialised elements and `off < BASE << k`.
        Some(unsafe { &*p.add(off) })
    }

    /// Element `i`, allocating its segment on first touch. Panics only past every `WinId` (index ≥ the
    /// u32 range), which no caller can name.
    pub fn get(&self, i: usize) -> &T {
        let (k, off) = locate(i);
        assert!(k < SEGS, "SegVec index past the WinId range");
        let mut p = self.segs[k].load(Ordering::Acquire);
        if p.is_null() {
            let n = BASE << k;
            let mut v: Vec<T> = Vec::with_capacity(n);
            for _ in 0..n {
                v.push((self.init)());
            }
            let raw = Box::into_raw(v.into_boxed_slice()) as *mut T;
            match self.segs[k].compare_exchange(core::ptr::null_mut(), raw, Ordering::AcqRel, Ordering::Acquire) {
                Ok(_) => p = raw,
                Err(won) => {
                    // SAFETY: `raw` came from `Box<[T]>` of length `n` above and was never published.
                    unsafe { drop(Box::from_raw(core::ptr::slice_from_raw_parts_mut(raw, n))) };
                    p = won;
                }
            }
        }
        self.hwm.fetch_max(i + 1, Ordering::AcqRel);
        // SAFETY: as in `peek`.
        unsafe { &*p.add(off) }
    }

    /// One past the highest index ever touched by [`get`](SegVec::get).
    #[inline]
    pub fn hwm(&self) -> usize {
        self.hwm.load(Ordering::Acquire)
    }
}

/// A growable bitset keyed by slot — what the `u32` slot masks became. Lock-free per bit.
pub struct SlotBits(SegVec<core::sync::atomic::AtomicBool>);

impl SlotBits {
    pub const fn new() -> SlotBits {
        SlotBits(SegVec::new(|| core::sync::atomic::AtomicBool::new(false)))
    }
    #[inline]
    pub fn test(&self, slot: usize) -> bool {
        self.0.peek(slot).is_some_and(|b| b.load(Ordering::Acquire))
    }
    #[inline]
    pub fn set(&self, slot: usize) -> bool {
        self.0.get(slot).swap(true, Ordering::AcqRel)
    }
    #[inline]
    pub fn clear(&self, slot: usize) -> bool {
        match self.0.peek(slot) {
            Some(b) => b.swap(false, Ordering::AcqRel),
            None => false,
        }
    }
    /// Any bit set — a scan to the high-water mark.
    pub fn any(&self) -> bool {
        (0..self.0.hwm()).any(|s| self.test(s))
    }
    /// How many bits are set.
    pub fn count(&self) -> usize {
        (0..self.0.hwm()).filter(|&s| self.test(s)).count()
    }
    #[inline]
    pub fn hwm(&self) -> usize {
        self.0.hwm()
    }
    /// Clear every bit, returning how many were set.
    pub fn take_count(&self) -> usize {
        (0..self.0.hwm()).filter(|&s| self.clear(s)).count()
    }
}

impl Default for SlotBits {
    fn default() -> Self {
        Self::new()
    }
}
