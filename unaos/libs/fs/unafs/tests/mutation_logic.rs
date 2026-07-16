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
use unafs::inode::FileKind;
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

    let free_before = fs.free_blocks();

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

    let free_during = fs.free_blocks();
    assert!(free_during < free_before, "creation must consume blocks");

    // Track the doomed PHYSICAL footprint for the reuse witness below
    // (inode ids are logical under K8 — the reuse question is about blocks).
    let doomed_inode = fs.read_inode(doomed_id).unwrap();
    let mut max_freed = fs.inode_block(doomed_id).unwrap();
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
        fs.free_blocks(),
        free_before,
        "free-space accounting must round-trip across create+unlink"
    );

    // Reuse witness: after the unlink's COMMIT retired the old tree, the
    // first-fit allocator hands the next inode a PHYSICAL block from the
    // freed pool (at or below the doomed file's high-water mark). The
    // logical id, by contrast, is never recycled.
    let reborn_id = fs.create_file(root_id, "reborn.txt".to_string()).unwrap();
    assert!(reborn_id > doomed_id, "logical ids must never be recycled");
    let reborn_block = fs.inode_block(reborn_id).unwrap();
    assert!(
        reborn_block <= max_freed,
        "expected first-fit reuse of a freed block: got {} > {}",
        reborn_block,
        max_freed
    );

    // And the keeper still resolves and queries.
    assert_eq!(fs.resolve_path("/keeper.txt").unwrap(), keeper_id);
    assert_eq!(fs.query("tag == \"keep\"").unwrap().len(), 1);
}

// --- M2: rename -------------------------------------------------------------

#[test]
fn rename_same_directory_old_gone_new_found_content_identical() {
    let mut fs = fresh_fs(5000);
    let root_id = fs.superblock.root_inode;

    let file_id = fs.create_file(root_id, "draft.txt".to_string()).unwrap();
    fs.write_data(file_id, 0, b"the manuscript").unwrap();
    fs.set_attribute(
        file_id,
        "state".to_string(),
        AttributeValue::String("polished".to_string()),
    )
    .unwrap();
    // A spilled attribute too — must survive the rename untouched.
    let big: Vec<f32> = (0..100).map(|i| i as f32 * 0.25).collect();
    fs.set_attribute(
        file_id,
        "embedding".to_string(),
        AttributeValue::Vector(big.clone()),
    )
    .unwrap();

    let free_before = fs.free_blocks();
    fs.rename(root_id, "draft.txt", root_id, "final.txt")
        .expect("rename failed");

    // Old name ENOENT, new name found, same inode.
    assert!(fs.resolve_path("/draft.txt").is_err());
    assert_eq!(fs.resolve_path("/final.txt").unwrap(), file_id);

    // Inode, data, and attributes untouched.
    let inode = fs.read_inode(file_id).unwrap();
    assert_eq!(fs.read_data(file_id, 0, inode.size).unwrap(), b"the manuscript");
    assert_eq!(
        fs.get_attribute(file_id, "state").unwrap(),
        Some(AttributeValue::String("polished".to_string()))
    );
    assert_eq!(
        fs.get_attribute(file_id, "embedding").unwrap(),
        Some(AttributeValue::Vector(big))
    );

    // Catalog consistent: the query still returns exactly this inode.
    let results = fs.query("state == \"polished\"").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0.id, file_id);

    // A pure rename allocates nothing net (directory rewrite nets to zero).
    assert_eq!(fs.free_blocks(), free_before);
}

#[test]
fn rename_cross_directory_moves_the_entry() {
    let mut fs = fresh_fs(5000);
    let root_id = fs.superblock.root_inode;
    let inbox_id = fs.mkdir(root_id, "inbox".to_string()).unwrap();
    let archive_id = fs.mkdir(root_id, "archive".to_string()).unwrap();

    let file_id = fs.create_file(inbox_id, "memo.txt".to_string()).unwrap();
    fs.write_data(file_id, 0, b"move me").unwrap();
    fs.set_attribute(
        file_id,
        "kind".to_string(),
        AttributeValue::String("memo".to_string()),
    )
    .unwrap();

    fs.rename(inbox_id, "memo.txt", archive_id, "memo-2026.txt")
        .expect("cross-directory rename failed");

    // Gone from the source, present in the destination.
    assert!(fs.ls(inbox_id).unwrap().is_empty());
    let dst = fs.ls(archive_id).unwrap();
    assert_eq!(dst.len(), 1);
    assert_eq!(dst[0].name, "memo-2026.txt");
    assert_eq!(dst[0].inode_id, file_id);
    assert_eq!(dst[0].kind, FileKind::File);

    // Path resolution follows.
    assert!(fs.resolve_path("/inbox/memo.txt").is_err());
    assert_eq!(fs.resolve_path("/archive/memo-2026.txt").unwrap(), file_id);

    // Content and query reach the same inode.
    let inode = fs.read_inode(file_id).unwrap();
    assert_eq!(fs.read_data(file_id, 0, inode.size).unwrap(), b"move me");
    let results = fs.query("kind == \"memo\"").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0.id, file_id);
}

