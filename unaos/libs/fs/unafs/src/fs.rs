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

//! K8a: the COPY-ON-WRITE UnaFS core.
//!
//! Every mutation allocates fresh blocks — nothing reachable from the last
//! committed root is EVER overwritten in place. Each public mutating
//! operation is one transaction: CoW the affected data/metadata to fresh
//! blocks, then [`commit`](UnaFS::commit) — persist the inode map and the
//! refcount map (themselves CoW), barrier, and flip ONE 512 B root sector
//! (A/B generation-stamped slots, `root.rs`). A power cut at any point yields
//! the old tree or the new tree, never a hybrid; the WAL is gone — atomicity
//! is structural, not logged.
//!
//! * **Inode identity (Fork-1 verdict):** inode ids are STABLE LOGICAL
//!   numbers; the inode map (`imap`, a CoW'd on-disk object) carries
//!   id → current physical block. Directory entries, catalog entries, and
//!   the kernel's K6 ACL keys survive every mutation unchanged.
//! * **Allocation:** a refcount map ([`crate::refmap::RefMap`]) — free ⇔
//!   reachable from no retained root. Its two-view (`current`/`frozen`)
//!   allocate discipline is what enforces never-overwrite-in-place.
//! * **Future shape, day one:** the root reaches a SNAPSHOT INDEX and a
//!   persistent RECLAIM QUEUE, both ordinary UnaFS objects (`System` inodes
//!   at reserved logical ids). v1 policy: snapshot cap 16 ([`SNAPSHOT_CAP`]),
//!   reclaim drains eagerly (a non-empty queue found at mount is drained
//!   before the mount returns — a half-drained queue is crash-safe because
//!   the drain itself is one commit).

use crate::catalog::{CatalogEntry, deserialize_catalog, serialize_catalog};
use crate::inode::{AttributeValue, Extent, ExtentList, FileKind, Inode, InodeError};
use crate::refmap::RefMap;
use crate::root::{ROOT_BLOCK, RootRecord, RootSlot};
use crate::storage::{BLOCK_SIZE, BlockDevice, Error as StorageError};
use crate::superblock::{
    CATALOG_INODE_ID, RECLAIM_INODE_ID, ROOT_INODE_ID, SNAP_INDEX_INODE_ID, Superblock,
    SuperblockError,
};
use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::catalog::hash_value;
use crate::hash::hash_bytes;
use crate::query::{Query, QueryOp};
#[cfg(feature = "std")]
use bandy::{BandyMember, SMessage};

/// Inode-map entries (u64 physical-block pointers) per 4096 B leaf block.
pub const IMAP_ENTRIES_PER_LEAF: u64 = BLOCK_SIZE / 8;
/// Leaf pointers per index block — bounds the map at one indirect level.
pub const IMAP_MAX_LEAVES: u64 = BLOCK_SIZE / 8;

/// v1 snapshot-retention POLICY cap (the structure is unbounded: the index is
/// an ordinary growable UnaFS object; lifting the cap is a constant change).
pub const SNAPSHOT_CAP: usize = 16;

#[derive(Error, Debug)]
pub enum FileSystemError {
    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("Superblock error: {0}")]
    Superblock(#[from] SuperblockError),
    #[error("Inode error: {0}")]
    Inode(#[from] InodeError),
    #[error("Serialization error: {0}")]
    Serialization(#[from] crate::codec::CodecError),
    #[error("No free space available")]
    NoSpace,
    #[error("Root inode missing")]
    RootMissing,
    #[error("Not a directory")]
    NotADirectory,
    #[error("File already exists")]
    FileExists,
    #[error("Attribute too large for inline storage")]
    AttributeTooLarge,
    #[error("Invalid Attribute Data")]
    InvalidAttributeData,
    #[error("Query error: {0}")]
    Query(String),
    #[error("Entry not found")]
    NotFound,
    #[error("Is a directory")]
    IsADirectory,
    #[error("Attribute not found")]
    AttributeNotFound,
    #[error("Cannot move a directory into itself or its descendants")]
    DirectoryLoop,
    #[error("Corrupt volume: {0}")]
    CorruptVolume(&'static str),
}

/// A directory entry pointing to an inode (by stable LOGICAL id).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, PartialOrd)]
pub struct DirEntry {
    pub name: String,
    pub inode_id: u64,
    pub kind: FileKind,
}

/// One retained root in the snapshot index (K8b populates these; the on-disk
/// object exists — empty — from format time, so retention is a code change,
/// never a format migration).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct SnapshotEntry {
    /// The commit generation this snapshot retains.
    pub generation: u64,
    /// The retained root's inode-map index block + leaf count (everything a
    /// two-root diff walk needs).
    pub imap_block: u64,
    pub imap_leaves: u64,
    /// K6 typed attributes ride the entry: name, creator principal, timestamp.
    pub name: String,
    pub creator: String,
    pub timestamp: u64,
}

/// One dropped root awaiting reclamation. Drop NEVER frees blocks directly —
/// it enqueues; the drain decrefs. v1 drains eagerly (to empty before the
/// dropping call returns / before a mount completes); background mode later
/// is the same queue drained by a worker. Crash-safe: the whole drain is one
/// commit, so a power cut mid-drain resumes from the full queue on next mount.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ReclaimEntry {
    /// The dropped root's generation (provenance).
    pub generation: u64,
    /// The blocks the dropped root held the last reference to.
    pub blocks: Vec<u64>,
}

/// Commit-path benchmark counters (vaire ruling: the numbers must exist).
/// The kernel witness prints these next to a CNTPCT tick delta; the host
/// bench reads them directly.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommitStats {
    /// Root flips since mount/format.
    pub commits: u64,
    /// Total blocks written through the CoW path (data + metadata + maps).
    pub blocks_written: u64,
    /// Blocks written by the most recent transaction (including its commit).
    pub last_commit_blocks: u64,
}

pub struct UnaFS<D: BlockDevice> {
    pub device: D,
    pub superblock: Superblock,
    /// The refcount allocator (current vs frozen views — `refmap.rs`).
    refmap: RefMap,
    /// Logical inode id → current physical block (0 = unallocated id).
    imap: Vec<u64>,
    /// The last COMMITTED root record and the slot holding it.
    root: RootRecord,
    active_slot: RootSlot,
    /// Blocks (index + leaves) holding the last committed inode map /
    /// refcount map — decref'd when the next commit writes fresh ones.
    imap_blocks: Vec<u64>,
    refmap_blocks: Vec<u64>,
    /// Commit automatically at the end of each public mutating op (default).
    /// The crash-simulation seam (`set_autocommit(false)`) leaves fresh
    /// blocks written but the root un-flipped — exactly a power cut before
    /// the atomic point.
    autocommit: bool,
    stats: CommitStats,
    /// Blocks written by the in-flight transaction.
    txn_blocks: u64,
}

impl<D: BlockDevice> UnaFS<D> {
    // =====================================================================
    // Lifecycle
    // =====================================================================

