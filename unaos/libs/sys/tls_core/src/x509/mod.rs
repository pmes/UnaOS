//! X.509 (RFC 5280) for TLS server authentication: a strict DER reader, certificate parsing, path building and
//! validation, RFC 6125 hostname matching, and the PEM trust store.

pub mod cert;
pub mod der;
pub mod name;
pub mod oid;
pub mod pem;
pub mod verify;

pub use cert::{Certificate, PublicKey, SignatureAlgorithm};
pub use verify::{Clock, FixedClock, LoadReport, PinnedLeafVerifier, TrustAnchor, TrustStore, WebPkiVerifier};

use crate::crypto::{CryptoError, CryptoProvider, EcCurve, HashAlg};
use crate::error::{CertError, TlsError};
use crate::msgs::SignatureScheme;

/// Verifies `cert`'s signature with `issuer_key` (RFC 5280 §6.1.3 (a)(1)).
pub fn verify_certificate_signature(
    p: &dyn CryptoProvider,
    issuer_key: &PublicKey,
    cert: &Certificate,
) -> Result<(), CertError> {
    let msg = cert.tbs();
    let sig = &cert.signature;
    let r = match (cert.signature_algorithm, issuer_key) {
        (SignatureAlgorithm::Ecdsa(h), PublicKey::Ec { curve, point }) => p.ecdsa_verify(*curve, h, point, msg, sig),
        (SignatureAlgorithm::Ed25519, PublicKey::Ed25519(k)) => p.ed25519_verify(k, msg, sig),
        (SignatureAlgorithm::RsaPkcs1(h), PublicKey::Rsa { n, e }) => p.rsa_pkcs1_verify(h, n, e, msg, sig),
        (SignatureAlgorithm::RsaPss { hash, salt_len }, PublicKey::Rsa { n, e }) => {
            if salt_len as usize != hash.output_len() {
                return Err(CertError::UnsupportedSignatureAlgorithm);
            }
            p.rsa_pss_verify(hash, n, e, msg, sig)
        }
        (SignatureAlgorithm::Unsupported, _) | (_, PublicKey::Unsupported(_)) => {
            return Err(CertError::UnsupportedSignatureAlgorithm);
        }
        _ => return Err(CertError::BadSignature), // algorithm / key-type mismatch
    };
    match r {
        Ok(()) => Ok(()),
        Err(CryptoError::Unsupported(_)) => Err(CertError::UnsupportedSignatureAlgorithm),
        Err(_) => Err(CertError::BadSignature),
    }
}

/// Verifies a TLS 1.3 signature (CertificateVerify, RFC 8446 §4.4.3) under `scheme` with the leaf key.
pub fn verify_tls_signature(
    p: &dyn CryptoProvider,
    key: &PublicKey,
    scheme: SignatureScheme,
    msg: &[u8],
    sig: &[u8],
) -> Result<(), TlsError> {
    use SignatureScheme as S;
    let mismatch = TlsError::Protocol(
        crate::error::AlertDescription::IllegalParameter,
        "signature scheme does not match the certificate key",
    );
    let r = match (scheme, key) {
        (S::EcdsaSecp256r1Sha256, PublicKey::Ec { curve: EcCurve::P256, point }) => {
            p.ecdsa_verify(EcCurve::P256, HashAlg::Sha256, point, msg, sig)
        }
        (S::EcdsaSecp384r1Sha384, PublicKey::Ec { curve: EcCurve::P384, point }) => {
            p.ecdsa_verify(EcCurve::P384, HashAlg::Sha384, point, msg, sig)
        }
        (S::Ed25519, PublicKey::Ed25519(k)) => p.ed25519_verify(k, msg, sig),
        (S::RsaPssRsaeSha256, PublicKey::Rsa { n, e }) => p.rsa_pss_verify(HashAlg::Sha256, n, e, msg, sig),
        (S::RsaPssRsaeSha384, PublicKey::Rsa { n, e }) => p.rsa_pss_verify(HashAlg::Sha384, n, e, msg, sig),
        (S::RsaPssRsaeSha512, PublicKey::Rsa { n, e }) => p.rsa_pss_verify(HashAlg::Sha512, n, e, msg, sig),
        _ => return Err(mismatch),
    };
    r.map_err(TlsError::Crypto)
}

/// Verifies a TLS 1.2 ServerKeyExchange signature (RFC 5246 §7.4.1.4.1, RFC 8422 §5.4, RFC 8446 §4.2.3 for
/// the rsa_pss_rsae codes). In TLS 1.2 a SignatureAndHashAlgorithm names a hash and a signature KIND — the ECDSA
/// curve is the certificate's, not the code's — and RSASSA-PKCS1-v1_5 is legal. `auth` is the suite's
/// authentication (ECDHE_ECDSA takes an EC or Ed25519 key, ECDHE_RSA an RSA key).
pub fn verify_tls12_signature(
    p: &dyn CryptoProvider,
    key: &PublicKey,
    auth: crate::msgs::Auth12,
    scheme: SignatureScheme,
    msg: &[u8],
    sig: &[u8],
) -> Result<(), TlsError> {
    use crate::msgs::Auth12;
    use SignatureScheme as S;
    let mismatch = TlsError::Protocol(
        crate::error::AlertDescription::IllegalParameter,
        "ServerKeyExchange signature does not match the suite / certificate key",
    );
    let r = match (auth, scheme, key) {
        (Auth12::Ecdsa, S::EcdsaSecp256r1Sha256, PublicKey::Ec { curve, point }) => p.ecdsa_verify(*curve, HashAlg::Sha256, point, msg, sig),
        (Auth12::Ecdsa, S::EcdsaSecp384r1Sha384, PublicKey::Ec { curve, point }) => p.ecdsa_verify(*curve, HashAlg::Sha384, point, msg, sig),
        (Auth12::Ecdsa, S::Ed25519, PublicKey::Ed25519(k)) => p.ed25519_verify(k, msg, sig),
        (Auth12::Rsa, S::RsaPkcs1Sha256, PublicKey::Rsa { n, e }) => p.rsa_pkcs1_verify(HashAlg::Sha256, n, e, msg, sig),
        (Auth12::Rsa, S::RsaPkcs1Sha384, PublicKey::Rsa { n, e }) => p.rsa_pkcs1_verify(HashAlg::Sha384, n, e, msg, sig),
        (Auth12::Rsa, S::RsaPkcs1Sha512, PublicKey::Rsa { n, e }) => p.rsa_pkcs1_verify(HashAlg::Sha512, n, e, msg, sig),
        (Auth12::Rsa, S::RsaPssRsaeSha256, PublicKey::Rsa { n, e }) => p.rsa_pss_verify(HashAlg::Sha256, n, e, msg, sig),
        (Auth12::Rsa, S::RsaPssRsaeSha384, PublicKey::Rsa { n, e }) => p.rsa_pss_verify(HashAlg::Sha384, n, e, msg, sig),
        (Auth12::Rsa, S::RsaPssRsaeSha512, PublicKey::Rsa { n, e }) => p.rsa_pss_verify(HashAlg::Sha512, n, e, msg, sig),
        _ => return Err(mismatch),
    };
    r.map_err(|e| match e {
        CryptoError::BadSignature => TlsError::Protocol(crate::error::AlertDescription::DecryptError, "ServerKeyExchange signature"),
        other => TlsError::Crypto(other),
    })
}
