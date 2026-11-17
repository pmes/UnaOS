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
    /// STRIATITLECLIP (SR828): a generated title card's text (`None`: a media reel; an LF is a line break, STRIAFREEZETHREE SR9029); `edl` keeps it as `* UNA TITLE`.
    pub title: Option<String>, pub title_style: TitleStyle, // STRIAFREEZETHREE (SR9029): the title's size / alignment / ink (DEFAULT: today's look; `* UNA TITLESTYLE`)
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
    pub crop: Crop, // STRIACROP (SR867): the per-clip crop, 1/1000 of the frame per side (Crop::NONE = none; file tail)
    /// STRIAOPACITY (SR868): the video clip's opacity, percent 0..=[`OPAQUE`] (opaque, the default); below it the track underneath shows through.
    pub opacity: u8,
    pub group: u32, // STRIACLIPLINK (SR863): the link group (0 = none): clips sharing a nonzero id select, move, delete and split together
    pub grade: Grade, // STRIACOLORLIFT (SR869): the clip's colour grade, IDENTITY = none (file tail)
    pub pan_cq8: i8, // STRIAPAN (LEDGER SR861): the clip's pan, -128..=127 (0 = centre; -127 / -128 hard left, 127 hard right; schedule::pan_q16 is the law)
    pub xform: Xform, // STRIAPOSITIONSCALE (SR879): the per-clip position / scale (picture-in-picture; Xform::NONE = the whole frame; file tail)
    pub xkeys: XKeys, // STRIAXFORMKEYS (SR9049): the per-clip placement keyframes on source ticks (XKeys::NONE = none: the static `xform`; file tail)
    /// STRIAOPACITYKEYS (SR9048): the opacity points on the clip's source ticks (none, the default: `opacity` holds).
    pub opacity_keys: OpacityKeys,
}

/// NLE4: the gain range a clip may carry, centi-dB (-144 dB .. +24 dB).
pub const GAIN_MIN_CDB: i32 = -14_400;
pub const GAIN_MAX_CDB: i32 = 2_400;

