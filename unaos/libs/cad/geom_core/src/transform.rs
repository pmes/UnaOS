//! `Transform`: a rigid motion with a uniform scale, `p -> scale * rot(p) + trans`.

use crate::mat::Mat4;
use crate::quat::Quat;
use crate::tol::Degenerate;
use crate::vec::Vec3;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    /// A unit quaternion.
    pub rot: Quat,
    /// Positive and finite.
    pub scale: f64,
    pub trans: Vec3,
}

impl Transform {
    pub const IDENTITY: Transform = Transform { rot: Quat::IDENTITY, scale: 1.0, trans: Vec3::ZERO };

    /// Normalises `rot`; refuses a zero quaternion, a non-positive or non-finite scale, a non-finite translation.
    pub fn new(rot: Quat, scale: f64, trans: Vec3) -> Result<Self, Degenerate> {
        if !(scale.is_finite() && scale > 0.0) {
            return Err(Degenerate::ZeroScale);
        }
        if !trans.is_finite() {
            return Err(Degenerate::NonFinite);
        }
        Ok(Transform { rot: rot.normalize()?, scale, trans })
    }
    pub fn from_translation(t: Vec3) -> Self {
        Transform { trans: t, ..Self::IDENTITY }
    }
    pub fn from_rotation(rot: Quat) -> Result<Self, Degenerate> {
        Self::new(rot, 1.0, Vec3::ZERO)
    }
    /// `self ∘ inner`: apply `inner` first, then `self`.
    pub fn compose(self, inner: Transform) -> Transform {
        let rot = (self.rot * inner.rot).normalize().unwrap_or(self.rot);
        Transform { rot, scale: self.scale * inner.scale, trans: self.rot.rotate(inner.trans) * self.scale + self.trans }
    }
    /// The inverse; refuses a scale whose reciprocal is not finite.
    pub fn invert(self) -> Result<Transform, Degenerate> {
        let s = 1.0 / self.scale;
        if !(s.is_finite() && s > 0.0) {
            return Err(Degenerate::ZeroScale);
        }
        let rot = self.rot.conjugate();
        Ok(Transform { rot, scale: s, trans: rot.rotate(self.trans) * -s })
    }
    pub fn apply_point(self, p: Vec3) -> Vec3 {
        self.rot.rotate(p) * self.scale + self.trans
    }
    pub fn apply_vector(self, v: Vec3) -> Vec3 {
        self.rot.rotate(v) * self.scale
    }
    /// A uniform scale leaves a normal's direction alone: a unit normal in, a unit normal out.
    pub fn apply_normal(self, n: Vec3) -> Vec3 {
        self.rot.rotate(n)
    }
    pub fn to_mat4(self) -> Mat4 {
        let m = self.rot.to_mat3();
        let s = self.scale;
        let [a, b, c] = m.cols;
        Mat4::from_cols(
            [a.x * s, a.y * s, a.z * s, 0.0],
            [b.x * s, b.y * s, b.z * s, 0.0],
            [c.x * s, c.y * s, c.z * s, 0.0],
            [self.trans.x, self.trans.y, self.trans.z, 1.0],
        )
    }
}

impl Default for Transform {
    fn default() -> Self {
        Self::IDENTITY
    }
}
