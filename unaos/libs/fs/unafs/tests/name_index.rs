// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// NAMEINDEX (rmbp-ledger B432) KATs: the `una:fsname` facts the core writes on create, rename, unlink and rmdir;
// readiness (format marks; an unmarked volume answers nothing until built); the chunked build; reindex; fsck.
use unafs::{BLOCK_SIZE, BatchFile, BlockDevice, MemDevice, UnaFS, name_keys, name_match};

fn fresh() -> UnaFS<MemDevice> {
    let mut device = MemDevice::new();
    device.write_block(3999, &vec![0u8; BLOCK_SIZE as usize]).unwrap();
    UnaFS::format(device, 20).unwrap()
}

fn paths(fs: &mut UnaFS<MemDevice>, q: &str) -> Vec<String> {
    fs.find_names(q, 64).unwrap().hits.into_iter().map(|h| h.path).collect()
}

#[test]
fn kat_name_keys_and_match() {
    assert_eq!(name_keys("My_Photo.PNG"), vec!["my_photo.png", "photo.png", "png"]);
    assert_eq!(name_keys("a..b"), vec!["a..b", "b"]);
    assert!(name_keys("").is_empty());
    assert_eq!(name_keys("a.b.c.d.e.f.g.h.i.j").len(), 8);
    assert_eq!(name_match("Test plan.md", "TEST"), Some(2));
    assert_eq!(name_match("Test plan.md", "pla"), Some(1));
    assert_eq!(name_match("Test plan.md", "lan"), None);
    assert_eq!(name_match("x", ""), None);
}

#[test]
fn kat_prefix_case_rename_unlink_rmdir() {
    let mut fs = fresh();
    let root = fs.superblock.root_inode;
    assert!(fs.name_index_ready().unwrap());
    let docs = fs.mkdir(root, "Docs".to_string()).unwrap();
    fs.create_file(docs, "Report-2026.TXT".to_string()).unwrap();
    assert_eq!(paths(&mut fs, "rep"), vec!["/Docs/Report-2026.TXT"]);
    assert_eq!(paths(&mut fs, "REPORT-2"), vec!["/Docs/Report-2026.TXT"]);
    assert_eq!(paths(&mut fs, "2026"), vec!["/Docs/Report-2026.TXT"]); // word prefix
    assert_eq!(paths(&mut fs, "docs"), vec!["/Docs"]);
    // Rename: the old name answers nothing, the new one answers.
    fs.rename(docs, "Report-2026.TXT", docs, "summary.txt").unwrap();
    assert!(paths(&mut fs, "rep").is_empty());
    assert_eq!(paths(&mut fs, "sum"), vec!["/Docs/summary.txt"]);
    // A cross-directory move keeps the name and follows the path.
    let arch = fs.mkdir(root, "archive".to_string()).unwrap();
    fs.rename(docs, "summary.txt", arch, "summary.txt").unwrap();
    assert_eq!(paths(&mut fs, "summary"), vec!["/archive/summary.txt"]);
    // Unlink and rmdir scrub.
    fs.unlink(arch, "summary.txt").unwrap();
    assert!(paths(&mut fs, "sum").is_empty());
    fs.rmdir(root, "Docs").unwrap();
    assert!(paths(&mut fs, "doc").is_empty());
    let chk = fs.name_index_check().unwrap();
    assert!(chk.ready && chk.missing.is_empty() && chk.stale.is_empty(), "{chk:?}");
    assert!(fs.fsck(false).unwrap().is_clean());
}

#[test]
fn kat_batch_create_is_indexed() {
    let mut fs = fresh();
    let root = fs.superblock.root_inode;
    let files = ["alpha.wav", "Beta.flac", "gamma_alpha.ogg"]
        .iter()
        .map(|n| BatchFile { name: n.to_string(), data: vec![1, 2, 3], attributes: Default::default() })
        .collect();
    fs.create_files_batch(root, files).unwrap();
    assert_eq!(paths(&mut fs, "alpha"), vec!["/alpha.wav", "/gamma_alpha.ogg"]);
    assert_eq!(paths(&mut fs, "BETA"), vec!["/Beta.flac"]);
    // alpha.wav → 2 keys, beta.flac → 2, gamma_alpha.ogg → 3 (leaf, alpha.ogg, ogg).
    let chk = fs.name_index_check().unwrap();
    assert_eq!((chk.expected, chk.indexed), (7, 7));
}

#[test]
fn kat_unmarked_volume_builds_in_chunks_then_answers() {
    let mut fs = fresh();
    let root = fs.superblock.root_inode;
    let d = fs.mkdir(root, "music".to_string()).unwrap();
    for i in 0..40 {
        fs.create_file(d, format!("track_{i:02}.mp3")).unwrap();
    }
    // Simulate a pre-NAMEINDEX volume: no name facts, no marker.
    fs.name_index_drop().unwrap();
    assert!(!fs.name_index_ready().unwrap());
    let f = fs.find_names("track", 64).unwrap();
    assert!(!f.indexed && f.hits.is_empty());
    // The login task's shape: chunks of 8 inodes, each committed.
    let (mut next, mut names) = (1u64, 0usize);
    loop {
        let (n, k, done) = fs.name_index_build_step(next, 8).unwrap();
        names += k;
        next = n;
        if done {
            break;
        }
    }
    fs.name_index_mark().unwrap();
    assert_eq!(names, 41);
    assert!(fs.name_index_ready().unwrap());
    assert_eq!(fs.find_names("track", 64).unwrap().hits.len(), 40);
    assert_eq!(fs.find_names("mp3", 64).unwrap().hits.len(), 40); // word prefix
    assert_eq!(fs.find_names("track_1", 64).unwrap().hits.len(), 10);
    assert!(fs.fsck(false).unwrap().is_clean());
}

#[test]
fn kat_fsck_sees_a_broken_index_and_reindex_heals() {
    let mut fs = fresh();
    let root = fs.superblock.root_inode;
    fs.create_file(root, "one.txt".to_string()).unwrap();
    fs.create_file(root, "two.txt".to_string()).unwrap();
    fs.name_index_drop().unwrap();
    fs.name_index_mark().unwrap(); // marked but empty: every name is missing
    let r = fs.fsck(false).unwrap();
    assert_eq!(r.name_index_missing.len(), 2);
    assert!(!r.is_clean());
    let n = fs.reindex_names().unwrap();
    assert_eq!(n, 2);
    let r = fs.fsck(false).unwrap();
    assert!(r.is_clean(), "{r:?}");
    assert_eq!(paths(&mut fs, "two"), vec!["/two.txt"]);
    // fsck --repair heals the same break.
    fs.name_index_drop().unwrap();
    fs.name_index_mark().unwrap();
    let r = fs.fsck(true).unwrap();
    assert_eq!(r.name_index_missing.len(), 2);
    assert!(fs.fsck(false).unwrap().is_clean());
}
