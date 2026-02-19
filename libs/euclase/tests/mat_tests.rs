use euclase::vec::{Vec3, Vec4};
use euclase::mat::{Mat3, Mat4};

#[test]
fn test_mat4_identity() {
    let m = Mat4::IDENTITY;
    let v = Vec4::new(1.0, 2.0, 3.0, 4.0);
    assert_eq!(m * v, v);
}

#[test]
fn test_mat4_mul() {
    let m1 = Mat4::from_translation(Vec3::new(1.0, 2.0, 3.0));
    let m2 = Mat4::from_scale(Vec3::new(2.0, 2.0, 2.0));
    // Translation then scale vs Scale then translation
    // m1 * m2 corresponds to applying m2 first then m1 (if column vectors).
    // v' = m1 * (m2 * v)
    // Scale by 2, then translate by (1,2,3).

    let m = m1 * m2;
    let v = Vec4::new(1.0, 1.0, 1.0, 1.0);
    let v_prime = m * v;

    // Scale (1,1,1) -> (2,2,2). Translate -> (3,4,5).
    assert_eq!(v_prime, Vec4::new(3.0, 4.0, 5.0, 1.0));
}

#[test]
fn test_mat4_inverse() {
    let m = Mat4::from_scale(Vec3::new(2.0, 4.0, 8.0));
    let inv = m.inverse();
    let identity = m * inv;
    // Floating point errors might occur, but for clean powers of 2 it should be exact.
    assert_eq!(identity, Mat4::IDENTITY);
}

#[test]
fn test_mat4_determinant() {
    let m = Mat4::from_scale(Vec3::new(2.0, 3.0, 4.0));
    assert_eq!(m.determinant(), 24.0);

    let m = Mat4::IDENTITY;
    assert_eq!(m.determinant(), 1.0);
}

#[test]
fn test_mat3_mul() {
    let m = Mat3::IDENTITY;
    let v = Vec3::new(1.0, 2.0, 3.0);
    assert_eq!(m * v, v);
}
