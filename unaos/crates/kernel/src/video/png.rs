// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! PNG-8/RGB encoder — SHOTZIP: fixed-Huffman deflate with LZ77, streamed in IDAT chunks.
//!
//! History: this module began as a STORED (BTYPE=00) writer, which is a length counter and a pair of
//! checksums and nothing else. Boot 17 measured its price on the rMBP: a 2880x1800 capture is
//! 15 555 053 bytes and the card took 44 s to take them ("i got the screenshot to open but it took
//! forever"). SHOTZIP replaces the body with a real compressor and keeps the contract: a pure
//! function of its inputs, `alloc` only, no kernel dependencies (host-testable by `include!`).
//!
//! ## The compressor
//!
//!  * **Filter.** PNG filter type 1 (Sub) on every row: `x[i] - x[i-3]` per byte (the previous
//!    PIXEL's same channel). A desktop is flat fills, gradients and text; Sub turns a flat fill into
//!    a run of zero bytes and a linear gradient into a run of one constant, which is exactly what
//!    LZ77 at distance 1 eats at 258 bytes per ~13 bits. It costs one subtract per byte and is a
//!    fixed choice (no per-row heuristic: the heuristic would need 5 candidate rows, and Sub alone
//!    gets nearly all of the desktop win).
//!  * **LZ77.** One 32 KiB window over the FILTERED stream (rows are not special: an identical row
//!    matches the previous one at distance `1 + 3*width` only if that is <= 32 KiB, i.e. up to
//!    width 10 922; wider panels still match inside a row). Matches are found by a 3-byte
//!    multiplicative hash into `head[1 << 15]` plus a `prev[32768]` chain, walked at most
//!    [`MAX_CHAIN`] links, stopped early at [`NICE_LEN`]. Greedy (no lazy matching): lazy buys ~3%
//!    and doubles the probing.
//!  * **Entropy coding.** ONE fixed-Huffman block (BTYPE=01, RFC 1951 §3.2.6) for the whole image.
//!    No tree is built, so there is nothing to buffer and the block can run for the whole stream
//!    while the output is cut into IDAT chunks of at most [`CHUNK`] bytes as it is produced.
//!  * **Memory.** 64 KiB ring (32 KiB window + lookahead) + 128 KiB `head` + 128 KiB `prev` + one
//!    row + about 80 KiB of pending output. Under 0.5 MiB and constant in the image size, where the
//!    stored writer reserved the whole 15 MiB.
//!
//! ## The self-check
//!
//! [`PngEncoder`] feeds every IDAT payload it emits to a small streaming fixed-Huffman inflater
//! ([`Verify`]) that rebuilds the filtered bytes, Adler-32s them and compares the length and the
//! trailer with what the encoder consumed. `verified()` is the `PASS` of the `:: SHOTZIP:` line: the
//! stream the card holds decodes to the bytes that went in. (`selfhost::inflate` is the tree's
//! other inflater but it is feature-gated and wants the whole stream behind a pull source; a
//! streaming encoder has no whole stream to hand it.)
//!
//! This module has **no kernel dependencies** — only `alloc`.

use alloc::vec::Vec;

/// The eight-byte PNG signature (`\x89PNG\r\n\x1a\n`).
const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

/// Largest payload a single stored deflate block may carry (`LEN` is a `u16`).
const MAX_STORED: usize = 65535;

/// Largest number of bytes Adler-32 can absorb before `b` could overflow `u32`. The classic
/// zlib `NMAX`: the biggest `n` with `255*n*(n+1)/2 + (n+1)*(BASE-1) <= 2^32-1`.
const ADLER_NMAX: usize = 5552;

/// Adler-32 modulus — the largest prime below 65536.
const ADLER_BASE: u32 = 65521;

/// CRC-32 table for the reflected polynomial `0xEDB88320`, built at compile time so the kernel
/// image carries 1 KiB of `.rodata` instead of a lazy initialiser and a lock.
const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut i = 0usize;
    while i < 256 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        table[i] = c;
        i += 1;
    }
    table
};

