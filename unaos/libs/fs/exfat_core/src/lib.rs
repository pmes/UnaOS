// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! # exfat_core — the exFAT reader both rings link (EXFAT, rmbp-ledger B392)
//!
//! Written from Microsoft's published *exFAT file system specification* (public since 2019 under
//! the OIN pledge; R83 — no driver source was read). Section numbers below are the spec's.
//!
//! The core is read-only (write is owed: bitmap allocation, the entry-set rewrite and its checksum,
//! VolumeDirty). It never touches a device: every read goes through [`SectorRead`], in 512-byte
//! device sectors, so the host KATs read an image file and the kernel reads a USB disk through the
//! same code.
//!
//! What is read: the main boot region and its checksum sector (§3.1, §3.4; the backup region at
//! sector 12 when the main one fails its checksum), the active FAT (§4), the allocation bitmap
//! (§7.1), the up-case table (§7.2, checksum-verified, decompressed), the volume label (§7.3),
//! directory entry sets — file (§7.4) + stream extension (§7.6) + file name (§7.7) — with the
//! entry-set checksum (§6.3.3) and the name hash (§7.6.4), cluster chains with the NoFatChain flag
//! (§6.3.4.2). Names are UTF-16 on the medium and UTF-8 here; lookup is case-insensitive through the
//! volume's own up-case table.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

/// The device sector every [`SectorRead`] call is addressed in.
pub const SECTOR: usize = 512;

/// The one device seam: read whole 512-byte sectors at an absolute device LBA. `buf.len()` is
/// always a non-zero multiple of [`SECTOR`]; an implementation may split it however its transport
/// needs.
pub trait SectorRead {
    fn read_sectors(&self, lba: u64, buf: &mut [u8]) -> Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The device failed the read.
    Io,
    /// The device is loaned out; try again (the kernel's `Busy`).
    Busy,
    /// No exFAT volume here (OEM name, signature or geometry).
    NotExfat,
    /// Neither boot region's checksum sector matches its sectors (§3.4).
    BootChecksum,
    /// The up-case table's bytes do not sum to its directory entry's TableChecksum (§7.2.2).
    UpcaseChecksum,
    /// A structure the spec makes mandatory is missing or out of range; the reason names it.
    Corrupt(&'static str),
    NotFound,
    NotADirectory,
    IsADirectory,
    Unsupported,
}

pub type Result<T> = core::result::Result<T, Error>;

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn le64(b: &[u8], o: usize) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[o..o + 8]);
    u64::from_le_bytes(a)
}

// --- checksums (§3.4, §6.3.3, §7.2.2, §7.6.4) ------------------------------------------------------

/// §3.4 BootChecksum over the first 11 sectors of a boot region, skipping VolumeFlags (106, 107)
/// and PercentInUse (112) — the three bytes a mounted volume changes without rewriting the region.
pub fn boot_checksum(region: &[u8]) -> u32 {
    let mut c: u32 = 0;
    for (i, &b) in region.iter().enumerate() {
        if i == 106 || i == 107 || i == 112 {
            continue;
        }
        c = c.rotate_right(1).wrapping_add(b as u32);
    }
    c
}

/// §7.2.2 TableChecksum: the same 32-bit rotate-add over every byte of the up-case table.
pub fn table_checksum(bytes: &[u8]) -> u32 {
    bytes.iter().fold(0u32, |c, &b| c.rotate_right(1).wrapping_add(b as u32))
}

/// §6.3.3 SetChecksum: 16-bit rotate-add over every byte of the set, skipping bytes 2 and 3 of the
/// first (primary) entry — the field itself.
pub fn set_checksum(set: &[u8]) -> u16 {
    let mut c: u16 = 0;
    for (i, &b) in set.iter().enumerate() {
        if i == 2 || i == 3 {
            continue;
        }
        c = c.rotate_right(1).wrapping_add(b as u16);
    }
    c
}

/// §7.6.4 NameHash: 16-bit rotate-add over the UP-CASED name, each unit low byte then high byte.
pub fn name_hash(upcased: &[u16]) -> u16 {
    let mut h: u16 = 0;
    for &u in upcased {
        h = h.rotate_right(1).wrapping_add(u & 0xff);
        h = h.rotate_right(1).wrapping_add(u >> 8);
    }
    h
}

// --- the up-case table (§7.2.5) ---------------------------------------------------------------------

/// The up-case table, decompressed to its NON-identity mappings (sorted by source unit). The
/// recommended table is ~5.8 KiB on the medium and maps ~1500 units; a full 128 KiB array per
/// mount would be waste.
#[derive(Debug, Clone, Default)]
pub struct Upcase {
    map: Vec<(u16, u16)>,
}

impl Upcase {
    /// Decompress the table bytes. `0xFFFF n` is a run of `n` identity mappings (the compressed
    /// form, §7.2.5.1); every other unit maps the next source unit. Units past the table's end map
    /// to themselves.
    pub fn decode(bytes: &[u8]) -> Self {
        let mut map = Vec::new();
        let mut ch: u32 = 0;
        let mut i = 0;
        let n = bytes.len() / 2;
        while i < n && ch <= 0xFFFF {
            let v = le16(bytes, i * 2);
            if v == 0xFFFF && i + 1 < n {
                ch += le16(bytes, (i + 1) * 2) as u32;
                i += 2;
                continue;
            }
            if v as u32 != ch {
                map.push((ch as u16, v));
            }
            ch += 1;
            i += 1;
        }
        Upcase { map }
    }

