// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Stria's media player (PLAYBACK M3, LEDGER SR26; AUDIOTRACK, SR45; CODEX Amendment II:
//! Stria owns A/V).
//!
//! [`Player`] is the whole pipeline for one stream:
//!
//! ```text
//!  bytes ─▶ dsp::demux::Demuxer ─▶ packets (decode order, tracks merged)
//!            ├─ video ─▶ dsp::video::VideoDecoder ─▶ frame queue (pts order)
//!            │                                      └▶ dsp::avsync::Scheduler ─▶ FrameSink (RGBA)
//!            └─ audio ─▶ dsp::audio_track (Opus/Vorbis/AAC/MP3/FLAC/PCM, gapless) ─┐
//!  bytes ─▶ dsp::audio::Decoder (bare MP3/Ogg/FLAC/WAV/AIFF/ADTS file) ─────────────┤
//!                       stereo f32 ─▶ AudioOut (resonance stereo StreamSource) ◀───┘
//!                                     └▶ consumed frames ─▶ AudioClock
//!                       peak L/R per 50 ms ─▶ FrameSink::levels (the meters)
//!  master clock = audio (when an audio track decodes and an output exists) else wall
//! ```
//!
//! A file with no video track is an **audio-only session**: no frames, the meters instead, and
//! the end comes when the clock passes the last sample. Every audio block is placed on the
//! media timeline sample-accurately — leading samples before the next expected time are dropped
//! (a seek decodes from the keyframe before the target), a gap is filled with silence — so the
//! audio clock and the presentation timeline never disagree. A seek also FLUSHES the device
//! ring (`StreamFeed::flush`), and a pause pauses it, so stale or paused audio is never heard.
//!
//! The player is driven by [`Player::tick`], one call per display refresh; it owns no thread,
//! no window and no device, so the same code runs under a tokio interval (the bus service in
//! `media_bus`), under a headless manual clock (`tools/play-check`), and in tests.

use std::collections::VecDeque;
use std::sync::Arc;

use gneiss_pal::dsp::audio::{self as ac, AudioDecoder};
use gneiss_pal::dsp::audio_track::{AudioTrackDecoder, Timing, audio_decoder_for};
use gneiss_pal::dsp::avsync::{Decision, DriftMeter, DriftReport, MasterClock, SchedStats, Scheduler, TimeSource};
use gneiss_pal::dsp::demux::{Demuxer, Packet, Track, TrackKind};
use gneiss_pal::dsp::video::{Frame, TestPattern, VideoDecoder, decoder_for};

/// Where presented frames (and the audio meters) go.
pub trait FrameSink {
    /// `ordinal` is the frame's index in presentation order since open (0-based; counted over
    /// every decoded frame, so drops leave gaps).
    fn present(&mut self, frame: &Frame, ordinal: u64);
    /// Audio meters that came due: per [`LEVEL_WINDOW_NS`] window from `pts_ns` on, the peak
    /// absolute sample of the left and right channel as played.
    fn levels(&mut self, _pts_ns: i64, _peaks: &[[f32; 2]]) {}
}

/// The meter window: 50 ms.
pub const LEVEL_WINDOW_NS: i64 = 50_000_000;

/// The audio device side the player feeds: interleaved STEREO `f32` at the stream's rate.
pub trait AudioOut: Send {
    /// Queue whole L/R frames; returns how many SAMPLES were taken.
    fn push(&mut self, stereo: &[f32]) -> usize;
    /// Source FRAMES the device has consumed since creation (monotonic).
    fn consumed(&self) -> u64;
    /// Drop everything queued and not yet played (a seek).
    fn flush(&mut self) {}
    /// Pause / resume the device side (silence, nothing consumed while paused).
    fn set_paused(&mut self, _paused: bool) {}
}

