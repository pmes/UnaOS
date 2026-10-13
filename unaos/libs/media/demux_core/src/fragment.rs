// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! FRAGMENT (AETHERMEDIA2, LEDGER SR134) — the byte-stream half of Media Source Extensions (W3C MSE,
//! 2024): the segment parser loop, the coded frame processing algorithm and the SourceBuffer track
//! buffers, as one `no_std` model that both sides of the bus run.
//!
//! CHARTER: Stria — shared-core. Aether (the page's `MediaSource`/`SourceBuffer`) and Stria (the
//! `MediaSourceFeed` that plays the appended frames) apply the SAME [`Op`] journal to the SAME
//! [`MediaSourceModel`], so the page's `buffered` and the player's packets cannot disagree.
//!
//! * [`SegmentParser`] — MSE §"Segment Parser Loop" for the two byte stream formats this core reads:
//!   **ISO BMFF** (MSE ISO BMFF Byte Stream Format: an initialization segment is `ftyp`? + `moov`; a
//!   media segment is `styp`? `moof` + `mdat`; every `traf` carries a `tfdt`) and **WebM** (MSE WebM
//!   Byte Stream Format: EBML header + Segment header + `Info` + `Tracks`; a media segment is one
//!   `Cluster`, known or unknown size). Bytes are kept until a whole segment is present; a media
//!   segment is demuxed by [`crate::mp4`] / [`crate::mkv`] behind its initialization segment (the
//!   init bytes are the parse prefix), so fragment parsing reuses the file demuxers exactly.
//! * [`SourceBufferModel`] — MSE §"Coded Frame Processing": `timestampOffset`, `mode`
//!   ("segments" / "sequence") with group start / end timestamps, the append window, the
//!   need-random-access-point flag, overlap removal with its decoding dependencies, the
//!   discontinuity rule (`dts` backwards or a gap over twice the last frame duration), the
//!   `remove()` range algorithm (to the next random access point), and `buffered` as the
//!   intersection of the track buffers' ranges.
//! * [`Op`] / [`MediaSourceModel`] — the journal: every page-side mutation of a MediaSource is one
//!   record (`[tag u8][len u32 LE][body]`); [`MediaSourceModel::demuxer`] hands the buffered frames
//!   of every SourceBuffer to the player as one [`crate::Demuxer`] (track ids namespaced per buffer).
//!
//! Times are nanoseconds (`i64`); +Infinity is `i64::MAX`.

use alloc::string::String;
use alloc::vec::Vec;

use crate::read::Reader;
use crate::{Demuxer, Error, Format, Track, TrackKind};

/// One coded frame (MSE's unit of the track buffer), in nanoseconds of presentation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodedFrame {
    /// The container's track id (as the initialization segment names it).
    pub track: u32,
    pub pts_ns: i64,
    pub dts_ns: i64,
    pub duration_ns: i64,
    /// A random access point.
    pub keyframe: bool,
    pub data: Vec<u8>,
    pub discard_ns: u64,
}

/// An initialization segment: the tracks, the duration it states, and the bytes a media segment is
/// demuxed behind.
#[derive(Debug, Clone)]
pub struct InitSegment {
    pub format: Format,
    pub tracks: Vec<Track>,
    pub duration_ns: Option<u64>,
    prefix: Vec<u8>,
}

/// What the parser loop completed.
#[derive(Debug, Clone)]
pub enum Segment {
    Init(InitSegment),
    Media(Vec<CodedFrame>),
}

// WebM element ids (RFC 9559 / MSE WebM byte stream format).
const ID_EBML: u32 = 0x1A45_DFA3;
const ID_SEGMENT: u32 = 0x1853_8067;
const ID_INFO: u32 = 0x1549_A966;
const ID_TRACKS: u32 = 0x1654_AE6B;
const ID_CLUSTER: u32 = 0x1F43_B675;
/// A Segment child that ends an unknown-size Cluster (with the EBML header of a new stream).
const LEVEL1: [u32; 9] = [ID_CLUSTER, 0x114D_9B74, ID_INFO, ID_TRACKS, 0x1C53_BB6B, 0x1941_A469, 0x1043_A770, 0x1254_C367, ID_EBML];
/// A Segment element of unknown size: the prefix every WebM media segment is parsed behind.
const SEGMENT_UNKNOWN: [u8; 12] = [0x18, 0x53, 0x80, 0x67, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF];
const UNKNOWN: u64 = u64::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stream {
    Unknown,
    Mp4,
    WebM,
}

/// MSE's segment parser loop over an append's bytes. Keeps what is not yet a whole segment.
#[derive(Debug, Clone)]
pub struct SegmentParser {
    buf: Vec<u8>,
    stream: Stream,
    init: Option<InitSegment>,
    /// ISO BMFF: a `moof` waiting for its `mdat`.
    moof: Option<Vec<u8>>,
    /// WebM: the EBML header, and the Info / Tracks elements of the stream being read.
    ebml: Vec<u8>,
    info: Option<Vec<u8>>,
    tracks: Option<Vec<u8>>,
    /// Info or Tracks arrived since the last initialization segment was emitted.
    fresh: bool,
    in_segment: bool,
}

