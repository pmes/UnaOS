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

//! HOLOCRON — the kernel-side classed-record store (BT-BOND M1).
//!
//! # What this is, and what it is not
//!
//! `handlers/holocron/` is a ring-3 design-stage stub: no crate, no entry point, no code, and the
//! vault / bus integration / `SMessage` variants are all still undefined. There is no kernel-side
//! Holocron code anywhere in the tree, and nothing in UnaOS persists a secret today. So this module
//! is **not** an RPC to a handler that does not exist. It is the minimal kernel-side seam the future
//! userspace vault will adopt: one classed-record blob store, one file on the writable FAT volume,
//! and a load/flush pair whose whole reason for existing is that the write must happen **somewhere
//! other than where the record is produced**.
//!
//! # The re-entrancy wall this exists to clear (the whole point)
//!
//! The Bluetooth chain that produces the first record — a bonded peer's link key — runs inside one
//! `service_ehci_hid()` pass holding the `EHCI_HID` mutex (`drivers/ehci/mod.rs`). The writable FAT
//! volume rides USB mass storage whose I/O goes through `drivers/block.rs` → `xhci::claim()` +
//! `storage_read10/write10`. A filesystem write issued from inside the BT chain would (a) contend the
//! xHCI storage loan from inside the EHCI service pass, and (b) hold the internal keyboard and
//! trackpad hostage for the write's duration, on top of SSP's 8 s worst case.
//!
//! Hence the split, and it is the load-bearing property of the module:
//!
//!   * [`put`] / [`remove`] are **RAM + a dirty flag**. Under the EHCI lock they are a `memcpy` and a
//!     bool. No I/O, no allocation, no wait.
//!   * [`flush_if_dirty`] is the write, and it runs from the main loop with no driver lock held. It
//!     **refuses to run** while `EHCI_HID` is held (x86; see the guard inside) rather than trusting
//!     the call site — the invariant is checked, not asserted in a comment.
//!
//! # On-disk format (v1)
//!
//! ```text
//! header:  magic "HCRN" | ver u8 = 1 | count u8 | seq u32 (LE) | hdr_crc32 (LE)   -- 14 bytes
//! record:  class u8 | len u8 | body[len] | crc32(class,len,body) (LE)
//! ```
//!
//! `hdr_crc32` covers the ten bytes before it. Every record carries its own CRC over its own framing
//! **and** body, so a torn write shows up as a refused record rather than a half-adopted one. CRC-32
//! comes from the arch-neutral [`crate::hash::crc32`] (CRC-32/ISO-HDLC) that the GPT writer and the
//! gzip trailer check already share, so the format is checkable by host tools without new code.
//!
//! `seq` is a monotonic write counter, bumped on every successful flush. There is no RTC on this
//! machine, so `seq` is the only clock the store has; the bond class uses it as its LRU clock.
//!
//! **FAIL-CLOSED, ALWAYS.** Bad magic, an unknown version, a bad header CRC, a bad record CRC, a
//! truncated body, an over-long body, more records than the table holds, or trailing bytes past the
//! last record — every one of them refuses the WHOLE image and the store starts EMPTY, witnessed.
//! There is no partial adoption: a store that adopted the records before the corruption would be a
//! store whose contents depend on where the damage happened to land.
//!
//! # At-rest posture (stated, not implied)
//!
//! The FAT volume is plaintext and this machine has no protected key storage — no TPM, no SEP path.
//! v1 therefore stores its records **plaintext-on-media, CRC'd, and says so**. The CRC is torn-write
//! detection, **not** authentication: it stops a half-written record from being adopted; it stops
//! nobody who can write the file. A kernel-embedded cipher key would be theatre — recoverable from
//! the image by anyone holding the medium — and is explicitly not claimed. What v1 does enforce is
//! process hygiene: record bytes are never printed to serial, and the flush staging buffer is
//! zeroized before it is dropped. Vault encryption (hardware-backed or passphrase-derived) is
//! Holocron-proper's job; the `ver` byte is the migration hook. See `docs/SECURITY.md`.
//!
//! # Keys
//!
//! The v1 framing carries no key field: a record is `class | len | body`. The key is therefore a
//! **span inside the body**, declared per class by [`class_key_span`]. [`put`] takes the caller's key
//! explicitly (the API shape the design names) and **refuses** a key that does not equal that span,
//! so a class codec and the store can never disagree about what a record's identity is.

use spin::Mutex;

use crate::hash::crc32;

// =========================================================================================
// FORMAT CONSTANTS
// =========================================================================================

/// On-disk magic. Four ASCII bytes so a `strings`/`xxd` of the medium identifies the file.
pub const HCRON_MAGIC: [u8; 4] = *b"HCRN";

/// Format version. Bumped only for an incompatible framing change; a reader that does not know a
/// version refuses the whole image rather than guessing at the layout.
pub const HCRON_VER: u8 = 1;

/// `magic(4) | ver(1) | count(1) | seq(4) | hdr_crc32(4)`.
pub const HCRON_HDR_LEN: usize = 14;

/// Bytes of the header the header CRC covers (everything before the CRC field itself).
pub const HCRON_HDR_CRC_SPAN: usize = 10;

/// Per-record framing overhead: `class(1) | len(1) | ... | crc32(4)`.
pub const HCRON_REC_OVERHEAD: usize = 6;

/// The class registry. One entry in v1.
///
/// Class 0x01 — a Bluetooth bond (`drivers/ehci/btbond.rs` owns the body schema).
pub const HCRON_CLASS_BTBOND: u8 = 0x01;

/// Records the in-RAM table holds, across all classes. Small on purpose: this is a bond store on a
/// laptop, not a database, and a fixed table means no allocation on the `put` path (which runs under
/// the EHCI lock).
pub const HCRON_MAX_RECORDS: usize = 8;

/// The longest record body v1 accepts. The bond schema is 37 bytes; the rest is headroom for the
/// next class without a format bump.
pub const HCRON_MAX_BODY: usize = 64;

/// The largest image the table can produce — the flush staging buffer's size, and the largest image
/// [`parse_image`] will look at. Fixed, so neither codec path allocates.
pub const HCRON_IMAGE_MAX: usize =
    HCRON_HDR_LEN + HCRON_MAX_RECORDS * (HCRON_REC_OVERHEAD + HCRON_MAX_BODY);

