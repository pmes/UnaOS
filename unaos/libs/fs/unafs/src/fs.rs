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

use crate::btree::{Btree, BtreeError, DeviceStore, LexCmp};
use crate::catalog::{CatalogEntry, deserialize_catalog, serialize_catalog};
use crate::index::{CatalogRecord, IndexFact, ReadStore};
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

use crate::query::{Expr, Predicate, Query, QueryOp};
use alloc::collections::BTreeSet;
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
    /// RMDIR (SO18): the directory named for removal still holds entries — the
    /// POSIX `ENOTEMPTY`. Deliberately a DISTINCT variant from
    /// [`IsADirectory`](Self::IsADirectory): `unlink` refuses a directory
    /// because of its KIND, `rmdir` refuses one because of its CONTENTS, and a
    /// caller that cannot tell those apart cannot render the two errors an
    /// operator has to act on differently (`-EISDIR` vs `-ENOTEMPTY`).
    #[error("Directory not empty")]
    DirectoryNotEmpty,
    #[error("Attribute not found")]
    AttributeNotFound,
    #[error("Cannot move a directory into itself or its descendants")]
    DirectoryLoop,
    #[error("Corrupt volume: {0}")]
    CorruptVolume(&'static str),
    #[error("Snapshot retention cap reached ({0} max)")]
    SnapshotCapReached(usize),
    #[error("Snapshot not found (generation {0})")]
    SnapshotNotFound(u64),
}

/// A directory entry pointing to an inode (by stable LOGICAL id).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, PartialOrd)]
pub struct DirEntry {
    pub name: String,
    pub inode_id: u64,
    pub kind: FileKind,
}

/// A file staged for the bulk create+write path
/// ([`UnaFS::create_files_batch`]). Carries everything one `create_file` +
/// `write_data` + N × `set_attribute` would, so the batch can fold them into
/// ONE inode write per file and a single parent-directory + catalog rewrite for
/// the whole set.
pub struct BatchFile {
    /// The child name under the batch's parent directory.
    pub name: String,
    /// The file's initial contents (empty for a zero-length file).
    pub data: Vec<u8>,
    /// Typed attributes to attach, indexed in the catalog exactly as
    /// `set_attribute` would.
    pub attributes: BTreeMap<String, AttributeValue>,
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

impl SnapshotEntry {
    /// Owner-or-kernel destructive authority (K8b §4, the BANDY
    /// owner-only-destructive ruling applied to snapshots): dropping a retained
    /// root is permitted iff the `invoker` principal is this snapshot's
    /// `creator` or the `kernel_principal`. The library records the creator and
    /// exposes this decision; the actual enforcement point is the ACL/verb
    /// layer (the kernel shell runs at kernel authority, so its drops always
    /// pass — the trivial `invoker == kernel_principal` case). Pure, so host
    /// twins can exercise owner/other/kernel without a mount.
    pub fn drop_permitted(&self, invoker: &str, kernel_principal: &str) -> bool {
        invoker == kernel_principal || invoker == self.creator
    }
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
    /// Snapshots retained since mount/format (K8b: `snapshot_create` calls that
    /// committed a new retained root).
    pub snapshots_created: u64,
    /// Snapshots dropped since mount/format (K8b: `snapshot_drop` calls that
    /// enqueued + drained a retained root).
    pub snapshots_dropped: u64,
}

/// The committed in-RAM state [`UnaFS::load_committed`] reads off the device —
/// the root record and the maps it reaches. Shared by mount and the failed-
/// transaction unwind (which re-derives ground truth from the disk).
struct LoadedState {
    root: RootRecord,
    active_slot: RootSlot,
    imap: Vec<u64>,
    imap_blocks: Vec<u64>,
    refmap_blocks: Vec<u64>,
    refmap: RefMap,
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

    /// Format the device with a new (K8, version 5) UnaFS filesystem. Version 4+
    /// is spill-capable: a file whose extent list overflows its inode block
    /// spills to indirect blocks (see [`crate::inode::IndirectTrailer`]).
    /// Version 5 admits volumes past 2 GiB (up to
    /// [`crate::superblock::MAX_BLOCK_COUNT`] blocks = 1 TiB) via a second
    /// refcount-map index level; a volume of ≤ 2 GiB stays single-level and
    /// byte-identical to v4 apart from the version stamp.
    pub fn format(device: D, size_mb: u64) -> Result<Self, FileSystemError> {
        Self::format_with_version(device, size_mb, crate::superblock::VERSION)
    }

    /// Format with an EXPLICIT on-disk version in
    /// `MIN_SUPPORTED_VERSION..=VERSION` — the compatibility seam: a v5 image
    /// (flat catalog, no inode meta trailers) for a pre-v6 reader, and the
    /// genuine old-format fixtures the migration and compatibility tests run
    /// against. Everything the chosen version does not declare is simply not
    /// written (no trees, no trailers, no spill on v3).
    pub fn format_with_version(
        mut device: D,
        size_mb: u64,
        version: u32,
    ) -> Result<Self, FileSystemError> {
        if !(crate::superblock::MIN_SUPPORTED_VERSION..=crate::superblock::VERSION).contains(&version) {
            return Err(SuperblockError::InvalidVersion(version).into());
        }
        let blocks_from_size = (size_mb * 1024 * 1024) / BLOCK_SIZE;
        let mut block_count = device.block_count();
        if block_count == 0 {
            block_count = blocks_from_size;
        }

        let mut superblock = Superblock::new(block_count);
        superblock.version = version;
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

        let mut refmap = RefMap::try_new(block_count)?;
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
        let root_id = fs.create_inode_inner(FileKind::Directory, BTreeMap::new(), 0, None)?;
        debug_assert_eq!(root_id, ROOT_INODE_ID);
        let catalog_id = fs.create_inode_inner(FileKind::System, BTreeMap::new(), 0, None)?;
        debug_assert_eq!(catalog_id, CATALOG_INODE_ID);
        if fs.superblock.indexed() {
            // v6: the catalog is two EMPTY B+trees named by the catalog record.
            let (eq_root, ord_root, written) = {
                let mut store = DeviceStore::new(&mut fs.device, &mut fs.refmap);
                let eq = Btree::create(&mut store, LexCmp)?;
                let ord = Btree::create(&mut store, LexCmp)?;
                (eq.root(), ord.root(), store.written)
            };
            fs.count_index_writes(written);
            let rec = CatalogRecord { eq_root, ord_root, entries: 0 };
            fs.rewrite_data_inner(catalog_id, &rec.to_bytes())?;
        }
        let snap_id = fs.create_inode_inner(FileKind::System, BTreeMap::new(), 0, None)?;
        debug_assert_eq!(snap_id, SNAP_INDEX_INODE_ID);
        let empty_snaps: Vec<SnapshotEntry> = Vec::new();
        let bytes = crate::codec::serialize(&empty_snaps)?;
        fs.rewrite_data_inner(snap_id, &bytes)?;
        let reclaim_id = fs.create_inode_inner(FileKind::System, BTreeMap::new(), 0, None)?;
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

        let loaded = Self::load_committed(&mut device, &superblock)?;

        let mut fs = Self {
            device,
            superblock,
            refmap: loaded.refmap,
            imap: loaded.imap,
            root: loaded.root,
            active_slot: loaded.active_slot,
            imap_blocks: loaded.imap_blocks,
            refmap_blocks: loaded.refmap_blocks,
            autocommit: true,
            stats: CommitStats::default(),
            txn_blocks: 0,
        };

        // Persistent reclaim queue: v1 drains eagerly on mount (crash-safe —
        // the drain is one commit; a power cut mid-drain resumes here).
        fs.reclaim_drain()?;

        Ok(fs)
    }

