//! Stapled OCSP (RFC 6960, delivered by RFC 6066 §8 in TLS 1.2 and RFC 8446 §4.4.2.1 in TLS 1.3): VERIFIED when
//! present, not required when absent (soft-fail is the Web PKI's reality; hard-fail would be "must-staple",
//! RFC 7633, which is owed).
//!
//! A stapled response is accepted only if, for the validated path's leaf:
//! 1. responseStatus = successful and the type is id-pkix-ocsp-basic;
//! 2. one SingleResponse's CertID names the leaf: issuerNameHash = H(leaf issuer DN), issuerKeyHash =
//!    H(issuer subjectPublicKey bits), serialNumber = the leaf's, H ∈ {SHA-1, SHA-256, SHA-384, SHA-512};
//! 3. the signer is the leaf's issuer itself, or a delegated responder certificate carried in the response that
//!    the issuer signed, that has id-kp-OCSPSigning, and that is valid now (§4.2.2.2); its signature over
//!    tbsResponseData verifies, and the responderID names it (by DN, or by SHA-1 key hash when the provider has SHA-1);
//! 4. thisUpdate ≤ now + 5 min, and now ≤ nextUpdate + 5 min (a response without nextUpdate is accepted for
//!    4 days after thisUpdate);
//! then `good` → [`OcspStatus::Good`], `revoked` → `CertError::Revoked`, `unknown` → [`OcspStatus::Unknown`].
//! Any other defect → `CertError::BadOcspResponse` (the alert is bad_certificate_status_response).

use alloc::vec::Vec;

use super::cert::{parse_signature_algorithm, Certificate, PublicKey};
use super::der::{self, tag, Der};
use super::sct::{self, Sct, SctSource};
use super::{oid, verify_signed};
use crate::crypto::{CryptoProvider, HashAlg};
use crate::error::CertError;

const SKEW: i64 = 300;
const NO_NEXT_UPDATE_WINDOW: i64 = 4 * 86_400;

fn bad(m: &'static str) -> CertError {
    CertError::BadOcspResponse(m)
}

/// The result of checking a stapled response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OcspStatus {
    /// We did not ask for one.
    NotRequested,
    /// We asked; the server stapled nothing.
    NotStapled,
    /// A verified `good` response.
    Good { produced_at: i64, this_update: i64, next_update: Option<i64>, delegated: bool },
    /// A verified `unknown` response (the responder does not know the certificate).
    Unknown,
    /// Stapled, but the verifier in use does not check OCSP.
    NotChecked,
}

/// The issuer of the leaf, as the validated path knows it.
pub struct Issuer<'a> {
    pub subject: &'a [u8],
    pub key: &'a PublicKey,
    pub key_bits: &'a [u8],
}

fn hash_with(p: &dyn CryptoProvider, alg_oid: &[u8], data: &[u8]) -> Result<Vec<u8>, CertError> {
    let h = match alg_oid {
        x if x == oid::SHA1 => return p.sha1(&[data]).map(|d| d.to_vec()).map_err(|_| bad("CertID uses SHA-1 and the provider has none")),
        x if x == oid::SHA256 => HashAlg::Sha256,
        x if x == oid::SHA384 => HashAlg::Sha384,
        x if x == oid::SHA512 => HashAlg::Sha512,
        _ => return Err(bad("CertID hash algorithm")),
    };
    Ok(p.hash(h, &[data]).as_bytes().to_vec())
}

fn gtime(t: &der::Tlv<'_>) -> Result<i64, CertError> {
    if t.tag != tag::GENERALIZED_TIME {
        return Err(bad("time is not GeneralizedTime"));
    }
    der::parse_time(t).map_err(|_| bad("GeneralizedTime"))
}

