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
//! [`survey`] is the editor's probe (NLEPROBE, SR195): one [`survey::Survey`] per file, read from the same
//! tables; [`Demuxer::keyframe_before`] and [`Demuxer::samples_from`] are the editor's two seek reads, which
//! never move the playback cursor.
//!
//! DEMUXSEEK (SR200): [`Demuxer::derive_dts`] gives a Matroska video track decode times from the codec core's reorder
//! depth ([`DecodeTime`]); [`Demuxer::open_indexed`] opens a file by its own index (Matroska Cues, MP4 `sidx` / `mfra`)
//! without walking its Clusters or fragments, and [`Demuxer::load_at`] reads the span that holds a time;
//! [`Demuxer::edit_list`] keeps every `elst` entry; [`Sample::depends`] carries `sdtp` / the trun dependency bits, and
//! [`SampleRead::skip`] marks the leading pictures an open-GOP start cannot decode (the LEADSKIP rule).
//!
//! [`build`] holds minimal *writers* for both containers — the test media the KATs and Stria's
//! test-pattern pipeline are made from, and the remux path that lets Chromium act as an oracle for
//! fragmented MP4 and WebM built around a real AV1 stream.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::sync::Arc;
use alloc::vec::Vec;

pub mod build;
/// AETHERMEDIA2 (SR134): the MSE byte-stream segment parser, coded frame processing and journal.
pub mod fragment;
pub mod mkv;
pub mod mime;
pub mod facts;
/// HEVCHEIC (SR220): the HEIF image-item reader (ISO/IEC 23008-12) — items, properties, grids; codec-blind.
pub mod heif;
pub mod mp4;
mod read;
/// NLEPROBE (SR195): the editor's probe of a file — streams, time bases, keyframe index, composition offsets.
pub mod survey;
/// AETHERMEDIA2 (SR134): WebVTT for `<track>`.
pub mod vtt;

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
    /// DEMUXSEEK (SR200): the sample's dependency word in the ISO/IEC 14496-12 §8.6.4 `sdtp` byte layout —
    /// `is_leading << 6 | sample_depends_on << 4 | sample_is_depended_on << 2 | sample_has_redundancy` — from `sdtp`
    /// (progressive) or the `trun` / `tfhd` / `trex` sample_flags bits 27..20 (§8.8.3.1, the same four fields). 0 = unknown
    /// (Matroska, and a file that states nothing).
    pub depends: u8,
}

/// What a format parser hands back: tracks plus each track's samples in decode order.
pub(crate) struct Parsed {
    pub format: Format,
    pub tracks: Vec<Track>,
    pub samples: Vec<Vec<Sample>>,
    /// The container's declared duration (mvhd/mehd, Info/Duration), if any.
    pub declared_duration_ns: Option<u64>,
    /// DEMUXSEEK (SR200): every track's edit list (MP4 `elst`, every entry); empty for Matroska.
    pub edits: Vec<Vec<EditSegment>>,
}