    /// The ASCII-only table the spec allows when no table is readable — used only by the KAT
    /// helpers; a volume without a valid table does not mount.
    pub fn ascii() -> Self {
        Upcase { map: (b'a'..=b'z').map(|c| (c as u16, (c - 32) as u16)).collect() }
    }

    pub fn up(&self, u: u16) -> u16 {
        match self.map.binary_search_by_key(&u, |p| p.0) {
            Ok(i) => self.map[i].1,
            Err(_) => u,
        }
    }

    pub fn mappings(&self) -> usize {
        self.map.len()
    }

    pub fn eq_ignore_case(&self, a: &[u16], b: &[u16]) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(&x, &y)| x == y || self.up(x) == self.up(y))
    }

    pub fn hash(&self, name: &[u16]) -> u16 {
        let up: Vec<u16> = name.iter().map(|&u| self.up(u)).collect();
        name_hash(&up)
    }
}

/// UTF-16 → UTF-8; an unpaired surrogate becomes U+FFFD (a name is shown, never refused).
pub fn utf16_to_string(units: &[u16]) -> String {
    core::char::decode_utf16(units.iter().copied())
        .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

// --- timestamps (§7.4.8) ----------------------------------------------------------------------------

/// A §7.4.8 timestamp as recorded (local time) plus its §7.4.10 UTC offset when valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stamp {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub min: u8,
    pub sec: u8,
    /// Minutes east of UTC, when the OffsetValid bit is set.
    pub utc_offset_min: Option<i16>,
}

impl Stamp {
    pub fn decode(ts: u32, ten_ms: u8, utc: u8) -> Self {
        let off = if utc & 0x80 != 0 {
            // 7-bit two's complement, 15-minute increments.
            let v = (utc & 0x7f) as i16;
            let v = if v & 0x40 != 0 { v - 0x80 } else { v };
            Some(v * 15)
        } else {
            None
        };
        Stamp {
            year: 1980 + (ts >> 25) as u16,
            month: ((ts >> 21) & 0xf) as u8,
            day: ((ts >> 16) & 0x1f) as u8,
            hour: ((ts >> 11) & 0x1f) as u8,
            min: ((ts >> 5) & 0x3f) as u8,
            sec: ((ts & 0x1f) * 2) as u8 + ten_ms / 100,
            utc_offset_min: off,
        }
    }

    /// A zero stamp is the medium's "unset" (month and day 0 are not dates).
    pub fn is_unset(&self) -> bool {
        self.month == 0 || self.day == 0
    }
}

// --- locating a volume on a disk ------------------------------------------------------------------

/// Does this sector open an exFAT volume? (§3.1.2 FileSystemName, §3.1.20 BootSignature.)
pub fn is_exfat_vbr(sec: &[u8]) -> bool {
    sec.len() >= SECTOR && &sec[3..11] == b"EXFAT   " && sec[510] == 0x55 && sec[511] == 0xAA
}

/// Where the first exFAT volume on a disk starts: `(start_lba, blocks, slot)` — a superfloppy
/// (LBA 0, slot 0), a GPT entry (slot 1..), or an MBR entry (slot 1..4), by the same first-match
/// order the FAT mount uses.
pub fn locate<D: SectorRead + ?Sized>(dev: &D, dev_blocks: u64) -> Result<(u64, u64, u8)> {
    let mut s0 = [0u8; SECTOR];
    dev.read_sectors(0, &mut s0)?;
    if is_exfat_vbr(&s0) {
        return Ok((0, dev_blocks, 0));
    }
    if s0[510] != 0x55 || s0[511] != 0xAA {
        return Err(Error::NotExfat);
    }
    let mut pbs = [0u8; SECTOR];
    let protective = (0..4).any(|k| s0[446 + k * 16 + 4] == 0xEE);
    if protective {
        let mut hdr = [0u8; SECTOR];
        dev.read_sectors(1, &mut hdr)?;
        if &hdr[0..8] == b"EFI PART" {
            let ents_lba = le64(&hdr, 72);
            let n = le32(&hdr, 80).min(128) as usize;
            let esz = le32(&hdr, 84) as usize;
            if esz >= 128 && esz % 128 == 0 && esz <= SECTOR && n > 0 {
                let bytes = n * esz;
                let secs = bytes.div_ceil(SECTOR);
                let mut tab = vec![0u8; secs * SECTOR];
                dev.read_sectors(ents_lba, &mut tab)?;
                for k in 0..n {
                    let e = &tab[k * esz..k * esz + 128];
                    if e[0..16].iter().all(|&b| b == 0) {
                        continue;
                    }
                    let first = le64(e, 32);
                    let last = le64(e, 40);
                    if first == 0 || last < first || last >= dev_blocks {
                        continue;
                    }
                    if dev.read_sectors(first, &mut pbs).is_ok() && is_exfat_vbr(&pbs) {
                        return Ok((first, last - first + 1, (k + 1) as u8));
                    }
                }
            }
        }
        return Err(Error::NotExfat);
    }
    for k in 0..4 {
        let e = &s0[446 + k * 16..446 + k * 16 + 16];
        let ty = e[4];
        let start = le32(e, 8) as u64;
        let count = le32(e, 12) as u64;
        if ty == 0 || ty == 0x05 || ty == 0x0F || start == 0 || count == 0 || start + count > dev_blocks {
            continue;
        }
        if dev.read_sectors(start, &mut pbs).is_ok() && is_exfat_vbr(&pbs) {
            return Ok((start, count, (k + 1) as u8));
        }
    }
    Err(Error::NotExfat)
}

