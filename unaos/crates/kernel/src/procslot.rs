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
    /// aarch64 keys rows by ASID, and ASID 0 IS the shared context: row 0 is then the inline `shared`
    /// element (a static, never a heap row — the shell's and the kernel tasks' row exists from boot).
    asid_keyed: bool,
}

// SAFETY: as `SegVec` — elements are only shared as `&T`.
unsafe impl<T: Sync + Send> Sync for SlotVec<T> {}

impl<T: 'static> SlotVec<T> {
    /// `init` builds a slot row's zero state; `shared` is the shared row's.
    pub const fn new(init: fn() -> T, shared: T) -> SlotVec<T> {
        SlotVec { rows: SegVec::new(init), shared, asid_keyed: false }
    }

    /// An ASID-keyed sidecar (aarch64): row 0 — the shared/boot context — is the inline `shared` element.
    pub const fn new_asid(init: fn() -> T, shared: T) -> SlotVec<T> {
        SlotVec { rows: SegVec::new(init), shared, asid_keyed: true }
    }

    #[inline]
    fn is_shared(&self, i: usize) -> bool {
        i == SHARED_ROW || (self.asid_keyed && i == 0)
    }

    /// Allocate slot `s`'s segment now (process context), so an ISR's later `X[s]` never allocates.
    #[inline]
    pub fn warm(&self, s: usize) {
        if s <= ROW_ID_MAX && !self.is_shared(s) {
            let _ = self.rows.get(s);
        }
    }

    /// Row `i` if it exists — never allocates.
    #[inline]
    pub fn peek(&self, i: usize) -> Option<&T> {
        if self.is_shared(i) {
            Some(&self.shared)
        } else if i <= ROW_ID_MAX {
            self.rows.peek(i)
        } else {
            None
        }
    }

    /// Row `i` if it exists — the `peek` name's twin for callers written against the `get` idiom
    /// (`arch/x86_64/syscall.rs` present_pace, feature `vsyncpace`); never allocates.
    #[inline]
    pub fn get(&self, i: usize) -> Option<&T> {
        self.peek(i)
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
        if self.is_shared(i) {
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
        if self.asid_keyed { ROW_ID_MAX + 1 } else { SHARED_ROW + 1 }
    }
}

// APPLAUNCH (rmbp-ledger B678, BP-bar-7, BP-ring3-14) — TAIL-APPENDED (no line above moves; knob-off the module is
// `#[cfg]`-erased). The launch-time sidecar: one row per slot carrying the EXEC stamp (TSC cycles, taken by the x86
// launcher before `load_program_common` maps the ELF into the slot), the program's name, and the measured exec→frame
// span once the slot's FIRST `wm::present` has taken the stamp. Writers: `exec_stamp` (process context, before the
// task exists — so a row is warm before any present can read it); readers: `take` (the present path, `peek` only,
// never allocates) and `frame_ms` (`tests applaunch`). The stamp is TAKEN (`swap(0)`), so one exec prints one line.
#[cfg(all(target_arch = "x86_64", feature = "wc"))]
pub mod launch {
    use super::{SlotVec, SLOT_ID_MAX};
    use core::sync::atomic::{AtomicU64, Ordering};

    /// Bytes of the program name kept per slot (two words; a longer name is cut, never refused).
    pub const NAME_MAX: usize = 16;

    pub struct Row {
        /// Exec stamp in TSC cycles; 0 = no launch pending a first frame.
        exec: AtomicU64,
        /// The measured exec→frame span in ms, plus one; 0 = no frame yet for the current exec.
        frame: AtomicU64,
        name: [AtomicU64; 2],
    }

    const fn row_zero() -> Row {
        Row { exec: AtomicU64::new(0), frame: AtomicU64::new(0), name: [AtomicU64::new(0), AtomicU64::new(0)] }
    }

    fn row_init() -> Row {
        row_zero()
    }

    static ROWS: SlotVec<Row> = SlotVec::new(row_init, row_zero());

    /// The launcher's stamp: slot `slot` was exec'd at cycle `t0` as `name`. Called before the task is spawned.
    pub fn exec_stamp(slot: usize, name: &str, t0: u64) {
        if slot >= SLOT_ID_MAX {
            return;
        }
        let r = &ROWS[slot];
        let mut b = [0u8; NAME_MAX];
        let n = name.len().min(NAME_MAX);
        b[..n].copy_from_slice(&name.as_bytes()[..n]);
        r.frame.store(0, Ordering::Relaxed);
        r.name[0].store(u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]]), Ordering::Relaxed);
        r.name[1].store(u64::from_le_bytes([b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]]), Ordering::Relaxed);
        r.exec.store(t0.max(1), Ordering::Release);
    }

    /// The first present's take: `Some((name, len, ms))` exactly once per exec of `slot`, `None` otherwise.
    pub fn take(slot: usize) -> Option<([u8; NAME_MAX], usize, u64)> {
        if slot >= SLOT_ID_MAX {
            return None;
        }
        let r = ROWS.peek(slot)?;
        let t0 = r.exec.swap(0, Ordering::AcqRel);
        if t0 == 0 {
            return None;
        }
        let ms = crate::video::strip::cycles_to_us(crate::arch::now_cycles().saturating_sub(t0)) / 1000;
        let mut b = [0u8; NAME_MAX];
        b[..8].copy_from_slice(&r.name[0].load(Ordering::Relaxed).to_le_bytes());
        b[8..].copy_from_slice(&r.name[1].load(Ordering::Relaxed).to_le_bytes());
        let len = b.iter().position(|&c| c == 0).unwrap_or(NAME_MAX);
        r.frame.store(ms + 1, Ordering::Release);
        Some((b, len, ms))
    }

    /// The exec→frame span of `slot`'s current exec, once its first present has landed.
    pub fn frame_ms(slot: usize) -> Option<u64> {
        if slot >= SLOT_ID_MAX {
            return None;
        }
        match ROWS.peek(slot)?.frame.load(Ordering::Acquire) {
            0 => None,
            v => Some(v - 1),
        }
    }
}
