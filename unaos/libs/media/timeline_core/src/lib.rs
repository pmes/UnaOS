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
//!   range) and refuses a rate <= 0 by name ([`EditError::Rate`]); a positive rate lays the clip at that `rate_q16` (STRIACLIPSPEED SR829).
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
    /// STRIATITLECLIP (SR828): a generated title card's one line of text (`None`: a media reel); `edl` keeps it as `* UNA TITLE`.
    pub title: Option<String>,
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
    /// STRIACLIPSPEED (SR829): the clip's speed, Q16.16 (`0x1_0000` = 1x, the MP4 `elst` media_rate encoding); [`Clip::len`] / [`Clip::src_at`].
    pub rate_q16: u32,
}

/// NLE4: the gain range a clip may carry, centi-dB (-144 dB .. +24 dB).
pub const GAIN_MIN_CDB: i32 = -14_400;
pub const GAIN_MAX_CDB: i32 = 2_400;

impl Clip {
    pub fn new(src: SourceId, src_in: i64, src_out: i64, at: i64) -> Clip {
        Clip { src, src_in, src_out, at, gain_cdb: 0, fade_in: 0, fade_out: 0, rate_q16: 0x1_0000 }
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
        rated_len(self.src_out - self.src_in, self.rate_q16) // STRIACLIPSPEED (SR829): ceil(source span / rate) (file tail)
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
    /// EDITMAP (SR223): an edit-list segment at a rate <= 0 (a rate-0 dwell hold, or reverse); STRIACLIPSPEED (SR829): a clip
    /// at rate 0, an M2 speed <= 0, or an op that would cut a rated clip off its frame grid.
    Rate,
    /// NLESIZEPOLICYUI (SR402): the editor's `size` verb named no [`SizePolicy`] word (or the wrong number of words).
    Policy,
    /// NLE5DRANGESAVE (SR723): both export marks set with the in mark not before the out mark.
    Marks, /** STRIAFREEZELEN (SR916): a `freeze` hold the one-tick hold rate cannot lay at exactly the asked length, or a hold length the `freeze` words cannot read. */ Hold,
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
            EditError::Rate => "rate is not a positive speed (a dwell or reverse)",
            EditError::Policy => "no such size policy (letterbox, fill, stretch, adopt)",
            EditError::Marks => "the in mark is not before the out mark", EditError::Hold => "no exact one-frame hold of that length", // STRIAFREEZELEN (SR916)
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
        self.sources.push(Source { id, reel: String::from(reel), duration, size_policy: SizePolicy::Letterbox, title: None });
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
                if c.rate_q16 == 0 { return Err(EditError::Rate); } if c.src_in >= c.src_out { // STRIACLIPSPEED (SR829): rate 0 refused by name
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
                if !cut_clean(t, clip.at) { return Err(EditError::Rate); } split_at(t, clip.at); // STRIACLIPSPEED (SR829): a rated clip cuts on its frame grid (file tail)
                for c in t.clips.iter_mut().filter(|c| c.at >= clip.at) {
                    c.at += clip.len();
                }
            }
            Mode::Overwrite => { if !cut_clean(t, clip.at) || !cut_clean(t, clip.end()) { return Err(EditError::Rate); } carve(t, clip.at, clip.end()) } // STRIACLIPSPEED (SR829)
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
        if !cut_clean(tr, t) { return Err(EditError::Rate); } split_at(tr, t); // STRIACLIPSPEED (SR829): off the frame grid refused by name
        tl.validate()?;
        Ok(tl)
    }

