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
//! | [`inter`]         | 8.4 MV prediction, P_Skip, B direct, interpolation, weighting        |
//! | [`dpb`]           | 8.2.1 POC, 8.2.4 reference lists, 8.2.5 marking, C.4 output order    |
//! | [`deblock`]       | 8.7 deblocking filter                                                |
//! | [`tables`]        | generated constant tables (tools/gen_tables.py)                      |

#![no_std]
#![forbid(unsafe_code)]
// The decoder indexes arrays the way the Recommendation's equations do; iterator rewrites would
// hide that.
#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

extern crate alloc;

use alloc::vec::Vec;

#[cfg(feature = "std")]
extern crate std;

pub mod bits;
pub mod cabac;
pub mod cavlc;
pub mod deblock;
pub mod dpb;
pub mod inter;
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

/// A decoded frame in output order, cropped (7.4.2.1.1): 8-bit Y, Cb, Cr planes (4:2:0), each
/// row-major without padding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub planes: [Vec<u8>; 3],
    /// PicOrderCnt of the frame.
    pub poc: i32,
    /// The caller's tag of the access unit the frame was decoded from (`Decoder::set_tag`).
    pub tag: i64,
    /// VUI video_full_range_flag.
    pub full_range: bool,
}

impl Frame {
    /// The cropped planes of a decoded picture.
    pub fn from_picture(p: &picture::Picture) -> Frame {
        let (x0, y0, w, h) = p.crop;
        let crop = |pl: &picture::Plane, x0: usize, y0: usize, w: usize, h: usize| {
            let mut v = Vec::with_capacity(w * h);
            for r in 0..h {
                let o = (y0 + r) * pl.w + x0;
                v.extend_from_slice(&pl.data[o..o + w]);
            }
            v
        };
        let (x0, y0, w, h) = (x0 as usize, y0 as usize, w as usize, h as usize);
        Frame {
            width: w as u32,
            height: h as u32,
            planes: [
                crop(&p.planes[0], x0, y0, w, h),
                crop(&p.planes[1], x0 / 2, y0 / 2, w / 2, h / 2),
                crop(&p.planes[2], x0 / 2, y0 / 2, w / 2, h / 2),
            ],
            poc: p.poc,
            tag: p.tag,
            full_range: p.full_range,
        }
    }

    /// Width and height of plane `p`.
    pub fn plane_size(&self, p: usize) -> (usize, usize) {
        if p == 0 { (self.width as usize, self.height as usize) } else { (self.width as usize / 2, self.height as usize / 2) }
    }

    /// The planar I420 bytes (Y, then Cb, then Cr) — the layout of the reference decoder's output
    /// YUV file.
    pub fn i420(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(self.planes.iter().map(|p| p.len()).sum());
        for p in &self.planes {
            v.extend_from_slice(p);
        }
        v
    }
}
