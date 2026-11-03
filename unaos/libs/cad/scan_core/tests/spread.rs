// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! VUGSCANSHAPESPREAD (LEDGER SR526): `Shape::spread_deg`, the RMS angle between each inlier's normal and the fitted
//! surface's normal at that point, and `fit::spread`'s (RMS, p95). On a synthetic plane and cylinder (shapes.rs's
//! geometry, ±0.2 mm position jitter) whose normals are all tilted by a fixed angle the spread recovers the tilt; on the
//! fixture fused through the TSDF it grows with the depth noise. Both print the RMS beside the p95 and hold the p95
//! within 3 RMS (doc_core's 3-sigma gate takes the RMS as sigma_deg and must cover the tail).

use core::f64::consts::PI;

use scan_core::fit::spread;
use scan_core::fixture::{self, CylPrim, Noise, Scene};
use scan_core::{DepthScale, Floor, Shape, Tol, Tsdf, Vec3, detect, extract};

const DEG: f64 = PI / 180.0;

/// A 64-bit LCG (Knuth's MMIX constants): the test's own deterministic source.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn jitter(&mut self) -> Vec3 {
        let j = 0.2;
        Vec3::new((self.next() * 2.0 - 1.0) * j, (self.next() * 2.0 - 1.0) * j, (self.next() * 2.0 - 1.0) * j)
    }
}

/// Two unit vectors across `a` (a helper axis away from `a`, as fit.rs's `cyl_refine` picks it).
fn across(a: Vec3) -> (Vec3, Vec3) {
    let helper = if a.x.abs() < 0.9 { Vec3::new(1.0, 0.0, 0.0) } else { Vec3::new(0.0, 1.0, 0.0) };
    let u = a.cross(helper).normalize().unwrap();
    (u, a.cross(u))
}

/// shapes.rs's tilted 180 x 180 mm plane patch through (0, 0, -20) and its r 30 cylinder from (0, 0, 30), with every
/// normal tilted by `tilt` (rad) about a seeded random direction across it.
fn cloud(tilt: f64) -> (Vec<Vec3>, Vec<Vec3>) {
    let (mut p, mut n) = (Vec::new(), Vec::new());
    let (mut rng, mut turn) = (Lcg(0x526), Lcg(0x5260));
    let mut push = |q: Vec3, nrm: Vec3, rng: &mut Lcg| {
        let (u, w) = across(nrm);
        let phi = turn.next() * 2.0 * PI;
        let t = u * phi.cos() + w * phi.sin();
        p.push(q + rng.jitter());
        n.push(nrm * tilt.cos() + t * tilt.sin());
    };
    let pn = Vec3::new(0.1, -0.2, 1.0).normalize().unwrap();
    let (u, w) = across(pn);
    for i in 0..60 {
        for j in 0..60 {
            push(Vec3::new(0.0, 0.0, -20.0) + u * (i as f64 * 3.0 - 90.0) + w * (j as f64 * 3.0 - 90.0), pn, &mut rng);
        }
    }
    let a = Vec3::new(0.2, 0.1, 1.0).normalize().unwrap();
    let (u, w) = across(a);
    for k in 0..64 {
        let th = k as f64 * 2.0 * PI / 64.0;
        let r = u * th.cos() + w * th.sin();
        for h in 0..34 {
            push(Vec3::new(0.0, 0.0, 30.0) + a * (h as f64 * 3.0) + r * 30.0, r, &mut rng);
        }
    }
    (p, n)
}

fn of<'a>(shapes: &'a [Shape], kind: &str) -> &'a Shape {
    shapes.iter().find(|s| s.kind.name() == kind).unwrap_or_else(|| panic!("no {kind} in {shapes:?}"))
}

