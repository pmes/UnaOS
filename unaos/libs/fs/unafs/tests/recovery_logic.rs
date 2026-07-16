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

//! K8a crash-safety KATs: the copy-on-write commit discipline.
//!
//! Each test models a power cut at a specific point in the commit sequence
//! and asserts the "old tree or new tree, never a hybrid" contract:
//!
//! * **Before the root flip** (`set_autocommit(false)` writes every fresh
//!   block but never flips): the next mount lands on the OLD tree, whole,
//!   refcount-consistent, with none of the transaction visible.
//! * **A torn root-sector write** (the A/B discipline): corrupting the
//!   just-written slot falls back to the other slot's tree.
//! * **The reclaim queue**: enqueue is durable; a mount drains a pending
//!   queue to empty (the eager v1 policy) and reclaims the blocks.
//! * **fsck**: detects and repairs refcount drift injected as raw media
//!   corruption (the one residue class CoW itself cannot produce).

use unafs::root::{ROOT_BLOCK, ROOT_SECTOR_SIZE};
use unafs::{BLOCK_SIZE, BlockDevice, MemDevice, ReclaimEntry, RootRecord, UnaFS};

/// Format a fresh in-memory volume of `block_count` blocks.
fn fresh_fs(block_count: u64) -> UnaFS<MemDevice> {
    let mut device = MemDevice::new();
    let empty_block = vec![0u8; BLOCK_SIZE as usize];
    device
        .write_block(block_count - 1, &empty_block)
        .expect("Failed to set disk size");
    UnaFS::format(device, 20).expect("Format failed")
}

// --- R1: power cut BEFORE the root flip → the old tree, whole ---------------

#[test]
fn uncommitted_transaction_converges_to_the_old_tree() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;

    // Committed baseline: one file with known bytes.
    let keep = fs.create_file(root, "keep.txt".into()).unwrap();
    fs.write_data(keep, 0, b"the committed tree").unwrap();
    let committed_gen = fs.root_generation();
    let committed_free = fs.free_blocks();

    // The crash-simulation seam: fresh blocks land, the root NEVER flips.
    fs.set_autocommit(false);
    let doomed = fs.create_file(root, "doomed.txt".into()).unwrap();
    fs.write_data(doomed, 0, &vec![0xEE; 3 * BLOCK_SIZE as usize])
        .unwrap();
    fs.write_data(keep, 0, b"OVERWRITTEN IN THE DOOMED TXN").unwrap();

    // Power cut: drop the instance mid-transaction, remount from raw bytes.
    let device = fs.device.clone();
    drop(fs);
    let mut fs2 = UnaFS::mount(device).expect("mount after simulated power cut");

    // The OLD tree, exactly: same generation, the new file absent, the
    // keeper's ORIGINAL bytes intact (CoW never touched its old blocks).
    assert_eq!(fs2.root_generation(), committed_gen);
    assert!(fs2.resolve_path("/doomed.txt").is_err());
    let inode = fs2.read_inode(keep).unwrap();
    assert_eq!(
        fs2.read_data(keep, 0, inode.size).unwrap(),
        b"the committed tree"
    );

    // Nothing leaked: the aborted transaction's blocks were never committed
    // as allocated, so free space and refcount consistency both hold.
    assert_eq!(fs2.free_blocks(), committed_free);
    let report = fs2.fsck(false).unwrap();
    assert!(report.is_clean(), "old tree must be clean: {report:?}");

    // And the volume is fully writable again after the "crash".
    let reborn = fs2.create_file(root, "doomed.txt".into()).unwrap();
    fs2.write_data(reborn, 0, b"second life").unwrap();
    assert_eq!(fs2.resolve_path("/doomed.txt").unwrap(), reborn);
}

// --- R2: the A/B root slots --------------------------------------------------

#[test]
fn commits_alternate_slots_and_generations_are_monotone() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;

    let g0 = fs.root_generation();
    fs.create_file(root, "a.txt".into()).unwrap();
    let g1 = fs.root_generation();
    fs.create_file(root, "b.txt".into()).unwrap();
    let g2 = fs.root_generation();
    assert!(g0 < g1 && g1 < g2, "generations must be monotone");

    // Both slots hold VALID records one generation apart (A/B alternation).
    let mut block = vec![0u8; BLOCK_SIZE as usize];
    fs.device.read_block(ROOT_BLOCK, &mut block).unwrap();
    let a = RootRecord::from_sector(&block[0..ROOT_SECTOR_SIZE]).expect("slot A valid");
    let b = RootRecord::from_sector(&block[ROOT_SECTOR_SIZE..2 * ROOT_SECTOR_SIZE])
        .expect("slot B valid");
    let (newer, older) = if a.generation > b.generation { (a, b) } else { (b, a) };
    assert_eq!(newer.generation, g2);
    assert_eq!(older.generation, g2 - 1);
}

