//! Handshake message encoding/decoding (RFC 8446 §4, Appendix B).

use alloc::string::String;
use alloc::vec::Vec;

use crate::codec::*;
use crate::crypto::{AeadAlg, HashAlg};
use crate::error::{AlertDescription, TlsError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ContentType {
    ChangeCipherSpec = 20,
    Alert = 21,
    Handshake = 22,
    ApplicationData = 23,
}

impl ContentType {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            20 => Some(ContentType::ChangeCipherSpec),
            21 => Some(ContentType::Alert),
            22 => Some(ContentType::Handshake),
            23 => Some(ContentType::ApplicationData),
            _ => None,
        }
    }
}

pub mod hs {
    /// TLS 1.2 only (RFC 5246 §7.4.1.1): the server asks for renegotiation — refused.
    pub const HELLO_REQUEST: u8 = 0;
    pub const CLIENT_HELLO: u8 = 1;
    pub const SERVER_HELLO: u8 = 2;
    pub const NEW_SESSION_TICKET: u8 = 4;
    pub const END_OF_EARLY_DATA: u8 = 5;
    pub const ENCRYPTED_EXTENSIONS: u8 = 8;
    pub const CERTIFICATE: u8 = 11;
    pub const CERTIFICATE_REQUEST: u8 = 13;
    /// TLS 1.2 (RFC 5246 §7.4.3 / RFC 8422 §5.4).
    pub const SERVER_KEY_EXCHANGE: u8 = 12;
    /// TLS 1.2 (RFC 5246 §7.4.5).
    pub const SERVER_HELLO_DONE: u8 = 14;
    pub const CERTIFICATE_VERIFY: u8 = 15;
    /// TLS 1.2 (RFC 5246 §7.4.7 / RFC 8422 §5.7).
    pub const CLIENT_KEY_EXCHANGE: u8 = 16;
    pub const FINISHED: u8 = 20;
    /// TLS 1.2 stapled OCSP (RFC 6066 §8).
    pub const CERTIFICATE_STATUS: u8 = 22;
    pub const KEY_UPDATE: u8 = 24;
    pub const MESSAGE_HASH: u8 = 254;
}

pub mod ext {
    pub const SERVER_NAME: u16 = 0;
    pub const MAX_FRAGMENT_LENGTH: u16 = 1;
    pub const STATUS_REQUEST: u16 = 5;
    pub const SUPPORTED_GROUPS: u16 = 10;
    pub const SIGNATURE_ALGORITHMS: u16 = 13;
    pub const USE_SRTP: u16 = 14;
    pub const HEARTBEAT: u16 = 15;
    /// RFC 8422 §5.1.2 (TLS 1.2): we offer `uncompressed` only.
    pub const EC_POINT_FORMATS: u16 = 11;
    pub const ALPN: u16 = 16;
    pub const SCT: u16 = 18;
    pub const CLIENT_CERT_TYPE: u16 = 19;
    pub const SERVER_CERT_TYPE: u16 = 20;
    pub const PADDING: u16 = 21;
    /// RFC 7627 (TLS 1.2): required by this client.
    pub const EXTENDED_MASTER_SECRET: u16 = 23;
    pub const RECORD_SIZE_LIMIT: u16 = 28;
    /// RFC 5077 (TLS 1.2 tickets): never offered.
    pub const SESSION_TICKET: u16 = 35;
    pub const PRE_SHARED_KEY: u16 = 41;
    pub const EARLY_DATA: u16 = 42;
    pub const SUPPORTED_VERSIONS: u16 = 43;
    pub const COOKIE: u16 = 44;
    pub const PSK_KEY_EXCHANGE_MODES: u16 = 45;
    pub const CERTIFICATE_AUTHORITIES: u16 = 47;
    pub const OID_FILTERS: u16 = 48;
    pub const POST_HANDSHAKE_AUTH: u16 = 49;
    pub const SIGNATURE_ALGORITHMS_CERT: u16 = 50;
    pub const KEY_SHARE: u16 = 51;
    /// RFC 5746 (TLS 1.2): must come back empty — renegotiation is never performed.
    pub const RENEGOTIATION_INFO: u16 = 0xff01;
}

/// TLS_EMPTY_RENEGOTIATION_INFO_SCSV (RFC 5746 §3.3): signals RFC 5746 support in a TLS 1.2 ClientHello.
pub const EMPTY_RENEGOTIATION_INFO_SCSV: u16 = 0x00ff;

/// RFC 8446 §4.1.3 downgrade sentinels: the last 8 bytes of a TLS 1.3-capable server's random when it
/// negotiates TLS 1.2 (`…01`) or TLS 1.1 and below (`…00`).
pub const DOWNGRADE_TLS12: [u8; 8] = *b"DOWNGRD\x01";
pub const DOWNGRADE_TLS11: [u8; 8] = *b"DOWNGRD\x00";

pub const TLS13: u16 = 0x0304;
pub const TLS12: u16 = 0x0303;

