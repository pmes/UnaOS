// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Known-answer tests, callable from BOTH rings: `cargo test -p amber_core` runs [`run`], and the
//! kernel's `tests install` fixture runs it in-kernel (no I/O, no writes).
//!
//! THE GOLDEN TABLE is the UNAFSX86 card layout (p1 ESP 128 MiB, p2 UnaFS 64 MiB) as the retired
//! Python writer `make-x86-card.py` produced it: the CRCs below were computed by THAT script's own
//! functions before it was deleted, so this crate reproducing them byte-for-byte is the proof that the
//! card image did not change when its writer moved into Rust.

use crate::crc32::crc32;
use crate::gpt::{self, Header};
use crate::plan::{PartKind, PartReq, Plan, Size};
use crate::{fat32, ClonePlan};

/// The golden card layout's requests.
pub const CARD_ESP_SECTORS: u64 = 262_144; // 128 MiB
pub const CARD_UNAFS_SECTORS: u64 = 131_072; // 64 MiB
pub const CARD_TOTAL: u64 = 397_312;
pub const CARD_MBR_CRC: u32 = 0xF2DA_C7B7;
pub const CARD_ARRAY_CRC: u32 = 0xE03A_F6F4;
pub const CARD_PRIMARY_CRC: u32 = 0x1299_2670;
pub const CARD_BACKUP_CRC: u32 = 0xE17A_721F;

/// The card plan the golden values describe (the same requests `tools/una-card` makes).
pub fn golden_card_plan() -> Plan {
    Plan::for_image(
        b"UNAOS-X86-CARD",
        &[
            PartReq { kind: PartKind::Esp, size: Size::Sectors(CARD_ESP_SECTORS), name: "UNAOS-ESP", seed: b"UNAOS-X86-ESP" },
            PartReq { kind: PartKind::UnaFS, size: Size::Sectors(CARD_UNAFS_SECTORS), name: "UNAOS-UNAFS", seed: b"UNAOS-X86-UFS" },
        ],
    )
    .expect("golden plan")
}

macro_rules! check {
    ($n:ident, $cond:expr, $why:expr) => {
        if !$cond {
            return Err($why);
        }
        $n += 1;
    };
}

