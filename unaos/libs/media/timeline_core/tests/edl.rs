// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! NLE1 (SR194) M1: the CMX 3600 EDL — writer and reader agree byte for byte; a real editor's shape reads; each
//! refusal is named.

use timeline_core::edl::{self, EdlError};
use timeline_core::{Clip, EditError, Kind, Timeline};

fn cut() -> Timeline {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Video);
    tl.add_track(Kind::Video);
    tl.add_track(Kind::Audio);
    tl.tracks[0].clips = vec![Clip::new(a, 13, 23, 0), Clip::new(a, 23, 33, 25)];
    tl.tracks[1].clips = vec![Clip::new(b, 0, 10, 10)];
    tl.tracks[2].clips = vec![Clip::new(a, 13, 23, 0), Clip::new(b, 0, 10, 10)];
    tl
}

const CUT: &str = "TITLE: CUT ONE
FCM: NON-DROP FRAME
* UNA FPS 25
* UNA SOURCE A 00:00:02:00
* UNA SOURCE B 00:00:02:00

001  A        V     C        00:00:00:13 00:00:00:23 00:00:00:00 00:00:00:10
002  A        A     C        00:00:00:13 00:00:00:23 00:00:00:00 00:00:00:10
003  B        V2    C        00:00:00:00 00:00:00:10 00:00:00:10 00:00:00:20
004  B        A     C        00:00:00:00 00:00:00:10 00:00:00:10 00:00:00:20
005  A        V     C        00:00:00:23 00:00:01:08 00:00:01:00 00:00:01:10
";

#[test]
fn writer_output_is_the_known_text_and_reads_back() {
    let tl = cut();
    let text = edl::write(&tl, "CUT ONE").unwrap();
    assert_eq!(text, CUT);
    let back = edl::parse(&text, 30).unwrap();
    assert_eq!(back, tl);
    assert_eq!(edl::write(&back, "CUT ONE").unwrap(), text);
}

#[test]
fn a_real_editors_edl_reads() {
    // The shape an editor exports: a record start at 01:00:00:00, FROM CLIP NAME comments, a black (BL) event, no
    // UNA comments (the caller's frame rate applies; reel lengths are their furthest out points).
    let text = "TITLE: SEQUENCE 01
FCM: NON-DROP FRAME

001  TAPE01   V     C        00:00:10:00 00:00:15:00 01:00:00:00 01:00:05:00
* FROM CLIP NAME: interview.mov
002  TAPE02   V     C        00:01:00:12 00:01:02:12 01:00:05:00 01:00:07:00
* FROM CLIP NAME: broll.mov
003  TAPE01   A     C        00:00:10:00 00:00:15:00 01:00:00:00 01:00:05:00
004  BL       V     C        00:00:00:00 00:00:01:00 01:00:07:00 01:00:08:00
";
    let tl = edl::parse(text, 25).unwrap();
    assert_eq!(tl.fps(), Some(25));
    assert_eq!(tl.sources.len(), 2);
    let t1 = tl.source_by_reel("TAPE01").unwrap();
    let t2 = tl.source_by_reel("TAPE02").unwrap();
    assert_eq!(t1.duration, 15 * 25);
    assert_eq!(t2.duration, 62 * 25 + 12);
    assert_eq!(tl.tracks.len(), 2);
    assert_eq!(tl.tracks[0].kind, Kind::Video);
    assert_eq!(tl.tracks[0].clips, vec![Clip::new(t1.id, 250, 375, 90_000), Clip::new(t2.id, 1512, 1562, 90_125)]);
    assert_eq!(tl.tracks[1].clips, vec![Clip::new(t1.id, 250, 375, 90_000)]);
    assert_eq!(tl.end(), 90_175);
}

#[test]
fn refusals_are_named() {
    let ev = |s: &str| format!("TITLE: X\nFCM: NON-DROP FRAME\n{s}\n");
    assert_eq!(edl::parse("TITLE: X\nFCM: DROP FRAME\n* UNA FPS 25\n", 30), Err(EdlError::DropFrame { line: 2 })); // STRIATCTHREE (SR9030): drop-frame off 30 / 60 (a bare DROP FRAME reads: file tail)
    assert_eq!(edl::parse("FCM: 24P\n", 24), Err(EdlError::Fcm { line: 1 }));
    assert_eq!(
        edl::parse(&ev("001  A        V     C        00:00:00;00 00:00:01;00 00:00:00;00 00:00:01;00"), 30),
        Err(EdlError::DropFrame { line: 3 })
    );
    assert_eq!(
        edl::parse(&ev("002  A        V     D    025 00:00:00:00 00:00:01:00 00:00:00:00 00:00:01:00"), 25),
        Err(EdlError::Transition { line: 3 })
    );
    assert_eq!(
        edl::parse(&ev("001  A        AA    C        00:00:00:00 00:00:01:00 00:00:00:00 00:00:01:00"), 25),
        Err(EdlError::Channel { line: 3 })
    );
    assert_eq!(
        edl::parse(&ev("001  A        V     C        00:00:00:00 00:00:01:00 00:00:00:00 00:00:02:00"), 25),
        Err(EdlError::Duration { line: 3 })
    );
    assert_eq!(
        edl::parse(&ev("001  A        V     C        00:00:00:25 00:00:01:00 00:00:00:00 00:00:00:00"), 25),
        Err(EdlError::Timecode { line: 3 })
    );
    assert_eq!(edl::parse(&ev("M2   A       050.0                00:00:00:00"), 25), Err(EdlError::Syntax { line: 3 }));
    assert_eq!(
        edl::parse(
            &ev("001  A        V     C        00:00:00:00 00:00:01:00 00:00:00:00 00:00:01:00\n002  A        V     C        00:00:00:00 00:00:01:00 00:00:00:20 00:00:01:20"),
            25
        ),
        Err(EdlError::Edit(EditError::Overlap))
    );
    assert_eq!(edl::parse("* UNA FPS 0\n", 25), Err(EdlError::Fps));
    // the writer refuses what CMX cannot carry
    let mut tl = Timeline::new(25);
    tl.add_source("NINECHARS", 10);
    assert_eq!(edl::write(&tl, "X"), Err(EdlError::Reel));
    let mut tl = Timeline::new(25);
    tl.tb.num = 1001;
    tl.tb.den = 30000;
    assert_eq!(edl::write(&tl, "X"), Err(EdlError::Fps));
}