    /// Format the device with a new (K8, version 3) UnaFS filesystem.
    pub fn format(mut device: D, size_mb: u64) -> Result<Self, FileSystemError> {
        let blocks_from_size = (size_mb * 1024 * 1024) / BLOCK_SIZE;
        let mut block_count = device.block_count();
        if block_count == 0 {
            block_count = blocks_from_size;
        }

        let superblock = Superblock::new(block_count);
        superblock.validate()?;

        // Static identity: block 0, written once.
        let sb_bytes = superblock.to_bytes()?;
        let mut sb_block = alloc::vec![0u8; BLOCK_SIZE as usize];
        sb_block[..sb_bytes.len()].copy_from_slice(&sb_bytes);
        device.write_block(0, &sb_block)?;

        // Zero the root area so neither slot parses as valid until the format
        // commit writes generation 1.
        let zero = alloc::vec![0u8; BLOCK_SIZE as usize];
        device.write_block(ROOT_BLOCK, &zero)?;

        let mut refmap = RefMap::new(block_count);
        // Pin the static blocks: superblock + root area.
        refmap.incref(0);
        refmap.incref(ROOT_BLOCK);

        let mut fs = Self {
            device,
            superblock,
            refmap,
            imap: alloc::vec![0u64; 1], // id 0 reserved/invalid
            root: RootRecord {
                generation: 0,
                imap_block: 0,
                imap_leaves: 0,
                next_inode: 1,
                refmap_block: 0,
                refmap_leaves: 0,
                free_blocks: 0,
                flags: 0,
            },
            // Dummy: the format commit writes `other()` == slot A.
            active_slot: RootSlot::B,
            imap_blocks: Vec::new(),
            refmap_blocks: Vec::new(),
            autocommit: true,
            stats: CommitStats::default(),
            txn_blocks: 0,
        };

        // The reserved system objects, in id order (1..=4).
        let root_id = fs.create_inode_inner(FileKind::Directory, BTreeMap::new())?;
        debug_assert_eq!(root_id, ROOT_INODE_ID);
        let catalog_id = fs.create_inode_inner(FileKind::System, BTreeMap::new())?;
        debug_assert_eq!(catalog_id, CATALOG_INODE_ID);
        let snap_id = fs.create_inode_inner(FileKind::System, BTreeMap::new())?;
        debug_assert_eq!(snap_id, SNAP_INDEX_INODE_ID);
        let empty_snaps: Vec<SnapshotEntry> = Vec::new();
        let bytes = crate::codec::serialize(&empty_snaps)?;
        fs.rewrite_data_inner(snap_id, &bytes)?;
        let reclaim_id = fs.create_inode_inner(FileKind::System, BTreeMap::new())?;
        debug_assert_eq!(reclaim_id, RECLAIM_INODE_ID);
        let empty_queue: Vec<ReclaimEntry> = Vec::new();
        let bytes = crate::codec::serialize(&empty_queue)?;
        fs.rewrite_data_inner(reclaim_id, &bytes)?;

        // Generation 1: the format commit.
        fs.commit()?;
        Ok(fs)
    }

    /// Mount an existing UnaFS (K8) filesystem: read the static superblock,
    /// pick the newer valid root slot, load the inode map and refcount map
    /// the root points at, and drain any pending reclaim-queue entries.
    pub fn mount(mut device: D) -> Result<Self, FileSystemError> {
        let mut sb_block = alloc::vec![0u8; BLOCK_SIZE as usize];
        device.read_block(0, &mut sb_block)?;
        let superblock = Superblock::from_bytes(&sb_block)?;
        let block_count = superblock.block_count;

        let (root, active_slot) = crate::root::read_active(&mut device)?
            .ok_or(FileSystemError::CorruptVolume("no valid root record"))?;

        // ---- Inode map (bounded: the volume is untrusted input) ----
        if root.imap_leaves == 0 || root.imap_leaves > IMAP_MAX_LEAVES {
            return Err(FileSystemError::CorruptVolume(
                "imap leaf count out of range",
            ));
        }
        if root.next_inode == 0 || root.next_inode > root.imap_leaves * IMAP_ENTRIES_PER_LEAF {
            return Err(FileSystemError::CorruptVolume("next inode out of range"));
        }
        if root.imap_block == 0 || root.imap_block >= block_count {
            return Err(FileSystemError::CorruptVolume("imap index out of bounds"));
        }
        let mut imap_blocks = alloc::vec![root.imap_block];
        let mut index = alloc::vec![0u8; BLOCK_SIZE as usize];
        device.read_block(root.imap_block, &mut index)?;
        let imap_cap = usize::try_from(root.next_inode)
            .map_err(|_| StorageError::AllocRefused(root.next_inode))?;
        let mut imap: Vec<u64> = Vec::new();
        imap.try_reserve_exact(imap_cap)
            .map_err(|_| StorageError::AllocRefused(root.next_inode))?;
        let mut leaf = alloc::vec![0u8; BLOCK_SIZE as usize];
        for l in 0..root.imap_leaves {
            let ptr = u64::from_le_bytes(
                index[(l * 8) as usize..(l * 8 + 8) as usize]
                    .try_into()
                    .unwrap(),
            );
            if ptr == 0 || ptr >= block_count {
                return Err(FileSystemError::CorruptVolume("imap leaf out of bounds"));
            }
            imap_blocks.push(ptr);
            device.read_block(ptr, &mut leaf)?;
            for e in 0..IMAP_ENTRIES_PER_LEAF {
                if imap.len() as u64 >= root.next_inode {
                    break;
                }
                let entry = u64::from_le_bytes(
                    leaf[(e * 8) as usize..(e * 8 + 8) as usize]
                        .try_into()
                        .unwrap(),
                );
                if entry >= block_count {
                    return Err(FileSystemError::CorruptVolume("imap entry out of bounds"));
                }
                imap.push(entry);
            }
        }

        // ---- Refcount map ----
        let expected_leaves = block_count.div_ceil(crate::refmap::REFS_PER_LEAF);
        if root.refmap_leaves != expected_leaves {
            return Err(FileSystemError::CorruptVolume(
                "refmap size inconsistent with block count",
            ));
        }
        if root.refmap_block == 0 || root.refmap_block >= block_count {
            return Err(FileSystemError::CorruptVolume("refmap index out of bounds"));
        }
        if root.refmap_leaves > BLOCK_SIZE / 8 {
            return Err(FileSystemError::CorruptVolume("refmap leaf count too large"));
        }
        let mut refmap_blocks = alloc::vec![root.refmap_block];
        device.read_block(root.refmap_block, &mut index)?;
        let count_cap =
            usize::try_from(block_count).map_err(|_| StorageError::AllocRefused(block_count))?;
        let mut counts: Vec<u32> = Vec::new();
        counts
            .try_reserve_exact(count_cap)
            .map_err(|_| StorageError::AllocRefused(block_count))?;
        for l in 0..root.refmap_leaves {
            let ptr = u64::from_le_bytes(
                index[(l * 8) as usize..(l * 8 + 8) as usize]
                    .try_into()
                    .unwrap(),
            );
            if ptr == 0 || ptr >= block_count {
                return Err(FileSystemError::CorruptVolume("refmap leaf out of bounds"));
            }
            refmap_blocks.push(ptr);
            device.read_block(ptr, &mut leaf)?;
            for e in 0..crate::refmap::REFS_PER_LEAF {
                if counts.len() >= count_cap {
                    break;
                }
                let c = u32::from_le_bytes(
                    leaf[(e * 4) as usize..(e * 4 + 4) as usize]
                        .try_into()
                        .unwrap(),
                );
                counts.push(c);
            }
        }
        let refmap = RefMap::from_counts(counts, block_count);

        let mut fs = Self {
            device,
            superblock,
            refmap,
            imap,
            root,
            active_slot,
            imap_blocks,
            refmap_blocks,
            autocommit: true,
            stats: CommitStats::default(),
            txn_blocks: 0,
        };

        // Persistent reclaim queue: v1 drains eagerly on mount (crash-safe —
        // the drain is one commit; a power cut mid-drain resumes here).
        fs.reclaim_drain()?;

        Ok(fs)
    }

