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
//!   [`Timeline::remove_transition`]; [`Timeline::validate`] refuses a transition off a cut ([`EditError::Transition`];
//!   NLE3E SR225: an op first moves each transition to its surviving cut, refusing one whose cut is gone) or longer than the
//!   outgoing's handle or the incoming clip ([`EditError::Handle`]).
//! * NLE3b/NLE3c (LEDGER SR209): a video transition implies the audio crossfade on every audio track whose clips butt
//!   at the same cut with the same sources ([`Timeline::audio_crossfade`]; `validate` refuses [`EditError::Handle`]
//!   when the audio clips' handle is short); wipes gain SMPTE W041 / W101 / W201; a transition may come FROM black
//!   ([`Transition::from_black`], [`Timeline::add_transition_from_black`], CMX's `BL` outgoing line); NLE3cBLACK (LEDGER SR323): or go TO black ([`Transition::to_black`], [`Timeline::add_transition_to_black`], CMX's `BL` incoming line: the outgoing's handle over black in the gap `at..at + dur`), and wipes gain W003-W006 / W021 / W022 / W042 / W202-W204.
//! * Audio (NLE4, LEDGER SR203): a clip carries a gain in centi-dB ([`Clip::gain_cdb`], 0 = unity) and linear fades in
//!   ticks ([`Clip::fade_in`], [`Clip::fade_out`], 0 = none; `fade_in + fade_out <= len`); [`Timeline::set_gain`] and
//!   [`Timeline::set_fades`] edit them; [`schedule::audio_runs`] mixes every audio track.
//! * [`editor`] (NLE6, LEDGER SR207): the editor's state (`EditorState`: selection, playhead, zoom, undo / redo over the ops), one
//!   model for the host face (`stria::nle_face`) and the on-UnaOS face (the kernel Player's EDL mode).
//! * Edit lists (EDITMAP, LEDGER SR223): an MP4 `elst` IS a timeline over one source; [`Timeline::lay_edit_list`] lays
//!   a stream's segments on a track (each media segment a clip, an empty edit a gap, a repeat two clips over one source
//!   range) and refuses a rate other than 1 by name ([`EditError::Rate`]).
//! * [`vmix`] (NLE6MIX, rmbp-ledger B899): the ONE video transition mix ([`vmix::mix_step`]) and outgoing-run rule
//!   ([`vmix::take_outgoing`]) Stria's render and the kernel Player's EDL mode both call.
//! * [`view`] (NLE6KEYS, rmbp-ledger B961): the editor's ONE track view — the geometry ([`view::Layout`]) and the painter
//!   ([`view::paint_in`]) the host face (`stria::nle_face`) and the kernel Player's EDL view band both draw.
//! * EXPORTCOLOURPRIM (rmbp-ledger B979): [`Timeline::reels_colour`] — the ONE file-colour rule both exports call (Stria's `nle_export::export`, the kernel's EDL export).
//! * NLESIZEPOLICY (LEDGER SR394): each [`Source`] carries a [`SizePolicy`] (`* UNA SIZE` in the [`edl`]) — how a frame of
//!   another size than its reel's first fills the reel's box; [`SizePolicy::place`] is the ONE geometry both faces read.
//! * NLEXFADEPOWER (LEDGER SR678): a crossfade has a [`Curve`] (linear, or equal-power through [`schedule::xfade_weight`]);
//!   an audio-only crossfade with no video transition is an explicit [`AudioXfade`] record ([`Timeline::audio_xfades`],
//!   CMX's A-only `D` event); a BL transition's paired audio fades from / to silence; [`Timeline::crossfade_at`] is the
//!   ONE reader of an audio cut's fade (explicit, derived, from black) and [`Timeline::fade_to_silence`] its to-black twin.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;

pub mod edl;
pub mod editor;
pub mod schedule;
pub mod vmix;
pub mod view;

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
    /// NLESIZEPOLICY (SR394): how a frame of another size than the reel's first fills it ([`SizePolicy::Letterbox`] by default).
    pub size_policy: SizePolicy,
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
    /// STRIAGAINENVELOPE (SR9047): the clip's gain keyframes on its SOURCE ticks, centi-dB offsets over `gain_cdb` (empty = the flat gain).
    pub gain_env: GainEnv,
}

/// NLE4: the gain range a clip may carry, centi-dB (-144 dB .. +24 dB).
pub const GAIN_MIN_CDB: i32 = -14_400;
pub const GAIN_MAX_CDB: i32 = 2_400;

