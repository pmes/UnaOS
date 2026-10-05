// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Facet's PNG writer, from the specifications, no crate: W3C PNG 3rd edition (signature §5.2,
//! chunk layout §5.3, IHDR/sRGB/IDAT/IEND §11, filters §9 with the §12.8 minimum-sum-of-absolute-
//! differences heuristic), RFC 1950 (zlib wrapper, Adler-32), RFC 1951 (DEFLATE: fixed-Huffman
//! blocks §3.2.6 over an LZ77 hash-chain matcher), and the CRC-32 of PNG Annex D.
//!
//! Output is always 8-bit RGBA (colour type 6) tagged `sRGB` (perceptual): what Facet renders is
//! sRGB-encoded straight RGBA, and the tag says so instead of leaving a reader to assume.

/// The PNG/zlib CRC-32 (ISO 3309, reflected polynomial 0xEDB88320), PNG Annex D.
pub fn crc32(parts: &[&[u8]]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let t = TABLE.get_or_init(|| {
        let mut t = [0u32; 256];
        for (n, slot) in t.iter_mut().enumerate() {
            let mut c = n as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            }
            *slot = c;
        }
        t
    });
    let mut c = 0xFFFF_FFFFu32;
    for p in parts {
        for &b in *p {
            c = t[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
        }
    }
    c ^ 0xFFFF_FFFF
}

/// RFC 1950 §8.2 Adler-32.
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

/// Encode `width x height` straight RGBA (row-major, top-down) as a PNG file.
pub fn encode(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    assert_eq!(rgba.len(), width as usize * height as usize * 4, "rgba length must be w*h*4");
    let mut out = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend(width.to_be_bytes());
    ihdr.extend(height.to_be_bytes());
    ihdr.extend([8, 6, 0, 0, 0]); // depth 8, RGBA, deflate, adaptive filtering, no interlace
    chunk(&mut out, b"IHDR", &ihdr);
    chunk(&mut out, b"sRGB", &[0]);
    chunk(&mut out, b"IDAT", &zlib(&filter(width as usize, height as usize, rgba)));
    chunk(&mut out, b"IEND", &[]);
    out
}

fn chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend((data.len() as u32).to_be_bytes());
    out.extend(kind);
    out.extend(data);
    out.extend(crc32(&[kind, data]).to_be_bytes());
}

