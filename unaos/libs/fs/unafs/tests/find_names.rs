// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// QUARRY3 (rmbp-ledger B413) + NAMEINDEX (B432): `UnaFS::find_names` — the name search Quarry's search field and
// the launcher ask: ONE range scan over the `una:fsname` index; leaf prefix, then word prefix; case-insensitive;
// paths absolute; the limit honoured.
use unafs::{BLOCK_SIZE, BlockDevice, MemDevice, UnaFS};

fn volume() -> UnaFS<MemDevice> {
    let block_count = 4000;
    let mut device = MemDevice::new();
    device.write_block(block_count - 1, &vec![0u8; BLOCK_SIZE as usize]).unwrap();
    let mut fs = UnaFS::format(device, 20).unwrap();
    let root = fs.superblock.root_inode;
    let sys = fs.mkdir(root, "system".to_string()).unwrap();
    let tf = fs.mkdir(sys, "test-f".to_string()).unwrap();
    for n in ["TEST.PNG", "TEST.MD", "notes.txt"] {
        fs.create_file(tf, n.to_string()).unwrap();
    }
    let home = fs.mkdir(root, "home".to_string()).unwrap();
    fs.create_file(home, "Test plan.md".to_string()).unwrap();
    fs.create_file(home, "my_test_notes".to_string()).unwrap();
    fs
}

fn paths(fs: &mut UnaFS<MemDevice>, q: &str, limit: usize) -> Vec<String> {
    fs.find_names(q, limit).unwrap().hits.into_iter().map(|h| h.path).collect()
}

#[test]
fn find_names_ranks_leaf_prefix_then_word_prefix() {
    let mut fs = volume();
    let f = fs.find_names("test", 64).unwrap();
    assert!(f.indexed);
    let p: Vec<&str> = f.hits.iter().map(|h| h.path.as_str()).collect();
    // rank 2 (leaf prefix): files before dirs, shallower first; then rank 1 (word prefix).
    assert_eq!(p, vec!["/home/Test plan.md", "/system/test-f/TEST.MD", "/system/test-f/TEST.PNG", "/system/test-f", "/home/my_test_notes"]);
    assert!(f.hits[3].dir && !f.hits[0].dir);
    assert_eq!(f.hits[4].rank, 1);
}

#[test]
fn find_names_bounds_case_and_empty() {
    let mut fs = volume();
    assert!(fs.find_names("", 64).unwrap().hits.is_empty());
    assert_eq!(fs.find_names("test", 2).unwrap().hits.len(), 2);
    assert!(paths(&mut fs, "zzz", 64).is_empty());
    assert_eq!(paths(&mut fs, "NOTES", 64), vec!["/system/test-f/notes.txt", "/home/my_test_notes"]);
    assert_eq!(paths(&mut fs, "md", 64), vec!["/home/Test plan.md", "/system/test-f/TEST.MD"]);
    // A mid-word substring is not a name prefix.
    assert!(paths(&mut fs, "est", 64).is_empty());
}