impl Clip {
    pub fn new(src: SourceId, src_in: i64, src_out: i64, at: i64) -> Clip {
        Clip { src, src_in, src_out, at, gain_cdb: 0, fade_in: 0, fade_out: 0, gain_env: GainEnv::EMPTY }
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
    /// STRIATRACKMUTESOLO (SR837): the track is muted (out of [`Timeline::active_tracks`] unless some track is solo).
    pub muted: bool,
    /// STRIATRACKMUTESOLO (SR837): the track is soloed (when any track is, only the solo tracks are active).
    pub solo: bool,
    /// STRIATRACKGAIN (SR864): the track's audio gain in centi-dB (0 = unity; the clip range), added to each clip's in the mix.
    pub gain_cdb: i32,
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
    /// NLE3cBLACK `W003` (SMPTE 258M boxWipe topLeft): a box growing from the top-left corner (the incoming inside).
    CornerTopLeft,
    /// NLE3cBLACK `W004` (boxWipe topRight): a box growing from the top-right corner.
    CornerTopRight,
    /// NLE3cBLACK `W005` (boxWipe bottomRight): a box growing from the bottom-right corner.
    CornerBottomRight,
    /// NLE3cBLACK `W006` (boxWipe bottomLeft): a box growing from the bottom-left corner.
    CornerBottomLeft,
    /// NLE3cBLACK `W021` (barnDoorWipe vertical): two vertical edges opening from the centre column (the incoming between).
    BarnDoorVertical,
    /// NLE3cBLACK `W022` (barnDoorWipe horizontal): two horizontal edges opening from the centre row.
    BarnDoorHorizontal,
    /// NLE3cBLACK `W042` (diagonalWipe topRight): a diagonal edge from the top-right corner.
    DiagonalTopRight,
    /// NLE3cBLACK `W202` (clockWipe clockwiseThree): a hand sweeping clockwise from three o'clock.
    ClockThree,
    /// NLE3cBLACK `W203` (clockWipe clockwiseSix): from six o'clock.
    ClockSix,
    /// NLE3cBLACK `W204` (clockWipe clockwiseNine): from nine o'clock.
    ClockNine,
    /// NLEWIPES (LEDGER SR679) `W007` (fourBoxWipe cornersIn): four boxes growing from the four corners, each to its quadrant.
    FourBoxCornersIn,
    /// NLEWIPES `W023` (boxWipe topCenter): a box growing from the top edge's midpoint.
    EdgeBoxTop,
    /// NLEWIPES `W024` (boxWipe rightCenter): from the right edge's midpoint.
    EdgeBoxRight,
    /// NLEWIPES `W025` (boxWipe bottomCenter): from the bottom edge's midpoint.
    EdgeBoxBottom,
    /// NLEWIPES `W026` (boxWipe leftCenter): from the left edge's midpoint.
    EdgeBoxLeft,
    /// NLEWIPES `W043` (bowTieWipe vertical): triangles on the top and bottom edges closing to the hourglass at t = 1/2.
    BowTieVertical,
    /// NLEWIPES `W044` (bowTieWipe horizontal): triangles on the left and right edges.
    BowTieHorizontal,
    /// NLEWIPES `W045` (barnDoorWipe diagonalBottomLeft): a band opening about the bottom-left to top-right diagonal.
    BarnDoorDiagonalBottomLeft,
    /// NLEWIPES `W046` (barnDoorWipe diagonalTopLeft): a band opening about the top-left to bottom-right diagonal.
    BarnDoorDiagonalTopLeft,
    /// NLEWIPES `W047` (miscDiagonalWipe doubleBarnDoor): both diagonal bands at once.
    DoubleBarnDoor,
    /// NLEWIPES `W102` (irisWipe diamond): a centred diamond growing to the corners.
    Diamond,
    /// NLEWIPES `W119` (ellipseWipe circle): a centred circle growing to the corners.
    Circle,
    /// NLEWIPES `W205` (pinWheelWipe twoBladeVertical): two hands from twelve and six, clockwise.
    PinWheelTwoVertical,
    /// NLEWIPES `W206` (pinWheelWipe twoBladeHorizontal): two hands from three and nine, clockwise.
    PinWheelTwoHorizontal,
    /// NLEWIPES `W207` (pinWheelWipe fourBlade): four hands from twelve, three, six and nine, clockwise.
    PinWheelFour,
    /// NLEWIPES `W211` (fanWipe centerTop): a fan about the centre opening both ways from twelve o'clock.
    FanCenterTop,
    /// NLEWIPES `W212` (fanWipe centerRight): from three o'clock.
    FanCenterRight,
    /// NLEWIPES `W213` (doubleFanWipe fanOutVertical): two fans about the centre opening from twelve and six.
    DoubleFanVertical,
    /// NLEWIPES `W214` (doubleFanWipe fanOutHorizontal): from three and nine.
    DoubleFanHorizontal,
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
            WipeDir::CornerTopLeft => 3,
            WipeDir::CornerTopRight => 4,
            WipeDir::CornerBottomRight => 5,
            WipeDir::CornerBottomLeft => 6,
            WipeDir::BarnDoorVertical => 21,
            WipeDir::BarnDoorHorizontal => 22,
            WipeDir::DiagonalTopRight => 42,
            WipeDir::ClockThree => 202,
            WipeDir::ClockSix => 203,
            WipeDir::ClockNine => 204,
            WipeDir::FourBoxCornersIn => 7,
            WipeDir::EdgeBoxTop => 23,
            WipeDir::EdgeBoxRight => 24,
            WipeDir::EdgeBoxBottom => 25,
            WipeDir::EdgeBoxLeft => 26,
            WipeDir::BowTieVertical => 43,
            WipeDir::BowTieHorizontal => 44,
            WipeDir::BarnDoorDiagonalBottomLeft => 45,
            WipeDir::BarnDoorDiagonalTopLeft => 46,
            WipeDir::DoubleBarnDoor => 47,
            WipeDir::Diamond => 102,
            WipeDir::Circle => 119,
            WipeDir::PinWheelTwoVertical => 205,
            WipeDir::PinWheelTwoHorizontal => 206,
            WipeDir::PinWheelFour => 207,
            WipeDir::FanCenterTop => 211,
            WipeDir::FanCenterRight => 212,
            WipeDir::DoubleFanVertical => 213,
            WipeDir::DoubleFanHorizontal => 214,
        }
    }
    pub fn from_code(code: u16) -> Option<WipeDir> {
        match code {
            1 => Some(WipeDir::LeftToRight),
            2 => Some(WipeDir::TopToBottom),
            41 => Some(WipeDir::Diagonal),
            101 => Some(WipeDir::Box),
            201 => Some(WipeDir::Clock),
            3 => Some(WipeDir::CornerTopLeft),
            4 => Some(WipeDir::CornerTopRight),
            5 => Some(WipeDir::CornerBottomRight),
            6 => Some(WipeDir::CornerBottomLeft),
            21 => Some(WipeDir::BarnDoorVertical),
            22 => Some(WipeDir::BarnDoorHorizontal),
            42 => Some(WipeDir::DiagonalTopRight),
            202 => Some(WipeDir::ClockThree),
            203 => Some(WipeDir::ClockSix),
            204 => Some(WipeDir::ClockNine),
            7 => Some(WipeDir::FourBoxCornersIn),
            23 => Some(WipeDir::EdgeBoxTop),
            24 => Some(WipeDir::EdgeBoxRight),
            25 => Some(WipeDir::EdgeBoxBottom),
            26 => Some(WipeDir::EdgeBoxLeft),
            43 => Some(WipeDir::BowTieVertical),
            44 => Some(WipeDir::BowTieHorizontal),
            45 => Some(WipeDir::BarnDoorDiagonalBottomLeft),
            46 => Some(WipeDir::BarnDoorDiagonalTopLeft),
            47 => Some(WipeDir::DoubleBarnDoor),
            102 => Some(WipeDir::Diamond),
            119 => Some(WipeDir::Circle),
            205 => Some(WipeDir::PinWheelTwoVertical),
            206 => Some(WipeDir::PinWheelTwoHorizontal),
            207 => Some(WipeDir::PinWheelFour),
            211 => Some(WipeDir::FanCenterTop),
            212 => Some(WipeDir::FanCenterRight),
            213 => Some(WipeDir::DoubleFanVertical),
            214 => Some(WipeDir::DoubleFanHorizontal),
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
    /// NLE3cBLACK (SR323): the incoming is BLACK (CMX `BL` incoming line): no clip starts at `at`; the outgoing's handle
    /// fades / wipes out to black over `at..at + dur`, the black filling that gap of the track. Never with `from_black`.
    pub to_black: bool,
    /// NLEWIPES (SR679): the easing curve the schedule applies ONCE to the incoming's weight `t` (Linear: unchanged).
    pub ease: Ease,
    /// NLE3ETRIMGROW (SR712): the length asked for while `dur` is a trim's FIT below it (`None`: `dur` is the ask).
    pub want: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Timeline {
    pub tb: Timebase,
    pub sources: Vec<Source>,
    pub tracks: Vec<Track>,
    /// NLEXFADEPOWER (SR678): the explicit audio crossfades (CMX's A-only `D` event), ordered by `(track, at)`.
    pub audio_xfades: Vec<AudioXfade>,
    /// NLE5DRANGESAVE (SR723): the export range's in / out marks (ticks; either may be unset), kept in the EDL as `* UNA MARK`.
    pub marks: (Option<i64>, Option<i64>),
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
    /// NLESIZEPOLICYUI (SR402): the editor's `size` verb named no [`SizePolicy`] word (or the wrong number of words).
    Policy,
    /// NLE5DRANGESAVE (SR723): both export marks set with the in mark not before the out mark.
    Marks,
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
            EditError::Policy => "no such size policy (letterbox, fill, stretch, adopt)",
            EditError::Marks => "the in mark is not before the out mark",
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
        Timeline { tb: Timebase { num: 1, den: fps.max(1) as u64 }, sources: Vec::new(), tracks: Vec::new(), audio_xfades: Vec::new(), marks: (None, None) }
    }
    /// The frame rate when the time base is `1/n`.
    pub fn fps(&self) -> Option<u32> {
        if self.tb.num == 1 && self.tb.den > 0 && self.tb.den <= u32::MAX as u64 { Some(self.tb.den as u32) } else { None }
    }
    /// Add a source (builder); its id is its index.
    pub fn add_source(&mut self, reel: &str, duration: i64) -> SourceId {
        let id = SourceId(self.sources.len() as u32);
        self.sources.push(Source { id, reel: String::from(reel), duration, size_policy: SizePolicy::Letterbox });
        id
    }
    /// Add an empty track (builder); returns its index.
    pub fn add_track(&mut self, kind: Kind) -> usize {
        self.tracks.push(Track { kind, clips: Vec::new(), transitions: Vec::new(), muted: false, solo: false, gain_cdb: 0 });
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
        for t in &self.tracks { if !(GAIN_MIN_CDB..=GAIN_MAX_CDB).contains(&t.gain_cdb) { return Err(EditError::Gain); } // STRIATRACKGAIN (SR864): a track's gain in the clip range
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
        self.validate_audio_xfades()?;
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
            if x.to_black {
                // NLE3cBLACK: to black: an outgoing clip, NO incoming one, its handle, and nothing starting inside the black.
                let (Some(o), None, false) = (out, inc, x.from_black) else { return Err(EditError::Transition) };
                let handle = self.source(o.src).ok_or(EditError::NoSource)?.duration - o.src_out;
                if x.dur > handle || t.clips.iter().any(|c| c.at > x.at && c.at < x.at + x.dur) {
                    return Err(EditError::Handle);
                }
                continue;
            }
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
        self.put_transition(track, Transition { at, kind, dur, from_black: false, to_black: false, ease: Ease::Linear, want: None })
    }

    /// NLE3c: put a transition FROM BLACK on the clip starting at tick `at` of `track` (no clip may end there).
    pub fn add_transition_from_black(&self, track: usize, at: i64, kind: TransitionKind, dur: i64) -> Result<Timeline, EditError> {
        self.put_transition(track, Transition { at, kind, dur, from_black: true, to_black: false, ease: Ease::Linear, want: None })
    }

    /// The one body of the two `add_transition` ops: replace whatever sits at `x.at`, keep `at` order, validate.
    fn put_transition(&self, track: usize, x: Transition) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        let tr = tl.tracks.get_mut(track).ok_or(EditError::NoTrack)?;
        let x = tr.transitions.iter().find(|y| y.at == x.at).map_or(x, |y| Transition { ease: y.ease, ..x }); // NLEWIPES (SR679): a re-put keeps its ease
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
        tl.retarget_transitions(self)?; tl.validate()?;
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
        tl.retarget_transitions(self)?; tl.fit_trimmed(track, idx); tl.validate()?;
        Ok(tl)
    }

    /// Move a clip's out point to source tick `src_out`; its timeline start stays.
    pub fn trim_out(&self, track: usize, idx: usize, src_out: i64) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        clip_mut(&mut tl, track, idx)?.src_out = src_out;
        tl.retarget_transitions(self)?; tl.fit_trimmed(track, idx); tl.validate()?;
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
        tl.retarget_transitions(self)?; tl.validate()?;
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
        tl.retarget_transitions(self)?; tl.validate()?;
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
        tl.retarget_transitions(self)?; tl.validate()?;
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

    /// NLE3E (LEDGER SR225): after an op moved clips, move every transition of `before` to where its cut now is. A cut
    /// is known by its clips' SOURCE edges (clips have no ids): the incoming's `(src, src_in)` and the outgoing's
    /// `(src, src_out)` (from black: no clip ends at the cut). The match nearest the old tick wins (ties: the earlier).
    /// A cut that no longer exists (a clip deleted, moved or trimmed at the cut, a clip inserted into it) is REFUSED by
    /// name, `EditError::Transition`, never dropped: the op did not name the transition. `validate` stays the one gate.
    fn retarget_transitions(&mut self, before: &Timeline) -> Result<(), EditError> {
        for (k, bt) in before.tracks.iter().enumerate() {
            if bt.transitions.is_empty() {
                continue;
            }
            let clips = &self.tracks.get(k).ok_or(EditError::NoTrack)?.clips;
            let mut moved = Vec::with_capacity(bt.transitions.len());
            for x in &bt.transitions {
                if x.to_black { moved.push(Transition { at: black_cut(bt, clips, x)?, ..*x }); continue; } // NLE3cBLACK
                let at = moved_cut(bt, clips, x.at)?; // NLEXFADEPOWER (SR678): the matcher moved to the file tail, shared
                moved.push(Transition { at, ..*x });
            }
            moved.sort_by_key(|y| y.at);
            self.tracks[k].transitions = moved;
        }
        self.retarget_audio_xfades(before)?;
        Ok(())
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

    /// NLE3cBLACK (LEDGER SR323): put a transition TO BLACK on the clip ending at tick `at_end` of `track` (no clip may
    /// start there): ticks `at_end..at_end + dur` show the outgoing's handle mixed with black, in the track's gap.
    pub fn add_transition_to_black(&self, track: usize, at_end: i64, kind: TransitionKind, dur: i64) -> Result<Timeline, EditError> {
        self.put_transition(track, Transition { at: at_end, kind, dur, from_black: false, to_black: true, ease: Ease::Linear, want: None })
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

/// NLE3cBLACK (LEDGER SR323): where a transition TO black on track `bt` (before an op) sits after it, among the op's
/// `clips`: its cut is known by the outgoing's `(src, src_out)` and by nothing starting at that clip's end. The match
/// nearest the old tick wins (ties: the earlier); none is the cut gone, refused by name like `retarget_transitions`.
fn black_cut(bt: &Track, clips: &[Clip], x: &Transition) -> Result<i64, EditError> {
    let out = bt.clips.iter().find(|c| c.end() == x.at).ok_or(EditError::Transition)?;
    clips
        .iter()
        .filter(|o| o.src == out.src && o.src_out == out.src_out)
        .map(|o| o.end())
        .filter(|&at| !clips.iter().any(|c| c.at == at))
        .min_by_key(|&at| ((at - x.at).abs(), at))
        .ok_or(EditError::Transition)
}

// ── EXPORTCOLOURPRIM (rmbp-ledger B979): the export's file primaries / transfer (handlers.md §5 NLEPROBE-HDR's EXPORT-COLOUR-PRIM) ──
// The mix converts each reel to R'G'B' by its own matrix and no primaries conversion exists (MIX-PRIMARIES owed), so an
// export can state ONE primaries / transfer pair only when every reel it cuts states the same one.
impl Timeline {
    /// The H.273 (primaries, transfer) every source a video-track clip cuts states, when all of them state one and the same
    /// pair; (2, 2) unspecified when one differs, one states no colour, or no video clip exists. `colour` is the face's reel
    /// colour: Stria's `Sources::open_report(id).colour`, the kernel's EDL reel's opened `Track::colour`.
    pub fn reels_colour(&self, colour: impl Fn(SourceId) -> Option<demux_core::colour::Colour>) -> (u8, u8) {
        let mut pair = None;
        for c in self.tracks.iter().filter(|t| t.kind == Kind::Video).flat_map(|t| t.clips.iter()) {
            let Some(p) = colour(c.src).map(|k| (k.primaries, k.transfer)) else { return (2, 2) };
            if *pair.get_or_insert(p) != p {
                return (2, 2);
            }
        }
        pair.unwrap_or((2, 2))
    }
}

// ── NLESIZEPOLICY (LEDGER SR394): a reel's size policy (handlers.md §5 NLEPROBE-HDR's NLE-SIZE-POLICY) ──────────────────────
// A reel whose frames change size mid-stream (AV1INBANDSIZE SR379) shows each frame in the reel's FIRST size; how is the
// reel's property, ONE per source, stored by the one EDL codec as `* UNA SIZE <reel> <policy>` (absent = Letterbox, so every
// EDL written before it is byte-identical). [`SizePolicy::place`] is the ONE geometry over yuv_core::scale's ONE `fit`; the
// editor's `view::Canvas::blit_policy` samples through yuv_core::scale's `nearest_rows`, Stria's render (`stria::nle`
// `reel_rgba`) and the kernel's EDL export (`video/export.rs` `Exp::sized`) both read the reel's policy from here.

/// How a frame of another size than its reel's first size is shown in the reel's box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SizePolicy {
    /// Aspect kept, centred, bars (SR384's rule): the default.
    #[default]
    Letterbox,
    /// Aspect kept, the box covered, the source's overflow cropped centred.
    Fill,
    /// The whole frame scaled to the whole box (aspect not kept).
    Stretch,
    /// The frame keeps its own size: honoured by a run whose sink takes any size, refused by name by an export (an mp4
    /// track holds one size). Placed into a box (a transition's mix), it places as [`SizePolicy::Letterbox`].
    Adopt,
}

/// Where a `sw x sh` frame goes in a `dw x dh` box: the source rect sampled and the box rect it fills.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    pub src: yuv_core::scale::Rect,
    pub dst: yuv_core::scale::Rect,
}

impl SizePolicy {
    /// NLESIZEPOLICYUI (SR402): every policy, in the face's cycle order (the ONE list `from_name` and `next` read).
    pub const ALL: [SizePolicy; 4] = [SizePolicy::Letterbox, SizePolicy::Fill, SizePolicy::Stretch, SizePolicy::Adopt];
    /// NLESIZEPOLICYUI (SR402): the policy after this one in [`SizePolicy::ALL`] (Adopt wraps to Letterbox): the face's cycle.
    pub fn next(self) -> SizePolicy {
        let i = SizePolicy::ALL.iter().position(|&p| p == self).unwrap_or(0);
        SizePolicy::ALL[(i + 1) % SizePolicy::ALL.len()]
    }
    /// The policy's word: the EDL's `* UNA SIZE` token and the wire's `fit=`.
    pub fn name(self) -> &'static str {
        match self {
            SizePolicy::Letterbox => "letterbox",
            SizePolicy::Fill => "fill",
            SizePolicy::Stretch => "stretch",
            SizePolicy::Adopt => "adopt",
        }
    }
    /// The policy a word names (ASCII case ignored).
    pub fn from_name(s: &str) -> Option<SizePolicy> {
        SizePolicy::ALL.into_iter().find(|p| p.name().eq_ignore_ascii_case(s))
    }
    /// THE GEOMETRY: a `sw x sh` frame in a `dw x dh` box (all four > 0). Letterbox: the whole frame into
    /// `yuv_core::scale::fit(sw, sh, dw, dh)`; Fill: the box's aspect centred inside the frame (`fit(dw, dh, sw, sh)`) into
    /// the whole box; Stretch: the whole frame into the whole box; Adopt: as Letterbox (see [`SizePolicy::Adopt`]).
    pub fn place(self, sw: usize, sh: usize, dw: usize, dh: usize) -> Place {
        let whole = |w, h| yuv_core::scale::Rect { w, h, x: 0, y: 0 };
        match self {
            SizePolicy::Letterbox | SizePolicy::Adopt => Place { src: whole(sw, sh), dst: yuv_core::scale::fit(sw, sh, dw, dh) },
            SizePolicy::Fill => Place { src: yuv_core::scale::fit(dw, dh, sw, sh), dst: whole(dw, dh) },
            SizePolicy::Stretch => Place { src: whole(sw, sh), dst: whole(dw, dh) },
        }
    }
}

impl Timeline {
    /// NLESIZEPOLICY (SR394): the same timeline with source `src`'s size policy `p` (the property's edit; the editor's `size`
    /// verb drives it, NLESIZEPOLICYUI SR402: `editor::EditorState::set_size_policy`). [`EditError::NoSource`] when the timeline holds no such source.
    pub fn set_size_policy(&self, src: SourceId, p: SizePolicy) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        tl.sources.iter_mut().find(|s| s.id == src).ok_or(EditError::NoSource)?.size_policy = p;
        Ok(tl)
    }
}

