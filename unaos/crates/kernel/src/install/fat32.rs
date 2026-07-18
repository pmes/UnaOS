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

/// The count of leading ESP sectors `format_esp` requires to be ZERO for its blank-precondition
/// optimization to hold: the reserved region + both FAT copies (an empty FAT is all-free = 0). The
/// engine's demo target is always blank, so it never zeroes; a REAL installer target (a possibly
/// non-blank microSD) must zero exactly this region — no more (the data area's free clusters may hold
/// stale bytes harmlessly) and no less (a stale FAT entry would forge an allocation). Same
/// `compute_fat_sz` math the formatter uses, so the two never diverge. Returns `TooSmall` if the ESP
/// cannot hold a FAT32 layout.
pub fn blank_region_sectors(esp_sectors: u64) -> Result<u64, InstallError> {
    if esp_sectors > u32::MAX as u64 {
        return Err(InstallError::TooSmall);
    }
    let tot_sec = esp_sectors as u32;
    if tot_sec <= RESERVED {
        return Err(InstallError::TooSmall);
    }
    let fat_sz = compute_fat_sz(tot_sec);
    Ok((RESERVED + NUM_FATS * fat_sz) as u64)
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

// ══════════════════════════════════════════════════════════════════════════════════════════════════
// INSTALL-2 — the real-payload tree writer (multi-file / multi-cluster / subdirectory), ADDITIVE to the
// single-file `write_payload_file` above (which the x86 engine witness still drives unchanged). This is
// the "copy the running system's real boot payload" path: the Orin installer mounts the USB boot stick's
// ESP, walks its directory tree, and mirrors it onto the freshly-formatted microSD ESP through this
// writer. Three capabilities `write_payload_file` lacked, all confined here:
//   * a running FREE-CLUSTER cursor, so many files (and subdirectories) allocate distinct chains;
//   * FAT chains that span MANY FAT sectors — the INSTALL-1 single-FAT-sector bound (≤125 clusters,
//     ~64 KiB) is LIFTED: `set_fat_run` read-modify-writes every FAT sector a run touches, in both FAT
//     copies (so a multi-MB kernel image links correctly), with the SAME verify discipline downstream
//     (the caller sha-extent-verifies every file the writer records);
//   * directory clusters built WHOLLY in memory then written once, so a possibly-stale data cluster on a
//     non-blank card never leaks bytes into a directory (the blank-precondition zero pass covers only the
//     reserved+FAT region; data clusters are not pre-zeroed).
// The tree is assumed small enough that each directory fits in one cluster (the boot ESP's
// root + EFI/ + EFI/BOOT/ layout does); a directory that would overflow one cluster is an honest error,
// not silent truncation.
// ══════════════════════════════════════════════════════════════════════════════════════════════════

/// Short-name directory-entry attribute bytes.
pub const ATTR_ARCHIVE: u8 = 0x20;
pub const ATTR_DIR: u8 = 0x10;

/// Directory entries that fit in one 512-byte cluster (16 × 32-byte slots).
pub const DIR_SLOTS_PER_CLUSTER: usize = SECTOR / 32;

/// A multi-file / multi-cluster / subdirectory writer over an `InstallTarget`, driven by the Orin
/// INSTALL-2 flow. Holds a running free-cluster cursor; the target must be a freshly `format_esp`'d ESP
/// (so allocation is a simple contiguous bump from cluster 3, and the reserved FAT entries are in place).
pub struct TreeWriter<'a, T: InstallTarget> {
    t: &'a mut T,
    geom: FatGeom,
    next_free: u32,
}

impl<'a, T: InstallTarget> TreeWriter<'a, T> {
    /// Bind a writer to a freshly-formatted ESP. Allocation starts at the first file cluster (3); the
    /// root directory (cluster 2) is filled by the caller and written via `write_dir_cluster`.
    pub fn new(t: &'a mut T, geom: FatGeom) -> Self {
        Self { t, geom, next_free: FIRST_FILE_CLUSTER }
    }

    /// The root directory's cluster (2).
    pub fn root_cluster(&self) -> u32 {
        ROOT_CLUSTER
    }

    /// Number of clusters allocated so far (for the install summary).
    pub fn clusters_used(&self) -> u32 {
        self.next_free - FIRST_FILE_CLUSTER
    }

    /// Allocate `n` contiguous data clusters, linking them as one EOC-terminated chain across whatever
    /// FAT sectors the run spans (both FAT copies). Returns the first cluster. `NoSpace` if the run would
    /// exceed the volume's cluster count.
    fn alloc_chain(&mut self, n: u32) -> Result<u32, InstallError> {
        if n == 0 {
            return Err(InstallError::BadArg);
        }
        let first = self.next_free;
        let last = first.checked_add(n - 1).ok_or(InstallError::NoSpace)?;
        // count_of_clusters counts data clusters starting at cluster 2, so the last valid cluster
        // number is count_of_clusters + 1.
        if last > self.geom.count_of_clusters + 1 {
            return Err(InstallError::NoSpace);
        }
        self.set_fat_run(first, n)?;
        self.next_free = last + 1;
        Ok(first)
    }

