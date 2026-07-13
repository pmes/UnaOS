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

//! Mutation KATs in the format-contract style: unlink / rename /
//! remove_attribute must leave the volume consistent — a deleted or renamed
//! entry unreachable by name AND by query, free-space accounting that
//! round-trips, and a mutated volume that re-mounts clean.

use unafs::fs::FileSystemError;
use unafs::{AttributeValue, BLOCK_SIZE, BlockDevice, MemDevice, UnaFS};

/// Format a fresh in-memory volume of `block_count` blocks.
fn fresh_fs(block_count: u64) -> UnaFS<MemDevice> {
    let mut device = MemDevice::new();
    let empty_block = vec![0u8; BLOCK_SIZE as usize];
    device
        .write_block(block_count - 1, &empty_block)
        .expect("Failed to set disk size");
    UnaFS::format(device, 20).expect("Format failed")
}

// --- M1: unlink -------------------------------------------------------------

#[test]
fn unlink_removes_name_and_every_query_path() {
    let mut fs = fresh_fs(5000);
    let root_id = fs.superblock.root_inode;

    // Doomed file: an inline attribute, a re-set (duplicated catalog entry)
    // attribute, and a spilled (>64-float) vector.
    let doomed_id = fs.create_file(root_id, "doomed.txt".to_string()).unwrap();
    fs.write_data(doomed_id, 0, b"soon gone").unwrap();
    fs.set_attribute(
        doomed_id,
        "emotion".to_string(),
        AttributeValue::String("fleeting".to_string()),
    )
    .unwrap();
    // Re-set with a different value: set_attribute APPENDS a catalog entry,
    // so the catalog now holds two entries for this key. Unlink must scrub
    // both.
    fs.set_attribute(
        doomed_id,
        "emotion".to_string(),
        AttributeValue::String("doomed".to_string()),
    )
    .unwrap();
    let big: Vec<f32> = (0..100).map(|i| i as f32 * 0.5).collect();
    fs.set_attribute(
        doomed_id,
        "embedding".to_string(),
        AttributeValue::Vector(big.clone()),
    )
    .unwrap();

    // Survivor file with its own attribute — must be untouched.
    let survivor_id = fs.create_file(root_id, "survivor.txt".to_string()).unwrap();
    fs.set_attribute(
        survivor_id,
        "emotion".to_string(),
        AttributeValue::String("steady".to_string()),
    )
    .unwrap();

    // Sanity: everything reachable before the unlink.
    assert_eq!(fs.query("emotion == \"doomed\"").unwrap().len(), 1);
    assert_eq!(fs.ls(root_id).unwrap().len(), 2);

    let freed_id = fs.unlink(root_id, "doomed.txt").expect("unlink failed");
    assert_eq!(freed_id, doomed_id);

    // Unreachable by name.
    let entries = fs.ls(root_id).unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "survivor.txt");
    assert!(fs.resolve_path("/doomed.txt").is_err());

    // Unreachable by query — current value, stale duplicate value, and the
    // spilled similarity path must ALL come back empty.
    assert!(fs.query("emotion == \"doomed\"").unwrap().is_empty());
    assert!(fs.query("emotion == \"fleeting\"").unwrap().is_empty());
    let target: Vec<String> = big.iter().map(|f| format!("{:?}", f)).collect();
    let sim_q = format!("similarity(embedding, [{}]) > 0.5", target.join(", "));
    assert!(fs.query(&sim_q).unwrap().is_empty());

    // The survivor is untouched: name, attribute, and query all intact.
    let sid = fs.resolve_path("/survivor.txt").unwrap();
    assert_eq!(sid, survivor_id);
    let results = fs.query("emotion == \"steady\"").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0.id, survivor_id);
}

#[test]
fn unlink_free_space_round_trips_and_blocks_reuse() {
    let mut fs = fresh_fs(5000);
    let root_id = fs.superblock.root_inode;

    // Seed the volume so the directory and catalog data blocks exist before
    // the baseline snapshot (their rewrites during unlink then net to zero).
    let keeper_id = fs.create_file(root_id, "keeper.txt".to_string()).unwrap();
    fs.set_attribute(
        keeper_id,
        "tag".to_string(),
        AttributeValue::String("keep".to_string()),
    )
    .unwrap();

    let free_before = fs.superblock.free_blocks;

    // Doomed file: inode block + 2 data blocks + spilled attribute extents.
    let doomed_id = fs.create_file(root_id, "bulky.bin".to_string()).unwrap();
    fs.write_data(doomed_id, 0, &vec![0xA5u8; 2 * BLOCK_SIZE as usize])
        .unwrap();
    let big: Vec<f32> = (0..200).map(|i| i as f32).collect();
    fs.set_attribute(
        doomed_id,
        "embedding".to_string(),
        AttributeValue::Vector(big),
    )
    .unwrap();

    let free_during = fs.superblock.free_blocks;
    assert!(free_during < free_before, "creation must consume blocks");

    // Track the doomed footprint for the reuse witness below.
    let doomed_inode = fs.read_inode(doomed_id).unwrap();
    let mut max_freed = doomed_id;
    for extent in doomed_inode
        .chunks
        .iter()
        .chain(doomed_inode.large_attributes.values().flatten())
    {
        let blocks = extent.length.div_ceil(BLOCK_SIZE);
        max_freed = max_freed.max(extent.physical_block + blocks - 1);
    }

    fs.unlink(root_id, "bulky.bin").expect("unlink failed");

    // Exact free-space round-trip: every block the file consumed (inode,
    // data, spill) came back, and the catalog/directory rewrites netted out.
    assert_eq!(
        fs.superblock.free_blocks, free_before,
        "free-space accounting must round-trip across create+unlink"
    );

    // Reuse witness: the first-fit allocator hands the next inode a block
    // from the freed pool (at or below the doomed file's high-water mark).
    let reborn_id = fs.create_file(root_id, "reborn.txt".to_string()).unwrap();
    assert!(
        reborn_id <= max_freed,
        "expected first-fit reuse of a freed block: got {} > {}",
        reborn_id,
        max_freed
    );

    // And the keeper still resolves and queries.
    assert_eq!(fs.resolve_path("/keeper.txt").unwrap(), keeper_id);
    assert_eq!(fs.query("tag == \"keep\"").unwrap().len(), 1);
}

#[test]
fn unlink_refuses_directories_and_missing_names() {
    let mut fs = fresh_fs(5000);
    let root_id = fs.superblock.root_inode;
    fs.mkdir(root_id, "home".to_string()).unwrap();

    assert!(matches!(
        fs.unlink(root_id, "home"),
        Err(FileSystemError::IsADirectory)
    ));
    assert!(matches!(
        fs.unlink(root_id, "nope.txt"),
        Err(FileSystemError::NotFound)
    ));
    // The refusals must not have disturbed the directory.
    assert_eq!(fs.ls(root_id).unwrap().len(), 1);
}
