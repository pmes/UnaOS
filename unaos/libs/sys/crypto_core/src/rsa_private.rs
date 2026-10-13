// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! RSA with a PRIVATE key (RFC 8017, PKCS #1 v2.2) — the half [`crate::rsa`] leaves out: key generation,
//! RSADP / RSASP1 by the Chinese Remainder Theorem (§5.1.2 / §5.2.1, second representation), RSASSA-PKCS1-v1_5
//! signing (§8.2.1, EMSA-PKCS1-v1_5 §9.2), RSASSA-PSS signing (§8.1.1, EMSA-PSS-ENCODE §9.1.1), RSAES-OAEP
//! decryption (§7.1.2) — and the public operations those need beside them, generic over the hash (SHA-1
//! included, which the Web Cryptography API still registers for RSA): RSAES-OAEP encryption (§7.1.1),
//! RSASSA-PKCS1-v1_5 and RSASSA-PSS verification with any salt length.
//!
//! First consumer: Aether's `crypto.subtle` (AETHERWPT3, RSASSA-PKCS1-v1_5 / RSA-PSS / RSA-OAEP).
//!
//! Arithmetic: fixed-capacity little-endian `u64` limbs (no allocation), moduli of [`MIN_BITS`]..=[`MAX_BITS`]
//! bits. Montgomery multiplication (CIOS) with a masked final subtraction; exponentiation by a fixed 4-bit
//! window over every bit position of the modulus with a full-table masked lookup; reduction modulo a secret
//! prime by shift-and-subtract with a masked subtraction (the iteration count depends only on the dividend's
//! limb count).
//!
//! CONSTANT-TIME:
//!  * [`PrivateKey::decrypt_raw`] and everything over it (signing, OAEP decryption): the exponentiations and
//!    reductions by p, q, dp, dq, qi are; the CRT recombination's multiplication and the final fault check
//!    (`m^e mod n == c`, Boneh–DeMillo–Lipton) are schoolbook over public lengths. OAEP decoding gathers its
//!    three checks into one flag before it branches (RFC 8017 §7.1.2 note: one error for every failure).
//!  * [`PrivateKey::generate`]: NOT claimed — candidate primes are trial-divided and rejected in variable time
//!    (the rejected candidates are discarded; the accepted primes' Miller–Rabin rounds use the constant-time
//!    exponentiation).
//!  * The public operations: not required — every input is public.

use crate::ct::Zeroize;
use crate::sha2::Digest;
use crate::Error;

/// Smallest modulus accepted, in bits. (Policy floors are the caller's: WebCrypto sets none.)
pub const MIN_BITS: usize = 256;
/// Largest modulus accepted, in bits.
pub const MAX_BITS: usize = 8192;
/// Largest modulus in bytes (the longest signature / ciphertext).
pub const MAX_BYTES: usize = MAX_BITS / 8;
const L: usize = MAX_BITS / 64;
/// Width of a general number: a product of two moduli plus slack.
const W: usize = 2 * L + 2;

type Limbs = [u64; L];
type Wide = [u64; W];

// ---- fixed-width naturals (variable time unless a function says otherwise) ----------------------------------

fn nlimbs(a: &Wide) -> usize {
    let mut n = W;
    while n > 0 && a[n - 1] == 0 {
        n -= 1;
    }
    n
}

fn nbits(a: &Wide) -> usize {
    let n = nlimbs(a);
    if n == 0 { 0 } else { 64 * (n - 1) + 64 - a[n - 1].leading_zeros() as usize }
}

fn small(v: u64) -> Wide {
    let mut a = [0u64; W];
    a[0] = v;
    a
}

/// Big-endian bytes (leading zeros allowed) to a number; `Error::Length` past the capacity.
fn from_be(b: &[u8]) -> Result<Wide, Error> {
    let nz = b.iter().position(|&x| x != 0).unwrap_or(b.len());
    let b = &b[nz..];
    if b.len() > W * 8 {
        return Err(Error::Length);
    }
    let mut a = [0u64; W];
    for (i, &byte) in b.iter().rev().enumerate() {
        a[i / 8] |= (byte as u64) << (8 * (i % 8));
    }
    Ok(a)
}

/// The low `out.len()` bytes of `a`, big-endian (I2OSP when `a` fits).
fn to_be(a: &Wide, out: &mut [u8]) {
    let n = out.len();
    for i in 0..n {
        out[n - 1 - i] = if i / 8 < W { (a[i / 8] >> (8 * (i % 8))) as u8 } else { 0 };
    }
}

fn cmp(a: &Wide, b: &Wide) -> core::cmp::Ordering {
    for i in (0..W).rev() {
        if a[i] != b[i] {
            return a[i].cmp(&b[i]);
        }
    }
    core::cmp::Ordering::Equal
}

fn add(a: &Wide, b: &Wide) -> Wide {
    let mut r = [0u64; W];
    let mut c = 0u64;
    for i in 0..W {
        let v = a[i] as u128 + b[i] as u128 + c as u128;
        r[i] = v as u64;
        c = (v >> 64) as u64;
    }
    r
}

