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

//! UNAFSWRITE (rmbp-ledger B825): the commit in three phases — `stage` (steps 0-3 in RAM, blocks captured),
//! `StagedCommit::write_staged` (the blocks to the medium, then the barrier), `flip` (the one root sector).
//!
//! * the three phases land the same bytes as one `commit`;
//! * a mutation after the stage makes the flip `Restage` (nothing written) and a `commit` lands both;
//! * a full `commit` between the write and the flip carries the staged changes (`Landed`);
//! * a power cut after the write and before the flip mounts the old tree, free count unchanged;
//! * another instance cannot flip what this one staged (`Lost`).
//! * TICKETOWNER (rmbp-ledger B844): the holder dies after the write phase — its mount is gone, a flip of its stage on
//!   the next mount is `Lost`, the old tree stands and the next writer lands;
//! * UNAFSWRITE2 (B844): with `hold_fresh` on, the op's own write_fresh blocks travel in the stage (read back from RAM
//!   inside the op, a freed one never landing over a re-allocation), a failed write phase commits them whole, and a cut
//!   before the write leaves the old tree.
//!
//! Every case ends with a remount and a clean fsck.

use unafs::fs::FlipOutcome;
use unafs::inode::AttributeValue;
use unafs::{BLOCK_SIZE, BlockDevice, MemDevice, UnaFS};

fn fresh_fs(block_count: u64) -> UnaFS<MemDevice> {
    let mut device = MemDevice::new();
    let empty_block = vec![0u8; BLOCK_SIZE as usize];
    device.write_block(block_count - 1, &empty_block).expect("size the disk");
    UnaFS::format(device, 20).expect("format")
}

/// A committed baseline file, then autocommit off (the kernel's HOLD 1 shape).
fn baseline(fs: &mut UnaFS<MemDevice>) -> u64 {
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "keep.txt".into()).unwrap();
    fs.write_data(id, 0, b"committed").unwrap();
    fs.set_autocommit(false);
    id
}

fn attr(fs: &mut UnaFS<MemDevice>, id: u64, k: &str) -> Option<AttributeValue> {
    fs.get_attribute(id, k).unwrap()
}

fn remount_clean(device: MemDevice) -> UnaFS<MemDevice> {
    let mut fs = UnaFS::mount(device).expect("remount");
    let report = fs.fsck(false).unwrap();
    assert!(report.is_clean(), "fsck: {report:?}");
    fs
}

#[test]
fn three_phases_land_the_bytes_of_one_commit() {
    let (mut a, mut b) = (fresh_fs(4000), fresh_fs(4000));
    let (ia, ib) = (baseline(&mut a), baseline(&mut b));
    let mutate = |fs: &mut UnaFS<MemDevice>, id: u64| {
        fs.set_attribute(id, "color".into(), AttributeValue::Int(7)).unwrap();
        let root = fs.superblock.root_inode;
        fs.create_file(root, "new.txt".into()).unwrap();
    };
    mutate(&mut a, ia);
    mutate(&mut b, ib);
    assert!(a.has_staged() && b.has_staged());

    a.commit().unwrap();

    let s = b.stage().unwrap().expect("staged");
    assert!(s.blocks() > 0);
    assert!(b.holds_staged(&s));
    s.write_staged(&mut b.device).unwrap();
    assert_eq!(b.flip(&s).unwrap(), FlipOutcome::Landed);
    assert!(!b.holds_staged(&s));
    assert!(!b.has_staged());

    assert_eq!(a.root_generation(), b.root_generation());
    assert_eq!(a.free_blocks(), b.free_blocks());
    assert!(a.device.as_bytes() == b.device.as_bytes(), "three phases wrote different bytes than one commit");

    let mut m = remount_clean(b.device.clone());
    assert_eq!(attr(&mut m, ib, "color"), Some(AttributeValue::Int(7)));
    assert!(m.resolve_path("/new.txt").is_ok());
}