    /// Move a clip's in point to source tick `src_in`; its timeline start moves with it, its out edge stays.
    pub fn trim_in(&self, track: usize, idx: usize, src_in: i64) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        let c = clip_mut(&mut tl, track, idx)?;
        c.at = c.end() - Clip { src_in, ..*c }.len(); // STRIACLIPSPEED (SR829): the out edge stays at any rate (1x: at += src_in - old src_in)
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
    /// rounding to no tick is skipped; a `src_out` one tick past the source, rounding, is clamped to it); a rate <= 0 is
    /// [`EditError::Rate`], a positive rate a clip at that `rate_q16` covering `(end - at) * rate` source ticks (STRIACLIPSPEED SR829). An empty list is the identity: the whole source at tick 0.
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
            if e.rate <= 0 { // STRIACLIPSPEED (SR829): a positive rate lays a rated clip; a dwell or reverse stays refused
                return Err(EditError::Rate);
            }
            let at = tb.from_ns(e.start_ns);
            let src_in = tb.from_ns(media.to_ns(pts).saturating_sub(origin_ns));
            let mut src_out = if e.duration_ns == 0 {
                dur
            } else {
                let end = tb.from_ns(e.start_ns.saturating_add(i64::try_from(e.duration_ns).unwrap_or(i64::MAX)));
                src_in + scale_q16(end - at, e.rate as u32) // STRIACLIPSPEED (SR829): the timeline span at the segment's rate (1x: end - at)
            };
            if src_out == dur + 1 {
                src_out = dur;
            }
            if src_out > src_in {
                t.clips.push(Clip::new(src, src_in, src_out, at).at_rate(e.rate as u32)); // STRIACLIPSPEED (SR829)
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
        let cut = c.src_at(at); // STRIACLIPSPEED (SR829): the source tick tick `at` shows (1x: src_in + (at - c.at))
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
        if slide_follows_rated(&tl.tracks[track], idx) { return Err(EditError::Rate); } let (cut_in, cut_out) = (c.at, c.end()); // STRIACLIPSPEED (SR829): a rated butted neighbour cannot follow tick for tick (file tail)
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

// ---- STRIACLIPSPEED (LEDGER SR829): per-clip speed — rate_q16, the one length road, src_at, set_rate (named tail block) ----
// A clip at `rate_q16` (Q16.16, `0x1_0000` = 1x: the MP4 `elst` media_rate encoding, so `lay_edit_list` lays a rated segment
// as one) shows source ticks `src_in..src_out` over `len() = ceil((src_out - src_in) * 0x1_0000 / rate)` timeline ticks. The
// rounding is CEILING, so every covered tick's source tick is below `src_out` and the last source frame is always shown.
// Timeline tick `t` shows source tick `src_at(t) = src_in + floor((t - at) * rate / 0x1_0000)`. At 1x both are the old
// identity, so a 1x timeline is unchanged everywhere. A rate-0 clip (refused `Rate` by `validate`) measures as 1x. A
// transition's outgoing handle plays at 1x past `src_out` (NLE-SPEED-HANDLE owed). The ops that cut a rated clip (`split`,
// `place` Insert / Overwrite) cut it only ON ITS FRAME GRID (both halves keep every tick and frame: `cut_clean`), else
// `Rate` by name. `slide` refuses a rated butted neighbour (`Rate`: it cannot follow tick for tick). `set_rate` is
// `trim_out`'s shape: `at` and the source window stay, the out edge moves, nothing ripples. `validate` stays the one gate.

/// STRIACLIPSPEED: the rate a length is measured at (0, refused by `validate`, measures as 1x).
fn eff_rate(rate_q16: u32) -> u32 {
    if rate_q16 == 0 { 0x1_0000 } else { rate_q16 }
}

/// STRIACLIPSPEED: `n` scaled by a Q16.16 rate, floored (i128: no overflow at any tick count).
fn scale_q16(n: i64, rate_q16: u32) -> i64 {
    ((n as i128 * rate_q16 as i128) >> 16) as i64
}

/// STRIACLIPSPEED: the timeline length of a source span `n` at `rate_q16`: `ceil(n * 0x1_0000 / rate)` (exactly `n` at 1x; a
/// non-positive span is returned as it is, so `is_empty` reads it as before).
fn rated_len(n: i64, rate_q16: u32) -> i64 {
    let r = eff_rate(rate_q16);
    if r == 0x1_0000 || n <= 0 {
        return n;
    }
    (((n as u128) << 16).div_ceil(r as u128)) as i64
}

impl Clip {
    /// STRIACLIPSPEED: the source tick timeline tick `t` shows: `src_in + floor((t - at) * rate / 0x1_0000)` (1x:
    /// `src_in + (t - at)`). The schedule's video and audio steps, `split` and the editor's mark-in / mark-out read it.
    pub fn src_at(&self, t: i64) -> i64 {
        self.src_in + scale_q16(t - self.at, eff_rate(self.rate_q16))
    }
    /// STRIACLIPSPEED: the same clip at speed `rate_q16` (Q16.16).
    pub fn at_rate(self, rate_q16: u32) -> Clip {
        Clip { rate_q16, ..self }
    }
}

/// STRIACLIPSPEED: whether cutting track `t` at tick `at` keeps a rated clip spanning it whole: the head `src_in..src_at(at)`
/// ends at `at` and the tail from `at` ends where the clip ended (always true at 1x and for any integer rate >= 1).
fn cut_clean(t: &Track, at: i64) -> bool {
    t.clips.iter().filter(|c| c.at < at && at < c.end() && c.rate_q16 != 0x1_0000).all(|c| {
        let cut = c.src_at(at);
        Clip { src_out: cut, ..*c }.end() == at && Clip { src_in: cut, at, ..*c }.end() == c.end()
    })
}

/// STRIACLIPSPEED: clip `idx` of `t` has a butted neighbour that `slide` would make follow and that neighbour is rated.
fn slide_follows_rated(t: &Track, idx: usize) -> bool {
    let Some(c) = t.clips.get(idx) else { return false };
    let rated = |o: &Clip| o.rate_q16 != 0x1_0000;
    idx.checked_sub(1).and_then(|i| t.clips.get(i)).is_some_and(|p| p.end() == c.at && rated(p))
        || t.clips.get(idx + 1).is_some_and(|n| n.at == c.end() && rated(n))
}

impl Timeline {
    /// STRIACLIPSPEED: set clip `idx`'s speed to `rate_q16` (Q16.16): its `at` and source window stay, its out edge moves
    /// (`trim_out`'s shape: a transition on the moving cut is refused `Transition`, never dropped; one on its head cut is
    /// fitted); no ripple, so a slower clip running into the next is `Overlap`; rate 0 is `Rate`.
    pub fn set_rate(&self, track: usize, idx: usize, rate_q16: u32) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        clip_mut(&mut tl, track, idx)?.rate_q16 = rate_q16;
        tl.retarget_transitions(self)?; tl.fit_trimmed(track, idx); tl.validate()?;
        Ok(tl)
    }
}

/// STRIACLIPSPEED: THE decimal speed reader (the EDL's M2 field with `per` = its fps, the face's `speed <q>` with `per` = 1):
/// `<digits>[.<digits>]` read exactly and rounded once to Q16.16 (`round(value * 0x1_0000 / per)`); `None` for any other text
/// (a sign, an exponent, more than 9 integer or 18 decimal digits) or a value past `u32`. A zero comes back as `Some(0)`.
pub fn speed_q16(s: &str, per: u32) -> Option<u32> {
    let (int, frac) = s.split_once('.').unwrap_or((s, ""));
    if (int.is_empty() && frac.is_empty()) || per == 0 || int.len() > 9 || frac.len() > 18 {
        return None;
    }
    if !int.bytes().chain(frac.bytes()).all(|b| b.is_ascii_digit()) {
        return None;
    }
    let num = int.bytes().chain(frac.bytes()).fold(0u128, |n, b| n * 10 + (b - b'0') as u128);
    let den = 10u128.pow(frac.len() as u32) * per as u128;
    u32::try_from((num * 0x1_0000 + den / 2) / den).ok()
}

#[cfg(test)]
mod striaclipspeed_tests {
    extern crate std;
    use super::*;

