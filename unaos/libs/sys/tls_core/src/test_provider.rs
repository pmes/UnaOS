//! TEST OPERAND — a `CryptoProvider` over the latest RustCrypto crates (feature `test-provider`).
//!
//! It exists only so tls_core's state machine, key schedule and X.509 validation are proven against the RFC 8448
//! traces and a live OpenSSL server before CRYPTOCORE lands. It is never linked into a product binary. Its RNG can
//! be replaced by a fixed byte pool so a trace's client random and key shares are reproduced exactly.

use alloc::vec::Vec;
use core::cell::RefCell;

use hmac::{KeyInit, Mac};

use crate::crypto::{AeadAlg, CryptoError, CryptoProvider, Digest, EcCurve, HashAlg, KxPrivate};
use crate::msgs::SignatureScheme;

pub struct RustCryptoProvider {
    pool: RefCell<Option<(Vec<u8>, usize)>>,
}

impl Default for RustCryptoProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl RustCryptoProvider {
    /// OS randomness (getrandom).
    pub fn new() -> Self {
        RustCryptoProvider { pool: RefCell::new(None) }
    }
    /// Deterministic: `random` hands out these bytes in order and fails when they run out.
    pub fn with_rng_pool(pool: Vec<u8>) -> Self {
        RustCryptoProvider { pool: RefCell::new(Some((pool, 0))) }
    }
    /// Bytes of the pool not yet consumed.
    pub fn pool_remaining(&self) -> usize {
        self.pool.borrow().as_ref().map_or(0, |(p, i)| p.len() - i)
    }
}

fn hmac_parts<M: Mac + KeyInit>(key: &[u8], parts: &[&[u8]]) -> Digest {
    let mut m = <M as KeyInit>::new_from_slice(key).expect("HMAC takes any key length");
    for p in parts {
        m.update(p);
    }
    Digest::new(&m.finalize().into_bytes())
}

fn hash_parts<D: sha2::Digest>(parts: &[&[u8]]) -> Digest {
    let mut h = D::new();
    for p in parts {
        h.update(p);
    }
    Digest::new(&h.finalize())
}

impl CryptoProvider for RustCryptoProvider {
    fn random(&self, out: &mut [u8]) -> Result<(), CryptoError> {
        let mut pool = self.pool.borrow_mut();
        match pool.as_mut() {
            Some((p, i)) => {
                if p.len() - *i < out.len() {
                    return Err(CryptoError::Rng);
                }
                out.copy_from_slice(&p[*i..*i + out.len()]);
                *i += out.len();
                Ok(())
            }
            None => getrandom::fill(out).map_err(|_| CryptoError::Rng),
        }
    }

    fn hash(&self, alg: HashAlg, parts: &[&[u8]]) -> Digest {
        match alg {
            HashAlg::Sha256 => hash_parts::<sha2::Sha256>(parts),
            HashAlg::Sha384 => hash_parts::<sha2::Sha384>(parts),
            HashAlg::Sha512 => hash_parts::<sha2::Sha512>(parts),
        }
    }

    fn hmac(&self, alg: HashAlg, key: &[u8], parts: &[&[u8]]) -> Digest {
        match alg {
            HashAlg::Sha256 => hmac_parts::<hmac::Hmac<sha2::Sha256>>(key, parts),
            HashAlg::Sha384 => hmac_parts::<hmac::Hmac<sha2::Sha384>>(key, parts),
            HashAlg::Sha512 => hmac_parts::<hmac::Hmac<sha2::Sha512>>(key, parts),
        }
    }

    fn aead_seal(&self, alg: AeadAlg, key: &[u8], nonce: &[u8; 12], aad: &[u8], in_out: &mut Vec<u8>) -> Result<(), CryptoError> {
        use aes_gcm::aead::AeadInOut;
        let bad = |_| CryptoError::BadKey;
        let fail = |_| CryptoError::Internal("aead seal");
        match alg {
            AeadAlg::Aes128Gcm => {
                let c = aes_gcm::Aes128Gcm::new_from_slice(key).map_err(bad)?;
                c.encrypt_in_place(nonce.into(), aad, in_out).map_err(fail)
            }
            AeadAlg::Aes256Gcm => {
                let c = aes_gcm::Aes256Gcm::new_from_slice(key).map_err(bad)?;
                c.encrypt_in_place(nonce.into(), aad, in_out).map_err(fail)
            }
            AeadAlg::ChaCha20Poly1305 => {
                let c = chacha20poly1305::ChaCha20Poly1305::new_from_slice(key).map_err(bad)?;
                c.encrypt_in_place(nonce.into(), aad, in_out).map_err(fail)
            }
        }
    }

