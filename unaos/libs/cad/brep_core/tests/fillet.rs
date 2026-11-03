//! VUGBREP3 (SR502): fillets and chamfers of plane / plane edges. Each result validates, its mass.rs volume equals the
//! hand value to 1e-9 relative, and it tessellates to a closed manifold mesh (chi 2) whose volume is within chord x
//! area of the exact one; the blend is found by its Name; oversize, cornered and curved-pair edges refuse by name.

mod common;
use brep_core::primitives::{box_, cylinder};
use brep_core::{
    Brep, Curve, Entity, Id, Name, OpKind, Refusal, Report, Role, Surface, SurfaceKind, chamfer, difference, faces_named,
    fillet, properties, tessellate, validate,
};
use brep_core::{Tol, Vec3};
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

fn cube() -> Brep {
    box_(10.0, 10.0, 10.0, T).unwrap()
}

fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

/// The Id of the one edge whose two end points both satisfy `at`.
fn edge_where(b: &Brep, at: impl Fn(Vec3) -> bool) -> Id {
    let hits: Vec<Id> = b
        .edges
        .iter()
        .filter(|e| e.v.iter().all(|&v| at(b.vertices[v as usize].p)))
        .map(|e| e.id)
        .collect();
    assert_eq!(hits.len(), 1, "one edge matches");
    hits[0]
}

fn top_front(b: &Brep) -> Id {
    edge_where(b, |p| near(p.y, 0.0) && near(p.z, 10.0))
}

/// Validate, volume, face count, and the tessellation witness.
fn check(r: &Brep, volume: f64, faces: usize) -> Report {
    let rep = validate(r, T).expect("validate");
    assert_eq!(rep.faces, faces, "face count");
    let m = properties(r, T).expect("mass");
    close(m.volume, volume, "volume");
    let t = tessellate(r, CHORD, T).expect("tessellate");
    assert_eq!(manifold_chi(&t.mesh).expect("manifold"), 2);
    let mv = mesh_volume(&t.mesh);
    let area = brep_core::area(r).unwrap();
    assert!((mv - volume).abs() <= CHORD * area, "mesh volume {mv} vs {volume}");
    rep
}

fn blend_name(op: OpKind, edge: Id) -> Name {
    Name { op, role: Role::Blend, a: edge.0 as u32, b: 0 }
}

#[test]
fn one_cube_edge_filleted_r2() {
    let a = cube();
    let e = top_front(&a);
    let r = fillet(&a, &[e], 2.0, T).expect("fillet");
    // The corner square r x r less the quarter disc, swept along the 10 mm edge.
    let rep = check(&r, 1000.0 - (4.0 - PI) * 2.0 * 2.0 / 4.0 * 10.0, 7);
    assert_eq!((rep.vertices, rep.edges, rep.genus.clone()), (10, 15, vec![0]));
    let found = faces_named(&r, blend_name(OpKind::Fillet, e));
    assert_eq!(found.len(), 1, "one blend face carries the fillet Name");
    let f = &r.faces[found[0] as usize];
    assert_eq!(f.surface.kind(), SurfaceKind::Cylinder);
    assert!(!f.reversed, "a convex blend faces away from its axis");
    // Persistent references: every input face keeps its Id and Name; the filleted edge is gone.
    for src in &a.faces {
        match r.find(src.id) {
            Some(Entity::Face(i)) => assert_eq!(r.faces[i as usize].name, src.name),
            other => panic!("face {:?} lost: {other:?}", src.id),
        }
    }
    assert_eq!(r.find(e), None);
}

#[test]
fn one_cube_edge_filleted_r6_is_a_valid_part() {
    let a = cube();
    let r = fillet(&a, &[top_front(&a)], 6.0, T).expect("fillet");
    check(&r, 1000.0 - (4.0 - PI) * 36.0 / 4.0 * 10.0, 7);
}

#[test]
fn one_cube_edge_chamfered() {
    let a = cube();
    let e = top_front(&a);
    let r = chamfer(&a, &[e], 2.0, 3.0, T).expect("chamfer");
    check(&r, 1000.0 - 2.0 * 3.0 / 2.0 * 10.0, 7);
    let found = faces_named(&r, blend_name(OpKind::Chamfer, e));
    assert_eq!(found.len(), 1);
    assert_eq!(r.faces[found[0] as usize].surface.kind(), SurfaceKind::Plane);
}

