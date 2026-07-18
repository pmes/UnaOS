// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// INSTALL-CORE — the GPT writer + parse-back verifier (UEFI 2.x spec).
//
// Lays a UEFI-conformant GUID Partition Table onto an `InstallTarget`:
//   * a protective MBR at LBA 0 (single 0xEE partition spanning the disk);
//   * a primary GPT header (LBA 1) + partition entry array (LBA 2..33);
//   * a backup entry array + backup header at the tail;
//   * TWO partition entries: one EFI System Partition (ESP) and one data partition, so the platform
//     boot layout (an ESP the firmware boots + a data area) has room from the first write.
// The header/array CRC-32s are the UEFI-mandated CRC-32/ISO-HDLC (see hash::crc32). This mirrors the
// host-side hand-written GPT in builder/src/vm_image.rs — same layout constants, same GUIDs — so an
// image the kernel writes is byte-compatible with the one the builder ships, and the in-tree FAT
// reader's `scan_gpt` mounts either.
//
// SELF-VERIFY is part of the write API: `write_gpt` re-reads the primary + backup headers and the
// entry array straight back off the device, recomputes every CRC, and checks the fixed invariants
// (signatures, cross-linked backup LBA, the two partition entries) before returning Ok. A write that
// cannot be read back and re-validated is a failure, not a success.

use super::{InstallError, InstallTarget};

const SECTOR: usize = 512;
pub const GPT_ENTRIES: u32 = 128;
pub const GPT_ENTRY_SIZE: u32 = 128;
const GPT_ARRAY_SECTORS: u64 = (GPT_ENTRIES as u64 * GPT_ENTRY_SIZE as u64) / SECTOR as u64; // 32
pub const ESP_LBA_START: u64 = 2048; // 1 MiB alignment for the ESP.

// EFI System Partition type GUID C12A7328-F81F-11D2-BA4B-00A0C93EC93B (GPT mixed-endian layout).
const EFI_SYSTEM_TYPE_GUID: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];
// Microsoft Basic Data partition type GUID EBD0A0A2-B9E5-4433-87C0-68B6B72699C7 — the data area.
const BASIC_DATA_TYPE_GUID: [u8; 16] = [
    0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99, 0xC7,
];

/// Where the ESP landed, for the FAT formatter that follows.
#[derive(Clone, Copy)]
pub struct GptLayout {
    pub esp_first_lba: u64,
    pub esp_last_lba: u64, // inclusive
    pub data_first_lba: u64,
    pub data_last_lba: u64, // inclusive (0 if no data partition fit)
    pub total_sectors: u64,
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u64le(b: &[u8], o: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(v)
}

/// Deterministic 16-byte GUID from a label seed (a fixed identity per disk build — reproducible, not
/// a random v4). RFC-4122 variant/version nibbles stamped so it is well-formed.
fn derive_guid(label: &[u8]) -> [u8; 16] {
    let mut g = [0u8; 16];
    for (i, b) in g.iter_mut().enumerate() {
        *b = label
            .get(i)
            .copied()
            .unwrap_or_else(|| (0x11u8.wrapping_mul(i as u8 + 1)) ^ 0x5A);
    }
    g[7] = (g[7] & 0x0F) | 0x40; // version 4
    g[8] = (g[8] & 0x3F) | 0x80; // variant RFC 4122
    g
}

fn write_protective_mbr(mbr: &mut [u8; SECTOR], total_sectors: u64) {
    let e = 446;
    mbr[e + 2] = 0x02; // CHS first
    mbr[e + 4] = 0xEE; // type: GPT protective
    mbr[e + 5] = 0xFF; // CHS last
    mbr[e + 6] = 0xFF;
    mbr[e + 7] = 0xFF;
    mbr[e + 8..e + 12].copy_from_slice(&1u32.to_le_bytes()); // first LBA
    let count = core::cmp::min(total_sectors - 1, 0xFFFF_FFFF) as u32;
    mbr[e + 12..e + 16].copy_from_slice(&count.to_le_bytes());
    mbr[510] = 0x55;
    mbr[511] = 0xAA;
}

fn write_entry(entry: &mut [u8], type_guid: &[u8; 16], first: u64, last: u64, name: &str, seed: &[u8]) {
    entry[0..16].copy_from_slice(type_guid);
    entry[16..32].copy_from_slice(&derive_guid(seed));
    entry[32..40].copy_from_slice(&first.to_le_bytes());
    entry[40..48].copy_from_slice(&last.to_le_bytes());
    for (i, ch) in name.encode_utf16().enumerate() {
        if 56 + i * 2 + 2 > entry.len() {
            break;
        }
        entry[56 + i * 2..58 + i * 2].copy_from_slice(&ch.to_le_bytes());
    }
}

#[allow(clippy::too_many_arguments)]
fn build_header(
    current_lba: u64,
    backup_lba: u64,
    first_usable: u64,
    last_usable: u64,
    disk_guid: &[u8; 16],
    entries_start_lba: u64,
    entries_crc: u32,
) -> [u8; SECTOR] {
    let mut h = [0u8; SECTOR];
    h[0..8].copy_from_slice(b"EFI PART");
    h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes()); // revision 1.0
    h[12..16].copy_from_slice(&92u32.to_le_bytes()); // header size
    h[24..32].copy_from_slice(&current_lba.to_le_bytes());
    h[32..40].copy_from_slice(&backup_lba.to_le_bytes());
    h[40..48].copy_from_slice(&first_usable.to_le_bytes());
    h[48..56].copy_from_slice(&last_usable.to_le_bytes());
    h[56..72].copy_from_slice(disk_guid);
    h[72..80].copy_from_slice(&entries_start_lba.to_le_bytes());
    h[80..84].copy_from_slice(&GPT_ENTRIES.to_le_bytes());
    h[84..88].copy_from_slice(&GPT_ENTRY_SIZE.to_le_bytes());
    h[88..92].copy_from_slice(&entries_crc.to_le_bytes());
    let crc = super::hash::crc32(&h[0..92]);
    h[16..20].copy_from_slice(&crc.to_le_bytes());
    h
}

