//! UNAFSCODEC (SR53): whole-VOLUME byte equality across the codec swap.
//!
//! Each test builds a volume through the public API along one deterministic
//! script (fixed clock, fixed data) that reaches every bincode-shaped record
//! the format writes — superblock, inodes (inline, with every attribute
//! variant, with large attributes, SPILLED with an indirect trailer + the
//! overflow extent list), directory entry lists, spilled attribute values, the
//! flat catalog (v5), the snapshot index and a NON-EMPTY reclaim queue — and
//! pins an FNV-1a digest of the whole image.
//!
//! The digests were cut at 723cabf1 (the merge of exec-rmbp-merge13 into
//! exec-fs-unafscodec), when every one of those records was still encoded by
//! `bincode 2.0.1` in its `legacy()` configuration. With the hand codec in
//! place the SAME script must yield the SAME image, byte for byte: that is
//! the oracle comparison at volume scale (the per-record property oracle is
//! `tests/codec_oracle.rs`). The script then remounts the image and reads
//! every object back, so the decoder is held to the same bytes.

use std::collections::BTreeMap;
use unafs::{AttributeValue, BLOCK_SIZE, MemDevice, UnaFS};

const BS: usize = BLOCK_SIZE as usize;

fn fixed_clock() -> u64 {
    1_759_622_400 // 2025-10-05T00:00:00Z
}

fn block_bytes(j: usize) -> Vec<u8> {
    (0..BS).map(|k| ((j.wrapping_mul(31).wrapping_add(k)) & 0xFF) as u8).collect()
}

/// The deterministic script. Returns the device after the last commit.
pub fn build_volume(version: u32, blocks: u64) -> MemDevice {
    unafs::clock::set_clock_hook(fixed_clock);
    let device = MemDevice::with_blocks(blocks);
    let mut fs = UnaFS::format_with_version(device, 0, version).expect("format");
    let root = fs.superblock.root_inode;

    let docs = fs.mkdir(root, "docs".into()).unwrap();
    let a = fs.create_file(docs, "alpha.txt".into()).unwrap();
    fs.write_data(a, 0, b"the crystal remembers").unwrap();
    fs.set_attribute(a, "rank".into(), AttributeValue::Int(-42)).unwrap();
    fs.set_attribute(a, "score".into(), AttributeValue::Float(3.25)).unwrap();
    fs.set_attribute(a, "title".into(), AttributeValue::String("Alpha \u{2603}".into()))
        .unwrap();
    fs.set_attribute(a, "thumb".into(), AttributeValue::Blob(vec![0, 1, 2, 0xFE, 0xFF]))
        .unwrap();
    fs.set_attribute(a, "embed".into(), AttributeValue::Vector(vec![0.5, -1.0, 1e-7]))
        .unwrap();
    // Large attributes: each spills to its own extents (large_attributes map).
    fs.set_attribute(
        a,
        "big_vec".into(),
        AttributeValue::Vector((0..200).map(|i| i as f32 * 0.125).collect()),
    )
    .unwrap();
    fs.set_attribute(a, "big_blob".into(), AttributeValue::Blob((0..700u32).map(|i| i as u8).collect()))
        .unwrap();
    fs.set_attribute(a, "big_str".into(), AttributeValue::String("x".repeat(300)))
        .unwrap();

    // A file with many extents: hole-fragmented so the inode SPILLS (v4+).
    let frag = fs.create_file(root, "frag.bin".into()).unwrap();
    let n = if version >= 4 { 220 } else { 40 };
    for j in 0..n {
        fs.write_data(frag, (2 * j * BS) as u64, &block_bytes(j)).unwrap();
    }

    // A batch of files with attributes (the bulk path).
    let batch: Vec<unafs::BatchFile> = (0..6)
        .map(|i| {
            let mut attributes = BTreeMap::new();
            attributes.insert("n".to_string(), AttributeValue::Int(i));
            attributes.insert("kind".to_string(), AttributeValue::String(format!("k{}", i % 2)));
            unafs::BatchFile { name: format!("b{i}.dat"), data: vec![i as u8; 100 * i as usize], attributes }
        })
        .collect();
    fs.create_files_batch(docs, batch).unwrap();

    // Snapshot, then mutate, then a second snapshot dropped only to the
    // reclaim queue (so the queue object is NON-empty on disk).
    fs.snapshot_create("first".into(), "kernel".into(), 1234).unwrap();
    fs.rename(docs, "alpha.txt", root, "alpha-moved.txt").unwrap();
    fs.unlink(docs, "b1.dat").unwrap();
    fs.write_data(a, 0, b"rewritten after the snapshot").unwrap();
    let g2 = fs.snapshot_create("second".into(), "una".into(), 5678).unwrap();
    fs.write_data(frag, 0, &block_bytes(999)).unwrap();
    fs.snapshot_drop_enqueue(g2).unwrap();
    fs.commit().unwrap();

    fs.device.clone()
}

