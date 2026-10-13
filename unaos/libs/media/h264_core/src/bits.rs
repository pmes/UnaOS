// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The RBSP bit reader (7.2 functions, 9.1 Exp-Golomb): most significant bit first, over a raw
//! byte sequence payload whose emulation-prevention bytes are already removed ([`crate::nal`]).
//! Reading past the end yields zero bits and sets [`BitReader::overran`], as does an Exp-Golomb
//! code longer than 32 bits, so a damaged syntax structure is reported once by its caller.

pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    /// Bit position of rbsp_stop_one_bit (the last set bit of the payload); `data.len() * 8` when
    /// the payload has no set bit.
    stop: usize,
    overran: bool,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        let mut stop = data.len() * 8;
        for (i, &b) in data.iter().enumerate().rev() {
            if b != 0 {
                stop = i * 8 + 7 - b.trailing_zeros() as usize;
                break;
            }
        }
        BitReader { data, pos: 0, stop, overran: false }
    }

    /// The payload this reader walks.
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// Current bit position.
    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn set_pos(&mut self, pos: usize) {
        self.pos = pos;
    }

    /// f(1) / u(1).
    #[inline]
    pub fn bit(&mut self) -> u32 {
        let b = match self.data.get(self.pos >> 3) {
            Some(v) => (v >> (7 - (self.pos & 7))) & 1,
            None => {
                self.overran = true;
                0
            }
        };
        self.pos += 1;
        b as u32
    }

    #[inline]
    pub fn flag(&mut self) -> bool {
        self.bit() != 0
    }

    /// u(n), n <= 32.
    pub fn u(&mut self, n: u32) -> u32 {
        let mut v = 0u64;
        for _ in 0..n {
            v = (v << 1) | self.bit() as u64;
        }
        v as u32
    }

    /// The next `n` (<= 24) bits without consuming them; zero past the end.
    #[inline]
    pub fn peek(&self, n: u32) -> u32 {
        let mut v = 0u32;
        let mut p = self.pos;
        for _ in 0..n {
            let b = self.data.get(p >> 3).map_or(0, |v| (v >> (7 - (p & 7))) & 1);
            v = (v << 1) | b as u32;
            p += 1;
        }
        v
    }

    pub fn skip(&mut self, n: usize) {
        self.pos += n;
        if self.pos > self.data.len() * 8 {
            self.overran = true;
        }
    }

    /// ue(v) (9.1): leadingZeroBits zeros, a one, then leadingZeroBits bits.
    pub fn ue(&mut self) -> u32 {
        let mut lz = 0u32;
        while self.bit() == 0 {
            lz += 1;
            if lz > 31 || self.overran {
                self.overran = true;
                return 0;
            }
        }
        if lz == 0 {
            return 0;
        }
        (((1u64 << lz) - 1) as u32).wrapping_add(self.u(lz))
    }

    /// se(v) (9.1.1, Table 9-3): codeNum k maps to (-1)^(k+1) * Ceil(k / 2).
    pub fn se(&mut self) -> i32 {
        let k = self.ue() as i64;
        if k & 1 == 1 { ((k + 1) / 2) as i32 } else { (-(k / 2)) as i32 }
    }

    /// te(v) (9.1): with a range greater than 1 it is ue(v); otherwise the inverted bit.
    pub fn te(&mut self, range: u32) -> u32 {
        if range > 1 { self.ue() } else { 1 - self.bit() }
    }

    pub fn byte_aligned(&self) -> bool {
        self.pos & 7 == 0
    }

    /// more_rbsp_data() (7.2): true while the position is before rbsp_stop_one_bit.
    pub fn more_rbsp_data(&self) -> bool {
        self.pos < self.stop
    }

    pub fn overran(&self) -> bool {
        self.overran
    }

    pub fn set_overran(&mut self) {
        self.overran = true;
    }

    /// Bits left before the end of the payload.
    pub fn bits_left(&self) -> isize {
        self.data.len() as isize * 8 - self.pos as isize
    }
}
