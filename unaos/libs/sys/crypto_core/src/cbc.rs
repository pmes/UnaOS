// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CBC mode over [`Aes`] (NIST SP 800-38A §6.2), whole blocks, in place, no allocation, no padding policy.
//! PDFCRYPT (LEDGER SR315): PDF's AESV2 / AESV3 crypt filters are AES-CBC with the IV as the first 16 bytes and
//! PKCS #7 padding (ISO 32000-1 §7.6.2), the /UE and /OE key unwrap is AES-256-CBC with a zero IV and no padding, and
//! the R6 password hash (ISO 32000-2 Algorithm 2.B) runs AES-128-CBC without padding — all three are these two calls.
//! CBC-ONE (CRYPTOONE, LEDGER SR330): Aether's WebCrypto AES-CBC (`handlers/aether/src/api/subtle.rs` `aes_cbc`) is
//! these calls with [`pkcs7_pad`] / [`pkcs7_unpad`], no second CBC.
//!
//! CONSTANT-TIME in the key and the data (the bitsliced [`Aes`] and XORs). [`pkcs7_unpad`] reads the last byte to
//! find the pad length, so it is NOT constant-time in the plaintext's padding; CBC with unauthenticated padding is a
//! padding oracle whenever an attacker can submit ciphertexts and observe the result — a reader of a file at rest
//! is not that setting, a network protocol is.

use crate::aes::{Aes, BLOCK};
use crate::Error;

/// Encrypt `data` in place under `aes` with `iv`. `Error::Length` when `data` is not a whole number of blocks.
pub fn encrypt(aes: &Aes, iv: &[u8; BLOCK], data: &mut [u8]) -> Result<(), Error> {
    if data.len() % BLOCK != 0 {
        return Err(Error::Length);
    }
    let mut prev = *iv;
    for chunk in data.chunks_exact_mut(BLOCK) {
        let mut b = [0u8; BLOCK];
        for i in 0..BLOCK {
            b[i] = chunk[i] ^ prev[i];
        }
        aes.encrypt_block(&mut b);
        chunk.copy_from_slice(&b);
        prev = b;
    }
    Ok(())
}

/// Decrypt `data` in place under `aes` with `iv` (four blocks per bitsliced pass). `Error::Length` when `data` is
/// not a whole number of blocks.
pub fn decrypt(aes: &Aes, iv: &[u8; BLOCK], data: &mut [u8]) -> Result<(), Error> {
    if data.len() % BLOCK != 0 {
        return Err(Error::Length);
    }
    let mut prev = *iv;
    for group in data.chunks_mut(4 * BLOCK) {
        let n = group.len() / BLOCK;
        let mut blocks = [[0u8; BLOCK]; 4];
        for k in 0..n {
            blocks[k].copy_from_slice(&group[k * BLOCK..(k + 1) * BLOCK]);
        }
        let cipher = blocks;
        aes.decrypt_blocks(&mut blocks[..n]);
        for k in 0..n {
            for i in 0..BLOCK {
                group[k * BLOCK + i] = blocks[k][i] ^ prev[i];
            }
            prev = cipher[k];
        }
    }
    Ok(())
}

/// PKCS #7 (RFC 5652 §6.3): the length of `data` with its padding removed, or `None` when the last byte is not a pad
/// length in 1..=16 repeated that many times.
pub fn pkcs7_unpad(data: &[u8]) -> Option<usize> {
    let &last = data.last()?;
    let n = last as usize;
    if n == 0 || n > BLOCK || n > data.len() {
        return None;
    }
    if data[data.len() - n..].iter().all(|&b| b == last) { Some(data.len() - n) } else { None }
}

