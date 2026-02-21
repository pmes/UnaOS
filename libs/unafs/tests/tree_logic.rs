use unafs::{MemDevice, BlockDevice, UnaFS, BLOCK_SIZE};
use unafs::inode::FileKind;

#[test]
fn test_tree_of_life() {
    // 1. Initialize Device (10 MB)
    let block_count = 2560;
    let mut device = MemDevice::new();
    let empty_block = vec![0u8; BLOCK_SIZE as usize];
    device.write_block(block_count - 1, &empty_block).expect("Failed to set disk size");

    // 2. Format
    let mut fs = UnaFS::format(device, 10).expect("Format failed");
    let root_id = fs.superblock.root_inode;

    // 3. Create Directory Structure
    // /home
    let home_id = fs.mkdir(root_id, "home".to_string()).expect("Failed to create /home");

    // /home/vector
    let vector_id = fs.mkdir(home_id, "vector".to_string()).expect("Failed to create /home/vector");

    // /home/vector/notes.txt
    let notes_id = fs.create_file(vector_id, "notes.txt".to_string()).expect("Failed to create notes.txt");

    // 4. Verify Structure (ls)
    // Check Root
    let root_entries = fs.ls(root_id).expect("Failed to ls root");
    assert_eq!(root_entries.len(), 1);
    assert_eq!(root_entries[0].name, "home");
    assert_eq!(root_entries[0].kind, FileKind::Directory);
    assert_eq!(root_entries[0].inode_id, home_id);

    // Check Home
    let home_entries = fs.ls(home_id).expect("Failed to ls home");
    assert_eq!(home_entries.len(), 1);
    assert_eq!(home_entries[0].name, "vector");
    assert_eq!(home_entries[0].inode_id, vector_id);

    // Check Vector
    let vector_entries = fs.ls(vector_id).expect("Failed to ls vector");
    assert_eq!(vector_entries.len(), 1);
    assert_eq!(vector_entries[0].name, "notes.txt");
    assert_eq!(vector_entries[0].kind, FileKind::File);
    assert_eq!(vector_entries[0].inode_id, notes_id);

    // 5. Write Data to File
    let data1 = b"Hello, ";
    fs.write_data(notes_id, 0, data1).expect("Failed to write data1");

    let data2 = b"World!";
    // Append to end
    let offset = data1.len() as u64;
    fs.write_data(notes_id, offset, data2).expect("Failed to write data2");

    // 6. Read Data Back
    let read_data = fs.read_data(notes_id, 0, 100).expect("Failed to read data");
    assert_eq!(read_data, b"Hello, World!");

    // 7. Verify Inode Size
    let notes_inode = fs.read_inode(notes_id).expect("Failed to read notes inode");
    assert_eq!(notes_inode.size, (data1.len() + data2.len()) as u64);

    // 8. Test Extent Spanning (Write across block boundary)
    // Write huge data to force new blocks
    let huge_data = vec![0xAAu8; 5000]; // > 4096
    fs.write_data(notes_id, notes_inode.size, &huge_data).expect("Failed to write huge data");

    let updated_inode = fs.read_inode(notes_id).expect("Failed to read updated inode");
    // Size should be old size + 5000
    assert_eq!(updated_inode.size, (data1.len() + data2.len() + 5000) as u64);

    // Verify data correctness at end
    // Note: read_data returns Vec<u8> which we compare to Vec<u8>.
    let read_huge = fs.read_data(notes_id, (data1.len() + data2.len()) as u64, 5000).expect("Failed to read huge data");
    assert_eq!(read_huge, huge_data);
}
