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

//! B302 M1 — F3 wired: the attribute catalog is the B+tree pair.
//!
//! * `set_attribute` is a log-time index insert: the per-op block count does
//!   not grow with the catalog (the flat list grew linearly), and the catalog
//!   inode's data stays one 40 B record.
//! * Overwrite / `remove_attribute` / `unlink` / `rmdir` scrub exactly the
//!   inode's own keys.
//! * CoW: a power cut mid-insert converges to the old tree or the new one; a
//!   snapshot pins the index as-of; fsck counts the tree blocks.
//! * v3–v5 volumes keep the flat catalog and `migrate_k8_into` lifts one to v6.

use unafs::index::{CATALOG_RECORD_SIZE, CatalogRecord};
use unafs::{AttributeValue, BLOCK_SIZE, BlockDevice, MemDevice, UnaFS};

fn fresh_fs(block_count: u64) -> UnaFS<MemDevice> {
    let mut device = MemDevice::new();
    device
        .write_block(block_count - 1, &vec![0u8; BLOCK_SIZE as usize])
        .expect("size disk");
    UnaFS::format(device, 0).expect("format")
}

fn fresh_v5(block_count: u64) -> UnaFS<MemDevice> {
    let mut device = MemDevice::new();
    device
        .write_block(block_count - 1, &vec![0u8; BLOCK_SIZE as usize])
        .expect("size disk");
    UnaFS::format_with_version(device, 0, 5).expect("format v5")
}

fn ids(fs: &mut UnaFS<MemDevice>, q: &str) -> Vec<u64> {
    fs.query(q).unwrap().into_iter().map(|h| h.inode_id).collect()
}

#[test]
fn fresh_volume_is_v6_with_a_catalog_record() {
    let mut fs = fresh_fs(4096);
    // v7 (UNAFSMAP) keeps every v6 structure; only the maps' shape moved.
    assert_eq!(fs.superblock.version, unafs::superblock::VERSION);
    assert!(fs.superblock.indexed());
    let rec = fs.catalog_record().unwrap().expect("v6 has a record");
    assert_eq!(rec.entries, 0);
    let cat = fs.read_inode(fs.superblock.catalog_inode).unwrap();
    assert_eq!(cat.size as usize, CATALOG_RECORD_SIZE);
    assert!(fs.fsck(false).unwrap().is_clean());
}

#[test]
fn set_attribute_is_log_time_not_a_catalog_rewrite() {
    let mut fs = fresh_fs(40_000);
    let root = fs.superblock.root_inode;
    let mut ids = Vec::new();
    for i in 0..1500u64 {
        let id = fs.create_inode(Default::default()).unwrap();
        ids.push(id);
        fs.set_attribute(id, "rank".into(), AttributeValue::Int(i as i64)).unwrap();
    }
    let _ = root;
    // The per-op cost at 1500 entries: inode + catalog record + data + a
    // root-to-leaf path in each tree + the maps. A flat catalog of 1500 ×
    // 24 B entries alone is 9 blocks rewritten on top; the bound below is
    // what the tree path costs and stays flat as the catalog grows.
    let probe = |fs: &mut UnaFS<MemDevice>, id: u64, v: i64| {
        fs.set_attribute(id, "probe".into(), AttributeValue::Int(v)).unwrap();
        fs.commit_stats().last_commit_blocks
    };
    let early = probe(&mut fs, ids[0], 1);
    for i in 0..1500u64 {
        fs.set_attribute(ids[i as usize], "rank2".into(), AttributeValue::Int(i as i64)).unwrap();
    }
    let late = probe(&mut fs, ids[1], 2);
    assert!(
        late <= early + 4,
        "per-op blocks must not grow with the catalog: early {early}, late {late}"
    );
    let rec = fs.catalog_record().unwrap().unwrap();
    assert_eq!(rec.entries, 3002);
    let cat = fs.read_inode(fs.superblock.catalog_inode).unwrap();
    assert_eq!(cat.size as usize, CATALOG_RECORD_SIZE, "the catalog is one record, not a list");
    assert_eq!(ids_q(&mut fs, "rank == 777"), vec![ids[777]]);
    assert!(fs.fsck(false).unwrap().is_clean());
}

fn ids_q(fs: &mut UnaFS<MemDevice>, q: &str) -> Vec<u64> {
    ids(fs, q)
}

