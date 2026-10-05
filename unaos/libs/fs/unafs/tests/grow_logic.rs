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

//! UNAFSGROW (rmbp-ledger B347) KATs: `UnaFS::grow` on IMAGE FILES.
//!
//! Format at N, write files, grow to 4N, remount, fsck clean, files intact,
//! the free count exact (old free + added blocks − the refcount map's own
//! growth); the one-level → two-level map crossing; the refusals (shrink, past
//! the map, pre-v5 past one level, a short device); the interrupted grow (root
//! flipped, superblock not rewritten) mounts at the OLD size and finishes on a
//! re-run; the in-RAM image shape the kernel's `tests unafsgrow` uses.

use std::path::PathBuf;
use unafs::refmap::REFS_PER_LEAF;
use unafs::superblock::{MAX_BLOCK_COUNT, MAX_BLOCK_COUNT_ONE_LEVEL};
use unafs::{AttributeValue, BLOCK_SIZE, FileDevice, MemDevice, UnaFS};

struct Img(PathBuf);

impl Img {
    fn new(tag: &str, blocks: u64) -> Img {
        let p = std::env::temp_dir().join(format!(
            "unafsgrow-{}-{}-{}.img",
            tag,
            std::process::id(),
            blocks
        ));
        let f = std::fs::File::create(&p).unwrap();
        f.set_len(blocks * BLOCK_SIZE).unwrap();
        Img(p)
    }
    fn set_blocks(&self, blocks: u64) {
        let f = std::fs::OpenOptions::new().write(true).open(&self.0).unwrap();
        f.set_len(blocks * BLOCK_SIZE).unwrap();
    }
    fn dev(&self) -> FileDevice {
        FileDevice::open(self.0.to_str().unwrap()).unwrap()
    }
}

impl Drop for Img {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn pattern(n: usize, seed: u8) -> Vec<u8> {
    (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed)).collect()
}

/// Write the fixture tree: a small file, a multi-block file in a directory,
/// and a typed attribute.
fn populate(fs: &mut UnaFS<FileDevice>) {
    let root = fs.superblock.root_inode;
    let a = fs.create_file(root, "a.txt".into()).unwrap();
    fs.write_data(a, 0, b"grown, not lost").unwrap();
    let d = fs.mkdir(root, "dir".into()).unwrap();
    let b = fs.create_file(d, "b.bin".into()).unwrap();
    fs.write_data(b, 0, &pattern(300_000, 7)).unwrap();
    fs.set_attribute(b, "kind".into(), AttributeValue::String("blob".into()))
        .unwrap();
}

fn check(fs: &mut UnaFS<FileDevice>) {
    let a = fs.resolve_path("/a.txt").unwrap();
    let n = fs.read_inode(a).unwrap().size;
    assert_eq!(fs.read_data(a, 0, n).unwrap(), b"grown, not lost");
    let b = fs.resolve_path("/dir/b.bin").unwrap();
    let n = fs.read_inode(b).unwrap().size;
    assert_eq!(fs.read_data(b, 0, n).unwrap(), pattern(300_000, 7));
    assert_eq!(
        fs.get_attribute(b, "kind").unwrap(),
        Some(AttributeValue::String("blob".into()))
    );
}

fn leaves(blocks: u64) -> u64 {
    blocks.div_ceil(REFS_PER_LEAF)
}

#[test]
fn grow_to_four_n_on_an_image_file() {
    const N: u64 = 2048; // 8 MiB
    let img = Img::new("4n", N);
    let mut fs = UnaFS::format(img.dev(), 0).unwrap();
    assert_eq!(fs.superblock.block_count, N);
    populate(&mut fs);
    let free_before = fs.free_blocks();
    drop(fs);

    img.set_blocks(4 * N);
    let (fs, r) = unafs::grow(img.dev(), 4 * N).unwrap();
    assert_eq!((r.from, r.to), (N, 4 * N));
    assert_eq!(r.free_before, free_before);
    // Single-level map either side: a legacy map grows by its extra leaves;
    // a paged (v7, UNAFSMAP) map's new leaves are holes — it owns no new block.
    let map_growth = if fs.map_shape() == unafs::maptree::Shape::Legacy {
        leaves(4 * N) - leaves(N)
    } else {
        0
    };
    assert_eq!(r.free_after, free_before + 3 * N - map_growth);
    drop(fs);

    // A fresh mount reads the new size off block 0, fscks clean, files intact.
    let mut fs = UnaFS::mount(img.dev()).unwrap();
    assert_eq!(fs.superblock.block_count, 4 * N);
    assert_eq!(fs.free_blocks(), r.free_after);
    let rep = fs.fsck(false).unwrap();
    assert!(rep.is_clean(), "{rep:?}");
    check(&mut fs);

    // The added blocks are allocatable: a file bigger than the OLD volume.
    let root = fs.superblock.root_inode;
    let big = fs.create_file(root, "big.bin".into()).unwrap();
    let data = pattern((N as usize + 512) * BLOCK_SIZE as usize, 3);
    fs.write_data(big, 0, &data).unwrap();
    drop(fs);
    let mut fs = UnaFS::mount(img.dev()).unwrap();
    let big = fs.resolve_path("/big.bin").unwrap();
    assert_eq!(fs.read_data(big, 0, data.len() as u64).unwrap(), data);
    assert!(fs.fsck(false).unwrap().is_clean());
    check(&mut fs);
}

