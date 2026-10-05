// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The BLOCK seam — what [`crate::plan_apply::apply`], [`crate::verify::verify`],
//! [`crate::recover`] and [`crate::fat32_format`] read and write through. A `Block` is a run of
//! 512-byte sectors addressed by LBA; every access is a whole number of sectors and is bounded by
//! [`Block::sectors`] before it reaches the medium (a hostile table must not be able to steer an
//! access past the end).
//!
//! Implementations here: [`MemBlock`] (a byte vector — the KATs and dry runs), [`Window`] (a
//! partition-relative view of another block: LBA 0 of the window is the partition's first sector,
//! accesses past the partition are refused) and, behind the `std` feature only, `FileBlock` (an
//! image file or a device node). The kernel implements `Block` over its own sector drivers.

use alloc::vec::Vec;
use core::fmt;

use crate::gpt::SECTOR;

/// Why a sector access failed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BlockError {
    /// The access runs past the last sector.
    OutOfRange,
    /// The buffer is not a whole number of sectors (or is empty).
    Unaligned,
    /// The medium was opened read-only.
    ReadOnly,
    /// The medium failed the access.
    Io,
}

impl fmt::Display for BlockError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            BlockError::OutOfRange => "sector access past the end of the medium",
            BlockError::Unaligned => "buffer is not a whole number of sectors",
            BlockError::ReadOnly => "medium is read-only",
            BlockError::Io => "medium I/O error",
        })
    }
}

/// A sector-addressed medium.
pub trait Block {
    /// Total 512-byte sectors.
    fn sectors(&self) -> u64;
    /// Read `buf.len() / 512` sectors starting at `lba`.
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError>;
    /// Write `buf.len() / 512` sectors starting at `lba`.
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError>;
    /// Make every completed write durable. Default: nothing buffered.
    fn flush(&mut self) -> Result<(), BlockError> {
        Ok(())
    }
}

/// The bounds rule every implementation applies: whole sectors, non-empty, inside `0..total`.
pub fn check(total: u64, lba: u64, len: usize) -> Result<u64, BlockError> {
    if len == 0 || len % SECTOR != 0 {
        return Err(BlockError::Unaligned);
    }
    let n = (len / SECTOR) as u64;
    match lba.checked_add(n) {
        Some(end) if end <= total => Ok(n),
        _ => Err(BlockError::OutOfRange),
    }
}

/// Read `n` sectors at `lba` into a fresh vector.
pub fn read_vec(dev: &mut dyn Block, lba: u64, n: u64) -> Result<Vec<u8>, BlockError> {
    let len = usize::try_from(n).ok().and_then(|n| n.checked_mul(SECTOR)).ok_or(BlockError::OutOfRange)?;
    check(dev.sectors(), lba, len)?;
    let mut v = alloc::vec![0u8; len];
    dev.read(lba, &mut v)?;
    Ok(v)
}

/// Zero `n` sectors from `lba`, in chunks of at most 256 sectors (128 KiB).
pub fn zero(dev: &mut dyn Block, lba: u64, n: u64) -> Result<(), BlockError> {
    if n == 0 {
        return Ok(());
    }
    lba.checked_add(n).filter(|&e| e <= dev.sectors()).ok_or(BlockError::OutOfRange)?;
    const CHUNK: u64 = 256;
    let buf = alloc::vec![0u8; (CHUNK as usize) * SECTOR];
    let mut done = 0;
    while done < n {
        let k = core::cmp::min(CHUNK, n - done);
        dev.write(lba + done, &buf[..k as usize * SECTOR])?;
        done += k;
    }
    Ok(())
}

/// A whole medium held in memory.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MemBlock {
    pub data: Vec<u8>,
}

impl MemBlock {
    /// A zeroed medium of `sectors` sectors.
    pub fn new(sectors: u64) -> Self {
        Self { data: alloc::vec![0u8; sectors as usize * SECTOR] }
    }
    /// Wrap existing bytes (truncated down to whole sectors).
    pub fn from_bytes(mut data: Vec<u8>) -> Self {
        let whole = data.len() / SECTOR * SECTOR;
        data.truncate(whole);
        Self { data }
    }
    /// The bytes of sector `lba` (panics past the end — test/inspection use).
    pub fn sector(&self, lba: u64) -> &[u8] {
        &self.data[lba as usize * SECTOR..(lba as usize + 1) * SECTOR]
    }
}

impl Block for MemBlock {
    fn sectors(&self) -> u64 {
        (self.data.len() / SECTOR) as u64
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        check(self.sectors(), lba, buf.len())?;
        let o = lba as usize * SECTOR;
        buf.copy_from_slice(&self.data[o..o + buf.len()]);
        Ok(())
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        check(self.sectors(), lba, buf.len())?;
        let o = lba as usize * SECTOR;
        self.data[o..o + buf.len()].copy_from_slice(buf);
        Ok(())
    }
}