    // =====================================================================
    // Transaction core
    // =====================================================================

    /// Toggle per-op auto-commit. `false` is the crash-simulation seam:
    /// mutations write fresh blocks but the root never flips, so dropping
    /// the instance models a power cut mid-commit (the next mount sees the
    /// old tree, whole and valid).
    pub fn set_autocommit(&mut self, on: bool) {
        self.autocommit = on;
    }

    /// The last committed root generation.
    pub fn root_generation(&self) -> u64 {
        self.root.generation
    }

    /// Free blocks in the current (in-flight) view.
    pub fn free_blocks(&self) -> u64 {
        self.refmap.free_blocks()
    }

    /// Commit-path benchmark counters.
    pub fn commit_stats(&self) -> CommitStats {
        self.stats
    }

    /// Blocks (index + leaves) holding the last committed inode map and
    /// refcount map — part of the fsck walk's system set.
    pub(crate) fn map_blocks(&self) -> impl Iterator<Item = u64> + '_ {
        self.imap_blocks
            .iter()
            .chain(self.refmap_blocks.iter())
            .copied()
    }

    pub(crate) fn refmap_ref(&self) -> &RefMap {
        &self.refmap
    }

    pub(crate) fn refmap_mut(&mut self) -> &mut RefMap {
        &mut self.refmap
    }

    pub(crate) fn imap_ref(&self) -> &[u64] {
        &self.imap
    }

    pub(crate) fn imap_clear(&mut self, id: u64) {
        if let Some(e) = self.imap.get_mut(id as usize) {
            *e = 0;
        }
    }

    fn maybe_commit(&mut self) -> Result<(), FileSystemError> {
        if self.autocommit {
            self.commit()?;
        }
        Ok(())
    }

    /// COMMIT: persist the inode map and refcount map to fresh blocks
    /// (CoW, like everything else), barrier, then flip ONE 512 B root
    /// sector — the transaction's single atomic point.
    pub fn commit(&mut self) -> Result<(), FileSystemError> {
        // 1. Retire the previously committed maps (their blocks stay
        //    protected by the frozen view until after the flip).
        for b in core::mem::take(&mut self.imap_blocks) {
            self.refmap.decref(b);
        }
        for b in core::mem::take(&mut self.refmap_blocks) {
            self.refmap.decref(b);
        }

        // 2. Inode map → fresh leaves + fresh index block.
        let next_inode = self.imap.len() as u64;
        let imap_leaves = next_inode.div_ceil(IMAP_ENTRIES_PER_LEAF).max(1);
        if imap_leaves > IMAP_MAX_LEAVES {
            return Err(FileSystemError::NoSpace);
        }
        let mut new_imap_blocks = Vec::new();
        let index_block = self.alloc_block()?;
        let mut index = alloc::vec![0u8; BLOCK_SIZE as usize];
        for l in 0..imap_leaves {
            let leaf_block = self.alloc_block()?;
            index[(l * 8) as usize..(l * 8 + 8) as usize]
                .copy_from_slice(&leaf_block.to_le_bytes());
            let mut leaf = alloc::vec![0u8; BLOCK_SIZE as usize];
            let base = (l * IMAP_ENTRIES_PER_LEAF) as usize;
            for e in 0..IMAP_ENTRIES_PER_LEAF as usize {
                if let Some(&entry) = self.imap.get(base + e) {
                    leaf[e * 8..e * 8 + 8].copy_from_slice(&entry.to_le_bytes());
                }
            }
            self.write_fresh(leaf_block, &leaf)?;
            new_imap_blocks.push(leaf_block);
        }

        // 3. Refcount map: allocate ALL its blocks first (allocation mutates
        //    the counts being persisted), then serialize. The leaf count is a
        //    pure function of the volume geometry, so it cannot change under
        //    us mid-step.
        let refmap_leaves = self.refmap.leaf_count();
        let ref_index_block = self.alloc_block()?;
        let mut ref_leaf_blocks = Vec::with_capacity(refmap_leaves as usize);
        for _ in 0..refmap_leaves {
            ref_leaf_blocks.push(self.alloc_block()?);
        }
        // Every allocation is done: the counts are final. Serialize.
        let mut ref_index = alloc::vec![0u8; BLOCK_SIZE as usize];
        for (l, &leaf_block) in ref_leaf_blocks.iter().enumerate() {
            ref_index[l * 8..l * 8 + 8].copy_from_slice(&leaf_block.to_le_bytes());
            let leaf = self.refmap.leaf_bytes(l as u64);
            self.write_fresh(leaf_block, &leaf)?;
        }
        self.write_fresh(ref_index_block, &ref_index)?;
        // (Written last among the fresh blocks so everything lands before
        //  the barrier; order among fresh blocks is immaterial — none are
        //  reachable until the flip.)
        self.write_fresh(index_block, &index)?;

        let free_blocks = self.refmap.free_blocks();

        // 4. Barrier: every fresh block must be on the medium before the
        //    root flip makes them reachable.
        self.device.flush()?;

        // 5. The atomic point: ONE 512 B write to the INACTIVE slot.
        let new_root = RootRecord {
            generation: self.root.generation + 1,
            imap_block: index_block,
            imap_leaves,
            next_inode,
            refmap_block: ref_index_block,
            refmap_leaves,
            free_blocks,
            flags: 0,
        };
        let slot = self.active_slot.other();
        crate::root::write_slot(&mut self.device, slot, &new_root)?;
        self.device.flush()?;

        // 6. The new tree is the committed tree.
        self.root = new_root;
        self.active_slot = slot;
        let mut blocks = new_imap_blocks;
        blocks.insert(0, index_block);
        self.imap_blocks = blocks;
        let mut rblocks = ref_leaf_blocks;
        rblocks.insert(0, ref_index_block);
        self.refmap_blocks = rblocks;
        self.refmap.freeze();

        self.stats.commits += 1;
        self.stats.last_commit_blocks = self.txn_blocks;
        self.txn_blocks = 0;
        Ok(())
    }

    /// Historical name for "make everything durable" — under CoW that is a
    /// commit. Kept for API compatibility (host tools call it).
    pub fn sync_metadata(&mut self) -> Result<(), FileSystemError> {
        self.commit()
    }

    fn alloc_block(&mut self) -> Result<u64, FileSystemError> {
        self.refmap.allocate().ok_or(FileSystemError::NoSpace)
    }

    /// Write a freshly allocated block, counting it for the bench.
    fn write_fresh(&mut self, block: u64, buf: &[u8]) -> Result<(), FileSystemError> {
        self.device.write_block(block, buf)?;
        self.txn_blocks += 1;
        self.stats.blocks_written += 1;
        Ok(())
    }

    // =====================================================================
    // Inode primitives (logical ids through the inode map)
    // =====================================================================

    /// The CURRENT physical block an inode lives at (moves on every CoW
    /// write). `None` for unallocated ids. Exposed for consistency witnesses
    /// and corruption fixtures — never store it across a mutation.
    pub fn inode_block(&self, id: u64) -> Option<u64> {
        match self.imap.get(id as usize).copied().unwrap_or(0) {
            0 => None,
            pb => Some(pb),
        }
    }

    /// Read an Inode by LOGICAL id.
    pub fn read_inode(&mut self, id: u64) -> Result<Inode, FileSystemError> {
        let pb = self.imap.get(id as usize).copied().unwrap_or(0);
        if pb == 0 {
            return Err(FileSystemError::NotFound);
        }
        let mut block = alloc::vec![0u8; BLOCK_SIZE as usize];
        self.device.read_block(pb, &mut block)?;
        let inode = Inode::from_bytes(&block)?;
        if inode.id != id {
            return Err(FileSystemError::CorruptVolume("inode id mismatch"));
        }
        Ok(inode)
    }

    /// CoW-write an Inode: fresh block, remap, release the old block.
    fn write_inode(&mut self, inode: &Inode) -> Result<(), FileSystemError> {
        let bytes = inode.to_bytes()?;
        let mut block = alloc::vec![0u8; BLOCK_SIZE as usize];
        block[..bytes.len()].copy_from_slice(&bytes);
        let nb = self.alloc_block()?;
        self.write_fresh(nb, &block)?;
        let idx = inode.id as usize;
        if idx >= self.imap.len() {
            return Err(FileSystemError::CorruptVolume("inode id beyond map"));
        }
        let old = self.imap[idx];
        if old != 0 {
            self.refmap.decref(old);
        }
        self.imap[idx] = nb;
        Ok(())
    }

    /// Allocate the next logical inode id and CoW-write a fresh inode there.
    fn create_inode_inner(
        &mut self,
        kind: FileKind,
        attributes: BTreeMap<String, AttributeValue>,
    ) -> Result<u64, FileSystemError> {
        let id = self.imap.len() as u64;
        if (id + 1).div_ceil(IMAP_ENTRIES_PER_LEAF) > IMAP_MAX_LEAVES {
            return Err(FileSystemError::NoSpace);
        }
        self.imap.push(0);
        let mut inode = Inode::new(id, kind);
        inode.attributes = attributes;
        self.write_inode(&inode)?;
        Ok(id)
    }

    /// Create a bare File inode (public API; one transaction).
    pub fn create_inode(
        &mut self,
        attributes: BTreeMap<String, AttributeValue>,
    ) -> Result<u64, FileSystemError> {
        let id = self.create_inode_inner(FileKind::File, attributes)?;
        self.maybe_commit()?;
        Ok(id)
    }

    // =====================================================================
    // Data path (CoW)
    // =====================================================================

    /// The volume's byte span, per its own superblock — the bound every
    /// disk-derived size/offset must respect (BEFS-HARDEN).
    fn volume_bytes(&self) -> u64 {
        self.superblock.block_count.saturating_mul(BLOCK_SIZE)
    }

    /// Materialize a file's logical-block → physical-block map from its
    /// extent list (bounded against the volume span; hostile geometry errs).
    fn file_block_map(&self, inode: &Inode, blocks: u64) -> Result<Vec<u64>, FileSystemError> {
        let cap = usize::try_from(blocks).map_err(|_| StorageError::AllocRefused(blocks))?;
        if blocks > self.superblock.block_count {
            return Err(FileSystemError::CorruptVolume("file exceeds volume"));
        }
        let mut map = Vec::new();
        map.try_reserve_exact(cap)
            .map_err(|_| StorageError::AllocRefused(blocks))?;
        map.resize(cap, 0u64);
        for extent in &inode.chunks {
            let end = extent
                .logical_offset
                .checked_add(extent.length)
                .ok_or(FileSystemError::CorruptVolume("extent span overflows"))?;
            if extent.logical_offset % BLOCK_SIZE != 0 {
                return Err(FileSystemError::CorruptVolume("unaligned extent"));
            }
            let first = extent.logical_offset / BLOCK_SIZE;
            let count = end.div_ceil(BLOCK_SIZE).saturating_sub(first);
            for i in 0..count {
                let idx = first + i;
                if idx >= blocks {
                    break;
                }
                let pb = extent
                    .physical_block
                    .checked_add(i)
                    .ok_or(FileSystemError::CorruptVolume("extent target overflows"))?;
                if pb >= self.superblock.block_count {
                    return Err(FileSystemError::CorruptVolume("extent points past volume"));
                }
                map[idx as usize] = pb;
            }
        }
        Ok(map)
    }

    /// Re-emit a coalesced extent list from a block map. Interior extents are
    /// whole blocks; the extent covering the file tail is trimmed so extent
    /// lengths sum to exactly `size` for a hole-free file (the spilled-
    /// attribute read path derives the value length from that sum).
    fn emit_extents(map: &[u64], size: u64) -> ExtentList {
        let mut out: ExtentList = Vec::new();
        for (idx, &pb) in map.iter().enumerate() {
            if pb == 0 {
                continue; // hole
            }
            let logical = idx as u64 * BLOCK_SIZE;
            let len = core::cmp::min(BLOCK_SIZE, size.saturating_sub(logical));
            if len == 0 {
                break;
            }
            match out.last_mut() {
                Some(last)
                    if last.length % BLOCK_SIZE == 0
                        && last.logical_offset + last.length == logical
                        && last.physical_block + last.length / BLOCK_SIZE == pb =>
                {
                    last.length += len;
                }
                _ => out.push(Extent {
                    logical_offset: logical,
                    physical_block: pb,
                    length: len,
                }),
            }
        }
        out
    }

    fn write_data_inner(
        &mut self,
        inode_id: u64,
        offset: u64,
        data: &[u8],
    ) -> Result<(), FileSystemError> {
        if data.is_empty() {
            return Ok(());
        }
        let mut inode = self.read_inode(inode_id)?;
        let end = offset
            .checked_add(data.len() as u64)
            .ok_or(FileSystemError::CorruptVolume("write span overflows"))?;
        let new_size = core::cmp::max(inode.size, end);
        if new_size > self.volume_bytes() {
            return Err(FileSystemError::NoSpace);
        }
        let blocks = new_size.div_ceil(BLOCK_SIZE);
        let mut map = self.file_block_map(&inode, blocks)?;

        let first_touched = offset / BLOCK_SIZE;
        let last_touched = (end - 1) / BLOCK_SIZE;
        let mut buf = alloc::vec![0u8; BLOCK_SIZE as usize];
        for idx in first_touched..=last_touched {
            let old = map[idx as usize];
            if old != 0 {
                self.device.read_block(old, &mut buf)?;
            } else {
                buf.fill(0);
            }
            // Overlay the slice of `data` covering this block.
            let block_start = idx * BLOCK_SIZE;
            let from = core::cmp::max(offset, block_start);
            let to = core::cmp::min(end, block_start + BLOCK_SIZE);
            let src = &data[(from - offset) as usize..(to - offset) as usize];
            buf[(from - block_start) as usize..(to - block_start) as usize].copy_from_slice(src);

            // NEVER overwrite in place: fresh block, release the old one.
            let nb = self.alloc_block()?;
            self.write_fresh(nb, &buf)?;
            if old != 0 {
                self.refmap.decref(old);
            }
            map[idx as usize] = nb;
        }

        inode.chunks = Self::emit_extents(&map, new_size);
        inode.size = new_size;
        self.write_inode(&inode)?;
        Ok(())
    }

    /// Write data to an Inode (grow-only size semantics, like the pre-K8
    /// path). One transaction.
    pub fn write_data(
        &mut self,
        inode_id: u64,
        offset: u64,
        data: &[u8],
    ) -> Result<(), FileSystemError> {
        self.write_data_inner(inode_id, offset, data)?;
        self.maybe_commit()
    }

    /// Read data from an Inode.
    pub fn read_data(
        &mut self,
        inode_id: u64,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, FileSystemError> {
        let inode = self.read_inode(inode_id)?;
        self.read_from_extents(&inode.chunks, offset, length, inode.size)
    }

    /// Internal helper to read data from a specific ExtentList.
    ///
    /// `total_size` and the extent geometry are disk-derived, so everything
    /// here is bounded (BEFS-HARDEN): `total_size` against the volume span,
    /// the output allocation via `try_reserve_exact`, extent arithmetic via
    /// `checked_add`, extent targets against the volume, and hole fills in
    /// bulk runs.
    fn read_from_extents(
        &mut self,
        chunks: &ExtentList,
        offset: u64,
        length: u64,
        total_size: u64,
    ) -> Result<Vec<u8>, FileSystemError> {
        if total_size > self.volume_bytes() {
            return Err(FileSystemError::CorruptVolume("data size exceeds volume"));
        }

        let available = total_size.saturating_sub(offset);
        let to_read_total = core::cmp::min(length, available);
        let to_read_capacity = usize::try_from(to_read_total)
            .map_err(|_| StorageError::AllocRefused(to_read_total))?;

        let mut buffer = Vec::new();
        buffer
            .try_reserve_exact(to_read_capacity)
            .map_err(|_| StorageError::AllocRefused(to_read_total))?;

        let mut read_so_far = 0;
        let mut current_offset = offset;

        while read_so_far < to_read_total {
            let mut physical_block = 0;
            let mut found = false;
            let mut next_start = u64::MAX;

            for extent in chunks {
                let extent_end = extent
                    .logical_offset
                    .checked_add(extent.length)
                    .ok_or(FileSystemError::CorruptVolume("extent span overflows"))?;
                if current_offset >= extent.logical_offset && current_offset < extent_end {
                    let offset_in_extent = current_offset - extent.logical_offset;
                    let block_idx = offset_in_extent / BLOCK_SIZE;
                    physical_block = extent
                        .physical_block
                        .checked_add(block_idx)
                        .ok_or(FileSystemError::CorruptVolume("extent target overflows"))?;
                    found = true;
                    break;
                }
                if extent.logical_offset > current_offset && extent.logical_offset < next_start {
                    next_start = extent.logical_offset;
                }
            }

            if !found {
                let remaining = to_read_total - read_so_far;
                let gap = core::cmp::min(remaining, next_start.saturating_sub(current_offset));
                buffer.resize(buffer.len() + gap as usize, 0);
                read_so_far += gap;
                current_offset += gap;
                continue;
            }

            if physical_block >= self.superblock.block_count {
                return Err(FileSystemError::CorruptVolume("extent points past volume"));
            }

            let block_offset = (current_offset % BLOCK_SIZE) as usize;
            let to_read = core::cmp::min(
                BLOCK_SIZE as usize - block_offset,
                (to_read_total - read_so_far) as usize,
            );

            let mut block_buf = alloc::vec![0u8; BLOCK_SIZE as usize];
            self.device.read_block(physical_block, &mut block_buf)?;

            buffer.extend_from_slice(&block_buf[block_offset..block_offset + to_read]);

            read_so_far += to_read as u64;
            current_offset += to_read as u64;
        }

        Ok(buffer)
    }

    // =====================================================================
    // Namespace
    // =====================================================================

    pub fn ls(&mut self, inode_id: u64) -> Result<Vec<DirEntry>, FileSystemError> {
        let inode = self.read_inode(inode_id)?;
        if inode.kind != FileKind::Directory {
            return Err(FileSystemError::NotADirectory);
        }
        if inode.size == 0 {
            return Ok(Vec::new());
        }
        let data = self.read_data(inode_id, 0, inode.size)?;
        let entries: Vec<DirEntry> = crate::codec::deserialize(&data)?;
        Ok(entries)
    }

    pub fn mkdir(&mut self, parent_id: u64, name: String) -> Result<u64, FileSystemError> {
        self.add_entry(parent_id, name, FileKind::Directory)
    }

    pub fn create_file(&mut self, parent_id: u64, name: String) -> Result<u64, FileSystemError> {
        self.add_entry(parent_id, name, FileKind::File)
    }

    /// Resolves a path string to an Inode ID.
    pub fn resolve_path(&mut self, path: &str) -> Result<u64, FileSystemError> {
        let path = path.trim_start_matches('/');
        if path.is_empty() {
            return Ok(self.superblock.root_inode);
        }

        let parts: Vec<&str> = path.split('/').collect();
        let mut current_id = self.superblock.root_inode;

        for part in parts {
            if part.is_empty() {
                continue;
            }

            let entries = self.ls(current_id)?;
            let entry = entries
                .into_iter()
                .find(|e| e.name == part)
                .ok_or(FileSystemError::RootMissing)?;
            current_id = entry.inode_id;
        }

        Ok(current_id)
    }

    /// Create a named child (file or directory) under `parent_id` — the new
    /// inode AND the parent-directory rewrite land in ONE transaction, so a
    /// power cut leaves both or neither.
    fn add_entry(
        &mut self,
        parent_id: u64,
        name: String,
        kind: FileKind,
    ) -> Result<u64, FileSystemError> {
        let parent_inode = self.read_inode(parent_id)?;
        if parent_inode.kind != FileKind::Directory {
            return Err(FileSystemError::NotADirectory);
        }

        let mut entries = if parent_inode.size > 0 {
            self.ls(parent_id)?
        } else {
            Vec::new()
        };

        if entries.iter().any(|e| e.name == name) {
            return Err(FileSystemError::FileExists);
        }

        let new_id = self.create_inode_inner(kind, BTreeMap::new())?;

        entries.push(DirEntry {
            name,
            inode_id: new_id,
            kind,
        });
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        let data = crate::codec::serialize(&entries)?;
        self.rewrite_data_inner(parent_id, &data)?;
        self.maybe_commit()?;

        Ok(new_id)
    }

    // =====================================================================
    // Attribute engine
    // =====================================================================

    pub fn set_attribute(
        &mut self,
        inode_id: u64,
        key: String,
        value: AttributeValue,
    ) -> Result<(), FileSystemError> {
        let mut inode = self.read_inode(inode_id)?;

        if let Some(extents) = inode.large_attributes.remove(&key) {
            self.decref_extents(&extents);
        }

        let is_large = match &value {
            AttributeValue::Vector(v) => v.len() > 64, // > 256 bytes
            AttributeValue::Blob(b) => b.len() > 256,
            AttributeValue::String(s) => s.len() > 256,
            _ => false,
        };

        if is_large {
            let data = crate::codec::serialize(&value)?;
            let extents = self.allocate_and_write_extents(&data)?;
            inode.large_attributes.insert(key.clone(), extents);
            inode.attributes.remove(&key);
        } else {
            inode.attributes.insert(key.clone(), value.clone());
        }

        self.write_inode(&inode)?;
        self.update_catalog(&key, &value, inode_id)?;
        self.maybe_commit()?;

        #[cfg(feature = "std")]
        {
            let msg = SMessage::FileEvent {
                path: format!("inode:{}", inode_id),
                event: format!("AttributeSet:{}", key),
            };
            let _ = self.publish("system/fs/change", msg);
        }

        Ok(())
    }

    pub fn get_attribute(
        &mut self,
        inode_id: u64,
        key: &str,
    ) -> Result<Option<AttributeValue>, FileSystemError> {
        let inode = self.read_inode(inode_id)?;

        if let Some(val) = inode.attributes.get(key) {
            return Ok(Some(val.clone()));
        }

        if let Some(extents) = inode.large_attributes.get(key) {
            let total_size = checked_extent_total(extents)?;
            let data = self.read_from_extents(extents, 0, total_size, total_size)?;
            let val: AttributeValue = crate::codec::deserialize(&data)
                .map_err(|_| FileSystemError::InvalidAttributeData)?;
            return Ok(Some(val));
        }

        Ok(None)
    }

    // =====================================================================
    // Mutation engine — under CoW every op below is ONE transaction: all of
    // its rewrites become visible at a single root flip, or none do. The
    // pre-K8 crash windows (unindexed-but-named, named-in-neither-directory,
    // leaked extents) are gone by construction.
    // =====================================================================

    /// Remove `name` from directory `parent_id`: the directory entry, every
    /// catalog index entry for the inode, the inode's data and spilled-
    /// attribute blocks, its inode block, and its inode-map slot all go —
    /// atomically, at the transaction's root flip. Returns the freed
    /// (logical) inode id.
    ///
    /// Only `File` and `Symlink` entries can be unlinked; a directory is
    /// refused with [`FileSystemError::IsADirectory`].
    ///
    /// # Open-handle semantics (honest note)
    /// unafs has no open-file table — callers address files by inode id.
    /// `unlink` invalidates the id immediately: later calls with the stale id
    /// fail with `NotFound` (the map slot is cleared; logical ids are never
    /// recycled, so a stale id can never alias a NEW file).
    pub fn unlink(&mut self, parent_id: u64, name: &str) -> Result<u64, FileSystemError> {
        let mut entries = self.ls(parent_id)?;
        let pos = entries
            .iter()
            .position(|e| e.name == name)
            .ok_or(FileSystemError::NotFound)?;
        if entries[pos].kind == FileKind::Directory {
            return Err(FileSystemError::IsADirectory);
        }
        let inode_id = entries[pos].inode_id;

        // Read the doomed inode up front.
        let inode = self.read_inode(inode_id)?;

        // 1. Scrub the attribute index.
        self.remove_catalog_entries_inner(|e| e.inode_id == inode_id)?;

        // 2. Unhook the name.
        entries.remove(pos);
        let dir_data = crate::codec::serialize(&entries)?;
        self.rewrite_data_inner(parent_id, &dir_data)?;

        // 3. Release everything the inode owned. The frozen view keeps the
        //    old tree intact until the flip.
        for extents in inode.large_attributes.values() {
            self.decref_extents(extents);
        }
        self.decref_extents(&inode.chunks);
        let pb = self.imap.get(inode_id as usize).copied().unwrap_or(0);
        if pb != 0 {
            self.refmap.decref(pb);
        }
        self.imap_clear(inode_id);

        self.maybe_commit()?;

        #[cfg(feature = "std")]
        {
            let msg = SMessage::FileEvent {
                path: format!("inode:{}", inode_id),
                event: format!("Unlinked:{}", name),
            };
            let _ = self.publish("system/fs/change", msg);
        }

        Ok(inode_id)
    }

    /// Rename `old_name` in `parent_id` to `new_name` in `new_parent_id` —
    /// a same-directory rename when the parents match, a cross-directory
    /// move otherwise. Only directory entries change; the inode, its data,
    /// and the catalog (which keys on the stable logical id) are untouched.
    /// Both directory rewrites land in ONE transaction, so the pre-K8
    /// "in neither directory" crash window no longer exists.
    ///
    /// `new_name` must not already exist (`FileExists`; deliberate divergence
    /// from POSIX overwrite). Renaming an entry to its own name is a no-op
    /// `Ok`. Moving a DIRECTORY into itself or a descendant is refused with
    /// `DirectoryLoop`.
    pub fn rename(
        &mut self,
        parent_id: u64,
        old_name: &str,
        new_parent_id: u64,
        new_name: &str,
    ) -> Result<(), FileSystemError> {
        let same_dir = parent_id == new_parent_id;
        if same_dir && old_name == new_name {
            return Ok(());
        }

        let mut src_entries = self.ls(parent_id)?;
        let pos = src_entries
            .iter()
            .position(|e| e.name == old_name)
            .ok_or(FileSystemError::NotFound)?;
        let moved = src_entries[pos].clone();

        if same_dir {
            if src_entries.iter().any(|e| e.name == new_name) {
                return Err(FileSystemError::FileExists);
            }
        } else {
            // `ls` also validates that the destination IS a directory.
            let dst_entries = self.ls(new_parent_id)?;
            if dst_entries.iter().any(|e| e.name == new_name) {
                return Err(FileSystemError::FileExists);
            }
            if moved.kind == FileKind::Directory
                && (moved.inode_id == new_parent_id
                    || self.is_descendant_of(new_parent_id, moved.inode_id)?)
            {
                return Err(FileSystemError::DirectoryLoop);
            }
        }

        if same_dir {
            src_entries[pos].name = new_name.into();
            src_entries.sort_by(|a, b| a.name.cmp(&b.name));
            let data = crate::codec::serialize(&src_entries)?;
            self.rewrite_data_inner(parent_id, &data)?;
        } else {
            src_entries.remove(pos);
            let src_data = crate::codec::serialize(&src_entries)?;
            self.rewrite_data_inner(parent_id, &src_data)?;

            let mut dst_entries = self.ls(new_parent_id)?;
            dst_entries.push(DirEntry {
                name: new_name.into(),
                inode_id: moved.inode_id,
                kind: moved.kind,
            });
            dst_entries.sort_by(|a, b| a.name.cmp(&b.name));
            let dst_data = crate::codec::serialize(&dst_entries)?;
            self.rewrite_data_inner(new_parent_id, &dst_data)?;
        }

        self.maybe_commit()?;

        #[cfg(feature = "std")]
        {
            let msg = SMessage::FileEvent {
                path: format!("inode:{}", moved.inode_id),
                event: format!("Renamed:{}->{}", old_name, new_name),
            };
            let _ = self.publish("system/fs/change", msg);
        }

        Ok(())
    }

    /// Remove attribute `key` from `inode_id`: the inline or spilled value
    /// and every catalog entry for the (inode, key) pair go, atomically.
    pub fn remove_attribute(&mut self, inode_id: u64, key: &str) -> Result<(), FileSystemError> {
        let mut inode = self.read_inode(inode_id)?;
        if !inode.attributes.contains_key(key) && !inode.large_attributes.contains_key(key) {
            return Err(FileSystemError::AttributeNotFound);
        }

        let key_hash = hash_bytes(key.as_bytes());
        self.remove_catalog_entries_inner(|e| e.inode_id == inode_id && e.key_hash == key_hash)?;

        inode.attributes.remove(key);
        let spilled = inode.large_attributes.remove(key);
        self.write_inode(&inode)?;

        if let Some(extents) = spilled {
            self.decref_extents(&extents);
        }

        self.maybe_commit()?;

        #[cfg(feature = "std")]
        {
            let msg = SMessage::FileEvent {
                path: format!("inode:{}", inode_id),
                event: format!("AttributeRemoved:{}", key),
            };
            let _ = self.publish("system/fs/change", msg);
        }

        Ok(())
    }

    // =====================================================================
    // Snapshot index + reclaim queue (the future shape, on disk today)
    // =====================================================================

    /// The snapshot index (empty until K8b retains roots). v1 policy cap:
    /// [`SNAPSHOT_CAP`].
    pub fn snapshot_index(&mut self) -> Result<Vec<SnapshotEntry>, FileSystemError> {
        let inode = self.read_inode(SNAP_INDEX_INODE_ID)?;
        let data = self.read_data(SNAP_INDEX_INODE_ID, 0, inode.size)?;
        if data.is_empty() {
            return Ok(Vec::new());
        }
        Ok(crate::codec::deserialize(&data)?)
    }

    /// The persistent reclaim queue's current entries.
    pub fn reclaim_queue(&mut self) -> Result<Vec<ReclaimEntry>, FileSystemError> {
        let inode = self.read_inode(RECLAIM_INODE_ID)?;
        let data = self.read_data(RECLAIM_INODE_ID, 0, inode.size)?;
        if data.is_empty() {
            return Ok(Vec::new());
        }
        Ok(crate::codec::deserialize(&data)?)
    }

    /// Enqueue a dropped root's blocks for reclamation, durably (one
    /// transaction), WITHOUT freeing anything. K8b's snapshot-drop calls
    /// this; the eager drain then runs immediately (v1 policy). Public so
    /// the host suite can prove the crash-safe drain independently.
    pub fn reclaim_enqueue(&mut self, entry: ReclaimEntry) -> Result<(), FileSystemError> {
        let mut queue = self.reclaim_queue()?;
        queue.push(entry);
        let bytes = crate::codec::serialize(&queue)?;
        self.rewrite_data_inner(RECLAIM_INODE_ID, &bytes)?;
        self.maybe_commit()
    }

    /// Drain the reclaim queue to empty (v1 eager policy): decref every
    /// enqueued block, empty the queue, ONE commit. Crash-safe: a power cut
    /// before the flip leaves the full queue for the next mount.
    pub fn reclaim_drain(&mut self) -> Result<(), FileSystemError> {
        let queue = self.reclaim_queue()?;
        if queue.is_empty() {
            return Ok(());
        }
        crate::warnlog::warn("[UNAFS] :: pending reclaim queue found — draining (eager v1)");
        for entry in &queue {
            for &b in &entry.blocks {
                if b > ROOT_BLOCK && b < self.superblock.block_count {
                    self.refmap.decref(b);
                }
            }
        }
        let empty: Vec<ReclaimEntry> = Vec::new();
        let bytes = crate::codec::serialize(&empty)?;
        self.rewrite_data_inner(RECLAIM_INODE_ID, &bytes)?;
        self.commit()
    }

    // =====================================================================
    // Query engine
    // =====================================================================

    /// Semantic query engine. no_std-capable: the similarity path routes its
    /// floating-point `sqrt` through `libm`, so kernel (`no_std`) and host
    /// (`std`) builds score along the same code path.
    pub fn query(&mut self, query_str: &str) -> Result<Vec<(Inode, f32)>, FileSystemError> {
        let query = Query::parse(query_str).map_err(FileSystemError::Query)?;

        let catalog_id = self.superblock.catalog_inode;
        let mut candidates = Vec::new();

        if catalog_id != 0 {
            let inode = self.read_inode(catalog_id)?;
            let data = self.read_data(catalog_id, 0, inode.size)?;
            let entries = deserialize_catalog(&data)?;

            let target_key_hash = hash_bytes(query.key.as_bytes());

            let target_val_hash = if let QueryOp::Eq = query.op {
                Some(hash_value(&query.value))
            } else {
                None
            };

            for entry in entries {
                if entry.key_hash == target_key_hash {
                    if let Some(tv) = target_val_hash {
                        if entry.val_hash == tv {
                            candidates.push(entry.inode_id);
                        }
                    } else {
                        candidates.push(entry.inode_id);
                    }
                }
            }
        }

        candidates.sort();
        candidates.dedup();

        let mut results = Vec::new();
        for id in candidates {
            let inode = match self.read_inode(id) {
                Ok(i) => i,
                Err(FileSystemError::NotFound) => continue, // stale index entry
                Err(e) => return Err(e),
            };

            let mut val_opt = None;
            if let Some(v) = inode.attributes.get(&query.key) {
                val_opt = Some(v.clone());
            } else if let Some(extents) = inode.large_attributes.get(&query.key) {
                let total = checked_extent_total(extents)?;
                let data = self.read_from_extents(extents, 0, total, total)?;
                if let Ok(v) = crate::codec::deserialize::<AttributeValue>(&data) {
                    val_opt = Some(v);
                }
            }

            if let Some(val) = val_opt {
                if let Some(score) = check_condition(&val, &query.op, &query.value) {
                    // Check secondary filters
                    let mut pass_secondary = true;
                    for (sec_key, sec_val) in &query.secondary_filters {
                        let mut match_found = false;
                        if let Some(v) = inode.attributes.get(sec_key) {
                            if let AttributeValue::String(s) = v {
                                if s == sec_val {
                                    match_found = true;
                                }
                            }
                        }
                        if !match_found {
                            pass_secondary = false;
                            break;
                        }
                    }

                    if pass_secondary {
                        results.push((inode, score));
                    }
                }
            }
        }

        Ok(results)
    }

    // =====================================================================
    // Helpers
    // =====================================================================

    /// Release every block an extent list covers (decref — the frozen view
    /// keeps the committed tree's blocks unallocatable until the next flip).
    /// Bounded like the read path: hostile extents release nothing real.
    pub(crate) fn decref_extents(&mut self, extents: &ExtentList) {
        for extent in extents {
            let blocks = extent.length.div_ceil(BLOCK_SIZE);
            for i in 0..blocks {
                let block = match extent.physical_block.checked_add(i) {
                    Some(b) if b < self.superblock.block_count => b,
                    _ => break,
                };
                self.refmap.decref(block);
            }
        }
    }

    /// Replace an inode's entire data contents (CoW): fresh extents for the
    /// new bytes, remap the inode, release the old extents. Part of the
    /// caller's transaction — becomes visible only at the root flip. Unlike
    /// `write_data`, the logical size shrinks to exactly `data.len()`.
    fn rewrite_data_inner(&mut self, inode_id: u64, data: &[u8]) -> Result<(), FileSystemError> {
        let new_chunks = if data.is_empty() {
            Vec::new()
        } else {
            self.allocate_and_write_extents(data)?
        };
        let mut inode = self.read_inode(inode_id)?;
        let old_chunks = core::mem::replace(&mut inode.chunks, new_chunks);
        inode.size = data.len() as u64;
        self.write_inode(&inode)?;
        self.decref_extents(&old_chunks);
        Ok(())
    }

    /// Depth-first walk: does directory `dir_id` live anywhere inside the
    /// subtree rooted at `root_id`?
    fn is_descendant_of(&mut self, dir_id: u64, root_id: u64) -> Result<bool, FileSystemError> {
        let mut stack = alloc::vec![root_id];
        while let Some(id) = stack.pop() {
            for entry in self.ls(id)? {
                if entry.kind == FileKind::Directory {
                    if entry.inode_id == dir_id {
                        return Ok(true);
                    }
                    stack.push(entry.inode_id);
                }
            }
        }
        Ok(false)
    }

    /// Rewrite the attribute catalog with every entry matching `pred`
    /// removed, inside the caller's transaction.
    fn remove_catalog_entries_inner<F: Fn(&CatalogEntry) -> bool>(
        &mut self,
        pred: F,
    ) -> Result<(), FileSystemError> {
        let catalog_id = self.superblock.catalog_inode;
        if catalog_id == 0 {
            return Ok(());
        }
        let inode = self.read_inode(catalog_id)?;
        let data = self.read_data(catalog_id, 0, inode.size)?;
        let mut entries = deserialize_catalog(&data)?;
        let before = entries.len();
        entries.retain(|e| !pred(e));
        if entries.len() == before {
            return Ok(());
        }
        let new_data = serialize_catalog(&entries)?;
        self.rewrite_data_inner(catalog_id, &new_data)?;
        Ok(())
    }

    /// Public catalog scrub (its own transaction) — the fsck repair path
    /// uses it.
    pub(crate) fn remove_catalog_entries<F: Fn(&CatalogEntry) -> bool>(
        &mut self,
        pred: F,
    ) -> Result<(), FileSystemError> {
        self.remove_catalog_entries_inner(pred)?;
        self.maybe_commit()
    }

    /// Write `data` to freshly allocated extents (always fresh — this IS the
    /// CoW allocation primitive), coalescing contiguous runs.
    fn allocate_and_write_extents(&mut self, data: &[u8]) -> Result<ExtentList, FileSystemError> {
        let mut extents: ExtentList = Vec::new();
        let mut data_written = 0;
        let mut current_logical = 0;

        while data_written < data.len() {
            let block_id = self.alloc_block()?;
            let to_write = core::cmp::min(BLOCK_SIZE as usize, data.len() - data_written);

            let mut block = alloc::vec![0u8; BLOCK_SIZE as usize];
            block[..to_write].copy_from_slice(&data[data_written..data_written + to_write]);
            self.write_fresh(block_id, &block)?;

            match extents.last_mut() {
                Some(last)
                    if last.length % BLOCK_SIZE == 0
                        && last.logical_offset + last.length == current_logical
                        && last.physical_block + last.length / BLOCK_SIZE == block_id =>
                {
                    last.length += to_write as u64;
                }
                _ => extents.push(Extent {
                    logical_offset: current_logical,
                    physical_block: block_id,
                    length: to_write as u64,
                }),
            }

            data_written += to_write;
            current_logical += to_write as u64;
        }

        Ok(extents)
    }

    fn update_catalog(
        &mut self,
        key: &str,
        value: &AttributeValue,
        inode_id: u64,
    ) -> Result<(), FileSystemError> {
        let catalog_id = self.superblock.catalog_inode;
        if catalog_id == 0 {
            return Ok(());
        }

        let inode = self.read_inode(catalog_id)?;
        let data = self.read_data(catalog_id, 0, inode.size)?;
        let mut entries = deserialize_catalog(&data)?;

        entries.push(CatalogEntry::new(key, value, inode_id));

        let new_data = serialize_catalog(&entries)?;
        self.rewrite_data_inner(catalog_id, &new_data)?;

        Ok(())
    }
}

