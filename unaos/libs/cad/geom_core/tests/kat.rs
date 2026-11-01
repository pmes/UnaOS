//! Known-answer tests for geom_core (VUGGEOM, LEDGER SR464).

use core::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};

use geom_core::*;

const T: Tol = Tol::DEFAULT;

fn near(a: f64, b: f64, eps: f64) -> bool {
    (a - b).abs() <= eps
}
fn up(x: f64) -> f64 {
    if x >= 0.0 { f64::from_bits(x.to_bits() + 1) } else { f64::from_bits(x.to_bits() - 1) }
}
fn down(x: f64) -> f64 {
    if x > 0.0 { f64::from_bits(x.to_bits() - 1) } else { f64::from_bits(x.to_bits() + 1) }
}
fn v2(x: f64, y: f64) -> Vec2 {
    Vec2::new(x, y)
}
fn v3(x: f64, y: f64, z: f64) -> Vec3 {
    Vec3::new(x, y, z)
}

// ---- tolerance and vectors ----

#[test]
fn tol_default_and_refusals() {
    assert_eq!(T, Tol { lin: 1e-6, ang: 1e-9 });
    assert_eq!(Tol::new(0.0, 1e-9), Err(Degenerate::BadTolerance));
    assert_eq!(Tol::new(f64::NAN, 1e-9), Err(Degenerate::NonFinite));
    assert_eq!(Vec3::ZERO.normalize(), Err(Degenerate::ZeroVector));
    assert_eq!(Vec2::new(f64::INFINITY, 0.0).normalize(), Err(Degenerate::NonFinite));
    assert_eq!(v3(3.0, 4.0, 0.0).mag(), 5.0);
    assert_eq!(v3(1.0, 0.0, 0.0).cross(v3(0.0, 1.0, 0.0)), Vec3::Z);
    assert_eq!(v3(1.0, 2.0, 3.0).to_f32(), [1.0f32, 2.0, 3.0]);
    let p = v3(0.0, 0.0, 7.0).any_perpendicular().unwrap();
    assert!(near(p.mag(), 1.0, 1e-15) && p.z == 0.0);
}

// ---- the 3-4-5 triangle ----

#[test]
fn three_four_five() {
    let tri = [v2(0.0, 0.0), v2(4.0, 0.0), v2(0.0, 3.0)];
    assert_eq!(area(&tri), 6.0);
    let c = centroid(&tri, T).unwrap();
    assert!(near(c.x, 4.0 / 3.0, 1e-15) && near(c.y, 1.0, 1e-15));
    assert_eq!(is_ccw(&tri), Predicate::Positive);
    let rev = [tri[2], tri[1], tri[0]];
    assert_eq!(area(&rev), -6.0);
    assert_eq!(is_ccw(&rev), Predicate::Negative);
    assert_eq!(Segment2::new(tri[1], tri[2]).len(), 5.0);
    // (4,3) is 12/5 from the hypotenuse 3x + 4y = 12, nearest on edge bc.
    let (q, f) = closest_point_triangle(v3(4.0, 3.0, 0.0), v3(0.0, 0.0, 0.0), v3(4.0, 0.0, 0.0), v3(0.0, 3.0, 0.0), T).unwrap();
    assert_eq!(f, TriFeature::Edge(1));
    assert!(near(q.dist(v3(4.0, 3.0, 0.0)), 2.4, 1e-12));
    let (q, f) = closest_point_triangle(v3(-1.0, -1.0, 2.0), v3(0.0, 0.0, 0.0), v3(4.0, 0.0, 0.0), v3(0.0, 3.0, 0.0), T).unwrap();
    assert_eq!((q, f), (Vec3::ZERO, TriFeature::Vertex(0)));
    let (q, f) = closest_point_triangle(v3(1.0, 1.0, 5.0), v3(0.0, 0.0, 0.0), v3(4.0, 0.0, 0.0), v3(0.0, 3.0, 0.0), T).unwrap();
    assert!(q.approx_eq(v3(1.0, 1.0, 0.0), T) && f == TriFeature::Face);
    assert_eq!(centroid(&tri[..2], T), Err(Degenerate::TooFewPoints));
    assert_eq!(centroid(&[v2(0.0, 0.0), v2(1.0, 0.0), v2(2.0, 0.0)], T), Err(Degenerate::ZeroArea));
}

