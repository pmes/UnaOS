// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! XTS-AES (IEEE Std 1619-2007, "Cryptographic Protection of Data on Block-Oriented Storage Devices";
//! the same mode NIST SP 800-38E approves) over the bitsliced [`Aes`] — XTS (rmbp-ledger B689), the cipher
//! of a UnaFS volume whose key slot says `cipher = 1` (`unafs::storage::VolumeCipher`).
//!
//! The mode, as §5 of the standard writes it. The key is `Key1 ‖ Key2` (two AES keys of one size: 2 × 16 B
//! = XTS-AES-128, 2 × 32 B = XTS-AES-256). A DATA UNIT (a disk block) of `len` bytes, `16 <= len <= 2^24`,
//! numbered `i` (the 128-bit tweak, little-endian), is enciphered 16 bytes at a time:
//!
//! ```text
//! T_0   = AES-enc(Key2, i)                      (§5.3.1, step 1)
//! T_j   = T_{j-1} ⊗ α        in GF(2^128) mod x^128 + x^7 + x^2 + x + 1, little-endian bit order
//! C_j   = AES-enc(Key1, P_j ⊕ T_j) ⊕ T_j        (decryption: AES-dec, the same T_j)
//! ```
//!
//! A data unit that is not a whole number of 16-byte blocks ends with CIPHERTEXT STEALING (§5.3.2 / §5.4.2):
//! the last full block is enciphered with `T_{m-1}`, its leading `b` bytes become the short final block, and
//! the short plaintext padded with the rest of that ciphertext is enciphered with `T_m` in its place.
//!
//! [`Xts::encrypt_unit_at`] / [`Xts::decrypt_unit_at`] encipher a whole-block SLICE of a data unit at a
//! 16-byte-aligned offset: block `j` of the unit always uses `T_j`, so a 512 B sector of a 4096 B UnaFS
//! block is enciphered on its own exactly as it is inside the whole block (the root flip's one-sector write,
//! `unafs::adapter`).
//!
//! CONSTANT-TIME: yes. [`Aes`] is table-free; `⊗ α` is a shift and a masked XOR (no branch on the carry);
//! lengths, offsets and the data-unit number are public. The tweak values `T_j` are secret (they are AES
//! outputs under Key2) and are wiped from the stack buffers after use.
//!
//! What this module does NOT decide: IEEE 1619 permits `Key1 = Key2` (its vector 1 uses it); SP 800-38E's
//! FIPS guidance forbids it. The one consumer (`unafs`) derives the two keys by HKDF and refuses equal halves.

use crate::aes::{Aes, BLOCK};
use crate::ct::Zeroize;
use crate::Error;

/// The largest data unit the standard allows: 2^20 AES blocks (IEEE 1619-2007 §5.1).
pub const MAX_UNIT: usize = BLOCK << 20;

/// An XTS-AES key: the data key (Key1) and the tweak key (Key2), both expanded. Zeroized on drop (each
/// [`Aes`] wipes its schedule).
#[derive(Clone)]
pub struct Xts {
    data: Aes,
    tweak: Aes,
}

/// `x ⊗ α` in GF(2^128), the standard's little-endian convention (§5.2): shift the 128-bit little-endian
/// value left by one; a carry out of bit 127 folds back as 0x87. Branch-free.
#[inline(always)]
fn mul_alpha(x: u128) -> u128 {
    let carry = x >> 127;
    (x << 1) ^ (carry.wrapping_neg() & 0x87)
}

impl Xts {
    /// `key` = Key1 ‖ Key2: 32 bytes (XTS-AES-128) or 64 bytes (XTS-AES-256), as IEEE 1619 §5.1 lays it
    /// out. `Error::Length` for any other size.
    pub fn new(key: &[u8]) -> Result<Self, Error> {
        if key.len() != 32 && key.len() != 64 {
            return Err(Error::Length);
        }
        let h = key.len() / 2;
        Ok(Xts { data: Aes::new(&key[..h])?, tweak: Aes::new(&key[h..])? })
    }

