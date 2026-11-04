// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VUGFACEPHOTO (LEDGER SR506): the photo face's host witness — photo_core's synthetic graph-paper photo opens as its
//! measured sketch (the same primitives, one profile with its two holes) over the dimmed photo (dark at the part, light
//! at the paper); proposals are verified through doc_core::intent and drawn as glyphs; Enter enters an accepted one
//! through the sketch face's apply_as road and the sketch is exact; Enter on a refused one changes nothing and names
//! the refusal; Backspace rejects; Tab cycles; `i` asks Vein over the bus (VUGFACEINTENTSEND SR514: one Propose out, the
//! live id's Proposals offered, a stale id ignored, a Refused named). The proposal road is witnessed twice: on
//! the fixture's hole circles, and on a hand-built photo face (the intent arc's 49.7 x 30.2 rectangle, sigma 0.3) that
//! does not depend on photo_core. VUGFACESTREAM (SR535): Vein's streamed proposals enter as previews (verified, marked
//! STREAMING), Enter on one waits by name, the terminal Proposals settles them, Refused withdraws them with the count.

use bandy::{IntentEvent, IntentProposal, SMessage};
use doc_core::{Proposal, Reason, Verdict};
use sketch_core::{Entity, EntityId, PointRef, Sketch, Vec2};
use vug_view::model::Model;
use vug_view::photo::{self, Photo, State};
use vug_view::raster;
use vug_view::sketch::{SketchFace, world_planes};
use vug_view::view::Preset;
use vug_view::Viewer;

const W: u32 = 640;
const H: u32 = 480;

fn opened() -> Viewer {
    Viewer::open_photo(&photo_core::fixture::photo_png(), "fixture", (W, H), Preset::Una, photo::PITCH_MM).expect("the fixture measures")
}

fn face(vw: &Viewer) -> &SketchFace {
    vw.view.design.face.as_ref().expect("the measured sketch is open")
}

fn ph(vw: &Viewer) -> &Photo {
    vw.view.photo.as_ref().expect("a photo is open")
}

fn primitives(sk: &Sketch) -> usize {
    sk.entities().iter().filter(|e| matches!(e, Entity::Line { .. } | Entity::Circle { .. } | Entity::Arc { .. })).count()
}

fn circles(sk: &Sketch) -> Vec<EntityId> {
    sk.entities().iter().enumerate().filter(|(_, e)| matches!(e, Entity::Circle { .. })).map(|(i, _)| EntityId(i as u32)).collect()
}

fn proposal(stmt: String) -> Proposal {
    Proposal { stmt, confidence: 0.9, why: "a round size".into() }
}

fn key(vw: &mut Viewer, k: &str) -> bool {
    vw.input(&SMessage::BrowserKey(k.into()))
}

fn luma(p: [u8; 4]) -> f64 {
    0.299 * p[0] as f64 + 0.587 * p[1] as f64 + 0.114 * p[2] as f64
}

/// Some pixel of the glyph's first character cell carries its colour.
fn drawn(fr: &raster::Frame, g: &photo::ProposalGlyph) -> bool {
    let col = raster::rgba(g.colour);
    let (x0, y0) = (g.at.0.round().max(0.0) as u32, g.at.1.round().max(0.0) as u32);
    (y0..(y0 + 10).min(fr.height)).any(|y| (x0..(x0 + 6).min(fr.width)).any(|x| fr.pixel(x, y) == col))
}

fn inside(poly: &[Vec2], p: Vec2) -> bool {
    let n = poly.len();
    let mut c = false;
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a.y > p.y) != (b.y > p.y) && p.x < a.x + (p.y - a.y) / (b.y - a.y) * (b.x - a.x) {
            c = !c;
        }
    }
    c
}

fn seg_dist(p: Vec2, a: Vec2, b: Vec2) -> f64 {
    let (dx, dy) = (b.x - a.x, b.y - a.y);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p.x - a.x) * dx + (p.y - a.y) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p.x - a.x - t * dx).hypot(p.y - a.y - t * dy)
}

