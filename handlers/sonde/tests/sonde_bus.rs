// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// REALSENSE (LEDGER SR92): Sonde's bus verbs and pane, driven without a runtime — `dispatch` / `tick` /
// `PaneService::handle` are the whole behaviour `serve` loops over.

use bandy::SMessage;
use probe_core::frame::{PixelFormat, StreamKind};
use sonde::model::{PaneInput, ProbePane, Rect, fit};
use sonde::{PANE_URL, PaneService, Sonde, frame_from_message};

fn open(s: &mut Sonde, stream: &str, w: u32, h: u32) -> Vec<SMessage> {
    s.dispatch(&SMessage::ProbeOpen { id: "replay:d435i".into(), stream: stream.into(), width: w, height: h, fps: 0 })
}

#[test]
fn list_names_the_replay_probe_and_ignores_its_own_answer() {
    let mut s = Sonde::new();
    let out = s.dispatch(&SMessage::ProbeList { probes: vec![] });
    assert_eq!(out.len(), 1);
    let SMessage::ProbeList { probes } = &out[0] else { panic!("{out:?}") };
    assert_eq!(probes.len(), 1);
    assert_eq!(probes[0].0, "replay:d435i");
    assert_eq!(probes[0].2, vec!["depth", "ir", "color"]);
    // the answer itself (non-empty) is not a request
    assert!(s.dispatch(&out[0]).is_empty());
}

#[test]
fn open_ticks_frames_at_the_d435i_defaults_and_close_stops_them() {
    let mut s = Sonde::new();
    assert!(open(&mut s, "depth", 0, 0).is_empty(), "a good open answers with frames, not a reply");
    assert!(open(&mut s, "color", 0, 0).is_empty());
    let f = s.tick();
    assert_eq!(f.len(), 2);
    let d = frame_from_message(&f[0]).expect("a valid depth frame");
    assert_eq!((d.frame.stream, d.frame.format, d.frame.width, d.frame.height), (StreamKind::Depth, PixelFormat::Z16, 848, 480));
    assert_eq!(d.scale.unwrap().micrometres(), 1000);
    assert!(d.intrinsics.is_some());
    let c = frame_from_message(&f[1]).expect("a valid colour frame");
    assert_eq!((c.frame.format, c.frame.width, c.frame.height), (PixelFormat::Yuy2, 1280, 720));
    assert!(c.scale.is_none() && c.intrinsics.is_none());
    // seq advances per stream
    let g = s.tick();
    assert_eq!(frame_from_message(&g[0]).unwrap().frame.seq, 1);

    let closed = s.dispatch(&SMessage::ProbeClose { id: "replay:d435i".into(), stream: "depth".into() });
    assert_eq!(closed.len(), 1);
    assert_eq!(s.tick().len(), 1, "colour still open");
    let all = s.dispatch(&SMessage::ProbeClose { id: "replay:d435i".into(), stream: String::new() });
    assert_eq!(all.len(), 1);
    assert!(s.tick().is_empty());
}

#[test]
fn a_bad_open_answers_close() {
    let mut s = Sonde::new();
    for (id, stream) in [("nope", "depth"), ("replay:d435i", "lidar"), ("replay:d435i", "gyro")] {
        let out = s.dispatch(&SMessage::ProbeOpen { id: id.into(), stream: stream.into(), width: 0, height: 0, fps: 0 });
        assert_eq!(out.len(), 1, "{id}/{stream}");
        assert!(matches!(&out[0], SMessage::ProbeClose { .. }));
    }
    assert!(s.tick().is_empty());
}

#[test]
fn input_grammar_round_trips() {
    for i in [
        PaneInput::PointerMove { x: 12.5, y: 3.0 },
        PaneInput::PointerLeave,
        PaneInput::Scroll { dy: -1.0 },
        PaneInput::Key("plus".into()),
        PaneInput::Resize { width: 640, height: 240 },
    ] {
        assert_eq!(PaneInput::parse(&i.to_text()), Some(i.clone()), "{i:?}");
    }
    assert_eq!(PaneInput::parse("resize 0 10"), None);
    assert_eq!(PaneInput::parse("move 1"), None);
    assert_eq!(PaneInput::parse("dance"), None);
}