/// `a - b` for `a >= b`.
fn sub(a: &Wide, b: &Wide) -> Wide {
    let mut r = [0u64; W];
    let mut br = 0u64;
    for i in 0..W {
        let v = (a[i] as u128).wrapping_sub(b[i] as u128 + br as u128);
        r[i] = v as u64;
        br = ((v >> 64) as u64) & 1;
    }
    r
}

/// `a * b`; the operands' limb counts must sum to at most `W` (true for any two numbers below 2^MAX_BITS).
fn mul(a: &Wide, b: &Wide) -> Wide {
    let (la, lb) = (nlimbs(a), nlimbs(b));
    let mut r = [0u64; W];
    for i in 0..la {
        let mut c = 0u64;
        for j in 0..lb {
            if i + j >= W {
                break;
            }
            let v = r[i + j] as u128 + (a[i] as u128) * (b[j] as u128) + c as u128;
            r[i + j] = v as u64;
            c = (v >> 64) as u64;
        }
        if i + lb < W {
            r[i + lb] = c;
        }
    }
    r
}

fn mul_small(a: &Wide, s: u64) -> Wide {
    let mut r = [0u64; W];
    let mut c = 0u64;
    for i in 0..W {
        let v = (a[i] as u128) * (s as u128) + c as u128;
        r[i] = v as u64;
        c = (v >> 64) as u64;
    }
    r
}

/// `(a / s, a mod s)` for a one-limb divisor.
fn divrem_small(a: &Wide, s: u64) -> (Wide, u64) {
    let mut q = [0u64; W];
    let mut r = 0u128;
    for i in (0..nlimbs(a)).rev() {
        let cur = (r << 64) | a[i] as u128;
        q[i] = (cur / s as u128) as u64;
        r = cur % s as u128;
    }
    (q, r as u64)
}

/// `a mod m` by shift-and-subtract with a MASKED subtraction: the work depends on `a`'s limb count and `m`'s,
/// never on their values (`m` may be a secret prime).
fn rem_ct(a: &Wide, m: &Wide) -> Wide {
    let ml = nlimbs(m);
    let mut r = [0u64; W];
    if ml == 0 || ml + 1 > W {
        return r;
    }
    let abits = 64 * nlimbs(a);
    for i in (0..abits).rev() {
        // r = 2r + bit (r < m, so 2r + 1 < 2m fits in ml + 1 limbs)
        let mut carry = (a[i / 64] >> (i % 64)) & 1;
        for limb in r.iter_mut().take(ml + 1) {
            let nc = *limb >> 63;
            *limb = (*limb << 1) | carry;
            carry = nc;
        }
        // t = r - m; keep t when there was no borrow
        let mut t = [0u64; W];
        let mut br = 0u64;
        for j in 0..=ml {
            let v = (r[j] as u128).wrapping_sub(m[j] as u128 + br as u128);
            t[j] = v as u64;
            br = ((v >> 64) as u64) & 1;
        }
        let keep = br.wrapping_sub(1); // borrow 0 → all ones (take t)
        for j in 0..=ml {
            r[j] = (t[j] & keep) | (r[j] & !keep);
        }
    }
    r
}

fn is_one(a: &Wide) -> bool {
    a[0] == 1 && a[1..].iter().all(|&x| x == 0)
}

// ---- Montgomery arithmetic modulo an odd m --------------------------------------------------------------------

struct Mont {
    m: Limbs,
    l: usize,
    n0: u64,
    r2: Limbs,
    one: Limbs,
}

impl Mont {
    fn new(m: &Wide) -> Option<Mont> {
        let l = nlimbs(m);
        if l == 0 || l > L || m[0] & 1 == 0 {
            return None;
        }
        let mut inv: u64 = 1;
        for _ in 0..6 {
            inv = inv.wrapping_mul(2u64.wrapping_sub(m[0].wrapping_mul(inv)));
        }
        // R^2 mod m, R = 2^(64 l)
        let mut big = [0u64; W];
        big[2 * l] = 1;
        let r2w = rem_ct(&big, m);
        let mut mm = [0u64; L];
        mm[..l].copy_from_slice(&m[..l]);
        let mut r2 = [0u64; L];
        r2[..l].copy_from_slice(&r2w[..l]);
        let mut me = Mont { m: mm, l, n0: inv.wrapping_neg(), r2, one: [0u64; L] };
        let mut one = [0u64; L];
        one[0] = 1;
        me.one = me.mul(&one, &me.r2); // R mod m
        Some(me)
    }

    /// CIOS Montgomery product `a b R^-1 mod m` (inputs below m), the final subtraction masked.
    fn mul(&self, a: &Limbs, b: &Limbs) -> Limbs {
        let l = self.l;
        let n = &self.m;
        let mut t = [0u64; L + 2];
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
            let q = t[0].wrapping_mul(self.n0);
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
        // t < 2m: subtract m unless that borrows out of the l + 1 limbs.
        let mut s = [0u64; L];
        let mut br = 0u64;
        for j in 0..l {
            let v = (t[j] as u128).wrapping_sub(n[j] as u128 + br as u128);
            s[j] = v as u64;
            br = ((v >> 64) as u64) & 1;
        }
        let take = ((t[l] & 1) | (br ^ 1)).wrapping_neg(); // t[l] = 1, or no borrow → all ones
        let mut r = [0u64; L];
        for j in 0..l {
            r[j] = (s[j] & take) | (t[j] & !take);
        }
        r
    }

