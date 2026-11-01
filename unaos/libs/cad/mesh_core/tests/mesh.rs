// VUGMESH (SR466): the mesh model — measures, half-edge topology, validate, weld, BVH pick, the f32 buffers.

mod common;

use common::{cube, sphere};
use mesh_core::{Bvh, HalfEdge, Invalid, Mesh, Ray, Tol, Topo, Vec3};

const T: Tol = Tol::DEFAULT;

#[test]
fn unit_cube_measures() {
    let m = cube();
    assert_eq!(m.triangles.len(), 12);
    assert_eq!(m.validate(T), Ok(()));
    assert!((m.area() - 6.0).abs() < 1e-12);
    assert!((m.volume(T).unwrap() - 1.0).abs() < 1e-12);
    assert_eq!(m.euler_characteristic(), 2);
    let he = HalfEdge::build(&m).unwrap();
    assert!(he.is_closed());
    assert_eq!(he.boundary_edges(), 0);
    assert!(m.boundary_loops().unwrap().is_empty());
    let b = m.aabb().unwrap();
    assert_eq!((b.min, b.max), (Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 1.0)));
}

#[test]
fn cube_face_normals_and_vertex_normals() {
    let m = cube();
    let fnorm = m.normals_face(T);
    assert_eq!(fnorm[0], Vec3::new(0.0, 0.0, -1.0));
    assert_eq!(fnorm[2], Vec3::new(0.0, 0.0, 1.0));
    // corner vertex 7 = (1,1,1): angle-weighted over three faces each at total angle pi/2 -> (1,1,1)/sqrt(3)
    let vn = m.normals_vertex(T);
    let s = 1.0 / 3f64.sqrt();
    assert!((vn[7] - Vec3::new(s, s, s)).magnitude() < 1e-12, "{:?}", vn[7]);
    assert!((vn[0] - Vec3::new(-s, -s, -s)).magnitude() < 1e-12, "{:?}", vn[0]);
}

#[test]
fn cube_minus_a_face_has_one_boundary_loop() {
    let mut m = cube();
    m.triangles.retain(|t| *t != [4, 5, 7] && *t != [4, 7, 6]);
    let loops = m.boundary_loops().unwrap();
    assert_eq!(loops.len(), 1);
    let mut lp = loops[0].clone();
    lp.sort_unstable();
    assert_eq!(lp, vec![4, 5, 6, 7]);
    assert_eq!(m.euler_characteristic(), 1);
    assert_eq!(m.volume(T), Err(Topo::NotClosed { boundary_edges: 4 }));
}

#[test]
fn validate_names_every_refusal() {
    let mut m = cube();
    m.triangles.push([0, 1, 9]);
    assert_eq!(m.validate(T), Err(Invalid::IndexOutOfRange { tri: 12, index: 9 }));

    let mut m = cube();
    m.positions.push(Vec3::new(2.0, 0.0, 0.0)); // 8: collinear with 0 and 1
    m.triangles.push([0, 1, 8]);
    assert_eq!(m.validate(T), Err(Invalid::Degenerate { tri: 12 }));

    let mut m = cube();
    m.triangles.push([3, 1, 0]); // the vertex set of tri 1, reversed
    assert_eq!(m.validate(T), Err(Invalid::Duplicate { tri: 12, of: 1 }));

    let mut m = cube();
    m.positions[3].y = f64::NAN;
    assert_eq!(m.validate(T), Err(Invalid::NonFinite { vertex: 3 }));
}

#[test]
fn half_edge_refusals_are_named() {
    let mut m = cube();
    m.triangles.push([0, 1, 4]); // a third triangle on edge 0-1
    assert_eq!(HalfEdge::build(&m), Err(Topo::NonManifold { edge: [0, 1], uses: 3 }));

    let mut m = cube();
    m.triangles[1] = [0, 1, 3]; // flipped: runs 0->1 like tri 4 does... and 3->0 like tri 0's 3->0
    assert!(matches!(HalfEdge::build(&m), Err(Topo::Inconsistent { .. })));

    let m = Mesh { positions: cube().positions, triangles: vec![[0, 1, 9]] };
    assert_eq!(HalfEdge::build(&m), Err(Topo::IndexOutOfRange { tri: 0, index: 9 }));

    // two triangles touching at one vertex only: a bowtie
    let m = Mesh {
        positions: vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(-1.0, -1.0, 0.0),
        ],
        triangles: vec![[0, 1, 2], [0, 3, 4]],
    };
    assert_eq!(HalfEdge::build(&m), Err(Topo::NonManifoldVertex { v: 0 }));
}

