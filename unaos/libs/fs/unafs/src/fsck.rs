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

//! K8a: the refcount-consistency checker (fsck) for the copy-on-write format.
//!
//! ## What crash recovery has to do here (and what it deliberately does not)
//!
//! Under CoW there is no torn-transaction concept to detect: every mutation
//! becomes visible in ONE atomic 512 B root-sector flip, so a power cut
//! yields the previous committed tree — whole, internally consistent, with
//! its own persisted refcount map. Blocks a crashed transaction wrote before
//! its flip are, by construction, blocks the committed refcount map calls
//! FREE, so they are reclaimed by the next allocation with no scavenging at
//! all. The pre-K8 dirty-journal/leak/query-orphan machinery is gone.
//!
//! What remains for fsck is INVARIANT CHECKING against media corruption or
//! implementation bugs: recompute the reachable-block set from the live root
//! (and, from K8b on, every retained root) and compare it with the persisted
//! refcount map. `repair == true` rewrites the map to the computed truth and
//! scrubs catalog entries that reference unallocated inode ids. Safe to run
//! on a clean volume: it finds nothing and (in repair mode) commits nothing
//! beyond a no-op root advance.
//!
//! ## Load-bearing invariant: liveness == name-reachability
//!
//! Repair's "never eat live data" guarantee rests on every live inode being
//! reachable through the name tree or a reserved system id (root, catalog,
//! snapshot index, reclaim queue); the catalog is a purely secondary index.
//! K8b's retained roots ADD roots to this walk — that arc must extend
//! [`UnaFS::fsck`]'s root set before it lands (standing STOP-class note).

use crate::catalog::deserialize_catalog;
use crate::fs::{FileSystemError, UnaFS};
use crate::root::ROOT_BLOCK;
use crate::storage::{BLOCK_SIZE, BlockDevice};
use alloc::collections::{BTreeMap, BTreeSet};
use alloc::vec::Vec;

/// What a consistency pass found (and, in repair mode, healed).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FsckReport {
    /// Always `false` under CoW (kept for report-shape compatibility: there
    /// is no journal to be dirty).
    pub dirty_journal: bool,
    /// Blocks with refcount > 0 in the persisted/current map.
    pub blocks_in_use: u64,
    /// Distinct blocks reachable from the roots (system set + name tree).
    pub reachable_blocks: u64,
    /// Blocks counted in-use but reachable from no root (media corruption or
    /// an implementation bug — the CoW commit cannot produce these).
    pub leaked_blocks: Vec<u64>,
    /// Inode ids the catalog references whose inode-map slot is empty.
    pub orphan_inodes: Vec<u64>,
    /// Blocks returned to the free pool (repair mode only).
    pub reclaimed_blocks: u64,
    /// Catalog index entries scrubbed while healing orphans (repair only).
    pub scrubbed_catalog_entries: usize,
    /// Whether this pass was allowed to mutate the volume.
    pub repaired: bool,
}

impl FsckReport {
    /// True when the scan found nothing inconsistent.
    pub fn is_clean(&self) -> bool {
        self.leaked_blocks.is_empty() && self.orphan_inodes.is_empty() && !self.dirty_journal
    }
}

impl<D: BlockDevice> UnaFS<D> {
    /// CoW volumes are never "dirty": a mount always lands on a committed
    /// tree. Kept for API compatibility with pre-K8 callers.
    pub fn is_dirty(&mut self) -> Result<bool, FileSystemError> {
        Ok(false)
    }

