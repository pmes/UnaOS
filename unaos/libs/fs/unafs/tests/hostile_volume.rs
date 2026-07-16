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

//! BEFS-HARDEN: hostile/corrupt volume fixtures for the on-disk parser —
//! K8 (v3) edition: the superblock is static identity, the mutable geometry
//! lives in the ROOT RECORD, and the inode map is disk-derived input too.
//!
//! Every test crafts a volume a physically swapped or corrupted card could
//! present (the parser is kernel-reachable via the kernel mount) and asserts
//! the library answers with a graceful `Err` — never a panic, a capacity
//! overflow, or an OOM abort.

use unafs::fs::FileSystemError;
use unafs::root::{ROOT_BLOCK, ROOT_SECTOR_SIZE};
use unafs::{
    AttributeValue, BLOCK_SIZE, BlockDevice, Extent, Inode, MemDevice, RootRecord, Superblock,
    UnaFS,
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

/// Format a small valid volume and hand back the raw device.
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

/// Clone `dev` and rewrite its ACTIVE root record (checksummed, so the
/// hostile record PARSES — the field bounds are what must refuse it).
fn with_corrupt_root(dev: &MemDevice, mutate: impl FnOnce(&mut RootRecord)) -> MemDevice {
    let mut dev = dev.clone();
    let mut block = vec![0u8; BLOCK_SIZE as usize];
    dev.read_block(ROOT_BLOCK, &mut block).expect("read root block");
    let a = RootRecord::from_sector(&block[0..ROOT_SECTOR_SIZE]);
    let b = RootRecord::from_sector(&block[ROOT_SECTOR_SIZE..2 * ROOT_SECTOR_SIZE]);
    let (mut rec, slot_a) = match (a, b) {
        (Some(ra), Some(rb)) => {
            if ra.generation >= rb.generation {
                (ra, true)
            } else {
                (rb, false)
            }
        }
        (Some(ra), None) => (ra, true),
        (None, Some(rb)) => (rb, false),
        (None, None) => panic!("valid volume must carry a root record"),
    };
    mutate(&mut rec);
    let bytes = rec.to_bytes();
    let off = if slot_a { 0 } else { ROOT_SECTOR_SIZE };
    block[off..off + bytes.len()].copy_from_slice(&bytes);
    dev.write_block(ROOT_BLOCK, &block).expect("write root block");
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

// --- Superblock (static identity) bounds -------------------------------------

#[test]
fn block_count_overflowing_volume_bytes_refused() {
    let dev = valid_volume();
    let hostile = with_corrupt_sb(&dev, |sb| sb.block_count = u64::MAX);
    assert!(UnaFS::mount(hostile).is_err());
}

#[test]
fn non_reserved_root_inode_id_refused() {
    let dev = valid_volume();
    let wrong = with_corrupt_sb(&dev, |sb| sb.root_inode = 9);
    assert!(UnaFS::mount(wrong).is_err());
    let zero = with_corrupt_sb(&dev, |sb| sb.root_inode = 0);
    assert!(UnaFS::mount(zero).is_err());
}

#[test]
fn non_reserved_catalog_inode_id_refused() {
    let dev = valid_volume();
    let hostile = with_corrupt_sb(&dev, |sb| sb.catalog_inode = 40);
    assert!(UnaFS::mount(hostile).is_err());
}

#[test]
fn oversized_volume_fails_format_cleanly_not_panic() {
    // Lens-A fix (2026-07-16): a volume past the one-indirect-level map
    // structure (> MAX_BLOCK_COUNT blocks = 2 GiB) used to slice-panic in
    // the FORMAT commit's refmap-index fill; it must be a clean Err at both
    // seams. (`migrate` sizes its target from the source, so format must
    // fail clean on any size.)
    use unafs::superblock::MAX_BLOCK_COUNT;

    // format(): sized purely by size_mb (empty device) — 4 GiB request.
    let empty = MemDevice::new();
    assert!(
        UnaFS::format(empty, 4096).is_err(),
        "format(4096 MB) must fail cleanly, not panic"
    );

    // The boundary itself is representable arithmetic: exactly MAX is valid
    // geometry per validate(); one past it is refused.
    assert!(Superblock::new(MAX_BLOCK_COUNT).validate().is_ok());
    assert!(Superblock::new(MAX_BLOCK_COUNT + 1).validate().is_err());

    // mount(): the same geometry planted in a superblock is refused too.
    let dev = valid_volume();
    let hostile = with_corrupt_sb(&dev, |sb| sb.block_count = MAX_BLOCK_COUNT + 1);
    assert!(UnaFS::mount(hostile).is_err());
}

#[test]
fn tiny_volume_refused() {
    let dev = valid_volume();
    let hostile = with_corrupt_sb(&dev, |sb| sb.block_count = 2);
    assert!(UnaFS::mount(hostile).is_err());
}

// --- Root-record (mutable geometry) bounds — K3-PARSE-1's K8 successor -------

#[test]
fn hostile_imap_leaf_count_refused_at_mount() {
    let dev = valid_volume();
    // Zero leaves: structurally impossible (the reserved inodes exist).
    let zero = with_corrupt_root(&dev, |r| r.imap_leaves = 0);
    assert!(UnaFS::mount(zero).is_err());
    // A leaf count past the one-indirect-block structure.
    let huge = with_corrupt_root(&dev, |r| r.imap_leaves = (BLOCK_SIZE / 8) + 1);
    assert!(UnaFS::mount(huge).is_err());
    // An absurd count that would demand a giant allocation.
    let absurd = with_corrupt_root(&dev, |r| {
        r.imap_leaves = u64::MAX / 8;
        r.next_inode = u64::MAX / 16;
    });
    assert!(UnaFS::mount(absurd).is_err());
}

#[test]
fn hostile_next_inode_refused_at_mount() {
    let dev = valid_volume();
    // next_inode beyond what the declared leaves can hold: refused before
    // any allocation is sized from it.
    let hostile = with_corrupt_root(&dev, |r| r.next_inode = r.imap_leaves * (BLOCK_SIZE / 8) + 1);
    assert!(UnaFS::mount(hostile).is_err());
    let zero = with_corrupt_root(&dev, |r| r.next_inode = 0);
    assert!(UnaFS::mount(zero).is_err());
}

#[test]
fn imap_index_out_of_bounds_refused() {
    let dev = valid_volume();
    let past = with_corrupt_root(&dev, |r| r.imap_block = u64::MAX - 5);
    assert!(UnaFS::mount(past).is_err());
    let zero = with_corrupt_root(&dev, |r| r.imap_block = 0);
    assert!(UnaFS::mount(zero).is_err());
}

#[test]
fn refmap_size_mismatch_refused() {
    // The refcount map's leaf count is a pure function of the volume
    // geometry; any other declaration describes a volume this code never
    // wrote (the old "bitmap size inconsistent" bound, reborn).
    let dev = valid_volume();
    let hostile = with_corrupt_root(&dev, |r| r.refmap_leaves += 1);
    assert!(UnaFS::mount(hostile).is_err());
    let oob = with_corrupt_root(&dev, |r| r.refmap_block = u64::MAX - 5);
    assert!(UnaFS::mount(oob).is_err());
}

#[test]
fn imap_leaf_pointer_out_of_bounds_refused() {
    // Corrupt the imap INDEX block: leaf pointer 0 → past the volume.
    let fs = formatted(16);
    let mut dev = fs.device.clone();
    let block_count = fs.superblock.block_count;
    drop(fs);
    let (rr, _) = unafs::root::read_active(&mut dev).unwrap().unwrap();
    let mut index = vec![0u8; BLOCK_SIZE as usize];
    dev.read_block(rr.imap_block, &mut index).unwrap();
    index[0..8].copy_from_slice(&(block_count + 100).to_le_bytes());
    dev.write_block(rr.imap_block, &index).unwrap();
    assert!(UnaFS::mount(dev).is_err());
}

#[test]
fn imap_entry_out_of_bounds_refused() {
    // Corrupt an imap LEAF: point a logical id past the volume.
    let fs = formatted(16);
    let mut dev = fs.device.clone();
    let block_count = fs.superblock.block_count;
    drop(fs);
    let (rr, _) = unafs::root::read_active(&mut dev).unwrap().unwrap();
    let mut index = vec![0u8; BLOCK_SIZE as usize];
    dev.read_block(rr.imap_block, &mut index).unwrap();
    let leaf0 = u64::from_le_bytes(index[0..8].try_into().unwrap());
    let mut leaf = vec![0u8; BLOCK_SIZE as usize];
    dev.read_block(leaf0, &mut leaf).unwrap();
    // Entry for logical id 1 (the root directory).
    leaf[8..16].copy_from_slice(&(block_count + 7).to_le_bytes());
    dev.write_block(leaf0, &leaf).unwrap();
    assert!(UnaFS::mount(dev).is_err());
}

// --- Read-path hardening (K3-PARSE-2/4 + the QSIM-flagged query surface) ---

/// Rewrite inode `id`'s CURRENT block on the live filesystem's device after
/// applying `mutate` — raw media corruption, bypassing the CoW path.
fn corrupt_inode(fs: &mut UnaFS<MemDevice>, id: u64, mutate: impl FnOnce(&mut Inode)) {
    let pb = fs.inode_block(id).expect("inode allocated before corruption");
    let mut inode = fs.read_inode(id).expect("inode readable before corruption");
    mutate(&mut inode);
    let bytes = inode.to_bytes().expect("hostile inode still serializes");
    let mut block = vec![0u8; BLOCK_SIZE as usize];
    block[..bytes.len()].copy_from_slice(&bytes);
    fs.device.write_block(pb, &block).expect("write inode block");
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
    // The CoW WRITE path materializes the same disk-derived extents — it
    // must refuse the same geometry, not wrap.
    assert!(fs.write_data(id, 0, b"x").is_err());
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
    assert!(fs.write_data(id, 0, b"x").is_err());

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
fn inode_id_mismatch_refused() {
    // A block whose decoded inode claims a DIFFERENT logical id than the
    // map slot that reached it: cross-linked map corruption, refused.
    let mut fs = formatted(16);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "victim".into()).unwrap();
    corrupt_inode(&mut fs, id, |inode| inode.id += 1000);
    assert!(matches!(
        fs.read_inode(id),
        Err(FileSystemError::CorruptVolume(_))
    ));
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

#[test]
fn hostile_string_prefix_in_inode_block_refused() {
    // A 4 KiB inode block whose attribute-key length claims ~2^62 bytes.
    let mut inode = Inode::new(12, unafs::FileKind::File);
    inode.attributes.insert("kk".into(), AttributeValue::Int(7));
    let mut bytes = inode.to_bytes().unwrap();

    // Layout: id u64 | kind u32 | size u64 | chunks len u64 |
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

#[test]
fn hostile_prefix_just_under_budget_stays_bounded() {
    // r12 panel must-fix regression guard: a claim just UNDER the budget
    // passes bincode's check and drives an INFALLIBLE internal allocation of
    // the claimed size — so MAX_RECORD_BYTES itself is the largest alloc a
    // hostile prefix can force, and it must stay small relative to the
    // kernel heap. This test pins two things: (1) a just-under claim still
    // yields a graceful Err (payload absent), (2) the budget constant stays
    // in the kernel-safe band — if a future arc raises it past 8 MiB, this
    // fails and forces a deliberate review against the kernel heap.
    const KERNEL_SAFE_CEILING: usize = 8 * 1024 * 1024;
    assert!(
        unafs::codec::MAX_RECORD_BYTES <= KERNEL_SAFE_CEILING,
        "MAX_RECORD_BYTES ({}) exceeds the kernel-safe ceiling — a passing \
         bincode claim allocates infallibly; review against the kernel heap",
        unafs::codec::MAX_RECORD_BYTES
    );

    // AttributeValue::String (variant tag 2) claiming budget − 1 bytes:
    // passes the claim, allocates ~4 MiB (host-fine), then fails the read.
    let mut hostile = Vec::new();
    hostile.extend_from_slice(&2u32.to_le_bytes());
    hostile.extend_from_slice(&((unafs::codec::MAX_RECORD_BYTES as u64) - 1).to_le_bytes());
    assert!(unafs::codec::deserialize::<AttributeValue>(&hostile).is_err());

    // And just OVER the budget: the claim itself is refused.
    let mut hostile = Vec::new();
    hostile.extend_from_slice(&2u32.to_le_bytes());
    hostile.extend_from_slice(&((unafs::codec::MAX_RECORD_BYTES as u64) + 1).to_le_bytes());
    assert!(unafs::codec::deserialize::<AttributeValue>(&hostile).is_err());
}

// =============================================================================
// K8b: hostile SNAPSHOT INDEX / reclaim queue (lens B coverage)
// =============================================================================

/// Plant a hostile serialized snapshot-entry list in the on-disk snapshot
/// index object of an otherwise-valid volume (what a corrupted or crafted
/// card could present), returning the raw device.
fn with_hostile_snapshot_index(entries: Vec<unafs::SnapshotEntry>) -> MemDevice {
    let mut fs = formatted(16);
    let bytes = unafs::codec::serialize(&entries).expect("hostile index serializes");
    fs.write_data(unafs::superblock::SNAP_INDEX_INODE_ID, 0, &bytes)
        .expect("planting the hostile index");
    fs.device.clone()
}

#[test]
fn snapshot_entry_with_out_of_range_imap_block_fails_closed() {
    let device = with_hostile_snapshot_index(vec![unafs::SnapshotEntry {
        generation: 7,
        imap_block: u64::MAX - 5, // far past any volume
        imap_leaves: 1,
        name: "evil".into(),
        creator: "evil".into(),
        timestamp: 0,
    }]);

    // Mount itself succeeds (the index is not walked at mount) …
    let mut fs = UnaFS::mount(device).expect("mount survives a hostile index");
    // … but every path that WALKS the hostile root fails with a clean Err —
    // never a panic, and repair refuses to act on a partial walk.
    assert!(matches!(fs.fsck(false), Err(FileSystemError::CorruptVolume(_))));
    assert!(matches!(fs.fsck(true), Err(FileSystemError::CorruptVolume(_))));
    assert!(matches!(fs.snapshot_drop(7), Err(FileSystemError::CorruptVolume(_))));
    // The volume stays otherwise usable (reads unaffected).
    let root = fs.superblock.root_inode;
    assert!(fs.ls(root).is_ok());
}

#[test]
fn snapshot_entry_with_garbage_leaf_pointers_fails_closed() {
    // A block full of 0xFF: every "leaf pointer" decodes wildly out of range.
    let mut fs = formatted(16);
    let root = fs.superblock.root_inode;
    let f = fs.create_file(root, "garbage.bin".into()).unwrap();
    fs.write_data(f, 0, &vec![0xFFu8; BLOCK_SIZE as usize]).unwrap();
    let garbage_block = fs.read_inode(f).unwrap().chunks[0].physical_block;
    let bytes = unafs::codec::serialize(&vec![unafs::SnapshotEntry {
        generation: 9,
        imap_block: garbage_block, // in range, but its CONTENT is garbage
        imap_leaves: 1,
        name: "evil".into(),
        creator: "evil".into(),
        timestamp: 0,
    }])
    .unwrap();
    fs.write_data(unafs::superblock::SNAP_INDEX_INODE_ID, 0, &bytes)
        .unwrap();
    let device = fs.device.clone();
    drop(fs);

    let mut fs = UnaFS::mount(device).expect("mount survives");
    assert!(matches!(fs.fsck(false), Err(FileSystemError::CorruptVolume(_))));
    assert!(matches!(fs.fsck(true), Err(FileSystemError::CorruptVolume(_))));
    assert!(matches!(fs.snapshot_drop(9), Err(FileSystemError::CorruptVolume(_))));
}

#[test]
fn snapshot_index_over_the_cap_fails_closed() {
    // 17 entries (> SNAPSHOT_CAP), each hostile. Listing is bounded and fine;
    // create refuses at the cap; the walkers fail closed.
    let entries: Vec<unafs::SnapshotEntry> = (0..17)
        .map(|i| unafs::SnapshotEntry {
            generation: 100 + i,
            imap_block: u64::MAX - i,
            imap_leaves: 1,
            name: format!("evil{i}"),
            creator: "evil".into(),
            timestamp: i,
        })
        .collect();
    let device = with_hostile_snapshot_index(entries);

    let mut fs = UnaFS::mount(device).expect("mount survives");
    assert_eq!(fs.snapshot_index().unwrap().len(), 17);
    assert!(matches!(
        fs.snapshot_create("one-more".into(), "x".into(), 0),
        Err(FileSystemError::SnapshotCapReached(_))
    ));
    assert!(matches!(fs.fsck(false), Err(FileSystemError::CorruptVolume(_))));
    assert!(matches!(fs.fsck(true), Err(FileSystemError::CorruptVolume(_))));
}
