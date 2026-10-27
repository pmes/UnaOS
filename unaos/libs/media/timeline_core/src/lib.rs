// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `timeline_core` — the non-linear editor's timeline model (NLE1, LEDGER SR194).
//!
//! CHARTER: Stria — shared-core. Stria (`handlers/stria`, CODEX §2 A/V, "Premiere, VLC") owns editing as it owns
//! playback; this crate is the part of the editor both rings link (BP-handlers-4): the EDL, its edit operations, its
//! text form and the render schedule. It decodes nothing and does no I/O — `stria::nle` renders a schedule through the
//! dsp decoder registry; the on-UnaOS face (NLE6) will link this same crate.
//!
//! * The model: a [`Timeline`] holds a time base (one tick = one frame, `1/fps`), the [`Source`]s it cuts from, and
//!   [`Track`]s of [`Clip`]s. A clip shows source ticks `src_in..src_out` from timeline tick `at`.
//! * The operations are pure: each takes `&self` and returns a new, validated `Timeline` or an [`EditError`]
//!   ([`Timeline::place`], [`Timeline::split`], [`Timeline::trim_in`], [`Timeline::trim_out`],
//!   [`Timeline::ripple_delete`], [`Timeline::lift`], [`Timeline::move_clip`]).
//! * The invariants ([`Timeline::validate`]): on every track clips are ordered by `at` and never overlap; `at >= 0`;
//!   `0 <= src_in < src_out <= source.duration`; every clip names a source that exists.
//! * [`edl`]: the CMX 3600 text EDL (non-drop frame) — reader and writer.
//! * [`schedule`]: the render schedule — what source tick each output tick shows, and where a decoder must seek.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

pub mod edl;
pub mod schedule;

/// The ONE time-base type (demux_core's): seconds per tick = `num / den`. A timeline's is `1/fps`.
pub use demux_core::Timebase;

/// A source's identity inside one timeline (its index in [`Timeline::sources`] when built by [`Timeline::add_source`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SourceId(pub u32);

/// What a track carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Video,
    Audio,
}

/// A source medium: a reel name (CMX 3600: at most 8 characters, no whitespace) and its length in timeline ticks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub id: SourceId,
    pub reel: String,
    pub duration: i64,
}

/// One event: source ticks `src_in..src_out` (exclusive end) shown from timeline tick `at`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Clip {
    pub src: SourceId,
    pub src_in: i64,
    pub src_out: i64,
    pub at: i64,
}

impl Clip {
    pub fn new(src: SourceId, src_in: i64, src_out: i64, at: i64) -> Clip {
        Clip { src, src_in, src_out, at }
    }
    /// Ticks on the timeline.
    pub fn len(&self) -> i64 {
        self.src_out - self.src_in
    }
    pub fn is_empty(&self) -> bool {
        self.len() <= 0
    }
    /// The first timeline tick after the clip.
    pub fn end(&self) -> i64 {
        self.at + self.len()
    }
    /// True when timeline tick `t` shows this clip.
    pub fn covers(&self, t: i64) -> bool {
        self.at <= t && t < self.end()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Track {
    pub kind: Kind,
    pub clips: Vec<Clip>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeline {
    pub tb: Timebase,
    pub sources: Vec<Source>,
    pub tracks: Vec<Track>,
}

/// How [`Timeline::place`] makes room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Split whatever spans `at` and push everything from `at` on later by the clip's length (ripple).
    Insert,
    /// Cut `at..at+len` out of whatever is there (trimming or splitting it) and lay the clip in the hole.
    Overwrite,
}

/// Why an edit (or a timeline) was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditError {
    /// No such track index.
    NoTrack,
    /// No such clip index, or no clip strictly spans the split point.
    NoClip,
    /// A clip names a source the timeline does not hold.
    NoSource,
    /// Two clips on one track overlap (or are out of order).
    Overlap,
    /// `src_out` runs past the source's duration.
    OutOfSource,
    /// `src_in >= src_out`.
    Empty,
    /// A negative `at` or `src_in`.
    Negative,
    /// A move between a video and an audio track.
    Kind,
}

impl core::fmt::Display for EditError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            EditError::NoTrack => "no such track",
            EditError::NoClip => "no such clip",
            EditError::NoSource => "no such source",
            EditError::Overlap => "clips overlap",
            EditError::OutOfSource => "past the source's end",
            EditError::Empty => "empty clip",
            EditError::Negative => "negative time",
            EditError::Kind => "video and audio tracks do not mix",
        })
    }
}

