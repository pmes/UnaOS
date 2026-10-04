// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// M1 — hashes + MACs + KDFs: SHA-224/256/384/512 (FIPS 180-4 examples, NIST CAVP SHAVS short + long
// byte-oriented messages), HMAC (RFC 4231, Wycheproof), HKDF (RFC 5869, Wycheproof), PBKDF2-HMAC-SHA256
// (RFC 7914 §11, the RFC 6070 inputs under SHA-256 that the kernel's LOGIN-HARD fixture asserts, Wycheproof).

use super::*;
use crypto_core::hkdf;
use crypto_core::hmac::{hmac_sha256, hmac_sha384, hmac_sha512, Hmac};
use crypto_core::pbkdf2::pbkdf2_hmac;
use crypto_core::sha2::{sha224, sha256, sha384, sha512, Digest, Sha224, Sha256, Sha384, Sha512};

pub const SETS: &[(&str, fn() -> Tally)] = &[
    ("sha2/fips180-4-examples", fips180_examples),
    ("sha2/cavp-shortmsg", cavp_short),
    ("sha2/cavp-longmsg", cavp_long),
    ("sha2/streaming-splits", streaming),
    ("hmac/rfc4231", hmac_rfc4231),
    ("hmac/wycheproof", hmac_wycheproof),
    ("hkdf/rfc5869", hkdf_rfc5869),
    ("hkdf/wycheproof", hkdf_wycheproof),
    ("pbkdf2/rfc7914+rfc6070-sha256", pbkdf2_embedded),
    ("pbkdf2/wycheproof", pbkdf2_wycheproof),
];

fn digest_by_len(md_len: usize, msg: &[u8]) -> Vec<u8> {
    match md_len {
        28 => sha224(msg).to_vec(),
        32 => sha256(msg).to_vec(),
        48 => sha384(msg).to_vec(),
        64 => sha512(msg).to_vec(),
        _ => panic!("md len {md_len}"),
    }
}

