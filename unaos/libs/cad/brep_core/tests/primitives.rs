//! VUGBREP1: each primitive validates with its Euler counts, matches its closed-form mass properties to 1e-9
//! relative, and tessellates to a closed manifold mesh within the stated chord.

mod common;
use brep_core::Tol;
use brep_core::primitives::{box_, cone, cylinder, sphere, torus};
use brep_core::{Brep, Entity, Report, properties, tessellate, validate};
use common::{close, manifold_chi, max_deviation, mesh_volume};
use std::f64::consts::PI;

const T: Tol = Tol::DEFAULT;

fn report(v: usize, e: usize, f: usize, rings: usize, genus: i64) -> (usize, usize, usize, usize, Vec<i64>) {
    (v, e, f, rings, vec![genus])
}

fn counts(r: &Report) -> (usize, usize, usize, usize, Vec<i64>) {
    (r.vertices, r.edges, r.faces, r.rings, r.genus.clone())
}

fn mesh_checks(b: &Brep, chord: f64, chi: i64, volume: f64) {
    let t = tessellate(b, chord, T).expect("tessellate");
    assert_eq!(t.face_of.len(), t.mesh.triangles.len());
    for id in &t.face_of {
        assert!(matches!(b.find(*id), Some(Entity::Face(_))));
    }
    assert_eq!(manifold_chi(&t.mesh).expect("manifold"), chi);
    let dev = max_deviation(b, &t);
    assert!(dev <= chord, "deviation {dev} > chord {chord}");
    // Every mesh point lies within `chord` of the surface, so the volumes differ by at most chord x area.
    let mv = mesh_volume(&t.mesh);
    let area = brep_core::area(b).unwrap();
    assert!(mv > 0.0 && (mv - volume).abs() <= chord * area, "mesh volume {mv} vs {volume}");
}

#[test]
fn box_closed_form() {
    let b = box_(2.0, 3.0, 4.0, T).unwrap();
    assert_eq!(counts(&validate(&b, T).unwrap()), report(8, 12, 6, 0, 0));
    let m = properties(&b, T).unwrap();
    close(m.volume, 24.0, "volume");
    close(m.area, 52.0, "area");
    close(m.centroid.x, 1.0, "cx");
    close(m.centroid.y, 1.5, "cy");
    close(m.centroid.z, 2.0, "cz");
    close(m.inertia[0][0], 50.0, "Ixx");
    close(m.inertia[1][1], 40.0, "Iyy");
    close(m.inertia[2][2], 26.0, "Izz");
    for (i, j) in [(0, 1), (0, 2), (1, 2)] {
        assert!(m.inertia[i][j].abs() < 1e-9 * 50.0, "I{i}{j} = {}", m.inertia[i][j]);
    }
    mesh_checks(&b, 0.05, 2, 24.0);
}

#[test]
fn cylinder_closed_form() {
    let (r, h) = (2.0, 5.0);
    let b = cylinder(r, h, T).unwrap();
    assert_eq!(counts(&validate(&b, T).unwrap()), report(2, 3, 3, 0, 0));
    let m = properties(&b, T).unwrap();
    let v = PI * r * r * h;
    close(m.volume, v, "volume");
    close(m.area, 2.0 * PI * r * h + 2.0 * PI * r * r, "area");
    assert!(m.centroid.x.abs() < 1e-9 && m.centroid.y.abs() < 1e-9);
    close(m.centroid.z, h / 2.0, "cz");
    close(m.inertia[2][2], v * r * r / 2.0, "Izz");
    close(m.inertia[0][0], v * (3.0 * r * r + h * h) / 12.0, "Ixx");
    close(m.inertia[1][1], v * (3.0 * r * r + h * h) / 12.0, "Iyy");
    mesh_checks(&b, 0.05, 2, v);
}

#[test]
fn cone_closed_form() {
    let (r, h) = (3.0, 4.0);
    let b = cone(r, 0.0, h, T).unwrap();
    assert_eq!(counts(&validate(&b, T).unwrap()), report(2, 2, 2, 0, 0));
    let m = properties(&b, T).unwrap();
    let v = PI * r * r * h / 3.0;
    close(m.volume, v, "volume");
    close(m.area, PI * r * r + PI * r * 5.0, "area");
    close(m.centroid.z, h / 4.0, "cz");
    close(m.inertia[2][2], 0.3 * v * r * r, "Izz");
    mesh_checks(&b, 0.05, 2, v);
}

