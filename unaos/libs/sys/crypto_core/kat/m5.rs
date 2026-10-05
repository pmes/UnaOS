// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// M5 — P-384: RFC 6979 A.2.6 (SHA-384 "sample"/"test", cross-checked against OpenSSL's
// evppkey_ecdsa_rfc6979.txt DER outputs, which are parsed here), NIST CAVP KeyPair / SigGen / SigVer for
// every SHA-2 hash on P-384, CAVP KAS ECC "ZZOnly" ECDH validity ([ED - SHA384]), Wycheproof ECDH (SEC 1
// points and SPKI) and ECDSA (SHA-384 DER + P1363, SHA-512 DER).
// Unsupported (counted): CAVP sections using SHA-1 (this crate carries no SHA-1).

use super::*;
use crypto_core::p384::{self, PublicKey, SecretKey};
use crypto_core::sha2::{sha224, sha256, sha384, sha512};

pub const SETS: &[(&str, fn() -> Tally)] = &[
    ("p384/rfc6979", rfc6979),
    ("p384/cavp-keypair", keypair),
    ("p384/cavp-siggen", siggen),
    ("p384/cavp-sigver", sigver),
    ("p384/cavp-kas-ecdh", kas),
    ("p384/wycheproof-ecdh", wy_ecdh),
    ("p384/wycheproof-ecdsa", wy_ecdsa),
];

fn a48(b: &[u8]) -> Option<[u8; 48]> {
    if b.len() > 48 {
        return None;
    }
    let mut o = [0u8; 48];
    o[48 - b.len()..].copy_from_slice(b);
    Some(o)
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
    if x.len() > 48 || y.len() > 48 {
        return Err(crypto_core::Error::Encoding);
    }
    let mut b = [0u8; 97];
    b[0] = 4;
    b[49 - x.len()..49].copy_from_slice(x);
    b[97 - y.len()..].copy_from_slice(y);
    PublicKey::from_sec1(&b)
}

fn rs(r: &[u8], s: &[u8]) -> [u8; 96] {
    let mut o = [0u8; 96];
    o[48 - r.len()..48].copy_from_slice(r);
    o[96 - s.len()..].copy_from_slice(s);
    o
}

fn rfc6979() -> Tally {
    let mut t = Tally::new("");
    let sk = SecretKey::from_bytes(&a48(&unhex("6b9d3dad2e1b8c1c05b19875b6659f4de23c3b667bf297ba9aa47740787137d896d5724e4c70a825f872c9ea60d2edf5")).unwrap()).unwrap();
    let pk = sk.public_key();
    t.check(
        hex(&pk.to_sec1_uncompressed()) == "04ec3a4e415b4e19a4568618029f427fa5da9a8bc4ae92e02e06aae5286b300c64def8f0ea9055866064a254515480bc138015d9b72d7d57244ea8ef9ac0c621896708a59367f9dfb9f54ca84b3f1c9db1288b231c3ae0d4fe7344fd2533264720",
        || "A.2.6 public key".into(),
    );
    // DER outputs from OpenSSL's evppkey_ecdsa_rfc6979.txt (RFC 6979 A.2.6, SHA-384).
    for (m, der) in [
        ("sample", "306602310094edbb92a5ecb8aad4736e56c691916b3f88140666ce9fa73d64c4ea95ad133c81a648152e44acf96e36dd1e80fabe4602310099ef4aeb15f178cea1fe40db2603138f130e740a19624526203b6351d0a3a94fa329c145786e679e7b82c71a38628ac8"),
        ("test", "30660231008203b63d3c853e8d77227fb377bcf7b7b772e97892a80f36ab775d509d7a5feb0542a7f0812998da8f1dd3ca3cf023db023100ddd0760448d42d8a43af45af836fce4de8be06b485e9b61b827c2f13173923e06a739f040649a667bf3b828246baa5a5"),
    ] {
        let want = p384::signature_from_der(&unhex(der));
        t.check(want.is_ok(), || format!("DER parse {m}"));
        let sig = sk.sign_sha384(m.as_bytes());
        t.check(Ok(sig) == want, || format!("A.2.6 SHA-384 {m}"));
        t.check(p384::verify_sha384(&pk, m.as_bytes(), &sig).is_ok(), || format!("A.2.6 verify {m}"));
        let mut bad = sig;
        bad[95] ^= 1;
        t.check(p384::verify_sha384(&pk, m.as_bytes(), &bad).is_err(), || format!("A.2.6 tamper {m}"));
    }
    // ECDH: both directions agree; compressed encoding decodes to the same key.
    let sk2 = SecretKey::from_bytes(&[7u8; 48]).unwrap();
    let pk2 = sk2.public_key();
    t.check(sk.diffie_hellman(&pk2).unwrap() == sk2.diffie_hellman(&pk).unwrap(), || "ECDH symmetric".into());
    let u = pk2.to_sec1_uncompressed();
    let mut c = [0u8; 49];
    c[0] = 2 | (u[96] & 1);
    c[1..].copy_from_slice(&u[1..49]);
    t.check(PublicKey::from_sec1(&c).map(|k| k.to_sec1_uncompressed()) == Ok(u), || "compressed point".into());
    t
}