// ---- NLEWIPES (LEDGER SR679): the transition's easing curve and the SMPTE codes refused by name (named tail block) ----

/// NLEWIPES (LEDGER SR679): a transition's easing curve over the incoming's weight `t` (the schedule's [`schedule::Frac`]),
/// applied ONCE where the schedule makes `t` (`vmix_for`, `black_step`), so the dissolve weight and every wipe mask in
/// both callers of [`vmix::mix_step`] read the eased value. Exact rationals, no float. The EDL's `* UNA EASE` line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Ease {
    /// `t` itself (the default; no EDL line).
    #[default]
    Linear,
    /// `t^2` (`* UNA EASE in`).
    In,
    /// `1 - (1 - t)^2` (`* UNA EASE out`).
    Out,
    /// smoothstep `3 t^2 - 2 t^3` (`* UNA EASE inout`).
    InOut,
}

impl Ease {
    /// Every curve, in the EDL's word order.
    pub const ALL: [Ease; 4] = [Ease::Linear, Ease::In, Ease::Out, Ease::InOut];
    /// The eased weight of `t = num / den` as an exact fraction (Linear returns `t` unchanged, the same `Frac`).
    pub fn apply(self, t: schedule::Frac) -> schedule::Frac {
        let (n, d) = (t.num, t.den);
        match self {
            Ease::Linear => t,
            Ease::In => schedule::Frac { num: n * n, den: d * d },
            Ease::Out => schedule::Frac { num: 2 * n * d - n * n, den: d * d },
            Ease::InOut => schedule::Frac { num: 3 * n * n * d - 2 * n * n * n, den: d * d * d },
        }
    }
    /// The EDL word (`linear` is never written).
    pub fn name(self) -> &'static str {
        match self {
            Ease::Linear => "linear",
            Ease::In => "in",
            Ease::Out => "out",
            Ease::InOut => "inout",
        }
    }
    /// The curve a word names (ASCII case ignored).
    pub fn from_name(s: &str) -> Option<Ease> {
        Ease::ALL.into_iter().find(|e| e.name().eq_ignore_ascii_case(s))
    }
}

