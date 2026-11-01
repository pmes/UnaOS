//! `Mat3` and `Mat4`, column-major like euclase's `Mat4 { cols }` (`libs/euclase/src/mat4.rs`).
//!
//! `inverse` carries no tolerance: it refuses only an exactly singular matrix (or one whose inverse is not finite);
//! judging the conditioning is the caller's, with its own [`crate::Tol`].

use core::ops::Mul;

use crate::quat::Quat;
use crate::tol::Degenerate;
use crate::vec::Vec3;

/// A 3x3 matrix; `cols[c]` is column `c`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat3 {
    pub cols: [Vec3; 3],
}

/// A 4x4 matrix; `cols[c][r]` is row `r` of column `c`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4 {
    pub cols: [[f64; 4]; 4],
}

impl Mat3 {
    pub const IDENTITY: Mat3 = Mat3 { cols: [Vec3::X, Vec3::Y, Vec3::Z] };

    pub const fn identity() -> Self {
        Self::IDENTITY
    }
    pub const fn zero() -> Self {
        Mat3 { cols: [Vec3::ZERO; 3] }
    }
    pub const fn from_cols(c0: Vec3, c1: Vec3, c2: Vec3) -> Self {
        Mat3 { cols: [c0, c1, c2] }
    }
    /// The rotation matrix of a quaternion (normalised first; a zero quaternion is refused).
    pub fn from_quat(q: Quat) -> Result<Self, Degenerate> {
        Ok(q.normalize()?.to_mat3())
    }
    pub fn transpose(self) -> Self {
        let [a, b, c] = self.cols;
        Mat3 { cols: [Vec3::new(a.x, b.x, c.x), Vec3::new(a.y, b.y, c.y), Vec3::new(a.z, b.z, c.z)] }
    }
    pub fn determinant(self) -> f64 {
        let [a, b, c] = self.cols;
        a.dot(b.cross(c))
    }
    pub fn inverse(self) -> Result<Self, Degenerate> {
        let [a, b, c] = self.cols;
        let det = a.dot(b.cross(c));
        if det == 0.0 || !det.is_finite() {
            return Err(Degenerate::Singular);
        }
        let inv = 1.0 / det;
        // The rows of the inverse are the cross products of the columns, over the determinant.
        let r0 = b.cross(c) * inv;
        let r1 = c.cross(a) * inv;
        let r2 = a.cross(b) * inv;
        let m = Mat3 { cols: [r0, r1, r2] }.transpose();
        if m.cols.iter().all(|v| v.is_finite()) { Ok(m) } else { Err(Degenerate::Singular) }
    }
    pub fn mul_vec3(self, v: Vec3) -> Vec3 {
        self.cols[0] * v.x + self.cols[1] * v.y + self.cols[2] * v.z
    }
}

impl Mul<Mat3> for Mat3 {
    type Output = Mat3;
    fn mul(self, o: Mat3) -> Mat3 {
        Mat3 { cols: [self.mul_vec3(o.cols[0]), self.mul_vec3(o.cols[1]), self.mul_vec3(o.cols[2])] }
    }
}

impl Mul<Vec3> for Mat3 {
    type Output = Vec3;
    fn mul(self, v: Vec3) -> Vec3 {
        self.mul_vec3(v)
    }
}

impl Mat4 {
    pub const IDENTITY: Mat4 =
        Mat4 { cols: [[1.0, 0.0, 0.0, 0.0], [0.0, 1.0, 0.0, 0.0], [0.0, 0.0, 1.0, 0.0], [0.0, 0.0, 0.0, 1.0]] };