/// Write a full GPT (protective MBR + primary/backup headers + entry array) with an ESP and a data
/// partition, then re-read and re-validate everything. Returns the ESP/data layout on success.
pub fn write_gpt<T: InstallTarget>(t: &mut T) -> Result<GptLayout, InstallError> {
    let total_sectors = t.capacity_sectors();
    // Need room for: primary GPT (34 sectors) + ESP + backup array + backup header, with the ESP
    // large enough to format FAT32. Refuse a disk too small to hold a meaningful layout.
    let backup_reserve = GPT_ARRAY_SECTORS + 1;
    let first_usable = 2 + GPT_ARRAY_SECTORS; // LBA 34
    let last_usable = total_sectors
        .checked_sub(backup_reserve + 1)
        .ok_or(InstallError::TooSmall)?;
    if ESP_LBA_START < first_usable || last_usable <= ESP_LBA_START {
        return Err(InstallError::TooSmall);
    }

    // ESP: from ESP_LBA_START, sized to the smaller of 64 MiB or half the usable tail, but at least
    // the FAT32 floor (~34 MiB with 512-byte, 1-sector clusters). The data partition takes the rest.
    const ESP_TARGET: u64 = 64 * 1024 * 1024 / SECTOR as u64; // 131072 sectors = 64 MiB
    const ESP_MIN: u64 = 40 * 1024 * 1024 / SECTOR as u64; // comfortably above the FAT32 floor
    let usable_tail = last_usable - ESP_LBA_START + 1;
    if usable_tail < ESP_MIN + 1 {
        return Err(InstallError::TooSmall);
    }
    let esp_sectors = core::cmp::min(ESP_TARGET, usable_tail - 1); // leave >=1 sector for data
    let esp_first = ESP_LBA_START;
    let esp_last = esp_first + esp_sectors - 1;

    // Data partition: 1 MiB-aligned start after the ESP, through last_usable.
    let data_aligned = (esp_last + 1 + 2047) & !2047;
    let (data_first_out, data_last_out) = if data_aligned <= last_usable {
        (data_aligned, last_usable)
    } else {
        (0, 0)
    };

    let disk_guid = derive_guid(b"UNAOS-INSTALL-DISK");

    // Entry array (identical bytes for primary + backup).
    let mut entries = alloc::vec![0u8; (GPT_ARRAY_SECTORS * SECTOR as u64) as usize];
    write_entry(
        &mut entries[0..GPT_ENTRY_SIZE as usize],
        &EFI_SYSTEM_TYPE_GUID,
        esp_first,
        esp_last,
        "UNAOS-ESP",
        b"UNAOS-INSTALL-ESP",
    );
    if data_first_out != 0 {
        let o = GPT_ENTRY_SIZE as usize;
        write_entry(
            &mut entries[o..o + GPT_ENTRY_SIZE as usize],
            &BASIC_DATA_TYPE_GUID,
            data_first_out,
            data_last_out,
            "UNAOS-DATA",
            b"UNAOS-INSTALL-DATA",
        );
    }
    let entries_crc = super::hash::crc32(&entries);

    let backup_header_lba = total_sectors - 1;
    let backup_array_lba = total_sectors - 1 - GPT_ARRAY_SECTORS;

    // Protective MBR (LBA 0).
    let mut mbr = [0u8; SECTOR];
    write_protective_mbr(&mut mbr, total_sectors);
    t.write_sectors(0, &mbr)?;

    // Primary header (LBA 1) + array (LBA 2..).
    let primary = build_header(1, backup_header_lba, first_usable, last_usable, &disk_guid, 2, entries_crc);
    t.write_sectors(1, &primary)?;
    t.write_sectors(2, &entries)?;

    // Backup array + header (tail).
    t.write_sectors(backup_array_lba, &entries)?;
    let backup = build_header(
        backup_header_lba,
        1,
        first_usable,
        last_usable,
        &disk_guid,
        backup_array_lba,
        entries_crc,
    );
    t.write_sectors(backup_header_lba, &backup)?;

    // --- SELF-VERIFY: re-read + re-validate everything off the device ---
    verify_gpt(t, esp_first, esp_last, data_first_out, data_last_out)?;

    Ok(GptLayout {
        esp_first_lba: esp_first,
        esp_last_lba: esp_last,
        data_first_lba: data_first_out,
        data_last_lba: data_last_out,
        total_sectors,
    })
}

