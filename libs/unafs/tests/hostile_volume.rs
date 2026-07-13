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

//! BEFS-HARDEN: hostile/corrupt volume fixtures for the on-disk parser.
//!
//! Every test crafts a volume a physically swapped or corrupted card could
//! present (the parser is kernel-reachable via the BeFS-K3 RO mount) and
//! asserts the library answers with a graceful `Err` — never a panic, a
//! capacity overflow, or an OOM abort. `#[should_panic]` is deliberately
//! absent: a panic IS the defect class under test.

use unafs::bitmap::SpaceMap;
use unafs::storage::Error as StorageError;
use unafs::{BLOCK_SIZE, BlockDevice, MemDevice, Superblock, UnaFS};

/// Format a small valid volume on an in-memory device and hand back the raw
/// device (post-format, superblock synced).
fn valid_volume() -> MemDevice {
    let fs = UnaFS::format(MemDevice::new(), 16).expect("format must succeed");
    fs.device.clone()
}

/// Clone `dev` and rewrite its superblock (block 0) after applying `mutate`.
fn with_corrupt_sb(dev: &MemDevice, mutate: impl FnOnce(&mut Superblock)) -> MemDevice {
    let mut dev = dev.clone();
    let mut block = vec![0u8; BLOCK_SIZE as usize];
    dev.read_block(0, &mut block).expect("read superblock");
    let mut sb = Superblock::from_bytes(&block).expect("valid superblock");
    mutate(&mut sb);
    let bytes = sb.to_bytes().expect("hostile superblock still serializes");
    let mut out = vec![0u8; BLOCK_SIZE as usize];
    out[..bytes.len()].copy_from_slice(&bytes);
    dev.write_block(0, &out).expect("write superblock");
    dev
}

#[test]
fn pristine_volume_still_mounts_and_lists() {
    // Positive control: hardening must not reject a valid volume.
    let dev = valid_volume();
    let mut fs = UnaFS::mount(dev).expect("valid volume must mount");
    let root = fs.superblock.root_inode;
    let entries = fs.ls(root).expect("root ls must succeed");
    assert!(entries.is_empty());
}

#[test]
fn oversized_bitmap_blocks_refused_at_mount() {
    // K3-PARSE-1: bitmap_blocks ~12289 drove a ~50 MiB mount-time allocation.
    // The geometry bound now refuses it before any allocation happens.
    let dev = valid_volume();
    let hostile = with_corrupt_sb(&dev, |sb| sb.bitmap_blocks = 12289);
    assert!(UnaFS::mount(hostile).is_err());
}

#[test]
fn absurd_bitmap_blocks_refused_not_capacity_panic() {
    // K3-PARSE-1: 2^51 bitmap blocks used to be a capacity-overflow panic.
    let dev = valid_volume();
    let hostile = with_corrupt_sb(&dev, |sb| sb.bitmap_blocks = 1 << 51);
    assert!(UnaFS::mount(hostile).is_err());
}

#[test]
fn spacemap_load_with_absurd_count_is_graceful() {
    // Defense in depth below the superblock bound: even a direct load with a
    // hostile count must fail with AllocRefused, not abort/panic.
    let mut dev = valid_volume();
    match SpaceMap::load(&mut dev, 11, 1 << 51) {
        Err(StorageError::AllocRefused(_)) => {}
        Err(other) => panic!("expected AllocRefused, got {other:?}"),
        Ok(_) => panic!("hostile bitmap count must not load"),
    }
    // And the multiplication-overflow flavor (count * BLOCK_SIZE wraps).
    assert!(matches!(
        SpaceMap::load(&mut dev, 11, u64::MAX / 8),
        Err(StorageError::AllocRefused(_))
    ));
}

#[test]
fn block_count_overflowing_volume_bytes_refused() {
    let dev = valid_volume();
    let hostile = with_corrupt_sb(&dev, |sb| sb.block_count = u64::MAX);
    assert!(UnaFS::mount(hostile).is_err());
}

#[test]
fn root_inode_out_of_bounds_refused() {
    let dev = valid_volume();
    let past_end = with_corrupt_sb(&dev, |sb| sb.root_inode = sb.block_count + 5);
    assert!(UnaFS::mount(past_end).is_err());

    let zero = with_corrupt_sb(&dev, |sb| sb.root_inode = 0);
    assert!(UnaFS::mount(zero).is_err());
}

#[test]
fn catalog_inode_out_of_bounds_refused() {
    let dev = valid_volume();
    let hostile = with_corrupt_sb(&dev, |sb| sb.catalog_inode = sb.block_count);
    assert!(UnaFS::mount(hostile).is_err());
}

#[test]
fn bitmap_span_past_volume_end_refused() {
    let dev = valid_volume();
    // Keep bitmap_blocks self-consistent but push the span off the end.
    let hostile = with_corrupt_sb(&dev, |sb| sb.bitmap_start = sb.block_count);
    assert!(UnaFS::mount(hostile).is_err());

    // And the wrapping flavor.
    let wrapping = with_corrupt_sb(&dev, |sb| sb.bitmap_start = u64::MAX);
    assert!(UnaFS::mount(wrapping).is_err());
}

#[test]
fn journal_layout_mismatch_refused() {
    // The WAL addresses the journal via compile-time constants; a superblock
    // declaring any other placement describes a volume this code never wrote.
    let dev = valid_volume();
    let moved = with_corrupt_sb(&dev, |sb| sb.journal_start = 2);
    assert!(UnaFS::mount(moved).is_err());

    let resized = with_corrupt_sb(&dev, |sb| sb.journal_blocks = 1 << 40);
    assert!(UnaFS::mount(resized).is_err());
}

#[test]
fn free_blocks_exceeding_volume_refused() {
    let dev = valid_volume();
    let hostile = with_corrupt_sb(&dev, |sb| sb.free_blocks = sb.block_count + 1);
    assert!(UnaFS::mount(hostile).is_err());
}
