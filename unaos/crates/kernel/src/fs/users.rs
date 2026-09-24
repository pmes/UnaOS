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
/// LOGIN14 (R65): NO credential yet — the row exists (`adduser` made it, or the boot made root's) and the
/// password is chosen at the FIRST USE: root's on the set-password screen the boot-to-root opens, a user's on
/// the same screen at the first login. `verify` never answers `true` for such a row.
pub const KDF_UNSET: u8 = 0;
/// The root credential's row name (LOGIN14). Root is still reached by BOOTING (R63); the row holds the
/// password the set-password screen asked for, so a later arc can log root in through the screen (R64).
pub const ROOT_NAME: &[u8] = b"root";
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
            KDF_UNSET => {} // LOGIN14: a row whose password is not chosen yet
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
    // SECLOGIN M5: 32 bytes from the entropy source (`rand::fill`, which says its source on the wire
    // once per boot), folded with the cycle counter, the name and the store's seq — so even on the
    // jitter path two rows can never share a salt, and on a hardware path the salt is unpredictable.
    let mut r = [0u8; 32];
    let _ = crate::rand::fill(&mut r);
    let mut sh = crate::hash::Sha256::new();
    sh.update(&r);
    sh.update(&crate::arch::now_cycles().to_le_bytes());
    sh.update(name);
    sh.update(&seq.to_le_bytes());
    let d = sh.finalize();
    let mut s = [0u8; SALT_LEN];
    s.copy_from_slice(&d[..SALT_LEN]);
    s
}