impl Clip {
    pub fn new(src: SourceId, src_in: i64, src_out: i64, at: i64) -> Clip {
        Clip { src, src_in, src_out, at, gain_cdb: 0, fade_in: 0, fade_out: 0, rate_q16: 0x1_0000, crop: Crop::NONE, opacity: OPAQUE, group: 0, grade: Grade::IDENTITY, pan_cq8: 0, xform: Xform::NONE, xkeys: XKeys::NONE, opacity_keys: OpacityKeys::NONE }
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
    pub markers: Vec<Marker>, // STRIAMARKERS (SR835): the named markers, sorted by `at`, one per tick; edl keeps them as `* UNA MARKER` (file tail)
}

/// How [`Timeline::place`] makes room.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Split whatever spans `at` and push everything from `at` on later by the clip's length (ripple).
    Insert,
    /// Cut `at..at+len` out of whatever is there (trimming or splitting it) and lay the clip in the hole.
    Overwrite,
    InsertAll, // STRIAINSERTMARKSKEYSNEXT (SR985): Insert on the target track and the same room (split, ripple) on every other track (file tail)
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
    /// STRIAMARKERS (SR835): a marker already at that tick (or none there to remove; or markers out of tick order).
    Marker,
    /// STRIATRACKADDREMOVE (SR856): a tenth track of one kind (CMX channels run V..V9 and A..A9).
    Tracks,
    /// STRIACROP (SR867): a crop side past the frame, `l + r` or `t + b` not under 1000, or a malformed `crop` word.
    Crop,
    /// STRIAOPACITY (SR868): an opacity above [`OPAQUE`] percent, or a non-opaque clip on an audio track.
    Opacity,
    Grade, // STRIACOLORLIFT (SR869): a grade value outside -1000..=1000, or the `color` word misshapen
    /// STRIAPOSITIONSCALE (SR879): a placement off the frame (`x + s` or `y + s` past 1000, `s` 0) or a malformed `pip` / `scale` word.
    Xform,
    NoMarks, // STRIAINSERTMARKS (SR905): `insert-marked` / `overwrite-marked` with an export mark unset
    InsertForm, // STRIAINSERTMARKS (SR905): an `insert` / `overwrite` word in a shape other than bare or `<reel> <in> <out>`
    Crossfade, // STRIATRACKORDERKEYS (SR922): a track reorder that would change an audio fade a video transition implies /** STRIAFREEZELEN (SR916): a `freeze` hold the one-tick hold rate cannot lay at exactly the asked length, or a hold length the `freeze` words cannot read. */ Hold, /** STRIAFREEZETHREE (SR9029): a `title-style` word naming no title style. */ Style,
    /// STRIAXFORMKEYS (SR9049): a placement key word with the playhead off the selected clip, a ninth point, or no point to remove.
    XKey,
    /// STRIAOPACITYKEYS (SR9048): no selected video clip under the playhead, a clip already at the most opacity points, or no point to clear there.
    OpacityKey,
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
            EditError::Marks => "the in mark is not before the out mark", EditError::Hold => "no exact one-frame hold of that length", EditError::Style => "no such title style (size 1-50, align left / centre / right, ink RRGGBB)", // STRIAFREEZELEN (SR916)
            EditError::Marker => "one marker per tick (one is already there, or none is there to remove)",
            EditError::Tracks => "nine tracks of a kind at most (V1..V9, A1..A9)",
            EditError::Crop => "crop is l t r b in 1/1000 with l + r and t + b under 1000",
            EditError::Opacity => "opacity is a percent 0..=100, and video only",
            EditError::Grade => "color is b c s, each in -1000..=1000",
            EditError::Xform => "pip is x y and scale s in 1/1000 with x + s and y + s at most 1000",
            EditError::NoMarks => "no in / out marks set", // STRIAINSERTMARKS (SR905)
            EditError::InsertForm => "insert / overwrite take nothing or <reel> <in> <out>", // STRIAINSERTMARKS (SR905)
            EditError::Crossfade => "the reorder changes an audio crossfade a video transition implies", EditError::Hold => "no exact one-frame hold of that length", // STRIAFREEZELEN (SR916), STRIAFREEZETHREE (SR9029)
            EditError::XKey => "a placement key needs the playhead on the selected clip, at most 8 points, and a point there to remove",
            EditError::OpacityKey => "no opacity key here: a selected video clip under the playhead, at most 8 keys",
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
        Timeline { tb: Timebase { num: 1, den: fps.max(1) as u64 }, sources: Vec::new(), tracks: Vec::new(), audio_xfades: Vec::new(), marks: (None, None), markers: Vec::new() }
    }
    /// The frame rate when the time base is `1/n`.
    pub fn fps(&self) -> Option<u32> {
        if self.tb.num == 1 && self.tb.den > 0 && self.tb.den <= u32::MAX as u64 { Some(self.tb.den as u32) } else { None }
    }
    /// Add a source (builder); its id is its index.
    pub fn add_source(&mut self, reel: &str, duration: i64) -> SourceId {
        let id = SourceId(self.sources.len() as u32);
        self.sources.push(Source { id, reel: String::from(reel), duration, size_policy: SizePolicy::Letterbox, title: None, title_style: TitleStyle::DEFAULT });
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
                opacity_ok(t.kind, c)?; if c.at < 0 || c.src_in < 0 {
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
                grade_ok(t.kind, c)?; if !(GAIN_MIN_CDB..=GAIN_MAX_CDB).contains(&c.gain_cdb) { // STRIACOLORLIFT (SR869): the grade's range and kind rule (file tail)
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
        self.validate_audio_xfades()?; self.validate_markers()?;
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
                    let handle = o.handle_room(self.source(o.src).ok_or(EditError::NoSource)?.duration); // STRIACLIPSPEEDTHREE (SR9031): the room at the outgoing's rate (1x: duration - src_out) (file tail)
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
                let handle = o.handle_room(self.source(o.src).ok_or(EditError::NoSource)?.duration); // STRIACLIPSPEEDTHREE (SR9031): the room at the outgoing's rate (1x: duration - src_out) (file tail)
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
            let handle = o.handle_room(self.source(o.src).ok_or(EditError::NoSource)?.duration); // STRIACLIPSPEEDTHREE (SR9031): the room at the outgoing's rate (1x: duration - src_out) (file tail)
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
            Mode::Insert | Mode::InsertAll => {
                if !cut_clean(t, clip.at) { return Err(EditError::Rate); } split_at(t, clip.at); // STRIACLIPSPEED (SR829): a rated clip cuts on its frame grid (file tail)
                for c in t.clips.iter_mut().filter(|c| c.at >= clip.at) {
                    c.at += clip.len();
                }
            }
            Mode::Overwrite => { if !cut_clean(t, clip.at) || !cut_clean(t, clip.end()) { return Err(EditError::Rate); } carve(t, clip.at, clip.end()) } // STRIACLIPSPEED (SR829)
        }
        t.clips.push(clip);
        sort(t);
        if mode == Mode::InsertAll { tl.ripple_others(track, clip.at, clip.len()); } tl.retarget_transitions(self)?; tl.validate()?;
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
            let handle = o.handle_room(self.source(o.src).ok_or(EditError::NoSource)?.duration); // STRIACLIPSPEEDTHREE (SR9031): the room at the outgoing's rate (1x: duration - src_out) (file tail)
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
                    let handle = c.handle_room(self.source(c.src).ok_or(EditError::NoSource)?.duration); // STRIACLIPSPEEDTHREE (SR9031): the room at the clip's rate (1x: duration - src_out) (file tail)
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
            let out = t.clips.iter().find(|o| o.end() == at).and_then(|o| tl.source(o.src).map(|s| o.handle_room(s.duration))); // STRIACLIPSPEEDTHREE (SR9031): the room at the outgoing's rate (file tail)
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
        tl.shift_from(track, edge, delta, Some(idx)); // STRIAGAPCLOSE (SR857): the shift body moved to the file tail, shared with the gap ops
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
        if slide_follows_rated(&tl.tracks[track], idx, delta) { return Err(EditError::Rate); } let (cut_in, cut_out) = (c.at, c.end()); // STRIACLIPSPEED (SR829): a rated butted neighbour cannot follow tick for tick (file tail); STRIACLIPSPEEDMORE (SR951): only one that cannot land exactly
        let tr = &mut tl.tracks[track];
        if let Some(p) = idx.checked_sub(1).and_then(|i| tr.clips.get_mut(i)).filter(|p| p.end() == cut_in) {
            p.src_out = follow_src(*p, cut_in, p.src_out, delta); // STRIACLIPSPEEDMORE (SR951): through src_at at a rate (1x: src_out + delta) (file tail)
        }
        if let Some(n) = tr.clips.get_mut(idx + 1).filter(|n| n.at == cut_out) {
            n.at = n.at.saturating_add(delta);
            n.src_in = follow_src(Clip { at: cut_out, ..*n }, cut_out, n.src_in, delta); // STRIACLIPSPEEDMORE (SR951): the pre-move clip through src_at (1x: src_in + delta) (file tail)
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

// ---- STRIACLIPSPEED (LEDGER SR829): per-clip speed — rate_q16, the one length road, src_at, set_rate (named tail block) ----
// A clip at `rate_q16` (Q16.16, `0x1_0000` = 1x: the MP4 `elst` media_rate encoding, so `lay_edit_list` lays a rated segment
// as one) shows source ticks `src_in..src_out` over `len() = ceil((src_out - src_in) * 0x1_0000 / rate)` timeline ticks. The
// rounding is CEILING, so every covered tick's source tick is below `src_out` and the last source frame is always shown.
// Timeline tick `t` shows source tick `src_at(t) = src_in + floor((t - at) * rate / 0x1_0000)`. At 1x both are the old
// identity, so a 1x timeline is unchanged everywhere. A rate-0 clip (refused `Rate` by `validate`) measures as 1x. A
// transition's outgoing handle is `src_at` past `end()` (STRIACLIPSPEEDTHREE SR9031). The ops that cut a rated clip (`split`,
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

/// STRIACLIPSPEED: clip `idx` of `t` has a butted neighbour that `slide` would make follow and that neighbour is rated;
/// STRIACLIPSPEEDMORE (SR951): and, followed by `delta` through `follow_src`, that neighbour does not land exactly.
fn slide_follows_rated(t: &Track, idx: usize, delta: i64) -> bool {
    let Some(c) = t.clips.get(idx) else { return false };
    let rated = |o: &Clip| o.rate_q16 != 0x1_0000;
    idx.checked_sub(1).and_then(|i| t.clips.get(i)).is_some_and(|p| p.end() == c.at && rated(p) && Clip { src_out: follow_src(*p, c.at, p.src_out, delta), ..*p }.end() != c.at.saturating_add(delta))
        || t.clips.get(idx + 1).is_some_and(|n| n.at == c.end() && rated(n) && Clip { src_in: follow_src(*n, n.at, n.src_in, delta), at: n.at.saturating_add(delta), ..*n }.end() != n.end())
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
        assert_eq!(b.slide(0, 1, 1).map(|s| s.tracks[0].clips.clone()), Ok(alloc::vec![Clip::new(SourceId(0), 0, 12, 0).at_rate(0x2_0000), Clip::new(SourceId(0), 20, 30, 6)]), "a 2x butted neighbour follows through src_at"); // STRIACLIPSPEEDMORE (SR951): was Err(EditError::Rate)
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

// ---- STRIAMARKERS (LEDGER SR835): named markers on the Timeline — the editor's notebook (named tail block) ----
// A marker is a named tick of the document (`Timeline::markers`, sorted by `at`, at most one per tick, `at >= 0`), edited
// only by the pure ops below (BP-handlers-14) through the editor's ONE commit (so undo / redo / dirty hold for free) and
// kept in the EDL as `* UNA MARKER <tc> "<name>"` (edl.rs tail). Refused by name: a negative tick `EditError::Negative`,
// a taken tick (or, to `validate`, markers out of tick order) `EditError::Marker`.

/// STRIAMARKERS (SR835): one named marker at timeline tick `at`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Marker {
    pub at: i64,
    pub name: String,
    pub colour: u8, // STRIAMARKERJUMPKEYSNEXT (SR984): an index into `MARKER_COLOURS` (0 = SR835's orange) (file tail)
    pub len: i64, // STRIAMARKERJUMPKEYSNEXT (SR984): a range marker's length in ticks (0 = a point marker) (file tail)
}

impl Timeline {
    /// The timeline with a marker `name` at tick `at` (inserted in tick order). Refused: `at < 0` (`Negative`), a
    /// marker already at `at` (`Marker`).
    pub fn mark(&self, at: i64, name: &str) -> Result<Timeline, EditError> {
        if at < 0 {
            return Err(EditError::Negative);
        }
        let i = match self.markers.binary_search_by_key(&at, |m| m.at) {
            Ok(_) => return Err(EditError::Marker),
            Err(i) => i,
        };
        let mut tl = self.clone();
        tl.markers.insert(i, Marker { at, name: String::from(name), colour: 0, len: 0 });
        Ok(tl)
    }
    /// The timeline without the marker at tick `at`; `None` when no marker stands there.
    pub fn unmark(&self, at: i64) -> Option<Timeline> {
        let i = self.markers.binary_search_by_key(&at, |m| m.at).ok()?;
        let mut tl = self.clone();
        tl.markers.remove(i);
        Some(tl)
    }
    /// The marker at tick `at`.
    pub fn marker_at(&self, at: i64) -> Option<&Marker> {
        self.markers.binary_search_by_key(&at, |m| m.at).ok().map(|i| &self.markers[i])
    }
    /// The default name of a new marker: the first `M<n>` (n from 1) no marker carries.
    pub fn marker_name(&self) -> String {
        (1u32..).map(|n| alloc::format!("M{n}")).find(|n| !self.markers.iter().any(|m| &m.name == n)).expect("a free M<n> name")
    }
    /// Every marker at a tick `>= 0` and the ticks strictly ascending.
    fn validate_markers(&self) -> Result<(), EditError> {
        if self.markers.iter().any(|m| m.at < 0 || m.len < 0) { // STRIAMARKERJUMPKEYSNEXT (SR984): a negative length
            return Err(EditError::Negative);
        }
        if self.markers.windows(2).any(|w| w[0].at >= w[1].at) || self.markers.iter().any(|m| usize::from(m.colour) >= MARKER_COLOURS.len()) { // STRIAMARKERJUMPKEYSNEXT (SR984): a colour outside the table
            return Err(EditError::Marker);
        }
        Ok(())
    }
}

#[cfg(test)]
mod striamarkers_tests {
    use super::*;

    fn tl() -> Timeline {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        tl
    }

    /// STRIAMARKERS M3: `mark` inserts in tick order and validates; a taken tick and a negative tick are refused by name.
    #[test]
    fn striamarkers_mark_sorted_insert_and_refusals() {
        let t = tl().mark(5, "B").unwrap().mark(2, "A").unwrap();
        assert_eq!(t.markers, alloc::vec![Marker { at: 2, name: "A".into(), colour: 0, len: 0 }, Marker { at: 5, name: "B".into(), colour: 0, len: 0 }]);
        assert_eq!(t.validate(), Ok(()));
        assert_eq!(t.mark(5, "C"), Err(EditError::Marker));
        assert_eq!(t.mark(-1, "C"), Err(EditError::Negative));
        assert_eq!(t.marker_at(5).map(|m| m.name.as_str()), Some("B"));
        assert_eq!(t.marker_at(4), None);
        assert_eq!(tl().markers, alloc::vec![], "a fresh timeline has none");
    }

    /// STRIAMARKERS M3: `unmark` of nothing is `None`; of a marker removes only it.
    #[test]
    fn striamarkers_unmark_and_default_names() {
        let t = tl().mark(5, "M1").unwrap();
        assert!(t.unmark(4).is_none());
        let u = t.unmark(5).expect("the marker at 5");
        assert!(u.markers.is_empty());
        assert_eq!(u, tl());
        assert_eq!(tl().marker_name(), "M1");
        assert_eq!(t.marker_name(), "M2");
        let only2 = tl().mark(1, "M2").unwrap();
        assert_eq!(only2.marker_name(), "M1", "the first free name");
    }

    /// STRIAMARKERS M3: `validate` refuses markers out of tick order, two at one tick, and a negative one.
    #[test]
    fn striamarkers_validate_refuses_disorder() {
        let mut t = tl();
        t.markers = alloc::vec![Marker { at: 5, name: "a".into(), colour: 0, len: 0 }, Marker { at: 2, name: "b".into(), colour: 0, len: 0 }];
        assert_eq!(t.validate(), Err(EditError::Marker));
        t.markers = alloc::vec![Marker { at: 2, name: "a".into(), colour: 0, len: 0 }, Marker { at: 2, name: "b".into(), colour: 0, len: 0 }];
        assert_eq!(t.validate(), Err(EditError::Marker));
        t.markers = alloc::vec![Marker { at: -2, name: "a".into(), colour: 0, len: 0 }];
        assert_eq!(t.validate(), Err(EditError::Negative));
    }
}

// ---- STRIAROLLEDIT (LEDGER SR851): the roll edit on one cut (named tail block) ----
// A ROLL moves ONE cut: the out of clip A and the in of the butted next clip B move together by `d` ticks, so A grows as
// B shrinks (or the reverse) and the track's length and every other clip stay. It is the two ripple trims composed, with
// no arithmetic of its own: `ripple_trim_out(A, src_out + d)` grows A and carries B, the cut's transition and everything
// later on the track by +d; `ripple_trim_in(B, src_in + d)` shrinks B at its kept (moved) start and carries everything
// past B's end back by -d. Each op fits the cut's transition to its room (`fit_trimmed`, NLE3ETRIMGROW's `fit_want`), as
// `slide` does on its two cuts, so a roll and its roll-back give the original Timeline. The plain trims cannot compose
// it (an `Overlap` or a gap between them, and `retarget_transitions` refuses a cut whose source edges moved). Refused by
// the two ops' names (`OutOfSource`, `Empty`, `Negative`, `Handle`), and `NoClip` when no clip is butted at A's end.
impl Timeline {
    /// STRIAROLLEDIT: roll the cut at the out edge of clip `idx` of `track` by `d` ticks: A's `src_out` and the butted
    /// next clip's `at` and `src_in` all move by `d`; nothing else on the timeline moves.
    pub fn roll(&self, track: usize, idx: usize, d: i64) -> Result<Timeline, EditError> {
        let t = self.tracks.get(track).ok_or(EditError::NoTrack)?;
        let a = *t.clips.get(idx).ok_or(EditError::NoClip)?;
        let b = *t.clips.get(idx + 1).filter(|b| b.at == a.end()).ok_or(EditError::NoClip)?;
        self.ripple_trim_out(track, idx, a.src_out.saturating_add(d))?.ripple_trim_in(track, idx + 1, b.src_in.saturating_add(d))
    }
}

// ---- STRIACOPYPASTE (LEDGER SR854): paste through place (named tail block) ----
// The editor's `paste` lays its clipboard through THE placement road, `Timeline::place` in `Mode::Overwrite`, item by item
// on the running value. A paste never overwrites: an item whose `[at, end)` meets a clip already on its track is refused
// `EditError::Overlap` BEFORE `place` (whose carve of the empty hole is then a no-op), so the hole is always empty.

impl Timeline {
    /// Lay `items` (`(track, the kind it was copied from, clip at its landing tick)`) as ONE edit. Refused by name, the
    /// whole paste together: none `EditError::NoClip`; a missing track `NoTrack`; a track of the other kind `Kind`; an
    /// item meeting a clip already on its track `Overlap`; anything `place` itself refuses. STRIAPASTEINSERT (SR896): `mode` None = that never-overwrite paste; `Some(Mode::Insert)` ripples each target track, `Some(Mode::Overwrite)` carves (no `Overlap`); the items walk ascending by `(track, at)`.
    pub fn paste_as(&self, items: &[(usize, Kind, Clip)], mode: Option<Mode>) -> Result<Timeline, EditError> {
        if items.is_empty() {
            return Err(EditError::NoClip);
        }
        let mut tl = self.clone(); let mut v = items.to_vec(); v.sort_unstable_by_key(|e| (e.0, e.2.at)); let items = &v[..]; // STRIAPASTEINSERT (SR896): ascending per track, so an insert never moves an earlier pasted item
        for &(track, kind, clip) in items {
            let t = tl.tracks.get(track).ok_or(EditError::NoTrack)?;
            if t.kind != kind {
                return Err(EditError::Kind);
            }
            if mode.is_none() && t.clips.iter().any(|c| c.at < clip.end() && clip.at < c.end()) {
                return Err(EditError::Overlap);
            }
            tl = tl.place(track, clip, mode.unwrap_or(Mode::Overwrite))?;
        }
        Ok(tl)
    }
}

// ---- STRIAGAPCLOSE (LEDGER SR857): close every gap of a track, insert a gap at a tick (named tail block) ----
// A gap is the space between two clips' `at`s (or before the first clip). `close_gaps` closes every one on ONE track, the
// leading gap (tick 0 to the first clip) included; `insert_gap` opens `n` ticks at a tick (a clip strictly spanning it splits,
// `place` Insert's rule) by pushing the next clip start and everything after it. Both move clips through ONE shift body,
// `shift_from` (`ripple_trim`'s three loops moved here verbatim): every clip, transition and explicit audio crossfade of the
// track at or past an edge moves by the delta, so a transition or crossfade on a clip that moves goes with it. Other tracks
// never move. A black transition that would gain a neighbour (a from-black whose gap closes, a to-black whose gap closes) and
// a gap opened on a transitioned cut are refused by `validate` by name (`Transition`), never dropped.
impl Timeline {
    /// The one direct shift (NLE3ETRIMSTRETCH's body): every clip of `track` but `keep` at or past `edge`, the track's
    /// transitions and its explicit audio crossfades at or past `edge`, move by `delta` ticks. No matcher, no validate.
    fn shift_from(&mut self, track: usize, edge: i64, delta: i64, keep: Option<usize>) {
        let tr = &mut self.tracks[track];
        for (i, c) in tr.clips.iter_mut().enumerate() {
            if Some(i) != keep && c.at >= edge {
                c.at += delta;
            }
        }
        for x in tr.transitions.iter_mut().filter(|x| x.at >= edge) {
            x.at += delta;
        }
        for x in self.audio_xfades.iter_mut().filter(|x| x.track == track && x.at >= edge) {
            x.at += delta;
        }
    }

    /// STRIAGAPCLOSE: close every gap on `track` (the leading one included): each clip starting after the previous clip's
    /// end (0 for the first) moves back to it with everything after it. No gap: an equal value.
    pub fn close_gaps(&self, track: usize) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        tl.close_gaps_on(track, false)?; // STRIAGAPSTHREE (SR9028): SR857's loop (with SR909's black rule) is the one body `close_gaps_on` (file tail)
        tl.validate()?;
        Ok(tl)
    }

    /// STRIAGAPCLOSE: open `n` ticks of gap at tick `at` of `track`: a clip strictly spanning `at` splits there, then the
    /// first clip starting at or after `at` and everything after it move `n` later. Refused: `n < 1` (`Empty`), `at < 0`
    /// (`Negative`), no clip at or after `at` (`NoClip`: nothing to push), a bad track (`NoTrack`).
    pub fn insert_gap(&self, track: usize, at: i64, n: i64) -> Result<Timeline, EditError> {
        if n < 1 {
            return Err(EditError::Empty);
        }
        if at < 0 {
            return Err(EditError::Negative);
        }
        let mut tl = self.clone();
        let t = tl.tracks.get_mut(track).ok_or(EditError::NoTrack)?;
        split_at(t, at);
        let edge = t.clips.iter().map(|c| c.at).find(|&a| a >= at).ok_or(EditError::NoClip)?;
        tl.shift_from(track, edge, n, None);
        tl.validate()?;
        Ok(tl)
    }
}

// ---- STRIATRACKADDREMOVE (LEDGER SR856): add / remove a track at its canonical slot, the xfade indices shifted (named tail block) ----
// A new track goes to its CANONICAL slot (after the last track of its kind; with none, video first and audio last), so a
// timeline built V1..Vn, A1..An stays so and an EDL save / reopen keeps every track index. The one carrier of a track index
// inside the Timeline is `audio_xfades[].track`: both ops shift it (a removed track's own records go with it) and validate.
// The editor's selection, set and selected transition are remapped by `EditorState::add_track` / `remove_track`.

/// STRIATRACKADDREMOVE: the most tracks of one kind (CMX 3600 channels `V`..`V9`, `A`..`A9`).
pub const TRACKS_MAX: usize = 9;

impl Timeline {
    /// The index of the `num`th (1-based) track of `kind`: the inverse of [`Timeline::track_number`].
    pub fn track_of(&self, kind: Kind, num: usize) -> Option<usize> {
        (0..self.tracks.len()).filter(|&i| self.tracks[i].kind == kind).nth(num.checked_sub(1)?)
    }
    /// A track's channel word as the EDL writes it: `V`, `V2`.., `A`, `A2`.. (`None`: no such track).
    pub fn track_word(&self, track: usize) -> Option<String> {
        let num = self.track_number(track)?;
        let k = if self.tracks[track].kind == Kind::Video { "V" } else { "A" };
        Some(if num == 1 { String::from(k) } else { alloc::format!("{k}{num}") })
    }
    /// Where a new track of `kind` goes: after the last track of its kind; with none, video at 0 and audio at the end.
    pub fn track_slot(&self, kind: Kind) -> usize {
        match self.tracks.iter().rposition(|t| t.kind == kind) {
            Some(i) => i + 1,
            None if kind == Kind::Video => 0,
            None => self.tracks.len(),
        }
    }
    /// The timeline with an empty track of `kind` at [`Timeline::track_slot`]; every explicit audio crossfade at or after
    /// the slot moves up one track. Refused: a tenth track of `kind` (`EditError::Tracks`).
    pub fn insert_track(&self, kind: Kind) -> Result<Timeline, EditError> {
        if self.tracks.iter().filter(|t| t.kind == kind).count() >= TRACKS_MAX {
            return Err(EditError::Tracks);
        }
        let slot = self.track_slot(kind);
        let mut tl = self.clone();
        let i = tl.add_track(kind);
        let t = tl.tracks.remove(i);
        tl.tracks.insert(slot, t);
        for x in tl.audio_xfades.iter_mut().filter(|x| x.track >= slot) {
            x.track += 1;
        }
        tl.validate()?;
        Ok(tl)
    }
    /// The timeline without track `track`: its clips and transitions go with it, its explicit audio crossfades are dropped
    /// and every later one moves down one track. Refused: no such track (`EditError::NoTrack`).
    pub fn remove_track(&self, track: usize) -> Result<Timeline, EditError> {
        if track >= self.tracks.len() {
            return Err(EditError::NoTrack);
        }
        let mut tl = self.clone();
        tl.tracks.remove(track);
        tl.audio_xfades.retain(|x| x.track != track);
        for x in tl.audio_xfades.iter_mut().filter(|x| x.track > track) {
            x.track -= 1;
        }
        tl.validate()?;
        Ok(tl)
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

// ---- STRIACROP (LEDGER SR867): the per-clip crop, its geometry and THE crop pass (named tail block) ----
// A video clip's crop keeps a rect of the frame its render step holds (each side in 1/1000 of the frame) and places that
// rect back into the frame by the clip reel's size policy (`SizePolicy::place`, THE geometry), sampled by yuv_core::scale's
// ONE `nearest_rows` (generic over the sample: RGBA quads on the host, packed pixels elsewhere): the crop comes BEFORE the
// size policy. `Crop::NONE` is no crop (the render skips the pass: byte-identical frames). Stored in the EDL as
// `* UNA CROP <l> <t> <r> <b>` under its event (edl.rs). The kernel Player crops in its frame road
// (vplay.rs, NLE-CROP-KERNEL SR9010).

/// A clip's crop: the left / top / right / bottom bands cut away, each in 1/1000 of the frame's width or height.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Crop {
    pub l: u16,
    pub t: u16,
    pub r: u16,
    pub b: u16,
}

impl Crop {
    /// No crop.
    pub const NONE: Crop = Crop { l: 0, t: 0, r: 0, b: 0 };
    /// Some picture is kept: `l + r < 1000` and `t + b < 1000`.
    pub fn ok(self) -> bool {
        (self.l as u32 + self.r as u32) < 1000 && (self.t as u32 + self.b as u32) < 1000
    }
    /// The rect a `w x h` frame keeps (at least 1 x 1, inside the frame; `w`, `h` > 0).
    pub fn rect(self, w: usize, h: usize) -> yuv_core::scale::Rect {
        let side = |n: usize, a: u16, b: u16| {
            let x = (n * a as usize / 1000).min(n - 1);
            let k = n.saturating_sub(x + n * b as usize / 1000).max(1).min(n - x);
            (x, k)
        };
        let ((x, w), (y, h)) = (side(w, self.l, self.r), side(h, self.t, self.b));
        yuv_core::scale::Rect { w, h, x, y }
    }
}

/// THE crop pass: the `w x h` frame `px` (row-major samples) with `crop`'s rect placed back into `w x h` by `fit`
/// ([`SizePolicy::place`]; Adopt places as Letterbox), the rest `bar`. A frame shorter than `w * h`, or an empty one, comes
/// back as it was.
pub fn crop_frame<T: Copy>(px: &[T], w: usize, h: usize, crop: Crop, fit: SizePolicy, bar: T) -> Vec<T> {
    if w == 0 || h == 0 || px.len() < w * h {
        return px.to_vec();
    }
    let mut out = alloc::vec![bar; w * h];
    place_frame(px, w, h, crop, fit, &mut out, w, yuv_core::scale::Rect { w, h, x: 0, y: 0 }); // STRIAPOSITIONSCALE (SR879): the one placement fn (file tail)
    out
}

impl Timeline {
    /// The same timeline with clip `idx` of `track` cropped by `crop` ([`Crop::NONE`] clears). Refused: no such track
    /// (`NoTrack`) or clip (`NoClip`), an audio track (`Kind`), a crop keeping no picture (`Crop`).
    pub fn set_crop(&self, track: usize, idx: usize, crop: Crop) -> Result<Timeline, EditError> {
        if self.tracks.get(track).ok_or(EditError::NoTrack)?.kind != Kind::Video {
            return Err(EditError::Kind);
        }
        if !crop.ok() {
            return Err(EditError::Crop);
        }
        let mut tl = self.clone();
        clip_mut(&mut tl, track, idx)?.crop = crop;
        Ok(tl)
    }
    /// The crop of the frame a video step on `track` at tick `t` holds: the clip covering `t` (`outgoing` false), or, inside
    /// a transition on that track (`outgoing` true), the clip that ends at the transition's cut. None found: [`Crop::NONE`].
    pub fn crop_at(&self, track: usize, t: i64, outgoing: bool) -> Crop {
        let Some(tr) = self.tracks.get(track) else { return Crop::NONE };
        let c = if outgoing {
            tr.transitions.iter().find(|x| x.at <= t && t < x.at + x.dur).and_then(|x| tr.clips.iter().find(|c| c.end() == x.at))
        } else {
            tr.clips.iter().find(|c| c.covers(t))
        };
        c.map_or(Crop::NONE, |c| c.crop)
    }
}

// ---- STRIAOPACITY (LEDGER SR868): per-clip opacity, the model and its one op (named tail block) ----
// A video clip's `opacity` (percent) below [`OPAQUE`] lets the highest video track under it show through: the schedule
// gives that step a Dissolve `VideoMix` of the track below at `opacity / 100` (`schedule::opacity_mix`), the one mix both
// renderers already run. `* UNA ALPHA <pct>` in the EDL; the editor's `opacity <pct>` word.

/// STRIAOPACITY: full opacity (percent), every clip's default.
pub const OPAQUE: u8 = 100;

/// STRIAOPACITY: `validate`'s rule: an opacity is at most [`OPAQUE`], and only a video clip is ever below it.
fn opacity_ok(kind: Kind, c: &Clip) -> Result<(), EditError> {
    if c.opacity > OPAQUE || (c.opacity != OPAQUE && kind == Kind::Audio) || !c.opacity_keys.valid(kind) {
        return Err(EditError::Opacity);
    }
    Ok(())
}

impl Clip {
    /// STRIAOPACITY: the same clip at opacity `pct` (percent).
    pub fn with_opacity(self, pct: u8) -> Clip {
        Clip { opacity: pct, ..self }
    }
}

impl Timeline {
    /// STRIAOPACITY: every clip in `picks` (`(track, index)`) set to opacity `pct` (percent) as ONE edit; refused by name
    /// (`EditError::Opacity`) above [`OPAQUE`] or on an audio clip.
    pub fn set_opacity(&self, picks: &[(usize, usize)], pct: u8) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        for &(track, idx) in picks {
            clip_mut(&mut tl, track, idx)?.opacity = pct;
        }
        tl.validate()?;
        Ok(tl)
    }
}

// ---- STRIACLIPLINK (LEDGER SR863): the clip link group, its mates and the linked split (named tail block) ----
// `Clip::group` links clips (0 = unlinked): the editor's selection takes a clip's whole group (`EditorState::select`), so
// the set ops (`move_many`, `ripple_delete_many`, `lift_many`) carry it. `split_linked` cuts every member spanning the
// tick: the heads keep the id, the new tails share ONE fresh id (two linked pairs, not one group of four). Every other op
// carries the id through unchanged (`split` / `place` keep it on both halves).
impl Timeline {
    /// STRIACLIPLINK: a group id no clip carries (the largest + 1).
    pub fn next_group(&self) -> u32 {
        self.tracks.iter().flat_map(|t| t.clips.iter()).map(|c| c.group).max().unwrap_or(0).saturating_add(1)
    }
    /// STRIACLIPLINK: every `(track, idx)` sharing clip `idx` of `track`'s nonzero group, itself included, in order (the
    /// clip alone for group 0; none for a bad pick).
    pub fn mates(&self, track: usize, idx: usize) -> Vec<(usize, usize)> {
        let Some(c) = self.tracks.get(track).and_then(|t| t.clips.get(idx)) else { return Vec::new() };
        if c.group == 0 {
            return alloc::vec![(track, idx)];
        }
        let mut v = Vec::new();
        for (t, tr) in self.tracks.iter().enumerate() {
            for (i, o) in tr.clips.iter().enumerate() {
                if o.group == c.group {
                    v.push((t, i));
                }
            }
        }
        v
    }
    /// STRIACLIPLINK: every clip in `picks` given group `g` (0 unlinks); refused `NoClip` / `NoTrack` like the set ops.
    pub fn set_group(&self, picks: &[(usize, usize)], g: u32) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        for (t, i) in picks_desc(self, picks)? {
            tl.tracks[t].clips[i].group = g;
        }
        tl.validate()?;
        Ok(tl)
    }
    /// STRIACLIPLINK: [`Timeline::split`] at `t` on `track`; when the clip spanning `t` there is linked, every track
    /// holding a member of its group strictly spanning `t` is split too, and the new tails share one fresh group.
    pub fn split_linked(&self, track: usize, t: i64) -> Result<Timeline, EditError> {
        let g = self.tracks.get(track).ok_or(EditError::NoTrack)?.clips.iter().find(|c| c.at < t && t < c.end()).map_or(0, |c| c.group);
        if g == 0 {
            return self.split(track, t);
        }
        let mut tl = self.split(track, t)?;
        let mut cut = alloc::vec![track];
        for (k, tr) in self.tracks.iter().enumerate() {
            if k != track && tr.clips.iter().any(|c| c.group == g && c.at < t && t < c.end()) {
                tl = tl.split(k, t)?;
                cut.push(k);
            }
        }
        let fresh = tl.next_group();
        for k in cut {
            if let Some(c) = tl.tracks[k].clips.iter_mut().find(|c| c.at == t && c.group == g) {
                c.group = fresh;
            }
        }
        tl.validate()?;
        Ok(tl)
    }
}

// ---- STRIACOLORLIFT (LEDGER SR869): the per-clip grade, its op and THE per-pixel pass (named tail block) ----
// A clip's colour grade: brightness / contrast / saturation, each in 1/1000 in `GRADE_MIN..=GRADE_MAX` (0 = unchanged).
// [`Grade::apply`] is THE per-pixel pass (integer, straight RGBA, alpha untouched): saturation about the pixel's luma Y' by
// yuv_core's ONE RGB -> Y' weights (`rgb_coeffs(KR_KB_601, true)`, the export converter's derivation; no second table), then
// contrast about 128, then brightness as an offset of `255 b / 1000`, one clamp at the end. Stria's render calls it on the
// RGBA a video step holds (`handlers/stria/src/nle.rs`, its own clip's and a transition's outgoing clip's); an IDENTITY
// grade is no pass at all. Video clips only (`validate`: a graded audio clip is `EditError::Kind`). The kernel Player
// grades it too (vplay.rs `edl_grade` / `edl_graded`, NLE-GRADE-KERNEL BUILT by STRIAGRADENEXT SR992).

/// A clip's colour grade, each field in 1/1000 (`GRADE_MIN..=GRADE_MAX`; 0 = unchanged).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Grade {
    pub brightness: i16,
    pub contrast: i16,
    pub saturation: i16,
}

