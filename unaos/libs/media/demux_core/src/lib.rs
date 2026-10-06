// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `demux_core` — the container half of UnaOS video playback (PLAYBACK, LEDGER SR26).
//!
//! CHARTER: Stria — shared-core. Stria (`handlers/stria`, CODEX A/V handler, the A/V owner by
//! Amendment II) owns playback; this crate is the part of it both rings link: it turns a container
//! file into tracks and timestamped compressed packets. It decodes nothing — a packet's payload goes
//! to a codec (AV1 from `unaos/libs/media/av1_core`, LEDGER SR24) — and it does no I/O: the caller
//! hands it the file's bytes.
//!
//! # Formats, from the specifications
//!
//! * **ISOBMFF / MP4** (ISO/IEC 14496-12): `ftyp`; `moov` → `mvhd`, `trak` → `tkhd`, `edts/elst`,
//!   `mdia` → `mdhd`, `hdlr`, `minf/stbl` → `stsd` (`av01`, `avc1`/`avc3`, `hvc1`/`hev1`, `vp08`,
//!   `vp09`, `mp4a`, `Opus`, `fLaC`, and UnaOS's `utp1` test pattern), `stts`, `ctts` (v0 and v1),
//!   `stsc`, `stsz`/`stz2`, `stco`/`co64`, `stss`; `mvex` → `mehd`, `trex`; fragmented `moof` →
//!   `traf` → `tfhd`, `tfdt`, `trun` (all optional fields, both base-offset rules). See [`mp4`].
//! * **Matroska / WebM** (RFC 9559): EBML header and DocType; `Segment` (known or unknown size) →
//!   `Info` (TimestampScale, Duration), `Tracks/TrackEntry` (number, type, CodecID, CodecPrivate,
//!   DefaultDuration, CodecDelay, Video pixel size, Audio rate/channels, header-stripping
//!   ContentCompression), `Cluster` (known or unknown size) → `Timestamp`, `SimpleBlock`,
//!   `BlockGroup` (`Block`, `BlockDuration`, `ReferenceBlock`); Xiph, fixed and EBML lacing. See
//!   [`mkv`].
//!
//! # One API
//!
//! [`Demuxer::open`] probes the bytes, builds every track's sample table, and merges the tables in
//! decode-time order. [`Demuxer::next_packet`] then yields [`Packet`]s; [`Demuxer::seek`] lands on
//! the last keyframe of the reference track at or before a time. Times are in each track's
//! [`Timebase`]; [`Track::to_ns`] converts.
//!
//! [`build`] holds minimal *writers* for both containers — the test media the KATs and Stria's
//! test-pattern pipeline are made from, and the remux path that lets Chromium act as an oracle for
//! fragmented MP4 and WebM built around a real AV1 stream.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

pub mod build;
pub mod mkv;
pub mod mime;
pub mod facts;
pub mod mp4;
mod read;

/// Why a file could not be demuxed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// A field ran past the end of its box/element or of the file.
    Truncated,
    /// Neither an ISOBMFF nor an EBML file.
    UnknownFormat,
    /// Structurally wrong (the static text names the rule broken).
    Invalid(&'static str),
    /// Valid, but outside what this core implements (encryption, zlib-compressed tracks, ...).
    Unsupported(&'static str),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Error::Truncated => f.write_str("truncated container"),
            Error::UnknownFormat => f.write_str("not an MP4 or Matroska/WebM file"),
            Error::Invalid(s) => write!(f, "invalid container: {s}"),
            Error::Unsupported(s) => write!(f, "unsupported container feature: {s}"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Mp4,
    Matroska,
    WebM,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Video,
    Audio,
    Other,
}

/// The codec a track's packets are for. `Other` keeps the container's own name (the fourcc or
/// CodecID) so nothing is lost.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Codec {
    Av1,
    Avc,
    Hevc,
    Vp8,
    Vp9,
    Aac,
    Opus,
    Vorbis,
    Flac,
    /// MPEG-1/2 Audio Layer III (MP4 ObjectTypeIndication 0x69/0x6B or `.mp3`; Matroska
    /// `A_MPEG/L3`) — AUDIOTRACK (SR45): one packet is one frame.
    Mp3,
    /// Uncompressed integer or IEEE-float PCM, interleaved (MP4 `sowt`/`twos`/`ipcm`/`fpcm`,
    /// Matroska `A_PCM/INT/LIT`, `A_PCM/INT/BIG`, `A_PCM/FLOAT/IEEE`).
    Pcm { bits: u16, float: bool, big_endian: bool },
    /// UnaOS's deterministic test-pattern stream (`utp1` / `V_UNAOS/TESTPATTERN`): each packet is
    /// a [`build::TestPatternPacket`]. Lets playback be proven before a real decoder exists.
    TestPattern,
    Other(String),
}

/// Seconds per tick = `num / den`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timebase {
    pub num: u64,
    pub den: u64,
}

