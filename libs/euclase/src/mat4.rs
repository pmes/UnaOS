use bytemuck::{Pod, Zeroable};
use core::ops::Mul;
use crate::vec3::Vec3;
use crate::vec4::Vec4;
use crate::quat::Quat;

/// A 4x4 matrix, stored in column-major order.
///
/// This struct is `#[repr(C)]`, `Pod`, and `Zeroable`.
/// It is compatible with WGSL `mat4x4<f32>`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Mat4 {
    pub cols: [Vec4; 4],
}

impl Mat4 {
    /// Identity matrix.
    #[inline]
    pub const fn identity() -> Self {
        Self {
            cols: [
                Vec4::new(1.0, 0.0, 0.0, 0.0),
                Vec4::new(0.0, 1.0, 0.0, 0.0),
                Vec4::new(0.0, 0.0, 1.0, 0.0),
                Vec4::new(0.0, 0.0, 0.0, 1.0),
            ],
        }
    }

    /// Zero matrix.
    #[inline]
    pub const fn zero() -> Self {
        Self {
            cols: [
                Vec4::new(0.0, 0.0, 0.0, 0.0),
                Vec4::new(0.0, 0.0, 0.0, 0.0),
                Vec4::new(0.0, 0.0, 0.0, 0.0),
                Vec4::new(0.0, 0.0, 0.0, 0.0),
            ],
        }
    }

    /// Creates a translation matrix.
    #[inline]
    pub fn from_translation(v: Vec3) -> Self {
        Self {
            cols: [
                Vec4::new(1.0, 0.0, 0.0, 0.0),
                Vec4::new(0.0, 1.0, 0.0, 0.0),
                Vec4::new(0.0, 0.0, 1.0, 0.0),
                Vec4::new(v.x, v.y, v.z, 1.0),
            ],
        }
    }

    /// Creates a scale matrix.
    #[inline]
    pub fn from_scale(v: Vec3) -> Self {
        Self {
            cols: [
                Vec4::new(v.x, 0.0, 0.0, 0.0),
                Vec4::new(0.0, v.y, 0.0, 0.0),
                Vec4::new(0.0, 0.0, v.z, 0.0),
                Vec4::new(0.0, 0.0, 0.0, 1.0),
            ],
        }
    }

    /// Creates a rotation matrix from a quaternion.
    #[inline]
    pub fn from_rotation(q: Quat) -> Self {
        let x2 = q.x + q.x;
        let y2 = q.y + q.y;
        let z2 = q.z + q.z;

        let xx = q.x * x2;
        let xy = q.x * y2;
        let xz = q.x * z2;
        let yy = q.y * y2;
        let yz = q.y * z2;
        let zz = q.z * z2;
        let wx = q.w * x2;
        let wy = q.w * y2;
        let wz = q.w * z2;

        Self {
            cols: [
                Vec4::new(1.0 - (yy + zz), xy + wz, xz - wy, 0.0),
                Vec4::new(xy - wz, 1.0 - (xx + zz), yz + wx, 0.0),
                Vec4::new(xz + wy, yz - wx, 1.0 - (xx + yy), 0.0),
                Vec4::new(0.0, 0.0, 0.0, 1.0),
            ],
        }
    }

    /// Creates a perspective projection matrix (Right-Handed, Z in [0, 1]).
    /// `fov_y` is in radians.
    #[inline]
    pub fn perspective_rh_zo(fov_y: f32, aspect_ratio: f32, z_near: f32, z_far: f32) -> Self {
        let f = 1.0 / libm::tanf(fov_y * 0.5);
        let fn_inv = 1.0 / (z_near - z_far);

        Self {
            cols: [
                Vec4::new(f / aspect_ratio, 0.0, 0.0, 0.0),
                Vec4::new(0.0, f, 0.0, 0.0),
                Vec4::new(0.0, 0.0, z_far * fn_inv, -1.0),
                Vec4::new(0.0, 0.0, z_near * z_far * fn_inv, 0.0),
            ],
        }
    }

