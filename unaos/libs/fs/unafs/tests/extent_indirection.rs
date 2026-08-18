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

//! Extent-list indirection (unafs-extent arc).
//!
//! Before this arc a file's extent list had to serialize inside its ONE 4096 B
//! inode block, so a fragmented file hit an `InodeTooLarge` wall at ~80 MiB
//! (~168 extents). These tests exercise the fix: an overflowing extent list
//! spills to INDIRECT blocks, the inline fast path stays byte-identical, fsck
//! validates the indirect blocks, and the whole thing survives a remount.
//!
//! ## Forcing fragmentation cheaply
//!
//! A contiguous write coalesces into one extent, so it never spills. Writing
//! single blocks at NON-ADJACENT LOGICAL offsets (every other block, leaving
//! holes) does not: extents coalesce only when logically AND physically
//! adjacent, so a hole between each written block guarantees one extent per
//! block — N writes ⇒ N extents — independent of where the allocator places
//! them, and the holes cost no physical blocks.

use unafs::inode::{INODE_SPILL_MAGIC, Inode};
use unafs::{BLOCK_SIZE, BlockDevice, MemDevice, UnaFS};

const BS: usize = BLOCK_SIZE as usize;

/// Format a fresh in-memory volume of `block_count` blocks.
fn fresh_fs(block_count: u64) -> UnaFS<MemDevice> {
    let mut device = MemDevice::new();
    let empty = vec![0u8; BS];
    device
        .write_block(block_count - 1, &empty)
        .expect("size the device");
    UnaFS::format(device, 0).expect("format")
}

/// Deterministic distinct content for block `j` (so a round-trip is meaningful).
fn block_bytes(j: usize) -> Vec<u8> {
    let mut b = vec![0u8; BS];
    for (k, byte) in b.iter_mut().enumerate() {
        *byte = ((j.wrapping_mul(31).wrapping_add(k)) & 0xFF) as u8;
    }
    b
}

/// The logical byte length of an `n`-block hole-fragmented file: blocks live at
/// even logical positions 0, 2, 4, …, 2(n-1), so the last byte is at 2(n-1)+1.
fn sparse_len(n: usize) -> usize {
    (2 * n - 1) * BS
}

/// Write `n` distinct single blocks at non-adjacent logical offsets (a hole
/// between each), forcing exactly `n` extents. Returns (fs, root, inode_id, the
/// full expected byte stream — written blocks at even positions, zero holes at
/// odd ones).
fn make_fragmented_file(blocks: u64, n: usize) -> (UnaFS<MemDevice>, u64, u64, Vec<u8>) {
    let mut fs = fresh_fs(blocks);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "big.bin".into()).expect("create");
    let mut expected = vec![0u8; sparse_len(n)];
    for j in 0..n {
        let blk = block_bytes(j);
        let off = 2 * j * BS;
        fs.write_data(id, off as u64, &blk).expect("write block");
        expected[off..off + BS].copy_from_slice(&blk);
    }
    (fs, root, id, expected)
}

/// The raw 4096 B on-disk image of an inode's CURRENT block.
fn raw_inode_block(fs: &mut UnaFS<MemDevice>, id: u64) -> Vec<u8> {
    let pb = fs.inode_block(id).expect("inode allocated");
    let mut block = vec![0u8; BS];
    fs.device.read_block(pb, &mut block).expect("read inode block");
    block
}

// ---------------------------------------------------------------------------
// 1. The inline fast path is byte-identical to the pre-indirection format.
// ---------------------------------------------------------------------------
#[test]
fn inline_small_file_inode_is_byte_identical() {
    let mut fs = fresh_fs(4096);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "small.txt".into()).unwrap();
    fs.write_data(id, 0, b"the crystal remembers").unwrap();

    let inode = fs.read_inode(id).unwrap();
    // A small file fits inline: no spill, one contiguous extent.
    assert!(inode.chunks.len() <= 1, "small file should not fragment");

    // The on-disk block is EXACTLY the plain inline serialization followed by
    // zero padding — no trailer, byte-for-byte the old format.
    let block = raw_inode_block(&mut fs, id);
    let inline = inode.to_bytes().expect("small inode fits inline");
    assert_eq!(&block[..inline.len()], &inline[..], "inline prefix unchanged");
    assert!(
        block[inline.len()..].iter().all(|&b| b == 0),
        "tail is zero padding, not a trailer"
    );

    // And decode_block agrees there is no indirect trailer.
    let (_ino, trailer) = Inode::decode_block(&block).unwrap();
    assert!(trailer.is_none(), "inline inode carries no trailer");
}