/// What the schedule needs to know about the media behind each source. `stria::nle::Sources` implements it over the
/// demuxer's own keyframe search; a KAT implements it over a fixed GOP.
pub trait SourceIndex {
    /// The source's length in timeline ticks, if known (clips past it are refused by [`schedule::schedule`]).
    fn duration(&self, src: SourceId) -> Option<i64>;
    /// The presentation time (ns from the source's start) of the last keyframe of the source's `kind` stream at or
    /// before `t_ns` — where a decoder must seek to show `t_ns`. `None` if the source has no such stream.
    fn keyframe_ns(&self, src: SourceId, kind: Kind, t_ns: i64) -> Option<i64>;
}

impl Timeline {
    /// An empty timeline at an integer frame rate (one tick = one frame).
    pub fn new(fps: u32) -> Timeline {
        Timeline { tb: Timebase { num: 1, den: fps.max(1) as u64 }, sources: Vec::new(), tracks: Vec::new() }
    }
    /// The frame rate when the time base is `1/n`.
    pub fn fps(&self) -> Option<u32> {
        if self.tb.num == 1 && self.tb.den > 0 && self.tb.den <= u32::MAX as u64 { Some(self.tb.den as u32) } else { None }
    }
    /// Add a source (builder); its id is its index.
    pub fn add_source(&mut self, reel: &str, duration: i64) -> SourceId {
        let id = SourceId(self.sources.len() as u32);
        self.sources.push(Source { id, reel: String::from(reel), duration });
        id
    }
    /// Add an empty track (builder); returns its index.
    pub fn add_track(&mut self, kind: Kind) -> usize {
        self.tracks.push(Track { kind, clips: Vec::new() });
        self.tracks.len() - 1
    }
    pub fn source(&self, id: SourceId) -> Option<&Source> {
        self.sources.iter().find(|s| s.id == id)
    }
    pub fn source_by_reel(&self, reel: &str) -> Option<&Source> {
        self.sources.iter().find(|s| s.reel == reel)
    }
    /// A track's number within its kind, 1-based (V1, V2, A1 ...).
    pub fn track_number(&self, track: usize) -> Option<usize> {
        let k = self.tracks.get(track)?.kind;
        Some(self.tracks[..=track].iter().filter(|t| t.kind == k).count())
    }
    /// The index of the clip on `track` showing tick `t`.
    pub fn clip_at(&self, track: usize, t: i64) -> Option<usize> {
        self.tracks.get(track)?.clips.iter().position(|c| c.covers(t))
    }
    /// The first tick after the last clip on any track (0 when empty).
    pub fn end(&self) -> i64 {
        self.tracks.iter().flat_map(|t| t.clips.iter()).map(|c| c.end()).max().unwrap_or(0)
    }

    /// Check every invariant (see the crate docs).
    pub fn validate(&self) -> Result<(), EditError> {
        for t in &self.tracks {
            let mut prev: Option<&Clip> = None;
            for c in &t.clips {
                let s = self.source(c.src).ok_or(EditError::NoSource)?;
                if c.at < 0 || c.src_in < 0 {
                    return Err(EditError::Negative);
                }
                if c.src_in >= c.src_out {
                    return Err(EditError::Empty);
                }
                if c.src_out > s.duration {
                    return Err(EditError::OutOfSource);
                }
                // An out-of-order pair always fails this too (prev.at > c.at implies prev.end > c.at).
                if prev.is_some_and(|p| p.end() > c.at) {
                    return Err(EditError::Overlap);
                }
                prev = Some(c);
            }
        }
        Ok(())
    }

