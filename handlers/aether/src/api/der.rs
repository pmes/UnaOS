//! AETHERWPT3 (LEDGER SR138): the ASN.1 DER (ITU-T X.690 §10) that `crypto.subtle`'s key formats are made of —
//! SubjectPublicKeyInfo (RFC 5280 §4.1.2.7), PrivateKeyInfo (RFC 5208 §5 / RFC 5958), RSAPublicKey and
//! RSAPrivateKey (RFC 8017 A.1), ECPrivateKey (RFC 5915 §3) and the CurvePrivateKey OCTET STRING (RFC 8410 §7).
//!
//! The reader is strict DER, because key material is attacker-supplied: definite lengths in their minimal form,
//! INTEGERs minimal and (where a key needs it) non-negative, nothing trailing. The writer emits only DER.

/// Universal tags used here.
pub const INTEGER: u8 = 0x02;
pub const BIT_STRING: u8 = 0x03;
pub const OCTET_STRING: u8 = 0x04;
pub const NULL: u8 = 0x05;
pub const OID: u8 = 0x06;
pub const SEQUENCE: u8 = 0x30;

/// A cursor over DER bytes.
pub struct Reader<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Reader { b, pos: 0 }
    }

    pub fn at_end(&self) -> bool {
        self.pos == self.b.len()
    }

    /// The next tag, if any, without consuming it.
    pub fn peek(&self) -> Option<u8> {
        self.b.get(self.pos).copied()
    }

    /// One TLV: (tag, contents). Low-tag-number form only; lengths definite and minimal (X.690 §10.1).
    pub fn tlv(&mut self) -> Option<(u8, &'a [u8])> {
        let tag = *self.b.get(self.pos)?;
        if tag & 0x1f == 0x1f {
            return None;
        }
        let l0 = *self.b.get(self.pos + 1)? as usize;
        let mut p = self.pos + 2;
        let len = if l0 < 0x80 {
            l0
        } else {
            let n = l0 & 0x7f;
            if n == 0 || n > 4 {
                return None;
            }
            let mut len = 0usize;
            for i in 0..n {
                len = (len << 8) | *self.b.get(p + i)? as usize;
            }
            // minimal: no leading zero octet, and the long form only for lengths >= 128
            if self.b[p] == 0 || len < 0x80 {
                return None;
            }
            p += n;
            len
        };
        let end = p.checked_add(len)?;
        let body = self.b.get(p..end)?;
        self.pos = end;
        Some((tag, body))
    }

    /// The next TLV, which must carry `tag`.
    pub fn expect(&mut self, tag: u8) -> Option<&'a [u8]> {
        match self.tlv()? {
            (t, body) if t == tag => Some(body),
            _ => None,
        }
    }

    /// A non-negative INTEGER's magnitude, big-endian, minimal (X.690 §8.3.2), at least one byte.
    pub fn uint(&mut self) -> Option<&'a [u8]> {
        let b = self.expect(INTEGER)?;
        if b.is_empty() || b[0] & 0x80 != 0 {
            return None;
        }
        if b.len() > 1 && b[0] == 0 && b[1] & 0x80 == 0 {
            return None;
        }
        if b.len() > 1 && b[0] == 0 { Some(&b[1..]) } else { Some(b) }
    }

    /// A small INTEGER (a version field).
    pub fn small_uint(&mut self) -> Option<u64> {
        let b = self.uint()?;
        if b.len() > 8 {
            return None;
        }
        Some(b.iter().fold(0u64, |a, &x| (a << 8) | x as u64))
    }

    /// An OBJECT IDENTIFIER in dotted form (X.690 §8.19).
    pub fn oid(&mut self) -> Option<String> {
        oid_to_string(self.expect(OID)?)
    }

    /// A BIT STRING with no unused bits (every key carried in one is whole octets).
    pub fn bits(&mut self) -> Option<&'a [u8]> {
        let b = self.expect(BIT_STRING)?;
        if b.first() != Some(&0) {
            return None;
        }
        Some(&b[1..])
    }
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

