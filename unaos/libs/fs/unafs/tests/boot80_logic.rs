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

//! BOOT80 (rmbp-ledger B350): the commit writes only the refcount-map leaves a transaction changed,
//! and the 512 B adapter moves a 4 KiB block in ONE device call.
//!
//! Boot 21 spent ~60 s in 54 UnaFS transactions on the rMBP card because every commit rewrote the
//! whole refcount map (128 leaves on a 512 MiB volume) one sector per command. These KATs pin the
//! replacement: a commit's map writes are bounded by the leaves its counts touched, the volume stays
//! fsck-clean and remount-identical through creates, attribute writes, unlinks and a snapshot cycle,
//! and the adapter's block I/O is one `read_sectors`/`write_sectors` call.

use std::collections::BTreeMap;
use unafs::{
    AttributeValue, BLOCK_SIZE, BatchFile, BlockAdapter, BlockDevice, MemDevice, MemSectorDevice, SectorDevice,
    SectorError, UnaFS,
};

fn fresh_fs(size_mb: u64) -> UnaFS<MemDevice> {
    let block_count = size_mb * 1024 * 1024 / BLOCK_SIZE;
    let mut device = MemDevice::new();
    device.write_block(block_count - 1, &vec![0u8; BLOCK_SIZE as usize]).unwrap();
    UnaFS::format(device, size_mb).unwrap()
}

/// 64 MiB = 16384 blocks = 16 refmap leaves: before BOOT80 every commit wrote all 16.
const MB: u64 = 64;
const LEAVES: u64 = 16;

#[test]
fn a_commit_writes_only_the_leaves_it_changed() {
    let mut fs = fresh_fs(MB);
    let root = fs.resolve_path("/").unwrap();
    let dir = fs.mkdir(root, "types".into()).unwrap();
    let mut worst = 0u64;
    for i in 0..40u32 {
        let id = fs.create_file(dir, format!("t{i}")).unwrap();
        worst = worst.max(fs.commit_stats().last_commit_blocks);
        fs.set_attribute(id, "una:opener".into(), AttributeValue::String(format!("op{i}"))).unwrap();
        worst = worst.max(fs.commit_stats().last_commit_blocks);
        fs.write_data(id, 0, &vec![i as u8; 5000]).unwrap();
        worst = worst.max(fs.commit_stats().last_commit_blocks);
    }
    // A small transaction touches a handful of leaves; the whole map (16) plus its data never.
    assert!(worst < LEAVES, "worst commit wrote {worst} blocks — the whole map is {LEAVES} leaves");
    assert!(fs.fsck(false).unwrap().is_clean());
    let device = fs.device.clone();
    let mut fs2 = UnaFS::mount(device).unwrap();
    assert_eq!(fs2.free_blocks(), fs.free_blocks(), "the persisted map is the in-RAM map");
    let t7 = fs2.resolve_path("/types/t7").unwrap();
    assert_eq!(fs2.read_data(t7, 0, 5000).unwrap(), vec![7u8; 5000]);
    let rep = fs2.fsck(true).unwrap();
    assert_eq!(rep.reclaimed_blocks, 0, "repair finds nothing to reclaim");
    assert!(fs2.fsck(false).unwrap().is_clean());
}

#[test]
fn unlinks_and_a_snapshot_cycle_stay_clean() {
    let mut fs = fresh_fs(MB);
    let root = fs.resolve_path("/").unwrap();
    let mut ids = Vec::new();
    for i in 0..24u32 {
        let id = fs.create_file(root, format!("f{i}")).unwrap();
        fs.write_data(id, 0, &vec![0x5A; 9000 + i as usize * 100]).unwrap();
        ids.push(id);
    }
    let snap_gen = fs.snapshot_create("boot80".into(), "kat".into(), 1).unwrap();
    for i in (0..24u32).step_by(2) {
        fs.unlink(root, &format!("f{i}")).unwrap();
    }
    fs.write_data(ids[1], 0, &vec![0xC3; 20000]).unwrap();
    assert!(fs.fsck(false).unwrap().is_clean());
    fs.snapshot_drop(snap_gen).unwrap();
    assert!(fs.fsck(false).unwrap().is_clean());
    let free = fs.free_blocks();
    let device = fs.device.clone();
    let mut fs2 = UnaFS::mount(device).unwrap();
    assert_eq!(fs2.free_blocks(), free);
    assert!(fs2.resolve_path("/f0").is_err());
    let f1 = fs2.resolve_path("/f1").unwrap();
    assert_eq!(fs2.read_data(f1, 0, 20000).unwrap(), vec![0xC3; 20000]);
    assert_eq!(fs2.fsck(true).unwrap().reclaimed_blocks, 0);
}

