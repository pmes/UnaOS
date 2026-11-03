// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! VUGSCANSHAPES (LEDGER SR497): `detect`, Schnabel's multi-shape loop, on synthetic clouds sampled here with exact
//! normals and ±0.2 mm position jitter: a plane and a cylinder (the SR469 scene's two primitives) are exactly two
//! shapes with their parameters; a sphere added makes three; a cloud of noise is no shape at all; a cone is one cone.

use core::f64::consts::PI;

use scan_core::{FitError, Floor, Primitive, Shape, Tol, Vec3, detect};

const DEG: f64 = PI / 180.0;
const BAND: Tol = Tol { lin: 1.0, ang: 10.0 * DEG };
const JITTER: f64 = 0.2;

/// A 64-bit LCG (Knuth's MMIX constants): the test's own deterministic source.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }

    fn jitter(&mut self) -> Vec3 {
        Vec3::new((self.next() * 2.0 - 1.0) * JITTER, (self.next() * 2.0 - 1.0) * JITTER, (self.next() * 2.0 - 1.0) * JITTER)
    }
}

#[derive(Default)]
struct Cloud {
    p: Vec<Vec3>,
    n: Vec<Vec3>,
}

impl Cloud {
    fn push(&mut self, rng: &mut Lcg, p: Vec3, n: Vec3) {
        self.p.push(p + rng.jitter());
        self.n.push(n);
    }
}

fn basis(a: Vec3) -> (Vec3, Vec3) {
    let u = a.cross(Vec3::new(1.0, 0.0, 0.0)).normalize().unwrap();
    (u, a.cross(u))
}

fn plane_normal() -> Vec3 {
    Vec3::new(0.1, -0.2, 1.0).normalize().unwrap()
}
const PLANE_AT: Vec3 = Vec3::new(0.0, 0.0, -20.0);

/// A 180 x 180 mm patch of the tilted plane through (0, 0, -20), 3 mm grid.
fn plane(c: &mut Cloud, rng: &mut Lcg) {
    let nrm = plane_normal();
    let (u, w) = basis(nrm);
    for i in 0..60 {
        for j in 0..60 {
            let p = PLANE_AT + u * (i as f64 * 3.0 - 90.0) + w * (j as f64 * 3.0 - 90.0);
            c.push(rng, p, nrm);
        }
    }
}

fn cyl_axis() -> Vec3 {
    Vec3::new(0.2, 0.1, 1.0).normalize().unwrap()
}
const CYL_BASE: Vec3 = Vec3::new(0.0, 0.0, 30.0);

/// The cylinder r 30 along `cyl_axis` from (0, 0, 30), 100 mm long, 64 around and a 3 mm step.
fn cylinder(c: &mut Cloud, rng: &mut Lcg) {
    let a = cyl_axis();
    let (u, w) = basis(a);
    for k in 0..64 {
        let th = k as f64 * 2.0 * PI / 64.0;
        let r = u * th.cos() + w * th.sin();
        for h in 0..34 {
            c.push(rng, CYL_BASE + a * (h as f64 * 3.0) + r * 30.0, r);
        }
    }
}

const SPHERE_AT: Vec3 = Vec3::new(-80.0, 0.0, 60.0);

/// The sphere r 40 about (-80, 0, 60): 1800 Fibonacci points.
fn sphere(c: &mut Cloud, rng: &mut Lcg) {
    let n = 1800;
    let golden = PI * (3.0 - 5f64.sqrt());
    for i in 0..n {
        let z = 1.0 - 2.0 * (i as f64 + 0.5) / n as f64;
        let rho = (1.0 - z * z).sqrt();
        let th = golden * i as f64;
        let d = Vec3::new(rho * th.cos(), rho * th.sin(), z);
        c.push(rng, SPHERE_AT + d * 40.0, d);
    }
}

/// `count` points uniform in the cube of side 200 about `centre`, each with a random unit normal.
fn noise(c: &mut Cloud, rng: &mut Lcg, centre: Vec3, count: usize) {
    for _ in 0..count {
        let p = centre + Vec3::new(rng.next() * 200.0 - 100.0, rng.next() * 200.0 - 100.0, rng.next() * 200.0 - 100.0);
        let n = loop {
            let v = Vec3::new(rng.next() * 2.0 - 1.0, rng.next() * 2.0 - 1.0, rng.next() * 2.0 - 1.0);
            if v.mag() > 0.1 && v.mag() <= 1.0 {
                break v.normalize().unwrap();
            }
        };
        c.p.push(p);
        c.n.push(n);
    }
}

fn angle_deg(a: Vec3, b: Vec3) -> f64 {
    (a.normalize().unwrap().dot(b.normalize().unwrap())).abs().min(1.0).acos() / DEG
}

fn print(tag: &str, shapes: &[Shape]) {
    for s in shapes {
        println!("{tag}: {} support={} frac={:.3} rms={:.3} {:?}", s.kind.name(), s.support, s.fraction, s.rms_mm, s.kind);
    }
}

fn check_plane(shapes: &[Shape]) {
    let pl = shapes.iter().find_map(|s| if let Primitive::Plane(p) = s.kind { Some(p) } else { None }).expect("a plane");
    assert!(angle_deg(pl.normal, plane_normal()) < 0.5, "plane normal {} deg", angle_deg(pl.normal, plane_normal()));
    assert!(pl.signed_distance(PLANE_AT).abs() < 0.5, "plane offset {}", pl.signed_distance(PLANE_AT));
}

