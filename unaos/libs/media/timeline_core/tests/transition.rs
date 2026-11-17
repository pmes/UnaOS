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
    // NLE3E: a transition follows its cut; an edit that deletes the cut is refused until the transition is removed
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
    // an unknown wipe code (NLE3cBLACK: W003 is a corner box now; NLEWIPES: W007 is four boxes, W008 is refused by name)
    assert_eq!(
        edl::parse(&ev("002  B        V     W008 005 00:00:00:00 00:00:00:10 00:00:00:20 00:00:01:05"), 25),
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
    assert!(s[20..29].iter().all(|x| x.vmix.is_some_and(|v| v.src == Some(SourceId(0)) && v.kind == TransitionKind::Dissolve)));
    // the incoming B seeks at the cut like a plain cut, then continues
    assert_eq!((s[20].src, s[20].src_t, s[20].seek), (Some(SourceId(1)), 0, Some(0)));
    assert_eq!((s[24].src_t, s[24].seek), (4, None));
    assert_eq!(s[29].vmix, None);
    // the wipe into C: the outgoing B at 20.., the incoming C seeks to keyframe 0 for its tick 5
    assert_eq!(m[9], (40, 20, None, f(1, 10)));
    assert_eq!(m[13], (44, 24, None, f(5, 10)));
    assert_eq!(m[17], (48, 28, None, f(9, 10)));
    assert!(s[40..49].iter().all(|x| x.vmix.is_some_and(|v| v.src == Some(SourceId(1)) && v.kind == WIPE)));
    assert_eq!((s[40].src, s[40].src_t, s[40].seek), (Some(SourceId(2)), 5, Some(0)));
    assert_eq!(s[49].vmix, None);
    // a range starting mid-transition seeks the outgoing too (keyframe 20 for A's tick 24)
    let s = schedule(&tl, 24..26, &Gop { dur: 50 }).unwrap();
    assert_eq!(mixes(&s)[0], (24, 24, Some(20 * NS), f(5, 10)));
    assert_eq!(mixes(&s)[1], (25, 25, None, f(6, 10)));
    // the index says the outgoing's medium ends inside the handle
    assert_eq!(schedule(&tl, 0..60, &Gop { dur: 27 }).unwrap_err(), timeline_core::schedule::ScheduleError::OutOfSource(SourceId(0)));
}

// ---- NLE3c (SR209): a transition from black and the W041 / W101 / W201 wipe codes.

/// V1: a gap 0..10, then B[0,20)@10 faded in from black over 5 ticks.
fn from_black() -> Timeline {
    let mut tl = Timeline::new(25);
    tl.add_source("B", 50);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(SourceId(0), 0, 20, 10)];
    tl.add_transition_from_black(0, 10, TransitionKind::Dissolve, 5).unwrap()
}

const FROM_BLACK: &str = "TITLE: BL
FCM: NON-DROP FRAME
* UNA FPS 25
* UNA SOURCE B 00:00:02:00

001  BL       V     C        00:00:00:00 00:00:00:00 00:00:00:10 00:00:00:10
001  B        V     D    005 00:00:00:00 00:00:00:20 00:00:00:10 00:00:01:05
";

#[test]
fn nle3c_from_black_op_and_refusals() {
    let tl = from_black();
    let x = tl.tracks[0].transitions[0];
    assert_eq!((x.at, x.dur, x.from_black, x.kind), (10, 5, true, TransitionKind::Dissolve));
    // a plain dissolve where nothing ends is still off a cut; a from-black one where a clip ends is refused too
    assert_eq!(tl.remove_transition(0, 10).unwrap().add_transition(0, 10, TransitionKind::Dissolve, 5), Err(EditError::Transition));
    assert_eq!(cuts().add_transition_from_black(0, 20, TransitionKind::Dissolve, 5), Err(EditError::Transition));
    // at tick 0 of a timeline (nothing before), and longer than the incoming clip
    let mut first = cuts();
    first.tracks[0].clips.truncate(1);
    assert!(first.add_transition_from_black(0, 0, WIPE, 20).is_ok());
    assert_eq!(first.add_transition_from_black(0, 0, WIPE, 21), Err(EditError::Handle));
}

#[test]
fn nle3c_the_edl_carries_bl_and_the_new_wipes_byte_exact() {
    let tl = from_black();
    let text = edl::write(&tl, "BL").unwrap();
    assert_eq!(text, FROM_BLACK);
    let back = edl::parse(&text, 30).unwrap();
    assert_eq!(back, tl);
    assert_eq!(edl::write(&back, "BL").unwrap(), text);
    // a zero-length BL line with no D / W line after it is a gap, not an empty clip
    let gap = "001  BL       V     C        00:00:00:00 00:00:00:00 00:00:00:10 00:00:00:10\n002  B        V     C        00:00:00:00 00:00:00:20 00:00:00:10 00:00:01:05\n";
    let g = edl::parse(gap, 25).unwrap();
    assert_eq!((g.tracks[0].clips.len(), g.tracks[0].transitions.len()), (1, 0));
    // NLE3cBLACK: BL as the incoming line is now read (a transition TO black); BL on BOTH lines is refused
    let both = "001  B        V     C        00:00:00:00 00:00:00:20 00:00:00:00 00:00:00:20\n002  BL       V     C        00:00:00:00 00:00:00:00 00:00:00:20 00:00:00:20\n002  BL       V     D    005 00:00:00:00 00:00:00:05 00:00:00:20 00:00:01:00\n";
    assert_eq!(edl::parse(both, 25), Err(EdlError::Transition { line: 3 }));
    // W041 / W101 / W201 round trip
    for (d, code) in [(WipeDir::Diagonal, "W041"), (WipeDir::Box, "W101"), (WipeDir::Clock, "W201")] {
        assert_eq!(WipeDir::from_code(d.code()), Some(d));
        let tl = cuts().add_transition(0, 20, TransitionKind::Wipe(d), 7).unwrap();
        let text = edl::write(&tl, "W").unwrap();
        assert!(text.contains(&format!("{code} 007")), "{text}");
        assert_eq!(edl::parse(&text, 25).unwrap(), tl);
    }
    assert_eq!(WipeDir::from_code(8), None); // NLEWIPES: W007 is built; W008 cornersOut is refused by name
}

#[test]
fn nle3c_the_schedule_mixes_from_black() {
    let tl = from_black();
    let s = schedule(&tl, 0..20, &Gop { dur: 50 }).unwrap();
    assert!(s[..10].iter().all(|x| x.src.is_none() && x.vmix.is_none()));
    let m = s[12].vmix.unwrap();
    assert_eq!((m.src, m.src_t, m.seek, m.t), (None, 0, None, Frac { num: 3, den: 6 }));
    assert_eq!((s[10].src, s[10].seek), (Some(SourceId(0)), Some(0)));
    assert!(s[10..15].iter().all(|x| x.vmix.is_some_and(|v| v.src.is_none())));
    assert_eq!(s[15].vmix, None);
}

