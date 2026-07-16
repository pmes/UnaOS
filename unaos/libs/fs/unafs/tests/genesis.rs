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

//! Genesis: format → superblock/root-record shape → persistence across a
//! genuine remount, on the K8 copy-on-write format.

use std::collections::BTreeMap;
use unafs::superblock::{
    CATALOG_INODE_ID, MAGIC, RECLAIM_INODE_ID, ROOT_INODE_ID, SNAP_INDEX_INODE_ID, VERSION,
};
use unafs::{AttributeValue, BLOCK_SIZE, BlockDevice, FileKind, MemDevice, UnaFS};

#[test]
fn test_big_bang() {
    // 1. A 10 MB raw disk (2560 blocks), pre-sized (MemDevice grows on write).
    let block_count = 2560;
    let mut device = MemDevice::new();
    let empty_block = vec![0u8; BLOCK_SIZE as usize];
    device
        .write_block(block_count - 1, &empty_block)
        .expect("Failed to set disk size");
    assert_eq!(device.block_count(), block_count);

    // 2. Format.
    let mut fs = UnaFS::format(device, 10).expect("Format failed");

    // 3. The static superblock: identity only.
    let sb = &fs.superblock;
    assert_eq!(sb.magic, MAGIC);
    assert_eq!(sb.version, VERSION);
    assert_eq!(sb.block_count, block_count);
    assert_eq!(sb.root_inode, ROOT_INODE_ID);
    assert_eq!(sb.catalog_inode, CATALOG_INODE_ID);

    // 4. The format commit is generation 1, and the reserved system objects
    //    exist at their fixed LOGICAL ids.
    assert_eq!(fs.root_generation(), 1);
    let root_inode = fs.read_inode(ROOT_INODE_ID).expect("root inode");
    assert_eq!(root_inode.id, ROOT_INODE_ID);
    assert_eq!(root_inode.kind, FileKind::Directory);
    assert_eq!(
        fs.read_inode(CATALOG_INODE_ID).unwrap().kind,
        FileKind::System
    );
    assert_eq!(
        fs.read_inode(SNAP_INDEX_INODE_ID).unwrap().kind,
        FileKind::System
    );
    assert_eq!(
        fs.read_inode(RECLAIM_INODE_ID).unwrap().kind,
        FileKind::System
    );

    // The future-shape objects are on disk and EMPTY.
    assert!(fs.snapshot_index().unwrap().is_empty());
    assert!(fs.reclaim_queue().unwrap().is_empty());

    // 5. Create a file: logical ids continue after the reserved range.
    let mut attrs = BTreeMap::new();
    attrs.insert(
        "filename".to_string(),
        AttributeValue::String("manifesto.txt".to_string()),
    );
    let file_id = fs.create_inode(attrs).expect("Failed to create file");
    assert_eq!(file_id, unafs::superblock::FIRST_USER_INODE_ID);
    let gen_after_create = fs.root_generation();
    assert!(gen_after_create > 1, "create must commit (advance the root)");

    // 6. Persistence across a genuine remount.
    let device_back = fs.device.clone();
    let mut fs2 = UnaFS::mount(device_back).expect("Mount failed");

    assert_eq!(fs2.superblock.magic, MAGIC);
    assert_eq!(fs2.superblock.root_inode, ROOT_INODE_ID);
    assert_eq!(fs2.root_generation(), gen_after_create);

    let file_inode = fs2
        .read_inode(file_id)
        .expect("Failed to read file after mount");
    match file_inode.attributes.get("filename") {
        Some(AttributeValue::String(s)) => assert_eq!(s, "manifesto.txt"),
        _ => panic!("Attribute 'filename' missing or wrong type"),
    }

    // 7. Free-space accounting round-trips through the persisted refcount map.
    assert_eq!(fs.free_blocks(), fs2.free_blocks());

    // 8. The volume is refcount-consistent out of the box.
    let report = fs2.fsck(false).expect("fsck");
    assert!(report.is_clean(), "fresh volume must be clean: {report:?}");
}
