// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The FAT32 layout the kernel installer formats an ESP with (Microsoft FAT spec, fatgen103) — ENCODE
//! ONLY, as the kernel's `install/fat32.rs` is today: the boot sector (BPB), the FSInfo sector and the
//! first sector of each FAT, plus the geometry they imply. 512-byte sectors, one sector per cluster,
//! 32 reserved sectors, two FATs, root at cluster 2. The kernel writes these bytes; the tree writer
//! (cluster allocation, directory images) stays in the kernel.

use core::fmt;

use crate::gpt::SECTOR;

pub const RESERVED: u32 = 32;
pub const NUM_FATS: u32 = 2;
/// Sectors per cluster (1 => extent == sector; the simplest deterministic layout).
pub const SPC: u32 = 1;
/// Volume serial "UNAS".
pub const VOL_ID: u32 = 0x554E_4153;
pub const ROOT_CLUSTER: u32 = 2;
/// Volume-relative sector of FSInfo, and of the backup boot sector / backup FSInfo.
pub const FSINFO_SECTOR: u32 = 1;
pub const BACKUP_BOOT_SECTOR: u32 = 6;
pub const BACKUP_FSINFO_SECTOR: u32 = 7;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FatError {
    /// The volume cannot hold a valid FAT32 layout.
    TooSmall,
}

impl fmt::Display for FatError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("volume too small for FAT32")
    }
}

/// Sectors per FAT for a `tot_sec` volume (fatgen §"Determining FAT type"; root_dir_sectors = 0).
pub fn fat_sz(tot_sec: u32) -> u32 {
    let tmpval1 = tot_sec - RESERVED;
    let tmpval2 = (256 * SPC + NUM_FATS) / 2;
    tmpval1.div_ceil(tmpval2)
}

/// The leading ESP sectors that must be ZERO for the blank-precondition format to hold (reserved
/// region + both FATs). Only the size floor is checked here, exactly as before.
pub fn blank_region_sectors(esp_sectors: u64) -> Result<u64, FatError> {
    if esp_sectors > u32::MAX as u64 {
        return Err(FatError::TooSmall);
    }
    let tot_sec = esp_sectors as u32;
    if tot_sec <= RESERVED {
        return Err(FatError::TooSmall);
    }
    Ok((RESERVED + NUM_FATS * fat_sz(tot_sec)) as u64)
}

/// The geometry of one formatted volume, in VOLUME-RELATIVE sectors.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Layout {
    pub tot_sec: u32,
    pub fat_sz: u32,
    pub fat_start: u32,
    pub data_start: u32,
    pub count_of_clusters: u32,
}

impl Layout {
    /// The FAT32 layout of an `esp_sectors` volume, or `TooSmall` when the cluster count is not FAT32's.
    pub fn for_sectors(esp_sectors: u64) -> Result<Self, FatError> {
        if esp_sectors > u32::MAX as u64 || esp_sectors <= RESERVED as u64 {
            return Err(FatError::TooSmall);
        }
        let tot_sec = esp_sectors as u32;
        let fat_sz = fat_sz(tot_sec);
        let data_start = RESERVED + NUM_FATS * fat_sz;
        if data_start >= tot_sec {
            return Err(FatError::TooSmall);
        }
        let count_of_clusters = (tot_sec - data_start) / SPC;
        if !(65525..=0x0FFF_FFF4).contains(&count_of_clusters) {
            return Err(FatError::TooSmall);
        }
        Ok(Self { tot_sec, fat_sz, fat_start: RESERVED, data_start, count_of_clusters })
    }

    /// The boot sector (BPB) for this layout, `hidden` = the partition's absolute first LBA.
    pub fn boot_sector(&self, hidden: u64) -> [u8; SECTOR] {
        let mut bs = [0u8; SECTOR];
        bs[0] = 0xEB;
        bs[1] = 0x58;
        bs[2] = 0x90;
        bs[3..11].copy_from_slice(b"UNAOS   ");
        bs[11..13].copy_from_slice(&(SECTOR as u16).to_le_bytes());
        bs[13] = SPC as u8;
        bs[14..16].copy_from_slice(&(RESERVED as u16).to_le_bytes());
        bs[16] = NUM_FATS as u8;
        bs[21] = 0xF8; // media
        bs[24..26].copy_from_slice(&63u16.to_le_bytes());
        bs[26..28].copy_from_slice(&255u16.to_le_bytes());
        bs[28..32].copy_from_slice(&(hidden as u32).to_le_bytes());
        bs[32..36].copy_from_slice(&self.tot_sec.to_le_bytes());
        bs[36..40].copy_from_slice(&self.fat_sz.to_le_bytes());
        bs[44..48].copy_from_slice(&ROOT_CLUSTER.to_le_bytes());
        bs[48..50].copy_from_slice(&(FSINFO_SECTOR as u16).to_le_bytes());
        bs[50..52].copy_from_slice(&(BACKUP_BOOT_SECTOR as u16).to_le_bytes());
        bs[64] = 0x80;
        bs[66] = 0x29;
        bs[67..71].copy_from_slice(&VOL_ID.to_le_bytes());
        bs[71..82].copy_from_slice(b"UNAOS      ");
        bs[82..90].copy_from_slice(b"FAT32   ");
        bs[510] = 0x55;
        bs[511] = 0xAA;
        bs
    }

    /// Volume-relative sector of FAT copy `n`'s first sector.
    pub fn fat_copy(&self, n: u32) -> u32 {
        self.fat_start + n * self.fat_sz
    }
}

/// The FSInfo sector (free count and next-free "unknown").
pub fn fsinfo() -> [u8; SECTOR] {
    let mut s = [0u8; SECTOR];
    s[0..4].copy_from_slice(&0x4161_5252u32.to_le_bytes());
    s[484..488].copy_from_slice(&0x6141_7272u32.to_le_bytes());
    s[488..492].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    s[492..496].copy_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
    s[510] = 0x55;
    s[511] = 0xAA;
    s
}

/// The first sector of each FAT: entry 0 (media), entry 1 (EOC), entry 2 = the root's one-cluster chain.
pub fn fat0() -> [u8; SECTOR] {
    let mut s = [0u8; SECTOR];
    s[0..4].copy_from_slice(&0x0FFF_FFF8u32.to_le_bytes());
    s[4..8].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
    s[8..12].copy_from_slice(&0x0FFF_FFFFu32.to_le_bytes());
    s
}
