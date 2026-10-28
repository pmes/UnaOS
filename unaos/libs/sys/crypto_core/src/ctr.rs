// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CTR mode with an n-bit counter (NIST SP 800-38A §6.5, the standard incrementing function of §B.1) over the
//! bitsliced [`Aes`] — CRYPTOONE-2 (LEDGER SR342).
//!
//! CHARTER: Holocron — shared-core. Arithmetic only (crypto_core's own charter): the ONE counter mode in the crate.
//! GCM's GCTR is this with a 32-bit counter (inc32, SP 800-38D §6.2), CCM's `A_i` with an 8·L-bit counter
//! (RFC 3610 §2.3), and Aether's WebCrypto AES-CTR with its `length` (W3C Web Cryptography §27).
//!
//! ```text
//! T_i = (T_0 & !mask) | ((T_0 + i) & mask)      mask = 2^counter_bits - 1     (128-bit big-endian arithmetic)
//! data ^= E(K, T_0) ‖ E(K, T_1) ‖ …             the last keystream block truncated to the data
//! ```
//!
//! The counter wraps within its bits and the high `128 - counter_bits` bits never change. A caller that must never
//! repeat a counter value checks [`fits`] first (WebCrypto's OperationError); GCM and CCM bound their lengths so the
//! counter cannot wrap.
//!
//! CONSTANT-TIME: yes in key, counter block and data contents ([`Aes`] is table-free; no branch or index on data).
//! Lengths and `counter_bits` are public.

use crate::aes::{Aes, BLOCK};
use crate::ct::Zeroize;

/// The low-`counter_bits` mask (128 and above: every bit; 0: none).
fn mask(counter_bits: u8) -> u128 {
    match counter_bits {
        0 => 0,
        b if b >= 128 => u128::MAX,
        b => (1u128 << b) - 1,
    }
}

/// XOR the CTR keystream from `counter_block` into `data` in place (encryption and decryption are the same
/// call). The low `counter_bits` bits of the block increment big-endian, wrapping within those bits; the rest stay
/// fixed. `counter_bits` above 128 is 128; 0 holds the block fixed. Four counter blocks per bitsliced pass, the
/// keystream zeroized after each; no allocation.
pub fn ctr(aes: &Aes, counter_block: &[u8; 16], counter_bits: u8, data: &mut [u8]) {
    let m = mask(counter_bits);
    let base = u128::from_be_bytes(*counter_block);
    let fixed = base & !m;
    let mut i = 0u128;
    for chunk in data.chunks_mut(4 * BLOCK) {
        let n = chunk.len().div_ceil(BLOCK);
        let mut ks = [[0u8; BLOCK]; 4];
        for (k, b) in ks.iter_mut().take(n).enumerate() {
            *b = (fixed | (base.wrapping_add(i.wrapping_add(k as u128)) & m)).to_be_bytes();
        }
        aes.encrypt_blocks(&mut ks[..n]);
        for (j, x) in chunk.iter_mut().enumerate() {
            *x ^= ks[j / BLOCK][j % BLOCK];
        }
        i = i.wrapping_add(n as u128);
        for b in ks.iter_mut() {
            b.zeroize();
        }
    }
}

