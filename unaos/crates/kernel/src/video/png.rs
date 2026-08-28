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

//! PNG-8/RGB encoder — stored (uncompressed) deflate, no compression dependency.
//!
//! PRTSCR needs a *valid* PNG on the FAT volume, not a small one. A DEFLATE compressor is a
//! dependency and a working-set; the format does not require one. `RFC 1951` §3.2.4 defines
//! BTYPE=00 "stored" blocks — a 5-byte header (`BFINAL`/`BTYPE`, `LEN`, `~LEN`) followed by
//! `LEN` literal bytes — and a zlib stream (`RFC 1950`) made entirely of stored blocks is a
//! legal zlib stream that every decoder accepts. So the whole "compressor" here is a length
//! counter, and the only real arithmetic is the two checksums the containers demand:
//! CRC-32 for each PNG chunk and Adler-32 over the zlib payload. Both are ~20 lines.
//!
//! The cost is honest and bounded: `1 + width*3` bytes per scanline (filter byte 0 = None,
//! then RGB triples) plus 5 bytes per 65535, so a 2880x1800 capture is ~15.5 MiB on disk.
//! Compression is explicitly out of scope; if it is ever wanted, it drops in behind this same
//! streaming API without the capture path noticing.
//!
//! ## Why the encoder streams, and why it owns exactly one buffer
//!
//! The panel can be 2880x1800x4 ≈ 20 MiB. Encoding "read the whole frame, then encode it"
//! would hold the frame copy *and* the encoded output at once — ~35 MiB of simultaneous heap
//! on a machine whose x86 heap is 256 MiB and whose desktop already eats a good share of it
//! (see `allocator::HEAP_SIZE`'s note on the GR27 famine). Instead the caller pushes one
//! scanline at a time and the encoder appends straight into its single output buffer, which is
//! `try_reserve_exact`ed to its FINAL size up front — so there is one allocation, no doubling
//! spike, and an out-of-memory answer arrives before any pixel is read rather than halfway
//! through. The frame copy shrinks to one row.
//!
//! Every block length is known before its header is written because the total raw size is
//! `height * (1 + width*3)`, fixed at construction. That is what makes single-buffer streaming
//! possible at all: a stored block must state its length up front, and here it always can.
//!
//! This module has **no kernel dependencies** — only `alloc`. That is deliberate: it makes the
//! encoder a pure function of its inputs, testable on the host (see the arc's host harness,
//! which `include!`s this file and decodes the result with python3's `zlib`).

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
    /// The image's encoded size does not fit a `usize`/PNG's `u32` chunk length.
    TooLarge,
    /// The allocator declined the output buffer. Reported before any pixel is read.
    OutOfMemory,
    /// A pushed scanline was not exactly `width * 3` bytes.
    BadRowLength,
    /// More scanlines were pushed than the declared height, or `finish` came early.
    RowCountMismatch,
}

/// A streaming truecolour-8 PNG writer. Build with [`PngEncoder::new`], push exactly `height`
/// scanlines of `width * 3` RGB bytes, then [`finish`](PngEncoder::finish).
pub struct PngEncoder {
    out: Vec<u8>,
    width: u32,
    height: u32,
    /// Index of the IDAT chunk's 4-byte length field, patched in `finish`.
    idat_len_at: usize,
    /// Raw (pre-deflate) bytes still owed to the zlib stream. Every stored block's length is
    /// derived from this, which is why the header can be written before the data exists.
    raw_left: usize,
    /// Bytes still owed to the stored block currently open.
    block_left: usize,
    adler: Adler,
    rows_pushed: u32,
}

impl PngEncoder {
    /// The exact byte length the finished PNG will have, for `width` x `height` RGB8.
    /// `None` on a zero dimension or an arithmetic overflow.
    pub fn encoded_len(width: u32, height: u32) -> Option<usize> {
        if width == 0 || height == 0 {
            return None;
        }
        let row = (width as usize).checked_mul(3)?.checked_add(1)?;
        let raw = row.checked_mul(height as usize)?;
        // Stored blocks: ceil(raw / 65535) of them, 5 bytes of header each.
        let blocks = raw.div_ceil(MAX_STORED);
        // zlib: 2-byte header + blocks + 4-byte Adler-32.
        let zlib = raw.checked_add(blocks.checked_mul(5)?)?.checked_add(6)?;
        // A PNG chunk length field is a u32 and must not exceed 2^31-1.
        if zlib > 0x7FFF_FFFF {
            return None;
        }
        // signature 8 + IHDR 25 + IDAT (12 + zlib) + IEND 12.
        zlib.checked_add(57)
    }

