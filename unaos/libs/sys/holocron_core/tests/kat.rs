// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! HOLOCRON1 known-answer tests. The header KATs are written FIELD BY FIELD from the layout table in
//! `src/format.rs` (not from the encoder), so the encoder and the parser are each checked against the
//! specification text. The TEST-suite vectors (`testsuite_*`) are regression pins of a deliberately
//! insecure stand-in, not cryptographic vectors; the real AEAD/KDF/Ed25519 vectors are CRYPTOCORE's.

use holocron_core::format::{self, FormatError, Meta, RingHeader, SecretHeader};
use holocron_core::ring::{Ring, RingError};
use holocron_core::seal::{KdfParams, Sealer};
use holocron_core::testseal::{TestEntropy, TestSealer};

fn hex(s: &str) -> Vec<u8> {
    let s: String = s.split_whitespace().collect();
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn kat_secret_header() -> SecretHeader {
    SecretHeader {
        suite: format::SUITE_ARGON2ID_CHACHA20POLY1305,
        kdf: KdfParams::DEFAULT,
        salt: core::array::from_fn(|i| i as u8),
        nonce: core::array::from_fn(|i| 0xa0 + i as u8),
        meta: Meta { created: 1_790_000_000, kind: "api-key".into(), label: "Claude".into() },
        sealed_len: 16 + 51,
    }
}

/// KAT 1: the secret header, every field at its table offset.
const SECRET_HEADER_KAT: &str = "
    4843524e 01 01 4b00
    00000100 03000000 04000000
    000102030405060708090a0b0c0d0e0f
    a0a1a2a3a4a5a6a7a8a9aaab
    803bb16a00000000 43000000 07 06
    6170692d6b6579 436c61756465";

#[test]
fn kat1_secret_header_encode() {
    let want = hex(SECRET_HEADER_KAT);
    let got = kat_secret_header().encode();
    assert_eq!(got.len(), 75);
    assert_eq!(got, want);
}

#[test]
fn kat2_secret_header_parse() {
    let mut file = hex(SECRET_HEADER_KAT);
    file.extend_from_slice(&[0x5a; 67]);
    let (h, hb, sealed) = format::parse_secret(&file).unwrap();
    assert_eq!(h, kat_secret_header());
    assert_eq!(hb.len(), 75);
    assert_eq!(sealed, &[0x5a; 67][..]);
}

fn kat_ring_header() -> RingHeader {
    RingHeader {
        suite: format::SUITE_ARGON2ID_CHACHA20POLY1305,
        kdf: KdfParams::DEFAULT,
        salt: [0x11; 16],
        nonce: [0x22; 12],
        owner: "user:peter#1001".into(),
    }
}

/// KAT 3: the ring header.
const RING_HEADER_KAT: &str = "
    48435252 01 01 0000
    00000100 03000000 04000000
    11111111111111111111111111111111
    222222222222222222222222
    0f 757365723a7065746572233130 3031";

#[test]
fn kat3_ring_header_encode_parse() {
    let want = hex(RING_HEADER_KAT);
    assert_eq!(kat_ring_header().encode(), want);
    let mut file = want.clone();
    file.extend_from_slice(&[0x33; format::VERIFIER_SEALED]);
    let (h, hb, ver) = format::parse_ring(&file).unwrap();
    assert_eq!(h, kat_ring_header());
    assert_eq!(hb, &want[..]);
    assert_eq!(ver.len(), 48);
}

/// KAT 4: every refusal of the secret parser fires (fail-closed: nothing partial is adopted).
#[test]
fn kat4_secret_parser_refusals() {
    let mut good = hex(SECRET_HEADER_KAT);
    good.extend_from_slice(&[0x5a; 67]);
    assert!(format::parse_secret(&good).is_ok());
    let cases: Vec<(&str, Box<dyn Fn(&mut Vec<u8>)>, FormatError)> = vec![
        ("short", Box::new(|f| f.truncate(61)), FormatError::Length),
        ("magic", Box::new(|f| f[0] = b'X'), FormatError::Magic),
        ("version", Box::new(|f| f[4] = 2), FormatError::Version),
        ("header_len", Box::new(|f| f[6] = 0x4c), FormatError::Length),
        ("kind_len", Box::new(|f| f[60] = 8), FormatError::Length),
        ("trailing", Box::new(|f| f.push(0)), FormatError::Length),
        ("truncated body", Box::new(|f| { f.pop(); }), FormatError::Length),
        ("sealed_len", Box::new(|f| f[56] = 0x44), FormatError::Length),
        ("tag-only short", Box::new(|f| { f.truncate(75 + 15); f[56] = 15; }), FormatError::Length),
        ("utf8", Box::new(|f| f[62] = 0xff), FormatError::Text),
    ];
    for (what, mutate, want) in cases {
        let mut f = good.clone();
        mutate(&mut f);
        assert_eq!(format::parse_secret(&f).err(), Some(want), "{what}");
    }
}

#[test]
fn kat5_ring_parser_refusals() {
    let mut good = hex(RING_HEADER_KAT);
    good.extend_from_slice(&[0x33; 48]);
    assert!(format::parse_ring(&good).is_ok());
    let cases: Vec<(&str, Box<dyn Fn(&mut Vec<u8>)>, FormatError)> = vec![
        ("short", Box::new(|f| f.truncate(48)), FormatError::Length),
        ("magic", Box::new(|f| f[3] = b'N'), FormatError::Magic),
        ("version", Box::new(|f| f[4] = 0), FormatError::Version),
        ("reserved", Box::new(|f| f[7] = 1), FormatError::Reserved),
        ("owner empty", Box::new(|f| f[48] = 0), FormatError::Text),
        ("trailing", Box::new(|f| f.push(0)), FormatError::Length),
        ("verifier short", Box::new(|f| { f.pop(); }), FormatError::Length),
        ("owner utf8", Box::new(|f| f[49] = 0xc3), FormatError::Text),
    ];
    for (what, mutate, want) in cases {
        let mut f = good.clone();
        mutate(&mut f);
        assert_eq!(format::parse_ring(&f).err(), Some(want), "{what}");
    }
}

// ---- the ring over the TEST suite -------------------------------------------------------------------

fn new_ring() -> (Ring<TestSealer>, Vec<u8>, TestEntropy) {
    let mut rng = TestEntropy::default();
    let mut r = Ring::new(TestSealer);
    let file = r.create("user:peter#1001", b"correct horse", KdfParams::DEFAULT, &mut rng).unwrap();
    (r, file, rng)
}

#[test]
fn ring_create_unlock_lock() {
    let (mut r, file, _) = new_ring();
    assert!(r.is_unlocked());
    assert_eq!(file.len(), 49 + 15 + 48);
    assert_eq!(file[5], format::SUITE_TEST_INSECURE);
    r.lock();
    assert!(!r.is_unlocked());
    assert_eq!(r.unlock(&file, b"wrong"), Err(RingError::BadPassword));
    assert!(!r.is_unlocked());
    r.unlock(&file, b"correct horse").unwrap();
    assert!(r.is_unlocked());
    assert_eq!(r.owner().as_deref(), Some("user:peter#1001"));
}

#[test]
fn ring_refuses_tampered_ring_file() {
    let (mut r, file, _) = new_ring();
    // Any header byte: the verifier's AAD is the header, so the right password no longer opens it.
    for i in [5usize, 8, 20, 36, 49, 60] {
        let mut f = file.clone();
        f[i] ^= 1;
        let e = r.unlock(&f, b"correct horse").unwrap_err();
        assert!(matches!(e, RingError::BadPassword | RingError::Suite | RingError::Params), "byte {i}: {e:?}");
    }
    // KDF parameters below the floor are refused before the KDF runs.
    let mut f = file.clone();
    f[8..12].copy_from_slice(&1024u32.to_le_bytes());
    assert_eq!(r.unlock(&f, b"correct horse"), Err(RingError::Params));
    // The production suite byte on a TEST ring is refused by the TEST sealer (and vice versa).
    let mut f = file.clone();
    f[5] = format::SUITE_ARGON2ID_CHACHA20POLY1305;
    assert_eq!(r.unlock(&f, b"correct horse"), Err(RingError::Suite));
}

#[test]
fn secret_round_trip_and_binding() {
    let (r, _, mut rng) = new_ring();
    let meta = Meta { created: 1_790_000_000, kind: "api-key".into(), label: "Claude".into() };
    let file = r.seal_secret("vein", "claude.api_key", &meta, b"sk-ant-TEST", &mut rng).unwrap();
    let (m, pt) = r.open_secret("vein", "claude.api_key", &file).unwrap();
    assert_eq!(m, meta);
    assert_eq!(pt.expose(), b"sk-ant-TEST");
    // Bound to its name and namespace (AAD).
    assert_eq!(r.open_secret("vein", "other", &file).err(), Some(RingError::Auth));
    assert_eq!(r.open_secret("aether", "claude.api_key", &file).err(), Some(RingError::Auth));
    // Every byte of the file is authenticated: header (metadata included) and body.
    for i in 0..file.len() {
        let mut f = file.clone();
        f[i] ^= 0x01;
        assert!(r.open_secret("vein", "claude.api_key", &f).is_err(), "flip at {i} accepted");
    }
    // Two seals of the same plaintext differ (fresh salt + nonce).
    let file2 = r.seal_secret("vein", "claude.api_key", &meta, b"sk-ant-TEST", &mut rng).unwrap();
    assert_ne!(file, file2);
}

#[test]
fn locked_ring_opens_nothing() {
    let (mut r, _, mut rng) = new_ring();
    let file = r.seal_secret("vein", "k", &Meta::default(), b"x", &mut rng).unwrap();
    r.lock();
    assert_eq!(r.open_secret("vein", "k", &file).err(), Some(RingError::Locked));
    assert_eq!(r.seal_secret("vein", "k", &Meta::default(), b"x", &mut rng).err(), Some(RingError::Locked));
}

#[test]
fn another_ring_cannot_open() {
    let (r1, _, mut rng) = new_ring();
    let mut rng2 = TestEntropy { counter: 1000 };
    let mut r2 = Ring::new(TestSealer);
    r2.create("user:peter#1001", b"correct horse", KdfParams::DEFAULT, &mut rng2).unwrap();
    let file = r1.seal_secret("vein", "k", &Meta::default(), b"x", &mut rng).unwrap();
    // Same password, different ring salt => different ring key.
    assert_eq!(r2.open_secret("vein", "k", &file).err(), Some(RingError::Auth));
}

#[test]
fn limits() {
    let (r, _, mut rng) = new_ring();
    let max = vec![7u8; format::SECRET_MAX];
    assert!(r.seal_secret("a", "b", &Meta::default(), &max, &mut rng).is_ok());
    let over = vec![7u8; format::SECRET_MAX + 1];
    assert_eq!(r.seal_secret("a", "b", &Meta::default(), &over, &mut rng).err(), Some(RingError::Invalid));
    assert_eq!(r.seal_secret("../x", "b", &Meta::default(), b"", &mut rng).err(), Some(RingError::Invalid));
    let long = Meta { label: "x".repeat(256), ..Meta::default() };
    assert_eq!(r.seal_secret("a", "b", &long, b"", &mut rng).err(), Some(RingError::Invalid));
}

/// TEST-suite regression pins (NOT cryptographic vectors): a change here changes every TEST file.
#[test]
fn testsuite_pins() {
    let s = TestSealer;
    let k = s.derive_key(b"password", &[0u8; 16], &KdfParams::DEFAULT).unwrap();
    let ct = s.seal(&k, &[0u8; 12], b"aad", b"plaintext");
    assert_eq!(ct.len(), 9 + 16);
    let pinned = std::env::var("HOLOCRON_PRINT_PINS").is_ok();
    if pinned {
        println!("key={:02x?}\nct={:02x?}", k.bytes(), ct);
    }
    assert_eq!(s.open(&k, &[0u8; 12], b"aad", &ct).unwrap(), b"plaintext");
    assert!(s.open(&k, &[0u8; 12], b"aaD", &ct).is_err());
    assert_eq!(&ct[..], &TESTSUITE_CT_PIN[..]);
}

const TESTSUITE_CT_PIN: [u8; 25] = [
    0x73, 0x30, 0x3c, 0x53, 0xf3, 0x2b, 0x10, 0x30, 0x64, 0xdb, 0x7c, 0x27, 0x45, 0xdd, 0x0f, 0x15, 0xfe, 0xa7, 0xcc,
    0x09, 0xf9, 0xac, 0x61, 0x37, 0x65,
];