    fn src_ats(c: &Clip) -> Vec<i64> {
        (c.at..c.end()).map(|t| c.src_at(t)).collect()
    }

    /// V1: A[0,10)@0 at `rate`, then A[20,30)@10.
    fn tl2(rate: u32) -> Timeline {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 10, 0).at_rate(rate), Clip::new(a, 20, 30, 10)];
        tl
    }

    #[test]
    fn striaclipspeed_len_and_src_at_kats() {
        let c = Clip::new(SourceId(0), 0, 10, 0);
        assert_eq!((c.rate_q16, c.len(), src_ats(&c)), (0x1_0000, 10, (0..10).collect::<Vec<_>>()));
        let d = c.at_rate(0x2_0000);
        assert_eq!((d.len(), src_ats(&d)), (5, alloc::vec![0, 2, 4, 6, 8]));
        let h = c.at_rate(0x8000);
        assert_eq!((h.len(), src_ats(&h)), (20, (0..20).map(|k| k / 2).collect::<Vec<_>>()));
        let f = c.at_rate(0x1_8000);
        assert_eq!((f.len(), src_ats(&f)), (7, alloc::vec![0, 1, 3, 4, 6, 7, 9]));
        let m = Clip::new(SourceId(0), 13, 23, 5).at_rate(0x2_0000);
        assert_eq!((m.end(), m.src_at(5), m.src_at(9)), (10, 13, 21));
        assert_eq!(c.at_rate(0).len(), 10, "rate 0 measures as 1x");
        std::println!("striaclipspeed: over 10 source ticks: 1x len 10, 2x len 5 (0,2,4,6,8), 0.5x len 20, 1.5x len 7 (0,1,3,4,6,7,9)");
    }

    #[test]
    fn striaclipspeed_validate_and_the_following_clip() {
        let tl = tl2(0x2_0000);
        assert_eq!(tl.validate(), Ok(()));
        assert_eq!((tl.tracks[0].clips[0].end(), tl.tracks[0].clips[1].at, tl.end()), (5, 10, 20));
        assert_eq!(tl2(0).validate(), Err(EditError::Rate));
        assert_eq!(tl2(0x8000).validate(), Err(EditError::Overlap), "0.5x over 10 is 20 ticks: into the clip at 10");
        assert_eq!(alloc::format!("{}", EditError::Rate), "rate is not a positive speed (a dwell or reverse)");
        std::println!("striaclipspeed: 2x over 10 occupies 0..5, next clip stays at 10; rate 0 -> {}", EditError::Rate);
    }

    #[test]
    fn striaclipspeed_split_and_insert_cut_on_the_frame_grid() {
        let tl = tl2(0x2_0000);
        let s = tl.split(0, 2).unwrap();
        let a = SourceId(0);
        assert_eq!(&s.tracks[0].clips[..2], &[Clip::new(a, 0, 4, 0).at_rate(0x2_0000), Clip::new(a, 4, 10, 2).at_rate(0x2_0000)]);
        for t in 0..5 {
            let i = s.clip_at(0, t).unwrap();
            assert_eq!(s.tracks[0].clips[i].src_at(t), tl.tracks[0].clips[0].src_at(t), "tick {t}");
        }
        let mut slow = Timeline::new(25);
        let a = slow.add_source("A", 50);
        slow.add_track(Kind::Video);
        slow.tracks[0].clips = alloc::vec![Clip::new(a, 0, 10, 0).at_rate(0x8000)];
        assert!(slow.split(0, 4).is_ok(), "a 0.5x clip cuts on a frame boundary");
        assert_eq!(slow.split(0, 3), Err(EditError::Rate), "mid-hold");
        assert_eq!(slow.place(0, Clip::new(a, 0, 2, 3), Mode::Insert), Err(EditError::Rate));
        assert_eq!(slow.place(0, Clip::new(a, 0, 2, 3), Mode::Overwrite), Err(EditError::Rate));
        std::println!("striaclipspeed: 2x split at 2 keeps frames 0,2 | 4,6,8; 0.5x split at 3 -> {}", EditError::Rate);
    }

    #[test]
    fn striaclipspeed_trim_in_slide_and_set_rate() {
        let tl = tl2(0x2_0000);
        let c = tl.trim_in(0, 0, 4).unwrap().tracks[0].clips[0];
        assert_eq!((c.src_in, c.at, c.end()), (4, 2, 5), "trim_in keeps the out edge at 2x");
        assert_eq!(tl2(0x1_0000).set_rate(0, 0, 0x2_0000), Ok(tl2(0x2_0000)));
        assert_eq!(tl2(0x1_0000).set_rate(0, 0, 0x8000), Err(EditError::Overlap));
        assert_eq!(tl2(0x1_0000).set_rate(0, 0, 0), Err(EditError::Rate));
        let mut b = tl2(0x2_0000);
        b.tracks[0].clips[1].at = 5;
        assert_eq!(b.slide(0, 1, 1), Err(EditError::Rate), "a rated butted neighbour cannot follow");
        assert!(tl2(0x1_0000).slide(0, 1, -1).is_ok());
        std::println!("striaclipspeed: trim_in at 2x keeps end 5; set_rate 2x == built 2x; 0.5x into a neighbour -> {}", EditError::Overlap);
    }

    #[test]
    fn striaclipspeed_speed_q16_reads_decimals_exactly() {
        assert_eq!(speed_q16("2", 1), Some(0x2_0000));
        assert_eq!(speed_q16("0.5", 1), Some(0x8000));
        assert_eq!(speed_q16("1.5", 1), Some(0x1_8000));
        assert_eq!(speed_q16("037.5", 25), Some(0x1_8000));
        assert_eq!(speed_q16("050.0", 25), Some(0x2_0000));
        assert_eq!(speed_q16("0", 1), Some(0));
        for bad in ["", ".", "x", "-2", "1.2.3", "1e3", "+1"] {
            assert_eq!(speed_q16(bad, 1), None, "{bad}");
        }
    }
}

