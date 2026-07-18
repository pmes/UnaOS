// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// INSTALL-CORE — the FAT32 formatter + extent-recording payload writer (Microsoft FAT spec).
//
// Formats the ESP partition an `InstallTarget` carries (a GPT ESP laid by `gpt::write_gpt`) as FAT32,
// then writes a payload file into it, RECORDING the exact byte extents written so the copy-and-verify
// primitive can re-read precisely what it wrote and SHA-check it.
//
// BLANK-PRECONDITION optimization (armed-scratch discipline): the engine only ever runs against a
// blank, armed target (see `mod::blank_check`), so the whole FAT + data region is already zeroed. The
// formatter therefore writes ONLY the defining structures — boot sector, FSInfo, backup copies, and
// the reserved FAT entries — and leaves the guaranteed-zero remainder untouched (an empty FAT is all
// free entries = 0, an empty root cluster is all zero). A general-purpose formatter on unknown media
// would zero the FAT region explicitly; here the blank contract makes it unnecessary and keeps the
// write count to a handful of sectors. The produced volume is a full, valid FAT32 the in-tree reader
// (`fs::fat::parse_bpb` / `scan_gpt`) mounts — that mount is the formatter's interop self-check.

use super::{InstallError, InstallTarget};
use alloc::vec::Vec;

const SECTOR: usize = 512;
const RESERVED: u32 = 32;
const NUM_FATS: u32 = 2;
const SPC: u32 = 1; // sectors per cluster (1 => extent == sector; simplest deterministic layout)
const VOL_ID: u32 = 0x554E_4153; // "UNAS"
const ROOT_CLUSTER: u32 = 2;
const FIRST_FILE_CLUSTER: u32 = 3; // cluster 2 is the root directory

/// Geometry of the formatted ESP, all in VOLUME-RELATIVE sectors (add `esp_first` for absolute LBA).
#[derive(Clone, Copy)]
pub struct FatGeom {
    pub esp_first: u64,   // absolute LBA of the ESP / BPB
    pub fat_sz: u32,      // sectors per FAT copy
    pub fat_start: u32,   // volume-relative: RESERVED
    pub data_start: u32,  // volume-relative first data sector (cluster 2)
    pub count_of_clusters: u32,
}

impl FatGeom {
    /// Absolute LBA of a volume-relative sector.
    fn abs(&self, vol_sector: u32) -> u64 {
        self.esp_first + vol_sector as u64
    }
    /// Absolute LBA of the first sector of `cluster`.
    fn cluster_lba(&self, cluster: u32) -> u64 {
        self.abs(self.data_start + (cluster - 2) * SPC)
    }
}

/// A written byte range on the device: `len` bytes starting at absolute `lba`.
#[derive(Clone, Copy)]
pub struct Extent {
    pub lba: u64,
    pub len: usize,
}

/// Standard Microsoft FAT32 FAT-size computation (fatgen §"Determining FAT type").
fn compute_fat_sz(tot_sec: u32) -> u32 {
    let tmpval1 = tot_sec - RESERVED; // root_dir_sectors == 0 on FAT32
    let tmpval2 = (256 * SPC + NUM_FATS) / 2;
    (tmpval1 + (tmpval2 - 1)) / tmpval2
}

