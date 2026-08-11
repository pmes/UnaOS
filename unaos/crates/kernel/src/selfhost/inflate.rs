// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// SELFHOST-2 — a streaming gzip (RFC 1952) + DEFLATE (RFC 1951) decoder, no_std + alloc.
//
// WHY STREAMING, BOTH ENDS. `SRC.TGZ` is ~5.6 MiB compressed and expands to tens of megabytes of
// tar. The kernel heap is 48 MiB (`allocator::HEAP_SIZE`), shared with everything else, and the
// source tree only grows. So neither end of this pipe is ever buffered whole:
//
//   * INPUT is pulled a byte at a time from a `ByteSource` the caller drives — in practice a FAT
//     cluster-chunk reader that also feeds the SHA-256 as it goes, so the file is read exactly once
//     for both the digest and the walk.
//   * OUTPUT is pushed a byte at a time into a `Sink` — in practice the tar walker, which only ever
//     holds one 512-byte block. The only sizeable allocation here is the mandatory 32 KiB DEFLATE
//     history window, plus the per-block Huffman tables.
//
// The Huffman decoder is the canonical count/symbol form from RFC 1951 §3.2.2 (the "puff" shape):
// decode bit-by-bit, comparing against the running count of codes of each length. It is not the
// fastest possible decoder — a real one would use a lookup table — but it is short enough to audit
// line by line, which is what a self-hosting rung needs from its first decompressor.

use alloc::boxed::Box;
use alloc::vec;
use alloc::vec::Vec;

use crate::hash::Crc32;

/// Everything that can go wrong between "the file is on the volume" and "the member list is real".
/// Each variant names a distinct falsifiable claim, so a FAIL line can say WHICH one broke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InflateError {
    /// The source ran out of bytes mid-stream.
    TruncatedInput,
    /// Not a gzip member: bad magic, or a compression method other than DEFLATE.
    BadGzipHeader,
    /// A reserved DEFLATE block type (BTYPE=3), or a stored block whose LEN/~LEN disagree.
    BadBlock,
    /// A Huffman code table that is over-subscribed or incomplete where completeness is required.
    BadHuffmanTable,
    /// A symbol that decodes to no code in the active table.
    BadSymbol,
    /// A back-reference pointing further back than the bytes produced so far.
    BadDistance,
    /// The gzip trailer's CRC-32 or ISIZE disagrees with what we actually produced.
    TrailerMismatch,
    /// The sink refused a byte (the tar walker rejecting a malformed member).
    SinkRejected,
}

/// A pull source of compressed bytes. `next()` returns `None` at end of input.
pub trait ByteSource {
    fn next(&mut self) -> Option<u8>;
}

/// A push sink for decompressed bytes.
pub trait Sink {
    /// Consume one decompressed byte. `Err(())` aborts the whole inflate.
    fn push(&mut self, byte: u8) -> Result<(), ()>;
}

// ── The bit reader ────────────────────────────────────────────────────────────────────────────
// DEFLATE packs codes LSB-first within each byte, except Huffman codes themselves, which are packed
// MSB-first as bits but are ACCUMULATED here one bit at a time by the decoder — so this reader only
// ever needs the LSB-first primitive.

struct BitReader<'a, S: ByteSource> {
    src: &'a mut S,
    bit_buf: u32,
    bit_cnt: u32,
    /// Total compressed bytes consumed — used only for the honest "how far did we get" report.
    consumed: u64,
}

impl<'a, S: ByteSource> BitReader<'a, S> {
    fn new(src: &'a mut S) -> Self {
        Self { src, bit_buf: 0, bit_cnt: 0, consumed: 0 }
    }

    #[inline]
    fn byte(&mut self) -> Result<u8, InflateError> {
        match self.src.next() {
            Some(b) => {
                self.consumed += 1;
                Ok(b)
            }
            None => Err(InflateError::TruncatedInput),
        }
    }

    #[inline]
    fn bit(&mut self) -> Result<u32, InflateError> {
        if self.bit_cnt == 0 {
            self.bit_buf = self.byte()? as u32;
            self.bit_cnt = 8;
        }
        let b = self.bit_buf & 1;
        self.bit_buf >>= 1;
        self.bit_cnt -= 1;
        Ok(b)
    }

