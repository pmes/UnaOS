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

//! FILEVERSIONS (rmbp-ledger B714) M2: an overwrite keeps the old contents as a version of the inode
//! (unafs-records.md §R17). Ten writes keep eight versions with their bytes; the volume bound drops the
//! oldest version anywhere; a remount keeps them; a restore round-trips; unlink releases them; and a
//! reader built before the record (the pre-B714 inode-block decode, reproduced below) opens every inode
//! block of a versioned volume whose stamp is still 7.

use unafs::inode::{INODE_SPILL_MAGIC, INODE_VERSIONS_MAGIC, IndirectTrailer};
use unafs::fs::FileSystemError;
use unafs::{BLOCK_SIZE, BlockDevice, FileKind, Inode, MemDevice, RING_MAX, ReclaimEntry, UnaFS};

fn vol(blocks: u64) -> UnaFS<MemDevice> {
    let mut fs = UnaFS::format(MemDevice::with_blocks(blocks), 0).expect("format");
    fs.set_versioning(true);
    fs
}

fn content(tag: u8, len: usize) -> Vec<u8> {
    (0..len).map(|i| tag.wrapping_mul(31).wrapping_add(i as u8)).collect()
}

fn remount(fs: UnaFS<MemDevice>) -> UnaFS<MemDevice> {
    let bytes = fs.device.as_bytes().to_vec();
    drop(fs);
    UnaFS::mount(MemDevice::from_bytes(bytes)).expect("remount")
}

#[test]
fn ten_writes_keep_eight_versions_with_their_bytes() {
    let mut fs = vol(2048);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "doc.txt".into()).unwrap();
    for i in 0..10u8 {
        fs.write_data(id, 0, &content(i, 5000)).unwrap();
    }
    // Write 0 filled an empty file (nothing replaced); writes 1..9 each kept the previous contents: nine
    // captures, the ring keeps the newest eight (contents 1..=8), newest first.
    let v = fs.versions(id).unwrap();
    assert_eq!(v.len(), RING_MAX);
    for (n, info) in v.iter().enumerate() {
        assert_eq!(info.n, n);
        assert_eq!(info.size, 5000);
        assert_eq!(fs.read_version(id, n, 0, 5000).unwrap(), content(8 - n as u8, 5000), "version {n}");
        if n > 0 {
            assert!(info.seq < v[n - 1].seq, "newest first");
        }
    }
    assert!(fs.read_version(id, RING_MAX, 0, 1).is_err(), "past the ring");
    assert_eq!(fs.read_data(id, 0, 5000).unwrap(), content(9, 5000));
    let r = fs.fsck(false).unwrap();
    assert!(r.is_clean(), "{r:?}");
    assert_eq!(r.versions, RING_MAX as u64);
    assert_eq!(r.version_blocks, fs.version_ledger().unwrap().kept());
}

#[test]
fn appends_keep_none_truncate_keeps_one() {
    let mut fs = vol(2048);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "log.txt".into()).unwrap();
    fs.write_data(id, 0, &content(1, 2 * BLOCK_SIZE as usize)).unwrap();
    fs.write_data(id, 2 * BLOCK_SIZE, &content(2, 100)).unwrap();
    assert!(fs.versions(id).unwrap().is_empty(), "an append replaces nothing");
    let whole = fs.read_data(id, 0, 2 * BLOCK_SIZE + 100).unwrap();
    fs.truncate_data(id, 10).unwrap();
    let v = fs.versions(id).unwrap();
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].size, 2 * BLOCK_SIZE + 100);
    assert_eq!(fs.read_version(id, 0, 0, v[0].size).unwrap(), whole);
    assert!(fs.fsck(false).unwrap().is_clean());
    // Off: the old extents are reclaimed at once and the ring is kept as it is.
    fs.set_versioning(false);
    fs.write_data(id, 0, b"0123456789").unwrap();
    assert_eq!(fs.versions(id).unwrap().len(), 1);
    assert!(fs.fsck(false).unwrap().is_clean());
}

