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

//! The kernel block adapter (BeFS-K2): present a 512 B-sector device as a
//! 4096 B [`BlockDevice`](crate::storage::BlockDevice), with GPT/MBR partition
//! offsets.
//!
//! The kernel exposes storage as 512 B logical sectors (USB/SD), possibly at a
//! partition offset inside a GPT- or MBR-partitioned volume. UnaFS speaks 4096 B
//! blocks. [`BlockAdapter`] bridges the two: one 4096 B block maps to exactly
//! eight contiguous 512 B sectors at `base_lba + block * 8`, where `base_lba` is
//! the partition's starting LBA.
//!
//! The seam is generic over a [`SectorDevice`] trait, so the crate is host-tested
//! with the in-memory [`MemSectorDevice`] backing while the kernel wires its real
//! sector driver in a later arc (BeFS-K3). Everything here is `no_std` + `alloc`.
//!
//! ## Bound checks
//!
//! All block/sector arithmetic is checked. [`BlockAdapter`] rejects blocks at or
//! beyond its configured `block_count` ([`Error::OutOfBounds`]) before touching
//! the device, and every `base_lba + id*8 + i` computation uses `checked_*` so a
//! crafted or corrupt partition span can never wrap into an in-bounds sector.
//! The partition-table parser ([`parse_partitions`]) validates signatures, entry
//! sizes, LBA ordering, and rejects any partition whose extent exceeds the
//! backing device.

use alloc::string::String;
use alloc::vec::Vec;

use crate::storage::{BLOCK_SIZE, BlockDevice, Error as StorageError};
use crate::superblock;

/// The kernel logical sector size (bytes).
pub const SECTOR_SIZE: u64 = 512;

/// Number of 512 B sectors per 4096 B UnaFS block.
pub const SECTORS_PER_BLOCK: u64 = BLOCK_SIZE / SECTOR_SIZE; // 8

/// Error raised by a [`SectorDevice`] backend.
///
/// A kernel driver implementing [`SectorDevice`] constructs these; the adapter
/// maps them onto [`crate::storage::Error`] for the [`BlockDevice`] surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SectorError {
    /// The requested LBA is outside the device.
    OutOfBounds(u64),
    /// A backend-specific I/O failure, carrying a diagnostic message.
    Io(String),
}

impl core::fmt::Display for SectorError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            SectorError::OutOfBounds(lba) => write!(f, "sector out of bounds: {lba}"),
            SectorError::Io(msg) => write!(f, "sector IO error: {msg}"),
        }
    }
}

impl core::error::Error for SectorError {}

/// A 512 B logical-sector storage device.
///
/// This is the primitive the kernel provides (its USB/SD block driver). Buffers
/// passed to `read_sector`/`write_sector` must be exactly [`SECTOR_SIZE`] bytes.
pub trait SectorDevice {
    /// Read one 512 B sector at `lba` into `buf` (exactly `SECTOR_SIZE` bytes).
    fn read_sector(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), SectorError>;

    /// Write one 512 B sector at `lba` from `buf` (exactly `SECTOR_SIZE` bytes).
    fn write_sector(&mut self, lba: u64, buf: &[u8]) -> Result<(), SectorError>;

    /// Total number of 512 B sectors on the device.
    fn sector_count(&self) -> u64;

    /// Flush any buffered writes to the underlying medium. Default: no-op.
    fn flush(&mut self) -> Result<(), SectorError> {
        Ok(())
    }
}

/// A `&mut S` is itself a [`SectorDevice`], so an adapter can borrow a device
/// for a probe (e.g. [`locate_unafs`]) and hand it back.
impl<S: SectorDevice + ?Sized> SectorDevice for &mut S {
    fn read_sector(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), SectorError> {
        (**self).read_sector(lba, buf)
    }
    fn write_sector(&mut self, lba: u64, buf: &[u8]) -> Result<(), SectorError> {
        (**self).write_sector(lba, buf)
    }
    fn sector_count(&self) -> u64 {
        (**self).sector_count()
    }
    fn flush(&mut self) -> Result<(), SectorError> {
        (**self).flush()
    }
}

fn map_sector_err(e: SectorError) -> StorageError {
    match e {
        SectorError::OutOfBounds(lba) => StorageError::OutOfBounds(lba),
        SectorError::Io(msg) => StorageError::Io(msg),
    }
}