// ---- NLE3E (SR225): a transition follows its cut through the ops; a cut that is gone is refused by name.

fn ats(tl: &Timeline) -> Vec<(i64, i64, TransitionKind)> {
    tl.tracks[0].transitions.iter().map(|x| (x.at, x.dur, x.kind)).collect()
}

#[test]
fn nle3e_an_insert_before_a_cut_moves_its_transition() {
    use timeline_core::Mode;
    let ins = |at| Clip::new(SourceId(2), 0, 10, at);
    let tl = three().place(0, ins(0), Mode::Insert).unwrap();
    assert_eq!(ats(&tl), vec![(30, 9, TransitionKind::Dissolve), (50, 9, WIPE)]);
    // at the cut: a clip now sits between the pair, the cut is gone
    assert_eq!(three().place(0, ins(20), Mode::Insert), Err(EditError::Transition));
    // inside the incoming's first 9 ticks: the cut stays at 20 but its incoming head is 5 ticks
    assert_eq!(three().place(0, ins(25), Mode::Insert), Err(EditError::Handle));
    // after every cut: nothing moves
    assert_eq!(ats(&three().place(0, ins(60), Mode::Insert).unwrap()), ats(&three()));
}

#[test]
fn nle3e_ripple_delete_and_move_clip() {
    // an unrelated clip: the B -> C wipe moves 20 earlier with its pair
    let bc = cuts().add_transition(0, 40, WIPE, 9).unwrap();
    assert_eq!(ats(&bc.ripple_delete(0, 0).unwrap()), vec![(20, 9, WIPE)]);
    // one of the pair: refused by name
    assert_eq!(three().ripple_delete(0, 1), Err(EditError::Transition));
    // move_clip moves one clip and never ripples: an unrelated clip keeps the transition, one of the pair is refused
    let ab = cuts().add_transition(0, 20, TransitionKind::Dissolve, 9).unwrap();
    let moved = ab.move_clip(0, 2, 0, 70).unwrap();
    assert_eq!((ats(&moved), moved.tracks[0].clips[2].at), (vec![(20, 9, TransitionKind::Dissolve)], 70));
    assert_eq!(ab.move_clip(0, 1, 0, 70), Err(EditError::Transition));
}

#[test]
fn nle3e_trims_keep_or_refuse_by_name() {
    // the outgoing's out edge (the cut's near edge) shorter: a gap opens at the cut
    assert_eq!(three().trim_out(0, 0, 15), Err(EditError::Transition));
    // the outgoing's in edge (far edge): the cut stays at 20
    let tl = three().trim_in(0, 0, 5).unwrap();
    assert_eq!((ats(&tl), tl.tracks[0].clips[0].at), (ats(&three()), 5));
    // the incoming shorter than the dissolve: NLE3ETRIMSTRETCH (SR692) shrinks the dissolve to fit (was refused, Handle)
    let ab = cuts().add_transition(0, 20, TransitionKind::Dissolve, 9).unwrap();
    assert_eq!(ats(&ab.trim_out(0, 1, 5).unwrap()), vec![(20, 5, TransitionKind::Dissolve)]);
}

#[test]
fn nle3e_the_audio_crossfade_follows_the_same_op() {
    use timeline_core::Mode;
    // V1 and A1: A[10,30)@0, B[0,40)@20, a 9-tick dissolve on V1's cut at 20 (mix.rs's xfade)
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Video);
    tl.add_track(Kind::Audio);
    for t in 0..2 {
        tl.tracks[t].clips = vec![Clip::new(a, 10, 30, 0), Clip::new(b, 0, 40, 20)];
    }
    let tl = tl.add_transition(0, 20, TransitionKind::Dissolve, 9).unwrap();
    let ins = Clip::new(b, 40, 50, 0);
    // the insert on V1 alone: the video transition moves, the audio cut at 20 is plain
    let v = tl.place(0, ins, Mode::Insert).unwrap();
    assert_eq!(ats(&v), vec![(30, 9, TransitionKind::Dissolve)]);
    assert_eq!((v.audio_crossfade(1, 1), v.validate()), (None, Ok(())));
    // the same insert on A1: the crossfade is derived at the new cut
    let va = v.place(1, ins, Mode::Insert).unwrap();
    assert_eq!(va.audio_crossfade(1, 2).map(|x| (x.at, x.dur)), Some((30, 9)));
}

// ---- NLE3cBLACK (SR323): a transition TO black (CMX `BL` incoming line) and the new SMPTE wipe codes.

/// V1: A[0,20)@0 (source 50 ticks), then a 5-tick dissolve to black in the gap 20..25.
fn to_black() -> Timeline {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 20, 0)];
    tl.add_transition_to_black(0, 20, TransitionKind::Dissolve, 5).unwrap()
}

const TO_BLACK: &str = "TITLE: TB
FCM: NON-DROP FRAME
* UNA FPS 25
* UNA SOURCE A 00:00:02:00

001  A        V     C        00:00:00:00 00:00:00:20 00:00:00:00 00:00:00:20
002  A        V     C        00:00:00:20 00:00:00:20 00:00:00:20 00:00:00:20
002  BL       V     D    005 00:00:00:00 00:00:00:05 00:00:00:20 00:00:01:00
";

#[test]
fn nle3cblack_to_black_op_and_refusals() {
    let tl = to_black();
    let x = tl.tracks[0].transitions[0];
    assert_eq!((x.at, x.dur, x.from_black, x.to_black, x.kind), (20, 5, false, true, TransitionKind::Dissolve));
    // an incoming clip at the cut is a plain cut, not a to-black one; nothing ends at 30
    assert_eq!(cuts().add_transition_to_black(0, 20, TransitionKind::Dissolve, 5), Err(EditError::Transition));
    assert_eq!(tl.add_transition_to_black(0, 30, TransitionKind::Dissolve, 5), Err(EditError::Transition));
    // never both from and to black
    let mut both = to_black();
    both.tracks[0].transitions[0].from_black = true;
    assert_eq!(both.validate(), Err(EditError::Transition));
    // a clip starting inside the black refuses; one starting where the black ends is fine
    let mut inside = to_black();
    inside.tracks[0].clips.push(Clip::new(SourceId(0), 30, 40, 22));
    assert_eq!(inside.validate(), Err(EditError::Handle));
    let mut after = to_black();
    after.tracks[0].clips.push(Clip::new(SourceId(0), 30, 40, 25));
    assert_eq!(after.validate(), Ok(()));
    // the outgoing's handle: source 22 ticks holds 2 past src_out 20
    let mut short = Timeline::new(25);
    let a = short.add_source("A", 22);
    short.add_track(Kind::Video);
    short.tracks[0].clips = vec![Clip::new(a, 0, 20, 0)];
    assert_eq!(short.add_transition_to_black(0, 20, WIPE, 5), Err(EditError::Handle));
    assert!(short.add_transition_to_black(0, 20, WIPE, 2).is_ok());
}

