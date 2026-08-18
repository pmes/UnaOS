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

//! The K8 (version 3) superblock: STATIC volume identity, written once at
//! format time and never rewritten.
//!
//! Under the copy-on-write format every mutable fact about the volume (the
//! live tree, allocation state, free-space count) lives in the tree reached
//! from the [`root record`](crate::root::RootRecord) — block 0 carries only
//! what never changes: the magic, the version, the geometry, and the two
//! reserved logical inode ids. Commit never touches block 0, so the one
//! at-rest mutation point of the whole volume is the 512 B root-sector flip.

use crate::storage::{BLOCK_SIZE, Error as StorageError};
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The Magic Number for UnaFS: "UNAFS" in ASCII.
pub const MAGIC: [u8; 5] = *b"UNAFS";
/// The current version of the filesystem written at format time.
///
/// * 3 — the K8 copy-on-write format (extent lists are inline-only: a file
///   whose inode does not fit one 4096 B block is rejected `InodeTooLarge`).
/// * 4 — adds EXTENT-LIST INDIRECTION: an overflowing extent list spills to
///   indirect blocks (see [`crate::inode::IndirectTrailer`]). A v4 volume may
///   therefore contain a spilled inode that a v3-only reader would silently
///   truncate — so the bump is an INCOMPAT marker, not a soft hint. Fresh
///   volumes format as v4; spilling is refused on a mounted v3 volume, which
///   keeps every v3 inode byte-identical and mountable by old readers.
/// * 5 — adds a SECOND REFCOUNT-MAP LEVEL: when the volume needs more than
///   [`REFMAP_LEAVES_PER_INDEX`] refmap leaves (i.e. it is larger than
///   [`MAX_BLOCK_COUNT_ONE_LEVEL`] blocks = 2 GiB), the root's `refmap_block`
///   points at an index-of-indexes whose entries are MID index blocks, each
///   holding up to 512 leaf pointers. The level is a PURE FUNCTION of the
///   volume geometry (leaves ≤ 512 ⇔ one level), so a v5 volume of ≤ 2 GiB is
///   laid out identically to v4 — only the version stamp differs — and no
///   root-record field changes. The bump is an INCOMPAT marker for the same
///   reason v4's was: a pre-v5 reader pointed at a two-level index would
///   misread mid pointers as leaf pointers. v3/v4 volumes (whose geometry
///   guarantees one level) mount and read exactly as before.
pub const VERSION: u32 = 5;

/// The oldest on-disk version this build still mounts. A v3 volume mounts
/// read/write but never spills (it has no v4 inodes and gains none), and no
/// v3/v4 volume can need the second refmap level (their geometry is capped at
/// one level), so old images keep working while new images that need the new
/// structures are stamped and REJECTED by older readers — detected, not
/// misread.
pub const MIN_SUPPORTED_VERSION: u32 = 3;

/// First version whose format admits spilled (indirect) extent lists.
pub const VERSION_EXTENT_SPILL: u32 = 4;

/// First version whose format admits a second refcount-map level (volumes
/// larger than [`MAX_BLOCK_COUNT_ONE_LEVEL`] blocks).
pub const VERSION_REFMAP_TREE: u32 = 5;

/// Reserved logical inode ids (stable across every mutation — Fork-1 verdict:
/// inode ids are LOGICAL; the inode map carries id → current physical block).
pub const ROOT_INODE_ID: u64 = 1;
/// The attribute catalog (a `System` inode).
pub const CATALOG_INODE_ID: u64 = 2;
/// The snapshot index — itself a UnaFS object (a `System` inode whose data is
/// the serialized snapshot entry list). v1 policy caps it at
/// [`SNAPSHOT_CAP`](crate::fs::SNAPSHOT_CAP) entries; the cap is policy, not
/// structure.
pub const SNAP_INDEX_INODE_ID: u64 = 3;
/// The persistent reclaim queue — a UnaFS object (a `System` inode whose data
/// is the serialized queue). Dropped roots enqueue here; v1 drains eagerly.
pub const RECLAIM_INODE_ID: u64 = 4;
/// First logical inode id handed to user objects.
pub const FIRST_USER_INODE_ID: u64 = 5;

