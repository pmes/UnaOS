// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! X25519 (RFC 7748 §5): Diffie–Hellman on Curve25519's Montgomery form.
//!
//! CONSTANT-TIME: yes — the Montgomery ladder runs all 255 steps for every scalar, the conditional swap
//! is a mask ([`crate::ct::ct_swap_u64`]), field arithmetic is branch-free; scalar bits are read by a
//! PUBLIC position. The `u`-coordinate is public by definition.

use crate::ct::{Choice, Zeroize};
use crate::field25519::Fe;
use crate::Error;

/// The base point u = 9.
pub const BASEPOINT: [u8; 32] = {
    let mut b = [0u8; 32];
    b[0] = 9;
    b
};

/// RFC 7748 `decodeScalar25519`: clear bits 0–2 and 255, set bit 254.
pub fn clamp(k: &[u8; 32]) -> [u8; 32] {
    let mut s = *k;
    s[0] &= 248;
    s[31] &= 127;
    s[31] |= 64;
    s
}

/// The X25519 function exactly as RFC 7748 §5 defines it: clamp the scalar, mask bit 255 of `u`
/// (non-canonical `u >= p` are accepted and reduced, as the RFC requires), ladder, encode. May return
/// all zeros for a low-order `u`: [`diffie_hellman`] is the checked form.
pub fn x25519(k: &[u8; 32], u: &[u8; 32]) -> [u8; 32] {
    let mut k = clamp(k);
    let x1 = Fe::from_bytes(u);
    let mut x2 = Fe::ONE;
    let mut z2 = Fe::ZERO;
    let mut x3 = x1;
    let mut z3 = Fe::ONE;
    let mut swap = 0u8;
    for t in (0..255).rev() {
        let kt = (k[t / 8] >> (t % 8)) & 1;
        swap ^= kt;
        Fe::cswap(Choice::from_u8(swap), &mut x2, &mut x3);
        Fe::cswap(Choice::from_u8(swap), &mut z2, &mut z3);
        swap = kt;
        let a = x2.add(&z2);
        let aa = a.square();
        let b = x2.sub(&z2);
        let bb = b.square();
        let e = aa.sub(&bb);
        let c = x3.add(&z3);
        let d = x3.sub(&z3);
        let da = d.mul(&a);
        let cb = c.mul(&b);
        x3 = da.add(&cb).square();
        z3 = x1.mul(&da.sub(&cb).square());
        x2 = aa.mul(&bb);
        z2 = e.mul(&aa.add(&e.mul_small(121665)));
    }
    Fe::cswap(Choice::from_u8(swap), &mut x2, &mut x3);
    Fe::cswap(Choice::from_u8(swap), &mut z2, &mut z3);
    k.zeroize();
    let out = x2.mul(&z2.invert()).to_bytes();
    x2.0.zeroize();
    x3.0.zeroize();
    z2.0.zeroize();
    z3.0.zeroize();
    out
}

/// The public key for a 32-byte secret: `X25519(k, 9)`.
pub fn public_key(secret: &[u8; 32]) -> [u8; 32] {
    x25519(secret, &BASEPOINT)
}

/// Checked Diffie–Hellman: `Error::Degenerate` when the shared secret is all zeros (the peer sent a
/// low-order point — RFC 7748 §6.1 "MAY check", TLS 1.3 RFC 8446 §7.4.2 MUST). The zero test is
/// constant-time; its RESULT is public.
pub fn diffie_hellman(secret: &[u8; 32], peer_public: &[u8; 32]) -> Result<[u8; 32], Error> {
    let s = x25519(secret, peer_public);
    if crate::ct::ct_eq(&s, &[0u8; 32]) {
        return Err(Error::Degenerate);
    }
    Ok(s)
}