#[test]
fn nle3cblack_the_edl_carries_bl_incoming_byte_exact() {
    let tl = to_black();
    let text = edl::write(&tl, "TB").unwrap();
    assert_eq!(text, TO_BLACK);
    let back = edl::parse(&text, 30).unwrap();
    assert_eq!(back, tl);
    assert_eq!(edl::write(&back, "TB").unwrap(), text);
    // a BL incoming line whose record span is shorter than the transition is refused
    let short = "001  A        V     C        00:00:00:00 00:00:00:20 00:00:00:00 00:00:00:20\n002  A        V     C        00:00:00:20 00:00:00:20 00:00:00:20 00:00:00:20\n002  BL       V     D    005 00:00:00:00 00:00:00:04 00:00:00:20 00:00:00:24\n";
    assert_eq!(edl::parse(short, 25), Err(EdlError::Transition { line: 3 }));
    // the new SMPTE codes round trip
    let codes = [
        (WipeDir::CornerTopLeft, "W003"),
        (WipeDir::CornerTopRight, "W004"),
        (WipeDir::CornerBottomRight, "W005"),
        (WipeDir::CornerBottomLeft, "W006"),
        (WipeDir::BarnDoorVertical, "W021"),
        (WipeDir::BarnDoorHorizontal, "W022"),
        (WipeDir::DiagonalTopRight, "W042"),
        (WipeDir::ClockThree, "W202"),
        (WipeDir::ClockSix, "W203"),
        (WipeDir::ClockNine, "W204"),
    ];
    for (d, code) in codes {
        assert_eq!(WipeDir::from_code(d.code()), Some(d));
        let tl = cuts().add_transition(0, 20, TransitionKind::Wipe(d), 7).unwrap();
        let text = edl::write(&tl, "W").unwrap();
        assert!(text.contains(&format!("{code} 007")), "{text}");
        assert_eq!(edl::parse(&text, 25).unwrap(), tl);
    }
}

#[test]
fn nle3cblack_the_schedule_mixes_to_black() {
    let tl = to_black();
    let s = schedule(&tl, 0..30, &Gop { dur: 50 }).unwrap();
    assert!(s[..20].iter().all(|x| x.src == Some(SourceId(0)) && x.vmix.is_none()));
    for k in 0..5i64 {
        let x = s[20 + k as usize];
        assert_eq!((x.track, x.src, x.seek), (Some(0), None, None), "tick {}", 20 + k);
        let m = x.vmix.unwrap();
        // the outgoing decodes on past its out point: no seek at the cut or after it
        assert_eq!((m.src, m.src_t, m.seek, m.t), (Some(SourceId(0)), 20 + k, None, Frac { num: k + 1, den: 6 }));
    }
    assert!(s[25..].iter().all(|x| x.track.is_none() && x.src.is_none() && x.vmix.is_none()));
    // a range that starts inside the black seeks the outgoing there
    let mid = schedule(&tl, 22..23, &Gop { dur: 50 }).unwrap();
    assert_eq!(mid[0].vmix.map(|m| (m.src_t, m.seek)), Some((22, Some(20 * NS))));
    // the black owns its track's gap: a lower track's clip under it loses there, wins after
    let mut two = Timeline::new(25);
    let a = two.add_source("A", 50);
    let b = two.add_source("B", 50);
    two.add_track(Kind::Video);
    two.add_track(Kind::Video);
    two.tracks[0].clips = vec![Clip::new(b, 0, 30, 0)];
    two.tracks[1].clips = vec![Clip::new(a, 0, 20, 0)];
    let two = two.add_transition_to_black(1, 20, TransitionKind::Dissolve, 5).unwrap();
    let s = schedule(&two, 0..30, &Gop { dur: 50 }).unwrap();
    assert_eq!((s[22].track, s[22].src), (Some(1), None));
    assert_eq!((s[25].track, s[25].src, s[25].src_t), (Some(0), Some(b), 25));
}

#[test]
fn nle3cblack_a_to_black_follows_its_cut() {
    use timeline_core::Mode;
    // an Insert before the cut moves it with the outgoing clip's end
    let tl = to_black().place(0, Clip::new(SourceId(0), 30, 40, 0), Mode::Insert).unwrap();
    assert_eq!(ats(&tl), vec![(30, 5, TransitionKind::Dissolve)]);
    assert!(tl.tracks[0].transitions[0].to_black);
    // lifting the outgoing clip leaves no cut: refused by name
    assert_eq!(to_black().lift(0, 0), Err(EditError::Transition));
}

// ---- NLEWIPES (LEDGER SR679): the SMPTE families, the ease, the refused codes (named tail block) ----

/// The 64x48 mask of `d` at `t`: which pixels show the incoming (white over a black outgoing).
fn nlewipes_mask(d: WipeDir, t: Frac) -> Vec<bool> {
    let mut o = [0u8, 0, 0, 255].repeat(64 * 48);
    timeline_core::vmix::mix_step(TransitionKind::Wipe(d), t, &mut o, &[255u8, 255, 255, 255].repeat(64 * 48), 64, 48);
    o.chunks_exact(4).map(|p| p[0] == 255).collect()
}

const NLEWIPES_BUILT: [(WipeDir, u16); 19] = [
    (WipeDir::FourBoxCornersIn, 7),
    (WipeDir::EdgeBoxTop, 23),
    (WipeDir::EdgeBoxRight, 24),
    (WipeDir::EdgeBoxBottom, 25),
    (WipeDir::EdgeBoxLeft, 26),
    (WipeDir::BowTieVertical, 43),
    (WipeDir::BowTieHorizontal, 44),
    (WipeDir::BarnDoorDiagonalBottomLeft, 45),
    (WipeDir::BarnDoorDiagonalTopLeft, 46),
    (WipeDir::DoubleBarnDoor, 47),
    (WipeDir::Diamond, 102),
    (WipeDir::Circle, 119),
    (WipeDir::PinWheelTwoVertical, 205),
    (WipeDir::PinWheelTwoHorizontal, 206),
    (WipeDir::PinWheelFour, 207),
    (WipeDir::FanCenterTop, 211),
    (WipeDir::FanCenterRight, 212),
    (WipeDir::DoubleFanVertical, 213),
    (WipeDir::DoubleFanHorizontal, 214),
];

#[test]
fn nlewipes_every_new_code_is_none_at_zero_and_all_at_one_and_round_trips() {
    for (d, code) in NLEWIPES_BUILT {
        assert!(nlewipes_mask(d, Frac { num: 0, den: 1 }).iter().all(|p| !p), "{d:?} at 0");
        assert!(nlewipes_mask(d, Frac { num: 1, den: 1 }).iter().all(|p| *p), "{d:?} at 1");
        assert_eq!((d.code(), WipeDir::from_code(code), WipeDir::refused(code)), (code, Some(d), None));
        let tl = cuts().add_transition(0, 20, TransitionKind::Wipe(d), 7).unwrap();
        let text = edl::write(&tl, "W").unwrap();
        assert!(text.contains(&format!("W{code:03} 007")), "{text}");
        assert_eq!(edl::parse(&text, 25).unwrap(), tl);
    }
}

