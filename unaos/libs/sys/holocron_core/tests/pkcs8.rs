// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PDF-KEYSTORE (LEDGER SR382) KATs: a signing key file put under ns `sign` (kind `pkcs8`) is parsed by tls_core's key
//! parser before it is sealed, and Sign's long form signs with it inside the service; every signature is checked by
//! tls_core's own `verify_signed` (the product provider). The key files are written by tls_core's key-container writers
//! (`pkcs8_build`, `rsa_private_build`, `ec_private_build`) from crypto_core keys: RSA-2048 generated deterministically,
//! P-256 and Ed25519 from fixed scalars. KEYSIGNONE (LEDGER SR393): the signatures come from tls_core's ONE signer and
//! each refusal is named on the wire (`WRONG_KIND`, `BAD_DIGEST_LEN`, `UNSUPPORTED_SCHEME`).

use crypto_core::ed25519::SigningKey;
use crypto_core::p256::SecretKey;
use crypto_core::rsa_private::{Part, PrivateKey};
use crypto_core::{Digest, Sha256, Sha384};
use holocron_core::seal::KdfParams;
use holocron_core::service::{Holocron, KIND_PKCS8, MemStore, SIGN_NS};
use holocron_core::testseal::{TestEntropy, TestSealer, TestSigner};
use holocron_core::wire::{self, Reply, Request, key_kind, sign_alg, status};
use tls_core::cryptocore_provider::CryptoCoreProvider;
use tls_core::crypto::{EcCurve, HashAlg};
use tls_core::key::{AlgParams, ec_private_build, pkcs8_build, rsa_private_build};
use tls_core::x509::{PublicKey, SignatureAlgorithm, oid, verify_signed};

const OWNER: &str = "user:peter#1001";
type H = Holocron<TestSealer, TestSigner, MemStore, TestEntropy>;

const MSG: &[u8] = b"PDF-KEYSTORE KAT: the CMS signed attributes stand-in";

fn svc() -> H {
    Holocron::new(TestSealer, TestSigner, MemStore::default(), TestEntropy::default(), OWNER.into(), KdfParams::DEFAULT)
}
fn call(h: &mut H, r: Request) -> Reply {
    h.handle(Some(OWNER), r.verb(), &r.encode_body(), 0, 1_790_000_000)
}
fn unlocked() -> H {
    let mut h = svc();
    assert_eq!(call(&mut h, Request::Unlock { create: true, password: b"pw".to_vec() }).status, status::OK);
    h
}
fn put(ns: &str, name: &str, kind: &str, data: &[u8]) -> Request {
    Request::Put { ns: ns.into(), name: name.into(), kind: kind.into(), label: "KAT".into(), data: data.to_vec() }
}
fn sign_with(name: &str, alg: u8, data: &[u8]) -> Request {
    Request::SignWith { ns: SIGN_NS.into(), key: name.into(), alg, data: data.to_vec() }
}
fn digest<D: Digest>(m: &[u8]) -> Vec<u8> {
    let mut h = D::new();
    h.update(m);
    let mut out = vec![0u8; D::OUTPUT_LEN];
    h.finalize_into(&mut out);
    out
}

fn rsa_key() -> PrivateKey {
    let mut ctr = 0u64;
    let mut rng = move |out: &mut [u8]| {
        for chunk in out.chunks_mut(32) {
            let d = digest::<Sha256>(&ctr.to_be_bytes());
            ctr += 1;
            chunk.copy_from_slice(&d[..chunk.len()]);
        }
    };
    PrivateKey::generate(2048, &[1, 0, 1], &mut rng).unwrap()
}
fn part(k: &PrivateKey, p: Part) -> Vec<u8> {
    let mut b = vec![0u8; k.public_key().size()];
    let n = k.part_be(p, &mut b);
    b.truncate(n);
    b
}
fn rsa_file(k: &PrivateKey) -> Vec<u8> {
    let parts: Vec<Vec<u8>> = [Part::N, Part::E, Part::D, Part::P, Part::Q, Part::Dp, Part::Dq, Part::Qi].into_iter().map(|p| part(k, p)).collect();
    let refs: Vec<&[u8]> = parts.iter().map(Vec::as_slice).collect();
    pkcs8_build(oid::RSA_ENCRYPTION, &AlgParams::Null, &rsa_private_build(&refs))
}
fn p256_file(k: &SecretKey, d: &[u8; 32]) -> Vec<u8> {
    pkcs8_build(oid::EC_PUBLIC_KEY, &AlgParams::Oid(oid::PRIME256V1), &ec_private_build(d, &k.public_key().to_sec1_uncompressed()))
}
fn ed_file(seed: &[u8; 32]) -> Vec<u8> {
    let mut inner = vec![0x04, 32];
    inner.extend_from_slice(seed);
    pkcs8_build(oid::ED25519, &AlgParams::Absent, &inner)
}

