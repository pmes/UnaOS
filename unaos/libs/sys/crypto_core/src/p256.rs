// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! NIST P-256 (secp256r1; SP 800-186 §3.2.1.3): ECDH (SP 800-56A §5.7.1.2) and ECDSA (FIPS 186-5 §6),
//! with RFC 6979 deterministic nonces, SEC 1 point encoding, and strict DER signatures.
//!
//! Field and scalar arithmetic: [`crate::bigint`] Montgomery form, constant-time. Points: homogeneous
//! projective (X:Y:Z) with the COMPLETE addition formula for a = -3 (Renes–Costello–Batina 2016,
//! Algorithm 4) — valid for every pair of inputs including P = Q, P = -Q and the identity, so doubling is
//! the same code and nothing branches on a point.
//!
//! CONSTANT-TIME:
//!  * [`SecretKey::public_key`], [`SecretKey::diffie_hellman`], [`SecretKey::sign_prehashed`]: yes — fixed
//!    4-bit window over all 64 nibbles, full-table masked lookup, complete formulas; the nonce inverse is
//!    Fermat (fixed exponent); RFC 6979's rejection loop branches only on the (overwhelmingly
//!    first-try) public-in-effect event k ∈ [1, n-1];
//!  * [`verify_prehashed`] and point decoding: NO — every input is public. (They use the constant-time
//!    multiplier anyway.)

use crate::bigint::{self, Modulus, U256};
use crate::ct::{Choice, Zeroize};
use crate::hmac::Hmac;
use crate::sha2::{Sha256};
use crate::Error;

/// p = 2^256 - 2^224 + 2^192 + 2^96 - 1
const P: Modulus = Modulus::new([0xffffffffffffffff, 0x00000000ffffffff, 0x0000000000000000, 0xffffffff00000001]);
/// n, the group order.
const N: Modulus = Modulus::new([0xf3b9cac2fc632551, 0xbce6faada7179e84, 0xffffffffffffffff, 0xffffffff00000000]);
/// b (plain, not Montgomery).
const B: U256 = [0x3bce3c3e27d2604b, 0x651d06b0cc53b0f6, 0xb3ebbd55769886bc, 0x5ac635d8aa3a93e7];
const GX: U256 = [0xf4a13945d898c296, 0x77037d812deb33a0, 0xf8bce6e563a440f2, 0x6b17d1f2e12c4247];
const GY: U256 = [0xcbb6406837bf51f5, 0x2bce33576b315ece, 0x8ee7eb4a7c0f9e16, 0x4fe342e2fe1a7f9b];

#[derive(Clone, Copy, Debug)]
struct Point {
    x: U256,
    y: U256,
    z: U256,
}

fn fadd(a: &U256, b: &U256) -> U256 {
    P.add(a, b)
}
fn fsub(a: &U256, b: &U256) -> U256 {
    P.sub(a, b)
}
fn fmul(a: &U256, b: &U256) -> U256 {
    P.mul(a, b)
}

impl Point {
    fn identity() -> Point {
        Point { x: [0; 4], y: P.one, z: [0; 4] }
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
        Point { x: bigint::select(c, &a.x, &b.x), y: bigint::select(c, &a.y, &b.y), z: bigint::select(c, &a.z, &b.z) }
    }

