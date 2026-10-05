//! §4.10 descriptors: the plain (non-arithmetic) bit reader.

use crate::{Error, Result};

/// MSB-first bit reader over a byte slice (§8.1: "the first bit is given by the most significant
/// bit of the first byte").
#[derive(Clone)]
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize, // bit position
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        BitReader { data, pos: 0 }
    }
    /// get_position(): the bit position.
    pub fn position(&self) -> usize {
        self.pos
    }
    pub fn byte_pos(&self) -> usize {
        self.pos >> 3
    }
    pub fn data(&self) -> &'a [u8] {
        self.data
    }
    pub fn read_bit(&mut self) -> Result<u32> {
        let byte = *self.data.get(self.pos >> 3).ok_or(Error::Truncated)?;
        let bit = (byte >> (7 - (self.pos & 7))) & 1;
        self.pos += 1;
        Ok(bit as u32)
    }
    /// f(n), n <= 32.
    pub fn f(&mut self, n: u32) -> Result<u32> {
        let mut x: u64 = 0;
        for _ in 0..n {
            x = 2 * x + self.read_bit()? as u64;
        }
        Ok(x as u32)
    }
    pub fn flag(&mut self) -> Result<bool> {
        Ok(self.read_bit()? != 0)
    }
    /// su(n)
    pub fn su(&mut self, n: u32) -> Result<i32> {
        let mut value = self.f(n)? as i64;
        let sign_mask = 1i64 << (n - 1);
        if value & sign_mask != 0 {
            value -= 2 * sign_mask;
        }
        Ok(value as i32)
    }
    /// ns(n)
    pub fn ns(&mut self, n: u32) -> Result<u32> {
        let w = floor_log2(n) + 1;
        let m = (1u32 << w) - n;
        let v = self.f(w - 1)?;
        if v < m {
            return Ok(v);
        }
        let extra_bit = self.f(1)?;
        Ok((v << 1) - m + extra_bit)
    }
    /// le(n): little-endian n bytes.
    pub fn le(&mut self, n: u32) -> Result<u32> {
        let mut t: u64 = 0;
        for i in 0..n {
            t += (self.f(8)? as u64) << (i * 8);
        }
        Ok(t as u32)
    }
    /// leb128()
    pub fn leb128(&mut self) -> Result<u64> {
        let mut value: u64 = 0;
        for i in 0..8 {
            let b = self.f(8)? as u64;
            value |= (b & 0x7f) << (i * 7);
            if b & 0x80 == 0 {
                break;
            }
        }
        Ok(value)
    }
    /// uvlc()
    pub fn uvlc(&mut self) -> Result<u32> {
        let mut leading_zeros = 0u32;
        loop {
            if self.read_bit()? != 0 {
                break;
            }
            leading_zeros += 1;
            if leading_zeros > 64 {
                return Err(Error::Invalid("uvlc"));
            }
        }
        if leading_zeros >= 32 {
            return Ok(u32::MAX);
        }
        let value = self.f(leading_zeros)?;
        Ok(value + ((1u64 << leading_zeros) - 1) as u32)
    }
    /// byte_alignment()
    pub fn byte_alignment(&mut self) -> Result<()> {
        while self.pos & 7 != 0 {
            self.read_bit()?;
        }
        Ok(())
    }
    pub fn skip_bits(&mut self, n: usize) {
        self.pos += n;
    }
}

/// FloorLog2(x) for x > 0.
pub fn floor_log2(x: u32) -> u32 {
    31 - x.leading_zeros()
}

/// CeilLog2(x): 0 for x < 2.
pub fn ceil_log2(x: u32) -> u32 {
    if x < 2 {
        return 0;
    }
    let mut i = 1;
    let mut p = 2u32;
    while p < x {
        i += 1;
        p <<= 1;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn descriptors() {
        // f / su
        let mut r = BitReader::new(&[0b1011_0011, 0xff]);
        assert_eq!(r.f(3).unwrap(), 0b101);
        assert_eq!(r.su(3).unwrap(), -4); // bits 100 -> 4 - 8
    }
    #[test]
    fn ns_table() {
        // §4.10.7 table for n = 5: 0:00 1:01 2:10 3:110 4:111
        let cases: [(u8, u32); 5] = [(0b0000_0000, 0), (0b0100_0000, 1), (0b1000_0000, 2), (0b1100_0000, 3), (0b1110_0000, 4)];
        for (byte, v) in cases {
            let b = [byte];
            let mut r = BitReader::new(&b);
            assert_eq!(r.ns(5).unwrap(), v);
        }
    }
    #[test]
    fn leb128_uvlc_le() {
        let mut r = BitReader::new(&[0xe5, 0x8e, 0x26]);
        assert_eq!(r.leb128().unwrap(), 624485);
        // uvlc: 00101 -> leadingZeros 2, value 01 -> 1 + 3 = 4
        let mut r = BitReader::new(&[0b0010_1000]);
        assert_eq!(r.uvlc().unwrap(), 4);
        let mut r = BitReader::new(&[0x34, 0x12]);
        assert_eq!(r.le(2).unwrap(), 0x1234);
    }
    #[test]
    fn logs() {
        assert_eq!(floor_log2(1), 0);
        assert_eq!(floor_log2(255), 7);
        assert_eq!(ceil_log2(1), 0);
        assert_eq!(ceil_log2(5), 3);
        assert_eq!(ceil_log2(8), 3);
    }
}
