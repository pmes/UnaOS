// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The CRYPTOCORE adapter — the fold of SR27 into Holocron is THIS file plus one Cargo line.
//!
//! Compiled only with `--features crypto_core`, which today has no dependency behind it (crypto_core is
//! being written on `exec-sec-crypto`), so enabling the feature before the fold fails to build — on
//! purpose. At the fold: uncomment the `crypto_core` dependency in this crate's Cargo.toml, make the
//! feature `crypto_core = ["dep:crypto_core"]`, confirm the four function names below against the
//! landed crate (`chacha20poly1305::{seal, open}` and `hkdf::hkdf` match exec-sec-crypto d74813aa;
//! `argon2::argon2id` and `ed25519::{public_key, sign, verify}` are that crate's M-later modules), and
//! run `cargo test -p holocron_core --features crypto_core` (the `cc_*` tests in tests/kat.rs then run
//! the RFC 8032 §7.1 TEST 1 vector through the agent and a real seal/open round trip).

use crate::seal::{KdfParams, NONCE_LEN, SALT_LEN, SealError, Sealer, Signer};
use crate::zero::{Key, wipe};
use alloc::vec::Vec;

/// The production suite: Argon2id + HKDF-SHA-256 + ChaCha20-Poly1305, Ed25519 for the agent.
#[derive(Clone, Copy, Debug, Default)]
pub struct CryptoCore;

impl Sealer for CryptoCore {
    const SUITE: u8 = crate::format::SUITE_ARGON2ID_CHACHA20POLY1305;

    fn derive_key(&self, password: &[u8], salt: &[u8; SALT_LEN], params: &KdfParams) -> Result<Key, SealError> {
        let mut out = [0u8; 32];
        crypto_core::argon2::argon2id(password, salt, params.m_kib, params.t, params.p, &mut out)
            .map_err(|_| SealError::Param)?;
        Ok(Key::from_bytes(out))
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
        crypto_core::ed25519::public_key(seed)
    }

    fn sign(&self, seed: &[u8; 32], msg: &[u8]) -> [u8; 64] {
        crypto_core::ed25519::sign(seed, msg)
    }

    fn verify(&self, public: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> bool {
        crypto_core::ed25519::verify(public, msg, sig)
    }
}