/// SECLOGIN M5: the first uid a FRESH store issues — `FIRST_UID` plus a random 24-bit base, so a
/// rebuilt credential file (a reformat, a refused image) is improbably rather than certainly reissuing
/// a uid an old owner row on a unafs volume still names (`multiuser.md` §2).
fn fresh_uid_base() -> u32 {
    let mut r = [0u8; 32];
    let _ = crate::rand::fill(&mut r);
    FIRST_UID + (u32::from_le_bytes([r[0], r[1], r[2], 0]) & 0x00FF_FFFF)
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
            t.next_uid = fresh_uid_base();
            serial_println!("[users] load volume=el0-fat({}) src=none users=0 (fresh store) next_uid={}", veto, t.next_uid);
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
                t.next_uid = fresh_uid_base();
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

/// LOGIN14: how many rows are NOT root's. Root's Log Out is refused while this is 0 — a screen with only
/// `root` on it has nobody to log in as, because root is reached by booting (R63) until the screen can
/// log root in (R64, owed).
pub fn user_count() -> usize {
    let t = TABLE.lock();
    (0..t.count as usize).filter(|&i| t.rows[i].name() != ROOT_NAME).count()
}

/// LOGIN14: `Some(true)` for a row whose password is not chosen yet (`KDF_UNSET`), `Some(false)` for a
/// row with a credential, `None` for no such row. The screen asks this BEFORE `verify` so a first login
/// goes to the set-password form instead of a denial.
pub fn password_unset(name: &[u8]) -> Option<bool> {
    let t = TABLE.lock();
    (0..t.count as usize).find(|&i| t.rows[i].name() == name).map(|i| t.rows[i].kdf == KDF_UNSET)
}

/// LOGIN14: write `password` as `name`'s credential (PBKDF2 at the calibrated count, a fresh salt) and
/// publish. The store does not decide WHO may: the set-password screen calls it only for an unset row
/// (`set_first_password`), the `passwd` verb for the session's own row or, from root, any row.
pub fn set_password(name: &[u8], password: &[u8]) -> Result<(), UsersError> {
    if password.is_empty() {
        return Err(UsersError::Refused);
    }
    let mut t = TABLE.lock();
    if !t.loaded {
        return Err(UsersError::Volume);
    }
    let Some(i) = (0..t.count as usize).find(|&i| t.rows[i].name() == name) else {
        return Err(UsersError::Refused);
    };
    let iters = calibrated_iters(&mut t);
    if iters < PBKDF2_ITERS_MIN {
        serial_println!("[users] kdf REFUSED iters={} floor={} (a credential is never written below the floor)", iters, PBKDF2_ITERS_MIN);
        return Err(UsersError::WeakKdf);
    }
    let first = t.rows[i].kdf == KDF_UNSET;
    let saved = t.rows[i];
    let mut r = saved;
    r.salt = new_salt(name, t.seq);
    r.kdf = KDF_PBKDF2;
    r.iters = iters;
    r.hash = password_hash(&r, password);
    t.rows[i] = r;
    match flush(&mut t) {
        Ok(()) => {
            serial_println!("[users] password set user={} first={} (LOGIN14/R65: chosen at the keyboard, never on the line or the wire)", wire_name(name), first);
            Ok(())
        }
        Err(e) => {
            t.rows[i] = saved;
            Err(e)
        }
    }
}

/// LOGIN14: the set-password SCREEN's entry — refused unless the row's password is unset, so the screen
/// (which anyone at the glass can reach) can only ever CHOOSE a first password, never change one.
pub fn set_first_password(name: &[u8], password: &[u8]) -> Result<(), UsersError> {
    if password_unset(name) != Some(true) {
        return Err(UsersError::Refused);
    }
    set_password(name, password)
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
    create_row(name, Some(password))
}

/// LOGIN14 (R65): a row with NO credential — `kdf = KDF_UNSET`, the password chosen at the first use
/// (`set_first_password`). `adduser <name>` and the boot's root row are the two callers.
pub fn create_user_unset(name: &[u8]) -> Result<u32, UsersError> {
    create_row(name, None)
}

fn create_row(name: &[u8], password: Option<&[u8]>) -> Result<u32, UsersError> {
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
    let iters = if password.is_some() { calibrated_iters(&mut t) } else { 1 };
    if password.is_some() && iters < PBKDF2_ITERS_MIN {
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
    match password {
        Some(pw) => {
            r.kdf = KDF_PBKDF2;
            r.iters = iters;
            r.hash = password_hash(&r, pw);
        }
        None => {
            r.kdf = KDF_UNSET;
            r.iters = 1;
        }
    }
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
            Some(i) if t.rows[i].kdf != KDF_UNSET => (true, t.rows[i], iters),
            _ => (false, UserRec::EMPTY, iters), // absent, or LOGIN14 unset: the same clock, the same answer
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
        serial_println!("[users] home=/home/{} NOT created reason={} volume={}", core::str::from_utf8(name).unwrap_or("?"), users_reason(e), crate::fs::fat::mount().map(|f| f.volume_fingerprint().0).unwrap_or(0)); // SECLOGIN M6: the serial, 0 when the volume itself could not be mounted
    }
    {
        let mut s = SESSION_LOCAL.lock();
        s.0[..name.len()].copy_from_slice(name);
        s.1 = name.len() as u8;
        s.2 = id; ROOT_LIVE.store(false, core::sync::atomic::Ordering::Release); // R63 (LOGIN13): a user session SUPERSEDES the root session — root is never re-entered this boot (there is no root row in the store; root is reached by booting). See `root_session`.
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
    serial_println!("[users] home=/home/{} {} volume={:08x}", leaf, verdict, fs.volume_fingerprint().0); // SECLOGIN M6: the DEVICE (the FAT volume serial the formatter stamped), not the role — a flight capture tells the card from a stick without cross-reading the block registry
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
    let root = root_session(); if root { ROOT_LIVE.store(false, core::sync::atomic::Ordering::Release); } let (ended, windows) = arch_session_logout(root); // LOGIN13 M3 (R63): the ROOT session's Log Out ends ROOT's programs (uid 0, the root epoch) by the same walk, and root does not come back this boot.
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
    if root { serial_println!("[users] root session closed ended={} windows={} (R63: root's programs are ended; the next session is a user's, from the login screen)", ended, windows); } serial_println!("[users] logout epoch={} ended={} windows={} (SO37: stamps from the closed session are refused; M3: its programs are ended first)", arch_session_epoch(), ended, windows);
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
fn arch_session_logout(root: bool) -> (usize, usize) { let _ = root; // LOGIN13 M3: `root` selects the ROOT session's programs (uid 0 in the root epoch) on x86; aarch64 stamps no epoch on a non-user program (`session_restamp` is a no-op with no session), so its root arm is OWED and it ends what it always ended.
    #[cfg(any(target_arch = "x86_64", feature = "aarch64_el0"))]
    {
        #[cfg(target_arch = "x86_64")] return crate::arch::syscall::session_logout_as(root); #[cfg(not(target_arch = "x86_64"))] return crate::arch::syscall::session_logout();
    }
    #[cfg(not(any(target_arch = "x86_64", feature = "aarch64_el0")))]
    (0, 0)
}

/// SO37: the LIVE session epoch where the syscall layer exists (see [`arch_session_login`]); `0` on an
/// image with no EL0 regime, where no program can be launched and so no stamp can be stranded.
fn arch_session_epoch() -> u64 {
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
            // LOGIN13 M2 (R63): `login` no longer creates the first user on an empty store — `adduser` (root
            // only, password asked for) is the one way a user is made.
            match login(name.as_bytes(), pw.as_bytes()) {
                Ok(()) => console.println(&alloc::format!("logged in as {} (user:{})", name, name)),
                Err(_) => console.println("login: refused (-EACCES)"),
            }
        }
        "logout" => {
            // LOGIN13 M3 (R63): the ROOT session logs out too, and wherever the screen is built the screen
            // comes back — the SAME `reopen_after_logout` the crystal's Log Out row calls.
            let mut nb = [0u8; NAME_MAX];
            match log_out_to_screen(&mut nb) {
                Ok(n) => console.println(&alloc::format!("logged out {}", core::str::from_utf8(&nb[..n]).unwrap_or("?"))),
                Err("refused") => console.println("logout: refused — add a user first (`adduser <name>`): the login screen would have nobody to log in as"),
                Err(_) => console.println("logout: no session is open"),
            }
        }
        "adduser" => adduser_begin(args, console), // LOGIN13 M2 (R63) — root adds a user; see `adduser_begin`
        "passwd" => passwd_begin(args, console),   // LOGIN14 (R65) — the prompt: a password, twice, never echoed
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

/// LOGINORDER (rmbp-ledger B206; B189 finding 1) — the loginst chain is RUNNING: it opens the screen
/// (windowed and headless) and every press on the panel is the screen's while it does
/// (`press_swallow`), so a desktop press battery that overlaps it is swallowed and reads FAIL. Set for
/// the whole `Ok(())` arm of [`service`], cleared at its end.
#[cfg(feature = "loginst")]
static LOGINST_LIVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// LOGINORDER — `true` once the loginst chain has run to its end (or was never compiled). The x86
/// desktop press battery (`winx_launcher`'s click family) waits on this, bounded, before its first press.
/// Without `loginst` there is no chain and nothing to wait for. A store that never mounts leaves the
/// chain unrun and this `false` for the boot: the battery's bound, not this predicate, ends that wait.
pub fn loginst_settled() -> bool {
    #[cfg(feature = "loginst")]
    {
        return SERVICED.load(core::sync::atomic::Ordering::Acquire) && !LOGINST_LIVE.load(core::sync::atomic::Ordering::Acquire);
    }
    #[cfg(not(feature = "loginst"))]
    true
}

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
            #[cfg(feature = "loginst")]
            LOGINST_LIVE.store(true, Ordering::Release); // LOGINORDER (B206): the chain is live from here to the end of this arm — the desktop press battery waits for it
            SERVICED.store(true, Ordering::Relaxed);
            root_credential_ignition(); // LOGIN14 (R65): root's row, and the set-password screen if its password is not chosen yet
            #[cfg(feature = "loginst")]
            { login_rootpw_fixture(); login_bootroot_fixture(); login_adduser_fixture(); login_rootout_fixture(); login_fixture(); login_hard_fixture(); login_ident_fixture(); login_end_fixture(); login_kown_fixture(); login_rand_fixture(); } // SECLOGIN M1/M2/M3/M4/M5 — PWHARD's own leg, chained here because it needs `una` in the store and the session CLOSED (login_fixture leaves it closed). ONE braced block, because the `#[cfg]` above governs exactly one statement (x86-mix-2, the loginst-off leg, caught the unbraced form).
            #[cfg(all(feature = "loginst", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
            crate::video::strip::login_press_fixture(b"una", b"correct-horse"); // SO36/SO44 — the INPUT GATE. Here, BEFORE the screen fixture, because it needs three things this point in `service` guarantees: the panel real (the fixture mints a stand-in `wm` row to be the window behind), `una` already in the store (`login_fixture` above created it), and the screen DOWN — which it does not assume: it measures `screen_press` at its own point first and REDS if that reads true, so a boot that had the screen up here goes loud instead of quietly passing. It puts the boot back where it found it: row closed, screen down, no session.
            #[cfg(all(feature = "loginst", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
            crate::video::crystal::login::screen_fixture(b"una", b"correct-horse", b"wrong-horse", crate::video::crystal::logout_row_fire, screen_press_via_router, screen_press_route_name()); #[cfg(feature = "loginst")] LOGINST_LIVE.store(false, Ordering::Release); // LOGINORDER (B206): the chain is over; the press battery may run. ⚠ SAME-LINE fold. // LOGIN M4 — the Log Out route under test is the CRYSTAL MENU'S ROW (`crystal::logout_row_fire`), not the screen's own reopen; M3's `logout_direct` retires with it. LOGINFLOW M1 — and the PRESS route under test is the arch's LIVE ROUTER (`screen_press_via_router`, this file's tail), handed in for exactly the reason the logout route is: a fixture that called `press_swallow` itself would stay green on a tree whose router gate had been deleted — B121's lesson one band over. The seam's NAME travels with it, so the verdict line says which entry was driven rather than leaving the reader to infer it from the arch.
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
// This module is declared under `#[cfg(feature = "login")]` (fs/mod.rs:105 — B157's "declared
// UNCONDITIONALLY" was off by one line, measured by 31 red aarch64 legs when M4 first referenced it
// from the unconditional resolver), so nothing here is in an image without the knob; within the LOGIN
// line nothing is knob-gated further (LAWS §5, 2026-09-22: security is not a knob; the proof is the
// replay and the fixture's go-red, never `knoboff`). Tail-appended so nothing above moves.

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

/// SECLOGIN M4: is `leaf` one of the kernel-owned credential leaves (`USERS.DAT`, `USERS.NEW`)?
/// Case-insensitive, because FAT's own lookup is (`DirEntry::eq_name`), so no spelling slips past.
/// The users service never asks this — it reads through `locate_in_dir(0, leaf)`, not the EL0
/// resolver — which is what "kernel-owned" means: reachable by the kernel's own path, by no program's.
pub fn kernel_owned_leaf(leaf: &str) -> bool {
    leaf.eq_ignore_ascii_case(USERS_FILE) || leaf.eq_ignore_ascii_case(USERS_TMP_FILE)
}

/// LOGIN-KOWN (`loginst`) — SECLOGIN M4. The predicate is asserted (both leaves, both cases, a
/// near-miss refused), and then the REAL resolver is asked: on aarch64 through `open_locate` (the
/// guarded seam every `sys_open` walks) and the answer is a verdict; on x86 through
/// `fs::vfs::el0_locate` DIRECTLY, the shared resolver the storage task's `resolve_path` calls, whose
/// guard VFSOWNED landed (B181, `multiuser.md` §6) and LOGIN13 M4 typed (`El0LocateError::KernelOwned`,
/// `-EACCES` through the storage task) — so x86 now has a VERDICT too, `:: LOGIN-KOWN:`, which passes
/// only when BOTH leaves come back as that variant (a refusal for any other reason is not this one).
#[cfg(feature = "loginst")]
pub fn login_kown_fixture() {
    let pred = kernel_owned_leaf("USERS.DAT") && kernel_owned_leaf("users.dat") && kernel_owned_leaf("USERS.NEW") && kernel_owned_leaf("Users.New") && !kernel_owned_leaf("USERS.TXT") && !kernel_owned_leaf("HELLO.BIN");
    #[cfg(all(target_arch = "aarch64", feature = "aarch64_el0"))]
    {
        let e1 = crate::arch::syscall::kernel_owned_probe("USERS.DAT");
        let e2 = crate::arch::syscall::kernel_owned_probe("/USERS.NEW");
        let refused = e1 < 0 && e2 < 0;
        serial_println!(":: LOGIN-KOWN: pred={} resolver={} errno={},{} reason=kernel-owned -> {} ::", if pred { "ok" } else { "FAIL" }, if refused { "refused" } else { "OPENED" }, e1, e2, if pred && refused { "PASS" } else { "FAIL —" });
    }
    #[cfg(target_arch = "x86_64")]
    {
        let probe = |fs: &crate::fs::fat::FatFs, path: &str| -> &'static str {
            let mut c = false;
            match crate::fs::vfs::el0_locate(fs, path, false, &mut c) {
                Ok(_) => "OPENED",
                Err(crate::fs::vfs::El0LocateError::KernelOwned) => "KernelOwned",
                Err(_) => "OTHER",
            }
        };
        let (e1, e2) = match crate::fs::fat::mount() {
            Ok(fs) => (probe(&fs, "USERS.DAT"), probe(&fs, "/USERS.NEW")),
            Err(_) => ("no-volume", "no-volume"),
        };
        let resolver = if e1 == "OPENED" || e2 == "OPENED" { "OPENED" } else if e1 == "no-volume" { "no-volume" } else { "refused" };
        serial_println!("[users] kernel-owned pred={} resolver={} (x86: `fs::vfs::el0_locate` refuses the credential file — VFSOWNED's guard, typed `El0LocateError::KernelOwned` since LOGIN13 M4, `-EACCES` through the storage task)", if pred { "ok" } else { "FAIL" }, resolver);
        let typed = e1 == "KernelOwned" && e2 == "KernelOwned";
        serial_println!(":: LOGIN-KOWN: pred={} resolver={} err={},{} reason=kernel-owned -> {} ::", if pred { "ok" } else { "FAIL" }, resolver, e1, e2, if pred && typed { "PASS" } else { "FAIL —" });
    }
    #[cfg(not(any(target_arch = "x86_64", all(target_arch = "aarch64", feature = "aarch64_el0"))))]
    {
        serial_println!("[users] kernel-owned pred={} resolver=unlinked", if pred { "ok" } else { "FAIL" });
    }
}

/// LOGIN-RAND (`loginst`) — SECLOGIN M5. Two draws differ and neither is all-zero; the source is
/// named and matches what the probe said; two rows' salts differ (`una` and a scratch row). GO-RED:
/// `rand::jitter_fill` mutated to a constant → `distinct=false -> FAIL` (the QEMU lane runs
/// `-cpu qemu64,+x2apic`, no RDRAND, so the mutation bites there); the SOURCE FLIP is measured with
/// the builder's existing `UNAOS_CPU=qemu64,+x2apic,+rdrand` override — `source=rdrand`, no new knob.
#[cfg(feature = "loginst")]
pub fn login_rand_fixture() {
    let mut a = [0u8; 32];
    let mut b = [0u8; 32];
    let sa = crate::rand::fill(&mut a);
    let sb = crate::rand::fill(&mut b);
    let distinct = a != b;
    let nonzero = a.iter().any(|&x| x != 0) && b.iter().any(|&x| x != 0);
    let same_source = sa == sb && sa == crate::rand::source();
    const SCRATCH: &[u8] = b"randx";
    let _ = delete_user(SCRATCH);
    let salts_differ = create_user(SCRATCH, b"salt-pw").is_ok() && {
        let t = TABLE.lock();
        let una = (0..t.count as usize).find(|&i| t.rows[i].name() == b"una").map(|i| t.rows[i].salt);
        let sx = (0..t.count as usize).find(|&i| t.rows[i].name() == SCRATCH).map(|i| t.rows[i].salt);
        matches!((una, sx), (Some(u), Some(x)) if u != x)
    };
    let cleaned = delete_user(SCRATCH).is_ok();
    let ok = distinct && nonzero && same_source && salts_differ && cleaned;
    serial_println!(":: LOGIN-RAND: source={} distinct={} nonzero={} same_source={} salts_differ={} draws={} epoch_bits=64 -> {} ::", sa.name(), distinct, nonzero, same_source, salts_differ, crate::rand::draws(), if ok { "PASS" } else { "FAIL —" });
}

/// ARMUSERS (rmbp-ledger B188) — has [`service`] latched for this boot (the store loaded and, under
/// `loginst`, the fixture chain ran; or the bound of mount refusals was reached and said so)? The
/// aarch64 virt GICv3 path polls its own bounded storage pass until this answers `true`, because that
/// path diverges into the CAPSTONE terminus before any main loop exists to keep calling `service`
/// (`main.rs::virt_users_pass`). One relaxed load; tail append, so no `Location` above moves.
pub fn serviced() -> bool {
    SERVICED.load(core::sync::atomic::Ordering::Relaxed)
}

// =========================================================================================
// LOGIN13 (rmbp-ledger B189, R63) — THE ROOT SESSION, AND A BOOT THAT OPENS NO SCREEN
// =========================================================================================
//
// R63 (Peter, flight 12 on the glass): *"for boot 13 lets boot into root like we have been i will add
// my user and log out then log into the user account"*. Flight 12 booted a `login` image and the screen
// opened at 8.4 s as a WINDOW over the live desktop (`[login] screen open window=2 box=1330x764 at
// (775,345)`) and never had the keyboard. R63's shape: the machine boots to the ROOT desktop as every
// flight before it did, the person adds a user from that session (`adduser`), and Log Out closes the
// root session and puts the screen up over nothing.
//
// WHAT "ROOT" IS IN THIS CODE, named rather than invented. Before this arc the boot's session had no
// name: `SESSION_LOCAL.1 == 0` here, `SESSION_USER == 0` on x86 (`arch/x86_64/syscall.rs`, "0 = no
// session"), `PrincipalRecord::NONE` on aarch64 — the "anonymous / pre-login world" the SO37 comments
// describe. Every program launched in it is stamped uid 0, which the ACL treats as anonymous (no
// by-user admission: `owned_user_ok`'s `u != 0` guard). R63's "root" IS that state; this arc gives it a
// name and ONE bit of lifetime — [`ROOT_LIVE`] — and changes nothing about what uid 0 may open. It is
// not a row in the store and has no credential: root is reached by BOOTING, and once it is closed (its
// Log Out) or superseded (a user logged in from it) it does not come back until the next boot.

/// R63 — is the root session still the machine's session? True from boot; cleared by the root
/// session's Log Out ([`logout`]) and by a user login from it ([`login`]). Never set again this boot.
static ROOT_LIVE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// R63 — the caller is the ROOT SESSION: no user session is open AND the root session has not been
/// closed or superseded since boot. This is how `adduser` knows its caller is root: a shell verb is a
/// kernel HOST verb with no per-caller principal of its own (`shell.rs` dispatches it on the render
/// task), so "the caller" is whoever holds the machine's one session, and this is that session's name.
pub fn root_session() -> bool {
    ROOT_LIVE.load(core::sync::atomic::Ordering::Acquire) && SESSION_LOCAL.lock().1 == 0
}

/// Is the login screen up (the form is `Open`)? `false` where no screen is built (the crystal's gate:
/// x86 `wc`, aarch64 `desktop_firmware`) — the same dispatch [`screen_key`] uses.
pub fn screen_up() -> bool {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    {
        return crate::video::crystal::login::is_open();
    }
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    false
}

/// LOGIN13 M1 — **THE BOOT'S SCREEN DECISION, and under R63 the whole of it is "no".** This is the
/// statement `main.rs`'s two boot-time ignitions used to be (`if desktop { screen_open_once(); }`, the
/// x86 `x86_render_service` and the Pi `render_service`), moved here so a fixture can drive it with the
/// `desktop = true` a QEMU boot never presents (no Kepler takeover, so `desktop_owns_backdrop()` is
/// false on every `./arroyo test`). It opens nothing. GO-RED: put the old statement back in this body
/// (`if desktop { screen_open_once(); }`) and `:: LOGIN-BOOTROOT:` reads `desk_screen=open -> FAIL —`.
///
/// The SO43 seam ([`screen_open_at_ignition`]) is NOT this seam and is untouched: it is the Tegra desk
/// cascade's ignition (`main.rs`, `tegra_desk_cascade`) and the ARMUSERS virt pass's, and R63 is a
/// ruling about boot 13 on this bench — widening it to the Orin is a suggestion for Peter, not a
/// reading of his words.
pub fn boot_ignition(desktop: bool) {
    let _ = desktop;
}

/// LOGIN13 M1 — the boot's session, on the wire, from the two call sites that used to open the screen.
/// `desktop=` is the caller's `desktop_owns_backdrop()` (true on the metal after the Kepler takeover,
/// false on QEMU); `screen=` is READ BACK after [`boot_ignition`] ran, never assumed from it.
pub fn boot_session(desktop: bool) {
    boot_ignition(desktop);
    serial_println!(
        "[login] boot session=root desktop={} screen={} (R63: the machine boots to the root desktop; the login screen opens at the root session's Log Out, never at boot)",
        desktop,
        if screen_up() { "open" } else { "closed" }
    );
    DESKTOP_IGNITED.store(true, core::sync::atomic::Ordering::Release);
    if ROOT_PW_PENDING.swap(false, core::sync::atomic::Ordering::AcqRel) {
        serial_println!("[login] root password unset (store loaded before the desktop) -> set-password screen now (LOGIN14/R65)");
        screen_set_password(ROOT_NAME);
    }
}

// =========================================================================================
// LOGIN14 (R65) — ROOT'S PASSWORD IS CHOSEN AT THE FIRST BOOT-TO-ROOT, ON THE GLASS
// =========================================================================================
//
// Peter, 2026-09-24 (RULINGS R65): boot to root, get an alert to set root's password, add a personal
// account, log out, log in to it — and get the same alert for it. So the root credential is a ROW named
// `root` in the same store (`ROOT_NAME`), made at the first store load of a boot-to-root with NO
// credential (`KDF_UNSET`), and the set-password screen (`video/login.rs`, `open_set_password`) asks for
// the password twice and writes it through `set_first_password`. Root is still reached by BOOTING (R63):
// the row holds the credential a later arc will let the screen check (R64); the screen refuses a `root`
// login until then. Two orders are possible on the rMBP (the render service and the SD card race, §8.3
// line 1 vs 2): store first → the screen is deferred to `boot_session` through `ROOT_PW_PENDING`, so it
// is never opened headless under a desktop that is not up yet (a headless screen swallows every key and
// press with nothing on the glass); desktop first → it opens at the load.

/// The desktop ignition (`boot_session`) has run: `wm` can name a surface for the screen.
static DESKTOP_IGNITED: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// Root's password is unset and the desktop was not up when the store loaded: `boot_session` opens it.
static ROOT_PW_PENDING: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// At the store's load: make root's row if it is absent; open (or defer) the set-password screen if its
/// password is not chosen yet. Nothing happens in a user session (the store loaded after a login).
pub fn root_credential_ignition() {
    if !root_session() {
        return;
    }
    let row = if password_unset(ROOT_NAME).is_none() {
        match create_user_unset(ROOT_NAME) {
            Ok(_) => "created",
            Err(e) => {
                serial_println!("[login] root row NOT created reason={} (LOGIN14: no set-password screen; root has no credential this boot)", users_reason(e));
                return;
            }
        }
    } else {
        "present"
    };
    if password_unset(ROOT_NAME) != Some(true) {
        serial_println!("[login] root password set row={} (LOGIN14: nothing to ask)", row);
        return;
    }
    if DESKTOP_IGNITED.load(core::sync::atomic::Ordering::Acquire) {
        serial_println!("[login] root password unset row={} -> set-password screen (LOGIN14/R65: chosen at the keyboard, twice; never on the wire)", row);
        screen_set_password(ROOT_NAME);
    } else {
        ROOT_PW_PENDING.store(true, core::sync::atomic::Ordering::Release);
        serial_println!("[login] root password unset row={} -> set-password screen deferred to the desktop ignition (LOGIN14)", row);
    }
}

fn screen_set_password(name: &[u8]) {
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    crate::video::crystal::login::open_set_password(name, false);
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    {
        let _ = name;
        serial_println!("[login] set-password screen not built in this image (LOGIN14: the credential stays unset)");
    }
}

/// LOGIN14 fixture support: put `name`'s row back to UNSET (a previous run of this image set it).
#[cfg(feature = "loginst")]
pub fn fixture_unset_password(name: &[u8]) -> bool {
    let mut t = TABLE.lock();
    if !t.loaded {
        return false;
    }
    let Some(i) = (0..t.count as usize).find(|&i| t.rows[i].name() == name) else {
        return true; // absent: the ignition creates it unset
    };
    let saved = t.rows[i];
    t.rows[i].kdf = KDF_UNSET;
    t.rows[i].iters = 1;
    t.rows[i].hash = [0; HASH_LEN];
    match flush(&mut t) {
        Ok(()) => true,
        Err(_) => {
            t.rows[i] = saved;
            false
        }
    }
}

/// LOGIN14 fixture: the set-password screen for root, driven through the LIVE key router where there is
/// one (x86 `wc`: `wc_route_event`, the path the keyboard takes), else through `screen_key`. A mismatched
/// retype keeps the screen and the row unset; a matching pair sets it, `verify` agrees, the screen is
/// down, and the session is still root's. Runs FIRST in the loginst chain: every later login fixture
/// assumes the screen is down.
#[cfg(feature = "loginst")]
pub fn login_rootpw_fixture() {
    const PW: &[u8] = b"root13-pw";
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    {
        serial_println!(":: LOGIN-ROOTPW: -> SKIP — no screen built in this image (the x86 `wc` lane is where this claim is proven) ::");
        return;
    }
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    {
        use crate::video::crystal::login as screen;
        fn key(b: u8) -> bool {
            #[cfg(all(target_arch = "x86_64", feature = "wc"))]
            {
                return matches!(crate::arch::syscall::wc_route_event(crate::pal::Event::Key(b)), crate::pal::Event::Unknown);
            }
            #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
            screen_key(b)
        }
        let feed = |s: &[u8]| {
            let mut ok = true;
            for &b in s {
                ok &= key(b);
            }
            ok
        };
        let reset = fixture_unset_password(ROOT_NAME);
        root_credential_ignition();
        let deferred = ROOT_PW_PENDING.swap(false, core::sync::atomic::Ordering::AcqRel);
        if deferred {
            screen::open_set_password(ROOT_NAME, false); // the desktop ignition has not run in this lane yet: open it here (headless where `wm` has no surface)
        }
        let opened = screen_up() && screen::fixture_setpw_for(ROOT_NAME);
        let mut routed = feed(PW);
        routed &= key(b'\t');
        routed &= feed(b"other-pw");
        routed &= key(b'\n');
        let mismatch_kept = screen_up() && password_unset(ROOT_NAME) == Some(true) && screen::fixture_setpw_for(ROOT_NAME);
        routed &= feed(PW);
        routed &= key(b'\t');
        routed &= feed(PW);
        routed &= key(b'\n');
        let set = password_unset(ROOT_NAME) == Some(false);
        let verify_ok = verify(ROOT_NAME, PW);
        let wrong_refused = !verify(ROOT_NAME, b"other-pw");
        let closed = !screen_up();
        let root_after = root_session();
        let ok = reset && opened && routed && mismatch_kept && set && verify_ok && wrong_refused && closed && root_after;
        serial_println!(
            ":: LOGIN-ROOTPW: reset={} deferred={} opened={} keys_routed={} mismatch_kept={} set={} verify={} wrong={} screen={} root_after={} -> {} ::",
            reset, deferred, opened, routed, mismatch_kept, set, if verify_ok { "ok" } else { "FAIL" }, if wrong_refused { "refused" } else { "ACCEPTED" },
            if closed { "closed" } else { "OPEN" }, root_after, if ok { "PASS" } else { "FAIL —" }
        );
    }
}

/// LOGIN-BOOTROOT (`loginst`, LOGIN13 M1) — the boot opens no screen and the session is root. Runs FIRST
/// in the `loginst` chain, before any login, because its first question is what the boot left behind:
///  * `root_at_boot` — [`root_session`] at the head of the battery: no user session, root not closed.
///  * `desk_screen` / `nodesk_screen` — [`boot_ignition`] driven with BOTH values of `desktop`; the screen
///    must be down after each. `desktop=true` is the metal's arm (flight 12's), which QEMU cannot reach
///    through the boot itself.
///  * `still_root` — the ignition left the session alone.
/// The boot's OWN line (`[login] boot session=root … screen=closed`) is printed by `main.rs`, never
/// here, so a spec pin on it cannot be satisfied by this fixture (SPECPINS2's lesson, B184).
#[cfg(feature = "loginst")]
pub fn login_bootroot_fixture() {
    let root_at_boot = root_session();
    let before = screen_up();
    boot_ignition(true);
    let desk_open = screen_up();
    boot_ignition(false);
    let nodesk_open = screen_up();
    let still_root = root_session();
    let ok = root_at_boot && !before && !desk_open && !nodesk_open && still_root;
    serial_println!(
        ":: LOGIN-BOOTROOT: session=root(uid0) root_at_boot={} desk_screen={} nodesk_screen={} still_root={} screen_built={} -> {} ::",
        root_at_boot,
        if desk_open { "open" } else { "closed" },
        if nodesk_open { "open" } else { "closed" },
        still_root,
        cfg!(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))),
        if ok { "PASS" } else { "FAIL —" }
    );
}

