// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The audio-track decoder seam of the player (PLAYBACK M3).
//!
//! A container's audio track arrives as packets; the player needs interleaved `f32` PCM with a
//! presentation time to feed resonance and drive the audio master clock. [`AudioTrackDecoder`]
//! is that contract, [`audio_decoder_for`] the registry. Built in today: [`PcmDecoder`]
//! (uncompressed integer 8/16/24/32-bit, either byte order, and IEEE float 32/64 — the whole
//! codec is the byte layout, so it is exact). Compressed audio (Opus, Vorbis, FLAC, AAC) is
//! AUDIOCODEC's (`dsp::audio`, `unaos/libs/media/audio_core`, LEDGER SR30): its decoders are
//! file/stream-oriented today; when it exposes a per-packet entry point the registry gains an arm
//! here. Until then such a track is reported `Unsupported` and the player runs on the wall
//! clock, silent — said in its status, never hidden.

use super::demux::{Codec, Packet, Track};
use super::video::DecodeError;

/// A decoded block: interleaved samples in [-1, 1].
#[derive(Debug, Clone, PartialEq)]
pub struct AudioBlock {
    pub pts_ns: i64,
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl AudioBlock {
    pub fn frames(&self) -> usize {
        self.samples.len() / self.channels.max(1) as usize
    }
}

pub trait AudioTrackDecoder: Send {
    fn name(&self) -> &'static str;
    fn decode(&mut self, pkt: &Packet) -> Result<AudioBlock, DecodeError>;
    fn reset(&mut self) {}
}

pub fn audio_decoder_for(track: &Track) -> Result<Box<dyn AudioTrackDecoder>, DecodeError> {
    match track.codec {
        Codec::Pcm { bits, float, big_endian } => Ok(Box::new(PcmDecoder::new(track, bits, float, big_endian)?)),
        ref other => Err(DecodeError::Unsupported(format!("{other:?} audio ({})", track.codec_name))),
    }
}

pub struct PcmDecoder {
    bits: u16,
    float: bool,
    big_endian: bool,
    rate: u32,
    channels: u16,
    timebase: super::demux::Timebase,
}

impl PcmDecoder {
    pub fn new(track: &Track, bits: u16, float: bool, big_endian: bool) -> Result<Self, DecodeError> {
        let ok = if float { matches!(bits, 32 | 64) } else { matches!(bits, 8 | 16 | 24 | 32) };
        if !ok || track.channels == 0 || track.sample_rate == 0 {
            return Err(DecodeError::Unsupported(format!("PCM {bits}-bit float={float} ch={} rate={}", track.channels, track.sample_rate)));
        }
        Ok(PcmDecoder { bits, float, big_endian, rate: track.sample_rate, channels: track.channels, timebase: track.timebase })
    }
}

impl AudioTrackDecoder for PcmDecoder {
    fn name(&self) -> &'static str {
        "pcm"
    }
    fn decode(&mut self, pkt: &Packet) -> Result<AudioBlock, DecodeError> {
        let bps = (self.bits / 8) as usize;
        let frame = bps * self.channels as usize;
        if pkt.data.len() % frame != 0 {
            return Err(DecodeError::Corrupt(format!("PCM packet of {} bytes is not whole {frame}-byte frames", pkt.data.len())));
        }
        let samples = pkt
            .data
            .chunks_exact(bps)
            .map(|c| {
                let mut b = [0u8; 8];
                // Normalise to little-endian in b[..bps].
                for i in 0..bps {
                    b[i] = if self.big_endian { c[bps - 1 - i] } else { c[i] };
                }
                match (self.float, self.bits) {
                    (true, 32) => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                    (true, _) => f64::from_le_bytes(b) as f32,
                    // 8-bit: unsigned in Matroska/WAV convention, signed in QuickTime `twos`;
                    // we follow the byte order flag: big-endian (twos) = signed.
                    (false, 8) => {
                        if self.big_endian { (b[0] as i8) as f32 / 128.0 } else { (b[0] as f32 - 128.0) / 128.0 }
                    }
                    (false, 16) => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
                    (false, 24) => (((b[0] as i32) | (b[1] as i32) << 8 | (b[2] as i32) << 16) << 8 >> 8) as f32 / 8_388_608.0,
                    (false, _) => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
                }
            })
            .collect();
        Ok(AudioBlock { pts_ns: self.timebase.to_ns(pkt.pts), sample_rate: self.rate, channels: self.channels, samples })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::demux::build::{self, MkvOptions, Mp4Options};
    use crate::dsp::demux::Demuxer;

    #[test]
    fn pcm16_round_trips_exactly_through_both_containers() {
        let pcm: Vec<i16> = (0..9600).map(|i| (((i * 7919) % 65536) as i32 - 32768) as i16).collect();
        let t = build::pcm16_track(1, 48_000, 2, &pcm, 1024);
        for file in [build::mp4(&[t.clone()], &Mp4Options::default()), build::mkv(&[t.clone()], &MkvOptions::default())] {
            let mut d = Demuxer::open(file).unwrap();
            let tr = d.tracks()[0].clone();
            let mut dec = audio_decoder_for(&tr).unwrap();
            let mut got = Vec::new();
            let mut frames = 0i64;
            while let Some(p) = d.next_packet() {
                let b = dec.decode(&p).unwrap();
                // MP4 carries the 48 kHz timescale (exact); Matroska our writer stamps at its
                // 1 ms TimestampScale, so the block time is the exact one truncated to the ms.
                let exact = (frames * 1_000_000_000 + 24_000) / 48_000;
                let want = if d.format() == crate::dsp::demux::Format::Mp4 { exact } else { exact / 1_000_000 * 1_000_000 };
                assert_eq!(b.pts_ns, want);
                frames += b.frames() as i64;
                got.extend(b.samples);
            }
            assert_eq!(got.len(), pcm.len());
            assert!(got.iter().zip(&pcm).all(|(g, &s)| (*g * 32768.0) as i32 == s as i32));
        }
    }

    #[test]
    fn pcm_layouts_known_answers() {
        let mk = |bits, float, be| {
            let mut t = build::pcm16_track(1, 8000, 1, &[0], 1).spec;
            t.bit_depth = bits;
            let file = build::mkv(&[build::MediaTrack { spec: t, samples: vec![] }], &MkvOptions::default());
            let tr = Demuxer::open(file).unwrap().tracks()[0].clone();
            PcmDecoder::new(&tr, bits, float, be).unwrap()
        };
        let pkt = |d: Vec<u8>| Packet { track: 1, pts: 0, dts: 0, duration: 1, keyframe: true, data: d };
        assert_eq!(mk(24, false, false).decode(&pkt(vec![0x00, 0x00, 0x80])).unwrap().samples, vec![-1.0]);
        assert_eq!(mk(24, false, true).decode(&pkt(vec![0x40, 0x00, 0x00])).unwrap().samples, vec![0.5]);
        assert_eq!(mk(8, false, false).decode(&pkt(vec![0x80, 0x00])).unwrap().samples, vec![0.0, -1.0]);
        assert_eq!(mk(32, true, false).decode(&pkt(0.25f32.to_le_bytes().to_vec())).unwrap().samples, vec![0.25]);
        assert!(mk(16, false, false).decode(&pkt(vec![1, 2, 3])).is_err());
    }
}
