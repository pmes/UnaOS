//! UNAFSCODEC (SR53) M4: `unafs dump --records` cross-checked against the
//! library on volumes the unafs library writes.
//!
//! The dump walks raw blocks with the §R1 primitives from the spec (a second,
//! independent reading of `docs/dev/OS/09_FILESYSTEM/unafs-records.md`); the
//! library reads the same volume through its own decoders and the mount.
//! Every record the dump prints must agree, field by field, with what the
//! library returns: superblock, root, every inode (record, meta trailer,
//! inline + overflow extents), every attribute value (small and large),
//! every directory list, the flat catalog (v5), the snapshot index and the
//! reclaim queue.

use std::collections::{BTreeMap, HashMap};
use std::process::Command;
use unafs::{AttributeValue, BLOCK_SIZE, MemDevice, UnaFS};

const BS: usize = BLOCK_SIZE as usize;

fn build(version: u32) -> UnaFS<MemDevice> {
    let mut fs = UnaFS::format_with_version(MemDevice::with_blocks(2048), 0, version).unwrap();
    let root = fs.superblock.root_inode;
    let docs = fs.mkdir(root, "docs".into()).unwrap();
    let a = fs.create_file(docs, "alpha \u{2603}.txt".into()).unwrap();
    fs.write_data(a, 0, b"the crystal remembers").unwrap();
    fs.set_attribute(a, "rank".into(), AttributeValue::Int(-42)).unwrap();
    fs.set_attribute(a, "score".into(), AttributeValue::Float(3.25)).unwrap();
    fs.set_attribute(a, "title".into(), AttributeValue::String("Alpha".into())).unwrap();
    fs.set_attribute(a, "thumb".into(), AttributeValue::Blob(vec![0, 1, 0xFF])).unwrap();
    fs.set_attribute(a, "embed".into(), AttributeValue::Vector(vec![0.5, -1.0, 1e-7])).unwrap();
    fs.set_attribute(a, "big".into(), AttributeValue::Vector((0..100).map(|i| i as f32 / 3.0).collect()))
        .unwrap();
    fs.set_attribute(a, "big_str".into(), AttributeValue::String("y".repeat(400))).unwrap();
    let frag = fs.create_file(root, "frag.bin".into()).unwrap();
    let n = if version >= 4 { 200 } else { 30 };
    for j in 0..n {
        fs.write_data(frag, (2 * j * BS) as u64, &vec![j as u8; BS]).unwrap();
    }
    let mut attributes = BTreeMap::new();
    attributes.insert("n".to_string(), AttributeValue::Int(7));
    fs.create_files_batch(
        docs,
        vec![unafs::BatchFile { name: "b.dat".into(), data: vec![9; 300], attributes }],
    )
    .unwrap();
    fs.snapshot_create("first".into(), "kernel".into(), 1234).unwrap();
    fs.write_data(a, 0, b"after").unwrap();
    let g = fs.snapshot_create("second".into(), "una".into(), 99).unwrap();
    fs.write_data(frag, 0, &[1u8; 10]).unwrap();
    fs.snapshot_drop_enqueue(g).unwrap();
    fs
}

/// `tag field -> [values]` in dump order.
fn parse(dump: &str) -> HashMap<(String, String), Vec<String>> {
    let mut m: HashMap<(String, String), Vec<String>> = HashMap::new();
    for line in dump.lines() {
        if line.starts_with('#') {
            continue;
        }
        let Some((lhs, value)) = line.split_once(" = ") else { continue };
        let parts: Vec<&str> = lhs.split(' ').collect();
        if parts.len() < 4 || !parts[1].starts_with('@') {
            continue;
        }
        m.entry((parts[0].to_string(), parts[2].to_string())).or_default().push(value.to_string());
    }
    m
}

fn get<'a>(m: &'a HashMap<(String, String), Vec<String>>, tag: &str, field: &str) -> &'a [String] {
    m.get(&(tag.to_string(), field.to_string())).map(|v| v.as_slice()).unwrap_or(&[])
}

