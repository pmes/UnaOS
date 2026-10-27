// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: tls_core — shared-core. PDFSIGNKEYPARSE (LEDGER SR361): the ONE private-key file parser, over this crate's
//! strict DER reader ([`crate::x509::der::Der`]), its PEM decoder ([`crate::x509::pem::pem_blocks`]) and its DER writer
//! ([`crate::x509::build`]) — no second reader, writer or base64.
//!
//! Two layers:
//! * the key CONTAINERS (always built; byte OIDs, borrowed slices): SubjectPublicKeyInfo (RFC 5280 §4.1.2.7),
//!   PrivateKeyInfo / OneAsymmetricKey v1 and v2 (RFC 5208 §5 / RFC 5958), RSAPublicKey and RSAPrivateKey (RFC 8017
//!   A.1), ECPrivateKey (RFC 5915 §3) and the CurvePrivateKey OCTET STRING (RFC 8410 §7), each with its writer. Aether's
//!   WebCrypto key formats (`handlers/aether/src/api/der.rs`) are adapters over these (the ASN.1 it once carried itself).
//! * [`parse`] (feature `cryptocore`): a key file, PEM (RFC 7468: `PRIVATE KEY`, `RSA PRIVATE KEY`, `EC PRIVATE KEY`) or
//!   DER, to a crypto_core key: RSA ([`PrivateKey`]), P-256 ([`SecretKey`]) or Ed25519 ([`SigningKey`]). Refused by
//!   name: encrypted PKCS#8 and encrypted PEM, P-384, P-521 and other curves, RSASSA-PSS-keyed RSA, multi-prime RSA,
//!   components that do not describe one key, a carried public key that is not the private key's, several keys.
//!
//! The reader is strict DER because key material is attacker-supplied. Decoded key bytes are wiped once parsed.

use alloc::vec;
use alloc::vec::Vec;

use crate::error::CertError;
use crate::x509::build;
use crate::x509::der::{Der, tag};
use crate::x509::oid;

/// secp521r1 (SEC 2 §2.6.1), named only to refuse it.
const SECP521R1: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x23];

/// Why a key container or key file was not read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyError {
    /// The DER is malformed (the strict reader's reason).
    Der(CertError),
    /// Well-formed but refused, by name.
    Refused(&'static str),
}

impl From<CertError> for KeyError {
    fn from(e: CertError) -> Self {
        KeyError::Der(e)
    }
}

impl KeyError {
    /// The reason as one short phrase.
    pub fn why(&self) -> &'static str {
        match self {
            KeyError::Refused(w) => w,
            KeyError::Der(CertError::BadDer(m)) => m,
            KeyError::Der(_) => "DER",
        }
    }
}

fn bad(m: &'static str) -> KeyError {
    KeyError::Der(CertError::BadDer(m))
}

/// An AlgorithmIdentifier's parameters: none, NULL, a named OID (a curve), or anything else (explicit curve
/// parameters, RSASSA-PSS-params, ...).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AlgParams<'a> {
    Absent,
    Null,
    Oid(&'a [u8]),
    Other,
}

/// A PrivateKeyInfo / OneAsymmetricKey (RFC 5958).
#[derive(Clone, Copy, Debug)]
pub struct Pkcs8<'a> {
    /// 0 (v1) or 1 (v2).
    pub version: u64,
    pub alg: &'a [u8],
    pub params: AlgParams<'a>,
    /// The privateKey OCTET STRING's contents.
    pub key: &'a [u8],
    /// The v2 `[1] publicKey` BIT STRING's contents (its unused-bits octet first), if present.
    pub public: Option<&'a [u8]>,
}

/// An ECPrivateKey (RFC 5915).
#[derive(Clone, Copy, Debug)]
pub struct EcPrivate<'a> {
    pub d: &'a [u8],
    /// `[0]` parameters: the named curve OID.
    pub curve: Option<&'a [u8]>,
    /// `[1]` publicKey: the SEC 1 point.
    pub public: Option<&'a [u8]>,
}

/// The single top-level SEQUENCE's contents (nothing may trail it).
fn outer(der: &[u8]) -> Result<Der<'_>, KeyError> {
    let mut r = Der::new(der);
    let s = r.sequence()?;
    r.expect_end()?;
    Ok(s)
}

fn alg_id<'a>(d: &mut Der<'a>) -> Result<(&'a [u8], AlgParams<'a>), KeyError> {
    let mut a = d.sequence()?;
    let o = a.oid()?;
    let params = if a.is_empty() {
        AlgParams::Absent
    } else {
        let t = a.tlv()?;
        match t.tag {
            tag::NULL if t.value.is_empty() => AlgParams::Null,
            tag::OID => AlgParams::Oid(t.value),
            _ => AlgParams::Other,
        }
    };
    a.expect_end()?;
    Ok((o, params))
}