#[test]
fn a_batch_of_thirteen_types_is_one_commit() {
    // The kernel's type-database seed (assoc) rides this: 13 objects with 3 attributes each.
    let mut fs = fresh_fs(MB);
    let root = fs.resolve_path("/").unwrap();
    let dir = fs.mkdir(root, "types".into()).unwrap();
    let before = fs.commit_stats().commits;
    let files: Vec<BatchFile> = (0..13)
        .map(|i| {
            let mut attributes = BTreeMap::new();
            for k in ["una:opener", "una:icon", "una:name"] {
                attributes.insert(k.to_string(), AttributeValue::String(format!("{k}-{i}")));
            }
            BatchFile { name: format!("type{i}"), data: Vec::new(), attributes }
        })
        .collect();
    let w0 = fs.commit_stats().blocks_written;
    fs.create_files_batch(dir, files).unwrap();
    assert_eq!(fs.commit_stats().commits - before, 1);
    let batch_blocks = fs.commit_stats().blocks_written - w0;
    assert!(fs.fsck(false).unwrap().is_clean());
    // The same 13 objects the per-op way (create + 3 set_attribute each = 52 commits).
    let mut per = fresh_fs(MB);
    let proot = per.resolve_path("/").unwrap();
    let pdir = per.mkdir(proot, "types".into()).unwrap();
    let (c0, p0) = (per.commit_stats().commits, per.commit_stats().blocks_written);
    for i in 0..13 {
        let id = per.create_file(pdir, format!("type{i}")).unwrap();
        for k in ["una:opener", "una:icon", "una:name"] {
            per.set_attribute(id, k.to_string(), AttributeValue::String(format!("{k}-{i}"))).unwrap();
        }
    }
    let (per_commits, per_blocks) = (per.commit_stats().commits - c0, per.commit_stats().blocks_written - p0);
    assert_eq!(per_commits, 52);
    assert!(batch_blocks * 4 < per_blocks, "batch wrote {batch_blocks} blocks, per-op {per_blocks}");
    // And the per-op path no longer pays the whole map per commit (16 leaves x 52 = 832 before BOOT80).
    assert!(per_blocks < 52 * LEAVES, "per-op wrote {per_blocks} blocks over 52 commits");
    assert!(per.fsck(false).unwrap().is_clean());
}

/// Counts device calls; serves from an inner `MemSectorDevice`.
struct Counting {
    inner: MemSectorDevice,
    single: u32,
    multi: u32,
    override_multi: bool,
}

impl SectorDevice for Counting {
    fn read_sector(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), SectorError> {
        self.single += 1;
        self.inner.read_sector(lba, buf)
    }
    fn write_sector(&mut self, lba: u64, buf: &[u8]) -> Result<(), SectorError> {
        self.single += 1;
        self.inner.write_sector(lba, buf)
    }
    fn sector_count(&self) -> u64 {
        self.inner.sector_count()
    }
    fn read_sectors(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), SectorError> {
        if !self.override_multi {
            for (i, c) in buf.chunks_mut(512).enumerate() {
                self.read_sector(lba + i as u64, c)?;
            }
            return Ok(());
        }
        self.multi += 1;
        for (i, c) in buf.chunks_mut(512).enumerate() {
            self.inner.read_sector(lba + i as u64, c)?;
        }
        Ok(())
    }
    fn write_sectors(&mut self, lba: u64, buf: &[u8]) -> Result<(), SectorError> {
        if !self.override_multi {
            for (i, c) in buf.chunks(512).enumerate() {
                self.write_sector(lba + i as u64, c)?;
            }
            return Ok(());
        }
        self.multi += 1;
        for (i, c) in buf.chunks(512).enumerate() {
            self.inner.write_sector(lba + i as u64, c)?;
        }
        Ok(())
    }
}

#[test]
fn the_adapter_moves_a_block_in_one_call() {
    for multi in [true, false] {
        let dev = Counting { inner: MemSectorDevice::with_sectors(64 + 8 * 16), single: 0, multi: 0, override_multi: multi };
        let mut a = BlockAdapter::new(dev, 64, 16);
        let blk: Vec<u8> = (0..BLOCK_SIZE as usize).map(|i| (i * 7) as u8).collect();
        a.write_block(3, &blk).unwrap();
        let mut back = vec![0u8; BLOCK_SIZE as usize];
        a.read_block(3, &mut back).unwrap();
        assert_eq!(back, blk);
        let d = a.into_inner();
        // The bytes landed at the partition offset, block 3 = sectors 64 + 24 ..
        assert_eq!(&d.inner.as_bytes()[(64 + 24) * 512..(64 + 32) * 512], &blk[..]);
        if multi {
            assert_eq!((d.multi, d.single), (2, 0), "one call per block, each way");
        } else {
            assert_eq!((d.multi, d.single), (0, 16), "the default is the per-sector loop");
        }
    }
}
