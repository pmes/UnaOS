// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PDFSIG (LEDGER SR337) KATs. A test-side builder makes everything from crypto_core's own signer: a CA and a leaf
//! certificate (sha256WithRSA, 2048-bit keys generated deterministically from a SHA-256 counter stream by
//! `rsa_private::PrivateKey::generate`), a detached CMS SignedData (signed attributes contentType / messageDigest /
//! signingTime, signer by issuerAndSerialNumber, rsaEncryption), and a one-page PDF with a /FT /Sig field whose /Sig
//! dictionary carries a fixed-width /ByteRange and an 8192-hex-digit /Contents placeholder, patched in place.

mod common;

use std::sync::OnceLock;

use common::{Body, PdfBuilder, incremental, replace_bytes};
use crypto_core::Sha256;
use crypto_core::rsa_private::PrivateKey;
use pdf_core::Document;
use pdf_core::sig::{Chain, Changes, Trust, Verdict, probe_line};
use tls_core::CertError;
use tls_core::x509::der::encode_tlv as tlv;
use tls_core::x509::verify::TrustStore;

const OID_CN: &[u8] = &[0x55, 0x04, 0x03];
const OID_BC: &[u8] = &[0x55, 0x1d, 0x13];
const OID_KU: &[u8] = &[0x55, 0x1d, 0x0f];
const OID_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
const OID_SHA256_RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x0b];
const OID_SHA256: &[u8] = &[0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01];
const OID_DATA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x01];
const OID_SIGNED_DATA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x07, 0x02];
const OID_CONTENT_TYPE: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x03];
const OID_MESSAGE_DIGEST: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x04];
const OID_SIGNING_TIME: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x09, 0x05];

/// 2026-10-27T12:00:00Z, inside both certificates' validity (2025-01-01 .. 2035-12-31).
const AT: i64 = 1_793_102_400;
const PLACE: usize = 8192;
const BR_ZERO: &str = "[0 0000000000 0000000000 0000000000]";

fn seq(parts: &[&[u8]]) -> Vec<u8> {
    tlv(0x30, &parts.concat())
}
fn set(parts: &[&[u8]]) -> Vec<u8> {
    tlv(0x31, &parts.concat())
}
fn oid(v: &[u8]) -> Vec<u8> {
    tlv(0x06, v)
}
fn null() -> Vec<u8> {
    tlv(0x05, &[])
}
fn octets(v: &[u8]) -> Vec<u8> {
    tlv(0x04, v)
}
fn utc(s: &str) -> Vec<u8> {
    tlv(0x17, s.as_bytes())
}
/// A non-negative INTEGER from big-endian magnitude bytes.
fn int(v: &[u8]) -> Vec<u8> {
    let mut m: Vec<u8> = v.iter().copied().skip_while(|&b| b == 0).collect();
    if m.is_empty() || m[0] & 0x80 != 0 {
        m.insert(0, 0);
    }
    tlv(0x02, &m)
}
fn bits(v: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8];
    b.extend_from_slice(v);
    tlv(0x03, &b)
}
fn name(cn: &str) -> Vec<u8> {
    seq(&[&set(&[&seq(&[&oid(OID_CN), &tlv(0x0c, cn.as_bytes())])])])
}
fn alg(o: &[u8]) -> Vec<u8> {
    seq(&[&oid(o), &null()])
}

fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut h = Sha256::new();
    for p in parts {
        h.update(p);
    }
    h.finalize()
}

/// A deterministic 2048-bit key: the rng is SHA-256(seed || counter).
fn gen_key(seed: u8) -> PrivateKey {
    let mut ctr = 0u64;
    let mut rng = move |out: &mut [u8]| {
        for chunk in out.chunks_mut(32) {
            let d = sha256(&[&[seed], &ctr.to_be_bytes()]);
            ctr += 1;
            chunk.copy_from_slice(&d[..chunk.len()]);
        }
    };
    PrivateKey::generate(2048, &[1, 0, 1], &mut rng).unwrap()
}

struct Keys {
    leaf: PrivateKey,
    other: PrivateKey,
    ca_der: Vec<u8>,
    leaf_der: Vec<u8>,
}

