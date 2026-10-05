//! Certificate Revocation Lists (RFC 5280 §5, checked per §6.3) — CTCORE (SR60). The CALLER fetches CRLs (from a
//! certificate's cRLDistributionPoints, [`super::Certificate::crl_dp`]) and puts them in
//! [`super::TrustStore::crls`]; this module never touches the network.
//!
//! For each certificate on a validated path (leaf and intermediates), with its issuer from the same path:
//! * a COMPLETE CRL is usable when its issuer Name equals the certificate's issuer, its authorityKeyIdentifier (if
//!   any) equals the issuer's subjectKeyIdentifier, the issuer's KeyUsage (if any) has cRLSign, its signature
//!   verifies under the issuer's key, thisUpdate ≤ now + 5 min ≤ nextUpdate + 10 min (nextUpdate is required), it
//!   carries no critical extension or entry extension we do not process (an indirect CRL — certificateIssuer —
//!   is refused), and its issuingDistributionPoint scope covers the certificate (onlyContainsUserCerts /
//!   onlyContainsCACerts / onlyContainsAttributeCerts; onlySomeReasons and indirectCRL are not supported, so such
//!   a CRL is not used; a distributionPoint fullName must share a URI with the certificate's cRLDistributionPoints
//!   when the certificate has any). The usable complete CRL with the highest cRLNumber wins.
//! * DELTA CRLs (§5.2.4, deltaCRLIndicator) apply only when the complete CRL or the certificate announces
//!   freshestCRL (§5.2.6 — OpenSSL's rule too), the delta is usable as above, has the same issuingDistributionPoint,
//!   deltaCRLIndicator ≤ the complete CRL's number < the delta's own number; the newest such delta is applied.
//! * Status: listed in the delta → revoked unless its reason is removeFromCRL (8), which UN-revokes an entry the
//!   complete CRL carried (certificateHold released); listed in the complete CRL → revoked (certificateHold
//!   included: on hold is not valid). Revoked → [`CertError::Revoked`].
//!
//! A CRL that names the issuer but is not usable is counted in [`CrlReport::unusable`] with its first reason and
//! otherwise ignored (a stale or damaged download is a soft failure); [`super::TrustStore::require_revocation`]
//! turns "no usable revocation information" into a hard [`CertError::BadCrl`].

use alloc::string::String;
use alloc::vec::Vec;

use super::cert::{key_usage, parse_signature_algorithm, Certificate, PublicKey, SignatureAlgorithm};
use super::der::{self, tag, Der};
use super::{oid, verify_signed};
use crate::crypto::CryptoProvider;
use crate::error::CertError;

const SKEW: i64 = 300;
const NEXT_UPDATE_GRACE: i64 = 600;
/// removeFromCRL (RFC 5280 §5.3.1).
pub const REASON_REMOVE_FROM_CRL: u8 = 8;
pub const REASON_CERTIFICATE_HOLD: u8 = 6;

fn bad(m: &'static str) -> CertError {
    CertError::BadCrl(m)
}

/// One revokedCertificates entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revoked {
    pub serial: Vec<u8>,
    pub date: i64,
    pub reason: Option<u8>,
}

/// issuingDistributionPoint (RFC 5280 §5.2.5).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Idp {
    pub uris: Vec<String>,
    pub only_user: bool,
    pub only_ca: bool,
    pub only_some_reasons: bool,
    pub indirect: bool,
    pub only_attribute: bool,
    pub raw: Vec<u8>,
}

/// A parsed CRL. Owns its DER.
#[derive(Debug, Clone)]
pub struct Crl {
    pub der: Vec<u8>,
    tbs: (usize, usize),
    pub signature_algorithm: SignatureAlgorithm,
    pub signature: Vec<u8>,
    pub issuer: Vec<u8>,
    pub this_update: i64,
    pub next_update: Option<i64>,
    pub entries: Vec<Revoked>,
    /// cRLNumber magnitude (big-endian, no leading zeros).
    pub number: Option<Vec<u8>>,
    /// deltaCRLIndicator's BaseCRLNumber: Some → this is a delta CRL.
    pub delta_base: Option<Vec<u8>>,
    pub authority_key_id: Option<Vec<u8>>,
    pub idp: Option<Idp>,
    /// freshestCRL present (deltas exist for this scope).
    pub freshest: bool,
    /// Why this CRL can never be used (an unprocessed critical extension, an indirect entry), if so.
    pub unsupported: Option<&'static str>,
}