#[test]
fn timecodes() {
    assert_eq!(edl::timecode(0, 25), "00:00:00:00");
    assert_eq!(edl::timecode(90_125, 25), "01:00:05:00");
    assert_eq!(edl::timecode(29, 30), "00:00:00:29");
}

// ── NLESIZEPOLICY (LEDGER SR394): a reel's size policy is a `* UNA SIZE` line of the one codec ──────────────────────────

#[test]
fn nlesizepolicy_edl_round_trip_carries_the_reels_policy() {
    use timeline_core::{SizePolicy, SourceId};
    // Absent = Letterbox: the known text is unchanged (no SIZE line), so every EDL written before reads and writes as it did.
    let tl = cut();
    assert!(tl.sources.iter().all(|s| s.size_policy == SizePolicy::Letterbox));
    assert_eq!(edl::write(&tl, "CUT ONE").unwrap(), CUT);
    assert!(edl::parse(CUT, 25).unwrap().sources.iter().all(|s| s.size_policy == SizePolicy::Letterbox));
    // Each non-default policy writes one line right after its reel's SOURCE line and reads back.
    for p in [SizePolicy::Fill, SizePolicy::Stretch, SizePolicy::Adopt] {
        let tl = cut().set_size_policy(SourceId(1), p).unwrap();
        let text = edl::write(&tl, "CUT ONE").unwrap();
        let want = CUT.replace("* UNA SOURCE B 00:00:02:00\n", &format!("* UNA SOURCE B 00:00:02:00\n* UNA SIZE B {}\n", p.name()));
        assert_eq!(text, want);
        let back = edl::parse(&text, 25).unwrap();
        assert_eq!(back, tl);
        assert_eq!((back.sources[0].size_policy, back.sources[1].size_policy), (SizePolicy::Letterbox, p));
        assert_eq!(edl::write(&back, "CUT ONE").unwrap(), text, "write(parse(write)) byte-identical");
    }
    // An explicit `letterbox` reads (and writes back as nothing); a SIZE line before an implicitly cited reel applies; case is ignored.
    let explicit = CUT.replace("* UNA SOURCE A 00:00:02:00\n", "* UNA SOURCE A 00:00:02:00\n* UNA SIZE A letterbox\n");
    assert_eq!(edl::write(&edl::parse(&explicit, 25).unwrap(), "CUT ONE").unwrap(), CUT);
    let implicit = "* UNA FPS 25\n* UNA SIZE C FILL\n001  C        V     C        00:00:00:00 00:00:00:10 00:00:00:00 00:00:00:10\n";
    let t = edl::parse(implicit, 25).unwrap();
    assert_eq!((t.sources[0].reel.as_str(), t.sources[0].size_policy), ("C", SizePolicy::Fill));
    // Refused by name: an unknown policy, a reel nothing cites, a second line for one reel, a malformed line.
    let with = |line: &str| CUT.replace("* UNA SOURCE B 00:00:02:00\n", &format!("* UNA SOURCE B 00:00:02:00\n{line}\n"));
    assert_eq!(edl::parse(&with("* UNA SIZE B crop"), 25), Err(EdlError::Syntax { line: 6 }));
    assert_eq!(edl::parse(&with("* UNA SIZE Z fill"), 25), Err(EdlError::Syntax { line: 6 }));
    assert_eq!(edl::parse(&with("* UNA SIZE B fill\n* UNA SIZE B stretch"), 25), Err(EdlError::Syntax { line: 7 }));
    assert_eq!(edl::parse(&with("* UNA SIZE B"), 25), Err(EdlError::Syntax { line: 6 }));
    // The property's edit refuses a source the timeline does not hold.
    assert_eq!(cut().set_size_policy(SourceId(9), SizePolicy::Fill), Err(EditError::NoSource));
}

#[test]
fn nlesizepolicy_place_is_the_one_geometry() {
    use timeline_core::SizePolicy;
    use yuv_core::scale::Rect;
    let r = |w, h, x, y| Rect { w, h, x, y };
    // SR379's switch: 1280x720 into the reel's 768x512.
    let lb = SizePolicy::Letterbox.place(1280, 720, 768, 512);
    assert_eq!((lb.src, lb.dst), (r(1280, 720, 0, 0), r(768, 432, 0, 40)));
    let fill = SizePolicy::Fill.place(1280, 720, 768, 512);
    assert_eq!((fill.src, fill.dst), (r(1080, 720, 100, 0), r(768, 512, 0, 0)), "the box's 3:2 centred in the 16:9 frame");
    let st = SizePolicy::Stretch.place(1280, 720, 768, 512);
    assert_eq!((st.src, st.dst), (r(1280, 720, 0, 0), r(768, 512, 0, 0)));
    assert_eq!(SizePolicy::Adopt.place(1280, 720, 768, 512), lb, "a box adopts as Letterbox");
    // A tall frame: Fill crops rows.
    let tall = SizePolicy::Fill.place(100, 400, 200, 100);
    assert_eq!((tall.src, tall.dst), (r(100, 50, 0, 175), r(200, 100, 0, 0)));
    assert_eq!(["letterbox", "fill", "stretch", "adopt"].map(|n| SizePolicy::from_name(n).map(|p| p.name())), [Some("letterbox"), Some("fill"), Some("stretch"), Some("adopt")]);
    assert_eq!(SizePolicy::from_name("crop"), None);
}

