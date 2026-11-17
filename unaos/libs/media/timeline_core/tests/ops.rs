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

// ---- STRIANUDGE (LEDGER SR855): `nudge <d>` on THE dispatch, over move_set (named tail block) ----

/// `nudge <d>` is `move_many` of the set by `d` ticks as ONE undo step; a refusal (negative, overlap, nothing selected)
/// leaves the state; `nudge-` / `nudge+` are unchanged; any other shape is not an editor verb.
#[test]
fn strianudge_verb_moves_the_set_by_a_count() {
    use timeline_core::editor::EditorState;
    let (tl, _, _) = base();
    let mut st = EditorState::new(tl.clone());
    st.select(0, 1).unwrap();
    assert_eq!(st.verb("nudge 10"), Some(Ok(())));
    assert_eq!(st.timeline, tl.move_many(&[(0, 1)], 10).unwrap());
    assert_eq!((st.undo_depth(), st.selection, st.set.clone()), (1, Some((0, 1)), vec![(0, 1)]));
    assert!(st.undo());
    assert_eq!(st.timeline, tl);
    st.select_many(&[(0, 0), (0, 1)], false);
    assert_eq!(st.verb("nudge 5"), Some(Ok(())));
    let five = tl.move_many(&[(0, 0), (0, 1)], 5).unwrap();
    assert_eq!((&st.timeline, st.set.clone()), (&five, vec![(0, 0), (0, 1)]));
    let depth = st.undo_depth();
    assert_eq!(st.verb("nudge -50"), Some(Err(EditError::Negative)));
    assert_eq!((&st.timeline, st.undo_depth()), (&five, depth));
    let mut st = EditorState::new(tl.clone());
    st.select(0, 0).unwrap();
    assert_eq!(st.verb("nudge 3"), Some(Err(EditError::Overlap)));
    assert_eq!((&st.timeline, st.undo_depth()), (&tl, 0));
    for v in ["nudge 0", "nudge x", "nudge 1 2", "nudge", "nudge+7"] {
        assert_eq!(st.verb(v), None, "{v}");
    }
    st.select(0, 1).unwrap();
    assert_eq!(st.verb("nudge+"), Some(Ok(())));
    assert_eq!(st.timeline, tl.move_many(&[(0, 1)], 1).unwrap());
    st.deselect();
    assert_eq!(st.verb("nudge 1"), Some(Err(EditError::NoClip)));
    println!("strianudge: nudge 10 / nudge 5 over a set of two, one commit each; -50 Negative, +3 Overlap, deselected NoClip");
}

// ---- STRIATRACKADDREMOVE (LEDGER SR856): add / remove a track, the canonical slot, the carried indices, the EDL (named tail block) ----

mod striatrackaddremove {
    use timeline_core::editor::EditorState;
    use timeline_core::{Clip, Curve, EditError, Kind, Timeline, edl};

    /// V1 A[0,20)@0 B[0,20)@20; A1 the same with an equal-power crossfade on its cut; A2 A[0,10)@0.
    fn three() -> Timeline {
        let mut tl = Timeline::new(25);
        let (a, b) = (tl.add_source("A", 100), tl.add_source("B", 100));
        let (v1, a1, a2) = (tl.add_track(Kind::Video), tl.add_track(Kind::Audio), tl.add_track(Kind::Audio));
        tl.tracks[v1].clips = vec![Clip::new(a, 0, 20, 0), Clip::new(b, 0, 20, 20)];
        tl.tracks[a1].clips = tl.tracks[v1].clips.clone();
        tl.tracks[a2].clips = vec![Clip::new(a, 0, 10, 0)];
        tl.add_audio_xfade(a1, 20, 4, Curve::EqualPower).unwrap()
    }

    /// A video track goes after the last video track (before the audio), an audio one after the last audio; the xfade's
    /// track index follows its track; a tenth of a kind is `Tracks`; with no track of a kind, video first, audio last.
    #[test]
    fn striatrackaddremove_insert_slots_and_cap() {
        let tl = three();
        let v = tl.insert_track(Kind::Video).unwrap();
        assert_eq!(v.tracks.iter().map(|t| t.kind).collect::<Vec<_>>(), vec![Kind::Video, Kind::Video, Kind::Audio, Kind::Audio]);
        assert!(v.tracks[1].clips.is_empty());
        assert_eq!((v.audio_xfades[0].track, v.tracks[2].clips.clone()), (2, tl.tracks[1].clips.clone()));
        let a = tl.insert_track(Kind::Audio).unwrap();
        assert_eq!((a.tracks.len(), a.tracks[3].kind, a.audio_xfades[0].track), (4, Kind::Audio, 1));
        assert_eq!((a.track_of(Kind::Audio, 3), a.track_word(3).as_deref(), a.track_word(0).as_deref()), (Some(3), Some("A3"), Some("V")));
        let mut full = tl.clone();
        for _ in 0..8 {
            full = full.insert_track(Kind::Video).unwrap();
        }
        assert_eq!(full.insert_track(Kind::Video), Err(EditError::Tracks));
        assert_eq!(full.insert_track(Kind::Audio).map(|t| t.tracks.len()), Ok(12));
        let empty = Timeline::new(25);
        assert_eq!((empty.track_slot(Kind::Video), empty.track_slot(Kind::Audio)), (0, 0));
        let only_a = empty.insert_track(Kind::Audio).unwrap();
        assert_eq!((only_a.track_slot(Kind::Video), only_a.track_slot(Kind::Audio)), (0, 1));
    }

    /// Removing a track takes its clips, transitions and explicit crossfades; a later crossfade moves down one track;
    /// no such track is `NoTrack`; the source timeline is untouched.
    #[test]
    fn striatrackaddremove_remove_shifts_xfades_and_drops_its_own() {
        let tl = three();
        let before = tl.clone();
        let r = tl.remove_track(0).unwrap();
        assert_eq!((r.tracks.len(), r.audio_xfades[0].track), (2, 0));
        assert_eq!(r.tracks[0], tl.tracks[1]);
        let own = tl.remove_track(1).unwrap();
        assert!(own.audio_xfades.is_empty());
        assert_eq!(own.tracks[1], tl.tracks[2]);
        let last = tl.remove_track(2).unwrap();
        assert_eq!(last.audio_xfades, tl.audio_xfades);
        assert_eq!(tl.remove_track(3), Err(EditError::NoTrack));
        assert_eq!(tl, before);
    }

    /// The words through THE dispatch: one undo step each; the selection and set follow their clips; a removed track's
    /// selection goes; each refusal by name; other `track+` shapes are not editor verbs.
    #[test]
    fn striatrackaddremove_verb_one_undo_step_and_selection_follows() {
        let tl = three();
        let mut st = EditorState::new(tl.clone());
        st.select(1, 1).unwrap();
        st.select_add(2, 0).unwrap();
        let sel = st.selected();
        assert_eq!(st.verb("track+ video"), Some(Ok(())));
        assert_eq!((st.undo_depth(), st.timeline.tracks.len()), (1, 4));
        assert_eq!((st.selected(), st.set.clone()), (sel, vec![(2, 1), (3, 0)]));
        assert_eq!(st.verb("track- V2"), Some(Ok(())));
        assert_eq!((st.timeline.clone(), st.selected(), st.undo_depth()), (tl.clone(), sel, 2));
        assert_eq!(st.verb("track-"), Some(Ok(())), "bare: the primary's track (A2) with its clip");
        assert_eq!((st.timeline.tracks.len(), st.selection, st.set.clone()), (2, Some((1, 1)), vec![(1, 1)]), "A2's entry dropped");
        assert_eq!(st.timeline, tl.remove_track(2).unwrap());
        assert!(st.undo());
        assert_eq!(st.timeline, tl);
        st.deselect();
        assert_eq!(st.verb("track-"), Some(Err(EditError::NoTrack)));
        assert_eq!(st.verb("track- A3"), Some(Err(EditError::NoTrack)));
        assert_eq!(st.verb("track- X"), Some(Err(EditError::NoTrack)));
        assert_eq!(st.verb("track- V V"), Some(Err(EditError::NoTrack)));
        assert_eq!(st.track_minus_target("track- A1"), Some(Ok(1)));
        assert_eq!(st.verb("track+"), None);
        assert_eq!(st.verb("track+ title"), None);
        for _ in 0..7 {
            assert_eq!(st.verb("track+ audio"), Some(Ok(())));
        }
        let depth = st.undo_depth();
        assert_eq!(st.verb("track+ audio"), Some(Err(EditError::Tracks)));
        assert_eq!(st.undo_depth(), depth, "a refusal commits nothing");
    }

