// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The boolean entropy decoder (RFC 6386 §7).
//!
//! `value` holds the not-yet-consumed bits left-aligned in a 64-bit window; its top byte plays the
//! role of the spec's two-byte `value` compared against `split << 8` (the comparison only ever looks
//! at the high byte, so a wider window is the same arithmetic with fewer refills). Reading past the
//! end of the partition feeds zero bytes, as the reference decoder does.

pub struct BoolDecoder<'a> {
    data: &'a [u8],
    pos: usize,
    value: u64,
    bits: i32,
    range: u32,
}

impl<'a> BoolDecoder<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        let mut d = BoolDecoder { data, pos: 0, value: 0, bits: 0, range: 255 };
        d.fill();
        d
    }

    #[inline]
    fn fill(&mut self) {
        while self.bits <= 56 {
            let b = if self.pos < self.data.len() { self.data[self.pos] } else { 0 };
            self.pos += 1;
            self.value |= (b as u64) << (56 - self.bits);
            self.bits += 8;
        }
    }

    /// `read_bool(prob)` — §7.3.
    #[inline]
    pub fn read(&mut self, prob: u8) -> bool {
        let split = 1 + (((self.range - 1) * prob as u32) >> 8);
        if self.bits < 8 {
            self.fill();
        }
        let big = (split as u64) << 56;
        let bit = if self.value >= big {
            self.range -= split;
            self.value -= big;
            true
        } else {
            self.range = split;
            false
        };
        let shift = (self.range as u8).leading_zeros();
        self.range <<= shift;
        self.value <<= shift;
        self.bits -= shift as i32;
        bit
    }

    #[inline]
    pub fn flag(&mut self) -> bool {
        self.read(128)
    }

    /// An `n`-bit unsigned literal, most significant bit first (§7.3 `read_literal`).
    pub fn literal(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.read(128) as u32;
        }
        v
    }

    /// A magnitude of `n` bits followed by a sign bit (the header's signed fields, §9).
    pub fn signed(&mut self, n: u32) -> i32 {
        let v = self.literal(n) as i32;
        if self.flag() { -v } else { v }
    }

    /// A flag-guarded optional signed value: absent reads as 0.
    pub fn opt_signed(&mut self, n: u32) -> i32 {
        if self.flag() { self.signed(n) } else { 0 }
    }

    /// Walk a token tree (§8.1): positive entries index the next node pair, entries `<= 0` are
    /// leaves holding `-value`.
    #[inline]
    pub fn tree(&mut self, tree: &[i8], probs: &[u8]) -> u8 {
        let mut i = 0usize;
        loop {
            let n = tree[i + self.read(probs[i >> 1]) as usize];
            if n <= 0 {
                return (-n) as u8;
            }
            i = n as usize;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A §7 boolean *encoder*, straight from the spec's reference pseudo-code, to round-trip
    /// against.
    struct Enc {
        out: alloc::vec::Vec<u8>,
        range: u32,
        bottom: u32,
        bit_count: i32,
    }
    impl Enc {
        fn new() -> Self {
            Enc { out: alloc::vec::Vec::new(), range: 255, bottom: 0, bit_count: 24 }
        }
        fn add_one(&mut self) {
            let mut i = self.out.len();
            while i > 0 {
                i -= 1;
                if self.out[i] == 255 {
                    self.out[i] = 0;
                } else {
                    self.out[i] += 1;
                    break;
                }
            }
        }
        fn put(&mut self, prob: u8, bit: bool) {
            let split = 1 + (((self.range - 1) * prob as u32) >> 8);
            if bit {
                self.bottom = self.bottom.wrapping_add(split);
                self.range -= split;
            } else {
                self.range = split;
            }
            while self.range < 128 {
                self.range <<= 1;
                if self.bottom & (1 << 31) != 0 {
                    self.add_one();
                }
                self.bottom <<= 1;
                self.bit_count -= 1;
                if self.bit_count == 0 {
                    self.out.push((self.bottom >> 24) as u8);
                    self.bottom &= (1 << 24) - 1;
                    self.bit_count = 8;
                }
            }
        }
        fn flush(mut self) -> alloc::vec::Vec<u8> {
            let mut c = self.bit_count;
            let mut v = self.bottom;
            if v & (1 << (32 - c)) != 0 {
                self.add_one();
            }
            v <<= c & 7;
            c >>= 3;
            while c > 0 {
                c -= 1;
                v <<= 8;
            }
            c = 4;
            while c > 0 {
                c -= 1;
                self.out.push((v >> 24) as u8);
                v <<= 8;
            }
            self.out
        }
    }

    #[test]
    fn round_trips_the_spec_encoder() {
        let mut seed = 0x1234_5678u32;
        let mut rnd = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let mut items = alloc::vec::Vec::new();
        for _ in 0..20000 {
            let p = (rnd() % 255 + 1) as u8;
            let bit = (rnd() % 256) as u8 >= p;
            items.push((p, bit));
        }
        let mut e = Enc::new();
        for &(p, b) in &items {
            e.put(p, b);
        }
        let bytes = e.flush();
        let mut d = BoolDecoder::new(&bytes);
        for (i, &(p, b)) in items.iter().enumerate() {
            assert_eq!(d.read(p), b, "bit {i}");
        }
    }
}
