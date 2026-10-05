// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! 256-bit modular arithmetic in Montgomery form (four 64-bit limbs, little-endian), for any odd modulus
//! below 2^256: the P-256 field prime and group order, and the Ed25519 group order L.
//!
//! Montgomery multiplication is CIOS (Koç, Acar, Kaliski 1996), R = 2^256. Every operation runs the same
//! instruction sequence for every input: the final conditional subtraction is a mask, the carries are
//! arithmetic. The constants R^2 and R^3 mod m and -m^-1 mod 2^64 are computed at COMPILE time
//! (`const fn`), so a modulus is just its limbs.
//!
//! CONSTANT-TIME: yes for `mul`, `add`, `sub`, `neg`, `reduce*`, `select`, `is_zero`; `pow` is constant-
//! time in the BASE (the exponent is public — it is always m-2 or (m+1)/4 here).

use crate::ct::Choice;

pub(crate) type U256 = [u64; 4];

#[inline(always)]
const fn adc(a: u64, b: u64, c: u64) -> (u64, u64) {
    let t = a as u128 + b as u128 + c as u128;
    (t as u64, (t >> 64) as u64)
}
#[inline(always)]
const fn sbb(a: u64, b: u64, borrow: u64) -> (u64, u64) {
    let t = (a as u128).wrapping_sub(b as u128 + (borrow >> 63) as u128);
    (t as u64, (t >> 64) as u64)
}
#[inline(always)]
const fn mac(a: u64, b: u64, c: u64, carry: u64) -> (u64, u64) {
    let t = a as u128 + (b as u128) * (c as u128) + carry as u128;
    (t as u64, (t >> 64) as u64)
}

/// `a - b` and the borrow (all-ones when a < b).
#[inline(always)]
const fn sub4(a: &U256, b: &U256) -> (U256, u64) {
    let (r0, br) = sbb(a[0], b[0], 0);
    let (r1, br) = sbb(a[1], b[1], br);
    let (r2, br) = sbb(a[2], b[2], br);
    let (r3, br) = sbb(a[3], b[3], br);
    ([r0, r1, r2, r3], br)
}

#[inline(always)]
const fn add4(a: &U256, b: &U256) -> (U256, u64) {
    let (r0, c) = adc(a[0], b[0], 0);
    let (r1, c) = adc(a[1], b[1], c);
    let (r2, c) = adc(a[2], b[2], c);
    let (r3, c) = adc(a[3], b[3], c);
    ([r0, r1, r2, r3], c)
}

/// `2x mod m` for `x < m` (compile-time helper).
const fn dbl_mod(x: &U256, m: &U256) -> U256 {
    let (d, carry) = add4(x, x);
    let (s, borrow) = sub4(&d, m);
    // take s when the doubling overflowed or when d >= m
    if carry == 1 || borrow == 0 { s } else { d }
}

/// A modulus with its Montgomery constants.
pub(crate) struct Modulus {
    pub m: U256,
    m0inv: u64,
    pub r2: U256,
    r3: U256,
    pub one: U256, // R mod m (1 in Montgomery form)
}

impl Modulus {
    pub const fn new(m: U256) -> Modulus {
        // -m^-1 mod 2^64 by Newton iteration (each step doubles the correct low bits).
        let mut inv: u64 = 1;
        let mut i = 0;
        while i < 6 {
            inv = inv.wrapping_mul(2u64.wrapping_sub(m[0].wrapping_mul(inv)));
            i += 1;
        }
        let m0inv = inv.wrapping_neg();
        // R mod m, R^2 mod m, R^3 mod m by repeated doubling of 1.
        let mut x: U256 = [1, 0, 0, 0];
        let mut one = [0u64; 4];
        let mut r2 = [0u64; 4];
        let mut k = 0;
        while k < 768 {
            x = dbl_mod(&x, &m);
            k += 1;
            if k == 256 {
                one = x;
            }
            if k == 512 {
                r2 = x;
            }
        }
        Modulus { m, m0inv, r2, r3: x, one }
    }

    /// `a * b * R^-1 mod m`, for `a < 2^256` and `b < m` (output fully reduced).
    #[inline]
    pub fn mul(&self, a: &U256, b: &U256) -> U256 {
        let m = &self.m;
        let mut t = [0u64; 6];
        let mut i = 0;
        while i < 4 {
            let mut c = 0u64;
            let mut j = 0;
            while j < 4 {
                let (lo, hi) = mac(t[j], a[j], b[i], c);
                t[j] = lo;
                c = hi;
                j += 1;
            }
            let (lo, hi) = adc(t[4], c, 0);
            t[4] = lo;
            t[5] = hi;
            let q = t[0].wrapping_mul(self.m0inv);
            let (_, mut c) = mac(t[0], q, m[0], 0);
            let mut j = 1;
            while j < 4 {
                let (lo, hi) = mac(t[j], q, m[j], c);
                t[j - 1] = lo;
                c = hi;
                j += 1;
            }
            let (lo, hi) = adc(t[4], c, 0);
            t[3] = lo;
            t[4] = t[5] + hi;
            i += 1;
        }
        let r = [t[0], t[1], t[2], t[3]];
        let (s, borrow) = sub4(&r, m);
        // keep s when t >= m: the extra limb is set, or the subtraction did not borrow
        let take_s = Choice::from_u8((t[4] as u8) | ((borrow as u8 & 1) ^ 1));
        select(take_s, &s, &r)
    }

    #[inline]
    pub fn add(&self, a: &U256, b: &U256) -> U256 {
        let (d, carry) = add4(a, b);
        let (s, borrow) = sub4(&d, &self.m);
        let take_s = Choice::from_u8((carry as u8) | ((borrow as u8 & 1) ^ 1));
        select(take_s, &s, &d)
    }

