// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! SHA-1 (FIPS 180-4 §4.1.1, §4.2.1, §5.3.1, §6.1). GITCORE (LEDGER SR59) needs it: a git object id
//! in the default object format is the SHA-1 of `"<type> <len>\0" + payload`.
//!
//! SHA-1 is BROKEN as a collision-resistant hash (SHAttered, 2017; chosen-prefix, 2020). It is here
//! for interoperability with the git object format only, never for a new security decision — RSA
//! verification in this crate still refuses SHA-1 signatures. Git itself hashes with the
//! collision-DETECTING variant (Stevens–Shumow `sha1dc`); this module is plain SHA-1, which gives
//! the identical digest for every input that is not a crafted near-collision block. The detector is
//! OWED (docs/dev/evidence/vcs-1005/GITCORE.md, "ceiling").
//!
//! Streaming, no allocation. CONSTANT-TIME in the message contents (fixed-schedule ARX); the
//! length is public.

use crate::ct::Zeroize;
use crate::sha2::Digest;

const H0: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];

/// SHA-1. Streaming; `Clone` forks a state.
#[derive(Clone)]
pub struct Sha1 {
    h: [u32; 5],
    buf: [u8; 64],
    buf_len: usize,
    total: u64,
}

impl Sha1 {
    /// A fresh state.
    pub const fn new() -> Self {
        Sha1 { h: H0, buf: [0; 64], buf_len: 0, total: 0 }
    }

    /// Absorb `data`.
    pub fn update(&mut self, mut data: &[u8]) {
        self.total = self.total.wrapping_add(data.len() as u64);
        if self.buf_len > 0 {
            let take = core::cmp::min(64 - self.buf_len, data.len());
            self.buf[self.buf_len..self.buf_len + take].copy_from_slice(&data[..take]);
            self.buf_len += take;
            data = &data[take..];
            if self.buf_len == 64 {
                let block = self.buf;
                compress(&mut self.h, &block);
                self.buf_len = 0;
            }
        }
        while data.len() >= 64 {
            compress(&mut self.h, data[..64].try_into().unwrap());
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    /// The 20-byte digest (§5.1.1 padding: 0x80, zeros, 64-bit big-endian bit length).
    pub fn finalize(mut self) -> [u8; 20] {
        let bit_len = self.total.wrapping_mul(8);
        let n = self.buf_len;
        self.buf[n] = 0x80;
        for b in &mut self.buf[n + 1..] {
            *b = 0;
        }
        if n >= 56 {
            let block = self.buf;
            compress(&mut self.h, &block);
            self.buf = [0; 64];
        }
        self.buf[56..].copy_from_slice(&bit_len.to_be_bytes());
        let block = self.buf;
        compress(&mut self.h, &block);
        let mut out = [0u8; 20];
        for (i, w) in self.h.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_be_bytes());
        }
        out
    }
}

impl Default for Sha1 {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Sha1 {
    fn drop(&mut self) {
        self.h.zeroize();
        self.buf.zeroize();
    }
}

impl Digest for Sha1 {
    const BLOCK_LEN: usize = 64;
    const OUTPUT_LEN: usize = 20;
    fn new() -> Self {
        Sha1::new()
    }
    fn update(&mut self, data: &[u8]) {
        Sha1::update(self, data)
    }
    fn finalize_into(self, out: &mut [u8]) {
        out[..20].copy_from_slice(&self.finalize());
    }
}

/// One-shot SHA-1.
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h = Sha1::new();
    h.update(data);
    h.finalize()
}

/// §6.1.2: the 80-step compression with the §4.1.1 functions Ch / Parity / Maj.
#[inline(always)]
fn compress(h: &mut [u32; 5], block: &[u8; 64]) {
    let mut w = [0u32; 80];
    for i in 0..16 {
        w[i] = u32::from_be_bytes(block[i * 4..i * 4 + 4].try_into().unwrap());
    }
    for i in 16..80 {
        w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
    }
    let [mut a, mut b, mut c, mut d, mut e] = *h;
    for (i, wi) in w.iter().enumerate() {
        let (f, k) = match i {
            0..=19 => ((b & c) | ((!b) & d), 0x5A827999),
            20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
            40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
            _ => (b ^ c ^ d, 0xCA62C1D6),
        };
        let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
        e = d;
        d = c;
        c = b.rotate_left(30);
        b = a;
        a = t;
    }
    for (x, y) in h.iter_mut().zip([a, b, c, d, e]) {
        *x = x.wrapping_add(y);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(d: &[u8]) -> [u8; 40] {
        let mut o = [0u8; 40];
        for (i, b) in d.iter().enumerate() {
            o[2 * i] = b"0123456789abcdef"[(b >> 4) as usize];
            o[2 * i + 1] = b"0123456789abcdef"[(b & 15) as usize];
        }
        o
    }

    /// FIPS 180-4 / NIST CSRC example values (SHA1.pdf): one-block, two-block, and a million 'a'.
    #[test]
    fn fips_examples() {
        assert_eq!(&hex(&sha1(b"abc")), b"a9993e364706816aba3e25717850c26c9cd0d89d");
        assert_eq!(
            &hex(&sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq")),
            b"84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        assert_eq!(&hex(&sha1(b"")), b"da39a3ee5e6b4b0d3255bfef95601890afd80709");
        let mut h = Sha1::new();
        for _ in 0..1000 {
            h.update(&[b'a'; 1000]);
        }
        assert_eq!(&hex(&h.finalize()), b"34aa973cd4c4daa4f61eeb2bdbad27316534016f");
        // git's empty blob: "blob 0\0".
        assert_eq!(&hex(&sha1(b"blob 0\0")), b"e69de29bb2d1d6434b8b29ae775ad8c2e48c5391");
    }

    /// Streaming split at every offset agrees with one-shot.
    #[test]
    fn streaming_splits() {
        let msg: [u8; 200] = core::array::from_fn(|i| (i * 7 + 3) as u8);
        let want = sha1(&msg);
        for cut in 0..msg.len() {
            let mut h = Sha1::new();
            h.update(&msg[..cut]);
            h.update(&msg[cut..]);
            assert_eq!(h.finalize(), want);
        }
    }
}