#[test]
fn photo_opens_as_its_measured_sketch() {
    let img = pixel_core::decode(&photo_core::fixture::photo_png()).expect("decodes");
    let m = photo_core::measure(&img, photo_core::PhotoParams::new(photo::PITCH_MM), sketch_core::Tol::DEFAULT).expect("measures");
    let vw = opened();
    let f = face(&vw);
    assert_eq!(primitives(&f.sketch), primitives(&m.sketch), "the face holds the measured primitives, no more");
    assert_eq!(f.profiles.len(), 1, "one profile");
    assert_eq!(f.profiles[0].holes.len(), 2, "with its two holes");
    assert!(ph(&vw).sigma_mm > 0.0);
}

#[test]
fn underlay_is_dark_at_the_part_and_light_at_the_paper() {
    let vw = opened();
    let f = face(&vw);
    let outer: Vec<Vec2> = f.profiles[0].outer.edges.iter().map(|e| e.start()).collect();
    let holes: Vec<(Vec2, f64)> = circles(&f.sketch)
        .into_iter()
        .filter_map(|c| Some((f.sketch.point(PointRef::Centre(c))?, f.sketch.radius(c)?)))
        .collect();
    let (lo, hi) = outer.iter().fold(((f64::MAX, f64::MAX), (f64::MIN, f64::MIN)), |(l, h), p| ((l.0.min(p.x), l.1.min(p.y)), (h.0.max(p.x), h.1.max(p.y))));
    // A part point: inside the outer loop, 2 mm clear of every edge and every hole.
    let mut part = None;
    'scan: for i in 0..200 {
        for j in 0..200 {
            let q = Vec2::new(lo.0 + (hi.0 - lo.0) * i as f64 / 200.0, lo.1 + (hi.1 - lo.1) * j as f64 / 200.0);
            let n = outer.len();
            if inside(&outer, q)
                && (0..n).all(|k| seg_dist(q, outer[k], outer[(k + 1) % n]) > 2.0)
                && holes.iter().all(|(c, r)| (q.x - c.x).hypot(q.y - c.y) > r + 2.0)
            {
                part = Some(q);
                break 'scan;
            }
        }
    }
    let part = part.expect("a part point clear of the edges");
    let paper = Vec2::new(lo.0 - 5.0, lo.1 - 5.0);
    let fr = raster::render(&vw.model, &vw.view);
    let px = |q: Vec2| {
        let (x, y) = f.to_screen(&vw.view, q).expect("on screen");
        fr.pixel(x as u32, y as u32)
    };
    let (dark, light) = (luma(px(part)), luma(px(paper)));
    assert!(dark < 90.0, "the part is dimmed dark: luma {dark}");
    assert!(light > 110.0 && light < 230.0, "the paper is dimmed light: luma {light}");
}

#[test]
fn hole_proposals_on_the_photo() {
    let mut vw = opened();
    let cs = circles(&face(&vw).sketch);
    assert!(cs.len() >= 2, "the two holes are circles");
    let (c1, c2) = (cs[0], cs[1]);
    assert!(photo::offer(
        &mut vw.view,
        vec![proposal(format!("diameter v1 sketch=s circle=e{} d=6", c1.0)), proposal(format!("diameter v2 sketch=s circle=e{} d=9", c2.0))]
    ));
    assert!(matches!(ph(&vw).offers[0].verdict, Verdict::Accept { .. }), "6 mm is inside the photo's band");
    assert!(matches!(ph(&vw).offers[1].verdict, Verdict::Refuse { why: Reason::Residual { .. } }), "9 mm is not");
    let fr = raster::render(&vw.model, &vw.view);
    let gs = photo::glyphs(&vw.view);
    assert_eq!(gs.len(), 2, "two glyphs");
    assert!(gs.iter().all(|g| drawn(&fr, g)), "both drawn in their colour");
    assert!(key(&mut vw, "Return"));
    assert_eq!(ph(&vw).offers[0].state, State::Applied);
    assert!((face(&vw).sketch.radius(c1).unwrap() - 3.0).abs() < 1e-6, "the hole is exactly 6 mm");
    let before = face(&vw).sketch.params().to_vec();
    assert_eq!(ph(&vw).focus, Some(1));
    assert!(key(&mut vw, "Return"));
    assert_eq!(face(&vw).sketch.params(), &before[..], "a refused proposal changes nothing");
    assert_eq!(ph(&vw).offers[1].state, State::Open);
    // Refused by name either road: by residual before the first hole was fixed, by the replay (the inferred EqualRadius
    // with the 6 mm hole now makes 9 mm inconsistent: the solver diverges) after it. Both are a refusal, never an apply.
    assert!(ph(&vw).note.as_deref().unwrap_or("").starts_with("refused "), "{:?}", ph(&vw).note);
    assert!(matches!(ph(&vw).offers[1].verdict, Verdict::Refuse { .. }));
}

