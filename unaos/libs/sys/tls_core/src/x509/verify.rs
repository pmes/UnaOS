//! Certification path building and validation (RFC 5280 §6, simplified to the Web PKI profile) and the trust store.
//!
//! TLSCORE2 (SR58): issuers are searched among the server's certificates AND a caller-supplied intermediate pool
//! ([`TrustStore::intermediates`] — e.g. Mozilla's CCADB intermediate list, the remedy for servers that omit an
//! intermediate when no AIA fetch is possible), with backtracking, so a cross-signed root (the same subject and
//! key under two issuers) resolves to whichever anchor the store holds, and an expired cross-sign is passed over
//! for a valid path. Name constraints (dNSName, iPAddress, rfc822Name, URI, directoryName) of every CA on a
//! completed path apply to every certificate below it (§6.1.3 (b)/(c), self-issued intermediates excepted); a
//! completed path that fails them is discarded and the search continues.
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
    /// subjectPublicKey bits (OCSP issuerKeyHash for leaves the anchor issued directly).
    pub key_bits: Vec<u8>,
}

impl TrustAnchor {
    pub fn from_cert(c: &Certificate) -> Self {
        TrustAnchor {
            subject: c.subject.clone(),
            public_key: c.public_key.clone(),
            name_constraints: c.name_constraints.clone(),
            subject_key_id: c.subject_key_id.clone(),
            key_bits: c.key_bits.clone(),
        }
    }
}

/// The set of trust anchors (e.g. `/system/trust/roots.pem`, the Mozilla bundle).
#[derive(Debug, Clone, Default)]
pub struct TrustStore {
    pub anchors: Vec<TrustAnchor>,
    /// NOT trusted: candidate issuers consulted after the server's own certificates (missing intermediates).
    pub intermediates: Vec<Certificate>,
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
        TrustStore { anchors: Vec::new(), intermediates: Vec::new() }
    }
    /// Adds every CERTIFICATE block of `text` to the intermediate pool; returns how many parsed.
    pub fn add_intermediates_pem(&mut self, text: &str) -> usize {
        let (blocks, _) = pem::pem_blocks(text, "CERTIFICATE");
        let before = self.intermediates.len();
        for b in blocks {
            if let Ok(c) = Certificate::parse(&b) {
                if c.is_ca() {
                    self.intermediates.push(c);
                }
            }
        }
        self.intermediates.len() - before
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

/// A validated path: the leaf, the intermediates above it (in order), and the anchor that ends it.
#[derive(Debug, Clone)]
pub struct VerifiedPath {
    pub leaf: Certificate,
    pub intermediates: Vec<Certificate>,
    pub anchor: TrustAnchor,
    /// How many of `intermediates` came from the store's pool rather than the server.
    pub from_pool: usize,
}

impl VerifiedPath {
    /// The leaf's issuer (subject, key, key bits) — the first intermediate, or the anchor.
    pub fn leaf_issuer(&self) -> super::ocsp::Issuer<'_> {
        match self.intermediates.first() {
            Some(c) => super::ocsp::Issuer { subject: &c.subject, key: &c.public_key, key_bits: &c.key_bits },
            None => super::ocsp::Issuer { subject: &self.anchor.subject, key: &self.anchor.public_key, key_bits: &self.anchor.key_bits },
        }
    }
}

struct Builder<'a> {
    p: &'a dyn CryptoProvider,
    store: &'a TrustStore,
    now: i64,
    /// Server-presented candidates first, then the pool's.
    cands: Vec<Certificate>,
    presented: usize,
    budget: u32,
}

impl<'a> Builder<'a> {
    /// RFC 5280 §6.1.3 (b)/(c) over a completed path: each CA's constraints (the anchor's over everything) apply to
    /// every certificate below it; self-issued intermediates are exempt as subjects.
    fn constraints_hold(&self, path: &[usize], leaf: &Certificate, anchor: &TrustAnchor) -> bool {
        let certs: Vec<&Certificate> = core::iter::once(leaf).chain(path.iter().map(|&i| &self.cands[i])).collect();
        let below_ok = |nc: &NameConstraints, upto: usize| {
            (0..upto).all(|j| (j > 0 && certs[j].self_issued()) || nc.permits(&certs[j].subject, &certs[j].san))
        };
        if let Some(nc) = &anchor.name_constraints {
            if !below_ok(nc, certs.len()) {
                return false;
            }
        }
        for (k, c) in certs.iter().enumerate().skip(1) {
            if let Some(nc) = &c.name_constraints {
                if !below_ok(nc, k) {
                    return false;
                }
            }
        }
        true
    }

