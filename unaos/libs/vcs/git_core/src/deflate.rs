// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! A DEFLATE ENCODER (RFC 1951) and zlib framing (RFC 1950) — the first compressor in the UnaOS
//! tree (every earlier core only inflates; pixel_core's decoder is the inverse and the oracle).
//!
//! * LZ77 (§4): a 3-byte hash with chained positions over the 32 KiB window, lazy evaluation one
//!   position ahead (a match is deferred when the next position matches longer), and the
//!   "too far" rule (a 3-byte match more than 4 KiB back is cheaper as literals). Levels 1–9 pick
//!   chain depth / lazy threshold / nice length from the classic zlib parameter table.
//! * Blocks (§3.2.3–§3.2.7): each block of up to 16 Ki symbols is costed exactly three ways —
//!   STORED (BTYPE 00, split at 65 535 bytes), FIXED Huffman (01) and DYNAMIC Huffman (10) — and
//!   the cheapest is emitted. Dynamic codes are built by Huffman's algorithm and limited to 15 bits
//!   (7 for the code-length alphabet) by the Kraft-sum repair; the code-length sequence is run-length
//!   coded with symbols 16/17/18 across the literal/length and distance tables as §3.2.7 allows.
//! * Every emitted code table is COMPLETE (a lone used symbol gets a 1-bit sibling), so a strict
//!   decoder that rejects incomplete codes accepts every stream this writes.
//!
//! Proof (tests/deflate.rs): round trip through pixel_core's inflater over random, text, binary and
//! degenerate inputs at every level, each block type forced and observed; `git fsck` accepts loose
//! objects and packs compressed here.

use alloc::vec;
use alloc::vec::Vec;

/// Compression level 0 (stored only) ..= 9. Git's default (`core.compression` -1) is zlib's 6.
pub const DEFAULT_LEVEL: u8 = 6;

const WSIZE: usize = 32768;
const WMASK: usize = WSIZE - 1;
const HASH_BITS: usize = 15;
const HASH_SIZE: usize = 1 << HASH_BITS;
const MIN_MATCH: usize = 3;
const MAX_MATCH: usize = 258;
const MAX_DIST: usize = 32768;
const TOO_FAR: usize = 4096;
const BLOCK_SYMBOLS: usize = 16384;

/// (good_length, max_lazy, nice_length, max_chain) per level, zlib's `configuration_table`.
const LEVELS: [(usize, usize, usize, usize); 10] = [
    (0, 0, 0, 0),
    (4, 4, 8, 4),
    (4, 5, 16, 8),
    (4, 6, 32, 32),
    (4, 4, 16, 16),
    (8, 16, 32, 32),
    (8, 16, 128, 128),
    (8, 32, 128, 256),
    (32, 128, 258, 1024),
    (32, 258, 258, 4096),
];

const LEN_BASE: [u16; 29] =
    [3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195, 227, 258];
