// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The GUID Partition Table (UEFI 2.x §5.3), encode AND decode, in one place.
//!
//! Layout this crate writes (and the only one the kernel installer and `tools/una-card` lay): a
//! protective MBR at LBA 0, the primary header at LBA 1, a 128 × 128-byte entry array at LBA 2..33,
//! the backup array at `total-33` and the backup header at `total-1`, CRC-32 over the header's 92
//! bytes and over the whole array. The decoder accepts any spec-shaped table (header size 92..=512,
//! entry size ≥ 128 and a multiple of 8, ≤ 512 entries) and BOUNDS every geometry field before it is
//! used as a length or an LBA: the bytes come off a medium, and a hostile or corrupt table must not be
//! able to steer a read.

use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;

use crate::crc32::crc32;

pub const SECTOR: usize = 512;
pub const ENTRIES: u32 = 128;
pub const ENTRY_SIZE: u32 = 128;
/// Sectors one 128 × 128 entry array occupies.
pub const ARRAY_SECTORS: u64 = (ENTRIES as u64 * ENTRY_SIZE as u64) / SECTOR as u64; // 32
/// First usable LBA of the layout this crate writes (MBR + header + array).
pub const FIRST_USABLE: u64 = 2 + ARRAY_SECTORS; // 34
/// Partition alignment: 1 MiB of 512-byte sectors.
pub const ALIGN: u64 = 2048;
/// Name field capacity in UTF-16 code units (72 bytes at entry offset 56).
pub const NAME_UNITS: usize = 36;

/// EFI System Partition C12A7328-F81F-11D2-BA4B-00A0C93EC93B (on-disk mixed-endian bytes).
pub const ESP_TYPE: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];
/// Microsoft Basic Data EBD0A0A2-B9E5-4433-87C0-68B6B72699C7 — a generic data area.
pub const BASIC_DATA_TYPE: [u8; 16] = [
    0xA2, 0xA0, 0xD0, 0xEB, 0xE5, 0xB9, 0x33, 0x44, 0x87, 0xC0, 0x68, 0xB6, 0xB7, 0x26, 0x99, 0xC7,
];
/// The UnaFS system volume ("UNAFS" + a fixed tail; the UNAFSX86 card's p2). ADVISORY ONLY: the kernel
/// finds a UnaFS volume by SUPERBLOCK MAGIC (`locate_unafs`), never by this GUID — it only stops other
/// operating systems guessing at the partition.
pub const UNAFS_TYPE: [u8; 16] = [
    0x55, 0x4E, 0x41, 0x46, 0x53, 0x00, 0x00, 0x40, 0x80, 0x55, 0x4E, 0x41, 0x4F, 0x53, 0x00, 0x02,
];

/// Why a table did not encode or did not validate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GptError {
    /// The disk cannot hold the layout.
    TooSmall,
    /// LBA 0 is not a protective MBR (no 0x55AA, or partition 1 is not type 0xEE).
    BadMbr,
    /// No `EFI PART` signature.
    BadSignature,
    /// Header size outside 92..=512.
    BadHeaderSize,
    /// Header CRC mismatch.
    BadHeaderCrc,
    /// A geometry field (array LBA, entry size/count, usable range) is out of bounds.
    BadGeometry,
    /// Entry-array CRC mismatch.
    BadArrayCrc,
    /// An in-use entry's extent is inverted or escapes the usable range (carries its slot).
    EntryOutOfBounds(u32),
    /// Two entries overlap (encode side).
    Overlap,
    /// A slot index that is out of range or unused.
    BadIndex,
    /// More entries than the array holds.
    TooManyEntries,
    /// AHCIROOT: the sector source refused a read ([`read_table_with`]).
    Io,
}