/// Sign through the service and return (kind, signature); asserts OK.
fn signed(h: &mut H, name: &str, alg: u8, data: &[u8]) -> (u8, Vec<u8>) {
    let r = call(h, sign_with(name, alg, data));
    assert_eq!(r.status, status::OK, "{name} alg {alg:#x}");
    let (k, s) = wire::decode_sign(&r.body).unwrap();
    (k, s.to_vec())
}
fn verifies(key: &PublicKey, alg: SignatureAlgorithm, sig: &[u8]) -> bool {
    verify_signed(&CryptoCoreProvider::new(), key, alg, MSG, sig).is_ok()
}

#[test]
fn put_and_sign_round_trip_three_key_kinds() {
    let mut h = unlocked();
    let rsa = rsa_key();
    let d = [0x11u8; 32];
    let ec = SecretKey::from_bytes(&d).unwrap();
    let seed = [0x22u8; 32];
    let ed = SigningKey::from_seed(&seed);
    for (name, file) in [("rsa", rsa_file(&rsa)), ("ec", p256_file(&ec, &d)), ("ed", ed_file(&seed))] {
        assert_eq!(call(&mut h, put(SIGN_NS, name, KIND_PKCS8, &file)).status, status::OK, "{name}");
    }
    let rsa_pub = PublicKey::Rsa { n: part(&rsa, Part::N), e: part(&rsa, Part::E) };
    let ec_pub = PublicKey::Ec { curve: EcCurve::P256, point: ec.public_key().to_sec1_uncompressed().to_vec() };
    let ed_pub = PublicKey::Ed25519(ed.public_key());

    let (k, s) = signed(&mut h, "rsa", sign_alg::SHA256, &digest::<Sha256>(MSG));
    assert_eq!((k, s.len()), (key_kind::RSA, 256));
    assert!(verifies(&rsa_pub, SignatureAlgorithm::RsaPkcs1(HashAlg::Sha256), &s));
    // PKCS#1 v1.5 is deterministic: the local crypto_core signature, byte for byte.
    let mut local = vec![0u8; 256];
    rsa.sign_pkcs1v15(&digest::<Sha256>(MSG), &mut local).unwrap();
    assert_eq!(s, local);
    let (_, s) = signed(&mut h, "rsa", sign_alg::SHA384, &digest::<Sha384>(MSG));
    assert!(verifies(&rsa_pub, SignatureAlgorithm::RsaPkcs1(HashAlg::Sha384), &s));
    let (_, s) = signed(&mut h, "rsa", sign_alg::SHA256 | sign_alg::PSS, &digest::<Sha256>(MSG));
    assert!(verifies(&rsa_pub, SignatureAlgorithm::RsaPss { hash: HashAlg::Sha256, salt_len: 32 }, &s));
    assert!(!verifies(&rsa_pub, SignatureAlgorithm::RsaPkcs1(HashAlg::Sha256), &s));

    let (k, s) = signed(&mut h, "ec", sign_alg::SHA256, &digest::<Sha256>(MSG));
    assert_eq!(k, key_kind::P256);
    assert!(verifies(&ec_pub, SignatureAlgorithm::Ecdsa(HashAlg::Sha256), &s));
    let (_, s) = signed(&mut h, "ec", sign_alg::SHA384, &digest::<Sha384>(MSG));
    assert!(verifies(&ec_pub, SignatureAlgorithm::Ecdsa(HashAlg::Sha384), &s));

    let (k, s) = signed(&mut h, "ed", sign_alg::NONE, MSG);
    assert_eq!((k, s.len()), (key_kind::ED25519, 64));
    assert!(verifies(&ed_pub, SignatureAlgorithm::Ed25519, &s));
    assert_eq!(s, ed.sign(MSG).to_vec());
    // The wrong key's verifier refuses.
    assert!(!verifies(&ed_pub, SignatureAlgorithm::Ed25519, &signed(&mut h, "rsa", sign_alg::SHA256, &digest::<Sha256>(MSG)).1));
}

