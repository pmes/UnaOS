// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! Read-only FAT16 / FAT32 reader built on the generic block device.
//!
//! Handles both a **superfloppy** (the FAT BPB sits at LBA 0, no partition table) and an
//! **MBR-partitioned** disk (an MBR at LBA 0 whose partition entry points at the BPB). All
//! multi-byte on-disk fields are little-endian. This module is read-only — it never writes to
//! the FAT, directories, or data — so a mis-parse can at worst report garbage, never corrupt a
//! volume. FAT type is determined strictly by the data-cluster count per the Microsoft FAT
//! specification (the only correct method). FAT12 and non-512-byte logical sectors are rejected.

use alloc::string::String;
use core::sync::atomic::{AtomicBool, Ordering};

/// Logical sector size we support. This equals the USB block device's block size (512 on every
/// stick we target); the BPB's `bytes_per_sector` must agree, so one FAT sector maps 1:1 onto one
/// device block and the LBA math stays exact.
const SECTOR_SIZE: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatError {
    /// No block device is registered (storage not brought up).
    NoDisk,
    /// A block read failed or returned short.
    Io,
    /// No recognizable FAT16/FAT32 volume (neither superfloppy nor MBR partition).
    NotFat,
    /// A FAT variant we do not implement (FAT12, or a non-512-byte logical sector).
    Unsupported,
    /// The named entry was not found in the directory.
    NotFound,
    /// The entry is a directory where a file was expected.
    IsDirectory,
    /// The cluster chain is malformed (free/bad cluster mid-chain, or a loop).
    BadChain,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatKind {
    Fat16,
    Fat32,
}

/// A parsed short (8.3) directory entry. Long-file-name (LFN) entries are skipped, so the name is
/// the on-disk short name (uppercase, e.g. `KERNEL.ELF`).
#[derive(Clone, Copy)]
pub struct DirEntry {
    name: [u8; 12], // "NAME.EXT", NUL-padded (max 8 + '.' + 3 = 12)
    name_len: u8,
    pub is_dir: bool,
    pub size: u32,
    #[allow(dead_code)] // read by `cat` (read_file); populated here so ls and cat share the parse
    first_cluster: u32,
}

impl DirEntry {
    /// The 8.3 name as text (e.g. `"KERNEL.ELF"`).
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("?")
    }
}

/// Parse one 512-byte directory sector, appending real file/dir entries to `out`. Returns `true`
/// if a 0x00 (end-of-directory) marker was reached, telling the caller to stop scanning.
fn scan_dir_sector(sec: &[u8; SECTOR_SIZE], out: &mut alloc::vec::Vec<DirEntry>) -> bool {
    for i in 0..(SECTOR_SIZE / 32) {
        let e = &sec[i * 32..i * 32 + 32];
        match e[0] {
            0x00 => return true, // no more entries in this directory
            0xE5 => continue,    // deleted entry
            _ => {}
        }
        let attr = e[11];
        if attr & 0x0F == 0x0F {
            continue; // long-file-name component
        }
        if attr & 0x08 != 0 {
            continue; // volume label
        }
        // 8.3 name: base (8) '.' ext (3), each with trailing spaces trimmed. 0x05 in byte 0 is an
        // escaped 0xE5 (a legitimate leading byte, distinct from the deleted marker).
        let mut name = [0u8; 12];
        let mut n = 0usize;
        let mut base = 8usize;
        while base > 0 && e[base - 1] == b' ' {
            base -= 1;
        }
        for k in 0..base {
            name[n] = if k == 0 && e[0] == 0x05 { 0xE5 } else { e[k] };
            n += 1;
        }
        let mut ext = 3usize;
        while ext > 0 && e[8 + ext - 1] == b' ' {
            ext -= 1;
        }
        if ext > 0 {
            name[n] = b'.';
            n += 1;
            for k in 0..ext {
                name[n] = e[8 + k];
                n += 1;
            }
        }
        out.push(DirEntry {
            name,
            name_len: n as u8,
            is_dir: attr & 0x10 != 0,
            size: u32le(e, 28),
            first_cluster: ((u16le(e, 20) as u32) << 16) | u16le(e, 26) as u32,
        });
    }
    false
}