impl fmt::Display for GptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GptError::TooSmall => f.write_str("disk too small for the layout"),
            GptError::BadMbr => f.write_str("LBA 0 is not a protective MBR"),
            GptError::BadSignature => f.write_str("no EFI PART signature"),
            GptError::BadHeaderSize => f.write_str("header size out of range"),
            GptError::BadHeaderCrc => f.write_str("header CRC mismatch"),
            GptError::BadGeometry => f.write_str("header geometry out of bounds"),
            GptError::BadArrayCrc => f.write_str("entry-array CRC mismatch"),
            GptError::EntryOutOfBounds(i) => write!(f, "entry {} extent out of bounds", i),
            GptError::Overlap => f.write_str("entries overlap"),
            GptError::BadIndex => f.write_str("slot index out of range or unused"),
            GptError::TooManyEntries => f.write_str("more entries than the array holds"),
            GptError::Io => f.write_str("a sector read failed"),
        }
    }
}

fn u32le(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn u64le(b: &[u8], o: usize) -> u64 {
    let mut v = [0u8; 8];
    v.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(v)
}

/// Deterministic 16-byte GUID from a label seed — a fixed identity per disk build (reproducible, not a
/// random v4), with the RFC-4122 version/variant nibbles stamped so it is well-formed. The SAME
/// derivation the kernel installer and the retired card script used, so GUIDs did not move.
pub fn derive_guid(seed: &[u8]) -> [u8; 16] {
    let mut g = [0u8; 16];
    for (i, b) in g.iter_mut().enumerate() {
        *b = seed.get(i).copied().unwrap_or_else(|| (0x11u8.wrapping_mul(i as u8 + 1)) ^ 0x5A);
    }
    g[7] = (g[7] & 0x0F) | 0x40;
    g[8] = (g[8] & 0x3F) | 0x80;
    g
}

/// Canonical text form of an on-disk (mixed-endian) GUID.
pub fn guid_string(g: &[u8; 16]) -> String {
    let mut s = String::with_capacity(36);
    let order = [3usize, 2, 1, 0, 5, 4, 7, 6, 8, 9, 10, 11, 12, 13, 14, 15];
    for (n, &i) in order.iter().enumerate() {
        if matches!(n, 4 | 6 | 8 | 10) {
            s.push('-');
        }
        let hex = b"0123456789ABCDEF";
        s.push(hex[(g[i] >> 4) as usize] as char);
        s.push(hex[(g[i] & 0xF) as usize] as char);
    }
    s
}

/// Short name for a type GUID this crate knows.
pub fn type_name(g: &[u8; 16]) -> &'static str {
    if *g == ESP_TYPE {
        "esp"
    } else if *g == UNAFS_TYPE {
        "unafs"
    } else if *g == BASIC_DATA_TYPE {
        "data"
    } else {
        "other"
    }
}

/// The protective MBR for a `total`-sector disk: one 0xEE partition from LBA 1 over the whole disk
/// (clamped to 32 bits), boot signature 0x55AA.
pub fn protective_mbr(total: u64) -> [u8; SECTOR] {
    let mut m = [0u8; SECTOR];
    let e = 446;
    m[e + 2] = 0x02; // CHS first
    m[e + 4] = 0xEE; // GPT protective
    m[e + 5] = 0xFF;
    m[e + 6] = 0xFF;
    m[e + 7] = 0xFF;
    m[e + 8..e + 12].copy_from_slice(&1u32.to_le_bytes());
    let count = core::cmp::min(total.saturating_sub(1), 0xFFFF_FFFF) as u32;
    m[e + 12..e + 16].copy_from_slice(&count.to_le_bytes());
    m[510] = 0x55;
    m[511] = 0xAA;
    m
}

/// Does LBA 0 carry a protective MBR?
pub fn check_protective_mbr(s: &[u8]) -> Result<(), GptError> {
    if s.len() < SECTOR || s[510] != 0x55 || s[511] != 0xAA || s[446 + 4] != 0xEE {
        return Err(GptError::BadMbr);
    }
    Ok(())
}

/// Last usable LBA of the layout this crate writes on a `total`-sector disk.
pub fn last_usable(total: u64) -> Option<u64> {
    total.checked_sub(ARRAY_SECTORS + 2)
}

/// One partition entry.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Entry {
    pub type_guid: [u8; 16],
    pub unique_guid: [u8; 16],
    pub first_lba: u64,
    /// Inclusive.
    pub last_lba: u64,
    pub attributes: u64,
    pub name: [u16; NAME_UNITS],
}

