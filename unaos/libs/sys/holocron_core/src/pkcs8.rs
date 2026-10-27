// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PDF-KEYSTORE (LEDGER SR382): signing keys Holocron holds — ns [`crate::service::SIGN_NS`], kind
//! [`crate::service::KIND_PKCS8`] — read by tls_core's ONE private-key parser ([`tls_core::key::parse`]: PKCS#8 v1 / v2,
//! PKCS#1, SEC 1, PEM or DER) at put time (a file that does not parse is never stored) and again inside the fulfiller at
//! each Sign: the key never leaves Holocron, only the signature does.
//!
//! [`sign`] answers Sign's long form ([`crate::wire::Request::SignWith`]) over crypto_core's signers: RSASSA-PKCS1-v1_5
//! or RSASSA-PSS (MGF1 with the same hash, an hLen salt from the service's entropy) and ECDSA P-256 (RFC 6979, DER
//! Ecdsa-Sig-Value) over the caller's DIGEST, Ed25519 (pure) over the caller's MESSAGE. Feature `pkcs8` (the only
//! module that links tls_core).

use alloc::vec;
use alloc::vec::Vec;

use crypto_core::{Sha256, Sha384, Sha512};
use crypto_core::p256;

use crate::seal::Entropy;
use crate::wire::{self, key_kind, sign_alg, status};

pub use tls_core::key::{Key, KeyError};

/// Parse a key file the way Sign will: `Ok(kind)` (`rsa`, `p256`, `ed25519`), else tls_core's refusal by name.
pub fn check(file: &[u8]) -> Result<&'static str, KeyError> {
    Ok(tls_core::key::parse(file)?.kind())
}

/// Sign `tbs` with the key `file` under `alg` ([`sign_alg`]); the long-form reply body ([`wire::encode_sign`]) or the
/// status: `CORRUPT` when the stored file no longer parses, `INVALID` for an algorithm the key cannot do or a digest of
/// the wrong length, `IO` when the entropy source fails.
pub fn sign<E: Entropy>(file: &[u8], alg: u8, tbs: &[u8], rng: &mut E) -> Result<Vec<u8>, i32> {
    let key = tls_core::key::parse(file).map_err(|_| status::CORRUPT)?;
    if alg & !(sign_alg::HASH_MASK | sign_alg::PSS) != 0 {
        return Err(status::INVALID);
    }
    let hash = alg & sign_alg::HASH_MASK;
    let pss = alg & sign_alg::PSS != 0;
    let hlen = match hash {
        sign_alg::NONE => 0,
        sign_alg::SHA256 => 32,
        sign_alg::SHA384 => 48,
        sign_alg::SHA512 => 64,
        _ => return Err(status::INVALID),
    };
    match key {
        Key::Ed25519(k) => {
            if hash != sign_alg::NONE || pss {
                return Err(status::INVALID);
            }
            Ok(wire::encode_sign(key_kind::ED25519, &k.sign(tbs)))
        }
        Key::P256(k) => {
            if hlen == 0 || pss || tbs.len() != hlen {
                return Err(status::INVALID);
            }
            let raw = k.sign_prehashed(tbs);
            let mut der = [0u8; 72];
            let n = p256::signature_to_der(&raw, &mut der);
            Ok(wire::encode_sign(key_kind::P256, &der[..n]))
        }
        Key::Rsa(k) => {
            if hlen == 0 || tbs.len() != hlen {
                return Err(status::INVALID);
            }
            let mut out = vec![0u8; k.public_key().size()];
            let r = if pss {
                let mut salt = vec![0u8; hlen];
                rng.fill(&mut salt).map_err(|_| status::IO)?;
                match hash {
                    sign_alg::SHA256 => k.sign_pss::<Sha256>(tbs, &salt, &mut out),
                    sign_alg::SHA384 => k.sign_pss::<Sha384>(tbs, &salt, &mut out),
                    _ => k.sign_pss::<Sha512>(tbs, &salt, &mut out),
                }
            } else {
                k.sign_pkcs1v15(tbs, &mut out)
            };
            let n = r.map_err(|_| status::INVALID)?;
            Ok(wire::encode_sign(key_kind::RSA, &out[..n]))
        }
    }
}
