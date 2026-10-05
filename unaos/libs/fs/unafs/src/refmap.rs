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

//! K8a / UNAFSMAP (B354): the REFCOUNT allocator — the successor of the old
//! free-space bitmap. A block is free ⇔ it is reachable from no retained
//! root, i.e. its refcount is 0. Counts are full u32s on disk (retained
//! snapshot roots raise them above 1).
//!
//! ## Paged, not held whole
//!
//! The map is a [`Tree`](crate::maptree::Tree) of 4096 B leaves of 1024 u32
//! counts. RAM holds the tree's pointers (16 B per leaf, 4 MiB at 1 TiB),
//! a 2 B "busy" count per leaf, and an LRU of leaf PAGES bounded by a budget
//! ([`DEFAULT_CACHE_BYTES`]); a leaf is read from disk when an operation
//! touches it. Pages a transaction modified are pinned until the commit.
//! Allocation is first-fit over the per-leaf busy counts, so a full leaf is
//! skipped without reading it.
//!
//! ## The copy-on-write discipline that lives HERE
//!
//! Every count has TWO views:
//! * `current` — as the in-flight transaction sees it (the page);
//! * `frozen`  — as of the LAST COMMITTED root (a copy of the page taken the
//!   first time the transaction modifies it; a clean page's frozen view IS its
//!   current view).
//!
//! [`RefMap::allocate`] hands out a block only when BOTH views say 0. That is
//! the whole "never overwrite in place" guarantee: a block freed during the
//! current transaction (current == 0, frozen == 1) still belongs to the
//! committed on-disk tree, so reusing it before the root flip would corrupt
//! the tree a power cut must fall back to. [`RefMap::freeze`] (called after
//! the root-sector flip) retires the old tree and makes those blocks
//! allocatable.
//!
//! On disk the map persists CoW like everything else: the commit writes the
//! dirty leaves and their index paths to fresh blocks
//! ([`relocate_own`](RefMap::relocate_own) + [`write_own`](RefMap::write_own));
//! clean leaves keep their blocks. A v3–v6 volume's map is the legacy shape
//! (raw pointer index, every leaf a block) and keeps it until the v6 → v7
//! migration (or forever, on v3–v5).

use crate::fs::FileSystemError;
use crate::maptree::{Shape, Tree, block_sum};
use crate::storage::{BLOCK_SIZE, BlockDevice, Error as StorageError};
use alloc::collections::BTreeMap;
use alloc::vec::Vec;

/// Refcount entries per 4096 B leaf block (u32 counts).
pub const REFS_PER_LEAF: u64 = crate::storage::BLOCK_SIZE / 4;
const RPL: usize = REFS_PER_LEAF as usize;

/// Default leaf-page cache budget: 4 MiB (1024 pages). Pinned (dirty) pages
/// may exceed it for the length of one transaction.
pub const DEFAULT_CACHE_BYTES: usize = 4 * 1024 * 1024;

/// Cache counters (the bench and the kernel witness read these).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MapStats {
    /// Leaf pages read from disk since mount/format.
    pub page_reads: u64,
    /// Pages resident now (clean + pinned; a pinned page's frozen copy counts).
    pub pages_resident: u64,
    /// Highest `pages_resident` seen.
    pub pages_peak: u64,
    /// The budget, in pages.
    pub budget_pages: u64,
    /// Leaves in the map, and how many of them own a block.
    pub leaves: u64,
    pub leaf_blocks: u64,
}

struct Page {
    cur: Vec<u32>,
    frozen: Option<Vec<u32>>,
    tick: u64,
}

fn try_page() -> Result<Vec<u32>, StorageError> {
    let mut v: Vec<u32> = Vec::new();
    v.try_reserve_exact(RPL)
        .map_err(|_| StorageError::AllocRefused(BLOCK_SIZE))?;
    v.resize(RPL, 0);
    Ok(v)
}

