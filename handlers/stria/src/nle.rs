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
//! * [`Sources`] is the schedule's [`SourceIndex`]: its keyframe answer is NLEPROBE's pure [`Demuxer::keyframe_before`]
//!   (the search `seek_track` shares; DEMUXWIRE, LEDGER SR213). A source opens by its own index when it has one
//!   ([`Demuxer::open_indexed`]) and a Matroska video track gets decode times from its codec's reorder depth
//!   ([`Demuxer::derive_dts`] through `dsp::video::reorder_depth`); [`Sources::open_report`] says how each reel opened.
//! * Source time 0 is a stream's first presentation time (vplay's rule).
//! * A media file is its own timeline (EDITMAP, LEDGER SR223): [`Sources::from_media`] lays each stream's whole edit
//!   list on its track through `timeline_core`'s `Timeline::lay_edit_list` (each segment a clip, an empty edit a gap),
//!   so an edit-listed MP4 renders as its author cut it; [`OpenReport`] says `applied=<n>` (else `first-only`).
//! * Transitions (NLE3, LEDGER SR202): a step carrying `vmix` decodes a second run (the outgoing source, which takes
//!   over the live run it continues) through the same open-or-continue path ([`vid_frame`]) and mixes the two: a
//!   dissolve is pixel_core's `blend_nonpremult` of the incoming at alpha `round(255 t)` over the outgoing, a wipe a
//!   column (`W001`) or row (`W002`) split at `round(w t)` / `round(h t)` with the incoming before the edge. NLE3c
//!   (SR209): `W041` a diagonal from the top-left (`x + y < t (w + h)`), `W101` a centred box growing with `t`, `W201`
//!   a clock hand from twelve sweeping `2 pi t` (exact fractions, the clock by the pixel centre's angle); a transition
//!   from black mixes the incoming over opaque black (the gap's black) the same way.
//! * Video is cut-only and one size: a frame whose size differs from the project's (the first scheduled video source's
//!   track) is refused (scaling is owed). A gap is black (opaque) or silence.
//! * Audio is a MIX (NLE4, LEDGER SR203): each audio step renders its `timeline_core::schedule::audio_runs` run: every
//!   layer decoded on its own track's decoder, resampled to the project rate when its rate differs
//!   (`audio_core::resample::Sinc`, NLE4B's windowed sinc; the project rate is [`Sources::set_audio_rate`]'s, else
//!   the first scheduled audio source's), scaled by its gain and its linear fade envelope, summed in integers and
//!   clamped to 16 bits once. Only a CHANNEL-count mismatch is refused. One layer at 0 dB with no fade passes through
//!   bit for bit. NLE3b (SR209): a
//!   crossfade's UNDER layer (the outgoing's handle, before the incoming layer of the same track) decodes on its own
//!   slot (`tracks + track`), so the track's own decoder carries the incoming on; the sum is the same `mix_into`.
//!
//! [`synth_source`] writes the codec-free test medium the KATs and `tools/nle --make-src` use: demux_core's `utp1`
//! test-pattern video (frame ordinals from `base`) plus 16-bit PCM whose every sample names its source and index.

use std::collections::VecDeque;
use std::sync::Arc;

use gneiss_pal::dsp::audio::resample;
use gneiss_pal::dsp::audio_track::{AudioTrackDecoder, Timing, audio_decoder_for};
use gneiss_pal::dsp::demux::{DecodeTime, Demuxer, TrackKind, build};
use gneiss_pal::dsp::video::{Frame, VideoDecoder, decoder_for, reorder_depth};
use timeline_core::schedule::{AudioRun, Layer, ScheduleError, audio_runs, schedule};
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
    /// An audio stream's channel count differs from the project's (NLE4: a rate is resampled).
    Format { src: SourceId, rate: u32, channels: u16 },
    /// The media ran out before the clip did.
    Short(SourceId),
    /// Video steps exist but no scheduled source has a video stream to size the frame.
    NoSize,
    /// EDITMAP (SR223): a media file's edit list could not be laid as a timeline (a rate edit, or a segment past the media).
    Edit(timeline_core::EditError),
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
            RenderError::Edit(e) => write!(f, "edit list: {e}"),
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
    /// DEMUXWIRE (SR213): the editor's open — by the file's own index when it has one (an index open that declares no
    /// duration opens whole: the schedule bounds clips by it), then decode times from the codec's reorder depth; an
    /// indexed reel loads its first span so the origin (and the audio `Timing`) read real samples.
    fn open(bytes: Vec<u8>) -> Result<Media, RenderError> {
        let err = |e: gneiss_pal::dsp::demux::Error| RenderError::Demux(e.to_string());
        let mut demux = Demuxer::open_indexed(bytes).map_err(err)?;
        if demux.indexed() && demux.duration_ns() == 0 {
            let data = demux.bytes();
            drop(demux);
            demux = Demuxer::open(Arc::try_unwrap(data).unwrap_or_else(|a| (*a).clone())).map_err(err)?;
        }
        demux.derive_dts(reorder_depth);
        let find = |k: TrackKind| demux.tracks().iter().position(|t| t.kind == k);
        let (video, audio) = (find(TrackKind::Video), find(TrackKind::Audio));
        if demux.indexed() {
            if let Some(i) = video.or(audio) {
                demux.load_at(i, i64::MIN).map_err(err)?;
            }
        }
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
    /// NLE4: the project's audio rate (0 = the first scheduled audio source's).
    audio_rate: u32,
    /// EDITMAP (SR223): the sources laid from their own edit list ([`Sources::from_media`]) and how many clips it gave.
    applied: Vec<(SourceId, usize)>,
}

