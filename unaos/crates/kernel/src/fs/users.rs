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

//! USERS — the human-user record store and the login session (LOGIN M1, `login` knob).
//!
//! # What a user is
//!
//! RULINGS R51 (Peter, 2026-09-12): "keep working on multi-user so i can login, get a home folder
//! and all". `docs/SECURITY.md`'s POSIX hedge, taken: a user is a NAMED BUNDLE OF CAPABILITIES on
//! the same `owner`/`grants:*` attributes the U-chain and K1/K2 built. Concretely a user IS a
//! principal — kind `user`, canonical string `user:<name>` — and this module holds the three facts
//! the ruling names: the NAME, the CREDENTIAL and the HOME PATH. Nothing here is a second identity
//! machinery: the session principal is stamped into the SAME per-slot principal the loader already
//! stamps (`arch/*/syscall.rs`, `session_restamp` / `slot_user_stamp`), and the SAME `SYS_OPEN`
//! owner/grants check enforces it. This file never decides an access; it only says who is logged in.
//!
//! # Where the record lives, and why (the decision the brief asked for)
//!
//! A dedicated file, `USERS.DAT` in the ROOT DIRECTORY of the EL0 FAT VOLUME — the one
//! `fs::fat::mount()` answers, the same volume `SYS_OPEN` names files on (SO20: the EL0 namespace
//! is that volume's root), the same volume the K1 `UNAFS.ATR` sidecar lived on. It exists on every
//! platform the kernel boots from (the Pi's boot FAT, the Orin's ESP, x86's data volume, QEMU
//! virt's stick), which the VFS root does not: on virt no block device carries this kernel, so
//! `bootdisk` answers `[vfs] root -> NONE` and — LEDGER SO33 — caches that for the boot for every
//! caller. A user's home has to be where the user's programs can open files, and today that is
//! this volume; the native `/` placement (owner attributes on the Pi) moves with the native-EL0
//! namespace gap, not before it. NOT a class in the Holocron classed-record store
//! (`fs/holocron.rs`), for three reasons stated rather than implied:
//!   1. Holocron is the BT-BOND store: its file is `/HCRON/BTBOND.DAT`, its bound is 8 records of
//!      64 bytes, and its arming knob means "this boot may write its boot medium for a bond". A
//!      credential file has a different reader (the login screen, before any session exists), a
//!      different writer (create-first-user), and a different lifetime; coupling them makes the
//!      Bluetooth link key and the human's password hash one file with one blast radius.
//!   2. Holocron's flush is deferred to the main loop because its PRODUCER runs under a driver lock.
//!      A user is created from the login screen or a shell verb — ordinary context, no lock — so the
//!      write can be synchronous and the swap-not-overwrite discipline is applied here directly.
//!   3. The record must be readable with NO session open (that is what a login screen is), so it
//!      cannot live behind any principal-gated namespace; the root of the root volume is the one
//!      place every boot can reach before anyone is anyone.
//!
//! What an attacker holding the card can read: every user NAME, every user's 16-byte SALT, its
//! iteration count, and the 32-byte credential digest. Never a password: there is no plaintext
//! credential on disk or on the wire (the shell verb takes the password from the typed line and
//! hashes it in kernel RAM; the login screen never echoes it; no witness line prints it). What the
//! attacker CAN do with the card is an OFFLINE GUESS against the digest — since SECLOGIN M1 one
//! PBKDF2-HMAC-SHA256 run per guess at a count calibrated to ~250 ms on the CPU that created the row
//! (`PBKDF2_TARGET_MS`), never below `PBKDF2_ITERS_MIN` — so a weak password on a lost card is still a
//! weak password, slowly. Nothing is encrypted at rest and there is no hardware key store on a 2012
//! rMBP; `docs/dev/OS/04_SECURITY_IMMUNITY/multiuser.md` §1/§5 is the threat model and what that arc
//! needs.
//!
//! # On-disk format (v2 since SECLOGIN M1; v1 read and adopted) — fixed stride, CRC per row, fail-closed
//!
//! ```text
//! header (32): magic "UNAUSR1\0" | ver u8 = 2 | count u8 | seq u16 LE | next_uid u32 LE | reserved[12] | crc32 LE (over [0..28))
//! row   (128): name_len u8 | name[24] | home_len u8 | home[32] | salt[16] | hash[32]
//!              | uid u32 LE @106 | iters u32 LE @110 | kdf u8 @114 | reserved[5] | crc32 LE @120 (over [0..120)) | pad[4]
//! v1 (read only): header (16): magic | ver = 1 | count | seq | crc32 (over [0..12)); row: the same first 106 bytes, crc32 @106
//! ```
//!
//! Bad magic, unknown version, a bad header CRC, a bad row CRC, a count past the table, a short
//! file, a uid of 0, a duplicate uid, a uid at or past `next_uid`, or a PBKDF2 row under the floor
//! refuses the WHOLE image and the store starts EMPTY, witnessed — no partial adoption. A v1 image is
//! adopted into RAM as v2 (uid = index + 1, `kdf = KDF_LEGACY`, `next_uid = count + 1`) and rewritten
//! as v2 at its first flush; a legacy row is re-hashed at its next SUCCESSFUL login (`login`, the one
//! path holding a verified password). The update is a SWAP: `USERS.NEW` is written, read back and
//! parsed, and only then does `USERS.DAT` go and the temp get renamed over it; a boot that finds no
//! `USERS.DAT` but a parseable `USERS.NEW` adopts it. CRC-32 is [`crate::hash::crc32`], SHA-256 is
//! [`crate::hash::sha256`], PBKDF2 is [`crate::hash::pbkdf2_hmac_sha256`] — all arch-neutral, all in
//! every image. The FAT calls are the ones `holocron.rs`'s store helpers use (`locate_in_dir`/
//! `create_in_dir`/`write_grow`/`read_at`/`delete_located`/`rename_entry`), against parent cluster 0
//! (the root directory) and honouring `write_veto`.
//!
//! # Identity
//!
//! A row's `uid` is the user's identity on both arches (`multiuser.md` §2): allocated from `next_uid`,
//! never reissued, so a deleted user's owned files never fall to whoever next takes the row's slot or
//! the row's name. The slot index is storage.
//!
//! # Salt
//!
//! `sha256(cycle counter || name || seq)[..16]` until SECLOGIN M5 (`rand.rs`) replaces the source.
//! Its job is that two users with the same password do not share a digest and that no precomputed
//! table applies; it is not secret and is not claimed to be.

use spin::Mutex;

use crate::fs::fat::{FatError, FatFs};
use crate::hash::crc32;

// =========================================================================================
// FORMAT CONSTANTS
// =========================================================================================

/// On-disk magic. ASCII so an `xxd` of the medium names the file. It names the FILE; `ver` names the
/// LAYOUT, which is why the magic did not change at v2.
pub const USERS_MAGIC: [u8; 8] = *b"UNAUSR1\0";
/// Format version written. A reader that does not know a version refuses; v1 is READ (and adopted).
pub const USERS_VER: u8 = 2;
pub const USERS_VER_V1: u8 = 1;
/// Header length and the span its CRC covers — v2 (32 bytes, `next_uid` at 12) and v1 (16 bytes).
pub const USERS_HDR_LEN: usize = 32;
const USERS_HDR_CRC_SPAN: usize = 28;
pub const USERS_HDR_LEN_V1: usize = 16;
const USERS_HDR_CRC_SPAN_V1: usize = 12;
/// One row's stride on disk and the span its CRC covers — v2 (CRC at 120) and v1 (CRC at 106).
pub const USERS_ROW_LEN: usize = 128;
const USERS_ROW_CRC_SPAN: usize = 120;
const USERS_ROW_CRC_SPAN_V1: usize = 106;
/// SECLOGIN M1: the credential KDF a row names. `KDF_LEGACY` is v1's one SHA-256 and exists only so a
/// v1 row can be verified ONCE more, at the login that migrates it.
pub const KDF_LEGACY: u8 = 1;
pub const KDF_PBKDF2: u8 = 2;
/// SECLOGIN M1: the PBKDF2 count floor, ceiling and target. A row under the floor is refused at parse
/// and at create; the count is calibrated once per boot to `PBKDF2_TARGET_MS` on this CPU.
pub const PBKDF2_ITERS_MIN: u32 = 10_000;
pub const PBKDF2_ITERS_MAX: u32 = 4_000_000;
pub const PBKDF2_TARGET_MS: u64 = 250;
/// SECLOGIN M2: the first uid a fresh store issues (0 is "no user" everywhere a uid is read).
pub const FIRST_UID: u32 = 1;
/// Bounds. `NAME_MAX` is 8 since M2: the home directory is a FAT 8.3 leaf (`HOME/<NAME>`) on the EL0
/// volume, so a name is at most 8 bytes (the on-disk row keeps its 24-byte field; `user:<name>` is at
/// most 13, inside the 30-byte principal value field).
pub const MAX_USERS: usize = 8;
pub const NAME_MAX: usize = 8;
pub const HOME_MAX: usize = 32;
pub const SALT_LEN: usize = 16;
const HASH_LEN: usize = 32;
/// The live leaf, the swap temp, and the mount-table spellings of each (root of the root volume).
pub const USERS_FILE: &str = "USERS.DAT";
pub const USERS_TMP_FILE: &str = "USERS.NEW";
pub const USERS_PATH: &str = "/USERS.DAT";
pub const USERS_TMP_PATH: &str = "/USERS.NEW";
/// The image bound: header + a full table.
pub const USERS_IMAGE_MAX: usize = USERS_HDR_LEN + USERS_ROW_LEN * MAX_USERS;
/// Where a home lands: `/home/<name>` — `HOME/<NAME>` on the EL0 FAT volume, created at the first login
/// ([`ensure_home`], M2).
pub const HOME_ROOT: &str = "/home";

// =========================================================================================
// RECORDS
// =========================================================================================

/// One user, inline and `Copy` so the table is a plain array.
#[derive(Clone, Copy)]
pub struct UserRec {
    name_len: u8,
    name: [u8; NAME_MAX],
    home_len: u8,
    home: [u8; HOME_MAX],
    salt: [u8; SALT_LEN],
    hash: [u8; HASH_LEN],
    /// SECLOGIN M2: the non-recyclable identity (`multiuser.md` §2). Never 0, never reissued.
    uid: u32,
    /// SECLOGIN M1: PBKDF2 iteration count when `kdf == KDF_PBKDF2`; 1 on a legacy row.
    iters: u32,
    /// SECLOGIN M1: `KDF_LEGACY` (v1: one SHA-256) or `KDF_PBKDF2`.
    kdf: u8,
}

