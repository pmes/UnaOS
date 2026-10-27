//! AETHERWPT3 (LEDGER SR138): the ASN.1 DER (ITU-T X.690 §10) that `crypto.subtle`'s key formats are made of —
//! SubjectPublicKeyInfo (RFC 5280 §4.1.2.7), PrivateKeyInfo (RFC 5208 §5 / RFC 5958), RSAPublicKey and
//! RSAPrivateKey (RFC 8017 A.1), ECPrivateKey (RFC 5915 §3) and the CurvePrivateKey OCTET STRING (RFC 8410 §7).
//!
//! PDFSIGNKEYPARSE (LEDGER SR361): the reader and the writer are tls_core's ONE key-container layer
//! (`tls_core::key`, over its strict `Der` and `x509::build`); what stays here is the JS face's vocabulary — dotted
//! OID strings and [`Params`] — mapped at the edge, every accept / refuse rule and every byte unchanged.

use tls_core::key::{self, AlgParams};
use tls_core::x509::build;

/// AlgorithmIdentifier's parameters as `crypto.subtle` carries them: none, NULL, or a named-curve OID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Params {
    Absent,
    Null,
    Oid(String),
    /// Anything else (explicit curve parameters, RSASSA-PSS-params, …): refused by every caller.
    Other,
}

pub fn oid_to_string(b: &[u8]) -> Option<String> {
    if b.is_empty() || b[b.len() - 1] & 0x80 != 0 {
        return None;
    }
    let mut arcs: Vec<u64> = Vec::new();
    let mut v: u64 = 0;
    let mut fresh = true;
    for &x in b {
        if fresh && x == 0x80 {
            return None; // non-minimal sub-identifier
        }
        fresh = false;
        v = v.checked_mul(128)? | (x & 0x7f) as u64;
        if x & 0x80 == 0 {
            arcs.push(v);
            v = 0;
            fresh = true;
        }
    }
    let first = arcs[0];
    let (a, b0) = if first < 40 { (0, first) } else if first < 80 { (1, first - 40) } else { (2, first - 80) };
    let mut s = format!("{a}.{b0}");
    for x in &arcs[1..] {
        s.push('.');
        s.push_str(&x.to_string());
    }
    Some(s)
}

pub fn oid_from_string(s: &str) -> Vec<u8> {
    let arcs: Vec<u64> = s.split('.').filter_map(|x| x.parse().ok()).collect();
    let mut out = Vec::new();
    if arcs.len() < 2 {
        return out;
    }
    let mut push = |mut v: u64| {
        let mut tmp = vec![(v & 0x7f) as u8];
        v >>= 7;
        while v > 0 {
            tmp.push(0x80 | (v & 0x7f) as u8);
            v >>= 7;
        }
        tmp.reverse();
        out.extend_from_slice(&tmp);
    };
    push(arcs[0] * 40 + arcs[1]);
    for &a in &arcs[2..] {
        push(a);
    }
    out
}

fn params_from(p: AlgParams<'_>) -> Option<Params> {
    Some(match p {
        AlgParams::Absent => Params::Absent,
        AlgParams::Null => Params::Null,
        AlgParams::Oid(b) => Params::Oid(oid_to_string(b)?),
        AlgParams::Other => Params::Other,
    })
}

/// The parameters' OID bytes (empty unless `Params::Oid`), which [`alg_params`] borrows.
fn param_oid(p: &Params) -> Vec<u8> {
    match p {
        Params::Oid(o) => oid_from_string(o),
        _ => Vec::new(),
    }
}

fn alg_params<'a>(p: &Params, oid: &'a [u8]) -> AlgParams<'a> {
    match p {
        Params::Absent => AlgParams::Absent,
        Params::Null => AlgParams::Null,
        Params::Oid(_) => AlgParams::Oid(oid),
        Params::Other => AlgParams::Other,
    }
}

pub fn octets(b: &[u8]) -> Vec<u8> {
    build::octets(b)
}

