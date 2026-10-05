// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Git's delta format (`gitformat-pack(5)`, "Deltified representation"): two size varints (source,
//! target), then instructions — `1xxxxxxx` COPY (offset bytes selected by bits 0–3, size bytes by
//! bits 4–6, size 0 meaning 0x10000) and `0xxxxxxx` INSERT of 1..=127 literal bytes; `0x00` is
//! reserved and refused.
//!
//! The ENCODER indexes the source in 16-byte blocks (a polynomial rolling hash), scans the target
//! with the same rolling window, extends each candidate forward and backward (into the pending
//! literals) and keeps the longest; copies are capped at 0x10000 per instruction for pack-v2 readers.

use alloc::vec;
use alloc::vec::Vec;

use crate::{Error, Result};

fn read_varint(d: &[u8], i: &mut usize) -> Result<u64> {
    let mut v = 0u64;
    let mut shift = 0;
    loop {
        let b = *d.get(*i).ok_or(Error::Corrupt("delta: truncated size"))?;
        *i += 1;
        if shift > 63 {
            return Err(Error::Corrupt("delta: size overflow"));
        }
        v |= ((b & 0x7f) as u64) << shift;
        shift += 7;
        if b & 0x80 == 0 {
            return Ok(v);
        }
    }
}

fn write_varint(out: &mut Vec<u8>, mut v: u64) {
    loop {
        let b = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(b);
            return;
        }
        out.push(b | 0x80);
    }
}

/// (source size, target size) from a delta's header.
pub fn sizes(delta: &[u8]) -> Result<(u64, u64)> {
    let mut i = 0;
    Ok((read_varint(delta, &mut i)?, read_varint(delta, &mut i)?))
}

/// Apply `delta` to `base`.
pub fn apply(base: &[u8], delta: &[u8]) -> Result<Vec<u8>> {
    let mut i = 0;
    let src = read_varint(delta, &mut i)?;
    let dst = read_varint(delta, &mut i)?;
    if src != base.len() as u64 {
        return Err(Error::Corrupt("delta: source size mismatch"));
    }
    let mut out = Vec::with_capacity(dst.min(1 << 30) as usize);
    while i < delta.len() {
        let op = delta[i];
        i += 1;
        if op & 0x80 != 0 {
            let mut off = 0u64;
            let mut size = 0u64;
            for k in 0..4 {
                if op & (1 << k) != 0 {
                    off |= (*delta.get(i).ok_or(Error::Corrupt("delta: truncated copy"))? as u64) << (8 * k);
                    i += 1;
                }
            }
            for k in 0..3 {
                if op & (0x10 << k) != 0 {
                    size |= (*delta.get(i).ok_or(Error::Corrupt("delta: truncated copy"))? as u64) << (8 * k);
                    i += 1;
                }
            }
            if size == 0 {
                size = 0x10000;
            }
            let end = off.checked_add(size).ok_or(Error::Corrupt("delta: copy overflow"))?;
            if end > base.len() as u64 || out.len() as u64 + size > dst {
                return Err(Error::Corrupt("delta: copy out of range"));
            }
            out.extend_from_slice(&base[off as usize..end as usize]);
        } else if op != 0 {
            let n = op as usize;
            if i + n > delta.len() || out.len() + n > dst as usize {
                return Err(Error::Corrupt("delta: insert out of range"));
            }
            out.extend_from_slice(&delta[i..i + n]);
            i += n;
        } else {
            return Err(Error::Corrupt("delta: reserved opcode 0"));
        }
    }
    if out.len() as u64 != dst {
        return Err(Error::Corrupt("delta: target size mismatch"));
    }
    Ok(out)
}

const BLOCK: usize = 16;
const MUL: u32 = 0x0100_0193; // FNV prime as the polynomial base
const MAX_COPY: usize = 0x10000;

fn block_hash(b: &[u8]) -> u32 {
    let mut h = 0u32;
    for &x in &b[..BLOCK] {
        h = h.wrapping_mul(MUL).wrapping_add(x as u32 + 1);
    }
    h
}

/// A reusable index of one source buffer.
pub struct DeltaIndex<'a> {
    src: &'a [u8],
    bits: u32,
    heads: Vec<u32>, // bucket -> 1 + offset, 0 empty
    next: Vec<u32>,  // block number -> 1 + offset of the previous block in the same bucket
}

impl<'a> DeltaIndex<'a> {
    /// Index `src`.
    pub fn new(src: &'a [u8]) -> Self {
        let blocks = src.len() / BLOCK;
        let mut bits = 4;
        while (1usize << bits) < blocks.max(1) && bits < 24 {
            bits += 1;
        }
        let mut heads = vec![0u32; 1 << bits];
        let mut next = vec![0u32; blocks];
        // Insert from the end so that chains start at the EARLIEST matching block.
        for b in (0..blocks).rev() {
            let off = b * BLOCK;
            let h = (block_hash(&src[off..]) >> (32 - bits)) as usize;
            next[b] = heads[h];
            heads[h] = off as u32 + 1;
        }
        DeltaIndex { src, bits, heads, next }
    }

