// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! MP4/M4A (ISO/IEC 14496-12, -14) through `demux_core`, the one ISOBMFF parser both rings link — MP4ONE
//! (rmbp-ledger B464, R79: a core depending on a core is the seam). `demux_core` builds the sample tables
//! (`stsc`/`stco`/`co64`/`stsz`/`stz2`, fragmented `moof/traf/trun`), reads the `esds`, the edit list and Apple's
//! `iTunSMPB`; this file picks the first AAC or MP3 audio track — a video track beside it is ignored — and hands
//! its access units to [`crate::aac::AacSource`] or [`crate::mp3::Mp3Stream`]. The gapless trim is computed here
//! in the codec's sample units from the file's raw numbers (`Track::edit`, `Track::smpb`), as the retired
//! `audio_core::mp4` reader did, so the decode is byte-identical to it (`tests/mp4one_kat.rs`).
//!
//! A file cut short plays to its cut (`Demuxer::open_partial`). The file is read whole (the `moov` may follow
//! the `mdat`).
//!
//! VPLAYAUDIO (rmbp-ledger B475): [`open_demuxed`] is the ONE door for a sound inside a demuxed file — the kernel's
//! video player hands it the picture job's `Demuxer::share` (one parse, no byte copy) — for MP4 (AAC, MP3) and
//! Matroska/WebM (Opus, Vorbis) alike. The Matroska sound is indexed from its own packets (Opus TOC sample counts,
//! Vorbis block sizes — no decode), so a seek is exact (`table=matroska`), and `DiscardPadding` trims the tail.
//! [`plays`] is the predicate a player's facts ask (`audio_ok`).
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec::Vec;

use demux_core::{Codec as DCodec, Demuxer, Format as DFormat, Sample, TrackKind};

use crate::io::Read;
use crate::{Codec, Format, Info, Pcm, SeekPoint};

use crate::io::ByteStream;
use crate::{Error, Result, Source};

fn err(e: demux_core::Error) -> Error {
    match e {
        demux_core::Error::Invalid(m) => Error::Invalid(m),
        demux_core::Error::Unsupported(m) => Error::Unsupported(m),
        demux_core::Error::Truncated => Error::Invalid("mp4: truncated container"),
        demux_core::Error::UnknownFormat => Error::Invalid("mp4: not an ISOBMFF file"),
    }
}

pub fn open(mut s: ByteStream) -> Result<Box<dyn Source>> {
    open_demuxed(Demuxer::open_partial(s.rest()?).map_err(err)?)
}

/// VPLAYAUDIO (rmbp B475): the sound [`open_demuxed`] plays for this container and codec.
pub fn plays(format: DFormat, codec: &DCodec) -> bool {
    match format {
        DFormat::Mp4 => matches!(codec, DCodec::Aac | DCodec::Mp3),
        DFormat::Matroska | DFormat::WebM => matches!(codec, DCodec::Opus | DCodec::Vorbis),
    }
}

/// The sound of an opened container as a [`Source`]: the first AAC or MP3 track of an MP4, the first Opus or Vorbis
/// track of a Matroska/WebM file. A player that already demuxed the file hands its `Demuxer` (or a
/// [`Demuxer::share`] of it) over; the file's bytes are shared, never copied.
pub fn open_demuxed(d: Demuxer) -> Result<Box<dyn Source>> {
    match d.format() {
        DFormat::Mp4 => open_mp4(d),
        DFormat::Matroska | DFormat::WebM => open_mkv(d),
    }
}