#[test]
fn a_mutation_after_the_stage_restages_and_both_land() {
    // Between the write and the flip.
    let mut fs = fresh_fs(4000);
    let id = baseline(&mut fs);
    fs.set_attribute(id, "first".into(), AttributeValue::Int(1)).unwrap();
    let s = fs.stage().unwrap().expect("staged");
    s.write_staged(&mut fs.device).unwrap();
    fs.set_attribute(id, "second".into(), AttributeValue::Int(2)).unwrap();
    let g0 = fs.root_generation();
    assert_eq!(fs.flip(&s).unwrap(), FlipOutcome::Restage);
    assert_eq!(fs.root_generation(), g0, "a Restage writes no root");
    assert!(fs.holds_staged(&s));
    fs.commit().unwrap();
    assert_eq!(fs.root_generation(), g0 + 1);
    let mut m = remount_clean(fs.device.clone());
    assert_eq!(attr(&mut m, id, "first"), Some(AttributeValue::Int(1)));
    assert_eq!(attr(&mut m, id, "second"), Some(AttributeValue::Int(2)));

    // Between the stage and the write.
    let mut fs = fresh_fs(4000);
    let id = baseline(&mut fs);
    fs.set_attribute(id, "first".into(), AttributeValue::Int(1)).unwrap();
    let s = fs.stage().unwrap().expect("staged");
    let root = fs.superblock.root_inode;
    fs.create_file(root, "late.txt".into()).unwrap();
    s.write_staged(&mut fs.device).unwrap();
    assert_eq!(fs.flip(&s).unwrap(), FlipOutcome::Restage);
    fs.commit().unwrap();
    let mut m = remount_clean(fs.device.clone());
    assert_eq!(attr(&mut m, id, "first"), Some(AttributeValue::Int(1)));
    assert!(m.resolve_path("/late.txt").is_ok());
}

#[test]
fn a_full_commit_between_write_and_flip_carries_the_stage() {
    let mut fs = fresh_fs(4000);
    let id = baseline(&mut fs);
    fs.set_attribute(id, "color".into(), AttributeValue::Int(3)).unwrap();
    let g0 = fs.root_generation();
    let s = fs.stage().unwrap().expect("staged");
    s.write_staged(&mut fs.device).unwrap();
    fs.commit().unwrap();
    assert_eq!(fs.root_generation(), g0 + 1);
    assert_eq!(fs.flip(&s).unwrap(), FlipOutcome::Landed);
    assert_eq!(fs.root_generation(), g0 + 1, "the carried stage flips no second root");
    let mut m = remount_clean(fs.device.clone());
    assert_eq!(attr(&mut m, id, "color"), Some(AttributeValue::Int(3)));
}

#[test]
fn a_power_cut_after_the_write_mounts_the_old_tree() {
    let mut fs = fresh_fs(4000);
    let id = baseline(&mut fs);
    let g0 = fs.root_generation();
    let free = fs.free_blocks();
    fs.set_attribute(id, "doomed".into(), AttributeValue::Int(9)).unwrap();
    let root = fs.superblock.root_inode;
    fs.create_file(root, "doomed.txt".into()).unwrap();
    let s = fs.stage().unwrap().expect("staged");
    s.write_staged(&mut fs.device).unwrap();
    // Power cut: the flip never runs.
    let device = fs.device.clone();
    drop(fs);
    let mut m = remount_clean(device);
    assert_eq!(m.root_generation(), g0);
    assert_eq!(m.free_blocks(), free, "the staged blocks are free in the old tree");
    assert_eq!(attr(&mut m, id, "doomed"), None);
    assert!(m.resolve_path("/doomed.txt").is_err());
    let ino = m.read_inode(id).unwrap();
    assert_eq!(m.read_data(id, 0, ino.size).unwrap(), b"committed");
}

