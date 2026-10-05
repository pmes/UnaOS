//! Certification path building and validation (RFC 5280 §6, simplified to the Web PKI profile) and the trust store.
//!
//! From the server's Certificate message (leaf first, then any intermediates in any order) the builder searches
//! for a path to a trust anchor: issuer Name ⇄ subject Name byte-equality, then the issuer key verifies the
//! signature. Each intermediate must be a CA (BasicConstraints cA), may sign certificates (KeyUsage keyCertSign
//! when present), must respect pathLenConstraint, must be valid now, and may restrict EKU to serverAuth. Name
//! constraints on any CA in the path (anchor included) are applied to the leaf's SAN dNSName/iPAddress.
//! The leaf must be valid now, must not be a CA, must permit digitalSignature and serverAuth when it restricts
//! them, and must name the server (RFC 6125). Any unprocessed critical extension rejects the certificate.

use alloc::vec::Vec;

use super::cert::{key_usage, Certificate, PublicKey};
use super::name::{self, NameConstraints};
use super::{oid, pem, verify_certificate_signature};
use crate::client::ServerCertVerifier;
use crate::crypto::CryptoProvider;
use crate::error::{CertError, TlsError};

/// Wall-clock time for validity checks.
pub trait Clock {
    /// Seconds since 1970-01-01T00:00:00Z.
    fn now(&self) -> i64;
    /// Milliseconds since the epoch (TLS 1.3 ticket ages, RFC 8446 §4.2.11.1). Default: `now() * 1000`.
    fn now_ms(&self) -> u64 {
        (self.now().max(0) as u64) * 1000
    }
}

/// A clock frozen at one instant (tests, and boot before the RTC is trusted).
pub struct FixedClock(pub i64);

impl Clock for FixedClock {
    fn now(&self) -> i64 {
        self.0
    }
}

/// A trust anchor: the parts of a root certificate path validation uses (RFC 5280 §6.1.1 (d)).
#[derive(Debug, Clone)]
pub struct TrustAnchor {
    pub subject: Vec<u8>,
    pub public_key: PublicKey,
    pub name_constraints: Option<NameConstraints>,
    pub subject_key_id: Option<Vec<u8>>,
}

impl TrustAnchor {
    pub fn from_cert(c: &Certificate) -> Self {
        TrustAnchor {
            subject: c.subject.clone(),
            public_key: c.public_key.clone(),
            name_constraints: c.name_constraints.clone(),
            subject_key_id: c.subject_key_id.clone(),
        }
    }
}

/// The set of trust anchors (e.g. `/system/trust/roots.pem`, the Mozilla bundle).
#[derive(Debug, Clone, Default)]
pub struct TrustStore {
    pub anchors: Vec<TrustAnchor>,
}

/// What loading a PEM bundle found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoadReport {
    pub loaded: usize,
    /// Blocks that failed base64 or DER parsing.
    pub rejected: usize,
    /// Anchors whose key type tls_core cannot verify with (kept, but they can never complete a path).
    pub unsupported_keys: usize,
}

impl TrustStore {
    pub fn new() -> Self {
        TrustStore { anchors: Vec::new() }
    }
    pub fn add_der(&mut self, der: &[u8]) -> Result<(), CertError> {
        let c = Certificate::parse(der)?;
        self.anchors.push(TrustAnchor::from_cert(&c));
        Ok(())
    }
    /// Loads every `CERTIFICATE` block of a PEM bundle.
    pub fn from_pem(text: &str) -> (TrustStore, LoadReport) {
        let (blocks, mut rejected) = pem::pem_blocks(text, "CERTIFICATE");
        let mut s = TrustStore::new();
        let mut unsupported = 0;
        for b in blocks {
            match Certificate::parse(&b) {
                Ok(c) => {
                    if matches!(c.public_key, PublicKey::Unsupported(_)) {
                        unsupported += 1;
                    }
                    s.anchors.push(TrustAnchor::from_cert(&c));
                }
                Err(_) => rejected += 1,
            }
        }
        let loaded = s.anchors.len();
        (s, LoadReport { loaded, rejected, unsupported_keys: unsupported })
    }
}