    /// An empty trailing track survives a save: `* UNA TRACKS` is written last in the header only then, parse restores
    /// the track, write(parse(write)) is byte-identical; a timeline without one writes exactly as before; refusals.
    #[test]
    fn striatrackaddremove_edl_empty_trailing_track_round_trips() {
        let tl = three();
        let plain = edl::write(&tl, "T").unwrap();
        assert!(!plain.contains("* UNA TRACKS"));
        let mid = tl.insert_track(Kind::Video).unwrap().remove_track(0).unwrap().insert_track(Kind::Video).unwrap();
        let both = tl.insert_track(Kind::Video).unwrap().insert_track(Kind::Audio).unwrap();
        let text = edl::write(&both, "T").unwrap();
        assert!(text.contains("* UNA TRACKS 2 3\n\n001"), "{text}");
        assert_eq!(text.replace("* UNA TRACKS 2 3\n", ""), plain, "only the one line differs");
        let back = edl::parse(&text, 25).unwrap();
        assert_eq!(back, both);
        assert_eq!(edl::write(&back, "T").unwrap(), text);
        assert_eq!(edl::parse(&edl::write(&mid, "T").unwrap(), 25).unwrap(), mid);
        let mut empty = Timeline::new(25);
        empty = empty.insert_track(Kind::Video).unwrap();
        let e = edl::write(&empty, "E").unwrap();
        assert!(e.contains("* UNA TRACKS 1 0\n"), "{e}");
        assert_eq!(edl::parse(&e, 25).unwrap(), empty);
        let at = text.lines().position(|l| l.starts_with("* UNA TRACKS")).unwrap() + 1;
        for bad in ["* UNA TRACKS 2", "* UNA TRACKS 2 3 4", "* UNA TRACKS x 3", "* UNA TRACKS 10 3", "* UNA TRACKS 2 3\n* UNA TRACKS 2 3"] {
            let t = text.replace("* UNA TRACKS 2 3", bad);
            let line = if bad.contains('\n') { at + 1 } else { at };
            assert_eq!(edl::parse(&t, 25), Err(edl::EdlError::Syntax { line }), "{bad}");
        }
        assert_eq!(edl::parse(&text.replace("* UNA TRACKS 2 3", "* UNA TRACKS 0 0"), 25).unwrap(), tl, "a floor below the events: theirs");
    }
}

// ---- STRIACROP (LEDGER SR867): the per-clip crop: geometry, the crop pass, the op, the word and the EDL line (named tail block) ----
mod striacrop {
    use super::base;
    use timeline_core::edl::{self, EdlError};
    use timeline_core::editor::EditorState;
    use timeline_core::{Clip, Crop, EditError, SizePolicy, TransitionKind, crop_frame};

    const C: Crop = Crop { l: 250, t: 0, r: 250, b: 0 };

    #[test]
    fn striacrop_rect_and_validity() {
        let r = Crop { l: 100, t: 200, r: 300, b: 400 }.rect(1000, 500);
        assert_eq!((r.x, r.y, r.w, r.h), (100, 100, 600, 200));
        let r = Crop::NONE.rect(7, 3);
        assert_eq!((r.x, r.y, r.w, r.h), (0, 0, 7, 3));
        let r = Crop { l: 999, t: 0, r: 0, b: 999 }.rect(4, 2);
        assert_eq!((r.x, r.y, r.w, r.h), (3, 0, 1, 1), "at least one sample, inside the frame");
        assert!(Crop { l: 499, t: 0, r: 500, b: 999 }.ok());
        assert!(!Crop { l: 500, t: 0, r: 500, b: 0 }.ok());
        assert!(!Crop { l: 0, t: 1000, r: 0, b: 0 }.ok());
        assert_eq!(Crop::default(), Crop::NONE);
        assert_eq!(Clip::new(timeline_core::SourceId(0), 0, 1, 0).crop, Crop::NONE);
    }

    #[test]
    fn striacrop_crop_frame_by_policy() {
        let px: Vec<u32> = (0..8).collect(); // 4 x 2: 0 1 2 3 / 4 5 6 7
        let l = Crop { l: 500, t: 0, r: 0, b: 0 };
        assert_eq!(crop_frame(&px, 4, 2, l, SizePolicy::Stretch, 99), vec![2, 2, 3, 3, 6, 6, 7, 7]);
        assert_eq!(crop_frame(&px, 4, 2, l, SizePolicy::Letterbox, 99), vec![99, 2, 3, 99, 99, 6, 7, 99]);
        assert_eq!(crop_frame(&px, 4, 2, l, SizePolicy::Adopt, 99), vec![99, 2, 3, 99, 99, 6, 7, 99], "Adopt places as Letterbox");
        assert_eq!(crop_frame(&px, 4, 2, l, SizePolicy::Fill, 99), vec![2, 2, 3, 3, 2, 2, 3, 3]);
        assert_eq!(crop_frame(&px, 4, 2, Crop::NONE, SizePolicy::Letterbox, 99), px, "no crop: the frame");
        assert_eq!(crop_frame(&px, 4, 2, Crop { l: 0, t: 500, r: 0, b: 0 }, SizePolicy::Stretch, 99), vec![4, 5, 6, 7, 4, 5, 6, 7]);
        assert_eq!(crop_frame(&px[..5], 4, 2, l, SizePolicy::Stretch, 99), px[..5].to_vec(), "a short frame comes back as it was");
    }

    #[test]
    fn striacrop_set_crop_and_refusals() {
        let (tl, _, _) = base();
        let t2 = tl.set_crop(0, 1, C).unwrap();
        assert_eq!(t2.tracks[0].clips[1].crop, C);
        assert_eq!(t2.tracks[0].clips[0].crop, Crop::NONE);
        assert_eq!(tl.tracks[0].clips[1].crop, Crop::NONE, "pure");
        assert_eq!(tl.set_crop(0, 1, Crop { l: 500, t: 0, r: 500, b: 0 }), Err(EditError::Crop));
        assert_eq!(tl.set_crop(0, 9, C), Err(EditError::NoClip));
        assert_eq!(tl.set_crop(9, 0, C), Err(EditError::NoTrack));
        assert_eq!(tl.set_crop(1, 0, C), Err(EditError::Kind));
        assert_eq!(EditError::Crop.to_string(), "crop is l t r b in 1/1000 with l + r and t + b under 1000");
        let s = t2.split(0, 15).unwrap();
        assert_eq!((s.tracks[0].clips[1].crop, s.tracks[0].clips[2].crop), (C, C), "both halves keep the crop");
        let m = t2.move_clip(0, 1, 0, 40).unwrap();
        assert_eq!(m.tracks[0].clips[1].crop, C);
    }

    #[test]
    fn striacrop_crop_at_incoming_and_outgoing() {
        let (tl, _, _) = base();
        let tl = tl.set_crop(0, 0, C).unwrap().add_transition(0, 10, TransitionKind::Dissolve, 4).unwrap();
        assert_eq!(tl.crop_at(0, 5, false), C);
        assert_eq!(tl.crop_at(0, 11, false), Crop::NONE, "the incoming B");
        assert_eq!(tl.crop_at(0, 11, true), C, "the transition's outgoing A");
        assert_eq!(tl.crop_at(0, 14, true), Crop::NONE, "past the transition");
        assert_eq!(tl.crop_at(7, 5, false), Crop::NONE);
    }

    #[test]
    fn striacrop_verb_one_undo_step() {
        let (tl, _, _) = base();
        let mut st = EditorState::new(tl.clone());
        assert_eq!(st.verb("crop 250 0 250 0"), Some(Err(EditError::NoClip)));
        st.select(0, 1).unwrap();
        assert_eq!(st.verb("crop 250 0 250 0"), Some(Ok(())));
        assert_eq!((st.undo_depth(), st.selected().map(|c| c.crop)), (1, Some(C)));
        assert_eq!(st.verb("crop 250 0 250 0"), Some(Ok(())));
        assert_eq!(st.undo_depth(), 1, "the same crop commits nothing");
        for bad in ["crop 1 2 3", "crop 1 2 3 4 5", "crop a 0 0 0", "crop -1 0 0 0", "crop 600 0 400 0"] {
            assert_eq!(st.verb(bad), Some(Err(EditError::Crop)), "{bad}");
        }
        assert_eq!(st.undo_depth(), 1);
        assert!(st.undo());
        assert_eq!(st.timeline, tl);
        assert!(st.redo());
        assert_eq!(st.selected().map(|c| c.crop), Some(C));
        assert_eq!(st.verb("crop 0 0 0 0"), Some(Ok(())));
        assert_eq!((st.undo_depth(), st.selected().map(|c| c.crop)), (2, Some(Crop::NONE)));
        assert_eq!(st.verb("cropx"), None, "not a crop word");
    }

    #[test]
    fn striacrop_edl_round_trip() {
        let (tl, _, _) = base();
        let plain = edl::write(&tl, "T").unwrap();
        assert!(!plain.contains("UNA CROP"));
        let t2 = tl.set_crop(0, 1, Crop { l: 1, t: 2, r: 3, b: 4 }).unwrap();
        let text = edl::write(&t2, "T").unwrap();
        assert!(text.contains("\n* UNA CROP 1 2 3 4\n"), "{text}");
        assert_eq!(text.replace("* UNA CROP 1 2 3 4\n", ""), plain, "the one added line");
        let back = edl::parse(&text, 25).unwrap();
        assert_eq!(back.tracks[0].clips[1].crop, Crop { l: 1, t: 2, r: 3, b: 4 });
        assert_eq!(edl::write(&back, "T").unwrap(), text);
        assert_eq!(edl::parse(&plain, 25).unwrap().tracks[0].clips[1].crop, Crop::NONE);
    }

