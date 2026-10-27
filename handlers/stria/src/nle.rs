// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Stria's editor render (NLE1 M3, LEDGER SR194; CODEX Amendment II: Stria owns A/V).
//!
//! CHARTER: Stria — the render half of the non-linear editor, over `timeline_core` (shared-core, BP-handlers-4).
//!
//! [`render`] turns a [`Timeline`] range into raw output — RGBA frames and interleaved `f32` PCM — handed to a
//! [`RenderSink`] in output order. It is an offline pull with no clock: it walks `timeline_core::schedule::schedule`
//! and drives the SAME dsp registry the realtime [`crate::media::Player`] plays through (`dsp::video::decoder_for`,
//! `dsp::audio_track::audio_decoder_for` + `Timing::of`); it holds no decoder, demuxer or sample conversion of its own.
//!
//! * A step with `seek` opens a fresh cursor on the source ([`Demuxer::share`], no byte copy), lands it on the keyframe
//!   ([`Demuxer::seek_track`]) and decodes forward, discarding frames before the step's source time unseen — the run-up,
//!   the shape vplay and the Player seek with. A continuing step decodes on.
//! * [`Sources`] is the schedule's [`SourceIndex`]: its keyframe answer is the demuxer's own search
//!   (`share()` + `seek_track` on a throwaway cursor). NLEPROBE (SR195) owes the pure keyframe-before; the swap is one fn.
//! * Source time 0 is a stream's first presentation time (vplay's rule).
//! * Cut-only and one format: a video frame whose size differs from the project's (the first scheduled video source's
//!   track) is refused (scaling is owed), as is an audio stream whose rate or channel count differs (the mix and the
//!   resampler are NLE4). A gap is black (opaque) or silence.
//!
//! [`synth_source`] writes the codec-free test medium the KATs and `tools/nle --make-src` use: demux_core's `utp1`
//! test-pattern video (frame ordinals from `base`) plus 16-bit PCM whose every sample names its source and index.

use std::collections::VecDeque;

use gneiss_pal::dsp::audio_track::{AudioTrackDecoder, Timing, audio_decoder_for};
use gneiss_pal::dsp::demux::{Demuxer, TrackKind, build};
use gneiss_pal::dsp::video::{Frame, VideoDecoder, decoder_for};
use timeline_core::schedule::{ScheduleError, schedule};
use timeline_core::{Kind, SourceId, SourceIndex, Timebase, Timeline};

/// Where rendered output goes, in output order.
pub trait RenderSink {
    /// One output frame at output tick `out_t`: `width * height * 4` RGBA bytes.
    fn frame(&mut self, out_t: i64, width: u32, height: u32, rgba: &[u8]);
    /// The PCM of output ticks `out_t..` (one audio step): interleaved `f32`.
    fn pcm(&mut self, out_t: i64, rate: u32, channels: u16, interleaved: &[f32]);
}

/// What a render did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RenderReport {
    pub frames: u64,
    pub pcm_frames: u64,
    /// Decoder (re)starts at a keyframe, video and audio.
    pub seeks: u64,
    /// Video frames decoded and not shown (the run-up from a keyframe, or frames a faster source skips).
    pub runup_frames: u64,
    pub width: u32,
    pub height: u32,
    pub rate: u32,
    pub channels: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RenderError {
    Schedule(ScheduleError),
    /// The timeline cites a source no media was added for.
    NoSource(SourceId),
    /// The source has no stream of the kind its track needs.
    NoStream(SourceId, Kind),
    Demux(String),
    Decode(String),
    /// A frame's size differs from the project's.
    Size { src: SourceId, width: u32, height: u32 },
    /// An audio stream's rate or channel count differs from the project's.
    Format { src: SourceId, rate: u32, channels: u16 },
    /// The media ran out before the clip did.
    Short(SourceId),
    /// Video steps exist but no scheduled source has a video stream to size the frame.
    NoSize,
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::Schedule(e) => write!(f, "schedule: {e}"),
            RenderError::NoSource(s) => write!(f, "no media for source {}", s.0),
            RenderError::NoStream(s, k) => write!(f, "source {} has no {k:?} stream", s.0),
            RenderError::Demux(e) => write!(f, "demux: {e}"),
            RenderError::Decode(e) => write!(f, "decode: {e}"),
            RenderError::Size { src, width, height } => write!(f, "source {} frame {width}x{height} is not the project size", src.0),
            RenderError::Format { src, rate, channels } => write!(f, "source {} audio {rate} Hz x{channels} is not the project format", src.0),
            RenderError::Short(s) => write!(f, "source {} ran out before its clip", s.0),
            RenderError::NoSize => f.write_str("no video source to size the frame"),
        }
    }
}
impl std::error::Error for RenderError {}

