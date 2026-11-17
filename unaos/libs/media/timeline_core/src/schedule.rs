// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The render schedule (NLE1 M2, LEDGER SR194): what each output tick shows, in output order, and where a decoder
//! must seek.
//!
//! Video is cut-only: at tick `t` the HIGHEST-numbered video track with a clip at `t` wins (V2 over V1); its transitions are
//! below. Audio is a MIX (NLE4, LEDGER SR203): [`audio_runs`] lists, per run, EVERY audio track's clip with its gain and
//! fade envelope; a run is the longest span over which every audio track shows the same clip in the same envelope
//! segment (fade-in, body, fade-out) or nothing. For each output tick the schedule holds one video [`Step`] (`len` 1),
//! then the audio step whose run starts at that tick, if any: one step per [`AudioRun`], in the same order, whose
//! `track` / `src` / `src_t` / `seek` are its TOP layer's (with one audio track, exactly NLE1's cut-only runs). A kind
//! with no track gets no steps. A step with `src: None` is a gap: black, or silence.
//!
//! Keyframe-aware: the first step of a run whose source position does not continue the previous step of its kind (a
//! cut to another source, or a jump inside the same one) carries `seek`: the keyframe at or before its source time,
//! from [`SourceIndex::keyframe_ns`]. The renderer seeks there and decodes up to `src_t` unseen (the run-up). A step
//! that continues the previous one (a split, or two clips butted end to start in source order) carries none.
//!
//! Transitions (NLE3, LEDGER SR202): the rule above still picks the winning track and clip. When the winning clip's
//! cut carries a [`crate::Transition`] and `t` lies in its first `dur` ticks, the video step also carries [`VideoMix`]:
//! the OUTGOING source at its handle tick (`src_out + k`, `k = t - at`), with its own seek (none when it continues the
//! previous step's source position: the outgoing run decodes on past its out point), and the incoming's weight
//! `t = (k + 1) / (dur + 1)` (never 0 or 1). The step's own `src` / `src_t` / `seek` stay the incoming clip's, which
//! seeks at the cut as a plain cut does. NLE3c (SR209): a transition from black names no outgoing source
//! (`VideoMix::src` `None`: opaque black, no seek). NLE3cBLACK (SR323): a transition TO black owns its track's gap
//! `at..at + dur` (that track wins those ticks): the step's own `src` is `None` (the incoming is black) and its
//! `VideoMix` names the outgoing at its handle, continuing the outgoing's run (no seek at the cut).
//!
//! Audio crossfades (NLE3b, LEDGER SR209): on an audio cut paired with a video transition
//! ([`crate::Timeline::audio_crossfade`]) a run breaks at `at` and `at + dur`, and inside that span the track gives TWO
//! layers in this order: the UNDER layer (the outgoing source at its handle `src_out + (t - at)`, its gain, the falling
//! ramp `ENV_ONE - r`, zero throughout if the outgoing clip faded out) and the incoming clip's own layer (its envelope
//! `min(fade, r)`), `r = (t - at) * ENV_ONE / dur` at each run edge: LINEAR, the two weights summing to `ENV_ONE`. The
//! under layer has its own continuity: it seeks at the span's first run and continues after. (Exact except where an
//! incoming fade-out reaches into the span: interpolated linearly between run edges.)
//!
//! NLEXFADEPOWER (LEDGER SR678): the fade on an audio cut is read through ONE accessor, [`crate::Timeline::crossfade_at`]
//! (an explicit [`crate::AudioXfade`] first, else the paired crossfade above, else a fade FROM silence under a transition
//! from black: the incoming's one layer, no under layer), and every weight through ONE fn, [`xfade_weight`]: the rising
//! weight is `xfade_weight(curve, r)` and the falling one `xfade_weight(curve, ENV_ONE - r)` (linear: exactly the ramp
//! above). An [`crate::Curve::EqualPower`] span breaks its runs at EVERY tick, so each edge carries the sine / cosine weight
//! (the renderers interpolate linearly between edges); those runs continue their sources (no seek, the same decoder). A
//! transition TO black fades the outgoing's handle out in the track's gap ([`crate::Timeline::fade_to_silence`]): the
//! clip's layer continues past its end under the falling weight, on its own slot.
//! STRIAOPACITY (LEDGER SR868): a winning clip below [`crate::OPAQUE`] carries a Dissolve [`VideoMix`] of the highest video track under it (its clip at its own tick, or black) at `t = opacity / 100`, the outgoing-run slot holding the track below; an opaque winner's step is unchanged, and a stack of more than four layers is refused by name ([`ScheduleError::Opacity`]); STRIASTACK (SR9066): the layers under the mixed one ride [`Step::stack`], bottom to top.

use alloc::collections::VecDeque;
use alloc::vec::Vec;
use core::ops::Range;

use crate::{EditError, Kind, SourceId, SourceIndex, Timeline, TransitionKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub kind: Kind,
    /// The winning track's index in [`Timeline::tracks`] (`None` for a gap).
    pub track: Option<usize>,
    /// `None` = gap.
    pub src: Option<SourceId>,
    /// The first source tick shown (0 for a gap).
    pub src_t: i64,
    /// The first output tick.
    pub out_t: i64,
    /// Ticks covered (1 for video).
    pub len: i64,
    /// Seek here (ns from the source's start, a keyframe) before decoding this step.
    pub seek: Option<i64>,
    /// NLE3: inside a transition (video only), the outgoing source mixed under this step's.
    pub vmix: Option<VideoMix>,
    /// STRIASTACK (SR9066): the layers under `vmix`'s source, bottom to top from index 0 (each `t` the weight of the layer on it; index 0 may be black); [`NO_STACK`] for a two-layer step.
    pub stack: [Option<VideoMix>; STACK_UNDER],
}

/// A fraction `num / den` (`den > 0`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Frac {
    pub num: i64,
    pub den: i64,
}

/// NLE3: the second source of a video step inside a transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VideoMix {
    pub kind: TransitionKind,
    /// The OUTGOING clip's source (`None`: black, a transition from `BL`).
    pub src: Option<SourceId>,
    /// Its source tick (in its handle: the outgoing's `src_at(t)` past its end, 1x `src_out + k`; STRIACLIPSPEEDTHREE SR9031).
    pub src_t: i64,
    /// Seek here before decoding it (`None`: its run continues).
    pub seek: Option<i64>,
    /// The incoming's weight, in `(0, 1)`.
    pub t: Frac,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScheduleError {
    /// A negative or reversed range.
    Range,
    /// The timeline breaks an invariant.
    Invalid(EditError),
    /// A clip runs past what the index says the source holds.
    OutOfSource(SourceId),
    /// The source has no stream of the kind its track needs.
    NoKeyframe(SourceId),
    /// STRIAOPACITY (SR868): a non-opaque clip of this source over a transition, or topping a stack of more than four layers (STRIASTACK SR9066).
    Opacity(SourceId),
    /// STRIAPOSITIONSCALE (SR879): a placed (picture-in-picture) clip of this source over a transition, or placed under another layer (two layers only).
    Xform(SourceId),
}

impl core::fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ScheduleError::Range => f.write_str("bad range"),
            ScheduleError::Invalid(e) => write!(f, "invalid timeline: {e}"),
            ScheduleError::OutOfSource(s) => write!(f, "clip past the end of source {}", s.0),
            ScheduleError::NoKeyframe(s) => write!(f, "source {} has no stream to seek", s.0),
            ScheduleError::Opacity(s) => write!(f, "source {} is not opaque over a transition, or tops a stack of more than four layers", s.0),
            ScheduleError::Xform(s) => write!(f, "source {} is placed (pip / scale) over a transition or under another layer (two layers only)", s.0),
        }
    }
}

/// The winner at tick `t` for `kind`: (track index, clip index).
pub(crate) pub pub fn top(tl: &Timeline, kind: Kind, t: i64) -> Option<(usize, usize)> { // STRIAFREEZEFRAME (SR880): pub(crate), `Timeline::freeze` reads the same winner // STRIAKMUTE (SR9017): pub, the kernel Player's shown clip (vplay.rs `shown_clip`)
    tl.active_tracks().rev().filter(|&i| tl.tracks[i].kind == kind).find_map(|i| tl.clip_at(i, t).map(|c| (i, c))) // STRIATRACKMUTESOLO (SR837): a muted / unsoloed track never wins
}

