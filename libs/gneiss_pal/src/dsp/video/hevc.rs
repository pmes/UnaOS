// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Stria (A/V) — seam `shared-core` consumer: hevc_core (HEVCPARSE SR199) in the one video registry.
//!
//! The H.265 / HEVC adapter (HEVCPAL, LEDGER SR215): `hevc_core`, UnaOS's from-specification HEVC decoder (8-bit
//! 4:2:0 pictures made of I slices), behind the [`VideoDecoder`] seam in [`super::h264::H264Decoder`]'s shape. MP4
//! `hvc1` / `hev1` and Matroska `V_MPEGH/ISO/HEVC` carry one access unit per sample in the length-prefixed framing of
//! ISO/IEC 14496-15, configured by the HEVCDecoderConfigurationRecord demux_core leaves in `Track::config` (`hvcC` /
//! CodecPrivate); a track without one is taken to carry Annex B byte-stream samples. Frames leave in output (POC)
//! order stamped with the pts of the sample they were decoded from; hevc_core's `Frame` is h264_core's, so the
//! conversion is [`super::h264::to_frame`]. Streams the decoder refuses (P / B slices until HEVCINTER, bit depth above
//! 8, 4:2:2 / 4:4:4, range extensions) are `DecodeError::Unsupported` with the decoder's own reason.

use std::collections::VecDeque;

use super::h264::to_frame;
use super::{DecodeError, Frame, VideoDecoder};
use crate::dsp::demux::{Packet, Timebase, Track};

pub struct HevcDecoder {
    dec: hevc_core::Decoder,
    timebase: Timebase,
    /// Samples are length-prefixed (an hvcC was present) rather than Annex B.
    length_prefixed: bool,
    ready: VecDeque<Frame>,
}

fn map_err(e: hevc_core::Error) -> DecodeError {
    match e {
        hevc_core::Error::Unsupported(_) => DecodeError::Unsupported(format!("{e}")),
        _ => DecodeError::Corrupt(format!("{e}")),
    }
}

/// HEVCPAL (SR215): the reorder depth of an hvcC (what `Demuxer::derive_dts` takes through `dsp::video::reorder_depth`).
/// HEVCDEPTH-CORE (SR304): the one body is hevc_core's, beside its SPS parser; this adapter keeps no copy.
pub use hevc_core::ps::hvcc_reorder_depth;

impl HevcDecoder {
    pub fn new(track: &Track) -> Result<HevcDecoder, DecodeError> {
        let mut dec = hevc_core::Decoder::new();
        let length_prefixed = !track.config.is_empty();
        if length_prefixed {
            dec.configure_hvcc(&track.config).map_err(map_err)?;
        }
        Ok(HevcDecoder { dec, timebase: track.timebase, length_prefixed, ready: VecDeque::new() })
    }

    fn collect(&mut self) {
        while let Some(f) = self.dec.next_frame() {
            self.ready.push_back(to_frame(f));
        }
    }
}

impl VideoDecoder for HevcDecoder {
    fn name(&self) -> &'static str {
        "hevc (hevc_core)"
    }

    fn decode(&mut self, pkt: &Packet) -> Result<Option<Frame>, DecodeError> {
        if pkt.data.is_empty() {
            return Ok(self.ready.pop_front());
        }
        self.dec.set_tag(self.timebase.to_ns(pkt.pts));
        let r = if self.length_prefixed {
            self.dec.decode_sample(&pkt.data)
        } else {
            let r = self.dec.decode_annexb(&pkt.data);
            self.dec.end_access_unit();
            r
        };
        self.collect();
        r.map_err(map_err)?;
        Ok(self.ready.pop_front())
    }

    fn flush(&mut self) -> Vec<Frame> {
        self.dec.flush();
        self.collect();
        self.ready.drain(..).collect()
    }

    fn reset(&mut self) {
        self.dec.reset();
        self.ready.clear();
    }
}
