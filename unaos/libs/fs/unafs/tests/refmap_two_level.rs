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

//! Second refcount-map level (unafs-refmap arc, format v5).
//!
//! Before this arc the refmap index was ONE 4096 B block of leaf pointers, so
//! a volume was capped at 512 leaves × 1024 refs = 524,288 blocks = 2 GiB
//! (`Superblock::validate` refused anything bigger). These tests exercise the
//! lift: a volume past 2 GiB formats with a TWO-LEVEL refmap index (top block
//! of mid pointers → mid blocks of leaf pointers), mounts, survives fsck and
//! snapshots, and — the point of the arc — blocks past index 524,288 are
//! genuinely allocatable, writable, and readable back (the FileDevice test
//! crosses the old boundary with real data). Volumes at or under 2 GiB keep
//! the single-level layout bit-for-bit: a fresh small volume differs from a
//! v4 one ONLY in the superblock's version byte, which is what the
//! stamp-it-v4-and-mount tests prove.

use unafs::superblock::{MAX_BLOCK_COUNT_ONE_LEVEL, Superblock};
use unafs::{BLOCK_SIZE, BlockDevice, MemDevice, UnaFS};

const BS: usize = BLOCK_SIZE as usize;

/// A volume size (in MB) safely past the one-level wall: 2112 MB = 540,672
/// blocks > 524,288. MemDevice grows lazily, so a mostly-EMPTY two-level
/// volume is cheap — only the ~1,600 blocks the format commit touches exist.
const TWO_LEVEL_MB: u64 = 2112;

/// Format a fresh volume sized by `size_mb` on a lazily-grown MemDevice.
fn fresh_fs(size_mb: u64) -> UnaFS<MemDevice> {
    UnaFS::format(MemDevice::new(), size_mb).expect("format")
}

/// Clone `dev` and re-stamp its superblock version (the with_corrupt_sb
/// pattern, used here to MANUFACTURE a genuine older-version volume: a fresh
/// small volume's layout is identical across v3/v4/v5, so rewriting the one
/// version byte yields exactly the bytes the older writer produced).
fn with_sb_version(dev: &MemDevice, version: u32) -> MemDevice {
    let mut dev = dev.clone();
    let mut block = vec![0u8; BS];
    dev.read_block(0, &mut block).expect("read superblock");
    let mut sb = Superblock::from_bytes(&block).expect("valid superblock");
    sb.version = version;
    let bytes = sb.to_bytes().expect("older superblock still serializes");
    let mut out = vec![0u8; BS];
    out[..bytes.len()].copy_from_slice(&bytes);
    dev.write_block(0, &out).expect("write superblock");
    dev
}

/// Deterministic distinct content for chunk `ci` of length `len`.
fn chunk_bytes(ci: usize, len: usize) -> Vec<u8> {
    let mut b = vec![0u8; len];
    for (k, byte) in b.iter_mut().enumerate() {
        *byte = ((ci.wrapping_mul(31)).wrapping_add(k.wrapping_mul(7)) & 0xFF) as u8;
    }
    b
}

// ---------------------------------------------------------------------------
// 1. A volume past 2 GiB formats v5 two-level, works, and survives a remount.
// ---------------------------------------------------------------------------
#[test]
fn two_level_volume_formats_mounts_and_roundtrips() {
    let mut fs = fresh_fs(TWO_LEVEL_MB);
    assert_eq!(fs.superblock.version, 5);
    assert!(fs.superblock.block_count > MAX_BLOCK_COUNT_ONE_LEVEL);
    assert!(fs.superblock.refmap_two_level());

    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "song.txt".into()).unwrap();
    fs.write_data(id, 0, b"the crystal grows past two gibibytes").unwrap();

    // The committed refmap really is two-level: more leaves than one index
    // block of pointers can carry.
    let leaves = fs.superblock.block_count.div_ceil(BLOCK_SIZE / 4);
    assert!(leaves > BLOCK_SIZE / 8, "geometry demands a second level");

    // Remount from the committed bytes and read back.
    let device = fs.device.clone();
    let mut fs2 = UnaFS::mount(device).expect("two-level volume mounts");
    let got = fs2.read_data(id, 0, 36).unwrap();
    assert_eq!(&got, b"the crystal grows past two gibibytes");
    assert!(fs2.fsck(false).unwrap().is_clean());
}

