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

//! VOLKEY (rmbp-ledger B684) M1 — the volume KEY SLOT: encryption with the key the user holds (BP-bar-15).
//!
//! CHARTER: Matrix (the filesystem) — shared-core. The slot is part of the UnaFS on-disk format, so the ONE
//! implementation lives in this crate and the kernel, `tools/unafs` and the installer call it (BP-storage-7);
//! the arithmetic is `crypto_core`'s (Holocron's shared-core), nothing here is a second primitive.
//!
//! # What is on the disk (unafs-records.md §R2a)
//!
//! Block 0, SECTOR 1 (bytes 512..696 of the volume): a hand-packed, magic-discriminated record. Sector 0
//! (the 37-byte superblock, §R2) is never changed by keying — the slot write rewrites block 0 with sector 0
//! byte-identical, so a torn write cannot reach the superblock. Zero bytes there (every volume before VOLKEY)
//! = unkeyed; the magic `UNAFSKS1` = keyed.
//!
//! The slot holds ONE per-volume DATA KEY (32 random bytes) wrapped twice, never in the clear:
//!
//! * by the LOGIN SECRET: KEK = Argon2id v1.3 (RFC 9106; `crypto_core::argon2`, the exact call `SYS_KDF`'s
//!   body makes — `keyring::kdf`) over the password, a 16-byte random salt and the parameters recorded in the
//!   slot (bounded to `SYS_KDF`'s own range, so a hostile slot cannot ask for 4 GiB);
//! * by the RECOVERY KEY: 24 words from [`WORDS`] (256 words, 8 bits each) = 22 random bytes + a 2-byte
//!   checksum; KEK = HKDF-SHA256(salt = 16 random bytes, ikm = the 22 bytes, info = `UnaFS volkey recovery v1`).
//!   176 bits need no stretching. The words are shown ONCE by whoever made the slot and are never written:
//!   [`RecoveryWords`] wipes itself on drop.
//!
//! Each wrap is ChaCha20-Poly1305 (RFC 8439) with a random 96-bit nonce and AAD = a role label ‖ the slot
//! header (magic … password salt), so a flipped parameter, a swapped salt or a wrap moved between roles is
//! refused by the tag before any key is released. A wrong password and a wrong recovery key read ONE answer:
//! [`KeySlotError::KeyRefused`].
//!
//! # The two ciphers a slot names
//!
//! `cipher = 0` ([`CIPHER_NONE`], VOLKEY M1): the data blocks are plaintext. The slot makes every reader that
//! goes through [`crate::UnaFS::mount`] refuse the volume (`KeyRequired`) and [`crate::UnaFS::mount_with`]
//! open it only after the key unwraps — the kernel's own mounts and `tools/unafs` alike — but a raw read of
//! the medium still sees plaintext, and erasing the slot makes the volume mount unkeyed.
//!
//! `cipher = 1` ([`CIPHER_AES256_XTS`], XTS rmbp-ledger B689 = VOLKEY M2): EVERY BLOCK BUT BLOCK 0 is
//! XTS-AES-256 (IEEE 1619, `crypto_core::xts`) under two keys HKDF-SHA256 expands from the data key, the tweak
//! the block number ([`crate::storage::VolumeCipher`], unafs-records.md §R2b); the superblock's version field
//! reads [`VERSION_CIPHERED`] (8), so a reader older than XTS refuses the volume instead of reading ciphertext.
//! A raw read sees only ciphertext, and erasing the slot leaves a v8 volume nobody mounts. Such a volume is
//! made by [`crate::fs::encipher`] (a block-for-block copy), never by keying a plaintext volume in place.
//! The cipher value is bound into both wraps (the header is AAD), so editing it on the disk fails the tag.
//!
//! Neither releases the data key to a caller: [`KeySlot::unwrap`] returns a [`VolumeKey`] that wipes itself on
//! drop; the mount of a `cipher = 1` volume expands it into the device's cipher and drops it at once.

use alloc::vec::Vec;
use crypto_core::ct::Zeroize;