#[test]
fn remount_keeps_versions() {
    let mut fs = vol(2048);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "keep.txt".into()).unwrap();
    for i in 0..3u8 {
        fs.write_data(id, 0, &content(i, 9000)).unwrap();
    }
    let mut fs = remount(fs);
    let v = fs.versions(id).unwrap();
    assert_eq!(v.len(), 2);
    assert_eq!(fs.read_version(id, 0, 0, 9000).unwrap(), content(1, 9000));
    assert_eq!(fs.read_version(id, 1, 0, 9000).unwrap(), content(0, 9000));
    assert_eq!(fs.version_ledger().unwrap().rows.len(), 1);
    assert!(fs.fsck(false).unwrap().is_clean());
}

#[test]
fn restore_round_trips() {
    let mut fs = vol(2048);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "draft.txt".into()).unwrap();
    let (a, b, c) = (content(0xA, 6000), content(0xB, 7000), content(0xC, 3000));
    fs.write_data(id, 0, &a).unwrap();
    fs.write_data(id, 0, &b).unwrap();
    fs.truncate_data(id, 0).unwrap();
    fs.write_data(id, 0, &c).unwrap();
    // Ring, newest first: b (the truncate), then a (b's write). The write of c filled an empty file.
    assert_eq!(fs.versions(id).unwrap().len(), 2);
    assert_eq!(fs.read_version(id, 1, 0, 6000).unwrap(), a);
    fs.restore_version(id, 1).unwrap();
    assert_eq!(fs.stat(id).unwrap().size, 6000);
    assert_eq!(fs.read_data(id, 0, 6000).unwrap(), a);
    // The restore kept c as the newest version; a stays in the ring.
    let v = fs.versions(id).unwrap();
    assert_eq!(v.len(), 3);
    assert_eq!(fs.read_version(id, 0, 0, v[0].size).unwrap(), c);
    assert_eq!(fs.read_version(id, 2, 0, 6000).unwrap(), a);
    // Round trip: back to c, which survives a remount.
    fs.restore_version(id, 0).unwrap();
    assert_eq!(fs.read_data(id, 0, 3000).unwrap(), c);
    assert!(fs.fsck(false).unwrap().is_clean());
    let mut fs = remount(fs);
    assert_eq!(fs.read_data(id, 0, 3000).unwrap(), c);
    assert!(fs.fsck(false).unwrap().is_clean());
    assert!(fs.restore_version(id, 99).is_err());
}

#[test]
fn bound_drops_the_oldest_version_anywhere() {
    let mut fs = vol(1024);
    let bound = fs.version_bound_blocks();
    assert_eq!(bound, 102);
    let root = fs.superblock.root_inode;
    let a = fs.create_file(root, "a.bin".into()).unwrap();
    let b = fs.create_file(root, "b.bin".into()).unwrap();
    let blk = BLOCK_SIZE as usize;
    fs.write_data(a, 0, &content(1, 20 * blk)).unwrap();
    fs.write_data(a, 0, &content(2, 20 * blk)).unwrap(); // a1: 20 blocks
    fs.write_data(b, 0, &content(3, 40 * blk)).unwrap();
    fs.write_data(b, 0, &content(4, 40 * blk)).unwrap(); // b1: 40
    fs.write_data(b, 0, &content(5, 40 * blk)).unwrap(); // b2: 40 — 100 kept, under 102
    assert_eq!(fs.version_ledger().unwrap().kept(), 100);
    assert_eq!(fs.versions(a).unwrap().len(), 1);
    fs.write_data(b, 0, &content(6, 40 * blk)).unwrap(); // b3: 140 → a1 (the oldest anywhere) then b1 go
    assert!(fs.versions(a).unwrap().is_empty(), "the oldest version anywhere went first");
    let v = fs.versions(b).unwrap();
    assert_eq!(v.len(), 2);
    assert_eq!(fs.read_version(b, 0, 0, 40 * blk as u64).unwrap(), content(5, 40 * blk));
    assert_eq!(fs.read_version(b, 1, 0, 40 * blk as u64).unwrap(), content(4, 40 * blk));
    let kept = fs.version_ledger().unwrap().kept();
    assert_eq!(kept, 80);
    assert!(kept <= bound);
    let r = fs.fsck(false).unwrap();
    assert!(r.is_clean(), "{r:?}");
    assert_eq!(r.version_blocks, 80);
}

