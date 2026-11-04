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
        first: None, // VUGFACEPHOTOSIDE (SR547)
        thickness: None, // VUGINTENTTHICKNESS (SR557)
        prefs: Default::default(), // VUGFACEPHOTOCAMERAPREF (SR576)
        crop: Default::default(), // VUGFACEPHOTOCROPSAVE (SR640)
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
    assert!(key(&mut vw, "\r") && ph(&vw).offers[1].state == State::Open, "VUGFACEFIELDKEYSREST (SR605): the one-char Enter reaches the same refusal as Return");
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

// ---- VUGFACEPHOTOSIDE (SR547): the side photo's thickness as one extrude proposal ----

/// The fixture part (50 x 30 x 6): the flat photo, then the side photo proposes the extrude of the measured sketch by
/// 6.0 within 0.1 through the one verify road; Enter enters it by the journal's pull road and the journal records it.
#[test]
fn a_side_photo_proposes_the_extrude_and_the_journal_records_it() {
    use photo_core::fixture::{PART_H, PART_T, PART_W, Spec, side_raised_png};
    let mut vw = opened();
    photo::side(&mut vw.view, &side_raised_png(&Spec::side_raised(), PART_W, PART_T, PART_H), "side.png").expect("the side photo measures");
    let p = ph(&vw);
    assert_eq!(p.offers.len(), 1, "one proposal");
    let o = &p.offers[0];
    assert!(o.proposal.stmt.starts_with("extrude x0 sketch=s "), "{}", o.proposal.stmt);
    assert!(o.proposal.why.contains("side.png"), "{}", o.proposal.why);
    assert!(matches!(o.verdict, Verdict::Accept { .. }), "{:?}", o.verdict);
    let note = p.note.as_deref().unwrap_or("");
    assert!(note.contains("thickness=") && note.contains("sigma=") && note.contains("paired="), "{note}");
    assert_eq!(p.focus, Some(0));
    assert!(key(&mut vw, "Return"));
    assert_eq!(ph(&vw).offers[0].state, State::Applied, "{:?}", ph(&vw).note);
    assert!(vw.view.design.face.is_none(), "the sketch closed with its extrude");
    assert!(vw.view.design.body.is_some(), "a body");
    let length = vw
        .view
        .design
        .journal
        .doc
        .stmts
        .iter()
        .find_map(|s| if let doc_core::Stmt::Extrude { length, .. } = s { Some(*length) } else { None })
        .expect("the journal records the extrude");
    assert!((length - PART_T).abs() < 0.1, "length {length} vs {PART_T}");
}

/// A side photo with no flat measurement held (a hand-built photo face) refuses by name and offers nothing.
#[test]
fn a_side_photo_with_no_first_photo_refuses_by_name() {
    use photo_core::fixture::{PART_T, PART_W, Spec, side_png};
    let mut vw = rectangle();
    let e = photo::side(&mut vw.view, &side_png(&Spec::side(), PART_W, PART_T), "side.png").expect_err("no first photo");
    assert!(e.contains("NoFirstPhoto"), "{e}");
    assert!(ph(&vw).offers.is_empty());
    assert!(ph(&vw).note.as_deref().unwrap_or("").starts_with("refused side NoFirstPhoto"), "{:?}", ph(&vw).note);
}

// ---- VUGINTENTTHICKNESS (SR557): the extrude judged by the side photo's thickness ----

/// The statement with its `length=` token replaced.
fn with_length(stmt: &str, length: f64) -> String {
    stmt.split_whitespace().map(|t| if t.starts_with("length=") { format!("length={length}") } else { t.to_string() }).collect::<Vec<_>>().join(" ")
}

/// The fixture part (50 x 30 x 6): the side photo's extrude accepts and the thickness is held; an extrude 2 mm long
/// (the outline alone cannot tell) refuses by Residual; a wrong side photo (a 9 mm side of the same length) re-holds the
/// thickness and Enter on the 6 mm extrude then refuses by Residual and applies nothing.
#[test]
fn a_wrong_side_photo_refuses_the_extrude_by_its_thickness() {
    use photo_core::fixture::{PART_H, PART_T, PART_W, Spec, side_raised_png};
    let mut vw = opened();
    photo::side(&mut vw.view, &side_raised_png(&Spec::side_raised(), PART_W, PART_T, PART_H), "side.png").expect("the side photo measures");
    assert!(matches!(ph(&vw).offers[0].verdict, Verdict::Accept { .. }), "{:?}", ph(&vw).offers[0].verdict);
    match &ph(&vw).thickness {
        Some(doc_core::Measurement::Thickness { mm, .. }) => assert!((mm - PART_T).abs() < 0.1, "held {mm}"),
        t => panic!("no thickness held: {t:?}"),
    }
    let stmt = ph(&vw).offers[0].proposal.stmt.clone();
    assert!(photo::offer(&mut vw.view, vec![proposal(with_length(&stmt, PART_T + 2.0))]));
    assert!(matches!(ph(&vw).offers[1].verdict, Verdict::Refuse { why: Reason::Residual { .. } }), "{:?}", ph(&vw).offers[1].verdict);
    photo::side(&mut vw.view, &side_raised_png(&Spec::side_raised(), PART_W, 9.0, PART_H), "wrong.png").expect("the wrong side photo measures");
    match &ph(&vw).thickness {
        Some(doc_core::Measurement::Thickness { mm, .. }) => assert!((mm - 9.0).abs() < 0.3, "held {mm}"),
        t => panic!("no thickness held: {t:?}"),
    }
    assert_eq!(ph(&vw).focus, Some(0));
    assert!(key(&mut vw, "Return"));
    assert!(matches!(ph(&vw).offers[0].verdict, Verdict::Refuse { why: Reason::Residual { .. } }), "{:?}", ph(&vw).offers[0].verdict);
    assert_eq!(ph(&vw).offers[0].state, State::Open, "never applied");
    assert!(vw.view.design.face.is_some(), "the sketch is still open");
    assert!(vw.view.design.body.is_none(), "no body");
    assert!(ph(&vw).note.as_deref().unwrap_or("").starts_with("refused residual"), "{:?}", ph(&vw).note);
}

// ---- VUGINTENTUNDOSET (SR563): an accepted statement set is ONE undo step ----

/// The rectangle face: a point and a distance naming it share one why (doc_core's one set); Enter on the set enters
/// both statements as ONE journal snapshot, one `u` puts the document back to the sketch before the set, one `U`
/// brings both statements back.
#[test]
fn a_set_enters_whole_and_undoes_in_one_step() {
    let mut vw = rectangle();
    let set = |stmt: &str| Proposal { stmt: stmt.into(), confidence: 0.9, why: "a datum point".into() };
    let before = photo::doc_of(face(&vw));
    let ents = face(&vw).sketch.entities().len();
    assert!(photo::offer(&mut vw.view, vec![set("point v3 sketch=s x=10 y=0"), set("distance v4 sketch=s a=e0 b=v3 d=10")]));
    assert_eq!(ph(&vw).focus, Some(0));
    assert!(key(&mut vw, "Return"));
    assert!(ph(&vw).offers.iter().all(|o| o.state == State::Applied), "{:?}", ph(&vw).note);
    assert!(ph(&vw).note.as_deref().unwrap_or("").starts_with("applied set n=2"), "{:?}", ph(&vw).note);
    let j = &vw.view.design.journal;
    assert_eq!(j.doc.stmts.len(), before.stmts.len() + 2, "both statements entered");
    assert_eq!(j.doc.stmts[..before.stmts.len()], before.stmts[..], "behind the sketch");
    let entered = j.doc.clone();
    let (undo, _) = j.depth();
    assert_eq!(face(&vw).sketch.entities().len(), ents + 1, "the point is in the face");
    // ONE `u`: the whole set out.
    assert!(key(&mut vw, "u"));
    assert_eq!(vw.view.design.journal.doc, before, "one undo puts the document back to before the set");
    assert_eq!(vw.view.design.journal.depth(), (undo - 1, 1));
    assert_eq!(face(&vw).sketch.entities().len(), ents, "the point is out of the face");
    // ONE `U`: the whole set back.
    assert!(key(&mut vw, "U"));
    assert_eq!(vw.view.design.journal.doc, entered, "one redo brings both statements back");
    assert_eq!(vw.view.design.journal.depth(), (undo, 0));
    assert_eq!(face(&vw).sketch.entities().len(), ents + 1);
}

// ---- VUGFACEPHOTOSIDEPARALLAX (SR569): the side photo through the raised-edge correction ----

/// The fixture part standing on its 50 x 6 face (its top PART_H above the paper) under the 400 mm camera: the face reads
/// 6.0 within 0.1 with the camera from the side lattice (`camera= .. lattice`); the square camera reads 6.0 with the
/// person's 400 mm (`camera=400 mm set`) and, without it, refuses NoCamera by name and offers nothing.
#[test]
fn a_raised_side_photo_reads_its_thickness_and_names_the_camera() {
    use photo_core::fixture::{PART_H, PART_T, PART_W, Spec, side_raised_png};
    let held = |vw: &Viewer| match &ph(vw).thickness {
        Some(doc_core::Measurement::Thickness { mm, .. }) => *mm,
        t => panic!("no thickness held: {t:?}"),
    };
    let mut vw = opened();
    photo::side(&mut vw.view, &side_raised_png(&Spec::side_raised(), PART_W, PART_T, PART_H), "raised.png").expect("the raised side photo measures");
    let mm = held(&vw);
    assert!((mm - PART_T).abs() < 0.1, "held {mm} vs {PART_T}");
    let o = &ph(&vw).offers[0];
    assert!(o.proposal.why.contains("camera=") && o.proposal.why.contains("lattice"), "{}", o.proposal.why);
    assert!(matches!(o.verdict, Verdict::Accept { .. }), "{:?}", o.verdict);
    let note = ph(&vw).note.as_deref().unwrap_or("").to_string();
    assert!(note.contains("camera=") && note.ends_with("mm lattice"), "{note}");

    let square = side_raised_png(&Spec::side_square(), PART_W, PART_T, PART_H);
    let mut vw = opened();
    photo::side_camera(&mut vw.view, &square, "square.png", Some(400.0)).expect("the square photo measures with camera_mm");
    let mm = held(&vw);
    assert!((mm - PART_T).abs() < 0.1, "held {mm} vs {PART_T}");
    let note = ph(&vw).note.as_deref().unwrap_or("").to_string();
    assert!(note.contains("camera=400 mm set"), "{note}");

    let mut vw = opened();
    let e = photo::side(&mut vw.view, &square, "square.png").expect_err("a square camera with no camera_mm");
    assert!(e.contains("NoCamera"), "{e}");
    assert!(ph(&vw).offers.is_empty(), "nothing offered");
    assert!(ph(&vw).note.as_deref().unwrap_or("").starts_with("refused side NoCamera"), "{:?}", ph(&vw).note);
}

// ---- VUGINTENTSETFACE (SR570): a statement set is ONE card; re-verified in the journal's names after a panel delete ----

fn datum(stmt: &str, why: &str) -> Proposal {
    Proposal { stmt: stmt.into(), confidence: 0.9, why: why.into() }
}

/// The panel's delete of the row `id` (its dependants with it), the regenerated face and body swapped in (journal::key's road).
fn delete_row(vw: &mut Viewer, id: &str) {
    let d = &mut vw.view.design;
    let i = d.journal.doc.stmts.iter().position(|s| s.id() == Some(id)).expect("the row");
    let r = d.journal.delete(i).expect("the delete regenerates");
    d.face = r.face;
    d.body = r.body;
}

/// The rectangle face: a point and a distance naming it share one why (doc_core's one set) and are painted as ONE card
/// (both statements listed, the term named); Tab stops on it once; ONE Escape rejects the whole; offered again, ONE
/// Enter enters the whole.
#[test]
fn a_set_is_one_card_entered_by_one_enter_and_refused_by_one_escape() {
    let mut vw = rectangle();
    let set = || vec![datum("point v3 sketch=s x=10 y=0", "a datum point"), datum("distance v4 sketch=s a=e0 b=v3 d=10", "a datum point")];
    let ents = face(&vw).sketch.entities().len();
    assert!(photo::offer(&mut vw.view, set()));
    assert!(ph(&vw).offers.iter().all(|o| o.set.is_some_and(|m| m.key == 0)), "{:?}", ph(&vw).offers);
    let fr = raster::render(&vw.model, &vw.view);
    let gs = photo::glyphs(&vw.view);
    assert_eq!(gs.len(), 1, "one card: {gs:?}");
    let card = gs[0].card.as_ref().expect("a card");
    assert_eq!(card.lines.len(), 2);
    assert!(card.lines[0].starts_with("POINT V3") && card.lines[1].starts_with("DISTANCE V4"), "{:?}", card.lines);
    assert_eq!(card.term, doc_core::Term::Outline); // VUGINTENTTERMTYPE (SR577): the typed term
    assert!(matches!(card.verdict, Verdict::Accept { .. }), "{:?}", card.verdict);
    assert!(gs[0].text.starts_with("SET N=2 A DATUM POINT") && gs[0].text.ends_with("TERM=OUTLINE"), "{}", gs[0].text);
    assert!(drawn(&fr, &gs[0]));
    // Tab: the card is one stop.
    assert_eq!(ph(&vw).focus, Some(0));
    key(&mut vw, "Tab");
    assert_eq!(ph(&vw).focus, Some(0), "a card is one stop");
    // ONE Escape: the whole set rejected, nothing entered.
    assert!(key(&mut vw, "Escape"));
    assert!(ph(&vw).offers.iter().all(|o| o.state == State::Rejected), "{:?}", ph(&vw).offers);
    assert!(ph(&vw).note.as_deref().unwrap_or("").starts_with("rejected set n=2 a datum point"), "{:?}", ph(&vw).note);
    assert!(photo::glyphs(&vw.view).is_empty());
    assert_eq!(face(&vw).sketch.entities().len(), ents);
    // Offered again: ONE Enter enters both.
    assert!(photo::offer(&mut vw.view, set()));
    assert_eq!(ph(&vw).focus, Some(2));
    assert!(key(&mut vw, "Return"));
    assert!(ph(&vw).offers[2..].iter().all(|o| o.state == State::Applied), "{:?}", ph(&vw).note);
    assert!(ph(&vw).note.as_deref().unwrap_or("").starts_with("applied set n=2"), "{:?}", ph(&vw).note);
    assert_eq!(face(&vw).sketch.entities().len(), ents + 1, "the point is in the face");
}