/// Where the slot sits in block 0 (sector 1).
pub const SLOT_OFFSET: usize = 512;
/// The slot's magic.
pub const SLOT_MAGIC: [u8; 8] = *b"UNAFSKS1";
/// The slot's encoded length.
pub const SLOT_LEN: usize = 184;
/// The slot format this build writes and reads.
pub const SLOT_VERSION: u32 = 1;
/// M1: data blocks plaintext; the slot gates every mount through this crate.
pub const CIPHER_NONE: u32 = 0;
/// XTS (B689, VOLKEY M2): every block but block 0 AES-256-XTS from the data key (§R2b).
pub const CIPHER_AES256_XTS: u32 = 1;
/// XTS (B689): the superblock version a `cipher = 1` volume carries on the disk (§R2 `version`, bytes 5..9 of
/// block 0). The layout under the cipher is v7's; 8 is the INCOMPAT stamp (BP-storage-6) that makes every
/// reader built before XTS — which mounts 3..=7 and has never heard of sector 1 — refuse the volume rather than
/// take ciphertext for maps. `UnaFS::mount_with` accepts it only beside a `cipher = 1` slot and reads the v7
/// layout through the cipher.
pub const VERSION_CIPHERED: u32 = 8;
/// The password KDF a slot names: Argon2id v1.3.
pub const KDF_ARGON2ID: u32 = 2;
/// The data key's length.
pub const KEY_LEN: usize = 32;
/// Words in a recovery key.
pub const RECOVERY_WORDS: usize = 24;
/// Random bytes behind a recovery key (the other 2 of the 24 are its checksum).
pub const RECOVERY_ENTROPY: usize = 22;

const AAD_PW: &[u8] = b"UnaFS volkey password v1";
const AAD_RC: &[u8] = b"UnaFS volkey recovery v1";
const HDR_LEN: usize = 48;

/// Argon2id costs recorded in a slot. Bounds = `SYS_KDF`'s (una-abi `KDF_*_MIN`/`MAX`): the floor is the OWASP
/// 2023 Argon2id floor, the ceiling RFC 9106's second recommended option.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KdfParams {
    /// Memory in KiB.
    pub m_kib: u32,
    /// Passes.
    pub t: u32,
    /// Lanes.
    pub p: u32,
}

impl KdfParams {
    /// The floor (19 MiB, t 2, p 1) — `tests volkey` and nothing on a real volume below it.
    pub const FLOOR: KdfParams = KdfParams { m_kib: 19 * 1024, t: 2, p: 1 };
    /// The host tool's default (RFC 9106 §4 second option: 64 MiB, t 3, p 4).
    pub const HOST: KdfParams = KdfParams { m_kib: 64 * 1024, t: 3, p: 4 };

    /// Inside `SYS_KDF`'s range.
    pub fn valid(&self) -> bool {
        (19 * 1024..=64 * 1024).contains(&self.m_kib)
            && (2..=10).contains(&self.t)
            && (1..=8).contains(&self.p)
            && self.m_kib >= 8 * self.p
    }
}

/// Why a slot refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeySlotError {
    /// The password or the recovery key does not unwrap the data key (one answer for both, and for a
    /// tampered slot).
    KeyRefused,
    /// The recovery words are not 24 words of the list, or their checksum is wrong (a typo, said before any
    /// key work).
    BadRecovery,
    /// The slot names a version, cipher or KDF this build does not implement, or parameters outside the
    /// bounds: refused, never guessed at.
    Unsupported,
    /// The KDF's memory could not be reserved.
    NoMemory,
    /// The volume already carries a slot (re-wrapping is a separate operation).
    AlreadyKeyed,
}

impl core::fmt::Display for KeySlotError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            KeySlotError::KeyRefused => "the key does not unlock this volume",
            KeySlotError::BadRecovery => "not a recovery key (24 words of the list, checksum)",
            KeySlotError::Unsupported => "key slot version, cipher or KDF not supported by this build",
            KeySlotError::NoMemory => "no memory for the key derivation",
            KeySlotError::AlreadyKeyed => "the volume already has a key slot",
        })
    }
}

