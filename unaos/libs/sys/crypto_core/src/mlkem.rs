// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! ML-KEM (FIPS 203, August 2024) — the Module-Lattice Key-Encapsulation Mechanism (CTCORE, SR60), written
//! from the standard's algorithms, for all three parameter sets; [`mlkem768`] is the one TLS uses
//! (X25519MLKEM768, draft-ietf-tls-ecdhe-mlkem).
//!
//! | FIPS 203 | here |
//! |---|---|
//! | §4.1 H, J, G, PRF, XOF (SHA3-256, SHAKE256, SHA3-512, SHAKE256, SHAKE128) | [`crate::sha3`] |
//! | Alg 3/4 BitsToBytes / BytesToBits, Alg 5/6 ByteEncode_d / ByteDecode_d | [`byte_encode`], [`byte_decode`] |
//! | §4.2.1 Compress_d / Decompress_d | [`compress`], [`decompress`] |
//! | Alg 7 SampleNTT (rejection sampling on 12-bit candidates from SHAKE128(ρ‖j‖i)) | [`sample_ntt`] |
//! | Alg 8 SamplePolyCBD_η | [`sample_cbd`] |
//! | Alg 9/10 NTT / NTT⁻¹ (ζ = 17, bit-reversed powers), Alg 11/12 MultiplyNTTs / BaseCaseMultiply | [`ntt`], [`ntt_inv`], [`multiply_ntts`] |
//! | Alg 13/14/15 K-PKE.KeyGen / Encrypt / Decrypt | [`kpke_keygen`], [`kpke_encrypt`], [`kpke_decrypt`] |
//! | Alg 16/17/18 ML-KEM.KeyGen_internal / Encaps_internal / Decaps_internal | [`keygen_internal`], [`encaps_internal`], [`decaps_internal`] |
//! | Alg 19/20/21 ML-KEM.KeyGen / Encaps / Decaps, §7.2 / §7.3 input checks | [`keygen`], [`encaps`], [`decaps`] |
//!
//! # Constant-time
//!
//! FIPS 203 §3.3 asks that secret-dependent work not leak through timing. Here:
//! * every arithmetic step on secret data (CBD sampling, NTT butterflies, base-case products, compression of `u`,
//!   `v`, `w`, encoding of m′) is branch-free and table-free — reduction mod q is a Barrett multiply and a masked
//!   conditional subtraction; Compress_d divides by q with a multiply-by-reciprocal and a masked correction (no
//!   `/` on secrets: the "KyberSlash" timing leak was exactly a hardware division by q);
//! * Decaps' re-encryption check compares the whole ciphertext with [`crate::ct::ct_eq`] and selects K′ or K̄ with
//!   a mask ([`crate::ct::ct_select`] style), so the implicit-rejection path takes the same time;
//! * NOT constant-time, by design: SampleNTT's rejection loop (its input ρ is public — part of `ek`), the input
//!   checks (`ek`, `c`, `dk` lengths, the modulus check on the public `ek`, the hash check — whose failure is
//!   public), and lengths everywhere.
//! * Multiplications are `u32 × u32 → u64`, constant-time on x86_64 and AArch64 (the crate-wide caveat).
//!
//! Secrets (ŝ, ê, ŷ, e₁, e₂, m′, the PRF outputs) are wiped before return, best effort (`black_box`).

use crate::ct;
use crate::sha3::{sha3_256, sha3_512, Shake128, Shake256};
use crate::Error;

/// n.
pub const N: usize = 256;
/// q.
pub const Q: u32 = 3329;

/// A parameter set (FIPS 203 §8, Table 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Params {
    /// Module rank.
    pub k: usize,
    /// CBD parameter for s, e, y.
    pub eta1: usize,
    /// CBD parameter for e1, e2.
    pub eta2: usize,
    /// Compression bits for u.
    pub du: u32,
    /// Compression bits for v.
    pub dv: u32,
}

impl Params {
    /// |ek| = 384k + 32.
    pub const fn ek_len(&self) -> usize {
        384 * self.k + 32
    }
    /// |dk| = 768k + 96.
    pub const fn dk_len(&self) -> usize {
        768 * self.k + 96
    }
    /// |c| = 32(du·k + dv).
    pub const fn ct_len(&self) -> usize {
        32 * (self.du as usize * self.k + self.dv as usize)
    }
}