/// Verifies `response` (DER OCSPResponse) for `leaf` issued by `issuer` at `now`. Returns the status and any SCTs
/// the response carries.
pub fn verify_stapled(
    p: &dyn CryptoProvider,
    leaf: &Certificate,
    issuer: &Issuer<'_>,
    response: &[u8],
    now: i64,
) -> Result<(OcspStatus, Vec<Sct>), CertError> {
    let d0 = |e: CertError| match e {
        CertError::BadDer(m) => bad(m),
        other => other,
    };
    // OCSPResponse ::= SEQUENCE { responseStatus ENUMERATED, responseBytes [0] EXPLICIT ResponseBytes OPTIONAL }
    let mut top = Der::new(response);
    let mut resp = top.sequence().map_err(d0)?;
    top.expect_end().map_err(d0)?;
    let status = resp.expect(0x0a).map_err(d0)?;
    if status.value != [0] {
        return Err(bad("responseStatus is not successful"));
    }
    let rb = resp.expect(tag::context_constructed(0)).map_err(d0)?;
    resp.expect_end().map_err(d0)?;
    let mut rbd = Der::new(rb.value);
    let mut rbs = rbd.sequence().map_err(d0)?;
    if rbs.oid().map_err(d0)? != oid::OCSP_BASIC {
        return Err(bad("responseType is not id-pkix-ocsp-basic"));
    }
    let basic_bytes = rbs.expect(tag::OCTET_STRING).map_err(d0)?.value;
    // BasicOCSPResponse ::= SEQUENCE { tbsResponseData, signatureAlgorithm, signature BIT STRING, certs [0] OPTIONAL }
    let mut bd = Der::new(basic_bytes);
    let mut basic = bd.sequence().map_err(d0)?;
    bd.expect_end().map_err(d0)?;
    let tbs = basic.expect(tag::SEQUENCE).map_err(d0)?;
    let sig_alg = parse_signature_algorithm(basic.expect(tag::SEQUENCE).map_err(d0)?.value).map_err(d0)?;
    let signature = basic.bit_string_bytes().map_err(d0)?;
    let mut certs = Vec::new();
    if let Some(c) = basic.optional(tag::context_constructed(0)).map_err(d0)? {
        let mut cd = Der::new(c.value);
        let mut list = cd.sequence().map_err(d0)?;
        while !list.is_empty() {
            let raw = list.tlv().map_err(d0)?.raw;
            certs.push(Certificate::parse(raw).map_err(|_| bad("responder certificate"))?);
        }
    }
    basic.expect_end().map_err(d0)?;

    // ResponseData
    let mut rd = Der::new(tbs.value);
    if let Some(v) = rd.optional(tag::context_constructed(0)).map_err(d0)? {
        if Der::new(v.value).small_uint().map_err(d0)? != 0 {
            return Err(bad("ResponseData version"));
        }
    }
    let rid = rd.tlv().map_err(d0)?;
    let produced_at = gtime(&rd.tlv().map_err(d0)?)?;
    let mut responses = rd.sequence().map_err(d0)?;
    // responseExtensions [1]: a nonce is irrelevant for a stapled response; nothing else is processed.
    let _ = rd.optional(tag::context_constructed(1)).map_err(d0)?;
    rd.expect_end().map_err(d0)?;

    // ---- the signer (§4.2.2.2)
    let rid_matches = |subject: &[u8], key_bits: &[u8]| -> bool {
        match rid.tag {
            0xa1 => Der::new(rid.value).expect(tag::SEQUENCE).map(|n| n.raw == subject).unwrap_or(false),
            0xa2 => match Der::new(rid.value).expect(tag::OCTET_STRING) {
                Ok(h) => match p.sha1(&[key_bits]) {
                    Ok(k) => h.value == k,
                    Err(_) => true, // cannot compute the key hash: let the signature decide
                },
                Err(_) => false,
            },
            _ => false,
        }
    };
    let msg = tbs.raw;
    let mut delegated = false;
    let signed_ok = if rid_matches(issuer.subject, issuer.key_bits) && verify_signed(p, issuer.key, sig_alg, msg, signature).is_ok() {
        true
    } else {
        let mut ok = false;
        for c in &certs {
            if c.issuer != issuer.subject || !rid_matches(&c.subject, &c.key_bits) {
                continue;
            }
            let eku_ok = c.ext_key_usage.as_ref().is_some_and(|v| v.iter().any(|o| o == oid::KP_OCSP_SIGNING));
            if !eku_ok || now < c.not_before || now > c.not_after {
                continue;
            }
            if verify_signed(p, issuer.key, c.signature_algorithm, c.tbs(), &c.signature).is_err() {
                continue;
            }
            if verify_signed(p, &c.public_key, sig_alg, msg, signature).is_ok() {
                ok = true;
                delegated = true;
                break;
            }
        }
        ok
    };
    if !signed_ok {
        return Err(bad("signature: not the issuer, nor a responder it authorised"));
    }

    // ---- the SingleResponse for our leaf
    while !responses.is_empty() {
        let mut sr = responses.sequence().map_err(d0)?;
        let mut cid = sr.sequence().map_err(d0)?;
        let mut alg = cid.sequence().map_err(d0)?;
        let halg = alg.oid().map_err(d0)?;
        let name_hash = cid.expect(tag::OCTET_STRING).map_err(d0)?.value;
        let key_hash = cid.expect(tag::OCTET_STRING).map_err(d0)?.value;
        let serial = cid.expect(tag::INTEGER).map_err(d0)?.value;
        let status = sr.tlv().map_err(d0)?;
        let this_update = gtime(&sr.tlv().map_err(d0)?)?;
        let next_update = match sr.optional(tag::context_constructed(0)).map_err(d0)? {
            Some(n) => Some(gtime(&Der::new(n.value).tlv().map_err(d0)?)?),
            None => None,
        };
        let mut scts = Vec::new();
        if let Some(e) = sr.optional(tag::context_constructed(1)).map_err(d0)? {
            let mut ed = Der::new(e.value);
            let mut list = ed.sequence().map_err(d0)?;
            while !list.is_empty() {
                let mut x = list.sequence().map_err(d0)?;
                let id = x.oid().map_err(d0)?;
                if x.peek_tag() == Some(tag::BOOLEAN) {
                    x.boolean().map_err(d0)?;
                }
                let v = x.expect(tag::OCTET_STRING).map_err(d0)?.value;
                if id == oid::CT_OCSP_SCTS {
                    if let Ok(inner) = Der::new(v).expect(tag::OCTET_STRING) {
                        scts = sct::parse_list(inner.value, SctSource::Ocsp).unwrap_or_default();
                    }
                }
            }
        }
        if serial != leaf.serial.as_slice() {
            continue;
        }
        if hash_with(p, halg, &leaf.issuer)? != name_hash || hash_with(p, halg, issuer.key_bits)? != key_hash {
            continue;
        }
        if this_update > now + SKEW {
            return Err(bad("thisUpdate is in the future"));
        }
        match next_update {
            Some(n) if now > n + SKEW => return Err(bad("stale: past nextUpdate")),
            None if now > this_update + NO_NEXT_UPDATE_WINDOW => return Err(bad("stale: no nextUpdate and old")),
            _ => {}
        }
        return match status.tag {
            0x80 => Ok((OcspStatus::Good { produced_at, this_update, next_update, delegated }, scts)),
            0xa1 => Err(CertError::Revoked),
            0x82 => Ok((OcspStatus::Unknown, scts)),
            _ => Err(bad("certStatus")),
        };
    }
    Err(bad("no SingleResponse for this certificate"))
}
