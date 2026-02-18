use crate::storage::{BlockDevice, Error as StorageError, BLOCK_SIZE};
use crate::superblock::{Superblock, SuperblockError};
use crate::bitmap::SpaceMap;
use crate::inode::{Inode, AttributeValue, InodeError, FileKind, Extent};
use thiserror::Error;
use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};

#[derive(Error, Debug)]
pub enum FileSystemError {
    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("Superblock error: {0}")]
    Superblock(#[from] SuperblockError),
    #[error("Inode error: {0}")]
    Inode(#[from] InodeError),
    #[error("Serialization error: {0}")]
    Serialization(#[from] bincode::Error),
    #[error("No free space available")]
    NoSpace,
    #[error("Root inode missing")]
    RootMissing,
    #[error("Not a directory")]
    NotADirectory,
    #[error("File already exists")]
    FileExists,
}

/// A directory entry pointing to an inode.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, PartialOrd)]
pub struct DirEntry {
    pub name: String,
    pub inode_id: u64,
    pub kind: FileKind,
}

pub struct UnaFS<D: BlockDevice> {
    pub device: D,
    pub superblock: Superblock,
    pub bitmap: SpaceMap,
}

impl<D: BlockDevice> UnaFS<D> {
    /// Format the device with a new UnaFS filesystem.
    pub fn format(mut device: D) -> Result<Self, FileSystemError> {
        let block_count = device.block_count();
        let mut superblock = Superblock::new(block_count);
        let mut bitmap = SpaceMap::new(block_count);

        bitmap.mark_used(0);
        for i in 0..superblock.bitmap_blocks {
            bitmap.mark_used(superblock.bitmap_start + i);
        }

        let root_id = bitmap.allocate().ok_or(FileSystemError::NoSpace)?;
        superblock.root_inode = root_id;

        if superblock.free_blocks > 0 {
            superblock.free_blocks -= 1;
        }

        let root_inode = Inode::new(root_id, FileKind::Directory);

        let root_bytes = root_inode.to_bytes()?;
        let mut root_block = vec![0u8; BLOCK_SIZE as usize];
        root_block[..root_bytes.len()].copy_from_slice(&root_bytes);
        device.write_block(root_id, &root_block)?;

        bitmap.save(&mut device, superblock.bitmap_start)?;

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
        let mut sb_block = vec![0u8; BLOCK_SIZE as usize];
        device.read_block(0, &mut sb_block)?;
        let superblock = Superblock::from_bytes(&sb_block)?;

        let bitmap = SpaceMap::load(&device, superblock.bitmap_start, superblock.bitmap_blocks)?;

        Ok(Self {
            device,
            superblock,
            bitmap,
        })
    }

    /// Read an Inode by ID.
    pub fn read_inode(&self, id: u64) -> Result<Inode, FileSystemError> {
        let mut block = vec![0u8; BLOCK_SIZE as usize];
        self.device.read_block(id, &mut block)?;
        let inode = Inode::from_bytes(&block)?;
        Ok(inode)
    }

    /// Write an Inode to disk.
    fn write_inode(&mut self, inode: &Inode) -> Result<(), FileSystemError> {
        let bytes = inode.to_bytes()?;
        let mut block = vec![0u8; BLOCK_SIZE as usize];
        block[..bytes.len()].copy_from_slice(&bytes);
        self.device.write_block(inode.id, &block)?;
        Ok(())
    }

    /// Create a new Inode with given attributes and kind.
    /// Internal helper.
    fn create_inode_internal(&mut self, kind: FileKind, attributes: BTreeMap<String, AttributeValue>) -> Result<u64, FileSystemError> {
        let inode_id = self.allocate_inode_block()?;
        let mut inode = Inode::new(inode_id, kind);
        inode.attributes = attributes;

        self.write_inode(&inode)?;
        self.sync_metadata()?;
        Ok(inode_id)
    }

    // Legacy support for tests that might use create_inode assuming File
    pub fn create_inode(&mut self, attributes: BTreeMap<String, AttributeValue>) -> Result<u64, FileSystemError> {
        self.create_inode_internal(FileKind::File, attributes)
    }

    /// Helper to allocate a block for an inode and update superblock free count.
    fn allocate_inode_block(&mut self) -> Result<u64, FileSystemError> {
        let block_id = self.bitmap.allocate().ok_or(FileSystemError::NoSpace)?;
        if self.superblock.free_blocks > 0 {
            self.superblock.free_blocks -= 1;
        }
        Ok(block_id)
    }

    /// Helper to sync Superblock and Bitmap to disk.
    fn sync_metadata(&mut self) -> Result<(), FileSystemError> {
        self.bitmap.save(&mut self.device, self.superblock.bitmap_start)?;

        let sb_bytes = self.superblock.to_bytes()?;
        let mut sb_block = vec![0u8; BLOCK_SIZE as usize];
        sb_block[..sb_bytes.len()].copy_from_slice(&sb_bytes);
        self.device.write_block(0, &sb_block)?;
        Ok(())
    }

    /// Write data to an Inode.
    pub fn write_data(&mut self, inode_id: u64, offset: u64, data: &[u8]) -> Result<(), FileSystemError> {
        if data.is_empty() {
            return Ok(());
        }

        let mut inode = self.read_inode(inode_id)?;

        let mut current_offset = offset;
        let mut data_written = 0;

        while data_written < data.len() {
            // Find which logical block we are in
            let block_offset = (current_offset % BLOCK_SIZE) as usize;
            let to_write = std::cmp::min(BLOCK_SIZE as usize - block_offset, data.len() - data_written);

            // Find physical block
            let mut physical_block = 0;
            let mut extent_found = false;

            // Search extents
            for extent in inode.chunks.iter() {
                let extent_end = extent.logical_offset + extent.length;
                if current_offset >= extent.logical_offset && current_offset < extent_end {
                     let offset_in_extent = current_offset - extent.logical_offset;
                     let block_offset_in_extent = offset_in_extent / BLOCK_SIZE;
                     physical_block = extent.physical_block + block_offset_in_extent;
                     extent_found = true;
                     break;
                }
            }

            if !extent_found {
                // We need to allocate a new block.
                let new_block = self.bitmap.allocate().ok_or(FileSystemError::NoSpace)?;
                if self.superblock.free_blocks > 0 {
                    self.superblock.free_blocks -= 1;
                }

                // Vector Optimization: Merge Extents
                let mut merged = false;
                if let Some(last) = inode.chunks.last_mut() {
                    let last_block_count = last.length.div_ceil(BLOCK_SIZE);
                    let last_physical_end = last.physical_block + last_block_count - 1;

                    if last.logical_offset + last.length <= current_offset
                         && last.length % BLOCK_SIZE == 0
                             && last_physical_end + 1 == new_block {
                                 last.length += BLOCK_SIZE;
                                 merged = true;
                                 physical_block = new_block;
                             }
                }

                if !merged {
                    let aligned_logical = (current_offset / BLOCK_SIZE) * BLOCK_SIZE;
                    let new_extent = Extent {
                        logical_offset: aligned_logical,
                        physical_block: new_block,
                        length: BLOCK_SIZE, // Allocated 1 block
                    };
                    inode.chunks.push(new_extent);
                    physical_block = new_block;
                }
            }

            let mut block_buf = vec![0u8; BLOCK_SIZE as usize];
            self.device.read_block(physical_block, &mut block_buf)?;
            block_buf[block_offset..block_offset + to_write].copy_from_slice(&data[data_written..data_written + to_write]);
            self.device.write_block(physical_block, &block_buf)?;

            data_written += to_write;
            current_offset += to_write as u64;
        }

        // Update Inode Size
        if current_offset > inode.size {
            inode.size = current_offset;
        }

        self.write_inode(&inode)?;
        self.sync_metadata()?; // To save bitmap changes

        Ok(())
    }

    /// Read data from an Inode.
    pub fn read_data(&self, inode_id: u64, offset: u64, length: u64) -> Result<Vec<u8>, FileSystemError> {
        let inode = self.read_inode(inode_id)?;
        let mut buffer = Vec::with_capacity(length as usize);
        let mut read_so_far = 0;
        let mut current_offset = offset;

        // Cap length at file size
        let available = inode.size.saturating_sub(offset);
        let to_read_total = std::cmp::min(length, available);

        while read_so_far < to_read_total {
             let mut physical_block = 0;
             let mut found = false;

             for extent in &inode.chunks {
                 let extent_end = extent.logical_offset + extent.length;
                 if current_offset >= extent.logical_offset && current_offset < extent_end {
                     let offset_in_extent = current_offset - extent.logical_offset;
                     let block_idx = offset_in_extent / BLOCK_SIZE;
                     physical_block = extent.physical_block + block_idx;
                     found = true;
                     break;
                 }
             }

             if !found {
                 buffer.push(0);
                 read_so_far += 1;
                 current_offset += 1;
                 continue;
             }

             let block_offset = (current_offset % BLOCK_SIZE) as usize;
             let to_read = std::cmp::min(BLOCK_SIZE as usize - block_offset, (to_read_total - read_so_far) as usize);

             let mut block_buf = vec![0u8; BLOCK_SIZE as usize];
             self.device.read_block(physical_block, &mut block_buf)?;

             buffer.extend_from_slice(&block_buf[block_offset..block_offset + to_read]);

             read_so_far += to_read as u64;
             current_offset += to_read as u64;
        }

        Ok(buffer)
    }

    /// List directory contents.
    pub fn ls(&self, inode_id: u64) -> Result<Vec<DirEntry>, FileSystemError> {
        let inode = self.read_inode(inode_id)?;

        if inode.kind != FileKind::Directory {
            return Err(FileSystemError::NotADirectory);
        }

        if inode.size == 0 {
            return Ok(Vec::new());
        }

        // Read all data from directory inode
        let data = self.read_data(inode_id, 0, inode.size)?;

        // Deserialize
        let entries: Vec<DirEntry> = bincode::deserialize(&data)?;
        Ok(entries)
    }

    /// Create a directory inside a parent directory.
    pub fn mkdir(&mut self, parent_id: u64, name: String) -> Result<u64, FileSystemError> {
        self.add_entry(parent_id, name, FileKind::Directory)
    }

    /// Create a file inside a parent directory.
    pub fn create_file(&mut self, parent_id: u64, name: String) -> Result<u64, FileSystemError> {
        self.add_entry(parent_id, name, FileKind::File)
    }

    /// Helper to add an entry to a directory.
    fn add_entry(&mut self, parent_id: u64, name: String, kind: FileKind) -> Result<u64, FileSystemError> {
        // 1. Verify Parent
        let parent_inode = self.read_inode(parent_id)?;
        if parent_inode.kind != FileKind::Directory {
            return Err(FileSystemError::NotADirectory);
        }

        // 2. Read Existing Entries
        let mut entries = if parent_inode.size > 0 {
            self.ls(parent_id)?
        } else {
            Vec::new()
        };

        // 3. Check for duplicates
        if entries.iter().any(|e| e.name == name) {
            return Err(FileSystemError::FileExists);
        }

        // 4. Create New Inode
        let new_id = self.create_inode_internal(kind, BTreeMap::new())?;

        // 5. Update Parent Directory List
        entries.push(DirEntry {
            name,
            inode_id: new_id,
            kind,
        });

        // Maintain sort order (Vector Special)
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        // 6. Serialize and Write Back
        let data = bincode::serialize(&entries)?;

        self.write_data(parent_id, 0, &data)?;

        Ok(new_id)
    }
}