/// A stray point e0 before the rectangle's corners e1..e4 (sides e5..e8), the journal recorded. Two sets are offered;
/// the panel deletes e0, so the face's slots renumber (doc_of's e1 is now the (49.7, 0) corner) while the journal keeps
/// its names: Enter on the first set still enters, re-verified in the journal's names (SR563 refused it by name). The
/// panel then deletes e3, the corner the second set names: Enter refuses it by that name and nothing enters.
#[test]
fn a_set_verified_before_a_panel_delete_enters_in_the_journal_names() {
    let mut vw = rectangle();
    let mut sk = Sketch::new();
    let _stray = sk.add_point(20.0, 10.0);
    let c = [(0.0, 0.0), (49.7, 0.0), (49.7, 30.2), (0.0, 30.2)];
    let p: Vec<EntityId> = c.iter().map(|&(x, y)| sk.add_point(x, y)).collect();
    for i in 0..4 {
        sk.add_line(p[i], p[(i + 1) % 4]).expect("a line");
    }
    vw.view.design.face = Some(SketchFace::over(world_planes()[0].1, sk));
    {
        let d = &mut vw.view.design;
        d.journal.record(d.face.as_ref(), None);
    }
    assert!(photo::offer(
        &mut vw.view,
        vec![
            datum("point v3 sketch=s x=10 y=0", "a datum point"),
            datum("distance v4 sketch=s a=e1 b=v3 d=10", "a datum point"),
            datum("point v5 sketch=s x=49.7 y=20", "a second datum"),
            datum("distance v6 sketch=s a=e3 b=v5 d=10.2", "a second datum"),
        ]
    ));
    assert!(ph(&vw).offers.iter().all(|o| o.set.is_some() && matches!(o.verdict, Verdict::Accept { .. })), "{:?}", ph(&vw).offers);
    // The panel deletes the stray point: the journal keeps e1..e8, the face's dense slots read e0..e7 again.
    delete_row(&mut vw, "e0");
    assert!(vw.view.design.journal.doc.stmts.iter().all(|s| s.id() != Some("e0")));
    assert!(photo::doc_of(face(&vw)).stmts.iter().any(|s| s.id() == Some("e0")), "the slots renumbered");
    assert_eq!(ph(&vw).focus, Some(0));
    assert!(key(&mut vw, "Return"));
    assert!(ph(&vw).note.as_deref().unwrap_or("").starts_with("applied set n=2 a datum point"), "{:?}", ph(&vw).note);
    assert!(ph(&vw).offers[..2].iter().all(|o| o.state == State::Applied));
    assert!(ph(&vw).offers[2..].iter().all(|o| o.state == State::Open));
    let v4 = vw.view.design.journal.doc.stmts.iter().find(|s| s.id() == Some("v4")).expect("v4 entered");
    assert!(v4.refs().contains(&"e1"), "the distance names the journal's e1: {v4:?}");
    // The new point is 10 from the (0, 0) corner (slot 0 after the delete), the slot the offer named.
    let sk = &face(&vw).sketch;
    let last = sk.entities().iter().rposition(|e| matches!(e, Entity::Point { .. })).expect("a point") as u32;
    assert!((dist(sk, 0, last) - 10.0).abs() < 1e-6, "{}", dist(sk, 0, last));
    // A true conflict: the panel deletes e3 (its sides go with it); Enter on the second set is refused by that name.
    delete_row(&mut vw, "e3");
    let before = vw.view.design.journal.doc.clone();
    assert_eq!(ph(&vw).focus, Some(2));
    assert!(key(&mut vw, "Return"));
    let note = ph(&vw).note.clone().unwrap_or_default();
    assert!(note.starts_with("refused lex") && note.contains("e3"), "{note}");
    assert!(ph(&vw).offers[2..].iter().all(|o| o.state == State::Open));
    assert_eq!(vw.view.design.journal.doc, before, "nothing entered");
}

// ---- VUGINTENTSINGLERENAMED (SR577): a single proposal re-verified in the journal's names after a panel delete ----

/// SR570's face (a stray point e0 before the rectangle's corners e1..e4, sides e5..e8), the journal recorded. A lone
/// distance on the journal's e1-e2 (the 49.7 side) is offered; the panel deletes e0, so doc_of's e1 is now the (49.7, 0)
/// corner where d=50 would be a far residual: Enter still enters it, verified and entered in the journal's names. A
/// second single names the journal's e3; the panel deletes e3: Enter refuses it by that name and nothing enters.
#[test]
fn a_single_verified_before_a_panel_delete_enters_in_the_journal_names() {
    let mut vw = rectangle();
    let mut sk = Sketch::new();
    let _stray = sk.add_point(20.0, 10.0);
    let c = [(0.0, 0.0), (49.7, 0.0), (49.7, 30.2), (0.0, 30.2)];
    let p: Vec<EntityId> = c.iter().map(|&(x, y)| sk.add_point(x, y)).collect();
    for i in 0..4 {
        sk.add_line(p[i], p[(i + 1) % 4]).expect("a line");
    }
    vw.view.design.face = Some(SketchFace::over(world_planes()[0].1, sk));
    {
        let d = &mut vw.view.design;
        d.journal.record(d.face.as_ref(), None);
    }
    assert!(photo::offer(&mut vw.view, vec![proposal("distance v4 sketch=s a=e1 b=e2 d=50".into())]));
    assert!(ph(&vw).offers[0].set.is_none(), "a single, not a card");
    assert!(matches!(ph(&vw).offers[0].verdict, Verdict::Accept { .. }), "{:?}", ph(&vw).offers[0].verdict);
    // The panel deletes the stray point: the journal keeps e1..e8, the face's dense slots read e0..e7 again.
    delete_row(&mut vw, "e0");
    assert!(photo::doc_of(face(&vw)).stmts.iter().any(|s| s.id() == Some("e0")), "the slots renumbered");
    assert_eq!(ph(&vw).focus, Some(0));
    assert!(key(&mut vw, "Return"));
    assert!(ph(&vw).note.as_deref().unwrap_or("").starts_with("applied distance v4"), "{:?}", ph(&vw).note);
    assert_eq!(ph(&vw).offers[0].state, State::Applied);
    let v4 = vw.view.design.journal.doc.stmts.iter().find(|s| s.id() == Some("v4")).expect("v4 entered");
    assert!(v4.refs().contains(&"e1") && v4.refs().contains(&"e2"), "the distance names the journal's e1 and e2: {v4:?}");
    // The (0, 0) and (49.7, 0) corners are slots 0 and 1 after the delete: the side the offer named is exactly 50.
    assert!((dist(&face(&vw).sketch, 0, 1) - 50.0).abs() < 1e-6, "{}", dist(&face(&vw).sketch, 0, 1));
    // A true conflict: a single on the top side, offered in the face's names e2-e3 (the journal's e3-e4), then the panel
    // deletes the journal's e3 (its sides go with it); Enter refuses it by that name.
    assert!(photo::offer(&mut vw.view, vec![proposal("distance v5 sketch=s a=e2 b=e3 d=50".into())]));
    assert!(ph(&vw).offers[1].named.contains("a=e3 b=e4"), "captured in the journal's names: {}", ph(&vw).offers[1].named);
    delete_row(&mut vw, "e3");
    let before = vw.view.design.journal.doc.clone();
    assert_eq!(ph(&vw).focus, Some(1));
    assert!(key(&mut vw, "Return"));
    let note = ph(&vw).note.clone().unwrap_or_default();
    assert!(note.starts_with("refused") && note.contains("e3"), "{note}");
    assert_eq!(ph(&vw).offers[1].state, State::Open);
    assert_eq!(vw.view.design.journal.doc, before, "nothing entered");
}

// ---- VUGFACEPHOTOCAMERAPREF (SR576): the camera's datum from Principia's store ----

