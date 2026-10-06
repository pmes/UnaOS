// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// QUARRY3 (rmbp-ledger B413): `UnaFS::find_names` — the name search Quarry's search field (and later the
// launcher) asks: case-insensitive substring over every name, paths absolute, both bounds honoured.
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
    fs
}

#[test]
fn find_names_matches_case_insensitively_with_absolute_paths() {
    let mut fs = volume();
    let (hits, dirs) = fs.find_names("test", 64, 64).unwrap();
    let paths: Vec<&str> = hits.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(paths, vec!["/home/Test plan.md", "/system/test-f", "/system/test-f/TEST.MD", "/system/test-f/TEST.PNG"]);
    assert!(hits[1].1 && !hits[0].1);
    assert_eq!(dirs, 4); // root, home, system, test-f
}

#[test]
fn find_names_bounds_and_empty_needle() {
    let mut fs = volume();
    assert!(fs.find_names("", 64, 64).unwrap().0.is_empty());
    assert_eq!(fs.find_names("test", 2, 64).unwrap().0.len(), 2);
    let (_, dirs) = fs.find_names("zzz", 64, 1).unwrap();
    assert_eq!(dirs, 1);
    assert!(fs.find_names("zzz", 64, 64).unwrap().0.is_empty());
}