/// SubjectPublicKeyInfo → (algorithm OID, parameters, subjectPublicKey octets).
pub fn spki_parse(b: &[u8]) -> Option<(String, Params, Vec<u8>)> {
    let (o, p, k) = key::spki(b).ok()?;
    Some((oid_to_string(o)?, params_from(p)?, k.to_vec()))
}

pub fn spki_build(oid_s: &str, params: &Params, key: &[u8]) -> Vec<u8> {
    let po = param_oid(params);
    key::spki_build(&oid_from_string(oid_s), &alg_params(params, &po), key)
}

/// PrivateKeyInfo / OneAsymmetricKey (version 0 or 1; attributes and the v2 public key are skipped) →
/// (algorithm OID, parameters, privateKey octets).
pub fn pkcs8_parse(b: &[u8]) -> Option<(String, Params, Vec<u8>)> {
    let p = key::pkcs8(b).ok()?;
    Some((oid_to_string(p.alg)?, params_from(p.params)?, p.key.to_vec()))
}

pub fn pkcs8_build(oid_s: &str, params: &Params, key: &[u8]) -> Vec<u8> {
    let po = param_oid(params);
    key::pkcs8_build(&oid_from_string(oid_s), &alg_params(params, &po), key)
}

/// RSAPublicKey → (n, e).
pub fn rsa_public_parse(b: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let [n, e] = key::rsa_public(b).ok()?;
    Some((n.to_vec(), e.to_vec()))
}

pub fn rsa_public_build(n: &[u8], e: &[u8]) -> Vec<u8> {
    key::rsa_public_build(n, e)
}

/// RSAPrivateKey (two-prime, version 0) → [n, e, d, p, q, dP, dQ, qInv].
pub fn rsa_private_parse(b: &[u8]) -> Option<Vec<Vec<u8>>> {
    Some(key::rsa_private(b).ok()?.iter().map(|c| c.to_vec()).collect())
}

pub fn rsa_private_build(parts: &[Vec<u8>]) -> Vec<u8> {
    key::rsa_private_build(&parts.iter().map(Vec::as_slice).collect::<Vec<_>>())
}

/// ECPrivateKey (version 1) → (the private key octets, the named curve if `[0]` carries one, the public key
/// if `[1]` carries one).
pub fn ec_private_parse(b: &[u8]) -> Option<(Vec<u8>, Option<String>, Option<Vec<u8>>)> {
    let e = key::ec_private(b).ok()?;
    let curve = match e.curve {
        Some(c) => Some(oid_to_string(c)?),
        None => None,
    };
    Some((e.d.to_vec(), curve, e.public.map(<[u8]>::to_vec)))
}

/// ECPrivateKey with the public key in `[1]` and no `[0]` (the curve is in the PrivateKeyInfo).
pub fn ec_private_build(d: &[u8], public: &[u8]) -> Vec<u8> {
    key::ec_private_build(d, public)
}

/// CurvePrivateKey (RFC 8410 §7): an OCTET STRING.
pub fn octets_parse(b: &[u8]) -> Option<Vec<u8>> {
    key::curve_private(b).ok().map(<[u8]>::to_vec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn der_round_trips() {
        assert_eq!(oid_from_string("1.2.840.10045.2.1"), vec![0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01]);
        assert_eq!(oid_to_string(&[0x2b, 0x65, 0x70]).as_deref(), Some("1.3.101.112"));
        let spki = spki_build("1.3.101.112", &Params::Absent, &[7u8; 32]);
        assert_eq!(&spki[..12], &[0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x70, 0x03, 0x21, 0x00]);
        assert_eq!(spki_parse(&spki), Some(("1.3.101.112".to_string(), Params::Absent, vec![7u8; 32])));
        let big = rsa_public_build(&[0x80; 200], &[1, 0, 1]);
        assert_eq!(rsa_public_parse(&big), Some((vec![0x80; 200], vec![1, 0, 1])));
        assert!(tls_core::x509::der::Der::new(&[0x02, 0x81, 0x01, 0x00]).uint().is_err(), "non-minimal length");
        assert!(tls_core::x509::der::Der::new(&[0x02, 0x02, 0x00, 0x01]).uint().is_err(), "non-minimal integer");
    }
}