/// CRC-32 as PNG defines it (ISO 3309 / ITU-T V.42): init all-ones, reflected, final complement.
pub fn crc32(data: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = CRC_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

/// Adler-32 (`RFC 1950` §9), batched so the two modulos run once per `ADLER_NMAX` bytes rather
/// than once per byte — the difference between a few milliseconds and a few seconds over 15 MiB.
pub fn adler32(data: &[u8]) -> u32 {
    let mut state = Adler::new();
    state.update(data);
    state.finish()
}

/// Running Adler-32 state, so the encoder can checksum scanlines as they stream past.
struct Adler {
    a: u32,
    b: u32,
}

impl Adler {
    const fn new() -> Self {
        Self { a: 1, b: 0 }
    }

    fn update(&mut self, data: &[u8]) {
        for chunk in data.chunks(ADLER_NMAX) {
            for &byte in chunk {
                self.a += byte as u32;
                self.b += self.a;
            }
            self.a %= ADLER_BASE;
            self.b %= ADLER_BASE;
        }
    }

    const fn finish(&self) -> u32 {
        (self.b << 16) | self.a
    }
}

/// Why an encoder could not be built or fed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PngError {
    /// Zero width or height — there is no such thing as a 0-pixel PNG.
    EmptyImage,
    /// The image's size does not fit a `usize`/PNG's `u32` chunk length.
    TooLarge,
    /// The allocator declined the window/hash tables. Reported before any pixel is read.
    OutOfMemory,
    /// A pushed scanline was not exactly `width * 3` bytes.
    BadRowLength,
    /// More scanlines were pushed than the declared height, or `finish` came early.
    RowCountMismatch,
}

/// Largest IDAT payload one emitted chunk carries.
pub const CHUNK: usize = 64 * 1024;
/// LZ77 window (RFC 1951 maximum distance).
const WIN: u32 = 32768;
const WMASK: u32 = WIN - 1;
/// Ring holding the window plus the lookahead; a power of two so a position maps by a mask.
const RING: usize = 65536;
const RMASK: u32 = (RING as u32) - 1;
const HASH_BITS: u32 = 15;
/// Hash-chain links followed per position. The speed/ratio knob: 32 is the brief's bound.
pub const MAX_CHAIN: u32 = 32;
/// A match this long ends the chain walk.
const NICE_LEN: u32 = 128;
const MIN_MATCH: u32 = 3;
const MAX_MATCH: u32 = 258;
/// Bytes copied into the ring between two compress passes (keeps `wr - pos` small).
const FEED: usize = 4096;

const fn rev(code: u32, n: u32) -> u32 {
    let mut r = 0;
    let mut i = 0;
    while i < n {
        if code & (1 << i) != 0 {
            r |= 1 << (n - 1 - i);
        }
        i += 1;
    }
    r
}

/// Fixed literal/length code per symbol (RFC 1951 §3.2.6), already bit-reversed for the LSB-first
/// packer: `(reversed code, bit length)`.
const FIXED_LIT: [(u16, u8); 288] = {
    let mut t = [(0u16, 0u8); 288];
    let mut s = 0usize;
    while s < 288 {
        let (code, n) = if s < 144 {
            (0x30 + s as u32, 8)
        } else if s < 256 {
            (0x190 + (s as u32 - 144), 9)
        } else if s < 280 {
            (s as u32 - 256, 7)
        } else {
            (0xC0 + (s as u32 - 280), 8)
        };
        t[s] = (rev(code, n) as u16, n as u8);
        s += 1;
    }
    t
};

/// Fixed distance code: 5 bits, MSB-first, so reversed.
const FIXED_DIST: [u8; 32] = {
    let mut t = [0u8; 32];
    let mut s = 0usize;
    while s < 32 {
        t[s] = rev(s as u32, 5) as u8;
        s += 1;
    }
    t
};

/// A streaming truecolour-8 PNG writer. Build with [`PngEncoder::new`], push exactly `height`
/// scanlines of `width * 3` RGB bytes, call [`finish`](PngEncoder::finish), and between pushes
/// drain finished file bytes with [`next_piece`](PngEncoder::next_piece).
pub struct PngEncoder {
    width: u32,
    height: u32,
    rows_pushed: u32,
    ring: Vec<u8>,
    head: Vec<u32>,
    prev: Vec<u32>,
    /// Stream position one past the last byte copied into the ring.
    wr: u32,
    /// Stream position of the next byte to code.
    pos: u32,
    bitbuf: u64,
    nbits: u32,
    /// Compressed zlib bytes not yet cut into an IDAT chunk.
    pend: Vec<u8>,
    filt: Vec<u8>,
    adler: Adler,
    head_sent: bool,
    finished: bool,
    iend_sent: bool,
    /// Filtered bytes consumed (the zlib stream's uncompressed length).
    raw_in: u64,
    /// Zlib stream bytes emitted into IDAT chunks so far.
    z_out: u64,
    /// IDAT chunks emitted so far.
    chunks: u32,
    verify: Verify,
}

