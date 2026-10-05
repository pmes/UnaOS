// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! RSA signature VERIFICATION (RFC 8017, PKCS #1 v2.2): RSASSA-PSS (§8.1.2, EMSA-PSS-VERIFY §9.1.2,
//! MGF1 §B.2.1) and RSASSA-PKCS1-v1_5 (§8.2.2, EMSA-PKCS1-v1_5 §9.2). No key generation, no signing,
//! no decryption: UnaOS needs RSA only to check the signatures other people's certificates and TLS
//! servers carry.
//!
//! Arithmetic: RSAVP1 `s^e mod n` by Montgomery multiplication (CIOS) over a runtime number of 64-bit
//! limbs, moduli of [`MIN_BITS`]..=[`MAX_BITS`] bits, no allocation (fixed 128-limb buffers).
//!
//! PKCS #1 v1.5 is checked the robust way (RFC 8017 §8.2.2 step 3, "compare"): the expected encoded
//! message `00 01 FF..FF 00 || DigestInfo || H` is CONSTRUCTED and compared byte for byte with the
//! recovered one, so no ASN.1 is parsed from attacker-controlled data (the Bleichenbacher-2006 class of
//! forgeries — BER lengths, garbage after the hash, missing NULL — cannot pass).
//!
//! CONSTANT-TIME: not required and not claimed — every input of a verification (key, message, signature)
//! is public. The code still has no secret to leak.
//!
//! Hashes: SHA-224/256/384/512 (this crate carries no SHA-1; SHA-1 signatures are refused as
//! `Error::Param`, which a caller reports as unsupported).

use crate::sha2::{Digest, Sha224, Sha256, Sha384, Sha512};
use crate::Error;

/// Smallest modulus accepted, in bits. (Policy — e.g. TLS's 2048-bit floor — is the caller's.)
pub const MIN_BITS: usize = 1024;
/// Largest modulus accepted, in bits.
pub const MAX_BITS: usize = 8192;
const MAXL: usize = MAX_BITS / 64;
const MAXB: usize = MAX_BITS / 8;

/// The hash a signature is made with (also the MGF1 hash for PSS, unless given separately).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hash {
    /// SHA-224.
    Sha224,
    /// SHA-256.
    Sha256,
    /// SHA-384.
    Sha384,
    /// SHA-512.
    Sha512,
}

impl Hash {
    /// Output length in bytes.
    pub const fn len(self) -> usize {
        match self {
            Hash::Sha224 => 28,
            Hash::Sha256 => 32,
            Hash::Sha384 => 48,
            Hash::Sha512 => 64,
        }
    }

    /// The hash of the concatenation of `parts` into `out[..len]`.
    fn digest(self, parts: &[&[u8]], out: &mut [u8; 64]) {
        fn run<D: Digest>(parts: &[&[u8]], out: &mut [u8; 64]) {
            let mut h = D::new();
            for p in parts {
                h.update(p);
            }
            h.finalize_into(out);
        }
        match self {
            Hash::Sha224 => run::<Sha224>(parts, out),
            Hash::Sha256 => run::<Sha256>(parts, out),
            Hash::Sha384 => run::<Sha384>(parts, out),
            Hash::Sha512 => run::<Sha512>(parts, out),
        }
    }

    /// RFC 8017 §9.2 note 1: the DER of `DigestInfo` up to (excluding) the hash value.
    const fn digest_info_prefix(self) -> &'static [u8] {
        match self {
            Hash::Sha224 => &[0x30, 0x2d, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x04, 0x05, 0x00, 0x04, 0x1c],
            Hash::Sha256 => &[0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00, 0x04, 0x20],
            Hash::Sha384 => &[0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02, 0x05, 0x00, 0x04, 0x30],
            Hash::Sha512 => &[0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03, 0x05, 0x00, 0x04, 0x40],
        }
    }
}

type Limbs = [u64; MAXL];

/// An RSA public key (n, e), validated: n odd, MIN_BITS..=MAX_BITS bits; 3 <= e < n, e odd.
#[derive(Clone)]
pub struct PublicKey {
    n: Limbs,
    e: Limbs,
    /// Limbs in use.
    l: usize,
    /// Modulus length in bytes (k) and bits.
    k: usize,
    bits: usize,
    n0inv: u64,
    r2: Limbs,
}

impl core::fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "rsa::PublicKey({} bits)", self.bits)
    }
}

fn load_be(b: &[u8], out: &mut Limbs) -> usize {
    // returns the number of significant limbs
    let mut n = 0;
    for (i, &byte) in b.iter().rev().enumerate() {
        out[i / 8] |= (byte as u64) << (8 * (i % 8));
        n = i / 8 + 1;
    }
    while n > 0 && out[n - 1] == 0 {
        n -= 1;
    }
    n
}

/// a < b over the first `l` limbs.
fn lt(a: &Limbs, b: &Limbs, l: usize) -> bool {
    for i in (0..l).rev() {
        if a[i] != b[i] {
            return a[i] < b[i];
        }
    }
    false
}