// ---- the unit circle ----

#[test]
fn unit_circle() {
    let a = Circle2::new(Vec2::ZERO, 1.0).unwrap();
    let b = Circle2::new(v2(1.0, 0.0), 1.0).unwrap();
    match circle_circle2(&a, &b, T) {
        CircleCircle::Two(p, q) => {
            let h = 3.0f64.sqrt() / 2.0;
            assert!(p.approx_eq(v2(0.5, h), T) && q.approx_eq(v2(0.5, -h), T), "{p:?} {q:?}");
        }
        o => panic!("{o:?}"),
    }
    let far = Circle2::new(v2(2.0, 0.0), 1.0).unwrap();
    assert_eq!(circle_circle2(&a, &far, T), CircleCircle::Tangent(v2(1.0, 0.0)));
    assert_eq!(circle_circle2(&a, &Circle2::new(v2(3.0, 0.0), 1.0).unwrap(), T), CircleCircle::Separate);
    assert_eq!(circle_circle2(&a, &Circle2::new(v2(0.1, 0.0), 0.5).unwrap(), T), CircleCircle::Contained);
    assert_eq!(circle_circle2(&a, &a, T), CircleCircle::Coincident);
    let top = Line2::new(v2(0.0, 1.0), Vec2::X).unwrap();
    assert_eq!(line_circle2(&top, &a, T), LineCircle::Tangent(v2(0.0, 1.0)));
    let axis = Line2::new(v2(-3.0, 0.0), Vec2::X).unwrap();
    assert_eq!(line_circle2(&axis, &a, T), LineCircle::Two(v2(-1.0, 0.0), v2(1.0, 0.0)));
    assert_eq!(line_circle2(&Line2::new(v2(0.0, 2.0), Vec2::X).unwrap(), &a, T), LineCircle::Miss);
    let q = Arc2::new(Vec2::ZERO, 1.0, 0.0, FRAC_PI_2).unwrap();
    assert!(q.end_point().approx_eq(v2(0.0, 1.0), T));
    assert!(near(q.len(), FRAC_PI_2, 1e-15));
    assert!(q.contains_angle(FRAC_PI_4, T) && !q.contains_angle(PI, T) && q.contains_angle(2.0 * PI + 0.1, T));
    let cw = Arc2::new(Vec2::ZERO, 1.0, 0.0, -FRAC_PI_2).unwrap();
    assert!(cw.contains_angle(-FRAC_PI_4, T) && !cw.contains_angle(FRAC_PI_4, T));
    assert_eq!(Circle2::new(Vec2::ZERO, 0.0), Err(Degenerate::ZeroRadius));
    let c3 = Circle3::new(Vec3::ZERO, Vec3::Z, 2.0).unwrap();
    for k in 0..8 {
        let p = c3.point_at(k as f64);
        assert!(near(p.mag(), 2.0, 1e-14) && near(p.z, 0.0, 1e-15));
    }
}

// ---- predicates: the overflow / underflow hole closed ----

fn naive2(a: Vec2, b: Vec2, c: Vec2) -> f64 {
    (a.x - c.x) * (b.y - c.y) - (a.y - c.y) * (b.x - c.x)
}

#[test]
fn orient2d_near_1e300() {
    let x = 1e300;
    let (a, b) = (v2(x, x), v2(2.0 * x, 2.0 * x));
    assert_eq!(orient2d(a, b, v2(4.0 * x, 4.0 * x)), Predicate::Zero);
    let above = v2(4.0 * x, up(4.0 * x));
    let below = v2(4.0 * x, down(4.0 * x));
    assert_eq!(orient2d(a, b, above), Predicate::Positive);
    assert_eq!(orient2d(a, b, below), Predicate::Negative);
    // The unscaled formula overflows to inf - inf.
    assert!(naive2(a, b, above).is_nan());
}

