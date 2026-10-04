// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! HKDF (RFC 5869) over any [`Digest`] — the TLS 1.3 key schedule's KDF.
//!
//! CONSTANT-TIME: yes in IKM, salt, PRK and info contents (HMAC is); the lengths are public.

use crate::ct::Zeroize;
use crate::hmac::{Hmac, MAX_OUTPUT};
use crate::sha2::Digest;
use crate::Error;

/// HKDF-Extract (§2.2): `PRK = HMAC-Hash(salt, IKM)`; an empty salt means `HashLen` zeros (which HMAC's
/// zero-padding makes identical to the empty key). Writes `prk[..D::OUTPUT_LEN]`.
pub fn extract<D: Digest>(salt: &[u8], ikm: &[u8], prk: &mut [u8]) {
    Hmac::<D>::new(salt).mac_into(ikm, prk);
}

/// HKDF-Expand (§2.3): fills `okm` (`L = okm.len() <= 255 * HashLen`, else `Error::Length`).
/// `prk` should be at least `HashLen` bytes (RFC 5869 §2.3); a shorter PRK is accepted, as the RFC's
/// own HMAC would accept it, but it is the caller's weakness.
pub fn expand<D: Digest>(prk: &[u8], info: &[u8], okm: &mut [u8]) -> Result<(), Error> {
    let n = D::OUTPUT_LEN;
    if okm.len() > 255 * n {
        return Err(Error::Length);
    }
    let mac = Hmac::<D>::new(prk);
    let mut t = [0u8; MAX_OUTPUT];
    let mut t_len = 0usize;
    let mut counter = 1u8;
    for chunk in okm.chunks_mut(n) {
        let mut h = mac.clone();
        h.update(&t[..t_len]);
        h.update(info);
        h.update(&[counter]);
        h.finalize_into(&mut t);
        t_len = n;
        chunk.copy_from_slice(&t[..chunk.len()]);
        counter = counter.wrapping_add(1);
    }
    t.zeroize();
    Ok(())
}

/// Extract-then-expand in one call.
pub fn hkdf<D: Digest>(salt: &[u8], ikm: &[u8], info: &[u8], okm: &mut [u8]) -> Result<(), Error> {
    let mut prk = [0u8; MAX_OUTPUT];
    extract::<D>(salt, ikm, &mut prk);
    let r = expand::<D>(&prk[..D::OUTPUT_LEN], info, okm);
    prk.zeroize();
    r
}
