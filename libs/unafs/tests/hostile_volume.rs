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

//! BEFS-HARDEN: hostile/corrupt volume fixtures for the on-disk parser.
//!
//! Every test crafts a volume a physically swapped or corrupted card could
//! present (the parser is kernel-reachable via the BeFS-K3 RO mount) and
//! asserts the library answers with a graceful `Err` — never a panic, a
//! capacity overflow, or an OOM abort. `#[should_panic]` is deliberately
//! absent: a panic IS the defect class under test.

use unafs::bitmap::SpaceMap;
use unafs::fs::FileSystemError;
use unafs::storage::Error as StorageError;
use unafs::{
    AttributeValue, BLOCK_SIZE, BlockDevice, Extent, Inode, MemDevice, Superblock, UnaFS,
};

/// A pre-sized in-memory device (MemDevice grows on write; reads past the
/// high-water mark fail, so the raw disk span must exist up front).
fn sized_device(size_mb: u64) -> MemDevice {
    let mut device = MemDevice::new();
    let blocks = size_mb * 1024 * 1024 / BLOCK_SIZE;
    let empty = vec![0u8; BLOCK_SIZE as usize];
    device.write_block(blocks - 1, &empty).expect("pre-size device");
    device
}

/// Format a small valid volume on an in-memory device.
fn formatted(size_mb: u64) -> UnaFS<MemDevice> {
    UnaFS::format(sized_device(size_mb), size_mb).expect("format must succeed")
}

/// Format a small valid volume and hand back the raw device (post-format,
/// superblock synced).
fn valid_volume() -> MemDevice {
    formatted(16).device.clone()
}

/// Clone `dev` and rewrite its superblock (block 0) after applying `mutate`.
fn with_corrupt_sb(dev: &MemDevice, mutate: impl FnOnce(&mut Superblock)) -> MemDevice {
    let mut dev = dev.clone();
    let mut block = vec![0u8; BLOCK_SIZE as usize];
    dev.read_block(0, &mut block).expect("read superblock");
    let mut sb = Superblock::from_bytes(&block).expect("valid superblock");
    mutate(&mut sb);
    let bytes = sb.to_bytes().expect("hostile superblock still serializes");
    let mut out = vec![0u8; BLOCK_SIZE as usize];
    out[..bytes.len()].copy_from_slice(&bytes);
    dev.write_block(0, &out).expect("write superblock");
    dev
}

#[test]
fn pristine_volume_still_mounts_and_lists() {
    // Positive control: hardening must not reject a valid volume.
    let dev = valid_volume();
    let mut fs = UnaFS::mount(dev).expect("valid volume must mount");
    let root = fs.superblock.root_inode;
    let entries = fs.ls(root).expect("root ls must succeed");
    assert!(entries.is_empty());
}

#[test]
fn oversized_bitmap_blocks_refused_at_mount() {
    // K3-PARSE-1: bitmap_blocks ~12289 drove a ~50 MiB mount-time allocation.
    // The geometry bound now refuses it before any allocation happens.
    let dev = valid_volume();
    let hostile = with_corrupt_sb(&dev, |sb| sb.bitmap_blocks = 12289);
    assert!(UnaFS::mount(hostile).is_err());
}

#[test]
fn absurd_bitmap_blocks_refused_not_capacity_panic() {
    // K3-PARSE-1: 2^51 bitmap blocks used to be a capacity-overflow panic.
    let dev = valid_volume();
    let hostile = with_corrupt_sb(&dev, |sb| sb.bitmap_blocks = 1 << 51);
    assert!(UnaFS::mount(hostile).is_err());
}

#[test]
fn spacemap_load_with_absurd_count_is_graceful() {
    // Defense in depth below the superblock bound: even a direct load with a
    // hostile count must fail with AllocRefused, not abort/panic.
    let mut dev = valid_volume();
    match SpaceMap::load(&mut dev, 11, 1 << 51) {
        Err(StorageError::AllocRefused(_)) => {}
        Err(other) => panic!("expected AllocRefused, got {other:?}"),
        Ok(_) => panic!("hostile bitmap count must not load"),
    }
    // And the multiplication-overflow flavor (count * BLOCK_SIZE wraps).
    assert!(matches!(
        SpaceMap::load(&mut dev, 11, u64::MAX / 8),
        Err(StorageError::AllocRefused(_))
    ));
}