impl Default for SegmentParser {
    fn default() -> Self {
        Self::new()
    }
}

/// One element/box header read at the front of `b`: (id or fourcc as u32, header length, body size
/// or [`UNKNOWN`]). `None` when `b` does not yet hold the whole header.
fn ebml_header(b: &[u8]) -> Result<Option<(u32, usize, u64)>, Error> {
    let mut r = Reader::new(b);
    let id = match crate::mkv::read_id(&mut r) {
        Ok(id) => id,
        Err(Error::Truncated) => return Ok(None),
        Err(e) => return Err(e),
    };
    let (size, _) = match crate::mkv::read_vint(&mut r) {
        Ok(v) => v,
        Err(Error::Truncated) => return Ok(None),
        Err(e) => return Err(e),
    };
    Ok(Some((id, r.pos(), size)))
}

fn box_header(b: &[u8]) -> Result<Option<([u8; 4], usize, u64)>, Error> {
    if b.len() < 8 {
        return Ok(None);
    }
    let size32 = u32::from_be_bytes([b[0], b[1], b[2], b[3]]);
    let kind = [b[4], b[5], b[6], b[7]];
    let (size, hdr) = match size32 {
        0 => return Err(Error::Invalid("a size-0 box in a byte stream")),
        1 => {
            if b.len() < 16 {
                return Ok(None);
            }
            (u64::from_be_bytes([b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]]), 16usize)
        }
        n => (n as u64, 8usize),
    };
    if size < hdr as u64 {
        return Err(Error::Invalid("box smaller than its header"));
    }
    Ok(Some((kind, hdr, size)))
}

/// Every coded frame of a demuxed media segment, in decode order.
fn frames_of(d: &Demuxer) -> Vec<CodedFrame> {
    let mut out = Vec::new();
    let order: Vec<crate::Sample> = d.samples().to_vec();
    for s in order {
        let t = &d.tracks()[s.track_index];
        let p = d.packet_at(&s);
        out.push(CodedFrame {
            track: t.id,
            pts_ns: t.to_ns(s.pts),
            dts_ns: t.to_ns(s.dts),
            duration_ns: t.to_ns(s.duration as i64),
            keyframe: s.keyframe,
            data: p.data,
            discard_ns: s.discard_ns,
        });
    }
    out
}

impl SegmentParser {
    pub fn new() -> Self {
        SegmentParser { buf: Vec::new(), stream: Stream::Unknown, init: None, moof: None, ebml: Vec::new(), info: None, tracks: None, fresh: false, in_segment: false }
    }

    /// The last initialization segment received.
    pub fn init(&self) -> Option<&InitSegment> {
        self.init.as_ref()
    }

    /// Bytes held that are not yet a whole segment.
    pub fn pending(&self) -> usize {
        self.buf.len() + self.moof.as_ref().map_or(0, |m| m.len())
    }

    /// MSE "reset parser state" (`abort()`, an append error): drop the partial bytes; the last
    /// initialization segment stays.
    pub fn reset(&mut self) {
        self.buf.clear();
        self.moof = None;
        if self.stream == Stream::WebM && self.init.is_some() {
            // the next bytes continue the same Segment (clusters, or a new EBML header)
            self.in_segment = true;
        }
    }

