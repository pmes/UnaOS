// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The CLONE plan — a sector copy of one partition span onto another, described before it runs: the
//! byte ranges, the count, and the time it should take at a measured rate. The kernel's
//! `install ssd --write` executes it for the UnaFS volume; the plan itself does no I/O.

use core::fmt;

use crate::gpt::SECTOR;
use crate::plan::Part;
use crate::sectors_mib;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ClonePlan {
    /// First source LBA (absolute, on the source disk).
    pub src_first: u64,
    /// First target LBA (absolute, on the target disk).
    pub dst_first: u64,
    /// Sectors to copy.
    pub sectors: u64,
}

impl ClonePlan {
    /// Copy `sectors` from `src_first` into the target partition `dst`; `None` if they do not fit.
    pub fn into_part(src_first: u64, sectors: u64, dst: &Part) -> Option<Self> {
        if sectors == 0 || sectors > dst.sectors() {
            return None;
        }
        Some(Self { src_first, dst_first: dst.first, sectors })
    }
    pub fn bytes(&self) -> u64 {
        self.sectors * SECTOR as u64
    }
    /// Source byte range `[start, end)`.
    pub fn src_bytes(&self) -> (u64, u64) {
        (self.src_first * SECTOR as u64, (self.src_first + self.sectors) * SECTOR as u64)
    }
    /// Target byte range `[start, end)`.
    pub fn dst_bytes(&self) -> (u64, u64) {
        (self.dst_first * SECTOR as u64, (self.dst_first + self.sectors) * SECTOR as u64)
    }
    /// The copy in `chunk`-sector steps: `(src_lba, dst_lba, sectors)`.
    pub fn chunks(&self, chunk: u64) -> impl Iterator<Item = (u64, u64, u64)> + '_ {
        let chunk = chunk.max(1);
        (0..self.sectors.div_ceil(chunk)).map(move |i| {
            let off = i * chunk;
            let n = core::cmp::min(chunk, self.sectors - off);
            (self.src_first + off, self.dst_first + off, n)
        })
    }
    /// Estimated milliseconds at `bytes_per_s` (read + write, so the copy moves every byte twice).
    pub fn eta_ms(&self, bytes_per_s: u64) -> Option<u64> {
        if bytes_per_s == 0 {
            return None;
        }
        Some(self.bytes().saturating_mul(2).saturating_mul(1000) / bytes_per_s)
    }
}

impl fmt::Display for ClonePlan {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "clone {} MiB ({} bytes) src lba {}.. -> dst lba {}..",
            sectors_mib(self.sectors),
            self.bytes(),
            self.src_first,
            self.dst_first
        )
    }
}
