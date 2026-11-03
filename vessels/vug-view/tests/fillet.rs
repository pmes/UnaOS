// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VUGFACEFILLET (LEDGER SR511): the edge tool's host witness — on a 10 mm box body, the cursor beside the top-front
//! edge and `f` picks that edge (its persistent Name), `2` Enter fillets it to the SR502 volume with 7 faces and a
//! re-tessellated body; `12` Enter shows `RadiusTooLarge max 10` and leaves the body unchanged; Shift+f `2,3` Enter
//! chamfers it to 970. VUGFACETREEFILLET (LEDGER SR518): on a 10 mm box drawn and pulled through the face (so the
//! document holds `x0`), `f 2 Enter` records ONE `fillet f0 on=x0` whose replay volume is the body's; `u` restores 1000
//! and drops the row (`U` brings it back as one solid); deleting the fillet row in the panel restores the body.
//! VUGFACETREEEDITR (LEDGER SR528): the fillet row's value edited in the panel from 2 to 3 re-fillets through replay
//! to the SR502 closed form at r 3; 12 is refused by the row's line with the document and the r 2 body kept; a chamfer
//! row edited to `2,4` (the edge tool's pair road) gives 960.

use bandy::SMessage;
use brep_core::Tol;
use brep_core::primitives::box_;
use std::f64::consts::PI;
use vug_view::Viewer;
use vug_view::extrude::Body;
use vug_view::fillet::Mode;
use vug_view::model::Model;
use vug_view::view::Preset;

const W: u32 = 400;
const H: u32 = 300;

/// The opened mesh (replaced by the body the moment one is set).
fn cube() -> Model {
    let s = 10.0;
    let p = vec![[-s, -s, -s], [s, -s, -s], [s, s, -s], [-s, s, -s], [-s, -s, s], [s, -s, s], [s, s, s], [-s, s, s]];
    let t = vec![[0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7], [0, 1, 5], [0, 5, 4], [3, 7, 6], [3, 6, 2], [0, 4, 7], [0, 7, 3], [1, 2, 6], [1, 6, 5]];
    Model::from_triangles(&p, &t).expect("cube")
}

fn typed(vw: &mut Viewer, t: &str) -> bool {
    vw.input(&SMessage::BrowserText(t.into()))
}

fn key(vw: &mut Viewer, k: &str) -> bool {
    vw.input(&SMessage::BrowserKey(k.into()))
}

/// The box [0, 10]^3 as the design's body, the Home view on it, the cursor 0.2 mm onto the top face beside the
/// top-front edge's midpoint (5, 0, 10).
fn boxed() -> Viewer {
    let mut vw = Viewer::new(cube(), "fillet", (W, H), Preset::Una);
    let b = box_(10.0, 10.0, 10.0, Tol::DEFAULT).expect("box");
    vw.view.design.body = Some(Body::from_breps(vec![b], Tol::DEFAULT).expect("body"));
    assert!(typed(&mut vw, "h"));
    let (x, y) = vw.view.project_f64([5.0, 0.2, 10.0]).expect("on screen");
    vw.input(&SMessage::BrowserMouseMove(x, y, 0));
    vw
}

fn body(vw: &Viewer) -> &Body {
    vw.view.design.body.as_ref().expect("a body")
}

/// The Name of the body's top-front edge (both ends at y = 0, z = 10).
fn top_front(vw: &Viewer) -> brep_core::Name {
    let b = &body(vw).breps[0];
    let near = |a: f64, x: f64| (a - x).abs() < 1e-9;
    let hits: Vec<brep_core::Name> = b
        .edges
        .iter()
        .filter(|e| e.v.iter().all(|&v| near(b.vertices[v as usize].p.y, 0.0) && near(b.vertices[v as usize].p.z, 10.0)))
        .map(|e| e.name)
        .collect();
    assert_eq!(hits.len(), 1);
    hits[0]
}

fn picked(vw: &mut Viewer, k: &str, mode: Mode) {
    let want = top_front(vw);
    assert!(typed(vw, k), "{k} opens the edge tool");
    let tool = vw.view.fillet.as_ref().expect("the tool is open");
    assert_eq!(tool.mode, mode);
    assert_eq!(tool.edge.as_ref().expect("an edge under the cursor").name, want, "the top-front edge is picked");
}