#[test]
fn unlink_releases_every_version() {
    let mut fs = vol(2048);
    let root = fs.superblock.root_inode;
    // Seed the directory and the indexes so the doomed file's rewrites net to zero.
    let keeper = fs.create_file(root, "keeper.txt".into()).unwrap();
    fs.write_data(keeper, 0, b"keep").unwrap();
    let free_before = fs.free_blocks();
    let id = fs.create_file(root, "gone.bin".into()).unwrap();
    for i in 0..5u8 {
        fs.write_data(id, 0, &content(i, 3 * BLOCK_SIZE as usize)).unwrap();
    }
    assert_eq!(fs.versions(id).unwrap().len(), 4);
    assert!(fs.free_blocks() < free_before);
    fs.unlink(root, "gone.bin").unwrap();
    assert_eq!(fs.free_blocks(), free_before, "data, versions and ring all came back");
    assert!(fs.version_ledger().unwrap().rows.is_empty());
    let r = fs.fsck(false).unwrap();
    assert!(r.is_clean(), "{r:?}");
    assert_eq!(r.versions, 0);
}

/// The inode-block decode as it stood before FILEVERSIONS (inode.rs `decode_block` at 73acb28b): the §R6
/// record, the meta trailer, and a spill trailer iff its magic sits right after — nothing past it.
fn v7_decode(block: &[u8]) -> (Inode, Option<IndirectTrailer>, usize) {
    let (mut inode, mut len): (Inode, usize) = unafs::codec::deserialize_block_prefix(block).expect("v7 record");
    if let Some(m) = inode.apply_meta(&block[len..]).expect("v7 meta") {
        len += m;
    }
    let tag = u64::from_le_bytes(block[len..len + 8].try_into().unwrap());
    if tag == INODE_SPILL_MAGIC {
        let t: IndirectTrailer = unafs::codec::deserialize_block(&block[len..]).expect("v7 spill trailer");
        return (inode, Some(t), len);
    }
    (inode, None, len)
}

#[test]
fn a_v7_reader_opens_the_versioned_volume() {
    let mut fs = vol(2048);
    let root = fs.superblock.root_inode;
    // An inline file with an inline ring.
    let small = fs.create_file(root, "small.txt".into()).unwrap();
    fs.write_data(small, 0, b"first").unwrap();
    fs.write_data(small, 0, b"second").unwrap();
    // A sparse file: 200 blocks with a hole between each, so 200 extents — a SPILLED inode. Writes past the
    // end replace nothing.
    let x = fs.create_file(root, "x.bin".into()).unwrap();
    let blk = BLOCK_SIZE as usize;
    for i in 0..200u64 {
        fs.write_data(x, 2 * i * BLOCK_SIZE, &content(i as u8, blk)).unwrap();
    }
    assert!(fs.versions(x).unwrap().is_empty());
    let x_size = 399 * BLOCK_SIZE;
    let x_old = fs.read_data(x, 0, x_size).unwrap();
    // One block of x replaced: its version names 200 extents — an extent-backed ring after the spill trailer.
    fs.write_data(x, 0, &content(0xEE, blk)).unwrap();
    assert_eq!(fs.versions(x).unwrap().len(), 1);
    assert_eq!(fs.versions(x).unwrap()[0].kept_blocks, 1);

    let mut fs = remount(fs);
    assert_eq!(fs.read_version(x, 0, 0, x_size).unwrap(), x_old);
    assert_eq!(fs.read_version(small, 0, 0, 5).unwrap(), b"first");

    // The stamp did not move.
    let mut b0 = vec![0u8; blk];
    fs.device.read_block(0, &mut b0).unwrap();
    assert_eq!(&b0[0..5], b"UNAFS");
    assert_eq!(u32::from_le_bytes(b0[5..9].try_into().unwrap()), 7);

    // Every allocated inode block decodes the v7 way, to the same record the new reader sees.
    let mut buf = vec![0u8; blk];
    let mut saw_inline_ring = false;
    let mut saw_store_after_spill = false;
    let mut id = 1u64;
    while let Some(pb) = fs.inode_block(id).or_else(|| (id < 8).then_some(0)) {
        if pb == 0 {
            id += 1;
            continue;
        }
        fs.device.read_block(pb, &mut buf).unwrap();
        let (old, spill, end) = v7_decode(&buf);
        let new = fs.read_inode(id).unwrap();
        assert_eq!((old.id, old.kind, old.size), (new.id, new.kind, new.size), "inode {id}");
        assert_eq!(old.attributes, new.attributes);
        assert_eq!(old.parent, new.parent);
        match &spill {
            Some(t) => assert_eq!(t.total_extents as usize, new.chunks.len(), "a v7 reader rebuilds the whole file"),
            None => assert_eq!(old.chunks, new.chunks),
        }
        if !new.versions.is_empty() {
            let after = match &spill {
                Some(t) => end + 32 + t.index.len() * 24,
                None => end,
            };
            assert_eq!(u64::from_le_bytes(buf[after..after + 8].try_into().unwrap()), INODE_VERSIONS_MAGIC);
            if new.kind == FileKind::File && spill.is_none() && new.versions_store.is_none() {
                saw_inline_ring = true;
            }
            if spill.is_some() && new.versions_store.is_some() {
                saw_store_after_spill = true;
            }
        }
        id += 1;
    }
    assert!(saw_inline_ring && saw_store_after_spill, "both trailer forms were exercised");
    // A mount that keeps no versions reads and writes the volume as before; fsck counts the versions.
    fs.write_data(small, 0, b"plain").unwrap();
    assert_eq!(fs.read_data(small, 0, 6).unwrap(), b"plaind");
    assert_eq!(fs.versions(small).unwrap().len(), 1, "versioning is off on a plain mount");
    let r = fs.fsck(false).unwrap();
    assert!(r.is_clean(), "{r:?}");
    assert_eq!(r.versions, 2);
}

