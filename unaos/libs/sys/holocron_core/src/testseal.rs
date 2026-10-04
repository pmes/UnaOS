// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The TEST suite: a deterministic stand-in for CRYPTOCORE so the format, the ring and the bus surface
//! can be built and proven before SR27 folds.
//!
//! ⚠ **NOT CRYPTOGRAPHY.** A SplitMix64-based sponge with no security claim of any kind: the "KDF" is
//! cheap, the "cipher" is a keyed XOR stream, the "tag" is a keyed checksum and the "signature" can be
//! forged by anyone holding the public key. It exists so that a wrong password, a flipped byte, a
//! renamed file or a swapped header FAIL the same way they will under the real suite. Its suite byte
//! [`SUITE_TEST_INSECURE`](crate::format::SUITE_TEST_INSECURE) is written into every header it seals,
//! and the production suite refuses such a file outright.

use crate::seal::{Entropy, KdfParams, NONCE_LEN, SALT_LEN, SealError, Sealer, Signer, TAG_LEN};
use crate::zero::{Key, wipe};
use alloc::vec::Vec;

/// SplitMix64 finaliser (Steele, Lea, Flood 2014). A mixer, not a cipher.
fn mix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// A four-lane absorb/squeeze toy sponge over [`mix64`].
struct Toy {
    s: [u64; 4],
    n: u64,
}

impl Toy {
    fn new(domain: &[u8]) -> Self {
        let mut t = Toy { s: [0x486F_6C6F_6372_6F6E, 0x5445_5354_4F4E_4C59, 0, 0], n: 0 };
        t.absorb(domain);
        t
    }
    fn absorb_byte(&mut self, b: u8) {
        let i = (self.n % 4) as usize;
        self.s[i] = mix64(self.s[i] ^ (b as u64) ^ (self.n << 8));
        self.s[(i + 1) % 4] ^= self.s[i].rotate_left(17);
        self.n = self.n.wrapping_add(1);
    }
    fn absorb(&mut self, data: &[u8]) {
        for &b in data {
            self.absorb_byte(b);
        }
        // length framing so absorb("ab")+absorb("c") != absorb("a")+absorb("bc")
        for b in (data.len() as u64).to_le_bytes() {
            self.absorb_byte(b);
        }
    }
    fn squeeze(&mut self, out: &mut [u8]) {
        for chunk in out.chunks_mut(8) {
            self.s[0] = mix64(self.s[0] ^ self.s[1] ^ self.s[2].rotate_left(29) ^ self.s[3].rotate_left(41) ^ self.n);
            self.s.rotate_left(1);
            self.n = self.n.wrapping_add(1);
            let w = self.s[3].to_le_bytes();
            chunk.copy_from_slice(&w[..chunk.len()]);
        }
    }
}

impl Drop for Toy {
    fn drop(&mut self) {
        for w in self.s.iter_mut() {
            let mut b = w.to_le_bytes();
            wipe(&mut b);
            *w = 0;
        }
    }
}

/// The INSECURE test sealer.
#[derive(Clone, Copy, Debug, Default)]
pub struct TestSealer;

impl TestSealer {
    fn tag(key: &Key, nonce: &[u8; NONCE_LEN], aad: &[u8], ct: &[u8]) -> [u8; TAG_LEN] {
        let mut t = Toy::new(b"holocron-test/tag");
        t.absorb(key.bytes());
        t.absorb(nonce);
        t.absorb(aad);
        t.absorb(ct);
        let mut out = [0u8; TAG_LEN];
        t.squeeze(&mut out);
        out
    }
    fn stream(key: &Key, nonce: &[u8; NONCE_LEN], buf: &mut [u8]) {
        let mut t = Toy::new(b"holocron-test/stream");
        t.absorb(key.bytes());
        t.absorb(nonce);
        let mut ks = [0u8; 8];
        for chunk in buf.chunks_mut(8) {
            t.squeeze(&mut ks);
            for (b, k) in chunk.iter_mut().zip(ks.iter()) {
                *b ^= *k;
            }
        }
        wipe(&mut ks);
    }
}

