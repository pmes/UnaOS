//! RFC 5280 §4.1 certificate parsing (the profile a TLS server chain uses).

use alloc::string::String;
use alloc::vec::Vec;

use super::der::{self, tag, Der};
use super::name::{NameConstraints, SubjectAltNames};
use super::oid;
use crate::crypto::{EcCurve, HashAlg};
use crate::error::CertError;

const fn bad(m: &'static str) -> CertError {
    CertError::BadDer(m)
}

/// A subject public key tls_core can hand to the provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicKey {
    /// SEC1 point (uncompressed or compressed as encoded).
    Ec { curve: EcCurve, point: Vec<u8> },
    Ed25519([u8; 32]),
    /// RSAPublicKey (RFC 8017 A.1.1) magnitudes.
    Rsa { n: Vec<u8>, e: Vec<u8> },
    /// A key type tls_core does not verify with (the reason names it).
    Unsupported(&'static str),
}

/// The certificate signature algorithms tls_core recognises (RFC 5758, RFC 8410, RFC 4055).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureAlgorithm {
    Ecdsa(HashAlg),
    Ed25519,
    RsaPkcs1(HashAlg),
    /// RSASSA-PSS with MGF1(same hash) and the given salt length.
    RsaPss { hash: HashAlg, salt_len: u32 },
    Unsupported,
}

/// KeyUsage bits (RFC 5280 §4.2.1.3), bit 0 = digitalSignature.
pub mod key_usage {
    pub const DIGITAL_SIGNATURE: u16 = 1 << 0;
    pub const KEY_CERT_SIGN: u16 = 1 << 5;
}

/// A parsed certificate. Owns its DER.
#[derive(Debug, Clone)]
pub struct Certificate {
    pub der: Vec<u8>,
    tbs: (usize, usize),
    pub signature_algorithm: SignatureAlgorithm,
    pub signature: Vec<u8>,
    pub version: u8,
    pub serial: Vec<u8>,
    /// The DER encoding of the issuer / subject Name (compared byte-for-byte when chaining).
    pub issuer: Vec<u8>,
    pub subject: Vec<u8>,
    pub not_before: i64,
    pub not_after: i64,
    pub public_key: PublicKey,
    /// BasicConstraints: (cA, pathLenConstraint).
    pub basic_constraints: Option<(bool, Option<u32>)>,
    pub key_usage: Option<u16>,
    /// ExtendedKeyUsage OIDs (contents octets).
    pub ext_key_usage: Option<Vec<Vec<u8>>>,
    pub san: SubjectAltNames,
    pub name_constraints: Option<NameConstraints>,
    pub authority_key_id: Option<Vec<u8>>,
    pub subject_key_id: Option<Vec<u8>>,
    /// A critical extension tls_core does not process (the certificate must then be rejected).
    pub unknown_critical: bool,
    /// Subject commonName (diagnostics only).
    pub subject_cn: Option<String>,
}

impl Certificate {
    pub fn tbs(&self) -> &[u8] {
        &self.der[self.tbs.0..self.tbs.1]
    }
    pub fn is_ca(&self) -> bool {
        matches!(self.basic_constraints, Some((true, _)))
    }
    pub fn self_issued(&self) -> bool {
        self.issuer == self.subject
    }