fn keys() -> &'static Keys {
    static K: OnceLock<Keys> = OnceLock::new();
    K.get_or_init(|| {
        let ca = gen_key(1);
        let leaf = gen_key(2);
        let other = gen_key(3);
        let ca_der = cert(1, "PDFSIG KAT CA", "PDFSIG KAT CA", &ca, &ca, true);
        let leaf_der = cert(2, "PDFSIG KAT CA", "PDFSIG KAT Signer", &leaf, &ca, false);
        Keys { leaf, other, ca_der, leaf_der }
    })
}

fn sign(key: &PrivateKey, msg: &[u8]) -> Vec<u8> {
    let mut out = vec![0u8; 512];
    let k = key.sign_pkcs1v15(&sha256(&[msg]), &mut out).unwrap();
    out.truncate(k);
    out
}

fn spki(key: &PrivateKey) -> Vec<u8> {
    let mut n = [0u8; 1024];
    let mut e = [0u8; 16];
    let nl = key.public_key().n_be(&mut n);
    let el = key.public_key().e_be(&mut e);
    seq(&[&alg(OID_RSA), &bits(&seq(&[&int(&n[..nl]), &int(&e[..el])]))])
}

fn cert(serial: u8, issuer: &str, subject: &str, key: &PrivateKey, issuer_key: &PrivateKey, ca: bool) -> Vec<u8> {
    let crit = tlv(0x01, &[0xff]);
    let exts = if ca {
        let bc = seq(&[&oid(OID_BC), &crit, &octets(&seq(&[&tlv(0x01, &[0xff])]))]);
        // keyCertSign (bit 5) + cRLSign (bit 6): 0x06 with one unused bit.
        let ku = seq(&[&oid(OID_KU), &crit, &octets(&tlv(0x03, &[0x01, 0x06]))]);
        seq(&[&bc, &ku])
    } else {
        // digitalSignature (bit 0): 0x80 with seven unused bits.
        seq(&[&seq(&[&oid(OID_KU), &crit, &octets(&tlv(0x03, &[0x07, 0x80]))])])
    };
    let a = alg(OID_SHA256_RSA);
    let tbs = seq(&[
        &tlv(0xa0, &int(&[2])),
        &int(&[serial]),
        &a,
        &name(issuer),
        &seq(&[&utc("250101000000Z"), &utc("351231000000Z")]),
        &name(subject),
        &spki(key),
        &tlv(0xa3, &exts),
    ]);
    seq(&[&tbs, &a, &bits(&sign(issuer_key, &tbs))])
}

/// A detached SignedData over `digest`, signed by `key`, naming the leaf (serial 2 under the CA) as signer.
fn cms(key: &PrivateKey, digest: &[u8], certs: &[&[u8]]) -> Vec<u8> {
    let attrs = [
        seq(&[&oid(OID_CONTENT_TYPE), &set(&[&oid(OID_DATA)])]),
        seq(&[&oid(OID_MESSAGE_DIGEST), &set(&[&octets(digest)])]),
        seq(&[&oid(OID_SIGNING_TIME), &set(&[&utc("261027120000Z")])]),
    ]
    .concat();
    let sig = sign(key, &tlv(0x31, &attrs));
    let signer_info = seq(&[
        &int(&[1]),
        &seq(&[&name("PDFSIG KAT CA"), &int(&[2])]),
        &alg(OID_SHA256),
        &tlv(0xa0, &attrs),
        &alg(OID_RSA),
        &octets(&sig),
    ]);
    let sd = seq(&[
        &int(&[1]),
        &set(&[&alg(OID_SHA256)]),
        &seq(&[&oid(OID_DATA)]),
        &tlv(0xa0, &certs.concat()),
        &set(&[&signer_info]),
    ]);
    seq(&[&oid(OID_SIGNED_DATA), &tlv(0xa0, &sd)])
}

