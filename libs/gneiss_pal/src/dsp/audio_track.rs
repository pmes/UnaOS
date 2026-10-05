// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The audio-track decoder seam of the player (PLAYBACK M3; AUDIOTRACK, LEDGER SR45).
//!
//! A container's audio track arrives as packets; the player needs interleaved `f32` PCM with a
//! presentation time to feed resonance and drive the audio master clock. [`AudioTrackDecoder`]
//! is that contract, [`audio_decoder_for`] the registry:
//!
//! | `demux::Codec` | decoder (per packet) | config record |
//! |---|---|---|
//! | `Pcm` | [`PcmDecoder`] (the byte layout; exact) | — |
//! | `Opus` | `audio_core::opus::OpusDecoder` (RFC 6716 fixed-point reference, bit-exact) | Matroska CodecPrivate = `OpusHead` (LE); MP4 `dOps` (BE) |
//! | `Vorbis` | `audio_core::vorbis::{Setup, VorbisDecoder}` | Xiph-laced identification / comment / setup headers |
//! | `Aac` | `audio_core::aac::{Asc, AacDecoder}` (AAC-LC) | AudioSpecificConfig (synthesised from rate/channels when absent) |
//! | `Mp3` | `audio_core::mp3::{Header, Mp3Decoder}` (one frame per packet; the bit reservoir spans packets) | — |
//! | `Flac` | `audio_core::flac::FrameDecoder` (one frame per packet) | MP4 `dfLa` / Matroska `fLaC` metadata → STREAMINFO |
//!
//! **Gapless.** Every compressed decoder is wrapped in the same presentation-time trimmer
//! ([`Timing`]): output samples are time-stamped by counting from the stream's first packet
//! (after a seek: the first packet that produced any), so a decoder's priming and a block's
//! length never drift the clock, shifted by the codec
//! delay the container states (Matroska `CodecDelay`, else the Opus header's pre-skip; MP4 Opus
//! `dOps` pre-skip or `iTunSMPB` priming when no edit list already shifted the timeline), then
//! every sample before the presentation start (`floor`) and after the presented length (MP4 edit
//! segment duration / `iTunSMPB` total) is dropped, as is each Matroska block's
//! `DiscardPadding` tail. MP3 files' LAME/Xing gapless and Ogg granule trimming are the file
//! path's (`dsp::audio::Decoder`), which Stria uses for bare audio files.
//!
//! Channels come out interleaved in WAVE order (Vorbis's own 3–8-channel order is remapped, as
//! the file path does); the player maps them to stereo.

use super::demux::{Codec, Demuxer, Format, Packet, Track};
use super::video::DecodeError;
use audio_core as ac;

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
    /// The rate and channel count of the PCM this decoder hands out (the codec's own, which
    /// can differ from what the container's sample entry claims).
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> u16;
    fn decode(&mut self, pkt: &Packet) -> Result<AudioBlock, DecodeError>;
    fn reset(&mut self) {}
}

/// Where a track's presented audio lies on the media timeline (all ns).
///
/// A decoded sample at decoder time `t` (counted from the first packet that produced output)
/// is presented at `t − shift_ns`; it is kept when `floor_ns ≤ t − shift_ns < end_ns`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    pub shift_ns: i64,
    pub floor_ns: i64,
    pub end_ns: Option<i64>,
}

impl Default for Timing {
    fn default() -> Self {
        Timing { shift_ns: 0, floor_ns: i64::MIN, end_ns: None }
    }
}

/// Opus pre-skip from a Matroska `OpusHead` (LE) or an MP4 `dOps` body (BE), in 48 kHz samples.
fn opus_pre_skip(cfg: &[u8]) -> Option<u64> {
    if cfg.len() >= 19 && &cfg[..8] == b"OpusHead" {
        return Some(u16::from_le_bytes([cfg[10], cfg[11]]) as u64);
    }
    (cfg.len() >= 4).then(|| u16::from_be_bytes([cfg[2], cfg[3]]) as u64)
}

