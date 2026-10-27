// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `sig` — PDF digital-signature validation (PDFSIG, LEDGER SR337; ISO 32000-1 §12.8).
//!
//! CHARTER: Aether — shared-core. Aether (CODEX §2 Web, "Chrome, Acrobat") owns reading a signed PDF; the X.509 and
//! signature arithmetic is NOT written here: the DER reader, certificate parser, signature dispatch and chain builder
//! are tls_core's (`tls_core::x509`: `Der`, `Certificate::parse`, `verify_signed`, `verify_server_chain_path`) and the
//! digests are crypto_core's. This module holds no key and no trust store: the caller passes anchors.
//!
//! [`Document::signatures`] reads every signed /FT /Sig field (its /V signature dictionary: /SubFilter, /ByteRange,
//! /Contents, /Name /Reason /Location /M, the /Reference DocMDP and FieldMDP transforms). [`Signature::verify`] climbs
//! the rungs in order and the first one that fails is the [`Verdict`]:
//!
//! 1. the /SubFilter: `adbe.pkcs7.detached` and `ETSI.CAdES.detached` are read; the SHA-1 and raw-RSA forms and
//!    document timestamps are refused by name;
//! 2. the /ByteRange: `[0 a b c]`, the gap `a..b` exactly the `<hex>` /Contents string, the signed revision ending at
//!    `%%EOF`;
//! 3. the CMS SignedData (RFC 5652): detached, one signer, a SHA-2 digest algorithm, strict DER (BER refused by name);
//! 4. the digest of the two byte ranges against the signed messageDigest attribute;
//! 5. the signer's signature over the signed attributes with its certificate's key;
//! 6. the certificate chain to the caller's anchors ([`Chain::Unchecked`] when none are passed — the signature is still
//!    checked). Revocation (CRL / OCSP) is NOT checked;
//! 7. an RFC 3161 timestamp token's genTime is read, NOT verified;
//! 8. incremental updates after the signed revision: the object numbers whose xref entry changed, judged under the
//!    certification signature's /DocMDP P (1: nothing, 2: form fill and signing, 3: also annotations) and this
//!    signature's /FieldMDP.

use alloc::format;
use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crypto_core::{Sha256, Sha384, Sha512};
use tls_core::CertError;
use tls_core::crypto::HashAlg;
use tls_core::cryptocore_provider::CryptoCoreProvider;
use tls_core::x509::cert::parse_signature_algorithm;
use tls_core::x509::der::{Der, parse_time, tag};
use tls_core::x509::verify::{TrustStore, verify_server_chain_path};
use tls_core::x509::{Certificate, SignatureAlgorithm, oid, verify_signed};

use crate::annots::FieldKind;
use crate::annots::form::full_name;
use crate::doc::Document;
use crate::lexer::is_white;
use crate::object::{Dict, Object, Ref, text_string};
use crate::xref::{self, XrefEntry};

const ID_DATA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x01];
const ID_SIGNED_DATA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02];
const ID_CONTENT_TYPE: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x03];
const ID_MESSAGE_DIGEST: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x04];
const ID_SIGNING_TIME: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x05];
const ID_TIMESTAMP_TOKEN: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x10, 0x02, 0x0e];
const ID_TST_INFO: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x10, 0x01, 0x04];

/// A /FieldMDP transform's /Action (§12.8.2.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldMdpAction {
    All,
    Include,
    Exclude,
}

/// A /FieldMDP transform: which fields the signature locks.
#[derive(Debug, Clone, PartialEq)]
pub struct FieldMdp {
    pub action: FieldMdpAction,
    /// /Fields: fully qualified field names.
    pub fields: Vec<String>,
}

/// One signed signature field, as read (nothing verified yet).
#[derive(Debug, Clone, PartialEq)]
pub struct Signature {
    /// The field's fully qualified name.
    pub field: String,
    /// The field dictionary's reference.
    pub obj: Option<Ref>,
    /// /ByteRange as two (offset, length) pairs.
    pub ranges: [(usize, usize); 2],
    /// /SubFilter.
    pub subfilter: Vec<u8>,
    pub name: Option<String>,
    pub reason: Option<String>,
    pub location: Option<String>,
    /// /M, the claimed signing time as written (unverified).
    pub m: Option<String>,
    /// The CMS DER from /Contents, its zero padding stripped.
    pub contents: Vec<u8>,
    /// Where the signed revision ends (the second range's end).
    pub signed_end: usize,
    /// This dictionary's /Reference DocMDP /P (a certification signature).
    pub doc_mdp: Option<u8>,
    pub field_mdp: Option<FieldMdp>,
    /// A structural refusal met while reading (/ByteRange, /Contents), reported by `verify`.
    pub refused: Option<&'static str>,
}

