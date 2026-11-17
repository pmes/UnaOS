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
        for bad in ["crop", "crop 1 2 3", "crop 1 2 3 4 5", "crop a 0 0 0", "crop -1 0 0 0", "crop 600 0 400 0"] {
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