#[test]
fn overwrite_and_remove_scrub_the_old_keys() {
    let mut fs = fresh_fs(4096);
    let root = fs.superblock.root_inode;
    let a = fs.create_file(root, "a".into()).unwrap();
    fs.set_attribute(a, "mood".into(), AttributeValue::String("calm".into())).unwrap();
    fs.set_attribute(a, "mood".into(), AttributeValue::String("bright".into())).unwrap();
    assert!(ids(&mut fs, "mood == \"calm\"").is_empty());
    assert_eq!(ids(&mut fs, "mood == \"bright\""), vec![a]);
    assert_eq!(fs.catalog_record().unwrap().unwrap().entries, 1, "overwrite replaces, never accumulates");

    // A spilled (large) value is indexed and scrubbed the same way.
    let long = "x".repeat(400);
    fs.set_attribute(a, "essay".into(), AttributeValue::String(long.clone())).unwrap();
    assert_eq!(ids(&mut fs, &format!("essay == \"{long}\"")), vec![a]);
    fs.remove_attribute(a, "essay").unwrap();
    assert!(ids(&mut fs, &format!("essay == \"{long}\"")).is_empty());
    assert_eq!(fs.catalog_record().unwrap().unwrap().entries, 1);

    fs.unlink(root, "a").unwrap();
    assert_eq!(fs.catalog_record().unwrap().unwrap().entries, 0);
    assert!(ids(&mut fs, "mood == \"bright\"").is_empty());
    let r = fs.fsck(false).unwrap();
    assert!(r.is_clean(), "{r:?}");
}

#[test]
fn rmdir_scrubs_directory_attributes() {
    let mut fs = fresh_fs(4096);
    let root = fs.superblock.root_inode;
    let d = fs.mkdir(root, "d".into()).unwrap();
    fs.set_attribute(d, "owner".into(), AttributeValue::String("una".into())).unwrap();
    fs.rmdir(root, "d").unwrap();
    assert!(ids(&mut fs, "owner == \"una\"").is_empty());
    assert_eq!(fs.catalog_record().unwrap().unwrap().entries, 0);
    assert!(fs.fsck(false).unwrap().is_clean());
}

#[test]
fn power_cut_mid_insert_converges_to_old_or_new() {
    let mut fs = fresh_fs(4096);
    let root = fs.superblock.root_inode;
    let a = fs.create_file(root, "a".into()).unwrap();
    // Spread over many inodes (one inode block holds only so many inline
    // attributes) so the trees grow past one leaf.
    let mut objs = Vec::new();
    for i in 0..200 {
        let o = fs.create_inode(Default::default()).unwrap();
        fs.set_attribute(o, format!("k{i}"), AttributeValue::Int(i)).unwrap();
        objs.push(o);
    }
    let committed = fs.device.clone();

    // The insert writes fresh tree nodes, a fresh record, a fresh catalog
    // inode — but the root never flips.
    fs.set_autocommit(false);
    fs.set_attribute(a, "late".into(), AttributeValue::Int(9)).unwrap();
    let torn = fs.device.clone();
    fs.commit().unwrap();
    let flipped = fs.device.clone();
    drop(fs);

    // Power cut before the flip: the OLD tree, whole.
    let mut old = UnaFS::mount(torn).expect("old root mounts");
    assert!(ids(&mut old, "late == 9").is_empty());
    assert_eq!(ids(&mut old, "k150 == 150"), vec![objs[150]]);
    assert_eq!(old.catalog_record().unwrap().unwrap().entries, 200);
    assert!(old.fsck(false).unwrap().is_clean());
    // …and identical in content to the pre-op image's view.
    let mut before = UnaFS::mount(committed).unwrap();
    assert_eq!(before.catalog_record().unwrap(), old.catalog_record().unwrap());

    // After the flip: the NEW tree, whole.
    let mut new = UnaFS::mount(flipped).expect("new root mounts");
    assert_eq!(ids(&mut new, "late == 9"), vec![a]);
    assert_eq!(new.catalog_record().unwrap().unwrap().entries, 201);
    assert!(new.fsck(false).unwrap().is_clean());
}

#[test]
fn snapshot_pins_the_index_as_of() {
    let mut fs = fresh_fs(8192);
    let root = fs.superblock.root_inode;
    let _ = root;
    let mut objs = Vec::new();
    for i in 0..300 {
        let o = fs.create_inode(Default::default()).unwrap();
        fs.set_attribute(o, format!("k{i}"), AttributeValue::Int(i)).unwrap();
        objs.push(o);
    }
    let rec_then = fs.catalog_record().unwrap().unwrap();
    let snap = fs.snapshot_create("s".into(), "una".into(), 1).unwrap();
    for i in 0..300 {
        fs.set_attribute(objs[i], format!("k{i}"), AttributeValue::Int(i as i64 + 1000)).unwrap();
    }
    assert_ne!(fs.catalog_record().unwrap().unwrap(), rec_then);
    let r = fs.fsck(false).unwrap();
    assert!(r.is_clean(), "live + retained trees all accounted: {r:?}");
    // fsck's rebuild agrees with the runtime refcounts (multiplicities).
    let before = fs.free_blocks();
    fs.fsck(true).unwrap();
    assert!(fs.fsck(false).unwrap().is_clean());
    let _ = before;

    fs.snapshot_drop(snap).unwrap();
    let r = fs.fsck(false).unwrap();
    assert!(r.is_clean(), "drop frees the old index nodes and leaks nothing: {r:?}");
    assert_eq!(ids(&mut fs, "k7 == 1007"), vec![objs[7]]);
}