impl Timing {
    /// The gapless facts of `track` in `demux` (see the module docs).
    pub fn of(demux: &Demuxer, track: &Track) -> Timing {
        let idx = demux.track_index(track.id).unwrap_or(0);
        let first = demux.track_samples(idx).map(|s| track.to_ns(s.pts)).min().unwrap_or(0);
        match demux.format() {
            Format::Mp4 => {
                if first < 0 {
                    // an edit list moved the media start: presentation begins at 0
                    let end = track.play_ns.map(|p| p as i64);
                    Timing { shift_ns: 0, floor_ns: 0, end_ns: end }
                } else {
                    let delay = if track.codec == Codec::Opus { track.codec_delay_ns } else { 0 };
                    let shift = (delay + track.trim_start_ns) as i64;
                    Timing { shift_ns: shift, floor_ns: first, end_ns: track.play_ns.map(|p| first + p as i64) }
                }
            }
            _ => {
                let mut shift = track.codec_delay_ns as i64;
                if shift == 0 && track.codec == Codec::Opus {
                    shift = opus_pre_skip(&track.config).map(|n| (n * 1_000_000_000 / 48_000) as i64).unwrap_or(0);
                }
                Timing { shift_ns: shift, floor_ns: first.max(0), end_ns: None }
            }
        }
    }
}

/// The registry. `timing` trims compressed tracks (PCM is presented as stored).
pub fn audio_decoder_for(track: &Track, timing: Timing) -> Result<Box<dyn AudioTrackDecoder>, DecodeError> {
    let codec: Box<dyn PacketCodec> = match track.codec {
        Codec::Pcm { bits, float, big_endian } => return Ok(Box::new(PcmDecoder::new(track, bits, float, big_endian)?)),
        Codec::Opus => Box::new(OpusCodec::new(track)?),
        Codec::Vorbis => Box::new(VorbisCodec::new(track)?),
        Codec::Aac => Box::new(AacCodec::new(track)?),
        Codec::Mp3 => Box::new(Mp3Codec::new(track)),
        Codec::Flac => Box::new(FlacCodec::new(track)?),
        ref other => return Err(DecodeError::Unsupported(format!("{other:?} audio ({})", track.codec_name))),
    };
    Ok(Box::new(Gapless { codec, timing, timebase: track.timebase, anchor: None, produced: 0, fresh: true, buf: Vec::new() }))
}

fn corrupt(e: ac::Error) -> DecodeError {
    DecodeError::Corrupt(e.to_string())
}
fn unsupported(e: ac::Error) -> DecodeError {
    DecodeError::Unsupported(e.to_string())
}

/// One codec, packet in → interleaved frames out (no timing).
trait PacketCodec: Send {
    fn name(&self) -> &'static str;
    fn rate(&self) -> u32;
    fn channels(&self) -> u16;
    /// Decode one packet, appending interleaved samples to `out`; returns frames appended.
    fn decode(&mut self, data: &[u8], out: &mut Vec<f32>) -> Result<usize, DecodeError>;
    fn reset(&mut self);
}

/// The presentation-time trimmer every compressed decoder sits behind.
struct Gapless {
    codec: Box<dyn PacketCodec>,
    timing: Timing,
    timebase: super::demux::Timebase,
    /// Decoder time (ns, before the shift) of the first output sample since open/reset.
    anchor: Option<i64>,
    /// Frames produced since the anchor.
    produced: u64,
    /// No packet decoded yet since open: the stream's first packet anchors the timeline even
    /// when it yields nothing (a Vorbis stream's first packet only primes the overlap, and its
    /// timestamp is the first sample's whichever muxing convention stamped the later ones).
    /// After a seek the anchor is the first packet that yields samples.
    fresh: bool,
    buf: Vec<f32>,
}

impl AudioTrackDecoder for Gapless {
    fn name(&self) -> &'static str {
        self.codec.name()
    }
    fn sample_rate(&self) -> u32 {
        self.codec.rate()
    }
    fn channels(&self) -> u16 {
        self.codec.channels()
    }
    fn reset(&mut self) {
        self.codec.reset();
        self.anchor = None;
        self.produced = 0;
        self.fresh = false;
    }
    fn decode(&mut self, pkt: &Packet) -> Result<AudioBlock, DecodeError> {
        let rate = self.codec.rate().max(1) as i64;
        let ch = self.codec.channels().max(1) as usize;
        self.buf.clear();
        let n = self.codec.decode(&pkt.data, &mut self.buf)?;
        let at = |frames: u64| (frames as i128 * 1_000_000_000 / rate as i128) as i64;
        if (n > 0 || self.fresh) && self.anchor.is_none() {
            self.anchor = Some(self.timebase.to_ns(pkt.pts));
        }
        self.fresh = false;
        let t0 = self.anchor.unwrap_or(self.timebase.to_ns(pkt.pts)) + at(self.produced) - self.timing.shift_ns;
        self.produced += n as u64;
        // frames whose presentation time lies before floor / at or after end are dropped
        let frames_until = |t: i64| -> usize {
            if t <= t0 {
                return 0;
            }
            let k = ((t - t0) as i128 * rate as i128 + 500_000_000) / 1_000_000_000;
            (k.max(0) as usize).min(n)
        };
        let lo = if self.timing.floor_ns == i64::MIN { 0 } else { frames_until(self.timing.floor_ns) };
        let mut hi = match self.timing.end_ns {
            Some(e) => frames_until(e),
            None => n,
        };
        if pkt.discard_ns > 0 {
            let d = ((pkt.discard_ns as i128 * rate as i128 + 500_000_000) / 1_000_000_000) as usize;
            hi = hi.min(n.saturating_sub(d));
        }
        let hi = hi.max(lo);
        Ok(AudioBlock {
            pts_ns: t0 + at(lo as u64),
            sample_rate: rate as u32,
            channels: ch as u16,
            samples: self.buf[lo * ch..hi * ch].to_vec(),
        })
    }
}