/// What opens a keyed volume.
#[derive(Clone, Copy)]
pub enum Unlock<'a> {
    /// The login secret.
    Password(&'a [u8]),
    /// The 24 recovery words (whitespace-separated; case-insensitive; a word may be cut to its first four
    /// letters).
    Recovery(&'a str),
}

/// The unwrapped data key. Wiped on drop; its one consumer is [`crate::storage::VolumeCipher::new`] (a
/// `cipher = 1` volume's block path).
pub struct VolumeKey([u8; KEY_LEN]);

impl VolumeKey {
    /// The key bytes (the block path's HKDF; `tests volkey`'s never-on-disk scan).
    pub fn bytes(&self) -> &[u8; KEY_LEN] {
        &self.0
    }
}

impl Drop for VolumeKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// The recovery words, shown once. ASCII lowercase words separated by single spaces. Wiped on drop.
pub struct RecoveryWords(Vec<u8>);

impl RecoveryWords {
    /// The words as one line.
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.0).unwrap_or("")
    }

    /// The `i`-th word (0-based).
    pub fn word(&self, i: usize) -> Option<&str> {
        self.as_str().split(' ').nth(i)
    }
}

impl Drop for RecoveryWords {
    fn drop(&mut self) {
        self.0.as_mut_slice().zeroize();
    }
}

/// The key slot (§R2a). Holds only wrapped material, salts and nonces: nothing in it is secret.
#[derive(Clone, PartialEq, Eq)]
pub struct KeySlot {
    /// [`CIPHER_NONE`] (M1) or [`CIPHER_AES256_XTS`] (XTS, B689).
    pub cipher: u32,
    /// The password KDF's costs.
    pub kdf: KdfParams,
    salt: [u8; 16],
    pw_nonce: [u8; 12],
    pw_wrapped: [u8; KEY_LEN],
    pw_tag: [u8; 16],
    rc_salt: [u8; 16],
    rc_nonce: [u8; 12],
    rc_wrapped: [u8; KEY_LEN],
    rc_tag: [u8; 16],
}

impl core::fmt::Debug for KeySlot {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "KeySlot {{ cipher: {}, kdf: argon2id m={}KiB t={} p={} }}", self.cipher, self.kdf.m_kib, self.kdf.t, self.kdf.p)
    }
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn arr<const N: usize>(b: &[u8], o: usize) -> [u8; N] {
    let mut a = [0u8; N];
    a.copy_from_slice(&b[o..o + N]);
    a
}

/// Argon2id(password, salt, kdf) → the password KEK. The same `crypto_core::argon2` call `SYS_KDF`'s body
/// makes (Argon2id, v1.3, no secret, no associated data, 32-byte output); memory reserved fallibly and zeroed
/// before it is freed.
pub fn password_kek(password: &[u8], salt: &[u8; 16], kdf: KdfParams) -> Result<[u8; KEY_LEN], KeySlotError> {
    use crypto_core::argon2::{argon2, Block, Params, Variant, Version};
    if !kdf.valid() {
        return Err(KeySlotError::Unsupported);
    }
    let params = Params { variant: Variant::Argon2id, version: Version::V0x13, m_kib: kdf.m_kib, t: kdf.t, p: kdf.p };
    let n = params.blocks();
    let mut mem: Vec<Block> = Vec::new();
    if mem.try_reserve_exact(n).is_err() {
        return Err(KeySlotError::NoMemory);
    }
    mem.resize(n, Block::ZERO);
    let mut out = [0u8; KEY_LEN];
    let r = argon2(&params, password, salt, &[], &[], &mut mem, &mut out);
    for b in mem.iter_mut() {
        b.0.zeroize();
    }
    match r {
        Ok(()) => Ok(out),
        Err(_) => Err(KeySlotError::Unsupported),
    }
}