/// Mount the image and read every object back (the decoder on real bytes).
fn read_everything(device: MemDevice) -> u64 {
    let mut fs = UnaFS::mount(device).expect("mount");
    let root = fs.superblock.root_inode;
    let mut h = unafs::hash::FnvHasher::new();
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        for e in fs.ls(dir).unwrap() {
            h.write(e.name.as_bytes());
            h.write(&e.inode_id.to_le_bytes());
            let inode = fs.read_inode(e.inode_id).unwrap();
            if inode.kind == unafs::FileKind::Directory {
                stack.push(e.inode_id);
            } else {
                h.write(&fs.read_data(e.inode_id, 0, inode.size).unwrap());
            }
            for k in inode.attributes.keys().chain(inode.large_attributes.keys()) {
                let v = fs.get_attribute(e.inode_id, k).unwrap();
                h.write(format!("{k}={v:?}").as_bytes());
            }
        }
    }
    for s in fs.snapshot_index().unwrap() {
        h.write(format!("{s:?}").as_bytes());
    }
    for r in fs.reclaim_queue().unwrap() {
        h.write(format!("{r:?}").as_bytes());
    }
    h.finish()
}

fn image_digest(dev: &MemDevice) -> u64 {
    unafs::hash::hash_bytes(dev.as_bytes())
}

fn check(version: u32, image: u64, readback: u64) {
    let dev = build_volume(version, 2048);
    let got_image = image_digest(&dev);
    let got_read = read_everything(dev);
    println!("v{version}: image {got_image:#018x} readback {got_read:#018x}");
    assert_eq!(got_image, image, "v{version} image bytes drifted from the bincode-era image");
    assert_eq!(got_read, readback, "v{version} read-back drifted");
}

#[test]
fn v7_volume_is_byte_identical_to_the_bincode_era_image() {
    // NAMEINDEX (B432): the v6+ catalog trees now carry the name facts (`una:fsname`, the marker), so the IMAGE
    // digest was re-cut (bincode era 0xe605fd3f4d00e53b); the read-back digest — every record decoded — is unchanged.
    check(7, 0x7f8e1c92c1087a6e, 0x595c3ca52d9e1fdc);
}

#[test]
fn v6_volume_is_byte_identical_to_the_bincode_era_image() {
    // NAMEINDEX (B432): image re-cut as v7 (bincode era 0x9fd7ba0da19302b1); read-back unchanged.
    check(6, 0x729d2854b01b21cc, 0xb518397df279dc7f);
}

#[test]
fn v5_flat_catalog_volume_is_byte_identical_to_the_bincode_era_image() {
    check(5, 0x02e4528e3cbcdf0f, 0x21ec9cb5392bb133);
}

#[test]
fn v3_volume_is_byte_identical_to_the_bincode_era_image() {
    check(3, 0x13b9449adaad13a3, 0x8e52f31734e163b7);
}