#[test]
fn orient2d_near_1e_minus_300() {
    let x = 1e-300;
    let (a, b) = (v2(x, x), v2(2.0 * x, 2.0 * x));
    assert_eq!(orient2d(a, b, v2(4.0 * x, 4.0 * x)), Predicate::Zero);
    let above = v2(4.0 * x, up(4.0 * x));
    assert_eq!(orient2d(a, b, above), Predicate::Positive);
    assert_eq!(orient2d(a, b, v2(4.0 * x, down(4.0 * x))), Predicate::Negative);
    // The unscaled formula underflows to 0 and would call the turn collinear.
    assert_eq!(naive2(a, b, above), 0.0);
    // Subnormal coordinates are exact too.
    let s = f64::from_bits(1);
    assert_eq!(orient2d(v2(0.0, 0.0), v2(s, 0.0), v2(0.0, s)), Predicate::Positive);
}

/// Kettner, Mehlhorn, Pion, Schirra, Yap, "Classroom examples of robustness problems in geometric computations":
/// points one ulp apart near (0.5, 0.5) against the line through (12, 12) and (24, 24). The exact orientation of
/// p, b, c is 12 * (py - px).
#[test]
fn orient2d_kettner_grid() {
    let ulp = 2f64.powi(-53);
    let (b, c) = (v2(12.0, 12.0), v2(24.0, 24.0));
    let mut naive_wrong = 0;
    for i in 0..64 {
        for j in 0..64 {
            let p = v2(0.5 + i as f64 * ulp, 0.5 + j as f64 * ulp);
            let want = match j.cmp(&i) {
                core::cmp::Ordering::Greater => Predicate::Positive,
                core::cmp::Ordering::Less => Predicate::Negative,
                core::cmp::Ordering::Equal => Predicate::Zero,
            };
            assert_eq!(orient2d(p, b, c), want, "i={i} j={j}");
            let n = naive2(p, b, c);
            let ns = if n > 0.0 { Predicate::Positive } else if n < 0.0 { Predicate::Negative } else { Predicate::Zero };
            if ns != want {
                naive_wrong += 1;
            }
        }
    }
    assert!(naive_wrong > 0, "the grid must exercise the exact stage");
}

#[test]
fn orient3d_coplanar_and_one_ulp() {
    let (a, b, c) = (v3(0.0, 0.0, 0.0), v3(1.0, 0.0, 0.0), v3(0.0, 1.0, 0.0));
    assert_eq!(orient3d(a, b, c, v3(1.0, 1.0, 0.0)), Predicate::Zero);
    assert_eq!(orient3d(a, b, c, v3(0.0, 0.0, 1.0)), Predicate::Negative);
    assert_eq!(orient3d(a, b, c, v3(0.0, 0.0, -1.0)), Predicate::Positive);
    // The plane x + y + z = 3, seen counterclockwise from the +(1,1,1) side.
    let (a, b, c) = (v3(3.0, 0.0, 0.0), v3(0.0, 3.0, 0.0), v3(0.0, 0.0, 3.0));
    assert_eq!(orient3d(a, b, c, v3(1.0, 1.0, 1.0)), Predicate::Zero);
    assert_eq!(orient3d(a, b, c, v3(1.0, 1.0, up(1.0))), Predicate::Negative);
    assert_eq!(orient3d(a, b, c, v3(1.0, 1.0, down(1.0))), Predicate::Positive);
    // A power of two near 1e300, so 3 * big is exact and the four points stay coplanar.
    let big = 2f64.powi(996);
    assert_eq!(orient3d(a * big, b * big, c * big, v3(1.0, 1.0, 1.0) * big), Predicate::Zero);
    assert_eq!(orient3d(a * big, b * big, c * big, v3(big, big, up(big))), Predicate::Negative);
}

