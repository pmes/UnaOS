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
//! conversion is [`super::h264::to_frame`]. Streams the decoder refuses (separate colour planes and the rest of hevc_core's
//! `check_supported`) are `DecodeError::Unsupported` with the decoder's own reason; COLOURONE422 (SR359): 4:2:2 / 4:4:4 and
//! unequal luma / chroma depths reach the Player (hevc_core's 4:2:0-only opt-in is no longer set).

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
        let mut dec = hevc_core::Decoder::new(); // COLOURONE422 (SR359): to_frame carries every format and depth; no 4:2:0-only opt-in
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

/// HEVCLEVELLINE (LEDGER SR368): the profile / tier / level of an hvcC's SPS, read by hevc_core's own decoder
/// (`configure_hvcc`, `Decoder::profile_tier_level`) — what `dsp::video::profile_level` formats. `None` when the record
/// carries no SPS that parses.
pub fn hvcc_ptl(hvcc: &[u8]) -> Option<hevc_core::ps::Ptl> {
    let mut d = hevc_core::Decoder::new();
    d.configure_hvcc(hvcc).ok()?;
    d.profile_tier_level()
}

/// COLOURONE422 (SR359): hevc_core's hand-assembled-stream kit, by path as tools/play-check takes it.
#[cfg(test)]
#[path = "../../../../../unaos/libs/media/hevc_core/tests/common/mod.rs"]
#[allow(dead_code, unused_imports)]
mod hevc_kit;

#[cfg(test)]
mod tests {
    use super::hevc_kit::{idr_slice_header, nal, pps, sps, vps, CabacEnc, PpsCfg, SpsCfg};
    use crate::dsp::demux::colour::Colour;
    use crate::dsp::video::{Chroma, Pixels};

    /// One 32x32 I_PCM picture (one CU, deblocking off) at `idc`, PcmBitDepth = BitDepth, through hevc_core and `to_frame`.
    fn pcm32(idc: u32, (bdy, bdc): (u32, u32), y: &[u16], cb: &[u16], cr: &[u16]) -> crate::dsp::video::Frame {
        let cfg = SpsCfg { width: 32, height: 32, log2_ctb: 5, chroma_format_idc: idc, bit_depth: bdy, chroma_bit_depth: Some(bdc), pcm: Some((bdy, bdc, 3, 5, true)), ..Default::default() };
        let mut s = vps(false);
        s.extend(sps(&cfg));
        s.extend(pps(&PpsCfg { init_qp: 26, deblocking_disabled: true, ..Default::default() }));
        let mut e = CabacEnc::new(idr_slice_header(false, false, false, true), 0, 26);
        let sub = [(2, 2), (2, 2), (2, 1), (1, 1)][idc as usize];
        e.pcm_cu_fmt(0, 5, bdy, bdc, y, cb, cr, sub);
        s.extend(nal(19, &e.finish().out));
        let mut d = hevc_core::Decoder::new();
        d.decode_annexb(&s).expect("decode");
        d.flush();
        super::to_frame(d.take_frames().remove(0))
    }

    fn bt601_limited() -> Colour {
        Colour { primaries: 2, transfer: 2, matrix: 6, full_range: Some(false), mastering: None, light: None }
    }

    fn at(rgba: &[u8], x: usize, y: usize) -> [u8; 3] {
        let o = (y * 32 + x) * 4;
        [rgba[o], rgba[o + 1], rgba[o + 2]]
    }