#[test]
fn nlesizepolicy_blit_policy_samples_by_the_place() {
    use timeline_core::SizePolicy;
    use timeline_core::view::{Canvas, Order, Still};
    // A 4x2 frame whose pixel (x, y) is [x, y, 7, 255].
    let s = Still { width: 4, height: 2, rgba: (0..2u8).flat_map(|y| (0..4u8).flat_map(move |x| [x, y, 7, 255])).collect() };
    let paint = |fit: SizePolicy, order: Order| {
        let mut c = Canvas::new(2, 2, order);
        c.rect(0, 0, 2, 2, [9, 9, 9, 9]);
        c.blit_policy(&s, 0, 0, 2, 2, fit);
        c.px.chunks(4).map(|p| [p[0], p[1], p[2], p[3]]).collect::<Vec<_>>()
    };
    // Letterbox: 2x1 at row 0 (y (2-1)/2 = 0), columns 0 and 2 of row 0; row 1 untouched.
    assert_eq!(paint(SizePolicy::Letterbox, Order::Rgba), vec![[0, 0, 7, 255], [2, 0, 7, 255], [9, 9, 9, 9], [9, 9, 9, 9]]);
    // Fill: the centred 2x2 (columns 1..3), every pixel.
    assert_eq!(paint(SizePolicy::Fill, Order::Rgba), vec![[1, 0, 7, 255], [2, 0, 7, 255], [1, 1, 7, 255], [2, 1, 7, 255]]);
    // Stretch: columns 0 and 2 of both rows.
    assert_eq!(paint(SizePolicy::Stretch, Order::Rgba), vec![[0, 0, 7, 255], [2, 0, 7, 255], [0, 1, 7, 255], [2, 1, 7, 255]]);
    // The canvas's byte order applies to every sample.
    assert_eq!(paint(SizePolicy::Stretch, Order::Bgra)[3], [7, 1, 2, 255]);
    // blit_fit is the Letterbox.
    let mut c = Canvas::new(2, 2, Order::Rgba);
    c.rect(0, 0, 2, 2, [9, 9, 9, 9]);
    c.blit_fit(&s, 0, 0, 2, 2);
    assert_eq!(c.px.chunks(4).map(|p| [p[0], p[1], p[2], p[3]]).collect::<Vec<_>>(), paint(SizePolicy::Letterbox, Order::Rgba));
}

// ---- NLEWIPES (LEDGER SR679): the `* UNA EASE` line (named tail block) ----

/// V1: A[0,20)@0, B[0,20)@20, a 5-tick dissolve on the cut eased `e`.
fn nlewipes_eased(e: timeline_core::Ease) -> Timeline {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Video);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 20, 0), Clip::new(b, 0, 20, 20)];
    let mut tl = tl.add_transition(0, 20, timeline_core::TransitionKind::Dissolve, 5).unwrap();
    tl.tracks[0].transitions[0].ease = e;
    tl
}

const NLEWIPES_EASED: &str = "TITLE: EASE
FCM: NON-DROP FRAME
* UNA FPS 25
* UNA SOURCE A 00:00:02:00
* UNA SOURCE B 00:00:02:00

001  A        V     C        00:00:00:00 00:00:00:20 00:00:00:00 00:00:00:20
002  A        V     C        00:00:00:20 00:00:00:20 00:00:00:20 00:00:00:20
002  B        V     D    005 00:00:00:00 00:00:00:20 00:00:00:20 00:00:01:15
* UNA EASE inout
";

#[test]
fn nlewipes_the_ease_line_round_trips_byte_exact() {
    use timeline_core::Ease;
    let tl = nlewipes_eased(Ease::InOut);
    let text = edl::write(&tl, "EASE").unwrap();
    assert_eq!(text, NLEWIPES_EASED);
    let back = edl::parse(&text, 25).unwrap();
    assert_eq!((back.tracks[0].transitions[0].ease, &back), (Ease::InOut, &tl));
    assert_eq!(edl::write(&back, "EASE").unwrap(), text);
    for e in [Ease::In, Ease::Out] {
        let t = edl::write(&nlewipes_eased(e), "EASE").unwrap();
        assert!(t.ends_with(&format!("* UNA EASE {}\n", e.name())), "{t}");
        assert_eq!(edl::parse(&t, 25).unwrap(), nlewipes_eased(e));
    }
    // a linear transition writes no EASE line: the text is the one before NLEWIPES
    assert_eq!(edl::write(&nlewipes_eased(Ease::Linear), "EASE").unwrap(), NLEWIPES_EASED.replace("* UNA EASE inout\n", ""));
    // a to-black carries it after its BL line
    let mut tb = Timeline::new(25);
    let a = tb.add_source("A", 50);
    tb.add_track(Kind::Video);
    tb.tracks[0].clips = vec![Clip::new(a, 0, 20, 0)];
    let mut tb = tb.add_transition_to_black(0, 20, timeline_core::TransitionKind::Dissolve, 5).unwrap();
    tb.tracks[0].transitions[0].ease = Ease::Out;
    let t = edl::write(&tb, "TB").unwrap();
    assert!(t.ends_with("002  BL       V     D    005 00:00:00:00 00:00:00:05 00:00:00:20 00:00:01:00\n* UNA EASE out\n"), "{t}");
    assert_eq!(edl::parse(&t, 25).unwrap(), tb);
}

#[test]
fn nlewipes_ease_refusals_are_named() {
    // the EASE line is line 10; an unknown word, the word `linear`, a second EASE, and one under no transition line
    let swap = |to: &str| NLEWIPES_EASED.replace("* UNA EASE inout\n", to);
    assert_eq!(edl::parse(&swap("* UNA EASE bounce\n"), 25), Err(EdlError::Syntax { line: 10 }));
    assert_eq!(edl::parse(&swap("* UNA EASE linear\n"), 25), Err(EdlError::Syntax { line: 10 }));
    assert_eq!(edl::parse(&swap("* UNA EASE in\n* UNA EASE out\n"), 25), Err(EdlError::Syntax { line: 11 }));
    let orphan = NLEWIPES_EASED.replace("001  A        V     C        00:00:00:00 00:00:00:20 00:00:00:00 00:00:00:20\n", "001  A        V     C        00:00:00:00 00:00:00:20 00:00:00:00 00:00:00:20\n* UNA EASE in\n");
    assert_eq!(edl::parse(&orphan, 25), Err(EdlError::Syntax { line: 8 }));
}

// ---- NLEXFADEPOWER (SR678): the A-only `D` event and `* UNA XFADE EQUAL`.

/// A1: A[10,30)@0, B[0,40)@20 and an explicit 8-tick crossfade on the cut at 20.
fn axf(curve: timeline_core::Curve) -> Timeline {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    let b = tl.add_source("B", 50);
    tl.add_track(Kind::Audio);
    tl.tracks[0].clips = vec![Clip::new(a, 10, 30, 0), Clip::new(b, 0, 40, 20)];
    tl.add_audio_xfade(0, 20, 8, curve).unwrap()
}