impl Sources {
    pub fn new(tl: &Timeline) -> Sources {
        Sources { tb: tl.tb, media: Vec::new(), audio_rate: 0, applied: Vec::new() }
    }
    /// NLE4: render audio at `hz` (sources at other rates are resampled); 0 = the first scheduled audio source's rate.
    pub fn set_audio_rate(&mut self, hz: u32) {
        self.audio_rate = hz;
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
    /// DEMUXWIRE (SR213): how source `id` opened — its video stream's (else its audio stream's) survey facts.
    pub fn open_report(&self, id: SourceId) -> Option<OpenReport> {
        let m = self.get(id).ok()?;
        let i = m.video.or(m.audio)?;
        let s = m.demux.survey().streams.into_iter().nth(i)?;
        let applied = self.applied.iter().find(|a| a.0 == id).map(|a| a.1);
        Some(OpenReport { indexed: s.indexed, decode_time: s.decode_time, edits: s.edits.len(), applied })
    }
}

/// DEMUXWIRE (SR213): one reel's open, as `nle schedule` prints it:
/// `open=<full|indexed> dts=<stored|derived:n|presentation> edits=<n> applied=<n|first-only>`. `edits` counts every `elst`
/// entry; EDITMAP (SR223): `applied` is the clips a reel opened by [`Sources::from_media`] laid from its whole edit list,
/// `first-only` a reel an EDL cuts (only the first media edit applies, as playback does).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenReport {
    pub indexed: bool,
    pub decode_time: DecodeTime,
    pub edits: usize,
    /// EDITMAP (SR223): `Some(clips)` when the reel's whole edit list was laid; `None` = the first media edit only.
    pub applied: Option<usize>,
}

impl std::fmt::Display for OpenReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let open = if self.indexed { "indexed" } else { "full" };
        write!(f, "open={open} dts=")?;
        match self.decode_time {
            DecodeTime::Stored => write!(f, "stored")?,
            DecodeTime::Derived(d) => write!(f, "derived:{d}")?,
            DecodeTime::Presentation => write!(f, "presentation")?,
        }
        write!(f, " edits={}", self.edits)?;
        match self.applied {
            Some(n) => write!(f, " applied={n}"),
            None => write!(f, " applied=first-only"),
        }
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
        // DEMUXWIRE (SR213): NLEPROBE's keyframe-before (the search seek_track shares); an indexed reel's window may not
        // hold t, so the span holding it is read into a share first.
        let t = origin + t_ns;
        let key = if m.demux.indexed() {
            let mut c = m.demux.share();
            c.load_at(i, t).ok()?;
            c.keyframe_before(i, t)
        } else {
            m.demux.keyframe_before(i, t)
        };
        key.map(|(_, p)| p - origin)
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
    /// NLE4B: the windowed-sinc converter (its table) for the last output rate asked of `take_at`.
    rs: Option<resample::Sinc>,
}