    /// Lay `clip` on `track` in `mode` (see [`Mode`]).
    pub fn place(&self, track: usize, clip: Clip, mode: Mode) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        if clip.is_empty() {
            return Err(EditError::Empty);
        }
        let t = tl.tracks.get_mut(track).ok_or(EditError::NoTrack)?;
        match mode {
            Mode::Insert => {
                split_at(t, clip.at);
                for c in t.clips.iter_mut().filter(|c| c.at >= clip.at) {
                    c.at += clip.len();
                }
            }
            Mode::Overwrite => carve(t, clip.at, clip.end()),
        }
        t.clips.push(clip);
        sort(t);
        tl.validate()?;
        Ok(tl)
    }

    /// Cut the clip strictly spanning tick `t` into two at `t` (the halves show the same frames as before).
    pub fn split(&self, track: usize, t: i64) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        let tr = tl.tracks.get_mut(track).ok_or(EditError::NoTrack)?;
        if !tr.clips.iter().any(|c| c.at < t && t < c.end()) {
            return Err(EditError::NoClip);
        }
        split_at(tr, t);
        tl.validate()?;
        Ok(tl)
    }

    /// Move a clip's in point to source tick `src_in`; its timeline start moves with it, its out edge stays.
    pub fn trim_in(&self, track: usize, idx: usize, src_in: i64) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        let c = clip_mut(&mut tl, track, idx)?;
        c.at += src_in - c.src_in;
        c.src_in = src_in;
        tl.validate()?;
        Ok(tl)
    }

    /// Move a clip's out point to source tick `src_out`; its timeline start stays.
    pub fn trim_out(&self, track: usize, idx: usize, src_out: i64) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        clip_mut(&mut tl, track, idx)?.src_out = src_out;
        tl.validate()?;
        Ok(tl)
    }

    /// Remove a clip and close the hole: every later clip on the track moves earlier by its length.
    pub fn ripple_delete(&self, track: usize, idx: usize) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        let tr = tl.tracks.get_mut(track).ok_or(EditError::NoTrack)?;
        if idx >= tr.clips.len() {
            return Err(EditError::NoClip);
        }
        let gone = tr.clips.remove(idx);
        for c in tr.clips.iter_mut().filter(|c| c.at >= gone.end()) {
            c.at -= gone.len();
        }
        tl.validate()?;
        Ok(tl)
    }

    /// Remove a clip and leave the hole (no ripple).
    pub fn lift(&self, track: usize, idx: usize) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        let tr = tl.tracks.get_mut(track).ok_or(EditError::NoTrack)?;
        if idx >= tr.clips.len() {
            return Err(EditError::NoClip);
        }
        tr.clips.remove(idx);
        tl.validate()?;
        Ok(tl)
    }

    /// Move a clip to tick `to_at` on track `to_track` (the same track or another of its kind); refused if it would
    /// overlap a clip there.
    pub fn move_clip(&self, track: usize, idx: usize, to_track: usize, to_at: i64) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        let (from_kind, to_kind) = match (tl.tracks.get(track), tl.tracks.get(to_track)) {
            (Some(a), Some(b)) => (a.kind, b.kind),
            _ => return Err(EditError::NoTrack),
        };
        if from_kind != to_kind {
            return Err(EditError::Kind);
        }
        if idx >= tl.tracks[track].clips.len() {
            return Err(EditError::NoClip);
        }
        let mut c = tl.tracks[track].clips.remove(idx);
        c.at = to_at;
        tl.tracks[to_track].clips.push(c);
        sort(&mut tl.tracks[to_track]);
        tl.validate()?;
        Ok(tl)
    }
}

fn clip_mut(tl: &mut Timeline, track: usize, idx: usize) -> Result<&mut Clip, EditError> {
    tl.tracks.get_mut(track).ok_or(EditError::NoTrack)?.clips.get_mut(idx).ok_or(EditError::NoClip)
}

fn sort(t: &mut Track) {
    t.clips.sort_by_key(|c| c.at);
}

/// Split the clip strictly spanning `t` (if any) into `..t` and `t..`.
fn split_at(t: &mut Track, at: i64) {
    if let Some(i) = t.clips.iter().position(|c| c.at < at && at < c.end()) {
        let c = t.clips[i];
        let cut = c.src_in + (at - c.at);
        t.clips[i].src_out = cut;
        t.clips.insert(i + 1, Clip { src: c.src, src_in: cut, src_out: c.src_out, at });
    }
}

/// Remove timeline ticks `a..b` from a track's clips (trimming or splitting what spans the edges).
fn carve(t: &mut Track, a: i64, b: i64) {
    split_at(t, a);
    split_at(t, b);
    t.clips.retain(|c| !(c.at >= a && c.end() <= b));
}
