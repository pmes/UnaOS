//! Certificate Transparency SCTs (RFC 6962 §3.2–3.3): the wire form. Verification against a log list and
//! Chrome's CT policy live in [`crate::ct`] (CTCORE, SR60).
//!
//! Three places carry a SignedCertificateTimestampList: the leaf's X.509 extension (1.3.6.1.4.1.11129.2.4.2),
//! the TLS signed_certificate_timestamp extension (1.2 ServerHello / 1.3 leaf CertificateEntry), and the stapled
//! OCSP response's single extension (…2.4.5).

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
    /// CtExtensions (opaque; RFC 6962 defines none, static-ct-api logs put a leaf_index here).
    pub extensions: Vec<u8>,
    /// The digitally-signed signature bytes (DER ECDSA-Sig-Value or an RSASSA-PKCS1-v1_5 block).
    pub signature: Vec<u8>,
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
        let extensions = s.vec16().ok()?.to_vec();
        let hash_alg = s.u8().ok()?;
        let sig_alg = s.u8().ok()?;
        let signature = s.vec16().ok()?.to_vec();
        s.expect_end().ok()?;
        out.push(Sct {
            source,
            version,
            log_id,
            timestamp,
            extensions_len: extensions.len(),
            hash_alg,
            sig_alg,
            signature_len: signature.len(),
            extensions,
            signature,
        });
    }
    Some(out)
}
