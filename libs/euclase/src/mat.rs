use crate::vec::{Vec3, Vec4};
use bytemuck::{Pod, Zeroable};
#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};
use core::ops::Mul;

/// A 3x3 matrix, column-major.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Pod, Zeroable)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Mat3 {
    pub cols: [Vec3; 3],
}

impl Default for Mat3 {
    #[inline]
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mat3 {
    pub const IDENTITY: Self = Self {
        cols: [
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
        ],
    };

    pub const ZERO: Self = Self {
        cols: [Vec3::ZERO; 3],
    };

    #[inline]
    pub const fn from_cols(x: Vec3, y: Vec3, z: Vec3) -> Self {
        Self { cols: [x, y, z] }
    }

    #[inline]
    pub fn transpose(&self) -> Self {
        Self {
            cols: [
                Vec3::new(self.cols[0].x, self.cols[1].x, self.cols[2].x),
                Vec3::new(self.cols[0].y, self.cols[1].y, self.cols[2].y),
                Vec3::new(self.cols[0].z, self.cols[1].z, self.cols[2].z),
            ],
        }
    }
}

impl Mul<Mat3> for Mat3 {
    type Output = Mat3;
    #[inline]
    fn mul(self, rhs: Mat3) -> Mat3 {
        let a = self.cols;
        let b = rhs.cols;

        let row0 = Vec3::new(a[0].x, a[1].x, a[2].x);
        let row1 = Vec3::new(a[0].y, a[1].y, a[2].y);
        let row2 = Vec3::new(a[0].z, a[1].z, a[2].z);

        Self {
            cols: [
                Vec3::new(row0.dot(b[0]), row1.dot(b[0]), row2.dot(b[0])),
                Vec3::new(row0.dot(b[1]), row1.dot(b[1]), row2.dot(b[1])),
                Vec3::new(row0.dot(b[2]), row1.dot(b[2]), row2.dot(b[2])),
            ],
        }
    }
}

impl Mul<Vec3> for Mat3 {
    type Output = Vec3;
    #[inline]
    fn mul(self, rhs: Vec3) -> Vec3 {
        self.cols[0] * rhs.x + self.cols[1] * rhs.y + self.cols[2] * rhs.z
    }
}

/// A 4x4 matrix, column-major.
#[repr(C)]
#[derive(Copy, Clone, Debug, PartialEq, Pod, Zeroable)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct Mat4 {
    pub cols: [Vec4; 4],
}

