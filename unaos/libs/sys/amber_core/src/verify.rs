// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VERIFY — re-read a medium and check EVERY structure of its partition table against the spec
//! (UEFI 2.x §5.2.3 protective MBR, §5.3.2 GPT header, §5.3.3 entry array), primary AND backup, and
//! the agreement between them. Nothing is trusted that has not been bounded first: [`Header::decode`]
//! bounds the geometry against the medium before an LBA from it is used to read.
//!
//! Each check is one named line with a level: `FAIL` (the table is not valid — an installer must
//! not proceed), `WARN` (valid by the spec but irregular: a hybrid MBR, a misaligned partition, a
//! backup header that is not at the last LBA because the disk was grown), `PASS`. The report also
//! carries the decoded headers and entries so a caller (the handler's `Verify`, `Recover`, the
//! kernel's installer) acts on the same reading.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::block::{read_vec, Block};
use crate::gpt::{self, Entry, Header, ALIGN};
use crate::mbr::{Mbr, MbrKind};
use crate::plan::Plan;

#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum Level {
    Pass,
    Warn,
    Fail,
}

impl Level {
    pub fn tag(self) -> &'static str {
        match self {
            Level::Pass => "PASS",
            Level::Warn => "WARN",
            Level::Fail => "FAIL",
        }
    }
}

/// One named check.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Check {
    pub name: &'static str,
    pub level: Level,
    pub detail: String,
}

impl fmt::Display for Check {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {:<16} {}", self.level.tag(), self.name, self.detail)
    }
}

/// Everything [`verify`] read and concluded.
#[derive(Clone, Debug)]
pub struct VerifyReport {
    pub total: u64,
    pub mbr: MbrKind,
    pub primary: Option<Header>,
    pub backup: Option<Header>,
    /// The in-use entries (from the primary array when it is valid, else the backup's).
    pub entries: Vec<(u32, Entry)>,
    /// The raw primary / backup arrays as read (when their header was valid and the read succeeded).
    pub primary_array: Option<Vec<u8>>,
    pub backup_array: Option<Vec<u8>>,
    pub checks: Vec<Check>,
}

impl VerifyReport {
    fn push(&mut self, name: &'static str, level: Level, detail: String) {
        self.checks.push(Check { name, level, detail });
    }
    /// No `FAIL`.
    pub fn ok(&self) -> bool {
        self.checks.iter().all(|c| c.level != Level::Fail)
    }
    /// The worst level seen.
    pub fn worst(&self) -> Level {
        self.checks.iter().map(|c| c.level).max().unwrap_or(Level::Pass)
    }
    pub fn first_failure(&self) -> String {
        self.checks
            .iter()
            .find(|c| c.level == Level::Fail)
            .map(|c| format!("{}: {}", c.name, c.detail))
            .unwrap_or_default()
    }
    pub fn level_of(&self, name: &str) -> Option<Level> {
        self.checks.iter().filter(|c| c.name == name).map(|c| c.level).max()
    }
    /// One line per check, then a verdict line.
    pub fn lines(&self) -> Vec<String> {
        let mut v: Vec<String> = self.checks.iter().map(|c| format!("{}", c)).collect();
        let (p, w, f) = self.checks.iter().fold((0, 0, 0), |(p, w, f), c| match c.level {
            Level::Pass => (p + 1, w, f),
            Level::Warn => (p, w + 1, f),
            Level::Fail => (p, w, f + 1),
        });
        v.push(format!("verify: {} ({} pass, {} warn, {} fail)", self.worst().tag(), p, w, f));
        v
    }
    /// Is the table on the medium exactly `plan`'s? (Every entry in slots 0.., nothing else, the disk
    /// GUID, the size, the usable range this crate lays.)
    pub fn matches_plan(&self, plan: &Plan) -> Result<(), String> {
        let h = self.primary.as_ref().ok_or_else(|| String::from("no valid primary header"))?;
        if self.total != plan.disk_sectors {
            return Err(format!("disk is {} sectors, plan {}", self.total, plan.disk_sectors));
        }
        if h.disk_guid != plan.disk_guid {
            return Err(String::from("disk GUID differs from the plan's"));
        }
        if Some(h.last_usable) != gpt::last_usable(self.total) || h.first_usable != gpt::FIRST_USABLE {
            return Err(String::from("usable range differs from the plan's layout"));
        }
        let want = plan.entries();
        if self.entries.len() != want.len() {
            return Err(format!("{} entries on disk, {} in the plan", self.entries.len(), want.len()));
        }
        for (i, ((slot, got), w)) in self.entries.iter().zip(want.iter()).enumerate() {
            if *slot as usize != i || got != w {
                return Err(format!("entry p{} differs from the plan's", i + 1));
            }
        }
        Ok(())
    }
}

