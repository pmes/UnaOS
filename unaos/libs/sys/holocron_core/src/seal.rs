// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The cryptographic seam. Holocron owns keys and policy; the arithmetic is CRYPTOCORE's
//! (`unaos/libs/sys/crypto_core`, LEDGER SR27). These two traits are the WHOLE contract the fold must
//! satisfy — see `src/cc.rs` for the adapter and `docs/dev/evidence/host-1004/HOLOCRON1.md` for the
//! required primitives (Argon2id RFC 9106, HKDF-SHA-256 RFC 5869, ChaCha20-Poly1305 RFC 8439,
//! Ed25519 RFC 8032).

use crate::zero::Key;
use alloc::vec::Vec;

/// Bytes of the AEAD authentication tag every [`Sealer`] appends.
pub const TAG_LEN: usize = 16;
/// Bytes of the per-message nonce.
pub const NONCE_LEN: usize = 12;
/// Bytes of a KDF / HKDF salt.
pub const SALT_LEN: usize = 16;

/// Argon2id cost parameters (RFC 9106 §3.1). Recorded in every header so a ring can be re-derived and
/// migrated; checked against [`KdfParams::FLOOR`] at unlock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KdfParams {
    /// Memory in KiB (`m`).
    pub m_kib: u32,
    /// Passes (`t`).
    pub t: u32,
    /// Lanes (`p`).
    pub p: u32,
}

impl KdfParams {
    /// RFC 9106 §4, "SECOND RECOMMENDED option": t=3, p=4, m=2^16 KiB (64 MiB).
    pub const DEFAULT: KdfParams = KdfParams { m_kib: 1 << 16, t: 3, p: 4 };
    /// The weakest parameters a ring may be opened with (refused below): 19 MiB, t=2, p=1 — the OWASP
    /// 2023 Argon2id floor. A header advertising less is treated as tampered.
    pub const FLOOR: KdfParams = KdfParams { m_kib: 19 * 1024, t: 2, p: 1 };

    /// True when every cost is at or above the floor and within RFC 9106's ranges.
    pub fn acceptable(&self) -> bool {
        self.m_kib >= Self::FLOOR.m_kib
            && self.t >= Self::FLOOR.t
            && self.p >= Self::FLOOR.p
            && self.p <= 0x00FF_FFFF
            && self.m_kib >= 8 * self.p
    }
}

/// What a sealer can say went wrong. Deliberately coarse: an AEAD that fails says only `Auth`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SealError {
    /// Authentication failed (wrong key, tampered ciphertext, header or name).
    Auth,
    /// A length or parameter is outside what the primitive allows.
    Param,
}

/// Password-based key derivation + key separation + authenticated encryption.
///
/// Required semantics (the crypto_core adapter must meet ALL of them):
/// * `derive_key` = Argon2id(password, salt, m, t, p) with a 32-byte tag, version 0x13, no secret, no
///   associated data (RFC 9106 §3.1).
/// * `subkey` = HKDF-SHA-256(ikm = key, salt, info) expanded to 32 bytes (RFC 5869).
/// * `seal` = ChaCha20-Poly1305(key, nonce, aad, plaintext) returning `ciphertext || tag(16)`
///   (RFC 8439 §2.8); `open` is its inverse and returns `Err(Auth)` on any mismatch, revealing nothing
///   else (no partial plaintext).
pub trait Sealer {
    /// The suite byte written into every header this sealer produces; a header carrying another suite
    /// is refused before any key is derived.
    const SUITE: u8;
    /// Argon2id over the password.
    fn derive_key(&self, password: &[u8], salt: &[u8; SALT_LEN], params: &KdfParams) -> Result<Key, SealError>;
    /// HKDF-SHA-256 key separation.
    fn subkey(&self, key: &Key, salt: &[u8], info: &[u8]) -> Key;
    /// AEAD seal: `ciphertext || tag`.
    fn seal(&self, key: &Key, nonce: &[u8; NONCE_LEN], aad: &[u8], plaintext: &[u8]) -> Vec<u8>;
    /// AEAD open.
    fn open(&self, key: &Key, nonce: &[u8; NONCE_LEN], aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>, SealError>;
}

/// Ed25519 (RFC 8032 §5.1, PureEdDSA). The secret is the 32-byte seed `k` of §5.1.5.
pub trait Signer {
    /// True only for a real RFC 8032 implementation (the agent advertises a test key's comment with a
    /// `[TEST-INSECURE]` suffix when false).
    const REAL: bool;
    /// §5.1.5: the 32-byte encoded public key `A` for seed `k`.
    fn public_key(&self, seed: &[u8; 32]) -> [u8; 32];
    /// §5.1.6: the 64-byte signature `R || S` of `msg`.
    fn sign(&self, seed: &[u8; 32], msg: &[u8]) -> [u8; 64];
    /// §5.1.7: verification.
    fn verify(&self, public: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> bool;
}

/// The entropy source failed: nothing may be sealed or minted with what it produced.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntropyError;

/// A source of unpredictable bytes (host and metal: CRYPTOCORE's ChaCha20 DRBG over the OS / SYS_GETRANDOM,
/// [`crate::cc::DrbgEntropy`]). FALLIBLE: a source that cannot deliver says so, and Holocron refuses the
/// operation (no ring created, no secret sealed, no key minted) instead of sealing under a guessable salt,
/// nonce or seed.
pub trait Entropy {
    /// Fill `buf` entirely, or fail (in which case `buf`'s contents are meaningless and unused).
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), EntropyError>;
}
