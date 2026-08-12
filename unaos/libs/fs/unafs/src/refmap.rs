// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! K8a: the REFCOUNT allocator — the in-RAM successor of the old free-space
//! bitmap. A block is free ⇔ it is reachable from no retained root, i.e. its
//! refcount is 0. K8a only ever has the live root in play, so counts are
//! 0/1 today; retained snapshot roots (K8b) raise them above 1 with no format
//! change — the on-disk map stores full u32 counts from day one.
//!
//! ## The copy-on-write discipline that lives HERE
//!
//! The map keeps TWO views:
//! * `current` — the refcounts as the in-flight transaction sees them;
//! * `frozen`  — the refcounts as of the LAST COMMITTED root.
//!
//! [`RefMap::allocate`] hands out a block only when BOTH views say 0. That is
//! the whole "never overwrite in place" guarantee: a block freed during the
//! current transaction (current == 0, frozen == 1) still belongs to the
//! committed on-disk tree, so reusing it before the root flip would corrupt
//! the tree a power cut must fall back to. [`RefMap::freeze`] (called after
//! the root-sector flip) retires the old tree and makes those blocks
//! allocatable.
//!
//! On disk the map persists CoW like everything else: fresh leaf blocks of
//! 1024 little-endian u32 counts + a fresh index per commit, reached from the
//! root record (`fs.rs` owns that serialization). The index is ONE block of
//! leaf pointers up to 512 leaves (volumes ≤ 2 GiB — the only shape v3/v4
//! ever wrote); past that (v5+) it is a two-level tree: one index-of-indexes
//! block of mid pointers, each mid block holding up to 512 leaf pointers
//! (cap: 512 × 512 × 1024 = 1 TiB).

use alloc::vec::Vec;

/// Refcount entries per 4096 B leaf block (u32 counts).
pub const REFS_PER_LEAF: u64 = crate::storage::BLOCK_SIZE / 4;

/// The refcount map: `current` vs `frozen` (see module docs).
pub struct RefMap {
    current: Vec<u32>,
    frozen: Vec<u32>,
    block_count: u64,
    /// First-fit SCAN CURSOR — a pure in-RAM accelerator with the invariant
    /// "every index below `hint` is non-allocatable (used in `current` OR
    /// `frozen`)". [`allocate`](Self::allocate) scans from here instead of 0,
    /// which turns a sequential volume fill from O(n²) into O(n) — without it
    /// a 2 GiB fill is ~1.4e11 scan steps. Allocation RESULTS are identical
    /// to the plain lowest-free-in-both-views scan: anything that can free a
    /// block below the cursor ([`decref`](Self::decref)) or change a view
    /// wholesale ([`freeze`](Self::freeze), [`thaw`](Self::thaw),
    /// [`set_counts`](Self::set_counts)) rewinds it, paying at most one
    /// re-scan. Never serialized — no format impact.
    hint: usize,
}

impl RefMap {
    /// A fresh, all-free map for `block_count` blocks (format path).
    pub fn new(block_count: u64) -> Self {
        Self {
            current: alloc::vec![0u32; block_count as usize],
            frozen: alloc::vec![0u32; block_count as usize],
            block_count,
            hint: 0,
        }
    }

    /// Adopt counts loaded from disk (mount path): both views start equal —
    /// the loaded map IS the committed tree.
    pub fn from_counts(mut counts: Vec<u32>, block_count: u64) -> Self {
        counts.resize(block_count as usize, 0);
        Self {
            frozen: counts.clone(),
            current: counts,
            block_count,
            hint: 0,
        }
    }

    /// First-fit allocate: the lowest block free in BOTH views. Marks it
    /// current-refcount 1. `None` when the volume is full.
    pub fn allocate(&mut self) -> Option<u64> {
        for i in self.hint..self.block_count as usize {
            if self.current[i] == 0 && self.frozen[i] == 0 {
                self.current[i] = 1;
                self.hint = i + 1;
                return Some(i as u64);
            }
        }
        self.hint = self.block_count as usize;
        None
    }

    /// Increment a block's (current) refcount. Out-of-range ids are ignored
    /// (they have no entry — hostile extents free nothing real).
    pub fn incref(&mut self, block: u64) {
        if let Some(c) = self.current.get_mut(block as usize) {
            *c = c.saturating_add(1);
        }
    }

    /// Decrement a block's (current) refcount, saturating at 0.
    pub fn decref(&mut self, block: u64) {
        if let Some(c) = self.current.get_mut(block as usize) {
            *c = c.saturating_sub(1);
            // The block may now be allocatable (if its frozen count is 0
            // too): rewind the scan cursor so first-fit stays exact.
            self.hint = self.hint.min(block as usize);
        }
    }

    /// The block's current refcount (0 for out-of-range ids).
    pub fn refcount(&self, block: u64) -> u32 {
        self.current.get(block as usize).copied().unwrap_or(0)
    }

    /// Whether the block is in use in the CURRENT view.
    pub fn is_used(&self, block: u64) -> bool {
        self.refcount(block) > 0
    }

    /// Free blocks in the current view.
    pub fn free_blocks(&self) -> u64 {
        self.current.iter().filter(|&&c| c == 0).count() as u64
    }

    /// Retire the previously committed tree: `frozen = current`. Called
    /// immediately after the root-sector flip — from here on, blocks the
    /// transaction freed are genuinely reusable.
    pub fn freeze(&mut self) {
        self.frozen.copy_from_slice(&self.current);
        // Blocks the retired tree held (current 0 / frozen 1) just became
        // allocatable — rewind the scan cursor (one re-scan per commit).
        self.hint = 0;
    }

    /// Discard the in-flight transaction: `current = frozen` (the committed
    /// tree). NOTE: deliberately NOT used by the failed-transaction unwind —
    /// `UnaFS::txn_unwind` reloads from the committed on-disk root instead,
    /// because a thaw-based restore composed unsafely with residue earlier
    /// failed writes leave in the in-RAM imap (lens A finding, 2026-07-16).
    /// Kept as the in-RAM primitive it is; callers must know `frozen` is
    /// consistent with every OTHER piece of in-RAM state before using it.
    pub fn thaw(&mut self) {
        self.current.copy_from_slice(&self.frozen);
        self.hint = 0;
    }

    /// Number of leaf blocks the persisted map spans (a pure function of the
    /// volume geometry — constant for a given volume, which is what makes the
    /// "allocate the map's own blocks first, then serialize" commit ordering
    /// sound).
    pub fn leaf_count(&self) -> u64 {
        self.block_count.div_ceil(REFS_PER_LEAF)
    }

    /// Serialize leaf `idx` of the CURRENT view into a 4096 B block image.
    pub fn leaf_bytes(&self, idx: u64) -> Vec<u8> {
        let mut buf = alloc::vec![0u8; crate::storage::BLOCK_SIZE as usize];
        let base = (idx * REFS_PER_LEAF) as usize;
        for i in 0..REFS_PER_LEAF as usize {
            if let Some(&c) = self.current.get(base + i) {
                buf[i * 4..i * 4 + 4].copy_from_slice(&c.to_le_bytes());
            }
        }
        buf
    }

    /// Rebuild `current` (used by fsck repair): overwrite the current view
    /// with externally computed counts. `freeze` is NOT implied — the caller
    /// commits, which freezes.
    pub fn set_counts(&mut self, counts: &[u32]) {
        for (i, c) in self.current.iter_mut().enumerate() {
            *c = counts.get(i).copied().unwrap_or(0);
        }
        self.hint = 0;
    }
}