/// Refmap leaf (or mid-index) pointers per 4096 B index block (u64 each).
pub const REFMAP_LEAVES_PER_INDEX: u64 = BLOCK_SIZE / 8;

/// Largest volume ONE refmap level addresses: the refcount-map index is one
/// 4096 B block of leaf pointers (512 leaves × 1024 u32 counts/leaf =
/// 524,288 blocks = 2 GiB). A volume at or under this uses the single-level
/// layout regardless of version — which is what keeps a small v5 volume
/// byte-identical to v4 apart from the version stamp.
pub const MAX_BLOCK_COUNT_ONE_LEVEL: u64 =
    REFMAP_LEAVES_PER_INDEX * (BLOCK_SIZE / 4); // 512 leaf ptrs × 1024 refs/leaf

/// Largest volume the map structure addresses at all: TWO refmap levels
/// (v5+) — one index-of-indexes block of 512 mid pointers, each mid block
/// holding 512 leaf pointers (512 × 512 × 1024 refs = 268,435,456 blocks =
/// 1 TiB). Enforced at format AND mount — a bigger volume is a clean
/// [`SuperblockError::Geometry`], never a slice panic. The next walls past
/// this are not format walls: the in-RAM map (8 bytes/block across the two
/// views) and the whole-map rewrite each commit grow linearly with the
/// volume, so a third level should arrive together with an incremental map.
pub const MAX_BLOCK_COUNT: u64 = REFMAP_LEAVES_PER_INDEX * MAX_BLOCK_COUNT_ONE_LEVEL;

#[derive(Error, Debug)]
pub enum SuperblockError {
    #[error("Invalid magic number")]
    InvalidMagic,
    #[error("Invalid version: {0}")]
    InvalidVersion(u32),
    #[error("Block size mismatch: expected {0}, found {1}")]
    BlockSizeMismatch(u32, u32),
    #[error("Serialization error: {0}")]
    Serialization(#[from] crate::codec::CodecError),
    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("Superblock too large: {0} bytes")]
    TooLarge(usize),
    #[error("Superblock geometry invalid: {0}")]
    Geometry(&'static str),
}

/// The Superblock resides at Block 0 and identifies the volume. Static: every
/// field is fixed at format time (BEFS-HARDEN validation still applies — the
/// volume is untrusted input and `block_count` bounds every later access).
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct Superblock {
    /// Magic number to identify the filesystem.
    pub magic: [u8; 5],
    /// Filesystem version.
    pub version: u32,
    /// Block size (must be 4096).
    pub block_size: u32,
    /// Total number of blocks in the device.
    pub block_count: u64,
    /// The LOGICAL inode id of the root directory (always [`ROOT_INODE_ID`]).
    pub root_inode: u64,
    /// The LOGICAL inode id of the attribute catalog ([`CATALOG_INODE_ID`]).
    pub catalog_inode: u64,
}

impl Superblock {
    /// Create a new Superblock for a device with the given block count.
    pub fn new(block_count: u64) -> Self {
        Self {
            magic: MAGIC,
            version: VERSION,
            block_size: BLOCK_SIZE as u32,
            block_count,
            root_inode: ROOT_INODE_ID,
            catalog_inode: CATALOG_INODE_ID,
        }
    }

    /// Whether this volume's format admits spilled (indirect) extent lists.
    /// A mounted v3 volume returns `false`, so an oversized inode is rejected
    /// exactly as before rather than silently spilling into a format the volume
    /// does not declare.
    pub fn spill_capable(&self) -> bool {
        self.version >= VERSION_EXTENT_SPILL
    }

