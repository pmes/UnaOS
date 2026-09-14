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
//! What an attacker holding the card can read: every user NAME, every user's 16-byte SALT, and the
//! 32-byte SHA-256 of `salt || password`. Never a password: there is no plaintext credential on
//! disk or on the wire (the shell verb takes the password from the typed line and hashes it in
//! kernel RAM; the login screen never echoes it; no witness line prints it). What the attacker CAN
//! do with the card is an OFFLINE GUESS against the hash — one SHA-256 per guess, unthrottled — so a
//! weak password on a lost card is a weak password. That is the honest posture of a salted hash
//! with no hardware key store (the same posture `holocron.rs` states for its records); a slow KDF
//! and a hardware-backed secret are later arcs, and the `ver` byte is the migration hook.
//!
//! # On-disk format (v1) — fixed stride, CRC per row, fail-closed like Holocron
//!
//! ```text
//! header (16): magic "UNAUSR1\0" | ver u8 = 1 | count u8 | seq u16 LE | crc32 LE (over [0..12))
//! row   (128): name_len u8 | name[24] | home_len u8 | home[32] | salt[16] | hash[32] | crc32 LE | pad
//! ```
//!
//! Bad magic, unknown version, a bad header CRC, a bad row CRC, a count past the table, or a short
//! file refuses the WHOLE image and the store starts EMPTY, witnessed — no partial adoption. The
//! update is a SWAP: `USERS.NEW` is written, read back and parsed, and only then does `USERS.DAT`
//! go and the temp get renamed over it; a boot that finds no `USERS.DAT` but a parseable `USERS.NEW`
//! adopts it. CRC-32 is [`crate::hash::crc32`], SHA-256 is [`crate::hash::sha256`] — both
//! arch-neutral, both already in every image. The FAT calls are the ones `holocron.rs`'s store
//! helpers use (`locate_in_dir`/`create_in_dir`/`write_grow`/`read_at`/`delete_located`/
//! `rename_entry`), against parent cluster 0 (the root directory) and honouring `write_veto`.
//!
//! # Salt
//!
//! `sha256(cycle counter || name || seq)[..16]`. This kernel has no entropy source and this is NOT
//! presented as one: the salt's job here is that two users with the same password do not share a
//! hash and that a precomputed table does not apply; it is not secret and is not claimed to be.

use spin::Mutex;

use crate::fs::fat::{FatError, FatFs};
use crate::hash::{crc32, sha256};

// =========================================================================================
// FORMAT CONSTANTS
// =========================================================================================

/// On-disk magic. ASCII so an `xxd` of the medium names the file.
pub const USERS_MAGIC: [u8; 8] = *b"UNAUSR1\0";
/// Format version. Bump for an incompatible layout; a reader that does not know it refuses.
pub const USERS_VER: u8 = 1;
/// Header length and the span its CRC covers.
pub const USERS_HDR_LEN: usize = 16;
const USERS_HDR_CRC_SPAN: usize = 12;
/// One row's stride on disk and the span its CRC covers.
pub const USERS_ROW_LEN: usize = 128;
const USERS_ROW_CRC_SPAN: usize = 106;
/// Bounds. `NAME_MAX` is 8 since M2: the home directory is a FAT 8.3 leaf (`HOME/<NAME>`) on the EL0
/// volume, so a name is at most 8 bytes (the on-disk row keeps its 24-byte field; `user:<name>` is at
/// most 13, inside the 30-byte principal value field).
pub const MAX_USERS: usize = 8;
pub const NAME_MAX: usize = 8;
pub const HOME_MAX: usize = 32;
const SALT_LEN: usize = 16;
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
}

impl UserRec {
    const EMPTY: Self = UserRec {
        name_len: 0,
        name: [0; NAME_MAX],
        home_len: 0,
        home: [0; HOME_MAX],
        salt: [0; SALT_LEN],
        hash: [0; HASH_LEN],
    };

    pub fn name(&self) -> &[u8] {
        &self.name[..self.name_len as usize]
    }

    pub fn home(&self) -> &[u8] {
        &self.home[..self.home_len as usize]
    }

    fn write(&self, out: &mut [u8]) {
        out[..USERS_ROW_LEN].fill(0);
        out[0] = self.name_len;
        out[1..1 + NAME_MAX].copy_from_slice(&self.name);
        out[25] = self.home_len;
        out[26..26 + HOME_MAX].copy_from_slice(&self.home);
        out[58..58 + SALT_LEN].copy_from_slice(&self.salt);
        out[74..74 + HASH_LEN].copy_from_slice(&self.hash);
        let c = crc32(&out[..USERS_ROW_CRC_SPAN]);
        out[106..110].copy_from_slice(&c.to_le_bytes());
    }