/// The schedule for output ticks `range` (see the module docs).
pub fn schedule(tl: &Timeline, range: Range<i64>, idx: &dyn SourceIndex) -> Result<Vec<Step>, ScheduleError> {
    check(tl, &range, idx)?;
    handles_fit(tl, idx)?;
    let has = |k: Kind| tl.tracks.iter().any(|t| t.kind == k);
    let (video, audio) = (has(Kind::Video), has(Kind::Audio));
    let mut out = Vec::new();
    let mut m_next: Option<(SourceId, i64)> = None; let mut s_next: [Option<(SourceId, i64)>; STACK_UNDER] = [None; STACK_UNDER]; // STRIASTACK (SR9066): each stack index's continuation. NLE3: the transition's outgoing run
    // The previous video step's (source, the source tick it would continue at).
    let mut v_next: Option<(SourceId, i64)> = None;
    // NLE4: the audio steps are the mix runs' headers, one per run.
    let runs = if audio { audio_runs(tl, range.clone(), idx)? } else { Vec::new() };
    let mut run = runs.iter().peekable();
    for t in range.clone() {
        if video {
            let hp = (v_next, m_next, s_next); let win = top(tl, Kind::Video, t); // STRIASTACKKERNEL (SR9092): the previous tick's continuations, every slot (file tail)
            let step = match (black_over(tl, t, win.map(|w| w.0)), win) {
                (Some((ti, x)), _) => black_step(tl, idx, ti, &x, t, &mut v_next, &mut m_next)?,
                (None, Some((ti, ci))) => {
                    let c = tl.tracks[ti].clips[ci];
                    let src_t = c.src_at(t); // STRIACLIPSPEED (SR829): the clip's rate (1x: src_in + (t - at))
                    let m0 = m_next; let vmix = vmix_for(tl, idx, ti, ci, t, v_next, &mut m_next)?; let vmix = opacity_mix(tl, idx, ti, ci, t, vmix, (v_next, m0), &mut m_next)?; let stack = stack_under(tl, idx, ti, ci, t, &mut s_next)?; // STRIASTACK (SR9066): the layers under the mixed one (file tail). STRIAOPACITY (SR868): a non-opaque winner mixes the track below (file tail)
                    let seek = seek_for(tl, idx, Kind::Video, c.src, src_t, hand_back_prev(vmix.is_some(), m0, v_next, (c.src, src_t)))?; // STRIAOPACITYNEXT (SR993): the lower run positioned by the overlay continues without a seek (file tail)
                    v_next = Some((c.src, c.src_at(t + 1))); // STRIACLIPSPEED (SR829): a rated clip continues its run (1x: src_t + 1)
                    Step { kind: Kind::Video, track: Some(ti), src: Some(c.src), src_t, out_t: t, len: 1, seek, vmix, stack }
                }
                (None, None) => {
                    (v_next, m_next) = (None, None);
                    Step { kind: Kind::Video, track: None, src: None, src_t: 0, out_t: t, len: 1, seek: None, vmix: None, stack: NO_STACK }
                }
            };
            let step = stack_handoff(step, hp); if step.stack == NO_STACK { s_next = [None; STACK_UNDER]; } out.push(step); // STRIASTACKKERNEL (SR9092): a layer that changes slot continues its run (file tail)
        }
        if let Some(r) = run.next_if(|r| r.out_t == t) {
            out.push(r.header());
        }
    }
    Ok(out)
}

/// The range, invariant and media-length checks [`schedule`] and [`audio_runs`] share.
fn check(tl: &Timeline, range: &Range<i64>, idx: &dyn SourceIndex) -> Result<(), ScheduleError> {
    if range.start < 0 || range.end < range.start {
        return Err(ScheduleError::Range);
    }
    tl.validate().map_err(ScheduleError::Invalid)?;
    for c in tl.tracks.iter().flat_map(|t| t.clips.iter()) {
        if idx.duration(c.src).is_some_and(|d| c.src_out > d) {
            return Err(ScheduleError::OutOfSource(c.src));
        }
    }
    Ok(())
}

/// Full scale of a fade envelope (Q16).
pub const ENV_ONE: u32 = 1 << 16;

/// NLE4: one audio track's part of an [`AudioRun`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Layer {
    /// The track's index in [`Timeline::tracks`].
    pub track: usize,
    pub src: SourceId,
    /// The first source tick heard.
    pub src_t: i64,
    /// Seek here (ns from the source's start, a sync point) before decoding this layer; `None` when it continues the
    /// same track's previous layer.
    pub seek: Option<i64>,
    /// The clip's gain plus its track's ([`Timeline::heard_cdb`], STRIATRACKGAIN SR864), centi-dB.
    pub gain_cdb: i32,
    pub pan_cq8: i8, // STRIAPAN (LEDGER SR861): the clip's pan (Stria's mix_into and the kernel Player's snd apply [`pan_mix_q16`], STRIAKPAN SR9015)
    pub pan_to: i8, // STRIAPANAUTOMATION (LEDGER SR9050): the pan at the run's END edge (`pan_cq8` is the pan at its first tick; equal without pan points)
    /// The fade envelope (Q16, [`ENV_ONE`] = full) at the run's first tick and at its end edge; linear between.
    pub env: [u32; 2],
}

/// NLE4: one mix run: output ticks `out_t..out_t + len`, every audio track's layer (track order; none = silence).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioRun {
    pub out_t: i64,
    pub len: i64,
    pub layers: Vec<Layer>,
}

impl AudioRun {
    /// The run's [`Step`]: its top (highest-track) layer, or a gap.
    pub fn header(&self) -> Step {
        let (track, src, src_t, seek) = match self.layers.last() {
            Some(l) => (Some(l.track), Some(l.src), l.src_t, l.seek),
            None => (None, None, 0, None),
        };
        Step { kind: Kind::Audio, track, src, src_t, out_t: self.out_t, len: self.len, seek, vmix: None, stack: NO_STACK }
    }
}

/// NLE4: the audio mix runs for output ticks `range` (see the module docs).
pub fn audio_runs(tl: &Timeline, range: Range<i64>, idx: &dyn SourceIndex) -> Result<Vec<AudioRun>, ScheduleError> {
    check(tl, &range, idx)?;
    let tracks: Vec<usize> = tl.active_tracks().filter(|&i| tl.tracks[i].kind == Kind::Audio).collect(); // STRIATRACKMUTESOLO (SR837): only the active audio tracks mix
    if !tl.tracks.iter().any(|t| t.kind == Kind::Audio) { // STRIATRACKMUTESOLO (SR837): every audio track muted is silent runs, not none
        return Ok(Vec::new());
    }
    // NLE3b: the crossfade on clip `ci` of track `ti` covering tick `t`, if any.
    let xfade = |ti: usize, ci: usize, t: i64| tl.crossfade_at(ti, ci).filter(|x| t < x.at + x.dur);
    // What each audio track shows at `t`: (clip index, envelope segment 0 fade-in / 1 body / 2 fade-out, in a
    // crossfade).
    let key = |t: i64| -> Vec<Option<(usize, u8, i64)>> {
        tracks
            .iter()
            .map(|&ti| {
                tl.clip_at(ti, t).map(|ci| {
                    let c = &tl.tracks[ti].clips[ci];
                    let seg = if t < c.at + c.fade_in { 0 } else if t >= c.end() - c.fade_out { 2 } else { 1 };
                    (ci, seg, under_piece(tl, ti, ci, t, env_piece(c, t, xpiece(xfade(ti, ci, t), t)))) // STRIAGAINENVCURVE (SR9083): the outgoing's envelope breaks the run too (file tail)
                }).or_else(|| black_key(tl, ti, t))
            })
            .collect()
    };
    // Per track: (source, the source tick its previous layer would continue at).
    let mut next: Vec<Option<(SourceId, i64)>> = alloc::vec![None; tl.tracks.len()];
    // NLE3b: the same for each track's UNDER layer (the outgoing's handle inside a crossfade).
    let mut xnext: Vec<Option<(SourceId, i64)>> = alloc::vec![None; tl.tracks.len()];
    let mut out = Vec::new();
    let mut t = range.start;
    while t < range.end {
        let here = key(t); let phere = pan_piece(tl, &tracks, t); let uhere = under_pan_piece(tl, &tracks, t); // STRIAPANAUTOWRITE (SR9084): a keyed outgoing clip's pan splits the under layer's runs (file tail); STRIAPANAUTOMATION (SR9050): an automated clip's pan splits runs too (file tail)
        let mut end = t + 1;
        while end < range.end && key(end) == here && pan_piece(tl, &tracks, end) == phere && under_pan_piece(tl, &tracks, end) == uhere {
            end += 1;
        }
        let mut layers = Vec::new();
        for (k, &ti) in tracks.iter().enumerate() {
            match here[k] {
                Some((ci, _, _)) => {
                    let c = tl.tracks[ti].clips[ci];
                    let mut env = [env_q16(&c, t), env_q16(&c, end)];
                    match xfade(ti, ci, t) {
                        Some(x) => {
                            if x.partner == crate::Partner::Clip {
                                let o = tl.tracks[ti].clips[ci - 1];
                                let under = under_layer(tl, idx, ti, &o, &x, t, end, &mut xnext[ti])?;
                                layers.push(under);
                            } else {
                                xnext[ti] = None; // NLEXFADEPOWER: from silence: no under layer
                            }
                            env = [env[0].min(rise(&x, t)), env[1].min(rise(&x, end))];
                        }
                        None => xnext[ti] = None,
                    }
                    env = tail_env(tl, ti, ci, t, end, env); // NLEXFADEPOWER: a fade to silence past the clip's end
                    let src_t = c.src_at(t); // STRIACLIPSPEED (SR829): the clip's rate (1x: src_in + (t - at))
                    let seek = seek_for(tl, idx, Kind::Audio, c.src, src_t, next[ti])?;
                    next[ti] = Some((c.src, c.src_at(end))); // STRIACLIPSPEED (SR829): where the run's source ends at the clip's rate (1x: src_t + (end - t))
                    layers.push(Layer { track: ti, src: c.src, src_t, seek, gain_cdb: tl.heard_cdb(ti, c.gain_cdb + c.gain_env.peak()), pan_cq8: c.pan_at(t), pan_to: c.pan_at(end), env: genv_scale(&c, t, end, env) }); // STRIATRACKGAIN (SR864): the clip's gain plus its track's // STRIAPANAUTOMATION (SR9050): the pan at both edges
                }
                None => (next[ti], xnext[ti]) = (None, None),
            }
        }
        out.push(AudioRun { out_t: t, len: end - t, layers });
        t = end;
    }
    Ok(out)
}