#[test]
fn fit_centres_and_keeps_aspect() {
    let a = Rect { x: 0, y: 0, w: 480, h: 360 };
    assert_eq!(fit(a, 848, 480), Rect { x: 0, y: 44, w: 480, h: 271 });
    assert_eq!(fit(a, 480, 720), Rect { x: 120, y: 0, w: 240, h: 360 });
}

#[test]
fn the_pane_shows_depth_left_colour_right_and_reads_the_pointer() {
    let mut s = Sonde::new();
    open(&mut s, "depth", 64, 48);
    open(&mut s, "color", 64, 48);
    let mut pane = PaneService::new(256, 96);
    let mut blits = 0;
    for m in s.tick() {
        if let Some(SMessage::SurfaceBlit { url, width, height, pixels }) = pane.handle(&m) {
            assert_eq!((url.as_str(), width, height, pixels.len()), (PANE_URL, 256, 96, 256 * 96 * 4));
            blits += 1;
        }
    }
    assert_eq!(blits, 2);
    let (px, w, _) = pane.pane.surface();
    let at = |x: u32, y: u32| &px[((y * w + x) * 4) as usize..((y * w + x) * 4 + 4) as usize];
    // depth half: 128x96 holding a 64:48 picture -> 128x96 at (0,0); column 0 is the plane's near edge
    // (400 mm, inside the 0.2..4 m ramp, so blue-ish and never the background)
    assert_ne!(at(0, 0), &[24, 26, 30, 255]);
    assert_eq!(at(0, 0)[2], 255, "near is on the blue end of the ramp");
    // the centre of the depth half is the hole: black
    assert_eq!(at(64, 48), &[0, 0, 0, 255]);
    // colour half: first bar of seq 0 is white
    let c = at(129, 1);
    assert!(c[0] > 150 && c[1] > 150 && c[2] > 150, "{c:?}");

    // pointer over the near edge reads 0.4 m and a point on the left of the axis
    let blit = pane.handle(&SMessage::Input { target: PANE_URL.into(), text: "move 0.5 0.5".into() });
    assert!(blit.is_some());
    let r = pane.pane.readout().expect("pointer is over the depth picture");
    assert_eq!((r.px, r.py), (0, 0));
    assert!((r.metres.unwrap() - 0.4).abs() < 1e-6);
    let p = r.point.unwrap();
    assert!(p[0] < 0.0 && p[1] < 0.0 && (p[2] - 0.4).abs() < 1e-6, "{p:?}");
    // over the hole: a pixel, no metres
    pane.handle(&SMessage::Input { target: PANE_URL.into(), text: "move 64 48".into() });
    assert_eq!(pane.pane.readout().unwrap().metres, None);
    // over the colour half: no depth readout
    pane.handle(&SMessage::Input { target: PANE_URL.into(), text: "move 200 48".into() });
    assert_eq!(pane.pane.readout(), None);
    // input for another target is not ours
    assert!(pane.handle(&SMessage::Input { target: "aether".into(), text: "move 1 1".into() }).is_none());
}

#[test]
fn scroll_and_keys_move_the_far_limit_within_bounds() {
    let mut p = ProbePane::new(64, 32);
    assert_eq!(p.range(), (0.2, 4.0));
    assert!(p.input(&PaneInput::Scroll { dy: 1.0 }));
    assert!((p.range().1 - 4.4).abs() < 1e-5);
    assert!(p.input(&PaneInput::Key("r".into())));
    assert_eq!(p.range().1, 4.0);
    for _ in 0..100 {
        p.input(&PaneInput::Key("minus".into()));
    }
    assert!((p.range().1 - 0.3).abs() < 1e-5, "never inside near + 0.1: {:?}", p.range());
    for _ in 0..100 {
        p.input(&PaneInput::Scroll { dy: 1.0 });
    }
    assert_eq!(p.range().1, 20.0);
    assert!(p.input(&PaneInput::Resize { width: 100, height: 50 }));
    assert_eq!(p.surface().1, 100);
    assert!(!p.input(&PaneInput::Resize { width: 100, height: 50 }), "same size is no change");
}