/// A mounted FAT volume: the fully-resolved geometry needed to walk the FAT, the root directory,
/// and cluster chains. All LBAs are **absolute** (device-relative), already offset by the
/// partition start, so callers pass them straight to `block::read_block`.
pub struct FatFs {
    kind: FatKind,
    /// Absolute LBA of the volume's boot sector (0 for a superfloppy).
    part_lba: u64,
    bytes_per_sec: u32,
    sec_per_clus: u32,
    reserved: u32,
    num_fats: u32,
    /// Sectors per FAT.
    fat_sz: u32,
    /// Absolute LBA of the first FAT.
    fat_start: u64,
    /// Absolute LBA of cluster 2 (start of the data region).
    data_start: u64,
    /// FAT32 root directory's first cluster (0 on FAT16).
    root_cluster: u32,
    /// FAT16 fixed root directory: absolute start LBA and length in sectors (0 on FAT32).
    root_dir_lba: u64,
    root_dir_sectors: u32,
    /// Number of data clusters. Valid cluster numbers are `2 ..= count_of_clusters + 1`.
    count_of_clusters: u32,
}

// ---- little-endian field readers ------------------------------------------------------------

#[inline]
fn u16le(b: &[u8], off: usize) -> u16 {
    (b[off] as u16) | ((b[off + 1] as u16) << 8)
}

#[inline]
fn u32le(b: &[u8], off: usize) -> u32 {
    (b[off] as u32)
        | ((b[off + 1] as u32) << 8)
        | ((b[off + 2] as u32) << 16)
        | ((b[off + 3] as u32) << 24)
}

/// Read one 512-byte sector at absolute `lba` into `buf`. Treats a short copy as I/O error, so
/// callers can assume a full sector on success.
fn read_sector(lba: u64, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), FatError> {
    match crate::drivers::block::read_block(lba, buf) {
        Ok(n) if n >= SECTOR_SIZE => Ok(()),
        _ => Err(FatError::Io),
    }
}

