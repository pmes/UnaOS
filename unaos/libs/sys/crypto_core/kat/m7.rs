// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// M7 (CTCORE, SR60) — SHA-3 / SHAKE (FIPS 202) and ML-KEM (FIPS 203) against NIST's ACVP vectors
// (usnistgov/ACVP-Server gen-val/json-files/*/internalProjection.json at a pinned commit; inputs AND expected
// outputs). SHA3-224/256/384/512: AFT (byte-oriented messages) + the 100-iteration Monte Carlo test; SHAKE128/256:
// AFT + VOT (byte-oriented outputs). ML-KEM-512/768/1024: keyGen (d, z → ek, dk), encapsulation (ek, m → c, k),
// decapsulation (dk, c → k, valid and modified ciphertexts → implicit rejection), encapsulationKeyCheck and
// decapsulationKeyCheck (§7.2 / §7.3). Unsupported, counted: bit-oriented SHA-3/SHAKE lengths (len % 8 ≠ 0 —
// this crate hashes bytes), SHAKE's MCT, the 64 GiB LDT messages.

use super::*;
use crypto_core::mlkem::{self, Params, ML_KEM_1024, ML_KEM_512, ML_KEM_768};
use crypto_core::sha3;

pub const SETS: &[(&str, fn() -> Tally)] = &[
    ("sha3/acvp-sha3-224", || sha3_set("SHA3-224", "acvp-sha3-224.json", 28)),
    ("sha3/acvp-sha3-256", || sha3_set("SHA3-256", "acvp-sha3-256.json", 32)),
    ("sha3/acvp-sha3-384", || sha3_set("SHA3-384", "acvp-sha3-384.json", 48)),
    ("sha3/acvp-sha3-512", || sha3_set("SHA3-512", "acvp-sha3-512.json", 64)),
    ("sha3/acvp-shake-128", || shake_set("acvp-shake-128.json", 128)),
    ("sha3/acvp-shake-256", || shake_set("acvp-shake-256.json", 256)),
    ("mlkem/acvp-keygen", mlkem_keygen),
    ("mlkem/acvp-encap-decap", mlkem_encap_decap),
    ("mlkem/acvp-keycheck", mlkem_keycheck),
];

fn sha3_of(bits: usize, m: &[u8]) -> Vec<u8> {
    match bits {
        28 => sha3::sha3_224(m).to_vec(),
        32 => sha3::sha3_256(m).to_vec(),
        48 => sha3::sha3_384(m).to_vec(),
        _ => sha3::sha3_512(m).to_vec(),
    }
}

fn bits_of(j: &J) -> usize {
    match j {
        J::Str(s) => s.parse().unwrap_or(0),
        J::Num(n) => *n as usize,
        _ => 0,
    }
}

fn sha3_set(name: &str, file: &str, out: usize) -> Tally {
    let text = match fetch_text(name, file) {
        Ok(t) => t,
        Err(t) => return t,
    };
    let mut t = Tally::new(name);
    for g in parse_json(&text).get("testGroups").arr() {
        let ty = g.get("testType").s().to_string();
        for c in g.get("tests").arr() {
            let len = bits_of(c.get("len"));
            match ty.as_str() {
                "AFT" if len % 8 == 0 => {
                    let msg = c.get("msg").bytes();
                    let got = sha3_of(out, &msg[..len / 8]);
                    t.check(got == c.get("md").bytes(), || format!("{name} tc {} len {len}", c.get("tcId").s()));
                }
                "MCT" => {
                    // ACVP SHA-3 standard MCT: MD0 = seed; 100 × (1000 × MD_i = SHA3(MD_{i-1})).
                    let mut seed = c.get("msg").bytes();
                    for (j, r) in c.get("resultsArray").arr().iter().enumerate() {
                        let mut md = seed.clone();
                        for _ in 0..1000 {
                            md = sha3_of(out, &md);
                        }
                        t.check(md == r.get("md").bytes(), || format!("{name} MCT round {j}"));
                        seed = md;
                    }
                }
                _ => t.unsupported += 1,
            }
        }
    }
    t
}