    /// Append `bytes` and run the loop: every whole segment now present, in order.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Segment>, Error> {
        self.buf.extend_from_slice(bytes);
        self.run(false)
    }

    /// End of stream: an unknown-size WebM Cluster ends at the last whole element it holds.
    pub fn flush(&mut self) -> Result<Vec<Segment>, Error> {
        self.run(true)
    }

    fn run(&mut self, eos: bool) -> Result<Vec<Segment>, Error> {
        if self.stream == Stream::Unknown {
            if self.buf.len() >= 4 && self.buf[..4] == [0x1A, 0x45, 0xDF, 0xA3] {
                self.stream = Stream::WebM;
            } else if self.buf.len() >= 8 {
                self.stream = Stream::Mp4;
            } else {
                return Ok(Vec::new()); // too few bytes to tell
            }
        }
        match self.stream {
            Stream::Mp4 => self.run_mp4(),
            Stream::WebM => self.run_webm(eos),
            Stream::Unknown => Ok(Vec::new()),
        }
    }

    fn run_mp4(&mut self) -> Result<Vec<Segment>, Error> {
        let mut out = Vec::new();
        let mut p = 0usize;
        loop {
            let Some((kind, _hdr, size)) = box_header(&self.buf[p..])? else { break };
            if size > (self.buf.len() - p) as u64 {
                break; // wait for the rest of the box
            }
            let end = p + size as usize;
            let bx = &self.buf[p..end];
            match &kind {
                b"moov" => {
                    let d = Demuxer::open(bx.to_vec())?;
                    let init = InitSegment { format: Format::Mp4, tracks: d.tracks().to_vec(), duration_ns: d.declared_duration_ns(), prefix: bx.to_vec() };
                    self.init = Some(init.clone());
                    out.push(Segment::Init(init));
                }
                b"moof" => {
                    if self.init.is_none() {
                        return Err(Error::Invalid("a media segment before any initialization segment"));
                    }
                    self.moof = Some(bx.to_vec());
                }
                b"mdat" => {
                    let Some(moof) = self.moof.take() else { return Err(Error::Invalid("mdat outside a media segment")) };
                    let init = self.init.as_ref().ok_or(Error::Invalid("a media segment before any initialization segment"))?;
                    if !moof_has_tfdt(&moof)? {
                        return Err(Error::Invalid("traf without tfdt (MSE ISO BMFF byte stream format)"));
                    }
                    let mut data = Vec::with_capacity(init.prefix.len() + moof.len() + bx.len());
                    data.extend_from_slice(&init.prefix);
                    data.extend_from_slice(&moof);
                    data.extend_from_slice(bx);
                    let d = Demuxer::open(data)?;
                    out.push(Segment::Media(frames_of(&d)));
                }
                // ftyp, styp, sidx, ssix, prft, emsg, free, skip, uuid, ...: no coded frames
                _ => {}
            }
            p = end;
        }
        self.buf.drain(..p);
        Ok(out)
    }

    fn run_webm(&mut self, eos: bool) -> Result<Vec<Segment>, Error> {
        let mut out = Vec::new();
        let mut p = 0usize;
        loop {
            let Some((id, hdr, size)) = ebml_header(&self.buf[p..])? else { break };
            let avail = self.buf.len() - p;
            if id == ID_EBML {
                if size == UNKNOWN {
                    return Err(Error::Invalid("EBML header of unknown size"));
                }
                if hdr as u64 + size > avail as u64 {
                    break;
                }
                let end = p + hdr + size as usize;
                // a new stream: a new initialization segment follows
                self.ebml = self.buf[p..end].to_vec();
                self.info = None;
                self.tracks = None;
                self.fresh = false;
                self.in_segment = false;
                p = end;
                continue;
            }
            if id == ID_SEGMENT {
                if self.ebml.is_empty() {
                    return Err(Error::Invalid("Segment before the EBML header"));
                }
                self.in_segment = true;
                p += hdr; // the Segment's children follow; its size is not waited for
                continue;
            }
            if !self.in_segment {
                return Err(Error::Invalid("WebM element outside the Segment"));
            }
            if id == ID_CLUSTER {
                if self.tracks.is_some() && self.fresh {
                    out.push(Segment::Init(self.make_init()?));
                }
                let init = self.init.as_ref().ok_or(Error::Invalid("a Cluster before any initialization segment"))?;
                let end = if size == UNKNOWN {
                    match unknown_cluster_end(&self.buf[p + hdr..], eos)? {
                        Some(n) => p + hdr + n,
                        None => break,
                    }
                } else {
                    if hdr as u64 + size > avail as u64 {
                        break;
                    }
                    p + hdr + size as usize
                };
                let mut data = Vec::with_capacity(init.prefix.len() + end - p);
                data.extend_from_slice(&init.prefix);
                data.extend_from_slice(&self.buf[p..end]);
                let d = Demuxer::open(data)?;
                out.push(Segment::Media(frames_of(&d)));
                p = end;
                continue;
            }
            if size == UNKNOWN {
                return Err(Error::Unsupported("unknown-size element other than Segment/Cluster"));
            }
            if hdr as u64 + size > avail as u64 {
                break;
            }
            let end = p + hdr + size as usize;
            match id {
                ID_INFO => {
                    self.info = Some(self.buf[p..end].to_vec());
                    self.fresh = true;
                }
                ID_TRACKS => {
                    self.tracks = Some(self.buf[p..end].to_vec());
                    self.fresh = true;
                }
                _ => {} // SeekHead, Cues, Tags, Chapters, Attachments, Void, CRC-32
            }
            p = end;
            if self.tracks.is_some() && self.info.is_some() && self.fresh {
                out.push(Segment::Init(self.make_init()?));
            }
        }
        self.buf.drain(..p);
        Ok(out)
    }

    fn make_init(&mut self) -> Result<InitSegment, Error> {
        let tracks = self.tracks.clone().ok_or(Error::Invalid("no Tracks element"))?;
        let mut prefix = self.ebml.clone();
        prefix.extend_from_slice(&SEGMENT_UNKNOWN);
        if let Some(i) = &self.info {
            prefix.extend_from_slice(i);
        }
        prefix.extend_from_slice(&tracks);
        let d = Demuxer::open(prefix.clone())?;
        let init = InitSegment { format: d.format(), tracks: d.tracks().to_vec(), duration_ns: d.declared_duration_ns(), prefix };
        self.init = Some(init.clone());
        self.fresh = false;
        Ok(init)
    }
}

