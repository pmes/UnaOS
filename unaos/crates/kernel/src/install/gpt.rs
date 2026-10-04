// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// INSTALL-CORE — the GPT writer + parse-back verifier + reader/editor, now a THIN CALLER of
// `amber_core::gpt` (SELFINSTALL2, rmbp-ledger B310: the installer ensemble's shared core, audit B296).
//
// Every byte of the table — protective MBR, headers, entry array, CRC-32s, the decoder's geometry
// bounds — is encoded and decoded by `unaos/libs/sys/amber_core`, the same `no_std` crate
// `tools/una-card` lays the x86 card image with and `amber_bytes gpt show` reads. What stays here is
// the I/O: reading and writing sectors through an `InstallTarget`, and the installer's own rules (the
// write order, the self-verify after every write, the one-field editor's scope).
//
// SELF-VERIFY is part of the write API: `write_gpt*` / `write_plan` re-read the primary + backup
// headers and the entry array straight back off the device, re-validate them through the core, and
// check the entries are the ones written before returning Ok. A write that cannot be read back and
// re-validated is a failure, not a success.
//
// THE EDITOR'S SCOPE. `set_entry_type_guid` changes a partition entry's TYPE GUID and nothing else
// (`amber_core::gpt::retype`), rewrites BOTH arrays then BOTH headers with fresh CRCs, and re-reads
// the table before returning — a half-edited GPT is worse than an unedited one.

use super::{InstallError, InstallTarget};
use amber_core::gpt::{self as core_gpt, GptError, Header};
use amber_core::plan::{PartKind, PartReq, Plan, Size};

const SECTOR: usize = core_gpt::SECTOR;
pub const GPT_ENTRIES: u32 = core_gpt::ENTRIES;
pub const GPT_ENTRY_SIZE: u32 = core_gpt::ENTRY_SIZE;
const GPT_ARRAY_SECTORS: u64 = core_gpt::ARRAY_SECTORS; // 32
pub const ESP_LBA_START: u64 = core_gpt::ALIGN; // 1 MiB alignment for the ESP.

/// The EFI System Partition type GUID, for callers that must recognise an ESP they did not write.
pub const ESP_TYPE_GUID: [u8; 16] = core_gpt::ESP_TYPE;
/// The Microsoft Basic Data type GUID.
pub const DATA_TYPE_GUID: [u8; 16] = core_gpt::BASIC_DATA_TYPE;
/// SELFINSTALL2: the (advisory) UnaFS system-volume type GUID the card's p2 and the installed SSD's p2 carry.
pub const UNAFS_TYPE_GUID: [u8; 16] = core_gpt::UNAFS_TYPE;

/// Where the ESP landed, for the FAT formatter that follows.
#[derive(Clone, Copy)]
pub struct GptLayout {
    pub esp_first_lba: u64,
    pub esp_last_lba: u64, // inclusive
    pub data_first_lba: u64,
    pub data_last_lba: u64, // inclusive (0 if no data partition fit)
    pub total_sectors: u64,
}

fn map(e: GptError) -> InstallError {
    match e {
        GptError::TooSmall => InstallError::TooSmall,
        GptError::BadIndex => InstallError::BadArg,
        _ => InstallError::VerifyFailed,
    }
}

/// The historical installer disk identity and partition names (unchanged GUIDs: the core's
/// `derive_guid` is the derivation this file always used).
pub const INSTALL_DISK_SEED: &[u8] = b"UNAOS-INSTALL-DISK";

/// Write a full GPT with an ESP and a data partition, then re-read and re-validate everything.
pub fn write_gpt<T: InstallTarget>(t: &mut T) -> Result<GptLayout, InstallError> {
    write_gpt_sized(t, 64 * 1024 * 1024 / SECTOR as u64, true)
}

