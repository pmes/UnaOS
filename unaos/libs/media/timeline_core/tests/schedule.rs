// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! NLE1 (SR194) M2: the keyframe-aware render schedule over a fixed-GOP index.

use timeline_core::schedule::{ScheduleError, Step, schedule};
use timeline_core::{Clip, EditError, Kind, Mode, SourceId, SourceIndex, Timeline};

const NS: i64 = 40_000_000; // one tick at 25 fps

/// Every source: `dur` ticks, a video keyframe every 10 frames, every audio packet a sync point (per tick).
struct Gop {
    dur: i64,
}
impl SourceIndex for Gop {
    fn duration(&self, _: SourceId) -> Option<i64> {
        Some(self.dur)
    }
    fn keyframe_ns(&self, _: SourceId, kind: Kind, t_ns: i64) -> Option<i64> {
        let f = t_ns / NS;
        Some(match kind {
            Kind::Video => f / 10 * 10 * NS,
            Kind::Audio => f * NS,
        })
    }
}

fn v(steps: &[Step]) -> Vec<(i64, Option<u32>, i64, Option<i64>)> {
    steps.iter().filter(|s| s.kind == Kind::Video).map(|s| (s.out_t, s.src.map(|x| x.0), s.src_t, s.seek)).collect()
}

/// V1: A[13,23)@0, B[0,10)@10, gap 20..25, A[23,33)@25; A1 the same clips.
fn three_clips() -> Timeline {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Video);
    tl.add_track(Kind::Audio);
    for t in 0..2 {
        tl.tracks[t].clips = vec![Clip::new(a, 13, 23, 0), Clip::new(b, 0, 10, 10), Clip::new(a, 23, 33, 25)];
    }
    tl
}

#[test]
fn three_clip_cut() {
    let tl = three_clips();
    let s = schedule(&tl, 0..35, &Gop { dur: 50 }).unwrap();
    let vs = v(&s);
    assert_eq!(vs.len(), 35);
    assert_eq!(vs[0], (0, Some(0), 13, Some(10 * NS))); // mid-GOP in point: seek to frame 10, run up 3
    assert_eq!(vs[1], (1, Some(0), 14, None));
    assert_eq!(vs[9], (9, Some(0), 22, None));
    assert_eq!(vs[10], (10, Some(1), 0, Some(0))); // the cut to B
    assert_eq!(vs[19], (19, Some(1), 9, None));
    for g in 20..25 {
        assert_eq!(vs[g], (g as i64, None, 0, None)); // the gap: black
    }
    // A again: it continues A's source time (23) but not the previous step, so it seeks (to keyframe 20).
    assert_eq!(vs[25], (25, Some(0), 23, Some(20 * NS)));
    assert_eq!(vs[34], (34, Some(0), 32, None));
    // Audio: one step per run, after the video step of its first tick.
    let a: Vec<Step> = s.iter().copied().filter(|x| x.kind == Kind::Audio).collect();
    assert_eq!(a.len(), 4);
    assert_eq!((a[0].out_t, a[0].src, a[0].src_t, a[0].len, a[0].seek), (0, Some(SourceId(0)), 13, 10, Some(13 * NS)));
    assert_eq!((a[1].out_t, a[1].src, a[1].len, a[1].seek), (10, Some(SourceId(1)), 10, Some(0)));
    assert_eq!((a[2].out_t, a[2].src, a[2].len), (20, None, 5));
    assert_eq!((a[3].out_t, a[3].src_t, a[3].len, a[3].seek), (25, 23, 10, Some(23 * NS)));
    // order: tick 0's video step, then its audio step
    assert_eq!((s[0].kind, s[1].kind, s[2].kind), (Kind::Video, Kind::Audio, Kind::Video));
}

#[test]
fn a_split_needs_no_seek() {
    let tl = three_clips().split(0, 15).unwrap().split(1, 15).unwrap();
    let s = schedule(&tl, 0..35, &Gop { dur: 50 }).unwrap();
    let vs = v(&s);
    assert_eq!(vs[15], (15, Some(1), 5, None)); // B continues across its split
    assert_eq!(s.iter().filter(|x| x.seek.is_some()).count(), 6);
    // the audio run breaks at the split (two clips) but the second half continues: no seek
    let a: Vec<Step> = s.iter().copied().filter(|x| x.kind == Kind::Audio).collect();
    assert_eq!((a[2].out_t, a[2].src_t, a[2].len, a[2].seek), (15, 5, 5, None));
}

#[test]
fn ripple_delete_reschedules() {
    let tl = three_clips().ripple_delete(0, 1).unwrap();
    let vs = v(&schedule(&tl, 0..25, &Gop { dur: 50 }).unwrap());
    assert_eq!(vs[9], (9, Some(0), 22, None));
    assert_eq!(vs[10], (10, None, 0, None)); // the gap moved up by B's 10
    assert_eq!(vs[15], (15, Some(0), 23, Some(20 * NS)));
}

#[test]
fn back_to_back_in_source_order_continues() {
    let tl = three_clips().lift(0, 1).unwrap().move_clip(0, 1, 0, 10).unwrap();
    let vs = v(&schedule(&tl, 0..20, &Gop { dur: 50 }).unwrap());
    assert_eq!(vs[10], (10, Some(0), 23, None)); // A[13,23) then A[23,33): one decode run
}

#[test]
fn upper_track_wins() {
    let mut tl = three_clips();
    let v2 = tl.add_track(Kind::Video);
    let b = SourceId(1);
    let tl = tl.place(v2, Clip::new(b, 40, 43, 5), Mode::Overwrite).unwrap();
    let vs = v(&schedule(&tl, 0..10, &Gop { dur: 50 }).unwrap());
    assert_eq!(vs[4], (4, Some(0), 17, None));
    assert_eq!(vs[5], (5, Some(1), 40, Some(40 * NS)));
    assert_eq!(vs[7], (7, Some(1), 42, None));
    assert_eq!(vs[8], (8, Some(0), 21, Some(20 * NS))); // back to V1: A at 21 does not continue B
    assert_eq!(s_track(&tl, 5), Some(v2));
}

fn s_track(tl: &Timeline, t: i64) -> Option<usize> {
    schedule(tl, t..t + 1, &Gop { dur: 50 }).unwrap()[0].track
}

#[test]
fn overlap_and_bad_ranges_are_refused() {
    let mut tl = three_clips();
    tl.tracks[0].clips.push(Clip::new(SourceId(1), 0, 5, 30));
    assert_eq!(schedule(&tl, 0..10, &Gop { dur: 50 }), Err(ScheduleError::Invalid(EditError::Overlap)));
    let tl = three_clips();
    assert_eq!(three_clips().place(0, Clip::new(SourceId(1), 0, 5, 30), Mode::Insert).map(|_| ()), Ok(()));
    assert_eq!(three_clips().move_clip(0, 0, 0, 30), Err(EditError::Overlap));
    assert_eq!(schedule(&tl, 5..2, &Gop { dur: 50 }), Err(ScheduleError::Range));
    // the index knows the media is shorter than the timeline claims
    assert_eq!(schedule(&tl, 0..10, &Gop { dur: 30 }), Err(ScheduleError::OutOfSource(SourceId(0))));
}
