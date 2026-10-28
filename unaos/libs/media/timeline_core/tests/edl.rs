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
    assert_eq!(edl::parse("TITLE: X\nFCM: DROP FRAME\n", 30), Err(EdlError::DropFrame { line: 2 }));
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
