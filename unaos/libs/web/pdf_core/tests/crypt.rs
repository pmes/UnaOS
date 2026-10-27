// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// PDFCRYPT (LEDGER SR315): the standard security handler. The ENCRYPT side lives here, written from the same
// algorithms run forward (ISO 32000-1 §7.6.3.3 Algorithms 1, 2, 3, 4, 5; ISO 32000-2 Algorithms 8 and 9 for R6), so
// PDF1's builder can make RC4-40, RC4-128, AESV2 and AESV3 (R6) files; each opens with the empty user password, with
// a set user password and with the owner password, and a wrong password is Error::Password.
// `PDF_CORE_DUMP=<dir>` writes every file built here into <dir> (for an outside reader to cross-check them).

mod common;

use common::PdfBuilder;
use crypto_core::aes::Aes;
use crypto_core::cbc;
use crypto_core::md5::{Md5, md5};
use crypto_core::rc4::rc4;
use pdf_core::crypt::{PAD, hash_r56, pad_password, rc4_passes};
use pdf_core::{Document, Error, Object, Ref};

const CONTENT: &[u8] = b"BT /F1 12 Tf 72 712 Td (Hello, encrypted world) Tj ET";
const TITLE: &str = "Secret Title";
const XMP: &[u8] = b"<x:xmpmeta xmlns:x='adobe:ns:meta/'/>";
const ID0: &[u8] = b"\x01\x23\x45\x67\x89\xab\xcd\xef\xfe\xdc\xba\x98\x76\x54\x32\x10";
const P: i32 = -3904;

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Rc4_40,
    Rc4_128,
    AesV2,
    AesV3,
}

/// What the writer needs: the file key, the /Encrypt dictionary text, the string and stream methods.
struct Enc {
    kind: Kind,
    key: Vec<u8>,
    dict: String,
    stm_identity: bool,
}

fn hexs(b: &[u8]) -> String {
    let mut s = String::from("<");
    for x in b {
        s.push_str(&format!("{x:02x}"));
    }
    s.push('>');
    s
}

fn rc4v(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut d = data.to_vec();
    rc4(key, &mut d).unwrap();
    d
}

fn cbc_seal(key: &[u8], iv: [u8; 16], data: &[u8]) -> Vec<u8> {
    let mut body = data.to_vec();
    let pad = 16 - body.len() % 16;
    body.extend(std::iter::repeat_n(pad as u8, pad));
    cbc::encrypt(&Aes::new(key).unwrap(), &iv, &mut body).unwrap();
    let mut out = iv.to_vec();
    out.extend_from_slice(&body);
    out
}

fn cbc_nopad(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut d = data.to_vec();
    cbc::encrypt(&Aes::new(key).unwrap(), &[0; 16], &mut d).unwrap();
    d
}

/// Algorithm 2 forward (the same derivation the reader runs).
fn file_key(user: &[u8], o: &[u8], r: i64, n: usize, em: bool) -> Vec<u8> {
    let mut h = Md5::new();
    h.update(&pad_password(user));
    h.update(o);
    h.update(&(P as u32).to_le_bytes());
    h.update(ID0);
    if r >= 4 && !em {
        h.update(&[0xff; 4]);
    }
    let mut d = h.finalize();
    if r >= 3 {
        for _ in 0..50 {
            d = md5(&d[..n]);
        }
    }
    d[..n].to_vec()
}