    /// Parses `Certificate ::= SEQUENCE { tbsCertificate, signatureAlgorithm, signatureValue }`.
    pub fn parse(input: &[u8]) -> Result<Certificate, CertError> {
        let der_bytes = input.to_vec();
        let mut outer = Der::new(input);
        let cert_tlv = outer.expect(tag::SEQUENCE)?;
        outer.expect_end()?;
        let mut cert = Der::new(cert_tlv.value);
        let tbs_tlv = cert.expect(tag::SEQUENCE)?;
        let tbs_off = tbs_tlv.raw.as_ptr() as usize - input.as_ptr() as usize;
        let tbs_range = (tbs_off, tbs_off + tbs_tlv.raw.len());
        let outer_alg = cert.expect(tag::SEQUENCE)?;
        let signature = cert.bit_string_bytes()?.to_vec();
        cert.expect_end()?;

        let mut tbs = Der::new(tbs_tlv.value);
        let version = match tbs.optional(tag::context_constructed(0))? {
            Some(v) => {
                let mut d = Der::new(v.value);
                let n = d.small_uint()?;
                d.expect_end()?;
                if n > 2 {
                    return Err(bad("unknown version"));
                }
                (n + 1) as u8
            }
            None => 1,
        };
        let serial_tlv = tbs.expect(tag::INTEGER)?;
        let serial = serial_tlv.value.to_vec();
        let inner_alg = tbs.expect(tag::SEQUENCE)?;
        // RFC 5280 §4.1.1.2: signatureAlgorithm MUST equal tbsCertificate.signature.
        if inner_alg.raw != outer_alg.raw {
            return Err(bad("signature algorithm mismatch"));
        }
        let signature_algorithm = parse_signature_algorithm(outer_alg.value)?;
        let issuer = tbs.expect(tag::SEQUENCE)?.raw.to_vec();
        let mut validity = tbs.sequence()?;
        let nb = validity.tlv()?;
        let na = validity.tlv()?;
        validity.expect_end()?;
        let not_before = der::parse_time(&nb)?;
        let not_after = der::parse_time(&na)?;
        let subject_tlv = tbs.expect(tag::SEQUENCE)?;
        let subject = subject_tlv.raw.to_vec();
        let subject_cn = common_name(subject_tlv.value);
        let spki = tbs.expect(tag::SEQUENCE)?;
        let public_key = parse_spki(spki.value)?;
        // issuerUniqueID [1], subjectUniqueID [2]: skipped.
        tbs.optional(tag::context_primitive(1))?;
        tbs.optional(tag::context_primitive(2))?;

        let mut c = Certificate {
            der: der_bytes,
            tbs: tbs_range,
            signature_algorithm,
            signature,
            version,
            serial,
            issuer,
            subject,
            not_before,
            not_after,
            public_key,
            basic_constraints: None,
            key_usage: None,
            ext_key_usage: None,
            san: SubjectAltNames::default(),
            name_constraints: None,
            authority_key_id: None,
            subject_key_id: None,
            unknown_critical: false,
            subject_cn,
        };
        if let Some(exts) = tbs.optional(tag::context_constructed(3))? {
            if version != 3 {
                return Err(bad("extensions in a pre-v3 certificate"));
            }
            let mut w = Der::new(exts.value);
            let mut list = w.sequence()?;
            w.expect_end()?;
            let mut seen: Vec<&[u8]> = Vec::new();
            while !list.is_empty() {
                let mut e = list.sequence()?;
                let id = e.oid()?;
                let critical = if e.peek_tag() == Some(tag::BOOLEAN) { e.boolean()? } else { false };
                let value = e.expect(tag::OCTET_STRING)?.value;
                e.expect_end()?;
                if seen.contains(&id) {
                    return Err(bad("duplicate extension"));
                }
                seen.push(id);
                c.parse_extension(id, critical, value)?;
            }
        }
        tbs.expect_end()?;
        Ok(c)
    }

    fn parse_extension(&mut self, id: &[u8], critical: bool, value: &[u8]) -> Result<(), CertError> {
        if id.len() != 3 || &id[..2] != oid::ID_CE {
            if critical {
                self.unknown_critical = true;
            }
            return Ok(());
        }
        let mut d = Der::new(value);
        match id[2] {
            oid::CE_BASIC_CONSTRAINTS => {
                let mut s = d.sequence()?;
                let ca = if s.peek_tag() == Some(tag::BOOLEAN) { s.boolean()? } else { false };
                let pl = if s.peek_tag() == Some(tag::INTEGER) { Some(s.small_uint()? as u32) } else { None };
                s.expect_end()?;
                self.basic_constraints = Some((ca, pl));
            }
            oid::CE_KEY_USAGE => {
                let v = d.expect(tag::BIT_STRING)?.value;
                let (unused, bits) = v.split_first().ok_or(bad("KeyUsage"))?;
                if *unused > 7 || bits.is_empty() || bits.len() > 2 {
                    return Err(bad("KeyUsage"));
                }
                // Bit 0 is the MSB of the first octet.
                let mut ku = 0u16;
                for (i, byte) in bits.iter().enumerate() {
                    for b in 0..8 {
                        if byte & (0x80 >> b) != 0 {
                            ku |= 1 << (i * 8 + b);
                        }
                    }
                }
                self.key_usage = Some(ku);
            }
            oid::CE_EXT_KEY_USAGE => {
                let mut s = d.sequence()?;
                let mut v = Vec::new();
                while !s.is_empty() {
                    v.push(s.oid()?.to_vec());
                }
                if v.is_empty() {
                    return Err(bad("empty ExtendedKeyUsage"));
                }
                self.ext_key_usage = Some(v);
            }
            oid::CE_SUBJECT_ALT_NAME => {
                let mut s = d.sequence()?;
                self.san.present = true;
                while !s.is_empty() {
                    let gn = s.tlv()?;
                    match gn.tag {
                        0x82 => self.san.dns.push(ascii(gn.value)?),
                        0x87 => {
                            if gn.value.len() != 4 && gn.value.len() != 16 {
                                return Err(bad("SAN iPAddress length"));
                            }
                            self.san.ip.push(gn.value.to_vec())
                        }
                        _ => {}
                    }
                }
            }
            oid::CE_NAME_CONSTRAINTS => {
                let mut s = d.sequence()?;
                let mut nc = NameConstraints::default();
                if let Some(p) = s.optional(tag::context_constructed(0))? {
                    parse_subtrees(p.value, &mut nc, true)?;
                }
                if let Some(x) = s.optional(tag::context_constructed(1))? {
                    parse_subtrees(x.value, &mut nc, false)?;
                }
                s.expect_end()?;
                self.name_constraints = Some(nc);
            }
            oid::CE_AUTHORITY_KEY_ID => {
                let mut s = d.sequence()?;
                if let Some(k) = s.optional(tag::context_primitive(0))? {
                    self.authority_key_id = Some(k.value.to_vec());
                }
            }
            oid::CE_SUBJECT_KEY_ID => {
                self.subject_key_id = Some(d.expect(tag::OCTET_STRING)?.value.to_vec());
            }
            // Recognised, not enforced: policy processing and CRL distribution are outside this core's ceiling,
            // and both are non-critical in the Web PKI.
            oid::CE_CERTIFICATE_POLICIES | oid::CE_CRL_DISTRIBUTION_POINTS | oid::CE_ISSUER_ALT_NAME => {
                return Ok(());
            }
            _ => {
                if critical {
                    self.unknown_critical = true;
                }
                return Ok(());
            }
        }
        d.expect_end()?;
        Ok(())
    }
}

