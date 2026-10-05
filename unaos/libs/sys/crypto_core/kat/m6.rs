// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// M6 — RSA signature verification (RFC 8017 / PKCS #1 v2.2): NIST CAVP FIPS 186-3 SigVer15 (RSASSA-PKCS1-v1_5)
// and SigVerPSS (RSASSA-PSS, salt 10) for 1024..4096-bit moduli and every SHA-2 hash; Wycheproof
// rsa_signature (PKCS #1 v1.5: 2048/3072/4096/8192 with SHA-224..512), rsa_pss (sLen 0/32/48/64, MGF1
// hash = or ≠ message hash, misc parameter sweep) and rsa_pkcs1_2048_sig_gen (valid signatures, verified).
// Unsupported (counted): everything using SHA-1 (this crate carries no SHA-1) — the RSA Labs
// pkcs-1v2-1d2-vec PSS vectors are all SHA-1, so the CAVP files are the PKCS #1 known answers here.

use super::*;
use crypto_core::rsa::{self, Hash, PublicKey};

pub const SETS: &[(&str, fn() -> Tally)] = &[
    ("rsa/cavp-sigver-pkcs1v15", cavp_pkcs1),
    ("rsa/cavp-sigver-pss", cavp_pss),
    ("rsa/wycheproof-pkcs1v15", wy_pkcs1),
    ("rsa/wycheproof-pss", wy_pss),
    ("rsa/wycheproof-pkcs1-siggen", wy_siggen),
];

fn hash_of(name: &str) -> Option<Hash> {
    Some(match name.replace('-', "").as_str() {
        "SHA224" => Hash::Sha224,
        "SHA256" => Hash::Sha256,
        "SHA384" => Hash::Sha384,
        "SHA512" => Hash::Sha512,
        _ => return None,
    })
}

fn cavp(file: &str, pss: bool) -> Tally {
    let text = match fetch_text("", file) {
        Ok(s) => s,
        Err(sk) => return sk,
    };
    let mut t = Tally::new("");
    let mut n: Vec<u8> = Vec::new();
    for r in parse_rsp(&text) {
        if r.has("N") {
            n = r.bytes("N");
            continue;
        }
        let Some(alg) = r.get("SHAALG") else { continue };
        let Some(h) = hash_of(alg) else {
            t.unsupported += 1;
            continue;
        };
        let want = r.get("RESULT").unwrap().starts_with('P');
        let got = match PublicKey::new(&n, &r.bytes("E")) {
            Ok(k) if pss => rsa::verify_pss(&k, h, r.bytes("SALTVAL").len(), &r.bytes("MSG"), &r.bytes("S")).is_ok(),
            Ok(k) => rsa::verify_pkcs1v15(&k, h, &r.bytes("MSG"), &r.bytes("S")).is_ok(),
            Err(_) => false,
        };
        t.check(got == want, || format!("{} {} {}", n.len() * 8, alg, r.get("RESULT").unwrap()));
    }
    t
}

fn cavp_pkcs1() -> Tally {
    cavp("SigVer15_186-3.rsp", false)
}
fn cavp_pss() -> Tally {
    cavp("SigVerPSS_186-3.rsp", true)
}

fn key_of(g: &J) -> Result<PublicKey, crypto_core::Error> {
    let pk = g.get("publicKey");
    PublicKey::new(&pk.get("modulus").bytes(), &pk.get("publicExponent").bytes())
}

const WY_PKCS1: &[&str] = &[
    "wycheproof-rsa_signature_2048_sha224_test.json",
    "wycheproof-rsa_signature_2048_sha256_test.json",
    "wycheproof-rsa_signature_2048_sha384_test.json",
    "wycheproof-rsa_signature_2048_sha512_test.json",
    "wycheproof-rsa_signature_3072_sha256_test.json",
    "wycheproof-rsa_signature_3072_sha384_test.json",
    "wycheproof-rsa_signature_3072_sha512_test.json",
    "wycheproof-rsa_signature_4096_sha384_test.json",
    "wycheproof-rsa_signature_4096_sha512_test.json",
    "wycheproof-rsa_signature_8192_sha512_test.json",
];

fn wy_pkcs1() -> Tally {
    let mut t = Tally::new("");
    for f in WY_PKCS1 {
        let doc = match fetch_text("", f) {
            Ok(s) => parse_json(&s),
            Err(sk) => return sk,
        };
        for (g, c) in wycheproof_tests(&doc) {
            let Some(h) = hash_of(g.get("sha").s()) else {
                t.unsupported += 1;
                continue;
            };
            let ok = key_of(g).map(|k| rsa::verify_pkcs1v15(&k, h, &c.get("msg").bytes(), &c.get("sig").bytes()).is_ok()).unwrap_or(false);
            match wy_expect(c) {
                Some(v) => t.check(ok == v, || format!("{f} tc{} {}", c.get("tcId").n(), c.get("comment").s())),
                None => t.pass += 1,
            }
        }
    }
    t
}