// --- the volume -----------------------------------------------------------------------------------

/// FileAttributes.Directory (§7.4.4).
pub const ATTR_DIR: u16 = 0x10;

/// A file or directory as its entry set describes it. The root directory is the one node no set
/// describes ([`Volume::root`]).
#[derive(Debug, Clone)]
pub struct Node {
    pub name: String,
    pub units: Vec<u16>,
    pub attrs: u16,
    pub first_cluster: u32,
    /// DataLength (§7.6.6). For the root directory: 0 = "until end of chain".
    pub data_len: u64,
    /// ValidDataLength (§7.6.5): bytes past it read as zero.
    pub valid_len: u64,
    /// GeneralSecondaryFlags.NoFatChain (§6.3.4.2): the clusters are contiguous and the FAT is not
    /// consulted.
    pub contiguous: bool,
    pub name_hash: u16,
    pub mtime: Stamp,
    pub is_root: bool,
}

impl Node {
    pub fn is_dir(&self) -> bool {
        self.is_root || self.attrs & ATTR_DIR != 0
    }
}

/// One directory read: the entry sets that verified, and what did not.
#[derive(Debug, Clone, Default)]
pub struct Listing {
    pub nodes: Vec<Node>,
    /// File sets whose SetChecksum did not verify, or whose secondaries were malformed — skipped.
    pub bad_sets: usize,
    /// File sets whose NameHash did not match the up-cased name (kept: the name is still readable).
    pub bad_hash: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Geometry {
    pub part_lba: u64,
    pub volume_length: u64,
    pub bps_shift: u8,
    pub spc_shift: u8,
    pub fat_offset: u32,
    pub fat_length: u32,
    pub heap_offset: u32,
    pub cluster_count: u32,
    pub root_cluster: u32,
    pub serial: u32,
    pub revision: u16,
    pub volume_flags: u16,
    pub number_of_fats: u8,
    pub percent_in_use: u8,
}

pub struct Volume {
    pub geo: Geometry,
    /// Which FAT/bitmap is active (VolumeFlags.ActiveFat, meaningful only with two FATs — TexFAT).
    active: u8,
    /// The main boot region failed its checksum and the backup (sector 12) mounted instead.
    pub backup_boot: bool,
    upcase: Upcase,
    upcase_loc: (u32, u64),
    bitmap_loc: (u32, u64),
    label: String,
}

/// The FAT sector a chain walk last read — one sector holds 128 links.
struct FatCache {
    lba: u64,
    buf: [u8; SECTOR],
}

impl FatCache {
    fn new() -> Self {
        FatCache { lba: u64::MAX, buf: [0; SECTOR] }
    }
}

/// The largest run one read asks the device for.
const MAX_RUN_BYTES: u64 = 256 * 1024;
/// The largest directory read (the spec caps a directory at 256 MiB; a card with a directory this
/// large is past what a listing should hold in memory).
const MAX_DIR_BYTES: u64 = 64 * 1024 * 1024;

fn parse_boot(region: &[u8], part_lba: u64, part_blocks: u64) -> Result<Geometry> {
    let s = region;
    if !is_exfat_vbr(s) || s[0] != 0xEB || s[1] != 0x76 || s[2] != 0x90 {
        return Err(Error::NotExfat);
    }
    if s[11..64].iter().any(|&b| b != 0) {
        return Err(Error::NotExfat); // MustBeZero: the BPB area of a FAT volume
    }
    let g = Geometry {
        part_lba,
        volume_length: le64(s, 72),
        fat_offset: le32(s, 80),
        fat_length: le32(s, 84),
        heap_offset: le32(s, 88),
        cluster_count: le32(s, 92),
        root_cluster: le32(s, 96),
        serial: le32(s, 100),
        revision: le16(s, 104),
        volume_flags: le16(s, 106),
        bps_shift: s[108],
        spc_shift: s[109],
        number_of_fats: s[110],
        percent_in_use: s[112],
    };
    let bad = |r| Err(Error::Corrupt(r));
    if !(9..=12).contains(&g.bps_shift) {
        return bad("bytes-per-sector");
    }
    if g.spc_shift as u32 + g.bps_shift as u32 > 25 {
        return bad("cluster-size");
    }
    if g.number_of_fats != 1 && g.number_of_fats != 2 {
        return bad("number-of-fats");
    }
    if g.revision >> 8 != 1 {
        return Err(Error::Unsupported); // major revision 1 is the only one the spec defines
    }
    if g.fat_offset < 24 || g.fat_length == 0 {
        return bad("fat-offset");
    }
    let fats_end = g.fat_offset as u64 + g.fat_length as u64 * g.number_of_fats as u64;
    if (g.heap_offset as u64) < fats_end {
        return bad("heap-offset");
    }
    if g.cluster_count == 0 || g.cluster_count > 0xFFFF_FFF5 {
        return bad("cluster-count");
    }
    // The FAT must hold an entry for every cluster (+ the two reserved).
    if (g.cluster_count as u64 + 2) * 4 > (g.fat_length as u64) << g.bps_shift {
        return bad("fat-length");
    }
    let heap_end = g.heap_offset as u64 + ((g.cluster_count as u64) << g.spc_shift);
    if heap_end > g.volume_length {
        return bad("volume-length");
    }
    let dev_per = 1u64 << (g.bps_shift - 9);
    if part_blocks != 0 && g.volume_length.saturating_mul(dev_per) > part_blocks {
        return bad("volume-past-partition");
    }
    if g.root_cluster < 2 || g.root_cluster > g.cluster_count + 1 {
        return bad("root-cluster");
    }
    Ok(g)
}

/// Verify one boot region (12 sectors of `bps` bytes): sector 11 repeats the checksum of 0..11.
fn region_ok(region: &[u8], bps: usize) -> bool {
    if region.len() < 12 * bps {
        return false;
    }
    let c = boot_checksum(&region[..11 * bps]);
    region[11 * bps..12 * bps].chunks_exact(4).all(|w| le32(w, 0) == c)
}

impl Volume {
    /// Mount the exFAT volume starting at device LBA `part_lba` (`part_blocks` device sectors long;
    /// 0 = unbounded). Verifies the boot checksum (falling back to the backup region), reads the
    /// root directory's critical entries, and loads and verifies the up-case table.
    pub fn mount<D: SectorRead + ?Sized>(dev: &D, part_lba: u64, part_blocks: u64) -> Result<Self> {
        let mut s0 = [0u8; SECTOR];
        dev.read_sectors(part_lba, &mut s0)?;
        if !is_exfat_vbr(&s0) {
            return Err(Error::NotExfat);
        }
        let shift = s0[108];
        if !(9..=12).contains(&shift) {
            return Err(Error::Corrupt("bytes-per-sector"));
        }
        let bps = 1usize << shift;
        let per = (bps / SECTOR) as u64;
        let mut region = vec![0u8; 12 * bps];
        dev.read_sectors(part_lba, &mut region)?;
        let mut backup = false;
        if !region_ok(&region, bps) {
            dev.read_sectors(part_lba + 12 * per, &mut region)?;
            if !region_ok(&region, bps) {
                return Err(Error::BootChecksum);
            }
            backup = true;
        }
        let geo = parse_boot(&region, part_lba, part_blocks)?;
        let active = if geo.number_of_fats == 2 { (geo.volume_flags & 1) as u8 } else { 0 };
        let mut v = Volume {
            geo,
            active,
            backup_boot: backup,
            upcase: Upcase::default(),
            upcase_loc: (0, 0),
            bitmap_loc: (0, 0),
            label: String::new(),
        };
        // §7: the root directory's critical primaries.
        let root = v.read_dir_bytes(dev, &v.root())?;
        let mut up: Option<(u32, u64, u32)> = None;
        let mut bitmap: Option<(u32, u64)> = None;
        for e in root.chunks_exact(32) {
            match e[0] {
                0x00 => break,
                0x81 => {
                    // BitmapFlags bit 0 names which FAT this bitmap goes with (two only under TexFAT).
                    if (e[1] & 1) == v.active || bitmap.is_none() {
                        bitmap = Some((le32(e, 20), le64(e, 24)));
                    }
                }
                0x82 => up = Some((le32(e, 20), le64(e, 24), le32(e, 4))),
                0x83 => {
                    let n = (e[1] as usize).min(11);
                    let units: Vec<u16> = (0..n).map(|k| le16(e, 2 + k * 2)).collect();
                    v.label = utf16_to_string(&units);
                }
                _ => {}
            }
        }
        let (bf, bl) = bitmap.ok_or(Error::Corrupt("no-bitmap"))?;
        if (bl * 8) < v.geo.cluster_count as u64 || !v.valid_cluster(bf) {
            return Err(Error::Corrupt("bitmap"));
        }
        v.bitmap_loc = (bf, bl);
        let (uf, ul, ucs) = up.ok_or(Error::Corrupt("no-upcase"))?;
        if ul == 0 || ul > 128 * 1024 || !v.valid_cluster(uf) {
            return Err(Error::Corrupt("upcase"));
        }
        let mut tab = vec![0u8; ul as usize];
        v.read_chain(dev, uf, false, 0, &mut tab)?;
        if table_checksum(&tab) != ucs {
            return Err(Error::UpcaseChecksum);
        }
        v.upcase = Upcase::decode(&tab);
        v.upcase_loc = (uf, ul);
        Ok(v)
    }