impl WipeDir {
    /// NLEWIPES (LEDGER SR679): the published SMPTE 258M codes (W3C SMIL 3.0 Transition Effects, Table 1) that
    /// [`WipeDir::from_code`] REFUSES, each by its SMIL type / subtype: the illustration fixes a parameter the name does not
    /// (where the boxes start, a vee's depth, a zig-zag's teeth, a polygon's proportions, a sweep's pivot, a matrix grid).
    pub const REFUSED: &'static [(u16, &'static str)] = &[
        (8, "fourBoxWipe cornersOut"),
        (48, "miscDiagonalWipe doubleDiamond"),
        (61, "veeWipe down"),
        (62, "veeWipe left"),
        (63, "veeWipe up"),
        (64, "veeWipe right"),
        (65, "barnVeeWipe down"),
        (66, "barnVeeWipe left"),
        (67, "barnVeeWipe up"),
        (68, "barnVeeWipe right"),
        (71, "zigZagWipe leftToRight"),
        (72, "zigZagWipe topToBottom"),
        (73, "barnZigZagWipe vertical"),
        (74, "barnZigZagWipe horizontal"),
        (103, "triangleWipe up"),
        (104, "triangleWipe right"),
        (105, "triangleWipe down"),
        (106, "triangleWipe left"),
        (107, "arrowHeadWipe up"),
        (108, "arrowHeadWipe right"),
        (109, "arrowHeadWipe down"),
        (110, "arrowHeadWipe left"),
        (111, "pentagonWipe up"),
        (112, "pentagonWipe down"),
        (113, "hexagonWipe horizontal"),
        (114, "hexagonWipe vertical"),
        (120, "ellipseWipe horizontal"),
        (121, "ellipseWipe vertical"),
        (122, "eyeWipe horizontal"),
        (123, "eyeWipe vertical"),
        (124, "roundRectWipe horizontal"),
        (125, "roundRectWipe vertical"),
        (127, "starWipe fourPoint"),
        (128, "starWipe fivePoint"),
        (129, "starWipe sixPoint"),
        (130, "miscShapeWipe heart"),
        (131, "miscShapeWipe keyhole"),
        (221, "singleSweepWipe clockwiseTop"),
        (222, "singleSweepWipe clockwiseRight"),
        (223, "singleSweepWipe clockwiseBottom"),
        (224, "singleSweepWipe clockwiseLeft"),
        (225, "doubleSweepWipe parallelVertical"),
        (226, "doubleSweepWipe parallelDiagonal"),
        (227, "doubleSweepWipe oppositeVertical"),
        (228, "doubleSweepWipe oppositeHorizontal"),
        (231, "fanWipe top"),
        (232, "fanWipe right"),
        (233, "fanWipe bottom"),
        (234, "fanWipe left"),
        (235, "doubleFanWipe fanInVertical"),
        (236, "doubleFanWipe fanInHorizontal"),
        (241, "singleSweepWipe clockwiseTopLeft"),
        (242, "singleSweepWipe counterClockwiseBottomLeft"),
        (243, "singleSweepWipe clockwiseBottomRight"),
        (244, "singleSweepWipe counterClockwiseTopRight"),
        (245, "doubleSweepWipe parallelDiagonalTopLeft"),
        (246, "doubleSweepWipe parallelDiagonalBottomLeft"),
        (251, "saloonDoorWipe top"),
        (252, "saloonDoorWipe left"),
        (253, "saloonDoorWipe bottom"),
        (254, "saloonDoorWipe right"),
        (261, "windshieldWipe right"),
        (262, "windshieldWipe up"),
        (263, "windshieldWipe vertical"),
        (264, "windshieldWipe horizontal"),
        (301, "snakeWipe topLeftHorizontal"),
        (302, "snakeWipe topLeftVertical"),
        (303, "snakeWipe topLeftDiagonal"),
        (304, "snakeWipe topRightDiagonal"),
        (305, "snakeWipe bottomRightDiagonal"),
        (306, "snakeWipe bottomLeftDiagonal"),
        (310, "spiralWipe topLeftClockwise"),
        (311, "spiralWipe topRightClockwise"),
        (312, "spiralWipe bottomRightClockwise"),
        (313, "spiralWipe bottomLeftClockwise"),
        (314, "spiralWipe topLeftCounterClockwise"),
        (315, "spiralWipe topRightCounterClockwise"),
        (316, "spiralWipe bottomRightCounterClockwise"),
        (317, "spiralWipe bottomLeftCounterClockwise"),
        (320, "parallelSnakesWipe verticalTopSame"),
        (321, "parallelSnakesWipe verticalBottomSame"),
        (322, "parallelSnakesWipe verticalTopLeftOpposite"),
        (323, "parallelSnakesWipe verticalBottomLeftOpposite"),
        (324, "parallelSnakesWipe horizontalLeftSame"),
        (325, "parallelSnakesWipe horizontalRightSame"),
        (326, "parallelSnakesWipe horizontalTopLeftOpposite"),
        (327, "parallelSnakesWipe horizontalTopRightOpposite"),
        (328, "parallelSnakesWipe diagonalBottomLeftOpposite"),
        (329, "parallelSnakesWipe diagonalTopLeftOpposite"),
        (340, "boxSnakesWipe twoBoxTop"),
        (341, "boxSnakesWipe twoBoxBottom"),
        (342, "boxSnakesWipe twoBoxLeft"),
        (343, "boxSnakesWipe twoBoxRight"),
        (344, "boxSnakesWipe fourBoxVertical"),
        (345, "boxSnakesWipe fourBoxHorizontal"),
        (350, "waterfallWipe verticalLeft"),
        (351, "waterfallWipe verticalRight"),
        (352, "waterfallWipe horizontalLeft"),
        (353, "waterfallWipe horizontalRight"),
    ];
    /// NLEWIPES: the SMIL type / subtype of a published code [`WipeDir::from_code`] refuses (`None`: built, or not in the table).
    pub fn refused(code: u16) -> Option<&'static str> {
        WipeDir::REFUSED.iter().find(|r| r.0 == code).map(|r| r.1)
    }
}

// ---- NLEXFADEPOWER (LEDGER SR678): the crossfade curve, the explicit audio-only crossfade and the BL audio fades (named tail block) ----
// A crossfade's weights come from ONE fn, `schedule::xfade_weight(curve, p)` over the linear phase. An audio cut's fade
// is read through ONE accessor, `Timeline::crossfade_at`: an explicit record (CMX's A-only `D` event) first, else the
// crossfade a video transition implies (NLE3b's pairing, linear), else a fade FROM silence under a video transition from
// black. `Timeline::fade_to_silence` is its to-black twin: the outgoing's handle fades out in the gap after the clip.

/// NLEXFADEPOWER: a crossfade's weight curve. `Linear` (the default, NLE3b's ramp: the weights sum to `ENV_ONE`) or
/// `EqualPower` (`w_in = sin(pi t / 2)`, `w_out = cos(pi t / 2)`: the squares sum to `ENV_ONE^2`); `* UNA XFADE EQUAL` in
/// the [`edl`], absent = linear.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Curve {
    #[default]
    Linear,
    EqualPower,
}

/// NLEXFADEPOWER: an explicit audio crossfade on the cut at tick `at` of audio track `track` (the outgoing clip ends
/// there, the incoming starts there), lasting `dur` ticks from the cut like a [`Transition`]: the outgoing's handle
/// `src_out..src_out + dur` under the falling weight, the incoming's first `dur` ticks under the rising one. Where a video
/// transition already implies a crossfade on that cut, the record overrides it (its `dur` and `curve`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioXfade {
    pub track: usize,
    pub at: i64,
    pub dur: i64,
    pub curve: Curve,
}

/// NLEXFADEPOWER: what the other side of an audio fade is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Partner {
    /// The outgoing clip's handle (a crossfade: two layers).
    Clip,
    /// Silence (a BL transition's audio: one layer, no partner to decode).
    Silence,
}

/// NLEXFADEPOWER: the fade on an audio cut, derived or explicit: over ticks `at..at + dur`, weighted by `curve`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Crossfade {
    pub at: i64,
    pub dur: i64,
    pub curve: Curve,
    pub partner: Partner,
}

impl Timeline {
    /// NLEXFADEPOWER: THE fade on the HEAD of clip `ci` of audio track `track`: an explicit [`AudioXfade`] on its cut,
    /// else the crossfade a video transition implies ([`Timeline::audio_crossfade`]'s pairing, linear), else, when no
    /// clip ends at its start, a fade FROM silence under a video transition from black whose incoming clip has the same
    /// source and starts there (linear). `None`: a plain start.
    pub fn crossfade_at(&self, track: usize, ci: usize) -> Option<Crossfade> {
        let t = self.tracks.get(track)?;
        if t.kind != Kind::Audio {
            return None;
        }
        let i = t.clips.get(ci)?;
        if ci > 0 && t.clips[ci - 1].end() == i.at {
            if let Some(x) = self.audio_xfades.iter().find(|x| x.track == track && x.at == i.at) {
                return Some(Crossfade { at: x.at, dur: x.dur, curve: x.curve, partner: Partner::Clip });
            }
            return self.crossfade_of(&t.clips, ci).map(|x| Crossfade { at: x.at, dur: x.dur, curve: Curve::Linear, partner: Partner::Clip });
        }
        self.black_pair(i.at, i.src, true).map(|dur| Crossfade { at: i.at, dur, curve: Curve::Linear, partner: Partner::Silence })
    }

    /// NLEXFADEPOWER: the fade TO silence after clip `ci` of audio track `track`: no clip starts at its end, and a video
    /// transition to black sits there whose outgoing clip has the same source and ends there. The clip's handle plays on
    /// in the gap `at..at + dur` under the falling weight (linear).
    pub fn fade_to_silence(&self, track: usize, ci: usize) -> Option<Crossfade> {
        let t = self.tracks.get(track)?;
        if t.kind != Kind::Audio {
            return None;
        }
        let o = t.clips.get(ci)?;
        if t.clips.iter().any(|c| c.at == o.end()) {
            return None;
        }
        self.black_pair(o.end(), o.src, false).map(|dur| Crossfade { at: o.end(), dur, curve: Curve::Linear, partner: Partner::Silence })
    }

    /// The duration of the video transition from black (`from`) whose incoming clip of source `src` starts at `at`, or to
    /// black whose outgoing clip of source `src` ends at `at`.
    fn black_pair(&self, at: i64, src: SourceId, from: bool) -> Option<i64> {
        self.tracks.iter().filter(|v| v.kind == Kind::Video).find_map(|v| {
            let x = v.transitions.iter().find(|x| x.at == at && if from { x.from_black } else { x.to_black })?;
            let clip = v.clips.iter().any(|c| c.src == src && if from { c.at == at } else { c.end() == at });
            clip.then_some(x.dur)
        })
    }