// =========================================================================================
// LOGIN13 M2 (rmbp-ledger B189, R63) — `adduser <name>`: ROOT ADDS A USER, AND THE PASSWORD IS ASKED FOR
// =========================================================================================
//
// R63: *"what about adduser since it's logging me in as root"*. The verb is `adduser` (R26: the standard
// name), a HOST verb beside `login`/`logout` (`shell.rs`'s arm, `midden_core::HOST_VERBS`), root-only.
//
// HOW THE PASSWORD ARRIVES, and why it is not the way `login` takes it. `login <name> <password>` reads
// the password off the TYPED LINE (this file's `shell_verb`), and that line is recorded in three places
// before any verb runs: `shell::history_record` (the `history` verb reads it back), the
// `:: [midden] cmd="…" -> Host verb=…` witness ON THE WIRE (`shell.rs`, every dispatched line), and the
// console's own scrollback. A password typed there is on the serial capture a flight is scored from. So
// `adduser` takes the NAME on the line and ASKS for the password: [`prompt_key`] is offered every key
// ahead of the shell's line editor (`main.rs::handle_key`, a line-neutral fold), holds the bytes in
// kernel RAM, echoes NOTHING (no glyph, no `current_input`, no history, no wire), asks twice, and hands
// the result to the same [`create_user`] the store has always used — the hashing path is unchanged. A
// password on the line (`adduser <name> <pw>`) is REFUSED rather than used, because by the time the
// verb sees it the line has already been recorded; the refusal says so.
//
// WHILE THE PROMPT IS LIVE the two raw-key instruments that print typed bytes are told to withhold them
// ([`secret_input`]): `USB-DEBUG: KEY` (`main.rs::usbdebug_event_print`, the `usbdebug` knob, which the
// flight images carry) and the bounded `[serialdoor] key=` witness (`arch/x86_64/syscall.rs`, `ftdirx`).
// The same predicate covers the login screen (LOGIN13 M3), which the module doc of `video/login.rs`
// promised since LOGIN M3 ("NO TYPED BYTE REACHES THE WIRE") and which `usbdebug` quietly broke.