/// Print a shape's spread and hold `spread_deg` to `spread`'s RMS and the p95 within 3 RMS; returns the RMS.
fn measured(tag: &str, s: &Shape, points: &[Vec3], normals: &[Vec3]) -> f64 {
    let (rms, p95) = spread(&s.kind, points, normals, &s.inliers);
    println!("spread: {tag} {} support={} spread_deg={:.4} rms={:.4} p95={:.4}", s.kind.name(), s.support, s.spread_deg, rms, p95);
    assert_eq!(s.spread_deg, rms, "{tag}: spread_deg is spread's RMS");
    assert!(p95 <= 3.0 * rms + 1e-12, "{tag} {}: p95 {p95} past 3 x rms {rms}", s.kind.name());
    rms
}

#[test]
fn the_spread_is_the_injected_normal_tilt() {
    let band = Tol { lin: 1.0, ang: 10.0 * DEG };
    let mut last = [-1.0f64; 2];
    for t in [0.0, 2.0, 4.0] {
        let (p, n) = cloud(t * DEG);
        let shapes = detect(&p, &n, band, Floor::DEFAULT).expect("detect");
        for (k, kind) in ["plane", "cylinder"].into_iter().enumerate() {
            let s = measured(&format!("t={t}"), of(&shapes, kind), &p, &n);
            if t == 0.0 {
                assert!(s < 0.3, "{kind}: exact normals spread {s} deg");
            } else {
                assert!((s - t).abs() <= 0.15 * t + 0.1, "{kind}: tilt {t} deg measured {s} deg");
            }
            assert!(s > last[k], "{kind}: spread {s} did not grow past {}", last[k]);
            last[k] = s;
        }
    }
}

/// The fixture through the TSDF: SR515's lifted-boss scene (the ground ±200 at z 0, a cylinder r 40 h 500 from z 30 so
/// no TSDF skirt rounds its foot into the plane), a 12-frame orbit, 4 mm voxels, tools/scan's band (3 mm, 15 deg).
fn fused(noise: Noise) -> (Vec<Vec3>, Vec<Vec3>, Vec<Shape>) {
    let scene = Scene {
        ground_centre: Vec3::ZERO,
        ground: [200.0, 200.0],
        boxes: Vec::new(),
        cylinders: vec![CylPrim { base: Vec3::new(0.0, 0.0, 30.0), radius: 40.0, height: 500.0 }],
    };
    let (intr, scale) = (fixture::bench_intrinsics(), DepthScale::D400_DEFAULT);
    let mut t = Tsdf::new(4.0, 12.0, Tol::DEFAULT).expect("volume");
    for k in 0..12u32 {
        let pose = fixture::orbit(Vec3::new(0.0, 0.0, 60.0), 600.0, 40.0 * DEG, k as f64 * 30.0 * DEG);
        let f = fixture::render_depth(&scene, &intr, scale, &pose, &noise, k as u64);
        t.integrate(&f, scale, &intr, &pose).expect("frame integrates");
    }
    let sm = extract(&t).expect("mesh");
    let shapes = detect(&sm.mesh.positions, &sm.normals, Tol { lin: 3.0, ang: 15.0 * DEG }, Floor::DEFAULT).expect("detect");
    (sm.mesh.positions, sm.normals, shapes)
}

#[test]
fn the_fused_fixture_spread_grows_with_depth_noise() {
    let (p0, n0, s0) = fused(Noise::NONE);
    let (p1, n1, s1) = fused(Noise::D435I);
    let plane0 = measured("fused NONE", of(&s0, "plane"), &p0, &n0);
    let cyl0 = measured("fused NONE", of(&s0, "cylinder"), &p0, &n0);
    let plane1 = measured("fused D435I", of(&s1, "plane"), &p1, &n1);
    let cyl1 = measured("fused D435I", of(&s1, "cylinder"), &p1, &n1);
    // TSDF gradient normals at 4 mm voxels: the clean fused plane measured 3.05 deg (5.89 under D435I); the 0.3 bound
    // holds on exact normals only and stays in the_spread_is_the_injected_normal_tilt.
    assert!(plane0 < 4.0, "the clean fused plane's spread {plane0} deg (TSDF normal discretization at 4 mm voxels)");
    assert!(plane1 > plane0, "plane spread {plane1} under noise not past {plane0}");
    assert!(cyl1 > cyl0, "cylinder spread {cyl1} under noise not past {cyl0}");
}