fn seek_for(
    tl: &Timeline,
    idx: &dyn SourceIndex,
    kind: Kind,
    src: SourceId,
    src_t: i64,
    next: Option<(SourceId, i64)>,
) -> Result<Option<i64>, ScheduleError> {
    if next == Some((src, src_t)) {
        return Ok(None);
    }
    if kind == Kind::Video && tl.source(src).is_some_and(|s| s.title.is_some()) { return Ok(Some(tl.tb.to_ns(src_t))); } idx.keyframe_ns(src, kind, tl.tb.to_ns(src_t)).map(Some).ok_or(ScheduleError::NoKeyframe(src)) // STRIATITLECLIP (SR828): a title on a video track is generated: every frame is its own key, no SourceIndex asked
}

/// NLE4: clip `c`'s fade envelope at timeline tick (edge) `t`, Q16: rising over `at..at + fade_in`, falling over
/// `end - fade_out..end`, full between.
pub(crate) fn env_q16(c: &crate::Clip, t: i64) -> u32 {
    let mut e = ENV_ONE as i64;
    if c.fade_in > 0 && t < c.at + c.fade_in {
        e = e.min((t - c.at).max(0) * ENV_ONE as i64 / c.fade_in);
    }
    if c.fade_out > 0 && t > c.end() - c.fade_out {
        e = e.min((c.end() - t).max(0) * ENV_ONE as i64 / c.fade_out);
    }
    e as u32
}

/// NLE3: the outgoing source under video tick `t` of clip `ci` on track `ti`, if `t` lies in the transition on that
/// clip's cut. Its run continues the previous video step at the transition's first tick (`v_next`), then itself
/// (`m_next`); `m_next` is reset outside a transition.
fn vmix_for(
    tl: &Timeline,
    idx: &dyn SourceIndex,
    ti: usize,
    ci: usize,
    t: i64,
    v_next: Option<(SourceId, i64)>,
    m_next: &mut Option<(SourceId, i64)>,
) -> Result<Option<VideoMix>, ScheduleError> {
    let tr = &tl.tracks[ti];
    let c = tr.clips[ci];
    let Some(x) = tr.transitions.iter().find(|x| x.at == c.at && t < c.at + x.dur) else {
        *m_next = None;
        return Ok(None);
    };
    let k = t - c.at;
    let w = x.ease.apply(Frac { num: k + 1, den: x.dur + 1 }); // NLEWIPES (SR679): the ease, once, where t is born
    if x.from_black {
        *m_next = None;
        return Ok(Some(VideoMix { kind: x.kind, src: None, src_t: 0, seek: None, t: w }));
    }
    // validate: the outgoing clip ends at the cut, so it is the clip before (clips are ordered and never overlap).
    let o = tr.clips[ci - 1];
    let src_t = o.src_at(t); // STRIACLIPSPEEDTHREE (SR9031): the outgoing's handle at its rate, src_at past its end (1x: src_out + k)
    let seek = seek_for(tl, idx, Kind::Video, o.src, src_t, if k == 0 { v_next } else { *m_next })?;
    *m_next = Some((o.src, o.src_at(t + 1))); // STRIACLIPSPEEDTHREE (SR9031): the outgoing's handle at its rate, src_at past its end (1x: src_t + 1)
    Ok(Some(VideoMix { kind: x.kind, src: Some(o.src), src_t, seek, t: w }))
}

/// NLE3: every transition's outgoing handle lies inside what the index says the source holds.
fn handles_fit(tl: &Timeline, idx: &dyn SourceIndex) -> Result<(), ScheduleError> {
    for tr in &tl.tracks {
        for x in tr.transitions.iter().filter(|x| !x.from_black) {
            if let Some(o) = tr.clips.iter().find(|c| c.end() == x.at) {
                if idx.duration(o.src).is_some_and(|d| x.dur > o.handle_room(d)) { // STRIACLIPSPEEDTHREE (SR9031): the room at the outgoing's rate (1x: src_out + dur > d)
                    return Err(ScheduleError::OutOfSource(o.src));
                }
            }
        }
    }
    Ok(())
}

/// NLE3b: a crossfade's rising weight at timeline edge `t`, Q16: 0 at the cut, [`ENV_ONE`] at `at + dur`.
fn ramp_q16(x: &crate::Crossfade, t: i64) -> u32 {
    ((t - x.at).clamp(0, x.dur) * ENV_ONE as i64 / x.dur.max(1)) as u32
}

/// NLE3b: the UNDER layer of run `t..end` on track `ti`: the outgoing clip `o`'s source at its handle, its gain, the
/// falling ramp (scaled by the outgoing's own envelope at its end: 0 when it faded out). Its continuity is `xnext`.
#[allow(clippy::too_many_arguments)]
fn under_layer(
    tl: &Timeline,
    idx: &dyn SourceIndex,
    ti: usize,
    o: &crate::Clip,
    x: &crate::Crossfade,
    t: i64,
    end: i64,
    xnext: &mut Option<(SourceId, i64)>,
) -> Result<Layer, ScheduleError> {
    let src_t = o.src_at(t); // STRIACLIPSPEEDTHREE (SR9031): the outgoing's handle at its rate, src_at past its end (1x: src_out + (t - at))
    let seek = seek_for(tl, idx, Kind::Audio, o.src, src_t, *xnext)?;
    *xnext = Some((o.src, o.src_at(end))); // STRIACLIPSPEEDTHREE (SR9031): the outgoing's handle at its rate, src_at past its end (1x: src_t + (end - t))
    let tail = env_q16(o, o.end()) as i64;
    let fall = |e: i64| (xfade_weight(x.curve, ENV_ONE - ramp_q16(x, e)) as i64 * tail / ENV_ONE as i64) as u32;
    Ok(Layer { track: ti, src: o.src, src_t, seek, gain_cdb: tl.heard_cdb(ti, o.gain_cdb), pan_cq8: o.pan_at(t), pan_to: o.pan_at(end + o.gain_env.peak()), env: genv_scale(o, t, end, [fall(t), fall(end)]) }) // STRIATRACKGAIN (SR864): the outgoing's gain plus its track's // STRIAPANAUTOMATION (SR9050): the pan at both edges
}

/// NLE3cBLACK (LEDGER SR323): the highest video track above the clip winner's (`above`; any when `None`) whose
/// transition TO black spans tick `t`, with that transition.
pub(crate) fn black_over(tl: &Timeline, t: i64, above: Option<usize>) -> Option<(usize, crate::Transition)> { // STRIAKDUR (SR9089): pub(crate), the editor's gap pick reads the one rule
    tl.tracks
        .iter()
        .enumerate()
        .rev()
        .filter(|(i, tr)| tr.kind == Kind::Video && above.is_none_or(|a| *i > a) && tl.active_tracks().any(|a| a == *i)) // STRIATRACKMUTESOLO (SR837): a muted track's to-black does not show
        .find_map(|(i, tr)| tr.transitions.iter().find(|x| x.to_black && x.at <= t && t < x.at + x.dur).map(|x| (i, *x)))
}

/// NLE3cBLACK: the video step at tick `t` of the transition to black `x` on track `ti`: black as its own picture
/// (`src` `None`) under a [`VideoMix`] of the outgoing at its handle `src_out + k`, the incoming (black) weighted
/// `(k + 1) / (dur + 1)`. The outgoing's run continues the previous video step at the cut (`v_next`), then itself.
fn black_step(
    tl: &Timeline,
    idx: &dyn SourceIndex,
    ti: usize,
    x: &crate::Transition,
    t: i64,
    v_next: &mut Option<(SourceId, i64)>,
    m_next: &mut Option<(SourceId, i64)>,
) -> Result<Step, ScheduleError> {
    // validate: the outgoing clip ends at the cut.
    let o = *tl.tracks[ti].clips.iter().find(|c| c.end() == x.at).ok_or(ScheduleError::Invalid(EditError::Transition))?;
    let k = t - x.at;
    let src_t = o.src_at(t); // STRIACLIPSPEEDTHREE (SR9031): the outgoing's handle at its rate, src_at past its end (1x: src_out + k)
    let seek = seek_for(tl, idx, Kind::Video, o.src, src_t, if k == 0 { *v_next } else { *m_next })?;
    (*v_next, *m_next) = (None, Some((o.src, o.src_at(t + 1)))); // STRIACLIPSPEEDTHREE (SR9031): the outgoing's handle at its rate, src_at past its end (1x: src_t + 1)
    let vmix = VideoMix { kind: x.kind, src: Some(o.src), src_t, seek, t: x.ease.apply(Frac { num: k + 1, den: x.dur + 1 }) }; // NLEWIPES (SR679): the ease
    Ok(Step { kind: Kind::Video, track: Some(ti), src: None, src_t: 0, out_t: t, len: 1, seek: None, vmix: Some(vmix), stack: NO_STACK })
}

