use euclase::vec::Vec3;
use euclase::ray::Ray;

#[test]
fn test_ray_at() {
    let ray = Ray::new(Vec3::new(1.0, 2.0, 3.0), Vec3::new(0.0, 0.0, 1.0));
    assert_eq!(ray.at(0.0), Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(ray.at(1.0), Vec3::new(1.0, 2.0, 4.0));
    assert_eq!(ray.at(-1.0), Vec3::new(1.0, 2.0, 2.0));
}
