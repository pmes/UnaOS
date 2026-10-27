// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria (A/V) — seam `shared-core` (hevc_core's level limits, A.4).
//!
//! HEVCSCCTAIL (LEDGER SR365, HEVCREXT-SCC-LEVELS): the general tier and level limits of ITU-T H.265 A.4 — Table A.8
//! (MaxLumaPs, MaxCPB, MaxSliceSegmentsPerPicture, MaxTileRows / MaxTileCols) and Table A.9 (MaxLumaSr, MaxBR) for
//! levels 1 to 6.2, MaxDpbSize by A.4.2 (maxDpbPicBuf 6, or 7 for the screen content coding profiles, whose current
//! picture takes a DPB slot of its own) — and [`check_level`], which refuses by name an active SPS / PPS above the
//! level its profile_tier_level signals. The decoder runs it only when asked ([`crate::Decoder::set_strict_levels`],
//! default off: streams in the wild violate their levels); the conformance walker turns it on. Level 8.5
//! (general_level_idc 255) has no limits. The rate limits (MaxLumaSr, MaxBR, MaxCPB) and the slice segment count need
//! timing and per-picture counts the core does not keep: carried for the Player's info line, not checked (owed
//! HEVC-LEVEL-RATE).

use crate::ps::{Pps, Sps};
use crate::{Error, Result};

/// One row of Tables A.8 and A.9 (bits in units of 1000; [Main tier, High tier], High 0 below level 4).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Level {
    /// general_level_idc: 30 times the level number.
    pub idc: u8,
    pub max_luma_ps: u32,
    pub max_cpb: [u32; 2],
    pub max_slice_segments: u32,
    pub max_tile_rows: u32,
    pub max_tile_cols: u32,
    pub max_luma_sr: u64,
    pub max_br: [u32; 2],
    /// The refusal of a picture above this level's MaxLumaPs (a name carrying the level).
    pub refusal: &'static str,
}

macro_rules! levels {
    ($($name:literal $idc:literal $ps:literal [$cm:literal $ch:literal] $ss:literal $tr:literal $tc:literal $sr:literal [$bm:literal $bh:literal]),* $(,)?) => {
        [$(Level {
            idc: $idc,
            max_luma_ps: $ps,
            max_cpb: [$cm, $ch],
            max_slice_segments: $ss,
            max_tile_rows: $tr,
            max_tile_cols: $tc,
            max_luma_sr: $sr,
            max_br: [$bm, $bh],
            refusal: concat!("picture above level ", $name, " (MaxLumaPs, HEVC-LEVEL)"),
        }),*]
    };
}

/// Tables A.8 and A.9, levels 1 to 6.2.
pub const LEVELS: [Level; 13] = levels![
    "1"   30  36864    [350 0]         16  1  1  552960     [128 0],
    "2"   60  122880   [1500 0]        16  1  1  3686400    [1500 0],
    "2.1" 63  245760   [3000 0]        20  1  1  7372800    [3000 0],
    "3"   90  552960   [6000 0]        30  2  2  16588800   [6000 0],
    "3.1" 93  983040   [10000 0]       40  3  3  33177600   [10000 0],
    "4"   120 2228224  [12000 30000]   75  5  5  66846720   [12000 30000],
    "4.1" 123 2228224  [20000 50000]   75  5  5  133693440  [20000 50000],
    "5"   150 8912896  [25000 100000]  200 11 10 267386880  [25000 100000],
    "5.1" 153 8912896  [40000 160000]  200 11 10 534773760  [40000 160000],
    "5.2" 156 8912896  [60000 240000]  200 11 10 1069547520 [60000 240000],
    "6"   180 35651584 [60000 240000]  600 22 20 1069547520 [60000 240000],
    "6.1" 183 35651584 [120000 480000] 600 22 20 2139095040 [120000 480000],
    "6.2" 186 35651584 [240000 800000] 600 22 20 4278190080 [240000 800000],
];

/// The row of general_level_idc `idc`, if it names one of Table A.8's levels.
pub fn level(idc: u8) -> Option<&'static Level> {
    LEVELS.iter().find(|l| l.idc == idc)
}

/// A.4.2 MaxDpbSize of `sps` at level `lvl`: maxDpbPicBuf is 6, or 7 for the screen content coding profiles
/// (general_profile_idc 9 or 11, or their general_profile_compatibility_flag), scaled up for pictures well below
/// MaxLumaPs, at most 16.
pub fn max_dpb_size(sps: &Sps, lvl: &Level) -> u32 {
    let p = &sps.ptl;
    // general_profile_compatibility_flag[j] is bit 31 - j of the 32 read MSB first.
    let compat = |j: u32| p.general_profile_compatibility_flags & (1 << (31 - j)) != 0;
    let scc = matches!(p.general_profile_idc, 9 | 11) || compat(9) || compat(11);
    let buf: u64 = if scc { 7 } else { 6 };
    let ps = sps.width as u64 * sps.height as u64;
    let max = lvl.max_luma_ps as u64;
    let size = if ps <= max >> 2 {
        4 * buf
    } else if ps <= max >> 1 {
        2 * buf
    } else if ps <= (3 * max) >> 2 {
        4 * buf / 3
    } else {
        buf
    };
    size.min(16) as u32
}

/// A.4.1's limits on an active SPS / PPS pair against the level of the SPS's profile_tier_level, refused by name
/// ([`Error::Unsupported`], "HEVC-LEVEL"): an unknown general_level_idc, the High tier below level 4,
/// PicSizeInSamplesY above MaxLumaPs, a width or height above Sqrt(MaxLumaPs * 8), sps_max_dec_pic_buffering_minus1 + 1
/// of the highest sub-layer above MaxDpbSize, more tile columns or rows than MaxTileCols / MaxTileRows.
pub fn check_level(sps: &Sps, pps: &Pps) -> Result<()> {
    let idc = sps.ptl.general_level_idc;
    if idc == 255 {
        return Ok(());
    }
    let lvl = level(idc).ok_or(Error::Unsupported("unknown general_level_idc (HEVC-LEVEL)"))?;
    if sps.ptl.general_tier_flag && lvl.max_br[1] == 0 {
        return Err(Error::Unsupported("high tier below level 4 (HEVC-LEVEL)"));
    }
    let (w, h, max) = (sps.width as u64, sps.height as u64, lvl.max_luma_ps as u64);
    if w * h > max {
        return Err(Error::Unsupported(lvl.refusal));
    }
    if w * w > 8 * max || h * h > 8 * max {
        return Err(Error::Unsupported("picture width or height above its level (Sqrt(8 x MaxLumaPs), HEVC-LEVEL)"));
    }
    if sps.highest_ordering().max_dec_pic_buffering_minus1 as u64 + 1 > max_dpb_size(sps, lvl) as u64 {
        return Err(Error::Unsupported("DPB above its level (MaxDpbSize, HEVC-LEVEL)"));
    }
    if pps.num_tile_columns > lvl.max_tile_cols || pps.num_tile_rows > lvl.max_tile_rows {
        return Err(Error::Unsupported("tiles above its level (MaxTileCols / MaxTileRows, HEVC-LEVEL)"));
    }
    Ok(())
}
