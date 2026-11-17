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

// ---- NLEXFADEPOWER (SR678): the equal-power curve, the explicit audio-only crossfade and the BL transitions' audio.

use timeline_core::schedule::xfade_weight;
use timeline_core::{Crossfade, Curve, Partner};

#[test]
fn nlexfadepower_equal_power_weights_keep_the_power_and_linear_is_unchanged() {
    for p in [0, ENV_ONE / 4, ENV_ONE / 2, 3 * ENV_ONE / 4, ENV_ONE] {
        let (w_in, w_out) = (xfade_weight(Curve::EqualPower, p) as u64, xfade_weight(Curve::EqualPower, ENV_ONE - p) as u64);
        let sum = (w_in * w_in + w_out * w_out) as f64 / 65536.0;
        assert!((sum - 65536.0).abs() <= 1.0, "p {p}: {w_in}^2 + {w_out}^2 = {sum} x 65536");
    }
    // the table's own points: sin 0, sin pi/8, sin pi/4, sin pi/2
    let eq = |p| xfade_weight(Curve::EqualPower, p);
    assert_eq!((eq(0), eq(ENV_ONE / 4), eq(ENV_ONE / 2), eq(ENV_ONE)), (0, 25080, 46341, ENV_ONE));
    // linear is NLE3b's ramp itself
    for p in [0, 1, 12_345, ENV_ONE / 2, ENV_ONE - 1, ENV_ONE] {
        assert_eq!(xfade_weight(Curve::Linear, p), p);
    }
}

/// A1 only: A[10,30)@0, B[0,40)@20 and an explicit 8-tick crossfade on the cut at 20 (no video transition anywhere).
fn audio_only(curve: Curve) -> Timeline {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Audio);
    tl.tracks[0].clips = vec![Clip::new(a, 10, 30, 0), Clip::new(b, 0, 40, 20)];
    tl.add_audio_xfade(0, 20, 8, curve).unwrap()
}

#[test]
fn nlexfadepower_an_audio_only_crossfade_mixes_like_the_paired_one() {
    // linear: NLE3b's three runs and edges exactly
    let lin = audio_only(Curve::Linear);
    assert_eq!(lin.audio_crossfade(0, 1), None); // nothing derived: no video
    assert_eq!(lin.crossfade_at(0, 1), Some(Crossfade { at: 20, dur: 8, curve: Curve::Linear, partner: Partner::Clip }));
    let runs = audio_runs(&lin, 0..60, &Gop).unwrap();
    assert_eq!(runs.iter().map(|r| (r.out_t, r.len, r.layers.len())).collect::<Vec<_>>(), vec![(0, 20, 1), (20, 8, 2), (28, 32, 1)]);
    let (under, inc) = (runs[1].layers[0], runs[1].layers[1]);
    assert_eq!((under.src, under.src_t, under.seek, under.env), (SourceId(0), 30, Some(30 * NS), [ENV_ONE, 0]));
    assert_eq!((inc.src, inc.src_t, inc.seek, inc.env), (SourceId(1), 0, Some(0), [0, ENV_ONE]));
    // equal power: one run per tick, every edge the sine / cosine weight, every run after the first CONTINUING (no seek)
    let eq = audio_only(Curve::EqualPower);
    let runs = audio_runs(&eq, 0..60, &Gop).unwrap();
    assert_eq!(runs.len(), 10);
    assert_eq!((runs[0].out_t, runs[0].len, runs[9].out_t, runs[9].len), (0, 20, 28, 32));
    let w = |k: i64| xfade_weight(Curve::EqualPower, (k * ENV_ONE as i64 / 8) as u32);
    let wf = |k: i64| xfade_weight(Curve::EqualPower, ENV_ONE - (k * ENV_ONE as i64 / 8) as u32);
    for k in 0..8i64 {
        let r = &runs[1 + k as usize];
        assert_eq!((r.out_t, r.len, r.layers.len(), r.under(0)), (20 + k, 1, 2, true), "tick {}", 20 + k);
        let (under, inc) = (r.layers[0], r.layers[1]);
        let seeks = if k == 0 { (Some(30 * NS), Some(0)) } else { (None, None) };
        assert_eq!((under.src_t, inc.src_t, under.seek, inc.seek), (30 + k, k, seeks.0, seeks.1), "tick {}", 20 + k);
        assert_eq!((under.env, inc.env), ([wf(k), wf(k + 1)], [w(k), w(k + 1)]), "tick {}", 20 + k);
    }
    // the midpoint edge: both at sin(pi/4), the power kept (vs the linear 32768 + 32768)
    assert_eq!((runs[5].layers[0].env[0], runs[5].layers[1].env[0]), (46341, 46341));
    assert_eq!((runs[9].layers[0].src_t, runs[9].layers[0].seek, runs[9].layers[0].env), (8, None, [ENV_ONE, ENV_ONE]));
}

