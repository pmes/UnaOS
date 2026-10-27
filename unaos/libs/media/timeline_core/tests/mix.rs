// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! NLE4 (SR203): per-clip gain and fades (validated ops, the EDL comment lines) and the audio mix runs.

use timeline_core::edl;
use timeline_core::schedule::{ENV_ONE, audio_runs, schedule};
use timeline_core::{Clip, EditError, Kind, SourceId, SourceIndex, Timeline, TransitionKind};

const NS: i64 = 40_000_000; // one tick at 25 fps

struct Gop;
impl SourceIndex for Gop {
    fn duration(&self, _: SourceId) -> Option<i64> {
        Some(50)
    }
    fn keyframe_ns(&self, _: SourceId, _: Kind, t_ns: i64) -> Option<i64> {
        Some(t_ns / NS * NS)
    }
}

/// A1: A[0,20)@0; A2: B[0,10)@5.
fn two_tracks() -> Timeline {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Audio);
    tl.add_track(Kind::Audio);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 20, 0)];
    tl.tracks[1].clips = vec![Clip::new(b, 0, 10, 5)];
    tl
}

#[test]
fn gain_and_fades_are_validated() {
    let tl = two_tracks();
    let g = tl.set_gain(0, 0, -602).unwrap();
    assert_eq!(g.tracks[0].clips[0].gain_cdb, -602);
    assert_eq!(tl.set_gain(0, 0, 2_401).map(|_| ()), Err(EditError::Gain));
    assert_eq!(tl.set_gain(0, 0, -14_401).map(|_| ()), Err(EditError::Gain));
    let f = tl.set_fades(0, 0, 5, 15).unwrap(); // exactly the clip's 20 ticks
    assert_eq!((f.tracks[0].clips[0].fade_in, f.tracks[0].clips[0].fade_out), (5, 15));
    assert_eq!(tl.set_fades(0, 0, 5, 16).map(|_| ()), Err(EditError::Fade));
    assert_eq!(tl.set_fades(0, 0, -1, 0).map(|_| ()), Err(EditError::Fade));
    assert_eq!(tl.set_gain(0, 3, 0).map(|_| ()), Err(EditError::NoClip));
    // a trim that would leave the fades longer than the clip is refused
    assert_eq!(f.trim_out(0, 0, 19).map(|_| ()), Err(EditError::Fade));
}

#[test]
fn a_split_keeps_the_gain_and_clamps_the_fades() {
    let tl = two_tracks().set_gain(0, 0, -300).unwrap().set_fades(0, 0, 8, 6).unwrap();
    let s = tl.split(0, 4).unwrap();
    let (h, t) = (s.tracks[0].clips[0], s.tracks[0].clips[1]);
    assert_eq!((h.at, h.len(), h.gain_cdb, h.fade_in, h.fade_out), (0, 4, -300, 4, 0));
    assert_eq!((t.at, t.len(), t.gain_cdb, t.fade_in, t.fade_out), (4, 16, -300, 0, 6));
}

#[test]
fn the_edl_carries_gain_and_fades_only_when_set() {
    let plain = two_tracks();
    let text = edl::write(&plain, "MIX").unwrap();
    assert!(!text.contains("UNA GAIN") && !text.contains("UNA FADE"));
    let tl = plain.set_gain(1, 0, -602).unwrap().set_fades(0, 0, 10, 0).unwrap().set_fades(1, 0, 0, 3).unwrap();
    let text = edl::write(&tl, "MIX").unwrap();
    assert_eq!(
        text,
        "TITLE: MIX
FCM: NON-DROP FRAME
* UNA FPS 25
* UNA SOURCE A 00:00:02:00
* UNA SOURCE B 00:00:02:00

001  A        A     C        00:00:00:00 00:00:00:20 00:00:00:00 00:00:00:20
* UNA FADE 00:00:00:10 00:00:00:00
002  B        A2    C        00:00:00:00 00:00:00:10 00:00:00:05 00:00:00:15
* UNA GAIN -602
* UNA FADE 00:00:00:00 00:00:00:03
"
    );
}