#[test]
fn block_count_overflowing_volume_bytes_refused() {
    let dev = valid_volume();
    let hostile = with_corrupt_sb(&dev, |sb| sb.block_count = u64::MAX);
    assert!(UnaFS::mount(hostile).is_err());
}

#[test]
fn root_inode_out_of_bounds_refused() {
    let dev = valid_volume();
    let past_end = with_corrupt_sb(&dev, |sb| sb.root_inode = sb.block_count + 5);
    assert!(UnaFS::mount(past_end).is_err());

    let zero = with_corrupt_sb(&dev, |sb| sb.root_inode = 0);
    assert!(UnaFS::mount(zero).is_err());
}

#[test]
fn catalog_inode_out_of_bounds_refused() {
    let dev = valid_volume();
    let hostile = with_corrupt_sb(&dev, |sb| sb.catalog_inode = sb.block_count);
    assert!(UnaFS::mount(hostile).is_err());
}

#[test]
fn bitmap_span_past_volume_end_refused() {
    let dev = valid_volume();
    // Keep bitmap_blocks self-consistent but push the span off the end.
    let hostile = with_corrupt_sb(&dev, |sb| sb.bitmap_start = sb.block_count);
    assert!(UnaFS::mount(hostile).is_err());

    // And the wrapping flavor.
    let wrapping = with_corrupt_sb(&dev, |sb| sb.bitmap_start = u64::MAX);
    assert!(UnaFS::mount(wrapping).is_err());
}

#[test]
fn journal_layout_mismatch_refused() {
    // The WAL addresses the journal via compile-time constants; a superblock
    // declaring any other placement describes a volume this code never wrote.
    let dev = valid_volume();
    let moved = with_corrupt_sb(&dev, |sb| sb.journal_start = 2);
    assert!(UnaFS::mount(moved).is_err());

    let resized = with_corrupt_sb(&dev, |sb| sb.journal_blocks = 1 << 40);
    assert!(UnaFS::mount(resized).is_err());
}

#[test]
fn free_blocks_exceeding_volume_refused() {
    let dev = valid_volume();
    let hostile = with_corrupt_sb(&dev, |sb| sb.free_blocks = sb.block_count + 1);
    assert!(UnaFS::mount(hostile).is_err());
}

// --- Read-path hardening (K3-PARSE-2/4 + the QSIM-flagged query surface) ---

/// Rewrite inode `id` on the live filesystem's device after applying `mutate`.
fn corrupt_inode(fs: &mut UnaFS<MemDevice>, id: u64, mutate: impl FnOnce(&mut Inode)) {
    let mut inode = fs.read_inode(id).expect("inode readable before corruption");
    mutate(&mut inode);
    let bytes = inode.to_bytes().expect("hostile inode still serializes");
    let mut block = vec![0u8; BLOCK_SIZE as usize];
    block[..bytes.len()].copy_from_slice(&bytes);
    fs.device.write_block(id, &block).expect("write inode block");
}

#[test]
fn huge_inode_size_read_is_graceful() {
    // K3-PARSE-2: inode.size == u64::MAX drove Vec::with_capacity → panic;
    // near-heap sizes drove OOM aborts. Reached at boot via ls → read_data.
    let mut fs = formatted(16);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "victim".into()).unwrap();
    fs.write_data(id, 0, b"hello").unwrap();

    corrupt_inode(&mut fs, id, |inode| inode.size = u64::MAX);
    assert!(matches!(
        fs.read_data(id, 0, u64::MAX),
        Err(FileSystemError::CorruptVolume(_))
    ));

    // The just-under-volume flavor must also be refused (size > volume span).
    let volume_bytes = fs.superblock.block_count * BLOCK_SIZE;
    corrupt_inode(&mut fs, id, |inode| inode.size = volume_bytes + 1);
    assert!(fs.read_data(id, 0, volume_bytes + 1).is_err());
}

