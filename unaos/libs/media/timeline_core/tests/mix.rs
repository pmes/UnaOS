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
