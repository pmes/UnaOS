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

//! K8c: the snapshot READ path (crate side).
//!
//! K8b proved the retained bytes SURVIVE (raw-block reads off the device). K8c
//! exposes them through a first-class read-only handle — `open_snapshot(snap_gen)
//! -> SnapshotView` — and these KATs pin its contract: read old bytes after a
//! live overwrite AND after a live delete; attributes AS OF the snapshot; the
//! view never observes a post-snapshot change; a read perturbs neither the
//! free-block ledger nor the reclaim queue nor the live root; fsck stays clean;
//! and a view of a dropped snapshot fails closed. Authority (the K8c
//! current-ACL rule) is enforced at the kernel verb layer, not in the crate —
//! the crate view is pure bytes, so these tests are about faithful bytes.

use unafs::fs::FileSystemError;
use unafs::{AttributeValue, BLOCK_SIZE, BlockDevice, MemDevice, UnaFS};

/// Format a fresh in-memory volume of `block_count` blocks.
fn fresh_fs(block_count: u64) -> UnaFS<MemDevice> {
    let mut device = MemDevice::new();
    let empty_block = vec![0u8; BLOCK_SIZE as usize];
    device
        .write_block(block_count - 1, &empty_block)
        .expect("Failed to set disk size");
    UnaFS::format(device, 20).expect("Format failed")
}

/// Two multi-block payloads (real extents to share and diverge).
const OLD: &[u8] = &[0xA1u8; 3 * 4096];
const NEW: &[u8] = &[0xB2u8; 3 * 4096];

/// The spine leg: snapshot a file, overwrite the LIVE file, and read the
/// SNAPSHOT back through the view — OLD bytes, while the live mount reads NEW.
#[test]
fn view_reads_old_bytes_after_live_overwrite() {
    let mut fs = fresh_fs(4096);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "f.bin".into()).unwrap();
    fs.write_data(id, 0, OLD).unwrap();

    let snap_gen = fs
        .snapshot_create("before".into(), "alice".into(), 1)
        .unwrap();

    // Diverge the live tree.
    fs.write_data(id, 0, NEW).unwrap();
    assert_eq!(fs.read_data(id, 0, NEW.len() as u64).unwrap(), NEW);

    // The snapshot still reads the OLD bytes, path-resolved through the view.
    let mut snap = fs.open_snapshot(snap_gen).unwrap();
    assert_eq!(snap.generation(), snap_gen);
    let sid = snap.resolve_path("/f.bin").unwrap();
    assert_eq!(sid, id, "logical id is stable across the overwrite");
    assert_eq!(snap.read_data(sid, 0, OLD.len() as u64).unwrap(), OLD);
}

/// After the live file is DELETED, the snapshot still serves its OLD bytes —
/// the retention-aware allocator kept the blocks alive, and the view resolves
/// through its own frozen directory (the live directory no longer names it).
#[test]
fn view_reads_old_bytes_after_live_delete() {
    let mut fs = fresh_fs(4096);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "gone.bin".into()).unwrap();
    fs.write_data(id, 0, OLD).unwrap();

    let snap_gen = fs.snapshot_create("before".into(), "alice".into(), 1).unwrap();

    // Delete from the LIVE tree; the live id is now unallocated.
    fs.unlink(root, "gone.bin").unwrap();
    assert!(matches!(fs.resolve_path("/gone.bin"), Err(FileSystemError::RootMissing)));
    assert!(matches!(fs.read_inode(id), Err(FileSystemError::NotFound)));

    // The snapshot still resolves it and reads the OLD bytes.
    let mut snap = fs.open_snapshot(snap_gen).unwrap();
    let sid = snap.resolve_path("/gone.bin").unwrap();
    assert_eq!(sid, id);
    assert_eq!(snap.read_data(sid, 0, OLD.len() as u64).unwrap(), OLD);
}

