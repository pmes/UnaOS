use unafs::{MemDevice, BlockDevice, Inode, AttributeValue, BLOCK_SIZE};

#[test]
fn test_inode_serialization_vector() {
    // 1. Initialize Device
    let mut device = MemDevice::new();

    // 2. Create Inode
    let mut inode = Inode::new(101);
    let vector_data = vec![0.1f32, 0.2, 0.9];
    inode.attributes.insert(
        "embedding".to_string(),
        AttributeValue::Vector(vector_data.clone())
    );
    inode.attributes.insert(
        "emotion".to_string(),
        AttributeValue::String("exhausted".to_string())
    );

    // 3. Serialize to "Disk"
    let bytes = inode.to_bytes().expect("Failed to serialize inode");

    // Write to block 0. block_data must be exactly BLOCK_SIZE.
    let mut block_data = vec![0u8; BLOCK_SIZE as usize];
    // Copy the serialized bytes into the start of the block buffer
    block_data[..bytes.len()].copy_from_slice(&bytes);

    device.write_block(0, &block_data).expect("Failed to write block 0");

    // 4. Wipe Inode (simulated by not using original 'inode' anymore)

    // 5. Read back
    let mut read_buffer = vec![0u8; BLOCK_SIZE as usize];
    device.read_block(0, &mut read_buffer).expect("Failed to read block 0");

    // 6. Deserialize
    let loaded_inode = Inode::from_bytes(&read_buffer).expect("Failed to deserialize inode");

    // 7. Assert
    assert_eq!(loaded_inode.id, 101);

    // Check "embedding"
    match loaded_inode.attributes.get("embedding") {
        Some(AttributeValue::Vector(v)) => {
            assert_eq!(v, &vector_data);
        },
        _ => panic!("Attribute 'embedding' missing or wrong type"),
    }

    // Check "emotion"
    match loaded_inode.attributes.get("emotion") {
        Some(AttributeValue::String(s)) => {
            assert_eq!(s, "exhausted");
        },
        _ => panic!("Attribute 'emotion' missing or wrong type"),
    }
}
