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

//! XTS (rmbp-ledger B689, VOLKEY M2): a CIPHERED volume (unafs-records.md §R2b) end to end on the host.
//!
//! Format a plaintext v7 volume with a witness file; `encipher` it under a `seal_ciphered` slot; the raw
//! medium holds no witness byte-run and its superblock reads v8 (an older reader refuses it); no key and a
//! wrong key are refused; the recovery words mount it, a write through the cipher lands, a re-mount with the
//! password reads both files and fsck is clean; the same volume through `BlockAdapter` (whose root flip is
//! ONE enciphered sector) re-mounts through `MemDevice` (whole-block cipher) — the two paths agree; a device
//! that cannot carry the cipher refuses; editing the slot's cipher field is refused.

use unafs::fs::{FileSystemError, encipher};
use unafs::keyslot::{CIPHER_AES256_XTS, VERSION_CIPHERED};
use unafs::storage::{Error as StorageError, VolumeCipher};
use unafs::{
    BLOCK_SIZE, BlockAdapter, BlockDevice, KdfParams, KeySlot, KeySlotError, MemDevice, MemSectorDevice,
    Superblock, UnaFS, Unlock,
};

const PW: &[u8] = b"xts-fixture-pw";
const WRONG: &[u8] = b"xts-fixture-pX";
const BLOCKS: u64 = 1024;

fn rng() -> impl FnMut(&mut [u8]) {
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    move |b: &mut [u8]| {
        for v in b.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *v = x as u8;
        }
    }
}

/// 48 KiB of text, every line naming `tag`: a plaintext a raw scan finds if a single block leaks.
fn witness(tag: &str) -> Vec<u8> {
    let mut v = Vec::new();
    for i in 0..1024 {
        v.extend_from_slice(format!("XTS witness {tag} line {i:06} -- plaintext -\n").as_bytes());
    }
    v
}

fn needle(tag: &str) -> Vec<u8> {
    format!("XTS witness {tag} line").into_bytes()
}

fn count(hay: &[u8], n: &[u8]) -> usize {
    hay.windows(n.len()).filter(|w| *w == n).count()
}

fn put<D: BlockDevice>(fs: &mut UnaFS<D>, name: &str, data: &[u8]) {
    let root = fs.superblock.root_inode;
    let id = fs.create_file(root, name.into()).unwrap();
    fs.write_data(id, 0, data).unwrap();
}

fn get<D: BlockDevice>(fs: &mut UnaFS<D>, path: &str, len: usize) -> Vec<u8> {
    let id = fs.resolve_path(path).unwrap();
    fs.read_data(id, 0, len as u64).unwrap()
}

/// The bytes of a mounted MemDevice volume, as the medium holds them.
fn medium(mut fs: UnaFS<MemDevice>) -> Vec<u8> {
    let bytes = fs.device.as_bytes().to_vec();
    fs.device = MemDevice::new();
    drop(fs);
    bytes
}

/// A device that forwards blocks but cannot carry a cipher (the trait default).
struct Plain(MemDevice);
impl BlockDevice for Plain {
    fn read_block(&mut self, id: u64, buf: &mut [u8]) -> Result<(), StorageError> {
        self.0.read_block(id, buf)
    }
    fn write_block(&mut self, id: u64, buf: &[u8]) -> Result<(), StorageError> {
        self.0.write_block(id, buf)
    }
    fn block_count(&self) -> u64 {
        self.0.block_count()
    }
    fn flush(&mut self) -> Result<(), StorageError> {
        self.0.flush()
    }
}