impl UserRec {
    const EMPTY: Self = UserRec {
        name_len: 0,
        name: [0; NAME_MAX],
        home_len: 0,
        home: [0; HOME_MAX],
        salt: [0; SALT_LEN],
        hash: [0; HASH_LEN],
        uid: 0,
        iters: 1,
        kdf: KDF_LEGACY,
    };

    pub fn name(&self) -> &[u8] {
        &self.name[..self.name_len as usize]
    }

    pub fn home(&self) -> &[u8] {
        &self.home[..self.home_len as usize]
    }

    /// The v2 row layout (`multiuser.md` §3.1). Offsets 0..106 are v1's exactly.
    fn write(&self, out: &mut [u8]) {
        out[..USERS_ROW_LEN].fill(0);
        out[0] = self.name_len;
        out[1..1 + NAME_MAX].copy_from_slice(&self.name);
        out[25] = self.home_len;
        out[26..26 + HOME_MAX].copy_from_slice(&self.home);
        out[58..58 + SALT_LEN].copy_from_slice(&self.salt);
        out[74..74 + HASH_LEN].copy_from_slice(&self.hash);
        out[106..110].copy_from_slice(&self.uid.to_le_bytes());
        out[110..114].copy_from_slice(&self.iters.to_le_bytes());
        out[114] = self.kdf;
        let c = crc32(&out[..USERS_ROW_CRC_SPAN]);
        out[120..124].copy_from_slice(&c.to_le_bytes());
    }

    /// The fields v1 and v2 share, validated as v1 always validated them.
    fn read_common(b: &[u8]) -> Option<Self> {
        let mut r = UserRec::EMPTY;
        r.name_len = b[0];
        r.name.copy_from_slice(&b[1..1 + NAME_MAX]);
        r.home_len = b[25];
        r.home.copy_from_slice(&b[26..26 + HOME_MAX]);
        r.salt.copy_from_slice(&b[58..58 + SALT_LEN]);
        r.hash.copy_from_slice(&b[74..74 + HASH_LEN]);
        if r.name_len as usize > NAME_MAX || r.home_len as usize > HOME_MAX || r.name_len == 0 {
            return None;
        }
        if !name_ok(r.name()) {
            return None;
        }
        Some(r)
    }

    /// A v1 row (CRC at 106 over 106 bytes), ADOPTED as v2: `uid` is the caller's (index + 1, the
    /// number the x86 tables carried for it under v1), `kdf = KDF_LEGACY`, `iters = 1`.
    fn read_v1(b: &[u8], uid: u32) -> Option<Self> {
        if b.len() < USERS_ROW_LEN {
            return None;
        }
        let want = u32::from_le_bytes([b[106], b[107], b[108], b[109]]);
        if crc32(&b[..USERS_ROW_CRC_SPAN_V1]) != want {
            return None;
        }
        let mut r = Self::read_common(b)?;
        r.uid = uid;
        r.iters = 1;
        r.kdf = KDF_LEGACY;
        Some(r)
    }

    /// A v2 row. A PBKDF2 row below the floor is a BAD ROW (the whole image is refused): a floor that
    /// only applied at creation could be edited off the card.
    fn read_v2(b: &[u8]) -> Option<Self> {
        if b.len() < USERS_ROW_LEN {
            return None;
        }
        let want = u32::from_le_bytes([b[120], b[121], b[122], b[123]]);
        if crc32(&b[..USERS_ROW_CRC_SPAN]) != want {
            return None;
        }
        let mut r = Self::read_common(b)?;
        r.uid = u32::from_le_bytes([b[106], b[107], b[108], b[109]]);
        r.iters = u32::from_le_bytes([b[110], b[111], b[112], b[113]]);
        r.kdf = b[114];
        match r.kdf {
            KDF_LEGACY => {}
            KDF_PBKDF2 if r.iters >= PBKDF2_ITERS_MIN => {}
            _ => return None,
        }
        Some(r)
    }
}

/// A user name: 1..=8 bytes of `[a-z0-9_-]`, first byte a letter. Lower-case only so the
/// principal string is canonical without a fold, and so a name is a valid 8.3 leaf on a FAT home.
pub fn name_ok(n: &[u8]) -> bool {
    if n.is_empty() || n.len() > NAME_MAX {
        return false;
    }
    if !n[0].is_ascii_lowercase() {
        return false;
    }
    n.iter().all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'_' || c == b'-')
}

/// The credential digest for a row, by the row's own `kdf`: the legacy v1 `sha256(salt || password)`
/// (one compression per guess — the whole of what B157 gap (1) measured) or PBKDF2-HMAC-SHA256 at the
/// row's own `iters` (`hash::pbkdf2_hmac_sha256`, `2 * iters` compressions). A row's cost is the
/// row's, so a credential hashed on the rMBP verifies at the rMBP's count on any box.
fn password_hash(rec: &UserRec, password: &[u8]) -> [u8; HASH_LEN] {
    if rec.kdf == KDF_PBKDF2 {
        let mut out = [0u8; HASH_LEN];
        crate::hash::pbkdf2_hmac_sha256(password, &rec.salt, rec.iters, &mut out);
        out
    } else {
        let mut h = crate::hash::Sha256::new();
        h.update(&rec.salt);
        h.update(password);
        h.finalize()
    }
}

/// The salt for a new row. Until SECLOGIN M5 lands an entropy source this is the v1 recipe —
/// uniqueness, not secrecy (two users with one password do not share a hash; no precomputed table
/// applies). M5 replaces the body with `rand::fill` and names its source on the wire.
fn new_salt(name: &[u8], seq: u16) -> [u8; SALT_LEN] {
    let mut sh = crate::hash::Sha256::new();
    sh.update(&crate::arch::now_cycles().to_le_bytes());
    sh.update(name);
    sh.update(&seq.to_le_bytes());
    let d = sh.finalize();
    let mut s = [0u8; SALT_LEN];
    s.copy_from_slice(&d[..SALT_LEN]);
    s
}

/// Constant-time equality over the two digests (a fold, never an early return).
fn digest_eq(a: &[u8; HASH_LEN], b: &[u8; HASH_LEN]) -> bool {
    let mut acc = 0u8;
    for i in 0..HASH_LEN {
        acc |= a[i] ^ b[i];
    }
    acc == 0
}

// =========================================================================================
// THE TABLE (RAM) + ITS IMAGE (pure)
// =========================================================================================

struct Table {
    loaded: bool,
    seq: u16,
    count: u8,
    /// SECLOGIN M2: the next uid to issue. Only ever increases; persisted in the v2 header.
    next_uid: u32,
    /// SECLOGIN M1: the PBKDF2 count calibrated on THIS boot (0 = not yet measured).
    kdf_iters: u32,
    rows: [UserRec; MAX_USERS],
}

static TABLE: Mutex<Table> = Mutex::new(Table {
    loaded: false,
    seq: 0,
    count: 0,
    next_uid: FIRST_UID,
    kdf_iters: 0,
    rows: [UserRec::EMPTY; MAX_USERS],
});

/// Why an image was refused. Every arm refuses the WHOLE image.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UsersError {
    BadMagic,
    BadVersion,
    BadHeaderCrc,
    BadRow,
    TooMany,
    Truncated,
    /// The name is not a valid user name (see [`name_ok`]).
    BadName,
    /// A user of that name already exists.
    Exists,
    /// The table is full.
    Full,
    /// No such user, or the credential did not verify. ONE arm on purpose: a login refusal never
    /// says which of the two it was.
    Refused,
    /// The volume could not be reached or written; the store did not change.
    Volume,
    /// SECLOGIN M1: the calibrated iteration count is below the floor; no credential is written.
    WeakKdf,
    /// SECLOGIN M2: the user is the open session's and cannot be deleted while logged in.
    InUse,
}

pub fn users_reason(e: UsersError) -> &'static str {
    match e {
        UsersError::BadMagic => "bad-magic",
        UsersError::BadVersion => "bad-version",
        UsersError::BadHeaderCrc => "bad-header-crc",
        UsersError::BadRow => "bad-row",
        UsersError::TooMany => "too-many",
        UsersError::Truncated => "truncated",
        UsersError::BadName => "bad-name",
        UsersError::Exists => "exists",
        UsersError::Full => "full",
        UsersError::Refused => "refused",
        UsersError::Volume => "volume",
        UsersError::WeakKdf => "weak-kdf",
        UsersError::InUse => "in-use",
    }
}

/// The parsed image: what `try_load` adopts and `flush` proves readable. `ver` is the version the
/// bytes carried (1 or 2) — a v1 image is returned ALREADY ADOPTED as v2 rows (uid = index + 1,
/// `kdf = KDF_LEGACY`, `next_uid = count + 1`), which is the migration rule of `multiuser.md` §3.2.
struct Parsed {
    ver: u8,
    seq: u16,
    count: u8,
    next_uid: u32,
    rows: [UserRec; MAX_USERS],
}

fn serialize_into(seq: u16, next_uid: u32, rows: &[UserRec], out: &mut [u8]) -> usize {
    let need = USERS_HDR_LEN + USERS_ROW_LEN * rows.len();
    debug_assert!(out.len() >= need);
    out[..USERS_HDR_LEN].fill(0);
    out[0..8].copy_from_slice(&USERS_MAGIC);
    out[8] = USERS_VER;
    out[9] = rows.len() as u8;
    out[10..12].copy_from_slice(&seq.to_le_bytes());
    out[12..16].copy_from_slice(&next_uid.to_le_bytes());
    let c = crc32(&out[..USERS_HDR_CRC_SPAN]);
    out[28..32].copy_from_slice(&c.to_le_bytes());
    let mut at = USERS_HDR_LEN;
    for r in rows {
        r.write(&mut out[at..at + USERS_ROW_LEN]);
        at += USERS_ROW_LEN;
    }
    at
}

