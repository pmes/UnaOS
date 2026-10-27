// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! NLE3 (SR202): transitions — the model's invariant and ops, the CMX 3600 two-line D / W events (byte-exact round
//! trip, named refusals) and the schedule's two-source steps.

use timeline_core::edl::{self, EdlError};
use timeline_core::schedule::{Frac, Step, schedule};
use timeline_core::{Clip, EditError, Kind, SourceId, SourceIndex, Timeline, TransitionKind, WipeDir};

const NS: i64 = 40_000_000; // one tick at 25 fps
const WIPE: TransitionKind = TransitionKind::Wipe(WipeDir::LeftToRight);

/// Every source: `dur` ticks, a video keyframe every 10 frames.
struct Gop {
    dur: i64,
}
impl SourceIndex for Gop {
    fn duration(&self, _: SourceId) -> Option<i64> {
        Some(self.dur)
    }
    fn keyframe_ns(&self, _: SourceId, _: Kind, t_ns: i64) -> Option<i64> {
        Some(t_ns / NS / 10 * 10 * NS)
    }
}

/// V1: A[0,20)@0, B[0,20)@20, C[5,25)@40 (sources 50 ticks), cut only.
fn cuts() -> Timeline {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    let c = tl.add_source("C", 50);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 20, 0), Clip::new(b, 0, 20, 20), Clip::new(c, 5, 25, 40)];
    tl
}

/// `cuts` with a 9-tick dissolve into B and a 9-tick W001 wipe into C.
fn three() -> Timeline {
    cuts().add_transition(0, 20, TransitionKind::Dissolve, 9).unwrap().add_transition(0, 40, WIPE, 9).unwrap()
}

const THREE: &str = "TITLE: NLE3
FCM: NON-DROP FRAME
* UNA FPS 25
* UNA SOURCE A 00:00:02:00
* UNA SOURCE B 00:00:02:00
* UNA SOURCE C 00:00:02:00

001  A        V     C        00:00:00:00 00:00:00:20 00:00:00:00 00:00:00:20
002  A        V     C        00:00:00:20 00:00:00:20 00:00:00:20 00:00:00:20
002  B        V     D    009 00:00:00:00 00:00:00:20 00:00:00:20 00:00:01:15
003  B        V     C        00:00:00:20 00:00:00:20 00:00:01:15 00:00:01:15
003  C        V     W001 009 00:00:00:05 00:00:01:00 00:00:01:15 00:00:02:10
";

#[test]
fn ops_add_and_remove() {
    let tl = three();
    assert_eq!(tl.tracks[0].transitions.len(), 2);
    assert_eq!((tl.tracks[0].transitions[0].at, tl.tracks[0].transitions[0].dur), (20, 9));
    assert_eq!(tl.tracks[0].transitions[1].kind, WIPE);
    // replacing on the same cut, then removing both, gives the cut-only timeline back
    let re = tl.add_transition(0, 20, WIPE, 3).unwrap();
    assert_eq!((re.tracks[0].transitions.len(), re.tracks[0].transitions[0].kind), (2, WIPE));
    assert_eq!(tl.remove_transition(0, 20).unwrap().remove_transition(0, 40).unwrap(), cuts());
    assert_eq!(cuts().remove_transition(0, 20), Err(EditError::Transition));
    assert_eq!(cuts().add_transition(5, 20, WIPE, 3), Err(EditError::NoTrack));
}

