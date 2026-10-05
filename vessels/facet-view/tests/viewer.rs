// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The viewer controller against a live Facet (handler dispatch, no window): every frame it blits
//! is exactly what Facet renders for the same view, and every shortcut lands as the right verb.

use bandy::signals::{FacetCommand, FacetView, FacetZoom};
use bandy::{Origin, SMessage};
use facet_view::Viewer;

const CARD: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../tools/eyes/suites/facet/fixtures/card.png");

/// Run requests through Facet until quiet; return the last blit.
fn pump(v: &mut Viewer, f: &mut facet::Facet, reqs: Vec<FacetCommand>) -> Option<(u32, u32, Vec<u8>)> {
    let mut queue = reqs;
    let mut blit = None;
    while let Some(r) = queue.pop() {
        for ans in f.dispatch(&r) {
            let (more, b) = v.answer(&ans);
            queue.extend(more);
            if let Some(SMessage::SurfaceBlit { width, height, pixels, url }) = b {
                assert_eq!(url, facet_view::SURFACE);
                blit = Some((width, height, pixels));
            }
        }
    }
    blit
}

#[test]
fn shortcuts_drive_facet_and_frames_match_its_render() {
    let mut f = facet::Facet::new();
    let mut v = Viewer::new(CARD, Origin::LocalUser("t".into()), (100, 80));
    let start = v.start();
    let (w, h, px) = pump(&mut v, &mut f, start).expect("first frame");
    assert_eq!((w, h), (100, 80));
    let handle = v.handle.unwrap();
    // The first frame is Facet's Fit render, byte for byte.
    assert_eq!(px, f.render(handle, 100, 80, &FacetView::default()).unwrap().rgba);
    assert_eq!(v.scale(), 1.0, "fit never enlarges a 48x32 card in 100x80");

    // Zoom in twice: 100 % -> 125 % -> 156 %.
    for k in ["+", "+"] {
        let r = v.key(k);
        pump(&mut v, &mut f, r);
    }
    assert_eq!(v.view.zoom, FacetZoom::Percent(156));
    // View turn + flip are display-only: no edit reaches Facet.
    for k in ["r", "h"] {
        let r = v.key(k);
        pump(&mut v, &mut f, r);
    }
    assert_eq!((v.view.quarter_turns, v.view.flip_h), (1, true));
    assert_eq!(f.info(handle).unwrap().edits, 0);
    let r = v.key("1");
    let (_, _, px) = pump(&mut v, &mut f, r).unwrap();
    assert_eq!(px, f.render(handle, 100, 80, &v.view).unwrap().rgba);

    // Edits: rotate, mirror, brightness, undo, redo, reset.
    for k in ["]", "m", "B"] {
        let r = v.key(k);
        pump(&mut v, &mut f, r);
    }
    let i = f.info(handle).unwrap();
    assert_eq!((i.edits, i.width, i.height), (3, 32, 48));
    let r = v.key("u");
    pump(&mut v, &mut f, r);
    assert_eq!(f.info(handle).unwrap().edits, 2);
    let r = v.key("U");
    pump(&mut v, &mut f, r);
    assert_eq!(f.info(handle).unwrap().edits, 3);
    let r = v.key("!");
    pump(&mut v, &mut f, r);
    assert_eq!(f.info(handle).unwrap().edits, 0);
    assert_eq!(v.info.as_ref().unwrap().edits, 0, "the viewer tracks Facet's info");
}

#[test]
fn crop_to_visible_through_every_view_transform() {
    for (turns, flip_h, flip_v) in [(0, false, false), (1, false, false), (2, true, false), (3, false, true), (1, true, true)] {
        let mut f = facet::Facet::new();
        let mut v = Viewer::new(CARD, Origin::LocalUser("t".into()), (40, 30));
        let s = v.start();
        pump(&mut v, &mut f, s);
        let handle = v.handle.unwrap();
        v.view = FacetView { zoom: FacetZoom::Percent(200), pan_x: -8, pan_y: 5, quarter_turns: turns, flip_h, flip_v };
        let before = f.render(handle, 40, 30, &v.view).unwrap();
        let r = v.key("x");
        pump(&mut v, &mut f, r);
        let i = f.info(handle).unwrap();
        assert_eq!(i.edits, 1);
        // 40x30 viewport at 200 % shows 20x15 picture pixels (displayed orientation).
        let (dw, dh) = if turns % 2 == 1 { (i.height, i.width) } else { (i.width, i.height) };
        assert_eq!((dw, dh), (20, 15), "turns {turns}");
        // The cropped picture, at 200 % centred in the same viewport, shows what was visible.
        let after = f.render(handle, 40, 30, &FacetView { pan_x: 0, pan_y: 0, ..v.view.clone() }).unwrap();
        assert_eq!(after, before, "turns {turns} flips {flip_h}/{flip_v}: crop-to-visible keeps the frame");
    }
}

#[test]
fn export_and_errors() {
    let dir = tempfile::tempdir().unwrap();
    let src = dir.path().join("pic.png");
    std::fs::copy(CARD, &src).unwrap();
    let mut f = facet::Facet::new();
    let mut v = Viewer::new(src.to_str().unwrap(), Origin::LocalUser("t".into()), (64, 64));
    let s = v.start();
    pump(&mut v, &mut f, s);
    let r = v.key("e");
    pump(&mut v, &mut f, r);
    assert_eq!(v.exported.as_deref(), Some(dir.path().join("pic.facet.png").to_str().unwrap()));
    assert!(dir.path().join("pic.facet.png").exists());

    let mut v = Viewer::new("/nonexistent/x.png", Origin::LocalUser("t".into()), (64, 64));
    let s = v.start();
    assert!(pump(&mut v, &mut f, s).is_none());
    assert!(v.last_error.is_some());
    // Answers that are not ours are ignored.
    let foreign = FacetCommand::ImageError { receipt_id: 1, handle: None, message: "x".into() };
    assert_eq!(v.answer(&foreign).0.len(), 0);
}