    /// Start an encoder, reserving the whole output up front. `Err(OutOfMemory)` here means no
    /// pixel was ever read — the refusal is cheap and total.
    pub fn new(width: u32, height: u32) -> Result<Self, PngError> {
        let total = match PngEncoder::encoded_len(width, height) {
            Some(n) => n,
            None if width == 0 || height == 0 => return Err(PngError::EmptyImage),
            None => return Err(PngError::TooLarge),
        };
        let mut out = Vec::new();
        if out.try_reserve_exact(total).is_err() {
            return Err(PngError::OutOfMemory);
        }
        let raw = (width as usize * 3 + 1) * height as usize;

        out.extend_from_slice(&SIGNATURE);

        // IHDR: 13 bytes — width, height, bit depth 8, colour type 2 (truecolour), compression
        // method 0 (deflate), filter method 0, interlace 0 (none).
        let mut ihdr = [0u8; 13];
        ihdr[0..4].copy_from_slice(&width.to_be_bytes());
        ihdr[4..8].copy_from_slice(&height.to_be_bytes());
        ihdr[8] = 8;
        ihdr[9] = 2;
        ihdr[10] = 0;
        ihdr[11] = 0;
        ihdr[12] = 0;
        push_chunk(&mut out, b"IHDR", &ihdr);

        // IDAT header: the length is not known until `finish` patches it, but it is *bounded*
        // by the reservation above, so nothing can move underneath it.
        let idat_len_at = out.len();
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(b"IDAT");
        // zlib header: CMF 0x78 (deflate, 32 KiB window) + FLG 0x01 (no dict, fastest level),
        // chosen so (CMF<<8 | FLG) % 31 == 0 as RFC 1950 §2.2 requires.
        out.extend_from_slice(&[0x78, 0x01]);

        let mut enc = Self {
            out,
            width,
            height,
            idat_len_at,
            raw_left: raw,
            block_left: 0,
            adler: Adler::new(),
            rows_pushed: 0,
        };
        enc.open_block();
        Ok(enc)
    }

    /// Open the next stored block, stating its length up front. Safe to do because `raw_left`
    /// is exact from construction.
    fn open_block(&mut self) {
        let n = if self.raw_left > MAX_STORED { MAX_STORED } else { self.raw_left };
        let final_block = self.raw_left <= MAX_STORED;
        self.out.push(if final_block { 1 } else { 0 });
        let len = n as u16;
        self.out.extend_from_slice(&len.to_le_bytes());
        self.out.extend_from_slice(&(!len).to_le_bytes());
        self.block_left = n;
    }

    /// Append raw (pre-deflate) bytes, splitting across stored blocks as needed.
    fn push_raw(&mut self, mut data: &[u8]) {
        self.adler.update(data);
        while !data.is_empty() {
            if self.block_left == 0 {
                self.open_block();
            }
            let take = if data.len() < self.block_left { data.len() } else { self.block_left };
            self.out.extend_from_slice(&data[..take]);
            self.block_left -= take;
            self.raw_left -= take;
            data = &data[take..];
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
        // Filter type 0 (None). Filters buy compression ratio; with stored blocks there is
        // nothing to buy, so the cheapest legal filter is the right one.
        self.push_raw(&[0u8]);
        self.push_raw(rgb);
        self.rows_pushed += 1;
        Ok(())
    }

    /// Close the zlib stream, patch the IDAT length, append IEND, and yield the file bytes.
    pub fn finish(mut self) -> Result<Vec<u8>, PngError> {
        if self.rows_pushed != self.height {
            return Err(PngError::RowCountMismatch);
        }
        self.out.extend_from_slice(&self.adler.finish().to_be_bytes());

        // The IDAT payload runs from just past its 4-byte type field to here.
        let data_at = self.idat_len_at + 8;
        let payload = self.out.len() - data_at;
        self.out[self.idat_len_at..self.idat_len_at + 4]
            .copy_from_slice(&(payload as u32).to_be_bytes());
        let crc = crc32(&self.out[self.idat_len_at + 4..]);
        self.out.extend_from_slice(&crc.to_be_bytes());

        push_chunk(&mut self.out, b"IEND", &[]);
        Ok(self.out)
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
