// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! SHA-1 (FIPS 180-4 §6.1) — FOR IDENTIFIERS ONLY, never for a signature or a MAC.
//!
//! TLSCORE2 (SR58) needs it for exactly one job: an OCSP `CertID` (RFC 6960 §4.1.1) names the certificate by
//! `issuerNameHash`/`issuerKeyHash`, and the responders the Web PKI staples (and OpenSSL's own `ocsp` tool by
//! default) compute those with SHA-1. Matching a CertID is a lookup, not a security decision — the response's
//! signature is checked with SHA-2 — so the collision weakness of SHA-1 does not reach a trust decision here.
//! tls_core never accepts a SHA-1 *signature* (its `SignatureAlgorithm` has no SHA-1 arm).
//!
//! CONSTANT-TIME: yes in the message contents (fixed-schedule ARX, public round index); length public.

use crate::sha2::Digest;

const H0: [u32; 5] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0];

/// A streaming SHA-1 state.
#[derive(Clone)]
pub struct Sha1 {
    h: [u32; 5],
    buf: [u8; 64],
    buf_len: usize,
    total: u64,
}

impl Sha1 {
    fn compress(h: &mut [u32; 5], block: &[u8]) {
        let mut w = [0u32; 80];
        for t in 0..16 {
            w[t] = u32::from_be_bytes([block[4 * t], block[4 * t + 1], block[4 * t + 2], block[4 * t + 3]]);
        }
        for t in 16..80 {
            w[t] = (w[t - 3] ^ w[t - 8] ^ w[t - 14] ^ w[t - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (t, wt) in w.iter().enumerate() {
            // §4.1.1 f_t and §4.2.1 K_t, selected by the public round number.
            let (f, k) = match t / 20 {
                0 => ((b & c) | (!b & d), 0x5a827999),
                1 => (b ^ c ^ d, 0x6ed9eba1),
                2 => ((b & c) | (b & d) | (c & d), 0x8f1bbcdc),
                _ => (b ^ c ^ d, 0xca62c1d6),
            };
            let tmp = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wt);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = tmp;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }

    /// The 20-byte digest.
    pub fn finalize(self) -> [u8; 20] {
        let mut o = [0u8; 20];
        Digest::finalize_into(self, &mut o);
        o
    }
}

impl Digest for Sha1 {
    const BLOCK_LEN: usize = 64;
    const OUTPUT_LEN: usize = 20;
    fn new() -> Self {
        Sha1 { h: H0, buf: [0; 64], buf_len: 0, total: 0 }
    }
    fn update(&mut self, mut data: &[u8]) {
        self.total = self.total.wrapping_add(data.len() as u64);
        if self.buf_len > 0 {
            let n = (64 - self.buf_len).min(data.len());
            self.buf[self.buf_len..self.buf_len + n].copy_from_slice(&data[..n]);
            self.buf_len += n;
            data = &data[n..];
            if self.buf_len == 64 {
                let b = self.buf;
                Self::compress(&mut self.h, &b);
                self.buf_len = 0;
            }
        }
        while data.len() >= 64 {
            Self::compress(&mut self.h, &data[..64]);
            data = &data[64..];
        }
        self.buf[..data.len()].copy_from_slice(data);
        self.buf_len += data.len();
    }
    fn finalize_into(mut self, out: &mut [u8]) {
        // §5.1.1 padding: 0x80, zeros, the 64-bit big-endian bit length.
        let bits = self.total.wrapping_mul(8);
        let mut pad = [0u8; 72];
        pad[0] = 0x80;
        let zeros = if self.buf_len < 56 { 56 - self.buf_len } else { 120 - self.buf_len };
        pad[zeros..zeros + 8].copy_from_slice(&bits.to_be_bytes());
        let total = self.total;
        self.update(&pad[..zeros + 8]);
        self.total = total;
        for (i, v) in self.h.iter().enumerate() {
            out[4 * i..4 * i + 4].copy_from_slice(&v.to_be_bytes());
        }
    }
}

/// One-shot SHA-1.
pub fn sha1(data: &[u8]) -> [u8; 20] {
    let mut h = <Sha1 as Digest>::new();
    h.update(data);
    h.finalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn hex(b: &[u8]) -> alloc_free::Hex<'_> {
        alloc_free::Hex(b)
    }
    mod alloc_free {
        pub struct Hex<'a>(pub &'a [u8]);
        impl PartialEq<&str> for Hex<'_> {
            fn eq(&self, o: &&str) -> bool {
                let o = o.as_bytes();
                o.len() == self.0.len() * 2
                    && self.0.iter().enumerate().all(|(i, b)| {
                        let d = |c: u8| (c as char).to_digit(16).unwrap() as u8;
                        (d(o[2 * i]) << 4 | d(o[2 * i + 1])) == *b
                    })
            }
        }
    }

    /// FIPS 180-4 examples (NIST CSRC "SHA1.pdf"): "abc", the 448-bit two-block message, one million 'a'.
    #[test]
    fn fips_180_4_vectors() {
        assert!(hex(&sha1(b"abc")) == "a9993e364706816aba3e25717850c26c9cd0d89d");
        assert!(hex(&sha1(b"")) == "da39a3ee5e6b4b0d3255bfef95601890afd80709");
        assert!(
            hex(&sha1(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"))
                == "84983e441c3bd26ebaae4aa1f95129e5e54670f1"
        );
        let mut h = <Sha1 as Digest>::new();
        for _ in 0..1000 {
            h.update(&[b'a'; 1000]);
        }
        assert!(hex(&h.finalize()) == "34aa973cd4c4daa4f61eeb2bdbad27316534016f");
        // Split at every offset of a 3-block message: streaming equals one-shot.
        let msg: [u8; 150] = core::array::from_fn(|i| i as u8);
        let one = sha1(&msg);
        for cut in 0..msg.len() {
            let mut s = <Sha1 as Digest>::new();
            s.update(&msg[..cut]);
            s.update(&msg[cut..]);
            assert_eq!(s.finalize(), one);
        }
    }
}
