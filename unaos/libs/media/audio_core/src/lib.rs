// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `audio_core` — the audio decoders UnaOS owns (AUDIOCODEC, ledger SR30).
//!
//! Peter (2026-10-04): "we will need to support more than wav files!" — and "true cutting edge, not chicken
//! wire": every format here is written from its specification, `no_std` + `alloc`, no dependencies. The
//! kernel's `play` and the host (Gneiss `dsp::audio`, Stria, `tools/audio-check`) link the same code.
//!
//! | format | specification | module |
//! |---|---|---|
//! | WAV (PCM 8–32, float 32/64, A-law/µ-law, EXTENSIBLE) | Microsoft RIFF / WAVEFORMATEXTENSIBLE | [`wav`] |
//! | AIFF / AIFF-C (`NONE`, `sowt`, `fl32`, `fl64`) | Apple AIFF 1.3 / AIFF-C | [`aiff`] |
//! | FLAC (native and Ogg) | RFC 9639 | [`flac`] |
//! | Ogg container | RFC 3533 | [`ogg`] |
//! | Opus (SILK, CELT, hybrid; Ogg mapping) | RFC 6716 + RFC 8251, RFC 7845 | [`opus`] |
//! | Vorbis (floors 0/1, residues 0/1/2; Ogg mapping) | Xiph Vorbis I specification | [`vorbis`] |
//! | MP3 (MPEG-1/2/2.5 Layer III; ID3v2, Xing/LAME gapless) | ISO/IEC 11172-3, 13818-3 | [`mp3`] |
//! | AAC-LC (ADTS; raw in MP4) | ISO/IEC 14496-3, 13818-7 | [`aac`] |
//! | MP4/M4A container (AAC, MP3; fragmented; edit-list/iTunSMPB gapless) | ISO/IEC 14496-12, -14 | [`mp4`] |
//!
//! One API: [`sniff`] names the format from the first bytes, [`Decoder::open`] picks the codec, and the
//! [`AudioDecoder`] trait hands out interleaved PCM either as `f32` or as left-justified `i32`.
//!
//! **Sample conventions (the oracle rule).** An integer source of depth `b` maps to `f32` as `s / 2^(b-1)`
//! — exact for every `b ≤ 24`, which is also what Chromium's `decodeAudioData` produces, so the lossless
//! formats compare bit-for-bit. `next_i32` returns integers left-justified to 32 bits (`s << (32-b)`), so a
//! consumer that wants 16-bit takes `>> 16` with no knowledge of the source depth. A float source maps to
//! `i32` as `round(clamp(x, -1, 1 - 2^-31) * 2^31)`; no dither anywhere.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;
#[cfg(test)]
extern crate std;

use alloc::boxed::Box;
use alloc::vec::Vec;

pub mod aac;
pub mod aiff;
pub mod bits;
pub mod crc;
pub mod facts;
pub use facts::{facts_of, AudioFacts};
pub mod flac;
pub mod io;
pub mod math;
pub mod md5;
pub mod mp3;
pub mod mp4;
pub mod ogg;
pub mod opus;
pub mod vorbis;
pub mod wav;

pub use io::{ByteStream, Read, VecReader};

/// Why a decode stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The stream ended inside a structure (truncated file).
    Eof,
    /// The bytes violate the format.
    Invalid(&'static str),
    /// Valid, but a feature this decoder does not implement (named).
    Unsupported(&'static str),
    /// A checksum the format carries did not match.
    Checksum(&'static str),
    /// The byte source failed.
    Io,
}
pub type Result<T> = core::result::Result<T, Error>;

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Eof => f.write_str("truncated stream"),
            Error::Invalid(s) => write!(f, "invalid: {}", s),
            Error::Unsupported(s) => write!(f, "unsupported: {}", s),
            Error::Checksum(s) => write!(f, "checksum mismatch: {}", s),
            Error::Io => f.write_str("read error"),
        }
    }
}

/// What [`sniff`] recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Wav,
    Aiff,
    Flac,
    /// An Ogg stream (the codec inside is found from its first page: FLAC, Opus or Vorbis).
    Ogg,
    Mp3,
    /// AAC in ADTS framing.
    Adts,
    /// ISO-BMFF (MP4/M4A).
    Mp4,
    Unknown,
}

