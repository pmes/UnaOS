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

use alloc::string::String;
use alloc::vec::Vec;
use thiserror::Error;

#[cfg(feature = "std")]
use std::fs::{File, OpenOptions};
#[cfg(feature = "std")]
use std::io::{Read, Seek, SeekFrom, Write};
#[cfg(feature = "std")]
use std::path::Path;

/// The fundamental atomic unit of the file system.
/// All reads and writes must be aligned to this size.
pub const BLOCK_SIZE: u64 = 4096;

/// Error types related to block storage operations.
#[derive(Error, Debug)]
pub enum Error {
    /// Buffer size does not match BLOCK_SIZE.
    #[error("Buffer size {0} does not match block size {1}")]
    BadBlockSize(usize, u64),
    /// Generic IO error.
    #[error("IO Error: {0}")]
    Io(String),
    /// Attempted to access a block outside the device boundaries.
    #[error("Block out of bounds: {0}")]
    OutOfBounds(u64),
    /// A disk-derived length demanded an allocation the host refused
    /// (corrupt or hostile volume geometry). Graceful `Err`, never an abort.
    #[error("Allocation of {0} bytes refused (corrupt or hostile geometry)")]
    AllocRefused(u64),
    /// XTS (B689): the volume is ciphered (§R2b) and this device cannot carry the volume cipher — refused,
    /// never read as ciphertext.
    #[error("This block device cannot carry the volume cipher (XTS)")]
    NoCipher,
}

/// XTS (rmbp-ledger B689, VOLKEY M2): the cipher of a `cipher = 1` volume (unafs-records.md §R2b), applied in
/// the [`BlockDevice`] seam — decrypt on read, encrypt on write — so everything above the device (the CoW
/// commit, the maps, the trees, the root flip) reads and writes plaintext and never knows.
///
/// * Keys: HKDF-SHA256 (RFC 5869; salt empty — the data key is already 32 uniformly random bytes; info
///   [`XTS_INFO`]) expands the 32-byte volume data key into 64 bytes = Key1 ‖ Key2 of XTS-AES-256 (IEEE 1619,
///   `crypto_core::xts`). Equal halves (probability 2^-256) are refused, as SP 800-38E's guidance asks.
/// * Data unit = one 4096 B block; tweak = the block number (`u64`, little-endian, zero-extended to 128 bits —
///   IEEE 1619's data-unit sequence number). The block number is the volume's (a partition adapter's block 0 is
///   the volume's block 0), so a volume image enciphers the same wherever it sits.
/// * Block 0 is NEVER enciphered: it carries the superblock and the key slot, which a mount must read before it
///   has a key. Every other block is, free blocks included (the copy that makes the volume writes them all).
/// * A 512 B sector inside a block (the root flip's single-sector write) is enciphered alone at its offset
///   (`Xts::encrypt_unit_at`), byte-identical to the same sector of the whole block.
///
/// CONSTANT-TIME: `crypto_core::xts` is. The expanded key schedules live as long as the mount and are wiped
/// when the device (and so this) is dropped.
#[derive(Clone)]
pub struct VolumeCipher {
    xts: crypto_core::xts::Xts,
    /// Ciphertext staging for writes (the caller's buffer is borrowed immutably). Holds no secret.
    scratch: Vec<u8>,
}

/// XTS (B689): the HKDF `info` the volume's XTS keys are expanded under.
pub const XTS_INFO: &[u8] = b"UnaFS volume XTS-AES-256 v1";

