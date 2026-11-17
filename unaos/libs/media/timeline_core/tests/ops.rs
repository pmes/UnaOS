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

// ---- STRIAROLLEDIT (LEDGER SR851): the roll edit on one cut (named tail block) ----

/// base() with a third clip butted: V1 A[0,10)@0 | B[20,30)@10 | A[50,60)@20, a 4-tick dissolve on the B|A cut at 20.
fn striarolledit_three() -> Timeline {
    let (mut tl, a, _b) = base();
    tl.tracks[0].clips.push(Clip::new(a, 50, 60, 20));
    tl.add_transition(0, 20, timeline_core::TransitionKind::Dissolve, 4).unwrap()
}

#[test]
fn striarolledit_roll_moves_one_cut() {
    let tl = striarolledit_three();
    let (a, b) = (SourceId(0), SourceId(1));
    // +3: A's out and B's in move together, the third clip and the later dissolve stay, the track's length stays
    let s = tl.roll(0, 0, 3).unwrap();
    assert_eq!(s.tracks[0].clips, vec![Clip::new(a, 0, 13, 0), Clip::new(b, 23, 30, 13), Clip::new(a, 50, 60, 20)]);
    assert_eq!(s.end(), tl.end());
    assert_eq!(s.tracks[0].transitions, tl.tracks[0].transitions, "the dissolve past B is at its original tick");
    assert_eq!(s.tracks[1], tl.tracks[1], "A1 does not move");
    // -3: the mirror; a roll and its roll-back give the original
    let m = tl.roll(0, 0, -3).unwrap();
    assert_eq!(m.tracks[0].clips, vec![Clip::new(a, 0, 7, 0), Clip::new(b, 17, 30, 7), Clip::new(a, 50, 60, 20)]);
    assert_eq!(m.tracks[0].transitions, tl.tracks[0].transitions);
    assert_eq!(s.roll(0, 0, -3), Ok(tl.clone()));
    // the EDL round-trips the rolled timeline
    let text = timeline_core::edl::write(&s, "ROLL").unwrap();
    let back = timeline_core::edl::parse(&text, 25).unwrap();
    let video = |t: &Timeline| t.tracks.iter().find(|k| k.kind == Kind::Video).map(|k| (k.clips.clone(), k.transitions.clone()));
    assert_eq!((video(&back), timeline_core::edl::write(&back, "ROLL").unwrap()), (video(&s), text));
}

#[test]
fn striarolledit_refusals_by_name() {
    let tl = striarolledit_three();
    let before = tl.clone();
    // B to nothing, A to nothing, A past its source (100 ticks), no track
    assert_eq!(tl.roll(0, 0, 10), Err(EditError::Empty));
    assert_eq!(tl.roll(0, 0, -10), Err(EditError::Empty));
    assert_eq!(tl.roll(0, 0, 91), Err(EditError::OutOfSource));
    assert_eq!(tl.roll(4, 0, 1), Err(EditError::NoTrack));
    // the track's last clip has no next clip, no such clip at all
    assert_eq!(tl.roll(0, 2, 1), Err(EditError::NoClip));
    assert_eq!(tl.roll(0, 3, 1), Err(EditError::NoClip));
    assert_eq!(tl, before);
    // not butted: B two ticks after A's end
    let (mut gap, a, b) = base();
    gap.tracks[0].clips = vec![Clip::new(a, 0, 10, 0), Clip::new(b, 20, 30, 12)];
    assert_eq!(gap.roll(0, 0, 1), Err(EditError::NoClip));
    // B's in before its source's start
    let (mut near, a, b) = base();
    near.tracks[0].clips = vec![Clip::new(a, 0, 10, 0), Clip::new(b, 2, 12, 10)];
    assert_eq!(near.roll(0, 0, -3), Err(EditError::Negative));
    assert!(near.roll(0, 0, -2).is_ok());
}