    #[test]
    fn striacrop_edl_refusals() {
        const V: &str = "001  A        V     C        00:00:00:00 00:00:00:10 00:00:00:00 00:00:00:10\n";
        const A: &str = "002  A        A     C        00:00:00:00 00:00:00:10 00:00:00:00 00:00:00:10\n";
        const BL: &str = "001  BL       V     C        00:00:00:00 00:00:00:10 00:00:00:00 00:00:00:10\n";
        let h = "FCM: NON-DROP FRAME\n";
        let ok = format!("{h}{V}* UNA CROP 10 20 30 40\n");
        assert_eq!(edl::parse(&ok, 25).unwrap().tracks[0].clips[0].crop, Crop { l: 10, t: 20, r: 30, b: 40 });
        for (text, line) in [
            (format!("{h}* UNA CROP 1 2 3 4\n{V}"), 2),
            (format!("{h}{V}{A}* UNA CROP 1 2 3 4\n"), 4),
            (format!("{h}{BL}* UNA CROP 1 2 3 4\n"), 3),
            (format!("{h}{V}* UNA CROP 1 2 3\n"), 3),
            (format!("{h}{V}* UNA CROP 1 2 3 x\n"), 3),
            (format!("{h}{V}* UNA CROP 600 0 400 0\n"), 3),
            (format!("{h}{V}* UNA CROP 0 0 0 0\n"), 3),
            (format!("{h}{V}* UNA CROP 1 2 3 4\n* UNA CROP 1 2 3 4\n"), 4),
        ] {
            assert_eq!(edl::parse(&text, 25).map(|_| ()), Err(EdlError::Syntax { line }), "{text}");
        }
    }
}

// ---- STRIAOPACITY (LEDGER SR868): the `opacity <pct>` word through the one commit (named tail block) ----

/// `opacity 50` sets every selected clip as ONE undo step; the same value again commits nothing; each refusal is named.
#[test]
fn striaopacity_opacity_word_is_one_undo_step() {
    use timeline_core::editor::EditorState;
    let (mut tl, a, _) = base();
    tl.tracks[1].clips = vec![Clip::new(a, 0, 10, 0)];
    let mut st = EditorState::new(tl.clone());
    assert_eq!(st.verb("opacity 50"), Some(Err(EditError::NoClip)), "nothing selected");
    st.select(0, 0).unwrap();
    st.select_add(0, 1).unwrap();
    let d0 = st.undo_depth();
    assert_eq!(st.verb("opacity 50"), Some(Ok(())));
    assert_eq!(st.undo_depth(), d0 + 1);
    assert_eq!(st.timeline, tl.set_opacity(&[(0, 0), (0, 1)], 50).unwrap());
    assert!(st.timeline.tracks[0].clips.iter().all(|c| c.opacity == 50));
    assert_eq!(st.verb("opacity 50"), Some(Ok(())));
    assert_eq!(st.undo_depth(), d0 + 1, "the same value commits nothing");
    for bad in ["opacity", "opacity 101", "opacity half", "opacity 50 60"] {
        assert_eq!(st.verb(bad), Some(Err(EditError::Opacity)), "{bad}");
    }
    assert!(st.undo());
    assert_eq!(st.timeline, tl);
    st.select(1, 0).unwrap();
    assert_eq!(st.verb("opacity 50"), Some(Err(EditError::Opacity)), "an audio clip");
    assert_eq!(st.timeline, tl);
}

// ---- STRIACLIPLINK (LEDGER SR863): the clip link group through the selection set (named tail block) ----
use timeline_core::editor::{Edge, EditorState};

/// V1 + A1 from one source each: A[0,10)@0 and B[20,30)@10 on both tracks (the picture and its sound).
fn striacliplink_base() -> Timeline {
    let (mut tl, a, b) = base();
    tl.tracks[1].clips = vec![Clip::new(a, 0, 10, 0), Clip::new(b, 20, 30, 10)];
    tl.validate().unwrap();
    tl
}

/// The same with V1's and A1's first clips linked as group 1.
fn striacliplink_linked() -> Timeline {
    striacliplink_base().set_group(&[(0, 0), (1, 0)], 1).unwrap()
}

#[test]
fn striacliplink_mates_and_set_group() {
    let tl = striacliplink_base();
    assert_eq!(tl.next_group(), 1);
    assert_eq!(tl.mates(0, 0), vec![(0, 0)], "an unlinked clip is its own set");
    let ln = striacliplink_linked();
    assert_eq!((ln.tracks[0].clips[0].group, ln.tracks[1].clips[0].group, ln.tracks[0].clips[1].group), (1, 1, 0));
    assert_eq!(ln.mates(1, 0), vec![(0, 0), (1, 0)]);
    assert_eq!(ln.mates(0, 9), Vec::<(usize, usize)>::new());
    assert_eq!(ln.next_group(), 2);
    assert_eq!(tl.set_group(&[], 1), Err(EditError::NoClip));
    assert_eq!(tl.set_group(&[(0, 9)], 1), Err(EditError::NoClip));
    assert_eq!(tl.set_group(&[(7, 0)], 1), Err(EditError::NoTrack));
    assert_eq!(tl, striacliplink_base(), "a refusal leaves the timeline");
    assert_eq!(ln.set_group(&[(0, 0), (1, 0)], 0).unwrap(), tl, "group 0 unlinks");
}

#[test]
fn striacliplink_split_linked_pairs_halves() {
    let tl = striacliplink_base();
    assert_eq!(tl.split_linked(0, 4), tl.split(0, 4), "unlinked: exactly split");
    assert_eq!(tl.split_linked(0, 10), tl.split(0, 10), "no clip spans a cut: the same refusal");
    assert_eq!(tl.split_linked(9, 4), Err(EditError::NoTrack));
    let ln = striacliplink_linked();
    let s = ln.split_linked(0, 4).unwrap();
    for k in 0..2 {
        let c = &s.tracks[k].clips;
        assert_eq!(c.len(), 3, "track {k} split");
        assert_eq!((c[0].src_in, c[0].src_out, c[0].at, c[0].group), (0, 4, 0, 1), "the head keeps the id");
        assert_eq!((c[1].src_in, c[1].src_out, c[1].at, c[1].group), (4, 10, 4, 2), "the tails share a fresh id");
        assert_eq!(c[2].group, 0);
    }
    assert_eq!(s.mates(0, 1), vec![(0, 1), (1, 1)]);
    assert_eq!(ln.split(0, 4).unwrap().tracks[0].clips[1].group, 1, "the bare op keeps the id on both halves");
}

#[test]
fn striacliplink_split_at_playhead_is_split_for_group_zero() {
    let mut st = EditorState::new(striacliplink_base());
    st.select(0, 0).unwrap();
    st.seek(4);
    st.split_at_playhead().unwrap();
    assert_eq!(st.timeline, striacliplink_base().split(0, 4).unwrap(), "unlinked: the trunk's split, byte for byte");
    let mut st = EditorState::new(striacliplink_linked());
    st.select(0, 0).unwrap();
    st.seek(4);
    assert_eq!(st.verb("split"), Some(Ok(())));
    assert_eq!(st.timeline, striacliplink_linked().split_linked(0, 4).unwrap());
    assert_eq!(st.set, vec![(0, 1), (1, 1)], "the right halves selected, as a pair");
    assert_eq!(st.undo_depth(), 1);
}

#[test]
fn striacliplink_select_takes_the_group() {
    let mut st = EditorState::new(striacliplink_linked());
    st.select(0, 0).unwrap();
    assert_eq!((st.selection, st.set.clone()), (Some((0, 0)), vec![(0, 0), (1, 0)]));
    st.deselect();
    st.select_add(1, 0).unwrap();
    assert_eq!(st.set, vec![(0, 0), (1, 0)], "shift-click on one member");
    st.select_many(&[(1, 0)], false);
    assert_eq!(st.set, vec![(0, 0), (1, 0)], "a marquee crossing one member");
    st.select(0, 1).unwrap();
    assert_eq!(st.set, vec![(0, 1)], "an unlinked clip alone");
}

#[test]
fn striacliplink_unlink_lets_one_member_be_selected_alone() {
    let mut st = EditorState::new(striacliplink_linked());
    st.select_many(&[(0, 0)], false);
    assert_eq!(st.verb("unlink"), Some(Ok(())));
    assert_eq!(st.timeline, striacliplink_base());
    st.select_many(&[(0, 0)], false);
    assert_eq!(st.set, vec![(0, 0)], "marquee: alone after unlink");
    st.deselect();
    st.select_add(1, 0).unwrap();
    assert_eq!(st.set, vec![(1, 0)], "shift-click: alone after unlink");
}

#[test]
fn striacliplink_move_set_moves_the_group() {
    let mut tl = striacliplink_linked();
    tl.tracks[0].clips[1].at = 20; // room after both first clips
    tl.tracks[1].clips[1].at = 20;
    let mut st = EditorState::new(tl.clone());
    st.select(1, 0).unwrap();
    assert_eq!(st.verb("nudge+"), Some(Ok(())));
    assert_eq!((st.timeline.tracks[0].clips[0].at, st.timeline.tracks[1].clips[0].at), (1, 1), "both moved");
    assert_eq!(st.timeline, tl.move_many(&[(0, 0), (1, 0)], 1).unwrap());
    assert_eq!(st.set, vec![(0, 0), (1, 0)], "the set follows");
    assert_eq!(st.undo_depth(), 1, "one undo step");
    assert!(st.undo());
    assert_eq!(st.timeline, tl);
}