/// Run every KAT. `Ok(n)` = n checks passed; `Err(name)` = the first that failed.
pub fn run() -> Result<u32, &'static str> {
    let mut n = 0u32;

    // CRC-32/ISO-HDLC check value.
    check!(n, crc32(b"123456789") == 0xCBF4_3926, "crc32 check value");

    // GOLDEN: the card plan and its table, byte-for-byte the retired Python writer's.
    let plan = golden_card_plan();
    check!(n, plan.disk_sectors == CARD_TOTAL, "golden: disk size");
    check!(n, plan.parts.len() == 2, "golden: two parts");
    check!(n, plan.parts[0].first == 2048 && plan.parts[0].last == 264_191, "golden: p1 extent");
    check!(n, plan.parts[1].first == 264_192 && plan.parts[1].last == 395_263, "golden: p2 extent");
    let img = match plan.gpt() {
        Ok(i) => i,
        Err(_) => return Err("golden: table did not build"),
    };
    check!(n, crc32(&img.mbr) == CARD_MBR_CRC, "golden: protective MBR bytes");
    check!(n, crc32(&img.array) == CARD_ARRAY_CRC, "golden: entry array bytes");
    check!(n, u32::from_le_bytes([img.primary[16], img.primary[17], img.primary[18], img.primary[19]]) == CARD_PRIMARY_CRC, "golden: primary header CRC");
    check!(n, u32::from_le_bytes([img.backup[16], img.backup[17], img.backup[18], img.backup[19]]) == CARD_BACKUP_CRC, "golden: backup header CRC");

    // Protective MBR.
    check!(n, gpt::check_protective_mbr(&img.mbr).is_ok(), "mbr: accepts the protective MBR");
    check!(n, gpt::check_protective_mbr(&[0u8; 512]) == Err(gpt::GptError::BadMbr), "mbr: refuses a zero sector");

    // Decode round trip.
    let ph = match Header::decode(&img.primary, CARD_TOTAL) {
        Ok(h) => h,
        Err(_) => return Err("decode: primary header refused"),
    };
    check!(n, ph.current_lba == 1 && ph.backup_lba == CARD_TOTAL - 1 && ph.entries_lba == 2, "decode: primary LBAs");
    check!(n, ph.first_usable == 34 && ph.last_usable == CARD_TOTAL - 34, "decode: usable range");
    let bh = match Header::decode(&img.backup, CARD_TOTAL) {
        Ok(h) => h,
        Err(_) => return Err("decode: backup header refused"),
    };
    check!(n, bh.current_lba == CARD_TOTAL - 1 && bh.backup_lba == 1 && bh.entries_lba == img.backup_array_lba(), "decode: backup LBAs");
    let ents = match gpt::decode_array(&img.array, &ph) {
        Ok(e) => e,
        Err(_) => return Err("decode: array refused"),
    };
    check!(n, ents.len() == 2 && ents[0].0 == 0 && ents[1].0 == 1, "decode: two in-use slots");
    check!(n, ents[0].1.is_esp() && ents[1].1.type_guid == gpt::UNAFS_TYPE, "decode: type GUIDs");
    check!(n, ents[0].1.name_string() == "UNAOS-ESP" && ents[1].1.name_string() == "UNAOS-UNAFS", "decode: names");
    check!(n, ents[0].1 == plan.parts[0].entry() && ents[1].1 == plan.parts[1].entry(), "decode: entries equal the plan's");

    // CRC checks bite.
    let mut bad = img.primary;
    bad[40] ^= 1;
    check!(n, Header::decode(&bad, CARD_TOTAL) == Err(gpt::GptError::BadHeaderCrc), "crc: header corruption refused");
    let mut bad_arr = img.array.clone();
    bad_arr[33] ^= 0x80;
    check!(n, gpt::decode_array(&bad_arr, &ph).err() == Some(gpt::GptError::BadArrayCrc), "crc: array corruption refused");

    // LBA bounds bite: a header naming an array past the disk, and an entry past last_usable.
    check!(n, Header::decode(&img.primary, 40) == Err(gpt::GptError::BadGeometry), "bounds: array past a short disk");
    let mut wide = plan.entries();
    wide[1].last_lba = CARD_TOTAL - 1;
    let arr = gpt::encode_array(&wide).unwrap_or_default();
    let mut h2 = ph;
    h2.entries_crc = crc32(&arr);
    check!(n, gpt::decode_array(&arr, &h2).err() == Some(gpt::GptError::EntryOutOfBounds(1)), "bounds: entry past last usable");
    check!(n, gpt::Image::build(CARD_TOTAL, plan.disk_guid, &wide).is_err(), "bounds: encode refuses it too");
    let mut overlap = plan.entries();
    overlap[1].first_lba = overlap[0].last_lba;
    check!(n, gpt::Image::build(CARD_TOTAL, plan.disk_guid, &overlap).err() == Some(gpt::GptError::Overlap), "bounds: overlap refused");

    // Retype edits one field and re-CRCs.
    let mut raw = img.array.clone();
    let crc = gpt::retype(&mut raw, &ph, 1, &gpt::BASIC_DATA_TYPE).unwrap_or(0);
    check!(n, crc == crc32(&raw) && raw[128..144] == gpt::BASIC_DATA_TYPE && raw[144..256] == img.array[144..256], "retype: one field");
    check!(n, gpt::retype(&mut raw, &ph, 5, &gpt::ESP_TYPE) == Err(gpt::GptError::BadIndex), "retype: unused slot refused");

    // Planner on a synthetic 500 GB disk: ESP 512 MiB + UnaFS 512 MiB, 1 MiB aligned.
    let disk = 976_773_168u64;
    let p = match Plan::layout(
        disk,
        b"UNAOS-INSTALL-DISK",
        &[
            PartReq { kind: PartKind::Esp, size: Size::Sectors(1_048_576), name: "UNAOS-ESP", seed: b"UNAOS-INSTALL-ESP" },
            PartReq { kind: PartKind::UnaFS, size: Size::Sectors(1_048_576), name: "UNAOS-UNAFS", seed: b"UNAOS-INSTALL-UFS" },
        ],
    ) {
        Ok(p) => p,
        Err(_) => return Err("planner: synthetic disk refused"),
    };
    check!(n, p.parts[0].first == 2048 && p.parts[0].last == 1_050_623, "planner: esp extent");
    check!(n, p.parts[1].first == 1_050_624 && p.parts[1].last == 2_099_199, "planner: unafs extent");
    check!(n, p.gpt().is_ok(), "planner: table builds");
    check!(n, Plan::layout(200_000, b"X", &[PartReq { kind: PartKind::Esp, size: Size::Sectors(1_048_576), name: "E", seed: b"E" }]).is_err(), "planner: too small refused");
    let rest = Plan::layout(disk, b"X", &[
        PartReq { kind: PartKind::Esp, size: Size::Sectors(131_072), name: "E", seed: b"E" },
        PartReq { kind: PartKind::Data, size: Size::Rest, name: "D", seed: b"D" },
    ]);
    check!(n, rest.map(|r| r.parts[1].last == disk - 34 && r.parts[1].first == 133_120).unwrap_or(false), "planner: rest takes the tail");
    check!(n, p.lines().len() == 3 && p.lines()[2].starts_with("  p2 unafs"), "planner: display lines");

    // Clone plan.
    let c = ClonePlan::into_part(264_192, CARD_UNAFS_SECTORS, &p.parts[1]);
    check!(n, c.map(|c| c.bytes() == 64 * 1024 * 1024 && c.dst_first == 1_050_624).unwrap_or(false), "clone: bytes and target");
    check!(n, c.map(|c| c.chunks(100_000).map(|(_, _, k)| k).sum::<u64>() == CARD_UNAFS_SECTORS).unwrap_or(false), "clone: chunks cover the span");
    check!(n, ClonePlan::into_part(0, p.parts[1].sectors() + 1, &p.parts[1]).is_none(), "clone: oversize refused");

    // FAT32 layout of a 512 MiB ESP.
    let l = match fat32::Layout::for_sectors(1_048_576) {
        Ok(l) => l,
        Err(_) => return Err("fat32: 512 MiB refused"),
    };
    check!(n, l.fat_sz == 8129 && l.data_start == 16_290 && l.count_of_clusters == 1_032_286, "fat32: geometry");
    let bs = l.boot_sector(2048);
    check!(n, bs[510] == 0x55 && bs[511] == 0xAA && bs[28..32] == 2048u32.to_le_bytes() && bs[36..40] == 8129u32.to_le_bytes(), "fat32: BPB fields");
    check!(n, fat32::Layout::for_sectors(60_000).is_err(), "fat32: sub-FAT32 refused");
    check!(n, fat32::blank_region_sectors(1_048_576) == Ok(16_290), "fat32: blank region");

    Ok(n)
}

#[cfg(test)]
mod tests {
    #[test]
    fn kat_passes() {
        let n = super::run().unwrap();
        assert_eq!(n, 39, "check count moved: {n}");
    }

    #[test]
    fn guid_text() {
        assert_eq!(crate::gpt::guid_string(&crate::gpt::ESP_TYPE), "C12A7328-F81F-11D2-BA4B-00A0C93EC93B");
    }

    #[test]
    fn parse_image_roundtrip() {
        let plan = super::golden_card_plan();
        let img = plan.gpt().unwrap();
        let mut disk = alloc::vec![0u8; plan.disk_sectors as usize * 512];
        for (lba, bytes) in img.writes() {
            let o = lba as usize * 512;
            disk[o..o + bytes.len()].copy_from_slice(bytes);
        }
        let t = crate::gpt::parse_image(&disk).unwrap();
        assert_eq!(t.entries.len(), 2);
    }
}
