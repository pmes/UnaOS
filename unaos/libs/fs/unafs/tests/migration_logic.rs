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

//! K8a pre-K8 card migration: build a version-2 volume byte-for-byte (the
//! test IS the v2 writer now — production code only reads it), migrate it
//! through `legacy::migrate_into`, and verify the K8 volume carries the
//! whole tree: names, data, inline attributes, spilled attributes.

use std::collections::BTreeMap;
use unafs::inode::{AttributeValue, Extent, FileKind, Inode};
use unafs::legacy::{LegacySuperblock, LegacyVolume, migrate_into};
use unafs::{BLOCK_SIZE, BlockDevice, DirEntry, MemDevice, UnaFS};

/// Hand-build a small, valid VERSION-2 volume:
///   block 0: v2 superblock          block 12: root dir inode
///   1..11 : (journal, zeroed)       block 13: catalog inode (empty)
///   11    : bitmap (raw bits)       block 14: file inode "hello.txt"
///                                   block 15: file data
///                                   block 16: dir inode "sub"
///                                   block 17: sub-dir data (entry list)
///                                   block 18: file inode "nested.bin"
///                                   block 19..21: nested data (2 blocks)
///                                   block 22: root dir data
///                                   block 23: spilled attribute extent
fn build_v2_volume() -> MemDevice {
    let blocks: u64 = 4096; // 16 MB
    let mut dev = MemDevice::new();
    let zero = vec![0u8; BLOCK_SIZE as usize];
    dev.write_block(blocks - 1, &zero).unwrap();

    let put = |dev: &mut MemDevice, block: u64, bytes: &[u8]| {
        let mut buf = vec![0u8; BLOCK_SIZE as usize];
        buf[..bytes.len()].copy_from_slice(bytes);
        dev.write_block(block, &buf).unwrap();
    };

    // Superblock (v2 layout, exactly as the old code wrote it).
    let sb = LegacySuperblock {
        magic: *b"UNAFS",
        version: 2,
        block_size: BLOCK_SIZE as u32,
        block_count: blocks,
        root_inode: 12,
        free_blocks: blocks - 24,
        bitmap_start: 11,
        bitmap_blocks: 1,
        journal_start: 1,
        journal_blocks: 10,
        catalog_inode: 13,
    };
    put(&mut dev, 0, &unafs::codec::serialize(&sb).unwrap());

    // Bitmap: blocks 0..=23 used (3 bytes of 1s).
    let mut bitmap = vec![0u8; BLOCK_SIZE as usize];
    bitmap[0] = 0xFF;
    bitmap[1] = 0xFF;
    bitmap[2] = 0xFF;
    dev.write_block(11, &bitmap).unwrap();

    // Root directory (inode 12): entries "hello.txt" (14), "sub" (16).
    let root_entries = vec![
        DirEntry { name: "hello.txt".into(), inode_id: 14, kind: FileKind::File },
        DirEntry { name: "sub".into(), inode_id: 16, kind: FileKind::Directory },
    ];
    let root_data = unafs::codec::serialize(&root_entries).unwrap();
    let mut root = Inode::new(12, FileKind::Directory);
    root.size = root_data.len() as u64;
    root.chunks = vec![Extent { logical_offset: 0, physical_block: 22, length: root_data.len() as u64 }];
    put(&mut dev, 22, &root_data);
    put(&mut dev, 12, &root.to_bytes().unwrap());

    // Catalog inode (13): empty System file.
    let catalog = Inode::new(13, FileKind::System);
    put(&mut dev, 13, &catalog.to_bytes().unwrap());

    // hello.txt (inode 14): small data + inline attr + SPILLED attr.
    let hello = b"Hello from the pre-K8 world!\n";
    let spilled_val = AttributeValue::Vector((0..100).map(|i| i as f32 * 0.5).collect());
    let spilled_bytes = unafs::codec::serialize(&spilled_val).unwrap();
    let mut f1 = Inode::new(14, FileKind::File);
    f1.size = hello.len() as u64;
    f1.chunks = vec![Extent { logical_offset: 0, physical_block: 15, length: hello.len() as u64 }];
    f1.attributes.insert("mood".into(), AttributeValue::String("archival".into()));
    let mut large: BTreeMap<String, Vec<Extent>> = BTreeMap::new();
    large.insert(
        "embedding".into(),
        vec![Extent { logical_offset: 0, physical_block: 23, length: spilled_bytes.len() as u64 }],
    );
    f1.large_attributes = large;
    put(&mut dev, 15, hello);
    put(&mut dev, 23, &spilled_bytes);
    put(&mut dev, 14, &f1.to_bytes().unwrap());

    // sub/ (inode 16) with nested.bin (inode 18, 2 blocks of pattern).
    let sub_entries = vec![DirEntry { name: "nested.bin".into(), inode_id: 18, kind: FileKind::File }];
    let sub_data = unafs::codec::serialize(&sub_entries).unwrap();
    let mut sub = Inode::new(16, FileKind::Directory);
    sub.size = sub_data.len() as u64;
    sub.chunks = vec![Extent { logical_offset: 0, physical_block: 17, length: sub_data.len() as u64 }];
    put(&mut dev, 17, &sub_data);
    put(&mut dev, 16, &sub.to_bytes().unwrap());

    let nested: Vec<u8> = (0..2 * BLOCK_SIZE as usize).map(|i| ((i * 7 + 3) & 0xFF) as u8).collect();
    let mut f2 = Inode::new(18, FileKind::File);
    f2.size = nested.len() as u64;
    f2.chunks = vec![Extent { logical_offset: 0, physical_block: 19, length: nested.len() as u64 }];
    dev.write_block(19, &nested[..BLOCK_SIZE as usize].to_vec()).unwrap();
    dev.write_block(20, &nested[BLOCK_SIZE as usize..].to_vec()).unwrap();
    put(&mut dev, 18, &f2.to_bytes().unwrap());

    dev
}

