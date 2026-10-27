//! CHARTER: tls_core — shared-core. The DER / X.509 WRITER (PDFSIGN, LEDGER SR350): the twin of [`super::der`]'s strict
//! reader. DER pieces (X.690 §8 / §10, definite lengths through [`encode_tlv`], SET OF sorted), RFC 5280 Time, a
//! one-CN Name, AlgorithmIdentifiers, SubjectPublicKeyInfo for RSA and P-256, and a minimal v3 certificate (a CA or an
//! end entity) whose signature the CALLER makes: tls_core holds no private key and its crypto is behind
//! `CryptoProvider`. First consumers: pdf_core's CMS writer (`pdf_core::sig::cms`) and the pdf_core signature KATs.

use alloc::vec::Vec;

use super::der::{encode_tlv, tag};
use super::oid;

/// A SEQUENCE of already-encoded parts.
pub fn seq(parts: &[&[u8]]) -> Vec<u8> {
    encode_tlv(tag::SEQUENCE, &parts.concat())
}

/// A SET of already-encoded parts, in the order given (one element, or a caller-ordered set).
pub fn set(parts: &[&[u8]]) -> Vec<u8> {
    encode_tlv(tag::SET, &parts.concat())
}

/// The CONTENT of a DER SET OF: the encodings sorted ascending (X.690 §11.6), concatenated.
pub fn set_of_content(mut items: Vec<Vec<u8>>) -> Vec<u8> {
    items.sort();
    items.concat()
}

/// A DER SET OF (X.690 §11.6: the elements sorted by their encodings).
pub fn set_of(items: Vec<Vec<u8>>) -> Vec<u8> {
    encode_tlv(tag::SET, &set_of_content(items))
}

pub fn oid(v: &[u8]) -> Vec<u8> {
    encode_tlv(tag::OID, v)
}

pub fn null() -> Vec<u8> {
    encode_tlv(tag::NULL, &[])
}

pub fn octets(v: &[u8]) -> Vec<u8> {
    encode_tlv(tag::OCTET_STRING, v)
}

pub fn utf8(s: &str) -> Vec<u8> {
    encode_tlv(tag::UTF8_STRING, s.as_bytes())
}

/// A non-negative INTEGER from big-endian magnitude bytes (leading zeros dropped, a 0x00 added before a high bit).
pub fn int(magnitude: &[u8]) -> Vec<u8> {
    let mut m: Vec<u8> = magnitude.iter().copied().skip_while(|&b| b == 0).collect();
    if m.is_empty() || m[0] & 0x80 != 0 {
        m.insert(0, 0);
    }
    encode_tlv(tag::INTEGER, &m)
}

/// A BIT STRING with no unused bits.
pub fn bits(v: &[u8]) -> Vec<u8> {
    let mut b = alloc::vec![0u8];
    b.extend_from_slice(v);
    encode_tlv(tag::BIT_STRING, &b)
}

/// `[n]` constructed (EXPLICIT, or an IMPLICIT SET / SEQUENCE: the same bytes) around `content`.
pub fn explicit(n: u8, content: &[u8]) -> Vec<u8> {
    encode_tlv(tag::context_constructed(n), content)
}

/// Seconds since the epoch as (year, month, day, hour, minute, second), proleptic Gregorian (Howard Hinnant's
/// civil_from_days: the inverse of the reader's `days_from_civil`).
pub fn civil(epoch: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = epoch.div_euclid(86400);
    let secs = epoch.rem_euclid(86400);
    let z = days + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    let y = yoe + era * 400 + if m <= 2 { 1 } else { 0 };
    (y, m, d, (secs / 3600) as u32, (secs % 3600 / 60) as u32, (secs % 60) as u32)
}

fn push2(out: &mut Vec<u8>, v: u32) {
    out.push(b'0' + (v / 10 % 10) as u8);
    out.push(b'0' + (v % 10) as u8);
}