#[test]
fn another_instance_cannot_flip_the_stage() {
    let mut fs = fresh_fs(4000);
    let id = baseline(&mut fs);
    fs.set_attribute(id, "color".into(), AttributeValue::Int(5)).unwrap();
    let g0 = fs.root_generation();
    let s = fs.stage().unwrap().expect("staged");
    s.write_staged(&mut fs.device).unwrap();
    let mut other = UnaFS::mount(fs.device.clone()).expect("second mount");
    assert!(!other.holds_staged(&s));
    assert_eq!(other.flip(&s).unwrap(), FlipOutcome::Lost);
    assert_eq!(other.root_generation(), g0);
    drop(other);
    // The staging instance still holds it and lands it.
    assert!(fs.holds_staged(&s));
    assert_eq!(fs.flip(&s).unwrap(), FlipOutcome::Landed);
    let mut m = remount_clean(fs.device.clone());
    assert_eq!(attr(&mut m, id, "color"), Some(AttributeValue::Int(5)));
}

#[test]
fn a_death_after_the_write_loses_the_flip() {
    // TICKETOWNER (B844): the kernel releases a dead holder's ticket and leaks its bound mount; the next `with_unafs`
    // re-binds a NEW instance from the medium.
    let mut fs = fresh_fs(4000);
    let id = baseline(&mut fs);
    fs.hold_fresh(true);
    let g0 = fs.root_generation();
    let free = fs.free_blocks();
    fs.set_attribute(id, "dead".into(), AttributeValue::Int(1)).unwrap();
    fs.hold_fresh(false);
    let s = fs.stage().unwrap().expect("staged");
    assert!(s.fresh_blocks() > 0);
    s.write_staged(&mut fs.device).unwrap();
    // The holder dies here: its instance never runs again (leaked, never flipped).
    let device = fs.device.clone();
    core::mem::forget(fs);
    let mut next = remount_clean(device);
    assert_eq!(next.flip(&s).unwrap(), FlipOutcome::Lost, "a flip after the death is Lost by instance");
    assert_eq!(next.root_generation(), g0);
    assert_eq!(next.free_blocks(), free, "the dead op's written blocks are free in the old tree");
    assert_eq!(attr(&mut next, id, "dead"), None);
    // The next writer re-stages and lands.
    next.set_autocommit(false);
    next.hold_fresh(true);
    next.set_attribute(id, "alive".into(), AttributeValue::Int(2)).unwrap();
    next.hold_fresh(false);
    let s2 = next.stage().unwrap().expect("staged");
    s2.write_staged(&mut next.device).unwrap();
    assert_eq!(next.flip(&s2).unwrap(), FlipOutcome::Landed);
    assert_eq!(next.root_generation(), g0 + 1);
    let mut m = remount_clean(next.device.clone());
    assert_eq!(attr(&mut m, id, "alive"), Some(AttributeValue::Int(2)));
    assert_eq!(attr(&mut m, id, "dead"), None);
}

/// UNAFSWRITE2 (B844): the op shapes the eight split VFS ops take — two writes to one inode (`set_attrs`: the second
/// reads the inode block the first HELD), a large attribute (extents), create + unlink + create (a held block freed
/// and re-allocated inside the transaction), mkdir, rename.
fn held_ops(fs: &mut UnaFS<MemDevice>, id: u64) {
    fs.set_attribute(id, "color".into(), AttributeValue::Int(7)).unwrap();
    fs.set_attribute(id, "shape".into(), AttributeValue::String("round".into())).unwrap();
    fs.set_attribute(id, "blob".into(), AttributeValue::Blob(vec![9u8; 9000])).unwrap();
    let root = fs.superblock.root_inode;
    fs.create_file(root, "gone.txt".into()).unwrap();
    fs.unlink(root, "gone.txt").unwrap();
    let n = fs.create_file(root, "new.txt".into()).unwrap();
    fs.set_attribute(n, "tag".into(), AttributeValue::Int(3)).unwrap();
    fs.mkdir(root, "dir".into()).unwrap();
    fs.rename(root, "new.txt", root, "renamed.txt").unwrap();
}

