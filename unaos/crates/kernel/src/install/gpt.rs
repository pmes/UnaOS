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
    // AHCIROOT (B332): the core's one reader (`read_table_with`); a refused read stays `Io`.
    let tb = core_gpt::read_table_with(total_sectors, |lba, buf| t.read_sectors(lba, buf).is_ok()).map_err(|e| match e {
        GptError::Io => InstallError::Io,
        _ => InstallError::VerifyFailed,
    })?;
    let hd = tb.header;
    let entries = tb
        .entries
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

// ---------------------------------------------------------------------------------------------
// INSTALL4 (rmbp-ledger B619) — THE ADDITIVE EDITOR: new entries in EMPTY slots of an existing table,
// inside free space, beside partitions it never names. File tail: nothing above moves.
//
// `read_raw_table` reads BOTH headers and BOTH arrays and refuses any table whose two copies disagree or
// whose arrays are not where a standard table keeps them (the primary array before `first_usable`, the
// backup array after `last_usable` and before the backup header) — so the extents the editor writes are
// provably outside every partition. `add_entries` encodes the new entries into all-zero slots only,
// refuses any extent an in-use entry overlaps, writes the BACKUP array then the BACKUP header, then the
// PRIMARY array then the PRIMARY header (at every instant one header + array pair is self-consistent),
// re-reads the whole table and checks every pre-existing entry is byte-identical and every new one is
// present; on any failure after the first write it puts the original bytes back.
// ---------------------------------------------------------------------------------------------

/// An existing table read raw and cross-checked: both headers, the primary array (`raw`), the in-use
/// entries with their SLOT index.
pub struct RawTable {
    pub primary: Header,
    pub backup: Header,
    pub raw: alloc::vec::Vec<u8>,
    pub entries: alloc::vec::Vec<(u32, core_gpt::Entry)>,
    pub total_sectors: u64,
}

impl RawTable {
    /// Last LBA of the primary array (inclusive).
    pub fn primary_end(&self) -> u64 {
        self.primary.entries_lba + self.primary.array_sectors() - 1
    }
    /// The slots whose 128 bytes are ALL zero, lowest first (a zero type GUID alone is "unused" to UEFI,
    /// but a slot carrying any other byte is not one this editor overwrites).
    pub fn empty_slots(&self) -> alloc::vec::Vec<u32> {
        let es = self.primary.entry_size as usize;
        (0..self.primary.num_entries)
            .filter(|&i| {
                let o = i as usize * es;
                self.raw.get(o..o + es).is_some_and(|b| b.iter().all(|&x| x == 0))
            })
            .collect()
    }
    /// Does `[first, last]` overlap any in-use entry?
    pub fn overlaps(&self, first: u64, last: u64) -> bool {
        self.entries.iter().any(|(_, e)| first <= e.last_lba && e.first_lba <= last)
    }
    /// The extents of `[first_usable, last_usable]` no in-use entry covers (inclusive, in LBA order).
    pub fn free_extents(&self) -> alloc::vec::Vec<(u64, u64)> {
        let mut used: alloc::vec::Vec<(u64, u64)> = self.entries.iter().map(|(_, e)| (e.first_lba, e.last_lba)).collect();
        used.sort_unstable();
        let mut out = alloc::vec::Vec::new();
        let mut cursor = self.primary.first_usable;
        for (a, b) in used {
            if a > cursor {
                out.push((cursor, a - 1));
            }
            cursor = cursor.max(b.saturating_add(1));
        }
        if cursor <= self.primary.last_usable {
            out.push((cursor, self.primary.last_usable));
        }
        out
    }
}

