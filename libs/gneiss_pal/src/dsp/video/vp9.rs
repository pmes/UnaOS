// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The VP9 adapter (VP9CORE, LEDGER SR116): `vp9_core`, UnaOS's from-specification VP9 decoder
//! (profiles 0 and 2), behind the [`VideoDecoder`] seam. Matroska/WebM `V_VP9` and MP4 `vp09` carry
//! one chunk per block / sample — a frame or a superframe (a hidden alt-ref plus the shown frame);
//! the decoder presents at most one picture per chunk, in presentation order, so there is nothing
//! to reorder and `flush` has nothing to drain. 10- and 12-bit streams are shown reduced to 8 bits
//! (the sink is 8-bit I420). Profiles 1 and 3 (4:2:2 / 4:4:0 / 4:4:4) are refused at the first
//! packet with the decoder's own reason, and the player keeps its labelled stand-in. VP9INBANDSWITCH (SR400): a later key
//! (or intra-only) frame that states another color_space / range or size is reported and re-picked by the ONE host step
//! (`super::inband_frame`, AV1's), VP9's nearest chroma; the open's colour and the switch state survive a seek.

use super::{DecodeError, Frame, Pixels, VideoDecoder};
use crate::dsp::demux::{Packet, Timebase, Track};

pub struct Vp9Decoder {
    dec: vp9_core::Decoder,
    timebase: Timebase,
    open: Option<crate::dsp::demux::colour::Colour>, // VP9INBANDSWITCH (SR400): the open's colour (`Track::colour`), what a key-frame switch merges under
    inband: yuv_core::inband::InBand, // VP9INBANDSWITCH (SR400): the ONE switch state, kept across `reset` (a seek)
}

impl Vp9Decoder {
    pub fn new(track: &Track) -> Result<Vp9Decoder, DecodeError> {
        // vpcC (MP4) / CodecPrivate (WebM) carry profile and bit depth; the bitstream's own
        // header is authoritative, so a profile the decoder refuses is reported at decode time.
        Ok(Vp9Decoder { dec: vp9_core::Decoder::new(), timebase: track.timebase, open: track.colour, inband: Default::default() })
    }
}

impl VideoDecoder for Vp9Decoder {
    fn name(&self) -> &'static str {
        "vp9 (vp9_core)"
    }

    fn decode(&mut self, pkt: &Packet) -> Result<Option<Frame>, DecodeError> {
        if pkt.data.is_empty() {
            return Ok(None);
        }
        let pts_ns = self.timebase.to_ns(pkt.pts);
        let f = match self.dec.decode(&pkt.data) {
            Ok(Some(f)) => f,
            Ok(None) => return Ok(None),
            Err(e @ vp9_core::Error::Unsupported(_)) => return Err(DecodeError::Unsupported(format!("{e}"))),
            Err(e) => return Err(DecodeError::Corrupt(format!("{e}"))),
        };
        if f.ss_x != 1 || f.ss_y != 1 {
            return Err(DecodeError::Unsupported(format!("vp9: chroma subsampling {}x{} (4:2:0 only)", f.ss_x, f.ss_y)));
        }
        let [y, u, v] = f.to_8bit();
        let (cw, _) = f.plane_size(1);
        let frame = Frame { width: f.width, height: f.height, pts_ns, pixels: Pixels::I420 { y, u, v, y_stride: f.width as usize, uv_stride: cw } };
        let now = vp9_core::header::colour_codes(f.color_space, f.color_range); // VP9INBANDSWITCH (SR400): the picture's own colour (CS_UNKNOWN: none, the last holds)
        Ok(Some(super::inband_frame(&mut self.inband, now, self.open, super::Chroma::Nearest, frame)))
    }

    fn reset(&mut self) {
        self.dec.reset();
    }
}
