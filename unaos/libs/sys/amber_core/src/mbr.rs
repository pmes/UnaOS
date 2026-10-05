// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The Master Boot Record — decode, classify, and encode (UEFI 2.x §5.2.1 "Legacy MBR" and §5.2.3
//! "Protective MBR"). [`crate::gpt::protective_mbr`] remains the one writer of the protective
//! record the GPT path lays (its bytes are pinned by the golden card KAT); this module READS any
//! MBR off a medium (classifying protective / hybrid / legacy, and naming every way a protective
//! record departs from §5.2.3), and WRITES a classic four-entry table for the media that boot from
//! one (the Pi 4's firmware reads FAT from an MBR partition, never from a GPT).
//!
//! On-disk shape: 440 bytes boot code, a 4-byte disk signature at 440, 2 reserved bytes, four
//! 16-byte partition records at 446, and 0x55 0xAA at 510. A record: boot indicator, CHS start,
//! OS type, CHS end, starting LBA (u32 LE), size in LBA (u32 LE).

use alloc::vec::Vec;
use core::fmt;

use crate::gpt::SECTOR;

pub const TYPE_EMPTY: u8 = 0x00;
pub const TYPE_FAT32_LBA: u8 = 0x0C;
pub const TYPE_LINUX: u8 = 0x83;
pub const TYPE_EFI_SYSTEM: u8 = 0xEF;
/// GPT protective (§5.2.3 OSType).
pub const TYPE_GPT_PROTECTIVE: u8 = 0xEE;

/// One partition record.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct MbrPart {
    /// 0x80 = active/bootable, 0x00 = not.
    pub boot: u8,
    pub os_type: u8,
    pub first_lba: u32,
    pub sectors: u32,
    /// The raw CHS fields as found (start, end) — decode keeps them; encode recomputes them.
    pub chs_first: [u8; 3],
    pub chs_last: [u8; 3],
}

impl MbrPart {
    pub fn is_empty(&self) -> bool {
        self.os_type == TYPE_EMPTY || self.sectors == 0
    }
    /// Last LBA (inclusive), as u64 so it cannot wrap.
    pub fn last_lba(&self) -> u64 {
        (self.first_lba as u64 + self.sectors as u64).saturating_sub(1)
    }
}

/// A decoded MBR.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Mbr {
    pub disk_signature: u32,
    pub parts: [MbrPart; 4],
}

/// What LBA 0 is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MbrKind {
    /// No 0x55AA boot signature: not an MBR at all.
    Absent,
    /// A signature and four empty records.
    Empty,
    /// One 0xEE record and nothing else (GPT behind it).
    Protective,
    /// A 0xEE record beside legacy records (Apple Boot Camp style). A GPT reader trusts the GPT;
    /// a legacy OS sees the legacy records.
    Hybrid,
    /// Legacy records only (no GPT is claimed).
    Legacy,
}

impl MbrKind {
    pub fn tag(self) -> &'static str {
        match self {
            MbrKind::Absent => "absent",
            MbrKind::Empty => "empty",
            MbrKind::Protective => "protective",
            MbrKind::Hybrid => "hybrid",
            MbrKind::Legacy => "legacy",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MbrError {
    /// Shorter than one sector, or no 0x55AA.
    NoSignature,
    /// More than four partitions requested.
    TooMany,
    /// A requested partition is empty, starts at LBA 0, or runs past the disk.
    OutOfBounds(usize),
    /// Two requested partitions overlap.
    Overlap,
    /// More than one partition is marked bootable.
    TwoActive,
    /// A value does not fit the record's 32-bit fields.
    TooLarge,
}

impl fmt::Display for MbrError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MbrError::NoSignature => f.write_str("no 0x55AA boot signature"),
            MbrError::TooMany => f.write_str("an MBR holds at most four partitions"),
            MbrError::OutOfBounds(i) => write!(f, "partition {} is empty or escapes the disk", i),
            MbrError::Overlap => f.write_str("partitions overlap"),
            MbrError::TwoActive => f.write_str("more than one active partition"),
            MbrError::TooLarge => f.write_str("value does not fit a 32-bit MBR field"),
        }
    }
}

/// CHS of `lba` on the conventional 255-head, 63-sector geometry; past cylinder 1023 the
/// saturated tuple 1023/254/63 (`FE FF FF`) that every LBA-era tool writes.
pub fn chs(lba: u64) -> [u8; 3] {
    const HEADS: u64 = 255;
    const SPT: u64 = 63;
    let c = lba / (HEADS * SPT);
    if c > 1023 {
        return [0xFE, 0xFF, 0xFF];
    }
    let h = (lba / SPT) % HEADS;
    let s = lba % SPT + 1;
    [h as u8, (s as u8 & 0x3F) | (((c >> 2) as u8) & 0xC0), (c & 0xFF) as u8]
}

impl Mbr {
    /// Decode LBA 0. Refuses a sector without the 0x55AA signature.
    pub fn decode(s: &[u8]) -> Result<Self, MbrError> {
        if s.len() < SECTOR || s[510] != 0x55 || s[511] != 0xAA {
            return Err(MbrError::NoSignature);
        }
        let mut parts = [MbrPart::default(); 4];
        for (i, p) in parts.iter_mut().enumerate() {
            let e = &s[446 + i * 16..446 + (i + 1) * 16];
            p.boot = e[0];
            p.chs_first.copy_from_slice(&e[1..4]);
            p.os_type = e[4];
            p.chs_last.copy_from_slice(&e[5..8]);
            p.first_lba = u32::from_le_bytes([e[8], e[9], e[10], e[11]]);
            p.sectors = u32::from_le_bytes([e[12], e[13], e[14], e[15]]);
        }
        Ok(Self { disk_signature: u32::from_le_bytes([s[440], s[441], s[442], s[443]]), parts })
    }