// ---- STRIAFREEZEFRAME (LEDGER SR880): a one-frame hold as a rated one-tick clip — hold_rate, Timeline::freeze (named tail block) ----
// A freeze is NOT a new clip kind: it is SR829's rated clip over a ONE-tick source span `f..f+1` at `hold_rate(n)` =
// `ceil(0x1_0000 / n)`. Its `len()` (`rated_len`) is `ceil(0x1_0000 / r)`, exactly `n` whenever `n * (n - 1) <= 0x1_0000`
// (n <= 256: a 2 s hold at any fps <= 128), and for EVERY rate >= 1 `(len - 1) * r < 0x1_0000`, so `src_at` is `f` on every
// covered tick: the schedule names one source tick, `seek_for` sees the run continue, `Vid::show` holds the decoded frame.
// The longest hold is 65536 ticks (rate 1). `freeze` lays it by `place` (`Mode::Insert`), which already splits the clip
// under the playhead (`split_at`, `cut_clean`) and ripples the rest of that track; the EDL keeps it as the hold's `M2` line.
// The kernel Player plays a video hold (STRIAKFREEZE SR9008: `Timeline::rated_unheld`, `held_ticks`) and refuses every other rated clip by name (`rate-unsupported`, SR829).

/// STRIAFREEZEFRAME: the length of the `freeze` word's hold, seconds.
pub const FREEZE_SECS: i64 = 2;

/// STRIAFREEZEFRAME: the Q16.16 rate at which a one-tick source span covers `ticks` timeline ticks: `ceil(0x1_0000 / ticks)`,
/// at least 1 (so a hold is never longer than 65536 ticks); `ticks < 1` gives 1x (callers refuse it first).
pub fn hold_rate(ticks: i64) -> u32 {
    if ticks < 1 {
        return 0x1_0000;
    }
    if ticks >= 0x1_0000 {
        return 1;
    }
    ((0x1_0000 + ticks - 1) / ticks) as u32
}

impl Timeline {
    /// STRIAFREEZEFRAME: hold the frame shown at tick `t` for `ticks` ticks: on the topmost video track with a clip covering
    /// `t` (`schedule::top`, what the preview shows), that clip's source tick `f = src_at(t)` laid as `f..f+1` at
    /// [`hold_rate`] by `place` (`Mode::Insert`) at `t`. Returns the new timeline and the track. Refused by name: `ticks < 1`
    /// `Empty`; no video clip covers `t` `NoClip`; a rated clip under `t` off its frame grid `Rate` (`place`'s own); any
    /// transition / `validate` refusal is `place`'s. Only that track ripples (other tracks stay: NLE-FREEZE-SYNC owed).
    pub fn freeze(&self, t: i64, ticks: i64) -> Result<(Timeline, usize), EditError> {
        if ticks < 1 {
            return Err(EditError::Empty);
        }
        let (track, ci) = crate::schedule::top(self, Kind::Video, t).ok_or(EditError::NoClip)?;
        let c = self.tracks[track].clips[ci];
        let f = c.src_at(t);
        let hold = Clip::new(c.src, f, f + 1, t).at_rate(hold_rate(ticks)); if hold.len() != ticks { return Err(EditError::Hold); } // STRIAFREEZELEN (SR916): an inexact hold refused by name
        Ok((self.place(track, hold, Mode::Insert)?, track))
    }
}

#[cfg(test)]
mod striafreezeframe_tests {
    extern crate std;
    use super::*;