/// Attributes are served AS OF the snapshot, not the live inode: change an
/// attribute after the snapshot and the view still returns the old value.
#[test]
fn view_reads_attrs_as_of_snapshot() {
    let mut fs = fresh_fs(4096);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "a.txt".into()).unwrap();
    fs.set_attribute(id, "owner".into(), AttributeValue::String("alice".into()))
        .unwrap();
    // A large attribute (spilled to extents) proves the extent path too.
    let big = AttributeValue::Blob(vec![7u8; 2000]);
    fs.set_attribute(id, "blob".into(), big.clone()).unwrap();

    let snap_gen = fs.snapshot_create("before".into(), "alice".into(), 1).unwrap();

    // Mutate the live attributes.
    fs.set_attribute(id, "owner".into(), AttributeValue::String("bob".into()))
        .unwrap();
    fs.set_attribute(id, "blob".into(), AttributeValue::Blob(vec![9u8; 2000]))
        .unwrap();

    let mut snap = fs.open_snapshot(snap_gen).unwrap();
    assert_eq!(
        snap.get_attribute(id, "owner").unwrap(),
        Some(AttributeValue::String("alice".into()))
    );
    assert_eq!(snap.get_attribute(id, "blob").unwrap(), Some(big));
    // Live sees the new values.
    assert_eq!(
        fs.get_attribute(id, "owner").unwrap(),
        Some(AttributeValue::String("bob".into()))
    );
}

/// The view is FROZEN: a file/dir created after the snapshot is invisible to it.
#[test]
fn view_cannot_observe_post_snapshot_changes() {
    let mut fs = fresh_fs(4096);
    let root = fs.superblock.root_inode;
    fs.create_file(root, "old.txt".into()).unwrap();

    let snap_gen = fs.snapshot_create("before".into(), "alice".into(), 1).unwrap();

    fs.create_file(root, "new.txt".into()).unwrap();

    let mut snap = fs.open_snapshot(snap_gen).unwrap();
    let names: Vec<String> = snap.ls(root).unwrap().into_iter().map(|e| e.name).collect();
    assert!(names.iter().any(|n| n == "old.txt"));
    assert!(!names.iter().any(|n| n == "new.txt"), "post-snapshot create leaks into the view");
    assert!(matches!(snap.resolve_path("/new.txt"), Err(FileSystemError::RootMissing)));
}

/// A read through the view perturbs NOTHING: the free-block ledger, the reclaim
/// queue, and the committed root generation are byte-for-byte unchanged across
/// open_snapshot + a full read. (Reads issue only `read_block`s.)
#[test]
fn reads_do_not_perturb_ledger_or_root() {
    let mut fs = fresh_fs(4096);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "f.bin".into()).unwrap();
    fs.write_data(id, 0, OLD).unwrap();
    let snap_gen = fs.snapshot_create("before".into(), "alice".into(), 1).unwrap();
    fs.write_data(id, 0, NEW).unwrap();

    let free_before = fs.free_blocks();
    let root_before = fs.root_generation();
    let queue_before = fs.reclaim_queue().unwrap();

    {
        let mut snap = fs.open_snapshot(snap_gen).unwrap();
        let sid = snap.resolve_path("/f.bin").unwrap();
        let _ = snap.read_data(sid, 0, OLD.len() as u64).unwrap();
        let _ = snap.ls(root).unwrap();
        let _ = snap.get_attribute(sid, "nope").unwrap();
    }

    assert_eq!(fs.free_blocks(), free_before, "read changed the free ledger");
    assert_eq!(fs.root_generation(), root_before, "read flipped the root");
    assert_eq!(fs.reclaim_queue().unwrap(), queue_before, "read touched the reclaim queue");
    assert!(fs.fsck(false).unwrap().is_clean(), "volume unclean after snapshot reads");
}

/// A view of a DROPPED snapshot fails closed: `open_snapshot` refuses the
/// missing generation (a dangling read handle is unrepresentable).
#[test]
fn view_of_dropped_snapshot_fails_closed() {
    let mut fs = fresh_fs(4096);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "f.bin".into()).unwrap();
    fs.write_data(id, 0, OLD).unwrap();
    let snap_gen = fs.snapshot_create("before".into(), "alice".into(), 1).unwrap();

    // Open once (works), then drop, then reopen (fails closed).
    assert!(fs.open_snapshot(snap_gen).is_ok());
    fs.snapshot_drop(snap_gen).unwrap();
    assert!(matches!(
        fs.open_snapshot(snap_gen),
        Err(FileSystemError::SnapshotNotFound(g)) if g == snap_gen
    ));
    // An unknown generation likewise refuses.
    assert!(matches!(
        fs.open_snapshot(snap_gen + 999),
        Err(FileSystemError::SnapshotNotFound(_))
    ));
}
