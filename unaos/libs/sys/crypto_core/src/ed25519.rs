// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Ed25519 (RFC 8032 §5.1): keys, signing, cofactorless verification.
//!
//! Points are extended twisted-Edwards coordinates (X:Y:Z:T), x = X/Z, y = Y/Z, xy = T/Z (Hisil–Wong–
//! Carter–Dawson 2008). Addition uses the unified a = -1 formula ("add-2008-hwcd-3"), which is COMPLETE
//! on edwards25519 (d is a non-square): it is also used for doubling, so there is no exceptional case
//! and no branch. Scalars mod L use [`crate::bigint`]'s constant-time Montgomery arithmetic.
//!
//! CONSTANT-TIME:
//!  * key generation and [`SigningKey::sign`]: yes — the scalar multiplication is a fixed 4-bit window
//!    over all 64 nibbles with a full-table masked lookup; the nonce and the secret scalar never steer a
//!    branch or an index;
//!  * [`verify`]: NO, and need not be — every input (public key, message, signature) is public. It still
//!    uses the same constant-time multiplier (simplicity over speed; ~2x slower than a variable-time
//!    double-scalar multiply).
//!
//! Verification is the COFACTORLESS equation `[S]B = R + [k]A` (RFC 8032 §5.1.7 permits it; it is
//! what Wycheproof's vectors expect), with `S < L` enforced, A decoded strictly (y < p, no "-0"), and R
//! compared by its canonical encoding (a non-canonical R can never match).

use crate::bigint::{self, Modulus, U256};
use crate::ct::{Choice, Zeroize};
use crate::field25519::Fe;
use crate::sha2::Sha512;
use crate::Error;

/// L = 2^252 + 27742317777372353535851937790883648493, the prime order of the base point.
const L: Modulus = Modulus::new([0x5812631a5cf5d3ed, 0x14def9dea2f79cd6, 0, 0x1000000000000000]);

/// d = -121665/121666 (little-endian encoding).
const D_BYTES: [u8; 32] = [
    0xa3, 0x78, 0x59, 0x13, 0xca, 0x4d, 0xeb, 0x75, 0xab, 0xd8, 0x41, 0x41, 0x4d, 0x0a, 0x70, 0x00, 0x98, 0xe8, 0x79,
    0x77, 0x79, 0x40, 0xc7, 0x8c, 0x73, 0xfe, 0x6f, 0x2b, 0xee, 0x6c, 0x03, 0x52,
];
/// sqrt(-1) = 2^((p-1)/4) (little-endian encoding).
const SQRT_M1_BYTES: [u8; 32] = [
    0xb0, 0xa0, 0x0e, 0x4a, 0x27, 0x1b, 0xee, 0xc4, 0x78, 0xe4, 0x2f, 0xad, 0x06, 0x18, 0x43, 0x2f, 0xa7, 0xd7, 0xfb,
    0x3d, 0x99, 0x00, 0x4d, 0x2b, 0x0b, 0xdf, 0xc1, 0x4f, 0x80, 0x24, 0x83, 0x2b,
];
/// The base point B, encoded (y = 4/5, x positive).
const B_BYTES: [u8; 32] = [
    0x58, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
    0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66, 0x66,
];

#[derive(Clone, Copy)]
struct Point {
    x: Fe,
    y: Fe,
    z: Fe,
    t: Fe,
}

impl Point {
    const IDENTITY: Point = Point { x: Fe::ZERO, y: Fe::ONE, z: Fe::ONE, t: Fe::ZERO };

    /// Unified addition (complete for a = -1, d non-square).
    fn add(&self, q: &Point, d2: &Fe) -> Point {
        let a = self.y.sub(&self.x).mul(&q.y.sub(&q.x));
        let b = self.y.add(&self.x).mul(&q.y.add(&q.x));
        let c = self.t.mul(d2).mul(&q.t);
        let d = self.z.add(&self.z).mul(&q.z);
        let e = b.sub(&a);
        let f = d.sub(&c);
        let g = d.add(&c);
        let h = b.add(&a);
        Point { x: e.mul(&f), y: g.mul(&h), t: e.mul(&h), z: f.mul(&g) }
    }