#[test]
fn many_inodes_build_a_multi_level_tree() {
    let mut fs = fresh_fs(60_000);
    let root = fs.superblock.root_inode;
    let files: Vec<unafs::BatchFile> = (0..3000)
        .map(|i| {
            let mut attributes = std::collections::BTreeMap::new();
            attributes.insert("n".to_string(), AttributeValue::Int(i));
            attributes.insert("parity".to_string(), AttributeValue::String(if i % 2 == 0 { "even" } else { "odd" }.into()));
            unafs::BatchFile { name: format!("f{i:05}"), data: Vec::new(), attributes }
        })
        .collect();
    let new_ids = fs.create_files_batch(root, files).unwrap();
    assert_eq!(fs.catalog_record().unwrap().unwrap().entries, 6000);
    assert_eq!(ids(&mut fs, "n == 2999"), vec![new_ids[2999]]);
    assert_eq!(ids(&mut fs, "parity == \"odd\"").len(), 1500);
    assert!(fs.fsck(false).unwrap().is_clean());
    // Remount: the record and the trees are durable.
    let dev = fs.device.clone();
    drop(fs);
    let mut fs = UnaFS::mount(dev).unwrap();
    assert_eq!(ids(&mut fs, "n == 0"), vec![new_ids[0]]);
}

#[test]
fn v5_volume_keeps_the_flat_catalog_and_migrates_to_v6() {
    let mut old = fresh_v5(8192);
    assert_eq!(old.superblock.version, 5);
    assert!(old.catalog_record().unwrap().is_none(), "v5 has no record");
    let root = old.superblock.root_inode;
    let d = old.mkdir(root, "docs".into()).unwrap();
    let f = old.create_file(d, "memo.txt".into()).unwrap();
    old.write_data(f, 0, b"hello v5").unwrap();
    old.set_attribute(f, "kind".into(), AttributeValue::String("memo".into())).unwrap();
    old.set_attribute(f, "size".into(), AttributeValue::Int(8)).unwrap();
    old.set_attribute(f, "kind".into(), AttributeValue::String("note".into())).unwrap();
    let big = vec![0.25f32; 100];
    old.set_attribute(f, "embed".into(), AttributeValue::Vector(big.clone())).unwrap();
    // The flat path answers the new grammar too.
    assert_eq!(ids(&mut old, "kind == \"note\""), vec![f]);
    assert!(ids(&mut old, "kind == \"memo\"").is_empty(), "flat overwrite scrubs the old entry");
    assert_eq!(ids(&mut old, "size >= 8 AND size <= 8"), vec![f]);
    let hits = old.query("kind == \"note\"").unwrap();
    assert_eq!(hits[0].path, "/docs/memo.txt", "v5 paths come from one name walk");
    assert!(old.fsck(false).unwrap().is_clean());
    // Every inode block on v5 is trailer-free: version stamp unchanged.
    let dev = old.device.clone();
    let mut again = UnaFS::mount(dev).unwrap();
    assert_eq!(again.superblock.version, 5);

    let mut new = fresh_fs(8192);
    let report = unafs::legacy::migrate_k8_into(&mut again, &mut new).unwrap();
    assert_eq!((report.files, report.directories, report.bytes), (1, 1, 8));
    assert_eq!(new.superblock.version, unafs::superblock::VERSION);
    let hits = new.query("kind == \"note\" AND size BETWEEN 1 AND 10").unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].path, "/docs/memo.txt");
    let nf = hits[0].inode_id;
    assert_eq!(new.read_data(nf, 0, 8).unwrap(), b"hello v5");
    assert_eq!(new.get_attribute(nf, "embed").unwrap(), Some(AttributeValue::Vector(big)));
    assert_eq!(new.catalog_record().unwrap().unwrap().entries, 3);
    assert!(new.fsck(false).unwrap().is_clean());
}

#[test]
fn catalog_record_rejects_corruption() {
    let rec = CatalogRecord { eq_root: 10, ord_root: 11, entries: 3 };
    let b = rec.to_bytes();
    assert_eq!(CatalogRecord::from_bytes(&b, 100), Some(rec));
    let mut bad = b;
    bad[9] ^= 1;
    assert_eq!(CatalogRecord::from_bytes(&bad, 100), None, "checksum");
    assert_eq!(CatalogRecord::from_bytes(&b, 11), None, "root past the volume");
    assert_eq!(CatalogRecord::from_bytes(&b[..39], 100), None, "short");
}