fn parse_image(img: &[u8]) -> Result<Parsed, UsersError> {
    if img.len() < USERS_HDR_LEN_V1 {
        return Err(UsersError::Truncated);
    }
    if img[0..8] != USERS_MAGIC {
        return Err(UsersError::BadMagic);
    }
    let ver = img[8];
    let (hdr_len, crc_span, crc_at) = match ver {
        USERS_VER_V1 => (USERS_HDR_LEN_V1, USERS_HDR_CRC_SPAN_V1, USERS_HDR_CRC_SPAN_V1),
        USERS_VER => (USERS_HDR_LEN, USERS_HDR_CRC_SPAN, USERS_HDR_CRC_SPAN),
        _ => return Err(UsersError::BadVersion),
    };
    if img.len() < hdr_len {
        return Err(UsersError::Truncated);
    }
    let want = u32::from_le_bytes([img[crc_at], img[crc_at + 1], img[crc_at + 2], img[crc_at + 3]]);
    if crc32(&img[..crc_span]) != want {
        return Err(UsersError::BadHeaderCrc);
    }
    let count = img[9] as usize;
    if count > MAX_USERS {
        return Err(UsersError::TooMany);
    }
    if img.len() < hdr_len + USERS_ROW_LEN * count {
        return Err(UsersError::Truncated);
    }
    let seq = u16::from_le_bytes([img[10], img[11]]);
    let mut rows = [UserRec::EMPTY; MAX_USERS];
    let mut next_uid = if ver == USERS_VER { u32::from_le_bytes([img[12], img[13], img[14], img[15]]) } else { count as u32 + 1 };
    if next_uid == 0 {
        return Err(UsersError::BadRow);
    }
    for i in 0..count {
        let at = hdr_len + USERS_ROW_LEN * i;
        let b = &img[at..at + USERS_ROW_LEN];
        let r = if ver == USERS_VER { UserRec::read_v2(b) } else { UserRec::read_v1(b, i as u32 + 1) }.ok_or(UsersError::BadRow)?;
        // every uid is below the counter and unique — a row that violates either would let a uid be
        // reissued, which is the one thing the counter exists to refuse
        if r.uid == 0 || r.uid >= next_uid || (0..i).any(|j| rows[j].uid == r.uid) {
            return Err(UsersError::BadRow);
        }
        rows[i] = r;
    }
    if next_uid < FIRST_UID {
        next_uid = FIRST_UID;
    }
    Ok(Parsed { ver, seq, count: count as u8, next_uid, rows })
}

// =========================================================================================
// STORAGE — load once, swap on write
// =========================================================================================

fn map_fat(e: FatError) -> UsersError {
    match e {
        FatError::NotFound => UsersError::Volume,
        _ => UsersError::Volume,
    }
}

/// The whole of root-directory file `leaf` (bounded by the image size), or `None` if it is
/// absent, a directory, over-long or unreadable.
fn read_root_file(fs: &FatFs, leaf: &str) -> Option<alloc::vec::Vec<u8>> {
    let (de, _, _) = fs.locate_in_dir(0, leaf).ok()?;
    if de.is_dir || de.size as usize > USERS_IMAGE_MAX {
        return None;
    }
    let mut out = alloc::vec::Vec::new();
    fs.read_at(de.first_cluster(), de.size, 0, &mut out, de.size as usize).ok()?;
    if out.len() != de.size as usize {
        return None;
    }
    Some(out)
}

/// Delete root-directory file `leaf` if it exists (absent is fine; a directory is refused).
fn delete_root_file(fs: &FatFs, leaf: &str) -> Result<(), UsersError> {
    match fs.locate_in_dir(0, leaf) {
        Ok((de, dl, doff)) => {
            if de.is_dir {
                return Err(UsersError::Volume);
            }
            fs.delete_located(dl, doff, de.first_cluster()).map(|_| ()).map_err(map_fat)
        }
        Err(FatError::NotFound) => Ok(()),
        Err(e) => Err(map_fat(e)),
    }
}

/// Create root-directory file `leaf` fresh (any prior one deleted first) with `data`.
fn write_root_file(fs: &FatFs, leaf: &str, data: &[u8]) -> Result<(), UsersError> {
    if fs.write_veto().is_some() {
        return Err(UsersError::Volume);
    }
    delete_root_file(fs, leaf)?;
    let (_, l, o) = fs.create_in_dir(0, leaf, 0x20).map_err(map_fat)?;
    let (written, _, _) = fs.write_grow(0, 0, l, o, 0, data).map_err(map_fat)?;
    if written != data.len() {
        return Err(UsersError::Volume);
    }
    Ok(())
}

/// Adopt the on-disk store into RAM, once. `USERS.DAT` first; a parseable `USERS.NEW` with no live
/// leaf is the interrupted-swap recovery. A refused image starts EMPTY and says why. Returns
/// `false` while the EL0 FAT volume is not mountable yet (the caller retries next pass).
pub fn load_once() -> bool {
    try_load().is_ok()
}

/// The mount attempt behind [`load_once`], with the FAT error kept for the witness.
fn try_load() -> Result<(), FatError> {
    if TABLE.lock().loaded {
        return Ok(());
    }
    let fs = crate::fs::fat::mount()?;
    let veto = if fs.write_veto().is_some() { "ro" } else { "rw" };
    let (src, img) = match read_root_file(&fs, USERS_FILE) {
        Some(b) => ("dat", Some(b)),
        None => match read_root_file(&fs, USERS_TMP_FILE) {
            Some(b) => ("new", Some(b)),
            None => ("none", None),
        },
    };
    let mut t = TABLE.lock();
    match img {
        None => {
            t.next_uid = FIRST_UID;
            serial_println!("[users] load volume=el0-fat({}) src=none users=0 (fresh store)", veto);
        }
        Some(b) => match parse_image(&b) {
            Ok(p) => {
                t.seq = p.seq;
                t.count = p.count;
                t.rows = p.rows;
                t.next_uid = p.next_uid;
                serial_println!(
                    "[users] load volume=el0-fat({}) src={} users={} seq={} ver={} next_uid={} legacy_rows={}",
                    veto, src, p.count, p.seq, p.ver, p.next_uid,
                    (0..p.count as usize).filter(|&i| p.rows[i].kdf == KDF_LEGACY).count()
                );
            }
            Err(e) => {
                t.next_uid = FIRST_UID;
                serial_println!(
                    "[users] load volume=el0-fat({}) src={} REFUSED reason={} len={} (store starts empty)",
                    veto,
                    src,
                    users_reason(e),
                    b.len()
                );
            }
        },
    }
    t.loaded = true;
    Ok(())
}

/// Publish the RAM table: write the temp, read it back and parse it, then swap it over the live
/// leaf. A failure anywhere leaves the previous live leaf in place and the RAM table as written.
/// Always writes v2 (`USERS_VER`): a v1 image adopted at load goes to disk as v2 at its first flush.
fn flush(t: &mut Table) -> Result<(), UsersError> {
    let fs = crate::fs::fat::mount().map_err(|_| UsersError::Volume)?;
    let mut img = [0u8; USERS_IMAGE_MAX];
    t.seq = t.seq.wrapping_add(1);
    let n = serialize_into(t.seq, t.next_uid, &t.rows[..t.count as usize], &mut img);
    // 1. a fresh temp
    write_root_file(&fs, USERS_TMP_FILE, &img[..n])?;
    // 2. prove the new generation is readable on the medium before the old one goes
    let back = read_root_file(&fs, USERS_TMP_FILE).ok_or(UsersError::Volume)?;
    if back[..] != img[..n] {
        return Err(UsersError::Volume);
    }
    let p = parse_image(&back)?;
    if p.seq != t.seq || p.count != t.count || p.next_uid != t.next_uid {
        return Err(UsersError::Volume);
    }
    // 3. swap: one directory-slot rename window
    delete_root_file(&fs, USERS_FILE)?;
    fs.rename_entry(0, USERS_TMP_FILE, USERS_FILE).map_err(map_fat)?;
    Ok(())
}

// =========================================================================================
// THE API
// =========================================================================================

/// How many users exist. `0` is the create-first-user state the login screen shows.
pub fn count() -> usize {
    TABLE.lock().count as usize
}

/// The `uid` of `name`, or `None`. SECLOGIN M2: this is the NON-RECYCLABLE identity both arches
/// compare (x86 in its u32 tables, aarch64 inside the `user:<name>#<uid>` principal string) — never
/// the row's index, which is storage and is reused after a delete. Until M2 it was `row + 1`.
pub fn id_of(name: &[u8]) -> Option<u32> {
    let t = TABLE.lock();
    (0..t.count as usize).find(|&i| t.rows[i].name() == name).map(|i| t.rows[i].uid)
}

/// `/home/<name>` for an existing user, copied into `out`; the length written.
pub fn home_of(name: &[u8], out: &mut [u8; HOME_MAX]) -> Option<usize> {
    let t = TABLE.lock();
    let i = (0..t.count as usize).find(|&i| t.rows[i].name() == name)?;
    let h = t.rows[i].home();
    out[..h.len()].copy_from_slice(h);
    Some(h.len())
}

/// Create a user: validate the name, refuse a duplicate or a full table, salt + hash the
/// password, publish. The first user created on an empty store is the create-first-user path
/// the login screen drives; every later one is the same call. Returns the new row's `uid`.
///
/// SECLOGIN M1/M2: the row is written `kdf = KDF_PBKDF2` at the table's calibrated iteration count
/// (refused below the floor — a count that only applied here could be edited off the card, so the
/// parser refuses it too), and its `uid` comes from `next_uid`, which only ever increases.
pub fn create_user(name: &[u8], password: &[u8]) -> Result<u32, UsersError> {
    if !name_ok(name) {
        return Err(UsersError::BadName);
    }
    let mut t = TABLE.lock();
    if !t.loaded {
        return Err(UsersError::Volume);
    }
    if (0..t.count as usize).any(|i| t.rows[i].name() == name) {
        return Err(UsersError::Exists);
    }
    if t.count as usize >= MAX_USERS || t.next_uid == u32::MAX {
        return Err(UsersError::Full);
    }
    let iters = calibrated_iters(&mut t);
    if iters < PBKDF2_ITERS_MIN {
        serial_println!("[users] kdf REFUSED iters={} floor={} (a credential is never written below the floor)", iters, PBKDF2_ITERS_MIN);
        return Err(UsersError::WeakKdf);
    }
    let mut r = UserRec::EMPTY;
    r.name_len = name.len() as u8;
    r.name[..name.len()].copy_from_slice(name);
    let mut home = [0u8; HOME_MAX];
    let hl = HOME_ROOT.len() + 1 + name.len();
    home[..HOME_ROOT.len()].copy_from_slice(HOME_ROOT.as_bytes());
    home[HOME_ROOT.len()] = b'/';
    home[HOME_ROOT.len() + 1..hl].copy_from_slice(name);
    r.home_len = hl as u8;
    r.home = home;
    r.salt = new_salt(name, t.seq);
    r.uid = t.next_uid;
    r.kdf = KDF_PBKDF2;
    r.iters = iters;
    r.hash = password_hash(&r, password);
    let i = t.count as usize;
    t.rows[i] = r;
    t.count += 1;
    t.next_uid += 1;
    match flush(&mut t) {
        Ok(()) => Ok(r.uid),
        Err(e) => {
            // the publish failed: the store did not change, so neither does RAM
            t.rows[i] = UserRec::EMPTY;
            t.count -= 1;
            t.next_uid -= 1;
            Err(e)
        }
    }
}

