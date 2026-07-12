// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! Known-Answer Tests (KATs) — the on-disk byte-layout contract for UnaFS.
//!
//! Every struct that reaches disk is serialized here with representative and
//! boundary values and asserted BYTE-FOR-BYTE against golden vectors baked in
//! as hex literals. These vectors were generated from the reference bincode
//! 1.3.3 encoding (little-endian, fixed-int, no length limit) and MUST NOT
//! change: the on-disk format is frozen. When the crate migrates to bincode
//! 2.x, only the `enc`/`dec` seam below flips — every golden constant stays
//! identical, which is exactly the proof the migration preserves the format.
//!
//! On-disk structs covered (all bincode-serialized paths in the crate):
//!   * Superblock            (block 0; superblock.rs to_bytes)
//!   * FileKind   (enum)     (nested in Inode / DirEntry)
//!   * Extent                (nested in Inode.chunks / large_attributes)
//!   * AttributeValue (enum) (nested in Inode; standalone for large_attributes)
//!   * Inode                 (inode blocks; inode.rs to_bytes)
//!   * CatalogEntry + list   (catalog.rs serialize_catalog)
//!   * JournalOp  (enum)     (wal.rs journal log)
//!   * DirEntry   + list     (fs.rs directory data)
//!
//! Bitmap/SpaceMap reaches disk as RAW bytes (not bincode) and so is out of
//! scope for the codec KATs.

use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};
use unafs::catalog::{serialize_catalog, CatalogEntry};
use unafs::inode::{AttributeValue, Extent, FileKind, Inode};
use unafs::superblock::Superblock;
use unafs::wal::JournalOp;
use unafs::DirEntry;

// ---- migration seam -------------------------------------------------------
// This routes through the crate's codec (bincode 2.x, legacy() config). The
// golden constants below were frozen from the reference bincode 1.3.3 encoding
// and MUST remain byte-identical: that equality is the proof the bincode 2.x
// migration preserves the on-disk format.
fn enc<T: Serialize + ?Sized>(v: &T) -> Vec<u8> {
    unafs::codec::serialize(v).expect("serialize")
}
fn dec<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> T {
    unafs::codec::deserialize(bytes).expect("deserialize")
}

// ---- hex helper -----------------------------------------------------------
fn h(s: &str) -> Vec<u8> {
    assert!(s.len() % 2 == 0, "odd hex length");
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).expect("hex"))
        .collect()
}

/// Assert forward (serialize == golden) AND roundtrip (deserialize(golden) == value).
fn kat<T>(value: &T, golden_hex: &str)
where
    T: Serialize + for<'de> Deserialize<'de> + PartialEq + std::fmt::Debug,
{
    let golden = h(golden_hex);
    let actual = enc(value);
    assert_eq!(
        actual, golden,
        "FORWARD KAT drift: serialized bytes differ from frozen golden vector"
    );
    let decoded: T = dec(&golden);
    assert_eq!(
        &decoded, value,
        "ROUNDTRIP KAT drift: deserialized golden vector differs from value"
    );
}

#[test]
fn kat_superblock() {
    let mut sb = Superblock::new(4096);
    sb.root_inode = 12;
    sb.catalog_inode = 13;
    kat(
        &sb,
        "554e414653020000000010000000100000000000000c00000000000000f40f0000000000000b00000000000000010000000000000001000000000000000a000000000000000d00000000000000",
    );
    // The real disk write path (to_bytes) must equal the golden too.
    assert_eq!(sb.to_bytes().unwrap(), enc(&sb));

    let sb_b = Superblock {
        magic: *b"UNAFS",
        version: 2,
        block_size: 4096,
        block_count: u64::MAX,
        root_inode: u64::MAX,
        free_blocks: 0,
        bitmap_start: u64::MAX,
        bitmap_blocks: u64::MAX,
        journal_start: u64::MAX,
        journal_blocks: u64::MAX,
        catalog_inode: u64::MAX,
    };
    kat(
        &sb_b,
        "554e4146530200000000100000ffffffffffffffffffffffffffffffff0000000000000000ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff",
    );
}

#[test]
fn kat_filekind() {
    kat(&FileKind::File, "00000000");
    kat(&FileKind::Directory, "01000000");
    kat(&FileKind::Symlink, "02000000");
    kat(&FileKind::System, "03000000");
}

#[test]
fn kat_extent() {
    kat(
        &Extent { logical_offset: 0, physical_block: 14, length: 4096 },
        "00000000000000000e000000000000000010000000000000",
    );
    kat(
        &Extent { logical_offset: u64::MAX, physical_block: u64::MAX, length: u64::MAX },
        "ffffffffffffffffffffffffffffffffffffffffffffffff",
    );
}