/// Presents a 512 B [`SectorDevice`] as a 4096 B [`BlockDevice`], applying a
/// partition base-LBA offset.
///
/// Block `id` maps to the eight sectors `[base_lba + id*8, base_lba + id*8 + 8)`.
/// `block_count` bounds the exposed volume: reads/writes at or beyond it fail
/// with [`Error::OutOfBounds`](crate::storage::Error::OutOfBounds).
pub struct BlockAdapter<S: SectorDevice> {
    dev: S,
    base_lba: u64,
    block_count: u64,
}

impl<S: SectorDevice> BlockAdapter<S> {
    /// Construct an adapter over `dev` at partition offset `base_lba`, exposing
    /// `block_count` 4096 B blocks. The caller is responsible for `block_count`
    /// fitting the device; every access is still bound-checked against it and
    /// against the device at read time.
    pub fn new(dev: S, base_lba: u64, block_count: u64) -> Self {
        Self {
            dev,
            base_lba,
            block_count,
        }
    }

    /// Construct an adapter over the whole device (`base_lba = 0`), exposing as
    /// many whole 4096 B blocks as the device's sector count allows.
    pub fn whole(dev: S) -> Self {
        let block_count = dev.sector_count() / SECTORS_PER_BLOCK;
        Self {
            dev,
            base_lba: 0,
            block_count,
        }
    }

    /// Construct an adapter positioned on a located partition span.
    pub fn for_partition(dev: S, span: &PartitionSpan) -> Self {
        Self {
            dev,
            base_lba: span.base_lba,
            block_count: span.block_count,
        }
    }

    /// The partition base LBA (in 512 B sectors) this adapter is offset by.
    pub fn base_lba(&self) -> u64 {
        self.base_lba
    }

    /// Consume the adapter and return the underlying sector device.
    pub fn into_inner(self) -> S {
        self.dev
    }

    /// Compute the first sector LBA for a block id, fully bound-checked.
    fn first_lba(&self, id: u64) -> Result<u64, StorageError> {
        id.checked_mul(SECTORS_PER_BLOCK)
            .and_then(|s| self.base_lba.checked_add(s))
            .ok_or(StorageError::OutOfBounds(id))
    }
}

impl<S: SectorDevice> BlockDevice for BlockAdapter<S> {
    fn read_block(&mut self, id: u64, buf: &mut [u8]) -> Result<(), StorageError> {
        if buf.len() as u64 != BLOCK_SIZE {
            return Err(StorageError::BadBlockSize(buf.len(), BLOCK_SIZE));
        }
        if id >= self.block_count {
            return Err(StorageError::OutOfBounds(id));
        }
        let first = self.first_lba(id)?;
        for i in 0..SECTORS_PER_BLOCK {
            let lba = first.checked_add(i).ok_or(StorageError::OutOfBounds(id))?;
            let start = (i * SECTOR_SIZE) as usize;
            let end = start + SECTOR_SIZE as usize;
            self.dev
                .read_sector(lba, &mut buf[start..end])
                .map_err(map_sector_err)?;
        }
        Ok(())
    }

    fn write_block(&mut self, id: u64, buf: &[u8]) -> Result<(), StorageError> {
        if buf.len() as u64 != BLOCK_SIZE {
            return Err(StorageError::BadBlockSize(buf.len(), BLOCK_SIZE));
        }
        if id >= self.block_count {
            return Err(StorageError::OutOfBounds(id));
        }
        let first = self.first_lba(id)?;
        for i in 0..SECTORS_PER_BLOCK {
            let lba = first.checked_add(i).ok_or(StorageError::OutOfBounds(id))?;
            let start = (i * SECTOR_SIZE) as usize;
            let end = start + SECTOR_SIZE as usize;
            self.dev
                .write_sector(lba, &buf[start..end])
                .map_err(map_sector_err)?;
        }
        Ok(())
    }

    fn block_count(&self) -> u64 {
        self.block_count
    }

    fn flush(&mut self) -> Result<(), StorageError> {
        self.dev.flush().map_err(map_sector_err)
    }