    /// 4:2:2 and 4:4:4 (was: refused by the 4:2:0-only opt-in): Y 120, Cb 90 in the left half / 160 in the right, Cr 160 in
    /// the top half / 90 in the bottom, BT.601 limited at 8 bits (298 / 409 / 100 / 208 / 516 >> 8): TL (172, 110, 44),
    /// TR (172, 83, 186), BL (60, 167, 44), BR (60, 139, 186) on each side of the centre — a wrong subsampling reads across.
    #[test]
    fn colourone422_hevc_422_and_444_pcm_convert_by_their_sampling() {
        for (idc, (sw, sh), ss) in [(2u32, (2usize, 1usize), (1u32, 0u32)), (3, (1, 1), (0, 0))] {
            let (cw, ch) = (32 / sw, 32 / sh);
            let cb: Vec<u16> = (0..cw * ch).map(|i| if (i % cw) * sw < 16 { 90 } else { 160 }).collect();
            let cr: Vec<u16> = (0..cw * ch).map(|i| if (i / cw) * sh < 16 { 160 } else { 90 }).collect();
            let f = pcm32(idc, (8, 8), &[120; 1024], &cb, &cr);
            assert!(matches!(&f.pixels, Pixels::I420Hi { ss: s, bit_depth: 8, bit_depth_c: 8, uv_stride, .. } if *s == ss && *uv_stride == cw), "idc {idc}");
            let rgba = f.to_rgba_colour(Some(&bt601_limited()), Chroma::Nearest);
            for (x, y, want) in [(15, 15, [172, 110, 44]), (16, 15, [172, 83, 186]), (15, 16, [60, 167, 44]), (16, 16, [60, 139, 186])] {
                assert_eq!(at(&rgba, x, y), want, "idc {idc} ({x}, {y})");
            }
            assert_eq!((at(&rgba, 0, 0), at(&rgba, 31, 31)), ([172, 110, 44], [60, 139, 186]), "idc {idc}");
        }
    }

    /// Luma 10 bits beside chroma 8 (was: refused, "luma and chroma bit depths differ"): Y 480 Cb 90 Cr 160 converts with
    /// each plane's depth to (172, 110, 44) — the 8-bit Y 120 Cb 90 Cr 160 colour; read at one depth it would be (0, 234, 0).
    #[test]
    fn colourone422_hevc_unequal_depths_convert_by_each_plane() {
        let f = pcm32(1, (10, 8), &[480; 1024], &[90; 256], &[160; 256]);
        assert!(matches!(&f.pixels, Pixels::I420Hi { ss: (1, 1), bit_depth: 10, bit_depth_c: 8, .. }));
        let rgba = f.to_rgba_colour(Some(&bt601_limited()), Chroma::Nearest);
        assert!(rgba.chunks_exact(4).all(|q| q == [172, 110, 44, 255]));
        // 4:2:0 at one depth is unchanged: 8-bit stays `I420`, Main 10 `I420Hi` 10 / 10.
        assert!(matches!(pcm32(1, (8, 8), &[120; 1024], &[90; 256], &[160; 256]).pixels, Pixels::I420 { .. }));
        assert!(matches!(pcm32(1, (10, 10), &[480; 1024], &[360; 256], &[640; 256]).pixels, Pixels::I420Hi { ss: (1, 1), bit_depth: 10, bit_depth_c: 10, .. }));
    }
}