#[test]
fn striacliplink_delete_and_lift_take_the_group() {
    let tl = striacliplink_linked();
    let mut st = EditorState::new(tl.clone());
    st.select(0, 0).unwrap();
    assert_eq!(st.verb("delete"), Some(Ok(())));
    assert_eq!(st.timeline, tl.ripple_delete_many(&[(0, 0), (1, 0)]).unwrap());
    assert_eq!((st.timeline.tracks[0].clips.len(), st.timeline.tracks[1].clips.len()), (1, 1));
    let mut st = EditorState::new(tl.clone());
    st.select(1, 0).unwrap();
    assert_eq!(st.verb("lift"), Some(Ok(())));
    assert_eq!(st.timeline, tl.lift_many(&[(0, 0), (1, 0)]).unwrap());
    assert_eq!(st.undo_depth(), 1);
}

#[test]
fn striacliplink_trim_is_the_primary_only() {
    let tl = striacliplink_linked();
    let mut st = EditorState::new(tl.clone());
    st.select(0, 0).unwrap();
    st.trim(Edge::Out, -2).unwrap();
    assert_eq!(st.timeline, tl.trim_out(0, 0, 8).unwrap());
    assert_eq!((st.timeline.tracks[1].clips[0].src_out, st.timeline.tracks[0].clips[0].group), (10, 1), "the mate stays; the id stays");
}

#[test]
fn striacliplink_link_unlink_words() {
    let tl = striacliplink_base();
    let mut st = EditorState::new(tl.clone());
    assert_eq!(st.verb("link"), Some(Err(EditError::NoClip)), "nothing selected");
    assert_eq!(st.verb("unlink"), Some(Err(EditError::NoClip)), "nothing selected");
    st.select(0, 0).unwrap();
    assert_eq!(st.verb("link"), Some(Err(EditError::NoClip)), "a set of one");
    assert_eq!(st.verb("unlink"), Some(Ok(())), "nothing linked: no commit");
    assert_eq!((st.undo_depth(), &st.timeline), (0, &tl));
    st.select_add(1, 0).unwrap();
    assert_eq!(st.verb("link"), Some(Ok(())));
    assert_eq!(st.timeline, striacliplink_linked());
    assert_eq!(st.undo_depth(), 1);
    st.select_add(0, 1).unwrap();
    assert_eq!(st.verb("link"), Some(Ok(())), "a relink merges the old group into a fresh one");
    assert_eq!(st.timeline.tracks[0].clips.iter().chain(&st.timeline.tracks[1].clips).map(|c| c.group).collect::<Vec<_>>(), vec![2, 2, 2, 0]);
    assert_eq!(st.verb("unlink"), Some(Ok(())));
    assert_eq!((st.timeline.clone(), st.undo_depth()), (tl, 3));
    assert_eq!(st.verb("links"), None, "not a word");
}

// ---- STRIACOLORLIFT (LEDGER SR869): the per-clip grade, its op, its word and THE per-pixel pass (named tail block) ----

fn striacolorlift_px(g: timeline_core::Grade, p: [u8; 4]) -> [u8; 4] {
    let mut v = p;
    g.apply(&mut v);
    v
}

/// KAT: identity at 0/0/0 is byte-identical over a grid of r / g / b (every step of 5, 0 and 255 in) with alpha varied.
#[test]
fn striacolorlift_identity_is_byte_identical() {
    let mut rgba = Vec::new();
    for r in (0..=255u8).step_by(5) {
        for g in (0..=255u8).step_by(5) {
            for b in (0..=255u8).step_by(5) {
                rgba.extend_from_slice(&[r, g, b, r ^ b]);
            }
        }
    }
    let before = rgba.clone();
    timeline_core::Grade::IDENTITY.apply(&mut rgba);
    assert_eq!(rgba, before);
}

/// KAT: brightness +100/1000 lifts mid grey by round(25.5) = 26; the ends clamp; alpha is untouched.
#[test]
fn striacolorlift_brightness_on_mid_grey() {
    let g = |b| timeline_core::Grade { brightness: b, ..timeline_core::Grade::IDENTITY };
    assert_eq!(striacolorlift_px(g(100), [128, 128, 128, 255]), [154, 154, 154, 255]);
    assert_eq!(striacolorlift_px(g(-100), [128, 128, 128, 7]), [102, 102, 102, 7]);
    assert_eq!(striacolorlift_px(g(1000), [128, 128, 128, 255]), [255, 255, 255, 255]);
    assert_eq!(striacolorlift_px(g(-1000), [128, 128, 128, 255]), [0, 0, 0, 255]);
}

/// KAT: saturation -1000 gives luma grey by yuv_core's ONE RGB -> Y' weights (the export converter's), and grey stays grey.
#[test]
fn striacolorlift_saturation_off_is_the_trees_luma() {
    let (ky, _, _, _) = yuv_core::rgb_coeffs(yuv_core::KR_KB_601, true);
    assert_eq!(ky, [19595, 38470, 7471]);
    let g = timeline_core::Grade { saturation: -1000, ..timeline_core::Grade::IDENTITY };
    for p in [[200u8, 100, 50, 255], [255, 0, 0, 9], [0, 255, 0, 255], [0, 0, 255, 255], [128, 128, 128, 255], [13, 200, 77, 1]] {
        let y = ((ky[0] * p[0] as i64 + ky[1] * p[1] as i64 + ky[2] * p[2] as i64 + 32768) >> 16) as u8;
        assert_eq!(striacolorlift_px(g, p), [y, y, y, p[3]], "{p:?}");
    }
    assert_eq!(striacolorlift_px(g, [200, 100, 50, 255]), [124, 124, 124, 255]);
}

/// KAT: contrast -1000 flattens to 128, +1000 doubles the distance from 128 (clamped).
#[test]
fn striacolorlift_contrast_about_mid_grey() {
    let g = |c| timeline_core::Grade { contrast: c, ..timeline_core::Grade::IDENTITY };
    assert_eq!(striacolorlift_px(g(-1000), [0, 77, 255, 255]), [128, 128, 128, 255]);
    assert_eq!(striacolorlift_px(g(1000), [64, 100, 192, 255]), [0, 72, 255, 255]);
}

/// The op: set_grade refuses by name (and leaves the timeline), a split keeps the grade on both halves, `grade_at` reads the
/// covering clip and a transition's outgoing one.
#[test]
fn striacolorlift_set_grade_refusals_and_lookup() {
    let (tl, _, _) = base();
    let g = timeline_core::Grade { brightness: 100, contrast: -200, saturation: 300 };
    assert_eq!(tl.set_grade(0, 1, timeline_core::Grade { saturation: 1001, ..g }), Err(EditError::Grade));
    assert_eq!(tl.set_grade(0, 1, timeline_core::Grade { brightness: -1001, ..g }), Err(EditError::Grade));
    assert_eq!(tl.set_grade(0, 9, g), Err(EditError::NoClip));
    assert_eq!(tl.set_grade(7, 0, g), Err(EditError::NoTrack));
    let mut au = tl.clone();
    au.tracks[1].clips = vec![Clip::new(au.tracks[0].clips[0].src, 0, 10, 0)];
    assert_eq!(au.set_grade(1, 0, g), Err(EditError::Kind));
    assert_eq!(EditError::Grade.to_string(), "color is b c s, each in -1000..=1000");
    let t2 = tl.set_grade(0, 1, g).unwrap();
    assert_eq!(t2.tracks[0].clips[1].grade, g);
    assert_eq!(t2.tracks[0].clips[0].grade, timeline_core::Grade::IDENTITY);
    let t3 = t2.split(0, 15).unwrap();
    assert_eq!((t3.tracks[0].clips[1].grade, t3.tracks[0].clips[2].grade), (g, g));
    assert_eq!(t2.grade_at(Some(0), 12, false), g);
    assert_eq!(t2.grade_at(Some(0), 5, false), timeline_core::Grade::IDENTITY);
    assert_eq!(t2.grade_at(Some(0), 25, true), g, "the last clip ending at or before 25 is B");
    assert_eq!(t2.grade_at(Some(0), 12, true), timeline_core::Grade::IDENTITY, "A is the outgoing at 12");
    assert_eq!(t2.grade_at(None, 12, false), timeline_core::Grade::IDENTITY);
}

