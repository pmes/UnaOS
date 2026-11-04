// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! VUGSCANGDT (SR542): the SR510 fixture scan (Noise::D435I) measured against its design built through the ONE builder
//! `design_of`, then read as ISO 1101 tolerances: the box top's flatness, the post's cylindricity, the post's position
//! against its nominal and against a nominal shifted 1 mm, the verdicts, and each face's nominal surface.

use core::f64::consts::PI;
use std::sync::OnceLock;

use brep_core::primitives;
use scan_core::fixture::{self, Noise, Scene};
use scan_core::{
    AlignParams, DepthScale, Design, Deviation, FaceGdt, Kind, Nominal, ScanMesh, Tol, Transform, Tsdf, Vec3, Verdict, design_of, extract,
    gdt, measure, nominal, position_mm,
};

const DEG: f64 = PI / 180.0;
const CENTRE: Vec3 = Vec3::new(-20.0, 0.0, 50.0);
const BAND: f64 = 1.0;

/// tests/deviation.rs's fusion (SR510): 12 orbit frames at 600 mm, 40° up, D435i noise; with the volume's truncation,
/// the edge margin `gdt` takes (the seat's ruling: read from the fuse, no second constant).
fn fused() -> &'static (ScanMesh, f64) {
    static S: OnceLock<(ScanMesh, f64)> = OnceLock::new();
    S.get_or_init(|| {
        let (scene, intr, scale) = (Scene::bench(), fixture::bench_intrinsics(), DepthScale::D400_DEFAULT);
        let mut t = Tsdf::new(4.0, 12.0, Tol::DEFAULT).expect("volume");
        for k in 0..12u32 {
            let p = fixture::orbit(CENTRE, 600.0, 40.0 * DEG, k as f64 * 2.0 * PI / 12.0);
            let f = fixture::render_depth(&scene, &intr, scale, &p, &Noise::D435I, k as u64);
            t.integrate(&f, scale, &intr, &p).expect("frame integrates");
        }
        (extract(&t).expect("surface"), t.trunc_mm())
    })
}

fn scan() -> &'static ScanMesh {
    &fused().0
}

/// The fixture's box (-230..70, ±100, 0..100) and r 40 h 150 post at (160, 0) through `design_of`, each solid's
/// vertices moved into place (the box's faces come first), measured from the identity.
fn measured() -> &'static (Design, Vec<String>, Deviation) {
    static M: OnceLock<(Design, Vec<String>, Deviation)> = OnceLock::new();
    M.get_or_init(|| {
        let tol = Tol::DEFAULT;
        let (b, c) = (primitives::box_(300.0, 200.0, 100.0, tol).expect("box"), primitives::cylinder(40.0, 150.0, tol).expect("cylinder"));
        let (mut d, names) = design_of([("box", &b), ("post", &c)], tol).expect("design");
        let nbox = b.faces.len() as u32;
        let mut at = vec![Vec3::ZERO; d.mesh.positions.len()];
        for (tri, &f) in d.mesh.triangles.iter().zip(&d.face_of) {
            let off = if f < nbox { Vec3::new(-230.0, -100.0, 0.0) } else { Vec3::new(160.0, 0.0, 0.0) };
            for &v in tri {
                at[v as usize] = off;
            }
        }
        for (p, o) in d.mesh.positions.iter_mut().zip(&at) {
            *p = *p + *o;
        }
        let dev = measure(scan(), &d, Transform::IDENTITY, &AlignParams::DEFAULT, BAND, tol).expect("aligns");
        (d, names, dev)
    })
}

fn faces() -> Vec<FaceGdt> {
    let (d, names, dev) = measured();
    let g = gdt(scan(), d, dev, fused().1, Tol::DEFAULT);
    for f in &g {
        println!(
            "gdt: {} {} n={}/{} flatness={:?} cylindricity={:?} position={:?} band={:?} extent={:?}",
            names[f.face as usize],
            f.nominal.name(),
            f.n,
            f.matched,
            f.flatness_mm,
            f.cylindricity_mm,
            f.position_mm,
            f.band_mm,
            f.extent
        );
    }
    g
}

fn top(g: &[FaceGdt]) -> FaceGdt {
    *g.iter()
        .find(|f| matches!(f.nominal, Nominal::Plane(p) if p.normal.z > 0.99 && (p.d - 100.0).abs() < 0.01))
        .expect("box top")
}

