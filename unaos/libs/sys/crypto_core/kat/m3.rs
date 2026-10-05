// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// M3 — curves: X25519 (RFC 7748 §5.2, the 1 / 1000 iteration chain — 1,000,000 with CRYPTO_SLOW=1 — §6.1,
// Wycheproof), Ed25519 (RFC 8032 §7.1 TEST 1-3, the 1024-vector sign.input from the Ed25519 authors,
// Wycheproof), P-256 (RFC 6979 A.2.5, NIST CAVP KeyPair / SigGen / SigVer for every SHA-2 hash on P-256,
// CAVP KAS ECC "ZZOnly" ECDH validity, Wycheproof ECDH (SEC 1 points and SPKI) and ECDSA (DER and P1363)).
// Unsupported (counted): CAVP sections using SHA-1 (this crate carries no SHA-1).

use super::*;
use crypto_core::ed25519::{self, SigningKey};
use crypto_core::p256::{self, PublicKey, SecretKey};
use crypto_core::sha2::{sha224, sha256, sha384, sha512};
use crypto_core::x25519::{diffie_hellman, public_key, x25519};

pub const SETS: &[(&str, fn() -> Tally)] = &[
    ("x25519/rfc7748", x25519_rfc),
    ("x25519/wycheproof", x25519_wy),
    ("ed25519/rfc8032", ed_rfc),
    ("ed25519/sign.input", ed_sign_input),
    ("ed25519/wycheproof", ed_wy),
    ("p256/rfc6979", p256_rfc6979),
    ("p256/cavp-keypair", p256_keypair),
    ("p256/cavp-siggen", p256_siggen),
    ("p256/cavp-sigver", p256_sigver),
    ("p256/cavp-kas-ecdh", p256_kas),
    ("p256/wycheproof-ecdh", p256_wy_ecdh),
    ("p256/wycheproof-ecdsa", p256_wy_ecdsa),
];

fn a32(b: Vec<u8>) -> [u8; 32] {
    b.try_into().unwrap()
}

fn x25519_rfc() -> Tally {
    let mut t = Tally::new("");
    for r in parse_rsp(include_str!("data/x25519-rfc7748.txt")) {
        let got = x25519(&a32(r.bytes("INPUT_SCALAR")), &a32(r.bytes("INPUT_U")));
        t.check(got.to_vec() == r.bytes("OUTPUT_U"), || format!("§5.2 count {}", r.get("COUNT").unwrap()));
    }
    // §5.2 iteration: k = u = 9; k, u = X25519(k, u), k.
    let mut k = crypto_core::x25519::BASEPOINT;
    let mut u = k;
    let slow = std::env::var_os("CRYPTO_SLOW").is_some();
    let stop = if slow { 1_000_000 } else { 1000 };
    for i in 1..=stop {
        let nk = x25519(&k, &u);
        u = k;
        k = nk;
        let want = match i {
            1 => Some("422c8e7a6227d7bca1350b3e2bb7279f7897b87bb6854b783c60e80311ae3079"),
            1000 => Some("684cf59ba83309552800ef566f2f4d3c1c3887c49360e3875f2eb94d99532c51"),
            1_000_000 => Some("7c3911e0ab2586fd864497297e575e6f3bc601c0883c30df5f4dd2d24f665424"),
            _ => None,
        };
        if let Some(w) = want {
            t.check(hex(&k) == w, || format!("iteration {i}"));
        }
    }
    // §6.1 Diffie-Hellman.
    let a = a32(unhex("77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a"));
    let b = a32(unhex("5dab087e624a8a4b79e17f8b83800ee66f3bb1292618b6fd1c2f8b27ff88e0eb"));
    let ap = public_key(&a);
    let bp = public_key(&b);
    t.check(hex(&ap) == "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a", || "§6.1 alice pub".into());
    t.check(hex(&bp) == "de9edb7d7b7dc1b4d35b61c2ece435373f8343c85b78674dadfc7e146f882b4f", || "§6.1 bob pub".into());
    let k1 = diffie_hellman(&a, &bp).unwrap();
    let k2 = diffie_hellman(&b, &ap).unwrap();
    t.check(k1 == k2 && hex(&k1) == "4a5d9d5ba4ce2de1728e3bf480350f25e07e21c947d19e3376f09b3c1e161742", || "§6.1 shared".into());
    t
}