#[test]
fn huge_directory_size_ls_is_graceful() {
    // The exact K3 boot shape: k3_mount_selftest calls ls(root).
    let mut fs = formatted(16);
    let root = fs.superblock.root_inode;
    fs.create_file(root, "a".into()).unwrap();

    corrupt_inode(&mut fs, root, |inode| inode.size = u64::MAX);
    assert!(fs.ls(root).is_err());
}

#[test]
fn overflowing_extent_span_read_is_graceful() {
    // K3-PARSE-4: logical_offset + length wrapped (a panic under a
    // debug/hardened profile). Now a checked, profile-independent Err.
    let mut fs = formatted(16);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "victim".into()).unwrap();
    fs.write_data(id, 0, b"hello").unwrap();

    corrupt_inode(&mut fs, id, |inode| {
        inode.chunks = vec![Extent {
            logical_offset: u64::MAX - 100,
            physical_block: 12,
            length: 4096,
        }];
    });
    assert!(matches!(
        fs.read_data(id, 0, 5),
        Err(FileSystemError::CorruptVolume(_))
    ));
}

#[test]
fn extent_pointing_past_volume_read_is_graceful() {
    let mut fs = formatted(16);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "victim".into()).unwrap();
    fs.write_data(id, 0, b"hello").unwrap();

    let past_end = fs.superblock.block_count + 100;
    corrupt_inode(&mut fs, id, |inode| {
        inode.chunks[0].physical_block = past_end;
    });
    assert!(matches!(
        fs.read_data(id, 0, 5),
        Err(FileSystemError::CorruptVolume(_))
    ));

    // And the wrapping flavor: physical_block + block_idx overflows.
    corrupt_inode(&mut fs, id, |inode| {
        inode.size = 4 * BLOCK_SIZE;
        inode.chunks = vec![Extent {
            logical_offset: 0,
            physical_block: u64::MAX - 1,
            length: 4 * BLOCK_SIZE,
        }];
    });
    assert!(fs.read_data(id, BLOCK_SIZE * 3, 5).is_err());
}

#[test]
fn sparse_hole_read_stays_bounded_and_bulk() {
    // Holes are legal; the fill must be a bounded bulk run, not a 2^64
    // byte-push. A wholly sparse inode at a legal size reads back as zeros.
    let mut fs = formatted(16);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "sparse".into()).unwrap();

    let size = 2 * BLOCK_SIZE + 17;
    corrupt_inode(&mut fs, id, |inode| {
        inode.size = size;
        inode.chunks = Vec::new();
    });
    let data = fs.read_data(id, 0, size).expect("sparse read succeeds");
    assert_eq!(data.len() as u64, size);
    assert!(data.iter().all(|&b| b == 0));
}

#[test]
fn hostile_catalog_size_fails_query_not_panic() {
    // The QSIM addendum surface: UnaFS::query sizes its catalog read from
    // the catalog inode's disk-derived size.
    let mut fs = formatted(16);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "tagged".into()).unwrap();
    fs.set_attribute(id, "kind".into(), AttributeValue::String("test".into()))
        .unwrap();

    let catalog = fs.superblock.catalog_inode;
    corrupt_inode(&mut fs, catalog, |inode| inode.size = u64::MAX);
    assert!(fs.query("kind == \"test\"").is_err());
}

#[test]
fn hostile_spilled_attribute_extents_fail_query_not_panic() {
    // The QSIM addendum's second site: query sums a spilled attribute's
    // extent lengths (hostile sum wraps / exceeds the volume).
    let mut fs = formatted(16);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "vec".into()).unwrap();
    // > 64 elements spills to large_attributes.
    let big = AttributeValue::Vector(vec![0.5f32; 128]);
    fs.set_attribute(id, "embed".into(), big).unwrap();

    corrupt_inode(&mut fs, id, |inode| {
        let extents = inode.large_attributes.get_mut("embed").unwrap();
        extents.push(Extent {
            logical_offset: 0,
            physical_block: 20,
            length: u64::MAX - 10,
        });
    });
    // Overflowing sum → graceful Err on the query path...
    assert!(fs.query("similarity(embed, [0.5, 0.5]) > 0.1").is_err());
    // ...and on the get_attribute path.
    assert!(fs.get_attribute(id, "embed").is_err());
}

