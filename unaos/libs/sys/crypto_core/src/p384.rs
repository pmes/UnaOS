// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! NIST P-384 (secp384r1; SP 800-186 §3.2.1.4): ECDH (SP 800-56A §5.7.1.2) and ECDSA (FIPS 186-5 §6),
//! RFC 6979 deterministic nonces (HMAC-SHA-384), SEC 1 point encoding, strict DER signatures. Public
//! PKI needs it: Let's Encrypt's E-series intermediates and many roots are P-384.
//!
//! The same construction as [`crate::p256`]: field and scalar arithmetic in Montgomery form
//! ([`crate::bignum`], six limbs, constant-time); homogeneous projective points with the COMPLETE
//! addition formula for a = -3 (Renes–Costello–Batina 2016, Algorithm 4), so doubling is the same code
//! and nothing branches on a point.
//!
//! CONSTANT-TIME:
//!  * [`SecretKey::public_key`], [`SecretKey::diffie_hellman`], [`SecretKey::sign_prehashed`]: yes —
//!    fixed 4-bit window over all 96 nibbles, full-table masked lookup, complete formulas, Fermat
//!    inversion; RFC 6979's rejection loop branches only on the public-in-effect event k ∈ [1, n-1];
//!  * [`verify_prehashed`] and point decoding: NO — every input is public (the constant-time multiplier
//!    is used anyway).

use crate::bignum::{self, from_hex, Mont};
use crate::ct::{Choice, Zeroize};
use crate::hmac::Hmac;
use crate::sha2::Sha384;
use crate::Error;

type U384 = [u64; 6];

/// p = 2^384 - 2^128 - 2^96 + 2^32 - 1
const P: Mont<6> = Mont::new(from_hex(
    "fffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffeffffffff0000000000000000ffffffff",
));
/// n, the group order.
const N: Mont<6> = Mont::new(from_hex(
    "ffffffffffffffffffffffffffffffffffffffffffffffffc7634d81f4372ddf581a0db248b0a77aecec196accc52973",
));
const B: U384 = from_hex("b3312fa7e23ee7e4988e056be3f82d19181d9c6efe8141120314088f5013875ac656398d8a2ed19d2a85c8edd3ec2aef");
const GX: U384 = from_hex("aa87ca22be8b05378eb1c71ef320ad746e1d3b628ba79b9859f741e082542a385502f25dbf55296c3a545e3872760ab7");
const GY: U384 = from_hex("3617de4a96262c6f5d9e98bf9292dc29f8f41dbd289a147ce9da3113b5f0b8c00a60b1ce1d7e819d7a431d7c90ea0e5f");

/// Bytes in a field element / scalar.
pub const SCALAR_LEN: usize = 48;

fn be(a: &U384) -> [u8; 48] {
    let mut o = [0u8; 48];
    bignum::to_be(a, &mut o);
    o
}

#[derive(Clone, Copy, Debug)]
struct Point {
    x: U384,
    y: U384,
    z: U384,
}

fn fadd(a: &U384, b: &U384) -> U384 {
    P.add(a, b)
}
fn fsub(a: &U384, b: &U384) -> U384 {
    P.sub(a, b)
}
fn fmul(a: &U384, b: &U384) -> U384 {
    P.mul(a, b)
}

impl Point {
    fn identity() -> Point {
        Point { x: [0; 6], y: P.one, z: [0; 6] }
    }

    fn generator() -> Point {
        Point { x: P.to_mont(&GX), y: P.to_mont(&GY), z: P.one }
    }

