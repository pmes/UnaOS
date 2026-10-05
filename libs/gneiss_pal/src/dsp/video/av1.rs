// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `dsp::video::av1` — the host face of AVCODEC (LEDGER SR24, `unaos/libs/media/av1_core`), the
//! from-specification AV1 decoder, behind PLAYBACK's [`VideoDecoder`] seam (SR26).
//!
//! Fold wiring (this file is written against PLAYBACK's `dsp/video.rs`, which declares
//! `#[cfg(feature = "av1")] pub mod av1;` and the registry arm `Codec::Av1 => Av1Decoder::new`):
//! in `libs/gneiss_pal/Cargo.toml` set `av1 = ["dep:av1_core"]` and add
//! `av1_core = { path = "../../unaos/libs/media/av1_core", optional = true, features = ["std"] }`.
//!
//! Honest ceiling (AVCODEC2): key, intra-only, inter and switch frames, show_existing_frame,
//! intra block copy and scalable (temporal / spatial layer) streams decode frame-exact against
//! libaom's per-frame MD5s (docs/dev/evidence/media-1004/AVCODEC2.md); superres returns
//! `DecodeError::Unsupported`, and film grain synthesis is reported in that doc. The decoder keeps
//! the reference frames across packets; `reset()` (a seek) drops them. Frames leave as RGBA converted with
//! the stream's own matrix / range (BT.601/709/2020, limited or full), any bit depth and
//! subsampling, never a guessed BT.601.

use super::{DecodeError, Frame, Pixels, VideoDecoder};
use crate::dsp::demux::{Packet, Track};
use av1_core::image::{planes_to_rgba, StreamDecoder, Upsampling};

pub struct Av1Decoder {
    inner: StreamDecoder,
    timebase: crate::dsp::demux::Timebase,
}

fn map_err(e: av1_core::Error) -> DecodeError {
    match e {
        av1_core::Error::Unsupported(s) => DecodeError::Unsupported(format!("AV1 {s} (AVCODEC: owed)")),
        other => DecodeError::Corrupt(format!("AV1: {other}")),
    }
}

impl Av1Decoder {
    /// `track.config` is the av1C record (MP4) or CodecPrivate (Matroska); empty is allowed when
    /// the stream carries its own sequence header.
    pub fn new(track: &Track) -> Result<Av1Decoder, DecodeError> {
        Ok(Av1Decoder { inner: StreamDecoder::new(&track.config).map_err(map_err)?, timebase: track.timebase })
    }
}

impl VideoDecoder for Av1Decoder {
    fn name(&self) -> &'static str {
        "av1 (AVCODEC)"
    }
    fn decode(&mut self, pkt: &Packet) -> Result<Option<Frame>, DecodeError> {
        let planes = self.inner.decode_temporal_unit(&pkt.data).map_err(map_err)?;
        let img = planes_to_rgba(&planes, Upsampling::Bilinear);
        Ok(Some(Frame { width: img.w, height: img.h, pts_ns: self.timebase.to_ns(pkt.pts), pixels: Pixels::Rgba(img.rgba) }))
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
}
