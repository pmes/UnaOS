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

//! UNAFS-F1: crash-recovery — the fsck-scavenger and the dirty-mount replay
//! path.
//!
//! ## What crash recovery has to do here (and what it deliberately does not)
//!
//! The mutation engine (`fs.rs`, F2) is crash-**ordered**, not
//! crash-**atomic**: every `unlink` / `rename` / `remove_attribute` /
//! attribute-rewrite is sequenced so that no intermediate on-disk state ever
//! references a freed block. The only residue an ill-timed power cut can leave
//! is therefore bounded and structurally sound:
//!
//! * **Leaked blocks** — allocated in the bitmap but reachable from no inode
//!   (a rewrite that wrote new extents but crashed before/at the single-block
//!   inode swap that adopts them, or an op that unhooked an inode but crashed
//!   before freeing its blocks). Never a dangling reference; just space the
//!   allocator can't hand out.
//! * **Query-orphaned inodes** — the one cross-directory `rename` window where
//!   an inode is reachable by query (its catalog entries key on the inode id,
//!   which is untouched) but by no name (it left the source directory before
//!   joining the destination). Its blocks are not leaked in the dangling
//!   sense — the catalog references them — but the file has no path.
//!
//! Because the volume is never torn, recovery does not need — and this arc
//! does not add — a redo/undo journal. The write-ahead log (`wal.rs`) records
//! only the begin/end of each op (no before/after block images), so a classic
//! log replay is not even expressible in the current on-disk format; growing
//! that format is a separate, KAT-recutting arc. What F1 adds is the pass that
//! actually reclaims the residue the crash-ordered design was designed to be
//! reclaimable: a mark-and-sweep scavenger that walks the volume from its
//! roots, and a [`UnaFS::recover`] entry point that runs it on a dirty mount
//! and clears the journal's dirty flag afterwards.
//!
//! ## Honest boundary: reordering write-back caches
//!
//! Everything above assumes writes hit the medium in **program order**. unafs
//! issues no write barriers, so a reordering write-back cache CAN land a later
//! write before an earlier one and, on power-cut, dangle a pointer the ordered
//! design never would. The scavenger is best-effort under that failure mode:
//! it reclaims leaks and heals query-orphans on a program-order volume; it is
//! not a general fsck for arbitrary corruption (the on-disk parser is hardened
//! separately — see `tests/hostile_volume.rs`), and the write-barrier question
//! is future work.

use crate::catalog::deserialize_catalog;
use crate::fs::{FileSystemError, UnaFS};
use crate::inode::{ExtentList, FileKind};
use crate::storage::{BLOCK_SIZE, BlockDevice};
use alloc::collections::BTreeSet;
use alloc::vec::Vec;

/// What a scavenger pass found (and, in repair mode, healed).
///
/// A volume is [`clean`](FsckReport::is_clean) when it has no leaked blocks,
/// no query-orphaned inodes, and no dirty journal.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FsckReport {
    /// The journal carried an unterminated transaction at recovery time.
    /// Only set by [`UnaFS::recover`] (a bare [`UnaFS::fsck`] does not consult
    /// the journal).
    pub dirty_journal: bool,
    /// Blocks marked used in the bitmap across the volume span.
    pub blocks_in_use: u64,
    /// Distinct blocks reachable from the roots (system + name tree + catalog).
    pub reachable_blocks: u64,
    /// Blocks allocated but reachable from no inode — detected before any
    /// repair. A query-orphan's own blocks appear here too (they have no name
    /// path); [`orphan_inodes`](FsckReport::orphan_inodes) names the subset
    /// that also needs a catalog scrub.
    pub leaked_blocks: Vec<u64>,
    /// Inodes reachable by query (catalog) but by no name — the cross-directory
    /// `rename` crash window.
    pub orphan_inodes: Vec<u64>,
    /// Blocks returned to the free pool (repair mode only).
    pub reclaimed_blocks: u64,
    /// Catalog index entries scrubbed while healing orphans (repair mode only).
    pub scrubbed_catalog_entries: usize,
    /// Whether this pass was allowed to mutate the volume.
    pub repaired: bool,
}