/// The range each [`Grade`] field may carry.
pub const GRADE_MIN: i16 = -1000;
pub const GRADE_MAX: i16 = 1000;

impl Grade {
    /// No change: the render runs no pass.
    pub const IDENTITY: Grade = Grade { brightness: 0, contrast: 0, saturation: 0 };

    /// Every field inside `GRADE_MIN..=GRADE_MAX`.
    pub fn in_range(self) -> bool {
        [self.brightness, self.contrast, self.saturation].iter().all(|v| (GRADE_MIN..=GRADE_MAX).contains(v))
    }

    /// THE per-pixel pass over straight RGBA `rgba` (alpha untouched). Exact at [`Grade::IDENTITY`].
    pub fn apply(self, rgba: &mut [u8]) {
        let (ky, _, _, _) = yuv_core::rgb_coeffs(yuv_core::KR_KB_601, true);
        let (b, c, s) = (self.brightness as i64, self.contrast as i64, self.saturation as i64);
        let off = grade_rdiv(255 * b, 1000);
        for px in rgba.chunks_exact_mut(4) {
            let (r, g, bl) = (px[0] as i64, px[1] as i64, px[2] as i64);
            let y = (ky[0] * r + ky[1] * g + ky[2] * bl + 32768) >> 16;
            for ch in px.iter_mut().take(3) {
                let v = *ch as i64;
                let v = y + grade_rdiv((v - y) * (1000 + s), 1000);
                let v = 128 + grade_rdiv((v - 128) * (1000 + c), 1000);
                *ch = (v + off).clamp(0, 255) as u8;
            }
        }
    }
}

/// `n / d` (`d > 0`) rounded half away from zero.
fn grade_rdiv(n: i64, d: i64) -> i64 {
    if n >= 0 { (2 * n + d) / (2 * d) } else { -((-2 * n + d) / (2 * d)) }
}

/// `validate`'s grade rule: every field in range (`EditError::Grade`), and only a video clip graded (`EditError::Kind`).
fn grade_ok(kind: Kind, c: &Clip) -> Result<(), EditError> {
    if !c.grade.in_range() {
        return Err(EditError::Grade);
    }
    if kind != Kind::Video && c.grade != Grade::IDENTITY {
        return Err(EditError::Kind);
    }
    Ok(())
}

impl Timeline {
    /// The timeline with clip `idx` of `track` graded `g` (refused by `validate`: `EditError::Grade`, `EditError::Kind`).
    pub fn set_grade(&self, track: usize, idx: usize, g: Grade) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        clip_mut(&mut tl, track, idx)?.grade = g;
        tl.validate()?;
        Ok(tl)
    }

    /// The grade of the clip a video step on `track` shows at tick `t`: its own clip (`outgoing` false: the clip covering
    /// `t`), or a transition's outgoing clip (`outgoing` true: the last clip on the track ending at or before `t`, the cut's
    /// outgoing for a plain transition and for one to black). No track or no such clip: [`Grade::IDENTITY`].
    pub fn grade_at(&self, track: Option<usize>, t: i64, outgoing: bool) -> Grade {
        let Some(tr) = track.and_then(|i| self.tracks.get(i)) else { return Grade::IDENTITY };
        let c = if outgoing { tr.clips.iter().filter(|c| c.end() <= t).max_by_key(|c| c.end()) } else { tr.clips.iter().find(|c| c.covers(t)) };
        c.map_or(Grade::IDENTITY, |c| c.grade)
    }
}

// ---- STRIAPAN (LEDGER SR861): per-clip pan (named tail block) ----
// A clip carries [`Clip::pan_cq8`] (0 = centre, the default every constructor gives); `set_pan` is `set_gain`'s shape, the
// editor's `pan <x>` word commits it, the EDL writes it as `* UNA PAN <cq8>`, and `schedule::audio_runs` copies it onto
// each [`schedule::Layer`]. The ONE law is [`schedule::pan_q16`]; Stria's render and the kernel Player apply it through
// [`schedule::pan_mix_q16`] (STRIAKPAN SR9015).
impl Timeline {
    /// STRIAPAN: set a clip's pan (`pan_cq8`, -128..=127; every i8 is a pan, so only the clip's address is refused).
    pub fn set_pan(&self, track: usize, idx: usize, pan_cq8: i8) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        clip_mut(&mut tl, track, idx)?.pan_cq8 = pan_cq8;
        tl.validate()?;
        Ok(tl)
    }
}

// ---- STRIATRACKREORDER (LEDGER SR877): swap a track with its same-kind neighbour (named tail block) ----
// `track-up` / `track-down` reorder tracks WITHIN a kind: the track swaps with the NEAREST track of the same kind at a lower
// (up: toward row 0 on screen, which is DOWN in the video composite, `schedule::top` taking the highest index) or higher
// index, other-kind tracks skipped. The kind in every index slot stays fixed, so `edl::write`'s per-kind channel numbers
// and `edl::parse`'s canonical rebuild (all video, then audio) give the same order back. A track carries its clips and its
// transitions; the one other model field naming a track index, `audio_xfades[].track`, is remapped by `track_swapped` and
// re-sorted. `validate` stays the one gate. Refused: no such track, or no same-kind track that way (`EditError::NoTrack`).

/// STRIATRACKREORDER: where index `i` lands when tracks `a` and `b` swap.
pub fn track_swapped(i: usize, a: usize, b: usize) -> usize {
    if i == a {
        b
    } else if i == b {
        a
    } else {
        i
    }
}

impl Timeline {
    /// STRIATRACKREORDER: the nearest track of `track`'s kind at a lower (`up`) or higher index (`EditError::NoTrack`: none,
    /// or no such track).
    pub fn track_neighbour(&self, track: usize, up: bool) -> Result<usize, EditError> {
        let k = self.tracks.get(track).ok_or(EditError::NoTrack)?.kind;
        let same = |&j: &usize| self.tracks[j].kind == k;
        let j = if up { (0..track).rev().find(same) } else { (track + 1..self.tracks.len()).find(same) };
        j.ok_or(EditError::NoTrack)
    }
    /// STRIATRACKREORDER: the timeline with `track` swapped with its same-kind neighbour ([`Timeline::track_neighbour`]).
    pub fn swap_track(&self, track: usize, up: bool) -> Result<Timeline, EditError> {
        let j = self.track_neighbour(track, up)?;
        let mut tl = self.clone();
        tl.tracks.swap(track, j);
        for x in tl.audio_xfades.iter_mut() {
            x.track = track_swapped(x.track, track, j);
        }
        tl.audio_xfades.sort_by_key(|x| (x.track, x.at));
        tl.validate()?; self.same_implied_fades(&tl, track, j)?; // STRIATRACKORDERKEYS (SR922): a reorder that re-times an implied audio fade is refused (file tail)
        Ok(tl)
    }
}

// ---- STRIAMARKERRENAME (LEDGER SR895): rename one marker / remove them all (named tail block) ----
// Two more pure ops on `Timeline::markers` (BP-handlers-14), reached from the editor's `mark <name>` / `unmark all` words
// through its ONE commit. `Timeline::mark` keeps refusing a taken tick: the EDL reader relies on it (a duplicate tick in a
// file is a syntax error), so the rename is its own op.

impl Timeline {
    /// The timeline with the marker at tick `at` renamed `name` (its tick and order kept); `None` when no marker stands
    /// there or it already carries `name`.
    pub fn rename_marker(&self, at: i64, name: &str) -> Option<Timeline> {
        let i = self.markers.binary_search_by_key(&at, |m| m.at).ok()?;
        if self.markers[i].name == name {
            return None;
        }
        let mut tl = self.clone();
        tl.markers[i].name = String::from(name);
        Some(tl)
    }
    /// The timeline with no marker; `None` when it has none.
    pub fn unmark_all(&self) -> Option<Timeline> {
        if self.markers.is_empty() {
            return None;
        }
        let mut tl = self.clone();
        tl.markers.clear();
        Some(tl)
    }
}

#[cfg(test)]
mod striamarkerrename_tests {
    use super::*;

    fn tl() -> Timeline {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        tl
    }

    /// STRIAMARKERRENAME M3: `rename_marker` renames only the marker at that tick (ticks and order kept, `validate` Ok);
    /// an empty tick and the same name are `None`.
    #[test]
    fn striamarkerrename_rename_marker_keeps_the_tick() {
        let t = tl().mark(2, "A").unwrap().mark(5, "B").unwrap();
        let r = t.rename_marker(5, "Intro").expect("the marker at 5");
        assert_eq!(r.markers, alloc::vec![Marker { at: 2, name: "A".into() }, Marker { at: 5, name: "Intro".into() }]);
        assert_eq!(r.validate(), Ok(()));
        assert!(t.rename_marker(4, "X").is_none(), "no marker at 4");
        assert!(t.rename_marker(5, "B").is_none(), "already named B");
        assert_eq!(t.mark(5, "C"), Err(EditError::Marker), "the pure mark still refuses a taken tick");
    }

    /// STRIAMARKERRENAME M3: `unmark_all` clears every marker; `None` with none.
    #[test]
    fn striamarkerrename_unmark_all() {
        let t = tl().mark(2, "A").unwrap().mark(5, "B").unwrap();
        let u = t.unmark_all().expect("two markers");
        assert!(u.markers.is_empty());
        assert_eq!(u, tl());
        assert!(tl().unmark_all().is_none());
    }
}

// ---- STRIAPASTEINSERT (LEDGER SR896): paste through place in a mode (named tail block) ----
// SR854's walk took a mode (`Timeline::paste_as`); the plain paste stays THE never-overwrite paste, one walk for all three.