impl AudioOut for resonance::StreamFeed {
    fn push(&mut self, stereo: &[f32]) -> usize {
        resonance::StreamFeed::push(self, stereo)
    }
    fn consumed(&self) -> u64 {
        resonance::StreamFeed::consumed(self)
    }
    fn flush(&mut self) {
        resonance::StreamFeed::flush(self)
    }
    fn set_paused(&mut self, paused: bool) {
        resonance::StreamFeed::set_paused(self, paused)
    }
}

/// Open a resonance output that plays this stream's audio: a one-node graph (a stereo
/// StreamSource) on the default device. `None` when there is no device (headless hosts): the
/// caller then runs on the wall clock. The engine must be kept alive as long as the feed is used.
pub fn resonance_out(sample_rate: u32) -> Option<(resonance::AudioEngine, resonance::StreamFeed)> {
    let (src, feed) = resonance::stream_pair_channels(sample_rate, 2, sample_rate as usize / 2);
    let mut g = resonance::AudioGraph::new(sample_rate as f64);
    g.add_node(Box::new(src));
    match resonance::AudioEngine::new(g) {
        Ok((engine, _handle)) => Some((engine, feed)),
        Err(e) => {
            log::warn!("[STRIA] :: no audio device for media ({e}); wall clock");
            None
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaInfo {
    pub duration_ns: u64,
    pub width: u32,
    pub height: u32,
    /// Codec names as the container spells them ("" when absent).
    pub video: String,
    pub audio: String,
    /// False when frames are the labelled test-pattern stand-in.
    pub real_video: bool,
    /// The video decoder's name ("test-pattern", "test-pattern (stand-in)", ...; "" audio-only).
    pub video_decoder: String,
    /// The audio decoder's name ("opus", "vorbis", "aac-lc", "mp3", "flac", "pcm"; "" none).
    pub audio_decoder: String,
    /// Why audio is silent, when it is (no track / no decoder / no device).
    pub audio_note: String,
    pub audio_clock: bool,
    pub has_video: bool,
    /// The audio stream's rate and channel count as decoded (0 without decodable audio).
    pub sample_rate: u32,
    pub channels: u16,
    /// Where the audio's first presented sample lies after the stream's start (the clock's
    /// origin), in ns: > 0 when the audio track starts later than the video (the player fills
    /// that lead with silence so the device clock stays the media clock).
    pub audio_start_ns: i64,
}

#[derive(Debug)]
pub enum OpenError {
    Container(gneiss_pal::dsp::demux::Error),
    /// Neither a container with a video or decodable audio track, nor an audio file.
    NoMedia(String),
}
impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Container(e) => write!(f, "{e}"),
            OpenError::NoMedia(why) => write!(f, "no playable track: {why}"),
        }
    }
}
impl std::error::Error for OpenError {}

/// What one tick did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tick {
    /// Paused or waiting; nothing new on glass.
    Idle,
    /// A new frame went on glass (audio-only: new meter windows came due).
    Presented,
    /// The stream is over: no frames left and the clock is past the last one.
    Ended,
}

/// Frames decoded ahead of the clock.
const VIDEO_AHEAD: usize = 4;
/// Audio queued ahead in the player (beyond the device ring), in ns.
const AUDIO_AHEAD_NS: i64 = 500_000_000;
/// Audio decoded before a seek target in an audio-only stream (decoder pre-roll: Opus asks
/// 80 ms, a Vorbis/AAC/MP3 overlap is one block).
const AUDIO_PREROLL_NS: i64 = 100_000_000;
/// Frames per read from a bare audio file.
const FILE_CHUNK: usize = 4096;

/// Where the audio comes from.
enum AudioSrc {
    /// A container track: packets from the demuxer through the registry.
    Track { track: Track, dec: Box<dyn AudioTrackDecoder> },
    /// A bare audio file (MP3/Ogg/FLAC/WAV/AIFF/ADTS): `dsp::audio`'s file decoder, which applies
    /// that format's own gapless rules (LAME/Xing, Ogg granules). Forward-only: a seek reopens
    /// the file and decodes up to the target.
    File { bytes: Vec<u8>, dec: Box<ac::Decoder>, rate: u32, channels: u16, frames: u64, buf: Vec<f32> },
}