/// [`write_gpt`] with the ESP size and the data partition made parameters (the historical shape:
/// ESP from LBA 2048, at least ~40 MiB of room, the data partition 1 MiB-aligned through the end).
pub fn write_gpt_sized<T: InstallTarget>(t: &mut T, esp_target: u64, with_data: bool) -> Result<GptLayout, InstallError> {
    let total_sectors = t.capacity_sectors();
    let last_usable = core_gpt::last_usable(total_sectors).ok_or(InstallError::TooSmall)?;
    if last_usable <= ESP_LBA_START {
        return Err(InstallError::TooSmall);
    }
    const ESP_MIN: u64 = 40 * 1024 * 1024 / SECTOR as u64; // comfortably above the FAT32 floor
    let usable_tail = last_usable - ESP_LBA_START + 1;
    if usable_tail < ESP_MIN + 1 {
        return Err(InstallError::TooSmall);
    }
    let esp_sectors = core::cmp::min(esp_target, if with_data { usable_tail - 1 } else { usable_tail });
    let esp = PartReq { kind: PartKind::Esp, size: Size::Sectors(esp_sectors), name: "UNAOS-ESP", seed: b"UNAOS-INSTALL-ESP" };
    let data = PartReq { kind: PartKind::Data, size: Size::Rest, name: "UNAOS-DATA", seed: b"UNAOS-INSTALL-DATA" };
    let reqs: &[PartReq<'_>] = if with_data { &[esp, data] } else { &[esp] };
    let plan = Plan::layout(total_sectors, INSTALL_DISK_SEED, reqs).map_err(|_| InstallError::TooSmall)?;
    write_plan(t, &plan)?;
    let e = plan.part(PartKind::Esp).ok_or(InstallError::VerifyFailed)?;
    let (df, dl) = plan.part(PartKind::Data).map_or((0, 0), |d| (d.first, d.last));
    Ok(GptLayout { esp_first_lba: e.first, esp_last_lba: e.last, data_first_lba: df, data_last_lba: dl, total_sectors })
}

/// SELFINSTALL2: lay the table an `amber_core::Plan` describes, then re-read and re-validate it.
/// The plan must have been made for this target's capacity.
pub fn write_plan<T: InstallTarget>(t: &mut T, plan: &Plan) -> Result<(), InstallError> {
    if plan.disk_sectors != t.capacity_sectors() {
        return Err(InstallError::BadArg);
    }
    let img = plan.gpt().map_err(map)?;
    for (lba, bytes) in img.writes() {
        t.write_sectors(lba, bytes)?;
    }
    verify_plan(t, plan)
}

/// Parse-back verification of a written plan: protective MBR, both headers (CRC, cross-linked LBAs,
/// equal array CRCs), the array off the device, and every planned entry in its slot.
pub fn verify_plan<T: InstallTarget>(t: &T, plan: &Plan) -> Result<(), InstallError> {
    let total = t.capacity_sectors();
    let mut s = [0u8; SECTOR];
    t.read_sectors(0, &mut s)?;
    core_gpt::check_protective_mbr(&s).map_err(map)?;
    t.read_sectors(1, &mut s)?;
    let p = Header::decode(&s, total).map_err(map)?;
    t.read_sectors(total - 1, &mut s)?;
    let b = Header::decode(&s, total).map_err(map)?;
    let backup_array_lba = total - 1 - GPT_ARRAY_SECTORS;
    if p.current_lba != 1 || p.backup_lba != total - 1 || b.current_lba != total - 1 || b.backup_lba != 1 {
        return Err(InstallError::VerifyFailed);
    }
    if p.entries_lba != 2 || b.entries_lba != backup_array_lba || p.entries_crc != b.entries_crc {
        return Err(InstallError::VerifyFailed);
    }
    let mut raw = alloc::vec![0u8; (GPT_ARRAY_SECTORS as usize) * SECTOR];
    t.read_sectors(2, &mut raw)?;
    let ents = core_gpt::decode_array(&raw, &p).map_err(map)?;
    let want = plan.entries();
    if ents.len() != want.len() || ents.iter().zip(want.iter()).enumerate().any(|(i, ((slot, e), w))| *slot != i as u32 || e != w) {
        return Err(InstallError::VerifyFailed);
    }
    let mut backup_raw = alloc::vec![0u8; (GPT_ARRAY_SECTORS as usize) * SECTOR];
    t.read_sectors(backup_array_lba, &mut backup_raw)?;
    if backup_raw != raw {
        return Err(InstallError::VerifyFailed);
    }
    Ok(())
}

/// Parse-back verification of the historical ESP(+data) layout (kept for its callers).
pub fn verify_gpt<T: InstallTarget>(t: &T, esp_first: u64, esp_last: u64, data_first: u64, data_last: u64) -> Result<(), InstallError> {
    let tb = read_table(t)?;
    let esp_ok = tb.entries.iter().any(|e| e.index == 0 && e.is_esp() && e.first_lba == esp_first && e.last_lba == esp_last);
    let data_ok = data_first == 0
        || tb.entries.iter().any(|e| e.index == 1 && e.type_guid == DATA_TYPE_GUID && e.first_lba == data_first && e.last_lba == data_last);
    if esp_ok && data_ok { Ok(()) } else { Err(InstallError::VerifyFailed) }
}

/// One IN-USE entry of an existing GPT, as read off the medium. `index` is the SLOT position — the
/// number the operator names and the witness prints, never "the nth non-empty entry".
#[derive(Clone, Copy)]
pub struct GptEntryView {
    pub index: u32,
    pub type_guid: [u8; 16],
    pub first_lba: u64,
    pub last_lba: u64, // inclusive
}

impl GptEntryView {
    pub fn sectors(&self) -> u64 {
        self.last_lba - self.first_lba + 1
    }
    pub fn is_esp(&self) -> bool {
        self.type_guid == ESP_TYPE_GUID
    }
    /// SELFINSTALL2: carries the (advisory) UnaFS type GUID.
    pub fn is_unafs_type(&self) -> bool {
        self.type_guid == UNAFS_TYPE_GUID
    }
    /// The first four bytes of the type GUID, the short form the census line prints.
    pub fn type_short(&self) -> u32 {
        u32::from_le_bytes([self.type_guid[0], self.type_guid[1], self.type_guid[2], self.type_guid[3]])
    }
}

/// An existing GPT, parsed and CRC-validated off a target.
pub struct GptTable {
    pub entries: alloc::vec::Vec<GptEntryView>,
    pub first_usable: u64,
    pub last_usable: u64,
    pub total_sectors: u64,
}

/// Read and VALIDATE the primary GPT through the core (signature, header CRC over its declared size,
/// geometry bounds, array CRC, every entry inside the usable range). A table that does not validate is
/// not a table: `VerifyFailed`, never entries parsed out of bytes nothing vouched for.
pub fn read_table<T: InstallTarget>(t: &T) -> Result<GptTable, InstallError> {
    let total_sectors = t.capacity_sectors();
    if total_sectors < 2 + GPT_ARRAY_SECTORS + 1 {
        return Err(InstallError::TooSmall);
    }
    let mut h = [0u8; SECTOR];
    t.read_sectors(1, &mut h)?;
    let hd = Header::decode(&h, total_sectors).map_err(|_| InstallError::VerifyFailed)?;
    let mut raw = alloc::vec![0u8; (hd.array_sectors() as usize) * SECTOR];
    t.read_sectors(hd.entries_lba, &mut raw)?;
    let ents = core_gpt::decode_array(&raw, &hd).map_err(|_| InstallError::VerifyFailed)?;
    let entries = ents
        .into_iter()
        .map(|(index, e)| GptEntryView { index, type_guid: e.type_guid, first_lba: e.first_lba, last_lba: e.last_lba })
        .collect();
    Ok(GptTable { entries, first_usable: hd.first_usable, last_usable: hd.last_usable, total_sectors })
}

/// THE ONE PARTITION-TABLE EDIT (off by default, `--as-esp`): set slot `index`'s TYPE GUID, leaving
/// every other byte as found; both arrays then both headers rewritten with recomputed CRCs (a power
/// cut leaves OLD headers over NEW arrays, which fails CRC loudly), then re-read and re-validated.
pub fn set_entry_type_guid<T: InstallTarget>(t: &mut T, index: u32, type_guid: &[u8; 16]) -> Result<(), InstallError> {
    let total_sectors = t.capacity_sectors();
    let mut h = [0u8; SECTOR];
    t.read_sectors(1, &mut h)?;
    let hd = Header::decode(&h, total_sectors).map_err(|_| InstallError::VerifyFailed)?;
    if index >= hd.num_entries {
        return Err(InstallError::BadArg);
    }
    let array_sectors = hd.array_sectors();
    let backup_header_lba = total_sectors - 1;
    let backup_array_lba = total_sectors - 1 - array_sectors;
    if hd.entries_lba + array_sectors > backup_array_lba {
        return Err(InstallError::VerifyFailed);
    }
    let mut raw = alloc::vec![0u8; (array_sectors as usize) * SECTOR];
    t.read_sectors(hd.entries_lba, &mut raw)?;
    let crc = core_gpt::retype(&mut raw, &hd, index, type_guid).map_err(map)?;
    t.write_sectors(hd.entries_lba, &raw)?;
    t.write_sectors(backup_array_lba, &raw)?;
    let primary = Header { current_lba: 1, backup_lba: backup_header_lba, entries_crc: crc, header_size: 92, ..hd };
    t.write_sectors(1, &primary.encode())?;
    let backup = Header { current_lba: backup_header_lba, backup_lba: 1, entries_lba: backup_array_lba, entries_crc: crc, header_size: 92, ..hd };
    t.write_sectors(backup_header_lba, &backup.encode())?;
    let table = read_table(t)?;
    match table.entries.iter().find(|e| e.index == index) {
        Some(e) if &e.type_guid == type_guid => Ok(()),
        _ => Err(InstallError::VerifyFailed),
    }
}