impl Aud {
    /// NLE4: refuses only a channel-count mismatch; the run keeps the decoder's own rate (`rate`).
    fn open(m: &Media, src: SourceId, key_ns: i64, channels: u16) -> Result<Aud, RenderError> {
        let (i, origin) = m.stream(Kind::Audio).ok_or(RenderError::NoStream(src, Kind::Audio))?;
        let track = &m.demux.tracks()[i];
        let dec = audio_decoder_for(track, Timing::of(&m.demux, track)).map_err(|e| RenderError::Decode(e.to_string()))?;
        if dec.channels() != channels {
            return Err(RenderError::Format { src, rate: dec.sample_rate(), channels: dec.channels() });
        }
        let rate = dec.sample_rate();
        let mut cursor = m.demux.share();
        cursor.seek_track(i, origin + key_ns);
        let ch = channels.max(1) as usize;
        Ok(Aud { src, origin, track_id: track.id, rate, ch, cursor, dec, buf: Vec::new(), buf_at: 0, started: false, done: false, rs: None })
    }
    /// Source samples `start..start + n`, interleaved (sample-exact; a hole in the stream is silence).
    fn take(&mut self, start: i64, n: i64) -> Result<Vec<f32>, RenderError> {
        self.fill(start + n, false)?;
        Ok(self.drain(start, n))
    }
    /// Decode until source sample `want` is buffered; at the stream's end `Short`, or (`soft`) whatever there is.
    fn fill(&mut self, want: i64, soft: bool) -> Result<(), RenderError> {
        let ch = self.ch;
        loop {
            let have_end = self.buf_at + (self.buf.len() / ch) as i64;
            if self.started && have_end >= want {
                break;
            }
            if self.done {
                return if soft { Ok(()) } else { Err(RenderError::Short(self.src)) };
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
        Ok(())
    }
    /// Move the buffer's head to source sample `start` (a hole before the buffer is silence).
    fn seat(&mut self, start: i64) {
        let ch = self.ch;
        if start >= self.buf_at {
            let drop = ((start - self.buf_at) as usize * ch).min(self.buf.len());
            self.buf.drain(..drop);
        } else {
            let pad = (self.buf_at - start) as usize * ch;
            self.buf.splice(0..0, std::iter::repeat_n(0.0, pad));
        }
        self.buf_at = start;
    }
    /// The buffered source samples `start..start + n`, drained.
    fn drain(&mut self, start: i64, n: i64) -> Vec<f32> {
        let ch = self.ch;
        self.seat(start);
        let take = (n.max(0) as usize * ch).min(self.buf.len());
        let out: Vec<f32> = self.buf.drain(..take).collect();
        self.buf_at += n.max(0);
        out
    }
}

/// Render output ticks `range` of `tl` (see the module docs).
pub fn render(tl: &Timeline, range: std::ops::Range<i64>, sources: &Sources, sink: &mut dyn RenderSink) -> Result<RenderReport, RenderError> {
    let steps = schedule(tl, range.clone(), sources).map_err(RenderError::Schedule)?;
    let runs = audio_runs(tl, range, sources).map_err(RenderError::Schedule)?;
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
    if sources.audio_rate > 0 {
        afmt = afmt.map(|(_, ch)| (sources.audio_rate, ch));
    }
    if let Some((rate, ch)) = afmt {
        (r.rate, r.channels) = (rate, ch);
    }
    let (w, h) = (r.width, r.height);
    let black: Vec<u8> = [0u8, 0, 0, 255].repeat(w as usize * h as usize);
    let tol = tl.tb.to_ns(1) / 2;
    let mut vmix_run: Option<Vid> = None; // NLE3: a transition's outgoing run
    let mut vid: Option<Vid> = None;
    // NLE4: one audio decoder per track; the k-th audio step is the k-th mix run.
    let mut auds: Vec<Option<Aud>> = (0..2 * tl.tracks.len()).map(|_| None).collect(); // NLE3b: + the under slots
    let mut run_of = runs.iter();
    for s in &steps {
        match (s.kind, s.src) {
            (Kind::Video, None) => {
                vmix_run = None;
                sink.frame(s.out_t, w, h, &black);
                r.frames += 1;
            }
            (Kind::Video, Some(src)) => {
                vmix_take(s, &mut vid, &mut vmix_run);
                let f = vid_frame(tl, sources, &mut vid, src, s.src_t, s.seek, tol, &mut r)?;
                if f.width != w || f.height != h {
                    return Err(RenderError::Size { src, width: f.width, height: f.height });
                }
                sink.frame(s.out_t, w, h, &vmix_frame(tl, sources, s, &mut vmix_run, &f, tol, &mut r)?);
                r.frames += 1;
            }
            (Kind::Audio, _) => {
                let run = run_of.next().expect("one mix run per audio step");
                let Some((rate, ch)) = afmt else { continue };
                let s0 = samples(tl.tb.to_ns(s.out_t), rate);
                let n = samples(tl.tb.to_ns(s.out_t + s.len), rate) - s0;
                let pcm = mix_run(tl, run, sources, &mut auds, rate, ch, n, &mut r.seeks)?;
                sink.pcm(s.out_t, rate, ch, &pcm);
                r.pcm_frames += n as u64;
            }
        }
    }
    Ok(r)
}

impl Aud {
    /// NLE4: output samples `out_start..out_start + n` at `out_rate` (counted from the source's origin): the source's
    /// own samples when the rates agree, else NLE4B's windowed sinc (`resample::Sinc`, its table kept in `rs`; the window's
    /// neighbours past the media's end hold).
    fn take_at(&mut self, out_start: i64, n: i64, out_rate: u32) -> Result<Vec<f32>, RenderError> {
        if self.rate == out_rate {
            return self.take(out_start, n);
        }
        if n <= 0 {
            return Ok(Vec::new());
        }
        if self.rs.as_ref().map(|s| s.rates()) != Some((self.rate, out_rate)) {
            self.rs = Some(resample::Sinc::new(self.rate, out_rate, self.ch));
        }
        let h = self.rs.as_ref().expect("built above").delay() as i64;
        let (i0, _, _) = resample::linear_pos(out_start, self.rate, out_rate);
        let (i1, _, _) = resample::linear_pos(out_start + n - 1, self.rate, out_rate);
        self.fill(i1 + 1, false)?;
        self.fill(i1 + h + 1, true)?;
        self.seat(i0 - h + 1);
        Ok(self.rs.as_ref().expect("built above").at(&self.buf, self.buf_at, out_start, n as usize))
    }
}

/// NLE4: one mix run's `n` output samples at the project format: each layer on its track's decoder (opened at its
/// `seek`, or when the track's source changed), x gain x envelope, summed in i64 and clamped to i16 once.
#[allow(clippy::too_many_arguments)]
fn mix_run(
    tl: &Timeline,
    run: &AudioRun,
    sources: &Sources,
    auds: &mut [Option<Aud>],
    rate: u32,
    ch: u16,
    n: i64,
    seeks: &mut u64,
) -> Result<Vec<f32>, RenderError> {
    let n = n.max(0);
    let mut acc = vec![0i64; n as usize * ch as usize];
    // NLE3b: a layer followed by another of its own track is a crossfade's UNDER layer: its own slot; a track with no
    // under layer in this run drops its under slot.
    let tracks = auds.len() / 2;
    let under = |li: usize| run.layers.get(li + 1).is_some_and(|m| m.track == run.layers[li].track);
    for t in 0..tracks {
        if !(0..run.layers.len()).any(|li| under(li) && run.layers[li].track == t) {
            auds[tracks + t] = None;
        }
    }
    for (li, l) in run.layers.iter().enumerate() {
        let slot = &mut auds[if under(li) { tracks + l.track } else { l.track }];
        if l.seek.is_some() || slot.as_ref().map(|a| a.src) != Some(l.src) {
            let key = match l.seek {
                Some(k) => k,
                None => sources.keyframe_ns(l.src, Kind::Audio, tl.tb.to_ns(l.src_t)).ok_or(RenderError::NoStream(l.src, Kind::Audio))?,
            };
            *slot = Some(Aud::open(sources.get(l.src)?, l.src, key, ch)?);
            *seeks += 1;
        }
        let start = samples(tl.tb.to_ns(l.src_t), rate);
        let pcm = slot.as_mut().expect("opened above").take_at(start, n, rate)?;
        mix_into(&mut acc, &pcm, l, n, ch.max(1) as usize);
    }
    Ok(acc.into_iter().map(|a| a.clamp(-32768, 32767) as f32 / 32768.0).collect())
}

/// NLE4: add one layer into the accumulator: each sample as i16, x its gain (Q16) x its envelope (Q16, linear from
/// `env[0]` at the run's first sample toward `env[1]` at its end edge).
fn mix_into(acc: &mut [i64], pcm: &[f32], l: &Layer, n: i64, ch: usize) {
    let g = gain_q16(l.gain_cdb);
    let (e0, e1) = (l.env[0] as i64, l.env[1] as i64);
    for (j, frame) in pcm.chunks(ch).enumerate() {
        let e = if n > 0 { e0 + (e1 - e0) * j as i64 / n } else { e0 };
        for (c, &x) in frame.iter().enumerate() {
            acc[j * ch + c] += (to_i16(x) * g * e) >> 32;
        }
    }
}

/// NLE4: a gain in centi-dB as a Q16 factor (0 cdB = 65536 exactly).
fn gain_q16(cdb: i32) -> i64 {
    (10f64.powf(cdb as f64 / 2000.0) * 65536.0).round() as i64
}

/// NLE4: an `f32` sample (full scale +-1) as a 16-bit value, rounded and clamped.
fn to_i16(x: f32) -> i64 {
    (x as f64 * 32768.0).round().clamp(-32768.0, 32767.0) as i64
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
    synth_source_with(base, frames, fps, &build::Mp4Extras::default())
}

/// EDITMAP (SR223): [`synth_source`] written with DEMUXSEEK's [`build::Mp4Extras`] (whole edit lists: track 1 is the video,
/// track 2 the audio).
pub fn synth_source_with(base: u32, frames: u32, fps: u32, x: &build::Mp4Extras) -> Vec<u8> {
    let fps = fps.max(1);
    let mut video = build::test_pattern_track(1, 64, 48, fps, frames, 10);
    for (i, s) in video.samples.iter_mut().enumerate() {
        s.data = build::TestPatternPacket { frame: base + i as u32, width: 64, height: 48 }.encode();
    }
    let n = frames as u64 * SYNTH_RATE as u64 / fps as u64;
    let pcm: Vec<i16> = (0..n).map(|i| synth_sample(base, i)).collect();
    let audio = build::pcm16_track(2, SYNTH_RATE, 1, &pcm, (SYNTH_RATE / fps / 2).max(1) as usize);
    build::mp4_with(&[video, audio], &build::Mp4Options::default(), x)
}

/// The frame at source tick `src_t` of `src` from the decode run `run`: a step with `seek`, or a run on another
/// source (or none), opens a fresh run at the keyframe (the run-up is decoded unseen); otherwise the run decodes on.
/// Shared by a step's own source and a transition's outgoing source (NLE3).
#[allow(clippy::too_many_arguments)]
fn vid_frame(
    tl: &Timeline,
    sources: &Sources,
    run: &mut Option<Vid>,
    src: SourceId,
    src_t: i64,
    seek: Option<i64>,
    tol: i64,
    r: &mut RenderReport,
) -> Result<Frame, RenderError> {
    let target = tl.tb.to_ns(src_t);
    if seek.is_some() || run.as_ref().map(|v| v.src) != Some(src) {
        let key = match seek {
            Some(k) => k,
            None => sources.keyframe_ns(src, Kind::Video, target).ok_or(RenderError::NoStream(src, Kind::Video))?,
        };
        *run = Some(Vid::open(sources.get(src)?, src, key)?);
        r.seeks += 1;
    }
    run.as_mut().expect("opened above").show(target, tol, &mut r.runup_frames)
}

/// NLE3: outside a transition the outgoing run is dropped; at a transition tick whose outgoing source continues the
/// live run (`seek` none) and is not already the mix run, that run becomes the mix run (the step's own source then
/// opens its run at the cut, as a plain cut does).
fn vmix_take(s: &timeline_core::schedule::Step, vid: &mut Option<Vid>, vmix_run: &mut Option<Vid>) {
    let Some(m) = s.vmix else {
        *vmix_run = None;
        return;
    };
    // NLE3c: a transition from black (`src` none) has no run to take.
    if m.src.is_some() && m.seek.is_none() && vmix_run.as_ref().map(|v| v.src) != m.src && vid.as_ref().map(|v| v.src) == m.src {
        *vmix_run = vid.take();
    }
}

/// NLE3: the RGBA a video step shows: its own frame `f`, or inside a transition `f` mixed with the outgoing source's
/// frame from `vmix_run`.
#[allow(clippy::too_many_arguments)]
fn vmix_frame(
    tl: &Timeline,
    sources: &Sources,
    s: &timeline_core::schedule::Step,
    vmix_run: &mut Option<Vid>,
    f: &Frame,
    tol: i64,
    r: &mut RenderReport,
) -> Result<Vec<u8>, RenderError> {
    let Some(m) = s.vmix else { return Ok(f.to_rgba()) };
    let a = match m.src {
        Some(src) => {
            let a = vid_frame(tl, sources, vmix_run, src, m.src_t, m.seek, tol, r)?;
            if a.width != f.width || a.height != f.height {
                return Err(RenderError::Size { src, width: a.width, height: a.height });
            }
            a.to_rgba()
        }
        // NLE3c: from black: the outgoing is opaque black (the gap's black).
        None => [0u8, 0, 0, 255].repeat(f.width as usize * f.height as usize),
    };
    Ok(vmix_rgba(m.kind, m.t, &a, &f.to_rgba(), f.width, f.height))
}

/// NLE3: the outgoing frame `a` and the incoming frame `b` (RGBA, `w x h`) mixed at the incoming's weight `t`.
fn vmix_rgba(kind: timeline_core::TransitionKind, t: timeline_core::schedule::Frac, a: &[u8], b: &[u8], w: u32, h: u32) -> Vec<u8> {
    use timeline_core::{TransitionKind, WipeDir};
    let den = t.den.max(1);
    let round = |n: i64| (2 * n * t.num + den) / (2 * den);
    let mut out = a.to_vec();
    match kind {
        TransitionKind::Dissolve => {
            let alpha = round(255).clamp(0, 255) as u8;
            for (o, p) in out.chunks_exact_mut(4).zip(b.chunks_exact(4)) {
                let px = gneiss_pal::dsp::image::blend_nonpremult([p[0], p[1], p[2], alpha], [o[0], o[1], o[2], o[3]]);
                o.copy_from_slice(&px);
            }
        }
        TransitionKind::Wipe(dir) => {
            let (w, h) = (w as usize, h as usize);
            let (wi, hi, num) = (w as i64, h as i64, t.num);
            for y in 0..h {
                for x in 0..w {
                    let (xi, yi) = (x as i64, y as i64);
                    let incoming = match dir {
                        WipeDir::LeftToRight => xi < round(wi),
                        WipeDir::TopToBottom => yi < round(hi),
                        // NLE3c: exact fractions of t = num / den.
                        WipeDir::Diagonal => (xi + yi) * den < (wi + hi) * num,
                        WipeDir::Box => (2 * xi + 1 - wi).abs() * den < wi * num && (2 * yi + 1 - hi).abs() * den < hi * num,
                        WipeDir::Clock => clock_angle(x, y, w, h) < std::f64::consts::TAU * num as f64 / den as f64,
                    };
                    if incoming {
                        let i = (y * w + x) * 4;
                        out[i..i + 4].copy_from_slice(&b[i..i + 4]);
                    }
                }
            }
        }
    }
    out
}

/// NLE3c: the angle of pixel `(x, y)`'s centre about the frame's centre, clockwise from twelve o'clock, in `[0, 2 pi)`.
fn clock_angle(x: usize, y: usize, w: usize, h: usize) -> f64 {
    let (dx, dy) = (x as f64 + 0.5 - w as f64 / 2.0, y as f64 + 0.5 - h as f64 / 2.0);
    dx.atan2(-dy).rem_euclid(std::f64::consts::TAU)
}

/// EDITMAP (LEDGER SR223): a media file as its own timeline.
impl Sources {
    /// Open `bytes` as the one source of a new timeline at `fps` (reel `reel`) and lay each stream's whole edit list on
    /// its own track (video V1, audio A1) through `Timeline::lay_edit_list` (source time 0 = the stream's first pts, the
    /// origin [`Media::open`] keeps; no edit list = the whole source). `Ok(None)` when the bytes are no container
    /// demux_core knows (`probe`): the caller reads them as an EDL.
    pub fn from_media(reel: &str, bytes: &[u8], fps: u32) -> Result<Option<(Timeline, Sources)>, RenderError> {
        if gneiss_pal::dsp::demux::probe(bytes).is_none() {
            return Ok(None);
        }
        let mut tl = Timeline::new(fps);
        let mut sources = Sources::new(&tl);
        let id = SourceId(0);
        sources.add(id, bytes.to_vec())?;
        let src = tl.add_source(reel, sources.duration(id).unwrap_or(0));
        let m = sources.get(id)?;
        let mut applied = None;
        for kind in [Kind::Video, Kind::Audio] {
            let Some((i, origin)) = m.stream(kind) else { continue };
            let track = tl.add_track(kind);
            let tb = m.demux.tracks()[i].timebase;
            tl = tl.lay_edit_list(track, src, m.demux.edit_list(i), tb, origin).map_err(RenderError::Edit)?;
            applied.get_or_insert(tl.tracks[track].clips.len());
        }
        sources.applied.push((src, applied.unwrap_or(0)));
        Ok(Some((tl, sources)))
    }
}