impl Timeline {
    /// SR854's paste: `paste_as` with no mode (an item meeting a clip is refused `EditError::Overlap`).
    pub fn paste(&self, items: &[(usize, Kind, Clip)]) -> Result<Timeline, EditError> {
        self.paste_as(items, None)
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


// ---- STRIAPOSITIONSCALE (LEDGER SR879): the per-clip position / scale, THE placement fn and the picture-in-picture composite (named tail block) ----
// A video clip's `xform` places its frame into a rect of the output frame: top-left (`x`, `y`) and side scale `s`, each in
// 1/1000 of the frame (uniform: the rect keeps the frame's aspect), inside the frame. [`Xform::NONE`] is the whole frame (no
// placement: byte-identical frames). A transformed clip rides STRIAOPACITY's two-layer schedule (the track below is the
// step's outgoing `VideoMix`); the render's [`xform_frame`] places the top into the rect over the lower frame through ONE
// [`place_frame`] (the crop pass's own placement, a destination rect instead of the whole frame) and mixes only the rect
// with `vmix::mix_step` when not opaque. Stored in the EDL as `* UNA XFORM <x> <y> <s>` under its event (edl.rs). The
// kernel Player composes a placed step through the same [`xform_frame`], by [`Timeline::place_step`] (STRIAKXFORM SR9011).

/// A clip's placement: top-left `x`, `y` and side scale `s`, each in 1/1000 of the frame's width / height.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Xform {
    pub x: u16,
    pub y: u16,
    pub s: u16,
}

impl Default for Xform {
    fn default() -> Xform {
        Xform::NONE
    }
}

impl Xform {
    /// The whole frame (no placement).
    pub const NONE: Xform = Xform { x: 0, y: 0, s: 1000 };
    /// The `pip` word's preset: the bottom-right quarter, 1/40 of the frame in from the edges.
    pub const PIP: Xform = Xform { x: 725, y: 725, s: 250 };
    /// The rect stays inside the frame: `1 <= s`, `x + s <= 1000`, `y + s <= 1000`.
    pub fn ok(self) -> bool {
        self.s >= 1 && self.x as u32 + self.s as u32 <= 1000 && self.y as u32 + self.s as u32 <= 1000
    }
    /// The rect of a `w x h` frame (at least 1 x 1, inside the frame; `w`, `h` > 0).
    pub fn rect(self, w: usize, h: usize) -> yuv_core::scale::Rect {
        let side = |n: usize, o: u16| {
            let x = (n * o as usize / 1000).min(n - 1);
            (x, (n * self.s as usize / 1000).max(1).min(n - x))
        };
        let ((x, w), (y, h)) = (side(w, self.x), side(h, self.y));
        yuv_core::scale::Rect { w, h, x, y }
    }
}

/// THE placement: `crop`'s rect of the `w x h` frame `px` placed by `fit` ([`SizePolicy::place`]; Adopt places as
/// Letterbox) into the rect `dst` of `out` (rows `ow` samples wide), sampled by yuv_core's ONE `nearest_rows`; the rest of
/// `out` is left as it was. A frame shorter than `w * h`, an empty one, an empty `dst`, or a `dst` past `out` places nothing.
#[allow(clippy::too_many_arguments)]
pub fn place_frame<T: Copy>(px: &[T], w: usize, h: usize, crop: Crop, fit: SizePolicy, out: &mut [T], ow: usize, dst: yuv_core::scale::Rect) {
    if w == 0 || h == 0 || px.len() < w * h || dst.w == 0 || dst.h == 0 || dst.x + dst.w > ow || (dst.y + dst.h) * ow > out.len() {
        return;
    }
    let k = crop.rect(w, h);
    let p = fit.place(k.w, k.h, dst.w, dst.h);
    let (sx, sy) = (k.x + p.src.x, k.y + p.src.y);
    let pic = yuv_core::Picture { w: p.src.w, h: p.src.h, y: &px[sy * w + sx..], u: &[], v: &[], y_stride: w, uv_stride: 0, ss: (0, 0), mono: true, bit_depth: 8, full: true };
    if p.dst.w == 0 || p.dst.h == 0 || !yuv_core::scale::fits(&pic) {
        return;
    }
    let mut ry = alloc::vec![px[0]; p.dst.w];
    yuv_core::scale::nearest_rows(&pic, p.dst.w, p.dst.h, &mut ry, &mut [], &mut [], |r, row| {
        let o = (dst.y + p.dst.y + r) * ow + dst.x + p.dst.x;
        out[o..o + p.dst.w].copy_from_slice(&row.y[..p.dst.w]);
    });
}

/// THE picture-in-picture composite: the step's `top` frame (`w x h` RGBA, its `crop` placed by `fit`) in `xf`'s rect over
/// the `under` frame (the track below, or black), at the top's weight `t` (the schedule's `VideoMix::t`, opacity / 100).
/// Opaque (`t >= 1`): the top placed straight into the rect. Otherwise only the rect passes through `vmix::mix_step`'s
/// Dissolve (the top placed into a copy of the rect's lower pixels, mixed over them). Outside the rect: `under`, byte for
/// byte. Frames shorter than `w * h` come back as `under`.
#[allow(clippy::too_many_arguments)]
pub fn xform_frame(top: &[[u8; 4]], under: &[[u8; 4]], w: usize, h: usize, crop: Crop, fit: SizePolicy, xf: Xform, t: schedule::Frac) -> Vec<[u8; 4]> {
    let mut out = under.to_vec();
    if w == 0 || h == 0 || top.len() < w * h || under.len() < w * h {
        return out;
    }
    let r = xf.rect(w, h);
    if t.num >= t.den {
        place_frame(top, w, h, crop, fit, &mut out, w, r);
        return out;
    }
    let mut low: Vec<[u8; 4]> = (0..r.h).flat_map(|y| under[(r.y + y) * w + r.x..(r.y + y) * w + r.x + r.w].iter().copied()).collect();
    let mut placed = low.clone();
    place_frame(top, w, h, crop, fit, &mut placed, r.w, yuv_core::scale::Rect { w: r.w, h: r.h, x: 0, y: 0 });
    vmix::mix_step(TransitionKind::Dissolve, t, low.as_flattened_mut(), placed.as_flattened(), r.w as u32, r.h as u32);
    for y in 0..r.h {
        let o = (r.y + y) * w + r.x;
        out[o..o + r.w].copy_from_slice(&low[y * r.w..(y + 1) * r.w]);
    }
    out
}

impl Timeline {
    /// The same timeline with clip `idx` of `track` placed by `xf` ([`Xform::NONE`] clears). Refused: no such track
    /// (`NoTrack`) or clip (`NoClip`), an audio track (`Kind`), a rect off the frame (`Xform`).
    pub fn set_xform(&self, track: usize, idx: usize, xf: Xform) -> Result<Timeline, EditError> {
        if self.tracks.get(track).ok_or(EditError::NoTrack)?.kind != Kind::Video {
            return Err(EditError::Kind);
        }
        if !xf.ok() {
            return Err(EditError::Xform);
        }
        let mut tl = self.clone();
        clip_mut(&mut tl, track, idx)?.xform = xf;
        Ok(tl)
    }
    /// The placement of the clip covering tick `t` on `track` ([`Xform::NONE`] when none).
    pub fn xform_at(&self, track: usize, t: i64) -> Xform {
        self.tracks.get(track).and_then(|tr| tr.clips.iter().find(|c| c.covers(t))).map_or(Xform::NONE, |c| c.xform_at(t)) // STRIAXFORMKEYS (SR9049): a keyed clip's placement at the tick (file tail)
    }
}

// ---- STRIALINKAUTO (LEDGER SR911): the opened media file's V1 / A1 pairs linked (named tail block) ----
// `stria::nle::Sources::from_media` lays one file's picture and sound on their own tracks; `link_av` gives each video
// clip and the audio clip laid at the same tick for the same length ONE fresh group (SR863's `Clip::group`), one id per
// pair, so selecting a segment takes its sound. Only `group` changes; an unpaired or already linked clip is left alone.
impl Timeline {
    /// STRIALINKAUTO: the first video and first audio track's equal clips (same `at`, same `len`) linked pair by pair.
    pub fn link_av(&self) -> Timeline {
        let mut tl = self.clone();
        let (Some(v), Some(a)) = (tl.tracks.iter().position(|t| t.kind == Kind::Video), tl.tracks.iter().position(|t| t.kind == Kind::Audio)) else { return tl };
        for i in 0..tl.tracks[v].clips.len() {
            let c = tl.tracks[v].clips[i];
            if c.group != 0 {
                continue;
            }
            let Some(j) = tl.tracks[a].clips.iter().position(|o| o.group == 0 && o.at == c.at && o.len() == c.len()) else { continue };
            let g = tl.next_group();
            tl.tracks[v].clips[i].group = g;
            tl.tracks[a].clips[j].group = g;
        }
        tl.link_av_near(v, a) // STRIALINKAUTOFUZZ (SR928): then the pairs one tick apart by rounding, mutually unique only (file tail)
    }
}

// ---- STRIALINKAUTOFUZZ (LEDGER SR928): link_av's near pass, the V1 / A1 pairs one tick apart by rounding (named tail block) ----
// `lay_edit_list` rounds each stream's edits in its own media timescale, so a picture and its sound can land one tick
// apart in `at` and / or `len()`. After `link_av`'s exact pass, a still-unlinked V clip and A clip of the same source
// within one tick in both are NEAR; a near pair links (one fresh group) only when each is the other's ONLY near
// candidate. Two audio clips near one video clip, or two video clips near one audio clip, are all left unlinked.
impl Timeline {
    /// STRIALINKAUTOFUZZ: the near pass of [`Timeline::link_av`] on video track `v` and audio track `a`.
    fn link_av_near(mut self, v: usize, a: usize) -> Timeline {
        let near = |c: &Clip, o: &Clip| c.group == 0 && o.group == 0 && c.src == o.src && (o.at - c.at).abs() <= 1 && (o.len() - c.len()).abs() <= 1 && (o.src_in - c.src_in).abs() <= 1; // STRIALINKAUTOMORE (SR968): src_in within one tick too
        let mut pairs: Vec<(usize, usize)> = Vec::new();
        for (i, c) in self.tracks[v].clips.iter().enumerate() {
            let mut cand = self.tracks[a].clips.iter().enumerate().filter(|(_, o)| near(c, *o)).map(|(j, _)| j);
            let (Some(j), None) = (cand.next(), cand.next()) else { continue };
            let o = &self.tracks[a].clips[j];
            if self.tracks[v].clips.iter().filter(|w| near(*w, o)).count() == 1 {
                pairs.push((i, j));
            }
        }
        for (i, j) in pairs {
            let g = self.next_group();
            self.tracks[v].clips[i].group = g;
            self.tracks[a].clips[j].group = g;
        }
        self
    }
}

// ---- STRIAGAPSBLACK (LEDGER SR909): close-gaps removes the black fades of a closed gap (named tail block) ----
// A gap between two clips that `close_gaps` closes leaves no black to fade to or from: a to-black at the gap's start (the
// outgoing's end) would gain an incoming clip, a from-black at its end (the incoming's start) an outgoing one. Both are
// REMOVED in the same value, before the shift, so the cut becomes a plain cut (their audio fades are derived from them by
// tick and go with them). The leading gap is not touched here: a from-black there stays valid at tick 0.
impl Timeline {
    /// STRIAGAPSBLACK: drop `track`'s to-black at `start` and from-black at `end` (the gap `start..end` is closing).
    fn drop_gap_blacks(&mut self, track: usize, start: i64, end: i64) {
        if let Some(t) = self.tracks.get_mut(track) {
            t.transitions.retain(|x| !gap_black(x, start, end)); // STRIAGAPSTHREE (SR9028): the one black predicate (file tail)
        }
    }