    /// From the two halves (each 16 or 32 bytes, the same size).
    pub fn from_halves(key1: &[u8], key2: &[u8]) -> Result<Self, Error> {
        if key1.len() != key2.len() || (key1.len() != 16 && key1.len() != 32) {
            return Err(Error::Length);
        }
        Ok(Xts { data: Aes::new(key1)?, tweak: Aes::new(key2)? })
    }

    /// `T_skip` for data unit `tweak`: AES-enc(Key2, tweak) then `skip` multiplications by α.
    fn tweak_at(&self, tweak: &[u8; 16], skip: usize) -> u128 {
        let mut t = *tweak;
        self.tweak.encrypt_block(&mut t);
        let mut x = u128::from_le_bytes(t);
        t.zeroize();
        for _ in 0..skip {
            x = mul_alpha(x);
        }
        x
    }

    /// Whole blocks (`data.len() % 16 == 0`) from tweak `*t`, four per bitsliced pass; `*t` advances past
    /// them.
    fn bulk(&self, t: &mut u128, data: &mut [u8], decrypt: bool) {
        let mut blocks = [[0u8; BLOCK]; 4];
        let mut ts = [[0u8; BLOCK]; 4];
        for chunk in data.chunks_mut(4 * BLOCK) {
            let n = chunk.len() / BLOCK;
            for i in 0..n {
                ts[i] = t.to_le_bytes();
                *t = mul_alpha(*t);
                for k in 0..BLOCK {
                    blocks[i][k] = chunk[BLOCK * i + k] ^ ts[i][k];
                }
            }
            if decrypt {
                self.data.decrypt_blocks(&mut blocks[..n]);
            } else {
                self.data.encrypt_blocks(&mut blocks[..n]);
            }
            for i in 0..n {
                for k in 0..BLOCK {
                    chunk[BLOCK * i + k] = blocks[i][k] ^ ts[i][k];
                }
            }
        }
        for b in blocks.iter_mut().chain(ts.iter_mut()) {
            b.zeroize();
        }
    }

    /// One block under tweak value `t`.
    fn one(&self, t: u128, block: &mut [u8; BLOCK], decrypt: bool) {
        let mut tb = t.to_le_bytes();
        for k in 0..BLOCK {
            block[k] ^= tb[k];
        }
        if decrypt {
            self.data.decrypt_block(block);
        } else {
            self.data.encrypt_block(block);
        }
        for k in 0..BLOCK {
            block[k] ^= tb[k];
        }
        tb.zeroize();
    }

    /// A whole data unit, ciphertext stealing included (§5.3 / §5.4).
    fn unit(&self, tweak: &[u8; 16], data: &mut [u8], decrypt: bool) -> Result<(), Error> {
        let len = data.len();
        if !(BLOCK..=MAX_UNIT).contains(&len) {
            return Err(Error::Length);
        }
        let mut t = self.tweak_at(tweak, 0);
        let b = len % BLOCK;
        if b == 0 {
            self.bulk(&mut t, data, decrypt);
            return Ok(());
        }
        // m full blocks (m >= 1) and a final partial block of b bytes: the first m - 1 run as usual.
        let head = (len / BLOCK - 1) * BLOCK;
        self.bulk(&mut t, &mut data[..head], decrypt);
        let t_prev = t; // T_{m-1}
        let t_last = mul_alpha(t); // T_m
        let mut last = [0u8; BLOCK];
        last.copy_from_slice(&data[head..head + BLOCK]);
        let mut stolen = [0u8; BLOCK];
        if !decrypt {
            // CC = enc(P_{m-1}, T_{m-1}); C_m = CC[..b]; C_{m-1} = enc(P_m ‖ CC[b..], T_m).
            self.one(t_prev, &mut last, false);
            stolen[..b].copy_from_slice(&data[head + BLOCK..]);
            stolen[b..].copy_from_slice(&last[b..]);
            data[head + BLOCK..].copy_from_slice(&last[..b]);
            self.one(t_last, &mut stolen, false);
            data[head..head + BLOCK].copy_from_slice(&stolen);
        } else {
            // PP = dec(C_{m-1}, T_m); P_m = PP[..b]; P_{m-1} = dec(C_m ‖ PP[b..], T_{m-1}).
            self.one(t_last, &mut last, true);
            stolen[..b].copy_from_slice(&data[head + BLOCK..]);
            stolen[b..].copy_from_slice(&last[b..]);
            data[head + BLOCK..].copy_from_slice(&last[..b]);
            self.one(t_prev, &mut stolen, true);
            data[head..head + BLOCK].copy_from_slice(&stolen);
        }
        last.zeroize();
        stolen.zeroize();
        Ok(())
    }

