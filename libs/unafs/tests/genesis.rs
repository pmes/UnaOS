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

use std::collections::BTreeMap;
use unafs::superblock::{MAGIC, VERSION};
use unafs::{AttributeValue, BLOCK_SIZE, BlockDevice, MemDevice, UnaFS};

#[test]
fn test_big_bang() {
    // 1. Initialize Device (10 MB = 10 * 1024 * 1024 bytes / 4096 = 2560 blocks)
    // Let's use exactly 2560 blocks.
    let block_count = 2560;
    let mut device = MemDevice::new();

    // Resize device to simulate raw disk size.
    // MemDevice grows on write, but for "format" to know the size, we need to pre-fill or expose a set size.
    // Wait, MemDevice::block_count() returns data.len() / BLOCK_SIZE.
    // If it's empty, format sees 0 blocks.
    // We must pre-allocate the "Disk".
    // Write the last block to force size.
    let empty_block = vec![0u8; BLOCK_SIZE as usize];
    device
        .write_block(block_count - 1, &empty_block)
        .expect("Failed to set disk size");

    // Verify block count
    assert_eq!(device.block_count(), block_count);

    // 2. Format
    let mut fs = UnaFS::format(device, 10).expect("Format failed");

    // 3. Assert Superblock
    let sb = &fs.superblock;
    assert_eq!(sb.magic, MAGIC);
    assert_eq!(sb.version, VERSION);
    assert_eq!(sb.block_count, block_count);

    // Updated Layout:
    // SB (0)
    // Journal (1..10) -> 10 blocks
    // Bitmap (11..) -> 1 block for 2560 bits
    assert_eq!(sb.bitmap_start, 11);

    // Check bitmap size: 2560 bits -> 320 bytes -> fits in 1 block (4096 bytes)
    assert_eq!(sb.bitmap_blocks, 1);

    // 4. Assert Root Inode
    let root_id = sb.root_inode;
    // Bitmap ends at 11.
    // Root Inode allocated next -> 12.
    assert_eq!(root_id, 12);

    // Assert Catalog Inode
    let catalog_id = sb.catalog_inode;
    assert_eq!(catalog_id, 13);

    // Read Root Inode using internal FS method
    let root_inode = fs.read_inode(root_id).expect("Failed to read root inode");
    assert_eq!(root_inode.id, root_id);

    // 5. Create a File
    let mut attrs = BTreeMap::new();
    attrs.insert(
        "filename".to_string(),
        AttributeValue::String("manifesto.txt".to_string()),
    );

    let file_id = fs.create_inode(attrs).expect("Failed to create file");

    // File should be next free block (14)
    assert_eq!(file_id, 14);

    // 6. Verify Persistence (Mount)
    // UnaFS consumes device. We need to extract it back.
    // Since `fs` owns `device` (public), we can take it.
    let device_back = fs.device;

    let mut fs2 = UnaFS::mount(device_back).expect("Mount failed");

    assert_eq!(fs2.superblock.magic, MAGIC);
    assert_eq!(fs2.superblock.root_inode, 12);

    // Check file exists
    let file_inode = fs2
        .read_inode(file_id)
        .expect("Failed to read file after mount");

    // Verify attributes
    match file_inode.attributes.get("filename") {
        Some(AttributeValue::String(s)) => assert_eq!(s, "manifesto.txt"),
        _ => panic!("Attribute 'filename' missing or wrong type"),
    }
}
