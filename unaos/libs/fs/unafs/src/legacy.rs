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

//! K8a: READ-ONLY access to the pre-K8 (version 2) on-disk format, existing
//! solely to feed the one-way migration pass (`tools/unafs migrate`).
//!
//! Per the do-it-right principle there is NO runtime compatibility with our
//! own old format: the K8 code speaks only version 3. A pre-K8 card is
//! converted once — walk the old tree read-only through this module, replay
//! it into a freshly formatted K8 volume — and the old format is retired.
//! Only the structures needed for a faithful walk live here: the v2
//! superblock and its (block-addressed) inode/extent tree. The `Inode`,
//! `DirEntry`, and `AttributeValue` encodings are unchanged between v2 and
//! v3, so those types are shared with the live crate.

use crate::fs::{DirEntry, FileSystemError};
use crate::inode::{AttributeValue, ExtentList, FileKind, Inode};
use crate::storage::{BLOCK_SIZE, BlockDevice, Error as StorageError};
use alloc::string::String;
use alloc::vec::Vec;
use serde::{Deserialize, Serialize};

/// The version this module (and only this module) can read.
pub const LEGACY_VERSION: u32 = 2;

/// The version-2 superblock, exactly as `Superblock` looked pre-K8.
/// (`Serialize` is derived only so tests can BUILD v2 fixtures to migrate;
/// no production path ever writes this format again.)
#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct LegacySuperblock {
    pub magic: [u8; 5],
    pub version: u32,
    pub block_size: u32,
    pub block_count: u64,
    pub root_inode: u64,
    pub free_blocks: u64,
    pub bitmap_start: u64,
    pub bitmap_blocks: u64,
    pub journal_start: u64,
    pub journal_blocks: u64,
    pub catalog_inode: u64,
}

/// A read-only view over a version-2 volume. Inode ids ARE physical block
/// ids in this format (the pre-Fork-1 identity).
pub struct LegacyVolume<D: BlockDevice> {
    pub device: D,
    pub superblock: LegacySuperblock,
}

impl<D: BlockDevice> LegacyVolume<D> {
    /// Open a v2 volume read-only. Refuses anything that is not magic-valid
    /// version 2 with sane geometry.
    pub fn open(mut device: D) -> Result<Self, FileSystemError> {
        let mut block = alloc::vec![0u8; BLOCK_SIZE as usize];
        device.read_block(0, &mut block)?;
        let sb: LegacySuperblock = crate::codec::deserialize_block(&block)?;
        if sb.magic != crate::superblock::MAGIC {
            return Err(FileSystemError::CorruptVolume("legacy: bad magic"));
        }
        if sb.version != LEGACY_VERSION {
            return Err(FileSystemError::CorruptVolume("legacy: not a v2 volume"));
        }
        if sb.block_size as u64 != BLOCK_SIZE {
            return Err(FileSystemError::CorruptVolume("legacy: bad block size"));
        }
        if sb.root_inode == 0 || sb.root_inode >= sb.block_count {
            return Err(FileSystemError::CorruptVolume("legacy: root out of bounds"));
        }
        if sb.catalog_inode >= sb.block_count {
            return Err(FileSystemError::CorruptVolume(
                "legacy: catalog out of bounds",
            ));
        }
        Ok(Self {
            device,
            superblock: sb,
        })
    }

    /// Read a v2 inode by its (physical-block) id.
    pub fn read_inode(&mut self, id: u64) -> Result<Inode, FileSystemError> {
        if id == 0 || id >= self.superblock.block_count {
            return Err(FileSystemError::CorruptVolume("legacy: inode out of bounds"));
        }
        let mut block = alloc::vec![0u8; BLOCK_SIZE as usize];
        self.device.read_block(id, &mut block)?;
        Ok(Inode::from_bytes(&block)?)
    }