#[test]
fn refusals() {
    let mut h = unlocked();
    let d = [0x11u8; 32];
    let ec = SecretKey::from_bytes(&d).unwrap();
    let ec_file = p256_file(&ec, &d);
    // Nothing that does not parse is stored.
    assert_eq!(call(&mut h, put(SIGN_NS, "junk", KIND_PKCS8, b"not a key")).status, status::INVALID);
    let p384 = pkcs8_build(oid::EC_PUBLIC_KEY, &AlgParams::Oid(oid::SECP384R1), &ec_private_build(&[0x11; 48], &[4; 97]));
    assert_eq!(call(&mut h, put(SIGN_NS, "p384", KIND_PKCS8, &p384)).status, status::INVALID);
    assert_eq!(holocron_core::pkcs8::check(&p384).unwrap_err().why(), "P-384");
    let listed = call(&mut h, Request::List { ns: SIGN_NS.into() });
    assert_eq!(wire::decode_list(&listed.body).unwrap().len(), 0);
    // The kind and the namespace go together.
    assert_eq!(call(&mut h, put("vein", "k", KIND_PKCS8, &ec_file)).status, status::INVALID);
    assert_eq!(call(&mut h, put(SIGN_NS, "k", "api-key", &ec_file)).status, status::INVALID);
    assert_eq!(call(&mut h, put(SIGN_NS, "ec", KIND_PKCS8, &ec_file)).status, status::OK);
    assert_eq!(holocron_core::pkcs8::check(&ec_file), Ok("p256"));
    // Algorithm / digest mismatches, each named on the wire (KEYSIGNONE SR393).
    let dg = digest::<Sha256>(MSG);
    assert_eq!(call(&mut h, sign_with("ec", sign_alg::SHA384, &dg)).status, status::BAD_DIGEST_LEN);
    assert_eq!(call(&mut h, sign_with("ec", sign_alg::SHA256, &dg[..31])).status, status::BAD_DIGEST_LEN);
    assert_eq!(call(&mut h, sign_with("ec", sign_alg::SHA256 | sign_alg::PSS, &dg)).status, status::WRONG_KIND);
    assert_eq!(call(&mut h, sign_with("ec", sign_alg::NONE, MSG)).status, status::WRONG_KIND);
    assert_eq!(call(&mut h, sign_with("ec", 0x41, &dg)).status, status::UNSUPPORTED_SCHEME);
    assert_eq!(call(&mut h, sign_with("ec", 0x04, &dg)).status, status::UNSUPPORTED_SCHEME);
    assert_eq!(call(&mut h, sign_with("ec", sign_alg::PSS, &dg)).status, status::UNSUPPORTED_SCHEME);
    assert_eq!(call(&mut h, put(SIGN_NS, "ed", KIND_PKCS8, &ed_file(&[0x22; 32]))).status, status::OK);
    assert_eq!(call(&mut h, sign_with("ed", sign_alg::SHA256, &dg)).status, status::WRONG_KIND);
    // Missing, other namespaces, locked.
    assert_eq!(call(&mut h, sign_with("nope", sign_alg::SHA256, &dg)).status, status::NOT_FOUND);
    let other = Request::SignWith { ns: "vein".into(), key: "ec".into(), alg: sign_alg::SHA256, data: dg.clone() };
    assert_eq!(call(&mut h, other).status, status::INVALID);
    assert_eq!(call(&mut h, Request::Lock).status, status::OK);
    assert_eq!(call(&mut h, sign_with("ec", sign_alg::SHA256, &dg)).status, status::LOCKED);
}

#[test]
fn long_form_wire() {
    let r = Request::SignWith { ns: "sign".into(), key: "k".into(), alg: 0x12, data: vec![9, 9] };
    let body = r.encode_body();
    assert_eq!(body, [1, b'k', 2, 0, 9, 9, 4, b's', b'i', b'g', b'n', 0x12]);
    assert_eq!(r.verb(), wire::VERB_SIGN);
    assert_eq!(Request::decode(wire::VERB_SIGN, &body), Ok(r));
    // The short form decodes as before.
    assert_eq!(Request::decode(wire::VERB_SIGN, &[1, b'k', 2, 0, 9, 9]), Ok(Request::Sign { key: "k".into(), data: vec![9, 9] }));
    // A truncated long form is EINVAL.
    assert_eq!(Request::decode(wire::VERB_SIGN, &body[..11]), Err(status::INVALID));
    assert_eq!(wire::decode_sign(&wire::encode_sign(key_kind::P256, &[1, 2, 3])), Some((key_kind::P256, &[1u8, 2, 3][..])));
    assert_eq!(wire::decode_sign(&[key_kind::RSA]), None);
}

