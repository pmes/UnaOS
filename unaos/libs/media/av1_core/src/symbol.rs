//! §8.2 the symbol decoder (non-binary arithmetic decoder) with CDF adaptation.
//!
//! Written exactly as the spec's pseudo-code: init_symbol / read_symbol / read_bool /
//! read_literal / exit_symbol, including the SymbolMaxBits accounting for padding bits.

use crate::bits::floor_log2;
use crate::tables::{EC_MIN_PROB, EC_PROB_SHIFT};
use crate::{Error, Result};

pub struct SymbolDecoder<'a> {
    data: &'a [u8],
    bitpos: usize,
    end_bit: usize,
    symbol_value: u32,
    symbol_range: u32,
    symbol_max_bits: i32,
    /// disable_cdf_update from the frame header.
    pub disable_cdf_update: bool,
}

impl<'a> SymbolDecoder<'a> {
    fn read_bits(&mut self, n: u32) -> u32 {
        let mut x = 0u32;
        for _ in 0..n {
            let bit = if self.bitpos < self.end_bit {
                (self.data[self.bitpos >> 3] >> (7 - (self.bitpos & 7))) & 1
            } else {
                0
            };
            self.bitpos += 1;
            x = 2 * x + bit as u32;
        }
        x
    }

    /// init_symbol( sz ) over `data` (exactly sz bytes).
    pub fn new(data: &'a [u8], disable_cdf_update: bool) -> Result<Self> {
        let sz = data.len();
        if sz == 0 {
            return Err(Error::Truncated);
        }
        let mut s = SymbolDecoder {
            data,
            bitpos: 0,
            end_bit: sz * 8,
            symbol_value: 0,
            symbol_range: 1 << 15,
            symbol_max_bits: 0,
            disable_cdf_update,
        };
        let num_bits = core::cmp::min(sz * 8, 15) as u32;
        let buf = s.read_bits(num_bits);
        let padded_buf = buf << (15 - num_bits);
        s.symbol_value = ((1 << 15) - 1) ^ padded_buf;
        s.symbol_range = 1 << 15;
        s.symbol_max_bits = 8 * sz as i32 - 15;
        Ok(s)
    }

    /// read_symbol( cdf ): cdf has N + 1 entries (N cumulative values ending in 32768, then the
    /// adaptation counter).
    pub fn read_symbol(&mut self, cdf: &mut [u16]) -> usize {
        let n = cdf.len() - 1;
        let mut cur = self.symbol_range;
        let mut symbol: i32 = -1;
        let mut prev;
        loop {
            symbol += 1;
            prev = cur;
            let f = (1u32 << 15) - cdf[symbol as usize] as u32;
            cur = ((self.symbol_range >> 8) * (f >> EC_PROB_SHIFT)) >> (7 - EC_PROB_SHIFT);
            cur += (EC_MIN_PROB * (n - symbol as usize - 1)) as u32;
            if self.symbol_value >= cur {
                break;
            }
        }
        let symbol = symbol as usize;
        self.symbol_range = prev - cur;
        self.symbol_value -= cur;
        // renormalize
        let bits = 15 - floor_log2(self.symbol_range);
        self.symbol_range <<= bits;
        let num_bits = core::cmp::min(bits as i32, core::cmp::max(0, self.symbol_max_bits)) as u32;
        let new_data = self.read_bits(num_bits);
        let padded_data = new_data << (bits - num_bits);
        self.symbol_value = padded_data ^ (((self.symbol_value + 1) << bits) - 1);
        self.symbol_max_bits -= bits as i32;
        if !self.disable_cdf_update {
            let count = cdf[n];
            let rate = 3 + (count > 15) as u32 + (count > 31) as u32 + core::cmp::min(floor_log2(n as u32), 2);
            let mut tmp: u32 = 0;
            for i in 0..n - 1 {
                if i == symbol {
                    tmp = 1 << 15;
                }
                let c = cdf[i] as u32;
                if tmp < c {
                    cdf[i] = (c - ((c - tmp) >> rate)) as u16;
                } else {
                    cdf[i] = (c + ((tmp - c) >> rate)) as u16;
                }
            }
            cdf[n] += (cdf[n] < 32) as u16;
        }
        symbol
    }

    /// read_bool(): a symbol from a fresh equiprobable cdf (never adapted).
    pub fn read_bool(&mut self) -> u32 {
        let mut cdf = [1u16 << 14, 1 << 15, 0];
        let save = self.disable_cdf_update;
        self.disable_cdf_update = true;
        let s = self.read_symbol(&mut cdf);
        self.disable_cdf_update = save;
        s as u32
    }

    /// read_literal( n ) / L(n)
    pub fn read_literal(&mut self, n: u32) -> u32 {
        let mut x = 0;
        for _ in 0..n {
            x = 2 * x + self.read_bool();
        }
        x
    }

    /// NS(n)
    pub fn read_ns(&mut self, n: u32) -> u32 {
        let w = floor_log2(n) + 1;
        let m = (1u32 << w) - n;
        let v = self.read_literal(w - 1);
        if v < m {
            return v;
        }
        let extra_bit = self.read_literal(1);
        (v << 1) - m + extra_bit
    }

    /// The conformance check of exit_symbol(): returns false when the padding is wrong (the
    /// trailing one bit must sit at trailingBitPosition and only zeros follow).
    pub fn exit_check(&self) -> bool {
        if self.symbol_max_bits < -14 {
            return false;
        }
        // get_position() is relative to the tile data start here.
        let pos = self.bitpos as i64;
        let trailing = pos - core::cmp::min(15, self.symbol_max_bits as i64 + 15);
        let padding_end = pos + core::cmp::max(0, self.symbol_max_bits as i64);
        let bit = |p: i64| -> u8 {
            if p < 0 || p as usize >= self.end_bit {
                return 0;
            }
            (self.data[(p as usize) >> 3] >> (7 - (p as usize & 7))) & 1
        };
        if bit(trailing) != 1 {
            return false;
        }
        let mut p = trailing + 1;
        while p < padding_end {
            if bit(p) != 0 {
                return false;
            }
            p += 1;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn adaptation_rule() {
        // A 2-symbol cdf: decoding must move cdf[0] toward the decoded symbol at rate 4 (count 0,
        // N = 2: rate = 3 + 0 + 0 + min(FloorLog2(2), 2) = 4).
        let data = [0u8; 8];
        let mut sd = SymbolDecoder::new(&data, false).unwrap();
        let mut cdf = [16384u16, 32768, 0];
        let s = sd.read_symbol(&mut cdf);
        // all-zero data => SymbolValue = 0x7fff, the largest value -> symbol 0.
        assert_eq!(s, 0);
        // symbol 0: tmp = 32768 for i >= 0, cdf[0] += (32768 - 16384) >> 4 = 1024
        assert_eq!(cdf, [17408, 32768, 1]);
    }
    #[test]
    fn bool_is_equiprobable() {
        // 0xff.. => SymbolValue = 0 at start => largest symbol each time.
        let data = [0xffu8; 4];
        let mut sd = SymbolDecoder::new(&data, false).unwrap();
        assert_eq!(sd.read_bool(), 1);
    }
}