    /// K8a root-flip primitive: ONE real 512 B sector write to the medium —
    /// this override is what makes the commit's root flip genuinely atomic
    /// at the device's own write granularity (the trait default would
    /// read-modify-write all eight sectors of the block).
    fn write_sector_in_block(
        &mut self,
        id: u64,
        sector: usize,
        buf: &[u8],
    ) -> Result<(), StorageError> {
        if buf.len() as u64 != SECTOR_SIZE {
            return Err(StorageError::BadBlockSize(buf.len(), SECTOR_SIZE));
        }
        if sector as u64 >= SECTORS_PER_BLOCK {
            return Err(StorageError::OutOfBounds(id));
        }
        if id >= self.block_count {
            return Err(StorageError::OutOfBounds(id));
        }
        let lba = self
            .first_lba(id)?
            .checked_add(sector as u64)
            .ok_or(StorageError::OutOfBounds(id))?;
        self.dev.write_sector(lba, buf).map_err(map_sector_err)
    }
}

// ---------------------------------------------------------------------------
// In-memory 512 B sector backing (host tests + kernel-free bring-up).
// ---------------------------------------------------------------------------

/// An in-memory [`SectorDevice`] backed by a `Vec<u8>` — the 512 B-sector twin
/// of [`MemDevice`](crate::storage::MemDevice), for host tests.
#[derive(Clone, Default)]
pub struct MemSectorDevice {
    data: Vec<u8>,
}

impl MemSectorDevice {
    /// Create an empty device (zero sectors).
    pub fn new() -> Self {
        Self { data: Vec::new() }
    }

    /// Create a zeroed device of `sectors` 512 B sectors.
    pub fn with_sectors(sectors: usize) -> Self {
        Self {
            data: alloc::vec![0u8; sectors * SECTOR_SIZE as usize],
        }
    }

    /// Wrap raw bytes as a device, padding up to a whole-sector multiple.
    pub fn from_bytes(mut data: Vec<u8>) -> Self {
        let ssz = SECTOR_SIZE as usize;
        let rem = data.len() % ssz;
        if rem != 0 {
            data.resize(data.len() + (ssz - rem), 0);
        }
        Self { data }
    }

    /// Borrow the raw backing bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.data
    }

    /// Mutably borrow the raw backing bytes (test fixtures poke sectors here).
    pub fn as_mut_bytes(&mut self) -> &mut [u8] {
        &mut self.data
    }
}

impl SectorDevice for MemSectorDevice {
    fn read_sector(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), SectorError> {
        if buf.len() as u64 != SECTOR_SIZE {
            return Err(SectorError::Io(alloc::format!(
                "read buffer {} != sector size {}",
                buf.len(),
                SECTOR_SIZE
            )));
        }
        let start = lba
            .checked_mul(SECTOR_SIZE)
            .ok_or(SectorError::OutOfBounds(lba))? as usize;
        let end = start + SECTOR_SIZE as usize;
        if end > self.data.len() {
            return Err(SectorError::OutOfBounds(lba));
        }
        buf.copy_from_slice(&self.data[start..end]);
        Ok(())
    }

    fn write_sector(&mut self, lba: u64, buf: &[u8]) -> Result<(), SectorError> {
        if buf.len() as u64 != SECTOR_SIZE {
            return Err(SectorError::Io(alloc::format!(
                "write buffer {} != sector size {}",
                buf.len(),
                SECTOR_SIZE
            )));
        }
        let start = lba
            .checked_mul(SECTOR_SIZE)
            .ok_or(SectorError::OutOfBounds(lba))? as usize;
        let end = start + SECTOR_SIZE as usize;
        if end > self.data.len() {
            self.data.resize(end, 0);
        }
        self.data[start..end].copy_from_slice(buf);
        Ok(())
    }

    fn sector_count(&self) -> u64 {
        (self.data.len() as u64) / SECTOR_SIZE
    }
}

// ---------------------------------------------------------------------------
// GPT / MBR partition-table parsing.
// ---------------------------------------------------------------------------

/// Which partitioning scheme a device carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartitionScheme {
    /// Legacy MBR partition table (LBA 0).
    Mbr,
    /// GUID Partition Table (protective MBR + GPT header at LBA 1).
    Gpt,
}

/// One partition entry, normalized across MBR and GPT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partition {
    /// Starting LBA (in 512 B sectors).
    pub start_lba: u64,
    /// Length in 512 B sectors.
    pub sector_count: u64,
    /// GPT partition type GUID (raw 16 bytes), if this came from a GPT.
    pub type_guid: Option<[u8; 16]>,
    /// MBR partition type byte, if this came from an MBR.
    pub type_byte: Option<u8>,
}

/// A parsed partition table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionTable {
    /// The scheme the table was parsed as.
    pub scheme: PartitionScheme,
    /// The non-empty partitions, in table order.
    pub partitions: Vec<Partition>,
}