/// The length of an unknown-size Cluster's body at the front of `b`: it ends where a Level-1 element
/// (or a new EBML header) begins. `None` while it may still grow (not at end of stream).
fn unknown_cluster_end(b: &[u8], eos: bool) -> Result<Option<usize>, Error> {
    let mut p = 0usize;
    loop {
        match ebml_header(&b[p..])? {
            None => return Ok(if eos { Some(p) } else { None }),
            Some((id, hdr, size)) => {
                if LEVEL1.contains(&id) {
                    return Ok(Some(p));
                }
                if size == UNKNOWN {
                    return Err(Error::Invalid("unknown-size element inside a Cluster"));
                }
                if hdr as u64 + size > (b.len() - p) as u64 {
                    return Ok(if eos { Some(p) } else { None });
                }
                p += hdr + size as usize;
            }
        }
    }
}

/// Every `traf` of a `moof` carries a `tfdt` (MSE ISO BMFF byte stream format §"Media Segments").
fn moof_has_tfdt(moof: &[u8]) -> Result<bool, Error> {
    let kids = crate::mp4::boxes(moof, 8, moof.len())?;
    for traf in kids.iter().filter(|b| &b.kind == b"traf") {
        let tk = crate::mp4::boxes(moof, traf.body, traf.end)?;
        if !tk.iter().any(|b| &b.kind == b"tfdt") {
            return Ok(false);
        }
    }
    Ok(true)
}

// ---------------------------------------------------------------------------------------------------
// Coded frame processing (MSE §3.5.8) and the track buffers
// ---------------------------------------------------------------------------------------------------

/// SourceBuffer `mode`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppendMode {
    Segments,
    Sequence,
}

/// One track buffer (MSE §3.5): its coded frames in decode order and the per-track variables.
#[derive(Debug, Clone)]
pub struct TrackBuffer {
    pub track: Track,
    pub frames: Vec<CodedFrame>,
    last_dts: Option<i64>,
    last_dur: i64,
    highest_end: Option<i64>,
    need_rap: bool,
}

impl TrackBuffer {
    fn new(track: Track) -> Self {
        TrackBuffer { track, frames: Vec::new(), last_dts: None, last_dur: 0, highest_end: None, need_rap: true }
    }

    /// The presentation intervals the frames cover, merged where a gap is no longer than the frame
    /// before it lasts (a frame's own duration is its fudge room).
    pub fn ranges(&self) -> Vec<(i64, i64)> {
        let mut iv: Vec<(i64, i64)> = self.frames.iter().map(|f| (f.pts_ns, f.pts_ns + f.duration_ns.max(0))).collect();
        iv.sort_unstable();
        let mut out: Vec<(i64, i64)> = Vec::new();
        let mut prev_dur = 0i64;
        for (s, e) in iv {
            let tol = prev_dur.max(1_000_000);
            match out.last_mut() {
                Some(last) if s <= last.1 + tol => last.1 = last.1.max(e),
                _ => out.push((s, e)),
            }
            prev_dur = e - s;
        }
        out
    }

    /// Remove the frames whose index is in `drop` and every frame that decodes from them, up to the
    /// next random access point (MSE "remove all possible decoding dependencies").
    fn remove_with_dependents(&mut self, mut drop: Vec<usize>) {
        if drop.is_empty() {
            return;
        }
        drop.sort_unstable();
        let mut gone = alloc::vec![false; self.frames.len()];
        for &i in &drop {
            gone[i] = true;
        }
        for &i in &drop {
            let mut j = i + 1;
            while j < self.frames.len() && !self.frames[j].keyframe {
                gone[j] = true;
                j += 1;
            }
        }
        let mut k = 0;
        self.frames.retain(|_| {
            let keep = !gone[k];
            k += 1;
            keep
        });
    }
}

