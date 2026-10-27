// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! AES-CCM (RFC 3610; NIST SP 800-38C): CBC-MAC then CTR, over the bitsliced [`Aes`] — WPA2CRYPTO
//! (rmbp-ledger B882).
//!
//! CHARTER: Holocron — shared-core. Arithmetic only (crypto_core's own charter): no key store, no frame
//! format; the station's CCMP (WPA2JOIN) builds its nonce and AAD and calls this.
//!
//! **The 802.11 instantiation.** CCMP (IEEE 802.11-2020 §12.5.3) is CCM with **M = 8** (an 8-octet MIC, the
//! `tag` slice is 8 bytes) and **L = 2** (a 2-octet length field), so the nonce is **13 octets** (Priority ‖ A2 ‖
//! PN, §12.5.3.3.4) and the AAD is the frame's constructed AAD (§12.5.3.3.3). That is the call WPA2JOIN makes:
//! `AesCcm::new(&tk)?.encrypt(&nonce13, &aad, payload, &mut mic8)`. CCMP-256 / GCMP are not this module.
//!
//! **The generic mode** (RFC 3610 §2): `M = tag.len()` ∈ {4, 6, 8, 10, 12, 14, 16} and `L = 15 - nonce.len()`
//! ∈ 2..=8 (a 7- to 13-octet nonce), message length `< 2^(8L)`.
//!
//! ```text
//! B_0   = Flags ‖ N ‖ l(m)            Flags = 64·Adata + 8·((M-2)/2) + (L-1)
//! AAD   = l(a) encoded (2 octets if l(a) < 2^16-2^8; FF FE ‖ 4 octets if < 2^32; FF FF ‖ 8 octets), ‖ a, zero-padded
//! T     = first M octets of CBC-MAC_K(B_0 ‖ AAD blocks ‖ m zero-padded)
//! A_i   = (L-1) ‖ N ‖ i (L octets)    S_i = E(K, A_i)
//! c     = m ⊕ (S_1 ‖ S_2 ‖ …)         U = T ⊕ first M octets of S_0
//! ```
//!
//! CONSTANT-TIME: yes in key, nonce, AAD and message contents ([`Aes`] is table-free; no branch on data). The
//! tag compare is [`crate::ct::ct_eq`]. On a failed tag the keystream is applied again, so the buffer is
//! ciphertext once more and no unauthenticated plaintext is released (gcm's rule). Lengths are public.

use crate::aes::{Aes, BLOCK};
use crate::ct::{ct_eq, Zeroize};
use crate::Error;

/// AES-CCM keyed once (16-, 24- or 32-byte key); every call takes its own nonce. The [`Aes`] schedule is
/// zeroized on drop.
#[derive(Clone)]
pub struct AesCcm {
    aes: Aes,
}

/// The tag lengths RFC 3610 §2 permits (bytes).
pub fn tag_len_ok(m: usize) -> bool {
    matches!(m, 4 | 6 | 8 | 10 | 12 | 14 | 16)
}

/// The streaming CBC-MAC state: `x` is the chaining value, `pos` the bytes XORed into the current block.
struct CbcMac<'a> {
    aes: &'a Aes,
    x: [u8; BLOCK],
    pos: usize,
}

impl CbcMac<'_> {
    fn absorb(&mut self, data: &[u8]) {
        for &b in data {
            self.x[self.pos] ^= b;
            self.pos += 1;
            if self.pos == BLOCK {
                self.aes.encrypt_block(&mut self.x);
                self.pos = 0;
            }
        }
    }
    /// Zero-pad the current block (encrypt it if anything was absorbed into it).
    fn pad(&mut self) {
        if self.pos != 0 {
            self.aes.encrypt_block(&mut self.x);
            self.pos = 0;
        }
    }
}

impl AesCcm {
    /// Key the AEAD (16, 24 or 32 bytes; CCMP's TK is 16).
    pub fn new(key: &[u8]) -> Result<Self, Error> {
        Ok(AesCcm { aes: Aes::new(key)? })
    }

    /// The length parameter L, or `Error::Length` for a nonce, tag or message outside RFC 3610 §2.
    fn params(nonce: &[u8], m: usize, len: usize) -> Result<usize, Error> {
        if !(7..=13).contains(&nonce.len()) || !tag_len_ok(m) {
            return Err(Error::Length);
        }
        let l = 15 - nonce.len();
        if l < 8 && (len as u64) >> (8 * l) != 0 {
            return Err(Error::Length);
        }
        Ok(l)
    }

