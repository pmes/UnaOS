// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! RECOVER — the GPT keeps two copies of itself (UEFI 2.x §5.3.2: "the primary and backup GPT
//! headers"), and when one is lost the other restores it. [`plan_restore`] reads the medium through
//! [`crate::verify::verify`], decides which copy is good, and returns the EXACT sector writes that
//! rebuild the other — a dry run by construction; [`crate::plan_apply::commit`] issues them and a
//! second `verify` proves the result. Nothing is written here.
//!
//! The cases:
//! * both copies valid and in place — nothing to do (a missing protective MBR is still rewritten);
//! * primary valid, backup corrupt / missing / disagreeing — rebuild the backup array + header at the
//!   end of the medium from the primary;
//! * primary valid, backup valid but NOT at the last LBA (the disk was grown, an image extended) —
//!   relocate the backup to the last LBA and point the primary at it;
//! * primary corrupt, backup valid — rebuild the primary header + array at LBA 1/2 from the backup;
//!   when the last LBA holds no backup (a grown disk), [`find_backup`] scans backwards for one;
//! * neither valid — refused; there is nothing trustworthy to restore from.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::block::{read_vec, Block, BlockError};
use crate::gpt::{self, GptError, Header, SECTOR};
use crate::mbr::MbrKind;
use crate::plan_apply::SectorWrite;
use crate::verify::{self, Level};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Direction {
    /// Nothing to restore.
    Healthy,
    /// The backup is rebuilt (or relocated) from the primary.
    BackupFromPrimary,
    /// The primary is rebuilt from the backup.
    PrimaryFromBackup,
}

impl Direction {
    pub fn tag(self) -> &'static str {
        match self {
            Direction::Healthy => "healthy",
            Direction::BackupFromPrimary => "backup-from-primary",
            Direction::PrimaryFromBackup => "primary-from-backup",
        }
    }
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RecoverError {
    /// Neither header (nor any scanned-for backup) is valid.
    NoValidCopy,
    /// The good copy's geometry leaves no room to rebuild the other in its standard place.
    NoRoom(&'static str),
    Block(BlockError),
    Gpt(GptError),
}

impl fmt::Display for RecoverError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RecoverError::NoValidCopy => f.write_str("no valid GPT header or array to restore from"),
            RecoverError::NoRoom(w) => write!(f, "cannot rebuild: {}", w),
            RecoverError::Block(e) => write!(f, "medium: {}", e),
            RecoverError::Gpt(e) => write!(f, "table: {}", e),
        }
    }
}

impl From<BlockError> for RecoverError {
    fn from(e: BlockError) -> Self {
        RecoverError::Block(e)
    }
}

/// The restore, ready to commit.
#[derive(Clone, Debug)]
pub struct Restore {
    pub direction: Direction,
    /// Where the good copy's header was read.
    pub source_lba: Option<u64>,
    pub writes: Vec<SectorWrite>,
    pub notes: Vec<String>,
}

fn valid_backup_at(dev: &mut dyn Block, lba: u64) -> Option<(Header, Vec<u8>)> {
    let total = dev.sectors();
    let raw = read_vec(dev, lba, 1).ok()?;
    let h = Header::decode(&raw, total).ok()?;
    if h.current_lba != lba || h.backup_lba != 1 || h.entries_lba <= h.last_usable || h.entries_lba + h.array_sectors() > lba {
        return None;
    }
    let arr = read_vec(dev, h.entries_lba, h.array_sectors()).ok()?;
    gpt::decode_array(&arr, &h).ok()?;
    Some((h, arr))
}

/// Find a valid backup header: at the last LBA, else scanning backwards over at most `max_scan`
/// sectors before it (a disk grown after it was partitioned keeps its backup where the old end was).
/// Returns its LBA, the header and its array.
pub fn find_backup(dev: &mut dyn Block, max_scan: u64) -> Option<(u64, Header, Vec<u8>)> {
    let total = dev.sectors();
    if total < 2 {
        return None;
    }
    if let Some((h, a)) = valid_backup_at(dev, total - 1) {
        return Some((total - 1, h, a));
    }
    const CHUNK: u64 = 2048;
    let floor = (total - 1).saturating_sub(max_scan).max(gpt::FIRST_USABLE);
    let mut hi = total - 1; // exclusive
    while hi > floor {
        let lo = hi.saturating_sub(CHUNK).max(floor);
        let buf = read_vec(dev, lo, hi - lo).ok()?;
        for s in (lo..hi).rev() {
            let o = ((s - lo) as usize) * SECTOR;
            if &buf[o..o + 8] == b"EFI PART" {
                if let Some((h, a)) = valid_backup_at(dev, s) {
                    return Some((s, h, a));
                }
            }
        }
        hi = lo;
    }
    None
}