/// a -= b over `l` limbs; returns the borrow.
fn sub_in(a: &mut Limbs, b: &Limbs, l: usize) -> u64 {
    let mut br = 0u64;
    for i in 0..l {
        let t = (a[i] as u128).wrapping_sub(b[i] as u128 + br as u128);
        a[i] = t as u64;
        br = ((t >> 64) as u64) & 1;
    }
    br
}

impl PublicKey {
    /// From the big-endian modulus and exponent (as in `RSAPublicKey`; leading zero bytes allowed).
    pub fn new(n: &[u8], e: &[u8]) -> Result<Self, Error> {
        let mut nn = [0u64; MAXL];
        let mut ee = [0u64; MAXL];
        let nz = n.iter().position(|&b| b != 0).unwrap_or(n.len());
        let n = &n[nz..];
        if n.len() > MAXB {
            return Err(Error::Param);
        }
        let ez = e.iter().position(|&b| b != 0).unwrap_or(e.len());
        let e = &e[ez..];
        if e.len() > n.len() {
            return Err(Error::Param);
        }
        let l = load_be(n, &mut nn);
        let el = load_be(e, &mut ee);
        let bits = if l == 0 { 0 } else { 64 * (l - 1) + (64 - nn[l - 1].leading_zeros() as usize) };
        if !(MIN_BITS..=MAX_BITS).contains(&bits) || nn[0] & 1 == 0 {
            return Err(Error::Param);
        }
        if el == 0 || ee[0] & 1 == 0 || (el == 1 && ee[0] < 3) || !lt(&ee, &nn, l) {
            return Err(Error::Param);
        }
        let mut inv: u64 = 1;
        for _ in 0..6 {
            inv = inv.wrapping_mul(2u64.wrapping_sub(nn[0].wrapping_mul(inv)));
        }
        // R^2 mod n by 2 * 64l modular doublings of 1 (public data: variable time is fine).
        let mut x = [0u64; MAXL];
        x[0] = 1;
        for _ in 0..128 * l {
            let mut carry = 0u64;
            for limb in x.iter_mut().take(l) {
                let nc = *limb >> 63;
                *limb = (*limb << 1) | carry;
                carry = nc;
            }
            if carry == 1 || !lt(&x, &nn, l) {
                sub_in(&mut x, &nn, l);
            }
        }
        Ok(PublicKey { n: nn, e: ee, l, k: bits.div_ceil(8), bits, n0inv: inv.wrapping_neg(), r2: x })
    }

    /// Modulus size in bits.
    pub fn bits(&self) -> usize {
        self.bits
    }

    /// Modulus size in bytes (the signature length k).
    pub fn size(&self) -> usize {
        self.k
    }

    fn mont_mul(&self, a: &Limbs, b: &Limbs) -> Limbs {
        let l = self.l;
        let n = &self.n;
        let mut t = [0u64; MAXL + 2];
        for i in 0..l {
            let mut c = 0u64;
            for j in 0..l {
                let v = t[j] as u128 + (a[j] as u128) * (b[i] as u128) + c as u128;
                t[j] = v as u64;
                c = (v >> 64) as u64;
            }
            let v = t[l] as u128 + c as u128;
            t[l] = v as u64;
            t[l + 1] = (v >> 64) as u64;
            let q = t[0].wrapping_mul(self.n0inv);
            let v = t[0] as u128 + (q as u128) * (n[0] as u128);
            let mut c = (v >> 64) as u64;
            for j in 1..l {
                let v = t[j] as u128 + (q as u128) * (n[j] as u128) + c as u128;
                t[j - 1] = v as u64;
                c = (v >> 64) as u64;
            }
            let v = t[l] as u128 + c as u128;
            t[l - 1] = v as u64;
            t[l] = t[l + 1] + (v >> 64) as u64;
        }
        let mut r = [0u64; MAXL];
        r[..l].copy_from_slice(&t[..l]);
        if t[l] != 0 || !lt(&r, n, l) {
            sub_in(&mut r, n, l);
        }
        r
    }

    /// RSAVP1 (§5.2.2): `s^e mod n` written as `k` big-endian bytes into `em[..k]`. `sig` must be exactly
    /// `k` bytes and represent an integer below n.
    fn rsavp1(&self, sig: &[u8], em: &mut [u8; MAXB]) -> Result<(), Error> {
        if sig.len() != self.k {
            return Err(Error::Length);
        }
        let mut s = [0u64; MAXL];
        load_be(sig, &mut s);
        if !lt(&s, &self.n, self.l) {
            return Err(Error::Auth);
        }
        let base = self.mont_mul(&s, &self.r2);
        let el = (0..self.l).rev().find(|&i| self.e[i] != 0).unwrap();
        let top = 63 - self.e[el].leading_zeros() as usize;
        let mut acc = base;
        for i in (0..64 * el + top).rev() {
            acc = self.mont_mul(&acc, &acc);
            if (self.e[i / 64] >> (i % 64)) & 1 == 1 {
                acc = self.mont_mul(&acc, &base);
            }
        }
        let mut one = [0u64; MAXL];
        one[0] = 1;
        let m = self.mont_mul(&acc, &one);
        for i in 0..self.k {
            em[self.k - 1 - i] = (m[i / 8] >> (8 * (i % 8))) as u8;
        }
        Ok(())
    }
}

