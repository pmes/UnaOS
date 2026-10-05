// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The video decoder seam (PLAYBACK M3).
//!
//! [`VideoDecoder`] is the contract every codec implements: packets in (decode order), frames out
//! (presentation order, each stamped with its pts in ns). [`decoder_for`] picks one for a track.
//! Today the registry holds [`TestPattern`], which decodes UnaOS's `utp1` stream exactly and
//! stands in — honestly labelled — for any codec whose decoder has not landed, so the whole
//! player (demux → decode → clock → sink) runs before AV1 does. AV1 arrives as `video::av1` from
//! AVCODEC (LEDGER SR24) behind the `av1` feature: that module provides
//! `av1::Av1Decoder::new(&Track) -> Result<Av1Decoder, DecodeError>` implementing
//! [`VideoDecoder`], and the registry arm below is already written against it.

use super::demux::{Codec, Packet, Track};
use super::demux::build::TestPatternPacket;

/// Decoded pixels: packed 8-bit RGBA, or planar 8-bit 4:2:0 YCbCr (what an AV1 decoder emits).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Pixels {
    Rgba(Vec<u8>),
    I420 { y: Vec<u8>, u: Vec<u8>, v: Vec<u8>, y_stride: usize, uv_stride: usize },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub pts_ns: i64,
    pub pixels: Pixels,
}

