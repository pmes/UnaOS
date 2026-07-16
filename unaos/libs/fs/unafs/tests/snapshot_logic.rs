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

//! K8b: retained roots (snapshots) + reclamation KATs.
//!
//! The host twins of the kernel `K8b-snap` witness. The spine leg proves the
//! whole promise end to end: snapshot a tree, mutate the LIVE tree, read the
//! SNAPSHOT back and get the OLD bytes (block sharing + never-overwrite), drop
//! the snapshot, watch reclamation free exactly the blocks no live/retained
//! root still reaches, and re-allocate them. Around it sit the security-core
//! invariants: the retention-aware allocator (a block a snapshot lives on can
//! never be reallocated while the snapshot lives), the crash seams (the
//! index+enqueue is atomic; a power cut mid-drain resumes on the next mount),
//! the cap-16 refusal, and the owner-or-kernel drop authority.

use unafs::{BLOCK_SIZE, BlockDevice, MemDevice, SNAPSHOT_CAP, UnaFS};

/// Format a fresh in-memory volume of `block_count` blocks.
fn fresh_fs(block_count: u64) -> UnaFS<MemDevice> {
    let mut device = MemDevice::new();
    let empty_block = vec![0u8; BLOCK_SIZE as usize];
    device
        .write_block(block_count - 1, &empty_block)
        .expect("Failed to set disk size");
    UnaFS::format(device, 20).expect("Format failed")
}

const KERNEL: &str = "kernel";

// =============================================================================
// The spine: create → mutate → snapshot reads OLD bytes → drop → reclaim → reuse
// =============================================================================

#[test]
fn snapshot_retains_old_bytes_while_live_tree_moves_on() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;

    // A committed baseline: one file with known OLD bytes.
    let f = fs.create_file(root, "doc.txt".into()).unwrap();
    let old = vec![0xA1u8; 3 * BLOCK_SIZE as usize];
    fs.write_data(f, 0, &old).unwrap();

    // Retain it.
    let snap_gen = fs
        .snapshot_create("before-edit".into(), "alice".into(), 1000)
        .unwrap();
    assert_eq!(fs.snapshot_index().unwrap().len(), 1);
    assert_eq!(fs.commit_stats().snapshots_created, 1);

    // Mutate the LIVE tree: overwrite every block with NEW bytes.
    let new = vec![0xB2u8; 3 * BLOCK_SIZE as usize];
    fs.write_data(f, 0, &new).unwrap();

    // The live tree reads the NEW bytes.
    assert_eq!(fs.read_data(f, 0, new.len() as u64).unwrap(), new);

    // The refcount map is internally consistent with two roots in play.
    assert!(fs.fsck(false).unwrap().is_clean());

    // The snapshot still points at the OLD tree: read its inode map straight
    // and byte-verify the retained content is UNCHANGED.
    let snap = fs
        .snapshot_index()
        .unwrap()
        .into_iter()
        .find(|e| e.generation == snap_gen)
        .unwrap();
    assert_eq!(
        read_via_snapshot(&mut fs, &snap, "doc.txt"),
        old,
        "the snapshot must still read the OLD bytes after the live overwrite"
    );

    // Repair mode must reproduce the SAME map (multi-root counts): a repair on
    // a clean two-root volume changes nothing structural and stays clean.
    assert!(fs.fsck(true).unwrap().is_clean());
    assert!(fs.fsck(false).unwrap().is_clean());
    assert_eq!(
        read_via_snapshot(&mut fs, &snap, "doc.txt"),
        old,
        "an fsck repair must not disturb the retained snapshot's blocks"
    );
}

