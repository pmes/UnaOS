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

// ---- STRIAHISTORYRIPPLE (LEDGER SR9072): a set edit's undo entry names its clip count (named tail block) ----

/// A set word over a set of more than one clip names its entry `<word> (<n> clips)` (`nudge+`, `lift`); a word that acts
/// on the primary only (`split`) and a set of one (`delete`) keep the bare word; undo / redo carry the counted names.
#[test]
fn striahistoryripple_set_words_carry_count() {
    use timeline_core::editor::{EditorState, HISTORY_N, SET_WORDS};
    let (mut tl, a, _) = base();
    tl.tracks[0].clips.push(Clip::new(a, 40, 50, 30));
    tl.validate().unwrap();

    let mut st = EditorState::new(tl.clone());
    st.select(0, 1).unwrap();
    st.select_add(0, 2).unwrap();
    assert_eq!(st.counted_word("nudge+"), "nudge+ (2 clips)");
    assert_eq!(st.counted_word("split"), "split");
    assert!(SET_WORDS.contains(&"drag move"));
    assert_eq!(st.verb("nudge+"), Some(Ok(())));
    assert_eq!(st.set.len(), 2, "the set follows its clips");
    st.seek(15);
    assert_eq!(st.verb("split"), Some(Ok(())));
    assert_eq!(st.history_line(HISTORY_N), "history: 2 split | 1 nudge+ (2 clips)");
    assert!(st.undo());
    assert!(st.undo());
    assert_eq!(st.redo_history(HISTORY_N), vec![(1, Some("nudge+ (2 clips)")), (2, Some("split"))]);
    assert!(st.redo());
    assert!(st.redo());
    assert_eq!(st.history_line(HISTORY_N), "history: 2 split | 1 nudge+ (2 clips)");

    let mut st = EditorState::new(tl);
    st.select(0, 1).unwrap();
    st.select_add(0, 2).unwrap();
    assert_eq!(st.verb("lift"), Some(Ok(())));
    st.select(0, 0).unwrap();
    assert_eq!(st.counted_word("delete"), "delete");
    assert_eq!(st.verb("delete"), Some(Ok(())));
    assert_eq!(st.history_line(HISTORY_N), "history: 2 delete | 1 lift (2 clips)");
}