    /// STRIAGAPSBLACK: how many gaps [`Timeline::close_gaps`] would close on `track`: the clips starting after the previous
    /// clip's end (0 for the first, so the leading gap counts). A bad track: 0.
    pub fn gap_count(&self, track: usize) -> usize {
        let Some(t) = self.tracks.get(track) else { return 0 };
        let mut end = 0;
        let mut n = 0;
        for c in &t.clips {
            if c.at > end {
                n += 1;
            }
            end = end.max(c.end());
        }
        n
    }
}

// ---- STRIALINKTRIM (LEDGER SR910): a trim, ripple trim or slip carried to every link mate (named tail block) ----
// The editor's trim doors (`EditorState::trim`, `ripple_trim`, `slip_selected`) run their op on the primary clip, then
// `linked` runs the same op, by the same source delta, on every other member of its group (`Timeline::mates`), chained
// on the edited timeline: one refusal refuses the whole edit, success is ONE commit. Group 0 is the primary alone, so
// `linked` returns the timeline unchanged. None of these ops reorders a track or touches a group, so the picks hold.
impl Timeline {
    /// STRIALINKTRIM: `self` (the timeline after the primary clip `idx` of `track` was edited) with `op(tl, track, idx,
    /// clip)` applied to each of that clip's link mates in turn; the first refusal is the result.
    pub fn linked(self, track: usize, idx: usize, op: impl Fn(&Timeline, usize, usize, Clip) -> Result<Timeline, EditError>) -> Result<Timeline, EditError> {
        let mut tl = self;
        for (k, i) in tl.mates(track, idx) {
            if (k, i) == (track, idx) {
                continue;
            }
            let m = *tl.tracks.get(k).and_then(|t| t.clips.get(i)).ok_or(EditError::NoClip)?;
            tl = op(&tl, k, i, m)?;
        }
        Ok(tl)
    }
}

// ---- STRIATRACKORDERKEYS (LEDGER SR922): the track's name and the implied-fade guard of a reorder (named tail block) ----
// `crossfade_of` and `black_pair` take the FIRST video track (lowest index) carrying a matching transition, so swapping two
// video tracks that both carry one at a cut, with different values, would change the audio fade that cut implies (and the
// schedule both faces call would re-time the sound). `swap_track` refuses that by name (`EditError::Crossfade`): a track
// ORDER edit never changes what is heard. An audio swap cannot trip it (the clips and their explicit xfade travel with the
// track). `track_name` is the message row's `V2` / `A1` (kind letter + `track_number`).
impl Timeline {
    /// STRIATRACKORDERKEYS: a track's name, its kind letter and its number within the kind (`V2`, `A1`).
    pub fn track_name(&self, track: usize) -> Option<String> {
        let k = if self.tracks.get(track)?.kind == Kind::Video { "V" } else { "A" };
        Some(alloc::format!("{k}{}", self.track_number(track)?))
    }
    /// STRIATRACKORDERKEYS: every audio clip's implied fades (`audio_crossfade`, `crossfade_at`, `fade_to_silence`) in
    /// `self` equal those of the same clip in `after`, where tracks `a` and `b` swapped (`EditError::Crossfade`: one differs).
    fn same_implied_fades(&self, after: &Timeline, a: usize, b: usize) -> Result<(), EditError> {
        for (t, tr) in self.tracks.iter().enumerate() {
            if tr.kind != Kind::Audio {
                continue;
            }
            let u = track_swapped(t, a, b);
            for ci in 0..tr.clips.len() {
                let before = (self.audio_crossfade(t, ci), self.crossfade_at(t, ci), self.fade_to_silence(t, ci));
                let now = (after.audio_crossfade(u, ci), after.crossfade_at(u, ci), after.fade_to_silence(u, ci));
                if before != now {
                    return Err(EditError::Crossfade);
                }
            }
        }
        Ok(())
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

// ---- STRIAGAPSALLTRACKS (LEDGER SR929): close-gaps on every track, all-or-nothing (named tail block) ----
// `close_gaps_all` chains SR857's ONE `close_gaps` (with SR909's black rule) over every track in a value built aside: the
// first track that refuses refuses the whole word, named by its index. A derived audio crossfade pairs by tick and source
// with a video transition on ANOTHER track, so tracks closed independently can lose a pairing that `validate` does not
// see (a wrong sound): an audio track ending with fewer derived crossfades than it had is refused by name (`Transition`).
// Each step validates its half-closed value (a coincidental mid-chain pairing can refuse: owed NLE-GAPS-ALL-ORDER).
impl Timeline {
    /// STRIAGAPSALLTRACKS: close every gap on every track (each the leading one included). Refused: `(track, why)` for the
    /// first track `close_gaps` refuses, or an audio track that would lose a crossfade paired with a video transition.
    pub fn close_gaps_all(&self) -> Result<Timeline, (usize, EditError)> {
        let mut tl = self.close_gaps_all_once().unwrap_or_else(|| self.clone()); // STRIAGAPSTHREE (SR9028): the finished value validated once; refused, the chain below names the track (file tail)
        for t in 0..tl.tracks.len() {
            tl = tl.close_gaps(t).map_err(|e| (t, e))?;
        }
        for t in 0..tl.tracks.len() {
            if tl.paired_xfades(t) < self.paired_xfades(t) {
                return Err((t, EditError::Transition));
            }
        }
        Ok(tl)
    }

    /// STRIAGAPSALLTRACKS: how many gaps [`Timeline::close_gaps_all`] would close (every track's [`Timeline::gap_count`]).
    pub fn gap_count_all(&self) -> usize {
        (0..self.tracks.len()).map(|t| self.gap_count(t)).sum()
    }

    /// The derived (video-paired) audio crossfades on `track`'s cuts ([`Timeline::audio_crossfade`]); 0 on a video track.
    fn paired_xfades(&self, track: usize) -> usize {
        let n = self.tracks.get(track).map_or(0, |t| t.clips.len());
        (0..n).filter(|&ci| self.audio_crossfade(track, ci).is_some()).count()
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

// ---- STRIAXFORMPRESETS (LEDGER SR938): the placement's corner presets and its centre-kept scale step (named tail block) ----
// `pip tl|tr|bl|br` put the quarter-size picture ([`Xform::PIP`]'s scale, 1/40 of the frame in from the edges) in that
// corner (`br` is [`Xform::PIP`] itself); `scale-` / `scale+` step the scale by [`Xform::SCALE_STEP`] about the rect's
// centre. Pure geometry: the words are editor.rs's (one `commit` through `EditorState::set_xform`).
impl Xform {
    /// The `scale-` / `scale+` step, 1/1000 of the frame (even, so the centre moves by none).
    pub const SCALE_STEP: u16 = 50;
    /// The quarter-size picture in corner `c` (`tl`, `tr`, `bl`, `br`); `None` for any other word.
    pub fn corner(c: &str) -> Option<Xform> {
        let (x, y) = match c {
            "tl" => (25, 25),
            "tr" => (725, 25),
            "bl" => (25, 725),
            "br" => (725, 725),
            _ => return None,
        };
        Some(Xform { x, y, s: Xform::PIP.s })
    }
    /// The same rect with its scale moved by `d` (1/1000) about its centre (`x`, `y` move by `-d / 2`); `None` when the
    /// rect would leave the frame (or vanish).
    pub fn stepped(self, d: i32) -> Option<Xform> {
        let (s, x, y) = (self.s as i32 + d, self.x as i32 - d / 2, self.y as i32 - d / 2);
        if s < 1 || x < 0 || y < 0 || s > 1000 || x > 1000 || y > 1000 {
            return None;
        }
        let xf = Xform { x: x as u16, y: y as u16, s: s as u16 };
        xf.ok().then_some(xf)
    }
}

// ---- STRIAMULTISELECTMORE (LEDGER SR950): a set moved to another track (named tail block) ----
// `move_many`'s all-leave / all-land with every clip landing `dtrack` tracks away: `move_clip`'s rules per clip (a track
// that does not exist is `NoTrack`, one of the other kind is `Kind`), then `move_clip`'s road (`retarget_transitions`,
// `validate`): a transitioned cut whose clips ALL move is carried to their new track (STRIAMULTISELECTNEXT `carry_cuts`), one with a side left behind is refused `Transition` by name.

impl Timeline {
    /// Move every clip in `picks` by `delta` ticks AND by `dtrack` tracks as ONE transaction (refused whole). `dtrack == 0`
    /// is [`Timeline::move_many`].
    pub fn move_many_track(&self, picks: &[(usize, usize)], dtrack: isize, delta: i64) -> Result<Timeline, EditError> {
        if dtrack == 0 {
            return self.move_many(picks, delta);
        }
        let order = picks_desc(self, picks)?;
        let mut tl = self.clone();
        let mut moved: Vec<(usize, Clip)> = Vec::new();
        for (t, i) in order {
            let to = t.checked_add_signed(dtrack).filter(|&u| u < tl.tracks.len()).ok_or(EditError::NoTrack)?;
            if tl.tracks[to].kind != tl.tracks[t].kind {
                return Err(EditError::Kind);
            }
            let mut c = tl.tracks[t].clips.remove(i);
            c.at = c.at.saturating_add(delta);
            moved.push((to, c));
        }
        for (u, c) in moved {
            tl.tracks[u].clips.push(c);
        }
        for t in &mut tl.tracks {
            sort(t);
        }
        tl.retarget_transitions(self)?;
        tl.validate()?;
        Ok(tl)
    }
}

// ---- STRIACLIPSPEEDMORE (LEDGER SR951): a rated clip edits like a 1x clip — slide follows through src_at, the rippling speed change (named tail block) ----
// `slide` makes its butted neighbours follow through ONE `follow_src`: at 1x the old `saturating_add` (so every 1x slide is
// unchanged), at a rate the source point THE `Clip::src_at` of the PRE-move clip gives at the moved edge; the parent's
// `slide_follows_rated` now refuses (`Rate`) only a rated neighbour that cannot land exactly there (a 0.5x hold's half
// frame). `ripple_set_rate` is the existing ripple body (`ripple_trim`) on the re-rated clip: later clips, transitions and
// explicit audio crossfades of its track at or past its old end shift by the length change (NLE-SPEED-RIPPLE), where
// `set_rate` refuses `Overlap`. The drag's timeline delta is converted in `EditorState::trim_by_ticks` (editor.rs tail).

/// STRIACLIPSPEEDMORE: the source edge `old` of `c` (the PRE-move clip) when its timeline edge at `edge` moves by `delta`:
/// 1x `old + delta` (saturating, the old slide), any other rate `c.src_at(edge + delta)`.
fn follow_src(c: Clip, edge: i64, old: i64, delta: i64) -> i64 {
    if c.rate_q16 == 0x1_0000 { old.saturating_add(delta) } else { c.src_at(edge.saturating_add(delta)) }
}

impl Timeline {
    /// STRIACLIPSPEEDMORE: set clip `idx`'s speed to `rate_q16` (Q16.16) keeping its `at`; every other clip, transition and
    /// explicit audio crossfade of `track` at or past its old end shifts by the length change (`ripple_trim`, the one ripple
    /// body). Refused by name: `NoTrack` / `NoClip`; rate 0 `Rate` (`validate`).
    pub fn ripple_set_rate(&self, track: usize, idx: usize, rate_q16: u32) -> Result<Timeline, EditError> {
        let c = *self.tracks.get(track).ok_or(EditError::NoTrack)?.clips.get(idx).ok_or(EditError::NoClip)?;
        self.ripple_trim(track, idx, c.at_rate(rate_q16))
    }
}

#[cfg(test)]
mod striaclipspeedmore_tests {
    extern crate std;
    use super::*;
    use crate::editor::{Edge, EditorState};
    use crate::view::{Drag, DragKind, Layout};

    /// V1 on source A (50): `clips` laid as given.
    fn tl(clips: Vec<Clip>) -> Timeline {
        let mut tl = Timeline::new(25);
        tl.add_source("A", 50);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = clips;
        tl
    }

    fn a(src_in: i64, src_out: i64, at: i64) -> Clip {
        Clip::new(SourceId(0), src_in, src_out, at)
    }

    #[test]
    fn striaclipspeedmore_slide_rated_neighbours_follow_through_src_at() {
        // P 2x 0..10 @0 (end 5) | M 1x 20..30 @5 (end 15) | N 2x 30..40 @15 (end 20).
        let base = tl(alloc::vec![a(0, 10, 0).at_rate(0x2_0000), a(20, 30, 5), a(30, 40, 15).at_rate(0x2_0000)]);
        let r = base.slide(0, 1, 1).unwrap();
        assert_eq!(r.tracks[0].clips, alloc::vec![a(0, 12, 0).at_rate(0x2_0000), a(20, 30, 6), a(32, 40, 16).at_rate(0x2_0000)]);
        let l = base.slide(0, 1, -1).unwrap();
        assert_eq!(l.tracks[0].clips, alloc::vec![a(0, 8, 0).at_rate(0x2_0000), a(20, 30, 4), a(28, 40, 14).at_rate(0x2_0000)]);
        let (p, n) = (base.tracks[0].clips[0], base.tracks[0].clips[2]);
        for t in 0..4 {
            assert_eq!(r.tracks[0].clips[0].src_at(t), p.src_at(t), "P tick {t}");
        }
        for t in 16..20 {
            assert_eq!(r.tracks[0].clips[2].src_at(t), n.src_at(t), "N tick {t}");
        }
        let slow = tl(alloc::vec![a(0, 10, 0).at_rate(0x8000), a(20, 30, 20)]);
        assert_eq!(slow.slide(0, 1, 1), Err(EditError::Rate), "a 0.5x neighbour cannot end on a held half frame");
        assert_eq!(slow.slide(0, 1, 2).map(|s| s.tracks[0].clips.clone()), Ok(alloc::vec![a(0, 11, 0).at_rate(0x8000), a(20, 30, 22)]));
        let one = tl(alloc::vec![a(0, 10, 0), a(20, 30, 10)]);
        assert_eq!(one.slide(0, 1, -1).map(|s| s.tracks[0].clips.clone()), Ok(alloc::vec![a(0, 9, 0), a(20, 30, 9)]), "1x unchanged");
        std::println!("striaclipspeedmore: slide +1 over 2x neighbours lands at 6 / 16; a 0.5x neighbour +1 -> {}", EditError::Rate);
    }

    #[test]
    fn striaclipspeedmore_ripple_set_rate_shifts_the_rest_of_the_track() {
        let base = tl(alloc::vec![a(0, 10, 0), a(20, 30, 10)]).add_transition(0, 10, TransitionKind::Dissolve, 2).unwrap();
        assert!(base.set_rate(0, 0, 0x8000).is_err(), "set_rate does not ripple: refused");
        let r = base.ripple_set_rate(0, 0, 0x8000).unwrap();
        assert_eq!(r.tracks[0].clips, alloc::vec![a(0, 10, 0).at_rate(0x8000), a(20, 30, 20)]);
        assert_eq!(r.tracks[0].transitions[0].at, 20, "the cut's transition moves with it");
        let f = base.ripple_set_rate(0, 0, 0x2_0000).unwrap();
        assert_eq!((f.tracks[0].clips[1].at, f.tracks[0].transitions[0].at), (5, 5));
        assert_eq!(base.ripple_set_rate(0, 0, 0), Err(EditError::Rate));
        assert_eq!(base.ripple_set_rate(5, 0, 0x2_0000), Err(EditError::NoTrack));
        assert_eq!(base.ripple_set_rate(0, 9, 0x2_0000), Err(EditError::NoClip));
        std::println!("striaclipspeedmore: ripple 0.5x moves the next clip and its dissolve 10 -> 20; plain set_rate refuses");
    }

    #[test]
    fn striaclipspeedmore_drag_trim_lands_under_the_pointer_and_rspeed_is_one_step() {
        let two = tl(alloc::vec![a(0, 10, 0).at_rate(0x2_0000), a(20, 30, 10)]);
        let mut st = EditorState::new(two.clone());
        st.select(0, 0).unwrap();
        assert_eq!(st.trim_by_ticks(Edge::Out, -2), Ok(()));
        let c = st.selected().unwrap();
        assert_eq!((c.src_out, c.end()), (6, 3), "the out edge lands 2 ticks earlier at 2x");
        let mut s1 = EditorState::new(tl(alloc::vec![a(0, 10, 0), a(20, 30, 10)]));
        let mut s2 = s1.clone();
        s1.select(0, 0).unwrap();
        s2.select(0, 0).unwrap();
        assert_eq!((s1.trim_by_ticks(Edge::In, 3), s2.trim(Edge::In, 3)), (Ok(()), Ok(())));
        assert_eq!(s1.timeline, s2.timeline, "1x: ticks are source ticks");
        let mut st = EditorState::new(two.clone());
        st.select(0, 0).unwrap();
        let d = Drag {
            kind: DragKind::TrimOut,
            track: 0,
            idx: 0,
            origin_tick: 5,
            layout: Layout::of(320, 56, &st),
            delta: -2,
            to_track: 0,
            origin: (0.0, 0.0),
            at: (0.0, 0.0),
            snap: false,
            additive: false,
            guide: None,
        };
        assert_eq!(d.finish(&mut st), Ok(()));
        assert_eq!(st.selected().unwrap().end(), 3, "the drag's edge is where the pointer left it");
        let mut st = EditorState::new(tl(alloc::vec![a(0, 10, 0), a(20, 30, 10)]));
        st.select(0, 0).unwrap();
        let want = st.timeline.ripple_set_rate(0, 0, 0x8000).unwrap();
        assert_eq!(st.verb("rspeed 0.5"), Some(Ok(())));
        assert_eq!((&st.timeline, st.undo_depth()), (&want, 1));
        assert_eq!(st.verb("rspeed 0"), Some(Err(EditError::Rate)));
        assert_eq!(st.undo_depth(), 1, "a refusal pushes nothing");
        assert_eq!(st.verb("rspeed x"), None);
        std::println!("striaclipspeedmore: 2x drag -2 ticks ends at 3; rspeed 0.5 one undo step; rspeed 0 -> {}", EditError::Rate);
    }
}

// ---- STRIARIPPLEMORE (LEDGER SR953): the ripple across every track (named tail block) ----
// SR692's `ripple_trim` ripples ONE track, so a derived audio crossfade (the audio cut and the video transition at one
// tick, `crossfade_of`) un-pairs. The all-tracks ripple runs that one `ripple_trim` on the primary's track and on every
// other track's TWIN (the clip with the primary's exact span `at..end`: the linked A/V case; no source match, a linked
// audio clip may carry its own SourceId), which takes the same edge change, and splices each result's track and its
// explicit audio crossfades into one timeline. A track with no twin shifts every clip, transition and explicit audio
// crossfade at or past the old end by the length change (`ripple_shift_from`); a clip straddling the old end cannot stay
// in sync and is refused `Overlap`. ONE final `validate` judges the whole (a non-twin cut at the old end shortened is
// `Overlap`). `ripple_shift_from` and SR857's `shift_from` become one at the fold.
impl Timeline {
    /// STRIARIPPLEMORE: [`Timeline::ripple_trim_in`] across every track: clip `idx`'s in point to source tick `src_in`
    /// keeping its start; every other track's twin trims the same, every other track's later material follows.
    pub fn ripple_all_in(&self, track: usize, idx: usize, src_in: i64) -> Result<Timeline, EditError> {
        let c = *self.tracks.get(track).ok_or(EditError::NoTrack)?.clips.get(idx).ok_or(EditError::NoClip)?;
        self.ripple_all(track, idx, Clip { src_in, ..c })
    }

    /// STRIARIPPLEMORE: [`Timeline::ripple_trim_out`] across every track: clip `idx`'s out point to source tick
    /// `src_out`; every other track's twin trims the same, every other track's later material follows.
    pub fn ripple_all_out(&self, track: usize, idx: usize, src_out: i64) -> Result<Timeline, EditError> {
        let c = *self.tracks.get(track).ok_or(EditError::NoTrack)?.clips.get(idx).ok_or(EditError::NoClip)?;
        self.ripple_all(track, idx, Clip { src_out, ..c })
    }

    /// The one body of the two all-tracks ripples: clip `idx` of `track` becomes `now` (same `at`).
    fn ripple_all(&self, track: usize, idx: usize, now: Clip) -> Result<Timeline, EditError> {
        let was = *self.tracks.get(track).ok_or(EditError::NoTrack)?.clips.get(idx).ok_or(EditError::NoClip)?;
        let (edge, d) = (was.end(), now.len() - was.len());
        let (din, dout) = (now.src_in - was.src_in, now.src_out - was.src_out);
        let mut tl = self.clone();
        for k in 0..self.tracks.len() {
            let twin = if k == track {
                Some((idx, now))
            } else {
                let clips = &self.tracks[k].clips;
                clips.iter().position(|c| c.at == was.at && c.end() == edge).map(|i| {
                    let c = clips[i];
                    (i, Clip { src_in: c.src_in + din, src_out: c.src_out + dout, ..c })
                })
            };
            match twin {
                Some((i, c)) => {
                    let r = self.ripple_trim(k, i, c)?;
                    tl.tracks[k] = r.tracks[k].clone();
                    tl.audio_xfades.retain(|x| x.track != k);
                    tl.audio_xfades.extend(r.audio_xfades.iter().filter(|x| x.track == k).copied());
                }
                None => tl.ripple_shift_from(k, edge, d)?,
            }
        }
        tl.audio_xfades.sort_by_key(|x| (x.track, x.at));
        tl.validate()?;
        Ok(tl)
    }

    /// STRIARIPPLEMORE: every clip, transition and explicit audio crossfade of track `k` at or past tick `edge` moves by
    /// `d` ticks; a clip straddling `edge` (`at < edge < end`) is refused `Overlap`. No validate (the caller's one gate).
    fn ripple_shift_from(&mut self, k: usize, edge: i64, d: i64) -> Result<(), EditError> {
        let tr = self.tracks.get_mut(k).ok_or(EditError::NoTrack)?;
        if tr.clips.iter().any(|c| c.at < edge && edge < c.end()) {
            return Err(EditError::Overlap);
        }
        for c in tr.clips.iter_mut().filter(|c| c.at >= edge) {
            c.at += d;
        }
        for x in tr.transitions.iter_mut().filter(|x| x.at >= edge) {
            x.at += d;
        }
        for x in self.audio_xfades.iter_mut().filter(|x| x.track == k && x.at >= edge) {
            x.at += d;
        }
        Ok(())
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

// ---- STRIAGAPSMORE (LEDGER SR969): close-gaps sync, the spans empty on every track (named tail block) ----
// A sync gap is a span no clip of ANY track covers (the leading one, tick 0 to the first clip anywhere, included).
// `close_gaps_sync` closes only those, shifting EVERY track by the same delta at the same edge through SR857's one
// `shift_from`, so a derived audio crossfade (paired by tick with a video transition on another track) keeps its pairing
// and A / V stay locked. A track whose own gap closes fully there (a clip ends at the span's start and one starts at its
// end) loses that gap's black fades through SR909's one `drop_gap_blacks`; a track whose own gap stays open keeps them.
// One `validate` at the end (no half-closed value is checked).
impl Timeline {
    /// STRIAGAPSMORE: the spans `(start, end)` no clip of any track covers, in order, the leading one included.
    pub fn sync_gaps(&self) -> Vec<(i64, i64)> {
        let mut spans: Vec<(i64, i64)> = self.tracks.iter().flat_map(|t| t.clips.iter().map(|c| (c.at, c.end()))).collect();
        spans.sort_unstable();
        let mut end = 0;
        let mut gaps = Vec::new();
        for (at, e) in spans {
            if at > end {
                gaps.push((end, at));
            }
            end = end.max(e);
        }
        gaps
    }

    /// STRIAGAPSMORE: close every span empty on every track ([`Timeline::sync_gaps`]): every track moves alike. No such
    /// span: an equal value.
    pub fn close_gaps_sync(&self) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        for _ in 0..self.sync_gaps().len() {
            let Some(&(s, e)) = tl.sync_gaps().first() else { break };
            for t in 0..tl.tracks.len() {
                let k = &tl.tracks[t];
                if s > 0 && k.clips.iter().any(|c| c.end() == s) && k.clips.iter().any(|c| c.at == e) {
                    tl.drop_gap_blacks(t, s, e);
                }
                tl.shift_from(t, e, s - e, None);
            }
        }
        tl.validate()?;
        Ok(tl)
    }
}

// ---- STRIAXFORMMORE (LEDGER SR975): the placement's clamp, clamped step and nudge (named tail block) ----
// ONE clamp pulls a rect into the frame ([`Xform::clamped`]); a `scale-` / `scale+` step that SR938's [`Xform::stepped`]
// refuses at an edge is clamped instead ([`Xform::fit_step`], the centre kept as far as the frame lets), and `pip left` /
// `right` / `up` / `down` move the rect by [`Xform::NUDGE_STEP`] ([`Xform::nudged`]). Both return `None` only when nothing
// moves (the rect already at that limit), which editor.rs refuses as `EditError::Xform`. Pure geometry: the words are
// editor.rs's (one `commit` through `EditorState::set_xform`).
impl Xform {
    /// The `pip left` / `right` / `up` / `down` step, 1/1000 of the frame (the corner presets' inset, 1/40).
    pub const NUDGE_STEP: u16 = 25;
    /// The rect of scale `s` at top-left (`x`, `y`) pulled inside the frame: `s` into 1..=1000, then `x`, `y` into
    /// 0..=1000 - `s`.
    pub fn clamped(s: i32, x: i32, y: i32) -> Xform {
        let s = s.clamp(1, 1000);
        let m = 1000 - s;
        Xform { x: x.clamp(0, m) as u16, y: y.clamp(0, m) as u16, s: s as u16 }
    }
    /// The scale moved by `d` about the centre: [`Xform::stepped`] when it fits, else the clamped rect (the centre kept
    /// as far as the frame lets); `None` when the result is this rect (nothing moves).
    pub fn fit_step(self, d: i32) -> Option<Xform> {
        if let Some(xf) = self.stepped(d) {
            return Some(xf);
        }
        let (s, x, y) = (self.s as i32, self.x as i32, self.y as i32);
        let t = (s + d).clamp(1, 1000);
        let xf = Xform::clamped(t, (2 * x + s - t) / 2, (2 * y + s - t) / 2);
        (xf != self).then_some(xf)
    }
    /// The rect moved by (`dx`, `dy`) (1/1000), stopped at the frame's edges; `None` when it does not move.
    pub fn nudged(self, dx: i32, dy: i32) -> Option<Xform> {
        let xf = Xform::clamped(self.s as i32, self.x as i32 + dx, self.y as i32 + dy);
        (xf != self).then_some(xf)
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

// ---- STRIAINSERTMARKSKEYSNEXT (LEDGER SR985): the all-tracks insert's room on the other tracks (named tail block) ----
// `Mode::InsertAll` rides THE one `Timeline::place`: its Insert arm makes room on the target track, then `ripple_others`
// makes the same room on every other track (the clip spanning `at` split there, every clip from `at` on later by `len`),
// before place's ONE `retarget_transitions` against the original and ONE `validate`. A transition or audio crossfade whose
// cut the ripple opens is refused there by name (`EditError::Transition`), never dropped. The export marks are not moved.
// Owed: NLE-INSERT-ROOM-ONE (the Insert arm's body onto this per-track room), NLE-INSERT-SYNC-MARKS.

impl Timeline {
    /// Split at `at` and move every clip from `at` on later by `len`, on every track but `track`.
    fn ripple_others(&mut self, track: usize, at: i64, len: i64) {
        for (k, t) in self.tracks.iter_mut().enumerate() {
            if k == track {
                continue;
            }
            split_at(t, at);
            for c in t.clips.iter_mut().filter(|c| c.at >= at) {
                c.at += len;
            }
        }
    }
}

// ---- STRIAMULTISELECTNEXT (LEDGER SR998): a set's transitioned cuts carried across tracks (named tail block) ----
// `move_many_track` hands the ONE matcher (`retarget_transitions`) this `before` instead of `self`: the picked clips laid on
// the track `dtrack` away at their OWN ticks, and every transition / explicit audio crossfade whose cut's clips (those
// starting or ending at it) are ALL picked moved to that track with them. The matcher then finds the cut where the clips
// went; a cut with an unpicked side stays behind and is refused `Transition`, as before. `validate` stays the one gate.
impl Timeline {
    /// The reference state a cross-track set move retargets against (see the block comment). Picks are already checked by
    /// the caller; a pick whose track has no destination is left where it is.
    fn carry_cuts(&self, picks: &[(usize, usize)], dtrack: isize) -> Timeline {
        let to = |t: usize| t.checked_add_signed(dtrack).filter(|&u| u < self.tracks.len());
        let whole = |t: usize, at: i64| {
            let mut touch = self.tracks[t].clips.iter().enumerate().filter(|(_, c)| c.at == at || c.end() == at).map(|(i, _)| i).peekable();
            touch.peek().is_some() && touch.all(|i| picks.contains(&(t, i)))
        };
        let mut pre = self.clone();
        let mut order: Vec<(usize, usize)> = picks.to_vec();
        order.sort_unstable_by(|a, b| b.cmp(a));
        order.dedup();
        for (t, i) in order {
            let Some(u) = to(t) else { continue };
            if i < pre.tracks[t].clips.len() {
                let c = pre.tracks[t].clips.remove(i);
                pre.tracks[u].clips.push(c);
            }
        }
        let mut carried: Vec<(usize, Transition)> = Vec::new();
        for (t, tr) in self.tracks.iter().enumerate() {
            let Some(u) = to(t) else { continue };
            pre.tracks[t].transitions = tr.transitions.iter().filter(|x| !whole(t, x.at)).copied().collect();
            carried.extend(tr.transitions.iter().filter(|x| whole(t, x.at)).map(|x| (u, *x)));
        }
        for (u, x) in carried {
            pre.tracks[u].transitions.push(x);
        }
        for tr in &mut pre.tracks {
            sort(tr);
            tr.transitions.sort_by_key(|x| x.at);
        }
        for x in &mut pre.audio_xfades {
            let (t, at) = (x.track, x.at);
            if let Some(u) = to(t).filter(|_| t < self.tracks.len() && whole(t, at)) {
                x.track = u;
            }
        }
        pre.audio_xfades.sort_by_key(|x| (x.track, x.at));
        pre
    }
}

// ---- STRIAMARKERJUMPKEYSNEXT (LEDGER SR984): a marker's colour and range length (named tail block) ----
// SR835's `Marker` carries a colour (`colour`, an index into [`MARKER_COLOURS`], 0 = the orange every marker had) and a
// range length (`len` ticks from `at`, 0 = a point marker). Both are set only through the pure ops below and the editor's
// ONE commit; the defaults paint (view.rs) and write (edl.rs) byte-identical to a marker without them. `validate` refuses a
// negative length (`Negative`) and a colour outside the table (`Marker`).

/// STRIAMARKERJUMPKEYSNEXT (SR984): the marker colours by name; a marker's `colour` indexes this table (view.rs
/// `MARKER_RGBA` holds the pixels, same order).
pub const MARKER_COLOURS: [&str; 8] = ["orange", "red", "yellow", "green", "cyan", "blue", "purple", "pink"];

/// STRIAMARKERJUMPKEYSNEXT (SR984): the index of the colour named `name` (trimmed, ASCII case ignored).
pub fn marker_colour(name: &str) -> Option<u8> {
    let name = name.trim();
    MARKER_COLOURS.iter().position(|c| c.eq_ignore_ascii_case(name)).map(|i| i as u8)
}

impl Timeline {
    /// The last marker at or before tick `t`.
    pub fn marker_before(&self, t: i64) -> Option<&Marker> {
        self.markers.iter().rev().find(|m| m.at <= t)
    }
    /// The timeline with the marker at tick `at` coloured `colour`; `None` when no marker stands there, the colour is
    /// outside [`MARKER_COLOURS`], or the marker already has it.
    pub fn colour_marker(&self, at: i64, colour: u8) -> Option<Timeline> {
        let i = self.markers.binary_search_by_key(&at, |m| m.at).ok()?;
        if usize::from(colour) >= MARKER_COLOURS.len() || self.markers[i].colour == colour {
            return None;
        }
        let mut tl = self.clone();
        tl.markers[i].colour = colour;
        Some(tl)
    }
    /// The timeline with the marker at tick `at` spanning `len` ticks (0: a point marker again); `None` when no marker
    /// stands there, `len < 0`, or the marker already spans `len`.
    pub fn range_marker(&self, at: i64, len: i64) -> Option<Timeline> {
        let i = self.markers.binary_search_by_key(&at, |m| m.at).ok()?;
        if len < 0 || self.markers[i].len == len {
            return None;
        }
        let mut tl = self.clone();
        tl.markers[i].len = len;
        Some(tl)
    }
}

#[cfg(test)]
mod striamarkerjumpkeysnext_tests {
    use super::*;

    fn tl() -> Timeline {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 20, 0)];
        tl.mark(5, "A").unwrap().mark(12, "B").unwrap()
    }

    /// STRIAMARKERJUMPKEYSNEXT M3: colour names resolve in table order; `colour_marker` sets only the marker at the tick
    /// and refuses no marker, a colour off the table and the same colour.
    #[test]
    fn striamarkerjumpkeysnext_colour_marker() {
        assert_eq!((marker_colour("orange"), marker_colour(" Red "), marker_colour("pink"), marker_colour("mauve")), (Some(0), Some(1), Some(7), None));
        let t = tl().colour_marker(12, 1).expect("B turns red");
        assert_eq!((t.markers[0].colour, t.markers[1].colour), (0, 1));
        assert_eq!(t.validate(), Ok(()));
        assert!(t.colour_marker(12, 1).is_none(), "already red");
        assert!(tl().colour_marker(6, 1).is_none(), "no marker at 6");
        assert!(tl().colour_marker(5, 8).is_none(), "off the table");
        let mut bad = tl();
        bad.markers[0].colour = 8;
        assert_eq!(bad.validate(), Err(EditError::Marker));
    }

    /// STRIAMARKERJUMPKEYSNEXT M3: `range_marker` sets the length, 0 clears it; no marker, a negative length and the same
    /// length are refused; `marker_before` is the last marker at or before a tick.
    #[test]
    fn striamarkerjumpkeysnext_range_marker() {
        let t = tl().range_marker(5, 4).expect("A spans 4");
        assert_eq!((t.markers[0].len, t.markers[1].len), (4, 0));
        assert_eq!(t.validate(), Ok(()));
        assert!(t.range_marker(5, 4).is_none(), "unchanged");
        assert_eq!(t.range_marker(5, 0).expect("cleared"), tl());
        assert!(tl().range_marker(6, 3).is_none(), "no marker at 6");
        assert!(tl().range_marker(5, -1).is_none(), "negative");
        assert_eq!(tl().marker_before(11).map(|m| m.at), Some(5));
        assert_eq!(tl().marker_before(12).map(|m| m.at), Some(12));
        assert!(tl().marker_before(4).is_none());
        let mut bad = tl();
        bad.markers[1].len = -1;
        assert_eq!(bad.validate(), Err(EditError::Negative));
    }
}

// ---- STRIAKXFORM (LEDGER SR9011): a schedule step's placement, the one rule both faces call (named tail block) ----
// A placed clip's step carries the track below as its outgoing `VideoMix` (STRIAOPACITY's two-layer schedule);
// [`Timeline::place_step`] is THE rule that turns that step's two frames into what it shows: the clip's placement, its
// crop and its source's size policy into [`xform_frame`]. Stria's render (nle.rs `xformed`) and the kernel Player's
// transition mix (vplay.rs `edl_mix`) both call it, so neither face holds a copy of the step -> clip lookup or the fit.
impl Timeline {
    /// The frame video step `s` shows when its clip is placed: the step's own frame `top` and the lower frame `under` (the
    /// step's outgoing: the track below, or black), both `w x h` RGBA, through [`xform_frame`] with the clip's
    /// [`Timeline::xform_at`], its [`Timeline::crop_at`] and its source's [`SizePolicy`] (Letterbox when the source is
    /// absent; Adopt places as Letterbox) at the mix weight `s.vmix.t`. `None` when the step has no track, no source, no
    /// mix, or its clip is not placed (the caller mixes as before).
    pub fn place_step(&self, s: &schedule::Step, under: &[[u8; 4]], top: &[[u8; 4]], w: usize, h: usize) -> Option<Vec<[u8; 4]>> {
        let (ti, src, m) = (s.track?, s.src?, s.vmix?);
        let xf = self.xform_at(ti, s.out_t);
        if xf == Xform::NONE {
            return None;
        }
        let fit = self.source(src).map_or(SizePolicy::Letterbox, |r| r.size_policy);
        Some(xform_frame(top, under, w, h, self.crop_at(ti, s.out_t, false), fit, xf, m.t))
    }
}

// ---- STRIAKCROP (LEDGER SR9010): a step's reel fit, lifted from stria nle.rs (named tail block) ----
// The size policy a video step places (and crops) its frame by: both faces call it (stria nle.rs `reel_fit`, the kernel
// Player's crop in vplay.rs), so the Adopt-in-a-mix rule lives once.

impl Timeline {
    /// Source `src`'s size policy as a step places its frame (absent: Letterbox). `alone` is false inside a transition:
    /// the mix is one size, so Adopt mixes as Letterbox there.
    pub fn reel_fit(&self, src: SourceId, alone: bool) -> SizePolicy {
        match self.source(src).map_or(SizePolicy::Letterbox, |s| s.size_policy) {
            SizePolicy::Adopt if !alone => SizePolicy::Letterbox,
            p => p,
        }
    }
}

// ---- STRIAKTRACKADD (LEDGER SR9007): the note a `track-` removal leaves, one text for both faces (named tail block) ----
// A removal takes the track's clips with it, so each face names what went: the host face's `note` (nle_face.rs
// `track_remove_verb`) and the kernel Player's `[nle] edit track-` line (vplay.rs `edit_track_note`) both print this text,
// read BEFORE the one commit (the track's channel word and clip count as they stood).
impl Timeline {
    /// `track-: removed <ch> (<n> clip|clips)` for track `track` (`None`: no such track).
    pub fn track_removed_note(&self, track: usize) -> Option<String> {
        let (word, n) = (self.track_word(track)?, self.tracks.get(track)?.clips.len());
        Some(alloc::format!("track-: removed {word} ({n} clip{})", if n == 1 { "" } else { "s" }))
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

// ---- STRIAGAPSTHREE (LEDGER SR9028): one close-gaps body, keep a faded gap open, the all-tracks close validated once (named tail block) ----
// SR857's per-track loop (with SR909's black rule folded on its `if at > end` line) is the ONE body `close_gaps_on`: the plain
// close, the keep close and the all-tracks fast path call it; it neither clones nor validates. `keep` leaves open an inner gap
// carrying a to-black at its start or a from-black at its end (`gap_black`, the one predicate `drop_gap_blacks` reads too), so
// the fade stays with its gap. `close_gaps_all_once` closes every track raw and validates the finished value ONCE (a
// coincidental half-closed pairing no longer refuses); `None` when that refuses, and SR929's chain then names the track.
impl Timeline {
    /// STRIAGAPSTHREE: close the gaps of `track` in place (the leading one included); with `keep`, an inner gap carrying a
    /// black fade stays open with it. No validate: the caller validates. Refused: a bad track (`NoTrack`).
    fn close_gaps_on(&mut self, track: usize, keep: bool) -> Result<(), EditError> {
        let n = self.tracks.get(track).ok_or(EditError::NoTrack)?.clips.len();
        let mut end = 0;
        for i in 0..n {
            let at = self.tracks[track].clips[i].at;
            if at > end && !(keep && i > 0 && self.tracks[track].transitions.iter().any(|x| gap_black(x, end, at))) {
                if i > 0 {
                    self.drop_gap_blacks(track, end, at);
                }
                self.shift_from(track, at, end - at, None);
            }
            end = self.tracks[track].clips[i].end();
        }
        Ok(())
    }

    /// STRIAGAPSTHREE: close every gap on `track` but an inner gap carrying a black fade (to-black at its start, from-black
    /// at its end), which stays open with its fade. Nothing to close: an equal value.
    pub fn close_gaps_keep(&self, track: usize) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        tl.close_gaps_on(track, true)?;
        tl.validate()?;
        Ok(tl)
    }

    /// STRIAGAPSTHREE: every track closed raw ([`Timeline::close_gaps_on`]), the finished value validated ONCE; `None`
    /// when that validate refuses.
    fn close_gaps_all_once(&self) -> Option<Timeline> {
        let mut tl = self.clone();
        for t in 0..tl.tracks.len() {
            tl.close_gaps_on(t, false).ok()?;
        }
        tl.validate().ok()?;
        Some(tl)
    }
}

/// STRIAGAPSTHREE: `x` is a black fade of the gap `start..end`: a to-black at its start or a from-black at its end.
fn gap_black(x: &Transition, start: i64, end: i64) -> bool {
    (x.to_black && x.at == start) || (x.from_black && x.at == end)
}

// ---- STRIAMARKERCHAPTERS (LEDGER SR9046): the markers as an ffmetadata chapters text (named tail block) ----
// An export's chapters sidecar: every marker with `span.start <= at < span.end`, in tick order, one `[CHAPTER]` in
// nanoseconds from the range's start through the one tick-to-ns rule (`Timebase::to_ns`). A ranged marker ends at
// `min(at + len, span.end)`; a point marker at the next in-range marker, else at `span.end`. The title escapes `=` `;`
// `#` `\` and newline with a backslash (ffmetadata's rule). No marker in the range: the empty string (no file).

impl Timeline {
    /// The markers inside `span` as an ffmetadata chapters text (`;FFMETADATA1` then one `[CHAPTER]` each); "" when none.
    pub fn chapters(&self, span: core::ops::Range<i64>) -> String {
        let inside: Vec<&Marker> = self.markers.iter().filter(|m| m.at >= span.start && m.at < span.end).collect();
        if inside.is_empty() {
            return String::new();
        }
        let mut out = String::from(";FFMETADATA1\n");
        for (i, m) in inside.iter().enumerate() {
            let end = if m.len > 0 { m.at.saturating_add(m.len).min(span.end) } else { inside.get(i + 1).map_or(span.end, |n| n.at) };
            out.push_str(&alloc::format!(
                "[CHAPTER]\nTIMEBASE=1/1000000000\nSTART={}\nEND={}\ntitle={}\n",
                self.tb.to_ns(m.at - span.start),
                self.tb.to_ns(end - span.start),
                ffmeta_escape(&m.name)
            ));
        }
        out
    }
}

/// STRIAMARKERCHAPTERS (SR9046): `s` with ffmetadata's special characters (`=` `;` `#` `\` newline) backslash-escaped.
fn ffmeta_escape(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if matches!(c, '=' | ';' | '#' | '\\' | '\n') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod striamarkerchapters_tests {
    use super::*;

    fn tl() -> Timeline {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 0, 40, 0)];
        tl.mark(5, "Intro").unwrap().mark(12, "Act=1; #2").unwrap().mark(20, "End").unwrap().range_marker(20, 30).unwrap()
    }

    /// STRIAMARKERCHAPTERS M3: only in-range markers; a point marker ends at the next one, a ranged one at its end
    /// clipped to the range; times relative to the range start at 25 fps (40 ms a tick); the title escaped; none: "".
    #[test]
    fn striamarkerchapters_ffmetadata() {
        let t = tl();
        assert_eq!(
            t.chapters(10..30),
            ";FFMETADATA1\n[CHAPTER]\nTIMEBASE=1/1000000000\nSTART=80000000\nEND=400000000\ntitle=Act\\=1\; \\#2\n[CHAPTER]\nTIMEBASE=1/1000000000\nSTART=400000000\nEND=800000000\ntitle=End\n"
        );
        let all = t.chapters(0..t.end());
        assert_eq!(all.lines().filter(|l| *l == "[CHAPTER]").count(), 3);
        assert!(all.contains("START=200000000\nEND=480000000\ntitle=Intro\n"), "{all}");
        assert!(all.contains("START=800000000\nEND=1600000000\ntitle=End\n"), "{all}");
        assert_eq!(t.chapters(21..30), "");
        assert_eq!(Timeline::new(25).chapters(0..10), "");
        assert_eq!(ffmeta_escape("a\\b\nc"), "a\\\\b\\\nc");
    }
}


// ---- STRIACLIPSPEEDTHREE (LEDGER SR9031): a rated outgoing's handle room, measured at its rate (named tail block) ----
// A transition's (or audio crossfade's, or fade-to-silence's) outgoing handle is THE `Clip::src_at` continued past the
// clip's `end()` (the schedule reads `o.src_at(t)`), so the timeline ticks of handle a source of `duration` holds are the
// largest `k` with `src_at(end + k - 1) < duration`: `rated_len(duration - src_in) - len()` (1x: `duration - src_out`).
// `validate` (the transitions, the audio crossfades, the fades to silence), `fit_trimmed`'s room and the schedule's
// `handles_fit` read this one fn.
impl Clip {
    /// STRIACLIPSPEEDTHREE: the timeline ticks of handle past `end()` that a source of `duration` ticks holds at this
    /// clip's rate (1x: `duration - src_out`; negative when the clip already runs past it).
    pub fn handle_room(&self, duration: i64) -> i64 {
        rated_len(duration - self.src_in, self.rate_q16) - self.len()
    }
}

#[cfg(test)]
mod striaclipspeedthree_tests {
    extern crate std;
    use super::*;
    use crate::editor::{Edge, EditorState};

    /// V1 on source A (`d` ticks): `clips` laid as given.
    fn tl(d: i64, clips: Vec<Clip>) -> Timeline {
        let mut tl = Timeline::new(25);
        tl.add_source("A", d);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = clips;
        tl
    }

    fn a(src_in: i64, src_out: i64, at: i64) -> Clip {
        Clip::new(SourceId(0), src_in, src_out, at)
    }

    /// Every source is `50` ticks with a keyframe at 0.
    struct Idx;
    impl SourceIndex for Idx {
        fn duration(&self, _: SourceId) -> Option<i64> {
            Some(50)
        }
        fn keyframe_ns(&self, _: SourceId, _: Kind, _: i64) -> Option<i64> {
            Some(0)
        }
    }

    #[test]
    fn striaclipspeedthree_handle_room_kats() {
        let c = a(0, 10, 0);
        assert_eq!(c.handle_room(50), 40, "1x: duration - src_out");
        assert_eq!(c.at_rate(0x2_0000).handle_room(50), 20, "2x: 25 - 5");
        assert_eq!(c.at_rate(0x8000).handle_room(50), 80, "0.5x: 100 - 20");
        let f = c.at_rate(0x1_8000);
        assert_eq!(f.handle_room(50), 27, "1.5x: 34 - 7");
        assert!(f.src_at(f.end() + 26) < 50 && f.src_at(f.end() + 27) >= 50, "the room's last tick reads inside the source");
        std::println!("striaclipspeedthree: handle room over 50: 1x 40, 2x 20, 0.5x 80, 1.5x 27");
    }

    #[test]
    fn striaclipspeedthree_validate_measures_the_handle_at_the_outgoing_rate() {
        // Source 14: the outgoing 0..10 leaves a 1x handle of 4 source ticks; at 2x that is 2 timeline ticks.
        let one = tl(14, alloc::vec![a(0, 10, 0), a(0, 10, 10)]);
        assert!(one.add_transition(0, 10, TransitionKind::Dissolve, 3).is_ok(), "1x: room 4");
        let two = tl(14, alloc::vec![a(0, 10, 0).at_rate(0x2_0000), a(0, 10, 5)]);
        assert_eq!(two.add_transition(0, 5, TransitionKind::Dissolve, 3), Err(EditError::Handle), "2x: room 2");
        assert!(two.add_transition(0, 5, TransitionKind::Dissolve, 2).is_ok());
        std::println!("striaclipspeedthree: a 3-tick dissolve on a 2x outgoing with 4 source ticks of handle -> {}", EditError::Handle);
    }

    #[test]
    fn striaclipspeedthree_the_mix_reads_the_outgoing_at_its_rate() {
        let base = tl(50, alloc::vec![a(0, 10, 0).at_rate(0x2_0000), a(20, 30, 5)]).add_transition(0, 5, TransitionKind::Dissolve, 3).unwrap();
        let steps = crate::schedule::schedule(&base, 5..8, &Idx).unwrap();
        let mix: Vec<(i64, bool)> = steps.iter().filter_map(|s| s.vmix).map(|m| (m.src_t, m.seek.is_some())).collect();
        assert_eq!(mix, alloc::vec![(10, true), (12, false), (14, false)], "src_out, +2, +4: one run");
        let one = tl(50, alloc::vec![a(0, 10, 0), a(20, 30, 10)]).add_transition(0, 10, TransitionKind::Dissolve, 3).unwrap();
        let m1: Vec<i64> = crate::schedule::schedule(&one, 10..13, &Idx).unwrap().iter().filter_map(|s| s.vmix).map(|m| m.src_t).collect();
        assert_eq!(m1, alloc::vec![10, 11, 12], "1x: src_out + k as before");
        assert_eq!(crate::schedule::handle_rate(&base, 0, SourceId(0), 6), 0x2_0000);
        assert_eq!(crate::schedule::handle_rate(&base, 0, SourceId(0), 4), 0x1_0000, "no clip of A ends by tick 4");
        std::println!("striaclipspeedthree: a 2x outgoing's dissolve mixes source 10, 12, 14 (1x: 10, 11, 12)");
    }

    #[test]
    fn striaclipspeedthree_trim_keys_step_one_timeline_tick_on_a_fast_clip() {
        let mut st = EditorState::new(tl(50, alloc::vec![a(0, 10, 0).at_rate(0x2_0000), a(20, 30, 10)]));
        st.select(0, 0).unwrap();
        assert_eq!(st.verb("}"), Some(Ok(())));
        assert_eq!(st.selected().map(|c| (c.src_out, c.end())), Some((12, 6)), "2x: out one tick later");
        assert_eq!(st.verb("{"), Some(Ok(())));
        assert_eq!(st.verb("{"), Some(Ok(())));
        assert_eq!(st.selected().map(|c| (c.src_out, c.end())), Some((8, 4)), "2x: out one tick earlier each press");
        for rate in [0x1_0000, 0x8000] {
            let base = tl(50, alloc::vec![a(4, 10, 5).at_rate(rate), a(20, 30, 30)]);
            let (mut k, mut s) = (EditorState::new(base.clone()), EditorState::new(base));
            k.select(0, 0).unwrap();
            s.select(0, 0).unwrap();
            for (key, edge, d) in [("[", Edge::In, -1), ("}", Edge::Out, 1)] {
                assert_eq!(k.verb(key), Some(Ok(())), "{key}");
                assert_eq!(s.trim(edge, d), Ok(()));
            }
            assert_eq!(k.timeline, s.timeline, "rate {rate:#x}: the keys step one source tick");
        }
        let mut none = EditorState::new(tl(50, alloc::vec![a(0, 10, 0)]));
        assert_eq!(none.verb("]"), Some(Err(EditError::NoClip)));
        std::println!("striaclipspeedthree: on a 2x clip }} ends at 6, {{ {{ at 4; 1x and 0.5x keys == trim by one source tick");
    }
}

// ---- STRIAFREEZETHREE (LEDGER SR9029): a title's style — size, alignment, ink (named tail block) ----
// A title source's look is ONE small value on its `Source` beside its text: `size` (the text height in percent of the frame
// height, 1..=50), `align` (left / centre / right) and `ink` (RGB). `TitleStyle::DEFAULT` (10, centre, white) is the look
// STRIATITLECLIP drew before, so a default-style title renders the same still and its EDL writes no `* UNA TITLESTYLE` line.
// The text may carry LF line breaks (the EDL's quoting already keeps them as `\n`); stria's one raster stacks the lines.

/// STRIAFREEZETHREE: where a title's lines sit across the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TitleAlign {
    Left,
    Centre,
    Right,
}

impl TitleAlign {
    /// STRIAFREEZETHREE: the EDL's and the word's name.
    pub fn name(self) -> &'static str {
        match self {
            TitleAlign::Left => "left",
            TitleAlign::Centre => "centre",
            TitleAlign::Right => "right",
        }
    }
    /// STRIAFREEZETHREE: the alignment a name says (`center` read as `centre`).
    pub fn from_name(s: &str) -> Option<TitleAlign> {
        match s {
            "left" => Some(TitleAlign::Left),
            "centre" | "center" => Some(TitleAlign::Centre),
            "right" => Some(TitleAlign::Right),
            _ => None,
        }
    }
}

/// STRIAFREEZETHREE: a title source's look (see the block comment).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TitleStyle {
    /// The text height, percent of the frame height (1..=50; 10 is a tenth, the look before SR9029).
    pub size: u8,
    pub align: TitleAlign,
    /// Straight RGB ink.
    pub ink: [u8; 3],
}

impl Default for TitleStyle {
    fn default() -> TitleStyle {
        TitleStyle::DEFAULT
    }
}

/// STRIAFREEZETHREE: the largest title size, percent of the frame height.
pub const TITLE_SIZE_MAX: u8 = 50;

impl TitleStyle {
    /// STRIAFREEZETHREE: a tenth of the frame height, centred, white.
    pub const DEFAULT: TitleStyle = TitleStyle { size: 10, align: TitleAlign::Centre, ink: [255, 255, 255] };

    /// STRIAFREEZETHREE: the EDL's three words, `<size> <align> <RRGGBB>` (e.g. `15 left FFCC00`).
    pub fn words(&self) -> String {
        alloc::format!("{} {} {:02X}{:02X}{:02X}", self.size, self.align.name(), self.ink[0], self.ink[1], self.ink[2])
    }
    /// STRIAFREEZETHREE: the style [`TitleStyle::words`] wrote: exactly three words; `None` for anything else.
    pub fn from_words(s: &str) -> Option<TitleStyle> {
        let mut w = s.split_whitespace();
        let (size, align, ink) = (title_size(w.next()?)?, TitleAlign::from_name(w.next()?)?, title_ink(w.next()?)?);
        w.next().is_none().then_some(TitleStyle { size, align, ink })
    }
    /// STRIAFREEZETHREE: this style with the `title-style` word's `size=N` / `align=A` / `ink=RRGGBB` pairs applied (any subset,
    /// any order); `None` for an unknown key, a bad value or a word with no `=`.
    pub fn with_words(self, s: &str) -> Option<TitleStyle> {
        let mut out = self;
        for kv in s.split_whitespace() {
            match kv.split_once('=')? {
                ("size", v) => out.size = title_size(v)?,
                ("align", v) => out.align = TitleAlign::from_name(v)?,
                ("ink", v) => out.ink = title_ink(v)?,
                _ => return None,
            }
        }
        Some(out)
    }
}

/// STRIAFREEZETHREE: a title size word, 1..=[`TITLE_SIZE_MAX`] in plain digits.
fn title_size(s: &str) -> Option<u8> {
    if s.is_empty() || s.len() > 2 || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse::<u8>().ok().filter(|n| (1..=TITLE_SIZE_MAX).contains(n))
}

/// STRIAFREEZETHREE: an ink word, exactly six hex digits `RRGGBB` (either case).
fn title_ink(s: &str) -> Option<[u8; 3]> {
    if s.len() != 6 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let h = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
    Some([h(0)?, h(2)?, h(4)?])
}

impl Timeline {
    /// STRIAFREEZETHREE: the timeline with title source `src`'s style set to `style`. Refused by name: no such source, or a
    /// media reel (`title == None`), `EditError::NoSource`.
    pub fn set_title_style(&self, src: SourceId, style: TitleStyle) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        let s = tl.sources.iter_mut().find(|s| s.id == src).filter(|s| s.title.is_some()).ok_or(EditError::NoSource)?;
        s.title_style = style;
        Ok(tl)
    }
}

#[cfg(test)]
mod striafreezethree_tests {
    use super::*;

