// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The H.264 adapter (H264CORE, LEDGER SR133): `h264_core`, UnaOS's from-specification H.264 / AVC
//! decoder (Baseline, Main and High profiles at 8-bit 4:2:0), behind the [`VideoDecoder`] seam.
//! MP4 `avc1` / `avc3` and Matroska `V_MPEG4/ISO/AVC` carry one access unit per sample in the
//! length-prefixed framing of ISO/IEC 14496-15, configured by the AVCDecoderConfigurationRecord
//! demux_core leaves in `Track::config` (`avcC` / CodecPrivate); a track without one is taken to
//! carry Annex B byte-stream samples. H.264 reorders: a sample's picture may leave later, in
//! presentation order, stamped with the pts of the sample it was decoded from, and `flush` drains
//! the decoded picture buffer. Streams the decoder refuses (interlaced, FMO, 4:2:2 / 4:4:4, high
//! bit depth) are refused at the first packet with the decoder's own reason, and the player keeps
//! its labelled stand-in.

use std::collections::VecDeque;

use super::{DecodeError, Frame, Pixels, VideoDecoder};
use crate::dsp::demux::{Packet, Timebase, Track};

pub struct H264Decoder {
    dec: h264_core::Decoder,
    timebase: Timebase,
    /// Samples are length-prefixed (an avcC was present) rather than Annex B.
    length_prefixed: bool,
    ready: VecDeque<Frame>,
}

fn map_err(e: h264_core::Error) -> DecodeError {
    match e {
        h264_core::Error::Unsupported(_) => DecodeError::Unsupported(format!("{e}")),
        _ => DecodeError::Corrupt(format!("{e}")),
    }
}

/// HEVCREXTTOOLS (SR345): hevc_core's monochrome (`ChromaFormat::Mono`, empty Cb / Cr) as a grey 4:2:0 frame — flat
/// 128 chroma, and 1 << (bit_depth - 1) in `planes16` — THE one copy of that fill, for the Player and the HEIC tile.
/// COLOURONE422 (SR359): the fill is at the luma's depth, so the frame's chroma depth becomes the luma's.
fn mono_grey(f: &mut h264_core::Frame) {
    if f.chroma != h264_core::ChromaFormat::Mono {
        return;
    }
    let n = (f.width as usize / 2) * (f.height as usize / 2);
    f.planes[1] = vec![128; n];
    f.planes[2] = vec![128; n];
    if let Some(p) = f.planes16.as_mut() {
        let mid = 1u16 << (f.bit_depth.clamp(1, 16) - 1);
        p[1] = vec![mid; n];
        p[2] = vec![mid; n];
    }
    f.bit_depth_c = f.bit_depth;
    f.chroma = h264_core::ChromaFormat::Yuv420;
}

/// The one h264_core / hevc_core `Frame` -> [`Frame`] bridge. COLOURONE (SR328): above 8 bits (HEVC Main10) the samples
/// as decoded (`planes16`) become `Pixels::I420Hi`, so the conversion narrows after it converts; 8-bit is `I420` as before.
/// COLOURONE422 (SR359): every format at its own sampling (`ChromaFormat::sub`: chroma rows of w / SubWidthC) and each
/// plane's depth (`bit_depth`, `bit_depth_c`); 8-bit 4:2:2 / 4:4:4 are `I420Hi` at depth 8, 8-bit 4:2:0 stays `I420`.
pub(crate) fn to_frame(mut f: h264_core::Frame) -> Frame {
    let (w, h) = (f.width, f.height);
    mono_grey(&mut f); // HEVCREXTTOOLS (SR345): 4:0:0 -> grey 4:2:0
    let (sw, sh) = f.chroma.sub();
    let ss = ((sw >> 1) as u32, (sh >> 1) as u32);
    let (y_stride, uv_stride) = (w as usize, w as usize / sw);
    let (bit_depth, bit_depth_c) = (f.bit_depth, f.bit_depth_c);
    let pixels = match (f.planes16, ss) {
        (Some([y, u, v]), _) => Pixels::I420Hi { y, u, v, y_stride, uv_stride, bit_depth, ss, bit_depth_c },
        (None, (1, 1)) => {
            let [y, u, v] = f.planes;
            Pixels::I420 { y, u, v, y_stride, uv_stride }
        }
        (None, _) => {
            let [y, u, v] = f.planes.map(|p| p.into_iter().map(u16::from).collect::<Vec<u16>>());
            Pixels::I420Hi { y, u, v, y_stride, uv_stride, bit_depth: 8, ss, bit_depth_c: 8 }
        }
    };
    Frame { width: w, height: h, pts_ns: f.tag, pixels }
}

impl H264Decoder {
    pub fn new(track: &Track) -> Result<H264Decoder, DecodeError> {
        let mut dec = h264_core::Decoder::new();
        let length_prefixed = !track.config.is_empty();
        if length_prefixed {
            dec.configure_avcc(&track.config).map_err(map_err)?;
        }
        Ok(H264Decoder { dec, timebase: track.timebase, length_prefixed, ready: VecDeque::new() })
    }

    fn collect(&mut self) {
        while let Some(f) = self.dec.next_frame() {
            self.ready.push_back(to_frame(f));
        }
    }
}

impl VideoDecoder for H264Decoder {
    fn name(&self) -> &'static str {
        "h264 (h264_core)"
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