#[test]
fn nlexfadepower_the_explicit_record_is_validated_and_follows_its_cut() {
    use timeline_core::Mode;
    let tl = audio_only(Curve::Linear);
    let base = tl.remove_audio_xfade(0, 20).unwrap();
    assert!(base.audio_xfades.is_empty());
    assert_eq!(base.remove_audio_xfade(0, 20), Err(EditError::Transition));
    // off a cut, zero length, no such track, longer than the outgoing's handle (50 - 30 = 20)
    assert_eq!(base.add_audio_xfade(0, 10, 4, Curve::Linear), Err(EditError::Transition));
    assert_eq!(base.add_audio_xfade(0, 20, 0, Curve::Linear), Err(EditError::Transition));
    assert_eq!(base.add_audio_xfade(3, 20, 4, Curve::Linear), Err(EditError::NoTrack));
    assert_eq!(base.add_audio_xfade(0, 20, 21, Curve::Linear), Err(EditError::Handle));
    assert!(base.add_audio_xfade(0, 20, 20, Curve::Linear).is_ok());
    // on a video track
    let mut v = base.clone();
    let v1 = v.add_track(Kind::Video);
    v.tracks[v1].clips = v.tracks[0].clips.clone();
    assert_eq!(v.add_audio_xfade(v1, 20, 4, Curve::Linear), Err(EditError::Transition));
    // an insert before the cut moves the record with it; deleting its outgoing clip is refused by name
    let ins = tl.place(0, Clip::new(SourceId(1), 40, 50, 0), Mode::Insert).unwrap();
    assert_eq!((ins.audio_xfades[0].at, ins.audio_xfades[0].dur), (30, 8));
    assert_eq!(tl.lift(0, 0), Err(EditError::Transition));
    assert_eq!(tl.ripple_delete(0, 1), Err(EditError::Transition));
}

#[test]
fn nlexfadepower_a_bl_transition_fades_its_audio_from_and_to_silence() {
    // from black: V1 and A1 B[0,20)@10, a 5-tick dissolve from black: the audio rises from silence, one layer
    let mut fb = Timeline::new(25);
    let b = fb.add_source("B", 50);
    fb.add_track(Kind::Video);
    fb.add_track(Kind::Audio);
    for t in 0..2 {
        fb.tracks[t].clips = vec![Clip::new(b, 0, 20, 10)];
    }
    let fb = fb.add_transition_from_black(0, 10, TransitionKind::Dissolve, 5).unwrap();
    assert_eq!(fb.crossfade_at(1, 0), Some(Crossfade { at: 10, dur: 5, curve: Curve::Linear, partner: Partner::Silence }));
    let runs = audio_runs(&fb, 0..30, &Gop).unwrap();
    assert_eq!(runs.iter().map(|r| (r.out_t, r.len, r.layers.len())).collect::<Vec<_>>(), vec![(0, 10, 0), (10, 5, 1), (15, 15, 1)]);
    assert_eq!((runs[1].layers[0].src_t, runs[1].layers[0].env, runs[2].layers[0].env), (0, [0, ENV_ONE], [ENV_ONE, ENV_ONE]));
    // the audio clip shorter than the fade is refused like NLE3b's short handle
    let mut short = fb.clone();
    short.tracks[1].clips = vec![Clip::new(b, 0, 3, 10)];
    assert_eq!(short.validate(), Err(EditError::Handle));
    // to black: V1 and A1 A[0,20)@0, a 5-tick dissolve to black: A's handle plays on in the gap, falling, continuing
    let mut tb = Timeline::new(25);
    let a = tb.add_source("A", 50);
    tb.add_track(Kind::Video);
    tb.add_track(Kind::Audio);
    for t in 0..2 {
        tb.tracks[t].clips = vec![Clip::new(a, 0, 20, 0)];
    }
    let tb = tb.add_transition_to_black(0, 20, TransitionKind::Dissolve, 5).unwrap();
    assert_eq!(tb.fade_to_silence(1, 0), Some(Crossfade { at: 20, dur: 5, curve: Curve::Linear, partner: Partner::Silence }));
    let runs = audio_runs(&tb, 0..30, &Gop).unwrap();
    assert_eq!(runs.iter().map(|r| (r.out_t, r.len, r.layers.len())).collect::<Vec<_>>(), vec![(0, 20, 1), (20, 5, 1), (25, 5, 0)]);
    let g = runs[1].layers[0];
    assert_eq!((g.track, g.src, g.src_t, g.seek, g.env, runs[1].under(0)), (1, SourceId(0), 20, None, [ENV_ONE, 0], false));
    // the audio's own handle must hold the fade
    let mut nohandle = tb.clone();
    nohandle.tracks[1].clips = vec![Clip::new(a, 30, 50, 0)];
    assert_eq!(nohandle.validate(), Err(EditError::Handle));
    // a BL fade's audio is derived: the EDL writes as before and reads back the same timeline
    for tl in [&fb, &tb] {
        let text = edl::write(tl, "BL").unwrap();
        assert!(!text.contains("XFADE") && !text.contains(" A     D"), "{text}");
        assert_eq!(&edl::parse(&text, 25).unwrap(), tl);
    }
}

// ---- STRIAPAN (LEDGER SR861): per-clip pan — the law, the layers, the edit road, the EDL line (named tail block) ----
use timeline_core::editor::EditorState;
use timeline_core::schedule::pan_q16;

/// THE law: hard left, centre (-3.01 dB each side), hard right, and the power kept at every pan.
#[test]
fn striapan_law_hard_left_centre_hard_right() {
    assert_eq!(pan_q16(-128), [ENV_ONE, 0]);
    assert_eq!(pan_q16(-127), [ENV_ONE, 0]);
    assert_eq!(pan_q16(0), [46341, 46341]);
    assert_eq!(pan_q16(127), [0, ENV_ONE]);
    let db = 20.0 * (46341.0f64 / 65536.0).log10();
    assert!((db + 3.0103).abs() < 0.001, "centre {db} dB");
    for p in -128i32..=127 {
        let [l, r] = pan_q16(p as i8).map(|w| w as u64);
        let sum = (l * l + r * r) as f64 / 65536.0;
        assert!((sum - 65536.0).abs() <= 4.0, "pan {p}: {l}^2 + {r}^2 = {sum} x 65536");
        if p > -127 {
            assert!(r >= pan_q16((p - 1) as i8)[1] as u64, "pan {p}: the right weight rises");
        }
    }
    println!("striapan law: -128 {:?} 0 {:?} 127 {:?} (centre {db:.4} dB)", pan_q16(-128), pan_q16(0), pan_q16(127));
}