// ---- writer ------------------------------------------------------------------------------------------------------

pub fn tlv(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let n = body.len();
    if n < 0x80 {
        out.push(n as u8);
    } else {
        let bytes = n.to_be_bytes();
        let skip = bytes.iter().position(|&b| b != 0).unwrap_or(bytes.len() - 1);
        out.push(0x80 | (bytes.len() - skip) as u8);
        out.extend_from_slice(&bytes[skip..]);
    }
    out.extend_from_slice(body);
    out
}

pub fn seq(parts: &[Vec<u8>]) -> Vec<u8> {
    tlv(SEQUENCE, &parts.concat())
}

/// A non-negative INTEGER from big-endian magnitude bytes (leading zeros dropped, a 0x00 added before a set
/// high bit).
pub fn uint(be: &[u8]) -> Vec<u8> {
    let nz = be.iter().position(|&b| b != 0).unwrap_or(be.len());
    let mut body = Vec::new();
    let mag = &be[nz..];
    if mag.is_empty() {
        body.push(0);
    } else {
        if mag[0] & 0x80 != 0 {
            body.push(0);
        }
        body.extend_from_slice(mag);
    }
    tlv(INTEGER, &body)
}

pub fn oid(s: &str) -> Vec<u8> {
    tlv(OID, &oid_from_string(s))
}

pub fn bits(b: &[u8]) -> Vec<u8> {
    let mut body = vec![0u8];
    body.extend_from_slice(b);
    tlv(BIT_STRING, &body)
}

pub fn octets(b: &[u8]) -> Vec<u8> {
    tlv(OCTET_STRING, b)
}

/// AlgorithmIdentifier's parameters as `crypto.subtle` carries them: none, NULL, or a named-curve OID.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Params {
    Absent,
    Null,
    Oid(String),
    /// Anything else (explicit curve parameters, RSASSA-PSS-params, …): refused by every caller.
    Other,
}

fn alg_id(oid_s: &str, params: &Params) -> Vec<u8> {
    let mut parts = vec![oid(oid_s)];
    match params {
        Params::Null => parts.push(tlv(NULL, &[])),
        Params::Oid(o) => parts.push(oid(o)),
        _ => {}
    }
    seq(&parts)
}

fn read_alg_id(r: &mut Reader) -> Option<(String, Params)> {
    let body = r.expect(SEQUENCE)?;
    let mut a = Reader::new(body);
    let o = a.oid()?;
    let params = if a.at_end() {
        Params::Absent
    } else {
        match a.tlv()? {
            (NULL, b) if b.is_empty() => Params::Null,
            (OID, b) => Params::Oid(oid_to_string(b)?),
            _ => Params::Other,
        }
    };
    if !a.at_end() {
        return None;
    }
    Some((o, params))
}

/// SubjectPublicKeyInfo → (algorithm OID, parameters, subjectPublicKey octets).
pub fn spki_parse(b: &[u8]) -> Option<(String, Params, Vec<u8>)> {
    let mut r = Reader::new(b);
    let body = r.expect(SEQUENCE)?;
    if !r.at_end() {
        return None;
    }
    let mut s = Reader::new(body);
    let (o, p) = read_alg_id(&mut s)?;
    let key = s.bits()?;
    if !s.at_end() {
        return None;
    }
    Some((o, p, key.to_vec()))
}

pub fn spki_build(oid_s: &str, params: &Params, key: &[u8]) -> Vec<u8> {
    seq(&[alg_id(oid_s, params), bits(key)])
}

/// PrivateKeyInfo / OneAsymmetricKey (version 0 or 1; attributes and the v2 public key are skipped) →
/// (algorithm OID, parameters, privateKey octets).
pub fn pkcs8_parse(b: &[u8]) -> Option<(String, Params, Vec<u8>)> {
    let mut r = Reader::new(b);
    let body = r.expect(SEQUENCE)?;
    if !r.at_end() {
        return None;
    }
    let mut s = Reader::new(body);
    let version = s.small_uint()?;
    if version > 1 {
        return None;
    }
    let (o, p) = read_alg_id(&mut s)?;
    let key = s.expect(OCTET_STRING)?;
    while !s.at_end() {
        let (tag, _) = s.tlv()?;
        if tag != 0xa0 && tag != 0x81 && tag != 0xa1 {
            return None;
        }
    }
    Some((o, p, key.to_vec()))
}