// ── NLE6MIXAUDIO (rmbp-ledger B953): the ONE audio slot rule (Stria's render and the kernel Player's EDL feed) ──────

impl AudioRun {
    /// Layer `li` is a crossfade's UNDER layer (the outgoing at its handle): the next layer is of the same track (NLE3b's
    /// order, under before incoming). It plays from its own decoder slot and, in the kernel, its own mixer source.
    pub fn under(&self, li: usize) -> bool {
        self.layers.get(li + 1).is_some_and(|m| self.layers.get(li).is_some_and(|l| m.track == l.track))
    }
}

// ---- NLEXFADEPOWER (LEDGER SR678): the ONE crossfade weight and its quarter-wave table (named tail block) ----

/// Q16 `sin(pi/2 * i / 256)`, `i` in `0..=256` (`QSIN[256] == ENV_ONE`): ONE quarter-wave table, built at compile time.
const QSIN: [u32; 257] = qsin_table();

/// The quarter-wave table by an integer Taylor series in Q60 (`i128`, no float): `sin x = x - x^3/3! + x^5/5! - ...` to
/// the x^23 term (the remainder is under 2^-60 for `x <= pi/2`), rounded to Q16. Each entry is `round(sin * 65536)`.
const fn qsin_table() -> [u32; 257] {
    const ONE: i128 = 1 << 60;
    const HALF_PI: i128 = 1_811_004_864_519_280_711; // round(pi / 2 * 2^60)
    let mut t = [0u32; 257];
    let mut i = 0;
    while i < 257 {
        let x = HALF_PI * i as i128 / 256;
        let x2 = x * x / ONE;
        let (mut term, mut sum, mut n) = (x, x, 1i128);
        while n < 12 {
            term = -term * x2 / ONE / ((2 * n) * (2 * n + 1));
            sum += term;
            n += 1;
        }
        t[i] = ((sum + (1 << 43)) >> 44) as u32;
        i += 1;
    }
    t
}

/// Q16 `sin(pi/2 * p / ENV_ONE)` for a phase `p` in `0..=ENV_ONE` (clamped), linear between the table's entries.
fn qsin(p: u32) -> u32 {
    let p = p.min(ENV_ONE);
    let (i, f) = ((p >> 8) as usize, p & 255);
    if i == 256 {
        return QSIN[256];
    }
    QSIN[i] + (((QSIN[i + 1] - QSIN[i]) * f + 128) >> 8)
}

/// NLEXFADEPOWER: THE crossfade weight (Q16) at the linear phase `p` (Q16, 0 at the cut, [`ENV_ONE`] at its end, clamped):
/// [`crate::Curve::Linear`] is `p` itself (NLE3b's ramp, unchanged), [`crate::Curve::EqualPower`] is `sin(pi p / 2)` from the
/// quarter-wave table. The rising weight is `xfade_weight(c, p)`, the falling one `xfade_weight(c, ENV_ONE - p)` (cos).
pub fn xfade_weight(curve: crate::Curve, p: u32) -> u32 {
    match curve {
        crate::Curve::Linear => p.min(ENV_ONE),
        crate::Curve::EqualPower => qsin(p),
    }
}

/// The rising weight of fade `x` at timeline edge `e`.
fn rise(x: &crate::Crossfade, e: i64) -> u32 {
    xfade_weight(x.curve, ramp_q16(x, e))
}

/// The run key's fade word for a tick inside fade `x` (or none): -1 outside, 0 a linear fade (one run per span), and
/// `t + 1` an equal-power one (one run per tick, so every edge carries its own weight).
fn xpiece(x: Option<crate::Crossfade>, t: i64) -> i64 {
    match x {
        None => -1,
        Some(x) if x.curve == crate::Curve::EqualPower => t + 1,
        Some(_) => 0,
    }
}

/// A gap tick `t` of audio track `ti` inside a fade to silence: keyed as the outgoing clip (segment 3, past its end).
fn black_key(tl: &Timeline, ti: usize, t: i64) -> Option<(usize, u8, i64)> {
    let tr = &tl.tracks[ti];
    let ci = tr.clips.iter().rposition(|c| c.end() <= t)?;
    let x = tl.fade_to_silence(ti, ci).filter(|x| x.at <= t && t < x.at + x.dur)?;
    Some((ci, 3, env_piece(&tr.clips[ci], t, xpiece(Some(x), t)))) // STRIAGAINENVCURVE (SR9083): the tail breaks per tick in a ramp
}

/// A run `t..end` of clip `ci` on track `ti` lying past the clip's end (a fade to silence): its envelope `env` scaled down
/// by the falling weight at both edges. Any other run's `env` comes back as it was.
fn tail_env(tl: &Timeline, ti: usize, ci: usize, t: i64, end: i64, env: [u32; 2]) -> [u32; 2] {
    match tl.fade_to_silence(ti, ci).filter(|x| t >= x.at) {
        Some(x) => {
            let fall = |e: i64| xfade_weight(x.curve, ENV_ONE - ramp_q16(&x, e));
            [env[0].min(fall(t)), env[1].min(fall(end))]
        }
        None => env,
    }
}

// ---- STRIAOPACITY (LEDGER SR868): a non-opaque winner mixes the video track below (named tail block) ----

/// STRIAOPACITY: the video step's mix at tick `t` for the winner, clip `ci` of track `ti`, given `vmix` (its transition's,
/// from [`vmix_for`]). An opaque clip returns `vmix` untouched, before any run state moves (an opaque timeline schedules
/// as it did). A non-opaque one is a Dissolve [`VideoMix`] at `opacity / 100` whose outgoing is the highest video track
/// below with a clip at `t` (its own source tick; black when none): that run continues the mix run when it is the same
/// source (`prev.1`, the mix run's continuation BEFORE [`vmix_for`] reset it), else the live run (`prev.0`, so the first overlay tick hands the lower track's live run to the mix
/// slot through [`crate::vmix::take_outgoing`]). Refused by name ([`ScheduleError::Opacity`]): a transition on the
/// non-opaque clip's own cut, a lower clip that is not opaque or lies in its own transition, and a transition TO black
/// between the two.
#[allow(clippy::too_many_arguments)]
fn opacity_mix(
    tl: &Timeline,
    idx: &dyn SourceIndex,
    ti: usize,
    ci: usize,
    t: i64,
    vmix: Option<VideoMix>,
    prev: (Option<(SourceId, i64)>, Option<(SourceId, i64)>),
    m_next: &mut Option<(SourceId, i64)>,
) -> Result<Option<VideoMix>, ScheduleError> {
    let c = tl.tracks[ti].clips[ci];
    if c.opacity_at(t) == crate::OPAQUE && !c.placed() { // STRIAPOSITIONSCALE (SR879): a placed clip is the two-layer case too // STRIAOPACITYKEYS (SR9048): the percent at this tick (`Clip::opacity_at`, lib.rs tail)
        return Ok(vmix);
    }
    if vmix.is_some() && c.placed() { return Err(ScheduleError::Xform(c.src)); } if vmix.is_some() { // STRIAPOSITIONSCALE (SR879): a placed clip over a transition, named
        return Err(ScheduleError::Opacity(c.src));
    }
    let lower = below(tl, ti, t);
    if black_over(tl, t, lower.map(|l| l.0)).is_some_and(|(i, _)| i < ti) {
        return Err(ScheduleError::Opacity(c.src));
    }
    let w = Frac { num: c.opacity_at(t) as i64, den: 100 }; // STRIAOPACITYKEYS (SR9048): the weight follows the points
    let Some((li, lci)) = lower else {
        *m_next = None;
        return Ok(Some(VideoMix { kind: TransitionKind::Dissolve, src: None, src_t: 0, seek: None, t: w }));
    };
    let lc = tl.tracks[li].clips[lci];
    ifplaced() { return Err(ScheduleError::Xform(c.src)); } if lc.opacity_at(t) tl.tracks[li].transitions.iter().any(|x| x.at == lc.at && t < lc.at + x.dur) {
        return Err(ScheduleError::Opacity(c.src));
    }
    let src_t = lc.src_in + (t - lc.at);
    let prev = match prev.1 {
        Some(m) if m.0 == lc.src => Some(m),
        _ => prev.0,
    };
    let seek = seek_for(tl, idx, Kind::Video, lc.src, src_t, prev)?;
    *m_next = Some((lc.src, src_t + 1));
    Ok(Some(VideoMix { kind: TransitionKind::Dissolve, src: Some(lc.src), src_t, seek, t: w }))
}