/// Interleaved `ch`-channel samples (WAVE order) → interleaved stereo. Mono is duplicated;
/// 3–8 channels take ITU-R BS.775's downmix (L = FL + ½√2·FC + ½√2·(BL|SL), likewise R; LFE
/// dropped), unnormalised as Chromium's downmix is.
pub fn to_stereo(x: &[f32], ch: usize) -> Vec<f32> {
    let ch = ch.max(1);
    let n = x.len() / ch;
    let mut o = Vec::with_capacity(n * 2);
    const K: f32 = std::f32::consts::FRAC_1_SQRT_2;
    for f in x.chunks_exact(ch) {
        let (l, r) = match ch {
            1 => (f[0], f[0]),
            2 => (f[0], f[1]),
            3 => (f[0] + K * f[2], f[1] + K * f[2]),
            4 => (f[0] + K * f[2], f[1] + K * f[3]),
            5 => (f[0] + K * f[2] + K * f[3], f[1] + K * f[2] + K * f[4]),
            _ => (f[0] + K * f[2] + K * f[4], f[1] + K * f[2] + K * f[5]),
        };
        o.push(l);
        o.push(r);
    }
    o
}

pub struct Player {
    demux: Option<Demuxer>,
    vtrack: Option<Track>,
    vdec: Option<Box<dyn VideoDecoder>>,
    audio: Option<AudioSrc>,
    aout: Option<Box<dyn AudioOut>>,
    /// Interleaved stereo waiting for the device, ending at `audio_next_ns`.
    pending: VecDeque<f32>,
    audio_rate: u32,
    /// The presentation time of the next queued sample = `audio_base_ns` + `audio_frames`/rate.
    audio_base_ns: i64,
    audio_frames: u64,
    /// The meter window being accumulated: (index, peak L, peak R), and the finished ones.
    level_acc: Option<(i64, f32, f32)>,
    levels: VecDeque<(i64, [f32; 2])>,
    audio_eof: bool,
    /// Muted: zeros go to the device in place of the samples (the clock keeps running).
    muted: bool,
    consumed_seen: u64,
    /// Device consumption at the last open/seek, and frames pushed to the device since: the
    /// audio-only end is when the device has played every pushed frame.
    consumed_base: u64,
    pushed_frames: u64,
    clock: MasterClock,
    sched: Scheduler,
    meter: DriftMeter,
    time: Arc<dyn TimeSource>,
    /// Decoded frames in pts order; a frame's ordinal is the count of frames popped before it.
    queue: VecDeque<Frame>,
    next_ordinal: u64,
    /// Frames with pts below this are decoded but not shown (after a seek).
    discard_before: i64,
    /// After a seek to a time inside a frame's interval: the latest frame decoded with pts
    /// below the target — the frame that COVERS the target (HTML seeks show the frame whose
    /// interval holds the new position; AETHERVIDEO found the gap). Shown first when the next
    /// decoded frame starts after the target, or at end of stream.
    held: Option<Frame>,
    on_glass: Option<i64>,
    eof: bool,
    start_ns: i64,
    info: MediaInfo,
}

/// The bare-audio-file probe: a format `dsp::audio` decodes and a demuxer does not.
fn open_audio_file(bytes: &[u8]) -> Result<(ac::Decoder, ac::Info), String> {
    let dec = ac::Decoder::open_bytes(bytes.to_vec()).map_err(|e| e.to_string())?;
    let info = dec.info();
    Ok((dec, info))
}

