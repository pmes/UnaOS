// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! NLE1 (SR194) M1: the edit operations and their invariants.

use timeline_core::{Clip, EditError, Kind, Mode, SourceId, Timeline};

/// A (100 ticks), B (60 ticks); V1, A1; V1 holds A[0,10)@0 and B[20,30)@10.
fn base() -> (Timeline, SourceId, SourceId) {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 100);
    let b = tl.add_source("B", 60);
    tl.add_track(Kind::Video);
    tl.add_track(Kind::Audio);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 10, 0), Clip::new(b, 20, 30, 10)];
    tl.validate().unwrap();
    (tl, a, b)
}

#[test]
fn insert_splits_and_ripples() {
    let (tl, a, b) = base();
    let t2 = tl.place(0, Clip::new(a, 50, 55, 5), Mode::Insert).unwrap();
    assert_eq!(
        t2.tracks[0].clips,
        vec![Clip::new(a, 0, 5, 0), Clip::new(a, 50, 55, 5), Clip::new(a, 5, 10, 10), Clip::new(b, 20, 30, 15)]
    );
    assert_eq!(t2.end(), 25);
    // The source is untouched: pure.
    assert_eq!(tl.end(), 20);
}

#[test]
fn overwrite_carves_without_ripple() {
    let (tl, a, b) = base();
    let t2 = tl.place(0, Clip::new(a, 70, 76, 7), Mode::Overwrite).unwrap();
    assert_eq!(
        t2.tracks[0].clips,
        vec![Clip::new(a, 0, 7, 0), Clip::new(a, 70, 76, 7), Clip::new(b, 23, 30, 13)]
    );
    assert_eq!(t2.end(), 20);
    // Covering a clip whole removes it.
    let t3 = tl.place(0, Clip::new(a, 0, 30, 0), Mode::Overwrite).unwrap();
    assert_eq!(t3.tracks[0].clips, vec![Clip::new(a, 0, 30, 0)]);
}

#[test]
fn split_keeps_the_frames() {
    let (tl, a, b) = base();
    let t2 = tl.split(0, 14).unwrap();
    assert_eq!(t2.tracks[0].clips, vec![Clip::new(a, 0, 10, 0), Clip::new(b, 20, 24, 10), Clip::new(b, 24, 30, 14)]);
    // At a clip boundary or in a gap there is nothing to split.
    assert_eq!(tl.split(0, 10), Err(EditError::NoClip));
    assert_eq!(tl.split(0, 40), Err(EditError::NoClip));
    assert_eq!(tl.split(9, 4), Err(EditError::NoTrack));
}

#[test]
fn trims_hold_the_invariants() {
    let (tl, a, b) = base();
    // trim in: the start moves with the in point, the out edge stays.
    let t2 = tl.trim_in(0, 1, 25).unwrap();
    assert_eq!(t2.tracks[0].clips[1], Clip::new(b, 25, 30, 15));
    // extending the in point back into A's clip overlaps it.
    assert_eq!(tl.trim_in(0, 1, 15), Err(EditError::Overlap));
    assert_eq!(tl.trim_in(0, 0, 10), Err(EditError::Empty));
    assert_eq!(tl.trim_in(0, 1, -1), Err(EditError::Negative));
    // trim out
    assert_eq!(tl.trim_out(0, 1, 60).unwrap().tracks[0].clips[1], Clip::new(b, 20, 60, 10));
    assert_eq!(tl.trim_out(0, 1, 61), Err(EditError::OutOfSource));
    assert_eq!(tl.trim_out(0, 0, 11), Err(EditError::Overlap));
    assert_eq!(tl.trim_out(0, 5, 11), Err(EditError::NoClip));
    let _ = a;
}

#[test]
fn ripple_delete_closes_the_hole_and_lift_leaves_it() {
    let (tl, a, b) = base();
    let t2 = tl.place(0, Clip::new(a, 40, 45, 25), Mode::Overwrite).unwrap();
    let r = t2.ripple_delete(0, 1).unwrap();
    assert_eq!(r.tracks[0].clips, vec![Clip::new(a, 0, 10, 0), Clip::new(a, 40, 45, 15)]);
    let l = t2.lift(0, 1).unwrap();
    assert_eq!(l.tracks[0].clips, vec![Clip::new(a, 0, 10, 0), Clip::new(a, 40, 45, 25)]);
    assert_eq!(t2.ripple_delete(0, 3), Err(EditError::NoClip));
    let _ = b;
}

#[test]
fn move_refuses_overlap_and_kind() {
    let (mut tl, a, b) = base();
    tl.add_track(Kind::Video); // V2 = track 2
    let m = tl.move_clip(0, 1, 0, 40).unwrap();
    assert_eq!(m.tracks[0].clips, vec![Clip::new(a, 0, 10, 0), Clip::new(b, 20, 30, 40)]);
    assert_eq!(tl.move_clip(0, 1, 0, 5), Err(EditError::Overlap));
    let up = tl.move_clip(0, 1, 2, 5).unwrap();
    assert_eq!(up.tracks[2].clips, vec![Clip::new(b, 20, 30, 5)]);
    assert_eq!(tl.move_clip(0, 1, 1, 0), Err(EditError::Kind));
    assert_eq!(tl.move_clip(0, 1, 0, -1), Err(EditError::Negative));
}

