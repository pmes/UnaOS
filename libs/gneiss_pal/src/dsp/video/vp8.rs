// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The VP8 adapter (VP8CORE, LEDGER SR40): `vp8_core`, UnaOS's from-specification RFC 6386 decoder,
//! behind the [`VideoDecoder`] seam. Matroska/WebM `V_VP8` (RFC 6386 frames, one per block, no
//! CodecPrivate); MP4 `vp08` is accepted the same way (its `vpcC` record carries nothing the
//! decoder needs). VP8 has no frame reordering: each packet is one frame in presentation order, and
//! an alt-ref frame (`show_frame` = 0) updates the references without presenting, so it returns
//! `Ok(None)` and `flush` has nothing to drain.

use super::{DecodeError, Frame, Pixels, VideoDecoder};
use crate::dsp::demux::{Packet, Timebase, Track};

pub struct Vp8Decoder {
    dec: vp8_core::Decoder,
    timebase: Timebase,
}

impl Vp8Decoder {
    pub fn new(track: &Track) -> Result<Vp8Decoder, DecodeError> {
        Ok(Vp8Decoder { dec: vp8_core::Decoder::new(), timebase: track.timebase })
    }
}

impl VideoDecoder for Vp8Decoder {
    fn name(&self) -> &'static str {
        "vp8 (vp8_core)"
    }

    fn decode(&mut self, pkt: &Packet) -> Result<Option<Frame>, DecodeError> {
        if pkt.data.is_empty() {
            // A zero-length frame is a dropped frame: nothing new to show.
            return Ok(None);
        }
        let pts_ns = self.timebase.to_ns(pkt.pts);
        let pic = match self.dec.decode(&pkt.data) {
            Ok(Some(p)) => p,
            Ok(None) => return Ok(None),
            Err(e) => return Err(DecodeError::Corrupt(format!("{e}"))),
        };
        let y = vp8_core::Yuv420::from_picture(&pic);
        let (y_stride, uv_stride) = (y.width as usize, y.chroma_width() as usize);
        Ok(Some(Frame { width: y.width, height: y.height, pts_ns, pixels: Pixels::I420 { y: y.y, u: y.u, v: y.v, y_stride, uv_stride } }))
    }

    fn reset(&mut self) {
        self.dec.reset();
    }
}