/// Decide and build the restore (see the module doc). `max_scan` bounds the backward search for a
/// displaced backup when the primary is lost.
pub fn plan_restore(dev: &mut dyn Block, max_scan: u64) -> Result<Restore, RecoverError> {
    let total = dev.sectors();
    let rep = verify::verify(dev);
    let mut writes = Vec::new();
    let mut notes = Vec::new();
    match rep.mbr {
        // A hybrid MBR is somebody's deliberate legacy view: reported by verify, never overwritten.
        MbrKind::Hybrid => {}
        MbrKind::Protective if rep.level_of("mbr") == Some(Level::Pass) => {}
        k => {
            writes.push(SectorWrite::bytes(0, "protective-mbr", gpt::protective_mbr(total).to_vec()));
            notes.push(format!("LBA 0 is a {} MBR not conformant for {} sectors — the protective MBR is rewritten", k.tag(), total));
        }
    }
    let primary_ok = rep.primary.is_some() && rep.primary_array.is_some();
    let backup_ok = rep.backup.is_some() && rep.backup_array.is_some();
    let agree = rep.level_of("agreement") == Some(Level::Pass);

    if let (true, Some(p), Some(parr)) = (primary_ok, rep.primary, rep.primary_array.clone()) {
        let in_place = p.backup_lba == total - 1;
        if backup_ok && agree && in_place {
            return Ok(Restore { direction: Direction::Healthy, source_lba: Some(1), writes, notes });
        }
        let barr_lba = (total - 1).checked_sub(p.array_sectors()).ok_or(RecoverError::NoRoom("medium too small"))?;
        if p.last_usable >= barr_lba {
            return Err(RecoverError::NoRoom("the usable range runs into the backup array's place"));
        }
        let mut b = p;
        b.current_lba = total - 1;
        b.backup_lba = 1;
        b.entries_lba = barr_lba;
        if !in_place {
            let mut np = p;
            np.backup_lba = total - 1;
            notes.push(format!("backup moves from LBA {} to the last LBA {}", p.backup_lba, total - 1));
            writes.push(SectorWrite::bytes(1, "primary-header", np.encode().to_vec()));
        } else {
            notes.push(String::from(if backup_ok { "backup disagrees with the primary" } else { "backup header or array is not valid" }));
        }
        writes.push(SectorWrite::bytes(barr_lba, "backup-array", parr));
        writes.push(SectorWrite::bytes(total - 1, "backup-header", b.encode().to_vec()));
        return Ok(Restore { direction: Direction::BackupFromPrimary, source_lba: Some(1), writes, notes });
    }

    // The primary is lost: restore it from a backup.
    let (blba, b, barr) = match (rep.backup, rep.backup_array.clone()) {
        (Some(b), Some(a)) if backup_ok => (b.current_lba, b, a),
        _ => find_backup(dev, max_scan).ok_or(RecoverError::NoValidCopy)?,
    };
    if 2 + b.array_sectors() > b.first_usable {
        return Err(RecoverError::NoRoom("the backup's usable range leaves no room for the primary array at LBA 2"));
    }
    let mut p = b;
    p.current_lba = 1;
    p.backup_lba = blba;
    p.entries_lba = 2;
    notes.push(format!("primary rebuilt from the backup at LBA {}", blba));
    if blba != total - 1 {
        notes.push(format!("the backup is not at the last LBA {} (grown disk?); run recover again to relocate it", total - 1));
    }
    writes.push(SectorWrite::bytes(1, "primary-header", p.encode().to_vec()));
    writes.push(SectorWrite::bytes(2, "primary-array", barr));
    Ok(Restore { direction: Direction::PrimaryFromBackup, source_lba: Some(blba), writes, notes })
}