impl FsckReport {
    /// True when the scan found nothing to reclaim, heal, or roll forward.
    pub fn is_clean(&self) -> bool {
        self.leaked_blocks.is_empty() && self.orphan_inodes.is_empty() && !self.dirty_journal
    }
}

/// The read-only result of a reachability walk.
struct Reachable {
    /// Every block reachable from a root.
    blocks: BTreeSet<u64>,
    /// Every inode reachable by name (plus the system inodes root + catalog).
    named: BTreeSet<u64>,
}

impl<D: BlockDevice> UnaFS<D> {
    /// Report whether the journal carries an unterminated transaction (a torn
    /// mutation from a crash). Read-only; does not repair.
    pub fn is_dirty(&mut self) -> Result<bool, FileSystemError> {
        Ok(self.journal.check_recovery(&mut self.device)?)
    }

    /// Mark-and-sweep scavenger.
    ///
    /// Walks the volume from its roots (superblock system blocks, the name
    /// tree rooted at the root inode, and the attribute catalog), then diffs
    /// the reachable-block set against the allocation bitmap. With
    /// `repair == false` it only reports; with `repair == true` it heals
    /// query-orphaned inodes (scrubbing their catalog entries, crash-ordered)
    /// and returns every leaked block to the free pool.
    ///
    /// Does not consult the journal — [`recover`](UnaFS::recover) is the
    /// dirty-mount entry point that also clears the dirty flag. Safe to run on
    /// a clean volume: it reclaims nothing and leaves free-space accounting
    /// untouched.
    pub fn fsck(&mut self, repair: bool) -> Result<FsckReport, FileSystemError> {
        let block_count = self.superblock.block_count;

        // Phase 0: read-only reachability + orphan detection.
        let reach = self.scavenge_walk()?;
        let catalog_ids = self.catalog_inode_ids()?;

        let orphan_inodes: Vec<u64> = catalog_ids
            .iter()
            .copied()
            .filter(|id| *id != 0 && *id < block_count && !reach.named.contains(id))
            .collect();

        let mut leaked_blocks = Vec::new();
        let mut blocks_in_use = 0u64;
        for block in 0..block_count {
            if self.bitmap.is_used(block) {
                blocks_in_use += 1;
                if !reach.blocks.contains(&block) {
                    leaked_blocks.push(block);
                }
            }
        }

        let mut report = FsckReport {
            dirty_journal: false,
            blocks_in_use,
            reachable_blocks: reach.blocks.len() as u64,
            leaked_blocks,
            orphan_inodes: orphan_inodes.clone(),
            reclaimed_blocks: 0,
            scrubbed_catalog_entries: 0,
            repaired: repair,
        };

        if !repair {
            return Ok(report);
        }

        // Phase 1: heal query-orphans. Scrub their catalog entries FIRST — via
        // the crash-ordered rewrite path — so that when their blocks are freed
        // below, no query reference is left dangling. (The rewrite reshapes the
        // catalog's own extents, which is exactly why phase 2 re-walks rather
        // than trusting the phase-0 leak list.)
        if !orphan_inodes.is_empty() {
            let orphan_set: BTreeSet<u64> = orphan_inodes.iter().copied().collect();
            report.scrubbed_catalog_entries = self.count_catalog_entries(&orphan_set)?;
            self.remove_catalog_entries(|e| orphan_set.contains(&e.inode_id))?;
        }

        // Phase 2: re-walk (the catalog scrub moved blocks around) and sweep
        // every still-leaked block back to the free pool.
        let reach2 = self.scavenge_walk()?;
        let mut reclaimed = 0u64;
        for block in 0..block_count {
            if self.bitmap.is_used(block) && !reach2.blocks.contains(&block) {
                self.free_block(block);
                reclaimed += 1;
            }
        }
        if reclaimed > 0 || report.scrubbed_catalog_entries > 0 {
            self.sync_metadata()?;
        }
        report.reclaimed_blocks = reclaimed;

        Ok(report)
    }