#[test]
fn weld_a_soup_of_duplicated_vertices() {
    let c = cube();
    let mut soup = Mesh::default();
    for (k, t) in c.triangles.iter().enumerate() {
        for (j, &i) in t.iter().enumerate() {
            // jitter some copies well inside Tol.lin (1e-6 mm)
            let e = if (k + j) % 3 == 0 { 3e-7 } else { 0.0 };
            soup.positions.push(c.positions[i as usize] + Vec3::new(e, -e, e));
        }
        let b = 3 * k as u32;
        soup.triangles.push([b, b + 1, b + 2]);
    }
    assert_eq!(soup.positions.len(), 36);
    assert!(!HalfEdge::build(&soup).unwrap().is_closed());
    let w = soup.weld(T);
    assert_eq!(w.mesh.positions.len(), 8);
    assert_eq!(w.mesh.triangles.len(), 12);
    assert!(w.collapsed.is_empty());
    assert_eq!(w.remap.len(), 36);
    assert!(HalfEdge::build(&w.mesh).unwrap().is_closed());
    assert!((w.mesh.volume(T).unwrap() - 1.0).abs() < 1e-5);
    assert_eq!(w.mesh.euler_characteristic(), 2);
    // a triangle whose corners weld together collapses and is reported
    let mut s2 = soup.clone();
    s2.positions.push(Vec3::new(5.0, 5.0, 5.0));
    s2.positions.push(Vec3::new(5.0, 5.0, 5.0 + 1e-7));
    s2.positions.push(Vec3::new(6.0, 5.0, 5.0));
    s2.triangles.push([36, 37, 38]);
    assert_eq!(s2.weld(T).collapsed, vec![12]);
}

#[test]
fn remove_degenerate_drops_slivers() {
    let mut m = cube();
    m.positions.push(Vec3::new(0.5, 0.0, 0.0)); // 8: on edge 0-1
    m.triangles.push([0, 8, 1]);
    let (clean, removed) = m.remove_degenerate(T);
    assert_eq!(removed, vec![12]);
    assert_eq!(clean.triangles.len(), 12);
}

#[test]
fn bvh_pick_on_a_tessellated_sphere() {
    let m = sphere(1.0, 32, 64);
    assert_eq!(m.validate(T), Ok(()));
    assert_eq!(m.euler_characteristic(), 2);
    assert!(HalfEdge::build(&m).unwrap().is_closed());
    assert!(m.volume(T).unwrap() > 0.0);
    let bvh = Bvh::build(&m);

    // straight down near the pole: the true sphere is hit at t = 5 - sqrt(1 - 0.0005); the polyhedron lies inside
    // the sphere within the sagitta over a cell's diagonal, 1 - cos((pi / 32) * sqrt(2) / 2) ~ 2.4e-3
    let ray = Ray { origin: Vec3::new(0.01, 0.02, 5.0), dir: Vec3::new(0.0, 0.0, -1.0) };
    let hit = bvh.pick(&m, &ray, T).expect("hit");
    let t_true = 5.0 - (1.0f64 - 0.0005).sqrt();
    assert!(hit.t >= t_true - 1e-12 && hit.t - t_true < 3e-3, "t={} true={}", hit.t, t_true);
    let s: f64 = hit.bary.iter().sum();
    assert!((s - 1.0).abs() < 1e-12 && hit.bary.iter().all(|&w| w >= 0.0));
    let [a, b, c] = m.corners(hit.tri as usize).unwrap();
    let p = a * hit.bary[0] + b * hit.bary[1] + c * hit.bary[2];
    assert!((p - (ray.origin + ray.dir * hit.t)).magnitude() < 1e-12);

    // a miss
    let miss = Ray { origin: Vec3::new(2.0, 2.0, 5.0), dir: Vec3::new(0.0, 0.0, -1.0) };
    assert_eq!(bvh.pick(&m, &miss, T), None);

    // from the centre along a Fibonacci set of directions: t ~ 1 and the BVH agrees with brute force
    let n = 64;
    let golden = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    for k in 0..n {
        let z = 1.0 - 2.0 * (k as f64 + 0.5) / n as f64;
        let rr = (1.0 - z * z).sqrt();
        let d = Vec3::new(rr * (golden * k as f64).cos(), rr * (golden * k as f64).sin(), z);
        let ray = Ray { origin: Vec3::ZERO, dir: d };
        let hit = bvh.pick(&m, &ray, T).expect("inside hit");
        assert!(hit.t <= 1.0 + 1e-12 && hit.t > 1.0 - 3e-3, "k={} t={}", k, hit.t);
        let brute = (0..m.triangles.len())
            .filter_map(|t| {
                let single = Mesh { positions: m.corners(t).unwrap().to_vec(), triangles: vec![[0, 1, 2]] };
                Bvh::build(&single).pick(&single, &ray, T).map(|h| h.t)
            })
            .fold(f64::INFINITY, f64::min);
        assert!((brute - hit.t).abs() < 1e-12, "k={} bvh={} brute={}", k, hit.t, brute);
    }
}

#[test]
fn empty_mesh_is_harmless() {
    let m = Mesh::default();
    assert_eq!(m.validate(T), Ok(()));
    assert_eq!(m.aabb(), None);
    assert_eq!(m.area(), 0.0);
    assert_eq!(m.euler_characteristic(), 0);
    let ray = Ray { origin: Vec3::ZERO, dir: Vec3::new(1.0, 0.0, 0.0) };
    assert_eq!(Bvh::build(&m).pick(&m, &ray, T), None);
    assert_eq!(m.weld(T).mesh, Mesh::default());
}

#[test]
fn f32_buffers_for_the_viewport() {
    let m = cube();
    let b = m.to_f32_buffers(T);
    assert_eq!(b.positions.len(), 24);
    assert_eq!(b.normals.len(), 24);
    assert_eq!(b.indices.len(), 36);
    assert_eq!(&b.positions[21..24], &[1.0f32, 1.0, 1.0]);
    let s = (1.0 / 3f64.sqrt()) as f32;
    assert!((b.normals[21] - s).abs() < 1e-6);
}
