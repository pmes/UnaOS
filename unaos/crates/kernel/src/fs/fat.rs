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
//! PI-FS-3: the read walkers parse VFAT **long filenames** (the 0x0F-attribute LFN component slots that
//! precede a short 8.3 entry) — accumulated across sector/cluster boundaries by [`LfnBuf`], checksum-
//! validated against the short entry, and decoded UTF-16→UTF-8 into `DirEntry`'s inline long-name buffer.
//! [`DirEntry::name`] returns the long name when present (else the 8.3 short name); `eq_name` matches
//! EITHER spelling. **Subdirectory traversal** to arbitrary depth is served by [`FatFs::read_dir`] (the
//! FAT16 fixed root, the FAT32 root cluster chain, and any subdirectory cluster chain all resolve through
//! one API — a directory's `first_cluster()` is the chain head). All of this is strictly read-only.
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
// aarch64: the F2 M3 witness counter. x86 + `witness`: the roster tripwire's overlap tallies.
#[cfg(any(target_arch = "aarch64", feature = "witness"))]
use core::sync::atomic::AtomicU32;

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
    /// PARTITION (GR9): a derived LBA fell outside THIS volume's own extent (`part_lba .. part_lba +
    /// vol_sectors`). Distinct from [`FatError::Io`] on purpose: `Io` means the medium refused a legal
    /// access, this means the filesystem asked for a sector that is not its to touch — a bug or a corrupt
    /// on-disk field, caught before it reached the medium. Never returned on a healthy volume, because
    /// [`parse_bpb`] rejects at mount time any BPB whose total-sector claim exceeds its partition.
    OutOfVolume,
    /// WEDGE-8 (F3): the storage driver is busy (the xHCI controller loan is held by another
    /// context) and this call refused to wait for it — a masked span must NEVER block on a driver
    /// lock (that wait is the F3 deadlock), and an unmasked one already waited its bounded budget.
    /// The RMW wrappers ([`with_fat_lock_src`]/[`with_dir_lock_src`]) retry it OUTSIDE the masked
    /// span; exhaustion surfaces as `-EAGAIN` — the caller may retry, nothing was mutated.
    Busy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FatKind {
    Fat16,
    Fat32,
}

/// PI-FS-3: the widest long name we carry inline. A VFAT LFN is at most 255 UTF-16 code units; its
/// UTF-8 encoding can be up to 3 bytes per BMP unit, so a fully-3-byte 255-unit name needs 765 bytes.
/// We store the decoded UTF-8 inline (keeps `DirEntry: Copy` and `name() -> &str` — no signature churn
/// on the presentation callers); a name whose UTF-8 would not fit falls back to its 8.3 short name.
const LNAME_MAX: usize = 768;

/// A parsed directory entry. Carries the on-disk short (8.3) name (uppercase, e.g. `KERNEL.ELF`) and,
/// when VFAT long-file-name (LFN) entries preceded it and validated (PI-FS-3), the decoded UTF-8 long
/// name. `name()` returns the long name when present, else the short; `eq_name` matches EITHER, so a
/// lookup by the long spelling OR the 8.3 short both resolve.
#[derive(Clone, Copy)]
pub struct DirEntry {
    name: [u8; 12], // 8.3 short name: "NAME.EXT", NUL-padded (max 8 + '.' + 3 = 12)
    name_len: u8,
    /// PI-FS-3: decoded UTF-8 long name; `lname_len == 0` means "no LFN — use the 8.3 short name".
    lname: [u8; LNAME_MAX],
    lname_len: u16,
    pub is_dir: bool,
    pub size: u32,
    first_cluster: u32,
    // JD16: raw FAT last-write timestamp, kept as the two packed on-disk words (time @0x16, date @0x18)
    // so read-side parsing costs nothing and existing callers stay byte-identical. Decoded on demand via
    // `mtime()`. An all-zero pair (host tools sometimes leave the field 0, and a kernel-created entry has
    // no RTC to stamp) means "no timestamp" — see `FatTimestamp::is_zero`.
    mtime_time: u16,
    mtime_date: u16,
}

/// A decoded FAT last-write timestamp (JD16). FAT stores the moment as two packed 16-bit words with
/// **2-second resolution and NO timezone** (the on-disk value is wall-clock local time as whatever tool
/// wrote it saw it; there is no stored UTC offset to correct by, so we present the packed fields
/// verbatim). The FAT epoch is **1980-01-01**; the packed year field is an offset from 1980. An all-zero
/// on-disk pair decodes to the sentinel year 1980 with month/day 0 — a value that never occurs in a real
/// stamp — so callers use `is_zero()` to render those honestly rather than printing a bogus date.
#[derive(Clone, Copy)]
pub struct FatTimestamp {
    pub year: u16, // full year, e.g. 2026 (1980 + packed offset)
    pub month: u8, // 1..=12 (0 only in the all-zero sentinel)
    pub day: u8,   // 1..=31 (0 only in the all-zero sentinel)
    pub hour: u8,  // 0..=23
    pub min: u8,   // 0..=59
    pub sec: u8,   // 0..=58, even (FAT stores seconds/2)
}

impl FatTimestamp {
    /// True when the on-disk stamp was all-zero — no meaningful last-write time was recorded (a host
    /// tool that left the field 0, or a kernel-written entry, which has no RTC to stamp — see §JD16).
    pub fn is_zero(&self) -> bool {
        self.month == 0 && self.day == 0
    }
}

impl DirEntry {
    /// The entry's short (8.3) name as text (e.g. `"KERNEL.ELF"`).
    fn short_name(&self) -> &str {
        core::str::from_utf8(&self.name[..self.name_len as usize]).unwrap_or("?")
    }

    /// The display name: the decoded VFAT long name when one is present (PI-FS-3), else the 8.3 short
    /// name. The long-name bytes are always valid UTF-8 (built by `LfnBuf::decode_into`), so the
    /// `unwrap_or` fallback to the short name never triggers in practice.
    pub fn name(&self) -> &str {
        if self.lname_len > 0 {
            core::str::from_utf8(&self.lname[..self.lname_len as usize]).unwrap_or_else(|_| self.short_name())
        } else {
            self.short_name()
        }
    }

    /// Case-insensitive match against EITHER the long name or the 8.3 short name. Short names are stored
    /// uppercase on disk (so `cat hello.txt` finds `HELLO.TXT`); with PI-FS-3 a lookup by the long
    /// spelling (`cat .fseventsd`) also resolves. The long compare is ASCII-case-insensitive byte-wise —
    /// exact for the ASCII names our callers use; non-ASCII long names match only by identical bytes.
    fn eq_name(&self, other: &str) -> bool {
        let short = &self.name[..self.name_len as usize];
        if short.len() == other.len()
            && short.iter().zip(other.bytes()).all(|(a, b)| a.eq_ignore_ascii_case(&b))
        {
            return true;
        }
        if self.lname_len > 0 {
            let long = &self.lname[..self.lname_len as usize];
            return long.len() == other.len()
                && long.iter().zip(other.bytes()).all(|(a, b)| a.eq_ignore_ascii_case(&b));
        }
        false
    }

    /// The entry's first data cluster — the chain head a `read_at` walk starts from (U6b: `SYS_OPEN`
    /// stores this in its per-task file-descriptor table so a later `SYS_READ` needs no re-scan of the
    /// directory). Read-only accessor; `0` for an empty/zero-length file (never a valid data cluster).
    pub fn first_cluster(&self) -> u32 {
        self.first_cluster
    }