pub struct Demuxer {
    /// VPLAYAUDIO (rmbp B475): the file's bytes, shared — [`Demuxer::share`] hands a second cursor over the same
    /// bytes and tables (the picture job and the sound job of one video: one read, one parse, no byte copy).
    data: Arc<Vec<u8>>,
    format: Format,
    tracks: Vec<Track>,
    /// All tracks' samples merged in decode-time order (ties: track order).
    order: Vec<Sample>,
    pos: usize,
    declared_duration_ns: Option<u64>,
    /// DEMUXSEEK (SR200): where each track's decode times come from.
    decode_time: Vec<DecodeTime>,
    /// DEMUXSEEK (SR200): each track's edit list.
    edits: Vec<Vec<EditSegment>>,
    /// DEMUXSEEK (SR200): the file's seek index (Matroska Cues, MP4 `sidx` / `mfra`) when opened by it.
    index: Vec<IndexPoint>,
    /// DEMUXSEEK (SR200): the spans not yet read, on an [`Demuxer::open_indexed`] open; `None` on a full open.
    lazy: Option<Lazy>,
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
        let Parsed { format, mut tracks, mut samples, declared_duration_ns, edits } = parsed;
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
        let decode_time = alloc::vec![if format == Format::Mp4 { DecodeTime::Stored } else { DecodeTime::Presentation }; tracks.len()];
        Ok(Demuxer { data: Arc::new(data), format, tracks, order, pos: 0, declared_duration_ns, decode_time, edits, index: Vec::new(), lazy: None })
    }

    pub fn format(&self) -> Format {
        self.format
    }
    /// MP4ONE (rmbp B464): the file's bytes back, for a caller that keeps the sample table's offsets and reads
    /// the payloads itself (`audio_core`'s MP4 path) — no second copy of the file.
    /// A shared file's bytes are copied once here (prefer [`Demuxer::bytes`]).
    pub fn into_data(self) -> Vec<u8> {
        Arc::try_unwrap(self.data).unwrap_or_else(|a| (*a).clone())
    }
    /// VPLAYAUDIO (rmbp B475): the file's bytes, shared (no copy) — the sample table's offsets index them.
    pub fn bytes(&self) -> Arc<Vec<u8>> {
        self.data.clone()
    }
    /// VPLAYAUDIO (rmbp B475): a second cursor over the same bytes and tables (from the first packet). The picture
    /// job keeps the file; the sound job reads its own track through the share — one parse, no byte copy.
    pub fn share(&self) -> Demuxer {
        Demuxer {
            data: self.data.clone(),
            format: self.format,
            tracks: self.tracks.clone(),
            order: self.order.clone(),
            pos: 0,
            declared_duration_ns: self.declared_duration_ns,
            decode_time: self.decode_time.clone(),
            edits: self.edits.clone(),
            index: self.index.clone(),
            lazy: self.lazy.clone(),
        }
    }
    /// VPLAYAUDIO (rmbp B475): a sample's payload as a slice of the file (no copy; no `frame_prefix`).
    pub fn sample_data(&self, s: &Sample) -> &[u8] {
        let (o, n) = (s.offset as usize, s.size as usize);
        self.data.get(o..o.saturating_add(n)).unwrap_or(&[])
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
        if self.pos >= self.order.len() && self.lazy.is_some() {
            self.load_next(); // DEMUXSEEK (SR200): an indexed open reads the next span at the window's end
        }
        let s = *self.order.get(self.pos)?;
        self.pos += 1;
        Some(self.packet_at(&s))
    }
    pub fn rewind(&mut self) {
        self.pos = 0;
    }

    /// AETHERMEDIA2 (SR134): the cursor — the index into [`Demuxer::samples`] of the next packet.
    pub fn position(&self) -> usize {
        self.pos
    }

    /// AETHERMEDIA2 (SR134): move the cursor (a player resuming in a rebuilt demuxer after an append).
    pub fn set_position(&mut self, pos: usize) {
        self.pos = pos.min(self.order.len());
    }

    /// AETHERMEDIA2 (SR134): a demuxer over coded frames already in hand (an MSE track buffer):
    /// `frames` pairs a track index with its frame; each track's frames are kept in decode order,
    /// times are nanoseconds (every track gets [`Timebase::NANOS`]), the payloads already carry any
    /// header-stripping prefix, and edit facts are dropped (the fragment parse applied them).
    pub fn from_frames<'a>(
        format: Format,
        mut tracks: Vec<Track>,
        frames: impl IntoIterator<Item = (usize, &'a fragment::CodedFrame)>,
        declared_duration_ns: Option<u64>,
    ) -> Demuxer {
        let mut data: Vec<u8> = Vec::new();
        let mut per: Vec<Vec<Sample>> = (0..tracks.len()).map(|_| Vec::new()).collect();
        for (ti, f) in frames {
            if ti >= tracks.len() {
                continue;
            }
            let offset = data.len() as u64;
            data.extend_from_slice(&f.data);
            per[ti].push(Sample {
                track_index: ti,
                offset,
                size: f.data.len() as u32,
                pts: f.pts_ns,
                dts: f.dts_ns,
                duration: f.duration_ns.max(0) as u64,
                keyframe: f.keyframe,
                discard_ns: f.discard_ns,
                depends: 0,
            });
        }
        for (i, t) in tracks.iter_mut().enumerate() {
            per[i].sort_by_key(|s| s.dts);
            t.timebase = Timebase::NANOS;
            t.frame_prefix.clear();
            t.edit = None;
            t.smpb = None;
            t.play_ns = None;
            t.trim_start_ns = 0;
            t.sample_count = per[i].len() as u64;
            let lo = per[i].iter().map(|x| x.pts).min();
            let hi = per[i].iter().map(|x| x.pts.saturating_add(x.duration as i64)).max();
            t.duration_ns = match (lo, hi) {
                (Some(lo), Some(hi)) => (hi - lo).max(0) as u64,
                _ => 0,
            };
        }
        let order = merge(&tracks, per);
        let decode_time = alloc::vec![if format == Format::Mp4 { DecodeTime::Stored } else { DecodeTime::Presentation }; tracks.len()];
        let edits = alloc::vec![Vec::new(); tracks.len()];
        Demuxer { data: Arc::new(data), format, tracks, order, pos: 0, declared_duration_ns, decode_time, edits, index: Vec::new(), lazy: None }
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
        if self.lazy.is_some() {
            self.load_at(ref_idx, target_ns).ok()?; // DEMUXSEEK (SR200): the index jumps to the span
        }
        let tb = self.tracks[ref_idx].timebase;
        let (i, p) = self.key_at_or_before(ref_idx, target_ns)?;
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

    /// NLEPROBE (SR195): THE keyframe search [`Demuxer::seek_track`] and the editor's reads share — the index into
    /// [`Demuxer::samples`] of the last keyframe of track `ref_idx` whose presentation time is <= `target_ns`
    /// (the first keyframe when none is), and that keyframe's pts in ns.
    fn key_at_or_before(&self, ref_idx: usize, target_ns: i64) -> Option<(usize, i64)> {
        let tb = self.tracks.get(ref_idx)?.timebase;
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
        best.or(first)
    }

    /// NLEPROBE (SR195), the editor's first read: SEEK-TO-KEYFRAME-BEFORE(t). The index into [`Demuxer::samples`]
    /// of the last keyframe of track `track_index` presented at or before `target_ns` (the first keyframe when t
    /// precedes them all), and its pts in ns. Does not move the playback cursor.
    pub fn keyframe_before(&self, track_index: usize, target_ns: i64) -> Option<(usize, i64)> {
        self.key_at_or_before(track_index, target_ns)
    }

    /// NLEPROBE (SR195), the editor's second read: READ-SAMPLES-FROM(t). Track `track_index`'s samples in decode
    /// order from [`Demuxer::keyframe_before`]'s keyframe to the end of the track; `preroll` marks a sample
    /// presented before `target_ns` (decode it, do not show it). Payloads through [`Demuxer::packet_at`] or
    /// [`Demuxer::sample_data`]. Empty when the track has no keyframe. Does not move the playback cursor.
    pub fn samples_from(&self, track_index: usize, target_ns: i64) -> impl Iterator<Item = SampleRead> + '_ {
        let start = self.key_at_or_before(track_index, target_ns).map_or(self.order.len(), |(i, _)| i);
        let tb = self.tracks.get(track_index).map_or(Timebase::NANOS, |t| t.timebase);
        let key = self.order.get(start).copied();
        let mut past_gop = false;
        self.order[start..].iter().enumerate().filter(move |(_, s)| s.track_index == track_index).map(move |(k, s)| {
            // DEMUXSEEK (SR200): LEADSKIP — leading pictures of the start key's GOP that cannot be decoded from it.
            if k > 0 && s.keyframe {
                past_gop = true;
            }
            let skip = match key {
                Some(key) if k > 0 && !past_gop => self.leading_skip(&key, s),
                _ => false,
            };
            SampleRead { sample: *s, preroll: tb.to_ns(s.pts) < target_ns, skip }
        })
    }
}

