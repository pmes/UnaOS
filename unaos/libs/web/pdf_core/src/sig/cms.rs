// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Aether — shared-core. PDFSIGN (LEDGER SR350): the CMS SignedData WRITER (RFC 5652), the twin of the reader
//! in `sig.rs` (its OIDs and `Hash` are the ones written here: one vocabulary). DER goes through tls_core's ONE writer
//! (`tls_core::x509::build`); signatures through crypto_core's signers with the CALLER's key — no key is kept here.
//!
//! [`build`] writes a detached SignedData: digestAlgorithms {the key's hash}, eContentType id-data with no eContent,
//! the certificate bag, and one SignerInfo (v1, issuerAndSerialNumber from the signer's certificate) whose DER-sorted
//! signed attributes are contentType, messageDigest, signingTime and, for CAdES, ESS signing-certificate-v2.
//!
//! PDFSIGNKEYPARSE (LEDGER SR361): [`SignerKey`] signs RSASSA-PKCS1-v1_5, RSASSA-PSS (MGF1 with the same hash, salt
//! the hash's length), ECDSA P-256 (RFC 6979) over SHA-256 / SHA-384 / SHA-512, and Ed25519 (RFC 8419: pure Ed25519
//! over the signed attributes, the digest SHA-512); key files are read by tls_core's ONE parser ([`parse_key`]).

use alloc::vec;
use alloc::vec::Vec;

use crypto_core::ct::Zeroize;
use crypto_core::p256;
use crypto_core::rsa_private::Part;
use crypto_core::{Sha256, Sha384, Sha512};
pub use crypto_core::ed25519::SigningKey;
pub use crypto_core::p256::SecretKey;
pub use crypto_core::rsa_private::PrivateKey;
pub use tls_core::key::{Key, KeyError, parse as parse_key};
use tls_core::x509::build::{self, alg, explicit, octets, oid, seq, set, set_of_content};
use tls_core::x509::der::{encode_tlv, tag};
use tls_core::x509::{Certificate, oid as xoid};

pub use super::Hash;
use super::{ID_CONTENT_TYPE, ID_DATA, ID_MESSAGE_DIGEST, ID_SIGNED_DATA, ID_SIGNING_TIME};
use crate::Error;

/// id-aa-signingCertificateV2 (RFC 5035 §3).
const ID_SIGNING_CERTIFICATE_V2: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x10, 0x02, 0x2f];

