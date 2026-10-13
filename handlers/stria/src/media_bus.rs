// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Stria's media bus face (PLAYBACK M4): the `Media*` verbs of `bandy::SMessage`.
//!
//! [`MediaService::spawn`] starts one thread that owns every playback session (a cpal stream is
//! not `Send` on every platform, so sessions never cross threads) and runs them at the display
//! cadence. Requests, keyed by the media url:
//!
//! | in | effect | out |
//! |---|---|---|
//! | `PlayMedia { url, .. }` | open (if new) and play | `MediaOpened`, `MediaFrame`…, `MediaEnded` |
//! | `MediaPoster { url }` | open, present the first frame, stay paused | `MediaOpened`, one `MediaFrame` |
//! | `MediaPause` / `MediaResume` | freeze / run the session's clock | frames stop / resume |
//! | `MediaSeek { url, position_ns }` | keyframe at or before, decode forward | the frame at the target |
//! | `MediaMute { url, muted }` | zero / restore the samples sent to the device (clock runs on) | |
//! | `MediaVolume { url, volume }` | scale the samples sent to the device (0..=1; mute wins) | |
//! | `MediaRate { url, rate }` | 0 holds the clock (session stays playing); 0.25..=4 runs the master clock at that rate, audio time-stretched (WSOLA, pitch kept); outside that, clamped (logged) | |
//! | `MediaStop { url }` | close the session | `MediaEnded` |
//!
//! A stream with no video track is an AUDIO-ONLY session (AUDIOTRACK, SR45): `MediaOpened`
//! carries width = height = 0 and `video` = "", and instead of pictures every tick whose audio
//! meters came due fires a 0×0 `MediaFrame` whose `levels` are the peak L/R per 50 ms window
//! (a video session attaches its due meters to the next frame). Failures (unreadable url,
//! nothing playable) answer `MediaError`. Urls are `file://` paths or bare paths; network
//! fetches are Aether's — it caches the http(s) resource and hands Stria the file
//! (AUDIOTRACK M4).
//!
//! AETHERMEDIA2 (SR134): a `mediasource:<journal>` key is a Media Source Extensions session — a
//! [`MediaSourceFeed`] tails the page's journal of MediaSource records each tick; the session opens
//! (`MediaOpened`) once every SourceBuffer has its initialization segment, plays the appended frames
//! through `Player::open_live`, holds at the buffered end, and ends (`MediaEnded`) after endOfStream.
//! Requests that arrive before it can open are remembered (play / pause).

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use bandy::{SMessage, Synapse};
use gneiss_pal::dsp::avsync::SystemTime;
use gneiss_pal::dsp::video::Frame;
use tokio::sync::broadcast::error::TryRecvError;

use crate::media::{AudioOut, FrameSink, Player, Tick, resonance_out};
use crate::media_source::{self, MediaSourceFeed};

struct Session {
    player: Player,
    _engine: Option<resonance::AudioEngine>,
    playing: bool,
    ended: bool,
    levels: Vec<[f32; 2]>,
    /// AETHERMEDIA2 (SR134): the MediaSource feed of a `mediasource:` session.
    feed: Option<MediaSourceFeed>,
}

/// AETHERMEDIA2 (SR134): a `mediasource:` session not yet open (no initialization segment on every
/// SourceBuffer): its feed and whether the requester asked it to play.
struct PendingFeed {
    feed: MediaSourceFeed,
    play: bool,
}

struct BusSink<'a> {
    synapse: &'a Synapse,
    url: &'a str,
    audio_only: bool,
    /// Meters waiting for the next video frame.
    levels: &'a mut Vec<[f32; 2]>,
}
impl FrameSink for BusSink<'_> {
    fn present(&mut self, f: &Frame, _ordinal: u64) {
        let levels = std::mem::take(self.levels);
        self.synapse.fire(SMessage::MediaFrame { url: self.url.to_string(), pts_ns: f.pts_ns, width: f.width, height: f.height, rgba: f.to_rgba(), levels });
    }
    fn levels(&mut self, pts_ns: i64, peaks: &[[f32; 2]]) {
        if self.audio_only {
            self.synapse.fire(SMessage::MediaFrame { url: self.url.to_string(), pts_ns, width: 0, height: 0, rgba: Vec::new(), levels: peaks.to_vec() });
        } else {
            self.levels.extend_from_slice(peaks);
            // a video's meters ride its frames; keep at most the last second's worth
            let n = self.levels.len();
            if n > 20 {
                self.levels.drain(..n - 20);
            }
        }
    }
}

/// Resolve a media url to a local path.
pub fn local_path(url: &str) -> Option<std::path::PathBuf> {
    if let Some(p) = url.strip_prefix("file://") {
        return Some(p.into());
    }
    if url.contains("://") {
        return None;
    }
    Some(url.into())
}