// ── crawl B (wave 36): the fixer's proofs for findings 6, 5 and 11 ──────────────────────────────────────

#[test]
fn a_version_index_past_usize_is_not_found_never_a_panic() {
    let mut fs = vol(1024);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "doc.txt".into()).unwrap();
    fs.write_data(id, 0, &content(1, 100)).unwrap();
    fs.write_data(id, 0, &content(2, 100)).unwrap();
    assert_eq!(fs.versions(id).unwrap().len(), 1);
    assert!(fs.read_version(id, usize::MAX, 0, 1).is_err());
    assert!(fs.restore_version(id, usize::MAX).is_err());
    assert!(fs.restore_version(id, 1).is_err());
    assert_eq!(fs.read_data(id, 0, 100).unwrap(), content(2, 100));
}

#[test]
fn restore_refuses_a_version_whose_blocks_lost_their_references() {
    let mut fs = vol(2048);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "doc.txt".into()).unwrap();
    let blk = BLOCK_SIZE as usize;
    fs.write_data(id, 0, &content(1, 3 * blk)).unwrap();
    fs.write_data(id, 0, &content(2, 3 * blk)).unwrap(); // one version: three blocks only the ring holds
    let ring = unafs::fs::versions::decode_ring(&fs.read_inode(id).unwrap().versions).unwrap();
    assert_eq!(ring.len(), 1);
    let blocks: Vec<u64> =
        ring[0].extents.iter().flat_map(|e| (0..e.length.div_ceil(BLOCK_SIZE)).map(move |i| e.physical_block + i)).collect();
    assert_eq!(blocks.len(), 3);
    // What a pre-B714 `fsck --repair` leaves: the ring still names the blocks, their references are gone.
    fs.reclaim_enqueue(ReclaimEntry { generation: 0, blocks }).unwrap();
    fs.reclaim_drain().unwrap();
    assert!(matches!(fs.restore_version(id, 0), Err(FileSystemError::CorruptVolume(_))), "refused, not restored onto free blocks");
    assert_eq!(fs.read_data(id, 0, 3 * blk as u64).unwrap(), content(2, 3 * blk), "the live file is untouched");
    assert_eq!(fs.versions(id).unwrap().len(), 1);
}