/// The caller's signing key (borrowed; never stored) and the hash it signs with.
pub enum SignerKey<'a> {
    /// RSASSA-PKCS1-v1_5.
    Rsa(&'a PrivateKey, Hash),
    /// RSASSA-PSS, MGF1 with the same hash, salt length = the hash's length.
    RsaPss(&'a PrivateKey, Hash),
    /// ECDSA P-256 (RFC 6979 deterministic nonce).
    P256(&'a SecretKey, Hash),
    /// Ed25519 (pure; the CMS digest is SHA-512, RFC 8419).
    Ed25519(&'a SigningKey),
}

impl SignerKey<'_> {
    /// The digest algorithm: the chosen one, SHA-512 for Ed25519.
    pub fn hash(&self) -> Hash {
        match self {
            SignerKey::Rsa(_, h) | SignerKey::RsaPss(_, h) | SignerKey::P256(_, h) => *h,
            SignerKey::Ed25519(_) => Hash::Sha512,
        }
    }

    /// The key's signature over `msg` (hashed with [`Self::hash`] first, except Ed25519): k bytes for RSA, a DER
    /// Ecdsa-Sig-Value for P-256, 64 bytes for Ed25519. (RSA signing of a digest cannot fail for a valid key; an
    /// impossible failure yields an empty signature, which no verifier accepts.)
    pub fn sign(&self, msg: &[u8]) -> Vec<u8> {
        match self {
            SignerKey::Ed25519(k) => k.sign(msg).to_vec(),
            SignerKey::P256(k, h) => {
                let raw = k.sign_prehashed(&h.digest(&[msg]));
                let mut der = [0u8; 72];
                let n = p256::signature_to_der(&raw, &mut der);
                der[..n].to_vec()
            }
            SignerKey::Rsa(k, h) => rsa_out(k, |out| k.sign_pkcs1v15(&h.digest(&[msg]), out)),
            SignerKey::RsaPss(k, h) => {
                let digest = h.digest(&[msg]);
                let salt = pss_salt(k, *h, msg);
                rsa_out(k, |out| match h {
                    Hash::Sha256 => k.sign_pss::<Sha256>(&digest, &salt, out),
                    Hash::Sha384 => k.sign_pss::<Sha384>(&digest, &salt, out),
                    Hash::Sha512 => k.sign_pss::<Sha512>(&digest, &salt, out),
                })
            }
        }
    }

    /// The public key as a SubjectPublicKeyInfo.
    pub fn spki(&self) -> Vec<u8> {
        match self {
            SignerKey::Rsa(k, _) | SignerKey::RsaPss(k, _) => {
                let p = k.public_key();
                let mut n = vec![0u8; p.size()];
                let mut e = vec![0u8; p.size()];
                let nl = p.n_be(&mut n);
                let el = p.e_be(&mut e);
                build::spki_rsa(&n[..nl], &e[..el])
            }
            SignerKey::P256(k, _) => build::spki_p256(&k.public_key().to_sec1_uncompressed()),
            SignerKey::Ed25519(k) => seq(&[&alg(xoid::ED25519, false), &build::bits(&k.public_key())]),
        }
    }

    /// The AlgorithmIdentifier of a certificate this key signs: sha{256,384,512}WithRSAEncryption, RSASSA-PSS with
    /// its parameters, ecdsa-with-SHA{256,384,512}, id-Ed25519.
    pub fn cert_alg(&self) -> Vec<u8> {
        match self {
            SignerKey::Rsa(_, h) => alg(
                match h {
                    Hash::Sha256 => xoid::SHA256_WITH_RSA,
                    Hash::Sha384 => xoid::SHA384_WITH_RSA,
                    Hash::Sha512 => xoid::SHA512_WITH_RSA,
                },
                true,
            ),
            SignerKey::RsaPss(_, h) => pss_alg(*h),
            SignerKey::P256(_, h) => alg(
                match h {
                    Hash::Sha256 => xoid::ECDSA_WITH_SHA256,
                    Hash::Sha384 => xoid::ECDSA_WITH_SHA384,
                    Hash::Sha512 => xoid::ECDSA_WITH_SHA512,
                },
                false,
            ),
            SignerKey::Ed25519(_) => alg(xoid::ED25519, false),
        }
    }

    /// The SignerInfo's signatureAlgorithm: rsaEncryption for PKCS#1 v1.5 (the reader takes the hash from the digest
    /// algorithm), else the certificate's identifier.
    fn cms_alg(&self) -> Vec<u8> {
        match self {
            SignerKey::Rsa(..) => alg(xoid::RSA_ENCRYPTION, true),
            _ => self.cert_alg(),
        }
    }

    /// The longest signature this key makes.
    fn max_sig(&self) -> usize {
        match self {
            SignerKey::Rsa(k, _) | SignerKey::RsaPss(k, _) => k.public_key().size(),
            SignerKey::P256(..) => 72,
            SignerKey::Ed25519(_) => 64,
        }
    }
}

fn hash_oid(h: Hash) -> &'static [u8] {
    match h {
        Hash::Sha256 => xoid::SHA256,
        Hash::Sha384 => xoid::SHA384,
        Hash::Sha512 => xoid::SHA512,
    }
}

/// RSASSA-PSS with RSASSA-PSS-params (RFC 4055 §3.1): hashAlgorithm `h`, MGF1(`h`), saltLength = `h`'s length,
/// trailerField the DEFAULT.
fn pss_alg(h: Hash) -> Vec<u8> {
    let hid = alg(hash_oid(h), true);
    let mgf = seq(&[&oid(xoid::MGF1), &hid]);
    let params = seq(&[&explicit(0, &hid), &explicit(1, &mgf), &explicit(2, &build::int(&[h.len() as u8]))]);
    seq(&[&oid(xoid::RSASSA_PSS), &params])
}

/// The RSASSA-PSS salt: the hash's length (tls_core's `verify_signed` rule) and DERIVED, `h("PDFSIGN-PSS" || d ||
/// msg)` — reproducible output (RFC 6979's spirit), unpredictable without the private key.
fn pss_salt(k: &PrivateKey, h: Hash, msg: &[u8]) -> Vec<u8> {
    let mut d = vec![0u8; k.public_key().size()];
    let n = k.part_be(Part::D, &mut d);
    let salt = h.digest(&[b"PDFSIGN-PSS", &d[..n], msg]);
    d[..].zeroize();
    salt
}

fn rsa_out(k: &PrivateKey, f: impl FnOnce(&mut [u8]) -> Result<usize, crypto_core::Error>) -> Vec<u8> {
    let mut out = vec![0u8; k.public_key().size()];
    match f(&mut out) {
        Ok(n) => {
            out.truncate(n);
            out
        }
        Err(_) => Vec::new(),
    }
}

/// A detached SignedData over the `digest` (the key's [`SignerKey::hash`]) of the signed bytes, signed by `key`; `chain[0]` is the signer's
/// certificate (its issuer and serial name the signer), the whole `chain` is the certificate bag; `signing_time` in
/// seconds since the epoch; `cades` adds the ESS signing-certificate-v2 attribute (ETSI.CAdES.detached).
pub fn build(key: &SignerKey<'_>, digest: &[u8], chain: &[&[u8]], signing_time: i64, cades: bool) -> Result<Vec<u8>, Error> {
    assemble(key, digest, chain, signing_time, cades, false)
}

/// The largest [`build`] can be for these inputs: the same assembly over a zero digest and a zero signature of the
/// key's longest length — the /Contents placeholder's size.
pub(crate) fn bound(key: &SignerKey<'_>, chain: &[&[u8]], signing_time: i64, cades: bool) -> Result<usize, Error> {
    Ok(assemble(key, &vec![0u8; key.hash().len()], chain, signing_time, cades, true)?.len())
}

fn assemble(key: &SignerKey<'_>, digest: &[u8], chain: &[&[u8]], signing_time: i64, cades: bool, dry: bool) -> Result<Vec<u8>, Error> {
    const WHY: Error = Error::Unsupported("signer certificate");
    let first = *chain.first().ok_or(WHY)?;
    let signer = Certificate::parse(first).map_err(|_| WHY)?;
    let mut attrs = vec![
        seq(&[&oid(ID_CONTENT_TYPE), &set(&[&oid(ID_DATA)])]),
        seq(&[&oid(ID_MESSAGE_DIGEST), &set(&[&octets(digest)])]),
        seq(&[&oid(ID_SIGNING_TIME), &set(&[&build::time(signing_time)])]),
    ];
    if cades {
        // SigningCertificateV2 ::= SEQUENCE { certs SEQUENCE OF ESSCertIDv2 }; ESSCertIDv2 ::= SEQUENCE { certHash }
        // (hashAlgorithm sha256 is the DEFAULT and so omitted; issuerSerial is optional).
        let cert_hash = Hash::Sha256.digest(&[first]);
        let ess = seq(&[&seq(&[&seq(&[&octets(&cert_hash)])])]);
        attrs.push(seq(&[&oid(ID_SIGNING_CERTIFICATE_V2), &set(&[&ess])]));
    }
    let content = set_of_content(attrs);
    // RFC 5652 §5.4: the signature is over the attributes with the EXPLICIT SET OF tag.
    let signature = if dry { vec![0u8; key.max_sig()] } else { key.sign(&encode_tlv(tag::SET, &content)) };
    let sid = seq(&[&signer.issuer, &encode_tlv(tag::INTEGER, &signer.serial)]);
    // NULL parameters as before for RSA / P-256; ABSENT for Ed25519's SHA-512 (RFC 8419 §3).
    let digest_alg = alg(hash_oid(key.hash()), !matches!(key, SignerKey::Ed25519(_)));
    let signer_info =
        seq(&[&build::int(&[1]), &sid, &digest_alg, &explicit(0, &content), &key.cms_alg(), &octets(&signature)]);
    let certs = set_of_content(chain.iter().map(|c| c.to_vec()).collect());
    let sd = seq(&[
        &build::int(&[1]),
        &set(&[&digest_alg]),
        &seq(&[&oid(ID_DATA)]),
        &explicit(0, &certs),
        &set(&[&signer_info]),
    ]);
    Ok(seq(&[&oid(ID_SIGNED_DATA), &explicit(0, &sd)]))
}