pub struct MediaService {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl MediaService {
    /// Start the service. `with_audio` opens a resonance output per session whose audio track
    /// decodes (false for headless hosts and tests: wall clock, silent).
    pub fn spawn(synapse: Synapse, display_hz: u32, with_audio: bool) -> MediaService {
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let mut rx = synapse.subscribe();
        let thread = std::thread::Builder::new()
            .name("stria-media".into())
            .spawn(move || {
                let period = Duration::from_nanos(1_000_000_000 / display_hz.max(1) as u64);
                let mut sessions: HashMap<String, Session> = HashMap::new();
                let mut feeds: HashMap<String, PendingFeed> = HashMap::new();
                let mut next = Instant::now();
                while !st.load(Ordering::Acquire) {
                    loop {
                        match rx.try_recv() {
                            Ok(msg) => {
                                if !handle_feed_request(&mut sessions, &mut feeds, &msg) {
                                    handle(&synapse, &mut sessions, msg, display_hz, with_audio)
                                }
                            }
                            Err(TryRecvError::Lagged(n)) => log::warn!("[STRIA] :: media bus lagged {n}"),
                            Err(_) => break,
                        }
                    }
                    poll_feeds(&synapse, &mut sessions, &mut feeds, display_hz, with_audio);
                    for (url, s) in sessions.iter_mut() {
                        if s.ended {
                            continue;
                        }
                        let audio_only = !s.player.info().has_video;
                        let mut sink = BusSink { synapse: &synapse, url, audio_only, levels: &mut s.levels };
                        if s.player.tick(&mut sink) == Tick::Ended && s.playing {
                            s.ended = true;
                            let st = s.player.stats();
                            synapse.fire(SMessage::MediaEnded { url: url.clone(), presented: st.presented, dropped: st.dropped });
                        }
                    }
                    next += period;
                    let now = Instant::now();
                    if next > now {
                        std::thread::sleep(next - now);
                    } else {
                        next = now;
                    }
                }
            })
            .expect("stria-media thread");
        MediaService { stop, thread: Some(thread) }
    }
}

impl Drop for MediaService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn open(synapse: &Synapse, url: &str, display_hz: u32, with_audio: bool) -> Option<Session> {
    let err = |e: String| {
        synapse.fire(SMessage::MediaError { url: url.to_string(), error: e });
        None
    };
    let Some(path) = local_path(url) else { return err("only local files (file://) are opened by Stria".into()) };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => return err(format!("{}: {e}", path.display())),
    };
    // Peek the audio rate to open a matching resonance stream.
    let rate = Player::probe_audio_rate(&bytes);
    let (engine, out): (Option<resonance::AudioEngine>, Option<Box<dyn AudioOut>>) = if with_audio && rate > 0 {
        match resonance_out(rate) {
            Some((e, f)) => (Some(e), Some(Box::new(f))),
            None => (None, None),
        }
    } else {
        (None, None)
    };
    match Player::open(bytes, Arc::new(SystemTime::new()), out, display_hz) {
        Ok(player) => {
            let i = player.info().clone();
            synapse.fire(SMessage::MediaOpened {
                url: url.to_string(),
                duration_ns: i.duration_ns,
                width: i.width,
                height: i.height,
                video: i.video,
                audio: i.audio,
                real_video: i.real_video,
                audio_clock: i.audio_clock,
            });
            Some(Session { player, _engine: engine, playing: false, ended: false, levels: Vec::new(), feed: None })
        }
        Err(e) => err(e.to_string()),
    }
}

/// AETHERMEDIA2 (SR134): a request for a `mediasource:` key that has no open session yet is kept on
/// its pending feed (created by `PlayMedia` / `MediaPoster`). True when the request was consumed.
fn handle_feed_request(sessions: &mut HashMap<String, Session>, feeds: &mut HashMap<String, PendingFeed>, msg: &SMessage) -> bool {
    let url = match msg {
        SMessage::PlayMedia { url, .. }
        | SMessage::MediaPoster { url }
        | SMessage::MediaPause { url }
        | SMessage::MediaResume { url }
        | SMessage::MediaStop { url } => url,
        _ => return false,
    };
    if !url.starts_with(media_source::SCHEME) || sessions.contains_key(url) {
        return false;
    }
    match msg {
        SMessage::PlayMedia { .. } | SMessage::MediaPoster { .. } | SMessage::MediaResume { .. } => {
            let play = !matches!(msg, SMessage::MediaPoster { .. });
            if let Some(p) = feeds.get_mut(url) {
                p.play |= play;
            } else if let Some(feed) = MediaSourceFeed::for_url(url) {
                feeds.insert(url.clone(), PendingFeed { feed, play });
            }
        }
        SMessage::MediaPause { .. } => {
            if let Some(p) = feeds.get_mut(url) {
                p.play = false;
            }
        }
        SMessage::MediaStop { .. } => {
            feeds.remove(url);
        }
        _ => {}
    }
    true
}