#[test]
fn drop_reclaims_only_blocks_no_root_reaches_then_reuses_them() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;

    // Baseline file, snapshot, then a full overwrite (the snapshot's data
    // blocks are now referenced by the snapshot ALONE — the live tree moved to
    // fresh blocks).
    let f = fs.create_file(root, "doc.txt".into()).unwrap();
    fs.write_data(f, 0, &vec![0xA1u8; 3 * BLOCK_SIZE as usize])
        .unwrap();
    let snap_gen = fs
        .snapshot_create("snap".into(), "alice".into(), 1)
        .unwrap();
    fs.write_data(f, 0, &vec![0xB2u8; 3 * BLOCK_SIZE as usize])
        .unwrap();

    let free_with_snapshot = fs.free_blocks();

    // Drop it: reclamation drains eagerly and frees the snapshot-only blocks.
    fs.snapshot_drop(snap_gen).unwrap();
    assert!(fs.snapshot_index().unwrap().is_empty());
    assert!(fs.reclaim_queue().unwrap().is_empty(), "eager drain to empty");
    assert_eq!(fs.commit_stats().snapshots_dropped, 1);
    assert!(fs.fsck(false).unwrap().is_clean());

    // Dropping FREED blocks (the snapshot-only ones): free count rose.
    let free_after_drop = fs.free_blocks();
    assert!(
        free_after_drop > free_with_snapshot,
        "drop must return the snapshot-only blocks to the free pool ({} -> {})",
        free_with_snapshot,
        free_after_drop
    );

    // The live file is UNHARMED — the drop only touched blocks the live root
    // no longer reached.
    assert_eq!(
        fs.read_data(f, 0, 3 * BLOCK_SIZE).unwrap(),
        vec![0xB2u8; 3 * BLOCK_SIZE as usize]
    );

    // The freed blocks are genuinely re-allocatable: fill more data and stay
    // consistent across a remount.
    let g = fs.create_file(root, "more.bin".into()).unwrap();
    fs.write_data(g, 0, &vec![0xCCu8; 4 * BLOCK_SIZE as usize])
        .unwrap();
    let device = fs.device.clone();
    drop(fs);
    let mut fs2 = UnaFS::mount(device).unwrap();
    assert!(fs2.fsck(false).unwrap().is_clean());
    let more_id = fs2.resolve_path("/more.bin").unwrap();
    assert_eq!(
        fs2.read_data(more_id, 0, 4 * BLOCK_SIZE).unwrap(),
        vec![0xCCu8; 4 * BLOCK_SIZE as usize]
    );
}

// =============================================================================
// Security core: a snapshot's blocks are UNREALLOCATABLE while it lives
// =============================================================================

#[test]
fn snapshot_blocks_are_never_reallocated_while_it_lives() {
    let mut fs = fresh_fs(64); // TIGHT volume: force allocation pressure.
    let root = fs.superblock.root_inode;

    let f = fs.create_file(root, "keep.txt".into()).unwrap();
    let payload = vec![0x11u8; 4 * BLOCK_SIZE as usize];
    fs.write_data(f, 0, &payload).unwrap();

    // The physical blocks the file's data lives on, right now.
    let snap_data_blocks = data_blocks(&mut fs, f);
    assert_eq!(snap_data_blocks.len(), 4);

    // Retain, then churn the live tree hard: repeatedly rewrite/unlink/create
    // to drive many fresh allocations. NONE may land on the retained blocks.
    let snap_gen = fs.snapshot_create("s".into(), "alice".into(), 1).unwrap();
    for i in 0..8 {
        let name = format!("churn{i}.bin");
        let c = fs.create_file(root, name.clone()).unwrap();
        fs.write_data(c, 0, &vec![i as u8; 2 * BLOCK_SIZE as usize])
            .unwrap();
        // Every freshly allocated block must avoid the snapshot's blocks.
        for b in data_blocks(&mut fs, c) {
            assert!(
                !snap_data_blocks.contains(&b),
                "allocator reused a block a live snapshot still holds (block {b})"
            );
        }
        fs.unlink(root, &name).unwrap();
    }

    // The snapshot still reads its original bytes byte-for-byte.
    let snap = find_snap(&mut fs, snap_gen);
    assert_eq!(read_via_snapshot(&mut fs, &snap, "keep.txt"), payload);
    assert!(fs.fsck(false).unwrap().is_clean());
}

#[test]
fn two_snapshots_share_blocks_and_drop_of_one_keeps_the_other() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;

    let f = fs.create_file(root, "doc.txt".into()).unwrap();
    let bytes = vec![0x33u8; 2 * BLOCK_SIZE as usize];
    fs.write_data(f, 0, &bytes).unwrap();

    // Two snapshots of the SAME committed tree share every data block (refcount
    // 3: live + snap_a + snap_b).
    let gen_a = fs.snapshot_create("a".into(), "alice".into(), 1).unwrap();
    let gen_b = fs.snapshot_create("b".into(), "bob".into(), 2).unwrap();
    assert_eq!(fs.snapshot_index().unwrap().len(), 2);
    assert!(fs.fsck(false).unwrap().is_clean());

    // Drop A: the shared blocks survive (B and the live tree still reach them).
    fs.snapshot_drop(gen_a).unwrap();
    assert!(fs.fsck(false).unwrap().is_clean());
    let snap_b = find_snap(&mut fs, gen_b);
    assert_eq!(
        read_via_snapshot(&mut fs, &snap_b, "doc.txt"),
        bytes,
        "dropping snapshot A must not disturb the blocks snapshot B shares"
    );
    assert_eq!(fs.read_data(f, 0, bytes.len() as u64).unwrap(), bytes);

    // Drop B too: now nothing but the live tree remains; still consistent.
    fs.snapshot_drop(gen_b).unwrap();
    assert!(fs.snapshot_index().unwrap().is_empty());
    assert!(fs.fsck(false).unwrap().is_clean());
    assert_eq!(fs.read_data(f, 0, bytes.len() as u64).unwrap(), bytes);
}