    fn neg(&self) -> Point {
        Point { x: self.x.neg(), y: self.y, z: self.z, t: self.t.neg() }
    }

    fn select(c: Choice, a: &Point, b: &Point) -> Point {
        Point { x: Fe::select(c, &a.x, &b.x), y: Fe::select(c, &a.y, &b.y), z: Fe::select(c, &a.z, &b.z), t: Fe::select(c, &a.t, &b.t) }
    }

    /// `[k]P` for a 32-byte little-endian scalar (any value < 2^256): fixed 4-bit window, every table
    /// entry read on every step.
    fn mul(&self, k: &[u8; 32], d2: &Fe) -> Point {
        let mut table = [Point::IDENTITY; 16];
        for i in 1..16 {
            table[i] = table[i - 1].add(self, d2);
        }
        let mut q = Point::IDENTITY;
        for i in (0..64).rev() {
            for _ in 0..4 {
                q = q.add(&q, d2);
            }
            let nib = (k[i / 2] >> (4 * (i % 2))) & 0xf;
            let mut sel = Point::IDENTITY;
            for (j, e) in table.iter().enumerate() {
                sel = Point::select(crate::ct::eq_u64(nib as u64, j as u64), e, &sel);
            }
            q = q.add(&sel, d2);
        }
        q
    }

    fn encode(&self) -> [u8; 32] {
        let zi = self.z.invert();
        let x = self.x.mul(&zi);
        let y = self.y.mul(&zi);
        let mut b = y.to_bytes();
        b[31] |= x.is_negative().unwrap_u8() << 7;
        b
    }

    /// RFC 8032 §5.1.3 decoding, strict: y must be canonical (< p), and x = 0 with the sign bit set is
    /// refused. Not constant-time (only public points are ever decoded).
    fn decode(b: &[u8; 32]) -> Option<Point> {
        let sign = b[31] >> 7;
        let mut yb = *b;
        yb[31] &= 0x7f;
        let y = Fe::from_bytes(&yb);
        if y.to_bytes() != yb {
            return None;
        }
        let d = Fe::from_bytes(&D_BYTES);
        let y2 = y.square();
        let u = y2.sub(&Fe::ONE);
        let v = d.mul(&y2).add(&Fe::ONE);
        let v3 = v.square().mul(&v);
        let v7 = v3.square().mul(&v);
        let mut x = u.mul(&v3).mul(&u.mul(&v7).pow22523());
        let vx2 = v.mul(&x.square());
        if vx2.ct_eq(&u).into_bool() {
        } else if vx2.ct_eq(&u.neg()).into_bool() {
            x = x.mul(&Fe::from_bytes(&SQRT_M1_BYTES));
        } else {
            return None;
        }
        if x.is_zero().into_bool() && sign == 1 {
            return None;
        }
        if x.is_negative().unwrap_u8() != sign {
            x = x.neg();
        }
        Some(Point { x, y, z: Fe::ONE, t: x.mul(&y) })
    }
}

fn d2() -> Fe {
    let d = Fe::from_bytes(&D_BYTES);
    d.add(&d)
}

fn base() -> Point {
    Point::decode(&B_BYTES).expect("base point")
}

/// SHA-512 output reduced mod L (little-endian 64 bytes → scalar).
fn reduce64(h: &[u8; 64]) -> U256 {
    let lo = bigint::from_le(h[..32].try_into().unwrap());
    let hi = bigint::from_le(h[32..].try_into().unwrap());
    L.reduce_wide(&lo, &hi)
}

/// An Ed25519 signing key: the 32-byte seed, the clamped secret scalar, the nonce prefix and the public
/// key. Zeroized on drop.
pub struct SigningKey {
    seed: [u8; 32],
    scalar: [u8; 32],
    prefix: [u8; 32],
    public: [u8; 32],
}