#[test]
fn ssh_key_through_the_long_form() {
    let mut h = unlocked();
    let mint = Request::Put { ns: "ssh".into(), name: "id".into(), kind: "ssh-ed25519".into(), label: "me".into(), data: vec![] };
    assert_eq!(call(&mut h, mint).status, status::OK);
    let short = call(&mut h, Request::Sign { key: "id".into(), data: MSG.to_vec() });
    assert_eq!(short.status, status::OK);
    let long = call(&mut h, Request::SignWith { ns: "ssh".into(), key: "id".into(), alg: sign_alg::NONE, data: MSG.to_vec() });
    assert_eq!(wire::decode_sign(&long.body), Some((key_kind::ED25519, &short.body[..])));
    let hashed = Request::SignWith { ns: "ssh".into(), key: "id".into(), alg: sign_alg::SHA256, data: MSG.to_vec() };
    assert_eq!(call(&mut h, hashed).status, status::WRONG_KIND);
}

/// KEYSIGNONE (SR393): the refusal reasons are distinct codes with their own words; INVALID stays the catch-all.
#[test]
fn reason_words() {
    let named = [
        (status::WRONG_KIND, "wrong-kind"),
        (status::BAD_DIGEST_LEN, "bad-digest-len"),
        (status::UNSUPPORTED_SCHEME, "unsupported-scheme"),
        (status::INVALID, "invalid"),
        (status::NOT_FOUND, "not-found"),
        (status::OK, "ok"),
    ];
    for (code, word) in named {
        assert_eq!(status::word(code), word, "{code}");
    }
    let codes = [status::WRONG_KIND, status::BAD_DIGEST_LEN, status::UNSUPPORTED_SCHEME, status::INVALID, status::CORRUPT, status::IO];
    for (i, a) in codes.iter().enumerate() {
        assert!(codes[i + 1..].iter().all(|b| b != a), "{a} is one reason");
    }
    assert_eq!(status::word(-1000), "error");
}

/// STATUS-WORDS-ONE (SR399): `status::word` is the union of every table the faces kept (user-holocron's
/// `status_name`, the kernel keyring's `status_word`, the holocron CLI's `status_name`): every code they named has its
/// own word (none falls to `error`), and the prefixes the kernel and CLI hints hang off are `word`'s.
#[test]
fn reason_words_union() {
    let all = [
        (status::OK, "ok"),
        (status::BAD_PASSWORD, "bad-password"),
        (status::NOT_FOUND, "not-found"),
        (status::IO, "io"),
        (status::RATE_LIMITED, "rate-limited"),
        (status::DENIED, "denied"),
        (status::EXISTS, "exists"),
        (status::NO_RING, "no-ring"),
        (status::INVALID, "invalid"),
        (status::TOO_BIG, "too-big"),
        (status::LOCKED, "locked"),
        (status::CORRUPT, "corrupt"),
        (status::NO_VERB, "no-verb"),
        (status::WRONG_KIND, "wrong-kind"),
        (status::BAD_DIGEST_LEN, "bad-digest-len"),
        (status::UNSUPPORTED_SCHEME, "unsupported-scheme"),
    ];
    for (i, (code, w)) in all.iter().enumerate() {
        assert_eq!(status::word(*code), *w, "{code}");
        assert_ne!(*w, "error", "{code} is named");
        assert!(all[i + 1..].iter().all(|(c, x)| c != code && x != w), "{code} is one code with one word");
    }
    // The kernel's login hint and the CLI's hints are `<word>: <hint>`; their heads are the table's words.
    let kernel_hint = "bad-password: the ring is under another password";
    assert_eq!(kernel_hint.split(':').next(), Some(status::word(status::BAD_PASSWORD)));
    for code in [status::BAD_PASSWORD, status::RATE_LIMITED, status::DENIED, status::NO_RING, status::LOCKED] {
        assert_ne!(status::word(code), "error");
    }
}