impl Player {
    /// The audio sample rate a stream will play at (0 when it has no decodable audio): what
    /// the bus opens the resonance output with.
    pub fn probe_audio_rate(bytes: &[u8]) -> u32 {
        match Demuxer::open(bytes.to_vec()) {
            Ok(d) => d.audio_track().map(|t| if t.codec == gneiss_pal::dsp::demux::Codec::Opus { 48_000 } else { t.sample_rate }).unwrap_or(0),
            Err(_) => open_audio_file(bytes).map(|(_, i)| i.rate).unwrap_or(0),
        }
    }

    /// Open a stream. `aout` is the audio device side (or `None` for silent / headless); it is
    /// used only when the stream has decodable audio. `display_hz` sets the scheduler's early
    /// window (half a display period).
    pub fn open(bytes: Vec<u8>, time: Arc<dyn TimeSource>, aout: Option<Box<dyn AudioOut>>, display_hz: u32) -> Result<Player, OpenError> {
        let mut audio_note = String::new();
        let mut audio_start_ns = 0i64;
        let (demux, vtrack, vdec, audio, duration_ns, start_ns) = match Demuxer::open(bytes.clone()) {
            Ok(demux) => {
                let vtrack = demux.tracks().iter().find(|t| t.kind == TrackKind::Video).cloned();
                let vdec: Option<Box<dyn VideoDecoder>> = vtrack.as_ref().map(|vt| match decoder_for(vt) {
                    Ok(d) => d,
                    Err(e) => {
                        log::info!("[STRIA] :: {e}; test-pattern stand-in");
                        Box::new(TestPattern::stand_in(vt)) as Box<dyn VideoDecoder>
                    }
                });
                let atrack = demux.tracks().iter().find(|t| t.kind == TrackKind::Audio).cloned();
                let mut audio = None;
                let mut astart: Option<i64> = None;
                match &atrack {
                    None => audio_note = "no audio track".into(),
                    Some(t) => {
                        let timing = Timing::of(&demux, t);
                        match audio_decoder_for(t, timing) {
                            Ok(d) => {
                                let idx = demux.track_index(t.id).unwrap_or(0);
                                let first = demux.track_samples(idx).map(|s| t.to_ns(s.pts)).min().unwrap_or(0) - timing.shift_ns;
                                astart = Some(if timing.floor_ns == i64::MIN { first } else { first.max(timing.floor_ns) });
                                audio = Some(AudioSrc::Track { track: t.clone(), dec: d });
                            }
                            Err(e) => audio_note = e.to_string(),
                        }
                    }
                }
                if vtrack.is_none() && audio.is_none() {
                    return Err(OpenError::NoMedia(if audio_note.is_empty() { "no video or audio track".into() } else { audio_note }));
                }
                // Media time starts at the first presented sample of either track (fragmented
                // MP4 and Matroska streams often start later than zero).
                let vstart = vtrack.as_ref().and_then(|vt| {
                    let vi = demux.track_index(vt.id)?;
                    demux.track_samples(vi).map(|s| vt.to_ns(s.pts)).min()
                });
                let start = match (vstart, astart) {
                    (Some(v), Some(a)) => v.min(a),
                    (Some(v), None) => v,
                    (None, Some(a)) => a,
                    (None, None) => 0,
                };
                let dur = demux.duration_ns();
                audio_start_ns = astart.map(|a| a - start).unwrap_or(0);
                (Some(demux), vtrack, vdec, audio, dur, start)
            }
            Err(container_err) => match open_audio_file(&bytes) {
                Ok((dec, info)) => {
                    let frames = match info.frames {
                        Some(f) => f,
                        None => {
                            // no stated length (an MP3 without a Xing header): count it once
                            let mut d = ac::Decoder::open_bytes(bytes.clone()).map_err(|e| OpenError::NoMedia(e.to_string()))?;
                            let mut b = vec![0f32; FILE_CHUNK * info.channels.max(1) as usize];
                            let mut n = 0u64;
                            while let Ok(k) = d.next(&mut b) {
                                if k == 0 {
                                    break;
                                }
                                n += k as u64;
                            }
                            n
                        }
                    };
                    let dur = (frames as u128 * 1_000_000_000 / info.rate.max(1) as u128) as u64;
                    let src = AudioSrc::File { bytes, dec: Box::new(dec), rate: info.rate, channels: info.channels, frames: 0, buf: Vec::new() };
                    (None, None, None, Some(src), dur, 0)
                }
                Err(_) => return Err(OpenError::Container(container_err)),
            },
        };
        let (audio_rate, channels, audio_decoder, audio_name) = match &audio {
            Some(AudioSrc::Track { track, dec }) => (dec.sample_rate(), dec.channels(), dec.name().to_string(), track.codec_name.clone()),
            Some(AudioSrc::File { rate, channels, dec, .. }) => (*rate, *channels, format!("{:?}", dec.info().codec).to_lowercase(), dec.info().format.name().to_string()),
            None => (0, 0, String::new(), String::new()),
        };
        let mut aout = match (&audio, aout) {
            (Some(_), Some(o)) => Some(o),
            (Some(_), None) => {
                audio_note = "no audio output".into();
                None
            }
            (None, _) => None,
        };
        if let Some(o) = aout.as_mut() {
            o.set_paused(true); // a player opens paused (the poster); play() releases the device
        }
        let mut clock = MasterClock::select(time.clone(), aout.as_ref().map(|_| (audio_rate, 0, 20_000_000)));
        clock.seek(start_ns);
        let (width, height, video, real_video, video_decoder) = match (&vtrack, &vdec) {
            (Some(t), Some(d)) => (t.width, t.height, t.codec_name.clone(), d.is_real(), d.name().to_string()),
            _ => (0, 0, String::new(), false, String::new()),
        };
        let info = MediaInfo {
            duration_ns,
            width,
            height,
            video,
            audio: audio_name,
            real_video,
            video_decoder,
            audio_decoder,
            audio_note,
            audio_clock: clock.is_audio(),
            has_video: vtrack.is_some(),
            sample_rate: if audio.is_some() { audio_rate } else { 0 },
            channels: if audio.is_some() { channels } else { 0 },
            audio_start_ns,
        };
        let early = 500_000_000 / display_hz.max(1) as u64;
        Ok(Player {
            demux,
            vtrack,
            vdec,
            audio,
            aout,
            pending: VecDeque::new(),
            audio_rate,
            audio_base_ns: start_ns,
            audio_frames: 0,
            level_acc: None,
            levels: VecDeque::new(),
            audio_eof: false,
            muted: false,
            consumed_seen: 0,
            consumed_base: 0,
            pushed_frames: 0,
            clock,
            sched: Scheduler::new(early),
            meter: DriftMeter::default(),
            time,
            queue: VecDeque::new(),
            next_ordinal: 0,
            discard_before: i64::MIN,
            held: None,
            on_glass: None,
            eof: false,
            start_ns,
            info,
        })
    }