#[test]
fn frustum_closed_form() {
    let (r1, r2, h) = (3.0, 1.0, 4.0);
    let b = cone(r1, r2, h, T).unwrap();
    assert_eq!(counts(&validate(&b, T).unwrap()), report(2, 3, 3, 0, 0));
    let m = properties(&b, T).unwrap();
    let v = PI * h / 3.0 * (r1 * r1 + r1 * r2 + r2 * r2);
    close(m.volume, v, "volume");
    let slant = ((r1 - r2) * (r1 - r2) + h * h).sqrt();
    close(m.area, PI * (r1 * r1 + r2 * r2) + PI * (r1 + r2) * slant, "area");
    let cz = h * (r1 * r1 + 2.0 * r1 * r2 + 3.0 * r2 * r2) / (4.0 * (r1 * r1 + r1 * r2 + r2 * r2));
    close(m.centroid.z, cz, "cz");
    mesh_checks(&b, 0.05, 2, v);
}

#[test]
fn sphere_closed_form() {
    let r = 3.0;
    let b = sphere(r, T).unwrap();
    assert_eq!(counts(&validate(&b, T).unwrap()), report(2, 1, 1, 0, 0));
    let m = properties(&b, T).unwrap();
    let v = 4.0 / 3.0 * PI * r * r * r;
    close(m.volume, v, "volume");
    close(m.area, 4.0 * PI * r * r, "area");
    for i in 0..3 {
        close(m.inertia[i][i], 0.4 * v * r * r, "I");
    }
    assert!(m.centroid.x.abs() < 1e-9 && m.centroid.y.abs() < 1e-9 && m.centroid.z.abs() < 1e-9);
    mesh_checks(&b, 0.05, 2, v);
}

#[test]
fn torus_closed_form() {
    let (big, small) = (5.0, 2.0);
    let b = torus(big, small, T).unwrap();
    assert_eq!(counts(&validate(&b, T).unwrap()), report(1, 2, 1, 0, 1));
    let m = properties(&b, T).unwrap();
    let v = 2.0 * PI * PI * big * small * small;
    close(m.volume, v, "volume");
    close(m.area, 4.0 * PI * PI * big * small, "area");
    close(m.inertia[2][2], v * (big * big + 0.75 * small * small), "Izz");
    close(m.inertia[0][0], v * (0.5 * big * big + 0.625 * small * small), "Ixx");
    mesh_checks(&b, 0.05, 0, v);
}

#[test]
fn ids_are_deterministic_and_resolve() {
    let (a, b) = (sphere(3.0, T).unwrap(), sphere(3.0, T).unwrap());
    assert_eq!(a, b);
    let (ta, tb) = (tessellate(&a, 0.05, T).unwrap(), tessellate(&b, 0.05, T).unwrap());
    assert_eq!(ta.face_of, tb.face_of);
    let bits = |t: &brep_core::Tessellation| -> Vec<u64> {
        t.mesh.positions.iter().flat_map(|p| [p.x.to_bits(), p.y.to_bits(), p.z.to_bits()]).collect()
    };
    assert_eq!(bits(&ta), bits(&tb));
    let c = box_(1.0, 1.0, 1.0, T).unwrap();
    for (i, f) in c.faces.iter().enumerate() {
        assert_eq!(c.find(f.id), Some(Entity::Face(i as u32)));
        assert_eq!(c.name(f.id), Some(f.name));
    }
    for (i, e) in c.edges.iter().enumerate() {
        assert_eq!(c.find(e.id), Some(Entity::Edge(i as u32)));
    }
    for (i, v) in c.vertices.iter().enumerate() {
        assert_eq!(c.find(v.id), Some(Entity::Vertex(i as u32)));
    }
}

#[test]
fn bad_primitive_parameters_are_refused() {
    use brep_core::Refusal;
    assert_eq!(box_(0.0, 1.0, 1.0, T), Err(Refusal::BadParam("box dx")));
    assert_eq!(torus(1.0, 2.0, T), Err(Refusal::BadParam("torus major <= minor")));
    assert_eq!(sphere(-1.0, T), Err(Refusal::BadParam("sphere r")));
}
