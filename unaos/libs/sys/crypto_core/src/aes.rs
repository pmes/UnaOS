// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! AES-128 / AES-192 / AES-256 (FIPS 197), forward cipher, BITSLICED — no lookup tables at all.
//!
//! Layout: four 16-byte blocks are processed together as eight `u64` bit-planes. Plane `i` holds bit
//! `i` (0 = least significant) of every state byte; byte `j` of block `k` (FIPS 197 order, `j = 4c + r`)
//! sits at bit position `16k + j`. In that layout:
//!  * SubBytes is the Boyar–Peralta S-box circuit (32 AND, 83 XOR, 4 XNOR — "A depth-16 circuit for the
//!    AES S-box", Boyar & Peralta 2011) evaluated on the planes, 64 S-boxes per gate;
//!  * ShiftRows and the MixColumns row rotations are fixed bit permutations inside each 16-bit lane
//!    (masks and constant shifts);
//!  * MixColumns' `xtime` is a plane permutation plus three XORs;
//!  * AddRoundKey XORs pre-bitsliced round keys.
//! The key schedule's SubWord runs through the same circuit.
//!
//! CONSTANT-TIME: yes — there is no table and no secret-dependent branch or index anywhere; the cost is
//! the same for every key and every block. (This is the property T-table AES does not have: its
//! cache-line footprint leaks key bytes.)
//!
//! Only the FORWARD cipher is provided: GCM, CTR and CMAC never run the inverse. A mode that needs
//! decryption (CBC) is not a mode UnaOS should be adopting; if a legacy one is ever required the inverse
//! S-box circuit is the extension point.

use crate::ct::Zeroize;
use crate::Error;

/// AES block size.
pub const BLOCK: usize = 16;

/// Lane-replicated mask: the 16-bit pattern `m` in each of the four 16-bit lanes.
const fn rep(m: u16) -> u64 {
    (m as u64) * 0x0001_0001_0001_0001
}