impl Default for Mat4 {
    #[inline]
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Mat4 {
    pub const IDENTITY: Self = Self {
        cols: [
            Vec4::X,
            Vec4::Y,
            Vec4::Z,
            Vec4::W,
        ],
    };

    pub const ZERO: Self = Self {
        cols: [Vec4::ZERO; 4],
    };

    #[inline]
    pub const fn from_cols(x: Vec4, y: Vec4, z: Vec4, w: Vec4) -> Self {
        Self { cols: [x, y, z, w] }
    }

    #[inline]
    pub fn from_cols_array(m: &[f32; 16]) -> Self {
        Self {
            cols: [
                Vec4::new(m[0], m[1], m[2], m[3]),
                Vec4::new(m[4], m[5], m[6], m[7]),
                Vec4::new(m[8], m[9], m[10], m[11]),
                Vec4::new(m[12], m[13], m[14], m[15]),
            ],
        }
    }

    #[inline]
    pub fn to_cols_array(&self) -> [f32; 16] {
        [
            self.cols[0].x, self.cols[0].y, self.cols[0].z, self.cols[0].w,
            self.cols[1].x, self.cols[1].y, self.cols[1].z, self.cols[1].w,
            self.cols[2].x, self.cols[2].y, self.cols[2].z, self.cols[2].w,
            self.cols[3].x, self.cols[3].y, self.cols[3].z, self.cols[3].w,
        ]
    }

    #[inline]
    pub fn transpose(&self) -> Self {
        Self {
            cols: [
                Vec4::new(self.cols[0].x, self.cols[1].x, self.cols[2].x, self.cols[3].x),
                Vec4::new(self.cols[0].y, self.cols[1].y, self.cols[2].y, self.cols[3].y),
                Vec4::new(self.cols[0].z, self.cols[1].z, self.cols[2].z, self.cols[3].z),
                Vec4::new(self.cols[0].w, self.cols[1].w, self.cols[2].w, self.cols[3].w),
            ],
        }
    }

    #[inline]
    pub fn from_scale(scale: Vec3) -> Self {
        Self {
            cols: [
                Vec4::new(scale.x, 0.0, 0.0, 0.0),
                Vec4::new(0.0, scale.y, 0.0, 0.0),
                Vec4::new(0.0, 0.0, scale.z, 0.0),
                Vec4::W,
            ],
        }
    }

    #[inline]
    pub fn from_translation(translation: Vec3) -> Self {
        Self {
            cols: [
                Vec4::X,
                Vec4::Y,
                Vec4::Z,
                Vec4::new(translation.x, translation.y, translation.z, 1.0),
            ],
        }
    }

    #[inline]
    pub fn look_at_rh(eye: Vec3, center: Vec3, up: Vec3) -> Self {
        let f = (center - eye).normalize();
        let s = f.cross(up).normalize();
        let u = s.cross(f);

        Self {
            cols: [
                Vec4::new(s.x, u.x, -f.x, 0.0),
                Vec4::new(s.y, u.y, -f.y, 0.0),
                Vec4::new(s.z, u.z, -f.z, 0.0),
                Vec4::new(-s.dot(eye), -u.dot(eye), f.dot(eye), 1.0),
            ],
        }
    }

    #[inline]
    pub fn perspective_rh_gl(fov_y_radians: f32, aspect_ratio: f32, z_near: f32, z_far: f32) -> Self {
        let inv_length = 1.0 / (z_near - z_far);
        let f = 1.0 / libm::tanf(0.5 * fov_y_radians);
        let a = f / aspect_ratio;
        let b = (z_near + z_far) * inv_length;
        let c = (2.0 * z_near * z_far) * inv_length;

        Self {
            cols: [
                Vec4::new(a, 0.0, 0.0, 0.0),
                Vec4::new(0.0, f, 0.0, 0.0),
                Vec4::new(0.0, 0.0, b, -1.0),
                Vec4::new(0.0, 0.0, c, 0.0),
            ],
        }
    }

    #[inline]
    pub fn inverse(&self) -> Self {
        let m = self.to_cols_array();

        let mut inv = [0.0; 16];

        inv[0] = m[5]  * m[10] * m[15] -
                 m[5]  * m[11] * m[14] -
                 m[9]  * m[6]  * m[15] +
                 m[9]  * m[7]  * m[14] +
                 m[13] * m[6]  * m[11] -
                 m[13] * m[7]  * m[10];

        inv[4] = -m[4]  * m[10] * m[15] +
                  m[4]  * m[11] * m[14] +
                  m[8]  * m[6]  * m[15] -
                  m[8]  * m[7]  * m[14] -
                  m[12] * m[6]  * m[11] +
                  m[12] * m[7]  * m[10];

        inv[8] = m[4]  * m[9] * m[15] -
                 m[4]  * m[11] * m[13] -
                 m[8]  * m[5] * m[15] +
                 m[8]  * m[7] * m[13] +
                 m[12] * m[5] * m[11] -
                 m[12] * m[7] * m[9];

        inv[12] = -m[4]  * m[9] * m[14] +
                   m[4]  * m[10] * m[13] +
                   m[8]  * m[5] * m[14] -
                   m[8]  * m[6] * m[13] -
                   m[12] * m[5] * m[10] +
                   m[12] * m[6] * m[9];

        inv[1] = -m[1]  * m[10] * m[15] +
                  m[1]  * m[11] * m[14] +
                  m[9]  * m[2] * m[15] -
                  m[9]  * m[3] * m[14] -
                  m[13] * m[2] * m[11] +
                  m[13] * m[3] * m[10];

        inv[5] = m[0]  * m[10] * m[15] -
                 m[0]  * m[11] * m[14] -
                 m[8]  * m[2] * m[15] +
                 m[8]  * m[3] * m[14] +
                 m[12] * m[2] * m[11] -
                 m[12] * m[3] * m[10];

        inv[9] = -m[0]  * m[9] * m[15] +
                  m[0]  * m[11] * m[13] +
                  m[8]  * m[1] * m[15] -
                  m[8]  * m[3] * m[13] -
                  m[12] * m[1] * m[11] +
                  m[12] * m[3] * m[9];

        inv[13] = m[0]  * m[9] * m[14] -
                  m[0]  * m[10] * m[13] -
                  m[8]  * m[1] * m[14] +
                  m[8]  * m[2] * m[13] +
                  m[12] * m[1] * m[10] -
                  m[12] * m[2] * m[9];

        inv[2] = m[1]  * m[6] * m[15] -
                 m[1]  * m[7] * m[14] -
                 m[5]  * m[2] * m[15] +
                 m[5]  * m[3] * m[14] +
                 m[13] * m[2] * m[7] -
                 m[13] * m[3] * m[6];

        inv[6] = -m[0]  * m[6] * m[15] +
                  m[0]  * m[7] * m[14] +
                  m[4]  * m[2] * m[15] -
                  m[4]  * m[3] * m[14] -
                  m[12] * m[2] * m[7] +
                  m[12] * m[3] * m[6];

        inv[10] = m[0]  * m[5] * m[15] -
                  m[0]  * m[7] * m[13] -
                  m[4]  * m[1] * m[15] +
                  m[4]  * m[3] * m[13] +
                  m[12] * m[1] * m[7] -
                  m[12] * m[3] * m[5];

        inv[14] = -m[0]  * m[5] * m[14] +
                   m[0]  * m[6] * m[13] +
                   m[4]  * m[1] * m[14] -
                   m[4]  * m[2] * m[13] -
                   m[12] * m[1] * m[6] +
                   m[12] * m[2] * m[5];

        inv[3] = -m[1] * m[6] * m[11] +
                  m[1] * m[7] * m[10] +
                  m[5] * m[2] * m[11] -
                  m[5] * m[3] * m[10] -
                  m[9] * m[2] * m[7] +
                  m[9] * m[3] * m[6];

        inv[7] = m[0] * m[6] * m[11] -
                 m[0] * m[7] * m[10] -
                 m[4] * m[2] * m[11] +
                 m[4] * m[3] * m[10] +
                 m[8] * m[2] * m[7] -
                 m[8] * m[3] * m[6];

        inv[11] = -m[0] * m[5] * m[11] +
                   m[0] * m[7] * m[9] +
                   m[4] * m[1] * m[11] -
                   m[4] * m[3] * m[9] -
                   m[8] * m[1] * m[7] +
                   m[8] * m[3] * m[5];

        inv[15] = m[0] * m[5] * m[10] -
                  m[0] * m[6] * m[9] -
                  m[4] * m[1] * m[10] +
                  m[4] * m[2] * m[9] +
                  m[8] * m[1] * m[6] -
                  m[8] * m[2] * m[5];

        let det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];

        if det == 0.0 {
            return Self::ZERO;
        }

        let inv_det = 1.0 / det;

        let mut res_arr = [0.0; 16];
        for i in 0..16 {
            res_arr[i] = inv[i] * inv_det;
        }

        Self::from_cols_array(&res_arr)
    }