/// STRIAOPACITY: the highest video track below track `ti` with a clip at tick `t`: (track index, clip index).
fn below(tl: &Timeline, ti: usize, t: i64) -> Option<(usize, usize)> {
    tl.tracks[..ti].iter().enumerate().rev().filter(|(_, tr)| tr.kind == Kind::Video).find_map(|(i, _)| tl.clip_at(i, t).map(|c| (i, c)))
}

// ---- STRIAPAN (LEDGER SR861): the ONE constant-power pan law over the quarter-wave table (named tail block) ----

/// STRIAPAN: THE pan law: the (left, right) weights (Q16, [`ENV_ONE`] = unity) of a clip at pan `pan_cq8`, constant power
/// over the same quarter-wave sine as [`xfade_weight`]: the phase `p = (pan + 127) * ENV_ONE / 254` (-128 is -127), and
/// `[sin(pi/2 (1 - p)), sin(pi/2 p)]`. Hard left `[ENV_ONE, 0]`, centre `[46341, 46341]` (-3.01 dB each side), hard right
/// `[0, ENV_ONE]`; `L^2 + R^2 = ENV_ONE^2` within the table's interpolation. Stria's `mix_into` divides by the centre (the
/// "-3 dB compensated" form: centre unity, the hard side +3.01 dB on its own channel).
pub fn pan_q16(pan_cq8: i8) -> [u32; 2] {
    let p = ((pan_cq8.max(-127) as i64 + 127) * ENV_ONE as i64 / 254) as u32;
    [qsin(ENV_ONE - p), qsin(p)]
}

// ---- STRIAOPACITYNEXT (LEDGER SR993): the lower run hands back after an opacity overlay (named tail block) ----
// NLE-OPACITY-HANDBACK. While a non-opaque clip overlays the track below, `opacity_mix` keeps the lower clip's run in the
// mix slot (`m_next`). At the first plain tick after the overlay the lower clip wins again at exactly the tick that run
// would continue at: the step's continuing reference is then that run (no seek), and `vmix::take_outgoing` moves it back
// to the live slot for both consumers (Stria's render, the kernel Player). Only when the live run (`v_next`) is of ANOTHER
// source: a stack of one reel on both tracks keeps its seek (the consumers could not tell the two runs apart).

/// STRIAOPACITYNEXT: the reference [`seek_for`] compares a video step against: `m0` (the mix run before this tick) when the
/// step mixes nothing, `m0` continues at `at` exactly and `v_next` is of another source; else `v_next`.
fn hand_back_prev(mixing: bool, m0: Option<(SourceId, i64)>, v_next: Option<(SourceId, i64)>, at: (SourceId, i64)) -> Option<(SourceId, i64)> {
    if !mixing && m0 == Some(at) && v_next.map(|v| v.0) != Some(at.0) {
        return m0;
    }
    v_next
}

// ---- STRIAKPAN (LEDGER SR9015): the per-channel pan factor both faces apply (named tail block) ----

/// STRIAKPAN: THE per-channel factor (Q16, 65536 = unity) a layer's samples are multiplied by, lifted from Stria's
/// `nle::pan_mix` so the kernel Player's sound road applies the same rule: on two channels [`pan_q16`] divided by its
/// centre (the "-3 dB compensated" form: centre exactly unity, so a centred clip mixes byte-identically; hard to one side
/// +3.01 dB on that channel and silence on the other, still constant power). One channel (no sides) or more than two
/// (NLE-PAN-SURROUND): unity, pan does nothing.
pub fn pan_mix_q16(pan_cq8: i8, ch: usize) -> [i64; 2] {
    if ch != 2 {
        return [65536; 2];
    }
    let (w, c) = (pan_q16(pan_cq8), pan_q16(0)[0] as i64);
    [w[0] as i64 * 65536 / c, w[1] as i64 * 65536 / c]
}

// ---- STRIAKSPEED (LEDGER SR9012): the speed a layer plays at, and its effective rate (named tail block) ----
// Lifted from Stria's render (nle.rs, STRIACLIPSPEED SR829) so the kernel Player's EDL feed calls the SAME rule: a layer of a
// clip at `rate_q16` plays at that speed, pitch following it as a tape does (its `n` output samples are asked at the
// EFFECTIVE rate `round(out_rate * 0x1_0000 / q)`); a crossfade's under layer and a handle past the clip's end play 1x.

impl AudioRun {
    /// The speed layer `li` plays at (Q16.16): 1x for an under layer or one no clip of its source covers at the run's first
    /// tick (a handle past the clip's end), else that clip's `rate_q16`.
    pub fn layer_rate(&self, tl: &Timeline, li: usize) -> u32 {
        let Some(l) = self.layers.get(li) else { return 0x1_0000 };
        if self.under(li) {
            return 0x1_0000;
        }
        tl.clip_at(l.track, self.out_t).map(|ci| tl.tracks[l.track].clips[ci]).filter(|c| c.src == l.src).map_or(0x1_0000, |c| c.rate_q16)
    }
}

/// The effective sample rate a layer at speed `q` (Q16.16) is asked at so its samples, played at `rate`, cover `q` times the
/// 1x source span: `round(rate * 0x1_0000 / q)`, at least 1; `q` 0 or 1x is `rate` itself.
pub fn rated_hz(rate: u32, q: u32) -> u32 {
    if q == 0x1_0000 || q == 0 {
        return rate;
    }
    ((rate as u64 * 0x1_0000 + q as u64 / 2) / q as u64).clamp(1, u32::MAX as u64) as u32
}

// ---- STRIACLIPSPEEDTHREE (LEDGER SR9031): the speed a handle plays at (named tail block) ----
// A transition's outgoing handle, a crossfade's UNDER layer and a fade-to-silence tail are the outgoing clip's source read
// through THE `Clip::src_at` past its `end()`, so they play at that clip's rate: the clip of the layer's source on its track
// that ends at or before the run's first tick (the last such one) names it.

/// STRIACLIPSPEEDTHREE: the rate (Q16.16) a handle of source `src` on track `track` plays at in a run starting at tick `t`:
/// that of the last clip of `src` there ending at or before `t`; 1x when none does.
pub fn handle_rate(tl: &Timeline, track: usize, src: SourceId, t: i64) -> u32 {
    tl.tracks.get(track).and_then(|tr| tr.clips.iter().rev().find(|c| c.src == src && c.end() <= t)).map_or(0x1_0000, |c| c.rate_q16)
}

// ---- STRIAPANAUTOMATION (LEDGER SR9050): the run split at an automated clip's pan (named tail block) ----

/// STRIAPANAUTOMATION: per audio track (`tracks`), the pan at tick `t` of the clip there when it carries pan points, else
/// `None`. `audio_runs` breaks a run where this changes (beside its fade key), so between two edges an automated pan is
/// linear and each [`Layer`] carries its true value at both edges; with no points anywhere every entry is `None` and the
/// runs are exactly the unautomated ones. A crossfade's under layer reads its own curve at its edges without a split of
/// its own (NLE-PAN-AUTO-UNDER).
fn pan_piece(tl: &Timeline, tracks: &[usize], t: i64) -> Vec<Option<i8>> {
    tracks
        .iter()
        .map(|&ti| {
            let c = tl.clip_at(ti, t).map(|ci| &tl.tracks[ti].clips[ci])?;
            (c.pan_keys.n > 0).then(|| c.pan_at(t))
        })
        .collect()
}

// ---- STRIAGAINENVELOPE (LEDGER SR9047): a clip's gain envelope through the layer's one gain and edge envelope (named tail block) ----
// No second ramp: a layer keeps ONE gain and the linear envelope at its two edges that every renderer interpolates. A clip
// with gain points is heard at `gain_cdb + peak` (the envelope's largest offset) and each edge of its envelope is scaled
// down by `cdb_q16(at(edge) - peak)` (<= ENV_ONE). Inside a RAMPING segment (two neighbouring points of different value)
// the run key breaks at every tick, as an equal-power span does, so every edge carries the exact dB value; a flat stretch
// stays one run. An empty envelope keys and scales nothing: the runs are byte-identical. The under layer of a crossfade
// and a fade-to-silence tail follow the envelope per tick through the same piece rule and scaling (SR9083).

/// The run key's fade word with clip `c`'s envelope: `t + 1` (one run per tick) inside a ramping segment, else `xp`.
fn env_piece(c: &crate::Clip, t: i64, xp: i64) -> i64 {
    if c.gain_env.ramps(c.src_tick(t)) { t + 1 } else { xp }
}

/// Run `t..end` of clip `c`: its edge envelope `env` scaled by the gain points relative to their peak.
fn genv_scale(c: &crate::Clip, t: i64, end: i64, env: [u32; 2]) -> [u32; 2] {
    if c.gain_env.is_empty() {
        return env;
    }
    let peak = c.gain_env.peak();
    let k = |e: i64, v: u32| (v as u64 * cdb_q16(c.gain_env.at(c.src_tick(e)) - peak) as u64 / ENV_ONE as u64) as u32;
    [k(t, env[0]), k(end, env[1])]
}