impl Timebase {
    pub const NANOS: Timebase = Timebase { num: 1, den: 1_000_000_000 };
    /// Ticks → nanoseconds, rounded to nearest, without overflow for any i64 tick count.
    pub fn to_ns(&self, ticks: i64) -> i64 {
        let n = ticks as i128 * self.num as i128 * 1_000_000_000;
        let d = self.den.max(1) as i128;
        let q = if n >= 0 { (n + d / 2) / d } else { (n - d / 2) / d };
        q as i64
    }
    pub fn from_ns(&self, ns: i64) -> i64 {
        let n = ns as i128 * self.den as i128;
        let d = (self.num.max(1) as i128) * 1_000_000_000;
        let q = if n >= 0 { (n + d / 2) / d } else { (n - d / 2) / d };
        q as i64
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    /// The container's own id (MP4 `track_ID`, Matroska `TrackNumber`); [`Packet::track`] carries it.
    pub id: u32,
    pub kind: TrackKind,
    pub codec: Codec,
    /// The raw codec name: an MP4 sample-entry fourcc or a Matroska CodecID.
    pub codec_name: String,
    /// The decoder configuration record: `av1C`/`avcC`/`hvcC`/`vpcC`/`dOps` body, the ES
    /// descriptor's DecoderSpecificInfo for `mp4a`, or Matroska CodecPrivate.
    pub config: Vec<u8>,
    pub timebase: Timebase,
    /// Coded size for video (sample entry / PixelWidth×PixelHeight).
    pub width: u32,
    pub height: u32,
    pub sample_rate: u32,
    pub channels: u16,
    /// Matroska CodecDelay / MP4 Opus pre-skip, in ns (audio priming the decoder discards).
    pub codec_delay_ns: u64,
    /// Number of packets the file holds for this track.
    pub sample_count: u64,
    /// Presentation span of the track in ns: max(pts + duration) − min(pts) over its packets.
    pub duration_ns: u64,
    /// Matroska header stripping: bytes prepended to every frame of this track.
    pub frame_prefix: Vec<u8>,
    /// AUDIOTRACK (SR45) gapless facts the edit list alone does not carry, in ns of presentation:
    /// leading priming to discard when no edit list already shifted it (Apple `iTunSMPB`), and
    /// the presented length (the media edit's segment duration, or `iTunSMPB`'s total) — the
    /// decoder discards what lies past it. Zero / `None` when the file says nothing.
    pub trim_start_ns: u64,
    pub play_ns: Option<u64>,
    /// MP4ONE (rmbp B464): the media edit as the file states it (MP4 `elst`), the raw numbers the ns fields
    /// above are derived from — an audio decoder trims in its own sample units from these, with no ns round
    /// trip (`audio_core`'s MP4 path). `None` without an edit list (and for Matroska).
    pub edit: Option<Edit>,
    /// MP4ONE (rmbp B464): Apple's `iTunSMPB` (priming, total samples) as the file states it, on every
    /// audio track of an MP4 that carries one; `None` elsewhere.
    pub smpb: Option<(u64, u64)>,
}

/// MP4ONE (rmbp B464): the first media edit of an MP4 `elst` (ISO/IEC 14496-12 §8.6.6) — the edit the
/// presentation shift and [`Track::play_ns`] come from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Edit {
    /// `media_time` in media (mdhd) ticks.
    pub media_time: i64,
    /// `segment_duration` in movie (mvhd) ticks.
    pub segment: u64,
    pub movie_timescale: u32,
}

impl Track {
    pub fn to_ns(&self, ticks: i64) -> i64 {
        self.timebase.to_ns(ticks)
    }
}

/// One compressed access unit. `pts`/`dts`/`duration` are in the track's [`Timebase`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packet {
    pub track: u32,
    pub pts: i64,
    pub dts: i64,
    pub duration: u64,
    pub keyframe: bool,
    pub data: Vec<u8>,
    /// Matroska `DiscardPadding` in ns (audio to drop at the end of this packet's output); 0
    /// elsewhere.
    pub discard_ns: u64,
}

