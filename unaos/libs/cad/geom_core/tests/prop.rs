//! Property tests over a Fibonacci sphere of directions (deterministic, no rand crate).

use geom_core::*;

const N: usize = 500;
const T: Tol = Tol::DEFAULT;

/// N near-uniform unit directions: the golden-angle spiral.
fn fibonacci(n: usize) -> Vec<Vec3> {
    let golden = core::f64::consts::PI * (3.0 - 5f64.sqrt());
    (0..n)
        .map(|i| {
            let y = 1.0 - 2.0 * (i as f64 + 0.5) / n as f64;
            let r = (1.0 - y * y).sqrt();
            let phi = i as f64 * golden;
            Vec3::new(phi.cos() * r, y, phi.sin() * r)
        })
        .collect()
}

#[test]
fn directions_are_unit_and_normalize_holds() {
    for d in fibonacci(N) {
        assert!((d.mag() - 1.0).abs() < 1e-15);
        let n = (d * 37.5).normalize().unwrap();
        assert!((n.mag() - 1.0).abs() < 1e-15 && n.approx_eq(d, T));
    }
}

#[test]
fn rotations_keep_lengths_and_axes() {
    let dirs = fibonacci(N);
    for (i, &axis) in dirs.iter().enumerate() {
        let q = Quat::from_axis_angle(axis, 0.7 + i as f64 * 0.01).unwrap();
        assert!(q.rotate(axis).approx_eq(axis, T));
        let v = dirs[(i * 7 + 3) % N] * 12.0;
        let r = q.rotate(v);
        assert!((r.mag() - 12.0).abs() < 1e-12);
        assert!(q.to_mat3().mul_vec3(v).approx_eq(r, T));
        let back = q.conjugate().rotate(r);
        assert!(back.approx_eq(v, T));
    }
}

#[test]
fn transform_round_trips() {
    let dirs = fibonacci(N);
    for (i, &d) in dirs.iter().enumerate() {
        let q = Quat::from_axis_angle(d, 1.0 + i as f64 * 0.003).unwrap();
        let t = Transform::new(q, 0.5 + (i % 5) as f64, d * 30.0).unwrap();
        let inv = t.invert().unwrap();
        let p = dirs[(i * 11 + 5) % N] * 100.0;
        assert!(inv.apply_point(t.apply_point(p)).approx_eq(p, T));
        assert!(t.compose(inv).apply_point(p).approx_eq(p, T));
        assert!(t.to_mat4().inverse().unwrap().transform_point3(t.apply_point(p)).approx_eq(p, T));
        let n = dirs[(i * 13 + 1) % N];
        assert!((t.apply_normal(n).mag() - 1.0).abs() < 1e-14);
    }
}

#[test]
fn rays_from_every_direction() {
    let cube = Aabb3::from_points(&[Vec3::splat(-1.0), Vec3::splat(1.0)]).unwrap();
    for d in fibonacci(N) {
        let ray = Ray::new(d * 10.0, -d).unwrap();
        match ray_sphere(&ray, Vec3::ZERO, 1.0, T).unwrap() {
            RaySphere::Hit { t_near, t_far } => assert!((t_near - 9.0).abs() < 1e-12 && (t_far - 11.0).abs() < 1e-12),
            o => panic!("{d:?} {o:?}"),
        }
        match ray_aabb(&ray, &cube) {
            RayBox::Hit { t_enter, t_exit } => {
                assert!(t_enter >= 10.0 - 3f64.sqrt() - 1e-12 && t_enter <= 9.0 + 1e-12, "{t_enter}");
                assert!((t_enter + t_exit - 20.0).abs() < 1e-12);
            }
            o => panic!("{d:?} {o:?}"),
        }
    }
}

#[test]
fn orient3d_is_antisymmetric() {
    let dirs = fibonacci(N);
    let flip = |p: Predicate| match p {
        Predicate::Positive => Predicate::Negative,
        Predicate::Negative => Predicate::Positive,
        o => o,
    };
    for i in 0..N - 3 {
        let (a, b, c, d) = (dirs[i], dirs[i + 1] * 2.0, dirs[i + 2] * 3.0, dirs[i + 3] * 0.5);
        let s = orient3d(a, b, c, d);
        assert!(s.sign().is_some());
        assert_eq!(orient3d(b, a, c, d), flip(s));
        assert_eq!(orient3d(a, b, d, c), flip(s));
        // Agrees with the plain determinant wherever that is far from zero.
        let det = (a - d).dot((b - d).cross(c - d));
        if det.abs() > 1e-9 {
            assert_eq!(s, if det > 0.0 { Predicate::Positive } else { Predicate::Negative });
        }
    }
}

#[test]
fn plane_distances() {
    let dirs = fibonacci(N);
    for (i, &n) in dirs.iter().enumerate() {
        let pl = Plane::from_point_normal(n * 2.0, n).unwrap();
        let p = dirs[(i * 17 + 9) % N] * 5.0;
        let (q, dist) = closest_point_plane(p, &pl);
        assert!((dist - (n.dot(p) - 2.0)).abs() < 1e-12);
        assert!(pl.signed_distance(q).abs() < 1e-12);
    }
}