/// A gain in centi-dB as a Q16 factor, rounded: `round(10^(cdb / 2000) * 65536)` (0 cdB = [`ENV_ONE`] exactly), through
/// libm (timeline_core's math dependency). audio_core's `ramp::gain_q16` is the same formula (NLE-GAIN-ENV-ONE-DB owed:
/// one dB-to-linear for both crates).
pub fn cdb_q16(cdb: i32) -> u32 {
    if cdb == 0 {
        return ENV_ONE;
    }
    let v = libm::pow(10.0, cdb as f64 / 2000.0) * ENV_ONE as f64;
    libm::round(v).clamp(0.0, u32::MAX as f64) as u32
}

#[cfg(test)]
mod striagainenvelope_tests {
    use super::*;
    use crate::{Clip, GainEnv, Timeline};

    struct Idx;
    impl SourceIndex for Idx {
        fn duration(&self, _: SourceId) -> Option<i64> {
            Some(100)
        }
        fn keyframe_ns(&self, _: SourceId, _: Kind, ns: i64) -> Option<i64> {
            Some(ns)
        }
    }

    fn one(c: Clip) -> Timeline {
        let mut tl = Timeline::new(25);
        let s = tl.add_source("A", 100);
        let a = tl.add_track(Kind::Audio);
        tl.tracks[a].clips.push(Clip { src: s, ..c });
        tl
    }

    /// M3: no points: one run at the flat gain; a ramp -6 dB at source tick 10 to 0 dB at 14 breaks per tick there,
    /// each edge the exact dB value under a gain of the peak (0); a flat -6 dB pair is one run at gain -600, env full.
    #[test]
    fn striagainenvelope_runs() {
        let c = Clip::new(SourceId(0), 0, 20, 0);
        let plain = audio_runs(&one(c), 0..20, &Idx).unwrap();
        assert_eq!(plain.len(), 1);
        assert_eq!((plain[0].layers[0].gain_cdb, plain[0].layers[0].env), (0, [ENV_ONE, ENV_ONE]));
        let ramp = GainEnv::EMPTY.with(10, -600).unwrap().with(14, 0).unwrap();
        let r = audio_runs(&one(Clip { gain_env: ramp, ..c }), 0..20, &Idx).unwrap();
        let spans: Vec<(i64, i64)> = r.iter().map(|x| (x.out_t, x.len)).collect();
        assert_eq!(spans, [(0, 10), (10, 1), (11, 1), (12, 1), (13, 1), (14, 6)]);
        assert!(r.iter().all(|x| x.layers[0].gain_cdb == 0));
        assert_eq!(r[0].layers[0].env, [cdb_q16(-600), cdb_q16(-600)]);
        assert_eq!(r[1].layers[0].env, [cdb_q16(-600), cdb_q16(-450)]);
        assert_eq!(r[4].layers[0].env, [cdb_q16(-150), ENV_ONE]);
        assert_eq!(r[5].layers[0].env, [ENV_ONE, ENV_ONE]);
        assert_eq!(r[1].layers[0].seek, None, "the per-tick runs continue the source");
        let flat = GainEnv::EMPTY.with(2, -600).unwrap().with(8, -600).unwrap();
        let f = audio_runs(&one(Clip { gain_env: flat, ..c }), 0..20, &Idx).unwrap();
        assert_eq!(f.len(), 1);
        assert_eq!((f[0].layers[0].gain_cdb, f[0].layers[0].env), (-600, [ENV_ONE, ENV_ONE]));
        assert_eq!(cdb_q16(0), ENV_ONE);
        assert!((32_700..=32_900).contains(&cdb_q16(-602)), "{}", cdb_q16(-602));
    }
}

// ---- STRIAKOPACITY (LEDGER SR9013): a step's layer opacity (named tail block) ----

/// STRIAKOPACITY: the percent of step `s`'s winning clip when the step mixes the track below by opacity: `s` carries a
/// [`VideoMix`] and its winner (clip at `s.out_t` on `s.track`) is below [`crate::OPAQUE`]. [`opacity_mix`] refuses a
/// transition on a non-opaque cut, so such a mix is always the opacity one. `None` for a transition, a gap, an opaque
/// winner, an audio step. The kernel Player names an opacity tick's mix on its wire by it.
pub fn step_opacity(tl: &Timeline, s: &Step) -> Option<u8> {
    s.vmix?;
    let ti = s.track?;
    let c = tl.tracks.get(ti)?.clips.get(tl.clip_at(ti, s.out_t)?)?;
    (c.opacity < crate::OPAQUE).then_some(c.opacity)
}

// ---- STRIASTACK (LEDGER SR9066): the full visible stack under a non-opaque winner (named tail block) ----
// NLE-OPACITY-STACK. `opacity_mix` mixes the layer right below the winner (`Step::vmix`, at the winner's opacity). When
// that layer is itself below [`crate::OPAQUE`], the layers under it ride [`Step::stack`], BOTTOM TO TOP from index 0: each
// a Dissolve [`VideoMix`] whose `t` is the opacity of the layer sitting on it (vmix's own meaning), index 0 black
// (`src: None`) when the lowest non-opaque layer has nothing under it. Each index keeps its own run ([`seek_for`] against
// that index's previous continuation); a step without index `j` drops run `j` ([`crate::vmix::take_stack`]). A two-layer
// step carries [`NO_STACK`], so every two-layer schedule is byte-identical.

/// STRIASTACK: how many layers a step carries under `vmix`'s source (four layers in all with the winner and `vmix`).
pub const STACK_UNDER: usize = 2;

/// STRIASTACK: the empty stack (every step of a timeline with at most two visible layers).
pub const NO_STACK: [Option<VideoMix>; STACK_UNDER] = [None; STACK_UNDER];

/// STRIASTACK: a layer clip's opacity at tick `t`: the ONE opacity read of [`stack_under`].
fn layer_opacity(c: &crate::Clip, _t: i64) -> u8 {
    c.opacity
}

/// STRIASTACK: the stack under the winner (clip `ci` of track `ti`) at tick `t`, bottom to top; `s_next` holds each
/// index's continuation and is rewritten for the next tick. Refused by name ([`ScheduleError::Opacity`] of the winner's
/// source): a transition TO black between a non-opaque layer and the one under it, a layer clip inside its own transition,
/// and a stack deeper than [`STACK_UNDER`] under `vmix`.
fn stack_under(
    tl: &Timeline,
    idx: &dyn SourceIndex,
    ti: usize,
    ci: usize,
    t: i64,
    s_next: &mut [Option<(SourceId, i64)>; STACK_UNDER],
) -> Result<[Option<VideoMix>; STACK_UNDER], ScheduleError> {
    let top = tl.tracks[ti].clips[ci];
    let mut down = NO_STACK; // top to bottom
    let mut n = 0;
    let mut at = if layer_opacity(&top, t) == crate::OPAQUE { None } else { below(tl, ti, t) };
    while let Some((li, lci)) = at {
        let lc = tl.tracks[li].clips[lci];
        let pct = layer_opacity(&lc, t);
        if pct == crate::OPAQUE {
            break;
        }
        let lower = below(tl, li, t);
        if n == STACK_UNDER || black_over(tl, t, lower.map(|l| l.0)).is_some_and(|(i, _)| i < li) {
            return Err(ScheduleError::Opacity(top.src));
        }
        let w = Frac { num: pct as i64, den: 100 };
        down[n] = Some(match lower {
            None => VideoMix { kind: TransitionKind::Dissolve, src: None, src_t: 0, seek: None, t: w },
            Some((l2, c2)) => {
                let c = tl.tracks[l2].clips[c2];
                if tl.tracks[l2].transitions.iter().any(|x| x.at == c.at && t < c.at + x.dur) {
                    return Err(ScheduleError::Opacity(top.src));
                }
                VideoMix { kind: TransitionKind::Dissolve, src: Some(c.src), src_t: c.src_in + (t - c.at), seek: None, t: w }
            }
        });
        n += 1;
        at = lower;
    }
    let mut stack = NO_STACK;
    for k in 0..n {
        let mut m = down[n - 1 - k].expect("filled above");
        if let Some(src) = m.src {
            m.seek = seek_for(tl, idx, Kind::Video, src, m.src_t, s_next[k])?;
        }
        stack[k] = Some(m);
    }
    for (k, next) in s_next.iter_mut().enumerate() {
        *next = stack[k].and_then(|m| m.src.map(|src| (src, m.src_t + 1)));
    }
    Ok(stack)
}