/// A partition-relative view: sector 0 of the window is `base` on the parent, and nothing past
/// `base + len` is reachable through it.
pub struct Window<'a> {
    dev: &'a mut dyn Block,
    base: u64,
    len: u64,
}

impl<'a> Window<'a> {
    /// `None` if the span does not lie inside the parent.
    pub fn new(dev: &'a mut dyn Block, base: u64, len: u64) -> Option<Self> {
        let end = base.checked_add(len)?;
        if end > dev.sectors() {
            return None;
        }
        Some(Self { dev, base, len })
    }
    pub fn base(&self) -> u64 {
        self.base
    }
}

impl Block for Window<'_> {
    fn sectors(&self) -> u64 {
        self.len
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        check(self.len, lba, buf.len())?;
        self.dev.read(self.base + lba, buf)
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        check(self.len, lba, buf.len())?;
        self.dev.write(self.base + lba, buf)
    }
    fn flush(&mut self) -> Result<(), BlockError> {
        self.dev.flush()
    }
}

/// A recorder: reads pass through to the parent, writes are CAPTURED (lba + bytes) and never reach
/// it. This is how a dry run is exact — the same code path that would write runs against the
/// recorder, and what it captured IS the list of sector writes, byte for byte. Reads of a sector the
/// recorder already captured return the captured bytes, so read-after-write inside one operation
/// sees its own writes.
pub struct Recorder<'a> {
    dev: &'a mut dyn Block,
    pub writes: Vec<(u64, Vec<u8>)>,
}

impl<'a> Recorder<'a> {
    pub fn new(dev: &'a mut dyn Block) -> Self {
        Self { dev, writes: Vec::new() }
    }
}

impl Block for Recorder<'_> {
    fn sectors(&self) -> u64 {
        self.dev.sectors()
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        check(self.dev.sectors(), lba, buf.len())?;
        self.dev.read(lba, buf)?;
        let n = (buf.len() / SECTOR) as u64;
        for (wl, wb) in &self.writes {
            let wn = (wb.len() / SECTOR) as u64;
            let lo = lba.max(*wl);
            let hi = (lba + n).min(wl + wn);
            for s in lo..hi {
                let d = (s - lba) as usize * SECTOR;
                let o = (s - wl) as usize * SECTOR;
                buf[d..d + SECTOR].copy_from_slice(&wb[o..o + SECTOR]);
            }
        }
        Ok(())
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        check(self.dev.sectors(), lba, buf.len())?;
        self.writes.push((lba, buf.to_vec()));
        Ok(())
    }
}

/// A SPARSE medium: `sectors` long, only the sectors ever written with non-zero bytes are held
/// (an unwritten or zero-written sector reads as zeros). A 200 MiB card or a 500 GB disk is a few
/// kilobytes of map — so the write-path KATs run in-kernel too.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SparseBlock {
    sectors: u64,
    pub map: alloc::collections::BTreeMap<u64, [u8; SECTOR]>,
}

impl SparseBlock {
    pub fn new(sectors: u64) -> Self {
        Self { sectors, map: alloc::collections::BTreeMap::new() }
    }
    /// A copy of sector `lba` (zeros when never written).
    pub fn sector(&self, lba: u64) -> [u8; SECTOR] {
        self.map.get(&lba).copied().unwrap_or([0u8; SECTOR])
    }
    /// Change the medium's length (a grown or shrunk disk); sectors past a shrunk end are dropped.
    pub fn resize(&mut self, sectors: u64) {
        self.sectors = sectors;
        self.map.retain(|&k, _| k < sectors);
    }
}

impl Block for SparseBlock {
    fn sectors(&self) -> u64 {
        self.sectors
    }
    fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), BlockError> {
        let n = check(self.sectors, lba, buf.len())?;
        for i in 0..n {
            let d = &mut buf[i as usize * SECTOR..(i as usize + 1) * SECTOR];
            match self.map.get(&(lba + i)) {
                Some(s) => d.copy_from_slice(s),
                None => d.fill(0),
            }
        }
        Ok(())
    }
    fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), BlockError> {
        let n = check(self.sectors, lba, buf.len())?;
        for i in 0..n {
            let src = &buf[i as usize * SECTOR..(i as usize + 1) * SECTOR];
            if src.iter().all(|&b| b == 0) {
                self.map.remove(&(lba + i));
            } else {
                let mut s = [0u8; SECTOR];
                s.copy_from_slice(src);
                self.map.insert(lba + i, s);
            }
        }
        Ok(())
    }
}
