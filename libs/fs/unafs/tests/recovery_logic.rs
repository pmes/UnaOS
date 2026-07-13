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

//! UNAFS-F1 recovery KATs: the fsck-scavenger and the dirty-mount replay path.
//!
//! Each test crafts one of the crash-window residues the F2 mutation engine is
//! *documented* to leave on a program-order power cut — a leaked block, a
//! leaked inode+extents, a query-orphaned inode, a dirty journal — and asserts
//! the scavenger reclaims/heals it to a consistent, remount-clean volume while
//! leaving live data untouched.
//!
//! The torn states are built with the crate's own public surface (the pub
//! `bitmap`/`superblock`/`journal`/`device` fields plus `unafs::codec`), which
//! reproduces exactly the on-disk shape a crash inside `unlink`/`rename` would
//! leave: an inode unhooked from its directory (a single-block directory
//! rewrite that landed) with its blocks not yet freed, or its catalog entries
//! not yet scrubbed.

use unafs::{
    AttributeValue, BLOCK_SIZE, BlockDevice, DirEntry, Journal, JournalOp, MemDevice, UnaFS,
};

/// Format a fresh in-memory volume of `block_count` blocks.
fn fresh_fs(block_count: u64) -> UnaFS<MemDevice> {
    let mut device = MemDevice::new();
    let empty_block = vec![0u8; BLOCK_SIZE as usize];
    device
        .write_block(block_count - 1, &empty_block)
        .expect("Failed to set disk size");
    UnaFS::format(device, 20).expect("Format failed")
}

/// Rewrite directory `dir_id`'s on-disk entry list to exactly `entries` — the
/// crash-ordered directory-rewrite step, in isolation, WITHOUT the block frees
/// or catalog scrub that would follow it in a completed mutation. Reproduces
/// the "directory rewrite landed, crash before the frees" torn state.
fn craft_dir_listing(fs: &mut UnaFS<MemDevice>, dir_id: u64, entries: &[DirEntry]) {
    let bytes = unafs::codec::serialize(&entries.to_vec()).expect("serialize dir entries");
    // The replacement list is never longer than the original (we only drop an
    // entry), so an in-place overwrite at offset 0 is sound: `ls` reads the
    // Vec length prefix and stops, ignoring the stale tail.
    fs.write_data(dir_id, 0, &bytes).expect("rewrite directory");
}

// --- F1.A: a bare leaked block ---------------------------------------------

#[test]
fn fsck_reclaims_a_leaked_block() {
    let mut fs = fresh_fs(5000);

    // Model a rewrite that allocated a block then crashed before any inode
    // adopted it: the bitmap bit is set and free_blocks was decremented, but
    // no inode references it.
    let victim = fs.bitmap.allocate().expect("allocate a block to leak");
    fs.superblock.free_blocks -= 1;
    fs.sync_metadata().unwrap();
    let free_leaked = fs.superblock.free_blocks;
    assert!(fs.bitmap.is_used(victim));

    // Dry run first: it must SEE the leak and touch nothing.
    let dry = fs.fsck(false).unwrap();
    assert_eq!(dry.leaked_blocks, vec![victim]);
    assert_eq!(dry.reclaimed_blocks, 0);
    assert!(fs.bitmap.is_used(victim), "dry run must not free anything");
    assert_eq!(fs.superblock.free_blocks, free_leaked);

    // Repair: the block comes back, free space round-trips.
    let repaired = fs.fsck(true).unwrap();
    assert_eq!(repaired.reclaimed_blocks, 1);
    assert!(!fs.bitmap.is_used(victim));
    assert_eq!(fs.superblock.free_blocks, free_leaked + 1);

    // Idempotent: a second pass finds a clean volume.
    let again = fs.fsck(true).unwrap();
    assert!(again.is_clean());
    assert_eq!(again.reclaimed_blocks, 0);
}

// --- F1.B: unlink window — inode + extents leaked, no name, no catalog ------