impl Format {
    pub fn name(self) -> &'static str {
        match self {
            Format::Wav => "wav",
            Format::Aiff => "aiff",
            Format::Flac => "flac",
            Format::Ogg => "ogg",
            Format::Mp3 => "mp3",
            Format::Adts => "aac-adts",
            Format::Mp4 => "mp4",
            Format::Unknown => "unknown",
        }
    }
}

/// The codec actually decoding (after the container has been looked into).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Pcm,
    Flac,
    Opus,
    Vorbis,
    Mp3,
    Aac,
}

/// Stream parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Info {
    pub rate: u32,
    pub channels: u16,
    /// Source sample depth: the integer width of a PCM/lossless source, 32/64 for float PCM, 16 for Opus
    /// (the fixed-point reference decoder's output), and 0 for the float lossy codecs.
    pub bits: u16,
    /// Total frames (samples per channel) when the container states it.
    pub frames: Option<u64>,
    pub format: Format,
    pub codec: Codec,
    /// The source samples are floating point (float PCM, and every lossy codec).
    pub float: bool,
}

/// A decoded block, planar. Either integer samples of depth `bits`, or float samples.
#[derive(Default)]
pub struct Pcm {
    pub bits: u32,
    pub float: bool,
    pub frames: usize,
    pub int: Vec<Vec<i32>>,
    pub flt: Vec<Vec<f32>>,
}

impl Pcm {
    pub(crate) fn set_int(&mut self, ch: usize, frames: usize, bits: u32) {
        self.float = false;
        self.bits = bits;
        self.frames = frames;
        self.int.resize_with(ch, Vec::new);
        self.int.truncate(ch);
        for c in self.int.iter_mut() { c.clear(); c.resize(frames, 0); }
    }
    pub(crate) fn set_float(&mut self, ch: usize, frames: usize) {
        self.float = true;
        self.frames = frames;
        self.flt.resize_with(ch, Vec::new);
        self.flt.truncate(ch);
        for c in self.flt.iter_mut() { c.clear(); c.resize(frames, 0.0); }
    }
    #[inline]
    fn get_f32(&self, c: usize, i: usize) -> f32 {
        if self.float { self.flt[c][i] } else { self.int[c][i] as f32 / (1u64 << (self.bits - 1)) as f32 }
    }
    #[inline]
    fn get_i32(&self, c: usize, i: usize) -> i32 {
        if self.float { f32_to_i32(self.flt[c][i]) } else { ((self.int[c][i] as i64) << (32 - self.bits)) as i32 }
    }
}

/// `round(clamp(x) * 2^31)`, the documented float→int rule.
#[inline]
pub fn f32_to_i32(x: f32) -> i32 {
    let v = math::round(x as f64 * 2147483648.0);
    if v >= 2147483647.0 { i32::MAX } else if v <= -2147483648.0 { i32::MIN } else { v as i32 }
}

/// What every codec implements internally: produce the next block. `Send`, so a decoder can move to an audio
/// thread (host) or sit in the kernel's player state.
pub trait Source: Send {
    fn info(&self) -> Info;
    /// Decode the next block into `pcm`; `Ok(false)` at end of stream. A block may hold zero frames.
    fn block(&mut self, pcm: &mut Pcm) -> Result<bool>;
    /// SEEKTABLE (rmbp B433): restart at a sync point at or before output sample `target`, found from the
    /// container's own index; the next `block` starts at the returned point's `sample`. `Ok(None)` = this format
    /// has no table here (the caller decodes from the top instead) — the stream is then unchanged.
    fn seek(&mut self, _target: u64) -> Result<Option<SeekPoint>> { Ok(None) }
}