/// The pinned pixels at t = 1/2, hand-derived on 64x48 with X = 2x - 63, Y = 2y - 47 (doubled centred coordinates):
/// * W007: in iff min(x, 63-x) < 16 and min(y, 47-y) < 12 (2 * dist * 2 < 64 / 48).
/// * W023 top: x in 16..=47 (|2x-63| < 32) and y < 24; W024 right: y in 12..=35 and x >= 32; W025 bottom: x in 16..=47
///   and y >= 24; W026 left: y in 12..=35 and x < 32.
/// * W043: 64|Y| > 48|X| (the hourglass): (32,0) X=1 Y=-47 -> 3008 > 48 in; (20,5) 2368 > 1104 in; (5,20) X=-53 Y=-7 ->
///   448 > 2544 out; (0,24) out. W044 the swap.
/// * W045: |48X + 64Y| < 3072: (0,47) -3024+3008 = -16 in, (63,0) 16 in, (32,24) 112 in, (0,0) -6032 out, (63,47) out.
///   W046: |48X - 64Y| < 3072: (0,0) -16 in, (63,47) in, (32,24) -16 in, (0,47) / (63,0) out.
/// * W102: 48|X| + 64|Y| < 3072: (32,24) 112 in, (16,12) 2960 in, (32,0) 3056 in, (15,11) 3184 out, (0,24) 3088 out.
/// * W119: X^2 + Y^2 < 1600: (51,24) 1522 in, (32,43) 1522 in, (52,24) 1682 out, (32,44) 1682 out.
/// * W205 (hands from 12 and 6): the top-right and bottom-left quadrants; W206 (3 and 9): bottom-right and top-left.
/// * W207 (four hands, each half its quadrant): (36,5) at 12 deg, (60,28) at 99, (28,44) at 190, (4,20) at 277 in;
///   (60,20) at 83, (4,28) at 261 out.
/// * W211 (fan from 12): the top half; W212 (from 3): the right half.
/// * W213 (fans from 12 and 6, within 45 deg of the vertical): (36,2) at 12, (34,46) at 174 in; (60,26) at 95, (4,22)
///   at 273 out. W214 the swap.
#[test]
fn nlewipes_each_new_mask_at_one_half() {
    let half = Frac { num: 1, den: 2 };
    type Pins = (WipeDir, &'static [(usize, usize)], &'static [(usize, usize)]);
    let cases: [Pins; 18] = [
        (WipeDir::FourBoxCornersIn, &[(0, 0), (15, 11), (63, 47), (48, 36), (0, 47)], &[(16, 0), (0, 12), (32, 24), (47, 35)]),
        (WipeDir::EdgeBoxTop, &[(16, 0), (47, 23), (32, 10)], &[(15, 0), (48, 0), (32, 24)]),
        (WipeDir::EdgeBoxRight, &[(63, 12), (32, 35)], &[(31, 24), (63, 11), (63, 36)]),
        (WipeDir::EdgeBoxBottom, &[(16, 47), (47, 24)], &[(15, 47), (32, 23)]),
        (WipeDir::EdgeBoxLeft, &[(0, 12), (31, 35)], &[(32, 24), (0, 11)]),
        (WipeDir::BowTieVertical, &[(32, 0), (32, 47), (20, 5)], &[(0, 24), (63, 24), (5, 20)]),
        (WipeDir::BowTieHorizontal, &[(0, 24), (63, 24), (5, 20)], &[(32, 0), (32, 47), (20, 5)]),
        (WipeDir::BarnDoorDiagonalBottomLeft, &[(0, 47), (63, 0), (32, 24)], &[(0, 0), (63, 47)]),
        (WipeDir::BarnDoorDiagonalTopLeft, &[(0, 0), (63, 47), (32, 24)], &[(0, 47), (63, 0)]),
        (WipeDir::Diamond, &[(32, 24), (16, 12), (32, 0)], &[(0, 0), (0, 24), (15, 11), (63, 47)]),
        (WipeDir::Circle, &[(32, 24), (51, 24), (32, 43)], &[(52, 24), (32, 44), (0, 0)]),
        (WipeDir::PinWheelTwoVertical, &[(40, 10), (20, 40)], &[(20, 10), (40, 40)]),
        (WipeDir::PinWheelTwoHorizontal, &[(40, 40), (20, 10)], &[(40, 10), (20, 40)]),
        (WipeDir::PinWheelFour, &[(36, 5), (60, 28), (28, 44), (4, 20)], &[(60, 20), (4, 28)]),
        (WipeDir::FanCenterTop, &[(10, 10), (50, 10)], &[(10, 40), (50, 40)]),
        (WipeDir::FanCenterRight, &[(50, 10), (50, 40)], &[(10, 10), (10, 40)]),
        (WipeDir::DoubleFanVertical, &[(36, 2), (34, 46)], &[(60, 26), (4, 22)]),
        (WipeDir::DoubleFanHorizontal, &[(60, 26), (4, 22)], &[(36, 2), (34, 46)]),
    ];
    for (d, ins, outs) in cases {
        let m = nlewipes_mask(d, half);
        for &(x, y) in ins {
            assert!(m[y * 64 + x], "{d:?} ({x},{y}) in");
        }
        for &(x, y) in outs {
            assert!(!m[y * 64 + x], "{d:?} ({x},{y}) out");
        }
    }
    // W047 (both diagonal bands) covers the frame at 1/2, so its pins are at t = 1/4: |48X +- 64Y| < 1536 for either
    // sign: (0,0) / (0,47) / (32,24) in; (32,0) BL -2960 TL 3056 out; (0,24) BL -2960 TL -3088 out.
    let m = nlewipes_mask(WipeDir::DoubleBarnDoor, Frac { num: 1, den: 4 });
    assert!(m[0] && m[47 * 64] && m[24 * 64 + 32] && !m[32] && !m[24 * 64]);
}

