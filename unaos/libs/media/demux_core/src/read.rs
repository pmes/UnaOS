// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Bounds-checked big-endian reader. Every container field passes through here, so a truncated or
//! hostile file is an [`Error`], never a panic.

use crate::Error;

#[derive(Clone)]
pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Reader { buf, pos: 0 }
    }
    pub fn pos(&self) -> usize {
        self.pos
    }
    pub fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }
    pub fn is_empty(&self) -> bool {
        self.remaining() == 0
    }
    pub fn bytes(&mut self, n: usize) -> Result<&'a [u8], Error> {
        if n > self.remaining() {
            return Err(Error::Truncated);
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn skip(&mut self, n: usize) -> Result<(), Error> {
        self.bytes(n).map(|_| ())
    }
    pub fn u8(&mut self) -> Result<u8, Error> {
        Ok(self.bytes(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, Error> {
        let b = self.bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }
    pub fn i16(&mut self) -> Result<i16, Error> {
        Ok(self.u16()? as i16)
    }
    pub fn u24(&mut self) -> Result<u32, Error> {
        let b = self.bytes(3)?;
        Ok(u32::from_be_bytes([0, b[0], b[1], b[2]]))
    }
    pub fn u32(&mut self) -> Result<u32, Error> {
        let b = self.bytes(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    pub fn i32(&mut self) -> Result<i32, Error> {
        Ok(self.u32()? as i32)
    }
    pub fn u64(&mut self) -> Result<u64, Error> {
        let b = self.bytes(8)?;
        let mut a = [0u8; 8];
        a.copy_from_slice(b);
        Ok(u64::from_be_bytes(a))
    }
    pub fn i64(&mut self) -> Result<i64, Error> {
        Ok(self.u64()? as i64)
    }
    pub fn fourcc(&mut self) -> Result<[u8; 4], Error> {
        let b = self.bytes(4)?;
        Ok([b[0], b[1], b[2], b[3]])
    }
    /// Unsigned big-endian integer of `n` (0..=8) bytes — Matroska's uint element body.
    pub fn uint_n(&mut self, n: usize) -> Result<u64, Error> {
        if n > 8 {
            return Err(Error::Invalid("integer wider than 8 bytes"));
        }
        let mut v = 0u64;
        for &b in self.bytes(n)? {
            v = (v << 8) | b as u64;
        }
        Ok(v)
    }
}