/// Does `password` verify for `name`? One answer for "no such user" and "wrong password".
///
/// SECLOGIN M1 — CONSTANT-TIME END TO END. The row is COPIED out of the table and the lock dropped
/// before the KDF runs (a 250 ms stretch under `TABLE` would stall the login screen's own painter,
/// which reads `name_at` under the same lock). An UNKNOWN name runs the SAME KDF, at the table's
/// calibrated count, against a fixed dummy salt and a digest that cannot match — so the clock says
/// nothing about which names exist, which is the property LOGINFLOW's one-answer denial has on the
/// glass and on the wire and would otherwise lose through timing. `digest_eq` is a fold, never an
/// early return. A legacy (v1) row is one SHA-256 until it migrates; that reveals the row's VERSION,
/// not its password, and `login` migrates it at the next success.
pub fn verify(name: &[u8], password: &[u8]) -> bool {
    let (found, rec, iters) = {
        let mut t = TABLE.lock();
        let iters = calibrated_iters(&mut t);
        match (0..t.count as usize).find(|&i| t.rows[i].name() == name) {
            Some(i) => (true, t.rows[i], iters),
            None => (false, UserRec::EMPTY, iters),
        }
    };
    let h = if found {
        password_hash(&rec, password)
    } else {
        let mut dummy = UserRec::EMPTY;
        dummy.kdf = KDF_PBKDF2;
        dummy.iters = iters;
        password_hash(&dummy, password)
    };
    // `found` is folded in AFTER the compare so the compare always runs.
    digest_eq(&h, &rec.hash) & found
}

/// Log in: verify, then open the session under `user:<name>` — the ONE principal every program
/// launched from now on is stamped with (`arch::syscall::session_login`). Refused means no change.
///
/// SECLOGIN M1: this is ALSO the migration point. `verify` is pure (it is what the screen asks first,
/// LOGINFLOW M2); a row still carrying the v1 credential (`kdf = KDF_LEGACY`) is re-hashed HERE, on the
/// one path that holds a verified password, and the file goes to disk as v2 through the same swap.
pub fn login(name: &[u8], password: &[u8]) -> Result<(), UsersError> {
    if !verify(name, password) {
        serial_println!("[users] login refused");
        return Err(UsersError::Refused);
    }
    rehash_if_legacy(name, password);
    let id = id_of(name).ok_or(UsersError::Refused)?;
    if !arch_session_login(id, name) {
        return Err(UsersError::Refused);
    }
    // M2: `/home/<name>` exists from the first login on — created here, on the EL0 FAT volume. A volume
    // that cannot take the write (read-only, vetoed) still opens the session: the home is owed, not the
    // login, and the witness line names the refusal.
    if let Err(e) = ensure_home(name) {
        serial_println!("[users] home=/home/{} NOT created reason={} volume=el0-fat", core::str::from_utf8(name).unwrap_or("?"), users_reason(e));
    }
    {
        let mut s = SESSION_LOCAL.lock();
        s.0[..name.len()].copy_from_slice(name);
        s.1 = name.len() as u8;
        s.2 = id;
    }
    let mut nb = [0u8; NAME_MAX];
    nb[..name.len()].copy_from_slice(name);
    serial_println!(
        "[users] login ok user={} id={} principal=user:{}#{}",
        core::str::from_utf8(name).unwrap_or("?"),
        id,
        core::str::from_utf8(&nb[..name.len()]).unwrap_or("?"),
        id
    );
    Ok(())
}

/// M2: make `/home/<name>` exist on the EL0 FAT volume — `HOME/` at the root, `<NAME>` inside it —
/// idempotent. Returns `"created"` or `"exists"`. FAT carries no owner attribute: the DIRECTORY has no
/// ACL row (LEDGER SO35); the FILES a program creates inside it are owned through the SYS_OPEN
/// owner/grants rows exactly as any private create, and that is what the home ACL proof exercises.
pub fn ensure_home(name: &[u8]) -> Result<&'static str, UsersError> {
    let fs = crate::fs::fat::mount().map_err(|_| UsersError::Volume)?;
    let home_fc = match fs.locate_in_dir(0, "HOME") {
        Ok((de, _, _)) if de.is_dir => de.first_cluster(),
        Ok(_) => return Err(UsersError::Volume),
        Err(FatError::NotFound) => {
            if fs.write_veto().is_some() {
                return Err(UsersError::Volume);
            }
            fs.create_dir(0, "HOME").map_err(map_fat)?.0.first_cluster()
        }
        Err(e) => return Err(map_fat(e)),
    };
    let leaf = core::str::from_utf8(name).map_err(|_| UsersError::BadName)?;
    let verdict = match fs.locate_in_dir(home_fc, leaf) {
        Ok((de, _, _)) if de.is_dir => "exists",
        Ok(_) => return Err(UsersError::Volume),
        Err(FatError::NotFound) => {
            if fs.write_veto().is_some() {
                return Err(UsersError::Volume);
            }
            fs.create_dir(home_fc, leaf).map_err(map_fat)?;
            "created"
        }
        Err(e) => return Err(map_fat(e)),
    };
    serial_println!("[users] home=/home/{} {} volume=el0-fat", leaf, verdict);
    Ok(verdict)
}

/// M2 fixture: the home ACL proof where the syscall layer exists (see [`arch_session_login`]);
/// `"unlinked"` on an image with no EL0 regime.
#[cfg(feature = "loginst")]
fn home_acl_proof(name: &[u8]) -> &'static str {
    #[cfg(all(target_arch = "aarch64", feature = "aarch64_el0"))]
    {
        let mut p = [0u8; 32];
        let pre = b"HOME/";
        p[..5].copy_from_slice(pre);
        p[5..5 + name.len()].copy_from_slice(name);
        let tail = b"/NOTES.TXT";
        p[5 + name.len()..5 + name.len() + tail.len()].copy_from_slice(tail);
        let path = core::str::from_utf8(&p[..5 + name.len() + tail.len()]).unwrap_or("HOME/X/NOTES.TXT");
        return if crate::arch::syscall::home_acl_fixture(path) { "ok" } else { "FAIL" };
    }
    #[cfg(target_arch = "x86_64")]
    {
        let id = id_of(name).unwrap_or(0);
        return if crate::arch::syscall::home_acl_fixture(id) { "ok" } else { "FAIL" };
    }
    #[cfg(not(any(target_arch = "x86_64", all(target_arch = "aarch64", feature = "aarch64_el0"))))]
    {
        let _ = name;
        "unlinked"
    }
}

/// SO37 fixture: **the session epoch, proved by refusal**, where the syscall layer exists (the same
/// dispatch [`home_acl_proof`] uses and for the same reason — an image with no EL0 regime can launch no
/// program, so there is no stamp to strand and the leg is `"unlinked"`, not a pass). Called with the
/// session OPEN; the arch fixture closes it, re-opens it as the SAME user and leaves session 2 running,
/// which [`login_fixture`]'s own `logout()` then closes.
#[cfg(feature = "loginst")]
fn epoch_proof(name: &[u8]) -> &'static str {
    #[cfg(all(target_arch = "aarch64", feature = "aarch64_el0"))]
    {
        let mut p = [0u8; 32];
        p[..5].copy_from_slice(b"HOME/");
        p[5..5 + name.len()].copy_from_slice(name);
        let tail = b"/EPOCH.TXT";
        p[5 + name.len()..5 + name.len() + tail.len()].copy_from_slice(tail);
        let path = core::str::from_utf8(&p[..5 + name.len() + tail.len()]).unwrap_or("HOME/X/EPOCH.TXT");
        let id = id_of(name).unwrap_or(0);
        return if crate::arch::syscall::session_epoch_fixture(path, id, name) { "ok" } else { "FAIL" };
    }
    #[cfg(target_arch = "x86_64")]
    {
        let id = id_of(name).unwrap_or(0);
        return if crate::arch::syscall::session_epoch_fixture(id, name) { "ok" } else { "FAIL" };
    }
    #[cfg(not(any(target_arch = "x86_64", all(target_arch = "aarch64", feature = "aarch64_el0"))))]
    {
        let _ = name;
        "unlinked"
    }
}

/// Log out: END the session's programs (SECLOGIN M3), close the session, and BURN THE EPOCH (SO37 — see
/// `arch::syscall::session_logout`), so any stamp from the closed session that somehow survived reads
/// ANONYMOUS from `slot_ppid_of` down. Returns `(ended, windows)`; the wire carries both.
pub fn logout() -> (usize, usize) {
    // SECLOGIN M3: the session's programs are ENDED first (windows closed, then killed through the close
    // box's own path), THEN the stamps — a program cannot outlive the session that started it.
    let (ended, windows) = arch_session_logout();
    {
        let mut s = SESSION_LOCAL.lock();
        s.1 = 0;
        s.2 = 0;
    }
    // SO37 ON THE WIRE. The epoch printed is the one now LIVE, i.e. the one that has just been opened by
    // this logout — so `epoch=N` says "every stamp carrying N-1 or older is refused from here on", and a
    // boot's session boundaries are countable on serial with no fixture armed. This is the ONE string SO37
    // adds to a shipping (`login`, no `loginst`) image; the enforcement itself is pure control flow and
    // deliberately prints nothing on the SYS_OPEN path, which is hot.
    serial_println!("[users] logout epoch={} ended={} windows={} (SO37: stamps from the closed session are refused; M3: its programs are ended first)", arch_session_epoch(), ended, windows);
    (ended, windows)
}

