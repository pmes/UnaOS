// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.

//! Known-answer / fixture tests for the BeFS-K2 block adapter.
//!
//! The GPT and MBR fixtures are hand-built with fully specified byte offsets, so
//! these tests pin the parser against a synthetic, deterministic on-disk layout
//! (the M2 KAT contract). The 512↔4096 mapping is exercised directly through the
//! public `BlockAdapter` surface.

use unafs::adapter::{
    parse_partitions, BlockAdapter, MemSectorDevice, PartError, PartitionScheme, SectorDevice,
    SECTOR_SIZE,
};
use unafs::{locate_unafs, superblock, BlockDevice};

const SECTOR: usize = SECTOR_SIZE as usize;

fn put_u32(dev: &mut MemSectorDevice, off: usize, v: u32) {
    dev.as_mut_bytes()[off..off + 4].copy_from_slice(&v.to_le_bytes());
}
fn put_u64(dev: &mut MemSectorDevice, off: usize, v: u64) {
    dev.as_mut_bytes()[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

// ---- MBR fixture ----------------------------------------------------------

/// A 4096-sector device with a single MBR primary: type 0x83, start 2048,
/// length 2048 sectors.
fn mbr_fixture() -> MemSectorDevice {
    let mut dev = MemSectorDevice::with_sectors(4096);
    let e0 = 446; // first primary entry
    dev.as_mut_bytes()[e0 + 4] = 0x83; // type: Linux
    put_u32(&mut dev, e0 + 8, 2048); // start LBA
    put_u32(&mut dev, e0 + 12, 2048); // sector count
    // boot signature
    dev.as_mut_bytes()[510] = 0x55;
    dev.as_mut_bytes()[511] = 0xAA;
    dev
}

#[test]
fn mbr_parse_single_primary() {
    let mut dev = mbr_fixture();
    let table = parse_partitions(&mut dev).unwrap();
    assert_eq!(table.scheme, PartitionScheme::Mbr);
    assert_eq!(table.partitions.len(), 1);
    let p = &table.partitions[0];
    assert_eq!(p.start_lba, 2048);
    assert_eq!(p.sector_count, 2048);
    assert_eq!(p.type_byte, Some(0x83));
    assert_eq!(p.type_guid, None);
}

#[test]
fn no_signature_rejected() {
    let mut dev = MemSectorDevice::with_sectors(64); // all zeros, no 0x55AA
    assert_eq!(parse_partitions(&mut dev), Err(PartError::NoSignature));
}

#[test]
fn mbr_partition_past_device_rejected() {
    let mut dev = MemSectorDevice::with_sectors(256); // small device
    let e0 = 446;
    dev.as_mut_bytes()[e0 + 4] = 0x83;
    put_u32(&mut dev, e0 + 8, 128);
    put_u32(&mut dev, e0 + 12, 4096); // 128 + 4096 > 256 sectors
    dev.as_mut_bytes()[510] = 0x55;
    dev.as_mut_bytes()[511] = 0xAA;
    assert_eq!(parse_partitions(&mut dev), Err(PartError::OutOfBounds(128)));
}

// ---- GPT fixture ----------------------------------------------------------

/// A 4096-sector device with a protective MBR + a GPT declaring one partition
/// (first LBA 34, last LBA 2081 => 2048 sectors), entry array at LBA 2.
fn gpt_fixture() -> MemSectorDevice {
    let mut dev = MemSectorDevice::with_sectors(4096);

    // Protective MBR: one 0xEE entry + boot signature.
    let e0 = 446;
    dev.as_mut_bytes()[e0 + 4] = 0xEE;
    put_u32(&mut dev, e0 + 8, 1);
    put_u32(&mut dev, e0 + 12, 0xFFFF_FFFF);
    dev.as_mut_bytes()[510] = 0x55;
    dev.as_mut_bytes()[511] = 0xAA;

    // GPT header at LBA 1.
    let h = SECTOR;
    dev.as_mut_bytes()[h..h + 8].copy_from_slice(b"EFI PART");
    put_u64(&mut dev, h + 72, 2); // partition entry array LBA
    put_u32(&mut dev, h + 80, 128); // number of entries
    put_u32(&mut dev, h + 84, 128); // entry size

    // Entry 0 at LBA 2 (byte 1024): nonzero type GUID, first=34, last=2081.
    let ent = 2 * SECTOR;
    for (i, b) in dev.as_mut_bytes()[ent..ent + 16].iter_mut().enumerate() {
        *b = (i as u8) + 1; // any nonzero type GUID
    }
    put_u64(&mut dev, ent + 32, 34); // first LBA
    put_u64(&mut dev, ent + 40, 2081); // last LBA (inclusive) => 2048 sectors
    // entries 1..128 remain zero (unused)
    dev
}

#[test]
fn gpt_parse_single_entry() {
    let mut dev = gpt_fixture();
    let table = parse_partitions(&mut dev).unwrap();
    assert_eq!(table.scheme, PartitionScheme::Gpt);
    assert_eq!(table.partitions.len(), 1);
    let p = &table.partitions[0];
    assert_eq!(p.start_lba, 34);
    assert_eq!(p.sector_count, 2048);
    assert_eq!(p.type_byte, None);
    assert!(p.type_guid.is_some());
}

#[test]
fn gpt_bad_signature_rejected() {
    let mut dev = gpt_fixture();
    // Corrupt the "EFI PART" signature at LBA 1.
    dev.as_mut_bytes()[SECTOR] = b'X';
    assert_eq!(parse_partitions(&mut dev), Err(PartError::BadGptSignature));
}

#[test]
fn gpt_entry_past_device_rejected() {
    let mut dev = gpt_fixture();
    // Push last LBA beyond the 4096-sector device.
    put_u64(&mut dev, 2 * SECTOR + 40, 999_999);
    assert_eq!(parse_partitions(&mut dev), Err(PartError::OutOfBounds(34)));
}

#[test]
fn gpt_bad_entry_size_rejected() {
    let mut dev = gpt_fixture();
    put_u32(&mut dev, SECTOR + 84, 64); // entry size < 128
    assert!(matches!(
        parse_partitions(&mut dev),
        Err(PartError::Malformed(_))
    ));
}

// ---- locate_unafs (magic probe) -------------------------------------------

#[test]
fn locate_finds_unafs_by_superblock_magic() {
    let mut dev = gpt_fixture(); // partition at LBA 34, 2048 sectors
                                 // Stamp the UnaFS magic at the partition's block 0 (byte 34*512).
    let base = 34 * SECTOR;
    dev.as_mut_bytes()[base..base + superblock::MAGIC.len()]
        .copy_from_slice(&superblock::MAGIC);

    let span = locate_unafs(&mut dev).unwrap().expect("should locate");
    assert_eq!(span.base_lba, 34);
    assert_eq!(span.block_count, 2048 / 8); // 256

    // The located span drives a working adapter positioned on the partition.
    let mut ad = BlockAdapter::for_partition(dev, &span);
    let mut buf = vec![0u8; unafs::BLOCK_SIZE as usize];
    ad.read_block(0, &mut buf).unwrap();
    assert_eq!(&buf[0..superblock::MAGIC.len()], &superblock::MAGIC);
}

#[test]
fn locate_returns_none_without_magic() {
    let mut dev = gpt_fixture(); // valid partition, but no superblock magic
    assert_eq!(locate_unafs(&mut dev).unwrap(), None);
}

#[test]
fn sector_device_count_reports_whole_sectors() {
    let dev = MemSectorDevice::with_sectors(100);
    assert_eq!(dev.sector_count(), 100);
}
