// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VUGFACE2 (LEDGER SR485): the sketch face's host witness — a scripted event sequence draws a closed rectangle with
//! four lines and the inferences (H / V / coincident), the dimensioned sketch solves to dof 0, profiles finds the one
//! loop, extrude gives the box's exact volume, the pull commits a body that replaces the drawn mesh, and the plane
//! unprojection round-trips on each world plane.

use bandy::SMessage;
use sketch_core::{Constraint, ConstraintId, Entity, PointRef, Vec2};
use vug_view::extrude;
use vug_view::model::Model;
use vug_view::raster;
use vug_view::sketch::{SketchFace, Tool, world_planes};
use vug_view::view::{Preset, View};
use vug_view::Viewer;

const W: u32 = 400;
const H: u32 = 300;
/// The rectangle drawn: 16 mm by 10 mm from the origin, counter-clockwise.
const RECT: [(f64, f64); 4] = [(0.0, 0.0), (16.0, 0.0), (16.0, 10.0), (0.0, 10.0)];

fn cube() -> Model {
    let s = 10.0;
    let p = vec![[-s, -s, -s], [s, -s, -s], [s, s, -s], [-s, s, -s], [-s, -s, s], [s, -s, s], [s, s, s], [-s, s, s]];
    let t = vec![[0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7], [0, 1, 5], [0, 5, 4], [3, 7, 6], [3, 6, 2], [0, 4, 7], [0, 7, 3], [1, 2, 6], [1, 6, 5]];
    Model::from_triangles(&p, &t).expect("cube")
}

fn face(vw: &Viewer) -> &SketchFace {
    vw.view.design.face.as_ref().expect("a sketch is open")
}

fn screen(vw: &Viewer, q: (f64, f64)) -> (f64, f64) {
    face(vw).to_screen(&vw.view, Vec2::new(q.0, q.1)).expect("on screen")
}

fn button(vw: &mut Viewer, down: bool, p: (f64, f64)) -> bool {
    vw.input(&SMessage::BrowserButton { down, x: p.0, y: p.1, mods: 0, surface: 0 })
}

fn move_to(vw: &mut Viewer, p: (f64, f64)) -> bool {
    vw.input(&SMessage::BrowserMouseMove(p.0, p.1, 0))
}

/// One line by press-drag-release from plane point `a` to `b` (through their midpoint).
fn stroke(vw: &mut Viewer, a: (f64, f64), b: (f64, f64)) {
    let (sa, sb) = (screen(vw, a), screen(vw, b));
    let mid = screen(vw, (0.5 * (a.0 + b.0), 0.5 * (a.1 + b.1)));
    button(vw, true, sa);
    assert!(move_to(vw, mid));
    assert!(move_to(vw, sb));
    assert!(button(vw, false, sb));
}

/// The top view, the line tool, the four sides of `RECT`.
fn rectangle() -> Viewer {
    let mut vw = Viewer::new(cube(), "sketch", (W, H), Preset::Una);
    assert!(vw.input(&SMessage::BrowserText("7".into())));
    assert!(vw.input(&SMessage::BrowserText("l".into())));
    for i in 0..4 {
        stroke(&mut vw, RECT[i], RECT[(i + 1) % 4]);
    }
    vw
}

/// The rectangle with its width and height dimensioned by two Distance constraints (the dimension tool is owed).
fn dimensioned() -> Viewer {
    let mut vw = rectangle();
    let f = vw.view.design.face.as_mut().unwrap();
    let lines: Vec<_> = f.sketch.entities().iter().filter_map(|e| if let Entity::Line { a, b } = e { Some((*a, *b)) } else { None }).collect();
    assert_eq!(lines.len(), 4);
    f.sketch.add(Constraint::Distance(PointRef::Point(lines[0].0), PointRef::Point(lines[0].1), 16.0)).expect("width");
    f.sketch.add(Constraint::Distance(PointRef::Point(lines[1].0), PointRef::Point(lines[1].1), 10.0)).expect("height");
    let s = f.solve().expect("the dimensioned rectangle solves");
    assert!(s.redundant.is_empty(), "redundant {:?}", s.redundant);
    vw
}