/// The logged-in user's name, copied into `out`; `None` when no session is open.
pub fn whoami(out: &mut [u8; NAME_MAX]) -> Option<usize> {
    let s = SESSION_LOCAL.lock();
    let n = s.1 as usize;
    if n == 0 {
        return None;
    }
    out[..n].copy_from_slice(&s.0[..n]);
    Some(n)
}

/// The open session as THIS module records it: (name, name_len, id; len 0 = no session). The
/// login screen and `whoami` read here; the arch mirror below is the principal machinery's copy.
static SESSION_LOCAL: Mutex<([u8; NAME_MAX], u8, u32)> = Mutex::new(([0u8; NAME_MAX], 0, 0));

/// Mirror the session into the SESSION PRINCIPAL where the syscall layer exists: x86 always,
/// aarch64 under `aarch64_el0` (the cfg on `arch/aarch64/mod.rs`'s `pub mod syscall` — the Pi's
/// `baremetal`, the Orin's `tegra_el0`, QEMU's `virt_el0`). A plain aarch64 image has no EL0
/// regime, so no program can be launched under a principal there; the store, the credential and
/// the session record still work (that is what `test-arm` without `virt_el0` proves), and the
/// principal half is proven where a regime exists (`UNAOS_VIRT_EL0=1`, the boards, x86).
fn arch_session_login(id: u32, name: &[u8]) -> bool {
    #[cfg(any(target_arch = "x86_64", feature = "aarch64_el0"))]
    {
        return crate::arch::syscall::session_login(id, name);
    }
    #[cfg(not(any(target_arch = "x86_64", feature = "aarch64_el0")))]
    {
        let _ = (id, name);
        true
    }
}

/// SECLOGIN M3: `(ended, windows)` — the closing session's programs that were ended and the windows
/// they held; `(0, 0)` on an image with no EL0 regime, where nothing can have been launched.
fn arch_session_logout() -> (usize, usize) {
    #[cfg(any(target_arch = "x86_64", feature = "aarch64_el0"))]
    {
        return crate::arch::syscall::session_logout();
    }
    #[cfg(not(any(target_arch = "x86_64", feature = "aarch64_el0")))]
    (0, 0)
}

/// SO37: the LIVE session epoch where the syscall layer exists (see [`arch_session_login`]); `0` on an
/// image with no EL0 regime, where no program can be launched and so no stamp can be stranded.
fn arch_session_epoch() -> u32 {
    #[cfg(any(target_arch = "x86_64", feature = "aarch64_el0"))]
    {
        return crate::arch::syscall::session_epoch();
    }
    #[cfg(not(any(target_arch = "x86_64", feature = "aarch64_el0")))]
    0
}

/// Does this image carry the session principal (see [`arch_session_login`])? On the witness line.
const fn principal_linked() -> bool {
    cfg!(any(target_arch = "x86_64", feature = "aarch64_el0"))
}

// =========================================================================================
// THE SHELL VERBS — `login <name> <password>` / `logout`
// =========================================================================================

/// Serviced from `shell.rs`'s host arm. The password arrives on the typed line (the fixture-driving
/// path the M1 brief names); it is hashed in kernel RAM and never printed. The login SCREEN (M3)
/// is the human path and does not echo.
pub fn shell_verb(verb: &str, args: &[&str], console: &mut crate::console::Console) {
    match verb {
        "login" => {
            let (name, pw) = match (args.first(), args.get(1)) {
                (Some(n), Some(p)) => (*n, *p),
                _ => return console.println("usage: login <name> <password>"),
            };
            if !load_once() {
                return console.println("login: storage is not up (-ENODEV)");
            }
            if count() == 0 {
                match create_user(name.as_bytes(), pw.as_bytes()) {
                    Ok(_) => console.println(&alloc::format!("login: created first user {}", name)),
                    Err(e) => return console.println(&alloc::format!("login: cannot create {}: {}", name, users_reason(e))),
                }
            }
            match login(name.as_bytes(), pw.as_bytes()) {
                Ok(()) => console.println(&alloc::format!("logged in as {} (user:{})", name, name)),
                Err(_) => console.println("login: refused (-EACCES)"),
            }
        }
        "logout" => {
            let mut nb = [0u8; NAME_MAX];
            match whoami(&mut nb) {
                Some(n) => {
                    let _ = logout();
                    console.println(&alloc::format!("logged out {}", core::str::from_utf8(&nb[..n]).unwrap_or("?")));
                }
                None => console.println("logout: no session is open"),
            }
        }
        _ => {}
    }
}

// =========================================================================================
// THE SCREEN'S ENTRY POINTS (M3) — what the key routes and the desktop ignition call
// =========================================================================================

/// M3: offer a key to the login screen; `true` = consumed (the screen is up). Gated where the screen is
/// built (the crystal's gate: x86 `wc`, aarch64 `desktop_firmware`); `false` elsewhere, so a route
/// compiled without a desktop is the pre-M3 route. Every route calls this BEFORE its serial echo.
pub fn screen_key(c: u8) -> bool {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    {
        return crate::video::crystal::login::consume_key(c);
    }
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    {
        let _ = c;
        false
    }
}

/// SO36 + SO44 — **THE PRESS TWIN OF [`screen_key`], and the whole of the input gate.**
///
/// `true` = the press is the SCREEN's and no window arm may run. It answers `true` for EVERY press while
/// the screen is up — inside its rectangle and outside it alike — which is the sentence the whole fix is:
///
///  * **SO44 (Peter, on the glass): *"the login window appeared over the top of the gui and when i click
///    it it went away"*.** The screen's `wm` row is minted in the shell/desktop owner band (`login::OWNER`
///    = 0), which `wm::hit_test` never names — LOGINCLOSE measured that and read it as "no press can
///    reach the row", which was true and was the wrong consequence. A press that hits NOTHING does not
///    close the window; it lands on whatever is BENEATH, which both routers then raise and focus, and the
///    screen is left behind the desktop it no longer covers. LOGINBOOT measured the miss at the screen's
///    own centre: `[login] press-probe win=1 centre=(640,331) hit=0 verdict=NOBODY`.
///  * **SO36 (the furniture half): before a login the dock's tiles, the crystal and the window menu still
///    answer the pointer,** so a program could be launched under no principal from the login screen.
///
/// ONE statement closes both, because both are the same defect: the screen is a WINDOW THAT HAPPENS TO BE
/// ON TOP where it needed to be a MODAL BARRIER. Asked FIRST in `video::strip::press_route` — ahead of
/// winmenu, crystal and dock, and therefore ahead of every window arm in both routers — the barrier is
/// what the router consults before it consults anything else, and nothing below it runs at all: no tile
/// launches, no menu opens, no window is raised, nothing takes focus.
///
/// **The alternative was considered and REJECTED** (LOGINBOOT, and this executor agrees): special-casing
/// `owner_asid == 0` in `wm::hit_test` so the screen's row names itself. That hands the screen a control
/// cluster and a close box back — the exact defect LOGINCLOSE measured and belted — and it would gate only
/// the points inside the rectangle, leaving every press outside it to the dock. Modality is a property of
/// the ROUTER, not of the row.
///
/// Same gate as [`screen_key`] (x86 `wc`, aarch64 `desktop_firmware`): `false` where no desktop is built,
/// so a router compiled without a screen is the pre-SO36 router.
pub fn screen_press(x: i32, y: i32) -> bool {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    {
        return crate::video::crystal::login::press_swallow(x, y);
    }
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    {
        let _ = (x, y);
        false
    }
}

/// M3: the desktop boots to the login screen — called once at desktop ignition; a no-op where no
/// screen is built, and idempotent (the screen keeps its own once-latch).
pub fn screen_open_once() {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    crate::video::crystal::login::open_once();
}

/// SO43 — **THE IGNITION PREDICATE, and the whole of what SO43 was.** The desktop boots to the login
/// screen iff the DESKTOP EXISTS; the console's glyph ROUTE is read, reported, and deliberately not
/// consulted.
///
/// ## The defect this function is the shape of
///
/// LOGIN M3 put the open at four ignition sites, and two of them wrote the gate as
/// `if activate() { … }`. That return value is `fbcon::console_is_routed()` — a statement about where
/// the CONSOLE's glyphs land, not about whether a desktop came up — and on a board whose panel reads
/// 0 at the first call it is `false` on a boot whose desktop came up perfectly. The Orin printed
/// exactly that, every flight
/// (`docs/dev/evidence/orin28/render14-boot1-desktop-menubar.log`, `awk 'index($0,"[deskcascade]")'`):
///
/// ```text
/// [deskcascade] -> CASCADED windows=2 bar=1 owns_pixels=1 route=ROUTED activate=false
/// ```
///
/// `bar=1` — the desktop exists. `activate=false` — the gate said no. So the first thing anyone sees
/// on this machine was gated on a flag that is false exactly where it matters, and the x86 ladder
/// never caught it because there `activate()` is true.
///
/// ## Why BOTH facts are arguments
///
/// `console_routed` is taken, printed and thrown away on purpose. LAWS §5: *"say what the check
/// measures and what the decision needs; if those are different sentences, the gap is the error"* —
/// this seam is handed both sentences so the gap is a line on the wire and a leg in the fixture
/// (`video/login.rs`'s IGNITION leg drives the Orin's own tuple, `desktop_up=true
/// console_routed=false`, and reds the instant the rule consults the second term again). A seam that
/// silently took one argument could not be told from the defect it replaces.
///
/// `desktop_up` is the CALLER's readback of the one unconditional step of its own bring-up — the menu
/// bar — never a local it inferred from control flow, which is the same discipline
/// `desktop_firmware::activate` applies to `console_is_routed` itself.
///
/// Idempotent: the screen keeps its own once-latch, so a second ignition site on the same boot is a
/// swap and a return.
pub fn screen_open_at_ignition(desktop_up: bool, console_routed: bool) {
    serial_println!(
        "[login] ignition desktop_up={} console_routed={} -> {} (SO43: the screen is gated on the DESKTOP EXISTING, never on the console route — `activate()` returns `console_is_routed()` and reads false on a board whose desktop is fully up)",
        desktop_up,
        console_routed,
        if desktop_up { "OPEN" } else { "HELD" }
    );
    if desktop_up {
        screen_open_once();
    }
}