/// How far the certificate chain was checked.
#[derive(Debug, Clone, PartialEq)]
pub enum Chain {
    /// No trust anchors were passed: the signature is checked, the chain is not.
    Unchecked,
    /// A path to one of the caller's anchors validated (revocation NOT checked).
    Trusted,
    /// tls_core's path validation refused it.
    Refused(CertError),
}

/// What happened to the file after the signed revision.
#[derive(Debug, Clone, PartialEq)]
pub enum Changes {
    /// The signature covers the whole file.
    None,
    /// Later updates touched these objects, all permitted under /DocMDP P.
    Allowed { p: u8, touched: Vec<u32> },
    /// A later update touched `offender`, which /DocMDP P (or this signature's /FieldMDP) forbids.
    Disallowed { p: u8, offender: u32 },
    /// Later updates touched these objects and no certification signature restricts changes.
    Unrestricted { touched: Vec<u32> },
    /// The signed revision's or the newest xref did not load.
    Unknown,
}

/// The verdict: the first rung that failed, or every rung reached.
#[derive(Debug, Clone, PartialEq)]
pub enum Verdict {
    /// Refused before (or instead of) the cryptography, by name.
    Refused(&'static str),
    /// The byte ranges' digest is not the signed messageDigest.
    DigestMismatch,
    /// The signer's signature does not verify with its certificate's key.
    BadSignature,
    /// Digest and signature hold.
    Signed {
        chain: Chain,
        changes: Changes,
        /// The signed signingTime attribute (seconds since the epoch), when present.
        signing_time: Option<i64>,
        /// An RFC 3161 token's genTime, read but NOT verified.
        timestamp: Option<i64>,
    },
}

/// The caller's trust anchors and the time to validate the chain at (seconds since the epoch).
pub struct Trust<'a> {
    pub store: &'a TrustStore,
    pub at: i64,
}

impl Document {
    /// Every SIGNED signature field (a /FT /Sig field with a /V dictionary). `bytes` are the bytes this document was
    /// parsed from: the byte ranges point into them.
    pub fn signatures(&self, bytes: &[u8]) -> Vec<Signature> {
        let Some(form) = self.form() else { return Vec::new() };
        let mut out = Vec::new();
        for f in &form.fields {
            if f.kind != FieldKind::Signature {
                continue;
            }
            if let Object::Dict(d) = &f.value {
                out.push(read_sig(self, bytes, f.name.clone(), f.obj, d));
            }
        }
        out
    }
}

