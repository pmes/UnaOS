// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VUGFACEFILLET (LEDGER SR511): the edge tool's host witness — on a 10 mm box body, the cursor beside the top-front
//! edge and `f` picks that edge (its persistent Name), `2` Enter fillets it to the SR502 volume with 7 faces and a
//! re-tessellated body; `12` Enter shows `RadiusTooLarge max 10` and leaves the body unchanged; Shift+f `2,3` Enter
//! chamfers it to 970.

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
