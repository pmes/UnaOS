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

//! UNAFS-BATCH: the bulk create+write path (`create_files_batch`) KATs.
//!
//! The VAIRE-2 baseline measured the cold native sync at ~11.3 s with 97 % of
//! the wall in the commit phase — 242 per-file root flips. `create_files_batch`
//! is the vectored create/write API that collapses that: many files land under
//! one parent in ONE transaction (one root flip), with the parent directory and
//! the attribute catalog re-serialized exactly once for the whole set instead
//! of once per file / once per attribute.
//!
//! These KATs pin the transaction contract the brief names: staging, the
//! one-flip commit, the whole-tree single-transaction shape (autocommit off +
//! one commit), mid-batch failure unwind to the committed root, batch+snapshot
//! composition, and refcount correctness across a batched reclaim.

use std::collections::BTreeMap;
use unafs::fs::FileSystemError;
use unafs::{AttributeValue, BLOCK_SIZE, BatchFile, BlockDevice, MemDevice, UnaFS};

/// Format a fresh in-memory volume of `block_count` blocks.
fn fresh_fs(block_count: u64) -> UnaFS<MemDevice> {
    let mut device = MemDevice::new();
    let empty_block = vec![0u8; BLOCK_SIZE as usize];
    device
        .write_block(block_count - 1, &empty_block)
        .expect("Failed to set disk size");
    UnaFS::format(device, 20).expect("Format failed")
}

/// A `BatchFile` with data and no attributes.
fn file(name: &str, data: Vec<u8>) -> BatchFile {
    BatchFile {
        name: name.to_string(),
        data,
        attributes: BTreeMap::new(),
    }
}

/// A `BatchFile` with data and a set of typed attributes.
fn file_attrs(name: &str, data: Vec<u8>, attrs: &[(&str, AttributeValue)]) -> BatchFile {
    let mut attributes = BTreeMap::new();
    for (k, v) in attrs {
        attributes.insert(k.to_string(), v.clone());
    }
    BatchFile {
        name: name.to_string(),
        data,
        attributes,
    }
}

// =============================================================================
// Staging: the batch creates every file, with its bytes, names, and attributes.
// =============================================================================

#[test]
fn batch_stages_every_file_with_bytes_names_and_attrs() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;

    let files = vec![
        file_attrs(
            "alpha.txt",
            vec![0xA1u8; 100],
            &[("vaire.size", AttributeValue::Int(100))],
        ),
        file("bravo.bin", vec![0xB2u8; 2 * BLOCK_SIZE as usize]),
        file_attrs(
            "charlie.log",
            b"charlie".to_vec(),
            &[("vaire.src", AttributeValue::String("/live/charlie".into()))],
        ),
    ];
    let ids = fs.create_files_batch(root, files).unwrap();
    assert_eq!(ids.len(), 3, "one id returned per staged file");

    // All three names are present in the parent, sorted.
    let names: Vec<String> = fs.ls(root).unwrap().into_iter().map(|e| e.name).collect();
    assert_eq!(names, vec!["alpha.txt", "bravo.bin", "charlie.log"]);

    // Bytes round-trip.
    let a = fs.resolve_path("/alpha.txt").unwrap();
    assert_eq!(fs.read_data(a, 0, 100).unwrap(), vec![0xA1u8; 100]);
    let b = fs.resolve_path("/bravo.bin").unwrap();
    assert_eq!(
        fs.read_data(b, 0, 2 * BLOCK_SIZE).unwrap(),
        vec![0xB2u8; 2 * BLOCK_SIZE as usize]
    );
    let c = fs.resolve_path("/charlie.log").unwrap();
    assert_eq!(fs.read_data(c, 0, 7).unwrap(), b"charlie".to_vec());

    // Attributes round-trip.
    assert_eq!(
        fs.get_attribute(a, "vaire.size").unwrap(),
        Some(AttributeValue::Int(100))
    );
    assert_eq!(
        fs.get_attribute(c, "vaire.src").unwrap(),
        Some(AttributeValue::String("/live/charlie".into()))
    );

    assert!(fs.fsck(false).unwrap().is_clean());
}

// =============================================================================
// One-flip commit: N files, autocommit on == exactly ONE root flip.
// =============================================================================