#[test]
fn f_two_enter_fillets_the_top_front_edge_to_the_sr502_volume() {
    let mut vw = boxed();
    let before = body(&vw).model.triangle_count();
    picked(&mut vw, "f", Mode::Fillet);
    assert!(typed(&mut vw, "2"));
    assert_eq!(vw.view.fillet.as_ref().unwrap().buffer, "2");
    assert!(key(&mut vw, "Return"));
    assert!(vw.view.fillet.is_none(), "Enter commits and closes the tool");
    let b = body(&vw);
    assert_eq!(b.breps[0].faces.len(), 7);
    let want = 1000.0 - (4.0 - PI) * 10.0;
    let v = b.volume().expect("volume");
    assert!((v - want).abs() <= 1e-9 * want, "volume {v} vs {want}");
    assert_ne!(b.model.triangle_count(), before, "the body is re-tessellated");
    assert_eq!(b.face_of.len(), vw.view.design.shown().expect("shown").triangle_count());
}

#[test]
fn a_radius_too_large_is_named_at_the_edge_and_the_body_is_unchanged() {
    let mut vw = boxed();
    picked(&mut vw, "f", Mode::Fillet);
    assert!(typed(&mut vw, "12"));
    assert!(key(&mut vw, "Return"));
    let tool = vw.view.fillet.as_ref().expect("a refusal leaves the tool open");
    assert_eq!(tool.note.as_ref().expect("a note").text, "RadiusTooLarge max 10");
    let b = body(&vw);
    assert_eq!(b.breps[0].faces.len(), 6);
    assert!((b.volume().unwrap() - 1000.0).abs() < 1e-9, "the body is unchanged");
    assert!(key(&mut vw, "Escape"));
    assert!(vw.view.fillet.is_none(), "Esc cancels");
}

#[test]
fn shift_f_two_comma_three_chamfers_to_970() {
    let mut vw = boxed();
    picked(&mut vw, "F", Mode::Chamfer);
    assert!(typed(&mut vw, "2,3"));
    assert_eq!(vw.view.fillet.as_ref().unwrap().buffer, "2,3");
    assert!(key(&mut vw, "Return"));
    assert!(vw.view.fillet.is_none());
    let b = body(&vw);
    assert_eq!(b.breps[0].faces.len(), 7);
    let v = b.volume().expect("volume");
    assert!((v - 970.0).abs() <= 1e-9 * 970.0, "volume {v}");
}

// ---- VUGFACETREEFILLET (SR518): the fillet as a statement of the face's document ----

fn button(vw: &mut Viewer, down: bool, p: (f64, f64)) -> bool {
    vw.input(&SMessage::BrowserButton { down, x: p.0, y: p.1, mods: 0, surface: 0 })
}

fn screen(vw: &Viewer, q: (f64, f64)) -> (f64, f64) {
    let face = vw.view.design.face.as_ref().expect("a sketch is open");
    face.to_screen(&vw.view, sketch_core::Vec2::new(q.0, q.1)).expect("on screen")
}

fn stroke(vw: &mut Viewer, a: (f64, f64), b: (f64, f64)) {
    let (sa, sb) = (screen(vw, a), screen(vw, b));
    let mid = screen(vw, (0.5 * (a.0 + b.0), 0.5 * (a.1 + b.1)));
    button(vw, true, sa);
    assert!(vw.input(&SMessage::BrowserMouseMove(mid.0, mid.1, 0)));
    assert!(vw.input(&SMessage::BrowserMouseMove(sb.0, sb.1, 0)));
    assert!(button(vw, false, sb));
}

fn click(vw: &mut Viewer, q: (f64, f64)) {
    let p = screen(vw, q);
    assert!(button(vw, true, p));
    button(vw, false, p);
}

