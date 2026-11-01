// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! VUGSCAN1 (SR469): the synthetic scene fused and measured. The fixture is rendered here, no binary files.

use core::f64::consts::PI;

use scan_core::fixture::{self, Noise, Scene};
use scan_core::{DepthScale, FitError, ScanError, Tol, Topo, Transform, Tsdf, Vec3, extract, fit_cylinder, fit_plane};

const DEG: f64 = PI / 180.0;
const CENTRE: Vec3 = Vec3::new(-20.0, 0.0, 50.0);

fn pose(k: u32, frames: u32) -> Transform {
    fixture::orbit(CENTRE, 600.0, 40.0 * DEG, k as f64 * 2.0 * PI / frames as f64)
}

fn fuse(noise: Noise, frames: u32) -> Tsdf {
    let (scene, intr, scale) = (Scene::bench(), fixture::bench_intrinsics(), DepthScale::D400_DEFAULT);
    let mut t = Tsdf::new(4.0, 12.0, Tol::DEFAULT).expect("volume");
    for k in 0..frames {
        let p = pose(k, frames);
        let f = fixture::render_depth(&scene, &intr, scale, &p, &noise, k as u64);
        t.integrate(&f, scale, &intr, &p).expect("frame integrates");
    }
    t
}

fn angle_deg(a: Vec3, b: Vec3) -> f64 {
    libm::acos((a.normalize().dot(b.normalize())).abs().min(1.0)) / DEG
}

#[test]
fn fused_scene_recovers_box_top_and_cylinder() {
    let t = fuse(Noise::D435I, 12);
    let sm = extract(&t).expect("surface");
    assert_eq!(sm.frames, 12);
    assert_eq!(sm.intrinsics, fixture::bench_intrinsics());
    let n = sm.mesh.positions.len();
    assert_eq!(sm.confidence.len(), n);
    assert_eq!(sm.normals.len(), n);
    assert!(sm.confidence.iter().all(|c| *c > 0.0 && *c <= 12.0));

    // The box top: z = 100 over x -230..70, y -100..100 (inset 15 mm from the edges).
    let top: Vec<Vec3> = sm
        .mesh
        .positions
        .iter()
        .copied()
        .filter(|p| (p.x + 80.0).abs() < 135.0 && p.y.abs() < 85.0 && p.z > 90.0 && p.z < 110.0)
        .collect();
    assert!(top.len() > 200, "box top vertices: {}", top.len());
    let fp = fit_plane(&top, Tol { lin: 3.0, ang: 10.0 * DEG }).expect("plane");
    let normal_deg = angle_deg(fp.shape.normal, Vec3::new(0.0, 0.0, 1.0));
    let off = fp.shape.distance(Vec3::new(-80.0, 0.0, 100.0)).abs();
    assert!(normal_deg < 0.5, "box top normal {normal_deg} deg");
    assert!(off < 2.0, "box top height off by {off} mm");
    assert!(fp.rms_mm <= 2.0, "box top rms {}", fp.rms_mm);

    // The cylinder: r 40 about x = 160, y = 0, z 0..150 (its side, away from the ground and the cap).
    let (mut pts, mut nrm) = (Vec::new(), Vec::new());
    for (p, q) in sm.mesh.positions.iter().zip(&sm.normals) {
        let rh = libm::hypot(p.x - 160.0, p.y);
        if rh < 55.0 && p.z > 15.0 && p.z < 135.0 {
            pts.push(*p);
            nrm.push(*q);
        }
    }
    assert!(pts.len() > 200, "cylinder vertices: {}", pts.len());
    let fc = fit_cylinder(&pts, &nrm, Tol { lin: 3.0, ang: 15.0 * DEG }).expect("cylinder");
    let axis_deg = angle_deg(fc.shape.axis, Vec3::new(0.0, 0.0, 1.0));
    let s = (75.0 - fc.shape.point.z) / fc.shape.axis.z;
    let on = fc.shape.point + fc.shape.axis * s;
    let axis_off = libm::hypot(on.x - 160.0, on.y);
    assert!(axis_deg < 1.0, "cylinder axis {axis_deg} deg");
    assert!((fc.shape.radius - 40.0).abs() < 2.0, "cylinder radius {}", fc.shape.radius);
    assert!(axis_off < 2.0, "cylinder axis {axis_off} mm off");

    // Topology: no non-manifold edge, no flipped edge; the open scan's rims are named as loops (or the bowtie).
    let loops = match sm.mesh.boundary_loops() {
        Ok(l) => format!("{} (longest {})", l.len(), l.iter().map(|x| x.len()).max().unwrap_or(0)),
        Err(Topo::NonManifoldVertex { v }) => format!("bowtie at vertex {v}"),
        Err(e) => panic!("fused mesh topology refused: {e:?}"),
    };
    println!(
        "scan: frames={} verts={} tris={} loops={} plane rms={:.3} normal_deg={:.3} off_mm={:.3} cyl r={:.3} axis_deg={:.3} axis_mm={:.3} rms={:.3}",
        sm.frames,
        n,
        sm.mesh.triangles.len(),
        loops,
        fp.rms_mm,
        normal_deg,
        off,
        fc.shape.radius,
        axis_deg,
        axis_off,
        fc.rms_mm
    );
}