fn x25519_wy() -> Tally {
    let doc = match fetch_text("", "wycheproof-x25519_test.json") {
        Ok(s) => parse_json(&s),
        Err(sk) => return sk,
    };
    let mut t = Tally::new("");
    for (_g, c) in wycheproof_tests(&doc) {
        let (k, u, want) = (a32(c.get("private").bytes()), a32(c.get("public").bytes()), c.get("shared").bytes());
        // The RFC 7748 function is total: it must equal `shared` for EVERY vector, acceptable ones too;
        // the checked DH must refuse exactly the all-zero results.
        let raw = x25519(&k, &u);
        let dh = diffie_hellman(&k, &u);
        let zero = want.iter().all(|&b| b == 0);
        t.check(raw.to_vec() == want && dh.is_err() == zero, || format!("tc{} {}", c.get("tcId").n(), c.get("comment").s()));
    }
    t
}

fn ed_case(t: &mut Tally, what: &str, seed: &[u8; 32], pk: &[u8], msg: &[u8], sig: &[u8]) {
    let sk = SigningKey::from_seed(seed);
    let s = sk.sign(msg);
    let ok = sk.public_key().to_vec() == pk && s.to_vec() == sig && ed25519::verify(&a32(pk.to_vec()), msg, &s).is_ok();
    // and a one-bit forgery is refused
    let mut bad = s;
    bad[0] ^= 1;
    let refused = ed25519::verify(&a32(pk.to_vec()), msg, &bad).is_err();
    t.check(ok && refused, || what.to_string());
}

fn ed_rfc() -> Tally {
    let mut t = Tally::new("");
    let v = [
        ("9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60", "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a", "", "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b"),
        ("4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb", "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c", "72", "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00"),
        ("c5aa8df43f9f837bedb7442f31dcb7b166d38535076f094b85ce3a2e0b4458f7", "fc51cd8e6218a1a38da47ed00230f0580816ed13ba3303ac5deb911548908025", "af82", "6291d657deec24024827e69c3abe01a30ce548a284743a445e3680d7db5ac3ac18ff9b538d16f290ae67f760984dc6594a7c15e9716ed28dc027beceea1ec40a"),
    ];
    for (i, (sk, pk, m, s)) in v.iter().enumerate() {
        ed_case(&mut t, &format!("RFC 8032 TEST {}", i + 1), &a32(unhex(sk)), &unhex(pk), &unhex(m), &unhex(s));
    }
    t
}

fn ed_sign_input() -> Tally {
    let text = match fetch_text("", "sign.input") {
        Ok(s) => s,
        Err(sk) => return sk,
    };
    let mut t = Tally::new("");
    for (n, line) in text.lines().enumerate() {
        let f: Vec<&str> = line.split(':').collect();
        if f.len() < 4 {
            continue;
        }
        let skpk = unhex(f[0]);
        let sm = unhex(f[3]);
        ed_case(&mut t, &format!("sign.input line {}", n + 1), &a32(skpk[..32].to_vec()), &unhex(f[1]), &unhex(f[2]), &sm[..64]);
    }
    t
}

fn ed_wy() -> Tally {
    let doc = match fetch_text("", "wycheproof-ed25519_test.json") {
        Ok(s) => parse_json(&s),
        Err(sk) => return sk,
    };
    let mut t = Tally::new("");
    for (g, c) in wycheproof_tests(&doc) {
        let pk = a32(g.get("publicKey").get("pk").bytes());
        let sig = c.get("sig").bytes();
        let ok = match <[u8; 64]>::try_from(sig) {
            Ok(s) => ed25519::verify(&pk, &c.get("msg").bytes(), &s).is_ok(),
            Err(_) => false,
        };
        match wy_expect(c) {
            Some(v) => t.check(ok == v, || format!("tc{} {}", c.get("tcId").n(), c.get("comment").s())),
            None => t.pass += 1,
        }
    }
    t
}

fn hash_by_name(name: &str, msg: &[u8]) -> Option<Vec<u8>> {
    Some(match name {
        "SHA-224" => sha224(msg).to_vec(),
        "SHA-256" => sha256(msg).to_vec(),
        "SHA-384" => sha384(msg).to_vec(),
        "SHA-512" => sha512(msg).to_vec(),
        _ => return None,
    })
}

fn pubkey_xy(x: &[u8], y: &[u8]) -> Result<PublicKey, crypto_core::Error> {
    if x.len() > 32 || y.len() > 32 {
        return Err(crypto_core::Error::Encoding);
    }
    let mut b = [0u8; 65];
    b[0] = 4;
    b[33 - x.len()..33].copy_from_slice(x);
    b[65 - y.len()..].copy_from_slice(y);
    PublicKey::from_sec1(&b)
}

fn rs(r: &[u8], s: &[u8]) -> [u8; 64] {
    let mut o = [0u8; 64];
    o[32 - r.len()..32].copy_from_slice(r);
    o[64 - s.len()..].copy_from_slice(s);
    o
}