    #[inline]
    pub fn determinant(&self) -> f32 {
         let m = self.to_cols_array();

         // Simplified cofactor expansion for det, but inverse already computes it.
         // Just copy paste the cofactor calculation for the first row?
         // det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
         // Note: inv[0], inv[4]... calculated in inverse are cofactors (transposed adjugate? No, adjugate is transpose of cofactor matrix).
         // My `inverse` implementation computes the adjugate matrix into `inv` array.
         // inv[0] is cofactor C00. inv[4] is cofactor C10 (because inv is adjugate, so it's transposed).
         // det = sum(m[0][i] * C0i).
         // m[0] is m00. inv[0] is C00.
         // m[1] is m10. inv[4] is C10.
         // Wait.
         // m[0] = m00. m[1] = m10 (row 1, col 0).
         // If I expand along first column:
         // det = m00 * C00 + m10 * C10 + m20 * C20 + m30 * C30.

         // In my code: `det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];`
         // m[0] is m00. inv[0] (calculated above) involves m5, m10, m15... (submatrix excluding row 0 col 0).

         // So yes, I can reuse the logic or just re-implement it.
         // To avoid code duplication I could extract it, but "inline everything" suggests copy paste is fine for performance/simplicity in no_std context without helper functions.

         // I'll implement determinant separately using standard formula.

         let m00 = m[0]; let m01 = m[4]; let m02 = m[8]; let m03 = m[12];
         let m10 = m[1]; let m11 = m[5]; let m12 = m[9]; let m13 = m[13];
         let m20 = m[2]; let m21 = m[6]; let m22 = m[10]; let m23 = m[14];
         let m30 = m[3]; let m31 = m[7]; let m32 = m[11]; let m33 = m[15];

         m03 * m12 * m21 * m30 - m02 * m13 * m21 * m30 -
         m03 * m11 * m22 * m30 + m01 * m13 * m22 * m30 +
         m02 * m11 * m23 * m30 - m01 * m12 * m23 * m30 -
         m03 * m12 * m20 * m31 + m02 * m13 * m20 * m31 +
         m03 * m10 * m22 * m31 - m00 * m13 * m22 * m31 -
         m02 * m10 * m23 * m31 + m00 * m12 * m23 * m31 +
         m03 * m11 * m20 * m32 - m01 * m13 * m20 * m32 -
         m03 * m10 * m21 * m32 + m00 * m13 * m21 * m32 +
         m01 * m10 * m23 * m32 - m00 * m11 * m23 * m32 -
         m02 * m11 * m20 * m33 + m01 * m12 * m20 * m33 +
         m02 * m10 * m21 * m33 - m00 * m12 * m21 * m33 -
         m01 * m10 * m22 * m33 + m00 * m11 * m22 * m33
    }
}