    pub fn info(&self) -> &MediaInfo {
        &self.info
    }
    pub fn stats(&self) -> SchedStats {
        self.sched.stats
    }
    pub fn drift(&self) -> DriftReport {
        self.meter.report()
    }
    pub fn clock_ns(&self) -> i64 {
        self.clock.media_ns()
    }
    /// The video track (None for an audio-only session).
    pub fn video_track(&self) -> Option<&Track> {
        self.vtrack.as_ref()
    }
    pub fn play(&mut self) {
        if let Some(o) = self.aout.as_mut() {
            o.set_paused(false);
        }
        self.clock.play();
    }
    pub fn pause(&mut self) {
        self.clock.pause();
        if let Some(o) = self.aout.as_mut() {
            o.set_paused(true);
        }
    }

    /// Mute / unmute: the device plays zeros in place of the stream (already-queued samples
    /// in the device ring, ≤ ½ s, play out first). Meters keep measuring the stream.
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    /// Seek: land on the keyframe at or before `ns`, decode forward silently to `ns`. The
    /// device ring is flushed, so no pre-seek audio plays after the jump.
    pub fn seek(&mut self, ns: i64) {
        if let Some(d) = self.demux.as_mut() {
            let target = if self.vtrack.is_none() { (ns - AUDIO_PREROLL_NS).max(self.start_ns) } else { ns };
            let _ = d.seek(target);
        }
        if let Some(v) = self.vdec.as_mut() {
            v.reset();
        }
        match self.audio.as_mut() {
            Some(AudioSrc::Track { dec, .. }) => dec.reset(),
            Some(AudioSrc::File { bytes, dec, frames, .. }) => {
                if let Ok(d) = ac::Decoder::open_bytes(bytes.clone()) {
                    **dec = d;
                }
                *frames = 0;
            }
            None => {}
        }
        self.queue.clear();
        self.pending.clear();
        if let Some(o) = self.aout.as_mut() {
            o.flush();
            self.consumed_seen = o.consumed();
        }
        self.consumed_base = self.consumed_seen;
        self.pushed_frames = 0;
        self.audio_base_ns = ns;
        self.audio_frames = 0;
        self.level_acc = None;
        self.levels.clear();
        self.audio_eof = false;
        self.eof = false;
        self.discard_before = ns;
        self.held = None;
        // Ordinals stay presentation indices: the first frame shown after the seek is preceded
        // by every video sample whose pts is earlier.
        self.next_ordinal = match (&self.vtrack, &self.demux) {
            (Some(vt), Some(d)) => {
                let vi = d.track_index(vt.id).unwrap_or(0);
                d.track_samples(vi).filter(|s| vt.to_ns(s.pts) < ns).count() as u64
            }
            _ => 0,
        };
        self.on_glass = None;
        self.clock.seek(ns);
    }