    pub fn label(&self) -> &str {
        &self.label
    }
    pub fn serial(&self) -> u32 {
        self.geo.serial
    }
    pub fn cluster_count(&self) -> u32 {
        self.geo.cluster_count
    }
    pub fn cluster_bytes(&self) -> u64 {
        1u64 << (self.geo.bps_shift + self.geo.spc_shift)
    }
    pub fn volume_bytes(&self) -> u64 {
        self.geo.volume_length << self.geo.bps_shift
    }
    pub fn upcase(&self) -> &Upcase {
        &self.upcase
    }

    pub fn root(&self) -> Node {
        Node {
            name: String::new(),
            units: Vec::new(),
            attrs: ATTR_DIR,
            first_cluster: self.geo.root_cluster,
            data_len: 0,
            valid_len: 0,
            contiguous: false,
            name_hash: 0,
            mtime: Stamp::default(),
            is_root: true,
        }
    }

    fn valid_cluster(&self, c: u32) -> bool {
        c >= 2 && c <= self.geo.cluster_count + 1
    }

    /// Device LBA of cluster `c`'s first sector (§3.1.8, §5.1).
    fn cluster_lba(&self, c: u32) -> u64 {
        let per = 1u64 << (self.geo.bps_shift - 9);
        let vs = self.geo.heap_offset as u64 + (((c - 2) as u64) << self.geo.spc_shift);
        self.geo.part_lba + vs * per
    }

