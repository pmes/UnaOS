// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The render schedule (NLE1 M2, LEDGER SR194): what each output tick shows, in output order, and where a decoder
//! must seek.
//!
//! Cut-only: at tick `t` the HIGHEST-numbered track of a kind with a clip at `t` wins (V2 over V1, A2 over A1);
//! dissolves are NLE3 and the audio mix NLE4. For each output tick the schedule holds one video [`Step`] (`len` 1),
//! then the audio step whose run starts at that tick, if any (an audio step covers one contiguous clip run or one gap
//! run). A kind with no track gets no steps. A step with `src: None` is a gap: black, or silence.
//!
//! Keyframe-aware: the first step of a run whose source position does not continue the previous step of its kind (a
//! cut to another source, or a jump inside the same one) carries `seek`: the keyframe at or before its source time,
//! from [`SourceIndex::keyframe_ns`]. The renderer seeks there and decodes up to `src_t` unseen (the run-up). A step
//! that continues the previous one (a split, or two clips butted end to start in source order) carries none.

use alloc::vec::Vec;
use core::ops::Range;

use crate::{EditError, Kind, SourceId, SourceIndex, Timeline};

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
    if range.start < 0 || range.end < range.start {
        return Err(ScheduleError::Range);
    }
    tl.validate().map_err(ScheduleError::Invalid)?;
    for c in tl.tracks.iter().flat_map(|t| t.clips.iter()) {
        if idx.duration(c.src).is_some_and(|d| c.src_out > d) {
            return Err(ScheduleError::OutOfSource(c.src));
        }
    }
    let has = |k: Kind| tl.tracks.iter().any(|t| t.kind == k);
    let (video, audio) = (has(Kind::Video), has(Kind::Audio));
    let mut out = Vec::new();
    // The previous step of each kind's (source, the source tick it would continue at).
    let mut v_next: Option<(SourceId, i64)> = None;
    let mut a_next: Option<(SourceId, i64)> = None;
    let mut a_run_end = range.start;
    for t in range.clone() {
        if video {
            let step = match top(tl, Kind::Video, t) {
                Some((ti, ci)) => {
                    let c = tl.tracks[ti].clips[ci];
                    let src_t = c.src_in + (t - c.at);
                    let seek = seek_for(tl, idx, Kind::Video, c.src, src_t, v_next)?;
                    v_next = Some((c.src, src_t + 1));
                    Step { kind: Kind::Video, track: Some(ti), src: Some(c.src), src_t, out_t: t, len: 1, seek }
                }
                None => {
                    v_next = None;
                    Step { kind: Kind::Video, track: None, src: None, src_t: 0, out_t: t, len: 1, seek: None }
                }
            };
            out.push(step);
        }
        if audio && t >= a_run_end {
            let here = top(tl, Kind::Audio, t);
            // The run lasts while the same clip (or the gap) wins.
            let mut end = t + 1;
            while end < range.end && top(tl, Kind::Audio, end) == here {
                end += 1;
            }
            a_run_end = end;
            let step = match here {
                Some((ti, ci)) => {
                    let c = tl.tracks[ti].clips[ci];
                    let src_t = c.src_in + (t - c.at);
                    let seek = seek_for(tl, idx, Kind::Audio, c.src, src_t, a_next)?;
                    a_next = Some((c.src, src_t + (end - t)));
                    Step { kind: Kind::Audio, track: Some(ti), src: Some(c.src), src_t, out_t: t, len: end - t, seek }
                }
                None => {
                    a_next = None;
                    Step { kind: Kind::Audio, track: None, src: None, src_t: 0, out_t: t, len: end - t, seek: None }
                }
            };
            out.push(step);
        }
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