/// The word: `color b c s` grades the selection in ONE undo step, the same grade again pushes none, bare `color` clears,
/// each refusal by name leaves the state.
#[test]
fn striacolorlift_color_word_is_one_commit() {
    let (tl, _, _) = base();
    let mut st = timeline_core::editor::EditorState::new(tl.clone());
    assert_eq!(st.verb("color 100 0 0"), Some(Err(EditError::NoClip)));
    st.select(0, 1).unwrap();
    assert_eq!(st.verb("color 100 -200 300"), Some(Ok(())));
    let g = timeline_core::Grade { brightness: 100, contrast: -200, saturation: 300 };
    assert_eq!((st.timeline.tracks[0].clips[1].grade, st.undo_depth()), (g, 1));
    assert_eq!(st.verb("color 100 -200 300"), Some(Ok(())));
    assert_eq!(st.undo_depth(), 1, "the same grade commits nothing");
    for bad in ["color 1 2", "color 1 2 3 4", "color a 0 0", "color 40000 0 0", "color 1001 0 0"] {
        assert_eq!(st.verb(bad), Some(Err(EditError::Grade)), "{bad}");
    }
    assert_eq!(st.undo_depth(), 1);
    assert_eq!(st.verb("color"), Some(Ok(())));
    assert_eq!((st.timeline.clone(), st.undo_depth()), (tl.clone(), 2));
    assert_eq!(st.verb("undo"), Some(Ok(())));
    assert_eq!(st.timeline.tracks[0].clips[1].grade, g);
    assert_eq!(st.verb("undo"), Some(Ok(())));
    assert_eq!(st.timeline, tl);
    assert_eq!(st.verb("colour 1 2 3"), None, "not the word");
}

// ---- STRIATRACKREORDER (LEDGER SR877): swap a track with its same-kind neighbour (named tail block) ----

/// Tracks of `kinds`, track k holding A[k, k + 10)@0 and B[20, 30)@10 (each track's clips its own); every video track a
/// dissolve on its cut at 10, the LAST audio track an explicit audio crossfade there.
fn striatrackreorder_tl(kinds: &[Kind]) -> Timeline {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 100);
    let b = tl.add_source("B", 60);
    for (k, &kind) in kinds.iter().enumerate() {
        let t = tl.add_track(kind);
        tl.tracks[t].clips = vec![Clip::new(a, k as i64, k as i64 + 10, 0), Clip::new(b, 20, 30, 10)];
    }
    for k in 0..kinds.len() {
        if kinds[k] == Kind::Video {
            tl = tl.add_transition(k, 10, timeline_core::TransitionKind::Dissolve, 2).unwrap();
        }
    }
    if let Some(k) = kinds.iter().rposition(|&x| x == Kind::Audio) {
        tl = tl.add_audio_xfade(k, 10, 2, timeline_core::Curve::Linear).unwrap();
    }
    tl.validate().unwrap();
    tl
}

/// Rule: the track swaps with the NEAREST track of its kind (other-kind tracks skipped); its clips and transitions travel
/// with it; every other track stays where it was.
#[test]
fn striatrackreorder_swaps_with_the_nearest_same_kind_track() {
    let tl = striatrackreorder_tl(&[Kind::Video, Kind::Audio, Kind::Video, Kind::Audio]);
    assert_eq!(tl.track_neighbour(2, true), Ok(0));
    assert_eq!(tl.track_neighbour(0, false), Ok(2));
    let up = tl.swap_track(2, true).unwrap();
    assert_eq!((&up.tracks[0], &up.tracks[2]), (&tl.tracks[2], &tl.tracks[0]), "V2 and V1 swapped, transitions with them");
    assert_eq!((&up.tracks[1], &up.tracks[3]), (&tl.tracks[1], &tl.tracks[3]), "the audio tracks unmoved");
    assert_eq!(up.audio_xfades, tl.audio_xfades);
    assert_eq!(tl.swap_track(0, false).unwrap(), up, "down from V1 is the same swap");
    assert_eq!(up.swap_track(0, false).unwrap(), tl, "swapping back is the identity");
}

/// Rule: `audio_xfades[].track` follows its track and the list stays in `(track, at)` order.
#[test]
fn striatrackreorder_remaps_the_audio_crossfade_track() {
    let tl = striatrackreorder_tl(&[Kind::Video, Kind::Audio, Kind::Video, Kind::Audio]);
    assert_eq!(tl.audio_xfades.len(), 1);
    assert_eq!(tl.audio_xfades[0].track, 3);
    let up = tl.swap_track(3, true).unwrap();
    assert_eq!(up.audio_xfades[0].track, 1, "the crossfade moved with A2");
    assert_eq!((&up.tracks[1], &up.tracks[3]), (&tl.tracks[3], &tl.tracks[1]));
    let mut two = tl.add_audio_xfade(1, 10, 3, timeline_core::Curve::EqualPower).unwrap();
    two = two.swap_track(1, false).unwrap();
    let order: Vec<(usize, i64)> = two.audio_xfades.iter().map(|x| (x.track, x.dur)).collect();
    assert_eq!(order, vec![(1, 2), (3, 3)], "re-sorted by track after the swap");
}

/// Refusals by name: no same-kind track that way, no such track; the timeline is unchanged.
#[test]
fn striatrackreorder_refusals() {
    let tl = striatrackreorder_tl(&[Kind::Video, Kind::Audio, Kind::Video, Kind::Audio]);
    let before = tl.clone();
    assert_eq!(tl.swap_track(0, true), Err(EditError::NoTrack), "V1 has no video track above");
    assert_eq!(tl.swap_track(2, false), Err(EditError::NoTrack), "V2 has no video track below (A2 is skipped)");
    assert_eq!(tl.swap_track(1, true), Err(EditError::NoTrack), "A1 has no audio track above (V1 is skipped)");
    assert_eq!(tl.swap_track(3, false), Err(EditError::NoTrack));
    assert_eq!(tl.swap_track(9, true), Err(EditError::NoTrack));
    assert_eq!(tl, before);
}

/// Rule: a within-kind reorder survives the EDL (channels renumber by rank within a kind; the reader is canonical).
#[test]
fn striatrackreorder_edl_round_trip_keeps_the_order() {
    use timeline_core::edl;
    let tl = striatrackreorder_tl(&[Kind::Video, Kind::Video, Kind::Audio, Kind::Audio]);
    for (track, up) in [(1, true), (3, true)] {
        let moved = tl.swap_track(track, up).unwrap();
        assert_ne!(moved, tl);
        let text = edl::write(&moved, "REORDER").unwrap();
        let back = edl::parse(&text, 25).unwrap();
        assert_eq!(back, moved, "track {track}");
        assert_eq!(edl::write(&back, "REORDER").unwrap(), text);
    }
    // Interleaved in memory, the reader brings the tracks back canonical, each kind in its swapped order.
    let mixed = striatrackreorder_tl(&[Kind::Video, Kind::Audio, Kind::Video, Kind::Audio]).swap_track(2, true).unwrap();
    let back = edl::parse(&edl::write(&mixed, "MIXED").unwrap(), 25).unwrap();
    let clips = |t: &Timeline, k: Kind| t.tracks.iter().filter(|x| x.kind == k).map(|x| x.clips.clone()).collect::<Vec<_>>();
    assert_eq!(clips(&back, Kind::Video), clips(&mixed, Kind::Video));
    assert_eq!(clips(&back, Kind::Audio), clips(&mixed, Kind::Audio));
}

// ---- STRIAOPACITYFACE (LEDGER SR898): the opacity step words and the body painted at its opacity (named tail block) ----

/// `opacity-` steps the primary's opacity 10 % down as ONE undo step each, clamped at 0 (the eleventh step commits
/// nothing); `opacity+` steps back up; no selection is `NoClip`, an audio clip `Opacity`.
#[test]
fn striaopacityface_step_words_clamp_and_commit_once() {
    use timeline_core::editor::EditorState;
    let (mut tl, a, _) = base();
    tl.tracks[1].clips = vec![Clip::new(a, 0, 10, 0)];
    let mut st = EditorState::new(tl.clone());
    assert_eq!(st.verb("opacity-"), Some(Err(EditError::NoClip)), "nothing selected");
    st.select(0, 0).unwrap();
    assert_eq!(st.verb("opacity-"), Some(Ok(())));
    assert_eq!((st.selected().map(|c| c.opacity), st.undo_depth()), (Some(90), 1));
    for _ in 0..9 {
        assert_eq!(st.verb("opacity-"), Some(Ok(())));
    }
    assert_eq!((st.selected().map(|c| c.opacity), st.undo_depth()), (Some(0), 10));
    assert_eq!(st.verb("opacity-"), Some(Ok(())));
    assert_eq!((st.selected().map(|c| c.opacity), st.undo_depth()), (Some(0), 10), "at 0 nothing commits");
    assert_eq!(st.verb("opacity+"), Some(Ok(())));
    assert_eq!((st.selected().map(|c| c.opacity), st.undo_depth()), (Some(10), 11));
    assert_eq!(st.timeline, tl.set_opacity(&[(0, 0)], 10).unwrap());
    st.select(1, 0).unwrap();
    assert_eq!(st.verb("opacity-"), Some(Err(EditError::Opacity)), "an audio clip");
    assert_eq!(st.undo_depth(), 11);
}

