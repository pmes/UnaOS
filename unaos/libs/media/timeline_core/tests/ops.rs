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
