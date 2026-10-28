// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PDF-KEYSTORE (LEDGER SR382): signing keys Holocron holds — ns [`crate::service::SIGN_NS`], kind
//! [`crate::service::KIND_PKCS8`] — read by tls_core's ONE private-key parser ([`tls_core::key::parse`]: PKCS#8 v1 / v2,
//! PKCS#1, SEC 1, PEM or DER) at put time (a file that does not parse is never stored) and again inside the fulfiller at
//! each Sign: the key never leaves Holocron, only the signature does.
//!
//! [`sign`] answers Sign's long form ([`crate::wire::Request::SignWith`]) through tls_core's ONE signer
//! ([`tls_core::key::sign`], KEYSIGNONE LEDGER SR393): RSASSA-PKCS1-v1_5 or RSASSA-PSS (MGF1 with the same hash, an hLen
//! salt from the service's entropy) and ECDSA P-256 (RFC 6979, DER Ecdsa-Sig-Value) over the caller's DIGEST, Ed25519
//! (pure) over the caller's MESSAGE; each refusal named on the wire ([`status::WRONG_KIND`], [`status::BAD_DIGEST_LEN`],
//! [`status::UNSUPPORTED_SCHEME`]; `INVALID` the catch-all). Feature `pkcs8` (the only module that links tls_core).

use alloc::vec::Vec;

use tls_core::key::{HashAlg, Salt, Scheme};

use crate::seal::Entropy;
use crate::wire::{self, key_kind, sign_alg, status};

pub use tls_core::key::{Key, KeyError};

/// Parse a key file the way Sign will: `Ok(kind)` (`rsa`, `p256`, `ed25519`), else tls_core's refusal by name.
pub fn check(file: &[u8]) -> Result<&'static str, KeyError> {
    Ok(tls_core::key::parse(file)?.kind())
}

/// Sign `tbs` with the key `file` under `alg` ([`sign_alg`]); the long-form reply body ([`wire::encode_sign`]) or the
/// status: `CORRUPT` when the stored file no longer parses, `UNSUPPORTED_SCHEME` for an `alg` byte no scheme has,
/// `WRONG_KIND` for a scheme the key cannot make, `BAD_DIGEST_LEN` for a digest that is not the hash's length, `IO`
/// when the entropy source fails, `INVALID` otherwise.
pub fn sign<E: Entropy>(file: &[u8], alg: u8, tbs: &[u8], rng: &mut E) -> Result<Vec<u8>, i32> {
    let key = tls_core::key::parse(file).map_err(|_| status::CORRUPT)?;
    let kind = match key {
        Key::Rsa(_) => key_kind::RSA,
        Key::P256(_) => key_kind::P256,
        Key::Ed25519(_) => key_kind::ED25519,
    };
    let scheme = scheme(&key, alg)?;
    let mut fill = |b: &mut [u8]| rng.fill(b).map_err(|_| ());
    let sig = tls_core::key::sign(&key, scheme, tbs, Salt::Random(&mut fill)).map_err(|e| reason(&e))?;
    Ok(wire::encode_sign(kind, &sig))
}

/// The scheme an `alg` byte asks of `key`: no hash = Ed25519, PSS = RSASSA-PSS, a hash = PKCS#1 v1.5 for an RSA key and
/// ECDSA P-256 otherwise (tls_core's signer refuses the scheme a key cannot make); an unknown bit, an unknown hash or
/// PSS without a hash is `UNSUPPORTED_SCHEME`.
fn scheme(key: &Key, alg: u8) -> Result<Scheme, i32> {
    if alg & !(sign_alg::HASH_MASK | sign_alg::PSS) != 0 {
        return Err(status::UNSUPPORTED_SCHEME);
    }
    let pss = alg & sign_alg::PSS != 0;
    let hash = match alg & sign_alg::HASH_MASK {
        sign_alg::NONE if !pss => return Ok(Scheme::Ed25519),
        sign_alg::SHA256 => HashAlg::Sha256,
        sign_alg::SHA384 => HashAlg::Sha384,
        sign_alg::SHA512 => HashAlg::Sha512,
        _ => return Err(status::UNSUPPORTED_SCHEME),
    };
    Ok(match key {
        _ if pss => Scheme::Pss(hash),
        Key::Rsa(_) => Scheme::Pkcs1(hash),
        _ => Scheme::P256(hash),
    })
}

/// tls_core's refusal as the wire's reason.
fn reason(e: &KeyError) -> i32 {
    match e {
        KeyError::Refused(w) if *w == tls_core::key::WRONG_KIND => status::WRONG_KIND,
        KeyError::Refused(w) if *w == tls_core::key::DIGEST_LEN => status::BAD_DIGEST_LEN,
        KeyError::Refused(w) if *w == tls_core::key::SALT_SOURCE => status::IO,
        _ => status::INVALID,
    }
}
