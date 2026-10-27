// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! NLE6EXPORT (rmbp-ledger B959): H264ENC's RGBA face — the ONE frame-to-encoder glue the editor's two exports drive
//! (Stria's `nle_export::H264Encoder` on the host, the kernel's `nle export` in `video/export.rs`).
//!
//! [`RgbaEncoder`] holds a [`Config`] whose `width` / `height` are taken from the FIRST frame, builds the [`Encoder`] then,
//! converts every straight-RGBA frame through the tree's one RGBA -> I420 ([`I420::from_rgba`], BT.601 limited; any matrix by
//! [`RgbaEncoder::push_rgba_in`]) and codes it with [`Encoder::push`] (so B pictures reorder, SR333 / SR349) through the ONE
//! planar entry [`RgbaEncoder::push_planes`] (H264ENCPLANES rmbp-ledger B973: the kernel export hands an unmixed 8-bit 4:2:0
//! decoder picture to it directly, no RGBA round trip); [`RgbaEncoder::flush`] drains the held frames.
//! Each access unit comes out as a [`Unit`]: its avcC sample ([`Access::avcc_sample`]) and its (dts, pts) in frames, the
//! shape an MP4 track takes per sample. A later frame of another size is refused (`Error::FrameSize`). Moved here from
//! Stria's `H264Encoder::encode` / `flush` / `h264_units`: the stream is byte for byte the one Stria wrote.

use alloc::vec::Vec;

use crate::yuv::I420;
use crate::{Access, Config, Encoder, Result, Yuv420};

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

    /// One `width x height` straight-RGBA frame (BT.601 limited): the units it releases (none while a B picture is held).
    pub fn push_rgba(&mut self, width: u32, height: u32, rgba: &[u8]) -> Result<Vec<Unit>> {
        self.push_rgba_in(width, height, rgba, yuv_core::KR_KB_601, false)
    }

    /// One straight-RGBA frame converted by (Kr, Kb) x 10000 and range ([`I420::from_rgba_in`]), then [`Self::push_planes`].
    pub fn push_rgba_in(&mut self, width: u32, height: u32, rgba: &[u8], kr_kb: (i64, i64), full: bool) -> Result<Vec<Unit>> {
        let yuv = I420::from_rgba_in(width, height, rgba, kr_kb, full)?;
        self.push_planes(&yuv.as_yuv420())
    }

    /// THE planar entry (H264ENCPLANES B973): one 8-bit 4:2:0 picture, the encoder built from its size at the first frame.
    pub fn push_planes(&mut self, f: &Yuv420) -> Result<Vec<Unit>> {
        if self.enc.is_none() {
            self.enc = Some(Encoder::new(Config { width: f.width, height: f.height, ..self.cfg })?);
        }
        let Some(enc) = self.enc.as_mut() else { return Ok(Vec::new()) };
        Ok(enc.push(f)?.into_iter().map(Unit::of).collect())
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
    use alloc::vec;

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

    /// H264ENCPLANES (B973): the RGBA face IS the planar entry fed by `from_rgba`: the same units and avcC, with and without B.
    #[test]
    fn planes_entry_is_the_rgba_path() {
        for bframes in [0u8, 2] {
            let cfg = Config { qp: 26, gop: 6, bframes, ..Default::default() };
            let (w, h) = (32u32, 32u32);
            let (mut by_rgba, mut by_planes) = (RgbaEncoder::new(cfg), RgbaEncoder::new(cfg));
            let (mut a, mut b) = (Vec::new(), Vec::new());
            for k in 0..7 {
                let rgba = frame(w, h, k);
                a.extend(by_rgba.push_rgba(w, h, &rgba).unwrap());
                b.extend(by_planes.push_planes(&I420::from_rgba(w, h, &rgba).unwrap().as_yuv420()).unwrap());
            }
            a.extend(by_rgba.flush().unwrap());
            b.extend(by_planes.flush().unwrap());
            assert_eq!(a, b);
            assert_eq!(by_rgba.avcc(), by_planes.avcc());
        }
    }

    /// H264ENCPLANES (B973): a flat 8-bit 4:2:0 picture and its RGBA (`yuv_core::to_rgba`, BT.601 limited, as the kernel's
    /// `Px::rgb` makes it) enter the encoder within 1 per sample (`I420::from_rgba` of that RGBA), and the RGBA face codes exactly
    /// what the planar entry codes from those samples.
    #[test]
    fn flat_planes_within_one_of_their_rgba() {
        let (w, h) = (32usize, 16usize);
        for (yy, uu, vv) in [(16u8, 128u8, 128u8), (235, 128, 128), (81, 90, 240), (145, 54, 34), (110, 160, 100), (200, 100, 140)] {
            let (py, pu, pv) = (vec![yy; w * h], vec![uu; w * h / 4], vec![vv; w * h / 4]);
            let p = yuv_core::Picture { w, h, y: &py[..], u: &pu[..], v: &pv[..], y_stride: w, uv_stride: w / 2, ss: (1, 1), mono: false, bit_depth: 8, full: false };
            let mut rgba = vec![0u8; w * h * 4];
            yuv_core::to_rgba(&p, yuv_core::Matrix::Kr(yuv_core::KR_KB_601), yuv_core::Chroma::Nearest, false, &mut rgba);
            let back = I420::from_rgba(w as u32, h as u32, &rgba).unwrap();
            let near = |a: &[u8], b: &[u8]| a.iter().zip(b).all(|(x, y)| x.abs_diff(*y) <= 1);
            assert!(near(&back.y, &py) && near(&back.u, &pu) && near(&back.v, &pv), "({}, {}, {}) -> {} {} {}", yy, uu, vv, back.y[0], back.u[0], back.v[0]);
            let cfg = Config { qp: 26, gop: 4, ..Default::default() };
            let (mut by_rgba, mut by_planes) = (RgbaEncoder::new(cfg), RgbaEncoder::new(cfg));
            let mut a = by_rgba.push_rgba(w as u32, h as u32, &rgba).unwrap();
            let mut b = by_planes.push_planes(&back.as_yuv420()).unwrap();
            a.extend(by_rgba.flush().unwrap());
            b.extend(by_planes.flush().unwrap());
            assert_eq!(a, b);
        }
    }
}