    /// NLEXFADEPOWER: put an explicit audio crossfade on the cut at tick `at` of audio track `track` (replacing one there).
    pub fn add_audio_xfade(&self, track: usize, at: i64, dur: i64, curve: Curve) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        tl.tracks.get(track).ok_or(EditError::NoTrack)?;
        tl.audio_xfades.retain(|x| !(x.track == track && x.at == at));
        tl.audio_xfades.push(AudioXfade { track, at, dur, curve });
        tl.audio_xfades.sort_by_key(|x| (x.track, x.at));
        tl.validate()?;
        Ok(tl)
    }

    /// NLEXFADEPOWER: take the explicit audio crossfade off the cut at tick `at` of `track` (none there: `Transition`).
    pub fn remove_audio_xfade(&self, track: usize, at: i64) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        let n = tl.audio_xfades.len();
        tl.audio_xfades.retain(|x| !(x.track == track && x.at == at));
        if tl.audio_xfades.len() == n {
            return Err(EditError::Transition);
        }
        tl.validate()?;
        Ok(tl)
    }

    /// NLEXFADEPOWER: every explicit record is in `(track, at)` order on a cut of an audio track (`Transition`) and fits the
    /// incoming clip and the outgoing source's handle (`Handle`); a BL fade's audio fits its clip (from black) or its
    /// handle (to black) (`Handle`, NLE3b's rule for the paired crossfade).
    fn validate_audio_xfades(&self) -> Result<(), EditError> {
        let mut last: Option<(usize, i64)> = None;
        for x in &self.audio_xfades {
            let t = self.tracks.get(x.track).ok_or(EditError::NoTrack)?;
            if t.kind != Kind::Audio || x.dur < 1 || last.is_some_and(|l| (x.track, x.at) <= l) {
                return Err(EditError::Transition);
            }
            last = Some((x.track, x.at));
            let (Some(o), Some(i)) = (t.clips.iter().find(|c| c.end() == x.at), t.clips.iter().find(|c| c.at == x.at)) else {
                return Err(EditError::Transition);
            };
            let handle = self.source(o.src).ok_or(EditError::NoSource)?.duration - o.src_out;
            if x.dur > i.len() || x.dur > handle {
                return Err(EditError::Handle);
            }
        }
        for (ti, t) in self.tracks.iter().enumerate().filter(|(_, t)| t.kind == Kind::Audio) {
            for (ci, c) in t.clips.iter().enumerate() {
                if self.crossfade_at(ti, ci).is_some_and(|x| x.partner == Partner::Silence && x.dur > c.len()) {
                    return Err(EditError::Handle);
                }
                if let Some(x) = self.fade_to_silence(ti, ci) {
                    let handle = self.source(c.src).ok_or(EditError::NoSource)?.duration - c.src_out;
                    if x.dur > handle {
                        return Err(EditError::Handle);
                    }
                }
            }
        }
        Ok(())
    }

    /// NLEXFADEPOWER: after an op moved clips, move every explicit record of `before` to where its cut now is, by the same
    /// matcher as the video transitions ([`moved_cut`]); a cut that is gone is refused by name (`Transition`), never dropped.
    fn retarget_audio_xfades(&mut self, before: &Timeline) -> Result<(), EditError> {
        let mut moved = Vec::with_capacity(before.audio_xfades.len());
        for x in &before.audio_xfades {
            let bt = before.tracks.get(x.track).ok_or(EditError::NoTrack)?;
            let clips = &self.tracks.get(x.track).ok_or(EditError::NoTrack)?.clips;
            moved.push(AudioXfade { at: moved_cut(bt, clips, x.at)?, ..*x });
        }
        moved.sort_by_key(|x| (x.track, x.at));
        self.audio_xfades = moved;
        Ok(())
    }
}

/// NLE3E (LEDGER SR225), lifted here by NLEXFADEPOWER: where the cut at tick `at` of track `bt` (before an op) sits among
/// the op's `clips`. A cut is known by its clips' SOURCE edges: the incoming's `(src, src_in)` and the outgoing's
/// `(src, src_out)` (from black: no clip ends at the cut). The match nearest the old tick wins (ties: the earlier); none is
/// the cut gone, `EditError::Transition`.
fn moved_cut(bt: &Track, clips: &[Clip], at: i64) -> Result<i64, EditError> {
    let inc = bt.clips.iter().find(|c| c.at == at).ok_or(EditError::Transition)?;
    let out = bt.clips.iter().find(|c| c.end() == at);
    clips
        .iter()
        .filter(|i| i.src == inc.src && i.src_in == inc.src_in)
        .map(|i| i.at)
        .filter(|&a| match (out, clips.iter().find(|c| c.end() == a)) {
            (Some(p), Some(q)) => p.src == q.src && p.src_out == q.src_out,
            (None, None) => true,
            _ => false,
        })
        .min_by_key(|&a| ((a - at).abs(), a))
        .ok_or(EditError::Transition)
}

// ---- NLE3ETRIMSTRETCH (LEDGER SR692): the stretching trim, the ripple trims and the transitioned pair's move (named tail block) ----
// A trim that keeps a transitioned cut but eats its room (the incoming's length, the outgoing source's handle) shrinks the
// transition to fit (`fit_trimmed`, never below 1, never growing back on its own) instead of refusing; the explicit audio
// crossfade on the trimmed audio clip's edges fits the same way, and the derived one reads the video transition's `dur`.
// A RIPPLE trim keeps the clip's start (so the cut at its head is kept) and shifts every later clip of its track, with that
// track's transitions and audio crossfades, by the length change (`ripple_delete`'s shape: no cut is made or lost and no
// clip changes order, so nothing needs the source-edge matcher). `move_pair` moves a clip and the clip across its
// transition as ONE `move_many` transaction and carries the transition through `retarget_transitions`. `validate` stays
// the one gate.
impl Timeline {
    /// NLE3ETRIMSTRETCH: shrink every transition (and explicit audio crossfade) of `track` on clip `idx`'s two edges whose
    /// `dur` no longer fits its room (the incoming clip's length, the outgoing source's handle past `src_out`) to that
    /// room. A transition grows back toward its asked length (`fit_want`, NLE3ETRIMGROW SR712); an explicit audio crossfade
    /// shrinks only; a room under 1 is left for `validate` to refuse (`Handle`, as before).
    fn fit_trimmed(&mut self, track: usize, idx: usize) {
        let Some(c) = self.tracks.get(track).and_then(|t| t.clips.get(idx)).copied() else { return };
        let room = |tl: &Timeline, at: i64| -> Option<i64> {
            let t = tl.tracks.get(track)?;
            let inc = t.clips.iter().find(|i| i.at == at).map(|i| i.len());
            let out = t.clips.iter().find(|o| o.end() == at).and_then(|o| tl.source(o.src).map(|s| s.duration - o.src_out));
            match (inc, out) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            }
        };
        let rooms = [(c.at, room(self, c.at)), (c.end(), room(self, c.end()))];
        let fit = |dur: &mut i64, at: i64| {
            if let Some(&(_, Some(r))) = rooms.iter().find(|e| e.0 == at) {
                if *dur > r && r >= 1 {
                    *dur = r;
                }
            }
        };
        for x in self.tracks[track].transitions.iter_mut() {
            fit_want(x, &rooms); // NLE3ETRIMGROW (SR712): grow back toward the ask
        }
        for x in self.audio_xfades.iter_mut().filter(|x| x.track == track) {
            fit(&mut x.dur, x.at);
        }
    }

    /// NLE3ETRIMSTRETCH: move clip `idx`'s in point to source tick `src_in` keeping its timeline start (the cut at its head
    /// is kept); every later clip of the track moves by the length change.
    pub fn ripple_trim_in(&self, track: usize, idx: usize, src_in: i64) -> Result<Timeline, EditError> {
        let c = *self.tracks.get(track).ok_or(EditError::NoTrack)?.clips.get(idx).ok_or(EditError::NoClip)?;
        self.ripple_trim(track, idx, Clip { src_in, ..c })
    }

    /// NLE3ETRIMSTRETCH: move clip `idx`'s out point to source tick `src_out`; every later clip of the track moves by the
    /// length change, so the cut at its end is kept.
    pub fn ripple_trim_out(&self, track: usize, idx: usize, src_out: i64) -> Result<Timeline, EditError> {
        let c = *self.tracks.get(track).ok_or(EditError::NoTrack)?.clips.get(idx).ok_or(EditError::NoClip)?;
        self.ripple_trim(track, idx, Clip { src_out, ..c })
    }

    /// The one body of the two ripple trims: clip `idx` becomes `now` (same `at`), and every other clip, transition and
    /// explicit audio crossfade of `track` at or past its old end shifts by the length change; then the fit and `validate`.
    fn ripple_trim(&self, track: usize, idx: usize, now: Clip) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        let tr = &mut tl.tracks[track];
        let was = tr.clips[idx];
        let (edge, delta) = (was.end(), now.len() - was.len());
        tr.clips[idx] = now;
        for (i, c) in tr.clips.iter_mut().enumerate() {
            if i != idx && c.at >= edge {
                c.at += delta;
            }
        }
        for x in tr.transitions.iter_mut().filter(|x| x.at >= edge) {
            x.at += delta;
        }
        for x in tl.audio_xfades.iter_mut().filter(|x| x.track == track && x.at >= edge) {
            x.at += delta;
        }
        tl.fit_trimmed(track, idx);
        tl.validate()?;
        Ok(tl)
    }

    /// NLE3ETRIMSTRETCH: move clip `idx` of `track` and its partner (the clip across the two-clip transition at its out
    /// edge, else at its in edge; none: `Transition`) by `delta` ticks as ONE [`Timeline::move_many`] transaction (an
    /// occupied span or a negative `at` refuses it by name); the transition goes with them (`retarget_transitions`), and a
    /// third clip's transitioned cut left behind is refused by name.
    pub fn move_pair(&self, track: usize, idx: usize, delta: i64) -> Result<Timeline, EditError> {
        let t = self.tracks.get(track).ok_or(EditError::NoTrack)?;
        let c = t.clips.get(idx).ok_or(EditError::NoClip)?;
        let pair = |at: i64| t.transitions.iter().any(|x| x.at == at && !x.from_black && !x.to_black);
        let partner = if pair(c.end()) {
            t.clips.iter().position(|p| p.at == c.end())
        } else if pair(c.at) {
            t.clips.iter().position(|p| p.end() == c.at)
        } else {
            None
        };
        let partner = partner.ok_or(EditError::Transition)?;
        let mut bare = self.clone();
        for tr in &mut bare.tracks {
            tr.transitions.clear();
        }
        bare.audio_xfades.clear();
        let mut tl = bare.move_many(&[(track, idx), (track, partner)], delta)?;
        tl.retarget_transitions(self)?;
        tl.validate()?;
        Ok(tl)
    }
}

