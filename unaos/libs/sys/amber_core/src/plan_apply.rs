// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PLAN → DISK. [`apply`] lays a [`Plan`]'s GUID Partition Table on a [`Block`], in one of two
//! modes: [`Mode::DryRun`] returns the EXACT sector writes (LBA, length, bytes, CRC-32 of the bytes)
//! and touches nothing; [`Mode::Write`] issues those same writes, flushes, then re-reads the medium
//! through [`crate::verify::verify`] and refuses success unless the table read back is valid AND is
//! the plan's (every entry, the disk GUID, the usable range). The dry run and the write share one
//! list ([`writes`]), so what was shown is what is written.
//!
//! [`render`] is the one text form of a dry run, and [`canonical`] its bytes — the thing a caller
//! digests and signs, so an `Apply` can be refused unless it presents the digest of the plan it was
//! shown (the Amber Bytes handler's signed-plan rule; the digest itself is the caller's, through its
//! `Signer`, since this core carries no hash beyond CRC-32).
//!
//! Write order is the one the kernel installer has always used (MBR, primary header, primary array,
//! backup array, backup header). A tear between writes leaves one valid header in every case but the
//! first two writes; [`crate::recover`] restores the other from it.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::{self, Write as _};

use crate::block::{Block, BlockError};
use crate::crc32::crc32;
use crate::gpt::{self, GptError, ALIGN, SECTOR};
use crate::plan::Plan;
use crate::verify::{self, VerifyReport};

/// One sector write: where, what, and the bytes — or a run of zero sectors (`zeros > 0`, `data`
/// empty), so a format's FAT clear is one entry and not megabytes held in memory.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SectorWrite {
    pub lba: u64,
    pub what: &'static str,
    pub data: Vec<u8>,
    pub zeros: u64,
}

impl SectorWrite {
    pub fn bytes(lba: u64, what: &'static str, data: Vec<u8>) -> Self {
        Self { lba, what, data, zeros: 0 }
    }
    pub fn zero(lba: u64, what: &'static str, sectors: u64) -> Self {
        Self { lba, what, data: Vec::new(), zeros: sectors }
    }
    pub fn sectors(&self) -> u64 {
        (self.data.len() / SECTOR) as u64 + self.zeros
    }
    /// CRC-32 of the bytes this write puts on the medium (a zero run is streamed, not allocated).
    pub fn crc32(&self) -> u32 {
        if self.zeros == 0 {
            return crc32(&self.data);
        }
        let z = [0u8; SECTOR];
        let mut c = crate::crc32::Crc32::new();
        for _ in 0..self.zeros {
            c.update(&z);
        }
        c.finish()
    }
    /// Issue this write.
    pub fn issue(&self, dev: &mut dyn Block) -> Result<(), BlockError> {
        if self.zeros > 0 {
            crate::block::zero(dev, self.lba, self.zeros)
        } else {
            dev.write(self.lba, &self.data)
        }
    }
}

impl fmt::Display for SectorWrite {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "lba {:<10} x{:<7} {:<15} {}crc32={:08X}",
            self.lba,
            self.sectors(),
            self.what,
            if self.zeros > 0 { "zero " } else { "" },
            self.crc32()
        )
    }
}

/// Dry run or write.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    DryRun,
    Write,
}

/// Why a plan was not applied.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ApplyError {
    /// The plan was made for a disk of a different size than the medium in hand.
    DiskMismatch { plan: u64, disk: u64 },
    /// A partition of the plan does not start 1 MiB-aligned.
    Misaligned(usize),
    /// The table did not encode.
    Gpt(GptError),
    /// The medium refused an access.
    Block(BlockError),
    /// The write went through but the table read back is not valid or is not the plan's.
    Verify(String),
}