// --- Codec-level hardening (K3-PARSE-3: hostile bincode length prefixes) ---
//
// bincode's owned-bytes decode (String / Vec<u8>) allocates from the CLAIMED
// length before reading any payload; under the old no-limit config a crafted
// prefix drove an unbounded infallible allocation. The budgeted decodes must
// refuse the claim instead.

#[test]
fn hostile_string_prefix_in_inode_block_refused() {
    // A 4 KiB inode block whose attribute-key length claims ~2^62 bytes.
    let mut inode = Inode::new(12, unafs::FileKind::File);
    inode
        .attributes
        .insert("kk".into(), AttributeValue::Int(7));
    let mut bytes = inode.to_bytes().unwrap();

    // Legacy layout: id u64 | kind u32 | size u64 | chunks len u64 |
    // attributes len u64 | first key len u64 | ...
    let key_len_off = 8 + 4 + 8 + 8 + 8;
    assert_eq!(
        &bytes[key_len_off..key_len_off + 8],
        &2u64.to_le_bytes(),
        "layout probe: key length prefix not where expected"
    );
    bytes[key_len_off..key_len_off + 8].copy_from_slice(&(u64::MAX / 2).to_le_bytes());

    assert!(Inode::from_bytes(&bytes).is_err());
}

#[test]
fn hostile_attribute_value_prefix_refused() {
    // A spilled attribute decoding as AttributeValue::String with a claimed
    // length of ~2^62 (variant tag 2 = String, then the u64 length prefix).
    let mut hostile = Vec::new();
    hostile.extend_from_slice(&2u32.to_le_bytes());
    hostile.extend_from_slice(&(u64::MAX / 2).to_le_bytes());
    assert!(unafs::codec::deserialize::<AttributeValue>(&hostile).is_err());
}

#[test]
fn hostile_directory_entry_name_prefix_fails_ls_not_abort() {
    // End-to-end: a directory whose on-disk entry list claims a ~2^62-byte
    // name. ls must return Err — under the old config this pre-allocated
    // unboundedly (an infallible vec! — an abort, not even a panic).
    let mut fs = formatted(16);
    let root = fs.superblock.root_inode;
    fs.create_file(root, "a".into()).unwrap();

    let root_inode = fs.read_inode(root).unwrap();
    let data_block = root_inode.chunks[0].physical_block;

    let mut block = vec![0u8; BLOCK_SIZE as usize];
    fs.device.read_block(data_block, &mut block).unwrap();
    // Vec<DirEntry> layout: count u64 | first name len u64 | ...
    assert_eq!(&block[0..8], &1u64.to_le_bytes(), "layout probe: entry count");
    block[8..16].copy_from_slice(&(u64::MAX / 2).to_le_bytes());
    fs.device.write_block(data_block, &block).unwrap();

    assert!(fs.ls(root).is_err());
}

#[test]
fn hostile_collection_count_prefix_refused() {
    // A directory entry list claiming ~2^61 entries with no payload: the
    // decode must fail promptly (serde's seq path reads element-wise and runs
    // out of input; the budget bounds any claimed owned-bytes on the way).
    let mut hostile = Vec::new();
    hostile.extend_from_slice(&(u64::MAX / 4).to_le_bytes());
    assert!(unafs::codec::deserialize::<Vec<unafs::DirEntry>>(&hostile).is_err());
}

#[test]
fn valid_records_decode_under_the_budgets() {
    // Positive control: the budgets must not reject legitimate records.
    let mut inode = Inode::new(12, unafs::FileKind::File);
    inode
        .attributes
        .insert("name".into(), AttributeValue::String("fixture".into()));
    let bytes = inode.to_bytes().unwrap();
    let back = Inode::from_bytes(&bytes).unwrap();
    assert_eq!(back, inode);
}