#[test]
fn rectangle_by_events_infers_h_v_and_coincident() {
    let vw = rectangle();
    let f = face(&vw);
    assert_eq!(f.tool, Tool::Line);
    assert!(f.plane.z.z > 0.999, "the top view opens the XY plane, got normal {:?}", f.plane.z);
    let (mut h, mut v, mut c) = (0, 0, 0);
    for i in 0..64 {
        match f.sketch.constraint(ConstraintId(i)) {
            Some(Constraint::Horizontal(_)) => h += 1,
            Some(Constraint::Vertical(_)) => v += 1,
            Some(Constraint::Coincident(..)) => c += 1,
            _ => {}
        }
    }
    assert_eq!((h, v, c), (2, 2, 5), "H / V / coincident inferred");
    assert_eq!(vw.view.design.refusal, None);
    // Width and height are the two freedoms left (the origin is fixed and every corner is tied).
    assert_eq!(f.dof, 2);
    // The corners sit where they were drawn.
    for (i, e) in f.sketch.entities().iter().enumerate() {
        if let Entity::Point { .. } = e {
            let p = f.sketch.point(PointRef::Point(sketch_core::EntityId(i as u32))).unwrap();
            assert!(RECT.iter().any(|q| (p.x - q.0).hypot(p.y - q.1) < 1e-5), "a point off the corners: {p:?}");
        }
    }
}

#[test]
fn dimensioned_rectangle_is_dof_zero_with_one_profile() {
    let vw = dimensioned();
    let f = face(&vw);
    assert_eq!(f.dof, 0);
    assert_eq!(f.profiles.len(), 1);
    assert!(f.profiles[0].holes.is_empty());
    assert_eq!(f.profiles[0].outer.edges.len(), 4);
    assert!((f.profiles[0].outer.area - 160.0).abs() < 1e-5, "area {}", f.profiles[0].outer.area);
}

#[test]
fn extrude_ten_gives_the_box_volume() {
    let vw = dimensioned();
    let f = face(&vw);
    let p = &f.profiles[0];
    let b = extrude::extrude(p, &f.plane, 10.0, brep_core::Tol::DEFAULT).expect("extrude");
    let vol = brep_core::volume(&b).expect("volume");
    let want = p.outer.area * 10.0;
    assert!((vol - want).abs() <= 1e-9 * want, "volume {vol} vs area x 10 {want}");
    assert!((vol - 1600.0).abs() < 1e-4, "volume {vol}");
    // A negative distance goes the other way with the same volume.
    let down = extrude::extrude(p, &f.plane, -10.0, brep_core::Tol::DEFAULT).expect("extrude down");
    assert!((brep_core::volume(&down).unwrap() - want).abs() <= 1e-9 * want);
}

#[test]
fn pull_commits_a_body_that_replaces_the_mesh() {
    let mut vw = dimensioned();
    assert!(vw.input(&SMessage::BrowserText("h".into())));
    assert!(vw.input(&SMessage::BrowserText("e".into())));
    let pull = vw.view.design.pull.as_ref().expect("e starts a pull");
    let c = pull.centroid;
    let (sx, sy) = vw.view.project_f64(c).expect("the centroid is on screen");
    assert!(move_to(&mut vw, (sx, sy - 40.0)));
    let pull = vw.view.design.pull.as_ref().unwrap();
    let (d, area) = (pull.distance, pull.profile.outer.area);
    assert!(d > 0.0, "moving up the screen pulls along +Z in the Home view: {d}");
    assert!(pull.preview.is_some(), "a live preview");
    assert!(button(&mut vw, true, (sx, sy - 40.0)));
    button(&mut vw, false, (sx, sy - 40.0));
    let design = &vw.view.design;
    assert!(design.pull.is_none() && design.face.is_none(), "the click commits and closes the sketch");
    let body = design.body.as_ref().expect("a body");
    let vol = body.volume().expect("volume");
    assert!((vol - area * d).abs() <= 1e-9 * area * d, "volume {vol} vs area x pull {}", area * d);
    let shown = design.shown().expect("the body is shown");
    assert_eq!(shown.triangle_count(), body.model.triangle_count());
    assert_eq!(body.face_of.len(), shown.triangle_count());
    // A box has six planar faces: every triangle names one.
    assert!((0..shown.triangle_count() as u32).all(|t| body.face_frame(t).is_some()));
}

#[test]
fn unproject_round_trips_on_each_world_plane() {
    let m = cube();
    let v = View::new(&m, (W, H), Preset::Una);
    for (name, plane) in world_planes() {
        let f = SketchFace::new(plane);
        for i in 0..10 {
            for j in 0..10 {
                let q = Vec2::new(-20.0 + 4.4 * i as f64, -20.0 + 4.4 * j as f64);
                let s = f.to_screen(&v, q).expect("in front of the eye");
                let r = f.unproject(&v, s.0, s.1).expect("the plane is hit");
                let e = (r.x - q.x).hypot(r.y - q.y);
                assert!(e <= 1e-9, "{name}: {q:?} -> {s:?} -> {r:?} ({e} mm)");
            }
        }
    }
}

#[test]
fn the_profile_is_tinted() {
    let vw = rectangle();
    let (x, y) = screen(&vw, (8.0, 5.0));
    let mut bare = vw.view.clone();
    bare.design = Default::default();
    let with = raster::render(&vw.model, &vw.view);
    let without = raster::render(&vw.model, &bare);
    assert_ne!(with.pixel(x as u32, y as u32), without.pixel(x as u32, y as u32));
}