#[test]
fn nlewipes_the_ease_curves_and_where_they_apply() {
    use timeline_core::Ease;
    let q = |n: i64, d: i64| Frac { num: n, den: d };
    let curve = |e: Ease| [q(1, 4), q(2, 4), q(3, 4)].map(|t| e.apply(t));
    assert_eq!(curve(Ease::Linear), [q(1, 4), q(2, 4), q(3, 4)]);
    assert_eq!(curve(Ease::In), [q(1, 16), q(4, 16), q(9, 16)]);
    assert_eq!(curve(Ease::Out), [q(7, 16), q(12, 16), q(15, 16)]);
    assert_eq!(curve(Ease::InOut), [q(10, 64), q(32, 64), q(54, 64)]);
    assert_eq!((Ease::default(), Ease::from_name("InOut"), Ease::from_name("smooth")), (Ease::Linear, Some(Ease::InOut), None));
    // the dissolve under InOut at t = 1/2 is Linear's (smoothstep(1/2) = 4/8 -> alpha round(255 * 4/8) = 128)
    let mix = |t: Frac| {
        let mut o = [10u8, 10, 10, 255].repeat(64 * 48);
        timeline_core::vmix::mix_step(TransitionKind::Dissolve, t, &mut o, &[210u8, 210, 210, 255].repeat(64 * 48), 64, 48);
        o
    };
    assert_eq!(mix(Ease::InOut.apply(q(1, 2))), mix(q(1, 2)));
    assert_ne!(mix(Ease::In.apply(q(1, 2))), mix(q(1, 2)));
    // the schedule eases the step's weight once: the dissolve into B (dur 9) at k = 4 is InOut(5/10) = 500/1000
    let mut tl = three();
    tl.tracks[0].transitions[0].ease = Ease::InOut;
    let s = schedule(&tl, 0..60, &Gop { dur: 50 }).unwrap();
    assert_eq!(s[24].vmix.map(|m| m.t), Some(q(500, 1000)));
    assert_eq!(s[44].vmix.map(|m| m.t), Some(q(5, 10))); // the W001 wipe stays linear
    // a re-put at the same cut (the editor's length change) keeps the ease
    let longer = tl.add_transition(0, 20, TransitionKind::Dissolve, 8).unwrap();
    assert_eq!((longer.tracks[0].transitions[0].dur, longer.tracks[0].transitions[0].ease), (8, Ease::InOut));
}

#[test]
fn nlewipes_the_rest_of_the_table_is_refused_by_name() {
    for &(code, name) in WipeDir::REFUSED {
        assert_eq!((WipeDir::from_code(code), WipeDir::refused(code)), (None, Some(name)), "W{code:03}");
    }
    assert_eq!(WipeDir::refused(8), Some("fourBoxWipe cornersOut"));
    assert_eq!(WipeDir::refused(1), None); // built
    assert_eq!(WipeDir::refused(9), None); // not in the table
}

// ---- NLE3ETRIMSTRETCH (LEDGER SR692): the stretching trim, the ripple trims and the transitioned pair's move (named tail block) ----

/// V1 and A1: A[10,30)@0 -> B[0,5)@20, a 5-tick dissolve on V1's cut (the incoming exactly fills it).
fn nle3etrimstretch_tight() -> Timeline {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Video);
    tl.add_track(Kind::Audio);
    for t in 0..2 {
        tl.tracks[t].clips = vec![Clip::new(a, 10, 30, 0), Clip::new(b, 0, 5, 20)];
    }
    tl.add_transition(0, 20, TransitionKind::Dissolve, 5).unwrap()
}

/// V1: A[0,20)@0 -> B[0,20)@20 under a 9-tick dissolve, C[5,25)@60 after a gap.
fn nle3etrimstretch_pair() -> Timeline {
    let mut tl = cuts();
    tl.tracks[0].clips[2].at = 60;
    tl.add_transition(0, 20, TransitionKind::Dissolve, 9).unwrap()
}

#[test]
fn nle3etrimstretch_a_ripple_trim_in_shrinks_the_dissolve_and_its_crossfade() {
    let tl = nle3etrimstretch_tight();
    // eating 2 of the incoming's 5 ticks keeps the cut at 20 and leaves the dissolve 3, the derived crossfade with it
    let r = tl.ripple_trim_in(0, 1, 2).unwrap();
    assert_eq!((ats(&r), r.tracks[0].clips[1]), (vec![(20, 3, TransitionKind::Dissolve)], Clip::new(SourceId(1), 2, 5, 20)));
    assert_eq!(r.audio_crossfade(1, 1).map(|x| (x.at, x.dur)), Some((20, 3)));
    assert_eq!(r.crossfade_at(1, 1).map(|x| (x.at, x.dur)), Some((20, 3)));
    // the plain trim_in at the cut opens a gap there: the cut is gone, refused by name as before
    assert_eq!(tl.trim_in(0, 1, 2), Err(EditError::Transition));
}

#[test]
fn nle3etrimstretch_an_explicit_audio_crossfade_fits_the_trim() {
    use timeline_core::Curve;
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Audio);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 20, 0), Clip::new(b, 0, 20, 20)];
    let tl = tl.add_audio_xfade(0, 20, 5, Curve::EqualPower).unwrap();
    let t = tl.trim_out(0, 1, 3).unwrap();
    assert_eq!((t.audio_xfades[0].at, t.audio_xfades[0].dur, t.audio_xfades[0].curve), (20, 3, Curve::EqualPower));
}

#[test]
fn nle3etrimstretch_a_ripple_trim_out_moves_the_rest_and_refuses_past_the_handle() {
    // the outgoing A[0,20) of a 50-tick source: out to 45 leaves a 5-tick handle, the dissolve shrinks to 5 at the moved cut
    let r = three().ripple_trim_out(0, 0, 45).unwrap();
    assert_eq!(ats(&r), vec![(45, 5, TransitionKind::Dissolve), (65, 9, WIPE)]);
    // out to the source's end: no handle at all, refused by name (never below 1)
    assert_eq!(three().ripple_trim_out(0, 0, 50), Err(EditError::Handle));
    // the middle B shortened by 5: C and the wipe move 5 earlier, the dissolve stays
    let r = three().ripple_trim_out(0, 1, 15).unwrap();
    assert_eq!((ats(&r), r.tracks[0].clips[2].at), (vec![(20, 9, TransitionKind::Dissolve), (35, 9, WIPE)], 35));
    let text = edl::write(&r, "NLE3").unwrap();
    let back = edl::parse(&text, 25).unwrap();
    assert_eq!((ats(&back), edl::write(&back, "NLE3").unwrap()), (ats(&r), text));
}

#[test]
fn nle3etrimstretch_move_pair_carries_the_transition() {
    let tl = nle3etrimstretch_pair();
    let m = tl.move_pair(0, 0, 10).unwrap();
    assert_eq!(ats(&m), vec![(30, 9, TransitionKind::Dissolve)]);
    assert_eq!(m.tracks[0].clips.iter().map(|c| c.at).collect::<Vec<_>>(), vec![10, 30, 60]);
    // from either clip of the pair
    assert_eq!(tl.move_pair(0, 1, 10), Ok(m));
    // onto C's span, before zero, a clip with no transition: refused by name
    assert_eq!(tl.move_pair(0, 0, 25), Err(EditError::Overlap));
    assert_eq!(tl.move_pair(0, 0, -1), Err(EditError::Negative));
    assert_eq!(tl.move_pair(0, 2, 1), Err(EditError::Transition));
    // move_many does not carry a transition (measured at the cut, unchanged)
    assert_eq!(tl.move_many(&[(0, 0), (0, 1)], 10), Err(EditError::Transition));
}

