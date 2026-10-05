// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The production suite over CRYPTOCORE (`unaos/libs/sys/crypto_core`, SR27): Argon2id (RFC 9106),
//! HKDF-SHA-256 (RFC 5869), ChaCha20-Poly1305 (RFC 8439), Ed25519 (RFC 8032), and the entropy bridge —
//! [`DrbgEntropy`], CRYPTOCORE's fast-key-erasure ChaCha20 DRBG over a fallible source (the OS on the
//! host, SYS_GETRANDOM on the metal), whose failure Holocron turns into a refusal to seal.

use crate::seal::{Entropy, EntropyError, KdfParams, NONCE_LEN, SALT_LEN, SealError, Sealer, Signer};
use crate::zero::{Key, wipe};
use alloc::vec::Vec;

/// The production suite: Argon2id + HKDF-SHA-256 + ChaCha20-Poly1305, Ed25519 for the agent.
#[derive(Clone, Copy, Debug, Default)]
pub struct CryptoCore;

impl Sealer for CryptoCore {
    const SUITE: u8 = crate::format::SUITE_ARGON2ID_CHACHA20POLY1305;

    fn derive_key(&self, password: &[u8], salt: &[u8; SALT_LEN], params: &KdfParams) -> Result<Key, SealError> {
        use crypto_core::argon2::{Params, Variant, Version};
        let p = Params { variant: Variant::Argon2id, version: Version::V0x13, m_kib: params.m_kib, t: params.t, p: params.p };
        let mut out = [0u8; 32];
        crypto_core::argon2::hash(&p, password, salt, &[], &[], &mut out).map_err(|_| SealError::Param)?;
        let k = Key::from_bytes(out);
        wipe(&mut out);
        Ok(k)
    }

    fn subkey(&self, key: &Key, salt: &[u8], info: &[u8]) -> Key {
        let mut out = [0u8; 32];
        // 32 <= 255 * 32: HKDF-Expand cannot fail for this length.
        let _ = crypto_core::hkdf::hkdf::<crypto_core::Sha256>(salt, key.bytes(), info, &mut out);
        let k = Key::from_bytes(out);
        wipe(&mut out);
        k
    }

    fn seal(&self, key: &Key, nonce: &[u8; NONCE_LEN], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
        crypto_core::chacha20poly1305::seal(key.bytes(), nonce, aad, plaintext)
            .expect("ChaCha20-Poly1305 seal: message within RFC 8439 limits")
    }

    fn open(&self, key: &Key, nonce: &[u8; NONCE_LEN], aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>, SealError> {
        crypto_core::chacha20poly1305::open(key.bytes(), nonce, aad, sealed).map_err(|_| SealError::Auth)
    }
}

impl Signer for CryptoCore {
    const REAL: bool = true;

    fn public_key(&self, seed: &[u8; 32]) -> [u8; 32] {
        crypto_core::ed25519::SigningKey::from_seed(seed).public_key()
    }

    fn sign(&self, seed: &[u8; 32], msg: &[u8]) -> [u8; 64] {
        crypto_core::ed25519::SigningKey::from_seed(seed).sign(msg)
    }

    fn verify(&self, public: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> bool {
        crypto_core::ed25519::verify(public, msg, sig).is_ok()
    }
}

/// Holocron's [`Entropy`] over CRYPTOCORE's ChaCha20 DRBG. Construction draws the seed (and fails if the
/// source cannot deliver it); every fill after that can fail too — at a reseed whose source has died —
/// and that failure reaches the caller as [`EntropyError`]: the ring refuses to create, a secret refuses
/// to seal, a key refuses to mint.
pub struct DrbgEntropy<E: crypto_core::drbg::Entropy> {
    drbg: crypto_core::drbg::ChaChaDrbg<E>,
}

impl<E: crypto_core::drbg::Entropy> DrbgEntropy<E> {
    /// Seed a DRBG from `source`, personalised with `personalization`.
    pub fn new(source: E, personalization: &[u8]) -> Result<Self, EntropyError> {
        crypto_core::drbg::ChaChaDrbg::new(source, personalization).map(|drbg| DrbgEntropy { drbg }).map_err(|_| EntropyError)
    }
}

impl<E: crypto_core::drbg::Entropy> Entropy for DrbgEntropy<E> {
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), EntropyError> {
        for chunk in buf.chunks_mut(crypto_core::drbg::MAX_REQUEST) {
            if self.drbg.fill(chunk).is_err() {
                wipe(buf);
                return Err(EntropyError);
            }
        }
        Ok(())
    }
}