pub fn pkcs8_build(oid_s: &str, params: &Params, key: &[u8]) -> Vec<u8> {
    seq(&[uint(&[0]), alg_id(oid_s, params), octets(key)])
}

/// RSAPublicKey → (n, e).
pub fn rsa_public_parse(b: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    let mut r = Reader::new(b);
    let body = r.expect(SEQUENCE)?;
    if !r.at_end() {
        return None;
    }
    let mut s = Reader::new(body);
    let n = s.uint()?.to_vec();
    let e = s.uint()?.to_vec();
    if !s.at_end() {
        return None;
    }
    Some((n, e))
}

pub fn rsa_public_build(n: &[u8], e: &[u8]) -> Vec<u8> {
    seq(&[uint(n), uint(e)])
}

/// RSAPrivateKey (two-prime, version 0) → [n, e, d, p, q, dP, dQ, qInv].
pub fn rsa_private_parse(b: &[u8]) -> Option<Vec<Vec<u8>>> {
    let mut r = Reader::new(b);
    let body = r.expect(SEQUENCE)?;
    if !r.at_end() {
        return None;
    }
    let mut s = Reader::new(body);
    if s.small_uint()? != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(8);
    for _ in 0..8 {
        out.push(s.uint()?.to_vec());
    }
    if !s.at_end() {
        return None;
    }
    Some(out)
}

pub fn rsa_private_build(parts: &[Vec<u8>]) -> Vec<u8> {
    let mut items = vec![uint(&[0])];
    items.extend(parts.iter().map(|p| uint(p)));
    seq(&items)
}

/// ECPrivateKey (version 1) → (the private key octets, the named curve if `[0]` carries one, the public key
/// if `[1]` carries one).
pub fn ec_private_parse(b: &[u8]) -> Option<(Vec<u8>, Option<String>, Option<Vec<u8>>)> {
    let mut r = Reader::new(b);
    let body = r.expect(SEQUENCE)?;
    if !r.at_end() {
        return None;
    }
    let mut s = Reader::new(body);
    if s.small_uint()? != 1 {
        return None;
    }
    let d = s.expect(OCTET_STRING)?.to_vec();
    let mut curve = None;
    let mut public = None;
    if s.peek() == Some(0xa0) {
        let inner = s.expect(0xa0)?;
        let mut i = Reader::new(inner);
        curve = Some(i.oid()?);
        if !i.at_end() {
            return None;
        }
    }
    if s.peek() == Some(0xa1) {
        let inner = s.expect(0xa1)?;
        let mut i = Reader::new(inner);
        public = Some(i.bits()?.to_vec());
        if !i.at_end() {
            return None;
        }
    }
    if !s.at_end() {
        return None;
    }
    Some((d, curve, public))
}

/// ECPrivateKey with the public key in `[1]` and no `[0]` (the curve is in the PrivateKeyInfo).
pub fn ec_private_build(d: &[u8], public: &[u8]) -> Vec<u8> {
    seq(&[uint(&[1]), octets(d), tlv(0xa1, &bits(public))])
}

/// CurvePrivateKey (RFC 8410 §7): an OCTET STRING.
pub fn octets_parse(b: &[u8]) -> Option<Vec<u8>> {
    let mut r = Reader::new(b);
    let v = r.expect(OCTET_STRING)?.to_vec();
    if !r.at_end() {
        return None;
    }
    Some(v)
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
        assert!(Reader::new(&[0x02, 0x81, 0x01, 0x00]).uint().is_none(), "non-minimal length");
        assert!(Reader::new(&[0x02, 0x02, 0x00, 0x01]).uint().is_none(), "non-minimal integer");
    }
}