/// Intersection of sorted range lists.
pub fn intersect(a: &[(i64, i64)], b: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() && j < b.len() {
        let s = a[i].0.max(b[j].0);
        let e = a[i].1.min(b[j].1);
        if s < e {
            out.push((s, e));
        }
        if a[i].1 < b[j].1 {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

/// What one append did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AppendOutcome {
    /// An initialization segment arrived; `first_init` when it was this buffer's first.
    pub init: bool,
    pub first_init: bool,
    /// The duration the (first) initialization segment states.
    pub init_duration_ns: Option<u64>,
    /// Coded frames added to the track buffers.
    pub frames: usize,
}

/// One SourceBuffer's state (MSE §3).
#[derive(Debug, Clone)]
pub struct SourceBufferModel {
    pub mime: String,
    pub mode: AppendMode,
    pub timestamp_offset_ns: i64,
    pub append_window_start_ns: i64,
    /// +Infinity = `i64::MAX`.
    pub append_window_end_ns: i64,
    group_start: Option<i64>,
    group_end: i64,
    parser: SegmentParser,
    pub tracks: Vec<TrackBuffer>,
    pub format: Option<Format>,
    /// Bumped by every change to the track buffers.
    pub generation: u64,
}

impl SourceBufferModel {
    pub fn new(mime: &str) -> Self {
        SourceBufferModel {
            mime: String::from(mime),
            mode: AppendMode::Segments,
            timestamp_offset_ns: 0,
            append_window_start_ns: 0,
            append_window_end_ns: i64::MAX,
            group_start: None,
            group_end: 0,
            parser: SegmentParser::new(),
            tracks: Vec::new(),
            format: None,
            generation: 0,
        }
    }

    /// The "first initialization segment received" flag.
    pub fn has_init(&self) -> bool {
        !self.tracks.is_empty()
    }

    /// `mode = "sequence"` sets group start timestamp to group end timestamp (MSE §3.2 mode setter).
    pub fn set_mode(&mut self, mode: AppendMode) {
        if mode == AppendMode::Sequence {
            self.group_start = Some(self.group_end);
        }
        self.mode = mode;
    }

    /// `timestampOffset` setter (in sequence mode it also restarts the coded frame group there).
    pub fn set_timestamp_offset(&mut self, ns: i64) {
        self.timestamp_offset_ns = ns;
        if self.mode == AppendMode::Sequence {
            self.group_start = Some(ns);
        }
    }

    /// `abort()`: reset parser state (MSE §3.5.2) and the append window.
    pub fn abort(&mut self) {
        self.parser.reset();
        for t in &mut self.tracks {
            t.last_dts = None;
            t.last_dur = 0;
            t.highest_end = None;
            t.need_rap = true;
        }
        self.append_window_start_ns = 0;
        self.append_window_end_ns = i64::MAX;
    }

    /// The buffer append algorithm: parse, then coded-frame-process every complete segment.
    pub fn append(&mut self, bytes: &[u8]) -> Result<AppendOutcome, Error> {
        let segs = match self.parser.push(bytes) {
            Ok(s) => s,
            Err(e) => {
                self.parser.reset();
                return Err(e);
            }
        };
        self.consume(segs)
    }

    /// End of stream: segments the parser can only complete now.
    pub fn flush(&mut self) -> Result<AppendOutcome, Error> {
        let segs = self.parser.flush()?;
        self.consume(segs)
    }

    fn consume(&mut self, segs: Vec<Segment>) -> Result<AppendOutcome, Error> {
        let mut out = AppendOutcome::default();
        for s in segs {
            match s {
                Segment::Init(init) => {
                    out.init = true;
                    if self.tracks.is_empty() {
                        out.first_init = true;
                        out.init_duration_ns = init.duration_ns;
                        self.format = Some(init.format);
                        self.tracks = init.tracks.iter().filter(|t| t.kind != TrackKind::Other).cloned().map(TrackBuffer::new).collect();
                        if self.tracks.is_empty() {
                            return Err(Error::Invalid("initialization segment without an audio or video track"));
                        }
                    } else {
                        // MSE §3.5.7: a later init segment keeps the track set (count and kinds)
                        let kinds: Vec<TrackKind> = init.tracks.iter().filter(|t| t.kind != TrackKind::Other).map(|t| t.kind).collect();
                        let have: Vec<TrackKind> = self.tracks.iter().map(|t| t.track.kind).collect();
                        if kinds != have {
                            return Err(Error::Invalid("initialization segment changed the track set"));
                        }
                        for (tb, t) in self.tracks.iter_mut().zip(init.tracks.iter().filter(|t| t.kind != TrackKind::Other)) {
                            tb.track = t.clone();
                            tb.need_rap = true;
                        }
                    }
                    self.generation += 1;
                }
                Segment::Media(frames) => {
                    if self.tracks.is_empty() {
                        return Err(Error::Invalid("a media segment before any initialization segment"));
                    }
                    for f in frames {
                        if self.process(f) {
                            out.frames += 1;
                        }
                    }
                    self.generation += 1;
                }
            }
        }
        Ok(out)
    }

    /// MSE §3.5.8 for one frame. True when it was added.
    fn process(&mut self, frame: CodedFrame) -> bool {
        let Some(ti) = self.tracks.iter().position(|t| t.track.id == frame.track) else { return false };
        let dur = frame.duration_ns.max(0);
        loop {
            // 1.1: sequence mode starts a new coded frame group at group start
            if self.mode == AppendMode::Sequence {
                if let Some(gs) = self.group_start.take() {
                    self.timestamp_offset_ns = gs - frame.pts_ns;
                    self.group_end = gs;
                    for t in &mut self.tracks {
                        t.need_rap = true;
                    }
                }
            }
            let pts = frame.pts_ns.saturating_add(self.timestamp_offset_ns);
            let dts = frame.dts_ns.saturating_add(self.timestamp_offset_ns);
            let tb = &self.tracks[ti];
            // 1.6: a discontinuity
            if let Some(last) = tb.last_dts {
                if dts < last || dts - last > 2 * tb.last_dur {
                    match self.mode {
                        AppendMode::Segments => self.group_end = pts,
                        AppendMode::Sequence => self.group_start = Some(self.group_end),
                    }
                    for t in &mut self.tracks {
                        t.last_dts = None;
                        t.last_dur = 0;
                        t.highest_end = None;
                        t.need_rap = true;
                    }
                    continue;
                }
            }
            let end = pts.saturating_add(dur);
            // 1.9-1.10: the append window
            if pts < self.append_window_start_ns || end > self.append_window_end_ns {
                self.tracks[ti].need_rap = true;
                return false;
            }
            let tb = &mut self.tracks[ti];
            // 1.11: wait for a random access point
            if tb.need_rap {
                if !frame.keyframe {
                    return false;
                }
                tb.need_rap = false;
            }
            // 1.13-1.15: overlapped frames and their dependents
            let mut drop = Vec::new();
            for (i, f) in tb.frames.iter().enumerate() {
                let overlaps = match tb.highest_end {
                    None => (f.pts_ns >= pts && f.pts_ns < end) || (tb.last_dts.is_none() && f.pts_ns < pts && pts < f.pts_ns + f.duration_ns),
                    Some(h) if h <= pts => f.pts_ns >= h && f.pts_ns < end,
                    Some(_) => false,
                };
                if overlaps {
                    drop.push(i);
                }
            }
            tb.remove_with_dependents(drop);
            // 1.16: add, in decode order
            let at = tb.frames.iter().rposition(|f| f.dts_ns <= dts).map_or(0, |i| i + 1);
            tb.frames.insert(at, CodedFrame { pts_ns: pts, dts_ns: dts, duration_ns: dur, ..frame });
            tb.last_dts = Some(dts);
            tb.last_dur = dur;
            tb.highest_end = Some(tb.highest_end.map_or(end, |h| h.max(end)));
            if end > self.group_end {
                self.group_end = end;
            }
            return true;
        }
    }

    /// The range removal algorithm (MSE §3.5.9) for [start, end).
    pub fn remove(&mut self, start: i64, end: i64, duration_ns: i64) {
        for tb in &mut self.tracks {
            // remove end timestamp: the first random access point at or after `end`, else duration
            let mut remove_end = duration_ns;
            for f in &tb.frames {
                if f.keyframe && f.pts_ns >= end && f.pts_ns < remove_end {
                    remove_end = f.pts_ns;
                }
            }
            let drop: Vec<usize> = tb.frames.iter().enumerate().filter(|(_, f)| f.pts_ns >= start && f.pts_ns < remove_end).map(|(i, _)| i).collect();
            tb.remove_with_dependents(drop);
            tb.need_rap = true;
        }
        self.generation += 1;
    }

    /// `SourceBuffer.buffered`: the intersection of the track buffers' ranges.
    pub fn buffered(&self) -> Vec<(i64, i64)> {
        let mut it = self.tracks.iter();
        let Some(first) = it.next() else { return Vec::new() };
        let mut acc = first.ranges();
        for t in it {
            acc = intersect(&acc, &t.ranges());
        }
        acc
    }

    /// The highest presentation end over the track buffers.
    pub fn highest_end(&self) -> Option<i64> {
        self.tracks.iter().flat_map(|t| t.frames.iter().map(|f| f.pts_ns + f.duration_ns)).max()
    }
}

// ---------------------------------------------------------------------------------------------------
// The journal: one MediaSource's mutations, applied the same way on both sides of the bus
// ---------------------------------------------------------------------------------------------------

/// `MediaSource.readyState`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadyState {
    Closed,
    Open,
    Ended,
}

/// One page-side mutation of a MediaSource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    AddSourceBuffer { mime: String },
    RemoveSourceBuffer { sb: u16 },
    Append { sb: u16, bytes: Vec<u8> },
    Remove { sb: u16, start_ns: i64, end_ns: i64 },
    Abort { sb: u16 },
    TimestampOffset { sb: u16, ns: i64 },
    Mode { sb: u16, sequence: bool },
    AppendWindow { sb: u16, start_ns: i64, end_ns: i64 },
    /// `duration` (ns; `i64::MAX` = +Infinity).
    Duration { ns: i64 },
    /// `endOfStream()` (`error` 0 none, 1 network, 2 decode).
    EndOfStream { error: u8 },
    /// An append (or remove) on an ended MediaSource reopened it.
    Reopen,
    /// The element detached (navigation, a new `src`): readyState closed.
    Close,
}