/// §9.2 Paeth predictor.
fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let p = a as i16 + b as i16 - c as i16;
    let (pa, pb, pc) = ((p - a as i16).abs(), (p - b as i16).abs(), (p - c as i16).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Filter every scanline, choosing per row the type with the smallest sum of absolute values of
/// the filtered bytes taken as signed (§12.8).
fn filter(w: usize, h: usize, rgba: &[u8]) -> Vec<u8> {
    let stride = w * 4;
    let mut out = Vec::with_capacity(h * (stride + 1));
    let zero = vec![0u8; stride];
    let mut cand = [vec![0u8; stride], vec![0u8; stride], vec![0u8; stride], vec![0u8; stride], vec![0u8; stride]];
    for y in 0..h {
        let row = &rgba[y * stride..(y + 1) * stride];
        let up = if y == 0 { &zero[..] } else { &rgba[(y - 1) * stride..y * stride] };
        for i in 0..stride {
            let a = if i >= 4 { row[i - 4] } else { 0 };
            let b = up[i];
            let c = if i >= 4 { up[i - 4] } else { 0 };
            let x = row[i];
            cand[0][i] = x;
            cand[1][i] = x.wrapping_sub(a);
            cand[2][i] = x.wrapping_sub(b);
            cand[3][i] = x.wrapping_sub(((a as u16 + b as u16) / 2) as u8);
            cand[4][i] = x.wrapping_sub(paeth(a, b, c));
        }
        let score = |v: &Vec<u8>| v.iter().map(|&b| (b as i8).unsigned_abs() as u32).sum::<u32>();
        let best = (0..5).min_by_key(|&k| score(&cand[k])).unwrap_or(0);
        out.push(best as u8);
        out.extend_from_slice(&cand[best]);
    }
    out
}

/// LSB-first bit packer (RFC 1951 §3.1.1).
struct Bits {
    out: Vec<u8>,
    acc: u64,
    n: u32,
}

impl Bits {
    fn put(&mut self, v: u32, len: u32) {
        self.acc |= (v as u64) << self.n;
        self.n += len;
        while self.n >= 8 {
            self.out.push(self.acc as u8);
            self.acc >>= 8;
            self.n -= 8;
        }
    }
    /// A Huffman code is defined MSB-first; reverse it into the LSB-first stream.
    fn code(&mut self, code: u32, len: u32) {
        self.put(code.reverse_bits() >> (32 - len), len);
    }
    fn finish(mut self) -> Vec<u8> {
        if self.n > 0 {
            self.out.push(self.acc as u8);
        }
        self.out
    }
}

/// §3.2.5 length codes 257..285: (base, extra bits).
const LEN_BASE: [(u16, u8); 29] = [
    (3, 0), (4, 0), (5, 0), (6, 0), (7, 0), (8, 0), (9, 0), (10, 0), (11, 1), (13, 1), (15, 1), (17, 1),
    (19, 2), (23, 2), (27, 2), (31, 2), (35, 3), (43, 3), (51, 3), (59, 3), (67, 4), (83, 4), (99, 4),
    (115, 4), (131, 5), (163, 5), (195, 5), (227, 5), (258, 0),
];
/// §3.2.5 distance codes 0..29: (base, extra bits).
const DIST_BASE: [(u16, u8); 30] = [
    (1, 0), (2, 0), (3, 0), (4, 0), (5, 1), (7, 1), (9, 2), (13, 2), (17, 3), (25, 3), (33, 4), (49, 4),
    (65, 5), (97, 5), (129, 6), (193, 6), (257, 7), (385, 7), (513, 8), (769, 8), (1025, 9), (1537, 9),
    (2049, 10), (3073, 10), (4097, 11), (6145, 11), (8193, 12), (12289, 12), (16385, 13), (24577, 13),
];

/// §3.2.6 the fixed literal/length code.
fn lit(bits: &mut Bits, v: u16) {
    match v {
        0..=143 => bits.code(0x30 + v as u32, 8),
        144..=255 => bits.code(0x190 + (v as u32 - 144), 9),
        256..=279 => bits.code(v as u32 - 256, 7),
        _ => bits.code(0xC0 + (v as u32 - 280), 8),
    }
}

fn emit_match(bits: &mut Bits, len: usize, dist: usize) {
    let li = LEN_BASE.iter().rposition(|&(b, _)| b as usize <= len).unwrap_or(0);
    let (lb, le) = LEN_BASE[li];
    lit(bits, 257 + li as u16);
    bits.put((len - lb as usize) as u32, le as u32);
    let di = DIST_BASE.iter().rposition(|&(b, _)| b as usize <= dist).unwrap_or(0);
    let (db, de) = DIST_BASE[di];
    bits.code(di as u32, 5); // fixed distance codes are 5-bit
    bits.put((dist - db as usize) as u32, de as u32);
}

/// RFC 1951 compressed with fixed-Huffman blocks (one final block) over a greedy LZ77 matcher
/// (32 KiB window, 3-byte hash, chains capped at 64 probes), wrapped in RFC 1950 zlib.
pub fn zlib(data: &[u8]) -> Vec<u8> {
    const WINDOW: usize = 32 * 1024;
    const HBITS: u32 = 15;
    let mut head = vec![usize::MAX; 1 << HBITS];
    let mut prev = vec![usize::MAX; data.len()];
    let hash = |i: usize| -> usize {
        let v = (data[i] as u32) << 16 | (data[i + 1] as u32) << 8 | data[i + 2] as u32;
        (v.wrapping_mul(0x9E37_79B1) >> (32 - HBITS)) as usize
    };
    let mut bits = Bits { out: vec![0x78, 0x01], acc: 0, n: 0 };
    bits.put(1, 1); // BFINAL
    bits.put(1, 2); // BTYPE = 01 fixed Huffman
    let mut i = 0;
    let insert = |head: &mut Vec<usize>, prev: &mut Vec<usize>, j: usize| {
        if j + 2 < data.len() {
            let h = hash(j);
            prev[j] = head[h];
            head[h] = j;
        }
    };
    while i < data.len() {
        let (mut best_len, mut best_dist) = (0usize, 0usize);
        if i + 2 < data.len() {
            let mut cand = head[hash(i)];
            let max = (data.len() - i).min(258);
            let mut probes = 0;
            while cand != usize::MAX && i - cand <= WINDOW && probes < 64 {
                let mut l = 0;
                while l < max && data[cand + l] == data[i + l] {
                    l += 1;
                }
                if l > best_len {
                    best_len = l;
                    best_dist = i - cand;
                    if l == max {
                        break;
                    }
                }
                cand = prev[cand];
                probes += 1;
            }
        }
        if best_len >= 3 {
            emit_match(&mut bits, best_len, best_dist);
            for j in i..i + best_len {
                insert(&mut head, &mut prev, j);
            }
            i += best_len;
        } else {
            lit(&mut bits, data[i] as u16);
            insert(&mut head, &mut prev, i);
            i += 1;
        }
    }
    lit(&mut bits, 256); // end of block
    let mut out = bits.finish();
    out.extend(adler32(data).to_be_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kat_crc32_and_adler32() {
        // The standard check values for the ASCII string "123456789".
        assert_eq!(crc32(&[b"123456789"]), 0xCBF4_3926);
        // RFC 1950: Adler-32 of "Wikipedia" is 0x11E60398.
        assert_eq!(adler32(b"Wikipedia"), 0x11E6_0398);
        assert_eq!(adler32(b""), 1);
        // The IEND chunk's CRC every PNG ends with.
        assert_eq!(crc32(&[b"IEND"]), 0xAE42_6082);
    }

    #[test]
    fn kat_zlib_header_and_empty_stream() {
        // Empty input: header, final fixed block holding only end-of-block (7 zero bits after the
        // 3 header bits: 0b011 then 0000000 -> bytes 0x03 0x00), Adler-32 of nothing = 1.
        assert_eq!(zlib(b""), vec![0x78, 0x01, 0x03, 0x00, 0, 0, 0, 1]);
        assert_eq!((0x78u32 * 256 + 0x01) % 31, 0, "FCHECK");
    }

    #[test]
    fn kat_paeth() {
        assert_eq!(paeth(10, 20, 10), 20);
        assert_eq!(paeth(20, 10, 10), 20);
        assert_eq!(paeth(10, 10, 20), 10);
    }

    #[test]
    fn png_layout() {
        let png = encode(2, 1, &[255, 0, 0, 255, 0, 0, 255, 128]);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
        assert_eq!(&png[12..16], b"IHDR");
        assert_eq!(&png[png.len() - 8..], &[b'I', b'E', b'N', b'D', 0xAE, 0x42, 0x60, 0x82]);
    }
}
