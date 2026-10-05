// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Fixed-width modular arithmetic in Montgomery form over `L` 64-bit limbs (little-endian), for any odd
//! modulus below 2^(64L) — the const-generic sibling of [`crate::bigint`] (which stays the audited
//! 256-bit core of P-256 and Ed25519). Used by P-384 (`L = 6`).
//!
//! Montgomery multiplication is CIOS (Koç, Acar, Kaliski 1996), R = 2^(64L); R mod m, R^2 mod m and
//! -m^-1 mod 2^64 are computed at COMPILE time, and a modulus is written as its big-endian hex string
//! (parsed at compile time too, so no limb is transcribed by hand).
//!
//! CONSTANT-TIME: yes for `mul`, `add`, `sub`, `neg`, `to_mont`, `from_mont`, `reduce`, `select`,
//! `is_zero`, `eq`, `lt` — the same instruction sequence for every input; conditional subtractions are
//! masks. `pow` is constant-time in the BASE (its exponent is public: m-2 or (m+1)/4).

use crate::ct::Choice;

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
pub(crate) const fn sub_n<const L: usize>(a: &[u64; L], b: &[u64; L]) -> ([u64; L], u64) {
    let mut r = [0u64; L];
    let mut br = 0u64;
    let mut i = 0;
    while i < L {
        let (v, b2) = sbb(a[i], b[i], br);
        r[i] = v;
        br = b2;
        i += 1;
    }
    (r, br)
}

#[inline(always)]
const fn add_n<const L: usize>(a: &[u64; L], b: &[u64; L]) -> ([u64; L], u64) {
    let mut r = [0u64; L];
    let mut c = 0u64;
    let mut i = 0;
    while i < L {
        let (v, c2) = adc(a[i], b[i], c);
        r[i] = v;
        c = c2;
        i += 1;
    }
    (r, c)
}

/// Big-endian hex (exactly 16L digits) to limbs, at compile time.
pub(crate) const fn from_hex<const L: usize>(s: &str) -> [u64; L] {
    let b = s.as_bytes();
    assert!(b.len() == 16 * L);
    let mut r = [0u64; L];
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        let d = match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => panic!("bad hex"),
        } as u64;
        let limb = L - 1 - i / 16;
        r[limb] = (r[limb] << 4) | d;
        i += 1;
    }
    r
}

const fn dbl_mod<const L: usize>(x: &[u64; L], m: &[u64; L]) -> [u64; L] {
    let (d, carry) = add_n(x, x);
    let (s, borrow) = sub_n(&d, m);
    if carry == 1 || borrow == 0 { s } else { d }
}

/// A modulus with its Montgomery constants.
pub(crate) struct Mont<const L: usize> {
    pub m: [u64; L],
    m0inv: u64,
    r2: [u64; L],
    /// R mod m (1 in Montgomery form).
    pub one: [u64; L],
}

impl<const L: usize> Mont<L> {
    pub const fn new(m: [u64; L]) -> Self {
        let mut inv: u64 = 1;
        let mut i = 0;
        while i < 6 {
            inv = inv.wrapping_mul(2u64.wrapping_sub(m[0].wrapping_mul(inv)));
            i += 1;
        }
        let mut x = [0u64; L];
        x[0] = 1;
        let mut one = [0u64; L];
        let mut k = 0;
        while k < 128 * L {
            x = dbl_mod(&x, &m);
            k += 1;
            if k == 64 * L {
                one = x;
            }
        }
        Mont { m, m0inv: inv.wrapping_neg(), r2: x, one }
    }

    /// `a * b * R^-1 mod m`, for `a < 2^(64L)` and `b < m` (output fully reduced).
    #[inline]
    pub fn mul(&self, a: &[u64; L], b: &[u64; L]) -> [u64; L] {
        let m = &self.m;
        // t has L + 2 limbs; const generic arithmetic on lengths is unstable, so use a fixed upper bound.
        let mut t = [0u64; 18];
        debug_assert!(L + 2 <= 18);
        for i in 0..L {
            let mut c = 0u64;
            for j in 0..L {
                let (lo, hi) = mac(t[j], a[j], b[i], c);
                t[j] = lo;
                c = hi;
            }
            let (lo, hi) = adc(t[L], c, 0);
            t[L] = lo;
            t[L + 1] = hi;
            let q = t[0].wrapping_mul(self.m0inv);
            let (_, mut c) = mac(t[0], q, m[0], 0);
            for j in 1..L {
                let (lo, hi) = mac(t[j], q, m[j], c);
                t[j - 1] = lo;
                c = hi;
            }
            let (lo, hi) = adc(t[L], c, 0);
            t[L - 1] = lo;
            t[L] = t[L + 1] + hi;
        }
        let mut r = [0u64; L];
        r.copy_from_slice(&t[..L]);
        let (s, borrow) = sub_n(&r, m);
        let take_s = Choice::from_u8((t[L] as u8) | ((borrow as u8 & 1) ^ 1));
        select(take_s, &s, &r)
    }

    #[inline]
    pub fn add(&self, a: &[u64; L], b: &[u64; L]) -> [u64; L] {
        let (d, carry) = add_n(a, b);
        let (s, borrow) = sub_n(&d, &self.m);
        let take_s = Choice::from_u8((carry as u8) | ((borrow as u8 & 1) ^ 1));
        select(take_s, &s, &d)
    }

    #[inline]
    pub fn sub(&self, a: &[u64; L], b: &[u64; L]) -> [u64; L] {
        let (d, mask) = sub_n(a, b);
        let mut madd = [0u64; L];
        for i in 0..L {
            madd[i] = self.m[i] & mask;
        }
        add_n(&d, &madd).0
    }