/// The store's directory on the writable FAT volume. 8.3-clean.
pub const HCRON_DIR: &str = "HCRON";

/// The store file's 8.3-clean leaf name.
pub const HCRON_FILE: &str = "BTBOND.DAT";

/// The store file, as a whole path — the text every witness prints.
pub const HCRON_PATH: &str = "/HCRON/BTBOND.DAT";

/// Consecutive failed flushes before the store gives up and stops retrying. A volume that vetoes
/// writes (or a stick pulled mid-boot) must not be able to make the main loop print forever.
pub const HCRON_FLUSH_ATTEMPTS: u8 = 8;

// =========================================================================================
// ERRORS
// =========================================================================================

/// Everything the store can refuse, named. Each variant is a distinguishable refusal on the wire —
/// "the store came up empty" is never printed without which of these caused it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HcronError {
    /// The image is shorter than a header, or a record's declared body runs past the end.
    Truncated,
    /// The first four bytes are not `HCRN`.
    BadMagic,
    /// A version this build does not know how to parse.
    BadVersion,
    /// The header's own CRC does not match the header.
    HeaderCrc,
    /// A record's CRC does not match its framing + body.
    RecordCrc,
    /// The header declares more records than the table can hold.
    TooManyRecords,
    /// A record declares a body longer than [`HCRON_MAX_BODY`].
    BodyTooLong,
    /// Bytes past the last record the header declared.
    TrailingBytes,
    /// No [`class_key_span`] is registered for this class.
    UnknownClass,
    /// The caller's key does not equal the key span inside the body it handed over.
    KeyMismatch,
    /// The record table is full and no existing record carries this key.
    Full,
    /// No record of this class carries this key.
    NotFound,
    /// The volume refused, or the block layer failed.
    Io,
    /// No block device / no FAT volume yet.
    NoStorage,
    /// The volume is mounted but ordinary file mutation is vetoed on it.
    ReadOnly,
}

/// One short phrase per refusal, for the witness lines.
pub fn hcron_reason(e: HcronError) -> &'static str {
    match e {
        HcronError::Truncated => "truncated image",
        HcronError::BadMagic => "bad magic",
        HcronError::BadVersion => "unknown version",
        HcronError::HeaderCrc => "bad header crc",
        HcronError::RecordCrc => "bad record crc",
        HcronError::TooManyRecords => "record count over table",
        HcronError::BodyTooLong => "record body over cap",
        HcronError::TrailingBytes => "trailing bytes",
        HcronError::UnknownClass => "unknown class",
        HcronError::KeyMismatch => "key does not match body",
        HcronError::Full => "table full",
        HcronError::NotFound => "no such record",
        HcronError::Io => "block/fs io",
        HcronError::NoStorage => "no storage",
        HcronError::ReadOnly => "write vetoed",
    }
}

// =========================================================================================
// THE CLASS REGISTRY
// =========================================================================================

/// Where a class's PRIMARY KEY lives inside its record body, as `(offset, len)`.
///
/// This is the whole class registry in v1. It exists because the framing carries no key field, and
/// because a store that let each caller decide what a record's identity is would happily hold two
/// records for the same peer. A class with no entry here cannot be stored at all.
///
/// Class 0x01 (bond): the BR/EDR `bd_addr`, six bytes at body offset 2 — see
/// `drivers/ehci/btbond.rs` for the schema this offset indexes into.
pub const fn class_key_span(class: u8) -> Option<(usize, usize)> {
    match class {
        HCRON_CLASS_BTBOND => Some((2, 6)),
        _ => None,
    }
}

// =========================================================================================
// RECORDS + IMAGES (pure — no lock, no filesystem, no allocation)
// =========================================================================================

/// One classed record, inline. `Copy` so the table is a plain array and `put` is a `memcpy`.
#[derive(Clone, Copy)]
pub struct Record {
    pub class: u8,
    len: u8,
    body: [u8; HCRON_MAX_BODY],
}

impl Record {
    /// An empty class-0 slot. `const` so the table is a `static` with no initializer to run.
    pub const fn empty() -> Self {
        Self {
            class: 0,
            len: 0,
            body: [0u8; HCRON_MAX_BODY],
        }
    }

    /// Build a record from a class and a body. Refuses an over-long body.
    pub fn new(class: u8, body: &[u8]) -> Result<Self, HcronError> {
        if body.len() > HCRON_MAX_BODY {
            return Err(HcronError::BodyTooLong);
        }
        let mut r = Self::empty();
        r.class = class;
        r.len = body.len() as u8;
        r.body[..body.len()].copy_from_slice(body);
        Ok(r)
    }

    /// The record's body, exactly as long as it was stored.
    pub fn body(&self) -> &[u8] {
        &self.body[..self.len as usize]
    }

    /// The record's primary key, per [`class_key_span`], or `None` when the class is unregistered or
    /// the body is too short to carry the span it declares.
    pub fn key(&self) -> Option<&[u8]> {
        let (off, len) = class_key_span(self.class)?;
        let end = off.checked_add(len)?;
        if end > self.len as usize {
            return None;
        }
        Some(&self.body[off..end])
    }

    /// Zero the whole inline body, not just the live prefix — a shortened record must not leave the
    /// tail of a longer predecessor readable in RAM.
    fn wipe(&mut self) {
        self.class = 0;
        self.len = 0;
        for b in self.body.iter_mut() {
            *b = 0;
        }
    }
}

/// A parsed image: everything the file said, with nothing adopted yet.
pub struct Image {
    pub seq: u32,
    pub count: usize,
    pub recs: [Record; HCRON_MAX_RECORDS],
}

impl Image {
    /// The records this image actually carried.
    pub fn records(&self) -> &[Record] {
        &self.recs[..self.count]
    }
}