impl VolumeCipher {
    /// Expand `key` (the slot's unwrapped data key) into the volume cipher.
    pub fn new(key: &crate::keyslot::VolumeKey) -> Result<Self, Error> {
        use crypto_core::ct::Zeroize;
        let mut okm = [0u8; 64];
        // 64 bytes of HKDF-SHA256 output is inside RFC 5869's 255 × 32 bound: this cannot fail.
        let _ = crypto_core::hkdf::hkdf::<crypto_core::sha2::Sha256>(&[], key.bytes(), XTS_INFO, &mut okm);
        let distinct = !crypto_core::ct::ct_eq(&okm[..32], &okm[32..]);
        let xts = crypto_core::xts::Xts::new(&okm);
        okm.zeroize();
        match xts {
            Ok(xts) if distinct => Ok(Self { xts, scratch: alloc::vec![0u8; BLOCK_SIZE as usize] }),
            _ => Err(Error::NoCipher),
        }
    }

    /// Decrypt, in place, `buf` as read from the medium at byte `offset` of block `id` (`offset` and
    /// `buf.len()` multiples of 16 inside the block: a whole block or a sector). Block 0 is untouched.
    pub fn open(&self, id: u64, offset: usize, buf: &mut [u8]) -> Result<(), Error> {
        if id == 0 {
            return Ok(());
        }
        Self::fit(offset, buf.len())?;
        self.xts.decrypt_unit_at(id, offset, buf).map_err(|_| Error::BadBlockSize(buf.len(), BLOCK_SIZE))
    }

    /// The bytes to put on the medium for plaintext `buf` at byte `offset` of block `id`: `buf` itself for
    /// block 0, else its ciphertext (staged in this cipher's scratch).
    pub fn seal<'a>(&'a mut self, id: u64, offset: usize, buf: &'a [u8]) -> Result<&'a [u8], Error> {
        if id == 0 {
            return Ok(buf);
        }
        Self::fit(offset, buf.len())?;
        let out = &mut self.scratch[..buf.len()];
        out.copy_from_slice(buf);
        self.xts.encrypt_unit_at(id, offset, out).map_err(|_| Error::BadBlockSize(buf.len(), BLOCK_SIZE))?;
        Ok(&self.scratch[..buf.len()])
    }

    fn fit(offset: usize, len: usize) -> Result<(), Error> {
        if len == 0 || offset % 16 != 0 || len % 16 != 0 || offset + len > BLOCK_SIZE as usize {
            return Err(Error::BadBlockSize(len, BLOCK_SIZE));
        }
        Ok(())
    }
}

/// A trait representing a block storage device.
///
/// This trait abstracts over physical disks, memory buffers, or network stores.
/// Implementations must enforce `BLOCK_SIZE` alignment.
pub trait BlockDevice {
    /// Read a block from the device.
    ///
    /// # Arguments
    /// * `id` - The block ID to read.
    /// * `buf` - The buffer to read into. Must be exactly `BLOCK_SIZE` bytes.
    fn read_block(&mut self, id: u64, buf: &mut [u8]) -> Result<(), Error>;

    /// Write a block to the device.
    ///
    /// # Arguments
    /// * `id` - The block ID to write to.
    /// * `buf` - The buffer to write. Must be exactly `BLOCK_SIZE` bytes.
    fn write_block(&mut self, id: u64, buf: &[u8]) -> Result<(), Error>;

    /// Return the total number of blocks in the device.
    fn block_count(&self) -> u64;

    /// Flush any buffered data to the underlying storage.
    fn flush(&mut self) -> Result<(), Error>;

    /// Write ONE 512 B sector inside block `id` (`sector` in `0..8`), leaving
    /// the block's other sectors untouched. This is the K8a root-flip
    /// primitive: the commit's atomic point must be a single-sector device
    /// write, not an 8-sector block rewrite. The default is a read-modify-
    /// write of the whole block (correct for host/test backends, which have
    /// no sector-tear model); the kernel's sector adapter overrides it with a
    /// true single-sector write to the medium.
    fn write_sector_in_block(
        &mut self,
        id: u64,
        sector: usize,
        buf: &[u8],
    ) -> Result<(), Error> {
        const SECTOR: usize = 512;
        if buf.len() != SECTOR {
            return Err(Error::BadBlockSize(buf.len(), SECTOR as u64));
        }
        if sector >= (BLOCK_SIZE as usize) / SECTOR {
            return Err(Error::OutOfBounds(id));
        }
        let mut block = alloc::vec![0u8; BLOCK_SIZE as usize];
        self.read_block(id, &mut block)?;
        block[sector * SECTOR..(sector + 1) * SECTOR].copy_from_slice(buf);
        self.write_block(id, &block)
    }