    /// Classify a sector (never fails: a sector without a signature is `Absent`).
    pub fn classify(s: &[u8]) -> MbrKind {
        match Self::decode(s) {
            Err(_) => MbrKind::Absent,
            Ok(m) => m.kind(),
        }
    }

    pub fn kind(&self) -> MbrKind {
        let used: Vec<&MbrPart> = self.parts.iter().filter(|p| !p.is_empty()).collect();
        let ee = used.iter().any(|p| p.os_type == TYPE_GPT_PROTECTIVE);
        match (used.len(), ee) {
            (0, _) => MbrKind::Empty,
            (1, true) => MbrKind::Protective,
            (_, true) => MbrKind::Hybrid,
            (_, false) => MbrKind::Legacy,
        }
    }

    /// Every way this record departs from a §5.2.3 protective MBR for a `total`-sector disk (empty =
    /// conformant). A GPT reader still trusts the GPT on most of these; `verify` reports them as
    /// warnings, and a missing 0xEE record as a failure.
    pub fn protective_issues(&self, total: u64) -> Vec<&'static str> {
        let mut v = Vec::new();
        let Some((slot, p)) = self.parts.iter().enumerate().find(|(_, p)| p.os_type == TYPE_GPT_PROTECTIVE) else {
            v.push("no 0xEE record");
            return v;
        };
        if slot != 0 {
            v.push("0xEE record is not record 0");
        }
        if p.boot != 0 {
            v.push("0xEE record marked bootable");
        }
        if p.first_lba != 1 {
            v.push("0xEE record does not start at LBA 1");
        }
        let want = core::cmp::min(total.saturating_sub(1), 0xFFFF_FFFF) as u32;
        if p.sectors != want {
            v.push("0xEE record size is not the disk size - 1");
        }
        if self.parts.iter().enumerate().any(|(i, q)| i != slot && !q.is_empty()) {
            v.push("legacy records beside the 0xEE record (hybrid MBR)");
        }
        v
    }

    /// A classic table for a `total`-sector disk: at most four records, each inside `1..total`, no
    /// overlaps, at most one active. CHS fields are computed (255/63 geometry, saturated past
    /// cylinder 1023), the disk signature is the caller's.
    pub fn legacy(total: u64, disk_signature: u32, parts: &[MbrPart]) -> Result<Self, MbrError> {
        if parts.len() > 4 {
            return Err(MbrError::TooMany);
        }
        let mut out = [MbrPart::default(); 4];
        for (i, p) in parts.iter().enumerate() {
            if p.is_empty() || p.first_lba == 0 || p.last_lba() >= total {
                return Err(MbrError::OutOfBounds(i));
            }
            for q in &parts[..i] {
                if (p.first_lba as u64) <= q.last_lba() && (q.first_lba as u64) <= p.last_lba() {
                    return Err(MbrError::Overlap);
                }
            }
            out[i] = MbrPart {
                boot: if p.boot & 0x80 != 0 { 0x80 } else { 0 },
                os_type: p.os_type,
                first_lba: p.first_lba,
                sectors: p.sectors,
                chs_first: chs(p.first_lba as u64),
                chs_last: chs(p.last_lba()),
            };
        }
        if out.iter().filter(|p| p.boot == 0x80).count() > 1 {
            return Err(MbrError::TwoActive);
        }
        Ok(Self { disk_signature, parts: out })
    }

    /// Encode with `boot_code` (≤ 440 bytes; the rest of the area zero).
    pub fn encode(&self, boot_code: &[u8]) -> [u8; SECTOR] {
        let mut s = [0u8; SECTOR];
        let n = core::cmp::min(boot_code.len(), 440);
        s[..n].copy_from_slice(&boot_code[..n]);
        s[440..444].copy_from_slice(&self.disk_signature.to_le_bytes());
        for (i, p) in self.parts.iter().enumerate() {
            let e = &mut s[446 + i * 16..446 + (i + 1) * 16];
            e[0] = p.boot;
            e[1..4].copy_from_slice(&p.chs_first);
            e[4] = p.os_type;
            e[5..8].copy_from_slice(&p.chs_last);
            e[8..12].copy_from_slice(&p.first_lba.to_le_bytes());
            e[12..16].copy_from_slice(&p.sectors.to_le_bytes());
        }
        s[510] = 0x55;
        s[511] = 0xAA;
        s
    }
}

/// Convenience for a requested record (CHS is filled by [`Mbr::legacy`]).
pub fn part(os_type: u8, first_lba: u64, sectors: u64, bootable: bool) -> Result<MbrPart, MbrError> {
    let first_lba = u32::try_from(first_lba).map_err(|_| MbrError::TooLarge)?;
    let sectors = u32::try_from(sectors).map_err(|_| MbrError::TooLarge)?;
    Ok(MbrPart { boot: if bootable { 0x80 } else { 0 }, os_type, first_lba, sectors, ..MbrPart::default() })
}
