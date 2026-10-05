// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! SHA-3 and SHAKE (FIPS 202) — Keccak-f[1600] and the sponge (CTCORE, SR60: ML-KEM's H, G, J, PRF, XOF).
//!
//! * [`keccak_f1600`] — FIPS 202 §3.3 (θ, ρ, π, χ, ι over 24 rounds), lanes as `u64` little-endian (§B.1).
//! * [`Sha3_224`], [`Sha3_256`], [`Sha3_384`], [`Sha3_512`] — §6.1, domain suffix `01` (pad byte 0x06).
//! * [`Shake128`], [`Shake256`] — §6.2, suffix `1111` (pad byte 0x1f), absorb then squeeze any length.
//!
//! Constant-time: the permutation is a fixed sequence of XOR / AND-NOT / rotate on the whole state — no branch
//! and no memory index depends on the data; the sponge's control flow depends only on lengths (public).

const RC: [u64; 24] = [
    0x0000000000000001, 0x0000000000008082, 0x800000000000808a, 0x8000000080008000,
    0x000000000000808b, 0x0000000080000001, 0x8000000080008081, 0x8000000000008009,
    0x000000000000008a, 0x0000000000000088, 0x0000000080008009, 0x000000008000000a,
    0x000000008000808b, 0x800000000000008b, 0x8000000000008089, 0x8000000000008003,
    0x8000000000008002, 0x8000000000000080, 0x000000000000800a, 0x800000008000000a,
    0x8000000080008081, 0x8000000000008080, 0x0000000080000001, 0x8000000080008008,
];
/// ρ offsets, indexed by lane x + 5y.
const RHO: [u32; 25] = [0, 1, 62, 28, 27, 36, 44, 6, 55, 20, 3, 10, 43, 25, 39, 41, 45, 15, 21, 8, 18, 2, 61, 56, 14];

/// Keccak-f[1600] (FIPS 202 §3.3, Algorithm 7) on 25 lanes, `a[x + 5y]`.
pub fn keccak_f1600(a: &mut [u64; 25]) {
    for rc in RC {
        // θ
        let mut c = [0u64; 5];
        for x in 0..5 {
            c[x] = a[x] ^ a[x + 5] ^ a[x + 10] ^ a[x + 15] ^ a[x + 20];
        }
        for x in 0..5 {
            let d = c[(x + 4) % 5] ^ c[(x + 1) % 5].rotate_left(1);
            for y in 0..5 {
                a[x + 5 * y] ^= d;
            }
        }
        // ρ and π: B[y, 2x+3y] = rot(A[x, y], r[x, y])
        let mut b = [0u64; 25];
        for x in 0..5 {
            for y in 0..5 {
                b[y + 5 * ((2 * x + 3 * y) % 5)] = a[x + 5 * y].rotate_left(RHO[x + 5 * y]);
            }
        }
        // χ
        for y in 0..5 {
            for x in 0..5 {
                a[x + 5 * y] = b[x + 5 * y] ^ (!b[(x + 1) % 5 + 5 * y] & b[(x + 2) % 5 + 5 * y]);
            }
        }
        // ι
        a[0] ^= rc;
    }
}

/// A Keccak sponge with rate `R` bytes and domain-separation pad byte `PAD`.
#[derive(Clone)]
pub struct Sponge<const R: usize, const PAD: u8> {
    st: [u64; 25],
    pos: usize,
    squeezing: bool,
}