    /// Into Montgomery form (`a` below m).
    fn to(&self, a: &Wide) -> Limbs {
        let mut x = [0u64; L];
        x[..self.l].copy_from_slice(&a[..self.l]);
        self.mul(&x, &self.r2)
    }

    fn from(&self, a: &Limbs) -> Wide {
        let mut one = [0u64; L];
        one[0] = 1;
        let r = self.mul(a, &one);
        let mut w = [0u64; W];
        w[..self.l].copy_from_slice(&r[..self.l]);
        w
    }

    /// `base^e` (Montgomery form in and out) over the low `ebits` bits of `e`, a fixed 4-bit window and a
    /// masked scan of the whole table per window: the sequence of operations depends only on `ebits`.
    fn pow(&self, base: &Limbs, e: &Wide, ebits: usize) -> Limbs {
        let mut table = [[0u64; L]; 16];
        table[0] = self.one;
        table[1] = *base;
        for i in 2..16 {
            table[i] = self.mul(&table[i - 1], base);
        }
        let mut acc = self.one;
        let windows = ebits.div_ceil(4);
        for w in (0..windows).rev() {
            for _ in 0..4 {
                acc = self.mul(&acc, &acc);
            }
            let bit = 4 * w;
            let mut idx = 0u64;
            for k in 0..4 {
                let b = bit + k;
                if b / 64 < W {
                    idx |= ((e[b / 64] >> (b % 64)) & 1) << k;
                }
            }
            let mut sel = [0u64; L];
            for (t, row) in table.iter().enumerate() {
                let hit = ((t as u64 ^ idx).wrapping_sub(1) >> 63).wrapping_neg(); // t == idx → all ones
                for j in 0..self.l {
                    sel[j] |= row[j] & hit;
                }
            }
            acc = self.mul(&acc, &sel);
        }
        for row in table.iter_mut() {
            row.zeroize();
        }
        acc
    }
}

/// `b^e mod m` for an odd m (`b` reduced first). Constant-time in `b` and `e` over `ebits`.
fn modpow(b: &Wide, e: &Wide, ebits: usize, m: &Mont, mw: &Wide) -> Wide {
    let br = rem_ct(b, mw);
    let x = m.to(&br);
    let y = m.pow(&x, e, ebits);
    m.from(&y)
}

/// Miller–Rabin (FIPS 186-5 B.3.1) with `rounds` random bases in [2, n - 2].
fn probably_prime(n: &Wide, rounds: usize, rng: &mut dyn FnMut(&mut [u8])) -> bool {
    let Some(m) = Mont::new(n) else { return false };
    let nm1 = sub(n, &small(1));
    let mut s = 0usize;
    let mut d = nm1;
    while d[0] & 1 == 0 {
        // d >>= 1
        for i in 0..W {
            d[i] = (d[i] >> 1) | if i + 1 < W { d[i + 1] << 63 } else { 0 };
        }
        s += 1;
    }
    let bits = nbits(n);
    let minus_one = m.to(&nm1);
    let nm3 = sub(n, &small(3));
    let mut buf = [0u8; MAX_BYTES / 2 + 16];
    let nbytes = bits.div_ceil(8) + 8;
    'round: for _ in 0..rounds {
        rng(&mut buf[..nbytes]);
        let Ok(r) = from_be(&buf[..nbytes]) else { return false };
        let a = add(&rem_ct(&r, &nm3), &small(2));
        let mut x = m.pow(&m.to(&a), &d, bits);
        if x == m.one || x == minus_one {
            continue;
        }
        for _ in 1..s {
            x = m.mul(&x, &x);
            if x == minus_one {
                continue 'round;
            }
        }
        buf.zeroize();
        return false;
    }
    buf.zeroize();
    true
}

/// The inverse of `a` modulo `m` for one-limb numbers (None when they are not coprime).
fn inv_u64(a: u64, m: u64) -> Option<u64> {
    let (mut r0, mut r1) = (m as i128, (a % m) as i128);
    let (mut t0, mut t1) = (0i128, 1i128);
    while r1 != 0 {
        let q = r0 / r1;
        (r0, r1) = (r1, r0 - q * r1);
        (t0, t1) = (t1, t0 - q * t1);
    }
    if r0 != 1 {
        return None;
    }
    Some(t0.rem_euclid(m as i128) as u64)
}