fn open_mp4(d: Demuxer) -> Result<Box<dyn Source>> {
    let audio = |c: &DCodec| matches!(c, DCodec::Aac | DCodec::Mp3);
    let idx = d
        .tracks()
        .iter()
        .position(|t| t.kind == TrackKind::Audio && audio(&t.codec))
        .ok_or_else(|| match d.audio_track() {
            Some(_) => Error::Unsupported("mp4: audio codec (owed: only AAC and MP3 in MP4)"),
            None => Error::Unsupported("mp4: no audio track"),
        })?;
    let t = d.tracks()[idx].clone();
    let units: Vec<(usize, usize)> = d.track_samples(idx).map(|s| (s.offset as usize, s.size as usize)).collect();
    if units.is_empty() {
        return Err(Error::Invalid("mp4: no samples"));
    }
    let data = d.bytes();
    drop(d);
    if t.codec == DCodec::Mp3 {
        // the access units, end to end, read in place (the MP3 frame walk sees one elementary stream)
        let bs = ByteStream::new(Box::new(UnitReader::new(data, units)));
        return Ok(Box::new(crate::mp3::Mp3Stream::new(bs)?));
    }
    let asc = if t.config.is_empty() {
        // MPEG-2 AAC without a DecoderSpecificInfo: synthesise LC at the sample entry's rate
        let sfi = crate::aac::RATES.iter().position(|&r| r == t.sample_rate).unwrap_or(4) as u16;
        let v: u16 = (2 << 11) | (sfi << 7) | ((t.channels & 15) << 3);
        v.to_be_bytes().to_vec()
    } else {
        t.config.clone()
    };
    let a = crate::aac::Asc::parse(&asc)?;
    let rate = crate::aac::RATES[a.sf_index] as u64;
    let ts = t.timebase.den.max(1);
    let (mut skip, mut total) = (0u64, None);
    if let Some(e) = t.edit.filter(|e| e.media_time >= 0) {
        let (dur, mt) = (e.segment, e.media_time as u64);
        if mt > 0 || dur > 0 {
            skip = (mt * rate + ts / 2) / ts;
            let mts = e.movie_timescale as u64;
            if dur > 0 && mts != 0 { total = Some((dur * rate + mts / 2) / mts); }
        }
    }
    if skip == 0 {
        if let Some((pri, tot)) = t.smpb {
            skip = pri;
            if tot > 0 { total = Some(tot); }
        }
    }
    Ok(crate::aac::AacSource::new(data, units, &asc, skip, total)?.boxed())
}

// ---------------------------------------------------------------- VPLAYAUDIO (rmbp B475): MP3 units in place

/// An MP4's MP3 access units as one byte stream, read from the shared file bytes (no concatenated copy). A seek is
/// in the stream's own offsets (the units end to end), as the MP3 frame walk asks.
struct UnitReader {
    data: Arc<Vec<u8>>,
    units: Vec<(usize, usize)>,
    /// `ends[i]`: the stream offset after unit `i`.
    ends: Vec<u64>,
    pos: u64,
}

impl UnitReader {
    fn new(data: Arc<Vec<u8>>, units: Vec<(usize, usize)>) -> UnitReader {
        let units: Vec<(usize, usize)> = units.into_iter().map(|(o, n)| (o, n.min(data.len().saturating_sub(o)))).collect();
        let mut e = 0u64;
        let ends = units.iter().map(|&(_, n)| { e += n as u64; e }).collect();
        UnitReader { data, units, ends, pos: 0 }
    }
}

impl Read for UnitReader {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        let i = self.ends.partition_point(|&e| e <= self.pos);
        let Some(&(o, n)) = self.units.get(i) else { return Ok(0) };
        let begin = self.ends[i] - n as u64;
        let at = (self.pos - begin) as usize;
        let k = (n - at).min(buf.len());
        buf[..k].copy_from_slice(&self.data[o + at..o + at + k]);
        self.pos += k as u64;
        Ok(k)
    }
    fn seek(&mut self, off: u64) -> Result<bool> {
        self.pos = off.min(self.ends.last().copied().unwrap_or(0));
        Ok(true)
    }
    fn len(&self) -> Option<u64> { Some(self.ends.last().copied().unwrap_or(0)) }
}

// ---------------------------------------------------------------- VPLAYAUDIO (rmbp B475): the Matroska sound