impl Op {
    fn tag(&self) -> u8 {
        match self {
            Op::AddSourceBuffer { .. } => 1,
            Op::RemoveSourceBuffer { .. } => 2,
            Op::Append { .. } => 3,
            Op::Remove { .. } => 4,
            Op::Abort { .. } => 5,
            Op::TimestampOffset { .. } => 6,
            Op::Mode { .. } => 7,
            Op::AppendWindow { .. } => 8,
            Op::Duration { .. } => 9,
            Op::EndOfStream { .. } => 10,
            Op::Reopen => 11,
            Op::Close => 12,
        }
    }

    /// `[tag u8][len u32 LE][body]`.
    pub fn encode(&self, out: &mut Vec<u8>) {
        let mut b: Vec<u8> = Vec::new();
        match self {
            Op::AddSourceBuffer { mime } => b.extend_from_slice(mime.as_bytes()),
            Op::RemoveSourceBuffer { sb } | Op::Abort { sb } => b.extend_from_slice(&sb.to_le_bytes()),
            Op::Append { sb, bytes } => {
                b.extend_from_slice(&sb.to_le_bytes());
                b.extend_from_slice(bytes);
            }
            Op::Remove { sb, start_ns, end_ns } | Op::AppendWindow { sb, start_ns, end_ns } => {
                b.extend_from_slice(&sb.to_le_bytes());
                b.extend_from_slice(&start_ns.to_le_bytes());
                b.extend_from_slice(&end_ns.to_le_bytes());
            }
            Op::TimestampOffset { sb, ns } => {
                b.extend_from_slice(&sb.to_le_bytes());
                b.extend_from_slice(&ns.to_le_bytes());
            }
            Op::Mode { sb, sequence } => {
                b.extend_from_slice(&sb.to_le_bytes());
                b.push(*sequence as u8);
            }
            Op::Duration { ns } => b.extend_from_slice(&ns.to_le_bytes()),
            Op::EndOfStream { error } => b.push(*error),
            Op::Reopen | Op::Close => {}
        }
        out.push(self.tag());
        out.extend_from_slice(&(b.len() as u32).to_le_bytes());
        out.extend_from_slice(&b);
    }

