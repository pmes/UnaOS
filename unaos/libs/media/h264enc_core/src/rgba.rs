// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! NLE6EXPORT (rmbp-ledger B959): H264ENC's RGBA face — the ONE frame-to-encoder glue the editor's two exports drive
//! (Stria's `nle_export::H264Encoder` on the host, the kernel's `nle export` in `video/export.rs`).
//!
//! [`RgbaEncoder`] holds a [`Config`] whose `width` / `height` are taken from the FIRST frame, builds the [`Encoder`] then,
//! converts every straight-RGBA frame through the tree's one RGBA -> I420 ([`I420::from_rgba`], BT.601 limited) and
//! codes it with [`Encoder::push`] (so B pictures reorder, SR333 / SR349); [`RgbaEncoder::flush`] drains the held frames.
//! Each access unit comes out as a [`Unit`]: its avcC sample ([`Access::avcc_sample`]) and its (dts, pts) in frames, the
//! shape an MP4 track takes per sample. A later frame of another size is refused (`Error::FrameSize`). Moved here from
//! Stria's `H264Encoder::encode` / `flush` / `h264_units`: the stream is byte for byte the one Stria wrote.

use alloc::vec::Vec;

use crate::yuv::I420;
use crate::{Access, Config, Encoder, Result};

/// One coded access unit as an MP4 sample: an IDR flag, the avcC (length-prefixed) bytes, and its decoding and
/// presentation times in frames (`dts <= pts`; equal without B pictures).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub keyframe: bool,
    pub sample: Vec<u8>,
    pub dts: i64,
    pub pts: i64,
}

impl Unit {
    fn of(au: Access) -> Unit {
        Unit { keyframe: au.keyframe, sample: au.avcc_sample(), dts: au.dts as i64, pts: au.pts as i64 }
    }
}

/// RGBA frames in output order in, [`Unit`]s in decoding order out.
pub struct RgbaEncoder {
    /// The stream's parameters; read when the first frame arrives (its size fills `width` / `height`).
    pub cfg: Config,
    enc: Option<Encoder>,
}

impl RgbaEncoder {
    pub fn new(cfg: Config) -> RgbaEncoder {
        RgbaEncoder { cfg, enc: None }
    }

    /// The encoder has been built (a first frame was pushed).
    pub fn started(&self) -> bool {
        self.enc.is_some()
    }

    /// One `width x height` straight-RGBA frame: the units it releases (none while a B picture is held).
    pub fn push_rgba(&mut self, width: u32, height: u32, rgba: &[u8]) -> Result<Vec<Unit>> {
        if self.enc.is_none() {
            self.enc = Some(Encoder::new(Config { width, height, ..self.cfg })?);
        }
        let Some(enc) = self.enc.as_mut() else { return Ok(Vec::new()) };
        let yuv = I420::from_rgba(width, height, rgba)?;
        Ok(enc.push(&yuv.as_yuv420())?.into_iter().map(Unit::of).collect())
    }

    /// The end of the stream: the held frames' units (none before the first frame).
    pub fn flush(&mut self) -> Result<Vec<Unit>> {
        match self.enc.as_mut() {
            Some(enc) => Ok(enc.flush()?.into_iter().map(Unit::of).collect()),
            None => Ok(Vec::new()),
        }
    }

    /// The AVCDecoderConfigurationRecord (the `avcC` box body); empty before the first frame.
    pub fn avcc(&self) -> Vec<u8> {
        self.enc.as_ref().map(Encoder::avcc).unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(w: u32, h: u32, k: u32) -> Vec<u8> {
        (0..w * h).flat_map(|i| [((i + k * 7) % 251) as u8, ((i / w + k) * 3 % 256) as u8, (k * 40 % 256) as u8, 255]).collect()
    }

    /// The face is the hand-driven encoder: the same units (bytes, flags, times) and the same avcC, with and without B pictures.
    #[test]
    fn rgba_face_is_the_encoder() {
        for bframes in [0u8, 2] {
            let cfg = Config { qp: 26, gop: 6, bframes, ..Default::default() };
            let (w, h) = (32u32, 32u32);
            let mut face = RgbaEncoder::new(cfg);
            let mut hand = Encoder::new(Config { width: w, height: h, ..cfg }).unwrap();
            let (mut a, mut b) = (Vec::new(), Vec::new());
            for k in 0..7 {
                let rgba = frame(w, h, k);
                a.extend(face.push_rgba(w, h, &rgba).unwrap());
                let yuv = I420::from_rgba(w, h, &rgba).unwrap();
                b.extend(hand.push(&yuv.as_yuv420()).unwrap().into_iter().map(Unit::of));
            }
            a.extend(face.flush().unwrap());
            b.extend(hand.flush().unwrap().into_iter().map(Unit::of));
            assert_eq!(a, b);
            assert_eq!(a.len(), 7);
            assert_eq!(face.avcc(), hand.avcc());
            assert_eq!(a.iter().any(|u| u.pts != u.dts), bframes > 0);
        }
    }
}