const LEN_EXTRA: [u8; 29] = [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073, 4097,
    6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] =
    [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
const CL_ORDER: [usize; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

fn len_code(len: usize) -> usize {
    // 3..=258 -> index into LEN_BASE
    let mut c = 28;
    while LEN_BASE[c] as usize > len {
        c -= 1;
    }
    c
}

fn dist_code(d: usize) -> usize {
    let mut lo = 0usize;
    let mut hi = 29usize;
    while lo < hi {
        let mid = (lo + hi + 1) / 2;
        if DIST_BASE[mid] as usize <= d {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    lo
}

// ---------------------------------------------------------------------------------------------
// Bit writer (LSB-first, §3.1.1)
// ---------------------------------------------------------------------------------------------

struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn put(&mut self, v: u32, n: u32) {
        debug_assert!(n <= 32);
        self.acc |= (v as u64) << self.n;
        self.n += n;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }
    fn align(&mut self) {
        if self.n > 0 {
            self.out.push(self.acc as u8);
            self.acc = 0;
            self.n = 0;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Huffman code construction
// ---------------------------------------------------------------------------------------------

/// Code lengths for `freq`, at most `limit` bits, every used symbol coded and the code complete.
fn build_lengths(freq: &[u32], limit: u8) -> Vec<u8> {
    let n = freq.len();
    let mut lens = vec![0u8; n];
    let mut used: Vec<usize> = (0..n).filter(|&i| freq[i] > 0).collect();
    if used.is_empty() {
        lens[0] = 1;
        lens[1] = 1;
        return lens;
    }
    if used.len() == 1 {
        let s = used[0];
        lens[s] = 1;
        lens[if s == 0 { 1 } else { 0 }] = 1;
        return lens;
    }
    // Huffman: leaves sorted by (freq, symbol); two-queue merge.
    used.sort_by_key(|&i| (freq[i], i));
    let m = used.len();
    let mut weight: Vec<u64> = used.iter().map(|&i| freq[i] as u64).collect();
    let mut parent: Vec<usize> = vec![usize::MAX; 2 * m - 1];
    weight.reserve(m - 1);
    let (mut li, mut ni) = (0usize, m); // next leaf, next internal
    for k in m..2 * m - 1 {
        let mut pick = || {
            if li < m && (ni >= k || weight[li] <= weight[ni]) {
                li += 1;
                li - 1
            } else {
                ni += 1;
                ni - 1
            }
        };
        let a = pick();
        let b = pick();
        weight.push(weight[a] + weight[b]);
        parent[a] = k;
        parent[b] = k;
    }
    // depth of each node (root = 2m-2, depth 0), computed top-down since parent index > child.
    let mut depth = vec![0u32; 2 * m - 1];
    for k in (0..2 * m - 2).rev() {
        depth[k] = depth[parent[k]] + 1;
    }
    let mut count = [0u32; 64];
    for k in 0..m {
        count[depth[k].min(63) as usize] += 1;
    }
    // Kraft repair: fold everything deeper than `limit` to `limit`, then split shorter codes until
    // the sum of 2^(limit-len) is exactly 2^limit.
    let limit = limit as usize;
    for d in limit + 1..64 {
        count[limit] += count[d];
        count[d] = 0;
    }
    let mut total: u64 = (1..=limit).map(|d| (count[d] as u64) << (limit - d)).sum();
    while total > 1u64 << limit {
        count[limit] -= 1;
        for d in (1..limit).rev() {
            if count[d] > 0 {
                count[d] -= 1;
                count[d + 1] += 2;
                break;
            }
        }
        total -= 1;
    }
    // Assign: the lowest frequencies (front of `used`) get the longest codes.
    let mut k = 0;
    for d in (1..=limit).rev() {
        for _ in 0..count[d] {
            lens[used[k]] = d as u8;
            k += 1;
        }
    }
    lens
}

/// Canonical codes (§3.2.2), returned BIT-REVERSED ready for the LSB-first writer.
fn canonical(lens: &[u8]) -> Vec<u16> {
    let mut bl_count = [0u16; 16];
    for &l in lens {
        if l > 0 {
            bl_count[l as usize] += 1;
        }
    }
    let mut next = [0u16; 16];
    let mut code = 0u16;
    for bits in 1..16 {
        code = (code + bl_count[bits - 1]) << 1;
        next[bits] = code;
    }
    lens.iter()
        .map(|&l| {
            if l == 0 {
                return 0;
            }
            let c = next[l as usize];
            next[l as usize] += 1;
            c.reverse_bits() >> (16 - l)
        })
        .collect()
}

fn fixed_lit_lens() -> [u8; 288] {
    let mut l = [0u8; 288];
    for (i, x) in l.iter_mut().enumerate() {
        *x = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    l
}

// ---------------------------------------------------------------------------------------------
// Symbols
// ---------------------------------------------------------------------------------------------

/// A literal (`dist == 0`, `lit` = byte) or a back-reference.
#[derive(Clone, Copy)]
struct Sym {
    len_or_lit: u16,
    dist: u16,
}

/// The encoder state over one whole input.
struct Encoder<'a> {
    data: &'a [u8],
    head: Vec<u32>,
    prev: Vec<u32>,
    good: usize,
    lazy: usize,
    nice: usize,
    chain: usize,
}

impl<'a> Encoder<'a> {
    #[inline]
    fn hash(&self, i: usize) -> usize {
        let d = self.data;
        (((d[i] as usize) << 10) ^ ((d[i + 1] as usize) << 5) ^ d[i + 2] as usize) & (HASH_SIZE - 1)
    }

    #[inline]
    fn insert(&mut self, i: usize) {
        if i + MIN_MATCH > self.data.len() {
            return;
        }
        let h = self.hash(i);
        self.prev[i & WMASK] = self.head[h];
        self.head[h] = i as u32 + 1;
    }

    /// Longest match for position `i` among chained earlier positions: (len, dist).
    fn find(&self, i: usize, prev_len: usize) -> (usize, usize) {
        let d = self.data;
        if i + MIN_MATCH > d.len() {
            return (0, 0);
        }
        let max = (d.len() - i).min(MAX_MATCH);
        let mut chain = if prev_len >= self.good { self.chain >> 2 } else { self.chain };
        let mut cand = self.head[self.hash(i)] as usize;
        let mut best = (0usize, 0usize);
        while cand > 0 && chain > 0 {
            let c = cand - 1;
            if c >= i || i - c > MAX_DIST {
                break;
            }
            if d[c + best.0.min(max - 1)] == d[i + best.0.min(max - 1)] {
                let mut l = 0;
                while l < max && d[c + l] == d[i + l] {
                    l += 1;
                }
                if l > best.0 {
                    best = (l, i - c);
                    if l >= self.nice || l == max {
                        break;
                    }
                }
            }
            let next = self.prev[c & WMASK] as usize;
            if next >= cand {
                break; // a slot overwritten by a newer position: the chain ends here
            }
            cand = next;
            chain -= 1;
        }
        if best.0 < MIN_MATCH || (best.0 == MIN_MATCH && best.1 > TOO_FAR) {
            return (0, 0);
        }
        best
    }
}

/// Raw DEFLATE (no zlib framing) of `data` at `level` (0..=9; above 9 is 9).
pub fn deflate(data: &[u8], level: u8) -> Vec<u8> {
    let level = level.min(9) as usize;
    let mut bits = Bits { out: Vec::with_capacity(data.len() / 2 + 64), acc: 0, n: 0 };
    if level == 0 {
        write_stored(&mut bits, data, true);
        bits.align();
        return bits.out;
    }
    if data.is_empty() {
        // One final fixed block holding only end-of-block.
        bits.put(1, 1);
        bits.put(1, 2);
        bits.put(0, 7);
        bits.align();
        return bits.out;
    }
    let (good, lazy, nice, chain) = LEVELS[level];
    let mut e = Encoder { data, head: vec![0; HASH_SIZE], prev: vec![0; WSIZE], good, lazy, nice, chain };
    let mut syms: Vec<Sym> = Vec::with_capacity(BLOCK_SYMBOLS);
    let mut block_start = 0usize;
    let mut i = 0usize;
    let mut pending: Option<(usize, usize)> = None;
    let mut finished = false;
    let n = data.len();
    while i < n {
        let m = match pending.take() {
            Some(m) => m,
            None => {
                let m = e.find(i, 0);
                e.insert(i);
                m
            }
        };
        if m.0 >= MIN_MATCH {
            if m.0 < e.lazy && i + 1 < n {
                let m2 = e.find(i + 1, m.0);
                e.insert(i + 1);
                if m2.0 > m.0 {
                    syms.push(Sym { len_or_lit: data[i] as u16, dist: 0 });
                    i += 1;
                    pending = Some(m2);
                } else {
                    syms.push(Sym { len_or_lit: m.0 as u16, dist: m.1 as u16 });
                    for p in i + 2..i + m.0 {
                        e.insert(p);
                    }
                    i += m.0;
                }
            } else {
                syms.push(Sym { len_or_lit: m.0 as u16, dist: m.1 as u16 });
                for p in i + 1..i + m.0 {
                    e.insert(p);
                }
                i += m.0;
            }
        } else {
            syms.push(Sym { len_or_lit: data[i] as u16, dist: 0 });
            i += 1;
        }
        if syms.len() >= BLOCK_SYMBOLS && pending.is_none() {
            write_block(&mut bits, &syms, &data[block_start..i], i == n);
            finished = i == n;
            syms.clear();
            block_start = i;
        }
    }
    if !finished {
        write_block(&mut bits, &syms, &data[block_start..], true);
    }
    bits.align();
    bits.out
}

/// Which block type [`write_block`] chose — exposed to the tests through [`deflate_with`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockType {
    /// BTYPE 00.
    Stored,
    /// BTYPE 01.
    Fixed,
    /// BTYPE 10.
    Dynamic,
}

fn write_stored(bits: &mut Bits, raw: &[u8], last: bool) {
    let mut chunks = raw.chunks(65535).peekable();
    if raw.is_empty() {
        bits.put(last as u32, 1);
        bits.put(0, 2);
        bits.align();
        bits.out.extend_from_slice(&[0, 0, 0xff, 0xff]);
        return;
    }
    while let Some(c) = chunks.next() {
        let fin = last && chunks.peek().is_none();
        bits.put(fin as u32, 1);
        bits.put(0, 2);
        bits.align();
        let l = c.len() as u16;
        bits.out.extend_from_slice(&l.to_le_bytes());
        bits.out.extend_from_slice(&(!l).to_le_bytes());
        bits.out.extend_from_slice(c);
    }
}

struct Dyn {
    lit_lens: Vec<u8>,
    dist_lens: Vec<u8>,
    hlit: usize,
    hdist: usize,
    cl_syms: Vec<(u8, u8)>, // (symbol, extra value)
    cl_lens: Vec<u8>,
    hclen: usize,
    header_bits: u64,
}

fn plan_dynamic(lf: &[u32; 286], df: &[u32; 30]) -> Dyn {
    let lit_lens = build_lengths(lf, 15);
    let dist_lens = build_lengths(df, 15);
    let hlit = (257..=286).rev().find(|&k| lit_lens[k - 1] != 0).unwrap_or(257).max(257);
    let hdist = (1..=30).rev().find(|&k| dist_lens[k - 1] != 0).unwrap_or(1).max(1);
    let mut seq: Vec<u8> = Vec::with_capacity(hlit + hdist);
    seq.extend_from_slice(&lit_lens[..hlit]);
    seq.extend_from_slice(&dist_lens[..hdist]);
    // Run-length code (§3.2.7).
    let mut cl_syms = Vec::new();
    let mut k = 0;
    while k < seq.len() {
        let v = seq[k];
        let mut run = 1;
        while k + run < seq.len() && seq[k + run] == v {
            run += 1;
        }
        if v == 0 && run >= 3 {
            let mut r = run;
            while r >= 3 {
                if r >= 11 {
                    let t = r.min(138);
                    cl_syms.push((18, (t - 11) as u8));
                    r -= t;
                } else {
                    cl_syms.push((17, (r - 3) as u8));
                    r = 0;
                }
            }
            for _ in 0..r {
                cl_syms.push((0, 0));
            }
            k += run;
        } else if v != 0 && run >= 4 {
            cl_syms.push((v, 0));
            let mut r = run - 1;
            while r >= 3 {
                let t = r.min(6);
                cl_syms.push((16, (t - 3) as u8));
                r -= t;
            }
            for _ in 0..r {
                cl_syms.push((v, 0));
            }
            k += run;
        } else {
            for _ in 0..run {
                cl_syms.push((v, 0));
            }
            k += run;
        }
    }
    let mut cf = [0u32; 19];
    for &(s, _) in &cl_syms {
        cf[s as usize] += 1;
    }
    let cl_lens = build_lengths(&cf, 7);
    let hclen = (4..=19).rev().find(|&k| cl_lens[CL_ORDER[k - 1]] != 0).unwrap_or(4).max(4);
    let mut header_bits: u64 = 5 + 5 + 4 + 3 * hclen as u64;
    for &(s, _) in &cl_syms {
        header_bits += cl_lens[s as usize] as u64 + match s {
            16 => 2,
            17 => 3,
            18 => 7,
            _ => 0,
        };
    }
    Dyn { lit_lens, dist_lens, hlit, hdist, cl_syms, cl_lens, hclen, header_bits }
}

fn body_bits(syms_freq_l: &[u32; 286], syms_freq_d: &[u32; 30], ll: &[u8], dl: &[u8]) -> u64 {
    let mut b = 0u64;
    for s in 0..286 {
        if syms_freq_l[s] > 0 {
            let extra = if s > 256 { LEN_EXTRA[s - 257] as u64 } else { 0 };
            b += syms_freq_l[s] as u64 * (ll[s] as u64 + extra);
        }
    }
    for s in 0..30 {
        if syms_freq_d[s] > 0 {
            b += syms_freq_d[s] as u64 * (dl[s] as u64 + DIST_EXTRA[s] as u64);
        }
    }
    b
}

fn write_block(bits: &mut Bits, syms: &[Sym], raw: &[u8], last: bool) -> BlockType {
    write_block_forced(bits, syms, raw, last, None)
}

fn write_block_forced(bits: &mut Bits, syms: &[Sym], raw: &[u8], last: bool, force: Option<BlockType>) -> BlockType {
    let mut lf = [0u32; 286];
    let mut df = [0u32; 30];
    for s in syms {
        if s.dist == 0 {
            lf[s.len_or_lit as usize] += 1;
        } else {
            lf[257 + len_code(s.len_or_lit as usize)] += 1;
            df[dist_code(s.dist as usize)] += 1;
        }
    }
    lf[256] = 1;
    let d = plan_dynamic(&lf, &df);
    let fixed_l = fixed_lit_lens();
    let fixed_d = [5u8; 30];
    let dyn_cost = 3 + d.header_bits + body_bits(&lf, &df, &d.lit_lens, &d.dist_lens);
    let fix_cost = 3 + body_bits(&lf, &df, &fixed_l[..286], &fixed_d);
    // Stored: per 65535-byte chunk 3 bits + alignment (<=7) + 32 bits + the bytes.
    let chunks = raw.len().div_ceil(65535).max(1) as u64;
    let stored_cost = chunks * (3 + 7 + 32) + 8 * raw.len() as u64;
    let choice = force.unwrap_or(if stored_cost <= fix_cost.min(dyn_cost) {
        BlockType::Stored
    } else if fix_cost <= dyn_cost {
        BlockType::Fixed
    } else {
        BlockType::Dynamic
    });
    match choice {
        BlockType::Stored => write_stored(bits, raw, last),
        BlockType::Fixed => {
            bits.put(last as u32, 1);
            bits.put(1, 2);
            let lc = canonical(&fixed_l);
            let dc = canonical(&[5u8; 30]);
            emit(bits, syms, &fixed_l, &lc, &fixed_d, &dc);
        }
        BlockType::Dynamic => {
            bits.put(last as u32, 1);
            bits.put(2, 2);
            bits.put((d.hlit - 257) as u32, 5);
            bits.put((d.hdist - 1) as u32, 5);
            bits.put((d.hclen - 4) as u32, 4);
            for &o in &CL_ORDER[..d.hclen] {
                bits.put(d.cl_lens[o] as u32, 3);
            }
            let cc = canonical(&d.cl_lens);
            for &(s, x) in &d.cl_syms {
                bits.put(cc[s as usize] as u32, d.cl_lens[s as usize] as u32);
                match s {
                    16 => bits.put(x as u32, 2),
                    17 => bits.put(x as u32, 3),
                    18 => bits.put(x as u32, 7),
                    _ => {}
                }
            }
            let lc = canonical(&d.lit_lens);
            let dc = canonical(&d.dist_lens);
            emit(bits, syms, &d.lit_lens, &lc, &d.dist_lens, &dc);
        }
    }
    choice
}

fn emit(bits: &mut Bits, syms: &[Sym], ll: &[u8], lc: &[u16], dl: &[u8], dc: &[u16]) {
    for s in syms {
        if s.dist == 0 {
            let v = s.len_or_lit as usize;
            bits.put(lc[v] as u32, ll[v] as u32);
        } else {
            let len = s.len_or_lit as usize;
            let c = len_code(len);
            bits.put(lc[257 + c] as u32, ll[257 + c] as u32);
            if LEN_EXTRA[c] > 0 {
                bits.put((len - LEN_BASE[c] as usize) as u32, LEN_EXTRA[c] as u32);
            }
            let dist = s.dist as usize;
            let k = dist_code(dist);
            bits.put(dc[k] as u32, dl[k] as u32);
            if DIST_EXTRA[k] > 0 {
                bits.put((dist - DIST_BASE[k] as usize) as u32, DIST_EXTRA[k] as u32);
            }
        }
    }
    bits.put(lc[256] as u32, ll[256] as u32);
}

/// Test hook: deflate `data` as ONE block of the forced type (symbols from level 6), so each of the
/// three block encodings is exercised by name. Inputs above 16 Ki symbols still form one block.
pub fn deflate_with(data: &[u8], force: BlockType) -> Vec<u8> {
    let mut bits = Bits { out: Vec::new(), acc: 0, n: 0 };
    let (good, lazy, nice, chain) = LEVELS[6];
    let mut e = Encoder { data, head: vec![0; HASH_SIZE], prev: vec![0; WSIZE], good, lazy, nice, chain };
    let mut syms = Vec::new();
    let mut i = 0;
    while i < data.len() {
        let m = e.find(i, 0);
        e.insert(i);
        if m.0 >= MIN_MATCH {
            syms.push(Sym { len_or_lit: m.0 as u16, dist: m.1 as u16 });
            for p in i + 1..i + m.0 {
                e.insert(p);
            }
            i += m.0;
        } else {
            syms.push(Sym { len_or_lit: data[i] as u16, dist: 0 });
            i += 1;
        }
    }
    write_block_forced(&mut bits, &syms, data, true, Some(force));
    bits.align();
    bits.out
}

// ---------------------------------------------------------------------------------------------
// zlib (RFC 1950)
// ---------------------------------------------------------------------------------------------

/// Adler-32 (RFC 1950 §9).
pub fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for chunk in data.chunks(5552) {
        for &x in chunk {
            a += x as u32;
            b += a;
        }
        a %= 65521;
        b %= 65521;
    }
    (b << 16) | a
}

/// A zlib stream of `data` at `level`: `78 01|5e|9c|da`, raw DEFLATE, big-endian Adler-32.
pub fn zlib_compress(data: &[u8], level: u8) -> Vec<u8> {
    let flevel: u16 = match level {
        0 | 1 => 0,
        2..=5 => 1,
        6 => 2,
        _ => 3,
    };
    let cmf: u16 = 0x78;
    let mut flg = flevel << 6;
    flg += 31 - ((cmf << 8) | flg) % 31;
    let mut out = Vec::with_capacity(data.len() / 2 + 16);
    out.push(cmf as u8);
    out.push(flg as u8);
    out.extend_from_slice(&deflate(data, level));
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zlib_headers_match_zlib() {
        assert_eq!(&zlib_compress(b"", 6)[..2], &[0x78, 0x9c]);
        assert_eq!(&zlib_compress(b"", 1)[..2], &[0x78, 0x01]);
        assert_eq!(&zlib_compress(b"", 9)[..2], &[0x78, 0xda]);
        assert_eq!(&zlib_compress(b"", 4)[..2], &[0x78, 0x5e]);
        // zlib's own empty stream at level 6 is 78 9c 03 00 00 00 00 01.
        assert_eq!(zlib_compress(b"", 6), vec![0x78, 0x9c, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01]);
    }

    #[test]
    fn huffman_lengths_are_complete_and_limited() {
        // Fibonacci frequencies force deep trees.
        let mut f = vec![0u32; 30];
        let (mut a, mut b) = (1u32, 1u32);
        for x in f.iter_mut() {
            *x = a;
            let c = a.saturating_add(b);
            a = b;
            b = c;
        }
        let l = build_lengths(&f, 7);
        assert!(l.iter().all(|&x| x <= 7));
        let kraft: u64 = l.iter().filter(|&&x| x > 0).map(|&x| 1u64 << (7 - x)).sum();
        assert_eq!(kraft, 128);
    }
}
