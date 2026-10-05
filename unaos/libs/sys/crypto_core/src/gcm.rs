// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! AES-GCM (NIST SP 800-38D) over the bitsliced [`crate::aes::Aes`], with a constant-time GHASH.
//!
//! GHASH multiplies in GF(2^128) mod `x^128 + x^7 + x^2 + x + 1`. GCM's bit order is reflected (the
//! first bit of a block is the coefficient of x^0), so a block is loaded big-endian and bit-reversed
//! into a "normal" polynomial (bit i = coefficient of x^i), multiplied carry-lessly, reduced, and
//! reversed back only for the tag.
//!
//! The carry-less multiply is built from INTEGER multiplications with holes (the technique of Thomas
//! Pornin's BearSSL `ghash_ctmul`): each 32-bit operand is split into four masks keeping every fourth bit,
//! so in any single integer product a column receives at most 8 one-bits — a sum that fits in the 3-bit
//! gap before the next kept column, so no carry ever reaches a kept bit. Sixteen such 32x32→64
//! products give a 64x64 carry-less product, Karatsuba-free schoolbook gives 128x128.
//!
//! CONSTANT-TIME: yes — no table (the 4-bit/8-bit "Shoup" tables most GCMs use are secret-indexed by
//! H and leak it through the cache), no secret-dependent branch; integer multiplication on x86_64 and
//! AArch64 is constant-time. The tag compare is [`crate::ct::ct_eq`]. On a failed tag the buffer stays
//! ciphertext. Lengths (and the IV length) are public.

use crate::aes::Aes;
use crate::ct::{ct_eq, Zeroize};
use crate::Error;

#[inline(always)]
fn bmul32(x: u32, y: u32) -> u64 {
    const M0: u64 = 0x1111_1111_1111_1111;
    const M1: u64 = 0x2222_2222_2222_2222;
    const M2: u64 = 0x4444_4444_4444_4444;
    const M3: u64 = 0x8888_8888_8888_8888;
    let x = x as u64;
    let y = y as u64;
    let (x0, x1, x2, x3) = (x & M0, x & M1, x & M2, x & M3);
    let (y0, y1, y2, y3) = (y & M0, y & M1, y & M2, y & M3);
    let z0 = (x0.wrapping_mul(y0)) ^ (x1.wrapping_mul(y3)) ^ (x2.wrapping_mul(y2)) ^ (x3.wrapping_mul(y1));
    let z1 = (x0.wrapping_mul(y1)) ^ (x1.wrapping_mul(y0)) ^ (x2.wrapping_mul(y3)) ^ (x3.wrapping_mul(y2));
    let z2 = (x0.wrapping_mul(y2)) ^ (x1.wrapping_mul(y1)) ^ (x2.wrapping_mul(y0)) ^ (x3.wrapping_mul(y3));
    let z3 = (x0.wrapping_mul(y3)) ^ (x1.wrapping_mul(y2)) ^ (x2.wrapping_mul(y1)) ^ (x3.wrapping_mul(y0));
    (z0 & M0) | (z1 & M1) | (z2 & M2) | (z3 & M3)
}

#[inline(always)]
fn clmul64(a: u64, b: u64) -> u128 {
    let (a0, a1) = (a as u32, (a >> 32) as u32);
    let (b0, b1) = (b as u32, (b >> 32) as u32);
    let lo = bmul32(a0, b0) as u128;
    let hi = bmul32(a1, b1) as u128;
    let mid = (bmul32(a0, b1) ^ bmul32(a1, b0)) as u128;
    lo ^ (mid << 32) ^ (hi << 64)
}

/// `a * b mod (x^128 + x^7 + x^2 + x + 1)`, operands in normal bit order.
#[inline(always)]
fn gf128_mul(a: u128, b: u128) -> u128 {
    let (a0, a1) = (a as u64, (a >> 64) as u64);
    let (b0, b1) = (b as u64, (b >> 64) as u64);
    let lo = clmul64(a0, b0);
    let hi = clmul64(a1, b1);
    let mid = clmul64(a0, b1) ^ clmul64(a1, b0);
    let lo = lo ^ (mid << 64);
    let hi = hi ^ (mid >> 64);
    // fold hi * x^128 = hi * (x^7 + x^2 + x + 1)
    let ov = (hi >> 127) ^ (hi >> 126) ^ (hi >> 121);
    let t = hi ^ (hi << 1) ^ (hi << 2) ^ (hi << 7) ^ ov ^ (ov << 1) ^ (ov << 2) ^ (ov << 7);
    lo ^ t
}