/// The Boyar–Peralta S-box circuit on eight planes. Input/output plane 7 is the most significant bit
/// (the circuit's U0 / S0).
#[inline(always)]
fn sub_bytes(p: &mut [u64; 8]) {
    let u0 = p[7];
    let u1 = p[6];
    let u2 = p[5];
    let u3 = p[4];
    let u4 = p[3];
    let u5 = p[2];
    let u6 = p[1];
    let u7 = p[0];
    // top linear transform
    let t1 = u0 ^ u3;
    let t2 = u0 ^ u5;
    let t3 = u0 ^ u6;
    let t4 = u3 ^ u5;
    let t5 = u4 ^ u6;
    let t6 = t1 ^ t5;
    let t7 = u1 ^ u2;
    let t8 = u7 ^ t6;
    let t9 = u7 ^ t7;
    let t10 = t6 ^ t7;
    let t11 = u1 ^ u5;
    let t12 = u2 ^ u5;
    let t13 = t3 ^ t4;
    let t14 = t6 ^ t11;
    let t15 = t5 ^ t11;
    let t16 = t5 ^ t12;
    let t17 = t9 ^ t16;
    let t18 = u3 ^ u7;
    let t19 = t7 ^ t18;
    let t20 = t1 ^ t19;
    let t21 = u6 ^ u7;
    let t22 = t7 ^ t21;
    let t23 = t2 ^ t22;
    let t24 = t2 ^ t10;
    let t25 = t20 ^ t17;
    let t26 = t3 ^ t16;
    let t27 = t1 ^ t12;
    let d = u7;
    // shared non-linear middle (GF(2^8) inversion in a tower field)
    let m1 = t13 & t6;
    let m2 = t23 & t8;
    let m3 = t14 ^ m1;
    let m4 = t19 & d;
    let m5 = m4 ^ m1;
    let m6 = t3 & t16;
    let m7 = t22 & t9;
    let m8 = t26 ^ m6;
    let m9 = t20 & t17;
    let m10 = m9 ^ m6;
    let m11 = t1 & t15;
    let m12 = t4 & t27;
    let m13 = m12 ^ m11;
    let m14 = t2 & t10;
    let m15 = m14 ^ m11;
    let m16 = m3 ^ m2;
    let m17 = m5 ^ t24;
    let m18 = m8 ^ m7;
    let m19 = m10 ^ m15;
    let m20 = m16 ^ m13;
    let m21 = m17 ^ m15;
    let m22 = m18 ^ m13;
    let m23 = m19 ^ t25;
    let m24 = m22 ^ m23;
    let m25 = m22 & m20;
    let m26 = m21 ^ m25;
    let m27 = m20 ^ m21;
    let m28 = m23 ^ m25;
    let m29 = m28 & m27;
    let m30 = m26 & m24;
    let m31 = m20 & m23;
    let m32 = m27 & m31;
    let m33 = m27 ^ m25;
    let m34 = m21 & m22;
    let m35 = m24 & m34;
    let m36 = m24 ^ m25;
    let m37 = m21 ^ m29;
    let m38 = m32 ^ m33;
    let m39 = m23 ^ m30;
    let m40 = m35 ^ m36;
    let m41 = m38 ^ m40;
    let m42 = m37 ^ m39;
    let m43 = m37 ^ m38;
    let m44 = m39 ^ m40;
    let m45 = m42 ^ m41;
    let m46 = m44 & t6;
    let m47 = m40 & t8;
    let m48 = m39 & d;
    let m49 = m43 & t16;
    let m50 = m38 & t9;
    let m51 = m37 & t17;
    let m52 = m42 & t15;
    let m53 = m45 & t27;
    let m54 = m41 & t10;
    let m55 = m44 & t13;
    let m56 = m40 & t23;
    let m57 = m39 & t19;
    let m58 = m43 & t3;
    let m59 = m38 & t22;
    let m60 = m37 & t20;
    let m61 = m42 & t1;
    let m62 = m45 & t4;
    let m63 = m41 & t2;
    // bottom linear transform
    let l0 = m61 ^ m62;
    let l1 = m50 ^ m56;
    let l2 = m46 ^ m48;
    let l3 = m47 ^ m55;
    let l4 = m54 ^ m58;
    let l5 = m49 ^ m61;
    let l6 = m62 ^ l5;
    let l7 = m46 ^ l3;
    let l8 = m51 ^ m59;
    let l9 = m52 ^ m53;
    let l10 = m53 ^ l4;
    let l11 = m60 ^ l2;
    let l12 = m48 ^ m51;
    let l13 = m50 ^ l0;
    let l14 = m52 ^ m61;
    let l15 = m55 ^ l1;
    let l16 = m56 ^ l0;
    let l17 = m57 ^ l1;
    let l18 = m58 ^ l8;
    let l19 = m63 ^ l4;
    let l20 = l0 ^ l1;
    let l21 = l1 ^ l7;
    let l22 = l3 ^ l12;
    let l23 = l18 ^ l2;
    let l24 = l15 ^ l9;
    let l25 = l6 ^ l10;
    let l26 = l7 ^ l9;
    let l27 = l8 ^ l10;
    let l28 = l11 ^ l14;
    let l29 = l11 ^ l17;
    p[7] = l6 ^ l24;
    p[6] = !(l16 ^ l26);
    p[5] = !(l19 ^ l28);
    p[4] = l6 ^ l21;
    p[3] = l20 ^ l22;
    p[2] = l25 ^ l29;
    p[1] = !(l13 ^ l27);
    p[0] = !(l6 ^ l23);
}

/// ShiftRows: row r rotates left by r columns, i.e. new byte `4c + r` = old byte `4((c + r) mod 4) + r`.
#[inline(always)]
fn shift_rows(p: &mut [u64; 8]) {
    for x in p.iter_mut() {
        let v = *x;
        *x = (v & rep(0x1111))
            | ((v >> 4) & rep(0x0222))
            | ((v << 12) & rep(0x2000))
            | ((v >> 8) & rep(0x0044))
            | ((v << 8) & rep(0x4400))
            | ((v >> 12) & rep(0x0008))
            | ((v << 4) & rep(0x8880));
    }
}