fn held_check(m: &mut UnaFS<MemDevice>, id: u64) {
    assert_eq!(attr(m, id, "color"), Some(AttributeValue::Int(7)));
    assert_eq!(attr(m, id, "shape"), Some(AttributeValue::String("round".into())));
    assert_eq!(attr(m, id, "blob"), Some(AttributeValue::Blob(vec![9u8; 9000])));
    assert!(m.resolve_path("/gone.txt").is_err());
    let n = m.resolve_path("/renamed.txt").expect("renamed");
    assert_eq!(attr(m, n, "tag"), Some(AttributeValue::Int(3)));
    assert!(m.resolve_path("/dir").is_ok());
    let ino = m.read_inode(id).unwrap();
    assert_eq!(m.read_data(id, 0, ino.size).unwrap(), b"committed");
}

#[test]
fn held_fresh_blocks_land_the_bytes_of_one_commit() {
    let (mut a, mut b) = (fresh_fs(4000), fresh_fs(4000));
    let (ia, ib) = (baseline(&mut a), baseline(&mut b));
    held_ops(&mut a, ia);
    a.commit().unwrap();

    b.hold_fresh(true);
    let before = b.device.as_bytes().to_vec();
    held_ops(&mut b, ib);
    b.hold_fresh(false);
    assert!(b.fresh_held() > 0);
    let s = b.stage().unwrap().expect("staged");
    assert_eq!(s.fresh_blocks(), b.fresh_held());
    assert!(s.fresh_blocks() > 0 && s.blocks() > s.fresh_blocks());
    // None of the held blocks is on the medium before the write phase.
    let after_op = b.device.as_bytes();
    let changed = (0..4000u64)
        .filter(|blk| {
            let r = (*blk * BLOCK_SIZE) as usize..((*blk + 1) * BLOCK_SIZE) as usize;
            before.get(r.clone()) != after_op.get(r)
        })
        .count();
    s.write_staged(&mut b.device).unwrap();
    assert_eq!(b.flip(&s).unwrap(), FlipOutcome::Landed);
    assert_eq!(b.fresh_held(), 0, "a landed flip clears the held set");

    assert_eq!(a.root_generation(), b.root_generation());
    assert_eq!(a.free_blocks(), b.free_blocks());
    assert!(a.device.as_bytes() == b.device.as_bytes(), "held + staged wrote different bytes than one commit");
    // Under the op only the btree store's own node writes reach the medium (UNAFSWRITE2's second half).
    assert!(changed < s.blocks(), "the op's write_fresh blocks reached the medium under the op: {changed}");

    let mut m = remount_clean(b.device.clone());
    held_check(&mut m, ib);
}

#[test]
fn a_failed_write_phase_commits_the_held_blocks() {
    let mut fs = fresh_fs(4000);
    let id = baseline(&mut fs);
    fs.hold_fresh(true);
    held_ops(&mut fs, id);
    fs.hold_fresh(false);
    let s = fs.stage().unwrap().expect("staged");
    // The write phase failed: nothing of the stage reached the medium; the kernel commits whole under its lock.
    assert!(fs.holds_staged(&s));
    fs.commit().unwrap();
    assert_eq!(fs.fresh_held(), 0);
    let mut m = remount_clean(fs.device.clone());
    held_check(&mut m, id);
}

#[test]
fn a_cut_before_the_write_leaves_nothing_held_on_disk() {
    let mut fs = fresh_fs(4000);
    let id = baseline(&mut fs);
    let g0 = fs.root_generation();
    let free = fs.free_blocks();
    fs.hold_fresh(true);
    held_ops(&mut fs, id);
    let _s = fs.stage().unwrap().expect("staged");
    let device = fs.device.clone();
    drop(fs);
    let mut m = remount_clean(device);
    assert_eq!(m.root_generation(), g0);
    assert_eq!(m.free_blocks(), free);
    assert_eq!(attr(&mut m, id, "color"), None);
    assert!(m.resolve_path("/renamed.txt").is_err());
}