/// A clip at 50 % paints its body as `view::opacity_colour` of its reel colour over the row colour, at 0 % the row colour
/// itself; an opaque clip (also one set back to 100 %) paints byte for byte as before.
#[test]
fn striaopacityface_body_paints_at_its_opacity() {
    use timeline_core::editor::EditorState;
    use timeline_core::view::{self, Layout, Order, ROW, clip_colour, opacity_colour};
    let (tl, a, _) = base();
    let (w, h) = (320u32, 160u32);
    let paint = |tl: &Timeline| view::paint(&EditorState::new(tl.clone()), w, h, Order::Rgba, None, None);
    let st = EditorState::new(tl.clone());
    let l = Layout::of(w, h, &st);
    let (x, y) = l.point(0, 5);
    let at = |px: &[u8]| {
        let i = ((y as usize) * w as usize + x as usize) * 4;
        [px[i], px[i + 1], px[i + 2], px[i + 3]]
    };
    let before = paint(&tl);
    let reel = clip_colour(a.0, Kind::Video);
    assert_eq!(at(&before), reel, "opaque: the reel colour");
    let half = tl.set_opacity(&[(0, 0)], 50).unwrap();
    let want = opacity_colour(reel, ROW[0], 50);
    assert_ne!(want, reel);
    assert_eq!(at(&paint(&half)), want, "50 %: the blend");
    for ch in 0..3 {
        let (c, r) = (reel[ch] as i32, ROW[0][ch] as i32);
        assert!((want[ch] as i32 - (r + (c - r) / 2)).abs() <= 1, "channel {ch}: half way");
    }
    assert_eq!(at(&paint(&tl.set_opacity(&[(0, 0)], 0).unwrap())), ROW[0], "0 %: the row colour");
    assert_eq!(paint(&half.set_opacity(&[(0, 0)], timeline_core::OPAQUE).unwrap()), before, "back to opaque: byte-identical");
    assert_eq!(opacity_colour(reel, ROW[0], timeline_core::OPAQUE), reel);
}

// ---- STRIAPASTEINSERT (LEDGER SR896): paste_as — SR854's one walk in a mode (named tail block) ----
#[test]
fn striapasteinsert_paste_as_insert_and_overwrite_through_place() {
    let (tl, a, b) = base(); // V1: A[0,10)@0 B[20,30)@10
    let one = Clip::new(b, 40, 45, 5);
    assert_eq!(tl.paste_as(&[(0, Kind::Video, one)], Some(Mode::Insert)).unwrap(), tl.place(0, one, Mode::Insert).unwrap());
    assert_eq!(tl.paste_as(&[(0, Kind::Video, one)], Some(Mode::Overwrite)).unwrap(), tl.place(0, one, Mode::Overwrite).unwrap(), "it carves");
    assert_eq!(tl.paste_as(&[(0, Kind::Video, one)], None), Err(EditError::Overlap), "None is SR854's never-overwrite paste");
    assert_eq!(tl.paste(&[(0, Kind::Video, one)]), Err(EditError::Overlap));
    // two items given out of order walk ascending: the later one never pushes the earlier
    let (x, y) = (Clip::new(a, 50, 55, 5), Clip::new(b, 0, 5, 12));
    let p = tl.paste_as(&[(0, Kind::Video, y), (0, Kind::Video, x)], Some(Mode::Insert)).unwrap();
    assert_eq!(p, tl.place(0, x, Mode::Insert).unwrap().place(0, y, Mode::Insert).unwrap());
    assert_eq!(p, tl.paste_as(&[(0, Kind::Video, x), (0, Kind::Video, y)], Some(Mode::Insert)).unwrap());
    assert_eq!((p.tracks[0].clips.iter().any(|c| *c == x), p.tracks[0].clips.iter().any(|c| *c == y)), (true, true), "each lands on its own tick");
}

#[test]
fn striapasteinsert_paste_as_refusals_by_name() {
    let (tl, a, _) = base();
    let before = tl.clone();
    for m in [Some(Mode::Insert), Some(Mode::Overwrite)] {
        assert_eq!(tl.paste_as(&[], m), Err(EditError::NoClip));
        assert_eq!(tl.paste_as(&[(2, Kind::Video, Clip::new(a, 0, 10, 40))], m), Err(EditError::NoTrack));
        assert_eq!(tl.paste_as(&[(1, Kind::Video, Clip::new(a, 0, 10, 40))], m), Err(EditError::Kind));
        assert_eq!(tl.paste_as(&[(0, Kind::Video, Clip::new(a, 90, 110, 40))], m), Err(EditError::OutOfSource), "place's own validate");
    }
    assert_eq!(tl, before);
}

// ---- STRIAGRADEFACE (LEDGER SR899): the grade step words (named tail block) ----

/// Each step word moves one field of the selected clip's grade by GRADE_STEP in ONE undo step; past the range it is
/// refused (EditError::Grade, not clamped) and the state stays; nothing selected is NoClip, an audio clip Kind.
#[test]
fn striagradeface_step_words_one_commit_each() {
    use timeline_core::editor::{EditorState, GRADE_STEP};
    let (tl, a, _) = base();
    let mut st = EditorState::new(tl.clone());
    assert_eq!(GRADE_STEP, 50);
    assert_eq!(st.verb("bright+"), Some(Err(EditError::NoClip)));
    st.select(0, 1).unwrap();
    assert_eq!(st.verb("bright+"), Some(Ok(())));
    assert_eq!((st.timeline.tracks[0].clips[1].grade.brightness, st.undo_depth()), (50, 1));
    assert_eq!(st.verb("bright-"), Some(Ok(())));
    assert_eq!((st.timeline.tracks[0].clips[1].grade, st.undo_depth()), (timeline_core::Grade::IDENTITY, 2));
    assert_eq!(st.verb("contrast-"), Some(Ok(())));
    assert_eq!(st.verb("sat+"), Some(Ok(())));
    let g = timeline_core::Grade { brightness: 0, contrast: -50, saturation: 50 };
    assert_eq!((st.timeline.tracks[0].clips[1].grade, st.undo_depth()), (g, 4));
    for _ in 0..19 {
        assert_eq!(st.verb("sat+"), Some(Ok(())));
    }
    assert_eq!(st.timeline.tracks[0].clips[1].grade.saturation, 1000);
    let (held, depth) = (st.timeline.clone(), st.undo_depth());
    assert_eq!(st.verb("sat+"), Some(Err(EditError::Grade)));
    assert_eq!((&st.timeline, st.undo_depth()), (&held, depth));
    for _ in 0..depth {
        assert_eq!(st.verb("undo"), Some(Ok(())));
    }
    assert_eq!(st.timeline, tl);
    for w in ["bright", "bright++", "sat", "contrast"] {
        assert_eq!(st.verb(w), None, "{w}: not a step word");
    }
    let mut tl2 = tl.clone();
    tl2.tracks[1].clips = vec![Clip::new(a, 0, 10, 0)];
    let mut st = EditorState::new(tl2.clone());
    st.select(1, 0).unwrap();
    assert_eq!(st.verb("bright+"), Some(Err(EditError::Kind)));
    assert_eq!((&st.timeline, st.undo_depth()), (&tl2, 0));
}


// ---- STRIAUNDOHISTORY (LEDGER SR876): op names on the undo entries (named tail block) ----

/// Each verb names the undo step it commits; undo / redo move the name with the snapshot (`redo` is not a step); a refused
/// or foreign word adds nothing; a commit outside any verb (a drag's `Drag::finish` road, `move_selected`) is unnamed; a
/// commit after an undo drops the redo names with the redo stack; an outer armed word wins over the verb it calls.
#[test]
fn striaundohistory_verbs_name_their_steps() {
    use timeline_core::editor::{EditorState, HISTORY_N};
    let (tl, _, _) = base();
    let mut st = EditorState::new(tl);
    assert_eq!(st.history_line(HISTORY_N), "history: nothing to undo");
    st.select(0, 1).unwrap();
    st.seek(15);
    assert_eq!(st.verb("split"), Some(Ok(())));
    assert_eq!(st.verb("]"), Some(Ok(())));
    assert_eq!(st.verb("mark in"), Some(Ok(())));
    assert_eq!(st.history(HISTORY_N), vec![(3, Some("mark in")), (2, Some("]")), (1, Some("split"))]);
    assert_eq!(st.history_line(HISTORY_N), "history: 3 mark in | 2 ] | 1 split");
    let sel = st.selection;
    st.deselect();
    assert_eq!(st.verb("split"), Some(Err(EditError::NoClip)));
    assert_eq!(st.verb("play"), None);
    assert_eq!(st.history(HISTORY_N).len(), 3);
    assert_eq!(st.verb("undo"), Some(Ok(())));
    assert_eq!(st.history(HISTORY_N), vec![(2, Some("]")), (1, Some("split"))]);
    assert_eq!(st.verb("redo"), Some(Ok(())));
    assert_eq!(st.history(HISTORY_N), vec![(3, Some("mark in")), (2, Some("]")), (1, Some("split"))]);
    st.select(0, 0).unwrap();
    st.move_selected(0, 100).unwrap();
    assert_eq!(st.history(2), vec![(4, None), (3, Some("mark in"))]);
    assert_eq!(st.history_line(2), "history: 4 (unnamed) | 3 mark in");
    assert_eq!(st.verb("undo"), Some(Ok(())));
    assert_eq!(st.verb("undo"), Some(Ok(())));
    assert_eq!(st.redo_depth(), 2);
    assert_eq!(st.verb("mark out"), Some(Ok(())));
    assert_eq!(st.redo_depth(), 0);
    assert_eq!(st.history(HISTORY_N), vec![(3, Some("mark out")), (2, Some("]")), (1, Some("split"))]);
    let armed = st.arm_word("outer");
    assert!(armed);
    assert!(!st.arm_word("inner"), "a word is armed: the outer one wins");
    assert_eq!(st.verb("mark clear"), Some(Ok(())));
    st.disarm_word(armed);
    assert_eq!(st.history(1), vec![(4, Some("outer"))]);
    println!("striaundohistory: {} (selection before the refusal {sel:?})", st.history_line(HISTORY_N));
}