    /// Read `total_size` bytes through a v2 extent list (bounded like the
    /// live read path — the old volume is untrusted input too).
    pub fn read_extents(
        &mut self,
        extents: &ExtentList,
        total_size: u64,
    ) -> Result<Vec<u8>, FileSystemError> {
        let volume_bytes = self
            .superblock
            .block_count
            .checked_mul(BLOCK_SIZE)
            .ok_or(FileSystemError::CorruptVolume("legacy: volume overflows"))?;
        if total_size > volume_bytes {
            return Err(FileSystemError::CorruptVolume(
                "legacy: data size exceeds volume",
            ));
        }
        let cap = usize::try_from(total_size)
            .map_err(|_| StorageError::AllocRefused(total_size))?;
        let mut out = Vec::new();
        out.try_reserve_exact(cap)
            .map_err(|_| StorageError::AllocRefused(total_size))?;
        out.resize(cap, 0u8);

        let mut block_buf = alloc::vec![0u8; BLOCK_SIZE as usize];
        for extent in extents {
            let end = extent
                .logical_offset
                .checked_add(extent.length)
                .ok_or(FileSystemError::CorruptVolume("legacy: extent overflows"))?;
            let take_end = core::cmp::min(end, total_size);
            let mut logical = extent.logical_offset;
            let mut i = 0u64;
            while logical < take_end {
                let pb = extent
                    .physical_block
                    .checked_add(i)
                    .ok_or(FileSystemError::CorruptVolume("legacy: extent target"))?;
                if pb >= self.superblock.block_count {
                    return Err(FileSystemError::CorruptVolume(
                        "legacy: extent past volume",
                    ));
                }
                self.device.read_block(pb, &mut block_buf)?;
                let n = core::cmp::min(BLOCK_SIZE, take_end - logical) as usize;
                out[logical as usize..logical as usize + n]
                    .copy_from_slice(&block_buf[..n]);
                logical += n as u64;
                i += 1;
            }
        }
        Ok(out)
    }

    /// Read a v2 file's whole data.
    pub fn read_file(&mut self, inode: &Inode) -> Result<Vec<u8>, FileSystemError> {
        self.read_extents(&inode.chunks.clone(), inode.size)
    }

    /// List a v2 directory.
    pub fn ls(&mut self, id: u64) -> Result<Vec<DirEntry>, FileSystemError> {
        let inode = self.read_inode(id)?;
        if inode.kind != FileKind::Directory {
            return Err(FileSystemError::NotADirectory);
        }
        if inode.size == 0 {
            return Ok(Vec::new());
        }
        let data = self.read_extents(&inode.chunks.clone(), inode.size)?;
        Ok(crate::codec::deserialize(&data)?)
    }

    /// Every attribute on a v2 inode, spilled values materialized.
    pub fn attributes(
        &mut self,
        inode: &Inode,
    ) -> Result<Vec<(String, AttributeValue)>, FileSystemError> {
        let mut out: Vec<(String, AttributeValue)> = Vec::new();
        for (k, v) in &inode.attributes {
            out.push((k.clone(), v.clone()));
        }
        for (k, extents) in inode.large_attributes.clone() {
            let total = extents
                .iter()
                .try_fold(0u64, |acc, e| acc.checked_add(e.length))
                .ok_or(FileSystemError::CorruptVolume("legacy: attr overflows"))?;
            let data = self.read_extents(&extents, total)?;
            let val: AttributeValue = crate::codec::deserialize(&data)
                .map_err(|_| FileSystemError::InvalidAttributeData)?;
            out.push((k, val));
        }
        Ok(out)
    }
}

/// Replay a v2 volume's entire tree (directories, files, attributes) into a
/// freshly formatted K8 filesystem — the migration pass's core. The target
/// must be empty (freshly formatted); name collisions surface as errors.
pub fn migrate_into<S: BlockDevice, T: BlockDevice>(
    old: &mut LegacyVolume<S>,
    new: &mut crate::UnaFS<T>,
) -> Result<MigrationReport, FileSystemError> {
    let mut report = MigrationReport::default();

    // Root attributes first (the root inode can carry attributes too).
    let old_root = old.read_inode(old.superblock.root_inode)?;
    let new_root = new.superblock.root_inode;
    for (k, v) in old.attributes(&old_root)? {
        new.set_attribute(new_root, k, v)?;
    }

    // DFS over the old name tree, mirroring into the new one.
    let mut stack: Vec<(u64, u64)> = alloc::vec![(old.superblock.root_inode, new_root)];
    while let Some((old_dir, new_dir)) = stack.pop() {
        for entry in old.ls(old_dir)? {
            let old_inode = old.read_inode(entry.inode_id)?;
            match entry.kind {
                FileKind::Directory => {
                    let nd = new.mkdir(new_dir, entry.name.clone())?;
                    report.directories += 1;
                    for (k, v) in old.attributes(&old_inode)? {
                        new.set_attribute(nd, k, v)?;
                    }
                    stack.push((entry.inode_id, nd));
                }
                FileKind::File | FileKind::Symlink | FileKind::System => {
                    let nf = new.create_file(new_dir, entry.name.clone())?;
                    let data = old.read_file(&old_inode)?;
                    if !data.is_empty() {
                        new.write_data(nf, 0, &data)?;
                        report.bytes += data.len() as u64;
                    }
                    for (k, v) in old.attributes(&old_inode)? {
                        new.set_attribute(nf, k, v)?;
                    }
                    report.files += 1;
                }
            }
        }
    }

    Ok(report)
}

/// What a migration replayed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MigrationReport {
    pub files: u64,
    pub directories: u64,
    pub bytes: u64,
}
