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
//!   [`Timeline::ripple_delete`], [`Timeline::lift`], [`Timeline::move_clip`]; NLE2c LEDGER SR217: over a set of clips
//!   as ONE edit, [`Timeline::ripple_delete_many`], [`Timeline::lift_many`], [`Timeline::move_many`]).
//! * The invariants ([`Timeline::validate`]): on every track clips are ordered by `at` and never overlap; `at >= 0`;
//!   `0 <= src_in < src_out <= source.duration`; every clip names a source that exists.
//! * [`edl`]: the CMX 3600 text EDL (non-drop frame) — reader and writer.
//! * [`schedule`]: the render schedule — what source tick each output tick shows, and where a decoder must seek.
//! * Transitions (NLE3, LEDGER SR202): a [`Transition`] (a dissolve or an edge wipe, `dur` ticks) sits on a cut of a
//!   video track and STARTS at it (CMX 3600's alignment): ticks `at..at+dur` show the incoming clip's own first `dur`
//!   ticks mixed with the outgoing source's handle (`src_out..src_out+dur`). [`Timeline::add_transition`] /
//!   [`Timeline::remove_transition`]; [`Timeline::validate`] refuses a transition off a cut ([`EditError::Transition`],
//!   so an op that moves or removes a transition's cut is refused until the transition is removed) or longer than the
//!   outgoing's handle or the incoming clip ([`EditError::Handle`]).
//! * NLE3b/NLE3c (LEDGER SR209): a video transition implies the audio crossfade on every audio track whose clips butt
//!   at the same cut with the same sources ([`Timeline::audio_crossfade`]; `validate` refuses [`EditError::Handle`]
//!   when the audio clips' handle is short); wipes gain SMPTE W041 / W101 / W201; a transition may come FROM black
//!   ([`Transition::from_black`], [`Timeline::add_transition_from_black`], CMX's `BL` outgoing line).
//! * Audio (NLE4, LEDGER SR203): a clip carries a gain in centi-dB ([`Clip::gain_cdb`], 0 = unity) and linear fades in
//!   ticks ([`Clip::fade_in`], [`Clip::fade_out`], 0 = none; `fade_in + fade_out <= len`); [`Timeline::set_gain`] and
//!   [`Timeline::set_fades`] edit them; [`schedule::audio_runs`] mixes every audio track.
//! * [`editor`] (NLE6, LEDGER SR207): the editor's state (`EditorState`: selection, playhead, zoom, undo / redo over the ops), one
//!   model for the host face (`stria::nle_face`) and the on-UnaOS face (the kernel Player's EDL mode).
//! * Edit lists (EDITMAP, LEDGER SR223): an MP4 `elst` IS a timeline over one source; [`Timeline::lay_edit_list`] lays
//!   a stream's segments on a track (each media segment a clip, an empty edit a gap, a repeat two clips over one source
//!   range) and refuses a rate other than 1 by name ([`EditError::Rate`]).

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

pub mod edl;
pub mod editor;
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
    /// NLE4: the clip's audio gain in centi-dB (0 = unity; [`GAIN_MIN_CDB`]..=[`GAIN_MAX_CDB`]).
    pub gain_cdb: i32,
    /// NLE4: a linear fade from silence over the clip's first `fade_in` ticks (0 = none).
    pub fade_in: i64,
    /// NLE4: a linear fade to silence over the clip's last `fade_out` ticks (0 = none).
    pub fade_out: i64,
}

/// NLE4: the gain range a clip may carry, centi-dB (-144 dB .. +24 dB).
pub const GAIN_MIN_CDB: i32 = -14_400;
pub const GAIN_MAX_CDB: i32 = 2_400;

