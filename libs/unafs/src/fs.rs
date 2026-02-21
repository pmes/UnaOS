use bandy::{BandyMember, SMessage};
use crate::storage::{BlockDevice, Error as StorageError, BLOCK_SIZE};
use crate::superblock::{Superblock, SuperblockError};
use crate::bitmap::SpaceMap;
use crate::inode::{Inode, AttributeValue, InodeError, FileKind, Extent, ExtentList};
use crate::wal::{Journal, JournalOp};
use crate::catalog::{CatalogEntry, serialize_catalog, deserialize_catalog, hash_value};
use crate::query::{Query, QueryOp};
use crate::hash::hash_bytes;
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
    #[error("Attribute too large for inline storage")]
    AttributeTooLarge,
    #[error("Journal error: {0}")]
    Journal(#[from] crate::wal::JournalError),
    #[error("Invalid Attribute Data")]
    InvalidAttributeData,
    #[error("Query error: {0}")]
    Query(String),
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
    pub journal: Journal,
}

impl<D: BlockDevice> UnaFS<D> {
    /// Format the device with a new UnaFS filesystem.
    pub fn format(mut device: D, size_mb: u64) -> Result<Self, FileSystemError> {
        // Use provided size if device is empty or for initialization
        let blocks_from_size = (size_mb * 1024 * 1024) / BLOCK_SIZE;
        let mut block_count = device.block_count();

        if block_count == 0 {
             block_count = blocks_from_size;
        }

        let mut superblock = Superblock::new(block_count);
        let mut bitmap = SpaceMap::new(block_count);
        let mut journal = Journal::new();

        // 1. Mark System Blocks as Used
        // Superblock
        bitmap.mark_used(0);

        // Journal Blocks
        for i in 0..superblock.journal_blocks {
            bitmap.mark_used(superblock.journal_start + i);
        }

        // Bitmap Blocks
        for i in 0..superblock.bitmap_blocks {
            bitmap.mark_used(superblock.bitmap_start + i);
        }

        // Initialize Journal on disk
        journal.reset(&mut device)?;

        // 2. Allocate Root Inode (Should be ID after bitmap, effectively)
        let root_id = bitmap.allocate().ok_or(FileSystemError::NoSpace)?;
        superblock.root_inode = root_id;
        if superblock.free_blocks > 0 { superblock.free_blocks -= 1; }

        let root_inode = Inode::new(root_id, FileKind::Directory);
        let root_bytes = root_inode.to_bytes()?;
        let mut root_block = vec![0u8; BLOCK_SIZE as usize];
        root_block[..root_bytes.len()].copy_from_slice(&root_bytes);
        device.write_block(root_id, &root_block)?;

        // 3. Allocate Attribute Catalog Inode (System File)
        let catalog_id = bitmap.allocate().ok_or(FileSystemError::NoSpace)?;
        superblock.catalog_inode = catalog_id;
        if superblock.free_blocks > 0 { superblock.free_blocks -= 1; }

        let catalog_inode = Inode::new(catalog_id, FileKind::System);
        let catalog_bytes = catalog_inode.to_bytes()?;
        let mut catalog_block = vec![0u8; BLOCK_SIZE as usize];
        catalog_block[..catalog_bytes.len()].copy_from_slice(&catalog_bytes);
        device.write_block(catalog_id, &catalog_block)?;

        // 4. Save Metadata
        bitmap.save(&mut device, superblock.bitmap_start)?;

        let sb_bytes = superblock.to_bytes()?;
        let mut sb_block = vec![0u8; BLOCK_SIZE as usize];
        sb_block[..sb_bytes.len()].copy_from_slice(&sb_bytes);
        device.write_block(0, &sb_block)?;

        Ok(Self {
            device,
            superblock,
            bitmap,
            journal,
        })
    }