#[test]
fn ciphered_volume_round_trip() {
    let w1 = witness("one");
    let w2 = witness("two");
    let w3 = witness("three");

    // ── a plaintext v7 volume with a witness ────────────────────────────────────────────────────────────────
    let mut fs = UnaFS::format(MemDevice::with_blocks(BLOCKS), 0).unwrap();
    put(&mut fs, "a.txt", &w1);
    let plain = medium(fs);
    assert!(count(&plain, &needle("one")) > 0, "the scan finds plaintext when it is there");

    // ── encipher it ─────────────────────────────────────────────────────────────────────────────────────────
    let (slot, words, key) = KeySlot::seal_ciphered(PW, KdfParams::FLOOR, &mut rng()).unwrap();
    assert_eq!(slot.cipher, CIPHER_AES256_XTS);
    let mut dst = MemDevice::new();
    assert_eq!(encipher(&mut MemDevice::from_bytes(plain.clone()), &mut dst, &slot, &key).unwrap(), BLOCKS);
    // The slot never names a key an unkeyed mount could use, and a plaintext volume cannot be keyed with it.
    let mut other = UnaFS::format(MemDevice::with_blocks(256), 0).unwrap();
    assert!(matches!(
        other.set_key_slot(slot.clone()),
        Err(FileSystemError::KeySlot(KeySlotError::Unsupported))
    ));
    drop(key);
    let raw = dst.into_bytes();
    let b0 = &raw[..BLOCK_SIZE as usize];
    assert_eq!(u32::from_le_bytes(b0[5..9].try_into().unwrap()), VERSION_CIPHERED);
    assert_eq!(&b0[..5], &plain[..5]);
    assert_eq!(&b0[9..37], &plain[9..37], "the superblock is byte-identical but for the stamp");
    assert_eq!(KeySlot::parse(b0).unwrap(), Some(slot.clone()));
    assert!(Superblock::from_bytes(b0).is_err(), "a reader older than XTS refuses v8");
    assert_eq!(count(&raw, &needle("one")), 0, "raw ciphertext carries no plaintext run");
    for id in 1..BLOCKS as usize {
        let r = id * BLOCK_SIZE as usize..(id + 1) * BLOCK_SIZE as usize;
        assert_ne!(raw[r.clone()], plain[r], "block {id} is enciphered (free blocks too)");
    }

    // ── no key, a wrong key: refused; the words open it; a write goes through the cipher ────────────────────
    assert!(matches!(UnaFS::mount(MemDevice::from_bytes(raw.clone())), Err(FileSystemError::KeyRequired)));
    assert!(matches!(
        UnaFS::mount_with(MemDevice::from_bytes(raw.clone()), &Unlock::Password(WRONG)),
        Err(FileSystemError::KeySlot(KeySlotError::KeyRefused))
    ));
    let mut fs = UnaFS::mount_with(MemDevice::from_bytes(raw.clone()), &Unlock::Recovery(words.as_str())).unwrap();
    assert_eq!(get(&mut fs, "/a.txt", w1.len()), w1);
    put(&mut fs, "b.txt", &w2);
    let raw2 = medium(fs);
    assert_eq!(count(&raw2, &needle("one")) + count(&raw2, &needle("two")), 0);
    assert_eq!(u32::from_le_bytes(raw2[5..9].try_into().unwrap()), VERSION_CIPHERED, "commits keep the stamp");

    // ── re-mount with the password: both files, fsck clean ──────────────────────────────────────────────────
    let mut fs = UnaFS::mount_with(MemDevice::from_bytes(raw2.clone()), &Unlock::Password(PW)).unwrap();
    assert_eq!(get(&mut fs, "/a.txt", w1.len()), w1);
    assert_eq!(get(&mut fs, "/b.txt", w2.len()), w2);
    assert!(fs.fsck(false).unwrap().is_clean());
    drop(fs);

    // ── the sector path: BlockAdapter flips the root with ONE enciphered sector; MemDevice reads it back ───
    let mut sd = MemSectorDevice::from_bytes(raw2.clone());
    {
        let mut fs = UnaFS::mount_with(BlockAdapter::whole(&mut sd), &Unlock::Password(PW)).unwrap();
        assert_eq!(get(&mut fs, "/b.txt", w2.len()), w2);
        put(&mut fs, "c.txt", &w3);
    }
    let raw3 = sd.as_bytes().to_vec();
    assert_eq!(count(&raw3, &needle("three")), 0);
    let mut fs = UnaFS::mount_with(MemDevice::from_bytes(raw3.clone()), &Unlock::Recovery(words.as_str())).unwrap();
    assert_eq!(get(&mut fs, "/a.txt", w1.len()), w1);
    assert_eq!(get(&mut fs, "/c.txt", w3.len()), w3);
    assert!(fs.fsck(false).unwrap().is_clean());
    drop(fs);

    // ── refusals: a device with no cipher; the slot's cipher field edited (the AAD binds it); the stamp ─────
    assert!(matches!(
        UnaFS::mount_with(Plain(MemDevice::from_bytes(raw3.clone())), &Unlock::Password(PW)),
        Err(FileSystemError::Storage(StorageError::NoCipher))
    ));
    let mut edited = raw3.clone();
    edited[512 + 12] = 0; // cipher 1 -> 0: no longer ciphered, so the v8 stamp is refused outright
    assert!(UnaFS::mount_with(MemDevice::from_bytes(edited.clone()), &Unlock::Password(PW)).is_err());
    edited[5..9].copy_from_slice(&7u32.to_le_bytes()); // and restamped v7: the wrap's tag refuses the edit
    assert!(matches!(
        UnaFS::mount_with(MemDevice::from_bytes(edited), &Unlock::Password(PW)),
        Err(FileSystemError::KeySlot(KeySlotError::KeyRefused))
    ));
    let mut unstamped = raw3;
    unstamped[5..9].copy_from_slice(&7u32.to_le_bytes());
    assert!(matches!(
        UnaFS::mount_with(MemDevice::from_bytes(unstamped), &Unlock::Password(PW)),
        Err(FileSystemError::CorruptVolume(_))
    ));
}

#[test]
fn cipher_shape() {
    let (slot, _words, key) = KeySlot::seal_ciphered(PW, KdfParams::FLOOR, &mut rng()).unwrap();
    let mut c = VolumeCipher::new(&key).unwrap();
    let block: Vec<u8> = (0..BLOCK_SIZE as usize).map(|i| (i % 251) as u8).collect();
    // Block 0 passes through untouched; every other block is enciphered under its own number.
    assert_eq!(c.seal(0, 0, &block).unwrap(), &block[..]);
    let c5 = c.seal(5, 0, &block).unwrap().to_vec();
    let c6 = c.seal(6, 0, &block).unwrap().to_vec();
    assert_ne!(c5, block);
    assert_ne!(c5, c6);
    // One sector alone equals that sector of the whole block, both ways.
    for s in 0..8 {
        let r = s * 512..(s + 1) * 512;
        assert_eq!(c.seal(5, s * 512, &block[r.clone()]).unwrap(), &c5[r.clone()]);
        let mut sec = c5[r.clone()].to_vec();
        c.open(5, s * 512, &mut sec).unwrap();
        assert_eq!(sec, &block[r]);
    }
    let mut back = c5.clone();
    c.open(5, 0, &mut back).unwrap();
    assert_eq!(back, block);
    assert!(c.seal(5, 8, &block[..512]).is_err());
    assert!(c.seal(5, 3584, &block[..1024]).is_err());
    drop(slot);
}