#[test]
fn incircle_cocircular_and_one_ulp() {
    let (a, b, c) = (v2(1.0, 0.0), v2(0.0, 1.0), v2(-1.0, 0.0));
    assert_eq!(incircle(a, b, c, v2(0.0, -1.0)), Predicate::Zero);
    assert_eq!(incircle(a, b, c, v2(0.0, up(-1.0))), Predicate::Positive);
    assert_eq!(incircle(a, b, c, v2(0.0, down(-1.0))), Predicate::Negative);
    assert_eq!(incircle(c, b, a, v2(0.0, 0.0)), Predicate::Negative);
    // 3-4-5 points on the radius-5 circle, offset to 1e15.
    let o = 1e15;
    let p = |x: f64, y: f64| v2(o + x, o + y);
    assert_eq!(incircle(p(5.0, 0.0), p(3.0, 4.0), p(-4.0, 3.0), p(0.0, -5.0)), Predicate::Zero);
    assert_eq!(incircle(p(5.0, 0.0), p(3.0, 4.0), p(-4.0, 3.0), p(0.0, -4.875)), Predicate::Positive);
}

#[test]
fn predicate_refusals_are_named() {
    assert_eq!(orient2d(v2(f64::NAN, 0.0), v2(1.0, 0.0), v2(0.0, 1.0)), Predicate::Uncertain(Unsure::NonFinite));
    assert_eq!(incircle(v2(0.0, 0.0), v2(1.0, 0.0), v2(0.0, 1.0), v2(f64::INFINITY, 0.0)), Predicate::Uncertain(Unsure::NonFinite));
    // A collinear triple spanning 2^600: the filter cannot decide and the exact stage would not be exact.
    let tiny = 2f64.powi(-600);
    assert_eq!(orient2d(v2(0.0, 0.0), v2(1.0, 0.0), v2(tiny, 0.0)), Predicate::Uncertain(Unsure::Range));
    assert_eq!(orient2d(v2(0.0, 0.0), v2(1e300, 0.0), v2(1e-300, 0.0)), Predicate::Uncertain(Unsure::Range));
    // The same range with a decisive determinant answers.
    assert_eq!(orient2d(v2(0.0, 0.0), v2(1.0, 0.0), v2(0.5, tiny)), Predicate::Positive);
    assert_eq!(Predicate::Uncertain(Unsure::Range).sign(), None);
    assert_eq!(Predicate::Positive.sign(), Some(core::cmp::Ordering::Greater));
    assert_eq!(orient2d(Vec2::ZERO, Vec2::ZERO, Vec2::ZERO), Predicate::Zero);
}

// ---- intersections ----

#[test]
fn ray_triangle_edges() {
    let (a, b, c) = (v3(0.0, 0.0, 0.0), v3(1.0, 0.0, 0.0), v3(0.0, 1.0, 0.0));
    let down = |x: f64, y: f64| Ray::new(v3(x, y, 1.0), -Vec3::Z).unwrap();
    match ray_triangle(&down(0.25, 0.25), a, b, c, T).unwrap() {
        RayTri::Hit { t, u, v, p } => assert_eq!((t, u, v, p), (1.0, 0.25, 0.25, v3(0.25, 0.25, 0.0))),
        o => panic!("{o:?}"),
    }
    assert!(matches!(ray_triangle(&down(0.5, 0.0), a, b, c, T).unwrap(), RayTri::Hit { v, .. } if v == 0.0));
    assert!(matches!(ray_triangle(&down(0.5, 0.5), a, b, c, T).unwrap(), RayTri::Hit { .. }));
    assert!(matches!(ray_triangle(&down(1.0, 0.0), a, b, c, T).unwrap(), RayTri::Hit { u, .. } if u == 1.0));
    assert!(matches!(ray_triangle(&down(0.0, 0.0), a, b, c, T).unwrap(), RayTri::Hit { .. }));
    assert_eq!(ray_triangle(&down(0.5, -1e-3), a, b, c, T).unwrap(), RayTri::Miss);
    assert_eq!(ray_triangle(&down(0.6, 0.6), a, b, c, T).unwrap(), RayTri::Miss);
    let flat = Ray::new(v3(0.0, 0.0, 1.0), Vec3::X).unwrap();
    assert_eq!(ray_triangle(&flat, a, b, c, T).unwrap(), RayTri::Parallel);
    let behind = Ray::new(v3(0.25, 0.25, -1.0), -Vec3::Z).unwrap();
    assert_eq!(ray_triangle(&behind, a, b, c, T).unwrap(), RayTri::Miss);
    assert_eq!(ray_triangle(&down(0.2, 0.2), a, b, v3(2.0, 0.0, 0.0), T), Err(Degenerate::ZeroArea));
}