    /// `n` bits, LSB-first (RFC 1951 §3.1.1). `n <= 16`.
    fn bits(&mut self, n: u32) -> Result<u32, InflateError> {
        let mut v = 0u32;
        for i in 0..n {
            v |= self.bit()? << i;
        }
        Ok(v)
    }

    /// Drop to the next byte boundary (stored blocks are byte-aligned).
    fn align(&mut self) {
        self.bit_buf = 0;
        self.bit_cnt = 0;
    }
}

// ── The Huffman decoder ───────────────────────────────────────────────────────────────────────

struct Huffman {
    /// `counts[len]` = how many symbols have a code of that bit length (index 0 unused).
    counts: [u16; 16],
    /// Symbols ordered by (code length, symbol) — the canonical order.
    symbols: Vec<u16>,
}

impl Huffman {
    /// Build from a code-length-per-symbol table. `allow_incomplete` exists for the one legal
    /// incomplete case in RFC 1951: a distance table with a single used code (or none at all, in a
    /// block that emits only literals).
    fn build(lengths: &[u8], allow_incomplete: bool) -> Result<Self, InflateError> {
        let mut counts = [0u16; 16];
        for &l in lengths {
            if l as usize > 15 {
                return Err(InflateError::BadHuffmanTable);
            }
            counts[l as usize] += 1;
        }
        counts[0] = 0; // symbols with length 0 are simply absent

        // Kraft check: `left` is the number of unused codes at each length. Negative = over-
        // subscribed (always fatal); positive at the end = incomplete (fatal unless allowed).
        let mut left: i32 = 1;
        for len in 1..16 {
            left <<= 1;
            left -= counts[len] as i32;
            if left < 0 {
                return Err(InflateError::BadHuffmanTable);
            }
        }
        let used: usize = counts[1..].iter().map(|&c| c as usize).sum();
        if left > 0 && !(allow_incomplete && used <= 1) {
            return Err(InflateError::BadHuffmanTable);
        }

        let mut offs = [0u16; 16];
        for len in 1..15 {
            offs[len + 1] = offs[len] + counts[len];
        }
        let mut symbols = vec![0u16; used];
        for (sym, &l) in lengths.iter().enumerate() {
            if l != 0 {
                symbols[offs[l as usize] as usize] = sym as u16;
                offs[l as usize] += 1;
            }
        }
        Ok(Self { counts, symbols })
    }

    /// Decode one symbol, consuming bits until the accumulated code falls inside the range of codes
    /// of the current length (RFC 1951 §3.2.2).
    fn decode<S: ByteSource>(&self, br: &mut BitReader<'_, S>) -> Result<u16, InflateError> {
        let mut code: i32 = 0;
        let mut first: i32 = 0;
        let mut index: i32 = 0;
        for len in 1..16 {
            code |= br.bit()? as i32;
            let count = self.counts[len] as i32;
            if code - count < first {
                let idx = (index + (code - first)) as usize;
                return self.symbols.get(idx).copied().ok_or(InflateError::BadSymbol);
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        Err(InflateError::BadSymbol)
    }
}

// RFC 1951 §3.2.5 — length codes 257..285 and distance codes 0..29.
const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
/// The order the code-length code lengths arrive in for a dynamic block (RFC 1951 §3.2.7).
const CLEN_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

const WINDOW: usize = 32 * 1024;

/// The 32 KiB history window plus the running gzip trailer state. Every produced byte passes through
/// here exactly once: into the window (so back-references can find it), into the CRC, and into the
/// caller's sink.
struct Window<'a, K: Sink> {
    buf: Box<[u8; WINDOW]>,
    pos: usize,
    produced: u64,
    crc: Crc32,
    sink: &'a mut K,
}

impl<'a, K: Sink> Window<'a, K> {
    fn new(sink: &'a mut K) -> Self {
        Self {
            buf: Box::new([0u8; WINDOW]),
            pos: 0,
            produced: 0,
            crc: Crc32::new(),
            sink,
        }
    }

    #[inline]
    fn emit(&mut self, b: u8) -> Result<(), InflateError> {
        self.buf[self.pos] = b;
        self.pos = (self.pos + 1) & (WINDOW - 1);
        self.produced += 1;
        self.crc.update(&[b]);
        self.sink.push(b).map_err(|_| InflateError::SinkRejected)
    }