#[test]
fn single_frame_is_confidence_one() {
    let t = fuse(Noise::NONE, 1);
    let sm = extract(&t).expect("surface");
    assert_eq!(sm.frames, 1);
    assert!(!sm.confidence.is_empty());
    assert!(sm.confidence.iter().all(|c| *c == 1.0));
}

#[test]
fn a_frame_with_no_overlap_is_refused() {
    let mut t = fuse(Noise::NONE, 1);
    let (blocks, frames) = (t.blocks(), t.frames());
    let far = Vec3::new(5000.0, 0.0, 0.0);
    let scene = Scene::bench().translated(far);
    let (intr, scale) = (fixture::bench_intrinsics(), DepthScale::D400_DEFAULT);
    let p = fixture::orbit(CENTRE + far, 600.0, 40.0 * DEG, 0.0);
    let f = fixture::render_depth(&scene, &intr, scale, &p, &Noise::NONE, 1);
    assert_eq!(t.integrate(&f, scale, &intr, &p), Err(ScanError::NoOverlap));
    assert_eq!((t.blocks(), t.frames()), (blocks, frames), "a refusal leaves the volume as it was");
}

#[test]
fn refusals_are_named() {
    let (scene, intr, scale) = (Scene::bench(), fixture::bench_intrinsics(), DepthScale::D400_DEFAULT);
    let p = pose(0, 12);
    let f = fixture::render_depth(&scene, &intr, scale, &p, &Noise::NONE, 0);

    let mut small = Tsdf::new(4.0, 12.0, Tol::DEFAULT).unwrap().with_budget(4);
    assert!(matches!(small.integrate(&f, scale, &intr, &p), Err(ScanError::VoxelTooFine { budget: 4, .. })));
    assert_eq!((small.blocks(), small.frames()), (0, 0));

    let mut t = Tsdf::new(4.0, 12.0, Tol::DEFAULT).unwrap();
    assert_eq!(extract(&t), Err(ScanError::NoFrames));
    let ir = probe_core::replay::ir_gradient(424, 240, 0);
    assert!(matches!(t.integrate(&ir, scale, &intr, &p), Err(ScanError::NotDepth { .. })));
    let blank = probe_core::Frame::packed(
        probe_core::StreamKind::Depth,
        probe_core::PixelFormat::Z16,
        424,
        240,
        0,
        0,
        vec![0u8; 424 * 240 * 2],
    )
    .unwrap();
    assert_eq!(t.integrate(&blank, scale, &intr, &p), Err(ScanError::NoDepth));
    let other = probe_core::Intrinsics { fx: 0.0, ..intr };
    assert_eq!(t.integrate(&f, scale, &other, &p), Err(ScanError::NoFocal));
    let wrong = probe_core::Intrinsics::ideal(640, 480);
    assert!(matches!(t.integrate(&f, scale, &wrong, &p), Err(ScanError::SizeMismatch { .. })));
    t.integrate(&f, scale, &intr, &p).expect("first frame");
    let shifted = probe_core::Intrinsics { ppx: 200.0, ..intr };
    assert_eq!(t.integrate(&f, scale, &shifted, &p), Err(ScanError::IntrinsicsChanged));

    assert!(matches!(Tsdf::new(0.0, 12.0, Tol::DEFAULT), Err(ScanError::BadVoxel { .. })));
    assert!(matches!(Tsdf::new(f64::NAN, 12.0, Tol::DEFAULT), Err(ScanError::BadVoxel { .. })));
    assert!(matches!(Tsdf::new(4.0, 4.0, Tol::DEFAULT), Err(ScanError::BadTrunc { .. })));
}

#[test]
fn render_depth_kat() {
    // A camera 600 mm above the origin looking straight down (+x right, image +y = world -y).
    let p = Transform::from_axes(
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, -1.0, 0.0),
        Vec3::new(0.0, 0.0, -1.0),
        Vec3::new(0.0, 0.0, 600.0),
    );
    let f = fixture::render_depth(
        &Scene::bench(),
        &fixture::bench_intrinsics(),
        DepthScale::D400_DEFAULT,
        &p,
        &Noise::NONE,
        0,
    );
    assert_eq!(f.u16_at(212, 120), Some(500), "the box top, 100 mm up");
    assert_eq!(f.u16_at(100, 120), Some(600), "the ground beside the box");
    assert_eq!(f.u16_at(0, 0), Some(0), "off the ground: no depth");
}

