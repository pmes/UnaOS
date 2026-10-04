// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// M4 — BLAKE2b (RFC 7693 Appendix A, the BLAKE2 authors' 256 keyed KATs), Argon2 (RFC 9106 §5.1-5.3 for
// Argon2d/i/id with secret and associated data; the reference implementation's test.c vectors for
// Argon2i v1.0 + v1.3 and Argon2id v1.3), the ChaCha20 DRBG (against its definition), constant-time helpers.

use super::*;
use crypto_core::argon2::{self, Params, Variant, Version};
use crypto_core::blake2b::{blake2b, Blake2b};
use crypto_core::chacha20::ChaCha20;
use crypto_core::drbg::{ChaChaDrbg, Entropy, GetrandomEntropy, OsEntropy, RESEED_INTERVAL};

pub const SETS: &[(&str, fn() -> Tally)] = &[
    ("blake2b/rfc7693", blake_rfc),
    ("blake2b/kat", blake_kat),
    ("argon2/rfc9106", argon_rfc),
    ("argon2/phc-reference", argon_phc),
    ("drbg/construction", drbg),
    ("ct/helpers", ct_helpers),
];

fn blake_rfc() -> Tally {
    let mut t = Tally::new("");
    let mut o = [0u8; 64];
    blake2b(&mut o, &[], b"abc").unwrap();
    t.check(hex(&o) == "ba80a53f981c4d0d6a2797b69f12f6e94c212f14685ac4b74b12bb6fdbffa2d17d87c5392aab792dc252d5de4533cc9518d38aa8dbf1925ab92386edd4009923", || "A abc".into());
    blake2b(&mut o, &[], b"").unwrap();
    t.check(hex(&o) == "786a02f742015903c6c6fd852552d272912f4740e15847618a86e217f71f5419d25e1031afee585313896444934eb04b903a685b1448b755d56f701afe9be2ce", || "empty".into());
    // streaming == one-shot across block boundaries
    let msg: Vec<u8> = (0..400u32).map(|i| i as u8).collect();
    for n in [0usize, 1, 127, 128, 129, 255, 256, 257, 400] {
        let mut one = [0u8; 64];
        blake2b(&mut one, b"k", &msg[..n]).unwrap();
        for k in [0, n / 3, n] {
            let mut h = Blake2b::new_keyed(64, b"k").unwrap();
            h.update(&msg[..k]);
            h.update(&msg[k..n]);
            let mut two = [0u8; 64];
            h.finalize_into(&mut two);
            t.check(one == two, || format!("stream n={n} k={k}"));
        }
    }
    t
}

fn blake_kat() -> Tally {
    let text = match fetch_text("", "blake2b-kat.txt") {
        Ok(s) => s,
        Err(sk) => return sk,
    };
    let mut t = Tally::new("");
    let (mut inp, mut key) = (Vec::new(), Vec::new());
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("in:") {
            inp = unhex(v.trim());
        } else if let Some(v) = line.strip_prefix("key:") {
            key = unhex(v.trim());
        } else if let Some(v) = line.strip_prefix("hash:") {
            let mut o = [0u8; 64];
            blake2b(&mut o, &key, &inp).unwrap();
            t.check(o.to_vec() == unhex(v.trim()), || format!("in len {}", inp.len()));
        }
    }
    t
}

fn argon_rfc() -> Tally {
    let mut t = Tally::new("");
    for (variant, want) in [
        (Variant::Argon2d, "512b391b6f1162975371d30919734294f868e3be3984f3c1a13a4db9fabe4acb"),
        (Variant::Argon2i, "c814d9d1dc7f37aa13f0d77f2494bda1c8de6b016dd388d29952a4c4672b6ce8"),
        (Variant::Argon2id, "0d640df58d78766c08c037a34a8b53c9d01ef0452d75b65eb52520e96b01e659"),
    ] {
        let p = Params { variant, version: Version::V0x13, m_kib: 32, t: 3, p: 4 };
        let mut out = [0u8; 32];
        argon2::hash(&p, &[1u8; 32], &[2u8; 16], &[3u8; 8], &[4u8; 12], &mut out).unwrap();
        t.check(hex(&out) == want, || format!("{variant:?} got {}", hex(&out)));
    }
    // the no-alloc entry with lent memory agrees, and refuses too little memory
    let p = Params { variant: Variant::Argon2id, version: Version::V0x13, m_kib: 32, t: 3, p: 4 };
    let mut mem = [argon2::Block::ZERO; 32];
    let mut out = [0u8; 32];
    argon2::argon2(&p, &[1u8; 32], &[2u8; 16], &[3u8; 8], &[4u8; 12], &mut mem, &mut out).unwrap();
    t.check(hex(&out) == "0d640df58d78766c08c037a34a8b53c9d01ef0452d75b65eb52520e96b01e659", || "lent memory".into());
    t.check(argon2::argon2(&p, b"p", b"saltsalt", &[], &[], &mut mem[..31], &mut out).is_err(), || "short memory accepted".into());
    t.check(mem.iter().all(|b| b.0.iter().all(|&w| w == 0)), || "memory not zeroized".into());
    t
}