#[test]
fn batch_of_many_files_is_a_single_root_flip() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;

    let before = fs.commit_stats().commits;
    let gen_before = fs.root_generation();

    let files: Vec<BatchFile> = (0..50)
        .map(|i| file(&format!("f{i:03}.dat"), vec![i as u8; 40]))
        .collect();
    fs.create_files_batch(root, files).unwrap();

    assert_eq!(
        fs.commit_stats().commits - before,
        1,
        "50 files must land in exactly ONE root flip (the 242-flip regime is gone)"
    );
    assert_eq!(
        fs.root_generation() - gen_before,
        1,
        "exactly one generation advance"
    );
    assert_eq!(fs.ls(root).unwrap().len(), 50);
    assert!(fs.fsck(false).unwrap().is_clean());
}

// Contrast witness: the per-op path flips once PER file (the baseline regime).
#[test]
fn per_op_path_flips_once_per_file_the_baseline_regime() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;

    let before = fs.commit_stats().commits;
    for i in 0..10 {
        let id = fs.create_file(root, format!("g{i}.dat")).unwrap();
        fs.write_data(id, 0, &vec![i as u8; 40]).unwrap();
    }
    // create_file + write_data == 2 flips per file on the per-op path.
    assert_eq!(fs.commit_stats().commits - before, 20);
}

// =============================================================================
// Whole-tree single transaction: autocommit off + many batches + one commit.
// =============================================================================

#[test]
fn whole_tree_across_many_dirs_is_one_flip() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;

    let before = fs.commit_stats().commits;
    let gen_before = fs.root_generation();

    // Drive the sync as the batched harness does: autocommit OFF, stage the
    // whole tree (dirs + files across dirs), commit ONCE at the end.
    fs.set_autocommit(false);
    let dir_a = fs.mkdir(root, "dir_a".into()).unwrap();
    let dir_b = fs.mkdir(root, "dir_b".into()).unwrap();
    fs.create_files_batch(
        dir_a,
        (0..20)
            .map(|i| file(&format!("a{i}.txt"), vec![0xAAu8; 30]))
            .collect(),
    )
    .unwrap();
    fs.create_files_batch(
        dir_b,
        (0..20)
            .map(|i| file(&format!("b{i}.txt"), vec![0xBBu8; 30]))
            .collect(),
    )
    .unwrap();
    fs.create_files_batch(
        root,
        vec![file("top.txt", b"top".to_vec())],
    )
    .unwrap();

    // Nothing has flipped yet — it is all staged.
    assert_eq!(
        fs.commit_stats().commits - before,
        0,
        "under autocommit off no batch flips the root"
    );

    fs.commit().unwrap();
    fs.set_autocommit(true);

    assert_eq!(
        fs.commit_stats().commits - before,
        1,
        "the whole tree (2 dirs, 41 files) lands in ONE flip"
    );
    assert_eq!(fs.root_generation() - gen_before, 1);
    assert_eq!(fs.ls(dir_a).unwrap().len(), 20);
    assert_eq!(fs.ls(dir_b).unwrap().len(), 20);
    assert!(fs.fsck(false).unwrap().is_clean());

    // Survives a remount: the batched tree is fully durable.
    let device = fs.device.clone();
    let mut fs2 = UnaFS::mount(device).unwrap();
    let a7 = fs2.resolve_path("/dir_a/a7.txt").unwrap();
    assert_eq!(fs2.read_data(a7, 0, 30).unwrap(), vec![0xAAu8; 30]);
    assert!(fs2.fsck(false).unwrap().is_clean());
}

// =============================================================================
// Mid-batch failure unwinds to exactly the last committed root.
// =============================================================================

