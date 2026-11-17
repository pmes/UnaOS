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
