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

use crate::storage::{BLOCK_SIZE, BlockDevice, Error as StorageError};
use alloc::vec::Vec;

/// A simple bitmap implementation for managing free space.
///
/// Uses a `Vec<u8>` where each bit represents a block.
/// 0 = Free, 1 = Used.
pub struct SpaceMap {
    bits: Vec<u8>,
    block_count: u64,
}

impl SpaceMap {
    /// Create a new SpaceMap with a specific capacity (in bits/blocks).
    pub fn new(block_count: u64) -> Self {
        // Calculate bytes needed: ceil(block_count / 8)
        let byte_count = block_count.div_ceil(8);
        Self {
            bits: vec![0; byte_count as usize],
            block_count,
        }
    }

    /// Load the bitmap from the device.
    ///
    /// Reads from `start_block` for `count` blocks. `count` is disk-derived
    /// (the superblock's `bitmap_blocks`), so the backing allocation is
    /// checked-and-fallible: absurd geometry yields a graceful
    /// [`StorageError::AllocRefused`], never a capacity-overflow panic or an
    /// OOM abort (BEFS-HARDEN, K3-PARSE-1).
    pub fn load<D: BlockDevice>(
        device: &mut D,
        start_block: u64,
        count: u64,
    ) -> Result<Self, StorageError> {
        let byte_len = count
            .checked_mul(BLOCK_SIZE)
            .ok_or(StorageError::AllocRefused(u64::MAX))?;
        let bit_count = byte_len
            .checked_mul(8)
            .ok_or(StorageError::AllocRefused(byte_len))?;
        let byte_len_usize =
            usize::try_from(byte_len).map_err(|_| StorageError::AllocRefused(byte_len))?;

        let mut bits = Vec::new();
        bits.try_reserve_exact(byte_len_usize)
            .map_err(|_| StorageError::AllocRefused(byte_len))?;
        let mut buf = vec![0u8; BLOCK_SIZE as usize];

        for i in 0..count {
            let block = start_block
                .checked_add(i)
                .ok_or(StorageError::OutOfBounds(start_block))?;
            device.read_block(block, &mut buf)?;
            bits.extend_from_slice(&buf);
        }

        // We load full blocks, so we might have more bits than block_count.
        // We can approximate block_count or take it as argument.
        // For now, let's assume it covers the disk size implied by the blocks read.
        Ok(Self {
            bits,
            block_count: bit_count,
        })
    }

    /// Save the bitmap to the device.
    ///
    /// Writes to `start_block`. It will use as many blocks as needed.
    pub fn save<D: BlockDevice>(
        &self,
        device: &mut D,
        start_block: u64,
    ) -> Result<(), StorageError> {
        let mut buf = vec![0u8; BLOCK_SIZE as usize];
        // Split bits into 4096-byte chunks
        let chunks = self.bits.chunks(BLOCK_SIZE as usize);

        for (i, chunk) in chunks.enumerate() {
            // Clear buffer
            buf.fill(0);
            // Copy chunk data
            buf[..chunk.len()].copy_from_slice(chunk);
            device.write_block(start_block + i as u64, &buf)?;
        }
        Ok(())
    }

    /// Allocate a free block.
    /// Returns the block ID if successful, or None if full.
    pub fn allocate(&mut self) -> Option<u64> {
        for (byte_idx, byte) in self.bits.iter_mut().enumerate() {
            if *byte != 0xFF {
                // Found a byte with at least one zero.
                // Iterate bits 0..7
                for bit_idx in 0..8 {
                    let mask = 1 << bit_idx;
                    if *byte & mask == 0 {
                        let block_id = (byte_idx * 8 + bit_idx) as u64;
                        if block_id >= self.block_count {
                            // If we hit the end of the logical disk size
                            return None;
                        }

                        // Mark as used
                        *byte |= mask;
                        return Some(block_id);
                    }
                }
            }
        }
        None
    }

    /// Report whether `block_id` is currently marked used.
    ///
    /// Out-of-range ids (no backing byte) report `false`. Used by the
    /// fsck-scavenger to diff the allocated set against the reachable set.
    pub fn is_used(&self, block_id: u64) -> bool {
        let byte_idx = (block_id / 8) as usize;
        let bit_idx = (block_id % 8) as usize;
        self.bits
            .get(byte_idx)
            .map(|byte| byte & (1 << bit_idx) != 0)
            .unwrap_or(false)
    }

    /// Mark a block as used explicitly (e.g., during format).
    pub fn mark_used(&mut self, block_id: u64) {
        let byte_idx = (block_id / 8) as usize;
        let bit_idx = (block_id % 8) as usize;

        if byte_idx < self.bits.len() {
            self.bits[byte_idx] |= 1 << bit_idx;
        }
    }

    /// Free a block.
    pub fn free(&mut self, block_id: u64) {
        let byte_idx = (block_id / 8) as usize;
        let bit_idx = (block_id % 8) as usize;

        if byte_idx < self.bits.len() {
            self.bits[byte_idx] &= !(1 << bit_idx);
        }
    }
}