fn gcd_u64(mut a: u64, mut b: u64) -> u64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// A random prime of exactly `bits` bits whose two top bits are set (so a product of two has exactly the sum
/// of their sizes), with `gcd(p - 1, e) = 1` (FIPS 186-5 A.1.3 shape; Miller–Rabin rounds from Table B.1).
fn gen_prime(bits: usize, e: u64, rng: &mut dyn FnMut(&mut [u8])) -> Wide {
    let nbytes = bits.div_ceil(8);
    let rounds = if bits >= 1536 { 4 } else if bits >= 1024 { 5 } else { 8 };
    let mut buf = [0u8; MAX_BYTES / 2 + 8];
    loop {
        rng(&mut buf[..nbytes]);
        let Ok(mut c) = from_be(&buf[..nbytes]) else { continue };
        for i in bits..64 * W {
            c[i / 64] &= !(1u64 << (i % 64));
        }
        c[(bits - 1) / 64] |= 1u64 << ((bits - 1) % 64);
        c[(bits - 2) / 64] |= 1u64 << ((bits - 2) % 64);
        c[0] |= 1;
        // trial division by every odd number below 2000 (a composite divisor implies a prime one)
        let mut composite = false;
        let mut f = 3u64;
        while f < 2000 {
            if divrem_small(&c, f).1 == 0 {
                composite = true;
                break;
            }
            f += 2;
        }
        if composite {
            continue;
        }
        let pm1_mod_e = divrem_small(&sub(&c, &small(1)), e).1;
        if gcd_u64(e, pm1_mod_e) != 1 {
            continue;
        }
        if probably_prime(&c, rounds, rng) {
            buf.zeroize();
            return c;
        }
    }
}

// ---- the keys ---------------------------------------------------------------------------------------------------

/// An RSA public key (n, e) for the operations here: n odd, [`MIN_BITS`]..=[`MAX_BITS`] bits; e odd, 3 <= e < n.
#[derive(Clone)]
pub struct PublicKey {
    n: Wide,
    e: Wide,
    bits: usize,
    k: usize,
}

impl core::fmt::Debug for PublicKey {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "rsa_private::PublicKey({} bits)", self.bits)
    }
}

impl PublicKey {
    /// From the big-endian modulus and exponent (leading zero bytes allowed).
    pub fn new(n: &[u8], e: &[u8]) -> Result<Self, Error> {
        let n = from_be(n)?;
        let e = from_be(e)?;
        let bits = nbits(&n);
        if !(MIN_BITS..=MAX_BITS).contains(&bits) || n[0] & 1 == 0 {
            return Err(Error::Param);
        }
        if e[0] & 1 == 0 || cmp(&e, &small(3)) == core::cmp::Ordering::Less || cmp(&e, &n) != core::cmp::Ordering::Less {
            return Err(Error::Param);
        }
        Ok(PublicKey { n, e, bits, k: bits.div_ceil(8) })
    }

    /// Modulus size in bits.
    pub fn bits(&self) -> usize {
        self.bits
    }

    /// Modulus size in bytes (k: every signature and ciphertext is this long).
    pub fn size(&self) -> usize {
        self.k
    }

    /// The modulus, big-endian, into `out[..k]`; returns k.
    pub fn n_be(&self, out: &mut [u8]) -> usize {
        to_be(&self.n, &mut out[..self.k]);
        self.k
    }

    /// The public exponent, big-endian and minimal, into `out`; returns its length.
    pub fn e_be(&self, out: &mut [u8]) -> usize {
        let len = nbits(&self.e).div_ceil(8);
        to_be(&self.e, &mut out[..len]);
        len
    }

    /// RSAEP / RSAVP1 (§5.1.1 / §5.2.2): `x^e mod n` for a `k`-byte representative below n, into `out[..k]`.
    fn public_raw(&self, x: &[u8], out: &mut [u8]) -> Result<(), Error> {
        if x.len() != self.k {
            return Err(Error::Length);
        }
        let v = from_be(x)?;
        if cmp(&v, &self.n) != core::cmp::Ordering::Less {
            return Err(Error::Param);
        }
        let m = Mont::new(&self.n).ok_or(Error::Param)?;
        let r = modpow(&v, &self.e, nbits(&self.e), &m, &self.n);
        to_be(&r, &mut out[..self.k]);
        Ok(())
    }

    /// RSAES-OAEP-ENCRYPT (§7.1.1) with MGF1 over `D`, the given label and a caller-drawn `seed`
    /// (`D::OUTPUT_LEN` random bytes). Writes k bytes into `out`; `Error::Length` when the message is longer
    /// than `k - 2 hLen - 2`.
    pub fn encrypt_oaep<D: Digest>(&self, label: &[u8], msg: &[u8], seed: &[u8], out: &mut [u8]) -> Result<usize, Error> {
        let k = self.k;
        let hl = D::OUTPUT_LEN;
        if seed.len() != hl || k < 2 * hl + 2 || msg.len() > k - 2 * hl - 2 || out.len() < k {
            return Err(Error::Length);
        }
        let mut em = [0u8; MAX_BYTES];
        let (_, rest) = em[..k].split_at_mut(1);
        let (mseed, db) = rest.split_at_mut(hl);
        // DB = lHash || PS || 0x01 || M
        hash_into::<D>(&[label], &mut db[..hl]);
        let db_len = k - hl - 1;
        db[db_len - msg.len() - 1] = 0x01;
        db[db_len - msg.len()..].copy_from_slice(msg);
        mseed.copy_from_slice(seed);
        mgf1_xor::<D>(seed, db);
        let mut sm = [0u8; 64];
        sm[..hl].fill(0);
        mgf1_xor::<D>(db, &mut sm[..hl]);
        for (s, m) in mseed.iter_mut().zip(sm.iter()) {
            *s ^= m;
        }
        let r = self.public_raw(&em[..k], out);
        em.zeroize();
        r.map(|_| k)
    }

