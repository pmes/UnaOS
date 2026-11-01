//! `Quat` in f64 with euclase's names (`libs/euclase/src/quat.rs`): `w` is the scalar part.

use core::ops::Mul;

use crate::mat::Mat3;
use crate::tol::Degenerate;
use crate::vec::Vec3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quat {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

impl Quat {
    pub const IDENTITY: Quat = Quat { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };

    pub const fn identity() -> Self {
        Self::IDENTITY
    }
    pub const fn new(x: f64, y: f64, z: f64, w: f64) -> Self {
        Quat { x, y, z, w }
    }
    /// A rotation of `angle` radians (right-handed) about `axis`; a zero axis is refused.
    pub fn from_axis_angle(axis: Vec3, angle: f64) -> Result<Self, Degenerate> {
        if !angle.is_finite() {
            return Err(Degenerate::NonFinite);
        }
        let a = axis.normalize()?;
        let (s, c) = (libm::sin(angle * 0.5), libm::cos(angle * 0.5));
        Ok(Quat { x: a.x * s, y: a.y * s, z: a.z * s, w: c })
    }
    pub fn dot(self, o: Self) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z + self.w * o.w
    }
    pub fn normalize(self) -> Result<Self, Degenerate> {
        let m = libm::sqrt(self.dot(self));
        if !m.is_finite() {
            return Err(Degenerate::NonFinite);
        }
        if m == 0.0 {
            return Err(Degenerate::ZeroVector);
        }
        Ok(Quat { x: self.x / m, y: self.y / m, z: self.z / m, w: self.w / m })
    }
    /// The inverse of a unit quaternion.
    pub fn conjugate(self) -> Self {
        Quat { x: -self.x, y: -self.y, z: -self.z, w: self.w }
    }
    /// Rotate `v` by this (unit) quaternion.
    pub fn rotate(self, v: Vec3) -> Vec3 {
        let u = Vec3::new(self.x, self.y, self.z);
        let t = u.cross(v) * 2.0;
        v + t * self.w + u.cross(t)
    }
    /// Spherical interpolation along the shorter arc; `t` in [0, 1].
    pub fn slerp(self, other: Self, t: f64) -> Self {
        let mut o = other;
        let mut d = self.dot(o);
        if d < 0.0 {
            o = Quat { x: -o.x, y: -o.y, z: -o.z, w: -o.w };
            d = -d;
        }
        let (a, b) = if d > 0.9995 {
            (1.0 - t, t)
        } else {
            let th = libm::acos(d.min(1.0));
            let s = libm::sin(th);
            (libm::sin((1.0 - t) * th) / s, libm::sin(t * th) / s)
        };
        let q = Quat { x: self.x * a + o.x * b, y: self.y * a + o.y * b, z: self.z * a + o.z * b, w: self.w * a + o.w * b };
        q.normalize().unwrap_or(self)
    }
    /// The rotation matrix of this (unit) quaternion.
    pub fn to_mat3(self) -> Mat3 {
        let (x, y, z, w) = (self.x, self.y, self.z, self.w);
        let (x2, y2, z2) = (x + x, y + y, z + z);
        let (xx, yy, zz) = (x * x2, y * y2, z * z2);
        let (xy, xz, yz) = (x * y2, x * z2, y * z2);
        let (wx, wy, wz) = (w * x2, w * y2, w * z2);
        Mat3::from_cols(
            Vec3::new(1.0 - (yy + zz), xy + wz, xz - wy),
            Vec3::new(xy - wz, 1.0 - (xx + zz), yz + wx),
            Vec3::new(xz + wy, yz - wx, 1.0 - (xx + yy)),
        )
    }
    /// The render-seam conversion (BP-vug-1): `[x, y, z, w]`.
    pub fn to_f32(self) -> [f32; 4] {
        [self.x as f32, self.y as f32, self.z as f32, self.w as f32]
    }
}

impl Default for Quat {
    fn default() -> Self {
        Self::IDENTITY
    }
}

/// The Hamilton product: `(a * b).rotate(v) == a.rotate(b.rotate(v))`.
impl Mul<Quat> for Quat {
    type Output = Quat;
    fn mul(self, b: Quat) -> Quat {
        let a = self;
        Quat {
            x: a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
            y: a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
            z: a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
            w: a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
        }
    }
}