/// The longest password the prompt holds. `login`'s screen caps a field at 32 (`video/login.rs`
/// `FIELD_MAX`); the prompt allows more, and a longer password is refused rather than truncated so
/// what verifies is exactly what was typed.
const PW_MAX: usize = 64;

/// The live `adduser` prompt: the name being added, which entry is being typed (1 = first, 2 = the
/// retype), and the two entries. Zeroed on every exit path.
struct Prompt {
    live: bool,
    stage: u8,
    name: [u8; NAME_MAX],
    nlen: usize,
    a: [u8; PW_MAX],
    alen: usize,
    b: [u8; PW_MAX],
    blen: usize,
    over: bool,
}

/// The idle prompt — every exit path writes this back, which is what zeroes both entries.
const PROMPT_IDLE: Prompt = Prompt { live: false, stage: 0, name: [0; NAME_MAX], nlen: 0, a: [0; PW_MAX], alen: 0, b: [0; PW_MAX], blen: 0, over: false };

static PROMPT: Mutex<Prompt> = Mutex::new(PROMPT_IDLE);

/// The last `adduser` outcome — `"created"` or the refusal's `reason=` word. The fixture reads it; the
/// wire carries the same word, so a reader of either sees one vocabulary.
static ADDUSER_LAST: Mutex<&'static str> = Mutex::new("");

