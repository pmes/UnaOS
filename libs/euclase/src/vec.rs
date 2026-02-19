use bytemuck::{Pod, Zeroable};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use core::ops::{Add, AddAssign, Div, DivAssign, Mul, MulAssign, Neg, Sub, SubAssign};

/// A 2-component vector.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Pod, Zeroable)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

/// A 3-component vector.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Pod, Zeroable)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// A 4-component vector.
#[repr(C)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Pod, Zeroable)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Vec2 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0 };
    pub const ONE: Self = Self { x: 1.0, y: 1.0 };
    pub const X: Self = Self { x: 1.0, y: 0.0 };
    pub const Y: Self = Self { x: 0.0, y: 1.0 };

    #[inline]
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    #[inline]
    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y
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
            Self::ZERO
        } else {
            self * (1.0 / len)
        }
    }
}

impl Vec3 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0, z: 0.0 };
    pub const ONE: Self = Self { x: 1.0, y: 1.0, z: 1.0 };
    pub const X: Self = Self { x: 1.0, y: 0.0, z: 0.0 };
    pub const Y: Self = Self { x: 0.0, y: 1.0, z: 0.0 };
    pub const Z: Self = Self { x: 0.0, y: 0.0, z: 1.0 };

    #[inline]
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    #[inline]
    pub fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    #[inline]
    pub fn cross(self, other: Self) -> Self {
        Self {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
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
            Self::ZERO
        } else {
            self * (1.0 / len)
        }
    }
}

impl Vec4 {
    pub const ZERO: Self = Self { x: 0.0, y: 0.0, z: 0.0, w: 0.0 };
    pub const ONE: Self = Self { x: 1.0, y: 1.0, z: 1.0, w: 1.0 };
    pub const X: Self = Self { x: 1.0, y: 0.0, z: 0.0, w: 0.0 };
    pub const Y: Self = Self { x: 0.0, y: 1.0, z: 0.0, w: 0.0 };
    pub const Z: Self = Self { x: 0.0, y: 0.0, z: 1.0, w: 0.0 };
    pub const W: Self = Self { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };

    #[inline]
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
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
            Self::ZERO
        } else {
            self * (1.0 / len)
        }
    }
}

macro_rules! impl_ops {
    ($T:ident, $($field:ident),+) => {
        impl Add for $T {
            type Output = Self;
            #[inline]
            fn add(self, other: Self) -> Self {
                Self {
                    $($field: self.$field + other.$field),+
                }
            }
        }

        impl AddAssign for $T {
            #[inline]
            fn add_assign(&mut self, other: Self) {
                *self = *self + other;
            }
        }

        impl Sub for $T {
            type Output = Self;
            #[inline]
            fn sub(self, other: Self) -> Self {
                Self {
                    $($field: self.$field - other.$field),+
                }
            }
        }

        impl SubAssign for $T {
            #[inline]
            fn sub_assign(&mut self, other: Self) {
                *self = *self - other;
            }
        }

        impl Mul<f32> for $T {
            type Output = Self;
            #[inline]
            fn mul(self, scalar: f32) -> Self {
                Self {
                    $($field: self.$field * scalar),+
                }
            }
        }

        impl MulAssign<f32> for $T {
            #[inline]
            fn mul_assign(&mut self, scalar: f32) {
                *self = *self * scalar;
            }
        }

        impl Mul<$T> for f32 {
            type Output = $T;
            #[inline]
            fn mul(self, vec: $T) -> $T {
                vec * self
            }
        }

        impl Mul for $T { // Component-wise multiplication
             type Output = Self;
             #[inline]
             fn mul(self, other: Self) -> Self {
                 Self {
                     $($field: self.$field * other.$field),+
                 }
             }
        }

        impl MulAssign for $T {
             #[inline]
             fn mul_assign(&mut self, other: Self) {
                 *self = *self * other;
             }
        }

        impl Div<f32> for $T {
            type Output = Self;
            #[inline]
            fn div(self, scalar: f32) -> Self {
                Self {
                    $($field: self.$field / scalar),+
                }
            }
        }

        impl DivAssign<f32> for $T {
            #[inline]
            fn div_assign(&mut self, scalar: f32) {
                *self = *self / scalar;
            }
        }

        impl Div for $T { // Component-wise division
             type Output = Self;
             #[inline]
             fn div(self, other: Self) -> Self {
                 Self {
                     $($field: self.$field / other.$field),+
                 }
             }
        }

        impl DivAssign for $T {
             #[inline]
             fn div_assign(&mut self, other: Self) {
                 *self = *self / other;
             }
        }

        impl Neg for $T {
            type Output = Self;
            #[inline]
            fn neg(self) -> Self {
                Self {
                    $($field: -self.$field),+
                }
            }
        }
    }
}

impl_ops!(Vec2, x, y);
impl_ops!(Vec3, x, y, z);
impl_ops!(Vec4, x, y, z, w);