#[test]
fn nle3etrimstretch_the_editor_moves_a_pair_and_ripple_trims() {
    use timeline_core::editor::EditorState;
    let tl = nle3etrimstretch_pair();
    let mut st = EditorState::new(tl.clone());
    st.select_many(&[(0, 0), (0, 1)], false);
    assert_eq!(st.verb("nudge+"), Some(Ok(())));
    assert_eq!(st.timeline, tl.move_pair(0, 0, 1).unwrap());
    assert_eq!(st.set, vec![(0, 0), (0, 1)]);
    let mut st = EditorState::new(three());
    st.select(0, 1).unwrap();
    assert_eq!(st.verb("rtrim out -5"), Some(Ok(())));
    assert_eq!(st.timeline, three().ripple_trim_out(0, 1, 15).unwrap());
    assert_eq!(st.verb("rtrim sideways 1"), None);
    st.deselect();
    assert_eq!(st.verb("rtrim in 1"), Some(Err(EditError::NoClip)));
}

// ---- NLE3ESLIP (LEDGER SR704): slip and slide around transitions (named tail block) ----

/// V1: A[0,20)@0 of a 30-tick source (a 10-tick handle) -> B[0,20)@20 under a 9-tick dissolve.
fn nle3eslip_short() -> Timeline {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 30);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 20, 0), Clip::new(b, 0, 20, 20)];
    tl.add_transition(0, 20, TransitionKind::Dissolve, 9).unwrap()
}

#[test]
fn nle3eslip_slip_keeps_the_transitions() {
    // B slipped 3: both of its cuts stay where they are, the dissolve and the wipe unchanged
    let s = three().slip(0, 1, 3).unwrap();
    assert_eq!(s.tracks[0].clips[1], Clip::new(SourceId(1), 3, 23, 20));
    assert_eq!(s.tracks[0].transitions, three().tracks[0].transitions);
    // A slipped 25 leaves a 5-tick handle under the 9-tick dissolve: refused by name (no fit on a slip)
    assert_eq!(three().slip(0, 0, 25), Err(EditError::Handle));
}

#[test]
fn nle3eslip_slide_retargets_and_fits_the_dissolve() {
    // a 9-tick dissolve into C; B slid 12 leaves C 8 ticks: the dissolve moves with the cut and fits its room
    let tl = cuts().add_transition(0, 40, TransitionKind::Dissolve, 9).unwrap();
    let s = tl.slide(0, 1, 12).unwrap();
    let (a, b, c) = (SourceId(0), SourceId(1), SourceId(2));
    assert_eq!(s.tracks[0].clips, vec![Clip::new(a, 0, 32, 0), Clip::new(b, 0, 20, 32), Clip::new(c, 17, 25, 52)]);
    assert_eq!((ats(&s), s.end()), (vec![(52, 8, TransitionKind::Dissolve)], tl.end()));
    let text = edl::write(&s, "NLE3").unwrap();
    let back = edl::parse(&text, 25).unwrap();
    assert_eq!((ats(&back), edl::write(&back, "NLE3").unwrap()), (ats(&s), text));
    // the previous clip's handle under the slid cut: 5 left fits the dissolve to 5, none left is Handle, past it OutOfSource
    assert_eq!(ats(&nle3eslip_short().slide(0, 1, 5).unwrap()), vec![(25, 5, TransitionKind::Dissolve)]);
    assert_eq!(nle3eslip_short().slide(0, 1, 10), Err(EditError::Handle));
    assert_eq!(nle3eslip_short().slide(0, 1, 11), Err(EditError::OutOfSource));
    // the next clip's in point before its source's start; the neighbour-less side onto C's span
    assert_eq!(three().slide(0, 1, -6), Err(EditError::OutOfSource));
    assert_eq!(nle3etrimstretch_pair().slide(0, 1, 21), Err(EditError::Overlap));
}

#[test]
fn nle3eslip_the_editor_slips_and_slides_once() {
    use timeline_core::editor::EditorState;
    let mut st = EditorState::new(three());
    st.select(0, 1).unwrap();
    assert_eq!(st.verb("slide 5"), Some(Ok(())));
    assert_eq!((st.timeline.clone(), st.undo_depth()), (three().slide(0, 1, 5).unwrap(), 1));
    assert_eq!(st.verb("undo"), Some(Ok(())));
    assert_eq!(st.timeline, three());
    assert_eq!(st.verb("slip 3"), Some(Ok(())));
    let slipped = three().slip(0, 1, 3).unwrap();
    assert_eq!((st.timeline.clone(), st.undo_depth()), (slipped.clone(), 1));
    // refused: the state and the undo stack unchanged
    assert_eq!(st.verb("slip -4"), Some(Err(EditError::OutOfSource)));
    assert_eq!((st.timeline.clone(), st.undo_depth()), (slipped, 1));
    assert_eq!(st.verb("slide sideways"), None);
    st.deselect();
    assert_eq!(st.verb("slip 1"), Some(Err(EditError::NoClip)));
}

// ---- NLE3ETRIMGROW (LEDGER SR712): a fitted transition grows back to its asked length (named tail block) ----

/// `nle3etrimstretch_pair` with B trimmed to 5 ticks: the 9-tick dissolve fitted to 5, its ask kept.
fn nle3etrimgrow_shrunk() -> Timeline {
    nle3etrimstretch_pair().trim_out(0, 1, 5).unwrap()
}

/// `(dur, want)` of the transition on V1's cut at 20.
fn nle3etrimgrow_dw(tl: &Timeline) -> (i64, Option<i64>) {
    let x = tl.tracks[0].transitions.iter().find(|x| x.at == 20).unwrap();
    (x.dur, x.want)
}

#[test]
fn nle3etrimgrow_a_freed_room_grows_the_dissolve_back_to_its_ask() {
    let s = nle3etrimgrow_shrunk();
    assert_eq!(nle3etrimgrow_dw(&s), (5, Some(9)));
    // a little room back: the dissolve grows with it, the ask kept
    assert_eq!(nle3etrimgrow_dw(&s.trim_out(0, 1, 7).unwrap()), (7, Some(9)));
    // more room than the ask: back to 9, never past it, the ask cleared
    assert_eq!(nle3etrimgrow_dw(&s.trim_out(0, 1, 30).unwrap()), (9, None));
    // the trim-back is the original timeline
    assert_eq!(s.trim_out(0, 1, 20), Ok(nle3etrimstretch_pair()));
}

#[test]
fn nle3etrimgrow_a_length_set_is_a_fresh_ask() {
    use timeline_core::editor::EditorState;
    let mut st = EditorState::new(nle3etrimgrow_shrunk());
    st.select_transition(0, 0).unwrap();
    st.set_transition_dur(4).unwrap();
    assert_eq!(nle3etrimgrow_dw(&st.timeline), (4, None));
    assert_eq!(nle3etrimgrow_dw(&st.timeline.trim_out(0, 1, 20).unwrap()), (4, None));
    // the add op on the cut is the same fresh ask
    let a = nle3etrimgrow_shrunk().add_transition(0, 20, TransitionKind::Dissolve, 3).unwrap();
    assert_eq!(nle3etrimgrow_dw(&a), (3, None));
    assert_eq!(nle3etrimgrow_dw(&a.trim_out(0, 1, 20).unwrap()), (3, None));
}