/// Is an `adduser` password prompt waiting for keys?
pub fn prompt_live() -> bool {
    PROMPT.lock().live
}

/// LOGIN13 — are the keys being typed right now SECRET? True while the `adduser` prompt is live or the
/// login screen is up. The raw-key witnesses consult this and print a placeholder instead of the byte.
pub fn secret_input() -> bool {
    prompt_live() || screen_up()
}

/// The name as the wire prints it: the bytes when they are printable ASCII, `?` otherwise. A name that
/// fails `name_ok` is still NAMED on its refusal line — root typed it — but never raw.
fn wire_name(n: &[u8]) -> &str {
    if !n.is_empty() && n.iter().all(|&b| (0x21..0x7f).contains(&b)) {
        core::str::from_utf8(n).unwrap_or("?")
    } else {
        "?"
    }
}

fn adduser_refuse(name: &[u8], reason: &'static str, console: &mut crate::console::Console) {
    *ADDUSER_LAST.lock() = reason;
    serial_println!("[users] adduser REFUSED user={} reason={}", wire_name(name), reason);
    console.println(&alloc::format!("adduser: {} not added (reason={})", wire_name(name), reason));
}

/// The `UsersError` a create returned, in `adduser`'s refusal vocabulary.
fn adduser_reason(e: UsersError) -> &'static str {
    match e {
        UsersError::Exists => "exists",
        UsersError::BadName => "bad-name",
        UsersError::Full => "full",
        UsersError::WeakKdf => "weak-kdf",
        UsersError::Volume => "volume",
        other => users_reason(other),
    }
}