    /// V1: A[0,20)@0 and B[0,10)@30; A1: A[0,20)@0.
    fn fixture() -> Timeline {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        let b = tl.add_source("B", 50);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 20, 0), Clip::new(b, 0, 10, 30)];
        tl.tracks[1].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        tl
    }

    #[test]
    fn striafreezeframe_hold_rate_exact() {
        for fps in [24i64, 25, 30, 50, 60, 120] {
            let n = FREEZE_SECS * fps;
            let h = Clip::new(SourceId(0), 7, 8, 0).at_rate(hold_rate(n));
            assert_eq!(h.len(), n, "fps {fps}");
            assert!((h.at..h.end()).all(|t| h.src_at(t) == 7), "fps {fps}: a pure hold");
        }
        let h = Clip::new(SourceId(0), 7, 8, 0).at_rate(hold_rate(1000));
        assert_eq!((hold_rate(1000), h.len()), (66, 993), "past n = 256 the hold rounds, still a hold");
        assert!((h.at..h.end()).all(|t| h.src_at(t) == 7));
        assert_eq!((hold_rate(50), hold_rate(1 << 20), hold_rate(0)), (1311, 1, 0x1_0000));
        std::println!("striafreezeframe: hold_rate(50) = 1311 -> len 50; hold_rate(1000) = 66 -> len 993");
    }

    #[test]
    fn striafreezeframe_inserts_and_ripples() {
        let tl = fixture();
        let (a, b) = (SourceId(0), SourceId(1));
        let (fr, track) = tl.freeze(7, 50).unwrap();
        assert_eq!(track, 0);
        let v = &fr.tracks[0].clips;
        assert_eq!(v.len(), 4);
        assert_eq!((v[0].src, v[0].src_in, v[0].src_out, v[0].at), (a, 0, 7, 0));
        assert_eq!((v[1].src, v[1].src_in, v[1].src_out, v[1].at, v[1].rate_q16, v[1].len()), (a, 7, 8, 7, 1311, 50));
        assert!((7..57).all(|t| v[1].src_at(t) == 7), "every hold tick shows source tick 7");
        assert_eq!((v[2].src, v[2].src_in, v[2].src_out, v[2].at), (a, 7, 20, 57));
        assert_eq!((v[3].src, v[3].at), (b, 80), "a later clip moves by the hold");
        assert_eq!(fr.tracks[1], tl.tracks[1], "the audio track does not ripple (NLE-FREEZE-SYNC owed)");
        let (on_cut, _) = tl.freeze(30, 50).unwrap();
        assert_eq!((on_cut.tracks[0].clips[1].src, on_cut.tracks[0].clips[1].src_in, on_cut.tracks[0].clips[1].at), (b, 0, 30), "on a cut: the incoming first frame");
        let mut two = tl.clone();
        two.add_track(Kind::Video);
        two.tracks[2].clips = alloc::vec![Clip::new(b, 20, 30, 5)];
        let (top, track) = two.freeze(7, 50).unwrap();
        assert_eq!((track, top.tracks[2].clips[1].src_in), (2, 22), "the topmost video track wins, as in the preview");
        assert_eq!(top.tracks[0], two.tracks[0]);
        std::println!("striafreezeframe: freeze at 7 -> A 0..7 | A 7..8 @7 len 50 | A 7..20 @57 | B @80");
    }

    #[test]
    fn striafreezeframe_refusals() {
        let tl = fixture();
        assert_eq!(tl.freeze(tl.end(), 50), Err(EditError::NoClip), "at the end");
        assert_eq!(tl.freeze(25, 50), Err(EditError::NoClip), "in a video gap");
        assert_eq!(tl.freeze(7, 0), Err(EditError::Empty));
        let mut audio = tl.clone();
        audio.tracks.remove(0);
        assert_eq!(audio.freeze(7, 50), Err(EditError::NoClip), "an audio clip is not a picture");
        let mut slow = tl.clone();
        slow.tracks[0].clips[0] = Clip::new(SourceId(0), 0, 10, 0).at_rate(0x8000);
        assert_eq!(slow.freeze(3, 50), Err(EditError::Rate), "mid-hold of a 0.5x clip");
        assert!(slow.freeze(2, 50).is_ok(), "on the 0.5x clip's frame grid");
    }

    #[test]
    fn striafreezeframe_one_undo_step() {
        let tl = fixture();
        let mut st = crate::editor::EditorState::new(tl.clone());
        st.seek(7);
        assert_eq!(st.verb("freeze"), Some(Ok(())));
        assert_eq!((st.undo_depth(), st.selection, st.timeline.end()), (1, Some((0, 1)), tl.end() + 50));
        assert!(st.undo());
        assert_eq!(st.timeline, tl, "one undo restores the whole timeline");
        st.seek(st.timeline.end());
        assert_eq!(st.verb("freeze"), Some(Err(EditError::NoClip)));
        assert_eq!(st.undo_depth(), 0, "a refusal pushes no undo step");
    }

    #[test]
    fn striafreezeframe_edl_round_trip() {
        let mut tl = fixture();
        tl.tracks.remove(1);
        let (fr, _) = tl.freeze(7, 50).unwrap();
        let s = crate::edl::write(&fr, "FREEZE").unwrap();
        assert_eq!(s.matches("M2 ").count(), 1, "{s}");
        assert!(s.contains("M2   A        000.5001068115234375 00:00:00:07\n"), "{s}");
        let back = crate::edl::parse(&s, 25).unwrap();
        assert_eq!(back, fr);
        assert_eq!(crate::edl::write(&back, "FREEZE").unwrap(), s, "write(parse(write)) byte-identical");
    }
}

// ---- STRIAFREEZELEN (LEDGER SR916): the hold's length and its sync choice — Timeline::freeze_all, ripple_gap (named tail block) ----
// `Timeline::freeze` stays THE one hold road; it now refuses by name (`EditError::Hold`) a length the one-tick hold rate cannot
// lay exactly: `len(hold) == ticks` holds for 511 counts in 1..=65536 (every n <= 256, n(n-1) <= 0x1_0000 being sufficient, not
// necessary: 258, 512, 65536 are exact, 257 and 500 are not). `freeze_all` is `freeze` and then, on EVERY other track, the same
// gap opened at the playhead (`ripple_gap`: `place` Insert's room-making without the clip), in ONE value: a refusal on any track
// (`Rate` off a rated clip's grid, `Transition` for a transition or audio crossfade on a cut at the playhead) refuses the whole
// op. The export marks at or after the playhead move with `freeze_all` (STRIAFREEZEMARKS SR931); plain `freeze` leaves them.
impl Timeline {
    /// STRIAFREEZELEN: [`Timeline::freeze`] at tick `t` for `ticks` ticks, and every other track rippled by `ticks` at `t` (the
    /// clip spanning `t` split, every clip from `t` on moved later), so picture and sound stay in sync after the hold. Returns
    /// the new timeline and the hold's track. Refused by name: `freeze`'s own refusals, and any track's `ripple_gap` refusal.
    pub fn freeze_all(&self, t: i64, ticks: i64) -> Result<(Timeline, usize), EditError> {
        let (mut tl, track) = self.freeze(t, ticks)?;
        for k in (0..tl.tracks.len()).filter(|&k| k != track) {
            tl = tl.ripple_gap(k, t, ticks)?;
        }
        Ok((tl.ripple_marks(t, ticks)?, track)) // STRIAFREEZEMARKS (SR931): the export marks at or after the hold ride the ripple
    }

