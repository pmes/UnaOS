// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `CryptoCoreProvider` — tls_core's `trait CryptoProvider` (TLSCORE, SR28) implemented over CRYPTOCORE
//! (`crypto_core`, SR27). THE FOLD FILE: it is not compiled inside crypto_core (the orphan rule puts an
//! `impl ForeignTrait for LocalType` in the trait's crate, and tls_core depends on crypto_core, not the
//! reverse). At the fold, copy it to `unaos/libs/sys/tls_core/src/cryptocore_provider.rs` and add:
//!
//! ```toml
//! # tls_core/Cargo.toml
//! [features]
//! cryptocore = ["dep:crypto_core"]
//! cryptocore-std = ["cryptocore", "crypto_core/std"]   # host: CryptoCoreProvider::new() over /dev/urandom
//! [dependencies]
//! crypto_core = { path = "../crypto_core", optional = true, default-features = false, features = ["alloc"] }
//! ```
//! ```ignore
//! // tls_core/src/lib.rs
//! #[cfg(feature = "cryptocore")]
//! pub mod cryptocore_provider;
//! ```
//!
//! (`new()`/`Default` are behind `cryptocore-std`; ring 3 on UnaOS and the kernel use `with_entropy`.) The constructor surface mirrors `test_provider::RustCryptoProvider` (`new`,
//! `with_rng_pool`, `pool_remaining`), so tls_core's own tests switch provider with a rename. Proven so,
//! from a scratch workspace holding exec-sec-tls's tls_core and this crate: see
//! docs/dev/evidence/sec-1004/CRYPTOCORE.md, "the TLSCORE fold proof".
//!
//! Randomness: a CRYPTOCORE `ChaChaDrbg` over the supplied `Entropy` (kernel: `rand::KernelEntropy`;
//! ring 3: `drbg::GetrandomEntropy` wrapping SYS_GETRANDOM; host: `drbg::OsEntropy`). A test pool replays
//! fixed bytes so the RFC 8448 traces reproduce exactly.
//!
//! What it does NOT have (default `Unsupported` bodies stay in force, and `supports_signature` keeps the
//! trait's baseline so no server is invited to pick them): ECDSA P-384, RSASSA-PSS, RSASSA-PKCS1-v1_5.

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::cell::RefCell;

use crypto_core::drbg::{ChaChaDrbg, Entropy};
use crypto_core::{chacha20poly1305, ed25519, gcm, hmac::Hmac, p256, x25519, Sha256, Sha384, Sha512};

use crate::crypto::{AeadAlg, CryptoError, CryptoProvider, Digest, EcCurve, HashAlg, KxPrivate};

/// A boxed entropy source, so the provider is one concrete (object-safe) type.
struct DynEntropy(Box<dyn Entropy>);

impl Entropy for DynEntropy {
    fn fill(&mut self, out: &mut [u8]) -> Result<(), crypto_core::Error> {
        self.0.fill(out)
    }
}

enum Rng {
    Drbg(ChaChaDrbg<DynEntropy>),
    Pool(Vec<u8>, usize),
}

/// The product `CryptoProvider`.
pub struct CryptoCoreProvider {
    rng: RefCell<Rng>,
}

impl CryptoCoreProvider {
    /// Over any CRYPTOCORE entropy source (seeded DRBG; an entropy failure is `CryptoError::Rng`).
    pub fn with_entropy(source: Box<dyn Entropy>) -> Result<Self, CryptoError> {
        let d = ChaChaDrbg::new(DynEntropy(source), b"tls_core CryptoCoreProvider").map_err(|_| CryptoError::Rng)?;
        Ok(CryptoCoreProvider { rng: RefCell::new(Rng::Drbg(d)) })
    }

    /// Host convenience: the DRBG over `/dev/urandom` (feature `cryptocore-std`).
    #[cfg(feature = "cryptocore-std")]
    pub fn new() -> Self {
        Self::with_entropy(Box::new(crypto_core::drbg::OsEntropy)).expect("/dev/urandom")
    }

    /// Deterministic: `random` hands out these bytes in order and fails when they run out (test traces).
    pub fn with_rng_pool(pool: Vec<u8>) -> Self {
        CryptoCoreProvider { rng: RefCell::new(Rng::Pool(pool, 0)) }
    }

    /// Bytes of the pool not yet consumed (0 for a DRBG provider).
    pub fn pool_remaining(&self) -> usize {
        match &*self.rng.borrow() {
            Rng::Pool(p, i) => p.len() - i,
            Rng::Drbg(_) => 0,
        }
    }
}