/// A resolved UnaFS partition span, ready to feed [`BlockAdapter::for_partition`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartitionSpan {
    /// Partition base LBA (512 B sectors).
    pub base_lba: u64,
    /// Number of whole 4096 B blocks the partition holds.
    pub block_count: u64,
}

/// Errors from partition-table parsing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PartError {
    /// LBA 0 lacks the 0x55AA boot signature.
    NoSignature,
    /// The GPT header at LBA 1 lacks the "EFI PART" signature.
    BadGptSignature,
    /// A structural field was out of a sane range (message names the field).
    Malformed(&'static str),
    /// A partition extent exceeds the backing device (its start LBA).
    OutOfBounds(u64),
    /// The underlying sector device failed.
    Device(SectorError),
}

impl core::fmt::Display for PartError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            PartError::NoSignature => write!(f, "no MBR boot signature (0x55AA) at LBA 0"),
            PartError::BadGptSignature => write!(f, "bad GPT signature at LBA 1"),
            PartError::Malformed(what) => write!(f, "malformed partition table: {what}"),
            PartError::OutOfBounds(lba) => write!(f, "partition extends past device at LBA {lba}"),
            PartError::Device(e) => write!(f, "device error: {e}"),
        }
    }
}

impl core::error::Error for PartError {}

impl From<SectorError> for PartError {
    fn from(e: SectorError) -> Self {
        PartError::Device(e)
    }
}

const MBR_SIG_OFFSET: usize = 510;
const MBR_ENTRY_BASE: usize = 446;
const MBR_ENTRY_SIZE: usize = 16;
/// MBR type byte marking a protective/hybrid GPT.
const MBR_TYPE_GPT_PROTECTIVE: u8 = 0xEE;
/// Upper sanity bound on GPT partition entry count.
const GPT_MAX_ENTRIES: u64 = 65536;