impl Signature {
    /// Climbs the rungs (module doc) over `bytes` (the bytes `doc` was parsed from); `trust` = None leaves the chain
    /// unchecked, said by [`Chain::Unchecked`].
    pub fn verify(&self, doc: &Document, bytes: &[u8], trust: Option<&Trust<'_>>) -> Verdict {
        match &self.subfilter[..] {
            b"adbe.pkcs7.detached" | b"ETSI.CAdES.detached" => {}
            b"adbe.pkcs7.sha1" => return Verdict::Refused("adbe.pkcs7.sha1"),
            b"adbe.x509.rsa_sha1" => return Verdict::Refused("adbe.x509.rsa_sha1"),
            b"ETSI.RFC3161" => return Verdict::Refused("ETSI.RFC3161"),
            _ => return Verdict::Refused("subfilter"),
        }
        if let Some(why) = self.refused {
            return Verdict::Refused(why);
        }
        let [(_, a), (b, c)] = self.ranges;
        if b.checked_add(c) != Some(self.signed_end) || self.signed_end > bytes.len() || a > b {
            return Verdict::Refused("byte range");
        }
        let cms = match parse_cms(&self.contents) {
            Ok(x) => x,
            Err(why) => return Verdict::Refused(why),
        };
        let parts = [&bytes[..a], &bytes[b..b + c]];
        let digest = cms.hash.digest(&parts);
        let mut signing_time = None;
        let msg = match cms.signed_attrs {
            Some(raw) => {
                let attrs = match read_attrs(raw) {
                    Ok(x) => x,
                    Err(e) => return Verdict::Refused(der_why(e)),
                };
                if attrs.content_type != Some(ID_DATA) {
                    return Verdict::Refused("content type");
                }
                let Some(md) = attrs.message_digest else { return Verdict::Refused("message digest") };
                if md != &digest[..] {
                    return Verdict::DigestMismatch;
                }
                signing_time = attrs.signing_time;
                // RFC 5652 §5.4: the signature is over the attributes' DER with the EXPLICIT SET OF tag.
                let mut m = raw.to_vec();
                m[0] = tag::SET;
                m
            }
            None => parts.concat(),
        };
        let alg = signature_algorithm(cms.sig_alg, cms.hash);
        let provider = CryptoCoreProvider::with_rng_pool(Vec::new());
        match verify_signed(&provider, &cms.signer.public_key, alg, &msg, cms.signature) {
            Ok(()) => {}
            Err(CertError::UnsupportedSignatureAlgorithm) => return Verdict::Refused("signature algorithm"),
            Err(_) => return Verdict::BadSignature,
        }
        let chain = match trust {
            None => Chain::Unchecked,
            Some(t) => {
                let mut list = vec![cms.signer.der.clone()];
                list.extend(cms.others);
                match verify_server_chain_path(&provider, t.store, t.at, &list, None) {
                    Ok(_) => Chain::Trusted,
                    Err(e) => Chain::Refused(e),
                }
            }
        };
        let changes = self.changes(doc, bytes);
        Verdict::Signed { chain, changes, signing_time, timestamp: cms.timestamp }
    }

    /// Rung 8: the objects touched after the signed revision, judged under the certification P and /FieldMDP.
    fn changes(&self, doc: &Document, bytes: &[u8]) -> Changes {
        if self.signed_end >= bytes.len() {
            return Changes::None;
        }
        let (Ok(old), Ok(new)) = (xref::load(&bytes[..self.signed_end]), xref::load(bytes)) else {
            return Changes::Unknown;
        };
        let touched: Vec<u32> =
            new.entries.iter().filter(|(n, e)| old.entries.get(*n) != Some(*e)).map(|(n, _)| *n).collect();
        if touched.is_empty() {
            return Changes::None;
        }
        let Some(p) = self.doc_mdp.or_else(|| certification_p(doc)) else {
            return Changes::Unrestricted { touched };
        };
        let cat = doc.catalog();
        let acroform = cat.as_ref().and_then(|c| c.get(b"AcroForm")).and_then(Object::as_ref).map(|r| r.num);
        let root = doc.trailer().get(b"Root").and_then(Object::as_ref).map(|r| r.num);
        for &n in &touched {
            let generation = match new.entries.get(&n) {
                Some(XrefEntry::InFile { generation, .. }) => *generation,
                Some(XrefEntry::InStream { .. }) => 0,
                _ => return Changes::Disallowed { p, offender: n },
            };
            let o = doc.resolve(Ref { num: n, generation }).unwrap_or(Object::Null);
            let allowed = match classify(&o, n, acroform, root) {
                Kind::Form => p >= 2 && !self.locks_field(doc, &o),
                Kind::Annot => p >= 3,
                Kind::Other => false,
            };
            if !allowed {
                return Changes::Disallowed { p, offender: n };
            }
        }
        Changes::Allowed { p, touched }
    }

    /// A field dictionary this signature's /FieldMDP locks.
    fn locks_field(&self, doc: &Document, o: &Object) -> bool {
        let Some(f) = &self.field_mdp else { return false };
        let Object::Dict(d) = o else { return false };
        if !d.contains(b"FT") && !d.contains(b"T") {
            return false;
        }
        let name = full_name(doc, d);
        let listed = f.fields.iter().any(|x| *x == name);
        match f.action {
            FieldMdpAction::All => true,
            FieldMdpAction::Include => listed,
            FieldMdpAction::Exclude => !listed,
        }
    }
}