    /// Mount an existing UnaFS filesystem.
    pub fn mount(mut device: D) -> Result<Self, FileSystemError> {
        let mut sb_block = vec![0u8; BLOCK_SIZE as usize];
        device.read_block(0, &mut sb_block)?;
        let superblock = Superblock::from_bytes(&sb_block)?;

        let bitmap = SpaceMap::load(&mut device, superblock.bitmap_start, superblock.bitmap_blocks)?;
        let mut journal = Journal::new();

        // Check for recovery (Log only for now)
        if journal.check_recovery(&mut device)? {
            println!("[WARNING] :: DIRTY MOUNT DETECTED. TORN TRANSACTION IN JOURNAL.");
        }

        Ok(Self {
            device,
            superblock,
            bitmap,
            journal,
        })
    }

    /// Read an Inode by ID.
    pub fn read_inode(&mut self, id: u64) -> Result<Inode, FileSystemError> {
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
    fn create_inode_internal(&mut self, kind: FileKind, attributes: BTreeMap<String, AttributeValue>) -> Result<u64, FileSystemError> {
        let inode_id = self.allocate_inode_block()?;

        // Log generic creation intent
        self.journal.log(&mut self.device, JournalOp::BeginCreate { parent_inode: 0, name: "unknown".into() })?;

        let mut inode = Inode::new(inode_id, kind);
        inode.attributes = attributes;

        self.write_inode(&inode)?;
        self.sync_metadata()?;

        self.journal.log(&mut self.device, JournalOp::EndCreate { inode_id })?;

        Ok(inode_id)
    }

    pub fn create_inode(&mut self, attributes: BTreeMap<String, AttributeValue>) -> Result<u64, FileSystemError> {
        self.create_inode_internal(FileKind::File, attributes)
    }

    fn allocate_inode_block(&mut self) -> Result<u64, FileSystemError> {
        let block_id = self.bitmap.allocate().ok_or(FileSystemError::NoSpace)?;
        if self.superblock.free_blocks > 0 {
            self.superblock.free_blocks -= 1;
        }
        Ok(block_id)
    }

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

        self.journal.log(&mut self.device, JournalOp::BeginWrite { inode_id })?;

        let mut inode = self.read_inode(inode_id)?;
        let mut current_offset = offset;
        let mut data_written = 0;

        while data_written < data.len() {
            let block_offset = (current_offset % BLOCK_SIZE) as usize;
            let to_write = std::cmp::min(BLOCK_SIZE as usize - block_offset, data.len() - data_written);

            let mut physical_block = 0;
            let mut extent_found = false;

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
                let new_block = self.bitmap.allocate().ok_or(FileSystemError::NoSpace)?;
                if self.superblock.free_blocks > 0 {
                    self.superblock.free_blocks -= 1;
                }

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
                        length: BLOCK_SIZE,
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

        if current_offset > inode.size {
            inode.size = current_offset;
        }

        self.write_inode(&inode)?;
        self.sync_metadata()?;

        self.journal.log(&mut self.device, JournalOp::EndWrite { inode_id })?;

        Ok(())
    }

    /// Read data from an Inode.
    pub fn read_data(&mut self, inode_id: u64, offset: u64, length: u64) -> Result<Vec<u8>, FileSystemError> {
        let inode = self.read_inode(inode_id)?;
        self.read_from_extents(&inode.chunks, offset, length, inode.size)
    }

    /// Internal helper to read data from a specific ExtentList.
    fn read_from_extents(&mut self, chunks: &ExtentList, offset: u64, length: u64, total_size: u64) -> Result<Vec<u8>, FileSystemError> {
        let mut buffer = Vec::with_capacity(length as usize);
        let mut read_so_far = 0;
        let mut current_offset = offset;

        let available = total_size.saturating_sub(offset);
        let to_read_total = std::cmp::min(length, available);

        while read_so_far < to_read_total {
             let mut physical_block = 0;
             let mut found = false;

             for extent in chunks {
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

    pub fn ls(&mut self, inode_id: u64) -> Result<Vec<DirEntry>, FileSystemError> {
        let inode = self.read_inode(inode_id)?;
        if inode.kind != FileKind::Directory {
            return Err(FileSystemError::NotADirectory);
        }
        if inode.size == 0 {
            return Ok(Vec::new());
        }
        let data = self.read_data(inode_id, 0, inode.size)?;
        let entries: Vec<DirEntry> = bincode::deserialize(&data)?;
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
            if part.is_empty() { continue; }

            let entries = self.ls(current_id)?;
            let entry = entries.into_iter().find(|e| e.name == part).ok_or(FileSystemError::RootMissing)?; // TODO: specific error
            current_id = entry.inode_id;
        }

        Ok(current_id)
    }

    fn add_entry(&mut self, parent_id: u64, name: String, kind: FileKind) -> Result<u64, FileSystemError> {
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

        let new_id = self.create_inode_internal(kind, BTreeMap::new())?;

        entries.push(DirEntry {
            name,
            inode_id: new_id,
            kind,
        });
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        let data = bincode::serialize(&entries)?;
        self.write_data(parent_id, 0, &data)?;

        Ok(new_id)
    }

    // --- ATTRIBUTE ENGINE (The Soul) ---

    pub fn set_attribute(&mut self, inode_id: u64, key: String, value: AttributeValue) -> Result<(), FileSystemError> {
        self.journal.log(&mut self.device, JournalOp::BeginOp { op_id: inode_id, desc: format!("SetAttr: {}", key) })?;

        let mut inode = self.read_inode(inode_id)?;

        if let Some(extents) = inode.large_attributes.remove(&key) {
             self.free_extents(&extents)?;
        }

        let is_large = match &value {
            AttributeValue::Vector(v) => v.len() > 64, // > 256 bytes
            AttributeValue::Blob(b) => b.len() > 256,
            AttributeValue::String(s) => s.len() > 256,
            _ => false,
        };

        if is_large {
            let data = bincode::serialize(&value)?;
            let extents = self.allocate_and_write_extents(&data)?;
            inode.large_attributes.insert(key.clone(), extents);
            inode.attributes.remove(&key);
        } else {
            inode.attributes.insert(key.clone(), value.clone());
        }

        self.write_inode(&inode)?;
        self.update_catalog(&key, &value, inode_id)?;
        self.journal.log(&mut self.device, JournalOp::EndOp { op_id: inode_id })?;

        let msg = SMessage::FileEvent {
            path: format!("inode:{}", inode_id),
            event: format!("AttributeSet:{}", key)
        };
        let _ = self.publish("system/fs/change", msg);

        Ok(())
    }

    pub fn get_attribute(&mut self, inode_id: u64, key: &str) -> Result<Option<AttributeValue>, FileSystemError> {
        let inode = self.read_inode(inode_id)?;

        if let Some(val) = inode.attributes.get(key) {
            return Ok(Some(val.clone()));
        }

        if let Some(extents) = inode.large_attributes.get(key) {
            let total_size: u64 = extents.iter().map(|e| e.length).sum();
            let data = self.read_from_extents(extents, 0, total_size, total_size)?;
            let val: AttributeValue = bincode::deserialize(&data).map_err(|_| FileSystemError::InvalidAttributeData)?;
            return Ok(Some(val));
        }

        Ok(None)
    }

    // --- QUERY ENGINE ---

    pub fn query(&mut self, query_str: &str) -> Result<Vec<Inode>, FileSystemError> {
        let query = Query::parse(query_str).map_err(|e| FileSystemError::Query(e))?;

        let catalog_id = self.superblock.catalog_inode;
        let mut candidates = Vec::new();

        if catalog_id != 0 {
            let inode = self.read_inode(catalog_id)?;
            let data = self.read_data(catalog_id, 0, inode.size)?;
            let entries = deserialize_catalog(&data)?;

            // Use Stable Hasher
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
            let inode = self.read_inode(id)?;

            let mut val_opt = None;
            if let Some(v) = inode.attributes.get(&query.key) {
                val_opt = Some(v.clone());
            } else if let Some(extents) = inode.large_attributes.get(&query.key) {
                 let total = extents.iter().map(|e| e.length).sum();
                 let data = self.read_from_extents(extents, 0, total, total)?;
                 if let Ok(v) = bincode::deserialize::<AttributeValue>(&data) {
                     val_opt = Some(v);
                 }
            }

            if let Some(val) = val_opt {
                if check_condition(&val, &query.op, &query.value) {
                    results.push(inode);
                }
            }
        }

        Ok(results)
    }

    // --- HELPERS ---

    fn free_extents(&mut self, extents: &ExtentList) -> Result<(), FileSystemError> {
        for extent in extents {
            let blocks = extent.length.div_ceil(BLOCK_SIZE);
            for i in 0..blocks {
                self.bitmap.free(extent.physical_block + i);
                if self.superblock.free_blocks < self.superblock.block_count {
                    self.superblock.free_blocks += 1;
                }
            }
        }
        self.sync_metadata()?;
        Ok(())
    }

    fn allocate_and_write_extents(&mut self, data: &[u8]) -> Result<ExtentList, FileSystemError> {
        let mut extents = Vec::new();
        let mut data_written = 0;
        let mut current_logical = 0;

        while data_written < data.len() {
            let block_id = self.bitmap.allocate().ok_or(FileSystemError::NoSpace)?;
            if self.superblock.free_blocks > 0 { self.superblock.free_blocks -= 1; }

            let to_write = std::cmp::min(BLOCK_SIZE as usize, data.len() - data_written);

            let mut block = vec![0u8; BLOCK_SIZE as usize];
            block[..to_write].copy_from_slice(&data[data_written..data_written+to_write]);
            self.device.write_block(block_id, &block)?;

            extents.push(Extent {
                logical_offset: current_logical,
                physical_block: block_id,
                length: to_write as u64,
            });

            data_written += to_write;
            current_logical += to_write as u64;
        }

        self.sync_metadata()?;
        Ok(extents)
    }

    fn update_catalog(&mut self, key: &str, value: &AttributeValue, inode_id: u64) -> Result<(), FileSystemError> {
        let catalog_id = self.superblock.catalog_inode;
        if catalog_id == 0 { return Ok(()); }

        let inode = self.read_inode(catalog_id)?;
        let data = self.read_data(catalog_id, 0, inode.size)?;
        let mut entries = deserialize_catalog(&data)?;

        entries.push(CatalogEntry::new(key, value, inode_id));

        let new_data = serialize_catalog(&entries)?;
        self.write_data(catalog_id, 0, &new_data)?;

        Ok(())
    }
}

impl<D: BlockDevice> BandyMember for UnaFS<D> {
    fn publish(&self, topic: &str, msg: SMessage) -> anyhow::Result<()> {
        println!("[UNAFS] Broadcasting event to '{}': {:?}", topic, msg);
        Ok(())
    }
}

pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() { return 0.0; }
    let dot: f32 = a.iter().zip(b).map(|(x, y)| x * y).sum();
    let mag_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let mag_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if mag_a == 0.0 || mag_b == 0.0 { return 0.0; }
    dot / (mag_a * mag_b)
}

fn check_condition(val: &AttributeValue, op: &QueryOp, target: &AttributeValue) -> bool {
    match op {
        QueryOp::Eq => val == target,
        QueryOp::Neq => val != target,
        QueryOp::Gt => partial_cmp_attr(val, target).map(|o| o.is_gt()).unwrap_or(false),
        QueryOp::Lt => partial_cmp_attr(val, target).map(|o| o.is_lt()).unwrap_or(false),
        QueryOp::SimilarityGt(threshold) => {
            if let (AttributeValue::Vector(v1), AttributeValue::Vector(v2)) = (val, target) {
                cosine_similarity(v1, v2) > *threshold
            } else {
                false
            }
        }
    }
}

use std::cmp::Ordering;
fn partial_cmp_attr(a: &AttributeValue, b: &AttributeValue) -> Option<Ordering> {
    match (a, b) {
        (AttributeValue::Int(i1), AttributeValue::Int(i2)) => i1.partial_cmp(i2),
        (AttributeValue::Float(f1), AttributeValue::Float(f2)) => f1.partial_cmp(f2),
        (AttributeValue::String(s1), AttributeValue::String(s2)) => s1.partial_cmp(s2),
        _ => None,
    }
}