// =============================================================================
// Crash seams
// =============================================================================

#[test]
fn snapshot_create_survives_a_remount() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;
    let f = fs.create_file(root, "doc.txt".into()).unwrap();
    let old = vec![0x7Eu8; 2 * BLOCK_SIZE as usize];
    fs.write_data(f, 0, &old).unwrap();
    let snap_gen = fs.snapshot_create("s".into(), "alice".into(), 9).unwrap();
    fs.write_data(f, 0, &vec![0x00u8; 2 * BLOCK_SIZE as usize])
        .unwrap();

    // Remount: the snapshot index and its retained blocks are durable.
    let device = fs.device.clone();
    drop(fs);
    let mut fs2 = UnaFS::mount(device).unwrap();
    assert_eq!(fs2.snapshot_index().unwrap().len(), 1);
    assert!(fs2.fsck(false).unwrap().is_clean());
    let snap = find_snap(&mut fs2, snap_gen);
    assert_eq!(read_via_snapshot(&mut fs2, &snap, "doc.txt"), old);
}

#[test]
fn power_cut_mid_drain_resumes_on_the_next_mount() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;
    let f = fs.create_file(root, "doc.txt".into()).unwrap();
    fs.write_data(f, 0, &vec![0xA1u8; 3 * BLOCK_SIZE as usize])
        .unwrap();
    let snap_gen = fs.snapshot_create("s".into(), "alice".into(), 1).unwrap();
    fs.write_data(f, 0, &vec![0xB2u8; 3 * BLOCK_SIZE as usize])
        .unwrap();

    // Enqueue the drop but DO NOT drain — the exact power-cut-mid-drop state.
    fs.snapshot_drop_enqueue(snap_gen).unwrap();
    assert!(fs.snapshot_index().unwrap().is_empty());
    assert_eq!(fs.reclaim_queue().unwrap().len(), 1, "entry sits on the queue");

    // Power cut: drop the mount before the drain. The next mount finds the
    // pending queue and drains it eagerly, converging to the freed state.
    let device = fs.device.clone();
    drop(fs);
    let mut fs2 = UnaFS::mount(device).unwrap();
    assert!(
        fs2.reclaim_queue().unwrap().is_empty(),
        "the mount's eager drain must resume and empty the queue"
    );
    assert!(fs2.snapshot_index().unwrap().is_empty());
    assert!(fs2.fsck(false).unwrap().is_clean());
    // The live file is intact; its blocks were never on the drained queue.
    let doc_id = fs2.resolve_path("/doc.txt").unwrap();
    assert_eq!(
        fs2.read_data(doc_id, 0, 3 * BLOCK_SIZE).unwrap(),
        vec![0xB2u8; 3 * BLOCK_SIZE as usize]
    );

    // Idempotent: a second remount finds nothing to drain.
    let device2 = fs2.device.clone();
    drop(fs2);
    let mut fs3 = UnaFS::mount(device2).unwrap();
    assert!(fs3.reclaim_queue().unwrap().is_empty());
    assert!(fs3.fsck(false).unwrap().is_clean());
}

// =============================================================================
// Policy: the v1 cap of 16, and drop authority
// =============================================================================