#[test]
fn rename_refusals_collision_loop_missing_noop() {
    let mut fs = fresh_fs(5000);
    let root_id = fs.superblock.root_inode;
    let a_id = fs.mkdir(root_id, "a".to_string()).unwrap();
    let b_id = fs.mkdir(a_id, "b".to_string()).unwrap();
    fs.create_file(root_id, "x.txt".to_string()).unwrap();
    fs.create_file(root_id, "y.txt".to_string()).unwrap();

    // Existing destination name: refused, no implicit overwrite.
    assert!(matches!(
        fs.rename(root_id, "x.txt", root_id, "y.txt"),
        Err(FileSystemError::FileExists)
    ));

    // Directory loop: /a into /a/b (its own descendant) and into itself.
    assert!(matches!(
        fs.rename(root_id, "a", b_id, "a-again"),
        Err(FileSystemError::DirectoryLoop)
    ));
    assert!(matches!(
        fs.rename(root_id, "a", a_id, "a-again"),
        Err(FileSystemError::DirectoryLoop)
    ));

    // Missing source.
    assert!(matches!(
        fs.rename(root_id, "ghost.txt", root_id, "real.txt"),
        Err(FileSystemError::NotFound)
    ));

    // Same-name same-dir rename is a no-op Ok.
    fs.rename(root_id, "x.txt", root_id, "x.txt").unwrap();

    // Nothing was disturbed by the refusals.
    let names: Vec<String> = fs.ls(root_id).unwrap().into_iter().map(|e| e.name).collect();
    assert_eq!(names, vec!["a", "x.txt", "y.txt"]);
    assert_eq!(fs.resolve_path("/a/b").unwrap(), b_id);
}

// --- M3: remove_attribute ----------------------------------------------------

#[test]
fn remove_attribute_inline_query_misses_others_intact() {
    let mut fs = fresh_fs(5000);
    let root_id = fs.superblock.root_inode;

    let file_id = fs.create_file(root_id, "tagged.txt".to_string()).unwrap();
    fs.set_attribute(
        file_id,
        "mood".to_string(),
        AttributeValue::String("bright".to_string()),
    )
    .unwrap();
    // Re-set so the catalog carries a stale duplicate entry for the key.
    fs.set_attribute(
        file_id,
        "mood".to_string(),
        AttributeValue::String("brighter".to_string()),
    )
    .unwrap();
    fs.set_attribute(file_id, "priority".to_string(), AttributeValue::Int(7))
        .unwrap();

    fs.remove_attribute(file_id, "mood")
        .expect("remove_attribute failed");

    // Gone from the inode...
    assert_eq!(fs.get_attribute(file_id, "mood").unwrap(), None);
    // ...and from every query path, including the stale duplicate value.
    assert!(fs.query("mood == \"brighter\"").unwrap().is_empty());
    assert!(fs.query("mood == \"bright\"").unwrap().is_empty());

    // The other attribute is intact and still queryable.
    assert_eq!(
        fs.get_attribute(file_id, "priority").unwrap(),
        Some(AttributeValue::Int(7))
    );
    let results = fs.query("priority == 7").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0.id, file_id);

    // Removing it again is AttributeNotFound.
    assert!(matches!(
        fs.remove_attribute(file_id, "mood"),
        Err(FileSystemError::AttributeNotFound)
    ));
}