/// The intent arc's rectangle as a photo face: four lines from 49.7 x 30.2, the outline at sigma 0.3, a 2 x 2 white
/// photo over the paper (the underlay is the fixture test's).
fn rectangle() -> Viewer {
    let mut sk = Sketch::new();
    let c = [(0.0, 0.0), (49.7, 0.0), (49.7, 30.2), (0.0, 30.2)];
    let p: Vec<EntityId> = c.iter().map(|&(x, y)| sk.add_point(x, y)).collect();
    for i in 0..4 {
        sk.add_line(p[i], p[(i + 1) % 4]).expect("a line");
    }
    let face = SketchFace::over(world_planes()[0].1, sk);
    let paper = Model::from_triangles(&[[-20.0, -20.0, 0.0], [70.0, -20.0, 0.0], [70.0, 50.0, 0.0], [-20.0, 50.0, 0.0]], &[[0, 1, 2], [0, 2, 3]]).expect("paper");
    let png = pixel_core::png::encode::encode_rgba(2, 2, &[255u8; 16]).expect("png");
    let image = pixel_core::decode(&png).expect("decodes");
    let mut vw = Viewer::new(paper, "rectangle", (W, H), Preset::Una);
    vw.view.top(&vw.model);
    vw.view.design.face = Some(face);
    vw.view.photo = Some(Photo {
        image,
        h: [[0.02, 0.0, 0.5], [0.0, 0.02, 0.5], [0.0, 0.0, 1.0]],
        outline: c.iter().map(|&(x, y)| Vec2::new(x, y)).collect(),
        loops: vec![c.iter().map(|&(x, y)| Vec2::new(x, y)).collect()],
        sigma_mm: 0.3,
        band_mm: 0.3,
        offers: Vec::new(),
        focus: None,
        note: None,
        outbox: Vec::new(), // VUGFACEINTENTSEND (SR514)
        request: 0,
        asked: None,
        ignored: 0,
    });
    vw
}

fn dist(sk: &Sketch, a: u32, b: u32) -> f64 {
    let (p, q) = (sk.point(PointRef::Point(EntityId(a))).unwrap(), sk.point(PointRef::Point(EntityId(b))).unwrap());
    (q.x - p.x).hypot(q.y - p.y)
}