#[inline(always)]
fn load(b: &[u8]) -> u128 {
    let mut x = [0u8; 16];
    x[..b.len()].copy_from_slice(b);
    u128::from_be_bytes(x).reverse_bits()
}

/// GHASH accumulator (SP 800-38D §6.4) keyed by H.
#[derive(Clone)]
pub struct Ghash {
    h: u128,
    y: u128,
}

impl Ghash {
    /// Key with `H` (the 16-byte block E(K, 0^128) for GCM).
    pub fn new(h: &[u8; 16]) -> Self {
        Ghash { h: load(h), y: 0 }
    }
    /// Absorb `data`, zero-padding its last partial block (GCM's `0^v` / `0^u`).
    pub fn update_padded(&mut self, data: &[u8]) {
        for c in data.chunks(16) {
            self.y = gf128_mul(self.y ^ load(c), self.h);
        }
    }
    /// The current value as a block.
    pub fn finalize(&self) -> [u8; 16] {
        self.y.reverse_bits().to_be_bytes()
    }
}

impl Drop for Ghash {
    fn drop(&mut self) {
        let mut w = [self.h as u64, (self.h >> 64) as u64, self.y as u64, (self.y >> 64) as u64];
        w.zeroize();
        self.h = 0;
        self.y = 0;
        core::hint::black_box(&self.h);
    }
}

/// AES-GCM with a 128-, 192- or 256-bit key.
#[derive(Clone)]
pub struct AesGcm {
    aes: Aes,
    h: [u8; 16],
}

/// Tag lengths SP 800-38D §5.2.1.2 permits (bytes).
pub fn tag_len_ok(t: usize) -> bool {
    matches!(t, 4 | 8 | 12 | 13 | 14 | 15 | 16)
}

impl AesGcm {
    /// Key the AEAD (16, 24 or 32 bytes).
    pub fn new(key: &[u8]) -> Result<Self, Error> {
        let aes = Aes::new(key)?;
        let mut h = [0u8; 16];
        aes.encrypt_block(&mut h);
        Ok(AesGcm { aes, h })
    }

    fn j0(&self, iv: &[u8]) -> [u8; 16] {
        if iv.len() == 12 {
            let mut j = [0u8; 16];
            j[..12].copy_from_slice(iv);
            j[15] = 1;
            j
        } else {
            let mut g = Ghash::new(&self.h);
            g.update_padded(iv);
            let mut l = [0u8; 16];
            l[8..].copy_from_slice(&((iv.len() as u64) * 8).to_be_bytes());
            g.update_padded(&l);
            g.finalize()
        }
    }

    /// GCTR from counter block `icb` (inc32 on the low 32 bits), four blocks per bitsliced pass.
    fn gctr(&self, icb: &[u8; 16], data: &mut [u8]) {
        let base = u32::from_be_bytes(icb[12..].try_into().unwrap());
        let mut ctr = 0u32;
        for chunk in data.chunks_mut(64) {
            let n = chunk.len().div_ceil(16);
            let mut ks = [[0u8; 16]; 4];
            for (k, b) in ks.iter_mut().take(n).enumerate() {
                b[..12].copy_from_slice(&icb[..12]);
                b[12..].copy_from_slice(&base.wrapping_add(ctr + k as u32).to_be_bytes());
            }
            self.aes.encrypt_blocks(&mut ks[..n]);
            for (i, x) in chunk.iter_mut().enumerate() {
                *x ^= ks[i / 16][i % 16];
            }
            ctr = ctr.wrapping_add(n as u32);
            for b in ks.iter_mut() {
                b.zeroize();
            }
        }
    }

    fn tag(&self, j0: &[u8; 16], aad: &[u8], ct: &[u8]) -> [u8; 16] {
        let mut g = Ghash::new(&self.h);
        g.update_padded(aad);
        g.update_padded(ct);
        let mut l = [0u8; 16];
        l[..8].copy_from_slice(&((aad.len() as u64) * 8).to_be_bytes());
        l[8..].copy_from_slice(&((ct.len() as u64) * 8).to_be_bytes());
        g.update_padded(&l);
        let mut s = g.finalize();
        self.gctr(j0, &mut s);
        s
    }