#[test]
fn a_click_with_the_line_tool_creates_nothing_and_esc_selects() {
    let mut vw = Viewer::new(cube(), "sketch", (W, H), Preset::Una);
    assert!(vw.input(&SMessage::BrowserText("l".into())));
    let n = face(&vw).sketch.entities().len();
    assert_eq!(n, 1, "only the origin");
    let p = screen(&vw, (3.0, 2.0));
    button(&mut vw, true, p);
    button(&mut vw, false, p);
    assert_eq!(face(&vw).sketch.entities().len(), n);
    assert!(vw.input(&SMessage::BrowserKey("Escape".into())));
    assert_eq!(face(&vw).tool, Tool::Select);
    // Outside a sketch `p` stays the preset key; inside it is the point tool.
    let mut bare = Viewer::new(cube(), "nav", (W, H), Preset::Una);
    assert!(bare.input(&SMessage::BrowserText("p".into())));
    assert_eq!(bare.view.preset, Preset::SolidWorks);
    assert!(bare.view.design.face.is_none());
    assert!(vw.input(&SMessage::BrowserText("p".into())));
    assert_eq!(face(&vw).tool, Tool::Point);
    assert_eq!(vw.view.preset, Preset::Una);
}

/// VUGSKETCHERASE (SR495): the circle tool's radius follows the cursor live (a sketch_core circle through
/// `set_radius`, its profile tinted before the release), and Backspace over its centre removes the centre and, by the
/// cascade, the circle — ids stay put and the origin is never erased.
#[test]
fn a_circle_grows_live_and_backspace_erases_it() {
    let mut vw = Viewer::new(cube(), "sketch", (W, H), Preset::Una);
    assert!(vw.input(&SMessage::BrowserText("7".into())));
    assert!(vw.input(&SMessage::BrowserText("c".into())));
    let (at, mid, rim, origin_px) = (screen(&vw, (5.0, 5.0)), screen(&vw, (7.0, 5.0)), screen(&vw, (8.0, 5.0)), screen(&vw, (0.0, 0.0)));
    button(&mut vw, true, at);
    assert!(move_to(&mut vw, mid));
    assert!(move_to(&mut vw, rim));
    let live = |vw: &Viewer| {
        face(vw).sketch.entities().iter().enumerate().find_map(|(i, e)| match e {
            Entity::Circle { centre, .. } => Some((sketch_core::EntityId(i as u32), *centre)),
            _ => None,
        })
    };
    let (circle, centre) = live(&vw).expect("the circle exists while the rim is dragged");
    assert!((face(&vw).sketch.radius(circle).unwrap() - 3.0).abs() < 1e-6, "{:?}", face(&vw).sketch.radius(circle));
    assert_eq!(face(&vw).profiles.len(), 1, "tinted live");
    assert!(button(&mut vw, false, rim));
    assert!((face(&vw).sketch.radius(circle).unwrap() - 3.0).abs() < 1e-6);
    let n = face(&vw).sketch.entities().len();
    let _ = move_to(&mut vw, at);
    assert!(vw.input(&SMessage::BrowserKey("BackSpace".into())), "the centre under the cursor is erased");
    assert_eq!(face(&vw).sketch.entity(centre), Some(&Entity::Removed));
    assert_eq!(face(&vw).sketch.entity(circle), Some(&Entity::Removed), "the circle goes with its centre");
    assert_eq!(face(&vw).sketch.entities().len(), n, "ids stay put");
    assert!(face(&vw).profiles.is_empty());
    assert!(vw.view.design.refusal.is_none());
    let origin = face(&vw).origin;
    let _ = move_to(&mut vw, origin_px);
    vw.input(&SMessage::BrowserKey("BackSpace".into()));
    assert!(matches!(face(&vw).sketch.entity(origin), Some(Entity::Point { .. })), "the origin stays");
}

// ---- VUGFACEDIMENSION (LEDGER SR494): the dimension tool and the typed value, by events ----

fn key(vw: &mut Viewer, k: &str) -> bool {
    vw.input(&SMessage::BrowserKey(k.into()))
}

fn typed(vw: &mut Viewer, t: &str) -> bool {
    vw.input(&SMessage::BrowserText(t.into()))
}

fn click(vw: &mut Viewer, q: (f64, f64)) {
    let p = screen(vw, q);
    assert!(button(vw, true, p), "the dimension tool takes the press at {q:?}");
    button(vw, false, p);
}