/// Serialize `recs` at write-counter `seq` into `out`. Returns the image length.
///
/// Pure: no lock, no filesystem, no allocation. `out` must be at least [`HCRON_IMAGE_MAX`] bytes;
/// a shorter buffer is refused rather than truncated into a file that would never parse back.
pub fn serialize_into(seq: u32, recs: &[Record], out: &mut [u8]) -> Result<usize, HcronError> {
    if recs.len() > HCRON_MAX_RECORDS {
        return Err(HcronError::TooManyRecords);
    }
    let mut need = HCRON_HDR_LEN;
    for r in recs {
        if r.len as usize > HCRON_MAX_BODY {
            return Err(HcronError::BodyTooLong);
        }
        need += HCRON_REC_OVERHEAD + r.len as usize;
    }
    if out.len() < need {
        return Err(HcronError::Truncated);
    }

    out[0..4].copy_from_slice(&HCRON_MAGIC);
    out[4] = HCRON_VER;
    out[5] = recs.len() as u8;
    out[6..10].copy_from_slice(&seq.to_le_bytes());
    let hdr_crc = crc32(&out[0..HCRON_HDR_CRC_SPAN]);
    out[10..14].copy_from_slice(&hdr_crc.to_le_bytes());

    let mut at = HCRON_HDR_LEN;
    for r in recs {
        let blen = r.len as usize;
        out[at] = r.class;
        out[at + 1] = r.len;
        out[at + 2..at + 2 + blen].copy_from_slice(r.body());
        // The record CRC covers the framing bytes too, so a flipped `class` or `len` is caught by
        // the same check that catches a flipped body byte.
        let rec_crc = crc32(&out[at..at + 2 + blen]);
        out[at + 2 + blen..at + 2 + blen + 4].copy_from_slice(&rec_crc.to_le_bytes());
        at += HCRON_REC_OVERHEAD + blen;
    }
    Ok(at)
}

/// Parse a whole image. Fail-closed: ANY defect refuses the WHOLE image.
///
/// Pure: no lock, no filesystem, no allocation. This is the function the corrupt-a-byte fixture
/// leg drives, and the one every load goes through — the same code refuses the fixture's damage and
/// a real torn write.
pub fn parse_image(img: &[u8]) -> Result<Image, HcronError> {
    if img.len() < HCRON_HDR_LEN {
        return Err(HcronError::Truncated);
    }
    if img[0..4] != HCRON_MAGIC {
        return Err(HcronError::BadMagic);
    }
    if img[4] != HCRON_VER {
        return Err(HcronError::BadVersion);
    }
    let want_hdr = u32::from_le_bytes([img[10], img[11], img[12], img[13]]);
    if crc32(&img[0..HCRON_HDR_CRC_SPAN]) != want_hdr {
        return Err(HcronError::HeaderCrc);
    }
    let count = img[5] as usize;
    if count > HCRON_MAX_RECORDS {
        return Err(HcronError::TooManyRecords);
    }
    let seq = u32::from_le_bytes([img[6], img[7], img[8], img[9]]);

    let mut out = Image {
        seq,
        count: 0,
        recs: [Record::empty(); HCRON_MAX_RECORDS],
    };
    let mut at = HCRON_HDR_LEN;
    for i in 0..count {
        if at + 2 > img.len() {
            return Err(HcronError::Truncated);
        }
        let class = img[at];
        let blen = img[at + 1] as usize;
        if blen > HCRON_MAX_BODY {
            return Err(HcronError::BodyTooLong);
        }
        let end = at + HCRON_REC_OVERHEAD + blen;
        if end > img.len() {
            return Err(HcronError::Truncated);
        }
        let want = u32::from_le_bytes([
            img[at + 2 + blen],
            img[at + 3 + blen],
            img[at + 4 + blen],
            img[at + 5 + blen],
        ]);
        if crc32(&img[at..at + 2 + blen]) != want {
            return Err(HcronError::RecordCrc);
        }
        out.recs[i] = Record::new(class, &img[at + 2..at + 2 + blen])?;
        at = end;
    }
    if at != img.len() {
        // A file longer than its own header claims is not a store this reader understands. Adopting
        // the prefix would mean adopting an image somebody else's writer produced.
        return Err(HcronError::TrailingBytes);
    }
    out.count = count;
    Ok(out)
}

// =========================================================================================
// THE IN-RAM TABLE
// =========================================================================================

struct Store {
    recs: [Record; HCRON_MAX_RECORDS],
    count: usize,
    /// The write counter, as last read from (or written to) the file.
    seq: u32,
    /// RAM differs from the medium; [`flush_if_dirty`] has work.
    dirty: bool,
    /// [`load_once`] reached a decision — the file was read, or proven absent. Until this is true a
    /// lookup miss means "not loaded yet", which is a different answer from "no such bond".
    loaded: bool,
    /// [`load_once`] has run and will not run again (whatever it decided).
    load_done: bool,
    /// Consecutive failed flushes.
    flush_fails: u8,
    /// [`HCRON_FLUSH_ATTEMPTS`] failures in a row — retries stopped, witnessed once.
    gave_up: bool,
}

impl Store {
    const fn new() -> Self {
        Self {
            recs: [Record::empty(); HCRON_MAX_RECORDS],
            count: 0,
            seq: 0,
            dirty: false,
            loaded: false,
            load_done: false,
            flush_fails: 0,
            gave_up: false,
        }
    }

    fn find(&self, class: u8, key: &[u8]) -> Option<usize> {
        (0..self.count).find(|&i| self.recs[i].class == class && self.recs[i].key() == Some(key))
    }
}

static STORE: Mutex<Store> = Mutex::new(Store::new());

/// Has [`load_once`] reached a decision? A `false` here is why a lookup miss must be witnessed as
/// "store not loaded yet" rather than "no such record" — see the boot-ordering note on [`service`].
pub fn is_loaded() -> bool {
    STORE.lock().loaded
}

/// Does RAM differ from the medium?
pub fn is_dirty() -> bool {
    STORE.lock().dirty
}

/// The write counter. `0` before the first successful flush.
pub fn seq() -> u32 {
    STORE.lock().seq
}

/// How many records of `class` the table holds.
pub fn count(class: u8) -> usize {
    let s = STORE.lock();
    (0..s.count).filter(|&i| s.recs[i].class == class).count()
}

/// Store `val` under `key` in `class`. **RAM and a dirty flag only** — no I/O, no allocation, no
/// wait. This is what a driver may call with a lock held.
///
/// `key` must equal the class's key span inside `val` ([`class_key_span`]); a mismatch is refused
/// rather than silently re-keyed. An existing record with the same class and key is **overwritten in
/// place** — the store never accumulates a second record for the same identity.
pub fn put(class: u8, key: &[u8], val: &[u8]) -> Result<(), HcronError> {
    let (off, len) = class_key_span(class).ok_or(HcronError::UnknownClass)?;
    if key.len() != len || val.len() < off + len || &val[off..off + len] != key {
        return Err(HcronError::KeyMismatch);
    }
    let rec = Record::new(class, val)?;
    let mut s = STORE.lock();
    if let Some(i) = s.find(class, key) {
        s.recs[i] = rec;
        s.dirty = true;
        return Ok(());
    }
    if s.count >= HCRON_MAX_RECORDS {
        return Err(HcronError::Full);
    }
    let n = s.count;
    s.recs[n] = rec;
    s.count = n + 1;
    s.dirty = true;
    Ok(())
}