impl<D: BlockDevice> Drop for UnaFS<D> {
    fn drop(&mut self) {
        // Under CoW every completed public op has already committed (unless
        // the caller disabled autocommit — the crash-simulation seam, whose
        // whole point is that dropping models a power cut). Just flush.
        let _ = self.device.flush();
    }
}

#[cfg(feature = "std")]
impl<D: BlockDevice> BandyMember for UnaFS<D> {
    fn publish(&self, topic: &str, msg: SMessage) -> anyhow::Result<()> {
        println!("[UNAFS] Broadcasting event to '{}': {:?}", topic, msg);
        Ok(())
    }
}

/// Cosine similarity between two `f32` vectors.
///
/// The square roots go through [`libm::sqrtf`] on every build — `std` and
/// `no_std` alike — so there is exactly ONE scoring path: a query answered by
/// the kernel over a mounted volume and the same query answered by a host
/// tool produce bit-identical scores. (`libm::sqrtf` is correctly rounded per
/// IEEE 754, matching `f32::sqrt` on hosts; `tests/query_kats.rs` pins both
/// facts with golden vectors.)
///
/// Mismatched lengths and zero-magnitude inputs score `0.0`.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let mag_a: f32 = libm::sqrtf(a.iter().map(|x| x * x).sum::<f32>());
    let mag_b: f32 = libm::sqrtf(b.iter().map(|x| x * x).sum::<f32>());
    if mag_a == 0.0 || mag_b == 0.0 {
        return 0.0;
    }
    dot / (mag_a * mag_b)
}