fn p256_rfc6979() -> Tally {
    let mut t = Tally::new("");
    let sk = SecretKey::from_bytes(&a32(unhex("c9afa9d845ba75166b5c215767b1d6934e50c3db36e89b127b8a622b120f6721"))).unwrap();
    let pk = sk.public_key();
    t.check(
        hex(&pk.to_sec1_uncompressed()) == "0460fed4ba255a9d31c961eb74c6356d68c049b8923b61fa6ce669622e60f29fb67903fe1008b8bc99a41ae9e95628bc64f2f1b20c2d7e9f5177a3c294d4462299",
        || "A.2.5 public key".into(),
    );
    for (m, r, s) in [
        ("sample", "efd48b2aacb6a8fd1140dd9cd45e81d69d2c877b56aaf991c34d0ea84eaf3716", "f7cb1c942d657c41d436c7a1b6e29f65f3e900dbb9aff4064dc4ab2f843acda8"),
        ("test", "f1abb023518351cd71d881567b1ea663ed3efcf6c5132b354f28d3b0b7d38367", "019f4113742a2b14bd25926b49c649155f267e60d3814b4c0cc84250e46f0083"),
    ] {
        let sig = sk.sign_sha256(m.as_bytes());
        t.check(hex(&sig[..32]) == r && hex(&sig[32..]) == s, || format!("A.2.5 SHA-256 {m}"));
        t.check(p256::verify_sha256(&pk, m.as_bytes(), &sig).is_ok(), || format!("A.2.5 verify {m}"));
        let mut der = [0u8; 72];
        let n = p256::signature_to_der(&sig, &mut der);
        t.check(p256::signature_from_der(&der[..n]) == Ok(sig), || "DER round trip".into());
    }
    t
}

fn p256_keypair() -> Tally {
    let text = match fetch_text("", "KeyPair.rsp") {
        Ok(s) => s,
        Err(sk) => return sk,
    };
    let mut t = Tally::new("");
    // P-256 is the only curve in the file with 32-byte d and Q (P-224: 28, K/B-233: 30, K/B-283: 36).
    for r in parse_rsp(&text) {
        let (d, qx, qy) = (r.bytes("D"), r.bytes("QX"), r.bytes("QY"));
        if d.len() != 32 || qx.len() != 32 {
            continue;
        }
        let sk = SecretKey::from_bytes(&a32(d)).unwrap();
        let mut want = vec![4u8];
        want.extend_from_slice(&qx);
        want.extend_from_slice(&qy);
        t.check(sk.public_key().to_sec1_uncompressed().to_vec() == want, || format!("keypair {}", hex(&qx)));
    }
    t
}

fn p256_siggen() -> Tally {
    let text = match fetch_text("", "SigGen.txt") {
        Ok(s) => s,
        Err(sk) => return sk,
    };
    let mut t = Tally::new("");
    for r in parse_rsp(&text) {
        let Some(sec) = r.h("_") else { continue };
        let Some(hname) = sec.strip_prefix("P-256,") else { continue };
        let Some(digest) = hash_by_name(hname, &r.bytes("MSG")) else {
            t.unsupported += 1;
            continue;
        };
        let sk = SecretKey::from_bytes(&a32(r.bytes("D"))).unwrap();
        let pk_ok = pubkey_xy(&r.bytes("QX"), &r.bytes("QY")).map(|p| p.to_sec1_uncompressed() == sk.public_key().to_sec1_uncompressed()).unwrap_or(false);
        let want = rs(&r.bytes("R"), &r.bytes("S"));
        let sig = sk.sign_prehashed_with_nonce_hazmat(&digest, &a32(r.bytes("K")));
        let ver = p256::verify_prehashed(&sk.public_key(), &digest, &want).is_ok();
        t.check(pk_ok && sig == Some(want) && ver, || format!("{sec} d={}", r.get("D").unwrap()));
    }
    t
}

fn p256_sigver() -> Tally {
    let text = match fetch_text("", "SigVer.rsp") {
        Ok(s) => s,
        Err(sk) => return sk,
    };
    let mut t = Tally::new("");
    for r in parse_rsp(&text) {
        let Some(sec) = r.h("_") else { continue };
        let Some(hname) = sec.strip_prefix("P-256,") else { continue };
        let Some(digest) = hash_by_name(hname, &r.bytes("MSG")) else {
            t.unsupported += 1;
            continue;
        };
        let want = r.get("RESULT").unwrap().starts_with('P');
        let got = match pubkey_xy(&r.bytes("QX"), &r.bytes("QY")) {
            Ok(pk) => p256::verify_prehashed(&pk, &digest, &rs(&r.bytes("R"), &r.bytes("S"))).is_ok(),
            Err(_) => false,
        };
        t.check(got == want, || format!("{sec} {}", r.get("RESULT").unwrap()));
    }
    t
}

