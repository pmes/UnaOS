// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria — shared-core (NLEPROBEHDR, LEDGER SR226: a track's colour description and HDR metadata).
//!
//! ONE type for what a file says about its colour: the ITU-T H.273 code points (colour primaries, transfer
//! characteristics, matrix coefficients; 2 = unspecified), the quantisation range, and the HDR static metadata
//! (SMPTE ST 2086 mastering display colour volume, CTA-861.3 content light levels). The container's statement is read at
//! open (MP4 `colr` / `mdcv` / `clli` in the visual sample entry, Matroska `Colour` in the Video master) into
//! [`crate::Track::colour`]; the bitstream's (H.264 SPS VUI, AV1 sequence header color_config and METADATA OBUs) is the
//! fallback, handed in by the caller through [`crate::Demuxer::fill_colour`] (demux_core keeps no codec dependency, the
//! [`crate::Demuxer::derive_dts`] shape). The container wins: [`Colour::or`].
//!
//! Units of [`Mastering`] are ST 2086's, as ISO `mdcv` stores them: chromaticity x, y in 0.00002, luminance in
//! 0.0001 cd/m²; primaries are ordered R, G, B (MP4 `mdcv` stores G, B, R and is reordered on read; Matroska's floats
//! and AV1's fixed-point values are converted, [`Mastering::from_av1`]).

use core::fmt;

use crate::{Demuxer, Track, TrackKind};

/// A track's colour description (H.273 code points) and HDR static metadata.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Colour {
    /// H.273 ColourPrimaries (1 BT.709, 9 BT.2020, 2 unspecified).
    pub primaries: u8,
    /// H.273 TransferCharacteristics (1 BT.709, 16 PQ / ST 2084, 18 HLG, 2 unspecified).
    pub transfer: u8,
    /// H.273 MatrixCoefficients (1 BT.709, 9 BT.2020 NCL, 0 identity, 2 unspecified).
    pub matrix: u8,
    /// `Some(true)` full range, `Some(false)` limited (video) range, `None` when the source does not say.
    pub full_range: Option<bool>,
    pub mastering: Option<Mastering>,
    pub light: Option<Light>,
}

/// SMPTE ST 2086 mastering display colour volume, in ISO `mdcv` units (see the module doc).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mastering {
    /// (x, y) of the R, G, B primaries, in 0.00002.
    pub primaries: [[u16; 2]; 3],
    /// (x, y) of the white point, in 0.00002.
    pub white: [u16; 2],
    /// Maximum and minimum display mastering luminance, in 0.0001 cd/m².
    pub max_lum: u32,
    pub min_lum: u32,
}

/// CTA-861.3 content light levels, in cd/m².
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Light {
    pub max_cll: u16,
    pub max_fall: u16,
}

impl Colour {
    /// Container wins: each of `self`'s code points stays unless it is 2 (unspecified), and the range, mastering and
    /// light levels are taken from `fallback` only where `self` has none.
    pub fn or(self, fallback: Colour) -> Colour {
        let pick = |a: u8, b: u8| if a == 2 { b } else { a };
        Colour {
            primaries: pick(self.primaries, fallback.primaries),
            transfer: pick(self.transfer, fallback.transfer),
            matrix: pick(self.matrix, fallback.matrix),
            full_range: self.full_range.or(fallback.full_range),
            mastering: self.mastering.or(fallback.mastering),
            light: self.light.or(fallback.light),
        }
    }
}

impl Mastering {
    /// From AV1's METADATA_TYPE_HDR_MDCV (AV1 §5.8.3, §6.7.4): chromaticity 0.16 fixed point (R, G, B order), luminance_max
    /// 24.8 and luminance_min 18.14 fixed point, in cd/m² — rounded to ST 2086 units.
    pub fn from_av1(primaries: [[u16; 2]; 3], white: [u16; 2], luminance_max: u32, luminance_min: u32) -> Mastering {
        let xy = |v: u16| ((v as u64 * 50_000 + 32_768) >> 16) as u16;
        Mastering {
            primaries: primaries.map(|p| [xy(p[0]), xy(p[1])]),
            white: [xy(white[0]), xy(white[1])],
            max_lum: ((luminance_max as u64 * 10_000 + 128) >> 8) as u32,
            min_lum: ((luminance_min as u64 * 10_000 + 8_192) >> 14) as u32,
        }
    }
}

/// The probe's token: `colour=<p>/<t>/<m>[/full|limited][ mdcv][ clli]`.
impl fmt::Display for Colour {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "colour={}/{}/{}", self.primaries, self.transfer, self.matrix)?;
        match self.full_range {
            Some(true) => f.write_str("/full")?,
            Some(false) => f.write_str("/limited")?,
            None => {}
        }
        if self.mastering.is_some() {
            f.write_str(" mdcv")?;
        }
        if self.light.is_some() {
            f.write_str(" clli")?;
        }
        Ok(())
    }
}

impl Demuxer {
    /// NLEPROBEHDR (SR226): the bitstream's colour as the fallback for what the container did not say. `colour_of` is the
    /// codec core's reading of a video track's configuration (the H.264 SPS VUI from `avcC`, the AV1 sequence header and
    /// METADATA OBUs from `av1C`), injected by the caller as [`Demuxer::derive_dts`]'s depth is. A track with no container
    /// colour takes it whole; one with a container colour keeps it and fills only its gaps ([`Colour::or`]). Returns how
    /// many tracks changed.
    pub fn fill_colour(&mut self, colour_of: impl Fn(&Track) -> Option<Colour>) -> usize {
        let mut n = 0;
        for t in self.tracks.iter_mut() {
            if t.kind != TrackKind::Video {
                continue;
            }
            let Some(b) = colour_of(t) else { continue };
            let next = match t.colour {
                Some(c) => c.or(b),
                None => b,
            };
            if t.colour != Some(next) {
                t.colour = Some(next);
                n += 1;
            }
        }
        n
    }
}