    /// JD16: decode the entry's FAT last-write timestamp from its two packed on-disk words. Packing
    /// (FAT spec): the DATE word holds `year-1980` in bits 15..9, month (1..12) in bits 8..5, day
    /// (1..31) in bits 4..0; the TIME word holds hour in bits 15..11, minute in bits 10..5, and
    /// seconds/2 in bits 4..0 (hence the 2-second resolution). No timezone is stored. An all-zero pair
    /// decodes to the `is_zero()` sentinel (month/day 0).
    pub fn mtime(&self) -> FatTimestamp {
        let d = self.mtime_date;
        let t = self.mtime_time;
        FatTimestamp {
            year: 1980 + (d >> 9),
            month: ((d >> 5) & 0x0F) as u8,
            day: (d & 0x1F) as u8,
            hour: (t >> 11) as u8,
            min: ((t >> 5) & 0x3F) as u8,
            sec: ((t & 0x1F) * 2) as u8,
        }
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
        lname: [0u8; LNAME_MAX],
        lname_len: 0,
        is_dir: attr & 0x10 != 0,
        size: u32le(e, 28),
        first_cluster: ((u16le(e, 20) as u32) << 16) | u16le(e, 26) as u32,
        // JD16 read-side: last-write time @0x16 (22), last-write date @0x18 (24). Stored raw; decoded
        // by DirEntry::mtime(). Creation time (@0x0E/0x10) is left unread — mtime is what `ls -l` shows.
        mtime_time: u16le(e, 22),
        mtime_date: u16le(e, 24),
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

/// PI-FS-3: the VFAT long-file-name checksum — computed over a short entry's 11 on-disk name bytes
/// (Microsoft FAT spec). Every LFN component slot stores this checksum at offset 13; a component whose
/// checksum disagrees with the short entry it precedes is orphaned (from a deleted/renamed file) and is
/// discarded, so a stale LFN run can never mislabel a live short entry.
fn lfn_checksum(short11: &[u8; 11]) -> u8 {
    let mut sum: u8 = 0;
    for &b in short11.iter() {
        sum = (((sum & 1) << 7).wrapping_add(sum >> 1)).wrapping_add(b);
    }
    sum
}

/// PI-FS-3: accumulates the VFAT long-file-name component slots (attribute 0x0F) that physically PRECEDE
/// a short (8.3) entry, then decodes them into a UTF-8 long name once the short entry is reached. LFN
/// components appear in REVERSE order — the LAST component first, flagged by bit 6 (0x40) in its ordinal
/// byte — so we see the highest ordinal first and place each component's 13 UTF-16 units at
/// `(ordinal-1)*13`. A run is accepted only when it is contiguous (ordinals N..1 with no gap), every
/// component carries the same checksum, and that checksum matches the short entry's name field. Any
/// inconsistency (out-of-range ordinal, checksum split, non-descending sequence, a deleted/volume-label
/// slot interrupting the run) marks the buffer broken and the entry falls back to its 8.3 short name.
struct LfnBuf {
    /// Up to 20 components × 13 UTF-16 code units (VFAT caps a name at 255 chars → ≤ 20 components).
    units: [u16; 20 * 13],
    max_ord: usize, // highest ordinal seen (0 = no active run)
    prev_ord: usize, // last ordinal consumed, for the descend-by-one contiguity check
    checksum: u8,
    broken: bool,
}

impl LfnBuf {
    fn new() -> Self {
        LfnBuf { units: [0u16; 20 * 13], max_ord: 0, prev_ord: 0, checksum: 0, broken: false }
    }

    /// Drop any partially-accumulated run (a live short entry with no LFN, a deleted slot, a volume
    /// label, or an inconsistency all break the run).
    fn reset(&mut self) {
        self.max_ord = 0;
        self.prev_ord = 0;
        self.broken = false;
    }

    /// Consume one 0x0F long-name component slot `e` (32 bytes).
    fn push(&mut self, e: &[u8]) {
        let b0 = e[0];
        let is_last = b0 & 0x40 != 0;
        let ord = (b0 & 0x1F) as usize;
        let cksum = e[13];
        if ord == 0 || ord > 20 {
            self.broken = true; // impossible ordinal — poison the run
            return;
        }
        if is_last {
            // Start a fresh run: the last component (highest ordinal) leads the reversed sequence.
            self.reset();
            self.max_ord = ord;
            self.checksum = cksum;
            self.prev_ord = ord + 1; // so the contiguity check below accepts this first component
        }
        if self.max_ord == 0 {
            self.broken = true; // a non-last component with no active run — orphan
            return;
        }
        if cksum != self.checksum || ord + 1 != self.prev_ord {
            self.broken = true; // checksum split, or a gap / non-descending ordinal
            return;
        }
        self.prev_ord = ord;
        // The 13 UTF-16 units live at three disjoint spans within the slot (offsets 1, 14, 28).
        const OFFS: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
        let base = (ord - 1) * 13;
        for (k, &o) in OFFS.iter().enumerate() {
            self.units[base + k] = u16le(e, o);
        }
    }

    /// The short entry `short11` (its 11 on-disk name bytes) has been reached: if a complete, checksum-
    /// matching LFN run accumulated, decode it into `de`'s long name. Consumes (resets) the run either
    /// way, so the next entry starts clean.
    fn attach(&mut self, short11: &[u8; 11], de: &mut DirEntry) {
        let ok = self.max_ord != 0
            && !self.broken
            && self.prev_ord == 1 // descended contiguously all the way to component 1
            && lfn_checksum(short11) == self.checksum;
        if ok {
            self.decode_into(de);
        }
        self.reset();
    }

    /// Decode the accumulated UTF-16 units (up to the 0x0000 terminator) into `de.lname` as UTF-8. On a
    /// name whose UTF-8 would overflow `LNAME_MAX`, leaves `de` untouched (falls back to the 8.3 name).
    fn decode_into(&self, de: &mut DirEntry) {
        let total = self.max_ord * 13;
        let mut n = 0usize;
        while n < total && self.units[n] != 0x0000 {
            n += 1; // stop at the NUL terminator; trailing 0xFFFF padding sits beyond it
        }
        let mut buf = [0u8; LNAME_MAX];
        let mut len = 0usize;
        for ch in core::char::decode_utf16(self.units[..n].iter().copied()) {
            let c = ch.unwrap_or('\u{FFFD}');
            let mut tmp = [0u8; 4];
            let s = c.encode_utf8(&mut tmp);
            if len + s.len() > LNAME_MAX {
                return; // too long to carry inline — keep the 8.3 fallback
            }
            buf[len..len + s.len()].copy_from_slice(s.as_bytes());
            len += s.len();
        }
        if len == 0 {
            return;
        }
        de.lname = buf;
        de.lname_len = len as u16;
    }
}

/// Parse one 512-byte directory sector, appending real file/dir entries to `out`, threading the VFAT
/// long-name accumulator `lfn` across sector (and cluster) boundaries so a long name split across the
/// slot preceding a short entry is reassembled correctly. Returns `true` if a 0x00 (end-of-directory)
/// marker was reached, telling the caller to stop scanning. LFN component slots (attr 0x0F) feed `lfn`;
/// a deleted slot or volume label breaks any run in progress; each real short entry consumes the run.
fn scan_dir_sector(
    sec: &[u8; SECTOR_SIZE],
    out: &mut alloc::vec::Vec<DirEntry>,
    lfn: &mut LfnBuf,
) -> bool {
    for i in 0..(SECTOR_SIZE / 32) {
        let e = &sec[i * 32..i * 32 + 32];
        match e[0] {
            0x00 => return true, // end of directory
            0xE5 => {
                lfn.reset(); // a deleted slot (incl. a deleted LFN component) interrupts any run
                continue;
            }
            _ => {}
        }
        let attr = e[11];
        if attr & 0x0F == 0x0F {
            lfn.push(e); // long-name component
            continue;
        }
        if attr & 0x08 != 0 {
            lfn.reset(); // volume label — not a name, breaks a run
            continue;
        }
        match classify_dir_slot(e) {
            DirSlot::Entry(mut de) => {
                let mut short11 = [0u8; 11];
                short11.copy_from_slice(&e[0..11]);
                lfn.attach(&short11, &mut de);
                out.push(de);
            }
            _ => lfn.reset(),
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
    /// K1 M2.2: `BS_VolID`, the volume serial number the formatter stamped into the boot sector (offset 0x27 on
    /// FAT16, 0x43 on FAT32). Read-only; exposed via [`FatFs::volume_fingerprint`] as one half of the UNAFS.ATR
    /// volume binding (a FOREIGN volume or a REFORMAT — a different serial/cluster-count — is rejected, so its
    /// rows never attach to this volume; a full byte-for-byte clone preserves both and is NOT rejected — offline
    /// tampering is out of scope).
    vol_id: u32,
    /// PI-FS-5: `BS_VolLab`, the 11-byte volume label the formatter stamped into the boot sector (offset 0x2B on
    /// FAT16, 0x47 on FAT32). Read-only; space-padded on disk, surfaced trimmed by [`FatFs::label`] for `diskinfo`.
    /// A blank/`NO NAME    ` field renders as empty (the caller shows a `-` then).
    vol_label: [u8; 11],
    /// PIUSB-27: which block device every sector read of this volume routes to. `Default` for the
    /// globally-registered device (SD on the Pi); `Usb` for a read-only mount of the USB stick read
    /// straight through the xHCI controller.
    source: BlockSource,
    /// PARTITION (GR9): this volume's extent in sectors, measured from `part_lba`. It is the BPB's
    /// own `tot_sec`, and at mount time it was proven `<=` the containing partition's declared length
    /// (see [`parse_bpb`]), so `part_lba .. part_lba + vol_sectors` is a sub-range of the partition.
    /// Every sector access this file makes is checked against it by [`FatFs::in_extent`] — that check,
    /// not the arithmetic, is what makes a FAT on partition 1 incapable of reaching partition 2.
    vol_sectors: u64,
    /// PARTITION (GR9): the MBR primary slot this volume was mounted from (1..=4), or 0 for a volume
    /// that did not come from an MBR entry (a superfloppy at LBA 0, or a GPT partition). Carried for
    /// the mount witness and `describe()` only; nothing keys behaviour off it.
    part_slot: u8,
    /// PARTITION (GR9): the containing partition as the block layer describes it, when this volume
    /// was found through a partition entry (`None` for a superfloppy — there is no container). It is
    /// consulted by [`FatFs::in_extent`] as a SECOND, independent bound: `vol_sectors` is what the
    /// volume's own BPB claims, this is what the partition table claims, and the two are written by
    /// different tools at different times. Agreeing at mount time (`parse_bpb` refuses a BPB larger
    /// than its partition) does not make one redundant at access time — a derived address is checked
    /// against both, so a wrong `vol_sectors` cannot by itself let an access leave the partition.
    range: Option<crate::drivers::block::PartitionRange>,
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

/// PIUSB-27: which block device a `FatFs` reads through. `Default` is the globally-registered block device
/// (`crate::drivers::block::read_block` — the microSD on the Pi, the USB stick on x86/QEMU); `Usb` reads the
/// USB mass-storage stick DIRECTLY through the xHCI controller (`read_block_usb`), independent of the block
/// layer's backend selector — so on the Pi (where the SD backend owns the global device) the USB stick can be
/// mounted alongside the SD-hosted unafs volume.
///
/// USBFALL F3 (was PIUSB-27): a `Usb`-sourced mount is NO LONGER read-only — USB-WRITE routed `write_sector`'s
/// `Usb` arm to the verified BOT WRITE(10) path (`drivers::block::write_block_usb`), so FAT, directory and data
/// writes DO reach the stick. Two consequences the rest of this file now states explicitly rather than assuming
/// away: the per-source lock-span cost documented on [`with_fat_lock`] (a `Usb` RMW is held under masked IRQs
/// for up to the BOT deadline, not for a polled sector transfer), and USBFALL F1 in `drivers::block`, which
/// stops a missing SD backend from silently redirecting `Default` writes onto this same stick.
///
/// SDHC-4b: `Sdhc` reads the card in the machine's INTERNAL SD slot directly through the SDHCI driver
/// (`drivers::block::read_block_sdhc`), independent of the backend selector — so on x86 the internal card
/// can be mounted alongside the USB stick that IS the boot volume, without either of them moving.
/// **It is READ-ONLY, unconditionally, in this arc**: [`write_sector`] and [`write_sectors`] refuse it with
/// a one-shot witness. That is the same blanket refusal PIUSB-27 shipped for `Usb`, and it is here for the
/// reason set out at length in `flight_recorder.rs` §SINGLE FAT WRITER — [`with_fat_lock`] / [`with_dir_lock`]
/// are INERT on x86, so a second FAT/directory MUTATOR on this target is a proven corruption generator
/// (A/B-measured cross-linked chains and stolen delete-witness snapshots), and adding a second mountable
/// volume must not quietly recreate it. As a READER a `Sdhc` mount cannot interact with anything: it is a
/// separate by-value `FatFs` sharing no state with the boot volume's mount, and the one cross-volume global
/// in this file (`ALLOC_HINT`) is read only by the cluster ALLOCATOR, which a read-only mount never enters.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BlockSource {
    Default,
    Usb,
    /// SDHC-4b (x86, `sdhcblk` knob): the internal SD card, READ-ONLY. See the note above.
    #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
    Sdhc,
}

/// SDHC-4c: the internal SD card's write decision, in ONE place — superseding SDHC-4b's
/// `refuse_sdhc_write`, which returned `Unsupported` unconditionally from these same two call sites.
///
/// The seam is deliberately unchanged in shape: one function, both write entry points go through
/// it, one-shot witness, [`FatError::Unsupported`] on refusal (a refusal is policy, not a device
/// fault). What changed is the answer — it is now "yes IF the span lies inside the reserved
/// extent", which is a STRICTER statement than 4b's blanket no was a loose one: 4b refused at the
/// FAT layer and left `PartitionRange::write_block` as an unbounded route to the card, while this
/// bound is checked in absolute LBAs at the point the CMD24 is about to be issued.
///
/// See [`crate::fs::sdhc4c`] for the writable set and why it is closed.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
#[inline]
fn permit_sdhc_write(site: &str, lba: u64, count: u64) -> Result<(), FatError> {
    crate::fs::sdhc4c::permit_write(site, lba, count)
}

/// Read one 512-byte sector at absolute `lba` into `buf` from `source`. Treats a short copy as I/O error, so
/// callers can assume a full sector on success.
fn read_sector(source: BlockSource, lba: u64, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), FatError> {
    let r = match source {
        BlockSource::Default => crate::drivers::block::read_block(lba, buf),
        BlockSource::Usb => crate::drivers::block::read_block_usb(lba, buf),
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        BlockSource::Sdhc => crate::drivers::block::read_block_sdhc(lba, buf),
    };
    match r {
        Ok(n) if n >= SECTOR_SIZE => Ok(()),
        // WEDGE-8 (F3): Busy is not an I/O failure — the controller is loaned out and this context
        // refused (masked) or exhausted (unmasked) its wait. Kept distinct so the RMW wrappers can
        // retry outside the masked span and user mode sees `-EAGAIN`, never a false `-EIO`.
        Err(crate::drivers::block::BlockError::Busy) => Err(FatError::Busy),
        _ => Err(FatError::Io),
    }
}

/// U9: write one full 512-byte sector at absolute `lba` from `buf`. The write half of `read_sector`; the
/// caller supplies a whole sector (a read-modify-write already merged the changed bytes). Any block error
/// is `FatError::Io`. Used ONLY by `write_at`, which passes only LBAs it walked out of an existing chain.
/// USB-WRITE (supersedes PIUSB-27's blanket refusal): a `Usb`-sourced volume routes to the verified
/// BOT WRITE(10) path (`write_block_usb`, MISSION-gated with an RMW+restore witness); any source
/// without a verified write path would still be refused here.
fn write_sector(source: BlockSource, lba: u64, buf: &[u8; SECTOR_SIZE]) -> Result<(), FatError> {
    // SDHC-4c: the internal SD card admits a write ONLY inside the reserved extent. Checked BEFORE
    // the block call, so a refused write issues no CMD24 and takes no card lock, however the volume
    // was mounted. Unarmed (the default, and every failure of the reserve pass) => refuses exactly
    // as SDHC-4b did.
    #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
    if source == BlockSource::Sdhc {
        permit_sdhc_write("write_sector", lba, 1)?;
    }
    let r = match source {
        BlockSource::Default => crate::drivers::block::write_block(lba, buf),
        BlockSource::Usb => crate::drivers::block::write_block_usb(lba, buf),
        // Reachable ONLY with the permit armed and this exact LBA inside the reserved extent — the
        // guard above returned otherwise.
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        BlockSource::Sdhc => crate::drivers::block::write_block_sdhc(lba, buf),
    };
    r.map_err(|e| match e {
        // WEDGE-8 (F3): see `read_sector` — Busy stays Busy so it can be retried, not mourned.
        crate::drivers::block::BlockError::Busy => FatError::Busy,
        _ => FatError::Io,
    })
}

// ===================== MULTIBLK (2026-07-29) — counted sector runs =====================
//
// Every writer below this point used to be a per-sector read-modify-write loop, and
// usb_xhci.md §12.1 priced exactly what that costs on real hardware: one flight-recorder
// reservation is ~730 BOT transactions and ~1460 awaited USB completion events, because the driver
// could move 512 bytes per round trip and this file dutifully asked it to, one sector at a time.
// That amplification (mechanism M1) is not itself the wedge — mechanism M2, a LOST completion event,
// is, and it remains unexplained. What M1 does is multiply M2's per-transaction hazard by a thousand
// until it is certain to be hit. Cutting the transaction count is therefore the structural repair
// available to us while M2's cause is still open, and it shrinks the exposure proportionally.
//
// Two independent wins, and it is worth keeping them separate because they compound:
//   1. CONTIGUITY — sectors inside one cluster are always consecutive on disk, and consecutive
//      clusters are consecutive LBAs, so a run can be handed to the block layer as ONE counted
//      transfer instead of N.
//   2. NO READ ON A FULL-SECTOR OVERWRITE — the old loop read every sector before writing it, even
//      when the caller's data covered the whole sector. That read exists only to preserve the bytes
//      OUTSIDE the written range, so it is needed only for a partial head or tail sector. Dropping
//      it on the interior is a further 2x on every data write.
//
// The seam is deliberately narrow: `read_sector` / `write_sector` above are untouched and still
// serve every partial-sector RMW, every FAT-entry mutation and every directory-slot mutation, so
// those paths keep the exact shape they were audited in. Only whole-sector RUNS come through here.

/// MULTIBLK: where the next `alloc_cluster` free search STARTS. This is the in-memory equivalent of
/// FAT32's FSInfo `FSI_Nxt_Free` field, and it exists for the reason every real driver keeps one:
/// restarting the scan at cluster 2 for every allocation makes allocating a run of clusters
/// quadratic in FAT sector reads, and on a USB stick each of those reads is a full BOT transaction.
/// It is advisory ONLY — `alloc_cluster` validates it into range, wraps back to cluster 2 when it
/// reaches the end, and still claims under the F3-M1 compare-and-claim — so a stale value (including
/// one left by a different volume, since this is a single global) can cost an extra wrap and can
/// never cost correctness. Deliberately not persisted to FSInfo: writing that sector would be a new
/// on-disk mutation on the destructive path, which this arc does not take.
static ALLOC_HINT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(2);

/// MULTIBLK: read a run of whole sectors starting at absolute `lba` into `buf`, chunked against the
/// block layer's published `MAX_BLOCKS_PER_OP`. `buf.len()` must be a non-zero multiple of
/// [`SECTOR_SIZE`]; anything else is a caller bug and returns `Io` rather than a partial result.
fn read_sectors(source: BlockSource, lba: u64, buf: &mut [u8]) -> Result<(), FatError> {
    if buf.is_empty() || buf.len() % SECTOR_SIZE != 0 {
        return Err(FatError::Io);
    }
    let step = crate::drivers::block::MAX_BLOCKS_PER_OP * SECTOR_SIZE;
    let mut off = 0usize;
    while off < buf.len() {
        let take = core::cmp::min(step, buf.len() - off);
        let at = lba + (off / SECTOR_SIZE) as u64;
        let chunk = &mut buf[off..off + take];
        let r = match source {
            BlockSource::Default => crate::drivers::block::read_blocks(at, chunk),
            BlockSource::Usb => crate::drivers::block::read_blocks_usb(at, chunk),
            #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
            BlockSource::Sdhc => crate::drivers::block::read_blocks_sdhc(at, chunk),
        };
        match r {
            Ok(n) if n == take => {}
            _ => return Err(FatError::Io), // short read == error, exactly as `read_sector`
        }
        off += take;
    }
    Ok(())
}

/// MULTIBLK: write a run of whole sectors starting at absolute `lba` from `buf`, chunked against
/// `MAX_BLOCKS_PER_OP`. The write twin of [`read_sectors`], with the same whole-sector precondition.
/// Callers reach this ONLY for spans they have proven are fully covered by `buf`, which is what
/// makes it sound to issue the write with no preceding read.
fn write_sectors(source: BlockSource, lba: u64, buf: &[u8]) -> Result<(), FatError> {
    if buf.is_empty() || buf.len() % SECTOR_SIZE != 0 {
        return Err(FatError::Io);
    }
    // SDHC-4c: the WHOLE run is checked against the reserved extent before the loop starts, so a run
    // that is only partly inside it is refused ENTIRELY rather than half-written and then stopped.
    // Each chunk is then re-checked inside the loop against the same predicate — the run check is
    // the atomicity property, the per-chunk check is the bound, and neither is derived from the
    // other. See `write_sector`.
    #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
    if source == BlockSource::Sdhc {
        crate::fs::sdhc4c::permit_span(
            "write_sectors(run)",
            lba,
            (buf.len() / SECTOR_SIZE) as u64,
        )?;
    }
    let step = crate::drivers::block::MAX_BLOCKS_PER_OP * SECTOR_SIZE;
    let mut off = 0usize;
    while off < buf.len() {
        let take = core::cmp::min(step, buf.len() - off);
        let at = lba + (off / SECTOR_SIZE) as u64;
        let chunk = &buf[off..off + take];
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        if source == BlockSource::Sdhc {
            permit_sdhc_write("write_sectors", at, (take / SECTOR_SIZE) as u64)?;
        }
        let r = match source {
            BlockSource::Default => crate::drivers::block::write_blocks(at, chunk),
            BlockSource::Usb => crate::drivers::block::write_blocks_usb(at, chunk),
            // Reachable ONLY with the permit armed and this chunk inside the reserved extent.
            #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
            BlockSource::Sdhc => crate::drivers::block::write_blocks_sdhc(at, chunk),
        };
        r.map_err(|_| FatError::Io)?;
        off += take;
    }
    Ok(())
}

/// F2 (SMP-hardening): the FAT-table mutation lock. Serializes the read-modify-write of a FAT sector so two
/// cores mutating entries that fall in the SAME sector cannot interleave read/write and lose an update — and
/// so the mirrored FAT copies never diverge under concurrency. See [`with_fat_lock`] for the lock-span and the
/// arch reasoning; the flag it closes is the U11-M2b reaper's downstream `set_fat_entry` RMW (docs/MILESTONES).
///
/// aarch64-only on purpose. ⚠ PH-3 (2026-07-27) CORRECTED the reason: this comment previously claimed "the
/// aarch64 storage path is fully POLLED (emmc2)". That is NOT an invariant of any aarch64 build — the xHCI
/// BOT path IS reachable under this lock. `with_fat_lock` is gated on `target_arch` alone (no `baremetal`),
/// while `block::write_block`/`read_block` route to `emmc2` only under `cfg(baremetal)` AND only once the
/// runtime `BACKEND` atomic has been flipped by `emmc2::finish` → `block::register_sd`. So on aarch64-virt
/// (no `emmc2` compiled at all) BOT is the ONLY path, and on Pi metal a failed card init leaves BACKEND_XHCI
/// with a later-enumerated USB stick owning `BLOCK_DEVICE` (`block::publish_usb_geometry`).
///
/// THE REAL INVARIANT that makes the IRQ-masked span safe on aarch64 — it holds for BOTH backends:
///   (a) emmc2 is a bounded busy-poll; and
///   (b) the xHCI BOT pump (`xhci::pump_until_bot_done`) drains its event ring by POLLING, never from an IRQ
///       handler, and is bounded by a `now_cycles`/`hw_wait_budget` WALL-CLOCK deadline (CNTVCT keeps
///       advancing with the timer masked); its idle step is `arch::hlt`, whose `wfi` WAKES ON A PENDING
///       PHYSICAL INTERRUPT EVEN WITH PSTATE.I SET, and which is never a scheduler yield.
/// x86 is EXCLUDED because its `hlt` under a cleared IF never wakes — masking IRQs across it would hang; the
/// x86 side carries its own U11x concurrency model in `arch/x86_64` regardless. Full traced citation chain:
/// `docs/dev/OS/01_BOOT_HAL/arch_arm64.md` ("PH-3 — is the aarch64 block-write path 'fully polled emmc2'?").
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
/// `mount()`. That structural rule is unchanged; what the span COSTS, however, is a property of the
/// [`BlockSource`] the RMW runs through, and USBFALL F2 states it per-source rather than as one blanket claim:
///
/// - [`BlockSource::Default`] — the pre-USB-WRITE premise, and still true. On Pi bare-metal the backend is the
///   microSD (`emmc2`, a polled CMD17/CMD24 busy-poll; USBFALL F1 now REFUSES a `Default` write when no SD
///   registered, so this arm can no longer be silently substituted by the stick). Under the QEMU raspi4b/virt
///   models the spin-loop degrades to a spin. Either way: a couple of bounded polled sector transfers, no
///   scheduler yield, microsecond-to-millisecond scale.
/// - [`BlockSource::Usb`] — NOT "a couple of bounded polled sector transfers". USB-WRITE made this source
///   writable (`write_sector` → `drivers::block::write_block_usb` → `xhci.storage_write10` → `scsi_write10` →
///   `bot_transfer` → `pump_until_bot_done`), whose wall-clock deadline is `arch::hw_wait_budget() * 3`
///   = 450_000_000 CNTVCT ticks (~8 s at the Pi's 54 MHz CNTFRQ), and whose pump body calls `crate::hlt()` —
///   i.e. a `wfi` executed with `PSTATE.I` MASKED by this very hold. It is bounded and it is NOT a deadlock
///   (WFI wakes on a pending physical interrupt even with I set — `arch/aarch64/mod.rs`), and the deadline is
///   honoured, so the volume stays consistent. But on a FAILING transfer the worst case is a multi-second,
///   non-preemptible hold (×`num_fats`): the scheduler cannot run, user mode is frozen, panel/input tasks stall.
///
/// The span is therefore honest-but-expensive on `Usb`, not "safe because polled". This is deliberately left
/// as a documented cost rather than a restructure: narrowing the span for one source would fork the RMW's
/// atomicity argument, and shortening the deadline belongs to the xHCI/BOT layer (out of this lane). What
/// USBFALL adds instead is EVIDENCE — see [`note_masked_usb_hold`], which witnesses the first masked-IRQ hold
/// taken on a `Usb` source so the cost is observable at the bench instead of inferred. Callers reach the lock
/// through [`with_fat_lock_src`]/[`with_dir_lock_src`], which carry the source explicitly: any NEW
/// `BlockSource` must answer this paragraph before it can be held across the RMW.
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
///
/// ⚠ THE X86 INVARIANT, STATED HONESTLY (2026-07-26; ROSTER AUDITED 2026-07-27 — see the block below).
/// Because this is a passthrough, x86 has NO in-`fat.rs` serialization of FAT/directory mutation. What keeps
/// the volume consistent is a discipline held ABOVE `fat.rs`, by its callers. That discipline is now written
/// down as a ROSTER, because it is caller-side and therefore only as good as the next caller added to it.
///
/// # THE X86 FAT-MUTATOR ROSTER (every x86 path that allocates/frees a cluster, writes a FAT entry, or
/// RMWs a directory sector). Full derivation + the interleavings in
/// `docs/dev/OS/07_USB_STORAGE/x86_interrupt_storage.md` ("x86 FAT concurrency audit").
///
/// | # | Mutator | Context (core) | Gate | Serialized by |
/// |---|---------|----------------|------|---------------|
/// | 1 | `flight_recorder::reserve_log` — `delete_located` + `create_in_root` + one `write_grow` | BSP main loop, IF=1, NOT a scheduled task | always (x86); ONE-SHOT for the whole boot | PROGRAM ORDER: its call site (`main.rs`) precedes every `U*_probe_once` and `install_probe_once` in the same iteration, and all of them gate on the same `block::info()` that has only just become `Some`. No other mutator can exist yet. |
/// | 2 | `flight_recorder::write_log` — `write_at` only | BSP main loop | always | NOT A MUTATOR. Bounded to clusters already in the reserved chain; writes no FAT entry and no directory sector, so it cannot interact with any writer at all. |
/// | 3 | storage service task — `create_in_root` / `write_grow` / `delete_located` / `write_at` (`drivers/xhci/irqstorage.rs`) | `storage-svc` task, AP[0], PRIO_HIGH | `irqstorage` | ONE task draining `REQ_QUEUE` one request at a time; every submitter blocks on the request's `done` semaphore. A real single writer for everything that reaches it. |
/// | 4 | demo-chain pre-flights — `u10x_preflight_grow_file`, `u10_preflight_absent` (`arch/x86_64/syscall.rs`) | the `u6bx-launch`/`u7x-launch` LAUNCHER task, AP[1], PRIO_NORMAL | `witness` **and** `HELLO_STAGED` | PROGRAM ORDER on the ONE launcher task: each stage spins on the previous stage's `*_LAUNCH_DONE` before it starts, and mutates only BEFORE it spawns its fixture. |
/// | 5 | demo-chain drains — `u10_drain_{grow,create_grow,create_grow_delete,delete}`, `flush_drain_one` | same launcher task | `witness` | ditto — reached only after the launcher has observed its fixture's teardown (`cleared`). |
/// | 6 | `openf_release` / `u11m2_phase` — `submit_grow` / `submit_delete` | launcher task / fixture teardown | `irqstorage` | routed through row 3 (they are submitters, not mutators). |
/// | 7 | **`shell::dispatch_command`** — `create_in_dir`, `create_dir`, `write_grow`, `delete_located`, `remove_dir`, `rename_entry`, `move_entry`, and a raw `write <lba> <byte>` | **BSP GUI main loop, INLINE** (`main.rs`'s `handle_key`), NOT a scheduled task | **NONE — compiled unconditionally** | **NOTHING.** See the rule below. |
/// | 8 | `install::write_sectors` — raw GPT/FAT32 format writes | BSP main loop | `installdemo` | PROGRAM ORDER on the BSP loop; its blank-scratch-disk configuration leaves `HELLO_STAGED` false (rows 4/5 skip) and permanently fails row 1's reserve. |
///
/// VERDICT: rows 1–6 and 8 are genuinely sequenced. Row 7 is NOT — the shell mutates the volume from the BSP
/// main loop with nothing between it and rows 3/4/5, which run on APs. It is unreachable in the QEMU
/// batteries (headless `-display none`: no HID key event ever reaches `handle_key` on x86 — the `poll_input`
/// feed in the main loop is aarch64-only) and harmless in shipping GUI/media builds (DEFAULT-QUIET leaves
/// `witness` OFF, so rows 4–6 do not exist and the shell is the sole writer). It becomes a REAL second-writer
/// window in exactly one configuration: an attended GUI boot built WITH `witness` (and/or `irqstorage`),
/// where a keystroke dispatched while the launcher chain is mid-`write_grow` interleaves two unsynchronized
/// cluster-chain mutations. Do not "fix" that here — see the rule.
///
/// THE RULE FOR A NEW X86 FAT WRITER: join one of the schemes above (submit through the storage service task,
/// or run in program order on the BSP main loop ahead of the launchers), and add yourself to this roster.
/// Do NOT make this lock real here: masking IRQs across the `hlt`-driven xHCI BOT pump would hang the core.
#[cfg(all(not(target_arch = "aarch64"), not(feature = "witness")))]
#[inline(always)]
fn with_fat_lock<R>(f: impl FnOnce() -> R) -> R {
    f()
}

/// Witness-build x86 [`with_fat_lock`]: the passthrough PLUS the roster tripwire (see
/// [`x86_rmw_tripwire`]). Behaviourally identical — it only counts.
#[cfg(all(not(target_arch = "aarch64"), feature = "witness"))]
#[inline]
fn with_fat_lock<R>(f: impl FnOnce() -> R) -> R {
    x86_rmw_tripwire(&FAT_RMW_INFLIGHT, &FAT_RMW_OVERLAPS, "FAT-table", f)
}

/// F3-M2: the DIRECTORY-sector mutation lock — the twin of [`FAT_MUTATION`] for the three directory-sector
/// read-modify-writes ([`FatFs::write_dir_entry_fields`], [`FatFs::mark_dir_deleted`], and
/// [`FatFs::create_in_root`]'s slot write). Each was a bare `read_sector -> modify -> write_sector` of a
/// directory sector with no serialization, so two cores RMW-ing entries in the SAME directory sector could
/// lose an update (e.g. a grow's size-publish resurrecting a racing delete's `0xE5`). One lock for the FAT and
/// one for directories is deliberate: they guard DISJOINT sectors and the two are never nested. Same arch
/// reasoning as `FAT_MUTATION` (aarch64-only; the x86 FAT path `hlt`-waits, masking across it would hang).
#[cfg(target_arch = "aarch64")]
static DIR_MUTATION: spin::Mutex<()> = spin::Mutex::new(());

/// Run `f` under the directory-sector mutation lock (aarch64), or unchanged (other arches). Same IRQ-masked,
/// non-preemptible discipline as [`with_fat_lock`] (see its doc for the deadlock reasoning). LOCK SPAN: only
/// a single directory-sector RMW (one read + one write) — never a directory SCAN (`find_free_root_slot` /
/// `find_located` stay outside; the F3-M3 namespace lock serializes those sequences). USBFALL F2: "one bounded
/// POLLED read + one write" holds for [`BlockSource::Default`] only — on a `Usb` source the same span rides the
/// BOT deadline with `wfi` under masked IRQs. See [`with_fat_lock`]'s LOCK SPAN paragraph for the per-source
/// statement; call sites take this lock through [`with_dir_lock_src`], which carries the source.
#[cfg(target_arch = "aarch64")]
#[inline]
fn with_dir_lock<R>(f: impl FnOnce() -> R) -> R {
    crate::arch::without_interrupts(|| {
        let _guard = DIR_MUTATION.lock();
        f()
    })
}

/// Non-aarch64 (x86): the directory-mutation lock is inert — the [`with_fat_lock`] passthrough reasoning,
/// including its "THE X86 INVARIANT, STATED HONESTLY" note and the MUTATOR ROSTER there (the caller-level
/// discipline is what serializes x86 directory-sector RMWs; there is no lock here).
#[cfg(all(not(target_arch = "aarch64"), not(feature = "witness")))]
#[inline(always)]
fn with_dir_lock<R>(f: impl FnOnce() -> R) -> R {
    f()
}

/// Witness-build x86 [`with_dir_lock`]: the passthrough PLUS the roster tripwire (see [`x86_rmw_tripwire`]).
#[cfg(all(not(target_arch = "aarch64"), feature = "witness"))]
#[inline]
fn with_dir_lock<R>(f: impl FnOnce() -> R) -> R {
    x86_rmw_tripwire(&DIR_RMW_INFLIGHT, &DIR_RMW_OVERLAPS, "directory-sector", f)
}

// ---------------------------------------------------------------------------------------------------------
// X86 ROSTER TRIPWIRE (`witness` builds only) — make a violation of the caller-side single-writer discipline
// documented on `with_fat_lock` SELF-REPORTING instead of showing up as a mystery cross-linked chain three
// arcs later.
//
// WHAT IT WATCHES: the two seams that ARE real locks on aarch64 — the FAT-sector RMW (`with_fat_lock`) and the
// directory-sector RMW (`with_dir_lock`). Those are precisely the spans where a second concurrent mutator
// corrupts the volume, and they are provably NON-NESTING, so a nonzero in-flight count at entry means a
// genuinely concurrent second mutator, never re-entrancy:
//   * `with_fat_lock` is taken at exactly two sites — `set_fat_entry` and `alloc_cluster`'s compare-and-claim
//     — and the claim body calls the lock-FREE `set_fat_entry_inner`, never `set_fat_entry` (that factoring
//     exists because the aarch64 lock is non-reentrant, so a nest would DEADLOCK there — the invariant is
//     enforced by aarch64, not merely assumed).
//   * `with_dir_lock`'s six bodies are pure single-sector read-modify-writes; none calls another (composites
//     like `create_dir`/`delete_located` call them SEQUENTIALLY), and `create_in_dir` dispatches to
//     `create_in_root` BEFORE taking the lock. The two locks are also never nested in each other.
//
// COST WHEN OFF: none. The whole family (statics, helper, and the tripwire-flavoured `with_*_lock`) is behind
// `feature = "witness"`, and the knob-off definitions above are the byte-identical `#[inline(always)]`
// passthroughs — the DEFAULT-QUIET rule (boot/media builds leave `witness` off).
// COST WHEN ON: two relaxed atomics per sector RMW. It NEVER prints in a correct run, so it cannot perturb a
// battery verdict; the one line it can emit fires only when the roster has actually been violated.
// aarch64 is untouched by design — it holds REAL locks, and this is an x86-only invariant.
// ---------------------------------------------------------------------------------------------------------

/// Mutators currently inside a FAT-sector RMW (`with_fat_lock`). See the block above.
#[cfg(all(not(target_arch = "aarch64"), feature = "witness"))]
static FAT_RMW_INFLIGHT: AtomicU32 = AtomicU32::new(0);
/// Mutators currently inside a directory-sector RMW (`with_dir_lock`).
#[cfg(all(not(target_arch = "aarch64"), feature = "witness"))]
static DIR_RMW_INFLIGHT: AtomicU32 = AtomicU32::new(0);
/// Sticky count of FAT-sector RMWs that began while another was already in flight.
#[cfg(all(not(target_arch = "aarch64"), feature = "witness"))]
static FAT_RMW_OVERLAPS: AtomicU32 = AtomicU32::new(0);
/// Sticky count of directory-sector RMWs that began while another was already in flight.
#[cfg(all(not(target_arch = "aarch64"), feature = "witness"))]
static DIR_RMW_OVERLAPS: AtomicU32 = AtomicU32::new(0);
/// One-shot latch so the tripwire reports at most ONE serial line per boot (a corrupting interleave tends to
/// repeat, and a spamming witness is a worse witness).
#[cfg(all(not(target_arch = "aarch64"), feature = "witness"))]
static RMW_TRIPPED: AtomicBool = AtomicBool::new(false);

/// Run one sector RMW with the roster tripwire around it: bump the in-flight count, and if it was ALREADY
/// nonzero, record the overlap (and report it once). Purely observational — `f` runs unchanged either way.
#[cfg(all(not(target_arch = "aarch64"), feature = "witness"))]
#[inline]
fn x86_rmw_tripwire<R>(
    inflight: &AtomicU32,
    overlaps: &AtomicU32,
    what: &str,
    f: impl FnOnce() -> R,
) -> R {
    if inflight.fetch_add(1, Ordering::AcqRel) != 0 {
        overlaps.fetch_add(1, Ordering::Relaxed);
        if !RMW_TRIPPED.swap(true, Ordering::Relaxed) {
            serial_println!(
                ":: FATRACE: SECOND CONCURRENT x86 {} MUTATOR DETECTED — the caller-side single-writer roster (see fs/fat.rs `with_fat_lock`) has been violated; expect cross-linked chains ::",
                what
            );
        }
    }
    let r = f();
    inflight.fetch_sub(1, Ordering::AcqRel);
    r
}

/// Roster-tripwire tallies `(FAT-sector overlaps, directory-sector overlaps)` for the boot. Both `0` is the
/// expected reading — it is the evidence that the caller-side discipline held for every RMW that actually ran.
#[cfg(all(not(target_arch = "aarch64"), feature = "witness"))]
pub fn x86_rmw_overlaps() -> (u32, u32) {
    (
        FAT_RMW_OVERLAPS.load(Ordering::Relaxed),
        DIR_RMW_OVERLAPS.load(Ordering::Relaxed),
    )
}

/// USBFALL F2: one-shot latch for [`note_masked_usb_hold`].
#[cfg(all(target_arch = "aarch64", feature = "witness"))]
static USBFALL_MASKED_USB_HOLD: core::sync::atomic::AtomicBool =
    core::sync::atomic::AtomicBool::new(false);

/// USBFALL F2 witness: record the FIRST time a FAT/dir sector RMW takes the masked-IRQ mutation lock on a
/// [`BlockSource::Usb`] volume — the case the [`with_fat_lock`] LOCK SPAN paragraph calls out as bounded but
/// expensive (a `pump_until_bot_done` deadline of `hw_wait_budget()*3`, with `wfi` inside, held under
/// `PSTATE.I` masked). One line, once per boot, so the cost is OBSERVED at the bench rather than inferred;
/// the quantity still owed from metal is the actual stall on a FAILING BOT write, which QEMU cannot produce.
/// Behind the `witness` feature (UNAOS_WITNESS) — a default-quiet build compiles this away entirely and the
/// hold is byte-identical to pre-USBFALL.
#[cfg(all(target_arch = "aarch64", feature = "witness"))]
#[inline]
fn note_masked_usb_hold(source: BlockSource, site: &str) {
    if source != BlockSource::Usb {
        return;
    }
    if !USBFALL_MASKED_USB_HOLD.swap(true, core::sync::atomic::Ordering::Relaxed) {
        serial_println!(
            ":: USBFALL: masked-IRQ FAT hold on the Usb source ({}) — span bounded by the BOT deadline, not by polled I/O ::",
            site
        );
    }
}

/// USBFALL F2: default-quiet / non-aarch64 build — the witness is compiled out.
#[cfg(not(all(target_arch = "aarch64", feature = "witness")))]
#[inline(always)]
fn note_masked_usb_hold(_source: BlockSource, _site: &str) {}

/// WEDGE-8 (F3): retry bounds for a `Busy` FAT/dir RMW — the masked closure found the storage
/// driver's controller loaned out and returned instantly (a masked context must never wait on a
/// driver lock; that wait IS the F3 deadlock). Each retry re-runs the WHOLE closure from outside
/// the `without_interrupts` span, with a `hlt()` between attempts so the scheduler can run the loan
/// holder to completion. Both bounds are needed: the attempt cap keeps the aarch64 masked path
/// (instant-`Busy` attempts, one timer tick apart) from spinning unbounded, and the wall-clock cap
/// keeps the x86 path (whose block layer already waits its own bounded budget per attempt) from
/// multiplying that budget by the attempt count. Exhaustion surfaces `FatError::Busy` → `-EAGAIN`:
/// the RMW never started, nothing was mutated, the caller may retry.
const RMW_BUSY_ATTEMPTS: u32 = 64;

/// USBFALL F2: [`with_fat_lock`], with the [`BlockSource`] the guarded RMW will run through made EXPLICIT.
/// Every `FatFs` call site uses this form so the span's per-source cost (see [`with_fat_lock`]'s LOCK SPAN
/// paragraph) is visible at the point the lock is taken, and so a newly added source cannot inherit the
/// "polled, therefore cheap" premise by accident.
///
/// WEDGE-8 (F3): the closure is now `Result`-typed and a `FatError::Busy` from inside it is RETRIED
/// here, OUTSIDE the masked span (see [`RMW_BUSY_ATTEMPTS`]). The invariant this establishes for
/// every span in `fs/`: no `without_interrupts` closure ever blocks on a driver lock — it fails
/// fast with `Busy` and the wait (a `hlt`, schedulable, unmasked) happens out here.
/// SDHC-4c: the FAT-MUTATION INSTRUMENT for the internal SD card.
///
/// [`with_fat_lock_src`] and [`with_dir_lock_src`] are the two wrappers EVERY FAT-table RMW
/// (`set_fat_entry`, `alloc_cluster`'s compare-and-claim) and EVERY directory RMW (all six
/// `with_dir_lock` bodies: `write_dir_entry_fields`, `..._mtime`, `create_in_root`, `create_in_dir`,
/// `write_dir_entry_name`, `mark_dir_deleted`) in this file passes through. Counting here therefore
/// counts the complete mutator surface with two call sites instead of eight, and — the property
/// that matters for the instrument-baseline law — it can print NON-ZERO: any future code that
/// mutates the card's FAT or directory is caught by construction, whether or not the write beneath
/// it is then refused.
///
/// Costs one integer comparison on every RMW on every source; the `Sdhc` variant does not exist
/// outside `x86_64 + sdhcblk`, so this compiles to nothing at all elsewhere.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
#[inline]
fn note_sdhc_mutation(source: BlockSource, site: &str) {
    if source == BlockSource::Sdhc {
        crate::fs::sdhc4c::note_fat_mutation(site);
    }
}

/// No `Sdhc` source exists in this build, so there is nothing to instrument. Byte-identical to
/// pre-4c on every other target.
#[cfg(not(all(target_arch = "x86_64", feature = "sdhcblk")))]
#[inline(always)]
fn note_sdhc_mutation(_source: BlockSource, _site: &str) {}

#[inline]
fn with_fat_lock_src<R>(
    source: BlockSource,
    site: &str,
    mut f: impl FnMut() -> Result<R, FatError>,
) -> Result<R, FatError> {
    note_masked_usb_hold(source, site);
    note_sdhc_mutation(source, site);
    let start = crate::arch::now_cycles();
    let budget = crate::arch::hw_wait_budget();
    for _ in 0..RMW_BUSY_ATTEMPTS {
        match with_fat_lock(&mut f) {
            Err(FatError::Busy) => {}
            other => return other,
        }
        if crate::arch::now_cycles().wrapping_sub(start) >= budget {
            break;
        }
        crate::hlt(); // unmasked here — the mask ended with the closure; let the holder run
    }
    Err(FatError::Busy)
}

/// USBFALL F2: the [`with_dir_lock`] twin of [`with_fat_lock_src`] — same reasoning, directory
/// sectors; WEDGE-8 (F3): same `Busy` retry-outside-the-mask discipline.
#[inline]
fn with_dir_lock_src<R>(
    source: BlockSource,
    site: &str,
    mut f: impl FnMut() -> Result<R, FatError>,
) -> Result<R, FatError> {
    note_masked_usb_hold(source, site);
    note_sdhc_mutation(source, site);
    let start = crate::arch::now_cycles();
    let budget = crate::arch::hw_wait_budget();
    for _ in 0..RMW_BUSY_ATTEMPTS {
        match with_dir_lock(&mut f) {
            Err(FatError::Busy) => {}
            other => return other,
        }
        if crate::arch::now_cycles().wrapping_sub(start) >= budget {
            break;
        }
        crate::hlt();
    }
    Err(FatError::Busy)
}

// ---------------------------------------------------------------------------------------------------------
// F2 M3 witness: a cross-core stress of the FAT_MUTATION serialization on an IN-RAM scratch counter — NOT the
// on-disk FAT, so it carries zero volume risk while exercising the EXACT lock that guards `set_fat_entry`. Two
// kernel tasks on distinct cores each drive `f2_witness_rmw(iters, locked)`. The step is a deliberately
// NON-ATOMIC read-modify-write (`Relaxed` load + `store`, NOT `fetch_add`), so if the two cores interleave the
// read->write windows an increment is LOST — unless `locked` routes the step through `with_fat_lock`, which
// serializes exactly as it does for the real `set_fat_entry` RMW. The scratch counter is the witness: after
// `2*iters` increments across two cores, a value below `2*iters` means updates were raced away. The launcher
// (`arch::syscall::f2_witness_launcher`) runs a LOCKED pass (must reach `2*iters` — serialization holds) and an
// UNLOCKED control (a nonzero loss proves the environment provoked real contention; a zero loss is reported
// honestly — RR-TCG did not interleave, so the race is metal-only). This stands in for `set_fat_entry`'s real
// on-disk RMW without scribbling the volume; the on-disk RMW under true metal parallelism rides the bench.
#[cfg(target_arch = "aarch64")]
static F2_WITNESS_COUNTER: AtomicU32 = AtomicU32::new(0);

/// F2 M3 witness — reset the scratch counter before a run. See the module note above.
#[cfg(target_arch = "aarch64")]
pub fn f2_witness_reset() {
    F2_WITNESS_COUNTER.store(0, Ordering::SeqCst);
}

/// F2 M3 witness — the scratch counter's current value (the increments that survived).
#[cfg(target_arch = "aarch64")]
pub fn f2_witness_value() -> u32 {
    F2_WITNESS_COUNTER.load(Ordering::SeqCst)
}

/// F2 M3 witness — drive `iters` non-atomic read-modify-writes of the scratch counter, optionally serialized
/// through the FAT_MUTATION lock (`locked`). NEVER touches the disk; safe to call concurrently from two cores.
#[cfg(target_arch = "aarch64")]
pub fn f2_witness_rmw(iters: u32, locked: bool) {
    for _ in 0..iters {
        if locked {
            with_fat_lock(f2_witness_step);
        } else {
            f2_witness_step();
        }
    }
}

/// One non-atomic read-modify-write of the scratch counter with a WIDE read->write window (a short spin), so a
/// round-robin-TCG quantum switch between the two cores is likely to land inside it. `#[inline(never)]` +
/// `Relaxed` load/store keep the load and store as two separate observable ops (the lost-update surface).
#[cfg(target_arch = "aarch64")]
#[inline(never)]
fn f2_witness_step() {
    let v = F2_WITNESS_COUNTER.load(Ordering::Relaxed);
    for _ in 0..48 {
        core::hint::spin_loop();
    }
    F2_WITNESS_COUNTER.store(v.wrapping_add(1), Ordering::Relaxed);
}

/// Try to interpret `sec` as a FAT boot sector (BPB) for a volume starting at absolute `part_lba`,
/// on a device of `dev_blocks` total blocks. Returns a fully-computed [`FatFs`] on success.
///
/// Rejects (as `NotFat`) anything that is not a plausible, self-consistent FAT volume: this is
/// also what distinguishes a superfloppy BPB from an MBR boot sector — an MBR's bootstrap bytes
/// won't pass the jump-instruction, sector-size, and geometry-consistency gates. FAT12 and
/// non-512-byte sectors are rejected as `Unsupported`.
///
/// PARTITION (GR9): `range` is the containing partition as the block layer describes it, when this
/// volume was found through a partition entry (`None` for a superfloppy, where the device is the
/// volume). Its declared length is a hard gate — see the `tot_sec` check below — and it is retained
/// in the mount so every later access is bound-checked against it. `part_slot` is carried only so
/// the mount witness can name which MBR slot the volume came from.
fn parse_bpb(
    sec: &[u8; SECTOR_SIZE],
    part_lba: u64,
    dev_blocks: u64,
    range: Option<crate::drivers::block::PartitionRange>,
    part_slot: u8,
    source: BlockSource,
) -> Result<FatFs, FatError> {
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

    // PARTITION (GR9): consistency vs the CONTAINING PARTITION. The gate above only proves the
    // volume fits the DISK — which is exactly what a FAT volume that overruns its partition and
    // runs on into the next one also satisfies. Nothing on the medium guarantees `tot_sec` agrees
    // with the partition entry that pointed here: they are two independent claims written by
    // (possibly) two different tools, and a mismatch is either a formatting bug or a deliberate
    // attempt to have partition 1's filesystem address partition 2's sectors. Refuse the mount
    // rather than clamp `tot_sec` down to the partition length: a volume whose own header disagrees
    // with its container is not a volume we understand, and silently shrinking it would leave a
    // filesystem whose FAT and cluster count describe sectors we then refuse to serve.
    if let Some(r) = range {
        if r.start_lba != part_lba || tot_sec as u64 > r.sector_count {
            return Err(FatError::NotFat);
        }
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

    // K1 M2.2: BS_VolID — the formatter's volume serial. Extended-BPB field: offset 0x27 on FAT12/16, 0x43 on
    // FAT32 (both well within the already-read boot sector). Read-only; used only as a UNAFS.ATR binding fingerprint.
    let vol_id = u32le(sec, if kind == FatKind::Fat32 { 0x43 } else { 0x27 });

    // PI-FS-5: BS_VolLab — the formatter's 11-byte volume label. Extended-BPB field: offset 0x2B on FAT12/16,
    // 0x47 on FAT32 (both within the already-read boot sector). Read-only, surfaced trimmed by `label()`.
    let mut vol_label = [0u8; 11];
    let lab_off = if kind == FatKind::Fat32 { 0x47 } else { 0x2B };
    vol_label.copy_from_slice(&sec[lab_off..lab_off + 11]);

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
        vol_id,
        vol_label,
        source,
        // PARTITION (GR9): the volume's own extent. `tot_sec` — proven above to fit the disk, to fit
        // the 32-bit LBA space, and (when this came from a partition entry) to fit the partition.
        vol_sectors: tot_sec as u64,
        part_slot,
        range,
    })
}

/// Mount the FAT volume on the registered block device. Tries, in order: a superfloppy (BPB at
/// LBA 0), a GPT (an `EFI PART` header at LBA 1 → partition entry → BPB), then a classic MBR at
/// LBA 0 whose first FAT-typed partition entry points at the BPB.
pub fn mount() -> Result<FatFs, FatError> {
    mount_source(BlockSource::Default)
}

/// APPLOAD: mount the volume a PROGRAM should be loaded from — the global block device if one is
/// registered, else (x86 + `sdhcblk`) the card in the machine's internal SD slot.
///
/// This is [`mount`] for the one class of caller that means "wherever I can find an executable",
/// rather than "the device the system is bound to". The distinction had no consequences while the
/// only readable medium was the boot stick; it acquired one the moment SDHC-4b gave the internal
/// reader a handle of its own, because a machine booted from that reader has a mounted, listed,
/// program-bearing volume and an EMPTY global slot at the same time.
///
/// Precedence and its compatibility argument live on [`crate::drivers::block::program_source`]; the
/// short form is that the global wins whenever it exists, so this is identical to [`mount`] on every
/// boot that already worked.
///
/// **The read path follows the handle, by construction.** The handle is mapped to the matching
/// [`BlockSource`] here and every subsequent read — the BPB probe, the partition scan, each directory
/// sector, each data cluster — routes through `read_sector` / `read_sectors`, which already dispatch
/// per source (`Sdhc` -> `read_block_sdhc` / `read_blocks_sdhc`, bypassing the backend selector).
/// No second read mechanism is introduced and none is needed.
pub fn mount_program_source() -> Result<FatFs, FatError> {
    let (_dev, handle) = crate::drivers::block::program_source().ok_or(FatError::NoDisk)?;
    mount_source(source_of(handle))
}

/// APPLOAD: the inverse of [`handle_of`] — which [`BlockSource`] reads a given registry handle.
///
/// Total by construction, so a handle added to the block layer without a source here is a compile
/// error rather than a silent mis-route. `Usb` is mapped for totality only:
/// [`crate::drivers::block::program_source`] never returns it (see the precedence note there).
fn source_of(handle: crate::drivers::block::BlockHandle) -> BlockSource {
    match handle {
        crate::drivers::block::BlockHandle::Global => BlockSource::Default,
        crate::drivers::block::BlockHandle::Usb => BlockSource::Usb,
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        crate::drivers::block::BlockHandle::Sdhc => BlockSource::Sdhc,
    }
}

/// PIUSB-27: mount a FAT volume from a chosen block `source`. `Default` is the globally-registered device
/// (SD on the Pi); `Usb` reads the USB stick directly through the xHCI controller so it can be browsed
/// read-only even while the SD backend owns the global block device. Geometry comes from the matching
/// source (`info` / `usb_info`); the partition/BPB scan and every derived read route through `source`.
pub fn mount_source(source: BlockSource) -> Result<FatFs, FatError> {
    let dev = match source {
        BlockSource::Default => crate::drivers::block::info(),
        BlockSource::Usb => crate::drivers::block::usb_info(),
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        BlockSource::Sdhc => crate::drivers::block::sdhc_info(),
    }
    .ok_or(FatError::NoDisk)?;
    if dev.block_size != SECTOR_SIZE as u32 {
        return Err(FatError::Unsupported);
    }
    let dev_blocks = dev.num_blocks;

    let mut sec = [0u8; SECTOR_SIZE];
    read_sector(source, 0, &mut sec)?;

    // PARTITION (GR9): the raw census, before any of the three interpretations below. It prints at
    // most once per handle per boot and returns the decoded table either way, so this call is both
    // the witness and the table the MBR branch uses — one decode, one printed opinion, no chance of
    // the log describing a table the mount did not use.
    let table = crate::drivers::block::mbr_census(handle_of(source), &sec, dev_blocks);

    // 1) Superfloppy: LBA 0 is itself the BPB.
    //
    //    Tried FIRST, and that ordering is load-bearing: a FAT superfloppy also carries 0x55AA at
    //    offset 510, and the bootstrap code occupying bytes 446..510 can decode as plausible-looking
    //    partition entries. Only the BPB gates (jump instruction, sector size, geometry consistency)
    //    tell the two apart, so they run before any partition-table reading is trusted. `None` for
    //    the partition length: there is no container — the device IS the volume, and the
    //    `part_lba + tot_sec <= dev_blocks` gate inside `parse_bpb` is its bound.
    if let Ok(fs) = parse_bpb(&sec, 0, dev_blocks, None, 0, source) {
        return Ok(fs);
    }

    // 2) GPT: an "EFI PART" header at LBA 1 (LBA 0 is a protective MBR). Checked BEFORE the MBR scan
    //    because a GPT disk's protective MBR entry (type 0xEE, start LBA 1) would otherwise be
    //    misread as a classic partition and send us to parse a BPB at the GPT header sector.
    if let Ok(fs) = scan_gpt(dev_blocks, source) {
        return Ok(fs);
    }

    // 3) MBR-partitioned — the UnaOS layout of record: partition 1 = ESP (FAT), partition 2 = UnaFS.
    //    Walk the ACCEPTED entries in slot order, so partition 1 is what a FAT mount binds when it is
    //    a FAT volume, and each candidate is parsed under ITS OWN partition length. That length is
    //    the difference between this and mounting off the raw device: a BPB claiming more sectors
    //    than its partition holds is refused here rather than becoming a volume that can address its
    //    neighbour. `decode_mbr` has already dropped empty, extended, zero-length, out-of-range,
    //    overlapping and GPT-protective slots, so anything reaching this loop is a disjoint,
    //    in-bounds extent on this medium.
    if let Some(t) = table {
        for p in t.iter() {
            let mut pbs = [0u8; SECTOR_SIZE];
            if read_sector(source, p.start_lba, &mut pbs).is_err() {
                continue;
            }
            let range = crate::drivers::block::PartitionRange::new(handle_of(source), &p);
            if let Ok(fs) = parse_bpb(&pbs, p.start_lba, dev_blocks, Some(range), p.slot, source) {
                mount_witness(&fs);
                return Ok(fs);
            }
        }
    }

    Err(FatError::NotFat)
}

/// PARTITION (GR9): which block-layer registry handle a FAT [`BlockSource`] reads through. The two
/// enums are deliberately separate types (one names a FAT read path, the other a registry slot);
/// this is the single place they are related, so the census can never be attributed to the wrong
/// device.
fn handle_of(source: BlockSource) -> crate::drivers::block::BlockHandle {
    match source {
        BlockSource::Default => crate::drivers::block::BlockHandle::Global,
        BlockSource::Usb => crate::drivers::block::BlockHandle::Usb,
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        BlockSource::Sdhc => crate::drivers::block::BlockHandle::Sdhc,
    }
}

/// PARTITION (GR9): announce a FAT volume mounted from an MBR slot, once per slot per boot.
///
/// INSTRUMENT NOTE (healthy-but-idle): the latch below reads 0 before the first partition-hosted FAT
/// mount and then holds whichever slot bits have been announced. It gates printing only — the mount
/// itself is unconditional — so a missing line means "already announced this boot", never "did not
/// mount". On the layout of record exactly one line appears, naming slot 1.
fn mount_witness(fs: &FatFs) {
    static ANNOUNCED: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
    if fs.part_slot == 0 || fs.part_slot > 4 {
        return;
    }
    let bit = 1u8 << (fs.part_slot - 1);
    if ANNOUNCED.fetch_or(bit, Ordering::Relaxed) & bit != 0 {
        return;
    }
    let (start, end) = fs.extent();
    serial_println!(
        ":: PART: fat mounted from MBR slot {} — extent LBA {}..{} ({} sectors), {} ::",
        fs.part_slot, start, end, fs.vol_sectors, fs.describe()
    );
}

/// The candidate volume-start LBAs named by a classic MBR partition table in `sec` (LBA 0). Empty if
/// `sec` carries no 0x55AA signature. Empty (0x00) and extended-partition containers (0x05 CHS /
/// 0x0F LBA) are skipped, as are starts at 0 or past the end of the device.
///
/// Extracted from `mount_source` so the INSTALL-SELF serial enumerator
/// ([`volume_serials`]) walks the exact same candidate set the mount path does — one table walk, so
/// the guard can never see a partition the mount would not, or miss one it would.
///
/// PARTITION (GR9): this walk stays DELIBERATELY BROADER than
/// [`crate::drivers::block::decode_mbr`], which the mount path now uses. `decode_mbr` additionally
/// drops entries whose extent overruns the device or overlaps an accepted neighbour; those drops are
/// right for "which volume do I mount" and WRONG for "which volumes might this disk be", because the
/// only failure mode that matters to the boot-device guard is missing a serial — that would offer
/// the boot disk as an erase target. Enumerating a SUPERSET of the mount's candidates keeps the
/// error in the safe direction (more refusals, never fewer), which is the property `volume_serials`'
/// doc comment above states. It is therefore not a drift between two parsers: it is the same rule
/// set minus the two rules that only make sense when choosing a volume to trust.
fn mbr_volume_starts(sec: &[u8; SECTOR_SIZE], dev_blocks: u64) -> alloc::vec::Vec<u64> {
    let mut out = alloc::vec::Vec::new();
    if sec[510] != 0x55 || sec[511] != 0xAA {
        return out;
    }
    for i in 0..4 {
        let e = 446 + i * 16;
        let ptype = sec[e + 4];
        let start = u32le(sec, e + 8);
        if ptype == 0x00 || ptype == 0x05 || ptype == 0x0F || start == 0 {
            continue;
        }
        if start as u64 >= dev_blocks {
            continue;
        }
        out.push(start as u64);
    }
    out
}

/// INSTALL-SELF: every FAT volume serial (`BS_VolID`) discoverable on `source`, in the same
/// superfloppy → GPT → MBR order [`mount_source`] scans, with duplicates collapsed.
///
/// [`mount_source`] is first-match-wins: it returns the FIRST volume that parses and stops. That is
/// the right rule for "mount the boot media", and the WRONG rule for "is this disk the one we booted
/// from" — a device whose second partition is the ESP we booted would go unrecognized and be offered
/// as an erase target. So the guard enumerates ALL of them. The direction of the difference is the
/// safe one: a superset of serials can only ever cause MORE candidates to be refused, never fewer.
///
/// Read-only, bounded (≤ 1 superfloppy + ≤ 128 GPT entries + ≤ 4 MBR entries), and every parse goes
/// through the same [`parse_bpb`] gates the mount path trusts. Errors degrade to "fewer serials
/// found", never to a panic; the caller treats an empty result as "no FAT here, cannot be the boot
/// device".
pub fn volume_serials(source: BlockSource) -> alloc::vec::Vec<u32> {
    let mut out: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
    let dev = match source {
        BlockSource::Default => crate::drivers::block::info(),
        BlockSource::Usb => crate::drivers::block::usb_info(),
        #[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
        BlockSource::Sdhc => crate::drivers::block::sdhc_info(),
    };
    let Some(dev) = dev else { return out };
    if dev.block_size != SECTOR_SIZE as u32 {
        return out;
    }
    let dev_blocks = dev.num_blocks;

    let mut sec = [0u8; SECTOR_SIZE];
    if read_sector(source, 0, &mut sec).is_err() {
        return out;
    }

    // 1) Superfloppy: LBA 0 is itself the BPB.
    //
    // PARTITION (GR9): every parse here passes `None` for the containing-partition length ON PURPOSE.
    // This function does not mount anything — it only reads `BS_VolID` — and applying the partition
    // gate would drop a volume whose BPB overruns its entry, i.e. would make the boot-device guard
    // recognize FEWER disks as "the disk we booted from". The gate's job is to keep an untrustworthy
    // volume from being MOUNTED; the guard's job is to recognize as many volumes as possible so it
    // can refuse them as install targets. Same reasoning as the broader MBR walk below.
    if let Ok(fs) = parse_bpb(&sec, 0, dev_blocks, None, 0, source) {
        push_unique(&mut out, fs.vol_id);
    }

    // 2) + 3) Every partition start either table names.
    let mut starts: alloc::vec::Vec<u64> =
        gpt_volume_spans(dev_blocks, source).into_iter().map(|(s, _)| s).collect();
    starts.extend_from_slice(&mbr_volume_starts(&sec, dev_blocks));
    for start in starts {
        let mut pbs = [0u8; SECTOR_SIZE];
        if read_sector(source, start, &mut pbs).is_err() {
            continue;
        }
        if let Ok(fs) = parse_bpb(&pbs, start, dev_blocks, None, 0, source) {
            push_unique(&mut out, fs.vol_id);
        }
    }
    out
}

/// Append `v` only if absent. The serial list is at most a handful of entries, so a linear scan is
/// both the simplest and the fastest thing here.
fn push_unique(out: &mut alloc::vec::Vec<u32>, v: u32) {
    if !out.contains(&v) {
        out.push(v);
    }
}

/// Look for a GUID Partition Table: a header at LBA 1 with the `EFI PART` signature, then walk its
/// partition entry array for the first entry whose start LBA holds a valid FAT BPB. Returns NotFat
/// if there is no GPT or no FAT partition. Read-only; the scan is bounded (≤128 entries), and only
/// entry sizes that divide a 512-byte sector (128 / 256) are handled so no entry straddles a read.
fn scan_gpt(dev_blocks: u64, source: BlockSource) -> Result<FatFs, FatError> {
    for (first_lba, sectors) in gpt_volume_spans(dev_blocks, source) {
        let mut pbs = [0u8; SECTOR_SIZE];
        if read_sector(source, first_lba, &mut pbs).is_err() {
            continue;
        }
        // PARTITION (GR9): bounded by the GPT entry's own extent, exactly as the MBR branch is
        // bounded by the primary entry's — a GPT partition is no more entitled to overrun into its
        // neighbour than an MBR one. `part_slot` stays 0: GPT entries are not MBR primary slots and
        // conflating the two numbers in a witness line would be a lie.
        let range = crate::drivers::block::PartitionRange {
            handle: handle_of(source),
            start_lba: first_lba,
            sector_count: sectors,
        };
        if let Ok(fs) = parse_bpb(&pbs, first_lba, dev_blocks, Some(range), 0, source) {
            return Ok(fs);
        }
    }
    Err(FatError::NotFat)
}

/// The candidate volume spans `(first_lba, sector_count)` named by a GUID Partition Table on
/// `source`, in entry order. Empty if there is no `EFI PART` header at LBA 1 or its geometry fields
/// are implausible.
///
/// Extracted from [`scan_gpt`] so the INSTALL-SELF serial enumerator ([`volume_serials`]) walks the
/// exact same entry set the mount path does. Bounded at 128 entries regardless of a corrupt count,
/// and only entry sizes that divide a 512-byte sector (128 / 256) are handled so no entry straddles a
/// read. A read failure mid-array stops the walk (as it always did); an implausible entry is skipped.
///
/// PARTITION (GR9): the entry's `last_lba` is now read alongside `first_lba` so the mount path can
/// bound the volume by its partition. Both are on-disk claims: an entry whose `last < first`, or
/// whose extent runs past the device, is SKIPPED rather than clamped — the same rule the MBR decoder
/// applies, for the same reason.
fn gpt_volume_spans(dev_blocks: u64, source: BlockSource) -> alloc::vec::Vec<(u64, u64)> {
    let mut out = alloc::vec::Vec::new();
    if dev_blocks < 3 {
        return out;
    }
    let mut hdr = [0u8; SECTOR_SIZE];
    if read_sector(source, 1, &mut hdr).is_err() {
        return out;
    }
    if &hdr[0..8] != b"EFI PART" {
        return out;
    }
    let entries_lba = u64le(&hdr, 72);
    let num_entries = u32le(&hdr, 80);
    let entry_size = u32le(&hdr, 84);
    if !(entry_size == 128 || entry_size == 256) || entries_lba == 0 || entries_lba >= dev_blocks {
        return out;
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
            if read_sector(source, sec, &mut buf).is_err() {
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
        // PARTITION (GR9): `last_lba` is INCLUSIVE in the UEFI entry format, so the length is
        // `last - first + 1`. Every step is checked: a reversed pair, an overflowing sum, or an
        // extent past the medium drops the entry.
        let last_lba = u64le(&buf, off + 40);
        if last_lba < first_lba {
            continue;
        }
        let Some(sectors) = last_lba.checked_sub(first_lba).and_then(|d| d.checked_add(1)) else {
            continue;
        };
        match first_lba.checked_add(sectors) {
            Some(end) if end <= dev_blocks => out.push((first_lba, sectors)),
            _ => continue,
        }
    }
    out
}

impl FatFs {
    pub fn kind(&self) -> FatKind {
        self.kind
    }

    /// One-line human summary of the parsed geometry (for `fatinfo` / boot log).
    pub fn describe(&self) -> String {
        let head = alloc::format!(
            "FAT{} vol@LBA{} volsec={} bps={} spc={} nfat={} fatsz={}sec reserved={} fat@LBA{} data@LBA{} clusters={}",
            match self.kind {
                FatKind::Fat16 => 16,
                FatKind::Fat32 => 32,
            },
            self.part_lba,
            self.vol_sectors,
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

    // --- PARTITION (GR9): the volume-extent gate every sector access passes through ---

    /// One past the last sector of this volume, absolute. Cannot overflow: `parse_bpb` already
    /// proved `part_lba + tot_sec <= u32::MAX + 1`.
    fn vol_end(&self) -> u64 {
        self.part_lba.saturating_add(self.vol_sectors)
    }

    /// Refuse an absolute span that is not entirely inside this volume.
    ///
    /// The FAT geometry checks in [`parse_bpb`] already make every DERIVED address (FAT sector,
    /// root-dir sector, cluster sector) fall inside `tot_sec` by construction. This gate is the
    /// second, independent layer: it does not reason about geometry at all, it just compares the
    /// address that is actually about to be handed to the block layer against the volume's own
    /// bounds. That is the layer that still holds if a future geometry derivation is wrong, if an
    /// on-disk field is corrupt, or if a caller hands in an LBA it computed itself — which is
    /// exactly the class of bug that writes into the neighbouring partition. Cheap: two compares per
    /// sector op, against a path that costs a USB round trip.
    fn in_extent(&self, lba: u64, sectors: u64) -> Result<(), FatError> {
        let end = lba.checked_add(sectors).ok_or(FatError::OutOfVolume)?;
        if sectors == 0 || lba < self.part_lba || end > self.vol_end() {
            return Err(FatError::OutOfVolume);
        }
        // The partition's own bound, as the block layer states it — a separate claim from a separate
        // on-disk structure. `contains_absolute` re-derives nothing from this file's fields.
        if let Some(r) = self.range {
            if !r.contains_absolute(lba, sectors) {
                return Err(FatError::OutOfVolume);
            }
        }
        Ok(())
    }

    /// Extent-checked [`read_sector`].
    fn rd_sector(&self, lba: u64, buf: &mut [u8; SECTOR_SIZE]) -> Result<(), FatError> {
        self.in_extent(lba, 1)?;
        read_sector(self.source, lba, buf)
    }

    /// Extent-checked [`write_sector`]. The write side matters most: a read that escapes the volume
    /// returns wrong bytes, a write that escapes it destroys a neighbour.
    fn wr_sector(&self, lba: u64, buf: &[u8; SECTOR_SIZE]) -> Result<(), FatError> {
        self.in_extent(lba, 1)?;
        write_sector(self.source, lba, buf)
    }

    /// Extent-checked [`read_sectors`] — the whole run is checked, not just its first sector.
    fn rd_sectors(&self, lba: u64, buf: &mut [u8]) -> Result<(), FatError> {
        if buf.is_empty() || buf.len() % SECTOR_SIZE != 0 {
            return Err(FatError::Io);
        }
        self.in_extent(lba, (buf.len() / SECTOR_SIZE) as u64)?;
        read_sectors(self.source, lba, buf)
    }

    /// Extent-checked [`write_sectors`].
    fn wr_sectors(&self, lba: u64, buf: &[u8]) -> Result<(), FatError> {
        if buf.is_empty() || buf.len() % SECTOR_SIZE != 0 {
            return Err(FatError::Io);
        }
        self.in_extent(lba, (buf.len() / SECTOR_SIZE) as u64)?;
        write_sectors(self.source, lba, buf)
    }

    /// PARTITION (GR9): this volume's extent as absolute LBAs `[start, end)` — for witnesses and
    /// for a caller that wants to assert two mounted volumes do not overlap.
    pub fn extent(&self) -> (u64, u64) {
        (self.part_lba, self.vol_end())
    }

    /// PARTITION (GR9): the MBR primary slot this volume came from (1..=4), or 0 if it did not come
    /// from an MBR entry.
    pub fn partition_slot(&self) -> u8 {
        self.part_slot
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
        self.rd_sector(self.fat_start + fat as u64 * self.fat_sz as u64 + sec, &mut buf)?;
        Ok(match self.kind {
            FatKind::Fat16 => u16le(&buf, within) as u32,
            FatKind::Fat32 => u32le(&buf, within) & 0x0FFF_FFFF,
        })
    }

    /// Number of FAT copies (`num_fats`, usually 2 on FAT32). Public for the launcher's FAT-copy-agreement check.
    pub fn num_fats(&self) -> u32 {
        self.num_fats
    }

    /// K1 M2.2: the volume FINGERPRINT — `(BS_VolID, count_of_clusters)`. Read-only; the aarch64 UNAFS.ATR ACL
    /// store binds to it so a FOREIGN volume or a REFORMAT (a DIFFERENT serial or cluster count) is rejected and
    /// its owner rows never attach to this volume's directory slots. (A full byte-for-byte clone preserves both
    /// fields and is NOT rejected — that is offline tampering, explicitly out of scope.) Two identity fields
    /// chosen for stability under non-destructive edits: the serial is fixed at format time, the cluster count by
    /// the geometry.
    pub fn volume_fingerprint(&self) -> (u32, u32) {
        (self.vol_id, self.count_of_clusters)
    }

    /// Bytes per cluster (`sec_per_clus * bytes_per_sec`). Public for the U10 GROW launcher's chain-length
    /// check: a grow allocates `new_size.div_ceil(cluster_size)` clusters, which differs across the image
    /// layouts (512-B clusters on FAT32 superfloppy/MBR vs 2048-B on the FAT16 fixed-root image), so the
    /// launcher computes the expected chain length rather than assuming a fixed 2.
    pub fn cluster_size(&self) -> u32 {
        self.sec_per_clus * self.bytes_per_sec
    }

    /// PI-FS-5: the volume's formatted usable capacity in bytes (`count_of_clusters * cluster_size`) — the
    /// data-region size a `diskinfo` line reports for the FAT volume. Not the raw device size (which the block
    /// geometry gives): this is what the filesystem actually addresses.
    pub fn volume_bytes(&self) -> u64 {
        self.count_of_clusters as u64 * self.cluster_size() as u64
    }

    /// PI-FS-5: the trimmed `BS_VolLab` volume label (ASCII, space-padded on disk). Returns an empty string when
    /// the field is blank or the conventional `NO NAME` placeholder, so `diskinfo` can show a `-` instead.
    pub fn label(&self) -> String {
        let raw = core::str::from_utf8(&self.vol_label).unwrap_or("").trim_end_matches([' ', '\0']);
        if raw.is_empty() || raw == "NO NAME" {
            String::new()
        } else {
            String::from(raw)
        }
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
        // F2: serialize the WHOLE all-copies RMW under `FAT_MUTATION`. A concurrent writer mutating a
        // different entry in this same sector (or the other FAT copy) can no longer read-before-our-write and
        // then clobber our update. The lock spans ONLY the bounded `num_fats` read-modify-write — never a
        // free-search or a data-cluster loop. USBFALL F2: what that span COSTS depends on `self.source`, and
        // the claim is stated per-source on `with_fat_lock`'s LOCK SPAN paragraph (`Default` = bounded polled
        // sector transfers; `Usb` = the BOT deadline with `wfi` under masked IRQs). Read it there rather than
        // assuming "polled" here. On x86 `with_fat_lock` is a zero-cost passthrough.
        with_fat_lock_src(self.source, "set_fat_entry", || self.set_fat_entry_inner(cluster, next))
    }

    /// F3: the lock-FREE body of [`FatFs::set_fat_entry`] — the all-copies FAT-sector RMW with NO
    /// `FAT_MUTATION` acquisition. The caller MUST already hold `FAT_MUTATION` (on aarch64; on x86 the lock is
    /// inert and this is just the shared body). Factored out so `alloc_cluster`'s compare-and-claim can
    /// re-check + claim a candidate entry inside ONE lock hold without re-entering the (non-reentrant) lock.
    fn set_fat_entry_inner(&self, cluster: u32, next: u32) -> Result<(), FatError> {
        let offset = match self.kind {
            FatKind::Fat16 => cluster as u64 * 2,
            FatKind::Fat32 => cluster as u64 * 4,
        };
        let sec = offset / SECTOR_SIZE as u64;
        if sec >= self.fat_sz as u64 {
            return Err(FatError::BadChain); // never index past the FAT region
        }
        let within = (offset % SECTOR_SIZE as u64) as usize;
        let mut buf = [0u8; SECTOR_SIZE];
        for f in 0..self.num_fats as u64 {
            let lba = self.fat_start + f * self.fat_sz as u64 + sec;
            self.rd_sector(lba, &mut buf)?;
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
            self.wr_sector(lba, &buf)?;
        }
        Ok(())
    }

    /// U10: zero-fill every sector of a data cluster. Called BEFORE a freshly allocated cluster joins a chain,
    /// so no stale bytes from a previously-freed file can leak into a grown/created region (an information-
    /// disclosure invariant).
    ///
    /// MULTIBLK: a cluster's sectors are consecutive on disk BY DEFINITION, and every one of them is
    /// written in full, so this is the purest case for a counted transfer — the whole cluster goes
    /// out as one run (chunked only by the block layer's `MAX_BLOCKS_PER_OP`). It used to be
    /// `sec_per_clus` separate WRITE(10)s, which on the 32 KiB clusters real sticks are formatted
    /// with is 64 USB round trips per allocated cluster; §12.1 counted ~192 of them in a single
    /// flight-recorder reservation. The zeroing is otherwise unchanged, and so is its ORDER relative
    /// to the claim in `alloc_cluster` — the information-disclosure invariant is untouched.
    fn zero_cluster(&self, cluster: u32) -> Result<(), FatError> {
        if !self.valid_cluster(cluster) {
            return Err(FatError::BadChain);
        }
        let step = core::cmp::min(
            self.sec_per_clus as usize,
            crate::drivers::block::MAX_BLOCKS_PER_OP,
        );
        let zeros = alloc::vec![0u8; step * SECTOR_SIZE];
        let mut done = 0u64;
        while done < self.sec_per_clus as u64 {
            let n = core::cmp::min(step as u64, self.sec_per_clus as u64 - done);
            self.wr_sectors(
                self.cluster_lba(cluster) + done,
                &zeros[..n as usize * SECTOR_SIZE],
            )?;
            done += n;
        }
        Ok(())
    }

    /// U10: allocate one data cluster — a bounded first-fit free search over `[2, count_of_clusters + 2)`,
    /// then claim it (EOC in all FAT copies) and zero-fill it, returning its number READY TO LINK (a terminated
    /// 1-cluster orphan until the caller links it onto a chain). NEVER returns a reserved/bad/out-of-range
    /// cluster — only one whose FAT entry reads `0` (free) INSIDE the claim's lock hold. `-ENOSPC` (`NoSpace`)
    /// when the volume is full.
    ///
    /// F3-M1 (compare-and-claim — closes the F2 cluster-aliasing leg): the free SEARCH stays unlocked (cheap,
    /// read-only), but the CLAIM re-reads the candidate's entry under `FAT_MUTATION` and sets EOC only if it is
    /// STILL free — so two cores whose unlocked searches both saw cluster `c` free can never both claim it (the
    /// loser sees the winner's EOC and keeps scanning; a bounded retry, each cluster visited once). ORDER
    /// REORDERED vs U10: the claim now PRECEDES the zero-fill — zero-filling before the claim would let the
    /// loser's zero pass scribble a cluster the winner already claimed, linked, and wrote. The disclosure
    /// invariant is intact: the cluster is zero-filled before the CALLER links it into any chain (it is
    /// EOC-reserved but UNLINKED during the fill, so no reader path can walk onto its stale bytes). Error path:
    /// a zero-fill failure AFTER the claim orphans `c` (EOC, unlinked — a benign lost cluster, chkdsk-
    /// reclaimable), never an aliased or stale-visible one.
    ///
    /// MULTIBLK (2026-07-29) — THE ROTATING START, and why it is the biggest single amplifier here.
    /// The search used to restart at cluster 2 on EVERY call. Allocating a run of N clusters is then
    /// quadratic in the FAT sectors it reads: the flight recorder's 66048-byte reservation on a
    /// 512-byte-cluster volume allocates 129 clusters, and if the free region starts around cluster
    /// 2530 each of those 129 searches re-reads the ~20 FAT sectors in front of it — ~2580 sector
    /// reads, which measured as the LARGEST source of BOT transactions in the QEMU FAT battery,
    /// larger than every data transfer put together. Starting the scan where the last successful
    /// claim left off makes the same reservation cost ~2 FAT sector reads, because a FAT32 sector
    /// holds 128 entries and consecutive allocations stay inside it.
    ///
    /// CORRECTNESS IS UNCHANGED, and deliberately so — this alters the ORDER of the search, nothing
    /// else. Every cluster in `[2, count + 2)` is still visited at most once per call (the scan wraps
    /// exactly once, back to cluster 2), so "the volume is full" still means the volume is full and
    /// `NoSpace` cannot be returned early. The claim is still the F3-M1 compare-and-claim under
    /// `FAT_MUTATION`, so two cores cannot alias a cluster no matter where their searches began; a
    /// stale or nonsensical hint (e.g. carried over from a different volume) is validated back into
    /// range and costs at most one extra wrap, never a bad allocation. The zero-fill-after-claim
    /// order, and with it the information-disclosure invariant, is untouched.
    fn alloc_cluster(&self) -> Result<u32, FatError> {
        let entry_bytes: u64 = if self.kind == FatKind::Fat32 { 4 } else { 2 };
        let last = self.count_of_clusters + 2; // exclusive: valid data clusters are 2 ..= count+1
        if last <= 2 {
            return Err(FatError::NoSpace);
        }
        let mut buf = [0u8; SECTOR_SIZE];
        let mut loaded = u64::MAX;
        // Where to begin. A hint outside this volume's cluster range is meaningless, not dangerous —
        // clamp it back to the classic start and carry on.
        let hint = ALLOC_HINT.load(core::sync::atomic::Ordering::Relaxed);
        let mut c = if hint < 2 || hint >= last { 2 } else { hint };
        let span = last - 2; // clusters that exist; the scan visits each at most once
        let mut visited = 0u32;
        while visited < span {
            visited += 1;
            let offset = c as u64 * entry_bytes;
            let sec = offset / SECTOR_SIZE as u64;
            if sec >= self.fat_sz as u64 {
                break; // past the FAT region (defensive — parse_bpb already gates this)
            }
            if sec != loaded {
                self.rd_sector(self.fat_start + sec, &mut buf)?;
                loaded = sec;
            }
            let within = (offset % SECTOR_SIZE as u64) as usize;
            let e = match self.kind {
                FatKind::Fat16 => u16le(&buf, within) as u32,
                FatKind::Fat32 => u32le(&buf, within) & 0x0FFF_FFFF,
            };
            if e == 0 {
                // Candidate. COMPARE-AND-CLAIM under FAT_MUTATION: re-read the entry inside the lock and
                // claim (EOC) only if still free — a racing allocator that claimed it first loses us nothing
                // but this re-check. The whole hold is one sector read + the bounded all-copies RMW (the
                // `with_fat_lock` span rule). On x86 the lock is INERT — see `with_fat_lock` for what
                // actually holds the x86 side together (it is NOT "one writer by construction").
                let claimed = with_fat_lock_src(self.source, "alloc_cluster", || -> Result<bool, FatError> {
                    let mut cbuf = [0u8; SECTOR_SIZE];
                    self.rd_sector(self.fat_start + sec, &mut cbuf)?;
                    let cur = match self.kind {
                        FatKind::Fat16 => u16le(&cbuf, within) as u32,
                        FatKind::Fat32 => u32le(&cbuf, within) & 0x0FFF_FFFF,
                    };
                    if cur != 0 {
                        return Ok(false); // lost the race — a concurrent claim took it
                    }
                    self.set_fat_entry_inner(c, self.eoc_value())?;
                    Ok(true)
                })?;
                if claimed {
                    // MULTIBLK: publish the hint BEFORE the zero-fill, so a zero-fill failure (which
                    // orphans `c`) still moves the next search past it rather than re-finding a
                    // cluster that is now EOC-marked and will simply be skipped.
                    let next = if c + 1 >= last { 2 } else { c + 1 };
                    ALLOC_HINT.store(next, core::sync::atomic::Ordering::Relaxed);
                    // Zero AFTER the claim (see the doc comment): EOC-reserved but unlinked, so no reader can
                    // see stale bytes; a failure here orphans `c` (benign lost cluster) rather than aliasing.
                    self.zero_cluster(c)?;
                    return Ok(c);
                }
                loaded = u64::MAX; // our search buffer is stale (a concurrent writer mutated this sector)
            }
            // MULTIBLK: advance, wrapping ONCE back to cluster 2 — `visited`/`span` is what bounds
            // the loop now, so wrapping cannot spin. The sector cache is dropped on a wrap because
            // the scan jumps to a different part of the FAT.
            c += 1;
            if c >= last {
                c = 2;
                loaded = u64::MAX;
            }
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

    /// JD4: list ANY directory by its first cluster — `0` means the root (the value a subdirectory's
    /// `..` entry stores when its parent is the root, and the FAT16 fixed root's convention), else the
    /// cluster chain is walked exactly as a FAT32 root (`read_dir_chain`). The purely additive public
    /// face of the existing read walkers; takes NO lock because it is read-only (F3's namespace-lock
    /// arc may revisit read-side locking).
    pub fn read_dir(&self, first_cluster: u32) -> Result<alloc::vec::Vec<DirEntry>, FatError> {
        if first_cluster == 0 {
            self.read_root()
        } else {
            self.read_dir_chain(first_cluster)
        }
    }

    /// MULTIBLK: feed a directory's sectors to `visit` in disk order, reading them in CONTIGUOUS RUNS
    /// instead of one sector per USB round trip. `start_cluster` is `None` for the FAT16 fixed root
    /// (one contiguous run of `root_dir_sectors`, no chain at all) and `Some(c)` for a cluster-chain
    /// directory (the FAT32 root or any subdirectory). `visit(lba, sector)` returns true to STOP —
    /// which is how the "0x00 end-of-directory" terminator, a name match and a free-slot hit all
    /// terminate the walk without this function knowing what any of them mean.
    ///
    /// This replaces six near-identical hand-rolled walks (read/locate/free-slot × fixed-root/chain),
    /// each of which was one `read_sector` per directory sector PLUS one `fat_entry` sector read per
    /// cluster hop — i.e. two USB transactions per 512 bytes of directory on a 512-byte-cluster
    /// volume. `collect_chain` caches the FAT sector, so the hops now cost ~1 read for the whole
    /// chain, and the sectors themselves come back in runs.
    ///
    /// ### Why the chunk size GROWS instead of being fixed at the maximum
    /// A directory scan very often stops in its first sector — `locate_in_dir_chain` finding a name,
    /// `free_slot_in_dir_chain` finding the terminator. Reading `MAX_BLOCKS_PER_OP` sectors up front
    /// would make the common case fetch 64 sectors to look at one. Starting at one sector and
    /// doubling gives the early exit its old cost exactly, and a full scan a logarithmic number of
    /// transfers instead of a linear one — the right shape for both, with no caller having to choose.
    ///
    /// Guards are `collect_chain`'s, unchanged from the walks this replaces: a bad/free/out-of-range
    /// cluster is `BadChain`, a chain longer than the volume has clusters is `BadChain`, and an EOC
    /// simply ends the walk (the caller then reports `NotFound` / `NoSpace` / the entries it got).
    ///
    /// ONE deliberate divergence, on corrupt media only: because the chain is collected before the
    /// first sector is scanned, a directory whose 0x00 terminator sits early but whose FAT chain is
    /// damaged LATER now reports `BadChain`, where the old lazy walk would have stopped at the
    /// terminator and never seen the damage. That is the stricter direction — the volume really is
    /// corrupt — and it cannot arise on a well-formed one, where a chain is walked to its EOC.
    fn walk_dir_sectors(
        &self,
        start_cluster: Option<u32>,
        mut visit: impl FnMut(u64, &[u8; SECTOR_SIZE]) -> bool,
    ) -> Result<(), FatError> {
        // 1. The runs, as (first LBA, sector count). Consecutive clusters are consecutive LBAs, so a
        //    contiguously-allocated directory collapses to a single run.
        let runs: alloc::vec::Vec<(u64, u64)> = match start_cluster {
            None => alloc::vec![(self.root_dir_lba, self.root_dir_sectors as u64)],
            Some(start) => {
                let clusters = self.collect_chain(start, self.count_of_clusters as usize + 1)?;
                let spc = self.sec_per_clus as u64;
                let mut runs = alloc::vec::Vec::new();
                let mut i = 0usize;
                while i < clusters.len() {
                    let base = self.cluster_lba(clusters[i]);
                    let mut n = spc;
                    while i + 1 < clusters.len() && clusters[i + 1] == clusters[i] + 1 {
                        n += spc;
                        i += 1;
                    }
                    runs.push((base, n));
                    i += 1;
                }
                runs
            }
        };

        // 2. Walk them, growing the transfer size as the scan proves it is going to be a long one.
        //    The cap is DELIBERATELY below `MAX_BLOCKS_PER_OP`: this buffer is allocated per scan and
        //    directory scans are frequent (every path resolution runs one), so 8 KiB is the right
        //    trade — it already covers 256 directory slots per transfer, which is more than any
        //    directory this filesystem creates, and it keeps the per-scan allocation small.
        let cap = core::cmp::min(crate::drivers::block::MAX_BLOCKS_PER_OP, 16);
        let mut chunk = 1usize;
        let mut buf = alloc::vec![0u8; cap * SECTOR_SIZE];
        for (base, count) in runs {
            let mut done = 0u64;
            while done < count {
                let n = core::cmp::min(chunk as u64, count - done) as usize;
                self.rd_sectors(base + done, &mut buf[..n * SECTOR_SIZE])?;
                for k in 0..n {
                    let sec: &[u8; SECTOR_SIZE] = buf[k * SECTOR_SIZE..(k + 1) * SECTOR_SIZE]
                        .try_into()
                        .map_err(|_| FatError::Io)?;
                    if visit(base + done + k as u64, sec) {
                        return Ok(());
                    }
                }
                done += n as u64;
                chunk = core::cmp::min(chunk * 2, cap);
            }
        }
        Ok(())
    }

    /// FAT16 fixed root directory: a contiguous run of sectors, no cluster chain.
    fn read_fixed_root16(&self) -> Result<alloc::vec::Vec<DirEntry>, FatError> {
        let mut out = alloc::vec::Vec::new();
        let mut lfn = LfnBuf::new();
        self.walk_dir_sectors(None, |_lba, sec| scan_dir_sector(sec, &mut out, &mut lfn))?;
        Ok(out)
    }

    /// Walk a directory stored as a cluster chain (the FAT32 root, or any subdirectory), collecting
    /// its entries. Stops at the 0x00 terminator or end-of-chain; guards against bad/free clusters
    /// and a chain longer than the whole volume (loop protection) — all now inside `walk_dir_sectors`
    /// / `collect_chain`, so this and its five siblings cannot drift apart on them.
    fn read_dir_chain(&self, start: u32) -> Result<alloc::vec::Vec<DirEntry>, FatError> {
        let mut out = alloc::vec::Vec::new();
        let mut lfn = LfnBuf::new();
        self.walk_dir_sectors(Some(start), |_lba, sec| scan_dir_sector(sec, &mut out, &mut lfn))?;
        Ok(out)
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
    ///
    /// FATREAD-1: `out` is REPLACED, never appended to — on return `out` holds exactly the bytes
    /// read and nothing else, so `out.len()` is the read length for ANY caller. The body copies with
    /// `extend_from_slice`, which means a caller that hands in a NON-EMPTY buffer used to get its old
    /// contents with the file appended after them. Every caller that pre-sized its buffer from the
    /// directory (`vec![0u8; de.size]` — a natural reading of "read the file into this") therefore got
    /// a result of exactly `2 * de.size`, silently, with the file's real bytes sitting behind a run of
    /// zeros. That is the doubling that blocked `bg /fat/STAT.ELF` and `bg /fat/VUG.ELF` on x86: the
    /// directory said 8472 / 12568 and the loader was handed 16944 / 25136 — past the 16 KiB user
    /// window, so both were rejected as oversize. It read as time-dependent (early boot fine, later
    /// broken) only because the doubling is invisible until `2 * size` crosses a caller's cap: U2's
    /// 72-byte HELLO.BIN doubles to 144 and still fits, an 8472-byte ELF does not. Clearing here is
    /// the fix at the definition rather than at each call site, because the contract — not the
    /// callers — was what was ambiguous. It is a no-op for every caller that already passed an empty
    /// `Vec` (which is all of them on aarch64), so this is byte-inert on that arch.
    pub fn read_file(
        &self,
        de: &DirEntry,
        out: &mut alloc::vec::Vec<u8>,
        max_bytes: usize,
    ) -> Result<(), FatError> {
        out.clear(); // FATREAD-1: "into `out`" means REPLACE — see the doc note above.
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
        // MULTIBLK: collect the chain, then read it in contiguous runs. This is the path
        // `load_program_into_slot` uses for every user program on the volume, so it is exactly the
        // path the boot-time `FS: 8472 STAT.ELF` / `FS: 12568 VUG.ELF` reads run down: 17 and 25
        // sectors respectively, previously 17 and 25 separate READ(10)s plus one FAT sector read per
        // cluster hop. Guards unchanged and now single-sourced in `collect_chain`: bad/free cluster
        // and chain loop are `BadChain`, an early EOC returns the short read.
        let clus_bytes = self.sec_per_clus as usize * SECTOR_SIZE;
        let need = (remaining + clus_bytes - 1) / clus_bytes;
        let clusters = self.collect_chain(de.first_cluster, need)?;
        remaining = core::cmp::min(remaining, clusters.len().saturating_mul(clus_bytes));
        self.read_span(&clusters, 0, remaining, out)?;
        Ok(())
    }

    /// Read up to `max` bytes of a file into `out`, starting at byte offset `start` within the file,
    /// given the file's first cluster and byte size (the two fields U6b's `SYS_OPEN` records so a later
    /// `SYS_READ` need not re-scan the directory). Sequential-only: it walks the chain from the first
    /// cluster, skipping whole clusters/sectors up to `start`, then copies `min(max, size - start)`
    /// bytes. Read-only — never writes the FAT, directory, or data. Stops at `size`, `start + max`, or
    /// end-of-chain; guards against bad/free clusters and chain loops exactly as `read_file`.
    ///
    /// This is the offset-aware twin of `read_file`. ⚠ MULTIBLK (2026-07-29) SUPERSEDES the note that
    /// stood here — it said the two "share no code by design, not by divergence", because `read_file`
    /// was being held byte-identical for its M6g/U4 `load_program_into_slot` caller. They now share
    /// `collect_chain` + `read_span`, which is the stronger form of the same guarantee: the twins
    /// cannot diverge on bounds, on the short-read rule or on the FATREAD-1 replace contract, because
    /// there is only one implementation of each. `read_at(fc, size, 0, out, max)` still delivers
    /// exactly the bytes `read_file` would for a non-directory entry.
    ///
    /// FATREAD-1: `out` is REPLACED, never appended to — the same contract `read_file` now states,
    /// for the same reason, so the twins cannot diverge on it. Every current caller already hands in
    /// an empty `Vec`, so the clear is byte-inert on both arches; it exists so the next caller that
    /// reasonably pre-sizes its buffer does not silently get a double-length result.
    pub fn read_at(
        &self,
        first_cluster: u32,
        size: u32,
        start: u32,
        out: &mut alloc::vec::Vec<u8>,
        max: usize,
    ) -> Result<(), FatError> {
        out.clear(); // FATREAD-1: "into `out`" means REPLACE — see the doc note above.
        if start >= size {
            return Ok(()); // at or past EOF — nothing to read (a legal 0-byte result)
        }
        // Total bytes to deliver: from `start` to the earlier of EOF and `start + max`. `start < size`
        // above, so `end > start` and `want >= 1` here.
        let end = core::cmp::min(size as usize, (start as usize).saturating_add(max));
        let mut want = end - start as usize;
        if want == 0 {
            return Ok(()); // caller requested 0 bytes
        }
        if !self.valid_cluster(first_cluster) {
            return Err(FatError::BadChain);
        }
        let clus_bytes = self.sec_per_clus as usize * SECTOR_SIZE;
        // MULTIBLK: collect the chain up to the cluster the LAST wanted byte falls in, then read
        // spans over it. `skip` is no longer a running subtraction — `read_span` addresses the file
        // by byte offset directly, and clusters entirely before `start` are simply never visited
        // (the old loop's "skip it without touching the disk" is now "never index it"). Every guard
        // is preserved inside `collect_chain`: bad/free cluster and chain loop are `BadChain`, an
        // early EOC yields a short read.
        let need = (end + clus_bytes - 1) / clus_bytes;
        let clusters = self.collect_chain(first_cluster, need)?;
        let covered = clusters.len().saturating_mul(clus_bytes);
        if covered <= start as usize {
            return Ok(()); // the chain ends before `start` — a legal short (empty) read
        }
        want = core::cmp::min(want, covered - start as usize);
        self.read_span(&clusters, start as usize, want, out)?;
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
    /// MULTIBLK: walk the cluster chain from `first`, collecting up to `max_clusters` clusters in
    /// order, and CACHE the FAT sector across hops.
    ///
    /// Two things it buys over the lazy `fat_entry`-per-hop walk it replaces in the span callers:
    ///   * the chain arrives as a slice, which is what makes contiguity detectable at all — you
    ///     cannot coalesce a run you are discovering one element at a time; and
    ///   * a FAT32 sector holds 128 entries and a FAT16 sector 256, so a file laid down contiguously
    ///     by any formatter costs ONE FAT read for its whole chain instead of one per cluster. On the
    ///     QEMU fixture volumes (512-byte clusters) that alone dominates: a 12568-byte VUG.ELF is 25
    ///     clusters, i.e. 25 FAT sector reads before this and 1 after.
    ///
    /// Bounds are the read walkers' bounds, unchanged: a free/bad/out-of-range cluster is
    /// `BadChain`, a chain longer than the volume has clusters is `BadChain` (loop guard), and an
    /// EOC before `max_clusters` simply ends the walk — the caller sees a SHORT chain and short-reads
    /// or short-writes against it, exactly as the lazy walkers did.
    ///
    /// The cache is per-CALL and never outlives the walk, so it introduces no new coherence claim: a
    /// chain walk was never atomic with respect to a concurrent FAT mutation before this either.
    fn collect_chain(&self, first: u32, max_clusters: usize) -> Result<alloc::vec::Vec<u32>, FatError> {
        let mut out = alloc::vec::Vec::new();
        if max_clusters == 0 {
            return Ok(out);
        }
        if !self.valid_cluster(first) {
            return Err(FatError::BadChain);
        }
        let entry_bytes: u64 = if self.kind == FatKind::Fat32 { 4 } else { 2 };
        let mut buf = [0u8; SECTOR_SIZE];
        let mut loaded = u64::MAX; // which FAT sector `buf` currently holds (u64::MAX = none)
        let mut cluster = first;
        let mut hops = 0u32;
        loop {
            out.push(cluster);
            if out.len() >= max_clusters {
                return Ok(out);
            }
            let offset = cluster as u64 * entry_bytes;
            let sec = offset / SECTOR_SIZE as u64;
            if sec >= self.fat_sz as u64 {
                return Err(FatError::BadChain); // outside the FAT region — same guard as fat_entry_copy
            }
            if sec != loaded {
                self.rd_sector(self.fat_start + sec, &mut buf)?;
                loaded = sec;
            }
            let within = (offset % SECTOR_SIZE as u64) as usize;
            let next = match self.kind {
                FatKind::Fat16 => u16le(&buf, within) as u32,
                FatKind::Fat32 => u32le(&buf, within) & 0x0FFF_FFFF,
            };
            if self.is_eoc(next) {
                return Ok(out); // chain ended early — the caller short-reads/short-writes
            }
            if self.is_bad(next) || next < 2 || !self.valid_cluster(next) {
                return Err(FatError::BadChain);
            }
            cluster = next;
            hops += 1;
            if hops > self.count_of_clusters {
                return Err(FatError::BadChain); // longer than the volume has clusters -> a loop
            }
        }
    }

    /// MULTIBLK: how many sectors, starting at `clusters[ci]` sector `s0`, are CONSECUTIVE on disk.
    ///
    /// Within one cluster the answer is trivially "the rest of the cluster". Across clusters it holds
    /// only while the chain is physically contiguous, i.e. `clusters[i + 1] == clusters[i] + 1` —
    /// which for FAT is exactly the condition `cluster_lba(c + 1) == cluster_lba(c) + sec_per_clus`,
    /// since cluster LBAs are a linear function of the cluster number. Files a formatter has just
    /// laid down are usually one long contiguous run, so this is the common case, not the lucky one;
    /// a fragmented file simply gets several runs and still beats one transfer per sector.
    fn contiguous_sectors(&self, clusters: &[u32], ci: usize, s0: u64) -> u64 {
        let spc = self.sec_per_clus as u64;
        let mut n = spc - s0;
        let mut i = ci;
        while i + 1 < clusters.len() && clusters[i + 1] == clusters[i] + 1 {
            n += spc;
            i += 1;
        }
        n
    }

    /// MULTIBLK: write `data` into the byte range `[pos, pos + data.len())` of a file whose chain is
    /// `clusters` (element `i` covering file bytes `[i * clus_bytes, (i + 1) * clus_bytes)`).
    /// Returns the number of bytes written; a `pos` past what `clusters` covers is a short write.
    ///
    /// This is the shape that replaces the per-sector read-modify-write loop, and it splits the span
    /// into at most three pieces per contiguous run:
    ///   * a HEAD partial sector (only when `pos` is not sector-aligned) — one sector RMW, because
    ///     the bytes before `pos` inside that sector must survive;
    ///   * a BODY of whole sectors — issued as ONE counted `write_sectors` with NO preceding read at
    ///     all, since `data` covers every byte of it. This is where both wins land;
    ///   * a TAIL partial sector (only when fewer than 512 bytes remain) — one sector RMW, for the
    ///     mirror-image reason to the head.
    /// A partial sector still costs a read; that is not an oversight, it is the only way to preserve
    /// the untouched bytes, and it is why the two RMWs are kept and only the interior is optimised.
    fn write_span(&self, clusters: &[u32], start: usize, data: &[u8]) -> Result<usize, FatError> {
        let clus_bytes = self.sec_per_clus as usize * SECTOR_SIZE;
        let mut done = 0usize;
        let mut pos = start;
        let mut buf = [0u8; SECTOR_SIZE];
        while done < data.len() {
            let ci = pos / clus_bytes;
            let Some(&cluster) = clusters.get(ci) else {
                break; // the chain does not reach this far — a short write, never a write off-chain
            };
            if !self.valid_cluster(cluster) {
                return Err(FatError::BadChain);
            }
            let in_clus = pos % clus_bytes;
            let s0 = (in_clus / SECTOR_SIZE) as u64;
            let in_sec = in_clus % SECTOR_SIZE;
            let lba = self.cluster_lba(cluster) + s0;
            let remaining = data.len() - done;

            if in_sec != 0 {
                // HEAD: partial sector — read, patch, write back. Exactly the old loop body.
                let take = core::cmp::min(remaining, SECTOR_SIZE - in_sec);
                self.rd_sector(lba, &mut buf)?;
                buf[in_sec..in_sec + take].copy_from_slice(&data[done..done + take]);
                self.wr_sector(lba, &buf)?;
                done += take;
                pos += take;
                continue;
            }

            let full = (remaining / SECTOR_SIZE) as u64;
            if full == 0 {
                // TAIL: fewer than a whole sector left — read, patch, write back.
                self.rd_sector(lba, &mut buf)?;
                buf[..remaining].copy_from_slice(&data[done..]);
                self.wr_sector(lba, &buf)?;
                done += remaining;
                pos += remaining;
                continue;
            }

            // BODY: the longest run that is contiguous on disk, still covered by `data`, and still
            // inside the chain we were given. No read — `data` supplies every byte.
            let run = core::cmp::min(full, self.contiguous_sectors(clusters, ci, s0));
            let bytes = run as usize * SECTOR_SIZE;
            self.wr_sectors(lba, &data[done..done + bytes])?;
            done += bytes;
            pos += bytes;
        }
        Ok(done)
    }

    /// MULTIBLK: the read twin of [`FatFs::write_span`] — append `len` bytes from byte offset `start`
    /// of the file whose chain is `clusters` onto `out`. Returns the number of bytes delivered (short
    /// if the chain ends first). Reads coalesce over the same contiguous runs; there is no
    /// read-modify-write asymmetry here, so the only split is "partial sector" vs "whole-sector run".
    fn read_span(
        &self,
        clusters: &[u32],
        start: usize,
        len: usize,
        out: &mut alloc::vec::Vec<u8>,
    ) -> Result<usize, FatError> {
        let clus_bytes = self.sec_per_clus as usize * SECTOR_SIZE;
        let mut done = 0usize;
        let mut pos = start;
        let mut buf = [0u8; SECTOR_SIZE];
        while done < len {
            let ci = pos / clus_bytes;
            let Some(&cluster) = clusters.get(ci) else {
                break; // chain ended before `len` — return the short read, as the walkers always did
            };
            if !self.valid_cluster(cluster) {
                return Err(FatError::BadChain);
            }
            let in_clus = pos % clus_bytes;
            let s0 = (in_clus / SECTOR_SIZE) as u64;
            let in_sec = in_clus % SECTOR_SIZE;
            let lba = self.cluster_lba(cluster) + s0;
            let remaining = len - done;

            if in_sec != 0 || remaining < SECTOR_SIZE {
                // A partial sector at either end: one sector read, copy out the covered slice.
                let take = core::cmp::min(remaining, SECTOR_SIZE - in_sec);
                self.rd_sector(lba, &mut buf)?;
                out.extend_from_slice(&buf[in_sec..in_sec + take]);
                done += take;
                pos += take;
                continue;
            }

            let full = (remaining / SECTOR_SIZE) as u64;
            let run = core::cmp::min(full, self.contiguous_sectors(clusters, ci, s0));
            let bytes = run as usize * SECTOR_SIZE;
            // Extend `out` in place and read the whole run straight into it — one transfer.
            let at = out.len();
            out.resize(at + bytes, 0);
            self.rd_sectors(lba, &mut out[at..at + bytes])?;
            done += bytes;
            pos += bytes;
        }
        Ok(done)
    }

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

        // MULTIBLK: the lazy per-cluster `fat_entry` walk that used to live here has been replaced by
        // "collect the chain, then write spans over it". EVERY bound this function documents is
        // preserved, and each is now enforced in exactly one place:
        //   * never grows        — `end` is still clamped to `size` above, and `write_span` refuses to
        //                          step past the clusters it was handed;
        //   * never allocates    — `collect_chain` only READS the FAT; nothing here writes it;
        //   * never touches dirs — unchanged, there is no directory access in this function;
        //   * bad/free cluster, chain loop -> `BadChain` — `collect_chain` carries the identical
        //     guards (`is_bad`, `< 2`, `valid_cluster`, hop count vs `count_of_clusters`);
        //   * a chain that ends before `size` yields a SHORT write — `collect_chain` returns the short
        //     chain and `write_span` stops at its end, returning what it managed.
        let need = (end + clus_bytes - 1) / clus_bytes; // clusters the write's END byte reaches into
        let clusters = self.collect_chain(first_cluster, need)?;
        // Clamp to what the collected chain actually covers, so a truncated chain short-writes rather
        // than the caller being told bytes landed that never did.
        let covered = clusters.len().saturating_mul(clus_bytes);
        if covered <= start as usize {
            return Ok(0);
        }
        want = core::cmp::min(want, covered - start as usize);
        let wrote = self.write_span(&clusters, start as usize, &data[..want])?;
        debug_assert!(wrote <= total);
        Ok(wrote)
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
        self.locate_in_dir_sectors(None, name)
    }

    /// A directory stored as a cluster chain (the FAT32 root, or any subdirectory): the located twin of
    /// `read_dir_chain`. Same bounded walk + bad/free-cluster + loop guards.
    fn locate_in_dir_chain(&self, start: u32, name: &str) -> Result<(DirEntry, u64, usize), FatError> {
        self.locate_in_dir_sectors(Some(start), name)
    }

    /// MULTIBLK: the one implementation behind [`FatFs::locate_in_fixed_root16`] and
    /// [`FatFs::locate_in_dir_chain`] — they differed only in how they enumerated sectors, which is
    /// now [`FatFs::walk_dir_sectors`]'s job. Semantics are preserved exactly: the first 0x00 slot
    /// ends the directory and yields `NotFound` even if later sectors still hold data (that is what
    /// "end of directory" means on FAT), a 0xE5/LFN slot is skipped, and the first short-name match
    /// wins with its (LBA, slot-offset).
    fn locate_in_dir_sectors(
        &self,
        start_cluster: Option<u32>,
        name: &str,
    ) -> Result<(DirEntry, u64, usize), FatError> {
        let mut found: Option<(DirEntry, u64, usize)> = None;
        self.walk_dir_sectors(start_cluster, |lba, sec| {
            for i in 0..(SECTOR_SIZE / 32) {
                match classify_dir_slot(&sec[i * 32..i * 32 + 32]) {
                    DirSlot::End => return true, // end of directory — stop, `found` stays None
                    DirSlot::Skip => continue,
                    DirSlot::Entry(de) => {
                        if de.eq_name(name) {
                            found = Some((de, lba, i * 32));
                            return true;
                        }
                    }
                }
            }
            false
        })?;
        found.ok_or(FatError::NotFound)
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
        // F3-M2: the whole sector RMW under DIR_MUTATION — a concurrent RMW of a NEIGHBOURING slot in this
        // same sector can no longer read-before-our-write and clobber this publish (or vice versa).
        with_dir_lock_src(self.source, "write_dir_entry_fields", || {
            let mut buf = [0u8; SECTOR_SIZE];
            self.rd_sector(lba, &mut buf)?;
            let hi = (first_cluster >> 16) as u16;
            let lo = (first_cluster & 0xFFFF) as u16;
            buf[off + 20..off + 22].copy_from_slice(&hi.to_le_bytes());
            buf[off + 26..off + 28].copy_from_slice(&lo.to_le_bytes());
            buf[off + 28..off + 32].copy_from_slice(&size.to_le_bytes());
            self.wr_sector(lba, &buf)?;
            Ok(())
        })
    }

    /// JD17: the mtime-stamping sibling of [`FatFs::write_dir_entry_fields`] — the same single-sector
    /// RMW publish of `first_cluster` + `size`, additionally refreshing the last-write time/date words
    /// (@0x16/@0x18) from the kernel wall clock IN THE SAME sector write (no extra I/O, no second
    /// crash window). While the clock is UNSET the existing on-disk words are left untouched — a
    /// host-stamped file rewritten by a clockless kernel keeps its old stamp rather than being zeroed
    /// (strictly less destructive than fabricating or erasing). Used by `write_grow`'s step-4 publish
    /// — the CONTENT-mutation path; `rename_entry`/`move_entry`/`create_dir`'s publish keep the plain
    /// sibling (a rename/move preserves mtime, and a fresh dir entry was already stamped at create).
    fn write_dir_entry_fields_mtime(
        &self,
        lba: u64,
        off: usize,
        first_cluster: u32,
        size: u32,
    ) -> Result<(), FatError> {
        if off + 32 > SECTOR_SIZE {
            return Err(FatError::Io); // a slot never straddles a sector; a bad offset is a caller bug
        }
        with_dir_lock_src(self.source, "write_dir_entry_fields_mtime", || {
            let mut buf = [0u8; SECTOR_SIZE];
            self.rd_sector(lba, &mut buf)?;
            let hi = (first_cluster >> 16) as u16;
            let lo = (first_cluster & 0xFFFF) as u16;
            buf[off + 20..off + 22].copy_from_slice(&hi.to_le_bytes());
            buf[off + 26..off + 28].copy_from_slice(&lo.to_le_bytes());
            buf[off + 28..off + 32].copy_from_slice(&size.to_le_bytes());
            let (mt, md) = crate::clock::fat_stamp();
            if (mt, md) != (0, 0) {
                buf[off + 22..off + 24].copy_from_slice(&mt.to_le_bytes());
                buf[off + 24..off + 26].copy_from_slice(&md.to_le_bytes());
            }
            self.wr_sector(lba, &buf)?;
            Ok(())
        })
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
        //    MULTIBLK: this was a per-sector read-modify-write loop, and §12.1 counted it as 129
        //    READ(10) + 129 WRITE(10) for a single 66048-byte flight-recorder reservation. `write_span`
        //    keeps the RMW for the head and tail partial sectors — those bytes outside the write MUST
        //    be preserved — and issues the whole-sector interior as counted, contiguous, READ-FREE
        //    writes. Step 2 has just guaranteed the chain covers `[0, needed * clus_bytes)`, so
        //    `write_span` can never run off its end here; it returns a short count only if handed a
        //    short chain, which is why the total is checked below rather than assumed.
        let written = self.write_span(&chain, start as usize, data)?;
        if written != data.len() {
            // The chain we just ensured covers the range did not, in fact, cover it. That is a
            // corrupt-volume / lost-race condition, not a legal short write for a GROWING write:
            // reporting fewer bytes while step 4 below publishes `new_size` would claim a size the
            // data does not back. Fail instead, leaving the OLD size on disk (the safe order).
            return Err(FatError::BadChain);
        }

        // 4. LAST: publish size (+ chain head if it changed) to the directory — data + FAT already
        //    durable. JD17: the _mtime sibling also refreshes the last-write stamp in this same RMW.
        self.write_dir_entry_fields_mtime(dir_lba, dir_off, new_first, new_size)?;
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
        // F3-M2: the slot WRITE is a sector RMW under DIR_MUTATION (the free-slot SCAN above stays outside —
        // per the with_dir_lock span rule; the scan-then-claim slot race itself is the F3-M3 namespace lock's).
        // JD6: this with_dir_lock slot-write body is TWINNED VERBATIM in `create_in_dir` — keep the two in sync.
        with_dir_lock_src(self.source, "create_in_root", || {
            let mut buf = [0u8; SECTOR_SIZE];
            self.rd_sector(lba, &mut buf)?;
            // Write a fresh 32-byte entry: 11-byte name, attr, everything else zero (NTRes/times = 0,
            // first_cluster hi@20 lo@26 = 0, size@28 = 0). Never set the volume-label bit — a file/dir entry.
            for b in buf[off..off + 32].iter_mut() {
                *b = 0;
            }
            buf[off..off + 11].copy_from_slice(&raw);
            buf[off + 11] = attr & !0x08;
            // JD17: stamp the last-write time/date words (@0x16/@0x18) from the kernel wall clock.
            // (0, 0) while the clock is unset — byte-identical to the pre-JD17 zeroed field.
            let (mt, md) = crate::clock::fat_stamp();
            buf[off + 22..off + 24].copy_from_slice(&mt.to_le_bytes());
            buf[off + 24..off + 26].copy_from_slice(&md.to_le_bytes());
            self.wr_sector(lba, &buf)?;
            // Re-parse the slot we just wrote so the returned DirEntry is byte-for-byte what a reader sees.
            match classify_dir_slot(&buf[off..off + 32]) {
                DirSlot::Entry(de) => Ok((de, lba, off)),
                _ => Err(FatError::Io), // unreachable: we just wrote a valid non-empty, non-LFN entry
            }
        })
    }

    // =============================================================================================
    // JD6 (seat-granted narrow additive exception, round 6 2026-07-11) — DIR-AWARE write entry points.
    //
    // Two ADDITIVE public wrappers that generalize the root-only `find_located` / `create_in_root`
    // twins to an arbitrary directory identified by its first data cluster (`first_cluster == 0` ⇒
    // the volume root, dispatching straight back to the root twin). They add NO new traversal or
    // mutation logic: they reuse the already-general PRIVATE chain-walkers the FAT32-root path
    // itself exercises (`locate_in_dir_chain`, `free_slot_in_dir_chain`). Every mutation rides
    // `DIR_MUTATION`/`FAT_MUTATION` exactly as the root twins do. Consumed by the aarch64 panel
    // shell's subdir write path (shell.rs); no x86 caller passes a non-zero cluster today.
    // (fat.rs mutation is the pi4-K1 lane — this block is a seat-approved exception, zero edits to
    //  existing fns, placed adjacent to its root twins for review.)
    // =============================================================================================

    /// JD6: locate `name` in the directory whose data begins at `first_cluster` (`0` ⇒ the volume
    /// root). The dir-aware twin of [`FatFs::find_located`]; for a subdirectory it is the existing
    /// private `locate_in_dir_chain` — a read-only bounded directory walk (no lock).
    pub fn locate_in_dir(
        &self,
        first_cluster: u32,
        name: &str,
    ) -> Result<(DirEntry, u64, usize), FatError> {
        if first_cluster == 0 {
            self.find_located(name)
        } else {
            self.locate_in_dir_chain(first_cluster, name)
        }
    }

    /// JD6: create a fresh 0-length entry `name` in the directory at `first_cluster` (`0` ⇒ the
    /// volume root ⇒ [`FatFs::create_in_root`]). The dir-aware twin of `create_in_root`: the free
    /// slot comes from the existing private `free_slot_in_dir_chain` (a FULL subdirectory — no free
    /// slot; extending a subdir's cluster chain is out of scope this arc — is an honest `NoSpace`),
    /// and the slot WRITE below is a VERBATIM copy of `create_in_root`'s `with_dir_lock` RMW.
    /// ⚠ TWIN — keep the `with_dir_lock` body in sync with `create_in_root` (the seat review diffs
    /// the two). Allocates NO clusters and touches NO FAT — only the one directory sector. The
    /// caller must have confirmed the name is absent (this does not de-duplicate).
    pub fn create_in_dir(
        &self,
        first_cluster: u32,
        name: &str,
        attr: u8,
    ) -> Result<(DirEntry, u64, usize), FatError> {
        if first_cluster == 0 {
            return self.create_in_root(name, attr);
        }
        let raw = format_83(name).ok_or(FatError::Unsupported)?;
        let (lba, off) = self.free_slot_in_dir_chain(first_cluster)?;
        // F3-M2: the slot WRITE is a sector RMW under DIR_MUTATION — the free-slot SCAN above stays
        // outside, exactly as in `create_in_root`.
        // ⚠ VERBATIM TWIN of `create_in_root`'s with_dir_lock block — keep in sync (seat review diffs these).
        with_dir_lock_src(self.source, "create_in_dir", || {
            let mut buf = [0u8; SECTOR_SIZE];
            self.rd_sector(lba, &mut buf)?;
            // Write a fresh 32-byte entry: 11-byte name, attr, everything else zero (NTRes/times = 0,
            // first_cluster hi@20 lo@26 = 0, size@28 = 0). Never set the volume-label bit — a file/dir entry.
            for b in buf[off..off + 32].iter_mut() {
                *b = 0;
            }
            buf[off..off + 11].copy_from_slice(&raw);
            buf[off + 11] = attr & !0x08;
            // JD17: stamp the last-write time/date words (@0x16/@0x18) from the kernel wall clock.
            // (0, 0) while the clock is unset — byte-identical to the pre-JD17 zeroed field.
            let (mt, md) = crate::clock::fat_stamp();
            buf[off + 22..off + 24].copy_from_slice(&mt.to_le_bytes());
            buf[off + 24..off + 26].copy_from_slice(&md.to_le_bytes());
            self.wr_sector(lba, &buf)?;
            // Re-parse the slot we just wrote so the returned DirEntry is byte-for-byte what a reader sees.
            match classify_dir_slot(&buf[off..off + 32]) {
                DirSlot::Entry(de) => Ok((de, lba, off)),
                _ => Err(FatError::Io), // unreachable: we just wrote a valid non-empty, non-LFN entry
            }
        })
    }

    // =============================================================================================
    // FATDIRS (pi4-lane, round 8 — seat-granted additive exception, sibling of JD6's block above):
    // directory CREATE / REMOVE. Two new public methods + one private helper, placed adjacent to the
    // JD6 dir-aware twins, with ZERO edits to any existing fn. They COMPOSE the reviewed primitives —
    // `alloc_cluster` (compare-and-claim under FAT_MUTATION), `create_in_dir`/`locate_in_dir` (the JD6
    // twins), `write_dir_entry_fields`/`read_dir`/`delete_located` — each of which already rides
    // `FAT_MUTATION`/`DIR_MUTATION`. Consumed by the aarch64 panel's `mkdir`/`rmdir` (JD7) AFTER this
    // arc merges: call, never edit.
    //
    // LOCKING (invariant 5 — SOUND WITHOUT the syscall-layer NAMESPACE lock, because kernel shell callers
    // reach fat.rs directly): every SECTOR mutation is SMP-atomic via the existing per-RMW locks, and
    // `DIR_MUTATION` is never widened past its documented single-sector-RMW span (holding it across a
    // directory SCAN or across `free_chain`'s block I/O would break the IRQ-masked-non-preemptible span
    // rule its doc-comment fixes). The COMPOSITE scan->mutate sequences are therefore NOT held under one
    // lock. That leaves exactly ONE honest-scope residual, ledgered in SECURITY.md like F3's
    // two-cores-mid-syscall interleave: `remove_dir`'s emptiness-scan -> `delete_located` is not atomic
    // against a concurrent `create_in_dir` INTO the same target directory (a file linked between the
    // scan and the free is orphaned in a freed chain). EXCLUDED_BY_SEQUENCING today — the only FS
    // mutators are the single-threaded kernel panel shell, user syscalls (serialized by the syscall
    // NAMESPACE lock), and the await-verdict-sequenced reaper; none races another FS mutator. The fix
    // when concurrent kernel mutators appear is a fat.rs namespace lock spanning both sequences — a future
    // seam change that would have to touch `create_in_dir`, so it is out of this additive grant.
    // =============================================================================================

    /// FATDIRS: write the two mandatory entries into a freshly allocated directory cluster — `.` (self,
    /// `first_cluster = self_cluster`) and `..` (parent, `first_cluster = parent_first_cluster`; `0` when
    /// the parent is the volume root, per the FAT convention the read walkers already honour). Both are
    /// DIR-attr (`0x10`), size 0. The cluster is a zero-filled UNLINKED orphan (`alloc_cluster` just
    /// claimed + zeroed it), so the bytes after `..` stay `0x00` — a correct end-of-directory terminator
    /// — and NO lock is needed: no other core can reach the cluster until `create_dir` publishes it into
    /// the parent (the same reason `alloc_cluster`'s zero-fill writes unlocked).
    fn init_subdir_cluster(&self, self_cluster: u32, parent_first_cluster: u32) -> Result<(), FatError> {
        let mut buf = [0u8; SECTOR_SIZE];
        // "." = 0x2E + ten spaces (name bytes 0..11); attr 0x10; first_cluster = self.
        buf[0] = b'.';
        for b in &mut buf[1..11] {
            *b = b' ';
        }
        buf[11] = 0x10; // ATTR_DIRECTORY
        buf[20..22].copy_from_slice(&((self_cluster >> 16) as u16).to_le_bytes());
        buf[26..28].copy_from_slice(&((self_cluster & 0xFFFF) as u16).to_le_bytes());
        // ".." = two 0x2E + nine spaces (name bytes 32..43); attr 0x10; first_cluster = parent (0 = root).
        buf[32] = b'.';
        buf[33] = b'.';
        for b in &mut buf[34..43] {
            *b = b' ';
        }
        buf[43] = 0x10;
        buf[52..54].copy_from_slice(&((parent_first_cluster >> 16) as u16).to_le_bytes());
        buf[58..60].copy_from_slice(&((parent_first_cluster & 0xFFFF) as u16).to_le_bytes());
        // CLOCK-3: stamp the last-write time/date words (@0x16/@0x18) of BOTH `.` and `..` from the
        // unified kernel clock, the same path every other fat.rs writer uses — a fresh subdir now carries
        // a real mtime instead of the all-zero (dashed) placeholder. `(0, 0)` while the clock is unset,
        // so the unset-boot on-disk bytes are byte-identical to the pre-CLOCK-3 zeroed field.
        let (mt, md) = crate::clock::fat_stamp();
        buf[22..24].copy_from_slice(&mt.to_le_bytes()); //  "."  time @0x16
        buf[24..26].copy_from_slice(&md.to_le_bytes()); //  "."  date @0x18
        buf[54..56].copy_from_slice(&mt.to_le_bytes()); //  ".." time @0x16 (entry base 32 + 22)
        buf[56..58].copy_from_slice(&md.to_le_bytes()); //  ".." date @0x18 (entry base 32 + 24)
        // size@28..32 and @60..64 stay 0 (directories report size 0); the rest of the cluster is already
        // zero (alloc_cluster), so this one sector fully initializes the directory.
        self.wr_sector(self.cluster_lba(self_cluster), &buf)
    }

    /// FATDIRS: create a subdirectory `name` in the directory at `parent_first_cluster` (`0` ⇒ the volume
    /// root). Allocates ONE cluster for the child, initializes it with the mandatory `.`/`..` entries,
    /// then links a fresh DIR-attr (`0x10`) entry in the parent and publishes the child cluster into it.
    /// Returns the PARENT's new directory entry with its on-disk (LBA, slot-offset) — the shape
    /// `create_in_dir` returns, so a caller (JD7) can hang a K-lineage ACL row on the entry.
    ///
    /// CRASH ORDERING (invariant 2 — init the child BEFORE linking the parent): the child cluster is
    /// fully allocated + `.`/`..`-initialized before the parent entry is written, and the parent link is
    /// itself two writes — `create_in_dir` writes a 0-cluster DIR entry, then `write_dir_entry_fields`
    /// publishes the child cluster (the last write). A crash/power-loss fails SAFE at every step:
    ///   * before/during the child init  -> an orphaned (leaked) cluster, chkdsk-reclaimable;
    ///   * after `create_in_dir`, before the publish -> a DIR entry with `first_cluster == 0` (the
    ///     JD6-ledgered FstClus==0 corner — a `cd` into it would list the root via `read_dir(0)`) plus
    ///     the child cluster leaked. Malformed but NEVER a live entry pointing at a cluster that later
    ///     gets freed/aliased — the safe-failure invariant holds.
    ///
    /// Mirrors `create_in_dir`'s de-dup contract: the CALLER confirms `name` is absent first (as
    /// shell.rs's `fs_touch` does for files) — this does not de-duplicate. Errors: `Unsupported` (name
    /// not a representable 8.3 short name), `NoSpace` (no free cluster, or the parent directory is full —
    /// subdir-chain extension is out of scope), `Io`/`BadChain`/`NoDisk` from the primitives.
    pub fn create_dir(
        &self,
        parent_first_cluster: u32,
        name: &str,
    ) -> Result<(DirEntry, u64, usize), FatError> {
        // Validate the name BEFORE any allocation, so a bad name leaks no cluster.
        let _ = format_83(name).ok_or(FatError::Unsupported)?;
        // 1. Allocate + zero-fill the child cluster (compare-and-claim under FAT_MUTATION; EOC-terminated,
        //    UNLINKED — unreachable by any reader until step 3/4 publish it).
        let child = self.alloc_cluster()?;
        // 2. Initialize `.`/`..` in the child. MUST complete before the parent link (crash order). On
        //    failure, release the just-claimed cluster so nothing leaks.
        if let Err(e) = self.init_subdir_cluster(child, parent_first_cluster) {
            let _ = self.set_fat_entry(child, 0);
            return Err(e);
        }
        // 3. Link the parent: a fresh 0-cluster DIR entry (create_in_dir's slot scan + DIR_MUTATION RMW).
        let (_, lba, off) = match self.create_in_dir(parent_first_cluster, name, 0x10) {
            Ok(t) => t,
            Err(e) => {
                let _ = self.set_fat_entry(child, 0); // release the child (nothing points at it yet)
                return Err(e);
            }
        };
        // 4. Publish the child cluster into the parent entry (DIR_MUTATION RMW); size stays 0. LAST write.
        //    A failure here leaves the fail-safe `first_cluster == 0` corner documented above.
        self.write_dir_entry_fields(lba, off, child, 0)?;
        // Re-read the finished entry so the returned DirEntry is byte-for-byte what a reader sees.
        match self.locate_in_dir(parent_first_cluster, name) {
            Ok(t) => Ok(t),
            Err(_) => Err(FatError::Io), // unreachable: we just created + published it
        }
    }

    /// FATDIRS: remove the EMPTY subdirectory `name` from the directory at `parent_first_cluster` (`0` ⇒
    /// the volume root). Locates the entry, refuses a non-directory target and a `first_cluster == 0`
    /// target (a root-like / malformed 0-cluster dir — the volume root is never nameable and must never
    /// be freed), verifies the directory holds ONLY `.` and `..` (the `read_dir` walk), then deletes it
    /// exactly as a file (`delete_located` = mark the parent entry `0xE5`, THEN free the chain). Returns
    /// the freed clusters (the `delete_located` shape).
    ///
    /// Errors (existing `FatError` variants — the ENOTDIR/ENOTEMPTY fidelity gap is ledgered for JD7):
    ///   * `NotFound`    -> `name` is absent in the parent (caller: -ENOENT);
    ///   * `Unsupported` -> the target is NOT a directory, or is a `first_cluster == 0` root-like entry
    ///                      (caller: -ENOTDIR / -EBUSY-or-EINVAL for root-refusal);
    ///   * `IsDirectory` -> the directory is NOT empty (holds an entry beyond `.`/`..`) (caller: today
    ///                      -EISDIR; -ENOTEMPTY once `FatError` grows a variant, which touches the
    ///                      jetson-lane shell.rs errno map — a future seam change, not this arc);
    ///   * `Io`/`BadChain`/`NoDisk` propagate from the primitives.
    ///
    /// CRASH ORDERING: `delete_located` marks the parent entry `0xE5` FIRST, then frees the chain — a
    /// crash leaves the directory GONE with its cluster still marked used (a benign leaked cluster),
    /// never a live entry pointing at a freed/aliased cluster.
    ///
    /// CONCURRENCY (invariant 3): the emptiness-scan -> `delete_located` is NOT atomic against a
    /// concurrent `create_in_dir` into THIS target (check-then-delete TOCTOU). Honest-scope,
    /// EXCLUDED_BY_SEQUENCING today (no concurrent kernel FS mutators; user sequences ride the syscall
    /// NAMESPACE lock) — ledgered in SECURITY.md alongside F3's residual; see the FATDIRS block comment.
    pub fn remove_dir(
        &self,
        parent_first_cluster: u32,
        name: &str,
    ) -> Result<alloc::vec::Vec<u32>, FatError> {
        let (de, lba, off) = self.locate_in_dir(parent_first_cluster, name)?;
        if !de.is_dir {
            return Err(FatError::Unsupported); // not a directory (caller: -ENOTDIR)
        }
        let child = de.first_cluster();
        if child == 0 {
            // A 0-cluster dir entry (the JD6 FstClus==0 corner) or a root-like target: refuse. read_dir(0)
            // lists the ROOT and "cluster 0" is not freeable — the root is never removed.
            return Err(FatError::Unsupported); // (caller: -EBUSY/-EINVAL for root-refusal)
        }
        // Emptiness: the child must hold ONLY `.` and `..`. Any third real entry -> refuse.
        for e in self.read_dir(child)? {
            let n = e.name();
            if n != "." && n != ".." {
                return Err(FatError::IsDirectory); // not empty (caller: -ENOTEMPTY)
            }
        }
        // Delete exactly as a file: 0xE5 the parent entry FIRST, then free the child's chain.
        self.delete_located(lba, off, child)
    }

    // =============================================================================================
    // FATMOVE (pi4-lane, round 9 — seat-granted additive exception, sibling of the FATDIRS block
    // above): directory-entry RENAME + cross-directory MOVE. Two new public methods + one private
    // single-sector helper, placed adjacent to FATDIRS, with ZERO edits to any existing fn. They
    // COMPOSE the reviewed primitives — `locate_in_dir`/`create_in_dir` (JD6 twins),
    // `write_dir_entry_fields`/`mark_dir_deleted` (each already riding `DIR_MUTATION`) — plus one new
    // single-sector helper (`write_dir_entry_name`, which rewrites JUST the 11-byte 8.3 name field in
    // place). Consumed by a future jetson `mv` arc (JD10) AFTER this arc merges: call, never edit.
    //
    // THE GENUINELY-NEW PRIMITIVE is MOVE's "unlink the source entry WITHOUT freeing the chain": a
    // move relinks a file's directory entry into a new parent over the SAME `first_cluster`/`size`,
    // so the data clusters move BY REFERENCE (no alloc, no copy, no free). It is `mark_dir_deleted`
    // (0xE5 the source name) but NOT `free_chain` — the chain stays live under the destination name.
    //
    // CRASH ORDERING (invariant 2 — NEVER lose the chain): MOVE publishes the DESTINATION entry FIRST
    // (`create_in_dir` writes a 0-cluster entry, then `write_dir_entry_fields` publishes the shared
    // chain head + size), and only THEN marks the source entry `0xE5`. A crash/power-loss between the
    // two leaves a benign DUPLICATE: two directory entries pointing at the SAME chain (the source's
    // original name and the destination's new name). Both are readable; the operator removes the
    // unwanted one BY ITS ENTRY (a plain `rm OLDNAME`). The reverse order (`0xE5` the source first)
    // could ORPHAN the chain if the crash landed before the destination published it — forbidden. FAT
    // is not journaled, so the window is fundamental; the bias is fail-SAFE (a leaked duplicate name,
    // never a lost or aliased chain).
    //
    // DIRECTORIES (invariant 3): `rename_entry` renames a directory IN PLACE — only the parent's entry
    // name field changes; the directory's own `.` (self) and its children's `..` (parent) links point
    // at `first_cluster` values a rename does NOT touch, so they stay correct. MOVE of a directory
    // ACROSS parents is REFUSED (`IsDirectory`): it would require rewriting the moved directory's `..`
    // entry to the new parent, which is out of this additive arc's scope.
    //
    // LOCKING (invariants 4/5 — SOUND WITHOUT the syscall-layer NAMESPACE lock, the FATDIRS bar):
    // every SECTOR mutation rides the existing per-RMW `DIR_MUTATION` span, and `DIR_MUTATION` is
    // NEVER widened to span both of MOVE's two dir-sector RMWs at once (its documented contract is
    // single-sector; cross-sector atomicity of the two writes is EXCLUDED_BY_SEQUENCING for kernel
    // callers). The composite locate->mutate sequences are therefore NOT held under one lock; the
    // residual is the SAME class FATDIRS ledgered (no concurrent kernel FS mutators today; user rides the
    // syscall NAMESPACE lock) — see SECURITY.md's FATMOVE entry.
    //
    // U6/ACL (invariant 5): `fat.rs` is ACL-blind by layering — the `OWNED_FILES` ACL keys by
    // `(dir_lba, dir_off)` up in aarch64 `syscall.rs`. A rename/move CHANGES that key, so a future
    // user rename/move path MUST re-key or refuse an OWNED file (ledgered in SECURITY.md + the JD10
    // brief). This arc builds NO user-mode plumbing; the kernel panel runs as ASID 0 (the PUBLIC principal),
    // so a panel-driven rename/move touches no ACL row.
    //
    // ERRNO FIDELITY: the seam reuses existing `FatError` variants (adding one would break the
    // exhaustive `fat_errno` match in the jetson-lane `shell.rs` — the FATDIRS precedent). The
    // dest-EXISTS refusal reuses `Unsupported` (shared with a bad 8.3 name); the CALLER confirms the
    // destination is absent first (as `fs_touch` does for create) and surfaces `-EEXIST` locally, so
    // the seam's `Unsupported`-on-exists is a defensive backstop that never writes a duplicate. The
    // dir-target refusal uses `IsDirectory` (the `-EISDIR`-equivalent the JD10 baton names). This
    // mirrors how JD7's `rmdir` maps `IsDirectory`->`-ENOTEMPTY` locally per call site.
    // =============================================================================================

    /// FATMOVE: rewrite JUST the 11-byte 8.3 name field (bytes 0..11 of the 32-byte slot) of the
    /// directory entry at (`lba`, `off`), preserving attr/cluster/size/timestamps. A SINGLE
    /// directory-sector read-modify-write under `DIR_MUTATION` — exactly the documented single-sector
    /// span (the twin of `mark_dir_deleted`, which RMWs byte 0 of a slot). The caller supplies the
    /// already-validated `format_83` raw name, so the rewritten slot re-parses to the same textual
    /// name a reader's `classify_dir_slot` produces.
    fn write_dir_entry_name(&self, lba: u64, off: usize, raw: &[u8; 11]) -> Result<(), FatError> {
        if off + 32 > SECTOR_SIZE {
            return Err(FatError::Io); // a slot never straddles a sector; a bad offset is a caller bug
        }
        with_dir_lock_src(self.source, "write_dir_entry_name", || {
            let mut buf = [0u8; SECTOR_SIZE];
            self.rd_sector(lba, &mut buf)?;
            buf[off..off + 11].copy_from_slice(raw);
            self.wr_sector(lba, &buf)?;
            Ok(())
        })
    }

    /// FATMOVE: rename the entry `old_leaf` to `new_leaf` IN PLACE within the directory at
    /// `parent_first_cluster` (`0` ⇒ the volume root) — rewrite the 8.3 name in the existing directory
    /// entry; `first_cluster`, `size`, and `attr` are unchanged (a SINGLE dir-sector RMW). Works on
    /// BOTH files and directories: an in-place rename leaves `first_cluster` untouched, so a renamed
    /// directory's own `.` and its children's `..` links stay correct (only a MOVE across parents
    /// would disturb `..` — see `move_entry`). Returns the entry at its (unchanged) location, now
    /// bearing `new_leaf` — the `create_in_dir` shape (so a JD10 caller can re-key an ACL row).
    ///
    /// Errors (existing `FatError` variants — see the FATMOVE block's errno-fidelity note):
    ///   * `Unsupported` -> `new_leaf` is not a representable 8.3 name;
    ///   * `NotFound`    -> `old_leaf` is absent in the parent (caller: -ENOENT);
    ///   * `Unsupported` -> `new_leaf` ALREADY EXISTS at a DIFFERENT slot (the dest-exists backstop —
    ///                      the caller confirms absence first and surfaces -EEXIST locally; this seam
    ///                      also refuses defensively so it NEVER writes a duplicate name);
    ///   * `Io`/`BadChain`/`NoDisk` propagate from the primitives.
    /// A rename to the SAME canonical 8.3 name (e.g. `foo.txt` -> `FOO.TXT`, which resolve to the same
    /// on-disk slot) is a no-op success.
    pub fn rename_entry(
        &self,
        parent_first_cluster: u32,
        old_leaf: &str,
        new_leaf: &str,
    ) -> Result<(DirEntry, u64, usize), FatError> {
        // Validate the new name up front so a bad name mutates nothing.
        let raw = format_83(new_leaf).ok_or(FatError::Unsupported)?;
        // Locate the source (NotFound if absent).
        let (_de, lba, off) = self.locate_in_dir(parent_first_cluster, old_leaf)?;
        // Dest-exists check (locate-first, the create discipline). A hit at the SAME slot means the
        // new canonical name == the old one -> a no-op rename (return the entry as-is); a hit at a
        // DIFFERENT slot -> refuse (would duplicate the name).
        match self.locate_in_dir(parent_first_cluster, new_leaf) {
            Ok((_, nlba, noff)) => {
                if nlba == lba && noff == off {
                    return self.locate_in_dir(parent_first_cluster, new_leaf); // already this name
                }
                return Err(FatError::Unsupported); // dest exists (caller: -EEXIST via its pre-check)
            }
            Err(FatError::NotFound) => {}
            Err(e) => return Err(e),
        }
        // Single dir-sector RMW: rewrite the name field in place.
        self.write_dir_entry_name(lba, off, &raw)?;
        // Re-read the finished entry so the returned DirEntry is byte-for-byte what a reader sees.
        match self.locate_in_dir(parent_first_cluster, new_leaf) {
            Ok(t) => Ok(t),
            Err(_) => Err(FatError::Io), // unreachable: we just wrote it
        }
    }

    /// FATMOVE: move the FILE entry `leaf` from the directory at `src_parent` to the directory at
    /// `dst_parent` (each `0` ⇒ the volume root), naming it `new_leaf` there. The file's data moves
    /// BY REFERENCE: the destination entry is written over the SAME `first_cluster`/`size`, then the
    /// source entry is marked deleted WITHOUT freeing the chain — no cluster is allocated, copied, or
    /// freed. Returns the NEW entry with its (LBA, slot-offset) — the `create_in_dir` shape.
    ///
    /// CRASH ORDERING (invariant 2 — NEVER lose the chain): the destination entry is fully published
    /// (`create_in_dir` -> `write_dir_entry_fields`, the shared chain head + size) BEFORE the source
    /// entry is `0xE5`'d. A crash between the two leaves a benign DUPLICATE (two names, one chain); the
    /// reverse order could orphan the chain. See the FATMOVE block comment for the full analysis.
    ///
    /// DIRECTORIES: refused (`IsDirectory`) — moving a directory across parents needs its `..` entry
    /// rewritten to the new parent, out of this additive arc's scope. (Rename a directory IN PLACE
    /// with `rename_entry`, which does not disturb `..`.)
    ///
    /// Errors (existing `FatError` variants — see the FATMOVE block's errno-fidelity note):
    ///   * `Unsupported` -> `new_leaf` is not a representable 8.3 name;
    ///   * `NotFound`    -> `leaf` absent in `src_parent` (caller: -ENOENT);
    ///   * `IsDirectory` -> `leaf` is a directory (caller: -EISDIR — cross-parent dir move unsupported);
    ///   * `Unsupported` -> `new_leaf` already exists in `dst_parent` (the dest-exists backstop — the
    ///                      caller confirms absence first and surfaces -EEXIST locally);
    ///   * `NoSpace`     -> `dst_parent` has no free slot; `Io`/`BadChain`/`NoDisk` from the primitives.
    pub fn move_entry(
        &self,
        src_parent: u32,
        leaf: &str,
        dst_parent: u32,
        new_leaf: &str,
    ) -> Result<(DirEntry, u64, usize), FatError> {
        // Validate the new name up front so a bad name mutates nothing (create_in_dir re-validates).
        if format_83(new_leaf).is_none() {
            return Err(FatError::Unsupported);
        }
        // Locate the source (NotFound if absent).
        let (src_de, src_lba, src_off) = self.locate_in_dir(src_parent, leaf)?;
        if src_de.is_dir {
            return Err(FatError::IsDirectory); // cross-parent directory move is out of scope (`..` rewrite)
        }
        // The chain head (`0` for a legitimately EMPTY 0-length file — a valid, movable case: an empty
        // file simply carries no clusters, so the "move the chain by reference" is a no-op relink and
        // there is no chain to lose).
        let head = src_de.first_cluster();
        // Dest-exists check (locate-first). Across different parents no self-collision is possible;
        // within the SAME parent a hit means `new_leaf` canon == `leaf` canon (a same-name move —
        // the caller routes those to `rename_entry`). Any hit -> refuse; NotFound -> proceed.
        match self.locate_in_dir(dst_parent, new_leaf) {
            Ok(_) => return Err(FatError::Unsupported), // dest exists (caller: -EEXIST via its pre-check)
            Err(FatError::NotFound) => {}
            Err(e) => return Err(e),
        }
        // Preserve the source's exact attribute byte (read-only/hidden/system/archive) — a faithful
        // move. A plain read of the source slot; the mutations below take their own DIR_MUTATION locks.
        let attr = {
            let mut sbuf = [0u8; SECTOR_SIZE];
            self.rd_sector(src_lba, &mut sbuf)?;
            sbuf[src_off + 11]
        };
        // 1. Publish the DESTINATION entry FIRST (crash order). `create_in_dir` writes a fresh
        //    0-cluster entry (its own DIR_MUTATION slot RMW); a full `dst_parent` is an honest NoSpace.
        let (_, dlba, doff) = self.create_in_dir(dst_parent, new_leaf, attr)?;
        // 2. Publish the SHARED chain head + size into the destination entry (DIR_MUTATION RMW). BOTH
        //    the source and destination names now point at the same chain (a transient duplicate).
        self.write_dir_entry_fields(dlba, doff, head, src_de.size)?;
        // 3. LAST: mark the SOURCE entry deleted (`0xE5`) WITHOUT freeing the chain — the chain stays
        //    live under the destination name. This is the genuinely-new "unlink-keep-chain" step
        //    (`mark_dir_deleted`, NOT `delete_located`, which would `free_chain`).
        self.mark_dir_deleted(src_lba, src_off)?;
        // Re-read the finished destination entry so the returned DirEntry is what a reader sees.
        match self.locate_in_dir(dst_parent, new_leaf) {
            Ok(t) => Ok(t),
            Err(_) => Err(FatError::Io), // unreachable: we just published it
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
        // MULTIBLK: `NoSpace` on exhaustion, exactly as before — a fixed root cannot be extended.
        self.free_slot_in_dir_sectors(None)
    }

    /// MULTIBLK: the one implementation behind [`FatFs::free_slot_in_fixed_root16`] and
    /// [`FatFs::free_slot_in_dir_chain`]. Unchanged rule: the first slot whose first byte is 0x00
    /// (end marker) or 0xE5 (deleted) wins. Writing into the first 0x00 slot still preserves the
    /// terminator, because every slot after it is also 0x00.
    fn free_slot_in_dir_sectors(&self, start_cluster: Option<u32>) -> Result<(u64, usize), FatError> {
        let mut found: Option<(u64, usize)> = None;
        self.walk_dir_sectors(start_cluster, |lba, sec| {
            for i in 0..(SECTOR_SIZE / 32) {
                let b0 = sec[i * 32];
                if b0 == 0x00 || b0 == 0xE5 {
                    found = Some((lba, i * 32));
                    return true;
                }
            }
            false
        })?;
        found.ok_or(FatError::NoSpace)
    }

    /// A directory stored as a cluster chain. `NoSpace` when the chain holds no free slot — extending
    /// a directory chain remains out of scope, exactly as before MULTIBLK.
    fn free_slot_in_dir_chain(&self, start: u32) -> Result<(u64, usize), FatError> {
        self.free_slot_in_dir_sectors(Some(start))
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
        // F3-M2: the sector RMW under DIR_MUTATION — a racing size-publish RMW of a sibling slot in this
        // sector can no longer resurrect the `0xE5` (lost-delete), nor this delete clobber its publish.
        with_dir_lock_src(self.source, "mark_dir_deleted", || {
            let mut buf = [0u8; SECTOR_SIZE];
            self.rd_sector(dir_lba, &mut buf)?;
            buf[dir_off] = 0xE5;
            self.wr_sector(dir_lba, &buf)?;
            Ok(())
        })
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
                self.rd_sector(self.fat_start + sec, &mut buf)?;
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
            // BPACE: the boot volume is readable. `d=` from `stor-ready` is the BPB + FAT read cost
            // — the first real filesystem I/O of the boot, and the gate every fixture waits on.
            // Inside `probe_once`'s one-shot, so it can only ever record once.
            crate::bootpace::record("fat-mount");
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

/// SDHC-4b: one-shot boot probe for the INTERNAL SD card — the witness that says whether the block
/// backend added by this arc actually reaches a filesystem.
///
/// Mirrors [`probe_once`] exactly (a `PROBED` latch, a registry check that no-ops until the device is
/// there, then one mount and one report), and it runs from the x86 main loop for the same reason
/// PIUSB-27's mount does: it must fire with no driver lock held, and `sdhc::bring_up` holds the card
/// lock through its own witnesses.
///
/// **This is a READER and nothing else.** It mounts, prints, lists the root, and drops the `FatFs`.
/// It creates no file, reserves nothing, and writes no sector — see `BlockSource::Sdhc` and
/// `flight_recorder.rs` §SINGLE FAT WRITER for why that is load-bearing rather than merely modest.
///
/// It is able to say NO in three distinguishable ways, which is the point of printing all of them:
/// * no line at all → `register_sdhc` never ran, i.e. no card was identified (the `[sdhc]` bring-up
///   lines above it in the log say why);
/// * `no FAT volume … (NotFat)` → the card is readable but carries no BPB this reader accepts. The
///   `:: PART: mbr-raw handle=sdhc …` census that `mount_source` emits just above is the raw evidence;
/// * `no FAT volume … (Io)` → the registry has the card but a read of LBA 0 failed, which contradicts
///   the bring-up read witnesses and is a real finding about the driver, not about the medium.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
pub fn sdhc_probe_once() {
    static PROBED: AtomicBool = AtomicBool::new(false);
    if PROBED.load(Ordering::Relaxed) {
        return;
    }
    let Some(dev) = crate::drivers::block::sdhc_info() else {
        return; // no card registered under the Sdhc handle (yet, or at all this boot)
    };
    PROBED.store(true, Ordering::Relaxed);

    let size_mib = dev.num_blocks.saturating_mul(dev.block_size as u64) / (1024 * 1024);
    match mount_source(BlockSource::Sdhc) {
        Ok(fs) => {
            serial_println!(
                ":: SDHCBLK: FAT mounted READ-ONLY on the internal SD card ({} MiB): {} ::",
                size_mib, fs.describe()
            );
            match fs.read_root() {
                Ok(entries) => {
                    serial_println!(
                        ":: SDHCBLK: sdhc root directory ({} entries) ::",
                        entries.len()
                    );
                    for de in &entries {
                        if de.is_dir {
                            serial_println!(":: SDHCBLK:   <DIR>              {} ::", de.name());
                        } else {
                            serial_println!(":: SDHCBLK:   {:>12}       {} ::", de.size, de.name());
                        }
                    }
                }
                Err(e) => serial_println!(
                    ":: SDHCBLK: sdhc root directory read error ({:?}) ::", e
                ),
            }
            // SDHC-4c: the reserve pass runs HERE, on the mount this function already has, for two
            // reasons. (1) Exclusivity: `sdhc_probe_once` is called from the x86 main loop ahead of
            // `flight_recorder::service()` and every `U*_probe_once` on the same pass, so no other
            // FAT writer of any volume can be running — the same program-order argument
            // `flight_recorder.rs` §SINGLE FAT WRITER makes for roster row 1, inherited verbatim.
            // (2) Cost: a second `mount_source` would re-read the MBR and BPB for nothing.
            // It is still a one-shot: `PROBED` above latched before the mount.
            if crate::fs::sdhc4c::claim_reserve_pass() {
                if let Some((first, sectors)) = fs.sdhc4c_reserve() {
                    fs.sdhc4c_write_verify(first, sectors);
                }
                // The tally closes the pass on EVERY outcome, so "armed and wrote", "refused" and
                // "the pass never ran" are three distinguishable states in a capture rather than
                // two states and a silence.
                crate::fs::sdhc4c::tally();
            }
        }
        Err(e) => {
            serial_println!(
                ":: SDHCBLK: no FAT volume on the internal SD card ({} MiB, {:?}) ::", size_mib, e
            );
            // SDHC-4c: no volume means no reservation to adopt. Say so — a silent skip here would
            // be indistinguishable from a pass that ran and found nothing.
            //
            // WITNESS HONESTY: the three ways a mount fails are three different findings about
            // three different layers, and one shared reason ("no FAT volume") made them read as the
            // same one. They are separated here so a capture never has to be re-flown to learn
            // which layer said no. The FOURTH state — the internal reader has no medium at all —
            // never reaches this arm: `sdhc_probe_once` returns above without latching `PROBED`
            // when `sdhc_info()` is `None`, so an empty slot prints NO `SDHCBLK:`/`SDHC4C:` line of
            // any kind and the reserve pass does not run. Absence of the whole block is that
            // state's signature, and the `[sdhc]` bring-up lines say why the card was never
            // registered.
            if crate::fs::sdhc4c::claim_reserve_pass() {
                crate::fs::sdhc4c::disarm(
                    crate::fs::sdhc4c::RESERVE_NAME,
                    match e {
                        // The handle was published when the probe latched and is gone now — a
                        // medium/registry disappearance, NOT a filesystem verdict.
                        FatError::NoDisk => {
                            "the internal SD backend has NO registered medium — the card handle \
                             vanished between the probe and the mount; nothing was searched"
                        }
                        // A read of LBA 0 failed. This says nothing about what the card contains.
                        FatError::Io => {
                            "reading LBA 0 of the internal SD card FAILED — this is a driver/medium \
                             read failure, NOT evidence about the card's contents"
                        }
                        // The card read fine and simply is not a filesystem this reader mounts.
                        _ => {
                            "a medium IS present and readable on the internal SD card, but it \
                             carries no FAT volume this reader accepts (see the PART mbr-raw census \
                             above) — no root directory was searched"
                        }
                    },
                );
                crate::fs::sdhc4c::tally();
            }
        }
    }
}

// ===================== SDHC-4c — the reserve-once flight-recorder writer =====================
//
// The FIRST persistent write UnaOS makes. Read `fs/sdhc4c.rs` first: it owns the permit, the
// writable sector set, and the argument for why that set is closed. This section owns the pass that
// derives the set and then exercises it, in four steps that are deliberately in this order:
//
//   RESERVE -> ARM (+ self-test) -> WRITE -> READ BACK
//
// ADOPT-ONLY is the whole safety idea. The kernel does not create, grow, delete, rename or truncate
// the reserved file, on this volume or any other: it LOCATES a file the host staged, proves the
// chain it already has is one contiguous run wholly inside the data region, and publishes that run
// as the writable set. Every one of `create_in_root`, `write_grow`, `delete_located` and
// `alloc_cluster` remains unreachable with `source == Sdhc`, so the card acquires a writer that is
// not a FAT mutator — the same class the flight recorder became on the boot volume, minus the
// bootstrap window the recorder still has to reason about (`flight_recorder.rs` §SINGLE FAT
// WRITER, cases B and C). There is no bootstrap window here because there is no bootstrap.
//
// Every failure lands on `sdhc4c::disarm`, which is permanent for the boot and leaves the card in
// exactly the SDHC-4b state: mounted, readable, and unwritable. The worst outcome this pass has is
// "refused, nothing written".

/// SDHC-4c: how many bytes the verify pass writes and reads back — one whole sector, at offset 0 of
/// the reserved file. A whole aligned sector is chosen so `write_span` needs no read-modify-write
/// and the card sees exactly one CMD24, which makes `cmd24=1` a falsifiable prediction rather than
/// an approximation.
#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
const SDHC4C_RECORD_BYTES: usize = SECTOR_SIZE;

#[cfg(all(target_arch = "x86_64", feature = "sdhcblk"))]
impl FatFs {
    /// SDHC-4c: ONE bounded line saying which volume the reserve pass searched and what its root
    /// walk actually saw. Printed only on the not-found / lookup-failed path, so a healthy boot pays
    /// nothing and a refusal is self-explanatory in the capture without a second boot.
    ///
    /// Three facts the bare `NotFound` could not carry, each of which has cost a boot:
    ///   * **volume identity** — kind, extent, label and `BS_VolID`. The internal SDHCI slot and the
    ///     medium this kernel booted from are separate block handles; a refusal that does not name
    ///     the volume cannot distinguish "the host staged nothing" from "the host staged onto the
    ///     other device".
    ///   * **read failure vs genuine absence** — the walk's `Result` is reported verbatim. A
    ///     truncated or failed sector read stops a walk early and is NOT evidence of absence.
    ///   * **where the walk stopped** — the 0x00 end-of-directory slot index, or `none` if the walk
    ///     ran off the end of the chain. A terminator at slot 0 means an empty (or unreadable-as-
    ///     directory) root, which reads very differently from a terminator after 200 entries.
    ///
    /// Bounded by construction: at most `WITNESS_NAMES` short names are rendered, counters are
    /// `u32`, and the whole thing is one line issued at most once per boot (the reserve pass is a
    /// one-shot). It re-walks the root rather than instrumenting `locate_in_dir_sectors`, so the
    /// hot lookup path every path resolution runs stays exactly as it was.
    fn sdhc4c_root_witness(&self, name: &str) {
        /// How many short names the line carries. Eight is what fits alongside the identity fields
        /// without wrapping the FTDI ring's useful width, and is enough to recognise a staging set.
        const WITNESS_NAMES: u32 = 8;

        let start = match self.kind {
            FatKind::Fat32 => Some(self.root_cluster),
            FatKind::Fat16 => None,
        };
        let mut sectors = 0u32;
        let mut slots = 0u32;
        let mut entries = 0u32;
        let mut shown = 0u32;
        let mut term: Option<u32> = None;
        let mut names = String::new();

        let walk = self.walk_dir_sectors(start, |_lba, sec| {
            sectors += 1;
            for i in 0..(SECTOR_SIZE / 32) {
                match classify_dir_slot(&sec[i * 32..i * 32 + 32]) {
                    DirSlot::End => {
                        term = Some(slots);
                        return true;
                    }
                    DirSlot::Skip => slots += 1,
                    DirSlot::Entry(de) => {
                        slots += 1;
                        entries += 1;
                        if shown < WITNESS_NAMES {
                            if shown > 0 {
                                names.push(' ');
                            }
                            names.push_str(de.short_name());
                            shown += 1;
                        }
                    }
                }
            }
            false
        });

        let label = self.label();
        serial_println!(
            ":: SDHC4C-ROOT: NAME={} not matched on vol=FAT{}@LBA{} volsec={} label={} \
             serial=0x{:08x} | walk: read={} sectors={} slots={} entries={} terminator={} | \
             first{}: {} ::",
            name,
            match self.kind {
                FatKind::Fat16 => 16,
                FatKind::Fat32 => 32,
            },
            self.part_lba,
            self.vol_sectors,
            if label.is_empty() { "-" } else { label.as_str() },
            self.vol_id,
            match walk {
                Ok(()) => String::from("OK"),
                // A failed walk means the counters below are a PREFIX, not a census — say so in the
                // same field that carries the error, so the two can never be read apart.
                Err(e) => alloc::format!("FAILED({:?})-counts-are-a-PREFIX", e),
            },
            sectors,
            slots,
            entries,
            match term {
                Some(at) => alloc::format!("slot#{}", at),
                None => String::from("none(ran-off-the-end)"),
            },
            shown,
            if names.is_empty() { "(none)" } else { names.as_str() },
        );
    }

    /// SDHC-4c step 1+2: ADOPT the host-staged reservation and publish its LBA extent, or refuse.
    ///
    /// Returns the `(first_cluster, sectors)` of the armed extent on success. Every `return` before
    /// the `arm` call has already named its reason on the wire through `disarm`.
    fn sdhc4c_reserve(&self) -> Option<(u32, u64)> {
        use crate::fs::sdhc4c as permit;
        const NAME: &str = crate::fs::sdhc4c::RESERVE_NAME;

        // The pass is only ever driven for the card, but state that as a check rather than as a
        // comment: a future caller that hands it the boot volume must be refused, not trusted.
        if self.source != BlockSource::Sdhc {
            permit::disarm(NAME, "internal error: reserve pass invoked on a non-Sdhc volume");
            return None;
        }

        // --- locate. ADOPT-ONLY: absent is a refusal, never a create. ---
        let de = match self.find_located(NAME) {
            Ok((de, _lba, _slot)) => de,
            Err(FatError::NotFound) => {
                // SDHC4C-ROOT: "not found" alone cannot say WHICH volume was searched, and on this
                // machine that is the whole question — the internal SDHCI slot and the boot medium
                // are two different devices, and the host stages onto one of them. Name the volume
                // and what the walk saw before refusing. Boot AR (2026-08-08) refused here while
                // the staged file sat on the boot volume: the Sdhc handle held a 29 MiB FAT16 card
                // (11 entries, no UNALOG.BIN) and the 59.5 GB FAT32 card was mounted on `Default`.
                self.sdhc4c_root_witness(NAME);
                permit::disarm(
                    NAME,
                    "absent from the root directory of the volume mounted on the internal SD card \
                     (identified on the SDHC4C-ROOT line above) — the HOST stages this file; the \
                     kernel never creates it, because a create is a directory mutation",
                );
                return None;
            }
            Err(e) => {
                serial_println!(
                    ":: SDHC4C: reserve NAME={} lookup failed ({:?}) ::", NAME, e
                );
                self.sdhc4c_root_witness(NAME);
                permit::disarm(
                    NAME,
                    "root-directory lookup FAILED — this is a read/geometry failure, NOT evidence \
                     that the file is absent (SDHC4C-ROOT above says where the walk stopped)",
                );
                return None;
            }
        };
        if de.is_dir {
            permit::disarm(NAME, "the name is a DIRECTORY on this card, not a file");
            return None;
        }
        if de.size < crate::fs::sdhc4c::RESERVE_BYTES {
            serial_println!(
                ":: SDHC4C: reserve NAME={} size={} < required {} ::",
                NAME, de.size, crate::fs::sdhc4c::RESERVE_BYTES
            );
            permit::disarm(
                NAME,
                "the staged file is SHORTER than the reservation; adopting it would mean growing \
                 it later, and a grow is a FAT mutation",
            );
            return None;
        }
        let first = de.first_cluster;
        if !self.valid_cluster(first) {
            serial_println!(
                ":: SDHC4C: reserve NAME={} first_cluster={} is not a valid data cluster (2..{}) ::",
                NAME, first, self.count_of_clusters + 2
            );
            permit::disarm(NAME, "the staged file has no valid cluster chain head");
            return None;
        }

        // --- walk the chain the file ALREADY has. `collect_chain` only READS the FAT. ---
        // The extent covers exactly the clusters `RESERVE_BYTES` needs, NOT the whole file: it is
        // the tightest closed set, and it agrees with the `size` the writer below passes to
        // `write_at`, so the two independent bounds (LBA extent, byte clamp) describe the same
        // region rather than one being slack against the other.
        let clus_bytes = self.sec_per_clus as usize * SECTOR_SIZE;
        let need = (crate::fs::sdhc4c::RESERVE_BYTES as usize).div_ceil(clus_bytes);
        let clusters = match self.collect_chain(first, need) {
            Ok(c) => c,
            Err(e) => {
                serial_println!(":: SDHC4C: reserve NAME={} chain walk failed ({:?}) ::", NAME, e);
                permit::disarm(NAME, "the staged file's cluster chain is malformed");
                return None;
            }
        };
        if clusters.len() < need {
            serial_println!(
                ":: SDHC4C: reserve NAME={} chain covers {} clusters, needs {} ::",
                NAME, clusters.len(), need
            );
            permit::disarm(NAME, "the cluster chain ends before the reservation does");
            return None;
        }
        // CONTIGUITY. Not an optimisation: a single run is what lets the writable set be ONE
        // interval, and one interval is what makes the bound a single comparison that a reader can
        // check by eye. A fragmented file is refused rather than described by a list of ranges.
        let mut runs = 1usize;
        for i in 1..clusters.len() {
            if clusters[i] != clusters[i - 1] + 1 {
                runs += 1;
            }
        }
        if runs != 1 {
            serial_println!(
                ":: SDHC4C: reserve NAME={} cluster={} size={} runs={} contiguous=0 ::",
                NAME, first, de.size, runs
            );
            permit::disarm(
                NAME,
                "the staged file is FRAGMENTED; the permit describes exactly one LBA interval",
            );
            return None;
        }

        // --- prove the interval before publishing it. Four independent bounds. ---
        let nsec = need as u64 * self.sec_per_clus as u64;
        let a = self.cluster_lba(clusters[0]);
        let b = a + nsec;

        // (1) THE load-bearing one: the extent starts at or after the first data sector. On FAT the
        // boot sector, the reserved sectors, both FAT copies and the FAT16 fixed root directory all
        // live BELOW `data_start`, so this single inequality puts every one of them permanently out
        // of reach of a permitted write — whatever the chain walk returned, and whether or not the
        // BPB is honest about anything else.
        if a < self.data_start {
            serial_println!(
                ":: SDHC4C: reserve NAME={} lba={} is BELOW data_start={} ::",
                NAME, a, self.data_start
            );
            permit::disarm(NAME, "the derived extent reaches metadata sectors");
            return None;
        }
        // (2) and it ends at or before the last addressable data sector.
        let data_end = self.data_start + self.count_of_clusters as u64 * self.sec_per_clus as u64;
        if b > data_end {
            serial_println!(
                ":: SDHC4C: reserve NAME={} end={} is PAST the data region end={} ::",
                NAME, b, data_end
            );
            permit::disarm(NAME, "the derived extent runs past the data region");
            return None;
        }
        // (3) the volume's and the partition's own extents — two separate on-disk claims, checked
        // by the same `in_extent` every read of this volume is checked against.
        if let Err(e) = self.in_extent(a, nsec) {
            serial_println!(
                ":: SDHC4C: reserve NAME={} extent=[{}..{}) rejected by in_extent ({:?}) ::",
                NAME, a, b, e
            );
            permit::disarm(NAME, "the derived extent leaves the volume or the partition");
            return None;
        }
        // (4) the device's own capacity, asked of the block layer rather than derived from the BPB.
        let dev_blocks = crate::drivers::block::sdhc_info()
            .map(|d| d.num_blocks)
            .unwrap_or(0);
        if b > dev_blocks {
            serial_println!(
                ":: SDHC4C: reserve NAME={} end={} is PAST the card's num_blocks={} ::",
                NAME, b, dev_blocks
            );
            permit::disarm(NAME, "the derived extent runs past the end of the card");
            return None;
        }

        if !permit::arm(
            NAME,
            first,
            de.size,
            runs,
            a,
            b,
            crate::drivers::block::boot_volume_serial(),
            self.vol_id,
        ) {
            return None;
        }
        // The bound, tested through the bound's own predicate, before anything is written.
        permit::selftest_bounds();
        if !permit::armed() {
            return None; // the self-test disarmed it; it printed why
        }
        Some((first, nsec))
    }

    /// SDHC-4c step 3+4: write one record in place at offset 0 of the reserved extent and READ IT
    /// BACK, checksumming both. An echo that cannot fail proves nothing, so the verdict is a
    /// comparison of two FNV-1a hashes over bytes that made a round trip through the card.
    #[cfg(feature = "sdw")]
    fn sdhc4c_write_verify(&self, first: u32, _sectors: u64) {
        use crate::fs::sdhc4c as permit;
        let (a, b) = permit::extent();

        // The record. Fixed length, one whole sector, ASCII, newline-terminated so a host `head -c
        // 512` on the file is readable without tooling. `cy=` is the raw cycle counter — the only
        // clock this pass can be sure of — so two boots produce different bytes and a stale-file
        // read cannot be mistaken for a fresh write.
        let body = alloc::format!(
            ":: SDHC4C-REC: unaos first-persistent-write cy={} cluster={} lba=[{}..{}) vol=0x{:08x} ::\n",
            crate::arch::now_cycles(),
            first,
            a,
            b,
            self.vol_id
        );
        let mut rec = alloc::vec![b' '; SDHC4C_RECORD_BYTES];
        let n = core::cmp::min(body.len(), SDHC4C_RECORD_BYTES);
        rec[..n].copy_from_slice(&body.as_bytes()[..n]);
        rec[SDHC4C_RECORD_BYTES - 1] = b'\n';
        let want = permit::fnv1a(&rec);

        // `size` is RESERVE_BYTES, not the file's on-disk size: `write_at` clamps to it, so the
        // writer's byte bound and the permit's LBA bound describe the same region.
        let wrote = match self.write_at(first, crate::fs::sdhc4c::RESERVE_BYTES, 0, &rec) {
            Ok(w) => w,
            Err(e) => {
                serial_println!(
                    ":: SDHC4C: in-place write FAILED ({:?}) lba=[{}..{}) — nothing is claimed to \
                     have landed ::",
                    e, a, b
                );
                return;
            }
        };
        if wrote != rec.len() {
            serial_println!(
                ":: SDHC4C: in-place write SHORT wrote={} of {} lba=[{}..{}) ::",
                wrote, rec.len(), a, b
            );
            return;
        }

        // READ BACK from the medium. Same extent, same offset, through the ordinary read path.
        let mut back = alloc::vec::Vec::new();
        if let Err(e) = self.read_at(
            first,
            crate::fs::sdhc4c::RESERVE_BYTES,
            0,
            &mut back,
            SDHC4C_RECORD_BYTES,
        ) {
            serial_println!(
                ":: SDHC4C: read-back FAILED ({:?}) — the write is UNVERIFIED lba=[{}..{}) ::",
                e, a, b
            );
            return;
        }
        let got = permit::fnv1a(&back);
        if back.len() == rec.len() && got == want && back[..] == rec[..] {
            serial_println!(
                ":: SDHC4C: in-place write ok bytes={} lba=[{}..{}) readback=MATCH fnv=0x{:08x} ::",
                wrote, a, b, got
            );
        } else {
            serial_println!(
                ":: SDHC4C: !! read-back MISMATCH bytes={} got={} fnv-want=0x{:08x} \
                 fnv-got=0x{:08x} lba=[{}..{}) — the card did NOT return what was written ::",
                wrote, back.len(), want, got, a, b
            );
        }
    }

    /// SDHC-4c: without `sdw` this image contains no CMD24 ladder at all (SDHC-4a's property, and
    /// the default x86 polarity), so the honest report is that the permit armed and the write leg is
    /// absent — not silence, and not a fabricated success.
    #[cfg(not(feature = "sdw"))]
    fn sdhc4c_write_verify(&self, _first: u32, _sectors: u64) {
        let (a, b) = crate::fs::sdhc4c::extent();
        serial_println!(
            ":: SDHC4C: in-place write SKIPPED lba=[{}..{}) — this build carries no `sdw` feature, \
             so it contains no CMD24 ladder for the internal SD card; the permit is armed and the \
             extent is published, but nothing can write to it (UNAOS_SDW=1 arms the write leg) ::",
            a, b
        );
    }
}

/// PIUSB-27: map a [`FatError`] to a short human reason for the mount witness line.
#[cfg(target_arch = "aarch64")]
pub fn fat_reason(e: FatError) -> &'static str {
    match e {
        FatError::NoDisk => "no USB block device",
        FatError::Unsupported => "unsupported sector geometry",
        FatError::Io => "block I/O error",
        FatError::NotFat => "no FAT partition/BPB found",
        FatError::BadChain => "corrupt FAT chain",
        FatError::NotFound => "entry not found",
        FatError::IsDirectory => "is a directory",
        _ => "mount failed",
    }
}

/// PIUSB-27: mount the USB stick's FAT volume and emit the storage-ready witness. Called from
/// the xHCI storage bring-up event (`service_storage`) so it fires ONCE per bring-up and again on every
/// hot-plug re-enumeration. The mount reads through [`BlockSource::Usb`] (the xHCI direct path), so it works
/// even when the microSD owns the global block device. USBFALL F3: that source is WRITABLE since USB-WRITE —
/// this particular witness only reads, but nothing about `Usb` makes it read-only. On success it prints the
/// geometry (FAT type / size / cluster size) and the first-level entry list; on failure an honest reason.
/// The live `GET /fs/usb` HTTP route re-mounts per request — this line is the boot/hot-plug evidence.
/// PIUSB-27: main-loop hook — when the xHCI bring-up has raised the USB storage-ready edge, mount the
/// stick's FAT volume and emit the witness (USBFALL F3: the mount is writable since USB-WRITE; only this
/// witness path is read-only in practice, because it merely reads geometry and the root listing). Runs with the xHCI controller lock RELEASED (the
/// mount re-locks it briefly through `read_block_usb`), exactly like `probe_once`; the edge re-arms on
/// every hot-plug re-enumeration, so a re-inserted stick is re-mounted and re-witnessed. Safe to call
/// every main-loop iteration — it no-ops until the edge is raised, then fires once per raise.
#[cfg(target_arch = "aarch64")]
pub fn piusb27_service() {
    if crate::drivers::block::take_usb_ready() {
        piusb27_mount_witness();
    }
}

/// PIUSB-27: mount the USB stick's FAT volume and emit the storage-ready witness — the
/// geometry (FAT type / size / cluster size) and the first-level entry list on success, an honest reason
/// on failure. The live `GET /fs/usb` HTTP route re-mounts per request; this line is the boot/hot-plug
/// evidence. The mount reads through [`BlockSource::Usb`] (the xHCI direct path), so it works even when the
/// microSD owns the global block device. USBFALL F3: this witness READS only — the `Usb` source itself has
/// been writable since USB-WRITE.
#[cfg(target_arch = "aarch64")]
pub fn piusb27_mount_witness() {
    match mount_source(BlockSource::Usb) {
        Ok(fs) => {
            let kind = match fs.kind() {
                FatKind::Fat16 => 16,
                FatKind::Fat32 => 32,
            };
            let (bs, nb) = match crate::drivers::block::usb_info() {
                Some(d) => (d.block_size as u64, d.num_blocks),
                None => (SECTOR_SIZE as u64, 0),
            };
            let size_mib = nb.saturating_mul(bs) / (1024 * 1024);
            serial_println!(
                ":: piusb27: mounted FAT{} {} MiB cluster-size={}B as /fs/usb ::",
                kind, size_mib, fs.cluster_size()
            );
            match fs.read_root() {
                Ok(entries) => {
                    let mut list = String::new();
                    for de in &entries {
                        if list.len() > 240 {
                            list.push_str(" …");
                            break;
                        }
                        if !list.is_empty() {
                            list.push_str(", ");
                        }
                        if de.is_dir {
                            list.push_str(de.name());
                            list.push('/');
                        } else {
                            list.push_str(de.name());
                        }
                    }
                    serial_println!(
                        ":: piusb27: /fs/usb root: {} entries [{}] ::",
                        entries.len(), list
                    );
                    // PI-FS-3: descend the tree to prove arbitrary-depth subdirectory traversal (root →
                    // subdir → nested…). Bounded depth guards a malformed self-referential volume.
                    piusb27_walk_subtree(&fs, &entries, "/fs/usb", 0);
                }
                Err(e) => serial_println!(":: piusb27: root read error ({}) ::", fat_reason(e)),
            }
            // PI-FS-5: prove the SHELL's `ls /usb` collector sees the same mount (the `:: ls1: /usb... ::`
            // witness), so the shell and the `/fs/usb` HTTP route never disagree on the namespace.
            crate::shell::pi_usb_ls_witness();
        }
        Err(e) => serial_println!(":: piusb27: no FAT volume ({}) ::", fat_reason(e)),
    }
}

/// PI-FS-3: recursively list every subdirectory of `entries` (already-listed contents of the directory
/// at `prefix`), emitting one witness line per subdirectory with its full path and entry list — the
/// proof that traversal reaches arbitrary depth (FAT16 fixed root / FAT32 root chain / subdir cluster
/// chains all resolve through `read_dir`). Skips the `.`/`..` self/parent links so the walk terminates,
/// and caps depth at 8 as a belt-and-braces guard against a malformed self-referential volume (on top of
/// `read_dir_chain`'s own chain-loop guard). aarch64-only, mirroring the witness it extends.
#[cfg(target_arch = "aarch64")]
fn piusb27_walk_subtree(fs: &FatFs, entries: &[DirEntry], prefix: &str, depth: u32) {
    if depth >= 8 {
        return;
    }
    for de in entries {
        if !de.is_dir {
            continue;
        }
        let nm = de.name();
        if nm == "." || nm == ".." {
            continue; // self/parent links — do not recurse
        }
        let path = alloc::format!("{}/{}", prefix, nm);
        match fs.read_dir(de.first_cluster()) {
            Ok(sub) => {
                let mut list = String::new();
                for e in &sub {
                    if list.len() > 200 {
                        list.push_str(" …");
                        break;
                    }
                    if !list.is_empty() {
                        list.push_str(", ");
                    }
                    list.push_str(e.name());
                    if e.is_dir {
                        list.push('/');
                    }
                }
                serial_println!(
                    ":: piusb27: {} ({} entries) [{}] ::",
                    path, sub.len(), list
                );
                piusb27_walk_subtree(fs, &sub, &path, depth + 1);
            }
            Err(e) => serial_println!(":: piusb27: {} read error ({}) ::", path, fat_reason(e)),
        }
    }
}