/// ML-KEM-512 (security category 1).
pub const ML_KEM_512: Params = Params { k: 2, eta1: 3, eta2: 2, du: 10, dv: 4 };
/// ML-KEM-768 (category 3).
pub const ML_KEM_768: Params = Params { k: 3, eta1: 2, eta2: 2, du: 10, dv: 4 };
/// ML-KEM-1024 (category 5).
pub const ML_KEM_1024: Params = Params { k: 4, eta1: 2, eta2: 2, du: 11, dv: 5 };

/// A polynomial (coefficients in [0, q)).
pub type Poly = [u16; N];
const MAXK: usize = 4;

// ---------------------------------------------------------------------------------------------- arithmetic

/// x − q if x ≥ q, for x < 2q — masked, no branch.
#[inline(always)]
fn csub(x: u32) -> u32 {
    let t = x.wrapping_sub(Q);
    t.wrapping_add(Q & 0u32.wrapping_sub(t >> 31))
}

/// x mod q for x < 2^26 (Barrett: ⌊2^32 / q⌋ = 1290167), masked final subtraction.
#[inline(always)]
fn reduce(x: u32) -> u32 {
    let qt = ((x as u64 * 1_290_167) >> 32) as u32;
    csub(x - qt * Q)
}

#[inline(always)]
fn mulq(a: u32, b: u32) -> u32 {
    reduce(a * b)
}

/// ⌊n / q⌋ for n < 2^26, constant-time (multiply by the reciprocal, one masked correction).
#[inline(always)]
fn div_q(n: u32) -> u32 {
    let mut qt = ((n as u64 * 1_290_167) >> 32) as u32;
    let r = n - qt * Q;
    // r ∈ [0, 2q): add one when r ≥ q.
    qt += 1 & !((r.wrapping_sub(Q)) >> 31);
    qt
}

const fn pow_mod(mut b: u32, mut e: u32) -> u32 {
    let mut r = 1u32;
    while e > 0 {
        if e & 1 == 1 {
            r = r * b % Q;
        }
        b = b * b % Q;
        e >>= 1;
    }
    r
}

const fn bitrev7(x: u32) -> u32 {
    let mut r = 0;
    let mut i = 0;
    while i < 7 {
        r |= ((x >> i) & 1) << (6 - i);
        i += 1;
    }
    r
}

/// ζ^BitRev7(i) mod q (FIPS 203 Appendix A).
const ZETAS: [u32; 128] = {
    let mut z = [0u32; 128];
    let mut i = 0;
    while i < 128 {
        z[i] = pow_mod(17, bitrev7(i as u32));
        i += 1;
    }
    z
};

/// ζ^(2·BitRev7(i)+1) mod q (Appendix A, MultiplyNTTs).
const GAMMAS: [u32; 128] = {
    let mut z = [0u32; 128];
    let mut i = 0;
    while i < 128 {
        z[i] = pow_mod(17, 2 * bitrev7(i as u32) + 1);
        i += 1;
    }
    z
};

/// 128⁻¹ mod q.
const INV128: u32 = 3303;

/// Algorithm 9: NTT, in place.
pub fn ntt(f: &mut Poly) {
    let mut i = 1;
    let mut len = 128;
    while len >= 2 {
        let mut start = 0;
        while start < N {
            let zeta = ZETAS[i];
            i += 1;
            for j in start..start + len {
                let t = mulq(zeta, f[j + len] as u32);
                f[j + len] = csub(f[j] as u32 + Q - t) as u16;
                f[j] = csub(f[j] as u32 + t) as u16;
            }
            start += 2 * len;
        }
        len /= 2;
    }
}

/// Algorithm 10: NTT⁻¹, in place.
pub fn ntt_inv(f: &mut Poly) {
    let mut i = 127;
    let mut len = 2;
    while len <= 128 {
        let mut start = 0;
        while start < N {
            let zeta = ZETAS[i];
            i -= 1;
            for j in start..start + len {
                let t = f[j] as u32;
                f[j] = csub(t + f[j + len] as u32) as u16;
                f[j + len] = mulq(zeta, csub(f[j + len] as u32 + Q - t)) as u16;
            }
            start += 2 * len;
        }
        len *= 2;
    }
    for c in f.iter_mut() {
        *c = mulq(*c as u32, INV128) as u16;
    }
}