    fn aead_open(&self, alg: AeadAlg, key: &[u8], nonce: &[u8; 12], aad: &[u8], in_out: &mut Vec<u8>) -> Result<(), CryptoError> {
        use aes_gcm::aead::AeadInOut;
        let bad = |_| CryptoError::BadKey;
        let fail = |_| CryptoError::DecryptFailed;
        match alg {
            AeadAlg::Aes128Gcm => {
                let c = aes_gcm::Aes128Gcm::new_from_slice(key).map_err(bad)?;
                c.decrypt_in_place(nonce.into(), aad, in_out).map_err(fail)
            }
            AeadAlg::Aes256Gcm => {
                let c = aes_gcm::Aes256Gcm::new_from_slice(key).map_err(bad)?;
                c.decrypt_in_place(nonce.into(), aad, in_out).map_err(fail)
            }
            AeadAlg::ChaCha20Poly1305 => {
                let c = chacha20poly1305::ChaCha20Poly1305::new_from_slice(key).map_err(bad)?;
                c.decrypt_in_place(nonce.into(), aad, in_out).map_err(fail)
            }
        }
    }

    fn x25519_keypair(&self) -> Result<(KxPrivate, [u8; 32]), CryptoError> {
        let mut sk = [0u8; 32];
        self.random(&mut sk)?;
        let secret = x25519_dalek::StaticSecret::from(sk);
        let public = x25519_dalek::PublicKey::from(&secret);
        Ok((KxPrivate { bytes: sk.to_vec() }, public.to_bytes()))
    }

    fn x25519_shared(&self, private: &KxPrivate, peer_public: &[u8]) -> Result<[u8; 32], CryptoError> {
        let sk: [u8; 32] = private.bytes.as_slice().try_into().map_err(|_| CryptoError::BadKey)?;
        let pk: [u8; 32] = peer_public.try_into().map_err(|_| CryptoError::BadKey)?;
        let shared = x25519_dalek::StaticSecret::from(sk).diffie_hellman(&x25519_dalek::PublicKey::from(pk));
        if !shared.was_contributory() {
            return Err(CryptoError::BadKey);
        }
        Ok(shared.to_bytes())
    }

    fn p256_keypair(&self) -> Result<(KxPrivate, Vec<u8>), CryptoError> {
        for _ in 0..8 {
            let mut sk = [0u8; 32];
            self.random(&mut sk)?;
            if let Ok(secret) = p256::SecretKey::from_slice(&sk) {
                use p256::elliptic_curve::sec1::ToSec1Point;
                let public = secret.public_key().to_sec1_point(false).as_bytes().to_vec();
                if public.len() != 65 {
                    return Err(CryptoError::Internal("p256 point encoding"));
                }
                return Ok((KxPrivate { bytes: sk.to_vec() }, public));
            }
        }
        Err(CryptoError::Rng)
    }

    fn p256_ecdh(&self, private: &KxPrivate, peer_public: &[u8]) -> Result<[u8; 32], CryptoError> {
        let secret = p256::SecretKey::from_slice(&private.bytes).map_err(|_| CryptoError::BadKey)?;
        let peer = p256::PublicKey::from_sec1_bytes(peer_public).map_err(|_| CryptoError::BadKey)?;
        let shared = p256::ecdh::diffie_hellman(secret.to_nonzero_scalar(), peer.as_affine());
        let mut out = [0u8; 32];
        out.copy_from_slice(shared.raw_secret_bytes());
        Ok(out)
    }

