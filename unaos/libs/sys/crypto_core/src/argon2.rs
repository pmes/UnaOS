// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Argon2 (RFC 9106): Argon2id (the RFC's and UnaOS's choice for passwords), Argon2i and Argon2d,
//! versions 0x13 (current) and 0x10 (legacy, for old hashes).
//!
//! No allocation: the caller lends the memory (`&mut [Block]`, at least [`Params::blocks`] blocks — the
//! kernel can hand it a page run, ring 3 a `Vec`); with the `alloc` feature [`hash`] allocates it. Lanes
//! are computed one after another (no threads in `no_std`); the result is identical to a parallel run.
//! The memory is zeroized before returning.
//!
//! CONSTANT-TIME / side channels — stated honestly, because Argon2's design trades here:
//!  * Argon2i, and Argon2id's first half-pass (slices 0–1 of pass 0): memory addresses are independent
//!    of the password (data-independent addressing), so the access pattern leaks nothing.
//!  * Argon2d, and the rest of Argon2id: block addresses DEPEND on the memory contents (that is what
//!    makes them GPU/ASIC-hard). An attacker who can observe this machine's cache-line accesses during
//!    hashing learns password-dependent indices. That is inherent to the algorithm, not a defect of this
//!    implementation; Argon2id is RFC 9106's recommended compromise.
//!  * The arithmetic itself (BlaMka G, BLAKE2b) is constant-time.

use crate::blake2b::Blake2b;
use crate::ct::Zeroize;
use crate::Error;

/// One 1 KiB Argon2 memory block.
#[derive(Clone, Copy)]
pub struct Block(pub [u64; 128]);

impl Block {
    /// The all-zero block.
    pub const ZERO: Block = Block([0; 128]);
}

impl Default for Block {
    fn default() -> Self {
        Block::ZERO
    }
}

/// Which Argon2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// Data-dependent addressing.
    Argon2d = 0,
    /// Data-independent addressing.
    Argon2i = 1,
    /// Hybrid: independent for the first half of the first pass, dependent after.
    Argon2id = 2,
}

/// Argon2 version.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Version {
    /// 1.0 — blocks are overwritten on later passes.
    V0x10 = 0x10,
    /// 1.3 — blocks are XORed on later passes (RFC 9106).
    V0x13 = 0x13,
}

/// Cost parameters (RFC 9106 §3.1).
#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Variant.
    pub variant: Variant,
    /// Version.
    pub version: Version,
    /// Memory size m in KiB (>= 8 * lanes).
    pub m_kib: u32,
    /// Passes t (>= 1).
    pub t: u32,
    /// Lanes p (1 ..= 2^24 - 1).
    pub p: u32,
}

impl Params {
    /// RFC 9106 §4 "SECOND RECOMMENDED" (memory-constrained) option: Argon2id, t = 3, p = 4, m = 64 MiB.
    pub const RFC9106_SECOND: Params = Params { variant: Variant::Argon2id, version: Version::V0x13, m_kib: 64 * 1024, t: 3, p: 4 };

    /// Blocks of memory needed: m' = 4p * floor(m / 4p).
    pub fn blocks(&self) -> usize {
        let p4 = 4 * self.p as usize;
        p4 * (self.m_kib as usize / p4)
    }

    fn validate(&self, out_len: usize, salt_len: usize) -> Result<(), Error> {
        if self.p == 0 || self.p > 0x00ff_ffff || self.t == 0 || (self.m_kib as u64) < 8 * self.p as u64 {
            return Err(Error::Param);
        }
        if out_len < 4 || salt_len < 8 {
            return Err(Error::Length);
        }
        Ok(())
    }
}

