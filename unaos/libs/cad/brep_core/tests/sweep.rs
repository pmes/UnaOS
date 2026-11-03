//! VUGBREP1: extrude and revolve of sketch profiles (sketch_core's `Profile` shape): holes become inner loops, a
//! revolved rectangle is the cylinder, partial revolves validate, and the refusals are named.

mod common;
use brep_core::dep::{Arc2, Edge as PEdge, EntityId, Loop as PLoop, Profile, Tol, Vec2, Vec3};
use brep_core::primitives::cylinder;
use brep_core::{Frame, Refusal, Role, extrude, properties, revolve, tessellate, validate};
use common::{close, manifold_chi};
use std::f64::consts::{FRAC_PI_2, PI, TAU};

const T: Tol = Tol::DEFAULT;

fn lines(pts: &[(f64, f64)]) -> PLoop {
    let n = pts.len();
    let mut area = 0.0;
    let edges = (0..n)
        .map(|i| {
            let (a, b) = (pts[i], pts[(i + 1) % n]);
            area += (a.0 * b.1 - b.0 * a.1) / 2.0;
            PEdge::Line { from: Vec2::new(a.0, a.1), to: Vec2::new(b.0, b.1), src: EntityId(i as u32) }
        })
        .collect();
    PLoop { edges, area }
}

fn circle(cx: f64, cy: f64, r: f64, sweep: f64) -> PLoop {
    let arc = Arc2 { centre: Vec2::new(cx, cy), radius: r, start: 0.0, sweep };
    PLoop { edges: vec![PEdge::Arc { arc, src: EntityId(99) }], area: sweep.signum() * PI * r * r }
}

fn xz() -> Frame {
    Frame::new(Vec3::ZERO, -Vec3::Y, Vec3::X).unwrap()
}

fn rect(x0: f64, y0: f64, x1: f64, y1: f64) -> Vec<(f64, f64)> {
    vec![(x0, y0), (x1, y0), (x1, y1), (x0, y1)]
}

fn rev(mut v: Vec<(f64, f64)>) -> Vec<(f64, f64)> {
    v.reverse();
    v
}

#[test]
fn extruded_rectangle_with_a_rectangular_hole() {
    let p = Profile { outer: lines(&rect(0.0, 0.0, 10.0, 8.0)), holes: vec![lines(&rev(rect(2.0, 2.0, 5.0, 6.0)))] };
    let b = extrude(&p, &Frame::WORLD, Vec3::Z, 5.0, T).unwrap();
    let r = validate(&b, T).unwrap();
    assert_eq!((r.vertices, r.edges, r.faces, r.rings, r.genus.clone()), (16, 24, 10, 2, vec![1]));
    let top = b.faces.iter().position(|f| f.name.role == Role::CapEnd).unwrap();
    assert_eq!(b.face_loops(top as u32).count(), 2);
    assert_eq!(b.faces[top].inner.len(), 1);
    let m = properties(&b, T).unwrap();
    close(m.volume, (80.0 - 12.0) * 5.0, "volume");
    close(m.area, 2.0 * 68.0 + 36.0 * 5.0 + 14.0 * 5.0, "area");
    let t = tessellate(&b, 0.1, T).unwrap();
    assert_eq!(manifold_chi(&t.mesh).unwrap(), 0);
}

#[test]
fn extruded_square_with_a_circular_hole() {
    let p = Profile { outer: lines(&rect(0.0, 0.0, 10.0, 10.0)), holes: vec![circle(5.0, 5.0, 2.0, -TAU)] };
    let b = extrude(&p, &Frame::WORLD, Vec3::Z, 3.0, T).unwrap();
    let r = validate(&b, T).unwrap();
    assert_eq!((r.vertices, r.edges, r.faces, r.rings, r.genus.clone()), (10, 15, 7, 2, vec![1]));
    let m = properties(&b, T).unwrap();
    close(m.volume, (100.0 - 4.0 * PI) * 3.0, "volume");
    close(m.area, 2.0 * (100.0 - 4.0 * PI) + 40.0 * 3.0 + 2.0 * PI * 2.0 * 3.0, "area");
    close(m.centroid.x, 5.0, "cx");
    close(m.centroid.z, 1.5, "cz");
    let t = tessellate(&b, 0.02, T).unwrap();
    assert_eq!(manifold_chi(&t.mesh).unwrap(), 0);
    assert!(common::max_deviation(&b, &t) <= 0.02);
}

