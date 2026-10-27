// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The writing side of NAL framing: an RBSP bit writer (7.2 u(n), 9.1 ue(v) / se(v),
//! rbsp_trailing_bits), emulation prevention (7.4.1: an emulation_prevention_three_byte after two
//! zero bytes when the next byte is 0..3), the NAL unit header (7.3.1) and the
//! AVCDecoderConfigurationRecord (ISO/IEC 14496-15 §5.3.3.1). h264_core's `bits`, `nal::unescape`
//! and `nal::parse_avcc` are the reading side this mirrors; its NAL type constants are used by name.

use alloc::vec::Vec;

pub use h264_core::nal::{NAL_IDR, NAL_PPS, NAL_SLICE, NAL_SPS};

/// An MSB-first bit writer over a growing byte buffer.
#[derive(Default)]
pub struct BitWriter {
    buf: Vec<u8>,
    cur: u32,
    n: u32,
}

impl BitWriter {
    pub fn new() -> BitWriter {
        BitWriter { buf: Vec::new(), cur: 0, n: 0 }
    }

    #[inline]
    fn bit(&mut self, b: u32) {
        self.cur = (self.cur << 1) | (b & 1);
        self.n += 1;
        if self.n == 8 {
            self.buf.push(self.cur as u8);
            self.cur = 0;
            self.n = 0;
        }
    }

    /// u(n): the low `n` bits of `v`, most significant first (n <= 32).
    pub fn u(&mut self, n: u32, v: u32) {
        for i in (0..n).rev() {
            self.bit(v >> i);
        }
    }

    pub fn flag(&mut self, b: bool) {
        self.bit(b as u32);
    }

    /// ue(v) (9.1): leadingZeroBits zeros, then codeNum + 1 in leadingZeroBits + 1 bits.
    pub fn ue(&mut self, v: u32) {
        let x = v as u64 + 1;
        let lz = 63 - x.leading_zeros();
        for _ in 0..lz {
            self.bit(0);
        }
        for i in (0..=lz).rev() {
            self.bit((x >> i) as u32);
        }
    }

    /// se(v) (9.1.1, Table 9-3): k > 0 -> 2k - 1, k <= 0 -> -2k.
    pub fn se(&mut self, v: i32) {
        let v = v as i64;
        let k = if v > 0 { 2 * v - 1 } else { -2 * v };
        self.ue(k as u32);
    }

    /// Bits written so far.
    pub fn bits(&self) -> usize {
        self.buf.len() * 8 + self.n as usize
    }

    /// rbsp_trailing_bits (7.3.2.11): the stop bit, then zeros to the byte boundary.
    pub fn trailing(&mut self) {
        self.bit(1);
        while self.n != 0 {
            self.bit(0);
        }
    }

    /// The bytes written (call after [`BitWriter::trailing`]; a partial byte is zero-padded).
    pub fn finish(mut self) -> Vec<u8> {
        while self.n != 0 {
            self.bit(0);
        }
        self.buf
    }
}

/// Length in bits of ue(v).
pub fn ue_bits(v: u32) -> u32 {
    let x = v as u64 + 1;
    2 * (63 - x.leading_zeros()) + 1
}

/// Length in bits of se(v).
pub fn se_bits(v: i32) -> u32 {
    let v = v as i64;
    ue_bits(if v > 0 { 2 * v - 1 } else { -2 * v } as u32)
}

/// Append `rbsp` to `out` with emulation prevention (7.4.1): within the NAL unit no 0x000000,
/// 0x000001, 0x000002 or 0x000003 may occur, so a 0x03 goes after every two zero bytes that
/// precede a byte <= 3. h264_core's `nal::unescape` removes exactly these.
pub fn escape(rbsp: &[u8], out: &mut Vec<u8>) {
    let mut zeros = 0usize;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(3);
            zeros = 0;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
}

/// One NAL unit (7.3.1): the header byte (forbidden_zero_bit 0, nal_ref_idc, nal_unit_type), then
/// the escaped RBSP. No start code or length prefix.
pub fn nal_unit(ref_idc: u8, nal_type: u8, rbsp: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(rbsp.len() + rbsp.len() / 64 + 2);
    v.push(((ref_idc & 3) << 5) | (nal_type & 0x1f));
    escape(rbsp, &mut v);
    v
}

/// AVCDecoderConfigurationRecord (ISO/IEC 14496-15 §5.3.3.1) for one SPS and one PPS NAL unit
/// (header byte included): configurationVersion 1, AVCProfileIndication / profile_compatibility /
/// AVCLevelIndication copied from the SPS, lengthSizeMinusOne 3 (4-byte sample lengths).
pub fn avcc_record(sps: &[u8], pps: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(11 + sps.len() + pps.len());
    v.extend_from_slice(&[1, sps[1], sps[2], sps[3], 0xFC | 3, 0xE0 | 1]);
    v.extend_from_slice(&(sps.len() as u16).to_be_bytes());
    v.extend_from_slice(sps);
    v.push(1);
    v.extend_from_slice(&(pps.len() as u16).to_be_bytes());
    v.extend_from_slice(pps);
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use h264_core::bits::BitReader;

    #[test]
    fn ue_se_round_trip_through_the_decoders_reader() {
        let mut w = BitWriter::new();
        let us = [0u32, 1, 2, 3, 7, 8, 254, 255, 1000, 65535, 1 << 20];
        let ss = [0i32, 1, -1, 2, -2, 17, -17, 4095, -4096];
        for &u in &us {
            w.ue(u);
        }
        for &s in &ss {
            w.se(s);
        }
        w.u(5, 0b10110);
        let bits = w.bits();
        w.trailing();
        let b = w.finish();
        let mut r = BitReader::new(&b);
        for &u in &us {
            assert_eq!(r.ue(), u);
        }
        for &s in &ss {
            assert_eq!(r.se(), s);
        }
        assert_eq!(r.u(5), 0b10110);
        assert_eq!(r.pos(), bits);
        assert!(!r.more_rbsp_data());
        let mut lens = 0;
        for &u in &us {
            lens += ue_bits(u) as usize;
        }
        for &s in &ss {
            lens += se_bits(s) as usize;
        }
        assert_eq!(lens + 5, bits);
    }

    #[test]
    fn escape_is_undone_by_unescape() {
        let rbsp = vec![0u8, 0, 0, 0, 0, 1, 0, 0, 2, 0, 0, 3, 0, 0, 4, 0, 0, 0, 0x80];
        let mut e = Vec::new();
        escape(&rbsp, &mut e);
        for w in e.windows(3) {
            assert!(!(w[0] == 0 && w[1] == 0 && w[2] <= 2), "start-code emulation left in {e:?}");
        }
        let mut back = Vec::new();
        h264_core::nal::unescape(&e, &mut back);
        assert_eq!(back, rbsp);
    }
}