    /// The three EDL words round-trip; the word pairs apply over a style; bad words are `None`.
    #[test]
    fn striafreezethree_title_style_words() {
        assert_eq!(TitleStyle::DEFAULT.words(), "10 centre FFFFFF");
        assert_eq!(TitleStyle::default(), TitleStyle::DEFAULT);
        let s = TitleStyle { size: 15, align: TitleAlign::Left, ink: [0xFF, 0xCC, 0x00] };
        assert_eq!(s.words(), "15 left FFCC00");
        assert_eq!(TitleStyle::from_words("15 left FFCC00"), Some(s));
        assert_eq!(TitleStyle::from_words("15 center ffcc00"), Some(TitleStyle { align: TitleAlign::Centre, ..s }));
        for bad in ["", "15 left", "15 left FFCC00 x", "0 left FFCC00", "51 left FFCC00", "15 up FFCC00", "15 left FFCC0", "+5 left FFCC00", "15 left GGCC00"] {
            assert_eq!(TitleStyle::from_words(bad), None, "{bad:?}");
        }
        assert_eq!(TitleStyle::DEFAULT.with_words("ink=FFCC00 size=15 align=left"), Some(s));
        assert_eq!(s.with_words("align=right"), Some(TitleStyle { align: TitleAlign::Right, ..s }), "a subset keeps the rest");
        assert_eq!(s.with_words(""), Some(s));
        for bad in ["size", "size=0", "size=51", "font=sans", "align=up", "ink=red", "size=15 x"] {
            assert_eq!(TitleStyle::DEFAULT.with_words(bad), None, "{bad:?}");
        }
    }