    /// Load the COMMITTED in-RAM state (root record + inode map + refcount
    /// map) from the device — the shared body of [`mount`](Self::mount) and
    /// [`txn_unwind`](Self::txn_unwind). Reads only; validates everything the
    /// way mount always has (the volume is untrusted input).
    fn load_committed(
        device: &mut D,
        superblock: &Superblock,
    ) -> Result<LoadedState, FileSystemError> {
        let block_count = superblock.block_count;

        let (root, active_slot) = crate::root::read_active(device)?
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
        // The leaf count is a pure function of the geometry, and so is the
        // index SHAPE: one block of leaf pointers up to 512 leaves (the only
        // shape a v3/v4 volume can have — `Superblock::validate` caps their
        // geometry at one level), a two-level tree past that (v5+): the root
        // points at an index-of-indexes of MID blocks, each holding up to 512
        // leaf pointers.
        let expected_leaves = block_count.div_ceil(crate::refmap::REFS_PER_LEAF);
        if root.refmap_leaves != expected_leaves {
            return Err(FileSystemError::CorruptVolume(
                "refmap size inconsistent with block count",
            ));
        }
        if root.refmap_block == 0 || root.refmap_block >= block_count {
            return Err(FileSystemError::CorruptVolume("refmap index out of bounds"));
        }
        let ptrs_per_index = BLOCK_SIZE / 8;
        if root.refmap_leaves > ptrs_per_index * ptrs_per_index {
            return Err(FileSystemError::CorruptVolume(
                "refmap leaf count too large",
            ));
        }
        // Gather the leaf pointers through one or two index levels. Bounded:
        // expected_leaves ≤ 512² by the guard above (and by MAX_BLOCK_COUNT).
        let leaves_cap = usize::try_from(root.refmap_leaves)
            .map_err(|_| StorageError::AllocRefused(root.refmap_leaves))?;
        let mut leaf_ptrs: Vec<u64> = Vec::new();
        leaf_ptrs
            .try_reserve_exact(leaves_cap)
            .map_err(|_| StorageError::AllocRefused(root.refmap_leaves))?;
        let mut refmap_blocks = alloc::vec![root.refmap_block];
        device.read_block(root.refmap_block, &mut index)?;
        let read_ptr = |buf: &[u8], i: u64| {
            u64::from_le_bytes(
                buf[(i * 8) as usize..(i * 8 + 8) as usize]
                    .try_into()
                    .unwrap(),
            )
        };
        if root.refmap_leaves <= ptrs_per_index {
            // Single level: the root's index block IS the leaf-pointer block.
            for l in 0..root.refmap_leaves {
                leaf_ptrs.push(read_ptr(&index, l));
            }
        } else {
            // Two levels: the root's block holds MID pointers; each mid block
            // holds this level's slice of leaf pointers.
            let mids = root.refmap_leaves.div_ceil(ptrs_per_index);
            let mut mid = alloc::vec![0u8; BLOCK_SIZE as usize];
            for m in 0..mids {
                let mp = read_ptr(&index, m);
                if mp == 0 || mp >= block_count {
                    return Err(FileSystemError::CorruptVolume(
                        "refmap mid index out of bounds",
                    ));
                }
                refmap_blocks.push(mp);
                device.read_block(mp, &mut mid)?;
                let lo = m * ptrs_per_index;
                let hi = core::cmp::min(root.refmap_leaves, lo + ptrs_per_index);
                for l in lo..hi {
                    leaf_ptrs.push(read_ptr(&mid, l - lo));
                }
            }
        }
        let count_cap =
            usize::try_from(block_count).map_err(|_| StorageError::AllocRefused(block_count))?;
        let mut counts: Vec<u32> = Vec::new();
        counts
            .try_reserve_exact(count_cap)
            .map_err(|_| StorageError::AllocRefused(block_count))?;
        for &ptr in &leaf_ptrs {
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
        let refmap = RefMap::try_from_counts(counts, block_count)?;

        Ok(LoadedState {
            root,
            active_slot,
            imap,
            imap_blocks,
            refmap_blocks,
            refmap,
        })
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
        //    us mid-step — and so is the index SHAPE: one block of leaf
        //    pointers up to 512 leaves (the only shape v3/v4 geometry admits,
        //    which keeps every pre-v5 volume byte-compatible), a two-level
        //    tree past that (v5+; `Superblock::validate` version-gates the
        //    geometry at format/mount).
        let refmap_leaves = self.refmap.leaf_count();
        let ptrs_per_index = BLOCK_SIZE / 8;
        // Belt-and-braces twin of the imap guard above (and of
        // `Superblock::validate`'s MAX_BLOCK_COUNT bound): never run the
        // index tree off its blocks, error cleanly.
        if refmap_leaves > ptrs_per_index * ptrs_per_index {
            return Err(FileSystemError::NoSpace);
        }
        let two_level = refmap_leaves > ptrs_per_index;
        let mids = if two_level {
            refmap_leaves.div_ceil(ptrs_per_index)
        } else {
            0
        };
        let ref_index_block = self.alloc_block()?;
        let mut ref_mid_blocks = Vec::with_capacity(mids as usize);
        for _ in 0..mids {
            ref_mid_blocks.push(self.alloc_block()?);
        }
        let mut ref_leaf_blocks = Vec::with_capacity(refmap_leaves as usize);
        for _ in 0..refmap_leaves {
            ref_leaf_blocks.push(self.alloc_block()?);
        }
        // Every allocation is done: the counts are final. Serialize.
        let mut ref_index = alloc::vec![0u8; BLOCK_SIZE as usize];
        if two_level {
            // Leaves first, then each mid block carries its slice of leaf
            // pointers, then the top block carries the mid pointers.
            for (l, &leaf_block) in ref_leaf_blocks.iter().enumerate() {
                let leaf = self.refmap.leaf_bytes(l as u64);
                self.write_fresh(leaf_block, &leaf)?;
            }
            for (m, &mid_block) in ref_mid_blocks.iter().enumerate() {
                let mut mid = alloc::vec![0u8; BLOCK_SIZE as usize];
                let lo = m * ptrs_per_index as usize;
                let hi = core::cmp::min(ref_leaf_blocks.len(), lo + ptrs_per_index as usize);
                for (i, &leaf_block) in ref_leaf_blocks[lo..hi].iter().enumerate() {
                    mid[i * 8..i * 8 + 8].copy_from_slice(&leaf_block.to_le_bytes());
                }
                self.write_fresh(mid_block, &mid)?;
                ref_index[m * 8..m * 8 + 8].copy_from_slice(&mid_block.to_le_bytes());
            }
        } else {
            for (l, &leaf_block) in ref_leaf_blocks.iter().enumerate() {
                ref_index[l * 8..l * 8 + 8].copy_from_slice(&leaf_block.to_le_bytes());
                let leaf = self.refmap.leaf_bytes(l as u64);
                self.write_fresh(leaf_block, &leaf)?;
            }
        }
        self.write_fresh(ref_index_block, &ref_index)?;
        // (Written last among the fresh blocks so everything lands before
        //  the barrier; order among fresh blocks is immaterial — none are
        //  reachable until the flip.)
        self.write_fresh(index_block, &index)?;

        let free_blocks = self.refmap.free_blocks();

        // 4. Barrier: every fresh block must be on the medium before the
        //    root flip makes them reachable.
        //
        //    LOAD-BEARING CONTRACT (lens B, 2026-07-16): on the kernel path
        //    `flush()` is a NO-OP — the ordering rests entirely on every
        //    `write_block`/`write_sector` being SYNCHRONOUS-TO-MEDIUM (the
        //    eMMC2 CMD24 busy-wait + CMD13 status check). This flush is
        //    decorative there (and a real fsync on host FileDevice). Any
        //    future write-cache, write-back, or DMA-queued storage path MUST
        //    implement a real flush at its device seam, or commit ordering
        //    (fresh blocks before the root flip) silently breaks.
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
        // Same contract as the pre-flip barrier above: decorative where every
        // write is already synchronous-to-medium; MANDATORY-real on any
        // future cached/queued device.
        self.device.flush()?;

        // 6. The new tree is the committed tree.
        self.root = new_root;
        self.active_slot = slot;
        let mut blocks = new_imap_blocks;
        blocks.insert(0, index_block);
        self.imap_blocks = blocks;
        let mut rblocks = alloc::vec![ref_index_block];
        rblocks.extend_from_slice(&ref_mid_blocks);
        rblocks.extend_from_slice(&ref_leaf_blocks);
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
        let bc = self.superblock.block_count;
        Self::read_inode_via(&mut self.device, bc, &self.imap, id)
    }

    /// Read an Inode by LOGICAL id, resolving the id through an EXPLICIT inode
    /// map rather than `self.imap`. The single physical read primitive shared by
    /// the live mount (`imap == self.imap`) and a retained-root
    /// [`SnapshotView`] (`imap` == the snapshot's frozen map) — one code path,
    /// no parallel read logic. `device`/`imap` are borrowed as disjoint fields
    /// so a `&mut self` caller can pass `&mut self.device` and `&self.imap`.
    ///
    /// A SPILLED inode is reconstructed here: its overflow extents are read from
    /// the indirect blocks and appended to `chunks`, so the returned inode
    /// carries its COMPLETE extent list and every consumer above this layer is
    /// oblivious to the split.
    fn read_inode_via(
        device: &mut D,
        block_count: u64,
        imap: &[u64],
        id: u64,
    ) -> Result<Inode, FileSystemError> {
        let (inode, _index) = Self::read_inode_full_via(device, block_count, imap, id)?;
        Ok(inode)
    }

    /// Like [`read_inode_via`](Self::read_inode_via) but also returns the
    /// inode's INDIRECT INDEX extents (empty for an inline inode). Reachability
    /// (fsck) and snapshot enumeration need the index blocks — they are
    /// in-use metadata a naive extent walk would miss and report as leaked.
    fn read_inode_full_via(
        device: &mut D,
        block_count: u64,
        imap: &[u64],
        id: u64,
    ) -> Result<(Inode, ExtentList), FileSystemError> {
        let pb = imap.get(id as usize).copied().unwrap_or(0);
        if pb == 0 {
            return Err(FileSystemError::NotFound);
        }
        let mut block = alloc::vec![0u8; BLOCK_SIZE as usize];
        device.read_block(pb, &mut block)?;
        let (inode, index) = Self::reconstruct_inode(device, block_count, &block)?;
        if inode.id != id {
            return Err(FileSystemError::CorruptVolume("inode id mismatch"));
        }
        Ok((inode, index))
    }

    /// Turn a raw 4096 B inode block into (full inode, indirect index extents).
    /// For an inline inode the index is empty and `chunks` is complete as
    /// decoded. For a spilled inode the overflow extent list is read back from
    /// the indirect blocks (bounded against the volume span, BEFS-HARDEN) and
    /// appended to `chunks`; the index extents are returned for refcount
    /// accounting. Takes only the device + span, so both the live mount and the
    /// snapshot walk (which has no inode map, only a physical block) can call it.
    fn reconstruct_inode(
        device: &mut D,
        block_count: u64,
        block: &[u8],
    ) -> Result<(Inode, ExtentList), FileSystemError> {
        let (mut inode, trailer) = Inode::decode_block(block)?;
        let index = match trailer {
            None => ExtentList::new(),
            Some(t) => {
                if t.magic != crate::inode::INODE_SPILL_MAGIC {
                    return Err(FileSystemError::CorruptVolume("indirect trailer magic"));
                }
                let overflow_bytes = Self::read_from_extents_via(
                    device,
                    block_count,
                    &t.index,
                    0,
                    t.overflow_len,
                    t.overflow_len,
                )?;
                let overflow: ExtentList = crate::codec::deserialize(&overflow_bytes)?;
                inode.chunks.extend(overflow);
                if inode.chunks.len() as u64 != t.total_extents {
                    return Err(FileSystemError::CorruptVolume("indirect extent count"));
                }
                t.index
            }
        };
        Ok((inode, index))
    }

    /// Read the INDIRECT INDEX extents of the inode at physical block `pb`
    /// (empty if inline). Used to release an inode's indirect blocks when its
    /// block is dropped (CoW rewrite or unlink), so the extent list's overflow
    /// storage is never leaked.
    pub(crate) fn inode_index_extents_at(&mut self, pb: u64) -> Result<ExtentList, FileSystemError> {
        let mut block = alloc::vec![0u8; BLOCK_SIZE as usize];
        self.device.read_block(pb, &mut block)?;
        let (_inode, trailer) = Inode::decode_block(&block)?;
        Ok(trailer.map(|t| t.index).unwrap_or_default())
    }

    /// CoW-write an Inode: fresh block, remap, release the old block (and any
    /// indirect blocks the old version owned).
    ///
    /// If the inode's full extent list fits one block it is written inline,
    /// byte-identical to the pre-indirection format. Otherwise — on a
    /// spill-capable (v4) volume — the leading extents stay inline and the
    /// overflow spills to freshly allocated indirect blocks described by a
    /// trailer appended after the inode's bytes. On a v3 volume an oversized
    /// inode is rejected `InodeTooLarge`, exactly as before.
    fn write_inode(&mut self, inode: &Inode) -> Result<(), FileSystemError> {
        let idx = inode.id as usize;
        if idx >= self.imap.len() {
            return Err(FileSystemError::CorruptVolume("inode id beyond map"));
        }
        let old = self.imap[idx];

        let block = self.encode_inode_block(inode)?;
        let nb = self.alloc_block()?;
        self.write_fresh(nb, &block)?;

        if old != 0 {
            // Release the previous version's indirect blocks (if any), then the
            // inode block itself — the standard CoW decref of the old record.
            let old_index = self.inode_index_extents_at(old)?;
            self.decref_extents(&old_index);
            self.refmap.decref(old);
        }
        self.imap[idx] = nb;
        Ok(())
    }

    /// Build the 4096 B on-disk image of an inode, spilling its extent-list
    /// overflow to indirect blocks when it will not fit inline. Allocates and
    /// writes the indirect blocks as a side effect (they join the transaction).
    fn encode_inode_block(&mut self, inode: &Inode) -> Result<Vec<u8>, FileSystemError> {
        // v6: the meta trailer (parent, name, times) rides right after the
        // inode's unchanged bincode bytes. v3–v5: no trailer, bytes as before.
        let meta = if self.superblock.indexed() {
            inode.meta_bytes()
        } else {
            Vec::new()
        };
        // Fast path: the whole inode fits one block → inline, unchanged bytes.
        match inode.to_bytes() {
            Ok(bytes) if bytes.len() + meta.len() <= BLOCK_SIZE as usize => {
                let mut block = alloc::vec![0u8; BLOCK_SIZE as usize];
                block[..bytes.len()].copy_from_slice(&bytes);
                block[bytes.len()..bytes.len() + meta.len()].copy_from_slice(&meta);
                Ok(block)
            }
            Ok(_) => {
                if !self.superblock.spill_capable() {
                    return Err(InodeError::InodeTooLarge(0, BLOCK_SIZE).into());
                }
                self.encode_spilled_inode_block(inode, &meta)
            }
            Err(InodeError::InodeTooLarge(_, _)) => {
                if !self.superblock.spill_capable() {
                    // v3 volume: no spill format is declared — reject as before.
                    return Err(InodeError::InodeTooLarge(0, BLOCK_SIZE).into());
                }
                self.encode_spilled_inode_block(inode, &meta)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Encode a spilled inode: keep the leading extents inline, serialize the
    /// overflow into indirect blocks, and append the [`IndirectTrailer`].
    fn encode_spilled_inode_block(
        &mut self,
        inode: &Inode,
        meta: &[u8],
    ) -> Result<Vec<u8>, FileSystemError> {
        let k = inode.inline_extent_count_with(meta.len())?;
        if k >= inode.chunks.len() {
            // The inode is oversized for a reason other than extent count
            // (e.g. attributes) — spilling extents cannot help.
            return Err(InodeError::InodeTooLarge(0, BLOCK_SIZE).into());
        }

        // Serialize the overflow extents and write them to fresh indirect
        // blocks; the coalescing allocator makes `index` a few extents when the
        // blocks land contiguously.
        let overflow: ExtentList = inode.chunks[k..].to_vec();
        let overflow_bytes = crate::codec::serialize(&overflow)?;
        let index = self.allocate_and_write_extents(&overflow_bytes)?;

        let trailer = crate::inode::IndirectTrailer {
            magic: crate::inode::INODE_SPILL_MAGIC,
            total_extents: inode.chunks.len() as u64,
            overflow_len: overflow_bytes.len() as u64,
            index: index.clone(),
        };

        let mut stub = inode.clone();
        stub.chunks.truncate(k);
        let inode_bytes = crate::codec::serialize(&stub)?;
        let trailer_bytes = crate::codec::serialize(&trailer)?;

        if inode_bytes.len() + meta.len() + trailer_bytes.len() > BLOCK_SIZE as usize {
            // The index fragmented past the reserve — undo the just-written
            // indirect blocks so nothing leaks, and fail gracefully.
            self.decref_extents(&index);
            return Err(InodeError::InodeTooLarge(
                inode_bytes.len() + meta.len() + trailer_bytes.len(),
                BLOCK_SIZE,
            )
            .into());
        }

        let mut block = alloc::vec![0u8; BLOCK_SIZE as usize];
        let mut off = 0;
        for part in [&inode_bytes[..], meta, &trailer_bytes[..]] {
            block[off..off + part.len()].copy_from_slice(part);
            off += part.len();
        }
        Ok(block)
    }

    /// Allocate the next logical inode id and CoW-write a fresh inode there.
    /// `parent`/`name` are the v6 parent pointer (0/`None` for an unnamed
    /// object); every timestamp is stamped now.
    fn create_inode_inner(
        &mut self,
        kind: FileKind,
        attributes: BTreeMap<String, AttributeValue>,
        parent: u64,
        name: Option<&str>,
    ) -> Result<u64, FileSystemError> {
        let id = self.imap.len() as u64;
        if (id + 1).div_ceil(IMAP_ENTRIES_PER_LEAF) > IMAP_MAX_LEAVES {
            return Err(FileSystemError::NoSpace);
        }
        self.imap.push(0);
        let mut inode = Inode::new(id, kind);
        inode.attributes = attributes;
        inode.parent = parent;
        inode.name = name.map(String::from);
        stamp_all(&mut inode);
        self.write_inode(&inode)?;
        Ok(id)
    }

    /// Create a bare File inode (public API; one transaction).
    pub fn create_inode(
        &mut self,
        attributes: BTreeMap<String, AttributeValue>,
    ) -> Result<u64, FileSystemError> {
        let facts: Vec<(String, AttributeValue)> =
            attributes.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let id = self.create_inode_inner(FileKind::File, attributes, 0, None)?;
        // Index the initial attributes exactly as `set_attribute` would (the
        // pre-v6 path never indexed them — a query could not find a bare
        // `create_inode` object by its creation attributes).
        let facts: Vec<IndexFact> = facts.iter().map(|(k, v)| IndexFact::new(k, v, id)).collect();
        self.index_apply(&[], &facts)?;
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
        stamp_data(&mut inode);
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
        Self::read_data_via(
            &mut self.device,
            self.superblock.block_count,
            &self.imap,
            inode_id,
            offset,
            length,
        )
    }

    /// Read data from a logical inode resolved through an EXPLICIT inode map —
    /// the shared body of the live [`read_data`](Self::read_data) and a
    /// [`SnapshotView`] read. Reads only; `imap == self.imap` gives the live
    /// object, a snapshot's frozen map gives the retained bytes.
    fn read_data_via(
        device: &mut D,
        block_count: u64,
        imap: &[u64],
        inode_id: u64,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, FileSystemError> {
        let inode = Self::read_inode_via(device, block_count, imap, inode_id)?;
        Self::read_from_extents_via(device, block_count, &inode.chunks, offset, length, inode.size)
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
        Self::read_from_extents_via(
            &mut self.device,
            self.superblock.block_count,
            chunks,
            offset,
            length,
            total_size,
        )
    }

    /// The extent-walking read body, parameterized on the device and volume
    /// span rather than `&self` — so a [`SnapshotView`] reuses the exact same
    /// bounded reader (BEFS-HARDEN bounds are on `block_count`, disk-derived).
    fn read_from_extents_via(
        device: &mut D,
        block_count: u64,
        chunks: &ExtentList,
        offset: u64,
        length: u64,
        total_size: u64,
    ) -> Result<Vec<u8>, FileSystemError> {
        let volume_bytes = block_count.saturating_mul(BLOCK_SIZE);
        if total_size > volume_bytes {
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

            if physical_block >= block_count {
                return Err(FileSystemError::CorruptVolume("extent points past volume"));
            }

            let block_offset = (current_offset % BLOCK_SIZE) as usize;
            let to_read = core::cmp::min(
                BLOCK_SIZE as usize - block_offset,
                (to_read_total - read_so_far) as usize,
            );

            let mut block_buf = alloc::vec![0u8; BLOCK_SIZE as usize];
            device.read_block(physical_block, &mut block_buf)?;

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
        Self::ls_via(
            &mut self.device,
            self.superblock.block_count,
            &self.imap,
            inode_id,
        )
    }

    /// List a directory resolved through an EXPLICIT inode map — the shared
    /// body of live [`ls`](Self::ls) and a [`SnapshotView`] listing.
    fn ls_via(
        device: &mut D,
        block_count: u64,
        imap: &[u64],
        inode_id: u64,
    ) -> Result<Vec<DirEntry>, FileSystemError> {
        let inode = Self::read_inode_via(device, block_count, imap, inode_id)?;
        if inode.kind != FileKind::Directory {
            return Err(FileSystemError::NotADirectory);
        }
        if inode.size == 0 {
            return Ok(Vec::new());
        }
        let data = Self::read_data_via(device, block_count, imap, inode_id, 0, inode.size)?;
        let entries: Vec<DirEntry> = crate::codec::deserialize(&data)?;
        Ok(entries)
    }

    pub fn mkdir(&mut self, parent_id: u64, name: String) -> Result<u64, FileSystemError> {
        self.add_entry(parent_id, name, FileKind::Directory)
    }

    pub fn create_file(&mut self, parent_id: u64, name: String) -> Result<u64, FileSystemError> {
        self.add_entry(parent_id, name, FileKind::File)
    }

    /// Bulk create+write path: stage many new files under ONE parent
    /// directory and land them in a SINGLE transaction. This is the vectored
    /// create/write API the VAIRE-2 baseline found missing. Where N individual
    /// `create_file` + `write_data` + M × `set_attribute` calls each
    /// re-serialize the parent directory and the attribute catalog and flip the
    /// root once per file (the 242-flip cold-sync regime — 97 % of the wall),
    /// this reads the parent directory once, folds every file's attributes into
    /// its creation inode write, appends all catalog entries in one pass, and
    /// rewrites the parent directory and catalog exactly ONCE.
    ///
    /// **Transaction shape.** Every staged create/write becomes visible
    /// together at the batch's single commit, or none does. With autocommit ON
    /// (default) the whole batch is one root flip; with autocommit OFF the batch
    /// stages into the caller's larger transaction and does not commit — the way
    /// a whole-tree sync drives many `create_files_batch` calls (one per
    /// directory) under a single outer [`commit`](Self::commit). Either way a
    /// power cut between staging and the flip leaves the mounted image at
    /// exactly the last committed root.
    ///
    /// **Failure = unwind to the committed root.** On ANY error mid-batch (a
    /// name collision, a full volume, an oversized inode) the whole batch is
    /// unwound via [`Self::txn_unwind`], which reloads ground truth from the
    /// committed root on disk — no partial file, name, or catalog entry
    /// survives, and the allocator is poison-closed if even the reload fails
    /// (the K8b thaw-unwind precedent). NOTE: with autocommit OFF this unwinds
    /// the caller's ENTIRE outer transaction — everything staged since the last
    /// commit is discarded, not just this batch. There is no partial-commit-
    /// then-recover; a whole-tree sync that fails mid-way restarts from the
    /// last committed root by design. A name already present in the parent, or
    /// duplicated WITHIN the batch, fails closed with
    /// [`FileSystemError::FileExists`]; the mounted image is a true no-op.
    ///
    /// **Snapshots compose.** A `snapshot_create` taken AFTER a batch retains
    /// the whole batch (one commit = one atomic root); refcount/reclaim
    /// accounting is identical to the per-op path because the batch drives the
    /// same CoW primitives (`create_inode` / `write_data` / directory + catalog
    /// rewrites) — only the transaction boundary and metadata-churn are batched.
    ///
    /// Every child is a `File`. Returns the new logical inode ids in the order
    /// the files were supplied.
    pub fn create_files_batch(
        &mut self,
        parent_id: u64,
        files: Vec<BatchFile>,
    ) -> Result<Vec<u64>, FileSystemError> {
        let ids = match self.create_files_batch_inner(parent_id, files) {
            Ok(ids) => ids,
            Err(e) => {
                self.txn_unwind();
                return Err(e);
            }
        };
        // An empty batch stages nothing — a true no-op, no root flip.
        if ids.is_empty() {
            return Ok(ids);
        }
        if let Err(e) = self.maybe_commit() {
            self.txn_unwind();
            return Err(e);
        }
        Ok(ids)
    }

    /// Stage the batch into the current transaction WITHOUT committing or
    /// unwinding — the fallible body of [`create_files_batch`](Self::create_files_batch),
    /// which owns the commit/unwind envelope.
    fn create_files_batch_inner(
        &mut self,
        parent_id: u64,
        files: Vec<BatchFile>,
    ) -> Result<Vec<u64>, FileSystemError> {
        let parent_inode = self.read_inode(parent_id)?;
        if parent_inode.kind != FileKind::Directory {
            return Err(FileSystemError::NotADirectory);
        }

        if files.is_empty() {
            return Ok(Vec::new());
        }

        let mut entries = if parent_inode.size > 0 {
            self.ls(parent_id)?
        } else {
            Vec::new()
        };

        // Every file's index facts are collected and applied ONCE below (one
        // catalog-record rewrite for the whole batch on v6, one flat-list
        // rewrite on v3–v5).
        let mut facts: Vec<IndexFact> = Vec::new();

        let mut new_ids = Vec::with_capacity(files.len());
        for f in files {
            // Collision check covers both existing names and earlier files in
            // THIS batch (already pushed into `entries`); a hit unwinds to a
            // true no-op via the caller.
            if entries.iter().any(|e| e.name == f.name) {
                return Err(FileSystemError::FileExists);
            }
            let id = self.create_file_with_attrs_inner(&f.attributes, parent_id, &f.name, &mut facts)?;
            if !f.data.is_empty() {
                self.write_data_inner(id, 0, &f.data)?;
            }
            entries.push(DirEntry {
                name: f.name,
                inode_id: id,
                kind: FileKind::File,
            });
            new_ids.push(id);
        }

        // ONE parent-directory rewrite for the whole batch.
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        let dir_data = crate::codec::serialize(&entries)?;
        self.rewrite_data_inner(parent_id, &dir_data)?;

        // ONE index application for the whole batch.
        self.index_apply(&[], &facts)?;

        Ok(new_ids)
    }

    /// Create one `File` inode with its attributes folded into a SINGLE inode
    /// write, appending each attribute's catalog entry to `catalog` (which the
    /// batch rewrites once). Small attributes ride inline; large ones spill to
    /// fresh extents — the same split [`set_attribute`](Self::set_attribute)
    /// makes. Stages only; the caller commits.
    fn create_file_with_attrs_inner(
        &mut self,
        attributes: &BTreeMap<String, AttributeValue>,
        parent_id: u64,
        name: &str,
        facts: &mut Vec<IndexFact>,
    ) -> Result<u64, FileSystemError> {
        let id = self.imap.len() as u64;
        if (id + 1).div_ceil(IMAP_ENTRIES_PER_LEAF) > IMAP_MAX_LEAVES {
            return Err(FileSystemError::NoSpace);
        }
        self.imap.push(0);

        let mut inode = Inode::new(id, FileKind::File);
        inode.parent = parent_id;
        inode.name = Some(String::from(name));
        stamp_all(&mut inode);
        for (key, value) in attributes {
            let is_large = match value {
                AttributeValue::Vector(v) => v.len() > 64, // > 256 bytes
                AttributeValue::Blob(b) => b.len() > 256,
                AttributeValue::String(s) => s.len() > 256,
                _ => false,
            };
            if is_large {
                let data = crate::codec::serialize(value)?;
                let extents = self.allocate_and_write_extents(&data)?;
                inode.large_attributes.insert(key.clone(), extents);
            } else {
                inode.attributes.insert(key.clone(), value.clone());
            }
            facts.push(IndexFact::new(key, value, id));
        }
        self.write_inode(&inode)?;
        Ok(id)
    }

    /// Resolves a path string to an Inode ID.
    pub fn resolve_path(&mut self, path: &str) -> Result<u64, FileSystemError> {
        Self::resolve_path_via(
            &mut self.device,
            self.superblock.block_count,
            &self.imap,
            self.superblock.root_inode,
            path,
        )
    }

    /// Resolve a path through an EXPLICIT inode map — the shared body of live
    /// [`resolve_path`](Self::resolve_path) and a [`SnapshotView`] lookup.
    fn resolve_path_via(
        device: &mut D,
        block_count: u64,
        imap: &[u64],
        root_inode: u64,
        path: &str,
    ) -> Result<u64, FileSystemError> {
        let path = path.trim_start_matches('/');
        if path.is_empty() {
            return Ok(root_inode);
        }

        let parts: Vec<&str> = path.split('/').collect();
        let mut current_id = root_inode;

        for part in parts {
            if part.is_empty() {
                continue;
            }

            let entries = Self::ls_via(device, block_count, imap, current_id)?;
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

        let new_id = self.create_inode_inner(kind, BTreeMap::new(), parent_id, Some(&name))?;

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

        // The value being replaced (if any) leaves the index: read it BEFORE
        // its spilled extents are released.
        let old = self.attribute_of(&inode, &key)?;

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
            inode.large_attributes.remove(&key);
        }
        stamp_meta(&mut inode);

        self.write_inode(&inode)?;
        let removes: Vec<IndexFact> = old
            .iter()
            .map(|v| IndexFact::new(&key, v, inode_id))
            .collect();
        self.index_apply(&removes, &[IndexFact::new(&key, &value, inode_id)])?;
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
        Self::get_attribute_via(
            &mut self.device,
            self.superblock.block_count,
            &self.imap,
            inode_id,
            key,
        )
    }

    /// Read one attribute of an inode resolved through an EXPLICIT inode map —
    /// the shared body of live [`get_attribute`](Self::get_attribute) and a
    /// [`SnapshotView`] attribute read (attrs AS-OF the snapshot).
    fn get_attribute_via(
        device: &mut D,
        block_count: u64,
        imap: &[u64],
        inode_id: u64,
        key: &str,
    ) -> Result<Option<AttributeValue>, FileSystemError> {
        let inode = Self::read_inode_via(device, block_count, imap, inode_id)?;

        if let Some(val) = inode.attributes.get(key) {
            return Ok(Some(val.clone()));
        }

        if let Some(extents) = inode.large_attributes.get(key) {
            let total_size = checked_extent_total(extents)?;
            let data = Self::read_from_extents_via(device, block_count, extents, 0, total_size, total_size)?;
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

        // 1. Scrub the attribute index (by the inode's own keys).
        let removes = self.facts_of(&inode)?;
        self.index_apply(&removes, &[])?;

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
            // Release the inode's indirect (extent-spill) blocks before the
            // inode block itself — otherwise a spilled file's overflow storage
            // would leak on unlink.
            let index = self.inode_index_extents_at(pb)?;
            self.decref_extents(&index);
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

    /// RMDIR (SO18): remove the EMPTY directory `name` from directory
    /// `parent_id` — the directory entry, the child's own (empty) entry-list
    /// blocks, its spilled attributes, its inode block and its inode-map slot,
    /// all in ONE CoW transaction, exactly as [`unlink`](Self::unlink) does for
    /// a file. Returns the freed (logical) inode id.
    ///
    /// # Why this is `unlink`'s twin and not a new mechanism
    /// A UnaFS directory IS an object with a serialized `Vec<DirEntry>` for its
    /// data; nothing about unhooking its NAME differs from unhooking a file's.
    /// The only thing `unlink` could not decide is whether removing it is
    /// SAFE, so it refused every directory unconditionally
    /// ([`IsADirectory`](FileSystemError::IsADirectory)). This verb answers
    /// that question — the directory must be EMPTY — and then runs the same
    /// removal. There is no orphan window: the emptiness test and the removal
    /// are both inside the one transaction that the closing
    /// [`maybe_commit`](Self::maybe_commit) flips.
    ///
    /// # Refusals
    /// * [`NotFound`](FileSystemError::NotFound) — no such name in `parent_id`.
    /// * [`NotADirectory`](FileSystemError::NotADirectory) — the name is a file
    ///   or a symlink (use [`unlink`](Self::unlink)).
    /// * [`DirectoryNotEmpty`](FileSystemError::DirectoryNotEmpty) — the child
    ///   still holds entries. Recursion is a CALLER's verb (`rm -r` composes
    ///   `read_dir` + `unlink` + this one); the primitive removes exactly one
    ///   empty directory so a half-walked tree can never be half-deleted here.
    /// * [`IsADirectory`](FileSystemError::IsADirectory) — the target is the
    ///   VOLUME ROOT. The root has no parent entry to unhook and the superblock
    ///   names it, so it is unremovable by construction; the guard is kept
    ///   anyway because a corrupt parent listing could otherwise name it.
    ///
    /// # Open-handle semantics
    /// Identical to [`unlink`](Self::unlink): the id is invalidated at once and
    /// logical ids are never recycled, so a stale id can never alias a new
    /// object.
    ///
    /// # ACL
    /// Deliberately NOT evaluated here, exactly as `unlink` does not evaluate
    /// it: the U6 `owner`/`grants:<principal>` rows are attributes this crate
    /// stores but does not interpret, and the ONE write-side evaluator lives in
    /// the kernel VFS adapter (`fs/vfs.rs::native_write_authz`), which authorizes
    /// BEFORE calling this. A second copy of the rule here is how two divergent
    /// ACLs happen.
    pub fn rmdir(&mut self, parent_id: u64, name: &str) -> Result<u64, FileSystemError> {
        let mut entries = self.ls(parent_id)?;
        let pos = entries
            .iter()
            .position(|e| e.name == name)
            .ok_or(FileSystemError::NotFound)?;
        if entries[pos].kind != FileKind::Directory {
            return Err(FileSystemError::NotADirectory);
        }
        let inode_id = entries[pos].inode_id;
        if inode_id == self.superblock.root_inode {
            return Err(FileSystemError::IsADirectory);
        }

        // EMPTINESS IS PROVEN, NOT ASSUMED FROM `size`. A directory that once
        // held entries and had them all removed carries a non-zero `size` (the
        // serialized EMPTY vector), so a `size == 0` test would refuse a
        // legitimately empty directory. `ls` is the one reader that answers the
        // question the verb actually asks.
        if !self.ls(inode_id)?.is_empty() {
            return Err(FileSystemError::DirectoryNotEmpty);
        }

        // From here down this is `unlink`'s body verbatim, on a directory inode.
        let inode = self.read_inode(inode_id)?;

        // 1. Scrub the attribute index (a directory carries owner/grants rows).
        let removes = self.facts_of(&inode)?;
        self.index_apply(&removes, &[])?;

        // 2. Unhook the name from the parent.
        entries.remove(pos);
        let dir_data = crate::codec::serialize(&entries)?;
        self.rewrite_data_inner(parent_id, &dir_data)?;

        // 3. Release everything the inode owned — including the blocks holding
        //    its own (now empty) entry list.
        for extents in inode.large_attributes.values() {
            self.decref_extents(extents);
        }
        self.decref_extents(&inode.chunks);
        let pb = self.imap.get(inode_id as usize).copied().unwrap_or(0);
        if pb != 0 {
            let index = self.inode_index_extents_at(pb)?;
            self.decref_extents(&index);
            self.refmap.decref(pb);
        }
        self.imap_clear(inode_id);

        self.maybe_commit()?;

        #[cfg(feature = "std")]
        {
            let msg = SMessage::FileEvent {
                path: format!("inode:{}", inode_id),
                event: format!("DirRemoved:{}", name),
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

        // v6: the moved inode's parent pointer + name follow it (same
        // transaction — a power cut leaves the old link or the new one).
        if self.superblock.indexed() {
            let mut inode = self.read_inode(moved.inode_id)?;
            inode.parent = new_parent_id;
            inode.name = Some(String::from(new_name));
            stamp_meta(&mut inode);
            self.write_inode(&inode)?;
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

        let old = self.attribute_of(&inode, key)?;
        let removes: Vec<IndexFact> = old.iter().map(|v| IndexFact::new(key, v, inode_id)).collect();
        self.index_apply(&removes, &[])?;

        inode.attributes.remove(key);
        let spilled = inode.large_attributes.remove(key);
        stamp_meta(&mut inode);
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

    /// The snapshot index (retained roots). v1 policy cap: [`SNAPSHOT_CAP`].
    pub fn snapshot_index(&mut self) -> Result<Vec<SnapshotEntry>, FileSystemError> {
        let inode = self.read_inode(SNAP_INDEX_INODE_ID)?;
        let data = self.read_data(SNAP_INDEX_INODE_ID, 0, inode.size)?;
        if data.is_empty() {
            return Ok(Vec::new());
        }
        Ok(crate::codec::deserialize(&data)?)
    }

    /// Load a retained root's INODE MAP (logical id → physical inode block) into
    /// RAM, exactly as [`load_committed`](Self::load_committed) loads the live
    /// map, but from a snapshot's `(imap_block, imap_leaves)` rather than the
    /// active root. The returned vector is indexed by logical inode id;
    /// unallocated slots are `0` (imap leaves are zero-filled beyond the last
    /// retained id, so trailing zeros are harmless — a lookup there resolves to
    /// `NotFound`). Every pointer is bounded against the volume span (the
    /// on-disk imap is untrusted input): an out-of-range pointer is a
    /// `CorruptVolume` error, never a slice panic.
    fn snapshot_imap(
        &mut self,
        imap_block: u64,
        imap_leaves: u64,
    ) -> Result<Vec<u64>, FileSystemError> {
        let block_count = self.superblock.block_count;
        if imap_block == 0 || imap_block >= block_count {
            return Err(FileSystemError::CorruptVolume("snapshot imap index out of bounds"));
        }
        if imap_leaves == 0 || imap_leaves > IMAP_MAX_LEAVES {
            return Err(FileSystemError::CorruptVolume(
                "snapshot imap leaf count out of range",
            ));
        }
        let cap = usize::try_from(imap_leaves.saturating_mul(IMAP_ENTRIES_PER_LEAF))
            .map_err(|_| StorageError::AllocRefused(imap_leaves))?;
        let mut imap: Vec<u64> = Vec::new();
        imap.try_reserve_exact(cap)
            .map_err(|_| StorageError::AllocRefused(imap_leaves))?;

        let mut index = alloc::vec![0u8; BLOCK_SIZE as usize];
        self.device.read_block(imap_block, &mut index)?;
        let mut leaf = alloc::vec![0u8; BLOCK_SIZE as usize];
        for l in 0..imap_leaves {
            let ptr = u64::from_le_bytes(
                index[(l * 8) as usize..(l * 8 + 8) as usize]
                    .try_into()
                    .unwrap(),
            );
            if ptr == 0 || ptr >= block_count {
                return Err(FileSystemError::CorruptVolume("snapshot imap leaf out of bounds"));
            }
            self.device.read_block(ptr, &mut leaf)?;
            for e in 0..IMAP_ENTRIES_PER_LEAF {
                let entry = u64::from_le_bytes(
                    leaf[(e * 8) as usize..(e * 8 + 8) as usize]
                        .try_into()
                        .unwrap(),
                );
                if entry >= block_count {
                    return Err(FileSystemError::CorruptVolume("snapshot imap entry out of bounds"));
                }
                imap.push(entry);
            }
        }
        Ok(imap)
    }

    /// Open a retained root (snapshot) by its `generation` stamp for READING —
    /// the K8c read path. Returns a [`SnapshotView`], a strictly read-only
    /// handle: it resolves paths, lists directories, reads data, and reads
    /// attributes AS THEY WERE when the snapshot was taken, and it has NO write
    /// method at all — read-only is a property of the type, not a runtime check.
    ///
    /// The view borrows the mount for its lifetime and reads through the
    /// snapshot's frozen inode map (loaded once here), sharing the exact same
    /// bounded read primitives (`read_inode_via`/`ls_via`/`read_data_via`/…) the
    /// live mount uses — one read code path, live and frozen. Reading a snapshot
    /// NEVER touches refcounts, the reclaim queue, or the live root: the view
    /// holds its own map and only issues `read_block`s.
    ///
    /// # Authority (K8c ruling, not enforced here)
    /// The crate view is pure bytes — it applies NO access control. The K8c
    /// current-ACL rule ("a principal that cannot read the live object cannot
    /// read any snapshot of it; revocation reaches the past") is enforced one
    /// layer up, at the kernel verb / ACL seam, by checking the LIVE object's
    /// current ACL before handing bytes out. Keeping policy out of the crate is
    /// deliberate: there is exactly one enforcement path (the live one), and the
    /// snapshot read defers to it.
    ///
    /// Fails with [`FileSystemError::SnapshotNotFound`] if no retained root
    /// carries that generation (e.g. it was dropped) — a dangling view is
    /// unrepresentable.
    pub fn open_snapshot(&mut self, generation: u64) -> Result<SnapshotView<'_, D>, FileSystemError> {
        let index = self.snapshot_index()?;
        let entry = index
            .into_iter()
            .find(|e| e.generation == generation)
            .ok_or(FileSystemError::SnapshotNotFound(generation))?;
        let imap = self.snapshot_imap(entry.imap_block, entry.imap_leaves)?;
        let block_count = self.superblock.block_count;
        let root_inode = self.superblock.root_inode;
        Ok(SnapshotView {
            device: &mut self.device,
            block_count,
            root_inode,
            imap,
            generation,
        })
    }

    /// Every block a root reaches THROUGH ITS INODE MAP: the imap index block,
    /// its leaf blocks, and — for every allocated inode — the inode block plus
    /// every data/spilled-attribute extent block it owns. This is the exact
    /// set a retained snapshot holds a reference to; refcount correctness under
    /// retention rests on [`snapshot_create`] increfing precisely this set and
    /// [`snapshot_drop`]'s drain decrefing precisely this set (perfect
    /// symmetry — a block shared by the live tree and/or other snapshots keeps
    /// exactly one refcount per referencing root, so drop frees a block iff no
    /// remaining root reaches it). Refcount- and refmap-map blocks are NOT
    /// included: a snapshot retains the DATA tree, not the allocator (the live
    /// refmap is rewritten every commit and is never snapshotted).
    ///
    /// Bounded against the volume span (the on-disk imap is untrusted input);
    /// a pointer out of range is a `CorruptVolume` error, never a slice panic,
    /// so a partial walk never drives an incref/decref.
    pub(crate) fn snapshot_blocks(
        &mut self,
        imap_block: u64,
        imap_leaves: u64,
    ) -> Result<Vec<u64>, FileSystemError> {
        let block_count = self.superblock.block_count;
        if imap_block == 0 || imap_block >= block_count {
            return Err(FileSystemError::CorruptVolume("snapshot imap index out of bounds"));
        }
        if imap_leaves == 0 || imap_leaves > IMAP_MAX_LEAVES {
            return Err(FileSystemError::CorruptVolume("snapshot imap leaf count out of range"));
        }
        let mut blocks = Vec::new();
        blocks.push(imap_block);
        let mut index = alloc::vec![0u8; BLOCK_SIZE as usize];
        self.device.read_block(imap_block, &mut index)?;
        let mut leaf = alloc::vec![0u8; BLOCK_SIZE as usize];
        for l in 0..imap_leaves {
            let ptr = u64::from_le_bytes(
                index[(l * 8) as usize..(l * 8 + 8) as usize]
                    .try_into()
                    .unwrap(),
            );
            if ptr == 0 || ptr >= block_count {
                return Err(FileSystemError::CorruptVolume("snapshot imap leaf out of bounds"));
            }
            blocks.push(ptr);
            self.device.read_block(ptr, &mut leaf)?;
            for e in 0..IMAP_ENTRIES_PER_LEAF {
                let inode_pb = u64::from_le_bytes(
                    leaf[(e * 8) as usize..(e * 8 + 8) as usize]
                        .try_into()
                        .unwrap(),
                );
                // Unallocated inode-map slots are zero (imap leaves are
                // zero-filled beyond the last live id) — nothing to reach.
                if inode_pb == 0 {
                    continue;
                }
                if inode_pb >= block_count {
                    return Err(FileSystemError::CorruptVolume(
                        "snapshot imap entry out of bounds",
                    ));
                }
                blocks.push(inode_pb);
                let mut ib = alloc::vec![0u8; BLOCK_SIZE as usize];
                self.device.read_block(inode_pb, &mut ib)?;
                // Reconstruct so a SPILLED inode contributes its overflow DATA
                // blocks (in the completed `chunks`) and its INDIRECT index
                // blocks — a snapshot references every block its root reaches,
                // and missing either would let a later drop free live storage.
                let (inode, index) = Self::reconstruct_inode(&mut self.device, block_count, &ib)?;
                Self::push_extent_blocks(&inode.chunks, block_count, &mut blocks)?;
                Self::push_extent_blocks(&index, block_count, &mut blocks)?;
                for extents in inode.large_attributes.values() {
                    Self::push_extent_blocks(extents, block_count, &mut blocks)?;
                }
                // v6: the catalog inode owns its two index trees' nodes — a
                // snapshot pins the index AS-OF, exactly like file data.
                if inode.id == CATALOG_INODE_ID && self.superblock.indexed() {
                    blocks.extend(Self::catalog_tree_blocks_via(
                        &mut self.device,
                        block_count,
                        &inode,
                    )?);
                }
            }
        }
        Ok(blocks)
    }

    /// Append every block an extent list covers to `out`, bounded to the
    /// volume span (a hostile extent errs rather than pointing past the disk).
    fn push_extent_blocks(
        extents: &ExtentList,
        block_count: u64,
        out: &mut Vec<u64>,
    ) -> Result<(), FileSystemError> {
        for extent in extents {
            let count = extent.length.div_ceil(BLOCK_SIZE);
            for i in 0..count {
                let pb = extent
                    .physical_block
                    .checked_add(i)
                    .ok_or(FileSystemError::CorruptVolume("snapshot extent overflows"))?;
                if pb >= block_count {
                    return Err(FileSystemError::CorruptVolume(
                        "snapshot extent points past volume",
                    ));
                }
                out.push(pb);
            }
        }
        Ok(())
    }

    /// Retain the CURRENT committed root as a snapshot (design V1): record a
    /// generation-stamped [`SnapshotEntry`] (name + creator principal +
    /// timestamp, K6 typed attrs) in the on-disk snapshot index, and — the
    /// security core — incref EVERY block the retained root reaches so no
    /// later mutation can reallocate a block the snapshot still lives on. The
    /// incref and the index write land in ONE commit (atomic: a crash yields
    /// the volume with or without the snapshot, never a half-retained tree).
    ///
    /// v1 policy caps retention at [`SNAPSHOT_CAP`] entries — refused cleanly
    /// with [`FileSystemError::SnapshotCapReached`], no format change (the
    /// index is an unbounded growable object; the cap is policy).
    ///
    /// Returns the snapshot's generation stamp — its unique, never-recycled id
    /// (monotone commit generation), the key [`snapshot_drop`] takes.
    ///
    /// **Failure is clean (lens A fix):** on ANY error after the tree-wide
    /// incref (e.g. `NoSpace` persisting the index or committing on a tight
    /// volume — a supported regime) the whole in-RAM transaction is unwound
    /// via [`Self::txn_unwind`], which RELOADS the imap/refmap/map-block state
    /// from the committed root on disk (ground truth — the root never flipped
    /// on the failing path), so the mount stays refcount-consistent and fully
    /// usable regardless of any prior in-RAM residue.
    pub fn snapshot_create(
        &mut self,
        name: String,
        creator: String,
        timestamp: u64,
    ) -> Result<u64, FileSystemError> {
        match self.snapshot_create_inner(name, creator, timestamp) {
            Ok(generation) => {
                self.stats.snapshots_created += 1;
                Ok(generation)
            }
            Err(e) => {
                self.txn_unwind();
                Err(e)
            }
        }
    }

    fn snapshot_create_inner(
        &mut self,
        name: String,
        creator: String,
        timestamp: u64,
    ) -> Result<u64, FileSystemError> {
        let mut index = self.snapshot_index()?;
        if index.len() >= SNAPSHOT_CAP {
            return Err(FileSystemError::SnapshotCapReached(SNAPSHOT_CAP));
        }

        // Capture the committed root (on disk in full right now — autocommit
        // leaves no in-flight txn between public ops).
        let generation = self.root.generation;
        let imap_block = self.root.imap_block;
        let imap_leaves = self.root.imap_leaves;

        // The security core: pin every block the retained root reaches. Walk
        // and incref BEFORE the index write so the commit that persists this
        // snapshot (which decrefs the old live imap blocks) leaves the shared
        // blocks at a refcount the snapshot still holds.
        let blocks = self.snapshot_blocks(imap_block, imap_leaves)?;
        for &b in &blocks {
            self.refmap.incref(b);
        }

        index.push(SnapshotEntry {
            generation,
            imap_block,
            imap_leaves,
            name,
            creator,
            timestamp,
        });
        let bytes = crate::codec::serialize(&index)?;
        self.rewrite_data_inner(SNAP_INDEX_INODE_ID, &bytes)?;
        self.commit()?;
        Ok(generation)
    }

    /// Unwind a FAILED (uncommitted) transaction by RELOADING the in-RAM
    /// state (root record, inode map, refcount map, map-block lists) from the
    /// COMMITTED root on disk. The root never flipped on any failing path, so
    /// the on-disk committed tree is ground truth BY DEFINITION — this makes
    /// the unwind correct regardless of any residue prior operations left in
    /// RAM, structurally rather than by discipline.
    ///
    /// **Rejected alternative (lens A composition finding, 2026-07-16):** an
    /// entry-snapshot restore (save imap/map-blocks at txn entry + `thaw()`)
    /// was strictly WORSE when composed with an earlier un-unwound failed
    /// write (K8a residue: in-RAM imap pointing at fresh blocks backed only by
    /// `current` refcounts). Restoring that contaminated entry state while
    /// thaw discarded the backing refcounts left reachable blocks at refcount
    /// 0 in BOTH views — the allocator could then legally hand out live
    /// blocks and overwrite committed data. Reloading from disk cannot
    /// reproduce that class: it never trusts anything in RAM.
    ///
    /// If even the reload fails (device error mid-unwind), the mount is
    /// poisoned: fail closed by keeping the refmap fully thawed AND marking
    /// every block used in the current view, so no further allocation can hand
    /// out anything — reads still work, a remount recovers.
    fn txn_unwind(&mut self) {
        match Self::load_committed(&mut self.device, &self.superblock) {
            Ok(loaded) => {
                self.root = loaded.root;
                self.active_slot = loaded.active_slot;
                self.imap = loaded.imap;
                self.imap_blocks = loaded.imap_blocks;
                self.refmap_blocks = loaded.refmap_blocks;
                self.refmap = loaded.refmap;
            }
            Err(_) => {
                crate::warnlog::warn(
                    "[UNAFS] :: txn unwind reload failed — allocator poisoned closed until remount",
                );
                let all_used = alloc::vec![u32::MAX; self.superblock.block_count as usize];
                self.refmap.set_counts(&all_used);
            }
        }
        self.txn_blocks = 0;
    }

    /// Drop a retained snapshot by its `generation` stamp (design V2): remove
    /// the index entry and enqueue the snapshot's block set onto the persistent
    /// reclaim queue in ONE commit, then eagerly drain (v1 policy). Dropping
    /// NEVER frees a block directly — the drain decrefs, and a block survives
    /// iff the live root or another retained root still reaches it.
    ///
    /// Crash-safe: the index-removal + enqueue is one atomic transaction, so a
    /// power cut leaves either the intact snapshot or the entry safely on the
    /// queue for the next mount's drain (no block is lost, none double-freed).
    ///
    /// This is the destructive MECHANISM; owner-or-kernel authority
    /// ([`SnapshotEntry::drop_permitted`]) is enforced by the calling verb.
    pub fn snapshot_drop(&mut self, generation: u64) -> Result<(), FileSystemError> {
        self.snapshot_drop_enqueue(generation)?;
        // Eager drain (v1): decref the enqueued blocks, empty the queue, commit.
        self.reclaim_drain()
    }

    /// The FIRST, atomic half of [`snapshot_drop`]: remove the index entry and
    /// enqueue its block set in ONE commit, WITHOUT draining. After this
    /// returns the snapshot is gone from the index and its blocks sit on the
    /// persistent reclaim queue, still ref-held — exactly the crash state a
    /// power cut mid-drop leaves. Public so the crash-mid-drain path can be
    /// exercised directly (drop the mount here → the next mount's eager drain
    /// resumes and converges). Bumps the drop counter (the drain that follows,
    /// whether eager or on a later mount, does the freeing).
    ///
    /// Failure is clean: any error before the commit unwinds the in-RAM
    /// transaction ([`Self::txn_unwind`]) — same discipline as
    /// [`snapshot_create`](Self::snapshot_create).
    pub fn snapshot_drop_enqueue(&mut self, generation: u64) -> Result<(), FileSystemError> {
        match self.snapshot_drop_enqueue_inner(generation) {
            Ok(()) => {
                self.stats.snapshots_dropped += 1;
                Ok(())
            }
            Err(e) => {
                self.txn_unwind();
                Err(e)
            }
        }
    }

    fn snapshot_drop_enqueue_inner(&mut self, generation: u64) -> Result<(), FileSystemError> {
        let mut index = self.snapshot_index()?;
        let pos = index
            .iter()
            .position(|e| e.generation == generation)
            .ok_or(FileSystemError::SnapshotNotFound(generation))?;
        let entry = index.remove(pos);

        // The blocks the snapshot referenced (identical set to the create-time
        // incref — symmetry is the safety property).
        let blocks = self.snapshot_blocks(entry.imap_block, entry.imap_leaves)?;

        // Atomic: index loses the entry AND the queue gains it, one root flip.
        let index_bytes = crate::codec::serialize(&index)?;
        self.rewrite_data_inner(SNAP_INDEX_INODE_ID, &index_bytes)?;
        let mut queue = self.reclaim_queue()?;
        queue.push(ReclaimEntry { generation, blocks });
        let queue_bytes = crate::codec::serialize(&queue)?;
        self.rewrite_data_inner(RECLAIM_INODE_ID, &queue_bytes)?;
        self.commit()?;
        Ok(())
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
    ///
    /// Failure is clean: any error before the commit unwinds the in-RAM
    /// transaction ([`Self::txn_unwind`] reloads from the committed root) —
    /// the decrefs are discarded and the queue stays intact on disk and in
    /// RAM for a retry or the next mount.
    pub fn reclaim_drain(&mut self) -> Result<(), FileSystemError> {
        let r = self.reclaim_drain_inner();
        if r.is_err() {
            self.txn_unwind();
        }
        r
    }

    fn reclaim_drain_inner(&mut self) -> Result<(), FileSystemError> {
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

    /// Semantic query engine (F4, B302): parse, gather index candidates,
    /// verify every candidate against the inode's real values, and name each
    /// hit by its path. no_std-capable: the similarity path routes its
    /// floating-point `sqrt` through `libm`, so kernel (`no_std`) and host
    /// (`std`) builds score along the same code path. Hits are in ascending
    /// inode-id order.
    pub fn query(&mut self, query_str: &str) -> Result<Vec<QueryHit>, FileSystemError> {
        let matched = self.query_matches(query_str)?;
        let mut names: Option<BTreeMap<u64, String>> = None;
        let mut out = Vec::with_capacity(matched.len());
        for (inode, score) in matched {
            let path = if self.superblock.indexed() {
                self.path_of(inode.id)?
            } else {
                // v3–v5 carry no parent pointers: one name-tree walk per query.
                if names.is_none() {
                    names = Some(self.path_map()?);
                }
                names
                    .as_ref()
                    .and_then(|m| m.get(&inode.id).cloned())
                    .unwrap_or_default()
            };
            out.push(QueryHit { inode_id: inode.id, path, score });
        }
        Ok(out)
    }

    /// [`query`](Self::query) returning the matched inodes themselves (the
    /// pre-B302 shape: callers that read data/attributes off each hit).
    pub fn query_inodes(&mut self, query_str: &str) -> Result<Vec<(Inode, f32)>, FileSystemError> {
        self.query_matches(query_str)
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
        stamp_data(&mut inode);
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
}

impl<D: BlockDevice> Drop for UnaFS<D> {
    fn drop(&mut self) {
        // Under CoW every completed public op has already committed (unless
        // the caller disabled autocommit — the crash-simulation seam, whose
        // whole point is that dropping models a power cut). Just flush.
        let _ = self.device.flush();
    }
}

/// A strictly READ-ONLY handle onto a retained root (snapshot), obtained from
/// [`UnaFS::open_snapshot`]. It reads paths, directories, data, and attributes
/// AS THEY WERE at snapshot time, through the snapshot's frozen inode map — and
/// it exposes NO mutating method, so "a snapshot cannot be written" is a fact of
/// the type, not a runtime policy check (the K8c read-only constraint made
/// unrepresentable).
///
/// The view borrows the mount (`&mut UnaFS`, via its device) for its lifetime,
/// carrying its own frozen `imap`; every read routes through the SAME bounded
/// primitives the live mount uses (`UnaFS::*_via`). Reads never touch the
/// refcount map, the reclaim queue, or the live/active root — the view only
/// issues `read_block`s against blocks the snapshot pins.
///
/// Access control lives one layer up (the kernel verb / ACL seam enforces the
/// K8c current-ACL rule against the LIVE object); this handle is pure bytes.
pub struct SnapshotView<'a, D: BlockDevice> {
    device: &'a mut D,
    block_count: u64,
    root_inode: u64,
    /// The snapshot's frozen inode map (logical id → physical inode block).
    imap: Vec<u64>,
    generation: u64,
}

impl<'a, D: BlockDevice> SnapshotView<'a, D> {
    /// The generation stamp of the retained root this view reads.
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Resolve a path to a logical inode id AS OF the snapshot.
    pub fn resolve_path(&mut self, path: &str) -> Result<u64, FileSystemError> {
        UnaFS::<D>::resolve_path_via(
            self.device,
            self.block_count,
            &self.imap,
            self.root_inode,
            path,
        )
    }

    /// Read an inode AS OF the snapshot (size, kind, attributes at snapshot time).
    pub fn read_inode(&mut self, inode_id: u64) -> Result<Inode, FileSystemError> {
        UnaFS::<D>::read_inode_via(self.device, self.block_count, &self.imap, inode_id)
    }

    /// List a directory AS OF the snapshot.
    pub fn ls(&mut self, inode_id: u64) -> Result<Vec<DirEntry>, FileSystemError> {
        UnaFS::<D>::ls_via(self.device, self.block_count, &self.imap, inode_id)
    }

    /// Read file data AS OF the snapshot — the OLD bytes, even after the live
    /// object was overwritten or deleted (block-sharing + never-overwrite).
    pub fn read_data(
        &mut self,
        inode_id: u64,
        offset: u64,
        length: u64,
    ) -> Result<Vec<u8>, FileSystemError> {
        UnaFS::<D>::read_data_via(
            self.device,
            self.block_count,
            &self.imap,
            inode_id,
            offset,
            length,
        )
    }

    /// Read one attribute of an inode AS OF the snapshot.
    pub fn get_attribute(
        &mut self,
        inode_id: u64,
        key: &str,
    ) -> Result<Option<AttributeValue>, FileSystemError> {
        UnaFS::<D>::get_attribute_via(
            self.device,
            self.block_count,
            &self.imap,
            inode_id,
            key,
        )
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

// =========================================================================
// B302: the indexed catalog, the query planner, paths, timestamps
// =========================================================================

/// One query hit: the inode, its path, and its score (1.0 for boolean
/// predicates, the cosine similarity for a similarity predicate; `AND`
/// multiplies, `OR` takes the max).
///
/// `path` is absolute (`/a/b/c`, `/` for the root). It is EMPTY for an
/// object no directory names (a bare [`UnaFS::create_inode`]).
#[derive(Debug, Clone, PartialEq)]
pub struct QueryHit {
    pub inode_id: u64,
    pub path: String,
    pub score: f32,
}

/// What [`UnaFS::stat`] reports: the inode's identity, kind, size, parent
/// link and timestamps (unix seconds; all 0 on a pre-v6 volume, which has
/// nowhere to keep them).
#[derive(Debug, Clone, PartialEq)]
pub struct Stat {
    pub inode_id: u64,
    pub kind: FileKind,
    pub size: u64,
    pub parent: u64,
    pub ctime: u64,
    pub mtime: u64,
    pub atime: u64,
}

/// Longest parent chain [`UnaFS::path_of`] follows before calling the volume
/// corrupt (a cycle or a forged chain can never spin it forever).
pub const MAX_PATH_DEPTH: usize = 4096;

impl From<BtreeError> for FileSystemError {
    fn from(e: BtreeError) -> Self {
        match e {
            BtreeError::Storage(s) => FileSystemError::Storage(s),
            BtreeError::NoSpace => FileSystemError::NoSpace,
            BtreeError::BadMagic(_) => FileSystemError::CorruptVolume("index node: bad magic"),
            BtreeError::BadVersion(_, _) => FileSystemError::CorruptVolume("index node: bad version"),
            BtreeError::BadChecksum(_) => FileSystemError::CorruptVolume("index node: bad checksum"),
            BtreeError::Malformed(_, why) => FileSystemError::CorruptVolume(why),
            BtreeError::Corrupt(why) => FileSystemError::CorruptVolume(why),
            BtreeError::KeyTooLarge(_) => FileSystemError::CorruptVolume("index key too large"),
            BtreeError::ValueTooLarge(_) => FileSystemError::CorruptVolume("index value too large"),
            BtreeError::NodeOverflow(_) => FileSystemError::CorruptVolume("index node overflow"),
        }
    }
}

/// Stamp every timestamp (creation).
fn stamp_all(inode: &mut Inode) {
    let now = crate::clock::now();
    inode.ctime = now;
    inode.mtime = now;
    inode.atime = now;
}

/// Stamp a data change (POSIX: a data write changes mtime AND ctime).
fn stamp_data(inode: &mut Inode) {
    let now = crate::clock::now();
    inode.ctime = now;
    inode.mtime = now;
    inode.atime = now;
}

/// Stamp a metadata-only change (attributes, rename).
fn stamp_meta(inode: &mut Inode) {
    inode.ctime = crate::clock::now();
}

/// Where a query's candidates come from: the v6 trees, or the v3–v5 flat list.
enum CandidateSource {
    Tree(CatalogRecord),
    Flat(Vec<CatalogEntry>),
}

impl<D: BlockDevice> UnaFS<D> {
    /// Account index-node writes in the bench counters (the tree writes
    /// through its own store, not `write_fresh`).
    fn count_index_writes(&mut self, written: u64) {
        self.txn_blocks += written;
        self.stats.blocks_written += written;
    }

    /// The v6 catalog record (`None` on a v3–v5 volume, whose catalog is the
    /// flat list).
    pub fn catalog_record(&mut self) -> Result<Option<CatalogRecord>, FileSystemError> {
        if !self.superblock.indexed() {
            return Ok(None);
        }
        let id = self.superblock.catalog_inode;
        let inode = self.read_inode(id)?;
        let data = self.read_data(id, 0, inode.size)?;
        CatalogRecord::from_bytes(&data, self.superblock.block_count)
            .map(Some)
            .ok_or(FileSystemError::CorruptVolume("catalog record invalid"))
    }

    /// One attribute's value off an already-read inode (inline, or spilled —
    /// read from its extents).
    fn attribute_of(
        &mut self,
        inode: &Inode,
        key: &str,
    ) -> Result<Option<AttributeValue>, FileSystemError> {
        if let Some(v) = inode.attributes.get(key) {
            return Ok(Some(v.clone()));
        }
        if let Some(extents) = inode.large_attributes.get(key) {
            let total = checked_extent_total(extents)?;
            let data = self.read_from_extents(extents, 0, total, total)?;
            let val: AttributeValue = crate::codec::deserialize(&data)
                .map_err(|_| FileSystemError::InvalidAttributeData)?;
            return Ok(Some(val));
        }
        Ok(None)
    }

    /// Every index fact an inode contributes (one per attribute).
    fn facts_of(&mut self, inode: &Inode) -> Result<Vec<IndexFact>, FileSystemError> {
        let mut out = Vec::new();
        for (k, v) in &inode.attributes {
            out.push(IndexFact::new(k, v, inode.id));
        }
        for k in inode.large_attributes.keys().cloned().collect::<Vec<_>>() {
            if inode.attributes.contains_key(&k) {
                continue;
            }
            if let Some(v) = self.attribute_of(inode, &k)? {
                out.push(IndexFact::new(&k, &v, inode.id));
            }
        }
        Ok(out)
    }

    /// Remove then insert index facts, inside the caller's transaction.
    ///
    /// v6: log-time B+tree removes/inserts on both trees (path-copy CoW through
    /// the volume's refcount map) and ONE catalog-record rewrite when a root
    /// moved. v3–v5: the flat list, read once, filtered by `(inode, key)`,
    /// appended, rewritten once (the legacy O(n) path those volumes keep).
    fn index_apply(
        &mut self,
        removes: &[IndexFact],
        inserts: &[IndexFact],
    ) -> Result<(), FileSystemError> {
        if removes.is_empty() && inserts.is_empty() {
            return Ok(());
        }
        let catalog_id = self.superblock.catalog_inode;
        if catalog_id == 0 {
            return Ok(());
        }
        let Some(rec) = self.catalog_record()? else {
            let inode = self.read_inode(catalog_id)?;
            let data = self.read_data(catalog_id, 0, inode.size)?;
            let mut entries = deserialize_catalog(&data)?;
            entries.retain(|e| {
                !removes
                    .iter()
                    .any(|f| f.inode_id == e.inode_id && f.key_hash == e.key_hash)
            });
            for f in inserts {
                entries.push(CatalogEntry {
                    key_hash: f.key_hash,
                    val_hash: hash_value(&f.value),
                    inode_id: f.inode_id,
                });
            }
            let new_data = serialize_catalog(&entries)?;
            if new_data != data {
                self.rewrite_data_inner(catalog_id, &new_data)?;
            }
            return Ok(());
        };

        let mut eq = Btree::open(rec.eq_root, LexCmp);
        let mut ord = Btree::open(rec.ord_root, LexCmp);
        let mut entries = rec.entries;
        let written = {
            let mut store = DeviceStore::new(&mut self.device, &mut self.refmap);
            for f in removes {
                if eq.remove(&mut store, &f.eq_key())?.is_some() {
                    entries = entries.saturating_sub(1);
                }
                if let Some(k) = f.ord_key() {
                    ord.remove(&mut store, &k)?;
                }
            }
            for f in inserts {
                if eq.insert(&mut store, &f.eq_key(), &[])?.is_none() {
                    entries = entries.saturating_add(1);
                }
                if let Some(k) = f.ord_key() {
                    ord.insert(&mut store, &k, &[])?;
                }
            }
            store.written
        };
        self.count_index_writes(written);
        let new = CatalogRecord {
            eq_root: eq.root(),
            ord_root: ord.root(),
            entries,
        };
        if new != rec {
            self.rewrite_data_inner(catalog_id, &new.to_bytes())?;
        }
        Ok(())
    }

    /// Every block the catalog inode's two index trees reach (v6), read
    /// through an explicit device — shared by the live fsck walk and the
    /// snapshot walk. Empty for a catalog without a record.
    fn catalog_tree_blocks_via(
        device: &mut D,
        block_count: u64,
        catalog: &Inode,
    ) -> Result<Vec<u64>, FileSystemError> {
        if catalog.size == 0 {
            return Ok(Vec::new());
        }
        let data = Self::read_from_extents_via(
            device,
            block_count,
            &catalog.chunks,
            0,
            catalog.size,
            catalog.size,
        )?;
        let rec = CatalogRecord::from_bytes(&data, block_count)
            .ok_or(FileSystemError::CorruptVolume("catalog record invalid"))?;
        let mut store = ReadStore { device };
        let mut out = Btree::open(rec.eq_root, LexCmp).reachable_blocks(&mut store)?;
        out.extend(Btree::open(rec.ord_root, LexCmp).reachable_blocks(&mut store)?);
        for &b in &out {
            if b >= block_count {
                return Err(FileSystemError::CorruptVolume("index node past volume"));
            }
        }
        Ok(out)
    }

    /// The live catalog's tree blocks (fsck's reachability walk).
    pub(crate) fn catalog_tree_blocks(&mut self) -> Result<Vec<u64>, FileSystemError> {
        if !self.superblock.indexed() {
            return Ok(Vec::new());
        }
        let catalog = self.read_inode(self.superblock.catalog_inode)?;
        Self::catalog_tree_blocks_via(&mut self.device, self.superblock.block_count, &catalog)
    }

    /// Every `(inode id, eq-key, ord-key?)` the index holds — fsck's orphan
    /// scan. v3–v5: the flat list's ids.
    pub(crate) fn index_inode_ids(&mut self) -> Result<BTreeSet<u64>, FileSystemError> {
        let mut ids = BTreeSet::new();
        match self.candidate_source()? {
            Some(CandidateSource::Flat(entries)) => {
                ids.extend(entries.iter().map(|e| e.inode_id));
            }
            Some(CandidateSource::Tree(rec)) => {
                let mut store = ReadStore { device: &mut self.device };
                for root in [rec.eq_root, rec.ord_root] {
                    for (k, _) in Btree::open(root, LexCmp).range(&mut store, None, None, false)? {
                        if let Some(id) = crate::index::inode_of_key(&k) {
                            ids.insert(id);
                        }
                    }
                }
            }
            None => {}
        }
        Ok(ids)
    }

    /// Scrub every index entry naming an inode in `dead` (fsck repair), in the
    /// caller's transaction. Returns the number of equality / flat entries
    /// removed (one per indexed attribute).
    pub(crate) fn index_scrub_ids(&mut self, dead: &BTreeSet<u64>) -> Result<usize, FileSystemError> {
        let catalog_id = self.superblock.catalog_inode;
        match self.candidate_source()? {
            Some(CandidateSource::Flat(mut entries)) => {
                let before = entries.len();
                entries.retain(|e| !dead.contains(&e.inode_id));
                let n = before - entries.len();
                if n > 0 {
                    let data = serialize_catalog(&entries)?;
                    self.rewrite_data_inner(catalog_id, &data)?;
                }
                Ok(n)
            }
            Some(CandidateSource::Tree(rec)) => {
                let mut doomed: [Vec<Vec<u8>>; 2] = [Vec::new(), Vec::new()];
                {
                    let mut store = ReadStore { device: &mut self.device };
                    for (i, root) in [rec.eq_root, rec.ord_root].into_iter().enumerate() {
                        for (k, _) in Btree::open(root, LexCmp).range(&mut store, None, None, false)? {
                            if crate::index::inode_of_key(&k).is_some_and(|id| dead.contains(&id)) {
                                doomed[i].push(k);
                            }
                        }
                    }
                }
                let n = doomed[0].len();
                if n == 0 && doomed[1].is_empty() {
                    return Ok(0);
                }
                let mut eq = Btree::open(rec.eq_root, LexCmp);
                let mut ord = Btree::open(rec.ord_root, LexCmp);
                let written = {
                    let mut store = DeviceStore::new(&mut self.device, &mut self.refmap);
                    for k in &doomed[0] {
                        eq.remove(&mut store, k)?;
                    }
                    for k in &doomed[1] {
                        ord.remove(&mut store, k)?;
                    }
                    store.written
                };
                self.count_index_writes(written);
                let new = CatalogRecord {
                    eq_root: eq.root(),
                    ord_root: ord.root(),
                    entries: rec.entries.saturating_sub(n as u64),
                };
                self.rewrite_data_inner(catalog_id, &new.to_bytes())?;
                Ok(n)
            }
            None => Ok(0),
        }
    }

    fn candidate_source(&mut self) -> Result<Option<CandidateSource>, FileSystemError> {
        let catalog_id = self.superblock.catalog_inode;
        if catalog_id == 0 {
            return Ok(None);
        }
        if let Some(rec) = self.catalog_record()? {
            return Ok(Some(CandidateSource::Tree(rec)));
        }
        let inode = self.read_inode(catalog_id)?;
        let data = self.read_data(catalog_id, 0, inode.size)?;
        Ok(Some(CandidateSource::Flat(deserialize_catalog(&data)?)))
    }

    /// The candidate inode ids for one predicate: a SUPERSET of its matches
    /// (the verifier decides). Equality is one prefix range of the equality
    /// tree; `!=` and similarity take every inode carrying the key; the
    /// ordering operators are ordered-tree range scans.
    fn predicate_candidates(
        &mut self,
        src: &CandidateSource,
        p: &Predicate,
    ) -> Result<BTreeSet<u64>, FileSystemError> {
        let kh = crate::hash::hash_bytes(p.key.as_bytes());
        let mut out = BTreeSet::new();
        match src {
            CandidateSource::Flat(entries) => {
                let vh = matches!(p.op, QueryOp::Eq).then(|| hash_value(&p.value));
                for e in entries {
                    if e.key_hash == kh && vh.is_none_or(|v| v == e.val_hash) {
                        out.insert(e.inode_id);
                    }
                }
            }
            CandidateSource::Tree(rec) => {
                let mut store = ReadStore { device: &mut self.device };
                let ranges: Vec<(u64, Vec<u8>, Vec<u8>)> = match &p.op {
                    QueryOp::Eq => {
                        let (lo, hi) = crate::index::eq_range(kh, Some(hash_value(&p.value)));
                        alloc::vec![(rec.eq_root, lo, hi)]
                    }
                    QueryOp::Neq | QueryOp::SimilarityGt(_) => {
                        let (lo, hi) = crate::index::eq_range(kh, None);
                        alloc::vec![(rec.eq_root, lo, hi)]
                    }
                    QueryOp::Gt | QueryOp::Ge => crate::index::ordered_scans(kh, Some(&p.value), None)
                        .into_iter()
                        .map(|s| (rec.ord_root, s.lo, s.hi))
                        .collect(),
                    QueryOp::Lt | QueryOp::Le => crate::index::ordered_scans(kh, None, Some(&p.value))
                        .into_iter()
                        .map(|s| (rec.ord_root, s.lo, s.hi))
                        .collect(),
                    QueryOp::Range { .. } => {
                        crate::index::ordered_scans(kh, Some(&p.value), p.value_hi.as_ref())
                            .into_iter()
                            .map(|s| (rec.ord_root, s.lo, s.hi))
                            .collect()
                    }
                };
                for (root, lo, hi) in ranges {
                    let tree = Btree::open(root, LexCmp);
                    for (k, _) in tree.range(&mut store, Some(&lo), Some(&hi), false)? {
                        if let Some(id) = crate::index::inode_of_key(&k) {
                            out.insert(id);
                        }
                    }
                }
            }
        }
        Ok(out)
    }

    /// `AND` intersects, `OR` unions, a predicate asks the index.
    fn expr_candidates(
        &mut self,
        src: &CandidateSource,
        e: &Expr,
    ) -> Result<BTreeSet<u64>, FileSystemError> {
        match e {
            Expr::Pred(p) => self.predicate_candidates(src, p),
            Expr::And(v) => {
                let mut acc: Option<BTreeSet<u64>> = None;
                for c in v {
                    let set = self.expr_candidates(src, c)?;
                    acc = Some(match acc {
                        None => set,
                        Some(a) => a.intersection(&set).copied().collect(),
                    });
                    if acc.as_ref().is_some_and(|a| a.is_empty()) {
                        break;
                    }
                }
                Ok(acc.unwrap_or_default())
            }
            Expr::Or(v) => {
                let mut acc = BTreeSet::new();
                for c in v {
                    acc.extend(self.expr_candidates(src, c)?);
                }
                Ok(acc)
            }
        }
    }

    /// Parse, gather candidates, verify. The shared body of
    /// [`query`](Self::query) and [`query_inodes`](Self::query_inodes).
    fn query_matches(&mut self, query_str: &str) -> Result<Vec<(Inode, f32)>, FileSystemError> {
        let query = Query::parse(query_str).map_err(FileSystemError::Query)?;
        let Some(src) = self.candidate_source()? else {
            return Ok(Vec::new());
        };
        let candidates = self.expr_candidates(&src, &query.expr)?;
        let mut keys: Vec<String> = query.expr.predicates().iter().map(|p| p.key.clone()).collect();
        keys.sort();
        keys.dedup();

        let mut results = Vec::new();
        for id in candidates {
            let inode = match self.read_inode(id) {
                Ok(i) => i,
                Err(FileSystemError::NotFound) => continue, // stale index entry
                Err(e) => return Err(e),
            };
            let mut values: BTreeMap<&str, AttributeValue> = BTreeMap::new();
            for k in &keys {
                // A spilled value that fails to decode is "absent", as before.
                match self.attribute_of(&inode, k) {
                    Ok(Some(v)) => {
                        values.insert(k.as_str(), v);
                    }
                    Ok(None) | Err(FileSystemError::InvalidAttributeData) => {}
                    Err(e) => return Err(e),
                }
            }
            if let Some(score) = query.expr.eval(&mut |k: &str| values.get(k).cloned()) {
                results.push((inode, score));
            }
        }
        Ok(results)
    }

    /// The absolute path of `inode_id`, derived from the v6 parent pointers
    /// in O(depth) inode reads — no name-tree walk. `/` for the root; EMPTY
    /// for an unnamed object (a bare `create_inode`, a system inode). On a
    /// v3–v5 volume (no parent pointers) this walks the name tree once.
    pub fn path_of(&mut self, inode_id: u64) -> Result<String, FileSystemError> {
        let root = self.superblock.root_inode;
        if inode_id == root {
            return Ok(String::from("/"));
        }
        if !self.superblock.indexed() {
            return Ok(self.path_map()?.remove(&inode_id).unwrap_or_default());
        }
        let mut parts: Vec<String> = Vec::new();
        let mut cur = inode_id;
        for _ in 0..MAX_PATH_DEPTH {
            if cur == root {
                let mut path = String::new();
                for p in parts.iter().rev() {
                    path.push('/');
                    path.push_str(p);
                }
                return Ok(path);
            }
            let inode = self.read_inode(cur)?;
            if inode.parent == 0 {
                return Ok(String::new());
            }
            let name = match inode.name {
                Some(n) => n,
                // A name too long for the trailer: the parent's listing has it.
                None => match self.ls(inode.parent)?.into_iter().find(|e| e.inode_id == cur) {
                    Some(e) => e.name,
                    None => return Ok(String::new()),
                },
            };
            parts.push(name);
            cur = inode.parent;
        }
        Err(FileSystemError::CorruptVolume("parent chain too deep or cyclic"))
    }

    /// Every name-reachable inode's absolute path, by one walk of the name
    /// tree (the v3–v5 fallback; the first name found wins).
    fn path_map(&mut self) -> Result<BTreeMap<u64, String>, FileSystemError> {
        let root = self.superblock.root_inode;
        let mut map: BTreeMap<u64, String> = BTreeMap::new();
        map.insert(root, String::from("/"));
        let mut stack: Vec<(u64, String)> = alloc::vec![(root, String::new())];
        while let Some((dir, prefix)) = stack.pop() {
            for e in self.ls(dir)? {
                if map.contains_key(&e.inode_id) {
                    continue; // a corrupt volume's cycle: first name wins
                }
                let path = format!("{}/{}", prefix, e.name);
                map.insert(e.inode_id, path.clone());
                if e.kind == FileKind::Directory {
                    stack.push((e.inode_id, path));
                }
            }
        }
        Ok(map)
    }

    /// Identity, size, parent link and timestamps of an inode (the kernel's
    /// `DirEnt.mtime` source).
    pub fn stat(&mut self, inode_id: u64) -> Result<Stat, FileSystemError> {
        let inode = self.read_inode(inode_id)?;
        Ok(Stat {
            inode_id,
            kind: inode.kind,
            size: inode.size,
            parent: inode.parent,
            ctime: inode.ctime,
            mtime: inode.mtime,
            atime: inode.atime,
        })
    }
}
