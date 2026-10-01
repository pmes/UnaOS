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

//! B302 M3 — queries return paths. v6 inodes carry a parent pointer + name in
//! their meta trailer; `query` derives each hit's path in O(depth) inode reads;
//! `mkdir`/create/batch stamp it, `rename` restamps it (same transaction), fsck
//! checks it against the name tree and repair restamps it.

use std::collections::BTreeMap;
use unafs::inode::INODE_META_MAGIC;
use unafs::{AttributeValue, BLOCK_SIZE, BatchFile, BlockDevice, MemDevice, UnaFS};

fn fresh_fs() -> UnaFS<MemDevice> {
    let mut device = MemDevice::new();
    device.write_block(8191, &vec![0u8; BLOCK_SIZE as usize]).unwrap();
    UnaFS::format(device, 0).unwrap()
}

fn paths(fs: &mut UnaFS<MemDevice>, q: &str) -> Vec<String> {
    fs.query(q).unwrap().into_iter().map(|h| h.path).collect()
}

fn tag(fs: &mut UnaFS<MemDevice>, id: u64, v: &str) {
    fs.set_attribute(id, "tag".into(), AttributeValue::String(v.into())).unwrap();
}

#[test]
fn hits_carry_their_paths() {
    let mut fs = fresh_fs();
    let root = fs.superblock.root_inode;
    let a = fs.mkdir(root, "a".into()).unwrap();
    let b = fs.mkdir(a, "b".into()).unwrap();
    let c = fs.create_file(b, "c.txt".into()).unwrap();
    tag(&mut fs, c, "x");
    tag(&mut fs, a, "x");
    tag(&mut fs, root, "x");
    let bare = fs.create_inode(BTreeMap::new()).unwrap();
    tag(&mut fs, bare, "x");
    let hits = fs.query("tag == \"x\"").unwrap();
    let got: Vec<(u64, &str)> = hits.iter().map(|h| (h.inode_id, h.path.as_str())).collect();
    assert_eq!(got, vec![(root, "/"), (a, "/a"), (c, "/a/b/c.txt"), (bare, "")]);
    assert_eq!(fs.path_of(b).unwrap(), "/a/b");
    let st = fs.stat(c).unwrap();
    assert_eq!(st.parent, b);
    assert!(fs.fsck(false).unwrap().is_clean());
}

#[test]
fn rename_and_move_restamp_the_link() {
    let mut fs = fresh_fs();
    let root = fs.superblock.root_inode;
    let a = fs.mkdir(root, "a".into()).unwrap();
    let z = fs.mkdir(root, "z".into()).unwrap();
    let f = fs.create_file(a, "f".into()).unwrap();
    tag(&mut fs, f, "m");
    fs.rename(a, "f", a, "g").unwrap();
    assert_eq!(paths(&mut fs, "tag == \"m\""), vec!["/a/g"]);
    fs.rename(a, "g", z, "h").unwrap();
    assert_eq!(paths(&mut fs, "tag == \"m\""), vec!["/z/h"]);
    // Moving a directory moves every descendant's derived path with it.
    fs.rename(root, "z", a, "zz").unwrap();
    assert_eq!(paths(&mut fs, "tag == \"m\""), vec!["/a/zz/h"]);
    assert!(fs.fsck(false).unwrap().is_clean());
    // Remount: the links are durable.
    let dev = fs.device.clone();
    drop(fs);
    let mut fs = UnaFS::mount(dev).unwrap();
    assert_eq!(paths(&mut fs, "tag == \"m\""), vec!["/a/zz/h"]);
}

#[test]
fn power_cut_mid_rename_keeps_the_old_link() {
    let mut fs = fresh_fs();
    let root = fs.superblock.root_inode;
    let f = fs.create_file(root, "old".into()).unwrap();
    tag(&mut fs, f, "p");
    fs.set_autocommit(false);
    fs.rename(root, "old", root, "new").unwrap();
    let torn = fs.device.clone();
    drop(fs);
    let mut fs = UnaFS::mount(torn).unwrap();
    assert_eq!(paths(&mut fs, "tag == \"p\""), vec!["/old"]);
    assert!(fs.fsck(false).unwrap().is_clean());
}

#[test]
fn batch_long_names_and_deep_chains() {
    let mut fs = fresh_fs();
    let root = fs.superblock.root_inode;
    let d = fs.mkdir(root, "batch".into()).unwrap();
    let files: Vec<BatchFile> = (0..20)
        .map(|i| {
            let mut attributes = BTreeMap::new();
            attributes.insert("i".to_string(), AttributeValue::Int(i));
            BatchFile { name: format!("b{i:02}"), data: vec![], attributes }
        })
        .collect();
    fs.create_files_batch(d, files).unwrap();
    assert_eq!(paths(&mut fs, "i BETWEEN 3 AND 4"), vec!["/batch/b03", "/batch/b04"]);

    // A name longer than the trailer stores: the path asks the parent.
    let long = "n".repeat(300);
    let l = fs.create_file(d, long.clone()).unwrap();
    assert_eq!(fs.read_inode(l).unwrap().name, None);
    tag(&mut fs, l, "long");
    assert_eq!(paths(&mut fs, "tag == \"long\""), vec![format!("/batch/{long}")]);

    // 60 levels deep: O(depth) inode reads, no name-tree walk.
    let mut cur = root;
    let mut want = String::new();
    for i in 0..60 {
        cur = fs.mkdir(cur, format!("d{i}")).unwrap();
        want.push_str(&format!("/d{i}"));
    }
    tag(&mut fs, cur, "deep");
    assert_eq!(paths(&mut fs, "tag == \"deep\""), vec![want]);
    assert!(fs.fsck(false).unwrap().is_clean());
}

#[test]
fn fsck_detects_and_repairs_a_forged_parent_link() {
    let mut fs = fresh_fs();
    let root = fs.superblock.root_inode;
    let a = fs.mkdir(root, "a".into()).unwrap();
    let f = fs.create_file(a, "f".into()).unwrap();
    // Media corruption: rewrite the parent field of f's meta trailer IN
    // PLACE (no CoW — this is the disk lying, not the file system).
    let pb = fs.inode_block(f).unwrap();
    let mut block = vec![0u8; BLOCK_SIZE as usize];
    fs.device.read_block(pb, &mut block).unwrap();
    let magic = INODE_META_MAGIC.to_le_bytes();
    let off = block.windows(8).position(|w| w == magic).expect("v6 trailer present");
    block[off + 8..off + 16].copy_from_slice(&root.to_le_bytes());
    fs.device.write_block(pb, &block).unwrap();

    let r = fs.fsck(false).unwrap();
    assert_eq!(r.bad_parent_links, vec![f]);
    assert!(!r.is_clean());
    let r = fs.fsck(true).unwrap();
    assert_eq!(r.bad_parent_links, vec![f]);
    let r = fs.fsck(false).unwrap();
    assert!(r.is_clean(), "{r:?}");
    assert_eq!(fs.path_of(f).unwrap(), "/a/f");
}