/// SEEKTABLE (rmbp B433): where a seek restarted the decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeekPoint {
    /// The byte offset of the sync point the decode restarted at (the first pre-roll frame's, if any).
    pub byte: u64,
    /// The output sample the source's next block starts at (≤ the target).
    pub sample: u64,
    /// `sample` is known exactly (an index that counts samples); `false` = an estimate (Xing TOC, CBR),
    /// within one frame.
    pub exact: bool,
    /// The index that answered: `pcm`, `flac`, `xing`, `vbri`, `cbr`, `mp4`.
    pub table: &'static str,
    /// The output sample playback resumes at, after the decoder drops the residual (filled by [`Decoder::seek`]).
    pub landed: u64,
}

impl SeekPoint {
    pub fn landed_ms(&self, rate: u32) -> u64 { if rate == 0 { 0 } else { self.landed * 1000 / rate as u64 } }
}

/// The one decoder API (Gneiss `dsp::audio`, Stria, PLAYBACK, the kernel's `play`).
pub trait AudioDecoder {
    /// Open a byte source; the format is sniffed.
    fn open(src: Box<dyn Read>) -> Result<Self>
    where
        Self: Sized;
    fn info(&self) -> Info;
    /// Fill `out` with interleaved `f32` samples; returns FRAMES written (0 = end of stream).
    /// `out.len()` must be at least `channels`.
    fn next(&mut self, out: &mut [f32]) -> Result<usize>;
    /// Same, as left-justified `i32`.
    fn next_i32(&mut self, out: &mut [i32]) -> Result<usize>;
}

/// The concrete decoder: a [`Source`] for whatever [`sniff`] found, plus the block buffer.
pub struct Decoder {
    src: Box<dyn Source>,
    pcm: Pcm,
    pos: usize,
    done: bool,
    /// SEEKTABLE: output frames still to drop after a seek (the residual between the sync point and the target).
    drop: u64,
}

impl Decoder {
    /// Open an in-memory file.
    pub fn open_bytes(bytes: Vec<u8>) -> Result<Decoder> { <Decoder as AudioDecoder>::open(Box::new(VecReader::new(bytes))) }
    /// Wrap an already-built codec source.
    pub fn from_source(src: Box<dyn Source>) -> Decoder { Decoder { src, pcm: Pcm::default(), pos: 0, done: false, drop: 0 } }

    fn refill(&mut self) -> Result<bool> {
        while self.pos >= self.pcm.frames {
            if self.done { return Ok(false); }
            self.pos = 0;
            self.pcm.frames = 0;
            if !self.src.block(&mut self.pcm)? { self.done = true; return Ok(false); }
            if self.drop > 0 { let k = self.drop.min(self.pcm.frames as u64); self.pos = k as usize; self.drop -= k; }
        }
        Ok(true)
    }

    /// SEEKTABLE (rmbp B433): seek to `ms` from the container's own index. The decode restarts at a sync point
    /// at or before the target and the residual (under one frame) is decoded and dropped, so an `exact` point
    /// lands on the target sample. `Ok(None)`: no table for this format (nothing moved; decode from the top).
    pub fn seek(&mut self, ms: u64) -> Result<Option<SeekPoint>> {
        let info = self.src.info();
        let mut target = ms.saturating_mul(info.rate as u64) / 1000;
        if let Some(t) = info.frames { target = target.min(t); }
        let Some(mut p) = self.src.seek(target)? else { return Ok(None) };
        // SEEKTABLE2: a source that learns its length while seeking (an Ogg's last granule, ADTS's header walk) says so
        if let Some(t) = self.src.info().frames { target = target.min(t); }
        self.pcm.frames = 0;
        self.pos = 0;
        self.done = false;
        self.drop = target.saturating_sub(p.sample);
        p.landed = p.sample + self.drop;
        Ok(Some(p))
    }
}

