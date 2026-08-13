// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! Known-Answer Tests (KATs) — the on-disk byte-layout contract for UnaFS,
//! version 3 (the K8 copy-on-write format).
//!
//! Every struct that reaches disk is serialized here with representative and
//! boundary values and asserted BYTE-FOR-BYTE against golden vectors baked in
//! as hex literals. The K8 arc lifted the pre-K8 format freeze (design pass,
//! 2026-07-16) and pinned NEW goldens for the structures the CoW format
//! changed or added; the record encodings the format KEPT (Inode, Extent,
//! AttributeValue, FileKind, CatalogEntry, DirEntry) retain their ORIGINAL
//! bincode-1.3.3-frozen golden bytes — proof the K8 migration touched only
//! what it claimed to.
//!
//! On-disk structs covered:
//!   * Superblock (v3)       (block 0; static identity)
//!   * RootRecord            (block 1, A/B 512 B slots; HAND-PACKED layout —
//!                            the root-fits-one-sector KAT lives here)
//!   * SnapshotEntry list    (the snapshot-index object's data)
//!   * ReclaimEntry list     (the reclaim-queue object's data)
//!   * FileKind   (enum)     (nested in Inode / DirEntry)      [unchanged]
//!   * Extent                (nested in Inode)                 [unchanged]
//!   * AttributeValue (enum) (nested in Inode; spilled values) [unchanged]
//!   * Inode                 (inode blocks)                    [unchanged]
//!   * CatalogEntry + list   (catalog.rs serialize_catalog)    [unchanged]
//!   * DirEntry   + list     (fs.rs directory data)            [unchanged]
//!
//! The inode-map and refcount-map leaves reach disk as RAW little-endian
//! u64/u32 arrays (not bincode) and are covered by the recovery suite's
//! remount round-trips.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use unafs::catalog::{CatalogEntry, serialize_catalog};
use unafs::inode::{AttributeValue, Extent, FileKind, Inode};
use unafs::root::{ROOT_RECORD_SIZE, ROOT_SECTOR_SIZE};
use unafs::superblock::Superblock;
use unafs::{DirEntry, ReclaimEntry, RootRecord, SnapshotEntry};

// ---- codec seam -----------------------------------------------------------
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

// =============================================================================
// NEW K8 (v3) structures
// =============================================================================

#[test]
fn kat_superblock_v5() {
    // v5 = the second-refmap-level format bump (volumes past 2 GiB). Exactly
    // like the v3 → v4 recut, the ONLY byte that moves from the previous
    // golden is the version field (04 → 05): the superblock layout is
    // otherwise identical, and the bump is the incompat marker that stops a
    // pre-v5 reader misreading a two-level refmap index. (A v5 volume of
    // ≤ 2 GiB keeps the single-level index, so its every OTHER byte matches
    // what v4 would have written.)
    let sb = Superblock::new(4096);
    kat(
        &sb,
        "554e4146530500000000100000001000000000000001000000000000000200000000000000",
    );
    // The real disk write path (to_bytes) must equal the golden too.
    assert_eq!(sb.to_bytes().unwrap(), enc(&sb));

    // Boundary flavor: the largest representable volume.
    let sb_b = Superblock::new(u64::MAX / 4096);
    kat(
        &sb_b,
        "554e4146530500000000100000ffffffffffff0f0001000000000000000200000000000000",
    );
}

/// THE root-fits-one-sector KAT (brief §1): the packed root record is a
/// fixed-size layout that must fit ONE 512 B sector — asserted here on the
/// real bytes (the compile-time `const` assert in `root.rs` covers the
/// constant; this covers the encoder).
#[test]
fn kat_root_record_fits_one_sector() {
    let rr = RootRecord {
        generation: 7,
        imap_block: 12,
        imap_leaves: 1,
        next_inode: 5,
        refmap_block: 14,
        refmap_leaves: 1,
        free_blocks: 4000,
        flags: 0,
    };
    let bytes = rr.to_bytes();
    assert_eq!(bytes.len(), ROOT_RECORD_SIZE);
    assert!(
        bytes.len() <= ROOT_SECTOR_SIZE,
        "root record must fit one 512 B sector"
    );
    // Golden vector: magic + 8 LE u64 fields + FNV-1a checksum.
    assert_eq!(
        bytes.to_vec(),
        h("554e41465352543107000000000000000c000000000000000100000000000000\
           05000000000000000e000000000000000100000000000000a00f000000000000\
           00000000000000004e03df1b00fa8d69")
    );

    // Roundtrip through the sector parser (with sector padding).
    let mut sector = vec![0u8; ROOT_SECTOR_SIZE];
    sector[..ROOT_RECORD_SIZE].copy_from_slice(&bytes);
    let back = RootRecord::from_sector(&sector).expect("valid record parses");
    assert_eq!(back, rr);

    // A flipped byte breaks the checksum → the slot reads as absent/torn.
    let mut torn = sector.clone();
    torn[20] ^= 0xFF;
    assert!(RootRecord::from_sector(&torn).is_none());
    // Generation 0 is never valid (the zeroed-slot signature).
    let zeroed = vec![0u8; ROOT_SECTOR_SIZE];
    assert!(RootRecord::from_sector(&zeroed).is_none());
}

#[test]
fn kat_snapshot_entry() {
    let se = SnapshotEntry {
        generation: 9,
        imap_block: 77,
        imap_leaves: 1,
        name: "alpha".to_string(),
        creator: "root".to_string(),
        timestamp: 1234567890,
    };
    let list = vec![se];
    assert_eq!(
        enc(&list),
        h("010000000000000009000000000000004d000000000000000100000000000000\
           0500000000000000616c7068610400000000000000726f6f74d2029649000000\
           00")
    );
    let back: Vec<SnapshotEntry> = dec(&enc(&list));
    assert_eq!(back, list);
    // The freshly formatted snapshot index: an EMPTY list (bare zero count).
    let empty: Vec<SnapshotEntry> = Vec::new();
    assert_eq!(enc(&empty), h("0000000000000000"));
}

#[test]
fn kat_reclaim_entry() {
    let re = ReclaimEntry {
        generation: 9,
        blocks: vec![10, 11, 12],
    };
    let list = vec![re];
    assert_eq!(
        enc(&list),
        h("0100000000000000090000000000000003000000000000000a00000000000000\
           0b000000000000000c00000000000000")
    );
    let back: Vec<ReclaimEntry> = dec(&enc(&list));
    assert_eq!(back, list);
    // The freshly formatted reclaim queue: an EMPTY list.
    let empty: Vec<ReclaimEntry> = Vec::new();
    assert_eq!(enc(&empty), h("0000000000000000"));
}

// =============================================================================
// KEPT structures — original frozen goldens, byte-identical across K8
// =============================================================================

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

    // Empty directory: an empty Vec<DirEntry> is a bare u64 length prefix of 0.
    let empty: Vec<DirEntry> = Vec::new();
    assert_eq!(enc(&empty), h("0000000000000000"));
    let decoded: Vec<DirEntry> = dec(&h("0000000000000000"));
    assert_eq!(decoded, empty);
}