/// Build the /Encrypt dictionary and the file key for `kind` with these passwords.
fn security(kind: Kind, user: &[u8], owner: &[u8], em: bool, stm_identity: bool) -> Enc {
    let em_txt = if em { "" } else { " /EncryptMetadata false" };
    let stmf = if stm_identity { "/Identity" } else { "/StdCF" };
    if kind == Kind::AesV3 {
        let key: Vec<u8> = (0..32u8).map(|i| i.wrapping_mul(37).wrapping_add(11)).collect();
        let (uvs, uks, ovs, oks) = ([1u8; 8], [2u8; 8], [3u8; 8], [4u8; 8]);
        // Algorithm 8 (R6): U = hash(user, validation salt) + the two salts; UE = the key under hash(user, key salt).
        let mut u = hash_r56(6, user, &uvs, &[]).to_vec();
        u.extend_from_slice(&uvs);
        u.extend_from_slice(&uks);
        let ue = cbc_nopad(&hash_r56(6, user, &uks, &[]), &key);
        // Algorithm 9: the same for the owner, with the 48-byte U as extra data.
        let mut o = hash_r56(6, owner, &ovs, &u).to_vec();
        o.extend_from_slice(&ovs);
        o.extend_from_slice(&oks);
        let oe = cbc_nopad(&hash_r56(6, owner, &oks, &u), &key);
        let dict = format!(
            "<< /Filter /Standard /V 5 /R 6 /Length 256 /O {} /U {} /OE {} /UE {} /P {P} /CF << /StdCF << /CFM /AESV3 /Length 32 /AuthEvent /DocOpen >> >> /StmF {stmf} /StrF /StdCF{em_txt} >>",
            hexs(&o),
            hexs(&u),
            hexs(&oe),
            hexs(&ue)
        );
        return Enc { kind, key, dict, stm_identity };
    }
    let (v, r, n) = match kind {
        Kind::Rc4_40 => (1, 2, 5),
        Kind::Rc4_128 => (2, 3, 16),
        _ => (4, 4, 16),
    };
    // Algorithm 3: /O.
    let mut d = md5(&pad_password(owner));
    if r >= 3 {
        for _ in 0..50 {
            d = md5(&d);
        }
    }
    let mut o = pad_password(user).to_vec();
    if r == 2 {
        o = rc4v(&d[..n], &o);
    } else {
        rc4_passes(&d[..n], &mut o, false);
    }
    let key = file_key(user, &o, r, n, em);
    // Algorithm 4 (R2) / 5 (R3+): /U.
    let u = if r == 2 {
        rc4v(&key, &PAD)
    } else {
        let mut h = Md5::new();
        h.update(&PAD);
        h.update(ID0);
        let mut x = h.finalize().to_vec();
        rc4_passes(&key, &mut x, false);
        x.extend_from_slice(&[0x5a; 16]);
        x
    };
    let cf = if kind == Kind::AesV2 {
        format!(" /CF << /StdCF << /CFM /AESV2 /Length 16 /AuthEvent /DocOpen >> >> /StmF {stmf} /StrF /StdCF")
    } else {
        String::new()
    };
    let dict = format!(
        "<< /Filter /Standard /V {v} /R {r} /Length {} /O {} /U {} /P {P}{cf}{em_txt} >>",
        n * 8,
        hexs(&o),
        hexs(&u)
    );
    Enc { kind, key, dict, stm_identity }
}

/// Algorithm 1 forward: encrypt `data` as object `num` 0.
fn seal(e: &Enc, num: u32, data: &[u8], stream: bool) -> Vec<u8> {
    if stream && e.stm_identity {
        return data.to_vec();
    }
    let iv = [num as u8 ^ 0xa5; 16];
    let objkey = |aes: bool| {
        let mut h = Md5::new();
        h.update(&e.key);
        h.update(&num.to_le_bytes()[..3]);
        h.update(&0u16.to_le_bytes());
        if aes {
            h.update(b"sAlT");
        }
        h.finalize()[..(e.key.len() + 5).min(16)].to_vec()
    };
    match e.kind {
        Kind::Rc4_40 | Kind::Rc4_128 => rc4v(&objkey(false), data),
        Kind::AesV2 => cbc_seal(&objkey(true), iv, data),
        Kind::AesV3 => cbc_seal(&e.key, iv, data),
    }
}