impl Entry {
    pub fn new(type_guid: [u8; 16], unique_guid: [u8; 16], first_lba: u64, last_lba: u64, name: &str) -> Self {
        let mut n = [0u16; NAME_UNITS];
        for (slot, ch) in n.iter_mut().zip(name.encode_utf16()) {
            *slot = ch;
        }
        Self { type_guid, unique_guid, first_lba, last_lba, attributes: 0, name: n }
    }
    pub fn sectors(&self) -> u64 {
        self.last_lba - self.first_lba + 1
    }
    pub fn is_esp(&self) -> bool {
        self.type_guid == ESP_TYPE
    }
    pub fn name_string(&self) -> String {
        let end = self.name.iter().position(|&u| u == 0).unwrap_or(NAME_UNITS);
        char::decode_utf16(self.name[..end].iter().copied())
            .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect()
    }
    /// Write this entry into a 128-byte (or larger) slot.
    pub fn encode_into(&self, e: &mut [u8]) {
        e[0..16].copy_from_slice(&self.type_guid);
        e[16..32].copy_from_slice(&self.unique_guid);
        e[32..40].copy_from_slice(&self.first_lba.to_le_bytes());
        e[40..48].copy_from_slice(&self.last_lba.to_le_bytes());
        e[48..56].copy_from_slice(&self.attributes.to_le_bytes());
        for (i, u) in self.name.iter().enumerate() {
            e[56 + i * 2..58 + i * 2].copy_from_slice(&u.to_le_bytes());
        }
    }
    /// Read an entry out of a slot (no validation; [`decode_array`] validates).
    pub fn decode(e: &[u8]) -> Self {
        let mut type_guid = [0u8; 16];
        type_guid.copy_from_slice(&e[0..16]);
        let mut unique_guid = [0u8; 16];
        unique_guid.copy_from_slice(&e[16..32]);
        let mut name = [0u16; NAME_UNITS];
        for (i, u) in name.iter_mut().enumerate() {
            *u = u16::from_le_bytes([e[56 + i * 2], e[57 + i * 2]]);
        }
        Self {
            type_guid,
            unique_guid,
            first_lba: u64le(e, 32),
            last_lba: u64le(e, 40),
            attributes: u64le(e, 48),
            name,
        }
    }
}

/// A GPT header's fields.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Header {
    pub header_size: u32,
    pub current_lba: u64,
    pub backup_lba: u64,
    pub first_usable: u64,
    pub last_usable: u64,
    pub disk_guid: [u8; 16],
    pub entries_lba: u64,
    pub num_entries: u32,
    pub entry_size: u32,
    pub entries_crc: u32,
}

impl Header {
    /// A revision-1.0, 92-byte header over the standard 128 × 128 array.
    pub fn standard(current_lba: u64, backup_lba: u64, first_usable: u64, last_usable: u64, disk_guid: [u8; 16], entries_lba: u64, entries_crc: u32) -> Self {
        Self {
            header_size: 92,
            current_lba,
            backup_lba,
            first_usable,
            last_usable,
            disk_guid,
            entries_lba,
            num_entries: ENTRIES,
            entry_size: ENTRY_SIZE,
            entries_crc,
        }
    }

    /// Bytes of the array this header names.
    pub fn array_bytes(&self) -> usize {
        self.num_entries as usize * self.entry_size as usize
    }
    /// Sectors of the array this header names.
    pub fn array_sectors(&self) -> u64 {
        (self.array_bytes() as u64).div_ceil(SECTOR as u64)
    }