#[test]
fn validate_refuses_a_hand_built_overlap() {
    let (mut tl, a, _) = base();
    tl.tracks[0].clips.push(Clip::new(a, 0, 5, 18));
    assert_eq!(tl.validate(), Err(EditError::Overlap));
    // out of order is refused the same way
    let (mut tl, a, _) = base();
    tl.tracks[0].clips.insert(0, Clip::new(a, 0, 2, 30));
    assert_eq!(tl.validate(), Err(EditError::Overlap));
    // a source that does not exist
    let (mut tl, _, _) = base();
    tl.tracks[0].clips.push(Clip::new(SourceId(9), 0, 2, 40));
    assert_eq!(tl.validate(), Err(EditError::NoSource));
    // an overlapping insert can never be built through place
    let (tl, a, _) = base();
    assert_eq!(tl.place(0, Clip::new(a, 5, 5, 0), Mode::Insert), Err(EditError::Empty));
    assert_eq!(tl.place(0, Clip::new(a, 0, 101, 0), Mode::Insert), Err(EditError::OutOfSource));
}

/// NLE2c (SR217): the set ops are ONE transaction each: ripple / lift over a set equal the single ops applied by hand
/// (descending), a set move lands butted clips together, and a refused set move or an empty set leaves the input as is.
#[test]
fn many_ops_are_one_transaction() {
    let (mut tl, a, b) = base();
    tl.tracks[0].clips.push(Clip::new(a, 50, 55, 25)); // V1: A[0,10)@0 B[20,30)@10 A[50,55)@25
    tl.tracks[1].clips = vec![Clip::new(a, 0, 10, 0)];
    tl.validate().unwrap();
    let picks = [(0, 0), (0, 2), (1, 0), (0, 0)];

    let want = tl.ripple_delete(1, 0).unwrap().ripple_delete(0, 2).unwrap().ripple_delete(0, 0).unwrap();
    assert_eq!(tl.ripple_delete_many(&picks).unwrap(), want);
    assert_eq!(want.tracks[0].clips, vec![Clip::new(b, 20, 30, 0)]);

    let lifted = tl.lift_many(&picks).unwrap();
    assert_eq!(lifted.tracks[0].clips, vec![Clip::new(b, 20, 30, 10)], "the holes stay");
    assert!(lifted.tracks[1].clips.is_empty());

    // A[0,10)@0 and B@10 are butted: moving both right by 5 is one value; the first single move_clip already overlaps.
    let moved = tl.move_many(&[(0, 0), (0, 1)], 5).unwrap();
    assert_eq!(moved.tracks[0].clips, vec![Clip::new(a, 0, 10, 5), Clip::new(b, 20, 30, 15), Clip::new(a, 50, 55, 25)]);
    assert_eq!(tl.move_clip(0, 0, 0, 5), Err(EditError::Overlap));

    // Into a neighbour, before zero, an empty set, a bad pick: refused, and the input is untouched (pure).
    let before = tl.clone();
    assert_eq!(tl.move_many(&[(0, 0), (0, 1)], 6), Err(EditError::Overlap));
    assert_eq!(tl.move_many(&[(0, 1), (0, 2)], -6), Err(EditError::Overlap));
    assert_eq!(tl.move_many(&[(0, 0)], -1), Err(EditError::Negative));
    assert_eq!(tl.move_many(&[], 1), Err(EditError::NoClip));
    assert_eq!(tl.lift_many(&[(0, 9)]), Err(EditError::NoClip));
    assert_eq!(tl.ripple_delete_many(&[(7, 0)]), Err(EditError::NoTrack));
    assert_eq!(tl, before);
}

// ── EXPORTCOLOURPRIM (rmbp-ledger B979): the export's ONE primaries / transfer rule ──
#[test]
fn exportcolourprim_reels_colour_is_the_one_pair() {
    use demux_core::colour::Colour;
    let col = |p: u8, t: u8, m: u8| Colour { primaries: p, transfer: t, matrix: m, full_range: Some(false), mastering: None, light: None };
    let (tl, a, b) = base(); // V1: A then B; A1 empty
    let by = move |ca: Option<Colour>, cb: Option<Colour>| move |id: SourceId| if id == a { ca } else if id == b { cb } else { None };
    assert_eq!(tl.reels_colour(by(Some(col(1, 1, 1)), Some(col(1, 1, 6)))), (1, 1), "all equal: the pair (the matrix is not part of it)");
    assert_eq!(tl.reels_colour(by(Some(col(1, 1, 1)), Some(col(9, 14, 9)))), (2, 2), "one differs");
    assert_eq!(tl.reels_colour(by(Some(col(1, 1, 1)), None)), (2, 2), "one states none");
    assert_eq!(tl.reels_colour(by(None, None)), (2, 2), "none stated");
    let mut audio = tl.clone();
    audio.tracks[1].clips = vec![Clip::new(b, 0, 10, 0)];
    assert_eq!(audio.reels_colour(by(Some(col(1, 1, 1)), Some(col(1, 1, 1)))), (1, 1), "same reels");
    audio.tracks[0].clips = vec![Clip::new(a, 0, 10, 0)];
    assert_eq!(audio.reels_colour(by(Some(col(9, 16, 9)), None)), (9, 16), "an audio-track clip's reel is not a picture");
    audio.tracks[0].clips.clear();
    assert_eq!(audio.reels_colour(by(Some(col(1, 1, 1)), Some(col(1, 1, 1)))), (2, 2), "no video clip");
}

