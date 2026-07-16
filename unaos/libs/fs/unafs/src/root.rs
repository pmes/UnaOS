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

//! K8a: the copy-on-write ROOT RECORD — the single 512 B sector the whole
//! live tree hangs from, and the only thing a commit mutates in place.
//!
//! ## Fork-2 verdict (Peter, 2026-07-16): A/B double-buffered root sectors
//!
//! Block 1 (immediately after the 4096 B superblock block, inside the
//! existing GPT/MBR partition — external standards untouched) reserves two
//! 512 B root slots: slot A = sector 0 of the block, slot B = sector 1.
//! A commit writes the whole new tree to FRESH blocks, barriers, then writes
//! the **inactive** slot with a generation-stamped, checksummed root record.
//! Mount reads both slots and picks the newer VALID one. A torn root-sector
//! write therefore corrupts only the slot being written; the other slot still
//! carries the previous committed tree — "old tree or new tree, never a
//! hybrid" holds even against a tear inside the 512 B root write itself.
//!
//! The record is a HAND-PACKED little-endian layout (not bincode): its size
//! is a compile-time constant, enforced ≤ 512 both by `const` assert and by
//! the golden-vector KAT (`tests/kat_vectors.rs`).

use crate::storage::{BlockDevice, Error as StorageError};

/// The block that carries the two root slots.
pub const ROOT_BLOCK: u64 = 1;
/// Root-record magic: "UNAFSRT1".
pub const ROOT_MAGIC: [u8; 8] = *b"UNAFSRT1";
/// The exact packed size of a [`RootRecord`] on disk (bytes).
pub const ROOT_RECORD_SIZE: usize = 80;
/// The atomicity unit of record: one 512 B sector.
pub const ROOT_SECTOR_SIZE: usize = 512;

// THE root-fits-one-sector guarantee, at compile time (brief §1). The KAT in
// `tests/kat_vectors.rs` enforces the same bound on the serialized bytes.
const _: () = assert!(ROOT_RECORD_SIZE <= ROOT_SECTOR_SIZE);

/// Which of the two root slots a record was read from / should be written to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RootSlot {
    /// Sector 0 of [`ROOT_BLOCK`].
    A,
    /// Sector 1 of [`ROOT_BLOCK`].
    B,
}

impl RootSlot {
    /// The other slot (the one a commit writes).
    pub fn other(self) -> RootSlot {
        match self {
            RootSlot::A => RootSlot::B,
            RootSlot::B => RootSlot::A,
        }
    }
    /// Sector index within [`ROOT_BLOCK`].
    fn sector(self) -> usize {
        match self {
            RootSlot::A => 0,
            RootSlot::B => 1,
        }
    }
}

/// The 512 B root record: everything mutable about the volume, reached from
/// one atomically-flipped sector. All other on-disk state (the inode map, the
/// refcount map, the snapshot index and reclaim queue objects, every inode and
/// data block) is found by walking from here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootRecord {
    /// Monotone commit generation (1 = the format commit).
    pub generation: u64,
    /// Block of the inode-map index (one block of leaf-block pointers).
    pub imap_block: u64,
    /// Number of inode-map leaf blocks.
    pub imap_leaves: u64,
    /// Next logical inode id to hand out.
    pub next_inode: u64,
    /// Block of the refcount-map index (one block of leaf-block pointers).
    pub refmap_block: u64,
    /// Number of refcount-map leaf blocks.
    pub refmap_leaves: u64,
    /// Free blocks at commit time (refcount == 0 in the committed map).
    pub free_blocks: u64,
    /// Reserved for future flags (0 today).
    pub flags: u64,
}