    /// Refcount-consistency check.
    ///
    /// Recomputes the reachable-block set (system blocks + the trees hanging
    /// from every reserved and name-reachable inode) and diffs it against the
    /// refcount map. With `repair == false` it only reports; with
    /// `repair == true` it scrubs catalog entries referencing dead inode ids,
    /// rewrites the refcount map to the computed truth, and commits.
    ///
    /// Caveat (ledgered NOTE): the read-only scan checks PRESENCE, not
    /// MAGNITUDE — a block in-use *and* reachable reports clean even if its
    /// stored count disagrees with the multi-root truth; only `repair == true`
    /// (which rebuilds true per-root counts) corrects magnitude drift.
    pub fn fsck(&mut self, repair: bool) -> Result<FsckReport, FileSystemError> {
        let block_count = self.superblock.block_count;

        // Phase 0: read-only reachability + orphan detection.
        let reach = self.reachability_walk()?;
        let catalog_ids = self.catalog_inode_ids()?;

        let orphan_inodes: Vec<u64> = catalog_ids
            .iter()
            .copied()
            .filter(|&id| self.imap_ref().get(id as usize).copied().unwrap_or(0) == 0)
            .collect();

        let mut leaked_blocks = Vec::new();
        let mut blocks_in_use = 0u64;
        for block in 0..block_count {
            if self.refmap_ref().is_used(block) {
                blocks_in_use += 1;
                if !reach.contains(&block) {
                    leaked_blocks.push(block);
                }
            }
        }

        let mut report = FsckReport {
            dirty_journal: false,
            blocks_in_use,
            reachable_blocks: reach.len() as u64,
            leaked_blocks,
            orphan_inodes: orphan_inodes.clone(),
            reclaimed_blocks: 0,
            scrubbed_catalog_entries: 0,
            repaired: repair,
        };

        if !repair {
            return Ok(report);
        }

        // Phase 1: scrub catalog entries pointing at dead inode ids (their
        // rewrite reshapes the catalog's blocks, hence the re-walk below).
        if !orphan_inodes.is_empty() {
            let orphan_set: BTreeSet<u64> = orphan_inodes.iter().copied().collect();
            report.scrubbed_catalog_entries = self.count_catalog_entries(&orphan_set)?;
            self.remove_catalog_entries(|e| orphan_set.contains(&e.inode_id))?;
        }

        // Phase 2: re-walk and rebuild the refcount map to the computed
        // truth. K8b: a block's true refcount is the NUMBER of referencing
        // roots — the live tree plus every retained snapshot that reaches it —
        // so the rebuild counts multiplicities, not a flat 1 (a flat rebuild
        // would under-count shared blocks and a later snapshot drop could then
        // free a block another root still lives on).
        let count_map = self.reachability_counts()?;
        let mut counts = alloc::vec![0u32; block_count as usize];
        for (&b, &n) in &count_map {
            if let Some(c) = counts.get_mut(b as usize) {
                *c = n;
            }
        }
        let mut reclaimed = 0u64;
        for block in 0..block_count {
            if self.refmap_ref().is_used(block) && !count_map.contains_key(&block) {
                reclaimed += 1;
            }
        }
        self.refmap_mut().set_counts(&counts);
        report.reclaimed_blocks = reclaimed;
        self.commit()?;

        Ok(report)
    }

    /// Consistency recovery entry point (host tools' `fsck --repair`). Under
    /// CoW there is no journal to replay or reset — this is `fsck(true)`.
    pub fn recover(&mut self) -> Result<FsckReport, FileSystemError> {
        self.fsck(true)
    }

    /// The set of every block reachable from ANY of the volume's roots (the
    /// live root and every retained snapshot). Used for leak detection: a
    /// block counted in-use but in no root's reach is a leak. Derived from
    /// [`reachability_counts`] (a block is reachable iff its count > 0).
    fn reachability_walk(&mut self) -> Result<BTreeSet<u64>, FileSystemError> {
        Ok(self.reachability_counts()?.into_keys().collect())
    }

