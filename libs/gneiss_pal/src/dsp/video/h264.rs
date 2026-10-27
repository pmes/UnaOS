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

/// SWAPBACK-SETIDS-AVC (LEDGER SR397): the (nal_unit_type, id, unit) of every parameter-set NAL in `blob` — one
/// [`super::param_sets`] blob, framed as `track` frames its samples (the avcC's NAL length size, else Annex B start codes) —
/// split by h264_core's ONE splitter and each id read by h264_core's own parsers (`parse_sps` / `parse_pps` over the
/// unescaped payload, the decoder's own dispatch: a PPS reads with its SPS's chroma_format_idc when that SPS is earlier in
/// the blob, else 1). The H.264 twin of `hevc::param_set_ids`, what Stria's `decode_swap::SetIndex::key` keys a set state by.
/// `None` when the track is not H.264, or any unit is not an SPS / PPS whose header and id parse (the caller then keys by
/// the blob itself).
pub fn param_set_ids(track: &Track, blob: &[u8]) -> Option<Vec<(u8, u32, Vec<u8>)>> {
    if track.codec != crate::dsp::demux::Codec::Avc {
        return None;
    }
    let units = if track.config.is_empty() {
        h264_core::nal::annexb_units(blob)
    } else {
        let n = h264_core::nal::parse_avcc(&track.config).ok()?.nal_length_size;
        h264_core::nal::length_prefixed_units(blob, n).ok()?
    };
    let mut chroma: Vec<(u32, u32)> = Vec::new();
    let mut out = Vec::with_capacity(units.len());
    let mut rbsp = Vec::new();
    for u in units {
        let h = h264_core::nal::parse_header(*u.first()?).ok()?;
        h264_core::nal::unescape(u.get(1..)?, &mut rbsp);
        let id = match h.nal_type {
            7 => {
                let s = h264_core::ps::parse_sps(&rbsp).ok()?;
                chroma.push((s.id, s.chroma_format_idc));
                s.id
            }
            8 => h264_core::ps::parse_pps(&rbsp, |sid| chroma.iter().rev().find(|e| e.0 == sid).map_or(1, |e| e.1)).ok()?.id,
            _ => return None,
        };
        out.push((h.nal_type, id, u.to_vec()));
    }
    Some(out)
}

#[cfg(test)]
mod setids_tests {
    use super::param_set_ids;
    use crate::dsp::demux::Demuxer;
    use crate::dsp::demux::build::{self, Mp4Options};

    /// An SPS (Main, 64x48; Stria's tests/nle.rs AVCC_REORDER2 unit) and a PPS re-coded to pic_parameter_set_id 1.
    const SPS: [u8; 12] = [0x67, 0x4D, 0x00, 0x1E, 0xED, 0x88, 0xF4, 0x03, 0xC2, 0x21, 0x16, 0x48];
    const PPS1: [u8; 4] = [0x68, 0x53, 0x8E, 0x20];

    /// Rule: an avcC track's 4-byte-length blob SPS + PPS reads (7, 0) (8, 1) with each unit; the same units as Annex B on a
    /// config-less track read the same; the blob on an HEVC track reads `None`, and a blob holding a slice unit reads `None`.
    #[test]
    fn swapbacksetidsavc_reads_sps_pps_ids() {
        let mut spec = build::TrackSpec::test_pattern(1, 64, 48, 25);
        spec.fourcc = *b"avc1";
        spec.config_box = Some(*b"avcC");
        spec.codec_id = "V_MPEG4/ISO/AVC";
        spec.config = vec![1, 0x4D, 0x00, 0x1E, 0xFF, 0xE0, 0];
        spec.timescale = 1000;
        let samples = vec![build::SampleSpec { data: vec![0, 0, 0, 2, 0x65, 0x88], dts: 0, pts: 0, duration: 40, keyframe: true }];
        let d = Demuxer::open(build::mp4(&[build::MediaTrack { spec, samples }], &Mp4Options::default())).unwrap();
        let t = d.tracks()[0].clone();
        let blob: Vec<u8> = [&SPS[..], &PPS1[..]].iter().flat_map(|u| [&(u.len() as u32).to_be_bytes()[..], *u].concat()).collect();
        let ids = param_set_ids(&t, &blob).expect("H.264 set ids");
        assert_eq!(ids.iter().map(|e| (e.0, e.1)).collect::<Vec<_>>(), [(7, 0), (8, 1)]);
        assert_eq!((ids[0].2.clone(), ids[1].2.clone()), (SPS.to_vec(), PPS1.to_vec()));
        let mut annexb = t.clone();
        annexb.config = Vec::new();
        let ab: Vec<u8> = [&SPS[..], &PPS1[..]].iter().flat_map(|u| [&[0u8, 0, 0, 1][..], *u].concat()).collect();
        assert_eq!(param_set_ids(&annexb, &ab), Some(ids));
        let mut hevc = t.clone();
        hevc.codec = crate::dsp::demux::Codec::Hevc;
        assert_eq!(param_set_ids(&hevc, &blob), None);
        let mut slice = blob.clone();
        slice.extend_from_slice(&[0, 0, 0, 2, 0x65, 0x88]);
        assert_eq!(param_set_ids(&t, &slice), None);
    }
}
