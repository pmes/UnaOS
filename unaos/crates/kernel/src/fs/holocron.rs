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
//!     **defers** rather than write while `EHCI_HID` cannot be proven free (x86; see the guard
//!     inside), so the invariant is checked instead of trusted — but read the guard's own doc for
//!     what that check can and cannot say. `spin::Mutex::is_locked` names no holder, so "held" is
//!     read as UNKNOWN, never as "this call site is inside the pass", and the deferral is bounded in
//!     what it prints ([`HCRON_DEFER_NOTES`]) so a contended lock can never make the main loop print
//!     forever.
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
//! # The update is a SWAP, not an overwrite (never fewer than one valid generation)
//!
//! The CRC catches a torn *record*. It cannot catch a torn *update*: a delete-then-create-then-grow
//! straight onto [`HCRON_FILE`] leaves a window in which the previous generation is already gone and
//! the new one is not yet whole, and a failure anywhere in that window loses the user's bonds
//! outright rather than falling back to a stale-but-valid store. So [`flush_if_dirty`] never writes
//! the live leaf. It writes [`HCRON_TMP_FILE`], reads it back and PARSES it, and only once the new
//! generation is proven readable on the medium does it drop the live leaf and `rename` the temp over
//! it — one directory-sector RMW, which is the smallest window `fs::fat` can offer. A boot that
//! finds no live leaf but a parseable temp adopts the temp and republishes it, so even that window is
//! covered by recovery rather than by luck.
//!
//! For the same reason a REFUSED image is **quarantined, not overwritten**: the refused bytes are
//! renamed to [`HCRON_BAD_FILE`] before the fresh empty store is published, so one medium bit-flip
//! costs the user a boot's bonds and not the bytes that might still be recoverable from them.
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

/// The staging leaf a flush writes and then renames over [`HCRON_FILE`]. 8.3-clean. Never adopted
/// as the store unless the live leaf is missing (the crash-in-the-swap-window recovery).
pub const HCRON_TMP_FILE: &str = "BTBOND.NEW";

/// Where a REFUSED image is renamed before the store publishes a fresh one over its name. One
/// generation is kept: a second refusal replaces it, so the quarantine cannot grow without bound.
pub const HCRON_BAD_FILE: &str = "BTBOND.BAD";

/// Consecutive failed flushes before the store gives up and stops retrying. A volume that vetoes
/// writes (or a stick pulled mid-boot) must not be able to make the main loop print forever.
pub const HCRON_FLUSH_ATTEMPTS: u8 = 8;

/// The HARD CAP on how many lines the store may ever print about a DEFERRED flush, per boot.
///
/// The same law as [`HCRON_FLUSH_ATTEMPTS`], enforced on the other refusal path. Deferral is not a
/// failure — it is "the lock could not be proven free on this pass" — so it must NOT consume the
/// flush-failure budget and must NOT stop the retries (a legitimately long service pass would
/// otherwise cost the boot its persistence). What it must not do is print once per main-loop pass
/// for as long as the contention lasts, which is exactly what the first version of this guard did.
/// The retry stays unbounded; the WITNESS is bounded, by construction, right here.
pub const HCRON_DEFER_NOTES: u8 = 2;

/// Consecutive deferred passes after which the second (and last) deferral line is printed.
///
/// Deliberately NOT a verdict, and deliberately not gated on by any spec. `is_locked()` names no
/// holder, so no number of deferrals can distinguish a call site inside the service pass from a
/// service pass that is legitimately long (an SSP chain runs for seconds on metal, and it is
/// precisely the chain that makes the store dirty). The line says both readings out loud and then
/// the store goes quiet.
pub const HCRON_DEFER_STUCK: u32 = 4096;

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
    /// Consecutive passes on which the flush could not prove `EHCI_HID` free and deferred. Reset by
    /// any pass that gets past the guard, so it reads as "in a row", not "since boot".
    defers: u32,
    /// Lines actually EMITTED about deferral this boot. Incremented where the print happens, so the
    /// count is of output and not of intent — [`HCRON_DEFER_NOTES`] is the cap it is checked against.
    defer_notes: u8,
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
            defers: 0,
            defer_notes: 0,
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