#[test]
fn v2_volume_migrates_whole_into_k8() {
    let v2 = build_v2_volume();

    // The live (v3) code REFUSES the old format outright — no runtime compat.
    assert!(UnaFS::mount(v2.clone()).is_err());

    // The legacy reader walks it...
    let mut old = LegacyVolume::open(v2).expect("legacy open");
    assert_eq!(old.superblock.version, 2);

    // ...and the migration replays it into a fresh K8 volume.
    let mut target_dev = MemDevice::new();
    let zero = vec![0u8; BLOCK_SIZE as usize];
    target_dev.write_block(4095, &zero).unwrap();
    let mut new = UnaFS::format(target_dev, 16).expect("format target");
    let report = migrate_into(&mut old, &mut new).expect("migration");
    assert_eq!(report.files, 2);
    assert_eq!(report.directories, 1);
    assert!(report.bytes > 0);

    // Remount the target from raw bytes and verify EVERYTHING.
    let dev = new.device.clone();
    drop(new);
    let mut fs = UnaFS::mount(dev).expect("mount migrated volume");

    // Names.
    let hello_id = fs.resolve_path("/hello.txt").expect("hello.txt");
    let nested_id = fs.resolve_path("/sub/nested.bin").expect("sub/nested.bin");

    // Data, byte-for-byte.
    let hi = fs.read_inode(hello_id).unwrap();
    assert_eq!(
        fs.read_data(hello_id, 0, hi.size).unwrap(),
        b"Hello from the pre-K8 world!\n"
    );
    let ni = fs.read_inode(nested_id).unwrap();
    let nested = fs.read_data(nested_id, 0, ni.size).unwrap();
    assert_eq!(nested.len(), 2 * BLOCK_SIZE as usize);
    assert!(nested.iter().enumerate().all(|(i, &b)| b == ((i * 7 + 3) & 0xFF) as u8));

    // Inline attribute.
    assert_eq!(
        fs.get_attribute(hello_id, "mood").unwrap(),
        Some(AttributeValue::String("archival".into()))
    );
    // Spilled attribute (re-spilled on the new volume) — and it re-indexed:
    // the query engine finds it on the MIGRATED volume.
    match fs.get_attribute(hello_id, "embedding").unwrap() {
        Some(AttributeValue::Vector(v)) => {
            assert_eq!(v.len(), 100);
            assert_eq!(v[2], 1.0);
        }
        other => panic!("embedding lost in migration: {other:?}"),
    }
    let hits = fs.query("mood == \"archival\"").unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].0.id, hello_id);

    // The migrated volume is a first-class K8 citizen: consistent and CoW.
    assert!(fs.fsck(false).unwrap().is_clean());
    let g = fs.root_generation();
    fs.write_data(hello_id, 0, b"post-migration write").unwrap();
    assert!(fs.root_generation() > g);
}