    /// Whether this volume's refcount map uses the TWO-LEVEL index tree — a
    /// pure function of the geometry (more leaves than one index block
    /// holds), never of the version: a v5 volume of ≤ 2 GiB is single-level
    /// and byte-identical to v4. [`validate`](Self::validate) guarantees
    /// two-level geometry only ever passes on a v5+ volume.
    pub fn refmap_two_level(&self) -> bool {
        self.block_count > MAX_BLOCK_COUNT_ONE_LEVEL
    }

    /// Serialize the Superblock to bytes, ensuring it fits in Block 0.
    pub fn to_bytes(&self) -> Result<Vec<u8>, SuperblockError> {
        let bytes = crate::codec::serialize(self)?;
        if bytes.len() as u64 > BLOCK_SIZE {
            return Err(SuperblockError::TooLarge(bytes.len()));
        }
        Ok(bytes)
    }

    /// Deserialize a Superblock from bytes and validate it.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SuperblockError> {
        // Block-sized record: block-budgeted decode (BEFS-HARDEN).
        let sb: Superblock = crate::codec::deserialize_block(bytes)?;

        if sb.magic != MAGIC {
            return Err(SuperblockError::InvalidMagic);
        }
        // Accept a RANGE: a v3 volume mounts (inline-only), a v4 volume mounts
        // with indirection. A future version is rejected — a v4-era build must
        // never guess at a layout it does not know.
        if sb.version < MIN_SUPPORTED_VERSION || sb.version > VERSION {
            return Err(SuperblockError::InvalidVersion(sb.version));
        }
        if sb.block_size as u64 != BLOCK_SIZE {
            return Err(SuperblockError::BlockSizeMismatch(
                BLOCK_SIZE as u32,
                sb.block_size,
            ));
        }

        sb.validate()?;

        Ok(sb)
    }

    /// Bound every on-disk-derived field against the volume span the
    /// superblock itself declares (BEFS-HARDEN). The volume is untrusted
    /// input: a physically swapped or corrupted card reaches this parser
    /// in-kernel, so anything that later drives an allocation or a block
    /// address is rejected here, at the boundary.
    pub fn validate(&self) -> Result<(), SuperblockError> {
        // The volume byte size must be representable, since read paths bound
        // lengths against `block_count * BLOCK_SIZE`.
        let _volume_bytes = self
            .block_count
            .checked_mul(BLOCK_SIZE)
            .ok_or(SuperblockError::Geometry("volume byte size overflows"))?;

        // Superblock (0) + root area (1) + at least one tree block.
        if self.block_count < 3 {
            return Err(SuperblockError::Geometry("volume too small"));
        }
        // The map structure (two indirect levels, v5+) addresses at most
        // MAX_BLOCK_COUNT blocks; a bigger volume must fail HERE, cleanly —
        // the commit path would otherwise run its refmap index tree off its
        // blocks.
        if self.block_count > MAX_BLOCK_COUNT {
            return Err(SuperblockError::Geometry(
                "volume exceeds the refcount-map structure (two indirect levels)",
            ));
        }
        // A pre-v5 volume's format has only ONE refmap level (one index block
        // of leaf pointers): geometry past that cap describes a volume the
        // pre-v5 code never wrote — refuse it rather than misread mid
        // pointers as leaf pointers.
        if self.version < VERSION_REFMAP_TREE && self.block_count > MAX_BLOCK_COUNT_ONE_LEVEL {
            return Err(SuperblockError::Geometry(
                "volume exceeds one refmap level (pre-v5 format)",
            ));
        }

        // The reserved logical ids are format constants.
        if self.root_inode != ROOT_INODE_ID {
            return Err(SuperblockError::Geometry(
                "root inode id not the reserved id",
            ));
        }
        if self.catalog_inode != CATALOG_INODE_ID {
            return Err(SuperblockError::Geometry(
                "catalog inode id not the reserved id",
            ));
        }

        Ok(())
    }
}