fn keypair() -> Tally {
    let text = match fetch_text("", "KeyPair.rsp") {
        Ok(s) => s,
        Err(sk) => return sk,
    };
    let mut t = Tally::new("");
    // P-384 is the only curve in the file with 48-byte d and Q (K/B-409: 52, P-521: 66).
    for r in parse_rsp(&text) {
        let (d, qx, qy) = (r.bytes("D"), r.bytes("QX"), r.bytes("QY"));
        if d.len() != 48 || qx.len() != 48 {
            continue;
        }
        let sk = SecretKey::from_bytes(&a48(&d).unwrap()).unwrap();
        let mut want = vec![4u8];
        want.extend_from_slice(&qx);
        want.extend_from_slice(&qy);
        t.check(sk.public_key().to_sec1_uncompressed().to_vec() == want, || format!("keypair {}", hex(&qx)));
    }
    t
}

fn siggen() -> Tally {
    let text = match fetch_text("", "SigGen.txt") {
        Ok(s) => s,
        Err(sk) => return sk,
    };
    let mut t = Tally::new("");
    for r in parse_rsp(&text) {
        let Some(sec) = r.h("_") else { continue };
        let Some(hname) = sec.strip_prefix("P-384,") else { continue };
        let Some(digest) = hash_by_name(hname, &r.bytes("MSG")) else {
            t.unsupported += 1;
            continue;
        };
        // CAVP fixes k, which the API never lets a caller choose: check Q, then that the published
        // (r, s) verifies, and that our own RFC 6979 signature over the same digest verifies.
        let sk = SecretKey::from_bytes(&a48(&r.bytes("D")).unwrap()).unwrap();
        let pk = sk.public_key();
        let pk_ok = pubkey_xy(&r.bytes("QX"), &r.bytes("QY")).map(|p| p.to_sec1_uncompressed() == pk.to_sec1_uncompressed()).unwrap_or(false);
        let want = rs(&r.bytes("R"), &r.bytes("S"));
        let ver = p384::verify_prehashed(&pk, &digest, &want).is_ok();
        let ours = p384::verify_prehashed(&pk, &digest, &sk.sign_prehashed(&digest)).is_ok();
        t.check(pk_ok && ver && ours, || format!("{sec} d={}", r.get("D").unwrap()));
    }
    t
}

fn sigver() -> Tally {
    let text = match fetch_text("", "SigVer.rsp") {
        Ok(s) => s,
        Err(sk) => return sk,
    };
    let mut t = Tally::new("");
    for r in parse_rsp(&text) {
        let Some(sec) = r.h("_") else { continue };
        let Some(hname) = sec.strip_prefix("P-384,") else { continue };
        let Some(digest) = hash_by_name(hname, &r.bytes("MSG")) else {
            t.unsupported += 1;
            continue;
        };
        let want = r.get("RESULT").unwrap().starts_with('P');
        let got = match pubkey_xy(&r.bytes("QX"), &r.bytes("QY")) {
            Ok(pk) => p384::verify_prehashed(&pk, &digest, &rs(&r.bytes("R"), &r.bytes("S"))).is_ok(),
            Err(_) => false,
        };
        t.check(got == want, || format!("{sec} {}", r.get("RESULT").unwrap()));
    }
    t
}

