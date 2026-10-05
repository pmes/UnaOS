// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! GF(2^255 - 19), radix 2^51: five 51-bit limbs in `u64`, products in `u128`. Shared by X25519 and
//! Ed25519.
//!
//! Invariant: every value leaving an operation has limbs below 2^52 (one carry pass after each op),
//! so no product or sum can overflow. Encoding is fully reduced (`to_bytes`); decoding ignores bit 255
//! (RFC 7748 §5) — Ed25519 adds its own canonicity check on top.
//!
//! CONSTANT-TIME: every operation — no branch or index on limb values; `invert`/`pow22523` are fixed
//! square-and-multiply chains over PUBLIC exponents.

use crate::ct::Choice;

const M51: u64 = (1 << 51) - 1;

#[derive(Clone, Copy, Debug)]
pub(crate) struct Fe(pub [u64; 5]);

impl Fe {
    pub const ZERO: Fe = Fe([0; 5]);
    pub const ONE: Fe = Fe([1, 0, 0, 0, 0]);

    #[allow(dead_code)]
    pub fn from_u64(x: u64) -> Fe {
        Fe([x & M51, x >> 51, 0, 0, 0])
    }

    pub fn from_bytes(b: &[u8; 32]) -> Fe {
        let ld = |i: usize| u64::from_le_bytes(b[i..i + 8].try_into().unwrap());
        Fe([
            ld(0) & M51,
            (ld(6) >> 3) & M51,
            (ld(12) >> 6) & M51,
            (ld(19) >> 1) & M51,
            (ld(24) >> 12) & M51,
        ])
    }

    #[inline(always)]
    fn carry(mut l: [u64; 5]) -> Fe {
        let c = l[0] >> 51;
        l[0] &= M51;
        l[1] += c;
        let c = l[1] >> 51;
        l[1] &= M51;
        l[2] += c;
        let c = l[2] >> 51;
        l[2] &= M51;
        l[3] += c;
        let c = l[3] >> 51;
        l[3] &= M51;
        l[4] += c;
        let c = l[4] >> 51;
        l[4] &= M51;
        l[0] += c * 19;
        Fe(l)
    }

    /// Fully reduced little-endian encoding.
    pub fn to_bytes(&self) -> [u8; 32] {
        let mut h = Self::carry(Self::carry(self.0).0).0;
        // h < 2^255 + small; subtract p when h >= p: q = 1 iff h + 19 >= 2^255.
        let mut q = (h[0] + 19) >> 51;
        q = (h[1] + q) >> 51;
        q = (h[2] + q) >> 51;
        q = (h[3] + q) >> 51;
        q = (h[4] + q) >> 51;
        h[0] += 19 * q;
        let c = h[0] >> 51;
        h[0] &= M51;
        h[1] += c;
        let c = h[1] >> 51;
        h[1] &= M51;
        h[2] += c;
        let c = h[2] >> 51;
        h[2] &= M51;
        h[3] += c;
        let c = h[3] >> 51;
        h[3] &= M51;
        h[4] += c;
        h[4] &= M51;
        let mut o = [0u8; 32];
        let w0 = h[0] | (h[1] << 51);
        let w1 = (h[1] >> 13) | (h[2] << 38);
        let w2 = (h[2] >> 26) | (h[3] << 25);
        let w3 = (h[3] >> 39) | (h[4] << 12);
        o[0..8].copy_from_slice(&w0.to_le_bytes());
        o[8..16].copy_from_slice(&w1.to_le_bytes());
        o[16..24].copy_from_slice(&w2.to_le_bytes());
        o[24..32].copy_from_slice(&w3.to_le_bytes());
        o
    }

    pub fn add(&self, b: &Fe) -> Fe {
        let a = &self.0;
        let b = &b.0;
        Self::carry([a[0] + b[0], a[1] + b[1], a[2] + b[2], a[3] + b[3], a[4] + b[4]])
    }

    /// `a - b`, adding 4p first so no limb underflows (limbs < 2^52 < 4p's limbs).
    pub fn sub(&self, b: &Fe) -> Fe {
        let a = &self.0;
        let b = &b.0;
        const P4_0: u64 = 4 * ((1 << 51) - 19);
        const P4_I: u64 = 4 * ((1 << 51) - 1);
        Self::carry([a[0] + P4_0 - b[0], a[1] + P4_I - b[1], a[2] + P4_I - b[2], a[3] + P4_I - b[3], a[4] + P4_I - b[4]])
    }

