// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// M2 — AEAD: ChaCha20 (RFC 8439 §2.3.2, A.1, A.2), Poly1305 (§2.5.2, A.3), ChaCha20-Poly1305 (§2.8.2,
// A.5 + OpenSSL's and BoringSSL's evp vectors, Wycheproof), HChaCha20 (draft-irtf-cfrg-xchacha §2.2.1),
// AES (FIPS 197 Appendix C, NIST AESAVS ECB GFSbox/KeySbox/VarKey/VarTxt), AES-GCM (NIST CAVP gcmEncryptExtIV
// / gcmDecrypt 128+256 — every IV length and tag length in them — and Wycheproof AES-GCM).
// Unsupported (counted): ChaCha20-Poly1305 vectors with a non-96-bit nonce in the BoringSSL file
// (the pre-RFC 64-bit-nonce construction).

use super::*;
use crypto_core::aes::Aes;
use crypto_core::chacha20::{block, hchacha20, ChaCha20};
use crypto_core::chacha20poly1305 as cp;
use crypto_core::gcm::AesGcm;
use crypto_core::poly1305::poly1305;

pub const SETS: &[(&str, fn() -> Tally)] = &[
    ("chacha20/rfc8439", chacha_rfc),
    ("poly1305/rfc8439", poly_rfc),
    ("chacha20poly1305/rfc8439+openssl", aead_openssl),
    ("chacha20poly1305/boringssl", aead_boringssl),
    ("chacha20poly1305/wycheproof", aead_wycheproof),
    ("aes/fips197+aesavs-sbox", aes_embedded),
    ("aes/aesavs-varkey-vartxt", aes_fetched),
    ("gcm/cavp", gcm_cavp),
    ("gcm/wycheproof", gcm_wycheproof),
];

fn chacha_rfc() -> Tally {
    let mut t = Tally::new("");
    // §2.3.2 block function test vector.
    let key: [u8; 32] = core::array::from_fn(|i| i as u8);
    let nonce: [u8; 12] = [0, 0, 0, 9, 0, 0, 0, 0x4a, 0, 0, 0, 0];
    t.check(
        hex(&block(&key, 1, &nonce))
            == "10f1e7e4d13b5915500fdd1fa32071c4c7d1f4c733c068030422aa9ac3d46c4ed2826446079faa0914c2d705d98b02a2b5129cd1de164eb9cbd083e8a2503c4e",
        || "§2.3.2".into(),
    );
    // A.2 (and A.1 as its zero-plaintext case) in NIST form.
    for r in parse_rsp(include_str!("data/chacha20-rfc7539.txt")) {
        let key: [u8; 32] = r.bytes("KEY").try_into().unwrap();
        let nonce: [u8; 12] = r.bytes("NONCE").try_into().unwrap();
        let ctr: u32 = r.get("INITIAL_BLOCK_COUNTER").unwrap().parse().unwrap();
        let mut buf = r.bytes("PLAINTEXT");
        // feed in odd-sized pieces to exercise the stream position bookkeeping
        let mut c = ChaCha20::new(&key, &nonce, ctr);
        let (a, b) = buf.split_at_mut(core::cmp::min(7, r.bytes("PLAINTEXT").len()));
        c.apply_keystream(a);
        c.apply_keystream(b);
        t.check(buf == r.bytes("CIPHERTEXT"), || format!("A.2 count {}", r.get("COUNT").unwrap()));
    }
    // HChaCha20, draft-irtf-cfrg-xchacha-03 §2.2.1.
    let n16: [u8; 16] = unhex("000000090000004a0000000031415927").try_into().unwrap();
    t.check(
        hex(&hchacha20(&key, &n16)) == "82413b4227b27bfed30e42508a877d73a0f9e4d58a74a853c12ec41326d3ecdc",
        || "hchacha20".into(),
    );
    t
}

fn poly_rfc() -> Tally {
    let mut t = Tally::new("");
    let key: [u8; 32] = unhex("85d6be7857556d337f4452fe42d506a80103808afb0db2fd4abff6af4149f51b").try_into().unwrap();
    t.check(hex(&poly1305(&key, b"Cryptographic Forum Research Group")) == "a8061dc1305136c6c22b8baf0c0127a9", || "§2.5.2".into());
    for r in parse_rsp(include_str!("data/poly1305-rfc7539.txt")) {
        let key: [u8; 32] = r.bytes("KEY").try_into().unwrap();
        t.check(poly1305(&key, &r.bytes("MSG")).to_vec() == r.bytes("TAG"), || format!("A.3 count {}", r.get("COUNT").unwrap()));
    }
    t
}