fn magnitude(v: &[u8]) -> Vec<u8> {
    let i = v.iter().position(|&b| b != 0).unwrap_or(v.len());
    v[i..].to_vec()
}

/// Big-endian magnitude comparison.
fn cmp_num(a: &[u8], b: &[u8]) -> core::cmp::Ordering {
    a.len().cmp(&b.len()).then_with(|| a.cmp(b))
}

impl Crl {
    pub fn tbs(&self) -> &[u8] {
        &self.der[self.tbs.0..self.tbs.1]
    }

    /// Parses `CertificateList ::= SEQUENCE { tbsCertList, signatureAlgorithm, signatureValue }`.
    pub fn parse(input: &[u8]) -> Result<Crl, CertError> {
        let d0 = |e: CertError| match e {
            CertError::BadDer(m) => bad(m),
            o => o,
        };
        let mut outer = Der::new(input);
        let top = outer.expect(tag::SEQUENCE).map_err(d0)?;
        outer.expect_end().map_err(d0)?;
        let mut cl = Der::new(top.value);
        let tbs_tlv = cl.expect(tag::SEQUENCE).map_err(d0)?;
        let off = tbs_tlv.raw.as_ptr() as usize - input.as_ptr() as usize;
        let outer_alg = cl.expect(tag::SEQUENCE).map_err(d0)?;
        let signature = cl.bit_string_bytes().map_err(d0)?.to_vec();
        cl.expect_end().map_err(d0)?;

        let mut t = Der::new(tbs_tlv.value);
        if t.peek_tag() == Some(tag::INTEGER) {
            if t.small_uint().map_err(d0)? != 1 {
                return Err(bad("CRL version"));
            }
        }
        let inner_alg = t.expect(tag::SEQUENCE).map_err(d0)?;
        if inner_alg.raw != outer_alg.raw {
            return Err(bad("signature algorithm mismatch"));
        }
        let signature_algorithm = parse_signature_algorithm(outer_alg.value).map_err(d0)?;
        let issuer = t.expect(tag::SEQUENCE).map_err(d0)?.raw.to_vec();
        let this_update = der::parse_time(&t.tlv().map_err(d0)?).map_err(d0)?;
        let next_update = match t.peek_tag() {
            Some(tag::UTC_TIME) | Some(tag::GENERALIZED_TIME) => Some(der::parse_time(&t.tlv().map_err(d0)?).map_err(d0)?),
            _ => None,
        };
        let mut c = Crl {
            der: input.to_vec(),
            tbs: (off, off + tbs_tlv.raw.len()),
            signature_algorithm,
            signature,
            issuer,
            this_update,
            next_update,
            entries: Vec::new(),
            number: None,
            delta_base: None,
            authority_key_id: None,
            idp: None,
            freshest: false,
            unsupported: None,
        };
        if t.peek_tag() == Some(tag::SEQUENCE) {
            let mut list = t.sequence().map_err(d0)?;
            while !list.is_empty() {
                let mut e = list.sequence().map_err(d0)?;
                let serial = e.expect(tag::INTEGER).map_err(d0)?.value.to_vec();
                let date = der::parse_time(&e.tlv().map_err(d0)?).map_err(d0)?;
                let mut reason = None;
                if !e.is_empty() {
                    let mut exts = e.sequence().map_err(d0)?;
                    while !exts.is_empty() {
                        let mut x = exts.sequence().map_err(d0)?;
                        let id = x.oid().map_err(d0)?;
                        let critical = if x.peek_tag() == Some(tag::BOOLEAN) { x.boolean().map_err(d0)? } else { false };
                        let v = x.expect(tag::OCTET_STRING).map_err(d0)?.value;
                        if id.len() == 3 && &id[..2] == oid::ID_CE && id[2] == oid::CE_REASON_CODE {
                            let r = Der::new(v).expect(0x0a).map_err(d0)?;
                            reason = r.value.last().copied();
                        } else if id.len() == 3 && &id[..2] == oid::ID_CE && id[2] == oid::CE_CERTIFICATE_ISSUER {
                            c.unsupported = Some("indirect CRL (certificateIssuer entry)");
                        } else if critical && !(id.len() == 3 && &id[..2] == oid::ID_CE && id[2] == oid::CE_INVALIDITY_DATE) {
                            c.unsupported = Some("unknown critical CRL entry extension");
                        }
                    }
                }
                c.entries.push(Revoked { serial, date, reason });
            }
        }
        if let Some(x) = t.optional(tag::context_constructed(0)).map_err(d0)? {
            let mut w = Der::new(x.value);
            let mut exts = w.sequence().map_err(d0)?;
            while !exts.is_empty() {
                let mut e = exts.sequence().map_err(d0)?;
                let id = e.oid().map_err(d0)?;
                let critical = if e.peek_tag() == Some(tag::BOOLEAN) { e.boolean().map_err(d0)? } else { false };
                let v = e.expect(tag::OCTET_STRING).map_err(d0)?.value;
                c.extension(id, critical, v).map_err(d0)?;
            }
        }
        t.expect_end().map_err(d0)?;
        Ok(c)
    }