impl PngEncoder {
    /// The WORST-CASE-by-stored-size byte length for `width` x `height` (what the old writer always
    /// produced). Kept as the bound a refusal quotes; the real size is `deflated + 69-ish`.
    /// `None` on a zero dimension or an arithmetic overflow.
    pub fn encoded_len(width: u32, height: u32) -> Option<usize> {
        if width == 0 || height == 0 {
            return None;
        }
        let row = (width as usize).checked_mul(3)?.checked_add(1)?;
        let raw = row.checked_mul(height as usize)?;
        let blocks = raw.div_ceil(MAX_STORED);
        let zlib = raw.checked_add(blocks.checked_mul(5)?)?.checked_add(6)?;
        if zlib > 0x7FFF_FFFF {
            return None;
        }
        zlib.checked_add(57)
    }

    /// Start an encoder. All of its memory is taken here, so `Err(OutOfMemory)` means no pixel was
    /// ever read.
    pub fn new(width: u32, height: u32) -> Result<Self, PngError> {
        let total = match PngEncoder::encoded_len(width, height) {
            Some(n) => n,
            None if width == 0 || height == 0 => return Err(PngError::EmptyImage),
            None => return Err(PngError::TooLarge),
        };
        let _ = total;
        let row = width as usize * 3 + 1;
        let mut ring: Vec<u8> = Vec::new();
        let mut head: Vec<u32> = Vec::new();
        let mut prev: Vec<u32> = Vec::new();
        let mut pend: Vec<u8> = Vec::new();
        let mut filt: Vec<u8> = Vec::new();
        if ring.try_reserve_exact(RING).is_err()
            || head.try_reserve_exact(1 << HASH_BITS).is_err()
            || prev.try_reserve_exact(WIN as usize).is_err()
            || pend.try_reserve_exact(CHUNK + 2 * row + 4096).is_err()
            || filt.try_reserve_exact(row).is_err()
        {
            return Err(PngError::OutOfMemory);
        }
        ring.resize(RING, 0);
        head.resize(1 << HASH_BITS, 0);
        prev.resize(WIN as usize, 0);
        filt.resize(row, 0);
        let verify = Verify::new().ok_or(PngError::OutOfMemory)?;
        // zlib header: CMF 0x78 (deflate, 32 KiB window) + FLG 0x01; (0x78<<8|0x01) % 31 == 0.
        pend.extend_from_slice(&[0x78, 0x01]);
        let mut enc = Self {
            width,
            height,
            rows_pushed: 0,
            ring,
            head,
            prev,
            wr: 0,
            pos: 0,
            bitbuf: 0,
            nbits: 0,
            pend,
            filt,
            adler: Adler::new(),
            head_sent: false,
            finished: false,
            iend_sent: false,
            raw_in: 0,
            z_out: 0,
            chunks: 0,
            verify,
        };
        // One block for the whole stream: BFINAL=1, BTYPE=01 (fixed Huffman). LSB-first: 1, 1, 0.
        enc.put(0b011, 3);
        Ok(enc)
    }

    #[inline]
    fn put(&mut self, val: u32, n: u32) {
        self.bitbuf |= (val as u64) << self.nbits;
        self.nbits += n;
        while self.nbits >= 8 {
            self.pend.push(self.bitbuf as u8);
            self.bitbuf >>= 8;
            self.nbits -= 8;
        }
    }

    #[inline]
    fn put_sym(&mut self, sym: usize) {
        let (c, n) = FIXED_LIT[sym];
        self.put(c as u32, n as u32);
    }

    #[inline]
    fn hash(&self, p: u32) -> usize {
        let r = &self.ring;
        let v = (r[(p & RMASK) as usize] as u32) << 16
            | (r[((p + 1) & RMASK) as usize] as u32) << 8
            | r[((p + 2) & RMASK) as usize] as u32;
        (v.wrapping_mul(0x9E37_79B1) >> (32 - HASH_BITS)) as usize
    }

    #[inline]
    fn insert(&mut self, p: u32, h: usize) {
        self.prev[(p & WMASK) as usize] = self.head[h];
        self.head[h] = p + 1;
    }

    fn match_len(&self, c: u32, p: u32, maxlen: u32) -> u32 {
        let r = &self.ring;
        let mut l = 0u32;
        while l < maxlen && r[((c + l) & RMASK) as usize] == r[((p + l) & RMASK) as usize] {
            l += 1;
        }
        l
    }

