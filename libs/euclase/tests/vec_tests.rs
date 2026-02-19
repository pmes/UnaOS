use euclase::vec::{Vec2, Vec3};

#[test]
fn test_vec2_math() {
    let v1 = Vec2::new(1.0, 2.0);
    let v2 = Vec2::new(3.0, 4.0);
    assert_eq!(v1 + v2, Vec2::new(4.0, 6.0));
    assert_eq!(v1 - v2, Vec2::new(-2.0, -2.0));
    assert_eq!(v1 * 2.0, Vec2::new(2.0, 4.0));
    assert_eq!(v1 / 2.0, Vec2::new(0.5, 1.0));
    assert_eq!(v1.dot(v2), 11.0);
}

#[test]
fn test_vec3_math() {
    let v1 = Vec3::new(1.0, 2.0, 3.0);
    let v2 = Vec3::new(4.0, 5.0, 6.0);
    assert_eq!(v1 + v2, Vec3::new(5.0, 7.0, 9.0));
    assert_eq!(v1.cross(v2), Vec3::new(-3.0, 6.0, -3.0));
    assert_eq!(v1.dot(v2), 32.0);
}

#[test]
fn test_normalization() {
    let v = Vec3::new(1.0, 0.0, 0.0);
    assert_eq!(v.normalize(), v);

    let v = Vec3::new(2.0, 0.0, 0.0);
    assert_eq!(v.normalize(), Vec3::new(1.0, 0.0, 0.0));

    let v = Vec3::ZERO;
    assert_eq!(v.normalize(), Vec3::ZERO);
}
