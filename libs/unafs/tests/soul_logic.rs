use unafs::{MemDevice, BlockDevice, UnaFS, BLOCK_SIZE, AttributeValue};

#[test]
fn test_soul_logic() {
    // 1. Initialize
    let block_count = 5000;
    let mut device = MemDevice::new();
    let empty_block = vec![0u8; BLOCK_SIZE as usize];
    device.write_block(block_count - 1, &empty_block).expect("Failed to set disk size");

    let mut fs = UnaFS::format(device, 20).expect("Format failed");
    let root_id = fs.superblock.root_inode;

    // 2. Create Files with Attributes
    let file1_id = fs.create_file(root_id, "happy.txt".to_string()).expect("Failed to create file1");
    let file2_id = fs.create_file(root_id, "sad.txt".to_string()).expect("Failed to create file2");
    let file3_id = fs.create_file(root_id, "neutral.txt".to_string()).expect("Failed to create file3");

    // 3. Set Attributes
    // Small attribute
    fs.set_attribute(file1_id, "emotion".to_string(), AttributeValue::String("happy".to_string())).expect("Set attr failed");
    fs.set_attribute(file2_id, "emotion".to_string(), AttributeValue::String("sad".to_string())).expect("Set attr failed");

    // Vector attribute (Small)
    let vec_small = AttributeValue::Vector(vec![0.9, 0.1]);
    fs.set_attribute(file1_id, "embedding".to_string(), vec_small.clone()).expect("Set vec failed");

    // Large attribute (Vector > 64 floats)
    let mut large_vec_data = Vec::new();
    for i in 0..100 {
        large_vec_data.push(i as f32);
    }
    let vec_large = AttributeValue::Vector(large_vec_data.clone());
    fs.set_attribute(file3_id, "embedding".to_string(), vec_large.clone()).expect("Set large vec failed");

    // 4. Verify Get Attribute
    let attr1 = fs.get_attribute(file1_id, "emotion").expect("Get attr failed").unwrap();
    assert_eq!(attr1, AttributeValue::String("happy".to_string()));

    let attr3 = fs.get_attribute(file3_id, "embedding").expect("Get large attr failed").unwrap();
    if let AttributeValue::Vector(v) = attr3 {
        assert_eq!(v.len(), 100);
        assert_eq!(v[0], 0.0);
        assert_eq!(v[99], 99.0);
    } else {
        panic!("Wrong type for large attribute");
    }

    // 5. Query Engine
    // Exact Match
    let results = fs.query("emotion == \"happy\"").expect("Query failed");
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].id, file1_id);

    // Similarity Query
    // [0.9, 0.1] vs [1.0, 0.0] -> dot=0.9, mag_a=sqrt(0.81+0.01)=0.905, mag_b=1.0. Sim = 0.9/0.905 ~= 0.99
    // [0.9, 0.1] vs [0.0, 1.0] -> dot=0.1, ... Sim ~= 0.11

    let results_sim = fs.query("similarity(embedding, [1.0, 0.0]) > 0.9").expect("Sim query failed");
    // Should match file1
    assert!(results_sim.iter().any(|inode| inode.id == file1_id));

    // Should NOT match file3 (vectors are very different)
    assert!(!results_sim.iter().any(|inode| inode.id == file3_id));

    // 6. Verify Catalog Persistence (Implicitly tested by query working)
    // But let's verify mount restores it.

    let device_back = fs.device;
    let mut fs2 = UnaFS::mount(device_back).expect("Mount failed");

    let results2 = fs2.query("emotion == \"sad\"").expect("Query after mount failed");
    assert_eq!(results2.len(), 1);
    assert_eq!(results2[0].id, file2_id);
}