// ---------------------------------------------------------------------------
// 2. A file too fragmented to fit inline spills, and round-trips byte-identical.
// ---------------------------------------------------------------------------
#[test]
fn fragmented_file_spills_and_roundtrips() {
    // 250 one-block extents is well past the ~168-extent inline wall.
    let (mut fs, _root, id, expected) = make_fragmented_file(20_000, 250);

    let inode = fs.read_inode(id).unwrap();
    assert_eq!(inode.chunks.len(), 250, "one extent per fragmented block");

    // Proof the file genuinely NEEDS indirection: its full extent list does not
    // fit one block (the old code would have failed here with InodeTooLarge).
    assert!(
        inode.to_bytes().is_err(),
        "full inline inode must overflow the block"
    );

    // Proof the block actually spilled: a magic-tagged trailer follows the inode.
    let block = raw_inode_block(&mut fs, id);
    let (_ino, trailer) = Inode::decode_block(&block).unwrap();
    let trailer = trailer.expect("spilled inode carries a trailer");
    assert_eq!(trailer.magic, INODE_SPILL_MAGIC);
    assert_eq!(trailer.total_extents, 250);
    assert!(!trailer.index.is_empty(), "overflow lives in indirect blocks");

    // The bytes come back identical.
    let got = fs.read_data(id, 0, expected.len() as u64).unwrap();
    assert_eq!(got, expected, "spilled file round-trips byte-identical");

    // fsck is clean — the indirect blocks are reachable, nothing leaked.
    let report = fs.fsck(false).unwrap();
    assert!(report.is_clean(), "fsck clean on spilled file: {report:?}");
    assert!(report.leaked_blocks.is_empty());
}

// ---------------------------------------------------------------------------
// 3. fsck --repair is a no-op on a healthy spilled file (never eats live data).
// ---------------------------------------------------------------------------
#[test]
fn fsck_repair_is_a_noop_on_a_healthy_spilled_file() {
    let (mut fs, _root, id, expected) = make_fragmented_file(20_000, 220);

    let before = fs.fsck(false).unwrap();
    assert!(before.is_clean());

    let repaired = fs.fsck(true).unwrap();
    assert!(repaired.is_clean());
    assert_eq!(repaired.reclaimed_blocks, 0, "repair frees nothing on a clean fs");
    assert_eq!(repaired.leaked_blocks.len(), 0);

    // Data intact after a repair pass.
    let got = fs.read_data(id, 0, expected.len() as u64).unwrap();
    assert_eq!(got, expected);
}

// ---------------------------------------------------------------------------
// 4. A spilled file survives a remount (the split is truly on disk).
// ---------------------------------------------------------------------------
#[test]
fn spilled_file_survives_remount() {
    let (fs, _root, id, expected) = make_fragmented_file(20_000, 240);
    let device = fs.device.clone(); // an independent copy of the committed volume

    let mut fs2 = UnaFS::mount(device).expect("remount");
    let inode = fs2.read_inode(id).unwrap();
    assert_eq!(inode.chunks.len(), 240);
    let got = fs2.read_data(id, 0, expected.len() as u64).unwrap();
    assert_eq!(got, expected, "remounted spilled file is intact");
    assert!(fs2.fsck(false).unwrap().is_clean());
}

// ---------------------------------------------------------------------------
// 5. Unlinking a spilled file leaks no indirect blocks.
// ---------------------------------------------------------------------------
#[test]
fn unlink_frees_indirect_blocks() {
    let (mut fs, root, _id, _expected) = make_fragmented_file(20_000, 230);

    // Baseline block usage with the spilled file present.
    let with_file = fs.fsck(false).unwrap();
    assert!(with_file.is_clean());

    fs.unlink(root, "big.bin").expect("unlink");

    // No leak: every indirect (and data) block the file owned is now free, and
    // fsck sees a consistent volume.
    let after = fs.fsck(false).unwrap();
    assert!(after.is_clean(), "no leak after unlink: {after:?}");
    assert!(
        after.blocks_in_use < with_file.blocks_in_use,
        "unlink released blocks"
    );
    // A repair pass confirms it too (rebuilds the map to the same truth).
    let repaired = fs.fsck(true).unwrap();
    assert_eq!(repaired.reclaimed_blocks, 0);
    assert!(repaired.leaked_blocks.is_empty());
}

// ---------------------------------------------------------------------------
// 6. Ceiling measurement: how many extents indirection admits in practice.
//    Thousands of extents round-trip cleanly, far past the ~168-extent inline
//    wall — the binding limit becomes the 2 GiB volume cap, not the inode.
// ---------------------------------------------------------------------------
#[test]
fn measure_extent_ceiling() {
    const N: usize = 4000; // ~24× the inline extent wall
    let mut fs = fresh_fs(2 * N as u64 + 4000);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "huge.bin".into()).unwrap();

    // One transaction for speed; hole-fragmentation keeps every block a
    // separate extent regardless of allocator placement.
    fs.set_autocommit(false);
    let mut expected = vec![0u8; sparse_len(N)];
    for j in 0..N {
        let blk = block_bytes(j);
        let off = 2 * j * BS;
        fs.write_data(id, off as u64, &blk).unwrap();
        expected[off..off + BS].copy_from_slice(&blk);
    }
    fs.commit().unwrap();

    let inode = fs.read_inode(id).unwrap();
    assert_eq!(inode.chunks.len(), N, "all {N} extents reconstructed");
    assert!(
        inode.to_bytes().is_err(),
        "{N} extents overflow the inline block many times over"
    );

    let got = fs.read_data(id, 0, expected.len() as u64).unwrap();
    assert_eq!(got, expected, "{N}-extent file round-trips byte-identical");
    assert!(fs.fsck(false).unwrap().is_clean());
}
