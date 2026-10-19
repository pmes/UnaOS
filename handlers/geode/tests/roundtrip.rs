// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! GEODEMETA (LEDGER SR148): UnaFS → .geode → UnaFS keeps every record whole; a plain (FAT) extraction warns
//! once, before writing, naming what it drops; a damaged archive is refused before anything is written.
//! The scratch images are in-RAM `MemDevice` volumes through the unafs crate; the plain target is a directory
//! under cargo's per-test tmp (`CARGO_TARGET_TMPDIR`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use una_abi::attr_keys as k;
use unafs::{AttributeValue, BLOCK_SIZE, BlockDevice, FileKind, MemDevice, UnaFS};

/// A 16 MiB scratch volume.
fn volume() -> UnaFS<MemDevice> {
    let mut d = MemDevice::new();
    d.write_block(4095, &vec![0u8; BLOCK_SIZE as usize]).unwrap();
    UnaFS::format(d, 0).unwrap()
}

/// /src: two directories and 20 files, each with an owner, a mode, a `una:*` type, and some with spilled
/// (large) values; distinct ctime/mtime/atime on every entry.
fn populate(fs: &mut UnaFS<MemDevice>) -> usize {
    let root = fs.superblock.root_inode;
    let src = fs.mkdir(root, "src".into()).unwrap();
    let docs = fs.mkdir(src, "docs".into()).unwrap();
    let deep = fs.mkdir(docs, "deep".into()).unwrap();
    fs.set_attribute(docs, k::VIEW_MODE.into(), AttributeValue::String("list".into())).unwrap();
    fs.set_attribute(deep, "owner".into(), AttributeValue::String("peter".into())).unwrap();
    let mut ids = vec![docs, deep];
    for i in 0..20u64 {
        let parent = [src, docs, deep][(i % 3) as usize];
        let id = fs.create_file(parent, format!("file-{i:02}.dat")).unwrap();
        let data: Vec<u8> = (0..(i * 997) as usize).map(|b| (b as u64 * 31 + i) as u8).collect();
        if !data.is_empty() {
            fs.write_data(id, 0, &data).unwrap();
        }
        let mut a: BTreeMap<String, AttributeValue> = BTreeMap::new();
        a.insert("owner".into(), AttributeValue::String(if i % 2 == 0 { "peter" } else { "una" }.into()));
        a.insert("mode".into(), AttributeValue::Int(0o644 + i as i64));
        a.insert(k::TYPE.into(), AttributeValue::String("application/octet-stream".into()));
        a.insert(k::MEDIA_WIDTH.into(), AttributeValue::Int(640 + i as i64));
        a.insert("score".into(), AttributeValue::Float(0.25 * i as f64 - 1.5));
        if i % 3 == 0 {
            a.insert(k::EMBED_DIMS.into(), AttributeValue::Int(100));
            a.insert("vec".into(), AttributeValue::Vector((0..100).map(|x| x as f32 / 7.0).collect()));
        }
        if i % 4 == 0 {
            a.insert(k::ICON_32.into(), AttributeValue::Blob((0..400u32).map(|x| (x ^ i as u32) as u8).collect()));
        }
        if i % 5 == 0 {
            a.insert(k::DOC_TITLE.into(), AttributeValue::String("t".repeat(300 + i as usize)));
        }
        for (key, v) in a {
            fs.set_attribute(id, key, v).unwrap();
        }
        ids.push(id);
    }
    for (n, id) in ids.iter().enumerate() {
        let base = 1_600_000_000 + n as u64 * 10;
        fs.set_times(*id, base, base + 3, base + 7).unwrap();
    }
    ids.len()
}

/// Every record field that is not a location, compared per path (small/large split included).
fn assert_records_equal(a: &mut UnaFS<MemDevice>, ad: &str, b: &mut UnaFS<MemDevice>, bd: &str) -> usize {
    let ea = geode::records(a, ad).unwrap();
    let eb = geode::records(b, bd).unwrap();
    assert_eq!(ea.len(), eb.len());
    for (x, y) in ea.iter().zip(&eb) {
        assert_eq!(x, y, "entry {}", x.path);
        let ida = a.resolve_path(&format!("{ad}/{}", x.path)).unwrap();
        let idb = b.resolve_path(&format!("{bd}/{}", y.path)).unwrap();
        let (ia, ib) = (a.read_inode(ida).unwrap(), b.read_inode(idb).unwrap());
        assert_eq!(ia.kind, ib.kind, "{}", x.path);
        assert_eq!(ia.size, ib.size, "{}", x.path);
        assert_eq!(ia.attributes, ib.attributes, "{}: small attributes", x.path);
        assert_eq!(
            ia.large_attributes.keys().collect::<Vec<_>>(),
            ib.large_attributes.keys().collect::<Vec<_>>(),
            "{}: spilled keys",
            x.path
        );
        assert_eq!((ia.ctime, ia.mtime, ia.atime), (ib.ctime, ib.mtime, ib.atime), "{}: times", x.path);
        assert_eq!(ia.name, ib.name, "{}", x.path);
        // The §R1 record bytes of the portable part are byte-identical.
        let bytes = |m: &BTreeMap<String, AttributeValue>| {
            let mut w = unafs::codec::Writer::new();
            w.map(m);
            w.into_bytes()
        };
        assert_eq!(bytes(&ia.attributes), bytes(&ib.attributes), "{}: §R5 bytes", x.path);
    }
    ea.len()
}

