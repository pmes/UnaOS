//! The authorityInfoAccess `caIssuers` fetch seam (RFC 5280 §4.2.2.1) — CTCORE (SR60).
//!
//! A server that omits its intermediate can still be verified when the leaf (or an intermediate) names where its
//! issuer's certificate lives. tls_core never opens a socket: the CALLER implements [`IssuerFetcher`] (the host
//! through http_core, the metal through NET) and wraps its verifier in [`AiaVerifier`], which on
//! `UnknownIssuer` fetches the caIssuers URIs of the presented certificates (and of what it fetched), adds every
//! CA certificate found to a copy of the trust store's intermediate pool — untrusted, exactly like a pool
//! certificate — and validates again. A fetched certificate grants nothing by itself: the path must still end at
//! an anchor, with every RFC 5280 check. At most [`AiaVerifier::max_fetches`] URIs are fetched per handshake.
//!
//! RFC 5280 §4.2.2.1: the caIssuers resource is a single DER certificate or a "certs-only" CMS SignedData
//! (`.p7c`, RFC 5652 §5.1 with no signers); both are read here, and a PEM body is accepted too.

use alloc::vec::Vec;
use core::cell::Cell;

use super::cert::Certificate;
use super::der::{tag, Der};
use super::verify::{CertVerdict, PeerCertificates, TrustStore, WebPkiVerifier};
use crate::client::ServerCertVerifier;
use crate::crypto::CryptoProvider;
use crate::error::{CertError, TlsError};

fn unknown_issuer(r: &Result<CertVerdict, TlsError>) -> bool {
    matches!(r, Err(TlsError::Certificate(CertError::UnknownIssuer)))
}

/// Fetches a caIssuers URI (typically `http://…/x.crt` or `.p7c`). `None` = could not.
pub trait IssuerFetcher {
    fn fetch(&self, uri: &str) -> Option<Vec<u8>>;
}

/// id-signedData (1.2.840.113549.1.7.2).
const SIGNED_DATA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02];

/// The certificates in a caIssuers response: DER certificate, certs-only CMS SignedData, or PEM.
pub fn certs_from_response(body: &[u8]) -> Vec<Vec<u8>> {
    if let Ok(text) = core::str::from_utf8(body) {
        if text.contains("-----BEGIN") {
            let (mut v, _) = super::pem::pem_blocks(text, "CERTIFICATE");
            if v.is_empty() {
                if let Some(b) = super::pem::pem_blocks(text, "PKCS7").0.first() {
                    v = certs_from_response(b);
                }
            }
            return v;
        }
    }
    if Certificate::parse(body).is_ok() {
        return alloc::vec![body.to_vec()];
    }
    pkcs7_certs(body).unwrap_or_default()
}

fn pkcs7_certs(body: &[u8]) -> Option<Vec<Vec<u8>>> {
    // ContentInfo ::= SEQUENCE { contentType OID, content [0] EXPLICIT SignedData }
    let mut d = Der::new(body);
    let mut ci = d.sequence().ok()?;
    if ci.oid().ok()? != SIGNED_DATA {
        return None;
    }
    let content = ci.expect(tag::context_constructed(0)).ok()?;
    // SignedData ::= SEQUENCE { version, digestAlgorithms SET, encapContentInfo SEQUENCE,
    //                           certificates [0] IMPLICIT CertificateSet OPTIONAL, crls [1] OPTIONAL, signerInfos SET }
    let mut sd = Der::new(content.value).sequence().ok()?;
    sd.expect(tag::INTEGER).ok()?;
    sd.expect(tag::SET).ok()?;
    sd.expect(tag::SEQUENCE).ok()?;
    let set = sd.optional(tag::context_constructed(0)).ok()??;
    let mut s = Der::new(set.value);
    let mut out = Vec::new();
    while !s.is_empty() {
        let c = s.tlv().ok()?;
        if c.tag == tag::SEQUENCE {
            out.push(c.raw.to_vec());
        }
    }
    Some(out)
}

/// A verifier that completes a path through caIssuers fetches when the server omitted an intermediate.
pub struct AiaVerifier<'a> {
    pub web: WebPkiVerifier<'a>,
    pub fetch: &'a dyn IssuerFetcher,
    pub max_fetches: usize,
    /// How many URIs the last handshake fetched, and how many CA certificates they yielded.
    pub fetched: Cell<(usize, usize)>,
}

impl<'a> AiaVerifier<'a> {
    pub fn new(web: WebPkiVerifier<'a>, fetch: &'a dyn IssuerFetcher) -> Self {
        AiaVerifier { web, fetch, max_fetches: 4, fetched: Cell::new((0, 0)) }
    }
}

impl ServerCertVerifier for AiaVerifier<'_> {
    fn verify_server_cert(&self, p: &dyn CryptoProvider, chain: &[Vec<u8>], name: Option<&str>) -> Result<super::PublicKey, TlsError> {
        let peer = PeerCertificates { chain, ocsp: None, sct_list: None, ocsp_requested: false };
        self.verify_server_cert_full(p, &peer, name).map(|v| v.key)
    }

    fn verify_server_cert_full(&self, p: &dyn CryptoProvider, peer: &PeerCertificates<'_>, name: Option<&str>) -> Result<CertVerdict, TlsError> {
        self.fetched.set((0, 0));
        let first = self.web.verify_server_cert_full(p, peer, name);
        if !unknown_issuer(&first) {
            return first;
        }
        let mut store: TrustStore = self.web.store.clone();
        let mut queue: Vec<alloc::string::String> = Vec::new();
        for d in peer.chain {
            if let Ok(c) = Certificate::parse(d) {
                queue.extend(c.aia_ca_issuers.iter().cloned());
            }
        }
        let (mut fetched, mut added) = (0usize, 0usize);
        let mut tried: Vec<alloc::string::String> = Vec::new();
        let mut last = first;
        while let Some(uri) = queue.first().cloned() {
            queue.remove(0);
            if tried.contains(&uri) || fetched >= self.max_fetches {
                continue;
            }
            tried.push(uri.clone());
            fetched += 1;
            let Some(body) = self.fetch.fetch(&uri) else { continue };
            let mut new = 0;
            for der in certs_from_response(&body) {
                if let Ok(c) = Certificate::parse(&der) {
                    if c.is_ca() && !store.intermediates.iter().any(|x| x.der == c.der) {
                        queue.extend(c.aia_ca_issuers.iter().cloned());
                        store.intermediates.push(c);
                        new += 1;
                    }
                }
            }
            added += new;
            self.fetched.set((fetched, added));
            if new == 0 {
                continue;
            }
            let v = WebPkiVerifier { store: &store, clock: self.web.clock };
            last = v.verify_server_cert_full(p, peer, name);
            if !unknown_issuer(&last) {
                return last;
            }
        }
        last
    }
}