const WY_PSS: &[&str] = &[
    "wycheproof-rsa_pss_2048_sha256_mgf1_0_test.json",
    "wycheproof-rsa_pss_2048_sha256_mgf1_32_test.json",
    "wycheproof-rsa_pss_2048_sha384_mgf1_48_test.json",
    "wycheproof-rsa_pss_3072_sha256_mgf1_32_test.json",
    "wycheproof-rsa_pss_4096_sha256_mgf1_32_test.json",
    "wycheproof-rsa_pss_4096_sha384_mgf1_48_test.json",
    "wycheproof-rsa_pss_4096_sha512_mgf1_32_test.json",
    "wycheproof-rsa_pss_4096_sha512_mgf1_64_test.json",
    "wycheproof-rsa_pss_misc_test.json",
];

fn wy_pss() -> Tally {
    let mut t = Tally::new("");
    for f in WY_PSS {
        let doc = match fetch_text("", f) {
            Ok(s) => parse_json(&s),
            Err(sk) => return sk,
        };
        for (g, c) in wycheproof_tests(&doc) {
            let (Some(h), Some(mh)) = (hash_of(g.get("sha").s()), hash_of(g.get("mgfSha").s())) else {
                t.unsupported += 1;
                continue;
            };
            let slen = g.get("sLen").n() as usize;
            let ok = key_of(g)
                .map(|k| {
                    let mut d = [0u8; 64];
                    let m = c.get("msg").bytes();
                    let dl = h.len();
                    match h {
                        Hash::Sha224 => d[..28].copy_from_slice(&crypto_core::sha2::sha224(&m)),
                        Hash::Sha256 => d[..32].copy_from_slice(&crypto_core::sha2::sha256(&m)),
                        Hash::Sha384 => d[..48].copy_from_slice(&crypto_core::sha2::sha384(&m)),
                        Hash::Sha512 => d.copy_from_slice(&crypto_core::sha2::sha512(&m)),
                    }
                    rsa::verify_pss_prehashed(&k, h, mh, slen, &d[..dl], &c.get("sig").bytes()).is_ok()
                })
                .unwrap_or(false);
            match wy_expect(c) {
                Some(v) => t.check(ok == v, || format!("{f} tc{} {}", c.get("tcId").n(), c.get("comment").s())),
                None => t.pass += 1,
            }
        }
    }
    t
}

/// Wycheproof's PKCS #1 v1.5 GENERATION vectors: deterministic signatures that must verify under the
/// group's public key (keyAsn is the PKCS #1 RSAPublicKey; parsed here minimally — harness only).
fn wy_siggen() -> Tally {
    let doc = match fetch_text("", "wycheproof-rsa_pkcs1_2048_sig_gen_test.json") {
        Ok(s) => parse_json(&s),
        Err(sk) => return sk,
    };
    let mut t = Tally::new("");
    for (g, c) in wycheproof_tests(&doc) {
        let Some(h) = hash_of(g.get("sha").s()) else {
            t.unsupported += 1;
            continue;
        };
        let key = rsa_public_key_der(&g.get("keyAsn").bytes()).and_then(|(n, e)| PublicKey::new(&n, &e).ok());
        let ok = key.map(|k| rsa::verify_pkcs1v15(&k, h, &c.get("msg").bytes(), &c.get("sig").bytes()).is_ok()).unwrap_or(false);
        match wy_expect(c) {
            Some(v) => t.check(ok == v, || format!("siggen tc{} {}", c.get("tcId").n(), c.get("comment").s())),
            None => t.pass += 1,
        }
    }
    t
}

/// `RSAPublicKey ::= SEQUENCE { modulus INTEGER, publicExponent INTEGER }` (definite lengths).
fn rsa_public_key_der(d: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
    fn tlv(d: &[u8], i: &mut usize, tag: u8) -> Option<Vec<u8>> {
        if *d.get(*i)? != tag {
            return None;
        }
        *i += 1;
        let l0 = *d.get(*i)? as usize;
        *i += 1;
        let len = if l0 < 0x80 {
            l0
        } else {
            let mut l = 0usize;
            for _ in 0..(l0 & 0x7f) {
                l = (l << 8) | *d.get(*i)? as usize;
                *i += 1;
            }
            l
        };
        let v = d.get(*i..*i + len)?.to_vec();
        *i += len;
        Some(v)
    }
    let mut i = 0;
    let body = tlv(d, &mut i, 0x30)?;
    let mut j = 0;
    let n = tlv(&body, &mut j, 0x02)?;
    let e = tlv(&body, &mut j, 0x02)?;
    Some((n, e))
}