#[test]
fn retention_refuses_cleanly_at_the_cap() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;
    let f = fs.create_file(root, "doc.txt".into()).unwrap();
    fs.write_data(f, 0, b"x").unwrap();

    // Fill retention to exactly the cap.
    let mut gens = Vec::new();
    for i in 0..SNAPSHOT_CAP {
        gens.push(
            fs.snapshot_create(format!("s{i}"), "alice".into(), i as u64)
                .unwrap(),
        );
    }
    assert_eq!(fs.snapshot_index().unwrap().len(), SNAPSHOT_CAP);

    // One more is refused — cleanly, no format damage, still consistent.
    let err = fs
        .snapshot_create("overflow".into(), "alice".into(), 99)
        .unwrap_err();
    assert!(
        matches!(err, unafs::fs::FileSystemError::SnapshotCapReached(n) if n == SNAPSHOT_CAP),
        "unexpected error at the cap: {err:?}"
    );
    assert_eq!(fs.snapshot_index().unwrap().len(), SNAPSHOT_CAP);
    assert!(fs.fsck(false).unwrap().is_clean());

    // Dropping one makes room again (cap is policy, not a one-way latch).
    fs.snapshot_drop(gens[0]).unwrap();
    assert_eq!(fs.snapshot_index().unwrap().len(), SNAPSHOT_CAP - 1);
    fs.snapshot_create("room".into(), "alice".into(), 100).unwrap();
    assert_eq!(fs.snapshot_index().unwrap().len(), SNAPSHOT_CAP);
    assert!(fs.fsck(false).unwrap().is_clean());
}

#[test]
fn drop_of_a_missing_generation_errors() {
    let mut fs = fresh_fs(5000);
    let err = fs.snapshot_drop(999).unwrap_err();
    assert!(matches!(
        err,
        unafs::fs::FileSystemError::SnapshotNotFound(999)
    ));
}

#[test]
fn owner_or_kernel_drop_authority() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;
    let f = fs.create_file(root, "doc.txt".into()).unwrap();
    fs.write_data(f, 0, b"x").unwrap();
    let snap_gen = fs.snapshot_create("s".into(), "alice".into(), 1).unwrap();
    let snap = find_snap(&mut fs, snap_gen);

    // Owner-or-kernel destructive authority (the BANDY ruling).
    assert!(snap.drop_permitted("alice", KERNEL), "owner may drop");
    assert!(snap.drop_permitted(KERNEL, KERNEL), "kernel may drop");
    assert!(!snap.drop_permitted("mallory", KERNEL), "a stranger may not");
}

// =============================================================================
// helpers
// =============================================================================

/// The physical data blocks a file's extents currently occupy.
fn data_blocks(fs: &mut UnaFS<MemDevice>, id: u64) -> Vec<u64> {
    let inode = fs.read_inode(id).unwrap();
    let mut out = Vec::new();
    for e in &inode.chunks {
        for i in 0..e.length.div_ceil(BLOCK_SIZE) {
            out.push(e.physical_block + i);
        }
    }
    out
}

fn find_snap(fs: &mut UnaFS<MemDevice>, generation: u64) -> unafs::SnapshotEntry {
    fs.snapshot_index()
        .unwrap()
        .into_iter()
        .find(|e| e.generation == generation)
        .expect("snapshot present")
}

/// Read a named file back THROUGH a snapshot's retained root, bypassing the
/// live tree entirely: walk the snapshot's inode map from disk to find the
/// directory-named inode, then read its extents. Proves the retained bytes are
/// the OLD bytes, independent of the live inode map.
fn read_via_snapshot(
    fs: &mut UnaFS<MemDevice>,
    snap: &unafs::SnapshotEntry,
    name: &str,
) -> Vec<u8> {
    // The root directory inode id is stable (ROOT_INODE_ID == 1). Read its
    // inode as the snapshot recorded it, list entries, find `name`, read data —
    // all via the snapshot's imap leaves, never the live map.
    let block_count = fs.superblock.block_count;
    let imap = read_snapshot_imap(fs, snap, block_count);
    // Root dir (logical id 1) → its physical block in the snapshot.
    let root_pb = imap[1];
    let root_inode = read_inode_at(fs, root_pb);
    let dir_bytes = read_extents(fs, &root_inode);
    let entries: Vec<unafs::DirEntry> = unafs::codec::deserialize(&dir_bytes).unwrap();
    let de = entries.into_iter().find(|e| e.name == name).unwrap();
    let file_pb = imap[de.inode_id as usize];
    let file_inode = read_inode_at(fs, file_pb);
    read_extents(fs, &file_inode)
}

