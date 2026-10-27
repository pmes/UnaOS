//! The AV1 codec configuration record, `av1C` (AV1-ISOBMFF §2.3.3), as an ISOBMFF / HEIF container carries it.
//!
//! HEIFONE-RETIRE (LEDGER SR318): this module was av1_core's own AVIF item walk (`avif_payload`); every container
//! read is now demux_core's codec-blind HEIF reader (`demux_core::heif`, HEIFONE SR303), which hands the raw record to
//! [`crate::image::decode_item_planes`]. What stays is the record's parser, which the item decode, the
//! [`crate::image::StreamDecoder`] and [`crate::image::av1c_colour`] share.

use crate::{Error, Result};
use alloc::vec::Vec;

/// `av1C` (AV1CodecConfigurationRecord).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Av1Config {
    pub seq_profile: u8,
    pub seq_level_idx_0: u8,
    pub seq_tier_0: u8,
    pub high_bitdepth: bool,
    pub twelve_bit: bool,
    pub monochrome: bool,
    pub chroma_subsampling_x: bool,
    pub chroma_subsampling_y: bool,
    pub chroma_sample_position: u8,
    /// configOBUs (normally the sequence header).
    pub config_obus: Vec<u8>,
}

impl Av1Config {
    /// Parse an AV1CodecConfigurationRecord body (AV1-ISOBMFF §2.3.3): marker/version, profile,
    /// level, tier, bit depth, mono, subsampling, chroma position, then the configOBUs.
    pub fn parse(b: &[u8]) -> Result<Av1Config> {
        if b.len() < 4 || b[0] & 0x7f != 1 {
            return Err(Error::Invalid("av1C"));
        }
        Ok(Av1Config {
            seq_profile: b[1] >> 5,
            seq_level_idx_0: b[1] & 31,
            seq_tier_0: b[2] >> 7,
            high_bitdepth: b[2] & 0x40 != 0,
            twelve_bit: b[2] & 0x20 != 0,
            monochrome: b[2] & 0x10 != 0,
            chroma_subsampling_x: b[2] & 0x08 != 0,
            chroma_subsampling_y: b[2] & 0x04 != 0,
            chroma_sample_position: b[2] & 3,
            config_obus: b[4..].to_vec(),
        })
    }
}