/// The 10 mm box through the face (the tests/journal.rs road): a 10 x 10 rectangle by strokes in the top view,
/// dimensioned 10 and 10, pulled 10 by a typed distance (`x0`); then Home and the cursor beside the top-front edge.
fn drawn() -> Viewer {
    const SQ: [(f64, f64); 4] = [(0.0, 0.0), (10.0, 0.0), (10.0, 10.0), (0.0, 10.0)];
    let mut vw = Viewer::new(cube(), "fillet-tree", (W, H), Preset::Una);
    assert!(typed(&mut vw, "7"));
    assert!(typed(&mut vw, "l"));
    for i in 0..4 {
        stroke(&mut vw, SQ[i], SQ[(i + 1) % 4]);
    }
    assert!(typed(&mut vw, "d"));
    click(&mut vw, (5.0, 0.0));
    assert!(typed(&mut vw, "10"));
    assert!(key(&mut vw, "Return"));
    click(&mut vw, (10.0, 5.0));
    assert!(typed(&mut vw, "10"));
    assert!(key(&mut vw, "Return"));
    assert_eq!(vw.view.design.refusal, None);
    assert!(typed(&mut vw, "h"));
    assert!(typed(&mut vw, "e"));
    assert!(typed(&mut vw, "10"));
    assert!(key(&mut vw, "Return"));
    assert!((body(&vw).volume().expect("volume") - 1000.0).abs() <= 1e-6 * 1000.0);
    assert!(vw.view.design.journal.doc.stmts.iter().any(|s| matches!(s, doc_core::Stmt::Extrude { id, .. } if id == "x0")));
    assert!(typed(&mut vw, "h"));
    let (x, y) = vw.view.project_f64([5.0, 0.2, 10.0]).expect("on screen");
    vw.input(&SMessage::BrowserMouseMove(x, y, 0));
    vw
}

/// `f 2 Enter` on the drawn box: the picked Name is returned for the statement check.
fn filleted() -> (Viewer, brep_core::Name) {
    let mut vw = drawn();
    let name = top_front(&vw);
    picked(&mut vw, "f", Mode::Fillet);
    assert!(typed(&mut vw, "2"));
    assert!(key(&mut vw, "Return"));
    assert!(vw.view.fillet.is_none(), "Enter commits and closes the tool");
    (vw, name)
}

const FILLETED: f64 = 1000.0 - (4.0 - PI) * 10.0;

fn fillets(vw: &Viewer) -> Vec<doc_core::Stmt> {
    vw.view.design.journal.doc.stmts.iter().filter(|s| matches!(s, doc_core::Stmt::Fillet { .. })).cloned().collect()
}

fn has_row(vw: &Viewer) -> bool {
    vw.view.design.journal.rows().iter().any(|r| r.starts_with("FILLET F0 ON=X0 "))
}

#[test]
fn f_two_enter_records_one_fillet_whose_replay_volume_is_the_bodys() {
    let (vw, name) = filleted();
    let f = fillets(&vw);
    assert_eq!(f.len(), 1, "one fillet statement");
    match &f[0] {
        doc_core::Stmt::Fillet { id, on, edges, r } => {
            assert_eq!((id.as_str(), on.as_str()), ("f0", "x0"));
            assert_eq!(edges, &vec![name], "the picked edge's Name");
            assert_eq!(*r, 2.0);
        }
        other => panic!("not a fillet: {other:?}"),
    }
    assert!(has_row(&vw), "{:?}", vw.view.design.journal.rows());
    let d = &vw.view.design.journal.doc;
    let rep = doc_core::replay(d, d.tol);
    assert!(rep.failures.is_empty(), "{:?}", rep.failures);
    let solid = rep.solids.iter().find(|s| s.id == "f0").expect("the fillet's solid");
    let rv = brep_core::volume(&solid.brep).expect("replay volume");
    let b = body(&vw);
    assert_eq!(b.breps.len(), 1, "one solid");
    let bv = b.volume().expect("volume");
    assert!((rv - bv).abs() <= 1e-9 * bv, "replay {rv} vs body {bv}");
    assert!((bv - FILLETED).abs() <= 1e-6 * FILLETED, "volume {bv} vs {FILLETED}");
}

#[test]
fn u_restores_the_unfilleted_body_and_removes_the_row() {
    let (mut vw, _) = filleted();
    assert!(typed(&mut vw, "u"));
    let v = body(&vw).volume().expect("volume");
    assert!((v - 1000.0).abs() <= 1e-6 * 1000.0, "undone: volume {v}");
    assert!(fillets(&vw).is_empty());
    assert!(!has_row(&vw));
    // Redo regenerates the filleted document: the edit's solid stands in x0's place (one solid, not two).
    assert!(typed(&mut vw, "U"));
    assert_eq!(fillets(&vw).len(), 1);
    let b = body(&vw);
    assert_eq!(b.breps.len(), 1, "redo: one solid");
    let v = b.volume().expect("volume");
    assert!((v - FILLETED).abs() <= 1e-6 * FILLETED, "redone: volume {v}");
}