impl RootRecord {
    /// Pack the record into an exactly-[`ROOT_RECORD_SIZE`]-byte buffer,
    /// checksummed (FNV-1a over everything before the checksum field).
    pub fn to_bytes(&self) -> [u8; ROOT_RECORD_SIZE] {
        let mut buf = [0u8; ROOT_RECORD_SIZE];
        buf[0..8].copy_from_slice(&ROOT_MAGIC);
        buf[8..16].copy_from_slice(&self.generation.to_le_bytes());
        buf[16..24].copy_from_slice(&self.imap_block.to_le_bytes());
        buf[24..32].copy_from_slice(&self.imap_leaves.to_le_bytes());
        buf[32..40].copy_from_slice(&self.next_inode.to_le_bytes());
        buf[40..48].copy_from_slice(&self.refmap_block.to_le_bytes());
        buf[48..56].copy_from_slice(&self.refmap_leaves.to_le_bytes());
        buf[56..64].copy_from_slice(&self.free_blocks.to_le_bytes());
        buf[64..72].copy_from_slice(&self.flags.to_le_bytes());
        let sum = crate::hash::hash_bytes(&buf[0..72]);
        buf[72..80].copy_from_slice(&sum.to_le_bytes());
        buf
    }

    /// Parse and VALIDATE a root record from a 512 B sector image: magic,
    /// checksum, and non-zero generation must all hold, or the slot is
    /// treated as absent/torn (`None`) — never an error, because one bad slot
    /// is the normal state after a torn root write.
    pub fn from_sector(sector: &[u8]) -> Option<RootRecord> {
        if sector.len() < ROOT_RECORD_SIZE {
            return None;
        }
        if sector[0..8] != ROOT_MAGIC {
            return None;
        }
        let sum_stored = u64::from_le_bytes(sector[72..80].try_into().ok()?);
        if crate::hash::hash_bytes(&sector[0..72]) != sum_stored {
            return None;
        }
        let rd = |o: usize| u64::from_le_bytes(sector[o..o + 8].try_into().unwrap());
        let rec = RootRecord {
            generation: rd(8),
            imap_block: rd(16),
            imap_leaves: rd(24),
            next_inode: rd(32),
            refmap_block: rd(40),
            refmap_leaves: rd(48),
            free_blocks: rd(56),
            flags: rd(64),
        };
        if rec.generation == 0 {
            return None;
        }
        Some(rec)
    }
}

/// Read both root slots and return the newer VALID one (record + its slot).
/// `None` when neither slot holds a valid record (not a K8 volume, or both
/// sectors torn — the latter is impossible under the A/B discipline short of
/// media loss, since only the inactive slot is ever written).
pub fn read_active<D: BlockDevice>(
    device: &mut D,
) -> Result<Option<(RootRecord, RootSlot)>, StorageError> {
    let mut block = alloc::vec![0u8; crate::storage::BLOCK_SIZE as usize];
    device.read_block(ROOT_BLOCK, &mut block)?;
    let a = RootRecord::from_sector(&block[0..ROOT_SECTOR_SIZE]);
    let b = RootRecord::from_sector(&block[ROOT_SECTOR_SIZE..2 * ROOT_SECTOR_SIZE]);
    Ok(match (a, b) {
        (Some(ra), Some(rb)) => {
            if ra.generation >= rb.generation {
                Some((ra, RootSlot::A))
            } else {
                Some((rb, RootSlot::B))
            }
        }
        (Some(ra), None) => Some((ra, RootSlot::A)),
        (None, Some(rb)) => Some((rb, RootSlot::B)),
        (None, None) => None,
    })
}

/// Write `record` into `slot` — ONE 512 B sector write, the commit's atomic
/// point. Routed through [`BlockDevice::write_sector_in_block`], which the
/// kernel's sector adapter implements as a true single-sector device write.
pub fn write_slot<D: BlockDevice>(
    device: &mut D,
    slot: RootSlot,
    record: &RootRecord,
) -> Result<(), StorageError> {
    let mut sector = [0u8; ROOT_SECTOR_SIZE];
    sector[0..ROOT_RECORD_SIZE].copy_from_slice(&record.to_bytes());
    device.write_sector_in_block(ROOT_BLOCK, slot.sector(), &sector)
}