/// Parse-back verification: read the primary header, backup header, and entry array straight off the
/// device and re-check the UEFI invariants + every CRC. Used by `write_gpt` (and re-runnable).
pub fn verify_gpt<T: InstallTarget>(
    t: &T,
    esp_first: u64,
    esp_last: u64,
    data_first: u64,
    data_last: u64,
) -> Result<(), InstallError> {
    let total_sectors = t.capacity_sectors();
    let backup_header_lba = total_sectors - 1;
    let backup_array_lba = total_sectors - 1 - GPT_ARRAY_SECTORS;

    // Protective MBR sanity.
    let mut mbr = [0u8; SECTOR];
    t.read_sectors(0, &mut mbr)?;
    if mbr[510] != 0x55 || mbr[511] != 0xAA || mbr[446 + 4] != 0xEE {
        return Err(InstallError::VerifyFailed);
    }

    let check_header = |lba: u64, expect_backup: u64| -> Result<(u64, u32), InstallError> {
        let mut h = [0u8; SECTOR];
        t.read_sectors(lba, &mut h)?;
        if &h[0..8] != b"EFI PART" {
            return Err(InstallError::VerifyFailed);
        }
        // Header CRC-32 over 92 bytes with the CRC field zeroed.
        let stored = u32le(&h, 16);
        h[16..20].copy_from_slice(&0u32.to_le_bytes());
        if super::hash::crc32(&h[0..92]) != stored {
            return Err(InstallError::VerifyFailed);
        }
        if u64le(&h, 24) != lba || u64le(&h, 32) != expect_backup {
            return Err(InstallError::VerifyFailed);
        }
        let entries_lba = u64le(&h, 72);
        let entries_crc = u32le(&h, 88);
        Ok((entries_lba, entries_crc))
    };

    let (p_entries_lba, p_entries_crc) = check_header(1, backup_header_lba)?;
    let (b_entries_lba, b_entries_crc) = check_header(backup_header_lba, 1)?;
    if p_entries_lba != 2 || b_entries_lba != backup_array_lba || p_entries_crc != b_entries_crc {
        return Err(InstallError::VerifyFailed);
    }

    // Entry array: re-read, re-CRC, and confirm the ESP + data entries parse as written.
    let mut entries = alloc::vec![0u8; (GPT_ARRAY_SECTORS * SECTOR as u64) as usize];
    t.read_sectors(2, &mut entries)?;
    if super::hash::crc32(&entries) != p_entries_crc {
        return Err(InstallError::VerifyFailed);
    }
    // ESP entry.
    if EFI_SYSTEM_TYPE_GUID != entries[0..16]
        || u64le(&entries, 32) != esp_first
        || u64le(&entries, 40) != esp_last
    {
        return Err(InstallError::VerifyFailed);
    }
    // Data entry (if one was laid).
    if data_first != 0 {
        let o = GPT_ENTRY_SIZE as usize;
        if BASIC_DATA_TYPE_GUID != entries[o..o + 16]
            || u64le(&entries, o + 32) != data_first
            || u64le(&entries, o + 40) != data_last
        {
            return Err(InstallError::VerifyFailed);
        }
    }
    // Backup array copy matches.
    let mut backup_entries = alloc::vec![0u8; (GPT_ARRAY_SECTORS * SECTOR as u64) as usize];
    t.read_sectors(backup_array_lba, &mut backup_entries)?;
    if backup_entries != entries {
        return Err(InstallError::VerifyFailed);
    }
    Ok(())
}