impl Verdict {
    /// The probe word: `refused(<why>)`, `digest-mismatch`, `bad-signature`, `signed+<chain>[+modified...]`.
    pub fn word(&self) -> String {
        match self {
            Verdict::Refused(why) => format!("refused({})", why.replace(' ', "-")),
            Verdict::DigestMismatch => "digest-mismatch".into(),
            Verdict::BadSignature => "bad-signature".into(),
            Verdict::Signed { chain, changes, .. } => {
                let mut w = String::from(match chain {
                    Chain::Unchecked => "signed+chain-unchecked",
                    Chain::Trusted => "signed+trusted",
                    Chain::Refused(_) => "signed+chain-refused",
                });
                w.push_str(match changes {
                    Changes::None => "",
                    Changes::Allowed { .. } => "+modified-allowed",
                    Changes::Disallowed { .. } => "+modified-disallowed",
                    Changes::Unrestricted { .. } => "+modified",
                    Changes::Unknown => "+modified-unknown",
                });
                if *chain == Chain::Trusted {
                    w.push_str("+revocation-unchecked");
                }
                w
            }
        }
    }
}

/// pdfprobe's line: `sigs=<n> <field>:<verdict> ...` (no anchors: chains unchecked).
pub fn probe_line(doc: &Document, bytes: &[u8]) -> String {
    let sigs = doc.signatures(bytes);
    let mut s = format!("sigs={}", sigs.len());
    for g in &sigs {
        let name: String = g.field.chars().map(|c| if c.is_whitespace() { '_' } else { c }).collect();
        let name = if name.is_empty() { String::from("-") } else { name };
        s.push_str(&format!(" {name}:{}", g.verify(doc, bytes, None).word()));
    }
    s
}

fn read_sig(doc: &Document, bytes: &[u8], field: String, obj: Option<Ref>, d: &Dict) -> Signature {
    let text = |k: &[u8]| d.get(k).map(|o| doc.get(o)).and_then(|o| o.as_str().map(text_string));
    let subfilter = d.get(b"SubFilter").map(|o| doc.get(o)).and_then(|o| o.as_name().map(<[u8]>::to_vec));
    let (doc_mdp, field_mdp) = transforms(doc, d);
    let mut s = Signature {
        field,
        obj,
        ranges: [(0, 0); 2],
        subfilter: subfilter.unwrap_or_default(),
        name: text(b"Name"),
        reason: text(b"Reason"),
        location: text(b"Location"),
        m: text(b"M"),
        contents: Vec::new(),
        signed_end: 0,
        doc_mdp,
        field_mdp,
        refused: None,
    };
    match byte_range(doc, bytes, d) {
        Ok((ranges, contents)) => {
            s.ranges = ranges;
            s.signed_end = ranges[1].0 + ranges[1].1;
            s.contents = contents;
        }
        Err(why) => s.refused = Some(why),
    }
    s
}