// =========================================================================================
// SERVICE + FIXTURE
// =========================================================================================

static SERVICED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Called from the storage-ready passes in `main.rs` (beside `holocron::service`, for the same
/// reason: which pass a build reaches depends on its knobs). Loads the store once the root volume
/// answers, then runs the M1 fixture exactly once. Costs one relaxed load per pass thereafter.
pub fn service() {
    use core::sync::atomic::Ordering;
    if SERVICED.load(Ordering::Relaxed) {
        return;
    }
    // STORAGE FIRST: the same predicate `holocron::load_once` waits on; until it answers, this pass costs
    // one load and prints nothing. (The VFS mount table is deliberately NOT consulted here at all —
    // LEDGER SO33: `bootdisk::bind` caches its first survey for the boot, and an early call would latch
    // an EMPTY root for every later caller, the shell's own verbs included.)
    if crate::drivers::block::info().is_none() {
        return;
    }
    // The block registry answers before the volume is quietly mountable: the moment a stick registers,
    // the driver loan is still held by the enumeration pump and `fat::mount()` answers `Busy` (measured on
    // QEMU virt, gate-4 capture: the first pass after `MISSION SUCCESS` refused). So the mount is RETRIED
    // across passes and only a BOUND of refusals is a verdict — the `HCRON_DEFER_STUCK` idiom, and the
    // last error is named so the line says which refusal it was.
    match try_load() {
        Ok(()) => {
            SERVICED.store(true, Ordering::Relaxed);
            #[cfg(feature = "loginst")]
            { login_fixture(); login_hard_fixture(); login_ident_fixture(); login_end_fixture(); } // SECLOGIN M1/M2/M3 — PWHARD's own leg, chained here because it needs `una` in the store and the session CLOSED (login_fixture leaves it closed). ONE braced block, because the `#[cfg]` above governs exactly one statement (x86-mix-2, the loginst-off leg, caught the unbraced form).
            #[cfg(all(feature = "loginst", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
            crate::video::strip::login_press_fixture(b"una", b"correct-horse"); // SO36/SO44 — the INPUT GATE. Here, BEFORE the screen fixture, because it needs three things this point in `service` guarantees: the panel real (the fixture mints a stand-in `wm` row to be the window behind), `una` already in the store (`login_fixture` above created it), and the screen DOWN — which it does not assume: it measures `screen_press` at its own point first and REDS if that reads true, so a boot that had the screen up here goes loud instead of quietly passing. It puts the boot back where it found it: row closed, screen down, no session.
            #[cfg(all(feature = "loginst", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
            crate::video::crystal::login::screen_fixture(b"una", b"correct-horse", b"wrong-horse", crate::video::crystal::logout_row_fire, screen_press_via_router, screen_press_route_name()); // LOGIN M4 — the Log Out route under test is the CRYSTAL MENU'S ROW (`crystal::logout_row_fire`), not the screen's own reopen; M3's `logout_direct` retires with it. LOGINFLOW M1 — and the PRESS route under test is the arch's LIVE ROUTER (`screen_press_via_router`, this file's tail), handed in for exactly the reason the logout route is: a fixture that called `press_swallow` itself would stay green on a tree whose router gate had been deleted — B121's lesson one band over. The seam's NAME travels with it, so the verdict line says which entry was driven rather than leaving the reader to infer it from the arch.
        }
        Err(e) => {
            let n = MOUNT_REFUSALS.fetch_add(1, Ordering::Relaxed) + 1;
            if n >= MOUNT_REFUSAL_BOUND {
                // Said ONCE, so silence is never mistaken for "never ran". TWO verdict forms, and the
                // difference is what the harness carries, not what the kernel did: `NotFat`/`NoDisk` after the
                // bound means NO FAT VOLUME IS PRESENT AT ALL — true of QEMU virt's aarch64 default `usb.img`
                // (a raw signed pattern image; `UNAOS_FATIMG=sf` gives it one; the x86 default carries a FAT32 volume since DEFAULTMEDIUM instead) — that is SKIPPED, a non-forbid, because a
                // leg red for a disk the harness never attached is wrong-strict. Any other refusal on a
                // registered disk (`Io`, `BadChain`, `Unsupported`, …) IS a defect and takes the FAIL form.
                SERVICED.store(true, Ordering::Relaxed);
                serial_println!("[users] el0-fat volume did not mount after {} passes — last={:?} — store unavailable this boot", n, e);
                #[cfg(feature = "loginst")]
                if matches!(e, FatError::NotFat | FatError::NoDisk) {
                    serial_println!(":: LOGIN: users+session -> SKIPPED — no FAT volume in this harness (last={:?}; UNAOS_FATIMG=sf attaches one) ::", e);
                } else {
                    serial_println!(":: LOGIN: users+session -> FAIL — el0-fat volume did not mount (last={:?}) ::", e);
                }
            }
        }
    }
}

/// Mount refusals seen by [`service`] since storage registered; the bound is the verdict.
static MOUNT_REFUSALS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
/// Passes of refusal before [`service`] gives up for the boot. Each pass is one main-loop iteration; on
/// QEMU virt the loan clears within a handful of passes, so 4096 is a wall, not a wait.
const MOUNT_REFUSAL_BOUND: u32 = 4096;

/// LOGIN fixture (`loginst` — a BOOT-TIME WRITE: user `una` with a KNOWN password) — the one QEMU-hosted proof the executor brief names for this milestone:
/// create a user, verify ok, wrong password refused, login opens the session under `user:<name>`,
/// logout closes it. Prints ONE verdict line; the `-> FAIL —` form is a `scan_serial_faults`
/// forbid, so a red run reds the leg with no spec needed. Idempotent across boots that share a
/// card: an existing `una` is re-used (`create=exists`), and the record's own hash is what verifies.
#[cfg(feature = "loginst")]
pub fn login_fixture() {
    const NAME: &[u8] = b"una";
    const PW: &[u8] = b"correct-horse";
    let vol = "el0-fat";
    let create = match create_user(NAME, PW) {
        Ok(_) => "ok",
        Err(UsersError::Exists) => "exists",
        Err(e) => {
            serial_println!(":: LOGIN: users+session -> FAIL — create_user reason={} volume={} ::", users_reason(e), vol);
            return;
        }
    };
    let verify_ok = verify(NAME, PW);
    let wrong_refused = !verify(NAME, b"wrong-horse");
    let wrong_login_refused = login(NAME, b"wrong-horse").is_err();
    let mut nb = [0u8; NAME_MAX];
    let none_before = whoami(&mut nb).is_none();
    let login_ok = login(NAME, PW).is_ok();
    let principal_ok = matches!(whoami(&mut nb), Some(n) if &nb[..n] == NAME);
    let home = match ensure_home(NAME) { Ok(v) => v, Err(_) => "FAIL" };
    let acl = home_acl_proof(NAME);
    // SO37 — runs HERE, with the session open, because the whole claim is about what a Log Out does to a
    // stamp taken before it. It leaves a NEW session open under the same name; the `logout()` below
    // closes it, so `none_after` reads exactly as it did before this leg existed.
    let epoch = epoch_proof(NAME);
    let _ = logout();
    let none_after = whoami(&mut nb).is_none();
    let all = verify_ok && wrong_refused && wrong_login_refused && none_before && login_ok && principal_ok && none_after && home != "FAIL" && acl != "FAIL" && epoch != "FAIL";
    if all {
        serial_println!(
            ":: LOGIN: users+session create={} verify=ok wrong=refused login=ok principal=user:una#{} linked={} home={} acl={} epoch={} logout=ok users={} volume={} -> PASS ::",
            create,
            id_of(NAME).unwrap_or(0),
            principal_linked(),
            home,
            acl,
            epoch,
            count(),
            vol
        );
    } else {
        serial_println!(
            ":: LOGIN: users+session -> FAIL — create={} verify={} wrong_refused={} wrong_login_refused={} none_before={} login={} principal={} none_after={} home={} acl={} epoch={} volume={} ::",
            create,
            verify_ok,
            wrong_refused,
            wrong_login_refused,
            none_before,
            login_ok,
            principal_ok,
            none_after,
            home,
            acl,
            epoch,
            vol
        );
    }
}

// =========================================================================================
// LOGINFLOW (M1) — THE TAIL APPEND, and the two reasons it is at the TAIL and knob-gated
// =========================================================================================
//
// `fs/mod.rs:106` declares this module UNCONDITIONALLY (`pub mod users;`), so unlike `video/login.rs`
// — which `video/crystal.rs`'s tail declares under `#[cfg(feature = "login")]` and which therefore
// does not exist knob-off at all — every line of this file is in a DEFAULT image. Two consequences,
// and both are rules rather than preferences (LAWS §5, rmbp-ledger PI5):
//
//  1. **TAIL, because a panic `Location` is a line number.** An insert anywhere above shifts the
//     `Location` of every panic site below it in this file, and `./arroyo knoboff login <baseline>`
//     compares the knob-OFF image BYTE FOR BYTE. Appending below the last item shifts nothing.
//  2. **`#[cfg(feature = "login")]`, because "unreferenced, so the linker drops it" is a claim about
//     an optimiser and not about the source.** Gated, the functions below are not COMPILED knob-off,
//     which is a fact the build reproduces on every host at every opt level.

/// LOGINFLOW M1 — **the store's own name-by-row accessor, so the login screen can SHOW who lives on
/// this machine.** The screen draws one row per user and a press on a row picks that name
/// (`video/login.rs`'s `Ctl::User`), which is the gesture that makes the name field optional for the
/// person who owns the machine — the Mac model, and the only reason a first-time user is not required
/// to remember a string they typed once.
///
/// Row order is the STORE's order (creation order, as `create_user` appends and `parse_image` reads
/// back), so the index on a `[login] press … control=user-row` line reads against `/USERS.DAT`
/// directly. Returns the length written into `out`, or `None` when `i` is past the end — never a
/// partially written buffer, and never the empty name a corrupt row would carry (`UserRec::read`
/// refuses those at parse time, so a row that is present is a row that is valid).
///
/// This is `home_of`'s shape with the lookup key inverted, and it takes the same one lock for the same
/// length of time. It is deliberately NOT a slice-returning accessor: `TABLE` is behind a `spin::Mutex`
/// and a borrow of a row could not outlive the guard.
#[cfg(feature = "login")]
pub fn name_at(i: usize, out: &mut [u8; NAME_MAX]) -> Option<usize> {
    let t = TABLE.lock();
    if i >= t.count as usize {
        return None;
    }
    let n = t.rows[i].name();
    if n.is_empty() {
        return None;
    }
    out[..n.len()].copy_from_slice(n);
    Some(n.len())
}

/// LOGINFLOW M1 fixture seam (`loginst`) — **route a press through the REAL router this board boots
/// with, so the fixture proves the PATH and not merely the predicate.**
///
/// `video/strip.rs::login_press_fixture` (SESSGATE) proves that `press_route` refuses to route a press
/// while the screen is up. That is the BARRIER, and it is one frame below the thing a person actually
/// touches: the arch router. B121's lesson is exactly this distance — `winmenu::selftest` was green on
/// every x86 boot for months while the metal press was inert, because the fixture called `press_at`
/// directly and the ROUTER had no arm to reach it. So the login flow's control legs are driven from
/// the top:
///
///  * **x86** takes `wc_click_route_at(Button(1), x, y)`, the coordinate-taking entry `MENUDROP`'s own
///    fixture drives, which is the LIVE router — the session gate at `arch/x86_64/syscall.rs:7452` is
///    the first statement of its press edge and `press_swallow` is reached THROUGH it.
///  * **aarch64** has no coordinate-taking router entry: `wc_click_route` reads the pointer from
///    `click_pointer_pos()`, which a fixture cannot set without moving a real pointer. It is therefore
///    driven at `strip::press_route` — the WHOLE of that router's furniture call
///    (`arch/aarch64/syscall.rs:14326` is `if strip::press_route(x, y) { … return true; }`), one call
///    below the top and above every window arm. The difference is REPORTED on the verdict line as
///    `via=`, never smoothed over: a leg that drove a different seam than it claims is the defect this
///    function exists to avoid.
///
/// Returns what the router returned: `true` = consumed, and no window arm ran.
#[cfg(all(feature = "loginst", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
pub fn screen_press_via_router(x: i32, y: i32) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        // PRESS then RELEASE, `menudrop_selftest`'s own shape: this router holds ONE outstanding press
        // (`CLICK_PRESS_TARGET`) and a fixture that left one armed would hand the next real gesture a
        // stale target. The verdict is the PRESS edge's — the release is consumed and discarded, which
        // is what `CLICK_TARGET_DROP` (stored by the session gate itself) already asks for.
        let hit = crate::arch::x86_64::syscall::wc_click_route_at(crate::pal::Event::Button(1), x, y);
        let _ = crate::arch::x86_64::syscall::wc_click_route_at(crate::pal::Event::Button(0), x, y);
        hit
    }
    #[cfg(target_arch = "aarch64")]
    {
        crate::video::strip::press_route(x, y)
    }
}

