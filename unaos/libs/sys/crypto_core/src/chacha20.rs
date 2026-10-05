// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! ChaCha20 (RFC 8439 §2.1–§2.4) and HChaCha20 (draft-irtf-cfrg-xchacha §2.2).
//!
//! CONSTANT-TIME: yes — ARX only (32-bit add, rotate, xor) on a fixed 20-round schedule; no table, no
//! data-dependent branch. Data lengths are public.

use crate::ct::Zeroize;

const SIGMA: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574]; // "expand 32-byte k"

#[inline(always)]
fn qr(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(7);
}

#[inline(always)]
fn rounds(s: &mut [u32; 16]) {
    for _ in 0..10 {
        qr(s, 0, 4, 8, 12);
        qr(s, 1, 5, 9, 13);
        qr(s, 2, 6, 10, 14);
        qr(s, 3, 7, 11, 15);
        qr(s, 0, 5, 10, 15);
        qr(s, 1, 6, 11, 12);
        qr(s, 2, 7, 8, 13);
        qr(s, 3, 4, 9, 14);
    }
}

fn init(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u32; 16] {
    let mut s = [0u32; 16];
    s[..4].copy_from_slice(&SIGMA);
    for i in 0..8 {
        s[4 + i] = u32::from_le_bytes(key[i * 4..i * 4 + 4].try_into().unwrap());
    }
    s[12] = counter;
    for i in 0..3 {
        s[13 + i] = u32::from_le_bytes(nonce[i * 4..i * 4 + 4].try_into().unwrap());
    }
    s
}

/// The ChaCha20 block function (§2.3): 64 bytes of keystream for (key, counter, nonce).
pub fn block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let init = init(key, counter, nonce);
    let mut s = init;
    rounds(&mut s);
    let mut out = [0u8; 64];
    for i in 0..16 {
        out[i * 4..i * 4 + 4].copy_from_slice(&s[i].wrapping_add(init[i]).to_le_bytes());
    }
    s.zeroize();
    out
}

/// HChaCha20: the 32-byte subkey for XChaCha20 from a key and a 16-byte nonce.
pub fn hchacha20(key: &[u8; 32], nonce16: &[u8; 16]) -> [u8; 32] {
    let mut s = [0u32; 16];
    s[..4].copy_from_slice(&SIGMA);
    for i in 0..8 {
        s[4 + i] = u32::from_le_bytes(key[i * 4..i * 4 + 4].try_into().unwrap());
    }
    for i in 0..4 {
        s[12 + i] = u32::from_le_bytes(nonce16[i * 4..i * 4 + 4].try_into().unwrap());
    }
    rounds(&mut s);
    let mut out = [0u8; 32];
    for i in 0..4 {
        out[i * 4..i * 4 + 4].copy_from_slice(&s[i].to_le_bytes());
        out[16 + i * 4..16 + i * 4 + 4].copy_from_slice(&s[12 + i].to_le_bytes());
    }
    s.zeroize();
    out
}

/// A ChaCha20 stream (§2.4): `apply_keystream` XORs the keystream into data, continuing where the last
/// call stopped. The 32-bit block counter wraps after 2^32 blocks (256 GiB) — RFC 8439 leaves that
/// undefined; the AEAD refuses messages that long before it can happen.
pub struct ChaCha20 {
    key: [u8; 32],
    nonce: [u8; 12],
    counter: u32,
    buf: [u8; 64],
    used: usize,
}

impl ChaCha20 {
    /// A stream starting at block `counter`.
    pub fn new(key: &[u8; 32], nonce: &[u8; 12], counter: u32) -> Self {
        ChaCha20 { key: *key, nonce: *nonce, counter, buf: [0; 64], used: 64 }
    }

    /// XOR the next `data.len()` keystream bytes into `data`.
    pub fn apply_keystream(&mut self, data: &mut [u8]) {
        let mut i = 0;
        while i < data.len() {
            if self.used == 64 {
                self.buf = block(&self.key, self.counter, &self.nonce);
                self.counter = self.counter.wrapping_add(1);
                self.used = 0;
            }
            let n = core::cmp::min(64 - self.used, data.len() - i);
            for k in 0..n {
                data[i + k] ^= self.buf[self.used + k];
            }
            self.used += n;
            i += n;
        }
    }
}

impl Drop for ChaCha20 {
    fn drop(&mut self) {
        self.key.zeroize();
        self.buf.zeroize();
    }
}