/// Copy the body of the record `class`/`key` into `out`, returning its length.
///
/// **Deviation from the design's `get(class, key) -> Option<&[u8]>`, stated rather than hidden:** the
/// table lives behind a `spin::Mutex`, so no reference into it can outlive the guard. Copying into a
/// caller buffer is the same operation with a lifetime the borrow checker can see; it costs one
/// `memcpy` of at most [`HCRON_MAX_BODY`] bytes and it means no caller can hold the store's lock
/// open across arbitrary work.
pub fn get(class: u8, key: &[u8], out: &mut [u8]) -> Option<usize> {
    let s = STORE.lock();
    let i = s.find(class, key)?;
    let body = s.recs[i].body();
    if out.len() < body.len() {
        return None;
    }
    out[..body.len()].copy_from_slice(body);
    Some(body.len())
}

/// The FIRST record of `class` whose body satisfies `pred`, copied into `out`.
///
/// The escape hatch for a class whose lookup is not by primary key — the bond class matches on an LE
/// identity address as well as the BR/EDR one. `pred` runs **with the store lock held**, so it must
/// be pure: it may read the body it is handed and nothing else. It must not call back into this
/// module (`spin::Mutex` is not re-entrant).
pub fn find_body(class: u8, pred: impl Fn(&[u8]) -> bool, out: &mut [u8]) -> Option<usize> {
    let s = STORE.lock();
    for i in 0..s.count {
        if s.recs[i].class != class {
            continue;
        }
        let body = s.recs[i].body();
        if pred(body) {
            if out.len() < body.len() {
                return None;
            }
            out[..body.len()].copy_from_slice(body);
            return Some(body.len());
        }
    }
    None
}

/// The `n`-th record of `class` (in table order), copied into `out`. Lets a class walk its own
/// records — the bond class's LRU victim search — without holding this module's lock across the walk.
pub fn nth_body(class: u8, n: usize, out: &mut [u8]) -> Option<usize> {
    let s = STORE.lock();
    let mut seen = 0usize;
    for i in 0..s.count {
        if s.recs[i].class != class {
            continue;
        }
        if seen == n {
            let body = s.recs[i].body();
            if out.len() < body.len() {
                return None;
            }
            out[..body.len()].copy_from_slice(body);
            return Some(body.len());
        }
        seen += 1;
    }
    None
}

/// Drop the record `class`/`key`. **RAM and a dirty flag only**, like [`put`] — the record leaves the
/// medium at the next flush. Returns whether anything was removed.
///
/// The vacated slot is wiped, not merely unlinked: a stale link key sitting in the tail of the table
/// after its bond was discarded would outlive the discard that was supposed to end it.
pub fn remove(class: u8, key: &[u8]) -> bool {
    let mut s = STORE.lock();
    let Some(i) = s.find(class, key) else {
        return false;
    };
    let last = s.count - 1;
    if i != last {
        s.recs[i] = s.recs[last];
    }
    s.recs[last].wipe();
    s.count = last;
    s.dirty = true;
    true
}

/// Drop EVERY record and mark the store dirty. The self-cleaning tail of the selftest, and the
/// recovery a fail-closed load performs before it declares the store empty.
pub fn clear() {
    let mut s = STORE.lock();
    for r in s.recs.iter_mut() {
        r.wipe();
    }
    s.count = 0;
    s.dirty = true;
}

// =========================================================================================
// THE FILESYSTEM HALF
// =========================================================================================
//
// NAMED-PATH DIVERGENCE, recorded here because it is the one place the BT-BOND design does not
// match the tree. The design specifies the whole-file rewrite "through the VFS/FAT write path
// (`fs/vfs.rs` `create`/`write`)". In this tree `impl VfsBackend for FatBackend` — along with
// `resolve_parent`, `fat_err` and `fat_create_err` — is `#[cfg(target_arch = "aarch64")]`. On
// x86_64, which is the platform this whole arc is FOR, `FatBackend` is a struct with no backend
// impl and cannot be mounted into a `MountTable` at all.
//
// So the write below goes to `fs::fat`'s dir-aware twins directly: `locate_in_dir` / `create_dir` /
// `create_in_dir` / `delete_located` / `write_grow`. That is not a substitute MECHANISM — it is the
// exact set of primitives the aarch64 `FatBackend` adapter wraps, reached the way the arch-neutral
// `shell.rs` file verbs and `flight_recorder.rs` already reach them, and it lands on the same
// `block::write_block_usb` BOT WRITE(10) path the design names. Adding an x86 arm to `fs/vfs.rs`
// would be the alternative, and it is out of this arc's lane.

/// Read `/HCRON/<leaf>` off the writable FAT volume into `out`.
fn read_store_file(leaf: &str, out: &mut alloc::vec::Vec<u8>) -> Result<(), HcronError> {
    let fs = crate::fs::fat::mount().map_err(|_| HcronError::NoStorage)?;
    let (dir, _, _) = fs
        .locate_in_dir(0, HCRON_DIR)
        .map_err(|e| map_fat(e, HcronError::NotFound))?;
    if !dir.is_dir {
        return Err(HcronError::Io);
    }
    let (de, _, _) = fs
        .locate_in_dir(dir.first_cluster(), leaf)
        .map_err(|e| map_fat(e, HcronError::NotFound))?;
    if de.is_dir {
        return Err(HcronError::Io);
    }
    if de.size as usize > HCRON_IMAGE_MAX {
        // Longer than any image this build can produce. Refuse before reading it: an oversized file
        // is either not ours or is damaged, and either way it is not adopted.
        return Err(HcronError::TrailingBytes);
    }
    fs.read_at(de.first_cluster(), de.size, 0, out, de.size as usize)
        .map_err(|e| map_fat(e, HcronError::Io))?;
    if out.len() != de.size as usize {
        return Err(HcronError::Truncated);
    }
    Ok(())
}