    /// RSASSA-PKCS1-v1_5 verification (§8.2.2) of `sig` over a message digest; the DigestInfo is chosen by the
    /// digest's length (SHA-1, SHA-224, SHA-256, SHA-384 or SHA-512). Compares the constructed encoding.
    pub fn verify_pkcs1v15(&self, digest: &[u8], sig: &[u8]) -> Result<(), Error> {
        let k = self.k;
        let mut em = [0u8; MAX_BYTES];
        if sig.len() != k {
            return Err(Error::Auth);
        }
        self.public_raw(sig, &mut em).map_err(|_| Error::Auth)?;
        let mut want = [0u8; MAX_BYTES];
        pkcs1v15_encode(digest, &mut want[..k])?;
        if crate::ct::ct_eq(&em[..k], &want[..k]) { Ok(()) } else { Err(Error::Auth) }
    }

    /// RSASSA-PSS verification (§8.1.2, EMSA-PSS-VERIFY §9.1.2) with MGF1 over `D` and salt length `salt_len`.
    pub fn verify_pss<D: Digest>(&self, digest: &[u8], salt_len: usize, sig: &[u8]) -> Result<(), Error> {
        let hl = D::OUTPUT_LEN;
        if digest.len() != hl || sig.len() != self.k {
            return Err(Error::Auth);
        }
        let mut buf = [0u8; MAX_BYTES];
        self.public_raw(sig, &mut buf).map_err(|_| Error::Auth)?;
        let em_bits = self.bits - 1;
        let em_len = em_bits.div_ceil(8);
        let off = self.k - em_len;
        if off == 1 && buf[0] != 0 {
            return Err(Error::Auth);
        }
        let em = &mut buf[off..self.k];
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
        mgf1_xor::<D>(&h[..hl], db);
        db[0] &= top_mask;
        let ps_len = db_len - salt_len - 1;
        if db[..ps_len].iter().any(|&b| b != 0) || db[ps_len] != 0x01 {
            return Err(Error::Auth);
        }
        let mut h2 = [0u8; 64];
        hash_into::<D>(&[&[0u8; 8], digest, &db[db_len - salt_len..]], &mut h2[..hl]);
        if crate::ct::ct_eq(&h[..hl], &h2[..hl]) { Ok(()) } else { Err(Error::Auth) }
    }
}

/// An RSA private key in the CRT representation (n, e, d, p, q, dP, dQ, qInv). Zeroized on drop.
pub struct PrivateKey {
    public: PublicKey,
    d: Wide,
    p: Wide,
    q: Wide,
    dp: Wide,
    dq: Wide,
    qi: Wide,
}

/// The parts of a private key, in `RSAPrivateKey` order (RFC 8017 A.1.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    /// The modulus n.
    N,
    /// The public exponent e.
    E,
    /// The private exponent d.
    D,
    /// The first prime p.
    P,
    /// The second prime q.
    Q,
    /// d mod (p - 1).
    Dp,
    /// d mod (q - 1).
    Dq,
    /// q^-1 mod p.
    Qi,
}

impl PrivateKey {
    /// From big-endian components. Checks n = p q, qInv q ≡ 1 (mod p), dP = d mod (p - 1), dQ = d mod (q - 1),
    /// and one round trip (`(2^e)^d ≡ 2`); `Error::Param` when they do not describe one key.
    #[allow(clippy::too_many_arguments)]
    pub fn from_components(n: &[u8], e: &[u8], d: &[u8], p: &[u8], q: &[u8], dp: &[u8], dq: &[u8], qi: &[u8]) -> Result<Self, Error> {
        let public = PublicKey::new(n, e)?;
        let key = PrivateKey { public, d: from_be(d)?, p: from_be(p)?, q: from_be(q)?, dp: from_be(dp)?, dq: from_be(dq)?, qi: from_be(qi)? };
        let one = small(1);
        if key.p[0] & 1 == 0 || key.q[0] & 1 == 0 || cmp(&mul(&key.p, &key.q), &key.public.n) != core::cmp::Ordering::Equal {
            return Err(Error::Param);
        }
        if cmp(&key.qi, &key.p) != core::cmp::Ordering::Less || !is_one(&rem_ct(&mul(&key.qi, &key.q), &key.p)) {
            return Err(Error::Param);
        }
        if cmp(&rem_ct(&key.d, &sub(&key.p, &one)), &key.dp) != core::cmp::Ordering::Equal
            || cmp(&rem_ct(&key.d, &sub(&key.q, &one)), &key.dq) != core::cmp::Ordering::Equal
        {
            return Err(Error::Param);
        }
        let mut probe = [0u8; MAX_BYTES];
        let k = key.public.k;
        probe[k - 1] = 2;
        let mut c = [0u8; MAX_BYTES];
        key.public.public_raw(&probe[..k], &mut c)?;
        let mut back = [0u8; MAX_BYTES];
        key.decrypt_raw(&c[..k], &mut back)?;
        if back[..k] != probe[..k] {
            return Err(Error::Param);
        }
        Ok(key)
    }