    /// Renes–Costello–Batina 2016, Algorithm 4 (complete, a = -3).
    fn add(&self, q: &Point) -> Point {
        let b = P.to_mont(&B);
        let (x1, y1, z1) = (&self.x, &self.y, &self.z);
        let (x2, y2, z2) = (&q.x, &q.y, &q.z);
        let mut t0 = fmul(x1, x2);
        let mut t1 = fmul(y1, y2);
        let mut t2 = fmul(z1, z2);
        let mut t3 = fadd(x1, y1);
        let mut t4 = fadd(x2, y2);
        t3 = fmul(&t3, &t4);
        t4 = fadd(&t0, &t1);
        t3 = fsub(&t3, &t4);
        t4 = fadd(y1, z1);
        let mut x3 = fadd(y2, z2);
        t4 = fmul(&t4, &x3);
        x3 = fadd(&t1, &t2);
        t4 = fsub(&t4, &x3);
        x3 = fadd(x1, z1);
        let mut y3 = fadd(x2, z2);
        x3 = fmul(&x3, &y3);
        y3 = fadd(&t0, &t2);
        y3 = fsub(&x3, &y3);
        let mut z3 = fmul(&b, &t2);
        x3 = fsub(&y3, &z3);
        z3 = fadd(&x3, &x3);
        x3 = fadd(&x3, &z3);
        z3 = fsub(&t1, &x3);
        x3 = fadd(&t1, &x3);
        y3 = fmul(&b, &y3);
        t1 = fadd(&t2, &t2);
        t2 = fadd(&t1, &t2);
        y3 = fsub(&y3, &t2);
        y3 = fsub(&y3, &t0);
        t1 = fadd(&y3, &y3);
        y3 = fadd(&t1, &y3);
        t1 = fadd(&t0, &t0);
        t0 = fadd(&t1, &t0);
        t0 = fsub(&t0, &t2);
        t1 = fmul(&t4, &y3);
        t2 = fmul(&t0, &y3);
        y3 = fmul(&x3, &z3);
        y3 = fadd(&y3, &t2);
        x3 = fmul(&t3, &x3);
        x3 = fsub(&x3, &t1);
        z3 = fmul(&t4, &z3);
        t1 = fmul(&t3, &t0);
        z3 = fadd(&z3, &t1);
        Point { x: x3, y: y3, z: z3 }
    }

    fn select(c: Choice, a: &Point, b: &Point) -> Point {
        Point { x: bignum::select(c, &a.x, &b.x), y: bignum::select(c, &a.y, &b.y), z: bignum::select(c, &a.z, &b.z) }
    }

    /// `[k]P`, k as little-endian limbs (< 2^384), fixed 4-bit window with masked table lookup.
    fn mul(&self, k: &U384) -> Point {
        let mut table = [Point::identity(); 16];
        for i in 1..16 {
            table[i] = table[i - 1].add(self);
        }
        let mut q = Point::identity();
        for i in (0..96).rev() {
            for _ in 0..4 {
                q = q.add(&q);
            }
            let nib = (k[i / 16] >> (4 * (i % 16))) & 0xf;
            let mut sel = Point::identity();
            for (j, e) in table.iter().enumerate() {
                sel = Point::select(crate::ct::eq_u64(nib, j as u64), e, &sel);
            }
            q = q.add(&sel);
        }
        q
    }

    fn is_identity(&self) -> Choice {
        bignum::is_zero(&self.z)
    }

    /// Affine (x, y), plain integers; `None` for the identity.
    fn to_affine(&self) -> Option<(U384, U384)> {
        if self.is_identity().into_bool() {
            return None;
        }
        let zi = P.inv(&self.z);
        Some((P.from_mont(&fmul(&self.x, &zi)), P.from_mont(&fmul(&self.y, &zi))))
    }

    /// SEC 1 §2.3.4 decoding of 04||x||y or 02/03||x, fully validated (coordinates < p, on the curve;
    /// cofactor 1, so on-curve means in the group; the identity has no encoding here).
    fn decode(b: &[u8]) -> Result<Point, Error> {
        let rd = |s: &[u8]| -> Result<U384, Error> {
            let v = bignum::from_be::<6>(s);
            if bignum::lt(&v, &P.m).into_bool() { Ok(v) } else { Err(Error::Encoding) }
        };
        let (x, y) = match (b.first(), b.len()) {
            (Some(4), 97) => (rd(&b[1..49])?, Some(rd(&b[49..97])?)),
            (Some(2), 49) | (Some(3), 49) => (rd(&b[1..49])?, None),
            _ => return Err(Error::Encoding),
        };
        let xm = P.to_mont(&x);
        let x3 = fmul(&fmul(&xm, &xm), &xm);
        let three_x = fadd(&fadd(&xm, &xm), &xm);
        let rhs = fadd(&fsub(&x3, &three_x), &P.to_mont(&B));
        let ym = match y {
            Some(y) => {
                let ym = P.to_mont(&y);
                if !bignum::eq(&fmul(&ym, &ym), &rhs).into_bool() {
                    return Err(Error::Encoding);
                }
                ym
            }
            None => {
                // p = 3 mod 4: sqrt(a) = a^((p+1)/4)
                let mut e = P.m;
                e[0] += 1; // p is odd and its low limb is not all-ones: no carry
                for i in 0..6 {
                    e[i] = (e[i] >> 2) | if i < 5 { e[i + 1] << 62 } else { 0 };
                }
                let r = P.pow(&rhs, &e);
                if !bignum::eq(&fmul(&r, &r), &rhs).into_bool() {
                    return Err(Error::Encoding);
                }
                let parity = (P.from_mont(&r)[0] & 1) as u8;
                if parity != (b[0] & 1) { P.neg(&r) } else { r }
            }
        };
        Ok(Point { x: xm, y: ym, z: P.one })
    }