/// `audio_runs` copies each clip's pan onto its layer, the crossfade's under layer the outgoing clip's.
#[test]
fn striapan_layers_carry_the_clip_pan() {
    let tl = two_tracks().set_pan(0, 0, -64).unwrap().set_pan(1, 0, 100).unwrap();
    let runs = audio_runs(&tl, 0..20, &Gop).unwrap();
    for r in &runs {
        for l in &r.layers {
            assert_eq!(l.pan_cq8, if l.track == 0 { -64 } else { 100 });
        }
    }
    assert_eq!(audio_runs(&two_tracks(), 0..20, &Gop).unwrap().iter().flat_map(|r| r.layers.iter()).filter(|l| l.pan_cq8 != 0).count(), 0);
    // pan does not break runs: the same runs as the centred timeline
    let shape = |tl: &Timeline| audio_runs(tl, 0..20, &Gop).unwrap().iter().map(|r| (r.out_t, r.len, r.layers.len())).collect::<Vec<_>>();
    assert_eq!(shape(&tl), shape(&two_tracks()));
    let x = audio_only(Curve::Linear).set_pan(0, 0, -127).unwrap().set_pan(0, 1, 127).unwrap();
    let runs = audio_runs(&x, 0..60, &Gop).unwrap();
    assert_eq!((runs[1].layers[0].src, runs[1].layers[0].pan_cq8), (SourceId(0), -127), "the under layer: the outgoing's pan");
    assert_eq!((runs[1].layers[1].src, runs[1].layers[1].pan_cq8), (SourceId(1), 127));
}

/// `set_pan` sets one clip; only the clip's address is refused; a split keeps the pan on both halves.
#[test]
fn striapan_set_pan_and_split() {
    let tl = two_tracks();
    let p = tl.set_pan(0, 0, -128).unwrap();
    assert_eq!(p.tracks[0].clips[0].pan_cq8, -128);
    assert_eq!(p.tracks[1].clips[0].pan_cq8, 0);
    assert_eq!(tl.set_pan(0, 3, 1).map(|_| ()), Err(EditError::NoClip));
    assert_eq!(tl.set_pan(9, 0, 1).map(|_| ()), Err(EditError::NoTrack));
    let s = tl.set_pan(0, 0, 40).unwrap().split(0, 4).unwrap();
    assert_eq!((s.tracks[0].clips[0].pan_cq8, s.tracks[0].clips[1].pan_cq8), (40, 40));
    assert_eq!(Clip::new(SourceId(0), 0, 1, 0).pan_cq8, 0);
}

/// The `pan <x>` word: one commit, one undo step, the same pan commits nothing, NoClip with nothing selected, a malformed
/// word is not the verb.
#[test]
fn striapan_verb_one_undo_step_and_refusals() {
    let mut st = EditorState::new(two_tracks());
    assert_eq!(st.verb("pan 0.5"), Some(Err(EditError::NoClip)));
    st.select(1, 0).unwrap();
    let d0 = st.undo_depth();
    assert_eq!(st.verb("pan -0.5"), Some(Ok(())));
    assert_eq!(st.timeline.tracks[1].clips[0].pan_cq8, -64); // round(-63.5) away from zero
    assert_eq!(st.undo_depth(), d0 + 1);
    assert_eq!(st.verb("pan -0.5"), Some(Ok(())));
    assert_eq!(st.undo_depth(), d0 + 1, "the same pan commits nothing");
    assert_eq!(st.verb("pan 1"), Some(Ok(())));
    assert_eq!(st.timeline.tracks[1].clips[0].pan_cq8, 127);
    assert_eq!(st.verb("pan -1"), Some(Ok(())));
    assert_eq!(st.timeline.tracks[1].clips[0].pan_cq8, -127);
    assert_eq!(st.verb("pan 0"), Some(Ok(())));
    assert_eq!(st.timeline, two_tracks());
    assert!(st.undo());
    assert_eq!(st.timeline.tracks[1].clips[0].pan_cq8, -127, "one undo step per pan");
    for bad in ["pan 2", "pan -1.01", "pan x", "pan 0.5 0.5", "pan NaN", "pan inf", "pans 0.5"] {
        assert_eq!(st.verb(bad), None, "{bad}");
    }
    assert_eq!(st.timeline.tracks[1].clips[0].pan_cq8, -127);
}