/// One Matroska audio track read in place: its samples, each packet's decoded-sample start (from the packets
/// themselves), the cursor, and each packet's `DiscardPadding` in samples.
struct MkvTrack {
    d: Demuxer,
    samples: Vec<Sample>,
    /// `starts[i]`: decoded samples before packet `i` (codec units, pre-skip included); `starts[len]` = the total.
    starts: Vec<u64>,
    next: usize,
    rate: u64,
    last_discard: u64,
}

impl MkvTrack {
    fn new(d: Demuxer, idx: usize, rate: u32, len_of: impl Fn(&[u8], Option<&[u8]>) -> u64) -> MkvTrack {
        let samples: Vec<Sample> = d.track_samples(idx).copied().collect();
        let mut starts = Vec::with_capacity(samples.len() + 1);
        let mut acc = 0u64;
        let mut prev: Option<&[u8]> = None;
        for s in &samples {
            starts.push(acc);
            let p = d.sample_data(s);
            acc += len_of(p, prev);
            prev = Some(p);
        }
        // the end padding (the last packet's DiscardPadding) is not part of the stream's length
        let pad = samples.last().map(|s| ns_to(s.discard_ns, rate as u64)).unwrap_or(0);
        starts.push(acc.saturating_sub(pad));
        MkvTrack { d, samples, starts, next: 0, rate: rate as u64, last_discard: 0 }
    }
    fn packet(&mut self) -> Option<&[u8]> {
        let s = self.samples.get(self.next)?;
        self.next += 1;
        self.last_discard = ns_to(s.discard_ns, self.rate);
        Some(self.d.sample_data(s))
    }
    /// The last packet `k ≥ min` whose start is ≤ `at`; the cursor moves there.
    fn seek_to(&mut self, at: u64, min: usize) -> Option<(u64, u64)> {
        let n = self.samples.len();
        if n <= min { return None; }
        let k = self.starts[..n].partition_point(|&s| s <= at).saturating_sub(1).max(min);
        self.next = k;
        self.last_discard = 0;
        Some((self.starts[k], self.samples[k].offset))
    }
    fn total(&self) -> u64 { *self.starts.last().unwrap_or(&0) }
}

fn ns_to(ns: u64, rate: u64) -> u64 { ((ns as u128 * rate as u128 + 500_000_000) / 1_000_000_000) as u64 }

impl crate::opus::Packets for MkvTrack {
    fn next_packet(&mut self) -> Option<Vec<u8>> { self.packet().map(|p| p.to_vec()) }
    fn discard(&self) -> u64 { self.last_discard }
    fn seek_packet(&mut self, at: u64) -> Option<(u64, u64)> { self.seek_to(at, 0) }
    fn total(&self) -> Option<u64> { Some(MkvTrack::total(self)) }
    fn table(&self) -> &'static str { "matroska" }
}

fn open_mkv(d: Demuxer) -> Result<Box<dyn Source>> {
    let idx = d
        .tracks()
        .iter()
        .position(|t| t.kind == TrackKind::Audio && matches!(t.codec, DCodec::Opus | DCodec::Vorbis))
        .ok_or_else(|| match d.audio_track() {
            Some(_) => Error::Unsupported("matroska: audio codec (owed: only Opus and Vorbis in Matroska/WebM)"),
            None => Error::Unsupported("matroska: no audio track"),
        })?;
    let t = d.tracks()[idx].clone();
    if t.sample_count == 0 { return Err(Error::Invalid("matroska: no audio packets")); }
    if t.codec == DCodec::Opus {
        let len = |p: &[u8], _: Option<&[u8]>| match crate::opus::decoder::parse_packet(p) {
            Ok((toc, _, sizes)) => sizes.len() as u64 * crate::opus::decoder::packet_samples_per_frame(toc, 48_000) as u64,
            Err(_) => 960, // a corrupt packet is concealed as one 20 ms frame (OpusPackets' rule)
        };
        let tr = MkvTrack::new(d, idx, 48_000, len);
        return Ok(Box::new(crate::opus::OpusPackets::new(&t.config, t.codec_delay_ns, Box::new(tr))?));
    }
    let h = xiph_split(&t.config).filter(|h| h.len() == 3).ok_or(Error::Invalid("matroska: Vorbis CodecPrivate (Xiph lacing)"))?;
    let setup = crate::vorbis::Setup::parse(h[0], h[2])?;
    let len = {
        let s = &setup;
        move |p: &[u8], prev: Option<&[u8]>| match (prev.and_then(|q| s.packet_blocksize(q)), s.packet_blocksize(p)) {
            (Some(a), Some(b)) => (a / 4 + b / 4) as u64,
            _ => 0, // the first packet primes the overlap (Vorbis I §4.3.8); a non-audio packet decodes to nothing
        }
    };
    let tr = MkvTrack::new(d, idx, setup.rate, len);
    Ok(Box::new(MkvVorbis { tr, dec: crate::vorbis::VorbisDecoder::new(setup), out: Vec::new() }))
}