fn kas() -> Tally {
    let text = match fetch_text("", "KASValidityTest_ECCStaticUnified_NOKC_ZZOnly_init.fax") {
        Ok(s) => s,
        Err(sk) => return sk,
    };
    let mut t = Tally::new("");
    for r in parse_rsp(&text) {
        if r.h("_") != Some("ED - SHA384") || !r.has("Z") {
            continue;
        }
        let want = r.get("RESULT").unwrap().starts_with('P');
        let got = (|| -> Option<bool> {
            let peer = pubkey_xy(&r.bytes("QSCAVSX"), &r.bytes("QSCAVSY")).ok()?;
            let mine = pubkey_xy(&r.bytes("QSIUTX"), &r.bytes("QSIUTY")).ok()?;
            let sk = SecretKey::from_bytes(&a48(&r.bytes("DSIUT"))?).ok()?;
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

/// The SubjectPublicKeyInfo shape of an id-ecPublicKey/secp384r1 key (uncompressed 120 bytes, compressed
/// 72). Harness-side only: SPKI parsing belongs to TLSCORE.
fn spki_point(der: &[u8]) -> Option<&[u8]> {
    const ALG: &[u8] = &[0x30, 0x10, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x05, 0x2b, 0x81, 0x04, 0x00, 0x22];
    let (outer, bitlen) = match der.len() {
        120 => (0x76u8, 0x62u8),
        72 => (0x46, 0x32),
        _ => return None,
    };
    if der[0] != 0x30 || der[1] != outer || &der[2..20] != ALG || der[20] != 0x03 || der[21] != bitlen || der[22] != 0 {
        return None;
    }
    Some(&der[23..])
}

fn wy_ecdh() -> Tally {
    let mut t = Tally::new("");
    for (f, spki) in [("wycheproof-ecdh_secp384r1_ecpoint_test.json", false), ("wycheproof-ecdh_secp384r1_test.json", true)] {
        let doc = match fetch_text("", f) {
            Ok(s) => parse_json(&s),
            Err(sk) => return sk,
        };
        for (_g, c) in wycheproof_tests(&doc) {
            let pubb = c.get("public").bytes();
            let mut priv_ = c.get("private").bytes();
            while priv_.len() > 48 && priv_[0] == 0 {
                priv_.remove(0);
            }
            let got = (|| -> Option<Vec<u8>> {
                let pt = if spki { spki_point(&pubb)? } else { &pubb[..] };
                let pk = PublicKey::from_sec1(pt).ok()?;
                Some(SecretKey::from_bytes(&a48(&priv_)?).ok()?.diffie_hellman(&pk).ok()?.to_vec())
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

fn wy_ecdsa() -> Tally {
    let mut t = Tally::new("");
    for (f, der, h) in [
        ("wycheproof-ecdsa_secp384r1_sha384_test.json", true, "SHA-384"),
        ("wycheproof-ecdsa_secp384r1_sha384_p1363_test.json", false, "SHA-384"),
        ("wycheproof-ecdsa_secp384r1_sha512_test.json", true, "SHA-512"),
    ] {
        let doc = match fetch_text("", f) {
            Ok(s) => parse_json(&s),
            Err(sk) => return sk,
        };
        for (g, c) in wycheproof_tests(&doc) {
            let pk = PublicKey::from_sec1(&g.get("publicKey").get("uncompressed").bytes());
            let sigb = c.get("sig").bytes();
            let sig = if der { p384::signature_from_der(&sigb).ok() } else { <[u8; 96]>::try_from(sigb).ok() };
            let digest = hash_by_name(h, &c.get("msg").bytes()).unwrap();
            let ok = match (pk, sig) {
                (Ok(pk), Some(sig)) => p384::verify_prehashed(&pk, &digest, &sig).is_ok(),
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