/// The EDL: `* UNA PAN <cq8>` under a panned event only, read back exactly; refused as `Syntax` with no event above or a
/// value that is not one i8.
#[test]
fn striapan_edl_round_trip_and_refusals() {
    assert!(!edl::write(&two_tracks(), "MIX").unwrap().contains("UNA PAN"));
    let tl = two_tracks().set_gain(1, 0, -602).unwrap().set_fades(1, 0, 0, 3).unwrap().set_pan(1, 0, -64).unwrap().set_pan(0, 0, 127).unwrap();
    let text = edl::write(&tl, "MIX").unwrap();
    assert!(text.contains("00:00:00:20\n* UNA PAN 127\n002"), "{text}");
    assert!(text.contains("* UNA GAIN -602\n* UNA FADE 00:00:00:00 00:00:00:03\n* UNA PAN -64\n"), "{text}");
    let back = edl::parse(&text, 25).unwrap();
    assert_eq!((back.tracks[0].clips[0].pan_cq8, back.tracks[1].clips[0].pan_cq8), (127, -64));
    assert_eq!(edl::write(&back, "MIX").unwrap(), text);
    let min = edl::write(&two_tracks().set_pan(0, 0, -128).unwrap(), "MIX").unwrap();
    assert_eq!(edl::parse(&min, 25).unwrap().tracks[0].clips[0].pan_cq8, -128);
    for (bad, with) in [("* UNA PAN 128", true), ("* UNA PAN x", true), ("* UNA PAN 1 2", true), ("* UNA PAN 5", false)] {
        let mut lines: Vec<&str> = text.lines().collect();
        let at = if with { lines.iter().position(|l| l.starts_with("001")).unwrap() + 1 } else { lines.iter().position(|l| l.starts_with("001")).unwrap() };
        lines.insert(at, bad);
        let t = lines.join("\n");
        assert_eq!(edl::parse(&t, 25).map(|_| ()), Err(edl::EdlError::Syntax { line: at + 1 }), "{bad}");
    }
}

// ---- STRIAPANFACE (LEDGER SR897): the face's pan words on the eighth grid, the pan tick on the body (named tail block) ----
use timeline_core::view::{self, HANDLE_W, Order, TICK, clip_colour};

/// `pan+` x8 walks the eighth grid to hard right (127), a 9th commits nothing; `pan-` steps back; bare `pan` centres; an
/// off-grid pan steps from its nearest grid point; each step is one undo step; nothing selected refuses `NoClip`.
#[test]
fn striapanface_step_words_on_the_eighth_grid() {
    let mut st = EditorState::new(two_tracks());
    for w in ["pan", "pan-", "pan+"] {
        assert_eq!(st.verb(w), Some(Err(EditError::NoClip)), "{w}");
    }
    st.select(1, 0).unwrap();
    let d0 = st.undo_depth();
    let grid = [16i8, 32, 48, 64, 79, 95, 111, 127];
    for (i, want) in grid.iter().enumerate() {
        assert_eq!(st.verb("pan+"), Some(Ok(())));
        assert_eq!(st.timeline.tracks[1].clips[0].pan_cq8, *want, "step {}", i + 1);
        assert_eq!(st.undo_depth(), d0 + i + 1, "one undo step per pan word");
    }
    assert_eq!(st.verb("pan+"), Some(Ok(())));
    assert_eq!((st.timeline.tracks[1].clips[0].pan_cq8, st.undo_depth()), (127, d0 + 8), "hard right: no commit");
    assert_eq!(st.verb("pan-"), Some(Ok(())));
    assert_eq!(st.timeline.tracks[1].clips[0].pan_cq8, 111);
    assert_eq!(st.verb("pan"), Some(Ok(())));
    assert_eq!(st.timeline, two_tracks(), "bare pan centres");
    assert_eq!(st.verb("pan 0.3"), Some(Ok(()))); // 38: nearest grid point 32 (k = 2)
    assert_eq!(st.verb("pan+"), Some(Ok(())));
    assert_eq!(st.timeline.tracks[1].clips[0].pan_cq8, 48);
    assert_eq!(st.verb("pan -1"), Some(Ok(())));
    assert_eq!(st.verb("pan-"), Some(Ok(())));
    assert_eq!(st.timeline.tracks[1].clips[0].pan_cq8, -127, "hard left stays");
    assert_eq!(st.verb("pan+"), Some(Ok(())));
    assert_eq!(st.timeline.tracks[1].clips[0].pan_cq8, -111);
    assert!(st.undo());
    assert_eq!(st.timeline.tracks[1].clips[0].pan_cq8, -127);
    println!("striapanface: pan+ grid {grid:?}");
}

/// The pan tick: a centred clip paints as before; a panned audio clip gains exactly the 2 x 4 px TICK mark at its pan
/// along the body's top edge; a panned VIDEO clip paints nothing new.
#[test]
fn striapanface_tick_paints_only_off_centre() {
    let (w, h) = (320u32, 160u32);
    let base = EditorState::new(two_tracks());
    let plain = view::paint(&base, w, h, Order::Rgba, None, None);
    let mut st = EditorState::new(two_tracks());
    st.select(0, 0).unwrap();
    assert_eq!(st.verb("pan"), Some(Ok(())));
    st.deselect();
    assert_eq!(view::paint(&st, w, h, Order::Rgba, None, None), plain, "pan 0 paints nothing");

    let panned = EditorState::new(two_tracks().set_pan(0, 0, 127).unwrap());
    let px = view::paint(&panned, w, h, Order::Rgba, None, None);
    let l = view::Layout::of(w, h, &panned);
    let (x0, x1, y0) = (l.x_of(0), l.x_of(20), l.row_y(0));
    let xm = (x0 + x1) / 2 + ((x1 - x0) / 2 - HANDLE_W - 1);
    let at = |x: i64, y: i64| &px[((y as usize) * w as usize + x as usize) * 4..][..4];
    assert_eq!(at(xm - 1, y0 + 2), TICK);
    assert_eq!(at(xm, y0 + 5), TICK);
    assert_eq!(at(xm + 1, y0 + 3), clip_colour(0, Kind::Audio));
    assert_eq!(at(xm, y0 + 6), clip_colour(0, Kind::Audio));
    let changed = px.chunks(4).zip(plain.chunks(4)).filter(|(a, b)| a != b).count();
    assert_eq!(changed, 8, "only the tick's 2 x 4 pixels");

    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 20, 0)];
    let v0 = view::paint(&EditorState::new(tl.clone()), w, h, Order::Rgba, None, None);
    let v1 = view::paint(&EditorState::new(tl.set_pan(0, 0, -100).unwrap()), w, h, Order::Rgba, None, None);
    assert_eq!(v0, v1, "no tick on a video clip");
    println!("striapanface: tick at x {xm}, y {}..{}", y0 + 2, y0 + 6);
}