    #[inline]
    pub fn neg(&self, a: &[u64; L]) -> [u64; L] {
        self.sub(&[0; L], a)
    }

    /// Into Montgomery form (any `a < 2^(64L)`, reduced on the way).
    #[inline]
    pub fn to_mont(&self, a: &[u64; L]) -> [u64; L] {
        self.mul(a, &self.r2)
    }
    /// Out of Montgomery form.
    #[inline]
    pub fn from_mont(&self, a: &[u64; L]) -> [u64; L] {
        let mut one = [0u64; L];
        one[0] = 1;
        self.mul(a, &one)
    }
    /// `a mod m` for any `a < 2^(64L)`.
    pub fn reduce(&self, a: &[u64; L]) -> [u64; L] {
        self.from_mont(&self.to_mont(a))
    }
    /// `a^e` in Montgomery form; `e` is PUBLIC.
    pub fn pow(&self, a: &[u64; L], e: &[u64; L]) -> [u64; L] {
        let mut r = self.one;
        for i in (0..64 * L).rev() {
            r = self.mul(&r, &r);
            if (e[i / 64] >> (i % 64)) & 1 == 1 {
                r = self.mul(&r, a);
            }
        }
        r
    }
    /// `a^-1` in Montgomery form (Fermat; m prime). `inv(0) = 0`.
    pub fn inv(&self, a: &[u64; L]) -> [u64; L] {
        let mut two = [0u64; L];
        two[0] = 2;
        self.pow(a, &sub_n(&self.m, &two).0)
    }
}

/// `if c { a } else { b }`.
#[inline(always)]
pub(crate) fn select<const L: usize>(c: Choice, a: &[u64; L], b: &[u64; L]) -> [u64; L] {
    let m = c.mask_u64();
    let mut r = [0u64; L];
    for i in 0..L {
        r[i] = (a[i] & m) | (b[i] & !m);
    }
    r
}

/// 1 when `a == 0`.
#[inline(always)]
pub(crate) fn is_zero<const L: usize>(a: &[u64; L]) -> Choice {
    let mut acc = 0u64;
    for x in a {
        acc |= x;
    }
    crate::ct::is_zero_u64(acc)
}

/// 1 when `a == b`.
#[inline(always)]
pub(crate) fn eq<const L: usize>(a: &[u64; L], b: &[u64; L]) -> Choice {
    let mut acc = 0u64;
    for i in 0..L {
        acc |= a[i] ^ b[i];
    }
    crate::ct::is_zero_u64(acc)
}

/// 1 when `a < b`.
#[inline(always)]
pub(crate) fn lt<const L: usize>(a: &[u64; L], b: &[u64; L]) -> Choice {
    Choice::from_u8((sub_n(a, b).1 & 1) as u8)
}

/// Big-endian bytes (exactly 8L) to limbs.
pub(crate) fn from_be<const L: usize>(b: &[u8]) -> [u64; L] {
    assert_eq!(b.len(), 8 * L);
    let mut r = [0u64; L];
    for i in 0..L {
        r[L - 1 - i] = u64::from_be_bytes(b[i * 8..i * 8 + 8].try_into().unwrap());
    }
    r
}

/// Limbs to big-endian bytes (`out.len() == 8L`).
pub(crate) fn to_be<const L: usize>(a: &[u64; L], out: &mut [u8]) {
    assert_eq!(out.len(), 8 * L);
    for i in 0..L {
        out[i * 8..i * 8 + 8].copy_from_slice(&a[L - 1 - i].to_be_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    // The P-384 prime: algebraic identities, and agreement with the 256-bit core on a 4-limb modulus.
    const P: Mont<6> = Mont::new(from_hex(
        "fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffeffffffff0000000000000000ffffffff",
    ));

    #[test]
    fn identities() {
        let a = P.to_mont(&[1234567, 89, 0, 77, 5, 0x7fff]);
        let b = P.to_mont(&[u64::MAX, 3, 5, 0x0fff_ffff_ffff_ffff, 9, 1]);
        let c = P.to_mont(&[42, 0, 0, 0, 0, 0]);
        assert_eq!(P.mul(&P.mul(&a, &b), &c), P.mul(&a, &P.mul(&b, &c)));
        assert_eq!(P.mul(&a, &P.inv(&a)), P.one);
        assert_eq!(P.from_mont(&P.one), [1, 0, 0, 0, 0, 0]);
        assert_eq!(P.reduce(&P.m), [0; 6]);
        assert_eq!(P.sub(&[3, 0, 0, 0, 0, 0], &[5, 0, 0, 0, 0, 0]), P.sub(&P.m, &[2, 0, 0, 0, 0, 0]));
        // against the audited 256-bit core, on the P-256 prime
        const M4: [u64; 4] = [0xffffffffffffffff, 0x00000000ffffffff, 0x0000000000000000, 0xffffffff00000001];
        let g: Mont<4> = Mont::new(M4);
        let h = crate::bigint::Modulus::new(M4);
        let x = [0x0123456789abcdef, 0xfedcba9876543210, 0x1111, 0x7777_0000_0000_0001];
        let y = [5, 6, 7, 8];
        assert_eq!(g.mul(&g.to_mont(&x), &g.to_mont(&y)), h.mul(&h.to_mont(&x), &h.to_mont(&y)));
        assert_eq!(g.inv(&g.to_mont(&x)), h.inv(&h.to_mont(&x)));
    }
}