/// Bounded: 300 verb commits keep UNDO_MAX snapshots and their names in lockstep (the cap drops the oldest name with the
/// oldest snapshot); a full undo and redo round brings every name back; the line lists `HISTORY_N`; a long word is clipped
/// to `WORD_MAX` bytes on a char boundary.
#[test]
fn striaundohistory_bounded() {
    use timeline_core::editor::{EditorState, HISTORY_N, UNDO_MAX, WORD_MAX};
    let (tl, _, _) = base();
    let mut st = EditorState::new(tl);
    st.select(0, 1).unwrap();
    for i in 0..12 {
        assert_eq!(st.verb(if i % 2 == 0 { "nudge+" } else { "nudge-" }), Some(Ok(())));
    }
    assert_eq!(st.history(HISTORY_N).len(), 10);
    assert_eq!(st.history(99).len(), 12);
    assert_eq!(st.history_line(HISTORY_N).matches(" | ").count(), 9);
    for i in 12..300 {
        assert_eq!(st.verb(if i % 2 == 0 { "nudge+" } else { "nudge-" }), Some(Ok(())));
    }
    assert_eq!(st.undo_depth(), UNDO_MAX);
    let want: Vec<(usize, Option<&str>)> = (0..HISTORY_N).map(|k| (UNDO_MAX - k, Some(if k % 2 == 0 { "nudge-" } else { "nudge+" }))).collect();
    assert_eq!(st.history(HISTORY_N), want);
    let all: Vec<(usize, Option<String>)> = st.history(UNDO_MAX).into_iter().map(|(i, w)| (i, w.map(String::from))).collect();
    assert_eq!(all.len(), UNDO_MAX);
    assert!(all.iter().all(|(_, w)| w.is_some()), "every kept snapshot is named");
    while st.undo() {}
    assert_eq!((st.undo_depth(), st.redo_depth()), (0, UNDO_MAX));
    while st.redo() {}
    let back: Vec<(usize, Option<String>)> = st.history(UNDO_MAX).into_iter().map(|(i, w)| (i, w.map(String::from))).collect();
    assert_eq!(back, all);
    let long = format!("a{}", "\u{e9}".repeat(20));
    let armed = st.arm_word(&long);
    assert_eq!(st.verb("nudge+"), Some(Ok(())));
    st.disarm_word(armed);
    let top = st.history(1)[0].1.unwrap().to_string();
    assert!(top.len() <= WORD_MAX && long.starts_with(&top));
    assert_eq!(top.len(), 23, "24 falls inside a two-byte char: clipped to 23");
    assert_eq!(st.undo_depth(), UNDO_MAX);
}

// ---- STRIACROPFACE (LEDGER SR900): the crop words over the selection set and the crop badge (named tail block) ----
mod striacropface {
    use super::base;
    use timeline_core::editor::{CROP_STEP, EditorState};
    use timeline_core::view::{self, Order};
    use timeline_core::{Crop, EditError};

    /// STRIACROPFACE (SR900): on a one-clip set the words step / clear the selected clip, one undo step each; bare `crop`
    /// now clears (SR867 refused it); a step keeping no picture is refused by name and leaves the state.
    #[test]
    fn striacropface_words_on_one_clip() {
        let (tl, _, _) = base();
        let mut st = EditorState::new(tl.clone());
        st.select(0, 1).unwrap();
        for w in ["crop-l+", "crop-t+", "crop-r+", "crop-b+"] {
            assert_eq!(st.verb(w), Some(Ok(())), "{w}");
        }
        let s = CROP_STEP;
        assert_eq!((st.selected().map(|c| c.crop), st.undo_depth()), (Some(Crop { l: s, t: s, r: s, b: s }), 4));
        assert_eq!(st.verb("crop-t-"), Some(Ok(())));
        assert_eq!(st.selected().map(|c| c.crop), Some(Crop { l: s, t: 0, r: s, b: s }));
        assert_eq!(st.verb("crop"), Some(Ok(())));
        assert_eq!((st.selected().map(|c| c.crop), st.undo_depth()), (Some(Crop::NONE), 6));
        assert_eq!(st.timeline, tl, "cleared: the timeline as before");
        assert_eq!(st.verb("crop 500 0 490 0"), Some(Ok(())));
        assert_eq!(st.verb("crop-l+"), Some(Err(EditError::Crop)));
        assert_eq!((st.selected().map(|c| c.crop), st.undo_depth()), (Some(Crop { l: 500, t: 0, r: 490, b: 0 }), 7));
    }

    /// STRIACROPFACE (SR900): `crop l t r b` and the words act on every video clip of a multiselection as ONE commit.
    #[test]
    fn striacropface_set_wide_one_commit() {
        let (tl, _, _) = base();
        let mut st = EditorState::new(tl.clone());
        st.select(0, 0).unwrap();
        st.select_add(0, 1).unwrap();
        let c = Crop { l: 100, t: 0, r: 100, b: 0 };
        assert_eq!(st.verb("crop 100 0 100 0"), Some(Ok(())));
        assert_eq!((st.timeline.tracks[0].clips[0].crop, st.timeline.tracks[0].clips[1].crop, st.undo_depth()), (c, c, 1));
        assert_eq!(st.verb("crop-l-"), Some(Ok(())));
        assert_eq!(st.timeline.tracks[0].clips[1].crop.l, 100 - CROP_STEP);
        assert_eq!(st.undo_depth(), 2);
        assert!(st.undo() && st.undo());
        assert_eq!(st.timeline, tl);
    }

    /// STRIACROPFACE (SR900): a cropped clip's body carries the badge (CROP_BADGE glyph in a FIELD box at its top right)
    /// and the canvas differs from the uncropped one only inside that box; a body under 16 px gets none.
    #[test]
    fn striacropface_badge_only_when_cropped() {
        let (tl, _, _) = base();
        let mut plain = EditorState::new(tl.clone());
        plain.zoom = 8;
        let mut cut = plain.clone();
        cut.timeline = tl.set_crop(0, 1, Crop { l: 100, t: 0, r: 0, b: 0 }).unwrap();
        let (w, h) = (320u32, 200u32);
        let a = view::paint(&plain, w, h, Order::Rgba, None, None);
        let b = view::paint(&cut, w, h, Order::Rgba, None, None);
        let l = view::Layout::of(w, h, &cut);
        let (x1, y0) = (l.x_of(20), l.row_y(0));
        let (bx, by) = (x1 - 11, y0 + 3);
        let px = |buf: &[u8], x: i64, y: i64| -> [u8; 4] {
            let o = (y as usize * w as usize + x as usize) * 4;
            buf[o..o + 4].try_into().unwrap()
        };
        assert_eq!(px(&b, bx + 2, by + 1), view::CROP_BADGE, "the glyph's first row");
        assert_eq!(px(&b, bx, by), view::FIELD, "the box");
        for y in 0..h as i64 {
            for x in 0..w as i64 {
                let inside = x >= bx && x < bx + 9 && y >= by && y < by + 9;
                if !inside {
                    assert_eq!(px(&a, x, y), px(&b, x, y), "({x}, {y}) outside the badge");
                }
            }
        }
        let mut narrow = cut.clone();
        narrow.zoom = 1;
        let mut narrow_plain = plain.clone();
        narrow_plain.zoom = 1;
        assert_eq!(view::paint(&narrow, w, h, Order::Rgba, None, None), view::paint(&narrow_plain, w, h, Order::Rgba, None, None), "10 px: no badge");
    }
}


// ---- STRIAPOSITIONSCALE (LEDGER SR879): the per-clip position / scale: geometry, the placement, the composite, the op, the words and the EDL line (named tail block) ----
mod striapositionscale {
    use super::base;
    use timeline_core::edl::{self, EdlError};
    use timeline_core::editor::EditorState;
    use timeline_core::schedule::Frac;
    use timeline_core::{Clip, Crop, EditError, SizePolicy, TransitionKind, Xform, crop_frame, place_frame, xform_frame};

    const Q: Xform = Xform { x: 500, y: 500, s: 500 };

    #[test]
    fn striapositionscale_rect_and_validity() {
        let r = Q.rect(8, 4);
        assert_eq!((r.x, r.y, r.w, r.h), (4, 2, 4, 2));
        let r = Xform::NONE.rect(7, 3);
        assert_eq!((r.x, r.y, r.w, r.h), (0, 0, 7, 3));
        let r = Xform { x: 999, y: 0, s: 1 }.rect(4, 2);
        assert_eq!((r.x, r.y, r.w, r.h), (3, 0, 1, 1), "at least one sample, inside the frame");
        assert!(Xform::PIP.ok() && Xform::NONE.ok() && Q.ok());
        assert!(!Xform { x: 600, y: 0, s: 500 }.ok());
        assert!(!Xform { x: 0, y: 501, s: 500 }.ok());
        assert!(!Xform { x: 0, y: 0, s: 0 }.ok());
        assert_eq!(Xform::default(), Xform::NONE);
        assert_eq!(Clip::new(timeline_core::SourceId(0), 0, 1, 0).xform, Xform::NONE);
    }