// ---- NLE3ETRIMSTRETCH (LEDGER SR692): the ripple trims on plain clips (named tail block) ----
#[test]
fn nle3etrimstretch_ripple_trims_shift_the_rest_of_the_track() {
    let (mut tl, a, b) = base();
    tl.tracks[0].clips.push(Clip::new(a, 50, 55, 25)); // V1: A[0,10)@0 B[20,30)@10 A[50,55)@25
    tl.tracks[1].clips = vec![Clip::new(a, 0, 10, 0)];
    tl.validate().unwrap();
    let before = tl.clone();
    // out 5 earlier: the clip keeps its start, the later clip of its track moves 5 earlier, the other track stays
    let r = tl.ripple_trim_out(0, 1, 25).unwrap();
    assert_eq!(r.tracks[0].clips, vec![Clip::new(a, 0, 10, 0), Clip::new(b, 20, 25, 10), Clip::new(a, 50, 55, 20)]);
    assert_eq!(r.tracks[1], tl.tracks[1]);
    // in 2 earlier: the clip is 2 longer from the same start, the later clip moves 2 later
    let r = tl.ripple_trim_in(0, 1, 18).unwrap();
    assert_eq!((r.tracks[0].clips[1], r.tracks[0].clips[2].at), (Clip::new(b, 18, 30, 10), 27));
    // refused by name, the input untouched
    assert_eq!(tl.ripple_trim_in(0, 1, 30), Err(EditError::Empty));
    assert_eq!(tl.ripple_trim_in(0, 1, -1), Err(EditError::Negative));
    assert_eq!(tl.ripple_trim_out(0, 1, 61), Err(EditError::OutOfSource));
    assert_eq!(tl.ripple_trim_out(0, 5, 1), Err(EditError::NoClip));
    assert_eq!(tl.ripple_trim_out(9, 0, 1), Err(EditError::NoTrack));
    assert_eq!(tl, before);
}

// ---- NLE3ESLIP (LEDGER SR704): slip and slide on plain clips (named tail block) ----
#[test]
fn nle3eslip_slip_slides_the_source_window() {
    let (tl, a, b) = base(); // V1: A[0,10)@0 B[20,30)@10 (B is 60 ticks)
    let before = tl.clone();
    // +3: the source window moves, the timeline start and length stay
    let s = tl.slip(0, 1, 3).unwrap();
    assert_eq!(s.tracks[0].clips, vec![Clip::new(a, 0, 10, 0), Clip::new(b, 23, 33, 10)]);
    assert_eq!((s.tracks[0].clips[1].at, s.tracks[0].clips[1].len(), s.end()), (10, 10, tl.end()));
    // up to the source's end exactly
    assert_eq!(tl.slip(0, 1, 30).unwrap().tracks[0].clips[1], Clip::new(b, 50, 60, 10));
    // past the source's start or end: refused by name, the input untouched
    assert_eq!(tl.slip(0, 1, -21), Err(EditError::OutOfSource));
    assert_eq!(tl.slip(0, 1, 31), Err(EditError::OutOfSource));
    assert_eq!(tl.slip(0, 2, 1), Err(EditError::NoClip));
    assert_eq!(tl.slip(5, 0, 1), Err(EditError::NoTrack));
    assert_eq!(tl, before);
}

#[test]
fn nle3eslip_slide_moves_the_clip_and_its_neighbours() {
    let (mut tl, a, b) = base();
    tl.tracks[0].clips.push(Clip::new(a, 50, 60, 20)); // V1: A[0,10)@0 B[20,30)@10 A[50,60)@20
    tl.validate().unwrap();
    let before = tl.clone();
    // +5: B moves 5 later with its content, A ends 5 later, the next A starts 5 later, the track's length stays
    let s = tl.slide(0, 1, 5).unwrap();
    assert_eq!(s.tracks[0].clips, vec![Clip::new(a, 0, 15, 0), Clip::new(b, 20, 30, 15), Clip::new(a, 55, 60, 25)]);
    assert_eq!(s.end(), tl.end());
    let text = timeline_core::edl::write(&s, "SLIDE").unwrap();
    let back = timeline_core::edl::parse(&text, 25).unwrap();
    let video = |t: &Timeline| t.tracks.iter().find(|k| k.kind == Kind::Video).map(|k| k.clips.clone());
    assert_eq!((video(&back), timeline_core::edl::write(&back, "SLIDE").unwrap()), (video(&s), text));
    // a neighbour slid to nothing, a neighbour past its source, the neighbour-less side before zero: refused by name
    assert_eq!(tl.slide(0, 1, -10), Err(EditError::Empty));
    assert_eq!(tl.slide(0, 1, 10), Err(EditError::Empty));
    assert_eq!(tl.slide(0, 1, 91), Err(EditError::OutOfSource));
    assert_eq!(tl.slide(0, 0, -1), Err(EditError::Negative));
    assert_eq!(tl.slide(0, 3, 1), Err(EditError::NoClip));
    assert_eq!(tl.slide(4, 0, 1), Err(EditError::NoTrack));
    assert_eq!(tl, before);
}