#[test]
fn remove_attribute_spilled_frees_extents() {
    let mut fs = fresh_fs(5000);
    let root_id = fs.superblock.root_inode;

    let file_id = fs.create_file(root_id, "vec.bin".to_string()).unwrap();
    // Seed a small attribute so the catalog data block exists before the
    // baseline snapshot.
    fs.set_attribute(file_id, "kept".to_string(), AttributeValue::Int(1))
        .unwrap();

    let free_before = fs.free_blocks();

    // 200 floats: over the 64-float inline threshold, spills to extents.
    let big: Vec<f32> = (0..200).map(|i| i as f32 * 0.5).collect();
    fs.set_attribute(
        file_id,
        "embedding".to_string(),
        AttributeValue::Vector(big.clone()),
    )
    .unwrap();
    assert!(
        fs.read_inode(file_id)
            .unwrap()
            .large_attributes
            .contains_key("embedding"),
        "test premise: the vector must have spilled"
    );
    assert!(fs.free_blocks() < free_before);

    fs.remove_attribute(file_id, "embedding")
        .expect("remove_attribute failed");

    // Gone from the inode maps, the value, and the similarity query path.
    let inode = fs.read_inode(file_id).unwrap();
    assert!(!inode.large_attributes.contains_key("embedding"));
    assert_eq!(fs.get_attribute(file_id, "embedding").unwrap(), None);
    let target: Vec<String> = big.iter().map(|f| format!("{:?}", f)).collect();
    let sim_q = format!("similarity(embedding, [{}]) > 0.5", target.join(", "));
    assert!(fs.query(&sim_q).unwrap().is_empty());

    // The spilled extents came back: free space round-trips exactly.
    assert_eq!(
        fs.free_blocks(), free_before,
        "spilled-attribute extents must be freed"
    );

    // The untouched attribute survives.
    assert_eq!(
        fs.get_attribute(file_id, "kept").unwrap(),
        Some(AttributeValue::Int(1))
    );
}

// --- The full cycle: mount -> mutate -> remount ------------------------------

#[test]
fn mutated_volume_remounts_clean() {
    let mut fs = fresh_fs(5000);
    let root_id = fs.superblock.root_inode;

    // Build a small world...
    let docs_id = fs.mkdir(root_id, "docs".to_string()).unwrap();
    let attic_id = fs.mkdir(root_id, "attic".to_string()).unwrap();
    let keep_id = fs.create_file(docs_id, "keep.txt".to_string()).unwrap();
    fs.write_data(keep_id, 0, b"kept words").unwrap();
    fs.set_attribute(
        keep_id,
        "state".to_string(),
        AttributeValue::String("final".to_string()),
    )
    .unwrap();
    let big: Vec<f32> = (0..100).map(|i| i as f32 * 0.1).collect();
    fs.set_attribute(
        keep_id,
        "embedding".to_string(),
        AttributeValue::Vector(big),
    )
    .unwrap();
    let junk_id = fs.create_file(docs_id, "junk.txt".to_string()).unwrap();
    fs.write_data(junk_id, 0, b"ephemeral").unwrap();
    fs.set_attribute(
        junk_id,
        "state".to_string(),
        AttributeValue::String("junk".to_string()),
    )
    .unwrap();

    // ...then mutate it: unlink the junk, move + rename the keeper into the
    // attic, and drop its spilled embedding.
    fs.unlink(docs_id, "junk.txt").unwrap();
    fs.rename(docs_id, "keep.txt", attic_id, "treasure.txt").unwrap();
    fs.remove_attribute(keep_id, "embedding").unwrap();

    // Remount from the raw device bytes.
    let device = fs.device.clone();
    drop(fs);

    let mut fs2 = UnaFS::mount(device).expect("re-mount failed");

    // The CoW cleanliness witness: every mutation committed atomically, so
    // the remounted volume is refcount-consistent with nothing leaked.
    let report = fs2.fsck(false).expect("fsck");
    assert!(
        report.is_clean(),
        "mutated volume must re-mount clean: {report:?}"
    );

    // Structure survived: the junk is gone everywhere, the treasure moved.
    assert!(fs2.resolve_path("/docs/junk.txt").is_err());
    assert!(fs2.resolve_path("/docs/keep.txt").is_err());
    assert!(fs2.ls(docs_id).unwrap().is_empty());
    assert_eq!(fs2.resolve_path("/attic/treasure.txt").unwrap(), keep_id);

    // Content, attributes, and queries all agree with the mutations.
    let inode = fs2.read_inode(keep_id).unwrap();
    assert_eq!(fs2.read_data(keep_id, 0, inode.size).unwrap(), b"kept words");
    assert_eq!(fs2.get_attribute(keep_id, "embedding").unwrap(), None);
    assert!(fs2.query("state == \"junk\"").unwrap().is_empty());
    let results = fs2.query("state == \"final\"").unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0.id, keep_id);
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