impl AudioDecoder for Decoder {
    fn open(src: Box<dyn Read>) -> Result<Decoder> {
        let mut s = ByteStream::new(src);
        s.fill(64)?;
        // MP3HANG (rmbp B373): each arm is an OUTLINED call, so this frame is the largest arm's, not the sum of
        // every constructor inlined into it (92504 bytes on the kernel target — three times a 32 KiB kernel stack;
        // flight 23's `tests play mp3` wrote through the guard). The arms are the same calls as before.
        let source: Box<dyn Source> = match sniff(s.data()) {
            Format::Wav => open_arm::wav(s)?,
            Format::Aiff => open_arm::aiff(s)?,
            Format::Flac => open_arm::flac(s)?,
            Format::Ogg => open_arm::ogg(s)?,
            Format::Mp3 => open_arm::mp3_or_adts(s)?,
            Format::Adts => open_arm::adts(s)?,
            Format::Mp4 => open_arm::mp4(s)?,
            // Not recognisable from the first bytes: an MP3 can still start after junk (an ICY header, a
            // truncated tag) — the frame sync scan verifies every candidate against the next header.
            Format::Unknown => match open_arm::mp3(s) {
                Ok(m) => m,
                Err(_) => return Err(Error::Unsupported("unrecognised format")),
            },
        };
        Ok(Decoder::from_source(source))
    }
    fn info(&self) -> Info { self.src.info() }
    fn next(&mut self, out: &mut [f32]) -> Result<usize> {
        let ch = self.src.info().channels as usize;
        let mut n = 0;
        while (n + 1) * ch <= out.len() {
            if !self.refill()? { break; }
            let take = (self.pcm.frames - self.pos).min(out.len() / ch - n);
            for i in 0..take {
                for c in 0..ch { out[(n + i) * ch + c] = self.pcm.get_f32(c, self.pos + i); }
            }
            self.pos += take;
            n += take;
        }
        Ok(n)
    }
    fn next_i32(&mut self, out: &mut [i32]) -> Result<usize> {
        let ch = self.src.info().channels as usize;
        let mut n = 0;
        while (n + 1) * ch <= out.len() {
            if !self.refill()? { break; }
            let take = (self.pcm.frames - self.pos).min(out.len() / ch - n);
            for i in 0..take {
                for c in 0..ch { out[(n + i) * ch + c] = self.pcm.get_i32(c, self.pos + i); }
            }
            self.pos += take;
            n += take;
        }
        Ok(n)
    }
}

/// Name the format from the first bytes of a file (64 is plenty; fewer may return `Unknown`).
pub fn sniff(d: &[u8]) -> Format {
    let at = |o: usize, m: &[u8]| d.len() >= o + m.len() && &d[o..o + m.len()] == m;
    if (at(0, b"RIFF") || at(0, b"RF64") || at(0, b"BW64")) && at(8, b"WAVE") { return Format::Wav; }
    if at(0, b"FORM") && (at(8, b"AIFF") || at(8, b"AIFC")) { return Format::Aiff; }
    if at(0, b"fLaC") { return Format::Flac; }
    if at(0, b"OggS") { return Format::Ogg; }
    if at(4, b"ftyp") { return Format::Mp4; }
    if flac::probe_frame(d).is_some() { return Format::Flac; }
    if at(0, b"ID3") { return Format::Mp3; }
    if d.len() >= 2 && d[0] == 0xFF && d[1] & 0xF6 == 0xF0 { return Format::Adts; }
    if d.len() >= 4 && d[0] == 0xFF && d[1] & 0xE0 == 0xE0 && (d[1] >> 1) & 3 != 0 && d[2] >> 4 != 15 && (d[2] >> 2) & 3 != 3 {
        return Format::Mp3;
    }
    Format::Unknown
}

/// Decode a whole in-memory file to interleaved `f32`.
pub fn decode_all(bytes: &[u8]) -> Result<(Info, Vec<f32>)> {
    let mut d = Decoder::open_bytes(bytes.to_vec())?;
    let info = d.info();
    let ch = info.channels as usize;
    let mut out = Vec::new();
    let mut buf = alloc::vec![0f32; 4096 * ch];
    loop {
        let n = d.next(&mut buf)?;
        if n == 0 { break; }
        out.extend_from_slice(&buf[..n * ch]);
    }
    Ok((d.info(), out))
}

/// Decode a whole in-memory file to interleaved left-justified `i32`.
pub fn decode_all_i32(bytes: &[u8]) -> Result<(Info, Vec<i32>)> {
    let mut d = Decoder::open_bytes(bytes.to_vec())?;
    let info = d.info();
    let ch = info.channels as usize;
    let mut out = Vec::new();
    let mut buf = alloc::vec![0i32; 4096 * ch];
    loop {
        let n = d.next_i32(&mut buf)?;
        if n == 0 { break; }
        out.extend_from_slice(&buf[..n * ch]);
    }
    Ok((d.info(), out))
}