/// A sample-table entry: where a packet lives, without its bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Sample {
    /// Index into [`Demuxer::tracks`].
    pub track_index: usize,
    pub offset: u64,
    pub size: u32,
    pub pts: i64,
    pub dts: i64,
    pub duration: u64,
    pub keyframe: bool,
    /// Matroska BlockGroup `DiscardPadding` (ns of decoded audio to drop at this frame's end;
    /// RFC 9559 §5.1.3.5.6); 0 elsewhere.
    pub discard_ns: u64,
}

/// What a format parser hands back: tracks plus each track's samples in decode order.
pub(crate) struct Parsed {
    pub format: Format,
    pub tracks: Vec<Track>,
    pub samples: Vec<Vec<Sample>>,
    /// The container's declared duration (mvhd/mehd, Info/Duration), if any.
    pub declared_duration_ns: Option<u64>,
}

pub struct Demuxer {
    data: Vec<u8>,
    format: Format,
    tracks: Vec<Track>,
    /// All tracks' samples merged in decode-time order (ties: track order).
    order: Vec<Sample>,
    pos: usize,
    declared_duration_ns: Option<u64>,
}

/// Which container the bytes are, by signature: ISOBMFF puts a box type at offset 4, EBML starts
/// with its 4-byte magic.
pub fn probe(data: &[u8]) -> Option<Format> {
    if data.len() >= 4 && data[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
        return Some(Format::Matroska);
    }
    if data.len() >= 8 {
        let t = &data[4..8];
        if matches!(t, b"ftyp" | b"moov" | b"styp" | b"mdat" | b"free" | b"skip" | b"wide" | b"moof") {
            return Some(Format::Mp4);
        }
    }
    None
}

impl Demuxer {
    pub fn open(data: Vec<u8>) -> Result<Demuxer, Error> {
        Demuxer::open_with(data, false)
    }

    /// MP4ONE (rmbp B464): a file cut short (a partial copy, an interrupted download) opens up to its cut —
    /// an MP4 top-level box that runs past the end is clamped to it and the samples past the end are dropped,
    /// where [`Demuxer::open`] refuses the file. A whole file opens exactly as with [`Demuxer::open`].
    pub fn open_partial(data: Vec<u8>) -> Result<Demuxer, Error> {
        Demuxer::open_with(data, true)
    }

    fn open_with(data: Vec<u8>, partial: bool) -> Result<Demuxer, Error> {
        let parsed = match probe(&data) {
            Some(Format::Mp4) => mp4::parse(&data, partial)?,
            Some(_) => mkv::parse(&data)?,
            None => return Err(Error::UnknownFormat),
        };
        let Parsed { format, mut tracks, mut samples, declared_duration_ns } = parsed;
        if partial {
            let len = data.len() as u64;
            for s in samples.iter_mut() {
                s.retain(|x| x.offset.checked_add(x.size as u64).is_some_and(|e| e <= len));
            }
        }
        for (i, t) in tracks.iter_mut().enumerate() {
            let s = &samples[i];
            t.sample_count = s.len() as u64;
            if let (Some(lo), Some(hi)) = (
                s.iter().map(|x| x.pts).min(),
                s.iter().map(|x| x.pts.saturating_add(x.duration as i64)).max(),
            ) {
                t.duration_ns = t.timebase.to_ns(hi - lo).max(0) as u64;
            }
        }
        let order = merge(&tracks, samples);
        for s in &order {
            let end = s.offset.checked_add(s.size as u64).ok_or(Error::Invalid("sample offset overflow"))?;
            if end > data.len() as u64 {
                return Err(Error::Truncated);
            }
        }
        Ok(Demuxer { data, format, tracks, order, pos: 0, declared_duration_ns })
    }