// ---------------------------------------------------------------------------
// 2. fsck --repair is a no-op on a healthy two-level volume (the mid/top
//    index blocks are reachable metadata, not leaks).
// ---------------------------------------------------------------------------
#[test]
fn fsck_repair_is_a_noop_on_a_healthy_two_level_volume() {
    let mut fs = fresh_fs(TWO_LEVEL_MB);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "keep.bin".into()).unwrap();
    let data = chunk_bytes(1, 64 * BS);
    fs.write_data(id, 0, &data).unwrap();

    let before = fs.fsck(false).unwrap();
    assert!(before.is_clean(), "clean before repair: {before:?}");

    let repaired = fs.fsck(true).unwrap();
    assert!(repaired.is_clean());
    assert_eq!(repaired.reclaimed_blocks, 0, "repair frees nothing on a clean fs");

    let got = fs.read_data(id, 0, data.len() as u64).unwrap();
    assert_eq!(got, data, "data intact after a repair pass");
}

// ---------------------------------------------------------------------------
// 3. Snapshots retain across the two-level map exactly as before: the vaire
//    tamper-anchor rides this machinery, so retained bytes must survive a
//    live overwrite on a two-level volume too.
// ---------------------------------------------------------------------------
#[test]
fn snapshot_retains_old_bytes_on_a_two_level_volume() {
    let mut fs = fresh_fs(TWO_LEVEL_MB);
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "ledger.bin".into()).unwrap();
    fs.write_data(id, 0, b"generation one").unwrap();

    let snap_gen = fs
        .snapshot_create("before".into(), "vaire".into(), 1)
        .expect("snapshot on a two-level volume");
    fs.write_data(id, 0, b"generation TWO").unwrap();

    let mut view = fs.open_snapshot(snap_gen).unwrap();
    assert_eq!(view.read_data(id, 0, 14).unwrap(), b"generation one");
    drop(view);

    assert_eq!(fs.read_data(id, 0, 14).unwrap(), b"generation TWO");
    assert!(fs.fsck(false).unwrap().is_clean());

    fs.snapshot_drop(snap_gen).expect("drop retained root");
    assert!(fs.fsck(false).unwrap().is_clean(), "drop leaks nothing");
}

// ---------------------------------------------------------------------------
// 4. Backward compat: a fresh SMALL volume is single-level and byte-identical
//    to what v4 (and v3) wrote apart from the version stamp — so re-stamping
//    the version byte manufactures a genuine old volume, which must mount and
//    read bit-perfect, and stay writable without being silently upgraded.
// ---------------------------------------------------------------------------
#[test]
fn v4_and_v3_volumes_mount_and_read_bit_perfect() {
    let mut fs = fresh_fs(16);
    assert!(!fs.superblock.refmap_two_level(), "small volume is one level");
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "old.txt".into()).unwrap();
    let payload = chunk_bytes(7, 3 * BS + 123);
    fs.write_data(id, 0, &payload).unwrap();
    let committed = fs.device.clone();
    drop(fs);

    for old_version in [4u32, 3] {
        let old_dev = with_sb_version(&committed, old_version);
        let mut old = UnaFS::mount(old_dev).unwrap_or_else(|e| {
            panic!("v{old_version} volume must mount: {e:?}")
        });
        assert_eq!(old.superblock.version, old_version);
        // Bit-perfect read of the pre-existing data.
        let got = old.read_data(id, 0, payload.len() as u64).unwrap();
        assert_eq!(got, payload, "v{old_version} volume reads bit-perfect");
        assert!(old.fsck(false).unwrap().is_clean());

        // Still writable — and the STATIC superblock is never rewritten, so
        // the volume keeps its old version stamp across commits (no silent
        // upgrade; an old reader can still mount it afterwards).
        let id2 = old.create_file(root, "new.txt".into()).unwrap();
        old.write_data(id2, 0, b"written by the v5 build").unwrap();
        let dev = old.device.clone();
        drop(old);
        let mut again = UnaFS::mount(dev).expect("remounts after new commits");
        assert_eq!(again.superblock.version, old_version, "version stamp untouched");
        assert_eq!(again.read_data(id2, 0, 23).unwrap(), b"written by the v5 build");
        assert_eq!(again.read_data(id, 0, payload.len() as u64).unwrap(), payload);
    }
}