/// Within each column, new row r = old row (r + k) mod 4.
#[inline(always)]
fn rot1(v: u64) -> u64 {
    ((v >> 1) & rep(0x7777)) | ((v << 3) & rep(0x8888))
}
#[inline(always)]
fn rot2(v: u64) -> u64 {
    ((v >> 2) & rep(0x3333)) | ((v << 2) & rep(0xCCCC))
}
#[inline(always)]
fn rot3(v: u64) -> u64 {
    ((v >> 3) & rep(0x1111)) | ((v << 1) & rep(0xEEEE))
}

/// MixColumns: `b_r = 2(a_r ^ a_{r+1}) ^ a_{r+1} ^ a_{r+2} ^ a_{r+3}`.
#[inline(always)]
fn mix_columns(p: &mut [u64; 8]) {
    let mut t = [0u64; 8];
    let mut r = [0u64; 8];
    for i in 0..8 {
        let a1 = rot1(p[i]);
        t[i] = p[i] ^ a1;
        r[i] = a1 ^ rot2(p[i]) ^ rot3(p[i]);
    }
    // xtime(t): multiply by x modulo x^8 + x^4 + x^3 + x + 1
    let hi = t[7];
    let xt = [hi, t[0] ^ hi, t[1], t[2] ^ hi, t[3] ^ hi, t[4], t[5], t[6]];
    for i in 0..8 {
        p[i] = xt[i] ^ r[i];
    }
}

#[inline(always)]
fn add_round_key(p: &mut [u64; 8], k: &[u64; 8]) {
    for i in 0..8 {
        p[i] ^= k[i];
    }
}

/// Bitslice up to four blocks (missing blocks are zero).
fn pack(blocks: &[[u8; 16]]) -> [u64; 8] {
    let mut p = [0u64; 8];
    for (k, b) in blocks.iter().enumerate() {
        for j in 0..16 {
            let byte = b[j] as u64;
            let pos = 16 * k + j;
            for i in 0..8 {
                p[i] |= ((byte >> i) & 1) << pos;
            }
        }
    }
    p
}

fn unpack(p: &[u64; 8], blocks: &mut [[u8; 16]]) {
    for (k, b) in blocks.iter_mut().enumerate() {
        for j in 0..16 {
            let pos = 16 * k + j;
            let mut byte = 0u8;
            for i in 0..8 {
                byte |= (((p[i] >> pos) & 1) as u8) << i;
            }
            b[j] = byte;
        }
    }
}

/// The S-box on four bytes (key schedule SubWord), through the same circuit.
fn sub_word(w: [u8; 4]) -> [u8; 4] {
    let mut blk = [[0u8; 16]; 1];
    blk[0][..4].copy_from_slice(&w);
    let mut p = pack(&blk);
    sub_bytes(&mut p);
    unpack(&p, &mut blk);
    let mut o = [0u8; 4];
    o.copy_from_slice(&blk[0][..4]);
    blk[0].zeroize();
    p.zeroize();
    o
}

/// One S-box lookup, constant-time (exposed for tests and for anyone needing the S-box).
pub fn sbox(x: u8) -> u8 {
    sub_word([x, 0, 0, 0])[0]
}

const RCON: [u8; 10] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36];

/// An expanded AES key (AES-128: 10 rounds, AES-192: 12, AES-256: 14), round keys stored bitsliced and
/// replicated across the four lanes. Zeroized on drop.
#[derive(Clone)]
pub struct Aes {
    rk: [[u64; 8]; 15],
    rounds: usize,
}