// ---- STRIAAUDIOCROSSFADE (LEDGER SR860): the audio crossfade word, its curve's runs and its bracket (named tail block) ----

use timeline_core::editor::EditorState;

/// V1: A[0,20)@0. A1: A[10,30)@0 (handle 20), B[0,20)@20 (the cut at 20), A[40,45)@50 (after a gap).
fn striaaudiocrossfade_state() -> EditorState {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Video);
    tl.add_track(Kind::Audio);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 20, 0)];
    tl.tracks[1].clips = vec![Clip::new(a, 10, 30, 0), Clip::new(b, 0, 20, 20), Clip::new(a, 40, 45, 50)];
    EditorState::new(tl)
}

struct StriaAudioCrossfadeGop;
impl timeline_core::SourceIndex for StriaAudioCrossfadeGop {
    fn duration(&self, _: SourceId) -> Option<i64> {
        Some(50)
    }
    fn keyframe_ns(&self, _: SourceId, _: Kind, t_ns: i64) -> Option<i64> {
        Some(t_ns / 40_000_000 * 40_000_000)
    }
}

/// Bare `xfade` on the incoming clip puts ONE equal-power record (half a second, 12 ticks, inside both handles), a second
/// bare `xfade` takes it off; `xfade 4` puts, `xfade 6` replaces; each change is one undo step; the EDL carries it.
#[test]
fn striaaudiocrossfade_word_puts_and_takes_one_record() {
    use timeline_core::{AudioXfade, Curve, edl};
    let mut st = striaaudiocrossfade_state();
    let tl0 = st.timeline.clone();
    st.select(1, 1).unwrap();
    assert_eq!(st.verb("xfade"), Some(Ok(())));
    assert_eq!(st.timeline.audio_xfades, vec![AudioXfade { track: 1, at: 20, dur: 12, curve: Curve::EqualPower }]);
    assert_eq!(st.timeline, tl0.add_audio_xfade(1, 20, 12, Curve::EqualPower).unwrap());
    assert_eq!(st.undo_depth(), 1);
    let text = edl::write(&st.timeline, "XF").unwrap();
    assert!(text.contains("* UNA XFADE EQUAL"), "{text}");
    assert_eq!(edl::parse(&text, 25).unwrap().audio_xfades, st.timeline.audio_xfades);
    assert_eq!(st.verb("xfade"), Some(Ok(())));
    assert_eq!((st.timeline.audio_xfades.len(), st.undo_depth()), (0, 2));
    assert!(st.undo());
    assert_eq!(st.timeline.audio_xfades[0].dur, 12);
    assert!(st.redo());
    assert!(st.timeline.audio_xfades.is_empty());
    assert_eq!(st.verb("xfade 4"), Some(Ok(())));
    assert_eq!(st.verb("xfade 6"), Some(Ok(())));
    assert_eq!(st.timeline.audio_xfades, vec![AudioXfade { track: 1, at: 20, dur: 6, curve: Curve::EqualPower }]);
    assert_eq!(st.undo_depth(), 4);
    assert!(st.undo());
    assert_eq!(st.timeline.audio_xfades[0].dur, 4);
    println!("striaaudiocrossfade: xfade -> 12 ticks EqualPower; xfade -> off; xfade 4 / 6 -> 6; undo -> 4");
}

/// Each refusal by name; a refused word changes neither the timeline nor the undo stack.
#[test]
fn striaaudiocrossfade_refusals_by_name() {
    let mut st = striaaudiocrossfade_state();
    let tl0 = st.timeline.clone();
    let check = |st: &mut EditorState, sel: Option<(usize, usize)>, v: &str, want: EditError| {
        match sel {
            Some((t, i)) => st.select(t, i).unwrap(),
            None => st.deselect(),
        }
        assert_eq!(st.verb(v), Some(Err(want)), "{v} on {sel:?}");
        assert_eq!((&st.timeline, st.undo_depth()), (&tl0, 0), "{v} on {sel:?}");
    };
    check(&mut st, None, "xfade", EditError::NoClip);
    check(&mut st, Some((0, 0)), "xfade", EditError::Transition);
    check(&mut st, Some((1, 0)), "xfade", EditError::Transition);
    check(&mut st, Some((1, 2)), "xfade", EditError::Transition);
    check(&mut st, Some((1, 1)), "xfade 0", EditError::Transition);
    check(&mut st, Some((1, 1)), "xfade 99", EditError::Handle);
    check(&mut st, Some((1, 1)), "xfade x", EditError::Transition);
    check(&mut st, Some((1, 1)), "xfade 3 4", EditError::Transition);
    println!("striaaudiocrossfade: refused NoClip / Transition / Handle by name, nothing committed");
}

