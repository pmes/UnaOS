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

//! DISKRESERVE (SR152): a failed write unwinds to the committed root (M1), and
//! the metadata reserve keeps a full volume able to delete, rename and
//! truncate (M2). unafs-records.md §R17.

use unafs::fs::FileSystemError;
use unafs::refmap::{NoSpaceKind, reserve_for};
use unafs::{BLOCK_SIZE, MemDevice, UnaFS};

const BLOCKS: u64 = 2048;

fn fresh() -> UnaFS<MemDevice> {
    UnaFS::format(MemDevice::with_blocks(BLOCKS), 0).expect("format")
}

/// What a mount of the committed root says, compared with the live mount.
fn assert_ram_is_committed(fs: &mut UnaFS<MemDevice>, ids: &[u64]) {
    let root = fs.superblock.root_inode;
    let mut disk = UnaFS::mount(fs.device.clone()).expect("mount the committed root");
    assert_eq!(fs.free_blocks(), disk.free_blocks(), "free count = the committed one");
    assert_eq!(fs.map_block_total(), disk.map_block_total(), "map blocks = the committed ones");
    assert_eq!(fs.root_generation(), disk.root_generation());
    assert_eq!(fs.ls(root).unwrap(), disk.ls(root).unwrap(), "root listing");
    for &id in ids {
        let (a, b) = (fs.read_inode(id).unwrap(), disk.read_inode(id).unwrap());
        assert_eq!((a.size, &a.chunks), (b.size, &b.chunks), "inode {id}");
        assert_eq!(fs.inode_block(id), disk.inode_block(id), "inode map entry {id}");
    }
    assert!(fs.fsck(false).unwrap().is_clean(), "live fsck");
    assert!(disk.fsck(false).unwrap().is_clean(), "committed fsck");
}

#[test]
fn failed_write_unwinds_to_committed_maps() {
    let mut fs = fresh();
    let root = fs.superblock.root_inode;
    assert_eq!(fs.reserve_blocks(), reserve_for(BLOCKS));
    assert_eq!(fs.reserve_blocks(), 64);
    let id = fs.create_file(root, "grow.bin".into()).unwrap();
    // 40-block appends: the last one allocates part of its span, then hits
    // the reserve — those blocks must not survive in RAM.
    let chunk = vec![0x5au8; (40 * BLOCK_SIZE) as usize];
    let mut off = 0u64;
    let err = loop {
        let free = fs.free_blocks();
        match fs.write_data(id, off, &chunk) {
            Ok(()) => off += chunk.len() as u64,
            Err(e) => {
                assert!(free < 40 + 8, "refused only near the reserve (usable {free})");
                assert_eq!(fs.free_blocks(), free, "the failed write left no block taken");
                break e;
            }
        }
    };
    assert!(matches!(err, FileSystemError::NoSpace), "{err:?}");
    assert_eq!(fs.no_space_kind(), Some(NoSpaceKind::Reserve));
    assert_eq!(fs.read_inode(id).unwrap().size, off, "size is the last committed write's");
    assert_ram_is_committed(&mut fs, &[id]);
    // The same mount goes on: a write that fits lands.
    fs.write_data(id, off, &[1u8; 100]).unwrap();
    assert_eq!(fs.read_data(id, off, 100).unwrap(), vec![1u8; 100]);
    assert_ram_is_committed(&mut fs, &[id]);
}

#[test]
fn full_volume_still_deletes_renames_truncates() {
    let mut fs = fresh();
    let root = fs.superblock.root_inode;
    let big = vec![0xa5u8; (32 * BLOCK_SIZE) as usize];
    let mut ids = Vec::new();
    // Fill with 32-block files, then single blocks, until DATA is refused.
    loop {
        let id = fs.create_file(root, format!("f{}", ids.len())).unwrap();
        ids.push(id);
        if fs.write_data(id, 0, &big).is_err() {
            break;
        }
    }
    let tail = fs.create_file(root, "tail".into()).unwrap();
    let mut off = 0u64;
    while fs.write_data(tail, off, &[7u8; BLOCK_SIZE as usize]).is_ok() {
        off += BLOCK_SIZE;
    }
    assert_eq!(fs.free_blocks(), 0, "FULL: no usable block left");
    assert_eq!(fs.no_space_kind(), Some(NoSpaceKind::Reserve));
    assert!(matches!(fs.write_data(tail, off, &[7u8; 1]), Err(FileSystemError::NoSpace)));

    // Metadata spends the reserve: every one of these succeeds on the full volume.
    fs.rename(root, "f1", root, "renamed").expect("rename on a full volume");
    fs.truncate_data(ids[2], 1000).expect("truncate on a full volume (tail CoW)");
    assert_eq!(fs.read_data(ids[2], 0, 1000).unwrap(), vec![0xa5u8; 1000]);
    fs.unlink(root, "f0").expect("unlink on a full volume");
    fs.unlink(root, "tail").expect("a second unlink");
    assert!(fs.free_blocks() >= 32, "the deletes gave the space back ({})", fs.free_blocks());

    // ... and DATA writes again.
    let again = fs.create_file(root, "again".into()).unwrap();
    fs.write_data(again, 0, &big[..(16 * BLOCK_SIZE) as usize]).expect("write after the deletes");
    assert_eq!(fs.read_data(again, 0, 16 * BLOCK_SIZE).unwrap(), big[..(16 * BLOCK_SIZE) as usize]);
    let names: Vec<String> = fs.ls(root).unwrap().into_iter().map(|e| e.name).collect();
    assert!(names.contains(&"renamed".to_string()) && !names.contains(&"f0".to_string()));
    assert_ram_is_committed(&mut fs, &[ids[1], ids[2], again]);
}