/// Try to interpret `sec` as a FAT boot sector (BPB) for a volume starting at absolute `part_lba`,
/// on a device of `dev_blocks` total blocks. Returns a fully-computed [`FatFs`] on success.
///
/// Rejects (as `NotFat`) anything that is not a plausible, self-consistent FAT volume: this is
/// also what distinguishes a superfloppy BPB from an MBR boot sector — an MBR's bootstrap bytes
/// won't pass the jump-instruction, sector-size, and geometry-consistency gates. FAT12 and
/// non-512-byte sectors are rejected as `Unsupported`.
fn parse_bpb(sec: &[u8; SECTOR_SIZE], part_lba: u64, dev_blocks: u64) -> Result<FatFs, FatError> {
    // BS_JmpBoot (offset 0): a FAT VBR starts with EB xx 90 or E9 xx xx. Strong VBR discriminator.
    if !(sec[0] == 0xEB || sec[0] == 0xE9) {
        return Err(FatError::NotFat);
    }
    // Boot signature 0x55AA at offset 510.
    if sec[510] != 0x55 || sec[511] != 0xAA {
        return Err(FatError::NotFat);
    }

    let bytes_per_sec = u16le(sec, 11) as u32;
    if bytes_per_sec != SECTOR_SIZE as u32 {
        // We only support 512-byte logical sectors (== the block device's block size).
        return Err(FatError::NotFat);
    }
    let sec_per_clus = sec[13] as u32;
    if sec_per_clus == 0 || !sec_per_clus.is_power_of_two() || sec_per_clus > 128 {
        return Err(FatError::NotFat);
    }
    let reserved = u16le(sec, 14) as u32;
    if reserved == 0 {
        return Err(FatError::NotFat);
    }
    let num_fats = sec[16] as u32;
    if num_fats == 0 || num_fats > 2 {
        return Err(FatError::NotFat);
    }
    let root_ent_cnt = u16le(sec, 17) as u32;
    let tot_sec16 = u16le(sec, 19) as u32;
    let fat_sz16 = u16le(sec, 22) as u32;
    let tot_sec32 = u32le(sec, 32);
    let fat_sz32 = u32le(sec, 36);

    let fat_sz = if fat_sz16 != 0 { fat_sz16 } else { fat_sz32 };
    let tot_sec = if tot_sec16 != 0 { tot_sec16 } else { tot_sec32 };
    if fat_sz == 0 || tot_sec == 0 {
        return Err(FatError::NotFat);
    }

    // Fixed root-directory size (0 on FAT32, where root_ent_cnt == 0). All arithmetic in u32:
    // root_ent_cnt <= 65535, so root_ent_cnt*32 <= ~2M — no overflow.
    let root_dir_sectors = ((root_ent_cnt * 32) + (bytes_per_sec - 1)) / bytes_per_sec;

    // Region layout, relative to the volume start. num_fats*fat_sz <= 2 * ~16M for FAT32; fits u32
    // for any real volume, but use checked math so a corrupt/hostile BPB can't wrap to a small
    // value that then passes the consistency gate below.
    let fat_region = num_fats.checked_mul(fat_sz).ok_or(FatError::NotFat)?;
    let first_data_sector = reserved
        .checked_add(fat_region)
        .and_then(|v| v.checked_add(root_dir_sectors))
        .ok_or(FatError::NotFat)?;
    if first_data_sector >= tot_sec {
        return Err(FatError::NotFat);
    }
    let data_sec = tot_sec - first_data_sector;
    let count_of_clusters = data_sec / sec_per_clus;

    // FAT type is defined SOLELY by the cluster count (Microsoft FAT spec). Not the FS-type string.
    let kind = if count_of_clusters < 4085 {
        return Err(FatError::Unsupported); // FAT12 — not implemented
    } else if count_of_clusters < 65525 {
        FatKind::Fat16
    } else {
        FatKind::Fat32
    };

    // Consistency vs the physical device: the whole volume must fit on the disk. This is the final
    // gate that makes an MBR boot sector (or random data) passing as a superfloppy essentially
    // impossible.
    if part_lba.saturating_add(tot_sec as u64) > dev_blocks {
        return Err(FatError::NotFat);
    }

    let root_cluster = if kind == FatKind::Fat32 {
        u32le(sec, 44) & 0x0FFF_FFFF
    } else {
        0
    };
    // A FAT32 root cluster must be a valid data cluster.
    if kind == FatKind::Fat32 && (root_cluster < 2 || root_cluster >= count_of_clusters + 2) {
        return Err(FatError::NotFat);
    }

    let fat_start = part_lba + reserved as u64;
    let root_dir_lba = part_lba + (reserved + fat_region) as u64; // FAT16 fixed region (unused on FAT32)
    let data_start = part_lba + first_data_sector as u64;

    Ok(FatFs {
        kind,
        part_lba,
        bytes_per_sec,
        sec_per_clus,
        reserved,
        num_fats,
        fat_sz,
        fat_start,
        data_start,
        root_cluster,
        root_dir_lba,
        root_dir_sectors,
        count_of_clusters,
    })
}