/// Whole-file rewrite of `/HCRON/<leaf>`. Creates `/HCRON` if absent.
///
/// Delete-then-create is how the tree's own arch-neutral write verb (`shell.rs::fs_write`) replaces a
/// file, and it is what "whole-file rewrite" means here: the record count can shrink, so an in-place
/// overwrite would leave a tail of the previous image behind. The window in which the file does not
/// exist is covered by the format, not by luck — a reader that finds no file starts empty, and a
/// reader that finds a half-written one refuses it on the CRC.
fn write_store_file(leaf: &str, data: &[u8]) -> Result<(), HcronError> {
    let fs = crate::fs::fat::mount().map_err(|_| HcronError::NoStorage)?;
    if fs.write_veto().is_some() {
        return Err(HcronError::ReadOnly);
    }
    // The store directory, created on first use.
    let dir_clus = match fs.locate_in_dir(0, HCRON_DIR) {
        Ok((de, _, _)) if de.is_dir => de.first_cluster(),
        Ok(_) => return Err(HcronError::Io), // a FILE named HCRON — not ours to replace
        Err(crate::fs::fat::FatError::NotFound) => {
            let (de, _, _) = fs
                .create_dir(0, HCRON_DIR)
                .map_err(|e| map_fat(e, HcronError::Io))?;
            de.first_cluster()
        }
        Err(e) => return Err(map_fat(e, HcronError::Io)),
    };
    // Replace the leaf: drop any existing entry, then a fresh 0-length one to grow into.
    let (dir_lba, dir_off) = match fs.locate_in_dir(dir_clus, leaf) {
        Ok((de, dl, doff)) => {
            if de.is_dir {
                return Err(HcronError::Io);
            }
            fs.delete_located(dl, doff, de.first_cluster())
                .map_err(|e| map_fat(e, HcronError::Io))?;
            let (_, l, o) = fs
                .create_in_dir(dir_clus, leaf, 0x20)
                .map_err(|e| map_fat(e, HcronError::Io))?;
            (l, o)
        }
        Err(crate::fs::fat::FatError::NotFound) => {
            let (_, l, o) = fs
                .create_in_dir(dir_clus, leaf, 0x20)
                .map_err(|e| map_fat(e, HcronError::Io))?;
            (l, o)
        }
        Err(e) => return Err(map_fat(e, HcronError::Io)),
    };
    let (written, _, _) = fs
        .write_grow(0, 0, dir_lba, dir_off, 0, data)
        .map_err(|e| map_fat(e, HcronError::Io))?;
    if written != data.len() {
        return Err(HcronError::Io);
    }
    Ok(())
}

/// Delete `/HCRON/<leaf>` if it is there. Absent is success — this is the selftest's self-clean, and
/// "already gone" is the state it wants.
fn unlink_store_file(leaf: &str) -> Result<(), HcronError> {
    let fs = crate::fs::fat::mount().map_err(|_| HcronError::NoStorage)?;
    if fs.write_veto().is_some() {
        return Err(HcronError::ReadOnly);
    }
    let dir_clus = match fs.locate_in_dir(0, HCRON_DIR) {
        Ok((de, _, _)) if de.is_dir => de.first_cluster(),
        Ok(_) => return Err(HcronError::Io),
        Err(crate::fs::fat::FatError::NotFound) => return Ok(()),
        Err(e) => return Err(map_fat(e, HcronError::Io)),
    };
    match fs.locate_in_dir(dir_clus, leaf) {
        Ok((de, dl, doff)) => {
            fs.delete_located(dl, doff, de.first_cluster())
                .map_err(|e| map_fat(e, HcronError::Io))?;
            Ok(())
        }
        Err(crate::fs::fat::FatError::NotFound) => Ok(()),
        Err(e) => Err(map_fat(e, HcronError::Io)),
    }
}

fn map_fat(e: crate::fs::fat::FatError, notfound: HcronError) -> HcronError {
    match e {
        crate::fs::fat::FatError::NotFound => notfound,
        _ => HcronError::Io,
    }
}

// =========================================================================================
// LOAD / FLUSH
// =========================================================================================

/// Read the store off the medium, exactly once, into the in-RAM table.
///
/// A no-op until a block device exists — it returns without latching, so the next main-loop pass
/// retries. Once it does run it latches whatever it decided, including "the file is not there".
///
/// **Fail-closed.** Any parse refusal leaves the table EMPTY and says which refusal it was. A store
/// that adopted the records it managed to read before the damage would be a store whose contents
/// depend on where the corruption landed.
pub fn load_once() {
    {
        let s = STORE.lock();
        if s.load_done {
            return;
        }
    }
    if crate::drivers::block::info().is_none() {
        return; // storage not up yet — try again next pass, without latching
    }

    let mut buf: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let read = read_store_file(HCRON_FILE, &mut buf);

    let mut s = STORE.lock();
    s.load_done = true;
    match read {
        Err(HcronError::NotFound) | Err(HcronError::NoStorage) => {
            s.loaded = true;
            drop(s);
            serial_println!(
                ":: [hcron] load: no store at {} yet -> store starts EMPTY (this is the first boot that could have written one) == witness ::",
                HCRON_PATH
            );
        }
        Err(e) => {
            s.loaded = true;
            drop(s);
            clear();
            // `clear` marks the table dirty so the next flush REPLACES the refused image rather than
            // leaving a file on the medium that every future boot will refuse in the same way.
            serial_println!(
                ":: [hcron] load: {} -> store starts EMPTY, fail-closed (nothing partially adopted) == witness ::",
                hcron_reason(e)
            );
        }
        Ok(()) => {
            drop(s);
            match parse_image(&buf) {
                Ok(img) => {
                    let mut s = STORE.lock();
                    s.count = img.count;
                    s.recs = img.recs;
                    s.seq = img.seq;
                    s.dirty = false;
                    s.loaded = true;
                    let n = s.count;
                    let q = s.seq;
                    drop(s);
                    serial_println!(
                        ":: [hcron] loaded n={} from {} (seq={}) == witness ::",
                        n, HCRON_PATH, q
                    );
                }
                Err(e) => {
                    let mut s = STORE.lock();
                    s.loaded = true;
                    drop(s);
                    clear();
                    serial_println!(
                        ":: [hcron] load: {} -> store starts EMPTY, fail-closed (nothing partially adopted) == witness ::",
                        hcron_reason(e)
                    );
                }
            }
        }
    }
    // The image buffer held record bodies. Wipe before it goes back to the allocator.
    for b in buf.iter_mut() {
        *b = 0;
    }
}