// ------------------------------------------------------------------------------------- Opus

struct OpusCodec {
    dec: ac::opus::OpusDecoder,
    pcm: Vec<i16>,
}

impl OpusCodec {
    fn new(track: &Track) -> Result<Self, DecodeError> {
        let c = &track.config;
        // (channels, output gain Q7.8 dB, mapping family)
        let (ch, gain, family) = if c.len() >= 19 && &c[..8] == b"OpusHead" {
            (c[9] as usize, i16::from_le_bytes([c[16], c[17]]), c[18])
        } else if c.len() >= 11 {
            (c[1] as usize, i16::from_be_bytes([c[8], c[9]]), c[10])
        } else {
            (track.channels as usize, 0, 0)
        };
        if family != 0 || !(1..=2).contains(&ch) {
            return Err(DecodeError::Unsupported(format!("Opus channel mapping family {family} with {ch} channels (multistream owed)")));
        }
        let mut dec = ac::opus::OpusDecoder::new(ch);
        dec.decode_gain = gain as i32;
        Ok(OpusCodec { dec, pcm: vec![0; 5760 * ch] })
    }
}

impl PacketCodec for OpusCodec {
    fn name(&self) -> &'static str {
        "opus"
    }
    fn rate(&self) -> u32 {
        48_000
    }
    fn channels(&self) -> u16 {
        self.dec.channels as u16
    }
    fn decode(&mut self, data: &[u8], out: &mut Vec<f32>) -> Result<usize, DecodeError> {
        if data.is_empty() {
            // a zero-length packet is a lost (or DTX) frame: conceal the previous packet's
            // duration (RFC 6716 §4.4 / `opus_demo`)
            let d = (self.dec.last_packet_duration.max(120) as usize).min(5760);
            let n = self.dec.decode(None, &mut self.pcm, d).map_err(corrupt)?;
            let ch = self.dec.channels;
            out.extend(self.pcm[..n * ch].iter().map(|&s| s as f32 / 32768.0));
            return Ok(n);
        }
        let n = match self.dec.decode(Some(data), &mut self.pcm, 5760) {
            Ok(n) => n,
            // a corrupt packet: conceal one 20 ms frame rather than stop (the Ogg path's rule)
            Err(_) => self.dec.decode(None, &mut self.pcm, 960).map_err(corrupt)?,
        };
        let ch = self.dec.channels;
        out.extend(self.pcm[..n * ch].iter().map(|&s| s as f32 / 32768.0));
        Ok(n)
    }
    fn reset(&mut self) {
        self.dec.reset();
    }
}

// ----------------------------------------------------------------------------------- Vorbis

/// Split a Xiph-laced header triple (Matroska CodecPrivate for Vorbis).
pub fn xiph_headers(cfg: &[u8]) -> Option<[&[u8]; 3]> {
    let (&count, mut rest) = cfg.split_first()?;
    if count != 2 {
        return None;
    }
    let mut sizes = [0usize; 2];
    for s in sizes.iter_mut() {
        loop {
            let (&b, r) = rest.split_first()?;
            rest = r;
            *s += b as usize;
            if b != 255 {
                break;
            }
        }
    }
    if sizes[0] + sizes[1] > rest.len() {
        return None;
    }
    let (a, r) = rest.split_at(sizes[0]);
    let (b, c) = r.split_at(sizes[1]);
    Some([a, b, c])
}

struct VorbisCodec {
    dec: ac::vorbis::VorbisDecoder,
    planes: Vec<Vec<f32>>,
}

impl VorbisCodec {
    fn new(track: &Track) -> Result<Self, DecodeError> {
        let [ident, _comment, setup] = xiph_headers(&track.config).ok_or_else(|| DecodeError::Corrupt("Vorbis CodecPrivate is not three Xiph-laced headers".into()))?;
        let s = ac::vorbis::Setup::parse(ident, setup).map_err(unsupported)?;
        Ok(VorbisCodec { dec: ac::vorbis::VorbisDecoder::new(s), planes: Vec::new() })
    }
}