    /// STRIAFREEZELEN: open a gap of `len` ticks at tick `at` on `track`: the clip strictly spanning `at` is split there (a
    /// rated clip off its frame grid `Rate`), every clip from `at` on moves later by `len`, the track's transitions and audio
    /// crossfades follow (`retarget_transitions`; one on a cut at `at` is gone: `Transition`); `validate` is the gate.
    fn ripple_gap(&self, track: usize, at: i64, len: i64) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        let t = tl.tracks.get_mut(track).ok_or(EditError::NoTrack)?;
        if !cut_clean(t, at) {
            return Err(EditError::Rate);
        }
        split_at(t, at);
        for c in t.clips.iter_mut().filter(|c| c.at >= at) {
            c.at += len;
        }
        tl.retarget_transitions(self)?;
        tl.validate()?;
        Ok(tl)
    }
}

#[cfg(test)]
mod striafreezelen_tests {
    extern crate std;
    use super::*;

    /// V1 and A1 alike: A[0,20)@0 and B[0,10)@30.
    fn fixture() -> Timeline {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        let b = tl.add_source("B", 50);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        for k in 0..2 {
            tl.tracks[k].clips = alloc::vec![Clip::new(a, 0, 20, 0), Clip::new(b, 0, 10, 30)];
        }
        tl
    }

    #[test]
    fn striafreezelen_exact_or_refused() {
        let tl = fixture();
        assert_eq!(tl.freeze(7, 257), Err(EditError::Hold), "257 would lay 256");
        assert_eq!(tl.freeze(7, 500), Err(EditError::Hold), "500 would lay 497");
        assert_eq!(tl.freeze_all(7, 257), Err(EditError::Hold));
        let (big, _) = tl.freeze(7, 512).unwrap();
        assert_eq!((big.tracks[0].clips[1].rate_q16, big.tracks[0].clips[1].len()), (128, 512), "past 256, an exact count is laid");
        let (two, _) = tl.freeze(7, 50).unwrap();
        assert_eq!(two.tracks[0].clips[1].len(), 50, "SR880's 2 s hold unchanged");
        std::println!("striafreezelen: 257 / 500 refused Hold; 512 laid at rate 128; 50 as before");
    }

    #[test]
    fn striafreezelen_all_ripples_every_track() {
        let mut tl = fixture();
        let b = SourceId(1);
        tl.add_track(Kind::Video);
        tl.tracks[2].clips = alloc::vec![Clip::new(b, 20, 30, 5)];
        let (fr, track) = tl.freeze_all(7, 50).unwrap();
        assert_eq!(track, 2, "the hold sits on the topmost video track, as in the preview");
        let v2 = &fr.tracks[2].clips;
        assert_eq!((v2[1].src_in, v2[1].src_out, v2[1].at, v2[1].len()), (22, 23, 7, 50));
        assert_eq!((v2[2].src_in, v2[2].at), (22, 57));
        let v1: Vec<(i64, i64, i64)> = fr.tracks[0].clips.iter().map(|c| (c.src_in, c.src_out, c.at)).collect();
        assert_eq!(v1, alloc::vec![(0, 7, 0), (7, 20, 57), (0, 10, 80)], "V1 split at 7 and rippled by 50");
        assert_eq!(fr.tracks[1].clips, fr.tracks[0].clips, "A1 takes the same gap: picture and sound stay in sync");
        assert_eq!(fr.end(), tl.end() + 50);
        let (one, _) = tl.freeze(7, 50).unwrap();
        assert_eq!(&one.tracks[..2], &tl.tracks[..2], "plain freeze ripples the hold's track only");
        std::println!("striafreezelen: freeze-all at 7 -> hold on V2, V1 and A1 split at 7 and moved by 50");
    }

    #[test]
    fn striafreezelen_all_or_nothing() {
        let mut slow = fixture();
        slow.tracks[1].clips[0] = Clip::new(SourceId(0), 0, 10, 0).at_rate(0x8000);
        assert_eq!(slow.freeze_all(3, 50), Err(EditError::Rate), "A1's 0.5x clip is off its grid at 3");
        assert!(slow.freeze(3, 50).is_ok(), "the hold's own track alone is fine");
        let mut xf = Timeline::new(25);
        let a = xf.add_source("A", 50);
        let b = xf.add_source("B", 50);
        xf.add_track(Kind::Video);
        xf.add_track(Kind::Audio);
        xf.tracks[0].clips = alloc::vec![Clip::new(a, 0, 40, 0)];
        xf.tracks[1].clips = alloc::vec![Clip::new(a, 0, 20, 0), Clip::new(b, 0, 10, 20)];
        let xf = xf.add_audio_xfade(1, 20, 4, Curve::Linear).unwrap();
        assert_eq!(xf.freeze_all(20, 50), Err(EditError::Transition), "a gap on the crossfaded cut is refused, never dropped");
        assert!(xf.freeze(20, 50).is_ok());
        assert_eq!(xf.freeze_all(10, 50).map(|(t, _)| t.audio_xfades[0].at), Ok(70), "a crossfade past the playhead moves");
    }

    #[test]
    fn striafreezelen_words() {
        use crate::editor::freeze_words;
        assert_eq!(freeze_words("freeze", Some(25)), Some(Ok((50, false))));
        assert_eq!(freeze_words("freeze-all", Some(25)), Some(Ok((50, true))));
        assert_eq!(freeze_words("freeze 1.5", Some(25)), Some(Ok((38, false))), "37.5 ticks rounds half up");
        assert_eq!(freeze_words("freeze 0.5", Some(25)), Some(Ok((13, false))));
        assert_eq!(freeze_words("freeze-all 00:00:01:05", Some(25)), Some(Ok((30, true))));
        assert_eq!(freeze_words("freeze 00:00:01:30", Some(25)), Some(Err(EditError::Hold)), "frames past the rate");
        assert_eq!(freeze_words("freeze x", Some(25)), Some(Err(EditError::Hold)));
        assert_eq!(freeze_words("freeze 1 2", Some(25)), Some(Err(EditError::Hold)));
        assert_eq!(freeze_words("freeze", None), Some(Err(EditError::Rate)));
        assert_eq!(freeze_words("freezer", Some(25)), None);
        assert_eq!(freeze_words("split", Some(25)), None);
    }