#[test]
fn nle3etrimgrow_the_derived_crossfade_and_the_slide_follow() {
    // the ripple trim-in that fitted the dissolve to 3, undone by its inverse: the dissolve and A1's crossfade back at 5
    let tl = nle3etrimstretch_tight();
    let r = tl.ripple_trim_in(0, 1, 2).unwrap();
    assert_eq!(nle3etrimgrow_dw(&r), (3, Some(5)));
    let back = r.ripple_trim_in(0, 1, 0).unwrap();
    assert_eq!(nle3etrimgrow_dw(&back), (5, None));
    assert_eq!(back.audio_crossfade(1, 1).map(|x| (x.at, x.dur)), Some((20, 5)));
    assert_eq!(back, tl);
    // SR704's slide that fitted the dissolve to the 5-tick handle, slid back: the original
    let s = nle3eslip_short().slide(0, 1, 5).unwrap();
    assert_eq!((s.tracks[0].transitions[0].dur, s.tracks[0].transitions[0].want), (5, Some(9)));
    assert_eq!(s.slide(0, 1, -5), Ok(nle3eslip_short()));
}

const NLE3ETRIMGROW_EDL: &str = "TITLE: GROW
FCM: NON-DROP FRAME
* UNA FPS 25
* UNA SOURCE A 00:00:02:00
* UNA SOURCE B 00:00:02:00
* UNA SOURCE C 00:00:02:00

001  A        V     C        00:00:00:00 00:00:00:20 00:00:00:00 00:00:00:20
002  A        V     C        00:00:00:20 00:00:00:20 00:00:00:20 00:00:00:20
002  B        V     D    005 00:00:00:00 00:00:00:05 00:00:00:20 00:00:01:00
* UNA WANT 9
003  C        V     C        00:00:00:05 00:00:01:00 00:00:02:10 00:00:03:05
";

#[test]
fn nle3etrimgrow_the_want_line_round_trips_byte_exact_and_refuses_by_name() {
    let s = nle3etrimgrow_shrunk();
    let text = edl::write(&s, "GROW").unwrap();
    assert_eq!(text, NLE3ETRIMGROW_EDL);
    let back = edl::parse(&text, 25).unwrap();
    assert_eq!((back.tracks[0].transitions.clone(), edl::write(&back, "GROW").unwrap()), (s.tracks[0].transitions.clone(), text));
    // at its ask: no WANT line, the trim-back writes the original's bytes
    let plain = edl::write(&nle3etrimstretch_pair(), "GROW").unwrap();
    assert!(!plain.contains("UNA WANT"), "{plain}");
    assert_eq!(edl::write(&s.trim_out(0, 1, 20).unwrap(), "GROW").unwrap(), plain);
    // refused by name at the WANT line: not above the dissolve's 5, not a number, twice, under a plain cut
    let swap = |to: &str| NLE3ETRIMGROW_EDL.replace("* UNA WANT 9\n", to);
    assert_eq!(edl::parse(&swap("* UNA WANT 5\n"), 25), Err(EdlError::Syntax { line: 11 }));
    assert_eq!(edl::parse(&swap("* UNA WANT 4\n"), 25), Err(EdlError::Syntax { line: 11 }));
    assert_eq!(edl::parse(&swap("* UNA WANT nine\n"), 25), Err(EdlError::Syntax { line: 11 }));
    assert_eq!(edl::parse(&swap("* UNA WANT 9\n* UNA WANT 9\n"), 25), Err(EdlError::Syntax { line: 12 }));
    let first = "001  A        V     C        00:00:00:00 00:00:00:20 00:00:00:00 00:00:00:20\n";
    let orphan = NLE3ETRIMGROW_EDL.replace(first, &format!("{first}* UNA WANT 9\n"));
    assert_eq!(edl::parse(&orphan, 25), Err(EdlError::Syntax { line: 9 }));
}

// ---- STRIATRANSITIONLENGTH (LEDGER SR870): the `dur <n>` word (named tail block) ----

/// `cuts` with a 4-tick dissolve into B at 20, eased In, selected.
fn striatransitionlength_state() -> timeline_core::editor::EditorState {
    let tl = cuts().add_transition(0, 20, TransitionKind::Dissolve, 4).unwrap();
    let mut st = timeline_core::editor::EditorState::new(tl);
    st.select_transition(0, 0).unwrap();
    st.set_transition_ease(timeline_core::Ease::In).unwrap();
    st
}

#[test]
fn striatransitionlength_dur_word_sets_the_length() {
    let mut st = striatransitionlength_state();
    let (tl0, depth) = (st.timeline.clone(), st.undo_depth());
    assert_eq!(st.verb("dur 7"), Some(Ok(())));
    let want = tl0.add_transition(0, 20, TransitionKind::Dissolve, 7).unwrap();
    assert_eq!(st.timeline, want, "the add op at the same cut, the ease kept");
    assert_eq!(st.timeline.tracks[0].transitions[0].ease, timeline_core::Ease::In);
    assert_eq!((st.transition, st.undo_depth()), (Some((0, 0)), depth + 1), "one undo step, the selection kept");
    assert_eq!(st.verb("dur 7"), Some(Ok(())));
    assert_eq!(st.undo_depth(), depth + 1, "the same length commits nothing");
    let text = edl::write(&st.timeline, "SR870").unwrap();
    assert!(text.contains(" D    007 "), "{text}");
    assert_eq!(edl::parse(&text, 25).unwrap().tracks[0].transitions, st.timeline.tracks[0].transitions);
    assert!(st.undo());
    assert_eq!(st.timeline, tl0);
    // from black: the length change keeps `from_black`
    let fb = cuts().add_transition_from_black(0, 0, TransitionKind::Dissolve, 4).unwrap();
    let mut st = timeline_core::editor::EditorState::new(fb.clone());
    st.select_transition(0, 0).unwrap();
    assert_eq!(st.verb("dur 6"), Some(Ok(())));
    assert_eq!(st.timeline, fb.add_transition_from_black(0, 0, TransitionKind::Dissolve, 6).unwrap());
    assert!(st.timeline.tracks[0].transitions[0].from_black);
}

#[test]
fn striatransitionlength_dur_refusals_by_name() {
    let mut none = timeline_core::editor::EditorState::new(cuts().add_transition(0, 20, TransitionKind::Dissolve, 4).unwrap());
    let before = none.clone();
    assert_eq!(none.verb("dur 7"), Some(Err(EditError::Transition)), "no transition selected");
    assert_eq!(none, before);
    let mut st = striatransitionlength_state();
    for (v, e) in [
        ("dur", EditError::Transition),
        ("dur x", EditError::Transition),
        ("dur 7 8", EditError::Transition),
        ("dur 0", EditError::Transition),
        ("dur -3", EditError::Transition),
        ("dur 21", EditError::Handle),
    ] {
        let before = st.clone();
        assert_eq!(st.verb(v), Some(Err(e)), "{v}");
        assert_eq!(st, before, "{v}");
    }
    assert_eq!(st.verb("durx 3"), None);
    assert_eq!(st.verb("play"), None);
}