#[test]
fn deleting_the_fillet_row_restores_the_body() {
    let (mut vw, _) = filleted();
    assert!(typed(&mut vw, "t"));
    assert!(vw.view.design.journal.panel);
    let i = vw.view.design.journal.doc.stmts.iter().position(|s| matches!(s, doc_core::Stmt::Fillet { .. })).expect("the row");
    vw.view.design.journal.cursor = i;
    assert!(key(&mut vw, "Delete"));
    assert_eq!(vw.view.design.journal.note.as_deref(), Some("deleted f0"));
    assert!(fillets(&vw).is_empty());
    let v = body(&vw).volume().expect("volume");
    assert!((v - 1000.0).abs() <= 1e-6 * 1000.0, "volume {v}");
}

// ---- VUGFACETREEEDITR (SR528): the fillet / chamfer row's value edit re-fillets ----

/// The panel open, the cursor on the first row `is` matches, Enter (the edit opens empty), `t` typed: the row's index.
fn edit_row(vw: &mut Viewer, is: impl Fn(&doc_core::Stmt) -> bool, t: &str) -> usize {
    assert!(typed(vw, "t"));
    let i = vw.view.design.journal.doc.stmts.iter().position(is).expect("the row");
    vw.view.design.journal.cursor = i;
    assert!(key(vw, "Return"));
    assert_eq!(vw.view.design.journal.edit.as_deref(), Some(""), "Enter opens the row's edit");
    assert!(typed(vw, t));
    assert_eq!(vw.view.design.journal.edit.as_deref(), Some(t), "the typed value");
    i
}

fn is_fillet(s: &doc_core::Stmt) -> bool {
    matches!(s, doc_core::Stmt::Fillet { .. })
}

#[test]
fn editing_the_fillet_row_to_three_refillets_to_the_closed_form() {
    let (mut vw, _) = filleted();
    edit_row(&mut vw, is_fillet, "3");
    assert!(key(&mut vw, "Return"));
    assert_eq!(vw.view.design.journal.note, None);
    let f = fillets(&vw);
    assert_eq!(f.len(), 1);
    assert!(matches!(&f[0], doc_core::Stmt::Fillet { id, r, .. } if id == "f0" && *r == 3.0), "{:?}", f[0]);
    let b = body(&vw);
    assert_eq!(b.breps.len(), 1, "one solid");
    let want = 1000.0 - (4.0 - PI) * 9.0 / 4.0 * 10.0;
    let v = b.volume().expect("volume");
    assert!((v - want).abs() <= 1e-6 * want, "r 3: volume {v} vs {want}");
}

#[test]
fn editing_the_fillet_row_to_twelve_is_refused_by_line_and_the_body_kept() {
    let (mut vw, _) = filleted();
    let before = vw.view.design.journal.doc.clone();
    let i = edit_row(&mut vw, is_fillet, "12");
    assert!(key(&mut vw, "Return"));
    let note = vw.view.design.journal.note.clone().expect("refused");
    assert!(note.starts_with(&format!("line {}:", i + 2)), "the fillet's line named: {note}");
    assert!(note.contains("RadiusTooLarge"), "{note}");
    assert_eq!(vw.view.design.journal.doc, before, "the document is unchanged");
    let v = body(&vw).volume().expect("volume");
    assert!((v - FILLETED).abs() <= 1e-6 * FILLETED, "the r 2 body is kept: volume {v}");
}

#[test]
fn editing_a_chamfer_row_to_two_comma_four_gives_960() {
    let mut vw = drawn();
    picked(&mut vw, "F", Mode::Chamfer);
    assert!(typed(&mut vw, "2,3"));
    assert!(key(&mut vw, "Return"));
    assert!((body(&vw).volume().expect("volume") - 970.0).abs() <= 1e-6 * 970.0);
    edit_row(&mut vw, |s| matches!(s, doc_core::Stmt::Chamfer { .. }), "2,4");
    assert!(key(&mut vw, "Return"));
    assert_eq!(vw.view.design.journal.note, None);
    let c: Vec<&doc_core::Stmt> = vw.view.design.journal.doc.stmts.iter().filter(|s| matches!(s, doc_core::Stmt::Chamfer { .. })).collect();
    assert_eq!(c.len(), 1);
    assert!(matches!(c[0], doc_core::Stmt::Chamfer { d1, d2, .. } if *d1 == 2.0 && *d2 == 4.0), "{:?}", c[0]);
    let v = body(&vw).volume().expect("volume");
    assert!((v - 960.0).abs() <= 1e-6 * 960.0, "2 x 4 chamfer: volume {v}");
}