/// The seam's own name on the wire — see [`screen_press_via_router`] for why the two arches differ and
/// why the difference is printed rather than hidden.
#[cfg(all(feature = "loginst", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
pub fn screen_press_route_name() -> &'static str {
    #[cfg(target_arch = "x86_64")]
    {
        "wc_click_route_at"
    }
    #[cfg(target_arch = "aarch64")]
    {
        "strip::press_route"
    }
}

// =========================================================================================
// SECLOGIN M1 (PWHARD) — THE STRETCH: calibration, migration, delete, and the LOGIN-HARD fixture
// =========================================================================================
//
// Unconditional code in the default image (LAWS §5, 2026-09-22: security is not a knob; the proof is
// the replay and the fixture's go-red, never `knoboff`). Tail-appended so nothing above moves.

/// The PBKDF2 count for THIS boot, measured once: `PBKDF2_CAL_PROBE` iterations are timed with
/// `arch::ms()` and scaled to `PBKDF2_TARGET_MS`, then clamped to `[PBKDF2_ITERS_MIN, PBKDF2_ITERS_MAX]`.
/// A probe that reads 0 ms (a coarse clock, or a very fast box) is doubled until it reads at least
/// `PBKDF2_CAL_MIN_MS`, so the scale is never a division by a rounding error. Said once on the wire.
/// What the metal will read: the rMBP's Ivy Bridge runs this SHA-256 at roughly ten times QEMU TCG's
/// rate, so a QEMU boot that calibrates to N lands the rMBP near 10N for the same 250 ms — a row
/// created on one verifies at ITS count on the other (a QEMU-created row is ~25 ms on the rMBP; an
/// rMBP-created row is ~2.5 s on QEMU, which is why the lane creates its own).
fn calibrated_iters(t: &mut Table) -> u32 {
    if t.kdf_iters != 0 {
        return t.kdf_iters;
    }
    let mut probe = PBKDF2_CAL_PROBE;
    let mut ms;
    loop {
        let t0 = crate::arch::ms();
        let mut out = [0u8; HASH_LEN];
        crate::hash::pbkdf2_hmac_sha256(b"calibrate", &[0u8; SALT_LEN], probe, &mut out);
        ms = crate::arch::ms().saturating_sub(t0);
        if ms >= PBKDF2_CAL_MIN_MS || probe >= PBKDF2_ITERS_MAX / 2 {
            break;
        }
        probe *= 2;
    }
    let scaled = (probe as u64).saturating_mul(PBKDF2_TARGET_MS) / core::cmp::max(ms, 1);
    let iters = scaled.clamp(PBKDF2_ITERS_MIN as u64, PBKDF2_ITERS_MAX as u64) as u32;
    t.kdf_iters = iters;
    serial_println!("[users] kdf calibrated iters={} ms={} (probe={} took {} ms; target {} ms; floor {})", iters, PBKDF2_TARGET_MS, probe, ms, PBKDF2_TARGET_MS, PBKDF2_ITERS_MIN);
    iters
}
/// Iterations timed by the first calibration probe (doubled until the clock reads `PBKDF2_CAL_MIN_MS`).
const PBKDF2_CAL_PROBE: u32 = 2_000;
const PBKDF2_CAL_MIN_MS: u64 = 20;

/// Rows re-hashed v1 -> v2 on this boot, for the fixture's `migrated=` term.
static REHASHED: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// SECLOGIN M1: the migration. Called by `login` AFTER `verify` succeeded, so `password` is known
/// good for `name`. A legacy row is re-hashed at the calibrated count and the whole file goes to disk
/// as v2; a v2 row is left alone. A flush failure leaves the RAM row as it was (still legacy), so the
/// next successful login tries again — nothing is half-migrated.
fn rehash_if_legacy(name: &[u8], password: &[u8]) {
    let mut t = TABLE.lock();
    let Some(i) = (0..t.count as usize).find(|&i| t.rows[i].name() == name) else { return };
    if t.rows[i].kdf != KDF_LEGACY {
        return;
    }
    let iters = calibrated_iters(&mut t);
    if iters < PBKDF2_ITERS_MIN {
        serial_println!("[users] kdf REFUSED iters={} floor={} (a credential is never written below the floor)", iters, PBKDF2_ITERS_MIN);
        return;
    }
    let old = t.rows[i];
    let t0 = crate::arch::ms();
    let mut r = old;
    r.kdf = KDF_PBKDF2;
    r.iters = iters;
    r.hash = password_hash(&r, password);
    let ms = crate::arch::ms().saturating_sub(t0);
    t.rows[i] = r;
    match flush(&mut t) {
        Ok(()) => {
            REHASHED.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
            serial_println!("[users] rehash user={} v1->v2 iters={} ms={}", core::str::from_utf8(name).unwrap_or("?"), iters, ms);
        }
        Err(e) => {
            t.rows[i] = old;
            serial_println!("[users] rehash user={} v1->v2 NOT written reason={} (row stays legacy; retried at the next login)", core::str::from_utf8(name).unwrap_or("?"), users_reason(e));
        }
    }
}

/// SECLOGIN M2: delete a user. Refused for the open session's own user (`InUse`) and for an unknown
/// name (`Refused`). The row's SLOT is compacted away and reused by the next create; the row's UID is
/// never reissued (`next_uid` does not move), which is the whole of `multiuser.md` §2. Published
/// through the same swap; a failed publish restores the RAM table.
pub fn delete_user(name: &[u8]) -> Result<u32, UsersError> {
    let mut nb = [0u8; NAME_MAX];
    if matches!(whoami(&mut nb), Some(n) if &nb[..n] == name) {
        return Err(UsersError::InUse);
    }
    let mut t = TABLE.lock();
    if !t.loaded {
        return Err(UsersError::Volume);
    }
    let Some(i) = (0..t.count as usize).find(|&i| t.rows[i].name() == name) else {
        return Err(UsersError::Refused);
    };
    let saved = t.rows;
    let uid = t.rows[i].uid;
    let n = t.count as usize;
    for j in i..n - 1 {
        t.rows[j] = t.rows[j + 1];
    }
    t.rows[n - 1] = UserRec::EMPTY;
    t.count -= 1;
    match flush(&mut t) {
        Ok(()) => {
            serial_println!("[users] delete user={} uid={} (slot {} freed; uid never reissued, next_uid={})", core::str::from_utf8(name).unwrap_or("?"), uid, i, t.next_uid);
            Ok(uid)
        }
        Err(e) => {
            t.rows = saved;
            t.count += 1;
            Err(e)
        }
    }
}

/// The row's `(uid, kdf, iters)` for a fixture's own assertions; `None` for an unknown name.
#[cfg(feature = "loginst")]
pub fn row_facts(name: &[u8]) -> Option<(u32, u8, u32)> {
    let t = TABLE.lock();
    (0..t.count as usize).find(|&i| t.rows[i].name() == name).map(|i| (t.rows[i].uid, t.rows[i].kdf, t.rows[i].iters))
}