/// Mount the FAT volume on the registered block device. Detects a superfloppy (BPB at LBA 0)
/// first; failing that, an MBR at LBA 0 whose first FAT-typed partition entry points at the BPB.
pub fn mount() -> Result<FatFs, FatError> {
    let dev = crate::drivers::block::info().ok_or(FatError::NoDisk)?;
    if dev.block_size != SECTOR_SIZE as u32 {
        return Err(FatError::Unsupported);
    }
    let dev_blocks = dev.num_blocks;

    let mut sec = [0u8; SECTOR_SIZE];
    read_sector(0, &mut sec)?;

    // 1) Superfloppy: LBA 0 is itself the BPB.
    if let Ok(fs) = parse_bpb(&sec, 0, dev_blocks) {
        return Ok(fs);
    }

    // 2) MBR-partitioned: 0x55AA signature + a partition table at offset 446. Scan the four
    //    primary entries; for each non-empty, non-extended entry, try to parse a BPB at its start
    //    LBA. First one that validates wins.
    if sec[510] == 0x55 && sec[511] == 0xAA {
        for i in 0..4 {
            let e = 446 + i * 16;
            let ptype = sec[e + 4];
            let start = u32le(&sec, e + 8);
            // Skip empty (0x00) and extended-partition containers (0x05 CHS / 0x0F LBA).
            if ptype == 0x00 || ptype == 0x05 || ptype == 0x0F || start == 0 {
                continue;
            }
            if start as u64 >= dev_blocks {
                continue;
            }
            let mut pbs = [0u8; SECTOR_SIZE];
            if read_sector(start as u64, &mut pbs).is_err() {
                continue;
            }
            if let Ok(fs) = parse_bpb(&pbs, start as u64, dev_blocks) {
                return Ok(fs);
            }
        }
    }

    Err(FatError::NotFat)
}

impl FatFs {
    pub fn kind(&self) -> FatKind {
        self.kind
    }

    /// One-line human summary of the parsed geometry (for `fatinfo` / boot log).
    pub fn describe(&self) -> String {
        let head = alloc::format!(
            "FAT{} vol@LBA{} bps={} spc={} nfat={} fatsz={}sec reserved={} fat@LBA{} data@LBA{} clusters={}",
            match self.kind {
                FatKind::Fat16 => 16,
                FatKind::Fat32 => 32,
            },
            self.part_lba,
            self.bytes_per_sec,
            self.sec_per_clus,
            self.num_fats,
            self.fat_sz,
            self.reserved,
            self.fat_start,
            self.data_start,
            self.count_of_clusters,
        );
        match self.kind {
            FatKind::Fat32 => alloc::format!("{head} rootclus={}", self.root_cluster),
            FatKind::Fat16 => {
                alloc::format!("{head} rootdir@LBA{} ({}sec)", self.root_dir_lba, self.root_dir_sectors)
            }
        }
    }

    // --- cluster / FAT-chain helpers ---

    fn valid_cluster(&self, c: u32) -> bool {
        c >= 2 && c < self.count_of_clusters + 2
    }

    /// Absolute LBA of the first sector of a data cluster (`cluster` >= 2).
    fn cluster_lba(&self, cluster: u32) -> u64 {
        self.data_start + (cluster as u64 - 2) * self.sec_per_clus as u64
    }

    fn is_eoc(&self, e: u32) -> bool {
        match self.kind {
            FatKind::Fat16 => e >= 0xFFF8,
            FatKind::Fat32 => e >= 0x0FFF_FFF8,
        }
    }

    fn is_bad(&self, e: u32) -> bool {
        match self.kind {
            FatKind::Fat16 => e == 0xFFF7,
            FatKind::Fat32 => e == 0x0FFF_FFF7,
        }
    }

    /// Read the FAT entry for `cluster` (the next cluster in the chain). A 2- or 4-byte entry never
    /// straddles a 512-byte sector boundary (2 and 4 both divide 512), so one sector read suffices.
    fn fat_entry(&self, cluster: u32) -> Result<u32, FatError> {
        let offset = match self.kind {
            FatKind::Fat16 => cluster as u64 * 2,
            FatKind::Fat32 => cluster as u64 * 4,
        };
        let sec = offset / SECTOR_SIZE as u64;
        let within = (offset % SECTOR_SIZE as u64) as usize;
        let mut buf = [0u8; SECTOR_SIZE];
        read_sector(self.fat_start + sec, &mut buf)?;
        Ok(match self.kind {
            FatKind::Fat16 => u16le(&buf, within) as u32,
            FatKind::Fat32 => u32le(&buf, within) & 0x0FFF_FFFF,
        })
    }