    #[test]
    fn striafreezelen_one_undo_step() {
        let tl = fixture();
        let mut st = crate::editor::EditorState::new(tl.clone());
        st.seek(7);
        assert_eq!(st.verb("freeze-all 1"), Some(Ok(())));
        assert_eq!((st.undo_depth(), st.selection, st.timeline.end()), (1, Some((0, 1)), tl.end() + 25));
        assert_eq!(st.timeline.tracks[1].clips[1].at, 32, "A1 rippled in the same commit");
        assert!(st.undo());
        assert_eq!(st.timeline, tl, "one undo restores every track");
        assert_eq!(st.verb("freeze 20"), Some(Err(EditError::Hold)), "500 ticks is not an exact hold");
        assert_eq!(st.verb("freeze 0"), Some(Err(EditError::Empty)));
        assert_eq!(st.undo_depth(), 0, "a refusal pushes no undo step");
    }
}

// ---- STRIAFREEZEMARKS (LEDGER SR931): the export marks ride freeze_all's ripple (named tail block) ----
// `freeze_all` moves EVERY track by the hold's length at the playhead, so each set export mark at or after the playhead
// (`m >= t`, `place` Insert's own `c.at >= at`) moves by the same length in the same value: the marked range keeps its
// meaning. A mark before the playhead stays. Plain `freeze` (one track moved) leaves the marks. `set_marks` stays the one
// gate (the shift keeps in < out and every mark >= 0). `title`'s Insert leaves the marks (NLE-INSERT-MARKS owed).
impl Timeline {
    /// STRIAFREEZEMARKS: the timeline with each set mark `m >= at` moved to `m + by` (marks before `at` stay), through
    /// [`Timeline::set_marks`].
    fn ripple_marks(&self, at: i64, by: i64) -> Result<Timeline, EditError> {
        let mv = |m: Option<i64>| m.map(|x| if x >= at { x + by } else { x });
        self.set_marks((mv(self.marks.0), mv(self.marks.1)))
    }
}

#[cfg(test)]
mod striafreezemarks_tests {
    extern crate std;
    use super::*;

    /// V1 and A1 alike: A[0,20)@0 and B[0,10)@30 (end 40), marks `m`.
    fn fixture(m: (Option<i64>, Option<i64>)) -> Timeline {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        let b = tl.add_source("B", 50);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        for k in 0..2 {
            tl.tracks[k].clips = alloc::vec![Clip::new(a, 0, 20, 0), Clip::new(b, 0, 10, 30)];
        }
        tl.set_marks(m).unwrap()
    }

    #[test]
    fn striafreezemarks_all_moves_marks_at_or_after() {
        let cases = [
            ((Some(2), Some(35)), (Some(2), Some(85)), "in before stays, out after moves"),
            ((Some(10), Some(35)), (Some(60), Some(85)), "both after move"),
            ((Some(7), Some(35)), (Some(57), Some(85)), "a mark exactly at the playhead moves"),
            ((Some(2), Some(7)), (Some(2), Some(57)), "an out mark at the playhead moves past the hold"),
            ((None, Some(12)), (None, Some(62)), "one mark set"),
            ((Some(3), None), (Some(3), None), "one mark before the playhead"),
            ((None, None), (None, None), "no marks"),
        ];
        for (m, want, why) in cases {
            let (fr, _) = fixture(m).freeze_all(7, 50).unwrap();
            assert_eq!(fr.marks, want, "{why}");
        }
        std::println!("striafreezemarks: freeze-all at 7 by 50 moves every mark >= 7 by 50");
    }

    #[test]
    fn striafreezemarks_plain_freeze_leaves_marks() {
        let tl = fixture((Some(10), Some(35)));
        let (fr, _) = tl.freeze(7, 50).unwrap();
        assert_eq!(fr.marks, (Some(10), Some(35)), "one track moved: the marks stay");
        assert_eq!(fr.end(), tl.end() + 50);
    }

    #[test]
    fn striafreezemarks_one_undo_step() {
        let tl = fixture((Some(2), Some(35)));
        let mut st = crate::editor::EditorState::new(tl.clone());
        st.seek(7);
        assert_eq!(st.verb("freeze-all 1"), Some(Ok(())));
        assert_eq!((st.undo_depth(), st.timeline.marks), (1, (Some(2), Some(60))), "the marks move in the hold's commit");
        assert!(st.undo());
        assert_eq!(st.timeline, tl, "one undo restores the marks with every track");
        assert_eq!(st.verb("freeze 1"), Some(Ok(())));
        assert_eq!(st.timeline.marks, (Some(2), Some(35)), "plain freeze leaves the marks");
    }
}

// ---- STRIAFREEZEMORE (LEDGER SR970): retype a title source (named tail block) ----
// A title's text lives on its ONE `Source` (`Source::title`, STRIATITLECLIP SR828); the render keys its still by text, so a
// new text needs no cache work. `EditorState::retitle` commits this op's value as one undo step.
impl Timeline {
    /// STRIAFREEZEMORE: the timeline with title source `src`'s text set to `text`. Refused by name: no such source, or a
    /// media reel (`title == None`), `EditError::NoSource`.
    pub fn set_title(&self, src: SourceId, text: &str) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        let s = tl.sources.iter_mut().find(|s| s.id == src).ok_or(EditError::NoSource)?;
        let t = s.title.as_mut().ok_or(EditError::NoSource)?;
        *t = String::from(text);
        Ok(tl)
    }
}