fn p256_kas() -> Tally {
    let text = match fetch_text("", "KASValidityTest_ECCStaticUnified_NOKC_ZZOnly_init.fax") {
        Ok(s) => s,
        Err(sk) => return sk,
    };
    let mut t = Tally::new("");
    for r in parse_rsp(&text) {
        if r.h("_") != Some("EC - SHA256") || !r.has("Z") {
            continue;
        }
        let want = r.get("RESULT").unwrap().starts_with('P');
        let got = (|| -> Option<bool> {
            let peer = pubkey_xy(&r.bytes("QSCAVSX"), &r.bytes("QSCAVSY")).ok()?;
            let mine = pubkey_xy(&r.bytes("QSIUTX"), &r.bytes("QSIUTY")).ok()?;
            let sk = SecretKey::from_bytes(&a32(r.bytes("DSIUT"))).ok()?;
            if sk.public_key().to_sec1_uncompressed() != mine.to_sec1_uncompressed() {
                return Some(false);
            }
            Some(sk.diffie_hellman(&peer).ok()?.to_vec() == r.bytes("Z"))
        })()
        .unwrap_or(false);
        t.check(got == want, || format!("KAS count {} {}", r.get("COUNT").unwrap_or("?"), r.get("RESULT").unwrap()));
    }
    t
}

/// The one SubjectPublicKeyInfo shape an id-ecPublicKey/prime256v1 key has in DER, uncompressed (91 bytes)
/// or compressed (59 bytes). Harness-side only: X.509/SPKI parsing belongs to TLSCORE, not crypto_core.
fn spki_point(der: &[u8]) -> Option<&[u8]> {
    const ALG: &[u8] = &[0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
    let (outer, bitlen) = match der.len() {
        91 => (0x59u8, 0x42u8),
        59 => (0x39, 0x22),
        _ => return None,
    };
    if der[0] != 0x30 || der[1] != outer || &der[2..23] != ALG || der[23] != 0x03 || der[24] != bitlen || der[25] != 0 {
        return None;
    }
    Some(&der[26..])
}

fn p256_wy_ecdh() -> Tally {
    let mut t = Tally::new("");
    for (f, spki) in [("wycheproof-ecdh_secp256r1_ecpoint_test.json", false), ("wycheproof-ecdh_secp256r1_test.json", true)] {
        let doc = match fetch_text("", f) {
            Ok(s) => parse_json(&s),
            Err(sk) => return sk,
        };
        for (_g, c) in wycheproof_tests(&doc) {
            let pubb = c.get("public").bytes();
            let mut priv_ = c.get("private").bytes();
            // Wycheproof writes private keys as minimal signed big-endian integers.
            while priv_.len() > 32 && priv_[0] == 0 {
                priv_.remove(0);
            }
            let got = (|| -> Option<Vec<u8>> {
                let pt = if spki { spki_point(&pubb)? } else { &pubb[..] };
                let pk = PublicKey::from_sec1(pt).ok()?;
                if priv_.len() > 32 {
                    return None;
                }
                let mut d = [0u8; 32];
                d[32 - priv_.len()..].copy_from_slice(&priv_);
                Some(SecretKey::from_bytes(&d).ok()?.diffie_hellman(&pk).ok()?.to_vec())
            })();
            match wy_expect(c) {
                Some(true) => t.check(got.as_deref() == Some(&c.get("shared").bytes()[..]), || format!("{f} tc{} {}", c.get("tcId").n(), c.get("comment").s())),
                Some(false) => t.check(got.is_none(), || format!("{f} tc{} accepted: {}", c.get("tcId").n(), c.get("comment").s())),
                None => t.pass += 1,
            }
        }
    }
    t
}

fn p256_wy_ecdsa() -> Tally {
    let mut t = Tally::new("");
    for (f, der) in [("wycheproof-ecdsa_secp256r1_sha256_test.json", true), ("wycheproof-ecdsa_secp256r1_sha256_p1363_test.json", false)] {
        let doc = match fetch_text("", f) {
            Ok(s) => parse_json(&s),
            Err(sk) => return sk,
        };
        for (g, c) in wycheproof_tests(&doc) {
            let pk = PublicKey::from_sec1(&g.get("publicKey").get("uncompressed").bytes());
            let sigb = c.get("sig").bytes();
            let sig = if der { p256::signature_from_der(&sigb).ok() } else { <[u8; 64]>::try_from(sigb).ok() };
            let ok = match (pk, sig) {
                (Ok(pk), Some(sig)) => p256::verify_sha256(&pk, &c.get("msg").bytes(), &sig).is_ok(),
                _ => false,
            };
            match wy_expect(c) {
                Some(v) => t.check(ok == v, || format!("{f} tc{} {}", c.get("tcId").n(), c.get("comment").s())),
                None => t.pass += 1,
            }
        }
    }
    t
}