    /// §4.1 FatEntry[c] in the active FAT.
    fn fat_entry<D: SectorRead + ?Sized>(&self, dev: &D, c: u32, cache: &mut FatCache) -> Result<u32> {
        let fat_vs = self.geo.fat_offset as u64 + self.active as u64 * self.geo.fat_length as u64;
        let byte = (fat_vs << self.geo.bps_shift) + c as u64 * 4;
        let lba = self.geo.part_lba + byte / SECTOR as u64;
        if cache.lba != lba {
            dev.read_sectors(lba, &mut cache.buf)?;
            cache.lba = lba;
        }
        Ok(le32(&cache.buf, (byte % SECTOR as u64) as usize))
    }

    /// The cluster after `c` in a chain: arithmetic under NoFatChain, the FAT otherwise. `None` at
    /// the end-of-chain mark; a free, reserved or bad link is corruption.
    fn next_cluster<D: SectorRead + ?Sized>(&self, dev: &D, c: u32, contiguous: bool, cache: &mut FatCache) -> Result<Option<u32>> {
        if contiguous {
            let n = c + 1;
            return if self.valid_cluster(n) { Ok(Some(n)) } else { Err(Error::Corrupt("extent")) };
        }
        match self.fat_entry(dev, c, cache)? {
            0xFFFF_FFFF => Ok(None),
            n if self.valid_cluster(n) => Ok(Some(n)),
            _ => Err(Error::Corrupt("chain")),
        }
    }

    /// Read `out.len()` bytes at byte `off` of the chain that starts at `first`. Adjacent clusters
    /// are coalesced into one device read (up to [`MAX_RUN_BYTES`]).
    fn read_chain<D: SectorRead + ?Sized>(&self, dev: &D, first: u32, contiguous: bool, off: u64, out: &mut [u8]) -> Result<()> {
        if out.is_empty() {
            return Ok(());
        }
        if !self.valid_cluster(first) {
            return Err(Error::Corrupt("first-cluster"));
        }
        let cb = self.cluster_bytes();
        let end = off + out.len() as u64;
        let mut cache = FatCache::new();
        // Walk to the cluster holding `off`.
        let mut cur = first;
        let mut ci = off / cb;
        if contiguous {
            cur = first.checked_add(ci as u32).filter(|&c| self.valid_cluster(c)).ok_or(Error::Corrupt("extent"))?;
        } else {
            for _ in 0..ci {
                cur = self.next_cluster(dev, cur, false, &mut cache)?.ok_or(Error::Corrupt("short-chain"))?;
            }
        }
        let max_run = (MAX_RUN_BYTES / cb).max(1);
        let mut pos = off;
        loop {
            let run_c0 = cur;
            let run_ci0 = ci;
            let mut run_n = 1u64;
            let mut have_next = false;
            while (run_ci0 + run_n) * cb < end {
                let prev = cur;
                cur = self.next_cluster(dev, prev, contiguous, &mut cache)?.ok_or(Error::Corrupt("short-chain"))?;
                ci += 1;
                if cur == prev + 1 && run_n < max_run {
                    run_n += 1;
                } else {
                    have_next = true;
                    break;
                }
            }
            let a = pos;
            let b = end.min((run_ci0 + run_n) * cb);
            let in_run = a - run_ci0 * cb;
            let s0 = in_run / SECTOR as u64;
            let s1 = (b - run_ci0 * cb).div_ceil(SECTOR as u64);
            let mut tmp = vec![0u8; ((s1 - s0) as usize) * SECTOR];
            dev.read_sectors(self.cluster_lba(run_c0) + s0, &mut tmp)?;
            let skip = (in_run - s0 * SECTOR as u64) as usize;
            let o = (a - off) as usize;
            let n = (b - a) as usize;
            out[o..o + n].copy_from_slice(&tmp[skip..skip + n]);
            pos = b;
            if pos >= end {
                return Ok(());
            }
            if !have_next {
                return Err(Error::Corrupt("short-chain"));
            }
        }
    }

