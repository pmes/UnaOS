use crate::vec::{Vec3, Vec4};
use crate::mat::Mat4;
use bytemuck::{Pod, Zeroable};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use core::ops::{Add, Mul, MulAssign, Neg};

#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Pod, Zeroable)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Default for Quat {
    #[inline]
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Quat {
    pub const IDENTITY: Self = Self { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };

    #[inline]
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    #[inline]
    pub fn from_axis_angle(axis: Vec3, angle_radians: f32) -> Self {
        let (s, c) = libm::sincosf(angle_radians * 0.5);
        let v = axis.normalize() * s;
        Self {
            x: v.x,
            y: v.y,
            z: v.z,
            w: c,
        }
    }

    #[inline]
    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z + self.w * other.w
    }

    #[inline]
    pub fn length_squared(self) -> f32 {
        self.dot(self)
    }

    #[inline]
    pub fn length(self) -> f32 {
        libm::sqrtf(self.length_squared())
    }

    #[inline]
    pub fn normalize(self) -> Self {
        let len = self.length();
        if len == 0.0 {
            Self::IDENTITY
        } else {
            let inv_len = 1.0 / len;
            Self {
                x: self.x * inv_len,
                y: self.y * inv_len,
                z: self.z * inv_len,
                w: self.w * inv_len,
            }
        }
    }

    #[inline]
    pub fn conjugate(self) -> Self {
        Self {
            x: -self.x,
            y: -self.y,
            z: -self.z,
            w: self.w,
        }
    }

    #[inline]
    pub fn inverse(self) -> Self {
        let sq_len = self.length_squared();
        if sq_len == 0.0 {
            Self::IDENTITY
        } else {
            let inv_sq_len = 1.0 / sq_len;
            Self {
                x: -self.x * inv_sq_len,
                y: -self.y * inv_sq_len,
                z: -self.z * inv_sq_len,
                w: self.w * inv_sq_len,
            }
        }
    }

    #[inline]
    pub fn slerp(self, mut end: Self, t: f32) -> Self {
        let mut cos_theta = self.dot(end);

        if cos_theta < 0.0 {
            end = -end;
            cos_theta = -cos_theta;
        }

        if cos_theta > 0.9995 {
            // Linear interpolation for small angles
            return ((self * (1.0 - t)) + (end * t)).normalize();
        }

        let theta = libm::acosf(cos_theta);
        let sin_theta = libm::sinf(theta);

        let w1 = libm::sinf((1.0 - t) * theta) / sin_theta;
        let w2 = libm::sinf(t * theta) / sin_theta;

        (self * w1) + (end * w2)
    }

    #[inline]
    pub fn to_mat4(self) -> Mat4 {
        let x2 = self.x + self.x;
        let y2 = self.y + self.y;
        let z2 = self.z + self.z;

        let xx = self.x * x2;
        let xy = self.x * y2;
        let xz = self.x * z2;

        let yy = self.y * y2;
        let yz = self.y * z2;
        let zz = self.z * z2;

        let wx = self.w * x2;
        let wy = self.w * y2;
        let wz = self.w * z2;

        Mat4::from_cols(
            Vec4::new(1.0 - (yy + zz), xy + wz, xz - wy, 0.0),
            Vec4::new(xy - wz, 1.0 - (xx + zz), yz + wx, 0.0),
            Vec4::new(xz + wy, yz - wx, 1.0 - (xx + yy), 0.0),
            Vec4::W,
        )
    }
}

impl Mul<Quat> for Quat {
    type Output = Quat;
    #[inline]
    fn mul(self, rhs: Quat) -> Quat {
        Self {
            x: self.w * rhs.x + self.x * rhs.w + self.y * rhs.z - self.z * rhs.y,
            y: self.w * rhs.y - self.x * rhs.z + self.y * rhs.w + self.z * rhs.x,
            z: self.w * rhs.z + self.x * rhs.y - self.y * rhs.x + self.z * rhs.w,
            w: self.w * rhs.w - self.x * rhs.x - self.y * rhs.y - self.z * rhs.z,
        }
    }
}

impl MulAssign<Quat> for Quat {
    #[inline]
    fn mul_assign(&mut self, rhs: Quat) {
        *self = *self * rhs;
    }
}

impl Mul<Vec3> for Quat {
    type Output = Vec3;
    #[inline]
    fn mul(self, v: Vec3) -> Vec3 {
        let q_vec = Vec3::new(self.x, self.y, self.z);
        let uv = q_vec.cross(v);
        let uuv = q_vec.cross(uv);
        v + ((uv * self.w) + uuv) * 2.0
    }
}

impl Add<Quat> for Quat {
    type Output = Quat;
    #[inline]
    fn add(self, rhs: Quat) -> Quat {
        Self {
            x: self.x + rhs.x,
            y: self.y + rhs.y,
            z: self.z + rhs.z,
            w: self.w + rhs.w,
        }
    }
}

impl Mul<f32> for Quat {
    type Output = Quat;
    #[inline]
    fn mul(self, s: f32) -> Quat {
        Self {
            x: self.x * s,
            y: self.y * s,
            z: self.z * s,
            w: self.w * s,
        }
    }
}

impl Neg for Quat {
    type Output = Quat;
    #[inline]
    fn neg(self) -> Quat {
        Self {
            x: -self.x,
            y: -self.y,
            z: -self.z,
            w: -self.w,
        }
    }
}