// ---- STRIAPANMORE (LEDGER SR959): the pan on the whole selection set (named tail block) ----

/// A set of two audio clips takes `pan <x>` on both in ONE undo step; the same pan again commits nothing; `pan+` steps from
/// the primary's pan and the whole set takes it; undo restores both; a one-clip selection still pans one clip.
#[test]
fn striapanmore_set_pans_every_clip_in_one_step() {
    let mut st = EditorState::new(two_tracks());
    st.select(0, 0).unwrap();
    st.select_add(1, 0).unwrap();
    let d0 = st.undo_depth();
    let pans = |st: &EditorState| (st.timeline.tracks[0].clips[0].pan_cq8, st.timeline.tracks[1].clips[0].pan_cq8);
    assert_eq!(st.verb("pan 0.5"), Some(Ok(())));
    assert_eq!((pans(&st), st.undo_depth()), ((64, 64), d0 + 1));
    assert_eq!(st.verb("pan 0.5"), Some(Ok(())));
    assert_eq!(st.undo_depth(), d0 + 1, "the same pan on the whole set commits nothing");
    assert_eq!(st.verb("pan+"), Some(Ok(())));
    assert_eq!((pans(&st), st.undo_depth()), ((79, 79), d0 + 2));
    assert!(st.undo());
    assert_eq!(pans(&st), (64, 64), "one undo step for the whole set");
    st.select(1, 0).unwrap();
    assert_eq!(st.verb("pan -1"), Some(Ok(())));
    assert_eq!(pans(&st), (64, -127), "a set of one: only the selected clip");
    println!("striapanmore: set pan 0.5 -> {:?}", pans(&st));
}

// ---- STRIAPANNEXT (LEDGER SR994): the pan drag on the pan tick's strip (named tail block) ----

/// A press on the selected audio clip's top `PAN_GRIP_H` px between its handles begins a `Pan` drag; half the swing right reads
/// 64 (previewed by `shown`, committed ONCE by `finish` through the one pan road); a far drag clamps at -127; the row's middle
/// still moves, shift / slip at the grip do not pan, and a video clip's top is no grip.
#[test]
fn striapannext_pan_drag_on_the_grip() {
    use timeline_core::view::{Drag, DragKind, Layout, PAN_GRIP_H, pan_half};
    let (w, h) = (320u32, 160u32);
    let mut st = EditorState::new(two_tracks());
    st.select(0, 0).unwrap();
    let l = Layout::of(w, h, &st);
    let (x0, x1, y0) = (l.x_of(0), l.x_of(20), l.row_y(0));
    let half = pan_half(x0, x1).max(1) as f64;
    let (x, y) = ((x0 + x1) as f64 / 2.0, y0 as f64 + 2.0);
    assert!(y < (y0 + PAN_GRIP_H) as f64);
    let d0 = st.undo_depth();
    let mut d = Drag::begin_with(&mut st, l, x, y, false, false, false).expect("a drag");
    assert_eq!((d.kind, d.track, d.idx), (DragKind::Pan, 0, 0));
    d.update(&st, x + half / 2.0, y + 9.0);
    assert_eq!((d.delta, d.guide, d.to_track), (64, None, 0));
    assert!(d.moved());
    assert_eq!(d.shown(&st).map(|s| s.timeline.tracks[0].clips[0].pan_cq8), Some(64), "the preview pans");
    assert_eq!(st.timeline.tracks[0].clips[0].pan_cq8, 0, "the timeline is untouched until the release");
    assert_eq!(d.finish(&mut st), Ok(()));
    assert_eq!((st.timeline.tracks[0].clips[0].pan_cq8, st.undo_depth()), (64, d0 + 1));

    let mut far = Drag::begin_with(&mut st, l, x, y, false, false, false).expect("a drag");
    far.update(&st, x - 4.0 * half, y);
    assert_eq!(far.delta, -127 - 64, "clamped at hard left");
    assert_eq!(far.finish(&mut st), Ok(()));
    assert_eq!((st.timeline.tracks[0].clips[0].pan_cq8, st.undo_depth()), (-127, d0 + 2));

    let mid = y0 as f64 + 20.0;
    assert_eq!(Drag::begin_with(&mut st, l, x, mid, false, false, false).map(|d| d.kind), Some(DragKind::Move));
    st.select(0, 0).unwrap();
    assert_ne!(Drag::begin_with(&mut st, l, x, y, true, false, false).map(|d| d.kind), Some(DragKind::Pan), "shift adds, not pans");
    st.select(0, 0).unwrap();
    assert_eq!(Drag::begin_with(&mut st, l, x, y, false, false, true).map(|d| d.kind), Some(DragKind::Slip), "slip at the grip slips");

    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 20, 0)];
    let mut vs = EditorState::new(tl);
    vs.select(0, 0).unwrap();
    let vl = Layout::of(w, h, &vs);
    let vy = vl.row_y(0) as f64 + 2.0;
    assert_eq!(Drag::begin_with(&mut vs, vl, x, vy, false, false, false).map(|d| d.kind), Some(DragKind::Move), "no grip on a video clip");
    println!("striapannext: grip drag +{} px -> pan 64, far left -> -127", half / 2.0);
}

