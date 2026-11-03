// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! VUGSCANDEVIATION (SR510): the SR469 fixture scan measured against brep_core designs built to the fixture's
//! dimensions — aligned from a perturbed start, a design 2 mm too wide a side named by its faces, a scan with no
//! overlap refused — and mesh_core's `Bvh::closest` against brute force.

use core::f64::consts::PI;
use std::sync::OnceLock;

use brep_core::geom_core::{Quat, closest_point_triangle};
use brep_core::{SurfaceKind, primitives, tessellate};
use mesh_core::bvh::Bvh;
use scan_core::fixture::{self, Noise, Scene};
use scan_core::{AlignParams, DepthScale, Design, DeviationError, Mesh, ScanMesh, Tol, Transform, Tsdf, Vec3, extract, measure};

const DEG: f64 = PI / 180.0;
const CENTRE: Vec3 = Vec3::new(-20.0, 0.0, 50.0);
const CHORD: f64 = 0.05;
const BAND: f64 = 1.0;

/// tests/scan.rs's fusion: 12 orbit frames at 600 mm, 40° up, D435i noise.
fn scan() -> &'static ScanMesh {
    static S: OnceLock<ScanMesh> = OnceLock::new();
    S.get_or_init(|| {
        let (scene, intr, scale) = (Scene::bench(), fixture::bench_intrinsics(), DepthScale::D400_DEFAULT);
        let mut t = Tsdf::new(4.0, 12.0, Tol::DEFAULT).expect("volume");
        for k in 0..12u32 {
            let p = fixture::orbit(CENTRE, 600.0, 40.0 * DEG, k as f64 * 2.0 * PI / 12.0);
            let f = fixture::render_depth(&scene, &intr, scale, &p, &Noise::D435I, k as u64);
            t.integrate(&f, scale, &intr, &p).expect("frame integrates");
        }
        extract(&t).expect("surface")
    })
}

/// One face of the design: its surface kind, its mean outward normal and its centroid (from its triangles).
#[derive(Clone, Copy, Debug)]
struct FaceInfo {
    kind: SurfaceKind,
    normal: Vec3,
    centre: Vec3,
}

/// The fixture's box (x from `x0`, `dx` wide) and cylinder tessellated, moved into place and concatenated.
fn design(x0: f64, dx: f64) -> (Design, Vec<FaceInfo>) {
    let tol = Tol::DEFAULT;
    let solids = [
        (primitives::box_(dx, 200.0, 100.0, tol).expect("box"), Vec3::new(x0, -100.0, 0.0)),
        (primitives::cylinder(40.0, 150.0, tol).expect("cylinder"), Vec3::new(160.0, 0.0, 0.0)),
    ];
    let mut mesh = Mesh::default();
    let mut face_of = Vec::new();
    let mut kinds = Vec::new();
    for (b, at) in &solids {
        let t = tessellate(b, CHORD, tol).expect("tessellates");
        let first = kinds.len() as u32;
        kinds.extend(b.faces.iter().map(|f| f.surface.kind()));
        let base = mesh.positions.len() as u32;
        mesh.positions.extend(t.mesh.positions.iter().map(|p| *p + *at));
        for (tri, id) in t.mesh.triangles.iter().zip(&t.face_of) {
            let f = b.faces.iter().position(|f| f.id == *id).expect("face id") as u32;
            mesh.triangles.push([tri[0] + base, tri[1] + base, tri[2] + base]);
            face_of.push(first + f);
        }
    }
    let nf = kinds.len();
    let normals = mesh.normals_face(tol);
    let mut acc = vec![(Vec3::ZERO, Vec3::ZERO, 0.0f64); nf];
    for (t, &f) in face_of.iter().enumerate() {
        let [a, b, c] = mesh.corners(t).unwrap();
        let area = (b - a).cross(c - a).mag() * 0.5;
        acc[f as usize].0 = acc[f as usize].0 + normals[t] * area;
        acc[f as usize].1 = acc[f as usize].1 + (a + b + c) * (area / 3.0);
        acc[f as usize].2 += area;
    }
    let info = (0..nf)
        .map(|f| FaceInfo {
            kind: kinds[f],
            normal: acc[f].0.normalize().unwrap_or(Vec3::ZERO),
            centre: acc[f].1 * (1.0 / acc[f].2.max(1e-12)),
        })
        .collect();
    (Design::new(mesh, face_of, nf as u32).expect("design"), info)
}