    /// List the root directory. FAT32 follows the root cluster chain; FAT16 reads its fixed region.
    pub fn read_root(&self) -> Result<alloc::vec::Vec<DirEntry>, FatError> {
        match self.kind {
            FatKind::Fat32 => self.read_dir_chain(self.root_cluster),
            FatKind::Fat16 => self.read_fixed_root16(),
        }
    }

    /// FAT16 fixed root directory: a contiguous run of sectors, no cluster chain.
    fn read_fixed_root16(&self) -> Result<alloc::vec::Vec<DirEntry>, FatError> {
        let mut out = alloc::vec::Vec::new();
        let mut buf = [0u8; SECTOR_SIZE];
        for s in 0..self.root_dir_sectors as u64 {
            read_sector(self.root_dir_lba + s, &mut buf)?;
            if scan_dir_sector(&buf, &mut out) {
                break;
            }
        }
        Ok(out)
    }

    /// Walk a directory stored as a cluster chain (the FAT32 root, or any subdirectory), collecting
    /// its entries. Stops at the 0x00 terminator or end-of-chain; guards against bad/free clusters
    /// and a chain longer than the whole volume (loop protection).
    fn read_dir_chain(&self, start: u32) -> Result<alloc::vec::Vec<DirEntry>, FatError> {
        let mut out = alloc::vec::Vec::new();
        let mut cluster = start;
        let mut hops = 0u32;
        let mut buf = [0u8; SECTOR_SIZE];
        loop {
            if !self.valid_cluster(cluster) {
                return Err(FatError::BadChain);
            }
            for s in 0..self.sec_per_clus as u64 {
                read_sector(self.cluster_lba(cluster) + s, &mut buf)?;
                if scan_dir_sector(&buf, &mut out) {
                    return Ok(out);
                }
            }
            let next = self.fat_entry(cluster)?;
            if self.is_eoc(next) {
                return Ok(out);
            }
            if self.is_bad(next) || next < 2 {
                return Err(FatError::BadChain);
            }
            cluster = next;
            hops += 1;
            if hops > self.count_of_clusters + 1 {
                return Err(FatError::BadChain);
            }
        }
    }
}

/// One-shot boot probe: the first time a block device is present, mount the FAT volume and log its
/// geometry to serial (captured on QEMU; visible on a serial-less metal boot only in bootlog /
/// usbdebug builds — the interactive `fatinfo`/`ls`/`cat` commands are the metal evidence). Safe to
/// call every main-loop iteration: it no-ops until storage is up, then runs exactly once.
pub fn probe_once() {
    static PROBED: AtomicBool = AtomicBool::new(false);
    if PROBED.load(Ordering::Relaxed) {
        return;
    }
    if crate::drivers::block::info().is_none() {
        return; // storage not brought up yet
    }
    PROBED.store(true, Ordering::Relaxed);

    match mount() {
        Ok(fs) => {
            serial_println!("FS: FAT mounted: {}", fs.describe());
            match fs.read_root() {
                Ok(entries) => {
                    serial_println!("FS: root directory ({} entries):", entries.len());
                    for de in &entries {
                        if de.is_dir {
                            serial_println!("FS:   <DIR>              {}", de.name());
                        } else {
                            serial_println!("FS:   {:>12}       {}", de.size, de.name());
                        }
                    }
                }
                Err(e) => serial_println!("FS: root directory read error ({:?})", e),
            }
        }
        Err(e) => serial_println!("FS: no FAT filesystem ({:?})", e),
    }
}