/// SubjectPublicKeyInfo -> (algorithm OID, parameters, subjectPublicKey octets).
pub fn spki(der: &[u8]) -> Result<(&[u8], AlgParams<'_>, &[u8]), KeyError> {
    let mut s = outer(der)?;
    let (o, p) = alg_id(&mut s)?;
    let key = s.bit_string_bytes()?;
    s.expect_end()?;
    Ok((o, p, key))
}

/// PrivateKeyInfo / OneAsymmetricKey, version 0 or 1 (attributes `[0]` skipped, the v2 public key kept).
pub fn pkcs8(der: &[u8]) -> Result<Pkcs8<'_>, KeyError> {
    let mut s = outer(der)?;
    let version = s.small_uint()?;
    if version > 1 {
        return Err(bad("PKCS#8 version"));
    }
    let (alg, params) = alg_id(&mut s)?;
    let key = s.expect(tag::OCTET_STRING)?.value;
    let mut public = None;
    while !s.is_empty() {
        let t = s.tlv()?;
        match t.tag {
            0x81 => public = Some(t.value),
            0xa0 | 0xa1 => {}
            _ => return Err(bad("PKCS#8 trailing element")),
        }
    }
    Ok(Pkcs8 { version, alg, params, key, public })
}

/// RSAPublicKey -> [n, e].
pub fn rsa_public(der: &[u8]) -> Result<[&[u8]; 2], KeyError> {
    let mut s = outer(der)?;
    let n = s.uint()?;
    let e = s.uint()?;
    s.expect_end()?;
    Ok([n, e])
}

/// RSAPrivateKey (two-prime, version 0) -> [n, e, d, p, q, dP, dQ, qInv]; version 1 (multi-prime) refused by name.
pub fn rsa_private(der: &[u8]) -> Result<[&[u8]; 8], KeyError> {
    let mut s = outer(der)?;
    match s.small_uint()? {
        0 => {}
        1 => return Err(KeyError::Refused("multi-prime RSA")),
        _ => return Err(bad("RSAPrivateKey version")),
    }
    let mut out: [&[u8]; 8] = [&[]; 8];
    for x in out.iter_mut() {
        *x = s.uint()?;
    }
    s.expect_end()?;
    Ok(out)
}

/// ECPrivateKey (version 1).
pub fn ec_private(der: &[u8]) -> Result<EcPrivate<'_>, KeyError> {
    let mut s = outer(der)?;
    if s.small_uint()? != 1 {
        return Err(bad("ECPrivateKey version"));
    }
    let d = s.expect(tag::OCTET_STRING)?.value;
    let curve = match s.optional(tag::context_constructed(0))? {
        Some(t) => {
            let mut i = Der::new(t.value);
            let o = i.oid()?;
            i.expect_end()?;
            Some(o)
        }
        None => None,
    };
    let public = match s.optional(tag::context_constructed(1))? {
        Some(t) => {
            let mut i = Der::new(t.value);
            let b = i.bit_string_bytes()?;
            i.expect_end()?;
            Some(b)
        }
        None => None,
    };
    s.expect_end()?;
    Ok(EcPrivate { d, curve, public })
}

/// CurvePrivateKey (RFC 8410 §7): an OCTET STRING.
pub fn curve_private(der: &[u8]) -> Result<&[u8], KeyError> {
    let mut r = Der::new(der);
    let v = r.expect(tag::OCTET_STRING)?.value;
    r.expect_end()?;
    Ok(v)
}

fn alg_build(alg: &[u8], params: &AlgParams<'_>) -> Vec<u8> {
    match params {
        AlgParams::Null => build::seq(&[&build::oid(alg), &build::null()]),
        AlgParams::Oid(o) => build::seq(&[&build::oid(alg), &build::oid(o)]),
        _ => build::seq(&[&build::oid(alg)]),
    }
}

/// SubjectPublicKeyInfo from an algorithm OID, its parameters and the key octets.
pub fn spki_build(alg: &[u8], params: &AlgParams<'_>, key: &[u8]) -> Vec<u8> {
    build::seq(&[&alg_build(alg, params), &build::bits(key)])
}

/// PrivateKeyInfo (version 0) from an algorithm OID, its parameters and the privateKey octets.
pub fn pkcs8_build(alg: &[u8], params: &AlgParams<'_>, key: &[u8]) -> Vec<u8> {
    build::seq(&[&build::int(&[0]), &alg_build(alg, params), &build::octets(key)])
}