/// H'^T (RFC 9106 §3.3): variable-length BLAKE2b.
fn h_prime(out: &mut [u8], parts: &[&[u8]]) {
    let t = out.len();
    let mut h = Blake2b::new(core::cmp::min(t, 64)).unwrap();
    h.update(&(t as u32).to_le_bytes());
    for p in parts {
        h.update(p);
    }
    if t <= 64 {
        h.finalize_into(out);
        return;
    }
    let mut v = [0u8; 64];
    h.finalize_into(&mut v);
    out[..32].copy_from_slice(&v[..32]);
    let r = t.div_ceil(32) - 2;
    let mut pos = 32;
    for _ in 1..r {
        let mut h = Blake2b::new(64).unwrap();
        h.update(&v);
        h.finalize_into(&mut v);
        out[pos..pos + 32].copy_from_slice(&v[..32]);
        pos += 32;
    }
    let last = t - 32 * r;
    let mut h = Blake2b::new(last).unwrap();
    h.update(&v);
    let mut tail = [0u8; 64];
    h.finalize_into(&mut tail);
    out[pos..pos + last].copy_from_slice(&tail[..last]);
    v.zeroize();
    tail.zeroize();
}

#[inline(always)]
fn fbla(a: u64, b: u64) -> u64 {
    a.wrapping_add(b).wrapping_add(2u64.wrapping_mul((a as u32 as u64) * (b as u32 as u64)))
}

#[inline(always)]
fn gb(v: &mut [u64; 128], a: usize, b: usize, c: usize, d: usize) {
    v[a] = fbla(v[a], v[b]);
    v[d] = (v[d] ^ v[a]).rotate_right(32);
    v[c] = fbla(v[c], v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(24);
    v[a] = fbla(v[a], v[b]);
    v[d] = (v[d] ^ v[a]).rotate_right(16);
    v[c] = fbla(v[c], v[d]);
    v[b] = (v[b] ^ v[c]).rotate_right(63);
}

/// The permutation P on 16 words named by index.
#[inline(always)]
fn perm(v: &mut [u64; 128], i: [usize; 16]) {
    gb(v, i[0], i[4], i[8], i[12]);
    gb(v, i[1], i[5], i[9], i[13]);
    gb(v, i[2], i[6], i[10], i[14]);
    gb(v, i[3], i[7], i[11], i[15]);
    gb(v, i[0], i[5], i[10], i[15]);
    gb(v, i[1], i[6], i[11], i[12]);
    gb(v, i[2], i[7], i[8], i[13]);
    gb(v, i[3], i[4], i[9], i[14]);
}

/// The compression G(X, Y) (RFC 9106 §3.5): R = X ^ Y, P on rows then columns, result Q ^ R.
fn g_block(x: &Block, y: &Block) -> Block {
    let mut r = [0u64; 128];
    for i in 0..128 {
        r[i] = x.0[i] ^ y.0[i];
    }
    let mut q = r;
    for row in 0..8 {
        let b = 16 * row;
        perm(&mut q, core::array::from_fn(|k| b + k));
    }
    for col in 0..8 {
        let b = 2 * col;
        perm(&mut q, core::array::from_fn(|k| b + 16 * (k / 2) + (k % 2)));
    }
    for i in 0..128 {
        q[i] ^= r[i];
    }
    Block(q)
}

fn block_from_bytes(b: &[u8; 1024]) -> Block {
    let mut o = [0u64; 128];
    for i in 0..128 {
        o[i] = u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().unwrap());
    }
    Block(o)
}