    fn emit_match(&mut self, len: u32, dist: u32) {
        if len == MAX_MATCH {
            self.put_sym(285);
        } else {
            let n = len - 3;
            if n < 8 {
                self.put_sym(257 + n as usize);
            } else {
                let nb = 31 - n.leading_zeros();
                let eb = nb - 2;
                self.put_sym(257 + 4 * (nb as usize - 1) + ((n >> eb) & 3) as usize);
                self.put(n & ((1 << eb) - 1), eb);
            }
        }
        let n = dist - 1;
        if n < 4 {
            self.put(FIXED_DIST[n as usize] as u32, 5);
        } else {
            let nb = 31 - n.leading_zeros();
            let eb = nb - 1;
            let sym = 2 * nb + ((n >> eb) & 1);
            self.put(FIXED_DIST[sym as usize] as u32, 5);
            self.put(n & ((1 << eb) - 1), eb);
        }
    }

    /// Code everything that has enough lookahead (all of it when `flush`).
    fn compress(&mut self, flush: bool) {
        let need = if flush { 1 } else { MAX_MATCH };
        while self.wr - self.pos >= need {
            let p = self.pos;
            let avail = self.wr - p;
            let mut best_len = 0u32;
            let mut best_dist = 0u32;
            if avail >= MIN_MATCH {
                let h = self.hash(p);
                let maxlen = if avail < MAX_MATCH { avail } else { MAX_MATCH };
                let mut cand = self.head[h];
                let mut chain = MAX_CHAIN;
                while cand != 0 && chain > 0 {
                    let c = cand - 1;
                    if c >= p || p - c > WIN {
                        break;
                    }
                    let skip = best_len > 0
                        && self.ring[((c + best_len) & RMASK) as usize]
                            != self.ring[((p + best_len) & RMASK) as usize];
                    if !skip {
                        let l = self.match_len(c, p, maxlen);
                        if l > best_len {
                            best_len = l;
                            best_dist = p - c;
                            if l >= NICE_LEN || l >= maxlen {
                                break;
                            }
                        }
                    }
                    let nx = self.prev[(c & WMASK) as usize];
                    if nx == 0 || nx - 1 >= c {
                        break;
                    }
                    cand = nx;
                    chain -= 1;
                }
                self.insert(p, h);
            }
            // A 3-byte match from far away costs more bits than its three literals (zlib's TOO_FAR).
            if best_len == MIN_MATCH && best_dist > 4096 {
                best_len = 0;
            }
            if best_len >= MIN_MATCH {
                self.emit_match(best_len, best_dist);
                let mut q = p + 1;
                let end = p + best_len;
                while q < end {
                    if q + 3 <= self.wr {
                        let h = self.hash(q);
                        self.insert(q, h);
                    }
                    q += 1;
                }
                self.pos = end;
            } else {
                let b = self.ring[(p & RMASK) as usize];
                self.put_sym(b as usize);
                self.pos = p + 1;
            }
        }
    }

    fn feed(&mut self, data: &[u8]) {
        for piece in data.chunks(FEED) {
            for &b in piece {
                self.ring[(self.wr & RMASK) as usize] = b;
                self.wr += 1;
            }
            self.adler.update(piece);
            self.raw_in += piece.len() as u64;
            self.compress(false);
        }
    }

    /// Push one scanline: exactly `width * 3` bytes, R,G,B per pixel, top row first.
    pub fn push_row(&mut self, rgb: &[u8]) -> Result<(), PngError> {
        if rgb.len() != self.width as usize * 3 {
            return Err(PngError::BadRowLength);
        }
        if self.rows_pushed >= self.height {
            return Err(PngError::RowCountMismatch);
        }
        let mut f = core::mem::take(&mut self.filt);
        f[0] = 1; // filter type 1: Sub
        for i in 0..rgb.len() {
            f[1 + i] = if i >= 3 { rgb[i].wrapping_sub(rgb[i - 3]) } else { rgb[i] };
        }
        self.feed(&f);
        self.filt = f;
        self.rows_pushed += 1;
        Ok(())
    }

