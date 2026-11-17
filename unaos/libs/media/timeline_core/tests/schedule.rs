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

// ---- NLEXFADEPOWER (SR678): the ONE crossfade weight against the curve it tabulates.

#[test]
fn nlexfadepower_xfade_weight_tracks_sine_and_cosine() {
    use timeline_core::Curve;
    use timeline_core::schedule::{ENV_ONE, xfade_weight};
    let mut prev = 0;
    for p in (0..=ENV_ONE).step_by(97).chain([ENV_ONE]) {
        let w = xfade_weight(Curve::EqualPower, p);
        let want = (core::f64::consts::FRAC_PI_2 * p as f64 / ENV_ONE as f64).sin() * ENV_ONE as f64;
        assert!((w as f64 - want).abs() <= 2.0, "p {p}: {w} vs {want}");
        assert!(w >= prev, "p {p}: not rising");
        prev = w;
    }
    // clamped past the end; linear the identity
    assert_eq!((xfade_weight(Curve::EqualPower, ENV_ONE + 9), xfade_weight(Curve::Linear, ENV_ONE + 9)), (ENV_ONE, ENV_ONE));
}

// ---- STRIAOPACITY (LEDGER SR868): a non-opaque winner mixes the video track below (named tail block) ----

/// V1 A[0,30)@0; V2 B[0,10)@10 at `pct` percent (sources 50 ticks, a keyframe every 10).
fn striaopacity_stack(pct: u8) -> Timeline {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Video);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 30, 0)];
    tl.tracks[1].clips = vec![Clip::new(b, 0, 10, 10).with_opacity(pct)];
    tl
}

/// An opaque clip over a lower one schedules byte-identically to the same timeline with the lower track's clips under
/// it gone: the lower clip is NOT scheduled at all (no hidden decode), and no step carries a mix.
#[test]
fn striaopacity_opaque_top_schedules_as_today() {
    let tl = striaopacity_stack(timeline_core::OPAQUE);
    assert_eq!(Clip::new(SourceId(0), 0, 1, 0).opacity, timeline_core::OPAQUE);
    let steps = schedule(&tl, 0..30, &Gop { dur: 50 }).unwrap();
    let mut alone = tl.clone();
    alone.tracks[0].clips = vec![Clip::new(SourceId(0), 0, 10, 0), Clip::new(SourceId(0), 20, 30, 20)];
    let ref_steps = schedule(&alone, 0..30, &Gop { dur: 50 }).unwrap();
    assert_eq!(steps[10..20], ref_steps[10..20], "the overlay span is V2's own steps, step for step");
    assert!(steps.iter().all(|s| s.vmix.is_none()));
    assert!(steps[10..20].iter().all(|s| s.src == Some(SourceId(1)) && s.track == Some(1)));
}

/// 50%: each overlay tick is V2's own step with a Dissolve mix of V1 at its own tick, weight 50/100; the lower run
/// continues V1's live run at the first tick (no seek: the mix slot takes it) and itself after; V1 reseeks after.
#[test]
fn striaopacity_half_top_mixes_the_track_below() {
    use timeline_core::TransitionKind;
    use timeline_core::schedule::Frac;
    let tl = striaopacity_stack(50);
    let steps = schedule(&tl, 0..30, &Gop { dur: 50 }).unwrap();
    for (t, s) in steps.iter().enumerate().take(20).skip(10) {
        let m = s.vmix.expect("an overlay tick mixes");
        assert_eq!((s.src, s.src_t, s.track), (Some(SourceId(1)), t as i64 - 10, Some(1)));
        assert_eq!((m.kind, m.src, m.src_t, m.t), (TransitionKind::Dissolve, Some(SourceId(0)), t as i64, Frac { num: 50, den: 100 }));
        assert_eq!(m.seek, None, "tick {t}: the lower run continues");
        assert_eq!(s.seek.is_some(), t == 10, "tick {t}: the top opens once");
    }
    assert!(steps[..10].iter().chain(&steps[20..]).all(|s| s.vmix.is_none()));
    assert_eq!(steps[20].seek, None, "V1 continues its run after the overlay"); // STRIAOPACITYNEXT (SR993): NLE-OPACITY-HANDBACK built
    assert_eq!(steps[21].seek, None);
}

