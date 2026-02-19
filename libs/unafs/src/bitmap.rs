use crate::storage::{BlockDevice, BLOCK_SIZE, Error as StorageError};

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
    /// Reads from `start_block` for `count` blocks.
    pub fn load<D: BlockDevice>(device: &mut D, start_block: u64, count: u64) -> Result<Self, StorageError> {
        let mut bits = Vec::with_capacity((count * BLOCK_SIZE) as usize);
        let mut buf = vec![0u8; BLOCK_SIZE as usize];

        for i in 0..count {
            device.read_block(start_block + i, &mut buf)?;
            bits.extend_from_slice(&buf);
        }

        // We load full blocks, so we might have more bits than block_count.
        // We can approximate block_count or take it as argument.
        // For now, let's assume it covers the disk size implied by the blocks read.
        Ok(Self {
            bits,
            block_count: count * BLOCK_SIZE * 8,
        })
    }

    /// Save the bitmap to the device.
    ///
    /// Writes to `start_block`. It will use as many blocks as needed.
    pub fn save<D: BlockDevice>(&self, device: &mut D, start_block: u64) -> Result<(), StorageError> {
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