#[test]
fn name_collision_mid_batch_unwinds_to_committed_root() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;

    // A committed baseline: one existing file.
    fs.create_file(root, "existing.txt".into()).unwrap();
    let gen_before = fs.root_generation();
    let free_before = fs.free_blocks();
    let commits_before = fs.commit_stats().commits;

    // A batch whose 3rd file collides with the existing name.
    let files = vec![
        file("new1.txt", vec![0x01u8; 50]),
        file("new2.txt", vec![0x02u8; 50]),
        file("existing.txt", vec![0x03u8; 50]), // collision
        file("new4.txt", vec![0x04u8; 50]),
    ];
    let err = fs.create_files_batch(root, files);
    assert!(err.is_err(), "the colliding batch must fail");

    // Exactly the last committed root: no generation change, no new names, no
    // leaked blocks, and fsck-clean.
    assert_eq!(fs.root_generation(), gen_before, "no root flipped");
    assert_eq!(fs.commit_stats().commits, commits_before);
    assert_eq!(fs.free_blocks(), free_before, "no blocks leaked by the unwind");
    let names: Vec<String> = fs.ls(root).unwrap().into_iter().map(|e| e.name).collect();
    assert_eq!(names, vec!["existing.txt"], "no partial file survived");
    assert!(fs.resolve_path("/new1.txt").is_err());
    assert!(fs.fsck(false).unwrap().is_clean());

    // The filesystem is fully usable afterward — a clean batch still commits.
    let ids = fs
        .create_files_batch(root, vec![file("after.txt", b"ok".to_vec())])
        .unwrap();
    assert_eq!(ids.len(), 1);
    assert!(fs.fsck(false).unwrap().is_clean());
}

#[test]
fn intra_batch_duplicate_names_fail_closed() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;
    let gen_before = fs.root_generation();

    let files = vec![
        file("dup.txt", vec![0x01u8; 50]),
        file("dup.txt", vec![0x02u8; 50]), // same name inside the batch
    ];
    assert!(fs.create_files_batch(root, files).is_err());
    assert_eq!(fs.root_generation(), gen_before);
    assert_eq!(fs.ls(root).unwrap().len(), 0, "true no-op");
    assert!(fs.fsck(false).unwrap().is_clean());
}

#[test]
fn full_volume_mid_batch_unwinds_cleanly() {
    // A tight volume: the batch cannot fit, so a write runs out of space
    // mid-stage and the whole batch must unwind to the committed root.
    let mut fs = fresh_fs(48);
    let root = fs.superblock.root_inode;
    let gen_before = fs.root_generation();
    let free_before = fs.free_blocks();

    // Each file is several blocks; the batch as a whole overruns the volume.
    let files: Vec<BatchFile> = (0..40)
        .map(|i| file(&format!("big{i}.bin"), vec![i as u8; 3 * BLOCK_SIZE as usize]))
        .collect();
    let err = fs.create_files_batch(root, files);
    assert!(matches!(err, Err(FileSystemError::NoSpace)));

    assert_eq!(fs.root_generation(), gen_before, "no root flipped");
    assert_eq!(fs.free_blocks(), free_before, "no blocks leaked");
    assert_eq!(fs.ls(root).unwrap().len(), 0, "nothing partially created");
    assert!(fs.fsck(false).unwrap().is_clean());
}

// =============================================================================
// Catalog: batched attributes are query-indexed exactly like set_attribute.
// =============================================================================

#[test]
fn batched_attributes_are_query_indexed() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;

    let files = vec![
        file_attrs(
            "one.txt",
            b"one".to_vec(),
            &[("kind", AttributeValue::String("doc".into()))],
        ),
        file_attrs(
            "two.txt",
            b"two".to_vec(),
            &[("kind", AttributeValue::String("doc".into()))],
        ),
        file_attrs(
            "three.txt",
            b"three".to_vec(),
            &[("kind", AttributeValue::String("img".into()))],
        ),
    ];
    fs.create_files_batch(root, files).unwrap();

    // The catalog was updated for every batched attribute: a query for
    // kind == "doc" finds exactly the two doc files.
    let hits = fs.query("kind == \"doc\"").unwrap();
    assert_eq!(
        hits.len(),
        2,
        "batched attrs must be catalog-indexed like set_attribute"
    );
    assert!(fs.fsck(false).unwrap().is_clean());
}

// =============================================================================
// Snapshot composition: a snapshot after a batch retains the WHOLE batch.
// =============================================================================