/// Algorithms 11/12: ĥ = f̂ ∘ ĝ, accumulated into `h` (h += f̂ ∘ ĝ) — the matrix-vector products sum these.
pub fn multiply_ntts_acc(h: &mut Poly, f: &Poly, g: &Poly) {
    for i in 0..128 {
        let (a0, a1, b0, b1) = (f[2 * i] as u32, f[2 * i + 1] as u32, g[2 * i] as u32, g[2 * i + 1] as u32);
        let c0 = reduce(mulq(a0, b0) + mulq(mulq(a1, b1), GAMMAS[i]));
        let c1 = reduce(mulq(a0, b1) + mulq(a1, b0));
        h[2 * i] = csub(h[2 * i] as u32 + c0) as u16;
        h[2 * i + 1] = csub(h[2 * i + 1] as u32 + c1) as u16;
    }
}

/// Algorithm 11: MultiplyNTTs.
pub fn multiply_ntts(f: &Poly, g: &Poly) -> Poly {
    let mut h = [0u16; N];
    multiply_ntts_acc(&mut h, f, g);
    h
}

fn poly_add(a: &mut Poly, b: &Poly) {
    for i in 0..N {
        a[i] = csub(a[i] as u32 + b[i] as u32) as u16;
    }
}

fn poly_sub(a: &mut Poly, b: &Poly) {
    for i in 0..N {
        a[i] = csub(a[i] as u32 + Q - b[i] as u32) as u16;
    }
}

// ---------------------------------------------------------------------------------------------- encodings

/// §4.2.1 Compress_d(x) = ⌈(2^d / q)·x⌋ mod 2^d — constant-time.
#[inline(always)]
pub fn compress(x: u16, d: u32) -> u16 {
    ((div_q(((x as u32) << d) + Q / 2)) & ((1 << d) - 1)) as u16
}

/// §4.2.1 Decompress_d(y) = ⌈(q / 2^d)·y⌋.
#[inline(always)]
pub fn decompress(y: u16, d: u32) -> u16 {
    ((y as u32 * Q + (1 << (d - 1))) >> d) as u16
}

/// Algorithm 5: ByteEncode_d (bits little-endian, coefficient i's bit j at position i·d + j). `out` = 32·d bytes.
pub fn byte_encode(f: &Poly, d: u32, out: &mut [u8]) {
    let d = d as usize;
    debug_assert_eq!(out.len(), 32 * d);
    out.fill(0);
    let mut bit = 0usize;
    for &c in f.iter() {
        let c = c as u32;
        for j in 0..d {
            out[bit / 8] |= (((c >> j) & 1) as u8) << (bit % 8);
            bit += 1;
        }
    }
}

/// Algorithm 6: ByteDecode_d (d = 12 reduces mod q, as the standard says; d < 12 mod 2^d).
pub fn byte_decode(b: &[u8], d: u32) -> Poly {
    let d = d as usize;
    debug_assert_eq!(b.len(), 32 * d);
    let mut f = [0u16; N];
    let mut bit = 0usize;
    for c in f.iter_mut() {
        let mut v = 0u32;
        for j in 0..d {
            v |= (((b[bit / 8] >> (bit % 8)) & 1) as u32) << j;
            bit += 1;
        }
        *c = if d == 12 { reduce(v) } else { v } as u16;
    }
    f
}

// ---------------------------------------------------------------------------------------------- sampling

/// Algorithm 7: SampleNTT(ρ‖j‖i) — rejection sampling from SHAKE128 (variable time; ρ is public).
pub fn sample_ntt(rho: &[u8; 32], j: u8, i: u8) -> Poly {
    let mut x = Shake128::new();
    x.update(rho);
    x.update(&[j, i]);
    let mut a = [0u16; N];
    let mut n = 0;
    let mut buf = [0u8; 168];
    let mut pos = buf.len();
    while n < N {
        if pos + 3 > buf.len() {
            x.squeeze(&mut buf);
            pos = 0;
        }
        let (c0, c1, c2) = (buf[pos] as u32, buf[pos + 1] as u32, buf[pos + 2] as u32);
        pos += 3;
        let d1 = c0 + 256 * (c1 & 15);
        let d2 = (c1 >> 4) + 16 * c2;
        if d1 < Q {
            a[n] = d1 as u16;
            n += 1;
        }
        if d2 < Q && n < N {
            a[n] = d2 as u16;
            n += 1;
        }
    }
    a
}

