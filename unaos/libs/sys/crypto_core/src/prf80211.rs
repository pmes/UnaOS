// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The IEEE 802.11 pseudorandom function PRF-n (IEEE Std 802.11-2020 §12.7.1.2) — WPA2CRYPTO
//! (rmbp-ledger B882).
//!
//! CHARTER: Holocron — shared-core. Arithmetic only (crypto_core's own charter): the RSN key hierarchy's
//! PRF, with no key store, no policy and no network code; the station's handshake (WPA2JOIN) calls it.
//!
//! The standard's definition, with `H-SHA-1(K, A, B, X) = HMAC-SHA-1(K, A || Y || B || X)`, `Y` a single
//! octet holding 0 and `X` a single octet holding the counter:
//!
//! ```text
//! PRF-n(K, A, B):
//!     for i <- 0 to (n + 159) / 160 - 1 do
//!         R <- R || H-SHA-1(K, A, B, i)
//!     return L(R, 0, n)
//! ```
//!
//! The PTK, for example, is `PRF-384/512/704(PMK, "Pairwise key expansion", Min(AA,SPA) || Max(AA,SPA) ||
//! Min(ANonce,SNonce) || Max(ANonce,SNonce))`; `data` takes those pieces as parts, so the caller never
//! concatenates. HMAC and SHA-1 are this crate's ([`Hmac`], [`Sha1`]) — the same building blocks as
//! [`crate::tls12_prf`], iterated by a counter octet instead of an `A(i)` chain.
//!
//! CONSTANT-TIME: as HMAC (key, label and data contents); the output length is public.

use crate::ct::Zeroize;
use crate::hmac::{Hmac, MAX_OUTPUT};
use crate::sha1::Sha1;
use crate::Error;

/// SHA-1's output: one PRF block (160 bits).
const BLOCK: usize = 20;

/// The longest PRF output: the counter is one octet, so at most 256 blocks; 255 full blocks bounds it
/// conservatively at 5100 bytes (every PRF-n the standard uses is at most PRF-704).
pub const MAX_OUT: usize = 255 * BLOCK;

/// `PRF-n(key, label, data)` into `out`, `n = 8 * out.len()` (IEEE 802.11-2020 §12.7.1.2). `data` is the
/// concatenation of its parts. `Error::Length` when `out` is empty or longer than [`MAX_OUT`].
pub fn prf(key: &[u8], label: &[u8], data: &[&[u8]], out: &mut [u8]) -> Result<(), Error> {
    if out.is_empty() || out.len() > MAX_OUT {
        return Err(Error::Length);
    }
    let mac = Hmac::<Sha1>::new(key);
    let mut block = [0u8; MAX_OUTPUT];
    for (i, chunk) in out.chunks_mut(BLOCK).enumerate() {
        let mut m = mac.clone();
        m.update(label);
        m.update(&[0u8]);
        for d in data {
            m.update(d);
        }
        m.update(&[i as u8]);
        m.finalize_into(&mut block[..BLOCK]);
        chunk.copy_from_slice(&block[..chunk.len()]);
    }
    block.zeroize();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// IEEE 802.11-2020 Annex J.3, test case 1 (PRF-192); the full J.3 set is `kat/m8.rs`.
    #[test]
    fn annex_j3_case1() {
        let mut out = [0u8; 24];
        prf(&[0x0b; 20], b"prefix", &[b"Hi There"], &mut out).unwrap();
        let want: [u8; 24] = [
            0xbc, 0xd4, 0xc6, 0x50, 0xb3, 0x0b, 0x96, 0x84, 0x95, 0x18, 0x29, 0xe0, 0xd7, 0x5f, 0x9d, 0x54, 0xb8, 0x62, 0x17,
            0x5e, 0xd9, 0xf0, 0x06, 0x06,
        ];
        assert_eq!(out, want);
        assert_eq!(prf(b"k", b"l", &[], &mut []), Err(Error::Length));
    }
}