const AXF: &str = "TITLE: AX
FCM: NON-DROP FRAME
* UNA FPS 25
* UNA SOURCE A 00:00:02:00
* UNA SOURCE B 00:00:02:00

001  A        A     C        00:00:00:10 00:00:01:05 00:00:00:00 00:00:00:20
002  A        A     C        00:00:01:05 00:00:01:05 00:00:00:20 00:00:00:20
002  B        A     D    008 00:00:00:00 00:00:01:15 00:00:00:20 00:00:02:10
* UNA XFADE EQUAL
";

#[test]
fn nlexfadepower_the_a_only_d_event_round_trips_byte_exact() {
    use timeline_core::{AudioXfade, Curve};
    let tl = axf(Curve::EqualPower);
    let text = edl::write(&tl, "AX").unwrap();
    assert_eq!(text, AXF);
    let back = edl::parse(&text, 30).unwrap();
    assert_eq!(back.audio_xfades, vec![AudioXfade { track: 0, at: 20, dur: 8, curve: Curve::EqualPower }]);
    assert_eq!(back, tl);
    assert_eq!(edl::write(&back, "AX").unwrap(), text);
    // linear: the same two-line event, no XFADE line (absent = linear)
    let lin = edl::write(&axf(Curve::Linear), "AX").unwrap();
    assert_eq!(lin, AXF.replace("* UNA XFADE EQUAL\n", ""));
    assert_eq!(edl::parse(&lin, 25).unwrap(), axf(Curve::Linear));
}

#[test]
fn nlexfadepower_audio_transitions_and_xfade_lines_are_refused_by_name() {
    // a D on an audio channel with no cut there (the outgoing line names an out point no clip ends on)
    let nocut = AXF.replace("002  A        A     C        00:00:01:05 00:00:01:05", "002  A        A     C        00:00:01:04 00:00:01:04");
    assert_eq!(edl::parse(&nocut, 25), Err(EdlError::Transition { line: 9 }));
    // a wipe or BL on an audio channel
    assert_eq!(edl::parse(&AXF.replace("D    008", "W001 008"), 25), Err(EdlError::Transition { line: 9 }));
    let bl = AXF.replace("002  A        A     C        00:00:01:05 00:00:01:05", "002  BL       A     C        00:00:00:00 00:00:00:00");
    assert_eq!(edl::parse(&bl, 25), Err(EdlError::Transition { line: 9 }));
    // an XFADE line with no D line above it, twice, with another word, under a VIDEO dissolve
    let first = AXF.replace("00:00:00:20\n002", "00:00:00:20\n* UNA XFADE EQUAL\n002");
    assert_eq!(edl::parse(&first, 25), Err(EdlError::Syntax { line: 8 }));
    assert_eq!(edl::parse(&format!("{AXF}* UNA XFADE EQUAL\n"), 25), Err(EdlError::Syntax { line: 11 }));
    assert_eq!(edl::parse(&AXF.replace("XFADE EQUAL", "XFADE LINEAR"), 25), Err(EdlError::Syntax { line: 10 }));
    let video = AXF.replace(" A     C", " V     C").replace(" A     D", " V     D");
    assert_eq!(edl::parse(&video, 25), Err(EdlError::Syntax { line: 10 }));
    assert!(edl::parse(&video.replace("* UNA XFADE EQUAL\n", ""), 25).is_ok());
}

// ---- NLE5DRANGESAVE (LEDGER SR723): the `* UNA MARK` line (named tail block) ----

/// CUT with `m` as the header's last line (line 6).
fn nle5drangesave_at(m: &str) -> String {
    CUT.replace("00:00:02:00\n\n", &format!("00:00:02:00\n{m}\n\n"))
}

#[test]
fn nle5drangesave_the_mark_line_round_trips_byte_exact() {
    let text = nle5drangesave_at("* UNA MARK 5 15");
    let tl = edl::parse(&text, 25).unwrap();
    assert_eq!(tl.marks, (Some(5), Some(15)));
    let mut want = cut();
    want.marks = (Some(5), Some(15));
    assert_eq!(tl, want);
    assert_eq!(edl::write(&tl, "CUT ONE").unwrap(), text);
    // one mark alone: the other is `-`
    let half = nle5drangesave_at("* UNA MARK - 15");
    let tl = edl::parse(&half, 25).unwrap();
    assert_eq!(tl.marks, (None, Some(15)));
    assert_eq!(edl::write(&tl, "CUT ONE").unwrap(), half);
    let tl = edl::parse(&nle5drangesave_at("* UNA MARK 5 -"), 25).unwrap();
    assert_eq!(tl.marks, (Some(5), None));
    // no marks: no MARK line, so every pinned EDL writes as before
    assert_eq!(cut().marks, (None, None));
    assert_eq!(edl::write(&cut(), "CUT ONE").unwrap(), CUT);
}

#[test]
fn nle5drangesave_mark_refusals_are_named() {
    for bad in ["* UNA MARK 15 5", "* UNA MARK 5 5", "* UNA MARK x 15", "* UNA MARK - -", "* UNA MARK -1 15", "* UNA MARK 5", "* UNA MARK 5 15 20"] {
        assert_eq!(edl::parse(&nle5drangesave_at(bad), 25), Err(EdlError::Syntax { line: 6 }), "{bad}");
    }
    assert_eq!(edl::parse(&nle5drangesave_at("* UNA MARK 5 15\n* UNA MARK 5 15"), 25), Err(EdlError::Syntax { line: 7 }), "a second MARK line");
    assert_eq!(cut().set_marks((Some(15), Some(5))), Err(EditError::Marks));
    assert_eq!(cut().set_marks((Some(5), Some(5))), Err(EditError::Marks));
    assert_eq!(cut().set_marks((Some(-1), None)), Err(EditError::Negative));
    assert_eq!(cut().set_marks((None, Some(3))).map(|t| t.marks), Ok((None, Some(3))));
}

