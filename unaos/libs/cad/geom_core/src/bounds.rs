//! `Interval`, `Aabb2`, `Aabb3`.

use crate::tol::{Degenerate, Tol};
use crate::vec::{Vec2, Vec3};

/// A closed interval `[lo, hi]`, `lo <= hi`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Interval {
    pub lo: f64,
    pub hi: f64,
}

impl Interval {
    pub fn new(lo: f64, hi: f64) -> Result<Self, Degenerate> {
        if !(lo.is_finite() && hi.is_finite()) {
            return Err(Degenerate::NonFinite);
        }
        if lo > hi {
            return Err(Degenerate::EmptyInterval);
        }
        Ok(Interval { lo, hi })
    }
    pub fn point(x: f64) -> Result<Self, Degenerate> {
        Self::new(x, x)
    }
    pub fn len(self) -> f64 {
        self.hi - self.lo
    }
    pub fn mid(self) -> f64 {
        self.lo + (self.hi - self.lo) * 0.5
    }
    /// `x` within `[lo - tol.lin, hi + tol.lin]`.
    pub fn contains(self, x: f64, tol: Tol) -> bool {
        x >= self.lo - tol.lin && x <= self.hi + tol.lin
    }
    /// The smallest interval holding both.
    pub fn hull(self, o: Self) -> Self {
        Interval { lo: self.lo.min(o.lo), hi: self.hi.max(o.hi) }
    }
    /// The overlap, or `None` when the two do not meet.
    pub fn intersect(self, o: Self) -> Option<Self> {
        let (lo, hi) = (self.lo.max(o.lo), self.hi.min(o.hi));
        if lo <= hi { Some(Interval { lo, hi }) } else { None }
    }
    pub fn expand(self, tol: Tol) -> Self {
        Interval { lo: self.lo - tol.lin, hi: self.hi + tol.lin }
    }
}

macro_rules! aabb {
    ($name:ident, $v:ident, $($f:ident),+) => {
        /// An axis-aligned box, `min <= max` on every axis.
        #[derive(Clone, Copy, Debug, PartialEq)]
        pub struct $name {
            pub min: $v,
            pub max: $v,
        }

        impl $name {
            /// The box of a set of points; an empty set or a non-finite point is refused.
            pub fn from_points(pts: &[$v]) -> Result<Self, Degenerate> {
                let first = *pts.first().ok_or(Degenerate::TooFewPoints)?;
                let mut b = $name { min: first, max: first };
                for p in pts {
                    if !p.is_finite() {
                        return Err(Degenerate::NonFinite);
                    }
                    $(b.min.$f = b.min.$f.min(p.$f); b.max.$f = b.max.$f.max(p.$f);)+
                }
                Ok(b)
            }
            pub fn union(self, o: Self) -> Self {
                $name { min: $v { $($f: self.min.$f.min(o.min.$f)),+ }, max: $v { $($f: self.max.$f.max(o.max.$f)),+ } }
            }
            /// The overlap, or `None` when the boxes do not meet (touching faces meet).
            pub fn intersect(self, o: Self) -> Option<Self> {
                let b = $name { min: $v { $($f: self.min.$f.max(o.min.$f)),+ }, max: $v { $($f: self.max.$f.min(o.max.$f)),+ } };
                if true $(&& b.min.$f <= b.max.$f)+ { Some(b) } else { None }
            }
            /// `p` inside the box grown by `tol.lin`.
            pub fn contains_point(self, p: $v, tol: Tol) -> bool {
                true $(&& p.$f >= self.min.$f - tol.lin && p.$f <= self.max.$f + tol.lin)+
            }
            /// `o` wholly inside `self` (exact comparison).
            pub fn contains(self, o: &Self) -> bool {
                true $(&& o.min.$f >= self.min.$f && o.max.$f <= self.max.$f)+
            }
            /// Grown by `tol.lin` on every side.
            pub fn expand(self, tol: Tol) -> Self {
                $name { min: $v { $($f: self.min.$f - tol.lin),+ }, max: $v { $($f: self.max.$f + tol.lin),+ } }
            }
            pub fn centre(self) -> $v {
                (self.min + self.max) * 0.5
            }
            pub fn size(self) -> $v {
                self.max - self.min
            }
        }
    };
}

aabb!(Aabb2, Vec2, x, y);
aabb!(Aabb3, Vec3, x, y, z);