#[test]
fn the_invariant_refuses_by_name() {
    // the outgoing A[0,20) of 50 has a 30-tick handle; B[0,20) is 20 long
    assert_eq!(cuts().add_transition(0, 20, TransitionKind::Dissolve, 30), Err(EditError::Handle)); // fits A, not B
    assert_eq!(cuts().add_transition(0, 20, TransitionKind::Dissolve, 21), Err(EditError::Handle));
    let mut short = cuts();
    short.sources[0].duration = 25; // A's handle past 20 is 5
    assert_eq!(short.add_transition(0, 20, TransitionKind::Dissolve, 6), Err(EditError::Handle));
    assert!(short.add_transition(0, 20, TransitionKind::Dissolve, 5).is_ok());
    // not on a cut, zero length, an audio track
    assert_eq!(cuts().add_transition(0, 10, TransitionKind::Dissolve, 3), Err(EditError::Transition));
    assert_eq!(cuts().add_transition(0, 0, TransitionKind::Dissolve, 3), Err(EditError::Transition));
    assert_eq!(cuts().add_transition(0, 20, TransitionKind::Dissolve, 0), Err(EditError::Transition));
    let mut au = cuts();
    let a1 = au.add_track(Kind::Audio);
    au.tracks[a1].clips = au.tracks[0].clips.clone();
    assert_eq!(au.add_transition(a1, 20, TransitionKind::Dissolve, 3), Err(EditError::Transition));
    // a transition pins its cut: an edit that moves it is refused until the transition is removed
    assert_eq!(three().ripple_delete(0, 0), Err(EditError::Transition));
    assert!(three().remove_transition(0, 20).unwrap().remove_transition(0, 40).unwrap().ripple_delete(0, 0).is_ok());
}

#[test]
fn the_edl_carries_d_and_w_events_byte_exact() {
    let tl = three();
    let text = edl::write(&tl, "NLE3").unwrap();
    assert_eq!(text, THREE);
    let back = edl::parse(&text, 30).unwrap();
    assert_eq!(back, tl);
    assert_eq!(edl::write(&back, "NLE3").unwrap(), text);
    // W002 and a dissolve read without UNA comments: the outgoing reel's length counts its handle
    let bare = "TITLE: X\nFCM: NON-DROP FRAME\n\n001  A        V     C        00:00:00:00 00:00:00:20 00:00:00:00 00:00:00:20\n002  A        V     C        00:00:00:20 00:00:00:20 00:00:00:20 00:00:00:20\n002  B        V     W002 005 00:00:00:00 00:00:00:10 00:00:00:20 00:00:01:05\n";
    let tl = edl::parse(bare, 25).unwrap();
    assert_eq!(tl.source_by_reel("A").unwrap().duration, 25);
    assert_eq!(tl.tracks[0].transitions[0].kind, TransitionKind::Wipe(WipeDir::TopToBottom));
    assert_eq!(tl.tracks[0].transitions[0].dur, 5);
}

#[test]
fn edl_refusals_are_named() {
    let head = "TITLE: X\nFCM: NON-DROP FRAME\n* UNA FPS 25\n* UNA SOURCE A 00:00:01:00\n* UNA SOURCE B 00:00:01:00\n";
    let one = "001  A        V     C        00:00:00:00 00:00:00:20 00:00:00:00 00:00:00:20\n";
    let from = "002  A        V     C        00:00:00:20 00:00:00:20 00:00:00:20 00:00:00:20\n";
    let ev = |s: &str| format!("{head}{one}{from}{s}\n");
    // a dissolve longer than the outgoing handle (A is 25 ticks: 5 past its out point)
    assert_eq!(
        edl::parse(&ev("002  B        V     D    006 00:00:00:00 00:00:00:10 00:00:00:20 00:00:01:05"), 25),
        Err(EdlError::Edit(EditError::Handle))
    );
    assert!(edl::parse(&ev("002  B        V     D    005 00:00:00:00 00:00:00:10 00:00:00:20 00:00:01:05"), 25).is_ok());
    // an unknown wipe code
    assert_eq!(
        edl::parse(&ev("002  B        V     W003 005 00:00:00:00 00:00:00:10 00:00:00:20 00:00:01:05"), 25),
        Err(EdlError::Transition { line: 8 })
    );
    // the incoming line's event number differs from its outgoing line's
    assert_eq!(
        edl::parse(&ev("003  B        V     D    005 00:00:00:00 00:00:00:10 00:00:00:20 00:00:01:05"), 25),
        Err(EdlError::Transition { line: 8 })
    );
    // the outgoing line's source point is not the clip's out point
    let bad_from = "002  A        V     C        00:00:00:19 00:00:00:19 00:00:00:20 00:00:00:20\n";
    assert_eq!(
        edl::parse(&format!("{head}{one}{bad_from}002  B        V     D    005 00:00:00:00 00:00:00:10 00:00:00:20 00:00:01:05\n"), 25),
        Err(EdlError::Transition { line: 8 })
    );
    // an outgoing line with no D / W line after it is an empty clip, as before NLE3
    assert_eq!(edl::parse(&format!("{head}{one}{from}"), 25), Err(EdlError::Edit(EditError::Empty)));
    // the writer refuses a duration CMX's three digits cannot carry
    let mut long = Timeline::new(25);
    let a = long.add_source("A", 3000);
    let b = long.add_source("B", 3000);
    long.add_track(Kind::Video);
    long.tracks[0].clips = vec![Clip::new(a, 0, 10, 0), Clip::new(b, 0, 1500, 10)];
    let long = long.add_transition(0, 10, TransitionKind::Dissolve, 1000).unwrap();
    assert_eq!(edl::write(&long, "X"), Err(EdlError::Transition { line: 0 }));
}