/// `adduser <name>` — the checks that need no password, then the prompt. Every refusal here happens
/// BEFORE a password is asked for, so nobody types a secret into a request that was never going to run.
fn adduser_begin(args: &[&str], console: &mut crate::console::Console) {
    let Some(name) = args.first() else {
        return console.println("usage: adduser <name>   (root only; the password is asked for and never shown)");
    };
    let nb = name.as_bytes();
    if args.len() > 1 {
        // The line — password included — is already in `history` and on the wire (`[midden] cmd=`). Using
        // it would make that the normal way to add a user; refusing it makes the prompt the only way.
        return adduser_refuse(nb, "password-on-line", console);
    }
    if !root_session() {
        return adduser_refuse(nb, "not-root", console);
    }
    if !load_once() {
        return adduser_refuse(nb, "storage-not-up", console);
    }
    if !name_ok(nb) {
        return adduser_refuse(nb, "bad-name", console);
    }
    if id_of(nb).is_some() {
        return adduser_refuse(nb, "exists", console);
    }
    if count() >= MAX_USERS {
        return adduser_refuse(nb, "full", console);
    }
    // LOGIN14 (R65): the row is made with NO password; the person chooses it at their first login, on
    // the set-password screen. `passwd <name>` (root) sets one from the shell if that is wanted first.
    match adduser_commit(nb) {
        Ok((uid, created)) => console.println(&alloc::format!("adduser: added {} (uid {}, home /home/{}{}); the password is chosen at the first login", wire_name(nb), uid, wire_name(nb), if created { ", created" } else { "" })),
        Err(r) => adduser_refuse(nb, r, console),
    }
}

fn passwd_refuse(name: &[u8], reason: &'static str, console: &mut crate::console::Console) {
    *ADDUSER_LAST.lock() = reason;
    serial_println!("[users] passwd REFUSED user={} reason={}", wire_name(name), reason);
    console.println(&alloc::format!("passwd: {} not changed (reason={})", wire_name(name), reason));
}

/// LOGIN14: `passwd` (the session's own password; root's in the root session) or `passwd <name>` (root
/// only). The password is asked for twice by the prompt `main.rs::handle_key` offers every key to first
/// — nothing echoed, nothing in `history`, nothing on the wire. `passwd <name> <pw>` is refused.
fn passwd_begin(args: &[&str], console: &mut crate::console::Console) {
    let mut own = [0u8; NAME_MAX];
    let target: &[u8] = match args.first() {
        Some(n) => n.as_bytes(),
        None => {
            if root_session() {
                ROOT_NAME
            } else {
                match whoami(&mut own) {
                    Some(n) => &own[..n],
                    None => return console.println("passwd: no session is open"),
                }
            }
        }
    };
    if args.len() > 1 {
        return passwd_refuse(target, "password-on-line", console);
    }
    let mut mine = [0u8; NAME_MAX];
    let is_own = matches!(whoami(&mut mine), Some(n) if &mine[..n] == target) || (root_session() && target == ROOT_NAME);
    if !is_own && !root_session() {
        return passwd_refuse(target, "not-root", console);
    }
    if !load_once() {
        return passwd_refuse(target, "storage-not-up", console);
    }
    if password_unset(target).is_none() {
        return passwd_refuse(target, "no-such-user", console);
    }
    {
        let mut p = PROMPT.lock();
        p.live = true;
        p.stage = 1;
        p.name = [0; NAME_MAX];
        p.name[..target.len()].copy_from_slice(target);
        p.nlen = target.len();
        p.a = [0; PW_MAX];
        p.alen = 0;
        p.b = [0; PW_MAX];
        p.blen = 0;
        p.over = false;
    }
    console.println(&alloc::format!("passwd: new password for {} (not shown; Enter ends it, Ctrl-C cancels):", wire_name(target)));
}

/// LOGIN13 M2 — offer a key to the `adduser` prompt. Called FIRST in `main.rs::handle_key`, ahead of the
/// shell's line editor. `0` = no prompt is live (the key is the shell's); `1` = consumed, nothing to
/// redraw (a password byte — nothing is echoed, so there is nothing to draw); `2` = consumed and the
/// console printed a line (the caller redraws it).
pub fn prompt_key(c: u8, console: &mut crate::console::Console) -> u8 {
    let mut p = PROMPT.lock();
    if !p.live {
        return 0;
    }
    match c {
        0x03 => {
            let n = p.name;
            let nl = p.nlen;
            *p = PROMPT_IDLE;
            drop(p);
            passwd_refuse(&n[..nl], "cancelled", console);
            2
        }
        8 | 0x7f => {
            if p.stage == 1 {
                p.alen = p.alen.saturating_sub(1);
            } else {
                p.blen = p.blen.saturating_sub(1);
            }
            1
        }
        b'\n' | b'\r' => {
            if p.stage == 1 && p.alen > 0 && !p.over {
                p.stage = 2;
                drop(p);
                console.println("passwd: retype it:");
                return 2;
            }
            // Finished (or refused at the first Enter): copy out, ZERO the prompt, then decide with no
            // lock held — `create_user` runs the calibrated KDF (~250 ms) and takes `TABLE`.
            let (n, nl, a, al, b, bl, over, stage) = (p.name, p.nlen, p.a, p.alen, p.b, p.blen, p.over, p.stage);
            *p = PROMPT_IDLE;
            drop(p);
            let name = &n[..nl];
            let verdict = if over {
                Err("password-too-long")
            } else if al == 0 {
                Err("empty-password")
            } else if stage != 2 || a[..al] != b[..bl] {
                Err("mismatch")
            } else {
                passwd_commit(name, &a[..al])
            };
            let mut a = a;
            let mut b = b;
            for x in a.iter_mut().chain(b.iter_mut()) {
                *x = 0;
            }
            match verdict {
                Ok(()) => console.println(&alloc::format!("passwd: password set for {}", wire_name(name))),
                Err(r) => passwd_refuse(name, r, console),
            }
            2
        }
        0x20..=0x7e => {
            if p.stage == 1 {
                if p.alen < PW_MAX { let i = p.alen; p.a[i] = c; p.alen += 1; } else { p.over = true; }
            } else if p.blen < PW_MAX {
                let i = p.blen;
                p.b[i] = c;
                p.blen += 1;
            } else {
                p.over = true;
            }
            1
        }
        _ => 1, // swallowed: nothing typed at a password prompt reaches the shell
    }
}