#[test]
fn the_edl_round_trips_gain_and_fades() {
    let tl = two_tracks().set_gain(1, 0, -602).unwrap().set_fades(0, 0, 10, 0).unwrap().set_fades(1, 0, 0, 3).unwrap();
    let text = edl::write(&tl, "MIX").unwrap();
    let back = edl::parse(&text, 30).unwrap();
    assert_eq!(back, tl);
    assert_eq!(edl::write(&back, "MIX").unwrap(), text);
    // an orphan line (no event above, or under a BL event) is refused by name
    let orphan = "FCM: NON-DROP FRAME\n* UNA GAIN -100\n";
    assert_eq!(edl::parse(orphan, 25), Err(edl::EdlError::Syntax { line: 2 }));
    let under_black = "001  BL       A     C        00:00:00:00 00:00:00:10 00:00:00:00 00:00:00:10\n* UNA GAIN -100\n";
    assert_eq!(edl::parse(under_black, 25), Err(edl::EdlError::Syntax { line: 2 }));
    assert_eq!(edl::parse(&text.replace("GAIN -602", "GAIN x"), 25), Err(edl::EdlError::Syntax { line: 10 }));
}

#[test]
fn overlapping_tracks_mix_in_one_run() {
    let tl = two_tracks();
    let runs = audio_runs(&tl, 0..20, &Gop).unwrap();
    // 0..5 A alone, 5..15 A + B, 15..20 A alone again
    assert_eq!(runs.iter().map(|r| (r.out_t, r.len, r.layers.len())).collect::<Vec<_>>(), vec![(0, 5, 1), (5, 10, 2), (15, 5, 1)]);
    let both = &runs[1];
    assert_eq!((both.layers[0].track, both.layers[0].src, both.layers[0].src_t, both.layers[0].seek), (0, SourceId(0), 5, None));
    assert_eq!((both.layers[1].track, both.layers[1].src, both.layers[1].src_t, both.layers[1].seek), (1, SourceId(1), 0, Some(0)));
    assert!(both.layers.iter().all(|l| l.env == [ENV_ONE, ENV_ONE] && l.gain_cdb == 0));
    // A1 decodes on under A2: its third run continues without a seek
    assert_eq!((runs[2].layers[0].src_t, runs[2].layers[0].seek), (15, None));
    // the schedule's audio steps are the runs' top layers
    let steps: Vec<_> = schedule(&tl, 0..20, &Gop).unwrap().into_iter().filter(|s| s.kind == Kind::Audio).collect();
    assert_eq!(steps.iter().map(|s| (s.out_t, s.len, s.track)).collect::<Vec<_>>(), vec![(0, 5, Some(0)), (5, 10, Some(1)), (15, 5, Some(0))]);
}

#[test]
fn a_fade_breaks_the_run_at_its_knee() {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    tl.add_track(Kind::Audio);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 20, 0).faded(10, 4).gained(-602)];
    let runs = audio_runs(&tl, 0..20, &Gop).unwrap();
    let env: Vec<_> = runs.iter().map(|r| (r.out_t, r.len, r.layers[0].env)).collect();
    assert_eq!(env, vec![(0, 10, [0, ENV_ONE]), (10, 6, [ENV_ONE, ENV_ONE]), (16, 4, [ENV_ONE, 0])]);
    assert!(runs.iter().all(|r| r.layers[0].gain_cdb == -602));
    // a range starting mid-fade begins its envelope there
    let mid = audio_runs(&tl, 5..10, &Gop).unwrap();
    assert_eq!(mid[0].layers[0].env, [ENV_ONE / 2, ENV_ONE]);
}