    /// Every cluster of a FAT chain, bounded by the cluster count (a loop is corruption).
    pub fn chain_clusters<D: SectorRead + ?Sized>(&self, dev: &D, first: u32) -> Result<Vec<u32>> {
        let mut out = Vec::new();
        if !self.valid_cluster(first) {
            return Err(Error::Corrupt("first-cluster"));
        }
        let mut cache = FatCache::new();
        let mut c = first;
        loop {
            out.push(c);
            if out.len() as u64 > self.geo.cluster_count as u64 {
                return Err(Error::Corrupt("chain-loop"));
            }
            match self.next_cluster(dev, c, false, &mut cache)? {
                Some(n) => c = n,
                None => return Ok(out),
            }
        }
    }

    fn read_dir_bytes<D: SectorRead + ?Sized>(&self, dev: &D, dir: &Node) -> Result<Vec<u8>> {
        if !dir.is_dir() {
            return Err(Error::NotADirectory);
        }
        let len = if dir.is_root {
            self.chain_clusters(dev, dir.first_cluster)?.len() as u64 * self.cluster_bytes()
        } else {
            dir.data_len
        };
        if len == 0 {
            return Ok(Vec::new());
        }
        if len > MAX_DIR_BYTES || len % 32 != 0 {
            return Err(Error::Corrupt("dir-length"));
        }
        let mut buf = vec![0u8; len as usize];
        self.read_chain(dev, dir.first_cluster, dir.contiguous, 0, &mut buf)?;
        Ok(buf)
    }

    /// Read a directory's entry sets (§6.3, §7.4–§7.7). Sets that fail their checksum are counted
    /// and skipped; unknown benign secondaries are skipped by count; an unknown CRITICAL secondary
    /// makes its set unrecognised (skipped), as §6.3.2 directs.
    pub fn read_dir<D: SectorRead + ?Sized>(&self, dev: &D, dir: &Node) -> Result<Listing> {
        let raw = self.read_dir_bytes(dev, dir)?;
        let ents = raw.len() / 32;
        let mut out = Listing::default();
        let mut i = 0;
        while i < ents {
            let e = &raw[i * 32..i * 32 + 32];
            let t = e[0];
            if t == 0x00 {
                break; // end of directory
            }
            if t != 0x85 {
                i += 1; // unused/deleted (InUse clear), or a non-file primary
                continue;
            }
            let sc = e[1] as usize;
            if !(2..=18).contains(&sc) || i + sc >= ents {
                out.bad_sets += 1;
                i += 1;
                continue;
            }
            let set = &raw[i * 32..(i + 1 + sc) * 32];
            if set_checksum(set) != le16(e, 2) {
                out.bad_sets += 1;
                i += 1 + sc;
                continue;
            }
            let st = &set[32..64];
            if st[0] != 0xC0 {
                out.bad_sets += 1;
                i += 1 + sc;
                continue;
            }
            let nl = st[3] as usize;
            let need = nl.div_ceil(15);
            if nl == 0 || need + 1 > sc {
                out.bad_sets += 1;
                i += 1 + sc;
                continue;
            }
            let mut units = Vec::with_capacity(nl);
            let mut ok = true;
            for k in 0..need {
                let ne = &set[(2 + k) * 32..(3 + k) * 32];
                if ne[0] != 0xC1 {
                    ok = false;
                    break;
                }
                for j in 0..15 {
                    if units.len() < nl {
                        units.push(le16(ne, 2 + j * 2));
                    }
                }
            }
            // Further secondaries: benign (TypeImportance bit 0x20) are skippable; critical are not.
            for k in (2 + need)..=sc {
                let ty = set[k * 32];
                if ty & 0x80 != 0 && ty & 0x20 == 0 {
                    ok = false;
                }
            }
            if !ok {
                out.bad_sets += 1;
                i += 1 + sc;
                continue;
            }
            let hash = le16(st, 4);
            if self.upcase.hash(&units) != hash {
                out.bad_hash += 1;
            }
            let flags = st[1];
            out.nodes.push(Node {
                name: utf16_to_string(&units),
                units,
                attrs: le16(e, 4),
                first_cluster: le32(st, 20),
                data_len: le64(st, 24),
                valid_len: le64(st, 8),
                contiguous: flags & 2 != 0,
                name_hash: hash,
                mtime: Stamp::decode(le32(e, 12), e[21], e[23]),
                is_root: false,
            });
            i += 1 + sc;
        }
        Ok(out)
    }

    /// Resolve a volume-relative path (`""`, `"/"`, `"/a/b"`), case-insensitively by the volume's
    /// up-case table, NameHash first.
    pub fn lookup<D: SectorRead + ?Sized>(&self, dev: &D, path: &str) -> Result<Node> {
        let mut node = self.root();
        for comp in path.split('/').filter(|c| !c.is_empty() && *c != ".") {
            if !node.is_dir() {
                return Err(Error::NotADirectory);
            }
            let want: Vec<u16> = comp.encode_utf16().collect();
            let h = self.upcase.hash(&want);
            let list = self.read_dir(dev, &node)?;
            node = list
                .nodes
                .into_iter()
                .find(|n| n.name_hash == h && self.upcase.eq_ignore_case(&n.units, &want))
                .ok_or(Error::NotFound)?;
        }
        Ok(node)
    }