#[test]
fn a_proposal_enters_through_apply_as_and_a_refused_one_never() {
    let mut vw = rectangle();
    assert_eq!(face(&vw).profiles.len(), 1);
    // e0..e3 the corners, e4..e7 the sides.
    assert!(photo::offer(
        &mut vw.view,
        vec![proposal("distance v1 sketch=s a=e0 b=e1 d=50".into()), proposal("distance v2 sketch=s a=e1 b=e2 d=33".into())]
    ));
    assert!(matches!(ph(&vw).offers[0].verdict, Verdict::Accept { .. }));
    assert!(matches!(ph(&vw).offers[1].verdict, Verdict::Refuse { why: Reason::Residual { .. } }));
    assert!(ph(&vw).note.as_deref().unwrap().contains("proposed=2 acceptable=1 refused=1"));
    let fr = raster::render(&vw.model, &vw.view);
    let gs = photo::glyphs(&vw.view);
    assert_eq!(gs.len(), 2);
    assert!(gs.iter().all(|g| drawn(&fr, g)));
    assert!(gs[1].text.contains("REFUSED RESIDUAL"), "the refusal by name: {}", gs[1].text);
    // Tab cycles the focus over the open offers.
    assert_eq!(ph(&vw).focus, Some(0));
    assert!(key(&mut vw, "Tab"));
    assert_eq!(ph(&vw).focus, Some(1));
    assert!(key(&mut vw, "Tab"));
    assert_eq!(ph(&vw).focus, Some(0));
    // Enter on the accepted one: exact.
    assert!(key(&mut vw, "Return"));
    assert_eq!(ph(&vw).offers[0].state, State::Applied);
    assert!((dist(&face(&vw).sketch, 0, 1) - 50.0).abs() < 1e-6, "the side is exactly 50");
    // Enter on the refused one: nothing moves, the refusal is named.
    let before = face(&vw).sketch.params().to_vec();
    assert_eq!(ph(&vw).focus, Some(1));
    assert!(key(&mut vw, "Return"));
    assert_eq!(face(&vw).sketch.params(), &before[..]);
    assert_eq!(ph(&vw).offers[1].state, State::Open);
    assert!(ph(&vw).note.as_deref().unwrap().starts_with("refused residual"));
    // Backspace rejects it; no glyph is left.
    assert!(key(&mut vw, "BackSpace"));
    assert_eq!(ph(&vw).offers[1].state, State::Rejected);
    assert!(photo::glyphs(&vw.view).is_empty());
    // With nothing focused, Backspace is the sketch face's erase again (the photo does not take it).
    assert_eq!(photo::key(&mut vw.view, "BackSpace"), None);
}

// ---- VUGFACEINTENTSEND (SR514): the ask over the bus and its answer ----

fn ask(vw: &mut Viewer) -> Vec<SMessage> {
    assert!(vw.input(&SMessage::BrowserText("i".into())), "`i` changes the corner");
    vw.outbox()
}

fn vein(stmt: &str) -> IntentProposal {
    IntentProposal { stmt: stmt.into(), confidence: 0.9, why: "a round size".into() }
}

#[test]
fn intent_sends_one_propose_over_the_bus() {
    let mut vw = rectangle();
    let out = ask(&mut vw);
    assert_eq!(out.len(), 1, "exactly one outbound message");
    let SMessage::Intent(IntentEvent::Propose { context, measurements, images, request, .. }) = &out[0] else {
        panic!("not a Propose: {:?}", out[0]);
    };
    let doc = doc_core::read(context).expect("the context parses back");
    assert_eq!(doc_core::write(&doc), doc_core::write(&photo::doc_of(face(&vw))), "the context is the face document");
    assert!(measurements.starts_with("outline:"), "{measurements}");
    assert_eq!(images.len(), 1);
    assert_eq!(images[0].media_type, "image/png");
    assert!(images[0].bytes.starts_with(b"\x89PNG\r\n\x1a\n"), "PNG bytes");
    assert_eq!(*request, 1);
    assert_eq!(ph(&vw).asked, Some(1));
    assert_eq!(ph(&vw).note.as_deref(), Some("asked Vein (request 1)"));
    assert!(vw.outbox().is_empty(), "drained once");
    // Our own Propose back on the shared bus is not an answer.
    assert!(!vw.input(&out[0]));
    assert_eq!(ph(&vw).ignored, 0);
    assert_eq!(ph(&vw).asked, Some(1));
}

#[test]
fn proposals_for_the_live_request_are_offered() {
    let mut vw = rectangle();
    // The Echo case: no proposals, proposed=0, never a fake one.
    ask(&mut vw);
    assert!(vw.input(&SMessage::Intent(IntentEvent::Proposals { request: 1, proposals: Vec::new() })));
    assert!(ph(&vw).offers.is_empty());
    assert!(ph(&vw).note.as_deref().unwrap().contains("proposed=0"));
    assert_eq!(ph(&vw).asked, None);
    ask(&mut vw);
    let proposals = vec![vein("distance v1 sketch=s a=e0 b=e1 d=50"), vein("distance v2 sketch=s a=e1 b=e2 d=33")];
    assert!(vw.input(&SMessage::Intent(IntentEvent::Proposals { request: 2, proposals })));
    assert_eq!(ph(&vw).asked, None, "answered");
    assert_eq!(ph(&vw).offers.len(), 2);
    assert_eq!(photo::glyphs(&vw.view).len(), 2, "two glyphs");
    assert!(ph(&vw).note.as_deref().unwrap().contains("proposed=2 acceptable=1 refused=1"));
}