/// PDF1's one-page file, encrypted: page contents (obj 4), /Info /Title (obj 5), XMP /Metadata (obj 7), a stream
/// behind an /Identity crypt filter (obj 8), the /Encrypt dictionary (obj 9).
fn build(e: &Enc, em: bool) -> Vec<u8> {
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R /Metadata 7 0 R >>")
        .obj(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << >> >>");
    b.stream(4, "", &seal(e, 4, CONTENT, true));
    b.obj(5, &format!("<< /Title {} >>", hexs(&seal(e, 5, TITLE.as_bytes(), false))));
    let xmp = if em { seal(e, 7, XMP, true) } else { XMP.to_vec() };
    b.stream(7, "/Type /Metadata /Subtype /XML", &xmp);
    if e.kind == Kind::AesV2 || e.kind == Kind::AesV3 {
        b.stream(8, "/Filter /Crypt /DecodeParms << /Name /Identity >>", b"in the clear");
    }
    b.obj(9, &e.dict);
    b.info = Some(5);
    b.trailer_extra = format!("/Encrypt 9 0 R /ID [{} {}]", hexs(ID0), hexs(ID0));
    b.version = if e.kind == Kind::AesV3 { "2.0" } else { "1.6" };
    b.classic()
}

fn dump(name: &str, pdf: &[u8]) {
    if let Ok(dir) = std::env::var("PDF_CORE_DUMP") {
        std::fs::write(format!("{dir}/{name}.pdf"), pdf).unwrap();
    }
}

fn reads_back(doc: &Document) {
    let pages = doc.pages().unwrap();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].contents, CONTENT);
    assert_eq!(doc.info().title.as_deref(), Some(TITLE));
    saved_reads_back(doc);
}

fn name_of(kind: Kind) -> &'static str {
    match kind {
        Kind::Rc4_40 => "rc4-40",
        Kind::Rc4_128 => "rc4-128",
        Kind::AesV2 => "aesv2",
        Kind::AesV3 => "aesv3",
    }
}

const KINDS: [Kind; 4] = [Kind::Rc4_40, Kind::Rc4_128, Kind::AesV2, Kind::AesV3];

#[test]
fn padding_string_is_the_specs() {
    assert_eq!(pad_password(b""), PAD);
    assert_eq!(&PAD[..8], b"\x28\xBF\x4E\x5E\x4E\x75\x8A\x41");
    assert_eq!(&PAD[24..], b"\x2F\x0C\xA9\xFE\x64\x53\x69\x7A");
    let p = pad_password(b"user");
    assert_eq!(&p[..4], b"user");
    assert_eq!(&p[4..], &PAD[..28]);
    assert_eq!(pad_password(&[7u8; 40]), [7u8; 32]);
}

#[test]
fn empty_user_password_opens_every_kind() {
    for kind in KINDS {
        let pdf = build(&security(kind, b"", b"owner", true, false), true);
        dump(&format!("{}-empty", name_of(kind)), &pdf);
        let doc = Document::parse(&pdf).unwrap_or_else(|e| panic!("{kind:?}: {e}"));
        reads_back(&doc);
        assert_eq!(doc.encryption(), name_of(kind));
        assert!(!doc.security().unwrap().owner());
        assert_eq!(doc.security().unwrap().permissions(), P);
        // The /Encrypt dictionary itself is never decrypted.
        let enc = doc.resolve(Ref { num: 9, generation: 0 }).unwrap();
        assert_eq!(enc.as_dict().unwrap().name(b"Filter"), Some(&b"Standard"[..]));
        // The owner password opens the same file as its owner; a wrong one is refused.
        let doc = Document::parse_with_password(&pdf, b"owner").unwrap_or_else(|e| panic!("{kind:?} owner: {e}"));
        reads_back(&doc);
        assert!(doc.security().unwrap().owner());
    }
}