fn ascii(v: &[u8]) -> Result<String, CertError> {
    if !v.iter().all(|c| c.is_ascii() && *c >= 0x20 && *c < 0x7f) {
        return Err(bad("IA5String"));
    }
    Ok(String::from(core::str::from_utf8(v).map_err(|_| bad("IA5String"))?))
}

fn parse_subtrees(v: &[u8], nc: &mut NameConstraints, permitted: bool) -> Result<(), CertError> {
    let mut d = Der::new(v);
    while !d.is_empty() {
        let mut st = d.sequence()?;
        let base = st.tlv()?;
        // minimum [0] / maximum [1] must be absent/0 in the Web PKI profile (RFC 5280: MUST be zero / absent).
        if let Some(m) = st.optional(tag::context_primitive(0))? {
            if der::integer_magnitude(m.value)?.iter().any(|&b| b != 0) {
                return Err(bad("name constraint minimum"));
            }
        }
        if st.optional(tag::context_primitive(1))?.is_some() {
            return Err(bad("name constraint maximum"));
        }
        st.expect_end()?;
        match base.tag {
            0x82 => {
                let s = ascii(base.value)?;
                if permitted {
                    nc.has_permitted_dns = true;
                    nc.permitted_dns.push(s);
                } else {
                    nc.excluded_dns.push(s);
                }
            }
            0x87 => {
                if base.value.len() != 8 && base.value.len() != 32 {
                    return Err(bad("iPAddress constraint length"));
                }
                if permitted {
                    nc.has_permitted_ip = true;
                    nc.permitted_ip.push(base.value.to_vec());
                } else {
                    nc.excluded_ip.push(base.value.to_vec());
                }
            }
            _ => nc.unenforced += 1,
        }
    }
    Ok(())
}

fn common_name(name: &[u8]) -> Option<String> {
    let mut rdns = Der::new(name);
    let mut cn = None;
    while !rdns.is_empty() {
        let set = rdns.expect(tag::SET).ok()?;
        let mut s = Der::new(set.value);
        while !s.is_empty() {
            let mut atv = s.sequence().ok()?;
            let ty = atv.oid().ok()?;
            let val = atv.tlv().ok()?;
            if ty == oid::AT_COMMON_NAME {
                cn = core::str::from_utf8(val.value).ok().map(String::from);
            }
        }
    }
    cn
}

fn hash_from_oid(o: &[u8]) -> Option<HashAlg> {
    match o {
        x if x == oid::SHA256 => Some(HashAlg::Sha256),
        x if x == oid::SHA384 => Some(HashAlg::Sha384),
        x if x == oid::SHA512 => Some(HashAlg::Sha512),
        _ => None,
    }
}