impl SigningKey {
    /// RFC 8032 §5.1.5 key generation from a 32-byte secret seed. Constant-time in the seed.
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        let mut h = Sha512::new();
        h.update(seed);
        let mut hh = h.finalize();
        let mut scalar = [0u8; 32];
        scalar.copy_from_slice(&hh[..32]);
        scalar[0] &= 248;
        scalar[31] &= 127;
        scalar[31] |= 64;
        let mut prefix = [0u8; 32];
        prefix.copy_from_slice(&hh[32..]);
        hh.zeroize();
        let public = base().mul(&scalar, &d2()).encode();
        SigningKey { seed: *seed, scalar, prefix, public }
    }

    /// The 32-byte public key.
    pub fn public_key(&self) -> [u8; 32] {
        self.public
    }

    /// The seed this key was made from.
    pub fn seed(&self) -> &[u8; 32] {
        &self.seed
    }

    /// RFC 8032 §5.1.6 signing (deterministic). Constant-time in the key and the nonce.
    pub fn sign(&self, msg: &[u8]) -> [u8; 64] {
        let d2 = d2();
        let mut h = Sha512::new();
        h.update(&self.prefix);
        h.update(msg);
        let mut rh = h.finalize();
        let r = reduce64(&rh);
        rh.zeroize();
        let mut r_bytes = bigint::to_le(&r);
        let big_r = base().mul(&r_bytes, &d2).encode();
        let mut h = Sha512::new();
        h.update(&big_r);
        h.update(&self.public);
        h.update(msg);
        let k = reduce64(&h.finalize());
        // S = r + k * a mod L
        let a = L.reduce(&bigint::from_le(&self.scalar));
        let ka = L.mul(&L.to_mont(&k), &a);
        let s = L.add(&r, &ka);
        let mut sig = [0u8; 64];
        sig[..32].copy_from_slice(&big_r);
        sig[32..].copy_from_slice(&bigint::to_le(&s));
        r_bytes.zeroize();
        sig
    }
}

impl Drop for SigningKey {
    fn drop(&mut self) {
        self.seed.zeroize();
        self.scalar.zeroize();
        self.prefix.zeroize();
    }
}

/// RFC 8032 §5.1.7 verification, cofactorless. `Err(Error::Encoding)` for a malformed public key or an
/// `S >= L`; `Err(Error::Auth)` when the equation fails. Variable-time (all inputs public).
pub fn verify(public: &[u8; 32], msg: &[u8], sig: &[u8; 64]) -> Result<(), Error> {
    let a = Point::decode(public).ok_or(Error::Encoding)?;
    let s = bigint::from_le(sig[32..].try_into().unwrap());
    if !bigint::lt(&s, &L.m).into_bool() {
        return Err(Error::Encoding);
    }
    let mut h = Sha512::new();
    h.update(&sig[..32]);
    h.update(public);
    h.update(msg);
    let k = reduce64(&h.finalize());
    let d2 = d2();
    let sb = base().mul(sig[32..].try_into().unwrap(), &d2);
    let ka = a.neg().mul(&bigint::to_le(&k), &d2);
    let r = sb.add(&ka, &d2).encode();
    if r == sig[..32] { Ok(()) } else { Err(Error::Auth) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn constants() {
        // d = -121665/121666
        let d = Fe::from_u64(121665).neg().mul(&Fe::from_u64(121666).invert());
        assert_eq!(d.to_bytes(), D_BYTES);
        // sqrt(-1)^2 = -1
        let i = Fe::from_bytes(&SQRT_M1_BYTES);
        assert_eq!(i.square().to_bytes(), Fe::ONE.neg().to_bytes());
        // B: y = 4/5, re-encodes to itself, and [L]B = identity
        let y = Fe::from_bytes(&B_BYTES);
        assert_eq!(y.mul(&Fe::from_u64(5)).to_bytes(), Fe::from_u64(4).to_bytes());
        let b = base();
        assert_eq!(b.encode(), B_BYTES);
        let l = bigint::to_le(&L.m);
        assert_eq!(b.mul(&l, &d2()).encode(), Point::IDENTITY.encode());
    }
}