/// NLEPROBE (SR195): one sample of [`Demuxer::samples_from`] — `preroll` is true when it is presented before the
/// read's target (a reference the decoder needs, not a frame the editor shows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SampleRead {
    pub sample: Sample,
    pub preroll: bool,
    /// DEMUXSEEK (SR200): a leading picture of an open-GOP start key that cannot be decoded from this start (the
    /// LEADSKIP rule, [`Demuxer::samples_from`]): do not decode it.
    pub skip: bool,
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

// ---------------------------------------------------------------------------------------------------
// DEMUXSEEK (LEDGER SR200): decode times derived from the codec's reorder depth, the open by the file's own index,
// the edit list, and the leading pictures an open-GOP start cannot decode.
// ---------------------------------------------------------------------------------------------------

/// DEMUXSEEK (SR200): where a track's decode times come from.
///
/// MP4 stores them (`stts` / `trun`): `Stored`. Matroska stores none — blocks are in decode order and carry their
/// presentation time — so a Matroska track starts as `Presentation` (dts = pts, its composition offsets are 0 and carry no
/// reorder fact) until the caller hands [`Demuxer::derive_dts`] the codec's reorder depth: `Derived(depth)`. The depth is the
/// codec core's number, injected by the caller (this crate links no decoder): `h264_core::ps::avcc_reorder_depth` (the SPS
/// VUI `max_num_reorder_frames`, else the H.264 E.2.1 inference) and `av1_core::image::av1c_reorder_depth` (0: one temporal
/// unit is one shown frame). A VP9 track has no accessor and stays `Presentation` — exact for VP9, whose superframe packs a
/// hidden frame with its shown one so every packet is presented in decode order — and an HEVC track stays `Presentation`
/// until an HEVC core reads `sps_max_num_reorder_pics` (the gap DEMUXSEEK-WIRE / DEMUXSEEK-HEVCDTS name in handlers.md §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeTime {
    Stored,
    Derived(u32),
    Presentation,
}