// ---------------------------------------------------------------------------
// 5. Hostile two-level index: a mid pointer past the volume is refused at
//    mount (the BEFS-HARDEN bound extended to the new level).
// ---------------------------------------------------------------------------
#[test]
fn hostile_mid_pointer_refused_at_mount() {
    let fs = fresh_fs(TWO_LEVEL_MB);
    let block_count = fs.superblock.block_count;
    let mut dev = fs.device.clone();
    drop(fs);

    let (rr, _) = unafs::root::read_active(&mut dev).unwrap().unwrap();
    // Two-level: the root's refmap block holds MID pointers. Corrupt the
    // first one to point past the volume.
    let mut top = vec![0u8; BS];
    dev.read_block(rr.refmap_block, &mut top).unwrap();
    top[0..8].copy_from_slice(&(block_count + 100).to_le_bytes());
    dev.write_block(rr.refmap_block, &top).unwrap();
    assert!(UnaFS::mount(dev.clone()).is_err(), "OOB mid pointer refused");

    // And a zero mid pointer likewise.
    top[0..8].copy_from_slice(&0u64.to_le_bytes());
    dev.write_block(rr.refmap_block, &top).unwrap();
    assert!(UnaFS::mount(dev).is_err(), "zero mid pointer refused");
}

// ---------------------------------------------------------------------------
// 6. THE BOUNDARY CROSSING (host-side, real file device): fill a volume past
//    the old 2 GiB wall with real data, prove blocks past index 524,288 are
//    allocated, and read every byte back — through a remount.
//
//    This writes ~2.1 GiB to CARGO_TARGET_TMPDIR and is the arc's load-
//    bearing proof; it takes tens of seconds, which is the price of actually
//    crossing the boundary (first-fit means every lower block must be live).
// ---------------------------------------------------------------------------
#[test]
fn data_crosses_the_old_2gib_boundary_and_survives_remount() {
    use unafs::FileDevice;

    /// Delete the (large) volume file even if an assert fires.
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("unafs-refmap-two-level-boundary.img");
    let _cleanup = Cleanup(path.clone());
    let _ = std::fs::remove_file(&path);

    let device = FileDevice::open(&path).expect("create volume file");
    let mut fs = UnaFS::format(device, TWO_LEVEL_MB).expect("format > 2 GiB");
    assert!(fs.superblock.refmap_two_level());
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, "across.bin".into()).unwrap();

    // 2 GiB + 16 MiB of file data — more than the old MAX_BLOCK_COUNT worth
    // of blocks, so the tail HAS to live past the old wall. Chunked writes,
    // one commit (a single transaction keeps the fill linear).
    const CHUNK: usize = 8 * 1024 * 1024;
    const CHUNKS: usize = 258; // 258 × 8 MiB = 2064 MiB > 2048 MiB
    fs.set_autocommit(false);
    for ci in 0..CHUNKS {
        let data = chunk_bytes(ci, CHUNK);
        fs.write_data(id, (ci * CHUNK) as u64, &data).unwrap();
    }
    fs.commit().unwrap();

    // The proof of the arc: the file's extents cover physical blocks past
    // the old one-level cap.
    let inode = fs.read_inode(id).unwrap();
    let max_physical = inode
        .chunks
        .iter()
        .map(|e| e.physical_block + e.length.div_ceil(BLOCK_SIZE) - 1)
        .max()
        .unwrap();
    assert!(
        max_physical > MAX_BLOCK_COUNT_ONE_LEVEL,
        "file must occupy blocks past the old 2 GiB wall (max physical {max_physical})"
    );
    assert_eq!(inode.size, (CHUNKS * CHUNK) as u64);
    drop(fs);

    // Remount from disk and verify EVERY byte, chunk by chunk.
    let device = FileDevice::open(&path).expect("reopen volume file");
    let mut fs2 = UnaFS::mount(device).expect("boundary-crossing volume mounts");
    for ci in 0..CHUNKS {
        let got = fs2.read_data(id, (ci * CHUNK) as u64, CHUNK as u64).unwrap();
        assert_eq!(got, chunk_bytes(ci, CHUNK), "chunk {ci} bit-perfect after remount");
    }
    assert!(fs2.fsck(false).unwrap().is_clean(), "fsck clean across the boundary");
}