#[test]
fn ray_box_and_sphere() {
    let bx = Aabb3::from_points(&[v3(-1.0, -1.0, -1.0), v3(1.0, 1.0, 1.0)]).unwrap();
    let on_face = Ray::new(v3(-5.0, 1.0, 0.0), Vec3::X).unwrap();
    assert_eq!(ray_aabb(&on_face, &bx), RayBox::Hit { t_enter: 4.0, t_exit: 6.0 });
    let outside = Ray::new(v3(-5.0, 1.5, 0.0), Vec3::X).unwrap();
    assert_eq!(ray_aabb(&outside, &bx), RayBox::Miss);
    let inside = Ray::new(Vec3::ZERO, Vec3::Y).unwrap();
    assert_eq!(ray_aabb(&inside, &bx), RayBox::Hit { t_enter: 0.0, t_exit: 1.0 });
    let away = Ray::new(v3(-5.0, 0.0, 0.0), -Vec3::X).unwrap();
    assert_eq!(ray_aabb(&away, &bx), RayBox::Miss);
    let r = Ray::new(v3(-5.0, 0.0, 0.0), Vec3::X).unwrap();
    assert_eq!(ray_sphere(&r, Vec3::ZERO, 1.0, T).unwrap(), RaySphere::Hit { t_near: 4.0, t_far: 6.0 });
    let graze = Ray::new(v3(-5.0, 1.0, 0.0), Vec3::X).unwrap();
    assert_eq!(ray_sphere(&graze, Vec3::ZERO, 1.0, T).unwrap(), RaySphere::Tangent { t: 5.0 });
    assert_eq!(ray_sphere(&away, Vec3::ZERO, 1.0, T).unwrap(), RaySphere::Miss);
    assert_eq!(ray_sphere(&r, Vec3::ZERO, 0.0, T), Err(Degenerate::ZeroRadius));
}