    /// `path[..]` indexes `cands` above the leaf; returns the anchor index that completed it.
    fn extend(&mut self, path: &mut Vec<usize>, leaf: &Certificate) -> Result<usize, CertError> {
        let current = match path.last() {
            Some(&i) => self.cands[i].clone(),
            None => leaf.clone(),
        };
        let mut best = CertError::UnknownIssuer;

        // 1. A trust anchor issued it?
        for (ai, a) in self.store.anchors.iter().enumerate() {
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
                    if self.constraints_hold(path, leaf, a) {
                        return Ok(ai);
                    }
                    best = CertError::NameConstraint;
                }
                Err(e) => best = e,
            }
        }

        // 2. An intermediate: the server's, then the pool's.
        if path.len() >= MAX_DEPTH {
            return Err(CertError::PathTooLong);
        }
        for i in 0..self.cands.len() {
            if path.contains(&i) {
                continue;
            }
            let cand = &self.cands[i];
            if cand.subject != current.issuer {
                continue;
            }
            // The same certificate presented twice (server list and pool): try it once.
            if path.iter().any(|&j| self.cands[j].der == cand.der) || (i >= self.presented && self.cands[..self.presented].iter().any(|c| c.der == cand.der)) {
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
                    let below = path.iter().filter(|&&j| !self.cands[j].self_issued()).count();
                    if below as u32 > max {
                        return Err(CertError::PathLenExceeded);
                    }
                }
                check_validity(&cand, self.now)?;
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
                        Ok(a) => return Ok(a),
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
    Ok(verify_server_chain_path(p, store, now, chain, server_name)?.leaf.public_key)
}

/// [`verify_server_chain`], returning the whole validated path.
pub fn verify_server_chain_path(
    p: &dyn CryptoProvider,
    store: &TrustStore,
    now: i64,
    chain: &[Vec<u8>],
    server_name: Option<&str>,
) -> Result<VerifiedPath, CertError> {
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
    let mut cands = Vec::new();
    for d in &chain[1..] {
        // An unparseable extra certificate is skipped, not fatal: servers send junk (e.g. the root itself).
        if let Ok(c) = Certificate::parse(d) {
            cands.push(c);
        }
    }
    let presented = cands.len();
    cands.extend(store.intermediates.iter().cloned());
    let mut b = Builder { p, store, now, cands, presented, budget: MAX_SIGNATURE_CHECKS };
    let mut path = Vec::new();
    let ai = b.extend(&mut path, &leaf)?;
    let from_pool = path.iter().filter(|&&i| i >= presented).count();
    let intermediates = path.iter().map(|&i| b.cands[i].clone()).collect();
    Ok(VerifiedPath { leaf, intermediates, anchor: store.anchors[ai].clone(), from_pool })
}

/// What the server sent with its chain (the verifier's whole input).
#[derive(Debug, Clone, Copy)]
pub struct PeerCertificates<'c> {
    pub chain: &'c [Vec<u8>],
    /// A stapled OCSPResponse (DER), when the server sent one.
    pub ocsp: Option<&'c [u8]>,
    /// The TLS-delivered SignedCertificateTimestampList.
    pub sct_list: Option<&'c [u8]>,
    /// Did we ask for a staple (status_request)?
    pub ocsp_requested: bool,
}

/// The verifier's verdict.
#[derive(Debug, Clone)]
pub struct CertVerdict {
    pub key: PublicKey,
    pub ocsp: super::ocsp::OcspStatus,
    /// Every SCT found (embedded, TLS, OCSP) — parsed, not verified.
    pub scts: Vec<super::sct::Sct>,
    /// Intermediates taken from the store's pool (the server omitted them).
    pub pool_intermediates: usize,
}

/// SCTs from the leaf's extension and the TLS extension.
pub fn collect_scts(leaf_der: Option<&[u8]>, tls_list: Option<&[u8]>) -> Vec<super::sct::Sct> {
    use super::sct::{parse_list, SctSource};
    let mut v = Vec::new();
    if let Some(c) = leaf_der.and_then(|d| Certificate::parse(d).ok()) {
        if let Some(l) = &c.sct_list {
            v.extend(parse_list(l, SctSource::Embedded).unwrap_or_default());
        }
    }
    if let Some(l) = tls_list {
        v.extend(parse_list(l, SctSource::Tls).unwrap_or_default());
    }
    v
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

    /// The path, then the stapled OCSP response (verified when present), then the SCTs.
    fn verify_server_cert_full(
        &self,
        provider: &dyn CryptoProvider,
        peer: &PeerCertificates<'_>,
        server_name: Option<&str>,
    ) -> Result<CertVerdict, TlsError> {
        use super::ocsp::{verify_stapled, OcspStatus};
        let now = self.clock.now();
        let path = verify_server_chain_path(provider, self.store, now, peer.chain, server_name)?;
        let mut scts = collect_scts(peer.chain.first().map(|v| v.as_slice()), peer.sct_list);
        let ocsp = match peer.ocsp {
            Some(r) => {
                let (st, more) = verify_stapled(provider, &path.leaf, &path.leaf_issuer(), r, now)?;
                scts.extend(more);
                st
            }
            None if peer.ocsp_requested => OcspStatus::NotStapled,
            None => OcspStatus::NotRequested,
        };
        Ok(CertVerdict { key: path.leaf.public_key.clone(), ocsp, scts, pool_intermediates: path.from_pool })
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
