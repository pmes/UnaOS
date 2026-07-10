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

//! FAT16 / FAT32 reader (+ U9 in-place writer, + U10 grow/create allocator) built on the generic
//! block device.
//!
//! Handles both a **superfloppy** (the FAT BPB sits at LBA 0, no partition table) and an
//! **MBR-partitioned** disk (an MBR at LBA 0 whose partition entry points at the BPB). All
//! multi-byte on-disk fields are little-endian. Parsing is read-only — a mis-parse can at worst
//! report garbage, never corrupt a volume. The mutating entry points are [`FatFs::write_at`]
//! (U9: a bounded **in-place overwrite** — read-modify-writes only data sectors already in an
//! existing chain; never grows, allocates, frees, or touches the FAT/directory) and, since U10,
//! [`FatFs::write_grow`] (extend a file: allocate + zero-fill + chain new clusters, then bump the
//! directory `size`) and [`FatFs::create_in_root`] (add a fresh 0-length root-directory entry). The
//! U10 allocation invariants are FAT-safety-critical: **every FAT mutation writes ALL `num_fats` copies**
//! ([`FatFs::set_fat_entry`]); the free search is **bounded** and returns `NoSpace` when full
//! ([`FatFs::alloc_cluster`]); a new cluster is **zero-filled before it joins a chain**; and the
//! directory `size` (the reader's truth) is bumped **last**, so a crash mid-grow leaves a consistent
//! smaller file. FAT type is determined strictly by the data-cluster count per the Microsoft FAT
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
    /// U10: no free space — the free-cluster search found no free cluster (volume full), or a directory has
    /// no free slot for a new entry (root-directory-chain extension is out of scope). Surfaces as `-ENOSPC`.
    NoSpace,
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
    first_cluster: u32,
}

impl DirEntry {
    /// The 8.3 name as text (e.g. `"KERNEL.ELF"`).
    pub fn name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("?")
    }

    /// Case-insensitive match against an 8.3 name (short names are stored uppercase on disk, so
    /// `cat hello.txt` finds `HELLO.TXT`).
    fn eq_name(&self, other: &str) -> bool {
        let me = &self.name[..self.name_len as usize];
        me.len() == other.len()
            && me.iter().zip(other.bytes()).all(|(a, b)| a.eq_ignore_ascii_case(&b))
    }

    /// The entry's first data cluster — the chain head a `read_at` walk starts from (U6b: `SYS_OPEN`
    /// stores this in its per-task file-descriptor table so a later `SYS_READ` needs no re-scan of the
    /// directory). Read-only accessor; `0` for an empty/zero-length file (never a valid data cluster).
    pub fn first_cluster(&self) -> u32 {
        self.first_cluster
    }
}

/// A classified 32-byte directory slot: `End` (a 0x00 marker — stop scanning this directory), `Skip` (a
/// deleted 0xE5, long-file-name component, or volume-label slot — not a real entry), or `Entry` (a parsed
/// 8.3 file/dir entry). The single source of truth for the on-disk dirent format, so the read walkers
/// (`scan_dir_sector`) and the U10 write-side locator (`locate_in_*`) never diverge on how a slot is parsed.
enum DirSlot {
    End,
    Skip,
    Entry(DirEntry),
}