    /// Encrypt one data unit in place under the 128-bit tweak `tweak` (the data-unit number, little-endian).
    /// `16 <= data.len() <= MAX_UNIT`, else `Error::Length`; a length that is not a multiple of 16 uses
    /// ciphertext stealing.
    pub fn encrypt(&self, tweak: &[u8; 16], data: &mut [u8]) -> Result<(), Error> {
        self.unit(tweak, data, false)
    }

    /// Decrypt one data unit in place (the inverse of [`encrypt`](Self::encrypt)).
    pub fn decrypt(&self, tweak: &[u8; 16], data: &mut [u8]) -> Result<(), Error> {
        self.unit(tweak, data, true)
    }

    /// Encrypt data unit number `unit` (the tweak = `unit` as a 128-bit little-endian integer, IEEE 1619
    /// §5.1's data-unit sequence number).
    pub fn encrypt_unit(&self, unit: u64, data: &mut [u8]) -> Result<(), Error> {
        self.unit(&unit_tweak(unit), data, false)
    }

    /// Decrypt data unit number `unit`.
    pub fn decrypt_unit(&self, unit: u64, data: &mut [u8]) -> Result<(), Error> {
        self.unit(&unit_tweak(unit), data, true)
    }

    /// Encrypt the whole-block SLICE `data` that sits at byte `offset` of data unit `unit`: `offset` and
    /// `data.len()` multiples of 16, `data` non-empty, `offset + data.len() <= MAX_UNIT`, else
    /// `Error::Length`. Block `j` of the unit uses `T_j`, so the result is byte-identical to the same bytes
    /// of the whole unit enciphered by [`encrypt_unit`](Self::encrypt_unit).
    pub fn encrypt_unit_at(&self, unit: u64, offset: usize, data: &mut [u8]) -> Result<(), Error> {
        self.slice(unit, offset, data, false)
    }

    /// Decrypt a whole-block slice of data unit `unit` at `offset` (see
    /// [`encrypt_unit_at`](Self::encrypt_unit_at)).
    pub fn decrypt_unit_at(&self, unit: u64, offset: usize, data: &mut [u8]) -> Result<(), Error> {
        self.slice(unit, offset, data, true)
    }

    fn slice(&self, unit: u64, offset: usize, data: &mut [u8], decrypt: bool) -> Result<(), Error> {
        let end = offset.checked_add(data.len()).ok_or(Error::Length)?;
        if data.is_empty() || offset % BLOCK != 0 || data.len() % BLOCK != 0 || end > MAX_UNIT {
            return Err(Error::Length);
        }
        let mut t = self.tweak_at(&unit_tweak(unit), offset / BLOCK);
        self.bulk(&mut t, data, decrypt);
        Ok(())
    }
}

/// The 128-bit little-endian tweak for data-unit number `unit`.
pub fn unit_tweak(unit: u64) -> [u8; 16] {
    let mut t = [0u8; 16];
    t[..8].copy_from_slice(&unit.to_le_bytes());
    t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alpha_matches_the_bytewise_definition() {
        // §5.2's byte loop: Cout = a[15] >> 7; a[k] = (a[k] << 1) | (a[k-1] >> 7); a[0] ^= 0x87 if Cout.
        let mut a: [u8; 16] = core::array::from_fn(|i| (i as u8).wrapping_mul(37) ^ 0xa5);
        for _ in 0..300 {
            let x = mul_alpha(u128::from_le_bytes(a));
            let cout = a[15] >> 7;
            for k in (1..16).rev() {
                a[k] = (a[k] << 1) | (a[k - 1] >> 7);
            }
            a[0] <<= 1;
            if cout == 1 {
                a[0] ^= 0x87;
            }
            assert_eq!(x.to_le_bytes(), a);
        }
    }
}
