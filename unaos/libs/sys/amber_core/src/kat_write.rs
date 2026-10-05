// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Known-answer tests for the WRITE half (AMBER1, SR34): plan → disk with an exact dry run, the
//! whole-table verify, backup-header recovery, the MBR, the complete FAT32 format. No I/O, no
//! allocation beyond a [`SparseBlock`]'s map — callable in-kernel like [`crate::kat::run`].
//!
//! Two independent oracles pin bytes here:
//! * the UNAFSX86 card table (the retired `make-x86-card.py`'s CRCs, [`crate::kat`]), now reached
//!   through [`plan_apply::writes`] — the list the dry run shows and `apply` issues;
//! * the PARTINSTALL fixture `unaos/scripts/make-gpt-fixture.py` writes — a separate GPT writer in
//!   Python (`binascii.crc32`, `struct`). Its four structure CRCs below were computed from that
//!   script's own output image (`python3 unaos/scripts/make-gpt-fixture.py`, 2026-10-04); this crate
//!   rebuilding the same five partitions and landing on the same CRCs is byte identity with it.
//!   `tests/oracle.rs` repeats the comparison against the live script output byte for byte, and
//!   against util-linux `partx`/`blkid` and dosfstools `fsck.fat`, when those are installed.

use alloc::vec::Vec;

use crate::block::{Block, SparseBlock};
use crate::crc32::crc32;
use crate::gpt::{self, Entry, Image};
use crate::kat::{golden_card_plan, CARD_ARRAY_CRC, CARD_BACKUP_CRC, CARD_MBR_CRC, CARD_PRIMARY_CRC, CARD_TOTAL};
use crate::mbr::{self, Mbr, MbrKind};
use crate::plan_apply::{self, ApplyError, Mode};
use crate::recover::{self, Direction, RecoverError};
use crate::verify::{self, Level};
use crate::{fat32, fat32_format};

/// The PARTINSTALL fixture: 128 MiB.
pub const FIXTURE_TOTAL: u64 = 262_144;
pub const FIXTURE_MBR_CRC: u32 = 0x0905_6F07;
pub const FIXTURE_PRIMARY_CRC: u32 = 0x5730_706D;
pub const FIXTURE_ARRAY_CRC: u32 = 0xAD4D_DE34;
pub const FIXTURE_BACKUP_CRC: u32 = 0xDAB5_9233;

/// Apple APFS 7C3457EF-0000-11AA-AA11-00306543ECAC (on-disk bytes), as the fixture writes it.
pub const APFS_TYPE: [u8; 16] = [0xEF, 0x57, 0x34, 0x7C, 0x00, 0x00, 0xAA, 0x11, 0xAA, 0x11, 0x00, 0x30, 0x65, 0x43, 0xEC, 0xAC];

/// The fixture's five entries, rebuilt from the script's PARTS table with this crate's encoder.
pub fn fixture_entries() -> Vec<Entry> {
    let parts: [(&str, [u8; 16], u64, u64, &[u8]); 5] = [
        ("FOREIGN-FAT", gpt::BASIC_DATA_TYPE, 2048, 16384, b"UNAOS-FIXTURE-P0"),
        ("FOREIGN-APFS", APFS_TYPE, 18432, 98304, b"UNAOS-FIXTURE-P1"),
        ("UNAOS-TARGET", gpt::BASIC_DATA_TYPE, 116_736, 98304, b"UNAOS-FIXTURE-P2"),
        ("ESP-SLOT", gpt::ESP_TYPE, 215_040, 8192, b"UNAOS-FIXTURE-P3"),
        ("TINY", gpt::BASIC_DATA_TYPE, 223_232, 2048, b"UNAOS-FIXTURE-P4"),
    ];
    parts.iter().map(|(n, t, f, c, seed)| Entry::new(*t, gpt::derive_guid(seed), *f, f + c - 1, n)).collect()
}