impl Frame {
    /// Packed RGBA, row-major, top row first. I420 converts with BT.601 limited-range
    /// coefficients in fixed point (Y 16..235, CbCr 16..240; chroma sited at the 2×2 block).
    pub fn to_rgba(&self) -> Vec<u8> {
        match &self.pixels {
            Pixels::Rgba(p) => p.clone(),
            Pixels::I420 { y, u, v, y_stride, uv_stride } => {
                let (w, h) = (self.width as usize, self.height as usize);
                let mut out = vec![0u8; w * h * 4];
                for row in 0..h {
                    for col in 0..w {
                        let yy = y[row * y_stride + col] as i32 - 16;
                        let cb = u[(row / 2) * uv_stride + col / 2] as i32 - 128;
                        let cr = v[(row / 2) * uv_stride + col / 2] as i32 - 128;
                        let c = 298 * yy + 128;
                        let r = (c + 409 * cr) >> 8;
                        let g = (c - 100 * cb - 208 * cr) >> 8;
                        let b = (c + 516 * cb) >> 8;
                        let o = (row * w + col) * 4;
                        out[o] = r.clamp(0, 255) as u8;
                        out[o + 1] = g.clamp(0, 255) as u8;
                        out[o + 2] = b.clamp(0, 255) as u8;
                        out[o + 3] = 255;
                    }
                }
                out
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// No decoder for this codec is built in.
    Unsupported(String),
    /// The bitstream is malformed.
    Corrupt(String),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DecodeError::Unsupported(s) => write!(f, "no decoder: {s}"),
            DecodeError::Corrupt(s) => write!(f, "corrupt bitstream: {s}"),
        }
    }
}
impl std::error::Error for DecodeError {}

/// A video codec. Packets arrive in decode order with their track's timestamps; frames leave
/// in presentation order with `pts_ns` set. A decoder with reordering delay returns `Ok(None)`
/// until it has a frame and drains the rest on [`VideoDecoder::flush`].
pub trait VideoDecoder: Send {
    fn name(&self) -> &'static str;
    fn decode(&mut self, pkt: &Packet) -> Result<Option<Frame>, DecodeError>;
    /// End of stream (or before a seek): return every frame still held, in presentation order.
    fn flush(&mut self) -> Vec<Frame> {
        Vec::new()
    }
    /// Forget all state (after a seek; the next packet is a keyframe).
    fn reset(&mut self) {}
    /// True when frames are the codec's real output; false for a stand-in (the test pattern
    /// standing in for a codec with no decoder yet).
    fn is_real(&self) -> bool {
        true
    }
}

/// Pick the decoder for a video track.
///
/// `utp1` → [`TestPattern`] (real: it is that stream's decoder). AV1 → `av1::Av1Decoder` when the
/// `av1` feature is on. Anything else → `Err(Unsupported)`; the caller may then ask for
/// [`TestPattern::stand_in`] to keep the pipeline (clock, scheduling, sink) running, which marks
/// itself `is_real() == false`.
pub fn decoder_for(track: &Track) -> Result<Box<dyn VideoDecoder>, DecodeError> {
    match &track.codec {
        Codec::TestPattern => Ok(Box::new(TestPattern::new(track))),
        #[cfg(feature = "av1")]
        Codec::Av1 => Ok(Box::new(av1::Av1Decoder::new(track)?)),
        other => Err(DecodeError::Unsupported(format!("{other:?} ({})", track.codec_name))),
    }
}

#[cfg(feature = "av1")]
pub mod av1;

// ---------------------------------------------------------------------------------------------
// TestPattern
// ---------------------------------------------------------------------------------------------

/// Deterministic colour bars plus a frame counter.
///
/// For a `utp1` packet the counter is the frame ordinal the packet carries. As a stand-in for a
/// codec with no decoder, the counter is `round(pts / frame_duration)` (frame duration from the
/// track: span ÷ sample count), so frame N of a real file still shows N and the scheduling can be
/// checked against Chromium's frame times. Layout (any size ≥ 64×48): seven 75 % bars (white,
/// yellow, cyan, green, magenta, red, blue) over the top 2/3; the bottom 1/3 black with the
/// counter as white 5×7 digits in a [`COUNTER_DIGITS`]-wide field, scale chosen to fit.
pub struct TestPattern {
    width: u32,
    height: u32,
    timebase: super::demux::Timebase,
    frame_ns: i64,
    stand_in: bool,
}

pub const COUNTER_DIGITS: usize = 6;

const BARS: [[u8; 3]; 7] = [
    [191, 191, 191],
    [191, 191, 0],
    [0, 191, 191],
    [0, 191, 0],
    [191, 0, 191],
    [191, 0, 0],
    [0, 0, 191],
];

/// 5×7 digit glyphs, one row per byte, bit 4 = leftmost column.
const GLYPHS: [[u8; 7]; 10] = [
    [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
    [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
    [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
    [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
    [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
    [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
    [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
    [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
    [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
    [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
];

/// Where the counter field sits: (x0, y0, scale) — glyph cells are 6×8 scaled units (5×7 plus a
/// one-unit gap).
fn counter_geometry(w: u32, h: u32) -> (u32, u32, u32) {
    let band_top = h * 2 / 3;
    let band_h = h - band_top;
    let scale = ((w * 9 / 10) / (COUNTER_DIGITS as u32 * 6)).min(band_h * 8 / 10 / 8).max(1);
    let field_w = COUNTER_DIGITS as u32 * 6 * scale;
    let x0 = (w.saturating_sub(field_w)) / 2;
    let y0 = band_top + (band_h.saturating_sub(8 * scale)) / 2;
    (x0, y0, scale)
}

impl TestPattern {
    pub fn new(track: &Track) -> Self {
        let frame_ns = if track.sample_count > 0 && track.duration_ns > 0 {
            (track.duration_ns / track.sample_count) as i64
        } else {
            33_333_333
        };
        TestPattern {
            width: track.width.max(64),
            height: track.height.max(48),
            timebase: track.timebase,
            frame_ns: frame_ns.max(1),
            stand_in: false,
        }
    }
    /// A test pattern standing in for a codec without a decoder (see the type docs).
    pub fn stand_in(track: &Track) -> Self {
        TestPattern { stand_in: true, ..Self::new(track) }
    }

    /// Render frame `n` at `w×h` as packed RGBA.
    pub fn render(w: u32, h: u32, n: u32) -> Vec<u8> {
        let (wu, hu) = (w as usize, h as usize);
        let mut px = vec![0u8; wu * hu * 4];
        let band_top = hu * 2 / 3;
        for y in 0..hu {
            for x in 0..wu {
                let c = if y < band_top { BARS[x * 7 / wu] } else { [0, 0, 0] };
                let o = (y * wu + x) * 4;
                px[o..o + 3].copy_from_slice(&c);
                px[o + 3] = 255;
            }
        }
        let (x0, y0, s) = counter_geometry(w, h);
        let digits = format!("{:0width$}", n % 10u32.pow(COUNTER_DIGITS as u32), width = COUNTER_DIGITS);
        for (i, ch) in digits.bytes().enumerate() {
            let g = &GLYPHS[(ch - b'0') as usize];
            for (gy, row) in g.iter().enumerate() {
                for gx in 0..5 {
                    if row & (0x10 >> gx) == 0 {
                        continue;
                    }
                    for dy in 0..s {
                        for dx in 0..s {
                            let x = (x0 + (i as u32 * 6 + gx) * s + dx) as usize;
                            let y = (y0 + gy as u32 * s + dy) as usize;
                            if x < wu && y < hu {
                                let o = (y * wu + x) * 4;
                                px[o..o + 3].copy_from_slice(&[255, 255, 255]);
                            }
                        }
                    }
                }
            }
        }
        px
    }

    /// Read the counter back out of a rendered frame (the play-check oracle). Samples the centre
    /// of every glyph cell unit and matches each digit's 5×7 bitmap exactly; `None` if any digit
    /// does not match a glyph.
    pub fn read_counter(rgba: &[u8], w: u32, h: u32) -> Option<u32> {
        let (x0, y0, s) = counter_geometry(w, h);
        let mut n = 0u32;
        for i in 0..COUNTER_DIGITS as u32 {
            let mut bits = [0u8; 7];
            for (gy, b) in bits.iter_mut().enumerate() {
                for gx in 0..5u32 {
                    let x = x0 + (i * 6 + gx) * s + s / 2;
                    let y = y0 + gy as u32 * s + s / 2;
                    let o = ((y * w + x) * 4) as usize;
                    let p = rgba.get(o..o + 3)?;
                    if p.iter().all(|&c| c > 127) {
                        *b |= 0x10 >> gx;
                    }
                }
            }
            let d = GLYPHS.iter().position(|g| *g == bits)? as u32;
            n = n * 10 + d;
        }
        Some(n)
    }
}

impl VideoDecoder for TestPattern {
    fn name(&self) -> &'static str {
        if self.stand_in { "test-pattern (stand-in)" } else { "test-pattern" }
    }
    fn decode(&mut self, pkt: &Packet) -> Result<Option<Frame>, DecodeError> {
        let pts_ns = self.timebase.to_ns(pkt.pts);
        let (n, w, h) = match TestPatternPacket::decode(&pkt.data) {
            Some(p) if !self.stand_in => (p.frame, p.width as u32, p.height as u32),
            None if !self.stand_in => return Err(DecodeError::Corrupt("not a UTP1 packet".into())),
            _ => (((pts_ns + self.frame_ns / 2) / self.frame_ns).max(0) as u32, self.width, self.height),
        };
        let (w, h) = (w.max(64), h.max(48));
        Ok(Some(Frame { width: w, height: h, pts_ns, pixels: Pixels::Rgba(Self::render(w, h, n)) }))
    }
    fn is_real(&self) -> bool {
        !self.stand_in
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::demux::build::{self, Mp4Options};
    use crate::dsp::demux::Demuxer;

    #[test]
    fn counter_round_trips_at_many_sizes() {
        for &(w, h) in &[(64, 48), (320, 240), (640, 480), (1920, 1080), (97, 61)] {
            for n in [0u32, 1, 7, 42, 999, 123_456] {
                let px = TestPattern::render(w, h, n);
                assert_eq!(TestPattern::read_counter(&px, w, h), Some(n), "{w}x{h} n={n}");
            }
        }
    }

    #[test]
    fn bars_are_where_the_layout_says() {
        let px = TestPattern::render(320, 240, 5);
        let at = |x: usize, y: usize| px[(y * 320 + x) * 4..(y * 320 + x) * 4 + 3].to_vec();
        assert_eq!(at(10, 10), vec![191, 191, 191]);
        assert_eq!(at(70, 100), vec![191, 191, 0]);
        assert_eq!(at(310, 150), vec![0, 0, 191]);
        assert_eq!(at(2, 238), vec![0, 0, 0]);
    }

    #[test]
    fn utp1_stream_decodes_to_its_ordinals() {
        let file = build::mp4(&[build::test_pattern_track(1, 160, 120, 25, 30, 10)], &Mp4Options::default());
        let mut d = Demuxer::open(file).unwrap();
        let t = d.tracks()[0].clone();
        let mut dec = decoder_for(&t).unwrap();
        assert!(dec.is_real());
        let mut i = 0;
        while let Some(p) = d.next_packet() {
            let f = dec.decode(&p).unwrap().unwrap();
            assert_eq!(f.pts_ns, i as i64 * 40_000_000);
            assert_eq!(TestPattern::read_counter(&f.to_rgba(), f.width, f.height), Some(i));
            i += 1;
        }
        assert_eq!(i, 30);
    }

    #[test]
    fn unsupported_codec_is_named_and_stand_in_is_labelled() {
        let mut t = build::test_pattern_track(1, 320, 240, 10, 10, 10).spec;
        t.fourcc = *b"vp09";
        let file = build::mp4(&[build::MediaTrack { spec: t, samples: vec![] }], &Mp4Options::default());
        let d = Demuxer::open(file).unwrap();
        assert!(matches!(decoder_for(&d.tracks()[0]), Err(DecodeError::Unsupported(_))));
        let s = TestPattern::stand_in(&d.tracks()[0]);
        assert!(!s.is_real());
    }

    #[test]
    fn i420_to_rgba_bt601_limited() {
        // Y=235 Cb=Cr=128 → white; Y=16 → black; Y=81 Cb=90 Cr=240 → BT.601 red (≈255,0,0).
        let f = |y: u8, u: u8, v: u8| {
            Frame { width: 2, height: 2, pts_ns: 0, pixels: Pixels::I420 { y: vec![y; 4], u: vec![u], v: vec![v], y_stride: 2, uv_stride: 1 } }.to_rgba()[..3].to_vec()
        };
        assert_eq!(f(235, 128, 128), vec![255, 255, 255]);
        assert_eq!(f(16, 128, 128), vec![0, 0, 0]);
        let red = f(81, 90, 240);
        assert!(red[0] >= 253 && red[1] <= 2 && red[2] <= 2, "{red:?}");
    }
}