/// RSASSA-PKCS1-v1_5 verification (RFC 8017 §8.2.2) of `sig` over the message digest `digest`
/// (`hash.len()` bytes).
pub fn verify_pkcs1v15_prehashed(key: &PublicKey, hash: Hash, digest: &[u8], sig: &[u8]) -> Result<(), Error> {
    if digest.len() != hash.len() {
        return Err(Error::Length);
    }
    let mut em = [0u8; MAXB];
    key.rsavp1(sig, &mut em)?;
    let k = key.k;
    let prefix = hash.digest_info_prefix();
    let t_len = prefix.len() + digest.len();
    if k < t_len + 11 {
        return Err(Error::Auth); // "intended encoded message length too short"
    }
    let mut want = [0u8; MAXB];
    want[1] = 0x01;
    for b in &mut want[2..k - t_len - 1] {
        *b = 0xff;
    }
    want[k - t_len - 1] = 0x00;
    want[k - t_len..k - digest.len()].copy_from_slice(prefix);
    want[k - digest.len()..k].copy_from_slice(digest);
    if crate::ct::ct_eq(&em[..k], &want[..k]) { Ok(()) } else { Err(Error::Auth) }
}

/// RSASSA-PKCS1-v1_5 verification over a message.
pub fn verify_pkcs1v15(key: &PublicKey, hash: Hash, msg: &[u8], sig: &[u8]) -> Result<(), Error> {
    let mut d = [0u8; 64];
    hash.digest(&[msg], &mut d);
    verify_pkcs1v15_prehashed(key, hash, &d[..hash.len()], sig)
}

/// MGF1 (§B.2.1) XORed into `out`.
fn mgf1_xor(h: Hash, seed: &[u8], out: &mut [u8]) {
    let hl = h.len();
    let mut d = [0u8; 64];
    for (counter, chunk) in out.chunks_mut(hl).enumerate() {
        h.digest(&[seed, &(counter as u32).to_be_bytes()], &mut d);
        for (o, m) in chunk.iter_mut().zip(d.iter()) {
            *o ^= m;
        }
    }
}

/// RSASSA-PSS verification (RFC 8017 §8.1.2 / EMSA-PSS-VERIFY §9.1.2) over the message digest `digest`
/// (`hash.len()` bytes), MGF1 with `mgf_hash`, salt length `salt_len` (TLS 1.3: = hash length, with
/// `mgf_hash == hash`).
pub fn verify_pss_prehashed(key: &PublicKey, hash: Hash, mgf_hash: Hash, salt_len: usize, digest: &[u8], sig: &[u8]) -> Result<(), Error> {
    if digest.len() != hash.len() {
        return Err(Error::Length);
    }
    let mut buf = [0u8; MAXB];
    key.rsavp1(sig, &mut buf)?;
    // I2OSP(m, emLen) with emBits = modBits - 1: when emLen < k the leading byte must be zero.
    let em_bits = key.bits - 1;
    let em_len = em_bits.div_ceil(8);
    let off = key.k - em_len;
    if off == 1 && buf[0] != 0 {
        return Err(Error::Auth);
    }
    let em = &mut buf[off..key.k];
    let hl = hash.len();
    if em_len < hl + salt_len + 2 || em[em_len - 1] != 0xbc {
        return Err(Error::Auth);
    }
    let db_len = em_len - hl - 1;
    let mut h = [0u8; 64];
    h[..hl].copy_from_slice(&em[db_len..db_len + hl]);
    let top_mask = 0xffu8 >> (8 * em_len - em_bits);
    if em[0] & !top_mask != 0 {
        return Err(Error::Auth);
    }
    let db = &mut em[..db_len];
    mgf1_xor(mgf_hash, &h[..hl], db);
    db[0] &= top_mask;
    let ps_len = db_len - salt_len - 1;
    if db[..ps_len].iter().any(|&b| b != 0) || db[ps_len] != 0x01 {
        return Err(Error::Auth);
    }
    let salt = &db[db_len - salt_len..];
    let mut h2 = [0u8; 64];
    hash.digest(&[&[0u8; 8], digest, salt], &mut h2);
    if crate::ct::ct_eq(&h[..hl], &h2[..hl]) { Ok(()) } else { Err(Error::Auth) }
}

/// RSASSA-PSS verification over a message, MGF1 with the same hash.
pub fn verify_pss(key: &PublicKey, hash: Hash, salt_len: usize, msg: &[u8], sig: &[u8]) -> Result<(), Error> {
    let mut d = [0u8; 64];
    hash.digest(&[msg], &mut d);
    verify_pss_prehashed(key, hash, hash, salt_len, &d[..hash.len()], sig)
}