#[test]
fn fsck_reclaims_an_unhooked_inode_and_its_extents() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;

    // A doomed file with two data blocks and NO attributes (so it leaves no
    // catalog entry — a pure name-tree leak, not a query-orphan).
    let ghost = fs.create_file(root, "ghost.txt".to_string()).unwrap();
    fs.write_data(ghost, 0, &vec![0xEEu8; 2 * BLOCK_SIZE as usize])
        .unwrap();
    let survivor = fs.create_file(root, "survivor.txt".to_string()).unwrap();
    fs.write_data(survivor, 0, b"still here").unwrap();

    // Footprint of the doomed file: its inode block + every data block.
    let ghost_inode = fs.read_inode(ghost).unwrap();
    let mut ghost_blocks = vec![ghost];
    for extent in &ghost_inode.chunks {
        let n = extent.length.div_ceil(BLOCK_SIZE);
        for i in 0..n {
            ghost_blocks.push(extent.physical_block + i);
        }
    }
    assert!(ghost_blocks.len() >= 3, "inode + 2 data blocks expected");
    for &b in &ghost_blocks {
        assert!(fs.bitmap.is_used(b));
    }

    // Craft the torn state: the directory rewrite that removed ghost landed,
    // but the block frees never ran.
    let survivors: Vec<DirEntry> = fs
        .ls(root)
        .unwrap()
        .into_iter()
        .filter(|e| e.name != "ghost.txt")
        .collect();
    craft_dir_listing(&mut fs, root, &survivors);
    let free_torn = fs.superblock.free_blocks;

    // Now ghost is reachable by no name and no query.
    assert!(fs.resolve_path("/ghost.txt").is_err());

    let report = fs.recover().unwrap();
    assert_eq!(report.reclaimed_blocks, ghost_blocks.len() as u64);
    assert!(report.orphan_inodes.is_empty());

    // Every ghost block returned; free space round-trips exactly.
    for &b in &ghost_blocks {
        assert!(!fs.bitmap.is_used(b), "block {b} must be reclaimed");
    }
    assert_eq!(fs.superblock.free_blocks, free_torn + ghost_blocks.len() as u64);

    // The survivor is entirely untouched.
    assert_eq!(fs.resolve_path("/survivor.txt").unwrap(), survivor);
    let sinode = fs.read_inode(survivor).unwrap();
    assert_eq!(fs.read_data(survivor, 0, sinode.size).unwrap(), b"still here");

    // And the volume remounts clean.
    let device = fs.device.clone();
    drop(fs);
    let mut probe = device.clone();
    assert!(!Journal::new().check_recovery(&mut probe).unwrap());
    let mut fs2 = UnaFS::mount(device).unwrap();
    assert!(fs2.fsck(false).unwrap().is_clean());
}

// --- F1.C: cross-dir rename window — query-orphan (name gone, catalog kept) -

#[test]
fn fsck_heals_a_query_orphaned_inode() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;
    let inbox = fs.mkdir(root, "inbox".to_string()).unwrap();

    let memo = fs.create_file(inbox, "memo.txt".to_string()).unwrap();
    fs.write_data(memo, 0, b"in flight").unwrap();
    fs.set_attribute(
        memo,
        "kind".to_string(),
        AttributeValue::String("memo".to_string()),
    )
    .unwrap();

    // Before: reachable by both name and query.
    assert_eq!(fs.resolve_path("/inbox/memo.txt").unwrap(), memo);
    assert_eq!(fs.query("kind == \"memo\"").unwrap().len(), 1);

    // Craft the cross-directory rename window: the entry LEFT the source
    // directory (that single-block rewrite landed) but never joined a
    // destination, and the catalog — which keys on the inode id — is untouched.
    craft_dir_listing(&mut fs, inbox, &[]);

    // The residue is exactly a query-orphan: no name, but query still finds it.
    assert!(fs.resolve_path("/inbox/memo.txt").is_err());
    assert_eq!(fs.query("kind == \"memo\"").unwrap().len(), 1);

    // Dry run identifies the orphan without touching it.
    let dry = fs.fsck(false).unwrap();
    assert_eq!(dry.orphan_inodes, vec![memo]);
    assert_eq!(fs.query("kind == \"memo\"").unwrap().len(), 1);

    let free_torn = fs.superblock.free_blocks;

    // Repair: scrub the catalog (so no query dangles) and reclaim the blocks.
    let report = fs.fsck(true).unwrap();
    assert_eq!(report.orphan_inodes, vec![memo]);
    assert!(report.scrubbed_catalog_entries >= 1);
    assert!(report.reclaimed_blocks >= 1);

    // Gone from query AND the inode block is freed.
    assert!(fs.query("kind == \"memo\"").unwrap().is_empty());
    assert!(!fs.bitmap.is_used(memo));
    assert!(fs.superblock.free_blocks > free_torn);

    // Remounts clean and consistent.
    let device = fs.device.clone();
    drop(fs);
    let mut fs2 = UnaFS::mount(device).unwrap();
    assert!(fs2.fsck(false).unwrap().is_clean());
    assert!(fs2.query("kind == \"memo\"").unwrap().is_empty());
    assert!(fs2.ls(inbox).unwrap().is_empty());
}

