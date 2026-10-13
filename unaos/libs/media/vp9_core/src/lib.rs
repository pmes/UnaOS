// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VP9CORE (LEDGER SR116): a VP9 decoder written from the VP9 Bitstream & Decoding Process
//! Specification (v0.6, 2016-03-31), `no_std` + `alloc`, no dependencies.
//!
//! CHARTER: Stria (A/V) — seam `shared-core` (a `no_std` lib both rings link, beside vp8_core and
//! av1_core).
//!
//! | module            | spec                                                              |
//! |-------------------|-------------------------------------------------------------------|
//! | [`bits`]          | §4.9 f(n), su(n)                                                  |
//! | [`bool_decoder`]  | §9.2 boolean decoder, T, L(n), §6.3.5 diff_update_prob            |
//! | [`header`]        | §6.2 uncompressed header, §6.3 compressed header, Annex B index   |
//! | [`probs`]         | §10.5 defaults, §8.4 backward adaptation                          |
//! | [`modeinfo`]      | §6.4 mode info + §9.3 contexts, §6.4.22 MV prediction             |
//! | [`decoder`]       | §8 frame decode: refs, tiles, partitions, tokens, reconstruction  |
//! | [`intra`]         | §8.5.1 intra prediction                                           |
//! | [`inter`]         | §8.5.2 inter prediction, reference scaling, 8-tap filters         |
//! | [`transform`]     | §8.7 inverse DCT/ADST/WHT and reconstruction                      |
//! | [`loopfilter`]    | §8.8 loop filter                                                  |
//! | [`tables`]        | generated constant tables (tools/gen_tables.py)                   |
//!
//! Decoded: profile 0 (8-bit 4:2:0) and profile 2 (10- and 12-bit 4:2:0); key, inter, intra-only
//! and error-resilient frames; superframes; show_existing_frame; frame-size changes with scaled
//! motion compensation from references up to 2x larger / 16x smaller; all four interpolation
//! filters; compound prediction; segmentation (map with temporal prediction, ALT_Q, ALT_LF,
//! REF_FRAME, SKIP); lossless (WHT); tiles (up to 64 columns, 4 rows); frame-parallel and
//! error-resilient context handling; the four saved frame contexts with backward adaptation.
//!
//! Refused by name ([`Error::Unsupported`]): profiles 1 and 3 (4:2:2, 4:4:0, 4:4:4, the alpha
//! extension). Not attempted: error concealment of corrupt frames (a corrupt frame is an error),
//! frames concatenated without a superframe index (the chunk decodes as one frame), and the
//! reference decoder's post-processing (not part of decoding).

#![no_std]
#![forbid(unsafe_code)]
// The decoder indexes arrays the way the spec's pseudo-code does; iterator rewrites would hide that.
#![allow(clippy::needless_range_loop, clippy::too_many_arguments)]

extern crate alloc;

use alloc::vec::Vec;
#[cfg(feature = "std")]
extern crate std;

pub mod bits;
pub mod bool_decoder;
pub mod consts;
pub mod decoder;
pub mod frame;
pub mod header;
pub mod inter;
pub mod intra;
pub mod loopfilter;
pub mod modeinfo;
pub mod probs;
pub mod tables;
pub mod transform;

/// Every way a decode can fail.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Ran out of bytes.
    Truncated,
    /// The bitstream violates the specification.
    Invalid(&'static str),
    /// A valid stream this decoder refuses by name (profiles 1 and 3: 4:2:2 / 4:4:0 / 4:4:4).
    Unsupported(&'static str),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Truncated => f.write_str("vp9: truncated"),
            Error::Invalid(s) => write!(f, "vp9: invalid ({s})"),
            Error::Unsupported(s) => write!(f, "vp9: unsupported ({s})"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

pub type Result<T> = core::result::Result<T, Error>;

pub use decoder::Decoder;
pub use header::superframe_frames;

/// A decoded picture, cropped to the frame size: three planes of `u16` samples (8-bit streams use
/// 0..=255), chroma subsampled by `ss_x` / `ss_y` with odd sizes rounded up.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    /// The display size the stream asks for (render_size, §7.2.5); the samples are width x height.
    pub render_width: u32,
    pub render_height: u32,
    pub bit_depth: u8,
    pub ss_x: u8,
    pub ss_y: u8,
    /// color_space (§7.2.2: 0 unknown, 1 BT.601, 2 BT.709, 3 SMPTE-170, 4 SMPTE-240, 5 BT.2020,
    /// 6 reserved, 7 sRGB) and color_range (0 studio, 1 full).
    pub color_space: u8,
    pub color_range: u8,
    pub planes: [Vec<u16>; 3],
}

impl Frame {
    pub fn from_buf(b: &frame::FrameBuf) -> Frame {
        let crop = |p: usize| {
            let (w, h) = b.plane_size(p);
            let pl = &b.planes[p];
            let mut v = Vec::with_capacity(w * h);
            for r in 0..h {
                v.extend_from_slice(&pl.data[r * pl.stride..r * pl.stride + w]);
            }
            v
        };
        Frame {
            width: b.width,
            height: b.height,
            render_width: b.render_width,
            render_height: b.render_height,
            bit_depth: b.bit_depth,
            ss_x: b.ss_x,
            ss_y: b.ss_y,
            color_space: b.color_space,
            color_range: b.color_range,
            planes: [crop(0), crop(1), crop(2)],
        }
    }

    /// Width and height of plane `p`.
    pub fn plane_size(&self, p: usize) -> (usize, usize) {
        if p == 0 {
            (self.width as usize, self.height as usize)
        } else {
            ((self.width as usize + self.ss_x as usize) >> self.ss_x, (self.height as usize + self.ss_y as usize) >> self.ss_y)
        }
    }

    /// Planes reduced to 8 bits (`>> (bit_depth - 8)`), for display.
    pub fn to_8bit(&self) -> [Vec<u8>; 3] {
        let s = self.bit_depth - 8;
        let f = |p: &Vec<u16>| p.iter().map(|&v| (v >> s) as u8).collect::<Vec<u8>>();
        [f(&self.planes[0]), f(&self.planes[1]), f(&self.planes[2])]
    }

    /// The bytes the reference decoder's `--md5` hashes: Y, U, V rows of 8-bit samples, or of
    /// 16-bit little-endian samples for 10/12-bit streams.
    pub fn md5_layout(&self) -> Vec<u8> {
        let mut out = Vec::new();
        for p in &self.planes {
            if self.bit_depth == 8 {
                out.extend(p.iter().map(|&v| v as u8));
            } else {
                for &v in p {
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        out
    }
}