/// RSAPublicKey from big-endian n and e.
pub fn rsa_public_build(n: &[u8], e: &[u8]) -> Vec<u8> {
    build::seq(&[&build::int(n), &build::int(e)])
}

/// RSAPrivateKey (version 0) from big-endian components [n, e, d, p, q, dP, dQ, qInv].
pub fn rsa_private_build(parts: &[&[u8]]) -> Vec<u8> {
    let mut items = vec![build::int(&[0])];
    items.extend(parts.iter().map(|p| build::int(p)));
    let refs: Vec<&[u8]> = items.iter().map(Vec::as_slice).collect();
    build::seq(&refs)
}

/// ECPrivateKey with the public key in `[1]` and no `[0]` (the curve is in the PrivateKeyInfo).
pub fn ec_private_build(d: &[u8], public: &[u8]) -> Vec<u8> {
    build::seq(&[&build::int(&[1]), &build::octets(d), &build::explicit(1, &build::bits(public))])
}

#[cfg(feature = "cryptocore")]
pub use self::keys::{Key, parse};
#[cfg(feature = "cryptocore")]
pub use crypto_core::{ed25519::SigningKey, p256::SecretKey, rsa_private::PrivateKey};

#[cfg(feature = "cryptocore")]
mod keys {
    use super::*;
    use crate::x509::pem;
    use crypto_core::ct::Zeroize;
    use crypto_core::ed25519::SigningKey;
    use crypto_core::p256::{self, SecretKey};
    use crypto_core::rsa_private::{Part, PrivateKey};

    /// A private key read from a key file. The crypto_core types wipe themselves on drop.
    pub enum Key {
        Rsa(PrivateKey),
        P256(SecretKey),
        Ed25519(SigningKey),
    }