/// One source's media: the demuxer (its bytes shared by every cursor) and each stream's origin.
struct Media {
    demux: Demuxer,
    video: Option<usize>,
    audio: Option<usize>,
    v0: i64,
    a0: i64,
}

impl Media {
    fn open(bytes: Vec<u8>) -> Result<Media, RenderError> {
        let demux = Demuxer::open(bytes).map_err(|e| RenderError::Demux(e.to_string()))?;
        let find = |k: TrackKind| demux.tracks().iter().position(|t| t.kind == k);
        let (video, audio) = (find(TrackKind::Video), find(TrackKind::Audio));
        let origin = |i: Option<usize>| {
            i.map(|i| {
                let tb = demux.tracks()[i].timebase;
                demux.track_samples(i).map(|s| tb.to_ns(s.pts)).min().unwrap_or(0)
            })
            .unwrap_or(0)
        };
        let (v0, a0) = (origin(video), origin(audio));
        Ok(Media { demux, video, audio, v0, a0 })
    }
    fn stream(&self, kind: Kind) -> Option<(usize, i64)> {
        match kind {
            Kind::Video => self.video.map(|i| (i, self.v0)),
            Kind::Audio => self.audio.map(|i| (i, self.a0)),
        }
    }
}

/// The media behind a timeline's sources; the schedule's [`SourceIndex`].
pub struct Sources {
    tb: Timebase,
    media: Vec<(SourceId, Media)>,
}

impl Sources {
    pub fn new(tl: &Timeline) -> Sources {
        Sources { tb: tl.tb, media: Vec::new() }
    }
    /// Open `bytes` (MP4 / Matroska / WebM) as source `id`.
    pub fn add(&mut self, id: SourceId, bytes: Vec<u8>) -> Result<(), RenderError> {
        let m = Media::open(bytes)?;
        self.media.retain(|(s, _)| *s != id);
        self.media.push((id, m));
        Ok(())
    }
    fn get(&self, id: SourceId) -> Result<&Media, RenderError> {
        self.media.iter().find(|(s, _)| *s == id).map(|(_, m)| m).ok_or(RenderError::NoSource(id))
    }
}

impl SourceIndex for Sources {
    fn duration(&self, src: SourceId) -> Option<i64> {
        let m = self.get(src).ok()?;
        let ns = [m.video, m.audio].into_iter().flatten().map(|i| m.demux.tracks()[i].duration_ns).max()?;
        // whole ticks only (1 µs of slack for a time base's rounding)
        Some(((ns as i128 + 1_000) * self.tb.den as i128 / (self.tb.num.max(1) as i128 * 1_000_000_000)) as i64)
    }
    fn keyframe_ns(&self, src: SourceId, kind: Kind, t_ns: i64) -> Option<i64> {
        let m = self.get(src).ok()?;
        let (i, origin) = m.stream(kind)?;
        // NLEPROBE (SR195) owes a pure keyframe-before; until then the demuxer's own search on a throwaway cursor.
        let mut cursor = m.demux.share();
        cursor.seek_track(i, origin + t_ns).map(|k| k - origin)
    }
}

/// Nanoseconds to a sample index at `rate`, rounded.
fn samples(ns: i64, rate: u32) -> i64 {
    let n = ns as i128 * rate as i128;
    (if n >= 0 { (n + 500_000_000) / 1_000_000_000 } else { (n - 500_000_000) / 1_000_000_000 }) as i64
}

/// A video decode run from one keyframe.
struct Vid {
    src: SourceId,
    origin: i64,
    track_id: u32,
    cursor: Demuxer,
    dec: Box<dyn VideoDecoder>,
    pending: VecDeque<Frame>,
    peeked: Option<Frame>,
    last: Option<Frame>,
    done: bool,
}

