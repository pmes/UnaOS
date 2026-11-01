//! `Vec2` and `Vec3` in f64, with euclase's method names (`libs/euclase/src/vec3.rs`).

use core::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

use crate::tol::{Degenerate, Tol};

/// A 2D vector or point, millimetres.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f64,
    pub y: f64,
}

/// A 3D vector or point, millimetres.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };
    pub const X: Vec2 = Vec2 { x: 1.0, y: 0.0 };
    pub const Y: Vec2 = Vec2 { x: 0.0, y: 1.0 };

    pub const fn new(x: f64, y: f64) -> Self {
        Vec2 { x, y }
    }
    pub const fn zero() -> Self {
        Self::ZERO
    }
    pub const fn one() -> Self {
        Vec2 { x: 1.0, y: 1.0 }
    }
    pub const fn splat(v: f64) -> Self {
        Vec2 { x: v, y: v }
    }
    pub const fn unit_x() -> Self {
        Self::X
    }
    pub const fn unit_y() -> Self {
        Self::Y
    }
    pub fn dot(self, o: Self) -> f64 {
        self.x * o.x + self.y * o.y
    }
    /// The 2D cross product (perp-dot): `self.x * o.y - self.y * o.x`, positive when `o` is counterclockwise of `self`.
    pub fn cross(self, o: Self) -> f64 {
        self.x * o.y - self.y * o.x
    }
    /// The vector turned a quarter counterclockwise.
    pub fn perp(self) -> Self {
        Vec2 { x: -self.y, y: self.x }
    }
    pub fn mag_sq(self) -> f64 {
        self.dot(self)
    }
    pub fn magnitude(self) -> f64 {
        libm::sqrt(self.mag_sq())
    }
    pub fn mag(self) -> f64 {
        self.magnitude()
    }
    /// The unit vector; a zero or non-finite vector is a named refusal (euclase returns zero here).
    pub fn normalize(self) -> Result<Self, Degenerate> {
        if !self.is_finite() {
            return Err(Degenerate::NonFinite);
        }
        let m = self.magnitude();
        if m > 0.0 && m.is_finite() { Ok(self / m) } else { Err(Degenerate::ZeroVector) }
    }
    /// `t` is unclamped.
    pub fn lerp(self, o: Self, t: f64) -> Self {
        self + (o - self) * t
    }
    pub fn dist(self, o: Self) -> f64 {
        (o - self).magnitude()
    }
    /// Equal within `tol.lin`.
    pub fn approx_eq(self, o: Self, tol: Tol) -> bool {
        self.dist(o) <= tol.lin
    }
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
    /// The render-seam conversion (BP-vug-1): mesh_core's `f32` buffers only.
    pub fn to_f32(self) -> [f32; 2] {
        [self.x as f32, self.y as f32]
    }
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 { x: 0.0, y: 0.0, z: 0.0 };
    pub const X: Vec3 = Vec3 { x: 1.0, y: 0.0, z: 0.0 };
    pub const Y: Vec3 = Vec3 { x: 0.0, y: 1.0, z: 0.0 };
    pub const Z: Vec3 = Vec3 { x: 0.0, y: 0.0, z: 1.0 };

    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Vec3 { x, y, z }
    }
    pub const fn zero() -> Self {
        Self::ZERO
    }
    pub const fn one() -> Self {
        Vec3 { x: 1.0, y: 1.0, z: 1.0 }
    }
    pub const fn splat(v: f64) -> Self {
        Vec3 { x: v, y: v, z: v }
    }
    pub const fn unit_x() -> Self {
        Self::X
    }
    pub const fn unit_y() -> Self {
        Self::Y
    }
    pub const fn unit_z() -> Self {
        Self::Z
    }
    pub fn dot(self, o: Self) -> f64 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    pub fn cross(self, o: Self) -> Self {
        Vec3 { x: self.y * o.z - self.z * o.y, y: self.z * o.x - self.x * o.z, z: self.x * o.y - self.y * o.x }
    }
    pub fn mag_sq(self) -> f64 {
        self.dot(self)
    }
    pub fn magnitude(self) -> f64 {
        libm::sqrt(self.mag_sq())
    }
    pub fn mag(self) -> f64 {
        self.magnitude()
    }
    /// The unit vector; a zero or non-finite vector is a named refusal (euclase returns zero here).
    pub fn normalize(self) -> Result<Self, Degenerate> {
        if !self.is_finite() {
            return Err(Degenerate::NonFinite);
        }
        let m = self.magnitude();
        if m > 0.0 && m.is_finite() { Ok(self / m) } else { Err(Degenerate::ZeroVector) }
    }
    /// `t` is unclamped.
    pub fn lerp(self, o: Self, t: f64) -> Self {
        self + (o - self) * t
    }
    pub fn dist(self, o: Self) -> f64 {
        (o - self).magnitude()
    }
    /// Equal within `tol.lin`.
    pub fn approx_eq(self, o: Self, tol: Tol) -> bool {
        self.dist(o) <= tol.lin
    }
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
    /// A unit vector perpendicular to `self`, chosen deterministically (crossed with the axis `self` is least
    /// aligned with); a zero vector is refused.
    pub fn any_perpendicular(self) -> Result<Self, Degenerate> {
        let n = self.normalize()?;
        let (ax, ay, az) = (n.x.abs(), n.y.abs(), n.z.abs());
        let other = if ax <= ay && ax <= az {
            Vec3::X
        } else if ay <= az {
            Vec3::Y
        } else {
            Vec3::Z
        };
        n.cross(other).normalize()
    }
    /// The render-seam conversion (BP-vug-1): mesh_core's `f32` buffers only.
    pub fn to_f32(self) -> [f32; 3] {
        [self.x as f32, self.y as f32, self.z as f32]
    }
}

macro_rules! vec_ops {
    ($t:ident, $($f:ident),+) => {
        impl Add for $t {
            type Output = $t;
            fn add(self, o: $t) -> $t { $t { $($f: self.$f + o.$f),+ } }
        }
        impl Sub for $t {
            type Output = $t;
            fn sub(self, o: $t) -> $t { $t { $($f: self.$f - o.$f),+ } }
        }
        impl Mul<f64> for $t {
            type Output = $t;
            fn mul(self, s: f64) -> $t { $t { $($f: self.$f * s),+ } }
        }
        impl Mul<$t> for f64 {
            type Output = $t;
            fn mul(self, v: $t) -> $t { $t { $($f: self * v.$f),+ } }
        }
        impl Div<f64> for $t {
            type Output = $t;
            fn div(self, s: f64) -> $t { $t { $($f: self.$f / s),+ } }
        }
        impl Neg for $t {
            type Output = $t;
            fn neg(self) -> $t { $t { $($f: -self.$f),+ } }
        }
        impl AddAssign for $t {
            fn add_assign(&mut self, o: $t) { $(self.$f += o.$f;)+ }
        }
        impl SubAssign for $t {
            fn sub_assign(&mut self, o: $t) { $(self.$f -= o.$f;)+ }
        }
    };
}

vec_ops!(Vec2, x, y);
vec_ops!(Vec3, x, y, z);