#[test]
fn unafs_to_geode_to_unafs_keeps_every_record() {
    let mut a = volume();
    let n = populate(&mut a);
    assert_eq!(n, 22);
    let archive = geode::pack(&mut a, "/src").unwrap();

    let mut b = volume();
    let root = b.superblock.root_inode;
    b.mkdir(root, "dst".into()).unwrap();
    let rep = geode::unpack(&mut b, &archive, "/dst").unwrap();
    assert_eq!((rep.files, rep.dirs), (20, 2));

    assert_eq!(assert_records_equal(&mut a, "/src", &mut b, "/dst"), 22);
    // The archive re-packed from the second volume is byte-identical.
    assert_eq!(geode::pack(&mut b, "/dst").unwrap(), archive);

    // And it survives a remount.
    let dev = std::mem::replace(&mut b.device, MemDevice::new());
    drop(b);
    let mut b = UnaFS::mount(dev).unwrap();
    assert_eq!(assert_records_equal(&mut a, "/src", &mut b, "/dst"), 22);
    assert_eq!(geode::pack(&mut b, "/dst").unwrap(), archive);
    let deep = b.resolve_path("/dst/docs/deep").unwrap();
    let s = b.stat(deep).unwrap();
    assert_eq!((s.ctime, s.mtime, s.atime), (1_600_000_010, 1_600_000_013, 1_600_000_017));
}

#[test]
fn unpack_refuses_a_taken_name_before_writing() {
    let mut a = volume();
    populate(&mut a);
    let archive = geode::pack(&mut a, "/src").unwrap();
    let mut b = volume();
    let root = b.superblock.root_inode;
    let dst = b.mkdir(root, "dst".into()).unwrap();
    b.mkdir(dst, "docs".into()).unwrap();
    match geode::unpack(&mut b, &archive, "/dst") {
        Err(geode::GeodeError::Exists(p)) => assert_eq!(p, "docs"),
        other => panic!("expected Exists, got {other:?}"),
    }
    assert_eq!(b.ls(dst).unwrap().len(), 1, "nothing written");
}

#[test]
fn a_damaged_archive_is_refused() {
    let mut a = volume();
    populate(&mut a);
    let archive = geode::pack(&mut a, "/src").unwrap();
    let mut flipped = archive.clone();
    flipped[40] ^= 1;
    assert!(matches!(geode::read_archive(&flipped), Err(geode::GeodeError::Format(_))));
    assert!(matches!(geode::read_archive(&archive[..archive.len() - 1]), Err(geode::GeodeError::Format(_))));

    // A well-checksummed archive naming "..", or a child before its directory, is refused too.
    let entry = |path: &str, kind| geode::Entry {
        path: path.into(),
        kind,
        ctime: 1,
        mtime: 2,
        atime: 3,
        attributes: BTreeMap::new(),
        data: Vec::new(),
    };
    for bad in [vec![entry("../x", FileKind::File)], vec![entry("d/x", FileKind::File), entry("d", FileKind::Directory)]] {
        assert!(matches!(geode::read_archive(&geode::write_archive(&bad)), Err(geode::GeodeError::Format(_))));
    }
    let ok = vec![entry("d", FileKind::Directory), entry("d/x", FileKind::File)];
    assert_eq!(geode::read_archive(&geode::write_archive(&ok)).unwrap(), ok);
}

#[test]
fn plain_extraction_warns_once_before_writing() {
    let mut a = volume();
    populate(&mut a);
    let archive = geode::pack(&mut a, "/src").unwrap();
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("geodemeta-plain");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    let mut warnings: Vec<String> = Vec::new();
    let rep = geode::unpack_plain_with(&archive, &dir, &mut |w| {
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "the warning comes before the first write");
        warnings.push(w.to_string());
    })
    .unwrap();
    assert_eq!(warnings.len(), 1);
    let w = &warnings[0];
    for key in ["owner", "mode", k::TYPE, k::ICON_32, k::DOC_TITLE, k::VIEW_MODE, "ctime"] {
        assert!(w.contains(key), "warning names {key}: {w}");
    }
    assert_eq!(rep.warning, *w);
    assert_eq!((rep.files, rep.dirs), (20, 2));

    // Names, bytes and mtime landed.
    for e in geode::records(&mut a, "/src").unwrap() {
        let p = dir.join(&e.path);
        let md = std::fs::metadata(&p).unwrap();
        let mtime = md.modified().unwrap().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs();
        assert_eq!(mtime, e.mtime, "{}", e.path);
        if e.kind == FileKind::File {
            assert_eq!(std::fs::read(&p).unwrap(), e.data, "{}", e.path);
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}