// ---- STRIAPANAUTOMATION (LEDGER SR9050): pan points on source ticks, the run split, the words and the EDL line (named tail block) ----
use timeline_core::{PAN_KEYS_MAX, PanKeys};

/// `PanKeys`: points stay sorted, a point at the same tick is replaced, the ninth is refused, `without` clears one and zeroes
/// the freed slot; `at` holds flat outside the points and is linear and rounded half away from zero between them.
#[test]
fn striapanautomation_pan_keys_at_with_without() {
    assert_eq!(PanKeys::EMPTY.at(5), None);
    let k = PanKeys::EMPTY.with(3, 10).unwrap().with(0, 0).unwrap();
    assert_eq!(k.points().to_vec(), vec![(0i64, 0i8), (3, 10)]);
    assert_eq!([k.at(-4), k.at(0), k.at(1), k.at(2), k.at(3), k.at(99)], [Some(0), Some(0), Some(3), Some(7), Some(10), Some(10)]);
    let m = PanKeys::EMPTY.with(0, 0).unwrap().with(3, -10).unwrap();
    assert_eq!((m.at(1), m.at(2)), (Some(-3), Some(-7)));
    let h = PanKeys::EMPTY.with(0, 0).unwrap().with(2, 1).unwrap();
    assert_eq!(h.at(1), Some(1), "half rounds away from zero");
    let h = PanKeys::EMPTY.with(0, 0).unwrap().with(2, -1).unwrap();
    assert_eq!(h.at(1), Some(-1));
    assert_eq!(k.with(3, -5).unwrap().points().to_vec(), vec![(0i64, 0i8), (3, -5)], "the same tick is replaced");
    let mut full = PanKeys::EMPTY;
    for i in 0..PAN_KEYS_MAX as i64 {
        full = full.with(10 - i, i as i8).unwrap();
    }
    assert_eq!(full.n as usize, PAN_KEYS_MAX);
    assert!(full.points().windows(2).all(|w| w[0].0 < w[1].0));
    assert_eq!(full.with(99, 1), None, "full");
    assert_eq!(full.with(10, 1).map(|f| f.n as usize), Some(PAN_KEYS_MAX), "a replace fits when full");
    assert_eq!(k.without(3).unwrap(), PanKeys::EMPTY.with(0, 0).unwrap(), "the freed slot is zero again");
    assert_eq!(k.without(1), None);
    println!("striapanautomation keys: {:?} at 1,2 = {:?},{:?}", k.points(), k.at(1), k.at(2));
}

/// `audio_runs`: a clip with points splits its runs wherever its pan changes and each layer carries the pan at both edges;
/// a flat stretch is one run; no points anywhere gives the unautomated runs with `pan_to == pan_cq8`; a split carries the
/// curve to both halves.
#[test]
fn striapanautomation_runs_split_at_the_pan_and_carry_both_edges() {
    let plain = audio_runs(&two_tracks(), 0..20, &Gop).unwrap();
    assert!(plain.iter().flat_map(|r| r.layers.iter()).all(|l| l.pan_to == l.pan_cq8));
    let tl = two_tracks().set_pan_key(0, 0, 0, -127).unwrap().set_pan_key(0, 0, 10, 127).unwrap();
    let c = tl.tracks[0].clips[0];
    assert_eq!([c.pan_at(0), c.pan_at(3), c.pan_at(5), c.pan_at(10), c.pan_at(19)], [-127, -51, 0, 127, 127]);
    let runs = audio_runs(&tl, 0..20, &Gop).unwrap();
    for r in &runs {
        let l = r.layers.iter().find(|l| l.track == 0).unwrap();
        assert_eq!((l.pan_cq8, l.pan_to), (c.pan_at(r.out_t), c.pan_at(r.out_t + r.len)), "run at {}", r.out_t);
        if r.out_t < 10 {
            assert_eq!(r.len, 1, "the ramp breaks every tick its pan changes");
        }
        let b = r.layers.iter().find(|l| l.track == 1);
        assert!(b.is_none_or(|b| (b.pan_cq8, b.pan_to) == (0, 0)), "a clip without points keeps its static pan");
    }
    assert_eq!(runs.iter().find(|r| r.out_t == 10).map(|r| r.len), Some(5), "flat 10..15 is one run");
    let s = tl.split(0, 4).unwrap();
    for t in 0..20 {
        let h = &s.tracks[0].clips[if t < 4 { 0 } else { 1 }];
        assert_eq!(h.pan_at(t), c.pan_at(t), "tick {t}");
    }
    assert_eq!(tl.set_pan_key(0, 3, 0, 1).map(|_| ()), Err(EditError::NoClip));
    assert_eq!(tl.clear_pan_key(0, 0, 4).map(|_| ()), Err(EditError::PanKey));
    println!("striapanautomation runs: {} (unautomated {})", runs.len(), plain.len());
}

