// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! AES Key Wrap (RFC 3394; the KW mode of NIST SP 800-38F) over the bitsliced [`Aes`] — WPA2CRYPTO
//! (rmbp-ledger B882).
//!
//! CHARTER: Holocron — shared-core. Arithmetic only (crypto_core's own charter): no key store. The RSN
//! 4-way handshake carries the GTK (and IGTK) in EAPOL-Key Key Data wrapped under the KEK with this mode
//! (IEEE 802.11-2020 §12.7.2); WPA2JOIN calls [`unwrap`].
//!
//! The index-based form (RFC 3394 §2.2.1 / §2.2.2), `n` 64-bit key blocks `R[1..n]`, `n >= 2`:
//!
//! ```text
//! wrap:    A = IV;  for j = 0..5, i = 1..n:  B = AES(K, A ‖ R[i]);  A = MSB64(B) ⊕ t;  R[i] = LSB64(B)
//! unwrap:  A = C0;  for j = 5..0, i = n..1:  B = AES⁻¹(K, (A ⊕ t) ‖ R[i]);  A = MSB64(B);  R[i] = LSB64(B)
//!          t = n·j + i (big-endian, XORed into A);  integrity: A must equal the IV (§2.2.3)
//! ```
//!
//! CONSTANT-TIME: yes in KEK and key contents ([`Aes`] is table-free); the IV check is
//! [`crate::ct::ct_eq`], and a failed check zeroizes the output before `Error::Auth`. Lengths are public.

use crate::aes::Aes;
use crate::ct::{ct_eq, Zeroize};
use crate::Error;

/// The default initial value (RFC 3394 §2.2.3.1).
pub const DEFAULT_IV: [u8; 8] = [0xA6; 8];

/// Wrap `key` (n >= 2 whole 64-bit blocks) under `kek` (16, 24 or 32 bytes) into `out`
/// (`key.len() + 8` bytes) with [`DEFAULT_IV`]. `Error::Length` for any other size.
pub fn wrap(kek: &[u8], key: &[u8], out: &mut [u8]) -> Result<(), Error> {
    if key.len() % 8 != 0 || key.len() < 16 || out.len() != key.len() + 8 {
        return Err(Error::Length);
    }
    let aes = Aes::new(kek)?;
    let n = key.len() / 8;
    out[8..].copy_from_slice(key);
    let mut a = DEFAULT_IV;
    let mut b = [0u8; 16];
    for j in 0..6u64 {
        for i in 1..=n {
            let r = &mut out[8 * i..8 * i + 8];
            b[..8].copy_from_slice(&a);
            b[8..].copy_from_slice(r);
            aes.encrypt_block(&mut b);
            let t = (n as u64) * j + i as u64;
            let msb = u64::from_be_bytes(b[..8].try_into().unwrap()) ^ t;
            a = msb.to_be_bytes();
            r.copy_from_slice(&b[8..]);
        }
    }
    out[..8].copy_from_slice(&a);
    b.zeroize();
    Ok(())
}

/// Unwrap `wrapped` (`8·(n+1)` bytes, n >= 2) under `kek` into `out` (`wrapped.len() - 8` bytes) and check
/// the [`DEFAULT_IV`]. `Error::Auth` (with `out` zeroized) when the check fails; `Error::Length` for any
/// other size.
pub fn unwrap(kek: &[u8], wrapped: &[u8], out: &mut [u8]) -> Result<(), Error> {
    if wrapped.len() % 8 != 0 || wrapped.len() < 24 || out.len() + 8 != wrapped.len() {
        return Err(Error::Length);
    }
    let aes = Aes::new(kek)?;
    let n = out.len() / 8;
    let mut a = [0u8; 8];
    a.copy_from_slice(&wrapped[..8]);
    out.copy_from_slice(&wrapped[8..]);
    let mut b = [0u8; 16];
    for j in (0..6u64).rev() {
        for i in (1..=n).rev() {
            let r = &mut out[8 * (i - 1)..8 * i];
            let t = (n as u64) * j + i as u64;
            let x = u64::from_be_bytes(a) ^ t;
            b[..8].copy_from_slice(&x.to_be_bytes());
            b[8..].copy_from_slice(r);
            aes.decrypt_block(&mut b);
            a.copy_from_slice(&b[..8]);
            r.copy_from_slice(&b[8..]);
        }
    }
    b.zeroize();
    if !ct_eq(&a, &DEFAULT_IV) {
        out.zeroize();
        return Err(Error::Auth);
    }
    Ok(())
}