/// Whether `len` bytes of data fit a `counter_bits`-bit counter without repeating a counter value: `counter_bits`
/// in 1..=128 and at most `2^counter_bits` blocks.
pub fn fits(counter_bits: u8, len: usize) -> bool {
    if counter_bits == 0 || counter_bits > 128 {
        return false;
    }
    counter_bits >= 64 || (len.div_ceil(BLOCK) as u128) <= 1u128 << counter_bits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> [u8; 64] {
        let mut out = [0u8; 64];
        for (k, o) in out.iter_mut().enumerate().take(s.len() / 2) {
            *o = u8::from_str_radix(&s[2 * k..2 * k + 2], 16).unwrap();
        }
        out
    }

    const F5_PT: &str = "6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e5130c81c46a35ce411e5fbc1191a0a52eff69f2445df4f9b17ad2b417be66c3710";
    const F5_CTR: [u8; 16] = [0xf0, 0xf1, 0xf2, 0xf3, 0xf4, 0xf5, 0xf6, 0xf7, 0xf8, 0xf9, 0xfa, 0xfb, 0xfc, 0xfd, 0xfe, 0xff];

    /// SP 800-38A F.5.1 (CTR-AES128) and F.5.5 (CTR-AES256), all four blocks, both directions; the counter does not
    /// wrap in four blocks, so 128-, 64- and 32-bit counters give the same bytes.
    #[test]
    fn sp800_38a_f5() {
        for (key, want) in [
            ("2b7e151628aed2a6abf7158809cf4f3c", "874d6191b620e3261bef6864990db6ce9806f66b7970fdff8617187bb9fffdff5ae4df3edbd5d35e5b4f09020db03eab1e031dda2fbe03d1792170a0f3009cee"),
            ("603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4", "601ec313775789a5b7a7f504bbf3d228f443e3ca4d62b59aca84e990cacaf5c52b0930daa23de94ce87017ba2d84988ddfc9c58db67aada613c2dd08457941a6"),
        ] {
            let aes = Aes::new(&unhex(key)[..key.len() / 2]).unwrap();
            let pt = unhex(F5_PT);
            let ct = unhex(want);
            for bits in [128u8, 64, 32] {
                let mut buf = pt;
                ctr(&aes, &F5_CTR, bits, &mut buf);
                assert_eq!(buf, ct, "encrypt, {bits}-bit counter");
                ctr(&aes, &F5_CTR, bits, &mut buf);
                assert_eq!(buf, pt, "decrypt, {bits}-bit counter");
            }
        }
    }

    /// The wrap at each counter width: the low `bits` bits start all ones, so block 1 is the wrap to zero; every
    /// block is checked against an explicit one-block reference (`encrypt_block` on the hand-built `T_i`), across
    /// two four-block passes and a partial last block.
    #[test]
    fn wrap_at_each_width() {
        let aes = Aes::new(&[0x2b, 0x7e, 0x15, 0x16, 0x28, 0xae, 0xd2, 0xa6, 0xab, 0xf7, 0x15, 0x88, 0x09, 0xcf, 0x4f, 0x3c]).unwrap();
        for bits in [1u8, 8, 16, 32, 64, 128] {
            let m = mask(bits);
            let t0 = u128::from_be_bytes(F5_CTR) | m;
            let mut data = [0u8; 6 * BLOCK + 5];
            for (k, b) in data.iter_mut().enumerate() {
                *b = (k * 7 + 3) as u8;
            }
            let orig = data;
            ctr(&aes, &t0.to_be_bytes(), bits, &mut data);
            for (i, (got, src)) in data.chunks(BLOCK).zip(orig.chunks(BLOCK)).enumerate() {
                let ti = (t0 & !m) | ((i as u128).wrapping_sub(1) & m);
                assert_eq!(ti & !m, u128::from_be_bytes(F5_CTR) & !m, "high bits fixed, {bits}-bit counter");
                let mut ks = ti.to_be_bytes();
                aes.encrypt_block(&mut ks);
                for (j, (g, s)) in got.iter().zip(src).enumerate() {
                    assert_eq!(*g, s ^ ks[j], "{bits}-bit counter, block {i}, byte {j}");
                }
            }
        }
    }

    /// The repeated-counter bound: at most 2^bits blocks, bits in 1..=128.
    #[test]
    fn fits_edges() {
        assert!(fits(1, 32));
        assert!(!fits(1, 33));
        assert!(fits(8, 4096));
        assert!(!fits(8, 4097));
        assert!(!fits(0, 0));
        assert!(!fits(129, 16));
        assert!(fits(128, usize::MAX));
        assert!(fits(64, usize::MAX));
    }
}
