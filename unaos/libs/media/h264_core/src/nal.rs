// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! NAL unit framing: the NAL unit header (7.3.1), emulation prevention (7.4.1:
//! emulation_prevention_three_byte), the Annex B byte stream (B.1 / B.2: start code prefixes,
//! zero_byte, trailing_zero_8bits), and the length-prefixed framing of ISO/IEC 14496-15 with its
//! AVCDecoderConfigurationRecord (`avcC`, §5.3.3.1), which MP4 `avc1`/`avc3` and Matroska
//! `V_MPEG4/ISO/AVC` (CodecPrivate) hand the decoder.

use alloc::vec::Vec;

use crate::{Error, Result};

/// nal_unit_type values (Table 7-1) this decoder acts on.
pub const NAL_SLICE: u8 = 1;
pub const NAL_SLICE_DPA: u8 = 2;
pub const NAL_SLICE_DPC: u8 = 4;
pub const NAL_IDR: u8 = 5;
pub const NAL_SEI: u8 = 6;
pub const NAL_SPS: u8 = 7;
pub const NAL_PPS: u8 = 8;
pub const NAL_AUD: u8 = 9;
pub const NAL_END_SEQ: u8 = 10;
pub const NAL_END_STREAM: u8 = 11;
pub const NAL_FILLER: u8 = 12;

/// The one-byte NAL unit header (7.3.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NalHeader {
    pub ref_idc: u8,
    pub nal_type: u8,
}

pub fn parse_header(b: u8) -> Result<NalHeader> {
    if b & 0x80 != 0 {
        return Err(Error::Invalid("forbidden_zero_bit set"));
    }
    Ok(NalHeader { ref_idc: (b >> 5) & 3, nal_type: b & 0x1f })
}

/// The RBSP of a NAL unit payload (the bytes after the header): every
/// emulation_prevention_three_byte (a 0x03 after two zero bytes, 7.3.1) dropped.
pub fn unescape(payload: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.reserve(payload.len());
    let mut zeros = 0usize;
    for &b in payload {
        if zeros >= 2 && b == 3 {
            zeros = 0;
            continue;
        }
        out.push(b);
        zeros = if b == 0 { zeros + 1 } else { 0 };
    }
}

/// The NAL units of an Annex B byte stream (B.2): every byte sequence that follows a 0x000001
/// start code prefix, up to the next prefix, with trailing zero bytes (trailing_zero_8bits and the
/// next start code's zero_byte) removed. Bytes before the first prefix are ignored.
pub fn annexb_units(data: &[u8]) -> Vec<&[u8]> {
    let mut starts = Vec::new();
    let mut i = 0;
    while i + 2 < data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push(i + 3);
            i += 3;
        } else {
            i += 1;
        }
    }
    let mut units = Vec::with_capacity(starts.len());
    for (k, &s) in starts.iter().enumerate() {
        let mut e = if k + 1 < starts.len() { starts[k + 1] - 3 } else { data.len() };
        while e > s && data[e - 1] == 0 {
            e -= 1;
        }
        if e > s {
            units.push(&data[s..e]);
        }
    }
    units
}

/// The NAL units of one length-prefixed sample (ISO/IEC 14496-15 §5.3.2): each unit is preceded
/// by its size as a `len_size`-byte big-endian integer.
pub fn length_prefixed_units(data: &[u8], len_size: usize) -> Result<Vec<&[u8]>> {
    if !(1..=4).contains(&len_size) || len_size == 3 {
        return Err(Error::Invalid("NAL length size not 1, 2 or 4"));
    }
    let mut units = Vec::new();
    let mut p = 0;
    while p < data.len() {
        if p + len_size > data.len() {
            return Err(Error::Truncated);
        }
        let mut n = 0usize;
        for k in 0..len_size {
            n = (n << 8) | data[p + k] as usize;
        }
        p += len_size;
        if p + n > data.len() {
            return Err(Error::Truncated);
        }
        if n > 0 {
            units.push(&data[p..p + n]);
        }
        p += n;
    }
    Ok(units)
}

/// AVCDecoderConfigurationRecord (ISO/IEC 14496-15 §5.3.3.1).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AvcConfig {
    pub profile_idc: u8,
    pub level_idc: u8,
    /// lengthSizeMinusOne + 1.
    pub nal_length_size: usize,
    /// The sequence and picture parameter set NAL units (header byte included).
    pub sps: Vec<Vec<u8>>,
    pub pps: Vec<Vec<u8>>,
}

pub fn parse_avcc(d: &[u8]) -> Result<AvcConfig> {
    if d.len() < 7 {
        return Err(Error::Truncated);
    }
    if d[0] != 1 {
        return Err(Error::Invalid("avcC configurationVersion is not 1"));
    }
    let mut c = AvcConfig { profile_idc: d[1], level_idc: d[3], nal_length_size: (d[4] & 3) as usize + 1, ..Default::default() };
    let mut p = 5;
    let take = |p: &mut usize| -> Result<Vec<u8>> {
        if *p + 2 > d.len() {
            return Err(Error::Truncated);
        }
        let n = ((d[*p] as usize) << 8) | d[*p + 1] as usize;
        *p += 2;
        if *p + n > d.len() {
            return Err(Error::Truncated);
        }
        let v = d[*p..*p + n].to_vec();
        *p += n;
        Ok(v)
    };
    let nsps = d[p] & 0x1f;
    p += 1;
    for _ in 0..nsps {
        c.sps.push(take(&mut p)?);
    }
    if p >= d.len() {
        return Err(Error::Truncated);
    }
    let npps = d[p];
    p += 1;
    for _ in 0..npps {
        c.pps.push(take(&mut p)?);
    }
    // High-profile records carry chroma_format / bit depth / SPS extensions after this; the SPS
    // itself is authoritative, so they are not read.
    Ok(c)
}
