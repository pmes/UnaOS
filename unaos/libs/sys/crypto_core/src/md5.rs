// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! MD5 (RFC 1321). PDFCRYPT (LEDGER SR315): PDF's standard security handler (ISO 32000-1 §7.6.3, Algorithms 1, 2, 3,
//! 5 and 7) derives its file and object keys through MD5, so a reader of encrypted PDF files needs it.
//!
//! LEGACY-ONLY. MD5 is BROKEN (practical collisions since 2004, chosen-prefix since 2007). It is here for reading the
//! formats that are built on it and for nothing else: never a new security decision, never a MAC, never a signature
//! digest. It deliberately does NOT implement [`crate::Digest`], so [`crate::hmac`] and the KDFs cannot be handed it.
//! Its consumers: `pdf_core::crypt` (PDF's standard security handler); MD5-ONE (owed) retires audio_core's FLAC
//! STREAMINFO checksum onto this module.
//!
//! Streaming, no allocation. CONSTANT-TIME in the message contents (fixed-schedule ARX); the length is public.

use crate::ct::Zeroize;

const S0: [u32; 4] = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476];

/// RFC 1321 §3.4: T[i] = floor(2^32 * abs(sin(i + 1))).
const K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

/// The per-round shift amounts (§3.4), four per round.
const R: [u32; 16] = [7, 12, 17, 22, 5, 9, 14, 20, 4, 11, 16, 23, 6, 10, 15, 21];

/// MD5. Streaming; `Clone` forks a state. LEGACY-ONLY (see the module doc).
#[derive(Clone)]
pub struct Md5 {
    s: [u32; 4],
    buf: [u8; 64],
    buf_len: usize,
    total: u64,
}

impl Md5 {
    /// A fresh state.
    pub const fn new() -> Self {
        Md5 { s: S0, buf: [0; 64], buf_len: 0, total: 0 }
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
                compress(&mut self.s, &block);
                self.buf_len = 0;
            }
        }
        while data.len() >= 64 {
            compress(&mut self.s, data[..64].try_into().unwrap());
            data = &data[64..];
        }
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.buf_len = data.len();
        }
    }

    /// The 16-byte digest (§3.1-3.2 padding: 0x80, zeros, the 64-bit LITTLE-endian bit length; §3.5 output order).
    pub fn finalize(mut self) -> [u8; 16] {
        let bit_len = self.total.wrapping_mul(8);
        let n = self.buf_len;
        self.buf[n] = 0x80;
        for b in &mut self.buf[n + 1..] {
            *b = 0;
        }
        if n >= 56 {
            let block = self.buf;
            compress(&mut self.s, &block);
            self.buf = [0; 64];
        }
        self.buf[56..].copy_from_slice(&bit_len.to_le_bytes());
        let block = self.buf;
        compress(&mut self.s, &block);
        let mut out = [0u8; 16];
        for (i, w) in self.s.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        out
    }
}

impl Default for Md5 {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for Md5 {
    fn drop(&mut self) {
        self.s.zeroize();
        self.buf.zeroize();
    }
}

/// One-shot MD5. LEGACY-ONLY (see the module doc).
pub fn md5(data: &[u8]) -> [u8; 16] {
    let mut h = Md5::new();
    h.update(data);
    h.finalize()
}

/// §3.4: the four rounds of sixteen steps with F, G, H, I.
fn compress(s: &mut [u32; 4], block: &[u8; 64]) {
    let mut m = [0u32; 16];
    for (i, w) in m.iter_mut().enumerate() {
        *w = u32::from_le_bytes(block[4 * i..4 * i + 4].try_into().unwrap());
    }
    let [mut a, mut b, mut c, mut d] = *s;
    for i in 0..64 {
        let (f, g) = match i / 16 {
            0 => ((b & c) | (!b & d), i),
            1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
            2 => (b ^ c ^ d, (3 * i + 5) % 16),
            _ => (c ^ (b | !d), (7 * i) % 16),
        };
        let t = a.wrapping_add(f).wrapping_add(K[i]).wrapping_add(m[g]);
        a = d;
        d = c;
        c = b;
        b = b.wrapping_add(t.rotate_left(R[(i / 16) * 4 + i % 4]));
    }
    for (x, y) in s.iter_mut().zip([a, b, c, d]) {
        *x = x.wrapping_add(y);
    }
    m.zeroize();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(d: &[u8; 16]) -> [u8; 32] {
        let mut o = [0u8; 32];
        for (i, b) in d.iter().enumerate() {
            o[2 * i] = b"0123456789abcdef"[(b >> 4) as usize];
            o[2 * i + 1] = b"0123456789abcdef"[(b & 15) as usize];
        }
        o
    }

    /// RFC 1321 Appendix A.5, the test suite.
    #[test]
    fn rfc1321_a5_suite() {
        let cases: [(&[u8], &[u8; 32]); 7] = [
            (b"", b"d41d8cd98f00b204e9800998ecf8427e"),
            (b"a", b"0cc175b9c0f1b6a831c399e269772661"),
            (b"abc", b"900150983cd24fb0d6963f7d28e17f72"),
            (b"message digest", b"f96b697d7cb7938d525a2f31aaf161d0"),
            (b"abcdefghijklmnopqrstuvwxyz", b"c3fcd3d76192e4007dfb496cca67e13b"),
            (b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789", b"d174ab98d277d9f5a5611c2c9f419d9f"),
            (
                b"12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                b"57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];
        for (msg, want) in cases {
            assert_eq!(&hex(&md5(msg)), want);
        }
    }

    /// Streaming split at every offset agrees with one-shot (crosses the 56- and 64-byte padding edges).
    #[test]
    fn streaming_splits() {
        let msg: [u8; 200] = core::array::from_fn(|i| (i * 7 + 3) as u8);
        let want = md5(&msg);
        for cut in 0..msg.len() {
            let mut h = Md5::new();
            h.update(&msg[..cut]);
            h.update(&msg[cut..]);
            assert_eq!(h.finalize(), want);
        }
    }
}