/// Fixture-only: create a row exactly as v1 wrote it (`KDF_LEGACY`, one SHA-256), so the migration
/// can be driven on a live store rather than argued from the parser. Same create path otherwise.
#[cfg(feature = "loginst")]
fn create_user_legacy(name: &[u8], password: &[u8]) -> Result<u32, UsersError> {
    if !name_ok(name) {
        return Err(UsersError::BadName);
    }
    let mut t = TABLE.lock();
    if !t.loaded {
        return Err(UsersError::Volume);
    }
    if (0..t.count as usize).any(|i| t.rows[i].name() == name) {
        return Err(UsersError::Exists);
    }
    if t.count as usize >= MAX_USERS || t.next_uid == u32::MAX {
        return Err(UsersError::Full);
    }
    let mut r = UserRec::EMPTY;
    r.name_len = name.len() as u8;
    r.name[..name.len()].copy_from_slice(name);
    let hl = HOME_ROOT.len() + 1 + name.len();
    r.home[..HOME_ROOT.len()].copy_from_slice(HOME_ROOT.as_bytes());
    r.home[HOME_ROOT.len()] = b'/';
    r.home[HOME_ROOT.len() + 1..hl].copy_from_slice(name);
    r.home_len = hl as u8;
    r.salt = new_salt(name, t.seq);
    r.uid = t.next_uid;
    r.kdf = KDF_LEGACY;
    r.iters = 1;
    r.hash = password_hash(&r, password);
    let i = t.count as usize;
    t.rows[i] = r;
    t.count += 1;
    t.next_uid += 1;
    match flush(&mut t) {
        Ok(()) => Ok(r.uid),
        Err(e) => {
            t.rows[i] = UserRec::EMPTY;
            t.count -= 1;
            t.next_uid -= 1;
            Err(e)
        }
    }
}

/// LOGIN-HARD (`loginst`) — PWHARD's proof on a live store: the known answers hold; a scratch user
/// written as a v1 row verifies through the legacy path; its next successful login MIGRATES it (the
/// `[users] rehash` line) and it verifies again as v2 at the calibrated count; a wrong password is
/// refused; the scratch user is deleted so the card is left as it was found. `ms=` is one v2 verify.
/// GO-RED: `calibrated_iters` mutated to answer 1 → `create_user` refuses `weak-kdf` and the wire
/// quotes `iters=1 floor=10000`; the LOGIN fixture above reds first (`create_user reason=weak-kdf`).
#[cfg(feature = "loginst")]
pub fn login_hard_fixture() {
    const NAME: &[u8] = b"hard1";
    const PW: &[u8] = b"stretch-me";
    let kat = crate::hash::pbkdf2_kat_ok();
    // a scratch row from a previous boot on a shared card is removed first, so the leg measures a
    // fresh v1 row every time rather than a v2 one it left behind
    let _ = delete_user(NAME);
    let legacy_created = create_user_legacy(NAME, PW).is_ok();
    let legacy_facts = row_facts(NAME);
    let legacy_verify = verify(NAME, PW);
    let wrong_refused = !verify(NAME, b"wrong-horse");
    let before = REHASHED.load(core::sync::atomic::Ordering::Relaxed);
    let login_ok = login(NAME, PW).is_ok();
    let _ = logout();
    let migrated = REHASHED.load(core::sync::atomic::Ordering::Relaxed) - before;
    let v2_facts = row_facts(NAME);
    let t0 = crate::arch::ms();
    let migrated_verify = verify(NAME, PW);
    let ms = crate::arch::ms().saturating_sub(t0);
    let wrong_after = !verify(NAME, b"wrong-horse");
    let unknown_refused = !verify(b"nobody", PW);
    let deleted = delete_user(NAME).is_ok();
    let iters = TABLE.lock().kdf_iters;
    let v2_rows = { let t = TABLE.lock(); (0..t.count as usize).filter(|&i| t.rows[i].kdf == KDF_PBKDF2).count() };
    let legacy_was_v1 = matches!(legacy_facts, Some((_, KDF_LEGACY, 1)));
    let now_v2 = matches!(v2_facts, Some((_, KDF_PBKDF2, n)) if n == iters && n >= PBKDF2_ITERS_MIN);
    let ok = kat && legacy_created && legacy_was_v1 && legacy_verify && wrong_refused && login_ok && migrated == 1 && now_v2 && migrated_verify && wrong_after && unknown_refused && deleted && iters >= PBKDF2_ITERS_MIN;
    if ok {
        serial_println!(":: LOGIN-HARD: kat=ok iters={} ms={} v2_rows={} migrated={} legacy_verify=ok migrated_verify=ok wrong=refused unknown=refused floor={} -> PASS ::", iters, ms, v2_rows, migrated, PBKDF2_ITERS_MIN);
    } else {
        serial_println!(":: LOGIN-HARD: -> FAIL — kat={} legacy_created={} legacy_was_v1={} legacy_verify={} wrong_refused={} login_ok={} migrated={} now_v2={} migrated_verify={} wrong_after={} unknown_refused={} deleted={} iters={} floor={} ::", kat, legacy_created, legacy_was_v1, legacy_verify, wrong_refused, login_ok, migrated, now_v2, migrated_verify, wrong_after, unknown_refused, deleted, iters, PBKDF2_ITERS_MIN);
    }
}

/// The row's storage slot (its index in the table) for a fixture's `slot_reused=` term.
#[cfg(feature = "loginst")]
fn slot_of(name: &[u8]) -> Option<usize> {
    let t = TABLE.lock();
    (0..t.count as usize).find(|&i| t.rows[i].name() == name)
}

/// LOGIN-IDENT (`loginst`) — SECLOGIN M2, THE IDENTITY RULE ON A LIVE STORE, both arches through one
/// call. Create A; delete A; create B (which lands in A's freed SLOT, measured, and gets a NEW uid);
/// create A again (a newer uid still). Then the arch half puts A's first uid on an owned row and asks
/// the ACL about B and about the second A: both refused, the owner admitted. GO-RED: `create_user`'s
/// `r.uid = t.next_uid` mutated to `i as u32 + 1` (v1's rule) → B is issued A's number and
/// `same_slot_refused=false -> FAIL` on BOTH arches from one mutation. Leaves the store as found.
#[cfg(feature = "loginst")]
pub fn login_ident_fixture() {
    const A: &[u8] = b"identa";
    const B: &[u8] = b"identb";
    const PW: &[u8] = b"ident-pw";
    let _ = delete_user(A);
    let _ = delete_user(B);
    let uid_a = create_user(A, PW).unwrap_or(0);
    let slot_a = slot_of(A);
    let deleted_a = delete_user(A).is_ok();
    let uid_b = create_user(B, PW).unwrap_or(0);
    let slot_b = slot_of(B);
    let uid_a2 = create_user(A, PW).unwrap_or(0);
    let slot_reused = slot_a.is_some() && slot_a == slot_b;
    let uids_distinct = uid_a != 0 && uid_b != 0 && uid_a2 != 0 && uid_a != uid_b && uid_b != uid_a2 && uid_a != uid_a2;
    let (owner_ok, same_slot_refused, same_name_refused) = ident_arch(uid_a, uid_b, uid_a2);
    let cleaned = delete_user(A).is_ok() && delete_user(B).is_ok();
    let ok = deleted_a && slot_reused && uids_distinct && owner_ok && same_slot_refused && same_name_refused && cleaned;
    if ok {
        serial_println!(":: LOGIN-IDENT: a_uid={} b_uid={} a2_uid={} slot_reused=true owner_ok=true same_slot_refused=true same_name_refused=true reason=recycled-id -> PASS ::", uid_a, uid_b, uid_a2);
    } else {
        serial_println!(":: LOGIN-IDENT: -> FAIL — a_uid={} b_uid={} a2_uid={} deleted_a={} slot_a={:?} slot_b={:?} slot_reused={} uids_distinct={} owner_ok={} same_slot_refused={} same_name_refused={} cleaned={} ::", uid_a, uid_b, uid_a2, deleted_a, slot_a, slot_b, slot_reused, uids_distinct, owner_ok, same_slot_refused, same_name_refused, cleaned);
    }
}

/// The arch half of [`login_ident_fixture`] where an ACL exists; `(true, true, true)` with a printed
/// `unlinked` where no EL0 regime is built (the same dispatch `home_acl_proof` uses).
#[cfg(feature = "loginst")]
fn ident_arch(uid_a: u32, uid_b: u32, uid_a2: u32) -> (bool, bool, bool) {
    #[cfg(all(target_arch = "aarch64", feature = "aarch64_el0"))]
    {
        return crate::arch::syscall::ident_fixture("HOME/IDENT.TXT", b"identa", uid_a, b"identb", uid_b, uid_a2);
    }
    #[cfg(target_arch = "x86_64")]
    {
        return crate::arch::syscall::ident_fixture(uid_a, uid_b, uid_a2);
    }
    #[cfg(not(any(target_arch = "x86_64", all(target_arch = "aarch64", feature = "aarch64_el0"))))]
    {
        let _ = (uid_a, uid_b, uid_a2);
        serial_println!("[users] ident: unlinked (no EL0 regime in this image)");
        (true, true, true)
    }
}

/// LOGIN-END (`loginst`) — SECLOGIN M3, on the x86 lane: open a session, launch `STAT.ELF` under it
/// through the desktop's own launcher, Log Out through the real `logout`, and prove the pid is gone
/// from the process table and the slot holds no window — `ended=1 windows>=1 pid_gone=true
/// window_gone=true`. A launch that could not happen is a SKIP with its reason, never a PASS.
/// GO-RED: `session_end_processes` mutated to skip the kill → `ended=0 pid_gone=false -> FAIL`.
#[cfg(feature = "loginst")]
pub fn login_end_fixture() {
    #[cfg(target_arch = "x86_64")]
    {
        if login(b"una", b"correct-horse").is_err() {
            serial_println!(":: LOGIN-END: -> FAIL — session did not open ::");
            return;
        }
        let (pid, wb, ended, windows, pid_gone, window_gone, why) = crate::arch::syscall::session_end_fixture();
        if pid == 0 {
            let _ = logout();
            serial_println!(":: LOGIN-END: launch -> SKIPPED reason={} (no program to end on this medium) ::", why);
            return;
        }
        let ok = ended == 1 && windows >= 1 && wb >= 1 && pid_gone && window_gone;
        serial_println!(":: LOGIN-END: pid={} stamp={} windows_before={} ended={} windows={} pid_gone={} window_gone={} -> {} ::", pid, why, wb, ended, windows, pid_gone, window_gone, if ok { "PASS" } else { "FAIL —" });
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        serial_println!("[users] session-end fixture: x86 lane only (the aarch64 walk is compiled by the arm-virt-el0 leg and runs on every Log Out)");
    }
}
