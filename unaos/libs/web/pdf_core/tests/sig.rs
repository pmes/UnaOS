// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PDFSIG (LEDGER SR337) KATs. Everything is made from crypto_core's own signers: a CA and a leaf certificate
//! (sha256WithRSA, 2048-bit keys generated deterministically from a SHA-256 counter stream by
//! `rsa_private::PrivateKey::generate`) written by tls_core's `x509::build`, a detached CMS SignedData written by
//! `pdf_core::sig::cms::build` (PDFSIGN SR350 moved both builders out of this file), and a one-page PDF with a /FT /Sig
//! field whose /Sig dictionary carries a fixed-width /ByteRange and an 8192-hex-digit /Contents placeholder, patched in
//! place. PDFSIGN (LEDGER SR350) KATs at the end: `Document::sign` then `Signature::verify` in the same test.

mod common;

use std::sync::OnceLock;

use common::{Body, PdfBuilder, incremental, replace_bytes};
use crypto_core::Sha256;
use pdf_core::crypt::Cipher;
use pdf_core::sig::cms::{self, PrivateKey, SecretKey, SignerKey};
use pdf_core::sig::{Chain, Changes, SignRequest, SubFilter, Trust, Verdict, probe_line};
use pdf_core::write::Edit;
use pdf_core::{Document, Error};
use tls_core::CertError;
use tls_core::x509::build::{self, CertSpec};
use tls_core::x509::verify::TrustStore;

/// 2026-10-27T12:00:00Z, inside both certificates' validity (2025-01-01 .. 2035-12-31).
const AT: i64 = 1_793_102_400;
const NOT_BEFORE: i64 = 1_735_689_600;
const NOT_AFTER: i64 = 2_082_672_000;
const PLACE: usize = 8192;
const BR_ZERO: &str = "[0 0000000000 0000000000 0000000000]";

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
    ec: SecretKey,
    ca_der: Vec<u8>,
    leaf_der: Vec<u8>,
    ec_der: Vec<u8>,
}

fn keys() -> &'static Keys {
    static K: OnceLock<Keys> = OnceLock::new();
    K.get_or_init(|| {
        let ca = gen_key(1);
        let leaf = gen_key(2);
        let other = gen_key(3);
        let ec = SecretKey::from_bytes(&[0x11; 32]).unwrap();
        let (cak, leafk, eck) = (SignerKey::Rsa(&ca), SignerKey::Rsa(&leaf), SignerKey::P256(&ec));
        let ca_der = cert(1, "PDFSIG KAT CA", &cak, &cak, true);
        let leaf_der = cert(2, "PDFSIG KAT Signer", &leafk, &cak, false);
        let ec_der = cert(3, "PDFSIGN KAT EC Signer", &eck, &cak, false);
        Keys { leaf, other, ec, ca_der, leaf_der, ec_der }
    })
}