impl Mul<Mat4> for Mat4 {
    type Output = Mat4;
    #[inline]
    fn mul(self, rhs: Mat4) -> Mat4 {
        let a = self.cols;
        let b = rhs.cols;

        let row0 = Vec4::new(a[0].x, a[1].x, a[2].x, a[3].x);
        let row1 = Vec4::new(a[0].y, a[1].y, a[2].y, a[3].y);
        let row2 = Vec4::new(a[0].z, a[1].z, a[2].z, a[3].z);
        let row3 = Vec4::new(a[0].w, a[1].w, a[2].w, a[3].w);

        Self {
            cols: [
                Vec4::new(row0.dot(b[0]), row1.dot(b[0]), row2.dot(b[0]), row3.dot(b[0])),
                Vec4::new(row0.dot(b[1]), row1.dot(b[1]), row2.dot(b[1]), row3.dot(b[1])),
                Vec4::new(row0.dot(b[2]), row1.dot(b[2]), row2.dot(b[2]), row3.dot(b[2])),
                Vec4::new(row0.dot(b[3]), row1.dot(b[3]), row2.dot(b[3]), row3.dot(b[3])),
            ],
        }
    }
}

impl Mul<Vec4> for Mat4 {
    type Output = Vec4;
    #[inline]
    fn mul(self, rhs: Vec4) -> Vec4 {
        self.cols[0] * rhs.x + self.cols[1] * rhs.y + self.cols[2] * rhs.z + self.cols[3] * rhs.w
    }
}