    /// Link clusters `[first .. first+n)` as a chain (each → next, last → EOC), RMW-ing each FAT sector
    /// the run touches ONCE per copy. This is the multi-FAT-sector extension: a run that crosses a
    /// 128-entry sector boundary updates every sector it lands in, so a payload of any size links
    /// correctly (INSTALL-1 refused anything past FAT sector 0).
    fn set_fat_run(&mut self, first: u32, n: u32) -> Result<(), InstallError> {
        const EPS: u32 = (SECTOR / 4) as u32; // FAT32 entries per sector = 128
        let last = first + n - 1;
        let mut c = first;
        while c <= last {
            let sec_idx = c / EPS; // FAT-sector index within one copy (volume-relative to fat_start)
            let sec_base = sec_idx * EPS; // first cluster number this sector holds
            let sec_end = sec_base + EPS - 1; // last cluster number this sector holds
            let hi = core::cmp::min(last, sec_end);
            for copy in 0..NUM_FATS {
                let abs = self.geom.abs(self.geom.fat_start + copy * self.geom.fat_sz + sec_idx);
                let mut buf = [0u8; SECTOR];
                self.t.read_sectors(abs, &mut buf)?;
                let mut cc = c;
                while cc <= hi {
                    let value = if cc == last { 0x0FFF_FFFFu32 } else { cc + 1 };
                    let o = ((cc - sec_base) * 4) as usize;
                    buf[o..o + 4].copy_from_slice(&value.to_le_bytes());
                    cc += 1;
                }
                self.t.write_sectors(abs, &buf)?;
            }
            c = hi + 1;
        }
        Ok(())
    }

    /// Absolute LBA of the first sector of `cluster`.
    fn cluster_lba(&self, cluster: u32) -> u64 {
        self.geom.cluster_lba(cluster)
    }

    /// Allocate one fresh cluster for a subdirectory. Its 512-byte content is written later (built in
    /// memory by the caller, then handed to `write_dir_cluster`).
    pub fn alloc_dir_cluster(&mut self) -> Result<u32, InstallError> {
        self.alloc_chain(1)
    }

    /// Write a directory's full 512-byte cluster buffer (built in memory so no stale bytes leak).
    pub fn write_dir_cluster(&mut self, cluster: u32, buf: &[u8; SECTOR]) -> Result<(), InstallError> {
        self.t.write_sectors(self.cluster_lba(cluster), buf)
    }

    /// Allocate a fresh contiguous chain and write `payload` across it, zero-padding the final sector.
    /// Returns `(first_cluster, extents)`; an empty payload stores no cluster (`(0, [])`). The extents
    /// are exactly what `super::verify_extents` re-reads and SHA-checks.
    pub fn write_file(&mut self, payload: &[u8]) -> Result<(u32, Vec<Extent>), InstallError> {
        if payload.is_empty() {
            return Ok((0, Vec::new()));
        }
        let n = ((payload.len() + SECTOR - 1) / SECTOR) as u32;
        let first = self.alloc_chain(n)?;
        let mut extents = Vec::with_capacity(n as usize);
        let mut off = 0usize;
        for i in 0..n {
            let take = core::cmp::min(SECTOR, payload.len() - off);
            let mut sec = [0u8; SECTOR];
            sec[..take].copy_from_slice(&payload[off..off + take]);
            let lba = self.cluster_lba(first + i);
            self.t.write_sectors(lba, &sec)?;
            extents.push(Extent { lba, len: take });
            off += take;
        }
        Ok((first, extents))
    }
}

/// Format one 32-byte 8.3 directory entry into `dir[slot]`. Handles the `.`/`..` self/parent entries a
/// subdirectory needs (their on-disk names are `.` and `..` space-padded, which the general 8.3 encoder
/// rejects). Returns false for an unrepresentable name or an out-of-range slot — the caller turns either
/// into an honest error rather than a silent corruption.
pub fn put_dir_entry(
    dir: &mut [u8; SECTOR],
    slot: usize,
    name: &str,
    attr: u8,
    first_cluster: u32,
    size: u32,
) -> bool {
    let raw = if name == "." {
        let mut r = [b' '; 11];
        r[0] = b'.';
        r
    } else if name == ".." {
        let mut r = [b' '; 11];
        r[0] = b'.';
        r[1] = b'.';
        r
    } else {
        match format_83(name) {
            Some(r) => r,
            None => return false,
        }
    };
    let o = slot * 32;
    if o + 32 > SECTOR {
        return false;
    }
    dir[o..o + 11].copy_from_slice(&raw);
    dir[o + 11] = attr;
    let hi = (first_cluster >> 16) as u16;
    let lo = (first_cluster & 0xFFFF) as u16;
    dir[o + 20..o + 22].copy_from_slice(&hi.to_le_bytes());
    dir[o + 26..o + 28].copy_from_slice(&lo.to_le_bytes());
    dir[o + 28..o + 32].copy_from_slice(&size.to_le_bytes());
    true
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