#[test]
fn plane_fit_exact_with_outliers() {
    let plane = |x: f64, y: f64| 0.5 * x - 0.25 * y + 10.0;
    let mut pts = Vec::new();
    for i in 0..20 {
        for j in 0..20 {
            let (x, y) = (i as f64 * 5.0, j as f64 * 5.0);
            pts.push(Vec3::new(x, y, plane(x, y)));
        }
    }
    for i in 0..100 {
        let (x, y) = ((i * 7 % 97) as f64, (i * 13 % 89) as f64);
        pts.push(Vec3::new(x, y, plane(x, y) + 5.0 + i as f64));
    }
    let band = Tol { lin: 0.5, ang: 0.1 };
    let f = fit_plane(&pts, band).expect("plane");
    let want = Vec3::new(-0.5, 0.25, 1.0).normalize();
    assert!((f.shape.normal - want).mag() < 1e-6, "{:?}", f.shape.normal);
    assert!(f.shape.distance(Vec3::new(0.0, 0.0, 10.0)).abs() < 1e-6);
    assert!(f.rms_mm < 1e-6 && f.max_mm < 1e-6);
    assert_eq!((f.inliers, f.outliers), (400, 100));
    assert_eq!(fit_plane(&pts, band), Ok(f), "deterministic");
    assert_eq!(fit_plane(&pts[..2], band), Err(FitError::TooFewPoints { have: 2, need: 3 }));
}

#[test]
fn cylinder_fit_exact_with_outliers() {
    let a = Vec3::new(0.2, 0.1, 1.0).normalize();
    let c0 = Vec3::new(10.0, -5.0, 3.0);
    let u = a.cross(Vec3::new(1.0, 0.0, 0.0)).normalize();
    let w = a.cross(u);
    let (mut pts, mut nrm) = (Vec::new(), Vec::new());
    for k in 0..36 {
        let th = k as f64 * 10.0 * DEG;
        let r = u * libm::cos(th) + w * libm::sin(th);
        for h in 0..10 {
            pts.push(c0 + a * (h as f64 * 5.0) + r * 25.0);
            nrm.push(r);
        }
    }
    for i in 0..50 {
        pts.push(c0 + u * (60.0 + i as f64) + a * i as f64);
        nrm.push(a);
    }
    let band = Tol { lin: 0.5, ang: 0.1 };
    let f = fit_cylinder(&pts, &nrm, band).expect("cylinder");
    assert!((f.shape.axis - a).mag() < 1e-6, "{:?}", f.shape.axis);
    assert!((f.shape.radius - 25.0).abs() < 1e-6, "{}", f.shape.radius);
    let d = c0 - f.shape.point;
    assert!((d - f.shape.axis * f.shape.axis.dot(d)).mag() < 1e-6, "axis passes through c0");
    assert_eq!((f.inliers, f.outliers), (360, 50));
    assert!(f.rms_mm < 1e-6);
    assert_eq!(fit_cylinder(&pts, &nrm, band), Ok(f), "deterministic");
    assert_eq!(
        fit_cylinder(&pts, &nrm[..3], band),
        Err(FitError::NormalsMismatch { points: pts.len(), normals: 3 })
    );
}

/// VUGSCANRIM (SR474): one frame's open scan is a manifold whose rim is named as boundary loops, not a bowtie
/// (the cube set's diagonal pair at the observation rim; mc.rs `rim_sheet`). On a refusal the vertex's star is dumped.
/// The frame is integrated from the IDENTITY pose, tools/scan's one-frame road (the grid on the camera axes): the
/// bowtie depends on the grid's orientation, and the orbit pose `fuse` uses did not reproduce it before the fix.
#[test]
fn single_frame_rim_is_named_loops() {
    let (scene, intr, scale) = (Scene::bench(), fixture::bench_intrinsics(), DepthScale::D400_DEFAULT);
    let f = fixture::render_depth(&scene, &intr, scale, &pose(0, 1), &Noise::NONE, 0);
    let mut t = Tsdf::new(4.0, 12.0, Tol::DEFAULT).expect("volume");
    t.integrate(&f, scale, &intr, &Transform::IDENTITY).expect("frame integrates");
    let sm = extract(&t).expect("surface");
    let m = &sm.mesh;
    let loops = match m.boundary_loops() {
        Ok(l) => l,
        Err(Topo::NonManifoldVertex { v }) => {
            let mut star = String::new();
            for (i, tri) in m.triangles.iter().enumerate().filter(|(_, tri)| tri.contains(&v)) {
                let p = tri.map(|k| m.positions[k as usize]);
                star += &format!("\n  tri {i} {tri:?} {:?} {:?} {:?}", p[0], p[1], p[2]);
            }
            panic!("bowtie at vertex {v} {:?} (voxel 4 mm), its star:{star}", m.positions[v as usize]);
        }
        Err(e) => panic!("single-frame topology refused: {e:?}"),
    };
    assert!(!loops.is_empty(), "an open scan has a rim");
    assert!(loops.iter().all(|l| l.len() >= 3), "every loop is a cycle");
    println!(
        "scan-rim: verts={} tris={} loops={} (longest {})",
        m.positions.len(),
        m.triangles.len(),
        loops.len(),
        loops.iter().map(|l| l.len()).max().unwrap_or(0)
    );
}