/// After the word, the schedule's runs over the cut are one per tick with the equal-power weights at each edge: the runs
/// the host mix and the kernel Player both play (the curve is the schedule's, not a mixer's).
#[test]
fn striaaudiocrossfade_runs_follow_the_curve() {
    use timeline_core::Curve;
    use timeline_core::schedule::{ENV_ONE, audio_runs, xfade_weight};
    let mut st = striaaudiocrossfade_state();
    st.select(1, 1).unwrap();
    assert_eq!(st.verb("xfade 8"), Some(Ok(())));
    let runs = audio_runs(&st.timeline, 0..60, &StriaAudioCrossfadeGop).unwrap();
    assert_eq!((runs[0].out_t, runs[0].len, runs[0].layers.len()), (0, 20, 1));
    for k in 0..8i64 {
        let r = &runs[1 + k as usize];
        assert_eq!((r.out_t, r.len, r.layers.len()), (20 + k, 1, 2), "tick {k}");
        let p = |k: i64| (k * ENV_ONE as i64 / 8) as u32;
        assert_eq!(r.layers[1].env, [xfade_weight(Curve::EqualPower, p(k)), xfade_weight(Curve::EqualPower, p(k + 1))]);
        assert_eq!(r.layers[0].env, [xfade_weight(Curve::EqualPower, ENV_ONE - p(k)), xfade_weight(Curve::EqualPower, ENV_ONE - p(k + 1))]);
    }
    println!("striaaudiocrossfade: 8 one-tick runs, sin / cos edges");
}

/// The explicit record paints its bracket on the audio row's foot band (THE transition widget's rect) and nothing else
/// moves; without a record the paint is what it was.
#[test]
fn striaaudiocrossfade_paint_shows_the_bracket() {
    use timeline_core::view::{Layout, Order, TRANS, paint_in};
    let mut st = striaaudiocrossfade_state();
    st.select(1, 1).unwrap();
    let l = Layout::of(320, 160, &st);
    let before = paint_in(&st, l, Order::Rgba, None, None);
    assert_eq!(st.verb("xfade 8"), Some(Ok(())));
    let after = paint_in(&st, l, Order::Rgba, None, None);
    let (x0, x1, y1) = (l.x_of(20), l.x_of(28), l.row_y(1) + 40 - 2);
    let y0 = y1 - 10;
    let mut moved = 0;
    for (i, (a, b)) in before.chunks(4).zip(after.chunks(4)).enumerate() {
        if a != b {
            let (x, y) = ((i % 320) as i64, (i / 320) as i64);
            assert!(x >= x0 && x < x1 && y >= y0 && y < y1, "pixel ({x},{y}) moved outside the bracket");
            moved += 1;
        }
    }
    assert!(moved > 0);
    let at = |x: i64, y: i64| { let i = ((y * 320 + x) * 4) as usize; [after[i], after[i + 1], after[i + 2], after[i + 3]] };
    assert_eq!(at(x0, y0), TRANS);
    assert!(st.undo());
    assert_eq!(paint_in(&st, l, Order::Rgba, None, None), before);
    println!("striaaudiocrossfade: bracket {x0}..{x1} x {y0}..{y1}, {moved} px moved");
}

// ---- STRIAXFADECURVE (LEDGER SR904): the crossfade curve words and the audio refusal text (named tail block) ----

/// `xfade linear` puts a LINEAR record of the default length (12 ticks), the EDL carries no `* UNA XFADE EQUAL` and parses
/// back linear; `xfade equal` keeps the length and swaps the curve; `xfade 6 linear` replaces; bare `xfade` still takes it
/// off; each change one undo step.
#[test]
fn striaxfadecurve_curve_words_one_undo_step_each() {
    use timeline_core::{AudioXfade, Curve, edl};
    let mut st = striaaudiocrossfade_state();
    st.select(1, 1).unwrap();
    assert_eq!(st.verb("xfade linear"), Some(Ok(())));
    assert_eq!(st.timeline.audio_xfades, vec![AudioXfade { track: 1, at: 20, dur: 12, curve: Curve::Linear }]);
    let text = edl::write(&st.timeline, "XC").unwrap();
    assert!(!text.contains("* UNA XFADE EQUAL"), "{text}");
    assert_eq!(edl::parse(&text, 25).unwrap().audio_xfades, st.timeline.audio_xfades);
    assert_eq!(st.verb("xfade equal"), Some(Ok(())));
    assert_eq!(st.timeline.audio_xfades, vec![AudioXfade { track: 1, at: 20, dur: 12, curve: Curve::EqualPower }]);
    assert_eq!(st.verb("xfade 6 linear"), Some(Ok(())));
    assert_eq!(st.timeline.audio_xfades, vec![AudioXfade { track: 1, at: 20, dur: 6, curve: Curve::Linear }]);
    assert_eq!(st.verb("xfade linear"), Some(Ok(())));
    assert_eq!(st.timeline.audio_xfades[0].dur, 6);
    assert_eq!(st.undo_depth(), 4);
    assert_eq!(st.verb("xfade"), Some(Ok(())));
    assert!(st.timeline.audio_xfades.is_empty());
    assert!(st.undo());
    assert_eq!(st.timeline.audio_xfades, vec![AudioXfade { track: 1, at: 20, dur: 6, curve: Curve::Linear }]);
    println!("striaxfadecurve: linear 12 -> equal 12 -> linear 6 -> linear keeps 6 -> bare off; undo -> linear 6");
}