    /// One record from the front of `buf`: (op, bytes consumed). `Ok(None)` when the record is not
    /// whole yet.
    pub fn decode(buf: &[u8]) -> Result<Option<(Op, usize)>, Error> {
        if buf.len() < 5 {
            return Ok(None);
        }
        let tag = buf[0];
        let len = u32::from_le_bytes([buf[1], buf[2], buf[3], buf[4]]) as usize;
        if buf.len() - 5 < len {
            return Ok(None);
        }
        let b = &buf[5..5 + len];
        let mut r = Reader::new(b);
        let op = match tag {
            1 => Op::AddSourceBuffer { mime: String::from_utf8_lossy(b).into_owned() },
            2 => Op::RemoveSourceBuffer { sb: le16(&mut r)? },
            3 => {
                let sb = le16(&mut r)?;
                Op::Append { sb, bytes: b[2..].to_vec() }
            }
            4 | 8 => {
                let sb = le16(&mut r)?;
                let (start_ns, end_ns) = (le64(&mut r)?, le64(&mut r)?);
                if tag == 4 { Op::Remove { sb, start_ns, end_ns } } else { Op::AppendWindow { sb, start_ns, end_ns } }
            }
            5 => Op::Abort { sb: le16(&mut r)? },
            6 => {
                let sb = le16(&mut r)?;
                Op::TimestampOffset { sb, ns: le64(&mut r)? }
            }
            7 => {
                let sb = le16(&mut r)?;
                Op::Mode { sb, sequence: r.u8()? != 0 }
            }
            9 => Op::Duration { ns: le64(&mut r)? },
            10 => Op::EndOfStream { error: r.u8()? },
            11 => Op::Reopen,
            12 => Op::Close,
            _ => return Err(Error::Invalid("unknown MediaSource journal record")),
        };
        Ok(Some((op, 5 + len)))
    }
}

fn le16(r: &mut Reader<'_>) -> Result<u16, Error> {
    r.u16().map(u16::swap_bytes)
}

fn le64(r: &mut Reader<'_>) -> Result<i64, Error> {
    r.i64().map(i64::swap_bytes)
}

/// What applying one op did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Applied {
    pub append: AppendOutcome,
    /// The append failed (MSE "append error": the page runs endOfStream("decode")).
    pub error: Option<Error>,
}

/// A MediaSource and its SourceBuffers.
#[derive(Debug, Clone)]
pub struct MediaSourceModel {
    /// Removed buffers stay as `None` so indices are stable.
    pub buffers: Vec<Option<SourceBufferModel>>,
    pub ready: ReadyState,
    /// `duration`: None = NaN, `i64::MAX` = +Infinity.
    pub duration_ns: Option<i64>,
    /// `endOfStream(error)` was called with an error.
    pub error: u8,
    pub generation: u64,
}

impl Default for MediaSourceModel {
    fn default() -> Self {
        MediaSourceModel { buffers: Vec::new(), ready: ReadyState::Open, duration_ns: None, error: 0, generation: 0 }
    }
}

impl MediaSourceModel {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn buffer(&self, sb: u16) -> Option<&SourceBufferModel> {
        self.buffers.get(sb as usize).and_then(|b| b.as_ref())
    }

    fn buffer_mut(&mut self, sb: u16) -> Option<&mut SourceBufferModel> {
        self.buffers.get_mut(sb as usize).and_then(|b| b.as_mut())
    }

    /// The live buffers.
    pub fn active(&self) -> impl Iterator<Item = (u16, &SourceBufferModel)> {
        self.buffers.iter().enumerate().filter_map(|(i, b)| b.as_ref().map(|b| (i as u16, b)))
    }

    /// Every live SourceBuffer has received its first initialization segment (the element's
    /// HAVE_METADATA, MSE §3.5.7 step 7).
    pub fn all_init(&self) -> bool {
        let mut n = 0;
        for (_, b) in self.active() {
            if !b.has_init() {
                return false;
            }
            n += 1;
        }
        n > 0
    }

    /// The highest end time over every buffer.
    pub fn highest_end(&self) -> Option<i64> {
        self.active().filter_map(|(_, b)| b.highest_end()).max()
    }