/// One AEAD vector: seal must reproduce (ct, tag) when `valid`; open must accept exactly when `valid`.
fn aead_case(t: &mut Tally, what: &str, key: &[u8], nonce: &[u8], aad: &[u8], pt: &[u8], ct: &[u8], tag: &[u8], valid: Option<bool>) {
    let (Ok(key), Ok(nonce12)) = (<[u8; 32]>::try_from(key), <[u8; 12]>::try_from(nonce)) else {
        // A key/nonce of the wrong size cannot even be expressed through the typed API: that is a
        // rejection, which is right exactly when the vector is invalid.
        match valid {
            Some(v) => t.check(!v, || format!("{what}: bad size accepted?")),
            None => t.pass += 1,
        }
        return;
    };
    let tag16: Result<[u8; 16], _> = tag.try_into();
    // One vector, one count: a valid vector must seal to (ct, tag) AND open back to pt; an invalid one
    // must be refused with the buffer left as ciphertext.
    let mut seal_ok = true;
    if valid == Some(true) {
        let mut buf = pt.to_vec();
        let tg = cp::seal_in_place(&key, &nonce12, aad, &mut buf).unwrap();
        seal_ok = buf == ct && tag16.as_ref().map(|x| *x == tg).unwrap_or(false);
    }
    let mut buf = ct.to_vec();
    let opened = match &tag16 {
        Ok(tg) => cp::open_in_place(&key, &nonce12, aad, &mut buf, tg).is_ok(),
        Err(_) => false,
    };
    match valid {
        Some(true) => t.check(seal_ok && opened && buf == pt, || format!("{what}: seal={seal_ok} open={opened}")),
        Some(false) => t.check(!opened && buf == ct, || format!("{what}: forgery accepted (or buffer touched)")),
        None => t.pass += 1,
    }
}

fn aead_openssl() -> Tally {
    let mut t = Tally::new("");
    for r in parse_rsp(include_str!("data/chacha20poly1305-openssl.txt")) {
        let valid = !r.has("RESULT");
        aead_case(&mut t, &format!("openssl count {}", r.get("COUNT").unwrap_or("?")), &r.bytes("KEY"), &r.bytes("IV"), &r.bytes("AAD"), &r.bytes("PLAINTEXT"), &r.bytes("CIPHERTEXT"), &r.bytes("TAG"), Some(valid));
    }
    // XChaCha20-Poly1305 round trip + tamper (no published KAT is carried for it; HChaCha20 has one).
    let key = [7u8; 32];
    let n = [9u8; 24];
    let mut b = b"xchacha".to_vec();
    let tag = cp::xseal_in_place(&key, &n, b"ad", &mut b).unwrap();
    let mut c = b.clone();
    t.check(cp::xopen_in_place(&key, &n, b"ad", &mut c, &tag).is_ok() && c == b"xchacha", || "xchacha rt".into());
    let mut c = b.clone();
    t.check(cp::xopen_in_place(&key, &n, b"aD", &mut c, &tag).is_err(), || "xchacha tamper".into());
    t
}

fn bssl_val(s: &str) -> Vec<u8> {
    if let Some(q) = s.strip_prefix('"') {
        q.trim_end_matches('"').as_bytes().to_vec()
    } else {
        unhex(s)
    }
}

fn aead_boringssl() -> Tally {
    let text = match fetch_text("", "chacha20poly1305-boringssl.txt") {
        Ok(s) => s,
        Err(sk) => return sk,
    };
    let mut t = Tally::new("");
    for r in parse_rsp(&text) {
        let nonce = bssl_val(r.get("NONCE").unwrap());
        if nonce.len() != 12 {
            t.unsupported += 1;
            continue;
        }
        let key = bssl_val(r.get("KEY").unwrap());
        let (pt, ad, ct, tag) = (bssl_val(r.get("IN").unwrap()), bssl_val(r.get("AD").unwrap()), bssl_val(r.get("CT").unwrap()), bssl_val(r.get("TAG").unwrap()));
        aead_case(&mut t, &format!("boringssl count {}", r.get("COUNT").unwrap_or("?")), &key, &nonce, &ad, &pt, &ct, &tag, Some(true));
    }
    t
}

fn aead_wycheproof() -> Tally {
    let doc = match fetch_text("", "wycheproof-chacha20_poly1305_test.json") {
        Ok(s) => parse_json(&s),
        Err(sk) => return sk,
    };
    let mut t = Tally::new("");
    for (_g, c) in wycheproof_tests(&doc) {
        aead_case(&mut t, &format!("tc{}", c.get("tcId").n()), &c.get("key").bytes(), &c.get("iv").bytes(), &c.get("aad").bytes(), &c.get("msg").bytes(), &c.get("ct").bytes(), &c.get("tag").bytes(), wy_expect(c));
    }
    t
}

fn run_ecb(t: &mut Tally, text: &str) {
    for r in parse_rsp(text) {
        if !r.has("CIPHERTEXT") || !r.has("PLAINTEXT") {
            continue;
        }
        let aes = Aes::new(&r.bytes("KEY")).unwrap();
        let mut b: [u8; 16] = r.bytes("PLAINTEXT").try_into().unwrap();
        aes.encrypt_block(&mut b);
        // ENCRYPT and DECRYPT records both state E(K, PLAINTEXT) = CIPHERTEXT.
        t.check(b.to_vec() == r.bytes("CIPHERTEXT"), || format!("key {} count {}", r.get("KEY").unwrap(), r.get("COUNT").unwrap_or("?")));
    }
}