#[test]
fn lines_planes_segments() {
    let l1 = Line2::new(Vec2::ZERO, v2(1.0, 1.0)).unwrap();
    let l2 = Line2::new(v2(0.0, 2.0), v2(1.0, -1.0)).unwrap();
    match line_line2(&l1, &l2, T) {
        LineLine2::Point { p, .. } => assert!(p.approx_eq(v2(1.0, 1.0), T)),
        o => panic!("{o:?}"),
    }
    let l3 = Line2::new(v2(0.0, 1.0), v2(2.0, 2.0)).unwrap();
    assert_eq!(line_line2(&l1, &l3, T), LineLine2::Parallel);
    let l4 = Line2::new(v2(3.0, 3.0), v2(-1.0, -1.0)).unwrap();
    assert_eq!(line_line2(&l1, &l4, T), LineLine2::Coincident);

    let s = |a: (f64, f64), b: (f64, f64)| Segment2::new(v2(a.0, a.1), v2(b.0, b.1));
    assert_eq!(segment_segment2(&s((0.0, 0.0), (2.0, 2.0)), &s((0.0, 2.0), (2.0, 0.0)), T), Ok(SegSeg2::Point(v2(1.0, 1.0))));
    assert_eq!(
        segment_segment2(&s((0.0, 0.0), (2.0, 0.0)), &s((1.0, 0.0), (3.0, 0.0)), T),
        Ok(SegSeg2::Overlap(v2(1.0, 0.0), v2(2.0, 0.0)))
    );
    assert_eq!(segment_segment2(&s((0.0, 0.0), (1.0, 0.0)), &s((1.0, 0.0), (2.0, 0.0)), T), Ok(SegSeg2::Point(v2(1.0, 0.0))));
    assert_eq!(segment_segment2(&s((0.0, 0.0), (1.0, 0.0)), &s((0.0, 1.0), (1.0, 1.0)), T), Ok(SegSeg2::Disjoint));
    assert_eq!(segment_segment2(&s((0.0, 0.0), (1.0, 0.0)), &s((2.0, -1.0), (2.0, 1.0)), T), Ok(SegSeg2::Disjoint));
    assert_eq!(segment_segment2(&s((0.0, 0.0), (0.0, 0.0)), &s((0.0, 1.0), (1.0, 1.0)), T), Err(Degenerate::ZeroVector));

    let floor = Plane::from_point_normal(Vec3::ZERO, Vec3::Z).unwrap();
    let wall = Plane::from_point_normal(Vec3::ZERO, Vec3::X).unwrap();
    match plane_plane(&floor, &wall, T) {
        PlanePlane::Line(l) => {
            assert!(l.dir.approx_eq(Vec3::Y, T) || l.dir.approx_eq(-Vec3::Y, T));
            assert!(l.point.approx_eq(Vec3::ZERO, T));
        }
        o => panic!("{o:?}"),
    }
    let up5 = Plane::from_point_normal(v3(0.0, 0.0, 5.0), -Vec3::Z).unwrap();
    assert_eq!(plane_plane(&floor, &up5, T), PlanePlane::Parallel);
    let same = Plane::from_point_normal(v3(3.0, 4.0, 0.0), -Vec3::Z).unwrap();
    assert_eq!(plane_plane(&floor, &same, T), PlanePlane::Coincident);
    let l = Line::new(v3(1.0, 2.0, 3.0), -Vec3::Z).unwrap();
    assert_eq!(line_plane(&l, &floor, T), LinePlane::Point { p: v3(1.0, 2.0, 0.0), t: 3.0 });
    let lx = Line::new(v3(0.0, 0.0, 1.0), Vec3::X).unwrap();
    assert_eq!(line_plane(&lx, &floor, T), LinePlane::Parallel);
    let lin = Line::new(Vec3::ZERO, Vec3::X).unwrap();
    assert_eq!(line_plane(&lin, &floor, T), LinePlane::InPlane);
    assert_eq!(Plane::from_points(Vec3::ZERO, Vec3::X, Vec3::X * 2.0, T), Err(Degenerate::Collinear));
    let (q, d) = closest_point_plane(v3(1.0, 1.0, -2.0), &floor);
    assert_eq!((q, d), (v3(1.0, 1.0, 0.0), -2.0));
    let seg = Segment::new(Vec3::ZERO, v3(10.0, 0.0, 0.0));
    assert_eq!(closest_point_segment(v3(-3.0, 1.0, 0.0), &seg), (Vec3::ZERO, 0.0));
    assert_eq!(closest_point_segment(v3(4.0, 1.0, 0.0), &seg), (v3(4.0, 0.0, 0.0), 0.4));
    assert_eq!(closest_point_line(v3(-3.0, 1.0, 0.0), &Line::through(Vec3::ZERO, Vec3::X).unwrap()), (v3(-3.0, 0.0, 0.0), -3.0));
}

// ---- matrices, quaternions, transforms, bounds ----