/// Rung 2: /ByteRange exactness; returns the ranges and the CMS DER read from the gap.
fn byte_range(doc: &Document, bytes: &[u8], d: &Dict) -> Result<([(usize, usize); 2], Vec<u8>), &'static str> {
    const WHY: &str = "byte range";
    let br = doc.get(d.get(b"ByteRange").ok_or(WHY)?);
    let arr = br.as_array().ok_or(WHY)?;
    if arr.len() != 4 {
        return Err(WHY);
    }
    let mut v = [0usize; 4];
    for (i, o) in arr.iter().enumerate() {
        match doc.get(o) {
            Object::Int(n) if n >= 0 => v[i] = n as usize,
            _ => return Err(WHY),
        }
    }
    let [s0, a, b, c] = v;
    let end = b.checked_add(c).ok_or(WHY)?;
    if s0 != 0 || a >= b || b - a < 2 || end > bytes.len() {
        return Err(WHY);
    }
    if bytes[a] != b'<' || bytes[b - 1] != b'>' {
        return Err(WHY);
    }
    // The signed revision ends at its %%EOF (and the end-of-line after it).
    let mut e = end;
    while e > 0 && matches!(bytes[e - 1], b'\r' | b'\n') {
        e -= 1;
    }
    if !bytes[..e].ends_with(b"%%EOF") {
        return Err(WHY);
    }
    let mut raw = Vec::with_capacity((b - a) / 2);
    let mut hi: Option<u8> = None;
    for &ch in &bytes[a + 1..b - 1] {
        if is_white(ch) {
            continue;
        }
        let n = hex_val(ch).ok_or(WHY)?;
        match hi.take() {
            None => hi = Some(n),
            Some(h) => raw.push((h << 4) | n),
        }
    }
    if let Some(h) = hi {
        raw.push(h << 4);
    }
    // The gap must BE this dictionary's /Contents (an encrypted file's strings are decrypted on resolve: not compared).
    if doc.security().is_none() {
        match d.get(b"Contents").map(|o| doc.get(o)) {
            Some(Object::String(sv)) if sv == raw => {}
            _ => return Err(WHY),
        }
    }
    if raw.get(1) == Some(&0x80) {
        return Err("BER");
    }
    let n = Der::new(&raw).tlv().map_err(der_why)?.raw.len();
    if raw[n..].iter().any(|&x| x != 0) {
        return Err("contents padding");
    }
    raw.truncate(n);
    Ok(([(0, a), (b, c)], raw))
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// The /Reference transforms: DocMDP /P (default 2) and FieldMDP.
fn transforms(doc: &Document, d: &Dict) -> (Option<u8>, Option<FieldMdp>) {
    let mut mdp = None;
    let mut fmdp = None;
    let refs = doc.get(d.get(b"Reference").unwrap_or(&Object::Null));
    for r in refs.as_array().unwrap_or(&[]) {
        let Object::Dict(rd) = doc.get(r) else { continue };
        let params = match rd.get(b"TransformParams").map(|o| doc.get(o)) {
            Some(Object::Dict(p)) => p,
            _ => Dict::new(),
        };
        let method = rd.get(b"TransformMethod").map(|o| doc.get(o)).unwrap_or(Object::Null);
        match method.as_name() {
            Some(b"DocMDP") => {
                let p = params.get(b"P").map(|o| doc.get(o)).and_then(|o| o.as_int()).unwrap_or(2);
                mdp = Some(p.clamp(1, 3) as u8);
            }
            Some(b"FieldMDP") => {
                let action = params.get(b"Action").map(|o| doc.get(o)).unwrap_or(Object::Null);
                let action = match action.as_name() {
                    Some(b"All") => FieldMdpAction::All,
                    Some(b"Include") => FieldMdpAction::Include,
                    Some(b"Exclude") => FieldMdpAction::Exclude,
                    _ => continue,
                };
                let list = params.get(b"Fields").map(|o| doc.get(o)).unwrap_or(Object::Null);
                let fields = list
                    .as_array()
                    .unwrap_or(&[])
                    .iter()
                    .filter_map(|o| doc.get(o).as_str().map(text_string))
                    .collect();
                fmdp = Some(FieldMdp { action, fields });
            }
            _ => {}
        }
    }
    (mdp, fmdp)
}

/// The certification signature's P: the catalog's /Perms /DocMDP dictionary, else any signature carrying a DocMDP
/// /Reference.
fn certification_p(doc: &Document) -> Option<u8> {
    let cat = doc.catalog()?;
    if let Some(perms) = cat.get(b"Perms").map(|o| doc.get(o)) {
        if let Some(Object::Dict(sd)) = perms.as_dict().and_then(|p| p.get(b"DocMDP")).map(|o| doc.get(o)) {
            return Some(transforms(doc, &sd).0.unwrap_or(2));
        }
    }
    let form = doc.form()?;
    form.fields.iter().filter(|f| f.kind == FieldKind::Signature).find_map(|f| match &f.value {
        Object::Dict(d) => transforms(doc, d).0,
        _ => None,
    })
}

/// What a touched object is, for /DocMDP.
enum Kind {
    /// Form filling and signing (P >= 2).
    Form,
    /// Annotations and the page dictionaries holding them (P = 3).
    Annot,
    Other,
}

fn classify(o: &Object, n: u32, acroform: Option<u32>, root: Option<u32>) -> Kind {
    if Some(n) == acroform || Some(n) == root {
        return Kind::Form;
    }
    let d = match o {
        Object::Dict(d) => d,
        Object::Stream(s) => &s.dict,
        _ => return Kind::Other,
    };
    let ty = d.name(b"Type");
    let sub = d.name(b"Subtype");
    match (ty, sub) {
        (Some(b"Sig" | b"DocTimeStamp" | b"XRef" | b"ObjStm" | b"Font"), _) => Kind::Form,
        (_, Some(b"Widget")) => Kind::Form,
        (_, Some(b"Form")) if matches!(o, Object::Stream(_)) => Kind::Form,
        (Some(b"Page" | b"Annot"), _) => Kind::Annot,
        (_, Some(_)) if d.contains(b"Rect") => Kind::Annot,
        _ if d.contains(b"FT") || d.contains(b"T") || (d.contains(b"Parent") && sub.is_none()) => Kind::Form,
        _ => Kind::Other,
    }
}

