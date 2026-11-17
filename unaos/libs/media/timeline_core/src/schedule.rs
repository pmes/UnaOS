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
    /// Its source tick (in its handle: `src_out + k`).
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
}

impl core::fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            ScheduleError::Range => f.write_str("bad range"),
            ScheduleError::Invalid(e) => write!(f, "invalid timeline: {e}"),
            ScheduleError::OutOfSource(s) => write!(f, "clip past the end of source {}", s.0),
            ScheduleError::NoKeyframe(s) => write!(f, "source {} has no stream to seek", s.0),
        }
    }
}

/// The winner at tick `t` for `kind`: (track index, clip index).
fn top(tl: &Timeline, kind: Kind, t: i64) -> Option<(usize, usize)> {
    tl.tracks.iter().enumerate().rev().filter(|(_, tr)| tr.kind == kind).find_map(|(i, _)| tl.clip_at(i, t).map(|c| (i, c)))
}

/// The schedule for output ticks `range` (see the module docs).
pub fn schedule(tl: &Timeline, range: Range<i64>, idx: &dyn SourceIndex) -> Result<Vec<Step>, ScheduleError> {
    check(tl, &range, idx)?;
    handles_fit(tl, idx)?;
    let has = |k: Kind| tl.tracks.iter().any(|t| t.kind == k);
    let (video, audio) = (has(Kind::Video), has(Kind::Audio));
    let mut out = Vec::new();
    let mut m_next: Option<(SourceId, i64)> = None; // NLE3: the transition's outgoing run
    // The previous video step's (source, the source tick it would continue at).
    let mut v_next: Option<(SourceId, i64)> = None;
    // NLE4: the audio steps are the mix runs' headers, one per run.
    let runs = if audio { audio_runs(tl, range.clone(), idx)? } else { Vec::new() };
    let mut run = runs.iter().peekable();
    for t in range.clone() {
        if video {
            let win = top(tl, Kind::Video, t);
            let step = match (black_over(tl, t, win.map(|w| w.0)), win) {
                (Some((ti, x)), _) => black_step(tl, idx, ti, &x, t, &mut v_next, &mut m_next)?,
                (None, Some((ti, ci))) => {
                    let c = tl.tracks[ti].clips[ci];
                    let src_t = c.src_at(t); // STRIACLIPSPEED (SR829): the clip's rate (1x: src_in + (t - at))
                    let vmix = vmix_for(tl, idx, ti, ci, t, v_next, &mut m_next)?;
                    let seek = seek_for(tl, idx, Kind::Video, c.src, src_t, v_next)?;
                    v_next = Some((c.src, c.src_at(t + 1))); // STRIACLIPSPEED (SR829): a rated clip continues its run (1x: src_t + 1)
                    Step { kind: Kind::Video, track: Some(ti), src: Some(c.src), src_t, out_t: t, len: 1, seek, vmix }
                }
                (None, None) => {
                    (v_next, m_next) = (None, None);
                    Step { kind: Kind::Video, track: None, src: None, src_t: 0, out_t: t, len: 1, seek: None, vmix: None }
                }
            };
            out.push(step);
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
    /// The clip's gain, centi-dB.
    pub gain_cdb: i32,
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
        Step { kind: Kind::Audio, track, src, src_t, out_t: self.out_t, len: self.len, seek, vmix: None }
    }
}

/// NLE4: the audio mix runs for output ticks `range` (see the module docs).
pub fn audio_runs(tl: &Timeline, range: Range<i64>, idx: &dyn SourceIndex) -> Result<Vec<AudioRun>, ScheduleError> {
    check(tl, &range, idx)?;
    let tracks: Vec<usize> = (0..tl.tracks.len()).filter(|&i| tl.tracks[i].kind == Kind::Audio).collect();
    if tracks.is_empty() {
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
                    (ci, seg, xpiece(xfade(ti, ci, t), t))
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
        let here = key(t);
        let mut end = t + 1;
        while end < range.end && key(end) == here {
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
                    layers.push(Layer { track: ti, src: c.src, src_t, seek, gain_cdb: c.gain_cdb, env });
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
fn env_q16(c: &crate::Clip, t: i64) -> u32 {
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
    let src_t = o.src_out + k;
    let seek = seek_for(tl, idx, Kind::Video, o.src, src_t, if k == 0 { v_next } else { *m_next })?;
    *m_next = Some((o.src, src_t + 1));
    Ok(Some(VideoMix { kind: x.kind, src: Some(o.src), src_t, seek, t: w }))
}

/// NLE3: every transition's outgoing handle lies inside what the index says the source holds.
fn handles_fit(tl: &Timeline, idx: &dyn SourceIndex) -> Result<(), ScheduleError> {
    for tr in &tl.tracks {
        for x in tr.transitions.iter().filter(|x| !x.from_black) {
            if let Some(o) = tr.clips.iter().find(|c| c.end() == x.at) {
                if idx.duration(o.src).is_some_and(|d| o.src_out + x.dur > d) {
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
    let src_t = o.src_out + (t - x.at);
    let seek = seek_for(tl, idx, Kind::Audio, o.src, src_t, *xnext)?;
    *xnext = Some((o.src, src_t + (end - t)));
    let tail = env_q16(o, o.end()) as i64;
    let fall = |e: i64| (xfade_weight(x.curve, ENV_ONE - ramp_q16(x, e)) as i64 * tail / ENV_ONE as i64) as u32;
    Ok(Layer { track: ti, src: o.src, src_t, seek, gain_cdb: o.gain_cdb, env: [fall(t), fall(end)] })
}

/// NLE3cBLACK (LEDGER SR323): the highest video track above the clip winner's (`above`; any when `None`) whose
/// transition TO black spans tick `t`, with that transition.
fn black_over(tl: &Timeline, t: i64, above: Option<usize>) -> Option<(usize, crate::Transition)> {
    tl.tracks
        .iter()
        .enumerate()
        .rev()
        .filter(|(i, tr)| tr.kind == Kind::Video && above.is_none_or(|a| *i > a))
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
    let src_t = o.src_out + k;
    let seek = seek_for(tl, idx, Kind::Video, o.src, src_t, if k == 0 { *v_next } else { *m_next })?;
    (*v_next, *m_next) = (None, Some((o.src, src_t + 1)));
    let vmix = VideoMix { kind: x.kind, src: Some(o.src), src_t, seek, t: x.ease.apply(Frac { num: k + 1, den: x.dur + 1 }) }; // NLEWIPES (SR679): the ease
    Ok(Step { kind: Kind::Video, track: Some(ti), src: None, src_t: 0, out_t: t, len: 1, seek: None, vmix: Some(vmix) })
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
    Some((ci, 3, xpiece(Some(x), t)))
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
