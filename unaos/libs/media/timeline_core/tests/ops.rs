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

// ---- STRIAGAPCLOSE (LEDGER SR857): close every gap of a track, insert a gap at a tick (named tail block) ----

/// A (100 ticks), B (60 ticks). V1: gap 0..5, A[0,10)@5, B[20,30)@15 under a 3-tick dissolve at 15, gap 25..30,
/// A[40,50)@30. A1: A[0,10)@0, gap 10..20, B[20,30)@20, A[50,60)@30 under a 4-tick explicit audio crossfade at 30.
fn striagapclose_base() -> (Timeline, SourceId, SourceId) {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 100);
    let b = tl.add_source("B", 60);
    tl.add_track(Kind::Video);
    tl.add_track(Kind::Audio);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 10, 5), Clip::new(b, 20, 30, 15), Clip::new(a, 40, 50, 30)];
    tl.tracks[1].clips = vec![Clip::new(a, 0, 10, 0), Clip::new(b, 20, 30, 20), Clip::new(a, 50, 60, 30)];
    let tl = tl.add_transition(0, 15, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    let tl = tl.add_audio_xfade(1, 30, 4, timeline_core::Curve::Linear).unwrap();
    (tl, a, b)
}

/// Every gap of the one track closes, the leading one included; the other track never moves; a track with no gap is an
/// equal value.
#[test]
fn striagapclose_close_gaps_closes_every_gap_on_one_track() {
    let (tl, a, b) = striagapclose_base();
    let c = tl.close_gaps(0).unwrap();
    assert_eq!(c.tracks[0].clips, vec![Clip::new(a, 0, 10, 0), Clip::new(b, 20, 30, 10), Clip::new(a, 40, 50, 20)]);
    assert_eq!(c.tracks[1], tl.tracks[1]);
    assert_eq!(c.audio_xfades, tl.audio_xfades);
    assert_eq!(c.close_gaps(0).unwrap(), c);
    let d = tl.close_gaps(1).unwrap();
    assert_eq!(d.tracks[1].clips, vec![Clip::new(a, 0, 10, 0), Clip::new(b, 20, 30, 10), Clip::new(a, 50, 60, 20)]);
    assert_eq!(d.tracks[0], tl.tracks[0]);
    println!("striagapclose: V1 {:?}", c.tracks[0].clips);
}

/// A gap inside a clip splits it and pushes the tail and the rest; inside a gap it pushes the next clip; at a clip start
/// it pushes that clip.
#[test]
fn striagapclose_insert_gap_splits_and_pushes() {
    let (tl, a, b) = striagapclose_base();
    let g = tl.insert_gap(0, 7, 4).unwrap();
    assert_eq!(g.tracks[0].clips, vec![Clip::new(a, 0, 2, 5), Clip::new(a, 2, 10, 11), Clip::new(b, 20, 30, 19), Clip::new(a, 40, 50, 34)]);
    assert_eq!(g.tracks[1], tl.tracks[1]);
    let g = tl.insert_gap(0, 27, 2).unwrap();
    assert_eq!(g.tracks[0].clips[2], Clip::new(a, 40, 50, 32));
    assert_eq!(g.tracks[0].clips[..2], tl.tracks[0].clips[..2]);
    let g = tl.insert_gap(0, 5, 3).unwrap();
    assert_eq!(g.tracks[0].clips[0], Clip::new(a, 0, 10, 8));
}

/// Each refusal by name; the input is untouched.
#[test]
fn striagapclose_refusals_by_name() {
    let (tl, _, _) = striagapclose_base();
    let before = tl.clone();
    assert_eq!(tl.close_gaps(9), Err(EditError::NoTrack));
    assert_eq!(tl.insert_gap(9, 0, 1), Err(EditError::NoTrack));
    assert_eq!(tl.insert_gap(0, 0, 0), Err(EditError::Empty));
    assert_eq!(tl.insert_gap(0, 0, -2), Err(EditError::Empty));
    assert_eq!(tl.insert_gap(0, -1, 1), Err(EditError::Negative));
    assert_eq!(tl.insert_gap(0, 40, 1), Err(EditError::NoClip));
    assert_eq!(tl.insert_gap(0, 45, 1), Err(EditError::NoClip));
    assert_eq!(tl, before);
}