// ---- STRIASPEEDREVERSE (LEDGER SR9067): the clip a mix layer plays (named tail block) ----
impl AudioRun {
    /// STRIASPEEDREVERSE: the clip layer `li` plays: `None` for an under layer (a crossfade's outgoing handle) or one no clip
    /// of its source covers at the run's first tick (a handle past the clip's end), else that covering clip. The host's
    /// reversed take reads its `reverse` / `rate_q16` here.
    pub fn layer_clip(&self, tl: &Timeline, li: usize) -> Option<crate::Clip> {
        let l = self.layers.get(li)?;
        if self.under(li) {
            return None;
        }
        tl.clip_at(l.track, self.out_t).map(|ci| tl.tracks[l.track].clips[ci]).filter(|c| c.src == l.src)
    }
}

// ---- STRIAGAINENVCURVE (LEDGER SR9083): the crossfade's under layer follows the outgoing's envelope (named tail block) ----
// Inside a crossfade with a clip partner the run key breaks at every tick where the OUTGOING clip's envelope ramps at its
// handle tick (`src_tick(t)`: the outgoing ends at the cut), the same piece rule as [`env_piece`]; `under_layer` hears it at
// `gain_cdb + peak` with each edge through [`genv_scale`]. An empty envelope never breaks a run.

/// The run key's fade word `xp` at tick `t` of clip `ci` on track `ti`: `t + 1` inside a crossfade whose outgoing clip's
/// envelope ramps at that tick, else `xp`.
fn under_piece(tl: &Timeline, ti: usize, ci: usize, t: i64, xp: i64) -> i64 {
    let x = tl.crossfade_at(ti, ci).filter(|x| x.partner == crate::Partner::Clip && t < x.at + x.dur);
    match x {
        Some(_) if tl.tracks[ti].clips[ci - 1].gain_env.ramps(tl.tracks[ti].clips[ci - 1].src_tick(t)) => t + 1,
        _ => xp,
    }
}

#[cfg(test)]
mod striagainenvcurve_tests {
    use super::*;
    use crate::{AudioXfade, Clip, Curve, GainEnv, Timeline};

    struct Idx;
    impl SourceIndex for Idx {
        fn duration(&self, _: SourceId) -> Option<i64> {
            Some(100)
        }
        fn keyframe_ns(&self, _: SourceId, _: Kind, ns: i64) -> Option<i64> {
            Some(ns)
        }
    }

    /// A1: `o` at 0..10 (source 0..10), `i` at 10..20 (source 50..60), a linear explicit crossfade of 4 on the cut.
    fn xf(env: GainEnv) -> Timeline {
        let mut tl = Timeline::new(25);
        let s = tl.add_source("A", 100);
        let a = tl.add_track(Kind::Audio);
        tl.tracks[a].clips = alloc::vec![Clip { gain_env: env, ..Clip::new(s, 0, 10, 0) }, Clip::new(s, 50, 60, 10)];
        tl.audio_xfades.push(AudioXfade { track: a, at: 10, dur: 4, curve: Curve::Linear });
        tl
    }

    /// M3: an equal-power ramp keys per tick with each edge the curve's dB value; the outgoing's ramp on its handle
    /// (source ticks 10..14) breaks the crossfade per tick and the under layer carries the exact value at both edges under
    /// a gain of the peak; with no point the runs are the plain ones.
    #[test]
    fn striagainenvcurve_runs() {
        let mut tl = Timeline::new(25);
        let s = tl.add_source("A", 100);
        let a = tl.add_track(Kind::Audio);
        let q = GainEnv::EMPTY.with(10, -600).unwrap().with(14, 0).unwrap().curved(Curve::EqualPower);
        tl.tracks[a].clips.push(Clip { gain_env: q, ..Clip::new(s, 0, 20, 0) });
        let r = audio_runs(&tl, 0..20, &Idx).unwrap();
        let spans: Vec<(i64, i64)> = r.iter().map(|x| (x.out_t, x.len)).collect();
        assert_eq!(spans, [(0, 10), (10, 1), (11, 1), (12, 1), (13, 1), (14, 6)]);
        assert_eq!(r[2].layers[0].env, [cdb_q16(q.at(11)), cdb_q16(q.at(12))]);
        assert!(q.at(11) > -450, "equal-power rises faster than linear");

        let plain = audio_runs(&xf(GainEnv::EMPTY), 0..20, &Idx).unwrap();
        assert_eq!(plain.iter().map(|x| (x.out_t, x.len)).collect::<Vec<_>>(), [(0, 10), (10, 4), (14, 6)]);
        let ramp = GainEnv::EMPTY.with(10, -600).unwrap().with(14, 0).unwrap();
        let h = audio_runs(&xf(ramp), 0..20, &Idx).unwrap();
        assert_eq!(h.iter().map(|x| (x.out_t, x.len)).collect::<Vec<_>>(), [(0, 10), (10, 1), (11, 1), (12, 1), (13, 1), (14, 6)]);
        let under = h[2].layers[0];
        assert!(h[2].under(0));
        let fall = |e: i64| ENV_ONE - ((e - 10) * ENV_ONE as i64 / 4) as u32;
        let k = |v: u32, cdb: i32| (v as u64 * cdb_q16(cdb) as u64 / ENV_ONE as u64) as u32;
        assert_eq!((under.gain_cdb, under.env), (0, [k(fall(11), -450), k(fall(12), -300)]));
        assert_eq!(h[0].layers[0].env, [cdb_q16(-600), cdb_q16(-600)], "the body before the handle: held");
        let inc = h[2].layers[1];
        assert_eq!((inc.gain_cdb, inc.env), (0, [ENV_ONE / 4, ENV_ONE / 2]), "the incoming: its linear rise, untouched");
    }
}

// ---- STRIAPANAUTOWRITE (LEDGER SR9084): the under layer's run split at a keyed outgoing clip's pan (named tail block) ----

/// STRIAPANAUTOWRITE: per audio track (`tracks`), the pan at tick `t` of the OUTGOING clip under a clip-to-clip crossfade
/// there when that outgoing clip carries pan points, else `None`. `audio_runs` breaks a run where this changes (beside
/// SR9050's [`pan_piece`] for the winner), so the under layer's pan is linear between its two edges; with no keyed outgoing
/// clip every entry is `None` and the runs are exactly as before.
fn under_pan_piece(tl: &Timeline, tracks: &[usize], t: i64) -> Vec<Option<i8>> {
    tracks
        .iter()
        .map(|&ti| {
            let ci = tl.clip_at(ti, t)?;
            tl.crossfade_at(ti, ci).filter(|x| t < x.at + x.dur && x.partner == crate::Partner::Clip)?;
            let o = tl.tracks[ti].clips.get(ci.checked_sub(1)?)?;
            (o.pan_keys.n > 0).then(|| o.pan_at(t))
        })
        .collect()
}

// ---- STRIASTACKTOOL (LEDGER SR9093): the visible layer count of a step and its stack text (named tail block) ----
// NLE-STACK-TOOL / NLE-STACK-FACE. ONE pure reader of [`Step::stack`] for its two view-only consumers: tools/nle `schedule`
// appends [`stack_line`] to a stack step's line, and Stria's face names [`Step::layers`] on its message row. A step with
// [`NO_STACK`] prints and shows what it always did.

impl Step {
    /// STRIASTACKTOOL: the visible layers of this step: the winner, `vmix`'s layer, and each filled [`Step::stack`] entry.
    pub fn layers(&self) -> usize {
        1 + self.vmix.is_some() as usize + self.stack.iter().flatten().count()
    }
}

/// STRIASTACKTOOL: `layers=<n> stack=<e>,..` bottom to top, each `<e>` = `<reel>:<src_t>@<num>/<den>` (`BL:0` for black,
/// the EDL's own word); `None` for a step with [`NO_STACK`].
pub fn stack_line(tl: &Timeline, s: &Step) -> Option<String> {
    if s.stack == NO_STACK {
        return None;
    }
    let es: Vec<String> = s
        .stack
        .iter()
        .flatten()
        .map(|m| match m.src {
            Some(id) => format!("{}:{}@{}/{}", tl.source(id).map_or_else(|| id.0.to_string(), |x| x.reel.clone()), m.src_t, m.t.num, m.t.den),
            None => format!("BL:0@{}/{}", m.t.num, m.t.den),
        })
        .collect();
    Some(format!("layers={} stack={}", s.layers(), es.join(",")))
}

// ---- STRIASTACKKERNEL (LEDGER SR9092): a layer that changes slot as a stack forms or shrinks continues its run (named tail block) ----
// NLE-STACK-HANDOFF. A step's video layers sit in SLOTS: 0 the step's own source, 1 `vmix`, 2.. `stack` bottom to top
// ([`slots`], the ONE order the schedule and `vmix::hand_stack` share). When a stack forms (a third layer arrives) or
// shrinks, a layer moves between slots; the per-slot rules above would reopen it at its keyframe. [`stack_handoff`]
// clears that seek when the layer continues the ONE previous slot that held its source at exactly its tick, and only on
// a step that carries a stack or follows one, so every two-layer schedule is byte-identical. A source named by two
// slots (one reel on two layers) keeps its seeks: the consumers could not tell the two runs apart (SR993's caution).

/// STRIASTACKKERNEL: a video step's slot count: its own source, `vmix`, and [`STACK_UNDER`] stack layers.
pub const SLOTS: usize = 2 + STACK_UNDER;