#[test]
fn torn_root_sector_falls_back_to_the_previous_tree() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;

    fs.create_file(root, "old.txt".into()).unwrap();
    let old_gen = fs.root_generation();
    fs.create_file(root, "new.txt".into()).unwrap();
    let new_gen = fs.root_generation();

    // Tear the NEWEST root sector (the slot the last commit wrote) — the
    // exact failure a power cut inside the 512 B root write produces.
    let mut device = fs.device.clone();
    drop(fs);
    let mut block = vec![0u8; BLOCK_SIZE as usize];
    device.read_block(ROOT_BLOCK, &mut block).unwrap();
    let a = RootRecord::from_sector(&block[0..ROOT_SECTOR_SIZE]);
    let b = RootRecord::from_sector(&block[ROOT_SECTOR_SIZE..2 * ROOT_SECTOR_SIZE]);
    let newest_is_a = match (a, b) {
        (Some(ra), Some(rb)) => ra.generation > rb.generation,
        _ => panic!("both slots must be valid after two commits"),
    };
    let torn_range = if newest_is_a {
        0..ROOT_SECTOR_SIZE
    } else {
        ROOT_SECTOR_SIZE..2 * ROOT_SECTOR_SIZE
    };
    for byte in &mut block[torn_range] {
        *byte ^= 0xA5; // garbage — checksum cannot survive
    }
    device.write_block(ROOT_BLOCK, &block).unwrap();

    // Mount falls back to the OTHER slot: the previous committed tree.
    let mut fs2 = UnaFS::mount(device).expect("mount must survive a torn root slot");
    assert_eq!(fs2.root_generation(), old_gen);
    assert!(new_gen > old_gen);
    assert!(fs2.resolve_path("/old.txt").is_ok());
    assert!(fs2.resolve_path("/new.txt").is_err(), "the torn commit is gone");
    assert!(fs2.fsck(false).unwrap().is_clean());
}

#[test]
fn volume_with_no_valid_root_refuses_to_mount() {
    let fs = fresh_fs(5000);
    let mut device = fs.device.clone();
    drop(fs);
    let zero = vec![0u8; BLOCK_SIZE as usize];
    device.write_block(ROOT_BLOCK, &zero).unwrap();
    assert!(UnaFS::mount(device).is_err());
}

// --- R3: the persistent reclaim queue ----------------------------------------

#[test]
fn reclaim_queue_enqueue_is_durable_and_mount_drains_it() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;

    // Model a dropped root's residue the way K8b's drop path will: blocks
    // whose only remaining holder is the queue entry. Here: a file's blocks,
    // unlinked (tree reference gone) — the queue entry then carries them.
    // Drain's decref saturates at 0, so the mechanism under test is the
    // DURABILITY of the enqueue and the DRAIN-TO-EMPTY mount policy.
    let f = fs.create_file(root, "snapdata.bin".into()).unwrap();
    fs.write_data(f, 0, &vec![0x5A; 2 * BLOCK_SIZE as usize])
        .unwrap();
    let inode = fs.read_inode(f).unwrap();
    let mut blocks: Vec<u64> = Vec::new();
    for e in &inode.chunks {
        for i in 0..e.length.div_ceil(BLOCK_SIZE) {
            blocks.push(e.physical_block + i);
        }
    }
    assert_eq!(blocks.len(), 2);
    fs.unlink(root, "snapdata.bin").unwrap();

    let generation = fs.root_generation();
    fs.reclaim_enqueue(ReclaimEntry { generation, blocks }).unwrap();

    // The enqueue is durable — visible through a fresh read of the object —
    // and a remount finds the pending queue and DRAINS it (eager v1).
    assert_eq!(fs.reclaim_queue().unwrap().len(), 1);
    let free_before_drain = fs.free_blocks();
    let device = fs.device.clone();
    drop(fs);
    let mut fs2 = UnaFS::mount(device).expect("mount with pending queue");
    assert!(
        fs2.reclaim_queue().unwrap().is_empty(),
        "mount must drain the reclaim queue to empty (eager v1)"
    );
    // Draining freed the queue object's old data blocks and nothing dangles.
    assert!(fs2.free_blocks() >= free_before_drain);
    assert!(fs2.fsck(false).unwrap().is_clean());

    // Idempotent: a second remount finds nothing to drain.
    let device2 = fs2.device.clone();
    drop(fs2);
    let mut fs3 = UnaFS::mount(device2).unwrap();
    assert!(fs3.reclaim_queue().unwrap().is_empty());
    assert!(fs3.fsck(false).unwrap().is_clean());
}

#[test]
fn reclaim_queue_is_empty_on_a_fresh_volume() {
    let mut fs = fresh_fs(5000);
    assert!(fs.reclaim_queue().unwrap().is_empty());
    assert!(fs.snapshot_index().unwrap().is_empty());
}

// --- R4: fsck against injected refcount drift ---------------------------------