    /// Key generation: a `bits`-bit modulus (`MIN_BITS..=MAX_BITS`) with public exponent `e` (big-endian,
    /// odd, 3 <= e < 2^64), primes of ceil(bits/2) and floor(bits/2) bits from `rng`, d = e^-1 mod (p-1)(q-1)
    /// (RFC 8017 §3.2 allows any d with e d ≡ 1 mod λ(n); λ(n) divides (p-1)(q-1)). `Error::Param` for an
    /// unsupported size or exponent. Not constant-time (see the module comment).
    pub fn generate(bits: usize, e: &[u8], rng: &mut dyn FnMut(&mut [u8])) -> Result<Self, Error> {
        if !(MIN_BITS..=MAX_BITS).contains(&bits) {
            return Err(Error::Param);
        }
        let ew = from_be(e)?;
        if nlimbs(&ew) > 1 || ew[0] < 3 || ew[0] & 1 == 0 {
            return Err(Error::Param);
        }
        let ev = ew[0];
        let pbits = bits.div_ceil(2);
        let qbits = bits - pbits;
        let one = small(1);
        loop {
            let p = gen_prime(pbits, ev, rng);
            let q = gen_prime(qbits, ev, rng);
            if cmp(&p, &q) == core::cmp::Ordering::Equal {
                continue;
            }
            let n = mul(&p, &q);
            if nbits(&n) != bits {
                continue;
            }
            let pm1 = sub(&p, &one);
            let qm1 = sub(&q, &one);
            let phi = mul(&pm1, &qm1);
            // d = (1 + k phi) / e with k = -phi^-1 mod e, so that e | 1 + k phi.
            let Some(inv) = inv_u64(divrem_small(&phi, ev).1, ev) else { continue };
            let k = (ev - inv) % ev;
            let (d, r) = divrem_small(&add(&mul_small(&phi, k), &one), ev);
            if r != 0 {
                continue;
            }
            let dp = rem_ct(&d, &pm1);
            let dq = rem_ct(&d, &qm1);
            let Some(mp) = Mont::new(&p) else { continue };
            let pm2 = sub(&p, &small(2));
            let qi = modpow(&q, &pm2, nbits(&p), &mp, &p);
            let mut nb = [0u8; MAX_BYTES];
            let k = bits.div_ceil(8);
            to_be(&n, &mut nb[..k]);
            let public = PublicKey::new(&nb[..k], e)?;
            return Ok(PrivateKey { public, d, p, q, dp, dq, qi });
        }
    }

    /// The public half.
    pub fn public_key(&self) -> &PublicKey {
        &self.public
    }

    /// One component, big-endian and minimal (at least one byte), into `out`; returns its length.
    pub fn part_be(&self, part: Part, out: &mut [u8]) -> usize {
        let v = match part {
            Part::N => &self.public.n,
            Part::E => &self.public.e,
            Part::D => &self.d,
            Part::P => &self.p,
            Part::Q => &self.q,
            Part::Dp => &self.dp,
            Part::Dq => &self.dq,
            Part::Qi => &self.qi,
        };
        let len = nbits(v).div_ceil(8).max(1);
        to_be(v, &mut out[..len]);
        len
    }

    /// RSADP / RSASP1 (§5.1.2 / §5.2.1, CRT): `c^d mod n` for a `k`-byte representative below n, into
    /// `out[..k]`, with the result checked against `m^e mod n` before it is released.
    pub fn decrypt_raw(&self, c: &[u8], out: &mut [u8]) -> Result<(), Error> {
        let k = self.public.k;
        if c.len() != k || out.len() < k {
            return Err(Error::Length);
        }
        let cv = from_be(c)?;
        if cmp(&cv, &self.public.n) != core::cmp::Ordering::Less {
            return Err(Error::Param);
        }
        let mp = Mont::new(&self.p).ok_or(Error::Param)?;
        let mq = Mont::new(&self.q).ok_or(Error::Param)?;
        let pbits = 64 * mp.l;
        let qbits = 64 * mq.l;
        let mut m1 = modpow(&cv, &self.dp, pbits, &mp, &self.p);
        let mut m2 = modpow(&cv, &self.dq, qbits, &mq, &self.q);
        // h = qInv (m1 - m2) mod p
        let m2p = rem_ct(&m2, &self.p);
        let mut diff = sub(&add(&m1, &self.p), &m2p);
        diff = rem_ct(&diff, &self.p);
        let hm = mp.mul(&mp.to(&diff), &mp.to(&self.qi));
        let h = mp.from(&hm);
        let mut m = add(&m2, &mul(&self.q, &h));
        to_be(&m, &mut out[..k]);
        let mut check = [0u8; MAX_BYTES];
        let r = self.public.public_raw(&out[..k], &mut check);
        let ok = r.is_ok() && crate::ct::ct_eq(&check[..k], c);
        m1.zeroize();
        m2.zeroize();
        diff.zeroize();
        m.zeroize();
        if !ok {
            out[..k].zeroize();
            return Err(Error::Param);
        }
        Ok(())
    }

