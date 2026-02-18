use crate::storage::{BlockDevice, Error as StorageError, BLOCK_SIZE};
use crate::superblock::{Superblock, SuperblockError};
use crate::bitmap::SpaceMap;
use crate::inode::{Inode, AttributeValue, InodeError};
use thiserror::Error;
use std::collections::BTreeMap;

#[derive(Error, Debug)]
pub enum FileSystemError {
    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("Superblock error: {0}")]
    Superblock(#[from] SuperblockError),
    #[error("Inode error: {0}")]
    Inode(#[from] InodeError),
    #[error("No free space available")]
    NoSpace,
    #[error("Root inode missing")]
    RootMissing,
}

pub struct UnaFS<D: BlockDevice> {
    pub device: D,
    pub superblock: Superblock,
    pub bitmap: SpaceMap,
}

impl<D: BlockDevice> UnaFS<D> {
    /// Format the device with a new UnaFS filesystem.
    ///
    /// Wipes the device, creates the Superblock, Bitmap, and Root Inode.
    /// Returns an initialized UnaFS instance.
    pub fn format(mut device: D) -> Result<Self, FileSystemError> {
        let block_count = device.block_count();

        // 1. Create Superblock (Calculates layout)
        let mut superblock = Superblock::new(block_count);

        // 2. Initialize Bitmap
        // Create an empty map covering the whole disk
        let mut bitmap = SpaceMap::new(block_count);

        // Mark reserved blocks as used:
        // Block 0: Superblock
        bitmap.mark_used(0);

        // Blocks 1..N: Bitmap itself
        for i in 0..superblock.bitmap_blocks {
            bitmap.mark_used(superblock.bitmap_start + i);
        }

        // 3. Create Root Inode
        // Allocate a block for the root inode from the bitmap
        // Since we just marked SB + Bitmap as used, allocate() should return next free block.
        let root_id = bitmap.allocate().ok_or(FileSystemError::NoSpace)?;
        superblock.root_inode = root_id;

        // Decrease free blocks count in superblock to reflect allocation
        if superblock.free_blocks > 0 {
            superblock.free_blocks -= 1;
        }

        // Create the Root Inode struct
        let root_inode = Inode::new(root_id);

        // Write Root Inode to disk
        let root_bytes = root_inode.to_bytes()?;
        let mut root_block = vec![0u8; BLOCK_SIZE as usize];
        root_block[..root_bytes.len()].copy_from_slice(&root_bytes);
        device.write_block(root_id, &root_block)?;

        // 4. Write Bitmap to disk
        bitmap.save(&mut device, superblock.bitmap_start)?;

        // 5. Write Superblock to disk (Block 0)
        let sb_bytes = superblock.to_bytes()?;
        let mut sb_block = vec![0u8; BLOCK_SIZE as usize];
        sb_block[..sb_bytes.len()].copy_from_slice(&sb_bytes);
        device.write_block(0, &sb_block)?;

        Ok(Self {
            device,
            superblock,
            bitmap,
        })
    }

    /// Mount an existing UnaFS filesystem.
    pub fn mount(device: D) -> Result<Self, FileSystemError> {
        // 1. Read Superblock
        let mut sb_block = vec![0u8; BLOCK_SIZE as usize];
        device.read_block(0, &mut sb_block)?;
        let superblock = Superblock::from_bytes(&sb_block)?;

        // 2. Load Bitmap
        let bitmap = SpaceMap::load(&device, superblock.bitmap_start, superblock.bitmap_blocks)?;

        Ok(Self {
            device,
            superblock,
            bitmap,
        })
    }

    /// Create a new Inode with the given attributes.
    /// Returns the new Inode ID.
    pub fn create_inode(&mut self, attributes: BTreeMap<String, AttributeValue>) -> Result<u64, FileSystemError> {
        // 1. Allocate block
        let inode_id = self.bitmap.allocate().ok_or(FileSystemError::NoSpace)?;

        // 2. Update Superblock free count
        if self.superblock.free_blocks > 0 {
            self.superblock.free_blocks -= 1;
        }

        // 3. Create Inode
        let mut inode = Inode::new(inode_id);
        inode.attributes = attributes;

        // 4. Write Inode to disk
        let bytes = inode.to_bytes()?;
        let mut block = vec![0u8; BLOCK_SIZE as usize];
        block[..bytes.len()].copy_from_slice(&bytes);
        self.device.write_block(inode_id, &block)?;

        // 5. Sync Bitmap (Mark as used on disk)
        self.bitmap.save(&mut self.device, self.superblock.bitmap_start)?;

        // 6. Sync Superblock (Update free count)
        let sb_bytes = self.superblock.to_bytes()?;
        let mut sb_block = vec![0u8; BLOCK_SIZE as usize];
        sb_block[..sb_bytes.len()].copy_from_slice(&sb_bytes);
        self.device.write_block(0, &sb_block)?;

        Ok(inode_id)
    }

    /// Read an Inode by ID.
    pub fn read_inode(&self, id: u64) -> Result<Inode, FileSystemError> {
        let mut block = vec![0u8; BLOCK_SIZE as usize];
        self.device.read_block(id, &mut block)?;
        let inode = Inode::from_bytes(&block)?;
        Ok(inode)
    }
}