    /// `[k]P`, k as little-endian limbs (< 2^256), fixed 4-bit window with masked table lookup.
    fn mul(&self, k: &U256) -> Point {
        let mut table = [Point::identity(); 16];
        for i in 1..16 {
            table[i] = table[i - 1].add(self);
        }
        let mut q = Point::identity();
        for i in (0..64).rev() {
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
        bigint::is_zero(&self.z)
    }

    /// Affine (x, y), plain (not Montgomery) integers; `None` for the identity.
    fn to_affine(&self) -> Option<(U256, U256)> {
        if self.is_identity().into_bool() {
            return None;
        }
        let zi = P.inv(&self.z);
        Some((P.from_mont(&fmul(&self.x, &zi)), P.from_mont(&fmul(&self.y, &zi))))
    }

    /// SEC 1 §2.3.4 decoding of 04||x||y or 02/03||x, with full validation (coordinates < p, on the
    /// curve; the identity has no encoding here). P-256 has cofactor 1, so on-curve means in the group.
    fn decode(b: &[u8]) -> Result<Point, Error> {
        let rd = |s: &[u8]| -> Result<U256, Error> {
            let v = bigint::from_be(s.try_into().unwrap());
            if bigint::lt(&v, &P.m).into_bool() { Ok(v) } else { Err(Error::Encoding) }
        };
        let (x, y) = match (b.first(), b.len()) {
            (Some(4), 65) => (rd(&b[1..33])?, Some(rd(&b[33..65])?)),
            (Some(2), 33) | (Some(3), 33) => (rd(&b[1..33])?, None),
            _ => return Err(Error::Encoding),
        };
        let xm = P.to_mont(&x);
        // rhs = x^3 - 3x + b
        let x3 = fmul(&fmul(&xm, &xm), &xm);
        let three_x = fadd(&fadd(&xm, &xm), &xm);
        let rhs = fadd(&fsub(&x3, &three_x), &P.to_mont(&B));
        let ym = match y {
            Some(y) => {
                let ym = P.to_mont(&y);
                if !bigint::eq(&fmul(&ym, &ym), &rhs).into_bool() {
                    return Err(Error::Encoding);
                }
                ym
            }
            None => {
                // p = 3 mod 4: sqrt(a) = a^((p+1)/4)
                let e = [0x0000000000000000, 0x0000000040000000, 0x4000000000000000, 0x3fffffffc0000000];
                let r = P.pow(&rhs, &e);
                if !bigint::eq(&fmul(&r, &r), &rhs).into_bool() {
                    return Err(Error::Encoding);
                }
                let parity = (P.from_mont(&r)[0] & 1) as u8;
                if parity != (b[0] & 1) { P.neg(&r) } else { r }
            }
        };
        Ok(Point { x: xm, y: ym, z: P.one })
    }

    fn encode_uncompressed(&self) -> Option<[u8; 65]> {
        let (x, y) = self.to_affine()?;
        let mut o = [0u8; 65];
        o[0] = 4;
        o[1..33].copy_from_slice(&bigint::to_be(&x));
        o[33..].copy_from_slice(&bigint::to_be(&y));
        Some(o)
    }
}

/// A validated public key (a point on P-256, not the identity).
#[derive(Clone, Copy, Debug)]
pub struct PublicKey(Point);

impl PublicKey {
    /// SEC 1 decoding (uncompressed `04||x||y` or compressed `02/03||x`), fully validated.
    pub fn from_sec1(b: &[u8]) -> Result<Self, Error> {
        Point::decode(b).map(PublicKey)
    }
    /// Uncompressed SEC 1 encoding.
    pub fn to_sec1_uncompressed(&self) -> [u8; 65] {
        self.0.encode_uncompressed().expect("public keys are never the identity")
    }
}

/// A P-256 private scalar d ∈ [1, n-1]. Zeroized on drop.
pub struct SecretKey {
    d: U256,
}

impl SecretKey {
    /// From 32 big-endian bytes; `Error::Encoding` unless 1 <= d < n. Constant-time range check (its
    /// result is public).
    pub fn from_bytes(b: &[u8; 32]) -> Result<Self, Error> {
        let d = bigint::from_be(b);
        let ok = bigint::lt(&d, &N.m) & !bigint::is_zero(&d);
        if ok.into_bool() { Ok(SecretKey { d }) } else { Err(Error::Encoding) }
    }

    /// The 32-byte big-endian scalar.
    pub fn to_bytes(&self) -> [u8; 32] {
        bigint::to_be(&self.d)
    }

    /// `d·G`. Constant-time.
    pub fn public_key(&self) -> PublicKey {
        PublicKey(Point::generator().mul(&self.d))
    }

    /// ECDH (SP 800-56A §5.7.1.2): the x-coordinate of `d·Q`, 32 bytes big-endian. Constant-time in d.
    /// The peer key was validated when it was decoded; an identity result is `Error::Degenerate`
    /// (impossible for a validated key on a prime-order curve, checked anyway).
    pub fn diffie_hellman(&self, peer: &PublicKey) -> Result<[u8; 32], Error> {
        let s = peer.0.mul(&self.d);
        let (x, _) = s.to_affine().ok_or(Error::Degenerate)?;
        Ok(bigint::to_be(&x))
    }