    fn extension(&mut self, id: &[u8], critical: bool, v: &[u8]) -> Result<(), CertError> {
        if id.len() != 3 || &id[..2] != oid::ID_CE {
            if critical {
                self.unsupported = Some("unknown critical CRL extension");
            }
            return Ok(());
        }
        let mut d = Der::new(v);
        match id[2] {
            oid::CE_CRL_NUMBER => self.number = Some(magnitude(d.expect(tag::INTEGER)?.value)),
            oid::CE_DELTA_CRL_INDICATOR => self.delta_base = Some(magnitude(d.expect(tag::INTEGER)?.value)),
            oid::CE_AUTHORITY_KEY_ID => {
                let mut s = d.sequence()?;
                if let Some(k) = s.optional(tag::context_primitive(0))? {
                    self.authority_key_id = Some(k.value.to_vec());
                }
            }
            oid::CE_FRESHEST_CRL => self.freshest = true,
            oid::CE_ISSUING_DISTRIBUTION_POINT => {
                let mut idp = Idp { raw: v.to_vec(), ..Idp::default() };
                let mut s = d.sequence()?;
                while !s.is_empty() {
                    let el = s.tlv()?;
                    match el.tag {
                        0xa0 => {
                            // DistributionPointName: fullName [0] GeneralNames
                            let mut n = Der::new(el.value);
                            if let Some(full) = n.optional(tag::context_constructed(0))? {
                                let mut g = Der::new(full.value);
                                while !g.is_empty() {
                                    let gn = g.tlv()?;
                                    if gn.tag == 0x86 {
                                        idp.uris.push(String::from(core::str::from_utf8(gn.value).map_err(|_| bad("IDP URI"))?));
                                    }
                                }
                            }
                        }
                        0x81 => idp.only_user = el.value == [0xff],
                        0x82 => idp.only_ca = el.value == [0xff],
                        0x83 => idp.only_some_reasons = true,
                        0x84 => idp.indirect = el.value == [0xff],
                        0x85 => idp.only_attribute = el.value == [0xff],
                        _ => return Err(bad("issuingDistributionPoint")),
                    }
                }
                self.idp = Some(idp);
            }
            oid::CE_ISSUER_ALT_NAME => {}
            _ => {
                if critical {
                    self.unsupported = Some("unknown critical CRL extension");
                }
            }
        }
        Ok(())
    }

    fn lists(&self, serial: &[u8]) -> Option<&Revoked> {
        let s = magnitude(serial);
        self.entries.iter().find(|e| magnitude(&e.serial) == s)
    }
}

/// The issuer of a certificate on the path, as CRL checking needs it.
pub struct CrlIssuer<'a> {
    pub key: &'a PublicKey,
    pub subject_key_id: Option<&'a [u8]>,
    pub key_usage: Option<u16>,
}

/// What CRL checking found over a path.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CrlReport {
    /// Certificates on the path a usable complete CRL covered (0 = CRLs not consulted / none applied).
    pub checked: usize,
    pub leaf_checked: bool,
    /// Delta CRLs applied.
    pub deltas: usize,
    /// CRLs naming an issuer on the path that could not be used, and the first reason.
    pub unusable: usize,
    pub unusable_why: Option<&'static str>,
}