    /// XTS (B689): carry `cipher` from now on — every block but block 0 decrypted on read and encrypted on
    /// write ([`VolumeCipher`]); `None` removes it. `UnaFS::mount_with` installs it for a `cipher = 1` volume
    /// before it reads anything past block 0. The default REFUSES a cipher ([`Error::NoCipher`]): a device
    /// that cannot carry one never mounts a ciphered volume. `MemDevice`, `FileDevice` and `BlockAdapter`
    /// carry it.
    fn set_cipher(&mut self, cipher: Option<VolumeCipher>) -> Result<(), Error> {
        match cipher {
            None => Ok(()),
            Some(_) => Err(Error::NoCipher),
        }
    }
}

/// A block device backed by a file on the host OS.
#[cfg(feature = "std")]
pub struct FileDevice {
    file: File,
    /// XTS (B689): the volume cipher of a mounted `cipher = 1` volume.
    cipher: Option<VolumeCipher>,
}

#[cfg(feature = "std")]
impl FileDevice {
    /// Open a file as a block device.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .open(path)?;
        Ok(Self { file, cipher: None })
    }
    /// Open a file as a read-only block device (for Sentinel/Diagnostics).
    pub fn open_read_only(path: impl AsRef<Path>) -> Result<Self, std::io::Error> {
        let file = OpenOptions::new()
            .read(true)
            .write(false)
            .create(false)
            .open(path)?;
        Ok(Self { file, cipher: None })
    }
}

#[cfg(feature = "std")]
impl BlockDevice for FileDevice {
    fn read_block(&mut self, id: u64, buf: &mut [u8]) -> Result<(), Error> {
        if buf.len() as u64 != BLOCK_SIZE {
            return Err(Error::BadBlockSize(buf.len(), BLOCK_SIZE));
        }

        self.file
            .seek(SeekFrom::Start(id * BLOCK_SIZE))
            .map_err(|e| Error::Io(e.to_string()))?;

        self.file
            .read_exact(buf)
            .map_err(|e| Error::Io(e.to_string()))?;

        if let Some(c) = &self.cipher {
            c.open(id, 0, buf)?;
        }
        Ok(())
    }

    fn write_block(&mut self, id: u64, buf: &[u8]) -> Result<(), Error> {
        if buf.len() as u64 != BLOCK_SIZE {
            return Err(Error::BadBlockSize(buf.len(), BLOCK_SIZE));
        }

        let buf = match self.cipher.as_mut() {
            Some(c) => c.seal(id, 0, buf)?,
            None => buf,
        };
        let mut file = &self.file;
        file.seek(SeekFrom::Start(id * BLOCK_SIZE))
            .map_err(|e| Error::Io(e.to_string()))?;

        file.write_all(buf).map_err(|e| Error::Io(e.to_string()))?;

        Ok(())
    }

    fn block_count(&self) -> u64 {
        self.file
            .metadata()
            .map(|m| m.len() / BLOCK_SIZE)
            .unwrap_or(0)
    }

    fn flush(&mut self) -> Result<(), Error> {
        self.file.sync_all().map_err(|e| Error::Io(e.to_string()))
    }

    fn set_cipher(&mut self, cipher: Option<VolumeCipher>) -> Result<(), Error> {
        self.cipher = cipher;
        Ok(())
    }
}

/// An in-memory block device backed by a `Vec<u8>`.
///
/// Useful for testing and temporary filesystems.
#[derive(Clone)]
pub struct MemDevice {
    data: Vec<u8>,
    /// XTS (B689): the volume cipher of a mounted `cipher = 1` volume (`as_bytes` shows the medium: ciphertext).
    cipher: Option<VolumeCipher>,
}