fn recovery_kek(entropy: &[u8; RECOVERY_ENTROPY], salt: &[u8; 16]) -> [u8; KEY_LEN] {
    let mut out = [0u8; KEY_LEN];
    // 32 bytes of HKDF-SHA256 output is always inside RFC 5869's 255 × 32 bound.
    let _ = crypto_core::hkdf::hkdf::<crypto_core::sha2::Sha256>(salt, entropy, AAD_RC, &mut out);
    out
}

fn checksum(entropy: &[u8; RECOVERY_ENTROPY]) -> [u8; 2] {
    let mut h = crypto_core::sha2::Sha256::new();
    h.update(b"UnaFS volkey words v1");
    h.update(entropy);
    let d = h.finalize();
    [d[0], d[1]]
}

/// The 24 words for `entropy` (22 bytes, then the 2 checksum bytes, one word per byte).
pub fn recovery_words(entropy: &[u8; RECOVERY_ENTROPY]) -> RecoveryWords {
    let c = checksum(entropy);
    let mut out = Vec::with_capacity(RECOVERY_WORDS * 7);
    for (i, b) in entropy.iter().chain(c.iter()).enumerate() {
        if i > 0 {
            out.push(b' ');
        }
        out.extend_from_slice(WORDS[*b as usize].as_bytes());
    }
    RecoveryWords(out)
}

/// One typed word → its byte: an exact (case-insensitive) match, or a unique first-four-letter prefix.
fn word_index(w: &str) -> Option<u8> {
    let w = w.as_bytes();
    if w.is_empty() || w.len() > 16 {
        return None;
    }
    let mut lower = [0u8; 16];
    for (d, s) in lower.iter_mut().zip(w) {
        *d = s.to_ascii_lowercase();
    }
    let w = &lower[..w.len()];
    if let Some(i) = WORDS.iter().position(|x| x.as_bytes() == w) {
        return Some(i as u8);
    }
    // A cut word: at least its first four letters, and a prefix of exactly one word (the list is
    // four-letter-unique, `words_ok`, so at most one word can match).
    if w.len() >= 4 {
        return WORDS.iter().position(|x| x.as_bytes().starts_with(w)).map(|i| i as u8);
    }
    None
}

/// The 22 random bytes behind typed recovery words, or [`KeySlotError::BadRecovery`].
pub fn parse_recovery(words: &str) -> Result<[u8; RECOVERY_ENTROPY], KeySlotError> {
    let mut raw = [0u8; RECOVERY_WORDS];
    let mut n = 0usize;
    for w in words.split_whitespace() {
        if n == RECOVERY_WORDS {
            raw.zeroize();
            return Err(KeySlotError::BadRecovery);
        }
        match word_index(w) {
            Some(b) => raw[n] = b,
            None => {
                raw.zeroize();
                return Err(KeySlotError::BadRecovery);
            }
        }
        n += 1;
    }
    let mut e: [u8; RECOVERY_ENTROPY] = arr(&raw, 0);
    let ok = n == RECOVERY_WORDS && checksum(&e) == [raw[22], raw[23]];
    raw.zeroize();
    if !ok {
        e.zeroize();
        return Err(KeySlotError::BadRecovery);
    }
    Ok(e)
}

impl KeySlot {
    /// Make a slot: a fresh data key, wrapped by `password` (Argon2id at `kdf`) and by a fresh recovery key.
    /// `rng` fills buffers from the caller's entropy source (the kernel's DRBG, the host's `/dev/urandom`).
    /// Returns the slot and the recovery words — show them ONCE, then drop them. The data key never leaves
    /// this function.
    pub fn seal(password: &[u8], kdf: KdfParams, rng: &mut dyn FnMut(&mut [u8])) -> Result<(KeySlot, RecoveryWords), KeySlotError> {
        Self::seal_cipher(password, kdf, CIPHER_NONE, rng).map(|(slot, words, key)| {
            drop(key);
            (slot, words)
        })
    }

    /// XTS (B689): make a `cipher = 1` slot — as [`seal`](Self::seal), and the fresh data key is returned too
    /// (wiped on drop), because the one thing such a slot is for is [`crate::fs::encipher`], which writes the
    /// volume's blocks under it. Show the words once, encipher, drop the key.
    pub fn seal_ciphered(
        password: &[u8],
        kdf: KdfParams,
        rng: &mut dyn FnMut(&mut [u8]),
    ) -> Result<(KeySlot, RecoveryWords, VolumeKey), KeySlotError> {
        Self::seal_cipher(password, kdf, CIPHER_AES256_XTS, rng)
    }