#[test]
fn transforms_and_matrices() {
    let q = Quat::from_axis_angle(Vec3::Z, FRAC_PI_2).unwrap();
    let t = Transform::new(q, 2.0, v3(1.0, 2.0, 3.0)).unwrap();
    assert!(t.apply_point(Vec3::X).approx_eq(v3(1.0, 4.0, 3.0), T));
    assert!(t.apply_vector(Vec3::X).approx_eq(v3(0.0, 2.0, 0.0), T));
    assert!(t.apply_normal(Vec3::X).approx_eq(Vec3::Y, T));
    let inv = t.invert().unwrap();
    let p = v3(-7.0, 0.5, 11.0);
    assert!(inv.apply_point(t.apply_point(p)).approx_eq(p, T));
    assert!(t.compose(inv).apply_point(p).approx_eq(p, T));
    let m = t.to_mat4();
    assert!(m.transform_point3(p).approx_eq(t.apply_point(p), T));
    let mi = m.inverse().unwrap();
    let id = m * mi;
    for c in 0..4 {
        for r in 0..4 {
            assert!(near(id.cols[c][r], if c == r { 1.0 } else { 0.0 }, 1e-14), "{id:?}");
        }
    }
    assert!(near(m.determinant(), 8.0, 1e-12));
    assert_eq!(Mat4::from_scale(v3(1.0, 0.0, 1.0)).inverse(), Err(Degenerate::Singular));
    let m3 = Mat3::from_cols(v3(2.0, 0.0, 0.0), v3(1.0, 3.0, 0.0), v3(0.0, 0.0, 4.0));
    let i3 = m3 * m3.inverse().unwrap();
    for (k, col) in i3.cols.iter().enumerate() {
        let want = [Vec3::X, Vec3::Y, Vec3::Z][k];
        assert!(col.approx_eq(want, Tol::new(1e-14, 1e-9).unwrap()));
    }
    assert_eq!(Mat3::from_cols(Vec3::X, Vec3::X, Vec3::Z).inverse(), Err(Degenerate::Singular));
    assert_eq!(Transform::new(Quat::IDENTITY, 0.0, Vec3::ZERO), Err(Degenerate::ZeroScale));
    assert_eq!(Quat::from_axis_angle(Vec3::ZERO, 1.0), Err(Degenerate::ZeroVector));
    assert_eq!(Mat4::IDENTITY.to_f32()[15], 1.0f32);
    let half = Quat::IDENTITY.slerp(q, 0.5);
    assert!(half.rotate(Vec3::X).approx_eq(v3(FRAC_PI_4.cos(), FRAC_PI_4.sin(), 0.0), T));
}

#[test]
fn bounds() {
    assert_eq!(Interval::new(2.0, 1.0), Err(Degenerate::EmptyInterval));
    let i = Interval::new(0.0, 2.0).unwrap();
    assert_eq!((i.len(), i.mid()), (2.0, 1.0));
    assert!(i.contains(2.0 + 1e-7, T) && !i.contains(2.0 + 1e-5, T));
    assert_eq!(i.intersect(Interval::new(3.0, 4.0).unwrap()), None);
    assert_eq!(i.hull(Interval::new(3.0, 4.0).unwrap()), Interval::new(0.0, 4.0).unwrap());
    assert_eq!(Aabb3::from_points(&[]), Err(Degenerate::TooFewPoints));
    let a = Aabb2::from_points(&[v2(0.0, 0.0), v2(2.0, 1.0)]).unwrap();
    let b = Aabb2::from_points(&[v2(1.0, -1.0), v2(3.0, 0.5)]).unwrap();
    assert_eq!(a.intersect(b), Some(Aabb2 { min: v2(1.0, 0.0), max: v2(2.0, 0.5) }));
    assert_eq!(a.union(b), Aabb2 { min: v2(0.0, -1.0), max: v2(3.0, 1.0) });
    assert!(a.contains_point(v2(2.0 + 5e-7, 1.0), T) && !a.contains_point(v2(2.1, 1.0), T));
    assert!(a.expand(T).contains(&a) && !a.contains(&b));
    assert_eq!(a.intersect(Aabb2::from_points(&[v2(5.0, 5.0)]).unwrap()), None);
}