#[test]
fn set_user_password_and_wrong_password() {
    for kind in KINDS {
        let pdf = build(&security(kind, b"user", b"owner", true, false), true);
        dump(&format!("{}-user", name_of(kind)), &pdf);
        assert_eq!(Document::parse(&pdf).err(), Some(Error::Password), "{kind:?}");
        assert_eq!(Document::parse_with_password(&pdf, b"nope").err(), Some(Error::Password), "{kind:?}");
        let doc = Document::parse_with_password(&pdf, b"user").unwrap_or_else(|e| panic!("{kind:?} user: {e}"));
        reads_back(&doc);
        assert!(!doc.security().unwrap().owner());
        let doc = Document::parse_with_password(&pdf, b"owner").unwrap_or_else(|e| panic!("{kind:?} owner: {e}"));
        reads_back(&doc);
        assert!(doc.security().unwrap().owner());
        assert_eq!(doc.encryption(), name_of(kind));
    }
}

#[test]
fn identity_crypt_filter_stream_is_read_in_the_clear() {
    for kind in [Kind::AesV2, Kind::AesV3] {
        let pdf = build(&security(kind, b"", b"owner", true, false), true);
        let doc = Document::parse(&pdf).unwrap();
        let Object::Stream(s) = doc.resolve(Ref { num: 8, generation: 0 }).unwrap() else { panic!() };
        assert_eq!(s.data, b"in the clear");
        assert_eq!(doc.decode(&s).unwrap().data, b"in the clear");
    }
}

#[test]
fn identity_stmf_leaves_streams_raw_and_strings_encrypted() {
    for kind in [Kind::AesV2, Kind::AesV3] {
        let pdf = build(&security(kind, b"", b"owner", true, true), true);
        dump(&format!("{}-identity-stmf", name_of(kind)), &pdf);
        let doc = Document::parse(&pdf).unwrap();
        reads_back(&doc);
        assert_eq!(doc.encryption(), name_of(kind));
    }
}

#[test]
fn encrypt_metadata_false_leaves_metadata_raw() {
    for kind in [Kind::AesV2, Kind::AesV3] {
        let pdf = build(&security(kind, b"", b"owner", false, false), false);
        dump(&format!("{}-meta-clear", name_of(kind)), &pdf);
        let doc = Document::parse(&pdf).unwrap();
        reads_back(&doc);
        let Object::Stream(s) = doc.resolve(Ref { num: 7, generation: 0 }).unwrap() else { panic!() };
        assert_eq!(s.data, XMP);
    }
}

#[test]
fn other_security_handlers_are_refused_by_name() {
    let e = Enc { kind: Kind::AesV2, key: vec![0; 16], dict: "<< /Filter /Adobe.PubSec /V 4 /R 4 /SubFilter /adbe.pkcs7.s5 >>".into(), stm_identity: false };
    assert_eq!(
        Document::parse(&build(&e, true)).err(),
        Some(Error::Unsupported("encrypt: public-key security handler"))
    );
    let e = Enc { dict: "<< /Filter /FooLock /V 2 /R 3 >>".into(), ..e };
    assert_eq!(
        Document::parse(&build(&e, true)).err(),
        Some(Error::Unsupported("encrypt: security handler other than /Standard"))
    );
}

/// PDFWRITE (SR325): the document saved (decrypted, /Encrypt dropped) re-parses with NO password to the same plaintext;
/// an incremental save of an encrypted file is refused (re-encryption owed).
fn saved_reads_back(doc: &Document) {
    let out = doc.save();
    let back = Document::parse(&out).unwrap();
    assert_eq!(back.encryption(), "none");
    assert!(!back.trailer().contains(b"Encrypt"));
    assert_eq!(back.pages().unwrap(), doc.pages().unwrap());
    assert_eq!(back.pages().unwrap()[0].contents, CONTENT);
    assert_eq!(back.info(), doc.info());
    assert!(matches!(pdf_core::write::Edit::new(doc).save_update(&out), Err(Error::Unsupported(_))));
}

// PDFREENCRYPT (LEDGER SR344): encrypting on save. `Document::save_encrypted` (R3 RC4-128, R4 AESV2, R6 AESV3) and
// `Edit::save_update_encrypted` read back through the same reader with the user and the owner password; the /O /U
// values are checked against the forward algorithms run here (no qpdf on the build box: the R6 /U /O /UE /OE /Perms
// are checked step by step below, not against an outside tool's fixture).