    /// RSASSA-PKCS1-v1_5 signature (§8.2.1) over a message digest (SHA-1/224/256/384/512 by its length), k
    /// bytes into `out`.
    pub fn sign_pkcs1v15(&self, digest: &[u8], out: &mut [u8]) -> Result<usize, Error> {
        let k = self.public.k;
        let mut em = [0u8; MAX_BYTES];
        pkcs1v15_encode(digest, &mut em[..k])?;
        self.decrypt_raw(&em[..k], out)?;
        Ok(k)
    }

    /// RSASSA-PSS signature (§8.1.1, EMSA-PSS-ENCODE §9.1.1) with MGF1 over `D` and the caller-drawn `salt`, k
    /// bytes into `out`; `Error::Length` when `emLen < hLen + sLen + 2`.
    pub fn sign_pss<D: Digest>(&self, digest: &[u8], salt: &[u8], out: &mut [u8]) -> Result<usize, Error> {
        let hl = D::OUTPUT_LEN;
        let k = self.public.k;
        let em_bits = self.public.bits - 1;
        let em_len = em_bits.div_ceil(8);
        if digest.len() != hl || em_len < hl + salt.len() + 2 {
            return Err(Error::Length);
        }
        let mut h = [0u8; 64];
        hash_into::<D>(&[&[0u8; 8], digest, salt], &mut h[..hl]);
        let mut buf = [0u8; MAX_BYTES];
        let off = k - em_len;
        {
            let em = &mut buf[off..k];
            let db_len = em_len - hl - 1;
            let ps_len = db_len - salt.len() - 1;
            em[ps_len] = 0x01;
            em[ps_len + 1..db_len].copy_from_slice(salt);
            mgf1_xor::<D>(&h[..hl], &mut em[..db_len]);
            em[0] &= 0xffu8 >> (8 * em_len - em_bits);
            em[db_len..db_len + hl].copy_from_slice(&h[..hl]);
            em[em_len - 1] = 0xbc;
        }
        self.decrypt_raw(&buf[..k], out)?;
        Ok(k)
    }

    /// RSAES-OAEP-DECRYPT (§7.1.2) with MGF1 over `D` and the given label: the message into `out`, its length
    /// returned. Every decoding failure is the one `Error::Auth` (the RFC's "decryption error").
    pub fn decrypt_oaep<D: Digest>(&self, label: &[u8], c: &[u8], out: &mut [u8]) -> Result<usize, Error> {
        let k = self.public.k;
        let hl = D::OUTPUT_LEN;
        if c.len() != k || k < 2 * hl + 2 {
            return Err(Error::Auth);
        }
        let mut em = [0u8; MAX_BYTES];
        self.decrypt_raw(c, &mut em).map_err(|_| Error::Auth)?;
        let y = em[0];
        let (mseed, db) = em[1..k].split_at_mut(hl);
        mgf1_xor::<D>(db, mseed);
        mgf1_xor::<D>(mseed, db);
        let mut lh = [0u8; 64];
        hash_into::<D>(&[label], &mut lh[..hl]);
        let mut good = crate::ct::ct_eq(&db[..hl], &lh[..hl]) as u8 & ((y == 0) as u8);
        // the first non-zero byte after lHash must be 0x01
        let mut idx = 0usize;
        let mut found = 0u8;
        for (i, &b) in db.iter().enumerate().skip(hl) {
            let is_one = (b == 1) as u8;
            let is_zero = (b == 0) as u8;
            let first = is_one & (found ^ 1);
            idx |= i & (first as usize).wrapping_neg();
            good &= found | is_one | is_zero;
            found |= is_one;
        }
        good &= found;
        let mlen = db.len() - idx - 1;
        if good == 0 || out.len() < mlen {
            em.zeroize();
            return Err(Error::Auth);
        }
        out[..mlen].copy_from_slice(&db[idx + 1..]);
        em.zeroize();
        Ok(mlen)
    }
}

impl Drop for PrivateKey {
    fn drop(&mut self) {
        self.d.zeroize();
        self.p.zeroize();
        self.q.zeroize();
        self.dp.zeroize();
        self.dq.zeroize();
        self.qi.zeroize();
    }
}

// ---- encodings -------------------------------------------------------------------------------------------------

fn hash_into<D: Digest>(parts: &[&[u8]], out: &mut [u8]) {
    let mut h = D::new();
    for p in parts {
        h.update(p);
    }
    let mut full = [0u8; 64];
    h.finalize_into(&mut full);
    out.copy_from_slice(&full[..out.len()]);
}

