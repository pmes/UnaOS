//! Bit readers. [`BitReader`] is MSB-first (FLAC, MP3, AAC, Opus headers); [`LsbReader`] is LSB-first
//! (Vorbis, Xiph spec I §2.1.4). Both read from a byte slice and return [`Error::Eof`] past its end.
use crate::{Error, Result};

#[derive(Clone)]
pub struct BitReader<'a> {
    d: &'a [u8],
    pos: usize, // bit position
}

impl<'a> BitReader<'a> {
    pub fn new(d: &'a [u8]) -> BitReader<'a> { BitReader { d, pos: 0 } }
    #[inline]
    pub fn bit_pos(&self) -> usize { self.pos }
    #[inline]
    pub fn bits_left(&self) -> usize { self.d.len() * 8 - self.pos.min(self.d.len() * 8) }
    #[inline]
    pub fn byte_pos(&self) -> usize { self.pos.div_ceil(8) }
    pub fn align(&mut self) { self.pos = self.pos.div_ceil(8) * 8; }
    pub fn seek_bits(&mut self, p: usize) { self.pos = p; }
    #[inline]
    pub fn skip(&mut self, n: usize) -> Result<()> {
        if self.pos + n > self.d.len() * 8 { return Err(Error::Eof); }
        self.pos += n;
        Ok(())
    }
    /// Up to 57 bits.
    #[inline]
    pub fn peek(&self, n: u32) -> u64 {
        if n == 0 { return 0; }
        let byte = self.pos >> 3;
        let mut w = [0u8; 8];
        let end = (byte + 8).min(self.d.len());
        if byte < end { w[..end - byte].copy_from_slice(&self.d[byte..end]); }
        let v = u64::from_be_bytes(w) << (self.pos & 7);
        v >> (64 - n)
    }
    #[inline]
    pub fn read(&mut self, n: u32) -> Result<u32> {
        if n == 0 { return Ok(0); }
        if self.pos + n as usize > self.d.len() * 8 { return Err(Error::Eof); }
        let v = self.peek(n);
        self.pos += n as usize;
        Ok(v as u32)
    }
    #[inline]
    pub fn read64(&mut self, n: u32) -> Result<u64> {
        if n <= 32 { return Ok(self.read(n)? as u64); }
        let hi = self.read(n - 32)? as u64;
        Ok((hi << 32) | self.read(32)? as u64)
    }
    #[inline]
    pub fn bit(&mut self) -> Result<bool> { Ok(self.read(1)? != 0) }
    /// Two's-complement signed value of `n` bits (n ≤ 64).
    #[inline]
    pub fn signed(&mut self, n: u32) -> Result<i64> {
        if n == 0 { return Ok(0); }
        let v = self.read64(n)?;
        Ok(((v << (64 - n)) as i64) >> (64 - n))
    }
    /// Count of 0 bits before the next 1 bit (the 1 is consumed).
    #[inline]
    pub fn unary(&mut self) -> Result<u32> {
        let mut n = 0u32;
        loop {
            if self.pos >= self.d.len() * 8 { return Err(Error::Eof); }
            let w = self.peek(32) as u32;
            if w != 0 {
                let z = w.leading_zeros();
                if self.pos + z as usize + 1 > self.d.len() * 8 { return Err(Error::Eof); }
                self.pos += z as usize + 1;
                return Ok(n + z);
            }
            n += 32;
            self.pos += 32;
        }
    }
}

/// LSB-first reader (Vorbis packs bits from the least significant end of each byte).
#[derive(Clone)]
pub struct LsbReader<'a> {
    d: &'a [u8],
    pos: usize,
}
impl<'a> LsbReader<'a> {
    pub fn new(d: &'a [u8]) -> LsbReader<'a> { LsbReader { d, pos: 0 } }
    pub fn bits_left(&self) -> usize { (self.d.len() * 8).saturating_sub(self.pos) }
    /// Up to 32 bits. Past the end: Eof (Vorbis treats an end-of-packet read as "the packet ends here").
    #[inline]
    pub fn read(&mut self, n: u32) -> Result<u32> {
        if n == 0 { return Ok(0); }
        if self.pos + n as usize > self.d.len() * 8 { self.pos = self.d.len() * 8; return Err(Error::Eof); }
        let byte = self.pos >> 3;
        let mut w = [0u8; 8];
        let end = (byte + 8).min(self.d.len());
        w[..end - byte].copy_from_slice(&self.d[byte..end]);
        let v = u64::from_le_bytes(w) >> (self.pos & 7);
        self.pos += n as usize;
        Ok((v & ((1u64 << n) - 1)) as u32)
    }
    #[inline]
    pub fn bit(&mut self) -> Result<bool> { Ok(self.read(1)? != 0) }
    /// Peek up to 32 bits without consuming; bits past the end read as zero. Returns (value, available).
    #[inline]
    pub fn peek(&self, n: u32) -> (u32, usize) {
        let byte = self.pos >> 3;
        let mut w = [0u8; 8];
        if byte < self.d.len() {
            let end = (byte + 8).min(self.d.len());
            w[..end - byte].copy_from_slice(&self.d[byte..end]);
        }
        let v = u64::from_le_bytes(w) >> (self.pos & 7);
        (if n == 32 { v as u32 } else { (v & ((1u64 << n) - 1)) as u32 }, self.bits_left())
    }
    #[inline]
    pub fn advance(&mut self, n: usize) { self.pos += n; }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn msb_and_lsb() {
        let d = [0b1010_0001u8, 0b0000_0001, 0xFF];
        let mut r = BitReader::new(&d);
        assert_eq!(r.read(3).unwrap(), 0b101);
        assert_eq!(r.unary().unwrap(), 4);
        assert_eq!(r.unary().unwrap(), 7);
        assert_eq!(r.signed(4).unwrap(), -1);
        let mut l = LsbReader::new(&d);
        assert_eq!(l.read(1).unwrap(), 1);
        assert_eq!(l.read(7).unwrap(), 0b101_0000);
        assert_eq!(l.read(9).unwrap(), 0b1_0000_0001);
    }
}