/// A certificate issued by the CA name through tls_core's writer, signed by `issuer_key`.
fn cert(serial: u8, subject: &str, key: &SignerKey<'_>, issuer_key: &SignerKey<'_>, ca: bool) -> Vec<u8> {
    let spki = key.spki();
    let alg = issuer_key.cert_alg();
    let spec = CertSpec {
        serial: &[serial],
        issuer: "PDFSIG KAT CA",
        subject,
        not_before: NOT_BEFORE,
        not_after: NOT_AFTER,
        spki: &spki,
        ca,
    };
    let tbs = build::tbs(&spec, &alg);
    build::certificate(&tbs, &alg, &issuer_key.sign(&tbs))
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
    let der = cms::build(&SignerKey::Rsa(key), &digest, certs, AT, false).unwrap();
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

// ---- PDFSIGN (LEDGER SR350): Document::sign, verified by Signature::verify ----

const ENTROPY: [u8; 32] = [7; 32];

/// One page. `fields`: the AcroForm with two EMPTY signature fields Sig1 (4) and Sig2 (5) and a text field Name (6);
/// else no form and no /Annots.
fn blank(fields: bool) -> Vec<u8> {
    let mut b = PdfBuilder::new(1);
    if fields {
        b.obj(1, "<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [4 0 R 5 0 R 6 0 R] /SigFlags 3 >> >>")
            .obj(3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 7 0 R /Annots [4 0 R 5 0 R 6 0 R] >>")
            .obj(4, "<< /Type /Annot /Subtype /Widget /FT /Sig /T (Sig1) /Rect [0 0 0 0] /F 132 /P 3 0 R >>")
            .obj(5, "<< /Type /Annot /Subtype /Widget /FT /Sig /T (Sig2) /Rect [0 0 0 0] /F 132 /P 3 0 R >>")
            .obj(6, "<< /Type /Annot /Subtype /Widget /FT /Tx /T (Name) /Rect [0 0 20 10] /P 3 0 R /V (before) >>");
    } else {
        b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>")
            .obj(3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 40 40] /Contents 7 0 R >>");
    }
    b.obj(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>").stream(7, "", b"0 0 m 40 40 l S\n");
    b.classic()
}

fn request<'a>(field: &'a str, key: SignerKey<'a>, chain: &'a [&'a [u8]]) -> SignRequest<'a> {
    SignRequest {
        field,
        page: 0,
        key,
        chain,
        subfilter: SubFilter::Pkcs7Detached,
        name: Some("PDFSIGN KAT Signer"),
        reason: Some("PDFSIGN KAT"),
        location: Some("Bench"),
        time: AT,
        certify: None,
    }
}

fn rsa_chain() -> [&'static [u8]; 2] {
    let k = keys();
    [k.leaf_der.as_slice(), k.ca_der.as_slice()]
}

/// Sign `pdf` (opened with `pw` when given) and return the new file.
fn sign(pdf: &[u8], pw: Option<&[u8]>, req: &SignRequest<'_>) -> Result<Vec<u8>, Error> {
    let doc = match pw {
        Some(p) => Document::parse_with_password(pdf, p).unwrap(),
        None => Document::parse(pdf).unwrap(),
    };
    doc.sign(pdf, req, &ENTROPY)
}

/// Every signature's (field, verdict) with the CA as the one anchor.
fn verdicts(pdf: &[u8], pw: Option<&[u8]>) -> Vec<(String, Verdict)> {
    let doc = match pw {
        Some(p) => Document::parse_with_password(pdf, p).unwrap(),
        None => Document::parse(pdf).unwrap(),
    };
    let mut store = TrustStore::new();
    store.add_der(&keys().ca_der).unwrap();
    let trust = Trust { store: &store, at: AT };
    doc.signatures(pdf).iter().map(|s| (s.field.clone(), s.verify(&doc, pdf, Some(&trust)))).collect()
}

fn trusted_whole(v: &Verdict) -> bool {
    matches!(v, Verdict::Signed { chain: Chain::Trusted, changes: Changes::None, signing_time: Some(AT), .. })
}

#[test]
fn sign_unsigned_rsa_trusted() {
    let pdf = blank(false);
    let chain = rsa_chain();
    let out = sign(&pdf, None, &request("Signature1", SignerKey::Rsa(&keys().leaf), &chain)).unwrap();
    assert!(out.starts_with(&pdf), "an incremental update keeps the original as a prefix");
    let v = verdicts(&out, None);
    assert_eq!(v.len(), 1);
    assert_eq!(v[0].0, "Signature1");
    assert!(trusted_whole(&v[0].1), "{:?}", v[0].1);
    let doc = Document::parse(&out).unwrap();
    let sig = &doc.signatures(&out)[0];
    assert_eq!(sig.subfilter, b"adbe.pkcs7.detached");
    assert_eq!(sig.name.as_deref(), Some("PDFSIGN KAT Signer"));
    assert_eq!(sig.reason.as_deref(), Some("PDFSIGN KAT"));
    assert_eq!(sig.location.as_deref(), Some("Bench"));
    assert_eq!(sig.m.as_deref(), Some("D:20261027120000Z"));
    assert_eq!(sig.signed_end, out.len());
    assert_eq!(probe_line(&doc, &out), "sigs=1 Signature1:signed+chain-unchecked");
}

#[test]
fn second_signature_keeps_first() {
    let chain = rsa_chain();
    let leaf = SignerKey::Rsa(&keys().leaf);
    let mut first = request("Sig1", leaf, &chain);
    first.certify = Some(2);
    let once = sign(&blank(true), None, &first).unwrap();
    let twice = sign(&once, None, &request("Sig2", SignerKey::Rsa(&keys().leaf), &chain)).unwrap();
    assert!(twice.starts_with(&once));
    let v = verdicts(&twice, None);
    assert_eq!(v.len(), 2);
    for (field, verdict) in &v {
        match (field.as_str(), verdict) {
            ("Sig1", Verdict::Signed { chain: Chain::Trusted, changes: Changes::Allowed { p: 2, touched }, .. }) => {
                assert_eq!(touched.len(), 2, "the Sig2 field and its /Sig dictionary: {touched:?}");
                assert!(touched.contains(&5));
            }
            ("Sig2", v) => assert!(trusted_whole(v), "{v:?}"),
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn sign_encrypted_file() {
    let chain = rsa_chain();
    for cipher in [Cipher::Aes128, Cipher::Aes256] {
        let enc = Document::parse(&blank(true)).unwrap().save_encrypted(b"user", b"owner", -4, cipher, &[3; 32]);
        let out = sign(&enc, Some(b"user"), &request("Sig1", SignerKey::Rsa(&keys().leaf), &chain)).unwrap();
        assert!(out.starts_with(&enc), "{cipher:?}: the signature covers the encrypted file's bytes");
        let v = verdicts(&out, Some(b"user"));
        assert_eq!(v.len(), 1, "{cipher:?}");
        assert!(trusted_whole(&v[0].1), "{cipher:?}: {:?}", v[0].1);
        // The update is under the original's key: the /Sig dictionary's /M is not in the clear, its /Contents is.
        assert!(!out[enc.len()..].windows(17).any(|w| w == b"D:20261027120000Z"), "{cipher:?}");
    }
}

#[test]
fn certify_then_fill() {
    let chain = rsa_chain();
    for (p, allowed) in [(2u8, true), (1, false)] {
        let mut r = request("Sig1", SignerKey::Rsa(&keys().leaf), &chain);
        r.certify = Some(p);
        let signed = sign(&blank(true), None, &r).unwrap();
        let doc = Document::parse(&signed).unwrap();
        let form = doc.form().unwrap();
        let name = form.fields.iter().find(|f| f.name == "Name").unwrap();
        let mut edit = Edit::new(&doc);
        for (r, o) in form.set_value(&doc, name, "after").unwrap() {
            edit.set(r, o);
        }
        let filled = edit.save_update(&signed).unwrap();
        let v = verdicts(&filled, None);
        match (&v[0].1, allowed) {
            (Verdict::Signed { chain: Chain::Trusted, changes: Changes::Allowed { p: 2, touched }, .. }, true) => {
                assert!(touched.contains(&6), "{touched:?}");
            }
            (Verdict::Signed { changes: Changes::Disallowed { p: 1, .. }, .. }, false) => {}
            (other, _) => panic!("P={p}: {other:?}"),
        }
    }
}

#[test]
fn tamper_after_sign() {
    let chain = rsa_chain();
    let out = sign(&blank(false), None, &request("Signature1", SignerKey::Rsa(&keys().leaf), &chain)).unwrap();
    let bad = replace_bytes(&out, b"/MediaBox [0 0 40 40]", b"/MediaBox [0 0 40 41]");
    assert_ne!(bad, out);
    assert_eq!(verdicts(&bad, None)[0].1, Verdict::DigestMismatch);
}

#[test]
fn ecdsa_p256_signer() {
    let k = keys();
    let chain = [k.ec_der.as_slice(), k.ca_der.as_slice()];
    let out = sign(&blank(false), None, &request("Signature1", SignerKey::P256(&k.ec), &chain)).unwrap();
    let v = verdicts(&out, None);
    assert!(trusted_whole(&v[0].1), "{:?}", v[0].1);
}

#[test]
fn deterministic_output() {
    let chain = rsa_chain();
    let r = request("Sig1", SignerKey::Rsa(&keys().leaf), &chain);
    assert_eq!(sign(&blank(true), None, &r).unwrap(), sign(&blank(true), None, &r).unwrap());
    let enc = Document::parse(&blank(true)).unwrap().save_encrypted(b"user", b"owner", -4, Cipher::Aes128, &[3; 32]);
    assert_eq!(sign(&enc, Some(b"user"), &r).unwrap(), sign(&enc, Some(b"user"), &r).unwrap());
}

#[test]
fn sign_refusals() {
    let chain = rsa_chain();
    let leaf = || SignerKey::Rsa(&keys().leaf);
    let signed = sign(&blank(true), None, &request("Sig1", leaf(), &chain)).unwrap();
    fn refused(pdf: &[u8], r: &SignRequest<'_>) -> Option<Error> {
        sign(pdf, None, r).err()
    }
    let why = |w: &'static str| Some(Error::Unsupported(w));
    assert_eq!(refused(&signed, &request("Sig1", leaf(), &chain)), why("field already signed"));
    let mut r = request("Sig2", leaf(), &chain);
    r.certify = Some(2);
    assert_eq!(refused(&signed, &r), why("certification after a signature"));
    r.certify = Some(4);
    assert_eq!(refused(&blank(true), &r), why("DocMDP P"));
    assert_eq!(refused(&blank(true), &request("Name", leaf(), &chain)), why("not a signature field"));
    let mut r = request("Signature9", leaf(), &chain);
    r.page = 5;
    assert_eq!(refused(&blank(false), &r), why("page"));
    assert_eq!(refused(&blank(false), &request("Signature1", leaf(), &[])), why("signer certificate"));
}

#[test]
fn cades_subfilter() {
    let chain = rsa_chain();
    let mut r = request("Signature1", SignerKey::Rsa(&keys().leaf), &chain);
    r.subfilter = SubFilter::CadesDetached;
    let out = sign(&blank(false), None, &r).unwrap();
    let doc = Document::parse(&out).unwrap();
    assert_eq!(doc.signatures(&out)[0].subfilter, b"ETSI.CAdES.detached");
    assert!(trusted_whole(&verdicts(&out, None)[0].1));
}