// ---- NLE3ESLIP (LEDGER SR704): slip and slide (named tail block) ----
// A SLIP slides the clip's source window under a fixed timeline position and length: no timeline tick moves, so its
// transitions and audio crossfades stay where they are (the source-edge matcher `moved_cut` is not run: the slip changes
// the very edges it keys on); a slip that eats the outgoing handle under its out-edge transition is refused `Handle` by
// `validate` (no fit on a slip). A SLIDE moves the clip along its track while the previous clip's out edge and the next
// clip's in edge follow (trim_out's and trim_in's rules), so the track's length and the clip's content stay; the two
// touched cuts' transitions and explicit audio crossfades shift by the same delta (`ripple_trim`'s direct shift: the
// matcher would refuse them, both cuts' source edges change), then `fit_trimmed` fits them to their room and ONE
// `validate` judges. A neighbour whose source cannot follow is `OutOfSource` (`in_source`, the window rule at both ends);
// `Handle` stays the transition's room. `validate` stays the one gate.
impl Timeline {
    /// NLE3ESLIP: slip clip `idx` of `track` by `delta` source ticks: `src_in` and `src_out` both move, `at` and the
    /// length stay. Refused `OutOfSource` when the window leaves the source at either end.
    pub fn slip(&self, track: usize, idx: usize, delta: i64) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        let c = clip_mut(&mut tl, track, idx)?;
        c.src_in = c.src_in.saturating_add(delta);
        c.src_out = c.src_out.saturating_add(delta);
        let now = *c;
        tl.in_source(now)?;
        tl.validate()?;
        Ok(tl)
    }

    /// NLE3ESLIP: slide clip `idx` of `track` by `delta` timeline ticks: the butted previous clip's out point and the
    /// butted next clip's in point follow (a side with no butted neighbour is a plain move there, judged `Overlap` /
    /// `Negative` as `move_clip` is); the transitions and explicit audio crossfades on the two cuts move with them and fit.
    pub fn slide(&self, track: usize, idx: usize, delta: i64) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        let c = *clip_mut(&mut tl, track, idx)?;
        let (cut_in, cut_out) = (c.at, c.end());
        let tr = &mut tl.tracks[track];
        if let Some(p) = idx.checked_sub(1).and_then(|i| tr.clips.get_mut(i)).filter(|p| p.end() == cut_in) {
            p.src_out = p.src_out.saturating_add(delta);
        }
        if let Some(n) = tr.clips.get_mut(idx + 1).filter(|n| n.at == cut_out) {
            n.at = n.at.saturating_add(delta);
            n.src_in = n.src_in.saturating_add(delta);
        }
        tr.clips[idx].at = cut_in.saturating_add(delta);
        for x in tr.transitions.iter_mut().filter(|x| x.at == cut_in || x.at == cut_out) {
            x.at = x.at.saturating_add(delta);
        }
        for x in tl.audio_xfades.iter_mut().filter(|x| x.track == track && (x.at == cut_in || x.at == cut_out)) {
            x.at = x.at.saturating_add(delta);
        }
        for i in [idx.checked_sub(1), Some(idx + 1)].into_iter().flatten() {
            if let Some(&n) = tl.tracks[track].clips.get(i) {
                tl.in_source(n)?;
            }
        }
        tl.fit_trimmed(track, idx);
        tl.validate()?;
        Ok(tl)
    }

    /// NLE3ESLIP: the one window judge of slip and slide: `OutOfSource` when `c`'s source window leaves its source at
    /// either end (`src_in < 0` or `src_out` past the duration).
    fn in_source(&self, c: Clip) -> Result<(), EditError> {
        let s = self.source(c.src).ok_or(EditError::NoSource)?;
        if c.src_in < 0 || c.src_out > s.duration {
            return Err(EditError::OutOfSource);
        }
        Ok(())
    }
}

// ---- NLE3ETRIMGROW (LEDGER SR712): a fitted transition grows back to its asked length (named tail block) ----
// `fit_trimmed` fits each transition on the trimmed clip's edges to that edge's room through ONE rule: the ask is `want`
// when the transition is a fit, else its `dur`; `dur` becomes min(ask, room) and `want` is kept only while `dur` is below
// the ask. A shrink records the ask, a freed room grows the transition back and never past it, and at the ask `want`
// clears, so a trim and its trim-back give the original Timeline. An explicit ask (`add_transition*`, so the editor's
// `set_transition_dur`) carries `want: None`; a room under 1 is left for `validate` (`Handle`).

/// NLE3ETRIMGROW: fit `x` to the room recorded for its cut in `rooms` (none, or under 1: unchanged).
fn fit_want(x: &mut Transition, rooms: &[(i64, Option<i64>)]) {
    let Some(&(_, Some(r))) = rooms.iter().find(|e| e.0 == x.at) else { return };
    if r < 1 {
        return;
    }
    let ask = x.want.unwrap_or(x.dur);
    x.dur = ask.min(r);
    x.want = (x.dur < ask).then_some(ask);
}

// ---- NLE5DRANGESAVE (LEDGER SR723): the export range's marks on the Timeline, the one in < out rule (named tail block) ----

impl Timeline {
    /// The timeline with its export marks `(in, out)` set (either may be `None`). Refused: a negative mark
    /// (`EditError::Negative`); both set with `in >= out` (`EditError::Marks`). The EDL reader and the editor both call it.
    pub fn set_marks(&self, m: (Option<i64>, Option<i64>)) -> Result<Timeline, EditError> {
        if m.0.is_some_and(|t| t < 0) || m.1.is_some_and(|t| t < 0) {
            return Err(EditError::Negative);
        }
        if let (Some(a), Some(b)) = m {
            if a >= b {
                return Err(EditError::Marks);
            }
        }
        let mut tl = self.clone();
        tl.marks = m;
        Ok(tl)
    }
}

// ---- STRIATRACKMUTESOLO (LEDGER SR837): per-track mute and solo, the ONE active-track rule (named tail block) ----
// `Track::muted` / `Track::solo` are plain fields of the Timeline value (undo, `dirty`, save follow). The audible / visible
// set is ONE fn, [`Timeline::active_tracks`]: if any track is solo, the solo tracks; else every track not muted (solo beats
// mute; all muted: none). `schedule::schedule` (the video winner, a to-black above it) and `schedule::audio_runs` (the
// mixed tracks) read it, so Stria's render, preview, play path and export, and the kernel Player's EDL mode, follow it;
// `view::paint_in` dims every inactive row and badges the flags; the EDL carries `* UNA MUTE` / `* UNA SOLO`.
impl Timeline {
    /// The active tracks' indices, ascending (see the block comment). No allocation: callers may ask per tick.
    pub fn active_tracks(&self) -> impl DoubleEndedIterator<Item = usize> + '_ {
        let any_solo = self.tracks.iter().any(|t| t.solo);
        (0..self.tracks.len()).filter(move |&i| if any_solo { self.tracks[i].solo } else { !self.tracks[i].muted })
    }
    /// The timeline with track `track`'s mute and solo flags set (`EditError::NoTrack` for no such track).
    pub fn set_track_flags(&self, track: usize, muted: bool, solo: bool) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        let t = tl.tracks.get_mut(track).ok_or(EditError::NoTrack)?;
        (t.muted, t.solo) = (muted, solo);
        Ok(tl)
    }
}

#[cfg(test)]
mod striatrackmutesolo_tests {
    use super::*;

    fn four() -> Timeline {
        let mut tl = Timeline::new(25);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        tl.add_track(Kind::Audio);
        tl
    }
    fn act(tl: &Timeline) -> Vec<usize> {
        tl.active_tracks().collect()
    }

    /// M3: none flagged -> all; one muted -> the rest; one solo -> only it; solo beats mute; all muted -> none; the
    /// iterator runs backwards too (the schedule's top-down search).
    #[test]
    fn striatrackmutesolo_active_tracks_kats() {
        let tl = four();
        assert_eq!(act(&tl), alloc::vec![0, 1, 2, 3]);
        let m = tl.set_track_flags(2, true, false).unwrap();
        assert_eq!(act(&m), alloc::vec![0, 1, 3]);
        let s = tl.set_track_flags(1, false, true).unwrap();
        assert_eq!(act(&s), alloc::vec![1]);
        let both = m.set_track_flags(2, true, true).unwrap();
        assert_eq!(act(&both), alloc::vec![2], "solo beats mute");
        let mut all = tl.clone();
        for i in 0..4 {
            all = all.set_track_flags(i, true, false).unwrap();
        }
        assert!(act(&all).is_empty());
        assert_eq!(m.active_tracks().rev().collect::<Vec<_>>(), alloc::vec![3, 1, 0]);
        assert_eq!(tl.set_track_flags(4, true, false), Err(EditError::NoTrack));
    }

    /// Every frame a keyframe, lengths unknown.
    struct AllKeys;
    impl SourceIndex for AllKeys {
        fn duration(&self, _: SourceId) -> Option<i64> {
            None
        }
        fn keyframe_ns(&self, _: SourceId, _: Kind, t_ns: i64) -> Option<i64> {
            Some(t_ns)
        }
    }

