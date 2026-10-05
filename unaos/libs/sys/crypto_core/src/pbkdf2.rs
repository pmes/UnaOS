// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! PBKDF2 (RFC 8018 §5.2) with HMAC over any [`Digest`] as the PRF, any output length.
//!
//! Moved here from the kernel's `hash.rs` (SECLOGIN M1 PWHARD, where it stretched `fs/users.rs`
//! credentials, first block only); generalised to every `dkLen`, no allocation, no salt bound (the salt
//! is streamed into the PRF, not copied into a buffer).
//!
//! CONSTANT-TIME: yes in password and salt contents; the iteration count and lengths are public.
//! (For NEW password storage prefer [`crate::argon2`] — PBKDF2 is memory-cheap and so GPU-cheap.)

use crate::ct::Zeroize;
use crate::hmac::{Hmac, MAX_OUTPUT};
use crate::sha2::{Digest, Sha256};
use crate::Error;

/// `DK = T1 || T2 || …` truncated to `out.len()`, `Ti = U1 ^ … ^ Uc`, `U1 = PRF(P, S || INT(i))`,
/// `Uj = PRF(P, U(j-1))`. `iters == 0` is refused (`Error::Param`, RFC 8018: c is a positive integer);
/// `out.len() > (2^32 - 1) * hLen` is refused (`Error::Length`).
pub fn pbkdf2_hmac<D: Digest>(password: &[u8], salt: &[u8], iters: u32, out: &mut [u8]) -> Result<(), Error> {
    if iters == 0 {
        return Err(Error::Param);
    }
    let h_len = D::OUTPUT_LEN;
    if (out.len() as u64) > (u32::MAX as u64) * (h_len as u64) {
        return Err(Error::Length);
    }
    let prf = Hmac::<D>::new(password);
    let mut u = [0u8; MAX_OUTPUT];
    let mut t = [0u8; MAX_OUTPUT];
    for (i, chunk) in out.chunks_mut(h_len).enumerate() {
        let mut m = prf.clone();
        m.update(salt);
        m.update(&((i as u32) + 1).to_be_bytes());
        m.finalize_into(&mut u);
        t[..h_len].copy_from_slice(&u[..h_len]);
        for _ in 1..iters {
            let prev = u;
            prf.mac_into(&prev[..h_len], &mut u);
            for k in 0..h_len {
                t[k] ^= u[k];
            }
        }
        chunk.copy_from_slice(&t[..chunk.len()]);
    }
    u.zeroize();
    t.zeroize();
    Ok(())
}

/// PBKDF2-HMAC-SHA256 into a 32-byte block — the shape the kernel's credential store calls
/// (`fs/users.rs`). `iters == 0` is treated as 1, exactly as the kernel's original did (its store
/// refuses anything under its own floor before this is ever reached).
pub fn pbkdf2_hmac_sha256(password: &[u8], salt: &[u8], iters: u32, out: &mut [u8; 32]) {
    let _ = pbkdf2_hmac::<Sha256>(password, salt, iters.max(1), out);
}