/// Materialize a snapshot's logical-id → physical-block map from its imap.
fn read_snapshot_imap(
    fs: &mut UnaFS<MemDevice>,
    snap: &unafs::SnapshotEntry,
    _block_count: u64,
) -> Vec<u64> {
    let mut index = vec![0u8; BLOCK_SIZE as usize];
    fs.device.read_block(snap.imap_block, &mut index).unwrap();
    // Leaf slot `i` IS logical id `i` (slot 0 is the reserved id-0 entry), so
    // start empty and push every slot — no prepended sentinel.
    let mut imap = Vec::new();
    let mut leaf = vec![0u8; BLOCK_SIZE as usize];
    let entries_per_leaf = BLOCK_SIZE / 8;
    for l in 0..snap.imap_leaves {
        let ptr = u64::from_le_bytes(
            index[(l * 8) as usize..(l * 8 + 8) as usize]
                .try_into()
                .unwrap(),
        );
        fs.device.read_block(ptr, &mut leaf).unwrap();
        for e in 0..entries_per_leaf {
            let v = u64::from_le_bytes(
                leaf[(e * 8) as usize..(e * 8 + 8) as usize]
                    .try_into()
                    .unwrap(),
            );
            imap.push(v);
        }
    }
    imap
}

fn read_inode_at(fs: &mut UnaFS<MemDevice>, pb: u64) -> unafs::Inode {
    let mut block = vec![0u8; BLOCK_SIZE as usize];
    fs.device.read_block(pb, &mut block).unwrap();
    unafs::Inode::from_bytes(&block).unwrap()
}

fn read_extents(fs: &mut UnaFS<MemDevice>, inode: &unafs::Inode) -> Vec<u8> {
    let mut out = Vec::new();
    let mut block = vec![0u8; BLOCK_SIZE as usize];
    for e in &inode.chunks {
        let n = e.length.div_ceil(BLOCK_SIZE);
        for i in 0..n {
            fs.device.read_block(e.physical_block + i, &mut block).unwrap();
            let take = core::cmp::min(BLOCK_SIZE, e.length - i * BLOCK_SIZE) as usize;
            out.extend_from_slice(&block[..take]);
        }
    }
    out.truncate(inode.size as usize);
    out
}

// =============================================================================
// Lens A fix: a FAILED snapshot_create must not strand its increfs
// =============================================================================

#[test]
fn failed_snapshot_create_unwinds_cleanly_on_a_full_volume() {
    // A tight volume driven near-full with SUCCESSFUL ops only, so the only
    // failing operation under test is the snapshot_create itself.
    let mut fs = fresh_fs(64);
    let root = fs.superblock.root_inode;
    let f = fs.create_file(root, "fill.bin".into()).unwrap();
    let marker = vec![0x5Au8; BLOCK_SIZE as usize];
    fs.write_data(f, 0, &marker).unwrap();

    // Retention itself consumes space: each retained snapshot pins its whole
    // tree while later commits move the live maps/index to fresh blocks, so
    // repeated snapshot_create alone exhausts the tight volume — only
    // successful ops run before the one failing call under test.
    let mut retained = 0usize;
    let mut failed_err = None;
    for i in 0..SNAPSHOT_CAP as u64 {
        match fs.snapshot_create(format!("s{i}"), "alice".into(), i) {
            Ok(_) => retained += 1,
            Err(e) => {
                failed_err = Some(e);
                break;
            }
        }
    }
    let err = failed_err.expect("the tight volume must refuse a snapshot before the cap");
    assert!(
        matches!(err, unafs::fs::FileSystemError::NoSpace),
        "expected NoSpace, got {err:?}"
    );

    // The disk was never touched in a reachable way and the in-RAM state
    // unwound: exactly the successful snapshots recorded, no stranded increfs
    // (free count equals a fresh-from-disk mount's view), fsck clean, and the
    // mount is USABLE.
    assert_eq!(fs.snapshot_index().unwrap().len(), retained);
    let free_after_fail = fs.free_blocks();
    assert!(
        fs.fsck(false).unwrap().is_clean(),
        "a failed snapshot_create must leave the refcount map consistent"
    );
    let data_id = fs.resolve_path("/fill.bin").unwrap();
    assert_eq!(fs.read_data(data_id, 0, BLOCK_SIZE).unwrap(), marker);

    // A remount (ground truth from disk) agrees exactly on the free count —
    // proof nothing was stranded in RAM or leaked on disk.
    let device = fs.device.clone();
    drop(fs);
    let mut fs2 = UnaFS::mount(device).unwrap();
    assert_eq!(fs2.free_blocks(), free_after_fail);
    assert!(fs2.fsck(false).unwrap().is_clean());
    // And retrying the same snapshot on the remount fails the same clean way.
    assert!(matches!(
        fs2.snapshot_create("retry".into(), "alice".into(), 999),
        Err(unafs::fs::FileSystemError::NoSpace)
    ));
    assert!(fs2.fsck(false).unwrap().is_clean());
}
