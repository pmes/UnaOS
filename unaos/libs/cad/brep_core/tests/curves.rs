//! VUGBREP1: the NURBS curve and the closed-form surface intersections (every intersection point lies on both
//! surfaces).

use brep_core::{Tol, Vec3};
use brep_core::{Curve, Frame, Intersection, NurbsCurve, Refusal, Surface, SurfaceKind, intersect};
use std::f64::consts::{FRAC_1_SQRT_2, PI};

const T: Tol = Tol::DEFAULT;

#[test]
fn rational_quadratic_is_a_quarter_circle() {
    let c = NurbsCurve::new(
        2,
        vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        vec![Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 0.0), Vec3::new(0.0, 1.0, 0.0)],
        vec![1.0, FRAC_1_SQRT_2, 1.0],
    )
    .unwrap();
    for k in 0..=20 {
        let p = c.point(k as f64 / 20.0);
        assert!((p.mag() - 1.0).abs() < 1e-12, "{p:?}");
    }
    assert!((c.point(1.0) - Vec3::new(0.0, 1.0, 0.0)).mag() < 1e-15);
}

#[test]
fn clamped_cubic_interpolates_its_ends() {
    let ctrl: Vec<Vec3> = (0..5).map(|i| Vec3::new(i as f64, (i * i) as f64, 0.0)).collect();
    let c = NurbsCurve::new(3, vec![0.0, 0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0, 1.0], ctrl.clone(), vec![1.0; 5]).unwrap();
    assert!((c.point(0.0) - ctrl[0]).mag() < 1e-15);
    assert!((c.point(1.0) - ctrl[4]).mag() < 1e-12);
    assert_eq!(c.range(), (0.0, 1.0));
}

#[test]
fn bad_nurbs_are_refused() {
    let ctrl = vec![Vec3::ZERO, Vec3::X, Vec3::Y];
    assert_eq!(
        NurbsCurve::new(2, vec![0.0, 0.0, 1.0, 0.5, 1.0, 1.0], ctrl.clone(), vec![1.0; 3]),
        Err(Refusal::BadParam("nurbs knots not non-decreasing"))
    );
    assert_eq!(
        NurbsCurve::new(2, vec![0.0; 6], ctrl.clone(), vec![1.0; 3]),
        Err(Refusal::BadParam("nurbs empty parameter range"))
    );
    assert_eq!(
        NurbsCurve::new(2, vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0], ctrl, vec![1.0, 0.0, 1.0]),
        Err(Refusal::BadParam("nurbs weight not positive"))
    );
}

fn samples(c: &Curve) -> Vec<Vec3> {
    match c {
        Curve::Line { .. } => [-3.0, 0.0, 2.5].iter().map(|&t| c.point(t)).collect(),
        _ => (0..12).map(|k| c.point(k as f64 * PI / 6.0)).collect(),
    }
}

fn on_both(a: &Surface, b: &Surface, want: usize) {
    let Ok(Intersection::Curves(cs)) = intersect(a, b, T) else { panic!("{:?}", intersect(a, b, T)) };
    assert_eq!(cs.len(), want);
    for c in &cs {
        for p in samples(c) {
            assert!(a.distance(p) < 1e-9 && b.distance(p) < 1e-9, "{c:?} at {p:?}");
        }
    }
}

fn frame(o: Vec3, z: Vec3) -> Frame {
    let hint = if z.cross(Vec3::X).mag() > 0.1 { Vec3::X } else { Vec3::Y };
    Frame::new(o, z, hint).unwrap()
}

#[test]
fn closed_form_intersections() {
    let xy = Surface::Plane { f: Frame::WORLD };
    let yz = Surface::Plane { f: frame(Vec3::ZERO, Vec3::X) };
    on_both(&xy, &yz, 1);
    let cyl = Surface::Cylinder { f: Frame::WORLD, r: 2.0 };
    on_both(&Surface::Plane { f: frame(Vec3::new(0.0, 0.0, 1.0), Vec3::Z) }, &cyl, 1);
    on_both(&Surface::Plane { f: frame(Vec3::new(1.0, 0.0, 0.0), Vec3::X) }, &cyl, 2);
    let tilted = Vec3::new(0.0, (PI / 6.0).sin(), (PI / 6.0).cos());
    on_both(&Surface::Plane { f: frame(Vec3::new(0.0, 0.0, 1.0), tilted) }, &cyl, 1);
    on_both(&Surface::Plane { f: frame(Vec3::new(0.0, 0.0, 1.0), Vec3::Z) }, &Surface::Sphere { f: Frame::WORLD, r: 2.0 }, 1);
    let cyl2 = Surface::Cylinder { f: frame(Vec3::new(3.0, 0.0, 0.0), Vec3::Z), r: 2.0 };
    on_both(&cyl, &cyl2, 2);
    // Argument order does not matter.
    on_both(&cyl, &Surface::Plane { f: frame(Vec3::new(1.0, 0.0, 0.0), Vec3::X) }, 2);
}

#[test]
fn degenerate_and_refused_intersections() {
    let xy = Surface::Plane { f: Frame::WORLD };
    let up = Surface::Plane { f: frame(Vec3::new(0.0, 0.0, 1.0), Vec3::Z) };
    assert_eq!(intersect(&xy, &up, T), Ok(Intersection::Empty));
    assert_eq!(intersect(&xy, &xy, T), Ok(Intersection::Coincident));
    let s = Surface::Sphere { f: Frame::WORLD, r: 1.0 };
    assert!(matches!(intersect(&up, &s, T), Ok(Intersection::Point(_))));
    let a = Surface::Cylinder { f: Frame::WORLD, r: 1.0 };
    let b = Surface::Cylinder { f: frame(Vec3::ZERO, Vec3::X), r: 1.0 };
    assert_eq!(intersect(&a, &b, T), Err(Refusal::NotClosedForm { a: SurfaceKind::Cylinder, b: SurfaceKind::Cylinder }));
    let tor = Surface::Torus { f: Frame::WORLD, major: 3.0, minor: 1.0 };
    assert_eq!(intersect(&xy, &tor, T), Err(Refusal::NotClosedForm { a: SurfaceKind::Plane, b: SurfaceKind::Torus }));
}