fn aes_embedded() -> Tally {
    let mut t = Tally::new("");
    let pt: [u8; 16] = core::array::from_fn(|i| (i as u8) * 0x11);
    for (klen, want) in [(16, "69c4e0d86a7b0430d8cdb78070b4c55a"), (24, "dda97ca4864cdfe06eaf70a0ec0d7191"), (32, "8ea2b7ca516745bfeafc49904b496089")] {
        let key: Vec<u8> = (0..klen as u8).collect();
        let mut b = pt;
        Aes::new(&key).unwrap().encrypt_block(&mut b);
        t.check(hex(&b) == want, || format!("FIPS 197 C AES-{}", klen * 8));
    }
    // four-block bitsliced pass == four single-block passes
    let aes = Aes::new(&[0x2bu8; 16]).unwrap();
    let mut four: [[u8; 16]; 4] = core::array::from_fn(|k| core::array::from_fn(|j| (k * 16 + j) as u8));
    let singles: Vec<[u8; 16]> = four.iter().map(|b| { let mut x = *b; aes.encrypt_block(&mut x); x }).collect();
    aes.encrypt_blocks(&mut four);
    t.check(four.to_vec() == singles, || "4-lane == 1-lane".into());
    for f in [include_str!("data/ECBGFSbox128.rsp"), include_str!("data/ECBGFSbox256.rsp"), include_str!("data/ECBKeySbox128.rsp"), include_str!("data/ECBKeySbox256.rsp")] {
        run_ecb(&mut t, f);
    }
    t
}

fn aes_fetched() -> Tally {
    let mut t = Tally::new("");
    for f in ["ECBVarKey128.rsp", "ECBVarKey256.rsp", "ECBVarTxt128.rsp", "ECBVarTxt256.rsp"] {
        match fetch_text("", f) {
            Ok(s) => run_ecb(&mut t, &s),
            Err(sk) => return sk,
        }
    }
    t
}

fn gcm_case(t: &mut Tally, what: &str, key: &[u8], iv: &[u8], aad: &[u8], pt: Option<&[u8]>, ct: &[u8], tag: &[u8], valid: Option<bool>) {
    let g = match AesGcm::new(key) {
        Ok(g) => g,
        Err(_) => {
            match valid {
                Some(v) => t.check(!v, || format!("{what}: key rejected")),
                None => t.pass += 1,
            }
            return;
        }
    };
    let mut seal_ok = true;
    if valid == Some(true) {
        if let Some(pt) = pt {
            let mut buf = pt.to_vec();
            let full = g.encrypt_in_place_detached(iv, aad, &mut buf);
            seal_ok = full.map(|f| buf == ct && f[..tag.len()] == tag[..]).unwrap_or(false);
        }
    }
    let mut buf = ct.to_vec();
    let opened = g.decrypt_in_place_detached(iv, aad, &mut buf, tag).is_ok();
    match valid {
        Some(true) => t.check(seal_ok && opened && pt.map(|p| buf == p).unwrap_or(true), || format!("{what}: seal={seal_ok} open={opened}")),
        Some(false) => t.check(!opened && buf == ct, || format!("{what}: forgery accepted")),
        None => t.pass += 1,
    }
}

fn gcm_cavp() -> Tally {
    let mut t = Tally::new("");
    for f in ["gcmEncryptExtIV128.rsp", "gcmEncryptExtIV256.rsp", "gcmDecrypt128.rsp", "gcmDecrypt256.rsp"] {
        let text = match fetch_text("", f) {
            Ok(s) => s,
            Err(sk) => return sk,
        };
        for r in parse_rsp(&text) {
            if !r.has("KEY") || !r.has("TAG") {
                continue;
            }
            let fail = r.has("FAIL");
            let pt = if fail { None } else { Some(r.bytes("PT")) };
            gcm_case(&mut t, &format!("{f} {:?} count {}", r.hdr, r.get("COUNT").unwrap_or("?")), &r.bytes("KEY"), &r.bytes("IV"), &r.bytes("AAD"), pt.as_deref(), &r.bytes("CT"), &r.bytes("TAG"), Some(!fail));
        }
    }
    t
}

fn gcm_wycheproof() -> Tally {
    let doc = match fetch_text("", "wycheproof-aes_gcm_test.json") {
        Ok(s) => parse_json(&s),
        Err(sk) => return sk,
    };
    let mut t = Tally::new("");
    for (_g, c) in wycheproof_tests(&doc) {
        let msg = c.get("msg").bytes();
        gcm_case(&mut t, &format!("tc{}", c.get("tcId").n()), &c.get("key").bytes(), &c.get("iv").bytes(), &c.get("aad").bytes(), Some(&msg), &c.get("ct").bytes(), &c.get("tag").bytes(), wy_expect(c));
    }
    t
}