/// The SR485 rectangle dimensioned by the TOOL: `d`, click the bottom line, type 16, Enter; click the right line, type
/// 10, Enter.
fn dimensioned_by_tool() -> Viewer {
    let mut vw = rectangle();
    assert!(typed(&mut vw, "d"));
    assert_eq!(face(&vw).tool, Tool::Dimension);
    click(&mut vw, (8.0, 0.0));
    assert!(face(&vw).edit.is_some(), "a click on a line opens its length for editing");
    assert!(typed(&mut vw, "16"));
    assert_eq!(face(&vw).edit.as_ref().unwrap().buffer, "16");
    assert!(key(&mut vw, "Return"));
    assert!(face(&vw).edit.is_none(), "Enter commits and closes: {:?}", vw.view.design.refusal);
    click(&mut vw, (16.0, 5.0));
    assert!(typed(&mut vw, "10"));
    assert!(key(&mut vw, "Return"));
    vw
}

fn live_constraints(f: &SketchFace) -> Vec<(u32, Constraint)> {
    (0..256).filter_map(|i| f.sketch.constraint(ConstraintId(i)).map(|c| (i, *c))).collect()
}

#[test]
fn dimension_tool_makes_the_rectangle_exact() {
    let vw = dimensioned_by_tool();
    let f = face(&vw);
    assert_eq!(vw.view.design.refusal, None);
    assert!(f.edit.is_none());
    assert_eq!(f.dims.len(), 2);
    assert_eq!(f.dof, 0);
    assert_eq!(f.profiles.len(), 1);
    assert!(f.profiles[0].holes.is_empty());
    assert!((f.profiles[0].outer.area - 160.0).abs() < 1e-5, "area {}", f.profiles[0].outer.area);
    // Both dimensions are Distance constraints with the typed values, drawn as glyphs with their labels.
    let mut vals: Vec<f64> = f.dims.iter().map(|d| match f.sketch.constraint(d.constraint) {
        Some(Constraint::Distance(_, _, v)) => *v,
        c => panic!("a dimension is not a Distance: {c:?}"),
    }).collect();
    vals.sort_by(|a, b| a.partial_cmp(b).unwrap());
    assert_eq!(vals, vec![10.0, 16.0]);
    let mut labels: Vec<String> = f.dimension_glyphs(&vw.view).into_iter().map(|g| g.text).collect();
    labels.sort();
    assert_eq!(labels, vec!["10".to_string(), "16".to_string()]);
}

#[test]
fn a_conflicting_dimension_is_refused_and_the_sketch_is_unchanged() {
    let mut vw = dimensioned_by_tool();
    let (params, cons) = (face(&vw).sketch.params().to_vec(), live_constraints(face(&vw)));
    // A third dimension on the bottom line, 17 against its 16.
    click(&mut vw, (12.0, 0.0));
    assert!(typed(&mut vw, "17"));
    assert!(key(&mut vw, "Return"));
    let refusal = vw.view.design.refusal.clone().expect("refused");
    assert!(refusal.starts_with("Conflicting"), "refused by name: {refusal}");
    let f = face(&vw);
    let edit = f.edit.as_ref().expect("the refused dimension stays open");
    assert_eq!(edit.refusal.as_deref(), Some("Conflicting"));
    let g = f.dimension_glyphs(&vw.view).into_iter().find(|g| g.editing).expect("the edit is drawn");
    assert!(g.refused && g.text == "CONFLICTING", "{g:?}");
    // Exactly that constraint rolled back: the same constraints, the same geometry, still exact.
    assert_eq!(live_constraints(f), cons);
    let moved = f.sketch.params().iter().zip(&params).map(|(a, b)| (a - b).abs()).fold(0.0, f64::max);
    assert!(moved <= 1e-12, "the sketch moved {moved} mm");
    assert_eq!(f.sketch.params().len(), params.len());
    assert_eq!(f.dof, 0);
    assert_eq!(f.dims.len(), 2);
}

#[test]
fn a_typed_pull_gives_the_box_volume() {
    let mut vw = dimensioned_by_tool();
    assert!(typed(&mut vw, "h"));
    assert!(typed(&mut vw, "e"));
    assert!(vw.view.design.pull.is_some(), "e starts a pull");
    assert!(typed(&mut vw, "10"));
    let pull = vw.view.design.pull.as_ref().unwrap();
    assert_eq!(pull.distance, 10.0, "the typed distance drives the pull");
    assert!(pull.preview.is_some(), "a live preview at the typed distance");
    assert!(key(&mut vw, "Return"));
    let design = &vw.view.design;
    assert!(design.pull.is_none() && design.face.is_none(), "Enter commits the pull and closes the sketch");
    let vol = design.body.as_ref().expect("a body").volume().expect("volume");
    assert!((vol - 1600.0).abs() < 1e-4, "volume {vol}");
}