#[test]
fn four_vertical_edges_filleted_together() {
    let a = cube();
    let ids: Vec<Id> = a
        .edges
        .iter()
        .filter(|e| {
            let (p, q) = (a.vertices[e.v[0] as usize].p, a.vertices[e.v[1] as usize].p);
            near(p.x, q.x) && near(p.y, q.y)
        })
        .map(|e| e.id)
        .collect();
    assert_eq!(ids.len(), 4);
    let r = fillet(&a, &ids, 1.0, T).expect("fillet");
    check(&r, 1000.0 - 4.0 * (1.0 - PI / 4.0) * 10.0, 10);
}

#[test]
fn concave_edge_of_an_l_block_filleted() {
    let a = cube();
    let tool = moved(box_(6.0, 6.0, 12.0, T).unwrap(), Vec3::new(5.0, 5.0, -1.0));
    let l = difference(&a, &tool, T).expect("difference");
    check(&l, 750.0, 8);
    let e = edge_where(&l, |p| near(p.x, 5.0) && near(p.y, 5.0));
    let r = fillet(&l, &[e], 2.0, T).expect("fillet");
    // A concave blend adds the corner square less the quarter disc.
    check(&r, 750.0 + (4.0 - PI) * 10.0, 9);
    let found = faces_named(&r, blend_name(OpKind::Fillet, e));
    assert_eq!(found.len(), 1);
    assert!(r.faces[found[0] as usize].reversed, "a concave blend faces its axis");
}