    #[test]
    fn striapositionscale_place_frame_into_a_rect() {
        let px: Vec<u32> = (0..8).collect(); // 4 x 2: 0 1 2 3 / 4 5 6 7
        let mut out = vec![99u32; 16]; // 4 x 4
        place_frame(&px, 4, 2, Crop::NONE, SizePolicy::Stretch, &mut out, 4, Xform { x: 500, y: 500, s: 500 }.rect(4, 4));
        assert_eq!(out, vec![99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 0, 2, 99, 99, 4, 6], "the frame scaled into the rect, the rest untouched");
        let mut whole = vec![7u32; 8];
        place_frame(&px, 4, 2, Crop { l: 500, t: 0, r: 0, b: 0 }, SizePolicy::Letterbox, &mut whole, 4, Xform::NONE.rect(4, 2));
        assert_eq!(whole, vec![7, 2, 3, 7, 7, 6, 7, 7], "the crop pass's placement: the rest left as it was");
        assert_eq!(crop_frame(&px, 4, 2, Crop { l: 500, t: 0, r: 0, b: 0 }, SizePolicy::Letterbox, 99), vec![99, 2, 3, 99, 99, 6, 7, 99], "crop_frame unchanged");
        let mut short = vec![1u32; 4];
        place_frame(&px, 4, 2, Crop::NONE, SizePolicy::Stretch, &mut short, 4, Xform::NONE.rect(4, 2));
        assert_eq!(short, vec![1; 4], "a destination past out places nothing");
    }

    #[test]
    fn striapositionscale_xform_frame_outside_is_the_lower_frame() {
        let (w, h) = (8usize, 4usize);
        let top: Vec<[u8; 4]> = (0..w * h).map(|i| [i as u8, 100, 200, 255]).collect();
        let under: Vec<[u8; 4]> = (0..w * h).map(|i| [10, i as u8, 20, 255]).collect();
        let r = Q.rect(w, h);
        let inside = |i: usize| (r.x..r.x + r.w).contains(&(i % w)) && (r.y..r.y + r.h).contains(&(i / w));
        let opaque = xform_frame(&top, &under, w, h, Crop::NONE, SizePolicy::Letterbox, Q, Frac { num: 100, den: 100 });
        let mut want = under.clone();
        place_frame(&top, w, h, Crop::NONE, SizePolicy::Letterbox, &mut want, w, r);
        assert_eq!(opaque, want, "opaque: the top placed straight into the rect");
        assert_eq!(opaque[2 * w + 4], top[0], "the rect's corner is the top's corner");
        let half = xform_frame(&top, &under, w, h, Crop::NONE, SizePolicy::Letterbox, Q, Frac { num: 50, den: 100 });
        for i in 0..w * h {
            if inside(i) {
                let mut px = under[i];
                timeline_core::vmix::mix_step(TransitionKind::Dissolve, Frac { num: 50, den: 100 }, &mut px, &opaque[i], 1, 1);
                assert_eq!(half[i], px, "inside {i}: THE mix over the lower pixel");
            } else {
                assert_eq!(half[i], under[i], "outside {i}: the lower frame byte for byte");
                assert_eq!(opaque[i], under[i]);
            }
        }
        assert_eq!(xform_frame(&top[..3], &under, w, h, Crop::NONE, SizePolicy::Letterbox, Q, Frac { num: 1, den: 1 }), under, "a short frame: the lower");
    }

    #[test]
    fn striapositionscale_set_xform_and_refusals() {
        let (tl, _, _) = base();
        let t2 = tl.set_xform(0, 1, Q).unwrap();
        assert_eq!((t2.tracks[0].clips[1].xform, t2.tracks[0].clips[0].xform), (Q, Xform::NONE));
        assert_eq!(tl.tracks[0].clips[1].xform, Xform::NONE, "pure");
        assert_eq!(tl.set_xform(0, 1, Xform { x: 600, y: 0, s: 500 }), Err(EditError::Xform));
        assert_eq!(tl.set_xform(0, 9, Q), Err(EditError::NoClip));
        assert_eq!(tl.set_xform(9, 0, Q), Err(EditError::NoTrack));
        assert_eq!(tl.set_xform(1, 0, Q), Err(EditError::Kind));
        assert_eq!(EditError::Xform.to_string(), "pip is x y and scale s in 1/1000 with x + s and y + s at most 1000");
        let s = t2.split(0, 15).unwrap();
        assert_eq!((s.tracks[0].clips[1].xform, s.tracks[0].clips[2].xform), (Q, Q), "both halves keep the placement");
        assert_eq!((t2.xform_at(0, 12), t2.xform_at(0, 3), t2.xform_at(7, 3)), (Q, Xform::NONE, Xform::NONE));
    }

    #[test]
    fn striapositionscale_words_one_undo_step_each() {
        let (tl, _, _) = base();
        let mut st = EditorState::new(tl.clone());
        assert_eq!(st.verb("pip"), Some(Err(EditError::NoClip)));
        st.select(0, 1).unwrap();
        assert_eq!(st.verb("pip"), Some(Ok(())));
        assert_eq!((st.undo_depth(), st.selected().map(|c| c.xform)), (1, Some(Xform::PIP)));
        assert_eq!(st.verb("pip"), Some(Ok(())));
        assert_eq!(st.undo_depth(), 1, "the same placement commits nothing");
        assert_eq!(st.verb("scale 100"), Some(Ok(())));
        assert_eq!(st.selected().map(|c| c.xform), Some(Xform { x: 725, y: 725, s: 100 }));
        assert_eq!(st.verb("pip 0 0"), Some(Ok(())));
        assert_eq!((st.undo_depth(), st.selected().map(|c| c.xform)), (3, Some(Xform { x: 0, y: 0, s: 100 })));
        for bad in ["pip 1", "pip 1 2 3", "pip a 0", "pip -1 0", "pip 950 0", "scale", "scale 0", "scale 1001", "scale x", "pip on"] {
            assert_eq!(st.verb(bad), Some(Err(EditError::Xform)), "{bad}");
        }
        assert_eq!(st.undo_depth(), 3);
        assert_eq!(st.verb("pip off"), Some(Ok(())));
        assert_eq!((st.undo_depth(), st.selected().map(|c| c.xform)), (4, Some(Xform::NONE)));
        for _ in 0..4 {
            assert!(st.undo());
        }
        assert_eq!(st.timeline, tl);
        st.select(1, 0).ok();
        assert_eq!(st.verb("pipx"), None, "not a pip word");
        assert_eq!(st.verb("scales 1"), None);
    }

    #[test]
    fn striapositionscale_edl_round_trip() {
        let (tl, _, _) = base();
        let plain = edl::write(&tl, "T").unwrap();
        assert!(!plain.contains("UNA XFORM"));
        let t2 = tl.set_crop(0, 1, Crop { l: 1, t: 2, r: 3, b: 4 }).unwrap().set_xform(0, 1, Xform { x: 10, y: 20, s: 300 }).unwrap();
        let text = edl::write(&t2, "T").unwrap();
        assert!(text.contains("\n* UNA CROP 1 2 3 4\n* UNA XFORM 10 20 300\n"), "{text}");
        let back = edl::parse(&text, 25).unwrap();
        assert_eq!(back.tracks[0].clips[1].xform, Xform { x: 10, y: 20, s: 300 });
        assert_eq!(edl::write(&back, "T").unwrap(), text);
        assert_eq!(edl::parse(&plain, 25).unwrap().tracks[0].clips[1].xform, Xform::NONE);
    }

    #[test]
    fn striapositionscale_edl_refusals() {
        const V: &str = "001  A        V     C        00:00:00:00 00:00:00:10 00:00:00:00 00:00:00:10\n";
        const A: &str = "002  A        A     C        00:00:00:00 00:00:00:10 00:00:00:00 00:00:00:10\n";
        const BL: &str = "001  BL       V     C        00:00:00:00 00:00:00:10 00:00:00:00 00:00:00:10\n";
        let h = "FCM: NON-DROP FRAME\n";
        let ok = format!("{h}{V}* UNA XFORM 10 20 300\n");
        assert_eq!(edl::parse(&ok, 25).unwrap().tracks[0].clips[0].xform, Xform { x: 10, y: 20, s: 300 });
        for (text, line) in [
            (format!("{h}* UNA XFORM 1 2 3\n{V}"), 2),
            (format!("{h}{V}{A}* UNA XFORM 1 2 3\n"), 4),
            (format!("{h}{BL}* UNA XFORM 1 2 3\n"), 3),
            (format!("{h}{V}* UNA XFORM 1 2\n"), 3),
            (format!("{h}{V}* UNA XFORM 1 2 x\n"), 3),
            (format!("{h}{V}* UNA XFORM 600 0 500\n"), 3),
            (format!("{h}{V}* UNA XFORM 0 0 1000\n"), 3),
            (format!("{h}{V}* UNA XFORM 1 2 3\n* UNA XFORM 1 2 3\n"), 4),
        ] {
            assert_eq!(edl::parse(&text, 25).map(|_| ()), Err(EdlError::Syntax { line }), "{text}");
        }
    }
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
