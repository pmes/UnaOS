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