/// Algorithm 8: SamplePolyCBD_η(B), |B| = 64η — branch-free bit sums.
pub fn sample_cbd(b: &[u8], eta: usize) -> Poly {
    debug_assert_eq!(b.len(), 64 * eta);
    let bit = |k: usize| ((b[k / 8] >> (k % 8)) & 1) as u32;
    let mut f = [0u16; N];
    for (i, c) in f.iter_mut().enumerate() {
        let mut x = 0;
        let mut y = 0;
        for j in 0..eta {
            x += bit(2 * i * eta + j);
            y += bit(2 * i * eta + eta + j);
        }
        *c = csub(x + Q - y) as u16;
    }
    f
}

/// PRF_η(s, b) = SHAKE256(s‖b, 64η) into `out`.
fn prf(s: &[u8; 32], b: u8, out: &mut [u8]) {
    let mut h = Shake256::new();
    h.update(s);
    h.update(&[b]);
    h.squeeze(out);
}

fn cbd_from_prf(s: &[u8; 32], nonce: u8, eta: usize) -> Poly {
    let mut buf = [0u8; 64 * 3];
    let b = &mut buf[..64 * eta];
    prf(s, nonce, b);
    let p = sample_cbd(b, eta);
    wipe(&mut buf);
    p
}

fn wipe(b: &mut [u8]) {
    b.fill(0);
    core::hint::black_box(&*b);
}

fn wipe_polys(v: &mut [Poly]) {
    for p in v.iter_mut() {
        p.fill(0);
    }
    core::hint::black_box(&*v);
}

// ---------------------------------------------------------------------------------------------- K-PKE

fn matrix(rho: &[u8; 32], k: usize) -> [[Poly; MAXK]; MAXK] {
    let mut a = [[[0u16; N]; MAXK]; MAXK];
    for i in 0..k {
        for j in 0..k {
            a[i][j] = sample_ntt(rho, j as u8, i as u8);
        }
    }
    a
}

/// Algorithm 13: K-PKE.KeyGen(d) → (ek_PKE, dk_PKE) written into `ek` (384k + 32) and `dk` (384k).
pub fn kpke_keygen(p: &Params, d: &[u8; 32], ek: &mut [u8], dk: &mut [u8]) {
    let k = p.k;
    let mut seed = [0u8; 33];
    seed[..32].copy_from_slice(d);
    seed[32] = k as u8;
    let g = sha3_512(&seed);
    wipe(&mut seed);
    let mut rho = [0u8; 32];
    let mut sigma = [0u8; 32];
    rho.copy_from_slice(&g[..32]);
    sigma.copy_from_slice(&g[32..]);
    let a = matrix(&rho, k);
    let mut s = [[0u16; N]; MAXK];
    let mut e = [[0u16; N]; MAXK];
    let mut nonce = 0u8;
    for si in s.iter_mut().take(k) {
        *si = cbd_from_prf(&sigma, nonce, p.eta1);
        nonce += 1;
    }
    for ei in e.iter_mut().take(k) {
        *ei = cbd_from_prf(&sigma, nonce, p.eta1);
        nonce += 1;
    }
    for i in 0..k {
        ntt(&mut s[i]);
        ntt(&mut e[i]);
    }
    for i in 0..k {
        let mut t = e[i];
        for j in 0..k {
            multiply_ntts_acc(&mut t, &a[i][j], &s[j]);
        }
        byte_encode(&t, 12, &mut ek[384 * i..384 * (i + 1)]);
        byte_encode(&s[i], 12, &mut dk[384 * i..384 * (i + 1)]);
    }
    ek[384 * k..384 * k + 32].copy_from_slice(&rho);
    wipe_polys(&mut s);
    wipe_polys(&mut e);
    wipe(&mut sigma);
}