    /// The TRUE refcount of every reachable block: how many roots reach it.
    /// The live tree contributes the static system blocks (superblock + root
    /// area), the committed inode-map and refcount-map blocks, and the trees
    /// hanging from the reserved inodes and the name tree; each retained
    /// snapshot (K8b) contributes the blocks its own root reaches through its
    /// inode map. A block shared by N roots is counted N times — this is the
    /// ground truth the persisted refcount map must match, and what repair
    /// rebuilds. Read-only; any parse failure aborts with `Err` so repair
    /// never acts on a partial walk.
    fn reachability_counts(&mut self) -> Result<BTreeMap<u64, u32>, FileSystemError> {
        let block_count = self.superblock.block_count;
        let mut counts: BTreeMap<u64, u32> = BTreeMap::new();

        // --- The live root ---
        Self::bump_block(0, block_count, &mut counts);
        Self::bump_block(ROOT_BLOCK, block_count, &mut counts);
        for b in self.map_blocks().collect::<Vec<_>>() {
            Self::bump_block(b, block_count, &mut counts);
        }
        // Every ALLOCATED inode-map slot is a reference: under CoW the imap
        // is the pointer graph's spine (root → imap → inode → extents), so a
        // file with no directory name — the `create_inode` shape host
        // embedders use — is still live. (Directory entries and the catalog
        // both resolve THROUGH the imap, so walking the slots covers every
        // name- and query-reachable inode too.)
        let ids: Vec<u64> = (1..self.imap_ref().len() as u64)
            .filter(|&id| self.imap_ref().get(id as usize).copied().unwrap_or(0) != 0)
            .collect();
        for id in ids {
            let pb = self.imap_ref()[id as usize];
            let inode = self.read_inode(id)?;
            Self::bump_block(pb, block_count, &mut counts);
            for e in &inode.chunks {
                Self::bump_extent(e, block_count, &mut counts);
            }
            for extents in inode.large_attributes.values() {
                for e in extents {
                    Self::bump_extent(e, block_count, &mut counts);
                }
            }
        }

        // --- Every retained snapshot root (K8b) ---
        // A snapshot holds one reference to each block its inode-map tree
        // reaches; `snapshot_blocks` enumerates exactly that set (the same set
        // `snapshot_create` increfed and `snapshot_drop` decrefs), so bumping
        // once per listed block reproduces the runtime refcounts precisely.
        for snap in self.snapshot_index()? {
            for b in self.snapshot_blocks(snap.imap_block, snap.imap_leaves)? {
                Self::bump_block(b, block_count, &mut counts);
            }
        }

        Ok(counts)
    }

    /// Bump one block's reachable-root count, bounded to the volume span.
    fn bump_block(block: u64, block_count: u64, counts: &mut BTreeMap<u64, u32>) {
        if block < block_count {
            *counts.entry(block).or_insert(0) += 1;
        }
    }

    /// Bump the count of every block a single extent covers (bounded to the
    /// volume span; a hostile extent marks nothing real).
    fn bump_extent(
        extent: &crate::inode::Extent,
        block_count: u64,
        counts: &mut BTreeMap<u64, u32>,
    ) {
        let n = extent.length.div_ceil(BLOCK_SIZE);
        for i in 0..n {
            match extent.physical_block.checked_add(i) {
                Some(block) if block < block_count => {
                    *counts.entry(block).or_insert(0) += 1;
                }
                _ => break,
            }
        }
    }

    /// The set of inode ids the attribute catalog references (deduplicated).
    fn catalog_inode_ids(&mut self) -> Result<BTreeSet<u64>, FileSystemError> {
        let catalog = self.superblock.catalog_inode;
        if catalog == 0 {
            return Ok(BTreeSet::new());
        }
        let inode = self.read_inode(catalog)?;
        if inode.size == 0 {
            return Ok(BTreeSet::new());
        }
        let data = self.read_data(catalog, 0, inode.size)?;
        let entries = deserialize_catalog(&data)?;
        Ok(entries.iter().map(|e| e.inode_id).collect())
    }

    /// Count catalog entries (with duplicates) whose inode is in `ids`.
    fn count_catalog_entries(&mut self, ids: &BTreeSet<u64>) -> Result<usize, FileSystemError> {
        let catalog = self.superblock.catalog_inode;
        if catalog == 0 {
            return Ok(0);
        }
        let inode = self.read_inode(catalog)?;
        if inode.size == 0 {
            return Ok(0);
        }
        let data = self.read_data(catalog, 0, inode.size)?;
        let entries = deserialize_catalog(&data)?;
        Ok(entries.iter().filter(|e| ids.contains(&e.inode_id)).count())
    }
}