    /// Read up to `len` bytes of a file at `off`. Bytes past ValidDataLength read as zero (§7.6.5).
    pub fn read<D: SectorRead + ?Sized>(&self, dev: &D, node: &Node, off: u64, len: usize) -> Result<Vec<u8>> {
        if node.is_dir() {
            return Err(Error::IsADirectory);
        }
        if off >= node.data_len {
            return Ok(Vec::new());
        }
        let n = (len as u64).min(node.data_len - off) as usize;
        let mut out = vec![0u8; n];
        let valid = node.valid_len.min(node.data_len);
        if off < valid {
            let m = ((valid - off) as usize).min(n);
            self.read_chain(dev, node.first_cluster, node.contiguous, off, &mut out[..m])?;
        }
        Ok(out)
    }

    /// §7.1: is cluster `c` marked allocated in the active bitmap?
    pub fn allocated<D: SectorRead + ?Sized>(&self, dev: &D, c: u32) -> Result<bool> {
        if !self.valid_cluster(c) {
            return Ok(false);
        }
        let bit = (c - 2) as u64;
        let byte = bit / 8;
        let in_sec = (byte % SECTOR as u64) as usize;
        let base = byte - in_sec as u64;
        let n = (SECTOR as u64).min(self.bitmap_loc.1.saturating_sub(base)) as usize;
        if n <= in_sec {
            return Ok(false);
        }
        let mut sec = [0u8; SECTOR];
        self.read_chain(dev, self.bitmap_loc.0, false, base, &mut sec[..n])?;
        Ok(sec[in_sec] & (1 << (bit % 8)) != 0)
    }

    /// §7.1: the number of clusters the active bitmap marks allocated.
    pub fn used_clusters<D: SectorRead + ?Sized>(&self, dev: &D) -> Result<u64> {
        let cc = self.geo.cluster_count as u64;
        let len = cc.div_ceil(8);
        let mut used = 0u64;
        let chunk = 64 * 1024u64;
        let mut off = 0u64;
        let mut buf = vec![0u8; chunk as usize];
        while off < len {
            let n = chunk.min(len - off) as usize;
            self.read_chain(dev, self.bitmap_loc.0, false, off, &mut buf[..n])?;
            for (k, &b) in buf[..n].iter().enumerate() {
                let byte_ix = off + k as u64;
                let mut v = b;
                if byte_ix == len - 1 && cc % 8 != 0 {
                    v &= (1u8 << (cc % 8)) - 1; // bits past ClusterCount are not clusters
                }
                used += v.count_ones() as u64;
            }
            off += n as u64;
        }
        Ok(used)
    }

    /// The clusters a node occupies (FAT walk, or the NoFatChain extent).
    pub fn node_clusters<D: SectorRead + ?Sized>(&self, dev: &D, n: &Node) -> Result<Vec<u32>> {
        if n.first_cluster == 0 {
            return Ok(Vec::new());
        }
        if n.contiguous {
            let k = n.data_len.div_ceil(self.cluster_bytes());
            let last = n.first_cluster as u64 + k.saturating_sub(1);
            if !self.valid_cluster(n.first_cluster) || last > self.geo.cluster_count as u64 + 1 {
                return Err(Error::Corrupt("extent"));
            }
            return Ok((0..k as u32).map(|j| n.first_cluster + j).collect());
        }
        self.chain_clusters(dev, n.first_cluster)
    }