const MAX_DEPTH: usize = 6;
const MAX_SIGNATURE_CHECKS: u32 = 64;

fn eku_allows_server_auth(c: &Certificate) -> bool {
    match &c.ext_key_usage {
        None => true,
        Some(v) => v.iter().any(|o| o == oid::KP_SERVER_AUTH || o == oid::ANY_EXTENDED_KEY_USAGE),
    }
}

fn check_validity(c: &Certificate, now: i64) -> Result<(), CertError> {
    if now < c.not_before {
        return Err(CertError::NotYetValid);
    }
    if now > c.not_after {
        return Err(CertError::Expired);
    }
    Ok(())
}

struct Builder<'a> {
    p: &'a dyn CryptoProvider,
    store: &'a TrustStore,
    now: i64,
    intermediates: Vec<Certificate>,
    budget: u32,
}

impl<'a> Builder<'a> {
    /// `path[0]` is the leaf; `path.last()` the certificate whose issuer we look for.
    fn extend(&mut self, path: &mut Vec<usize>, leaf: &Certificate) -> Result<(), CertError> {
        let current = match path.last() {
            Some(&i) => &self.intermediates[i],
            None => leaf,
        };
        let current = current.clone();
        let mut best = CertError::UnknownIssuer;

        // 1. A trust anchor issued it?
        for a in self.store.anchors.iter() {
            if a.subject != current.issuer {
                continue;
            }
            if let (Some(aki), Some(ski)) = (&current.authority_key_id, &a.subject_key_id) {
                if aki != ski {
                    continue;
                }
            }
            if self.budget == 0 {
                return Err(CertError::PathTooLong);
            }
            self.budget -= 1;
            match verify_certificate_signature(self.p, &a.public_key, &current) {
                Ok(()) => {
                    // Anchor name constraints cover everything below it.
                    if let Some(nc) = &a.name_constraints {
                        if nc.unenforced > 0 && !nc.has_permitted_dns && !nc.has_permitted_ip && nc.excluded_dns.is_empty() && nc.excluded_ip.is_empty() {
                            // only constraint types we cannot enforce: refuse rather than ignore
                            best = CertError::NameConstraint;
                            continue;
                        }
                        if !nc.allows(&leaf.san) {
                            best = CertError::NameConstraint;
                            continue;
                        }
                    }
                    return Ok(());
                }
                Err(e) => best = e,
            }
        }

        // 2. An intermediate from the server's list?
        if path.len() >= MAX_DEPTH {
            return Err(CertError::PathTooLong);
        }
        for i in 0..self.intermediates.len() {
            if path.contains(&i) {
                continue;
            }
            let cand = &self.intermediates[i];
            if cand.subject != current.issuer {
                continue;
            }
            if let (Some(aki), Some(ski)) = (&current.authority_key_id, &cand.subject_key_id) {
                if aki != ski {
                    continue;
                }
            }
            let cand = cand.clone();
            let r = (|| -> Result<(), CertError> {
                if cand.unknown_critical {
                    return Err(CertError::UnknownCriticalExtension);
                }
                if !cand.is_ca() {
                    return Err(CertError::NotCa);
                }
                if let Some(ku) = cand.key_usage {
                    if ku & key_usage::KEY_CERT_SIGN == 0 {
                        return Err(CertError::NotCa);
                    }
                }
                if !eku_allows_server_auth(&cand) {
                    return Err(CertError::KeyUsage);
                }
                // pathLenConstraint: the number of non-self-issued intermediates BELOW this one.
                if let Some((_, Some(max))) = cand.basic_constraints {
                    let below = path.iter().filter(|&&j| !self.intermediates[j].self_issued()).count();
                    if below as u32 > max {
                        return Err(CertError::PathLenExceeded);
                    }
                }
                check_validity(&cand, self.now)?;
                if let Some(nc) = &cand.name_constraints {
                    if !nc.allows(&leaf.san) {
                        return Err(CertError::NameConstraint);
                    }
                }
                if self.budget == 0 {
                    return Err(CertError::PathTooLong);
                }
                self.budget -= 1;
                verify_certificate_signature(self.p, &cand.public_key, &current)
            })();
            match r {
                Ok(()) => {
                    path.push(i);
                    match self.extend(path, leaf) {
                        Ok(()) => return Ok(()),
                        Err(e) => {
                            path.pop();
                            best = e;
                        }
                    }
                }
                Err(e) => best = e,
            }
        }
        Err(best)
    }
}