/// `pan-key` / `pan-key <x>` / `pan-unkey`: a point at the playhead's source tick, one undo step each, the same value commits
/// nothing; refused `NoClip` with nothing selected or the playhead off the clip, `PanKey` full or nothing to clear; malformed
/// words are not the verb.
#[test]
fn striapanautomation_words_one_undo_step_and_refusals() {
    let mut st = EditorState::new(two_tracks());
    assert_eq!(st.verb("pan-key"), Some(Err(EditError::NoClip)));
    st.select(1, 0).unwrap();
    st.seek(2);
    assert_eq!(st.verb("pan-key"), Some(Err(EditError::NoClip)), "the playhead is off the clip");
    st.seek(7);
    let d0 = st.undo_depth();
    assert_eq!(st.verb("pan-key"), Some(Ok(())));
    assert_eq!(st.timeline.tracks[1].clips[0].pan_keys.points().to_vec(), vec![(2i64, 0i8)], "src_in 0 + (7 - at 5)");
    assert_eq!(st.undo_depth(), d0 + 1);
    assert_eq!(st.verb("pan-key"), Some(Ok(())));
    assert_eq!(st.undo_depth(), d0 + 1, "the same point commits nothing");
    assert_eq!(st.verb("pan-key -0.5"), Some(Ok(())));
    assert_eq!(st.timeline.tracks[1].clips[0].pan_keys.points().to_vec(), vec![(2i64, -64i8)]);
    assert_eq!(st.verb("pan-unkey"), Some(Ok(())));
    assert_eq!(st.timeline, two_tracks());
    assert_eq!(st.verb("pan-unkey"), Some(Err(EditError::PanKey)));
    assert!(st.undo());
    assert_eq!(st.timeline.tracks[1].clips[0].pan_keys.points().to_vec(), vec![(2i64, -64i8)], "one undo step per word");
    for t in 5..5 + PAN_KEYS_MAX as i64 {
        st.seek(t);
        assert_eq!(st.verb("pan-key 1"), Some(Ok(())));
    }
    st.seek(14);
    assert_eq!(st.verb("pan-key 1"), Some(Err(EditError::PanKey)), "full");
    for bad in ["pan-key 2", "pan-key x", "pan-keyx", "pan-key 0.5 1", "pan-unkey 1", "pan-keys"] {
        assert_eq!(st.verb(bad), None, "{bad}");
    }
}

/// The EDL: `* UNA PANKEYS <tc>=<cq8> ...` under an automated event only, after `* UNA PAN`, read back exactly; refused as
/// `Syntax` with no event above, a token not `<tc>=<i8>`, ticks not rising, or more than eight points.
#[test]
fn striapanautomation_edl_round_trip_and_refusals() {
    assert!(!edl::write(&two_tracks(), "MIX").unwrap().contains("PANKEYS"));
    let tl = two_tracks().set_pan(1, 0, 20).unwrap().set_pan_key(1, 0, 0, -127).unwrap().set_pan_key(1, 0, 8, 127).unwrap();
    let text = edl::write(&tl, "MIX").unwrap();
    assert!(text.contains("* UNA PAN 20\n* UNA PANKEYS 00:00:00:00=-127 00:00:00:08=127\n"), "{text}");
    let back = edl::parse(&text, 25).unwrap();
    assert_eq!(back.tracks[1].clips[0], tl.tracks[1].clips[0]);
    assert_eq!(edl::write(&back, "MIX").unwrap(), text);
    let nine: Vec<String> = (0..9).map(|i| format!("00:00:00:{i:02}=1")).collect();
    let nine = format!("* UNA PANKEYS {}", nine.join(" "));
    for (bad, with) in [
        ("* UNA PANKEYS 00:00:00:01", true),
        ("* UNA PANKEYS 00:00:00:02=1 00:00:00:01=2", true),
        ("* UNA PANKEYS 00:00:00:01=1 00:00:00:01=2", true),
        ("* UNA PANKEYS 00:00:00:01=128", true),
        (nine.as_str(), true),
        ("* UNA PANKEYS 00:00:00:01=5", false),
    ] {
        let mut lines: Vec<&str> = text.lines().collect();
        let at = if with { lines.iter().position(|l| l.starts_with("001")).unwrap() + 1 } else { lines.iter().position(|l| l.starts_with("001")).unwrap() };
        lines.insert(at, bad);
        let t = lines.join("\n");
        assert_eq!(edl::parse(&t, 25).map(|_| ()), Err(edl::EdlError::Syntax { line: at + 1 }), "{bad}");
    }
}

// ---- STRIAPANAUTOWRITE (LEDGER SR9084): a keyed clip's pan words / drag write the point, the curve, the under split (named tail block) ----
use timeline_core::view::{PAN_INK, pan_curve_y};

/// A1 / A2 of `two_tracks` with A2's clip keyed hard left at its source tick 0 and hard right at 8 (timeline ticks 5 / 13).
fn keyed() -> Timeline {
    two_tracks().set_pan_key(1, 0, 0, -127).unwrap().set_pan_key(1, 0, 8, 127).unwrap()
}

/// On a keyed clip every pan word writes the point at the playhead's source tick (one undo step each), stepping from the pan
/// shown there; the shown pan unchanged commits nothing; the static pan is untouched; off the clip `NoClip`; an unkeyed clip
/// keeps the static road.
#[test]
fn striapanautowrite_pan_words_write_the_point_at_the_playhead() {
    let mut st = EditorState::new(keyed());
    st.select(1, 0).unwrap();
    st.seek(9);
    let pts = |st: &EditorState| st.timeline.tracks[1].clips[0].pan_keys.points().to_vec();
    assert_eq!(st.timeline.tracks[1].clips[0].pan_at(9), 0);
    let d0 = st.undo_depth();
    assert_eq!(st.verb("pan"), Some(Ok(())));
    assert_eq!((pts(&st).len(), st.undo_depth()), (2, d0), "shown centre already: no commit");
    assert_eq!(st.verb("pan 0.5"), Some(Ok(())));
    assert_eq!((pts(&st), st.undo_depth()), (vec![(0i64, -127i8), (4, 64), (8, 127)], d0 + 1));
    assert_eq!(st.verb("pan+"), Some(Ok(())));
    assert_eq!((pts(&st)[1], st.undo_depth()), ((4i64, 79i8), d0 + 2), "the step starts from the shown pan");
    assert_eq!(st.timeline.tracks[1].clips[0].pan_cq8, 0, "the static pan is untouched");
    assert!(st.undo());
    assert_eq!(pts(&st)[1], (4, 64));
    st.seek(2);
    assert_eq!(st.verb("pan 0.5"), Some(Err(EditError::NoClip)), "the playhead is off the keyed clip");
    st.select(0, 0).unwrap();
    st.seek(3);
    assert_eq!(st.verb("pan 0.5"), Some(Ok(())));
    assert_eq!((st.timeline.tracks[0].clips[0].pan_cq8, st.timeline.tracks[0].clips[0].pan_keys.n), (64, 0), "unkeyed: the static pan");
    println!("striapanautowrite words: {:?}", pts(&st));
}