    fn insert_frame(&mut self, f: Frame) {
        if f.pts_ns < self.discard_before {
            if self.held.as_ref().is_none_or(|h| h.pts_ns < f.pts_ns) {
                self.held = Some(f);
            }
            return;
        }
        if f.pts_ns > self.discard_before {
            self.release_held();
        } else {
            self.held = None;
        }
        // Presentation order: insert by pts (a reordering decoder already emits in order; the
        // stand-in decodes B-frame streams in decode order).
        let pos = self.queue.iter().position(|q| q.pts_ns > f.pts_ns).unwrap_or(self.queue.len());
        self.queue.insert(pos, f);
    }

    /// Queue the held pre-target frame (see `held`): it precedes every queued frame, and its
    /// ordinal is one below the first frame at or after the target.
    fn release_held(&mut self) {
        if let Some(h) = self.held.take() {
            self.next_ordinal = self.next_ordinal.saturating_sub(1);
            self.queue.push_front(h);
        }
    }

    fn audio_next_ns(&self) -> i64 {
        self.audio_base_ns + (self.audio_frames as i128 * 1_000_000_000 / self.audio_rate.max(1) as i128) as i64
    }

    /// Place a decoded block on the timeline: drop what lies before the next expected sample,
    /// fill a gap with silence, convert to stereo, meter it, queue it.
    fn queue_audio(&mut self, pts_ns: i64, rate: u32, channels: u16, samples: &[f32]) {
        if rate != self.audio_rate || samples.is_empty() {
            if rate != self.audio_rate && !samples.is_empty() {
                log::warn!("[STRIA] :: audio rate changed mid-stream ({} → {rate}); block skipped", self.audio_rate);
            }
            return;
        }
        let ch = channels.max(1) as usize;
        let frames = samples.len() / ch;
        let next = self.audio_next_ns();
        let diff = ((pts_ns - next) as i128 * rate as i128 / 1_000_000_000) as i64;
        let mut stereo = to_stereo(samples, ch);
        if diff < 0 {
            let drop = ((-diff) as usize).min(frames);
            stereo.drain(..drop * 2);
        } else if diff > 0 {
            // a gap (a track starting later, a lost stretch): silence keeps the clock true
            let gap = (diff as usize).min(rate as usize * 10);
            let mut g = vec![0f32; gap * 2];
            g.extend_from_slice(&stereo);
            stereo = g;
        }
        let t0 = self.audio_next_ns();
        for (i, fr) in stereo.chunks_exact(2).enumerate() {
            let t = t0 + (i as i128 * 1_000_000_000 / rate as i128) as i64;
            let w = t.div_euclid(LEVEL_WINDOW_NS);
            match self.level_acc.as_mut() {
                Some((idx, l, r)) if *idx == w => {
                    *l = l.max(fr[0].abs());
                    *r = r.max(fr[1].abs());
                }
                _ => {
                    if let Some((idx, l, r)) = self.level_acc.take() {
                        self.levels.push_back((idx * LEVEL_WINDOW_NS, [l, r]));
                    }
                    self.level_acc = Some((w, fr[0].abs(), fr[1].abs()));
                }
            }
        }
        self.audio_frames += (stereo.len() / 2) as u64;
        self.pending.extend(stereo);
    }

