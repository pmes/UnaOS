// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! FAT32 FORMAT — a complete, mountable, fsck-clean empty FAT32 volume written through a
//! [`Block`] (Microsoft "FAT: General Overview of On-Disk Format" v1.03, fatgen103: §3 BPB, §3.3
//! FAT32 extended BPB, §3.5 FAT size computation, §4 FAT entries and the reserved entries 0/1,
//! §5 FSInfo, §6 directory entries and the volume-label entry).
//!
//! [`crate::fat32`] holds the layout the kernel installer has always written (one sector per
//! cluster, free count "unknown"); this module COMPLETES it: any cluster size (the fatgen103 §3.5
//! table picks one when the caller does not), a volume label in the BPB AND as the root
//! directory's volume-ID entry, a real FSInfo free count and next-free hint, the backup boot sector
//! and backup FSInfo, both FATs zeroed whole and seeded, the root cluster zeroed. With one sector
//! per cluster its geometry is [`crate::fat32::Layout`]'s exactly (a KAT pins it).
//!
//! The format is the write list [`writes`] returns; [`format`] issues it. A dry run is [`writes`]
//! alone — the same list, nothing issued.

use alloc::vec::Vec;

use crate::block::{Block, BlockError};
use crate::fat32::{FatError, BACKUP_BOOT_SECTOR, BACKUP_FSINFO_SECTOR, FSINFO_SECTOR, NUM_FATS, RESERVED, ROOT_CLUSTER};
use crate::gpt::SECTOR;
use crate::plan_apply::SectorWrite;

/// Directory-entry attribute: volume label (fatgen103 §6).
pub const ATTR_VOLUME_ID: u8 = 0x08;

/// What to format.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FormatParams {
    /// 11 bytes, space padded, upper case by convention ("NO NAME    " is the unlabelled value).
    pub label: [u8; 11],
    /// The volume serial number.
    pub volume_id: u32,
    /// Sectors per cluster (a power of two, 1..=128), or `None` for the fatgen103 §3.5 table.
    pub sectors_per_cluster: Option<u8>,
    /// The partition's absolute first LBA (BPB_HiddSec).
    pub hidden: u64,
}

impl FormatParams {
    /// A label from text: upper-cased ASCII, characters FAT forbids replaced by `_`, padded/clipped
    /// to 11.
    pub fn label_from(s: &str) -> [u8; 11] {
        let mut l = [b' '; 11];
        for (slot, c) in l.iter_mut().zip(s.bytes()) {
            let c = c.to_ascii_uppercase();
            *slot = if c < 0x20 || c > 0x7E || b"\"*+,./:;<=>?[\\]|".contains(&c) { b'_' } else { c };
        }
        l
    }
    /// The installer's ESP: one sector per cluster (the kernel's tree writer assumes it), the
    /// "UNAS" serial.
    pub fn installer_esp(hidden: u64) -> Self {
        Self { label: *b"UNAOS      ", volume_id: crate::fat32::VOL_ID, sectors_per_cluster: Some(1), hidden }
    }
}

/// fatgen103 §3.5's FAT32 cluster-size table (512-byte sectors): up to 260 MB 0.5 KiB clusters, to
/// 8 GB 4 KiB, to 16 GB 8 KiB, to 32 GB 16 KiB, beyond 32 KiB.
pub fn default_spc(sectors: u64) -> u8 {
    match sectors {
        0..=532_480 => 1,
        532_481..=16_777_216 => 8,
        16_777_217..=33_554_432 => 16,
        33_554_433..=67_108_864 => 32,
        _ => 64,
    }
}

/// The geometry of one volume, volume-relative.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Geometry {
    pub tot_sec: u32,
    pub spc: u32,
    pub fat_sz: u32,
    pub fat_start: u32,
    pub data_start: u32,
    pub count_of_clusters: u32,
}

impl Geometry {
    /// fatgen103 §3.5: FATSz = ceil((TotSec − Rsvd) / ((256·SPC + NumFATs) / 2)) — then the cluster
    /// count must be FAT32's (≥ 65525, and below the reserved range).
    pub fn new(sectors: u64, spc: u8) -> Result<Self, FatError> {
        if !spc.is_power_of_two() || spc > 128 || sectors > u32::MAX as u64 || sectors <= RESERVED as u64 {
            return Err(FatError::TooSmall);
        }
        let tot_sec = sectors as u32;
        let spc = spc as u32;
        let tmp1 = tot_sec - RESERVED;
        let tmp2 = (256 * spc + NUM_FATS) / 2;
        let fat_sz = tmp1.div_ceil(tmp2);
        let data_start = RESERVED + NUM_FATS * fat_sz;
        if data_start >= tot_sec {
            return Err(FatError::TooSmall);
        }
        let count_of_clusters = (tot_sec - data_start) / spc;
        if !(65525..=0x0FFF_FFF4).contains(&count_of_clusters) {
            return Err(FatError::TooSmall);
        }
        // Every cluster (plus the two reserved entries) has a FAT entry.
        if (count_of_clusters as u64 + 2) * 4 > fat_sz as u64 * SECTOR as u64 {
            return Err(FatError::TooSmall);
        }
        Ok(Self { tot_sec, spc, fat_sz, fat_start: RESERVED, data_start, count_of_clusters })
    }