/// 0% schedules the lower clip with fraction 0; nothing below mixes over black.
#[test]
fn striaopacity_zero_and_over_black() {
    use timeline_core::schedule::Frac;
    let steps = schedule(&striaopacity_stack(0), 10..12, &Gop { dur: 50 }).unwrap();
    let m = steps[0].vmix.unwrap();
    assert_eq!((m.src, m.src_t, m.t), (Some(SourceId(0)), 10, Frac { num: 0, den: 100 }));
    assert_eq!(m.seek, Some(10 * NS), "a first tick with no run before it seeks the lower source");
    let mut tl = striaopacity_stack(25);
    tl.tracks[0].clips.clear();
    let steps = schedule(&tl, 10..11, &Gop { dur: 50 }).unwrap();
    let m = steps[0].vmix.unwrap();
    assert_eq!((m.src, m.seek, m.t), (None, None, Frac { num: 25, den: 100 }));
}

/// Two layers only: each stack the shape cannot serve is refused by name.
#[test]
fn striaopacity_refusals_are_named() {
    use timeline_core::TransitionKind;
    let g = Gop { dur: 50 };
    let refused = Err(ScheduleError::Opacity(SourceId(1)));
    // a non-opaque clip over a second non-opaque one
    let mut tl = striaopacity_stack(50);
    tl.tracks[0].clips[0].opacity = 40;
    assert_eq!(schedule(&tl, 0..30, &g), refused);
    // a transition on the non-opaque clip's own cut: V2 B[0,10)@10 then B[10,20)@20 at 50%, a dissolve at 20
    let mut tl = striaopacity_stack(timeline_core::OPAQUE);
    tl.tracks[1].clips.push(Clip::new(SourceId(1), 20, 25, 20).with_opacity(50));
    let tl = tl.add_transition(1, 20, TransitionKind::Dissolve, 3).unwrap();
    assert_eq!(schedule(&tl, 0..30, &g), refused);
    // the lower clip inside its own transition: V1 A[0,15)@0, A[20,35)@15 with a dissolve at 15
    let mut tl = striaopacity_stack(50);
    tl.tracks[0].clips = vec![Clip::new(SourceId(0), 0, 15, 0), Clip::new(SourceId(0), 20, 35, 15)];
    let tl = tl.add_transition(0, 15, TransitionKind::Dissolve, 3).unwrap();
    assert_eq!(schedule(&tl, 0..30, &g), refused);
    assert_eq!(ScheduleError::Opacity(SourceId(1)).to_string(), "source 1 is not opaque over a transition or a second non-opaque layer (two layers only)");
}

/// validate: above 100 percent, or a non-opaque audio clip, is `EditError::Opacity`.
#[test]
fn striaopacity_validate_refuses_by_name() {
    let mut tl = striaopacity_stack(101);
    assert_eq!(tl.validate(), Err(EditError::Opacity));
    tl.tracks[1].clips[0].opacity = 100;
    let a = tl.add_track(Kind::Audio);
    tl.tracks[a].clips = vec![Clip::new(SourceId(0), 0, 5, 0).with_opacity(50)];
    assert_eq!(tl.validate(), Err(EditError::Opacity));
    assert_eq!(tl.set_opacity(&[(a, 0)], 100).map(|t| t.tracks[a].clips[0].opacity), Ok(100));
    assert_eq!(EditError::Opacity.to_string(), "opacity is a percent 0..=100, and video only");
}

// ---- STRIAPOSITIONSCALE (LEDGER SR879): a placed (picture-in-picture) winner schedules the video track below (named tail block) ----

/// An opaque placed V2 clip rides STRIAOPACITY's two-layer step: each overlay tick is V2's own step with a Dissolve mix of
/// V1 at its own tick at weight 100/100 (the renderer places the top over it); unplaced, the same stack schedules as today.
#[test]
fn striapositionscale_placed_top_schedules_the_track_below() {
    use timeline_core::schedule::Frac;
    use timeline_core::{TransitionKind, Xform};
    let g = Gop { dur: 50 };
    let plain = striaopacity_stack(timeline_core::OPAQUE);
    let tl = plain.set_xform(1, 0, Xform::PIP).unwrap();
    let steps = schedule(&tl, 0..30, &g).unwrap();
    for (t, s) in steps.iter().enumerate().take(20).skip(10) {
        let m = s.vmix.expect("a placed tick carries the track below");
        assert_eq!((s.src, s.src_t, s.track), (Some(SourceId(1)), t as i64 - 10, Some(1)));
        assert_eq!((m.kind, m.src, m.src_t, m.t), (TransitionKind::Dissolve, Some(SourceId(0)), t as i64, Frac { num: 100, den: 100 }));
        assert_eq!(m.seek, None, "tick {t}: the lower run continues");
    }
    assert!(steps[..10].iter().chain(&steps[20..]).all(|s| s.vmix.is_none()));
    assert!(schedule(&plain, 0..30, &g).unwrap().iter().all(|s| s.vmix.is_none()), "unplaced: no mix, as today");
    let mut alone = tl.clone();
    alone.tracks[0].clips.clear();
    let m = schedule(&alone, 10..11, &g).unwrap()[0].vmix.unwrap();
    assert_eq!((m.src, m.seek, m.t), (None, None, Frac { num: 100, den: 100 }), "nothing below: over black");
}