    fn seal_cipher(
        password: &[u8],
        kdf: KdfParams,
        cipher: u32,
        rng: &mut dyn FnMut(&mut [u8]),
    ) -> Result<(KeySlot, RecoveryWords, VolumeKey), KeySlotError> {
        if !kdf.valid() || !cipher_known(cipher) {
            return Err(KeySlotError::Unsupported);
        }
        let mut key = [0u8; KEY_LEN];
        let mut entropy = [0u8; RECOVERY_ENTROPY];
        let mut slot = KeySlot {
            cipher,
            kdf,
            salt: [0; 16],
            pw_nonce: [0; 12],
            pw_wrapped: [0; KEY_LEN],
            pw_tag: [0; 16],
            rc_salt: [0; 16],
            rc_nonce: [0; 12],
            rc_wrapped: [0; KEY_LEN],
            rc_tag: [0; 16],
        };
        rng(&mut key);
        rng(&mut entropy);
        rng(&mut slot.salt);
        rng(&mut slot.pw_nonce);
        rng(&mut slot.rc_salt);
        rng(&mut slot.rc_nonce);
        let r = slot.wrap_both(password, &key, &entropy);
        let vk = VolumeKey(key);
        key.zeroize();
        let words = recovery_words(&entropy);
        entropy.zeroize();
        r.map(|()| (slot, words, vk))
    }

    fn wrap_both(&mut self, password: &[u8], key: &[u8; KEY_LEN], entropy: &[u8; RECOVERY_ENTROPY]) -> Result<(), KeySlotError> {
        let hdr = self.header();
        let mut kek = password_kek(password, &self.salt, self.kdf)?;
        let mut buf = *key;
        let t = crypto_core::chacha20poly1305::seal_in_place(&kek, &self.pw_nonce, &aad(AAD_PW, &hdr, &[]), &mut buf);
        kek.zeroize();
        self.pw_wrapped = buf;
        self.pw_tag = t.map_err(|_| KeySlotError::Unsupported)?;
        let mut kek = recovery_kek(entropy, &self.rc_salt);
        let mut buf = *key;
        let t = crypto_core::chacha20poly1305::seal_in_place(&kek, &self.rc_nonce, &aad(AAD_RC, &hdr, &self.rc_salt), &mut buf);
        kek.zeroize();
        self.rc_wrapped = buf;
        self.rc_tag = t.map_err(|_| KeySlotError::Unsupported)?;
        Ok(())
    }

    /// Unwrap the data key with `unlock`. A wrong password, a wrong (but well-formed) recovery key and a
    /// tampered slot all answer [`KeySlotError::KeyRefused`].
    pub fn unwrap(&self, unlock: &Unlock<'_>) -> Result<VolumeKey, KeySlotError> {
        if !cipher_known(self.cipher) || !self.kdf.valid() {
            return Err(KeySlotError::Unsupported);
        }
        let hdr = self.header();
        let (mut kek, nonce, wrapped, tag, a) = match unlock {
            Unlock::Password(pw) => (password_kek(pw, &self.salt, self.kdf)?, self.pw_nonce, self.pw_wrapped, self.pw_tag, aad(AAD_PW, &hdr, &[])),
            Unlock::Recovery(words) => {
                let mut e = parse_recovery(words)?;
                let k = recovery_kek(&e, &self.rc_salt);
                e.zeroize();
                (k, self.rc_nonce, self.rc_wrapped, self.rc_tag, aad(AAD_RC, &hdr, &self.rc_salt))
            }
        };
        let mut buf = wrapped;
        let r = crypto_core::chacha20poly1305::open_in_place(&kek, &nonce, &a, &mut buf, &tag);
        kek.zeroize();
        match r {
            Ok(()) => Ok(VolumeKey(buf)),
            Err(_) => {
                buf.zeroize();
                Err(KeySlotError::KeyRefused)
            }
        }
    }

