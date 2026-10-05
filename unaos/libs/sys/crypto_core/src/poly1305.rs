// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Poly1305 (RFC 8439 §2.5): the one-time authenticator, radix 2^26 (five 26-bit limbs, 64-bit
//! products — the "donna-32" layout, which keeps every product inside a `u64`).
//!
//! CONSTANT-TIME: yes — no branch or index depends on the key or message; the final reduction mod
//! 2^130-5 selects between `h` and `h - p` with a mask. Message length is public.

use crate::ct::Zeroize;

const M26: u32 = 0x3ff_ffff;

/// A Poly1305 MAC under a one-time 32-byte key (`r || s`).
pub struct Poly1305 {
    r: [u32; 5],
    s: [u32; 4],
    h: [u32; 5],
    buf: [u8; 16],
    used: usize,
}

#[inline(always)]
fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes(b[..4].try_into().unwrap())
}

impl Poly1305 {
    /// Key the MAC. `r` is clamped per §2.5.1.
    pub fn new(key: &[u8; 32]) -> Self {
        let r = [
            le32(&key[0..]) & 0x3ff_ffff,
            (le32(&key[3..]) >> 2) & 0x3ff_ff03,
            (le32(&key[6..]) >> 4) & 0x3ff_c0ff,
            (le32(&key[9..]) >> 6) & 0x3f0_3fff,
            (le32(&key[12..]) >> 8) & 0x00f_ffff,
        ];
        let s = [le32(&key[16..]), le32(&key[20..]), le32(&key[24..]), le32(&key[28..])];
        Poly1305 { r, s, h: [0; 5], buf: [0; 16], used: 0 }
    }

    fn block(&mut self, m: &[u8; 16], hibit: u32) {
        let [r0, r1, r2, r3, r4] = self.r.map(|x| x as u64);
        let (s1, s2, s3, s4) = (r1 * 5, r2 * 5, r3 * 5, r4 * 5);
        let mut h0 = self.h[0] as u64 + (le32(&m[0..]) & M26) as u64;
        let mut h1 = self.h[1] as u64 + ((le32(&m[3..]) >> 2) & M26) as u64;
        let mut h2 = self.h[2] as u64 + ((le32(&m[6..]) >> 4) & M26) as u64;
        let mut h3 = self.h[3] as u64 + ((le32(&m[9..]) >> 6) & M26) as u64;
        let mut h4 = self.h[4] as u64 + ((le32(&m[12..]) >> 8) | hibit) as u64;

        let d0 = h0 * r0 + h1 * s4 + h2 * s3 + h3 * s2 + h4 * s1;
        let mut d1 = h0 * r1 + h1 * r0 + h2 * s4 + h3 * s3 + h4 * s2;
        let mut d2 = h0 * r2 + h1 * r1 + h2 * r0 + h3 * s4 + h4 * s3;
        let mut d3 = h0 * r3 + h1 * r2 + h2 * r1 + h3 * r0 + h4 * s4;
        let mut d4 = h0 * r4 + h1 * r3 + h2 * r2 + h3 * r1 + h4 * r0;

        let mut c = d0 >> 26;
        h0 = d0 & M26 as u64;
        d1 += c;
        c = d1 >> 26;
        h1 = d1 & M26 as u64;
        d2 += c;
        c = d2 >> 26;
        h2 = d2 & M26 as u64;
        d3 += c;
        c = d3 >> 26;
        h3 = d3 & M26 as u64;
        d4 += c;
        c = d4 >> 26;
        h4 = d4 & M26 as u64;
        h0 += c * 5;
        c = h0 >> 26;
        h0 &= M26 as u64;
        h1 += c;
        self.h = [h0 as u32, h1 as u32, h2 as u32, h3 as u32, h4 as u32];
    }