    fn ecdsa_verify(&self, curve: EcCurve, hash: HashAlg, public_key: &[u8], msg: &[u8], sig_der: &[u8]) -> Result<(), CryptoError> {
        use p256::ecdsa::signature::hazmat::PrehashVerifier;
        let digest = self.hash(hash, &[msg]);
        match curve {
            EcCurve::P256 => {
                let vk = p256::ecdsa::VerifyingKey::from_sec1_bytes(public_key).map_err(|_| CryptoError::BadKey)?;
                let sig = p256::ecdsa::Signature::from_der(sig_der).map_err(|_| CryptoError::BadSignature)?;
                vk.verify_prehash(digest.as_bytes(), &sig).map_err(|_| CryptoError::BadSignature)
            }
            EcCurve::P384 => {
                let vk = p384::ecdsa::VerifyingKey::from_sec1_bytes(public_key).map_err(|_| CryptoError::BadKey)?;
                let sig = p384::ecdsa::Signature::from_der(sig_der).map_err(|_| CryptoError::BadSignature)?;
                vk.verify_prehash(digest.as_bytes(), &sig).map_err(|_| CryptoError::BadSignature)
            }
        }
    }

    fn ed25519_verify(&self, public_key: &[u8], msg: &[u8], sig: &[u8]) -> Result<(), CryptoError> {
        let pk: [u8; 32] = public_key.try_into().map_err(|_| CryptoError::BadKey)?;
        let vk = ed25519_dalek::VerifyingKey::from_bytes(&pk).map_err(|_| CryptoError::BadKey)?;
        let s = ed25519_dalek::Signature::from_slice(sig).map_err(|_| CryptoError::BadSignature)?;
        vk.verify_strict(msg, &s).map_err(|_| CryptoError::BadSignature)
    }

    fn rsa_pss_verify(&self, hash: HashAlg, n: &[u8], e: &[u8], msg: &[u8], sig: &[u8]) -> Result<(), CryptoError> {
        use rsa::signature::Verifier;
        let key = rsa_key(n, e)?;
        let s = rsa::pss::Signature::try_from(sig).map_err(|_| CryptoError::BadSignature)?;
        let r = match hash {
            HashAlg::Sha256 => rsa::pss::VerifyingKey::<sha2::Sha256>::new(key).verify(msg, &s),
            HashAlg::Sha384 => rsa::pss::VerifyingKey::<sha2::Sha384>::new(key).verify(msg, &s),
            HashAlg::Sha512 => rsa::pss::VerifyingKey::<sha2::Sha512>::new(key).verify(msg, &s),
        };
        r.map_err(|_| CryptoError::BadSignature)
    }

    fn rsa_pkcs1_verify(&self, hash: HashAlg, n: &[u8], e: &[u8], msg: &[u8], sig: &[u8]) -> Result<(), CryptoError> {
        use rsa::signature::Verifier;
        let key = rsa_key(n, e)?;
        let s = rsa::pkcs1v15::Signature::try_from(sig).map_err(|_| CryptoError::BadSignature)?;
        let r = match hash {
            HashAlg::Sha256 => rsa::pkcs1v15::VerifyingKey::<sha2::Sha256>::new(key).verify(msg, &s),
            HashAlg::Sha384 => rsa::pkcs1v15::VerifyingKey::<sha2::Sha384>::new(key).verify(msg, &s),
            HashAlg::Sha512 => rsa::pkcs1v15::VerifyingKey::<sha2::Sha512>::new(key).verify(msg, &s),
        };
        r.map_err(|_| CryptoError::BadSignature)
    }

    fn supports_signature(&self, _scheme: SignatureScheme) -> bool {
        true
    }
}

fn rsa_key(n: &[u8], e: &[u8]) -> Result<rsa::RsaPublicKey, CryptoError> {
    let bits = (n.len() * 8) as u32;
    let n = rsa::BoxedUint::from_be_slice(n, bits).map_err(|_| CryptoError::BadKey)?;
    let e = rsa::BoxedUint::from_be_slice(e, (e.len() * 8).max(64) as u32).map_err(|_| CryptoError::BadKey)?;
    rsa::RsaPublicKey::new(n, e).map_err(|_| CryptoError::BadKey)
}