fn shake_set(file: &str, which: usize) -> Tally {
    let name = if which == 128 { "SHAKE-128" } else { "SHAKE-256" };
    let text = match fetch_text(name, file) {
        Ok(t) => t,
        Err(t) => return t,
    };
    let mut t = Tally::new(name);
    for g in parse_json(&text).get("testGroups").arr() {
        let ty = g.get("testType").s().to_string();
        for c in g.get("tests").arr() {
            let len = bits_of(c.get("len"));
            let out_bits = bits_of(c.get("outLen"));
            if !(ty == "AFT" || ty == "VOT") || len % 8 != 0 || out_bits % 8 != 0 || out_bits == 0 {
                t.unsupported += 1;
                continue;
            }
            let msg = c.get("msg").bytes();
            let mut o = vec![0u8; out_bits / 8];
            if which == 128 {
                sha3::shake128(&msg[..len / 8], &mut o);
            } else {
                sha3::shake256(&msg[..len / 8], &mut o);
            }
            t.check(o == c.get("md").bytes(), || format!("{name} tc {} len {len} out {out_bits}", c.get("tcId").s()));
        }
    }
    t
}

fn params(g: &J) -> Params {
    match g.get("parameterSet").s() {
        "ML-KEM-512" => ML_KEM_512,
        "ML-KEM-768" => ML_KEM_768,
        _ => ML_KEM_1024,
    }
}

fn arr32(b: Vec<u8>) -> [u8; 32] {
    b.try_into().expect("32 bytes")
}

fn mlkem_keygen() -> Tally {
    let text = match fetch_text("mlkem/acvp-keygen", "acvp-mlkem-keygen.json") {
        Ok(t) => t,
        Err(t) => return t,
    };
    let mut t = Tally::new("mlkem/acvp-keygen");
    for g in parse_json(&text).get("testGroups").arr() {
        let p = params(g);
        for c in g.get("tests").arr() {
            let mut ek = vec![0u8; p.ek_len()];
            let mut dk = vec![0u8; p.dk_len()];
            mlkem::keygen_internal(&p, &arr32(c.get("d").bytes()), &arr32(c.get("z").bytes()), &mut ek, &mut dk);
            t.check(ek == c.get("ek").bytes() && dk == c.get("dk").bytes(), || format!("{} keyGen tc {}", g.get("parameterSet").s(), c.get("tcId").n()));
        }
    }
    t
}

fn mlkem_encap_decap() -> Tally {
    let text = match fetch_text("mlkem/acvp-encap-decap", "acvp-mlkem-encapdecap.json") {
        Ok(t) => t,
        Err(t) => return t,
    };
    let mut t = Tally::new("mlkem/acvp-encap-decap");
    for g in parse_json(&text).get("testGroups").arr() {
        let p = params(g);
        let f = g.get("function").s().to_string();
        for c in g.get("tests").arr() {
            let tc = c.get("tcId").n();
            match f.as_str() {
                "encapsulation" => {
                    let ek = c.get("ek").bytes();
                    let mut ct = vec![0u8; p.ct_len()];
                    let k = mlkem::encaps_internal(&p, &ek, &arr32(c.get("m").bytes()), &mut ct);
                    // The decapsulation of our own ciphertext must give the same key (dk is in the vector too).
                    let back = mlkem::decaps(&p, &c.get("dk").bytes(), &ct);
                    t.check(ct == c.get("c").bytes() && k.to_vec() == c.get("k").bytes() && back == Ok(k), || format!("{} encap tc {tc}", g.get("parameterSet").s()));
                }
                "decapsulation" => {
                    let k = mlkem::decaps(&p, &c.get("dk").bytes(), &c.get("c").bytes());
                    t.check(k.map(|k| k.to_vec()) == Ok(c.get("k").bytes()), || format!("{} decap tc {tc} ({})", g.get("parameterSet").s(), c.get("reason").s()));
                }
                _ => {} // the key-check groups: mlkem/acvp-keycheck
            }
        }
    }
    t
}

fn mlkem_keycheck() -> Tally {
    let text = match fetch_text("mlkem/acvp-keycheck", "acvp-mlkem-encapdecap.json") {
        Ok(t) => t,
        Err(t) => return t,
    };
    let mut t = Tally::new("mlkem/acvp-keycheck");
    for g in parse_json(&text).get("testGroups").arr() {
        let p = params(g);
        let f = g.get("function").s().to_string();
        for c in g.get("tests").arr() {
            let want = matches!(c.get("testPassed"), J::Bool(true));
            let got = match f.as_str() {
                "encapsulationKeyCheck" => mlkem::check_ek(&p, &c.get("ek").bytes()),
                "decapsulationKeyCheck" => mlkem::check_dk(&p, &c.get("dk").bytes()),
                _ => continue,
            };
            t.check(got == want, || format!("{} {f} tc {} ({})", g.get("parameterSet").s(), c.get("tcId").n(), c.get("reason").s()));
        }
    }
    t
}
