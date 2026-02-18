use unafs::{MemDevice, BlockDevice, UnaFS, BLOCK_SIZE, AttributeValue};
use unafs::superblock::{MAGIC, VERSION};
use std::collections::BTreeMap;

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
    device.write_block(block_count - 1, &empty_block).expect("Failed to set disk size");

    // Verify block count
    assert_eq!(device.block_count(), block_count);

    // 2. Format
    let mut fs = UnaFS::format(device).expect("Format failed");

    // 3. Assert Superblock
    let sb = &fs.superblock;
    assert_eq!(sb.magic, MAGIC);
    assert_eq!(sb.version, VERSION);
    assert_eq!(sb.block_count, block_count);
    assert_eq!(sb.bitmap_start, 1);

    // Check bitmap size: 2560 bits -> 320 bytes -> fits in 1 block (4096 bytes)
    assert_eq!(sb.bitmap_blocks, 1);

    // 4. Assert Root Inode
    let root_id = sb.root_inode;
    // Root should be after SB (0) and Bitmap (1) -> Block 2
    // Wait, if bitmap is large it might take more blocks. 2560 bits fits in 1 block.
    // Bitmap is at block 1.
    // Allocation starts searching from 0.
    // Block 0 is marked used (SB).
    // Block 1 is marked used (Bitmap).
    // Block 2 is first free -> Root.
    assert_eq!(root_id, 2);

    // Read Root Inode using internal FS method
    let root_inode = fs.read_inode(root_id).expect("Failed to read root inode");
    assert_eq!(root_inode.id, root_id);

    // 5. Create a File
    let mut attrs = BTreeMap::new();
    attrs.insert("filename".to_string(), AttributeValue::String("manifesto.txt".to_string()));

    let file_id = fs.create_inode(attrs).expect("Failed to create file");

    // File should be next free block (3)
    assert_eq!(file_id, 3);

    // 6. Verify Persistence (Mount)
    // UnaFS consumes device. We need to extract it back.
    // Since `fs` owns `device` (public), we can take it.
    let device_back = fs.device;

    let fs2 = UnaFS::mount(device_back).expect("Mount failed");

    assert_eq!(fs2.superblock.magic, MAGIC);
    assert_eq!(fs2.superblock.root_inode, 2);

    // Check file exists
    let file_inode = fs2.read_inode(file_id).expect("Failed to read file after mount");

    // Verify attributes
    match file_inode.attributes.get("filename") {
        Some(AttributeValue::String(s)) => assert_eq!(s, "manifesto.txt"),
        _ => panic!("Attribute 'filename' missing or wrong type"),
    }
}