    pub const fn identity() -> Self {
        Self::IDENTITY
    }
    pub const fn zero() -> Self {
        Mat4 { cols: [[0.0; 4]; 4] }
    }
    pub const fn from_cols(c0: [f64; 4], c1: [f64; 4], c2: [f64; 4], c3: [f64; 4]) -> Self {
        Mat4 { cols: [c0, c1, c2, c3] }
    }
    pub fn from_translation(v: Vec3) -> Self {
        let mut m = Self::IDENTITY;
        m.cols[3] = [v.x, v.y, v.z, 1.0];
        m
    }
    pub fn from_scale(v: Vec3) -> Self {
        let mut m = Self::IDENTITY;
        m.cols[0][0] = v.x;
        m.cols[1][1] = v.y;
        m.cols[2][2] = v.z;
        m
    }
    /// The rotation of a quaternion (normalised first; a zero quaternion is refused).
    pub fn from_rotation(q: Quat) -> Result<Self, Degenerate> {
        Ok(Self::from_mat3(Mat3::from_quat(q)?))
    }
    /// The 3x3 block in the upper left, the rest identity.
    pub fn from_mat3(m: Mat3) -> Self {
        let [a, b, c] = m.cols;
        Mat4 { cols: [[a.x, a.y, a.z, 0.0], [b.x, b.y, b.z, 0.0], [c.x, c.y, c.z, 0.0], [0.0, 0.0, 0.0, 1.0]] }
    }
    pub fn transpose(self) -> Self {
        let mut m = Self::zero();
        for c in 0..4 {
            for r in 0..4 {
                m.cols[c][r] = self.cols[r][c];
            }
        }
        m
    }
    pub fn mul_vec4(self, v: [f64; 4]) -> [f64; 4] {
        let mut o = [0.0; 4];
        for (c, col) in self.cols.iter().enumerate() {
            for r in 0..4 {
                o[r] += col[r] * v[c];
            }
        }
        o
    }
    /// The affine image of a point (w = 1; no perspective divide, as euclase's `transform_point3`).
    pub fn transform_point3(self, p: Vec3) -> Vec3 {
        let o = self.mul_vec4([p.x, p.y, p.z, 1.0]);
        Vec3::new(o[0], o[1], o[2])
    }
    /// The image of a direction (w = 0).
    pub fn transform_vector3(self, v: Vec3) -> Vec3 {
        let o = self.mul_vec4([v.x, v.y, v.z, 0.0]);
        Vec3::new(o[0], o[1], o[2])
    }
    fn flat(self) -> [f64; 16] {
        let mut a = [0.0; 16];
        for c in 0..4 {
            for r in 0..4 {
                a[c * 4 + r] = self.cols[c][r];
            }
        }
        a
    }
    fn unflat(a: [f64; 16]) -> Self {
        let mut m = Self::zero();
        for c in 0..4 {
            for r in 0..4 {
                m.cols[c][r] = a[c * 4 + r];
            }
        }
        m
    }
    pub fn determinant(self) -> f64 {
        let b = Self::minors(&self.flat());
        b[0] * b[11] - b[1] * b[10] + b[2] * b[9] + b[3] * b[8] - b[4] * b[7] + b[5] * b[6]
    }
    // The twelve 2x2 minors of the column pairs (0,1) and (2,3).
    fn minors(a: &[f64; 16]) -> [f64; 12] {
        [
            a[0] * a[5] - a[1] * a[4],
            a[0] * a[6] - a[2] * a[4],
            a[0] * a[7] - a[3] * a[4],
            a[1] * a[6] - a[2] * a[5],
            a[1] * a[7] - a[3] * a[5],
            a[2] * a[7] - a[3] * a[6],
            a[8] * a[13] - a[9] * a[12],
            a[8] * a[14] - a[10] * a[12],
            a[8] * a[15] - a[11] * a[12],
            a[9] * a[14] - a[10] * a[13],
            a[9] * a[15] - a[11] * a[13],
            a[10] * a[15] - a[11] * a[14],
        ]
    }
    pub fn inverse(self) -> Result<Self, Degenerate> {
        let a = self.flat();
        let b = Self::minors(&a);
        let det = b[0] * b[11] - b[1] * b[10] + b[2] * b[9] + b[3] * b[8] - b[4] * b[7] + b[5] * b[6];
        if det == 0.0 || !det.is_finite() {
            return Err(Degenerate::Singular);
        }
        let i = 1.0 / det;
        let o = [
            (a[5] * b[11] - a[6] * b[10] + a[7] * b[9]) * i,
            (a[2] * b[10] - a[1] * b[11] - a[3] * b[9]) * i,
            (a[13] * b[5] - a[14] * b[4] + a[15] * b[3]) * i,
            (a[10] * b[4] - a[9] * b[5] - a[11] * b[3]) * i,
            (a[6] * b[8] - a[4] * b[11] - a[7] * b[7]) * i,
            (a[0] * b[11] - a[2] * b[8] + a[3] * b[7]) * i,
            (a[14] * b[2] - a[12] * b[5] - a[15] * b[1]) * i,
            (a[8] * b[5] - a[10] * b[2] + a[11] * b[1]) * i,
            (a[4] * b[10] - a[5] * b[8] + a[7] * b[6]) * i,
            (a[1] * b[8] - a[0] * b[10] - a[3] * b[6]) * i,
            (a[12] * b[4] - a[13] * b[2] + a[15] * b[0]) * i,
            (a[9] * b[2] - a[8] * b[4] - a[11] * b[0]) * i,
            (a[5] * b[7] - a[4] * b[9] - a[6] * b[6]) * i,
            (a[0] * b[9] - a[1] * b[7] + a[2] * b[6]) * i,
            (a[13] * b[1] - a[12] * b[3] - a[14] * b[0]) * i,
            (a[8] * b[3] - a[9] * b[1] + a[10] * b[0]) * i,
        ];
        if o.iter().all(|v| v.is_finite()) { Ok(Self::unflat(o)) } else { Err(Degenerate::Singular) }
    }
    /// The render-seam conversion (BP-vug-1), column-major in euclase's `to_cols_array` order.
    pub fn to_f32(self) -> [f32; 16] {
        let a = self.flat();
        let mut o = [0.0f32; 16];
        for (d, s) in o.iter_mut().zip(a.iter()) {
            *d = *s as f32;
        }
        o
    }
}

impl Mul<Mat4> for Mat4 {
    type Output = Mat4;
    fn mul(self, o: Mat4) -> Mat4 {
        Mat4 { cols: [self.mul_vec4(o.cols[0]), self.mul_vec4(o.cols[1]), self.mul_vec4(o.cols[2]), self.mul_vec4(o.cols[3])] }
    }
}