/// DEMUXSEEK (SR200): one entry of a file's seek index — a sync sample's presentation time and the file offset of the
/// Matroska Cluster (a Cues CuePoint) or the MP4 `moof` (a `sidx` reference, a `tfra` entry) that holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexPoint {
    pub track_index: usize,
    pub pts_ns: i64,
    pub pos: u64,
}

/// DEMUXSEEK (SR200): one entry of an MP4 edit list (ISO/IEC 14496-12 §8.6.6), every entry kept — the segment list an editor
/// cuts by. Playback's pts still follow the first media edit alone ([`Track::edit`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EditSegment {
    /// Where the segment starts on the movie timeline, ns (the sum of the segments before it).
    pub start_ns: i64,
    pub duration_ns: u64,
    /// The first presented media time in the track's ticks on the same axis as [`Sample::pts`] (the file's `media_time` less
    /// the shift the first media edit applied); `None` for an empty edit (`media_time == -1`).
    pub media_pts: Option<i64>,
    /// `media_rate` as 16.16: 0x10000 = normal speed, 0 = a dwell (the frame at `media_pts` held for the duration).
    pub rate: i32,
}

/// What a format's index open hands back: the headers' tracks (no samples), the index and the spans to read.
pub(crate) struct IndexedOpen {
    pub parsed: Parsed,
    pub index: Vec<IndexPoint>,
    /// `[start, end)` file ranges in file order, each a run of Clusters / moofs beginning at an index position.
    pub spans: Vec<(u64, u64)>,
    pub reader: SpanReader,
}

#[derive(Clone)]
pub(crate) enum SpanReader {
    Mkv(mkv::Spans),
    Mp4(mp4::Spans),
}

/// An [`Demuxer::open_indexed`] open's spans and which of them `order` holds.
#[derive(Clone)]
struct Lazy {
    spans: Vec<(u64, u64)>,
    loaded: core::ops::Range<usize>,
    reader: SpanReader,
}