#[test]
fn a_gap_is_a_run_with_no_layers() {
    let mut tl = two_tracks();
    tl.tracks[0].clips = vec![Clip::new(SourceId(0), 0, 3, 0)];
    let runs = audio_runs(&tl, 0..20, &Gop).unwrap();
    assert_eq!(runs.iter().map(|r| (r.out_t, r.len, r.layers.len())).collect::<Vec<_>>(), vec![(0, 3, 1), (3, 2, 0), (5, 10, 1), (15, 5, 0)]);
    let gap = schedule(&tl, 3..5, &Gop).unwrap();
    assert_eq!((gap[0].src, gap[0].len), (None, 2));
}

// ---- NLE3b (SR209): a video transition implies the audio crossfade on the paired audio cut.

/// V1 and A1: A[10,30)@0, B[0,40)@20, a 9-tick dissolve on V1's cut at 20.
fn xfade() -> Timeline {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Video);
    tl.add_track(Kind::Audio);
    for t in 0..2 {
        tl.tracks[t].clips = vec![Clip::new(a, 10, 30, 0), Clip::new(b, 0, 40, 20)];
    }
    tl.add_transition(0, 20, TransitionKind::Dissolve, 9).unwrap()
}

#[test]
fn nle3b_a_dissolve_crossfades_the_paired_audio() {
    let tl = xfade();
    assert_eq!(tl.audio_crossfade(1, 1).map(|x| (x.at, x.dur)), Some((20, 9)));
    assert_eq!(tl.audio_crossfade(0, 1), None); // a video track
    let runs = audio_runs(&tl, 0..60, &Gop).unwrap();
    assert_eq!(runs.iter().map(|r| (r.out_t, r.len, r.layers.len())).collect::<Vec<_>>(), vec![(0, 20, 1), (20, 9, 2), (29, 31, 1)]);
    let (under, inc) = (runs[1].layers[0], runs[1].layers[1]);
    // the outgoing A at its handle (30..), seeking on its own; the incoming B at its start, seeking at the cut
    assert_eq!((under.track, under.src, under.src_t, under.seek, under.env), (1, SourceId(0), 30, Some(30 * NS), [ENV_ONE, 0]));
    assert_eq!((inc.track, inc.src, inc.src_t, inc.seek, inc.env), (1, SourceId(1), 0, Some(0), [0, ENV_ONE]));
    // after the span B continues alone at full
    assert_eq!((runs[2].layers[0].src_t, runs[2].layers[0].seek, runs[2].layers[0].env), (9, None, [ENV_ONE, ENV_ONE]));
    // mid-span the two weights still sum to full scale at every edge
    let mid = audio_runs(&tl, 23..27, &Gop).unwrap();
    assert_eq!(mid[0].layers[0].env[0] + mid[0].layers[1].env[0], ENV_ONE);
    assert_eq!(mid[0].layers[0].env[1] + mid[0].layers[1].env[1], ENV_ONE);
    // the schedule's audio step is the incoming (the top layer)
    let steps: Vec<_> = schedule(&tl, 0..60, &Gop).unwrap().into_iter().filter(|s| s.kind == Kind::Audio).collect();
    assert_eq!(steps[1].src, Some(SourceId(1)));
}

#[test]
fn nle3b_the_pairing_rule_and_the_audio_handle() {
    // the audio's outgoing leaves 5 ticks of A past its out point: a 9-tick dissolve is refused by name
    let tl = xfade();
    let mut short = tl.remove_transition(0, 20).unwrap();
    short.tracks[1].clips[0] = Clip::new(SourceId(0), 25, 45, 0);
    assert_eq!(short.add_transition(0, 20, TransitionKind::Dissolve, 9), Err(EditError::Handle));
    assert!(short.add_transition(0, 20, TransitionKind::Dissolve, 5).is_ok());
    // an audio clip of another source at the cut: a plain audio cut
    let mut other = tl.clone();
    other.tracks[1].clips[1] = Clip::new(SourceId(0), 0, 40, 20);
    assert_eq!(other.audio_crossfade(1, 1), None);
    assert!(audio_runs(&other, 0..60, &Gop).unwrap().iter().all(|r| r.layers.len() == 1));
}
