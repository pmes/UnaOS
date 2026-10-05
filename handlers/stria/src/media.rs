// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Stria's VIDEO track (PLAYBACK M3, LEDGER SR26; CODEX Amendment II: Stria owns A/V).
//!
//! [`Player`] is the whole pipeline for one stream:
//!
//! ```text
//!  bytes ─▶ dsp::demux::Demuxer ─▶ packets (decode order, tracks merged)
//!            ├─ video ─▶ dsp::video::VideoDecoder ─▶ frame queue (pts order)
//!            │                                      └▶ dsp::avsync::Scheduler ─▶ FrameSink (RGBA)
//!            └─ audio ─▶ dsp::audio_track decoder ─▶ mono f32 ─▶ AudioOut (resonance StreamSource)
//!                                                                  └▶ consumed count ─▶ AudioClock
//!  master clock = audio (when an audio track decodes and an output exists) else wall
//! ```
//!
//! The player is driven by [`Player::tick`], one call per display refresh; it owns no thread,
//! no window and no device, so the same code runs under a tokio interval (the bus service in
//! `media_bus`), under a headless manual clock (`tools/play-check`), and in tests.

use std::collections::VecDeque;
use std::sync::Arc;

use gneiss_pal::dsp::audio_track::{AudioTrackDecoder, audio_decoder_for};
use gneiss_pal::dsp::avsync::{Decision, DriftMeter, DriftReport, MasterClock, SchedStats, Scheduler, TimeSource};
use gneiss_pal::dsp::demux::{Demuxer, Packet, Track, TrackKind};
use gneiss_pal::dsp::video::{Frame, TestPattern, VideoDecoder, decoder_for};

/// Where presented frames go.
pub trait FrameSink {
    /// `ordinal` is the frame's index in presentation order since open (0-based; counted over
    /// every decoded frame, so drops leave gaps).
    fn present(&mut self, frame: &Frame, ordinal: u64);
}

/// The audio device side the player feeds: mono `f32` at the stream's rate.
pub trait AudioOut: Send {
    /// Queue samples; returns how many were taken.
    fn push(&mut self, mono: &[f32]) -> usize;
    /// Source samples the device has consumed since creation (monotonic).
    fn consumed(&self) -> u64;
}

impl AudioOut for resonance::StreamFeed {
    fn push(&mut self, mono: &[f32]) -> usize {
        resonance::StreamFeed::push(self, mono)
    }
    fn consumed(&self) -> u64 {
        resonance::StreamFeed::consumed(self)
    }
}

/// Open a resonance output that plays this stream's audio: a one-node graph (StreamSource) on
/// the default device. `None` when there is no device (headless hosts): the caller then runs on
/// the wall clock. The engine must be kept alive as long as the feed is used.
pub fn resonance_out(sample_rate: u32) -> Option<(resonance::AudioEngine, resonance::StreamFeed)> {
    let (src, feed) = resonance::stream_pair(sample_rate, sample_rate as usize / 2);
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
    /// The video decoder's name ("test-pattern", "test-pattern (stand-in)", ...).
    pub video_decoder: String,
    /// Why audio is silent, when it is (no track / no decoder / no device).
    pub audio_note: String,
    pub audio_clock: bool,
}

#[derive(Debug)]
pub enum OpenError {
    Container(gneiss_pal::dsp::demux::Error),
    NoVideo,
}
impl std::fmt::Display for OpenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OpenError::Container(e) => write!(f, "{e}"),
            OpenError::NoVideo => f.write_str("no video track"),
        }
    }
}
impl std::error::Error for OpenError {}

/// What one tick did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tick {
    /// Paused or waiting; nothing new on glass.
    Idle,
    /// A new frame went on glass.
    Presented,
    /// The stream is over: no frames left and the clock is past the last one.
    Ended,
}

/// Frames decoded ahead of the clock.
const VIDEO_AHEAD: usize = 4;
/// Audio queued ahead in the player (beyond the device ring), in ns.
const AUDIO_AHEAD_NS: i64 = 500_000_000;