#[cfg(feature = "cryptocore-std")]
impl Default for CryptoCoreProvider {
    fn default() -> Self {
        Self::new()
    }
}

fn hash_parts<D: crypto_core::Digest>(parts: &[&[u8]]) -> Digest {
    let mut h = D::new();
    for p in parts {
        h.update(p);
    }
    let mut o = [0u8; 64];
    h.finalize_into(&mut o);
    Digest::new(&o[..D::OUTPUT_LEN])
}

fn hmac_parts<D: crypto_core::Digest>(key: &[u8], parts: &[&[u8]]) -> Digest {
    let mut m = Hmac::<D>::new(key);
    for p in parts {
        m.update(p);
    }
    let mut o = [0u8; 64];
    m.finalize_into(&mut o);
    let d = Digest::new(&o[..D::OUTPUT_LEN]);
    crypto_core::ct::Zeroize::zeroize(&mut o);
    d
}

fn key32(k: &[u8]) -> Result<[u8; 32], CryptoError> {
    k.try_into().map_err(|_| CryptoError::BadKey)
}

impl CryptoProvider for CryptoCoreProvider {
    fn random(&self, out: &mut [u8]) -> Result<(), CryptoError> {
        match &mut *self.rng.borrow_mut() {
            Rng::Pool(p, i) => {
                if p.len() - *i < out.len() {
                    return Err(CryptoError::Rng);
                }
                out.copy_from_slice(&p[*i..*i + out.len()]);
                *i += out.len();
                Ok(())
            }
            Rng::Drbg(d) => {
                for chunk in out.chunks_mut(crypto_core::drbg::MAX_REQUEST) {
                    d.fill(chunk).map_err(|_| CryptoError::Rng)?;
                }
                Ok(())
            }
        }
    }

    fn hash(&self, alg: HashAlg, parts: &[&[u8]]) -> Digest {
        match alg {
            HashAlg::Sha256 => hash_parts::<Sha256>(parts),
            HashAlg::Sha384 => hash_parts::<Sha384>(parts),
            HashAlg::Sha512 => hash_parts::<Sha512>(parts),
        }
    }

    fn hmac(&self, alg: HashAlg, key: &[u8], parts: &[&[u8]]) -> Digest {
        match alg {
            HashAlg::Sha256 => hmac_parts::<Sha256>(key, parts),
            HashAlg::Sha384 => hmac_parts::<Sha384>(key, parts),
            HashAlg::Sha512 => hmac_parts::<Sha512>(key, parts),
        }
    }

    fn aead_seal(&self, alg: AeadAlg, key: &[u8], nonce: &[u8; 12], aad: &[u8], in_out: &mut Vec<u8>) -> Result<(), CryptoError> {
        if key.len() != alg.key_len() {
            return Err(CryptoError::BadKey);
        }
        let tag = match alg {
            AeadAlg::ChaCha20Poly1305 => chacha20poly1305::seal_in_place(&key32(key)?, nonce, aad, in_out),
            AeadAlg::Aes128Gcm | AeadAlg::Aes256Gcm => {
                let c = gcm::AesGcm::new(key).map_err(|_| CryptoError::BadKey)?;
                c.encrypt_in_place_detached(nonce, aad, in_out)
            }
        }
        .map_err(|_| CryptoError::Internal("aead seal: length"))?;
        in_out.extend_from_slice(&tag);
        Ok(())
    }

    fn aead_open(&self, alg: AeadAlg, key: &[u8], nonce: &[u8; 12], aad: &[u8], in_out: &mut Vec<u8>) -> Result<(), CryptoError> {
        if key.len() != alg.key_len() {
            return Err(CryptoError::BadKey);
        }
        if in_out.len() < AeadAlg::TAG_LEN {
            return Err(CryptoError::DecryptFailed);
        }
        let ct_len = in_out.len() - AeadAlg::TAG_LEN;
        let mut tag = [0u8; 16];
        tag.copy_from_slice(&in_out[ct_len..]);
        in_out.truncate(ct_len);
        let r = match alg {
            AeadAlg::ChaCha20Poly1305 => chacha20poly1305::open_in_place(&key32(key)?, nonce, aad, in_out, &tag),
            AeadAlg::Aes128Gcm | AeadAlg::Aes256Gcm => {
                let c = gcm::AesGcm::new(key).map_err(|_| CryptoError::BadKey)?;
                c.decrypt_in_place_detached(nonce, aad, in_out, &tag)
            }
        };
        if r.is_err() {
            // Reveal no unauthenticated plaintext.
            crypto_core::ct::Zeroize::zeroize(&mut in_out[..]);
            in_out.clear();
            return Err(CryptoError::DecryptFailed);
        }
        Ok(())
    }