impl Sealer for TestSealer {
    const SUITE: u8 = crate::format::SUITE_TEST_INSECURE;

    fn derive_key(&self, password: &[u8], salt: &[u8; SALT_LEN], params: &KdfParams) -> Result<Key, SealError> {
        if params.t == 0 || params.p == 0 {
            return Err(SealError::Param);
        }
        let mut t = Toy::new(b"holocron-test/kdf");
        t.absorb(password);
        t.absorb(salt);
        t.absorb(&params.m_kib.to_le_bytes());
        t.absorb(&params.t.to_le_bytes());
        t.absorb(&params.p.to_le_bytes());
        // `t` rounds of re-absorbing the squeeze: costs nothing, documents the shape.
        let mut k = [0u8; 32];
        for _ in 0..params.t.min(16) {
            t.squeeze(&mut k);
            t.absorb(&k);
        }
        t.squeeze(&mut k);
        Ok(Key::from_bytes(k))
    }

    fn subkey(&self, key: &Key, salt: &[u8], info: &[u8]) -> Key {
        let mut t = Toy::new(b"holocron-test/subkey");
        t.absorb(key.bytes());
        t.absorb(salt);
        t.absorb(info);
        let mut k = [0u8; 32];
        t.squeeze(&mut k);
        Key::from_bytes(k)
    }

    fn seal(&self, key: &Key, nonce: &[u8; NONCE_LEN], aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(plaintext.len() + TAG_LEN);
        out.extend_from_slice(plaintext);
        Self::stream(key, nonce, &mut out);
        let tag = Self::tag(key, nonce, aad, &out);
        out.extend_from_slice(&tag);
        out
    }

    fn open(&self, key: &Key, nonce: &[u8; NONCE_LEN], aad: &[u8], sealed: &[u8]) -> Result<Vec<u8>, SealError> {
        if sealed.len() < TAG_LEN {
            return Err(SealError::Auth);
        }
        let (ct, tag) = sealed.split_at(sealed.len() - TAG_LEN);
        let want = Self::tag(key, nonce, aad, ct);
        if !crate::ct_eq(&want, tag) {
            return Err(SealError::Auth);
        }
        let mut pt = ct.to_vec();
        Self::stream(key, nonce, &mut pt);
        Ok(pt)
    }
}

/// The INSECURE test signer (anyone with the public key can produce a "valid" signature).
#[derive(Clone, Copy, Debug, Default)]
pub struct TestSigner;

impl Signer for TestSigner {
    const REAL: bool = false;

    fn public_key(&self, seed: &[u8; 32]) -> [u8; 32] {
        let mut t = Toy::new(b"holocron-test/ed25519-pub");
        t.absorb(seed);
        let mut a = [0u8; 32];
        t.squeeze(&mut a);
        a
    }

    fn sign(&self, seed: &[u8; 32], msg: &[u8]) -> [u8; 64] {
        let a = self.public_key(seed);
        let mut t = Toy::new(b"holocron-test/ed25519-sig");
        t.absorb(&a);
        t.absorb(msg);
        let mut s = [0u8; 64];
        t.squeeze(&mut s);
        s
    }

    fn verify(&self, public: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> bool {
        let mut t = Toy::new(b"holocron-test/ed25519-sig");
        t.absorb(public);
        t.absorb(msg);
        let mut s = [0u8; 64];
        t.squeeze(&mut s);
        crate::ct_eq(&s, sig)
    }
}

/// Deterministic "entropy" for tests: a counter through the toy sponge.
#[derive(Clone, Debug, Default)]
pub struct TestEntropy {
    /// Next counter value.
    pub counter: u64,
}

impl Entropy for TestEntropy {
    fn fill(&mut self, buf: &mut [u8]) {
        let mut t = Toy::new(b"holocron-test/entropy");
        t.absorb(&self.counter.to_le_bytes());
        self.counter += 1;
        t.squeeze(buf);
    }
}