fn check_header_extras(raw: &[u8], h: &Header, name: &'static str, rep: &mut VerifyReport) {
    let rev = u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]);
    let reserved = u32::from_le_bytes([raw[20], raw[21], raw[22], raw[23]]);
    let tail_zero = raw[h.header_size as usize..].iter().all(|&b| b == 0);
    let mut issues = Vec::new();
    if rev != 0x0001_0000 {
        issues.push("revision is not 1.0");
    }
    if reserved != 0 {
        issues.push("reserved bytes 20..24 are not zero");
    }
    if !tail_zero {
        issues.push("bytes past the header size are not zero");
    }
    if issues.is_empty() {
        rep.push(name, Level::Pass, String::from("revision 1.0, reserved fields zero"));
    } else {
        rep.push(name, Level::Warn, issues.join("; "));
    }
}

/// Re-read `dev` and check its partition table (see the module doc).
pub fn verify(dev: &mut dyn Block) -> VerifyReport {
    let total = dev.sectors();
    let mut rep = VerifyReport {
        total,
        mbr: MbrKind::Absent,
        primary: None,
        backup: None,
        entries: Vec::new(),
        primary_array: None,
        backup_array: None,
        checks: Vec::new(),
    };
    if total < gpt::FIRST_USABLE * 2 {
        rep.push("medium", Level::Fail, format!("{} sectors is too small for a GPT", total));
        return rep;
    }
    rep.push("medium", Level::Pass, format!("{} sectors ({} MiB)", total, crate::sectors_mib(total)));

    // LBA 0: the protective MBR.
    match read_vec(dev, 0, 1) {
        Err(e) => rep.push("mbr", Level::Fail, format!("read LBA 0: {}", e)),
        Ok(s) => {
            rep.mbr = Mbr::classify(&s);
            match Mbr::decode(&s) {
                Err(e) => rep.push("mbr", Level::Fail, format!("{}", e)),
                Ok(m) => {
                    let issues = m.protective_issues(total);
                    match rep.mbr {
                        MbrKind::Protective if issues.is_empty() => {
                            rep.push("mbr", Level::Pass, String::from("protective, one 0xEE record over the disk"))
                        }
                        MbrKind::Protective | MbrKind::Hybrid => {
                            rep.push("mbr", Level::Warn, format!("{}: {}", rep.mbr.tag(), issues.join("; ")))
                        }
                        k => rep.push("mbr", Level::Fail, format!("{} MBR, no GPT protective record", k.tag())),
                    }
                }
            }
        }
    }

    // LBA 1: the primary header, and its array.
    match read_vec(dev, 1, 1).map_err(|e| format!("read LBA 1: {}", e)).and_then(|raw| {
        Header::decode(&raw, total).map(|h| (raw, h)).map_err(|e| format!("{}", e))
    }) {
        Err(e) => rep.push("primary-header", Level::Fail, e),
        Ok((raw, h)) => {
            if h.current_lba != 1 {
                rep.push("primary-header", Level::Fail, format!("MyLBA is {}, not 1", h.current_lba));
            } else {
                rep.push(
                    "primary-header",
                    Level::Pass,
                    format!("signature, size {}, CRC, usable {}..{}", h.header_size, h.first_usable, h.last_usable),
                );
                check_header_extras(&raw, &h, "primary-fields", &mut rep);
                rep.primary = Some(h);
            }
        }
    }
    if let Some(h) = rep.primary {
        if h.entries_lba + h.array_sectors() > h.first_usable {
            rep.push("primary-array", Level::Fail, String::from("array overlaps the usable range"));
        } else {
            match read_vec(dev, h.entries_lba, h.array_sectors()) {
                Err(e) => rep.push("primary-array", Level::Fail, format!("read: {}", e)),
                Ok(raw) => match gpt::decode_array(&raw, &h) {
                    Err(e) => rep.push("primary-array", Level::Fail, format!("{}", e)),
                    Ok(ents) => {
                        rep.push(
                            "primary-array",
                            Level::Pass,
                            format!("lba {} x{}, CRC, {} in use, extents in range", h.entries_lba, h.array_sectors(), ents.len()),
                        );
                        rep.entries = ents;
                        rep.primary_array = Some(raw);
                    }
                },
            }
        }
    }

    // The backup header: where the primary says, else the last LBA.
    let blba = rep.primary.map(|h| h.backup_lba).unwrap_or(total - 1);
    if blba != total - 1 {
        rep.push(
            "backup-location",
            if blba < total { Level::Warn } else { Level::Fail },
            format!("primary names LBA {} as the backup, the last LBA is {}", blba, total - 1),
        );
    }
    if blba < total {
        match read_vec(dev, blba, 1).map_err(|e| format!("read LBA {}: {}", blba, e)).and_then(|raw| {
            Header::decode(&raw, total).map(|h| (raw, h)).map_err(|e| format!("{}", e))
        }) {
            Err(e) => rep.push("backup-header", Level::Fail, e),
            Ok((raw, h)) => {
                if h.current_lba != blba || h.backup_lba != 1 {
                    rep.push(
                        "backup-header",
                        Level::Fail,
                        format!("MyLBA {} / AlternateLBA {} (want {} / 1)", h.current_lba, h.backup_lba, blba),
                    );
                } else {
                    rep.push("backup-header", Level::Pass, format!("at LBA {}, signature, CRC, AlternateLBA 1", blba));
                    check_header_extras(&raw, &h, "backup-fields", &mut rep);
                    rep.backup = Some(h);
                }
            }
        }
    }
    if let Some(h) = rep.backup {
        if h.entries_lba <= h.last_usable || h.entries_lba + h.array_sectors() > h.current_lba {
            rep.push("backup-array", Level::Fail, String::from("array not between the usable range and the backup header"));
        } else {
            match read_vec(dev, h.entries_lba, h.array_sectors()) {
                Err(e) => rep.push("backup-array", Level::Fail, format!("read: {}", e)),
                Ok(raw) => match gpt::decode_array(&raw, &h) {
                    Err(e) => rep.push("backup-array", Level::Fail, format!("{}", e)),
                    Ok(ents) => {
                        rep.push("backup-array", Level::Pass, format!("lba {} x{}, CRC, {} in use", h.entries_lba, h.array_sectors(), ents.len()));
                        if rep.primary_array.is_none() {
                            rep.entries = ents;
                        }
                        rep.backup_array = Some(raw);
                    }
                },
            }
        }
    }

    // Agreement.
    if let (Some(p), Some(b)) = (rep.primary, rep.backup) {
        let mut diffs = Vec::new();
        if p.disk_guid != b.disk_guid {
            diffs.push("disk GUID");
        }
        if p.first_usable != b.first_usable || p.last_usable != b.last_usable {
            diffs.push("usable range");
        }
        if p.num_entries != b.num_entries || p.entry_size != b.entry_size {
            diffs.push("array geometry");
        }
        if p.entries_crc != b.entries_crc {
            diffs.push("array CRC");
        }
        if let (Some(pa), Some(ba)) = (&rep.primary_array, &rep.backup_array) {
            if pa != ba {
                diffs.push("array bytes");
            }
        }
        if diffs.is_empty() {
            rep.push("agreement", Level::Pass, String::from("primary and backup agree (GUID, range, geometry, array)"));
        } else {
            rep.push("agreement", Level::Fail, format!("primary and backup differ: {}", diffs.join(", ")));
        }
    }

    // Entries: overlaps, identities, alignment.
    if rep.primary.is_some() || rep.backup.is_some() {
        let ents = rep.entries.clone();
        let mut overlap = None;
        for (i, (si, a)) in ents.iter().enumerate() {
            for (sj, b) in &ents[..i] {
                if a.first_lba <= b.last_lba && b.first_lba <= a.last_lba {
                    overlap = Some((*sj, *si));
                }
            }
        }
        match overlap {
            Some((a, b)) => rep.push("overlap", Level::Fail, format!("slots {} and {} overlap", a, b)),
            None => rep.push("overlap", Level::Pass, format!("{} extents, none overlap", ents.len())),
        }
        let zero_guid = ents.iter().any(|(_, e)| e.unique_guid == [0u8; 16]);
        let dup_guid = ents.iter().enumerate().any(|(i, (_, a))| ents[..i].iter().any(|(_, b)| b.unique_guid == a.unique_guid));
        if zero_guid || dup_guid {
            rep.push("identity", Level::Warn, String::from("a unique partition GUID is zero or repeated"));
        } else {
            rep.push("identity", Level::Pass, String::from("unique partition GUIDs distinct and non-zero"));
        }
        let mis: Vec<String> = ents.iter().filter(|(_, e)| e.first_lba % ALIGN != 0).map(|(s, _)| format!("slot {}", s)).collect();
        if mis.is_empty() {
            rep.push("alignment", Level::Pass, String::from("every partition starts 1 MiB aligned"));
        } else {
            rep.push("alignment", Level::Warn, format!("not 1 MiB aligned: {}", mis.join(", ")));
        }
    }
    rep
}