/// Each curve-word refusal by name; nothing committed.
#[test]
fn striaxfadecurve_curve_refusals_by_name() {
    let mut st = striaaudiocrossfade_state();
    let tl0 = st.timeline.clone();
    st.select(1, 1).unwrap();
    for (v, want) in [
        ("xfade sine", EditError::Transition),
        ("xfade 6 linear x", EditError::Transition),
        ("xfade x linear", EditError::Transition),
        ("xfade 0 linear", EditError::Transition),
        ("xfade 99 equal", EditError::Handle),
    ] {
        assert_eq!(st.verb(v), Some(Err(want)), "{v}");
        assert_eq!((&st.timeline, st.undo_depth()), (&tl0, 0), "{v}");
    }
    st.select(0, 0).unwrap();
    assert_eq!(st.verb("xfade linear"), Some(Err(EditError::Transition)));
    println!("striaxfadecurve: sine / third word / bad count / 0 -> Transition; 99 -> Handle; video clip -> Transition");
}

/// `xfade_refusal` words each audio refusal from the unchanged state; a non-xfade word or an unworded error is `None`.
#[test]
fn striaxfadecurve_refusal_text() {
    let mut st = striaaudiocrossfade_state();
    assert_eq!(st.xfade_refusal("xfade", EditError::NoClip), Some("no clip selected (select the incoming audio clip)"));
    st.select(0, 0).unwrap();
    assert_eq!(st.xfade_refusal("xfade", EditError::Transition), Some("the selected clip is not on an audio track"));
    st.select(1, 2).unwrap();
    assert_eq!(st.xfade_refusal("xfade linear", EditError::Transition), Some("no clip ends where the selected clip starts"));
    st.select(1, 1).unwrap();
    assert_eq!(st.xfade_refusal("xfade sine", EditError::Transition), Some("usage: xfade [<ticks>] [linear|equal], ticks at least 1"));
    assert_eq!(st.xfade_refusal("xfade 99", EditError::Handle), Some("the crossfade is longer than the incoming clip or the outgoing clip's handle"));
    assert_eq!(st.xfade_refusal("kind", EditError::Transition), None);
    assert_eq!(st.xfade_refusal("xfade", EditError::NoSource), None);
    println!("striaxfadecurve: five audio wordings; kind / NoSource -> None");
}

// ---- STRIAXFADECURVEBUTTON (LEDGER SR925): the crossfade curve cycle word and the xfade reading (named tail block) ----

/// `xfade-curve` swaps the head cut's crossfade curve at the same length: equal 12 -> linear 12 -> equal 12, one undo step
/// each; undo takes the swap back.
#[test]
fn striaxfadecurvebutton_cycle_one_undo_step_each() {
    use timeline_core::{AudioXfade, Curve};
    let mut st = striaaudiocrossfade_state();
    st.select(1, 1).unwrap();
    assert_eq!(st.verb("xfade"), Some(Ok(())));
    assert_eq!(st.timeline.audio_xfades, vec![AudioXfade { track: 1, at: 20, dur: 12, curve: Curve::EqualPower }]);
    assert_eq!(st.verb("xfade-curve"), Some(Ok(())));
    assert_eq!(st.timeline.audio_xfades, vec![AudioXfade { track: 1, at: 20, dur: 12, curve: Curve::Linear }]);
    assert_eq!(st.verb("xfade-curve"), Some(Ok(())));
    assert_eq!(st.timeline.audio_xfades, vec![AudioXfade { track: 1, at: 20, dur: 12, curve: Curve::EqualPower }]);
    assert_eq!(st.undo_depth(), 3);
    assert!(st.undo());
    assert_eq!(st.timeline.audio_xfades[0].curve, Curve::Linear);
    println!("striaxfadecurvebutton: equal 12 -> linear 12 -> equal 12 (3 undo steps); undo -> linear");
}