    fn check(iv: &[u8], len: usize) -> Result<(), Error> {
        // SP 800-38D §5.2.1.1: 1 <= len(IV) <= 2^64 - 1 bits; len(P) <= 2^39 - 256 bits.
        if iv.is_empty() || (len as u64) > (1u64 << 36) - 32 {
            return Err(Error::Length);
        }
        Ok(())
    }

    /// Encrypt `buf` in place under `iv` (any non-empty length; 12 bytes is the fast, recommended
    /// path); returns the full 16-byte tag (truncate it yourself for a shorter tag).
    pub fn encrypt_in_place_detached(&self, iv: &[u8], aad: &[u8], buf: &mut [u8]) -> Result<[u8; 16], Error> {
        Self::check(iv, buf.len())?;
        let j0 = self.j0(iv);
        let mut icb = j0;
        let c = u32::from_be_bytes(icb[12..].try_into().unwrap()).wrapping_add(1);
        icb[12..].copy_from_slice(&c.to_be_bytes());
        self.gctr(&icb, buf);
        Ok(self.tag(&j0, aad, buf))
    }

    /// Verify `tag` (4, 8 or 12–16 bytes) over (aad, ciphertext `buf`), then decrypt `buf` in place.
    pub fn decrypt_in_place_detached(&self, iv: &[u8], aad: &[u8], buf: &mut [u8], tag: &[u8]) -> Result<(), Error> {
        Self::check(iv, buf.len())?;
        if !tag_len_ok(tag.len()) {
            return Err(Error::Length);
        }
        let j0 = self.j0(iv);
        let want = self.tag(&j0, aad, buf);
        if !ct_eq(&want[..tag.len()], tag) {
            return Err(Error::Auth);
        }
        let mut icb = j0;
        let c = u32::from_be_bytes(icb[12..].try_into().unwrap()).wrapping_add(1);
        icb[12..].copy_from_slice(&c.to_be_bytes());
        self.gctr(&icb, buf);
        Ok(())
    }

    /// `ciphertext || tag` as a new vector (12-byte IV recommended).
    #[cfg(feature = "alloc")]
    pub fn seal(&self, iv: &[u8], aad: &[u8], plaintext: &[u8]) -> Result<alloc::vec::Vec<u8>, Error> {
        let mut v = plaintext.to_vec();
        let t = self.encrypt_in_place_detached(iv, aad, &mut v)?;
        v.extend_from_slice(&t);
        Ok(v)
    }

    /// Open `ciphertext || 16-byte tag` into a new vector.
    #[cfg(feature = "alloc")]
    pub fn open(&self, iv: &[u8], aad: &[u8], sealed: &[u8]) -> Result<alloc::vec::Vec<u8>, Error> {
        if sealed.len() < 16 {
            return Err(Error::Length);
        }
        let (ct, tag) = sealed.split_at(sealed.len() - 16);
        let mut v = ct.to_vec();
        self.decrypt_in_place_detached(iv, aad, &mut v, tag)?;
        Ok(v)
    }
}

impl Drop for AesGcm {
    fn drop(&mut self) {
        self.h.zeroize();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bit-by-bit GF(2^128) multiply straight from SP 800-38D Algorithm 1, the oracle for gf128_mul.
    fn mul_ref(x: [u8; 16], y: [u8; 16]) -> [u8; 16] {
        let x = u128::from_be_bytes(x);
        let mut v = u128::from_be_bytes(y);
        let mut z = 0u128;
        for i in 0..128 {
            if (x >> (127 - i)) & 1 == 1 {
                z ^= v;
            }
            let lsb = v & 1;
            v >>= 1;
            if lsb == 1 {
                v ^= 0xe1u128 << 120;
            }
        }
        z.to_be_bytes()
    }

    #[test]
    fn ghash_mul_matches_algorithm_1() {
        let mut s = 0x0123_4567_89ab_cdefu64;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        for _ in 0..2000 {
            let mut a = [0u8; 16];
            let mut b = [0u8; 16];
            a[..8].copy_from_slice(&next().to_le_bytes());
            a[8..].copy_from_slice(&next().to_le_bytes());
            b[..8].copy_from_slice(&next().to_le_bytes());
            b[8..].copy_from_slice(&next().to_le_bytes());
            let got = gf128_mul(load(&a), load(&b)).reverse_bits().to_be_bytes();
            assert_eq!(got, mul_ref(a, b));
        }
        let ones = [0xffu8; 16];
        assert_eq!(gf128_mul(load(&ones), load(&ones)).reverse_bits().to_be_bytes(), mul_ref(ones, ones));
    }
}