#[test]
fn grow_crosses_into_the_two_level_map() {
    const N: u64 = 4096;
    let to = MAX_BLOCK_COUNT_ONE_LEVEL + 3 * REFS_PER_LEAF + 17; // sparse file, > 2 GiB
    let img = Img::new("2lvl", N);
    let mut fs = UnaFS::format(img.dev(), 0).unwrap();
    populate(&mut fs);
    assert!(!fs.superblock.refmap_two_level());
    drop(fs);
    img.set_blocks(to);
    let (fs, r) = unafs::grow(img.dev(), to).unwrap();
    assert_eq!(r.to, to);
    assert!(fs.superblock.refmap_two_level());
    drop(fs);
    let mut fs = UnaFS::mount(img.dev()).unwrap();
    assert_eq!(fs.superblock.block_count, to);
    assert!(fs.fsck(false).unwrap().is_clean());
    check(&mut fs);
}

#[test]
fn grow_refusals() {
    const N: u64 = 1024;
    let img = Img::new("refuse", N);
    let mut fs = UnaFS::format(img.dev(), 0).unwrap();
    populate(&mut fs);
    let generation = fs.root_generation();
    // Shrink.
    assert!(fs.grow(N - 1).is_err());
    // Past what the map can address (two levels), whatever the device.
    assert!(fs.grow(MAX_BLOCK_COUNT + 1).is_err());
    // A device that does not reach the new last block (the file is still N).
    assert!(fs.grow(2 * N).is_err());
    // Same size: a no-op.
    let r = fs.grow(N).unwrap();
    assert_eq!((r.from, r.to), (N, N));
    // Nothing was written by any of them.
    assert_eq!(fs.root_generation(), generation);
    drop(fs);
    let mut fs = UnaFS::mount(img.dev()).unwrap();
    assert_eq!(fs.superblock.block_count, N);
    assert!(fs.fsck(false).unwrap().is_clean());
    check(&mut fs);
}

#[test]
fn grow_refuses_a_pre_v5_volume_past_one_map_level() {
    const N: u64 = 1024;
    let img = Img::new("v4", N);
    let fs = UnaFS::format_with_version(img.dev(), 0, 4).unwrap();
    drop(fs);
    let to = MAX_BLOCK_COUNT_ONE_LEVEL + 1;
    img.set_blocks(to);
    let mut fs = UnaFS::mount(img.dev()).unwrap();
    assert!(fs.grow(to).is_err());
    // Within one level it grows (v4 stays v4).
    let r = fs.grow(MAX_BLOCK_COUNT_ONE_LEVEL).unwrap();
    assert_eq!(r.to, MAX_BLOCK_COUNT_ONE_LEVEL);
    assert_eq!(fs.superblock.version, 4);
    assert!(fs.fsck(false).unwrap().is_clean());
}

#[test]
fn interrupted_grow_mounts_at_the_old_size_and_finishes_on_rerun() {
    const N: u64 = 2048;
    let img = Img::new("cut", N);
    let mut fs = UnaFS::format(img.dev(), 0).unwrap();
    populate(&mut fs);
    drop(fs);
    img.set_blocks(4 * N);
    let mut fs = UnaFS::mount(img.dev()).unwrap();
    // The GROW root flips; block 0 is never rewritten (the power cut).
    fs.grow_interrupted_before_superblock(4 * N).unwrap();
    drop(fs);

    let mut fs = UnaFS::mount(img.dev()).unwrap();
    assert_eq!(fs.superblock.block_count, N, "block 0 still names the old size");
    assert!(fs.fsck(false).unwrap().is_clean());
    check(&mut fs);
    // A normal commit at the old size is consistent too.
    let root = fs.superblock.root_inode;
    let c = fs.create_file(root, "after-cut.txt".into()).unwrap();
    fs.write_data(c, 0, b"still the old volume").unwrap();
    drop(fs);
    let mut fs = UnaFS::mount(img.dev()).unwrap();
    assert_eq!(fs.superblock.block_count, N);
    assert!(fs.fsck(false).unwrap().is_clean());

    // The re-run finishes it.
    let r = fs.grow(4 * N).unwrap();
    assert_eq!((r.from, r.to), (N, 4 * N));
    drop(fs);
    let mut fs = UnaFS::mount(img.dev()).unwrap();
    assert_eq!(fs.superblock.block_count, 4 * N);
    assert!(fs.fsck(false).unwrap().is_clean());
    check(&mut fs);
}

/// The kernel `tests unafsgrow` shape: an in-RAM image grown by 1 MiB, then
/// carried as bytes (what the kernel persists to /var/tmp and reads back).
#[test]
fn ram_image_grows_by_one_mib_and_survives_as_bytes() {
    const N: u64 = 512;
    let mut fs = UnaFS::format(MemDevice::with_blocks(N), 0).unwrap();
    let root = fs.superblock.root_inode;
    let w = fs.create_file(root, "w.txt".into()).unwrap();
    fs.write_data(w, 0, b"unafsgrow").unwrap();
    fs.device.resize_blocks(N + 256);
    let r = fs.grow(N + 256).unwrap();
    assert_eq!((r.from, r.to), (N, N + 256));
    assert!(fs.fsck(false).unwrap().is_clean());
    let bytes = core::mem::take(&mut fs.device).into_bytes();
    drop(fs);
    assert_eq!(bytes.len() as u64, (N + 256) * BLOCK_SIZE);
    let mut fs = UnaFS::mount(MemDevice::from_bytes(bytes)).unwrap();
    assert_eq!(fs.superblock.block_count, N + 256);
    assert!(fs.fsck(false).unwrap().is_clean());
    let w = fs.resolve_path("/w.txt").unwrap();
    assert_eq!(fs.read_data(w, 0, 9).unwrap(), b"unafsgrow");
}