    /// ECDSA signature over a message digest, RFC 6979 deterministic nonce (HMAC-SHA-256 DRBG), raw
    /// `r || s` (each 32 bytes big-endian). The digest may be any length (FIPS 186-5: its leftmost 256
    /// bits are used).
    pub fn sign_prehashed(&self, digest: &[u8]) -> [u8; 64] {
        let e = bits2int(digest);
        let h1 = bigint::to_be(&N.reduce(&e));
        let x = bigint::to_be(&self.d);
        let mut v = [0x01u8; 32];
        let mut k = [0x00u8; 32];
        for round in [0x00u8, 0x01] {
            let mut m = Hmac::<Sha256>::new(&k);
            m.update(&v);
            m.update(&[round]);
            m.update(&x);
            m.update(&h1);
            m.finalize_into(&mut k);
            let m = Hmac::<Sha256>::new(&k);
            v = m.mac(&v);
        }
        loop {
            let m = Hmac::<Sha256>::new(&k);
            v = m.mac(&v);
            let cand = bigint::from_be(&v);
            let in_range = bigint::lt(&cand, &N.m) & !bigint::is_zero(&cand);
            if in_range.into_bool() {
                if let Some(sig) = self.sign_with_nonce(&e, &cand) {
                    k.zeroize();
                    v.zeroize();
                    return sig;
                }
            }
            let mut m = Hmac::<Sha256>::new(&k);
            m.update(&v);
            m.update(&[0x00]);
            m.finalize_into(&mut k);
            v = Hmac::<Sha256>::new(&k).mac(&v);
        }
    }

    /// The signing equation for a given nonce (`None` if r or s is 0). Exposed only for the CAVP
    /// SigGen known answers, which fix k; never call it with a nonce you did not draw fresh.
    #[doc(hidden)]
    pub fn sign_prehashed_with_nonce_hazmat(&self, digest: &[u8], k: &[u8; 32]) -> Option<[u8; 64]> {
        let kk = bigint::from_be(k);
        if !(bigint::lt(&kk, &N.m) & !bigint::is_zero(&kk)).into_bool() {
            return None;
        }
        self.sign_with_nonce(&bits2int(digest), &kk)
    }

    fn sign_with_nonce(&self, e: &U256, k: &U256) -> Option<[u8; 64]> {
        let (rx, _) = Point::generator().mul(k).to_affine()?;
        let r = N.reduce(&rx);
        if bigint::is_zero(&r).into_bool() {
            return None;
        }
        // s = k^-1 (e + r d) mod n, all in Montgomery form
        let km = N.to_mont(k);
        let kinv = N.inv(&km);
        let rd = N.mul(&N.to_mont(&r), &N.reduce(&self.d)); // r*d (plain)
        let sum = N.add(&N.reduce(e), &rd);
        let s = N.mul(&kinv, &sum); // k^-1 * R * ... : kinv is k^-1·R, times sum·1 → k^-1·sum (plain)
        if bigint::is_zero(&s).into_bool() {
            return None;
        }
        let mut sig = [0u8; 64];
        sig[..32].copy_from_slice(&bigint::to_be(&r));
        sig[32..].copy_from_slice(&bigint::to_be(&s));
        Some(sig)
    }