    fn encode_uncompressed(&self) -> Option<[u8; 97]> {
        let (x, y) = self.to_affine()?;
        let mut o = [0u8; 97];
        o[0] = 4;
        o[1..49].copy_from_slice(&be(&x));
        o[49..].copy_from_slice(&be(&y));
        Some(o)
    }
}

/// A validated public key (a point on P-384, not the identity).
#[derive(Clone, Copy, Debug)]
pub struct PublicKey(Point);

impl PublicKey {
    /// SEC 1 decoding (uncompressed `04||x||y` or compressed `02/03||x`), fully validated.
    pub fn from_sec1(b: &[u8]) -> Result<Self, Error> {
        Point::decode(b).map(PublicKey)
    }
    /// Uncompressed SEC 1 encoding (97 bytes).
    pub fn to_sec1_uncompressed(&self) -> [u8; 97] {
        self.0.encode_uncompressed().expect("public keys are never the identity")
    }
}

/// A P-384 private scalar d ∈ [1, n-1]. Zeroized on drop.
pub struct SecretKey {
    d: U384,
}

impl SecretKey {
    /// From 48 big-endian bytes; `Error::Encoding` unless 1 <= d < n (constant-time check, public result).
    pub fn from_bytes(b: &[u8; 48]) -> Result<Self, Error> {
        let d = bignum::from_be::<6>(b);
        let ok = bignum::lt(&d, &N.m) & !bignum::is_zero(&d);
        if ok.into_bool() { Ok(SecretKey { d }) } else { Err(Error::Encoding) }
    }

    /// The 48-byte big-endian scalar.
    pub fn to_bytes(&self) -> [u8; 48] {
        be(&self.d)
    }

    /// `d·G`. Constant-time.
    pub fn public_key(&self) -> PublicKey {
        PublicKey(Point::generator().mul(&self.d))
    }

    /// ECDH: the x-coordinate of `d·Q`, 48 bytes big-endian. Constant-time in d.
    pub fn diffie_hellman(&self, peer: &PublicKey) -> Result<[u8; 48], Error> {
        let s = peer.0.mul(&self.d);
        let (x, _) = s.to_affine().ok_or(Error::Degenerate)?;
        Ok(be(&x))
    }

    /// ECDSA over a message digest with an RFC 6979 nonce (HMAC-SHA-384 DRBG); raw `r || s`, 48 bytes
    /// each. The digest may be any length (its leftmost 384 bits are used).
    pub fn sign_prehashed(&self, digest: &[u8]) -> [u8; 96] {
        let e = bits2int(digest);
        let h1 = be(&N.reduce(&e));
        let x = be(&self.d);
        let mut v = [0x01u8; 48];
        let mut k = [0x00u8; 48];
        let mac = |key: &[u8; 48], parts: &[&[u8]]| -> [u8; 48] {
            let mut m = Hmac::<Sha384>::new(key);
            for p in parts {
                m.update(p);
            }
            let mut o = [0u8; 48];
            m.finalize_into(&mut o);
            o
        };
        for round in [0x00u8, 0x01] {
            k = mac(&k, &[&v, &[round], &x, &h1]);
            v = mac(&k, &[&v]);
        }
        loop {
            v = mac(&k, &[&v]);
            let cand = bignum::from_be::<6>(&v);
            let in_range = bignum::lt(&cand, &N.m) & !bignum::is_zero(&cand);
            if in_range.into_bool() {
                if let Some(sig) = self.sign_with_nonce(&e, &cand) {
                    k.zeroize();
                    v.zeroize();
                    return sig;
                }
            }
            k = mac(&k, &[&v, &[0x00]]);
            v = mac(&k, &[&v]);
        }
    }

