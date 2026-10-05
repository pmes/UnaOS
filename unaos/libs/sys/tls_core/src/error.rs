//! Errors and TLS alerts (RFC 8446 §6).

use crate::crypto::CryptoError;

/// AlertDescription values (RFC 8446 §6, plus the TLS 1.2 values a 1.3 stack still recognises).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum AlertDescription {
    CloseNotify = 0,
    UnexpectedMessage = 10,
    BadRecordMac = 20,
    RecordOverflow = 22,
    HandshakeFailure = 40,
    BadCertificate = 42,
    UnsupportedCertificate = 43,
    CertificateRevoked = 44,
    CertificateExpired = 45,
    CertificateUnknown = 46,
    IllegalParameter = 47,
    UnknownCa = 48,
    AccessDenied = 49,
    DecodeError = 50,
    DecryptError = 51,
    ProtocolVersion = 70,
    InsufficientSecurity = 71,
    InternalError = 80,
    InappropriateFallback = 86,
    UserCanceled = 90,
    MissingExtension = 109,
    UnsupportedExtension = 110,
    UnrecognizedName = 112,
    BadCertificateStatusResponse = 113,
    UnknownPskIdentity = 115,
    CertificateRequired = 116,
    NoApplicationProtocol = 120,
}

impl AlertDescription {
    pub fn from_u8(v: u8) -> Option<Self> {
        use AlertDescription::*;
        Some(match v {
            0 => CloseNotify,
            10 => UnexpectedMessage,
            20 => BadRecordMac,
            22 => RecordOverflow,
            40 => HandshakeFailure,
            42 => BadCertificate,
            43 => UnsupportedCertificate,
            44 => CertificateRevoked,
            45 => CertificateExpired,
            46 => CertificateUnknown,
            47 => IllegalParameter,
            48 => UnknownCa,
            49 => AccessDenied,
            50 => DecodeError,
            51 => DecryptError,
            70 => ProtocolVersion,
            71 => InsufficientSecurity,
            80 => InternalError,
            86 => InappropriateFallback,
            90 => UserCanceled,
            109 => MissingExtension,
            110 => UnsupportedExtension,
            112 => UnrecognizedName,
            113 => BadCertificateStatusResponse,
            115 => UnknownPskIdentity,
            116 => CertificateRequired,
            120 => NoApplicationProtocol,
            _ => return None,
        })
    }
}

/// Why certificate validation failed (RFC 5280 §6 / RFC 6125).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CertError {
    /// DER did not parse as an RFC 5280 certificate.
    BadDer(&'static str),
    /// The server sent no certificate.
    NoCertificate,
    /// No path from the leaf to any trust anchor.
    UnknownIssuer,
    /// A certificate in the path is outside its validity window.
    Expired,
    NotYetValid,
    /// A signature in the path did not verify.
    BadSignature,
    /// The signature algorithm is not one tls_core or the provider handles.
    UnsupportedSignatureAlgorithm,
    /// An intermediate is not a CA (BasicConstraints) or may not sign certificates (KeyUsage).
    NotCa,
    /// pathLenConstraint exceeded.
    PathLenExceeded,
    /// KeyUsage / ExtendedKeyUsage forbid this use.
    KeyUsage,
    /// A name constraint excludes (or fails to permit) a name in the leaf.
    NameConstraint,
    /// A critical extension tls_core does not process.
    UnknownCriticalExtension,
    /// The leaf does not cover the requested server name.
    NameMismatch,
    /// Path longer than the builder will search.
    PathTooLong,
    /// The verifier was configured to reject everything (e.g. empty trust store).
    NoTrustAnchors,
}

/// Every failure tls_core can report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TlsError {
    /// The transport failed or closed mid-record.
    Transport,
    /// The peer closed the transport without close_notify.
    UnexpectedEof,
    /// Malformed message.
    Decode(&'static str),
    /// A protocol rule was broken; carries the alert tls_core sends.
    Protocol(AlertDescription, &'static str),
    /// The peer sent a fatal alert.
    PeerAlert(AlertDescription),
    /// The peer sent an alert code we do not know.
    PeerAlertUnknown(u8),
    /// A cryptographic operation failed (or the provider lacks it).
    Crypto(CryptoError),
    /// Certificate validation failed.
    Certificate(CertError),
    /// AEAD open failed (bad_record_mac).
    BadRecordMac,
    /// The connection is closed.
    Closed,
    /// API misuse.
    State(&'static str),
}

impl TlsError {
    /// The alert to send for this error, if any.
    pub fn alert(&self) -> Option<AlertDescription> {
        Some(match self {
            TlsError::Transport | TlsError::UnexpectedEof | TlsError::Closed | TlsError::State(_) => return None,
            TlsError::PeerAlert(_) | TlsError::PeerAlertUnknown(_) => return None,
            TlsError::Decode(_) => AlertDescription::DecodeError,
            TlsError::Protocol(a, _) => *a,
            TlsError::Crypto(CryptoError::BadSignature) => AlertDescription::DecryptError,
            TlsError::Crypto(CryptoError::Unsupported(_)) => AlertDescription::HandshakeFailure,
            TlsError::Crypto(_) => AlertDescription::InternalError,
            TlsError::Certificate(c) => match c {
                CertError::Expired | CertError::NotYetValid => AlertDescription::CertificateExpired,
                CertError::UnknownIssuer | CertError::NoTrustAnchors => AlertDescription::UnknownCa,
                CertError::UnsupportedSignatureAlgorithm => AlertDescription::UnsupportedCertificate,
                CertError::BadDer(_) => AlertDescription::BadCertificate,
                CertError::NoCertificate => AlertDescription::DecodeError,
                _ => AlertDescription::BadCertificate,
            },
            TlsError::BadRecordMac => AlertDescription::BadRecordMac,
        })
    }
}

impl From<CryptoError> for TlsError {
    fn from(e: CryptoError) -> Self {
        TlsError::Crypto(e)
    }
}

impl From<CertError> for TlsError {
    fn from(e: CertError) -> Self {
        TlsError::Certificate(e)
    }
}