// ---- STRIATRANSITIONMORE (LEDGER SR956): `dur` reads a timecode count (named tail block) ----

#[test]
fn striatransitionmore_dur_timecode() {
    let mut st = striatransitionlength_state();
    let (tl0, depth) = (st.timeline.clone(), st.undo_depth());
    assert_eq!(st.verb("dur 00:00:00:07"), Some(Ok(())));
    let mut by_ticks = striatransitionlength_state();
    assert_eq!(by_ticks.verb("dur 7"), Some(Ok(())));
    assert_eq!(st.timeline, by_ticks.timeline, "a timecode count is the same length as the ticks");
    assert_eq!(st.undo_depth(), depth + 1, "one undo step");
    assert_eq!(st.verb("dur 00:00:00:07"), Some(Ok(())));
    assert_eq!(st.undo_depth(), depth + 1, "the same length by timecode commits nothing");
    for (v, e) in [
        ("dur 00:00:00:25", EditError::Transition),
        ("dur 00;00;00;07", EditError::Transition),
        ("dur 0:7", EditError::Transition),
        ("dur 00:00:00:00", EditError::Transition),
        ("dur 00:00:01:00", EditError::Handle),
    ] {
        let before = st.clone();
        assert_eq!(st.verb(v), Some(Err(e)), "{v}");
        assert_eq!(st, before, "{v}");
    }
    assert!(st.undo());
    assert_eq!(st.timeline, tl0);
    // a time base that is not 1/fps: ticks only
    let mut tl = cuts().add_transition(0, 20, TransitionKind::Dissolve, 4).unwrap();
    tl.tb.num = 1001;
    let mut st = timeline_core::editor::EditorState::new(tl);
    st.select_transition(0, 0).unwrap();
    assert_eq!(st.verb("dur 00:00:00:07"), Some(Err(EditError::Transition)));
    assert_eq!(st.verb("dur 7"), Some(Ok(())));
    assert_eq!(st.timeline.tracks[0].transitions[0].dur, 7);
}

// ---- STRIATRANSITIONNEXT (LEDGER SR991): `dur` short timecode forms (named tail block) ----

#[test]
fn striatransitionnext_dur_short_timecode() {
    let mut st = striatransitionlength_state();
    let (tl0, depth) = (st.timeline.clone(), st.undo_depth());
    assert_eq!(st.verb("dur 00:07"), Some(Ok(())));
    let mut by_ticks = striatransitionlength_state();
    assert_eq!(by_ticks.verb("dur 7"), Some(Ok(())));
    assert_eq!(st.timeline, by_ticks.timeline, "`SS:FF` is the same length as the ticks");
    assert_eq!(st.undo_depth(), depth + 1, "one undo step");
    assert_eq!(st.verb("dur 00:00:07"), Some(Ok(())));
    assert_eq!(st.undo_depth(), depth + 1, "the same length by `MM:SS:FF` commits nothing");
    assert_eq!(st.verb("dur 00:09"), Some(Ok(())));
    assert_eq!(st.timeline.tracks[0].transitions[0].dur, 9);
    for (v, e) in [
        ("dur 0:7", EditError::Transition),
        ("dur 1:00:07", EditError::Transition),
        ("dur 00:25", EditError::Transition),
        ("dur 60:00", EditError::Transition),
        ("dur 00;07", EditError::Transition),
        ("dur 00:00", EditError::Transition),
        ("dur 007:01", EditError::Transition),
        ("dur 01:00", EditError::Handle),
    ] {
        let before = st.clone();
        assert_eq!(st.verb(v), Some(Err(e)), "{v}");
        assert_eq!(st, before, "{v}");
    }
    assert!(st.undo() && st.undo());
    assert_eq!(st.timeline, tl0);
    // a time base that is not 1/fps: ticks only
    let mut tl = cuts().add_transition(0, 20, TransitionKind::Dissolve, 4).unwrap();
    tl.tb.num = 1001;
    let mut st = timeline_core::editor::EditorState::new(tl);
    st.select_transition(0, 0).unwrap();
    assert_eq!(st.verb("dur 00:07"), Some(Err(EditError::Transition)));
}

// ---- STRIAKSLIP (LEDGER SR9002): THE dispatch takes the faces' slip / slide step words (named tail block) ----
// The kernel Player's `,` / `.` keys name `slip-` / `slip+` and hand them to `EditorState::verb` as the host face does:
// each word is its `slip <d>` / `slide <d>` twin (same result, same timeline, same undo depth); any other count is not a
// word. The Player side itself is unexercised here (NLE3E-SLIP-KERNEL-WITNESS).

#[test]
fn striakslip_the_dispatch_takes_the_face_step_words() {
    use timeline_core::editor::EditorState;
    let fresh = || {
        let mut st = EditorState::new(three());
        st.select(0, 1).unwrap();
        st
    };
    for (word, twin) in [("slip+", "slip 1"), ("slip-", "slip -1"), ("slide+", "slide 1"), ("slide-", "slide -1"), ("slip+10", "slip 10"), ("slip-10", "slip -10"), ("slide+10", "slide 10"), ("slide-10", "slide -10")] {
        let (mut a, mut b) = (fresh(), fresh());
        let r = a.verb(word);
        assert!(r.is_some(), "{word} is a word of THE dispatch");
        assert_eq!(r, b.verb(twin), "{word} answers as {twin}");
        assert_eq!((a.timeline.clone(), a.undo_depth()), (b.timeline.clone(), b.undo_depth()), "{word} edits as {twin}");
        assert!(a.undo_depth() <= 1, "{word}: at most one undo step");
    }
    let mut st = fresh();
    for v in ["slip+7", "slip+10x", "slide-010x", "slip10", "slip", "slide*"] {
        assert_eq!(st.verb(v), None, "{v} is not a word");
    }
    assert_eq!((st.timeline.clone(), st.undo_depth()), (three(), 0));
    st.deselect();
    assert_eq!(st.verb("slip+"), Some(Err(EditError::NoClip)));
    assert_eq!((st.timeline.clone(), st.undo_depth()), (three(), 0));
}

#[test]
fn striakslip_slipslide_word_maps_the_words() {
    use timeline_core::editor::{SLIP_STEP, slipslide_word};
    assert_eq!(SLIP_STEP, 10);
    for (w, want) in [("slip-", "slip -1"), ("slip+", "slip 1"), ("slide-", "slide -1"), ("slide+", "slide 1"), ("slip-10", "slip -10"), ("slide+10", "slide 10")] {
        assert_eq!(slipslide_word(w).as_deref(), Some(want), "{w}");
    }
    for w in ["slip", "slip 1", "slip+7", "slide-010x", "slipx", "nudge+"] {
        assert_eq!(slipslide_word(w), None, "{w}");
    }
}