/// RFC 8446 §4.1.3: SHA-256("HelloRetryRequest").
pub const HRR_RANDOM: [u8; 32] = [
    0xCF, 0x21, 0xAD, 0x74, 0xE5, 0x9A, 0x61, 0x11, 0xBE, 0x1D, 0x8C, 0x02, 0x1E, 0x65, 0xB8, 0x91, 0xC2, 0xA2, 0x11,
    0x16, 0x7A, 0xBB, 0x8C, 0x5E, 0x07, 0x9E, 0x09, 0xE2, 0xC8, 0xA8, 0x33, 0x9C,
];

/// Cipher suites: the three TLS 1.3 suites (RFC 8446 B.4) and the six TLS 1.2 ECDHE + AEAD suites this client
/// speaks (RFC 5289 AES-GCM, RFC 7905 ChaCha20-Poly1305). Deliberately absent from TLS 1.2: CBC (Lucky13 /
/// padding oracles), static-RSA key exchange (no forward secrecy, Bleichenbacher), finite-field DHE, RC4, 3DES,
/// NULL and export suites.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CipherSuite {
    Aes128GcmSha256,
    Aes256GcmSha384,
    ChaCha20Poly1305Sha256,
    /// TLS_ECDHE_ECDSA_WITH_AES_128_GCM_SHA256 (0xC02B)
    EcdheEcdsaAes128GcmSha256,
    /// TLS_ECDHE_RSA_WITH_AES_128_GCM_SHA256 (0xC02F)
    EcdheRsaAes128GcmSha256,
    /// TLS_ECDHE_ECDSA_WITH_AES_256_GCM_SHA384 (0xC02C)
    EcdheEcdsaAes256GcmSha384,
    /// TLS_ECDHE_RSA_WITH_AES_256_GCM_SHA384 (0xC030)
    EcdheRsaAes256GcmSha384,
    /// TLS_ECDHE_ECDSA_WITH_CHACHA20_POLY1305_SHA256 (0xCCA9)
    EcdheEcdsaChaCha20Poly1305Sha256,
    /// TLS_ECDHE_RSA_WITH_CHACHA20_POLY1305_SHA256 (0xCCA8)
    EcdheRsaChaCha20Poly1305Sha256,
}

/// Who signs a TLS 1.2 ECDHE ServerKeyExchange.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth12 {
    /// ECDSA or (RFC 8422 §5.1.3) Ed25519.
    Ecdsa,
    Rsa,
}