    /// Creates a view matrix (Look At).
    /// Right-Handed: Z comes out of screen (eye - center).
    #[inline]
    pub fn look_at_rh(eye: Vec3, center: Vec3, up: Vec3) -> Self {
        let f = (center - eye).normalize(); // Forward (into screen, -Z)
        let s = f.cross(up).normalize();    // Right (+X)
        let u = s.cross(f);                 // Up (+Y)

        Self {
            cols: [
                Vec4::new(s.x, u.x, -f.x, 0.0),
                Vec4::new(s.y, u.y, -f.y, 0.0),
                Vec4::new(s.z, u.z, -f.z, 0.0),
                Vec4::new(-s.dot(eye), -u.dot(eye), f.dot(eye), 1.0),
            ],
        }
    }

    /// Multiplies this matrix by a vector.
    #[inline]
    pub fn mul_vec4(self, v: Vec4) -> Vec4 {
        self.cols[0] * v.x + self.cols[1] * v.y + self.cols[2] * v.z + self.cols[3] * v.w
    }

    /// Transforms a point (assuming w=1). Returns Vec3 (with perspective divide).
    #[inline]
    pub fn transform_point3(self, v: Vec3) -> Vec3 {
        let res = self.mul_vec4(Vec4::from_vec3(v, 1.0));
        res.xyz() / res.w
    }

    /// Transforms a vector (assuming w=0). Returns Vec3.
    #[inline]
    pub fn transform_vector3(self, v: Vec3) -> Vec3 {
        let res = self.mul_vec4(Vec4::from_vec3(v, 0.0));
        res.xyz()
    }
}

impl Default for Mat4 {
    #[inline]
    fn default() -> Self {
        Self::identity()
    }
}

impl Mul<Mat4> for Mat4 {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: Self) -> Self {
        Self {
            cols: [
                self.mul_vec4(rhs.cols[0]),
                self.mul_vec4(rhs.cols[1]),
                self.mul_vec4(rhs.cols[2]),
                self.mul_vec4(rhs.cols[3]),
            ],
        }
    }
}

impl Mul<Vec4> for Mat4 {
    type Output = Vec4;
    #[inline]
    fn mul(self, rhs: Vec4) -> Vec4 {
        self.mul_vec4(rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mat4_identity() {
        let m = Mat4::identity();
        let v = Vec4::new(1.0, 2.0, 3.0, 4.0);
        let v_prime = m * v;
        assert_eq!(v, v_prime);
    }

    #[test]
    fn test_mat4_translation() {
        let t = Vec3::new(10.0, 20.0, 30.0);
        let m = Mat4::from_translation(t);
        let p = Vec3::new(1.0, 2.0, 3.0);
        let p_prime = m.transform_point3(p);
        assert_eq!(p_prime, Vec3::new(11.0, 22.0, 33.0));
    }

    #[test]
    fn test_mat4_mul() {
        let t1 = Mat4::from_translation(Vec3::new(1.0, 0.0, 0.0));
        let t2 = Mat4::from_translation(Vec3::new(0.0, 1.0, 0.0));
        let t3 = t2 * t1; // Apply t1 then t2
        let p = Vec3::zero();
        let p_prime = t3.transform_point3(p);
        assert_eq!(p_prime, Vec3::new(1.0, 1.0, 0.0));
    }

    #[test]
    fn test_perspective_bounds() {
        let z_near = 1.0;
        let z_far = 10.0;
        let m = Mat4::perspective_rh_zo(to_radians(90.0), 1.0, z_near, z_far);

        // Near plane point (Z = -near in view space)
        let p_near = Vec3::new(0.0, 0.0, -z_near);
        let ndc_near = m.transform_point3(p_near);
        // Z should be 0.0
        assert!((ndc_near.z).abs() < 1e-6);

        // Far plane point (Z = -far in view space)
        let p_far = Vec3::new(0.0, 0.0, -z_far);
        let ndc_far = m.transform_point3(p_far);
        // Z should be 1.0
        assert!((ndc_far.z - 1.0).abs() < 1e-6);
    }

    fn to_radians(deg: f32) -> f32 {
        deg * (core::f32::consts::PI / 180.0)
    }
}