impl Vid {
    fn open(m: &Media, src: SourceId, key_ns: i64) -> Result<Vid, RenderError> {
        let (i, origin) = m.stream(Kind::Video).ok_or(RenderError::NoStream(src, Kind::Video))?;
        let track = &m.demux.tracks()[i];
        let dec = decoder_for(track).map_err(|e| RenderError::Decode(e.to_string()))?;
        let mut cursor = m.demux.share();
        cursor.seek_track(i, origin + key_ns);
        Ok(Vid { src, origin, track_id: track.id, cursor, dec, pending: VecDeque::new(), peeked: None, last: None, done: false })
    }
    fn pull(&mut self) -> Result<Option<Frame>, RenderError> {
        loop {
            if let Some(f) = self.pending.pop_front() {
                return Ok(Some(f));
            }
            if self.done {
                return Ok(None);
            }
            match self.cursor.next_packet() {
                Some(p) if p.track == self.track_id => {
                    if let Some(f) = self.dec.decode(&p).map_err(|e| RenderError::Decode(e.to_string()))? {
                        return Ok(Some(f));
                    }
                }
                Some(_) => {}
                None => {
                    self.done = true;
                    self.pending.extend(self.dec.flush());
                }
            }
        }
    }
    /// The frame on screen at source time `target` (ns): the first frame within `tol` of it; frames before are
    /// decoded unseen; if the next frame is later (a source slower than the timeline) the last one holds.
    fn show(&mut self, target: i64, tol: i64, runup: &mut u64) -> Result<Frame, RenderError> {
        loop {
            if self.peeked.is_none() {
                self.peeked = self.pull()?;
            }
            let Some(f) = self.peeked.as_ref() else { return Err(RenderError::Short(self.src)) };
            let rel = f.pts_ns - self.origin;
            if rel < target - tol {
                *runup += 1;
                self.last = self.peeked.take();
                continue;
            }
            if rel <= target + tol {
                self.last = self.peeked.take();
                return Ok(self.last.clone().expect("just set"));
            }
            return Ok(self.last.clone().unwrap_or_else(|| f.clone()));
        }
    }
}

/// An audio decode run from one sync point; `buf[0]` is source sample `buf_at`.
struct Aud {
    src: SourceId,
    origin: i64,
    track_id: u32,
    rate: u32,
    ch: usize,
    cursor: Demuxer,
    dec: Box<dyn AudioTrackDecoder>,
    buf: Vec<f32>,
    buf_at: i64,
    started: bool,
    done: bool,
}

impl Aud {
    fn open(m: &Media, src: SourceId, key_ns: i64, rate: u32, channels: u16) -> Result<Aud, RenderError> {
        let (i, origin) = m.stream(Kind::Audio).ok_or(RenderError::NoStream(src, Kind::Audio))?;
        let track = &m.demux.tracks()[i];
        let dec = audio_decoder_for(track, Timing::of(&m.demux, track)).map_err(|e| RenderError::Decode(e.to_string()))?;
        if dec.sample_rate() != rate || dec.channels() != channels {
            return Err(RenderError::Format { src, rate: dec.sample_rate(), channels: dec.channels() });
        }
        let mut cursor = m.demux.share();
        cursor.seek_track(i, origin + key_ns);
        let ch = channels.max(1) as usize;
        Ok(Aud { src, origin, track_id: track.id, rate, ch, cursor, dec, buf: Vec::new(), buf_at: 0, started: false, done: false })
    }
    /// Source samples `start..start + n`, interleaved (sample-exact; a hole in the stream is silence).
    fn take(&mut self, start: i64, n: i64) -> Result<Vec<f32>, RenderError> {
        let ch = self.ch;
        loop {
            let have_end = self.buf_at + (self.buf.len() / ch) as i64;
            if self.started && have_end >= start + n {
                break;
            }
            if self.done {
                return Err(RenderError::Short(self.src));
            }
            match self.cursor.next_packet() {
                Some(p) if p.track == self.track_id => {
                    let b = self.dec.decode(&p).map_err(|e| RenderError::Decode(e.to_string()))?;
                    if b.samples.is_empty() {
                        continue;
                    }
                    let at = samples(b.pts_ns - self.origin, self.rate);
                    if !self.started {
                        self.started = true;
                        self.buf.clear();
                        self.buf_at = at;
                    }
                    let end = self.buf_at + (self.buf.len() / ch) as i64;
                    if at > end {
                        self.buf.resize(self.buf.len() + (at - end) as usize * ch, 0.0);
                    }
                    let skip = if at < end { (end - at) as usize * ch } else { 0 };
                    if skip < b.samples.len() {
                        self.buf.extend_from_slice(&b.samples[skip..]);
                    }
                }
                Some(_) => {}
                None => self.done = true,
            }
        }
        if start >= self.buf_at {
            let drop = ((start - self.buf_at) as usize * ch).min(self.buf.len());
            self.buf.drain(..drop);
        } else {
            let pad = (self.buf_at - start) as usize * ch;
            self.buf.splice(0..0, std::iter::repeat_n(0.0, pad));
        }
        self.buf_at = start;
        let take = (n.max(0) as usize * ch).min(self.buf.len());
        let out: Vec<f32> = self.buf.drain(..take).collect();
        self.buf_at += n.max(0);
        Ok(out)
    }
}