/// A transition or explicit audio crossfade on a clip that moves goes with it (the shared direct shift); a black
/// transition that would gain a neighbour, and a gap opened on a transitioned cut, are refused by name (`Transition`).
#[test]
fn striagapclose_transitions_and_crossfades_move_with_their_clips() {
    let (tl, a, b) = striagapclose_base();
    assert_eq!(tl.close_gaps(0).unwrap().tracks[0].transitions[0].at, 10);
    assert_eq!(tl.insert_gap(0, 7, 4).unwrap().tracks[0].transitions[0].at, 19);
    assert_eq!(tl.close_gaps(1).unwrap().audio_xfades[0].at, 20);
    assert_eq!(tl.insert_gap(1, 25, 3).unwrap().audio_xfades[0].at, 33);
    assert_eq!(tl.insert_gap(0, 15, 1), Err(EditError::Transition));
    assert_eq!(tl.insert_gap(1, 30, 1), Err(EditError::Transition));
    // black: A[0,10)@0, B[0,10)@20 on V1
    let mut k = Timeline::new(25);
    let (ka, kb) = (k.add_source("A", 100), k.add_source("B", 60));
    k.add_track(Kind::Video);
    k.tracks[0].clips = vec![Clip::new(ka, 0, 10, 0), Clip::new(kb, 0, 10, 20)];
    let to = k.add_transition_to_black(0, 10, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    assert_eq!(to.close_gaps(0).map(|c| (c.tracks[0].clips[1].at, c.tracks[0].transitions.len())), Ok((10, 0))); // STRIAGAPSBLACK (SR909): the closed gap's to-black is removed, no longer refused
    let pushed = to.insert_gap(0, 12, 5).unwrap();
    assert_eq!((pushed.tracks[0].transitions[0].at, pushed.tracks[0].clips[1].at), (10, 25));
    let from = k.add_transition_from_black(0, 20, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    assert_eq!(from.close_gaps(0).map(|c| (c.tracks[0].clips[1].at, c.tracks[0].transitions.len())), Ok((10, 0))); // STRIAGAPSBLACK (SR909): the closed gap's from-black is removed, no longer refused
    // a from-black on the LEADING gap stays valid when it closes: nothing ends at 0
    let mut l = Timeline::new(25);
    let lb = l.add_source("B", 60);
    l.add_track(Kind::Video);
    l.tracks[0].clips = vec![Clip::new(lb, 0, 10, 20)];
    let l = l.add_transition_from_black(0, 20, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    let lc = l.close_gaps(0).unwrap();
    assert_eq!((lc.tracks[0].clips[0].at, lc.tracks[0].transitions[0].at), (0, 0));
    let _ = (a, b);
}

/// The document after each op round-trips through the EDL byte-exact.
#[test]
fn striagapclose_edl_round_trip() {
    let (tl, _, _) = striagapclose_base();
    for t in [tl.close_gaps(0).unwrap(), tl.close_gaps(1).unwrap(), tl.insert_gap(0, 7, 4).unwrap()] {
        let text = timeline_core::edl::write(&t, "GAPS").unwrap();
        let back = timeline_core::edl::parse(&text, 25).unwrap();
        let video = |t: &Timeline| t.tracks.iter().find(|k| k.kind == Kind::Video).map(|k| (k.clips.clone(), k.transitions.clone()));
        assert_eq!((video(&back), timeline_core::edl::write(&back, "GAPS").unwrap()), (video(&t), text));
    }
}

/// `close-gaps` / `gap <n>` through THE verb dispatch: one commit each, undo restores, an equal value commits nothing,
/// nothing selected is `NoClip`, the selection follows its clip across a split, malformed words are not this verb.
#[test]
fn striagapclose_editor_one_undo_step() {
    use timeline_core::editor::EditorState;
    let (tl, _, b) = striagapclose_base();
    let mut st = EditorState::new(tl.clone());
    st.select(0, 1).unwrap();
    assert_eq!(st.verb("close-gaps"), Some(Ok(())));
    assert_eq!((st.timeline.clone(), st.undo_depth()), (tl.close_gaps(0).unwrap(), 1));
    assert_eq!(st.verb("close-gaps"), Some(Ok(())));
    assert_eq!(st.undo_depth(), 1);
    assert!(st.undo());
    assert_eq!(st.timeline, tl);
    st.seek(7);
    assert_eq!(st.verb("gap 4"), Some(Ok(())));
    assert_eq!((st.timeline.clone(), st.undo_depth()), (tl.insert_gap(0, 7, 4).unwrap(), 1));
    assert_eq!(st.selection, Some((0, 2)));
    assert_eq!(st.selected(), Some(Clip::new(b, 20, 30, 19)));
    assert!(st.undo());
    assert_eq!(st.timeline, tl);
    assert_eq!(st.verb("gap 0"), Some(Err(EditError::Empty)));
    assert_eq!(st.verb("gap x"), None);
    assert_eq!(st.verb("gap -1"), None);
    assert_eq!(st.verb("gap 1 2"), None);
    st.deselect();
    assert_eq!(st.verb("close-gaps"), Some(Err(EditError::NoClip)));
    assert_eq!(st.verb("gap 3"), Some(Err(EditError::NoClip)));
    assert_eq!(st.timeline, tl);
}

// ---- STRIAGAPSBLACK (LEDGER SR909): close-gaps removes a closed gap's black fades; a bare `gap` opens one second (named tail block) ----

/// V1: A[0,10)@0 to black (3) at 10, gap 10..20, from black (3) at 20 on B[0,10)@20, a 2-tick dissolve on the cut at 30 into
/// A[20,30)@30, gap 40..45, B[20,30)@45 to black (2) at 55 (trailing space). A1 untouched.
fn striagapsblack_base() -> Timeline {
    let mut k = Timeline::new(25);
    let (a, b) = (k.add_source("A", 100), k.add_source("B", 60));
    k.add_track(Kind::Video);
    k.add_track(Kind::Audio);
    k.tracks[0].clips = vec![Clip::new(a, 0, 10, 0), Clip::new(b, 0, 10, 20), Clip::new(a, 20, 30, 30), Clip::new(b, 20, 30, 45)];
    k.tracks[1].clips = vec![Clip::new(a, 0, 10, 0)];
    let k = k.add_transition_to_black(0, 10, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    let k = k.add_transition_from_black(0, 20, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    let k = k.add_transition(0, 30, timeline_core::TransitionKind::Dissolve, 2).unwrap();
    k.add_transition_to_black(0, 55, timeline_core::TransitionKind::Dissolve, 2).unwrap()
}

/// Both black fades of the closed gap go (one value); the dissolve after it moves with its cut; the to-black on the last
/// clip's trailing space stays (trailing space is no gap); the document round-trips through the EDL.
#[test]
fn striagapsblack_close_gaps_removes_the_closed_gaps_black_fades() {
    let k = striagapsblack_base();
    let c = k.close_gaps(0).unwrap();
    let ats: Vec<i64> = c.tracks[0].clips.iter().map(|x| x.at).collect();
    assert_eq!(ats, vec![0, 10, 20, 30]);
    let xs: Vec<(i64, bool, bool)> = c.tracks[0].transitions.iter().map(|x| (x.at, x.from_black, x.to_black)).collect();
    assert_eq!(xs, vec![(20, false, false), (40, false, true)]);
    assert_eq!(c.tracks[1], k.tracks[1]);
    assert_eq!(c.validate(), Ok(()));
    let text = timeline_core::edl::write(&c, "GAPSBLACK").unwrap();
    let back = timeline_core::edl::parse(&text, 25).unwrap();
    assert_eq!(timeline_core::edl::write(&back, "GAPSBLACK").unwrap(), text);
    println!("striagapsblack: V1 {:?} transitions {:?}", ats, xs);
}

/// `gap_count` counts what `close_gaps` closes (the leading gap included); none after the close; a bad track is 0.
#[test]
fn striagapsblack_gap_count() {
    let k = striagapsblack_base();
    assert_eq!(k.gap_count(0), 2);
    assert_eq!(k.close_gaps(0).unwrap().gap_count(0), 0);
    assert_eq!(k.gap_count(1), 0);
    assert_eq!(k.gap_count(9), 0);
    let (tl, _, _) = striagapclose_base();
    assert_eq!(tl.gap_count(0), 2);
}

/// A bare `gap` through THE verb dispatch opens one second (25 ticks at 25 fps) at the playhead: one commit, undo restores;
/// nothing selected is `NoClip`; `close-gaps` that drops black fades is one undo step and undo brings them back.
#[test]
fn striagapsblack_editor_bare_gap_is_one_second() {
    use timeline_core::editor::EditorState;
    let k = striagapsblack_base();
    let mut st = EditorState::new(k.clone());
    st.select(0, 1).unwrap();
    st.seek(20);
    assert_eq!(st.verb("gap"), Some(Ok(())));
    assert_eq!((st.timeline.clone(), st.undo_depth()), (k.insert_gap(0, 20, 25).unwrap(), 1));
    assert!(st.undo());
    assert_eq!(st.verb("close-gaps"), Some(Ok(())));
    assert_eq!((st.timeline.clone(), st.undo_depth()), (k.close_gaps(0).unwrap(), 1));
    assert!(st.undo());
    assert_eq!(st.timeline, k);
    st.deselect();
    assert_eq!(st.verb("gap"), Some(Err(EditError::NoClip)));
    assert_eq!(st.timeline, k);
}