/// The digest algorithms read (SHA-1 is refused by name).
#[derive(Clone, Copy)]
enum Hash {
    Sha256,
    Sha384,
    Sha512,
}

impl Hash {
    fn digest(self, parts: &[&[u8]]) -> Vec<u8> {
        match self {
            Hash::Sha256 => {
                let mut h = Sha256::new();
                parts.iter().for_each(|p| h.update(p));
                h.finalize().to_vec()
            }
            Hash::Sha384 => {
                let mut h = Sha384::new();
                parts.iter().for_each(|p| h.update(p));
                h.finalize().to_vec()
            }
            Hash::Sha512 => {
                let mut h = Sha512::new();
                parts.iter().for_each(|p| h.update(p));
                h.finalize().to_vec()
            }
        }
    }

    fn alg(self) -> HashAlg {
        match self {
            Hash::Sha256 => HashAlg::Sha256,
            Hash::Sha384 => HashAlg::Sha384,
            Hash::Sha512 => HashAlg::Sha512,
        }
    }
}

/// A SignedData's parts (RFC 5652 §5.1).
struct SignedData<'a> {
    econtent_type: &'a [u8],
    econtent: Option<&'a [u8]>,
    /// Each CertificateChoices `certificate` (a SEQUENCE), raw.
    certs: Vec<&'a [u8]>,
    signers: Vec<Der<'a>>,
}

fn signed_data(der: &[u8]) -> Result<SignedData<'_>, CertError> {
    let mut outer = Der::new(der);
    let mut ci = outer.sequence()?;
    outer.expect_end()?;
    if ci.oid()? != ID_SIGNED_DATA {
        return Err(CertError::BadDer("not signedData"));
    }
    let explicit = ci.expect(tag::context_constructed(0))?;
    let mut sd = Der::new(explicit.value).sequence()?;
    sd.small_uint()?;
    sd.expect(tag::SET)?;
    let mut eci = sd.sequence()?;
    let econtent_type = eci.oid()?;
    let econtent = match eci.optional(tag::context_constructed(0))? {
        Some(t) => Some(Der::new(t.value).expect(tag::OCTET_STRING)?.value),
        None => None,
    };
    let mut certs = Vec::new();
    if let Some(cs) = sd.optional(tag::context_constructed(0))? {
        let mut d = Der::new(cs.value);
        while !d.is_empty() {
            let t = d.tlv()?;
            if t.tag == tag::SEQUENCE {
                certs.push(t.raw);
            }
        }
    }
    sd.optional(tag::context_constructed(1))?;
    let si = sd.expect(tag::SET)?;
    let mut signers = Vec::new();
    let mut d = Der::new(si.value);
    while !d.is_empty() {
        signers.push(d.sequence()?);
    }
    Ok(SignedData { econtent_type, econtent, certs, signers })
}

/// The one signer of a detached signature.
struct Cms<'a> {
    hash: Hash,
    /// The signedAttrs element, raw (its first byte is the [0] IMPLICIT tag).
    signed_attrs: Option<&'a [u8]>,
    /// The signatureAlgorithm AlgorithmIdentifier's contents.
    sig_alg: &'a [u8],
    signature: &'a [u8],
    signer: Certificate,
    /// The other certificates of the bag (chain candidates), DER.
    others: Vec<Vec<u8>>,
    timestamp: Option<i64>,
}

fn der_why(e: CertError) -> &'static str {
    match e {
        CertError::BadDer("indefinite length") => "BER",
        _ => "cms",
    }
}

