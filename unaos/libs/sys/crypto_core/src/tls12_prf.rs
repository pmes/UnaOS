// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The TLS 1.2 pseudorandom function (RFC 5246 §5): `PRF(secret, label, seed) = P_<hash>(secret, label + seed)`
//! with `P_hash(secret, seed) = HMAC(secret, A(1) + seed) + HMAC(secret, A(2) + seed) + …`,
//! `A(0) = seed`, `A(i) = HMAC(secret, A(i-1))`. TLSCORE2 (SR58) uses it with SHA-256 (every RFC 5288/7905
//! `_SHA256` suite) and SHA-384 (the `_SHA384` suites, RFC 5289) for the master secret (RFC 7627's extended
//! master secret), the key block and the Finished verify_data.
//!
//! CONSTANT-TIME: as HMAC (secret and seed contents); the output length is public.

use crate::hmac::{Hmac, MAX_OUTPUT};
use crate::sha2::Digest;

/// `PRF(secret, label, seed_parts…)` into `out` (any length). `seed` is the concatenation of `seed`'s parts.
pub fn prf<D: Digest>(secret: &[u8], label: &[u8], seed: &[&[u8]], out: &mut [u8]) {
    let key = Hmac::<D>::new(secret);
    let n = D::OUTPUT_LEN;
    // A(1) = HMAC(secret, label + seed)
    let mut a = [0u8; MAX_OUTPUT];
    let mut m = key.clone();
    m.update(label);
    for s in seed {
        m.update(s);
    }
    m.finalize_into(&mut a[..n]);
    let mut done = 0;
    while done < out.len() {
        let mut m = key.clone();
        m.update(&a[..n]);
        m.update(label);
        for s in seed {
            m.update(s);
        }
        let mut block = [0u8; MAX_OUTPUT];
        m.finalize_into(&mut block[..n]);
        let take = n.min(out.len() - done);
        out[done..done + take].copy_from_slice(&block[..take]);
        done += take;
        // A(i+1) = HMAC(secret, A(i))
        let mut m = key.clone();
        m.update(&a[..n]);
        m.finalize_into(&mut a[..n]);
        crate::ct::Zeroize::zeroize(&mut block);
    }
    crate::ct::Zeroize::zeroize(&mut a);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Sha256, Sha384};

    fn unhex(s: &str) -> [u8; 256] {
        let mut o = [0u8; 256];
        for i in 0..s.len() / 2 {
            o[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
        }
        o
    }

    /// The two published TLS 1.2 PRF vectors (Joseph Birr-Pixton, ietf-tls list 2009; reproduced by OpenSSL 3.0.13
    /// `openssl kdf … TLS1-PRF` byte for byte in this container): P_SHA256 100 bytes, P_SHA384 148 bytes.
    #[test]
    fn published_prf_vectors() {
        let mut out = [0u8; 100];
        prf::<Sha256>(&unhex("9bbe436ba940f017b17652849a71db35")[..16], b"test label", &[&unhex("a0ba9f936cda311827a6f796ffd5198c")[..16]], &mut out);
        let want = "e3f229ba727be17b8d122620557cd453c2aab21d07c3d495329b52d4e61edb5a6b301791e90d35c9c9a46b4e14baf9af0fa022f7077def17abfd3797c0564bab4fbc91666e9def9b97fce34f796789baa48082d122ee42c5a72e5a5110fff70187347b66";
        assert_eq!(out[..], unhex(want)[..100]);
        let mut out = [0u8; 148];
        // The seed split across parts must not matter.
        let seed = unhex("cd665cf6a8447dd6ff8b27555edb7465");
        prf::<Sha384>(&unhex("b80b733d6ceefcdc71566ea48e5567df")[..16], b"test label", &[&seed[..5], &seed[5..16]], &mut out);
        let want = "7b0c18e9ced410ed1804f2cfa34a336a1c14dffb4900bb5fd7942107e81c83cde9ca0faa60be9fe34f82b1233c9146a0e534cb400fed2700884f9dc236f80edd8bfa961144c9e8d792eca722a7b32fc3d416d473ebc2c5fd4abfdad05d9184259b5bf8cd4d90fa0d31e2dec479e4f1a26066f2eea9a69236a3e52655c9e9aee691c8f3a26854308d5eaa3be85e0990703d73e56f";
        assert_eq!(out[..], unhex(want)[..148]);
    }
}