/// Algorithm 14: K-PKE.Encrypt(ek_PKE, m, r) → c (written into `c`, |c| = 32(du·k + dv)).
pub fn kpke_encrypt(p: &Params, ek: &[u8], m: &[u8; 32], r: &[u8; 32], c: &mut [u8]) {
    let k = p.k;
    let mut t = [[0u16; N]; MAXK];
    for (i, ti) in t.iter_mut().enumerate().take(k) {
        *ti = byte_decode(&ek[384 * i..384 * (i + 1)], 12);
    }
    let mut rho = [0u8; 32];
    rho.copy_from_slice(&ek[384 * k..384 * k + 32]);
    let a = matrix(&rho, k);
    let mut y = [[0u16; N]; MAXK];
    let mut e1 = [[0u16; N]; MAXK];
    let mut nonce = 0u8;
    for yi in y.iter_mut().take(k) {
        *yi = cbd_from_prf(r, nonce, p.eta1);
        nonce += 1;
    }
    for ei in e1.iter_mut().take(k) {
        *ei = cbd_from_prf(r, nonce, p.eta2);
        nonce += 1;
    }
    let mut e2 = [cbd_from_prf(r, nonce, p.eta2)];
    for yi in y.iter_mut().take(k) {
        ntt(yi);
    }
    let du = p.du as usize;
    for i in 0..k {
        // u[i] = NTT⁻¹(Σ_j Â[j][i] ∘ ŷ[j]) + e1[i]   (Âᵀ)
        let mut u = [0u16; N];
        for j in 0..k {
            multiply_ntts_acc(&mut u, &a[j][i], &y[j]);
        }
        ntt_inv(&mut u);
        poly_add(&mut u, &e1[i]);
        for x in u.iter_mut() {
            *x = compress(*x, p.du);
        }
        byte_encode(&u, p.du, &mut c[32 * du * i..32 * du * (i + 1)]);
    }
    let mut v = [0u16; N];
    for i in 0..k {
        multiply_ntts_acc(&mut v, &t[i], &y[i]);
    }
    ntt_inv(&mut v);
    poly_add(&mut v, &e2[0]);
    let mut mu = byte_decode(m, 1);
    for x in mu.iter_mut() {
        *x = decompress(*x, 1);
    }
    poly_add(&mut v, &mu);
    for x in v.iter_mut() {
        *x = compress(*x, p.dv);
    }
    byte_encode(&v, p.dv, &mut c[32 * du * k..32 * du * k + 32 * p.dv as usize]);
    wipe_polys(&mut y);
    wipe_polys(&mut e1);
    wipe_polys(&mut e2);
    mu.fill(0);
    v.fill(0);
    core::hint::black_box((&mu, &v));
}

/// Algorithm 15: K-PKE.Decrypt(dk_PKE, c) → m.
pub fn kpke_decrypt(p: &Params, dk: &[u8], c: &[u8]) -> [u8; 32] {
    let k = p.k;
    let du = p.du as usize;
    let mut acc = [0u16; N];
    for i in 0..k {
        let mut u = byte_decode(&c[32 * du * i..32 * du * (i + 1)], p.du);
        for x in u.iter_mut() {
            *x = decompress(*x, p.du);
        }
        ntt(&mut u);
        let mut s = byte_decode(&dk[384 * i..384 * (i + 1)], 12);
        multiply_ntts_acc(&mut acc, &s, &u);
        s.fill(0);
        core::hint::black_box(&s);
    }
    ntt_inv(&mut acc);
    let mut w = byte_decode(&c[32 * du * k..32 * du * k + 32 * p.dv as usize], p.dv);
    for x in w.iter_mut() {
        *x = decompress(*x, p.dv);
    }
    poly_sub(&mut w, &acc);
    for x in w.iter_mut() {
        *x = compress(*x, 1);
    }
    let mut m = [0u8; 32];
    byte_encode(&w, 1, &mut m);
    w.fill(0);
    acc.fill(0);
    core::hint::black_box((&w, &acc));
    m
}

// ---------------------------------------------------------------------------------------------- ML-KEM