/// Rung 3.
fn parse_cms(der: &[u8]) -> Result<Cms<'_>, &'static str> {
    let sd = signed_data(der).map_err(der_why)?;
    if sd.econtent_type != ID_DATA || sd.econtent.is_some() {
        return Err("encapsulated content");
    }
    if sd.signers.len() != 1 {
        return Err("signer count");
    }
    let mut si = sd.signers[0].clone();
    si.small_uint().map_err(der_why)?;
    let sid = si.tlv().map_err(der_why)?;
    let mut da = si.sequence().map_err(der_why)?;
    let hash = match da.oid().map_err(der_why)? {
        x if x == oid::SHA256 => Hash::Sha256,
        x if x == oid::SHA384 => Hash::Sha384,
        x if x == oid::SHA512 => Hash::Sha512,
        x if x == oid::SHA1 => return Err("sha1"),
        _ => return Err("digest algorithm"),
    };
    let signed_attrs = si.optional(tag::context_constructed(0)).map_err(der_why)?.map(|t| t.raw);
    let sig_alg = si.expect(tag::SEQUENCE).map_err(der_why)?.value;
    let signature = si.expect(tag::OCTET_STRING).map_err(der_why)?.value;
    let unsigned = si.optional(tag::context_constructed(1)).map_err(der_why)?;
    let timestamp = unsigned.and_then(|t| timestamp(t.value));

    let mut signer = None;
    let mut others = Vec::new();
    for raw in &sd.certs {
        let Ok(c) = Certificate::parse(raw) else { continue };
        let is_signer = signer.is_none()
            && match sid.tag {
                tag::SEQUENCE => {
                    let mut d = Der::new(sid.value);
                    let issuer = d.expect(tag::SEQUENCE).map(|t| t.raw);
                    let serial = d.expect(tag::INTEGER).map(|t| t.value);
                    matches!((issuer, serial), (Ok(i), Ok(s)) if c.issuer == i && c.serial == s)
                }
                t if t == tag::context_primitive(0) => c.subject_key_id.as_deref() == Some(sid.value),
                _ => false,
            };
        if is_signer {
            signer = Some(c);
        } else {
            others.push(c.der);
        }
    }
    let signer = signer.ok_or("signer certificate")?;
    Ok(Cms { hash, signed_attrs, sig_alg, signature, signer, others, timestamp })
}

/// rsaEncryption names only the key: its hash is the digest algorithm's. Anything else is tls_core's mapping.
fn signature_algorithm(sig_alg: &[u8], hash: Hash) -> SignatureAlgorithm {
    if Der::new(sig_alg).oid().is_ok_and(|id| id == oid::RSA_ENCRYPTION) {
        return SignatureAlgorithm::RsaPkcs1(hash.alg());
    }
    parse_signature_algorithm(sig_alg).unwrap_or(SignatureAlgorithm::Unsupported)
}

struct Attrs<'a> {
    content_type: Option<&'a [u8]>,
    message_digest: Option<&'a [u8]>,
    signing_time: Option<i64>,
}

/// The signed attributes (`raw` is the whole [0] element).
fn read_attrs(raw: &[u8]) -> Result<Attrs<'_>, CertError> {
    let el = Der::new(raw).tlv()?;
    let mut d = Der::new(el.value);
    let mut out = Attrs { content_type: None, message_digest: None, signing_time: None };
    while !d.is_empty() {
        let mut a = d.sequence()?;
        let id = a.oid()?;
        let set = a.expect(tag::SET)?;
        let first = Der::new(set.value).tlv()?;
        if id == ID_CONTENT_TYPE && first.tag == tag::OID {
            out.content_type = Some(first.value);
        } else if id == ID_MESSAGE_DIGEST && first.tag == tag::OCTET_STRING {
            out.message_digest = Some(first.value);
        } else if id == ID_SIGNING_TIME {
            out.signing_time = parse_time(&first).ok();
        }
    }
    Ok(out)
}

/// Rung 7: an unsigned timeStampToken's TSTInfo genTime (RFC 3161 §2.4.2) — read, NOT verified. A genTime with
/// fractional seconds reads as None (tls_core's `parse_time` takes YYYYMMDDHHMMSSZ).
fn timestamp(unsigned: &[u8]) -> Option<i64> {
    let mut d = Der::new(unsigned);
    while !d.is_empty() {
        let mut a = d.sequence().ok()?;
        if a.oid().ok()? != ID_TIMESTAMP_TOKEN {
            continue;
        }
        let set = a.expect(tag::SET).ok()?;
        let token = Der::new(set.value).tlv().ok()?;
        let sd = signed_data(token.raw).ok()?;
        if sd.econtent_type != ID_TST_INFO {
            return None;
        }
        let mut tst = Der::new(sd.econtent?).sequence().ok()?;
        tst.small_uint().ok()?;
        tst.oid().ok()?;
        tst.sequence().ok()?;
        tst.expect(tag::INTEGER).ok()?;
        let gen_time = tst.tlv().ok()?;
        return parse_time(&gen_time).ok();
    }
    None
}