/// Is the EHCI HID mutex held on this core right now?
///
/// The one invariant this module exists to keep, checked instead of asserted. On x86 with the HID
/// path built, `flush_if_dirty` consults this and REFUSES rather than issuing block I/O from inside
/// a `service_ehci_hid()` pass — the exact deadlock-and-hostage shape described at the top of the
/// file. Everywhere else there is no such lock and the answer is a constant `false`.
#[cfg(all(target_arch = "x86_64", feature = "ehcihid"))]
fn ehci_hid_held() -> bool {
    crate::drivers::ehci::EHCI_HID.is_locked()
}

#[cfg(not(all(target_arch = "x86_64", feature = "ehcihid")))]
fn ehci_hid_held() -> bool {
    false
}

/// The deferred write. Main-loop context only, and it proves that rather than assuming it.
///
/// Rewrites the whole file when RAM differs from the medium, bumping `seq`. On failure the dirty
/// flag STAYS SET so the next pass retries, bounded at [`HCRON_FLUSH_ATTEMPTS`] consecutive
/// failures — a write-vetoed volume must not be able to make the main loop print forever.
pub fn flush_if_dirty() {
    {
        let s = STORE.lock();
        if !s.dirty || s.gave_up {
            return;
        }
    }
    if crate::drivers::block::info().is_none() {
        return; // no medium to write to yet; stay dirty and retry
    }
    if ehci_hid_held() {
        // Not a warning to be tuned out: a flush from inside the EHCI service pass is the bug this
        // whole seam was designed to avoid, so it is named on the wire and refused.
        serial_println!(
            ":: [hcron] flush REFUSED — EHCI_HID is held; the store write is deferred past the service pass BY CONSTRUCTION and this call site is inside it == witness ::"
        );
        return;
    }

    // Stage the image under the lock (a bounded memcpy), then write with the lock RELEASED — block
    // I/O must never run with the store's own mutex held either.
    let mut img = [0u8; HCRON_IMAGE_MAX];
    let (len, next_seq, n) = {
        let s = STORE.lock();
        let next = s.seq.wrapping_add(1);
        match serialize_into(next, &s.recs[..s.count], &mut img) {
            Ok(len) => (len, next, s.count),
            Err(e) => {
                drop(s);
                serial_println!(
                    ":: [hcron] flush -> {} REFUSED before any write: {} == witness ::",
                    HCRON_PATH,
                    hcron_reason(e)
                );
                let mut s = STORE.lock();
                s.gave_up = true;
                return;
            }
        }
    };

    let res = write_store_file(HCRON_FILE, &img[..len]);
    // The staging buffer carried record bodies (link key material, for class 0x01). Zeroize it here,
    // on every path, before the frame goes away.
    for b in img.iter_mut() {
        *b = 0;
    }

    let mut s = STORE.lock();
    match res {
        Ok(()) => {
            s.seq = next_seq;
            s.dirty = false;
            s.flush_fails = 0;
            drop(s);
            serial_println!(
                ":: [hcron] flush -> {} n={} seq={} bytes={} ok == witness ::",
                HCRON_PATH, n, next_seq, len
            );
        }
        Err(e) => {
            s.flush_fails = s.flush_fails.saturating_add(1);
            let fails = s.flush_fails;
            if fails >= HCRON_FLUSH_ATTEMPTS {
                s.gave_up = true;
                drop(s);
                serial_println!(
                    ":: [hcron] flush -> {} FAILED {} ({}/{}) — GIVING UP; the store stays in RAM for this boot and nothing further is attempted == witness ::",
                    HCRON_PATH, hcron_reason(e), fails, HCRON_FLUSH_ATTEMPTS
                );
            } else {
                drop(s);
                serial_println!(
                    ":: [hcron] flush -> {} FAILED {} ({}/{}) — still dirty, the next pass retries == witness ::",
                    HCRON_PATH, hcron_reason(e), fails, HCRON_FLUSH_ATTEMPTS
                );
            }
        }
    }
}

// =========================================================================================
// FIXTURE — the CRC refusal path, proven with no hardware at all
// =========================================================================================