fn fips180_examples() -> Tally {
    let mut t = Tally::new("");
    let abc = b"abc".as_slice();
    let two = b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq".as_slice();
    let four = b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu".as_slice();
    let million = vec![b'a'; 1_000_000];
    let cases: &[(&[u8], usize, &str)] = &[
        (b"", 32, "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"),
        (abc, 32, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"),
        (two, 32, "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"),
        (&million, 32, "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"),
        (b"", 28, "d14a028c2a3a2bc9476102bb288234c415a2b01f828ea62ac5b3e42f"),
        (abc, 28, "23097d223405d8228642a477bda255b32aadbce4bda0b3f7e36c9da7"),
        (b"", 64, "cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e"),
        (abc, 64, "ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f"),
        (four, 64, "8e959b75dae313da8cf4f72814fc143f8f7779c6eb9f7fa17299aeadb6889018501d289e4900f7e4331b99dec4b5433ac7d329eeb6dd26545e96e55b874be909"),
        (&million, 64, "e718483d0ce769644e2e42c7bc15b4638e1f98b13b2044285632a803afa973ebde0ff244877ea60a4cb0432ce577c31beb009c5c2c49aa2e4eadb217ad8cc09b"),
        (b"", 48, "38b060a751ac96384cd9327eb1b1e36a21fdb71114be07434c0cc7bf63f6e1da274edebfe76f65fbd51ad2f14898b95b"),
        (abc, 48, "cb00753f45a35e8bb5a03d699ac65007272c32ab0eded1631a8b605a43ff5bed8086072ba1e7cc2358baeca134c825a7"),
        (four, 48, "09330c33f71147e83d192fc782cd1b4753111b173b3b05d22fa08086e3b0f712fcc7c71a557e2db966c3e9fa91746039"),
        (&million, 48, "9d0e1809716474cb086e834e310a4a1ced149e9c00f248527972cec5704c2a5b07b8b3dc38ecc4ebae97ddd87f3d8985"),
    ];
    for (msg, n, want) in cases {
        let got = hex(&digest_by_len(*n, msg));
        t.check(got == *want, || format!("len={} md{} got {got}", msg.len(), n * 8));
    }
    t
}

fn run_shavs(t: &mut Tally, text: &str, md_len: usize) {
    for r in parse_rsp(text) {
        let (Some(len), Some(_)) = (r.get("LEN"), r.get("MD")) else { continue };
        let bits: usize = len.parse().unwrap();
        if bits % 8 != 0 {
            continue;
        }
        let msg = r.bytes("MSG");
        let msg = &msg[..bits / 8];
        let want = r.bytes("MD");
        assert_eq!(want.len(), md_len);
        let got = digest_by_len(md_len, msg);
        t.check(got == want, || format!("md{} Len={bits}", md_len * 8));
    }
}

fn cavp_short() -> Tally {
    let mut t = Tally::new("");
    run_shavs(&mut t, include_str!("data/SHA224ShortMsg.rsp"), 28);
    run_shavs(&mut t, include_str!("data/SHA256ShortMsg.rsp"), 32);
    run_shavs(&mut t, include_str!("data/SHA384ShortMsg.rsp"), 48);
    run_shavs(&mut t, include_str!("data/SHA512ShortMsg.rsp"), 64);
    t
}

fn cavp_long() -> Tally {
    let mut t = Tally::new("");
    for (f, n) in [("SHA224LongMsg.rsp", 28), ("SHA256LongMsg.rsp", 32), ("SHA384LongMsg.rsp", 48), ("SHA512LongMsg.rsp", 64)] {
        match fetch_text("", f) {
            Ok(s) => run_shavs(&mut t, &s, n),
            Err(sk) => return sk,
        }
    }
    t
}

/// The streaming API agrees with the one-shot for every split point of a 300-byte message (the buffer
/// edge cases a single-call KAT never reaches: 55/56/63/64/111/112/127/128 byte boundaries).
fn streaming() -> Tally {
    let mut t = Tally::new("");
    let msg: Vec<u8> = (0..300u32).map(|i| (i * 7 + 3) as u8).collect();
    fn split<D: Digest>(msg: &[u8], k: usize) -> Vec<u8> {
        let mut h = D::new();
        h.update(&msg[..k]);
        h.update(&[]);
        h.update(&msg[k..]);
        let mut o = vec![0u8; D::OUTPUT_LEN];
        h.finalize_into(&mut o);
        o
    }
    for n in [0usize, 1, 55, 56, 57, 63, 64, 65, 111, 112, 113, 127, 128, 129, 300] {
        let m = &msg[..n];
        for k in 0..=n {
            t.check(split::<Sha256>(m, k) == sha256(m), || format!("sha256 n={n} k={k}"));
            t.check(split::<Sha224>(m, k) == sha224(m), || format!("sha224 n={n} k={k}"));
            t.check(split::<Sha512>(m, k) == sha512(m), || format!("sha512 n={n} k={k}"));
            t.check(split::<Sha384>(m, k) == sha384(m), || format!("sha384 n={n} k={k}"));
        }
    }
    t
}

fn hmac_any(md_len: usize, key: &[u8], msg: &[u8]) -> Vec<u8> {
    match md_len {
        28 => {
            let mut o = [0u8; 28];
            Hmac::<Sha224>::new(key).mac_into(msg, &mut o);
            o.to_vec()
        }
        32 => hmac_sha256(key, msg).to_vec(),
        48 => hmac_sha384(key, msg).to_vec(),
        64 => hmac_sha512(key, msg).to_vec(),
        _ => panic!(),
    }
}

fn hmac_rfc4231() -> Tally {
    let mut t = Tally::new("");
    for (text, n) in [
        (include_str!("data/hmac-rfc4231-sha224.txt"), 28),
        (include_str!("data/hmac-rfc4231-sha256.txt"), 32),
        (include_str!("data/hmac-rfc4231-sha384.txt"), 48),
        (include_str!("data/hmac-rfc4231-sha512.txt"), 64),
    ] {
        for r in parse_rsp(text) {
            if !r.has("MD") {
                continue;
            }
            let want = r.bytes("MD");
            let got = hmac_any(n, &r.bytes("KEY"), &r.bytes("MSG"));
            t.check(got[..want.len()] == want[..], || format!("hmac{} key={}", n * 8, r.get("KEY").unwrap()));
            // streaming + verify path agrees
            if n == 32 {
                let mut h = Hmac::<Sha256>::new(&r.bytes("KEY"));
                let m = r.bytes("MSG");
                let (a, b) = m.split_at(m.len() / 2);
                h.update(a);
                h.update(b);
                t.check(h.verify(&want).is_ok(), || "hmac256 verify".into());
            }
        }
    }
    // RFC 4231 test case 5 (truncation to 128 bits), which the transcription above omits.
    let k = [0x0cu8; 20];
    let m = b"Test With Truncation";
    for (n, want) in [
        (28, "0e2aea68a90c8d37c988bcdb9fca6fa8"),
        (32, "a3b6167473100ee06e0c796c2955552b"),
        (48, "3abf34c3503b2a23a46efc619baef897"),
        (64, "415fad6271580a531d4179bc891d87a6"),
    ] {
        t.check(hex(&hmac_any(n, &k, m)[..16]) == want, || format!("rfc4231 tc5 hmac{}", n * 8));
    }
    t
}

fn hmac_wycheproof() -> Tally {
    let mut t = Tally::new("");
    for (f, n) in [("wycheproof-hmac_sha256_test.json", 32), ("wycheproof-hmac_sha384_test.json", 48), ("wycheproof-hmac_sha512_test.json", 64)] {
        let doc = match fetch_text("", f) {
            Ok(s) => parse_json(&s),
            Err(sk) => return sk,
        };
        for (_g, c) in wycheproof_tests(&doc) {
            let tag = c.get("tag").bytes();
            let got = hmac_any(n, &c.get("key").bytes(), &c.get("msg").bytes());
            let matches = got[..tag.len()] == tag[..];
            match wy_expect(c) {
                Some(v) => t.check(matches == v, || format!("{f} tc{}", c.get("tcId").n())),
                None => t.pass += 1,
            }
        }
    }
    t
}

fn hkdf_rfc5869() -> Tally {
    let mut t = Tally::new("");
    for r in parse_rsp_by(include_str!("data/rfc-5869-HKDF-SHA256.txt"), "COUNT") {
        if !r.has("OKM") {
            continue;
        }
        let mut prk = [0u8; 32];
        hkdf::extract::<Sha256>(&r.bytes("SALT"), &r.bytes("IKM"), &mut prk);
        t.check(prk.to_vec() == r.bytes("PRK"), || format!("prk count {}", r.get("COUNT").unwrap_or("?")));
        let l: usize = r.get("L").unwrap().parse().unwrap();
        let mut okm = vec![0u8; l];
        hkdf::expand::<Sha256>(&prk, &r.bytes("INFO"), &mut okm).unwrap();
        t.check(okm == r.bytes("OKM"), || format!("okm count {}", r.get("COUNT").unwrap_or("?")));
    }
    t
}

fn hkdf_wycheproof() -> Tally {
    let mut t = Tally::new("");
    for (f, sha512) in [("wycheproof-hkdf_sha256_test.json", false), ("wycheproof-hkdf_sha512_test.json", true)] {
        let doc = match fetch_text("", f) {
            Ok(s) => parse_json(&s),
            Err(sk) => return sk,
        };
        for (_g, c) in wycheproof_tests(&doc) {
            let size = c.get("size").n() as usize;
            let mut okm = vec![0u8; size];
            let r = if sha512 {
                hkdf::hkdf::<Sha512>(&c.get("salt").bytes(), &c.get("ikm").bytes(), &c.get("info").bytes(), &mut okm)
            } else {
                hkdf::hkdf::<Sha256>(&c.get("salt").bytes(), &c.get("ikm").bytes(), &c.get("info").bytes(), &mut okm)
            };
            let ok = r.is_ok() && okm == c.get("okm").bytes();
            match wy_expect(c) {
                Some(true) => t.check(ok, || format!("{f} tc{}", c.get("tcId").n())),
                Some(false) => t.check(r.is_err(), || format!("{f} tc{} accepted", c.get("tcId").n())),
                None => t.pass += 1,
            }
        }
    }
    t
}

fn pbkdf2_embedded() -> Tally {
    let mut t = Tally::new("");
    // RFC 7914 §11 (PBKDF2-HMAC-SHA256 test vectors).
    let cases: &[(&[u8], &[u8], u32, &str)] = &[
        (b"passwd", b"salt", 1, "55ac046e56e3089fec1691c22544b605f94185216dde0465e68b9d57c20dacbc49ca9cccf179b645991664b39d77ef317c71b845b1e30bd509112041d3a19783"),
        (b"Password", b"NaCl", 80000, "4ddcd8f60b98be21830cee5ef22701f9641a4418d04c0414aeff08876b34ab56a1d425a1225833549adb841b51c9b3176a272bdebba1d078478f62b397f33c8d"),
        // RFC 6070's inputs with SHA-256 as the PRF — the kernel's `PBKDF2_KAT` (LOGIN-HARD fixture).
        (b"password", b"salt", 1, "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"),
        (b"password", b"salt", 2, "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43"),
        (b"password", b"salt", 4096, "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a"),
        (b"passwordPASSWORDpassword", b"saltSALTsaltSALTsaltSALTsaltSALTsalt", 4096, "348c89dbcbd32b2f32d814b8116e84cf2b17347ebc1800181c4e2a1fb8dd53e1c635518c7dac47e9"),
        (b"pass\0word", b"sa\0lt", 4096, "89b69d0516f829893c696226650a8687"),
    ];
    for (p, s, c, want) in cases {
        let w = unhex(want);
        let mut out = vec![0u8; w.len()];
        pbkdf2_hmac::<Sha256>(p, s, *c, &mut out).unwrap();
        t.check(out == w, || format!("pbkdf2 c={c} got {}", hex(&out)));
    }
    // The kernel-shaped entry point (fixed 32-byte block) agrees.
    let mut o = [0u8; 32];
    crypto_core::pbkdf2::pbkdf2_hmac_sha256(b"password", b"salt", 2, &mut o);
    t.check(hex(&o) == "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43", || "pbkdf2_hmac_sha256 shape".into());
    t.check(pbkdf2_hmac::<Sha256>(b"p", b"s", 0, &mut o).is_err(), || "c=0 accepted".into());
    t
}

fn pbkdf2_wycheproof() -> Tally {
    let mut t = Tally::new("");
    let doc = match fetch_text("", "wycheproof-pbkdf2_hmacsha256_test.json") {
        Ok(s) => parse_json(&s),
        Err(sk) => return sk,
    };
    for (_g, c) in wycheproof_tests(&doc) {
        let dk = c.get("dk").bytes();
        let mut out = vec![0u8; c.get("dkLen").n() as usize];
        let r = pbkdf2_hmac::<Sha256>(&c.get("password").bytes(), &c.get("salt").bytes(), c.get("iterationCount").n() as u32, &mut out);
        let ok = r.is_ok() && out == dk;
        match wy_expect(c) {
            Some(v) => t.check(ok == v, || format!("tc{}", c.get("tcId").n())),
            None => t.pass += 1,
        }
    }
    t
}