    /// M3 (the seat's pin): the SHARED schedule follows the rule, so the kernel Player's EDL mode does too: V2 muted over
    /// V1 -> every video step names V1's source; V1 alone muted -> gaps; every audio track muted -> `audio_runs` still
    /// covers the range with runs that have no layers (silence), and the schedule keeps one audio header per run.
    #[test]
    fn striatrackmutesolo_schedule_follows_the_rule() {
        let mut tl = Timeline::new(25);
        let (a, b) = (tl.add_source("A", 50), tl.add_source("B", 50));
        let (v1, v2) = (tl.add_track(Kind::Video), tl.add_track(Kind::Video));
        let (a1, a2) = (tl.add_track(Kind::Audio), tl.add_track(Kind::Audio));
        tl.tracks[v1].clips = alloc::vec![Clip::new(a, 0, 10, 0)];
        tl.tracks[v2].clips = alloc::vec![Clip::new(b, 0, 10, 0)];
        tl.tracks[a1].clips = alloc::vec![Clip::new(a, 0, 10, 0)];
        tl.tracks[a2].clips = alloc::vec![Clip::new(b, 0, 10, 0)];
        let video = |tl: &Timeline| -> Vec<(Option<usize>, Option<SourceId>)> {
            schedule::schedule(tl, 0..10, &AllKeys).unwrap().iter().filter(|s| s.kind == Kind::Video).map(|s| (s.track, s.src)).collect()
        };
        assert!(video(&tl).iter().all(|&v| v == (Some(v2), Some(b))), "unflagged: the top track wins");
        let m = tl.set_track_flags(v2, true, false).unwrap();
        assert!(video(&m).iter().all(|&v| v == (Some(v1), Some(a))), "V2 muted: the lower track shows");
        let low = tl.set_track_flags(v2, false, false).unwrap().set_track_flags(v1, false, true).unwrap();
        assert!(video(&low).iter().all(|&v| v == (Some(v1), Some(a))), "V1 solo: it shows");
        let runs = schedule::audio_runs(&tl.set_track_flags(a1, true, false).unwrap(), 0..10, &AllKeys).unwrap();
        assert!(runs.iter().all(|r| r.layers.len() == 1 && r.layers[0].track == a2), "A1 muted: A2's layer alone");
        let mut mute = tl.set_track_flags(a1, true, false).unwrap().set_track_flags(a2, true, false).unwrap();
        let runs = schedule::audio_runs(&mute, 0..10, &AllKeys).unwrap();
        assert_eq!(runs.iter().map(|r| r.len).sum::<i64>(), 10);
        assert!(runs.iter().all(|r| r.layers.is_empty()), "every audio track muted: silence");
        let audio = schedule::schedule(&mute, 0..10, &AllKeys).unwrap().iter().filter(|s| s.kind == Kind::Audio).count();
        assert_eq!(audio, runs.len());
        mute = mute.set_track_flags(v1, true, false).unwrap().set_track_flags(v2, true, false).unwrap();
        assert!(video(&mute).iter().all(|&v| v == (None, None)), "every video track muted: black steps, one per tick");
        assert_eq!(video(&mute).len(), 10);
    }
}

// ---- STRIATRACKGAIN (LEDGER SR864): the per-track gain and the one heard-gain sum (named tail block) ----
// `Track::gain_cdb` is a plain field of the Timeline value (undo, `dirty`, save follow), in the clip range
// (`GAIN_MIN_CDB..=GAIN_MAX_CDB`, `validate`'s `EditError::Gain`). THE RULE: a layer is heard at its clip's gain PLUS its
// track's ([`Timeline::heard_cdb`]: centi-dB add = linear gains multiply; no clamp, the sum is the true product).
// `schedule::audio_runs` builds every `Layer.gain_cdb` through it (both of its Layer sites), so Stria's render, preview,
// meters and export (`mix_run` / `mix_into`) AND the kernel Player's EDL mode (`vplay.rs` `layer_q8`) follow with no mix
// edit; a track at 0 dB gives the runs it gave before.
impl Timeline {
    /// The timeline with track `track`'s gain set to `cdb` centi-dB (`EditError::NoTrack` for no such track,
    /// `EditError::Gain` outside the clip range).
    pub fn set_track_gain(&self, track: usize, cdb: i32) -> Result<Timeline, EditError> {
        if !(GAIN_MIN_CDB..=GAIN_MAX_CDB).contains(&cdb) {
            return Err(EditError::Gain);
        }
        let mut tl = self.clone();
        tl.tracks.get_mut(track).ok_or(EditError::NoTrack)?.gain_cdb = cdb;
        Ok(tl)
    }
    /// The gain a clip of gain `clip_cdb` on track `track` is heard at: the clip's plus the track's (0 for no such track).
    pub fn heard_cdb(&self, track: usize, clip_cdb: i32) -> i32 {
        clip_cdb + self.tracks.get(track).map_or(0, |t| t.gain_cdb)
    }
}

#[cfg(test)]
mod striatrackgain_tests {
    use super::*;

    /// M3: set_track_gain sets one track; refused NoTrack / Gain by name (the state untouched); validate refuses a raw
    /// out-of-range field; heard_cdb is the sum.
    #[test]
    fn striatrackgain_set_and_refusals() {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        let t = tl.add_track(Kind::Audio);
        tl.tracks[t].clips = alloc::vec![Clip::new(a, 0, 10, 0).gained(-300)];
        assert_eq!(tl.tracks[t].gain_cdb, 0);
        let g = tl.set_track_gain(t, -602).unwrap();
        assert_eq!((g.tracks[t].gain_cdb, g.tracks[t].clips.clone(), g.heard_cdb(t, -300)), (-602, tl.tracks[t].clips.clone(), -902));
        assert_eq!(tl.set_track_gain(1, -602), Err(EditError::NoTrack));
        assert_eq!(tl.set_track_gain(t, GAIN_MAX_CDB + 1), Err(EditError::Gain));
        assert_eq!(tl.set_track_gain(t, GAIN_MIN_CDB - 1), Err(EditError::Gain));
        assert!(tl.set_track_gain(t, GAIN_MIN_CDB).is_ok() && tl.set_track_gain(t, GAIN_MAX_CDB).is_ok());
        let mut raw = tl.clone();
        raw.tracks[t].gain_cdb = GAIN_MAX_CDB + 1;
        assert_eq!(raw.validate(), Err(EditError::Gain));
        assert_eq!(tl.heard_cdb(7, -300), -300, "no such track: the clip's own");
    }
}

// ---- STRIATRACKGAINKEYSNEXT (LEDGER SR983): the track's channel word both ways, ONE name rule (named tail block) ----
// A track as a person names it is its kind's letter and its 1-based number within the kind (`V1`, `A2`):
// [`Timeline::track_name`] is THE spelling (the face's message lines read it). [`Timeline::track_named`] is its inverse
// over the EDL's channel parser (`V` / `V1`..`V9`, `A` / `A1`..`A9`, uppercase only): the nth track of that kind.
impl Timeline {
    /// Track `ti`'s name (`V1`, `A2`); `None` for no such track.
    pub fn track_name(&self, ti: usize) -> Option<String> {
        let k = if self.tracks.get(ti)?.kind == Kind::Video { "V" } else { "A" };
        Some(alloc::format!("{k}{}", self.track_number(ti)?))
    }
    /// The index of the track a channel word names; `None` for a word that is not a channel or names no track.
    pub fn track_named(&self, word: &str) -> Option<usize> {
        let (kind, num) = edl::channel(word)?;
        (0..self.tracks.len()).filter(|&t| self.tracks[t].kind == kind).nth(num - 1)
    }
}

#[cfg(test)]
mod striatrackgainkeysnext_tests {
    use super::*;

    /// M3: interleaved tracks name and parse back; `V` is `V1`; a bad word or a missing track is `None`.
    #[test]
    fn striatrackgainkeysnext_track_name_round_trip() {
        let mut tl = Timeline::new(25);
        for k in [Kind::Video, Kind::Audio, Kind::Video, Kind::Audio] {
            tl.add_track(k);
        }
        let names: Vec<String> = (0..4).map(|t| tl.track_name(t).unwrap()).collect();
        assert_eq!(names, ["V1", "A1", "V2", "A2"]);
        for t in 0..4 {
            assert_eq!(tl.track_named(&names[t]), Some(t));
        }
        assert_eq!((tl.track_named("V"), tl.track_named("A")), (Some(0), Some(1)));
        for bad in ["V3", "A9", "a2", "X1", "", "V10", "A 2"] {
            assert_eq!(tl.track_named(bad), None, "{bad}");
        }
        assert_eq!(tl.track_name(4), None);
    }
}

// ---- STRIAGAINENVELOPE (LEDGER SR9047): a clip's gain keyframes on its source ticks (named tail block) ----
// A clip carries up to [`GAIN_ENV_MAX`] points `(source tick, centi-dB offset)`, ticks strictly increasing, each offset in
// `GAIN_MIN_CDB..=GAIN_MAX_CDB`. The heard gain at source tick `s` is `gain_cdb + gain_env.at(s)` (plus the track's):
// linear in centi-dB between two points, held before the first and after the last, 0 when empty. The points sit on
// SOURCE ticks so a split, trim or move (update syntax copies the field) keeps them on the same audio. A fixed array keeps
// `Clip` `Copy`; the fields are private so the invariant lives in [`GainEnv::with`] / [`GainEnv::without`].

/// STRIAGAINENVELOPE (SR9047): the most points one clip's envelope holds.
pub const GAIN_ENV_MAX: usize = 8;

/// STRIAGAINENVELOPE (SR9047): a clip's gain envelope (see the block comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct GainEnv {
    n: u8,
    pts: [(i64, i32); GAIN_ENV_MAX],
    curve: Curve, // STRIAGAINENVCURVE (SR9083): the segments' shape (Linear when empty; `curve()` / `curved()`)
}

