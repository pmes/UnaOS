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
