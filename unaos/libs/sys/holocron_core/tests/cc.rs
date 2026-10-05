// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The production suite (CRYPTOCORE) under the ring, the service and the agent, and the entropy bridge's
//! refusal. The ring uses the Argon2id floor (19 MiB, t=2).

use holocron_core::agent;
use holocron_core::cc::{CryptoCore, DrbgEntropy};
use holocron_core::format::{self, Meta};
use holocron_core::ring::{Ring, RingError};
use holocron_core::seal::{Entropy, EntropyError, KdfParams, Signer};
use holocron_core::service::{Holocron, MemStore};
use holocron_core::testseal::{TestEntropy, TestSealer};
use holocron_core::wire::{Request, status};

fn hex(s: &str) -> Vec<u8> {
    let s: String = s.split_whitespace().collect();
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

/// RFC 8032 §7.1 TEST 1 (empty message).
const T1_SEED: &str = "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60";
const T1_PUB: &str = "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a";
const T1_SIG: &str = "e5564300c360ac729086e2cc806e828a84877f1eb8e5d974d873e065224901555fb8821590a33bacc61e39701cf9b46bd25bf5f0595bbe24655141438e7a100b";
/// RFC 8032 §7.1 TEST 2 (one byte 0x72).
const T2_SEED: &str = "4ccd089b28ff96da9db6c346ec114e0f5b8a319f35aba624da8cf6ed4fb8a6fb";
const T2_PUB: &str = "3d4017c3e843895a92b70aa74d1b7ebc9c982ccf2ec4968cc0cd55f12af4660c";
const T2_SIG: &str = "92a009a9f0d4cab8720e820b5f642540a2b27b5416503f8fb3762223ebdb69da085ac1e43e15996e458f3613d0f11d8c387b2eaeb4302aeeb00d291612bb0c00";

const OWNER: &str = "user:peter#1001";

fn arr32(v: &[u8]) -> [u8; 32] {
    v.try_into().unwrap()
}

#[test]
fn cc_signer_rfc8032_vectors() {
    let s = CryptoCore;
    for (seed, public, msg, sig) in [(T1_SEED, T1_PUB, &b""[..], T1_SIG), (T2_SEED, T2_PUB, &[0x72u8][..], T2_SIG)] {
        let seed = arr32(&hex(seed));
        assert_eq!(s.public_key(&seed).to_vec(), hex(public));
        let got = s.sign(&seed, msg);
        assert_eq!(got.to_vec(), hex(sig));
        assert!(s.verify(&arr32(&hex(public)), msg, &got));
        let mut bad = got;
        bad[0] ^= 1;
        assert!(!s.verify(&arr32(&hex(public)), msg, &bad));
    }
}

#[test]
fn cc_agent_signs_rfc8032_test1() {
    let mut h = Holocron::new(CryptoCore, CryptoCore, MemStore::default(), TestEntropy::default(), OWNER.into(), KdfParams::FLOOR);
    let call = |h: &mut Holocron<_, _, _, _>, r: Request| h.handle(Some(OWNER), r.verb(), &r.encode_body(), 0, 0);
    assert_eq!(call(&mut h, Request::Unlock { create: true, password: b"pw".to_vec() }).status, status::OK);
    let put = Request::Put { ns: "ssh".into(), name: "t1".into(), kind: "ssh-ed25519".into(), label: "rfc8032-t1".into(), data: hex(T1_SEED) };
    assert_eq!(call(&mut h, put).status, status::OK);
    // REQUEST_IDENTITIES -> the RFC 8709 blob of TEST 1's public key, comment without the test suffix.
    let ans = agent::handle(&mut h, Some(OWNER), &[agent::REQUEST_IDENTITIES], 0, 0);
    let mut want = vec![agent::IDENTITIES_ANSWER, 0, 0, 0, 1];
    let blob = agent::ed25519_blob(&arr32(&hex(T1_PUB)));
    want.extend_from_slice(&(blob.len() as u32).to_be_bytes());
    want.extend_from_slice(&blob);
    want.extend_from_slice(&10u32.to_be_bytes());
    want.extend_from_slice(b"rfc8032-t1");
    assert_eq!(ans, want);
    // SIGN_REQUEST over the empty message -> TEST 1's signature in the RFC 8709 §6 blob.
    let mut req = vec![agent::SIGN_REQUEST];
    req.extend_from_slice(&(blob.len() as u32).to_be_bytes());
    req.extend_from_slice(&blob);
    req.extend_from_slice(&0u32.to_be_bytes());
    req.extend_from_slice(&0u32.to_be_bytes());
    let rsp = agent::handle(&mut h, Some(OWNER), &req, 0, 0);
    assert_eq!(rsp, agent::sign_response(&hex(T1_SIG).try_into().unwrap()));
    // And the bus verb gives the same 64 bytes.
    let r = call(&mut h, Request::Sign { key: "t1".into(), data: vec![] });
    assert_eq!(r.body, hex(T1_SIG));
}

fn urandom() -> DrbgEntropy<crypto_core::drbg::OsEntropy> {
    DrbgEntropy::new(crypto_core::drbg::OsEntropy, b"holocron tests").unwrap()
}

/// A CRYPTOCORE entropy source that delivers `ok` fills, then fails forever.
struct Dies {
    ok: u32,
}
impl crypto_core::drbg::Entropy for Dies {
    fn fill(&mut self, out: &mut [u8]) -> Result<(), crypto_core::Error> {
        if self.ok == 0 {
            return Err(crypto_core::Error::Entropy);
        }
        self.ok -= 1;
        out.fill(0x5a);
        Ok(())
    }
}

/// A Holocron entropy source that fails after `ok` fills.
struct Flaky {
    ok: u32,
    inner: TestEntropy,
}
impl Entropy for Flaky {
    fn fill(&mut self, buf: &mut [u8]) -> Result<(), EntropyError> {
        if self.ok == 0 {
            return Err(EntropyError);
        }
        self.ok -= 1;
        self.inner.fill(buf)
    }
}

#[test]
fn cc_entropy_failure_refuses_to_seal() {
    // A dead source cannot even seed the DRBG: the daemon refuses to start rather than run unseeded.
    assert!(DrbgEntropy::new(Dies { ok: 0 }, b"x").is_err());
    // Seeded once, then the source dies: the DRBG keeps serving until its reseed interval, then the
    // failure surfaces — it is never papered over.
    let mut e = DrbgEntropy::new(Dies { ok: 1 }, b"x").unwrap();
    let mut buf = [0u8; 16];
    assert!(e.fill(&mut buf).is_ok());
    let mut forced = 0;
    while e.fill(&mut buf).is_ok() {
        forced += 1;
        assert!(forced < 2_000_000, "DRBG never reseeded");
    }
    // Ring creation with no entropy: refused, nothing returned.
    let mut r = Ring::new(CryptoCore);
    assert_eq!(r.create(OWNER, b"pw", KdfParams::FLOOR, &mut Flaky { ok: 0, inner: TestEntropy::default() }).err(), Some(RingError::Entropy));
    assert_eq!(r.create(OWNER, b"pw", KdfParams::FLOOR, &mut Flaky { ok: 1, inner: TestEntropy::default() }).err(), Some(RingError::Entropy));
    // Through the service: create works (2 fills), the put's salt fill fails -> EIO and the store is untouched.
    let mut h = Holocron::new(CryptoCore, CryptoCore, MemStore::default(), Flaky { ok: 2, inner: TestEntropy::default() }, OWNER.into(), KdfParams::FLOOR);
    let call = |h: &mut Holocron<_, _, _, _>, r: Request| h.handle(Some(OWNER), r.verb(), &r.encode_body(), 0, 0);
    assert_eq!(call(&mut h, Request::Unlock { create: true, password: b"pw".to_vec() }).status, status::OK);
    let put = Request::Put { ns: "vein".into(), name: "k".into(), kind: "api-key".into(), label: String::new(), data: b"sk".to_vec() };
    assert_eq!(call(&mut h, put).status, status::IO);
    // A minted key with no entropy: refused before any seed exists.
    let keygen = Request::Put { ns: "ssh".into(), name: "id".into(), kind: "ssh-ed25519".into(), label: String::new(), data: vec![] };
    assert_eq!(call(&mut h, keygen).status, status::IO);
    assert!(h.store_mut().files.is_empty());
}

#[test]
fn cc_ring_round_trip_and_refusals() {
    let mut rng = urandom();
    let mut r = Ring::new(CryptoCore);
    let ring = r.create(OWNER, b"correct horse battery staple", KdfParams::FLOOR, &mut rng).unwrap();
    assert_eq!(ring[5], format::SUITE_ARGON2ID_CHACHA20POLY1305);
    let meta = Meta { created: 1_790_000_000, kind: "api-key".into(), label: "Claude".into() };
    let file = r.seal_secret("vein", "claude.api_key", &meta, b"sk-ant-REAL", &mut rng).unwrap();
    r.lock();
    assert_eq!(r.unlock(&ring, b"wrong"), Err(RingError::BadPassword));
    r.unlock(&ring, b"correct horse battery staple").unwrap();
    let (m, pt) = r.open_secret("vein", "claude.api_key", &file).unwrap();
    assert_eq!((m, pt.expose()), (meta.clone(), &b"sk-ant-REAL"[..]));
    for i in [0usize, 5, 20, 48, 61, 70, file.len() - 1] {
        let mut f = file.clone();
        f[i] ^= 0x80;
        assert!(r.open_secret("vein", "claude.api_key", &f).is_err(), "flip {i}");
    }
    assert_eq!(r.open_secret("vein", "other", &file).err(), Some(RingError::Auth));
    // A TEST-suite ring is refused by the production suite before any KDF runs.
    let mut t = Ring::new(TestSealer);
    let test_ring = t.create(OWNER, b"pw", KdfParams::DEFAULT, &mut TestEntropy::default()).unwrap();
    assert_eq!(Ring::new(CryptoCore).unlock(&test_ring, b"pw"), Err(RingError::Suite));
}