impl GainEnv {
    /// No point: the flat gain.
    pub const EMPTY: GainEnv = GainEnv { n: 0, pts: [(0, 0); GAIN_ENV_MAX], curve: Curve::Linear };
    /// The points, in tick order.
    pub fn points(&self) -> &[(i64, i32)] {
        &self.pts[..self.n as usize]
    }
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
    /// The same envelope with a point at source tick `src_t` of `cdb` (the point already there takes the value). Refused:
    /// `Negative` for a negative tick, `Gain` for an offset out of range or a full envelope.
    pub fn with(self, src_t: i64, cdb: i32) -> Result<GainEnv, EditError> {
        if src_t < 0 {
            return Err(EditError::Negative);
        }
        if !(GAIN_MIN_CDB..=GAIN_MAX_CDB).contains(&cdb) {
            return Err(EditError::Gain);
        }
        let mut e = self;
        let n = e.n as usize;
        let i = e.pts[..n].iter().position(|p| p.0 >= src_t).unwrap_or(n);
        if i < n && e.pts[i].0 == src_t {
            e.pts[i].1 = cdb;
            return Ok(e);
        }
        if n == GAIN_ENV_MAX {
            return Err(EditError::Gain);
        }
        let mut j = n;
        while j > i {
            e.pts[j] = e.pts[j - 1];
            j -= 1;
        }
        e.pts[i] = (src_t, cdb);
        e.n += 1;
        Ok(e)
    }
    /// The same envelope without its point at source tick `src_t`; `Gain` when no point sits there.
    pub fn without(self, src_t: i64) -> Result<GainEnv, EditError> {
        let n = self.n as usize;
        let i = self.pts[..n].iter().position(|p| p.0 == src_t).ok_or(EditError::Gain)?;
        let mut e = self;
        for j in i..n - 1 {
            e.pts[j] = e.pts[j + 1];
        }
        e.pts[n - 1] = (0, 0);
        e.n -= 1; if e.n == 0 { e.curve = Curve::Linear; } // STRIAGAINENVCURVE (SR9083): no point is always linear
        Ok(e)
    }
    /// The offset (centi-dB) at source tick `src_t`: linear between points (truncating toward zero), held at both ends,
    /// 0 when empty.
    pub fn at(&self, src_t: i64) -> i32 {
        let p = self.points();
        let (Some(first), Some(last)) = (p.first(), p.last()) else { return 0 };
        if src_t <= first.0 {
            return first.1;
        }
        if src_t >= last.0 {
            return last.1;
        }
        let i = p.iter().rposition(|q| q.0 <= src_t).unwrap_or(0);
        let ((s0, v0), (s1, v1)) = (p[i], p[i + 1]);
        self.seg_at(v0, v1, src_t - s0, s1 - s0) // STRIAGAINENVCURVE (SR9083): the segment through the envelope's curve (file tail)
    }
    /// The largest offset (0 when empty).
    pub fn peak(&self) -> i32 {
        self.points().iter().map(|p| p.1).max().unwrap_or(0)
    }
    /// True when source tick `src_t` lies in a RAMPING segment: between two neighbouring points of different value
    /// (from the first's tick, up to but not including the second's).
    pub fn ramps(&self, src_t: i64) -> bool {
        self.points().windows(2).any(|w| w[0].0 <= src_t && src_t < w[1].0 && w[0].1 != w[1].1)
    }
}

impl Clip {
    /// STRIAGAINENVELOPE (SR9047): the source tick heard at timeline tick (edge) `t` (past the ends too).
    pub fn src_tick(&self, t: i64) -> i64 {
        self.src_in + (t - self.at)
    }
}

impl Timeline {
    /// STRIAGAINENVELOPE (SR9047): set a clip's gain envelope.
    pub fn set_gain_env(&self, track: usize, idx: usize, env: GainEnv) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        clip_mut(&mut tl, track, idx)?.gain_env = env;
        tl.validate()?;
        Ok(tl)
    }
}

#[cfg(test)]
mod striagainenvelope_tests {
    use super::*;

    /// M3: points insert sorted, a point at the same tick takes the new value, removal and refusals by name; `at` is
    /// linear in cdB between points and held at both ends; `peak` / `ramps`; a new clip's envelope is empty.
    #[test]
    fn striagainenvelope_points_and_value() {
        assert!(Clip::new(SourceId(0), 0, 10, 0).gain_env.is_empty());
        assert_eq!(GainEnv::default(), GainEnv::EMPTY);
        let e = GainEnv::EMPTY.with(20, 0).unwrap().with(10, -600).unwrap();
        assert_eq!(e.points(), &[(10, -600), (20, 0)]);
        assert_eq!((e.at(0), e.at(10), e.at(15), e.at(20), e.at(99)), (-600, -600, -300, 0, 0));
        assert_eq!(e.peak(), 0);
        assert!(!e.ramps(9) && e.ramps(10) && e.ramps(19) && !e.ramps(20));
        let e2 = e.with(20, -600).unwrap();
        assert_eq!((e2.points(), e2.peak(), e2.ramps(15)), (&[(10, -600), (20, -600)][..], -600, false));
        assert_eq!(e.without(10).unwrap().points(), &[(20, 0)]);
        assert_eq!(e.without(11), Err(EditError::Gain));
        assert_eq!(e.with(-1, 0), Err(EditError::Negative));
        assert_eq!(e.with(5, GAIN_MAX_CDB + 1), Err(EditError::Gain));
        let mut full = GainEnv::EMPTY;
        for k in 0..GAIN_ENV_MAX as i64 {
            full = full.with(k, 0).unwrap();
        }
        assert_eq!(full.with(99, 0), Err(EditError::Gain));
        assert!(full.with(3, -100).is_ok(), "a full envelope still moves a point it has");
        assert_eq!(GainEnv::EMPTY.at(5), 0);
    }

    /// M3: set_gain_env sets one clip and a split keeps the points on both halves (source ticks).
    #[test]
    fn striagainenvelope_set_and_split() {
        let mut tl = Timeline::new(25);
        let s = tl.add_source("A", 50);
        let a = tl.add_track(Kind::Audio);
        tl.tracks[a].clips.push(Clip::new(s, 0, 20, 0));
        let e = GainEnv::EMPTY.with(5, -600).unwrap();
        let tl = tl.set_gain_env(a, 0, e).unwrap();
        assert_eq!(tl.tracks[a].clips[0].gain_env, e);
        assert_eq!(tl.set_gain_env(a, 3, e), Err(EditError::NoClip));
        let sp = tl.split(a, 10).unwrap();
        assert!(sp.tracks[a].clips.iter().all(|c| c.gain_env == e));
    }
}

// ---- STRIAGAINENVCURVE (LEDGER SR9083): the gain envelope's segment curve (named tail block) ----
// A clip's envelope carries ONE [`Curve`] for all its segments. `Linear` (the default, written as nothing) is linear in
// centi-dB between two points, today's exact integer formula. `EqualPower` takes the segment's phase through THE crossfade
// ramp curve ([`crate::schedule::xfade_weight`], the quarter-wave table), in either direction: `v0 + (v1 - v0) * w(p)`.
// An envelope with no point is always `Linear` (`without` of the last point resets it; `curved` leaves an empty one as
// it is), so `is_empty` still means the default. `curve()` / `curved()` are the only way to the field.

impl GainEnv {
    /// STRIAGAINENVCURVE (SR9083): the segments' curve.
    pub fn curve(&self) -> Curve {
        self.curve
    }
    /// STRIAGAINENVCURVE (SR9083): the same envelope with curve `c` (an empty envelope comes back unchanged: linear).
    pub fn curved(self, c: Curve) -> GainEnv {
        if self.is_empty() {
            return self;
        }
        GainEnv { curve: c, ..self }
    }
    /// The value `k` ticks into a segment of `len` ticks (`len > 0`) from `v0` to `v1`, through the curve.
    fn seg_at(&self, v0: i32, v1: i32, k: i64, len: i64) -> i32 {
        match self.curve {
            Curve::Linear => (v0 as i64 + (v1 - v0) as i64 * k / len) as i32,
            Curve::EqualPower => {
                let one = crate::schedule::ENV_ONE as i64;
                let w = crate::schedule::xfade_weight(Curve::EqualPower, (k.clamp(0, len) * one / len) as u32) as i64;
                (v0 as i64 + (v1 - v0) as i64 * w / one) as i32
            }
        }
    }
}

#[cfg(test)]
mod striagainenvcurve_tests {
    use super::*;

    /// M3: linear is unchanged; equal-power is the quarter sine of the phase (mid of -600..0 = -600 + 600 * 46341 / 65536
    /// = -176), the ends are the points; an empty envelope stays linear; removing the last point resets the curve.
    #[test]
    fn striagainenvcurve_value() {
        let e = GainEnv::EMPTY.with(10, -600).unwrap().with(20, 0).unwrap();
        assert_eq!((e.curve(), e.at(15)), (Curve::Linear, -300));
        let q = e.curved(Curve::EqualPower);
        assert_eq!((q.curve(), q.at(10), q.at(15), q.at(20), q.at(5), q.at(25)), (Curve::EqualPower, -600, -176, 0, -600, 0));
        let d = GainEnv::EMPTY.with(10, 0).unwrap().with(20, -600).unwrap().curved(Curve::EqualPower);
        assert_eq!(d.at(15), -424, "falling: the same phase weight");
        assert!(q.at(12) > e.at(12), "faster start than linear");
        assert_eq!(GainEnv::EMPTY.curved(Curve::EqualPower), GainEnv::EMPTY);
        assert_eq!(GainEnv::default(), GainEnv::EMPTY);
        let one = q.without(10).unwrap();
        assert_eq!(one.curve(), Curve::EqualPower, "one point left keeps the curve");
        assert_eq!(one.without(20).unwrap(), GainEnv::EMPTY);
        assert_ne!(q, e);
    }
}