#[test]
fn nle5drangesave_the_editors_mark_words_commit_and_write() {
    use timeline_core::editor::EditorState;
    let mut st = EditorState::open(CUT, 25).unwrap();
    st.seek(5);
    assert_eq!(st.verb("mark in"), Some(Ok(())));
    st.seek(15);
    assert_eq!(st.verb("mark out"), Some(Ok(())));
    assert_eq!((st.timeline.marks, st.undo_depth()), ((Some(5), Some(15)), 2));
    assert_eq!(edl::write(&st.timeline, "CUT ONE").unwrap(), nle5drangesave_at("* UNA MARK 5 15"));
    assert_eq!(st.verb("mark out"), Some(Ok(())), "the same marks again");
    assert_eq!(st.undo_depth(), 2, "commit nothing");
    st.seek(2);
    assert_eq!(st.verb("mark out"), Some(Err(EditError::Marks)));
    assert_eq!(st.timeline.marks, (Some(5), Some(15)), "a refused mark leaves the marks");
    assert!(st.undo());
    assert_eq!(st.timeline.marks, (Some(5), None), "undo takes the out mark back");
    assert_eq!(st.verb("mark clear"), Some(Ok(())));
    assert_eq!(st.timeline.marks, (None, None));
    assert_eq!(st.verb("mark sideways"), Some(Ok(())), "STRIAMARKERS (SR835): a marker named sideways"); assert!(st.undo());
    assert_eq!(edl::write(&st.timeline, "CUT ONE").unwrap(), CUT);
}

// ---- STRIAOPACITY (LEDGER SR868): the `* UNA ALPHA` line (named tail block) ----

/// A non-opaque clip writes ONE `* UNA ALPHA <pct>` line under its event and reads back equal; the opaque cut writes the
/// known text unchanged (`CUT`); each refusal is named.
#[test]
fn striaopacity_alpha_line_round_trips_and_refusals_are_named() {
    let tl = cut();
    assert!(!edl::write(&tl, "CUT ONE").unwrap().contains("ALPHA"), "an opaque timeline writes no ALPHA line");
    let tl = tl.set_opacity(&[(1, 0)], 50).unwrap();
    let text = edl::write(&tl, "CUT ONE").unwrap();
    assert_eq!(text.matches("* UNA ALPHA ").count(), 1);
    let ev = text.lines().position(|l| l.contains(" V2 ")).unwrap();
    assert_eq!(text.lines().nth(ev + 1), Some("* UNA ALPHA 50"), "under its own event");
    let back = edl::parse(&text, 30).unwrap();
    assert_eq!(back, tl);
    assert_eq!(back.tracks[1].clips[0].opacity, 50);
    assert_eq!(edl::write(&back, "CUT ONE").unwrap(), text);
    let head = "TITLE: X\nFCM: NON-DROP FRAME\n";
    let v = "001  A        V     C        00:00:00:00 00:00:01:00 00:00:00:00 00:00:01:00\n";
    let a = "001  A        A     C        00:00:00:00 00:00:01:00 00:00:00:00 00:00:01:00\n";
    assert_eq!(edl::parse(&format!("{head}* UNA ALPHA 50\n"), 25), Err(EdlError::Syntax { line: 3 }), "no event above");
    assert_eq!(edl::parse(&format!("{head}{v}* UNA ALPHA 101\n"), 25), Err(EdlError::Syntax { line: 4 }));
    assert_eq!(edl::parse(&format!("{head}{v}* UNA ALPHA half\n"), 25), Err(EdlError::Syntax { line: 4 }));
    assert_eq!(edl::parse(&format!("{head}{a}* UNA ALPHA 50\n"), 25), Err(EdlError::Edit(EditError::Opacity)), "audio");
    assert_eq!(edl::parse(&format!("{head}{v}* UNA ALPHA 0\n"), 25).unwrap().tracks[0].clips[0].opacity, 0);
}

// ---- STRIACLIPLINK (LEDGER SR863): `* UNA GROUP` is the event's link group (named tail block) ----
fn striacliplink_tl() -> Timeline {
    let mut tl = Timeline::new(25);
    let a = tl.add_source("A", 50);
    tl.add_track(Kind::Video);
    tl.add_track(Kind::Audio);
    tl.tracks[0].clips = vec![Clip::new(a, 0, 10, 0), Clip::new(a, 10, 20, 10)];
    tl.tracks[1].clips = vec![Clip::new(a, 0, 10, 0), Clip::new(a, 10, 20, 10)];
    tl
}

#[test]
fn striacliplink_edl_group_round_trip() {
    let tl = striacliplink_tl().set_group(&[(0, 0), (1, 0)], 7).unwrap();
    let text = edl::write(&tl, "LINK").unwrap();
    assert_eq!(text.matches("* UNA GROUP 7\n").count(), 2, "{text}");
    assert!(text.contains("00:00:00:10\n* UNA GROUP 7\n002"), "the line follows its event: {text}");
    assert_eq!(edl::parse(&text, 25).unwrap(), tl);
}

#[test]
fn striacliplink_edl_group_zero_writes_nothing() {
    let text = edl::write(&striacliplink_tl(), "LINK").unwrap();
    assert!(!text.contains("GROUP"), "{text}");
    assert_eq!(edl::parse(&text, 25).unwrap(), striacliplink_tl());
}

#[test]
fn striacliplink_edl_group_refusals() {
    let text = edl::write(&striacliplink_tl(), "LINK").unwrap();
    let head = text.find("001 ").unwrap();
    let early = format!("{}* UNA GROUP 3\n{}", &text[..head], &text[head..]);
    let line = early[..early.find("* UNA GROUP").unwrap()].lines().count() + 1;
    assert_eq!(edl::parse(&early, 25), Err(EdlError::Syntax { line }), "no event above");
    for bad in ["0", "x", "1 2", "-1"] {
        let t = text.replacen("\n002", &format!("\n* UNA GROUP {bad}\n002"), 1);
        let line = t[..t.find("* UNA GROUP").unwrap()].lines().count() + 1;
        assert_eq!(edl::parse(&t, 25), Err(EdlError::Syntax { line }), "{bad}");
    }
}

// ---- STRIACOLORLIFT (LEDGER SR869): the `* UNA COLOR` line (named tail block) ----

