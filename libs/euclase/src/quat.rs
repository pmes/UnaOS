use bytemuck::{Pod, Zeroable};
use core::ops::{Add, Mul, Neg, Sub};
use crate::vec3::Vec3;

/// A quaternion for representing rotation.
///
/// This struct is `#[repr(C)]`, `Pod`, and `Zeroable`.
/// Components: x, y, z (vector part), w (scalar part).
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    /// Identity quaternion (no rotation).
    #[inline]
    pub const fn identity() -> Self {
        Self { x: 0.0, y: 0.0, z: 0.0, w: 1.0 }
    }

    /// Creates a quaternion from an axis and an angle (in radians).
    /// Axis must be normalized.
    #[inline]
    pub fn from_axis_angle(axis: Vec3, angle: f32) -> Self {
        let half_angle = angle * 0.5;
        let s = libm::sinf(half_angle);
        let c = libm::cosf(half_angle);
        Self {
            x: axis.x * s,
            y: axis.y * s,
            z: axis.z * s,
            w: c,
        }
    }

    /// Normalizes the quaternion.
    #[inline]
    pub fn normalize(self) -> Self {
        let mag_sq = self.dot(self);
        if mag_sq > 0.0 {
            let m = libm::sqrtf(mag_sq);
            Self {
                x: self.x / m,
                y: self.y / m,
                z: self.z / m,
                w: self.w / m,
            }
        } else {
            Self::identity()
        }
    }

    /// Dot product.
    #[inline]
    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z + self.w * other.w
    }

    /// Spherical Linear Interpolation.
    #[inline]
    pub fn slerp(self, mut other: Self, t: f32) -> Self {
        let mut dot = self.dot(other);

        // If the dot product is negative, the quaternions point in opposite directions.
        // Negate one to take the shorter path.
        if dot < 0.0 {
            other = -other;
            dot = -dot;
        }

        const DOT_THRESHOLD: f32 = 0.9995;
        if dot > DOT_THRESHOLD {
            // Linear interpolation for small angles to avoid division by zero.
            // (1-t)*self + t*other
            let result = self * (1.0 - t) + other * t;
            return result.normalize();
        }

        // Clamp dot to avoid NaN with acos
        if dot > 1.0 { dot = 1.0; }
        if dot < -1.0 { dot = -1.0; }

        let theta_0 = libm::acosf(dot);
        let sin_theta_0 = libm::sinf(theta_0);

        // We know sin_theta_0 != 0 because dot <= threshold (0.9995)

        // s0 = sin((1-t)*theta_0) / sin(theta_0)
        // s1 = sin(t*theta_0) / sin(theta_0)
        let s0 = libm::sinf((1.0 - t) * theta_0) / sin_theta_0;
        let s1 = libm::sinf(t * theta_0) / sin_theta_0;

        self * s0 + other * s1
    }
}

impl Default for Quat {
    #[inline]
    fn default() -> Self {
        Self::identity()
    }
}

impl Add for Quat {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
            z: self.z + rhs.z,
            w: self.w + rhs.w,
        }
    }
}

impl Sub for Quat {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self {
            x: self.x - rhs.x,
            y: self.y - rhs.y,
            z: self.z - rhs.z,
            w: self.w - rhs.w,
        }
    }
}

impl Mul<f32> for Quat {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: f32) -> Self {
        Self {
            x: self.x * rhs,
            y: self.y * rhs,
            z: self.z * rhs,
            w: self.w * rhs,
        }
    }
}

impl Neg for Quat {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
            z: -self.z,
            w: -self.w,
        }
    }
}

/// Quaternion multiplication (Hamilton product).
/// Represents composing rotation: `self * rhs` means apply `rhs` then `self`?
/// No, `(Q2 * Q1) * v` applies Q1 then Q2.
impl Mul<Quat> for Quat {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: Self) -> Self {
        Self {
            x: self.w * rhs.x + self.x * rhs.w + self.y * rhs.z - self.z * rhs.y,
            y: self.w * rhs.y - self.x * rhs.z + self.y * rhs.w + self.z * rhs.x,
            z: self.w * rhs.z + self.x * rhs.y - self.y * rhs.x + self.z * rhs.w,
            w: self.w * rhs.w - self.x * rhs.x - self.y * rhs.y - self.z * rhs.z,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quat_identity() {
        let q = Quat::identity();
        assert_eq!(q.w, 1.0);
        assert_eq!(q.x, 0.0);
    }

    #[test]
    fn test_quat_mul() {
        // Rotating 90 degrees around X, then 90 around Y.
        let q1 = Quat::from_axis_angle(Vec3::unit_x(), core::f32::consts::FRAC_PI_2);
        let q2 = Quat::from_axis_angle(Vec3::unit_y(), core::f32::consts::FRAC_PI_2);
        let q3 = q2 * q1; // Apply q1 then q2
        // We expect specific values.
        // But mainly just checking it compiles and runs without panic for now.
        let _ = q3.normalize();
    }
}