// --- F1.D: recover() clears a dirty journal while reclaiming ----------------

#[test]
fn recover_clears_dirty_journal_and_reclaims() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;
    fs.create_file(root, "keep.txt".to_string()).unwrap();

    // A leaked block...
    let victim = fs.bitmap.allocate().expect("allocate a block to leak");
    fs.superblock.free_blocks -= 1;
    fs.sync_metadata().unwrap();

    // ...plus a torn transaction in the journal (a BeginOp with no EndOp),
    // exactly what a power cut mid-mutation leaves behind.
    fs.journal
        .append(&mut fs.device, JournalOp::BeginOp {
            op_id: 42,
            desc: "torn mutation".to_string(),
        })
        .unwrap();
    assert!(fs.is_dirty().unwrap(), "test premise: journal must read dirty");

    let report = fs.recover().unwrap();
    assert!(report.dirty_journal, "recover must report the torn journal");
    assert_eq!(report.reclaimed_blocks, 1);

    // The dirty flag is cleared: a fresh scan reads clean.
    assert!(!fs.is_dirty().unwrap());
    assert!(!fs.bitmap.is_used(victim));

    // keep.txt survived the whole recovery.
    assert!(fs.resolve_path("/keep.txt").is_ok());

    // Remount witnesses a clean journal.
    let device = fs.device.clone();
    drop(fs);
    let mut probe = device.clone();
    assert!(!Journal::new().check_recovery(&mut probe).unwrap());
}

// --- F1.E: the scavenger never eats a live, healthy volume ------------------

#[test]
fn fsck_leaves_a_clean_volume_untouched() {
    let mut fs = fresh_fs(6000);
    let root = fs.superblock.root_inode;

    // Build a small live world: nested dirs, files with data, inline + spilled
    // attributes, and a query index over all of it.
    let docs = fs.mkdir(root, "docs".to_string()).unwrap();
    let sub = fs.mkdir(docs, "sub".to_string()).unwrap();
    let a = fs.create_file(docs, "a.txt".to_string()).unwrap();
    fs.write_data(a, 0, &vec![0x11u8; 3 * BLOCK_SIZE as usize])
        .unwrap();
    fs.set_attribute(a, "tag".to_string(), AttributeValue::String("alpha".into()))
        .unwrap();
    let b = fs.create_file(sub, "b.bin".to_string()).unwrap();
    let big: Vec<f32> = (0..200).map(|i| i as f32 * 0.5).collect();
    fs.set_attribute(b, "embedding".to_string(), AttributeValue::Vector(big))
        .unwrap();
    fs.set_attribute(b, "tag".to_string(), AttributeValue::String("beta".into()))
        .unwrap();

    let free_before = fs.superblock.free_blocks;
    let in_use_before = fs.fsck(false).unwrap().blocks_in_use;

    // A clean volume: zero leaks, zero orphans — and in repair mode it frees
    // NOTHING (the critical "don't eat live data" property).
    let dry = fs.fsck(false).unwrap();
    assert!(dry.is_clean(), "healthy volume must scan clean: {dry:?}");
    assert!(dry.leaked_blocks.is_empty());
    assert!(dry.orphan_inodes.is_empty());
    assert_eq!(dry.reachable_blocks, in_use_before, "every used block is reachable");

    let repaired = fs.fsck(true).unwrap();
    assert_eq!(repaired.reclaimed_blocks, 0, "repair on a clean volume frees nothing");
    assert_eq!(fs.superblock.free_blocks, free_before);

    // Everything still resolves, reads, and queries after the pass.
    assert_eq!(fs.resolve_path("/docs/a.txt").unwrap(), a);
    assert_eq!(fs.resolve_path("/docs/sub/b.bin").unwrap(), b);
    let ai = fs.read_inode(a).unwrap();
    assert_eq!(fs.read_data(a, 0, ai.size).unwrap(), vec![0x11u8; 3 * BLOCK_SIZE as usize]);
    assert_eq!(fs.query("tag == \"alpha\"").unwrap().len(), 1);
    assert_eq!(fs.query("tag == \"beta\"").unwrap().len(), 1);
}