    /// The password KDF's salt (not secret: it is on the disk) — `tests volkey` derives the same KEK through
    /// `SYS_KDF`'s body with it.
    pub fn salt(&self) -> &[u8; 16] {
        &self.salt
    }

    /// The first 48 bytes (magic … password salt): bound into both wraps.
    fn header(&self) -> [u8; HDR_LEN] {
        let mut h = [0u8; HDR_LEN];
        h[0..8].copy_from_slice(&SLOT_MAGIC);
        h[8..12].copy_from_slice(&SLOT_VERSION.to_le_bytes());
        h[12..16].copy_from_slice(&self.cipher.to_le_bytes());
        h[16..20].copy_from_slice(&KDF_ARGON2ID.to_le_bytes());
        h[20..24].copy_from_slice(&self.kdf.m_kib.to_le_bytes());
        h[24..28].copy_from_slice(&self.kdf.t.to_le_bytes());
        h[28..32].copy_from_slice(&self.kdf.p.to_le_bytes());
        h[32..48].copy_from_slice(&self.salt);
        h
    }

    /// Write the slot into `block0` (a whole 4096-byte block 0) at [`SLOT_OFFSET`].
    pub fn encode_into(&self, block0: &mut [u8]) {
        let s = &mut block0[SLOT_OFFSET..SLOT_OFFSET + SLOT_LEN];
        s[0..HDR_LEN].copy_from_slice(&self.header());
        s[48..60].copy_from_slice(&self.pw_nonce);
        s[60..92].copy_from_slice(&self.pw_wrapped);
        s[92..108].copy_from_slice(&self.pw_tag);
        s[108..124].copy_from_slice(&self.rc_salt);
        s[124..136].copy_from_slice(&self.rc_nonce);
        s[136..168].copy_from_slice(&self.rc_wrapped);
        s[168..184].copy_from_slice(&self.rc_tag);
    }

    /// Read the slot from `block0`: `Ok(None)` when the magic is absent (an unkeyed volume, every volume
    /// before VOLKEY), [`KeySlotError::Unsupported`] for a slot this build cannot read.
    pub fn parse(block0: &[u8]) -> Result<Option<KeySlot>, KeySlotError> {
        let Some(s) = block0.get(SLOT_OFFSET..SLOT_OFFSET + SLOT_LEN) else {
            return Ok(None);
        };
        if s[0..8] != SLOT_MAGIC {
            return Ok(None);
        }
        let kdf = KdfParams { m_kib: u32_at(s, 20), t: u32_at(s, 24), p: u32_at(s, 28) };
        let cipher = u32_at(s, 12);
        if u32_at(s, 8) != SLOT_VERSION || !cipher_known(cipher) || u32_at(s, 16) != KDF_ARGON2ID || !kdf.valid() {
            return Err(KeySlotError::Unsupported);
        }
        Ok(Some(KeySlot {
            cipher,
            kdf,
            salt: arr(s, 32),
            pw_nonce: arr(s, 48),
            pw_wrapped: arr(s, 60),
            pw_tag: arr(s, 92),
            rc_salt: arr(s, 108),
            rc_nonce: arr(s, 124),
            rc_wrapped: arr(s, 136),
            rc_tag: arr(s, 168),
        }))
    }
}

/// The ciphers this build implements: none (M1) and AES-256-XTS (B689). Any other is refused, never guessed at.
fn cipher_known(cipher: u32) -> bool {
    cipher == CIPHER_NONE || cipher == CIPHER_AES256_XTS
}

fn aad(role: &[u8], hdr: &[u8; HDR_LEN], extra: &[u8]) -> Vec<u8> {
    let mut a = Vec::with_capacity(role.len() + HDR_LEN + extra.len());
    a.extend_from_slice(role);
    a.extend_from_slice(hdr);
    a.extend_from_slice(extra);
    a
}