/// Format the ESP `[esp_first .. esp_first+esp_sectors)` as FAT32. Returns the geometry the payload
/// writer + verifier use. `esp_sectors` must be large enough for a FAT32 volume (>= 65525 clusters);
/// the GPT writer sizes the ESP so this always holds.
pub fn format_esp<T: InstallTarget>(
    t: &mut T,
    esp_first: u64,
    esp_sectors: u64,
) -> Result<FatGeom, InstallError> {
    if esp_sectors > u32::MAX as u64 {
        return Err(InstallError::TooSmall);
    }
    let tot_sec = esp_sectors as u32;
    let fat_sz = compute_fat_sz(tot_sec);
    let fat_region = NUM_FATS * fat_sz;
    let data_start = RESERVED + fat_region;
    if data_start >= tot_sec {
        return Err(InstallError::TooSmall);
    }
    let count_of_clusters = (tot_sec - data_start) / SPC;
    if count_of_clusters < 65525 || count_of_clusters > 0x0FFF_FFF4 {
        return Err(InstallError::TooSmall); // not a valid FAT32 cluster count
    }

    // --- Boot sector (BPB) ---
    let mut bs = [0u8; SECTOR];
    bs[0] = 0xEB;
    bs[1] = 0x58;
    bs[2] = 0x90;
    bs[3..11].copy_from_slice(b"UNAOS   "); // OEM name (8)
    bs[11..13].copy_from_slice(&(SECTOR as u16).to_le_bytes());
    bs[13] = SPC as u8;
    bs[14..16].copy_from_slice(&(RESERVED as u16).to_le_bytes());
    bs[16] = NUM_FATS as u8;
    // root_ent_cnt(17..19)=0, tot_sec16(19..21)=0, fat_sz16(22..24)=0 — all FAT32
    bs[21] = 0xF8; // media
    bs[24..26].copy_from_slice(&63u16.to_le_bytes()); // sectors per track
    bs[26..28].copy_from_slice(&255u16.to_le_bytes()); // number of heads
    bs[28..32].copy_from_slice(&(esp_first as u32).to_le_bytes()); // hidden sectors (part LBA)
    bs[32..36].copy_from_slice(&tot_sec.to_le_bytes());
    bs[36..40].copy_from_slice(&fat_sz.to_le_bytes());
    // ext_flags(40..42)=0, fs_ver(42..44)=0
    bs[44..48].copy_from_slice(&ROOT_CLUSTER.to_le_bytes());
    bs[48..50].copy_from_slice(&1u16.to_le_bytes()); // FSInfo sector
    bs[50..52].copy_from_slice(&6u16.to_le_bytes()); // backup boot sector
    bs[64] = 0x80; // drive number
    bs[66] = 0x29; // extended boot signature
    bs[67..71].copy_from_slice(&VOL_ID.to_le_bytes());
    bs[71..82].copy_from_slice(b"UNAOS      "); // volume label (11)
    bs[82..90].copy_from_slice(b"FAT32   "); // FS type (8)
    bs[510] = 0x55;
    bs[511] = 0xAA;

    // --- FSInfo ---
    let mut fsi = [0u8; SECTOR];
    fsi[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes()); // lead signature "RRaA"
    fsi[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes()); // struct signature "rrAa"
    fsi[488..492].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // free count: unknown
    fsi[492..496].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // next free: unknown
    fsi[510] = 0x55;
    fsi[511] = 0xAA;

    // --- reserved FAT entries (first FAT sector of each copy) ---
    let mut fat0 = [0u8; SECTOR];
    fat0[0..4].copy_from_slice(&0x0FFF_FFF8u32.to_le_bytes()); // entry 0: media | high bits
    fat0[4..8].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes()); // entry 1: EOC
    fat0[8..12].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes()); // entry 2 (root): EOC (1 cluster)

    let geom = FatGeom {
        esp_first,
        fat_sz,
        fat_start: RESERVED,
        data_start,
        count_of_clusters,
    };

    // Write the defining structures (blank-precondition: the rest is already zero).
    t.write_sectors(geom.abs(0), &bs)?;
    t.write_sectors(geom.abs(1), &fsi)?;
    t.write_sectors(geom.abs(6), &bs)?; // backup boot sector
    t.write_sectors(geom.abs(7), &fsi)?; // backup FSInfo
    t.write_sectors(geom.abs(RESERVED), &fat0)?; // FAT copy 0, sector 0
    t.write_sectors(geom.abs(RESERVED + fat_sz), &fat0)?; // FAT copy 1, sector 0

    Ok(geom)
}

