use serde::{Deserialize, Serialize};
use crate::storage::{BLOCK_SIZE, Error as StorageError};
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
    Serialization(#[from] bincode::Error),
    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("Superblock too large: {0} bytes")]
    TooLarge(usize),
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
        let bytes = bincode::serialize(self)?;
        if bytes.len() as u64 > BLOCK_SIZE {
            return Err(SuperblockError::TooLarge(bytes.len()));
        }
        Ok(bytes)
    }

    /// Deserialize a Superblock from bytes and validate it.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, SuperblockError> {
        let sb: Superblock = bincode::deserialize(bytes)?;

        if sb.magic != MAGIC {
            return Err(SuperblockError::InvalidMagic);
        }
        // Backward compatibility: If we were really maintaining it, we would check version.
        // But for this exercise, we are upgrading the format entirely.
        if sb.version != VERSION {
            return Err(SuperblockError::InvalidVersion(sb.version));
        }
        if sb.block_size as u64 != BLOCK_SIZE {
            return Err(SuperblockError::BlockSizeMismatch(BLOCK_SIZE as u32, sb.block_size));
        }

        Ok(sb)
    }
}
