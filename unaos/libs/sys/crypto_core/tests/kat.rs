// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// `cargo test -p crypto_core --release`: every KAT set (embedded + fetched; fetched sets skip offline).
// One #[test] per milestone so a failure names its milestone; the per-set counts print with --nocapture.

#[path = "../kat/mod.rs"]
mod kat;

fn run(prefixes: &[&str]) {
    let mut bad = Vec::new();
    for p in prefixes {
        for t in kat::run_all(Some(p)) {
            match &t.skipped {
                Some(why) => println!("{:<36} SKIPPED ({why})", t.name),
                None if t.unsupported > 0 => println!("{:<36} pass={:<6} fail={} unsupported={}", t.name, t.pass, t.fail, t.unsupported),
                None => println!("{:<36} pass={:<6} fail={}", t.name, t.pass, t.fail),
            }
            if t.fail > 0 {
                bad.push(format!("{}: {} failed: {:?}", t.name, t.fail, t.failures));
            }
            if t.skipped.is_none() && t.pass == 0 {
                bad.push(format!("{}: ran zero vectors", t.name));
            }
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn m1_hashes_macs_kdfs() {
    run(&["sha2/", "hmac/", "hkdf/", "pbkdf2/"]);
}

#[test]
fn m2_aead() {
    run(&["chacha20/", "poly1305/", "chacha20poly1305/", "aes/", "gcm/"]);
}

#[test]
fn m3_curves() {
    run(&["x25519/", "ed25519/", "p256/"]);
}

#[test]
fn m4_argon2_drbg_ct() {
    run(&["blake2b/", "argon2/", "drbg/", "ct/"]);
}

#[test]
fn m5_p384() {
    run(&["p384/"]);
}

#[test]
fn m6_rsa_verify() {
    run(&["rsa/"]);
}

#[test]
fn m7_sha3_mlkem() {
    run(&["sha3/", "mlkem/"]);
}