fn check_cylinder(shapes: &[Shape]) {
    let cy = shapes.iter().find_map(|s| if let Primitive::Cylinder(c) = s.kind { Some(c) } else { None }).expect("a cylinder");
    assert!(angle_deg(cy.axis, cyl_axis()) < 1.0, "cylinder axis {} deg", angle_deg(cy.axis, cyl_axis()));
    assert!((cy.radius - 30.0).abs() < 0.5, "cylinder radius {}", cy.radius);
    let d = CYL_BASE - cy.point;
    let off = (d - cy.axis * cy.axis.dot(d)).mag();
    assert!(off < 0.5, "cylinder axis {off} mm off the base");
}

#[test]
fn a_plane_and_a_cylinder_are_two_shapes() {
    let (mut c, mut rng) = (Cloud::default(), Lcg(0x497));
    plane(&mut c, &mut rng);
    cylinder(&mut c, &mut rng);
    noise(&mut c, &mut rng, Vec3::ZERO, 200);
    let shapes = detect(&c.p, &c.n, BAND, Floor::DEFAULT).expect("detect");
    print("plane+cyl", &shapes);
    assert_eq!(shapes.iter().map(|s| s.kind.name()).collect::<Vec<_>>(), ["plane", "cylinder"]);
    assert!(shapes.windows(2).all(|w| w[0].support >= w[1].support), "ordered by support");
    check_plane(&shapes);
    check_cylinder(&shapes);
    for s in &shapes {
        assert!(s.rms_mm <= JITTER && s.max_mm <= BAND.lin, "{} rms {} max {}", s.kind.name(), s.rms_mm, s.max_mm);
        assert_eq!(s.support as usize, s.inliers.len());
        assert!(s.rms_band(BAND) < 1.0);
    }
    assert!(shapes[0].support >= 3500 && shapes[1].support >= 2100, "supports {} {}", shapes[0].support, shapes[1].support);
    assert_eq!(detect(&c.p, &c.n, BAND, Floor::DEFAULT), Ok(shapes), "deterministic");
}

#[test]
fn a_sphere_added_is_a_third_shape() {
    let (mut c, mut rng) = (Cloud::default(), Lcg(0x498));
    plane(&mut c, &mut rng);
    cylinder(&mut c, &mut rng);
    sphere(&mut c, &mut rng);
    noise(&mut c, &mut rng, Vec3::ZERO, 200);
    let shapes = detect(&c.p, &c.n, BAND, Floor::DEFAULT).expect("detect");
    print("plane+cyl+sphere", &shapes);
    let mut names: Vec<&str> = shapes.iter().map(|s| s.kind.name()).collect();
    names.sort_unstable();
    assert_eq!(names, ["cylinder", "plane", "sphere"]);
    check_plane(&shapes);
    check_cylinder(&shapes);
    let sp = shapes.iter().find_map(|s| if let Primitive::Sphere(p) = s.kind { Some(p) } else { None }).unwrap();
    assert!((sp.centre - SPHERE_AT).mag() < 0.5, "sphere centre {:?}", sp.centre);
    assert!((sp.radius - 40.0).abs() < 0.5, "sphere radius {}", sp.radius);
}

#[test]
fn noise_is_no_shape() {
    let (mut c, mut rng) = (Cloud::default(), Lcg(0x499));
    noise(&mut c, &mut rng, Vec3::ZERO, 2000);
    let shapes = detect(&c.p, &c.n, BAND, Floor::DEFAULT).expect("detect");
    print("noise", &shapes);
    assert!(shapes.is_empty(), "a cloud of noise is no shape: {} found", shapes.len());
}

#[test]
fn a_cone_is_one_cone() {
    let (mut c, mut rng) = (Cloud::default(), Lcg(0x49A));
    let apex = Vec3::new(10.0, -5.0, 120.0);
    let a = Vec3::new(0.1, 0.05, -1.0).normalize().unwrap();
    let alpha = 30.0 * DEG;
    let (u, w) = basis(a);
    let mut h = 20.0;
    while h <= 80.0 {
        let rho = h * alpha.tan();
        let k = (2.0 * PI * rho / 3.0).ceil() as usize;
        for i in 0..k {
            let th = i as f64 * 2.0 * PI / k as f64;
            let r = u * th.cos() + w * th.sin();
            c.push(&mut rng, apex + a * h + r * rho, r * alpha.cos() - a * alpha.sin());
        }
        h += 2.5;
    }
    noise(&mut c, &mut rng, apex + a * 50.0, 200);
    let shapes = detect(&c.p, &c.n, BAND, Floor::DEFAULT).expect("detect");
    print("cone", &shapes);
    assert_eq!(shapes.iter().map(|s| s.kind.name()).collect::<Vec<_>>(), ["cone"]);
    let Primitive::Cone(cone) = shapes[0].kind else { unreachable!() };
    assert!((cone.apex - apex).mag() < 1.0, "cone apex {:?}", cone.apex);
    assert!(angle_deg(cone.axis, a) < 1.0, "cone axis {} deg", angle_deg(cone.axis, a));
    assert!(cone.axis.dot(a) > 0.0, "the axis points into the opening");
    assert!((cone.half_angle - alpha).abs() < 0.5 * DEG, "cone half-angle {} deg", cone.half_angle / DEG);
}

#[test]
fn refusals_are_named() {
    let p = [Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), Vec3::new(0.0, 1.0, 0.0)];
    let n = [Vec3::new(0.0, 0.0, 1.0); 2];
    assert_eq!(detect(&p, &n, BAND, Floor::DEFAULT), Err(FitError::NormalsMismatch { points: 3, normals: 2 }));
    assert_eq!(detect(&p[..2], &n, BAND, Floor::DEFAULT), Ok(Vec::new()), "fewer than three points: no shape");
}