impl Clip {
    pub fn new(src: SourceId, src_in: i64, src_out: i64, at: i64) -> Clip {
        Clip { src, src_in, src_out, at, gain_cdb: 0, fade_in: 0, fade_out: 0 }
    }
    /// The same clip at gain `cdb` (centi-dB).
    pub fn gained(self, cdb: i32) -> Clip {
        Clip { gain_cdb: cdb, ..self }
    }
    /// The same clip with fades of `fade_in` / `fade_out` ticks.
    pub fn faded(self, fade_in: i64, fade_out: i64) -> Clip {
        Clip { fade_in, fade_out, ..self }
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
    /// NLE3: the transitions on this track's cuts, ordered by `at` (video tracks only).
    pub transitions: Vec<Transition>,
}

/// An edge wipe's SMPTE pattern (CMX 3600 `W<nnn>`): the incoming source enters behind a moving edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WipeDir {
    /// `W001`: a vertical edge moving left to right (the incoming on the left).
    LeftToRight,
    /// `W002`: a horizontal edge moving top to bottom (the incoming on top).
    TopToBottom,
    /// NLE3c `W041` (SMPTE 258M diagonalWipe topLeft): a diagonal edge from the top-left corner (`x + y < t (w + h)`).
    Diagonal,
    /// NLE3c `W101` (SMPTE 258M irisWipe rectangle): a centred box growing to the frame (the incoming inside).
    Box,
    /// NLE3c `W201` (SMPTE 258M clockWipe clockwiseTwelve): a hand sweeping clockwise from twelve o'clock.
    Clock,
}

