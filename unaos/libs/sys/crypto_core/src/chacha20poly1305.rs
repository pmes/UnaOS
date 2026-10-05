// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! ChaCha20-Poly1305 AEAD (RFC 8439 §2.6–§2.8), plus XChaCha20-Poly1305 (24-byte nonce via HChaCha20).
//!
//! CONSTANT-TIME: yes in key, nonce, plaintext and tag (ChaCha20 and Poly1305 are; the tag compare is
//! [`crate::ct::ct_eq`]). On a failed tag the buffer is left as CIPHERTEXT — decryption happens only
//! after the tag verified, so no unauthenticated plaintext is ever released. Lengths are public.

use crate::chacha20::{block, hchacha20, ChaCha20};
use crate::ct::{ct_eq, Zeroize};
use crate::poly1305::Poly1305;
use crate::Error;

/// Longest plaintext RFC 8439 allows: 2^32 - 1 blocks of 64 bytes after the Poly1305 key block
/// (2^38 - 64 bytes).
pub const MAX_PLAINTEXT: u64 = (1u64 << 38) - 64;

fn mac(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], ct: &[u8]) -> [u8; 16] {
    let mut otk = [0u8; 32];
    otk.copy_from_slice(&block(key, 0, nonce)[..32]); // §2.6 Poly1305 key generation
    let mut p = Poly1305::new(&otk);
    otk.zeroize();
    p.update(aad);
    p.pad16();
    p.update(ct);
    p.pad16();
    p.update(&(aad.len() as u64).to_le_bytes());
    p.update(&(ct.len() as u64).to_le_bytes());
    p.finalize()
}

/// Encrypt `buf` in place; returns the 16-byte tag.
pub fn seal_in_place(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], buf: &mut [u8]) -> Result<[u8; 16], Error> {
    if buf.len() as u64 > MAX_PLAINTEXT {
        return Err(Error::Length);
    }
    ChaCha20::new(key, nonce, 1).apply_keystream(buf);
    Ok(mac(key, nonce, aad, buf))
}

/// Verify `tag` over (aad, ciphertext `buf`) and, only if it verifies, decrypt `buf` in place.
pub fn open_in_place(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], buf: &mut [u8], tag: &[u8; 16]) -> Result<(), Error> {
    if buf.len() as u64 > MAX_PLAINTEXT {
        return Err(Error::Length);
    }
    let want = mac(key, nonce, aad, buf);
    if !ct_eq(&want, tag) {
        return Err(Error::Auth);
    }
    ChaCha20::new(key, nonce, 1).apply_keystream(buf);
    Ok(())
}

fn xkey(key: &[u8; 32], nonce: &[u8; 24]) -> ([u8; 32], [u8; 12]) {
    let sub = hchacha20(key, nonce[..16].try_into().unwrap());
    let mut n = [0u8; 12];
    n[4..].copy_from_slice(&nonce[16..]);
    (sub, n)
}

/// XChaCha20-Poly1305 seal (24-byte nonce, safe to draw at random).
pub fn xseal_in_place(key: &[u8; 32], nonce: &[u8; 24], aad: &[u8], buf: &mut [u8]) -> Result<[u8; 16], Error> {
    let (mut sub, n) = xkey(key, nonce);
    let r = seal_in_place(&sub, &n, aad, buf);
    sub.zeroize();
    r
}

/// XChaCha20-Poly1305 open.
pub fn xopen_in_place(key: &[u8; 32], nonce: &[u8; 24], aad: &[u8], buf: &mut [u8], tag: &[u8; 16]) -> Result<(), Error> {
    let (mut sub, n) = xkey(key, nonce);
    let r = open_in_place(&sub, &n, aad, buf, tag);
    sub.zeroize();
    r
}

/// `ciphertext || tag` as a new vector.
#[cfg(feature = "alloc")]
pub fn seal(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], plaintext: &[u8]) -> Result<alloc::vec::Vec<u8>, Error> {
    let mut v = alloc::vec::Vec::with_capacity(plaintext.len() + 16);
    v.extend_from_slice(plaintext);
    let tag = seal_in_place(key, nonce, aad, &mut v)?;
    v.extend_from_slice(&tag);
    Ok(v)
}

/// Open `ciphertext || tag` into a new vector.
#[cfg(feature = "alloc")]
pub fn open(key: &[u8; 32], nonce: &[u8; 12], aad: &[u8], sealed: &[u8]) -> Result<alloc::vec::Vec<u8>, Error> {
    if sealed.len() < 16 {
        return Err(Error::Length);
    }
    let (ct, tag) = sealed.split_at(sealed.len() - 16);
    let mut v = ct.to_vec();
    open_in_place(key, nonce, aad, &mut v, tag.try_into().unwrap())?;
    Ok(v)
}