    /// A delta that turns the source into `target`, or `None` once it would exceed `max_size`.
    pub fn delta(&self, target: &[u8], max_size: usize) -> Option<Vec<u8>> {
        let src = self.src;
        let mut out = Vec::with_capacity(target.len() / 4 + 16);
        write_varint(&mut out, src.len() as u64);
        write_varint(&mut out, target.len() as u64);
        let mut lit_start = 0usize; // first byte of the pending literal run
        let mut j = 0usize;
        let mut pow = 1u32; // MUL^(BLOCK-1)
        for _ in 0..BLOCK - 1 {
            pow = pow.wrapping_mul(MUL);
        }
        let mut h = if target.len() >= BLOCK { block_hash(target) } else { 0 };
        while j + BLOCK <= target.len() {
            let bucket = (h >> (32 - self.bits)) as usize;
            let mut cand = self.heads[bucket];
            let mut best_off = 0usize;
            let mut best_len = 0usize;
            let mut tries = 0;
            while cand != 0 && tries < 64 {
                let off = cand as usize - 1;
                let max = (src.len() - off).min(target.len() - j);
                let mut l = 0;
                while l < max && src[off + l] == target[j + l] {
                    l += 1;
                }
                if l > best_len {
                    best_len = l;
                    best_off = off;
                    if l >= 4096 {
                        break;
                    }
                }
                cand = self.next[off / BLOCK];
                tries += 1;
            }
            if best_len >= BLOCK {
                // Extend backwards into the pending literals.
                let mut back = 0;
                while j - back > lit_start && best_off > back && src[best_off - back - 1] == target[j - back - 1] {
                    back += 1;
                }
                let (mut off, mut len) = (best_off - back, best_len + back);
                flush_insert(&mut out, &target[lit_start..j - back]);
                while len > 0 {
                    let n = len.min(MAX_COPY);
                    emit_copy(&mut out, off, n);
                    off += n;
                    len -= n;
                }
                j += best_len;
                lit_start = j;
                if out.len() > max_size {
                    return None;
                }
                if j + BLOCK <= target.len() {
                    h = block_hash(&target[j..]);
                }
                continue;
            }
            // Roll one byte.
            if j + BLOCK < target.len() {
                h = h.wrapping_sub((target[j] as u32 + 1).wrapping_mul(pow)).wrapping_mul(MUL).wrapping_add(target[j + BLOCK] as u32 + 1);
            }
            j += 1;
            if j - lit_start > 4096 && out.len() + (j - lit_start) > max_size {
                return None;
            }
        }
        flush_insert(&mut out, &target[lit_start..]);
        if out.len() > max_size { None } else { Some(out) }
    }
}

fn flush_insert(out: &mut Vec<u8>, mut lit: &[u8]) {
    while !lit.is_empty() {
        let n = lit.len().min(127);
        out.push(n as u8);
        out.extend_from_slice(&lit[..n]);
        lit = &lit[n..];
    }
}

fn emit_copy(out: &mut Vec<u8>, off: usize, size: usize) {
    let pos = out.len();
    out.push(0x80);
    let mut op = 0x80u8;
    for k in 0..4 {
        let b = (off >> (8 * k)) as u8;
        if b != 0 {
            op |= 1 << k;
            out.push(b);
        }
    }
    let s = if size == 0x10000 { 0 } else { size };
    for k in 0..3 {
        let b = (s >> (8 * k)) as u8;
        if b != 0 {
            op |= 0x10 << k;
            out.push(b);
        }
    }
    out[pos] = op;
}

/// One-shot delta of `target` against `src`.
pub fn encode(src: &[u8], target: &[u8]) -> Vec<u8> {
    DeltaIndex::new(src).delta(target, usize::MAX).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip() {
        let mut a: Vec<u8> = (0..100_000u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
        let mut b = a.clone();
        b.splice(5000..5010, b"INSERTED TEXT HERE".iter().copied());
        b.drain(70_000..71_000);
        b.extend_from_slice(&a[..300]);
        let d = encode(&a, &b);
        assert!(d.len() < 200, "delta {} bytes", d.len());
        assert_eq!(apply(&a, &d).unwrap(), b);
        // degenerate
        assert_eq!(apply(b"", &encode(b"", b"xyz")).unwrap(), b"xyz");
        assert_eq!(apply(b"abc", &encode(b"abc", b"")).unwrap(), b"");
        a.truncate(10);
        assert_eq!(apply(&a, &encode(&a, &a)).unwrap(), a);
    }
}