/// LOGIN13 M2 — create the user root asked for and make its home. Root is RE-CHECKED here, at the
/// moment of the write: the prompt may have been open across a Log Out. `Ok((uid, home_created))`;
/// the wire line is `[users] adduser user=<n> id=<uid> home=/home/<n> created=<bool>`, where `created=`
/// is the HOME's verdict (`ensure_home`, which also prints its own `[users] home=` line), `false` when
/// the directory was already there or the volume refused it (then the `NOT created reason=` line says
/// why — the user exists either way and `/home/<n>` is made again at its first login, `login`'s rule).
/// LOGIN14: the prompt's commit — the caller was checked at `passwd_begin`; checked again here at the
/// write (root, or the session's own row).
fn passwd_commit(name: &[u8], password: &[u8]) -> Result<(), &'static str> {
    let mut mine = [0u8; NAME_MAX];
    let is_own = matches!(whoami(&mut mine), Some(n) if &mine[..n] == name) || (root_session() && name == ROOT_NAME);
    if !is_own && !root_session() {
        return Err("not-root");
    }
    if password.is_empty() {
        return Err("empty-password");
    }
    set_password(name, password).map_err(adduser_reason)?;
    *ADDUSER_LAST.lock() = "set";
    Ok(())
}

/// LOGIN14: root adds a user with NO credential; `/home/<name>` is made at once.
pub fn adduser_commit(name: &[u8]) -> Result<(u32, bool), &'static str> {
    if !root_session() {
        return Err("not-root");
    }
    let uid = create_user_unset(name).map_err(adduser_reason)?;
    let created = match ensure_home(name) {
        Ok(v) => v == "created",
        Err(e) => {
            serial_println!("[users] home=/home/{} NOT created reason={} volume={}", wire_name(name), users_reason(e), crate::fs::fat::mount().map(|f| f.volume_fingerprint().0).unwrap_or(0));
            false
        }
    };
    *ADDUSER_LAST.lock() = "created";
    serial_println!("[users] adduser user={} id={} home=/home/{} created={} password=unset (R63/R65: root added it; the password is chosen at the first login, never on the line)", wire_name(name), uid, wire_name(name), created);
    Ok((uid, created))
}

/// LOGIN-ADDUSER (`loginst`, LOGIN13 M2) — x86 lane. Drives the REAL verb (`shell_verb("adduser", …)`)
/// and the REAL prompt (`prompt_key`, the function `handle_key` offers every key to) on a scratch console,
/// in the root session the boot left (LOGIN-BOOTROOT ran first):
///  * `prompted` — the verb opened the prompt instead of creating anything;
///  * `echo=none` — after typing the password twice, the console's input line is EMPTY: no byte reached
///    the line editor (and so neither history nor the `[midden] cmd=` witness);
///  * `created` / `verify` — the user exists and the TYPED credential verifies (the prompt handed the
///    bytes it was given to `create_user`, not a truncation or the retype's leftovers);
///  * the four refusals, each by its wire word: `dup=exists`, `empty=empty-password`, `mismatch=mismatch`,
///    `on_line=password-on-line` — and none of them created a row.
/// Leaves `boot13` in the store: LOGIN-ROOTOUT (M3) logs in as it after the root session's Log Out.
/// GO-RED (LOGIN13 M2, run on this gate): the root check in `adduser_begin` inverted → the verb refuses
/// `reason=not-root` from root → `prompted=false created=false -> FAIL —`.
#[cfg(feature = "loginst")]
pub fn login_adduser_fixture() {
    #[cfg(target_arch = "x86_64")]
    {
        const N: &str = "boot13";
        const PW: &[u8] = b"boot13-pw";
        let mut con = crate::console::Console::new();
        let feed = |con: &mut crate::console::Console, s: &[u8]| {
            for &b in s {
                let _ = prompt_key(b, con);
            }
        };
        let root = root_session();
        // LOGIN14: `adduser` makes the row with NO password and asks for nothing
        shell_verb("adduser", &[N], &mut con);
        let prompted_at_adduser = prompt_live();
        let created = *ADDUSER_LAST.lock() == "created" && password_unset(N.as_bytes()) == Some(true);
        let uid = id_of(N.as_bytes()).unwrap_or(0);
        let unset_refused = !verify(N.as_bytes(), PW) && !verify(N.as_bytes(), b"");
        // `passwd <name>` from root: the prompt, twice, nothing echoed
        shell_verb("passwd", &[N], &mut con);
        let prompted = prompt_live();
        feed(&mut con, PW);
        feed(&mut con, b"\n");
        feed(&mut con, PW);
        feed(&mut con, b"\n");
        let echo_none = con.current_input.is_empty() && !prompt_live();
        let set = *ADDUSER_LAST.lock() == "set" && password_unset(N.as_bytes()) == Some(false);
        let verify_ok = verify(N.as_bytes(), PW);
        shell_verb("adduser", &[N], &mut con);
        let dup = if prompt_live() { "PROMPTED" } else { *ADDUSER_LAST.lock() };
        shell_verb("passwd", &[N], &mut con);
        feed(&mut con, b"\n");
        let empty = if !verify(N.as_bytes(), PW) { "CHANGED" } else { *ADDUSER_LAST.lock() };
        shell_verb("passwd", &[N], &mut con);
        feed(&mut con, b"one-pw\n");
        feed(&mut con, b"two-pw\n");
        let mismatch = if !verify(N.as_bytes(), PW) { "CHANGED" } else { *ADDUSER_LAST.lock() };
        shell_verb("adduser", &["boot13p", "on-the-line"], &mut con);
        let on_line = if prompt_live() || id_of(b"boot13p").is_some() { "ACCEPTED" } else { *ADDUSER_LAST.lock() };
        shell_verb("passwd", &[N, "on-the-line"], &mut con);
        let pw_on_line = if prompt_live() || !verify(N.as_bytes(), PW) { "ACCEPTED" } else { *ADDUSER_LAST.lock() };
        let ok = root && !prompted_at_adduser && created && unset_refused && prompted && echo_none && set && verify_ok && dup == "exists" && empty == "empty-password" && mismatch == "mismatch" && on_line == "password-on-line" && pw_on_line == "password-on-line";
        serial_println!(
            ":: LOGIN-ADDUSER: root={} created=unset:{} prompted_at_adduser={} unset_verify={} passwd_prompted={} echo={} uid={} set={} verify={} dup={} empty={} mismatch={} on_line={} passwd_on_line={} -> {} ::",
            root, created, prompted_at_adduser, if unset_refused { "refused" } else { "ACCEPTED" }, prompted, if echo_none { "none" } else { "LEAKED" }, uid, set, if verify_ok { "ok" } else { "FAIL" }, dup, empty, mismatch, on_line, pw_on_line,
            if ok { "PASS" } else { "FAIL —" }
        );
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        serial_println!("[users] adduser fixture: x86 lane only (the verb and the prompt are arch-neutral and compiled here; the leg runs on the x86 login lane)");
    }
}

// =========================================================================================
// LOGIN13 M3 (rmbp-ledger B189, R63) — LOG OUT CLOSES THE ROOT SESSION AND THE SCREEN TAKES THE INPUT
// =========================================================================================
//
// R63: *"i will add my user and log out then log into the user account"*. Two routes reach the same
// action — the shell's `logout` ([`log_out_to_screen`]) and the crystal's Log Out row
// (`video/crystal.rs` `Verb::LogOut` -> `login::reopen_after_logout`) — and both now:
//   1. refuse the ROOT session's Log Out while the store has nobody to log in as ([`root_logout_refused`]);
//   2. close the session through [`logout`], which for ROOT ends root's programs by the same walk
//      SECLOGIN M3 built for a user's (x86: `session_logout_as(true)` selects the running rows stamped
//      uid 0 in the root epoch — `arch/x86_64/syscall.rs`, `session_end_processes`), so the screen
//      comes up over nothing a program drew;
//   3. put the screen up, and the screen is the FIRST taker of every key on the x86 router
//      (`wc_route_event`, ahead of the Esc/Quarry doors, the Tab focus ring and the focused ring —
//      flight 12's keys went to `[wc-c] focus tab-cycle` and the desktop), as it already was of every
//      press (`wc_click_route_at`, SO44).