#[test]
fn a_stale_request_id_changes_nothing() {
    let mut vw = rectangle();
    ask(&mut vw);
    ask(&mut vw);
    assert_eq!(ph(&vw).asked, Some(2), "the second ask is the live one");
    let (note, focus) = (ph(&vw).note.clone(), ph(&vw).focus);
    let stale = SMessage::Intent(IntentEvent::Proposals { request: 1, proposals: vec![vein("distance v1 sketch=s a=e0 b=e1 d=50")] });
    assert!(!vw.input(&stale), "a stale answer draws nothing");
    assert!(ph(&vw).offers.is_empty());
    assert_eq!(ph(&vw).note, note);
    assert_eq!(ph(&vw).focus, focus);
    assert_eq!(ph(&vw).asked, Some(2));
    assert_eq!(ph(&vw).ignored, 1, "counted");
    assert!(!vw.input(&SMessage::Intent(IntentEvent::Refused { request: 99, why: "NoProvider: x".into() })), "an unknown id too");
    assert_eq!(ph(&vw).note, note);
    assert_eq!(ph(&vw).ignored, 2);
}

#[test]
fn a_refusal_names_its_why() {
    let mut vw = rectangle();
    ask(&mut vw);
    assert!(vw.input(&SMessage::Intent(IntentEvent::Refused { request: 1, why: "NoProvider: no key".into() })));
    assert_eq!(ph(&vw).note.as_deref(), Some("Vein refused: NoProvider: no key"));
    assert!(ph(&vw).offers.is_empty());
    assert_eq!(ph(&vw).asked, None);
    // Answered: a second answer to the same id is stale.
    assert!(!vw.input(&SMessage::Intent(IntentEvent::Refused { request: 1, why: "again".into() })));
    assert_eq!(ph(&vw).ignored, 1);
}

// ---- VUGFACESTREAM (SR535): the streamed preview ----

const V1: &str = "distance v1 sketch=s a=e0 b=e1 d=50";
const V2: &str = "distance v2 sketch=s a=e1 b=e2 d=33";
const V3: &str = "distance v3 sketch=s a=e2 b=e3 d=50";

fn streamed(vw: &mut Viewer, request: u64, index: u32, stmt: &str) -> bool {
    vw.input(&SMessage::Intent(IntentEvent::Proposal { request, index, proposal: vein(stmt) }))
}

#[test]
fn three_streamed_proposals_settle_on_the_terminal() {
    let mut vw = rectangle();
    ask(&mut vw);
    for (i, s) in [V1, V2, V3].iter().enumerate() {
        assert!(streamed(&mut vw, 1, i as u32, s), "a preview draws");
    }
    assert_eq!(ph(&vw).asked, Some(1), "the terminal answer still follows");
    assert_eq!(ph(&vw).offers.len(), 3);
    assert!(ph(&vw).offers.iter().enumerate().all(|(i, o)| o.streaming == Some(i as u32)), "each marked by its index");
    assert!(matches!(ph(&vw).offers[0].verdict, Verdict::Accept { .. }));
    assert!(matches!(ph(&vw).offers[1].verdict, Verdict::Refuse { why: Reason::Residual { .. } }), "verified on arrival");
    let gs = photo::glyphs(&vw.view);
    assert_eq!(gs.len(), 3, "three glyphs");
    assert!(gs.iter().all(|g| g.text.ends_with(" STREAMING")), "{:?}", gs.iter().map(|g| &g.text).collect::<Vec<_>>());
    assert!(gs[0].text.contains(" R="), "the residual shows: {}", gs[0].text);
    assert_eq!(ph(&vw).note.as_deref(), Some("Vein streaming: 3 proposals (request 1)"));
    // Tab focuses a preview.
    assert_eq!(ph(&vw).focus, Some(0));
    assert!(key(&mut vw, "Tab"));
    assert_eq!(ph(&vw).focus, Some(1));
    // A stale id's preview is ignored and counted.
    assert!(!streamed(&mut vw, 7, 3, V1));
    assert_eq!(ph(&vw).ignored, 1);
    assert_eq!(ph(&vw).offers.len(), 3);
    let verdicts: Vec<Verdict> = ph(&vw).offers.iter().map(|o| o.verdict.clone()).collect();
    let proposals = vec![vein(V1), vein(V2), vein(V3)];
    assert!(vw.input(&SMessage::Intent(IntentEvent::Proposals { request: 1, proposals })));
    assert_eq!(ph(&vw).asked, None, "answered");
    assert_eq!(ph(&vw).offers.len(), 3, "the same three, settled in place");
    assert!(ph(&vw).offers.iter().all(|o| o.streaming.is_none()), "the mark cleared");
    assert_eq!(ph(&vw).offers.iter().map(|o| o.verdict.clone()).collect::<Vec<_>>(), verdicts, "equal items keep their verdict");
    let gs = photo::glyphs(&vw.view);
    assert_eq!(gs.len(), 3);
    assert!(gs.iter().all(|g| !g.text.contains("STREAMING")));
    assert!(ph(&vw).note.as_deref().unwrap().contains("proposed=3 acceptable=2 refused=1"), "{:?}", ph(&vw).note);
}