fn post(g: &[FaceGdt]) -> FaceGdt {
    *g.iter().find(|f| matches!(f.nominal, Nominal::Cylinder(_))).expect("post side")
}

#[test]
fn each_face_reads_its_nominal_surface() {
    let (d, names, _) = measured();
    let mut planes = 0;
    for f in 0..d.faces {
        let n = nominal(d, f, Tol::DEFAULT);
        let name = &names[f as usize];
        match n {
            Nominal::Plane(p) => {
                assert!(name.ends_with(":plane"), "{name} read as {n:?}");
                assert!((p.normal.mag() - 1.0).abs() < 1e-9);
                planes += 1;
            }
            Nominal::Cylinder(c) => {
                assert!(name.ends_with(":cylinder"), "{name} read as {n:?}");
                assert!((c.radius - 40.0).abs() < 0.05, "post radius {}", c.radius);
                assert!(c.axis.z.abs() > 1.0 - 1e-6, "post axis {:?}", c.axis);
                let r = c.point - c.axis * c.axis.dot(c.point);
                assert!((r - Vec3::new(160.0, 0.0, 0.0)).mag() < 0.05, "post axis point {:?}", c.point);
            }
            Nominal::Other => panic!("{name} read as Other"),
        }
    }
    assert_eq!(planes, 8, "six box faces and the post's two caps");
}

#[test]
fn the_box_top_is_flat_at_the_scan_noise() {
    let t = top(&faces());
    let fl = t.flatness_mm.expect("top measured");
    assert!(t.n >= 200, "top form vertices {}", t.n);
    assert!(fl < 0.2, "top flatness {fl}");
    assert!(t.band_mm.is_some_and(f64::is_finite), "the nominal-relative band is read");
    assert_eq!(t.cylindricity_mm, None);
    assert_eq!(t.position_mm, None);
}

#[test]
fn the_post_is_cylindrical_at_the_scan_noise() {
    let p = post(&faces());
    let cy = p.cylindricity_mm.expect("post measured");
    assert!(p.n >= 200, "post form vertices {}", p.n);
    assert!(cy < 0.3, "post cylindricity {cy}");
    assert_eq!(p.flatness_mm, None);
}

#[test]
fn the_post_position_reads_a_one_mm_shift_as_a_two_mm_zone() {
    let p = post(&faces());
    let Nominal::Cylinder(nc) = p.nominal else { panic!("post nominal {:?}", p.nominal) };
    let axis = p.axis.expect("post axis fitted");
    let at = p.position_mm.expect("post position");
    println!("post position at its nominal {at:.4}, extent {:?}", p.extent);
    assert!(at < 0.5, "post position at its own nominal {at}");
    let shifted = scan_core::Cylinder { point: nc.point + Vec3::X, ..nc };
    let off = position_mm(&axis, &shifted, p.extent);
    println!("post position against the nominal shifted 1 mm {off:.4}");
    assert!((off - 2.0).abs() <= 0.2, "position against the shifted nominal {off}");
}

#[test]
fn verdicts_pass_fail_and_refuse_a_kind_the_surface_does_not_take() {
    let g = faces();
    let (t, p) = (top(&g), post(&g));
    let fl = t.flatness_mm.expect("top measured");
    assert!(matches!(t.verdict(Kind::Flatness, fl + 0.1), Verdict::Pass { .. }));
    assert!(matches!(t.verdict(Kind::Flatness, fl - 0.1), Verdict::Fail { .. }) || fl < 0.1);
    assert_eq!(t.verdict(Kind::Flatness, fl), Verdict::Pass { value_mm: fl, tol_mm: fl });
    assert!(!p.accepts(Kind::Flatness) && p.accepts(Kind::Cylindricity) && p.accepts(Kind::Position));
    assert_eq!(p.verdict(Kind::Flatness, 1.0), Verdict::Unmeasured);
    assert_eq!(t.verdict(Kind::Position, 1.0), Verdict::Unmeasured);
    assert_eq!(Kind::parse("cylindricity"), Some(Kind::Cylindricity));
    assert_eq!(Kind::parse("roundness"), None);
    for k in [Kind::Flatness, Kind::Cylindricity, Kind::Position] {
        assert_eq!(Kind::parse(k.name()), Some(k));
    }
}