/// MP3HANG (rmbp B373): `Decoder::open`'s arms, one outlined function each, every codec state boxed at once —
/// the open frame is bounded by the deepest single arm (see the note in `open`).
mod open_arm {
    use super::*;
    #[inline(never)] pub fn wav(s: ByteStream) -> Result<Box<dyn Source>> { Ok(Box::new(wav::WavDecoder::new(s)?)) }
    #[inline(never)] pub fn aiff(s: ByteStream) -> Result<Box<dyn Source>> { Ok(Box::new(aiff::AiffDecoder::new(s)?)) }
    #[inline(never)] pub fn flac(s: ByteStream) -> Result<Box<dyn Source>> { Ok(Box::new(flac::FlacDecoder::new(s)?)) }
    #[inline(never)] pub fn ogg(s: ByteStream) -> Result<Box<dyn Source>> { ogg::open(s) }
    #[inline(never)] pub fn adts(s: ByteStream) -> Result<Box<dyn Source>> { Ok(Box::new(aac::AdtsStream::new(s)?)) }
    #[inline(never)] pub fn mp4(s: ByteStream) -> Result<Box<dyn Source>> { mp4::open(s) }
    #[inline(never)] pub fn mp3(s: ByteStream) -> Result<Box<dyn Source>> { Ok(Box::new(mp3::Mp3Stream::new(s)?)) }
    /// An ID3v2 tag can front ADTS as well as MP3: look past it.
    #[inline(never)]
    pub fn mp3_or_adts(mut s: ByteStream) -> Result<Box<dyn Source>> {
        let d = s.data();
        let mut is_adts = false;
        if d.len() >= 10 && &d[0..3] == b"ID3" {
            let size = ((d[6] as usize & 127) << 21) | ((d[7] as usize & 127) << 14) | ((d[8] as usize & 127) << 7) | (d[9] as usize & 127);
            let at = 10 + size + if d[5] & 0x10 != 0 { 10 } else { 0 };
            if s.fill(at + 2)? >= at + 2 { let d = s.data(); is_adts = d[at] == 0xFF && d[at + 1] & 0xF6 == 0xF0; }
        }
        if is_adts { adts(s) } else { mp3(s) }
    }
}

/// OPENERS (rmbp-ledger B379) — the MIME type of an audio file, from its first bytes, for the kernel's type table
/// (`fs/filetype.rs`) and any ring-3 caller: [`sniff`]'s answer, so the type and the decoder cannot disagree.
/// Returns `(mime, strong)`. `strong` is a magic the format defines (`RIFF…WAVE`, `FORM…AIFF`, `fLaC`, `OggS`,
/// `ID3`); a bare frame-sync guess (an MPEG audio frame header, an ADTS header, a FLAC frame header) is WEAK, and
/// the caller lets a file's name overrule it. ISO-BMFF answers `None`: whether an `ftyp` file is `audio/mp4` or
/// `video/mp4` is the container core's question (`demux_core::mime_of`). An Ogg stream is `audio/ogg` whatever
/// codec it carries (RFC 5334 §10.3: Opus, Vorbis and FLAC in Ogg are all `audio/ogg`). Pure.
pub fn mime_of(d: &[u8]) -> Option<(&'static str, bool)> {
    let strong_mp3 = d.len() >= 3 && &d[..3] == b"ID3";
    let strong_flac = d.len() >= 4 && &d[..4] == b"fLaC";
    Some(match sniff(d) {
        Format::Wav => ("audio/wav", true),
        Format::Aiff => ("audio/aiff", true),
        Format::Flac => ("audio/flac", strong_flac),
        Format::Ogg => ("audio/ogg", true),
        Format::Mp3 => ("audio/mpeg", strong_mp3),
        Format::Adts => ("audio/aac", false),
        Format::Mp4 | Format::Unknown => return None,
    })
}