/// The refcount map (see module docs).
pub struct RefMap {
    tree: Tree,
    /// Per leaf: in-range entries with current ≠ 0 OR frozen ≠ 0 (the
    /// non-allocatable count; equals the leaf's `used` when it is clean).
    busy: Vec<u16>,
    pages: BTreeMap<u64, Page>,
    /// tick → leaf, for CLEAN resident pages only (eviction order).
    lru: BTreeMap<u64, u64>,
    tick: u64,
    frozen_pages: usize,
    budget_pages: usize,
    block_count: u64,
    /// First-fit SCAN CURSOR — every block below it is non-allocatable.
    /// Pure accelerator, never serialized.
    hint: u64,
    /// UNAFSGROW: the allocation CEILING (below the map's size only inside a
    /// grow). Never serialized.
    limit: u64,
    /// In-range entries with current ≠ 0.
    used_total: u64,
    /// The first error a page load hit (I/O, checksum, allocation). Sticky:
    /// the commit refuses (and the caller reloads from disk) while it is set.
    fault: Option<FileSystemError>,
    /// The failed-unwind fail-closed state: nothing is allocatable.
    poisoned: bool,
    stats: MapStats,
}

impl RefMap {
    fn from_tree(tree: Tree, block_count: u64) -> Result<Self, StorageError> {
        let n = tree.leaf_count();
        let mut busy: Vec<u16> = Vec::new();
        busy.try_reserve_exact(n as usize)
            .map_err(|_| StorageError::AllocRefused(n * 2))?;
        busy.extend(tree.leaves.iter().map(|p| p.used.min(u16::MAX as u32) as u16));
        let used_total = tree.used_total();
        Ok(Self {
            tree,
            busy,
            pages: BTreeMap::new(),
            lru: BTreeMap::new(),
            tick: 0,
            frozen_pages: 0,
            budget_pages: DEFAULT_CACHE_BYTES / BLOCK_SIZE as usize,
            block_count,
            hint: 0,
            limit: block_count,
            used_total,
            fault: None,
            poisoned: false,
            stats: MapStats::default(),
        })
    }

    /// A fresh, all-free map for `block_count` blocks (format path). Paged:
    /// every leaf a hole. Legacy: every leaf marked dirty so the format commit
    /// gives each a block (the pre-v7 layout has no holes). FALLIBLE.
    pub fn try_new(block_count: u64, shape: Shape) -> Result<Self, StorageError> {
        let leaves = block_count.div_ceil(REFS_PER_LEAF);
        let mut m = Self::from_tree(Tree::empty(shape, leaves)?, block_count)?;
        if shape == Shape::Legacy {
            for l in 0..leaves {
                m.mark_dirty_zero(l)?;
            }
        }
        Ok(m)
    }

    /// Mount path: adopt a tree loaded from disk. A LEGACY tree carries no
    /// sums or counts, so every leaf is read once here to compute them (the
    /// same full read pre-v7 mounts always did; the pages are not kept).
    pub fn load<D: BlockDevice>(
        device: &mut D,
        mut tree: Tree,
        block_count: u64,
    ) -> Result<Self, FileSystemError> {
        if tree.shape == Shape::Legacy {
            let mut buf = alloc::vec![0u8; BLOCK_SIZE as usize];
            for l in 0..tree.leaf_count() {
                let b = tree.leaves[l as usize].block;
                device.read_block(b, &mut buf)?;
                let base = l * REFS_PER_LEAF;
                let in_range = block_count.saturating_sub(base).min(REFS_PER_LEAF) as usize;
                let mut used = 0u32;
                for i in 0..in_range {
                    if u32::from_le_bytes(buf[i * 4..i * 4 + 4].try_into().unwrap()) != 0 {
                        used += 1;
                    }
                }
                tree.leaves[l as usize].used = used;
                tree.leaves[l as usize].sum = block_sum(&buf);
            }
        }
        Ok(Self::from_tree(tree, block_count)?)
    }

    /// The on-disk shape this map commits in.
    pub fn shape(&self) -> Shape {
        self.tree.shape
    }

    /// The map's tree (pointers only).
    pub fn tree(&self) -> &Tree {
        &self.tree
    }

    /// v6 → v7: re-shape as paged, releasing the legacy index blocks.
    pub fn convert_to_paged<D: BlockDevice>(&mut self, device: &mut D) -> Result<(), FileSystemError> {
        for b in self.tree.convert_to_paged()? {
            self.decref(device, b);
        }
        self.take_fault()
    }

    /// Set the page budget (bytes; at least 16 pages).
    pub fn set_budget(&mut self, bytes: usize) {
        self.budget_pages = (bytes / BLOCK_SIZE as usize).max(16);
        self.evict();
    }

    /// Cache counters.
    pub fn stats(&self) -> MapStats {
        let mut s = self.stats;
        s.pages_resident = (self.pages.len() + self.frozen_pages) as u64;
        s.budget_pages = self.budget_pages as u64;
        s.leaves = self.tree.leaf_count();
        s.leaf_blocks = self.tree.leaves.iter().filter(|p| p.block != 0).count() as u64;
        s
    }