/// A store holding `vug.photo.camera_mm = 400.0` (written through Principia's PrefStore, the schema's check): the face
/// opened under it reads the square-camera side photo 6.0 within 0.1 with NO argument, the corner `camera=400 mm set`;
/// the same photo under the default prefs still refuses NoCamera by name.
#[test]
fn a_stored_camera_height_sets_the_side_camera_with_no_argument() {
    use photo_core::fixture::{PART_H, PART_T, PART_W, Spec, side_raised_png};
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("vug-photo-prefs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    principia::PrefStore::empty(&dir).set(photo::PREF_NS, "photo.camera_mm", bandy::PrefValue::Float(400.0)).expect("the store takes the camera height");
    let prefs = photo::PhotoPrefs::load_from(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(prefs, photo::PhotoPrefs { camera_mm: Some(400.0), principal_px: None });

    let square = side_raised_png(&Spec::side_square(), PART_W, PART_T, PART_H);
    let mut vw = Viewer::open_photo_with(&photo_core::fixture::photo_png(), "fixture", (W, H), Preset::Una, photo::PITCH_MM, prefs).expect("the fixture measures");
    photo::side(&mut vw.view, &square, "square.png").expect("the square photo measures with the stored camera height");
    let mm = match &ph(&vw).thickness {
        Some(doc_core::Measurement::Thickness { mm, .. }) => *mm,
        t => panic!("no thickness held: {t:?}"),
    };
    let note = ph(&vw).note.as_deref().unwrap_or("").to_string();
    eprintln!("photo: pref {note}");
    assert!((mm - PART_T).abs() < 0.1, "held {mm} vs {PART_T}");
    assert!(note.contains("camera=400 mm set"), "{note}");

    let mut vw = opened();
    let e = photo::side(&mut vw.view, &square, "square.png").expect_err("no stored camera height");
    assert!(e.contains("NoCamera"), "{e}");
}


// ---- VUGPHOTOCROPMETA (SR582): the photo's command road writes the camera's datum ----

/// VUGPHOTOCROPMETA (SR582): `camera 400` on the photo's command road writes `vug.photo.camera_mm` through Principia's
/// PrefStore (the store reads 400 back) and the open photo takes it: the square-camera side photo then reads 6.0 within
/// 0.1 with the corner `camera=400 mm set`; `pp 250 320` stores `250,320`; `camera x` is the usage, nothing stored.
#[test]
fn a_camera_line_writes_the_store_and_the_next_side_reads_it() {
    use photo_core::fixture::{PART_H, PART_T, PART_W, Spec, side_raised_png};
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("vug-photo-command-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut vw = opened();
    assert_eq!(photo::command_in(&mut vw.view, &dir, "camera 400"), Ok(true));
    let stored = ph(&vw).note.clone().unwrap_or_default();
    assert_eq!(stored, "pref camera=400 mm stored");
    assert_eq!(photo::PhotoPrefs::load_from(&dir).camera_mm, Some(400.0));
    assert_eq!(photo::command_in(&mut vw.view, &dir, "camera x"), Err(photo::PREF_USAGE.to_string()));

    // The next side read takes the stored height.
    let square = side_raised_png(&Spec::side_square(), PART_W, PART_T, PART_H);
    photo::side(&mut vw.view, &square, "square.png").expect("the square photo measures with the stored camera height");
    let mm = match &ph(&vw).thickness {
        Some(doc_core::Measurement::Thickness { mm, .. }) => *mm,
        t => panic!("no thickness held: {t:?}"),
    };
    let note = ph(&vw).note.as_deref().unwrap_or("").to_string();
    eprintln!("photo: pref-face {stored} | {note}");
    assert!((mm - PART_T).abs() < 0.1, "held {mm} vs {PART_T}");
    assert!(note.contains("camera=400 mm set"), "{note}");

    assert_eq!(photo::command_in(&mut vw.view, &dir, "pp 250 320"), Ok(true));
    assert_eq!(ph(&vw).note.as_deref(), Some("pref pp=250,320 stored"));
    let prefs = photo::PhotoPrefs::load_from(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(prefs, photo::PhotoPrefs { camera_mm: Some(400.0), principal_px: Some(Vec2::new(250.0, 320.0)) });
    assert_eq!(ph(&vw).prefs, prefs);
}

// ---- VUGINTENTSETFACESCROLL (SR583): a card taller than the sheet pages; a single extrude in the journal's names ----

/// The rectangle face on a sheet 148 px tall (the listed head at y = 42: eight 12 px rows under it): a chain of six
/// points along the bottom side, each with its distance to the one before (one why, one connected set: twelve lines),
/// is ONE card paged by B1119's rule: seven lines and `... 5 MORE`; PageDown shows the last seven and `... 5 BACK`, a
/// second PageDown does not move, PageUp returns to the top; the pages cover every line; Tab stays one stop; ONE Enter
/// enters all twelve.
#[test]
fn a_card_taller_than_the_sheet_pages_and_enters_whole() {
    let mut vw = rectangle();
    vw.view.resize(W, 148);
    let why = "a chain of datum points";
    let mut set = Vec::new();
    let mut prev = "e0".to_string();
    for k in 0..6u32 {
        let (pt, d) = (format!("v{}", 3 + 2 * k), format!("v{}", 4 + 2 * k));
        set.push(datum(&format!("point {pt} sketch=s x={} y=0", 5 * (k + 1)), why));
        set.push(datum(&format!("distance {d} sketch=s a={prev} b={pt} d=5"), why));
        prev = pt;
    }
    let ents = face(&vw).sketch.entities().len();
    assert!(photo::offer(&mut vw.view, set));
    assert!(ph(&vw).offers.iter().all(|o| o.set.is_some_and(|m| m.key == 0) && matches!(o.verdict, Verdict::Accept { .. })), "{:?}", ph(&vw).offers);
    let card = |vw: &Viewer| {
        let gs = photo::glyphs(&vw.view);
        assert_eq!(gs.len(), 1, "one card: {gs:?}");
        gs[0].card.clone().expect("a card")
    };
    let c = card(&vw);
    assert_eq!(c.lines.len(), 12);
    assert_eq!((c.first, c.fit), (0, 7), "eight rows: seven lines and the more line");
    let rows = c.rows();
    assert_eq!(rows.len(), 8);
    assert_eq!(rows[..7], c.lines[..7]);
    assert_eq!(rows[7], "... 5 MORE");
    let _ = raster::render(&vw.model, &vw.view);
    let mut seen: Vec<String> = rows[..7].to_vec();
    // PageDown: the last page, full.
    assert!(key(&mut vw, "Page_Down"));
    let c = card(&vw);
    assert_eq!(c.first, 5);
    let rows = c.rows();
    assert_eq!(rows[..7], c.lines[5..]);
    assert_eq!(rows[7], "... 5 BACK");
    seen.extend(rows[..7].iter().cloned());
    assert!(c.lines.iter().all(|l| seen.contains(l)), "the pages cover every line");
    // A second PageDown does not move; PageUp is back at the top.
    key(&mut vw, "Page_Down");
    assert_eq!(card(&vw).first, 5);
    assert!(key(&mut vw, "Page_Up"));
    assert_eq!(card(&vw).first, 0);
    // Tab: still one stop.
    key(&mut vw, "Tab");
    assert_eq!(ph(&vw).focus, Some(0), "a card is one stop");
    // ONE Enter: all twelve.
    assert!(key(&mut vw, "Return"));
    assert!(ph(&vw).offers.iter().all(|o| o.state == State::Applied), "{:?}", ph(&vw).note);
    assert!(ph(&vw).note.as_deref().unwrap_or("").starts_with("applied set n=12"), "{:?}", ph(&vw).note);
    assert_eq!(face(&vw).sketch.entities().len(), ents + 6, "six points in the face");
}

/// SR570's face (a stray point e0 before the rectangle's corners e1..e4, sides e5..e8), the journal recorded. A lone
/// extrude of the profile (its `outer=` the profile's first edge slot) is offered; the panel deletes e0, so the face's
/// slots renumber while the journal keeps its names: Enter still enters it, judged in the journal's names and entered by
/// SR547's pull road, and the journal's extrude names the same outer it was offered on.
#[test]
fn a_single_extrude_offered_before_a_panel_delete_enters_in_the_journal_names() {
    let mut vw = rectangle();
    let mut sk = Sketch::new();
    let _stray = sk.add_point(20.0, 10.0);
    let c = [(0.0, 0.0), (49.7, 0.0), (49.7, 30.2), (0.0, 30.2)];
    let p: Vec<EntityId> = c.iter().map(|&(x, y)| sk.add_point(x, y)).collect();
    for i in 0..4 {
        sk.add_line(p[i], p[(i + 1) % 4]).expect("a line");
    }
    vw.view.design.face = Some(SketchFace::over(world_planes()[0].1, sk));
    {
        let d = &mut vw.view.design;
        d.journal.record(d.face.as_ref(), None);
    }
    let outer = format!("e{}", face(&vw).profiles[0].outer.edges[0].src().0);
    assert!(photo::offer(&mut vw.view, vec![proposal(format!("extrude x0 sketch=s outer={outer} dir=[0 0 1] length=6"))]));
    assert!(ph(&vw).offers[0].set.is_none(), "a single, not a card");
    assert!(matches!(ph(&vw).offers[0].verdict, Verdict::Accept { .. }), "{:?}", ph(&vw).offers[0].verdict);
    // The panel deletes the stray point: the journal keeps its names, the face's dense slots renumber.
    delete_row(&mut vw, "e0");
    assert!(photo::doc_of(face(&vw)).stmts.iter().any(|s| s.id() == Some("e0")), "the slots renumbered");
    assert_eq!(ph(&vw).focus, Some(0));
    assert!(key(&mut vw, "Return"));
    assert!(ph(&vw).note.as_deref().unwrap_or("").starts_with("applied extrude x0"), "{:?}", ph(&vw).note);
    assert_eq!(ph(&vw).offers[0].state, State::Applied);
    assert!(vw.view.design.face.is_none(), "the sketch closed with its extrude");
    assert!(vw.view.design.body.is_some(), "the extrude regenerated a body");
    let x = vw.view.design.journal.doc.stmts.iter().find(|s| matches!(s, doc_core::Stmt::Extrude { .. })).expect("the extrude entered");
    assert!(matches!(x, doc_core::Stmt::Extrude { outer: o, .. } if *o == outer), "the journal's outer is the one offered: {x:?}");
}

// ---- VUGINTENTSETFACELISTROWS (SR589): the listed column reserves the paged height; the more line and the wheel page ----

/// SR583's chain of six datum points (twelve lines, one why): one connected set, ONE card.
fn chain() -> Vec<Proposal> {
    let why = "a chain of datum points";
    let mut set = Vec::new();
    let mut prev = "e0".to_string();
    for k in 0..6u32 {
        let (pt, d) = (format!("v{}", 3 + 2 * k), format!("v{}", 4 + 2 * k));
        set.push(datum(&format!("point {pt} sketch=s x={} y=0", 5 * (k + 1)), why));
        set.push(datum(&format!("distance {d} sketch=s a={prev} b={pt} d=5"), why));
        prev = pt;
    }
    set
}

/// The rectangle face on a sheet 148 px tall: the twelve-line card listed at y = 42 shows seven lines and the more line
/// (eight rows), so the lone listed proposal after it sits at the PAGED height, 30 + 12 * (1 + 8 + 1) = 150, not at the
/// twelve lines' 198.
#[test]
fn a_glyph_after_a_paged_card_sits_at_the_paged_height() {
    let mut vw = rectangle();
    vw.view.resize(W, 148);
    let mut ps = chain();
    ps.push(datum("point v40 sketch=s x=20 y=10", "a lone datum point"));
    assert!(photo::offer(&mut vw.view, ps));
    let gs = photo::glyphs(&vw.view);
    assert_eq!(gs.len(), 2, "the card and the lone point: {gs:?}");
    let c = gs[0].card.as_ref().expect("a card");
    assert_eq!((c.lines.len(), c.fit, c.rows().len()), (12, 7, 8));
    assert_eq!(gs[0].at, (6.0, 42.0));
    assert_eq!(gs[1].at, (6.0, 150.0), "the list below moves by the shown rows");
}

/// The same card: a Left press on `... 5 MORE` (the eighth row, y 138..150 from x 14) turns the page as PageDown does
/// (`... 5 BACK`); a press on the BACK line returns to the top (B1119: 0 from the last page). The wheel over the card
/// turns one page per event and does not zoom; a press away from every more line is not photo's.
#[test]
fn a_click_on_the_more_line_turns_the_page() {
    use vug_view::view::Button;
    let mut vw = rectangle();
    vw.view.resize(W, 148);
    assert!(photo::offer(&mut vw.view, chain()));
    let card = |vw: &Viewer| photo::glyphs(&vw.view)[0].card.clone().expect("a card");
    assert_eq!(card(&vw).rows()[7], "... 5 MORE");
    assert!(vw.view.press(&vw.model, Button::Left, 0, 16.0, 140.0));
    let c = card(&vw);
    assert_eq!(c.first, 5);
    assert_eq!(c.rows()[7], "... 5 BACK");
    assert_eq!(ph(&vw).focus, Some(0), "the card is focused");
    assert!(vw.view.press(&vw.model, Button::Left, 0, 16.0, 140.0));
    assert_eq!(card(&vw).first, 0, "back to the top");
    // The wheel over the card: one page per notch, the camera untouched.
    let cam = vw.view.camera;
    vw.view.cursor = (16.0, 60.0);
    assert!(vw.view.wheel(&vw.model, 1.0));
    assert_eq!(card(&vw).first, 5);
    assert!(vw.view.wheel(&vw.model, -1.0));
    assert_eq!(card(&vw).first, 0);
    assert_eq!(vw.view.camera, cam, "the wheel over a card does not zoom");
    // Off the card the wheel zooms and a press is not photo's.
    vw.view.cursor = (400.0, 20.0);
    assert!(vw.view.wheel(&vw.model, 1.0));
    assert_ne!(vw.view.camera, cam, "the wheel off the card zooms");
    assert!(photo::press(&mut vw.view, Button::Left, 400.0, 20.0).is_none());
    assert_eq!(card(&vw).first, 0);
}

// ---- VUGPHOTOPREFFACEWIRE (SR588): the flags' lines on the command road, and clearing a pref ----

/// VUGPHOTOPREFFACEWIRE (SR588): `camera 400` (the line main.rs's `--camera 400` hands photo::command) stores the height
/// and the square-camera side reads `camera=400 mm set`; `camera clear` unsets `vug.photo.camera_mm` in Principia's store
/// and the open photo (corner `pref camera cleared`), so the tilted raised side then reads `... mm lattice` and the square
/// side refuses NoCamera by name; a second clear is `Ok(false)` (`(was unset)`); `pp 250 320` then `pp clear` leaves no
/// principal point in the store or the photo.
#[test]
fn a_camera_flag_line_sets_the_side_and_clear_returns_it_to_the_lattice() {
    use photo_core::fixture::{PART_H, PART_T, PART_W, Spec, side_raised_png};
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("vug-photo-clear-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let square = side_raised_png(&Spec::side_square(), PART_W, PART_T, PART_H);
    let raised = side_raised_png(&Spec::side_raised(), PART_W, PART_T, PART_H);
    let mut vw = opened();
    assert_eq!(photo::command_in(&mut vw.view, &dir, "camera 400"), Ok(true));
    let stored = ph(&vw).note.clone().unwrap_or_default();
    assert_eq!(stored, "pref camera=400 mm stored");
    photo::side(&mut vw.view, &square, "square.png").expect("the square photo measures with the stored camera height");
    let set = ph(&vw).note.clone().unwrap_or_default();
    assert!(set.contains("camera=400 mm set"), "{set}");

    assert_eq!(photo::command_in(&mut vw.view, &dir, "camera clear"), Ok(true));
    let cleared = ph(&vw).note.clone().unwrap_or_default();
    assert_eq!(cleared, "pref camera cleared");
    assert_eq!(ph(&vw).prefs.camera_mm, None, "the open photo dropped it");
    assert_eq!(photo::PhotoPrefs::load_from(&dir).camera_mm, None, "the store dropped it");

    photo::side(&mut vw.view, &raised, "raised.png").expect("the raised side photo measures with the lattice's camera");
    let lattice = ph(&vw).note.clone().unwrap_or_default();
    eprintln!("photo: pref-clear {stored} | {set} | {cleared} | {lattice}");
    assert!(lattice.contains("camera=") && lattice.ends_with("mm lattice"), "{lattice}");
    let e = photo::side(&mut vw.view, &square, "square.png").expect_err("a square camera with the height cleared");
    assert!(e.contains("NoCamera"), "{e}");

    assert_eq!(photo::command_in(&mut vw.view, &dir, "camera clear"), Ok(false));
    assert_eq!(ph(&vw).note.as_deref(), Some("pref camera cleared (was unset)"));

    assert_eq!(photo::command_in(&mut vw.view, &dir, "pp 250 320"), Ok(true));
    assert_eq!(ph(&vw).prefs.principal_px, Some(Vec2::new(250.0, 320.0)));
    assert_eq!(photo::command_in(&mut vw.view, &dir, "pp clear"), Ok(true));
    assert_eq!(ph(&vw).note.as_deref(), Some("pref pp cleared"));
    let prefs = photo::PhotoPrefs::load_from(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(prefs, photo::PhotoPrefs::default());
    assert_eq!(ph(&vw).prefs, prefs);
}

// ---- VUGINTENTSETFACEANCHOREDOVERLAP (SR596): anchored cards stack; the more line's hover cue ----

/// The rectangle face: two statement sets (two whys), each head naming the corner e0 first, so both cards are drawn at
/// e0's anchor. The second is pushed down to the first's foot (head + two member rows = 36 px), x unmoved; the two
/// painted rects are disjoint and both cards are drawn.
#[test]
fn two_anchored_cards_at_one_anchor_stack_without_overlap() {
    let mut vw = rectangle();
    let ps = vec![
        datum("line v10 sketch=s a=e0 b=e1", "the bottom side"),
        datum("horizontal v11 sketch=s line=v10", "the bottom side"),
        datum("line v12 sketch=s a=e0 b=e3", "the left side"),
        datum("vertical v13 sketch=s line=v12", "the left side"),
    ];
    assert!(photo::offer(&mut vw.view, ps));
    let gs = photo::glyphs(&vw.view);
    assert_eq!(gs.len(), 2, "two cards: {gs:?}");
    let (a, b) = (gs[0].card.as_ref().expect("a card"), gs[1].card.as_ref().expect("a card"));
    assert_eq!((a.rows().len(), b.rows().len()), (2, 2), "{a:?} {b:?}");
    assert_ne!(gs[0].at.0, 6.0, "anchored, not listed: {:?}", gs[0].at);
    assert_eq!(gs[1].at, (gs[0].at.0, gs[0].at.1 + 36.0), "the second card sits at the first's foot");
    let (ra, rb) = (photo::glyph_rect(&gs[0]), photo::glyph_rect(&gs[1]));
    let overlap = ra.0 < rb.0 + rb.2 && rb.0 < ra.0 + ra.2 && ra.1 < rb.1 + rb.3 && rb.1 < ra.1 + ra.3;
    assert!(!overlap, "disjoint: {ra:?} {rb:?}");
    let fr = raster::render(&vw.model, &vw.view);
    assert!(drawn(&fr, &gs[0]) && drawn(&fr, &gs[1]));
}

/// SR589's chain on a 148 px sheet (the card focused, its `... 5 MORE` at x 14.., y 138..150): the pointer moving onto
/// the more line asks for a frame and the line paints ACTIVE; moving within it asks none; moving off it asks for one
/// frame and the line paints INK (not the focused card's ACTIVE), no ACTIVE pixel left in it.
#[test]
fn the_more_line_paints_active_under_the_cursor_and_ink_off_it() {
    use vug_view::sketch::{ACTIVE, INK};
    let mut vw = rectangle();
    vw.view.resize(W, 148);
    assert!(photo::offer(&mut vw.view, chain()));
    assert_eq!(ph(&vw).focus, Some(0));
    assert_eq!(photo::glyphs(&vw.view)[0].card.as_ref().expect("a card").rows()[7], "... 5 MORE");
    let any = |fr: &raster::Frame, col: u32| (138..148u32).any(|y| (14..94u32).any(|x| fr.pixel(x, y) == raster::rgba(col)));
    vw.view.motion(400.0, 20.0);
    assert!(vw.view.motion(16.0, 140.0), "entering the more line asks for a frame");
    let fr = raster::render(&vw.model, &vw.view);
    assert!(any(&fr, ACTIVE), "the more line paints ACTIVE under the cursor");
    assert!(!photo::hover(&mut vw.view, (16.0, 140.0), 18.0, 141.0), "moving within it asks no frame of photo");
    assert!(photo::hover(&mut vw.view, (16.0, 140.0), 400.0, 20.0), "photo's hover sees the leave");
    assert!(vw.view.motion(400.0, 20.0), "leaving the more line asks for a frame");
    let fr = raster::render(&vw.model, &vw.view);
    assert!(any(&fr, INK), "off the cursor the more line paints INK");
    assert!(!any(&fr, ACTIVE), "and no ACTIVE pixel is left in it");
}

// ---- VUGFACEPHOTOPREFLINE (SR595): the typed `:` line on the photo's command road ----

/// Type `text` into the open `:` line one key at a time.
fn typed(vw: &mut Viewer, text: &str) {
    for c in text.chars() {
        key(vw, &c.to_string());
    }
}

/// VUGFACEPHOTOPREFLINE (SR595): over a photo with no map `:` opens (the photo's sketch face no longer shuts it);
/// `:camera x` Return refuses PREF_USAGE and the line stays open; `:camera 400` Return runs photo::command_in in the
/// line's store folder: the line closes as the last, the corner reads `pref camera=400 mm stored`, the open photo and
/// Principia's store hold 400; `:camera clear` Return unsets it from both (corner `pref camera cleared`).
#[test]
fn a_typed_camera_line_sets_the_pref_and_the_corner_reads_it() {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("vug-photo-line-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut vw = opened();
    vw.view.entry.store = Some(dir.clone());
    assert!(vw.view.deviation.is_none() && vw.view.design.face.is_some(), "a photo with its sketch face, no map");
    assert!(key(&mut vw, ":"), "`:` opens over a photo");
    assert!(vw.view.entry.open);
    typed(&mut vw, "camera x");
    assert!(key(&mut vw, "Return"));
    assert_eq!(vw.view.entry.refused.as_deref(), Some(photo::PREF_USAGE));
    assert!(vw.view.entry.open, "a refused line stays open");
    assert_eq!(photo::PhotoPrefs::load_from(&dir).camera_mm, None, "nothing stored");

    assert!(key(&mut vw, "Escape"));
    assert!(key(&mut vw, ":"));
    typed(&mut vw, "camera 400");
    assert!(key(&mut vw, "Return"));
    assert!(!vw.view.entry.open, "a run line closes");
    assert_eq!(vw.view.entry.last.as_deref(), Some("camera 400"));
    let corner = ph(&vw).note.clone().unwrap_or_default();
    eprintln!("photo: pref-line {corner}");
    assert_eq!(corner, "pref camera=400 mm stored");
    assert_eq!(ph(&vw).prefs.camera_mm, Some(400.0));
    assert_eq!(photo::PhotoPrefs::load_from(&dir).camera_mm, Some(400.0));

    assert!(key(&mut vw, ":"));
    typed(&mut vw, "camera clear");
    assert!(key(&mut vw, "Return"));
    assert_eq!(ph(&vw).note.as_deref(), Some("pref camera cleared"));
    assert_eq!(ph(&vw).prefs.camera_mm, None);
    let prefs = photo::PhotoPrefs::load_from(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(prefs.camera_mm, None);
}

// ---- VUGINTENTSETFACELISTEDOVERLAP (SR601): an anchored glyph over the listed column moves right; its leader ----

/// The rectangle's side drawn leftmost on screen: its line entity and its midpoint's screen point (photo's anchor of a
/// statement naming that line). Its leader runs right through the listed text, clear of every sketch edge.
fn left_side(vw: &Viewer) -> (u32, (f32, f32)) {
    let f = face(vw);
    let sk = &f.sketch;
    (4..8u32)
        .filter_map(|e| {
            let Entity::Line { a, b } = sk.entity(EntityId(e))? else { return None };
            let (p, q) = (sk.point(PointRef::Point(*a))?, sk.point(PointRef::Point(*b))?);
            let (x, y) = f.to_screen(&vw.view, Vec2::new(0.5 * (p.x + q.x), 0.5 * (p.y + q.y)))?;
            Some((e, (x as f32, y as f32)))
        })
        .min_by(|l, r| (l.1).0.partial_cmp(&(r.1).0).unwrap_or(core::cmp::Ordering::Equal))
        .expect("the rectangle's sides are on screen")
}

/// `n` listed singles (no `e<n>` named, one why each).
fn listed(n: usize) -> Vec<Proposal> {
    (0..n).map(|j| datum(&format!("point v{} sketch=s x={} y=10", 40 + j, 10 + j), &format!("a lone datum point {j}"))).collect()
}

/// The rectangle face: enough listed singles that the column's rows pass the left side's midpoint, then one single
/// anchored there. Its natural place overlaps the column, so it sits at the column's right edge + 6 px at its own
/// height, and its leader runs from its cross back to the anchor, painted INK (the midpoint pixel, over the listed text
/// it crosses); a listed glyph has no leader.
#[test]
fn an_anchored_glyph_over_the_listed_column_moves_right_with_a_leader() {
    use vug_view::sketch::INK;
    let mut vw = rectangle();
    vw.view.resize(W + 96, H); // VUGINTENTSETFACEPUSHEDGE (SR606): a sheet wide enough that the right push fits (at W the push passes the edge, SR606's down case); x shifts by half the growth, so the left side stays over the column
    let (e, (sx, sy)) = left_side(&vw);
    let natural = (sx + 6.0, sy - 14.0);
    let n = ((natural.1 - 30.0) / 12.0).ceil().max(0.0) as usize + 1;
    let mut ps = listed(n);
    ps.push(datum(&format!("horizontal v12 sketch=s line=e{e}"), "the left side"));
    assert!(photo::offer(&mut vw.view, ps));
    let gs = photo::glyphs(&vw.view);
    assert_eq!(gs.len(), n + 1, "{n} listed singles and the anchored one: {gs:?}");
    let (list, g) = (&gs[..n], &gs[n]);
    assert!(list.iter().all(|l| l.at.0 == 6.0), "listed: {list:?}");
    let col = list.iter().map(photo::glyph_rect).fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |c, r| (c.0.min(r.0), c.1.min(r.1), c.2.max(r.0 + r.2), c.3.max(r.1 + r.3)));
    let r = photo::glyph_rect(g);
    let before = (natural.0, natural.1, r.2, r.3);
    assert!(before.0 < col.2 && col.0 < before.0 + before.2 && before.1 < col.3 && col.1 < before.1 + before.3, "the natural place overlaps the column: {before:?} column {col:?}");
    assert!((g.at.0 - (col.2 + 6.0)).abs() < 1e-3 && (g.at.1 - natural.1).abs() < 1e-3, "right to the column's edge + 6, y kept: {:?} column {col:?}", g.at);
    let (a, b) = photo::leader(&vw.view, g).expect("a pushed glyph has a leader");
    assert_eq!((a, b), ((g.at.0 - 6.0, g.at.1 + 14.0), (sx, sy)), "from its cross to its anchor");
    assert!(list.iter().all(|l| photo::leader(&vw.view, l).is_none()), "a listed glyph has no leader");
    let fr = raster::render(&vw.model, &vw.view);
    let (mx, my) = ((0.5 * (a.0 + b.0)) as u32, (0.5 * (a.1 + b.1)) as u32);
    let ink = raster::rgba(INK);
    let ys = my.saturating_sub(1)..=my + 1;
    assert!(ys.clone().any(|y| fr.pixel(mx, y) == ink), "the leader's midpoint ({mx}, {ys:?}) paints INK");
}

/// The same anchored single offered alone: nothing to push, so it keeps its anchor's place and has no leader.
#[test]
fn a_glyph_not_moved_has_no_leader() {
    let mut vw = rectangle();
    let (e, (sx, sy)) = left_side(&vw);
    assert!(photo::offer(&mut vw.view, vec![datum(&format!("horizontal v12 sketch=s line=e{e}"), "the left side")]));
    let gs = photo::glyphs(&vw.view);
    assert_eq!(gs.len(), 1, "{gs:?}");
    assert_eq!(gs[0].at, (sx + 6.0, sy - 14.0), "nothing to push: its anchor's place");
    assert_eq!(photo::leader(&vw.view, &gs[0]), None, "an unmoved glyph has no leader");
}

// ---- VUGINTENTSETFACEPUSHEDGE (SR606): a right push past the sheet's edge goes down to the column's foot; the corner names a glyph wider than the sheet ----

/// SR601's rectangle face on a sheet `grow` px wider than W, with `n` listed singles and one single anchored at the left
/// side's midpoint whose id is `id`: the anchored glyph (last) and the listed column's bounding rect (x0, y0, x1, y1).
fn anchored_over_column(id: &str, n: usize, grow: u32) -> (Viewer, vug_view::photo::ProposalGlyph, (f32, f32, f32, f32)) {
    let mut vw = rectangle();
    vw.view.resize(W + grow, H);
    let (e, _) = left_side(&vw);
    let mut ps = listed(n);
    ps.push(datum(&format!("horizontal {id} sketch=s line=e{e}"), "the left side"));
    assert!(photo::offer(&mut vw.view, ps));
    let gs = photo::glyphs(&vw.view);
    assert_eq!(gs.len(), n + 1, "{n} listed singles and the anchored one: {gs:?}");
    let col = gs[..n].iter().map(photo::glyph_rect).fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |c, r| (c.0.min(r.0), c.1.min(r.1), c.2.max(r.0 + r.2), c.3.max(r.1 + r.3)));
    let g = gs[n].clone();
    (vw, g, col)
}

/// The listed count that puts the column's rows past the left side's midpoint (SR601's), and the width of the anchored
/// glyph with the short id `v12` (its text grows 8 px per id character).
fn column_and_base() -> (usize, (f32, f32), f32) {
    let vw = rectangle();
    let (_, (sx, sy)) = left_side(&vw);
    let natural = (sx + 6.0, sy - 14.0);
    let n = ((natural.1 - 30.0) / 12.0).ceil().max(0.0) as usize + 1;
    let (_, g, _) = anchored_over_column("v12", n, 0);
    (n, natural, photo::glyph_rect(&g).2)
}

/// A wide anchored single over the listed column: a right push to the column's edge + 6 would carry its right edge past
/// the sheet, so it goes DOWN to the column's foot with its anchor's x; its rect stays inside the sheet, its leader runs
/// from its cross to the anchor in INK, and the corner names nothing.
#[test]
fn a_wide_anchored_glyph_over_the_column_is_pushed_down_to_its_foot() {
    use vug_view::sketch::INK;
    let (n, natural, w) = column_and_base();
    let (_, _, col) = anchored_over_column("v12", n, 0);
    // The v12 glyph is 408 px at W (its text carries the refusal): it fits at its anchor's x and not after a right
    // push only on a sheet W + g with 2 (natural.0 + w - W) <= g < column edge + 6 + w - W (x shifts by g / 2).
    let (lo, hi) = ((2.0 * (natural.0 + w - W as f32)).max(0.0), col.2 + 6.0 + w - W as f32);
    assert!(lo + 2.0 < hi, "a sheet width holds case (a): growth {lo}..{hi} (w {w}, natural {natural:?}, column {col:?})");
    let grow = (0.5 * (lo + hi)).round() as u32;
    let sheet = (W + grow) as f32;
    let (vw, g, col) = anchored_over_column("v12", n, grow);
    let (_, (sx, sy)) = left_side(&vw);
    let natural = (sx + 6.0, sy - 14.0);
    let r = photo::glyph_rect(&g);
    assert!(natural.0 + r.2 <= sheet, "it fits at its anchor's x: {natural:?} w {} sheet {sheet}", r.2);
    assert!(col.2 + 6.0 + r.2 > sheet, "a right push would pass the sheet's edge: column {col:?} w {} sheet {sheet}", r.2);
    assert!((g.at.0 - natural.0).abs() < 1e-3 && (g.at.1 - col.3).abs() < 1e-3, "down to the column's foot, x unmoved: {:?} column {col:?}", g.at);
    assert!(r.0 >= 0.0 && r.0 + r.2 <= sheet, "the rect inside the sheet: {r:?} sheet {sheet}");
    let (a, b) = photo::leader(&vw.view, &g).expect("a pushed glyph has a leader");
    assert_eq!(a, (g.at.0 - 6.0, g.at.1 + 14.0), "the leader starts at its cross");
    let fr = raster::render(&vw.model, &vw.view);
    let (mx, my) = ((0.5 * (a.0 + b.0)) as u32, (0.5 * (a.1 + b.1)) as u32);
    let ink = raster::rgba(INK);
    let xs = mx.saturating_sub(1)..=mx + 1;
    assert!(xs.clone().any(|x| fr.pixel(x, my) == ink), "the leader's midpoint ({xs:?}, {my}) paints INK");
    assert_eq!(photo::wide_note(&vw.view), None, "a glyph the sheet holds is not named");
}

/// An anchored single wider than the sheet itself: no push can hold it, so it keeps its anchor's x (down to the
/// column's foot), is clipped, and the corner's second row says `glyph wider than the sheet: <id>` once, in INK.
#[test]
fn a_glyph_wider_than_the_sheet_keeps_its_anchor_x_and_the_corner_says_so() {
    use vug_view::sketch::INK;
    let (n, natural, w0) = column_and_base();
    let extra = ((W as f32 - w0) / 8.0).ceil() as usize + 2;
    let id = format!("v12{}", "a".repeat(extra));
    let (vw, g, col) = anchored_over_column(&id, n, 0);
    assert!(photo::glyph_rect(&g).2 > W as f32, "wider than the sheet: {:?}", photo::glyph_rect(&g));
    assert!((g.at.0 - natural.0).abs() < 1e-3, "its anchor's x: {:?} natural {natural:?}", g.at);
    assert!((g.at.1 - col.3).abs() < 1e-3, "down to the column's foot: {:?} column {col:?}", g.at);
    assert_eq!(photo::wide_note(&vw.view).as_deref(), Some(format!("glyph wider than the sheet: {id}").as_str()));
    let fr = raster::render(&vw.model, &vw.view);
    let ink = raster::rgba(INK);
    assert!((30..40u32).any(|y| (6..46u32).any(|x| fr.pixel(x, y) == ink)), "the corner's second row paints INK");
}

// ---- VUGINTENTSETFACEPUSHFOOT (SR617): a down push past the sheet's bottom lifts above its obstacle; a glyph taller than the free band is clipped and the corner says so ----

/// The rectangle's side drawn lowest on screen (left_side's shape, the largest screen y): its line entity and its
/// midpoint's screen point. A leader rising from it runs inside the rectangle, clear of every sketch edge.
fn low_side(vw: &Viewer) -> (u32, (f32, f32)) {
    let f = face(vw);
    let sk = &f.sketch;
    (4..8u32)
        .filter_map(|e| {
            let Entity::Line { a, b } = sk.entity(EntityId(e))? else { return None };
            let (p, q) = (sk.point(PointRef::Point(*a))?, sk.point(PointRef::Point(*b))?);
            let (x, y) = f.to_screen(&vw.view, Vec2::new(0.5 * (p.x + q.x), 0.5 * (p.y + q.y)))?;
            Some((e, (x as f32, y as f32)))
        })
        .max_by(|l, r| (l.1).1.partial_cmp(&(r.1).1).unwrap_or(core::cmp::Ordering::Equal))
        .expect("the rectangle's sides are on screen")
}

/// SR601's rectangle face on a (W + `grow`, `h`) sheet with `n` listed singles (SR601's [`listed`]) then `k` singles
/// anchored at the low side's midpoint (one why each, so no set forms; ids v20..), offered: the viewer, the anchored
/// glyph's natural place (sx + 6, sy - 14) and the glyphs (the listed first). SR596 stacks the anchored ones down from
/// the first: the j-th fits at y0 + 12 j while its 10 px glyph ends on the sheet.
fn stacked(h: u32, grow: u32, n: usize, k: usize) -> (Viewer, (f32, f32), Vec<vug_view::photo::ProposalGlyph>) {
    let mut vw = rectangle();
    vw.view.resize(W + grow, h);
    let (e, (sx, sy)) = low_side(&vw);
    let mut ps = listed(n);
    ps.extend((0..k).map(|j| datum(&format!("horizontal v{} sketch=s line=e{e}", 20 + j), &format!("the low side {j}"))));
    assert!(photo::offer(&mut vw.view, ps));
    let gs = photo::glyphs(&vw.view);
    assert_eq!(gs.len(), n + k, "{n} listed and {k} anchored singles: {gs:?}");
    (vw, (sx + 6.0, sy - 14.0), gs)
}

/// The singles that fit stacked from y0 on a sheet `h` tall (the j-th at y0 + 12 j, its glyph 10 px).
fn fitting(h: u32, y0: f32) -> usize {
    let f = ((h as f32 - 10.0 - y0) / 12.0).floor();
    if f < 0.0 { 0 } else { f as usize + 1 }
}

/// On a half-height sheet: as many singles at the low side as fit stacked, plus one. The last one's SR596 push would
/// carry it past the sheet's bottom, so it lifts over the stack to the first's top less its own height, x unmoved: its
/// rect inside the sheet and clear of every other, its leader from its cross to the anchor painted INK, no corner word.
#[test]
fn a_glyph_pushed_past_the_bottom_lifts_above_its_obstacle() {
    use vug_view::sketch::INK;
    let h = H / 2;
    let (_, (_, y0), _) = stacked(h, 200, 0, 1);
    let m = fitting(h, y0);
    assert!(m >= 1 && y0 - 12.0 >= 42.0, "the case exists on this sheet: y0 {y0}, {m} fit, the foot 42");
    let (vw, natural, gs) = stacked(h, 200, 0, m + 1);
    assert!(y0 + 12.0 * m as f32 + 10.0 > h as f32, "a drop under the stack passes the bottom: y0 {y0}, {m} fit, h {h}");
    let g = &gs[m];
    assert!((g.at.0 - natural.0).abs() < 1e-3 && (g.at.1 - (natural.1 - 12.0)).abs() < 1e-3, "above the stack, x unmoved: {:?} natural {natural:?}", g.at);
    let r = photo::glyph_rect(g);
    assert!(r.0 >= 0.0 && r.1 >= 0.0 && r.0 + r.2 <= (W + 200) as f32 && r.1 + r.3 <= h as f32, "the rect inside the sheet: {r:?}");
    for o in &gs[..m] {
        let q = photo::glyph_rect(o);
        assert!(!(r.0 < q.0 + q.2 && q.0 < r.0 + r.2 && r.1 < q.1 + q.3 && q.1 < r.1 + r.3), "clear of {q:?}: {r:?}");
    }
    let (a, b) = photo::leader(&vw.view, g).expect("a lifted glyph has a leader");
    assert_eq!(a, (g.at.0 - 6.0, g.at.1 + 14.0), "the leader starts at its cross");
    let fr = raster::render(&vw.model, &vw.view);
    let (mx, my) = ((0.5 * (a.0 + b.0)) as u32, (0.5 * (a.1 + b.1)) as u32);
    let ink = raster::rgba(INK);
    let xs = mx.saturating_sub(1)..=mx + 1;
    assert!(xs.clone().any(|x| fr.pixel(x, my) == ink), "the leader's midpoint ({xs:?}, {my}) paints INK");
    assert!(drawn(&fr, g));
    assert_eq!(photo::wide_note(&vw.view), None, "a glyph the sheet holds is not named");
}

/// A listed column of `n` singles (its foot, the running row's, at 42 + 12 n) on a sheet 9 px taller than the foot, so
/// the free band is shorter than a single's 10 px glyph; the low side's stack sits clear right of the column (the sheet
/// grown until it does: x shifts by half the growth). The stack's last single, pushed past the bottom, lifts over the
/// stack onto the column's foot, x unmoved, is clipped there, and the corner's second row says `glyph taller than the
/// free band: <id>` in INK. (With nothing listed no case exists: on this face y0 = 0.616 h - 14, so for every sheet
/// 43..51 px tall the drop lands at 36.5..41.4, above the corner foot 42, and the glyph stays at its drop y.)
#[test]
fn a_glyph_taller_than_the_free_band_is_clipped_and_the_corner_says_so() {
    use vug_view::sketch::INK;
    let mut tried = Vec::new();
    let case = (1..=12usize).find_map(|n| {
        let (foot, h) = (42.0 + 12.0 * n as f32, 51 + 12 * n as u32);
        let mut grow = 200;
        for _ in 0..4 {
            let (_, natural, gs) = stacked(h, grow, n, 1);
            let right = gs[..n].iter().map(|l| { let r = photo::glyph_rect(l); r.0 + r.2 }).fold(0.0f32, f32::max);
            if right + 6.0 < natural.0 {
                let m = fitting(h, natural.1);
                tried.push((n, h, grow, natural.1, m));
                return (natural.1 >= 0.0 && m >= 1 && natural.1 + 12.0 * m as f32 > foot).then_some((n, foot, h, grow, m));
            }
            grow += 2 * (right + 6.0 - natural.0).ceil() as u32 + 16;
        }
        None
    });
    let (n, foot, h, grow, m) = case.unwrap_or_else(|| panic!("a listed column holds the case: (n, h, grow, y0, fit) {tried:?}"));
    let (vw, natural, gs) = stacked(h, grow, n, m + 1);
    let col = gs[..n].iter().map(photo::glyph_rect).fold((f32::MAX, f32::MAX, f32::MIN, f32::MIN), |c, r| (c.0.min(r.0), c.1.min(r.1), c.2.max(r.0 + r.2), c.3.max(r.1 + r.3)));
    assert!((col.3 - foot).abs() < 1e-3, "the column's foot: {col:?} foot {foot}");
    let g = &gs[n + m];
    assert!((g.at.0 - natural.0).abs() < 1e-3 && (g.at.1 - foot).abs() < 1e-3, "lifted onto the column's foot, x unmoved: {:?} natural {natural:?} foot {foot}", g.at);
    let r = photo::glyph_rect(g);
    assert!(r.1 + r.3 - 2.0 > h as f32, "clipped at the bottom: {r:?} h {h}");
    assert_eq!(photo::wide_note(&vw.view).as_deref(), Some(format!("glyph taller than the free band: v{}", 20 + m).as_str()));
    let fr = raster::render(&vw.model, &vw.view);
    let ink = raster::rgba(INK);
    assert!((30..40u32).any(|y| (6..46u32).any(|x| fr.pixel(x, y) == ink)), "the corner's second row paints INK");
}

// ---- VUGINTENTSETFACEPUSHFULL (SR629): a band full of placed rects: the glyph goes back to the walk's last free y and the corner says so ----

/// No listed column (the foot, the corner's second row's, at 42) on a sheet short enough that the low side's stack
/// starts within one row under the foot (30 < y0 < 54): every y from the foot down to the stack overlaps a placed
/// single, and the sheet has no overflow road (it never pages or grows). The last single, pushed past the bottom, has
/// no free band: it goes back to the walk's last free y (under the stack, where SR596's push turned it), x unmoved,
/// clear of every placed rect, clipped, and the corner's second row says `glyph has no free band: <id>` in INK.
#[test]
fn a_glyph_in_a_full_band_stays_at_its_last_free_y_and_the_corner_says_so() {
    use vug_view::sketch::INK;
    let mut tried = Vec::new();
    let case = (52..=200u32).find_map(|h| {
        let (_, (_, y0), _) = stacked(h, 200, 0, 1);
        let m = fitting(h, y0);
        tried.push((h, y0, m));
        (y0 > 30.0 && y0 < 54.0 && m >= 1).then_some((h, y0, m))
    });
    let (h, y0, m) = case.unwrap_or_else(|| panic!("a sheet holds the full band: (h, y0, fit) {tried:?}"));
    let (vw, natural, gs) = stacked(h, 200, 0, m + 1);
    let g = &gs[m];
    let turn = y0 + 12.0 * m as f32;
    assert!((g.at.0 - natural.0).abs() < 1e-3 && (g.at.1 - turn).abs() < 1e-3, "back under the stack, x unmoved: {:?} natural {natural:?} turn {turn}", g.at);
    let r = photo::glyph_rect(g);
    assert!(r.1 + r.3 - 2.0 > h as f32, "clipped at the bottom: {r:?} h {h}");
    for o in &gs[..m] {
        let q = photo::glyph_rect(o);
        assert!(!(r.0 < q.0 + q.2 && q.0 < r.0 + r.2 && r.1 < q.1 + q.3 && q.1 < r.1 + r.3), "clear of {q:?}: {r:?}");
    }
    assert_eq!(photo::wide_note(&vw.view).as_deref(), Some(format!("glyph has no free band: v{}", 20 + m).as_str()));
    let fr = raster::render(&vw.model, &vw.view);
    let ink = raster::rgba(INK);
    assert!((30..40u32).any(|y| (6..46u32).any(|x| fr.pixel(x, y) == ink)), "the corner's second row paints INK");
}

// ---- VUGFACEPHOTOCROPSAVE (SR640): the face's crop and its save beside the photo (named tail block) ----

/// A fresh folder under the test target for one crop test.
fn crop_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("vug-photo-crop-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the test folder is made");
    dir
}

/// The fixture photo written as `fixture.png` in `dir` and opened by its path (the open road that knows the file).
fn crop_opened(dir: &std::path::Path) -> (Viewer, std::path::PathBuf) {
    let path = dir.join("fixture.png");
    std::fs::write(&path, photo_core::fixture::photo_png()).expect("the fixture is written");
    let vw = Viewer::open(path.to_str().expect("a UTF-8 path"), (W, H), Preset::Una).expect("the fixture opens by its path");
    (vw, path)
}

/// `:crop` Return on the photo's line.
fn crop_armed(vw: &mut Viewer) {
    assert!(key(vw, ":"), "`:` opens over a photo");
    typed(vw, "crop");
    assert!(key(vw, "Return"));
    assert!(!vw.view.entry.open, "a run line closes");
    assert!(ph(vw).crop.armed, "`:crop` arms the drag");
}

/// A Left drag from `a` to `b` (surface pixels): press, one motion, release.
fn crop_drag(vw: &mut Viewer, a: (f64, f64), b: (f64, f64)) {
    vw.input(&SMessage::BrowserButton { down: true, x: a.0, y: a.1, mods: 0, surface: 0 });
    vw.input(&SMessage::BrowserMouseMove(b.0, b.1, 0));
    vw.input(&SMessage::BrowserButton { down: false, x: b.0, y: b.1, mods: 0, surface: 0 });
}

/// VUGFACEPHOTOCROPSAVE (SR640): `:crop`, a drag on the photo and Return save the CROPPED pixels beside the photo as
/// `fixture.crop.png`, byte for byte photo_core's `fixture::crop_png` of the same rectangle (its tEXt `vug:crop` pair the
/// frame `x,y,900,720`, read back by `crop_principal` as the uncropped centre less the offset); the original is untouched;
/// the face re-opens the saved file through the open road (title and corner `saved fixture.crop.png`) and shows the
/// cropped measurement (the outline `photo::open` measures on the saved bytes); a second keep onto an existing name is
/// refused by name and writes nothing.
#[test]
fn a_dragged_crop_saves_beside_the_photo_and_reopens_measured() {
    let dir = crop_dir("save");
    let (mut vw, path) = crop_opened(&dir);
    crop_armed(&mut vw);
    crop_drag(&mut vw, (40.0, 40.0), (600.0, 440.0));
    let [x, y, w, h] = photo::crop_rect(ph(&vw)).expect("the drag spans a rectangle");
    assert_eq!(ph(&vw).note.clone().unwrap_or_default(), format!("crop {x},{y} {w}x{h} px: Return keeps it, Esc cancels"));
    let full = pixel_core::decode(&photo_core::fixture::photo_png()).expect("the fixture decodes");
    assert!((w, h) != (full.width, full.height), "the drag cuts the photo: {x},{y} {w}x{h}");

    assert!(key(&mut vw, "Return"));
    let out = dir.join("fixture.crop.png");
    let saved = std::fs::read(&out).expect("the crop is saved beside the photo");
    assert_eq!(saved, photo_core::fixture::crop_png(&full, x, y, w, h), "the cropped pixels with their crop pair, as PNG");
    let img = pixel_core::decode(&saved).expect("the saved crop decodes");
    assert_eq!((img.width, img.height), (w, h));
    let pair = img.text.iter().find(|(k, _)| k == photo_core::CROP_KEY).map(|(_, v)| v.clone());
    assert_eq!(pair, Some(format!("{x},{y},{},{}", full.width, full.height)));
    let pp = photo_core::crop_principal(&img).expect("the saved crop states its principal point");
    assert_eq!((pp.x, pp.y), (full.width as f64 * 0.5 - x as f64, full.height as f64 * 0.5 - y as f64));
    assert_eq!(std::fs::read(&path).expect("the original reads"), photo_core::fixture::photo_png(), "the original is untouched");

    assert_eq!(vw.title, "fixture.crop.png", "the face re-opened the saved crop");
    assert_eq!(ph(&vw).note.as_deref(), Some("saved fixture.crop.png"));
    assert!(!ph(&vw).crop.armed);
    assert_eq!(ph(&vw).crop.source.as_deref(), Some(out.as_path()));
    assert_eq!((ph(&vw).image.width, ph(&vw).image.height), (w, h));
    let (_, _, direct) = photo::open(&saved, photo::PITCH_MM).expect("the saved crop measures");
    assert_eq!(ph(&vw).outline, direct.outline, "the face shows the cropped measurement");
    eprintln!("photo: crop-save rect={x},{y},{w}x{h} saved=fixture.crop.png principal=({},{}) outline={}", pp.x, pp.y, direct.outline.len());

    // The next crop of the saved photo would be fixture.crop.crop.png: an existing file there is refused by name.
    let taken = dir.join("fixture.crop.crop.png");
    std::fs::write(&taken, b"not ours").expect("the blocker is written");
    crop_armed(&mut vw);
    crop_drag(&mut vw, (40.0, 40.0), (600.0, 440.0));
    assert!(photo::crop_rect(ph(&vw)).is_some());
    assert!(key(&mut vw, "Return"));
    assert_eq!(ph(&vw).note.as_deref(), Some("refused crop: fixture.crop.crop.png exists"));
    assert_eq!(std::fs::read(&taken).expect("the blocker reads"), b"not ours", "never written over");
    assert!(ph(&vw).crop.armed, "a refusal keeps the crop armed");
    let _ = std::fs::remove_dir_all(&dir);
}

/// VUGFACEPHOTOCROPSAVE (SR640): while armed the dragged rectangle dims the sheet outside it (the house tint) and not its
/// centre; Escape cancels: disarmed, no rectangle, the corner `crop cancelled`, the frame below the corner rows back to
/// the pre-drag frame, and nothing written beside the photo (a later Return saves nothing).
#[test]
fn escape_cancels_the_crop_and_writes_nothing() {
    let dir = crop_dir("cancel");
    let (mut vw, _) = crop_opened(&dir);
    crop_armed(&mut vw);
    let (a, b) = ((40.0, 40.0), (600.0, 440.0));
    vw.input(&SMessage::BrowserMouseMove(a.0, a.1, 0));
    let before = raster::render(&vw.model, &vw.view);
    crop_drag(&mut vw, a, b);
    vw.input(&SMessage::BrowserMouseMove(a.0, a.1, 0));
    assert!(photo::crop_rect(ph(&vw)).is_some(), "the drag spans a rectangle");
    let changed = |f: &raster::Frame| (48..H).flat_map(|y| (0..W).map(move |x| (x, y))).filter(|&(x, y)| f.pixel(x, y) != before.pixel(x, y)).count();
    let masked = raster::render(&vw.model, &vw.view);
    assert!(changed(&masked) > 0, "the sheet outside the crop dims");
    assert_eq!(masked.pixel(W / 2, H / 2), before.pixel(W / 2, H / 2), "the crop's centre is not masked");

    assert!(key(&mut vw, "Escape"));
    assert!(!ph(&vw).crop.armed && photo::crop_rect(ph(&vw)).is_none(), "Escape disarms and drops the rectangle");
    assert_eq!(ph(&vw).note.as_deref(), Some("crop cancelled"));
    let after = raster::render(&vw.model, &vw.view);
    assert_eq!(changed(&after), 0, "the mask is gone");
    key(&mut vw, "Return");
    let names: Vec<_> = std::fs::read_dir(&dir).expect("the folder reads").filter_map(|e| e.ok()).map(|e| e.file_name()).collect();
    assert_eq!(names, vec![std::ffi::OsString::from("fixture.png")], "nothing saved beside the photo");
    assert_eq!(vw.title, "fixture.png");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- VUGFACEPHOTOCROPKEYPREF (SR645): the crop key beside `:crop`, and the saved crop's frame over a stale pref (named tail block) ----

/// VUGFACEPHOTOCROPKEYPREF (SR645): `r` arms the crop through the ONE arm fn `:crop` takes: two viewers on one photo, one
/// `:crop` Return, the other `r`, read the same corner word and the same crop state; `R` arms too; the key's drag spans
/// the rectangle and Escape cancels it by name; SHORTCUTS (the `--help` table) carries the `r` row naming `:crop`; with the
/// `:` line open `r` is the line's text, never the crop (the key sits under the line's own gate).
#[test]
fn the_r_key_arms_the_crop_as_the_crop_line_does() {
    let dir = crop_dir("key");
    let (mut line, _) = crop_opened(&dir);
    let (mut vw, path) = crop_opened(&dir);
    crop_armed(&mut line);
    assert!(!ph(&vw).crop.armed);
    assert!(key(&mut vw, "r"), "`r` arms the crop");
    let (a, b) = (&ph(&line).crop, &ph(&vw).crop);
    assert!(b.armed, "`r` arms the drag");
    assert_eq!((a.armed, a.from, a.to, a.dragging, a.reopen.clone()), (b.armed, b.from, b.to, b.dragging, b.reopen.clone()), "the same crop state as `:crop`");
    assert_eq!(b.source.as_deref(), Some(path.as_path()), "the photo's file is kept");
    assert_eq!(ph(&vw).note, ph(&line).note, "the same corner word as `:crop`");
    let corner = ph(&vw).note.clone().unwrap_or_default();
    eprintln!("photo: crop-key armed={} corner={corner}", b.armed);

    assert!(key(&mut vw, "Escape"));
    assert_eq!(ph(&vw).note.as_deref(), Some("crop cancelled"));
    assert!(key(&mut vw, "R"), "`R` arms the crop too");
    assert!(ph(&vw).crop.armed);
    assert_eq!(ph(&vw).note.as_deref(), Some(corner.as_str()));
    crop_drag(&mut vw, (40.0, 40.0), (600.0, 440.0));
    assert!(photo::crop_rect(ph(&vw)).is_some(), "the key's drag spans a rectangle");
    assert!(key(&mut vw, "Escape"));
    assert!(!ph(&vw).crop.armed && photo::crop_rect(ph(&vw)).is_none());
    assert_eq!(ph(&vw).note.as_deref(), Some("crop cancelled"));

    assert!(vug_view::SHORTCUTS.iter().any(|(k, what)| *k == "r" && what.contains(":crop")), "SHORTCUTS carries the crop key's row");

    assert!(key(&mut vw, ":"), "`:` opens over the photo");
    key(&mut vw, "r");
    assert_eq!(vw.view.entry.text, "r", "the open line takes `r` as text");
    assert!(!ph(&vw).crop.armed, "the key does not arm under the open line");
    assert!(key(&mut vw, "Escape"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// One open photo's measurement for the comparisons: its lens term and its outline.
fn measured(p: &Photo) -> (Option<f64>, Vec<Vec2>) {
    (p.first.as_ref().map(|m| m.k1), p.outline.clone())
}

/// VUGFACEPHOTOCROPKEYPREF (SR645): VUG-FACE-PHOTO-CROP-PREF closes by measurement: photo_core `principal_of` ranks the
/// photo's own `vug:crop` frame over the pref, and the face's save always writes one. The lens photo (k1 -0.08: the
/// principal point moves its measurement) opens under the pref `principal_px` 450,360 (the full frame's point, stale for
/// any crop); `r`, the rectangle 200,40 700x560, Return: `follow` re-opens `lens.crop.png` with the stale pref still
/// held, and it measures exactly as the saved bytes do with NO pref (the frame 200,40,900,720 states 250,320), while the
/// meta-free crop under the same pref measures otherwise (the pref is live without the frame). A crop of the crop
/// (`lens.crop.crop.png`, frame 210,50,900,720: SR640's composed offset) keeps the rank on its re-open.
#[test]
fn a_saved_crop_outranks_a_stale_principal_pref_on_reopen() {
    let dir = crop_dir("pref");
    let path = dir.join("lens.png");
    let spec = photo_core::fixture::Spec::lens();
    std::fs::write(&path, photo_core::fixture::png(&spec)).expect("the lens photo is written");
    let full = photo_core::fixture::render(&spec);
    let stale = Vec2::new(full.width as f64 * 0.5, full.height as f64 * 0.5);
    let prefs = photo::PhotoPrefs { camera_mm: None, principal_px: Some(stale) };
    let mut vw = Viewer::open_with(path.to_str().expect("a UTF-8 path"), (W, H), Preset::Una, prefs).expect("the lens photo opens by its path");

    let keep = |vw: &mut Viewer, from: (f64, f64), to: (f64, f64)| {
        assert!(key(vw, "r"), "`r` arms the crop");
        let crop = &mut vw.view.photo.as_mut().expect("a photo is open").crop;
        crop.from = Some(from);
        crop.to = Some(to);
        assert!(key(vw, "Return"));
    };
    keep(&mut vw, (200.0, 40.0), (900.0, 600.0));
    let out = dir.join("lens.crop.png");
    assert_eq!(vw.title, "lens.crop.png", "the face re-opened the saved crop");
    assert_eq!(ph(&vw).prefs.principal_px, Some(stale), "the stale pref is still held");
    let saved = std::fs::read(&out).expect("the crop is saved beside the photo");
    let img = pixel_core::decode(&saved).expect("the saved crop decodes");
    assert_eq!(photo_core::crop_frame(&img), Some([200, 40, full.width, full.height]));
    let meta = photo_core::crop_principal(&img).expect("the saved crop states its principal point");
    assert_eq!((meta.x, meta.y), (250.0, 320.0));
    let (_, _, bare) = photo::open(&saved, photo::PITCH_MM).expect("the saved crop measures with no pref");
    let reopened = measured(ph(&vw));
    assert_eq!(reopened, measured(&bare), "the written frame outranks the stale pref");

    let c = photo_core::fixture::crop(&full, 200, 40, 700, 560);
    let free = pixel_core::png::encode::encode_rgba(c.width, c.height, &c.rgba).expect("the meta-free crop encodes");
    let free_stale = photo::open_with(&free, photo::PITCH_MM, prefs).ok().map(|(_, _, p)| measured(&p));
    assert_ne!(free_stale, Some(reopened.clone()), "with no frame the stale pref moves the measurement");
    eprintln!(
        "photo: crop-pref stale={},{} meta={},{} k1={:?} reopened_k1={:?} free_stale_k1={:?}",
        stale.x, stale.y, meta.x, meta.y, measured(&bare).0, reopened.0, free_stale.as_ref().map(|m| m.0)
    );

    keep(&mut vw, (10.0, 10.0), (690.0, 550.0));
    let out2 = dir.join("lens.crop.crop.png");
    assert_eq!(vw.title, "lens.crop.crop.png", "the face re-opened the crop of the crop");
    assert_eq!(ph(&vw).prefs.principal_px, Some(stale));
    let saved2 = std::fs::read(&out2).expect("the crop of the crop is saved");
    let img2 = pixel_core::decode(&saved2).expect("it decodes");
    assert_eq!(photo_core::crop_frame(&img2), Some([210, 50, full.width, full.height]), "SR640's composed frame");
    let (_, _, bare2) = photo::open(&saved2, photo::PITCH_MM).expect("the crop of the crop measures with no pref");
    assert_eq!(measured(ph(&vw)), measured(&bare2), "the composed frame keeps the rank");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- VUGFACEPHOTOCROPPREFFRAC (SR648): a meta-free photo's principal pref carried into its crop (named tail block) ----

/// VUGFACEPHOTOCROPPREFFRAC (SR648): SR645's control case with the pref OFF the centre: the lens photo (no crop meta)
/// opens under the pref `principal_px` (446.5, 357.25), the full frame's centre less (3.5, 2.75); `r`, the rectangle 200,40
/// 700x560, Return: the saved pair carries the pref's point, `200,40,900,720,446.5,357.25`, read back as the pref less the
/// offset (246.5, 317.25), and the face's re-open measures exactly as the meta-free crop under that point, not as it
/// under the centre-derived (250, 320) a 4-field frame stated.
#[test]
fn a_meta_free_crop_carries_the_principal_pref_on_reopen() {
    let dir = crop_dir("preffrac");
    let path = dir.join("lens.png");
    let spec = photo_core::fixture::Spec::lens();
    std::fs::write(&path, photo_core::fixture::png(&spec)).expect("the lens photo is written");
    let full = photo_core::fixture::render(&spec);
    let pref = Vec2::new(full.width as f64 * 0.5 - 3.5, full.height as f64 * 0.5 - 2.75);
    let prefs = photo::PhotoPrefs { camera_mm: None, principal_px: Some(pref) };
    let mut vw = Viewer::open_with(path.to_str().expect("a UTF-8 path"), (W, H), Preset::Una, prefs).expect("the lens photo opens by its path");

    assert!(key(&mut vw, "r"), "`r` arms the crop");
    let crop = &mut vw.view.photo.as_mut().expect("a photo is open").crop;
    crop.from = Some((200.0, 40.0));
    crop.to = Some((900.0, 600.0));
    assert!(key(&mut vw, "Return"));
    let out = dir.join("lens.crop.png");
    assert_eq!(vw.title, "lens.crop.png", "the face re-opened the saved crop");
    let saved = std::fs::read(&out).expect("the crop is saved beside the photo");
    let img = pixel_core::decode(&saved).expect("the saved crop decodes");
    let pair = img.text.iter().find(|(k, _)| k == photo_core::CROP_KEY).map(|(_, v)| v.clone());
    assert_eq!(pair, Some(format!("200,40,{},{},{},{}", full.width, full.height, pref.x, pref.y)), "the pref's point carried");
    assert_eq!(photo_core::crop_frame(&img), Some([200, 40, full.width, full.height]));
    let meta = photo_core::crop_principal(&img).expect("the saved crop states its principal point");
    assert_eq!((meta.x, meta.y), (pref.x - 200.0, pref.y - 40.0));

    let c = photo_core::fixture::crop(&full, 200, 40, 700, 560);
    let free = pixel_core::png::encode::encode_rgba(c.width, c.height, &c.rgba).expect("the meta-free crop encodes");
    let at = |pp: Vec2| photo::open_with(&free, photo::PITCH_MM, photo::PhotoPrefs { camera_mm: None, principal_px: Some(pp) }).ok().map(|(_, _, p)| measured(&p));
    let reopened = measured(ph(&vw));
    let carried = at(meta);
    let centre = at(Vec2::new(full.width as f64 * 0.5 - 200.0, full.height as f64 * 0.5 - 40.0));
    assert_eq!(Some(reopened.clone()), carried, "the re-open measures about the pref's point less the offset");
    assert_ne!(Some(reopened.clone()), centre, "a 4-field frame's centre would measure otherwise");
    eprintln!(
        "photo: crop-pref-frac pref={},{} meta={},{} k1={:?} centre_k1={:?}",
        pref.x, pref.y, meta.x, meta.y, reopened.0, centre.as_ref().map(|m| m.0)
    );
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- VUGFACEPHOTOCROPDOC (SR649): the open document carried onto the cropped photo (named tail block) ----

/// The fixture opened by its path, `:crop` dragged (40, 40) -> (600, 440), a 5 mm line sketched at the paper point under
/// the crop's centre (H⁻¹ of its centre pixel) and recorded (the journal dirty), then Return: the viewer, the folder,
/// the rectangle, the document and the photo as they were before the keep.
fn crop_sketched(tag: &str) -> (Viewer, std::path::PathBuf, [u32; 4], doc_core::Doc, Photo, [Vec2; 2]) {
    let dir = crop_dir(tag);
    let (mut vw, _) = crop_opened(&dir);
    crop_armed(&mut vw);
    crop_drag(&mut vw, (40.0, 40.0), (600.0, 440.0));
    let r = photo::crop_rect(ph(&vw)).expect("the drag spans a rectangle");
    let Ok(inv) = photo_core::homography::invert(&ph(&vw).h) else { panic!("the photo's H inverts") };
    let q0 = photo_core::homography::apply(&inv, Vec2::new(r[0] as f64 + r[2] as f64 * 0.5, r[1] as f64 + r[3] as f64 * 0.5)).expect("the crop's centre on the paper");
    let q1 = Vec2::new(q0.x + 5.0, q0.y);
    let mut sk = face(&vw).sketch.clone();
    let (a, b) = (sk.add_point(q0.x, q0.y), sk.add_point(q1.x, q1.y));
    sk.add_line(a, b).expect("a line");
    {
        let d = &mut vw.view.design;
        d.face = Some(SketchFace::over(world_planes()[0].1, sk));
        d.journal.record(d.face.as_ref(), None);
    }
    assert!(vw.view.design.journal.dirty(), "the sketched line is unsaved");
    let doc = vw.view.design.journal.doc.clone();
    let before = ph(&vw).clone();
    assert!(key(&mut vw, "Return"));
    (vw, dir, r, doc, before, [q0, q1])
}

/// VUGFACEPHOTOCROPDOC (SR649): a line sketched over the photo survives the crop at the same paper mm. The keep no longer
/// refuses the unsaved document: the crop is saved (SR640's bytes), and the controller slides it under the KEPT face
/// instead of re-opening: the document is the same (the line's statements included) and still dirty, `save_to` is still
/// `fixture.vug`, the title names the crop with the unsaved mark, the outline is kept, and `Photo.h` is T(-x, -y) · H:
/// the line's paper points land on the old pixel less the offset and the photo's colour under them is the same.
#[test]
fn a_sketched_line_survives_the_crop_at_the_same_paper_mm() {
    let (vw, dir, [x, y, w, h], doc, before, line) = crop_sketched("doc");
    let out = dir.join("fixture.crop.png");
    let full = pixel_core::decode(&photo_core::fixture::photo_png()).expect("the fixture decodes");
    assert_eq!(std::fs::read(&out).expect("the crop is saved beside the photo"), photo_core::fixture::crop_png(&full, x, y, w, h));
    assert_eq!(vw.view.design.journal.doc, doc, "the document is carried, the line at the same paper mm");
    assert!(vw.view.design.journal.dirty(), "the journal stays dirty");
    assert_eq!(vw.title, "fixture.crop.png*", "the title names the crop, unsaved");
    assert_eq!(vw.save_to.as_deref(), Some(dir.join("fixture.vug").as_path()), "the document's own file is kept");
    assert_eq!(ph(&vw).note.as_deref(), Some("saved fixture.crop.png"));
    assert_eq!(ph(&vw).crop.source.as_deref(), Some(out.as_path()));
    assert_eq!((ph(&vw).image.width, ph(&vw).image.height), (w, h));
    assert_eq!(ph(&vw).outline, before.outline, "the measurement is kept in the document's paper frame");
    let mid = Vec2::new((line[0].x + line[1].x) * 0.5, (line[0].y + line[1].y) * 0.5);
    for q in [line[0], line[1], mid] {
        let p0 = photo_core::homography::apply(&before.h, q).expect("the old pixel");
        let p1 = photo_core::homography::apply(&ph(&vw).h, q).expect("the new pixel");
        assert!((p1.x - (p0.x - x as f64)).abs() < 1e-9 && (p1.y - (p0.y - y as f64)).abs() < 1e-9, "H moved by the offset: {p0:?} -> {p1:?}");
        let (c0, c1) = (before.sample([q.x, q.y]).expect("on the old photo"), ph(&vw).sample([q.x, q.y]).expect("on the crop"));
        assert!((0..3).all(|k| (c0[k] - c1[k]).abs() < 1e-3), "the same colour under the line: {c0:?} {c1:?}");
    }
    eprintln!("photo: crop-doc rect={x},{y},{w}x{h} carried stmts={} dirty=true saved=fixture.crop.png doc=fixture.vug", doc.stmts.len());
    let _ = std::fs::remove_dir_all(&dir);
}

/// VUGFACEPHOTOCROPDOC (SR649): the carried document then saves to its own file beside the ORIGINAL photo (`s` writes
/// doc_core's text to the kept `fixture.vug`; VUGDOCPHOTOREFCROP (SR660): its `photo` line names the crop
/// `fixture.crop.png` with the slid frame `h` the document is drawn in), clean, the title the crop's name; no `fixture.crop.vug` is
/// written and the original photo is untouched.
#[test]
fn the_carried_document_saves_beside_the_original_photo() {
    let (mut vw, dir, _, doc, _, _) = crop_sketched("docsave");
    assert!(key(&mut vw, "s"));
    let saved = std::fs::read_to_string(dir.join("fixture.vug")).expect("the document is saved beside the original photo");
    assert_eq!(saved, with_framed(&doc, "fixture.crop.png", &ph(&vw).h), "the carried document, the line's statements included, over the crop in its slid frame"); // VUGDOCPHOTOREFCROP (SR660)
    assert!(!vw.view.design.journal.dirty(), "saved");
    assert_eq!(vw.title, "fixture.crop.png");
    assert!(!dir.join("fixture.crop.vug").exists(), "the crop's name never becomes the document's");
    assert_eq!(std::fs::read(dir.join("fixture.png")).expect("the original reads"), photo_core::fixture::photo_png(), "the original is untouched");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- VUGDOCPHOTOREF (SR652): the document names its photo; a re-open lays the underlay (named tail block) ----

/// The text `s` writes for `doc` drawn over the photo `path` (relative): doc_core's writer with the `photo` statement
/// first, at the fixture's pitch.
fn with_photo(doc: &doc_core::Doc, path: &str) -> String {
    let mut stmts = vec![doc_core::Stmt::Photo { path: path.into(), pitch_mm: photo::PITCH_MM, h: None }]; // VUGDOCPHOTOREFCROP (SR660): no frame
    stmts.extend(doc.stmts.iter().cloned());
    doc_core::write(&doc_core::Doc { tol: doc.tol, stmts, lines: Vec::new() })
}

/// The fixture opened by its path in a fresh folder, a 5 mm line sketched at the paper point under the photo's centre
/// and recorded, then `s`: the viewer, the folder, the photo's path, the document and the photo as saved.
fn ref_saved(tag: &str) -> (Viewer, std::path::PathBuf, std::path::PathBuf, doc_core::Doc, Photo, [Vec2; 2]) {
    let dir = crop_dir(tag);
    let (mut vw, path) = crop_opened(&dir);
    let (iw, ih) = (ph(&vw).image.width as f64, ph(&vw).image.height as f64);
    let Ok(inv) = photo_core::homography::invert(&ph(&vw).h) else { panic!("the photo's H inverts") };
    let q0 = photo_core::homography::apply(&inv, Vec2::new(iw * 0.5, ih * 0.5)).expect("the photo's centre on the paper");
    let q1 = Vec2::new(q0.x + 5.0, q0.y);
    let mut sk = face(&vw).sketch.clone();
    let (a, b) = (sk.add_point(q0.x, q0.y), sk.add_point(q1.x, q1.y));
    sk.add_line(a, b).expect("a line");
    {
        let d = &mut vw.view.design;
        d.face = Some(SketchFace::over(world_planes()[0].1, sk));
        d.journal.record(d.face.as_ref(), None);
    }
    assert!(key(&mut vw, "s"), "`s` saves");
    assert!(!vw.view.design.journal.dirty(), "saved: {:?}", vw.view.design.journal.disk);
    let doc = vw.view.design.journal.doc.clone();
    let before = ph(&vw).clone();
    (vw, dir, path, doc, before, [q0, q1])
}

/// VUGDOCPHOTOREF (SR652): a line sketched over a photo opened by its path saves as doc_core's text with `photo
/// path="fixture.png" pitch=5` after the header (relative to the .vug's folder); `Viewer::open` of the .vug lays the
/// photo again (THE photo road, the same bytes measured to the same H) under the SAME document (the line at the same
/// paper mm, clean, the face the document's), the title the document's, the crop road on the photo's file, and the photo's
/// colour under the line is the one it was drawn over; a re-save writes the same bytes.
#[test]
fn a_sketched_line_saved_over_a_photo_reopens_with_the_underlay_at_the_same_paper_mm() {
    let (vw, dir, path, doc, before, line) = ref_saved("ref");
    let vug = dir.join("fixture.vug");
    let saved = std::fs::read_to_string(&vug).expect("the document is saved beside the photo");
    assert_eq!(saved, with_photo(&doc, "fixture.png"), "the photo line after the header, the line's statements after it");
    assert_eq!(saved.lines().nth(1), Some(format!("photo path=\"fixture.png\" pitch={}", doc_core::f64_text(photo::PITCH_MM)).as_str()));

    let mut back = Viewer::open(vug.to_str().expect("a UTF-8 path"), (W, H), Preset::Una).unwrap_or_else(|e| panic!("the document opens: {e}"));
    assert!(back.view.photo.is_some(), "the photo is laid under the document");
    assert_eq!(back.view.design.journal.doc, doc, "the same document: the line at the same paper mm");
    assert!(!back.view.design.journal.doc.stmts.iter().any(|s| matches!(s, doc_core::Stmt::Photo { .. })), "the photo line never enters the journal");
    assert!(!back.view.design.journal.dirty(), "opened clean");
    assert_eq!(back.title, "fixture.vug");
    assert_eq!(back.save_to.as_deref(), Some(vug.as_path()));
    assert_eq!(ph(&back).h, before.h, "the same bytes measure to the same paper frame");
    assert_eq!(ph(&back).crop.source.as_deref(), Some(path.as_path()), "the crop road on the photo's file");
    let mid = Vec2::new((line[0].x + line[1].x) * 0.5, (line[0].y + line[1].y) * 0.5);
    for q in [line[0], line[1], mid] {
        let (c0, c1) = (before.sample([q.x, q.y]).expect("on the photo"), ph(&back).sample([q.x, q.y]).expect("on the re-laid photo"));
        assert_eq!(c0, c1, "the same colour under the line at {q:?}");
    }
    assert_eq!(face(&back).sketch.entities().len(), face(&vw).sketch.entities().len(), "the face is the document's sketch (the line included), not a fresh measurement");

    back.save().expect("the re-save");
    assert_eq!(std::fs::read_to_string(&vug).expect("re-saved"), saved, "a re-save writes the same bytes: the photo line relative again");
    eprintln!("photo: doc-ref saved=fixture.vug photo=fixture.png pitch={} stmts={} h-equal=true", photo::PITCH_MM, doc.stmts.len());
    let _ = std::fs::remove_dir_all(&dir);
}

/// VUGDOCPHOTOREF (SR652): the photo gone from its path, the document still opens — bare (no photo), the same
/// statements, clean — with one word in the corner, `photo: missing <path>`; never a refusal. The reference is kept:
/// a save still writes the photo line.
#[test]
fn a_document_whose_photo_is_missing_opens_bare_and_names_it() {
    let (_, dir, path, doc, _, _) = ref_saved("refmissing");
    let vug = dir.join("fixture.vug");
    let saved = std::fs::read_to_string(&vug).expect("saved");
    std::fs::remove_file(&path).expect("the photo is removed");

    let mut back = Viewer::open(vug.to_str().expect("a UTF-8 path"), (W, H), Preset::Una).unwrap_or_else(|e| panic!("a missing photo never refuses the document: {e}"));
    assert!(back.view.photo.is_none(), "bare: no underlay");
    assert_eq!(back.view.design.journal.doc, doc, "the same document");
    assert!(!back.view.design.journal.dirty());
    assert_eq!(back.view.design.journal.disk, Some(Ok(format!("photo: missing {}", path.display()))), "the one word in the corner");
    assert_eq!(back.photo_doc.as_ref().map(|(p, _)| p.as_path()), Some(path.as_path()), "the reference is kept");

    back.save().expect("the save");
    assert_eq!(std::fs::read_to_string(&vug).expect("re-saved"), saved, "the photo line survives a save without the photo");
    eprintln!("photo: doc-ref missing=fixture.png bare=true kept-on-save=true");
    let _ = std::fs::remove_dir_all(&dir);
}

// ---- VUGDOCPHOTOREFCROP (SR660): the document names the crop with its frame; a re-open from the crop alone (named tail block) ----

/// The text `s` writes for `doc` drawn over the photo `path` in the frame `h` (row-major `h=[..]` after the pitch).
fn with_framed(doc: &doc_core::Doc, path: &str, h: &[[f64; 3]; 3]) -> String {
    let flat = [h[0][0], h[0][1], h[0][2], h[1][0], h[1][1], h[1][2], h[2][0], h[2][1], h[2][2]];
    let mut stmts = vec![doc_core::Stmt::Photo { path: path.into(), pitch_mm: photo::PITCH_MM, h: Some(flat) }];
    stmts.extend(doc.stmts.iter().cloned());
    doc_core::write(&doc_core::Doc { tol: doc.tol, stmts, lines: Vec::new() })
}

/// The bounds of a closed outline: (min x, min y, max x, max y).
fn bounds(o: &[Vec2]) -> (f64, f64, f64, f64) {
    o.iter().fold((f64::MAX, f64::MAX, f64::MIN, f64::MIN), |b, p| (b.0.min(p.x), b.1.min(p.y), b.2.max(p.x), b.3.max(p.y)))
}

/// VUGDOCPHOTOREFCROP (SR660): a line sketched over the photo survives crop -> save -> re-open from the CROP file alone at
/// the same paper mm. After SR649's carry `s` writes `photo path="fixture.crop.png" pitch=5 h=[..]` (the slid H' bit for
/// bit) and `photo_doc` names the crop; the ORIGINAL is then removed; `Viewer::open` of the .vug measures the crop
/// through THE photo road and lays it on the stated frame: the same document, clean, `Photo.h` == H', the colour under
/// the line the one it was drawn over, the outline within the band of the carried one, the crop road on the crop's file,
/// and a re-save writes the same bytes. The crop measured by itself numbers its lattice from its own first line: the
/// paper map from its frame to the document's at the crop's centre is printed in pitches.
#[test]
fn a_sketched_line_survives_the_crop_and_reopens_from_the_crop_alone_at_the_same_paper_mm() {
    let (mut vw, dir, [_, _, w, h], doc, _, line) = crop_sketched("refcrop");
    let crop = dir.join("fixture.crop.png");
    let carried = ph(&vw).clone();
    assert!(key(&mut vw, "s"), "`s` saves");
    assert!(!vw.view.design.journal.dirty(), "saved: {:?}", vw.view.design.journal.disk);
    let vug = dir.join("fixture.vug");
    let saved = std::fs::read_to_string(&vug).expect("the document is saved");
    assert_eq!(saved, with_framed(&doc, "fixture.crop.png", &carried.h), "the photo line names the crop in its slid frame");
    assert_eq!(vw.photo_doc.as_ref().map(|(p, _)| p.as_path()), Some(crop.as_path()), "the reference is the crop");
    assert_eq!(vw.photo_frame, Some(carried.h));
    std::fs::remove_file(dir.join("fixture.png")).expect("the original is removed");

    let alone = Viewer::open(crop.to_str().expect("a UTF-8 path"), (W, H), Preset::Una).unwrap_or_else(|e| panic!("the crop measures alone: {e}"));
    let own = ph(&alone).h;
    let c = Vec2::new(w as f64 * 0.5, h as f64 * 0.5);
    let (Ok(inv_own), Ok(inv_doc)) = (photo_core::homography::invert(&own), photo_core::homography::invert(&carried.h)) else { panic!("both frames invert") };
    let (q_own, q_doc) = (photo_core::homography::apply(&inv_own, c).expect("own"), photo_core::homography::apply(&inv_doc, c).expect("doc"));

    let mut back = Viewer::open(vug.to_str().expect("a UTF-8 path"), (W, H), Preset::Una).unwrap_or_else(|e| panic!("the document opens: {e}"));
    assert!(back.view.photo.is_some(), "the crop is laid under the document: {:?}", back.view.design.journal.disk);
    assert_eq!(back.view.design.journal.doc, doc, "the same document: the line at the same paper mm");
    assert!(!back.view.design.journal.dirty(), "opened clean");
    assert_eq!(back.title, "fixture.vug");
    assert_eq!(ph(&back).h, carried.h, "the stated frame, bit for bit, not the crop's own");
    assert_eq!(ph(&back).crop.source.as_deref(), Some(crop.as_path()), "the crop road on the crop's file");
    let mid = Vec2::new((line[0].x + line[1].x) * 0.5, (line[0].y + line[1].y) * 0.5);
    for q in [line[0], line[1], mid] {
        let (c0, c1) = (carried.sample([q.x, q.y]).expect("on the carried crop"), ph(&back).sample([q.x, q.y]).expect("on the re-laid crop"));
        assert_eq!(c0, c1, "the same colour under the line at {q:?}");
    }
    let (b0, b1) = (bounds(&carried.outline), bounds(&ph(&back).outline));
    let band = 2.0 * carried.band_mm.max(ph(&back).band_mm);
    assert!((b0.0 - b1.0).abs() <= band && (b0.1 - b1.1).abs() <= band && (b0.2 - b1.2).abs() <= band && (b0.3 - b1.3).abs() <= band, "the outline on the document's frame: {b0:?} vs {b1:?} (band {band})");

    back.save().expect("the re-save");
    assert_eq!(std::fs::read_to_string(&vug).expect("re-saved"), saved, "a re-save writes the same bytes: the crop and its frame");
    eprintln!(
        "photo: doc-ref-crop saved=fixture.vug photo=fixture.crop.png h-equal=true original=removed shift={},{} pitches own-h-equal={} outline={:?}/{:?}",
        (q_doc.x - q_own.x) / photo::PITCH_MM,
        (q_doc.y - q_own.y) / photo::PITCH_MM,
        own == carried.h,
        b0,
        b1
    );
    let _ = std::fs::remove_dir_all(&dir);
}