/// The scan moved by `t` (positions and normals).
fn moved(s: &ScanMesh, t: Transform) -> ScanMesh {
    let mut m = s.clone();
    for p in &mut m.mesh.positions {
        *p = t.apply_point(*p);
    }
    for n in &mut m.normals {
        *n = t.apply_vector(*n);
    }
    m
}

fn perturbation() -> Transform {
    let rot = Quat::from_axis_angle(Vec3::new(0.2, -0.1, 1.0), 3.0 * DEG).unwrap();
    Transform::new(rot, 1.0, Vec3::new(8.0, -5.0, 4.0)).unwrap()
}

#[test]
fn a_perturbed_scan_aligns_to_its_design_and_the_cylinder_holds_its_radius() {
    let (d, info) = design(-230.0, 300.0);
    let tp = perturbation();
    let s = moved(scan(), tp);
    let dev = measure(&s, &d, Transform::IDENTITY, &AlignParams::DEFAULT, BAND, Tol::DEFAULT).expect("aligns");
    let st = dev.stats;
    println!(
        "deviation: aligned rms={:.3} max={:.3} p95={:.3} inside={:.4} matched={} unmatched={} align_rms={:.3} pairs={} iters={}",
        st.rms_mm, st.max_mm, st.p95_mm, st.inside, st.matched, st.unmatched, dev.aligned.rms_mm, dev.aligned.pairs, dev.aligned.iters
    );
    for f in &dev.faces {
        println!("face: {} {:?} n={} max={:.3} rms={:.3} mean={:.3}", f.face, info[f.face as usize].kind, f.n, f.max_mm, f.rms_mm, f.mean_mm);
    }
    assert!(dev.aligned.rms_mm < 1.0, "aligned rms {}", dev.aligned.rms_mm);
    assert!(st.rms_mm < 1.0, "deviation rms {}", st.rms_mm);
    // The pose undoes the perturbation: every probe comes back within 1 mm.
    for p in [Vec3::new(-230.0, -100.0, 100.0), Vec3::new(70.0, 100.0, 0.0), Vec3::new(160.0, 0.0, 150.0)] {
        let back = dev.aligned.pose.apply_point(tp.apply_point(p));
        assert!((back - p).mag() < 1.0, "probe {p:?} back at {back:?}");
    }
    let x = dev.aligned.pose.apply_vector(tp.apply_vector(Vec3::X));
    let z = dev.aligned.pose.apply_vector(tp.apply_vector(Vec3::Z));
    assert!(libm::acos(x.dot(Vec3::X).min(1.0)) / DEG < 0.3 && libm::acos(z.dot(Vec3::Z).min(1.0)) / DEG < 0.3, "rotation left");
    // The cylinder's side: its signed mean is the radius error, inside the scan's sigma.
    let sigma = AlignParams::DEFAULT.sigma_mm;
    let cyl = dev.faces.iter().find(|f| info[f.face as usize].kind == SurfaceKind::Cylinder).expect("cylinder face");
    assert!(cyl.n > 200, "cylinder vertices {}", cyl.n);
    assert!(cyl.mean_mm.abs() < sigma && cyl.rms_mm < sigma, "cylinder mean {} rms {}", cyl.mean_mm, cyl.rms_mm);
    // The bottoms stand on the floor: no vertex is theirs.
    assert!(dev.faces.iter().filter(|f| info[f.face as usize].normal.z < -0.99).all(|f| f.n == 0));
    assert_eq!(dev.per_vertex.len(), s.mesh.positions.len());
    assert_eq!(st.matched + st.unmatched, s.mesh.positions.len() as u32);
}