#[test]
fn striarolledit_dissolve_moves_with_the_cut_and_fits() {
    use timeline_core::TransitionKind;
    // V1: A[0,20)@0 of a 30-tick source (a 10-tick handle) -> B[10,30)@20 under a 9-tick dissolve
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 30);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 20, 0), Clip::new(b, 10, 30, 20)];
    let tl = tl.add_transition(0, 20, TransitionKind::Dissolve, 9).unwrap();
    let x = |t: &Timeline| (t.tracks[0].transitions[0].at, t.tracks[0].transitions[0].dur, t.tracks[0].transitions[0].want);
    // +5: the dissolve moves with the cut and fits A's 5-tick handle (slide's rule), asking 9
    let s = tl.roll(0, 0, 5).unwrap();
    assert_eq!(s.tracks[0].clips, vec![Clip::new(a, 0, 25, 0), Clip::new(b, 15, 30, 25)]);
    assert_eq!((x(&s), s.end()), ((25, 5, Some(9)), tl.end()));
    // rolled back it grows to its ask: the original
    assert_eq!(s.roll(0, 0, -5), Ok(tl.clone()));
    // no handle left, past the source
    assert_eq!(tl.roll(0, 0, 10), Err(EditError::Handle));
    assert_eq!(tl.roll(0, 0, 11), Err(EditError::OutOfSource));
    // -5: the dissolve at 15, whole
    assert_eq!(x(&tl.roll(0, 0, -5).unwrap()), (15, 9, None));
}

#[test]
fn striarolledit_the_editor_rolls_once_per_word() {
    use timeline_core::editor::EditorState;
    let tl = striarolledit_three();
    let mut st = EditorState::new(tl.clone());
    st.select(0, 0).unwrap();
    for _ in 0..3 {
        assert_eq!(st.verb("roll+"), Some(Ok(())));
    }
    assert_eq!((st.timeline.clone(), st.undo_depth()), (tl.roll(0, 0, 3).unwrap(), 3));
    assert_eq!(st.verb("undo"), Some(Ok(())));
    assert_eq!(st.timeline, tl.roll(0, 0, 2).unwrap());
    assert_eq!(st.verb("roll -2"), Some(Ok(())));
    assert_eq!(st.timeline, tl);
    assert_eq!(st.verb("roll-"), Some(Ok(())));
    assert_eq!((st.timeline.clone(), st.selection), (tl.roll(0, 0, -1).unwrap(), Some((0, 0))));
    // refused: the state and the undo stack unchanged
    let depth = st.undo_depth();
    assert_eq!(st.verb("roll 20"), Some(Err(EditError::Empty)));
    assert_eq!(st.undo_depth(), depth);
    assert_eq!(st.verb("roll sideways"), None);
    assert_eq!(st.verb("roll"), None);
    st.deselect();
    assert_eq!(st.verb("roll+"), Some(Err(EditError::NoClip)));
}

// ---- STRIAZOOMTOFIT (LEDGER SR859): the fit zoom over the whole timeline or the selection set (named tail block) ----

/// The rule: the largest integer zoom that puts the range in `w` px (ZOOM_MIN when it cannot fit, ZOOM_MAX at most), the
/// origin the range's start; nothing to fit is NoClip with the zoom unchanged; never an undo step, the timeline untouched.
#[test]
fn striazoomtofit_fit_rule() {
    use timeline_core::editor::{EditorState, ZOOM_MAX, ZOOM_MIN};
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(a, 13, 23, 0), Clip::new(b, 0, 10, 10), Clip::new(a, 23, 33, 25)];
    let mut st = EditorState::new(tl.clone());
    assert_eq!(st.fit(320, false), Ok(0));
    assert_eq!(st.zoom, 9); // 320 / 35
    assert_eq!(st.fit(320, true), Err(EditError::NoClip), "an empty set");
    assert_eq!(st.zoom, 9, "a refusal leaves the zoom");
    st.select(0, 1).unwrap();
    assert_eq!(st.fit(320, true), Ok(10));
    assert_eq!(st.zoom, 32); // 320 / 10
    st.select_add(0, 0).unwrap();
    assert_eq!(st.fit(320, true), Ok(0));
    assert_eq!(st.zoom, 16); // 320 / 20
    assert_eq!(st.fit(20, false), Ok(0));
    assert_eq!(st.zoom, ZOOM_MIN, "35 ticks in 20 px: the smallest zoom, the origin at the start");
    assert_eq!(st.fit(10_000, false), Ok(0));
    assert_eq!(st.zoom, ZOOM_MAX);
    assert_eq!((st.undo_depth(), &st.timeline), (0, &tl), "a view change, not an edit");
    let mut empty = EditorState::new(Timeline::new(25));
    assert_eq!(empty.fit(320, false), Err(EditError::NoClip), "an empty timeline");
    assert_eq!(empty.zoom, timeline_core::editor::ZOOM_DEFAULT);
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

