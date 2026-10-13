// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! H264CORE (LEDGER SR133): an H.264 / AVC decoder written from ITU-T Rec. H.264 ("Advanced video
//! coding for generic audiovisual services"; the decoding process is unchanged from the 05/2003
//! edition through the v14 editions for the profiles decoded here), `no_std` + `alloc`, no
//! dependencies.
//!
//! CHARTER: Stria (A/V) — seam `shared-core` (a `no_std` lib both rings link, beside vp8_core,
//! vp9_core and av1_core).
//!
//! | module            | Recommendation                                                       |
//! |-------------------|----------------------------------------------------------------------|
//! | [`bits`]          | 7.2 RBSP functions, 9.1 Exp-Golomb ue / se / te                      |
//! | [`nal`]           | 7.3.1 NAL unit, emulation prevention, Annex B byte stream, avcC      |
//! | [`cabac`]         | 9.3 CABAC: context init, arithmetic decoding engine, residual blocks |
//! | [`cavlc`]         | 9.2 CAVLC residual blocks                                            |
//! | [`ps`]            | 7.3.2 SPS / PPS (+ VUI E.1), scaling lists, 7.3.3 slice header       |
//! | [`picture`]       | decoded frames, motion field, per-macroblock neighbour facts         |
//! | [`mb`]            | 7.3.5 macroblock layer, 9.3.3.1 CABAC contexts, 9.2.1 nC             |
//! | [`transform`]     | 8.5 scaling, 4x4 / 8x8 inverse transforms, luma / chroma DC          |
//! | [`intra`]         | 8.3 Intra_4x4, Intra_8x8 (+ reference filtering), Intra_16x16, chroma |
//! | [`tables`]        | generated constant tables (tools/gen_tables.py)                      |

#![no_std]
#![forbid(unsafe_code)]
// The decoder indexes arrays the way the Recommendation's equations do; iterator rewrites would
// hide that.
#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

extern crate alloc;

#[cfg(feature = "std")]
extern crate std;

pub mod bits;
pub mod cabac;
pub mod cavlc;
pub mod intra;
pub mod mb;
pub mod nal;
pub mod picture;
pub mod ps;
pub mod tables;
pub mod transform;

/// Every way a decode can fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Ran out of bytes.
    Truncated,
    /// The bitstream violates the Recommendation.
    Invalid(&'static str),
    /// A valid stream this decoder refuses by name (interlace, FMO, 4:2:2 / 4:4:4, high bit
    /// depth, Extended-profile data partitioning and SP/SI slices).
    Unsupported(&'static str),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Truncated => f.write_str("h264: truncated"),
            Error::Invalid(s) => write!(f, "h264: invalid ({s})"),
            Error::Unsupported(s) => write!(f, "h264: unsupported ({s})"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

pub type Result<T> = core::result::Result<T, Error>;