/// The recovery word list: 256 words (one per byte value), lowercase ASCII, 3..=6 letters, sorted, every
/// word unique in its first four letters (so a word may be typed cut to four). Index = the byte.
pub const WORDS: [&str; 256] = [
    "able", "acid", "acorn", "actor", "adapt", "admit", "adobe", "adult", "agent", "agree", "alarm", "album",
    "alert", "alley", "alpha", "amber", "anchor", "angle", "ankle", "apple", "apron", "arena", "argue",
    "armor", "arrow", "aspen", "atlas", "atom", "attic", "audio", "aunt", "avoid", "awake", "bacon", "badge",
    "bagel", "baker", "banjo", "barn", "basil", "beach", "beard", "bench", "berry", "birch", "bison", "blade",
    "bloom", "blue", "board", "bonus", "boost", "brave", "bread", "brick", "broom", "brush", "cabin", "camel",
    "canoe", "cargo", "cedar", "cello", "chalk", "chess", "chief", "cider", "civic", "clay", "cliff", "clock",
    "cloud", "coast", "cocoa", "comet", "coral", "crane", "crown", "cube", "cycle", "daisy", "dance", "delta",
    "denim", "dream", "drift", "drum", "duck", "dune", "eagle", "earth", "easel", "echo", "edge", "elbow",
    "elder", "ember", "envoy", "epic", "equal", "erode", "ether", "event", "exact", "exit", "fancy", "farm",
    "fence", "ferry", "fiber", "field", "finch", "flame", "flute", "focus", "fox", "frame", "frost", "fruit",
    "gecko", "giant", "globe", "glove", "goat", "grape", "green", "gulf", "habit", "hazel", "heart", "hedge",
    "hero", "honey", "hotel", "icon", "idea", "igloo", "image", "index", "inlet", "iris", "ivory", "jelly",
    "jewel", "judge", "juice", "kayak", "kiwi", "knee", "koala", "lamp", "lava", "lemon", "lens", "lilac",
    "lime", "linen", "lion", "lunar", "mango", "maple", "melon", "menu", "metal", "mint", "model", "moose",
    "motor", "music", "navy", "nest", "noble", "north", "novel", "oasis", "ocean", "olive", "omega", "onion",
    "opera", "orbit", "otter", "oven", "owl", "panda", "paper", "peach", "piano", "pilot", "pine", "plum",
    "polar", "pony", "prism", "quail", "queen", "quiet", "quill", "radar", "radio", "raft", "raven", "razor",
    "reef", "rice", "ridge", "river", "robin", "rose", "ruby", "rugby", "sand", "satin", "scarf", "scout",
    "shell", "slate", "smile", "snow", "sofa", "solar", "spice", "spoon", "squid", "stamp", "stone", "sugar",
    "swan", "table", "tango", "tiger", "toast", "topaz", "torch", "tower", "tulip", "uncle", "union", "urban",
    "vapor", "video", "visit", "vivid", "water", "whale", "wheat", "wolf", "wool", "xenon", "yacht", "yarn",
    "yoga", "zebra", "zero", "zinc", "zone",];

const fn words_ok() -> bool {
    let mut i = 0;
    while i < WORDS.len() {
        let a = WORDS[i].as_bytes();
        if a.len() < 3 || a.len() > 6 {
            return false;
        }
        if i + 1 < WORDS.len() {
            let b = WORDS[i + 1].as_bytes();
            // strictly ascending (so unique), and different in the first four letters
            let mut j = 0;
            let mut differ_at = usize::MAX;
            while j < a.len() && j < b.len() {
                if a[j] != b[j] {
                    differ_at = j;
                    break;
                }
                j += 1;
            }
            if differ_at == usize::MAX || a[differ_at] > b[differ_at] || differ_at >= 4 {
                return false;
            }
        }
        i += 1;
    }
    true
}
const _: () = assert!(words_ok(), "VOLKEY: the recovery word list must be sorted and four-letter-unique");
const _: () = assert!(SLOT_OFFSET >= 512 && SLOT_OFFSET + SLOT_LEN <= 1024, "the slot lives in sector 1 of block 0");