#[test]
fn kat_attribute_value() {
    kat(&AttributeValue::Int(-42), "00000000d6ffffffffffffff");
    kat(&AttributeValue::Int(i64::MIN), "000000000000000000000080");
    kat(&AttributeValue::Float(1.5), "01000000000000000000f83f");
    kat(&AttributeValue::String("hello".to_string()), "02000000050000000000000068656c6c6f");
    kat(&AttributeValue::String(String::new()), "020000000000000000000000");
    kat(&AttributeValue::Blob(vec![0xde, 0xad, 0xbe, 0xef]), "030000000400000000000000deadbeef");
    kat(&AttributeValue::Blob(vec![]), "030000000000000000000000");
    kat(&AttributeValue::Vector(vec![0.1f32, 0.2, 0.9]), "040000000300000000000000cdcccc3dcdcc4c3e6666663f");
}

#[test]
fn kat_inode() {
    let inode_empty = Inode::new(101, FileKind::File);
    kat(
        &inode_empty,
        "6500000000000000000000000000000000000000000000000000000000000000000000000000000000000000",
    );
    assert_eq!(inode_empty.to_bytes().unwrap(), enc(&inode_empty));

    let mut inode = Inode::new(7, FileKind::Directory);
    inode.size = 8192;
    inode.chunks = vec![
        Extent { logical_offset: 0, physical_block: 20, length: 4096 },
        Extent { logical_offset: 4096, physical_block: 21, length: 4096 },
    ];
    inode.attributes.insert("emotion".to_string(), AttributeValue::String("calm".to_string()));
    inode.attributes.insert("count".to_string(), AttributeValue::Int(3));
    let mut large: BTreeMap<String, Vec<Extent>> = BTreeMap::new();
    large.insert(
        "embedding".to_string(),
        vec![Extent { logical_offset: 0, physical_block: 30, length: 256 }],
    );
    inode.large_attributes = large;
    kat(
        &inode,
        "0700000000000000010000000020000000000000020000000000000000000000000000001400000000000000001000000000000000100000000000001500000000000000001000000000000002000000000000000500000000000000636f756e740000000003000000000000000700000000000000656d6f74696f6e02000000040000000000000063616c6d01000000000000000900000000000000656d62656464696e67010000000000000000000000000000001e000000000000000001000000000000",
    );
    assert_eq!(inode.to_bytes().unwrap(), enc(&inode));
}

#[test]
fn kat_catalog() {
    let ce = CatalogEntry { key_hash: 0x0123456789abcdef, val_hash: 0xfedcba9876543210, inode_id: 42 };
    kat(&ce, "efcdab89674523011032547698badcfe2a00000000000000");

    // serialize_catalog is the real on-disk path for the attribute catalog.
    let ce2 = CatalogEntry { key_hash: 1, val_hash: 2, inode_id: 3 };
    let list = vec![ce, ce2];
    assert_eq!(
        serialize_catalog(&list).unwrap(),
        h("0200000000000000efcdab89674523011032547698badcfe2a00000000000000010000000000000002000000000000000300000000000000"),
    );
    // Also assert plain Vec encoding matches serialize_catalog (they must agree).
    assert_eq!(serialize_catalog(&list).unwrap(), enc(&list));
    // Empty catalog.
    assert_eq!(serialize_catalog(&[]).unwrap(), h("0000000000000000"));
}

#[test]
fn kat_journal_op() {
    kat(
        &JournalOp::BeginOp { op_id: 5, desc: "SetAttr: x".to_string() },
        "0000000005000000000000000a00000000000000536574417474723a2078",
    );
    kat(&JournalOp::EndOp { op_id: 5 }, "010000000500000000000000");
    kat(
        &JournalOp::BeginCreate { parent_inode: 0, name: "unknown".to_string() },
        "0200000000000000000000000700000000000000756e6b6e6f776e",
    );
    kat(&JournalOp::EndCreate { inode_id: 14 }, "030000000e00000000000000");
    kat(&JournalOp::BeginWrite { inode_id: 14 }, "040000000e00000000000000");
    kat(&JournalOp::EndWrite { inode_id: 14 }, "050000000e00000000000000");
}

#[test]
fn kat_direntry() {
    let de = DirEntry { name: "manifesto.txt".to_string(), inode_id: 14, kind: FileKind::File };
    kat(&de, "0d000000000000006d616e69666573746f2e7478740e0000000000000000000000");

    // Directories are stored as a bincode Vec<DirEntry>.
    let de2 = DirEntry { name: "sub".to_string(), inode_id: 15, kind: FileKind::Directory };
    let list = vec![de, de2];
    assert_eq!(
        enc(&list),
        h("02000000000000000d000000000000006d616e69666573746f2e7478740e000000000000000000000003000000000000007375620f0000000000000001000000"),
    );
}