    /// Copy `len` bytes from `dist` back in the history. Overlapping copies are correct and
    /// intentional (that is how DEFLATE encodes runs): each byte is emitted before the next is read.
    fn copy(&mut self, dist: usize, len: usize) -> Result<(), InflateError> {
        if dist == 0 || dist as u64 > self.produced || dist > WINDOW {
            return Err(InflateError::BadDistance);
        }
        for _ in 0..len {
            let b = self.buf[(self.pos + WINDOW - dist) & (WINDOW - 1)];
            self.emit(b)?;
        }
        Ok(())
    }
}

/// What a completed walk of one gzip member produced.
pub struct GzipReport {
    /// Decompressed byte count (checked against the gzip trailer's ISIZE, mod 2^32).
    pub uncompressed: u64,
    /// Compressed bytes consumed, including the header and trailer.
    pub compressed: u64,
    /// CRC-32 of the decompressed stream, checked against the trailer.
    pub crc: u32,
}

/// Decompress one gzip member from `src`, pushing every decompressed byte into `sink`.
///
/// The gzip trailer is CHECKED, not merely skipped: a decoder that silently produced garbage would
/// still be internally consistent, but it could not also reproduce the CRC-32 and length the packer
/// stamped. That check is what makes `files=` on the witness line a claim about the real tree rather
/// than about whatever this decoder happened to emit.
pub fn gunzip<S: ByteSource, K: Sink>(
    src: &mut S,
    sink: &mut K,
) -> Result<GzipReport, InflateError> {
    let mut br = BitReader::new(src);

    // --- RFC 1952 §2.3 header ---
    if br.byte()? != 0x1F || br.byte()? != 0x8B {
        return Err(InflateError::BadGzipHeader);
    }
    if br.byte()? != 0x08 {
        // CM: only DEFLATE is defined.
        return Err(InflateError::BadGzipHeader);
    }
    let flg = br.byte()?;
    for _ in 0..6 {
        br.byte()?; // MTIME(4) + XFL + OS
    }
    if flg & 0x04 != 0 {
        // FEXTRA
        let xlen = br.byte()? as u32 | ((br.byte()? as u32) << 8);
        for _ in 0..xlen {
            br.byte()?;
        }
    }
    if flg & 0x08 != 0 {
        // FNAME, NUL-terminated
        while br.byte()? != 0 {}
    }
    if flg & 0x10 != 0 {
        // FCOMMENT, NUL-terminated
        while br.byte()? != 0 {}
    }
    if flg & 0x02 != 0 {
        // FHCRC
        br.byte()?;
        br.byte()?;
    }

    // --- RFC 1951 deflate stream ---
    let mut win = Window::new(sink);
    loop {
        let last = br.bit()?;
        let btype = br.bits(2)?;
        match btype {
            0 => {
                br.align();
                let len = br.byte()? as u32 | ((br.byte()? as u32) << 8);
                let nlen = br.byte()? as u32 | ((br.byte()? as u32) << 8);
                if len != (!nlen & 0xFFFF) {
                    return Err(InflateError::BadBlock);
                }
                for _ in 0..len {
                    let b = br.byte()?;
                    win.emit(b)?;
                }
            }
            1 => {
                let (lit, dist) = fixed_tables()?;
                inflate_block(&mut br, &mut win, &lit, &dist)?;
            }
            2 => {
                let (lit, dist) = dynamic_tables(&mut br)?;
                inflate_block(&mut br, &mut win, &lit, &dist)?;
            }
            _ => return Err(InflateError::BadBlock),
        }
        if last == 1 {
            break;
        }
    }

    // --- RFC 1952 §2.3.1 trailer: CRC32 then ISIZE, both little-endian ---
    br.align();
    let produced = win.produced;
    let got_crc = win.crc.finish();
    let mut want_crc = 0u32;
    for i in 0..4 {
        want_crc |= (br.byte()? as u32) << (8 * i);
    }
    let mut want_isize = 0u32;
    for i in 0..4 {
        want_isize |= (br.byte()? as u32) << (8 * i);
    }
    if got_crc != want_crc || want_isize != (produced as u32) {
        return Err(InflateError::TrailerMismatch);
    }

    Ok(GzipReport {
        uncompressed: produced,
        compressed: br.consumed,
        crc: got_crc,
    })
}

fn fixed_tables() -> Result<(Huffman, Huffman), InflateError> {
    let mut lit = [0u8; 288];
    for (i, l) in lit.iter_mut().enumerate() {
        *l = match i {
            0..=143 => 8,
            144..=255 => 9,
            256..=279 => 7,
            _ => 8,
        };
    }
    // 32 five-bit distance codes, not 30: the fixed table is DEFINED over 32 slots (RFC 1951 §3.2.6),
    // and building it that way keeps it complete so the strict Kraft check above stays strict. Codes
    // 30 and 31 can never legally appear; `inflate_block` rejects them on decode.
    let dist = [5u8; 32];
    Ok((Huffman::build(&lit, false)?, Huffman::build(&dist, false)?))
}

fn dynamic_tables<S: ByteSource>(
    br: &mut BitReader<'_, S>,
) -> Result<(Huffman, Huffman), InflateError> {
    let hlit = br.bits(5)? as usize + 257;
    let hdist = br.bits(5)? as usize + 1;
    let hclen = br.bits(4)? as usize + 4;
    if hlit > 286 || hdist > 30 {
        return Err(InflateError::BadHuffmanTable);
    }

    let mut clen = [0u8; 19];
    for i in 0..hclen {
        clen[CLEN_ORDER[i]] = br.bits(3)? as u8;
    }
    let clen_tab = Huffman::build(&clen, false)?;

    // The code-length alphabet encodes the two real tables back to back.
    let total = hlit + hdist;
    let mut lengths = vec![0u8; total];
    let mut i = 0usize;
    while i < total {
        let sym = clen_tab.decode(br)?;
        match sym {
            0..=15 => {
                lengths[i] = sym as u8;
                i += 1;
            }
            16 => {
                // Repeat the PREVIOUS length 3..6 times — illegal as the first symbol.
                if i == 0 {
                    return Err(InflateError::BadHuffmanTable);
                }
                let prev = lengths[i - 1];
                let n = 3 + br.bits(2)? as usize;
                if i + n > total {
                    return Err(InflateError::BadHuffmanTable);
                }
                for _ in 0..n {
                    lengths[i] = prev;
                    i += 1;
                }
            }
            17 | 18 => {
                let n = if sym == 17 {
                    3 + br.bits(3)? as usize
                } else {
                    11 + br.bits(7)? as usize
                };
                if i + n > total {
                    return Err(InflateError::BadHuffmanTable);
                }
                i += n; // lengths are already 0
            }
            _ => return Err(InflateError::BadSymbol),
        }
    }

    let lit = Huffman::build(&lengths[..hlit], false)?;
    // A block that emits only literals legally carries a degenerate distance table.
    let dist = Huffman::build(&lengths[hlit..], true)?;
    Ok((lit, dist))
}

fn inflate_block<S: ByteSource, K: Sink>(
    br: &mut BitReader<'_, S>,
    win: &mut Window<'_, K>,
    lit: &Huffman,
    dist: &Huffman,
) -> Result<(), InflateError> {
    loop {
        let sym = lit.decode(br)?;
        match sym {
            0..=255 => win.emit(sym as u8)?,
            256 => return Ok(()), // end of block
            257..=285 => {
                let idx = (sym - 257) as usize;
                let len = LEN_BASE[idx] as usize + br.bits(LEN_EXTRA[idx] as u32)? as usize;
                let dsym = dist.decode(br)? as usize;
                if dsym >= 30 {
                    return Err(InflateError::BadSymbol);
                }
                let d = DIST_BASE[dsym] as usize + br.bits(DIST_EXTRA[dsym] as u32)? as usize;
                win.copy(d, len)?;
            }
            _ => return Err(InflateError::BadSymbol),
        }
    }
}

/// A human-readable reason for a FAIL line.
pub fn inflate_reason(e: InflateError) -> &'static str {
    match e {
        InflateError::TruncatedInput => "input ended mid-stream",
        InflateError::BadGzipHeader => "not a gzip/DEFLATE member",
        InflateError::BadBlock => "malformed deflate block",
        InflateError::BadHuffmanTable => "malformed huffman table",
        InflateError::BadSymbol => "undecodable symbol",
        InflateError::BadDistance => "back-reference past the window",
        InflateError::TrailerMismatch => "gzip trailer crc/isize mismatch",
        InflateError::SinkRejected => "tar walk rejected the stream",
    }
}
