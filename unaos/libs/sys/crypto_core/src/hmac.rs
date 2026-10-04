// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! HMAC (RFC 2104, FIPS 198-1) over any [`Digest`].
//!
//! The two padded-key prefix states (`H(K ^ ipad)`, `H(K ^ opad)`) are computed ONCE at [`Hmac::new`];
//! [`Hmac::mac_into`] forks them per message, so a PBKDF2 iteration costs exactly two compressions for
//! SHA-256 (the property the kernel's SECLOGIN M1 port relied on, kept).
//!
//! CONSTANT-TIME: yes in the key and message contents (SHA-2 is; the key padding is a fixed-length
//! XOR). The key LENGTH is public (a key longer than the block is hashed first, RFC 2104 §2).
//! [`Hmac::verify`] compares tags with [`crate::ct::ct_eq`].

use crate::ct::{ct_eq, Zeroize};
use crate::sha2::{Digest, Sha256, Sha384, Sha512};

/// Largest block of any supported digest (SHA-512: 128 bytes).
const MAX_BLOCK: usize = 128;
/// Largest output of any supported digest (SHA-512: 64 bytes).
pub const MAX_OUTPUT: usize = 64;

/// A keyed HMAC. `update` + `finalize_into` streams one message; `mac_into` is the one-shot fork that
/// leaves the keyed state reusable.
#[derive(Clone)]
pub struct Hmac<D: Digest> {
    inner_init: D,
    outer_init: D,
    inner: D,
}

impl<D: Digest> Hmac<D> {
    /// Key the MAC (any key length; RFC 2104 §2: a key longer than the block is replaced by its hash).
    pub fn new(key: &[u8]) -> Self {
        let b = D::BLOCK_LEN;
        let mut k = [0u8; MAX_BLOCK];
        if key.len() > b {
            let mut h = D::new();
            h.update(key);
            h.finalize_into(&mut k[..D::OUTPUT_LEN]);
        } else {
            k[..key.len()].copy_from_slice(key);
        }
        let mut pad = [0u8; MAX_BLOCK];
        for i in 0..b {
            pad[i] = k[i] ^ 0x36;
        }
        let mut inner = D::new();
        inner.update(&pad[..b]);
        for i in 0..b {
            pad[i] = k[i] ^ 0x5c;
        }
        let mut outer = D::new();
        outer.update(&pad[..b]);
        k.zeroize();
        pad.zeroize();
        Hmac { inner_init: inner.clone(), outer_init: outer, inner }
    }

    /// Absorb message bytes.
    pub fn update(&mut self, data: &[u8]) {
        self.inner.update(data);
    }

    /// Finish the streamed message into `out[..D::OUTPUT_LEN]` and reset for the next message.
    pub fn finalize_into(&mut self, out: &mut [u8]) {
        let inner = core::mem::replace(&mut self.inner, self.inner_init.clone());
        let mut d = [0u8; MAX_OUTPUT];
        inner.finalize_into(&mut d[..D::OUTPUT_LEN]);
        let mut o = self.outer_init.clone();
        o.update(&d[..D::OUTPUT_LEN]);
        o.finalize_into(out);
        d.zeroize();
    }

    /// One-shot HMAC(key, msg) into `out[..D::OUTPUT_LEN]`; the keyed state is untouched.
    pub fn mac_into(&self, msg: &[u8], out: &mut [u8]) {
        let mut i = self.inner_init.clone();
        i.update(msg);
        let mut d = [0u8; MAX_OUTPUT];
        i.finalize_into(&mut d[..D::OUTPUT_LEN]);
        let mut o = self.outer_init.clone();
        o.update(&d[..D::OUTPUT_LEN]);
        o.finalize_into(out);
        d.zeroize();
    }

    /// Finish the streamed message and compare against `tag` (which may be a truncation, FIPS 198-1
    /// §5: at least half the output and never under 10 bytes is the conventional floor — enforced:
    /// `Error::Length` below 10 bytes). Constant-time compare.
    pub fn verify(&mut self, tag: &[u8]) -> Result<(), crate::Error> {
        if tag.len() < 10 || tag.len() > D::OUTPUT_LEN {
            return Err(crate::Error::Length);
        }
        let mut t = [0u8; MAX_OUTPUT];
        self.finalize_into(&mut t);
        let ok = ct_eq(&t[..tag.len()], tag);
        t.zeroize();
        if ok { Ok(()) } else { Err(crate::Error::Auth) }
    }
}

/// HMAC-SHA256 — the kernel's `HmacSha256` name, kept.
pub type HmacSha256 = Hmac<Sha256>;
/// HMAC-SHA384.
pub type HmacSha384 = Hmac<Sha384>;
/// HMAC-SHA512.
pub type HmacSha512 = Hmac<Sha512>;

impl Hmac<Sha256> {
    /// HMAC-SHA256(key, msg) as an array (the kernel's `HmacSha256::mac`).
    pub fn mac(&self, msg: &[u8]) -> [u8; 32] {
        let mut out = [0u8; 32];
        self.mac_into(msg, &mut out);
        out
    }
}

/// One-shot HMAC-SHA256.
pub fn hmac_sha256(key: &[u8], msg: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    Hmac::<Sha256>::new(key).mac_into(msg, &mut out);
    out
}
/// One-shot HMAC-SHA384.
pub fn hmac_sha384(key: &[u8], msg: &[u8]) -> [u8; 48] {
    let mut out = [0u8; 48];
    Hmac::<Sha384>::new(key).mac_into(msg, &mut out);
    out
}
/// One-shot HMAC-SHA512.
pub fn hmac_sha512(key: &[u8], msg: &[u8]) -> [u8; 64] {
    let mut out = [0u8; 64];
    Hmac::<Sha512>::new(key).mac_into(msg, &mut out);
    out
}