    fn route(&mut self, p: Packet) {
        if Some(p.track) == self.vtrack.as_ref().map(|t| t.id) {
            if let Some(v) = self.vdec.as_mut() {
                match v.decode(&p) {
                    Ok(Some(f)) => self.insert_frame(f),
                    Ok(None) => {}
                    Err(e) => log::warn!("[STRIA] :: video decode: {e}"),
                }
            }
        } else if let Some(AudioSrc::Track { track, dec }) = self.audio.as_mut() {
            if p.track == track.id {
                match dec.decode(&p) {
                    Ok(b) => self.queue_audio(b.pts_ns, b.sample_rate, b.channels, &b.samples),
                    Err(e) => log::warn!("[STRIA] :: audio decode: {e}"),
                }
            }
        }
    }

    fn audio_ahead_ns(&self) -> i64 {
        if self.audio_rate == 0 {
            return i64::MAX;
        }
        (self.pending.len() / 2) as i64 * 1_000_000_000 / self.audio_rate as i64
    }

    /// Decode until the video queue holds enough frames (or audio enough samples), or EOF.
    fn pump(&mut self) {
        loop {
            let want_video = self.vtrack.is_some() && !self.eof && self.queue.len() < VIDEO_AHEAD;
            let want_audio = self.audio.is_some() && !self.audio_eof && self.audio_ahead_ns() < AUDIO_AHEAD_NS && (self.vtrack.is_none() || self.queue.len() < VIDEO_AHEAD * 4);
            if !want_video && !want_audio {
                break;
            }
            if let Some(d) = self.demux.as_mut() {
                match d.next_packet() {
                    Some(p) => self.route(p),
                    None => {
                        self.eof = true;
                        self.audio_eof = true;
                        if let Some(v) = self.vdec.as_mut() {
                            for f in v.flush() {
                                self.insert_frame(f);
                            }
                        }
                        // A seek past the last frame's start still shows the last frame.
                        if self.queue.is_empty() {
                            self.release_held();
                        }
                    }
                }
            } else if let Some(AudioSrc::File { dec, rate, channels, frames, buf, .. }) = self.audio.as_mut() {
                let ch = (*channels).max(1) as usize;
                buf.resize(FILE_CHUNK * ch, 0.0);
                let n = dec.next(buf).unwrap_or(0);
                if n == 0 {
                    self.audio_eof = true;
                    self.eof = true;
                    continue;
                }
                let pts = (*frames as i128 * 1_000_000_000 / (*rate).max(1) as i128) as i64;
                *frames += n as u64;
                let (r, c) = (*rate, *channels);
                let block: Vec<f32> = buf[..n * ch].to_vec();
                self.queue_audio(pts, r, c, &block);
            } else {
                break;
            }
        }
    }

