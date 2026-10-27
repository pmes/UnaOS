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