impl PacketCodec for VorbisCodec {
    fn name(&self) -> &'static str {
        "vorbis"
    }
    fn rate(&self) -> u32 {
        self.dec.setup.rate
    }
    fn channels(&self) -> u16 {
        self.dec.setup.channels as u16
    }
    fn decode(&mut self, data: &[u8], out: &mut Vec<f32>) -> Result<usize, DecodeError> {
        if data.first().is_some_and(|b| b & 1 == 1) {
            return Ok(0); // a header packet inside the stream
        }
        // a corrupt audio packet decodes to nothing (the Ogg path's rule)
        let n = self.dec.decode(data, &mut self.planes).unwrap_or(0);
        let ch = self.planes.len();
        // Vorbis I §4.3.9 orders 3–8 channels L,C,R,…; hand out WAVE order (as the file path).
        const ORDER: [&[usize]; 6] = [&[0, 2, 1], &[0, 1, 2, 3], &[0, 2, 1, 3, 4], &[0, 2, 1, 5, 3, 4], &[0, 2, 1, 6, 5, 3, 4], &[0, 2, 1, 7, 5, 6, 3, 4]];
        let map: Vec<usize> = if (3..=8).contains(&ch) { ORDER[ch - 3].to_vec() } else { (0..ch).collect() };
        out.reserve(n * ch);
        for i in 0..n {
            for &c in &map {
                out.push(self.planes[c][i]);
            }
        }
        Ok(n)
    }
    fn reset(&mut self) {
        self.dec.reset();
    }
}

// -------------------------------------------------------------------------------------- AAC

struct AacCodec {
    dec: ac::aac::AacDecoder,
    rate: u32,
}

impl AacCodec {
    fn new(track: &Track) -> Result<Self, DecodeError> {
        let asc = if track.config.len() >= 2 {
            track.config.clone()
        } else {
            // no AudioSpecificConfig (legacy Matroska `A_AAC/MPEG4/LC`, MPEG-2 MP4): LC at the
            // track's rate and channel count
            let sfi = ac::aac::RATES.iter().position(|&r| r == track.sample_rate).unwrap_or(4) as u16;
            ((2u16 << 11) | (sfi << 7) | ((track.channels & 15) << 3)).to_be_bytes().to_vec()
        };
        let a = ac::aac::Asc::parse(&asc).map_err(unsupported)?;
        let layout = a.layout().map_err(unsupported)?;
        let rate = ac::aac::RATES[a.sf_index];
        Ok(AacCodec { dec: ac::aac::AacDecoder::new(a.sf_index, layout).map_err(unsupported)?, rate })
    }
}

impl PacketCodec for AacCodec {
    fn name(&self) -> &'static str {
        "aac-lc"
    }
    fn rate(&self) -> u32 {
        self.rate
    }
    fn channels(&self) -> u16 {
        self.dec.layout.channels as u16
    }
    fn decode(&mut self, data: &[u8], out: &mut Vec<f32>) -> Result<usize, DecodeError> {
        let mut r = ac::bits::BitReader::new(data);
        if self.dec.decode_block(&mut r, 0).is_err() {
            // a damaged access unit plays as silence of its length (the file path's rule)
            for o in self.dec.out.iter_mut() {
                o.iter_mut().for_each(|x| *x = 0.0);
            }
        }
        let ch = self.dec.out.len();
        out.reserve(1024 * ch);
        for i in 0..1024 {
            for c in 0..ch {
                out.push(self.dec.out[c][i]);
            }
        }
        Ok(1024)
    }
    fn reset(&mut self) {
        // the overlap state of the filterbank: a fresh decoder (cheap next to the stream)
        if let Ok(d) = ac::aac::AacDecoder::new(self.dec.sf_index, self.dec.layout.clone()) {
            self.dec = d;
        }
    }
}

// -------------------------------------------------------------------------------------- MP3

struct Mp3Codec {
    dec: ac::mp3::Mp3Decoder,
    rate: u32,
    channels: usize,
    planes: Vec<Vec<f32>>,
}

impl Mp3Codec {
    fn new(track: &Track) -> Self {
        Mp3Codec { dec: ac::mp3::Mp3Decoder::new(), rate: track.sample_rate, channels: track.channels.clamp(1, 2) as usize, planes: vec![Vec::new(), Vec::new()] }
    }
}

