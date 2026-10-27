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
//! * UNAFSWRITE2 second half (rmbp-ledger B861): with `hold_fresh` on, the btree node store's writes (catalog, name
//!   index, attribute index) join the held set too, so an op writes NOTHING to the medium before the stage; `write` and
//!   `truncate` stage like the rest; a full versioned volume spends its oldest version inside a split write (the drop
//!   commits whole, the overwrite is staged); a cut before the flip leaves the old tree in every case.
//!
//! Every case ends with a remount and a clean fsck. The clock is pinned (inode stamps are wall seconds: two twin
//! volumes built across a second boundary would differ in bytes).

use unafs::fs::FlipOutcome;
use unafs::inode::AttributeValue;
use unafs::{BLOCK_SIZE, BlockDevice, MemDevice, UnaFS};

fn fresh_fs(block_count: u64) -> UnaFS<MemDevice> {
    unafs::clock::set_clock_hook(|| 1_790_000_000);
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
    assert_eq!(s.fresh_blocks() + s.node_blocks(), b.fresh_held()); // UNAFSWRITE2 (B861): the nodes are held too
    assert!(s.fresh_blocks() > 0 && s.node_blocks() > 0 && s.blocks() > s.fresh_blocks() + s.node_blocks());
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
    // Under the op NOTHING reaches the medium (UNAFSWRITE2's second half, B861: the node writes are held too).
    assert_eq!(changed, 0, "the op wrote to the medium under the hold: {changed}");

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

// ---- UNAFSWRITE2 second half (rmbp-ledger B861): the btree node store's writes, write and truncate, all staged ----

/// Blocks whose bytes differ between two images of the medium.
fn changed_blocks(before: &[u8], after: &[u8]) -> usize {
    let n = before.len().max(after.len()) as u64 / BLOCK_SIZE;
    (0..n)
        .filter(|blk| {
            let r = (*blk * BLOCK_SIZE) as usize..((*blk + 1) * BLOCK_SIZE) as usize;
            before.get(r.clone()) != after.get(r)
        })
        .count()
}

/// Run `op` on `b` the way the kernel's HOLD 1 does (autocommit off, `hold_fresh` on): the medium must not move under
/// the op (no write_fresh block, no btree node, no spill). Returns the stage.
fn held_op(b: &mut UnaFS<MemDevice>, op: impl FnOnce(&mut UnaFS<MemDevice>)) -> unafs::fs::StagedCommit {
    b.hold_fresh(true);
    let before = b.device.as_bytes().to_vec();
    let d0 = b.commit_stats().held_direct;
    op(b);
    b.hold_fresh(false);
    assert_eq!(changed_blocks(&before, b.device.as_bytes()), 0, "the op wrote to the medium under the hold");
    assert_eq!(b.commit_stats().held_direct, d0, "nothing spilled past FRESH_HOLD_MAX");
    b.stage().unwrap().expect("staged")
}

/// The twin `a` committed whole and `b` staged: same generation, free count and bytes; `b` remounts clean.
fn land_twin(a: &mut UnaFS<MemDevice>, b: &mut UnaFS<MemDevice>, s: unafs::fs::StagedCommit) -> UnaFS<MemDevice> {
    a.commit().unwrap();
    s.write_staged(&mut b.device).unwrap();
    assert_eq!(b.flip(&s).unwrap(), FlipOutcome::Landed);
    assert_eq!(b.fresh_held(), 0);
    assert_eq!(a.root_generation(), b.root_generation());
    assert_eq!(a.free_blocks(), b.free_blocks());
    assert!(a.device.as_bytes() == b.device.as_bytes(), "the staged commit wrote different bytes than one commit");
    remount_clean(b.device.clone())
}

fn node_op(fs: &mut UnaFS<MemDevice>, id: u64) {
    let root = fs.superblock.root_inode;
    let n = fs.create_file(root, "node.txt".into()).unwrap();
    fs.set_attribute(n, "kind".into(), AttributeValue::String("node".into())).unwrap();
    fs.set_attribute(id, "rank".into(), AttributeValue::Int(4)).unwrap();
    fs.rename(root, "node.txt", root, "moved.txt").unwrap();
    // The index answers from the HELD nodes inside the transaction (the read overlay).
    assert_eq!(fs.query("kind == \"node\"").unwrap().len(), 1);
    assert!(fs.find_names("moved", 8).unwrap().hits.iter().any(|h| h.path == "/moved.txt"));
}

fn node_check(m: &mut UnaFS<MemDevice>, id: u64) {
    let n = m.resolve_path("/moved.txt").expect("moved");
    assert_eq!(attr(m, n, "kind"), Some(AttributeValue::String("node".into())));
    assert_eq!(attr(m, id, "rank"), Some(AttributeValue::Int(4)));
    assert_eq!(m.query("kind == \"node\"").unwrap().len(), 1);
    assert!(m.find_names("moved", 8).unwrap().hits.iter().any(|h| h.path == "/moved.txt"));
}

#[test]
fn the_node_store_writes_nothing_under_the_op() {
    let (mut a, mut b) = (fresh_fs(4000), fresh_fs(4000));
    let (ia, ib) = (baseline(&mut a), baseline(&mut b));
    node_op(&mut a, ia);
    let s = held_op(&mut b, |fs| node_op(fs, ib));
    assert!(s.node_blocks() > 0, "the index nodes travel in the stage");
    assert!(s.fresh_blocks() > 0);
    let mut m = land_twin(&mut a, &mut b, s);
    node_check(&mut m, ib);

    // A cut after the write phase, before the flip: the old tree, its index unchanged.
    let mut fs = fresh_fs(4000);
    let id = baseline(&mut fs);
    let (g0, free) = (fs.root_generation(), fs.free_blocks());
    let s = held_op(&mut fs, |fs| node_op(fs, id));
    s.write_staged(&mut fs.device).unwrap();
    let device = fs.device.clone();
    drop(fs);
    let mut m = remount_clean(device);
    assert_eq!((m.root_generation(), m.free_blocks()), (g0, free));
    assert!(m.resolve_path("/moved.txt").is_err());
    assert_eq!(m.query("kind == \"node\"").unwrap().len(), 0);
    assert_eq!(attr(&mut m, id, "rank"), None);
}

const BLK: usize = BLOCK_SIZE as usize;

fn bytes(tag: u8, len: usize) -> Vec<u8> {
    (0..len).map(|i| tag.wrapping_mul(31).wrapping_add(i as u8)).collect()
}

#[test]
fn a_write_lands_staged() {
    let (mut a, mut b) = (fresh_fs(4000), fresh_fs(4000));
    let (ia, ib) = (baseline(&mut a), baseline(&mut b));
    a.set_versioning(true);
    b.set_versioning(true);
    let op = |fs: &mut UnaFS<MemDevice>, id: u64| {
        fs.write_data(id, 0, &bytes(5, 3 * BLK + 100)).unwrap(); // an overwrite: keeps a version
        fs.write_data(id, (3 * BLK + 100) as u64, b"tail").unwrap(); // an append
        assert_eq!(fs.read_data(id, (3 * BLK + 100) as u64, 4).unwrap(), b"tail"); // read back from the held set
    };
    op(&mut a, ia);
    let s = held_op(&mut b, |fs| op(fs, ib));
    assert!(s.fresh_blocks() >= 4);
    let mut m = land_twin(&mut a, &mut b, s);
    let mut want = bytes(5, 3 * BLK + 100);
    want.extend_from_slice(b"tail");
    assert_eq!(m.read_data(ib, 0, want.len() as u64).unwrap(), want);
    assert_eq!(m.versions(ib).unwrap().len(), 1);

    // A cut before the flip: the old bytes.
    let mut fs = fresh_fs(4000);
    let id = baseline(&mut fs);
    fs.set_versioning(true);
    let free = fs.free_blocks();
    let s = held_op(&mut fs, |fs| op(fs, id));
    s.write_staged(&mut fs.device).unwrap();
    let device = fs.device.clone();
    drop(fs);
    let mut m = remount_clean(device);
    assert_eq!(m.free_blocks(), free);
    assert_eq!(m.read_data(id, 0, 9).unwrap(), b"committed");
    assert_eq!(m.read_inode(id).unwrap().size, 9);
}

#[test]
fn a_truncate_lands_staged() {
    let (mut a, mut b) = (fresh_fs(4000), fresh_fs(4000));
    let (ia, ib) = (baseline(&mut a), baseline(&mut b));
    for (fs, id) in [(&mut a, ia), (&mut b, ib)] {
        fs.set_versioning(true);
        fs.write_data(id, 0, &bytes(6, 3 * BLK)).unwrap();
        fs.commit().unwrap();
    }
    let op = |fs: &mut UnaFS<MemDevice>, id: u64| {
        fs.truncate_data(id, 5000).unwrap(); // a shrink to a partial tail block (CoW'd, zeroed past the end)
        fs.truncate_data(id, 9000).unwrap(); // a grow: zeros
    };
    op(&mut a, ia);
    let s = held_op(&mut b, |fs| op(fs, ib));
    let mut m = land_twin(&mut a, &mut b, s);
    let mut want = bytes(6, 5000);
    want.resize(9000, 0);
    assert_eq!(m.read_inode(ib).unwrap().size, 9000);
    assert_eq!(m.read_data(ib, 0, 9000).unwrap(), want);

    // A cut before the flip: the old three blocks.
    let mut fs = fresh_fs(4000);
    let id = baseline(&mut fs);
    fs.set_versioning(true);
    fs.write_data(id, 0, &bytes(6, 3 * BLK)).unwrap();
    fs.commit().unwrap();
    let s = held_op(&mut fs, |fs| op(fs, id));
    s.write_staged(&mut fs.device).unwrap();
    let device = fs.device.clone();
    drop(fs);
    let mut m = remount_clean(device);
    assert_eq!(m.read_data(id, 0, 3 * BLK as u64).unwrap(), bytes(6, 3 * BLK));
}

#[test]
fn a_full_volume_spends_a_version_inside_a_split_write() {
    // `nospace_room`'s drop runs in a split op whose transaction is its own attempt: the drop commits WHOLE (its freed
    // blocks become allocatable only at that flip — K8a), the retried overwrite is staged.
    let mut fs = UnaFS::format(MemDevice::with_blocks(512), 0).expect("format");
    fs.set_versioning(true);
    let root = fs.superblock.root_inode;
    let doc = fs.create_file(root, "doc.bin".into()).unwrap();
    fs.write_data(doc, 0, &bytes(1, 30 * BLK)).unwrap();
    fs.write_data(doc, 0, &bytes(2, 30 * BLK)).unwrap(); // one version: 30 blocks kept
    fs.set_versioning(false);
    let fill = fs.create_file(root, "fill.bin".into()).unwrap();
    let mut at = 0u64;
    while fs.free_blocks() > 12 {
        fs.write_data(fill, at, &bytes(9, BLK)).unwrap();
        at += BLOCK_SIZE;
    }
    fs.set_versioning(true);
    assert!(fs.free_blocks() < 20);
    let g0 = fs.root_generation();
    fs.set_autocommit(false);
    fs.hold_fresh(true);
    fs.write_data(doc, 0, &bytes(3, 20 * BLK)).expect("the oldest version gives its room to the split overwrite");
    fs.hold_fresh(false);
    assert_eq!(fs.root_generation(), g0 + 1, "the drop landed in its own (whole) commit");
    let s = fs.stage().unwrap().expect("the overwrite is staged");
    assert!(s.fresh_blocks() >= 20);
    s.write_staged(&mut fs.device).unwrap();
    // INVARIANT (nospace_room's doc): no block the overwrite freed was re-allocated before the flip — a cut here
    // mounts the dropped-version tree with the old contents whole.
    let mut cut = remount_clean(fs.device.clone());
    assert_eq!(cut.read_data(doc, 0, 30 * BLK as u64).unwrap(), bytes(2, 30 * BLK));
    assert_eq!(cut.versions(doc).unwrap().len(), 0, "the drop landed");
    assert_eq!(fs.flip(&s).unwrap(), FlipOutcome::Landed);
    let mut want = bytes(3, 20 * BLK);
    want.extend_from_slice(&bytes(2, 30 * BLK)[20 * BLK..]);
    let mut m = remount_clean(fs.device.clone());
    assert_eq!(m.read_data(doc, 0, 30 * BLK as u64).unwrap(), want);
    assert_eq!(m.versions(doc).unwrap().len(), 1, "the overwrite kept its own version");
}
