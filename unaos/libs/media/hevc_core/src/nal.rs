// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! NAL units: the two-byte header (7.3.1.2), the nal_unit_type values (Table 7-1), and the
//! HEVCDecoderConfigurationRecord (`hvcC`, ISO/IEC 14496-15 8.3.3.1) that MP4 `hvc1`/`hev1` and Matroska
//! `V_MPEGH/ISO/HEVC` hand the decoder. Emulation prevention (7.4.2), the Annex B byte stream (Annex B) and the
//! length-prefixed samples (ISO/IEC 14496-15) are framed exactly as in H.264: h264_core's functions are re-exported.

use alloc::vec::Vec;

use crate::{Error, Result};

pub use h264_core::nal::{annexb_units, length_prefixed_units, unescape};

/// nal_unit_type values (Table 7-1).
pub const TRAIL_N: u8 = 0;
pub const TRAIL_R: u8 = 1;
pub const TSA_N: u8 = 2;
pub const TSA_R: u8 = 3;
pub const STSA_N: u8 = 4;
pub const STSA_R: u8 = 5;
pub const RADL_N: u8 = 6;
pub const RADL_R: u8 = 7;
pub const RASL_N: u8 = 8;
pub const RASL_R: u8 = 9;
pub const BLA_W_LP: u8 = 16;
pub const BLA_W_RADL: u8 = 17;
pub const BLA_N_LP: u8 = 18;
pub const IDR_W_RADL: u8 = 19;
pub const IDR_N_LP: u8 = 20;
pub const CRA_NUT: u8 = 21;
pub const VPS_NUT: u8 = 32;
pub const SPS_NUT: u8 = 33;
pub const PPS_NUT: u8 = 34;
pub const AUD_NUT: u8 = 35;
pub const EOS_NUT: u8 = 36;
pub const EOB_NUT: u8 = 37;
pub const FD_NUT: u8 = 38;
pub const PREFIX_SEI_NUT: u8 = 39;
pub const SUFFIX_SEI_NUT: u8 = 40;

/// The NAL unit header (7.3.1.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NalHeader {
    pub nal_type: u8,
    pub layer_id: u8,
    /// nuh_temporal_id_plus1 - 1.
    pub temporal_id: u8,
}

impl NalHeader {
    /// A VCL NAL unit (types 0..=31).
    pub fn is_vcl(&self) -> bool {
        self.nal_type < 32
    }
    /// An IRAP picture (BLA, IDR, CRA and the reserved 22, 23).
    pub fn is_irap(&self) -> bool {
        (BLA_W_LP..=23).contains(&self.nal_type)
    }
    pub fn is_idr(&self) -> bool {
        self.nal_type == IDR_W_RADL || self.nal_type == IDR_N_LP
    }
    pub fn is_bla(&self) -> bool {
        (BLA_W_LP..=BLA_N_LP).contains(&self.nal_type)
    }
    pub fn is_rasl(&self) -> bool {
        self.nal_type == RASL_N || self.nal_type == RASL_R
    }
    pub fn is_radl(&self) -> bool {
        self.nal_type == RADL_N || self.nal_type == RADL_R
    }
    /// A sub-layer non-reference picture (7.4.2.2: the even types below 16, i.e. *_N).
    pub fn is_slnr(&self) -> bool {
        self.nal_type <= 14 && self.nal_type % 2 == 0
    }
}

pub fn parse_header(d: &[u8]) -> Result<NalHeader> {
    if d.len() < 2 {
        return Err(Error::Truncated);
    }
    if d[0] & 0x80 != 0 {
        return Err(Error::Invalid("forbidden_zero_bit set"));
    }
    let tid1 = d[1] & 7;
    if tid1 == 0 {
        return Err(Error::Invalid("nuh_temporal_id_plus1 is 0"));
    }
    Ok(NalHeader { nal_type: (d[0] >> 1) & 0x3f, layer_id: ((d[0] & 1) << 5) | (d[1] >> 3), temporal_id: tid1 - 1 })
}

/// One array of the record: its NAL unit type and the NAL units (two header bytes included).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct NalArray {
    pub array_completeness: bool,
    pub nal_type: u8,
    pub units: Vec<Vec<u8>>,
}

/// HEVCDecoderConfigurationRecord (ISO/IEC 14496-15 8.3.3.1.2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HvcConfig {
    pub general_profile_space: u8,
    pub general_tier_flag: bool,
    pub general_profile_idc: u8,
    pub general_profile_compatibility_flags: u32,
    /// The 48 bits of general_constraint_indicator_flags.
    pub general_constraint_indicator_flags: u64,
    pub general_level_idc: u8,
    pub min_spatial_segmentation_idc: u16,
    pub parallelism_type: u8,
    pub chroma_format_idc: u8,
    pub bit_depth_luma: u8,
    pub bit_depth_chroma: u8,
    pub avg_frame_rate: u16,
    pub constant_frame_rate: u8,
    pub num_temporal_layers: u8,
    pub temporal_id_nested: bool,
    /// lengthSizeMinusOne + 1.
    pub nal_length_size: usize,
    pub arrays: Vec<NalArray>,
}

impl HvcConfig {
    /// Every NAL unit of every array, in record order (VPS, SPS, PPS, SEI as the muxer wrote them).
    pub fn units(&self) -> impl Iterator<Item = &[u8]> + '_ {
        self.arrays.iter().flat_map(|a| a.units.iter().map(|u| u.as_slice()))
    }
}

pub fn parse_hvcc(d: &[u8]) -> Result<HvcConfig> {
    if d.len() < 23 {
        return Err(Error::Truncated);
    }
    if d[0] != 1 {
        return Err(Error::Invalid("hvcC configurationVersion is not 1"));
    }
    let be = |s: &[u8]| s.iter().fold(0u64, |a, &b| (a << 8) | b as u64);
    let mut c = HvcConfig {
        general_profile_space: d[1] >> 6,
        general_tier_flag: d[1] & 0x20 != 0,
        general_profile_idc: d[1] & 0x1f,
        general_profile_compatibility_flags: be(&d[2..6]) as u32,
        general_constraint_indicator_flags: be(&d[6..12]),
        general_level_idc: d[12],
        min_spatial_segmentation_idc: (be(&d[13..15]) & 0x0fff) as u16,
        parallelism_type: d[15] & 3,
        chroma_format_idc: d[16] & 3,
        bit_depth_luma: (d[17] & 7) + 8,
        bit_depth_chroma: (d[18] & 7) + 8,
        avg_frame_rate: be(&d[19..21]) as u16,
        constant_frame_rate: d[21] >> 6,
        num_temporal_layers: (d[21] >> 3) & 7,
        temporal_id_nested: d[21] & 4 != 0,
        nal_length_size: (d[21] & 3) as usize + 1,
        arrays: Vec::new(),
    };
    let num_arrays = d[22];
    let mut p = 23;
    for _ in 0..num_arrays {
        if p + 3 > d.len() {
            return Err(Error::Truncated);
        }
        let mut a = NalArray { array_completeness: d[p] & 0x80 != 0, nal_type: d[p] & 0x3f, units: Vec::new() };
        let n = be(&d[p + 1..p + 3]) as usize;
        p += 3;
        for _ in 0..n {
            if p + 2 > d.len() {
                return Err(Error::Truncated);
            }
            let len = be(&d[p..p + 2]) as usize;
            p += 2;
            if p + len > d.len() {
                return Err(Error::Truncated);
            }
            a.units.push(d[p..p + len].to_vec());
            p += len;
        }
        c.arrays.push(a);
    }
    Ok(c)
}