/// The `/HCRON` directory's first cluster. `create` decides whether an absent directory is made or
/// reported — a reader must never conjure a directory it is only looking in.
fn store_dir(fs: &crate::fs::fat::FatFs, create: bool) -> Result<u32, HcronError> {
    match fs.locate_in_dir(0, HCRON_DIR) {
        Ok((de, _, _)) if de.is_dir => Ok(de.first_cluster()),
        Ok(_) => Err(HcronError::Io), // a FILE named HCRON — not ours to replace
        Err(crate::fs::fat::FatError::NotFound) => {
            if !create {
                return Err(HcronError::NotFound);
            }
            let (de, _, _) = fs
                .create_dir(0, HCRON_DIR)
                .map_err(|e| map_fat(e, HcronError::Io))?;
            Ok(de.first_cluster())
        }
        Err(e) => Err(map_fat(e, HcronError::Io)),
    }
}

/// Whole-file rewrite of `/HCRON/<leaf>`. Creates `/HCRON` if absent.
///
/// Delete-then-create is how the tree's own arch-neutral write verb (`shell.rs::fs_write`) replaces a
/// file, and it is what "whole-file rewrite" means here: the record count can shrink, so an in-place
/// overwrite would leave a tail of the previous image behind.
///
/// **This is the primitive, not the publish.** It is deliberately NOT how the live store is updated:
/// between the delete and a completed `write_grow` there is no file at all, and a failure in that
/// window destroys the previous generation rather than falling back to it. The live leaf is written
/// by [`publish_store_file`], which uses this on [`HCRON_TMP_FILE`] and then swaps. The only callers
/// that name a leaf directly are that publish and the selftest's own scratch leaf, neither of which
/// has a previous generation worth protecting.
fn write_store_file(leaf: &str, data: &[u8]) -> Result<(), HcronError> {
    let fs = crate::fs::fat::mount().map_err(|_| HcronError::NoStorage)?;
    if fs.write_veto().is_some() {
        return Err(HcronError::ReadOnly);
    }
    // The store directory, created on first use.
    let dir_clus = store_dir(&fs, true)?;
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
/// "already gone" is the state it wants. The selftest is its only caller, so it rides the same knob:
/// the store's own paths never delete a leaf outright, they swap over it.
#[cfg(feature = "hcronst")]
fn unlink_store_file(leaf: &str) -> Result<(), HcronError> {
    let fs = crate::fs::fat::mount().map_err(|_| HcronError::NoStorage)?;
    if fs.write_veto().is_some() {
        return Err(HcronError::ReadOnly);
    }
    let dir_clus = match store_dir(&fs, false) {
        Ok(c) => c,
        Err(HcronError::NotFound) => return Ok(()), // no /HCRON at all — already gone
        Err(e) => return Err(e),
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

/// PUBLISH the live store: stage into [`HCRON_TMP_FILE`], PROVE the staged bytes are readable, then
/// swap the temp over [`HCRON_FILE`].
///
/// The property this exists for, stated as the invariant it keeps: **at no point between two calls
/// is there neither a valid live store nor a valid temp.** The old sequence
/// (`delete_located(BTBOND.DAT)` → `create_in_dir` → `write_grow`) violated that for the whole
/// duration of the grow — a failure or a pull anywhere in there left the volume with no store at all
/// instead of the previous generation, and the per-record CRC is no help: it detects a torn RECORD,
/// while this is a torn UPDATE.
///
/// The four steps, and what each one buys:
///
///   1. **Stage.** Whole-file rewrite of the TEMP leaf. The live leaf is untouched, so a failure here
///      costs nothing: the previous generation is still the live one and the next pass retries.
///   2. **Prove.** Read the temp back off the medium and `parse_image` it. The old generation is not
///      allowed to die on the strength of a `write_grow` return code; it dies only once the bytes
///      that will replace it have been read back through the same path a future boot will read them
///      through, and have parsed. This is also a torn-write check on the write that just happened.
///   3. **Drop the live leaf.** `mark_dir_deleted` + free chain.
///   4. **Rename.** `rename_entry` rewrites the name field of the temp's directory entry IN PLACE —
///      a single directory-sector RMW, the smallest window `fs::fat` can offer.
///
/// The window between 3 and 4 is one sector write wide, and even inside it the data is intact under
/// the temp name — which is why [`load_once`] adopts a parseable temp when the live leaf is absent.
/// Nothing here is atomic in the hardware sense; FAT cannot be. What it is, is recoverable at every
/// point, which the previous sequence was not.
fn publish_store_file(data: &[u8]) -> Result<(), HcronError> {
    // 1. Stage.
    write_store_file(HCRON_TMP_FILE, data)?;

    // 2. Prove: the staged generation must READ BACK and PARSE before the live one may die.
    let mut back: alloc::vec::Vec<u8> = alloc::vec::Vec::new();
    let proof = read_store_file(HCRON_TMP_FILE, &mut back).and_then(|()| {
        if back.len() != data.len() || back[..] != data[..] {
            return Err(HcronError::Io);
        }
        parse_image(&back).map(|_| ())
    });
    // The readback buffer held record bodies. Wipe before it goes back to the allocator, on every
    // path — a verification step is not an exception to hygiene.
    for b in back.iter_mut() {
        *b = 0;
    }
    proof?;

    // 3 + 4. Swap, under one mount.
    let fs = crate::fs::fat::mount().map_err(|_| HcronError::NoStorage)?;
    if fs.write_veto().is_some() {
        return Err(HcronError::ReadOnly);
    }
    let dir_clus = store_dir(&fs, true)?;
    match fs.locate_in_dir(dir_clus, HCRON_FILE) {
        Ok((de, dl, doff)) => {
            if de.is_dir {
                return Err(HcronError::Io);
            }
            fs.delete_located(dl, doff, de.first_cluster())
                .map_err(|e| map_fat(e, HcronError::Io))?;
        }
        Err(crate::fs::fat::FatError::NotFound) => {}
        Err(e) => return Err(map_fat(e, HcronError::Io)),
    }
    fs.rename_entry(dir_clus, HCRON_TMP_FILE, HCRON_FILE)
        .map_err(|e| map_fat(e, HcronError::Io))?;
    Ok(())
}

/// QUARANTINE a refused image: rename `/HCRON/<leaf>` to [`HCRON_BAD_FILE`], replacing any previous
/// quarantine.
///
/// A refused load calls [`clear`], which marks the table dirty so the next flush replaces the bad
/// image rather than leaving a file every future boot refuses identically. That is right, and it used
/// to mean the refused bytes were DESTROYED — one bit-flip on the medium and the user's bonds were
/// gone for good, with nothing left to recover them from. So the bytes are moved aside first. One
/// generation is kept (a second refusal replaces it), the rename is a single directory-sector RMW,
/// and a failure here is reported to the caller rather than swallowed: the witness says whether the
/// evidence survived.
fn quarantine_store_file(leaf: &str) -> Result<(), HcronError> {
    let fs = crate::fs::fat::mount().map_err(|_| HcronError::NoStorage)?;
    if fs.write_veto().is_some() {
        return Err(HcronError::ReadOnly);
    }
    let dir_clus = store_dir(&fs, false)?;
    // The refused leaf must exist — nothing to quarantine otherwise, and that is not an error.
    match fs.locate_in_dir(dir_clus, leaf) {
        Ok((de, _, _)) if !de.is_dir => {}
        Ok(_) => return Err(HcronError::Io),
        Err(crate::fs::fat::FatError::NotFound) => return Ok(()),
        Err(e) => return Err(map_fat(e, HcronError::Io)),
    }
    // Free the name first: `rename_entry` refuses a destination that already exists.
    match fs.locate_in_dir(dir_clus, HCRON_BAD_FILE) {
        Ok((de, dl, doff)) => {
            if de.is_dir {
                return Err(HcronError::Io);
            }
            fs.delete_located(dl, doff, de.first_cluster())
                .map_err(|e| map_fat(e, HcronError::Io))?;
        }
        Err(crate::fs::fat::FatError::NotFound) => {}
        Err(e) => return Err(map_fat(e, HcronError::Io)),
    }
    fs.rename_entry(dir_clus, leaf, HCRON_BAD_FILE)
        .map_err(|e| map_fat(e, HcronError::Io))?;
    Ok(())
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
/// depend on where the corruption landed. The refused BYTES are moved aside
/// ([`quarantine_store_file`]) before the empty store is allowed to publish over their name.
///
/// **Swap-window recovery.** [`publish_store_file`] leaves a one-sector window in which the live leaf
/// is already gone and the temp has not been renamed over it yet. A boot landing in that window finds
/// no [`HCRON_FILE`] and a complete [`HCRON_TMP_FILE`]; it adopts the temp and marks the table dirty
/// so the next flush republishes it under the live name. Without this the window would be exactly the
/// data loss the swap exists to prevent, moved one step later.
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
    // The live leaf first. Only if it is ABSENT does the staging leaf get a look — a live store that
    // merely fails to PARSE is a finding about the medium, not a reason to reach for a temp whose own
    // provenance is a crash.
    let mut leaf = HCRON_FILE;
    let mut read = read_store_file(HCRON_FILE, &mut buf);
    let mut from_temp = false;
    if matches!(read, Err(HcronError::NotFound)) {
        buf.clear();
        match read_store_file(HCRON_TMP_FILE, &mut buf) {
            Ok(()) => {
                leaf = HCRON_TMP_FILE;
                read = Ok(());
                from_temp = true;
            }
            Err(HcronError::NotFound) => buf.clear(), // neither leaf: genuinely a first boot
            Err(e) => {
                leaf = HCRON_TMP_FILE;
                read = Err(e);
            }
        }
    }

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
            // Move the refused bytes aside BEFORE `clear` marks the table dirty: the next flush
            // publishes a fresh empty store, and it must not publish it over the only copy of
            // whatever the user's bonds were. One generation is kept, as /HCRON/BTBOND.BAD.
            let q = quarantine_store_file(leaf);
            clear();
            serial_println!(
                ":: [hcron] load: {} ({}) -> store starts EMPTY, fail-closed (nothing partially adopted); refused bytes {} == witness ::",
                hcron_reason(e),
                leaf,
                quarantine_note(q)
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
                    // A store recovered from the STAGING leaf is not yet published under the live
                    // name. Dirty, so the next flush finishes the swap the crash interrupted.
                    s.dirty = from_temp;
                    s.loaded = true;
                    let n = s.count;
                    let q = s.seq;
                    drop(s);
                    if from_temp {
                        serial_println!(
                            ":: [hcron] loaded n={} from /{}/{} (seq={}) — the LIVE leaf was absent and the staging leaf parsed, so a previous boot died inside the publish swap; adopted and marked dirty so the next flush finishes it == witness ::",
                            n, HCRON_DIR, HCRON_TMP_FILE, q
                        );
                    } else {
                        serial_println!(
                            ":: [hcron] loaded n={} from {} (seq={}) == witness ::",
                            n, HCRON_PATH, q
                        );
                    }
                }
                Err(e) => {
                    let mut s = STORE.lock();
                    s.loaded = true;
                    drop(s);
                    let q = quarantine_store_file(leaf);
                    clear();
                    serial_println!(
                        ":: [hcron] load: {} ({}) -> store starts EMPTY, fail-closed (nothing partially adopted); refused bytes {} == witness ::",
                        hcron_reason(e),
                        leaf,
                        quarantine_note(q)
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

/// One phrase for what became of a refused image — the quarantine either happened or it is named why
/// it did not. "Kept" and "lost" must never be indistinguishable on the wire.
fn quarantine_note(q: Result<(), HcronError>) -> &'static str {
    match q {
        Ok(()) => "KEPT as /HCRON/BTBOND.BAD (one generation; recoverable off the medium)",
        Err(HcronError::ReadOnly) => "NOT kept (volume vetoes writes) — nothing was destroyed either",
        Err(_) => "NOT kept (the quarantine rename failed) — the next flush will publish over them",
    }
}

/// Is the `EHCI_HID` mutex held by SOMEONE right now?
///
/// **Named for what it can actually answer.** `spin::Mutex::is_locked` is a GLOBAL predicate: it
/// reports that the lock is taken, never by whom. It therefore CANNOT distinguish
///
///   * "this call stack is inside a `service_ehci_hid()` pass" — the bug this seam exists to
///     prevent — from
///   * "another task is mid-pass" — benign, and expected: `main.rs` spawns `usb-pump` (which calls
///     [`service`]) and `input` (which reaches `service_ehci_hid` at roughly 1 kHz) as two separate
///     preemptible tasks, so an interleaving that finds the lock taken is an ordinary scheduling
///     outcome on a correct build, not a defect.
///
/// The earlier version of this function was documented as "held on this core right now" and the
/// witness it fed asserted "this call site is inside it". Both claims were false whenever the lock
/// was merely contended, and the refusal they produced returned before the flush budget, so a
/// contended lock printed once per main-loop pass forever.
///
/// The predicate is still worth reading, because it is sound in exactly one direction:
///
///   * **not held** — nobody holds it, so THIS stack does not. A PROOF that the flush may run.
///   * **held** — UNKNOWN. Both readings say the same thing about this instant: do not write now.
///
/// So the flush treats a `true` as a DEFERRAL, never as a verdict about its caller, and bounds what
/// the deferral may print ([`note_defer`]). A sharper predicate is possible — a marker set and
/// cleared around the body of `service_ehci_hid` in `drivers/ehci/mod.rs`, compared against the
/// current task — but it belongs on the EHCI side of the seam, where the pass is bracketed.
///
/// Everywhere else there is no such lock and the answer is a constant `false`.
#[cfg(all(target_arch = "x86_64", feature = "ehcihid"))]
fn ehci_hid_busy() -> bool {
    crate::drivers::ehci::EHCI_HID.is_locked()
}

#[cfg(not(all(target_arch = "x86_64", feature = "ehcihid")))]
fn ehci_hid_busy() -> bool {
    false
}

/// Account for one deferred pass, and print about it AT MOST [`HCRON_DEFER_NOTES`] times per boot.
///
/// The module's stated law — "a volume that vetoes writes must not be able to make the main loop
/// print forever" ([`HCRON_FLUSH_ATTEMPTS`]) — applied to the other refusal path, which used to be
/// exempt from it. Two lines get emitted at most: the first deferral, which explains what the reading
/// does and does not mean, and one at [`HCRON_DEFER_STUCK`] consecutive, which says that the
/// contention has outlasted any interpretation this witness can choose between. After that the store
/// is silent about deferral for the rest of the boot.
///
/// Retries are NOT bounded, and that is deliberate: deferral is not a failure, it costs one atomic
/// load per pass, and a legitimately long service pass (an SSP chain runs for seconds) must not cost
/// the boot its persistence. It does not touch `flush_fails` or `gave_up` either — those count I/O
/// that was ATTEMPTED and refused by the volume, and no I/O is attempted here.
fn note_defer() {
    let n = {
        let mut s = STORE.lock();
        s.defers = s.defers.saturating_add(1);
        s.defers
    };
    let first = n == 1;
    let stuck = n == HCRON_DEFER_STUCK;
    if !(first || stuck) {
        return;
    }
    // THE BOUND, and it is checked where the print happens so the counter counts OUTPUT, not intent.
    {
        let mut s = STORE.lock();
        if s.defer_notes >= HCRON_DEFER_NOTES {
            return;
        }
        s.defer_notes = s.defer_notes.saturating_add(1);
    }
    if first {
        serial_println!(
            ":: [hcron] flush deferred — EHCI_HID is held at this instant, so the store write waits for a pass that can PROVE the lock free. This reading is GLOBAL (spin::Mutex::is_locked names no holder): \"held\" means UNKNOWN, NOT that this call site is inside the service pass. Retries continue every pass; the witness does not, and is capped at {} lines this boot == witness ::",
            HCRON_DEFER_NOTES
        );
    } else {
        serial_println!(
            ":: [hcron] flush deferred x{} consecutive — EHCI_HID has not been provably free for {} passes. Two readings fit and this witness cannot choose between them: a service pass legitimately holding the lock for a long time (an SSP chain runs for seconds), or a call site issuing the flush from INSIDE the pass. Retries continue, silently; this is the last line the store prints about deferral this boot == witness ::",
            n, n
        );
    }
}

/// The deferred write. Main-loop context only, and it proves that rather than assuming it.
///
/// Publishes the whole file when RAM differs from the medium, bumping `seq`. On failure the dirty
/// flag STAYS SET so the next pass retries, bounded at [`HCRON_FLUSH_ATTEMPTS`] consecutive
/// failures — a write-vetoed volume must not be able to make the main loop print forever.
///
/// The write itself is [`publish_store_file`]: stage into the temp leaf, prove it reads back and
/// parses, then swap it over the live one. The live store is never the thing being written into.
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
    if ehci_hid_busy() {
        // Cannot be proven safe on this pass, so nothing is written on this pass. Read the guard's
        // doc for why this is a deferral and not an accusation; `note_defer` is what keeps a
        // contended lock from printing once per main-loop pass forever.
        note_defer();
        return;
    }
    // Past the guard: whatever contention there was is over, so the consecutive count starts again.
    {
        let mut s = STORE.lock();
        s.defers = 0;
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

    // Stage into the TEMP leaf, prove it reads back and parses, then swap it over the live one. The
    // previous generation is never the thing being overwritten.
    let res = publish_store_file(&img[..len]);
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
// FIXTURE — the DEFERRAL BOUND, proven by making the guard fire thousands of times
// =========================================================================================

/// Drive [`flush_if_dirty`] with `EHCI_HID` GENUINELY HELD, more times than the witness budget, and
/// prove that the guard defers every pass, writes nothing, and goes quiet after
/// [`HCRON_DEFER_NOTES`] lines.
///
/// **Why this exists as an executed fixture rather than an argument.** The defect it closes was not
/// that the guard was absent; it was that the guard's refusal returned before every budget the module
/// had, so a contended lock printed once per main-loop pass with nothing to stop it. "It is bounded
/// now" is a claim about a code path that only runs when the lock is held, which no ordinary gate run
/// exercises. So the fixture holds the lock itself and makes the path run
/// `HCRON_DEFER_STUCK + PAST_CAP` times.
///
/// **It writes nothing and leaves nothing behind.** Every driven pass takes the deferral return, so
/// no block I/O is issued at all — which the fixture then PROVES by checking `seq` never moved and
/// the dirty flag it set is still set. The dirty flag and the deferral accounting are restored
/// afterwards, so a boot's real budget is not spent by a test; the bound holds independently for the
/// real path because it is the same code.
///
/// `try_lock`, never `lock`: this runs from the main loop, and blocking here to acquire `EHCI_HID`
/// would hold the very keyboard and trackpad the seam exists to keep free. A pass that cannot take
/// the lock uninstantly is simply not this fixture's pass — it returns unlatched and retries.
#[cfg(all(target_arch = "x86_64", feature = "ehcihid"))]
pub fn defer_bound_fixture_once() {
    use core::sync::atomic::{AtomicBool, Ordering};
    static DONE: AtomicBool = AtomicBool::new(false);
    if DONE.swap(true, Ordering::Relaxed) {
        return;
    }
    /// Passes driven PAST the point where the cap is already reached, to prove the silence is
    /// permanent and not merely a gap between the two notes.
    const PAST_CAP: u32 = 64;
    let driven = HCRON_DEFER_STUCK + PAST_CAP;

    if crate::drivers::block::info().is_none() {
        DONE.store(false, Ordering::Relaxed); // no medium => the flush returns before the guard
        return;
    }
    let (dirty0, seq0, notes0, defers0, gave_up) = {
        let s = STORE.lock();
        (s.dirty, s.seq, s.defer_notes, s.defers, s.gave_up)
    };
    if gave_up {
        serial_println!(
            ":: [hcron] deferral bound: the store has already given up, so flush_if_dirty returns before the guard — SKIPPED ::"
        );
        return;
    }
    // Take EHCI_HID for real. Without it the guard reads false and the fixture would prove nothing.
    let Some(_hid) = crate::drivers::ehci::EHCI_HID.try_lock() else {
        DONE.store(false, Ordering::Relaxed); // someone is mid-pass — not our turn, retry next pass
        return;
    };
    {
        let mut s = STORE.lock();
        s.dirty = true; // the flush must get past its first early-return to reach the guard
    }
    for _ in 0..driven {
        flush_if_dirty();
    }
    let (dirty1, seq1, notes1, defers1) = {
        let s = STORE.lock();
        (s.dirty, s.seq, s.defer_notes, s.defers)
    };
    drop(_hid);
    // Restore: the fixture's deferrals were synthetic and must not spend the boot's real budget.
    {
        let mut s = STORE.lock();
        s.dirty = dirty0;
        s.defers = defers0;
        s.defer_notes = notes0;
    }

    let emitted = notes1.saturating_sub(notes0);
    let counted = defers1.saturating_sub(defers0);
    let no_write = seq1 == seq0 && dirty1;
    if counted == driven && emitted <= HCRON_DEFER_NOTES && no_write {
        serial_println!(
            ":: [hcron] deferral bound: EHCI_HID HELD, flush_if_dirty driven {} times (past the {}-pass escalation and {} further) — every pass deferred, ZERO writes issued (seq unmoved at {}, still dirty), and the witness emitted {} line(s) against a cap of {} -> PASS ::",
            driven, HCRON_DEFER_STUCK, PAST_CAP, seq0, emitted, HCRON_DEFER_NOTES
        );
    } else if counted != driven {
        serial_println!(
            ":: [hcron] deferral bound: drove {} passes with EHCI_HID held but only {} reached the guard — the flush is returning somewhere earlier and this fixture proves nothing -> FAIL ::",
            driven, counted
        );
    } else if !no_write {
        serial_println!(
            ":: [hcron] deferral bound: a DEFERRED pass issued a write (seq {} -> {}, dirty={}) — the guard did not guard -> FAIL ::",
            seq0, seq1, dirty1
        );
    } else {
        serial_println!(
            ":: [hcron] deferral bound: {} deferred passes emitted {} witness lines against a cap of {} — the main loop can print without bound while the lock is contended -> FAIL ::",
            driven, emitted, HCRON_DEFER_NOTES
        );
    }
}

/// Builds with no `EHCI_HID` to hold: the guard is a compile-time `false` there, so there is no
/// deferral path to bound and nothing to prove. Kept as a no-op rather than a `cfg` at the call site
/// so [`service`] reads the same on every arch.
#[cfg(not(all(target_arch = "x86_64", feature = "ehcihid")))]
pub fn defer_bound_fixture_once() {}

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
// THE SELFTEST — the same load/flush, against the REAL block path (`hcronst`)
// =========================================================================================
//
// ITS OWN ARMING KNOB, and the reason is the repo's convention rather than a fresh opinion. The two
// selftests below (this one and `btbond::selftest_once`) perform BOOT-TIME WRITES TO THE USER'S BOOT
// MEDIUM: this one writes and unlinks `/HCRON/HCRNTEST.DAT`, and the bond one stages a fixture bond
// through the real flush, which creates `/HCRON/BTBOND.DAT` and leaves it behind as an empty store.
// Both check `write_veto()` first and both self-clean, so neither is reckless — but `sdw` gates
// `sdhc::write_block_512` separately from `sdhcblk` for exactly this reason: in this tree a
// destructive write gets a dedicated knob, so that arming a MECHANISM is never the same act as
// arming a TEST that writes.
//
// So `holocron` now arms the store alone — the seam M2's real consumer needs, which touches the
// medium only when a bond is actually staged — and `hcronst` (implies `holocron`) arms these two.
// Every gate that asserts the round-trip witnesses carries both knobs; see `x86-holocron.spec`.

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
#[cfg(feature = "hcronst")]
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
///   3. [`defer_bound_fixture_once`], which needs a block device and writes NOTHING — it belongs to
///      `holocron` rather than to `hcronst` for exactly that reason: it proves the guard, not the
///      medium, so arming the store arms its own bound-proof and nothing that touches the volume;
///   4. the class clients, which need the store loaded before they may answer a lookup;
///   5. [`flush_if_dirty`], LAST, so a record a client staged this pass reaches the medium this pass
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

    // Writes nothing (every driven pass takes the deferral return), so it is not behind `hcronst`.
    defer_bound_fixture_once();

    // The two boot-time-write selftests, behind their own arming knob — see the section header above.
    #[cfg(feature = "hcronst")]
    selftest_once();

    #[cfg(all(target_arch = "x86_64", feature = "ehcihid"))]
    crate::drivers::ehci::btbond::service();

    flush_if_dirty();
}