/// Sum a disk-derived extent list's lengths with overflow checking
/// (BEFS-HARDEN): a hostile volume's spilled-attribute extents drive the
/// allocation in `read_from_extents`, so a wrapping sum must be a graceful
/// `Err`, never a silently small (or debug-panicking) total.
fn checked_extent_total(extents: &ExtentList) -> Result<u64, FileSystemError> {
    extents
        .iter()
        .try_fold(0u64, |acc, e| acc.checked_add(e.length))
        .ok_or(FileSystemError::CorruptVolume("extent lengths overflow"))
}

fn check_condition(val: &AttributeValue, op: &QueryOp, target: &AttributeValue) -> Option<f32> {
    match op {
        QueryOp::Eq => {
            if val == target {
                Some(1.0)
            } else {
                None
            }
        }
        QueryOp::Neq => {
            if val != target {
                Some(1.0)
            } else {
                None
            }
        }
        QueryOp::Gt => {
            if partial_cmp_attr(val, target)
                .map(|o| o.is_gt())
                .unwrap_or(false)
            {
                Some(1.0)
            } else {
                None
            }
        }
        QueryOp::Lt => {
            if partial_cmp_attr(val, target)
                .map(|o| o.is_lt())
                .unwrap_or(false)
            {
                Some(1.0)
            } else {
                None
            }
        }
        QueryOp::SimilarityGt(threshold) => {
            if let (AttributeValue::Vector(v1), AttributeValue::Vector(v2)) = (val, target) {
                let score = cosine_similarity(v1, v2);
                if score > *threshold {
                    Some(score)
                } else {
                    None
                }
            } else {
                None
            }
        }
    }
}

use core::cmp::Ordering;
fn partial_cmp_attr(a: &AttributeValue, b: &AttributeValue) -> Option<Ordering> {
    match (a, b) {
        (AttributeValue::Int(i1), AttributeValue::Int(i2)) => i1.partial_cmp(i2),
        (AttributeValue::Float(f1), AttributeValue::Float(f2)) => f1.partial_cmp(f2),
        (AttributeValue::String(s1), AttributeValue::String(s2)) => s1.partial_cmp(s2),
        _ => None,
    }
}
