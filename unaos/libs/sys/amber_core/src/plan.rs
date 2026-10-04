// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The partition PLAN — what goes where on a disk, decided before a byte is written, and printed the
//! same way by every face that shows it (the kernel's `install ssd --dry-run`, the installer window,
//! `tools/una-card`). `Plan`'s `Display` is the one rendering; callers split it on lines.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::gpt::{self, Entry, GptError, ALIGN, ARRAY_SECTORS, FIRST_USABLE};
use crate::sectors_mib;

/// What a partition is for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PartKind {
    /// The EFI System Partition (FAT32): the firmware boots it; the kernel mounts it at /boot.
    Esp,
    /// The UnaFS system volume (found by superblock magic; the GUID is advisory).
    UnaFS,
    /// A generic data area (Basic Data).
    Data,
}

impl PartKind {
    pub fn type_guid(self) -> [u8; 16] {
        match self {
            PartKind::Esp => gpt::ESP_TYPE,
            PartKind::UnaFS => gpt::UNAFS_TYPE,
            PartKind::Data => gpt::BASIC_DATA_TYPE,
        }
    }
    pub fn tag(self) -> &'static str {
        match self {
            PartKind::Esp => "esp",
            PartKind::UnaFS => "unafs",
            PartKind::Data => "data",
        }
    }
    pub fn from_type(g: &[u8; 16]) -> Option<Self> {
        if *g == gpt::ESP_TYPE {
            Some(PartKind::Esp)
        } else if *g == gpt::UNAFS_TYPE {
            Some(PartKind::UnaFS)
        } else if *g == gpt::BASIC_DATA_TYPE {
            Some(PartKind::Data)
        } else {
            None
        }
    }
}

/// How big a requested partition is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Size {
    /// Exactly this many 512-byte sectors; the plan fails `TooSmall` if they do not fit.
    Sectors(u64),
    /// Everything from the next aligned LBA through the last usable one — and NOTHING (the request is
    /// dropped) if the aligned start is already past it.
    Rest,
}

/// One requested partition.
#[derive(Clone, Copy, Debug)]
pub struct PartReq<'a> {
    pub kind: PartKind,
    pub size: Size,
    pub name: &'a str,
    /// Seed for the deterministic unique GUID ([`gpt::derive_guid`]).
    pub seed: &'a [u8],
}

/// One planned partition.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Part {
    pub kind: PartKind,
    pub first: u64,
    /// Inclusive.
    pub last: u64,
    pub guid: [u8; 16],
    pub name: String,
}

impl Part {
    pub fn sectors(&self) -> u64 {
        self.last - self.first + 1
    }
    pub fn bytes(&self) -> u64 {
        self.sectors() * gpt::SECTOR as u64
    }
    pub fn entry(&self) -> Entry {
        Entry::new(self.kind.type_guid(), self.guid, self.first, self.last, &self.name)
    }
}

impl fmt::Display for Part {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:<5} lba {}..{} {} MiB {}",
            self.kind.tag(),
            self.first,
            self.last,
            sectors_mib(self.sectors()),
            self.name
        )
    }
}

/// Why a plan could not be made.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlanError {
    TooSmall,
    Empty,
}

impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            PlanError::TooSmall => "the disk is too small for the plan",
            PlanError::Empty => "the plan has no partitions",
        })
    }
}

fn align_up(lba: u64) -> u64 {
    lba.div_ceil(ALIGN) * ALIGN
}

/// The disk-layout decision.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Plan {
    pub disk_sectors: u64,
    pub disk_guid: [u8; 16],
    pub parts: Vec<Part>,
}

impl Plan {
    /// Lay `reqs` in order on a fixed `disk_sectors` disk: each partition starts 1 MiB-aligned (the
    /// first at LBA 2048), `Sectors(n)` must fit inside the usable range, `Rest` takes what is left.
    pub fn layout(disk_sectors: u64, disk_seed: &[u8], reqs: &[PartReq<'_>]) -> Result<Self, PlanError> {
        let last_usable = gpt::last_usable(disk_sectors).ok_or(PlanError::TooSmall)?;
        if last_usable < ALIGN {
            return Err(PlanError::TooSmall);
        }
        let mut parts = Vec::new();
        let mut cursor = ALIGN;
        for r in reqs {
            let first = align_up(cursor.max(FIRST_USABLE));
            let last = match r.size {
                Size::Sectors(0) => return Err(PlanError::TooSmall),
                Size::Sectors(n) => {
                    let last = first.checked_add(n - 1).ok_or(PlanError::TooSmall)?;
                    if last > last_usable {
                        return Err(PlanError::TooSmall);
                    }
                    last
                }
                Size::Rest => {
                    if first > last_usable {
                        continue;
                    }
                    last_usable
                }
            };
            parts.push(Part { kind: r.kind, first, last, guid: gpt::derive_guid(r.seed), name: String::from(r.name) });
            cursor = last + 1;
        }
        if parts.is_empty() {
            return Err(PlanError::Empty);
        }
        Ok(Self { disk_sectors, disk_guid: gpt::derive_guid(disk_seed), parts })
    }

    /// Plan an IMAGE: every request is `Sectors(n)` and the disk is sized to fit them plus the backup
    /// table, rounded up to 1 MiB (the UNAFSX86 card's rule). `Rest` is refused here.
    pub fn for_image(disk_seed: &[u8], reqs: &[PartReq<'_>]) -> Result<Self, PlanError> {
        let mut end = ALIGN;
        for r in reqs {
            match r.size {
                Size::Sectors(n) if n > 0 => end = align_up(end) + n,
                _ => return Err(PlanError::TooSmall),
            }
        }
        if reqs.is_empty() {
            return Err(PlanError::Empty);
        }
        let total = align_up(end + ARRAY_SECTORS + 1);
        Self::layout(total, disk_seed, reqs)
    }

    /// The first partition of `kind`.
    pub fn part(&self, kind: PartKind) -> Option<&Part> {
        self.parts.iter().find(|p| p.kind == kind)
    }

    /// The GPT entries, in plan order (slot i = part i).
    pub fn entries(&self) -> Vec<Entry> {
        self.parts.iter().map(Part::entry).collect()
    }

    /// The whole table for this plan.
    pub fn gpt(&self) -> Result<gpt::Image, GptError> {
        gpt::Image::build(self.disk_sectors, self.disk_guid, &self.entries())
    }

    /// The plan's `Display`, one string per line — what the installer window paints.
    pub fn lines(&self) -> Vec<String> {
        let s = alloc::format!("{}", self);
        s.lines().map(String::from).collect()
    }
}

impl fmt::Display for Plan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "plan: {} parts on {} sectors ({} MiB)", self.parts.len(), self.disk_sectors, sectors_mib(self.disk_sectors))?;
        for (i, p) in self.parts.iter().enumerate() {
            write!(f, "\n  p{} {}", i + 1, p)?;
        }
        Ok(())
    }
}
