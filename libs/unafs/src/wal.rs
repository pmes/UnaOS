use serde::{Deserialize, Serialize};
use crate::storage::{BlockDevice, BLOCK_SIZE, Error as StorageError};
use thiserror::Error;

/// The number of blocks reserved for the journal.
/// This matches the value in `Superblock`.
pub const JOURNAL_BLOCKS: u64 = 10;
pub const JOURNAL_START: u64 = 1;

/// Represents an atomic operation in the file system.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub enum JournalOp {
    /// Start of an Inode creation or update.
    BeginOp {
        op_id: u64,
        desc: String,
    },
    /// Successful completion of an operation.
    EndOp {
        op_id: u64,
    },
    // --- Phase 2: Total Transaction Coverage ---
    BeginCreate {
        parent_inode: u64,
        name: String,
    },
    EndCreate {
        inode_id: u64,
    },
    BeginWrite {
        inode_id: u64,
    },
    EndWrite {
        inode_id: u64,
    },
}

#[derive(Error, Debug)]
pub enum JournalError {
    #[error("Storage error: {0}")]
    Storage(#[from] StorageError),
    #[error("Serialization error: {0}")]
    Serialization(#[from] bincode::Error),
    #[error("Journal full")]
    JournalFull,
}

/// The Write-Ahead Log manager.
/// Currently implements a simple append-only log in a ring buffer style (conceptually),
/// but for this iteration, we just append linearly until full/reset on mount.
/// Actually, to be useful, it needs to persist.
///
/// Simplified Logic:
/// We write `JournalOp`s sequentially into the reserved blocks.
/// On mount, we scan these blocks. If we find a `BeginOp` without a matching `EndOp`,
/// we know the FS is dirty.
pub struct Journal {
    /// The current write offset (in bytes) within the journal region.
    write_offset: u64,
}

impl Journal {
    pub fn new() -> Self {
        Self { write_offset: 0 }
    }

    /// Reset the journal (e.g., after clean mount or recovery).
    pub fn reset<D: BlockDevice>(&mut self, device: &mut D) -> Result<(), JournalError> {
        self.write_offset = 0;
        // Zero out the first block to invalidate previous logs
        let zero_block = vec![0u8; BLOCK_SIZE as usize];
        device.write_block(JOURNAL_START, &zero_block)?;
        Ok(())
    }

    /// Append an entry to the journal.
    pub fn append<D: BlockDevice>(&mut self, device: &mut D, op: JournalOp) -> Result<(), JournalError> {
        self.log(device, op)
    }

    /// Recover state from the journal.
    /// Returns true if the FS was dirty (unclosed transaction found).
    pub fn check_recovery<D: BlockDevice>(&mut self, device: &mut D) -> Result<bool, JournalError> {
        let mut offset = 0;
        let mut open_ops = std::collections::HashSet::new(); // Tracks op_ids
        // For Create/Write ops without explicit ID in Begin, we need a way to track pairing.
        // But `BeginCreate` doesn't have an ID yet. `EndCreate` has the new ID.
        // Wait, if we crash during Create, we might have allocated the ID but not finished.
        // Or we might not have allocated it.
        // Simple heuristic: Count Begins vs Ends.
        // Actually, the requirement is just to detect *dirty state*.
        // Any Begin without an End is dirty.
        // But `BeginCreate` doesn't have a unique ID to match with `EndCreate` easily if multiple happen (single threaded assumption?).
        // UnaFS is single threaded context usually.
        // Let's track nesting depth or just strict pairing?
        // Since we are linear, we can just say: "Is the last op a Begin?"
        // Or "Do we have any open transactions?"
        // Let's use a counter for general "open transactions".

        let mut open_transaction_count = 0;

        loop {
             if offset >= JOURNAL_BLOCKS * BLOCK_SIZE {
                 break;
             }

             let block_idx = offset / BLOCK_SIZE;
             let offset_in_block = (offset % BLOCK_SIZE) as usize;
             let physical_block = JOURNAL_START + block_idx;

             // Check if we can read length (8 bytes)
             if offset_in_block + 8 > BLOCK_SIZE as usize {
                 offset = (block_idx + 1) * BLOCK_SIZE;
                 continue;
             }

             let mut block = vec![0u8; BLOCK_SIZE as usize];
             device.read_block(physical_block, &mut block)?;

             let len_bytes: [u8; 8] = block[offset_in_block..offset_in_block+8].try_into().unwrap();
             let len = u64::from_le_bytes(len_bytes);

             if len == 0 {
                 break;
             }

             if offset_in_block + 8 + (len as usize) > BLOCK_SIZE as usize {
                 break;
             }

             let data = &block[offset_in_block+8 .. offset_in_block+8+(len as usize)];
             if let Ok(op) = bincode::deserialize::<JournalOp>(data) {
                 match op {
                     JournalOp::BeginOp { op_id, .. } => {
                         open_ops.insert(op_id);
                     }
                     JournalOp::EndOp { op_id } => {
                         open_ops.remove(&op_id);
                     }
                     JournalOp::BeginCreate { .. } | JournalOp::BeginWrite { .. } => {
                         open_transaction_count += 1;
                     }
                     JournalOp::EndCreate { .. } | JournalOp::EndWrite { .. } => {
                         if open_transaction_count > 0 {
                             open_transaction_count -= 1;
                         }
                     }
                 }
             } else {
                 break;
             }

             offset += 8 + len;
        }

        Ok(!open_ops.is_empty() || open_transaction_count > 0)
    }

    // Helper to write with length prefix (Refined append logic)
    pub fn log<D: BlockDevice>(&mut self, device: &mut D, op: JournalOp) -> Result<(), JournalError> {
        let bytes = bincode::serialize(&op)?;
        let len = bytes.len() as u64;
        let total_len = 8 + len; // 8 bytes for length prefix

         // Check capacity
        if self.write_offset + total_len > (JOURNAL_BLOCKS * BLOCK_SIZE) {
            self.reset(device)?;
        }

        let journal_rel_offset = self.write_offset;
        let block_idx = journal_rel_offset / BLOCK_SIZE;
        let offset_in_block = (journal_rel_offset % BLOCK_SIZE) as usize;
        let physical_block = JOURNAL_START + block_idx;

        // Check if it fits in current block
        if offset_in_block + (total_len as usize) > BLOCK_SIZE as usize {
             // Move to next block start
             self.write_offset = (block_idx + 1) * BLOCK_SIZE;
             return self.log(device, op);
        }

        let mut block = vec![0u8; BLOCK_SIZE as usize];
        device.read_block(physical_block, &mut block)?;

        // Write Length
        let len_bytes = len.to_le_bytes();
        block[offset_in_block..offset_in_block+8].copy_from_slice(&len_bytes);

        // Write Data
        block[offset_in_block+8..offset_in_block+8+bytes.len()].copy_from_slice(&bytes);

        device.write_block(physical_block, &block)?;

        self.write_offset += total_len;
        Ok(())
    }
}