#[test]
fn a_full_volume_spends_the_oldest_version_before_it_refuses_an_overwrite() {
    let mut fs = vol(512);
    let root = fs.superblock.root_inode;
    let doc = fs.create_file(root, "doc.bin".into()).unwrap();
    let blk = BLOCK_SIZE as usize;
    fs.write_data(doc, 0, &content(1, 30 * blk)).unwrap();
    fs.write_data(doc, 0, &content(2, 30 * blk)).unwrap(); // one version: 30 blocks kept
    assert_eq!(fs.version_ledger().unwrap().kept(), 30);
    // Fill the volume to a dozen free blocks with versioning off (so the filler spends no version): too few for
    // a 20-block overwrite, enough for the drop's own metadata transaction (a volume at 0 free cannot run one).
    fs.set_versioning(false);
    let fill = fs.create_file(root, "fill.bin".into()).unwrap();
    let mut at = 0u64;
    while fs.free_blocks() > 12 {
        fs.write_data(fill, at, &content(9, blk)).unwrap();
        at += BLOCK_SIZE;
    }
    fs.set_versioning(true);
    assert!(fs.free_blocks() < 20, "the volume is full: {}", fs.free_blocks());
    fs.write_data(doc, 0, &content(3, 20 * blk)).expect("the oldest version gives its room to the overwrite");
    let mut want = content(3, 20 * blk);
    want.extend_from_slice(&content(2, 30 * blk)[20 * blk..]);
    assert_eq!(fs.read_data(doc, 0, 30 * blk as u64).unwrap(), want);
    let v = fs.versions(doc).unwrap();
    assert_eq!(v.len(), 1, "the old version went; the overwrite kept its own");
    assert_eq!(fs.read_version(doc, 0, 0, 30 * blk as u64).unwrap(), content(2, 30 * blk));
    let r = fs.fsck(false).unwrap();
    assert!(r.is_clean(), "{r:?}");
}

// ---- VERSIONSCOMPAT (SR154): the root record's ROCOMPAT `versions` bit and the read-only rule ----

use unafs::fs::FileSystemError;
use unafs::root::{ROOT_FLAG_VERSIONS, read_active, write_slot};

/// The active root's `flags` word as it sits on the device.
fn disk_flags(dev: &mut MemDevice) -> u64 {
    read_active(dev).unwrap().expect("a valid root").0.flags
}

/// Commit a new root (generation + 1, to the inactive slot) whose `flags` are `f(old flags)` — what a
/// reader built later, or one built before the flag, would have left on the disk.
fn reroot(bytes: Vec<u8>, f: impl Fn(u64) -> u64) -> Vec<u8> {
    let mut dev = MemDevice::from_bytes(bytes);
    let (mut rec, slot) = read_active(&mut dev).unwrap().expect("a valid root");
    rec.generation += 1;
    rec.flags = f(rec.flags);
    write_slot(&mut dev, slot.other(), &rec).unwrap();
    dev.as_bytes().to_vec()
}

#[test]
fn the_first_version_sets_the_rocompat_bit_and_repair_keeps_every_version_block() {
    // A volume that never kept a version says nothing.
    let mut plain = vol(2048);
    let root = plain.superblock.root_inode;
    let id = plain.create_file(root, "once.txt".into()).unwrap();
    plain.write_data(id, 0, &content(7, 5000)).unwrap();
    assert_eq!(disk_flags(&mut plain.device) & ROOT_FLAG_VERSIONS, 0);
    assert_eq!(plain.rocompat(), 0);

    let mut fs = vol(2048);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "doc.txt".into()).unwrap();
    for i in 0..10u8 {
        fs.write_data(id, 0, &content(i, 5000)).unwrap();
    }
    assert_eq!(fs.rocompat(), ROOT_FLAG_VERSIONS);
    assert!(!fs.read_only());
    assert_ne!(disk_flags(&mut fs.device) & ROOT_FLAG_VERSIONS, 0, "the commit carries the bit");

    // The current reader's repair counts every ring's references: nothing reclaimed, nothing cleared.
    let r = fs.fsck(true).unwrap();
    assert_eq!(r.reclaimed_blocks, 0, "{r:?}");
    assert_eq!(r.versions, RING_MAX as u64);
    assert_ne!(disk_flags(&mut fs.device) & ROOT_FLAG_VERSIONS, 0, "repair keeps the bit");

    // Every block a version names is still held: fill the free space, then read every version back.
    for f in 0..40u8 {
        let n = fs.create_file(root, format!("fill{f}.bin")).unwrap();
        fs.write_data(n, 0, &content(100 + f, 8 * BLOCK_SIZE as usize)).unwrap();
    }
    let mut fs = remount(fs);
    assert_eq!(fs.rocompat(), ROOT_FLAG_VERSIONS);
    for n in 0..RING_MAX {
        assert_eq!(fs.read_version(id, n, 0, 5000).unwrap(), content(8 - n as u8, 5000), "version {n}");
    }
    let r = fs.fsck(false).unwrap();
    assert!(r.is_clean(), "{r:?}");
}