impl Aes {
    /// Expand a 16-, 24- or 32-byte key (FIPS 197 §5.2). `Error::Length` for any other size.
    pub fn new(key: &[u8]) -> Result<Self, Error> {
        let nk = match key.len() {
            16 => 4,
            24 => 6,
            32 => 8,
            _ => return Err(Error::Length),
        };
        let rounds = nk + 6;
        let total = 4 * (rounds + 1);
        let mut w = [[0u8; 4]; 60];
        for i in 0..nk {
            w[i].copy_from_slice(&key[4 * i..4 * i + 4]);
        }
        for i in nk..total {
            let mut t = w[i - 1];
            if i % nk == 0 {
                t = [t[1], t[2], t[3], t[0]];
                t = sub_word(t);
                t[0] ^= RCON[i / nk - 1];
            } else if nk > 6 && i % nk == 4 {
                t = sub_word(t);
            }
            for b in 0..4 {
                w[i][b] = w[i - nk][b] ^ t[b];
            }
        }
        let mut rk = [[0u64; 8]; 15];
        for r in 0..=rounds {
            let mut b = [0u8; 16];
            for c in 0..4 {
                b[4 * c..4 * c + 4].copy_from_slice(&w[4 * r + c]);
            }
            let blocks = [b, b, b, b];
            rk[r] = pack(&blocks);
            b.zeroize();
        }
        for x in w.iter_mut() {
            x.zeroize();
        }
        Ok(Aes { rk, rounds })
    }

    fn encrypt_planes(&self, p: &mut [u64; 8]) {
        add_round_key(p, &self.rk[0]);
        for r in 1..self.rounds {
            sub_bytes(p);
            shift_rows(p);
            mix_columns(p);
            add_round_key(p, &self.rk[r]);
        }
        sub_bytes(p);
        shift_rows(p);
        add_round_key(p, &self.rk[self.rounds]);
    }

    /// Encrypt up to four blocks in place (one bitsliced pass). Panics on more than four.
    pub fn encrypt_blocks(&self, blocks: &mut [[u8; 16]]) {
        assert!(blocks.len() <= 4);
        let mut p = pack(blocks);
        self.encrypt_planes(&mut p);
        unpack(&p, blocks);
        p.zeroize();
    }

    /// Encrypt one block in place.
    pub fn encrypt_block(&self, block: &mut [u8; 16]) {
        let mut b = [*block];
        self.encrypt_blocks(&mut b);
        *block = b[0];
    }
}

impl Drop for Aes {
    fn drop(&mut self) {
        for r in self.rk.iter_mut() {
            r.zeroize();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The S-box from its definition (FIPS 197 §5.1.1): multiplicative inverse in GF(2^8) then the
    /// affine map — computed the slow, obviously-correct way, as the oracle for the circuit.
    fn sbox_ref(x: u8) -> u8 {
        fn mul(mut a: u8, mut b: u8) -> u8 {
            let mut r = 0u8;
            while b != 0 {
                if b & 1 != 0 {
                    r ^= a;
                }
                let hi = a & 0x80;
                a <<= 1;
                if hi != 0 {
                    a ^= 0x1b;
                }
                b >>= 1;
            }
            r
        }
        let inv = if x == 0 { 0 } else { (1..=255u8).find(|&y| mul(x, y) == 1).unwrap() };
        let mut s = inv;
        for k in 1..5 {
            s ^= inv.rotate_left(k);
        }
        s ^ 0x63
    }

    #[test]
    fn sbox_circuit_matches_definition() {
        for x in 0..=255u8 {
            assert_eq!(sbox(x), sbox_ref(x), "x={x:#04x}");
        }
    }

    #[test]
    fn fips197_appendix_c() {
        // C.1 AES-128, C.3 AES-256
        let pt: [u8; 16] = core::array::from_fn(|i| (i as u8) * 0x11);
        let k128: [u8; 16] = core::array::from_fn(|i| i as u8);
        let k256: [u8; 32] = core::array::from_fn(|i| i as u8);
        let mut b = pt;
        Aes::new(&k128).unwrap().encrypt_block(&mut b);
        assert_eq!(b, [0x69, 0xc4, 0xe0, 0xd8, 0x6a, 0x7b, 0x04, 0x30, 0xd8, 0xcd, 0xb7, 0x80, 0x70, 0xb4, 0xc5, 0x5a]);
        let mut b = pt;
        Aes::new(&k256).unwrap().encrypt_block(&mut b);
        assert_eq!(b, [0x8e, 0xa2, 0xb7, 0xca, 0x51, 0x67, 0x45, 0xbf, 0xea, 0xfc, 0x49, 0x90, 0x4b, 0x49, 0x60, 0x89]);
    }
}