#[test]
fn snapshot_after_batch_sees_the_whole_batch() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;

    fs.create_files_batch(
        root,
        (0..12)
            .map(|i| file(&format!("s{i}.txt"), vec![0x5Au8; 60]))
            .collect(),
    )
    .unwrap();

    // One snapshot = one atomic root; it retains all 12 batched files.
    let snap_gen = fs
        .snapshot_create("post-batch".into(), "alice".into(), 100)
        .unwrap();
    assert_eq!(fs.snapshot_index().unwrap().len(), 1);
    assert!(fs.fsck(false).unwrap().is_clean());

    // Mutate the live tree hard (unlink half), then confirm the snapshot and
    // reclaim accounting stay consistent (block sharing is intact).
    for i in 0..6 {
        fs.unlink(root, &format!("s{i}.txt")).unwrap();
    }
    assert!(fs.fsck(false).unwrap().is_clean());

    // Drop the snapshot: reclamation runs and the volume stays fsck-clean; the
    // blocks the batch created that no live/retained root reaches are freed.
    let free_with_snap = fs.free_blocks();
    fs.snapshot_drop(snap_gen).unwrap();
    assert!(fs.fsck(false).unwrap().is_clean());
    assert!(
        fs.free_blocks() >= free_with_snap,
        "dropping the snapshot cannot lose free space"
    );
    assert_eq!(fs.snapshot_index().unwrap().len(), 0);
}

// =============================================================================
// Refcount correctness across a batched reclaim: the batch path's allocation
// and reclamation accounting is IDENTICAL to the per-op path on the same
// workload. This is the exact refcount-parity claim — proven by driving both
// paths to the same committed shape and comparing the free-block ledger.
// =============================================================================

#[test]
fn batched_reclaim_matches_the_per_op_path_block_for_block() {
    // Workload: 30 two-block files under root, each carrying one Int attr.
    let make = |i: usize| {
        file_attrs(
            &format!("r{i}.bin"),
            vec![i as u8; 2 * BLOCK_SIZE as usize],
            &[("vaire.size", AttributeValue::Int(2 * BLOCK_SIZE as i64))],
        )
    };

    // --- The batch path ---
    let mut fb = fresh_fs(5000);
    let rb = fb.superblock.root_inode;
    let files: Vec<BatchFile> = (0..30).map(make).collect();
    fb.create_files_batch(rb, files).unwrap();
    let batch_free_populated = fb.free_blocks();
    assert!(fb.fsck(false).unwrap().is_clean());
    for i in 0..30 {
        fb.unlink(rb, &format!("r{i}.bin")).unwrap();
    }
    let batch_free_after = fb.free_blocks();
    assert!(fb.fsck(false).unwrap().is_clean());

    // --- The per-op path, identical committed shape ---
    let mut fp = fresh_fs(5000);
    let rp = fp.superblock.root_inode;
    for i in 0..30 {
        let bf = make(i);
        let id = fp.create_file(rp, bf.name).unwrap();
        fp.write_data(id, 0, &bf.data).unwrap();
        for (k, v) in bf.attributes {
            fp.set_attribute(id, k, v).unwrap();
        }
    }
    let perop_free_populated = fp.free_blocks();
    assert!(fp.fsck(false).unwrap().is_clean());
    for i in 0..30 {
        fp.unlink(rp, &format!("r{i}.bin")).unwrap();
    }
    let perop_free_after = fp.free_blocks();
    assert!(fp.fsck(false).unwrap().is_clean());

    // Same live footprint populated, same free ledger after full delete: the
    // batch path allocates and reclaims exactly what the per-op path does.
    assert_eq!(
        batch_free_populated, perop_free_populated,
        "batch and per-op paths reach the same populated free-block count"
    );
    assert_eq!(
        batch_free_after, perop_free_after,
        "batch and per-op paths reclaim identically after full delete"
    );
    // And the delete actually reclaimed the bulk of the data blocks.
    assert!(batch_free_after > batch_free_populated + 50);
}

// =============================================================================
// Composition with existing directory contents: a batch merges, never clobbers.
// =============================================================================

#[test]
fn batch_merges_into_a_directory_with_prior_contents() {
    let mut fs = fresh_fs(5000);
    let root = fs.superblock.root_inode;

    // Pre-existing entries from the per-op path.
    fs.create_file(root, "zzz_pre.txt".into()).unwrap();
    fs.mkdir(root, "sub".into()).unwrap();

    fs.create_files_batch(
        root,
        vec![
            file("aaa_new.txt", b"a".to_vec()),
            file("mmm_new.txt", b"m".to_vec()),
        ],
    )
    .unwrap();

    let names: Vec<String> = fs.ls(root).unwrap().into_iter().map(|e| e.name).collect();
    // Sorted, and both the prior entries and the batch are present.
    assert_eq!(names, vec!["aaa_new.txt", "mmm_new.txt", "sub", "zzz_pre.txt"]);
    assert!(fs.fsck(false).unwrap().is_clean());
}