    /// Volume-relative first sector of cluster `n` (n ≥ 2).
    pub fn cluster_sector(&self, n: u32) -> u32 {
        self.data_start + (n - 2) * self.spc
    }
}

/// The BPB (boot sector) for `g`.
pub fn boot_sector(g: &Geometry, p: &FormatParams) -> [u8; SECTOR] {
    let mut bs = [0u8; SECTOR];
    bs[0..3].copy_from_slice(&[0xEB, 0x58, 0x90]);
    bs[3..11].copy_from_slice(b"UNAOS   ");
    bs[11..13].copy_from_slice(&(SECTOR as u16).to_le_bytes());
    bs[13] = g.spc as u8;
    bs[14..16].copy_from_slice(&(RESERVED as u16).to_le_bytes());
    bs[16] = NUM_FATS as u8;
    // RootEntCnt (17) = 0, TotSec16 (19) = 0, FATSz16 (22) = 0 on FAT32.
    bs[21] = 0xF8;
    bs[24..26].copy_from_slice(&63u16.to_le_bytes());
    bs[26..28].copy_from_slice(&255u16.to_le_bytes());
    bs[28..32].copy_from_slice(&(core::cmp::min(p.hidden, u32::MAX as u64) as u32).to_le_bytes());
    bs[32..36].copy_from_slice(&g.tot_sec.to_le_bytes());
    bs[36..40].copy_from_slice(&g.fat_sz.to_le_bytes());
    // ExtFlags (40) = 0: the FATs are mirrored. FSVer (42) = 0.0.
    bs[44..48].copy_from_slice(&ROOT_CLUSTER.to_le_bytes());
    bs[48..50].copy_from_slice(&(FSINFO_SECTOR as u16).to_le_bytes());
    bs[50..52].copy_from_slice(&(BACKUP_BOOT_SECTOR as u16).to_le_bytes());
    bs[64] = 0x80;
    bs[66] = 0x29;
    bs[67..71].copy_from_slice(&p.volume_id.to_le_bytes());
    bs[71..82].copy_from_slice(&p.label);
    bs[82..90].copy_from_slice(b"FAT32   ");
    bs[510] = 0x55;
    bs[511] = 0xAA;
    bs
}

/// FSInfo with a real free count (every cluster but the root's) and next-free hint (the cluster
/// after the root).
pub fn fsinfo(g: &Geometry) -> [u8; SECTOR] {
    let mut s = crate::fat32::fsinfo();
    s[488..492].copy_from_slice(&(g.count_of_clusters - 1).to_le_bytes());
    s[492..496].copy_from_slice(&(ROOT_CLUSTER + 1).to_le_bytes());
    s
}

/// The format of an `sectors`-sector volume, as the exact write list (volume-relative LBAs).
pub fn writes(sectors: u64, p: &FormatParams) -> Result<(Geometry, Vec<SectorWrite>), FatError> {
    let spc = p.sectors_per_cluster.unwrap_or_else(|| default_spc(sectors));
    let g = Geometry::new(sectors, spc)?;
    let mut w = Vec::new();

    // The reserved region, whole: boot sector, FSInfo, their backups, zero elsewhere.
    let mut rsv = alloc::vec![0u8; RESERVED as usize * SECTOR];
    let bs = boot_sector(&g, p);
    let fi = fsinfo(&g);
    for (sec, bytes) in [(0, &bs), (FSINFO_SECTOR, &fi), (BACKUP_BOOT_SECTOR, &bs), (BACKUP_FSINFO_SECTOR, &fi)] {
        rsv[sec as usize * SECTOR..(sec as usize + 1) * SECTOR].copy_from_slice(bytes);
    }
    w.push(SectorWrite::bytes(0, "fat32-reserved", rsv));

    // Both FATs, whole: entry 0 = media, entry 1 = EOC with the clean bits, entry 2 = the root's EOC.
    for n in 0..NUM_FATS {
        let at = (g.fat_start + n * g.fat_sz) as u64;
        w.push(SectorWrite::bytes(at, if n == 0 { "fat32-fat1" } else { "fat32-fat2" }, crate::fat32::fat0().to_vec()));
        w.push(SectorWrite::zero(at + 1, if n == 0 { "fat32-fat1-rest" } else { "fat32-fat2-rest" }, g.fat_sz as u64 - 1));
    }

    // The root directory cluster: the volume-label entry, the rest zero (end of directory).
    let mut root = alloc::vec![0u8; g.spc as usize * SECTOR];
    root[0..11].copy_from_slice(&p.label);
    root[11] = ATTR_VOLUME_ID;
    w.push(SectorWrite::bytes(g.cluster_sector(ROOT_CLUSTER) as u64, "fat32-root", root));
    Ok((g, w))
}

/// Why a format failed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FormatError {
    Fat(FatError),
    Block(BlockError),
}

impl core::fmt::Display for FormatError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FormatError::Fat(e) => write!(f, "{}", e),
            FormatError::Block(e) => write!(f, "{}", e),
        }
    }
}

/// Format the whole of `vol` (pass a [`crate::block::Window`] for a partition) and flush.
pub fn format(vol: &mut dyn Block, p: &FormatParams) -> Result<Geometry, FormatError> {
    let (g, ws) = writes(vol.sectors(), p).map_err(FormatError::Fat)?;
    crate::plan_apply::commit(&ws, vol).map_err(FormatError::Block)?;
    Ok(g)
}