/// Two layers only: a placed clip over a transition, or a placed clip under another layer, is refused as `Xform`.
#[test]
fn striapositionscale_refusals_are_named() {
    use timeline_core::{TransitionKind, Xform};
    let g = Gop { dur: 50 };
    // a placed clip on a transition's cut: V2 B[0,10)@10 then B[10,15)@20 placed, a dissolve at 20
    let mut tl = striaopacity_stack(timeline_core::OPAQUE);
    tl.tracks[1].clips.push(Clip::new(SourceId(1), 20, 25, 20));
    let tl = tl.add_transition(1, 20, TransitionKind::Dissolve, 3).unwrap().set_xform(1, 1, Xform::PIP).unwrap();
    assert_eq!(schedule(&tl, 0..30, &g), Err(ScheduleError::Xform(SourceId(1))));
    // a placed lower clip under a 50% top
    let tl = striaopacity_stack(50).set_xform(0, 0, Xform::PIP).unwrap();
    assert_eq!(schedule(&tl, 0..30, &g), Err(ScheduleError::Xform(SourceId(1))));
    // a placed clip with nothing above it over a lower placed clip: V2 placed over V1 placed
    let tl = striaopacity_stack(timeline_core::OPAQUE).set_xform(0, 0, Xform::PIP).unwrap().set_xform(1, 0, Xform::PIP).unwrap();
    assert_eq!(schedule(&tl, 0..30, &g), Err(ScheduleError::Xform(SourceId(1))));
    assert_eq!(ScheduleError::Xform(SourceId(1)).to_string(), "source 1 is placed (pip / scale) over a transition or under another layer (two layers only)");
}

// ---- STRIAOPACITYNEXT (LEDGER SR993): the lower run hands back after an opacity overlay (named tail block) ----

/// 50 % B over A: the first tick after the overlay is A's own step at its own tick with NO seek (the run the overlay
/// positioned continues), and the tick after it too; the same stack of ONE reel on both tracks still seeks there (the
/// live run is that reel's too); an opaque stack schedules as before (A reopens at 20).
#[test]
fn striaopacitynext_lower_run_hands_back() {
    let g = Gop { dur: 50 };
    let steps = schedule(&striaopacity_stack(50), 0..30, &g).unwrap();
    assert_eq!((steps[20].src, steps[20].src_t, steps[20].track), (Some(SourceId(0)), 20, Some(0)));
    assert_eq!((steps[20].seek, steps[20].vmix), (None, None));
    assert_eq!(steps[21].seek, None);
    let mut one = striaopacity_stack(50);
    one.tracks[1].clips = vec![Clip::new(SourceId(0), 0, 10, 10).with_opacity(50)];
    let steps = schedule(&one, 0..30, &g).unwrap();
    assert_eq!(steps[20].seek, Some(20 * NS), "one reel on both tracks: the live run is not the lower one");
    let steps = schedule(&striaopacity_stack(timeline_core::OPAQUE), 0..30, &g).unwrap();
    assert_eq!(steps[20].seek, Some(20 * NS), "opaque: no overlay ran the lower clip");
}

/// `vmix::take_outgoing` outside a mix: a step with no seek on the mix run's source (the live run another's) takes the
/// mix run as its live run; with a seek, or the live run already its source's, the mix run is dropped as before.
#[test]
fn striaopacitynext_take_outgoing_hands_back() {
    use timeline_core::vmix::take_outgoing;
    let (a, b) = (SourceId(0), SourceId(1));
    let step = |seek: Option<i64>| Step { kind: Kind::Video, track: Some(0), src: Some(a), src_t: 20, out_t: 20, len: 1, seek, vmix: None };
    let (mut live, mut mix) = (Some(b), Some(a));
    assert!(!take_outgoing(&step(None), &mut live, &mut mix, |r| *r));
    assert_eq!((live, mix), (Some(a), None));
    let (mut live, mut mix) = (Some(b), Some(a));
    assert!(!take_outgoing(&step(Some(0)), &mut live, &mut mix, |r| *r));
    assert_eq!((live, mix), (Some(b), None));
    let (mut live, mut mix) = (Some(a), Some(a));
    assert!(!take_outgoing(&step(None), &mut live, &mut mix, |r| *r));
    assert_eq!((live, mix), (Some(a), None));
    println!("striaopacitynext: the mix run hands back to the live slot");
}