    #[inline]
    pub fn sub(&self, a: &U256, b: &U256) -> U256 {
        let (d, borrow) = sub4(a, b);
        let mask = borrow; // all-ones when a < b
        let madd = [self.m[0] & mask, self.m[1] & mask, self.m[2] & mask, self.m[3] & mask];
        add4(&d, &madd).0
    }

    #[inline]
    pub fn neg(&self, a: &U256) -> U256 {
        self.sub(&[0; 4], a)
    }

    /// Into Montgomery form (any `a < 2^256`, reduced mod m on the way).
    #[inline]
    pub fn to_mont(&self, a: &U256) -> U256 {
        self.mul(a, &self.r2)
    }
    /// Out of Montgomery form.
    #[inline]
    pub fn from_mont(&self, a: &U256) -> U256 {
        self.mul(a, &[1, 0, 0, 0])
    }
    /// `a mod m` for any `a < 2^256`.
    pub fn reduce(&self, a: &U256) -> U256 {
        self.from_mont(&self.to_mont(a))
    }
    /// `(lo + hi * 2^256) mod m`.
    pub fn reduce_wide(&self, lo: &U256, hi: &U256) -> U256 {
        let a = self.mul(lo, &self.r2); // lo * R
        let b = self.mul(hi, &self.r3); // hi * R^2
        self.from_mont(&self.add(&a, &b))
    }
    /// `a^e` in Montgomery form; `e` is PUBLIC (left-to-right square-and-multiply over all 256 bits).
    pub fn pow(&self, a: &U256, e: &U256) -> U256 {
        let mut r = self.one;
        let mut i = 256;
        while i > 0 {
            i -= 1;
            r = self.mul(&r, &r);
            if (e[i / 64] >> (i % 64)) & 1 == 1 {
                r = self.mul(&r, a);
            }
        }
        r
    }
    /// `a^-1` in Montgomery form (Fermat; m must be prime). `inv(0) = 0`.
    pub fn inv(&self, a: &U256) -> U256 {
        let e = sub4(&self.m, &[2, 0, 0, 0]).0;
        self.pow(a, &e)
    }
}

/// `if c { a } else { b }`.
#[inline(always)]
pub(crate) fn select(c: Choice, a: &U256, b: &U256) -> U256 {
    let m = c.mask_u64();
    [(a[0] & m) | (b[0] & !m), (a[1] & m) | (b[1] & !m), (a[2] & m) | (b[2] & !m), (a[3] & m) | (b[3] & !m)]
}

/// 1 when `a == 0`.
#[inline(always)]
pub(crate) fn is_zero(a: &U256) -> Choice {
    crate::ct::is_zero_u64(a[0] | a[1] | a[2] | a[3])
}

/// 1 when `a == b`.
#[inline(always)]
pub(crate) fn eq(a: &U256, b: &U256) -> Choice {
    is_zero(&[a[0] ^ b[0], a[1] ^ b[1], a[2] ^ b[2], a[3] ^ b[3]])
}

/// 1 when `a < b` (borrow of a - b). Constant-time.
#[inline(always)]
pub(crate) fn lt(a: &U256, b: &U256) -> Choice {
    Choice::from_u8((sub4(a, b).1 & 1) as u8)
}

pub(crate) fn from_be(b: &[u8; 32]) -> U256 {
    let mut r = [0u64; 4];
    for i in 0..4 {
        r[3 - i] = u64::from_be_bytes(b[i * 8..i * 8 + 8].try_into().unwrap());
    }
    r
}
pub(crate) fn to_be(a: &U256) -> [u8; 32] {
    let mut o = [0u8; 32];
    for i in 0..4 {
        o[i * 8..i * 8 + 8].copy_from_slice(&a[3 - i].to_be_bytes());
    }
    o
}
pub(crate) fn from_le(b: &[u8; 32]) -> U256 {
    let mut r = [0u64; 4];
    for i in 0..4 {
        r[i] = u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().unwrap());
    }
    r
}
pub(crate) fn to_le(a: &U256) -> [u8; 32] {
    let mut o = [0u8; 32];
    for i in 0..4 {
        o[i * 8..i * 8 + 8].copy_from_slice(&a[i].to_le_bytes());
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    // Ed25519's L — small enough that a u128 cross-check on the low limbs is meaningless, so the test
    // checks algebraic identities instead: (a*b)*c == a*(b*c), a*a^-1 == 1, reduce_wide of a known value.
    const L: Modulus = Modulus::new([0x5812631a5cf5d3ed, 0x14def9dea2f79cd6, 0, 0x1000000000000000]);

    #[test]
    fn identities() {
        let a = L.to_mont(&[1234567, 89, 0, 77]);
        let b = L.to_mont(&[u64::MAX, 3, 5, 0x0fff_ffff_ffff_ffff]);
        let c = L.to_mont(&[42, 0, 0, 0]);
        assert_eq!(L.mul(&L.mul(&a, &b), &c), L.mul(&a, &L.mul(&b, &c)));
        assert_eq!(L.mul(&a, &L.inv(&a)), L.one);
        assert_eq!(L.from_mont(&L.one), [1, 0, 0, 0]);
        // L itself reduces to 0; L + 5 to 5; 2^256 mod L via reduce_wide(0, 1) == R mod L == from_mont(R2).
        assert_eq!(L.reduce(&L.m), [0; 4]);
        assert_eq!(L.reduce_wide(&[0; 4], &[1, 0, 0, 0]), L.one);
        assert_eq!(L.sub(&[3, 0, 0, 0], &[5, 0, 0, 0]), L.sub(&L.m, &[2, 0, 0, 0]));
    }
}