    /// ECDSA-SHA-256 over a message.
    pub fn sign_sha256(&self, msg: &[u8]) -> [u8; 64] {
        self.sign_prehashed(&crate::sha2::sha256(msg))
    }
}

impl Drop for SecretKey {
    fn drop(&mut self) {
        self.d.zeroize();
    }
}

/// FIPS 186-5 / RFC 6979 bits2int for a 256-bit order: the leftmost 256 bits of the digest as an
/// integer (a shorter digest is taken whole).
fn bits2int(d: &[u8]) -> U256 {
    let mut b = [0u8; 32];
    if d.len() >= 32 {
        b.copy_from_slice(&d[..32]);
    } else {
        b[32 - d.len()..].copy_from_slice(d);
    }
    bigint::from_be(&b)
}

/// ECDSA verification (FIPS 186-5 §6.4.2) of a raw `r || s` signature over a digest. Variable-time
/// (public inputs).
pub fn verify_prehashed(key: &PublicKey, digest: &[u8], sig: &[u8; 64]) -> Result<(), Error> {
    let r = bigint::from_be(sig[..32].try_into().unwrap());
    let s = bigint::from_be(sig[32..].try_into().unwrap());
    for v in [&r, &s] {
        if bigint::is_zero(v).into_bool() || !bigint::lt(v, &N.m).into_bool() {
            return Err(Error::Auth);
        }
    }
    let e = N.reduce(&bits2int(digest));
    let w = N.inv(&N.to_mont(&s)); // s^-1 · R
    let u1 = N.mul(&w, &e); // s^-1 · e (plain)
    let u2 = N.mul(&w, &r); // s^-1 · r (plain)
    let pt = Point::generator().mul(&u1).add(&key.0.mul(&u2));
    let (x, _) = pt.to_affine().ok_or(Error::Auth)?;
    if bigint::eq(&N.reduce(&x), &r).into_bool() { Ok(()) } else { Err(Error::Auth) }
}

/// ECDSA-SHA-256 verification of a raw `r || s` signature.
pub fn verify_sha256(key: &PublicKey, msg: &[u8], sig: &[u8; 64]) -> Result<(), Error> {
    verify_prehashed(key, &crate::sha2::sha256(msg), sig)
}

/// Strict DER (X.690) decoding of `SEQUENCE { INTEGER r, INTEGER s }` into raw `r || s`: definite
/// minimal lengths, positive minimal integers (no superfluous leading zero, no negative), r and s at
/// most 32 bytes of magnitude, nothing trailing. Anything else is `Error::Encoding`.
pub fn signature_from_der(der: &[u8]) -> Result<[u8; 64], Error> {
    let mut sig = [0u8; 64];
    der_sig_decode(der, &mut sig, 32)?;
    Ok(sig)
}

/// The strict DER reader shared with P-384: `out` is `2 * w` bytes, r and s right-aligned in `w` each.
pub(crate) fn der_sig_decode(der: &[u8], out: &mut [u8], w: usize) -> Result<(), Error> {
    fn len(b: &[u8], i: &mut usize) -> Result<usize, Error> {
        let l0 = *b.get(*i).ok_or(Error::Encoding)?;
        *i += 1;
        if l0 < 0x80 {
            return Ok(l0 as usize);
        }
        if l0 == 0x81 {
            let l = *b.get(*i).ok_or(Error::Encoding)?;
            *i += 1;
            if l < 0x80 {
                return Err(Error::Encoding); // not minimal
            }
            return Ok(l as usize);
        }
        Err(Error::Encoding) // an ECDSA P-256/P-384 signature is never longer than 255 bytes
    }
    fn int(b: &[u8], i: &mut usize, out: &mut [u8]) -> Result<(), Error> {
        if b.get(*i) != Some(&0x02) {
            return Err(Error::Encoding);
        }
        *i += 1;
        let l = len(b, i)?;
        let v = b.get(*i..*i + l).ok_or(Error::Encoding)?;
        *i += l;
        if v.is_empty() || v[0] & 0x80 != 0 {
            return Err(Error::Encoding); // empty or negative
        }
        if v.len() > 1 && v[0] == 0 && v[1] & 0x80 == 0 {
            return Err(Error::Encoding); // superfluous leading zero
        }
        let mag = if v[0] == 0 { &v[1..] } else { v };
        if mag.len() > out.len() {
            return Err(Error::Encoding);
        }
        let w = out.len();
        out[w - mag.len()..].copy_from_slice(mag);
        Ok(())
    }
    let mut i = 0;
    if der.first() != Some(&0x30) {
        return Err(Error::Encoding);
    }
    i += 1;
    let l = len(der, &mut i)?;
    if i + l != der.len() {
        return Err(Error::Encoding);
    }
    let (r, s) = out.split_at_mut(w);
    int(der, &mut i, r)?;
    int(der, &mut i, s)?;
    if i != der.len() {
        return Err(Error::Encoding);
    }
    Ok(())
}

/// DER encoding of a raw `r || s` signature (at most 72 bytes); returns the length written.
pub fn signature_to_der(sig: &[u8; 64], out: &mut [u8; 72]) -> usize {
    fn put(v: &[u8], o: &mut [u8], at: &mut usize) {
        let mut s = 0;
        while s < v.len() - 1 && v[s] == 0 {
            s += 1;
        }
        let v = &v[s..];
        let pad = v[0] & 0x80 != 0;
        o[*at] = 0x02;
        o[*at + 1] = (v.len() + pad as usize) as u8;
        *at += 2;
        if pad {
            o[*at] = 0;
            *at += 1;
        }
        o[*at..*at + v.len()].copy_from_slice(v);
        *at += v.len();
    }
    let mut body = [0u8; 70];
    let mut n = 0;
    put(&sig[..32], &mut body, &mut n);
    put(&sig[32..], &mut body, &mut n);
    out[0] = 0x30;
    out[1] = n as u8;
    out[2..2 + n].copy_from_slice(&body[..n]);
    n + 2
}
