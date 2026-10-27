// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Aether — shared-core. PDFSIGN (LEDGER SR350): the CMS SignedData WRITER (RFC 5652), the twin of the reader
//! in `sig.rs` (its OIDs and `Hash` are the ones written here: one vocabulary). DER goes through tls_core's ONE writer
//! (`tls_core::x509::build`); signatures through crypto_core's signers with the CALLER's key — no key is kept here.
//!
//! [`build`] writes a detached SignedData: digestAlgorithms {sha256}, eContentType id-data with no eContent, the
//! certificate bag, and one SignerInfo (v1, issuerAndSerialNumber from the signer's certificate) whose DER-sorted
//! signed attributes are contentType, messageDigest, signingTime and, for CAdES, ESS signing-certificate-v2.

use alloc::vec;
use alloc::vec::Vec;

use crypto_core::p256;
pub use crypto_core::p256::SecretKey;
pub use crypto_core::rsa_private::PrivateKey;
use tls_core::x509::build::{self, alg, explicit, octets, oid, seq, set, set_of_content};
use tls_core::x509::der::{encode_tlv, tag};
use tls_core::x509::{Certificate, oid as xoid};

use super::{Hash, ID_CONTENT_TYPE, ID_DATA, ID_MESSAGE_DIGEST, ID_SIGNED_DATA, ID_SIGNING_TIME};
use crate::Error;

/// id-aa-signingCertificateV2 (RFC 5035 §3).
const ID_SIGNING_CERTIFICATE_V2: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x10, 0x02, 0x2f];

/// The caller's signing key (borrowed; never stored).
pub enum SignerKey<'a> {
    /// RSASSA-PKCS1-v1_5 with SHA-256.
    Rsa(&'a PrivateKey),
    /// ECDSA P-256 with SHA-256 (RFC 6979 deterministic nonce).
    P256(&'a SecretKey),
}

impl SignerKey<'_> {
    /// SHA-256 over `msg`, then the key's signature: k bytes for RSA, a DER Ecdsa-Sig-Value for P-256. (RSA signing
    /// of a 32-byte digest cannot fail for a valid key; an impossible failure yields an empty signature, which no
    /// verifier accepts.)
    pub fn sign(&self, msg: &[u8]) -> Vec<u8> {
        let digest = Hash::Sha256.digest(&[msg]);
        match self {
            SignerKey::Rsa(k) => {
                let mut out = vec![0u8; k.public_key().size()];
                match k.sign_pkcs1v15(&digest, &mut out) {
                    Ok(n) => {
                        out.truncate(n);
                        out
                    }
                    Err(_) => Vec::new(),
                }
            }
            SignerKey::P256(k) => {
                let raw = k.sign_prehashed(&digest);
                let mut der = [0u8; 72];
                let n = p256::signature_to_der(&raw, &mut der);
                der[..n].to_vec()
            }
        }
    }

    /// The public key as a SubjectPublicKeyInfo.
    pub fn spki(&self) -> Vec<u8> {
        match self {
            SignerKey::Rsa(k) => {
                let p = k.public_key();
                let mut n = vec![0u8; p.size()];
                let mut e = vec![0u8; p.size()];
                let nl = p.n_be(&mut n);
                let el = p.e_be(&mut e);
                build::spki_rsa(&n[..nl], &e[..el])
            }
            SignerKey::P256(k) => build::spki_p256(&k.public_key().to_sec1_uncompressed()),
        }
    }

    /// The AlgorithmIdentifier of a certificate this key signs (sha256WithRSAEncryption / ecdsa-with-SHA256).
    pub fn cert_alg(&self) -> Vec<u8> {
        match self {
            SignerKey::Rsa(_) => alg(xoid::SHA256_WITH_RSA, true),
            SignerKey::P256(_) => alg(xoid::ECDSA_WITH_SHA256, false),
        }
    }

    /// The SignerInfo's signatureAlgorithm: rsaEncryption (the reader takes the hash from the digest algorithm) or
    /// ecdsa-with-SHA256.
    fn cms_alg(&self) -> Vec<u8> {
        match self {
            SignerKey::Rsa(_) => alg(xoid::RSA_ENCRYPTION, true),
            SignerKey::P256(_) => alg(xoid::ECDSA_WITH_SHA256, false),
        }
    }

    /// The longest signature this key makes.
    fn max_sig(&self) -> usize {
        match self {
            SignerKey::Rsa(k) => k.public_key().size(),
            SignerKey::P256(_) => 72,
        }
    }
}

/// A detached SignedData over the SHA-256 `digest` of the signed bytes, signed by `key`; `chain[0]` is the signer's
/// certificate (its issuer and serial name the signer), the whole `chain` is the certificate bag; `signing_time` in
/// seconds since the epoch; `cades` adds the ESS signing-certificate-v2 attribute (ETSI.CAdES.detached).
pub fn build(key: &SignerKey<'_>, digest: &[u8; 32], chain: &[&[u8]], signing_time: i64, cades: bool) -> Result<Vec<u8>, Error> {
    assemble(key, digest, chain, signing_time, cades, false)
}

/// The largest [`build`] can be for these inputs: the same assembly over a zero digest and a zero signature of the
/// key's longest length — the /Contents placeholder's size.
pub(crate) fn bound(key: &SignerKey<'_>, chain: &[&[u8]], signing_time: i64, cades: bool) -> Result<usize, Error> {
    Ok(assemble(key, &[0u8; 32], chain, signing_time, cades, true)?.len())
}

fn assemble(key: &SignerKey<'_>, digest: &[u8; 32], chain: &[&[u8]], signing_time: i64, cades: bool, dry: bool) -> Result<Vec<u8>, Error> {
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
    let sha256 = alg(xoid::SHA256, true);
    let signer_info =
        seq(&[&build::int(&[1]), &sid, &sha256, &explicit(0, &content), &key.cms_alg(), &octets(&signature)]);
    let certs = set_of_content(chain.iter().map(|c| c.to_vec()).collect());
    let sd = seq(&[
        &build::int(&[1]),
        &set(&[&sha256]),
        &seq(&[&oid(ID_DATA)]),
        &explicit(0, &certs),
        &set(&[&signer_info]),
    ]);
    Ok(seq(&[&oid(ID_SIGNED_DATA), &explicit(0, &sd)]))
}