/// Round-trip the v1 framing over vectors whose answer is known before anything is asked, and prove
/// the refusal path exists by **making it fire**.
///
/// Pure: registers and one stack buffer. No block device, no filesystem, no radio — so it runs
/// identically on x86 and aarch64, in QEMU and on metal, and a red leg is a statement about the
/// codec rather than about the medium.
///
/// The legs, in order:
///   1. serialize two records → parse → every field byte-identical;
///   2. flip ONE body byte → parse must refuse `RecordCrc`;
///   3. flip ONE header byte (`count`) → parse must refuse `HeaderCrc`;
///   4. truncate by one byte → parse must refuse `Truncated`;
///   5. append one byte → parse must refuse `TrailingBytes`;
///   6. corrupt the magic → parse must refuse `BadMagic`;
///   7. bump the version → parse must refuse `BadVersion`;
///   8. the untouched copy still parses byte-identical (the damage was in the copy, not the codec).
///
/// Returns true when every leg held.
pub fn framing_fixture() -> bool {
    let mut fails = 0u32;
    let mut legs = 0u32;

    // Two class-0x01-shaped bodies. The key span (offset 2, six bytes) is what distinguishes them.
    let mut a = [0u8; 12];
    let mut b = [0u8; 12];
    for (i, v) in a.iter_mut().enumerate() {
        *v = 0xA0 + i as u8;
    }
    for (i, v) in b.iter_mut().enumerate() {
        *v = 0xB0 + i as u8;
    }
    let recs = [
        match Record::new(HCRON_CLASS_BTBOND, &a) {
            Ok(r) => r,
            Err(_) => return false,
        },
        match Record::new(HCRON_CLASS_BTBOND, &b) {
            Ok(r) => r,
            Err(_) => return false,
        },
    ];

    let mut img = [0u8; HCRON_IMAGE_MAX];
    let len = match serialize_into(7, &recs, &mut img) {
        Ok(n) => n,
        Err(e) => {
            serial_println!(
                ":: [hcron] framing fixture leg=serialize -> FAIL ({}) == witness ::",
                hcron_reason(e)
            );
            return false;
        }
    };

    // Leg 1 — clean round-trip.
    legs += 1;
    match parse_image(&img[..len]) {
        Ok(p) => {
            let ok = p.seq == 7
                && p.count == 2
                && p.records()[0].class == HCRON_CLASS_BTBOND
                && p.records()[0].body() == a
                && p.records()[1].body() == b
                && p.records()[0].key() == Some(&a[2..8]);
            if !ok {
                fails += 1;
                serial_println!(
                    ":: [hcron] framing fixture leg=roundtrip -> FAIL (seq/count/body/key disagreed after a clean serialize+parse) == witness ::"
                );
            }
        }
        Err(e) => {
            fails += 1;
            serial_println!(
                ":: [hcron] framing fixture leg=roundtrip -> FAIL (a clean image was refused: {}) == witness ::",
                hcron_reason(e)
            );
        }
    }

    // Legs 2..7 — every refusal, MADE TO FIRE. Each leg damages a COPY, so leg 8 can prove the codec
    // itself was never the thing that changed.
    let mut damaged = [0u8; HCRON_IMAGE_MAX];

    // Leg 2 — one body byte.
    legs += 1;
    damaged[..len].copy_from_slice(&img[..len]);
    damaged[HCRON_HDR_LEN + 2] ^= 0x01; // first body byte of record 0
    fails += expect_refusal("body-byte", &damaged[..len], HcronError::RecordCrc);

    // Leg 3 — the header's record count.
    legs += 1;
    damaged[..len].copy_from_slice(&img[..len]);
    damaged[5] = 1;
    fails += expect_refusal("header-count", &damaged[..len], HcronError::HeaderCrc);

    // Leg 4 — one byte short.
    legs += 1;
    damaged[..len].copy_from_slice(&img[..len]);
    fails += expect_refusal("truncated", &damaged[..len - 1], HcronError::Truncated);

    // Leg 5 — one byte long.
    legs += 1;
    damaged[..len].copy_from_slice(&img[..len]);
    damaged[len] = 0x00;
    fails += expect_refusal("trailing", &damaged[..len + 1], HcronError::TrailingBytes);

    // Leg 6 — the magic.
    legs += 1;
    damaged[..len].copy_from_slice(&img[..len]);
    damaged[0] = b'X';
    fails += expect_refusal("magic", &damaged[..len], HcronError::BadMagic);

    // Leg 7 — the version byte. Checked BEFORE the header CRC on purpose: a future v2 image must be
    // refused as "unknown version", not mis-reported as corruption.
    legs += 1;
    damaged[..len].copy_from_slice(&img[..len]);
    damaged[4] = HCRON_VER.wrapping_add(1);
    fails += expect_refusal("version", &damaged[..len], HcronError::BadVersion);

    // Leg 8 — the pristine copy still parses. Without this, seven refusals would be equally
    // consistent with a parser that refuses everything.
    legs += 1;
    match parse_image(&img[..len]) {
        Ok(p) if p.count == 2 && p.records()[1].body() == b => {}
        _ => {
            fails += 1;
            serial_println!(
                ":: [hcron] framing fixture leg=pristine-after-damage -> FAIL (the untouched image stopped parsing) == witness ::"
            );
        }
    }

    // Wipe both buffers: they carried record bodies, and a fixture is not an exception to hygiene.
    for v in img.iter_mut() {
        *v = 0;
    }
    for v in damaged.iter_mut() {
        *v = 0;
    }

    if fails == 0 {
        serial_println!(
            ":: [hcron] framing fixture: {}/{} legs — clean round-trip, and every refusal (body crc, header crc, truncation, trailing bytes, magic, version) FIRED -> PASS ::",
            legs, legs
        );
        true
    } else {
        serial_println!(
            ":: [hcron] framing fixture: {}/{} legs failed -> FAIL ::",
            fails, legs
        );
        false
    }
}

/// One refusal leg: `img` MUST be refused, and refused with `want`. Returns 1 on failure.
fn expect_refusal(what: &str, img: &[u8], want: HcronError) -> u32 {
    match parse_image(img) {
        Err(e) if e == want => 0,
        Err(e) => {
            serial_println!(
                ":: [hcron] framing fixture leg={} -> FAIL (refused, but as \"{}\" where \"{}\" was required) == witness ::",
                what,
                hcron_reason(e),
                hcron_reason(want)
            );
            1
        }
        Ok(_) => {
            serial_println!(
                ":: [hcron] framing fixture leg={} -> FAIL (a DAMAGED image PARSED — the refusal path did not fire) == witness ::",
                what
            );
            1
        }
    }
}

// =========================================================================================
// THE SELFTEST — the same load/flush, against the REAL block path
// =========================================================================================

