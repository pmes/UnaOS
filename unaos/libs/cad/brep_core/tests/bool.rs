//! VUGBREP2 (SR481): booleans over plane and cylinder faces. Each result validates, its mass.rs volume equals the
//! hand value to 1e-9 relative, and it tessellates to a closed manifold mesh (chi = 2 - 2 genus per shell) whose
//! volume is within chord x area of the exact one; the persistent face Name survives a cut; contacts refuse by name.

mod common;
use brep_core::{Tol, Vec3};
use brep_core::primitives::{box_, cylinder};
use brep_core::{
    Brep, Curve, OpKind, Refusal, Report, Role, Surface, SurfaceKind, difference, faces_named, intersection, properties,
    tessellate, union, validate,
};
use common::{close, manifold_chi, mesh_volume};
use std::f64::consts::PI;

const T: Tol = Tol::DEFAULT;
const CHORD: f64 = 0.05;

/// A test-only translation of a Brep (vertices, curve origins, surface frames).
fn moved(mut b: Brep, d: Vec3) -> Brep {
    for v in &mut b.vertices {
        v.p = v.p + d;
    }
    for e in &mut b.edges {
        match &mut e.curve {
            Curve::Line { o, .. } => *o = *o + d,
            Curve::Circle { f, .. } | Curve::Ellipse { f, .. } => f.o = f.o + d,
            Curve::Nurbs(_) => panic!("no NURBS edge in a primitive"),
        }
    }
    for f in &mut b.faces {
        match &mut f.surface {
            Surface::Plane { f }
            | Surface::Cylinder { f, .. }
            | Surface::Cone { f, .. }
            | Surface::Sphere { f, .. }
            | Surface::Torus { f, .. } => f.o = f.o + d,
        }
    }
    b
}

fn cube(a: f64, at: (f64, f64, f64)) -> Brep {
    moved(box_(a, a, a, T).unwrap(), Vec3::new(at.0, at.1, at.2))
}

/// Validate, volume, face count, and the tessellation witness.
fn check(r: &Brep, volume: f64, faces: usize) -> Report {
    let rep = validate(r, T).expect("validate");
    assert_eq!(rep.faces, faces, "face count");
    let m = properties(r, T).expect("mass");
    close(m.volume, volume, "volume");
    let t = tessellate(r, CHORD, T).expect("tessellate");
    let chi: i64 = rep.genus.iter().map(|g| 2 - 2 * g).sum();
    assert_eq!(manifold_chi(&t.mesh).expect("manifold"), chi);
    let mv = mesh_volume(&t.mesh);
    let area = brep_core::area(r).unwrap();
    assert!((mv - volume).abs() <= CHORD * area, "mesh volume {mv} vs {volume}");
    rep
}

#[test]
fn cube_minus_a_cube_through_one_face() {
    let a = cube(2.0, (0.0, 0.0, 0.0));
    let b = moved(box_(1.0, 1.0, 2.0, T).unwrap(), Vec3::new(0.5, 0.5, 1.0));
    let r = difference(&a, &b, T).expect("difference");
    // A's five untouched faces, its top with a square hole, B's bottom and four lower side pieces (flipped).
    let rep = check(&r, 8.0 - 1.0, 11);
    assert_eq!((rep.vertices, rep.edges, rep.rings, rep.genus.clone()), (16, 24, 1, vec![0]));
    assert_eq!(r.solids.len(), 1);
    // The four section edges are named by the boolean.
    let sections = r.edges.iter().filter(|e| e.name.op == OpKind::Boolean && e.name.role == Role::Section).count();
    assert_eq!(sections, 4);
}

#[test]
fn cube_union_an_offset_cube() {
    let a = cube(2.0, (0.0, 0.0, 0.0));
    let b = cube(2.0, (1.0, 1.0, 1.0));
    let r = union(&a, &b, T).expect("union");
    check(&r, 8.0 + 8.0 - 1.0, 12);
}

#[test]
fn cube_intersect_an_offset_cube() {
    let a = cube(2.0, (0.0, 0.0, 0.0));
    let b = cube(2.0, (1.0, 1.0, 1.0));
    let r = intersection(&a, &b, T).expect("intersection");
    check(&r, 1.0, 6);
}

#[test]
fn plate_minus_a_through_cylinder_keeps_the_cylinder_name() {
    let plate = box_(4.0, 4.0, 1.0, T).unwrap();
    let tool = moved(cylinder(1.0, 3.0, T).unwrap(), Vec3::new(2.0, 2.0, -1.0));
    let side = tool.faces.iter().find(|f| f.surface.kind() == SurfaceKind::Cylinder).expect("cylinder face");
    let (name, was_reversed) = (side.name, side.reversed);
    let r = difference(&plate, &tool, T).expect("difference");
    // Six plate faces (top and bottom with a round hole) and the cylinder band, flipped: genus 1.
    let rep = check(&r, 16.0 - PI, 7);
    assert_eq!((rep.rings, rep.genus.clone()), (2, vec![1]));
    let found = faces_named(&r, name);
    assert_eq!(found.len(), 1, "one face carries the tool's cylinder Name");
    let f = &r.faces[found[0] as usize];
    assert_eq!(f.surface.kind(), SurfaceKind::Cylinder);
    assert_eq!(f.reversed, !was_reversed, "the kept tool face is flipped");
}

#[test]
fn disjoint_union_is_two_solids() {
    let a = cube(1.0, (0.0, 0.0, 0.0));
    let b = cube(1.0, (3.0, 0.0, 0.0));
    let r = union(&a, &b, T).expect("union");
    let rep = check(&r, 2.0, 12);
    assert_eq!((r.solids.len(), rep.shells, rep.genus.clone()), (2, 2, vec![0, 0]));
}

#[test]
fn disjoint_intersection_is_empty() {
    let a = cube(1.0, (0.0, 0.0, 0.0));
    let b = cube(1.0, (3.0, 0.0, 0.0));
    let r = intersection(&a, &b, T).expect("intersection");
    assert!(r.faces.is_empty() && r.solids.is_empty());
}

#[test]
fn coplanar_union_refuses_by_name() {
    let a = cube(2.0, (0.0, 0.0, 0.0));
    let b = moved(box_(2.0, 2.0, 2.0, T).unwrap(), Vec3::new(1.0, 1.0, 0.0));
    let got = union(&a, &b, T);
    assert!(matches!(got, Err(Refusal::Contact { what: "coincident faces", .. })), "{got:?}");
}