/// Write `payload` as the file `name` (8.3, uppercase) in the root directory, returning the exact
/// byte extents written (the data clusters, in chain order). The chain starts at cluster 3 and its FAT
/// entries live wholly within FAT sector 0 — the caller's payloads are a few KiB, so this holds; a
/// larger payload would need multi-FAT-sector RMW (out of this arc's scope, guarded below).
pub fn write_payload_file<T: InstallTarget>(
    t: &mut T,
    geom: &FatGeom,
    name: &str,
    payload: &[u8],
) -> Result<Vec<Extent>, InstallError> {
    if payload.is_empty() {
        return Err(InstallError::BadArg);
    }
    let clusters_needed = ((payload.len() + SECTOR - 1) / SECTOR) as u32; // SPC == 1
    // Keep the whole chain inside FAT sector 0 (entries 0..127) for the single-sector RMW below.
    let last_cluster = FIRST_FILE_CLUSTER + clusters_needed - 1;
    if last_cluster >= (SECTOR / 4) as u32 {
        return Err(InstallError::BadArg);
    }
    if FIRST_FILE_CLUSTER + clusters_needed - 2 > geom.count_of_clusters {
        return Err(InstallError::NoSpace);
    }

    // 1) FAT chain: RMW FAT sector 0 of BOTH copies (link 3->4->...->EOC on top of the reserved entries).
    let apply_chain = |fat: &mut [u8; SECTOR]| {
        for i in 0..clusters_needed {
            let cluster = FIRST_FILE_CLUSTER + i;
            let value = if i + 1 == clusters_needed {
                0x0FFF_FFFFu32 // EOC
            } else {
                cluster + 1
            };
            let o = (cluster * 4) as usize;
            fat[o..o + 4].copy_from_slice(&value.to_le_bytes());
        }
    };
    for copy in 0..NUM_FATS {
        let fat_sec0 = geom.fat_start + copy * geom.fat_sz;
        let mut fat = [0u8; SECTOR];
        t.read_sectors(geom.abs(fat_sec0), &mut fat)?;
        apply_chain(&mut fat);
        t.write_sectors(geom.abs(fat_sec0), &fat)?;
    }

    // 2) Payload into the data clusters; record extents.
    let mut extents = Vec::with_capacity(clusters_needed as usize);
    let mut off = 0usize;
    for i in 0..clusters_needed {
        let cluster = FIRST_FILE_CLUSTER + i;
        let lba = geom.cluster_lba(cluster);
        let take = core::cmp::min(SECTOR, payload.len() - off);
        let mut sec = [0u8; SECTOR];
        sec[..take].copy_from_slice(&payload[off..off + take]);
        t.write_sectors(lba, &sec)?;
        extents.push(Extent { lba, len: take });
        off += take;
    }

    // 3) Root directory entry (cluster 2, first data sector). Blank-precondition: the root cluster is
    //    zeroed, so one 8.3 entry at offset 0 is the whole directory.
    let raw = format_83(name).ok_or(InstallError::BadArg)?;
    let mut dir = [0u8; SECTOR];
    dir[0..11].copy_from_slice(&raw);
    dir[11] = 0x20; // ATTR_ARCHIVE (a plain file)
    let hi = (FIRST_FILE_CLUSTER >> 16) as u16;
    let lo = (FIRST_FILE_CLUSTER & 0xFFFF) as u16;
    dir[20..22].copy_from_slice(&hi.to_le_bytes());
    dir[26..28].copy_from_slice(&lo.to_le_bytes());
    dir[28..32].copy_from_slice(&(payload.len() as u32).to_le_bytes());
    t.write_sectors(geom.cluster_lba(ROOT_CLUSTER), &dir)?;

    Ok(extents)
}

/// Encode an 8.3 name into the 11-byte on-disk form (uppercase, space-padded). Returns None for a
/// name that is not representable as a short name.
fn format_83(name: &str) -> Option<[u8; 11]> {
    let mut raw = [b' '; 11];
    let (base, ext) = match name.rsplit_once('.') {
        Some((b, e)) => (b, e),
        None => (name, ""),
    };
    if base.is_empty() || base.len() > 8 || ext.len() > 3 {
        return None;
    }
    for (i, c) in base.bytes().enumerate() {
        raw[i] = upcase_83(c)?;
    }
    for (i, c) in ext.bytes().enumerate() {
        raw[8 + i] = upcase_83(c)?;
    }
    Some(raw)
}

fn upcase_83(c: u8) -> Option<u8> {
    match c {
        b'a'..=b'z' => Some(c - 32),
        b'A'..=b'Z' | b'0'..=b'9' => Some(c),
        b'_' | b'-' | b'~' | b'!' | b'#' | b'$' | b'%' | b'&' | b'(' | b')' | b'@' | b'^' | b'{'
        | b'}' => Some(c),
        _ => None,
    }
}