/// Render output ticks `range` of `tl` (see the module docs).
pub fn render(tl: &Timeline, range: std::ops::Range<i64>, sources: &Sources, sink: &mut dyn RenderSink) -> Result<RenderReport, RenderError> {
    let steps = schedule(tl, range, sources).map_err(RenderError::Schedule)?;
    let mut r = RenderReport::default();
    let mut afmt: Option<(u32, u16)> = None;
    for s in &steps {
        let Some(src) = s.src else { continue };
        let m = sources.get(src)?;
        match s.kind {
            Kind::Video if r.width == 0 => {
                if let Some(i) = m.video {
                    let t = &m.demux.tracks()[i];
                    (r.width, r.height) = (t.width, t.height);
                }
            }
            Kind::Audio if afmt.is_none() => {
                if let Some(i) = m.audio {
                    let t = &m.demux.tracks()[i];
                    afmt = Some((t.sample_rate, t.channels.max(1)));
                }
            }
            _ => {}
        }
    }
    if r.width == 0 && steps.iter().any(|s| s.kind == Kind::Video) {
        return Err(RenderError::NoSize);
    }
    if let Some((rate, ch)) = afmt {
        (r.rate, r.channels) = (rate, ch);
    }
    let (w, h) = (r.width, r.height);
    let black: Vec<u8> = [0u8, 0, 0, 255].repeat(w as usize * h as usize);
    let tol = tl.tb.to_ns(1) / 2;
    let mut vid: Option<Vid> = None;
    let mut aud: Option<Aud> = None;
    for s in &steps {
        match (s.kind, s.src) {
            (Kind::Video, None) => {
                sink.frame(s.out_t, w, h, &black);
                r.frames += 1;
            }
            (Kind::Video, Some(src)) => {
                let target = tl.tb.to_ns(s.src_t);
                if s.seek.is_some() || vid.as_ref().map(|v| v.src) != Some(src) {
                    let key = match s.seek {
                        Some(k) => k,
                        None => sources.keyframe_ns(src, Kind::Video, target).ok_or(RenderError::NoStream(src, Kind::Video))?,
                    };
                    vid = Some(Vid::open(sources.get(src)?, src, key)?);
                    r.seeks += 1;
                }
                let f = vid.as_mut().expect("opened above").show(target, tol, &mut r.runup_frames)?;
                if f.width != w || f.height != h {
                    return Err(RenderError::Size { src, width: f.width, height: f.height });
                }
                sink.frame(s.out_t, w, h, &f.to_rgba());
                r.frames += 1;
            }
            (Kind::Audio, src) => {
                let Some((rate, ch)) = afmt else { continue };
                let s0 = samples(tl.tb.to_ns(s.out_t), rate);
                let n = samples(tl.tb.to_ns(s.out_t + s.len), rate) - s0;
                let pcm = match src {
                    None => vec![0.0f32; n as usize * ch as usize],
                    Some(src) => {
                        let start = samples(tl.tb.to_ns(s.src_t), rate);
                        if s.seek.is_some() || aud.as_ref().map(|a| a.src) != Some(src) {
                            let key = match s.seek {
                                Some(k) => k,
                                None => sources
                                    .keyframe_ns(src, Kind::Audio, tl.tb.to_ns(s.src_t))
                                    .ok_or(RenderError::NoStream(src, Kind::Audio))?,
                            };
                            aud = Some(Aud::open(sources.get(src)?, src, key, rate, ch)?);
                            r.seeks += 1;
                        }
                        aud.as_mut().expect("opened above").take(start, n)?
                    }
                };
                sink.pcm(s.out_t, rate, ch, &pcm);
                r.pcm_frames += n as u64;
            }
        }
    }
    Ok(r)
}

/// The synthetic sources' audio rate.
pub const SYNTH_RATE: u32 = 48_000;

/// Sample `i` of the synthetic source whose frame ordinals start at `base`: `(base / 1000 mod 3) * 10000 + i mod 10000`
/// (so a sample names its source and its index).
pub fn synth_sample(base: u32, i: u64) -> i16 {
    ((base / 1000 % 3) as i32 * 10_000 + (i % 10_000) as i32) as i16
}

/// A codec-free test source (MP4): `frames` `utp1` test-pattern frames, 64x48 at `fps`, ordinals `base..`, a keyframe
/// every 10 frames; plus mono 16-bit PCM at [`SYNTH_RATE`] of [`synth_sample`]s, as long as the video.
pub fn synth_source(base: u32, frames: u32, fps: u32) -> Vec<u8> {
    let fps = fps.max(1);
    let mut video = build::test_pattern_track(1, 64, 48, fps, frames, 10);
    for (i, s) in video.samples.iter_mut().enumerate() {
        s.data = build::TestPatternPacket { frame: base + i as u32, width: 64, height: 48 }.encode();
    }
    let n = frames as u64 * SYNTH_RATE as u64 / fps as u64;
    let pcm: Vec<i16> = (0..n).map(|i| synth_sample(base, i)).collect();
    let audio = build::pcm16_track(2, SYNTH_RATE, 1, &pcm, (SYNTH_RATE / fps / 2).max(1) as usize);
    build::mp4(&[video, audio], &build::Mp4Options::default())
}