#[test]
fn enter_during_the_stream_is_refused_by_name() {
    let mut vw = rectangle();
    ask(&mut vw);
    assert!(streamed(&mut vw, 1, 0, V1));
    assert!(matches!(ph(&vw).offers[0].verdict, Verdict::Accept { .. }));
    assert_eq!(ph(&vw).focus, Some(0));
    let before = face(&vw).sketch.params().to_vec();
    assert!(key(&mut vw, "Return"));
    assert_eq!(ph(&vw).note.as_deref(), Some("wait: the answer is not complete"));
    assert_eq!(face(&vw).sketch.params(), &before[..], "nothing entered");
    assert_eq!(ph(&vw).offers[0].state, State::Open);
    assert_eq!(ph(&vw).offers[0].streaming, Some(0));
    // Settled by the terminal answer, the same Enter enters it exactly.
    assert!(vw.input(&SMessage::Intent(IntentEvent::Proposals { request: 1, proposals: vec![vein(V1)] })));
    assert!(key(&mut vw, "Return"));
    assert_eq!(ph(&vw).offers[0].state, State::Applied);
    assert!((dist(&face(&vw).sketch, 0, 1) - 50.0).abs() < 1e-6, "the side is exactly 50");
}

#[test]
fn a_refusal_after_two_withdraws_both() {
    let mut vw = rectangle();
    ask(&mut vw);
    assert!(streamed(&mut vw, 1, 0, V1));
    assert!(streamed(&mut vw, 1, 1, V2));
    assert_eq!(photo::glyphs(&vw.view).len(), 2);
    // Vein's own wire already names the count (handlers/vein intent): never written twice.
    assert!(vw.input(&SMessage::Intent(IntentEvent::Refused { request: 1, why: "ProseAroundJson after 2 proposals".into() })));
    assert!(ph(&vw).offers.is_empty(), "both withdrawn");
    assert!(photo::glyphs(&vw.view).is_empty());
    assert_eq!(ph(&vw).note.as_deref(), Some("Vein refused: ProseAroundJson after 2 proposals"));
    assert_eq!(ph(&vw).asked, None);
    assert_eq!(ph(&vw).focus, None);
    // A bare why: the face names the count that arrived.
    ask(&mut vw);
    assert!(streamed(&mut vw, 2, 0, V1));
    assert!(streamed(&mut vw, 2, 1, V3));
    assert!(vw.input(&SMessage::Intent(IntentEvent::Refused { request: 2, why: "ProseAroundJson".into() })));
    assert!(ph(&vw).offers.is_empty());
    assert_eq!(ph(&vw).note.as_deref(), Some("Vein refused: ProseAroundJson after 2 proposals"));
    // A new ask withdraws the stale request's previews.
    ask(&mut vw);
    assert!(streamed(&mut vw, 3, 0, V1));
    ask(&mut vw);
    assert!(ph(&vw).offers.is_empty(), "the stale preview went with its request");
    assert_eq!(ph(&vw).asked, Some(4));
}
