// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VP8CORE (LEDGER SR40): a VP8 decoder written from RFC 6386 ("VP8 Data Format and Decoding
//! Guide", 2011), `no_std` + `alloc`, no dependencies.
//!
//! Covered: the boolean entropy decoder (§7); the frame tag, key-frame start code and dimensions
//! (§9.1, §19.1); the frame header (§9.2–§9.11, §19.2) — segmentation with map and per-segment
//! quantiser / filter-level data, loop-filter type, level, sharpness and the reference / mode
//! deltas, 1/2/4/8 token partitions, the six quantiser indices, golden / alt-ref refresh, copy and
//! sign-bias flags, `refresh_entropy_probs` save/restore, token-probability updates (§13.4), the
//! skip flag; per-macroblock modes (§11, §19.3) — key-frame contextual sub-block modes, inter-frame
//! intra modes with their updatable probabilities, reference-frame selection, the near-MV search
//! with sign bias and clamping (§16.3), NEAREST / NEAR / ZERO / NEW / SPLIT (16×8, 8×16, 8×8, 4×4
//! partitions with LEFT / ABOVE / ZERO / NEW sub-vectors, §16.4), motion-vector decoding with
//! updatable probabilities (§17); DCT tokens with the four block types, bands and contexts (§13);
//! dequantisation (§14.1); the inverse WHT and DCT (§14.3, §14.4); all intra predictors — 16×16
//! and chroma DC / V / H / TM with the §12.2 edge rules and the ten 4×4 sub-block modes (§12.3);
//! inter prediction from last / golden / alt-ref with the six-tap filters (§18.3) or bilinear
//! (versions 1–2) and full-pixel chroma (version 3), chroma vectors derived per §17.4; the normal
//! and simple loop filters with per-macroblock levels (§15); the reference-buffer updates (§9.7).
//!
//! Output is the macroblock-aligned I420 picture cropped to the frame size; [`yuv`] converts it to
//! RGBA the way libwebp does (BT.601 limited range, "fancy" chroma upsampling), which is what a
//! browser shows for a lossy WebP.
//!
//! NOT decoded: the horizontal / vertical scale bits are reported but not applied (they are a
//! display hint); `color_space` = 1 (reserved) is decoded as 0; corrupt-partition concealment is
//! not attempted (a truncated partition decodes on zeros, as the reference decoder does).

#![no_std]
#![forbid(unsafe_code)]
// The decoder indexes arrays the way the RFC's pseudo-code does; iterator rewrites would hide that.
#![allow(clippy::needless_range_loop)]

extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

mod bool_decoder;
pub mod consts;
mod decoder;
mod idct;
mod loopfilter;
mod predict;
mod tables;
pub mod yuv;

pub use decoder::{Decoder, FrameTag, Mv, Picture, parse_tag};

use alloc::vec::Vec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    Truncated,
    Malformed(&'static str),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Truncated => f.write_str("vp8: truncated"),
            Error::Malformed(s) => write!(f, "vp8: malformed ({s})"),
        }
    }
}

#[cfg(feature = "std")]
impl std::error::Error for Error {}

pub type Result<T> = core::result::Result<T, Error>;

/// An owned, cropped I420 picture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Yuv420 {
    pub width: u32,
    pub height: u32,
    /// `width × height`.
    pub y: Vec<u8>,
    /// `ceil(width/2) × ceil(height/2)` each.
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl Yuv420 {
    pub fn chroma_width(&self) -> u32 {
        self.width.div_ceil(2)
    }
    pub fn from_picture(p: &Picture) -> Yuv420 {
        let (w, h) = (p.width as usize, p.height as usize);
        let (cw, ch) = (p.chroma_width() as usize, p.chroma_height() as usize);
        let crop = |src: &[u8], stride: usize, w: usize, h: usize| {
            let mut v = Vec::with_capacity(w * h);
            for r in 0..h {
                v.extend_from_slice(&src[r * stride..r * stride + w]);
            }
            v
        };
        Yuv420 {
            width: p.width,
            height: p.height,
            y: crop(p.y, p.y_stride, w, h),
            u: crop(p.u, p.uv_stride, cw, ch),
            v: crop(p.v, p.uv_stride, cw, ch),
        }
    }
}

/// Decode a single key frame (a WebP `VP8 ` chunk payload) to an owned I420 picture.
pub fn decode_key_frame(data: &[u8]) -> Result<Yuv420> {
    let tag = parse_tag(data)?;
    if !tag.key_frame {
        return Err(Error::Malformed("vp8: not a key frame"));
    }
    let mut d = Decoder::new();
    let pic = d.decode(data)?.ok_or(Error::Malformed("vp8: key frame not shown"))?;
    Ok(Yuv420::from_picture(&pic))
}

/// [`decode_key_frame`] under libwebp's contract (ANIMWEBP): a frame whose partition 0 or token
/// partition runs out before the last macroblock is refused as [`Error::Truncated`], as libwebp
/// refuses it ("Premature end-of-file encountered") and Blink then fails that WebP image or frame.
pub fn decode_key_frame_strict(data: &[u8]) -> Result<Yuv420> {
    let tag = parse_tag(data)?;
    if !tag.key_frame {
        return Err(Error::Malformed("vp8: not a key frame"));
    }
    let mut d = Decoder::new();
    let pic = d.decode(data)?.ok_or(Error::Malformed("vp8: key frame not shown"))?;
    let yuv = Yuv420::from_picture(&pic);
    if d.overran() {
        return Err(Error::Truncated);
    }
    Ok(yuv)
}