    /// Dirty-mount recovery: the host-side replay path.
    ///
    /// Detects whether the journal carries a torn transaction, runs the
    /// scavenger in repair mode to bring the volume to a consistent state
    /// (leaks reclaimed, query-orphans healed), and — if the journal was
    /// dirty — resets it, clearing the dirty flag so the next mount is clean.
    ///
    /// This is "replay" in the sense the crash-ordered format supports:
    /// reconciliation to a consistent state, not redo/undo of logged block
    /// images (which the format does not carry — see the module docs). Intended
    /// for host (`std`) mounts; the kernel's read-only mount must never call it
    /// (it would attempt writes the RO seam refuses).
    pub fn recover(&mut self) -> Result<FsckReport, FileSystemError> {
        let dirty = self.is_dirty()?;
        let mut report = self.fsck(true)?;
        report.dirty_journal = dirty;
        if dirty {
            self.journal.reset(&mut self.device)?;
        }
        Ok(report)
    }

    /// Add every block an extent list covers to `blocks`, bounded to the volume
    /// span. Mirrors `free_extents`' hostile-geometry discipline: `checked_add`
    /// on the block address, and the run stops at the volume edge so a corrupt
    /// `length` can neither wrap nor spin for 2^64 iterations.
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

    /// Walk the volume from its roots, collecting every reachable block and
    /// every name-reachable inode. Read-only. Cycle-guarded (a corrupt
    /// directory cannot spin the walk) and bounded (inode ids and extent runs
    /// are clamped to the volume span). Any parse failure aborts the whole walk
    /// with `Err`, so a caller in repair mode frees nothing on a walk it could
    /// not complete.
    fn scavenge_walk(&mut self) -> Result<Reachable, FileSystemError> {
        let block_count = self.superblock.block_count;
        let root = self.superblock.root_inode;
        let catalog = self.superblock.catalog_inode;
        let bitmap_start = self.superblock.bitmap_start;
        let bitmap_blocks = self.superblock.bitmap_blocks;
        let journal_start = self.superblock.journal_start;
        let journal_blocks = self.superblock.journal_blocks;

        let mut blocks = BTreeSet::new();
        let mut named = BTreeSet::new();

        // System blocks: superblock, journal region, bitmap region.
        blocks.insert(0u64);
        for i in 0..journal_blocks {
            if let Some(b) = journal_start.checked_add(i) {
                if b < block_count {
                    blocks.insert(b);
                }
            }
        }
        for i in 0..bitmap_blocks {
            if let Some(b) = bitmap_start.checked_add(i) {
                if b < block_count {
                    blocks.insert(b);
                }
            }
        }

        // Name tree: DFS from the root over directories.
        let mut stack = alloc::vec![root];
        while let Some(id) = stack.pop() {
            if id == 0 || id >= block_count {
                continue;
            }
            if !named.insert(id) {
                continue; // already visited — cycle guard
            }
            let inode = self.read_inode(id)?;
            blocks.insert(id);
            self.mark_extent_blocks(&inode.chunks, &mut blocks);
            for extents in inode.large_attributes.values() {
                self.mark_extent_blocks(extents, &mut blocks);
            }
            if inode.kind == FileKind::Directory {
                for entry in self.ls(id)? {
                    if entry.inode_id != 0 && entry.inode_id < block_count {
                        stack.push(entry.inode_id);
                    }
                }
            }
        }

        // The attribute catalog (a System inode, reachable only via the
        // superblock, never a directory child).
        if catalog != 0 && catalog < block_count && named.insert(catalog) {
            let inode = self.read_inode(catalog)?;
            blocks.insert(catalog);
            self.mark_extent_blocks(&inode.chunks, &mut blocks);
            for extents in inode.large_attributes.values() {
                self.mark_extent_blocks(extents, &mut blocks);
            }
        }

        Ok(Reachable { blocks, named })
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