    /// Encode with a freshly computed header CRC.
    pub fn encode(&self) -> [u8; SECTOR] {
        let mut h = [0u8; SECTOR];
        h[0..8].copy_from_slice(b"EFI PART");
        h[8..12].copy_from_slice(&0x0001_0000u32.to_le_bytes());
        h[12..16].copy_from_slice(&self.header_size.to_le_bytes());
        h[24..32].copy_from_slice(&self.current_lba.to_le_bytes());
        h[32..40].copy_from_slice(&self.backup_lba.to_le_bytes());
        h[40..48].copy_from_slice(&self.first_usable.to_le_bytes());
        h[48..56].copy_from_slice(&self.last_usable.to_le_bytes());
        h[56..72].copy_from_slice(&self.disk_guid);
        h[72..80].copy_from_slice(&self.entries_lba.to_le_bytes());
        h[80..84].copy_from_slice(&self.num_entries.to_le_bytes());
        h[84..88].copy_from_slice(&self.entry_size.to_le_bytes());
        h[88..92].copy_from_slice(&self.entries_crc.to_le_bytes());
        let n = (self.header_size as usize).clamp(92, SECTOR);
        let crc = crc32(&h[0..n]);
        h[16..20].copy_from_slice(&crc.to_le_bytes());
        h
    }

    /// Decode and VALIDATE a header sector against a `total_sectors` disk: signature, size, CRC over
    /// the declared size with the CRC field zeroed, and every geometry field bounded.
    pub fn decode(sector: &[u8], total_sectors: u64) -> Result<Self, GptError> {
        if sector.len() < SECTOR {
            return Err(GptError::BadHeaderSize);
        }
        if &sector[0..8] != b"EFI PART" {
            return Err(GptError::BadSignature);
        }
        let header_size = u32le(sector, 12);
        if !(92..=SECTOR as u32).contains(&header_size) {
            return Err(GptError::BadHeaderSize);
        }
        let mut h = [0u8; SECTOR];
        h.copy_from_slice(&sector[..SECTOR]);
        let stored = u32le(&h, 16);
        h[16..20].copy_from_slice(&0u32.to_le_bytes());
        if crc32(&h[0..header_size as usize]) != stored {
            return Err(GptError::BadHeaderCrc);
        }
        let mut disk_guid = [0u8; 16];
        disk_guid.copy_from_slice(&h[56..72]);
        let hd = Self {
            header_size,
            current_lba: u64le(&h, 24),
            backup_lba: u64le(&h, 32),
            first_usable: u64le(&h, 40),
            last_usable: u64le(&h, 48),
            disk_guid,
            entries_lba: u64le(&h, 72),
            num_entries: u32le(&h, 80),
            entry_size: u32le(&h, 84),
            entries_crc: u32le(&h, 88),
        };
        if hd.entry_size < 128 || hd.entry_size % 8 != 0 || hd.num_entries == 0 || hd.num_entries > 512 {
            return Err(GptError::BadGeometry);
        }
        if hd.entries_lba < 2 || hd.entries_lba + hd.array_sectors() > total_sectors {
            return Err(GptError::BadGeometry);
        }
        if hd.first_usable >= hd.last_usable || hd.last_usable >= total_sectors {
            return Err(GptError::BadGeometry);
        }
        Ok(hd)
    }
}

/// Encode `entries` into slots 0.. of a standard 32-sector array (the rest zero).
pub fn encode_array(entries: &[Entry]) -> Result<Vec<u8>, GptError> {
    if entries.len() > ENTRIES as usize {
        return Err(GptError::TooManyEntries);
    }
    let mut a = alloc::vec![0u8; (ARRAY_SECTORS as usize) * SECTOR];
    for (i, e) in entries.iter().enumerate() {
        let o = i * ENTRY_SIZE as usize;
        e.encode_into(&mut a[o..o + ENTRY_SIZE as usize]);
    }
    Ok(a)
}