/// Argon2 into `out` (4 or more bytes) using the lent `memory` (at least `params.blocks()` blocks).
/// `secret` (K) and `ad` (X) may be empty. The salt must be at least 8 bytes (RFC 9106 §3.1; 16 is
/// recommended).
pub fn argon2(params: &Params, password: &[u8], salt: &[u8], secret: &[u8], ad: &[u8], memory: &mut [Block], out: &mut [u8]) -> Result<(), Error> {
    params.validate(out.len(), salt.len())?;
    let mm = params.blocks();
    if memory.len() < mm {
        return Err(Error::Param);
    }
    let mem = &mut memory[..mm];
    let lanes = params.p as usize;
    let q = mm / lanes;
    let sl = q / 4;

    // H0 (§3.2 step 1)
    let mut h0 = [0u8; 64];
    {
        let mut h = Blake2b::new(64).unwrap();
        for v in [params.p, out.len() as u32, params.m_kib, params.t, params.version as u32, params.variant as u32] {
            h.update(&v.to_le_bytes());
        }
        for s in [password, salt, secret, ad] {
            h.update(&(s.len() as u32).to_le_bytes());
            h.update(s);
        }
        h.finalize_into(&mut h0);
    }
    let mut tmp = [0u8; 1024];
    for l in 0..lanes {
        for j in 0..2u32 {
            h_prime(&mut tmp, &[&h0, &j.to_le_bytes(), &(l as u32).to_le_bytes()]);
            mem[l * q + j as usize] = block_from_bytes(&tmp);
        }
    }
    tmp.zeroize();
    h0.zeroize();

    for pass in 0..params.t as usize {
        for slice in 0..4usize {
            for lane in 0..lanes {
                let independent = match params.variant {
                    Variant::Argon2i => true,
                    Variant::Argon2d => false,
                    Variant::Argon2id => pass == 0 && slice < 2,
                };
                let mut input = Block::ZERO;
                let mut addr = Block::ZERO;
                if independent {
                    input.0[0] = pass as u64;
                    input.0[1] = lane as u64;
                    input.0[2] = slice as u64;
                    input.0[3] = mm as u64;
                    input.0[4] = params.t as u64;
                    input.0[5] = params.variant as u64;
                }
                let start = if pass == 0 && slice == 0 { 2 } else { 0 };
                if independent && start == 2 {
                    input.0[6] += 1;
                    addr = g_block(&Block::ZERO, &g_block(&Block::ZERO, &input));
                }
                for idx in start..sl {
                    let cur = slice * sl + idx;
                    let prev = if cur == 0 { q - 1 } else { cur - 1 };
                    let pseudo = if independent {
                        if idx % 128 == 0 {
                            input.0[6] += 1;
                            addr = g_block(&Block::ZERO, &g_block(&Block::ZERO, &input));
                        }
                        addr.0[idx % 128]
                    } else {
                        mem[lane * q + prev].0[0]
                    };
                    let j1 = pseudo & 0xffff_ffff;
                    let j2 = pseudo >> 32;
                    let ref_lane = if pass == 0 && slice == 0 { lane } else { (j2 % lanes as u64) as usize };
                    let same = ref_lane == lane;
                    let area = if pass == 0 {
                        if slice == 0 || same {
                            slice * sl + idx - 1
                        } else {
                            slice * sl - if idx == 0 { 1 } else { 0 }
                        }
                    } else if same {
                        q - sl + idx - 1
                    } else {
                        q - sl - if idx == 0 { 1 } else { 0 }
                    };
                    let x = (j1 * j1) >> 32;
                    let y = (area as u64 * x) >> 32;
                    let rel = area as u64 - 1 - y;
                    let start_pos = if pass == 0 || slice == 3 { 0 } else { (slice + 1) * sl };
                    let ref_idx = (start_pos + rel as usize) % q;
                    let new = g_block(&mem[lane * q + prev], &mem[ref_lane * q + ref_idx]);
                    let dst = &mut mem[lane * q + cur];
                    if pass > 0 && params.version == Version::V0x13 {
                        for i in 0..128 {
                            dst.0[i] ^= new.0[i];
                        }
                    } else {
                        *dst = new;
                    }
                }
            }
        }
    }

    let mut c = mem[q - 1];
    for l in 1..lanes {
        for i in 0..128 {
            c.0[i] ^= mem[l * q + q - 1].0[i];
        }
    }
    let mut cb = [0u8; 1024];
    for i in 0..128 {
        cb[i * 8..i * 8 + 8].copy_from_slice(&c.0[i].to_le_bytes());
    }
    h_prime(out, &[&cb]);
    cb.zeroize();
    c.0.zeroize();
    for b in mem.iter_mut() {
        b.0.zeroize();
    }
    Ok(())
}

/// Argon2 with self-allocated memory.
#[cfg(feature = "alloc")]
pub fn hash(params: &Params, password: &[u8], salt: &[u8], secret: &[u8], ad: &[u8], out: &mut [u8]) -> Result<(), Error> {
    let mut mem = alloc::vec![Block::ZERO; params.blocks()];
    argon2(params, password, salt, secret, ad, &mut mem, out)
}
