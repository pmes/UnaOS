use bytemuck::{Pod, Zeroable};
use core::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};
use crate::vec3::Vec3;

/// A 4-dimensional vector.
///
/// This struct is `#[repr(C)]`, `Pod`, and `Zeroable`.
/// It is 16-byte aligned and suitable for SIMD and GPU usage.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Vec4 {
    /// Creates a new `Vec4`.
    #[inline]
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    /// Creates a `Vec4` from a `Vec3` and a `w` component.
    #[inline]
    pub const fn from_vec3(v: Vec3, w: f32) -> Self {
        Self { x: v.x, y: v.y, z: v.z, w }
    }

    /// Returns the XYZ components as a `Vec3`.
    #[inline]
    pub const fn xyz(self) -> Vec3 {
        Vec3::new(self.x, self.y, self.z)
    }

    /// Creates a `Vec4` with all components set to zero.
    #[inline]
    pub const fn zero() -> Self {
        Self::new(0.0, 0.0, 0.0, 0.0)
    }

    /// Creates a `Vec4` with all components set to one.
    #[inline]
    pub const fn one() -> Self {
        Self::new(1.0, 1.0, 1.0, 1.0)
    }

    /// Creates a `Vec4` with all components set to `v`.
    #[inline]
    pub const fn splat(v: f32) -> Self {
        Self::new(v, v, v, v)
    }

    /// Calculates the dot product of this vector and another.
    #[inline]
    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z + self.w * other.w
    }

    /// Returns the squared magnitude (length) of the vector.
    #[inline]
    pub fn mag_sq(self) -> f32 {
        self.dot(self)
    }

    /// Returns the magnitude (length) of the vector.
    #[inline]
    pub fn magnitude(self) -> f32 {
        libm::sqrtf(self.mag_sq())
    }

    /// Alias for `magnitude`.
    #[inline]
    pub fn mag(self) -> f32 {
        self.magnitude()
    }

    /// Returns a normalized version of the vector.
    #[inline]
    pub fn normalize(self) -> Self {
        let m = self.magnitude();
        if m > 0.0 {
            self / m
        } else {
            Self::zero()
        }
    }

    /// Linearly interpolates between this vector and another.
    #[inline]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        self * (1.0 - t) + other * t
    }
}

impl Default for Vec4 {
    #[inline]
    fn default() -> Self {
        Self::zero()
    }
}

impl Add for Vec4 {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z, self.w + rhs.w)
    }
}

impl AddAssign for Vec4 {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl Sub for Vec4 {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z, self.w - rhs.w)
    }
}

impl SubAssign for Vec4 {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl Mul<f32> for Vec4 {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: f32) -> Self {
        Self::new(self.x * rhs, self.y * rhs, self.z * rhs, self.w * rhs)
    }
}

impl MulAssign<f32> for Vec4 {
    #[inline]
    fn mul_assign(&mut self, rhs: f32) {
        *self = *self * rhs;
    }
}

impl Mul<Vec4> for f32 {
    type Output = Vec4;
    #[inline]
    fn mul(self, rhs: Vec4) -> Vec4 {
        rhs * self
    }
}

/// Component-wise multiplication.
impl Mul<Vec4> for Vec4 {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: Self) -> Self {
        Self::new(self.x * rhs.x, self.y * rhs.y, self.z * rhs.z, self.w * rhs.w)
    }
}

impl MulAssign<Vec4> for Vec4 {
    #[inline]
    fn mul_assign(&mut self, rhs: Self) {
        *self = *self * rhs;
    }
}

impl Div<f32> for Vec4 {
    type Output = Self;
    #[inline]
    fn div(self, rhs: f32) -> Self {
        Self::new(self.x / rhs, self.y / rhs, self.z / rhs, self.w / rhs)
    }
}

impl DivAssign<f32> for Vec4 {
    #[inline]
    fn div_assign(&mut self, rhs: f32) {
        *self = *self / rhs;
    }
}

impl Div<Vec4> for Vec4 {
    type Output = Self;
    #[inline]
    fn div(self, rhs: Self) -> Self {
        Self::new(self.x / rhs.x, self.y / rhs.y, self.z / rhs.z, self.w / rhs.w)
    }
}

impl DivAssign<Vec4> for Vec4 {
    #[inline]
    fn div_assign(&mut self, rhs: Self) {
        *self = *self / rhs;
    }
}

impl Neg for Vec4 {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z, -self.w)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vec4_dot() {
        let a = Vec4::new(1.0, 2.0, 3.0, 4.0);
        let b = Vec4::new(5.0, 6.0, 7.0, 8.0);
        assert_eq!(a.dot(b), 1.0 * 5.0 + 2.0 * 6.0 + 3.0 * 7.0 + 4.0 * 8.0);
    }

    #[test]
    fn test_vec4_from_vec3() {
        let v3 = Vec3::new(1.0, 2.0, 3.0);
        let v4 = Vec4::from_vec3(v3, 4.0);
        assert_eq!(v4, Vec4::new(1.0, 2.0, 3.0, 4.0));
        assert_eq!(v4.xyz(), v3);
    }
}
