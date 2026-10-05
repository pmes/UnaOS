//! Certificate Transparency SCTs (RFC 6962 §3.2–3.3): PARSED and REPORTED, not verified and not required.
//!
//! Three places carry a SignedCertificateTimestampList: the leaf's X.509 extension (1.3.6.1.4.1.11129.2.4.2),
//! the TLS signed_certificate_timestamp extension (1.2 ServerHello / 1.3 leaf CertificateEntry), and the stapled
//! OCSP response's single extension (…2.4.5). Verifying an SCT needs the log's public key from a log list
//! (Chrome's / Apple's), which this core does not carry — the ceiling, stated in TLSCORE2.md.

use alloc::vec::Vec;

/// Where an SCT came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SctSource {
    Embedded,
    Tls,
    Ocsp,
}

/// One SignedCertificateTimestamp (v1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sct {
    pub source: SctSource,
    pub version: u8,
    pub log_id: [u8; 32],
    /// Milliseconds since the epoch.
    pub timestamp: u64,
    pub extensions_len: usize,
    /// digitally-signed: HashAlgorithm and SignatureAlgorithm codes (RFC 5246 §7.4.1.4.1).
    pub hash_alg: u8,
    pub sig_alg: u8,
    pub signature_len: usize,
}

/// Parses a TLS-encoded SignedCertificateTimestampList. Malformed lists yield `None` (an SCT is informational:
/// a broken list is reported as absent rather than failing the connection). Unknown versions are skipped.
pub fn parse_list(list: &[u8], source: SctSource) -> Option<Vec<Sct>> {
    let mut r = crate::codec::Reader::new(list);
    let mut items = crate::codec::Reader::new(r.vec16().ok()?);
    r.expect_end().ok()?;
    let mut out = Vec::new();
    while !items.is_empty() {
        let one = items.vec16().ok()?;
        let mut s = crate::codec::Reader::new(one);
        let version = s.u8().ok()?;
        if version != 0 {
            continue; // v1 is 0; RFC 9162's v2 travels differently
        }
        let mut log_id = [0u8; 32];
        log_id.copy_from_slice(s.take(32).ok()?);
        let ts = s.take(8).ok()?;
        let timestamp = u64::from_be_bytes(ts.try_into().ok()?);
        let extensions_len = s.vec16().ok()?.len();
        let hash_alg = s.u8().ok()?;
        let sig_alg = s.u8().ok()?;
        let signature_len = s.vec16().ok()?.len();
        s.expect_end().ok()?;
        out.push(Sct { source, version, log_id, timestamp, extensions_len, hash_alg, sig_alg, signature_len });
    }
    Some(out)
}