    /// `HTMLMediaElement.buffered` for this MediaSource (MSE §2.4.5): the intersection of the
    /// active buffers' ranges; once ended, each buffer's last range reaches the highest end time.
    pub fn buffered(&self) -> Vec<(i64, i64)> {
        let hi = self.highest_end().unwrap_or(0);
        let mut acc: Option<Vec<(i64, i64)>> = None;
        for (_, b) in self.active() {
            let mut r = b.buffered();
            if self.ready == ReadyState::Ended {
                if let Some(last) = r.last_mut() {
                    last.1 = last.1.max(hi);
                }
            }
            acc = Some(match acc {
                None => r,
                Some(a) => intersect(&a, &r),
            });
        }
        acc.unwrap_or_default()
    }

    /// Apply one journal record.
    pub fn apply(&mut self, op: &Op) -> Applied {
        let mut out = Applied::default();
        match op {
            Op::AddSourceBuffer { mime } => self.buffers.push(Some(SourceBufferModel::new(mime))),
            Op::RemoveSourceBuffer { sb } => {
                if let Some(b) = self.buffers.get_mut(*sb as usize) {
                    *b = None;
                }
            }
            Op::Append { sb, bytes } => {
                if let Some(b) = self.buffer_mut(*sb) {
                    match b.append(bytes) {
                        Ok(a) => {
                            if a.first_init && self.duration_ns.is_none() {
                                // MSE §3.5.7 step 1: duration from the init segment, else +Infinity
                                self.duration_ns = Some(a.init_duration_ns.map_or(i64::MAX, |d| d as i64));
                            }
                            out.append = a;
                        }
                        Err(e) => out.error = Some(e),
                    }
                }
                // §3.5.8 step 5: media data past the duration extends it
                if let (Some(hi), Some(d)) = (self.highest_end(), self.duration_ns) {
                    if hi > d {
                        self.duration_ns = Some(hi);
                    }
                }
            }
            Op::Remove { sb, start_ns, end_ns } => {
                let dur = self.duration_ns.unwrap_or(i64::MAX);
                if let Some(b) = self.buffer_mut(*sb) {
                    b.remove(*start_ns, *end_ns, dur);
                }
            }
            Op::Abort { sb } => {
                if let Some(b) = self.buffer_mut(*sb) {
                    b.abort();
                }
            }
            Op::TimestampOffset { sb, ns } => {
                if let Some(b) = self.buffer_mut(*sb) {
                    b.set_timestamp_offset(*ns);
                }
            }
            Op::Mode { sb, sequence } => {
                if let Some(b) = self.buffer_mut(*sb) {
                    b.set_mode(if *sequence { AppendMode::Sequence } else { AppendMode::Segments });
                }
            }
            Op::AppendWindow { sb, start_ns, end_ns } => {
                if let Some(b) = self.buffer_mut(*sb) {
                    b.append_window_start_ns = *start_ns;
                    b.append_window_end_ns = *end_ns;
                }
            }
            Op::Duration { ns } => {
                // duration change (§2.4.6): a shorter duration removes what lies past it
                let old = self.duration_ns;
                self.duration_ns = Some(*ns);
                if old.is_some_and(|o| *ns < o) {
                    for b in self.buffers.iter_mut().flatten() {
                        b.remove(*ns, i64::MAX, i64::MAX);
                    }
                }
            }
            Op::EndOfStream { error } => {
                self.ready = ReadyState::Ended;
                self.error = *error;
                for b in self.buffers.iter_mut().flatten() {
                    let _ = b.flush();
                }
                if *error == 0 {
                    // §2.4.7: duration becomes the highest end time
                    if let Some(hi) = self.highest_end() {
                        self.duration_ns = Some(hi);
                    }
                }
            }
            Op::Reopen => {
                self.ready = ReadyState::Open;
                self.error = 0;
            }
            Op::Close => self.ready = ReadyState::Closed,
        }
        self.generation += 1;
        out
    }

    /// Every buffered frame of every live buffer as one demuxer (decode order), track ids namespaced
    /// per buffer (`sb << 24 | id`). `None` until every buffer has its initialization segment.
    pub fn demuxer(&self) -> Option<Demuxer> {
        if !self.all_init() {
            return None;
        }
        let mut tracks: Vec<Track> = Vec::new();
        let mut frames: Vec<(usize, &CodedFrame)> = Vec::new();
        let mut format = Format::Mp4;
        for (sb, b) in self.active() {
            if let Some(f) = b.format {
                format = f;
            }
            for tb in &b.tracks {
                let mut t = tb.track.clone();
                t.id = ((sb as u32) << 24) | (t.id & 0x00FF_FFFF);
                let ti = tracks.len();
                tracks.push(t);
                for f in &tb.frames {
                    frames.push((ti, f));
                }
            }
        }
        let dur = self.duration_ns.filter(|d| *d != i64::MAX).map(|d| d.max(0) as u64);
        Some(Demuxer::from_frames(format, tracks, frames, dur))
    }
}