    /// The spec checklist, run against a mounted volume — the metal `tests exfat` and the host KATs
    /// call this one function, so the two read the same verdicts.
    pub fn audit<D: SectorRead + ?Sized>(&self, dev: &D) -> Audit {
        let mut a = Audit { label: self.label.clone(), ..Audit::default() };
        // 1. boot region checksum (verified at mount; the backup is a degraded pass).
        a.push("boot_csum", !self.backup_boot);
        // 2. up-case table: checksum verified at mount; a usable table maps at least a..z.
        a.push("upcase", (b'a'..=b'z').all(|c| self.upcase.up(c as u16) == (c - 32) as u16));
        // 3. the bitmap marks the structures the root directory names.
        let mut crit: Vec<u32> = Vec::new();
        let rootc = self.chain_clusters(dev, self.geo.root_cluster);
        if let Ok(r) = &rootc {
            crit.extend_from_slice(r);
        }
        for (f, _) in [self.bitmap_loc, self.upcase_loc] {
            if let Ok(cs) = self.chain_clusters(dev, f) {
                crit.extend(cs);
            }
        }
        a.push("bitmap_marks", rootc.is_ok() && crit.iter().all(|&c| self.allocated(dev, c).unwrap_or(false)));
        // 4. the used count is within the volume and agrees with PercentInUse when recorded.
        let used = self.used_clusters(dev);
        a.used_clusters = *used.as_ref().unwrap_or(&0);
        let pct_ok = match used {
            Ok(u) if u <= self.geo.cluster_count as u64 => {
                self.geo.percent_in_use == 0xFF || {
                    let p = (u * 100 / self.geo.cluster_count as u64) as i32;
                    (p - self.geo.percent_in_use as i32).abs() <= 1
                }
            }
            _ => false,
        };
        a.push("percent_in_use", pct_ok);
        // 5/6. root entry sets: checksums and name hashes.
        let list = self.read_dir(dev, &self.root());
        match &list {
            Ok(l) => {
                a.files = l.nodes.len();
                a.push("set_csum", l.bad_sets == 0);
                a.push("name_hash", l.bad_hash == 0);
            }
            Err(_) => {
                a.push("set_csum", false);
                a.push("name_hash", false);
            }
        }
        let nodes = list.map(|l| l.nodes).unwrap_or_default();
        // 7. every root entry's clusters: the chain is as long as DataLength says, and allocated.
        let cb = self.cluster_bytes();
        let chains_ok = nodes.iter().all(|n| match self.node_clusters(dev, n) {
            Ok(cs) => {
                let want = n.data_len.div_ceil(cb);
                let len_ok = if n.contiguous { true } else { cs.len() as u64 == want };
                len_ok && cs.iter().all(|&c| self.allocated(dev, c).unwrap_or(false))
            }
            Err(_) => false,
        });
        a.push("chains", chains_ok);
        // 8. the first non-empty file reads (up to one cluster) without error.
        let read_ok = match nodes.iter().find(|n| !n.is_dir() && n.data_len > 0) {
            Some(n) => self.read(dev, n, 0, cb.min(n.data_len) as usize).map(|v| !v.is_empty()).unwrap_or(false),
            None => true,
        };
        a.push("read", read_ok);
        a
    }
}

/// The checklist's verdicts — `kat=<passed>/<total>` on the wire.
#[derive(Debug, Clone, Default)]
pub struct Audit {
    pub checks: Vec<(&'static str, bool)>,
    pub label: String,
    /// Entries (files and directories) in the root directory.
    pub files: usize,
    pub used_clusters: u64,
}

impl Audit {
    fn push(&mut self, k: &'static str, ok: bool) {
        self.checks.push((k, ok));
    }
    pub fn passed(&self) -> usize {
        self.checks.iter().filter(|c| c.1).count()
    }
    pub fn total(&self) -> usize {
        self.checks.len()
    }
    pub fn first_fail(&self) -> Option<&'static str> {
        self.checks.iter().find(|c| !c.1).map(|c| c.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upcase_compressed_run() {
        // a..c map up, then `0xFFFF 3` skips d..f as identity, then g → G.
        let mut b = Vec::new();
        for u in [0x0000u16] {
            b.extend_from_slice(&u.to_le_bytes());
        }
        // units 0x00 identity, then compress a run to 'a' (0x61 - 1 = 0x60 units)
        b.extend_from_slice(&0xFFFFu16.to_le_bytes());
        b.extend_from_slice(&0x0060u16.to_le_bytes());
        for u in [0x41u16, 0x42, 0x43, 0xFFFF, 3, 0x47] {
            b.extend_from_slice(&u.to_le_bytes());
        }
        let t = Upcase::decode(&b);
        assert_eq!(t.up(b'a' as u16), b'A' as u16);
        assert_eq!(t.up(b'c' as u16), b'C' as u16);
        assert_eq!(t.up(b'd' as u16), b'd' as u16);
        assert_eq!(t.up(b'g' as u16), b'G' as u16);
        assert_eq!(t.up(b'z' as u16), b'z' as u16); // past the table's end: identity
        assert_eq!(t.mappings(), 4);
    }

    #[test]
    fn checksums_rotate_add() {
        // A byte stream hand-folded: c = ror(c) + b.
        assert_eq!(table_checksum(&[1, 2]), 0x8000_0000u32.wrapping_add(2));
        assert_eq!(set_checksum(&[1, 0, 0xAA, 0xBB, 2]), {
            let c: u16 = 1;
            c.rotate_right(1).wrapping_add(0).rotate_right(1).wrapping_add(2)
        });
        // Bytes 106, 107 and 112 never move the boot checksum.
        let mut r = vec![0x5Au8; 11 * 512];
        let c0 = boot_checksum(&r);
        r[106] = 1;
        r[107] = 2;
        r[112] = 3;
        assert_eq!(boot_checksum(&r), c0);
        r[105] = 0;
        assert_ne!(boot_checksum(&r), c0);
    }

    #[test]
    fn name_hash_is_case_blind() {
        let t = Upcase::ascii();
        let a: Vec<u16> = "ReadMe.txt".encode_utf16().collect();
        let b: Vec<u16> = "README.TXT".encode_utf16().collect();
        assert_eq!(t.hash(&a), t.hash(&b));
        assert!(t.eq_ignore_case(&a, &b));
    }

    #[test]
    fn utf16_names() {
        let s = "Ünï 日本 😀";
        let u: Vec<u16> = s.encode_utf16().collect();
        assert_eq!(utf16_to_string(&u), s);
        assert_eq!(utf16_to_string(&[0xD800, 0x41]), "\u{FFFD}A");
    }

    #[test]
    fn stamp_decode() {
        // 2026-10-06 13:45:58 + 10ms=150 → sec 59, UTC offset -4h (0x80 | -16 as 7-bit).
        let ts = ((2026 - 1980) << 25) | (10 << 21) | (6 << 16) | (13 << 11) | (45 << 5) | 29;
        let s = Stamp::decode(ts, 150, 0x80 | (0x80 - 16));
        assert_eq!((s.year, s.month, s.day, s.hour, s.min, s.sec), (2026, 10, 6, 13, 45, 59));
        assert_eq!(s.utc_offset_min, Some(-240));
        assert!(Stamp::decode(0, 0, 0).is_unset());
    }
}