/// Drive the store's real load and flush against the real FAT volume, once, and clean up after
/// itself.
///
/// The framing fixture above proves the codec. This proves the *plumbing*: that a record staged into
/// the table reaches the medium through `write_grow` → `block::write_block*`, that reading it back
/// through `read_at` reproduces it byte for byte, and that a byte flipped **on the medium** is
/// refused by the same fail-closed path a torn write would hit.
///
/// It never touches [`HCRON_FILE`]: the scratch leaf below is its own, so a real bond store on the
/// medium is neither read nor overwritten by a selftest. Self-cleaning — the scratch file is unlinked
/// on every exit path, verdict or not.
///
/// Honest-skip when no writable FAT volume is present, which is what a QEMU boot without a FAT image
/// backing sees.
pub fn selftest_once() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    const SCRATCH: &str = "HCRNTEST.DAT";

    let Ok(fs) = crate::fs::fat::mount() else {
        serial_println!(":: [hcron] store round-trip: no FAT volume — SKIPPED ::");
        return;
    };
    if let Some(why) = fs.write_veto() {
        serial_println!(
            ":: [hcron] store round-trip: volume vetoes writes ({}) — SKIPPED ::",
            why
        );
        return;
    }
    drop(fs);

    // A class-0x01-shaped body on a fixture address, never a real peer's.
    let mut body = [0u8; 12];
    body[0] = HCRON_VER;
    body[2..8].copy_from_slice(&[0xAA, 0xBB, 0xCC, 0xDD, 0xEE, 0xFF]);
    body[8] = 0x5A;
    let rec = match Record::new(HCRON_CLASS_BTBOND, &body) {
        Ok(r) => r,
        Err(e) => {
            serial_println!(
                ":: [hcron] store round-trip: FAIL — fixture record refused ({}) ::",
                hcron_reason(e)
            );
            return;
        }
    };

    let mut img = [0u8; HCRON_IMAGE_MAX];
    let len = match serialize_into(41, &[rec], &mut img) {
        Ok(n) => n,
        Err(e) => {
            serial_println!(
                ":: [hcron] store round-trip: FAIL — serialize ({}) ::",
                hcron_reason(e)
            );
            return;
        }
    };

    let mut verdict: Result<(), &'static str> = Ok(());
    let mut back: alloc::vec::Vec<u8> = alloc::vec::Vec::new();

    // Stage 1 — write it, read it, parse it.
    if verdict.is_ok() {
        if let Err(e) = write_store_file(SCRATCH, &img[..len]) {
            serial_println!(
                ":: [hcron] store round-trip stage=write -> {} ::",
                hcron_reason(e)
            );
            verdict = Err("write");
        }
    }
    if verdict.is_ok() {
        match read_store_file(SCRATCH, &mut back) {
            Ok(()) if back.len() == len && back[..] == img[..len] => {}
            Ok(()) => verdict = Err("readback bytes differ"),
            Err(e) => {
                serial_println!(
                    ":: [hcron] store round-trip stage=read -> {} ::",
                    hcron_reason(e)
                );
                verdict = Err("read");
            }
        }
    }
    if verdict.is_ok() {
        match parse_image(&back) {
            Ok(p) if p.seq == 41 && p.count == 1 && p.records()[0].body() == body => {}
            Ok(_) => verdict = Err("parsed image disagreed with what was written"),
            Err(e) => {
                serial_println!(
                    ":: [hcron] store round-trip stage=parse -> {} ::",
                    hcron_reason(e)
                );
                verdict = Err("parse");
            }
        }
    }

    // Stage 2 — corrupt ONE byte ON THE MEDIUM and prove the load refuses it. This is the refusal
    // path a torn write would take, driven end to end through the real block layer.
    if verdict.is_ok() {
        let mut bad = [0u8; HCRON_IMAGE_MAX];
        bad[..len].copy_from_slice(&img[..len]);
        bad[HCRON_HDR_LEN + 2] ^= 0x01; // one body byte — the record CRC must catch it
        if let Err(e) = write_store_file(SCRATCH, &bad[..len]) {
            serial_println!(
                ":: [hcron] store round-trip stage=corrupt-write -> {} ::",
                hcron_reason(e)
            );
            verdict = Err("corrupt-write");
        } else {
            back.clear();
            match read_store_file(SCRATCH, &mut back) {
                Ok(()) => match parse_image(&back) {
                    Err(HcronError::RecordCrc) => {}
                    Err(e) => {
                        serial_println!(
                            ":: [hcron] store round-trip stage=corrupt -> refused as \"{}\" where \"bad record crc\" was required ::",
                            hcron_reason(e)
                        );
                        verdict = Err("corrupt refused for the wrong reason");
                    }
                    Ok(_) => verdict = Err("a CORRUPTED on-disk image PARSED"),
                },
                Err(e) => {
                    serial_println!(
                        ":: [hcron] store round-trip stage=corrupt-read -> {} ::",
                        hcron_reason(e)
                    );
                    verdict = Err("corrupt-read");
                }
            }
        }
        for v in bad.iter_mut() {
            *v = 0;
        }
    }

    // Self-clean, whatever the verdict.
    let cleaned = unlink_store_file(SCRATCH).is_ok();
    for v in img.iter_mut() {
        *v = 0;
    }
    for v in back.iter_mut() {
        *v = 0;
    }

    match verdict {
        Ok(()) => serial_println!(
            ":: [hcron] store round-trip: wrote {} bytes to /{}/{}, read back byte-identical, parsed seq=41 n=1; then flipped ONE on-disk body byte and the load REFUSED it (bad record crc); scratch removed={} -> PASS ::",
            len, HCRON_DIR, SCRATCH, cleaned
        ),
        Err(why) => serial_println!(
            ":: [hcron] store round-trip: {} ; scratch removed={} -> FAIL ::",
            why, cleaned
        ),
    }
}

// =========================================================================================
// THE MAIN-LOOP HOOK
// =========================================================================================

/// The store's whole main-loop presence: one call, from the storage-ready service passes.
///
/// Ordering inside is the arc's argument in miniature:
///
///   1. the pure fixtures, which need nothing and run on the first pass;
///   2. [`load_once`], which needs a block device;
///   3. the class clients, which need the store loaded before they may answer a lookup;
///   4. [`flush_if_dirty`], LAST, so a record a client staged this pass reaches the medium this pass
///      — deferred past the driver's lock, not deferred by a whole extra loop iteration.
///
/// **Boot ordering, stated honestly rather than engineered around.** `service_ehci_hid()` — where
/// the boot-time BT chain runs — is polled from the very first main-loop passes, while the FAT mount
/// is a later storage-ready one-shot. So on a boot that arms the radio, the first BT chain CAN run
/// before this function has ever had a block device to read. That window is real; it is why
/// [`is_loaded`] exists and why a miss taken before the load is witnessed as "not loaded yet" rather
/// than as "no such bond". The paths that matter for reconnection — the `Ctrl+Alt+B` re-trigger and
/// any future auto-reconnect — run long after storage is up.
///
/// The client call in step 3 is a direct `cfg`'d call rather than a registration table. There is
/// exactly one class in v1; a hook registry for one client would be scaffolding standing in for a
/// decision that has not been made yet.
pub fn service() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static FIXTURES: AtomicBool = AtomicBool::new(false);
    if !FIXTURES.swap(true, Ordering::Relaxed) {
        framing_fixture();
        #[cfg(all(target_arch = "x86_64", feature = "ehcihid"))]
        crate::drivers::ehci::btbond::codec_fixture();
    }

    load_once();
    if !is_loaded() {
        return; // no medium yet; nothing below has anything to be right about
    }

    selftest_once();

    #[cfg(all(target_arch = "x86_64", feature = "ehcihid"))]
    crate::drivers::ehci::btbond::service();

    flush_if_dirty();
}