    fn read(b: &[u8]) -> Option<Self> {
        if b.len() < USERS_ROW_LEN {
            return None;
        }
        let want = u32::from_le_bytes([b[106], b[107], b[108], b[109]]);
        if crc32(&b[..USERS_ROW_CRC_SPAN]) != want {
            return None;
        }
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

fn password_hash(salt: &[u8; SALT_LEN], password: &[u8]) -> [u8; HASH_LEN] {
    let mut h = crate::hash::Sha256::new();
    h.update(salt);
    h.update(password);
    h.finalize()
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
    rows: [UserRec; MAX_USERS],
}

static TABLE: Mutex<Table> = Mutex::new(Table {
    loaded: false,
    seq: 0,
    count: 0,
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
    }
}

fn serialize_into(seq: u16, rows: &[UserRec], out: &mut [u8]) -> usize {
    let need = USERS_HDR_LEN + USERS_ROW_LEN * rows.len();
    debug_assert!(out.len() >= need);
    out[0..8].copy_from_slice(&USERS_MAGIC);
    out[8] = USERS_VER;
    out[9] = rows.len() as u8;
    out[10..12].copy_from_slice(&seq.to_le_bytes());
    let c = crc32(&out[..USERS_HDR_CRC_SPAN]);
    out[12..16].copy_from_slice(&c.to_le_bytes());
    let mut at = USERS_HDR_LEN;
    for r in rows {
        r.write(&mut out[at..at + USERS_ROW_LEN]);
        at += USERS_ROW_LEN;
    }
    at
}

fn parse_image(img: &[u8]) -> Result<(u16, u8, [UserRec; MAX_USERS]), UsersError> {
    if img.len() < USERS_HDR_LEN {
        return Err(UsersError::Truncated);
    }
    if img[0..8] != USERS_MAGIC {
        return Err(UsersError::BadMagic);
    }
    if img[8] != USERS_VER {
        return Err(UsersError::BadVersion);
    }
    let want = u32::from_le_bytes([img[12], img[13], img[14], img[15]]);
    if crc32(&img[..USERS_HDR_CRC_SPAN]) != want {
        return Err(UsersError::BadHeaderCrc);
    }
    let count = img[9] as usize;
    if count > MAX_USERS {
        return Err(UsersError::TooMany);
    }
    if img.len() < USERS_HDR_LEN + USERS_ROW_LEN * count {
        return Err(UsersError::Truncated);
    }
    let seq = u16::from_le_bytes([img[10], img[11]]);
    let mut rows = [UserRec::EMPTY; MAX_USERS];
    for i in 0..count {
        let at = USERS_HDR_LEN + USERS_ROW_LEN * i;
        rows[i] = UserRec::read(&img[at..at + USERS_ROW_LEN]).ok_or(UsersError::BadRow)?;
    }
    Ok((seq, count as u8, rows))
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
            serial_println!("[users] load volume=el0-fat({}) src=none users=0 (fresh store)", veto);
        }
        Some(b) => match parse_image(&b) {
            Ok((seq, count, rows)) => {
                t.seq = seq;
                t.count = count;
                t.rows = rows;
                serial_println!("[users] load volume=el0-fat({}) src={} users={} seq={}", veto, src, count, seq);
            }
            Err(e) => {
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
fn flush(t: &mut Table) -> Result<(), UsersError> {
    let fs = crate::fs::fat::mount().map_err(|_| UsersError::Volume)?;
    let mut img = [0u8; USERS_IMAGE_MAX];
    t.seq = t.seq.wrapping_add(1);
    let n = serialize_into(t.seq, &t.rows[..t.count as usize], &mut img);
    // 1. a fresh temp
    write_root_file(&fs, USERS_TMP_FILE, &img[..n])?;
    // 2. prove the new generation is readable on the medium before the old one goes
    let back = read_root_file(&fs, USERS_TMP_FILE).ok_or(UsersError::Volume)?;
    if back[..] != img[..n] {
        return Err(UsersError::Volume);
    }
    let (seq, count, _) = parse_image(&back)?;
    if seq != t.seq || count != t.count {
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

/// The 1-based id of `name` (its row + 1), or `None`. Ids are what the x86 session carries
/// (its ACL has no principal string); aarch64 carries the name itself.
pub fn id_of(name: &[u8]) -> Option<u32> {
    let t = TABLE.lock();
    (0..t.count as usize).find(|&i| t.rows[i].name() == name).map(|i| i as u32 + 1)
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
/// the login screen drives; every later one is the same call.
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
    if t.count as usize >= MAX_USERS {
        return Err(UsersError::Full);
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
    // salt: uniqueness, not secrecy (module doc)
    let mut sh = crate::hash::Sha256::new();
    sh.update(&crate::arch::now_cycles().to_le_bytes());
    sh.update(name);
    sh.update(&t.seq.to_le_bytes());
    let d = sh.finalize();
    r.salt.copy_from_slice(&d[..SALT_LEN]);
    r.hash = password_hash(&r.salt, password);
    let i = t.count as usize;
    t.rows[i] = r;
    t.count += 1;
    match flush(&mut t) {
        Ok(()) => Ok(i as u32 + 1),
        Err(e) => {
            // the publish failed: the store did not change, so neither does RAM
            t.rows[i] = UserRec::EMPTY;
            t.count -= 1;
            Err(e)
        }
    }
}

/// Does `password` verify for `name`? One answer for "no such user" and "wrong password".
pub fn verify(name: &[u8], password: &[u8]) -> bool {
    let t = TABLE.lock();
    let Some(i) = (0..t.count as usize).find(|&i| t.rows[i].name() == name) else {
        return false;
    };
    let h = password_hash(&t.rows[i].salt, password);
    digest_eq(&h, &t.rows[i].hash)
}

/// Log in: verify, then open the session under `user:<name>` — the ONE principal every program
/// launched from now on is stamped with (`arch::syscall::session_login`). Refused means no change.
pub fn login(name: &[u8], password: &[u8]) -> Result<(), UsersError> {
    if !verify(name, password) {
        serial_println!("[users] login refused");
        return Err(UsersError::Refused);
    }
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
        "[users] login ok user={} id={} principal=user:{}",
        core::str::from_utf8(name).unwrap_or("?"),
        id,
        core::str::from_utf8(&nb[..name.len()]).unwrap_or("?")
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

/// Log out: close the session (SO37: and BURN THE EPOCH — see `arch::syscall::session_logout`). The
/// programs the session launched keep the principal they were stamped with at load, and that stamp now
/// reads ANONYMOUS from `slot_ppid_of` down, so it opens nothing the user owns and creates nothing in the
/// user's name. What it keeps is what its own live `(asid, gen)` incarnation owns — the pre-login world.
pub fn logout() {
    arch_session_logout();
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
    serial_println!("[users] logout epoch={} (SO37: stamps from the closed session are refused)", arch_session_epoch());
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

fn arch_session_logout() {
    #[cfg(any(target_arch = "x86_64", feature = "aarch64_el0"))]
    crate::arch::syscall::session_logout();
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
                    logout();
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
            login_fixture();
            #[cfg(all(feature = "loginst", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
            crate::video::strip::login_press_fixture(b"una", b"correct-horse"); // SO36/SO44 — the INPUT GATE. Here, BEFORE the screen fixture, because it needs three things this point in `service` guarantees: the panel real (the fixture mints a stand-in `wm` row to be the window behind), `una` already in the store (`login_fixture` above created it), and the screen DOWN — which it does not assume: it measures `screen_press` at its own point first and REDS if that reads true, so a boot that had the screen up here goes loud instead of quietly passing. It puts the boot back where it found it: row closed, screen down, no session.
            #[cfg(all(feature = "loginst", any(all(target_arch = "x86_64", feature = "wc"), all(target_arch = "aarch64", feature = "desktop_firmware"))))]
            crate::video::crystal::login::screen_fixture(b"una", b"correct-horse", b"wrong-horse", crate::video::crystal::logout_row_fire); // LOGIN M4 — the Log Out route under test is the CRYSTAL MENU'S ROW (`crystal::logout_row_fire`), not the screen's own reopen; M3's `logout_direct` retires with it.
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
    logout();
    let none_after = whoami(&mut nb).is_none();
    let all = verify_ok && wrong_refused && wrong_login_refused && none_before && login_ok && principal_ok && none_after && home != "FAIL" && acl != "FAIL" && epoch != "FAIL";
    if all {
        serial_println!(
            ":: LOGIN: users+session create={} verify=ok wrong=refused login=ok principal=user:una linked={} home={} acl={} epoch={} logout=ok users={} volume={} -> PASS ::",
            create,
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