fn argon_phc() -> Tally {
    let mut t = Tally::new("");
    for line in include_str!("data/argon2-phc.txt").lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = line.split_whitespace().collect();
        let variant = match f[0] {
            "i" => Variant::Argon2i,
            "d" => Variant::Argon2d,
            _ => Variant::Argon2id,
        };
        let version = if f[1] == "0x10" { Version::V0x10 } else { Version::V0x13 };
        let p = Params { variant, version, t: f[2].parse().unwrap(), m_kib: 1 << f[3].parse::<u32>().unwrap(), p: f[4].parse().unwrap() };
        let mut out = [0u8; 32];
        argon2::hash(&p, f[5].as_bytes(), f[6].as_bytes(), &[], &[], &mut out).unwrap();
        t.check(hex(&out) == f[7], || line.to_string());
    }
    t
}

/// A deterministic "entropy" source for the construction proof: counts its calls.
struct Counter {
    next: u8,
    calls: usize,
}
impl Entropy for Counter {
    fn fill(&mut self, out: &mut [u8]) -> Result<(), crypto_core::Error> {
        self.calls += 1;
        for b in out.iter_mut() {
            *b = self.next;
            self.next = self.next.wrapping_add(1);
        }
        Ok(())
    }
}

fn drbg() -> Tally {
    let mut t = Tally::new("");
    let mut d = ChaChaDrbg::new(Counter { next: 0, calls: 0 }, b"pers").unwrap();
    // Oracle: the definition. key0 = SHA-256(domain || 0^32 || entropy[0..48] || "pers");
    // request 1: stream = ChaCha20(key0, 0^12, ctr 0); key1 = stream[0..32]; out = stream[32..32+n].
    let ent: Vec<u8> = (0..48u8).collect();
    let mut h = crypto_core::sha2::Sha256::new();
    h.update(b"UnaOS CRYPTOCORE ChaCha20-DRBG v1");
    h.update(&[0u8; 32]);
    h.update(&ent);
    h.update(b"pers");
    let key0 = h.finalize();
    let mut stream = vec![0u8; 32 + 100];
    ChaCha20::new(&key0, &[0u8; 12], 0).apply_keystream(&mut stream);
    let mut out = [0u8; 100];
    d.fill(&mut out).unwrap();
    t.check(out[..] == stream[32..], || "request 1 = keystream[32..]".into());
    let key1: [u8; 32] = stream[..32].try_into().unwrap();
    let mut s2 = vec![0u8; 32 + 50];
    ChaCha20::new(&key1, &[0u8; 12], 0).apply_keystream(&mut s2);
    let mut out2 = [0u8; 50];
    d.fill(&mut out2).unwrap();
    t.check(out2[..] == s2[32..], || "request 2 under the erased-forward key".into());
    // reseed interval: exactly one more entropy draw after RESEED_INTERVAL requests
    let mut d = ChaChaDrbg::new(Counter { next: 7, calls: 0 }, b"").unwrap();
    let mut b = [0u8; 1];
    for _ in 0..RESEED_INTERVAL {
        d.fill(&mut b).unwrap();
    }
    t.check(d.requests_since_seed() == RESEED_INTERVAL, || "interval count".into());
    d.fill(&mut b).unwrap();
    t.check(d.requests_since_seed() == 1, || "reseeded at the interval".into());
    t.check(d.fill(&mut vec![0u8; crypto_core::drbg::MAX_REQUEST + 1]).is_err(), || "oversized request accepted".into());
    // SYS_GETRANDOM adapter: short counts are looped, errors surface
    let mut served = 0usize;
    let mut g = GetrandomEntropy::new(|buf: &mut [u8]| {
        let n = core::cmp::min(buf.len(), 7);
        for (i, x) in buf[..n].iter_mut().enumerate() {
            *x = (served + i) as u8;
        }
        served += n;
        n as isize
    });
    let mut e = [0u8; 600];
    t.check(g.fill(&mut e).is_ok() && e.iter().enumerate().all(|(i, &x)| x == i as u8), || "getrandom short counts".into());
    let mut bad = GetrandomEntropy::new(|_: &mut [u8]| -11isize);
    t.check(bad.fill(&mut e).is_err(), || "getrandom error swallowed".into());
    // the host source works and two DRBGs from it differ
    let mut a = ChaChaDrbg::new(OsEntropy, b"").unwrap();
    let mut c = ChaChaDrbg::new(OsEntropy, b"").unwrap();
    let (mut x, mut y) = ([0u8; 32], [0u8; 32]);
    a.fill(&mut x).unwrap();
    c.fill(&mut y).unwrap();
    t.check(x != y && x != [0u8; 32], || "OsEntropy".into());
    t
}

fn ct_helpers() -> Tally {
    use crypto_core::ct::*;
    let mut t = Tally::new("");
    t.check(ct_eq(b"same", b"same") && !ct_eq(b"same", b"samf") && !ct_eq(b"a", b"ab"), || "ct_eq".into());
    let mut o = [0u8; 3];
    ct_select(Choice::from_u8(1), b"abc", b"xyz", &mut o);
    t.check(&o == b"abc", || "ct_select 1".into());
    ct_select(Choice::from_u8(0), b"abc", b"xyz", &mut o);
    t.check(&o == b"xyz", || "ct_select 0".into());
    t.check(ct_select_u64(Choice::from_u8(1), 1, 2) == 1 && ct_select_u32(Choice::from_u8(0), 1, 2) == 2, || "select ints".into());
    let mut k = [0xAAu8; 32];
    k.zeroize();
    t.check(k == [0u8; 32], || "zeroize".into());
    t
}