/// AETHERMEDIA2 (SR134): read every feed's journal; open the sessions whose SourceBuffers all have an
/// initialization segment (`MediaOpened`), and hand open sessions their rebuilt demuxer.
fn poll_feeds(synapse: &Synapse, sessions: &mut HashMap<String, Session>, feeds: &mut HashMap<String, PendingFeed>, hz: u32, with_audio: bool) {
    let mut ready: Vec<String> = Vec::new();
    for (url, p) in feeds.iter_mut() {
        p.feed.poll();
        if let Some(why) = &p.feed.broken {
            synapse.fire(SMessage::MediaError { url: url.clone(), error: format!("MediaSource journal: {why}") });
            ready.push(url.clone());
        } else if p.feed.ready() {
            ready.push(url.clone());
        }
    }
    for url in ready {
        let Some(p) = feeds.remove(&url) else { continue };
        if p.feed.broken.is_some() {
            continue;
        }
        let Some(demux) = p.feed.demuxer() else { continue };
        let rate = p.feed.audio_rate();
        let (engine, out): (Option<resonance::AudioEngine>, Option<Box<dyn AudioOut>>) = if with_audio && rate > 0 {
            match resonance_out(rate) {
                Some((e, f)) => (Some(e), Some(Box::new(f))),
                None => (None, None),
            }
        } else {
            (None, None)
        };
        match Player::open_live(demux, Arc::new(SystemTime::new()), out, hz) {
            Ok(mut player) => {
                player.refresh_live(demux_again(&p.feed), p.feed.duration_ns(), p.feed.ended());
                let i = player.info().clone();
                synapse.fire(SMessage::MediaOpened {
                    url: url.clone(),
                    duration_ns: p.feed.duration_ns(),
                    width: i.width,
                    height: i.height,
                    video: i.video,
                    audio: i.audio,
                    real_video: i.real_video,
                    audio_clock: i.audio_clock,
                });
                if p.play {
                    player.play();
                }
                sessions.insert(url, Session { player, _engine: engine, playing: p.play, ended: false, levels: Vec::new(), feed: Some(p.feed) });
            }
            Err(e) => synapse.fire(SMessage::MediaError { url, error: e.to_string() }),
        }
    }
    for s in sessions.values_mut() {
        let Some(feed) = s.feed.as_mut() else { continue };
        if feed.poll() {
            if let Some(d) = feed.demuxer() {
                s.player.refresh_live(d, feed.duration_ns(), feed.ended());
                s.ended = false;
            }
        }
    }
}

/// The feed's demuxer again (it is Some once the feed is ready), for the first `refresh_live`, which
/// positions the new session at the keyframe at or before 0 and states endOfStream.
fn demux_again(feed: &MediaSourceFeed) -> gneiss_pal::dsp::demux::Demuxer {
    feed.demuxer().unwrap_or_else(|| gneiss_pal::dsp::demux::Demuxer::from_frames(gneiss_pal::dsp::demux::Format::Mp4, Vec::new(), std::iter::empty(), None))
}

fn handle(synapse: &Synapse, sessions: &mut HashMap<String, Session>, msg: SMessage, hz: u32, with_audio: bool) {
    match msg {
        SMessage::PlayMedia { url, .. } => {
            if !sessions.contains_key(&url) {
                match open(synapse, &url, hz, with_audio) {
                    Some(s) => {
                        sessions.insert(url.clone(), s);
                    }
                    None => return,
                }
            }
            if let Some(s) = sessions.get_mut(&url) {
                s.player.play();
                s.playing = true;
            }
        }
        SMessage::MediaPoster { url } => {
            if !sessions.contains_key(&url) {
                if let Some(s) = open(synapse, &url, hz, with_audio) {
                    sessions.insert(url, s);
                }
            }
        }
        SMessage::MediaPause { url } => {
            if let Some(s) = sessions.get_mut(&url) {
                s.player.pause();
            }
        }
        SMessage::MediaResume { url } => {
            if let Some(s) = sessions.get_mut(&url) {
                s.player.play();
                s.playing = true;
            }
        }
        SMessage::MediaSeek { url, position_ns } => {
            if let Some(s) = sessions.get_mut(&url) {
                s.player.seek(position_ns as i64);
                s.ended = false;
            }
        }
        SMessage::MediaMute { url, muted } => {
            if let Some(s) = sessions.get_mut(&url) {
                s.player.set_muted(muted);
            }
        }
        // AETHERMEDIA (SR107): HTMLMediaElement.volume / playbackRate
        SMessage::MediaVolume { url, volume } => {
            if let Some(s) = sessions.get_mut(&url) {
                s.player.set_volume(volume);
            }
        }
        SMessage::MediaRate { url, rate } => {
            if let Some(s) = sessions.get_mut(&url) {
                if !s.player.set_rate(rate) {
                    log::warn!("[STRIA] :: {url}: playbackRate {rate} outside 0.25..=4; clamped to {}", s.player.rate());
                }
            }
        }
        SMessage::MediaStop { url } => {
            if let Some(s) = sessions.remove(&url) {
                let st = s.player.stats();
                synapse.fire(SMessage::MediaEnded { url, presented: st.presented, dropped: st.dropped });
            }
        }
        _ => {}
    }
}