fn usable(p: &dyn CryptoProvider, crl: &Crl, cert: &Certificate, iss: &CrlIssuer<'_>, now: i64) -> Result<(), &'static str> {
    if let Some(u) = crl.unsupported {
        return Err(u);
    }
    if let (Some(a), Some(s)) = (&crl.authority_key_id, iss.subject_key_id) {
        if a.as_slice() != s {
            return Err("authorityKeyIdentifier is not the issuer's key");
        }
    }
    if let Some(ku) = iss.key_usage {
        if ku & key_usage::CRL_SIGN == 0 {
            return Err("issuer KeyUsage lacks cRLSign");
        }
    }
    if verify_signed(p, iss.key, crl.signature_algorithm, crl.tbs(), &crl.signature).is_err() {
        return Err("signature");
    }
    if crl.this_update > now + SKEW {
        return Err("thisUpdate in the future");
    }
    match crl.next_update {
        None => return Err("no nextUpdate"),
        Some(n) if now > n + NEXT_UPDATE_GRACE => return Err("stale: past nextUpdate"),
        _ => {}
    }
    if let Some(idp) = &crl.idp {
        if idp.indirect {
            return Err("indirect CRL");
        }
        if idp.only_some_reasons {
            return Err("onlySomeReasons partition");
        }
        if idp.only_attribute || (idp.only_user && cert.is_ca()) || (idp.only_ca && !cert.is_ca()) {
            return Err("issuingDistributionPoint scope excludes the certificate");
        }
        if !idp.uris.is_empty() && !cert.crl_dp.is_empty() && !idp.uris.iter().any(|u| cert.crl_dp.contains(u)) {
            return Err("issuingDistributionPoint names another distribution point");
        }
    }
    Ok(())
}

/// Checks `cert` (issued by `iss`) against `crls` at `now`. `Ok(true)` = a usable complete CRL covered it and it
/// is not revoked; `Ok(false)` = no usable CRL; `Err(Revoked)` = revoked.
pub fn check_cert(p: &dyn CryptoProvider, crls: &[Crl], cert: &Certificate, iss: &CrlIssuer<'_>, now: i64, report: &mut CrlReport) -> Result<bool, CertError> {
    let mut complete: Option<&Crl> = None;
    let mut deltas: Vec<&Crl> = Vec::new();
    for c in crls.iter().filter(|c| c.issuer == cert.issuer) {
        match usable(p, c, cert, iss, now) {
            Err(why) => {
                report.unusable += 1;
                report.unusable_why.get_or_insert(why);
            }
            Ok(()) if c.delta_base.is_some() => deltas.push(c),
            Ok(()) => {
                let newer = match complete {
                    None => true,
                    Some(b) => match (&c.number, &b.number) {
                        (Some(x), Some(y)) => cmp_num(x, y).is_gt(),
                        _ => c.this_update > b.this_update,
                    },
                };
                if newer {
                    complete = Some(c);
                }
            }
        }
    }
    let Some(base) = complete else { return Ok(false) };
    let delta = match (&base.number, base.freshest || cert.freshest_crl) {
        (Some(bn), true) => deltas
            .into_iter()
            .filter(|d| {
                let (Some(db), Some(dn)) = (&d.delta_base, &d.number) else { return false };
                cmp_num(db, bn).is_le() && cmp_num(dn, bn).is_gt() && d.idp.as_ref().map(|i| &i.raw) == base.idp.as_ref().map(|i| &i.raw)
            })
            .max_by(|a, b| cmp_num(a.number.as_deref().unwrap_or(&[]), b.number.as_deref().unwrap_or(&[]))),
        _ => None,
    };
    report.checked += 1;
    if let Some(d) = delta {
        report.deltas += 1;
        if let Some(e) = d.lists(&cert.serial) {
            if e.reason == Some(REASON_REMOVE_FROM_CRL) {
                return Ok(true);
            }
            return Err(CertError::Revoked);
        }
    }
    match base.lists(&cert.serial) {
        Some(e) if e.reason != Some(REASON_REMOVE_FROM_CRL) => Err(CertError::Revoked),
        _ => Ok(true),
    }
}

/// Checks every certificate of a validated path (leaf, then intermediates) against the caller's CRLs.
pub fn check_path(p: &dyn CryptoProvider, crls: &[Crl], path: &super::VerifiedPath, now: i64) -> Result<(CrlReport, Vec<bool>), CertError> {
    let mut report = CrlReport::default();
    let mut covered = Vec::new();
    let certs: Vec<&Certificate> = core::iter::once(&path.leaf).chain(path.intermediates.iter()).collect();
    for (i, c) in certs.iter().enumerate() {
        let iss = match certs.get(i + 1) {
            Some(n) => CrlIssuer { key: &n.public_key, subject_key_id: n.subject_key_id.as_deref(), key_usage: n.key_usage },
            None => CrlIssuer { key: &path.anchor.public_key, subject_key_id: path.anchor.subject_key_id.as_deref(), key_usage: None },
        };
        let ok = check_cert(p, crls, c, &iss, now, &mut report)?;
        if i == 0 {
            report.leaf_checked = ok;
        }
        covered.push(ok);
    }
    Ok((report, covered))
}