impl fmt::Display for ApplyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ApplyError::DiskMismatch { plan, disk } => {
                write!(f, "plan is for a {} sector disk, the medium has {}", plan, disk)
            }
            ApplyError::Misaligned(i) => write!(f, "partition p{} is not 1 MiB aligned", i + 1),
            ApplyError::Gpt(e) => write!(f, "table: {}", e),
            ApplyError::Block(e) => write!(f, "medium: {}", e),
            ApplyError::Verify(s) => write!(f, "read-back verify failed: {}", s),
        }
    }
}

impl From<GptError> for ApplyError {
    fn from(e: GptError) -> Self {
        ApplyError::Gpt(e)
    }
}
impl From<BlockError> for ApplyError {
    fn from(e: BlockError) -> Self {
        ApplyError::Block(e)
    }
}

/// The exact sector writes that lay `plan`'s table, in issue order.
pub fn writes(plan: &Plan) -> Result<Vec<SectorWrite>, ApplyError> {
    for (i, p) in plan.parts.iter().enumerate() {
        if p.first % ALIGN != 0 {
            return Err(ApplyError::Misaligned(i));
        }
    }
    let img = plan.gpt()?;
    let names = ["protective-mbr", "primary-header", "primary-array", "backup-array", "backup-header"];
    Ok(img
        .writes()
        .iter()
        .zip(names)
        .map(|((lba, bytes), what)| SectorWrite::bytes(*lba, what, bytes.to_vec()))
        .collect())
}

/// Issue `writes` in order, then flush. Every write is bounds-checked by the medium.
pub fn commit(writes: &[SectorWrite], dev: &mut dyn Block) -> Result<(), BlockError> {
    for w in writes {
        w.issue(dev)?;
    }
    dev.flush()
}

/// What [`apply`] did.
pub struct ApplyReport {
    pub writes: Vec<SectorWrite>,
    /// False for a dry run.
    pub written: bool,
    /// The read-back verification (write mode only).
    pub verify: Option<VerifyReport>,
}

/// Lay `plan` on `dev` (see the module doc). The medium must be exactly the plan's size.
pub fn apply(plan: &Plan, dev: &mut dyn Block, mode: Mode) -> Result<ApplyReport, ApplyError> {
    if dev.sectors() != plan.disk_sectors {
        return Err(ApplyError::DiskMismatch { plan: plan.disk_sectors, disk: dev.sectors() });
    }
    let ws = writes(plan)?;
    if mode == Mode::DryRun {
        return Ok(ApplyReport { writes: ws, written: false, verify: None });
    }
    commit(&ws, dev)?;
    let rep = verify::verify(dev);
    if !rep.ok() {
        return Err(ApplyError::Verify(rep.first_failure()));
    }
    if let Err(why) = rep.matches_plan(plan) {
        return Err(ApplyError::Verify(why));
    }
    Ok(ApplyReport { writes: ws, written: true, verify: Some(rep) })
}

/// The dry run as text — the plan, every partition's identity, and every sector write. This is the
/// ONE rendering: the CLI prints it, the bus carries it, and [`canonical`] is its bytes.
pub fn render(plan: &Plan, writes: &[SectorWrite]) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "{}", plan);
    let _ = writeln!(s, "disk guid {}", gpt::guid_string(&plan.disk_guid));
    for (i, p) in plan.parts.iter().enumerate() {
        let _ = writeln!(
            s,
            "  p{} type {} guid {}",
            i + 1,
            gpt::guid_string(&p.kind.type_guid()),
            gpt::guid_string(&p.guid)
        );
    }
    let _ = writeln!(s, "writes: {} ({} sectors)", writes.len(), writes.iter().map(SectorWrite::sectors).sum::<u64>());
    for (i, w) in writes.iter().enumerate() {
        let _ = writeln!(s, "  w{} {}", i + 1, w);
    }
    s
}

/// The bytes a caller digests to sign `plan`: a version line, then [`render`]'s text.
pub fn canonical(plan: &Plan) -> Result<Vec<u8>, ApplyError> {
    let ws = writes(plan)?;
    let mut v = Vec::from(&b"amber-plan v1\n"[..]);
    v.extend_from_slice(render(plan, &ws).as_bytes());
    Ok(v)
}