#[test]
fn a_design_two_mm_too_wide_a_side_names_its_two_side_faces() {
    let (d, info) = design(-232.0, 304.0);
    let dev = measure(scan(), &d, Transform::IDENTITY, &AlignParams::DEFAULT, BAND, Tol::DEFAULT).expect("aligns");
    for f in &dev.faces {
        let i = info[f.face as usize];
        println!("face: {} {:?} n={:?} n={} max={:.3} rms={:.3} mean={:.3}", f.face, i.kind, i.normal, f.n, f.max_mm, f.rms_mm, f.mean_mm);
    }
    let is_x_side = |f: u32| info[f as usize].kind == SurfaceKind::Plane && info[f as usize].normal.x.abs() > 0.99;
    let first: Vec<u32> = dev.faces.iter().take(2).map(|f| f.face).collect();
    assert!(first.iter().all(|&f| is_x_side(f)), "worst two faces {first:?}");
    for f in dev.faces.iter().filter(|f| is_x_side(f.face)) {
        assert!((f.mean_mm + 2.0).abs() < 0.6, "x side {} mean {}", f.face, f.mean_mm);
        assert!(f.max_mm.abs() >= 1.5, "x side {} max {}", f.face, f.max_mm);
    }
    let top = dev
        .faces
        .iter()
        .find(|f| info[f.face as usize].normal.z > 0.99 && (info[f.face as usize].centre.z - 100.0).abs() < 1.0)
        .expect("box top");
    assert!(top.mean_mm.abs() < 0.6, "box top mean {}", top.mean_mm);
}

#[test]
fn a_scan_with_no_overlap_is_refused_no_alignment() {
    let (d, _) = design(-230.0, 300.0);
    let far = moved(scan(), Transform::from_translation(Vec3::new(2000.0, 0.0, 0.0)));
    match measure(&far, &d, Transform::IDENTITY, &AlignParams::DEFAULT, BAND, Tol::DEFAULT) {
        Err(DeviationError::NoAlignment { pairs: 0, .. }) => {}
        other => panic!("expected NoAlignment with no pair, got {:?}", other.map(|d| d.stats)),
    }
}

#[test]
fn bvh_closest_matches_brute_force() {
    let tol = Tol::DEFAULT;
    let b = primitives::box_(1.0, 1.0, 1.0, tol).unwrap();
    let mesh = tessellate(&b, CHORD, tol).unwrap().mesh;
    let bvh = Bvh::build(&mesh);
    let kat = [
        (Vec3::new(0.5, 0.5, 2.0), Vec3::new(0.5, 0.5, 1.0)),
        (Vec3::new(2.0, 2.0, 0.5), Vec3::new(1.0, 1.0, 0.5)),
        (Vec3::new(2.0, 2.0, 2.0), Vec3::new(1.0, 1.0, 1.0)),
        (Vec3::new(0.5, 0.5, 0.4), Vec3::new(0.5, 0.5, 0.0)),
    ];
    for (p, want) in kat {
        let n = bvh.closest(&mesh, p, 10.0, tol).expect("within reach");
        assert!((n.point - want).mag() < 1e-12, "{p:?}: {:?} want {want:?}", n.point);
        assert!((n.dist - (p - want).mag()).abs() < 1e-12);
    }
    assert!(bvh.closest(&mesh, Vec3::new(0.5, 0.5, 5.0), 1.0, tol).is_none(), "beyond reach");
    for i in 0..343 {
        let p = Vec3::new((i % 7) as f64 * 0.5 - 1.0, ((i / 7) % 7) as f64 * 0.5 - 1.0, (i / 49) as f64 * 0.5 - 1.0);
        let brute = (0..mesh.triangles.len())
            .filter_map(|t| {
                let [a, b, c] = mesh.corners(t)?;
                closest_point_triangle(p, a, b, c, tol).ok().map(|(q, _)| (q - p).mag())
            })
            .fold(f64::INFINITY, f64::min);
        let n = bvh.closest(&mesh, p, 100.0, tol).expect("within reach");
        assert!((n.dist - brute).abs() < 1e-12, "{p:?}: bvh {} brute {brute}", n.dist);
    }
}
