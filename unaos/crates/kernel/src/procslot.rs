// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Kernel — kernel-by-ruling (R90)
//!
//! WINDOWCAP3 (rmbp-ledger B399, R90 "no hardcoding!!!") — PER-ADDRESS-SPACE STORAGE WITH NO POOL WIDTH.
//!
//! Until this arc every per-process sidecar in `arch::*::syscall` (handles, files, input rings, focus and
//! pacing stamps, mailboxes …) was a `[_; USER_SLOTS]` / `[_; USER_SLOTS + 1]` static, and `USER_SLOTS`
//! (12 on x86, 8 on aarch64) was the `asids` term of the process limit on every flight's wire
//! (`[wm] limit windows=148 procs=10 from=mem:189,asids:10`). [`SlotVec`] replaces them: a
//! [`SegVec`](crate::video::rowstore::SegVec) keyed by slot (segments allocated on first touch, never
//! moved, so a `&T` stays valid and readers need no lock) plus the ONE shared row that used to sit at
//! index `USER_SLOTS`, now at the sentinel [`SHARED_ROW`].
//!
//! The bound on a slot index is its TYPE, never a policy (the WINDOWCAP-2 `WinId` rule): a ring-3 futex
//! key carries `slot + 1` in its tag byte with bit 63 reserved for the kernel's input keys
//! (`sched::futex_key`, `syscall::input_futex_key`), so a slot is `0..SLOT_ID_MAX`. The live process
//! limit is memory's (`video::wincap`), far below it.
//!
//! ISR rule: [`SlotVec::warm`] is called for a slot when the pool claims it (process context), so every
//! later `X[s]` of a claimed slot finds its segment and never allocates; [`SlotVec::peek`] never allocates.

use crate::video::rowstore::SegVec;

/// One past the highest slot index the futex key's tag byte can carry (`(slot + 1) << 56`, bit 63 the
/// kernel's): slots are `0..SLOT_ID_MAX`. A TYPE bound, like `wm::WIN_ID_SENTINEL_FLOOR`.
pub const SLOT_ID_MAX: usize = 127;

/// Highest ROW a `SlotVec` stores: x86 keys rows by slot (`0..SLOT_ID_MAX`), aarch64 by ASID (`slot + 1`,
/// ASID 0 the shared context), so rows are `0..=SLOT_ID_MAX`.
pub const ROW_ID_MAX: usize = SLOT_ID_MAX;

/// The x86 shared row (kernel tasks with no private address space) — was index `USER_SLOTS`. Past every
/// row, so it is never a slot nor an ASID; a `SlotVec` keeps its element apart.
pub const SHARED_ROW: usize = ROW_ID_MAX + 1;

/// A per-slot sidecar: one `T` per address-space slot, plus the shared row's `T`.
pub struct SlotVec<T: 'static> {
    rows: SegVec<T>,
    shared: T,
}

// SAFETY: as `SegVec` — elements are only shared as `&T`.
unsafe impl<T: Sync + Send> Sync for SlotVec<T> {}

impl<T: 'static> SlotVec<T> {
    /// `init` builds a slot row's zero state; `shared` is the shared row's.
    pub const fn new(init: fn() -> T, shared: T) -> SlotVec<T> {
        SlotVec { rows: SegVec::new(init), shared }
    }

    /// Allocate slot `s`'s segment now (process context), so an ISR's later `X[s]` never allocates.
    #[inline]
    pub fn warm(&self, s: usize) {
        if s <= ROW_ID_MAX {
            let _ = self.rows.get(s);
        }
    }

    /// Row `i` if it exists — never allocates.
    #[inline]
    pub fn peek(&self, i: usize) -> Option<&T> {
        if i == SHARED_ROW {
            Some(&self.shared)
        } else if i <= ROW_ID_MAX {
            self.rows.peek(i)
        } else {
            None
        }
    }

    /// One past the highest slot row ever touched (the shared row excluded) — every "all slots" scan's bound.
    #[inline]
    pub fn hwm(&self) -> usize {
        self.rows.hwm()
    }
}

impl<T: 'static> core::ops::Index<usize> for SlotVec<T> {
    type Output = T;
    /// Row `i` (the shared row at [`SHARED_ROW`]). Panics past the slot TYPE (`ROW_ID_MAX`), which no
    /// validated caller names — every ring-3-derived index is range-checked against the live pool first.
    #[inline]
    fn index(&self, i: usize) -> &T {
        if i == SHARED_ROW {
            return &self.shared;
        }
        assert!(i <= ROW_ID_MAX, "SlotVec index past the slot type");
        self.rows.get(i)
    }
}

impl<T: 'static> SlotVec<T> {
    /// The row-index bound every `row < X.len()` range check reads (the `[_; USER_SLOTS + 1]` arrays'
    /// `len()`): every slot/ASID row and the shared row. Not an allocation — rows exist on first touch.
    #[inline]
    pub const fn len(&self) -> usize {
        SHARED_ROW + 1
    }
}
