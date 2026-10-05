// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! AMBER1 (SR34): `unafs::format` — the one format entry `tools/unafs init`, Amber Bytes and the
//! installer share. KATs: byte-identical to `UnaFS::format_with_version` under a pinned clock; the
//! volume mounts, is generation 1 and fsck-clean; a volume narrower than its device records the
//! volume and leaves the rest untouched; a partition format at an LBA offset touches nothing outside
//! the partition and mounts through `BlockAdapter`; every refusal writes nothing.

use unafs::adapter::{BlockAdapter, MemSectorDevice, SectorDevice};
use unafs::format::{MIN_BLOCKS, volume_blocks};
use unafs::superblock::{MIN_SUPPORTED_VERSION, VERSION};
use unafs::{BLOCK_SIZE, BlockDevice, FormatParams, MemDevice, Superblock, UnaFS};

const EPOCH: u64 = 1_790_000_000;
fn pinned() -> u64 {
    EPOCH
}

fn device(blocks: u64) -> MemDevice {
    let mut d = MemDevice::new();
    d.write_block(blocks - 1, &vec![0u8; BLOCK_SIZE as usize]).unwrap();
    d
}

fn dump(d: &mut impl BlockDevice) -> Vec<u8> {
    let mut out = Vec::new();
    let mut b = vec![0u8; BLOCK_SIZE as usize];
    for i in 0..d.block_count() {
        d.read_block(i, &mut b).unwrap();
        out.extend_from_slice(&b);
    }
    out
}

#[test]
fn format_is_the_reference_format_byte_for_byte() {
    unafs::clock::set_clock_hook(pinned);
    for version in MIN_SUPPORTED_VERSION..=VERSION {
        let mut a = device(2048);
        let rep = unafs::format(&mut a, &FormatParams { version, blocks: None }).unwrap();
        assert_eq!((rep.version, rep.blocks, rep.generation), (version, 2048, 1));
        let reference = UnaFS::format_with_version(device(2048), 8, version).unwrap();
        let mut r = MemDevice::new();
        // Copy the reference volume out through its device.
        let mut fsdev = reference;
        let mut buf = vec![0u8; BLOCK_SIZE as usize];
        for i in 0..2048 {
            fsdev.device.read_block(i, &mut buf).unwrap();
            r.write_block(i, &buf).unwrap();
        }
        assert_eq!(dump(&mut a), dump(&mut r), "v{version}: format differs from format_with_version");
    }
}

#[test]
fn formatted_volume_mounts_clean_at_generation_one() {
    unafs::clock::set_clock_hook(pinned);
    let mut d = device(4096);
    let rep = unafs::format(&mut d, &FormatParams::default()).unwrap();
    let mut fs = UnaFS::mount(d).unwrap();
    assert_eq!(fs.root_generation(), 1);
    assert_eq!(fs.free_blocks(), rep.free_blocks);
    let f = fs.fsck(false).unwrap();
    assert!(f.is_clean(), "{f:?}");
    assert!(fs.ls(unafs::superblock::ROOT_INODE_ID).unwrap().is_empty());
}

#[test]
fn narrower_volume_records_itself_and_leaves_the_rest() {
    unafs::clock::set_clock_hook(pinned);
    let mut d = device(4096);
    let mut junk = vec![0xA5u8; BLOCK_SIZE as usize];
    junk[0] = 1;
    d.write_block(3000, &junk).unwrap();
    let rep = unafs::format(&mut d, &FormatParams::sized_mb(4)).unwrap();
    assert_eq!(rep.blocks, 1024);
    let mut b0 = vec![0u8; BLOCK_SIZE as usize];
    d.read_block(0, &mut b0).unwrap();
    assert_eq!(Superblock::from_bytes(&b0).unwrap().block_count, 1024);
    let mut b = vec![0u8; BLOCK_SIZE as usize];
    d.read_block(3000, &mut b).unwrap();
    assert_eq!(b, junk, "a block past the volume was touched");
    assert_eq!(d.block_count(), 4096);
}

#[test]
fn partition_format_stays_inside_the_partition() {
    unafs::clock::set_clock_hook(pinned);
    // 16 MiB medium; partition at LBA 2048, 8 MiB + 5 sectors (the tail is not a whole block).
    let total = 32768u64;
    let mut dev = MemSectorDevice::with_sectors(total as usize);
    for b in dev.as_mut_bytes().iter_mut() {
        *b = 0xEE;
    }
    let (base, len) = (2048u64, 16384u64 + 5);
    let rep = unafs::format_partition(&mut dev, base, len, &FormatParams::default()).unwrap();
    assert_eq!(rep.blocks, 2048);
    let bytes = dev.as_bytes();
    let used_end = (base + rep.blocks * 8) as usize * 512;
    assert!(bytes[..base as usize * 512].iter().all(|&x| x == 0xEE), "wrote before the partition");
    assert!(bytes[used_end..].iter().all(|&x| x == 0xEE), "wrote after the volume");
    let sb = Superblock::from_bytes(&bytes[base as usize * 512..base as usize * 512 + BLOCK_SIZE as usize]).unwrap();
    assert_eq!(sb.block_count, 2048);
    let adapter = BlockAdapter::new(&mut dev, base, rep.blocks);
    let mut fs = UnaFS::mount(adapter).unwrap();
    assert!(fs.fsck(false).unwrap().is_clean());
}

#[test]
fn refusals_write_nothing() {
    unafs::clock::set_clock_hook(pinned);
    let cases = [
        FormatParams { version: MIN_SUPPORTED_VERSION - 1, blocks: None },
        FormatParams { version: VERSION + 1, blocks: None },
        FormatParams { version: VERSION, blocks: Some(4097) },
        FormatParams { version: VERSION, blocks: Some(MIN_BLOCKS - 1) },
    ];
    for p in cases {
        let mut d = device(4096);
        assert!(unafs::format(&mut d, &p).is_err(), "{p:?} accepted");
        assert!(dump(&mut d).iter().all(|&x| x == 0), "{p:?} wrote before refusing");
    }
    assert!(volume_blocks(MIN_BLOCKS - 1, &FormatParams::default()).is_err());
    let mut dev = MemSectorDevice::with_sectors(4096);
    assert!(unafs::format_partition(&mut dev, 2048, 4096, &FormatParams::default()).is_err());
    assert_eq!(dev.sector_count(), 4096);
    assert!(dev.as_bytes().iter().all(|&x| x == 0));
}