impl<const R: usize, const PAD: u8> Default for Sponge<R, PAD> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const R: usize, const PAD: u8> Sponge<R, PAD> {
    /// A fresh sponge.
    pub const fn new() -> Self {
        Sponge { st: [0; 25], pos: 0, squeezing: false }
    }
    fn xor_byte(&mut self, i: usize, b: u8) {
        self.st[i / 8] ^= (b as u64) << (8 * (i % 8));
    }
    fn byte(&self, i: usize) -> u8 {
        (self.st[i / 8] >> (8 * (i % 8))) as u8
    }
    /// Absorb more input (only before the first squeeze).
    pub fn update(&mut self, data: &[u8]) {
        assert!(!self.squeezing, "sha3: absorb after squeeze");
        for &b in data {
            self.xor_byte(self.pos, b);
            self.pos += 1;
            if self.pos == R {
                keccak_f1600(&mut self.st);
                self.pos = 0;
            }
        }
    }
    fn finish_absorb(&mut self) {
        if !self.squeezing {
            // pad10*1 with the domain bits folded into PAD (FIPS 202 §5.1, §B.2).
            self.xor_byte(self.pos, PAD);
            self.xor_byte(R - 1, 0x80);
            keccak_f1600(&mut self.st);
            self.pos = 0;
            self.squeezing = true;
        }
    }
    /// Squeeze `out.len()` more bytes (an XOF may be squeezed repeatedly).
    pub fn squeeze(&mut self, out: &mut [u8]) {
        self.finish_absorb();
        for o in out.iter_mut() {
            if self.pos == R {
                keccak_f1600(&mut self.st);
                self.pos = 0;
            }
            *o = self.byte(self.pos);
            self.pos += 1;
        }
    }
}

impl<const R: usize, const PAD: u8> Drop for Sponge<R, PAD> {
    fn drop(&mut self) {
        for l in self.st.iter_mut() {
            *l = 0;
        }
        core::hint::black_box(&self.st);
    }
}

/// SHA3-224 (rate 144).
pub type Sha3_224 = Sponge<144, 0x06>;
/// SHA3-256 (rate 136).
pub type Sha3_256 = Sponge<136, 0x06>;
/// SHA3-384 (rate 104).
pub type Sha3_384 = Sponge<104, 0x06>;
/// SHA3-512 (rate 72).
pub type Sha3_512 = Sponge<72, 0x06>;
/// SHAKE128 (rate 168).
pub type Shake128 = Sponge<168, 0x1f>;
/// SHAKE256 (rate 136).
pub type Shake256 = Sponge<136, 0x1f>;

/// One-shot SHA3-224.
pub fn sha3_224(m: &[u8]) -> [u8; 28] {
    let mut s = Sha3_224::new();
    s.update(m);
    let mut o = [0u8; 28];
    s.squeeze(&mut o);
    o
}
/// One-shot SHA3-256 (FIPS 203's H).
pub fn sha3_256(m: &[u8]) -> [u8; 32] {
    let mut s = Sha3_256::new();
    s.update(m);
    let mut o = [0u8; 32];
    s.squeeze(&mut o);
    o
}
/// One-shot SHA3-384.
pub fn sha3_384(m: &[u8]) -> [u8; 48] {
    let mut s = Sha3_384::new();
    s.update(m);
    let mut o = [0u8; 48];
    s.squeeze(&mut o);
    o
}
/// One-shot SHA3-512 (FIPS 203's G).
pub fn sha3_512(m: &[u8]) -> [u8; 64] {
    let mut s = Sha3_512::new();
    s.update(m);
    let mut o = [0u8; 64];
    s.squeeze(&mut o);
    o
}
/// One-shot SHAKE128 into `out`.
pub fn shake128(m: &[u8], out: &mut [u8]) {
    let mut s = Shake128::new();
    s.update(m);
    s.squeeze(out);
}
/// One-shot SHAKE256 into `out`.
pub fn shake256(m: &[u8], out: &mut [u8]) {
    let mut s = Shake256::new();
    s.update(m);
    s.squeeze(out);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fips202_empty() {
        // NIST's published digests of the empty message (first bytes).
        assert_eq!(sha3_224(b"")[..4], [0x6b, 0x4e, 0x03, 0x42]);
        assert_eq!(sha3_256(b"")[..4], [0xa7, 0xff, 0xc6, 0xf8]);
        assert_eq!(sha3_384(b"")[..4], [0x0c, 0x63, 0xa7, 0x5b]);
        assert_eq!(sha3_512(b"")[..4], [0xa6, 0x9f, 0x73, 0xcc]);
        let mut o = [0u8; 4];
        shake128(b"", &mut o);
        assert_eq!(o, [0x7f, 0x9c, 0x2b, 0xa4]);
        shake256(b"", &mut o);
        assert_eq!(o, [0x46, 0xb9, 0xdd, 0x2b]);
    }
}