/// INSTALL4: read and cross-check a whole existing table. `VerifyFailed` for anything this editor would
/// not write beside: a header that does not validate, the two headers not naming each other, different
/// usable ranges / disk GUID / entry geometry / array CRC, an entry size other than 128, an array that is
/// not outside the usable range, or two arrays that are not byte-equal.
pub fn read_raw_table<T: InstallTarget>(t: &T) -> Result<RawTable, InstallError> {
    let total = t.capacity_sectors();
    if total < 2 + 2 * GPT_ARRAY_SECTORS + 2 {
        return Err(InstallError::TooSmall);
    }
    let bad = |_| InstallError::VerifyFailed;
    let mut s = [0u8; SECTOR];
    t.read_sectors(1, &mut s)?;
    let p = Header::decode(&s, total).map_err(bad)?;
    if p.current_lba != 1 || p.entry_size != GPT_ENTRY_SIZE || p.backup_lba >= total || p.backup_lba <= p.last_usable {
        return Err(InstallError::VerifyFailed);
    }
    t.read_sectors(p.backup_lba, &mut s)?;
    let b = Header::decode(&s, total).map_err(bad)?;
    if b.current_lba != p.backup_lba
        || b.backup_lba != 1
        || b.first_usable != p.first_usable
        || b.last_usable != p.last_usable
        || b.disk_guid != p.disk_guid
        || b.num_entries != p.num_entries
        || b.entry_size != p.entry_size
        || b.entries_crc != p.entries_crc
    {
        return Err(InstallError::VerifyFailed);
    }
    let n = p.array_sectors();
    if p.entries_lba < 2 || p.entries_lba + n > p.first_usable || b.entries_lba <= p.last_usable || b.entries_lba + n > b.current_lba {
        return Err(InstallError::VerifyFailed);
    }
    let mut raw = alloc::vec![0u8; (n as usize) * SECTOR];
    t.read_sectors(p.entries_lba, &mut raw)?;
    let entries = core_gpt::decode_array(&raw, &p).map_err(bad)?;
    let mut braw = alloc::vec![0u8; (n as usize) * SECTOR];
    t.read_sectors(b.entries_lba, &mut braw)?;
    if braw[..p.array_bytes()] != raw[..p.array_bytes()] {
        return Err(InstallError::VerifyFailed);
    }
    Ok(RawTable { primary: p, backup: b, raw, entries, total_sectors: total })
}

/// INSTALL4: add `add` = `(slot, entry)` to the table `rt` was read from, on `t` (which should admit
/// writes to the two table extents only). See the section head for the order and the re-read.
pub fn add_entries<T: InstallTarget>(t: &mut T, rt: &RawTable, add: &[(u32, core_gpt::Entry)]) -> Result<(), InstallError> {
    let es = rt.primary.entry_size as usize;
    let empty = rt.empty_slots();
    for (i, (slot, e)) in add.iter().enumerate() {
        let inside = e.first_lba <= e.last_lba && e.first_lba >= rt.primary.first_usable && e.last_lba <= rt.primary.last_usable;
        let clash = add[..i].iter().any(|(s2, e2)| s2 == slot || (e.first_lba <= e2.last_lba && e2.first_lba <= e.last_lba));
        if !empty.contains(slot) || !inside || clash || rt.overlaps(e.first_lba, e.last_lba) || e.type_guid == [0u8; 16] {
            return Err(InstallError::BadArg);
        }
    }
    let mut raw = rt.raw.clone();
    for (slot, e) in add {
        let o = *slot as usize * es;
        e.encode_into(&mut raw[o..o + es]);
    }
    let crc = amber_core::crc32(&raw[..rt.primary.array_bytes()]);
    let primary = Header { entries_crc: crc, ..rt.primary };
    let backup = Header { entries_crc: crc, ..rt.backup };
    let wrote = (|| -> Result<(), InstallError> {
        t.write_sectors(rt.backup.entries_lba, &raw)?;
        t.write_sectors(rt.backup.current_lba, &backup.encode())?;
        t.write_sectors(rt.primary.entries_lba, &raw)?;
        t.write_sectors(1, &primary.encode())?;
        let back = read_raw_table(t)?;
        let kept = rt.entries.iter().all(|(s, e)| back.entries.iter().any(|(s2, e2)| s2 == s && e2 == e));
        let added = add.iter().all(|(s, e)| back.entries.iter().any(|(s2, e2)| s2 == s && e2 == e));
        if kept && added && back.entries.len() == rt.entries.len() + add.len() {
            Ok(())
        } else {
            Err(InstallError::VerifyFailed)
        }
    })();
    if wrote.is_err() {
        // Put the table back exactly as it was read (the original headers re-encode to their own bytes:
        // same fields, recomputed CRC over the same 92 bytes).
        let _ = t.write_sectors(rt.backup.entries_lba, &rt.raw);
        let _ = t.write_sectors(rt.backup.current_lba, &rt.backup.encode());
        let _ = t.write_sectors(rt.primary.entries_lba, &rt.raw);
        let _ = t.write_sectors(1, &rt.primary.encode());
    }
    wrote
}