/// STRIASTACKKERNEL: `(source, source tick, seek)` of each of `s`'s slots (0 own, 1 `vmix`, 2.. `stack` bottom to top);
/// `None` for a slot that names no source (black, unused).
pub fn slots(s: &Step) -> [Option<(SourceId, i64, Option<i64>)>; SLOTS] {
    let mut out = [None; SLOTS];
    out[0] = s.src.map(|src| (src, s.src_t, s.seek));
    for (o, m) in out[1..].iter_mut().zip(core::iter::once(s.vmix).chain(s.stack.iter().copied())) {
        *o = m.and_then(|m| m.src.map(|src| (src, m.src_t, m.seek)));
    }
    out
}

/// STRIASTACKKERNEL: `step` with the seek of each slot that continues the previous tick's ONE slot on its source cleared;
/// `prev` is `(v_next, m_next, s_next)` as they stood before the tick.
fn stack_handoff(mut step: Step, prev: (Option<(SourceId, i64)>, Option<(SourceId, i64)>, [Option<(SourceId, i64)>; STACK_UNDER])) -> Step {
    let (v, m, s) = prev;
    if step.stack == NO_STACK && s.iter().all(|c| c.is_none()) {
        return step;
    }
    let conts: [Option<(SourceId, i64)>; SLOTS] = core::array::from_fn(|i| match i {
        0 => v,
        1 => m,
        k => s[k - 2],
    });
    let named = slots(&step);
    for (i, l) in named.iter().enumerate() {
        let Some((src, src_t, Some(_))) = *l else { continue };
        let once = named.iter().filter(|n| n.is_some_and(|n| n.0 == src)).count() == 1;
        let mut held = conts.iter().filter(|c| c.is_some_and(|c| c.0 == src));
        if once && held.clone().count() == 1 && held.next() == Some(&Some((src, src_t))) {
            clear_seek(&mut step, i);
        }
    }
    step
}

/// STRIASTACKKERNEL: slot `i`'s seek (in [`slots`] order) set to `None`.
fn clear_seek(step: &mut Step, i: usize) {
    match i {
        0 => step.seek = None,
        1 => {
            if let Some(m) = step.vmix.as_mut() {
                m.seek = None;
            }
        }
        k => {
            if let Some(m) = step.stack[k - 2].as_mut() {
                m.seek = None;
            }
        }
    }
}

// ---- STRIAKREVERSE (LEDGER SR9095): the back-cache rule and the reversed layer, both faces (named tail block) ----
// A reversed clip's source ticks DESCEND (THE `Clip::src_at`); a decoder runs forward only. The host's render (nle.rs
// `vid_back` / `take_reversed`) and the kernel Player (vplay.rs `edl_back` / `snd::seat_rev`) serve it by ONE rule:
// VIDEO: a continuing run (same source, no seek) steps by [`back_step`]: on or past the frame it last showed with no cache
// is the forward road; with a cache, the same frame holds and a later one drops the run (the forward road re-opens it);
// behind it, the newest cached frame at or before the target ([`back_pick`]) is served, else the run is refilled from the
// keyframe at or before the target, keeping the last [`BACK_CACHE`] frames ([`back_keep`]). AUDIO: a reversed layer's run
// hears source ticks `reversed_lo ..= src_at(out_t)`; it is read FORWARD from [`AudioRun::reversed_lo`] and its frames
// turned round by [`reverse_frames`].

/// STRIAKREVERSE: how many decoded frames a backward run keeps (both faces).
pub const BACK_CACHE: usize = 16;

/// STRIAKREVERSE: what a continuing video run does for its next target ([`back_step`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackStep {
    /// Not behind the shown frame and no cache: the forward road (every forward run).
    Forward,
    /// Within `tol` of the shown frame, a cache held: the shown frame again.
    Hold,
    /// Past the shown frame with a cache: drop the run, the forward road re-opens it.
    Drop,
    /// Behind the shown frame and the cache reaches the target: serve [`back_pick`].
    Cached,
    /// Behind the shown frame and the cache does not reach it: re-open at the keyframe and decode up to the target.
    Refill,
}

/// STRIAKREVERSE: the step for `target` (ns) when the run last showed `shown` (ns); `front`: the oldest cached frame's ns
/// (`None`: no cache); `tol` the faces' half tick.
pub fn back_step(target: i64, shown: i64, tol: i64, front: Option<i64>) -> BackStep {
    if target >= shown - tol {
        return match front {
            None => BackStep::Forward,
            Some(_) if target <= shown + tol => BackStep::Hold,
            Some(_) => BackStep::Drop,
        };
    }
    if front.is_some_and(|f| f <= target + tol) { BackStep::Cached } else { BackStep::Refill }
}

/// STRIAKREVERSE: keep `f` as the newest cached frame, the oldest dropped past [`BACK_CACHE`].
pub fn back_keep<T>(q: &mut VecDeque<T>, f: T) {
    q.push_back(f);
    while q.len() > BACK_CACHE {
        q.pop_front();
    }
}

/// STRIAKREVERSE: the newest cached frame at or before `target + tol` (`ns` reads a frame's time).
pub fn back_pick<T>(q: &VecDeque<T>, target: i64, tol: i64, ns: impl Fn(&T) -> i64) -> Option<&T> {
    q.iter().rev().find(|f| ns(f) <= target + tol)
}

/// STRIAKREVERSE: interleaved frames of `ch` channels turned round in place (the channel order inside a frame kept).
pub fn reverse_frames<T>(pcm: &mut [T], ch: usize) {
    let ch = ch.max(1);
    let n = pcm.len() / ch * ch;
    pcm[..n].reverse();
    for f in pcm[..n].chunks_mut(ch) {
        f.reverse();
    }
}

impl AudioRun {
    /// STRIAKREVERSE: the lowest source tick layer `li` hears when it plays a reversed clip ([`AudioRun::layer_clip`]):
    /// `src_at` of the run's last tick; `None` for a forward layer, an under layer or a handle.
    pub fn reversed_lo(&self, tl: &Timeline, li: usize) -> Option<i64> {
        let c = self.layer_clip(tl, li).filter(|c| c.reverse)?;
        Some(c.src_at(self.out_t + self.len - 1))
    }
}

#[cfg(test)]
mod striakreverse_tests {
    extern crate std;
    use super::*;
    use crate::Clip;

    #[test]
    fn striakreverse_back_step_names_each_arm() {
        assert_eq!(back_step(40, 40, 20, None), BackStep::Forward);
        assert_eq!(back_step(80, 40, 20, None), BackStep::Forward);
        assert_eq!(back_step(40, 40, 20, Some(0)), BackStep::Hold);
        assert_eq!(back_step(80, 40, 20, Some(0)), BackStep::Drop);
        assert_eq!(back_step(0, 40, 10, Some(0)), BackStep::Cached);
        assert_eq!(back_step(0, 40, 10, Some(20)), BackStep::Refill);
        assert_eq!(back_step(0, 40, 10, None), BackStep::Refill);
    }

    #[test]
    fn striakreverse_back_keep_caps_and_back_pick_takes_the_newest_at_or_before() {
        let mut q = VecDeque::new();
        for i in 0..(BACK_CACHE as i64 + 4) {
            back_keep(&mut q, i * 10);
        }
        assert_eq!(q.len(), BACK_CACHE);
        assert_eq!(q.front().copied(), Some(40), "the oldest four dropped");
        assert_eq!(back_pick(&q, 95, 4, |f| *f).copied(), Some(90));
        assert_eq!(back_pick(&q, 96, 4, |f| *f).copied(), Some(100));
        assert_eq!(back_pick(&q, 30, 4, |f| *f), None);
    }

    #[test]
    fn striakreverse_reverse_frames_keeps_the_channel_order() {
        let mut st = [1, 2, 3, 4, 5, 6];
        reverse_frames(&mut st, 2);
        assert_eq!(st, [5, 6, 3, 4, 1, 2]);
        let mut mono = [1.0f32, 2.0, 3.0];
        reverse_frames(&mut mono, 1);
        assert_eq!(mono, [3.0, 2.0, 1.0]);
    }

    #[test]
    fn striakreverse_reversed_lo_is_the_runs_lowest_source_tick() {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 50);
        tl.add_track(Kind::Audio);
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 10, 15, 0).reversed(true)];
        let layer = Layer { track: 0, src: a, src_t: 14, seek: None, gain_cdb: 0, env: [ENV_ONE, ENV_ONE] };
        let whole = AudioRun { out_t: 0, len: 5, layers: alloc::vec![layer] };
        assert_eq!(whole.reversed_lo(&tl, 0), Some(10));
        let head = AudioRun { out_t: 0, len: 2, layers: alloc::vec![layer] };
        assert_eq!(head.reversed_lo(&tl, 0), Some(13));
        tl.tracks[0].clips = alloc::vec![Clip::new(a, 10, 15, 0)];
        assert_eq!(whole.reversed_lo(&tl, 0), None, "a forward clip");
    }
}