    pub fn format(&self) -> Format {
        self.format
    }
    /// MP4ONE (rmbp B464): the file's bytes back, for a caller that keeps the sample table's offsets and reads
    /// the payloads itself (`audio_core`'s MP4 path) — no second copy of the file.
    pub fn into_data(self) -> Vec<u8> {
        self.data
    }
    pub fn tracks(&self) -> &[Track] {
        &self.tracks
    }
    pub fn track_index(&self, id: u32) -> Option<usize> {
        self.tracks.iter().position(|t| t.id == id)
    }
    /// The first video track, the clock reference for seeking.
    pub fn video_track(&self) -> Option<&Track> {
        self.tracks.iter().find(|t| t.kind == TrackKind::Video)
    }
    pub fn audio_track(&self) -> Option<&Track> {
        self.tracks.iter().find(|t| t.kind == TrackKind::Audio)
    }
    /// The container's declared duration if it has one, else the longest track span.
    pub fn duration_ns(&self) -> u64 {
        match self.declared_duration_ns {
            Some(d) if d > 0 => d,
            _ => self.tracks.iter().map(|t| t.duration_ns).max().unwrap_or(0),
        }
    }
    pub fn declared_duration_ns(&self) -> Option<u64> {
        self.declared_duration_ns
    }
    /// Every sample, merged in decode order (metadata only).
    pub fn samples(&self) -> &[Sample] {
        &self.order
    }
    /// One track's samples in decode order.
    pub fn track_samples(&self, track_index: usize) -> impl Iterator<Item = &Sample> {
        self.order.iter().filter(move |s| s.track_index == track_index)
    }
    pub fn packet_at(&self, s: &Sample) -> Packet {
        let t = &self.tracks[s.track_index];
        let body = &self.data[s.offset as usize..(s.offset + s.size as u64) as usize];
        let mut data = Vec::with_capacity(t.frame_prefix.len() + body.len());
        data.extend_from_slice(&t.frame_prefix);
        data.extend_from_slice(body);
        Packet { track: t.id, pts: s.pts, dts: s.dts, duration: s.duration, keyframe: s.keyframe, data, discard_ns: s.discard_ns }
    }
    pub fn next_packet(&mut self) -> Option<Packet> {
        let s = *self.order.get(self.pos)?;
        self.pos += 1;
        Some(self.packet_at(&s))
    }
    pub fn rewind(&mut self) {
        self.pos = 0;
    }
    /// Seek to the last keyframe of the reference track (first video track, else track 0) whose
    /// presentation time is ≤ `target_ns` (the first keyframe if none is). Packets of every track
    /// are then yielded from that keyframe's decode time on. Returns the keyframe's pts in ns.
    pub fn seek(&mut self, target_ns: i64) -> Option<i64> {
        let ref_idx = self
            .tracks
            .iter()
            .position(|t| t.kind == TrackKind::Video)
            .or(if self.tracks.is_empty() { None } else { Some(0) })?;
        self.seek_track(ref_idx, target_ns)
    }

    /// SEEKTABLE (rmbp-ledger B433): the same seek with any track as the reference — an audio track's samples
    /// are all sync samples (no `stss`), so this lands on the access unit whose presentation time is the last at
    /// or before `target_ns` (`stts` times, `stsc`+`stco`+`stsz` offsets). Returns that sample's pts in ns.
    pub fn seek_track(&mut self, ref_idx: usize, target_ns: i64) -> Option<i64> {
        if ref_idx >= self.tracks.len() {
            return None;
        }
        let tb = self.tracks[ref_idx].timebase;
        let mut best: Option<(usize, i64)> = None;
        let mut first: Option<(usize, i64)> = None;
        for (i, s) in self.order.iter().enumerate() {
            if s.track_index != ref_idx || !s.keyframe {
                continue;
            }
            let p = tb.to_ns(s.pts);
            if first.is_none() {
                first = Some((i, p));
            }
            if p <= target_ns && best.is_none_or(|(_, bp)| p >= bp) {
                best = Some((i, p));
            }
        }
        let (i, p) = best.or(first)?;
        let d = tb.to_ns(self.order[i].dts);
        // First index (any track) whose decode time reaches the keyframe's; the keyframe itself
        // is at or after it, so the reference track restarts exactly on its keyframe.
        let mut start = i;
        while start > 0 {
            let prev = &self.order[start - 1];
            if self.tracks[prev.track_index].timebase.to_ns(prev.dts) >= d && prev.track_index != ref_idx {
                start -= 1;
            } else {
                break;
            }
        }
        self.pos = start;
        Some(p)
    }
}

/// k-way merge of per-track decode-order tables by decode time in ns. Each track's own order is
/// preserved exactly (a merge never reorders within a track).
fn merge(tracks: &[Track], per: Vec<Vec<Sample>>) -> Vec<Sample> {
    let total = per.iter().map(|v| v.len()).sum();
    let mut out = Vec::with_capacity(total);
    let mut idx = alloc::vec![0usize; per.len()];
    loop {
        let mut pick: Option<(usize, i64)> = None;
        for (t, v) in per.iter().enumerate() {
            if let Some(s) = v.get(idx[t]) {
                let d = tracks[t].timebase.to_ns(s.dts);
                if pick.is_none_or(|(_, pd)| d < pd) {
                    pick = Some((t, d));
                }
            }
        }
        let Some((t, _)) = pick else { break };
        out.push(per[t][idx[t]]);
        idx[t] += 1;
    }
    out
}