    /// Absorb message bytes.
    pub fn update(&mut self, mut data: &[u8]) {
        if self.used > 0 {
            let n = core::cmp::min(16 - self.used, data.len());
            self.buf[self.used..self.used + n].copy_from_slice(&data[..n]);
            self.used += n;
            data = &data[n..];
            if self.used == 16 {
                let b = self.buf;
                self.block(&b, 1 << 24);
                self.used = 0;
            }
        }
        while data.len() >= 16 {
            self.block(data[..16].try_into().unwrap(), 1 << 24);
            data = &data[16..];
        }
        if !data.is_empty() {
            self.buf[..data.len()].copy_from_slice(data);
            self.used = data.len();
        }
    }

    /// Absorb zeros up to the next 16-byte boundary (the AEAD's `pad16`, §2.8).
    pub fn pad16(&mut self) {
        if self.used > 0 {
            let z = [0u8; 16];
            let n = 16 - self.used;
            self.update(&z[..n]);
        }
    }

    /// The 16-byte tag.
    pub fn finalize(mut self) -> [u8; 16] {
        if self.used > 0 {
            let mut b = [0u8; 16];
            b[..self.used].copy_from_slice(&self.buf[..self.used]);
            b[self.used] = 1;
            self.block(&b, 0);
        }
        let [mut h0, mut h1, mut h2, mut h3, mut h4] = self.h;
        let mut c = h1 >> 26;
        h1 &= M26;
        h2 += c;
        c = h2 >> 26;
        h2 &= M26;
        h3 += c;
        c = h3 >> 26;
        h3 &= M26;
        h4 += c;
        c = h4 >> 26;
        h4 &= M26;
        h0 += c * 5;
        c = h0 >> 26;
        h0 &= M26;
        h1 += c;

        // g = h + 5 - 2^130; take g when it did not borrow (h >= p).
        let mut g0 = h0.wrapping_add(5);
        c = g0 >> 26;
        g0 &= M26;
        let mut g1 = h1.wrapping_add(c);
        c = g1 >> 26;
        g1 &= M26;
        let mut g2 = h2.wrapping_add(c);
        c = g2 >> 26;
        g2 &= M26;
        let mut g3 = h3.wrapping_add(c);
        c = g3 >> 26;
        g3 &= M26;
        let g4 = h4.wrapping_add(c).wrapping_sub(1 << 26);
        let mask = (g4 >> 31).wrapping_sub(1); // all-ones when no borrow
        h0 = (h0 & !mask) | (g0 & mask);
        h1 = (h1 & !mask) | (g1 & mask);
        h2 = (h2 & !mask) | (g2 & mask);
        h3 = (h3 & !mask) | (g3 & mask);
        h4 = (h4 & !mask) | (g4 & mask);

        let w0 = h0 | (h1 << 26);
        let w1 = (h1 >> 6) | (h2 << 20);
        let w2 = (h2 >> 12) | (h3 << 14);
        let w3 = (h3 >> 18) | (h4 << 8);
        let mut f = w0 as u64 + self.s[0] as u64;
        let t0 = f as u32;
        f = w1 as u64 + self.s[1] as u64 + (f >> 32);
        let t1 = f as u32;
        f = w2 as u64 + self.s[2] as u64 + (f >> 32);
        let t2 = f as u32;
        f = w3 as u64 + self.s[3] as u64 + (f >> 32);
        let t3 = f as u32;
        let mut out = [0u8; 16];
        out[0..4].copy_from_slice(&t0.to_le_bytes());
        out[4..8].copy_from_slice(&t1.to_le_bytes());
        out[8..12].copy_from_slice(&t2.to_le_bytes());
        out[12..16].copy_from_slice(&t3.to_le_bytes());
        out
    }
}

impl Drop for Poly1305 {
    fn drop(&mut self) {
        self.r.zeroize();
        self.s.zeroize();
        self.h.zeroize();
        self.buf.zeroize();
    }
}

/// One-shot Poly1305.
pub fn poly1305(key: &[u8; 32], msg: &[u8]) -> [u8; 16] {
    let mut p = Poly1305::new(key);
    p.update(msg);
    p.finalize()
}