pub struct Player {
    demux: Demuxer,
    vtrack: Track,
    vdec: Box<dyn VideoDecoder>,
    atrack: Option<Track>,
    adec: Option<Box<dyn AudioTrackDecoder>>,
    aout: Option<Box<dyn AudioOut>>,
    pending_audio: VecDeque<f32>,
    audio_rate: u32,
    consumed_seen: u64,
    clock: MasterClock,
    sched: Scheduler,
    meter: DriftMeter,
    time: Arc<dyn TimeSource>,
    /// Decoded frames in pts order; a frame's ordinal is the count of frames popped before it.
    queue: VecDeque<Frame>,
    next_ordinal: u64,
    /// Frames with pts below this are decoded but not shown (after a seek).
    discard_before: i64,
    on_glass: Option<i64>,
    eof: bool,
    info: MediaInfo,
}

impl Player {
    /// Open a stream. `aout` is the audio device side (or `None` for silent / headless); it is
    /// used only when the audio track has a decoder. `display_hz` sets the scheduler's early
    /// window (half a display period).
    pub fn open(bytes: Vec<u8>, time: Arc<dyn TimeSource>, aout: Option<Box<dyn AudioOut>>, display_hz: u32) -> Result<Player, OpenError> {
        let demux = Demuxer::open(bytes).map_err(OpenError::Container)?;
        let vtrack = demux.tracks().iter().find(|t| t.kind == TrackKind::Video).cloned().ok_or(OpenError::NoVideo)?;
        let vdec: Box<dyn VideoDecoder> = match decoder_for(&vtrack) {
            Ok(d) => d,
            Err(e) => {
                log::info!("[STRIA] :: {e}; test-pattern stand-in");
                Box::new(TestPattern::stand_in(&vtrack))
            }
        };
        let atrack = demux.tracks().iter().find(|t| t.kind == TrackKind::Audio).cloned();
        let mut audio_note = String::new();
        let (adec, aout) = match (&atrack, aout) {
            (None, _) => {
                audio_note = "no audio track".into();
                (None, None)
            }
            (Some(t), out) => match audio_decoder_for(t) {
                Ok(d) => match out {
                    Some(o) => (Some(d), Some(o)),
                    None => {
                        audio_note = "no audio output".into();
                        (None, None)
                    }
                },
                Err(e) => {
                    audio_note = e.to_string();
                    (None, None)
                }
            },
        };
        let audio_rate = atrack.as_ref().map(|t| t.sample_rate).unwrap_or(0);
        let mut clock = MasterClock::select(time.clone(), adec.as_ref().map(|_| (audio_rate, 0, 20_000_000)));
        // Media time starts at the first packet's presentation, not at zero (fragmented MP4 and
        // Matroska streams often start later).
        let start = demux.samples().iter().map(|s| demux.tracks()[s.track_index].to_ns(s.pts)).min().unwrap_or(0);
        clock.seek(start);
        let info = MediaInfo {
            duration_ns: demux.duration_ns(),
            width: vtrack.width,
            height: vtrack.height,
            video: vtrack.codec_name.clone(),
            audio: atrack.as_ref().map(|t| t.codec_name.clone()).unwrap_or_default(),
            real_video: vdec.is_real(),
            video_decoder: vdec.name().to_string(),
            audio_note,
            audio_clock: clock.is_audio(),
        };
        let early = 500_000_000 / display_hz.max(1) as u64;
        Ok(Player {
            demux,
            vtrack,
            vdec,
            atrack,
            adec,
            aout,
            pending_audio: VecDeque::new(),
            audio_rate,
            consumed_seen: 0,
            clock,
            sched: Scheduler::new(early),
            meter: DriftMeter::default(),
            time,
            queue: VecDeque::new(),
            next_ordinal: 0,
            discard_before: i64::MIN,
            on_glass: None,
            eof: false,
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
    pub fn video_track(&self) -> &Track {
        &self.vtrack
    }
    pub fn play(&mut self) {
        self.clock.play();
    }
    pub fn pause(&mut self) {
        self.clock.pause();
    }

    /// Seek: land on the keyframe at or before `ns`, decode forward silently to `ns`.
    pub fn seek(&mut self, ns: i64) {
        let _ = self.demux.seek(ns);
        self.vdec.reset();
        if let Some(a) = self.adec.as_mut() {
            a.reset();
        }
        self.queue.clear();
        self.pending_audio.clear();
        if let Some(o) = self.aout.as_ref() {
            self.consumed_seen = o.consumed();
        }
        self.eof = false;
        self.discard_before = ns;
        // Ordinals stay presentation indices: the first frame shown after the seek is preceded
        // by every video sample whose pts is earlier.
        let vt = &self.vtrack;
        let vi = self.demux.track_index(vt.id).unwrap_or(0);
        self.next_ordinal = self.demux.track_samples(vi).filter(|s| vt.to_ns(s.pts) < ns).count() as u64;
        self.on_glass = None;
        self.clock.seek(ns);
    }

    fn insert_frame(&mut self, f: Frame) {
        if f.pts_ns < self.discard_before {
            return;
        }
        // Presentation order: insert by pts (a reordering decoder already emits in order; the
        // stand-in decodes B-frame streams in decode order).
        let pos = self.queue.iter().position(|q| q.pts_ns > f.pts_ns).unwrap_or(self.queue.len());
        self.queue.insert(pos, f);
    }

    fn route(&mut self, p: Packet) {
        if p.track == self.vtrack.id {
            match self.vdec.decode(&p) {
                Ok(Some(f)) => self.insert_frame(f),
                Ok(None) => {}
                Err(e) => log::warn!("[STRIA] :: video decode: {e}"),
            }
        } else if Some(p.track) == self.atrack.as_ref().map(|t| t.id) {
            if let Some(dec) = self.adec.as_mut() {
                match dec.decode(&p) {
                    Ok(b) => {
                        let ch = b.channels.max(1) as usize;
                        // resonance's graph is mono: downmix by averaging.
                        for fr in b.samples.chunks_exact(ch) {
                            self.pending_audio.push_back(fr.iter().sum::<f32>() / ch as f32);
                        }
                    }
                    Err(e) => log::warn!("[STRIA] :: audio decode: {e}"),
                }
            }
        }
    }

    fn audio_ahead_ns(&self) -> i64 {
        if self.audio_rate == 0 {
            return i64::MAX;
        }
        self.pending_audio.len() as i64 * 1_000_000_000 / self.audio_rate as i64
    }

    /// Decode until the video queue holds enough frames (or audio enough samples), or EOF.
    fn pump(&mut self) {
        while !self.eof && (self.queue.len() < VIDEO_AHEAD || (self.adec.is_some() && self.audio_ahead_ns() < AUDIO_AHEAD_NS && self.queue.len() < VIDEO_AHEAD * 4)) {
            match self.demux.next_packet() {
                Some(p) => self.route(p),
                None => {
                    self.eof = true;
                    for f in self.vdec.flush() {
                        self.insert_frame(f);
                    }
                }
            }
        }
    }

    fn feed_audio(&mut self) {
        let Some(out) = self.aout.as_mut() else { return };
        let c = out.consumed();
        let delta = c.saturating_sub(self.consumed_seen);
        self.consumed_seen = c;
        self.clock.on_audio_played(delta);
        while !self.pending_audio.is_empty() {
            let (a, b) = self.pending_audio.as_slices();
            let chunk = if a.is_empty() { b } else { a };
            let took = out.push(chunk);
            if took == 0 {
                break;
            }
            self.pending_audio.drain(..took);
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
        } else if self.eof && self.queue.is_empty() {
            Tick::Ended
        } else {
            Tick::Idle
        }
    }
}