/// Decode and VALIDATE an entry array read at `h.entries_lba` (`raw` covers at least
/// `h.array_bytes()`): the CRC, then every IN-USE entry's extent inside the usable range. Returns the
/// in-use entries with their SLOT index (a zero type GUID is an empty slot, UEFI 2.x §5.3.3) — slot
/// position, never "the nth non-empty entry", because those differ the moment a partition is deleted.
pub fn decode_array(raw: &[u8], h: &Header) -> Result<Vec<(u32, Entry)>, GptError> {
    let n = h.array_bytes();
    if raw.len() < n {
        return Err(GptError::BadGeometry);
    }
    if crc32(&raw[..n]) != h.entries_crc {
        return Err(GptError::BadArrayCrc);
    }
    let mut out = Vec::new();
    for i in 0..h.num_entries {
        let o = i as usize * h.entry_size as usize;
        let e = &raw[o..o + h.entry_size as usize];
        if e[0..16].iter().all(|&b| b == 0) {
            continue;
        }
        let ent = Entry::decode(e);
        if ent.last_lba < ent.first_lba || ent.first_lba < h.first_usable || ent.last_lba > h.last_usable {
            return Err(GptError::EntryOutOfBounds(i));
        }
        out.push((i, ent));
    }
    Ok(out)
}

/// Retype slot `index` of a raw array in place (TYPE GUID only — boundaries, unique GUID, name and
/// attributes are left exactly as found) and return the new array CRC. Refuses an unused slot:
/// turning an empty entry into a typed one would be CREATING a partition.
pub fn retype(raw: &mut [u8], h: &Header, index: u32, type_guid: &[u8; 16]) -> Result<u32, GptError> {
    if index >= h.num_entries || raw.len() < h.array_bytes() {
        return Err(GptError::BadIndex);
    }
    let o = index as usize * h.entry_size as usize;
    if raw[o..o + 16].iter().all(|&b| b == 0) {
        return Err(GptError::BadIndex);
    }
    raw[o..o + 16].copy_from_slice(type_guid);
    Ok(crc32(&raw[..h.array_bytes()]))
}

/// A whole table, ready to write: the five structures and where each goes.
pub struct Image {
    pub total_sectors: u64,
    pub mbr: [u8; SECTOR],
    pub primary: [u8; SECTOR],
    pub array: Vec<u8>,
    pub backup: [u8; SECTOR],
}

impl Image {
    /// Build the table for `entries` on a `total`-sector disk. Every entry must lie inside the usable
    /// range and no two may overlap.
    pub fn build(total: u64, disk_guid: [u8; 16], entries: &[Entry]) -> Result<Self, GptError> {
        let last = last_usable(total).ok_or(GptError::TooSmall)?;
        if last <= FIRST_USABLE {
            return Err(GptError::TooSmall);
        }
        for (i, e) in entries.iter().enumerate() {
            if e.last_lba < e.first_lba || e.first_lba < FIRST_USABLE || e.last_lba > last {
                return Err(GptError::EntryOutOfBounds(i as u32));
            }
            for f in &entries[..i] {
                if e.first_lba <= f.last_lba && f.first_lba <= e.last_lba {
                    return Err(GptError::Overlap);
                }
            }
        }
        let array = encode_array(entries)?;
        let acrc = crc32(&array);
        let backup_lba = total - 1;
        let backup_array = total - 1 - ARRAY_SECTORS;
        Ok(Self {
            total_sectors: total,
            mbr: protective_mbr(total),
            primary: Header::standard(1, backup_lba, FIRST_USABLE, last, disk_guid, 2, acrc).encode(),
            array,
            backup: Header::standard(backup_lba, 1, FIRST_USABLE, last, disk_guid, backup_array, acrc).encode(),
        })
    }
    pub fn backup_array_lba(&self) -> u64 {
        self.total_sectors - 1 - ARRAY_SECTORS
    }
    pub fn backup_header_lba(&self) -> u64 {
        self.total_sectors - 1
    }
    /// The writes in the order the kernel has always issued them: MBR, primary header, primary array,
    /// backup array, backup header.
    pub fn writes(&self) -> [(u64, &[u8]); 5] {
        [
            (0, &self.mbr[..]),
            (1, &self.primary[..]),
            (2, &self.array[..]),
            (self.backup_array_lba(), &self.array[..]),
            (self.backup_header_lba(), &self.backup[..]),
        ]
    }
}

/// A parsed table: the validated primary header and its in-use entries.
pub struct Table {
    pub header: Header,
    pub entries: Vec<(u32, Entry)>,
}