impl Demuxer {
    /// DEMUXSEEK (SR200): derive each Matroska/WebM video track's decode times from the codec's reorder depth, which
    /// `depth_of` returns (see [`DecodeTime`]). With depth d the presentation times sorted ascending give
    /// `dts[i] = sorted[i - d]` (the first d extrapolated back by the smallest frame step): at least i-d+1 of the first i+1
    /// decoded frames are presented no later than frame i, so dts <= pts and dts strictly increases. A depth the samples
    /// contradict (some dts would pass its pts) is refused and the track stays `Presentation`. Call it right after open: the
    /// table is re-merged and the cursor goes to the start. Returns how many tracks were derived. MP4 is left untouched.
    pub fn derive_dts(&mut self, depth_of: impl Fn(&Track) -> Option<u32>) -> usize {
        if self.format == Format::Mp4 {
            return 0;
        }
        let mut per: Vec<Vec<Sample>> = (0..self.tracks.len()).map(|ti| self.track_samples(ti).copied().collect()).collect();
        let mut n = 0;
        for (ti, t) in self.tracks.iter().enumerate() {
            if t.kind != TrackKind::Video || self.decode_time[ti] != DecodeTime::Presentation {
                continue;
            }
            let Some(d) = depth_of(t) else { continue };
            if mkv::derive_dts(&mut per[ti], d) {
                self.decode_time[ti] = DecodeTime::Derived(d);
                n += 1;
            }
        }
        if n > 0 {
            self.order = merge(&self.tracks, per);
            self.pos = 0;
        }
        n
    }

    /// DEMUXSEEK (SR200): open a file by its own seek index — Matroska Cues (found before the first Cluster or through the
    /// SeekHead), MP4 `sidx` or `mfra` — reading the headers and the index only: no Cluster or fragment is walked, and the
    /// sample table starts empty. [`Demuxer::load_at`] reads the span that holds a time ([`Demuxer::seek_track`] calls it),
    /// and [`Demuxer::next_packet`] reads the next span at the end of the loaded window. Track `sample_count` is 0 (unknown)
    /// and `duration_ns` the declared duration. [`Demuxer::samples`], the editor's reads and [`Demuxer::survey`] see the
    /// loaded window; an editor holds its own [`Demuxer::share`]. A file with no usable index (none, a position that is not a
    /// Cluster / moof, a hierarchical `sidx`, a progressive MP4) opens exactly as [`Demuxer::open`].
    pub fn open_indexed(data: Vec<u8>) -> Result<Demuxer, Error> {
        let io = match probe(&data) {
            Some(Format::Mp4) => mp4::parse_index(&data).ok().flatten(),
            Some(_) => mkv::parse_index(&data).ok().flatten(),
            None => return Err(Error::UnknownFormat),
        };
        let Some(IndexedOpen { parsed, index, spans, reader }) = io else {
            return Demuxer::open(data);
        };
        let Parsed { format, mut tracks, declared_duration_ns, edits, .. } = parsed;
        for t in tracks.iter_mut() {
            t.sample_count = 0;
            t.duration_ns = declared_duration_ns.unwrap_or(0);
        }
        let decode_time = alloc::vec![if format == Format::Mp4 { DecodeTime::Stored } else { DecodeTime::Presentation }; tracks.len()];
        Ok(Demuxer {
            data: Arc::new(data),
            format,
            tracks,
            order: Vec::new(),
            pos: 0,
            declared_duration_ns,
            decode_time,
            edits,
            index,
            lazy: Some(Lazy { spans, loaded: 0..0, reader }),
        })
    }

    /// DEMUXSEEK (SR200): true when this Demuxer was opened by its index ([`Demuxer::open_indexed`] found one).
    pub fn indexed(&self) -> bool {
        self.lazy.is_some()
    }

    /// DEMUXSEEK (SR200): the seek index read at an [`Demuxer::open_indexed`] open (empty on a full open).
    pub fn index(&self) -> &[IndexPoint] {
        &self.index
    }

    /// DEMUXSEEK (SR200): each track's edit list ([`EditSegment`]); empty for Matroska and for a file without `elst`.
    pub fn edit_list(&self, track_index: usize) -> &[EditSegment] {
        self.edits.get(track_index).map_or(&[], |e| e.as_slice())
    }

    /// DEMUXSEEK (SR200): on an indexed open, read the span holding the reference track's last index point at or before
    /// `target_ns` (any track's points when `track_index` has none; the first span when no point is that early) into the
    /// sample table, replacing the loaded window, and put the cursor at its start. A full open is left as it is.
    pub fn load_at(&mut self, track_index: usize, target_ns: i64) -> Result<(), Error> {
        let Some(lz) = self.lazy.as_ref() else { return Ok(()) };
        let own = self.index.iter().any(|x| x.track_index == track_index);
        let best = self
            .index
            .iter()
            .filter(|x| (!own || x.track_index == track_index) && x.pts_ns <= target_ns)
            .max_by_key(|x| (x.pts_ns, core::cmp::Reverse(x.pos)));
        let k = best.map_or(0, |x| lz.spans.partition_point(|s| s.0 <= x.pos).saturating_sub(1));
        if k >= lz.spans.len() {
            return Ok(());
        }
        let per = self.read_span(k)?;
        self.order = merge(&self.tracks, per);
        self.pos = 0;
        if let Some(lz) = self.lazy.as_mut() {
            lz.loaded = k..k + 1;
        }
        Ok(())
    }