/// Validates `chain` (DER, leaf first) for `server_name` at `now`; returns the leaf's public key.
pub fn verify_server_chain(
    p: &dyn CryptoProvider,
    store: &TrustStore,
    now: i64,
    chain: &[Vec<u8>],
    server_name: Option<&str>,
) -> Result<PublicKey, CertError> {
    if store.anchors.is_empty() {
        return Err(CertError::NoTrustAnchors);
    }
    let leaf_der = chain.first().ok_or(CertError::NoCertificate)?;
    let leaf = Certificate::parse(leaf_der)?;
    if leaf.unknown_critical {
        return Err(CertError::UnknownCriticalExtension);
    }
    if leaf.is_ca() {
        return Err(CertError::KeyUsage);
    }
    if let Some(ku) = leaf.key_usage {
        if ku & key_usage::DIGITAL_SIGNATURE == 0 {
            return Err(CertError::KeyUsage);
        }
    }
    if !eku_allows_server_auth(&leaf) {
        return Err(CertError::KeyUsage);
    }
    check_validity(&leaf, now)?;
    if let Some(n) = server_name {
        if !name::matches_server_name(&leaf.san, n) {
            return Err(CertError::NameMismatch);
        }
    }
    let mut intermediates = Vec::new();
    for d in &chain[1..] {
        // An unparseable extra certificate is skipped, not fatal: servers send junk (e.g. the root itself).
        if let Ok(c) = Certificate::parse(d) {
            intermediates.push(c);
        }
    }
    let mut b = Builder { p, store, now, intermediates, budget: MAX_SIGNATURE_CHECKS };
    let mut path = Vec::new();
    b.extend(&mut path, &leaf)?;
    Ok(leaf.public_key)
}

/// The production verifier: a trust store plus a clock.
pub struct WebPkiVerifier<'a> {
    pub store: &'a TrustStore,
    pub clock: &'a dyn Clock,
}

impl ServerCertVerifier for WebPkiVerifier<'_> {
    fn verify_server_cert(
        &self,
        provider: &dyn CryptoProvider,
        chain: &[Vec<u8>],
        server_name: Option<&str>,
    ) -> Result<PublicKey, TlsError> {
        Ok(verify_server_chain(provider, self.store, self.clock.now(), chain, server_name)?)
    }
}

/// Accepts exactly one pinned leaf certificate (by DER) — for KATs whose server certificate is not a Web PKI
/// certificate (the RFC 8448 traces). The CertificateVerify signature is still checked with its key.
pub struct PinnedLeafVerifier {
    pub leaf_der: Vec<u8>,
}

impl ServerCertVerifier for PinnedLeafVerifier {
    fn verify_server_cert(&self, _p: &dyn CryptoProvider, chain: &[Vec<u8>], _n: Option<&str>) -> Result<PublicKey, TlsError> {
        let leaf = chain.first().ok_or(CertError::NoCertificate)?;
        if *leaf != self.leaf_der {
            return Err(CertError::UnknownIssuer.into());
        }
        Ok(Certificate::parse(leaf)?.public_key)
    }
}
