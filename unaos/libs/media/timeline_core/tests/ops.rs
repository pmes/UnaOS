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
    let before = paint_in(&st, l, Order::Rgba, None, None, None);
    assert_eq!(st.verb("xfade 8"), Some(Ok(())));
    let after = paint_in(&st, l, Order::Rgba, None, None, None);
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
    assert_eq!(paint_in(&st, l, Order::Rgba, None, None, None), before);
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
use timeline_core::editor::Edge;

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
fn striacliplink_trim_carries_the_group() {
    let tl = striacliplink_linked();
    let mut st = EditorState::new(tl.clone());
    st.select(0, 0).unwrap();
    st.trim(Edge::Out, -2).unwrap();
    assert_eq!(st.timeline, tl.trim_out(0, 0, 8).unwrap().trim_out(1, 0, 8).unwrap());
    assert_eq!((st.timeline.tracks[1].clips[0].src_out, st.timeline.tracks[0].clips[0].group), (8, 1), "the mate follows (STRIALINKTRIM SR910); the id stays");
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

// ---- STRIALINKAUTO (LEDGER SR911): the V1 / A1 pairs of an opened file linked, and `unlink-all` (named tail block) ----

#[test]
fn strialinkauto_link_av_pairs_equal_clips() {
    let tl = striacliplink_base();
    let ln = tl.link_av();
    let groups = |t: &Timeline| t.tracks[0].clips.iter().chain(&t.tracks[1].clips).map(|c| c.group).collect::<Vec<_>>();
    assert_eq!(groups(&ln), vec![1, 2, 1, 2], "one fresh id per pair");
    assert_eq!(ln.mates(0, 0), vec![(0, 0), (1, 0)]);
    assert_eq!(ln.mates(1, 1), vec![(0, 1), (1, 1)]);
    assert_eq!(ln.set_group(&[(0, 0), (1, 0), (0, 1), (1, 1)], 0).unwrap(), tl, "only the group changed");
    // An audio clip of another length stays unlinked; so does its video clip.
    let mut odd = striacliplink_base();
    odd.tracks[1].clips[1].src_out = 28;
    assert_eq!(groups(&odd.link_av()), vec![1, 0, 1, 0]);
    // An already linked clip is left alone; the free pair takes the next id.
    let pre = striacliplink_linked().link_av();
    assert_eq!(groups(&pre), vec![1, 2, 1, 2]);
    assert_eq!(striacliplink_linked().set_group(&[(0, 1), (1, 1)], 2).unwrap(), pre);
    // A video-only timeline is unchanged.
    let (v, _, _) = base();
    let mut v = v;
    v.tracks.truncate(1);
    assert_eq!(v.link_av(), v);
}

#[test]
fn strialinkauto_unlink_all_word() {
    let tl = striacliplink_base();
    let ln = tl.link_av();
    let mut st = EditorState::new(ln.clone());
    assert_eq!(st.verb("unlink-all"), Some(Ok(())), "no selection needed");
    assert_eq!((st.timeline.clone(), st.undo_depth()), (tl.clone(), 1), "every link parted, one commit");
    assert_eq!(st.verb("unlink-all"), Some(Ok(())), "nothing linked");
    assert_eq!(st.undo_depth(), 1, "nothing linked: no commit");
    assert!(st.undo());
    assert_eq!(st.timeline, ln, "one undo step restores both pairs");
}

#[test]
fn strialinkauto_unlink_all_keeps_selection() {
    let mut st = EditorState::new(striacliplink_base().link_av());
    st.select(0, 1).unwrap();
    assert_eq!(st.set, vec![(0, 1), (1, 1)], "the pair selects together");
    assert_eq!(st.verb("unlink-all"), Some(Ok(())));
    assert_eq!((st.selection, st.set.clone()), (Some((0, 1)), vec![(0, 1), (1, 1)]), "no clip moved: the selection stands");
    st.select(0, 1).unwrap();
    assert_eq!(st.set, vec![(0, 1)], "unlinked: the clip alone");
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

// ---- STRIALINKTRIM (LEDGER SR910): linked trims, ripple trims and slips, all-or-nothing, and the chain mark (named tail block) ----
use timeline_core::view::{self, Drag, DragKind, Layout, Order, ROW_H};

/// `trim` (and the `]` word) on one member trims every mate by the same source delta; ONE undo step each.
#[test]
fn strialinktrim_trim_carries_the_mates() {
    let tl = striacliplink_linked();
    let mut st = EditorState::new(tl.clone());
    st.select(0, 0).unwrap();
    st.trim(Edge::Out, -2).unwrap();
    let one = tl.trim_out(0, 0, 8).unwrap().trim_out(1, 0, 8).unwrap();
    assert_eq!(st.timeline, one);
    assert_eq!(st.verb("]"), Some(Ok(())));
    let two = one.trim_in(0, 0, 1).unwrap().trim_in(1, 0, 1).unwrap();
    assert_eq!(st.timeline, two);
    assert_eq!((st.timeline.tracks[1].clips[0].src_in, st.timeline.tracks[1].clips[0].at), (1, 1), "the mate's in moved with its start");
    assert_eq!(st.undo_depth(), 2, "one step per trim");
    assert!(st.undo());
    assert_eq!(st.timeline, one);
    assert!(st.undo());
    assert_eq!(st.timeline, tl, "one undo restores both members");
}

/// `rtrim out -2` on one member ripple-trims every mate: each track's later clip follows.
#[test]
fn strialinktrim_ripple_trim_carries_the_mates() {
    let tl = striacliplink_linked();
    let mut st = EditorState::new(tl.clone());
    st.select(1, 0).unwrap();
    assert_eq!(st.verb("rtrim out -2"), Some(Ok(())));
    assert_eq!(st.timeline, tl.ripple_trim_out(1, 0, 8).unwrap().ripple_trim_out(0, 0, 8).unwrap());
    assert_eq!((st.timeline.tracks[0].clips[1].at, st.timeline.tracks[1].clips[1].at), (8, 8), "both tracks rippled");
    assert_eq!(st.undo_depth(), 1);
}

/// `slip 3` on one member slips every mate's source window by the same delta.
#[test]
fn strialinktrim_slip_carries_the_mates() {
    let tl = striacliplink_linked();
    let mut st = EditorState::new(tl.clone());
    st.select(0, 0).unwrap();
    assert_eq!(st.verb("slip 3"), Some(Ok(())));
    assert_eq!(st.timeline, tl.slip(0, 0, 3).unwrap().slip(1, 0, 3).unwrap());
    assert_eq!((st.timeline.tracks[0].clips[0].src_in, st.timeline.tracks[1].clips[0].src_in), (3, 3));
    assert_eq!(st.undo_depth(), 1);
}

/// A mate that cannot follow refuses the whole edit (nothing committed); an unlinked clip trims exactly as the trunk did.
#[test]
fn strialinktrim_refusal_is_all_or_nothing() {
    let mut tl = striacliplink_base();
    tl.tracks[1].clips[0] = Clip { src_in: 90, src_out: 100, ..tl.tracks[1].clips[0] };
    let tl = tl.set_group(&[(0, 0), (1, 0)], 1).unwrap();
    let mut st = EditorState::new(tl.clone());
    st.select(0, 0).unwrap();
    assert_eq!(st.verb("slip 5"), Some(Err(EditError::OutOfSource)), "the mate would leave its source");
    assert_eq!(st.verb("rtrim out 1"), Some(Err(EditError::OutOfSource)), "the mate's out would pass its source");
    assert_eq!((&st.timeline, st.undo_depth()), (&tl, 0), "nothing committed");
    let base = striacliplink_base();
    let mut st = EditorState::new(base.clone());
    st.select(0, 0).unwrap();
    st.trim(Edge::Out, -2).unwrap();
    assert_eq!(st.timeline, base.trim_out(0, 0, 8).unwrap(), "group 0: the trunk's single-clip trim");
    assert_eq!(st.timeline.tracks[1].clips[0].src_out, 10);
}

/// A drag on a member's out handle (no snap) previews and commits the linked trim: `Drag::finish` rides `trim`.
#[test]
fn strialinktrim_drag_trim_carries_the_mates() {
    let tl = striacliplink_linked();
    let want = tl.trim_out(0, 0, 8).unwrap().trim_out(1, 0, 8).unwrap();
    let mut st = EditorState::new(tl);
    st.select(0, 0).unwrap();
    let l = Layout::of(400, 240, &st);
    let (z, y) = (l.zoom as f64, l.row_y(0) as f64 + ROW_H as f64 / 2.0);
    let x = l.x_of(10) as f64 - 2.0;
    let mut d = Drag::begin_with(&mut st, l, x, y, false, true, false).unwrap();
    assert_eq!(d.kind, DragKind::TrimOut);
    d.update(&st, x - 2.0 * z, y);
    assert_eq!(d.shown(&st).map(|s| s.timeline), Some(want.clone()), "the preview shows both members trimmed");
    assert_eq!(d.finish(&mut st), Ok(()));
    assert_eq!((&st.timeline, st.undo_depth()), (&want, 1));
}

/// The chain mark: an unlinked timeline paints exactly as before (no mark); linked, the picture differs only inside
/// each linked body's mark box, whose top-left pixel is `LINK`.
#[test]
fn strialinktrim_chain_mark_paints_only_when_linked() {
    let (w, h) = (400u32, 240u32);
    let base = EditorState::new(striacliplink_base());
    let linked = EditorState::new(striacliplink_linked());
    let l = Layout::of(w, h, &base);
    let a = view::paint(&base, w, h, Order::Rgba, None, None);
    let b = view::paint(&linked, w, h, Order::Rgba, None, None);
    let boxes: Vec<(i64, i64)> = (0..2).map(|row| (l.x_of(10) - 12, l.row_y(row) + 4)).collect();
    let in_box = |x: i64, y: i64| boxes.iter().any(|&(bx, by)| x >= bx && x < bx + 8 && y >= by && y < by + 4);
    for y in 0..h as i64 {
        for x in 0..w as i64 {
            let i = ((y * w as i64 + x) * 4) as usize;
            if a[i..i + 4] != b[i..i + 4] {
                assert!(in_box(x, y), "a pixel outside the mark boxes changed at ({x}, {y})");
            }
        }
    }
    for &(bx, by) in &boxes {
        let i = ((by * w as i64 + bx) * 4) as usize;
        assert_eq!(b[i..i + 4], view::LINK, "the mark on each linked body");
        assert_ne!(a[i..i + 4], view::LINK, "no mark unlinked");
    }
}

// ---- STRIAUNDOHISTORYDRAG (LEDGER SR912): a drag's undo entry named by its kind (named tail block) ----

/// Every drag kind that commits names its undo entry with a `drag ` word within `WORD_MAX`; a marquee and a scrub commit
/// nothing and have no word.
#[test]
fn striaundohistorydrag_drag_kind_words() {
    use timeline_core::editor::WORD_MAX;
    use timeline_core::view::DragKind;
    let named = [
        (DragKind::TrimIn, "drag trim in"),
        (DragKind::TrimOut, "drag trim out"),
        (DragKind::TransitionIn, "drag transition in"),
        (DragKind::TransitionOut, "drag transition out"),
        (DragKind::Move, "drag move"),
        (DragKind::Slip, "drag slip"),
        (DragKind::Slide, "drag slide"),
    ];
    for (k, w) in named {
        assert_eq!(k.word(), Some(w), "{k:?}");
        assert!(w.len() <= WORD_MAX, "{w}");
    }
    assert_eq!(DragKind::Marquee.word(), None);
    assert_eq!(DragKind::Scrub.word(), None);
}

// ---- STRIATRACKORDERKEYS (LEDGER SR922): a reorder never re-times an implied audio fade; the track's name (named tail block) ----

/// V1, V2, A1, each A[0, 10)@0 + B[20, 30)@10 (the audio cut pairs with every video cut); V1's dissolve `d1`, V2's `d2`.
fn striatrackorderkeys_tl(d1: i64, d2: i64) -> Timeline {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 100);
    let b = tl.add_source("B", 60);
    for kind in [Kind::Video, Kind::Video, Kind::Audio] {
        let t = tl.add_track(kind);
        tl.tracks[t].clips = vec![Clip::new(a, 0, 10, 0), Clip::new(b, 20, 30, 10)];
    }
    tl = tl.add_transition(0, 10, timeline_core::TransitionKind::Dissolve, d1).unwrap();
    tl = tl.add_transition(1, 10, timeline_core::TransitionKind::Dissolve, d2).unwrap();
    tl.validate().unwrap();
    tl
}

/// Rule: the implied crossfade takes the FIRST video track's transition, so swapping two video tracks whose transitions on
/// the paired cut differ is refused by name (`EditError::Crossfade`), both ways; equal transitions swap.
#[test]
fn striatrackorderkeys_swap_that_retimes_a_crossfade_is_refused() {
    let tl = striatrackorderkeys_tl(2, 4);
    assert_eq!(tl.audio_crossfade(2, 1).map(|x| x.dur), Some(2));
    let before = tl.clone();
    assert_eq!(tl.swap_track(1, true), Err(EditError::Crossfade));
    assert_eq!(tl.swap_track(0, false), Err(EditError::Crossfade));
    assert_eq!(tl, before);
    let same = striatrackorderkeys_tl(3, 3);
    let up = same.swap_track(1, true).unwrap();
    assert_eq!(up.audio_crossfade(2, 1), same.audio_crossfade(2, 1));
}

/// Rule: an audio swap never trips the guard (the clips and their explicit crossfade travel with the track).
#[test]
fn striatrackorderkeys_audio_swap_is_allowed() {
    let mut tl = striatrackorderkeys_tl(2, 4);
    let a2 = tl.add_track(Kind::Audio);
    tl.tracks[a2].clips = tl.tracks[2].clips.clone();
    tl = tl.add_audio_xfade(a2, 10, 5, timeline_core::Curve::EqualPower).unwrap();
    let up = tl.swap_track(a2, true).unwrap();
    assert_eq!(up.crossfade_at(2, 1), tl.crossfade_at(a2, 1));
    assert_eq!(up.crossfade_at(a2, 1), tl.crossfade_at(2, 1));
}

/// `track_name`: the kind letter and the number within the kind; `None` past the last track.
#[test]
fn striatrackorderkeys_track_name() {
    let tl = striatrackreorder_tl(&[Kind::Video, Kind::Audio, Kind::Video, Kind::Audio]);
    let names: Vec<Option<String>> = (0..5).map(|t| tl.track_name(t)).collect();
    assert_eq!(names, vec![Some("V1".into()), Some("A1".into()), Some("V2".into()), Some("A2".into()), None]);
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

// ---- STRIALINKSLIDE (LEDGER SR927): a linked slide, all-or-nothing, and the linked drags' snap (named tail block) ----

/// `slide 2` on one member slides every mate by the same timeline delta (each track's butted next clip follows); ONE
/// undo step restores both tracks.
#[test]
fn strialinkslide_slide_carries_the_mates() {
    let tl = striacliplink_linked();
    let mut st = EditorState::new(tl.clone());
    st.select(0, 0).unwrap();
    assert_eq!(st.verb("slide 2"), Some(Ok(())));
    assert_eq!(st.timeline, tl.slide(0, 0, 2).unwrap().slide(1, 0, 2).unwrap());
    for k in 0..2 {
        let (c, n) = (st.timeline.tracks[k].clips[0], st.timeline.tracks[k].clips[1]);
        assert_eq!((c.at, n.at, n.src_in), (2, 12, 22), "track {k} slid");
    }
    assert_eq!(st.undo_depth(), 1, "one undo step");
    assert!(st.undo());
    assert_eq!(st.timeline, tl, "one undo restores both members");
}

/// A mate whose neighbour cannot follow refuses the whole slide (nothing committed); an unlinked clip slides exactly as
/// the trunk did.
#[test]
fn strialinkslide_refusal_is_all_or_nothing() {
    let mut tl = striacliplink_base();
    tl.tracks[1].clips[0] = Clip { src_in: 90, src_out: 100, ..tl.tracks[1].clips[0] };
    let base = tl.clone();
    let tl = tl.set_group(&[(0, 1), (1, 1)], 1).unwrap();
    let mut st = EditorState::new(tl.clone());
    st.select(0, 1).unwrap();
    assert_eq!(st.verb("slide 1"), Some(Err(EditError::OutOfSource)), "A1's butted first clip would pass its source");
    assert_eq!((&st.timeline, st.undo_depth()), (&tl, 0), "nothing committed");
    let mut st = EditorState::new(base.clone());
    st.select(0, 1).unwrap();
    assert_eq!(st.verb("slide 2"), Some(Ok(())));
    assert_eq!(st.timeline, base.slide(0, 1, 2).unwrap(), "group 0: the trunk's single-clip slide");
    assert_eq!(st.timeline.tracks[1].clips, base.tracks[1].clips, "A1 untouched");
}

/// A slide drag (the slip modifier on the out handle) previews and commits the linked slide through `slide_selected`.
#[test]
fn strialinkslide_drag_slide_carries_the_mates() {
    let tl = striacliplink_linked();
    let want = tl.slide(0, 0, 2).unwrap().slide(1, 0, 2).unwrap();
    let mut st = EditorState::new(tl);
    st.select(0, 0).unwrap();
    let l = Layout::of(400, 240, &st);
    let (z, y) = (l.zoom as f64, l.row_y(0) as f64 + ROW_H as f64 / 2.0);
    let x = l.x_of(10) as f64 - 2.0;
    let mut d = Drag::begin_with(&mut st, l, x, y, false, true, true).unwrap();
    assert_eq!(d.kind, DragKind::Slide);
    d.update(&st, x + 2.0 * z, y);
    assert_eq!(d.shown(&st).map(|s| s.timeline), Some(want.clone()), "the preview shows both members slid");
    assert_eq!(d.finish(&mut st), Ok(()));
    assert_eq!((&st.timeline, st.undo_depth()), (&want, 1));
}

/// NLE-LINK-SNAP: a linked pair's slide and trim-in drags of 5 px (one tick, inside `SNAP_PX`) keep their delta: the
/// mate's still edges are no snap target. The same drags on the unlinked base snap back to A1's edge (delta 0).
#[test]
fn strialinkslide_linked_drag_does_not_snap_to_its_mates() {
    let drag = |tl: Timeline, slip: bool, edge: i64, off: f64| {
        let mut st = EditorState::new(tl);
        st.select(0, 0).unwrap();
        st.seek(20);
        let l = Layout::of(400, 240, &st);
        let y = l.row_y(0) as f64 + ROW_H as f64 / 2.0;
        let x = l.x_of(edge) as f64 + off;
        let mut d = Drag::begin_with(&mut st, l, x, y, false, false, slip).unwrap();
        d.update(&st, x + 5.0, y);
        (d.kind, d.delta, d.guide, d, st)
    };
    let (k, delta, guide, d, mut st) = drag(striacliplink_linked(), true, 10, -1.0);
    assert_eq!((k, delta, guide), (DragKind::Slide, 1, None), "the linked slide keeps its tick");
    assert_eq!(d.finish(&mut st), Ok(()));
    let tl = striacliplink_linked();
    assert_eq!(st.timeline, tl.slide(0, 0, 1).unwrap().slide(1, 0, 1).unwrap());
    let (k, delta, guide, ..) = drag(striacliplink_base(), true, 10, -1.0);
    assert_eq!((k, delta, guide), (DragKind::Slide, 0, Some(0)), "unlinked: A1's in edge is a target (the first moving edge wins the tie)");
    let (k, delta, guide, ..) = drag(striacliplink_linked(), false, 0, 3.0);
    assert_eq!((k, delta, guide), (DragKind::TrimIn, 1, None), "the linked trim keeps its tick");
    let (k, delta, guide, ..) = drag(striacliplink_base(), false, 0, 3.0);
    assert_eq!((k, delta, guide), (DragKind::TrimIn, 0, Some(0)), "unlinked: A1's in edge is a target");
}

// ---- STRIALINKAUTOFUZZ (LEDGER SR928): link_av's near pass and the `link-av` word (named tail block) ----

/// V1 / A1 of one source `S` (100 ticks) holding the given clips (src_in, src_out, at).
fn strialinkautofuzz_tl(v: &[(i64, i64, i64)], a: &[(i64, i64, i64)]) -> Timeline {
    let mut tl = Timeline::new(25);
    let s = tl.add_source("S", 100);
    tl.add_track(Kind::Video);
    tl.add_track(Kind::Audio);
    tl.tracks[0].clips = v.iter().map(|&(i, o, at)| Clip::new(s, i, o, at)).collect();
    tl.tracks[1].clips = a.iter().map(|&(i, o, at)| Clip::new(s, i, o, at)).collect();
    tl.validate().unwrap();
    tl
}

fn strialinkautofuzz_groups(t: &Timeline) -> Vec<Vec<u32>> {
    t.tracks.iter().map(|tr| tr.clips.iter().map(|c| c.group).collect()).collect()
}

#[test]
fn strialinkautofuzz_near_pairs_link() {
    let (_, a, b) = base();
    // A1's second clip one tick later and one tick shorter than V1's: a rounding twin, linked with its own id.
    let mut tl = striacliplink_base();
    tl.tracks[1].clips[1] = Clip::new(b, 20, 29, 11);
    tl.validate().unwrap();
    assert_eq!(strialinkautofuzz_groups(&tl.link_av()), vec![vec![1, 2], vec![1, 2]], "exact pair 1, near pair 2");
    assert_eq!(tl.link_av().mates(0, 1), vec![(0, 1), (1, 1)]);
    // Two ticks shorter: not a twin.
    tl.tracks[1].clips[1] = Clip::new(b, 20, 28, 11);
    assert_eq!(strialinkautofuzz_groups(&tl.link_av()), vec![vec![1, 0], vec![1, 0]]);
    // Another source within one tick: not a twin either.
    tl.tracks[1].clips[1] = Clip::new(a, 20, 29, 11);
    assert_eq!(strialinkautofuzz_groups(&tl.link_av()), vec![vec![1, 0], vec![1, 0]]);
}

#[test]
fn strialinkautofuzz_ambiguous_left_unlinked() {
    // A length-2 video clip over two length-1 audio clips at its tick and the next: both near, neither exact.
    let tl = strialinkautofuzz_tl(&[(0, 2, 0)], &[(0, 1, 0), (1, 2, 1)]);
    assert_eq!(tl.link_av(), tl, "two audio candidates: nothing linked");
    // Two video clips near one audio clip.
    let tl = strialinkautofuzz_tl(&[(0, 1, 0), (1, 2, 1)], &[(0, 2, 0)]);
    assert_eq!(tl.link_av(), tl, "two video candidates: nothing linked");
}

#[test]
fn strialinkautofuzz_exact_wins_over_near() {
    let tl = strialinkautofuzz_tl(&[(0, 1, 0)], &[(0, 1, 0), (1, 2, 1)]);
    assert_eq!(strialinkautofuzz_groups(&tl.link_av()), vec![vec![1], vec![1, 0]], "the exact pair links; the near one stays free");
}

#[test]
fn strialinkautofuzz_link_av_word() {
    let tl = striacliplink_base();
    let mut st = EditorState::new(tl.clone());
    st.select(0, 1).unwrap();
    assert_eq!(st.set, vec![(0, 1)], "unlinked: the clip alone");
    assert_eq!(st.verb("link-av"), Some(Ok(())));
    assert_eq!((st.timeline.clone(), st.undo_depth()), (tl.link_av(), 1), "both pairs linked, one commit");
    assert_eq!(st.set, vec![(0, 1), (1, 1)], "the selected clip brings its new mate");
    assert_eq!(st.verb("link-av"), Some(Ok(())), "nothing new to link");
    assert_eq!(st.undo_depth(), 1, "nothing new linked: no commit");
    assert!(st.undo());
    assert_eq!(st.timeline, tl, "one undo step parts both pairs");
}

// ---- STRIAGAPSALLTRACKS (LEDGER SR929): close-gaps on every track, all-or-nothing (named tail block) ----

/// SR857's base: V1 A@5|B@15|A@30 with a dissolve at 15, A1 A@0|B@20|A@30 with an explicit xfade at 30. Closing every track is
/// the per-track close chained; every gap goes; A1's cut at 10 now pairs with V1's moved dissolve (a GAINED derived crossfade,
/// its handle fits) and the explicit xfade moves with its own track to 20.
#[test]
fn striagapsalltracks_closes_every_track() {
    let (tl, _, _) = striagapclose_base();
    let c = tl.close_gaps_all().unwrap();
    assert_eq!(c, tl.close_gaps(0).unwrap().close_gaps(1).unwrap());
    assert_eq!((c.gap_count(0), c.gap_count(1), c.gap_count_all(), tl.gap_count_all()), (0, 0, 0, tl.gap_count(0) + tl.gap_count(1)));
    assert_eq!(c.audio_crossfade(1, 1).map(|x| (x.at, x.dur)), Some((10, 3)));
    assert_eq!(c.audio_xfades[0].at, 20);
    assert_eq!(c.validate(), Ok(()));
    assert_eq!(c.close_gaps_all().unwrap(), c);
}

/// A track that refuses refuses the whole word, named by its index: A1's B closing onto A[90,100) pairs with V1's dissolve at
/// 10, and A's handle is 0 (`Handle`).
#[test]
fn striagapsalltracks_refuses_the_whole_word_by_track() {
    let mut k = Timeline::new(25);
    let (a, b) = (k.add_source("A", 100), k.add_source("B", 60));
    k.add_track(Kind::Video);
    k.add_track(Kind::Audio);
    k.tracks[0].clips = vec![Clip::new(a, 0, 10, 0), Clip::new(b, 0, 10, 10)];
    k.tracks[1].clips = vec![Clip::new(a, 90, 100, 0), Clip::new(b, 0, 10, 15)];
    let k = k.add_transition(0, 10, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    assert_eq!(k.close_gaps_all(), Err((1, EditError::Handle)));
}

/// A derived crossfade LOST by closing tracks differently is refused by name: V1 A@5|B@15 dissolve at 15 pairs with A1's cut
/// at 15 (A[0,15)@0|B@15, no gap); V1's close moves the dissolve to 10 and A1's cut stays at 15 (`Transition`, track 1).
#[test]
fn striagapsalltracks_refuses_a_lost_pairing() {
    let mut k = Timeline::new(25);
    let (a, b) = (k.add_source("A", 100), k.add_source("B", 60));
    k.add_track(Kind::Video);
    k.add_track(Kind::Audio);
    k.tracks[0].clips = vec![Clip::new(a, 0, 10, 5), Clip::new(b, 0, 10, 15)];
    k.tracks[1].clips = vec![Clip::new(a, 0, 15, 0), Clip::new(b, 0, 10, 15)];
    let k = k.add_transition(0, 15, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    assert!(k.audio_crossfade(1, 1).is_some());
    assert!(k.close_gaps(0).is_ok());
    assert_eq!(k.close_gaps_all(), Err((1, EditError::Transition)));
}

/// `close-gaps all` through THE verb dispatch: no selection needed, one commit, a second is an equal value (nothing), undo
/// restores; a refusal commits nothing; a third word is not this verb.
#[test]
fn striagapsalltracks_editor_one_undo_step() {
    use timeline_core::editor::EditorState;
    let (tl, _, _) = striagapclose_base();
    let mut st = EditorState::new(tl.clone());
    assert_eq!(st.verb("close-gaps all"), Some(Ok(())));
    assert_eq!((st.timeline.clone(), st.undo_depth()), (tl.close_gaps_all().unwrap(), 1));
    assert_eq!(st.verb("close-gaps all"), Some(Ok(())));
    assert_eq!(st.undo_depth(), 1);
    assert!(st.undo());
    assert_eq!(st.timeline, tl);
    assert_eq!(st.verb("close-gaps all x"), None);
    let mut k = Timeline::new(25);
    let (a, b) = (k.add_source("A", 100), k.add_source("B", 60));
    k.add_track(Kind::Video);
    k.add_track(Kind::Audio);
    k.tracks[0].clips = vec![Clip::new(a, 0, 10, 0), Clip::new(b, 0, 10, 10)];
    k.tracks[1].clips = vec![Clip::new(a, 90, 100, 0), Clip::new(b, 0, 10, 15)];
    let k = k.add_transition(0, 10, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    let mut st = EditorState::new(k.clone());
    assert_eq!(st.verb("close-gaps all"), Some(Err(EditError::Handle)));
    assert_eq!((st.timeline.clone(), st.undo_depth()), (k, 0));
}

// ---- STRIAXFORMPRESETS (LEDGER SR938): the placement's corner presets and centre-kept scale steps (named tail block) ----
mod striaxformpresets {
    use super::base;
    use timeline_core::editor::EditorState;
    use timeline_core::{EditError, Xform};

    #[test]
    fn striaxformpresets_corners_and_steps() {
        assert_eq!(Xform::corner("tl"), Some(Xform { x: 25, y: 25, s: 250 }));
        assert_eq!(Xform::corner("tr"), Some(Xform { x: 725, y: 25, s: 250 }));
        assert_eq!(Xform::corner("bl"), Some(Xform { x: 25, y: 725, s: 250 }));
        assert_eq!(Xform::corner("br"), Some(Xform::PIP));
        assert_eq!(Xform::corner("off"), None);
        assert!(["tl", "tr", "bl", "br"].iter().all(|c| Xform::corner(c).is_some_and(Xform::ok)));
        let d = Xform::SCALE_STEP as i32;
        assert_eq!(Xform::PIP.stepped(d), Some(Xform { x: 700, y: 700, s: 300 }));
        assert_eq!(Xform::PIP.stepped(-d), Some(Xform { x: 750, y: 750, s: 200 }));
        assert_eq!(Xform::NONE.stepped(-d), Some(Xform { x: 25, y: 25, s: 950 }));
        assert_eq!(Xform::NONE.stepped(d), None, "past the whole frame");
        assert_eq!(Xform { x: 0, y: 0, s: 500 }.stepped(d), None, "off the left / top edge");
        assert_eq!(Xform { x: 500, y: 500, s: 500 }.stepped(d), None, "off the right / bottom edge");
        assert_eq!(Xform { x: 10, y: 10, s: 50 }.stepped(-d), None, "the rect would vanish");
        assert_eq!(Xform { x: 10, y: 10, s: 51 }.stepped(-d), Some(Xform { x: 35, y: 35, s: 1 }));
    }

    #[test]
    fn striaxformpresets_words_one_undo_step_each() {
        let (tl, _, _) = base();
        let mut st = EditorState::new(tl.clone());
        assert_eq!(st.verb("pip tl"), Some(Err(EditError::NoClip)));
        assert_eq!(st.verb("scale+"), Some(Err(EditError::NoClip)));
        st.select(0, 1).unwrap();
        assert_eq!(st.verb("pip tr"), Some(Ok(())));
        assert_eq!((st.undo_depth(), st.selected().map(|c| c.xform)), (1, Some(Xform { x: 725, y: 25, s: 250 })));
        assert_eq!(st.verb("pip tr"), Some(Ok(())));
        assert_eq!(st.undo_depth(), 1, "the same placement commits nothing");
        assert_eq!(st.verb("scale+"), Some(Ok(())));
        assert_eq!(st.verb("scale-"), Some(Ok(())));
        assert_eq!(st.verb("scale-"), Some(Ok(())));
        assert_eq!((st.undo_depth(), st.selected().map(|c| c.xform)), (4, Some(Xform { x: 750, y: 50, s: 200 })));
        assert_eq!(st.verb("pip off"), Some(Ok(())));
        assert_eq!(st.verb("scale+"), Some(Err(EditError::Xform)));
        assert_eq!(st.undo_depth(), 5, "a refused step commits nothing");
        assert_eq!(st.verb("pip on"), Some(Err(EditError::Xform)), "SR879's arms still refuse an unknown word");
        assert_eq!(st.verb("scale+ 1"), None, "not a step word");
        for _ in 0..5 {
            assert!(st.undo());
        }
        assert_eq!(st.timeline, tl);
        st.timeline.tracks[1].clips = tl.tracks[0].clips.clone();
        st.select(1, 0).unwrap();
        assert_eq!(st.verb("pip bl"), Some(Err(EditError::Kind)));
        assert_eq!(st.verb("scale-"), Some(Err(EditError::Kind)));
    }
}

// ---- STRIANUDGEMORE (LEDGER SR944): `nudge_words` is the one parser of the free count (named tail block) ----

/// The faces and the witness call `editor::nudge_words` for `nudge <d>` instead of re-parsing it: a signed nonzero count
/// is taken, any other shape is not the verb.
#[test]
fn strianudgemore_nudge_words_is_the_one_parser() {
    use timeline_core::editor::nudge_words;
    assert_eq!(nudge_words("nudge -5"), Some(-5));
    assert_eq!(nudge_words("nudge 10"), Some(10));
    for v in ["nudge 0", "nudge+", "nudge-10", "nudge 1 2", "nudge x", "nudge", "slip 3"] {
        assert_eq!(nudge_words(v), None, "{v}");
    }
    println!("strianudgemore: nudge_words takes `nudge <d>` only");
}

// ---- STRIAOPACITYMORE (LEDGER SR958): each clip of a set from its own opacity; the label names its percent (named tail block) ----

/// A set at 50 % / 100 %: `opacity-` steps each from its own value (40 / 90) as ONE undo step; `opacity+` twice reads
/// 60 / 100 then 70 / 100 (the opaque one clamps); undo restores; a set all opaque commits nothing on `opacity+`.
#[test]
fn striaopacitymore_mixed_set_steps_each_from_its_own() {
    use timeline_core::editor::EditorState;
    let (tl, _, _) = base();
    let tl = tl.set_opacity(&[(0, 0)], 50).unwrap();
    let mut st = EditorState::new(tl.clone());
    st.select_all();
    let both = |st: &EditorState| (st.timeline.tracks[0].clips[0].opacity, st.timeline.tracks[0].clips[1].opacity);
    assert_eq!(st.verb("opacity-"), Some(Ok(())));
    assert_eq!((both(&st), st.undo_depth()), ((40, 90), 1));
    assert_eq!(st.verb("opacity+"), Some(Ok(())));
    assert_eq!((both(&st), st.undo_depth()), ((50, 100), 2));
    assert_eq!(st.verb("opacity+"), Some(Ok(())));
    assert_eq!((both(&st), st.undo_depth()), ((60, 100), 3));
    assert_eq!(st.verb("opacity+"), Some(Ok(())));
    assert_eq!(both(&st), (70, 100));
    assert!(st.undo());
    assert_eq!(both(&st), (60, 100));
    let mut op = EditorState::new(tl.set_opacity(&[(0, 0)], timeline_core::OPAQUE).unwrap());
    op.select_all();
    assert_eq!(op.verb("opacity+"), Some(Ok(())));
    assert_eq!((both(&op), op.undo_depth()), ((100, 100), 0), "all opaque: nothing commits");
    let mut none = EditorState::new(tl);
    assert_eq!(none.step_opacity_each(-10), Err(EditError::NoClip));
    println!("striaopacitymore: mixed set 50/100 -> 40/90 (1 undo), +3 -> 70/100");
}

/// The clip label: an opaque clip reads its reel (`A`), at 50 % `A 50%`, set back to opaque `A` again.
#[test]
fn striaopacitymore_label_names_the_percent() {
    use timeline_core::editor::EditorState;
    use timeline_core::view::{self, Canvas, Order, Text, opacity_label};
    struct Rec(Vec<String>);
    impl Text for Rec {
        fn text(&mut self, _: &mut Canvas, s: &str, _: f32, _: f32, _: (i32, i32)) {
            self.0.push(String::from(s));
        }
    }
    let (tl, _, _) = base();
    let first = |tl: &Timeline| {
        let mut rec = Rec(Vec::new());
        view::paint(&EditorState::new(tl.clone()), 320, 160, Order::Rgba, Some(&mut rec as &mut dyn Text), None);
        rec.0.into_iter().find(|s| s.starts_with('A')).expect("the A clip's label")
    };
    let half = tl.set_opacity(&[(0, 0)], 50).unwrap();
    let got = [first(&tl), first(&half), first(&half.set_opacity(&[(0, 0)], timeline_core::OPAQUE).unwrap())];
    assert_eq!(got, ["A", "A 50%", "A"]);
    assert_eq!(opacity_label(String::from("A"), 0), "A 0%");
    assert_eq!(opacity_label(String::from("A"), timeline_core::OPAQUE), "A");
    println!("striaopacitymore: labels {got:?}");
}

// ---- STRIATRACKSMORE (LEDGER SR955): undo / redo of a track add or remove remap the selection (named tail block) ----
mod striatracksmore {
    use timeline_core::editor::{EditorState, TrackMap, UNDO_MAX};
    use timeline_core::{Clip, Kind, Timeline};

    /// V1 A[0,20)@0 B[0,20)@20; A1 the same; A2 A[0,10)@0 (A2's clip differs from A1's first).
    fn three() -> Timeline {
        let mut tl = Timeline::new(25);
        let (a, b) = (tl.add_source("A", 100), tl.add_source("B", 100));
        let (v1, a1, a2) = (tl.add_track(Kind::Video), tl.add_track(Kind::Audio), tl.add_track(Kind::Audio));
        tl.tracks[v1].clips = vec![Clip::new(a, 0, 20, 0), Clip::new(b, 0, 20, 20)];
        tl.tracks[a1].clips = tl.tracks[v1].clips.clone();
        tl.tracks[a2].clips = vec![Clip::new(a, 0, 10, 0)];
        tl
    }

    /// The map's forward and inverse take each other back; a removed track has no forward, an inserted one no inverse.
    #[test]
    fn striatracksmore_track_map_fwd_inv() {
        for m in [TrackMap::Insert(1), TrackMap::Remove(1), TrackMap::Swap(0, 2)] {
            for t in 0..4 {
                if let Some(f) = m.fwd(t) {
                    assert_eq!(m.inv(f), Some(t), "{m:?} {t}");
                }
            }
        }
        assert_eq!((TrackMap::Remove(1).fwd(1), TrackMap::Insert(1).inv(1)), (None, None));
        assert_eq!((TrackMap::Insert(1).fwd(0), TrackMap::Insert(1).fwd(1), TrackMap::Remove(1).inv(1)), (Some(0), Some(2), Some(2)));
        assert_eq!((TrackMap::Swap(0, 2).fwd(0), TrackMap::Swap(0, 2).inv(1)), (Some(2), Some(1)));
    }

    /// `track- V` then undo: the A2 selection is back on A2's own clip (it read A1's before: `settle` only bounds-checks);
    /// redo moves it down again; `track+ video` then undo / redo the same; a plain edit after an undo clears the redo side.
    #[test]
    fn striatracksmore_undo_redo_remap_selection() {
        let tl = three();
        let a2 = tl.tracks[2].clips[0];
        let mut st = EditorState::new(tl.clone());
        st.select(2, 0).unwrap();
        assert_eq!(st.verb("track- V"), Some(Ok(())));
        assert_eq!((st.selection, st.selected()), (Some((1, 0)), Some(a2)));
        assert!(st.undo());
        assert_eq!((st.timeline.clone(), st.selection, st.set.clone(), st.selected()), (tl.clone(), Some((2, 0)), vec![(2, 0)], Some(a2)));
        assert!(st.redo());
        assert_eq!((st.timeline.clone(), st.selection, st.selected()), (tl.remove_track(0).unwrap(), Some((1, 0)), Some(a2)));
        assert!(st.undo());
        assert_eq!(st.verb("track+ video"), Some(Ok(())));
        assert_eq!((st.selection, st.selected()), (Some((3, 0)), Some(a2)));
        assert!(st.undo());
        assert_eq!((st.timeline.clone(), st.selection, st.selected()), (tl.clone(), Some((2, 0)), Some(a2)));
        assert!(st.redo());
        assert_eq!((st.selection, st.selected()), (Some((3, 0)), Some(a2)));
        assert!(st.undo());
        st.select(1, 0).unwrap();
        assert_eq!(st.verb("delete"), Some(Ok(())));
        assert!(!st.redo(), "a commit clears the redo side");
        assert!(st.undo());
        assert_eq!(st.timeline, tl);
    }

    /// Past UNDO_MAX the maps trim with the timelines: 300 alternating `track+ video` / `track- V2`, then every kept undo
    /// leaves the selection on A2's clip (a map off by one entry would land it elsewhere).
    #[test]
    fn striatracksmore_maps_stay_in_lock_step_past_undo_max() {
        let tl = three();
        let a2 = tl.tracks[2].clips[0];
        let mut st = EditorState::new(tl.clone());
        for i in 0..300 {
            assert_eq!(st.verb(if i % 2 == 0 { "track+ video" } else { "track- V2" }), Some(Ok(())), "{i}");
        }
        assert_eq!((st.undo_depth(), st.timeline.clone()), (UNDO_MAX, tl.clone()));
        st.select(2, 0).unwrap();
        let mut n = 0;
        while st.undo() {
            n += 1;
            assert_eq!(st.selected(), Some(a2), "undo {n}");
        }
        assert_eq!((n, st.selection), (UNDO_MAX, Some((2, 0))));
        while st.redo() {
            assert_eq!(st.selected(), Some(a2));
        }
        assert_eq!(st.timeline, tl);
    }
}

// ---- STRIARIPPLEMORE (LEDGER SR953): the ripple across every track (named tail block) ----

/// `base()` with A1 holding V1's twins (A[0,10)@0 B[20,30)@10) and a 4-tick dissolve on V1's cut at 10, so A1's cut at
/// 10 carries the derived crossfade.
fn striaripplemore_linked() -> Timeline {
    let (mut tl, _, _) = base();
    tl.tracks[1].clips = tl.tracks[0].clips.clone();
    let tl = tl.add_transition(0, 10, timeline_core::TransitionKind::Dissolve, 4).unwrap();
    assert!(tl.audio_crossfade(1, 1).is_some_and(|x| x.at == 10));
    tl
}

#[test]
fn striaripplemore_the_twin_follows_and_the_derived_crossfade_stays_paired() {
    let (_, a, b) = base();
    let tl = striaripplemore_linked();
    let r = tl.ripple_all_out(0, 0, 7).unwrap();
    let want = vec![Clip::new(a, 0, 7, 0), Clip::new(b, 20, 30, 7)];
    assert_eq!((&r.tracks[0].clips, &r.tracks[1].clips), (&want, &want), "V1 and its A1 twin ripple together");
    assert_eq!(r.tracks[0].transitions.iter().map(|x| (x.at, x.dur)).collect::<Vec<_>>(), vec![(7, 4)]);
    assert_eq!(r.audio_crossfade(1, 1).map(|x| x.at), Some(7), "the derived crossfade is still paired");
    let one = tl.ripple_trim_out(0, 0, 7).unwrap();
    assert_eq!(one.audio_crossfade(1, 1), None, "the one-track ripple un-pairs it");
    let back = r.ripple_all_out(0, 0, 10).unwrap();
    assert_eq!(back, tl, "the ripple back is the original timeline");
}

#[test]
fn striaripplemore_a_twin_less_track_shifts_its_later_clips() {
    let (mut tl, a, b) = base();
    tl.tracks[1].clips = vec![Clip::new(b, 0, 5, 0), Clip::new(a, 50, 55, 22)];
    tl.validate().unwrap();
    let r = tl.ripple_all_out(0, 0, 7).unwrap();
    assert_eq!(r.tracks[0].clips, vec![Clip::new(a, 0, 7, 0), Clip::new(b, 20, 30, 7)]);
    assert_eq!(r.tracks[1].clips, vec![Clip::new(b, 0, 5, 0), Clip::new(a, 50, 55, 19)], "the clip before the edge stays");
    let g = tl.ripple_all_in(0, 1, 18).unwrap();
    assert_eq!(g.tracks[0].clips, vec![Clip::new(a, 0, 10, 0), Clip::new(b, 18, 30, 10)]);
    assert_eq!(g.tracks[1].clips, vec![Clip::new(b, 0, 5, 0), Clip::new(a, 50, 55, 24)], "a lengthening ripple pushes later");
}

#[test]
fn striaripplemore_a_straddler_is_refused_by_name() {
    let (mut tl, a, _) = base();
    tl.tracks[1].clips = vec![Clip::new(a, 50, 60, 5)];
    tl.validate().unwrap();
    let before = tl.clone();
    assert_eq!(tl.ripple_all_out(0, 0, 7), Err(EditError::Overlap));
    assert_eq!(tl.ripple_all_out(0, 0, 12), Err(EditError::Overlap));
    assert_eq!(tl.ripple_all_out(0, 2, 7), Err(EditError::NoClip));
    assert_eq!(tl.ripple_all_out(9, 0, 7), Err(EditError::NoTrack));
    assert_eq!(tl, before);
}

#[test]
fn striaripplemore_the_editor_word_is_one_undo_step() {
    let (_, a, b) = base();
    let tl = striaripplemore_linked();
    let mut st = timeline_core::editor::EditorState::new(tl.clone());
    st.select(0, 1).unwrap();
    assert_eq!(st.verb("rtrim-all in 3"), Some(Ok(())));
    let want = vec![Clip::new(a, 0, 10, 0), Clip::new(b, 23, 30, 10)];
    assert_eq!((&st.timeline.tracks[0].clips, &st.timeline.tracks[1].clips), (&want, &want));
    assert_eq!(st.timeline, tl.ripple_all_in(0, 1, 23).unwrap());
    assert_eq!(st.undo_depth(), 1);
    for bad in ["rtrim-all up 3", "rtrim-all in", "rtrim-allin 3", "rtrim-all in 3 x"] {
        assert_eq!(st.verb(bad), None, "{bad}");
    }
    assert!(st.undo());
    assert_eq!(st.timeline, tl);
}

// ---- STRIALINKAUTOMORE (LEDGER SR968): the near pass also needs src_in within one tick (named tail block) ----

#[test]
fn strialinkautomore_near_needs_src_in() {
    let (_, _, b) = base();
    // A1's second clip one tick later and one tick longer than V1's B[20,30)@10, but from src_in 25: another piece of B.
    let mut tl = striacliplink_base();
    tl.tracks[1].clips[1] = Clip::new(b, 25, 34, 11);
    tl.validate().unwrap();
    assert_eq!(strialinkautofuzz_groups(&tl.link_av()), vec![vec![1, 0], vec![1, 0]], "src_in five ticks apart: not a twin");
    // src_in one tick apart: a rounding twin, linked with its own id.
    tl.tracks[1].clips[1] = Clip::new(b, 21, 30, 11);
    tl.validate().unwrap();
    assert_eq!(strialinkautofuzz_groups(&tl.link_av()), vec![vec![1, 2], vec![1, 2]], "src_in within one tick: linked");
}

// ---- STRIAZOOMFITMORE (LEDGER SR965): the view anchor rule both faces apply (named tail block) ----

/// `Layout::anchor`: a stored origin `(t0, at)` holds while the playhead is `at` or inside the anchored view, else the
/// page holding the playhead; `None` leaves `Layout::of`'s page; a fit's origin anchored puts the range's start at x 0.
#[test]
fn striazoomfitmore_anchor_rule() {
    use timeline_core::editor::EditorState;
    use timeline_core::view::Layout;
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(a, 13, 23, 0), Clip::new(b, 0, 10, 10), Clip::new(a, 23, 33, 25)];
    let mut st = EditorState::new(tl);
    st.select(0, 1).unwrap();
    let t0 = st.fit(320, true).unwrap();
    assert_eq!((t0, st.zoom), (10, 32)); // 10 ticks per 320 px page
    let view = Some((t0, st.playhead));
    let l = Layout::of(320, 160, &st);
    assert_eq!(l.t0, 0, "the page holding the playhead");
    assert_eq!(l.anchor(None, st.playhead), l, "no view: unchanged");
    let anchored = l.anchor(view, st.playhead);
    assert_eq!((anchored.t0, anchored.x_of(10)), (10, 0), "at the anchor: the fit's range starts at x 0");
    assert_eq!(anchored.x_of(20), 320, "and ends at the window's right edge");
    st.seek(19);
    assert_eq!(Layout::of(320, 160, &st).anchor(view, st.playhead).t0, 10, "inside the view: kept");
    st.seek(5);
    assert_eq!(Layout::of(320, 160, &st).anchor(view, st.playhead).t0, 0, "outside, away from the anchor: the page");
    st.seek(25);
    assert_eq!(Layout::of(320, 160, &st).anchor(view, st.playhead).t0, 20, "outside: the page holding the playhead");
}

// ---- STRIALINKMORE (LEDGER SR967): `alone` narrows the selection set to the primary, no edit (named tail block) ----

/// V1's and A1's SECOND clips (B[20,30)@10, nothing after them) linked as group 1.
fn strialinkmore_linked() -> Timeline {
    striacliplink_base().set_group(&[(0, 1), (1, 1)], 1).unwrap()
}

#[test]
fn strialinkmore_alone_narrows_the_set() {
    let tl = strialinkmore_linked();
    let mut st = EditorState::new(tl.clone());
    assert_eq!(st.verb("alone"), Some(Err(EditError::NoClip)), "nothing selected");
    st.select(0, 1).unwrap();
    assert_eq!(st.set, vec![(0, 1), (1, 1)]);
    assert_eq!(st.verb("alone"), Some(Ok(())));
    assert_eq!((st.selection, st.set.clone()), (Some((0, 1)), vec![(0, 1)]));
    assert_eq!((&st.timeline, st.undo_depth()), (&tl, 0), "a selection word: no edit, no undo step");
    assert_eq!(st.verb("nudge+"), Some(Ok(())));
    assert_eq!(st.timeline, tl.move_many(&[(0, 1)], 1).unwrap(), "the set op moves the primary alone");
    assert_eq!(st.timeline.tracks[1].clips[1].at, 10, "its mate stays");
    assert_eq!(st.undo_depth(), 1);
    st.select(0, 1).unwrap();
    assert_eq!(st.set, vec![(0, 1), (1, 1)], "a fresh select brings the mate back");
}

#[test]
fn strialinkmore_alone_on_an_unlinked_clip_keeps_it() {
    let tl = striacliplink_base();
    let mut st = EditorState::new(tl.clone());
    st.select(0, 1).unwrap();
    st.select_add(1, 1).unwrap();
    assert_eq!(st.verb("alone"), Some(Ok(())));
    assert_eq!((st.selection, st.set.clone()), (Some((1, 1)), vec![(1, 1)]), "the primary is the last added");
    assert_eq!((&st.timeline, st.undo_depth()), (&tl, 0));
}

// ---- STRIAGAPSMORE (LEDGER SR969): close-gaps sync, the spans empty on every track (named tail block) ----

/// SR909's black base: V1 A@0|B@20|A@30|B@45, A1 A@0. The spans no track covers are 10..20 and 40..45; A1 has no clip
/// there, so the sync close is V1's own close (both black fades of 10..20 go, V1 abutting on both sides).
#[test]
fn striagapsmore_sync_close_on_one_covered_track_is_its_own_close() {
    let k = striagapsblack_base();
    assert_eq!(k.sync_gaps(), vec![(10, 20), (40, 45)]);
    let c = k.close_gaps_sync().unwrap();
    assert_eq!(c, k.close_gaps(0).unwrap());
    assert_eq!(c.sync_gaps(), Vec::<(i64, i64)>::new());
    assert_eq!(c.close_gaps_sync().unwrap(), c);
    println!("striagapsmore: V1 {:?}", c.tracks[0].clips);
}

/// SR857's base: every tick 0..40 is covered by one track or the other: no sync gap, an equal value, although each track
/// has gaps of its own (the per-track and all-tracks closes would move them).
#[test]
fn striagapsmore_no_span_empty_everywhere_is_an_equal_value() {
    let (tl, _, _) = striagapclose_base();
    assert_eq!(tl.sync_gaps(), Vec::<(i64, i64)>::new());
    assert_eq!(tl.close_gaps_sync().unwrap(), tl);
    assert!(tl.gap_count_all() > 0);
}

/// Differing layouts: V1 A[0,10)@5 | B[20,30)@25 with a from-black at 25, A1 A[0,10)@8 | B[20,30)@30. The spans empty on
/// both are 0..5 (leading) and 18..25; every track moves alike: V1 A@0 B@13 (its own gap 10..13 stays open, so its
/// from-black stays, now at 13), A1 A@3 B@18. The value validates and round-trips through the EDL.
#[test]
fn striagapsmore_every_track_moves_alike() {
    let mut k = Timeline::new(25);
    let (a, b) = (k.add_source("A", 100), k.add_source("B", 60));
    k.add_track(Kind::Video);
    k.add_track(Kind::Audio);
    k.tracks[0].clips = vec![Clip::new(a, 0, 10, 5), Clip::new(b, 20, 30, 25)];
    k.tracks[1].clips = vec![Clip::new(a, 0, 10, 8), Clip::new(b, 20, 30, 30)];
    let k = k.add_transition_from_black(0, 25, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    assert_eq!(k.sync_gaps(), vec![(0, 5), (18, 25)]);
    let c = k.close_gaps_sync().unwrap();
    assert_eq!(c.tracks[0].clips, vec![Clip::new(a, 0, 10, 0), Clip::new(b, 20, 30, 13)]);
    assert_eq!(c.tracks[1].clips, vec![Clip::new(a, 0, 10, 3), Clip::new(b, 20, 30, 18)]);
    let xs: Vec<(i64, bool, bool)> = c.tracks[0].transitions.iter().map(|x| (x.at, x.from_black, x.to_black)).collect();
    assert_eq!(xs, vec![(13, true, false)]);
    assert_eq!(c.sync_gaps(), Vec::<(i64, i64)>::new());
    assert_eq!(c.validate(), Ok(()));
    let text = timeline_core::edl::write(&c, "GAPSMORE").unwrap();
    let back = timeline_core::edl::parse(&text, 25).unwrap();
    assert_eq!(timeline_core::edl::write(&back, "GAPSMORE").unwrap(), text);
    println!("striagapsmore: V1 {:?} A1 {:?}", c.tracks[0].clips, c.tracks[1].clips);
}

/// `close-gaps sync` through THE verb dispatch: no selection needed, one commit, a second is an equal value (nothing), undo
/// restores; a third word is not this verb.
#[test]
fn striagapsmore_editor_one_undo_step() {
    use timeline_core::editor::EditorState;
    let k = striagapsblack_base();
    let mut st = EditorState::new(k.clone());
    assert_eq!(st.verb("close-gaps sync"), Some(Ok(())));
    assert_eq!((st.timeline.clone(), st.undo_depth()), (k.close_gaps_sync().unwrap(), 1));
    assert_eq!(st.verb("close-gaps sync"), Some(Ok(())));
    assert_eq!(st.undo_depth(), 1);
    assert!(st.undo());
    assert_eq!(st.timeline, k);
    assert_eq!(st.verb("close-gaps sync x"), None);
}

// ---- STRIAXFORMMORE (LEDGER SR975): the placement's clamped step and nudge (named tail block) ----
mod striaxformmore {
    use super::base;
    use timeline_core::editor::EditorState;
    use timeline_core::{EditError, Xform};

    #[test]
    fn striaxformmore_clamp_step_and_nudge() {
        let d = Xform::SCALE_STEP as i32;
        let n = Xform::NUDGE_STEP as i32;
        assert_eq!(Xform::clamped(2000, -5, 7), Xform::NONE);
        assert_eq!(Xform::clamped(0, 2000, -1), Xform { x: 999, y: 0, s: 1 });
        assert_eq!(Xform::PIP.fit_step(d), Xform::PIP.stepped(d), "a step that fits is SR938's");
        assert_eq!(Xform { x: 0, y: 0, s: 500 }.fit_step(d), Some(Xform { x: 0, y: 0, s: 550 }));
        assert_eq!(Xform { x: 500, y: 500, s: 500 }.fit_step(d), Some(Xform { x: 450, y: 450, s: 550 }));
        assert_eq!(Xform { x: 10, y: 10, s: 50 }.fit_step(-d), Some(Xform { x: 34, y: 34, s: 1 }));
        assert_eq!(Xform { x: 0, y: 0, s: 980 }.fit_step(d), Some(Xform::NONE));
        assert_eq!(Xform::NONE.fit_step(d), None, "the whole frame cannot grow");
        assert_eq!(Xform { x: 3, y: 4, s: 1 }.fit_step(-d), None, "the smallest rect cannot shrink");
        assert_eq!(Xform::PIP.nudged(n, 0), Some(Xform { x: 750, y: 725, s: 250 }));
        assert_eq!(Xform { x: 750, y: 725, s: 250 }.nudged(n, 0), None, "at the right edge");
        assert_eq!(Xform { x: 10, y: 725, s: 250 }.nudged(-n, -n), Some(Xform { x: 0, y: 700, s: 250 }));
        assert_eq!(Xform::NONE.nudged(0, n), None, "the whole frame cannot move");
    }

    #[test]
    fn striaxformmore_nudge_words_one_undo_step_each() {
        let (tl, _, _) = base();
        let mut st = EditorState::new(tl.clone());
        assert_eq!(st.verb("pip left"), Some(Err(EditError::NoClip)));
        st.select(0, 1).unwrap();
        assert_eq!(st.verb("pip left"), Some(Err(EditError::Xform)), "the whole frame cannot move");
        assert_eq!(st.undo_depth(), 0);
        assert_eq!(st.verb("pip tl"), Some(Ok(())));
        assert_eq!(st.verb("pip left"), Some(Ok(())));
        assert_eq!((st.undo_depth(), st.selected().map(|c| c.xform)), (2, Some(Xform { x: 0, y: 25, s: 250 })));
        assert_eq!(st.verb("pip left"), Some(Err(EditError::Xform)));
        assert_eq!(st.undo_depth(), 2, "a refused nudge commits nothing");
        assert_eq!(st.verb("pip down"), Some(Ok(())));
        assert_eq!(st.verb("pip right"), Some(Ok(())));
        assert_eq!(st.verb("pip up"), Some(Ok(())));
        assert_eq!((st.undo_depth(), st.selected().map(|c| c.xform)), (5, Some(Xform { x: 25, y: 25, s: 250 })));
        assert_eq!(st.verb("pip 0 0"), Some(Ok(())));
        assert_eq!(st.verb("scale+"), Some(Ok(())), "a step at the top-left edge clamps");
        assert_eq!((st.undo_depth(), st.selected().map(|c| c.xform)), (7, Some(Xform { x: 0, y: 0, s: 300 })));
        assert_eq!(st.verb("pip sideways"), Some(Err(EditError::Xform)), "SR879's arms still refuse an unknown word");
        for _ in 0..7 {
            assert!(st.undo());
        }
        assert_eq!(st.timeline, tl);
        st.timeline.tracks[1].clips = tl.tracks[0].clips.clone();
        st.select(1, 0).unwrap();
        assert_eq!(st.verb("pip left"), Some(Err(EditError::Kind)));
    }
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


// ---- STRIAGRADENEXT (LEDGER SR992): the grade over the selection set (named tail block) ----

/// With more than one clip selected, `color b c s` grades every video member in ONE undo step (the same grade again
/// pushes none), audio members are skipped, and a set with no video member is refused `Kind`.
#[test]
fn striagradenext_color_grades_the_set() {
    use timeline_core::{Grade, editor::EditorState};
    let (mut tl, a, _) = base();
    tl.tracks[0].clips.push(Clip::new(a, 50, 60, 30));
    tl.tracks[1].clips = vec![Clip::new(a, 0, 10, 0), Clip::new(a, 20, 30, 20)];
    tl.validate().unwrap();
    let mut st = EditorState::new(tl.clone());
    st.select_many(&[(0, 0), (0, 1)], false);
    assert_eq!(st.set.len(), 2);
    assert_eq!(st.verb("color 100 0 0"), Some(Ok(())));
    let g = Grade { brightness: 100, ..Grade::IDENTITY };
    let v = |st: &EditorState, i: usize| st.timeline.tracks[0].clips[i].grade;
    assert_eq!((v(&st, 0), v(&st, 1), v(&st, 2), st.undo_depth()), (g, g, Grade::IDENTITY, 1));
    assert_eq!(st.verb("color 100 0 0"), Some(Ok(())));
    assert_eq!(st.undo_depth(), 1, "the same grade commits nothing");
    assert_eq!(st.verb("undo"), Some(Ok(())));
    assert_eq!(st.timeline, tl);
    st.select_many(&[(0, 0), (1, 0)], false);
    assert_eq!(st.verb("color 0 200 0"), Some(Ok(())));
    assert_eq!(v(&st, 0), Grade { contrast: 200, ..Grade::IDENTITY });
    assert_eq!(st.timeline.tracks[1].clips[0].grade, Grade::IDENTITY, "the audio member is skipped");
    assert_eq!(st.undo_depth(), 1);
    st.select_many(&[(1, 0), (1, 1)], false);
    let (held, depth) = (st.timeline.clone(), st.undo_depth());
    assert_eq!(st.verb("color 1 0 0"), Some(Err(EditError::Kind)));
    assert_eq!(st.verb("bright+"), Some(Err(EditError::Kind)));
    assert_eq!((&st.timeline, st.undo_depth()), (&held, depth));
}

/// A step word over a set moves every video member by the primary's step (relative, ONE commit); a member pushed past
/// the range refuses the whole edit and the state stays.
#[test]
fn striagradenext_step_moves_each_by_the_delta() {
    use timeline_core::editor::EditorState;
    let (tl, _, _) = base();
    let mut st = EditorState::new(tl.clone());
    st.select(0, 1).unwrap();
    assert_eq!(st.verb("color 300 0 0"), Some(Ok(())));
    st.select_many(&[(0, 0), (0, 1)], false);
    assert_eq!(st.selection, Some((0, 0)));
    assert_eq!(st.verb("bright+"), Some(Ok(())));
    let b = |st: &EditorState, i: usize| st.timeline.tracks[0].clips[i].grade.brightness;
    assert_eq!((b(&st, 0), b(&st, 1), st.undo_depth()), (50, 350, 2));
    assert_eq!(st.verb("sat-"), Some(Ok(())));
    let s = |st: &EditorState, i: usize| st.timeline.tracks[0].clips[i].grade.saturation;
    assert_eq!((s(&st, 0), s(&st, 1), st.undo_depth()), (-50, -50, 3));
    st.select(0, 1).unwrap();
    assert_eq!(st.verb("color 1000 0 0"), Some(Ok(())));
    st.select_many(&[(0, 0), (0, 1)], false);
    let (held, depth) = (st.timeline.clone(), st.undo_depth());
    assert_eq!(st.verb("bright+"), Some(Err(EditError::Grade)), "B would pass 1000: refused whole");
    assert_eq!((&st.timeline, st.undo_depth()), (&held, depth));
    assert_eq!(b(&st, 0), 50, "A did not move either");
}

// ---- STRIAKGAPS (LEDGER SR9005): the Player's g / G keys over the one verb dispatch (named tail block) ----

/// The kernel Player's key road in host terms (vplay.rs `edit_key`): seek to the playhead, select the top video track's
/// clip there (`shown_clip`) or deselect, then the key's word. `g` -> `gap` opens one second on the shown clip's track;
/// `G` -> `close-gaps sync` closes the spans empty on every track; one undo step each; in a gap `gap` is `NoClip` while
/// `close-gaps sync` needs no selection.
#[test]
fn striakgaps_player_keys_route() {
    use timeline_core::editor::EditorState;
    fn key(st: &mut EditorState, at: i64, word: &str) -> Option<Result<(), EditError>> {
        st.seek(at);
        let ph = st.playhead;
        match st.timeline.clip_at(0, ph) {
            Some(i) => st.select(0, i).unwrap(),
            None => st.deselect(),
        }
        st.verb(word)
    }
    let (tl, _, _) = striagapclose_base();
    let mut st = EditorState::new(tl.clone());
    let sec = tl.tb.from_ns(1_000_000_000).max(1);
    assert_eq!(key(&mut st, 7, "gap"), Some(Ok(())));
    let gapped = tl.insert_gap(0, 7, sec).unwrap();
    assert_eq!((st.timeline.clone(), st.undo_depth()), (gapped.clone(), 1));
    assert_eq!(key(&mut st, 7, "close-gaps sync"), Some(Ok(())));
    let synced = gapped.close_gaps_sync().unwrap();
    assert_ne!(synced, gapped);
    assert_eq!((st.timeline.clone(), st.undo_depth()), (synced, 2));
    assert!(st.undo());
    assert!(st.undo());
    assert_eq!(st.timeline, tl);
    assert_eq!(key(&mut st, 2, "gap"), Some(Err(EditError::NoClip)));
    assert_eq!(st.timeline, tl);
    assert_eq!(key(&mut st, 2, "close-gaps sync"), Some(Ok(())));
    assert_eq!(st.timeline, tl.close_gaps_sync().unwrap());
}

// ---- STRIAKUNDOHIST (LEDGER SR9004): the count jump lifted into the editor state (named tail block) ----

/// Three named steps; `undo 2` names the oldest it undid and lands two back; `redo 2` returns exactly and names the newest
/// it redid; `undo 3` lands on the opened timeline. Each step stays its own entry.
#[test]
fn striakundohist_jump_lands_and_returns() {
    use timeline_core::editor::EditorState;
    let (tl, _, _) = base();
    let mut st = EditorState::new(tl.clone());
    st.select(0, 1).unwrap();
    st.seek(15);
    assert_eq!(st.verb("split"), Some(Ok(())));
    let after_split = st.timeline.clone();
    assert_eq!(st.verb("]"), Some(Ok(())));
    assert_eq!(st.verb("mark in"), Some(Ok(())));
    let top = st.timeline.clone();
    assert_eq!(st.jump("undo 2").as_deref(), Some("undo 2: back to before \"]\""));
    assert_eq!((&st.timeline, st.undo_depth(), st.redo_depth()), (&after_split, 1, 2));
    assert_eq!(st.jump("redo 2").as_deref(), Some("redo 2: forward to after \"mark in\""));
    assert_eq!((&st.timeline, st.undo_depth(), st.redo_depth()), (&top, 3, 0));
    assert_eq!(st.jump("undo 3").as_deref(), Some("undo 3: back to before \"split\""));
    assert_eq!((&st.timeline, st.undo_depth()), (&tl, 0));
}

/// A count outside 1..=depth, a non-number, or an empty stack is refused by name and nothing moves.
#[test]
fn striakundohist_jump_refuses_by_name() {
    use timeline_core::editor::EditorState;
    let (tl, _, _) = base();
    let mut st = EditorState::new(tl);
    assert_eq!(st.jump("undo 1").as_deref(), Some("undo: nothing to undo"));
    assert_eq!(st.jump("redo 1").as_deref(), Some("redo: nothing to redo"));
    st.select(0, 1).unwrap();
    st.seek(15);
    assert_eq!(st.verb("split"), Some(Ok(())));
    let before = st.timeline.clone();
    for (w, line) in [("undo 0", "undo: \"0\" is not a count 1..=1"), ("undo 2", "undo: \"2\" is not a count 1..=1"), ("undo x", "undo: \"x\" is not a count 1..=1")] {
        assert_eq!(st.jump(w).as_deref(), Some(line), "{w}");
        assert_eq!((&st.timeline, st.undo_depth(), st.redo_depth()), (&before, 1, 0), "{w}");
    }
}

/// Plain `undo` / `redo` and every other word are not the jump's (`None`) and change nothing.
#[test]
fn striakundohist_jump_ignores_other_words() {
    use timeline_core::editor::EditorState;
    let (tl, _, _) = base();
    let mut st = EditorState::new(tl.clone());
    for w in ["undo", "redo", "history", "split", "undo3", ""] {
        assert_eq!(st.jump(w), None, "{w}");
    }
    assert_eq!((&st.timeline, st.undo_depth()), (&tl, 0));
}

// ---- STRIAKXFORM (LEDGER SR9011): a schedule step's placement, the one rule both faces call (named tail block) ----
mod striakxform {
    use timeline_core::schedule::{Frac, Step, VideoMix};
    use timeline_core::{Clip, Crop, Kind, SizePolicy, SourceId, Timeline, TransitionKind, Xform, xform_frame};

    const W: usize = 8;
    const H: usize = 4;

    /// V1 holds source A, V2 holds source B placed by `xf`; the step is V2's at tick 5 over A (the schedule's shape for a
    /// placed clip: STRIAOPACITY's Dissolve `VideoMix` of the track below at the clip's weight `t`).
    fn two(xf: Xform, t: Frac) -> (Timeline, Step, SourceId) {
        let mut tl = Timeline::new(25);
        let a = tl.add_source("A", 100);
        let b = tl.add_source("B", 100);
        tl.add_track(Kind::Video);
        tl.add_track(Kind::Video);
        tl.tracks[0].clips = vec![Clip::new(a, 0, 20, 0)];
        tl.tracks[1].clips = vec![Clip { xform: xf, ..Clip::new(b, 0, 20, 0) }];
        tl.validate().unwrap();
        let s = Step { kind: Kind::Video, track: Some(1), src: Some(b), src_t: 5, out_t: 5, len: 1, seek: None, vmix: Some(VideoMix { kind: TransitionKind::Dissolve, src: Some(a), src_t: 5, seek: None, t }) };
        (tl, s, b)
    }

    fn frames() -> (Vec<[u8; 4]>, Vec<[u8; 4]>) {
        let under: Vec<[u8; 4]> = (0..W * H).map(|i| [i as u8, 10, 20, 255]).collect();
        let top: Vec<[u8; 4]> = (0..W * H).map(|i| [200, i as u8, 100, 255]).collect();
        (under, top)
    }

    #[test]
    fn striakxform_place_step_is_xform_frame() {
        let (under, top) = frames();
        for t in [Frac { num: 100, den: 100 }, Frac { num: 50, den: 100 }] {
            let (tl, s, _) = two(Xform::PIP, t);
            let want = xform_frame(&top, &under, W, H, Crop::NONE, SizePolicy::Letterbox, Xform::PIP, t);
            assert_eq!(tl.place_step(&s, &under, &top, W, H), Some(want), "the step's frame is the one composite at t {:?}", t);
        }
        let (mut tl, s, _) = two(Xform::PIP, Frac { num: 1, den: 1 });
        let crop = Crop { l: 250, t: 0, r: 0, b: 0 };
        tl.tracks[1].clips[0].crop = crop;
        let want = xform_frame(&top, &under, W, H, crop, SizePolicy::Letterbox, Xform::PIP, Frac { num: 1, den: 1 });
        assert_eq!(tl.place_step(&s, &under, &top, W, H), Some(want), "the clip's crop goes inside the rect");
    }

    #[test]
    fn striakxform_place_step_none_unplaced() {
        let (under, top) = frames();
        let (tl, s, _) = two(Xform::NONE, Frac { num: 1, den: 1 });
        assert_eq!(tl.place_step(&s, &under, &top, W, H), None, "an unplaced clip mixes as before");
        let (tl, s, _) = two(Xform::PIP, Frac { num: 1, den: 1 });
        assert_eq!(tl.place_step(&Step { vmix: None, ..s }, &under, &top, W, H), None, "no mix: no placement");
        assert_eq!(tl.place_step(&Step { track: None, ..s }, &under, &top, W, H), None, "a gap: no placement");
        assert_eq!(tl.place_step(&Step { src: None, ..s }, &under, &top, W, H), None, "no source: no placement");
        assert_eq!(tl.place_step(&Step { out_t: 30, ..s }, &under, &top, W, H), None, "past the clip: no placement");
    }

    #[test]
    fn striakxform_place_step_adopt_is_letterbox() {
        let (under, top) = frames();
        let (tl, s, b) = two(Xform::PIP, Frac { num: 1, den: 1 });
        let letterbox = tl.place_step(&s, &under, &top, W, H);
        let adopt = tl.set_size_policy(b, SizePolicy::Adopt).unwrap();
        assert_eq!(adopt.place_step(&s, &under, &top, W, H), letterbox, "Adopt places as Letterbox (the host's reel_fit(false))");
        let stretch = tl.set_size_policy(b, SizePolicy::Stretch).unwrap();
        let want = xform_frame(&top, &under, W, H, Crop::NONE, SizePolicy::Stretch, Xform::PIP, Frac { num: 1, den: 1 });
        assert_eq!(stretch.place_step(&s, &under, &top, W, H), Some(want), "the source's own policy");
    }
}

// ---- STRIAKCROP (LEDGER SR9010): the lifted reel fit and the crop pass on packed pixels (named tail block) ----
mod striakcrop {
    use super::base;
    use timeline_core::{Crop, SizePolicy, SourceId, crop_frame};

    #[test]
    fn striakcrop_reel_fit_alone_and_in_mix() {
        let (tl, a, b) = base();
        assert_eq!(tl.reel_fit(SourceId(999), true), SizePolicy::Letterbox, "an absent source places as Letterbox");
        assert_eq!(tl.reel_fit(a, true), SizePolicy::Letterbox);
        let tl = tl.set_size_policy(a, SizePolicy::Fill).unwrap().set_size_policy(b, SizePolicy::Adopt).unwrap();
        assert_eq!((tl.reel_fit(a, true), tl.reel_fit(a, false)), (SizePolicy::Fill, SizePolicy::Fill));
        assert_eq!(tl.reel_fit(b, true), SizePolicy::Adopt, "Adopt alone keeps its own size");
        assert_eq!(tl.reel_fit(b, false), SizePolicy::Letterbox, "Adopt mixes as Letterbox");
    }

    #[test]
    fn striakcrop_crop_frame_packed_equals_rgba() {
        // the kernel Player crops 0RGB u32, the host RGBA quads: one pass, the same pixels
        let (w, h) = (6usize, 4usize);
        let rgba: Vec<[u8; 4]> = (0..w * h).map(|i| [i as u8, (i * 3) as u8, (i * 7) as u8, 255]).collect();
        let packed: Vec<u32> = rgba.iter().map(|p| ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | p[2] as u32).collect();
        let crop = Crop { l: 200, t: 250, r: 100, b: 0 };
        for fit in [SizePolicy::Letterbox, SizePolicy::Fill, SizePolicy::Stretch, SizePolicy::Adopt] {
            let a = crop_frame(&rgba, w, h, crop, fit, [0, 0, 0, 255]);
            let b = crop_frame(&packed, w, h, crop, fit, 0u32);
            let a: Vec<u32> = a.iter().map(|p| ((p[0] as u32) << 16) | ((p[1] as u32) << 8) | p[2] as u32).collect();
            assert_eq!(a, b, "{fit:?}");
        }
    }
}

// ---- STRIAKTRACKADD (LEDGER SR9007): the removal note's one text, and the kernel Player's two key words (named tail block) ----
mod striaktrackadd {
    use timeline_core::editor::EditorState;
    use timeline_core::{Clip, EditError, Kind, Timeline};

    /// V1 A[0,20)@0 B[0,20)@20; A1 the same; A2 A[0,10)@0; V2 (added last, empty) is made by the tests.
    fn three() -> Timeline {
        let mut tl = Timeline::new(25);
        let (a, b) = (tl.add_source("A", 100), tl.add_source("B", 100));
        let (v1, a1, a2) = (tl.add_track(Kind::Video), tl.add_track(Kind::Audio), tl.add_track(Kind::Audio));
        tl.tracks[v1].clips = vec![Clip::new(a, 0, 20, 0), Clip::new(b, 0, 20, 20)];
        tl.tracks[a1].clips = tl.tracks[v1].clips.clone();
        tl.tracks[a2].clips = vec![Clip::new(a, 0, 10, 0)];
        tl
    }

    /// The host face's exact text (SR856's pins): the channel word, the count, `clip` / `clips`; `None` past the end.
    #[test]
    fn striaktrackadd_track_removed_note() {
        let tl = three();
        assert_eq!(tl.track_removed_note(0).as_deref(), Some("track-: removed V (2 clips)"));
        assert_eq!(tl.track_removed_note(1).as_deref(), Some("track-: removed A (2 clips)"));
        assert_eq!(tl.track_removed_note(2).as_deref(), Some("track-: removed A2 (1 clip)"));
        assert_eq!(tl.track_removed_note(3), None);
        let tl = tl.insert_track(Kind::Video).unwrap();
        assert_eq!(tl.track_removed_note(1).as_deref(), Some("track-: removed V2 (0 clips)"));
    }

    /// The Player's words as its keys send them (`T` -> `track+ video`, `R` -> bare `track-` after the shown clip is
    /// selected): one undo step each; the note read BEFORE the verb names what went; nothing shown -> `NoTrack`, no step.
    #[test]
    fn striaktrackadd_player_words_through_verb() {
        let tl = three();
        let mut st = EditorState::new(tl.clone());
        assert_eq!(st.verb("track+ video"), Some(Ok(())));
        assert_eq!((st.undo_depth(), st.timeline.tracks.len(), st.timeline.tracks[1].kind), (1, 4, Kind::Video), "V2 after V1");
        assert!(st.timeline.tracks[1].clips.is_empty());
        st.select(0, 1).unwrap();
        let gone = st.track_minus_target("track-").and_then(|r| r.ok()).and_then(|t| st.timeline.track_removed_note(t));
        assert_eq!(gone.as_deref(), Some("track-: removed V (2 clips)"));
        assert_eq!(st.verb("track-"), Some(Ok(())));
        assert_eq!((st.undo_depth(), st.timeline.tracks.len(), st.selection), (2, 3, None));
        assert!(st.undo());
        assert_eq!((st.timeline.tracks.len(), st.timeline.tracks[0].clips.len()), (4, 2));
        assert!(st.undo());
        assert_eq!(st.timeline, tl);
        st.deselect();
        let depth = st.undo_depth();
        assert_eq!(st.track_minus_target("track-"), Some(Err(EditError::NoTrack)));
        assert_eq!(st.verb("track-"), Some(Err(EditError::NoTrack)));
        assert_eq!(st.undo_depth(), depth, "a refusal commits nothing");
    }
}

// ---- STRIAKTC (LEDGER SR9018): the playhead readout text, lifted from nle_face (named tail block) ----

/// `view::tc_readout` reads the timeline's rate: a 1/n base at n (SR872's values), an NTSC base at its real rate (SR964's
/// drop-frame), any other base at the fallback 25; a negative tick reads 0; the unit words are the witness's.
#[test]
fn striaktc_tc_readout_reads_the_timeline() {
    use timeline_core::Timebase;
    use timeline_core::view::{TcUnit, tc_readout, tc_text};
    let mut tl = Timeline::new(25);
    assert_eq!(tc_readout(&tl, 312, TcUnit::Timecode), "00:00:12:12");
    assert_eq!(tc_readout(&tl, 312, TcUnit::Frames), "312");
    assert_eq!(tc_readout(&tl, 312, TcUnit::Seconds), "12.48 s");
    assert_eq!(tc_readout(&tl, -3, TcUnit::Timecode), "00:00:00:00");
    assert_eq!(tc_readout(&tl, -3, TcUnit::Frames), "0");
    assert_eq!(tc_readout(&Timeline::new(30), 45, TcUnit::Timecode), "00:00:01:15");
    assert_eq!(tc_readout(&Timeline::new(30), 45, TcUnit::Seconds), "1.50 s");
    tl.tb = Timebase { num: 1001, den: 30000 };
    assert_eq!(tc_readout(&tl, 1800, TcUnit::Timecode), "00:01:00;02");
    assert_eq!(tc_readout(&tl, 1800, TcUnit::Frames), "1800");
    assert_eq!(tc_readout(&tl, 1800, TcUnit::Seconds), "60.06 s");
    tl.tb = Timebase { num: 2, den: 50 };
    for u in [TcUnit::Timecode, TcUnit::Frames, TcUnit::Seconds] {
        assert_eq!(tc_readout(&tl, 312, u), tc_text(312, 25, u), "a non-1/n base reads at 25");
    }
    assert_eq!([TcUnit::Timecode, TcUnit::Frames, TcUnit::Seconds].map(TcUnit::word), ["timecode", "frames", "seconds"]);
}

// ---- STRIAUNDOTHREE (LEDGER SR9027): the redo entries' words (named tail block) ----

/// `redo_history` lists the redo entries next first with the count that reaches each; an unnamed step is None; a redo
/// moves the entry back to the undo side; a commit drops them with the redo stack.
#[test]
fn striaundothree_redo_history() {
    use timeline_core::editor::{EditorState, HISTORY_N};
    let (tl, _, _) = base();
    let mut st = EditorState::new(tl);
    assert!(st.redo_history(HISTORY_N).is_empty());
    st.select(0, 1).unwrap();
    st.seek(15);
    assert_eq!(st.verb("split"), Some(Ok(())));
    assert_eq!(st.verb("]"), Some(Ok(())));
    assert_eq!(st.verb("mark in"), Some(Ok(())));
    assert!(st.undo() && st.undo());
    assert_eq!(st.redo_history(HISTORY_N), vec![(1, Some("]")), (2, Some("mark in"))]);
    assert_eq!(st.redo_history(1), vec![(1, Some("]"))]);
    assert!(st.redo());
    assert_eq!(st.redo_history(HISTORY_N), vec![(1, Some("mark in"))]);
    assert_eq!(st.history(1), vec![(2, Some("]"))]);
    assert!(st.undo());
    assert_eq!(st.verb("mark out"), Some(Ok(())));
    assert!(st.redo_history(HISTORY_N).is_empty(), "a commit drops the redo words");
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

// ---- STRIAGAPSTHREE (LEDGER SR9028): the all-tracks close validated once, keep a faded gap open (named tail block) ----

/// V1 A[0,10)@5 | B[0,10)@15 under a 3-tick dissolve at 15; A1 C[0,2)@0 | A[94,100)@4 | B[0,10)@10. Closing V1 alone moves
/// the dissolve to 10, where A1's A|B cut coincidentally pairs onto A's zero handle (`Handle`): the half-closed value refuses.
/// Closing A1 too moves that cut to 8 and the pairing is gone: the finished value is valid, so `close-gaps all` stands.
#[test]
fn striagapsthree_all_order_final_value_validates_once() {
    let mut k = Timeline::new(25);
    let (a, b, c) = (k.add_source("A", 100), k.add_source("B", 60), k.add_source("C", 50));
    k.add_track(Kind::Video);
    k.add_track(Kind::Audio);
    k.tracks[0].clips = vec![Clip::new(a, 0, 10, 5), Clip::new(b, 0, 10, 15)];
    k.tracks[1].clips = vec![Clip::new(c, 0, 2, 0), Clip::new(a, 94, 100, 4), Clip::new(b, 0, 10, 10)];
    let k = k.add_transition(0, 15, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    assert_eq!(k.validate(), Ok(()));
    assert_eq!(k.close_gaps(0), Err(EditError::Handle));
    let all = k.close_gaps_all().unwrap();
    let ats: Vec<i64> = all.tracks[1].clips.iter().map(|x| x.at).collect();
    assert_eq!(ats, vec![0, 2, 8]);
    assert_eq!(all.tracks[0].transitions[0].at, 10);
    assert_eq!((all.audio_crossfade(1, 1), all.audio_crossfade(1, 2)), (None, None));
    assert_eq!((all.gap_count_all(), all.validate()), (0, Ok(())));
    println!("striagapsthree: A1 after close-gaps all {ats:?}");
}

/// A finished value that refuses falls back to SR929's chain, which names the refusing track as before; a plain timeline's
/// all-tracks close is still the per-track close chained.
#[test]
fn striagapsthree_all_order_keeps_naming() {
    let mut k = Timeline::new(25);
    let (a, b) = (k.add_source("A", 100), k.add_source("B", 60));
    k.add_track(Kind::Video);
    k.add_track(Kind::Audio);
    k.tracks[0].clips = vec![Clip::new(a, 0, 10, 0), Clip::new(b, 0, 10, 10)];
    k.tracks[1].clips = vec![Clip::new(a, 90, 100, 0), Clip::new(b, 0, 10, 15)];
    let h = k.add_transition(0, 10, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    assert_eq!(h.close_gaps_all(), Err((1, EditError::Handle)));
    k.tracks[0].clips = vec![Clip::new(a, 0, 10, 5), Clip::new(b, 0, 10, 15)];
    k.tracks[1].clips = vec![Clip::new(a, 0, 15, 0), Clip::new(b, 0, 10, 15)];
    let p = k.add_transition(0, 15, timeline_core::TransitionKind::Dissolve, 3).unwrap();
    assert_eq!(p.close_gaps_all(), Err((1, EditError::Transition)));
    let (tl, _, _) = striagapclose_base();
    assert_eq!(tl.close_gaps_all().unwrap(), tl.close_gaps(0).unwrap().close_gaps(1).unwrap());
}

/// SR909's base: `close_gaps_keep` leaves the 10..20 gap open with its to-black and from-black and closes the plain 40..45
/// gap (the trailing to-black moves with its clip); one gap is left. With no black fade it is the plain close.
#[test]
fn striagapsthree_keep_leaves_a_faded_gap_open() {
    let k = striagapsblack_base();
    let c = k.close_gaps_keep(0).unwrap();
    let ats: Vec<i64> = c.tracks[0].clips.iter().map(|x| x.at).collect();
    assert_eq!(ats, vec![0, 20, 30, 40]);
    let xs: Vec<(i64, bool, bool)> = c.tracks[0].transitions.iter().map(|x| (x.at, x.from_black, x.to_black)).collect();
    assert_eq!(xs, vec![(10, false, true), (20, true, false), (30, false, false), (50, false, true)]);
    assert_eq!((c.gap_count(0), c.validate()), (1, Ok(())));
    assert_eq!(c.close_gaps_keep(0).unwrap(), c);
    let (tl, _, _) = striagapclose_base();
    assert_eq!(tl.close_gaps_keep(0).unwrap(), tl.close_gaps(0).unwrap());
    assert_eq!(tl.close_gaps_keep(9), Err(EditError::NoTrack));
    println!("striagapsthree: keep V1 {ats:?} transitions {xs:?}");
}

/// `close-gaps keep` through THE verb dispatch: nothing selected is `NoClip`; one commit; a second is an equal value
/// (nothing); undo restores; a third word is not this verb.
#[test]
fn striagapsthree_editor_keep_one_undo_step() {
    use timeline_core::editor::EditorState;
    let k = striagapsblack_base();
    let mut st = EditorState::new(k.clone());
    assert_eq!(st.verb("close-gaps keep"), Some(Err(EditError::NoClip)));
    st.select(0, 1).unwrap();
    assert_eq!(st.verb("close-gaps keep"), Some(Ok(())));
    assert_eq!((st.timeline.clone(), st.undo_depth()), (k.close_gaps_keep(0).unwrap(), 1));
    assert_eq!(st.verb("close-gaps keep"), Some(Ok(())));
    assert_eq!(st.undo_depth(), 1);
    assert!(st.undo());
    assert_eq!(st.timeline, k);
    assert_eq!(st.verb("close-gaps keep x"), None);
}

// ---- STRIAZOOMFITTHREE (LEDGER SR9040): the squeeze, a fit below one px per tick (named tail block) ----

/// 35 ticks fit in 20 px at zoom 1 squeeze 2 (the end inside the window); `zoom+` halves the squeeze, `zoom-` at the
/// floor doubles it, `zoom0` resets it; the ruler visits every tick at squeeze 1 and whole-second strides squeezed; never
/// an undo step.
#[test]
fn striazoomfitthree_fractional_fit() {
    use timeline_core::editor::{EditorState, ZOOM_DEFAULT, ZOOM_MIN};
    use timeline_core::view::Layout;
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(a, 13, 23, 0), Clip::new(b, 0, 10, 10), Clip::new(a, 23, 33, 25)];
    let mut st = EditorState::new(tl.clone());
    let l = Layout::of(320, 160, &st);
    assert_eq!(l.squeeze, 1);
    assert_eq!(l.ruler_ticks(10, 25).collect::<Vec<_>>(), (l.t0..=10).collect::<Vec<_>>(), "squeeze 1: every tick");
    assert_eq!(st.fit(20, false), Ok(0));
    assert_eq!(st.scale(), (ZOOM_MIN, 2), "35 ticks in 20 px: 2 ticks per px");
    let l = Layout::of(20, 160, &st).anchor(Some((0, st.playhead)), st.playhead);
    assert_eq!((l.squeeze, l.t0, l.page()), (2, 0, 40));
    assert!(l.x_of(tl.end()) <= l.w as i64, "the end inside the window");
    assert_eq!((l.x_of(34), l.tick_at(17.0)), (17, 34));
    assert_eq!(st.verb("zoom+"), Some(Ok(())));
    assert_eq!(st.scale(), (1, 1), "zoom+ halves the squeeze first");
    assert_eq!(st.verb("zoom-"), Some(Ok(())));
    assert_eq!(st.verb("zoom-"), Some(Ok(())));
    assert_eq!(st.scale(), (1, 4), "zoom- at the floor doubles the squeeze");
    let l = Layout::of(20, 160, &st);
    assert_eq!(l.ruler_ticks(200, 25).collect::<Vec<_>>(), vec![0, 50, 100, 150, 200], "squeezed: seconds, 8 px apart");
    assert_eq!(st.verb("zoom0"), Some(Ok(())));
    assert_eq!(st.scale(), (ZOOM_DEFAULT, 1));
    assert_eq!(st.fit(320, false), Ok(0));
    assert_eq!(st.scale(), (9, 1), "a fit that fits has no squeeze");
    assert_eq!((st.undo_depth(), &st.timeline), (0, &tl), "a view change, not an edit");
}

// ---- STRIATRACKSTHREE (LEDGER SR9038): the opt-in track header column through THE layout (named tail block) ----
mod striatracksthree {
    use super::base;
    use timeline_core::editor::EditorState;
    use timeline_core::view::{self, Canvas, HEADER_W, Hit, Layout, Order};

    /// Off is the layout and the pixels as before: `with_headers(false)` changes nothing, and `Layout::of` sets no column.
    #[test]
    fn striatracksthree_headers_off_is_identity() {
        let (tl, _, _) = base();
        let st = EditorState::new(tl);
        let l = Layout::of(320, 160, &st);
        assert_eq!((l.with_headers(false, &st), l.left), (l, 0));
        let px = view::paint_in(&st, l, Order::Rgba, None, None);
        assert_eq!(view::paint_in(&st, l.with_headers(false, &st), Order::Rgba, None, None), px);
        assert_ne!(view::paint_in(&st, l.with_headers(true, &st), Order::Rgba, None, None), px, "on paints the column");
        let narrow = Layout::of(2 * HEADER_W, 160, &st);
        assert_eq!(narrow.with_headers(true, &st), narrow, "too narrow for a column: unchanged");
    }

    /// On: tick t0 sits at the column's right edge, the two maps stay inverse, a press in the column is a header (the
    /// row) and no hit, and the clip right of it is the clip it was.
    #[test]
    fn striatracksthree_header_inset_geometry() {
        let (tl, _, _) = base();
        let st = EditorState::new(tl);
        let l = Layout::of(320, 160, &st).with_headers(true, &st);
        assert_eq!((l.left, l.x_of(l.t0)), (HEADER_W, HEADER_W as i64));
        for t in l.t0..l.t0 + 20 {
            assert_eq!(l.tick_at(l.x_of(t) as f64), t, "tick {t}");
        }
        let y1 = l.row_y(1) as f64 + 1.0;
        assert_eq!((l.header_at(5.0, y1), l.hit(&st.timeline, 5.0, y1)), (Some(1), Hit::None));
        assert_eq!(l.header_at(5.0, l.ruler_y as f64 + 1.0), None, "the ruler's corner is no header");
        assert_eq!(l.header_at(HEADER_W as f64, y1), None, "right of the column");
        let (x, y) = (l.x_of(2) as f64 + 0.5, l.row_y(0) as f64 + 1.0);
        assert_eq!(l.hit(&st.timeline, x, y), Hit::Clip(0, 0, 2));
        assert_eq!(Layout::of(320, 160, &st).header_at(5.0, y1), None, "no column, no header");
    }

    /// The label hook as a recorder: every `(text, x, clip)` call.
    struct Rec(Vec<(String, f32, (i32, i32))>);
    impl view::Text for Rec {
        fn text(&mut self, _c: &mut Canvas, s: &str, x: f32, _baseline: f32, clip: (i32, i32)) {
            self.0.push((s.to_string(), x, clip));
        }
    }

    /// Each row's channel word goes through the Text hook at x 4, clipped to the column; off, no call has that clip.
    #[test]
    fn striatracksthree_header_labels() {
        let (tl, _, _) = base();
        let st = EditorState::new(tl);
        let col = (0, HEADER_W as i32 - 1);
        let words = |l: Layout| {
            let mut rec = Rec(Vec::new());
            view::paint_in(&st, l, Order::Rgba, Some(&mut rec as &mut dyn view::Text), None);
            rec.0.into_iter().filter(|r| r.2 == col).map(|r| (r.0, r.1)).collect::<Vec<_>>()
        };
        let l = Layout::of(320, 160, &st);
        assert_eq!(words(l.with_headers(true, &st)), vec![("V".to_string(), 4.0), ("A".to_string(), 4.0)]);
        assert_eq!(words(l), vec![]);
    }
}

// ---- STRIAEXPORTPRESETSTHREE (LEDGER SR9021): THE export preset table lives in the shared core (named tail block) ----
#[test]
fn striaexportpresetsthree_table_lives_in_the_core() {
    use timeline_core::export_presets::{AAC_KBPS, PRESETS, preset_named, preset_words, scaled_size};
    assert_eq!(preset_words(), "web|archive|proxy");
    assert_eq!(PRESETS.iter().map(|p| p.words()).collect::<Vec<_>>(), ["web: h264 + aac crf=23 aq=10 scenecut=40", "archive: png + wav", "proxy: h264 + aac:64 crf=32 scale=1/2"]);
    assert_eq!(preset_named("gif"), None);
    assert_eq!(AAC_KBPS, 128);
    let kbps = |n: &str| preset_named(n).unwrap().aac_kbps();
    assert_eq!((kbps("web"), kbps("proxy"), kbps("archive")), (Some(128), Some(64), None));
    assert_eq!(scaled_size(1280, 720, 2), (640, 360));
    assert_eq!(scaled_size(5, 5, 2), (2, 2));
    assert_eq!(scaled_size(1280, 720, 1), (1280, 720));
}

// ---- STRIAXFORMKEYS (LEDGER SR9049): per-clip placement keyframes, their words and their EDL lines (named tail block) ----
mod striaxformkeys {
    use super::base;
    use timeline_core::edl::{self, EdlError};
    use timeline_core::editor::EditorState;
    use timeline_core::{Clip, EditError, XKeys, Xform};

    #[test]
    fn striaxformkeys_keys_interpolate_through_the_clamp() {
        let half = Xform { x: 500, y: 500, s: 500 };
        assert_eq!(XKeys::NONE.at(0), None, "no points: the static placement");
        let k = XKeys::NONE.with(10, half).unwrap().with(0, Xform::NONE).unwrap();
        assert_eq!(k.iter().map(|p| p.0).collect::<Vec<_>>(), vec![0, 10], "points sorted by tick");
        assert_eq!(k.at(-3), Some(Xform::NONE), "before the first point: held");
        assert_eq!(k.at(5), Some(Xform { x: 250, y: 250, s: 750 }), "linear between points");
        assert_eq!(k.at(99), Some(half), "after the last point: held");
        let mut full = XKeys::NONE;
        for t in 0..XKeys::MAX as i64 {
            full = full.with(t * 10, Xform::PIP).unwrap();
        }
        assert_eq!(full.len(), XKeys::MAX);
        assert_eq!(full.with(5, Xform::PIP), None, "a ninth point is refused");
        assert_eq!(full.with(10, Xform::NONE).map(|f| (f.len(), f.get(10))), Some((XKeys::MAX, Some(Xform::NONE))), "a point at a held tick is replaced");
        assert_eq!(full.without(5), None, "no point there");
        assert_eq!(full.without(10).map(|f| (f.len(), f.get(10), f.get(20))), Some((XKeys::MAX - 1, None, Some(Xform::PIP))));
        let (tl, a, _) = base();
        let k2 = XKeys::NONE.with(20, Xform::NONE).unwrap().with(30, half).unwrap();
        let mut c = Clip::new(a, 20, 40, 100);
        assert!(!c.placed(), "a new clip is not placed");
        c.xkeys = k2;
        assert!(c.placed());
        assert_eq!((c.src_tick(105), c.xform_at(105)), (25, Xform { x: 250, y: 250, s: 750 }), "points on source ticks");
        let t2 = tl.set_xkeys(0, 1, k2).unwrap();
        assert_eq!((t2.xform_at(0, 15), t2.xform_at(0, 10)), (Xform { x: 250, y: 250, s: 750 }, Xform::NONE), "the one lookup follows the points");
        assert_eq!(tl.set_xkeys(1, 0, k2), Err(EditError::Kind));
        assert_eq!(tl.set_xkeys(0, 9, k2), Err(EditError::NoClip));
    }

    #[test]
    fn striaxformkeys_key_words_one_undo_step_each() {
        let (tl, _, _) = base();
        let mut st = EditorState::new(tl.clone());
        assert_eq!(st.verb("pip key"), Some(Err(EditError::NoClip)));
        st.select(0, 1).unwrap();
        st.playhead = 10;
        assert_eq!(st.verb("pip key"), Some(Ok(())));
        assert_eq!((st.undo_depth(), st.selected().map(|c| c.xkeys.get(20))), (1, Some(Some(Xform::NONE))));
        assert_eq!(st.verb("pip tl"), Some(Ok(())), "a keyed clip's placement word edits the point at the playhead");
        assert_eq!((st.undo_depth(), st.selected().map(|c| (c.xform, c.xkeys.get(20)))), (2, Some((Xform::NONE, Xform::corner("tl")))));
        st.playhead = 19;
        assert_eq!(st.verb("pip br"), Some(Ok(())));
        assert_eq!((st.undo_depth(), st.selected().map(|c| c.xkeys.len())), (3, Some(2)));
        st.playhead = 14;
        assert_eq!(st.selected().map(|c| c.xform_at(14)), Some(Xform { x: 336, y: 336, s: 250 }), "linear between the points");
        assert_eq!(st.timeline.xform_at(0, 14), Xform { x: 336, y: 336, s: 250 });
        assert_eq!(st.verb("pip unkey"), Some(Err(EditError::XKey)), "no point at the playhead");
        assert_eq!(st.undo_depth(), 3, "a refused word commits nothing");
        st.playhead = 19;
        assert_eq!(st.verb("pip br"), Some(Ok(())), "the placement shown there again");
        assert_eq!(st.undo_depth(), 3, "the shown placement unchanged commits nothing");
        assert_eq!(st.verb("pip unkey"), Some(Ok(())));
        assert_eq!((st.undo_depth(), st.selected().map(|c| c.xkeys.len())), (4, Some(1)));
        st.playhead = 30;
        assert_eq!(st.verb("pip key"), Some(Err(EditError::XKey)), "the playhead off the clip");
        assert_eq!(st.verb("pip tl"), Some(Err(EditError::XKey)));
        assert_eq!(st.verb("pip unkey all"), Some(Ok(())));
        assert_eq!((st.undo_depth(), st.selected().map(|c| c.xkeys.is_empty())), (5, Some(true)));
        assert_eq!(st.verb("pip unkey all"), Some(Err(EditError::XKey)), "no points to clear");
        assert_eq!(st.undo_depth(), 5);
        for _ in 0..5 {
            assert!(st.undo());
        }
        assert_eq!(st.timeline, tl);
        st.timeline.tracks[1].clips = tl.tracks[0].clips.clone();
        st.select(1, 0).unwrap();
        st.playhead = 0;
        assert_eq!(st.verb("pip key"), Some(Err(EditError::Kind)));
    }

    #[test]
    fn striaxformkeys_edl_xkey_round_trip_and_refusals() {
        let (tl, _, _) = base();
        let plain = edl::write(&tl, "T").unwrap();
        assert!(!plain.contains("XKEY"), "no points: the EDL as before");
        let k = XKeys::NONE.with(20, Xform::PIP).unwrap().with(29, Xform { x: 0, y: 0, s: 500 }).unwrap();
        let t2 = tl.set_xkeys(0, 1, k).unwrap();
        let text = edl::write(&t2, "T").unwrap();
        assert!(text.contains("* UNA XKEY 20 725 725 250\n* UNA XKEY 29 0 0 500\n"), "{text}");
        let back = edl::parse(&text, 25).unwrap();
        assert_eq!(back.tracks[0].clips[1].xkeys, k);
        assert_eq!(edl::write(&back, "T").unwrap(), text);
        for bad in ["* UNA XKEY 20 0 0 500", "* UNA XKEY 29 600 0 500", "* UNA XKEY 29 0 0", "* UNA XKEY -1 0 0 500"] {
            let t = text.replace("* UNA XKEY 29 0 0 500", bad);
            assert!(matches!(edl::parse(&t, 25), Err(EdlError::Syntax { .. })), "{bad}");
        }
        let lead = plain.replacen("\n001 ", "\n* UNA XKEY 0 0 0 500\n001 ", 1);
        assert!(lead != plain && matches!(edl::parse(&lead, 25), Err(EdlError::Syntax { .. })), "a point with no event above");
    }
}

// ---- STRIAOPACITYKEYS (LEDGER SR9048): the opacity point words through the one commit (named tail block) ----

/// `opacity-key <pct>` sets a point at the playhead on the selected clip under it as ONE undo step; the same point again
/// commits nothing; a bare word pins the clip's own value; `opacity-unkey` removes it; each refusal is named.
#[test]
fn striaopacitykeys_words_are_one_undo_step() {
    use timeline_core::OpacityKeys;
    use timeline_core::editor::EditorState;
    let (tl, _, _) = base();
    let mut st = EditorState::new(tl.clone());
    assert_eq!(st.verb("opacity-key 50"), Some(Err(EditError::NoClip)), "nothing selected");
    st.select(0, 0).unwrap();
    st.seek(5);
    let d0 = st.undo_depth();
    assert_eq!(st.verb("opacity-key 50"), Some(Ok(())));
    assert_eq!(st.undo_depth(), d0 + 1);
    assert_eq!(st.timeline.tracks[0].clips[0].opacity_keys, OpacityKeys::NONE.with(5, 50).unwrap());
    assert_eq!(st.verb("opacity-key"), Some(Ok(())), "bare: the clip's own value at the playhead");
    assert_eq!(st.undo_depth(), d0 + 1, "the same point commits nothing");
    st.seek(0);
    assert_eq!(st.verb("opacity-key 0"), Some(Ok(())));
    assert_eq!(st.timeline.tracks[0].clips[0].opacity_at(2), 20, "linear between the points");
    assert_eq!(st.timeline.tracks[0].clips[0].opacity_at(9), 50, "held after the last");
    for bad in ["opacity-key 101", "opacity-key half", "opacity-key 5 6", "opacity-unkey now"] {
        assert_eq!(st.verb(bad), Some(Err(EditError::Opacity)), "{bad}");
    }
    st.seek(15);
    assert_eq!(st.verb("opacity-key 10"), Some(Err(EditError::OpacityKey)), "the selected clip is not under the playhead");
    st.seek(3);
    assert_eq!(st.verb("opacity-unkey"), Some(Err(EditError::OpacityKey)), "no point at the playhead");
    st.seek(0);
    let d1 = st.undo_depth();
    assert_eq!(st.verb("opacity-unkey"), Some(Ok(())));
    assert_eq!(st.undo_depth(), d1 + 1);
    assert_eq!(st.timeline.tracks[0].clips[0].opacity_keys, OpacityKeys::NONE.with(5, 50).unwrap());
    for t in 0..10 {
        st.seek(t);
        assert_eq!(st.verb("opacity-key 30").map(|r| r.is_ok()), Some(t < timeline_core::OPACITY_KEYS_MAX as i64 || t == 5), "tick {t}");
    }
    while st.undo_depth() > 0 {
        assert!(st.undo());
    }
    assert_eq!(st.timeline, tl);
}
