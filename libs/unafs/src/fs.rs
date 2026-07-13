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

use crate::bitmap::SpaceMap;
use crate::catalog::{CatalogEntry, deserialize_catalog, serialize_catalog};
use crate::inode::{AttributeValue, Extent, ExtentList, FileKind, Inode, InodeError};
use crate::storage::{BLOCK_SIZE, BlockDevice, Error as StorageError};
use crate::superblock::{Superblock, SuperblockError};
use crate::wal::{Journal, JournalOp};
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
    #[error("Journal error: {0}")]
    Journal(#[from] crate::wal::JournalError),
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
        if superblock.free_blocks > 0 {
            superblock.free_blocks -= 1;
        }

        let root_inode = Inode::new(root_id, FileKind::Directory);
        let root_bytes = root_inode.to_bytes()?;
        let mut root_block = vec![0u8; BLOCK_SIZE as usize];
        root_block[..root_bytes.len()].copy_from_slice(&root_bytes);
        device.write_block(root_id, &root_block)?;

        // 3. Allocate Attribute Catalog Inode (System File)
        let catalog_id = bitmap.allocate().ok_or(FileSystemError::NoSpace)?;
        superblock.catalog_inode = catalog_id;
        if superblock.free_blocks > 0 {
            superblock.free_blocks -= 1;
        }

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

        let bitmap = SpaceMap::load(
            &mut device,
            superblock.bitmap_start,
            superblock.bitmap_blocks,
        )?;
        let mut journal = Journal::new();

        // Check for recovery (Log only for now)
        if journal.check_recovery(&mut device)? {
            #[cfg(feature = "std")]
            println!("[WARNING] :: DIRTY MOUNT DETECTED. TORN TRANSACTION IN JOURNAL.");
            // K3 observability seam: also surface the warning through the no_std
            // hook, so a kernel (no println) mount is not silent about a torn journal.
            crate::warnlog::warn("[WARNING] :: DIRTY MOUNT DETECTED. TORN TRANSACTION IN JOURNAL.");
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
    fn create_inode_internal(
        &mut self,
        kind: FileKind,
        attributes: BTreeMap<String, AttributeValue>,
    ) -> Result<u64, FileSystemError> {
        let inode_id = self.allocate_inode_block()?;

        // Log generic creation intent
        self.journal.log(
            &mut self.device,
            JournalOp::BeginCreate {
                parent_inode: 0,
                name: "unknown".into(),
            },
        )?;

        let mut inode = Inode::new(inode_id, kind);
        inode.attributes = attributes;

        self.write_inode(&inode)?;
        self.sync_metadata()?;

        self.journal
            .log(&mut self.device, JournalOp::EndCreate { inode_id })?;

        Ok(inode_id)
    }

    pub fn create_inode(
        &mut self,
        attributes: BTreeMap<String, AttributeValue>,
    ) -> Result<u64, FileSystemError> {
        self.create_inode_internal(FileKind::File, attributes)
    }

    fn allocate_inode_block(&mut self) -> Result<u64, FileSystemError> {
        let block_id = self.bitmap.allocate().ok_or(FileSystemError::NoSpace)?;
        if self.superblock.free_blocks > 0 {
            self.superblock.free_blocks -= 1;
        }
        Ok(block_id)
    }

    pub fn sync_metadata(&mut self) -> Result<(), FileSystemError> {
        self.bitmap
            .save(&mut self.device, self.superblock.bitmap_start)?;

        let sb_bytes = self.superblock.to_bytes()?;
        let mut sb_block = vec![0u8; BLOCK_SIZE as usize];
        sb_block[..sb_bytes.len()].copy_from_slice(&sb_bytes);
        self.device.write_block(0, &sb_block)?;
        Ok(())
    }

    /// Write data to an Inode.
    pub fn write_data(
        &mut self,
        inode_id: u64,
        offset: u64,
        data: &[u8],
    ) -> Result<(), FileSystemError> {
        if data.is_empty() {
            return Ok(());
        }

        self.journal
            .log(&mut self.device, JournalOp::BeginWrite { inode_id })?;

        let mut inode = self.read_inode(inode_id)?;
        let mut current_offset = offset;
        let mut data_written = 0;

        while data_written < data.len() {
            let block_offset = (current_offset % BLOCK_SIZE) as usize;
            let to_write = core::cmp::min(
                BLOCK_SIZE as usize - block_offset,
                data.len() - data_written,
            );

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
                        && last_physical_end + 1 == new_block
                    {
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
            block_buf[block_offset..block_offset + to_write]
                .copy_from_slice(&data[data_written..data_written + to_write]);
            self.device.write_block(physical_block, &block_buf)?;

            data_written += to_write;
            current_offset += to_write as u64;
        }

        if current_offset > inode.size {
            inode.size = current_offset;
        }

        self.write_inode(&inode)?;
        self.sync_metadata()?;

        self.journal
            .log(&mut self.device, JournalOp::EndWrite { inode_id })?;

        Ok(())
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
    fn read_from_extents(
        &mut self,
        chunks: &ExtentList,
        offset: u64,
        length: u64,
        total_size: u64,
    ) -> Result<Vec<u8>, FileSystemError> {
        let mut buffer = Vec::with_capacity(length as usize);
        let mut read_so_far = 0;
        let mut current_offset = offset;

        let available = total_size.saturating_sub(offset);
        let to_read_total = core::cmp::min(length, available);

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
            let to_read = core::cmp::min(
                BLOCK_SIZE as usize - block_offset,
                (to_read_total - read_so_far) as usize,
            );

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
                .ok_or(FileSystemError::RootMissing)?; // TODO: specific error
            current_id = entry.inode_id;
        }

        Ok(current_id)
    }

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

        let new_id = self.create_inode_internal(kind, BTreeMap::new())?;

        entries.push(DirEntry {
            name,
            inode_id: new_id,
            kind,
        });
        entries.sort_by(|a, b| a.name.cmp(&b.name));

        let data = crate::codec::serialize(&entries)?;
        self.write_data(parent_id, 0, &data)?;

        Ok(new_id)
    }

    // --- ATTRIBUTE ENGINE (The Soul) ---

    pub fn set_attribute(
        &mut self,
        inode_id: u64,
        key: String,
        value: AttributeValue,
    ) -> Result<(), FileSystemError> {
        self.journal.log(
            &mut self.device,
            JournalOp::BeginOp {
                op_id: inode_id,
                desc: format!("SetAttr: {}", key),
            },
        )?;

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
            let data = crate::codec::serialize(&value)?;
            let extents = self.allocate_and_write_extents(&data)?;
            inode.large_attributes.insert(key.clone(), extents);
            inode.attributes.remove(&key);
        } else {
            inode.attributes.insert(key.clone(), value.clone());
        }

        self.write_inode(&inode)?;
        self.update_catalog(&key, &value, inode_id)?;
        self.sync_metadata()?;
        self.journal
            .log(&mut self.device, JournalOp::EndOp { op_id: inode_id })?;

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
            let total_size: u64 = extents.iter().map(|e| e.length).sum();
            let data = self.read_from_extents(extents, 0, total_size, total_size)?;
            let val: AttributeValue =
                crate::codec::deserialize(&data).map_err(|_| FileSystemError::InvalidAttributeData)?;
            return Ok(Some(val));
        }

        Ok(None)
    }

    // --- MUTATION ENGINE (unlink / rename / remove_attribute) ---
    //
    // Crash-window honesty: there is NO journal replay — the WAL detects a
    // torn operation on the next mount, it does not roll it back. Every op
    // below is therefore ordered so that no intermediate on-disk state ever
    // references a freed block: the failure mode of an ill-timed power cut
    // is a LEAK (allocated-but-unreachable blocks, reclaimable only by a
    // future fsck/scavenger arc), never a dangling reference. The concrete
    // windows are documented on each method.

    /// Remove `name` from directory `parent_id`: the directory entry, every
    /// attribute-catalog index entry pointing at the inode, the inode's data
    /// extents, its spilled attribute extents, and the inode block itself
    /// all go. After completion the file is unreachable by name AND by
    /// query. Returns the freed inode id.
    ///
    /// Only `File` and `Symlink` entries can be unlinked; a directory is
    /// refused with [`FileSystemError::IsADirectory`].
    ///
    /// # Open-handle semantics (honest note)
    /// unafs has no open-file table — callers address files by raw inode id.
    /// `unlink` invalidates the id immediately: a caller that keeps the id
    /// and calls `read_data`/`write_data` afterwards touches freed (and
    /// possibly reallocated) blocks. That is the caller's problem, by
    /// design; a host library cannot know who still holds an id.
    ///
    /// # Crash windows (no journal replay; ordered leak-not-dangle)
    /// 1. After the catalog rewrite, before the directory rewrite: the file
    ///    is still reachable by name but invisible to queries (unindexed).
    /// 2. After the directory rewrite, before the frees: the inode and all
    ///    its blocks are leaked (allocated, unreachable by name or query).
    /// 3. The catalog and directory rewrites each write new extents first
    ///    and swap the inode last (a single-block write), so each is
    ///    old-or-new, never torn; an interrupted rewrite leaks the freshly
    ///    written extents.
    /// A power cut anywhere inside the op additionally leaves an unmatched
    /// `BeginOp` in the journal, reported as a dirty mount.
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

        self.journal.log(
            &mut self.device,
            JournalOp::BeginOp {
                op_id: inode_id,
                desc: format!("Unlink: {}", name),
            },
        )?;

        // Read the doomed inode up front; its extents are freed at the end.
        let inode = self.read_inode(inode_id)?;

        // 1. Scrub the attribute index: every catalog entry pointing at this
        //    inode goes. (`set_attribute` appends on re-set, so duplicate
        //    entries for one key are expected — all of them are removed.)
        self.remove_catalog_entries(|e| e.inode_id == inode_id)?;

        // 2. Make the name unreachable.
        entries.remove(pos);
        let dir_data = crate::codec::serialize(&entries)?;
        self.rewrite_data(parent_id, &dir_data)?;

        // 3. Free everything the inode owned: spilled attribute extents,
        //    data extents, then the inode block itself. The block contents
        //    are not zeroed — the blocks are simply marked free and will be
        //    reused by the first-fit allocator.
        for extents in inode.large_attributes.values() {
            self.free_extents(extents)?;
        }
        self.free_extents(&inode.chunks)?;
        self.free_block(inode_id);
        self.sync_metadata()?;

        self.journal
            .log(&mut self.device, JournalOp::EndOp { op_id: inode_id })?;

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
    /// move otherwise. The inode, its data extents, and its attributes are
    /// untouched: only directory entries change, and the attribute catalog
    /// keys on inode id (names are not indexed), so the catalog is
    /// consistent by construction — queries return the renamed file
    /// unchanged. The format already models a move this way: directories
    /// are independent serialized entry lists, and an entry simply switches
    /// lists. No new format structure is invented.
    ///
    /// `new_name` must not already exist — this refuses with `FileExists`
    /// rather than implicitly unlinking the target (a deliberate divergence
    /// from POSIX rename's overwrite). Renaming an entry to its own name in
    /// the same directory is a no-op `Ok`. Moving a DIRECTORY into itself
    /// or one of its descendants is refused with `DirectoryLoop`: the tree
    /// has no parent pointers, so a cycle would orphan the whole subtree.
    ///
    /// # Crash windows (no journal replay)
    /// * Same-directory: one crash-ordered directory rewrite — old-or-new,
    ///   never torn.
    /// * Cross-directory: the entry leaves the source directory FIRST, then
    ///   joins the destination. A power cut between the two leaves the file
    ///   in NEITHER directory — the inode and its blocks are leaked (still
    ///   allocated, still query-reachable via the catalog, unreachable by
    ///   name). The reverse order was rejected deliberately: it would
    ///   briefly give one inode TWO names, and unlinking either name would
    ///   free blocks the other still references.
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

        self.journal.log(
            &mut self.device,
            JournalOp::BeginOp {
                op_id: moved.inode_id,
                desc: format!("Rename: {} -> {}", old_name, new_name),
            },
        )?;

        if same_dir {
            src_entries[pos].name = new_name.into();
            src_entries.sort_by(|a, b| a.name.cmp(&b.name));
            let data = crate::codec::serialize(&src_entries)?;
            self.rewrite_data(parent_id, &data)?;
        } else {
            // Remove-then-add: see the crash-window note above.
            src_entries.remove(pos);
            let src_data = crate::codec::serialize(&src_entries)?;
            self.rewrite_data(parent_id, &src_data)?;

            let mut dst_entries = self.ls(new_parent_id)?;
            dst_entries.push(DirEntry {
                name: new_name.into(),
                inode_id: moved.inode_id,
                kind: moved.kind,
            });
            dst_entries.sort_by(|a, b| a.name.cmp(&b.name));
            let dst_data = crate::codec::serialize(&dst_entries)?;
            self.rewrite_data(new_parent_id, &dst_data)?;
        }

        self.journal.log(
            &mut self.device,
            JournalOp::EndOp {
                op_id: moved.inode_id,
            },
        )?;

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

    // --- QUERY ENGINE ---

    /// Semantic query engine. no_std-capable: the similarity path routes its
    /// floating-point `sqrt` through `libm`, so kernel (`no_std`) and host
    /// (`std`) builds score along the same code path.
    pub fn query(&mut self, query_str: &str) -> Result<Vec<(Inode, f32)>, FileSystemError> {
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

    // --- HELPERS ---

    /// Return a single block to the free pool (in-memory; callers persist
    /// via `sync_metadata`).
    fn free_block(&mut self, block_id: u64) {
        self.bitmap.free(block_id);
        if self.superblock.free_blocks < self.superblock.block_count {
            self.superblock.free_blocks += 1;
        }
    }

    fn free_extents(&mut self, extents: &ExtentList) -> Result<(), FileSystemError> {
        for extent in extents {
            let blocks = extent.length.div_ceil(BLOCK_SIZE);
            for i in 0..blocks {
                self.free_block(extent.physical_block + i);
            }
        }
        self.sync_metadata()?;
        Ok(())
    }

    /// Replace an inode's entire data contents, crash-ordered: the new bytes
    /// are written to freshly allocated extents FIRST, then the inode is
    /// swapped to point at them (a single-block write — the atomic point),
    /// and only then are the old extents freed. A power cut leaves the
    /// inode's data old-or-new, never torn; an interrupted rewrite leaks
    /// blocks, never dangles. Unlike `write_data`, the logical size shrinks
    /// to exactly `data.len()`.
    fn rewrite_data(&mut self, inode_id: u64, data: &[u8]) -> Result<(), FileSystemError> {
        let new_chunks = if data.is_empty() {
            Vec::new()
        } else {
            coalesce_extents(self.allocate_and_write_extents(data)?)
        };
        let mut inode = self.read_inode(inode_id)?;
        let old_chunks = core::mem::replace(&mut inode.chunks, new_chunks);
        inode.size = data.len() as u64;
        self.write_inode(&inode)?;
        self.free_extents(&old_chunks)?;
        Ok(())
    }

    /// Depth-first walk: does directory `dir_id` live anywhere inside the
    /// subtree rooted at `root_id`? Used by `rename` to refuse moving a
    /// directory into itself or its descendants.
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
    /// removed. No-op (and no rewrite) when nothing matches.
    fn remove_catalog_entries<F: Fn(&CatalogEntry) -> bool>(
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
        self.rewrite_data(catalog_id, &new_data)?;
        Ok(())
    }

    fn allocate_and_write_extents(&mut self, data: &[u8]) -> Result<ExtentList, FileSystemError> {
        let mut extents = Vec::new();
        let mut data_written = 0;
        let mut current_logical = 0;

        while data_written < data.len() {
            let block_id = self.bitmap.allocate().ok_or(FileSystemError::NoSpace)?;
            if self.superblock.free_blocks > 0 {
                self.superblock.free_blocks -= 1;
            }

            let to_write = core::cmp::min(BLOCK_SIZE as usize, data.len() - data_written);

            let mut block = vec![0u8; BLOCK_SIZE as usize];
            block[..to_write].copy_from_slice(&data[data_written..data_written + to_write]);
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
        self.write_data(catalog_id, 0, &new_data)?;

        Ok(())
    }
}

impl<D: BlockDevice> Drop for UnaFS<D> {
    fn drop(&mut self) {
        let _ = self.sync_metadata();
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

/// Merge logically and physically contiguous extents into single runs, so a
/// rewritten catalog or directory keeps its extent list (and therefore its
/// inode, which must fit in one block) small. This is purely an in-memory
/// shaping of extent VALUES — the on-disk `Extent` encoding is untouched.
fn coalesce_extents(extents: ExtentList) -> ExtentList {
    let mut merged: ExtentList = Vec::with_capacity(extents.len());
    for e in extents {
        match merged.last_mut() {
            Some(last)
                if last.length % BLOCK_SIZE == 0
                    && last.logical_offset + last.length == e.logical_offset
                    && last.physical_block + last.length / BLOCK_SIZE == e.physical_block =>
            {
                last.length += e.length;
            }
            _ => merged.push(e),
        }
    }
    merged
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