    /// A titled source's style changes and nothing else; a media reel and a missing source are `NoSource`.
    #[test]
    fn striafreezethree_set_title_style_only_titles() {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        let t = tl.add_source("T1", 125);
        tl.sources[1].title = Some(String::from("Hello"));
        assert_eq!(tl.sources[1].title_style, TitleStyle::DEFAULT, "a new source has the default style");
        let s = TitleStyle { size: 20, ..TitleStyle::DEFAULT };
        let out = tl.set_title_style(t, s).unwrap();
        assert_eq!(out.source(t).map(|x| x.title_style), Some(s));
        let mut back = out.clone();
        back.sources[1].title_style = TitleStyle::DEFAULT;
        assert_eq!(back, tl, "only the style moved");
        assert_eq!(tl.set_title_style(a, s), Err(EditError::NoSource), "a media reel");
        assert_eq!(tl.set_title_style(SourceId(99), s), Err(EditError::NoSource), "no such source");
    }
}

// ---- STRIAEXPORTPRESETSTHREE (LEDGER SR9021): THE export preset table, shared by both rings (named tail block) ----
/// THE export preset table (SR871's rows, SR989's scale), moved here unchanged so the host export (`stria::nle_export`
/// re-exports every name) and the kernel's `nle export --preset` (`video/export.rs`) read the same rows.
pub mod export_presets {
    use alloc::string::String;
    use alloc::vec::Vec;