#[test]
fn extrude_below_the_plane_and_oblique() {
    let p = Profile { outer: lines(&rect(0.0, 0.0, 2.0, 3.0)), holes: vec![] };
    let down = extrude(&p, &Frame::WORLD, -Vec3::Z, 4.0, T).unwrap();
    validate(&down, T).unwrap();
    close(properties(&down, T).unwrap().volume, 24.0, "down volume");
    close(properties(&down, T).unwrap().centroid.z, -2.0, "down cz");
    let slant = extrude(&p, &Frame::WORLD, Vec3::new(0.0, 0.6, 0.8), 10.0, T).unwrap();
    validate(&slant, T).unwrap();
    close(properties(&slant, T).unwrap().volume, 48.0, "oblique volume");
}

#[test]
fn revolved_rectangle_is_the_cylinder() {
    let p = Profile { outer: lines(&rect(0.0, 0.0, 2.0, 5.0)), holes: vec![] };
    let b = revolve(&p, &xz(), Vec2::new(0.0, 0.0), Vec2::new(0.0, 1.0), TAU, T).unwrap();
    let c = cylinder(2.0, 5.0, T).unwrap();
    assert_eq!(validate(&b, T).unwrap(), validate(&c, T).unwrap());
    let (mb, mc) = (properties(&b, T).unwrap(), properties(&c, T).unwrap());
    close(mb.volume, mc.volume, "volume");
    close(mb.area, mc.area, "area");
    close(mb.volume, PI * 4.0 * 5.0, "closed form");
}

#[test]
fn washer_has_genus_one() {
    let p = Profile { outer: lines(&rect(3.0, 0.0, 5.0, 2.0)), holes: vec![] };
    let b = revolve(&p, &xz(), Vec2::new(0.0, 0.0), Vec2::new(0.0, 1.0), TAU, T).unwrap();
    let r = validate(&b, T).unwrap();
    assert_eq!((r.vertices, r.edges, r.faces, r.rings, r.genus.clone()), (4, 6, 4, 2, vec![1]));
    let m = properties(&b, T).unwrap();
    close(m.volume, PI * (25.0 - 9.0) * 2.0, "volume");
    close(m.area, 64.0 * PI, "area");
    let t = tessellate(&b, 0.05, T).unwrap();
    assert_eq!(manifold_chi(&t.mesh).unwrap(), 0);
}

#[test]
fn partial_revolves_validate() {
    let p = Profile { outer: lines(&rect(0.0, 0.0, 2.0, 5.0)), holes: vec![] };
    let b = revolve(&p, &xz(), Vec2::new(0.0, 0.0), Vec2::new(0.0, 1.0), FRAC_PI_2, T).unwrap();
    let r = validate(&b, T).unwrap();
    assert_eq!((r.vertices, r.edges, r.faces, r.genus.clone()), (6, 9, 5, vec![0]));
    let m = properties(&b, T).unwrap();
    close(m.volume, 5.0 * PI, "quarter cylinder volume");
    close(m.area, 20.0 + 7.0 * PI, "quarter cylinder area");
    let t = tessellate(&b, 0.05, T).unwrap();
    assert_eq!(manifold_chi(&t.mesh).unwrap(), 2);

    let w = Profile { outer: lines(&rect(3.0, 0.0, 5.0, 2.0)), holes: vec![] };
    let b = revolve(&w, &xz(), Vec2::new(0.0, 0.0), Vec2::new(0.0, 1.0), FRAC_PI_2, T).unwrap();
    let r = validate(&b, T).unwrap();
    assert_eq!((r.vertices, r.edges, r.faces, r.genus.clone()), (8, 12, 6, vec![0]));
    close(properties(&b, T).unwrap().volume, 8.0 * PI, "quarter washer volume");
}

#[test]
fn named_refusals() {
    let crossing = Profile { outer: lines(&rect(-1.0, 0.0, 2.0, 3.0)), holes: vec![] };
    assert_eq!(
        revolve(&crossing, &xz(), Vec2::new(0.0, 0.0), Vec2::new(0.0, 1.0), TAU, T),
        Err(Refusal::ProfileCrossesAxis)
    );
    let holed = Profile { outer: lines(&rect(0.0, 0.0, 10.0, 10.0)), holes: vec![circle(5.0, 5.0, 2.0, -TAU)] };
    assert_eq!(extrude(&holed, &Frame::WORLD, Vec3::new(0.3, 0.0, 1.0), 3.0, T), Err(Refusal::ObliqueArc));
    let cw = Profile { outer: lines(&rev(rect(0.0, 0.0, 1.0, 1.0))), holes: vec![] };
    assert_eq!(extrude(&cw, &Frame::WORLD, Vec3::Z, 1.0, T), Err(Refusal::ProfileOrientation));
}