/// Classify one 32-byte directory slot per the FAT short-entry rules.
fn classify_dir_slot(e: &[u8]) -> DirSlot {
    match e[0] {
        0x00 => return DirSlot::End,  // no more entries in this directory
        0xE5 => return DirSlot::Skip, // deleted entry
        _ => {}
    }
    let attr = e[11];
    if attr & 0x0F == 0x0F {
        return DirSlot::Skip; // long-file-name component
    }
    if attr & 0x08 != 0 {
        return DirSlot::Skip; // volume label
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
    DirSlot::Entry(DirEntry {
        name,
        name_len: n as u8,
        is_dir: attr & 0x10 != 0,
        size: u32le(e, 28),
        first_cluster: ((u16le(e, 20) as u32) << 16) | u16le(e, 26) as u32,
    })
}

/// U10: one legal 8.3 character (upcased), or `None` if the byte cannot appear in a short name. The reserved
/// set (`" * + , . / : ; < = > ? [ \ ] |`), spaces, and control bytes are rejected; letters are upcased so a
/// created name matches how `classify_dir_slot` reads it back (short names are stored uppercase on disk).
fn to_upper_83(b: u8) -> Option<u8> {
    let up = b.to_ascii_uppercase();
    match up {
        b'A'..=b'Z'
        | b'0'..=b'9'
        | b'_'
        | b'-'
        | b'~'
        | b'!'
        | b'#'
        | b'$'
        | b'%'
        | b'&'
        | b'\''
        | b'('
        | b')'
        | b'@'
        | b'^'
        | b'{'
        | b'}' => Some(up),
        _ => None,
    }
}

/// U10: format an 8.3 name (e.g. `"FRESH.BIN"`) into the on-disk 11-byte space-padded field (`"FRESH   BIN"`),
/// or `None` if it is not a representable short name (LFN, subdirectory paths, and multi-dot names are out of
/// scope this arc). Base is 1..=8 chars, extension 0..=3, each a legal 8.3 char. The result re-parses (via
/// `classify_dir_slot`) to the same textual name, so a freshly created entry is found by the same `eq_name`.
fn format_83(name: &str) -> Option<[u8; 11]> {
    let mut out = [b' '; 11];
    let bytes = name.as_bytes();
    let (base, ext): (&[u8], &[u8]) = match name.find('.') {
        Some(i) => {
            let ext = &bytes[i + 1..];
            // Reject a trailing dot ("FILE.") or a second dot ("A.B.C"): neither is a distinct 8.3 name.
            // "FILE." would store as "FILE" and never re-parse (classify_dir_slot) back to "FILE.", so a later
            // find_located("FILE.") would miss it and create_in_root would write a DUPLICATE "FILE" entry.
            if ext.is_empty() || ext.contains(&b'.') {
                return None;
            }
            (&bytes[..i], ext)
        }
        None => (bytes, &[][..]),
    };
    if base.is_empty() || base.len() > 8 || ext.len() > 3 {
        return None;
    }
    for (k, &b) in base.iter().enumerate() {
        out[k] = to_upper_83(b)?;
    }
    for (k, &b) in ext.iter().enumerate() {
        out[8 + k] = to_upper_83(b)?;
    }
    Some(out)
}

/// Parse one 512-byte directory sector, appending real file/dir entries to `out`. Returns `true`
/// if a 0x00 (end-of-directory) marker was reached, telling the caller to stop scanning.
fn scan_dir_sector(sec: &[u8; SECTOR_SIZE], out: &mut alloc::vec::Vec<DirEntry>) -> bool {
    for i in 0..(SECTOR_SIZE / 32) {
        match classify_dir_slot(&sec[i * 32..i * 32 + 32]) {
            DirSlot::End => return true,
            DirSlot::Skip => continue,
            DirSlot::Entry(de) => out.push(de),
        }
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

#[inline]
fn u64le(b: &[u8], off: usize) -> u64 {
    (u32le(b, off) as u64) | ((u32le(b, off + 4) as u64) << 32)
}

/// Read one 512-byte sector at absolute `lba` into `buf`. Treats a short copy as I/O error, so
/// callers can assume a full sector on success.
fn read_sector(lba: u64, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), FatError> {
    match crate::drivers::block::read_block(lba, buf) {
        Ok(n) if n >= SECTOR_SIZE => Ok(()),
        _ => Err(FatError::Io),
    }
}

/// U9: write one full 512-byte sector at absolute `lba` from `buf`. The write half of `read_sector`; the
/// caller supplies a whole sector (a read-modify-write already merged the changed bytes). Any block error
/// is `FatError::Io`. Used ONLY by `write_at`, which passes only LBAs it walked out of an existing chain.
fn write_sector(lba: u64, buf: &[u8; SECTOR_SIZE]) -> Result<(), FatError> {
    match crate::drivers::block::write_block(lba, buf) {
        Ok(()) => Ok(()),
        Err(_) => Err(FatError::Io),
    }
}

/// F2 (SMP-hardening): the FAT-table mutation lock. Serializes the read-modify-write of a FAT sector so two
/// cores mutating entries that fall in the SAME sector cannot interleave read/write and lose an update — and
/// so the mirrored FAT copies never diverge under concurrency. See [`with_fat_lock`] for the lock-span and the
/// arch reasoning; the flag it closes is the U11-M2b reaper's downstream `set_fat_entry` RMW (docs/MILESTONES).
///
/// aarch64-only on purpose: the aarch64 storage path is fully POLLED (an emmc2 busy-poll on metal; a spin-loop
/// under the QEMU raspi4b/virt models, which deliver no timer IRQ so `hlt` degrades to a spin), so the lock can
/// span the bounded RMW block I/O without ever yielding the scheduler. x86 is EXCLUDED because its FAT path
/// `hlt`s awaiting an async xHCI transfer event and a `hlt` under a cleared IF never wakes — masking IRQs
/// across it would hang; the x86 side carries its own U11x concurrency model in `arch/x86_64` regardless.
#[cfg(target_arch = "aarch64")]
static FAT_MUTATION: spin::Mutex<()> = spin::Mutex::new(());

/// Run `f` under the FAT-table mutation lock (aarch64), or unchanged (other arches).
///
/// IRQ-masked via `arch::without_interrupts` so the hold is NON-PREEMPTIBLE: aarch64 EL0 syscalls run
/// I-unmasked and the U11-M2b reaper task is preemptible, so a metal timer preempt of a lock holder followed
/// by a re-entry into the FAT writer on that same core would deadlock (run queues never migrate → the
/// preempted holder never releases). Masking makes `FAT_MUTATION` a proper IRQ-safe spinlock at any core
/// count — the exact discipline the reaper's `IrqGuard` established. `without_interrupts` SAVES and restores
/// DAIF, so this nests correctly inside an already-masked caller (it never blindly unmasks).
///
/// LOCK SPAN: callers hold this ONLY across a single FAT-sector RMW (`set_fat_entry`'s bounded `num_fats`
/// read+write loop) — never across a free-search (`alloc_cluster`), a data-cluster zero-fill/write loop, or a
/// `mount()`. That is why "held across block I/O" is safe here: the aarch64 I/O is polled, so the span is a
/// couple of bounded polled sector transfers with no scheduler yield, not an unbounded wait.
#[cfg(target_arch = "aarch64")]
#[inline]
fn with_fat_lock<R>(f: impl FnOnce() -> R) -> R {
    crate::arch::without_interrupts(|| {
        let _guard = FAT_MUTATION.lock();
        f()
    })
}

/// Non-aarch64 (x86): the FAT-mutation lock is inert — see [`FAT_MUTATION`] for why masking IRQs across the
/// x86 `hlt`-driven xHCI FAT path would hang. Byte-identical to the pre-F2 behaviour (a zero-cost passthrough).
#[cfg(not(target_arch = "aarch64"))]
#[inline(always)]
fn with_fat_lock<R>(f: impl FnOnce() -> R) -> R {
    f()
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

    // Cap FAT32 at its architectural maximum: cluster numbers are 28-bit and >= 0x0FFFFFF6 are
    // reserved for EOC/bad, so a valid volume has at most 0x0FFFFFF4 data clusters. This rejects a
    // corrupt BPB claiming a cluster count near u32::MAX and keeps every chain hop count far below
    // u32::MAX, so the `hops` loop guards below can never wrap. (FAT16 is already < 65525.)
    if kind == FatKind::Fat32 && count_of_clusters > 0x0FFF_FFF4 {
        return Err(FatError::NotFat);
    }

    // The FAT must be large enough to hold an entry (2 bytes FAT16 / 4 FAT32) for every cluster —
    // the 2 reserved entries plus count_of_clusters. A BPB whose data region implies more clusters
    // than its FAT can address is corrupt; reject it so fat_entry() can never index past the FAT.
    let entry_bytes: u64 = if kind == FatKind::Fat32 { 4 } else { 2 };
    if (count_of_clusters as u64 + 2) * entry_bytes > fat_sz as u64 * bytes_per_sec as u64 {
        return Err(FatError::NotFat);
    }

    // Consistency vs the physical device: the whole volume must fit on the disk. This is the final
    // gate that makes an MBR boot sector (or random data) passing as a superfloppy essentially
    // impossible.
    if part_lba.saturating_add(tot_sec as u64) > dev_blocks {
        return Err(FatError::NotFat);
    }
    // The block layer addresses sectors with a 32-bit LBA (SCSI READ(10)), so the whole volume must
    // live within the 32-bit LBA space. This guarantees every derived LBA (FAT / data / cluster)
    // fits in u32, so no LBA computation (e.g. cluster_lba) can overflow u64.
    if part_lba.saturating_add(tot_sec as u64) > u32::MAX as u64 + 1 {
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

/// Mount the FAT volume on the registered block device. Tries, in order: a superfloppy (BPB at
/// LBA 0), a GPT (an `EFI PART` header at LBA 1 → partition entry → BPB), then a classic MBR at
/// LBA 0 whose first FAT-typed partition entry points at the BPB.
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

    // 2) GPT: an "EFI PART" header at LBA 1 (LBA 0 is a protective MBR). Checked BEFORE the MBR scan
    //    because a GPT disk's protective MBR entry (type 0xEE, start LBA 1) would otherwise be
    //    misread as a classic partition and send us to parse a BPB at the GPT header sector.
    if let Ok(fs) = scan_gpt(dev_blocks) {
        return Ok(fs);
    }

    // 3) MBR-partitioned: 0x55AA signature + a partition table at offset 446. Scan the four
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

/// Look for a GUID Partition Table: a header at LBA 1 with the `EFI PART` signature, then walk its
/// partition entry array for the first entry whose start LBA holds a valid FAT BPB. Returns NotFat
/// if there is no GPT or no FAT partition. Read-only; the scan is bounded (≤128 entries), and only
/// entry sizes that divide a 512-byte sector (128 / 256) are handled so no entry straddles a read.
fn scan_gpt(dev_blocks: u64) -> Result<FatFs, FatError> {
    if dev_blocks < 3 {
        return Err(FatError::NotFat);
    }
    let mut hdr = [0u8; SECTOR_SIZE];
    read_sector(1, &mut hdr)?;
    if &hdr[0..8] != b"EFI PART" {
        return Err(FatError::NotFat);
    }
    let entries_lba = u64le(&hdr, 72);
    let num_entries = u32le(&hdr, 80);
    let entry_size = u32le(&hdr, 84);
    if !(entry_size == 128 || entry_size == 256) || entries_lba == 0 || entries_lba >= dev_blocks {
        return Err(FatError::NotFat);
    }
    let num = num_entries.min(128); // bound the scan regardless of a corrupt count
    let per_sec = SECTOR_SIZE as u32 / entry_size; // 4 or 2 — exact, no straddle
    let mut buf = [0u8; SECTOR_SIZE];
    let mut cur_sec = u64::MAX;
    for i in 0..num {
        let sec = entries_lba + (i / per_sec) as u64;
        if sec >= dev_blocks {
            break;
        }
        if sec != cur_sec {
            if read_sector(sec, &mut buf).is_err() {
                break;
            }
            cur_sec = sec;
        }
        let off = ((i % per_sec) * entry_size) as usize;
        // Unused entry: all-zero partition type GUID.
        if buf[off..off + 16].iter().all(|&b| b == 0) {
            continue;
        }
        let first_lba = u64le(&buf, off + 32);
        if first_lba == 0 || first_lba >= dev_blocks {
            continue;
        }
        let mut pbs = [0u8; SECTOR_SIZE];
        if read_sector(first_lba, &mut pbs).is_err() {
            continue;
        }
        if let Ok(fs) = parse_bpb(&pbs, first_lba, dev_blocks) {
            return Ok(fs);
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

    /// Read the FAT entry for `cluster` (the next cluster in the chain), from the FIRST FAT copy — the read
    /// walkers' single-copy accessor. Delegates to [`FatFs::fat_entry_copy`] so the FAT-offset math (and its
    /// out-of-region guard) lives in exactly one place.
    fn fat_entry(&self, cluster: u32) -> Result<u32, FatError> {
        self.fat_entry_copy(cluster, 0)
    }

    /// U10: read the FAT entry for `cluster` from a SPECIFIC FAT copy (`fat` in `0..num_fats`). A 2- or
    /// 4-byte entry never straddles a 512-byte sector boundary (2 and 4 both divide 512), so one sector read
    /// suffices. The multi-copy accessor: `fat_entry` (copy 0) is the read path; the launcher compares copies
    /// to prove every FAT mutation mirrored to all of them. `parse_bpb` already gates the FAT size against the
    /// cluster count, but re-check `sec < fat_sz` so a stray out-of-range cluster can never read a sector
    /// outside the FAT (defense in depth on untrusted media).
    pub fn fat_entry_copy(&self, cluster: u32, fat: u32) -> Result<u32, FatError> {
        if fat >= self.num_fats {
            return Err(FatError::BadChain);
        }
        let offset = match self.kind {
            FatKind::Fat16 => cluster as u64 * 2,
            FatKind::Fat32 => cluster as u64 * 4,
        };
        let sec = offset / SECTOR_SIZE as u64;
        if sec >= self.fat_sz as u64 {
            return Err(FatError::BadChain);
        }
        let within = (offset % SECTOR_SIZE as u64) as usize;
        let mut buf = [0u8; SECTOR_SIZE];
        read_sector(self.fat_start + fat as u64 * self.fat_sz as u64 + sec, &mut buf)?;
        Ok(match self.kind {
            FatKind::Fat16 => u16le(&buf, within) as u32,
            FatKind::Fat32 => u32le(&buf, within) & 0x0FFF_FFFF,
        })
    }

    /// Number of FAT copies (`num_fats`, usually 2 on FAT32). Public for the launcher's FAT-copy-agreement check.
    pub fn num_fats(&self) -> u32 {
        self.num_fats
    }

    /// Bytes per cluster (`sec_per_clus * bytes_per_sec`). Public for the U10 GROW launcher's chain-length
    /// check: a grow allocates `new_size.div_ceil(cluster_size)` clusters, which differs across the image
    /// layouts (512-B clusters on FAT32 superfloppy/MBR vs 2048-B on the FAT16 fixed-root image), so the
    /// launcher computes the expected chain length rather than assuming a fixed 2.
    pub fn cluster_size(&self) -> u32 {
        self.sec_per_clus * self.bytes_per_sec
    }

    /// The end-of-chain marker to write into a terminal cluster's FAT entry (`>= 0xFFF8` / `>= 0x0FFFFFF8`
    /// both read as EOC; write the canonical all-ones form).
    fn eoc_value(&self) -> u32 {
        match self.kind {
            FatKind::Fat16 => 0xFFFF,
            FatKind::Fat32 => 0x0FFF_FFFF,
        }
    }

    /// U10: write the FAT entry for `cluster` to `next` (a cluster number, or 0 for free / EOC for terminal) in
    /// EVERY FAT copy (`num_fats`). A one-FAT write is a corrupt volume, so this ALWAYS mirrors to all copies —
    /// the whole point of the primitive. Read-modify-write per copy so neighbouring entries in the sector are
    /// preserved; on FAT32 the reserved high 4 bits of the 32-bit slot are preserved per the Microsoft FAT spec.
    fn set_fat_entry(&self, cluster: u32, next: u32) -> Result<(), FatError> {
        let offset = match self.kind {
            FatKind::Fat16 => cluster as u64 * 2,
            FatKind::Fat32 => cluster as u64 * 4,
        };
        let sec = offset / SECTOR_SIZE as u64;
        if sec >= self.fat_sz as u64 {
            return Err(FatError::BadChain); // never index past the FAT region
        }
        let within = (offset % SECTOR_SIZE as u64) as usize;
        // F2: serialize the WHOLE all-copies RMW under `FAT_MUTATION`. A concurrent writer mutating a
        // different entry in this same sector (or the other FAT copy) can no longer read-before-our-write and
        // then clobber our update. The lock spans ONLY this bounded `num_fats` read-modify-write — the index
        // math above is pure and stays outside — so on aarch64 the hold is a couple of polled sector transfers
        // with no scheduler yield (see `with_fat_lock`). On x86 `with_fat_lock` is a zero-cost passthrough.
        with_fat_lock(|| {
            let mut buf = [0u8; SECTOR_SIZE];
            for f in 0..self.num_fats as u64 {
                let lba = self.fat_start + f * self.fat_sz as u64 + sec;
                read_sector(lba, &mut buf)?;
                match self.kind {
                    FatKind::Fat16 => {
                        let v = (next & 0xFFFF) as u16;
                        buf[within..within + 2].copy_from_slice(&v.to_le_bytes());
                    }
                    FatKind::Fat32 => {
                        let existing = u32le(&buf, within);
                        let v = (existing & 0xF000_0000) | (next & 0x0FFF_FFFF);
                        buf[within..within + 4].copy_from_slice(&v.to_le_bytes());
                    }
                }
                write_sector(lba, &buf)?;
            }
            Ok(())
        })
    }

    /// U10: zero-fill every sector of a data cluster. Called BEFORE a freshly allocated cluster joins a chain,
    /// so no stale bytes from a previously-freed file can leak into a grown/created region (an information-
    /// disclosure invariant).
    fn zero_cluster(&self, cluster: u32) -> Result<(), FatError> {
        if !self.valid_cluster(cluster) {
            return Err(FatError::BadChain);
        }
        let zeros = [0u8; SECTOR_SIZE];
        for s in 0..self.sec_per_clus as u64 {
            write_sector(self.cluster_lba(cluster) + s, &zeros)?;
        }
        Ok(())
    }

    /// U10: allocate one data cluster — a bounded first-fit free search over `[2, count_of_clusters + 2)`,
    /// then zero-fill it and mark it EOC in all FAT copies, returning its number READY TO LINK (a terminated
    /// 1-cluster orphan until the caller links it onto a chain). NEVER spins (each FAT sector is read at most
    /// once) and NEVER returns a reserved/bad/out-of-range cluster — only one whose FAT entry reads `0` (free).
    /// `-ENOSPC` (`NoSpace`) when the volume is full. Zero-fill precedes any linkage (see [`FatFs::zero_cluster`]).
    fn alloc_cluster(&self) -> Result<u32, FatError> {
        let entry_bytes: u64 = if self.kind == FatKind::Fat32 { 4 } else { 2 };
        let last = self.count_of_clusters + 2; // exclusive: valid data clusters are 2 ..= count+1
        let mut buf = [0u8; SECTOR_SIZE];
        let mut loaded = u64::MAX;
        let mut c = 2u32;
        while c < last {
            let offset = c as u64 * entry_bytes;
            let sec = offset / SECTOR_SIZE as u64;
            if sec >= self.fat_sz as u64 {
                break; // past the FAT region (defensive — parse_bpb already gates this)
            }
            if sec != loaded {
                read_sector(self.fat_start + sec, &mut buf)?;
                loaded = sec;
            }
            let within = (offset % SECTOR_SIZE as u64) as usize;
            let e = match self.kind {
                FatKind::Fat16 => u16le(&buf, within) as u32,
                FatKind::Fat32 => u32le(&buf, within) & 0x0FFF_FFFF,
            };
            if e == 0 {
                // Free. Zero-fill FIRST (no stale bytes when it joins a chain), then terminate it (EOC) so the
                // caller receives a valid 1-cluster chain to link onto the tail.
                self.zero_cluster(c)?;
                self.set_fat_entry(c, self.eoc_value())?;
                return Ok(c);
            }
            c += 1;
        }
        Err(FatError::NoSpace)
    }

    /// U10: every cluster in a file's chain, in order (empty for a 0-length / 0-cluster file). Bounded exactly
    /// as the read walkers (loop guard vs `count_of_clusters`). Public for the launcher's post-grow check and
    /// used by `write_grow` to find the chain tail.
    pub fn chain_clusters(&self, first_cluster: u32) -> Result<alloc::vec::Vec<u32>, FatError> {
        let mut out = alloc::vec::Vec::new();
        if first_cluster == 0 {
            return Ok(out); // a 0-length file owns no clusters
        }
        if !self.valid_cluster(first_cluster) {
            return Err(FatError::BadChain);
        }
        let mut c = first_cluster;
        let mut hops = 0u32;
        loop {
            if !self.valid_cluster(c) {
                return Err(FatError::BadChain);
            }
            out.push(c);
            let next = self.fat_entry(c)?;
            if self.is_eoc(next) {
                break;
            }
            if self.is_bad(next) || next < 2 {
                return Err(FatError::BadChain);
            }
            c = next;
            hops += 1;
            if hops > self.count_of_clusters {
                return Err(FatError::BadChain); // longer than the volume has clusters -> loop
            }
        }
        Ok(out)
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
            if hops > self.count_of_clusters {
                return Err(FatError::BadChain); // chain longer than the volume has clusters -> loop
            }
        }
    }

    /// Find a top-level entry by 8.3 name (case-insensitive).
    pub fn find_in_root(&self, name: &str) -> Result<DirEntry, FatError> {
        for de in self.read_root()? {
            if de.eq_name(name) {
                return Ok(de);
            }
        }
        Err(FatError::NotFound)
    }

    /// Read up to `max_bytes` of a file into `out` by following its cluster chain. Stops at
    /// `de.size`, `max_bytes`, or end-of-chain (whichever comes first). Guards against bad/free
    /// clusters and chain loops. Rejects a directory. A file whose chain ends before `de.size` (a
    /// malformed volume) yields a short read rather than an error — `out.len()` tells the caller.
    pub fn read_file(
        &self,
        de: &DirEntry,
        out: &mut alloc::vec::Vec<u8>,
        max_bytes: usize,
    ) -> Result<(), FatError> {
        if de.is_dir {
            return Err(FatError::IsDirectory);
        }
        let mut remaining = core::cmp::min(de.size as usize, max_bytes);
        if remaining == 0 {
            return Ok(()); // empty file (or nothing requested) — no clusters to read
        }
        if !self.valid_cluster(de.first_cluster) {
            return Err(FatError::BadChain);
        }
        let mut cluster = de.first_cluster;
        let mut hops = 0u32;
        let mut buf = [0u8; SECTOR_SIZE];
        'chain: loop {
            if !self.valid_cluster(cluster) {
                return Err(FatError::BadChain);
            }
            for s in 0..self.sec_per_clus as u64 {
                read_sector(self.cluster_lba(cluster) + s, &mut buf)?;
                let take = core::cmp::min(remaining, SECTOR_SIZE);
                out.extend_from_slice(&buf[..take]);
                remaining -= take;
                if remaining == 0 {
                    break 'chain;
                }
            }
            let next = self.fat_entry(cluster)?;
            if self.is_eoc(next) {
                break; // chain ended before de.size (malformed) — return the short read
            }
            if self.is_bad(next) || next < 2 {
                return Err(FatError::BadChain);
            }
            cluster = next;
            hops += 1;
            if hops > self.count_of_clusters {
                return Err(FatError::BadChain); // chain longer than the volume has clusters -> loop
            }
        }
        Ok(())
    }

    /// Read up to `max` bytes of a file into `out`, starting at byte offset `start` within the file,
    /// given the file's first cluster and byte size (the two fields U6b's `SYS_OPEN` records so a later
    /// `SYS_READ` need not re-scan the directory). Sequential-only: it walks the chain from the first
    /// cluster, skipping whole clusters/sectors up to `start`, then copies `min(max, size - start)`
    /// bytes. Read-only — never writes the FAT, directory, or data. Stops at `size`, `start + max`, or
    /// end-of-chain; guards against bad/free clusters and chain loops exactly as `read_file`.
    ///
    /// This is the offset-aware twin of `read_file`; `read_file` is deliberately left untouched (its
    /// M6g/U4 `load_program_into_slot` caller reads whole programs from offset 0 and must stay
    /// byte-identical). `read_at(fc, size, 0, out, max)` delivers the same bytes `read_file` would for a
    /// non-directory entry, so the two share no code by design, not by divergence.
    pub fn read_at(
        &self,
        first_cluster: u32,
        size: u32,
        start: u32,
        out: &mut alloc::vec::Vec<u8>,
        max: usize,
    ) -> Result<(), FatError> {
        if start >= size {
            return Ok(()); // at or past EOF — nothing to read (a legal 0-byte result)
        }
        // Total bytes to deliver: from `start` to the earlier of EOF and `start + max`. `start < size`
        // above, so `end > start` and `want >= 1` here.
        let end = core::cmp::min(size as usize, (start as usize).saturating_add(max));
        let mut want = end - start as usize;
        let mut skip = start as usize; // bytes still to skip before the first delivered byte
        if want == 0 {
            return Ok(()); // caller requested 0 bytes
        }
        if !self.valid_cluster(first_cluster) {
            return Err(FatError::BadChain);
        }
        let clus_bytes = self.sec_per_clus as usize * SECTOR_SIZE;
        let mut cluster = first_cluster;
        let mut hops = 0u32;
        let mut buf = [0u8; SECTOR_SIZE];
        'chain: loop {
            if !self.valid_cluster(cluster) {
                return Err(FatError::BadChain);
            }
            if skip >= clus_bytes {
                // The whole cluster is before `start` — skip it without touching the disk.
                skip -= clus_bytes;
            } else {
                for s in 0..self.sec_per_clus as u64 {
                    if skip >= SECTOR_SIZE {
                        skip -= SECTOR_SIZE; // this whole sector is still before `start`
                        continue;
                    }
                    read_sector(self.cluster_lba(cluster) + s, &mut buf)?;
                    let from = skip; // nonzero only on the first partially-skipped sector
                    skip = 0;
                    let take = core::cmp::min(want, SECTOR_SIZE - from);
                    out.extend_from_slice(&buf[from..from + take]);
                    want -= take;
                    if want == 0 {
                        break 'chain;
                    }
                }
            }
            let next = self.fat_entry(cluster)?;
            if self.is_eoc(next) {
                break; // chain ended before `end` (malformed vs `size`) — return the short read
            }
            if self.is_bad(next) || next < 2 {
                return Err(FatError::BadChain);
            }
            cluster = next;
            hops += 1;
            if hops > self.count_of_clusters {
                return Err(FatError::BadChain); // chain longer than the volume has clusters -> loop
            }
        }
        Ok(())
    }

    /// U9: overwrite up to `data.len()` bytes of a file **in place**, starting at byte offset `start` within
    /// the file, given the file's first cluster and byte `size` (the two fields `SYS_OPEN` records). The write
    /// half of `read_at` and its exact structural twin: it walks the cluster chain from the first cluster,
    /// skipping whole clusters/sectors up to `start`, then read-modify-writes each touched sector (read it,
    /// overwrite the `[start..)` slice, write it back). Returns the number of bytes written.
    ///
    /// STRICTLY BOUNDED — this is what makes it safe to call on a real volume:
    ///   * **Never grows a file**: the write is clamped to `min(size, start + data.len())`, so it never writes
    ///     past the file's current EOF. A `start >= size` write is a clean 0-byte no-op (never grows).
    ///   * **Never allocates/frees clusters**: it only visits clusters ALREADY in the chain; no FAT entry is
    ///     ever written (`fat_entry` is read-only here, exactly as in `read_at`).
    ///   * **Never mutates a directory**: the on-disk `size` and the directory entry are untouched — a caller
    ///     that re-`mount`s and `find_in_root`s the file sees the same size and chain head afterwards.
    /// Guards against bad/free clusters and chain loops exactly as `read_at`. A chain that ends before `size`
    /// (a malformed volume) yields a SHORT write (the returned count) rather than writing outside the chain.
    pub fn write_at(
        &self,
        first_cluster: u32,
        size: u32,
        start: u32,
        data: &[u8],
    ) -> Result<usize, FatError> {
        if start >= size {
            return Ok(0); // at or past EOF — nothing to overwrite (never grows the file)
        }
        // Total bytes to write: from `start` to the earlier of EOF and `start + data.len()`. `start < size`
        // above, so `end > start` whenever `data` is non-empty.
        let end = core::cmp::min(size as usize, (start as usize).saturating_add(data.len()));
        let mut want = end - start as usize; // bytes still to write; decremented as sectors are written
        let total = want;
        if want == 0 {
            return Ok(0); // empty source slice
        }
        if !self.valid_cluster(first_cluster) {
            return Err(FatError::BadChain);
        }
        let clus_bytes = self.sec_per_clus as usize * SECTOR_SIZE;
        let mut cluster = first_cluster;
        let mut hops = 0u32;
        let mut buf = [0u8; SECTOR_SIZE];
        let mut skip = start as usize; // bytes still to skip before the first written byte
        let mut src_off = 0usize; // bytes of `data` already written
        'chain: loop {
            if !self.valid_cluster(cluster) {
                return Err(FatError::BadChain);
            }
            if skip >= clus_bytes {
                // The whole cluster is before `start` — skip it without touching the disk.
                skip -= clus_bytes;
            } else {
                for s in 0..self.sec_per_clus as u64 {
                    if skip >= SECTOR_SIZE {
                        skip -= SECTOR_SIZE; // this whole sector is still before `start`
                        continue;
                    }
                    let lba = self.cluster_lba(cluster) + s;
                    let from = skip; // nonzero only on the first partially-skipped sector
                    skip = 0;
                    let take = core::cmp::min(want, SECTOR_SIZE - from);
                    // Read-modify-write: read the sector, overwrite [from..from+take], write it back. Even a
                    // full-sector overwrite reads first (uniform RMW — the brief's "read-modify-write the
                    // touched sectors"; a partial first/last sector's untouched bytes MUST be preserved).
                    read_sector(lba, &mut buf)?;
                    buf[from..from + take].copy_from_slice(&data[src_off..src_off + take]);
                    write_sector(lba, &buf)?;
                    src_off += take;
                    want -= take;
                    if want == 0 {
                        break 'chain;
                    }
                }
            }
            let next = self.fat_entry(cluster)?;
            if self.is_eoc(next) {
                break; // chain ended before `end` (malformed vs `size`) — return the short write
            }
            if self.is_bad(next) || next < 2 {
                return Err(FatError::BadChain);
            }
            cluster = next;
            hops += 1;
            if hops > self.count_of_clusters {
                return Err(FatError::BadChain); // chain longer than the volume has clusters -> loop
            }
        }
        Ok(total - want)
    }

    /// U10: like [`FatFs::find_in_root`] but also returns the on-disk LOCATION of the matched 8.3 entry — the
    /// absolute LBA of its directory sector and the byte offset of its 32-byte slot within that sector. That
    /// location is what [`FatFs::write_dir_entry_fields`] read-modify-writes to publish a grown `size` / a new
    /// `first_cluster`. Read-only; `NotFound` if the entry is absent.
    pub fn find_located(&self, name: &str) -> Result<(DirEntry, u64, usize), FatError> {
        match self.kind {
            FatKind::Fat32 => self.locate_in_dir_chain(self.root_cluster, name),
            FatKind::Fat16 => self.locate_in_fixed_root16(name),
        }
    }

    /// FAT16 fixed root directory: a contiguous run of sectors, no cluster chain. Returns the matched entry
    /// with its (LBA, slot-offset). Stops at the 0x00 end marker exactly as `read_fixed_root16`.
    fn locate_in_fixed_root16(&self, name: &str) -> Result<(DirEntry, u64, usize), FatError> {
        let mut buf = [0u8; SECTOR_SIZE];
        for s in 0..self.root_dir_sectors as u64 {
            let lba = self.root_dir_lba + s;
            read_sector(lba, &mut buf)?;
            for i in 0..(SECTOR_SIZE / 32) {
                match classify_dir_slot(&buf[i * 32..i * 32 + 32]) {
                    DirSlot::End => return Err(FatError::NotFound),
                    DirSlot::Skip => continue,
                    DirSlot::Entry(de) => {
                        if de.eq_name(name) {
                            return Ok((de, lba, i * 32));
                        }
                    }
                }
            }
        }
        Err(FatError::NotFound)
    }

    /// A directory stored as a cluster chain (the FAT32 root, or any subdirectory): the located twin of
    /// `read_dir_chain`. Same bounded walk + bad/free-cluster + loop guards.
    fn locate_in_dir_chain(&self, start: u32, name: &str) -> Result<(DirEntry, u64, usize), FatError> {
        let mut cluster = start;
        let mut hops = 0u32;
        let mut buf = [0u8; SECTOR_SIZE];
        loop {
            if !self.valid_cluster(cluster) {
                return Err(FatError::BadChain);
            }
            for s in 0..self.sec_per_clus as u64 {
                let lba = self.cluster_lba(cluster) + s;
                read_sector(lba, &mut buf)?;
                for i in 0..(SECTOR_SIZE / 32) {
                    match classify_dir_slot(&buf[i * 32..i * 32 + 32]) {
                        DirSlot::End => return Err(FatError::NotFound),
                        DirSlot::Skip => continue,
                        DirSlot::Entry(de) => {
                            if de.eq_name(name) {
                                return Ok((de, lba, i * 32));
                            }
                        }
                    }
                }
            }
            let next = self.fat_entry(cluster)?;
            if self.is_eoc(next) {
                return Err(FatError::NotFound);
            }
            if self.is_bad(next) || next < 2 {
                return Err(FatError::BadChain);
            }
            cluster = next;
            hops += 1;
            if hops > self.count_of_clusters {
                return Err(FatError::BadChain);
            }
        }
    }

    /// U10: publish a directory entry's `first_cluster` (bytes 20-21 hi, 26-27 lo) and `size` (bytes 28-31) at
    /// its on-disk location, read-modify-write so the rest of the 32-byte entry (name / attr / timestamps) is
    /// preserved. This is the LAST write of a grow or create — the directory `size` is the reader's source of
    /// truth, so bumping it only after the data + FAT are durable keeps a crash mid-grow consistent (the file
    /// never claims unwritten clusters).
    fn write_dir_entry_fields(
        &self,
        lba: u64,
        off: usize,
        first_cluster: u32,
        size: u32,
    ) -> Result<(), FatError> {
        if off + 32 > SECTOR_SIZE {
            return Err(FatError::Io); // a slot never straddles a sector; a bad offset is a caller bug
        }
        let mut buf = [0u8; SECTOR_SIZE];
        read_sector(lba, &mut buf)?;
        let hi = (first_cluster >> 16) as u16;
        let lo = (first_cluster & 0xFFFF) as u16;
        buf[off + 20..off + 22].copy_from_slice(&hi.to_le_bytes());
        buf[off + 26..off + 28].copy_from_slice(&lo.to_le_bytes());
        buf[off + 28..off + 32].copy_from_slice(&size.to_le_bytes());
        write_sector(lba, &buf)?;
        Ok(())
    }

    /// U10: WRITE with GROWTH — overwrite `data` at byte offset `start`, EXTENDING the file (allocating,
    /// zero-filling, and chaining new clusters, then bumping the directory `size`) when the write runs past the
    /// current EOF. The growth twin of `write_at`; the caller uses `write_at` for the pure in-place case (a
    /// write wholly within the current bytes) and this only when `start + data.len()` exceeds `size`. `start`
    /// must be `<= size` (the seek gate enforces it — there are no sparse holes). Returns
    /// `(bytes_written, new_size, new_first_cluster)`: the caller republishes size + chain-head into its
    /// descriptor, and this file's on-disk directory entry (`dir_lba`, `dir_off`) is already updated here.
    ///
    /// SAFE ORDER (crash-consistency + no information disclosure):
    ///   1. walk the existing chain (bounded) to find its tail;
    ///   2. for each new cluster needed: `alloc_cluster` (free-search + ZERO-FILL + EOC), then link the tail to
    ///      it — so a newly allocated cluster is always zero-filled BEFORE it joins the chain;
    ///   3. read-modify-write the `data` into the (now-existing) clusters;
    ///   4. LAST, publish the new `size` (+ `first_cluster` if the file had none) to the directory entry.
    /// A crash before step 4 leaves the OLD (smaller) size on disk — never a size that claims unwritten
    /// clusters. Every FAT mutation (`alloc_cluster`, the tail link) writes ALL FAT copies via `set_fat_entry`.
    pub fn write_grow(
        &self,
        first_cluster: u32,
        size: u32,
        dir_lba: u64,
        dir_off: usize,
        start: u32,
        data: &[u8],
    ) -> Result<(usize, u32, u32), FatError> {
        if data.is_empty() {
            return Ok((0, size, first_cluster));
        }
        // No sparse holes: the caller's seek keeps `start <= size`. Reject a hole defensively.
        if start > size {
            return Err(FatError::BadChain);
        }
        let clus_bytes = self.sec_per_clus as usize * SECTOR_SIZE;
        let end = (start as usize).checked_add(data.len()).ok_or(FatError::BadChain)?;
        if end > u32::MAX as usize {
            return Err(FatError::NoSpace); // a FAT file size is a 32-bit field — cannot exceed it
        }
        let new_size = core::cmp::max(size, end as u32);

        // 1. The existing chain, in order. Empty for a 0-length / 0-cluster file.
        let mut chain = self.chain_clusters(first_cluster)?;

        // 2. Clusters the file must span to hold `end` bytes (`end >= 1`), then append as needed. Each append
        //    allocates+zeroes+terminates a cluster, then links the current tail onto it (all FAT copies).
        let needed = (end + clus_bytes - 1) / clus_bytes; // ceil
        let mut new_first = first_cluster;
        while chain.len() < needed {
            let n = self.alloc_cluster()?; // free + zero + EOC — a terminated orphan ready to link
            match chain.last() {
                Some(&tail) => self.set_fat_entry(tail, n)?, // link old tail -> n
                None => new_first = n,                       // the file had no clusters; n is the head
            }
            chain.push(n);
        }

        // 3. RMW the data across the chain. `start <= size <= end`, and the chain now covers [0, needed*clus),
        //    so every byte in [start, end) maps to an existing cluster. A partial sector preserves its other
        //    bytes; a freshly allocated cluster's untouched bytes stay zero (from step 2's zero-fill).
        let mut written = 0usize;
        let mut pos = start as usize;
        let mut buf = [0u8; SECTOR_SIZE];
        while written < data.len() {
            let ci = pos / clus_bytes;
            let cluster = *chain.get(ci).ok_or(FatError::BadChain)?;
            let in_clus = pos % clus_bytes;
            let sec_in_clus = (in_clus / SECTOR_SIZE) as u64;
            let in_sec = in_clus % SECTOR_SIZE;
            let lba = self.cluster_lba(cluster) + sec_in_clus;
            let take = core::cmp::min(data.len() - written, SECTOR_SIZE - in_sec);
            read_sector(lba, &mut buf)?;
            buf[in_sec..in_sec + take].copy_from_slice(&data[written..written + take]);
            write_sector(lba, &buf)?;
            written += take;
            pos += take;
        }

        // 4. LAST: publish size (+ chain head if it changed) to the directory — data + FAT already durable.
        self.write_dir_entry_fields(dir_lba, dir_off, new_first, new_size)?;
        Ok((written, new_size, new_first))
    }

    /// U10: CREATE a new 8.3 entry in the ROOT directory — a 0-length file (`first_cluster = 0`, `size = 0`)
    /// with attribute `attr` (a plain file is `0x20`). Finds a free directory slot (a `0x00` end-of-directory or
    /// a `0xE5` deleted slot) and writes the 8.3 name + zeroed metadata there; the first `write_grow` of the
    /// 0-cluster file allocates its first cluster and sets `first_cluster`. Returns the parsed entry with its
    /// on-disk (LBA, slot-offset). `NoSpace` if the root directory has no free slot (extending the root-dir chain
    /// is out of scope); `Unsupported` if the name is not a representable short name. Allocates NO clusters and
    /// touches NO FAT — only the one directory sector. The caller must have confirmed the name is absent (this
    /// does not de-duplicate); a 0-length entry never aliases another file's data (it owns no clusters).
    pub fn create_in_root(&self, name: &str, attr: u8) -> Result<(DirEntry, u64, usize), FatError> {
        let raw = format_83(name).ok_or(FatError::Unsupported)?;
        let (lba, off) = self.find_free_root_slot()?;
        let mut buf = [0u8; SECTOR_SIZE];
        read_sector(lba, &mut buf)?;
        // Write a fresh 32-byte entry: 11-byte name, attr, everything else zero (NTRes/times = 0, first_cluster
        // hi@20 lo@26 = 0, size@28 = 0). Never set the volume-label bit — this is a file/dir entry.
        for b in buf[off..off + 32].iter_mut() {
            *b = 0;
        }
        buf[off..off + 11].copy_from_slice(&raw);
        buf[off + 11] = attr & !0x08;
        write_sector(lba, &buf)?;
        // Re-parse the slot we just wrote so the returned DirEntry is byte-for-byte what a reader would see.
        match classify_dir_slot(&buf[off..off + 32]) {
            DirSlot::Entry(de) => Ok((de, lba, off)),
            _ => Err(FatError::Io), // unreachable: we just wrote a valid non-empty, non-LFN entry
        }
    }

    /// U10: the on-disk location of the first FREE root-directory slot (a `0x00` end marker or a `0xE5` deleted
    /// slot). `NoSpace` if the root directory is full — extending the root-directory chain is out of scope this
    /// arc. Writing into the first `0x00` slot preserves the terminator (the slots after it stay `0x00`), and a
    /// `0xE5` slot is mid-directory, so either choice keeps the directory correctly terminated.
    fn find_free_root_slot(&self) -> Result<(u64, usize), FatError> {
        match self.kind {
            FatKind::Fat32 => self.free_slot_in_dir_chain(self.root_cluster),
            FatKind::Fat16 => self.free_slot_in_fixed_root16(),
        }
    }

    fn free_slot_in_fixed_root16(&self) -> Result<(u64, usize), FatError> {
        let mut buf = [0u8; SECTOR_SIZE];
        for s in 0..self.root_dir_sectors as u64 {
            let lba = self.root_dir_lba + s;
            read_sector(lba, &mut buf)?;
            for i in 0..(SECTOR_SIZE / 32) {
                let b0 = buf[i * 32];
                if b0 == 0x00 || b0 == 0xE5 {
                    return Ok((lba, i * 32));
                }
            }
        }
        Err(FatError::NoSpace) // fixed root full — no extension possible
    }

    fn free_slot_in_dir_chain(&self, start: u32) -> Result<(u64, usize), FatError> {
        let mut cluster = start;
        let mut hops = 0u32;
        let mut buf = [0u8; SECTOR_SIZE];
        loop {
            if !self.valid_cluster(cluster) {
                return Err(FatError::BadChain);
            }
            for s in 0..self.sec_per_clus as u64 {
                let lba = self.cluster_lba(cluster) + s;
                read_sector(lba, &mut buf)?;
                for i in 0..(SECTOR_SIZE / 32) {
                    let b0 = buf[i * 32];
                    if b0 == 0x00 || b0 == 0xE5 {
                        return Ok((lba, i * 32));
                    }
                }
            }
            let next = self.fat_entry(cluster)?;
            if self.is_eoc(next) {
                return Err(FatError::NoSpace); // root-dir chain full; extending it is out of scope
            }
            if self.is_bad(next) || next < 2 {
                return Err(FatError::BadChain);
            }
            cluster = next;
            hops += 1;
            if hops > self.count_of_clusters {
                return Err(FatError::BadChain);
            }
        }
    }

    /// U11-M2: mark a directory entry deleted (first byte -> `0xE5`) via RMW, preserving the rest of the sector.
    /// Factored out of `delete_located` so the cross-process unlink-defers-free path can make the NAME disappear
    /// immediately (a subsequent re-open is `-ENOENT`) while the cluster chain stays allocated until the last
    /// open handle closes. `dir_off` must address a whole 32-byte slot within the sector. This is the crash-
    /// safety-critical FIRST step of any delete: after it the file is gone from the directory, but its clusters
    /// are still marked used — a crash here loses clusters (benign, chkdsk-reclaimable), never aliases live data.
    pub fn mark_dir_deleted(&self, dir_lba: u64, dir_off: usize) -> Result<(), FatError> {
        if dir_off + 32 > SECTOR_SIZE {
            return Err(FatError::Io);
        }
        let mut buf = [0u8; SECTOR_SIZE];
        read_sector(dir_lba, &mut buf)?;
        buf[dir_off] = 0xE5;
        write_sector(dir_lba, &buf)?;
        Ok(())
    }

    /// U11-M2: free every cluster in a file's chain (each FAT entry -> `0`, in ALL FAT copies), returning the
    /// freed clusters. Factored out of `delete_located` so the deferred (cross-process) path can free the chain
    /// at the LAST close, separately in time from marking the name deleted. Collects the chain BEFORE freeing
    /// anything, so a bad chain aborts with nothing freed; freeing low-to-high keeps first-fit reuse
    /// deterministic. The caller MUST have already made the directory entry unreachable (`mark_dir_deleted`) —
    /// freeing a chain a live entry still points at would alias. A 0-length file (`first_cluster == 0`) owns no
    /// clusters, so this is a no-op returning the empty chain.
    pub fn free_chain(&self, first_cluster: u32) -> Result<alloc::vec::Vec<u32>, FatError> {
        let chain = self.chain_clusters(first_cluster)?;
        for &c in &chain {
            self.set_fat_entry(c, 0)?;
        }
        Ok(chain)
    }

    /// U10: DELETE a file whose directory entry is at (`dir_lba`, `dir_off`) and whose chain head is
    /// `first_cluster` — the IMMEDIATE (single-syscall) delete. Order is crash-safety-critical: mark the
    /// directory entry deleted (`0xE5`) FIRST, THEN free the cluster chain (every entry -> `0`, ALL FAT copies).
    /// A crash after the `0xE5` mark but before the chain is fully freed leaves the file GONE with some clusters
    /// still marked used (lost clusters — benign, reclaimable by chkdsk); it can NEVER leave a live directory
    /// entry pointing at freed (and possibly re-allocated) clusters, which would alias another file's data.
    /// Returns the freed clusters (for the launcher's re-allocatability check). A 0-length file
    /// (`first_cluster == 0`) frees nothing. U11-M2: this is now exactly `mark_dir_deleted` + `free_chain`; the
    /// cross-process defer path (`sys_unlink`) calls those two halves at DIFFERENT times (mark at unlink, free at
    /// the last close). Pre-validates the chain up front so the immediate path keeps its U10 "a bad chain aborts
    /// with nothing changed" contract byte-for-byte (the write order — dir `0xE5`, then free — is unchanged).
    pub fn delete_located(
        &self,
        dir_lba: u64,
        dir_off: usize,
        first_cluster: u32,
    ) -> Result<alloc::vec::Vec<u32>, FatError> {
        // Pre-validate the chain BEFORE any mutation so a bad chain aborts with nothing changed (the U10
        // immediate-path contract). `free_chain` re-walks it below; the extra walk is read-only.
        let _ = self.chain_clusters(first_cluster)?;
        self.mark_dir_deleted(dir_lba, dir_off)?;
        self.free_chain(first_cluster)
    }

    /// U10: the number of the first FREE data cluster (a bounded read-only first-fit scan), or `NoSpace` if the
    /// volume is full. Does NOT allocate — the peek twin of `alloc_cluster`'s search, for the launcher's
    /// re-allocatability proof (the cluster a just-deleted file used is free again == the first-free is unchanged).
    pub fn first_free_cluster(&self) -> Result<u32, FatError> {
        let entry_bytes: u64 = if self.kind == FatKind::Fat32 { 4 } else { 2 };
        let last = self.count_of_clusters + 2;
        let mut buf = [0u8; SECTOR_SIZE];
        let mut loaded = u64::MAX;
        let mut c = 2u32;
        while c < last {
            let offset = c as u64 * entry_bytes;
            let sec = offset / SECTOR_SIZE as u64;
            if sec >= self.fat_sz as u64 {
                break;
            }
            if sec != loaded {
                read_sector(self.fat_start + sec, &mut buf)?;
                loaded = sec;
            }
            let within = (offset % SECTOR_SIZE as u64) as usize;
            let e = match self.kind {
                FatKind::Fat16 => u16le(&buf, within) as u32,
                FatKind::Fat32 => u32le(&buf, within) & 0x0FFF_FFFF,
            };
            if e == 0 {
                return Ok(c);
            }
            c += 1;
        }
        Err(FatError::NoSpace)
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
                    // Demonstrate cat: dump the first small file found (headless evidence).
                    if let Some(de) = entries.iter().find(|d| !d.is_dir && d.size > 0 && d.size <= 512) {
                        let mut data = alloc::vec::Vec::new();
                        if fs.read_file(de, &mut data, 512).is_ok() {
                            serial_println!("FS: cat {} ({} bytes):", de.name(), de.size);
                            let text: String = data
                                .iter()
                                .filter_map(|&b| match b {
                                    b'\n' => Some('\n'),
                                    b'\r' => None,
                                    0x20..=0x7e => Some(b as char),
                                    _ => Some('.'),
                                })
                                .collect();
                            for line in text.split('\n') {
                                serial_println!("FS:  | {}", line);
                            }
                        }
                    }
                }
                Err(e) => serial_println!("FS: root directory read error ({:?})", e),
            }
        }
        Err(e) => serial_println!("FS: no FAT filesystem ({:?})", e),
    }
}