fn mixes(steps: &[Step]) -> Vec<(i64, i64, Option<i64>, Frac)> {
    steps.iter().filter_map(|s| s.vmix.map(|m| (s.out_t, m.src_t, m.seek, m.t))).collect()
}

#[test]
fn the_schedule_names_two_sources_inside_a_transition() {
    let tl = three();
    let s = schedule(&tl, 0..60, &Gop { dur: 50 }).unwrap();
    assert_eq!(s.len(), 60); // video only: one step per tick
    let m = mixes(&s);
    assert_eq!(m.len(), 18);
    let f = |num, den| Frac { num, den };
    // the dissolve into B: the outgoing A decodes on past its out point (no seek); the weight 1/10 .. 9/10
    assert_eq!(m[0], (20, 20, None, f(1, 10)));
    assert_eq!(m[4], (24, 24, None, f(5, 10)));
    assert_eq!(m[8], (28, 28, None, f(9, 10)));
    assert!(s[20..29].iter().all(|x| x.vmix.is_some_and(|v| v.src == SourceId(0) && v.kind == TransitionKind::Dissolve)));
    // the incoming B seeks at the cut like a plain cut, then continues
    assert_eq!((s[20].src, s[20].src_t, s[20].seek), (Some(SourceId(1)), 0, Some(0)));
    assert_eq!((s[24].src_t, s[24].seek), (4, None));
    assert_eq!(s[29].vmix, None);
    // the wipe into C: the outgoing B at 20.., the incoming C seeks to keyframe 0 for its tick 5
    assert_eq!(m[9], (40, 20, None, f(1, 10)));
    assert_eq!(m[13], (44, 24, None, f(5, 10)));
    assert_eq!(m[17], (48, 28, None, f(9, 10)));
    assert!(s[40..49].iter().all(|x| x.vmix.is_some_and(|v| v.src == SourceId(1) && v.kind == WIPE)));
    assert_eq!((s[40].src, s[40].src_t, s[40].seek), (Some(SourceId(2)), 5, Some(0)));
    assert_eq!(s[49].vmix, None);
    // a range starting mid-transition seeks the outgoing too (keyframe 20 for A's tick 24)
    let s = schedule(&tl, 24..26, &Gop { dur: 50 }).unwrap();
    assert_eq!(mixes(&s)[0], (24, 24, Some(20 * NS), f(5, 10)));
    assert_eq!(mixes(&s)[1], (25, 25, None, f(6, 10)));
    // the index says the outgoing's medium ends inside the handle
    assert_eq!(schedule(&tl, 0..60, &Gop { dur: 27 }).unwrap_err(), timeline_core::schedule::ScheduleError::OutOfSource(SourceId(0)));
}