/// The Matroska codec mapping's Xiph lacing (CodecPrivate of `A_VORBIS`): count-1, then the sizes of all but the
/// last in 255-runs, then the packets.
pub fn xiph_split(c: &[u8]) -> Option<Vec<&[u8]>> {
    let n = *c.first()? as usize + 1;
    let mut i = 1;
    let mut sizes = Vec::new();
    for _ in 0..n - 1 {
        let mut s = 0usize;
        loop {
            let b = *c.get(i)?;
            i += 1;
            s += b as usize;
            if b != 255 { break; }
        }
        sizes.push(s);
    }
    let mut out = Vec::new();
    for s in sizes {
        out.push(c.get(i..i + s)?);
        i += s;
    }
    out.push(c.get(i..)?);
    Some(out)
}

/// Vorbis inside Matroska/WebM: the track's packets into the one [`crate::vorbis::VorbisDecoder`].
struct MkvVorbis {
    tr: MkvTrack,
    dec: crate::vorbis::VorbisDecoder,
    out: Vec<Vec<f32>>,
}

impl Source for MkvVorbis {
    fn info(&self) -> Info {
        let s = &self.dec.setup;
        Info { rate: s.rate, channels: s.channels as u16, bits: 0, frames: Some(self.tr.total()), format: Format::Unknown, codec: Codec::Vorbis, float: true }
    }
    fn block(&mut self, pcm: &mut Pcm) -> Result<bool> {
        loop {
            let Some(p) = self.tr.packet() else { return Ok(false) };
            let n = self.dec.decode(p, &mut self.out).unwrap_or(0); // a corrupt packet decodes to nothing (OggVorbis's rule)
            let n = n.saturating_sub(self.tr.last_discard as usize); // DiscardPadding: the tail is padding
            if n == 0 { continue; }
            let planes: Vec<Vec<f32>> = self.out.iter().map(|c| c[..n.min(c.len())].to_vec()).collect();
            let planes = crate::vorbis::wave_order(planes);
            pcm.set_float(planes.len(), n);
            for (c, p) in planes.into_iter().enumerate() {
                pcm.flt[c] = p;
                pcm.flt[c].resize(n, 0.0);
            }
            return Ok(true);
        }
    }
    /// The packet index: packet `k`'s output starts at `starts[k]` (the block-size walk); a fresh decoder decodes
    /// packet `k − 1` as the overlap pre-roll (its output is none) — bit-exact with the full decode.
    fn seek(&mut self, t: u64) -> Result<Option<SeekPoint>> {
        let n = self.tr.samples.len();
        if n < 2 { return Ok(None); }
        let k = self.tr.starts[1..n].partition_point(|&s| s <= t).max(1); // index into starts[1..]: packet k ≥ 1
        let (sample, byte) = (self.tr.starts[k], self.tr.samples[k - 1].offset);
        self.tr.next = k - 1;
        self.tr.last_discard = 0;
        self.dec.reset();
        Ok(Some(SeekPoint { byte, sample, exact: true, table: "matroska", landed: sample }))
    }
}