#[test]
fn radius_past_the_face_refuses() {
    let a = cube();
    let e = top_front(&a);
    match fillet(&a, &[e], 12.0, T) {
        Err(Refusal::RadiusTooLarge { edge, max }) => {
            assert_eq!(edge, e);
            close(max, 10.0, "max");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn two_parallel_fillets_that_meet_refuse() {
    let a = cube();
    let front = top_front(&a);
    let back = edge_where(&a, |p| near(p.y, 10.0) && near(p.z, 10.0));
    match fillet(&a, &[front, back], 6.0, T) {
        Err(Refusal::RadiusTooLarge { edge, max }) => {
            assert_eq!(edge, front);
            close(max, 5.0, "max");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn two_edges_at_one_vertex_refuse_by_vertex() {
    let a = cube();
    let front = top_front(&a);
    let left = edge_where(&a, |p| near(p.x, 0.0) && near(p.z, 10.0));
    let corner = a.vertices.iter().find(|v| near(v.p.x, 0.0) && near(v.p.y, 0.0) && near(v.p.z, 10.0)).unwrap().id;
    // VUGBREPCORNER (SR512): two of the corner's three edges listed is "partial".
    let got = fillet(&a, &[front, left], 1.0, T);
    assert_eq!(got, Err(Refusal::CornerNotBlended { vertex: corner, reason: "partial" }));
    let got = chamfer(&a, &[front, left], 1.0, 1.0, T);
    assert_eq!(got, Err(Refusal::CornerNotBlended { vertex: corner, reason: "partial" }));
}

#[test]
fn a_plane_cylinder_edge_refuses_by_pair() {
    let c = cylinder(2.0, 5.0, T).unwrap();
    let cap = c
        .edges
        .iter()
        .find(|e| matches!(e.curve, Curve::Circle { .. }) && near(c.vertices[e.v[0] as usize].p.z, 5.0))
        .expect("the top cap edge")
        .id;
    let got = fillet(&c, &[cap], 0.5, T);
    assert!(
        matches!(
            got,
            Err(Refusal::NotClosedForm { a: SurfaceKind::Plane, b: SurfaceKind::Cylinder })
                | Err(Refusal::NotClosedForm { a: SurfaceKind::Cylinder, b: SurfaceKind::Plane })
        ),
        "{got:?}"
    );
}

// VUGBREPCORNER (SR512): the box corner — the sphere octant for a fillet, the mitre triangle for a chamfer.

/// The vertex at `p` and the Ids of the edges ending there.
fn at_vertex(b: &Brep, p: Vec3) -> (Id, Vec<Id>) {
    let vi = b.vertices.iter().position(|v| v.p.dist(p) < 1e-9).expect("the vertex");
    let edges = b.edges.iter().filter(|e| e.v.contains(&(vi as u32))).map(|e| e.id).collect();
    (b.vertices[vi].id, edges)
}

fn corner_name(op: OpKind, vertex: Id) -> Name {
    Name { op, role: Role::Blend, a: vertex.0 as u32, b: 0 }
}

#[test]
fn all_twelve_edges_filleted_is_the_minkowski_box() {
    let a = cube();
    let ids: Vec<Id> = a.edges.iter().map(|e| e.id).collect();
    assert_eq!(ids.len(), 12);
    let (av, r) = (10.0, 1.0);
    let s = av - 2.0 * r;
    let vol = s * s * s + 3.0 * s * s * (2.0 * r) + 3.0 * s * (PI * r * r) + 4.0 / 3.0 * PI * r * r * r;
    let out = fillet(&a, &ids, r, T).expect("fillet");
    let rep = check(&out, vol, 26);
    assert_eq!((rep.vertices, rep.edges, rep.genus.clone()), (24, 48, vec![0]));
    let (corner, _) = at_vertex(&a, Vec3::new(0.0, 0.0, 0.0));
    let found = faces_named(&out, corner_name(OpKind::Fillet, corner));
    assert_eq!(found.len(), 1, "one patch carries the corner vertex's Name");
    let f = &out.faces[found[0] as usize];
    assert_eq!(f.surface.kind(), SurfaceKind::Sphere);
    assert!(!f.reversed, "a convex corner faces away from its centre");
    let spheres = out.faces.iter().filter(|f| f.surface.kind() == SurfaceKind::Sphere).count();
    let cylinders = out.faces.iter().filter(|f| f.surface.kind() == SurfaceKind::Cylinder).count();
    assert_eq!((spheres, cylinders), (8, 12));
}

#[test]
fn three_edges_at_one_vertex_filleted_r2() {
    let a = cube();
    let (_, ids) = at_vertex(&a, Vec3::new(0.0, 0.0, 0.0));
    assert_eq!(ids.len(), 3);
    let (av, r) = (10.0, 2.0);
    // Each edge loses (1 - pi/4) r^2 over a - r (from the corner setback to its far end); the corner cube r^3 keeps
    // the ball's eighth.
    let vol = av * av * av - 3.0 * (av - r) * (1.0 - PI / 4.0) * r * r - (1.0 - PI / 6.0) * r * r * r;
    let out = fillet(&a, &ids, r, T).expect("fillet");
    check(&out, vol, 10);
}

#[test]
fn all_twelve_edges_chamfered_cut_the_corners_with_triangles() {
    let a = cube();
    let ids: Vec<Id> = a.edges.iter().map(|e| e.id).collect();
    let (av, d) = (10.0, 1.0);
    // Each edge loses d^2 / 2 over a - 2d; each corner cube d^3 keeps the tetrahedron d^3 / 6.
    let vol = av * av * av - 12.0 * (d * d / 2.0) * (av - 2.0 * d) - 8.0 * (5.0 / 6.0) * d * d * d;
    let out = chamfer(&a, &ids, d, d, T).expect("chamfer");
    let rep = check(&out, vol, 26);
    assert_eq!((rep.vertices, rep.edges), (24, 48));
    let (corner, _) = at_vertex(&a, Vec3::new(10.0, 10.0, 10.0));
    let found = faces_named(&out, corner_name(OpKind::Chamfer, corner));
    assert_eq!(found.len(), 1);
    assert_eq!(out.faces[found[0] as usize].surface.kind(), SurfaceKind::Plane);
}

#[test]
fn unequal_chamfer_at_a_corner_refuses_by_reason() {
    let a = cube();
    let (corner, ids) = at_vertex(&a, Vec3::new(0.0, 0.0, 0.0));
    let got = chamfer(&a, &ids, 1.0, 2.0, T);
    assert_eq!(got, Err(Refusal::CornerNotBlended { vertex: corner, reason: "unequal-radii" }));
}

#[test]
fn oblique_corner_refuses_by_reason() {
    let a = cube();
    let c = chamfer(&a, &[top_front(&a)], 2.0, 2.0, T).expect("chamfer");
    // The chamfer plane meets the top face at 135 degrees at its trim vertex (0, 2, 10).
    let (corner, ids) = at_vertex(&c, Vec3::new(0.0, 2.0, 10.0));
    assert_eq!(ids.len(), 3);
    let got = fillet(&c, &ids, 0.5, T);
    assert_eq!(got, Err(Refusal::CornerNotBlended { vertex: corner, reason: "oblique" }));
}