fn le_u32(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn le_u64(b: &[u8]) -> u64 {
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

/// Read `out.len()` bytes starting at absolute byte offset `byte_off`, spanning
/// sector boundaries. Bound-checked via the device's own `read_sector`.
fn read_bytes<S: SectorDevice>(
    dev: &mut S,
    byte_off: u64,
    out: &mut [u8],
) -> Result<(), PartError> {
    let mut done: usize = 0;
    let mut sector = [0u8; SECTOR_SIZE as usize];
    while done < out.len() {
        let abs = byte_off
            .checked_add(done as u64)
            .ok_or(PartError::Malformed("byte offset overflow"))?;
        let lba = abs / SECTOR_SIZE;
        let within = (abs % SECTOR_SIZE) as usize;
        dev.read_sector(lba, &mut sector)?;
        let n = core::cmp::min(SECTOR_SIZE as usize - within, out.len() - done);
        out[done..done + n].copy_from_slice(&sector[within..within + n]);
        done += n;
    }
    Ok(())
}

/// Parse the partition table on `dev`.
///
/// Reads the MBR at LBA 0; if a protective GPT entry (type `0xEE`) is present,
/// parses the GPT at LBA 1, otherwise parses the four MBR primaries. Empty
/// slots are skipped. Every partition is bound-checked against the device sector
/// count; a partition extending past the device is rejected with
/// [`PartError::OutOfBounds`].
pub fn parse_partitions<S: SectorDevice>(dev: &mut S) -> Result<PartitionTable, PartError> {
    let mut mbr = [0u8; SECTOR_SIZE as usize];
    dev.read_sector(0, &mut mbr)?;

    if mbr[MBR_SIG_OFFSET] != 0x55 || mbr[MBR_SIG_OFFSET + 1] != 0xAA {
        return Err(PartError::NoSignature);
    }

    // Detect a protective/hybrid GPT among the four primary entries.
    let mut is_gpt = false;
    for i in 0..4 {
        let off = MBR_ENTRY_BASE + i * MBR_ENTRY_SIZE;
        if mbr[off + 4] == MBR_TYPE_GPT_PROTECTIVE {
            is_gpt = true;
            break;
        }
    }

    if is_gpt {
        parse_gpt(dev)
    } else {
        parse_mbr(&mbr, dev.sector_count())
    }
}

fn parse_mbr(mbr: &[u8], dev_sectors: u64) -> Result<PartitionTable, PartError> {
    let mut partitions = Vec::new();
    for i in 0..4 {
        let off = MBR_ENTRY_BASE + i * MBR_ENTRY_SIZE;
        let type_byte = mbr[off + 4];
        if type_byte == 0 {
            continue; // empty slot
        }
        let start_lba = le_u32(&mbr[off + 8..off + 12]) as u64;
        let sector_count = le_u32(&mbr[off + 12..off + 16]) as u64;
        if sector_count == 0 {
            continue;
        }
        let end = start_lba
            .checked_add(sector_count)
            .ok_or(PartError::Malformed("MBR extent overflow"))?;
        if end > dev_sectors {
            return Err(PartError::OutOfBounds(start_lba));
        }
        partitions.push(Partition {
            start_lba,
            sector_count,
            type_guid: None,
            type_byte: Some(type_byte),
        });
    }
    Ok(PartitionTable {
        scheme: PartitionScheme::Mbr,
        partitions,
    })
}

fn parse_gpt<S: SectorDevice>(dev: &mut S) -> Result<PartitionTable, PartError> {
    let mut hdr = [0u8; SECTOR_SIZE as usize];
    dev.read_sector(1, &mut hdr)?;

    if &hdr[0..8] != b"EFI PART" {
        return Err(PartError::BadGptSignature);
    }

    let entries_lba = le_u64(&hdr[72..80]);
    let num_entries = le_u32(&hdr[80..84]) as u64;
    let entry_size = le_u32(&hdr[84..88]) as u64;

    // Sanity bounds: a GPT entry is at least 128 bytes; cap the array so a
    // corrupt count can't spin us. We only read the first 128 bytes (type GUID
    // + first/last LBA) of each entry, so entry_size only advances the cursor.
    if entry_size < 128 {
        return Err(PartError::Malformed("GPT entry size too small"));
    }
    if num_entries == 0 || num_entries > GPT_MAX_ENTRIES {
        return Err(PartError::Malformed("GPT entry count out of range"));
    }

    let dev_sectors = dev.sector_count();
    let base_byte = entries_lba
        .checked_mul(SECTOR_SIZE)
        .ok_or(PartError::Malformed("GPT entry array offset overflow"))?;

    let mut partitions = Vec::new();
    for i in 0..num_entries {
        let ent_off = i
            .checked_mul(entry_size)
            .and_then(|o| base_byte.checked_add(o))
            .ok_or(PartError::Malformed("GPT entry offset overflow"))?;

        let mut ent = [0u8; 128];
        read_bytes(dev, ent_off, &mut ent)?;

        // Unused entries have an all-zero type GUID.
        if ent[0..16].iter().all(|&b| b == 0) {
            continue;
        }

        let first = le_u64(&ent[32..40]);
        let last = le_u64(&ent[40..48]);
        if last < first {
            return Err(PartError::Malformed("GPT entry last < first LBA"));
        }
        let sector_count = last
            .checked_sub(first)
            .and_then(|d| d.checked_add(1))
            .ok_or(PartError::Malformed("GPT extent overflow"))?;
        let end = first
            .checked_add(sector_count)
            .ok_or(PartError::Malformed("GPT extent overflow"))?;
        if end > dev_sectors {
            return Err(PartError::OutOfBounds(first));
        }

        let mut type_guid = [0u8; 16];
        type_guid.copy_from_slice(&ent[0..16]);
        partitions.push(Partition {
            start_lba: first,
            sector_count,
            type_guid: Some(type_guid),
            type_byte: None,
        });
    }

    Ok(PartitionTable {
        scheme: PartitionScheme::Gpt,
        partitions,
    })
}

/// Locate the UnaFS partition on `dev` and return its span.
///
/// Parses the partition table, then probes each partition's block 0 for the
/// UnaFS superblock magic (`b"UNAFS"`). This identifies the volume by its
/// on-disk signature rather than by a reserved partition type, so any tool that
/// carved the partition works. Returns the first match, or `None` if no
/// partition holds a UnaFS superblock.
pub fn locate_unafs<S: SectorDevice>(
    dev: &mut S,
) -> Result<Option<PartitionSpan>, PartError> {
    let table = parse_partitions(dev)?;
    for p in &table.partitions {
        let block_count = p.sector_count / SECTORS_PER_BLOCK;
        if block_count == 0 {
            continue; // too small to hold even block 0
        }
        let mut adapter = BlockAdapter::new(&mut *dev, p.start_lba, block_count);
        let mut buf = alloc::vec![0u8; BLOCK_SIZE as usize];
        if adapter.read_block(0, &mut buf).is_ok()
            && buf[0..superblock::MAGIC.len()] == superblock::MAGIC
        {
            return Ok(Some(PartitionSpan {
                base_lba: p.start_lba,
                block_count,
            }));
        }
    }
    Ok(None)
}

// ---------------------------------------------------------------------------
// Unit tests (std harness).
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Fill each sector with a byte equal to (lba & 0xFF) so mapping is checkable.
    fn patterned(sectors: usize) -> MemSectorDevice {
        let mut dev = MemSectorDevice::with_sectors(sectors);
        let bytes = dev.as_mut_bytes();
        for lba in 0..sectors {
            let start = lba * SECTOR_SIZE as usize;
            for b in &mut bytes[start..start + SECTOR_SIZE as usize] {
                *b = (lba & 0xFF) as u8;
            }
        }
        dev
    }

    #[test]
    fn block_maps_to_eight_contiguous_sectors() {
        let dev = patterned(64);
        let mut ad = BlockAdapter::whole(dev);
        assert_eq!(ad.block_count(), 8); // 64 sectors / 8

        let mut buf = alloc::vec![0u8; BLOCK_SIZE as usize];
        ad.read_block(1, &mut buf).unwrap();
        // Block 1 == sectors 8..16; each sector's fill == its lba.
        for i in 0..8u64 {
            let lba = 8 + i;
            let s = (i * SECTOR_SIZE) as usize;
            assert!(buf[s..s + SECTOR_SIZE as usize]
                .iter()
                .all(|&b| b == (lba & 0xFF) as u8));
        }
    }

    #[test]
    fn partition_offset_shifts_base_lba() {
        let dev = patterned(4096);
        // Partition starts at sector 2048, 256 sectors long => 32 blocks.
        let mut ad = BlockAdapter::new(dev, 2048, 32);
        let mut buf = alloc::vec![0u8; BLOCK_SIZE as usize];
        ad.read_block(0, &mut buf).unwrap();
        // Block 0 of the partition == sectors 2048..2056.
        for i in 0..8u64 {
            let lba = 2048 + i;
            let s = (i * SECTOR_SIZE) as usize;
            assert!(buf[s..s + SECTOR_SIZE as usize]
                .iter()
                .all(|&b| b == (lba & 0xFF) as u8));
        }
    }

    #[test]
    fn out_of_bounds_block_rejected() {
        let dev = MemSectorDevice::with_sectors(64);
        let mut ad = BlockAdapter::whole(dev); // 8 blocks
        let mut buf = alloc::vec![0u8; BLOCK_SIZE as usize];
        match ad.read_block(8, &mut buf) {
            Err(StorageError::OutOfBounds(8)) => {}
            other => panic!("expected OutOfBounds(8), got {other:?}"),
        }
    }

    #[test]
    fn bad_block_size_rejected() {
        let dev = MemSectorDevice::with_sectors(64);
        let mut ad = BlockAdapter::whole(dev);
        let mut small = alloc::vec![0u8; 512];
        assert!(matches!(
            ad.read_block(0, &mut small),
            Err(StorageError::BadBlockSize(512, BLOCK_SIZE))
        ));
    }

    #[test]
    fn write_then_read_roundtrip_with_offset() {
        let dev = MemSectorDevice::with_sectors(4096);
        let mut ad = BlockAdapter::new(dev, 100, 10);
        let payload: Vec<u8> = (0..BLOCK_SIZE as usize).map(|i| (i % 251) as u8).collect();
        ad.write_block(3, &payload).unwrap();
        let mut back = alloc::vec![0u8; BLOCK_SIZE as usize];
        ad.read_block(3, &mut back).unwrap();
        assert_eq!(back, payload);
        // Verify it truly landed at sector 100 + 3*8 = 124.
        let dev = ad.into_inner();
        let base = 124 * SECTOR_SIZE as usize;
        assert_eq!(&dev.as_bytes()[base..base + BLOCK_SIZE as usize], &payload[..]);
    }

    #[test]
    fn checked_arithmetic_no_wrap() {
        // A huge base_lba would wrap without checked math; must OOB instead.
        let dev = MemSectorDevice::with_sectors(64);
        let mut ad = BlockAdapter::new(dev, u64::MAX - 3, u64::MAX);
        let mut buf = alloc::vec![0u8; BLOCK_SIZE as usize];
        assert!(matches!(
            ad.read_block(1, &mut buf),
            Err(StorageError::OutOfBounds(_))
        ));
    }
}