    fn x25519_keypair(&self) -> Result<(KxPrivate, [u8; 32]), CryptoError> {
        let mut sk = [0u8; 32];
        self.random(&mut sk)?;
        let public = x25519::public_key(&sk);
        let k = KxPrivate { bytes: sk.to_vec() };
        crypto_core::ct::Zeroize::zeroize(&mut sk);
        Ok((k, public))
    }

    fn x25519_shared(&self, private: &KxPrivate, peer_public: &[u8]) -> Result<[u8; 32], CryptoError> {
        let mut sk = key32(&private.bytes)?;
        let pk = key32(peer_public)?;
        let r = x25519::diffie_hellman(&sk, &pk).map_err(|_| CryptoError::BadKey);
        crypto_core::ct::Zeroize::zeroize(&mut sk);
        r
    }

    fn p256_keypair(&self) -> Result<(KxPrivate, Vec<u8>), CryptoError> {
        // Rejection sampling (FIPS 186-5 A.4.2): a draw of 0 or >= n is discarded (probability ~2^-32).
        for _ in 0..8 {
            let mut sk = [0u8; 32];
            self.random(&mut sk)?;
            if let Ok(secret) = p256::SecretKey::from_bytes(&sk) {
                let public = secret.public_key().to_sec1_uncompressed().to_vec();
                let k = KxPrivate { bytes: sk.to_vec() };
                crypto_core::ct::Zeroize::zeroize(&mut sk);
                return Ok((k, public));
            }
        }
        Err(CryptoError::Rng)
    }

    fn p256_ecdh(&self, private: &KxPrivate, peer_public: &[u8]) -> Result<[u8; 32], CryptoError> {
        let mut sk = key32(&private.bytes)?;
        let secret = p256::SecretKey::from_bytes(&sk).map_err(|_| CryptoError::BadKey);
        crypto_core::ct::Zeroize::zeroize(&mut sk);
        let peer = p256::PublicKey::from_sec1(peer_public).map_err(|_| CryptoError::BadKey)?;
        secret?.diffie_hellman(&peer).map_err(|_| CryptoError::BadKey)
    }

    fn ecdsa_verify(&self, curve: EcCurve, hash: HashAlg, public_key: &[u8], msg: &[u8], sig_der: &[u8]) -> Result<(), CryptoError> {
        match curve {
            EcCurve::P256 => {
                let key = p256::PublicKey::from_sec1(public_key).map_err(|_| CryptoError::BadKey)?;
                let sig = p256::signature_from_der(sig_der).map_err(|_| CryptoError::BadSignature)?;
                let digest = self.hash(hash, &[msg]);
                p256::verify_prehashed(&key, digest.as_bytes(), &sig).map_err(|_| CryptoError::BadSignature)
            }
            EcCurve::P384 => Err(CryptoError::Unsupported("ECDSA P-384 verify: CRYPTOCORE has no P-384 yet (owed)")),
        }
    }

    fn ed25519_verify(&self, public_key: &[u8], msg: &[u8], sig: &[u8]) -> Result<(), CryptoError> {
        let pk: [u8; 32] = public_key.try_into().map_err(|_| CryptoError::BadKey)?;
        let s: [u8; 64] = sig.try_into().map_err(|_| CryptoError::BadSignature)?;
        match ed25519::verify(&pk, msg, &s) {
            Ok(()) => Ok(()),
            Err(crypto_core::Error::Encoding) if ed25519_point_bad(&pk) => Err(CryptoError::BadKey),
            Err(_) => Err(CryptoError::BadSignature),
        }
    }
}

/// Whether the public key itself fails to decode (BadKey) as opposed to the signature (BadSignature).
fn ed25519_point_bad(pk: &[u8; 32]) -> bool {
    // A valid key verifies-or-fails on the signature; probe with an all-zero (S = 0 < L) signature so
    // only a point-decoding failure can surface as `Encoding`.
    matches!(ed25519::verify(pk, b"", &[0u8; 64]), Err(crypto_core::Error::Encoding))
}
