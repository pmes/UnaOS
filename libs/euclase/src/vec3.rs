use bytemuck::{Pod, Zeroable};
use core::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};

/// A 3-dimensional vector.
///
/// This struct is `#[repr(C)]`, `Pod`, and `Zeroable`, making it safe to memcpy directly
/// to GPU buffers. It contains three `f32` components: x, y, z.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    /// Creates a new `Vec3`.
    #[inline]
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    /// Creates a `Vec3` with all components set to zero.
    #[inline]
    pub const fn zero() -> Self {
        Self::new(0.0, 0.0, 0.0)
    }

    /// Creates a `Vec3` with all components set to one.
    #[inline]
    pub const fn one() -> Self {
        Self::new(1.0, 1.0, 1.0)
    }

    /// Creates a `Vec3` with all components set to `v`.
    #[inline]
    pub const fn splat(v: f32) -> Self {
        Self::new(v, v, v)
    }

    /// The X axis (1, 0, 0).
    #[inline]
    pub const fn unit_x() -> Self {
        Self::new(1.0, 0.0, 0.0)
    }

    /// The Y axis (0, 1, 0).
    #[inline]
    pub const fn unit_y() -> Self {
        Self::new(0.0, 1.0, 0.0)
    }

    /// The Z axis (0, 0, 1).
    #[inline]
    pub const fn unit_z() -> Self {
        Self::new(0.0, 0.0, 1.0)
    }

    /// Calculates the dot product of this vector and another.
    ///
    /// The dot product represents the alignment of two vectors.
    #[inline]
    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    /// Calculates the cross product of this vector and another.
    ///
    /// The cross product results in a vector orthogonal to both input vectors.
    /// This uses a right-handed coordinate system.
    #[inline]
    pub fn cross(self, other: Self) -> Self {
        Self {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
    }

    /// Returns the squared magnitude (length) of the vector.
    ///
    /// This is faster than `magnitude()` as it avoids a square root.
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

    /// Returns a normalized version of the vector (length = 1.0).
    ///
    /// If the vector has zero length, it returns the zero vector.
    #[inline]
    pub fn normalize(self) -> Self {
        let m = self.magnitude();
        if m > 0.0 {
            self / m
        } else {
            Self::zero()
        }
    }

    /// Linearly interpolates between this vector and another based on `t`.
    ///
    /// `t` is unclamped.
    #[inline]
    pub fn lerp(self, other: Self, t: f32) -> Self {
        self * (1.0 - t) + other * t
    }
}

impl Default for Vec3 {
    #[inline]
    fn default() -> Self {
        Self::zero()
    }
}

impl Add for Vec3 {
    type Output = Self;
    #[inline]
    fn add(self, rhs: Self) -> Self {
        Self::new(self.x + rhs.x, self.y + rhs.y, self.z + rhs.z)
    }
}

impl AddAssign for Vec3 {
    #[inline]
    fn add_assign(&mut self, rhs: Self) {
        *self = *self + rhs;
    }
}

impl Sub for Vec3 {
    type Output = Self;
    #[inline]
    fn sub(self, rhs: Self) -> Self {
        Self::new(self.x - rhs.x, self.y - rhs.y, self.z - rhs.z)
    }
}

impl SubAssign for Vec3 {
    #[inline]
    fn sub_assign(&mut self, rhs: Self) {
        *self = *self - rhs;
    }
}

impl Mul<f32> for Vec3 {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: f32) -> Self {
        Self::new(self.x * rhs, self.y * rhs, self.z * rhs)
    }
}

impl MulAssign<f32> for Vec3 {
    #[inline]
    fn mul_assign(&mut self, rhs: f32) {
        *self = *self * rhs;
    }
}

impl Mul<Vec3> for f32 {
    type Output = Vec3;
    #[inline]
    fn mul(self, rhs: Vec3) -> Vec3 {
        rhs * self
    }
}

/// Component-wise multiplication.
impl Mul<Vec3> for Vec3 {
    type Output = Self;
    #[inline]
    fn mul(self, rhs: Self) -> Self {
        Self::new(self.x * rhs.x, self.y * rhs.y, self.z * rhs.z)
    }
}

impl MulAssign<Vec3> for Vec3 {
    #[inline]
    fn mul_assign(&mut self, rhs: Self) {
        *self = *self * rhs;
    }
}

impl Div<f32> for Vec3 {
    type Output = Self;
    #[inline]
    fn div(self, rhs: f32) -> Self {
        Self::new(self.x / rhs, self.y / rhs, self.z / rhs)
    }
}

impl DivAssign<f32> for Vec3 {
    #[inline]
    fn div_assign(&mut self, rhs: f32) {
        *self = *self / rhs;
    }
}

impl Div<Vec3> for Vec3 {
    type Output = Self;
    #[inline]
    fn div(self, rhs: Self) -> Self {
        Self::new(self.x / rhs.x, self.y / rhs.y, self.z / rhs.z)
    }
}

impl DivAssign<Vec3> for Vec3 {
    #[inline]
    fn div_assign(&mut self, rhs: Self) {
        *self = *self / rhs;
    }
}

impl Neg for Vec3 {
    type Output = Self;
    #[inline]
    fn neg(self) -> Self {
        Self::new(-self.x, -self.y, -self.z)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vec3_dot() {
        let a = Vec3::new(1.0, 2.0, 3.0);
        let b = Vec3::new(4.0, 5.0, 6.0);
        assert_eq!(a.dot(b), 1.0 * 4.0 + 2.0 * 5.0 + 3.0 * 6.0);
    }

    #[test]
    fn test_vec3_cross_orthogonality() {
        let a = Vec3::new(1.0, 0.0, 0.0);
        let b = Vec3::new(0.0, 1.0, 0.0);
        let c = a.cross(b);
        // Cross product of X and Y is Z
        assert_eq!(c, Vec3::new(0.0, 0.0, 1.0));
        // Result should be orthogonal to inputs
        assert_eq!(c.dot(a), 0.0);
        assert_eq!(c.dot(b), 0.0);
    }

    #[test]
    fn test_vec3_normalize() {
        let a = Vec3::new(3.0, 0.0, 0.0);
        let n = a.normalize();
        assert_eq!(n, Vec3::new(1.0, 0.0, 0.0));
    }
}