/// Parse a table out of a whole-disk byte slice (host images; `disk.len()` is a whole number of
/// sectors). Validates the protective MBR, the primary header and its array.
pub fn parse_image(disk: &[u8]) -> Result<Table, GptError> {
    let total = (disk.len() / SECTOR) as u64;
    if total < FIRST_USABLE + 2 {
        return Err(GptError::TooSmall);
    }
    check_protective_mbr(&disk[0..SECTOR])?;
    let header = Header::decode(&disk[SECTOR..2 * SECTOR], total)?;
    let o = header.entries_lba as usize * SECTOR;
    let entries = decode_array(&disk[o..o + header.array_sectors() as usize * SECTOR], &header)?;
    Ok(Table { header, entries })
}

// ---------------------------------------------------------------------------------------------
// AHCIROOT (rmbp-ledger B332): THE ONE GPT READER. The kernel carried six hand-rolled readers of the
// same bytes (fs/fat.rs's volume walk, drivers/ahci.rs's LBA-0 census, install/pi.rs's and the Orin
// card driver's sector-0 classifiers and scratch chooser, install/gpt.rs's table read); every one of
// them now asks these functions, so "is this a GPT" has one answer in both rings.
// ---------------------------------------------------------------------------------------------

/// Does this sector carry the `EFI PART` header signature (primary or backup header)?
pub fn has_signature(sector: &[u8]) -> bool {
    sector.len() >= 8 && &sector[0..8] == b"EFI PART"
}

/// The first of the four MBR entries whose TYPE byte is 0xEE (protective), if any. Reads the type
/// byte only — the caller decides whether the 0x55AA signature or the entry size matter to it.
pub fn protective_slot(s: &[u8]) -> Option<usize> {
    if s.len() < SECTOR {
        return None;
    }
    (0..4usize).find(|&i| s[446 + 16 * i + 4] == 0xEE)
}

/// What LBA 0 says about the disk's partitioning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sector0 {
    /// No 0x55AA, or a signature over no real partition entry (a FAT superfloppy reads here).
    None,
    /// A classic MBR: at least one entry with a non-zero type AND a non-zero size, no 0xEE.
    Mbr,
    /// A protective (or hybrid) MBR: an 0xEE entry with a non-zero size — the real table is the GPT.
    Protective,
}

/// Classify LBA 0. BOTH the type and the size of an entry are read, so a superfloppy's boot code is
/// never mistaken for a table (the AHCI census rule, now the only copy).
pub fn classify_sector0(s: &[u8]) -> Sector0 {
    if s.len() < SECTOR || s[510] != 0x55 || s[511] != 0xAA {
        return Sector0::None;
    }
    let mut real = false;
    for e in 0..4usize {
        let b = 446 + e * 16;
        let ptype = s[b + 4];
        let size = u32le(s, b + 12);
        if ptype == 0xEE && size != 0 {
            return Sector0::Protective;
        }
        if ptype != 0 && size != 0 {
            real = true;
        }
    }
    if real { Sector0::Mbr } else { Sector0::None }
}

/// Read and VALIDATE a GPT off any sector source: the header at LBA 1 (signature, CRC, bounds), then
/// its entry array (CRC, every in-use entry inside the usable range). `read(lba, buf)` fills `buf`
/// (a whole number of 512-byte sectors) or fails. The protective MBR is NOT required here: a
/// superfloppy-GPT and a hybrid both carry a valid header, and the header is what vouches for the
/// entries. A table that does not validate is not a table — no entries are guessed out of it.
pub fn read_table_with<F: FnMut(u64, &mut [u8]) -> bool>(total: u64, mut read: F) -> Result<Table, GptError> {
    if total < 3 {
        return Err(GptError::TooSmall);
    }
    let mut h = [0u8; SECTOR];
    if !read(1, &mut h) {
        return Err(GptError::Io);
    }
    let header = Header::decode(&h, total)?;
    let mut raw = alloc::vec![0u8; header.array_sectors() as usize * SECTOR];
    if !read(header.entries_lba, &mut raw) {
        return Err(GptError::Io);
    }
    let entries = decode_array(&raw, &header)?;
    Ok(Table { header, entries })
}
