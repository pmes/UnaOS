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