/// `cut()` with the V2 clip (B) graded: the line sits under its event, the text reads back to the same timeline, and the
/// ungraded `cut()` still writes the pinned `CUT` byte for byte.
#[test]
fn striacolorlift_color_line_round_trips() {
    let g = timeline_core::Grade { brightness: 100, contrast: -200, saturation: 300 };
    let tl = cut().set_grade(1, 0, g).unwrap();
    let text = edl::write(&tl, "CUT ONE").unwrap();
    let want = CUT.replace(
        "003  B        V2    C        00:00:00:00 00:00:00:10 00:00:00:10 00:00:00:20\n",
        "003  B        V2    C        00:00:00:00 00:00:00:10 00:00:00:10 00:00:00:20\n* UNA COLOR 100 -200 300\n",
    );
    assert_eq!(text, want);
    assert_eq!(edl::parse(&text, 25).unwrap(), tl);
    assert_eq!(edl::write(&cut(), "CUT ONE").unwrap(), CUT);
}

/// Refused by name: no event above, misshapen words (Syntax with the line); out of range Edit(Grade); an audio event Edit(Kind).
#[test]
fn striacolorlift_color_refusals_are_named() {
    let head = "TITLE: C\nFCM: NON-DROP FRAME\n* UNA FPS 25\n* UNA SOURCE A 00:00:02:00\n";
    let v = "001  A        V     C        00:00:00:13 00:00:00:23 00:00:00:00 00:00:00:10\n";
    let a = "001  A        A     C        00:00:00:13 00:00:00:23 00:00:00:00 00:00:00:10\n";
    assert_eq!(edl::parse(&format!("{head}* UNA COLOR 1 2 3\n{v}"), 25), Err(EdlError::Syntax { line: 5 }));
    assert_eq!(edl::parse(&format!("{head}{v}* UNA COLOR 1 2\n"), 25), Err(EdlError::Syntax { line: 6 }));
    assert_eq!(edl::parse(&format!("{head}{v}* UNA COLOR 1 x 3\n"), 25), Err(EdlError::Syntax { line: 6 }));
    assert_eq!(edl::parse(&format!("{head}{v}* UNA COLOR 1001 0 0\n"), 25), Err(EdlError::Edit(EditError::Grade)));
    assert_eq!(edl::parse(&format!("{head}{a}* UNA COLOR 1 2 3\n"), 25), Err(EdlError::Edit(EditError::Kind)));
    let ok = edl::parse(&format!("{head}{v}* UNA COLOR 0 0 -1000\n"), 25).unwrap();
    assert_eq!(ok.tracks[0].clips[0].grade.saturation, -1000);
}

// ---- STRIATCMORE (LEDGER SR964): drop-frame timecode text (named tail block) ----

/// SMPTE drop-frame: two numbers skipped each minute but every tenth (four at 60), labels strictly increasing, other
/// nominals non-drop, a negative tick reads as zero.
#[test]
fn striatcmore_timecode_drop_counts_smpte() {
    use timeline_core::edl::{timecode, timecode_drop};
    for (t, s) in [(0, "00:00:00;00"), (1799, "00:00:59;29"), (1800, "00:01:00;02"), (3597, "00:01:59;29"), (3598, "00:02:00;02"), (17981, "00:09:59;29"), (17982, "00:10:00;00"), (17983, "00:10:00;01"), (19782, "00:11:00;02"), (107892, "01:00:00;00"), (-5, "00:00:00;00")] {
        assert_eq!(timecode_drop(t, 30), s, "30 @ {t}");
    }
    assert_eq!(timecode_drop(3599, 60), "00:00:59;59");
    assert_eq!(timecode_drop(3600, 60), "00:01:00;04");
    assert_eq!(timecode_drop(35964, 60), "00:10:00;00");
    let mut last = timecode_drop(0, 30);
    for t in 1..18000 {
        let s = timecode_drop(t, 30);
        assert!(s > last, "{t}: {s} after {last}");
        last = s;
    }
    for (t, nom) in [(312, 25), (45, 24), (1800, 50)] {
        assert_eq!(timecode_drop(t, nom), timecode(t, nom), "non-drop nominal {nom}");
    }
}

// ---- STRIAKMARKERS (LEDGER SR9001): the kernel Player's marker keys' calls, host side (named tail block) ----

/// The Player's `m` / `M` bytes (vplay.rs `EDIT_KEYS`) name exactly `mark` / `unmark`; `edit_key` runs them through
/// `EditorState::verb` at the playhead, saves through `edl::write`, and `view_band` paints `view::paint_in` at
/// `Role::Tracks`. This is those same calls: one commit per word, the marker saved and re-read, the band showing it on the
/// ruler only, and `unmark` writing the EDL back byte for byte. The Player itself is unexercised (NLE-MARKERS-KERNEL-WITNESS).
#[test]
fn striakmarkers_the_players_key_words_mark_save_and_paint_the_band() {
    use timeline_core::editor::EditorState;
    use timeline_core::view::{self, Layout, Order, Role};
    let mut st = EditorState::open(CUT, 25).unwrap();
    st.seek(5);
    let w = 320u32;
    let h = view::RULER_H + st.timeline.tracks.len() as u32 * view::ROW_H;
    let plain = view::paint_in(&st, Layout::of_role(Role::Tracks, w, h, &st), Order::Rgba, None, None);
    assert_eq!(st.verb("mark"), Some(Ok(())));
    assert_eq!(st.undo_depth(), 1, "one commit");
    assert_eq!(st.timeline.markers, vec![timeline_core::Marker { at: 5, name: "M1".into() }]);
    assert_eq!(st.verb("mark"), Some(Err(EditError::Marker)), "a taken tick");
    assert_eq!(st.undo_depth(), 1, "a refused mark commits nothing");
    let text = edl::write(&st.timeline, "CUT ONE").unwrap();
    assert!(text.contains("* UNA MARKER 00:00:00:05 \"M1\"\n"), "{text}");
    assert_eq!(edl::parse(&text, 25).unwrap().markers, st.timeline.markers, "the saved marker re-reads");
    let l = Layout::of_role(Role::Tracks, w, h, &st);
    let marked = view::paint_in(&st, l, Order::Rgba, None, None);
    assert_eq!((plain.len(), marked.len()), ((w * h * 4) as usize, (w * h * 4) as usize));
    let mut inside = 0usize;
    for y in 0..h as usize {
        for x in 0..w as usize {
            let i = (y * w as usize + x) * 4;
            if plain[i..i + 4] != marked[i..i + 4] {
                assert!((l.ruler_y as usize..l.tracks_y as usize).contains(&y), "a marker paints only on the ruler (y={y})");
                inside += 1;
            }
        }
    }
    assert!(inside > 0, "the band shows the marker");
    let (x, y) = ((l.x_of(5) - 1) as usize, l.ruler_y as usize); // the playhead line owns the apex column itself
    assert_eq!(marked[(y * w as usize + x) * 4..(y * w as usize + x) * 4 + 4], view::MARKER);
    assert_eq!(st.verb("unmark"), Some(Ok(())));
    assert_eq!(st.undo_depth(), 2, "one commit");
    assert!(st.timeline.markers.is_empty());
    assert_eq!(st.verb("unmark"), Some(Err(EditError::Marker)), "nothing to remove");
    assert_eq!(edl::write(&st.timeline, "CUT ONE").unwrap(), CUT, "unmark writes the EDL back byte for byte");
    println!("striakmarkers: mark -> M1 @5 saved, painted on the ruler ({inside} px); unmark -> CUT byte for byte");
}