    /// The sticky load error, if any (cleared: the caller reloads).
    pub fn take_fault(&mut self) -> Result<(), FileSystemError> {
        match self.fault.take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    /// Whether a load error is pending.
    pub fn faulted(&self) -> bool {
        self.fault.is_some()
    }

    fn set_fault(&mut self, e: FileSystemError) {
        if self.fault.is_none() {
            self.fault = Some(e);
        }
    }

    fn in_range(&self, leaf: u64) -> usize {
        self.block_count
            .saturating_sub(leaf * REFS_PER_LEAF)
            .min(REFS_PER_LEAF) as usize
    }

    fn evict(&mut self) {
        while self.pages.len() + self.frozen_pages > self.budget_pages {
            let Some((&t, &leaf)) = self.lru.iter().next() else { break };
            self.lru.remove(&t);
            self.pages.remove(&leaf);
        }
    }

    fn note_peak(&mut self) {
        let r = (self.pages.len() + self.frozen_pages) as u64;
        if r > self.stats.pages_peak {
            self.stats.pages_peak = r;
        }
    }

    /// Make `leaf`'s page resident (reading and verifying it if needed).
    /// `false` (with the fault recorded) when it cannot be.
    fn ensure<D: BlockDevice>(&mut self, device: &mut D, leaf: u64) -> bool {
        if self.fault.is_some() || leaf >= self.tree.leaf_count() {
            return false;
        }
        self.tick += 1;
        let tick = self.tick;
        let pinned = self.tree.dirty.contains(&leaf);
        if let Some(p) = self.pages.get_mut(&leaf) {
            if !pinned {
                self.lru.remove(&p.tick);
                self.lru.insert(tick, leaf);
            }
            p.tick = tick;
            return true;
        }
        let meta = self.tree.leaves[leaf as usize];
        let mut cur = match try_page() {
            Ok(v) => v,
            Err(e) => {
                self.set_fault(e.into());
                return false;
            }
        };
        if meta.block != 0 {
            let mut buf = alloc::vec![0u8; BLOCK_SIZE as usize];
            if let Err(e) = device.read_block(meta.block, &mut buf) {
                self.set_fault(e.into());
                return false;
            }
            self.stats.page_reads += 1;
            if block_sum(&buf) != meta.sum {
                self.set_fault(FileSystemError::CorruptVolume("refmap leaf checksum mismatch"));
                return false;
            }
            let in_range = self.in_range(leaf);
            let mut used = 0u32;
            for (i, c) in cur.iter_mut().enumerate() {
                *c = u32::from_le_bytes(buf[i * 4..i * 4 + 4].try_into().unwrap());
                if i < in_range && *c != 0 {
                    used += 1;
                }
            }
            if used != meta.used {
                self.set_fault(FileSystemError::CorruptVolume("refmap leaf count mismatch"));
                return false;
            }
        }
        self.evict();
        self.pages.insert(leaf, Page { cur, frozen: None, tick });
        self.lru.insert(tick, leaf);
        self.note_peak();
        true
    }

    /// First modification of a resident page in this transaction: keep its
    /// frozen copy, pin it, mark the leaf dirty.
    fn begin_write(&mut self, leaf: u64) -> bool {
        let Some(p) = self.pages.get_mut(&leaf) else { return false };
        if p.frozen.is_none() {
            let mut f: Vec<u32> = Vec::new();
            if f.try_reserve_exact(RPL).is_err() {
                self.set_fault(StorageError::AllocRefused(BLOCK_SIZE).into());
                return false;
            }
            f.extend_from_slice(&p.cur);
            p.frozen = Some(f);
            self.lru.remove(&p.tick);
            self.frozen_pages += 1;
            self.tree.dirty.insert(leaf);
            self.note_peak();
        }
        true
    }

    /// Pin a leaf as dirty with no content change (legacy materialization).
    fn mark_dirty_zero(&mut self, leaf: u64) -> Result<(), StorageError> {
        let cur = try_page()?;
        self.tick += 1;
        self.pages.insert(leaf, Page { cur, frozen: None, tick: self.tick });
        self.lru.insert(self.tick, leaf);
        if !self.begin_write(leaf) {
            return Err(StorageError::AllocRefused(BLOCK_SIZE));
        }
        Ok(())
    }

    /// Set entry `i` of resident, write-begun `leaf` to `new`, keeping the
    /// leaf's used/busy and the map's used total exact.
    fn set_entry(&mut self, leaf: u64, i: usize, new: u32) {
        let in_range = i < self.in_range(leaf);
        let p = self.pages.get_mut(&leaf).expect("resident page");
        let old = p.cur[i];
        let fz = p.frozen.as_ref().map(|f| f[i]).unwrap_or(old);
        p.cur[i] = new;
        if !in_range {
            return;
        }
        let (was_used, now_used) = (old != 0, new != 0);
        let (was_busy, now_busy) = (was_used || fz != 0, now_used || fz != 0);
        let meta = &mut self.tree.leaves[leaf as usize];
        if was_used != now_used {
            if now_used {
                meta.used += 1;
                self.used_total += 1;
            } else {
                meta.used -= 1;
                self.used_total -= 1;
            }
        }
        if was_busy != now_busy {
            let b = &mut self.busy[leaf as usize];
            if now_busy {
                *b += 1;
            } else {
                *b -= 1;
            }
        }
    }

    /// First-fit allocate: the lowest block (below the ceiling) free in BOTH
    /// views. Marks it current-refcount 1. `None` when the volume is full —
    /// or when a leaf cannot be read (the fault is recorded; the commit then
    /// refuses).
    pub fn allocate<D: BlockDevice>(&mut self, device: &mut D) -> Option<u64> {
        if self.poisoned || self.fault.is_some() {
            return None;
        }
        let end = self.limit.min(self.block_count);
        let mut b = self.hint;
        while b < end {
            let leaf = b / REFS_PER_LEAF;
            let base = leaf * REFS_PER_LEAF;
            if self.busy[leaf as usize] as usize >= self.in_range(leaf) {
                b = base + REFS_PER_LEAF;
                continue;
            }
            if !self.ensure(device, leaf) {
                return None;
            }
            let stop = (end - base).min(REFS_PER_LEAF) as usize;
            let found = {
                let p = &self.pages[&leaf];
                let mut hit = None;
                for i in (b - base) as usize..stop {
                    let fz = p.frozen.as_ref().map(|f| f[i]).unwrap_or(p.cur[i]);
                    if p.cur[i] == 0 && fz == 0 {
                        hit = Some(i);
                        break;
                    }
                }
                hit
            };
            if let Some(i) = found {
                if !self.begin_write(leaf) {
                    return None;
                }
                self.set_entry(leaf, i, 1);
                self.hint = base + i as u64 + 1;
                return Some(base + i as u64);
            }
            b = base + REFS_PER_LEAF;
        }
        self.hint = self.hint.max(end);
        None
    }

    /// Increment a block's (current) refcount. Out-of-range ids are ignored
    /// (they have no entry — hostile extents free nothing real).
    pub fn incref<D: BlockDevice>(&mut self, device: &mut D, block: u64) {
        if block >= self.block_count {
            return;
        }
        let leaf = block / REFS_PER_LEAF;
        let i = (block % REFS_PER_LEAF) as usize;
        if !self.ensure(device, leaf) || !self.begin_write(leaf) {
            return;
        }
        let c = self.pages[&leaf].cur[i];
        self.set_entry(leaf, i, c.saturating_add(1));
    }

    /// Decrement a block's (current) refcount, saturating at 0.
    pub fn decref<D: BlockDevice>(&mut self, device: &mut D, block: u64) {
        if block >= self.block_count {
            return;
        }
        let leaf = block / REFS_PER_LEAF;
        let i = (block % REFS_PER_LEAF) as usize;
        if !self.ensure(device, leaf) {
            return;
        }
        let c = self.pages[&leaf].cur[i];
        if c == 0 || !self.begin_write(leaf) {
            return;
        }
        self.set_entry(leaf, i, c - 1);
        // The block may now be allocatable: rewind the cursor.
        self.hint = self.hint.min(block);
    }

    /// The block's current refcount (0 for out-of-range ids or an unreadable
    /// leaf — the fault is recorded).
    pub fn refcount<D: BlockDevice>(&mut self, device: &mut D, block: u64) -> u32 {
        if block >= self.block_count {
            return 0;
        }
        let leaf = block / REFS_PER_LEAF;
        if !self.ensure(device, leaf) {
            return 0;
        }
        self.pages[&leaf].cur[(block % REFS_PER_LEAF) as usize]
    }

    /// The current view of one leaf's in-range counts (fsck walks leaves). A
    /// hole leaf with no resident page answers without a read.
    pub fn leaf_counts<D: BlockDevice>(
        &mut self,
        device: &mut D,
        leaf: u64,
    ) -> Result<Vec<u32>, FileSystemError> {
        let n = self.in_range(leaf);
        if self.tree.leaves[leaf as usize].block == 0 && !self.pages.contains_key(&leaf) {
            return Ok(alloc::vec![0u32; n]);
        }
        if !self.ensure(device, leaf) {
            self.take_fault()?;
            return Err(FileSystemError::CorruptVolume("refmap leaf unreadable"));
        }
        Ok(self.pages[&leaf].cur[..n].to_vec())
    }

    /// Leaves in the map (a pure function of the geometry, or of a pending
    /// grow's larger size).
    pub fn leaf_count(&self) -> u64 {
        self.tree.leaf_count()
    }

    /// Whether `leaf` has any in-use entry in the current view.
    pub fn leaf_used(&self, leaf: u64) -> u32 {
        self.tree.leaves[leaf as usize].used
    }

    /// Free blocks in the current view.
    pub fn free_blocks(&self) -> u64 {
        self.block_count.saturating_sub(self.used_total)
    }

    /// Phase A of the map's own commit: give every dirty leaf (and every
    /// index node on a dirty path) a fresh block — or a hole, for a paged leaf
    /// left empty — releasing the block it held, until no allocation dirties
    /// a node that has not moved yet. Every other map (the inode map) must
    /// have relocated before this runs: after it, nothing may allocate.
    pub fn relocate_own<D: BlockDevice>(&mut self, device: &mut D) -> Result<(), FileSystemError> {
        loop {
            let pend = self.tree.pending();
            if pend.is_empty() {
                break;
            }
            for (level, idx) in pend {
                let nb = if self.tree.wants_block(level, idx) {
                    match self.allocate(device) {
                        Some(b) => b,
                        None => {
                            self.take_fault()?;
                            return Err(FileSystemError::NoSpace);
                        }
                    }
                } else {
                    0
                };
                let old = self.tree.relocate(level, idx, nb);
                if old != 0 {
                    self.decref(device, old);
                }
            }
            self.take_fault()?;
        }
        Ok(())
    }

    /// Phase B: write the relocated leaves (their final counts) and index
    /// nodes. `write` puts one fresh block.
    pub fn write_own(
        &mut self,
        write: &mut dyn FnMut(u64, &[u8]) -> Result<(), FileSystemError>,
    ) -> Result<(), FileSystemError> {
        let mut buf = alloc::vec![0u8; BLOCK_SIZE as usize];
        for (leaf, block) in self.tree.moved_leaves() {
            let p = self
                .pages
                .get(&leaf)
                .ok_or(FileSystemError::CorruptVolume("relocated refmap leaf not resident"))?;
            for (i, c) in p.cur.iter().enumerate() {
                buf[i * 4..i * 4 + 4].copy_from_slice(&c.to_le_bytes());
            }
            write(block, &buf)?;
            self.tree.set_leaf_sum(leaf, block_sum(&buf));
        }
        self.tree.write_upper(write)
    }

    /// The top node's block (the root record's `refmap_block`).
    pub fn top(&self) -> u64 {
        self.tree.top()
    }

    /// Retire the previously committed tree: `frozen = current`. Called
    /// immediately after the root-sector flip — from here on, blocks the
    /// transaction freed are genuinely reusable.
    pub fn freeze(&mut self) {
        let dirty: Vec<u64> = self.tree.dirty.iter().copied().collect();
        let mut low = self.hint;
        // A leaf modified but never written (a freeze with no commit — the
        // B+tree store tests do this) must stay pinned: its counts exist
        // nowhere else. It stays dirty, so the next commit writes it.
        let mut unwritten = Vec::new();
        for leaf in dirty {
            let written = self.tree.is_moved(0, leaf);
            if let Some(p) = self.pages.get_mut(&leaf) {
                if p.frozen.take().is_some() {
                    self.frozen_pages -= 1;
                }
                if written {
                    self.lru.insert(p.tick, leaf);
                }
            }
            if !written {
                unwritten.push(leaf);
            }
            let u = self.tree.leaves[leaf as usize].used;
            self.busy[leaf as usize] = u as u16;
            low = low.min(leaf * REFS_PER_LEAF);
        }
        self.tree.finish_commit();
        self.tree.dirty.extend(unwritten);
        self.hint = low;
        self.evict();
    }

    /// Discard the in-flight transaction: `current = frozen` for every pinned
    /// page. NOTE: deliberately NOT used by the failed-transaction unwind —
    /// `UnaFS::txn_unwind` reloads from the committed on-disk root instead.
    pub fn thaw(&mut self) {
        let dirty: Vec<u64> = self.tree.dirty.iter().copied().collect();
        for leaf in dirty {
            let in_range = self.in_range(leaf);
            if let Some(p) = self.pages.get_mut(&leaf) {
                if let Some(f) = p.frozen.take() {
                    p.cur = f;
                    self.frozen_pages -= 1;
                }
                self.lru.insert(p.tick, leaf);
                let used = p.cur[..in_range].iter().filter(|&&c| c != 0).count() as u32;
                self.tree.leaves[leaf as usize].used = used;
                self.busy[leaf as usize] = used as u16;
            }
        }
        self.tree.finish_commit();
        self.used_total = self.tree.used_total();
        self.hint = 0;
    }

    /// fsck repair: make the current view equal `truth` (block → count; every
    /// block absent is 0). Leaf by leaf; only leaves that differ are touched
    /// (and pinned). Returns how many in-use blocks became free. `freeze` is
    /// NOT implied — the caller commits.
    pub fn rebuild_from<D: BlockDevice>(
        &mut self,
        device: &mut D,
        truth: &BTreeMap<u64, u32>,
    ) -> Result<u64, FileSystemError> {
        let mut reclaimed = 0u64;
        for leaf in 0..self.tree.leaf_count() {
            let base = leaf * REFS_PER_LEAF;
            let mut want = [0u32; RPL];
            let mut any = false;
            for (&b, &c) in truth.range(base..base + REFS_PER_LEAF) {
                if b < self.block_count {
                    want[(b - base) as usize] = c;
                    any = any || c != 0;
                }
            }
            if !any && self.tree.leaves[leaf as usize].used == 0 && !self.pages.contains_key(&leaf) {
                continue;
            }
            if !self.ensure(device, leaf) {
                self.take_fault()?;
            }
            let differs = self.pages[&leaf].cur[..] != want[..];
            if !differs {
                continue;
            }
            if !self.begin_write(leaf) {
                self.take_fault()?;
            }
            for (i, &w) in want.iter().enumerate() {
                let c = self.pages[&leaf].cur[i];
                if c != w {
                    if c != 0 && w == 0 && i < self.in_range(leaf) {
                        reclaimed += 1;
                    }
                    self.set_entry(leaf, i, w);
                }
            }
        }
        self.hint = 0;
        Ok(reclaimed)
    }

    /// Fail closed (a failed unwind): nothing is allocatable until remount.
    pub fn poison(&mut self) {
        self.poisoned = true;
    }

    /// UNAFSGROW: extend the map to `new_block_count` blocks (the new blocks
    /// free) and hold the allocation ceiling at `limit`. Paged: the new leaves
    /// are holes. Legacy: each new leaf is pinned dirty so the commit gives it
    /// a block. A smaller size is refused (`AllocRefused(0)`).
    pub fn try_grow(&mut self, new_block_count: u64, limit: u64) -> Result<(), StorageError> {
        if new_block_count < self.block_count {
            return Err(StorageError::AllocRefused(0));
        }
        let old_leaves = self.tree.leaf_count();
        let leaves = new_block_count.div_ceil(REFS_PER_LEAF);
        self.tree.resize(leaves)?;
        let n = self.tree.leaf_count() as usize;
        self.busy
            .try_reserve_exact(n.saturating_sub(self.busy.len()))
            .map_err(|_| StorageError::AllocRefused(n as u64 * 2))?;
        self.busy.resize(n, 0);
        if self.tree.shape == Shape::Legacy {
            for l in old_leaves..self.tree.leaf_count() {
                self.mark_dirty_zero(l)?;
            }
        }
        self.block_count = new_block_count;
        self.limit = limit.min(new_block_count);
        Ok(())
    }

    /// UNAFSGROW: move the allocation ceiling (clamped to the map's size).
    pub fn set_limit(&mut self, limit: u64) {
        self.limit = limit.min(self.block_count);
    }

    /// The number of blocks the map covers.
    pub fn block_count(&self) -> u64 {
        self.block_count
    }
}
