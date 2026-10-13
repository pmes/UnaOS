// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The uncompressed header's bit reader (spec §4.9 f(n), §9.1 su(n)): most significant bit first.
//! Reading past the end yields zero bits and sets [`BitReader::overran`], so a truncated header is
//! reported once by the caller instead of at every field.

pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
    overran: bool,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0, overran: false }
    }

    /// f(1).
    pub fn bit(&mut self) -> u32 {
        let byte = self.pos >> 3;
        let b = match self.data.get(byte) {
            Some(v) => (v >> (7 - (self.pos & 7))) & 1,
            None => {
                self.overran = true;
                0
            }
        };
        self.pos += 1;
        b as u32
    }

    pub fn flag(&mut self) -> bool {
        self.bit() != 0
    }

    /// f(n), n <= 32.
    pub fn f(&mut self, n: u32) -> u32 {
        let mut v = 0u32;
        for _ in 0..n {
            v = (v << 1) | self.bit();
        }
        v
    }

    /// su(1+n): n magnitude bits then a sign bit (libvpx vpx_rb_read_signed_literal).
    pub fn su(&mut self, n: u32) -> i32 {
        let v = self.f(n) as i32;
        if self.flag() { -v } else { v }
    }

    /// Bytes consumed, rounded up (the compressed header starts on the next byte).
    pub fn bytes_read(&self) -> usize {
        self.pos.div_ceil(8)
    }

    pub fn overran(&self) -> bool {
        self.overran
    }
}