use pdf_core::crypt::Cipher;
use pdf_core::render::TextLog;
use pdf_core::write::Edit;

const SEED: [u8; 32] = [0x5c; 32];
const SEED2: [u8; 32] = [0xa3; 32];
const ART: &[u8] = b"1 0 0 rg 20 20 120 80 re f BT /F1 12 Tf 72 712 Td (Hello, encrypted world) Tj ET";
const CIPHERS: [(Cipher, &str); 3] = [(Cipher::Rc4_128, "rc4-128"), (Cipher::Aes128, "aesv2"), (Cipher::Aes256, "aesv3")];

/// A plaintext one-page file with ink on it (a red box) and an /Info /Title.
fn plain() -> Vec<u8> {
    let mut b = PdfBuilder::new(1);
    b.obj(1, "<< /Type /Catalog /Pages 2 0 R >>")
        .obj(2, "<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(3, "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R /Resources << >> >>");
    b.stream(4, "", ART);
    b.obj(5, &format!("<< /Title ({TITLE}) >>"));
    b.info = Some(5);
    b.classic()
}

fn pixels(doc: &Document) -> Vec<u8> {
    let pages = doc.pages().unwrap();
    pages[0].render(doc, 36.0, &mut TextLog::default()).unwrap().pixmap.data
}

fn contains(hay: &[u8], needle: &[u8]) -> bool {
    hay.windows(needle.len()).any(|w| w == needle)
}

fn same_doc(a: &Document, b: &Document, what: &str) {
    assert_eq!(a.pages().unwrap(), b.pages().unwrap(), "{what}: pages");
    assert_eq!(a.info(), b.info(), "{what}: info");
    assert!(pixels(a) == pixels(b), "{what}: pixels");
}

/// The saved /Encrypt dictionary (never decrypted) and /ID[0].
fn encrypt_of(doc: &Document) -> (pdf_core::Dict, Vec<u8>) {
    let enc = match doc.trailer().get(b"Encrypt").unwrap() {
        Object::Ref(r) => doc.resolve(*r).unwrap(),
        o => o.clone(),
    };
    let id0 = match doc.trailer().get(b"ID").unwrap() {
        Object::Array(a) => a[0].as_str().unwrap().to_vec(),
        _ => panic!("no /ID"),
    };
    (enc.as_dict().unwrap().clone(), id0)
}

fn s(d: &pdf_core::Dict, k: &[u8]) -> Vec<u8> {
    d.get(k).and_then(Object::as_str).unwrap_or_else(|| panic!("no /{}", String::from_utf8_lossy(k))).to_vec()
}

#[test]
fn save_encrypted_round_trips_with_user_and_owner_passwords() {
    let plain_pdf = plain();
    let enc_src = build(&security(Kind::AesV2, b"", b"owner", true, false), true);
    for (src_name, src) in [("plain", &plain_pdf), ("aesv2-source", &enc_src)] {
        let doc = Document::parse(src).unwrap();
        if src_name == "plain" {
            assert!(pixels(&doc).chunks(4).any(|p| p[0] > 200 && p[1] < 60), "the fixture has red ink");
        }
        for (cipher, kind) in CIPHERS {
            let what = format!("{src_name} -> {kind}");
            let out = doc.save_encrypted(b"u-pw", b"o-pw", P, cipher, &SEED);
            dump(&format!("reencrypt-{src_name}-{kind}"), &out);
            assert_eq!(Document::parse(&out).err(), Some(Error::Password), "{what}");
            assert_eq!(Document::parse_with_password(&out, b"wrong").err(), Some(Error::Password), "{what}");
            for (pw, owner) in [(&b"u-pw"[..], false), (&b"o-pw"[..], true)] {
                let back = Document::parse_with_password(&out, pw).unwrap_or_else(|e| panic!("{what}: {e}"));
                assert_eq!(back.encryption(), kind, "{what}");
                assert_eq!(back.security().unwrap().owner(), owner, "{what}");
                assert_eq!(back.security().unwrap().permissions(), P, "{what}");
                same_doc(&doc, &back, &what);
            }
            assert!(!contains(&out, b"Hello, encrypted"), "{what}: content in the clear");
            assert!(!contains(&out, TITLE.as_bytes()), "{what}: title in the clear");
            // An empty user password opens without one; the owner password still marks the owner.
            let open = doc.save_encrypted(b"", b"o-pw", P, cipher, &SEED);
            let back = Document::parse(&open).unwrap_or_else(|e| panic!("{what} empty: {e}"));
            assert!(!back.security().unwrap().owner());
            same_doc(&doc, &back, &what);
        }
    }
}

#[test]
fn r3_r4_o_and_u_match_the_forward_algorithms() {
    let doc = Document::parse(&plain()).unwrap();
    for (cipher, v, r) in [(Cipher::Rc4_128, 2, 3), (Cipher::Aes128, 4, 4)] {
        let out = doc.save_encrypted(b"u-pw", b"o-pw", P, cipher, &SEED);
        let back = Document::parse_with_password(&out, b"u-pw").unwrap();
        let (enc, id0) = encrypt_of(&back);
        assert_eq!((enc.int(b"V"), enc.int(b"R"), enc.int(b"Length")), (Some(v), Some(r), Some(128)));
        assert_eq!(enc.int(b"P"), Some(P as i64));
        let (o, u) = (s(&enc, b"O"), s(&enc, b"U"));
        // Algorithm 3: MD5 of the padded owner password, 50 more rounds, RC4 x 20 over the padded user password.
        let mut d = md5(&pad_password(b"o-pw"));
        for _ in 0..50 {
            d = md5(&d);
        }
        let mut want_o = pad_password(b"u-pw").to_vec();
        rc4_passes(&d[..16], &mut want_o, false);
        assert_eq!(o, want_o, "{cipher:?}: /O");
        // Algorithm 2 over the saved /O, /P and /ID[0], then Algorithm 5: /U's first 16 bytes.
        let mut h = Md5::new();
        h.update(&pad_password(b"u-pw"));
        h.update(&o);
        h.update(&(P as u32).to_le_bytes());
        h.update(&id0);
        let mut k = h.finalize();
        for _ in 0..50 {
            k = md5(&k[..16]);
        }
        let mut h = Md5::new();
        h.update(&PAD);
        h.update(&id0);
        let mut x = h.finalize().to_vec();
        rc4_passes(&k, &mut x, false);
        assert_eq!(u.len(), 32);
        assert_eq!(&u[..16], &x[..], "{cipher:?}: /U");
    }
}

#[test]
fn r6_u_o_ue_oe_perms_checked_step_by_step() {
    let doc = Document::parse(&plain()).unwrap();
    let out = doc.save_encrypted(b"u-pw", b"o-pw", P, Cipher::Aes256, &SEED);
    assert!(out.starts_with(b"%PDF-2.0"));
    let back = Document::parse_with_password(&out, b"u-pw").unwrap();
    let (enc, _) = encrypt_of(&back);
    assert_eq!((enc.int(b"V"), enc.int(b"R"), enc.int(b"Length")), (Some(5), Some(6), Some(256)));
    let (u, o, ue, oe, perms) = (s(&enc, b"U"), s(&enc, b"O"), s(&enc, b"UE"), s(&enc, b"OE"), s(&enc, b"Perms"));
    assert_eq!((u.len(), o.len(), ue.len(), oe.len(), perms.len()), (48, 48, 32, 32, 16));
    // Algorithm 8 / 9: the hashes over the validation salts (the owner's with the 48-byte /U as extra data).
    assert_eq!(&u[..32], &hash_r56(6, b"u-pw", &u[32..40], &[])[..]);
    assert_eq!(&o[..32], &hash_r56(6, b"o-pw", &o[32..40], &u)[..]);
    // The file key unwrapped from /UE (user key salt) and from /OE (owner key salt) is the same key.
    let unwrap = |ik: [u8; 32], wrapped: &[u8]| {
        let mut k = wrapped.to_vec();
        cbc::decrypt(&Aes::new(&ik).unwrap(), &[0; 16], &mut k).unwrap();
        k
    };
    let ku = unwrap(hash_r56(6, b"u-pw", &u[40..48], &[]), &ue);
    let ko = unwrap(hash_r56(6, b"o-pw", &o[40..48], &u), &oe);
    assert_eq!(ku, ko);
    // Algorithm 10 checked (ISO 32000-2 Algorithm 13): /Perms under the file key reads P, FF FF FF FF, 'T', "adb".
    let mut pb = perms.clone();
    cbc::decrypt(&Aes::new(&ku).unwrap(), &[0; 16], &mut pb).unwrap();
    assert_eq!(&pb[..4], &(P as u32).to_le_bytes());
    assert_eq!(&pb[4..8], &[0xff; 4]);
    assert_eq!(pb[8], b'T');
    assert_eq!(&pb[9..12], b"adb");
    // The owner password opens the same file as the owner.
    assert!(Document::parse_with_password(&out, b"o-pw").unwrap().security().unwrap().owner());
}

#[test]
fn save_update_of_an_encrypted_file_reads_back_under_the_same_password() {
    for kind in KINDS {
        let pdf = build(&security(kind, b"user", b"owner", true, false), true);
        for pw in [&b"user"[..], &b"owner"[..]] {
            let doc = Document::parse_with_password(&pdf, pw).unwrap();
            let mut e = Edit::new(&doc);
            e.set_info(b"Title", "Updated Title");
            assert!(matches!(e.save_update(&pdf), Err(Error::Unsupported(_))), "{kind:?}: no entropy, refused");
            let out = e.save_update_encrypted(&pdf, &SEED).unwrap();
            dump(&format!("update-{}-{}", name_of(kind), String::from_utf8_lossy(pw)), &out);
            assert!(out.starts_with(&pdf));
            assert!(!contains(&out[pdf.len()..], b"Updated Title"), "{kind:?}: appended title in the clear");
            assert_eq!(Document::parse(&out).err(), Some(Error::Password), "{kind:?}");
            let back = Document::parse_with_password(&out, pw).unwrap_or_else(|e| panic!("{kind:?}: {e}"));
            assert_eq!(back.encryption(), name_of(kind));
            assert_eq!(back.info().title.as_deref(), Some("Updated Title"), "{kind:?}");
            assert_eq!(back.pages().unwrap()[0].contents, CONTENT, "{kind:?}");
        }
    }
    // Unencrypted: the same bytes as save_update.
    let pdf = plain();
    let doc = Document::parse(&pdf).unwrap();
    let mut e = Edit::new(&doc);
    e.set_info(b"Title", "Updated Title");
    assert!(e.save_update(&pdf).unwrap() == e.save_update_encrypted(&pdf, &SEED).unwrap());
}

#[test]
fn save_encrypted_is_deterministic_per_entropy() {
    let doc = Document::parse(&plain()).unwrap();
    for (cipher, kind) in CIPHERS {
        let a = doc.save_encrypted(b"u-pw", b"o-pw", P, cipher, &SEED);
        let b = doc.save_encrypted(b"u-pw", b"o-pw", P, cipher, &SEED);
        let c = doc.save_encrypted(b"u-pw", b"o-pw", P, cipher, &SEED2);
        assert!(a == b, "{kind}: same entropy, same bytes");
        assert!(a != c, "{kind}: other entropy, other bytes");
        let id = |x: &[u8]| encrypt_of(&Document::parse_with_password(x, b"u-pw").unwrap()).1;
        assert_ne!(id(&a), id(&c), "{kind}: /ID[0]");
    }
}
