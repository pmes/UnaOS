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

use crate::storage::{BLOCK_SIZE, Error as StorageError};
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// The Magic Number for UnaFS: "UNAFS" in ASCII.
pub const MAGIC: [u8; 5] = *b"UNAFS";
/// The current version of the filesystem.
pub const VERSION: u32 = 2; // Bumped version for Attribute Engine

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

/// The Superblock resides at Block 0 and describes the filesystem layout.
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
    /// The block ID of the root inode.
    pub root_inode: u64,
    /// Number of free blocks available.
    pub free_blocks: u64,
    /// The starting block of the allocation bitmap.
    pub bitmap_start: u64,
    /// The number of blocks occupied by the bitmap.
    pub bitmap_blocks: u64,

    // --- UNAFS 2.0: The Semantic Vault ---
    /// The starting block of the Write-Ahead Log (WAL).
    pub journal_start: u64,
    /// The number of blocks reserved for the WAL.
    pub journal_blocks: u64,

    /// The Inode ID of the Attribute Catalog (System File).
    pub catalog_inode: u64,
}

impl Superblock {
    /// Create a new Superblock for a device with the given block count.
    /// Calculates the bitmap size and placement automatically.
    pub fn new(block_count: u64) -> Self {
        // Calculate bitmap size.
        let bitmap_bytes = block_count.div_ceil(8);
        let bitmap_blocks = bitmap_bytes.div_ceil(BLOCK_SIZE);

        // Layout:
        // Block 0: Superblock
        // Block 1..11: Journal (10 blocks)
        // Block 11..(11+bitmap_blocks): Bitmap

        let journal_start = 1;
        let journal_blocks = 10;

        let bitmap_start = journal_start + journal_blocks;

        // Total used blocks initially: Superblock (1) + Journal (10) + Bitmap blocks.
        // Root inode and Catalog inode will be allocated later.
        let initial_used = 1 + journal_blocks + bitmap_blocks;
        let free_blocks = block_count.saturating_sub(initial_used);

        Self {
            magic: MAGIC,
            version: VERSION,
            block_size: BLOCK_SIZE as u32,
            block_count,
            root_inode: 0, // Will be set after allocation
            free_blocks,
            bitmap_start,
            bitmap_blocks,
            journal_start,
            journal_blocks,
            catalog_inode: 0, // Will be set after allocation
        }
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
        let sb: Superblock = crate::codec::deserialize(bytes)?;

        if sb.magic != MAGIC {
            return Err(SuperblockError::InvalidMagic);
        }
        // Backward compatibility: If we were really maintaining it, we would check version.
        // But for this exercise, we are upgrading the format entirely.
        if sb.version != VERSION {
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

    /// Bound every on-disk-derived block-count/offset field against the
    /// volume span the superblock itself declares (BEFS-HARDEN).
    ///
    /// The volume is untrusted input: a physically swapped or corrupted card
    /// reaches this parser in-kernel (BeFS-K3), so any field that later
    /// drives an allocation or a block address must be rejected here — at
    /// the boundary — rather than trusted downstream. Validation only: every
    /// volume `format()` has ever produced passes unchanged (the golden KATs
    /// pin this).
    pub fn validate(&self) -> Result<(), SuperblockError> {
        // The volume byte size must be representable, since read paths bound
        // lengths against `block_count * BLOCK_SIZE`.
        let _volume_bytes = self
            .block_count
            .checked_mul(BLOCK_SIZE)
            .ok_or(SuperblockError::Geometry("volume byte size overflows"))?;

        // The WAL addresses the journal via its compile-time layout constants
        // (`wal::JOURNAL_START`/`JOURNAL_BLOCKS`); a superblock declaring any
        // other journal placement describes a volume this code never wrote.
        if self.journal_start != crate::wal::JOURNAL_START
            || self.journal_blocks != crate::wal::JOURNAL_BLOCKS
        {
            return Err(SuperblockError::Geometry("journal layout mismatch"));
        }
        let journal_end = self
            .journal_start
            .checked_add(self.journal_blocks)
            .ok_or(SuperblockError::Geometry("journal span overflows"))?;
        if journal_end > self.block_count {
            return Err(SuperblockError::Geometry("journal exceeds volume"));
        }

        // The bitmap must be exactly the size the declared block count
        // implies (what `Superblock::new` computes), and must lie after the
        // superblock and inside the volume. `bitmap_blocks` sizes the mount-
        // time bitmap allocation, so this is the K3-PARSE-1 bound.
        let expected_bitmap_blocks = self.block_count.div_ceil(8).div_ceil(BLOCK_SIZE);
        if self.bitmap_blocks != expected_bitmap_blocks {
            return Err(SuperblockError::Geometry(
                "bitmap size inconsistent with block count",
            ));
        }
        if self.bitmap_start == 0 {
            return Err(SuperblockError::Geometry("bitmap overlaps superblock"));
        }
        let bitmap_end = self
            .bitmap_start
            .checked_add(self.bitmap_blocks)
            .ok_or(SuperblockError::Geometry("bitmap span overflows"))?;
        if bitmap_end > self.block_count {
            return Err(SuperblockError::Geometry("bitmap exceeds volume"));
        }

        // Inode ids are used directly as block addresses.
        if self.root_inode == 0 || self.root_inode >= self.block_count {
            return Err(SuperblockError::Geometry("root inode out of bounds"));
        }
        // `catalog_inode == 0` means "no catalog" (checked at every use).
        if self.catalog_inode >= self.block_count {
            return Err(SuperblockError::Geometry("catalog inode out of bounds"));
        }

        if self.free_blocks > self.block_count {
            return Err(SuperblockError::Geometry("free blocks exceed volume"));
        }

        Ok(())
    }
}
