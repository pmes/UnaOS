// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! FORMAT — the one way a UnaFS volume is made (AMBER1, SR34). Before this module the `init` verb of
//! `tools/unafs` was the only caller-facing format path: it sized a host file, opened it and called
//! [`UnaFS::format`] inline, so the installer ensemble (Amber Bytes, the kernel installer) had no
//! format entry of its own. [`format`] is that entry, over ANY [`BlockDevice`] — a host file, a
//! [`MemDevice`](crate::MemDevice), or a partition of a 512 B-sector medium through
//! [`BlockAdapter`] ([`format_partition`]). `tools/unafs init` and `handlers/amber_bytes` both call
//! it; the bytes it writes are exactly those of [`UnaFS::format_with_version`] (it is that function
//! behind a parameter block and a geometry check).
//!
//! Determinism: the format writes the clock ([`crate::clock::now`]) into the reserved inodes'
//! timestamps; with a clock hook installed the format is byte-reproducible (the KATs pin it).

use crate::adapter::{BlockAdapter, SECTORS_PER_BLOCK, SectorDevice};
use crate::fs::{FileSystemError, UnaFS};
use crate::storage::{BLOCK_SIZE, BlockDevice};
use crate::superblock::{MAX_BLOCK_COUNT, MIN_SUPPORTED_VERSION, SuperblockError, VERSION};

/// The smallest volume [`format`] accepts, in 4096 B blocks (1 MiB): the superblock, the root
/// area, the reserved inodes, the maps and the catalog trees with room to spare.
pub const MIN_BLOCKS: u64 = 256;

/// What to make.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FormatParams {
    /// On-disk version, in `MIN_SUPPORTED_VERSION..=VERSION` (default: the current one).
    pub version: u32,
    /// Volume size in blocks; `None` = the whole device.
    pub blocks: Option<u64>,
}

impl Default for FormatParams {
    fn default() -> Self {
        Self { version: VERSION, blocks: None }
    }
}

impl FormatParams {
    /// The current version, sized to `size_mb` MiB (the `tools/unafs init --size-mb` spelling).
    pub fn sized_mb(size_mb: u64) -> Self {
        Self { version: VERSION, blocks: Some(size_mb.saturating_mul(1024 * 1024) / BLOCK_SIZE) }
    }
}

/// Why a volume was not formatted (before a byte was written).
fn refuse(why: &'static str) -> FileSystemError {
    SuperblockError::Geometry(why).into()
}

/// The volume size [`format`] will use on a device of `device_blocks` blocks.
pub fn volume_blocks(device_blocks: u64, p: &FormatParams) -> Result<u64, FileSystemError> {
    if !(MIN_SUPPORTED_VERSION..=VERSION).contains(&p.version) {
        return Err(SuperblockError::InvalidVersion(p.version).into());
    }
    let n = match p.blocks {
        Some(b) if device_blocks != 0 && b > device_blocks => {
            return Err(refuse("the volume is larger than the device"));
        }
        Some(b) => b,
        None => device_blocks,
    };
    if !(MIN_BLOCKS..=MAX_BLOCK_COUNT).contains(&n) {
        return Err(refuse("volume size outside MIN_BLOCKS..=MAX_BLOCK_COUNT"));
    }
    Ok(n)
}

/// A device narrowed to its first `blocks` blocks, so the superblock records the volume, not the
/// medium (a 64 MiB volume in a 1 GiB file is a 64 MiB volume).
struct Bounded<'a, D> {
    dev: &'a mut D,
    blocks: u64,
}

impl<D: BlockDevice> BlockDevice for Bounded<'_, D> {
    fn read_block(&mut self, id: u64, buf: &mut [u8]) -> Result<(), crate::storage::Error> {
        if id >= self.blocks {
            return Err(crate::storage::Error::OutOfBounds(id));
        }
        self.dev.read_block(id, buf)
    }
    fn write_block(&mut self, id: u64, buf: &[u8]) -> Result<(), crate::storage::Error> {
        if id >= self.blocks {
            return Err(crate::storage::Error::OutOfBounds(id));
        }
        self.dev.write_block(id, buf)
    }
    fn block_count(&self) -> u64 {
        self.blocks
    }
    fn flush(&mut self) -> Result<(), crate::storage::Error> {
        self.dev.flush()
    }
    fn write_sector_in_block(&mut self, id: u64, sector: usize, buf: &[u8]) -> Result<(), crate::storage::Error> {
        if id >= self.blocks {
            return Err(crate::storage::Error::OutOfBounds(id));
        }
        self.dev.write_sector_in_block(id, sector, buf)
    }
}

/// What [`format`] made.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Formatted {
    pub version: u32,
    pub blocks: u64,
    /// The root generation the format commit wrote (1).
    pub generation: u64,
    pub free_blocks: u64,
}

/// Format `block` with a fresh UnaFS (see the module doc) and flush it. Refuses — before writing —
/// a version outside the supported range, a volume larger than the device, or one outside
/// `MIN_BLOCKS..=MAX_BLOCK_COUNT`. The volume is committed (generation 1) when this returns; mount
/// it with [`UnaFS::mount`].
pub fn format<D: BlockDevice>(block: &mut D, params: &FormatParams) -> Result<Formatted, FileSystemError> {
    let blocks = volume_blocks(block.block_count(), params)?;
    let fs = UnaFS::format_with_version(Bounded { dev: block, blocks }, 0, params.version)?;
    let rep = Formatted {
        version: params.version,
        blocks,
        generation: fs.root_generation(),
        free_blocks: fs.free_blocks(),
    };
    drop(fs); // Drop flushes the device.
    block.flush()?;
    Ok(rep)
}

/// Format the partition of a 512 B-sector medium that spans `sectors` sectors from `base_lba`
/// (whole 4096 B blocks; a tail of fewer than eight sectors is left unused).
pub fn format_partition<S: SectorDevice + ?Sized>(
    dev: &mut S,
    base_lba: u64,
    sectors: u64,
    params: &FormatParams,
) -> Result<Formatted, FileSystemError> {
    let end = base_lba.checked_add(sectors).ok_or_else(|| refuse("partition span overflows"))?;
    if end > dev.sector_count() {
        return Err(refuse("partition runs past the end of the medium"));
    }
    let mut adapter = BlockAdapter::new(dev, base_lba, sectors / SECTORS_PER_BLOCK);
    format(&mut adapter, params)
}