    /// One export preset: a video word (the host's `VIDEO_NAMES`), an audio word (the host's `audio_named`), the H.264
    /// knobs it sets and the frame's divisor.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct ExportPreset {
        /// The word after `--preset`.
        pub name: &'static str,
        /// A video word (`h264`, `png`, ...).
        pub video: &'static str,
        /// An audio word (`aac`, `aac:K`, `wav`, ...).
        pub audio: &'static str,
        /// The rate controller's starting QP under a CRF.
        pub qp: u8,
        /// Constant quality 0..=51.
        pub crf: Option<u8>,
        /// Scene-cut IDR threshold in percent.
        pub scenecut: Option<u8>,
        /// Adaptive quantisation strength in tenths.
        pub aq: Option<u8>,
        /// The frame's width / height divisor (1 = the timeline's size), applied through [`scaled_size`].
        pub scale: u8,
    }

    /// The AAC-LC rate of the bare `aac` word, kb/s (the host's `DEFAULT_AAC_KBPS`).
    pub const AAC_KBPS: u32 = 128;

    /// THE preset table: `web` (H.264 at CRF 23 with AQ and scene cuts, AAC-LC 128), `archive` (the lossless PNG sequence and
    /// PCM16 WAV), `proxy` (H.264 at CRF 32, AAC-LC 64, half size).
    pub const PRESETS: [ExportPreset; 3] = [
        ExportPreset { name: "web", video: "h264", audio: "aac", qp: 26, crf: Some(23), scenecut: Some(40), aq: Some(10), scale: 1 },
        ExportPreset { name: "archive", video: "png", audio: "wav", qp: 26, crf: None, scenecut: None, aq: None, scale: 1 },
        ExportPreset { name: "proxy", video: "h264", audio: "aac:64", qp: 26, crf: Some(32), scenecut: None, aq: None, scale: 2 },
    ];

    /// The preset named `word`; None for any other word.
    pub fn preset_named(word: &str) -> Option<&'static ExportPreset> {
        PRESETS.iter().find(|p| p.name == word)
    }

    /// The preset names as the usages and refusals spell them: `web|archive|proxy`.
    pub fn preset_words() -> String {
        PRESETS.iter().map(|p| p.name).collect::<Vec<_>>().join("|")
    }

    /// The coded size of a `w x h` frame divided by `div`: each side divided, rounded down to even (4:2:0), at least 2.
    pub fn scaled_size(w: u32, h: u32, div: u32) -> (u32, u32) {
        let d = div.max(1);
        (((w / d) & !1).max(2), ((h / d) & !1).max(2))
    }

    impl ExportPreset {
        /// The preset's one listing line: `<name>: <video> + <audio>` then ` crf=N` / ` aq=N` / ` scenecut=N` for each set knob,
        /// and ` scale=1/N` when the frame is divided.
        pub fn words(&self) -> String {
            let knobs: String = [("crf", self.crf), ("aq", self.aq), ("scenecut", self.scenecut)].iter().filter_map(|(k, v)| v.map(|v| alloc::format!(" {k}={v}"))).collect();
            alloc::format!("{}: {} + {}{knobs}{}", self.name, self.video, self.audio, if self.scale > 1 { alloc::format!(" scale=1/{}", self.scale) } else { String::new() })
        }
        /// The AAC-LC rate the audio word names: `aac` = [`AAC_KBPS`], `aac:K` = K (K > 0); None for any other word (the kernel's
        /// export, which has no other audio word a preset can name, refuses it).
        pub fn aac_kbps(&self) -> Option<u32> {
            match self.audio.split_once(':') {
                None => (self.audio == "aac").then_some(AAC_KBPS),
                Some(("aac", k)) => k.parse::<u32>().ok().filter(|&k| k > 0),
                Some(_) => None,
            }
        }
    }
}

// ---- STRIAXFORMKEYS (LEDGER SR9049): per-clip placement keyframes, linear between points through the one clamp (named tail block) ----
// A video clip may carry up to [`XKeys::MAX`] placement points ([`Clip::xkeys`]), each a SOURCE tick and an [`Xform`], so
// split / trim / move / slip keep every point on the frame it was set on. [`XKeys::at`] holds the first point before it and
// the last after it, and between two points moves x, y and s linearly, through SR975's ONE clamp [`Xform::clamped`].
// [`Clip::xform_at`] is the placement shown at a timeline tick (the static `xform` when unkeyed: byte-identical), read by
// [`Timeline::xform_at`], the one lookup the render's composite reads; [`Clip::placed`] is what the schedule (two-layer
// step) and the kernel Player's refusal ask. Stored in the EDL as `* UNA XKEY <src_tick> <x> <y> <s>` lines (edl.rs).

/// A clip's placement points: `n` of them, sorted by source tick, unique ticks, each rect inside the frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct XKeys {
    n: u8,
    k: [(i64, Xform); XKEYS_MAX],
}

/// The most points one clip carries (the array length; [`XKeys::MAX`]).
const XKEYS_MAX: usize = 8;

impl Default for XKeys {
    fn default() -> XKeys {
        XKeys::NONE
    }
}

impl XKeys {
    /// The most points one clip carries.
    pub const MAX: usize = XKEYS_MAX;
    /// No points (the clip's static `xform` is shown).
    pub const NONE: XKeys = XKeys { n: 0, k: [(0, Xform::NONE); XKEYS_MAX] };
    pub fn len(&self) -> usize {
        self.n as usize
    }
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
    /// The points, by source tick.
    pub fn iter(&self) -> impl Iterator<Item = (i64, Xform)> + '_ {
        self.k[..self.len()].iter().copied()
    }
    /// The point at source tick `t`, if one is there.
    pub fn get(&self, t: i64) -> Option<Xform> {
        self.iter().find(|p| p.0 == t).map(|p| p.1)
    }
    /// The same points with `xf` at source tick `t` (replacing the one there); `None` when a new tick finds them full.
    pub fn with(self, t: i64, xf: Xform) -> Option<XKeys> {
        let mut out = self;
        if let Some(i) = self.iter().position(|p| p.0 == t) {
            out.k[i].1 = xf;
            return Some(out);
        }
        if self.len() >= XKeys::MAX {
            return None;
        }
        let i = self.iter().position(|p| p.0 > t).unwrap_or(self.len());
        out.k.copy_within(i..self.len(), i + 1);
        out.k[i] = (t, xf);
        out.n += 1;
        Some(out)
    }
    /// The same points without the one at source tick `t`; `None` when none is there.
    pub fn without(self, t: i64) -> Option<XKeys> {
        let i = self.iter().position(|p| p.0 == t)?;
        let mut out = self;
        out.k.copy_within(i + 1..self.len(), i);
        out.n -= 1;
        out.k[out.len()] = (0, Xform::NONE);
        Some(out)
    }
    /// Sorted, unique ticks, at most [`XKeys::MAX`], every rect inside the frame.
    pub fn ok(&self) -> bool {
        self.len() <= XKeys::MAX && self.iter().all(|p| p.1.ok()) && self.k[..self.len()].windows(2).all(|w| w[0].0 < w[1].0)
    }
    /// The placement at source tick `t`: `None` with no points; the first point's before it, the last point's after it,
    /// else x, y and s linear between the two points around `t`, through [`Xform::clamped`].
    pub fn at(&self, t: i64) -> Option<Xform> {
        let k = &self.k[..self.len()];
        let (first, last) = (*k.first()?, k[k.len() - 1]);
        if t <= first.0 {
            return Some(first.1);
        }
        if t >= last.0 {
            return Some(last.1);
        }
        let i = k.iter().position(|p| p.0 > t)?;
        let ((t0, a), (t1, b)) = (k[i - 1], k[i]);
        let lerp = |p: u16, q: u16| (p as i64 + (q as i64 - p as i64) * (t - t0) / (t1 - t0)) as i32;
        Some(Xform::clamped(lerp(a.s, b.s), lerp(a.x, b.x), lerp(a.y, b.y)))
    }
}

impl Clip {
    /// The source tick timeline tick `t` maps to on this clip (rate 1).
    pub fn src_tick(&self, t: i64) -> i64 {
        self.src_in + (t - self.at)
    }
    /// The placement shown at timeline tick `t`: the points' ([`XKeys::at`]) when keyed, else the static `xform`.
    pub fn xform_at(&self, t: i64) -> Xform {
        self.xkeys.at(self.src_tick(t)).unwrap_or(self.xform)
    }
    /// True when the clip is placed anywhere: a static placement other than the whole frame, or any point.
    pub fn placed(&self) -> bool {
        self.xform != Xform::NONE || !self.xkeys.is_empty()
    }
}

impl Timeline {
    /// The same timeline with clip `idx` of `track` carrying the points `keys` ([`XKeys::NONE`] clears). Refused: no such
    /// track (`NoTrack`) or clip (`NoClip`), an audio track (`Kind`), points out of order or off the frame (`Xform`).
    pub fn set_xkeys(&self, track: usize, idx: usize, keys: XKeys) -> Result<Timeline, EditError> {
        if self.tracks.get(track).ok_or(EditError::NoTrack)?.kind != Kind::Video {
            return Err(EditError::Kind);
        }
        if !keys.ok() {
            return Err(EditError::Xform);
        }
        let mut tl = self.clone();
        clip_mut(&mut tl, track, idx)?.xkeys = keys;
        Ok(tl)
    }
}

// ---- STRIAOPACITYKEYS (LEDGER SR9048): animated opacity, the points and their one reader (named tail block) ----
// A clip may carry up to [`OPACITY_KEYS_MAX`] opacity points (source tick, percent) on its SOURCE ticks, so split, trims,
// moves and rolls keep the curve with the picture and no op edits them. [`Clip::opacity_at`] is the ONE reader: no
// points, the static `opacity`; one point, its percent; before the first / after the last, held; between, linear
// (rounded half up). `schedule::opacity_mix` reads it per tick; `* UNA ALPHAKEYS` in the EDL; the editor's
// `opacity-key [<pct>]` / `opacity-unkey` words.

/// STRIAOPACITYKEYS: the most opacity points one clip carries.
pub const OPACITY_KEYS_MAX: usize = 8;

/// STRIAOPACITYKEYS: a clip's opacity points, ascending source ticks (a fixed array: `Clip` stays `Copy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpacityKeys {
    n: u8,
    tick: [i64; OPACITY_KEYS_MAX],
    pct: [u8; OPACITY_KEYS_MAX],
}

impl OpacityKeys {
    /// STRIAOPACITYKEYS: no points (every clip's default).
    pub const NONE: OpacityKeys = OpacityKeys { n: 0, tick: [0; OPACITY_KEYS_MAX], pct: [0; OPACITY_KEYS_MAX] };
    /// STRIAOPACITYKEYS: the number of points.
    pub fn len(&self) -> usize {
        self.n as usize
    }
    /// STRIAOPACITYKEYS: no points.
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
    /// STRIAOPACITYKEYS: the points, (source tick, percent), ascending.
    pub fn iter(&self) -> impl Iterator<Item = (i64, u8)> + '_ {
        (0..self.len()).map(move |i| (self.tick[i], self.pct[i]))
    }
    /// STRIAOPACITYKEYS: these points with `pct` at source tick `tick` (replacing a point there); `None` when full.
    pub fn with(self, tick: i64, pct: u8) -> Option<OpacityKeys> {
        let n = self.len();
        let i = self.tick[..n].iter().position(|&k| k >= tick).unwrap_or(n);
        let mut k = self;
        if i < n && k.tick[i] == tick {
            k.pct[i] = pct;
            return Some(k);
        }
        if n == OPACITY_KEYS_MAX {
            return None;
        }
        for j in (i..n).rev() {
            k.tick[j + 1] = k.tick[j];
            k.pct[j + 1] = k.pct[j];
        }
        k.tick[i] = tick;
        k.pct[i] = pct;
        k.n += 1;
        Some(k)
    }
    /// STRIAOPACITYKEYS: these points without the one at source tick `tick`; `None` when there is none.
    pub fn without(self, tick: i64) -> Option<OpacityKeys> {
        let n = self.len();
        let i = self.tick[..n].iter().position(|&k| k == tick)?;
        let mut k = self;
        for j in i..n - 1 {
            k.tick[j] = k.tick[j + 1];
            k.pct[j] = k.pct[j + 1];
        }
        k.tick[n - 1] = 0;
        k.pct[n - 1] = 0;
        k.n -= 1;
        Some(k)
    }
    /// STRIAOPACITYKEYS: the percent at source tick `s` (held outside, linear between, rounded half up); `None` with no points.
    pub fn at(&self, s: i64) -> Option<u8> {
        let n = self.len();
        if n == 0 {
            return None;
        }
        if s <= self.tick[0] {
            return Some(self.pct[0]);
        }
        if s >= self.tick[n - 1] {
            return Some(self.pct[n - 1]);
        }
        let j = self.tick[..n].iter().position(|&k| k > s)?;
        let (k0, k1, p0, p1) = (self.tick[j - 1], self.tick[j], self.pct[j - 1] as i64, self.pct[j] as i64);
        let span = k1 - k0;
        Some((p0 + ((p1 - p0) * (s - k0) * 2 + span).div_euclid(2 * span)) as u8)
    }
    /// STRIAOPACITYKEYS: `validate`'s rule: ticks >= 0 strictly ascending, each percent at most [`OPAQUE`], points on video only.
    fn valid(&self, kind: Kind) -> bool {
        let n = self.len();
        if n == 0 {
            return true;
        }
        kind == Kind::Video
            && n <= OPACITY_KEYS_MAX
            && self.tick[0] >= 0
            && self.pct[..n].iter().all(|&p| p <= OPAQUE)
            && self.tick[..n].windows(2).all(|w| w[0] < w[1])
    }
}

impl Clip {
    /// STRIAOPACITYKEYS: the source tick timeline tick `t` shows (the clock the opacity points live on).
    pub fn key_tick(&self, t: i64) -> i64 {
        self.src_in + (t - self.at)
    }
    /// STRIAOPACITYKEYS: THE opacity at timeline tick `t`: the points' value, else the static `opacity`.
    pub fn opacity_at(&self, t: i64) -> u8 {
        self.opacity_keys.at(self.key_tick(t)).unwrap_or(self.opacity)
    }
    /// STRIAOPACITYKEYS: the same clip with opacity points `keys`.
    pub fn with_opacity_keys(self, keys: OpacityKeys) -> Clip {
        Clip { opacity_keys: keys, ..self }
    }
}

impl Timeline {
    /// STRIAOPACITYKEYS: a point at timeline tick `t` on every clip in `picks` that covers `t`, at `pct` (or the clip's
    /// own `opacity_at(t)`), as ONE edit. Refused by name: no pick covers `t`, or a clip is full
    /// (`EditError::OpacityKey`); a percent above [`OPAQUE`] or an audio clip (`EditError::Opacity`).
    pub fn set_opacity_key(&self, picks: &[(usize, usize)], t: i64, pct: Option<u8>) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        let mut any = false;
        for &(track, idx) in picks {
            let c = clip_mut(&mut tl, track, idx)?;
            if !c.covers(t) {
                continue;
            }
            let p = pct.unwrap_or(c.opacity_at(t));
            if p > OPAQUE {
                return Err(EditError::Opacity);
            }
            let kt = c.key_tick(t);
            c.opacity_keys = c.opacity_keys.with(kt, p).ok_or(EditError::OpacityKey)?;
            any = true;
        }
        if !any {
            return Err(EditError::OpacityKey);
        }
        tl.validate()?;
        Ok(tl)
    }
    /// STRIAOPACITYKEYS: the point at timeline tick `t` removed from every clip in `picks` that covers `t` and has one
    /// there, as ONE edit; refused by name (`EditError::OpacityKey`) when no such point exists.
    pub fn clear_opacity_key(&self, picks: &[(usize, usize)], t: i64) -> Result<Timeline, EditError> {
        let mut tl = self.clone();
        let mut any = false;
        for &(track, idx) in picks {
            let c = clip_mut(&mut tl, track, idx)?;
            if !c.covers(t) {
                continue;
            }
            let kt = c.key_tick(t);
            if let Some(k) = c.opacity_keys.without(kt) {
                c.opacity_keys = k;
                any = true;
            }
        }
        if !any {
            return Err(EditError::OpacityKey);
        }
        tl.validate()?;
        Ok(tl)
    }
}