/// The grip drag on a keyed clip starts at the pan shown at the playhead, previews the point and commits it once.
#[test]
fn striapanautowrite_pan_drag_writes_the_point() {
    use timeline_core::view::{Drag, DragKind, Layout, pan_half};
    let mut st = EditorState::new(keyed());
    st.select(1, 0).unwrap();
    st.seek(9);
    let l = Layout::of(320, 160, &st);
    let (x0, x1, y0) = (l.x_of(5), l.x_of(15), l.row_y(1));
    let half = pan_half(x0, x1).max(1) as f64;
    let (x, y) = ((x0 + x1) as f64 / 2.0, y0 as f64 + 2.0);
    let d0 = st.undo_depth();
    let mut d = Drag::begin_with(&mut st, l, x, y, false, false, false).expect("a drag");
    assert_eq!(d.kind, DragKind::Pan);
    d.update(&st, x + half / 2.0, y);
    assert_eq!(d.delta, 64, "from the shown pan 0");
    let shown = d.shown(&st).expect("a preview");
    assert_eq!(shown.timeline.tracks[1].clips[0].pan_at(9), 64, "the preview writes the point");
    assert_eq!(d.finish(&mut st), Ok(()));
    let c = st.timeline.tracks[1].clips[0];
    assert_eq!((c.pan_keys.points().to_vec(), c.pan_cq8, st.undo_depth()), (vec![(0i64, -127i8), (4, 64), (8, 127)], 0, d0 + 1));
}

/// A linear crossfade out of a keyed clip: the under layer's runs break wherever the outgoing clip's pan changes and carry
/// its curve at both edges; without points the crossfade is one run as before.
#[test]
fn striapanautowrite_under_layer_splits_at_its_pan() {
    let plain = audio_runs(&audio_only(Curve::Linear), 0..60, &Gop).unwrap();
    assert_eq!(plain.iter().map(|r| (r.out_t, r.len)).collect::<Vec<_>>(), vec![(0, 20), (20, 8), (28, 32)]);
    let tl = audio_only(Curve::Linear).set_pan_key(0, 0, 30, -127).unwrap().set_pan_key(0, 0, 38, 127).unwrap();
    let o = tl.tracks[0].clips[0];
    let runs = audio_runs(&tl, 0..60, &Gop).unwrap();
    for r in runs.iter().filter(|r| (20..28).contains(&r.out_t)) {
        assert_eq!(r.len, 1, "run at {}", r.out_t);
        let u = r.layers[0];
        assert_eq!((u.src, u.pan_cq8, u.pan_to), (SourceId(0), o.pan_at(r.out_t), o.pan_at(r.out_t + 1)), "run at {}", r.out_t);
    }
    assert_eq!(runs.iter().filter(|r| (20..28).contains(&r.out_t)).count(), 8);
    assert_eq!(runs.first().map(|r| (r.out_t, r.len)), Some((0, 20)), "the held pan before the crossfade is one run");
    println!("striapanautowrite under: {} runs (unkeyed {})", runs.len(), plain.len());
}

/// The pan curve paints only on the SELECTED keyed audio clip: a keyed clip unselected paints exactly as unkeyed; selected,
/// the curve's ink sits at the shown pan's y and a point's plus mark at its tick.
#[test]
fn striapanautowrite_curve_paints_on_the_selected_keyed_clip() {
    let (w, h) = (320u32, 160u32);
    assert_eq!(view::paint(&EditorState::new(keyed()), w, h, Order::Rgba, None, None), view::paint(&EditorState::new(two_tracks()), w, h, Order::Rgba, None, None), "unselected: nothing new");
    let mut plain = EditorState::new(two_tracks());
    plain.select(1, 0).unwrap();
    let mut st = EditorState::new(keyed());
    st.select(1, 0).unwrap();
    let (a, px) = (view::paint(&plain, w, h, Order::Rgba, None, None), view::paint(&st, w, h, Order::Rgba, None, None));
    assert_ne!(a, px);
    let l = view::Layout::of(w, h, &st);
    let at = |x: i64, y: i64| &px[((y as usize) * w as usize + x as usize) * 4..][..4];
    let (x, y) = (l.x_of(9) + 3, pan_curve_y(&l, 1, 0));
    assert_eq!((at(x, y), at(x, y + 1)), (&PAN_INK[..], &PAN_INK[..]), "the curve at tick 9 (pan 0)");
    let (xp, yp) = (l.x_of(13), pan_curve_y(&l, 1, 127));
    assert_eq!((at(xp, yp + 2), at(xp + 2, yp)), (&PAN_INK[..], &PAN_INK[..]), "the plus at the hard-right point");
    assert!(pan_curve_y(&l, 1, -127) < pan_curve_y(&l, 1, 127), "hard left up");
}