/// An RFC 5280 §4.1.2.5 Time: UTCTime YYMMDDHHMMSSZ for 1950..=2049, else GeneralizedTime YYYYMMDDHHMMSSZ (the
/// forms [`super::der::parse_time`] reads). A year outside 0..=9999 is clamped.
pub fn time(epoch: i64) -> Vec<u8> {
    let (y, mo, d, h, mi, s) = civil(epoch);
    let y = y.clamp(0, 9999) as u32;
    let mut v = Vec::with_capacity(15);
    let t = if (1950..=2049).contains(&y) {
        push2(&mut v, y % 100);
        tag::UTC_TIME
    } else {
        push2(&mut v, y / 100);
        push2(&mut v, y % 100);
        tag::GENERALIZED_TIME
    };
    for x in [mo, d, h, mi, s] {
        push2(&mut v, x);
    }
    v.push(b'Z');
    encode_tlv(t, &v)
}

/// A Name of one RDN: CN = `cn` (UTF8String).
pub fn name_cn(cn: &str) -> Vec<u8> {
    seq(&[&set(&[&seq(&[&oid(oid::AT_COMMON_NAME), &utf8(cn)])])])
}

/// An AlgorithmIdentifier: the OID, then NULL parameters (RSA) or none (ECDSA).
pub fn alg(id: &[u8], null_params: bool) -> Vec<u8> {
    if null_params { seq(&[&oid(id), &null()]) } else { seq(&[&oid(id)]) }
}

/// SubjectPublicKeyInfo for an RSA key from its big-endian modulus and exponent.
pub fn spki_rsa(n: &[u8], e: &[u8]) -> Vec<u8> {
    seq(&[&alg(oid::RSA_ENCRYPTION, true), &bits(&seq(&[&int(n), &int(e)]))])
}

/// SubjectPublicKeyInfo for a P-256 key from its SEC 1 point.
pub fn spki_p256(point: &[u8]) -> Vec<u8> {
    seq(&[&seq(&[&oid(oid::EC_PUBLIC_KEY), &oid(oid::PRIME256V1)]), &bits(point)])
}

/// What a minimal certificate says.
pub struct CertSpec<'a> {
    /// The serial number's big-endian magnitude.
    pub serial: &'a [u8],
    /// The issuer's CN.
    pub issuer: &'a str,
    /// The subject's CN.
    pub subject: &'a str,
    /// Validity, seconds since the epoch.
    pub not_before: i64,
    pub not_after: i64,
    /// The subject's SubjectPublicKeyInfo, DER.
    pub spki: &'a [u8],
    /// A CA (basicConstraints cA, keyUsage keyCertSign + cRLSign) or an end entity (keyUsage digitalSignature).
    pub ca: bool,
}

fn ext(id: u8, value: &[u8]) -> Vec<u8> {
    let crit = encode_tlv(tag::BOOLEAN, &[0xff]);
    seq(&[&oid(&[oid::ID_CE[0], oid::ID_CE[1], id]), &crit, &octets(value)])
}

/// The TBSCertificate (v3) under the issuer's signature AlgorithmIdentifier `sig_alg`; extensions critical.
pub fn tbs(spec: &CertSpec<'_>, sig_alg: &[u8]) -> Vec<u8> {
    let exts = if spec.ca {
        let bc = ext(oid::CE_BASIC_CONSTRAINTS, &seq(&[&encode_tlv(tag::BOOLEAN, &[0xff])]));
        // keyCertSign (bit 5) + cRLSign (bit 6): 0x06 with one unused bit.
        let ku = ext(oid::CE_KEY_USAGE, &encode_tlv(tag::BIT_STRING, &[0x01, 0x06]));
        seq(&[&bc, &ku])
    } else {
        // digitalSignature (bit 0): 0x80 with seven unused bits.
        seq(&[&ext(oid::CE_KEY_USAGE, &encode_tlv(tag::BIT_STRING, &[0x07, 0x80]))])
    };
    seq(&[
        &explicit(0, &int(&[2])),
        &int(spec.serial),
        sig_alg,
        &name_cn(spec.issuer),
        &seq(&[&time(spec.not_before), &time(spec.not_after)]),
        &name_cn(spec.subject),
        spec.spki,
        &explicit(3, &exts),
    ])
}

/// The Certificate: `tbs`, the AlgorithmIdentifier, and the caller's signature over `tbs` as a BIT STRING.
pub fn certificate(tbs: &[u8], sig_alg: &[u8], signature: &[u8]) -> Vec<u8> {
    seq(&[tbs, sig_alg, &bits(signature)])
}