    /// Counter block `A_i` (RFC 3610 §2.3).
    fn a_block(nonce: &[u8], l: usize, i: u64) -> [u8; BLOCK] {
        let mut a = [0u8; BLOCK];
        a[0] = (l - 1) as u8;
        a[1..1 + nonce.len()].copy_from_slice(nonce);
        a[1 + nonce.len()..].copy_from_slice(&i.to_be_bytes()[8 - l..]);
        a
    }

    /// XOR `S_1 ‖ S_2 ‖ …` into `data`: crate::ctr from `A_1` with an 8·L-bit counter (CRYPTOONE-2, SR342; `params`
    /// bounds the message below 2^(8L) octets, so the counter never wraps).
    fn ctr(&self, nonce: &[u8], l: usize, data: &mut [u8]) {
        crate::ctr::ctr(&self.aes, &Self::a_block(nonce, l, 1), (8 * l) as u8, data);
    }

    /// The encrypted tag `U = T ⊕ S_0` over (aad, plaintext `msg`), into `u[..m]`.
    fn auth(&self, nonce: &[u8], l: usize, m: usize, aad: &[u8], msg: &[u8], u: &mut [u8; BLOCK]) {
        let mut b0 = [0u8; BLOCK];
        b0[0] = (if aad.is_empty() { 0 } else { 0x40 }) | ((((m - 2) / 2) as u8) << 3) | (l - 1) as u8;
        b0[1..1 + nonce.len()].copy_from_slice(nonce);
        b0[1 + nonce.len()..].copy_from_slice(&(msg.len() as u64).to_be_bytes()[8 - l..]);
        let mut mac = CbcMac { aes: &self.aes, x: [0u8; BLOCK], pos: 0 };
        mac.absorb(&b0);
        if !aad.is_empty() {
            let la = aad.len() as u64;
            if la < 0xFF00 {
                mac.absorb(&(la as u16).to_be_bytes());
            } else if la <= u32::MAX as u64 {
                mac.absorb(&[0xFF, 0xFE]);
                mac.absorb(&(la as u32).to_be_bytes());
            } else {
                mac.absorb(&[0xFF, 0xFF]);
                mac.absorb(&la.to_be_bytes());
            }
            mac.absorb(aad);
            mac.pad();
        }
        mac.absorb(msg);
        mac.pad();
        let mut s0 = Self::a_block(nonce, l, 0);
        self.aes.encrypt_block(&mut s0);
        for k in 0..m {
            u[k] = mac.x[k] ^ s0[k];
        }
        mac.x.zeroize();
        s0.zeroize();
        b0.zeroize();
    }

    /// Encrypt `buf` in place under `nonce` (7..13 bytes; CCMP: 13) and write the `tag.len()`-byte tag
    /// (4..16, even; CCMP: 8) into `tag`. `Error::Length` for a nonce, tag or message RFC 3610 §2 refuses.
    pub fn encrypt(&self, nonce: &[u8], aad: &[u8], buf: &mut [u8], tag: &mut [u8]) -> Result<(), Error> {
        let m = tag.len();
        let l = Self::params(nonce, m, buf.len())?;
        let mut u = [0u8; BLOCK];
        self.auth(nonce, l, m, aad, buf, &mut u);
        self.ctr(nonce, l, buf);
        tag.copy_from_slice(&u[..m]);
        u.zeroize();
        Ok(())
    }

    /// Decrypt `buf` in place and verify `tag` (constant-time). `Error::Auth` on a mismatch, with `buf`
    /// returned to the ciphertext it was; `Error::Length` as [`AesCcm::encrypt`].
    pub fn decrypt(&self, nonce: &[u8], aad: &[u8], buf: &mut [u8], tag: &[u8]) -> Result<(), Error> {
        let m = tag.len();
        let l = Self::params(nonce, m, buf.len())?;
        self.ctr(nonce, l, buf);
        let mut u = [0u8; BLOCK];
        self.auth(nonce, l, m, aad, buf, &mut u);
        let ok = ct_eq(&u[..m], tag);
        u.zeroize();
        if !ok {
            self.ctr(nonce, l, buf);
            return Err(Error::Auth);
        }
        Ok(())
    }
}

/// One-shot CCM encrypt (expands `key` for this call): [`AesCcm::encrypt`].
pub fn ccm_encrypt(key: &[u8], nonce: &[u8], aad: &[u8], buf: &mut [u8], tag: &mut [u8]) -> Result<(), Error> {
    AesCcm::new(key)?.encrypt(nonce, aad, buf, tag)
}

/// One-shot CCM decrypt-and-verify (expands `key` for this call): [`AesCcm::decrypt`].
pub fn ccm_decrypt(key: &[u8], nonce: &[u8], aad: &[u8], buf: &mut [u8], tag: &[u8]) -> Result<(), Error> {
    AesCcm::new(key)?.decrypt(nonce, aad, buf, tag)
}