    /// Close the deflate block and the zlib stream. Remaining bytes are drained by `next_piece`.
    pub fn finish(&mut self) -> Result<(), PngError> {
        if self.rows_pushed != self.height {
            return Err(PngError::RowCountMismatch);
        }
        if self.finished {
            return Ok(());
        }
        self.compress(true);
        self.put_sym(256);
        if self.nbits > 0 {
            self.pend.push(self.bitbuf as u8);
            self.bitbuf = 0;
            self.nbits = 0;
        }
        let a = self.adler.finish();
        self.pend.extend_from_slice(&a.to_be_bytes());
        self.finished = true;
        Ok(())
    }

    /// Is a full IDAT chunk (or, once finished, anything at all) waiting to be drained?
    pub fn ready(&self) -> bool {
        !self.head_sent || self.pend.len() >= CHUNK || self.finished
    }

    /// Cut the next complete PNG piece into `out` (cleared first): the signature + IHDR, then IDAT
    /// chunks of at most [`CHUNK`] payload bytes, then IEND. `false` = nothing to emit now.
    pub fn next_piece(&mut self, out: &mut Vec<u8>) -> bool {
        out.clear();
        if !self.head_sent {
            out.extend_from_slice(&SIGNATURE);
            let mut ihdr = [0u8; 13];
            ihdr[0..4].copy_from_slice(&self.width.to_be_bytes());
            ihdr[4..8].copy_from_slice(&self.height.to_be_bytes());
            ihdr[8] = 8;
            ihdr[9] = 2;
            push_chunk(out, b"IHDR", &ihdr);
            self.head_sent = true;
            return true;
        }
        if self.pend.len() >= CHUNK || (self.finished && !self.pend.is_empty()) {
            let n = core::cmp::min(CHUNK, self.pend.len());
            let last = self.finished && n == self.pend.len();
            self.verify.feed(&self.pend[..n], last);
            push_chunk(out, b"IDAT", &self.pend[..n]);
            self.pend.drain(..n);
            self.z_out += n as u64;
            self.chunks += 1;
            return true;
        }
        if self.finished && !self.iend_sent {
            push_chunk(out, b"IEND", &[]);
            self.iend_sent = true;
            return true;
        }
        false
    }

    /// `finish` has closed the zlib stream.
    pub fn finished(&self) -> bool {
        self.finished
    }
    /// Everything has been emitted (IEND included).
    pub fn done(&self) -> bool {
        self.iend_sent
    }
    /// Filtered bytes the zlib stream carries (what a stored writer would have stored).
    pub fn raw_len(&self) -> u64 {
        self.raw_in
    }
    /// Zlib stream bytes emitted into IDAT chunks.
    pub fn deflated_len(&self) -> u64 {
        self.z_out
    }
    /// IDAT chunks emitted.
    pub fn chunks(&self) -> u32 {
        self.chunks
    }
    /// The streamed inflate round trip matched (length + Adler-32 + clean EOB).
    pub fn verified(&self) -> bool {
        self.verify.ok(self.raw_in, self.adler.finish())
    }
}

/// Append a complete PNG chunk: length, type, data, CRC over type+data.
fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut c = 0xFFFF_FFFFu32;
    for &b in kind.iter().chain(data.iter()) {
        c = CRC_TABLE[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    out.extend_from_slice(&(c ^ 0xFFFF_FFFF).to_be_bytes());
}

// ───────────────────────── the streaming self-check (fixed-Huffman inflate) ─────────────────────────

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LEN_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];

/// Streaming decoder for exactly the stream [`PngEncoder`] writes (zlib header, ONE fixed block,
/// Adler trailer). Resumable: a token is decoded only when 32 bits are in hand (a token is at most
/// 31), unless it is fed the last piece.
struct Verify {
    inb: Vec<u8>,
    bitpos: usize,
    win: Vec<u8>,
    out: u64,
    /// Adler-32 running sums and the bytes since the last modulo (per-byte, NMAX-lazy).
    a: u32,
    b: u32,
    n: u32,
    /// 0 zlib header, 1 block header, 2 tokens, 3 trailer, 4 done, 5 failed
    state: u8,
    want: u32,
}

impl Verify {
    fn new() -> Option<Self> {
        let mut win: Vec<u8> = Vec::new();
        let mut inb: Vec<u8> = Vec::new();
        if win.try_reserve_exact(WIN as usize).is_err() || inb.try_reserve_exact(CHUNK + 64).is_err() {
            return None;
        }
        win.resize(WIN as usize, 0);
        Some(Self { inb, bitpos: 0, win, out: 0, a: 1, b: 0, n: 0, state: 0, want: 0 })
    }

    fn avail(&self) -> usize {
        (self.inb.len() * 8).saturating_sub(self.bitpos)
    }

