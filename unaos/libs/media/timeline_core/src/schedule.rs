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
//! seeks at the cut as a plain cut does.

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
    /// The OUTGOING clip's source.
    pub src: SourceId,
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
            let step = match top(tl, Kind::Video, t) {
                Some((ti, ci)) => {
                    let c = tl.tracks[ti].clips[ci];
                    let src_t = c.src_in + (t - c.at);
                    let vmix = vmix_for(tl, idx, ti, ci, t, v_next, &mut m_next)?;
                    let seek = seek_for(tl, idx, Kind::Video, c.src, src_t, v_next)?;
                    v_next = Some((c.src, src_t + 1));
                    Step { kind: Kind::Video, track: Some(ti), src: Some(c.src), src_t, out_t: t, len: 1, seek, vmix }
                }
                None => {
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
    // What each audio track shows at `t`: (clip index, envelope segment 0 fade-in / 1 body / 2 fade-out).
    let key = |t: i64| -> Vec<Option<(usize, u8)>> {
        tracks
            .iter()
            .map(|&ti| {
                tl.clip_at(ti, t).map(|ci| {
                    let c = &tl.tracks[ti].clips[ci];
                    let seg = if t < c.at + c.fade_in { 0 } else if t >= c.end() - c.fade_out { 2 } else { 1 };
                    (ci, seg)
                })
            })
            .collect()
    };
    // Per track: (source, the source tick its previous layer would continue at).
    let mut next: Vec<Option<(SourceId, i64)>> = alloc::vec![None; tl.tracks.len()];
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
                Some((ci, _)) => {
                    let c = tl.tracks[ti].clips[ci];
                    let src_t = c.src_in + (t - c.at);
                    let seek = seek_for(tl, idx, Kind::Audio, c.src, src_t, next[ti])?;
                    next[ti] = Some((c.src, src_t + (end - t)));
                    layers.push(Layer { track: ti, src: c.src, src_t, seek, gain_cdb: c.gain_cdb, env: [env_q16(&c, t), env_q16(&c, end)] });
                }
                None => next[ti] = None,
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
    idx.keyframe_ns(src, kind, tl.tb.to_ns(src_t)).map(Some).ok_or(ScheduleError::NoKeyframe(src))
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
    // validate: the outgoing clip ends at the cut, so it is the clip before (clips are ordered and never overlap).
    let o = tr.clips[ci - 1];
    let k = t - c.at;
    let src_t = o.src_out + k;
    let seek = seek_for(tl, idx, Kind::Video, o.src, src_t, if k == 0 { v_next } else { *m_next })?;
    *m_next = Some((o.src, src_t + 1));
    Ok(Some(VideoMix { kind: x.kind, src: o.src, src_t, seek, t: Frac { num: k + 1, den: x.dur + 1 } }))
}

/// NLE3: every transition's outgoing handle lies inside what the index says the source holds.
fn handles_fit(tl: &Timeline, idx: &dyn SourceIndex) -> Result<(), ScheduleError> {
    for tr in &tl.tracks {
        for x in &tr.transitions {
            if let Some(o) = tr.clips.iter().find(|c| c.end() == x.at) {
                if idx.duration(o.src).is_some_and(|d| o.src_out + x.dur > d) {
                    return Err(ScheduleError::OutOfSource(o.src));
                }
            }
        }
    }
    Ok(())
}