impl WipeDir {
    /// The SMPTE wipe code.
    pub fn code(self) -> u16 {
        match self {
            WipeDir::LeftToRight => 1,
            WipeDir::TopToBottom => 2,
            WipeDir::Diagonal => 41,
            WipeDir::Box => 101,
            WipeDir::Clock => 201,
        }
    }
    pub fn from_code(code: u16) -> Option<WipeDir> {
        match code {
            1 => Some(WipeDir::LeftToRight),
            2 => Some(WipeDir::TopToBottom),
            41 => Some(WipeDir::Diagonal),
            101 => Some(WipeDir::Box),
            201 => Some(WipeDir::Clock),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransitionKind {
    /// CMX `D`: the incoming fades in over the outgoing.
    Dissolve,
    /// CMX `W<nnn>`.
    Wipe(WipeDir),
}

/// A transition on the cut at timeline tick `at` (the outgoing clip's end = the incoming clip's `at`), lasting `dur`
/// ticks from the cut: the incoming shows its own first `dur` ticks, the outgoing its `dur` ticks past `src_out`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Transition {
    pub at: i64,
    pub kind: TransitionKind,
    pub dur: i64,
    /// NLE3c: the outgoing is BLACK (CMX `BL`): no clip ends at `at`, and the incoming fades / wipes in from black.
    pub from_black: bool,
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
    /// A transition not on a cut between two butted clips of a video track (or none there to remove, or `dur < 1`).
    Transition,
    /// A transition longer than the outgoing source's handle past its out point, or than the incoming clip.
    Handle,
    /// NLE4: a negative fade, or `fade_in + fade_out` longer than the clip.
    Fade,
    /// NLE4: a gain outside `GAIN_MIN_CDB..=GAIN_MAX_CDB`.
    Gain,
    /// EDITMAP (SR223): an edit-list segment at a rate other than 1 (slow / fast motion, or a rate-0 dwell hold): the
    /// model has no speed op.
    Rate,
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
            EditError::Transition => "transition is not on a cut of a video track",
            EditError::Handle => "transition is longer than a clip's handle",
            EditError::Fade => "fades longer than the clip",
            EditError::Gain => "gain out of range",
            EditError::Rate => "edit list rate is not 1 (no speed op)",
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
        self.tracks.push(Track { kind, clips: Vec::new(), transitions: Vec::new() });
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
                if c.fade_in < 0 || c.fade_out < 0 || c.fade_in + c.fade_out > c.len() {
                    return Err(EditError::Fade);
                }
                if !(GAIN_MIN_CDB..=GAIN_MAX_CDB).contains(&c.gain_cdb) {
                    return Err(EditError::Gain);
                }
                // An out-of-order pair always fails this too (prev.at > c.at implies prev.end > c.at).
                if prev.is_some_and(|p| p.end() > c.at) {
                    return Err(EditError::Overlap);
                }
                prev = Some(c);
            }
            self.validate_transitions(t)?;
        }
        Ok(())
    }

    /// NLE3: every transition sits on a cut between two butted clips of a video track, in `at` order, and fits the
    /// outgoing source's handle (`src_out + dur <= duration`) and the incoming clip (`dur <= len`).
    fn validate_transitions(&self, t: &Track) -> Result<(), EditError> {
        // NLE3b: an audio cut paired with a video transition crossfades, so its clips need the transition's handle too.
        if t.kind == Kind::Audio {
            for ci in 1..t.clips.len() {
                if let Some(x) = self.crossfade_of(&t.clips, ci) {
                    let (o, i) = (t.clips[ci - 1], t.clips[ci]);
                    let handle = self.source(o.src).ok_or(EditError::NoSource)?.duration - o.src_out;
                    if x.dur > i.len() || x.dur > handle {
                        return Err(EditError::Handle);
                    }
                }
            }
        }
        let mut last: Option<i64> = None;
        for x in &t.transitions {
            if t.kind != Kind::Video || x.dur < 1 || last.is_some_and(|l| x.at <= l) {
                return Err(EditError::Transition);
            }
            last = Some(x.at);
            let out = t.clips.iter().find(|c| c.end() == x.at);
            let inc = t.clips.iter().find(|c| c.at == x.at);
            if x.from_black {
                // NLE3c: from black: an incoming clip and NO outgoing one (a cut with an outgoing is a plain transition).
                let (None, Some(i)) = (out, inc) else { return Err(EditError::Transition) };
                if x.dur > i.len() {
                    return Err(EditError::Handle);
                }
                continue;
            }
            let (Some(o), Some(i)) = (out, inc) else { return Err(EditError::Transition) };
            let handle = self.source(o.src).ok_or(EditError::NoSource)?.duration - o.src_out;
            if x.dur > i.len() || x.dur > handle {
                return Err(EditError::Handle);
            }
        }
        Ok(())
    }

    /// NLE3: put a transition on the cut at tick `at` of `track` (replacing one already there).
    pub fn add_transition(&self, track: usize, at: i64, kind: TransitionKind, dur: i64) -> Result<Timeline, EditError> {
        self.put_transition(track, Transition { at, kind, dur, from_black: false })
    }

    /// NLE3c: put a transition FROM BLACK on the clip starting at tick `at` of `track` (no clip may end there).
    pub fn add_transition_from_black(&self, track: usize, at: i64, kind: TransitionKind, dur: i64) -> Result<Timeline, EditError> {
        self.put_transition(track, Transition { at, kind, dur, from_black: true })
    }

    /// The one body of the two `add_transition` ops: replace whatever sits at `x.at`, keep `at` order, validate.
    fn put_transition(&self, track: usize, x: Transition) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        let tr = tl.tracks.get_mut(track).ok_or(EditError::NoTrack)?;
        tr.transitions.retain(|y| y.at != x.at);
        tr.transitions.push(x);
        tr.transitions.sort_by_key(|y| y.at);
        tl.validate()?;
        Ok(tl)
    }

    /// NLE3b: the video transition whose audio crossfade sits on the cut before clip `ci` of audio track `track`.
    /// THE PAIRING RULE: the audio clip `ci` and the one before it butt at a cut `at` where some video track carries a
    /// (not from-black) transition whose incoming clip has the same source and starts at `at`, and whose outgoing clip
    /// has the same source as the audio's outgoing and ends at `at`. `None`: a plain audio cut.
    pub fn audio_crossfade(&self, track: usize, ci: usize) -> Option<Transition> {
        let t = self.tracks.get(track)?;
        if t.kind != Kind::Audio {
            return None;
        }
        self.crossfade_of(&t.clips, ci)
    }

    fn crossfade_of(&self, clips: &[Clip], ci: usize) -> Option<Transition> {
        let (o, i) = (clips.get(ci.checked_sub(1)?)?, clips.get(ci)?);
        if o.end() != i.at {
            return None;
        }
        self.tracks.iter().filter(|v| v.kind == Kind::Video).find_map(|v| {
            let x = v.transitions.iter().find(|x| x.at == i.at && !x.from_black)?;
            let inc = v.clips.iter().any(|c| c.at == i.at && c.src == i.src);
            let out = v.clips.iter().any(|c| c.end() == i.at && c.src == o.src);
            (inc && out).then_some(*x)
        })
    }

    /// NLE3: take the transition off the cut at tick `at` of `track` (the cut is a plain cut again).
    pub fn remove_transition(&self, track: usize, at: i64) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        let tr = tl.tracks.get_mut(track).ok_or(EditError::NoTrack)?;
        let n = tr.transitions.len();
        tr.transitions.retain(|x| x.at != at);
        if tr.transitions.len() == n {
            return Err(EditError::Transition);
        }
        tl.validate()?;
        Ok(tl)
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

    /// NLE4: set a clip's audio gain (centi-dB).
    pub fn set_gain(&self, track: usize, idx: usize, gain_cdb: i32) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        clip_mut(&mut tl, track, idx)?.gain_cdb = gain_cdb;
        tl.validate()?;
        Ok(tl)
    }

    /// NLE4: set a clip's fades (ticks); refused when they are negative or together longer than the clip.
    pub fn set_fades(&self, track: usize, idx: usize, fade_in: i64, fade_out: i64) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        let c = clip_mut(&mut tl, track, idx)?;
        (c.fade_in, c.fade_out) = (fade_in, fade_out);
        tl.validate()?;
        Ok(tl)
    }

    /// EDITMAP (LEDGER SR223): lay one stream's edit list (demux_core's [`demux_core::EditSegment`]s, every `elst` entry)
    /// on `track` as clips of `src`. `media` is the stream's time base and `origin_ns` its first presentation time (source
    /// tick 0, vplay's rule), so a segment's `media_pts` maps to source tick `from_ns(media.to_ns(media_pts) - origin_ns)`.
    /// An empty edit is a gap (the later segments' `start_ns` already count it); a media segment is a clip at
    /// `from_ns(start_ns)` lasting to `from_ns(start_ns + duration_ns)` (duration 0 = the rest of the source; a segment
    /// rounding to no tick is skipped; a `src_out` one tick past the source, rounding, is clamped to it); a rate other
    /// than 1 (`0x1_0000`) is [`EditError::Rate`]. An empty list is the identity: the whole source at tick 0.
    pub fn lay_edit_list(&self, track: usize, src: SourceId, edits: &[demux_core::EditSegment], media: Timebase, origin_ns: i64) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        let dur = tl.source(src).ok_or(EditError::NoSource)?.duration;
        let tb = tl.tb;
        let t = tl.tracks.get_mut(track).ok_or(EditError::NoTrack)?;
        if edits.is_empty() {
            t.clips.push(Clip::new(src, 0, dur, 0));
        }
        for e in edits {
            let Some(pts) = e.media_pts else { continue };
            if e.rate != 0x1_0000 {
                return Err(EditError::Rate);
            }
            let at = tb.from_ns(e.start_ns);
            let src_in = tb.from_ns(media.to_ns(pts).saturating_sub(origin_ns));
            let mut src_out = if e.duration_ns == 0 {
                dur
            } else {
                let end = tb.from_ns(e.start_ns.saturating_add(i64::try_from(e.duration_ns).unwrap_or(i64::MAX)));
                src_in + (end - at)
            };
            if src_out == dur + 1 {
                src_out = dur;
            }
            if src_out > src_in {
                t.clips.push(Clip::new(src, src_in, src_out, at));
            }
        }
        sort(t);
        tl.validate()?;
        Ok(tl)
    }

    /// NLE2c: ripple-delete every clip in `picks` (`(track, idx)`, indices into `self`) as ONE edit: [`Timeline::ripple_delete`]
    /// applied per pick in descending `(track, idx)` order on the running value. Any refusal refuses the whole set.
    pub fn ripple_delete_many(&self, picks: &[(usize, usize)]) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        for (t, i) in picks_desc(self, picks)? {
            tl = tl.ripple_delete(t, i)?;
        }
        Ok(tl)
    }

    /// NLE2c: lift every clip in `picks` as ONE edit ([`Timeline::lift`] per pick, descending; the holes stay).
    pub fn lift_many(&self, picks: &[(usize, usize)]) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        for (t, i) in picks_desc(self, picks)? {
            tl = tl.lift(t, i)?;
        }
        Ok(tl)
    }

    /// NLE2c: move every clip in `picks` by `delta` ticks on its own track as ONE transaction: all leave, all land, ONE
    /// `validate` (an overlap, a negative `at` or a pinned transition refuses the whole set). Butted clips move together,
    /// where a sequence of [`Timeline::move_clip`]s would trip on an intermediate overlap.
    pub fn move_many(&self, picks: &[(usize, usize)], delta: i64) -> Result<Timeline, EditError> {
        let order = picks_desc(self, picks)?;
        let mut tl = self.clone();
        let mut moved: Vec<(usize, Clip)> = Vec::new();
        for (t, i) in order {
            let mut c = tl.tracks[t].clips.remove(i);
            c.at = c.at.saturating_add(delta);
            moved.push((t, c));
        }
        for (t, c) in moved {
            tl.tracks[t].clips.push(c);
        }
        for t in &mut tl.tracks {
            sort(t);
        }
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

/// Split the clip strictly spanning `t` (if any) into `..t` and `t..`. Both halves keep the gain; the head keeps the
/// fade-in and the tail the fade-out, each clamped to its half (a split inside a fade shortens it, never invalidly).
fn split_at(t: &mut Track, at: i64) {
    if let Some(i) = t.clips.iter().position(|c| c.at < at && at < c.end()) {
        let c = t.clips[i];
        let cut = c.src_in + (at - c.at);
        let head = &mut t.clips[i];
        head.src_out = cut;
        head.fade_in = c.fade_in.min(head.len());
        head.fade_out = 0;
        let tail = Clip { src_in: cut, at, fade_in: 0, ..c };
        let tail = Clip { fade_out: c.fade_out.min(tail.len()), ..tail };
        t.clips.insert(i + 1, tail);
    }
}

/// Remove timeline ticks `a..b` from a track's clips (trimming or splitting what spans the edges).
fn carve(t: &mut Track, a: i64, b: i64) {
    split_at(t, a);
    split_at(t, b);
    t.clips.retain(|c| !(c.at >= a && c.end() <= b));
}

/// NLE2c: a set of `(track, idx)` picks checked against `tl` (none -> `NoClip`, a bad track -> `NoTrack`, a bad index ->
/// `NoClip`), deduped and sorted descending so removing them one by one never shifts a later pick.
fn picks_desc(tl: &Timeline, picks: &[(usize, usize)]) -> Result<Vec<(usize, usize)>, EditError> {
    if picks.is_empty() {
        return Err(EditError::NoClip);
    }
    for &(t, i) in picks {
        if i >= tl.tracks.get(t).ok_or(EditError::NoTrack)?.clips.len() {
            return Err(EditError::NoClip);
        }
    }
    let mut v = picks.to_vec();
    v.sort_unstable_by(|a, b| b.cmp(a));
    v.dedup();
    Ok(v)
}
