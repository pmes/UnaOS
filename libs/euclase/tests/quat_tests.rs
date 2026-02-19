use euclase::vec::{Vec3, Vec4};
use euclase::quat::Quat;

#[test]
fn test_quat_identity() {
    let q = Quat::IDENTITY;
    let v = Vec3::new(1.0, 2.0, 3.0);
    assert_eq!(q * v, v);
}

#[test]
fn test_quat_rotation() {
    // Rotate 90 degrees around X axis.
    let axis = Vec3::X;
    let angle = core::f32::consts::FRAC_PI_2;
    let q = Quat::from_axis_angle(axis, angle);

    // Y axis should become Z axis.
    let v = Vec3::Y;
    let rotated = q * v;

    // (0, 1, 0) -> (0, 0, 1)
    let expected = Vec3::Z;

    // Approx comparison needed
    assert!((rotated.x - expected.x).abs() < 1e-5);
    assert!((rotated.y - expected.y).abs() < 1e-5);
    assert!((rotated.z - expected.z).abs() < 1e-5);
}

#[test]
fn test_quat_mul() {
    // Rotate 90 X then 90 Y.
    let q1 = Quat::from_axis_angle(Vec3::X, core::f32::consts::FRAC_PI_2);
    let q2 = Quat::from_axis_angle(Vec3::Y, core::f32::consts::FRAC_PI_2);

    // q2 * q1 means apply q1 then q2.
    let q = q2 * q1;

    // v = Z.
    // q1(Z) = -Y.
    // q2(-Y) = -Y (since Y rotation doesn't affect Y axis).
    // So q2(-Y) = -Y.

    let v = Vec3::Z;
    let rotated = q * v;

    let expected = -Vec3::Y;

    assert!((rotated.x - expected.x).abs() < 1e-5);
    assert!((rotated.y - expected.y).abs() < 1e-5);
    assert!((rotated.z - expected.z).abs() < 1e-5);
}

#[test]
fn test_quat_slerp() {
    // Slerp from 0 to 90 degrees. Midpoint should be 45 degrees.
    let q1 = Quat::from_axis_angle(Vec3::Z, 0.0);
    let q2 = Quat::from_axis_angle(Vec3::Z, core::f32::consts::FRAC_PI_2);

    let q_mid = q1.slerp(q2, 0.5);
    let q_expected = Quat::from_axis_angle(Vec3::Z, core::f32::consts::FRAC_PI_4);

    // Dot product should be close to 1 (or -1, double cover).
    assert!((q_mid.dot(q_expected).abs() - 1.0).abs() < 1e-5);
}

#[test]
fn test_quat_to_mat4() {
    let q = Quat::from_axis_angle(Vec3::Z, core::f32::consts::FRAC_PI_2);
    let m = q.to_mat4();

    // Rotate vector X by matrix. Should be Y.
    let v = Vec4::new(1.0, 0.0, 0.0, 1.0);
    let rotated = m * v;

    // (0, 1, 0, 1)
    assert!((rotated.x - 0.0).abs() < 1e-5);
    assert!((rotated.y - 1.0).abs() < 1e-5);
    assert!((rotated.z - 0.0).abs() < 1e-5);
    assert!((rotated.w - 1.0).abs() < 1e-5);
}