/// Refusals commit nothing: no selection `NoClip`, a plain cut with no crossfade / a video clip / no butting clip
/// `Transition`; each worded (the new no-crossfade text, else SR904's).
#[test]
fn striaxfadecurvebutton_cycle_refusals_and_text() {
    let mut st = striaaudiocrossfade_state();
    let tl0 = st.timeline.clone();
    assert_eq!(st.verb("xfade-curve"), Some(Err(EditError::NoClip)));
    assert_eq!(st.xfade_curve_refusal("xfade-curve", EditError::NoClip), Some("no clip selected (select the incoming audio clip)"));
    st.select(1, 1).unwrap();
    assert_eq!(st.verb("xfade-curve"), Some(Err(EditError::Transition)));
    assert_eq!(st.xfade_curve_refusal("xfade-curve", EditError::Transition), Some("no crossfade on the selected clip's head cut (xfade puts one)"));
    st.select(0, 0).unwrap();
    assert_eq!(st.verb("xfade-curve"), Some(Err(EditError::Transition)));
    assert_eq!(st.xfade_curve_refusal("xfade-curve", EditError::Transition), Some("the selected clip is not on an audio track"));
    st.select(1, 2).unwrap();
    assert_eq!(st.verb("xfade-curve"), Some(Err(EditError::Transition)));
    assert_eq!(st.xfade_curve_refusal("xfade-curve", EditError::Transition), Some("no clip ends where the selected clip starts"));
    assert_eq!(st.xfade_curve_refusal("xfade", EditError::Transition), None);
    assert_eq!((&st.timeline, st.undo_depth()), (&tl0, 0));
    println!("striaxfadecurvebutton: no selection / no crossfade / video clip / no butting clip refused in words");
}

/// `xfade_reading`: the head cut's explicit record as `<ticks> <curve>`, `off` without one; any other word `None`.
#[test]
fn striaxfadecurvebutton_reading() {
    let mut st = striaaudiocrossfade_state();
    assert_eq!(st.xfade_reading("xfade"), None);
    st.select(1, 1).unwrap();
    assert_eq!(st.xfade_reading("xfade").as_deref(), Some("off"));
    assert_eq!(st.verb("xfade 10"), Some(Ok(())));
    assert_eq!(st.xfade_reading("xfade 10").as_deref(), Some("10 equal"));
    assert_eq!(st.verb("xfade-curve"), Some(Ok(())));
    assert_eq!(st.xfade_reading("xfade-curve").as_deref(), Some("10 linear"));
    assert_eq!(st.xfade_reading("kind"), None);
    println!("striaxfadecurvebutton: off -> 10 equal -> 10 linear; kind -> None");
}

// ---- STRIAXFADEMORE (LEDGER SR980): the bracket names its curve, and the curve's one word (named tail block) ----

/// Every label the painter asks for: (text, clip span).
struct StriaXfadeMoreLabels(Vec<(String, (i32, i32))>);
impl timeline_core::view::Text for StriaXfadeMoreLabels {
    fn text(&mut self, _: &mut timeline_core::view::Canvas, s: &str, _: f32, _: f32, clip: (i32, i32)) {
        self.0.push((s.to_string(), clip));
    }
}

/// The curve labels of a labels-on paint of `st`.
fn striaxfademore_names(st: &EditorState, l: timeline_core::view::Layout) -> Vec<(String, (i32, i32))> {
    use timeline_core::view::{Order, Text, paint_in};
    let mut rec = StriaXfadeMoreLabels(Vec::new());
    let _ = paint_in(st, l, Order::Rgba, Some(&mut rec as &mut dyn Text), None);
    rec.0.into_iter().filter(|(s, _)| s == "equal" || s == "linear").collect()
}

/// With labels on, the explicit crossfade's bracket names its curve once, clipped to the bracket; the curve word follows
/// `xfade-curve`; without a record (undo) nothing is named.
#[test]
fn striaxfademore_bracket_names_its_curve() {
    use timeline_core::view::{Layout, TRANS_MIN_W};
    let mut st = striaaudiocrossfade_state();
    st.select(1, 1).unwrap();
    let l = Layout::of(320, 160, &st);
    assert_eq!(striaxfademore_names(&st, l), Vec::new());
    assert_eq!(st.verb("xfade 8"), Some(Ok(())));
    let (x0, x1) = (l.x_of(20), l.x_of(28).max(l.x_of(20) + TRANS_MIN_W));
    assert_eq!(striaxfademore_names(&st, l), vec![("equal".to_string(), (x0 as i32, x1 as i32))]);
    assert_eq!(st.verb("xfade-curve"), Some(Ok(())));
    assert_eq!(striaxfademore_names(&st, l), vec![("linear".to_string(), (x0 as i32, x1 as i32))]);
    assert!(st.undo() && st.undo());
    assert_eq!(striaxfademore_names(&st, l), Vec::new());
    println!("striaxfademore: bracket {x0}..{x1} named equal -> linear; none after undo");
}

