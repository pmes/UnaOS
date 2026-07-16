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
use crate::inode::ExtentList;
use crate::root::ROOT_BLOCK;
use crate::storage::{BLOCK_SIZE, BlockDevice};
use alloc::collections::BTreeSet;
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
        // truth (K8a: every reachable block has exactly one referencing
        // root, so count == 1; K8b's multi-root walk raises counts here).
        let reach2 = self.reachability_walk()?;
        let mut counts = alloc::vec![0u32; block_count as usize];
        for &b in &reach2 {
            if let Some(c) = counts.get_mut(b as usize) {
                *c = 1;
            }
        }
        let mut reclaimed = 0u64;
        for block in 0..block_count {
            if self.refmap_ref().is_used(block) && !reach2.contains(&block) {
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

    /// Add every block an extent list covers to `blocks`, bounded to the
    /// volume span (hostile geometry marks nothing real).
    fn mark_extent_blocks(&self, extents: &ExtentList, blocks: &mut BTreeSet<u64>) {
        let volume = self.superblock.block_count;
        for extent in extents {
            let count = extent.length.div_ceil(BLOCK_SIZE);
            for i in 0..count {
                match extent.physical_block.checked_add(i) {
                    Some(block) if block < volume => {
                        blocks.insert(block);
                    }
                    _ => break,
                }
            }
        }
    }

    /// Every block reachable from the volume's roots: the static system
    /// blocks (superblock + root area), the committed inode-map and
    /// refcount-map blocks, and the trees hanging from the reserved inodes
    /// and the name tree. Read-only; cycle-guarded; any parse failure aborts
    /// with `Err` so repair mode never acts on a partial walk.
    fn reachability_walk(&mut self) -> Result<BTreeSet<u64>, FileSystemError> {
        let block_count = self.superblock.block_count;

        let mut blocks: BTreeSet<u64> = BTreeSet::new();
        blocks.insert(0);
        blocks.insert(ROOT_BLOCK);
        for b in self.map_blocks().collect::<Vec<_>>() {
            if b < block_count {
                blocks.insert(b);
            }
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
            blocks.insert(pb);
            self.mark_extent_blocks(&inode.chunks, &mut blocks);
            for extents in inode.large_attributes.values() {
                self.mark_extent_blocks(extents, &mut blocks);
            }
        }

        Ok(blocks)
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