impl Default for MemDevice {
    fn default() -> Self {
        Self::new()
    }
}

impl MemDevice {
    /// Create a new empty `MemDevice`.
    pub fn new() -> Self {
        Self { data: Vec::new(), cipher: None }
    }

    /// UNAFSGROW: a zero-filled device of exactly `blocks` blocks (so
    /// `block_count` reports the full span, not the high-water mark).
    pub fn with_blocks(blocks: u64) -> Self {
        Self { data: alloc::vec![0u8; (blocks * BLOCK_SIZE) as usize], cipher: None }
    }

    /// UNAFSGROW: a device over an existing image (`bytes.len()` should be a
    /// whole number of blocks; a partial tail block is unreadable).
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self { data: bytes, cipher: None }
    }

    /// UNAFSGROW: extend (zero-fill) or truncate the device to `blocks`.
    pub fn resize_blocks(&mut self, blocks: u64) {
        self.data.resize((blocks * BLOCK_SIZE) as usize, 0);
    }

    /// The image bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// The image bytes, by value.
    pub fn into_bytes(self) -> Vec<u8> {
        self.data
    }
}

impl BlockDevice for MemDevice {
    fn read_block(&mut self, id: u64, buf: &mut [u8]) -> Result<(), Error> {
        if buf.len() as u64 != BLOCK_SIZE {
            return Err(Error::BadBlockSize(buf.len(), BLOCK_SIZE));
        }

        let start = (id * BLOCK_SIZE) as usize;
        let end = start + BLOCK_SIZE as usize;

        if start >= self.data.len() {
            // Reading uninitialized memory - return zeros? Or error?
            // "We define our own reality."
            // Standard behavior for unwritten blocks is usually zeros or error.
            // Let's return zeros for simplicity in a "sparse" like feel,
            // but strict implementation might error.
            // Given "MemDevice (RAM Disk)", usually it has a size.
            // If I try to read beyond the end of the "disk", it should probably be an error or zeros.
            // Let's assume explicit resizing is not in the trait, so write expands, read beyond end returns zeros?
            // Or should read fail if not written?
            // "The file system never hangs."
            // Let's go with: if it's out of bounds of the current vector, we treat it as zeros (sparse)
            // OR we return OutOfBounds.
            // Let's return OutOfBounds for now to be safe, unless we want auto-growth on read (which is weird).
            // Actually, if we want to support "Resizing", maybe we should just error if out of bounds.
            return Err(Error::OutOfBounds(id));
        }

        // partial read handling if data len is not multiple of block size (shouldn't happen if we only write blocks)
        if end > self.data.len() {
            return Err(Error::OutOfBounds(id));
        }

        buf.copy_from_slice(&self.data[start..end]);
        if let Some(c) = &self.cipher {
            c.open(id, 0, buf)?;
        }
        Ok(())
    }

    fn write_block(&mut self, id: u64, buf: &[u8]) -> Result<(), Error> {
        if buf.len() as u64 != BLOCK_SIZE {
            return Err(Error::BadBlockSize(buf.len(), BLOCK_SIZE));
        }

        let buf = match self.cipher.as_mut() {
            Some(c) => c.seal(id, 0, buf)?,
            None => buf,
        };
        let start = (id * BLOCK_SIZE) as usize;
        let end = start + BLOCK_SIZE as usize;

        if end > self.data.len() {
            // Resize to accommodate new block
            self.data.resize(end, 0);
        }

        self.data[start..end].copy_from_slice(buf);
        Ok(())
    }

    fn block_count(&self) -> u64 {
        (self.data.len() as u64) / BLOCK_SIZE
    }

    fn flush(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn set_cipher(&mut self, cipher: Option<VolumeCipher>) -> Result<(), Error> {
        self.cipher = cipher;
        Ok(())
    }
}