    pub fn neg(&self) -> Fe {
        Fe::ZERO.sub(self)
    }

    pub fn mul(&self, b: &Fe) -> Fe {
        let a = self.0.map(|x| x as u128);
        let b = b.0.map(|x| x as u128);
        let (b1_19, b2_19, b3_19, b4_19) = (b[1] * 19, b[2] * 19, b[3] * 19, b[4] * 19);
        let r0 = a[0] * b[0] + a[1] * b4_19 + a[2] * b3_19 + a[3] * b2_19 + a[4] * b1_19;
        let r1 = a[0] * b[1] + a[1] * b[0] + a[2] * b4_19 + a[3] * b3_19 + a[4] * b2_19;
        let r2 = a[0] * b[2] + a[1] * b[1] + a[2] * b[0] + a[3] * b4_19 + a[4] * b3_19;
        let r3 = a[0] * b[3] + a[1] * b[2] + a[2] * b[1] + a[3] * b[0] + a[4] * b4_19;
        let r4 = a[0] * b[4] + a[1] * b[3] + a[2] * b[2] + a[3] * b[1] + a[4] * b[0];
        Self::carry_wide([r0, r1, r2, r3, r4])
    }

    #[inline(always)]
    fn carry_wide(r: [u128; 5]) -> Fe {
        let m = M51 as u128;
        let c = r[0] >> 51;
        let l0 = (r[0] & m) as u64;
        let r1 = r[1] + c;
        let c = r1 >> 51;
        let l1 = (r1 & m) as u64;
        let r2 = r[2] + c;
        let c = r2 >> 51;
        let l2 = (r2 & m) as u64;
        let r3 = r[3] + c;
        let c = r3 >> 51;
        let l3 = (r3 & m) as u64;
        let r4 = r[4] + c;
        let c = (r4 >> 51) as u64;
        let l4 = (r4 & m) as u64;
        let l0 = l0 + c * 19;
        let c = l0 >> 51;
        Fe([l0 & M51, l1 + c, l2, l3, l4])
    }

    pub fn square(&self) -> Fe {
        self.mul(self)
    }

    pub fn mul_small(&self, k: u32) -> Fe {
        let k = k as u128;
        let a = self.0.map(|x| x as u128 * k);
        Self::carry_wide(a)
    }

    /// `self^e` for a PUBLIC exponent given as little-endian bytes.
    fn pow_bytes(&self, e: &[u8; 32]) -> Fe {
        let mut r = Fe::ONE;
        for i in (0..256).rev() {
            r = r.square();
            if (e[i / 8] >> (i % 8)) & 1 == 1 {
                r = r.mul(self);
            }
        }
        r
    }

    /// `self^(p-2)` = `1/self` (0 maps to 0).
    pub fn invert(&self) -> Fe {
        // p - 2 = 2^255 - 21
        let mut e = [0xffu8; 32];
        e[0] = 0xeb;
        e[31] = 0x7f;
        self.pow_bytes(&e)
    }

    /// `self^((p-5)/8)` = `self^(2^252 - 3)` (square-root helper, RFC 8032 §5.1.3).
    pub fn pow22523(&self) -> Fe {
        let mut e = [0xffu8; 32];
        e[0] = 0xfd;
        e[31] = 0x0f;
        self.pow_bytes(&e)
    }

    pub fn is_zero(&self) -> Choice {
        crate::ct::ct_eq_choice(&self.to_bytes(), &[0u8; 32])
    }
    pub fn ct_eq(&self, b: &Fe) -> Choice {
        crate::ct::ct_eq_choice(&self.to_bytes(), &b.to_bytes())
    }
    /// The low bit of the canonical encoding ("negative" in RFC 8032 terms).
    pub fn is_negative(&self) -> Choice {
        Choice::from_u8(self.to_bytes()[0] & 1)
    }
    pub fn select(c: Choice, a: &Fe, b: &Fe) -> Fe {
        let m = c.mask_u64();
        let mut r = [0u64; 5];
        for i in 0..5 {
            r[i] = (a.0[i] & m) | (b.0[i] & !m);
        }
        Fe(r)
    }
    pub fn cswap(c: Choice, a: &mut Fe, b: &mut Fe) {
        crate::ct::ct_swap_u64(c, &mut a.0, &mut b.0);
    }
}