/// PKCS #7 (RFC 5652 §6.3), the pad side: write `n` bytes of value `n` (1..=16, a whole block when `len` is already
/// aligned) after the first `len` bytes of `buf`, and answer the padded length; `None` when `buf` is shorter than it.
/// CRYPTOONE (LEDGER SR330): WebCrypto's AES-CBC encrypt (Aether `subtle.rs`), beside [`pkcs7_unpad`].
pub fn pkcs7_pad(buf: &mut [u8], len: usize) -> Option<usize> {
    let n = BLOCK - len % BLOCK;
    let end = len.checked_add(n)?;
    if buf.len() < end {
        return None;
    }
    for b in &mut buf[len..end] {
        *b = n as u8;
    }
    Some(end)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex<const N: usize>(s: &str) -> [u8; N] {
        let mut o = [0u8; N];
        for i in 0..N {
            o[i] = u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap();
        }
        o
    }

    const IV: &str = "000102030405060708090a0b0c0d0e0f";
    const PT: &str = "6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51\
30c81c46a35ce411e5fbc1191a0a52eff69f2445df4f9b17ad2b417be66c3710";

    /// SP 800-38A Appendix F.2.1-F.2.6: CBC-AES128 / AES192 / AES256, encrypt and decrypt.
    #[test]
    fn sp800_38a_f2() {
        let cases = [
            (
                &b"2b7e151628aed2a6abf7158809cf4f3c"[..],
                "7649abac8119b246cee98e9b12e9197d5086cb9b507219ee95db113a917678b2\
73bed6b8e3c1743b7116e69e222295163ff1caa1681fac09120eca307586e1a7",
            ),
            (
                &b"8e73b0f7da0e6452c810f32b809079e562f8ead2522c6b7b"[..],
                "4f021db243bc633d7178183a9fa071e8b4d9ada9ad7dedf4e5e738763f69145a\
571b242012fb7ae07fa9baac3df102e008b0e27988598881d920a9e64f5615cd",
            ),
            (
                &b"603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4"[..],
                "f58c4c04d6e5f1ba779eabfb5f7bfbd69cfc4e967edb808d679f777bc6702c7d\
39f23369a9d9bacfa530e26304231461b2eb05e2c39be9fcda6c19078c6a9d1b",
            ),
        ];
        let iv = unhex::<16>(IV);
        let pt = unhex::<64>(PT);
        for (khex, cthex) in cases {
            let khex = core::str::from_utf8(khex).unwrap();
            let mut key = [0u8; 32];
            let kl = khex.len() / 2;
            for i in 0..kl {
                key[i] = u8::from_str_radix(&khex[2 * i..2 * i + 2], 16).unwrap();
            }
            let aes = Aes::new(&key[..kl]).unwrap();
            let ct = unhex::<64>(cthex);
            let mut buf = pt;
            encrypt(&aes, &iv, &mut buf).unwrap();
            assert_eq!(buf, ct, "encrypt, {}-bit key", kl * 8);
            decrypt(&aes, &iv, &mut buf).unwrap();
            assert_eq!(buf, pt, "decrypt, {}-bit key", kl * 8);
            // Five blocks: crosses the four-block bitsliced group.
            let mut five = [0u8; 80];
            five[..64].copy_from_slice(&pt);
            five[64..].copy_from_slice(&pt[..16]);
            let want = five;
            encrypt(&aes, &iv, &mut five).unwrap();
            decrypt(&aes, &iv, &mut five).unwrap();
            assert_eq!(five, want);
        }
    }

    #[test]
    fn lengths_and_pkcs7() {
        let aes = Aes::new(&[0u8; 16]).unwrap();
        assert_eq!(encrypt(&aes, &[0; 16], &mut [0u8; 15]), Err(Error::Length));
        assert_eq!(decrypt(&aes, &[0; 16], &mut [0u8; 17]), Err(Error::Length));
        assert_eq!(pkcs7_unpad(b"abc\x03\x03\x03"), Some(3));
        assert_eq!(pkcs7_unpad(&[16u8; 16]), Some(0));
        assert_eq!(pkcs7_unpad(b"abc\x00"), None);
        assert_eq!(pkcs7_unpad(b"abc\x02\x03"), None);
        assert_eq!(pkcs7_unpad(&[17u8; 17]), None);
        assert_eq!(pkcs7_unpad(b""), None);
        // CRYPTOONE (SR330): the pad side, and pad -> unpad round trips across the block edges.
        let mut b = [0u8; 32];
        assert_eq!(pkcs7_pad(&mut b, 0), Some(16));
        assert_eq!(b[..16], [16u8; 16]);
        let mut b = *b"abc\0\0\0\0\0\0\0\0\0\0\0\0\0";
        assert_eq!(pkcs7_pad(&mut b, 3), Some(16));
        assert_eq!(&b[3..], &[13u8; 13]);
        let mut b = [0u8; 32];
        assert_eq!(pkcs7_pad(&mut b, 16), Some(32));
        assert_eq!(b[16..], [16u8; 16]);
        assert_eq!(pkcs7_pad(&mut [0u8; 15], 3), None);
        assert_eq!(pkcs7_pad(&mut [0u8; 16], 16), None);
        for len in 0..=33 {
            let mut b = [0xa5u8; 48];
            let end = pkcs7_pad(&mut b, len).unwrap();
            assert_eq!(end % BLOCK, 0);
            assert_eq!(pkcs7_unpad(&b[..end]), Some(len));
        }
    }
}
