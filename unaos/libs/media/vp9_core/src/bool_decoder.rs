// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The boolean entropy decoder (spec §9.2): init_bool / read_bool / exit_bool, the tree reader
//! (§9.3.1 T), the literal L(n), and the compressed header's sub-exponential probability delta
//! (§6.3.5 diff_update_prob, inv_remap_prob).
//!
//! `split = 1 + (((range - 1) * prob) >> 8)`; the window holds up to 64 bits, refilled a byte at a
//! time. Past the end of the partition the decoder reads zero bytes (the reference decoder does the
//! same) and remembers that it ran out, which the frame decoder reports as truncation.

pub struct BoolDecoder<'a> {
    data: &'a [u8],
    pos: usize,
    /// Bits of `value` not yet consumed, left-aligned at bit 63.
    value: u64,
    /// Number of valid bits in `value` beyond the top 8.
    count: i32,
    range: u32,
    overran: bool,
}

impl<'a> BoolDecoder<'a> {
    /// init_bool(sz): load the window and read the marker bit, which must be 0 (§9.2.1).
    pub fn new(data: &'a [u8]) -> crate::Result<Self> {
        if data.is_empty() {
            return Err(crate::Error::Truncated);
        }
        let mut d = BoolDecoder { data, pos: 0, value: 0, count: -8, range: 255, overran: false };
        d.fill();
        if d.read(128) {
            return Err(crate::Error::Invalid("bool decoder marker bit set"));
        }
        Ok(d)
    }

    fn fill(&mut self) {
        // Fill the window so that at least 8 bits beyond the top byte are valid.
        let mut shift = 64 - 8 - (self.count + 8);
        while shift >= 0 {
            let byte = match self.data.get(self.pos) {
                Some(b) => {
                    self.pos += 1;
                    *b
                }
                None => {
                    if self.pos >= self.data.len() + 8 {
                        self.overran = true;
                    }
                    self.pos += 1;
                    0
                }
            };
            self.value |= (byte as u64) << shift;
            self.count += 8;
            shift -= 8;
        }
    }

    /// read_bool(p): one bit whose probability of being 0 is p/256.
    #[inline]
    pub fn read(&mut self, prob: u8) -> bool {
        let split = 1 + (((self.range - 1) * prob as u32) >> 8);
        if self.count < 0 {
            self.fill();
        }
        let bigsplit = (split as u64) << 56;
        let bit;
        let range;
        if self.value >= bigsplit {
            range = self.range - split;
            self.value -= bigsplit;
            bit = true;
        } else {
            range = split;
            bit = false;
        }
        let shift = (range as u8).leading_zeros();
        self.range = range << shift;
        self.value <<= shift;
        self.count -= shift as i32;
        bit
    }

    #[inline]
    pub fn bit(&mut self) -> u32 {
        self.read(128) as u32
    }

    /// L(n): n equiprobable bits, most significant first.
    pub fn literal(&mut self, n: u32) -> u32 {
        let mut v = 0;
        for _ in 0..n {
            v = (v << 1) | self.bit();
        }
        v
    }

    /// T: walk a libvpx-shaped tree (`tree[i]` > 0 is the next node pair, <= 0 is a leaf `-v`) with
    /// one probability per node pair.
    pub fn tree(&mut self, tree: &[i8], probs: &[u8]) -> usize {
        let mut i: i8 = 0;
        loop {
            i = tree[i as usize + self.read(probs[i as usize >> 1]) as usize];
            if i <= 0 {
                return (-i) as usize;
            }
        }
    }

    /// True once the decoder has read well past the end of its partition (corrupt or truncated).
    pub fn overran(&self) -> bool {
        self.overran
    }

    /// diff_update_prob (§6.3.5): with probability 252 a delta follows, coded sub-exponentially and
    /// remapped around the current value.
    pub fn diff_update_prob(&mut self, p: &mut u8) {
        if self.read(252) {
            let delp = self.decode_term_subexp();
            *p = inv_remap_prob(delp, *p);
        }
    }

    fn decode_term_subexp(&mut self) -> usize {
        if self.bit() == 0 {
            return self.literal(4) as usize;
        }
        if self.bit() == 0 {
            return self.literal(4) as usize + 16;
        }
        if self.bit() == 0 {
            return self.literal(5) as usize + 32;
        }
        // decode_uniform: 7 bits, then one more when the value is past the 65-value short range.
        let v = self.literal(7) as usize;
        let m = (1 << 8) - 191;
        let u = if v < m { v } else { (v << 1) - m + self.bit() as usize };
        u + 64
    }

    /// The MV probability update (§6.3.18 update_mv_prob): with probability 252, a 7-bit value
    /// becomes (v << 1) | 1.
    pub fn update_mv_prob(&mut self, p: &mut u8) {
        if self.read(252) {
            *p = ((self.literal(7) << 1) | 1) as u8;
        }
    }
}

fn inv_recenter_nonneg(v: i32, m: i32) -> i32 {
    if v > 2 * m {
        v
    } else if v & 1 != 0 {
        m - ((v + 1) >> 1)
    } else {
        m + (v >> 1)
    }
}

/// inv_map_table (§6.3.5): the first 20 entries are the coarse steps, the rest fill in between.
fn inv_map(v: usize) -> i32 {
    // Closed form of the spec's 255-entry table: entries 0..19 are 7 + 13k; the
    // remaining entries enumerate 1..=253 skipping those, and the last entry repeats 253.
    if v < 20 {
        return 7 + 13 * v as i32;
    }
    let mut idx = v - 20;
    let mut x = 1i32;
    loop {
        if (x - 7) % 13 == 0 && x <= 254 && x >= 7 {
            x += 1;
            continue;
        }
        if idx == 0 {
            return x.min(253);
        }
        idx -= 1;
        x += 1;
    }
}

pub fn inv_remap_prob(delp: usize, prob: u8) -> u8 {
    let v = inv_map(delp.min(254));
    let m = prob as i32 - 1;
    let r = if (m << 1) <= 255 { 1 + inv_recenter_nonneg(v, m) } else { 255 - inv_recenter_nonneg(v, 255 - 1 - m) };
    r.clamp(1, 255) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The spec's inv_map_table, entries checked against the printed table's landmarks.
    #[test]
    fn inv_map_matches_the_table() {
        let first: [i32; 26] = [7, 20, 33, 46, 59, 72, 85, 98, 111, 124, 137, 150, 163, 176, 189, 202, 215, 228, 241, 254, 1, 2, 3, 4, 5, 6];
        for (i, &v) in first.iter().enumerate() {
            assert_eq!(inv_map(i), v, "entry {i}");
        }
        assert_eq!(inv_map(26), 8);
        assert_eq!(inv_map(252), 252);
        assert_eq!(inv_map(253), 253);
        assert_eq!(inv_map(254), 253);
        // Strictly increasing over 20..=252 and never one of the coarse steps.
        for i in 21..=253 {
            assert!(inv_map(i) > inv_map(i - 1), "{i}");
            assert_ne!((inv_map(i) - 7) % 13, 0);
        }
    }

    /// A stream of 0x00 bytes decodes as all-zero bits at any probability; the marker is 0.
    #[test]
    fn zero_stream_reads_zeros() {
        let data = [0u8; 16];
        let mut d = BoolDecoder::new(&data).unwrap();
        for p in [1u8, 64, 128, 200, 255] {
            assert!(!d.read(p));
        }
        assert_eq!(d.literal(16), 0);
    }
}