// ---- STRIATCTHREE (LEDGER SR9030): drop-frame documents (named tail block) ----

/// A 30000/1001 timeline writes `FCM: DROP FRAME`, `* UNA FPS 30` and `;` labels (a clip past the first minute), and reads
/// back equal.
#[test]
fn striatcthree_drop_frame_round_trips() {
    let mut tl = Timeline::new(30);
    tl.tb = timeline_core::Timebase { num: 1001, den: 30000 };
    let a = tl.add_source("A", 4000);
    let v = tl.add_track(Kind::Video);
    tl.tracks[v].clips.push(Clip::new(a, 100, 2000, 0));
    let text = edl::write(&tl, "DF").expect("an NTSC 30 timeline writes");
    assert!(text.contains("\nFCM: DROP FRAME\n* UNA FPS 30\n"), "{text}");
    assert!(text.contains("* UNA SOURCE A 00:02:13;14\n"), "{text}");
    assert!(text.contains("A        V     C        00:00:03;10 00:01:06;22 00:00:00;00 00:01:03;12\n"), "{text}");
    assert_eq!(edl::parse(&text, 25), Ok(tl));
    println!("striatcthree: 30000/1001 writes and reads back drop-frame");
}

/// A foreign drop-frame EDL (no `* UNA FPS`) reads at 29.97: `00:01:00;02` is tick 1800, the base 30000/1001; a bare
/// `FCM: DROP FRAME` reads an empty 30000/1001 timeline (tests/edl.rs `refusals_are_named` held the old refusal).
#[test]
fn striatcthree_foreign_drop_frame_reads() {
    let ntsc = timeline_core::Timebase { num: 1001, den: 30000 };
    let text = "TITLE: F\nFCM: DROP FRAME\n\n001  A        V     C        00:00:00;00 00:01:00;02 00:00:00;00 00:01:00;02\n";
    let tl = edl::parse(text, 25).expect("drop-frame reads");
    assert_eq!(tl.tb, ntsc);
    let c = tl.tracks[0].clips[0];
    assert_eq!((c.src_in, c.src_out, c.at, c.end()), (0, 1800, 0, 1800));
    assert_eq!(edl::parse("TITLE: X\nFCM: DROP FRAME\n", 30).map(|t| (t.tb, t.tracks.len())), Ok((ntsc, 0)));
    let tl60 = edl::parse("FCM: DROP FRAME\n* UNA FPS 60\n001  A        V     C        00:00:00;00 00:01:00;04 00:00:00;00 00:01:00;04\n", 25).expect("59.94 reads");
    assert_eq!((tl60.tb, tl60.tracks[0].clips[0].src_out), (timeline_core::Timebase { num: 1001, den: 60000 }, 3600));
}

/// Still refused, each by name: a skipped label (`;00` at minute 1), drop-frame at a nominal but 30 / 60, `;` in a non-drop file.
#[test]
fn striatcthree_drop_refusals() {
    let ev = "001  A        V     C        00:00:00;00 00:01:00;00 00:00:00;00 00:01:00;00";
    assert_eq!(edl::parse(&format!("TITLE: X\nFCM: DROP FRAME\n\n{ev}\n"), 30), Err(EdlError::Timecode { line: 4 }));
    assert_eq!(edl::parse("FCM: DROP FRAME\n* UNA FPS 24\n", 30), Err(EdlError::DropFrame { line: 1 }));
    assert_eq!(edl::parse(&format!("FCM: NON-DROP FRAME\n{ev}\n"), 30), Err(EdlError::DropFrame { line: 2 }));
    let mut tl = Timeline::new(24);
    tl.tb = timeline_core::Timebase { num: 1001, den: 24000 };
    assert_eq!(edl::write(&tl, "X"), Err(EdlError::Fps), "23.976 has no drop-frame (NLE-TC-NTSC24-EDL owed)");
}

/// `drop_tick` inverts `drop_number` at 30 and 60 (and `timecode_drop` is built on it); other nominals are the identity.
#[test]
fn striatcthree_drop_number_inverse() {
    use timeline_core::edl::{drop_number, drop_tick, timecode_drop};
    for nom in [30u32, 60] {
        let mut prev = -1;
        for t in 0..40_000i64 {
            let n = drop_number(t, nom);
            assert!(n > prev, "strictly increasing at {t}");
            assert_eq!(drop_tick(n, nom), Some(t), "nominal {nom} tick {t}");
            prev = n;
        }
    }
    assert_eq!((drop_tick(1800, 30), drop_tick(1801, 30), drop_tick(18000, 30)), (None, None, Some(17982)));
    assert_eq!((drop_number(7, 25), drop_tick(7, 25), drop_number(-3, 30)), (7, Some(7), 0));
    assert_eq!(timecode_drop(1800, 30), "00:01:00;02");
}