/// SWAPBACK-SETIDS (LEDGER SR388): the (nal_unit_type, id, unit) of every parameter-set NAL in `blob` — one
/// [`super::param_sets`] blob, framed as `track` frames its samples (the hvcC's NAL length size, else Annex B start codes) —
/// split by h264_core's ONE splitter and each id read by hevc_core's own parsers (`parse_vps` / `parse_sps` / `parse_pps`
/// over the unescaped payload, the decoder's own dispatch). What Stria's `decode_swap::SetIndex::key` keys a set state by,
/// so equal states in different blobs are one key. `None` when the track is not HEVC, or any unit is not a VPS / SPS / PPS
/// whose header and id parse (the caller then keys by the blob itself).
pub fn param_set_ids(track: &Track, blob: &[u8]) -> Option<Vec<(u8, u32, Vec<u8>)>> {
    if track.codec != crate::dsp::demux::Codec::Hevc {
        return None;
    }
    let units = if track.config.is_empty() {
        hevc_core::nal::annexb_units(blob)
    } else {
        let n = hevc_core::nal::parse_hvcc(&track.config).ok()?.nal_length_size;
        hevc_core::nal::length_prefixed_units(blob, n).ok()?
    };
    let mut out = Vec::with_capacity(units.len());
    let mut rbsp = Vec::new();
    for u in units {
        let h = hevc_core::nal::parse_header(u).ok()?;
        hevc_core::nal::unescape(u.get(2..)?, &mut rbsp);
        let id = match h.nal_type {
            32 => hevc_core::ps::parse_vps(&rbsp).ok()?.id as u32,
            33 => hevc_core::ps::parse_sps(&rbsp).ok()?.id as u32,
            34 => hevc_core::ps::parse_pps(&rbsp).ok()?.id as u32,
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

    /// The HEVC KAT units (Main, 64x48, 8-bit: Stria's `tests/media.rs` HEVC_VPS / HEVC_SPS / HEVC_PPS).
    const VPS: [u8; 23] = [0x40, 0x01, 0x0c, 0x01, 0xff, 0xff, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x3c, 0xac, 0x09];
    const SPS: [u8; 27] = [0x42, 0x01, 0x01, 0x01, 0x60, 0x00, 0x00, 0x03, 0x00, 0x90, 0x00, 0x00, 0x03, 0x00, 0x00, 0x03, 0x00, 0x3c, 0xa0, 0x20, 0x83, 0x16, 0x5f, 0x92, 0x4c, 0x20, 0x80];
    const PPS: [u8; 6] = [0x44, 0x01, 0xc0, 0x71, 0x81, 0x12];

    /// Rule: an hvcC track's 4-byte-length blob VPS + SPS + PPS reads (32, 0) (33, 0) (34, 0) with each unit; the same blob
    /// on an H.264 track reads `None` (no HEVC ids there), and a blob holding a non-set unit reads `None`.
    #[test]
    fn swapbacksetids_reads_vps_sps_pps_ids() {
        let mut hvcc = vec![1, 0x01, 0x60, 0, 0, 0, 0x90, 0, 0, 0, 0, 0, 60, 0xf0, 0x00, 0xfc, 0xfd, 0xf8, 0xf8, 0, 0, 0x0f, 3];
        for (ty, unit) in [(32u8, &VPS[..]), (33, &SPS[..]), (34, &PPS[..])] {
            hvcc.extend_from_slice(&[0x80 | ty, 0, 1]);
            hvcc.extend_from_slice(&(unit.len() as u16).to_be_bytes());
            hvcc.extend_from_slice(unit);
        }
        let mut spec = build::TrackSpec::test_pattern(1, 64, 48, 25);
        spec.fourcc = *b"hvc1";
        spec.config_box = Some(*b"hvcC");
        spec.codec_id = "V_MPEGH/ISO/HEVC";
        spec.config = hvcc;
        spec.timescale = 1000;
        let samples = vec![build::SampleSpec { data: vec![0, 0, 0, 1, 0x26], dts: 0, pts: 0, duration: 40, keyframe: true }];
        let d = Demuxer::open(build::mp4(&[build::MediaTrack { spec, samples }], &Mp4Options::default())).unwrap();
        let t = d.tracks()[0].clone();
        let blob: Vec<u8> = [&VPS[..], &SPS[..], &PPS[..]].iter().flat_map(|u| [&(u.len() as u32).to_be_bytes()[..], *u].concat()).collect();
        let ids = param_set_ids(&t, &blob).expect("HEVC set ids");
        assert_eq!(ids.iter().map(|e| (e.0, e.1)).collect::<Vec<_>>(), [(32, 0), (33, 0), (34, 0)]);
        assert_eq!(ids[1].2, SPS.to_vec());
        let mut avc = t.clone();
        avc.codec = crate::dsp::demux::Codec::Avc;
        assert_eq!(param_set_ids(&avc, &blob), None);
        let mut idr = blob.clone();
        idr.extend_from_slice(&[0, 0, 0, 3, 0x26, 0x01, 0xaf]);
        assert_eq!(param_set_ids(&t, &idr), None);
    }
}