/// The unsigned file: catalog (AcroForm inline; /Perms /DocMDP when `p` is set), one page, the signature field 4
/// (merged widget) with /V 5 0 R, the /Sig dictionary 5, a text field 6, the content stream 7.
fn unsigned(subfilter: &str, p: Option<u8>) -> Vec<u8> {
    let perms = if p.is_some() { " /Perms << /DocMDP 5 0 R >>" } else { "" };
    let reference = match p {
        Some(p) => format!(
            " /Reference [<< /Type /SigRef /TransformMethod /DocMDP /TransformParams << /Type /TransformParams /P {p} /V /1.2 >> >>]"
        ),
        None => String::new(),
    };
    let zeros = "0".repeat(PLACE);
    let mut b = PdfBuilder::new(1);
    b.obj(1, &format!("<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 6 0 R] /SigFlags 3 >>{perms} >>"))
        .obj(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 7 0 R /Annots [4 0 R 6 0 R] >>")
        .obj(4, "<< /Type /Annot /Subtype /Widget /FT /Sig /T (Sig1) /Rect [0 0 0 0] /F 132 /P 3 0 R /V 5 0 R >>")
        .obj(
            5,
            &format!(
                "<< /Type /Sig /Filter /Adobe.PPKLite /SubFilter /{subfilter} /ByteRange {BR_ZERO} /Contents <{zeros}> /M (D:20261027120000Z) /Name (PDFSIG KAT Signer) /Reason (PDFSIG KAT){reference} >>"
            ),
        )
        .obj(6, "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /Rect [0 0 20 10] /P 3 0 R /V (before) >>")
        .stream(7, "", b"0 0 m 40 40 l S\n");
    b.classic()
}

fn find(hay: &[u8], needle: &[u8]) -> usize {
    hay.windows(needle.len()).position(|w| w == needle).unwrap()
}

/// Patches /ByteRange (the last length `short` bytes short of the file's end), then signs the ranges with `key`
/// carrying `certs`.
fn sign_pdf(mut pdf: Vec<u8>, key: &PrivateKey, certs: &[&[u8]], short: usize) -> Vec<u8> {
    let a = find(&pdf, b"/Contents <") + 10;
    let b = a + PLACE + 2;
    let c = pdf.len() - b - short;
    let at = find(&pdf, BR_ZERO.as_bytes());
    let br = format!("[0 {a:010} {b:010} {c:010}]");
    pdf[at..at + br.len()].copy_from_slice(br.as_bytes());
    let digest = sha256(&[&pdf[..a], &pdf[b..b + c]]);
    let der = cms(key, &digest, certs);
    let hex: String = der.iter().map(|x| format!("{x:02X}")).collect();
    assert!(hex.len() <= PLACE, "CMS {} hex digits over the placeholder", hex.len());
    pdf[a + 1..a + 1 + hex.len()].copy_from_slice(hex.as_bytes());
    pdf
}

fn signed(subfilter: &str, p: Option<u8>) -> Vec<u8> {
    let k = keys();
    sign_pdf(unsigned(subfilter, p), &k.leaf, &[&k.leaf_der, &k.ca_der], 0)
}

fn verdict(pdf: &[u8], trust: Option<&Trust<'_>>) -> Verdict {
    let doc = Document::parse(pdf).unwrap();
    let sigs = doc.signatures(pdf);
    assert_eq!(sigs.len(), 1);
    assert_eq!(sigs[0].field, "Sig1");
    sigs[0].verify(&doc, pdf, trust)
}

#[test]
fn valid_detached_pkcs7_sha256_rsa() {
    let pdf = signed("adbe.pkcs7.detached", None);
    let doc = Document::parse(&pdf).unwrap();
    let sig = &doc.signatures(&pdf)[0];
    assert_eq!(sig.subfilter, b"adbe.pkcs7.detached");
    assert_eq!(sig.name.as_deref(), Some("PDFSIG KAT Signer"));
    assert_eq!(sig.reason.as_deref(), Some("PDFSIG KAT"));
    assert_eq!(sig.signed_end, pdf.len());

    // No anchors: the signature is checked, the chain is not — said by name.
    match verdict(&pdf, None) {
        Verdict::Signed { chain: Chain::Unchecked, changes: Changes::None, signing_time, timestamp: None } => {
            assert_eq!(signing_time, Some(AT));
        }
        v => panic!("{v:?}"),
    }
    assert_eq!(probe_line(&doc, &pdf), "sigs=1 Sig1:signed+chain-unchecked");

    // The CA as the one anchor: the path validates.
    let mut store = TrustStore::new();
    store.add_der(&keys().ca_der).unwrap();
    let trust = Trust { store: &store, at: AT };
    match verdict(&pdf, Some(&trust)) {
        Verdict::Signed { chain: Chain::Trusted, changes: Changes::None, .. } => {}
        v => panic!("{v:?}"),
    }
    assert_eq!(verdict(&pdf, Some(&trust)).word(), "signed+trusted+revocation-unchecked");

    // An anchor that did not issue the leaf: the signature holds, the chain is refused.
    let mut wrong = TrustStore::new();
    wrong.add_der(&keys().leaf_der).unwrap();
    let trust = Trust { store: &wrong, at: AT };
    match verdict(&pdf, Some(&trust)) {
        Verdict::Signed { chain: Chain::Refused(CertError::UnknownIssuer), .. } => {}
        v => panic!("{v:?}"),
    }
}

#[test]
fn tampered_byte_is_a_digest_mismatch() {
    let pdf = signed("adbe.pkcs7.detached", None);
    // Inside the first range (the page dictionary precedes the /Sig dictionary); same length, still parses.
    let bad = replace_bytes(&pdf, b"/MediaBox [0 0 40 40]", b"/MediaBox [0 0 40 41]");
    assert_ne!(bad, pdf);
    assert!(find(&bad, b"/MediaBox [0 0 40 41]") < find(&bad, b"/Contents <"));
    assert_eq!(verdict(&bad, None), Verdict::DigestMismatch);
}

#[test]
fn wrong_key_is_a_bad_signature() {
    let k = keys();
    // Signed by `other`, carrying the leaf's certificate: the digest matches, the signature does not.
    let pdf = sign_pdf(unsigned("adbe.pkcs7.detached", None), &k.other, &[&k.leaf_der, &k.ca_der], 0);
    assert_eq!(verdict(&pdf, None), Verdict::BadSignature);
}

#[test]
fn byte_range_short_of_eof_is_refused() {
    let k = keys();
    // The last length stops inside the final %%EOF: the signed span is not the whole revision.
    let pdf = sign_pdf(unsigned("adbe.pkcs7.detached", None), &k.leaf, &[&k.leaf_der, &k.ca_der], 2);
    assert_eq!(verdict(&pdf, None), Verdict::Refused("byte range"));
    let doc = Document::parse(&pdf).unwrap();
    assert_eq!(probe_line(&doc, &pdf), "sigs=1 Sig1:refused(byte-range)");
}

#[test]
fn incremental_fill_after_certification() {
    let fill = |base: &[u8]| {
        let field = b"<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /Rect [0 0 20 10] /P 3 0 R /V (after) >>";
        incremental(base, &[(6, Body::Plain(field.to_vec()))], 1, 8)
    };
    // /DocMDP P=2 permits a form fill.
    let pdf = fill(&signed("adbe.pkcs7.detached", Some(2)));
    match verdict(&pdf, None) {
        Verdict::Signed { chain: Chain::Unchecked, changes: Changes::Allowed { p: 2, touched }, .. } => {
            assert_eq!(touched, vec![6]);
        }
        v => panic!("{v:?}"),
    }
    let doc = Document::parse(&pdf).unwrap();
    assert_eq!(probe_line(&doc, &pdf), "sigs=1 Sig1:signed+chain-unchecked+modified-allowed");

    // /DocMDP P=1 permits nothing.
    let pdf = fill(&signed("adbe.pkcs7.detached", Some(1)));
    match verdict(&pdf, None) {
        Verdict::Signed { changes: Changes::Disallowed { p: 1, offender: 6 }, .. } => {}
        v => panic!("{v:?}"),
    }

    // No certification signature: the change is reported, unrestricted.
    let pdf = fill(&signed("adbe.pkcs7.detached", None));
    match verdict(&pdf, None) {
        Verdict::Signed { changes: Changes::Unrestricted { touched }, .. } => assert_eq!(touched, vec![6]),
        v => panic!("{v:?}"),
    }
}

#[test]
fn unknown_subfilters_refused_by_name() {
    assert_eq!(verdict(&signed("adbe.x509.rsa_sha1", None), None), Verdict::Refused("adbe.x509.rsa_sha1"));
    assert_eq!(verdict(&signed("adbe.pkcs7.sha1", None), None), Verdict::Refused("adbe.pkcs7.sha1"));
    assert_eq!(verdict(&signed("Foo.bar", None), None), Verdict::Refused("subfilter"));
}

#[test]
fn unsigned_file_has_no_signatures() {
    let pdf = common::annot_pdf("", &[("", "")], &[]);
    let doc = Document::parse(&pdf).unwrap();
    assert!(doc.signatures(&pdf).is_empty());
    assert_eq!(probe_line(&doc, &pdf), "sigs=0");
}