#[test]
fn a_volume_versioned_before_the_flag_gets_it_on_repair() {
    let mut fs = vol(2048);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "old.txt".into()).unwrap();
    for i in 0..3u8 {
        fs.write_data(id, 0, &content(i, 6000)).unwrap();
    }
    // FILEVERSIONS (B714) before SR154 kept versions under a root with no ROCOMPAT word.
    let bytes = reroot(fs.device.as_bytes().to_vec(), |f| f & !ROOT_FLAG_VERSIONS);
    let mut fs = UnaFS::mount(MemDevice::from_bytes(bytes)).unwrap();
    assert_eq!(fs.rocompat(), 0);
    let r = fs.fsck(true).unwrap();
    assert_eq!(r.reclaimed_blocks, 0, "{r:?}");
    assert_eq!(r.versions, 2);
    assert_eq!(fs.rocompat(), ROOT_FLAG_VERSIONS);
    assert_ne!(disk_flags(&mut fs.device) & ROOT_FLAG_VERSIONS, 0);
    assert_eq!(fs.read_version(id, 1, 0, 6000).unwrap(), content(0, 6000));
}

#[test]
fn an_unknown_rocompat_bit_mounts_read_only_and_refuses_repair_and_writes() {
    const FUTURE: u64 = 1 << 40;
    let mut fs = vol(2048);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "kept.txt".into()).unwrap();
    for i in 0..3u8 {
        fs.write_data(id, 0, &content(i, 5000)).unwrap();
    }
    let bytes = reroot(fs.device.as_bytes().to_vec(), |f| f | FUTURE);
    let mut fs = UnaFS::mount(MemDevice::from_bytes(bytes.clone())).expect("mounts (read-only)");
    assert!(fs.read_only());
    assert_eq!(fs.rocompat(), ROOT_FLAG_VERSIONS | FUTURE);
    let generation = fs.root_generation();

    // Reads work.
    assert_eq!(fs.read_data(id, 0, 5000).unwrap(), content(2, 5000));
    assert_eq!(fs.versions(id).unwrap().len(), 2);
    assert_eq!(fs.read_version(id, 1, 0, 5000).unwrap(), content(0, 5000));
    assert!(fs.fsck(false).unwrap().is_clean(), "the dry run reports");

    // Every write and the repair are refused before they touch the device.
    let ro = |r: Result<(), FileSystemError>| matches!(r, Err(FileSystemError::ReadOnly(b)) if b == FUTURE);
    assert!(ro(fs.write_data(id, 0, b"x").map(|_| ())));
    assert!(ro(fs.truncate_data(id, 1).map(|_| ())));
    assert!(ro(fs.create_file(root, "new.txt".into()).map(|_| ())));
    assert!(ro(fs.mkdir(root, "dir".into()).map(|_| ())));
    assert!(ro(fs.unlink(root, "kept.txt").map(|_| ())));
    assert!(ro(fs.fsck(true).map(|_| ())));
    assert!(ro(fs.recover().map(|_| ())));
    assert!(ro(fs.commit()));
    assert_eq!(fs.device.as_bytes(), &bytes[..], "not one byte written");

    // A mutation outside the guarded set stages, then its commit is refused: the root never flips.
    assert!(ro(fs.restore_version(id, 1)));
    let root_block = BLOCK_SIZE as usize..2 * BLOCK_SIZE as usize;
    assert_eq!(fs.device.as_bytes()[..root_block.end], bytes[..root_block.end], "block 0 and the root slots");
    let mut again = UnaFS::mount(MemDevice::from_bytes(fs.device.as_bytes().to_vec())).unwrap();
    assert_eq!(again.root_generation(), generation);
    assert_eq!(again.read_data(id, 0, 5000).unwrap(), content(2, 5000));
    assert!(again.read_only());
}