/// R63 — refuse the ROOT session's Log Out when it would strand the person at a screen with nobody to
/// log in as. Prints the refusal and answers `true`; `false` (and silent) for every other session.
pub fn root_logout_refused() -> bool {
    if !root_session() {
        return false;
    }
    let reason = if !load_once() {
        "storage-not-up"
    } else if user_count() == 0 {
        "no-users"
    } else {
        return false;
    };
    serial_println!("[users] logout REFUSED session=root reason={} (R63: `adduser <name>` first — root's Log Out would leave a login screen with nobody to log in as)", reason);
    true
}

/// LOGIN13 M3 — the shell's `logout`: close the open session (the root session included) and, where
/// the screen is built, put it up — the SAME `reopen_after_logout` the crystal's Log Out row calls, so
/// the two routes cannot drift. Where no screen is built (a headless image, the aarch64 `virt` lane) it
/// is the plain [`logout`]. `Ok(name)` names the session that closed (`root` for the root session).
pub fn log_out_to_screen(out: &mut [u8; NAME_MAX]) -> Result<usize, &'static str> {
    let root = root_session();
    let n = if root {
        out[..4].copy_from_slice(b"root");
        4
    } else {
        match whoami(out) {
            Some(n) => n,
            None => return Err("no-session"),
        }
    };
    if root_logout_refused() {
        return Err("refused");
    }
    #[cfg(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware")))]
    crate::video::crystal::login::reopen_after_logout();
    #[cfg(not(any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
    let _ = logout();
    Ok(n)
}

/// LOGIN-ROOTOUT (`loginst`, LOGIN13 M3) — x86 `wc` lane: the root session's Log Out, end to end, driven
/// through the entries a person uses. Runs after LOGIN-ADDUSER (which left `boot13` in the store) and
/// before every other `loginst` leg:
///  * `empty_refused` — with the store emptied of `boot13` for one call, the shell's `logout` from root is
///    REFUSED `reason=no-users` and root stays live; `boot13` is then put back through `create_user` (LOGIN14: with its password);
///  * `pid` / `root_stamped` — `STAT.ELF` launched IN THE ROOT SESSION (uid 0, the root epoch — the
///    desktop's own launcher, `spawn_user_image_bg`); `others=` is every OTHER running root-session
///    program the Log Out is about to end (0 on this lane: nothing else runs at the storage pass);
///  * `root_after=false ended>=1 pid_gone window_gone` — the shell's `logout` closed root and ENDED its
///    program; `screen=up screen_window=true` — the screen came up on a real `wm` row;
///  * `not_root=not-root` — `adduser` from the screen's side of the Log Out is refused;
///  * `keys_routed=true` — EVERY key below went through the LIVE x86 key router (`wc_route_event`) and was
///    consumed there (`Event::Unknown`), never handed on; `name_typed` / `tab=password` — the bytes landed
///    in the form's name field and Tab moved the FORM's focus (not `[wc-c] focus tab-cycle`);
///    `wrong=denied` — a wrong password through the router is the one-answer denial and the screen stays;
///    `login=boot13` — the right one opens the session as the new user (its `[users] home=/home/boot13`
///    line is `login`'s); the screen goes down.
/// It then logs `boot13` out (plainly — no screen), deletes it, and resets the form, leaving the store
/// and the screen as the rest of the battery expects them.
/// GO-RED (LOGIN13 M3, run on this gate): the screen-first fold deleted from `wc_route_event` →
/// `keys_routed=false name_typed=false … -> FAIL —`.
#[cfg(feature = "loginst")]
pub fn login_rootout_fixture() {
    #[cfg(all(target_arch = "x86_64", feature = "wc"))]
    {
        use crate::video::crystal::login as screen;
        const N: &[u8] = b"boot13";
        const PW: &[u8] = b"boot13-pw";
        let mut con = crate::console::Console::new();
        let mut nb = [0u8; NAME_MAX];
        let root_before = root_session() && id_of(N).is_some();
        // 1 — the refusal: root with nobody to log in as. Only when `boot13` is the store's ONE row (a
        // fresh medium, which this lane's is), so the arm never deletes a user it did not create.
        let empty_refused = if user_count() == 1 && delete_user(N).is_ok() {
            shell_verb("logout", &[], &mut con);
            let refused = root_session() && !screen_up();
            let back = create_user(N, PW).is_ok(); // LOGIN14: made WITH its password here — the screen leg below logs in with it
            if refused && back { "no-users" } else { "NOT-REFUSED" }
        } else {
            "store-not-fresh"
        };
        // 2 — a program of the root session.
        let (pid, slot, windowed, root_stamped) = match crate::arch::syscall::root_session_launch() {
            Ok(v) => v,
            Err(why) => {
                serial_println!(":: LOGIN-ROOTOUT: launch -> FAIL — {} (the lane carries STAT.ELF; WINX-2 loads it every boot) ::", why);
                return;
            }
        };
        let others = crate::arch::syscall::root_session_others(slot);
        // 3 — the shell's `logout`, from root.
        shell_verb("logout", &[], &mut con);
        let root_after = root_session();
        let (pid_gone, window_gone) = crate::arch::syscall::root_session_probe(pid, slot);
        let up = screen_up();
        let screen_window = screen::fixture_windowed();
        // 4 — root is gone, so `adduser` is refused.
        shell_verb("adduser", &["boot13x"], &mut con);
        let not_root = *ADDUSER_LAST.lock();
        // 5 — the keyboard, through the LIVE x86 key router.
        let route = |b: u8| matches!(crate::arch::syscall::wc_route_event(crate::pal::Event::Key(b)), crate::pal::Event::Unknown);
        let mut routed = true;
        for &b in N {
            routed &= route(b);
        }
        let name_typed = screen::fixture_form_is(N, false);
        routed &= route(b'\t');
        let tab_pw = screen::fixture_form_is(N, true);
        for &b in b"wrong-pw" {
            routed &= route(b);
        }
        routed &= route(b'\n');
        let wrong_kept = screen_up() && whoami(&mut nb).is_none();
        for &b in PW {
            routed &= route(b);
        }
        routed &= route(b'\n');
        let logged_in = !screen_up() && matches!(whoami(&mut nb), Some(n) if &nb[..n] == N);
        // 6 — put the battery's world back: session closed plainly, the scratch user gone, form reset.
        let _ = logout();
        screen::fixture_reset();
        let cleaned = delete_user(N).is_ok() && !screen_up();
        let ok = root_before && empty_refused == "no-users" && windowed && root_stamped && !root_after && pid_gone && window_gone && up && screen_window && not_root == "not-root" && routed && name_typed && tab_pw && wrong_kept && logged_in && cleaned;
        serial_println!(
            ":: LOGIN-ROOTOUT: root_before={} empty_refused={} pid={} root_stamped={} others={} root_after={} pid_gone={} window_gone={} screen={} screen_window={} not_root={} keys_routed={} name_typed={} tab={} wrong={} login={} cleaned={} -> {} ::",
            root_before, empty_refused, pid, root_stamped, others, root_after, pid_gone, window_gone,
            if up { "up" } else { "DOWN" }, screen_window, not_root, routed, name_typed,
            if tab_pw { "password" } else { "NOT-MOVED" }, if wrong_kept { "denied" } else { "NOT-DENIED" },
            if logged_in { "boot13" } else { "NONE" }, cleaned, if ok { "PASS" } else { "FAIL —" }
        );
    }
    #[cfg(not(all(target_arch = "x86_64", feature = "wc")))]
    {
        serial_println!("[users] root log-out fixture: x86 `wc` lane only (the screen and the x86 key router are what it drives)");
    }
}