#[test]
fn fsck_detects_and_repairs_injected_refcount_drift() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;
    let f = fs.create_file(root, "real.txt".into()).unwrap();
    fs.write_data(f, 0, b"real data").unwrap();

    // Clean baseline.
    assert!(fs.fsck(false).unwrap().is_clean());
    let free_before = fs.free_blocks();

    // Inject drift the way media corruption would: mark a genuinely FREE
    // block as allocated in the persisted refcount map, then remount. Find
    // the persisted map through the on-disk root record (public parser).
    let mut device = fs.device.clone();
    drop(fs);
    let (rr, _) = unafs::root::read_active(&mut device).unwrap().unwrap();
    let mut index = vec![0u8; BLOCK_SIZE as usize];
    device.read_block(rr.refmap_block, &mut index).unwrap();
    let leaf0 = u64::from_le_bytes(index[0..8].try_into().unwrap());
    let mut leaf = vec![0u8; BLOCK_SIZE as usize];
    device.read_block(leaf0, &mut leaf).unwrap();
    // Find a zero count in the first leaf and corrupt it to 1.
    let mut victim = None;
    for i in 0..(BLOCK_SIZE as usize / 4) {
        let c = u32::from_le_bytes(leaf[i * 4..i * 4 + 4].try_into().unwrap());
        if c == 0 {
            leaf[i * 4..i * 4 + 4].copy_from_slice(&1u32.to_le_bytes());
            victim = Some(i as u64);
            break;
        }
    }
    let victim = victim.expect("a free block must exist");
    device.write_block(leaf0, &leaf).unwrap();

    let mut fs2 = UnaFS::mount(device).expect("mount");
    assert_eq!(fs2.free_blocks(), free_before - 1, "drift visible after mount");

    // Dry run sees exactly the injected leak and touches nothing.
    let dry = fs2.fsck(false).unwrap();
    assert_eq!(dry.leaked_blocks, vec![victim]);
    assert!(!dry.is_clean());
    assert_eq!(fs2.free_blocks(), free_before - 1);

    // Repair reclaims it and the repaired state is durable.
    let repaired = fs2.fsck(true).unwrap();
    assert_eq!(repaired.reclaimed_blocks, 1);
    assert_eq!(fs2.free_blocks(), free_before);
    let device2 = fs2.device.clone();
    drop(fs2);
    let mut fs3 = UnaFS::mount(device2).unwrap();
    assert!(fs3.fsck(false).unwrap().is_clean());
    // Live data untouched by the repair.
    let inode = fs3.read_inode(f).unwrap();
    assert_eq!(fs3.read_data(f, 0, inode.size).unwrap(), b"real data");
}

// --- R5: CoW never overwrites a committed block --------------------------------

#[test]
fn committed_blocks_are_never_overwritten_within_the_next_transaction() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;

    let f = fs.create_file(root, "gold.txt".into()).unwrap();
    fs.write_data(f, 0, b"generation one bytes").unwrap();

    // Snapshot the file's committed physical block and its raw contents.
    let inode = fs.read_inode(f).unwrap();
    let old_block = inode.chunks[0].physical_block;
    let mut before = vec![0u8; BLOCK_SIZE as usize];
    fs.device.read_block(old_block, &mut before).unwrap();

    // Overwrite the file. CoW: the data must land on a DIFFERENT block and
    // the old block's raw bytes must be untouched by this transaction.
    fs.write_data(f, 0, b"generation two bytes").unwrap();
    let inode2 = fs.read_inode(f).unwrap();
    let new_block = inode2.chunks[0].physical_block;
    assert_ne!(new_block, old_block, "CoW must allocate a fresh block");

    let mut after = vec![0u8; BLOCK_SIZE as usize];
    fs.device.read_block(old_block, &mut after).unwrap();
    assert_eq!(before, after, "the committed block must not be overwritten");

    // The inode itself also moved (its block is CoW'd too).
    // (Reading through the new tree returns the new bytes, of course.)
    assert_eq!(
        fs.read_data(f, 0, inode2.size).unwrap(),
        b"generation two bytes"
    );

    // AFTER the commit retired the old tree, the old block is reusable —
    // free space reflects the release.
    assert!(fs.fsck(false).unwrap().is_clean());
}

// --- R6: commit stats exist (the bench instrumentation) ------------------------

#[test]
fn commit_stats_count_commits_and_blocks() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;
    let s0 = fs.commit_stats();
    assert!(s0.commits >= 1, "format itself commits");

    let f = fs.create_file(root, "bench.txt".into()).unwrap();
    fs.write_data(f, 0, &vec![0x11; 2 * BLOCK_SIZE as usize]).unwrap();

    let s1 = fs.commit_stats();
    assert!(s1.commits >= s0.commits + 2, "each op commits");
    assert!(s1.blocks_written > s0.blocks_written);
    assert!(s1.last_commit_blocks > 0);
}