    fn feed_audio(&mut self) {
        let Some(out) = self.aout.as_mut() else {
            // no device: the samples behind the clock are gone (wall-clock playback, silent)
            let now = self.clock.media_ns();
            let behind = ((now - (self.audio_next_ns() - self.audio_ahead_ns())) as i128 * self.audio_rate as i128 / 1_000_000_000).max(0) as usize;
            let k = (behind * 2).min(self.pending.len());
            self.pending.drain(..k);
            return;
        };
        let c = out.consumed();
        let delta = c.saturating_sub(self.consumed_seen);
        self.consumed_seen = c;
        self.clock.on_audio_played(delta);
        while !self.pending.is_empty() {
            let (a, b) = self.pending.as_slices();
            let chunk = if a.len() >= 2 { a } else { b };
            if chunk.len() < 2 {
                // a frame split across the ring's seam: make it contiguous
                self.pending.make_contiguous();
                continue;
            }
            let took = if self.muted { out.push(&vec![0.0; chunk.len()]) } else { out.push(chunk) };
            if took == 0 {
                break;
            }
            self.pushed_frames += (took / 2) as u64;
            self.pending.drain(..took);
        }
    }

    /// The meter windows that are due at `now`.
    fn due_levels(&mut self, now: i64, eof: bool) -> Option<(i64, Vec<[f32; 2]>)> {
        if eof && self.pending.is_empty() {
            if let Some((idx, l, r)) = self.level_acc.take() {
                self.levels.push_back((idx * LEVEL_WINDOW_NS, [l, r]));
            }
        }
        let mut first = None;
        let mut out = Vec::new();
        while let Some(&(t, p)) = self.levels.front() {
            if t > now {
                break;
            }
            self.levels.pop_front();
            first.get_or_insert(t);
            out.push(p);
        }
        first.map(|t| (t, out))
    }

    /// Every queued sample has been played: by the device (all pushed frames consumed), or,
    /// with no device, by the wall clock.
    fn audio_drained(&self, now: i64) -> bool {
        match self.aout.as_ref() {
            Some(o) => o.consumed().saturating_sub(self.consumed_base) >= self.pushed_frames,
            None => now >= self.audio_next_ns(),
        }
    }

    /// One display refresh.
    pub fn tick(&mut self, sink: &mut dyn FrameSink) -> Tick {
        self.feed_audio();
        self.pump();
        self.feed_audio();
        let now = self.clock.media_ns();
        self.meter.record(self.time.now_ns() as i64, now);
        let mut presented = false;
        if let Some((t, peaks)) = self.due_levels(now, self.audio_eof) {
            sink.levels(t, &peaks);
            if self.vtrack.is_none() {
                presented = true;
            }
        }
        while let Some(head) = self.queue.front() {
            let next = self.queue.get(1).map(|f| f.pts_ns);
            match self.sched.decide(head.pts_ns, next, now) {
                Decision::Wait(_) => break,
                Decision::Drop => {
                    self.queue.pop_front();
                    self.next_ordinal += 1;
                }
                Decision::Present => {
                    let f = self.queue.pop_front().unwrap();
                    self.meter.av_offset(f.pts_ns, now);
                    sink.present(&f, self.next_ordinal);
                    self.next_ordinal += 1;
                    self.on_glass = Some(f.pts_ns);
                    presented = true;
                    break;
                }
            }
        }
        self.sched.on_display_tick(self.on_glass.is_some());
        if presented {
            Tick::Presented
        } else if self.vtrack.is_some() && self.eof && self.queue.is_empty() && (self.audio.is_none() || (self.pending.is_empty() && self.audio_drained(now))) {
            // a video ends with its last frame AND its audio (a soundtrack may outlast the picture)
            Tick::Ended
        } else if self.vtrack.is_none() && self.audio_eof && self.pending.is_empty() && self.levels.is_empty() && self.audio_drained(now) {
            Tick::Ended
        } else {
            Tick::Idle
        }
    }
}