    fn sign_with_nonce(&self, e: &U384, k: &U384) -> Option<[u8; 96]> {
        let (rx, _) = Point::generator().mul(k).to_affine()?;
        let r = N.reduce(&rx);
        if bignum::is_zero(&r).into_bool() {
            return None;
        }
        let kinv = N.inv(&N.to_mont(k));
        let rd = N.mul(&N.to_mont(&r), &N.reduce(&self.d));
        let sum = N.add(&N.reduce(e), &rd);
        let s = N.mul(&kinv, &sum);
        if bignum::is_zero(&s).into_bool() {
            return None;
        }
        let mut sig = [0u8; 96];
        sig[..48].copy_from_slice(&be(&r));
        sig[48..].copy_from_slice(&be(&s));
        Some(sig)
    }

    /// ECDSA-SHA-384 over a message.
    pub fn sign_sha384(&self, msg: &[u8]) -> [u8; 96] {
        self.sign_prehashed(&crate::sha2::sha384(msg))
    }
}

impl Drop for SecretKey {
    fn drop(&mut self) {
        self.d.zeroize();
    }
}

/// bits2int for a 384-bit order: the leftmost 384 bits of the digest (a shorter digest is taken whole).
fn bits2int(d: &[u8]) -> U384 {
    let mut b = [0u8; 48];
    if d.len() >= 48 {
        b.copy_from_slice(&d[..48]);
    } else {
        b[48 - d.len()..].copy_from_slice(d);
    }
    bignum::from_be::<6>(&b)
}

/// ECDSA verification (FIPS 186-5 §6.4.2) of a raw `r || s` signature over a digest. Variable-time
/// (public inputs).
pub fn verify_prehashed(key: &PublicKey, digest: &[u8], sig: &[u8; 96]) -> Result<(), Error> {
    let r = bignum::from_be::<6>(&sig[..48]);
    let s = bignum::from_be::<6>(&sig[48..]);
    for v in [&r, &s] {
        if bignum::is_zero(v).into_bool() || !bignum::lt(v, &N.m).into_bool() {
            return Err(Error::Auth);
        }
    }
    let e = N.reduce(&bits2int(digest));
    let w = N.inv(&N.to_mont(&s));
    let u1 = N.mul(&w, &e);
    let u2 = N.mul(&w, &r);
    let pt = Point::generator().mul(&u1).add(&key.0.mul(&u2));
    let (x, _) = pt.to_affine().ok_or(Error::Auth)?;
    if bignum::eq(&N.reduce(&x), &r).into_bool() { Ok(()) } else { Err(Error::Auth) }
}

/// ECDSA-SHA-384 verification of a raw `r || s` signature.
pub fn verify_sha384(key: &PublicKey, msg: &[u8], sig: &[u8; 96]) -> Result<(), Error> {
    verify_prehashed(key, &crate::sha2::sha384(msg), sig)
}

/// Strict DER decoding of `SEQUENCE { INTEGER r, INTEGER s }` into raw `r || s` (48 bytes each): the
/// same rules as [`crate::p256::signature_from_der`] (minimal definite lengths, positive minimal
/// integers, nothing trailing).
pub fn signature_from_der(der: &[u8]) -> Result<[u8; 96], Error> {
    let mut sig = [0u8; 96];
    crate::p256::der_sig_decode(der, &mut sig, 48)?;
    Ok(sig)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn curve_constants() {
        let g = Point::generator();
        let enc = g.encode_uncompressed().unwrap();
        assert!(Point::decode(&enc).is_ok(), "G on the curve");
        assert!(g.mul(&N.m).is_identity().into_bool(), "[n]G = O");
        // compressed round trip
        let mut c = [0u8; 49];
        c[0] = 2 | (enc[96] & 1);
        c[1..].copy_from_slice(&enc[1..49]);
        assert_eq!(Point::decode(&c).unwrap().encode_uncompressed().unwrap(), enc);
    }
}