// ---- STRIAPERFTHREE (LEDGER SR9037): timecode_in is timecode into a reused buffer (named tail block) ----
#[test]
fn striaperfthree_timecode_in_matches() {
    let mut buf = String::new();
    for fps in [0u32, 24, 25, 30] {
        for t in [-5i64, 0, 24, 25, 90_125, 10_000_000] {
            let want = edl::timecode(t, fps);
            assert_eq!(edl::timecode_in(&mut buf, t, fps), want, "t {t} fps {fps}");
        }
    }
    let cap = buf.capacity();
    assert_eq!(edl::timecode_in(&mut buf, 0, 25), "00:00:00:00");
    assert_eq!(buf.capacity(), cap, "the buffer is reused");
}

// ---- STRIAOPACITYKEYS (LEDGER SR9048): the `* UNA ALPHAKEYS` line (named tail block) ----

/// A clip with opacity points writes ONE `* UNA ALPHAKEYS` line under its event (after its ALPHA line) and reads back
/// equal; a timeline with no points writes what it wrote; each refusal is a named syntax error.
#[test]
fn striaopacitykeys_alphakeys_round_trip_and_refusals() {
    let tl = cut();
    let plain = edl::write(&tl, "CUT ONE").unwrap();
    assert!(!plain.contains("ALPHAKEYS"), "no points, no line");
    let tl = tl.set_opacity(&[(1, 0)], 50).unwrap().set_opacity_key(&[(1, 0)], 10, Some(0)).unwrap().set_opacity_key(&[(1, 0)], 19, Some(90)).unwrap();
    let text = edl::write(&tl, "CUT ONE").unwrap();
    let ev = text.lines().position(|l| l.contains(" V2 ")).unwrap();
    assert_eq!(text.lines().nth(ev + 1), Some("* UNA ALPHA 50"));
    assert_eq!(text.lines().nth(ev + 2), Some("* UNA ALPHAKEYS 0:0 9:90"), "source ticks, under its ALPHA line");
    let back = edl::parse(&text, 30).unwrap();
    assert_eq!(back, tl);
    assert_eq!(edl::write(&back, "CUT ONE").unwrap(), text);
    let head = "TITLE: X\nFCM: NON-DROP FRAME\n";
    let v = "001  A        V     C        00:00:00:00 00:00:01:00 00:00:00:00 00:00:01:00\n";
    let a = "001  A        A     C        00:00:00:00 00:00:01:00 00:00:00:00 00:00:01:00\n";
    assert_eq!(edl::parse(&format!("{head}* UNA ALPHAKEYS 0:50\n"), 25), Err(EdlError::Syntax { line: 3 }), "no event above");
    for bad in ["0:101", "0-50", "x:50", "5:50 5:60", "5:50 2:60", "-1:50", "0:1 1:1 2:1 3:1 4:1 5:1 6:1 7:1 8:1"] {
        assert_eq!(edl::parse(&format!("{head}{v}* UNA ALPHAKEYS {bad}\n"), 25), Err(EdlError::Syntax { line: 4 }), "{bad:?}");
    }
    assert_eq!(edl::parse(&format!("{head}{v}* UNA ALPHAKEYS 0:50\n* UNA ALPHAKEYS 1:50\n"), 25), Err(EdlError::Syntax { line: 5 }), "a second line");
    assert_eq!(edl::parse(&format!("{head}{a}* UNA ALPHAKEYS 0:50\n"), 25), Err(EdlError::Edit(EditError::Opacity)), "audio");
    let one = edl::parse(&format!("{head}{v}* UNA ALPHAKEYS 0:0 24:96\n"), 25).unwrap();
    assert_eq!(one.tracks[0].clips[0].opacity_at(12), 48);
    println!("striaopacitykeys: ALPHAKEYS round trip");
}

// ---- STRIASEQRATEEDL (LEDGER SR9053): the `* UNA SEQRATE` line (named tail block) ----

/// CUT with `l` right after reel A's SOURCE line (line 5).
fn striaseqrateedl_at(l: &str) -> String {
    CUT.replace("* UNA SOURCE A 00:00:02:00\n", &format!("* UNA SOURCE A 00:00:02:00\n{l}\n"))
}

#[test]
fn striaseqrateedl_seqrate_round_trips() {
    for (word, rate) in [("12", (12u64, 1u64)), ("25/2", (25, 2))] {
        let text = striaseqrateedl_at(&format!("* UNA SEQRATE A {word}"));
        let tl = edl::parse(&text, 25).unwrap();
        let mut want = cut();
        want.sources[0].seq_rate = Some(rate);
        assert_eq!(tl, want, "{word}");
        assert_eq!(edl::write(&tl, "CUT ONE").unwrap(), text, "{word}: written right after its SOURCE line, byte exact");
    }
    let reduced = edl::parse(&striaseqrateedl_at("* UNA SEQRATE A 50/4"), 25).unwrap();
    assert_eq!(reduced.sources[0].seq_rate, Some((25, 2)), "read reduced");
    assert_eq!(edl::write(&cut(), "CUT ONE").unwrap(), CUT, "no rate: no SEQRATE line");
    println!("striaseqrateedl: `* UNA SEQRATE A 12` / `25/2` round-trip byte exact; 50/4 reads as 25/2");
}

#[test]
fn striaseqrateedl_seqrate_refused_by_name() {
    for bad in ["* UNA SEQRATE A", "* UNA SEQRATE A 12 13", "* UNA SEQRATE A 0", "* UNA SEQRATE A 1/0", "* UNA SEQRATE A x", "* UNA SEQRATE A -3", "* UNA SEQRATE A 12.5", "* UNA SEQRATE B 12"] {
        assert_eq!(edl::parse(&striaseqrateedl_at(bad), 25), Err(EdlError::Syntax { line: 5 }), "{bad}");
    }
    let twice = striaseqrateedl_at("* UNA SEQRATE A 12\n* UNA SEQRATE A 12");
    assert_eq!(edl::parse(&twice, 25), Err(EdlError::Syntax { line: 6 }), "a second SEQRATE for one reel");
    let titled = striaseqrateedl_at("* UNA TITLE A \"Hi\"\n* UNA SEQRATE A 12");
    assert_eq!(edl::parse(&titled, 25), Err(EdlError::Syntax { line: 6 }), "a title has no file");
}