    /// The next span after the loaded window, appended (an indexed open's `next_packet`).
    fn load_next(&mut self) {
        let Some(lz) = self.lazy.as_ref() else { return };
        let k = lz.loaded.end;
        if k >= lz.spans.len() {
            return;
        }
        let Ok(per) = self.read_span(k) else {
            if let Some(lz) = self.lazy.as_mut() {
                lz.loaded.end = lz.spans.len(); // a span that will not read ends the stream
            }
            return;
        };
        let more = merge(&self.tracks, per);
        self.order.extend(more);
        if let Some(lz) = self.lazy.as_mut() {
            if lz.loaded.is_empty() {
                lz.loaded = k..k + 1;
            } else {
                lz.loaded.end = k + 1;
            }
        }
    }

    /// One span's samples per track, through the format's own Cluster / moof reader; a `Derived` track's dts derived over
    /// the span (a span begins at an index position, a sync sample).
    fn read_span(&self, k: usize) -> Result<Vec<Vec<Sample>>, Error> {
        let lz = self.lazy.as_ref().ok_or(Error::Invalid("not an indexed open"))?;
        let (from, to) = lz.spans[k];
        let last = k + 1 == lz.spans.len();
        let mut per = match &lz.reader {
            SpanReader::Mkv(sp) => mkv::load_span(&self.data, sp, from as usize, to as usize, last)?,
            SpanReader::Mp4(sp) => mp4::load_span(&self.data, sp, from as usize, to as usize)?,
        };
        for (ti, v) in per.iter_mut().enumerate() {
            if let Some(DecodeTime::Derived(d)) = self.decode_time.get(ti) {
                mkv::derive_dts(v, *d);
            }
            for s in v.iter() {
                let end = s.offset.checked_add(s.size as u64).ok_or(Error::Invalid("sample offset overflow"))?;
                if end > self.data.len() as u64 {
                    return Err(Error::Truncated);
                }
            }
        }
        Ok(per)
    }

    /// DEMUXSEEK (SR200), the LEADSKIP rule for [`Demuxer::samples_from`]: `s` follows the start `key` in decode order inside
    /// its GOP. It is a leading picture when presented before the key; a leading picture is skipped when
    /// (1) its sample flags say `is_leading` 1 (a dependency before the key: undecodable from here; 3 = decodable, 2 = not
    /// leading: kept), else (2) for HEVC its first NAL's `nal_unit_type` is RASL_N / RASL_R (8 / 9; RADL_N / RADL_R 6 / 7 are
    /// kept), else (3) always — an IDR has no leading pictures (H.264 8.2.1, HEVC IDR_N_LP), so a picture decoded after the
    /// key and presented before it follows an open-GOP I and references the GOP before it.
    fn leading_skip(&self, key: &Sample, s: &Sample) -> bool {
        if s.pts >= key.pts {
            return false;
        }
        match s.depends >> 6 {
            1 => return true,
            2 | 3 => return false,
            _ => {}
        }
        if let Some(t) = self.tracks.get(s.track_index) {
            if t.codec == Codec::Hevc {
                if let Some(ty) = self.hevc_nal_type(t, s) {
                    match ty {
                        8 | 9 => return true,
                        6 | 7 => return false,
                        _ => {}
                    }
                }
            }
        }
        true
    }

    /// The first NAL's `nal_unit_type` of an HEVC sample (length size from `hvcC` byte 21, ISO/IEC 14496-15 §8.3.3.1).
    fn hevc_nal_type(&self, t: &Track, s: &Sample) -> Option<u8> {
        let len = (*t.config.get(21)? & 3) as usize + 1;
        let b = *self.sample_data(s).get(len)?;
        Some((b >> 1) & 0x3F)
    }
}