/// The fixture's table, built by this crate.
pub fn fixture_image() -> Image {
    Image::build(FIXTURE_TOTAL, gpt::derive_guid(b"UNAOS-PARTFIXTURE"), &fixture_entries()).expect("fixture table")
}

fn hcrc(h: &[u8]) -> u32 {
    u32::from_le_bytes([h[16], h[17], h[18], h[19]])
}

/// A sparse medium holding `img`.
pub fn lay(img: &Image) -> SparseBlock {
    let mut d = SparseBlock::new(img.total_sectors);
    for (lba, b) in img.writes() {
        let _ = d.write(lba, b);
    }
    d
}

macro_rules! check {
    ($n:ident, $cond:expr, $why:expr) => {
        if !$cond {
            return Err($why);
        }
        $n += 1;
    };
}

/// Run every write-half KAT. `Ok(n)` = n checks passed; `Err(name)` = the first that failed.
pub fn run() -> Result<u32, &'static str> {
    let mut n = 0u32;

    // Streaming CRC equals the one-shot CRC.
    let mut c = crate::crc32::Crc32::new();
    c.update(b"1234").update(b"56789");
    check!(n, c.finish() == 0xCBF4_3926, "crc32: streaming check value");

    // ---- plan → disk: the card ----
    let plan = golden_card_plan();
    let ws = match plan_apply::writes(&plan) {
        Ok(w) => w,
        Err(_) => return Err("apply: card writes did not build"),
    };
    check!(n, ws.len() == 5, "apply: five writes");
    check!(n, ws.iter().map(|w| (w.lba, w.sectors())).eq([(0, 1), (1, 1), (2, 32), (CARD_TOTAL - 33, 32), (CARD_TOTAL - 1, 1)]), "apply: write LBAs and lengths");
    check!(n, ws[0].crc32() == CARD_MBR_CRC && ws[2].crc32() == CARD_ARRAY_CRC && ws[3].crc32() == CARD_ARRAY_CRC, "apply: card MBR + both arrays byte-identical");
    check!(n, hcrc(&ws[1].data) == CARD_PRIMARY_CRC && hcrc(&ws[4].data) == CARD_BACKUP_CRC, "apply: card headers byte-identical");

    let mut disk = SparseBlock::new(CARD_TOTAL);
    let dry = plan_apply::apply(&plan, &mut disk, Mode::DryRun);
    check!(n, dry.as_ref().map(|r| !r.written && r.writes == ws).unwrap_or(false), "apply: dry run returns the writes");
    check!(n, disk.map.is_empty(), "apply: dry run wrote nothing");
    let wet = plan_apply::apply(&plan, &mut disk, Mode::Write);
    check!(n, wet.as_ref().map(|r| r.written && r.verify.as_ref().map(|v| v.ok()).unwrap_or(false)).unwrap_or(false), "apply: write verifies");
    check!(n, ws.iter().all(|w| (0..w.sectors()).all(|i| disk.sector(w.lba + i)[..] == w.data[i as usize * 512..(i as usize + 1) * 512])), "apply: medium holds exactly the writes");
    check!(n, disk.map.len() == 5, "apply: nothing else written (MBR, two headers, the one non-zero sector of each array)");
    let mut small = SparseBlock::new(CARD_TOTAL - 1);
    check!(n, matches!(plan_apply::apply(&plan, &mut small, Mode::DryRun), Err(ApplyError::DiskMismatch { .. })), "apply: wrong-size medium refused");
    let mut skew = plan.clone();
    skew.parts[1].first += 1;
    check!(n, matches!(plan_apply::writes(&skew), Err(ApplyError::Misaligned(1))), "apply: misaligned plan refused");

    // Signing input: deterministic, and moves with the plan.
    let c1 = plan_apply::canonical(&plan).unwrap_or_default();
    check!(n, !c1.is_empty() && c1 == plan_apply::canonical(&golden_card_plan()).unwrap_or_default(), "canonical: deterministic");
    let mut other = plan.clone();
    other.parts[1].name = alloc::string::String::from("OTHER");
    check!(n, plan_apply::canonical(&other).unwrap_or_default() != c1, "canonical: a renamed partition changes it");

    // ---- verify ----
    let rep = verify::verify(&mut disk);
    check!(n, rep.ok() && rep.worst() == Level::Pass, "verify: card all PASS");
    check!(n, rep.matches_plan(&plan).is_ok(), "verify: card matches its plan");
    check!(n, rep.matches_plan(&other).is_err(), "verify: a different plan does not match");
    check!(n, rep.checks.len() == 12, "verify: twelve checks on a clean table");

    // ---- the Python fixture, rebuilt byte for byte ----
    let fx = fixture_image();
    check!(n, crc32(&fx.mbr) == FIXTURE_MBR_CRC, "fixture: protective MBR bytes");
    check!(n, crc32(&fx.array) == FIXTURE_ARRAY_CRC, "fixture: entry array bytes");
    check!(n, hcrc(&fx.primary) == FIXTURE_PRIMARY_CRC && hcrc(&fx.backup) == FIXTURE_BACKUP_CRC, "fixture: header bytes");
    let mut fd = lay(&fx);
    let fr = verify::verify(&mut fd);
    check!(n, fr.ok() && fr.worst() == Level::Pass && fr.entries.len() == 5, "fixture: verify all PASS, five entries");
    check!(n, fr.entries[1].1.type_guid == APFS_TYPE && fr.entries[4].1.name_string() == "TINY", "fixture: foreign type + names survive");
    let pristine = fd.clone();

    // ---- corruption, detection, recovery ----
    // (a) primary header CRC broken -> primary from backup.
    let mut d = pristine.clone();
    let mut s = d.sector(1);
    s[40] ^= 1;
    let _ = d.write(1, &s);
    let r = verify::verify(&mut d);
    check!(n, !r.ok() && r.level_of("primary-header") == Some(Level::Fail) && r.level_of("backup-header") == Some(Level::Pass), "corrupt: primary header detected");
    let rs = recover::plan_restore(&mut d, 0);
    check!(n, rs.as_ref().map(|r| r.direction == Direction::PrimaryFromBackup && r.writes.len() == 2).unwrap_or(false), "recover: primary from backup planned");
    let before = d.clone();
    check!(n, d == before, "recover: planning wrote nothing");
    if let Ok(r) = rs {
        let _ = plan_apply::commit(&r.writes, &mut d);
    }
    check!(n, d == pristine, "recover: primary restored byte-identical");

    // (b) backup array corrupted -> backup from primary.
    let mut d = pristine.clone();
    let bal = FIXTURE_TOTAL - 33;
    let mut s = d.sector(bal);
    s[0] ^= 0xFF;
    let _ = d.write(bal, &s);
    let r = verify::verify(&mut d);
    check!(n, r.level_of("backup-array") == Some(Level::Fail) && r.level_of("primary-array") == Some(Level::Pass), "corrupt: backup array detected");
    let rs = recover::plan_restore(&mut d, 0);
    check!(n, rs.as_ref().map(|r| r.direction == Direction::BackupFromPrimary).unwrap_or(false), "recover: backup from primary planned");
    if let Ok(r) = rs {
        let _ = plan_apply::commit(&r.writes, &mut d);
    }
    check!(n, d == pristine, "recover: backup restored byte-identical");

    // (c) MBR zeroed -> rewritten.
    let mut d = pristine.clone();
    let _ = d.write(0, &[0u8; 512]);
    check!(n, verify::verify(&mut d).level_of("mbr") == Some(Level::Fail), "corrupt: missing MBR detected");
    if let Ok(r) = recover::plan_restore(&mut d, 0) {
        let _ = plan_apply::commit(&r.writes, &mut d);
    }
    check!(n, d == pristine, "recover: protective MBR restored byte-identical");

    // (d) both headers gone -> refused.
    let mut d = pristine.clone();
    let _ = d.write(1, &[0u8; 512]);
    let _ = d.write(FIXTURE_TOTAL - 1, &[0u8; 512]);
    check!(n, matches!(recover::plan_restore(&mut d, 4096), Err(RecoverError::NoValidCopy)), "recover: nothing to restore from refused");

    // (e) the disk grew by 2 MiB -> the backup is relocated to the new end.
    let mut d = pristine.clone();
    d.resize(FIXTURE_TOTAL + 4096);
    let r = verify::verify(&mut d);
    check!(n, r.ok() && r.level_of("backup-location") == Some(Level::Warn), "grown: displaced backup is a warning");
    let rs = recover::plan_restore(&mut d, 0);
    check!(n, rs.as_ref().map(|r| r.direction == Direction::BackupFromPrimary && r.writes.len() == 4 && r.writes[0].what == "protective-mbr").unwrap_or(false), "grown: relocation planned (MBR resized, primary repointed, backup moved)");
    if let Ok(r) = rs {
        let _ = plan_apply::commit(&r.writes, &mut d);
    }
    let r = verify::verify(&mut d);
    check!(n, r.ok() && r.worst() == Level::Pass, "grown: relocated table all PASS");

    // (f) grown AND the primary lost -> the old backup is found by scanning.
    let mut d = pristine.clone();
    d.resize(FIXTURE_TOTAL + 4096);
    let _ = d.write(1, &[0u8; 512]);
    check!(n, recover::find_backup(&mut d, 1024).is_none(), "scan: a short scan does not reach it");
    check!(n, recover::find_backup(&mut d, 8192).map(|(l, _, _)| l) == Some(FIXTURE_TOTAL - 1), "scan: the displaced backup is found");
    let rs = recover::plan_restore(&mut d, 8192);
    check!(n, rs.as_ref().map(|r| r.direction == Direction::PrimaryFromBackup && r.source_lba == Some(FIXTURE_TOTAL - 1)).unwrap_or(false), "scan: primary restored from the displaced backup");
    if let Ok(r) = rs {
        let _ = plan_apply::commit(&r.writes, &mut d);
    }
    check!(n, (1..34).all(|l| d.sector(l) == pristine.sector(l)) && d.sector(0) == gpt::protective_mbr(FIXTURE_TOTAL + 4096), "scan: primary header + array byte-identical, MBR resized");

    // (g) overlapping entries with valid CRCs -> FAIL; (h) a misaligned start -> WARN.
    let mut ents = fixture_entries();
    ents[2].first_lba = ents[1].last_lba; // overlaps p1 by one sector
    let mut arr = gpt::encode_array(&ents).unwrap_or_default();
    let mut d = pristine.clone();
    let acrc = crc32(&arr);
    let ph = gpt::Header::standard(1, FIXTURE_TOTAL - 1, 34, FIXTURE_TOTAL - 34, gpt::derive_guid(b"UNAOS-PARTFIXTURE"), 2, acrc);
    let bh = gpt::Header::standard(FIXTURE_TOTAL - 1, 1, 34, FIXTURE_TOTAL - 34, gpt::derive_guid(b"UNAOS-PARTFIXTURE"), FIXTURE_TOTAL - 33, acrc);
    for (l, b) in [(1, &ph.encode()[..]), (2, &arr[..]), (FIXTURE_TOTAL - 33, &arr[..]), (FIXTURE_TOTAL - 1, &bh.encode()[..])] {
        let _ = d.write(l, b);
    }
    check!(n, verify::verify(&mut d).level_of("overlap") == Some(Level::Fail), "verify: overlap is a failure");
    ents = fixture_entries();
    ents[4].first_lba += 7;
    arr = gpt::encode_array(&ents).unwrap_or_default();
    let acrc = crc32(&arr);
    let ph = gpt::Header { entries_crc: acrc, ..ph };
    let bh = gpt::Header { entries_crc: acrc, ..bh };
    for (l, b) in [(1, &ph.encode()[..]), (2, &arr[..]), (FIXTURE_TOTAL - 33, &arr[..]), (FIXTURE_TOTAL - 1, &bh.encode()[..])] {
        let _ = d.write(l, b);
    }
    let r = verify::verify(&mut d);
    check!(n, r.ok() && r.level_of("alignment") == Some(Level::Warn), "verify: misalignment is a warning");

    // (i) primary and backup disagreeing (both individually valid) -> FAIL agreement.
    let mut d = pristine.clone();
    let mut bh = gpt::Header::decode(&d.sector(FIXTURE_TOTAL - 1), FIXTURE_TOTAL).map_err(|_| "agree: backup decode")?;
    bh.disk_guid[0] ^= 1;
    let _ = d.write(FIXTURE_TOTAL - 1, &bh.encode());
    check!(n, verify::verify(&mut d).level_of("agreement") == Some(Level::Fail), "verify: disagreeing copies are a failure");

    // ---- MBR ----
    let pm = Mbr::decode(&gpt::protective_mbr(CARD_TOTAL)).map_err(|_| "mbr: protective decode")?;
    check!(n, pm.kind() == MbrKind::Protective && pm.protective_issues(CARD_TOTAL).is_empty(), "mbr: our protective MBR is conformant");
    check!(n, !pm.protective_issues(CARD_TOTAL + 1).is_empty(), "mbr: a size mismatch is named");
    check!(n, Mbr::classify(&[0u8; 512]) == MbrKind::Absent, "mbr: no signature is absent");
    check!(n, mbr::chs(0) == [0, 1, 0] && mbr::chs(2048) == [32, 33, 0] && mbr::chs(16_450_559) == [254, 0xFF, 0xFF] && mbr::chs(16_450_560) == [0xFE, 0xFF, 0xFF], "mbr: CHS encoding");
    let parts = [
        mbr::part(mbr::TYPE_FAT32_LBA, 2048, 524_288, true).map_err(|_| "mbr: part")?,
        mbr::part(mbr::TYPE_LINUX, 526_336, 1_048_576, false).map_err(|_| "mbr: part")?,
    ];
    let lm = Mbr::legacy(4_194_304, 0x554E_4153, &parts).map_err(|_| "mbr: legacy refused")?;
    let lbytes = lm.encode(&[]);
    let back = Mbr::decode(&lbytes).map_err(|_| "mbr: legacy decode")?;
    check!(n, back == lm && back.kind() == MbrKind::Legacy && back.parts[0].boot == 0x80 && back.parts[1].first_lba == 526_336, "mbr: legacy round trip");
    let mut hy = lm;
    hy.parts[2] = mbr::MbrPart { os_type: mbr::TYPE_GPT_PROTECTIVE, first_lba: 1, sectors: 2047, ..Default::default() };
    check!(n, hy.kind() == MbrKind::Hybrid, "mbr: hybrid classified");
    let ov = [parts[0], mbr::part(mbr::TYPE_LINUX, 4096, 100, false).map_err(|_| "mbr: part")?];
    check!(n, Mbr::legacy(4_194_304, 0, &ov) == Err(mbr::MbrError::Overlap), "mbr: overlap refused");
    let aa = [parts[0], mbr::part(mbr::TYPE_LINUX, 526_336, 100, true).map_err(|_| "mbr: part")?];
    check!(n, Mbr::legacy(4_194_304, 0, &aa) == Err(mbr::MbrError::TwoActive), "mbr: two active refused");
    check!(n, Mbr::legacy(600_000, 0, &parts) == Err(mbr::MbrError::OutOfBounds(1)), "mbr: past the disk refused");
    check!(n, mbr::part(0x83, 1 << 32, 1, false).is_err(), "mbr: 32-bit overflow refused");

    // ---- FAT32 format ----
    let l = fat32::Layout::for_sectors(1_048_576).map_err(|_| "fat: layout")?;
    let g = fat32_format::Geometry::new(1_048_576, 1).map_err(|_| "fat: geometry")?;
    check!(n, g.fat_sz == l.fat_sz && g.data_start == l.data_start && g.count_of_clusters == l.count_of_clusters, "fat: spc=1 geometry equals the installer's Layout");
    check!(n, fat32_format::default_spc(262_144) == 1 && fat32_format::default_spc(1_048_576) == 8 && fat32_format::default_spc(40_000_000) == 32 && fat32_format::default_spc(1 << 30) == 64, "fat: cluster-size table");
    check!(n, fat32_format::Geometry::new(60_000, 1).is_err() && fat32_format::Geometry::new(1_048_576, 3).is_err(), "fat: sub-FAT32 and bad spc refused");
    let p = fat32_format::FormatParams { label: fat32_format::FormatParams::label_from("una esp"), volume_id: 0x1234_5678, sectors_per_cluster: None, hidden: 2048 };
    check!(n, &p.label == b"UNA ESP    ", "fat: label from text");
    let mut vol = SparseBlock::new(1_048_576);
    let g = fat32_format::format(&mut vol, &p).map_err(|_| "fat: format refused")?;
    check!(n, g.spc == 8 && g.fat_sz == 1023 && g.count_of_clusters == 130_812, "fat: 512 MiB geometry at 4 KiB clusters");
    let bs = vol.sector(0);
    check!(n, bs[13] == 8 && bs[36..40] == 1023u32.to_le_bytes() && bs[28..32] == 2048u32.to_le_bytes() && &bs[71..82] == b"UNA ESP    " && bs[67..71] == 0x1234_5678u32.to_le_bytes(), "fat: BPB fields");
    check!(n, vol.sector(6) == bs && vol.sector(7) == vol.sector(1), "fat: backup boot sector and FSInfo");
    let fi = vol.sector(1);
    check!(n, fi[0..4] == 0x4161_5252u32.to_le_bytes() && fi[484..488] == 0x6141_7272u32.to_le_bytes() && fi[488..492] == 130_811u32.to_le_bytes() && fi[492..496] == 3u32.to_le_bytes() && fi[510..512] == [0x55, 0xAA], "fat: FSInfo signatures, free count, next free");
    let f1 = vol.sector(32);
    let f2 = vol.sector(32 + 1023);
    check!(n, f1 == f2 && f1[0..12] == [0xF8, 0xFF, 0xFF, 0x0F, 0xFF, 0xFF, 0xFF, 0x0F, 0xFF, 0xFF, 0xFF, 0x0F] && f1[12..].iter().all(|&b| b == 0), "fat: both FATs seeded identically");
    let root = vol.sector(g.data_start as u64);
    check!(n, &root[0..11] == b"UNA ESP    " && root[11] == fat32_format::ATTR_VOLUME_ID && root[32..].iter().all(|&b| b == 0), "fat: root holds the volume label entry");
    check!(n, vol.map.len() == 7, "fat: nothing else non-zero (boot, FSInfo, their backups, two FAT heads, root)");
    let (_, fw) = fat32_format::writes(1_048_576, &p).map_err(|_| "fat: writes")?;
    check!(n, fw.len() == 6 && fw.iter().map(|w| w.sectors()).sum::<u64>() == 32 + 2 * 1023 + 8, "fat: dry-run list covers reserved + both FATs + root");

    Ok(n)
}

#[cfg(test)]
mod tests {
    #[test]
    fn kat_write_passes() {
        let n = super::run().unwrap();
        assert_eq!(n, 65, "check count moved: {n}");
    }
}