    impl Key {
        /// `rsa`, `p256` or `ed25519`.
        pub fn kind(&self) -> &'static str {
            match self {
                Key::Rsa(_) => "rsa",
                Key::P256(_) => "p256",
                Key::Ed25519(_) => "ed25519",
            }
        }
    }

    /// Decoded key bytes, wiped on drop.
    struct Secret(Vec<u8>);

    impl Drop for Secret {
        fn drop(&mut self) {
            self.0.zeroize();
        }
    }

    #[derive(Clone, Copy)]
    enum Form {
        Pkcs8,
        Pkcs1,
        Sec1,
    }

    const LABELS: [(&str, Form); 3] = [("PRIVATE KEY", Form::Pkcs8), ("RSA PRIVATE KEY", Form::Pkcs1), ("EC PRIVATE KEY", Form::Sec1)];

    /// A key file, PEM or DER, to its key (see the module comment for what is read and what is refused by name).
    pub fn parse(bytes: &[u8]) -> Result<Key, KeyError> {
        let Some(text) = core::str::from_utf8(bytes).ok().filter(|t| t.contains("-----BEGIN ")) else {
            return from_der(bytes);
        };
        if text.contains("-----BEGIN ENCRYPTED PRIVATE KEY-----") {
            return Err(KeyError::Refused("encrypted PKCS#8"));
        }
        if text.contains("Proc-Type: 4,ENCRYPTED") {
            return Err(KeyError::Refused("encrypted PEM"));
        }
        let mut found: Option<(Secret, Form)> = None;
        for (label, form) in LABELS {
            let (blocks, malformed) = pem::pem_blocks(text, label);
            let blocks: Vec<Secret> = blocks.into_iter().map(Secret).collect();
            if malformed > 0 {
                return Err(KeyError::Refused("PEM"));
            }
            for b in blocks {
                if found.is_some() {
                    return Err(KeyError::Refused("key count"));
                }
                found = Some((b, form));
            }
        }
        let (der, form) = found.ok_or(KeyError::Refused("no private key"))?;
        match form {
            Form::Pkcs8 => from_pkcs8(&der.0),
            Form::Pkcs1 => Ok(Key::Rsa(rsa_key(&der.0)?)),
            Form::Sec1 => from_sec1(&der.0, None, None),
        }
    }

    /// DER classified by its shape: PKCS#8 {0|1, SEQUENCE}, PKCS#1 {0, INTEGER}, SEC 1 {1, OCTET STRING},
    /// EncryptedPrivateKeyInfo {SEQUENCE, OCTET STRING}.
    fn from_der(der: &[u8]) -> Result<Key, KeyError> {
        let mut s = outer(der)?;
        let first = s.tlv()?;
        match (first.tag, s.peek_tag()) {
            (tag::SEQUENCE, Some(tag::OCTET_STRING)) => Err(KeyError::Refused("encrypted PKCS#8")),
            (tag::INTEGER, Some(tag::SEQUENCE)) => from_pkcs8(der),
            (tag::INTEGER, Some(tag::INTEGER)) => Ok(Key::Rsa(rsa_key(der)?)),
            (tag::INTEGER, Some(tag::OCTET_STRING)) => from_sec1(der, None, None),
            _ => Err(KeyError::Refused("not a private key")),
        }
    }

    fn from_pkcs8(der: &[u8]) -> Result<Key, KeyError> {
        let p = pkcs8(der)?;
        let public = match p.public {
            None => None,
            Some([0, rest @ ..]) => Some(rest),
            Some(_) => return Err(bad("BIT STRING with unused bits")),
        };
        match p.alg {
            a if a == oid::RSA_ENCRYPTION => {
                if !matches!(p.params, AlgParams::Null | AlgParams::Absent) {
                    return Err(KeyError::Refused("RSA parameters"));
                }
                let k = rsa_key(p.key)?;
                if let Some(pk) = public {
                    let [n, e] = rsa_public(pk)?;
                    let mut buf = vec![0u8; k.public_key().size()];
                    let ln = k.part_be(Part::N, &mut buf);
                    let same_n = &buf[..ln] == n;
                    let le = k.part_be(Part::E, &mut buf);
                    let same_e = &buf[..le] == e;
                    if !(same_n && same_e) {
                        return Err(KeyError::Refused("public key mismatch"));
                    }
                }
                Ok(Key::Rsa(k))
            }
            a if a == oid::RSASSA_PSS => Err(KeyError::Refused("RSASSA-PSS key")),
            a if a == oid::EC_PUBLIC_KEY => {
                let AlgParams::Oid(curve) = p.params else { return Err(KeyError::Refused("curve")) };
                curve_ok(curve)?;
                from_sec1(p.key, Some(curve), public)
            }
            a if a == oid::ED25519 => {
                if p.params != AlgParams::Absent {
                    return Err(bad("Ed25519 parameters"));
                }
                let raw = curve_private(p.key)?;
                if raw.len() != 32 {
                    return Err(KeyError::Refused("Ed25519 key length"));
                }
                let mut seed = [0u8; 32];
                seed.copy_from_slice(raw);
                let k = SigningKey::from_seed(&seed);
                seed.zeroize();
                if public.is_some_and(|pk| pk != &k.public_key()[..]) {
                    return Err(KeyError::Refused("public key mismatch"));
                }
                Ok(Key::Ed25519(k))
            }
            _ => Err(KeyError::Refused("algorithm")),
        }
    }

    /// prime256v1 only; the others by name.
    fn curve_ok(curve: &[u8]) -> Result<(), KeyError> {
        match curve {
            c if c == oid::PRIME256V1 => Ok(()),
            c if c == oid::SECP384R1 => Err(KeyError::Refused("P-384")),
            c if c == SECP521R1 => Err(KeyError::Refused("P-521")),
            _ => Err(KeyError::Refused("curve")),
        }
    }

    /// An ECPrivateKey; `outer` / `outer_public` are a PrivateKeyInfo's curve and v2 public key.
    fn from_sec1(der: &[u8], outer: Option<&[u8]>, outer_public: Option<&[u8]>) -> Result<Key, KeyError> {
        let e = ec_private(der)?;
        let curve = match (outer, e.curve) {
            (Some(a), Some(b)) if a != b => return Err(KeyError::Refused("curve mismatch")),
            (Some(a), _) => a,
            (None, Some(b)) => b,
            (None, None) => return Err(KeyError::Refused("curve")),
        };
        curve_ok(curve)?;
        if e.d.is_empty() || e.d.len() > 32 {
            return Err(KeyError::Refused("P-256 key length"));
        }
        let mut d = [0u8; 32];
        d[32 - e.d.len()..].copy_from_slice(e.d);
        let k = SecretKey::from_bytes(&d);
        d.zeroize();
        let k = k.map_err(|_| KeyError::Refused("P-256 scalar"))?;
        let mine = k.public_key().to_sec1_uncompressed();
        for pk in [e.public, outer_public].into_iter().flatten() {
            let theirs = p256::PublicKey::from_sec1(pk).map_err(|_| KeyError::Refused("public key"))?;
            if theirs.to_sec1_uncompressed() != mine {
                return Err(KeyError::Refused("public key mismatch"));
            }
        }
        Ok(Key::P256(k))
    }

    /// An RSAPrivateKey through crypto_core's validating constructor.
    fn rsa_key(der: &[u8]) -> Result<PrivateKey, KeyError> {
        let [n, e, d, p, q, dp, dq, qi] = rsa_private(der)?;
        PrivateKey::from_components(n, e, d, p, q, dp, dq, qi).map_err(|_| KeyError::Refused("RSA components"))
    }
}