impl PacketCodec for Mp3Codec {
    fn name(&self) -> &'static str {
        "mp3"
    }
    fn rate(&self) -> u32 {
        self.rate
    }
    fn channels(&self) -> u16 {
        self.channels as u16
    }
    fn decode(&mut self, data: &[u8], out: &mut Vec<f32>) -> Result<usize, DecodeError> {
        if data.len() < 4 {
            return Err(DecodeError::Corrupt("MP3 packet shorter than a header".into()));
        }
        let h = ac::mp3::Header::parse(u32::from_be_bytes([data[0], data[1], data[2], data[3]])).ok_or_else(|| DecodeError::Corrupt("MP3 packet without a frame header".into()))?;
        if self.rate == 0 {
            self.rate = h.rate();
        }
        let n = self.dec.decode_frame(&h, data, &mut self.planes).map_err(corrupt)?;
        let hc = h.channels();
        out.reserve(n * self.channels);
        for i in 0..n {
            match (hc, self.channels) {
                (2, 1) => out.push(0.5 * (self.planes[0][i] + self.planes[1][i])),
                _ => {
                    for c in 0..self.channels {
                        out.push(self.planes[c.min(hc - 1)][i]);
                    }
                }
            }
        }
        Ok(n)
    }
    fn reset(&mut self) {
        self.dec.reset();
    }
}

// ------------------------------------------------------------------------------------- FLAC

/// STREAMINFO from an MP4 `dfLa` body (FullBox + metadata blocks) or a Matroska CodecPrivate
/// (`fLaC` + metadata blocks).
fn flac_stream_info(cfg: &[u8]) -> Option<ac::flac::StreamInfo> {
    let blocks = if cfg.starts_with(b"fLaC") { &cfg[4..] } else { cfg.get(4..)? };
    let mut p = 0usize;
    while p + 4 <= blocks.len() {
        let kind = blocks[p] & 0x7F;
        let len = ((blocks[p + 1] as usize) << 16) | ((blocks[p + 2] as usize) << 8) | blocks[p + 3] as usize;
        if kind == 0 {
            return ac::flac::StreamInfo::parse(blocks.get(p + 4..p + 4 + len)?).ok();
        }
        if blocks[p] & 0x80 != 0 {
            break;
        }
        p += 4 + len;
    }
    None
}

struct FlacCodec {
    dec: ac::flac::FrameDecoder,
    pcm: ac::Pcm,
}

impl FlacCodec {
    fn new(track: &Track) -> Result<Self, DecodeError> {
        let si = flac_stream_info(&track.config).ok_or_else(|| DecodeError::Corrupt("FLAC config without STREAMINFO".into()))?;
        Ok(FlacCodec { dec: ac::flac::FrameDecoder::new(si), pcm: ac::Pcm::default() })
    }
}

impl PacketCodec for FlacCodec {
    fn name(&self) -> &'static str {
        "flac"
    }
    fn rate(&self) -> u32 {
        self.dec.si.rate
    }
    fn channels(&self) -> u16 {
        self.dec.si.channels as u16
    }
    fn decode(&mut self, data: &[u8], out: &mut Vec<f32>) -> Result<usize, DecodeError> {
        self.dec.decode(data, &mut self.pcm).map_err(corrupt)?;
        let n = self.pcm.frames;
        let ch = self.pcm.int.len();
        let scale = 1.0 / (1u64 << (self.pcm.bits - 1)) as f32;
        out.reserve(n * ch);
        for i in 0..n {
            for c in 0..ch {
                out.push(self.pcm.int[c][i] as f32 * scale);
            }
        }
        Ok(n)
    }
    fn reset(&mut self) {}
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
    fn sample_rate(&self) -> u32 {
        self.rate
    }
    fn channels(&self) -> u16 {
        self.channels
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
            let mut dec = audio_decoder_for(&tr, Timing::default()).unwrap();
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
        let pkt = |d: Vec<u8>| Packet { track: 1, pts: 0, dts: 0, duration: 1, keyframe: true, data: d, discard_ns: 0 };
        assert_eq!(mk(24, false, false).decode(&pkt(vec![0x00, 0x00, 0x80])).unwrap().samples, vec![-1.0]);
        assert_eq!(mk(24, false, true).decode(&pkt(vec![0x40, 0x00, 0x00])).unwrap().samples, vec![0.5]);
        assert_eq!(mk(8, false, false).decode(&pkt(vec![0x80, 0x00])).unwrap().samples, vec![0.0, -1.0]);
        assert_eq!(mk(32, true, false).decode(&pkt(0.25f32.to_le_bytes().to_vec())).unwrap().samples, vec![0.25]);
        assert!(mk(16, false, false).decode(&pkt(vec![1, 2, 3])).is_err());
    }
}