#[cfg(test)]
mod striafreezemore_tests {
    use super::*;

    /// A titled source retypes and nothing else changes; a media reel and a missing source are `NoSource`.
    #[test]
    fn striafreezemore_set_title_retypes_only_a_title() {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        let t = tl.add_source("T1", 125);
        tl.sources[1].title = Some(String::from("Hello"));
        let out = tl.set_title(t, "Bye").unwrap();
        assert_eq!(out.source(t).and_then(|s| s.title.as_deref()), Some("Bye"));
        let mut back = out.clone();
        back.sources[1].title = Some(String::from("Hello"));
        assert_eq!(back, tl, "only the text moved");
        assert_eq!(tl.set_title(a, "X"), Err(EditError::NoSource), "a media reel");
        assert_eq!(tl.set_title(SourceId(99), "X"), Err(EditError::NoSource), "no such source");
    }
}

// ---- STRIAKFREEZE (LEDGER SR9008): the Player's hold — Clip::is_hold, Timeline::rated_unheld, held_ticks (named tail block) ----
// The kernel Player decodes ONE sample per video step (no target pts), so a hold plays only when the steps that repeat a picture
// are named: `held_ticks` reads them off the schedule's own steps (no seek, the same source, the same source tick; never at 1x).
// `rated_unheld` is the Player's refusal: every rated clip but a hold (`is_hold`: a one-tick source span, a constant `src_at`
// at any rate >= 1) on a Video track. The host face holds by pts in `Vid::show` and calls neither.
impl Clip {
    /// STRIAKFREEZE: a hold: a one-tick source span, so every covered tick shows the same source tick (`src_at` constant).
    pub fn is_hold(&self) -> bool {
        self.src_out - self.src_in == 1
    }
}

impl Timeline {
    /// STRIAKFREEZE: the first clip whose speed is not 1x and that is not a hold on a Video track (the kernel Player's
    /// `rate-unsupported` refusal); `None`: every clip plays there.
    pub fn rated_unheld(&self) -> Option<&Clip> {
        self.tracks.iter().flat_map(|t| t.clips.iter().map(move |c| (t.kind, c))).find(|(k, c)| c.rate_q16 != 0x1_0000 && !(*k == Kind::Video && c.is_hold())).map(|(_, c)| c)
    }
}

/// STRIAKFREEZE: the ascending `out_t` of each video step whose NEXT video step shows the same picture (no seek, the same
/// `Some(src)`, the same `src_t`): the ticks whose decoded picture a one-sample-per-step player keeps for the repeat. Empty
/// for every 1x timeline (consecutive 1x steps never repeat a source tick).
pub fn held_ticks(steps: &[schedule::Step]) -> Vec<i64> {
    let v: Vec<&schedule::Step> = steps.iter().filter(|s| s.kind == Kind::Video).collect();
    v.windows(2).filter(|w| w[1].seek.is_none() && w[0].src.is_some() && w[1].src == w[0].src && w[1].src_t == w[0].src_t).map(|w| w[0].out_t).collect()
}

#[cfg(test)]
mod striakfreeze_tests {
    extern crate std;
    use super::*;

    /// Every source 1000 ticks long, a keyframe at 0.
    struct All;
    impl SourceIndex for All {
        fn duration(&self, _: SourceId) -> Option<i64> {
            Some(1000)
        }
        fn keyframe_ns(&self, _: SourceId, _: Kind, _: i64) -> Option<i64> {
            Some(0)
        }
    }

    /// V1: A[0,20)@0 and B[0,10)@30; A1: A[0,20)@0.
    fn fixture() -> Timeline {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        let b = tl.add_source("B", 50);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Audio);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 20, 0), Clip::new(b, 0, 10, 30)];
        tl.tracks[1].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        tl
    }

    #[test]
    fn striakfreeze_rated_unheld() {
        let tl = fixture();
        assert_eq!(tl.rated_unheld(), None, "a 1x timeline plays");
        let (fr, _) = tl.freeze(7, 50).unwrap();
        assert!(fr.tracks[0].clips[1].is_hold());
        assert_eq!(fr.rated_unheld(), None, "a video hold plays");
        let mut slow = tl.clone();
        slow.tracks[0].clips[0] = Clip::new(SourceId(0), 0, 10, 0).at_rate(0x8000);
        assert_eq!(slow.rated_unheld().map(|c| c.rate_q16), Some(0x8000), "a 0.5x clip is refused");
        let mut aud = tl.clone();
        aud.tracks[1].clips = alloc::vec![Clip::new(SourceId(0), 7, 8, 0).at_rate(hold_rate(20))];
        assert!(aud.tracks[1].clips[0].is_hold());
        assert!(aud.rated_unheld().is_some(), "a rated audio clip is refused, even a one-tick span");
    }

    #[test]
    fn striakfreeze_held_ticks() {
        let tl = fixture();
        let plain = crate::schedule::schedule(&tl, 0..tl.end(), &All).unwrap();
        assert!(held_ticks(&plain).is_empty(), "no 1x step repeats a source tick");
        let (fr, _) = tl.freeze(7, 50).unwrap();
        let steps = crate::schedule::schedule(&fr, 0..fr.end(), &All).unwrap();
        let held = held_ticks(&steps);
        assert_eq!(held, (7..57).collect::<Vec<i64>>(), "the hold's 50 ticks: each next step (the tail's first included) shows source tick 7");
        assert!(steps.iter().filter(|s| s.kind == Kind::Video && (7..=57).contains(&s.out_t)).all(|s| s.src_t == 7 && s.seek.is_none()));
        std::println!("striakfreeze: freeze at 7 for 50 -> held ticks 7..57 ({})", held.len());
    }
}