impl CipherSuite {
    /// The TLS 1.3 suites (the historical name: `ClientConfig::new` offers these).
    pub const ALL: [CipherSuite; 3] =
        [CipherSuite::Aes128GcmSha256, CipherSuite::Aes256GcmSha384, CipherSuite::ChaCha20Poly1305Sha256];
    pub const TLS13: [CipherSuite; 3] = Self::ALL;
    /// The TLS 1.2 suites, in preference order (AEAD only, ECDHE only).
    pub const TLS12: [CipherSuite; 6] = [
        CipherSuite::EcdheEcdsaAes128GcmSha256,
        CipherSuite::EcdheRsaAes128GcmSha256,
        CipherSuite::EcdheEcdsaChaCha20Poly1305Sha256,
        CipherSuite::EcdheRsaChaCha20Poly1305Sha256,
        CipherSuite::EcdheEcdsaAes256GcmSha384,
        CipherSuite::EcdheRsaAes256GcmSha384,
    ];
    pub fn code(self) -> u16 {
        match self {
            CipherSuite::Aes128GcmSha256 => 0x1301,
            CipherSuite::Aes256GcmSha384 => 0x1302,
            CipherSuite::ChaCha20Poly1305Sha256 => 0x1303,
            CipherSuite::EcdheEcdsaAes128GcmSha256 => 0xc02b,
            CipherSuite::EcdheRsaAes128GcmSha256 => 0xc02f,
            CipherSuite::EcdheEcdsaAes256GcmSha384 => 0xc02c,
            CipherSuite::EcdheRsaAes256GcmSha384 => 0xc030,
            CipherSuite::EcdheEcdsaChaCha20Poly1305Sha256 => 0xcca9,
            CipherSuite::EcdheRsaChaCha20Poly1305Sha256 => 0xcca8,
        }
    }
    pub fn from_code(c: u16) -> Option<Self> {
        Self::TLS13.iter().chain(Self::TLS12.iter()).copied().find(|s| s.code() == c)
    }
    pub fn is_tls13(self) -> bool {
        matches!(self, CipherSuite::Aes128GcmSha256 | CipherSuite::Aes256GcmSha384 | CipherSuite::ChaCha20Poly1305Sha256)
    }
    /// TLS 1.2: who signs the ServerKeyExchange (`None` for a TLS 1.3 suite).
    pub fn auth12(self) -> Option<Auth12> {
        match self {
            CipherSuite::EcdheEcdsaAes128GcmSha256
            | CipherSuite::EcdheEcdsaAes256GcmSha384
            | CipherSuite::EcdheEcdsaChaCha20Poly1305Sha256 => Some(Auth12::Ecdsa),
            CipherSuite::EcdheRsaAes128GcmSha256
            | CipherSuite::EcdheRsaAes256GcmSha384
            | CipherSuite::EcdheRsaChaCha20Poly1305Sha256 => Some(Auth12::Rsa),
            _ => None,
        }
    }
    /// The transcript / key-schedule hash (TLS 1.3) or PRF hash (TLS 1.2).
    pub fn hash(self) -> HashAlg {
        match self {
            CipherSuite::Aes256GcmSha384 | CipherSuite::EcdheEcdsaAes256GcmSha384 | CipherSuite::EcdheRsaAes256GcmSha384 => {
                HashAlg::Sha384
            }
            _ => HashAlg::Sha256,
        }
    }
    pub fn aead(self) -> AeadAlg {
        match self {
            CipherSuite::Aes128GcmSha256 | CipherSuite::EcdheEcdsaAes128GcmSha256 | CipherSuite::EcdheRsaAes128GcmSha256 => {
                AeadAlg::Aes128Gcm
            }
            CipherSuite::Aes256GcmSha384 | CipherSuite::EcdheEcdsaAes256GcmSha384 | CipherSuite::EcdheRsaAes256GcmSha384 => {
                AeadAlg::Aes256Gcm
            }
            _ => AeadAlg::ChaCha20Poly1305,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamedGroup {
    X25519,
    Secp256r1,
}

impl NamedGroup {
    pub fn code(self) -> u16 {
        match self {
            NamedGroup::X25519 => 0x001d,
            NamedGroup::Secp256r1 => 0x0017,
        }
    }
    pub fn from_code(c: u16) -> Option<Self> {
        match c {
            0x001d => Some(NamedGroup::X25519),
            0x0017 => Some(NamedGroup::Secp256r1),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureScheme {
    EcdsaSecp256r1Sha256,
    EcdsaSecp384r1Sha384,
    Ed25519,
    RsaPssRsaeSha256,
    RsaPssRsaeSha384,
    RsaPssRsaeSha512,
    RsaPkcs1Sha256,
    RsaPkcs1Sha384,
    RsaPkcs1Sha512,
}

impl SignatureScheme {
    pub const ALL: [SignatureScheme; 9] = [
        SignatureScheme::EcdsaSecp256r1Sha256,
        SignatureScheme::Ed25519,
        SignatureScheme::EcdsaSecp384r1Sha384,
        SignatureScheme::RsaPssRsaeSha256,
        SignatureScheme::RsaPssRsaeSha384,
        SignatureScheme::RsaPssRsaeSha512,
        SignatureScheme::RsaPkcs1Sha256,
        SignatureScheme::RsaPkcs1Sha384,
        SignatureScheme::RsaPkcs1Sha512,
    ];
    pub fn code(self) -> u16 {
        match self {
            SignatureScheme::EcdsaSecp256r1Sha256 => 0x0403,
            SignatureScheme::EcdsaSecp384r1Sha384 => 0x0503,
            SignatureScheme::Ed25519 => 0x0807,
            SignatureScheme::RsaPssRsaeSha256 => 0x0804,
            SignatureScheme::RsaPssRsaeSha384 => 0x0805,
            SignatureScheme::RsaPssRsaeSha512 => 0x0806,
            SignatureScheme::RsaPkcs1Sha256 => 0x0401,
            SignatureScheme::RsaPkcs1Sha384 => 0x0501,
            SignatureScheme::RsaPkcs1Sha512 => 0x0601,
        }
    }
    pub fn from_code(c: u16) -> Option<Self> {
        Self::ALL.iter().copied().find(|s| s.code() == c)
    }
    /// RSASSA-PKCS1-v1_5 is never valid in a TLS 1.3 CertificateVerify (RFC 8446 §4.2.3).
    pub fn allowed_in_certificate_verify(self) -> bool {
        !matches!(self, SignatureScheme::RsaPkcs1Sha256 | SignatureScheme::RsaPkcs1Sha384 | SignatureScheme::RsaPkcs1Sha512)
    }
}

/// Wraps a handshake body: msg_type(1) || length(3) || body.
pub fn handshake_message(msg_type: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + body.len());
    out.push(msg_type);
    put_vec24(&mut out, body);
    out
}

fn put_ext(out: &mut Vec<u8>, ty: u16, f: impl FnOnce(&mut Vec<u8>)) {
    put_u16(out, ty);
    put_len_prefixed(out, 2, f);
}

/// The parameters a ClientHello is built from.
#[derive(Debug, Clone)]
pub struct ClientHelloParams<'a> {
    pub random: [u8; 32],
    pub session_id: &'a [u8],
    pub cipher_suites: &'a [CipherSuite],
    pub server_name: Option<&'a str>,
    pub groups: &'a [NamedGroup],
    pub key_shares: &'a [(NamedGroup, &'a [u8])],
    pub signature_schemes: &'a [SignatureScheme],
    pub alpn: &'a [Vec<u8>],
    pub cookie: Option<&'a [u8]>,
    /// supported_versions, preference order (`[TLS13]`, `[TLS13, TLS12]`); empty = omit (a TLS 1.2-only hello).
    pub versions: &'a [u16],
    /// TLS 1.2 offered: extended_master_secret, ec_point_formats and the renegotiation SCSV are added.
    pub tls12: bool,
    /// status_request (OCSP stapling, RFC 6066 §8 / RFC 8446 §4.4.2.1).
    pub status_request: bool,
    /// signed_certificate_timestamp (RFC 6962 §3.3.1).
    pub sct: bool,
    /// psk_key_exchange_modes = [psk_dhe_ke] (RFC 8446 §4.2.9): sent when the caller keeps tickets.
    pub psk_dhe_mode: bool,
    /// pre_shared_key (RFC 8446 §4.2.11), always the LAST extension; its binder is written as zeros and patched
    /// by the caller over the truncated hello ([`psk_binders_len`]).
    pub psk: Option<PskOffer<'a>>,
}

/// One PSK identity offered for resumption.
#[derive(Debug, Clone, Copy)]
pub struct PskOffer<'a> {
    pub identity: &'a [u8],
    pub obfuscated_ticket_age: u32,
    pub binder_len: usize,
}

/// Length of the `binders` vector (its 2-byte length included) at the end of a hello carrying one PSK whose binder
/// is `binder_len` bytes: the part of the message excluded from the binder's transcript (RFC 8446 §4.2.11.2).
pub const fn psk_binders_len(binder_len: usize) -> usize {
    2 + 1 + binder_len
}

/// Encodes a full ClientHello handshake message (RFC 8446 §4.1.2).
pub fn encode_client_hello(p: &ClientHelloParams<'_>) -> Vec<u8> {
    let mut body = Vec::with_capacity(512);
    put_u16(&mut body, TLS12); // legacy_version
    body.extend_from_slice(&p.random);
    put_vec8(&mut body, p.session_id);
    put_len_prefixed(&mut body, 2, |o| {
        for cs in p.cipher_suites {
            put_u16(o, cs.code());
        }
        if p.tls12 {
            put_u16(o, EMPTY_RENEGOTIATION_INFO_SCSV);
        }
    });
    put_vec8(&mut body, &[0]); // legacy_compression_methods = { null }
    put_len_prefixed(&mut body, 2, |o| {
        if let Some(name) = p.server_name {
            // RFC 6066 §3: ServerNameList { NameType host_name(0), HostName<1..2^16-1> }
            put_ext(o, ext::SERVER_NAME, |o| {
                put_len_prefixed(o, 2, |o| {
                    o.push(0);
                    put_vec16(o, name.as_bytes());
                })
            });
        }
        put_ext(o, ext::SUPPORTED_GROUPS, |o| {
            put_len_prefixed(o, 2, |o| {
                for g in p.groups {
                    put_u16(o, g.code());
                }
            })
        });
        put_ext(o, ext::SIGNATURE_ALGORITHMS, |o| {
            put_len_prefixed(o, 2, |o| {
                for s in p.signature_schemes {
                    put_u16(o, s.code());
                }
            })
        });
        if !p.alpn.is_empty() {
            put_ext(o, ext::ALPN, |o| {
                put_len_prefixed(o, 2, |o| {
                    for proto in p.alpn {
                        put_vec8(o, proto);
                    }
                })
            });
        }
        if p.status_request {
            // CertificateStatusRequest { status_type ocsp(1), responder_id_list <0>, request_extensions <0> }
            put_ext(o, ext::STATUS_REQUEST, |o| o.extend_from_slice(&[1, 0, 0, 0, 0]));
        }
        if p.sct {
            put_ext(o, ext::SCT, |_| {});
        }
        if p.tls12 {
            put_ext(o, ext::EC_POINT_FORMATS, |o| put_vec8(o, &[0]));
            put_ext(o, ext::EXTENDED_MASTER_SECRET, |_| {});
        }
        if !p.versions.is_empty() {
            put_ext(o, ext::SUPPORTED_VERSIONS, |o| {
                put_len_prefixed(o, 1, |o| {
                    for v in p.versions {
                        put_u16(o, *v);
                    }
                })
            });
        }
        if let Some(c) = p.cookie {
            put_ext(o, ext::COOKIE, |o| put_vec16(o, c));
        }
        if !p.key_shares.is_empty() {
            put_ext(o, ext::KEY_SHARE, |o| {
                put_len_prefixed(o, 2, |o| {
                    for (g, k) in p.key_shares {
                        put_u16(o, g.code());
                        put_vec16(o, k);
                    }
                })
            });
        }
        if p.psk_dhe_mode || p.psk.is_some() {
            put_ext(o, ext::PSK_KEY_EXCHANGE_MODES, |o| put_vec8(o, &[1]));
        }
        if let Some(psk) = &p.psk {
            put_ext(o, ext::PRE_SHARED_KEY, |o| {
                put_len_prefixed(o, 2, |o| {
                    put_vec16(o, psk.identity);
                    o.extend_from_slice(&psk.obfuscated_ticket_age.to_be_bytes());
                });
                put_len_prefixed(o, 2, |o| {
                    o.push(psk.binder_len as u8);
                    o.resize(o.len() + psk.binder_len, 0);
                });
            });
        }
    });
    handshake_message(hs::CLIENT_HELLO, &body)
}

/// What a sent ClientHello offered — parsed back from its bytes, so a substituted hello (a KAT driving the state
/// machine with a trace's exact ClientHello) and a built one are treated the same way.
#[derive(Debug, Clone, Default)]
pub struct OfferedHello {
    pub random: [u8; 32],
    pub session_id: Vec<u8>,
    pub cipher_suites: Vec<u16>,
    pub extensions: Vec<u16>,
    pub groups: Vec<u16>,
    pub key_share_groups: Vec<u16>,
    pub signature_schemes: Vec<u16>,
    pub alpn: Vec<Vec<u8>>,
    /// supported_versions (empty when absent).
    pub versions: Vec<u16>,
    /// pre_shared_key identities offered.
    pub psk_identities: usize,
}

/// Parses a ClientHello handshake message (with its 4-byte header).
pub fn parse_client_hello(msg: &[u8]) -> Result<OfferedHello, TlsError> {
    let mut r = Reader::new(msg);
    if r.u8()? != hs::CLIENT_HELLO {
        return Err(TlsError::Decode("not a ClientHello"));
    }
    let mut r = Reader::new(r.vec24()?);
    let mut o = OfferedHello::default();
    r.u16()?;
    o.random.copy_from_slice(r.take(32)?);
    o.session_id = r.vec8()?.to_vec();
    let mut cs = Reader::new(r.vec16()?);
    while !cs.is_empty() {
        o.cipher_suites.push(cs.u16()?);
    }
    r.vec8()?;
    let mut exts = Reader::new(r.vec16()?);
    while !exts.is_empty() {
        let ty = exts.u16()?;
        let data = exts.vec16()?;
        o.extensions.push(ty);
        let mut d = Reader::new(data);
        match ty {
            ext::SUPPORTED_GROUPS => {
                let mut l = Reader::new(d.vec16()?);
                while !l.is_empty() {
                    o.groups.push(l.u16()?);
                }
            }
            ext::KEY_SHARE => {
                let mut l = Reader::new(d.vec16()?);
                while !l.is_empty() {
                    o.key_share_groups.push(l.u16()?);
                    l.vec16()?;
                }
            }
            ext::SIGNATURE_ALGORITHMS => {
                let mut l = Reader::new(d.vec16()?);
                while !l.is_empty() {
                    o.signature_schemes.push(l.u16()?);
                }
            }
            ext::ALPN => {
                let mut l = Reader::new(d.vec16()?);
                while !l.is_empty() {
                    o.alpn.push(l.vec8()?.to_vec());
                }
            }
            ext::SUPPORTED_VERSIONS => {
                let mut l = Reader::new(d.vec8()?);
                while !l.is_empty() {
                    o.versions.push(l.u16()?);
                }
            }
            ext::PRE_SHARED_KEY => {
                let mut l = Reader::new(d.vec16()?);
                while !l.is_empty() {
                    l.vec16()?;
                    l.u32()?;
                    o.psk_identities += 1;
                }
            }
            _ => {}
        }
    }
    r.expect_end()?;
    Ok(o)
}

/// A parsed ServerHello or HelloRetryRequest (RFC 8446 §4.1.3, §4.1.4).
#[derive(Debug, Clone)]
pub struct ServerHello {
    pub is_hrr: bool,
    pub random: [u8; 32],
    pub session_id: Vec<u8>,
    pub cipher_suite: u16,
    /// supported_versions selected_version (absent → TLS 1.2 or older server).
    pub selected_version: Option<u16>,
    /// ServerHello key_share: (group, key_exchange).
    pub key_share: Option<(u16, Vec<u8>)>,
    /// HelloRetryRequest key_share: selected_group.
    pub hrr_group: Option<u16>,
    pub cookie: Option<Vec<u8>>,
    pub extensions: Vec<u16>,
    pub has_pre_shared_key: bool,
    /// pre_shared_key selected_identity (RFC 8446 §4.2.11).
    pub psk_identity: Option<u16>,
    /// legacy_version as sent.
    pub legacy_version: u16,
    /// Every extension's body, in order (TLS 1.2 extensions are validated by the 1.2 handshake).
    pub ext_data: Vec<(u16, Vec<u8>)>,
}

impl ServerHello {
    pub fn ext(&self, ty: u16) -> Option<&[u8]> {
        self.ext_data.iter().find(|(t, _)| *t == ty).map(|(_, d)| d.as_slice())
    }
}

fn illegal(msg: &'static str) -> TlsError {
    TlsError::Protocol(AlertDescription::IllegalParameter, msg)
}

/// Parses a ServerHello handshake body (without the 4-byte header).
pub fn parse_server_hello(body: &[u8]) -> Result<ServerHello, TlsError> {
    let mut r = Reader::new(body);
    let legacy_version = r.u16()?;
    let mut random = [0u8; 32];
    random.copy_from_slice(r.take(32)?);
    let is_hrr = random == HRR_RANDOM;
    let session_id = r.vec8()?.to_vec();
    let cipher_suite = r.u16()?;
    let compression = r.u8()?;
    let mut sh = ServerHello {
        is_hrr,
        random,
        session_id,
        cipher_suite,
        selected_version: None,
        key_share: None,
        hrr_group: None,
        cookie: None,
        extensions: Vec::new(),
        has_pre_shared_key: false,
        psk_identity: None,
        legacy_version,
        ext_data: Vec::new(),
    };
    if r.is_empty() {
        // A TLS 1.2-or-older ServerHello may omit extensions entirely.
        return Ok(sh);
    }
    let mut exts = Reader::new(r.vec16()?);
    r.expect_end()?;
    while !exts.is_empty() {
        let ty = exts.u16()?;
        let data = exts.vec16()?;
        if sh.extensions.contains(&ty) {
            return Err(illegal("duplicate extension in ServerHello"));
        }
        sh.extensions.push(ty);
        sh.ext_data.push((ty, data.to_vec()));
        let mut d = Reader::new(data);
        match ty {
            ext::SUPPORTED_VERSIONS => {
                sh.selected_version = Some(d.u16()?);
                d.expect_end()?;
            }
            ext::KEY_SHARE => {
                let g = d.u16()?;
                if is_hrr {
                    sh.hrr_group = Some(g);
                } else {
                    sh.key_share = Some((g, d.vec16()?.to_vec()));
                }
                d.expect_end()?;
            }
            ext::COOKIE => {
                if !is_hrr {
                    return Err(TlsError::Protocol(AlertDescription::UnsupportedExtension, "cookie in ServerHello"));
                }
                let c = d.vec16()?;
                if c.is_empty() {
                    return Err(TlsError::Decode("empty cookie"));
                }
                sh.cookie = Some(c.to_vec());
                d.expect_end()?;
            }
            ext::PRE_SHARED_KEY => {
                sh.has_pre_shared_key = true;
                sh.psk_identity = Some(d.u16()?);
                d.expect_end()?;
            }
            _ => {}
        }
    }
    // RFC 8446 §4.2: a TLS 1.3 ServerHello / HelloRetryRequest carries only these. A TLS 1.2 ServerHello's
    // extensions are checked by the 1.2 handshake (RFC 5246 §7.4.1.4: only ones the client offered).
    if is_hrr || sh.selected_version == Some(TLS13) {
        for &ty in &sh.extensions {
            if !matches!(ty, ext::SUPPORTED_VERSIONS | ext::KEY_SHARE | ext::COOKIE | ext::PRE_SHARED_KEY) {
                return Err(TlsError::Protocol(AlertDescription::UnsupportedExtension, "extension not permitted in ServerHello"));
            }
        }
    }
    if legacy_version != TLS12 && sh.selected_version == Some(TLS13) {
        return Err(illegal("legacy_version must be 0x0303"));
    }
    if compression != 0 {
        return Err(illegal("non-null compression"));
    }
    Ok(sh)
}

/// EncryptedExtensions (RFC 8446 §4.3.1): what the client cares about.
#[derive(Debug, Clone, Default)]
pub struct EncryptedExtensions {
    pub extensions: Vec<u16>,
    pub alpn: Option<Vec<u8>>,
}

/// Extensions that may appear in EncryptedExtensions only if the client offered them; any other type tls_core
/// recognises is illegal there (RFC 8446 §4.2 table).
pub fn parse_encrypted_extensions(body: &[u8], offered: &[u16], offered_alpn: &[Vec<u8>]) -> Result<EncryptedExtensions, TlsError> {
    let mut r = Reader::new(body);
    let mut exts = Reader::new(r.vec16()?);
    r.expect_end()?;
    let mut ee = EncryptedExtensions::default();
    while !exts.is_empty() {
        let ty = exts.u16()?;
        let data = exts.vec16()?;
        if ee.extensions.contains(&ty) {
            return Err(illegal("duplicate extension in EncryptedExtensions"));
        }
        ee.extensions.push(ty);
        // Not permitted in EE at all (they belong to ServerHello/Certificate/etc.).
        if matches!(
            ty,
            ext::KEY_SHARE
                | ext::SUPPORTED_VERSIONS
                | ext::PRE_SHARED_KEY
                | ext::COOKIE
                | ext::PSK_KEY_EXCHANGE_MODES
                | ext::SIGNATURE_ALGORITHMS
                | ext::SIGNATURE_ALGORITHMS_CERT
                | ext::STATUS_REQUEST
                | ext::SCT
                | ext::PADDING
                | ext::POST_HANDSHAKE_AUTH
                | ext::OID_FILTERS
                | ext::CERTIFICATE_AUTHORITIES
        ) {
            return Err(illegal("extension not permitted in EncryptedExtensions"));
        }
        if !offered.contains(&ty) {
            return Err(TlsError::Protocol(AlertDescription::UnsupportedExtension, "unsolicited extension"));
        }
        let mut d = Reader::new(data);
        match ty {
            ext::ALPN => {
                let mut l = Reader::new(d.vec16()?);
                let proto = l.vec8()?;
                l.expect_end()?;
                d.expect_end()?;
                if proto.is_empty() || !offered_alpn.iter().any(|p| p.as_slice() == proto) {
                    return Err(illegal("server selected an ALPN protocol not offered"));
                }
                ee.alpn = Some(proto.to_vec());
            }
            ext::SERVER_NAME => {
                if !data.is_empty() {
                    return Err(TlsError::Decode("server_name ack must be empty"));
                }
            }
            _ => {}
        }
    }
    Ok(ee)
}

/// Certificate message (RFC 8446 §4.4.2): the cert_data of every CertificateEntry, leaf first.
pub fn parse_certificate(body: &[u8]) -> Result<Vec<Vec<u8>>, TlsError> {
    Ok(parse_certificate_entries(body, &[])?.chain)
}

/// A server's certificates plus what rides with the leaf: a stapled OCSP response and a TLS-delivered SCT list.
#[derive(Debug, Clone, Default)]
pub struct ServerCertificates {
    pub chain: Vec<Vec<u8>>,
    /// The DER OCSPResponse (RFC 6960 §4.2.1) from CertificateStatus / the leaf's status_request entry.
    pub ocsp: Option<Vec<u8>>,
    /// SignedCertificateTimestampList (RFC 6962 §3.3) from the TLS extension.
    pub sct_list: Option<Vec<u8>>,
}

/// CertificateStatus body (RFC 6066 §8): status_type ocsp(1) || opaque OCSPResponse<1..2^24-1>. The same
/// structure is the extension_data of a TLS 1.3 leaf's status_request entry (RFC 8446 §4.4.2.1).
pub fn parse_certificate_status(body: &[u8]) -> Result<Vec<u8>, TlsError> {
    let mut r = Reader::new(body);
    if r.u8()? != 1 {
        return Err(illegal("CertificateStatus type is not ocsp"));
    }
    let resp = r.vec24()?;
    r.expect_end()?;
    if resp.is_empty() {
        return Err(TlsError::Decode("empty OCSPResponse"));
    }
    Ok(resp.to_vec())
}

/// RFC 8446 §4.4.2 with the per-entry extensions: status_request (OCSP) and signed_certificate_timestamp are
/// read from the leaf when the client offered them (`offered`); any other entry extension, or one not offered, is
/// refused (§4.4.2: "extensions in the Certificate message … MUST correspond to ones from the ClientHello").
pub fn parse_certificate_entries(body: &[u8], offered: &[u16]) -> Result<ServerCertificates, TlsError> {
    let mut r = Reader::new(body);
    let ctx = r.vec8()?;
    if !ctx.is_empty() {
        return Err(illegal("non-empty certificate_request_context from server"));
    }
    let mut list = Reader::new(r.vec24()?);
    r.expect_end()?;
    let mut out = ServerCertificates::default();
    while !list.is_empty() {
        let cert = list.vec24()?;
        if cert.is_empty() {
            return Err(TlsError::Decode("empty cert_data"));
        }
        let mut exts = Reader::new(list.vec16()?);
        let leaf = out.chain.is_empty();
        let mut seen: Vec<u16> = Vec::new();
        while !exts.is_empty() {
            let ty = exts.u16()?;
            let data = exts.vec16()?;
            if seen.contains(&ty) {
                return Err(illegal("duplicate CertificateEntry extension"));
            }
            seen.push(ty);
            if !offered.contains(&ty) && !offered.is_empty() {
                return Err(TlsError::Protocol(AlertDescription::UnsupportedExtension, "unsolicited CertificateEntry extension"));
            }
            match ty {
                ext::STATUS_REQUEST if leaf => out.ocsp = Some(parse_certificate_status(data)?),
                ext::SCT if leaf => {
                    if data.len() < 2 {
                        return Err(TlsError::Decode("empty SCT list"));
                    }
                    out.sct_list = Some(data.to_vec())
                }
                // Intermediates' OCSP / SCTs are not used (RFC 8446 §4.4.2.1 lets a server send them; we verify
                // the leaf's only).
                ext::STATUS_REQUEST | ext::SCT => {}
                _ if offered.is_empty() => {} // legacy call: accepted, not processed
                _ => return Err(illegal("extension not permitted in a CertificateEntry")),
            }
        }
        out.chain.push(cert.to_vec());
    }
    Ok(out)
}

/// TLS 1.2 Certificate (RFC 5246 §7.4.2): certificate_list <0..2^24-1> of ASN.1Cert <1..2^24-1>, leaf first.
pub fn parse_certificate12(body: &[u8]) -> Result<Vec<Vec<u8>>, TlsError> {
    let mut r = Reader::new(body);
    let mut list = Reader::new(r.vec24()?);
    r.expect_end()?;
    let mut out = Vec::new();
    while !list.is_empty() {
        let cert = list.vec24()?;
        if cert.is_empty() {
            return Err(TlsError::Decode("empty ASN.1Cert"));
        }
        out.push(cert.to_vec());
    }
    Ok(out)
}

/// A TLS 1.2 ECDHE ServerKeyExchange (RFC 8422 §5.4 with RFC 5246 §7.4.1.4.1 signatures).
#[derive(Debug, Clone)]
pub struct ServerKeyExchange {
    pub group: u16,
    pub public: Vec<u8>,
    /// ServerECDHParams as sent: curve_type || namedcurve || public (the signed part after the randoms).
    pub params: Vec<u8>,
    pub scheme: u16,
    pub signature: Vec<u8>,
}

pub fn parse_server_key_exchange(body: &[u8]) -> Result<ServerKeyExchange, TlsError> {
    let mut r = Reader::new(body);
    let curve_type = r.u8()?;
    if curve_type != 3 {
        // explicit_prime / explicit_char2 curves were deprecated by RFC 8422 §5.4.
        return Err(illegal("ECParameters curve_type is not named_curve"));
    }
    let group = r.u16()?;
    let public = r.vec8()?.to_vec();
    if public.is_empty() {
        return Err(TlsError::Decode("empty ECPoint"));
    }
    let params = body[..4 + public.len()].to_vec();
    let scheme = r.u16()?;
    let signature = r.vec16()?.to_vec();
    r.expect_end()?;
    Ok(ServerKeyExchange { group, public, params, scheme, signature })
}

/// TLS 1.2 CertificateRequest (RFC 5246 §7.4.4): certificate_types, supported_signature_algorithms,
/// certificate_authorities — only its well-formedness matters (we answer with an empty Certificate).
pub fn parse_certificate_request12(body: &[u8]) -> Result<(), TlsError> {
    let mut r = Reader::new(body);
    r.vec8()?;
    r.vec16()?;
    r.vec16()?;
    r.expect_end()
}

/// CertificateVerify (RFC 8446 §4.4.3).
pub fn parse_certificate_verify(body: &[u8]) -> Result<(u16, Vec<u8>), TlsError> {
    let mut r = Reader::new(body);
    let scheme = r.u16()?;
    let sig = r.vec16()?.to_vec();
    r.expect_end()?;
    Ok((scheme, sig))
}

/// The content covered by a server CertificateVerify signature (RFC 8446 §4.4.3).
pub fn certificate_verify_message(transcript_hash: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(64 + 34 + transcript_hash.len());
    m.resize(64, 0x20);
    m.extend_from_slice(b"TLS 1.3, server CertificateVerify");
    m.push(0);
    m.extend_from_slice(transcript_hash);
    m
}

/// NewSessionTicket (RFC 8446 §4.6.1).
#[derive(Debug, Clone)]
pub struct NewSessionTicket {
    pub lifetime: u32,
    pub age_add: u32,
    pub nonce: Vec<u8>,
    pub ticket: Vec<u8>,
    /// early_data's max_early_data_size (RFC 8446 §4.2.10) when the server would take 0-RTT — recorded, never
    /// used: this client does not send early data.
    pub max_early_data: Option<u32>,
}

pub fn parse_new_session_ticket(body: &[u8]) -> Result<NewSessionTicket, TlsError> {
    let mut r = Reader::new(body);
    let lifetime = r.u32()?;
    let age_add = r.u32()?;
    let nonce = r.vec8()?.to_vec();
    let ticket = r.vec16()?;
    if ticket.is_empty() {
        return Err(TlsError::Decode("empty ticket"));
    }
    let ticket = ticket.to_vec();
    let mut exts = Reader::new(r.vec16()?);
    r.expect_end()?;
    let mut max_early_data = None;
    let mut seen: Vec<u16> = Vec::new();
    while !exts.is_empty() {
        let ty = exts.u16()?;
        let data = exts.vec16()?;
        if seen.contains(&ty) {
            return Err(illegal("duplicate NewSessionTicket extension"));
        }
        seen.push(ty);
        if ty == ext::EARLY_DATA {
            let mut d = Reader::new(data);
            max_early_data = Some(d.u32()?);
            d.expect_end()?;
        }
        // §4.6.1: other (unknown) extensions are ignored by clients.
    }
    Ok(NewSessionTicket { lifetime, age_add, nonce, ticket, max_early_data })
}

/// Normalises a server name for SNI: a DNS name, lower-case, no trailing dot; IP literals are not sent (RFC 6066 §3).
pub fn sni_name(name: &str) -> Option<String> {
    let n = name.strip_suffix('.').unwrap_or(name);
    if n.is_empty() || crate::x509::name::parse_ip(n).is_some() {
        return None;
    }
    Some(n.to_ascii_lowercase())
}