/// Algorithm 16: ML-KEM.KeyGen_internal(d, z) → (ek, dk) into `ek` (384k+32) and `dk` (768k+96).
pub fn keygen_internal(p: &Params, d: &[u8; 32], z: &[u8; 32], ek: &mut [u8], dk: &mut [u8]) {
    let k = p.k;
    assert!(ek.len() == p.ek_len() && dk.len() == p.dk_len(), "mlkem: buffer sizes");
    kpke_keygen(p, d, ek, &mut dk[..384 * k]);
    dk[384 * k..768 * k + 32].copy_from_slice(ek);
    dk[768 * k + 32..768 * k + 64].copy_from_slice(&sha3_256(ek));
    dk[768 * k + 64..].copy_from_slice(z);
}

/// Algorithm 17: ML-KEM.Encaps_internal(ek, m) → (K, c). No input check (see [`encaps`]).
pub fn encaps_internal(p: &Params, ek: &[u8], m: &[u8; 32], c: &mut [u8]) -> [u8; 32] {
    let mut g_in = [0u8; 64];
    g_in[..32].copy_from_slice(m);
    g_in[32..].copy_from_slice(&sha3_256(ek));
    let mut g = sha3_512(&g_in);
    let mut key = [0u8; 32];
    let mut r = [0u8; 32];
    key.copy_from_slice(&g[..32]);
    r.copy_from_slice(&g[32..]);
    kpke_encrypt(p, ek, m, &r, c);
    wipe(&mut g);
    wipe(&mut g_in);
    wipe(&mut r);
    key
}

/// The largest ciphertext (ML-KEM-1024).
const MAX_CT: usize = 1568;

/// Algorithm 18: ML-KEM.Decaps_internal(dk, c) → K, with implicit rejection (constant-time select).
pub fn decaps_internal(p: &Params, dk: &[u8], c: &[u8]) -> [u8; 32] {
    let k = p.k;
    let dk_pke = &dk[..384 * k];
    let ek_pke = &dk[384 * k..768 * k + 32];
    let h = &dk[768 * k + 32..768 * k + 64];
    let z = &dk[768 * k + 64..768 * k + 96];
    let mut m2 = kpke_decrypt(p, dk_pke, c);
    let mut g_in = [0u8; 64];
    g_in[..32].copy_from_slice(&m2);
    g_in[32..].copy_from_slice(h);
    let mut g = sha3_512(&g_in);
    let mut k1 = [0u8; 32];
    let mut r2 = [0u8; 32];
    k1.copy_from_slice(&g[..32]);
    r2.copy_from_slice(&g[32..]);
    // K̄ = J(z‖c)
    let mut kbar = [0u8; 32];
    let mut j = Shake256::new();
    j.update(z);
    j.update(c);
    j.squeeze(&mut kbar);
    let mut c2 = [0u8; MAX_CT];
    kpke_encrypt(p, ek_pke, &m2, &r2, &mut c2[..c.len()]);
    let same = ct::ct_eq(c, &c2[..c.len()]);
    // K' if c = c', else K̄ — a mask, no branch.
    let mask = 0u8.wrapping_sub(same as u8);
    let mut out = [0u8; 32];
    for i in 0..32 {
        out[i] = (k1[i] & mask) | (kbar[i] & !mask);
    }
    wipe(&mut m2);
    wipe(&mut g);
    wipe(&mut g_in);
    wipe(&mut k1);
    wipe(&mut r2);
    wipe(&mut kbar);
    wipe(&mut c2);
    out
}

/// §7.2 encapsulation key check: length and modulus (ByteEncode12(ByteDecode12(t̂)) = t̂).
pub fn check_ek(p: &Params, ek: &[u8]) -> bool {
    if ek.len() != p.ek_len() {
        return false;
    }
    let mut re = [0u8; 384];
    for i in 0..p.k {
        let chunk = &ek[384 * i..384 * (i + 1)];
        byte_encode(&byte_decode(chunk, 12), 12, &mut re);
        if re[..] != chunk[..] {
            return false;
        }
    }
    true
}

/// §7.3 decapsulation key check: length and the hash check H(ek) = h.
pub fn check_dk(p: &Params, dk: &[u8]) -> bool {
    let k = p.k;
    dk.len() == p.dk_len() && sha3_256(&dk[384 * k..768 * k + 32])[..] == dk[768 * k + 32..768 * k + 64]
}

