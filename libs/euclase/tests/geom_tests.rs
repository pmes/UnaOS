use euclase::vec::Vec3;
use euclase::ray::Ray;
use euclase::geom::{Sphere, AABB};

#[test]
fn test_sphere_intersection() {
    let sphere = Sphere::new(Vec3::new(0.0, 0.0, 5.0), 1.0);

    let ray = Ray::new(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0));
    assert!(sphere.intersect(&ray).is_some());
    assert!((sphere.intersect(&ray).unwrap() - 4.0).abs() < 1e-5);

    let ray_miss = Ray::new(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
    assert!(sphere.intersect(&ray_miss).is_none());
}

#[test]
fn test_aabb_intersection() {
    let aabb = AABB::new(Vec3::new(-1.0, -1.0, 2.0), Vec3::new(1.0, 1.0, 4.0));

    let ray = Ray::new(Vec3::new(0.0, 0.0, 0.0), Vec3::new(0.0, 0.0, 1.0));
    assert!(aabb.intersect(&ray).is_some());
    assert!((aabb.intersect(&ray).unwrap() - 2.0).abs() < 1e-5);

    let ray_inside = Ray::new(Vec3::new(0.0, 0.0, 3.0), Vec3::new(0.0, 0.0, 1.0));
    assert!(aabb.intersect(&ray_inside).is_some());
    assert!((aabb.intersect(&ray_inside).unwrap() - 1.0).abs() < 1e-5);

    let ray_miss = Ray::new(Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
    assert!(aabb.intersect(&ray_miss).is_none());
}