    fn bits(&mut self, n: u32) -> u32 {
        let mut v = 0u32;
        for i in 0..n {
            if (self.bitpos >> 3) >= self.inb.len() {
                self.state = 5;
                return 0;
            }
            let b = (self.inb[self.bitpos >> 3] >> (self.bitpos & 7)) & 1;
            v |= (b as u32) << i;
            self.bitpos += 1;
        }
        v
    }

    fn emit(&mut self, b: u8) {
        self.win[(self.out as usize) & (WIN as usize - 1)] = b;
        self.out += 1;
        self.a += b as u32;
        self.b += self.a;
        self.n += 1;
        if self.n == ADLER_NMAX as u32 {
            self.a %= ADLER_BASE;
            self.b %= ADLER_BASE;
            self.n = 0;
        }
    }

    fn feed(&mut self, data: &[u8], last: bool) {
        if self.state >= 4 {
            return;
        }
        self.inb.extend_from_slice(data);
        loop {
            match self.state {
                0 => {
                    if self.avail() < 16 {
                        break;
                    }
                    let cmf = self.bits(8);
                    let flg = self.bits(8);
                    self.state = if cmf & 0x0F == 8 && ((cmf << 8) | flg) % 31 == 0 { 1 } else { 5 };
                }
                1 => {
                    if self.avail() < 3 {
                        break;
                    }
                    let h = self.bits(3);
                    self.state = if h == 0b011 { 2 } else { 5 };
                }
                2 => {
                    if self.avail() < 32 && !last {
                        break;
                    }
                    if self.avail() < 9 {
                        self.state = 5;
                        break;
                    }
                    // Fixed literal/length code, read MSB-first a bit at a time.
                    let mut code = 0u32;
                    for _ in 0..7 {
                        code = (code << 1) | self.bits(1);
                    }
                    let sym: u32;
                    if code <= 0x17 {
                        sym = 256 + code;
                    } else {
                        code = (code << 1) | self.bits(1);
                        if (0x30..=0xBF).contains(&code) {
                            sym = code - 0x30;
                        } else if (0xC0..=0xC7).contains(&code) {
                            sym = 280 + code - 0xC0;
                        } else {
                            code = (code << 1) | self.bits(1);
                            if (0x190..=0x1FF).contains(&code) {
                                sym = 144 + code - 0x190;
                            } else {
                                self.state = 5;
                                break;
                            }
                        }
                    }
                    if sym < 256 {
                        self.emit(sym as u8);
                    } else if sym == 256 {
                        self.bitpos = (self.bitpos + 7) & !7;
                        self.state = 3;
                    } else if sym <= 285 {
                        let li = (sym - 257) as usize;
                        let len = LEN_BASE[li] as u32 + self.bits(LEN_EXTRA[li] as u32);
                        let mut dc = 0u32;
                        for _ in 0..5 {
                            dc = (dc << 1) | self.bits(1);
                        }
                        if dc >= 30 {
                            self.state = 5;
                            break;
                        }
                        let dist = if dc < 4 {
                            dc + 1
                        } else {
                            let eb = dc / 2 - 1;
                            let base = ((2 + (dc & 1)) << eb) + 1;
                            base + self.bits(eb)
                        };
                        if dist as u64 > self.out {
                            self.state = 5;
                            break;
                        }
                        for _ in 0..len {
                            let b = self.win[((self.out - dist as u64) as usize) & (WIN as usize - 1)];
                            self.emit(b);
                        }
                    } else {
                        self.state = 5;
                        break;
                    }
                }
                3 => {
                    if self.avail() < 32 {
                        if last {
                            self.state = 5;
                        }
                        break;
                    }
                    let mut w = 0u32;
                    for _ in 0..4 {
                        w = (w << 8) | self.bits(8);
                    }
                    self.want = w;
                    self.state = 4;
                }
                _ => break,
            }
        }
        // Drop consumed whole bytes so the carry stays one chunk wide.
        let gone = self.bitpos >> 3;
        if gone > 0 {
            self.inb.drain(..gone);
            self.bitpos -= gone * 8;
        }
        if last && self.state < 4 {
            self.state = 5;
        }
    }

    fn ok(&self, raw_in: u64, enc_adler: u32) -> bool {
        self.state == 4 && self.out == raw_in && (((self.b % ADLER_BASE) << 16) | (self.a % ADLER_BASE)) == self.want && self.want == enc_adler
    }
}