fn cross_check(version: u32) -> (usize, usize) {
    let mut fs = build(version);
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR"));
    let img = dir.join(format!("dump_records_v{version}.img"));
    std::fs::write(&img, fs.device.as_bytes()).unwrap();

    let out = Command::new(env!("CARGO_BIN_EXE_unafs"))
        .args(["dump", "--records", "--img", img.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(out.status.success(), "dump failed: {}", String::from_utf8_lossy(&out.stderr));
    let dump = String::from_utf8(out.stdout).unwrap();
    let m = parse(&dump);
    let mut checked = 0usize;
    let mut eq = |a: String, b: String, what: &str| {
        assert_eq!(a, b, "v{version} {what}");
        checked += 1;
    };

    let sb = fs.superblock.clone();
    eq(get(&m, "superblock", "Superblock.version")[0].clone(), sb.version.to_string(), "sb.version");
    eq(get(&m, "superblock", "Superblock.block_count")[0].clone(), sb.block_count.to_string(), "sb.block_count");
    eq(get(&m, "superblock", "Superblock.block_size")[0].clone(), sb.block_size.to_string(), "sb.block_size");
    eq(get(&m, "superblock", "Superblock.root_inode")[0].clone(), sb.root_inode.to_string(), "sb.root_inode");
    assert!(dump.contains(&format!("active slot")), "active slot named");
    let gen_line = dump.lines().find(|l| l.starts_with("active slot")).unwrap();
    eq(gen_line.rsplit(' ').next().unwrap().to_string(), fs.root_generation().to_string(), "root generation");

    let mut inodes = 0;
    for id in 1..100_000u64 {
        let Some(_) = fs.inode_block(id) else {
            if id > 64 {
                break;
            }
            continue;
        };
        inodes += 1;
        let ino = fs.read_inode(id).unwrap();
        let tag = format!("inode#{id}");
        eq(get(&m, &tag, "Inode.id")[0].clone(), id.to_string(), "Inode.id");
        eq(get(&m, &tag, "Inode.size")[0].clone(), ino.size.to_string(), "Inode.size");
        eq(
            get(&m, &tag, "Inode.kind")[0].split(' ').nth(1).unwrap().to_string(),
            format!("{:?}", ino.kind),
            "Inode.kind",
        );
        // Inline + overflow extents == the library's full list.
        let mut ext: Vec<String> = get(&m, &tag, "Inode.chunks[]").to_vec();
        ext.extend(get(&m, &format!("overflow#{id}"), "Vec<Extent>[]").iter().cloned());
        let lib: Vec<String> = ino
            .chunks
            .iter()
            .map(|e| format!("logical_offset={} physical_block={} length={}", e.logical_offset, e.physical_block, e.length))
            .collect();
        eq(format!("{ext:?}"), format!("{lib:?}"), "extents (inline + overflow)");
        // Small attributes, in key order.
        let keys = get(&m, &tag, "Inode.attributes.key");
        let vals = get(&m, &tag, "Inode.attributes.value");
        let lib_k: Vec<String> = ino.attributes.keys().map(|k| format!("{k:?}")).collect();
        let lib_v: Vec<String> = ino.attributes.values().map(|v| format!("{v:?}")).collect();
        eq(format!("{keys:?}"), format!("{lib_k:?}"), "attribute keys");
        eq(format!("{vals:?}"), format!("{lib_v:?}"), "attribute values");
        // Large attributes: the dumped value == the library's get_attribute.
        for k in ino.large_attributes.keys() {
            let v = get(&m, &format!("large#{id}:{k}"), "AttributeValue");
            let lib = fs.get_attribute(id, k).unwrap().unwrap();
            eq(v[0].clone(), format!("{lib:?}"), "large attribute value");
        }
        // Meta trailer (v6+).
        if version >= 6 {
            eq(get(&m, &format!("{tag}.meta"), "Meta.parent")[0].clone(), ino.parent.to_string(), "meta parent");
            let name = get(&m, &format!("{tag}.meta"), "Meta.name").first().cloned();
            eq(format!("{name:?}"), format!("{:?}", ino.name.as_ref().map(|n| format!("{n:?}"))), "meta name");
        } else {
            assert!(get(&m, &format!("{tag}.meta"), "Meta.parent").is_empty(), "no meta trailer pre-v6");
        }
        // Directory lists.
        if ino.kind == unafs::FileKind::Directory {
            let names = get(&m, &format!("data#{id}"), "DirEntry.name");
            let ids = get(&m, &format!("data#{id}"), "DirEntry.inode_id");
            let ls = fs.ls(id).unwrap();
            let lib_n: Vec<String> = ls.iter().map(|e| format!("{:?}", e.name)).collect();
            let lib_i: Vec<String> = ls.iter().map(|e| e.inode_id.to_string()).collect();
            eq(format!("{names:?}"), format!("{lib_n:?}"), "dir names");
            eq(format!("{ids:?}"), format!("{lib_i:?}"), "dir ids");
        }
    }
    // The spill trailer was exercised (v4+).
    if version >= 4 {
        assert!(dump.contains("IndirectTrailer.overflow_len"), "a spilled inode was dumped");
    }
    // Snapshot index and reclaim queue.
    let snaps = fs.snapshot_index().unwrap();
    let names = get(&m, "data#3", "SnapshotEntry.name");
    let lib_n: Vec<String> = snaps.iter().map(|s| format!("{:?}", s.name)).collect();
    eq(format!("{names:?}"), format!("{lib_n:?}"), "snapshot names");
    let gens = get(&m, "data#3", "SnapshotEntry.generation");
    let lib_g: Vec<String> = snaps.iter().map(|s| s.generation.to_string()).collect();
    eq(format!("{gens:?}"), format!("{lib_g:?}"), "snapshot generations");
    let q = fs.reclaim_queue().unwrap();
    assert!(!q.is_empty(), "reclaim queue is non-empty in the fixture");
    let blocks = get(&m, "data#4", "ReclaimEntry.blocks[]");
    let lib_b: Vec<String> = q.iter().flat_map(|r| r.blocks.iter().map(|b| b.to_string())).collect();
    eq(format!("{blocks:?}"), format!("{lib_b:?}"), "reclaim blocks");
    // Catalog: v6+ record, v3–v5 flat list.
    if version >= 6 {
        let rec = fs.catalog_record().unwrap().unwrap();
        eq(get(&m, "data#2", "CatalogRecord.eq_root")[0].clone(), rec.eq_root.to_string(), "catalog eq_root");
        eq(get(&m, "data#2", "CatalogRecord.entries")[0].clone(), rec.entries.to_string(), "catalog entries");
    } else {
        assert!(!get(&m, "data#2", "CatalogEntry.inode_id").is_empty(), "flat catalog dumped");
    }
    let _ = std::fs::remove_file(&img);
    println!("v{version}: {inodes} inodes, {checked} field groups cross-checked, {} dump lines", dump.lines().count());
    (inodes, checked)
}

#[test]
fn dump_records_agrees_with_the_library_v7() {
    cross_check(7);
}

#[test]
fn dump_records_agrees_with_the_library_v6() {
    cross_check(6);
}

#[test]
fn dump_records_agrees_with_the_library_v5_flat_catalog() {
    cross_check(5);
}