/// MGF1 (§B.2.1) over `D`, XORed into `out`.
fn mgf1_xor<D: Digest>(seed: &[u8], out: &mut [u8]) {
    let hl = D::OUTPUT_LEN;
    let mut d = [0u8; 64];
    for (counter, chunk) in out.chunks_mut(hl).enumerate() {
        let mut h = D::new();
        h.update(seed);
        h.update(&(counter as u32).to_be_bytes());
        h.finalize_into(&mut d);
        for (o, m) in chunk.iter_mut().zip(d.iter()) {
            *o ^= m;
        }
    }
}

/// EMSA-PKCS1-v1_5 (§9.2) of a digest into `em` (k bytes): `00 01 FF..FF 00 || DigestInfo || H`, the
/// DigestInfo prefix (note 1) chosen by the digest's length.
fn pkcs1v15_encode(digest: &[u8], em: &mut [u8]) -> Result<(), Error> {
    let prefix: &[u8] = match digest.len() {
        20 => &[0x30, 0x21, 0x30, 0x09, 0x06, 0x05, 0x2b, 0x0e, 0x03, 0x02, 0x1a, 0x05, 0x00, 0x04, 0x14],
        28 => &[0x30, 0x2d, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x04, 0x05, 0x00, 0x04, 0x1c],
        32 => &[0x30, 0x31, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x01, 0x05, 0x00, 0x04, 0x20],
        48 => &[0x30, 0x41, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x02, 0x05, 0x00, 0x04, 0x30],
        64 => &[0x30, 0x51, 0x30, 0x0d, 0x06, 0x09, 0x60, 0x86, 0x48, 0x01, 0x65, 0x03, 0x04, 0x02, 0x03, 0x05, 0x00, 0x04, 0x40],
        _ => return Err(Error::Param),
    };
    let k = em.len();
    let t_len = prefix.len() + digest.len();
    if k < t_len + 11 {
        return Err(Error::Length);
    }
    em[0] = 0x00;
    em[1] = 0x01;
    for b in &mut em[2..k - t_len - 1] {
        *b = 0xff;
    }
    em[k - t_len - 1] = 0x00;
    em[k - t_len..k - digest.len()].copy_from_slice(prefix);
    em[k - digest.len()..].copy_from_slice(digest);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sha2::Sha256;

    /// A deterministic byte stream for the tests (not a CSPRNG).
    fn stream(seed: u64) -> impl FnMut(&mut [u8]) {
        let mut s = seed;
        move |buf: &mut [u8]| {
            for b in buf.iter_mut() {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                *b = s as u8;
            }
        }
    }

    /// Generate a 512-bit key, re-import it from its components, then sign / verify (PKCS #1 v1.5 and PSS)
    /// and encrypt / decrypt (OAEP) across the two halves; a flipped bit fails each check.
    #[test]
    fn generate_sign_verify_encrypt_decrypt() {
        let mut rng = stream(0x5eed);
        let key = PrivateKey::generate(512, &[1, 0, 1], &mut rng).unwrap();
        assert_eq!(key.public_key().bits(), 512);
        let mut parts = [[0u8; 128]; 8];
        let mut lens = [0usize; 8];
        for (i, part) in [Part::N, Part::E, Part::D, Part::P, Part::Q, Part::Dp, Part::Dq, Part::Qi].into_iter().enumerate() {
            lens[i] = key.part_be(part, &mut parts[i]);
        }
        let again = PrivateKey::from_components(
            &parts[0][..lens[0]], &parts[1][..lens[1]], &parts[2][..lens[2]], &parts[3][..lens[3]],
            &parts[4][..lens[4]], &parts[5][..lens[5]], &parts[6][..lens[6]], &parts[7][..lens[7]],
        )
        .unwrap();
        let digest = crate::sha2::sha256(b"abc");
        let mut sig = [0u8; 64];
        again.sign_pkcs1v15(&digest, &mut sig).unwrap();
        key.public_key().verify_pkcs1v15(&digest, &sig).unwrap();
        let mut sig2 = [0u8; 64];
        key.sign_pss::<Sha256>(&digest, &[7u8; 10], &mut sig2).unwrap();
        key.public_key().verify_pss::<Sha256>(&digest, 10, &sig2).unwrap();
        sig2[5] ^= 1;
        assert!(key.public_key().verify_pss::<Sha256>(&digest, 10, &sig2).is_err());
        let mut ct = [0u8; 64];
        key.public_key().encrypt_oaep::<Sha256>(b"L", b"hello", &[9u8; 32], &mut ct).unwrap_err(); // 64 < 2*32+2+5
        let big = PrivateKey::generate(1024, &[1, 0, 1], &mut rng).unwrap();
        let mut ct = [0u8; 128];
        big.public_key().encrypt_oaep::<Sha256>(b"L", b"hello", &[9u8; 32], &mut ct).unwrap();
        let mut pt = [0u8; 128];
        let n = big.decrypt_oaep::<Sha256>(b"L", &ct, &mut pt).unwrap();
        assert_eq!(&pt[..n], b"hello");
        assert!(big.decrypt_oaep::<Sha256>(b"M", &ct, &mut pt).is_err());
    }
}