// ---- STRIACOPYPASTE (LEDGER SR854): paste through place, refused by name, never overwriting (named tail block) ----
#[test]
fn striacopypaste_paste_lays_through_place_and_round_trips() {
    let (tl, a, b) = base(); // V1: A[0,10)@0 B[20,30)@10
    let items = [(0, Kind::Video, Clip::new(b, 20, 30, 30)), (1, Kind::Audio, Clip::new(a, 0, 10, 5).gained(-600))];
    let p = tl.paste(&items).unwrap();
    assert_eq!(p, tl.place(0, items[0].2, Mode::Overwrite).unwrap().place(1, items[1].2, Mode::Overwrite).unwrap());
    assert_eq!(p.tracks[0].clips, vec![Clip::new(a, 0, 10, 0), Clip::new(b, 20, 30, 10), Clip::new(b, 20, 30, 30)]);
    assert_eq!(p.tracks[1].clips, vec![Clip::new(a, 0, 10, 5).gained(-600)], "gain rides the clip");
    let text = timeline_core::edl::write(&p, "PASTE").unwrap();
    let back = timeline_core::edl::parse(&text, 25).unwrap();
    let video = |t: &Timeline| t.tracks.iter().find(|k| k.kind == Kind::Video).map(|k| k.clips.clone());
    assert_eq!((video(&back), timeline_core::edl::write(&back, "PASTE").unwrap()), (video(&p), text));
}

#[test]
fn striacopypaste_paste_refusals_by_name() {
    let (tl, a, b) = base();
    let before = tl.clone();
    assert_eq!(tl.paste(&[]), Err(EditError::NoClip));
    assert_eq!(tl.paste(&[(2, Kind::Video, Clip::new(a, 0, 10, 40))]), Err(EditError::NoTrack));
    assert_eq!(tl.paste(&[(1, Kind::Video, Clip::new(a, 0, 10, 40))]), Err(EditError::Kind));
    // meeting a clip by one tick is an overlap; butting is not; the whole paste is refused together
    assert_eq!(tl.paste(&[(0, Kind::Video, Clip::new(a, 0, 10, 19))]), Err(EditError::Overlap));
    assert!(tl.paste(&[(0, Kind::Video, Clip::new(a, 0, 10, 20))]).is_ok());
    assert_eq!(tl.paste(&[(0, Kind::Video, Clip::new(a, 0, 10, 40)), (0, Kind::Video, Clip::new(b, 0, 10, 45))]), Err(EditError::Overlap));
    assert_eq!(tl.paste(&[(0, Kind::Video, Clip::new(b, 50, 70, 40))]), Err(EditError::OutOfSource), "place's own validate");
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
    assert_eq!(to.close_gaps(0), Err(EditError::Transition));
    let pushed = to.insert_gap(0, 12, 5).unwrap();
    assert_eq!((pushed.tracks[0].transitions[0].at, pushed.tracks[0].clips[1].at), (10, 25));
    let from = k.add_transition_from_black(0, 20, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    assert_eq!(from.close_gaps(0), Err(EditError::Transition));
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