/// Algorithm 19: ML-KEM.KeyGen with d, z from `rng` (a DRBG or the entropy source).
pub fn keygen(p: &Params, rng: &mut dyn FnMut(&mut [u8]), ek: &mut [u8], dk: &mut [u8]) {
    let mut d = [0u8; 32];
    let mut z = [0u8; 32];
    rng(&mut d);
    rng(&mut z);
    keygen_internal(p, &d, &z, ek, dk);
    wipe(&mut d);
    wipe(&mut z);
}

/// Algorithm 20: ML-KEM.Encaps with the §7.2 input check; m from `rng`.
pub fn encaps(p: &Params, ek: &[u8], rng: &mut dyn FnMut(&mut [u8]), c: &mut [u8]) -> Result<[u8; 32], Error> {
    if c.len() != p.ct_len() {
        return Err(Error::Length);
    }
    if !check_ek(p, ek) {
        return Err(Error::Encoding);
    }
    let mut m = [0u8; 32];
    rng(&mut m);
    let key = encaps_internal(p, ek, &m, c);
    wipe(&mut m);
    Ok(key)
}

/// Algorithm 21: ML-KEM.Decaps with the §7.3 input checks.
pub fn decaps(p: &Params, dk: &[u8], c: &[u8]) -> Result<[u8; 32], Error> {
    if c.len() != p.ct_len() {
        return Err(Error::Length);
    }
    if !check_dk(p, dk) {
        return Err(Error::Encoding);
    }
    Ok(decaps_internal(p, dk, c))
}

/// ML-KEM-768 with fixed-size buffers (what X25519MLKEM768 uses).
pub mod mlkem768 {
    use super::*;
    /// |ek|.
    pub const EK_LEN: usize = 1184;
    /// |dk|.
    pub const DK_LEN: usize = 2400;
    /// |c|.
    pub const CT_LEN: usize = 1088;
    const P: Params = ML_KEM_768;
    const _: () = assert!(P.ek_len() == EK_LEN && P.dk_len() == DK_LEN && P.ct_len() == CT_LEN);

    /// Algorithm 16 for ML-KEM-768.
    pub fn keygen_internal(d: &[u8; 32], z: &[u8; 32]) -> ([u8; EK_LEN], [u8; DK_LEN]) {
        let mut ek = [0u8; EK_LEN];
        let mut dk = [0u8; DK_LEN];
        super::keygen_internal(&P, d, z, &mut ek, &mut dk);
        (ek, dk)
    }
    /// Algorithm 19 for ML-KEM-768.
    pub fn keygen(rng: &mut dyn FnMut(&mut [u8])) -> ([u8; EK_LEN], [u8; DK_LEN]) {
        let mut ek = [0u8; EK_LEN];
        let mut dk = [0u8; DK_LEN];
        super::keygen(&P, rng, &mut ek, &mut dk);
        (ek, dk)
    }
    /// Algorithm 20 for ML-KEM-768: (shared key, ciphertext).
    pub fn encaps(ek: &[u8], rng: &mut dyn FnMut(&mut [u8])) -> Result<([u8; 32], [u8; CT_LEN]), Error> {
        let mut c = [0u8; CT_LEN];
        let k = super::encaps(&P, ek, rng, &mut c)?;
        Ok((k, c))
    }
    /// Algorithm 21 for ML-KEM-768.
    pub fn decaps(dk: &[u8], c: &[u8]) -> Result<[u8; 32], Error> {
        super::decaps(&P, dk, c)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ntt_roundtrip_and_tables() {
        assert_eq!(ZETAS[1], 1729); // ζ^64 (FIPS 203 Appendix A: 1, 1729, 2580, 3289, ...)
        assert_eq!(&ZETAS[..4], &[1, 1729, 2580, 3289]);
        assert_eq!(&GAMMAS[..4], &[17, 3312, 2761, 568]);
        let mut f = [0u16; N];
        for (i, c) in f.iter_mut().enumerate() {
            *c = ((i * 7919) % Q as usize) as u16;
        }
        let g = f;
        ntt(&mut f);
        ntt_inv(&mut f);
        assert_eq!(f, g);
        for x in 0..Q as u16 {
            for d in [1, 4, 5, 10, 11] {
                // Compress against the exact rational rounding.
                let exact = (((x as u64) << d) * 2 + Q as u64) / (2 * Q as u64) % (1 << d);
                assert_eq!(compress(x, d) as u64, exact, "x={x} d={d}");
            }
        }
    }
}