/// `Curve::word` is the `xfade` words' spelling, and the reading uses it.
#[test]
fn striaxfademore_curve_word() {
    use timeline_core::Curve;
    assert_eq!((Curve::Linear.word(), Curve::EqualPower.word()), ("linear", "equal"));
    let mut st = striaaudiocrossfade_state();
    st.select(1, 1).unwrap();
    assert_eq!(st.verb("xfade 8 linear"), Some(Ok(())));
    assert_eq!(st.xfade_reading("xfade").as_deref(), Some("8 linear"));
    println!("striaxfademore: Linear -> linear, EqualPower -> equal");
}


// ---- STRIAXFADETHREE (LEDGER SR9023): the audio crossfade bracket's edge drag (named tail block) ----

/// SR860's fixture at zoom 8 with an 8-tick equal-power crossfade on A1's cut at 20 (its bracket x 160..224); the layout.
fn striaxfadethree_state() -> (EditorState, timeline_core::view::Layout) {
    let mut st = striaaudiocrossfade_state();
    st.zoom = 8;
    st.select(1, 1).unwrap();
    assert_eq!(st.verb("xfade 8"), Some(Ok(())));
    st.deselect();
    let l = timeline_core::view::Layout::of(320, 160, &st);
    (st, l)
}

/// The bracket's one geometry is THE transition widget's rect over the record; a press on its right / left edge zone drags
/// the length (`dur + delta` / `dur - delta`) in ONE commit with the curve kept and the incoming clip selected; the middle
/// is no edge drag; past the outgoing's handle the model refuses and nothing changes; the moving edge snaps to a clip edge.
#[test]
fn striaxfadethree_xfade_edge_drag() {
    use timeline_core::view::{Drag, DragKind, ROW_H, TRANS_H};
    use timeline_core::{Curve, EditError};
    let (st0, l) = striaxfadethree_state();
    let y1 = l.row_y(1) + ROW_H as i64 - 2;
    assert_eq!(l.xfade_rect(&st0.timeline.audio_xfades[0]), (160, y1 - TRANS_H, 224, y1));
    let y = (y1 - TRANS_H / 2) as f64;
    assert_eq!(l.xfade_at(&st0.timeline, 200.0, y), Some(0));
    assert_eq!(l.xfade_at(&st0.timeline, 200.0, (y1 - TRANS_H - 1) as f64), None, "above the bracket");
    // The right edge, +2 ticks, no snap.
    let mut st = st0.clone();
    let mut d = Drag::begin_xfade_edge(&mut st, l, 222.0, y, true).expect("the right edge zone");
    assert_eq!((d.kind, d.track, d.idx, st.selection), (DragKind::XfadeOut, 1, 1, Some((1, 1))));
    d.update(&st, 238.0, y);
    assert_eq!(d.delta, 2);
    assert_eq!(d.shown(&st).map(|s| s.timeline.audio_xfades[0].dur), Some(10), "the preview");
    assert_eq!(d.finish(&mut st), Ok(()));
    let x = st.timeline.audio_xfades[0];
    assert_eq!((x.track, x.at, x.dur, x.curve), (1, 20, 10, Curve::EqualPower));
    assert_eq!(st.undo_depth(), st0.undo_depth() + 1, "one undo step");
    assert!(st.undo());
    assert_eq!(st.timeline, st0.timeline);
    // The left edge, -2 ticks: the crossfade lengthens, anchored at the cut.
    let mut st = st0.clone();
    let mut d = Drag::begin_xfade_edge(&mut st, l, 161.0, y, true).expect("the left edge zone");
    assert_eq!(d.kind, DragKind::XfadeIn);
    d.update(&st, 145.0, y);
    assert_eq!(d.finish(&mut st), Ok(()));
    assert_eq!((st.timeline.audio_xfades[0].at, st.timeline.audio_xfades[0].dur), (20, 10));
    // The middle is a clip press, not an edge drag.
    let mut st = st0.clone();
    assert!(Drag::begin_xfade_edge(&mut st, l, 192.0, y, true).is_none());
    assert_eq!(st.selection, None);
    // Past the outgoing's handle (20 ticks): refused by name, nothing changes.
    let mut st = st0.clone();
    let mut d = Drag::begin_xfade_edge(&mut st, l, 222.0, y, true).unwrap();
    d.update(&st, 222.0 + 13.0 * 8.0, y);
    assert!(d.shown(&st).is_none());
    assert_eq!(d.finish(&mut st), Err(EditError::Handle));
    assert_eq!((&st.timeline, st.undo_depth()), (&st0.timeline, st0.undo_depth()));
    // Snap on: the right edge (28) carried 94 px lands 2 px short of the incoming clip's end (40) and snaps there.
    let mut st = st0.clone();
    let mut d = Drag::begin_xfade_edge(&mut st, l, 222.0, y, false).unwrap();
    d.update(&st, 222.0 + 94.0, y);
    assert_eq!((d.delta, d.guide), (12, Some(40)));
    assert_eq!(d.finish(&mut st), Ok(()));
    assert_eq!(st.timeline.audio_xfades[0].dur, 20);
    println!("striaxfadethree: right +2 -> 10, left -2 -> 10, middle none, +13 refused (Handle), snap -> 40 (dur 20)");
}