/// AlgorithmIdentifier contents → SignatureAlgorithm.
pub fn parse_signature_algorithm(v: &[u8]) -> Result<SignatureAlgorithm, CertError> {
    let mut d = Der::new(v);
    let id = d.oid()?;
    let params = if d.is_empty() { None } else { Some(d.tlv()?) };
    d.expect_end()?;
    let null_or_absent = |p: &Option<der::Tlv<'_>>| p.map_or(true, |t| t.tag == tag::NULL && t.value.is_empty());
    Ok(match id {
        x if x == oid::ECDSA_WITH_SHA256 && params.is_none() => SignatureAlgorithm::Ecdsa(HashAlg::Sha256),
        x if x == oid::ECDSA_WITH_SHA384 && params.is_none() => SignatureAlgorithm::Ecdsa(HashAlg::Sha384),
        x if x == oid::ECDSA_WITH_SHA512 && params.is_none() => SignatureAlgorithm::Ecdsa(HashAlg::Sha512),
        x if x == oid::ED25519 && params.is_none() => SignatureAlgorithm::Ed25519,
        x if x == oid::SHA256_WITH_RSA && null_or_absent(&params) => SignatureAlgorithm::RsaPkcs1(HashAlg::Sha256),
        x if x == oid::SHA384_WITH_RSA && null_or_absent(&params) => SignatureAlgorithm::RsaPkcs1(HashAlg::Sha384),
        x if x == oid::SHA512_WITH_RSA && null_or_absent(&params) => SignatureAlgorithm::RsaPkcs1(HashAlg::Sha512),
        x if x == oid::RSASSA_PSS => match params {
            Some(p) if p.tag == tag::SEQUENCE => parse_pss_params(p.value)?,
            _ => SignatureAlgorithm::Unsupported,
        },
        _ => SignatureAlgorithm::Unsupported,
    })
}

/// RSASSA-PSS-params (RFC 4055 §3.1). Only MGF1 with the same hash, trailerField 1.
fn parse_pss_params(v: &[u8]) -> Result<SignatureAlgorithm, CertError> {
    let mut d = Der::new(v);
    let mut hash = None;
    if let Some(h) = d.optional(tag::context_constructed(0))? {
        let mut a = Der::new(h.value).sequence()?;
        hash = hash_from_oid(a.oid()?);
    }
    let mut mgf_hash = None;
    if let Some(m) = d.optional(tag::context_constructed(1))? {
        let mut a = Der::new(m.value).sequence()?;
        if a.oid()? != oid::MGF1 {
            return Ok(SignatureAlgorithm::Unsupported);
        }
        let mut h = a.sequence()?;
        mgf_hash = hash_from_oid(h.oid()?);
    }
    let mut salt_len = 20u32;
    if let Some(s) = d.optional(tag::context_constructed(2))? {
        salt_len = Der::new(s.value).small_uint()? as u32;
    }
    if let Some(t) = d.optional(tag::context_constructed(3))? {
        if Der::new(t.value).small_uint()? != 1 {
            return Ok(SignatureAlgorithm::Unsupported);
        }
    }
    match (hash, mgf_hash) {
        (Some(h), Some(m)) if h == m => Ok(SignatureAlgorithm::RsaPss { hash: h, salt_len }),
        _ => Ok(SignatureAlgorithm::Unsupported),
    }
}

/// SubjectPublicKeyInfo contents → PublicKey.
pub fn parse_spki(v: &[u8]) -> Result<PublicKey, CertError> {
    let mut d = Der::new(v);
    let mut alg = d.sequence()?;
    let key = d.bit_string_bytes()?;
    d.expect_end()?;
    let id = alg.oid()?;
    Ok(match id {
        x if x == oid::EC_PUBLIC_KEY => {
            let curve = alg.oid()?;
            let curve = if curve == oid::PRIME256V1 {
                EcCurve::P256
            } else if curve == oid::SECP384R1 {
                EcCurve::P384
            } else {
                return Ok(PublicKey::Unsupported("EC curve other than P-256/P-384"));
            };
            PublicKey::Ec { curve, point: key.to_vec() }
        }
        x if x == oid::ED25519 => {
            if key.len() != 32 {
                return Err(bad("Ed25519 key length"));
            }
            let mut k = [0u8; 32];
            k.copy_from_slice(key);
            PublicKey::Ed25519(k)
        }
        x if x == oid::RSA_ENCRYPTION => {
            let mut k = Der::new(key);
            let mut s = k.sequence()?;
            let n = s.uint()?.to_vec();
            let e = s.uint()?.to_vec();
            s.expect_end()?;
            k.expect_end()?;
            PublicKey::Rsa { n, e }
        }
        _ => PublicKey::Unsupported("public key algorithm"),
    })
}
