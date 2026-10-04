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

//! B302 M4 (audit B292): v6 inodes carry ctime/mtime/atime (unix seconds) in
//! the meta trailer. The crate stamps from `clock::now()` — an embedder hook
//! (the kernel's clock), else `SystemTime` under `std`, else 0. One test fn:
//! the hook is process-global, so the cases run in sequence.

use std::sync::atomic::{AtomicU64, Ordering};
use unafs::{AttributeValue, BLOCK_SIZE, BlockDevice, MemDevice, UnaFS, clock};

static NOW: AtomicU64 = AtomicU64::new(0);
fn fake_clock() -> u64 {
    NOW.load(Ordering::SeqCst)
}

fn dev() -> MemDevice {
    let mut d = MemDevice::new();
    d.write_block(4095, &vec![0u8; BLOCK_SIZE as usize]).unwrap();
    d
}

#[test]
fn timestamps_are_stamped_persisted_and_v6_only() {
    // No hook, std build: wall-clock seconds.
    clock::clear_clock_hook();
    let mut fs = UnaFS::format(dev(), 0).unwrap();
    let root = fs.superblock.root_inode;
    let w = fs.create_file(root, "wall".into()).unwrap();
    assert!(fs.stat(w).unwrap().mtime > 1_600_000_000, "SystemTime under std");

    clock::set_clock_hook(fake_clock);
    NOW.store(1000, Ordering::SeqCst);
    let d = fs.mkdir(root, "d".into()).unwrap();
    let f = fs.create_file(d, "f".into()).unwrap();
    let s = fs.stat(f).unwrap();
    assert_eq!((s.ctime, s.mtime, s.atime), (1000, 1000, 1000), "create stamps all three");
    assert_eq!(fs.stat(d).unwrap().mtime, 1000, "adding an entry is a directory data change");

    NOW.store(2000, Ordering::SeqCst);
    fs.write_data(f, 0, b"bytes").unwrap();
    let s = fs.stat(f).unwrap();
    assert_eq!((s.ctime, s.mtime, s.atime), (2000, 2000, 2000), "write: data change");

    NOW.store(3000, Ordering::SeqCst);
    fs.set_attribute(f, "k".into(), AttributeValue::Int(1)).unwrap();
    let s = fs.stat(f).unwrap();
    assert_eq!((s.ctime, s.mtime, s.atime), (3000, 2000, 2000), "attribute: metadata only");

    NOW.store(4000, Ordering::SeqCst);
    let _ = fs.read_data(f, 0, 5).unwrap();
    assert_eq!(fs.stat(f).unwrap().atime, 2000, "noatime: a read never writes");
    fs.rename(d, "f", d, "g").unwrap();
    let s = fs.stat(f).unwrap();
    assert_eq!((s.ctime, s.mtime), (4000, 2000), "rename: metadata only");
    assert_eq!(fs.stat(d).unwrap().mtime, 4000);

    NOW.store(5000, Ordering::SeqCst);
    fs.remove_attribute(f, "k").unwrap();
    assert_eq!(fs.stat(f).unwrap().ctime, 5000);

    // Durable across a remount; a query can range over them via stat.
    let image = fs.device.clone();
    drop(fs);
    let mut fs = UnaFS::mount(image).unwrap();
    let s = fs.stat(f).unwrap();
    assert_eq!((s.ctime, s.mtime, s.atime), (5000, 2000, 2000));
    assert!(fs.fsck(false).unwrap().is_clean());

    // A v5 volume has nowhere to keep them: stamped in RAM, never written,
    // read back as 0 — and its inode blocks stay byte-identical to v5.
    let mut old = UnaFS::format_with_version(dev(), 0, 5).unwrap();
    let r = old.superblock.root_inode;
    let o = old.create_file(r, "o".into()).unwrap();
    let image = old.device.clone();
    drop(old);
    let mut old = UnaFS::mount(image).unwrap();
    let s = old.stat(o).unwrap();
    assert_eq!((s.ctime, s.mtime, s.atime, s.parent), (0, 0, 0, 0));

    clock::clear_clock_hook();
}
