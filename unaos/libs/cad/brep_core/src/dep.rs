//! The sibling seam: every name brep_core takes from geom_core, sketch_core and mesh_core comes through this ONE module,
//! and the rest of the crate imports `crate::dep::*` and nothing else.
//!
//! At the VUGBREP1 build none of the three siblings had committed on its branch (exec-rmbp-vuggeom, -vugsketch,
//! -vugmesh). Each block below is therefore a minimal PRIVATE SHIM of exactly the names their designs publish. At the
//! fold the seat swaps each block for the `pub use` line in its header and adds the dep line in Cargo.toml.

pub use self::geom::*;
pub use self::mesh::*;
pub use self::sketch::*;

// SHIM: geom_core (SR464) not on its branch at build; the seat swaps this block for
// `pub use geom_core::{Vec2, Vec3, Tol, Degenerate, Predicate, Unsure, orient2d, Arc2, Plane, Line, PlanePlane,
//  plane_plane, Circle2, CircleCircle, circle_circle2};` + the Cargo dep.
mod geom {
    use core::ops::{Add, AddAssign, Div, Mul, Neg, Sub};

    /// geom_core's 2D f64 vector (euclase names).
    #[derive(Clone, Copy, Debug, PartialEq, Default)]
    pub struct Vec2 {
        pub x: f64,
        pub y: f64,
    }
    impl Vec2 {
        pub const fn new(x: f64, y: f64) -> Self {
            Vec2 { x, y }
        }
        pub fn dot(self, o: Self) -> f64 {
            self.x * o.x + self.y * o.y
        }
        /// The scalar perp-dot (z of the 3D cross).
        pub fn cross(self, o: Self) -> f64 {
            self.x * o.y - self.y * o.x
        }
        pub fn mag_sq(self) -> f64 {
            self.dot(self)
        }
        pub fn mag(self) -> f64 {
            libm::sqrt(self.mag_sq())
        }
        pub fn normalize(self) -> Result<Self, Degenerate> {
            let m = self.mag();
            if !m.is_finite() {
                return Err(Degenerate::NonFinite);
            }
            if m == 0.0 {
                return Err(Degenerate::ZeroVector);
            }
            Ok(Vec2::new(self.x / m, self.y / m))
        }
    }
    impl Add for Vec2 {
        type Output = Self;
        fn add(self, o: Self) -> Self {
            Vec2::new(self.x + o.x, self.y + o.y)
        }
    }
    impl Sub for Vec2 {
        type Output = Self;
        fn sub(self, o: Self) -> Self {
            Vec2::new(self.x - o.x, self.y - o.y)
        }
    }
    impl Mul<f64> for Vec2 {
        type Output = Self;
        fn mul(self, s: f64) -> Self {
            Vec2::new(self.x * s, self.y * s)
        }
    }
    impl Neg for Vec2 {
        type Output = Self;
        fn neg(self) -> Self {
            Vec2::new(-self.x, -self.y)
        }
    }

    /// geom_core's 3D f64 vector (euclase names; `normalize` refuses a zero vector by name).
    #[derive(Clone, Copy, Debug, PartialEq, Default)]
    pub struct Vec3 {
        pub x: f64,
        pub y: f64,
        pub z: f64,
    }
    impl Vec3 {
        pub const ZERO: Vec3 = Vec3::new(0.0, 0.0, 0.0);
        pub const X: Vec3 = Vec3::new(1.0, 0.0, 0.0);
        pub const Y: Vec3 = Vec3::new(0.0, 1.0, 0.0);
        pub const Z: Vec3 = Vec3::new(0.0, 0.0, 1.0);
        pub const fn new(x: f64, y: f64, z: f64) -> Self {
            Vec3 { x, y, z }
        }
        pub fn dot(self, o: Self) -> f64 {
            self.x * o.x + self.y * o.y + self.z * o.z
        }
        pub fn cross(self, o: Self) -> Self {
            Vec3::new(self.y * o.z - self.z * o.y, self.z * o.x - self.x * o.z, self.x * o.y - self.y * o.x)
        }
        pub fn mag_sq(self) -> f64 {
            self.dot(self)
        }
        pub fn mag(self) -> f64 {
            libm::sqrt(self.mag_sq())
        }
        pub fn normalize(self) -> Result<Self, Degenerate> {
            let m = self.mag();
            if !m.is_finite() {
                return Err(Degenerate::NonFinite);
            }
            if m == 0.0 {
                return Err(Degenerate::ZeroVector);
            }
            Ok(self / m)
        }
        pub fn lerp(self, o: Self, t: f64) -> Self {
            self + (o - self) * t
        }
        pub fn dist(self, o: Self) -> f64 {
            (self - o).mag()
        }
    }
    impl Add for Vec3 {
        type Output = Self;
        fn add(self, o: Self) -> Self {
            Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
        }
    }
    impl AddAssign for Vec3 {
        fn add_assign(&mut self, o: Self) {
            *self = *self + o;
        }
    }
    impl Sub for Vec3 {
        type Output = Self;
        fn sub(self, o: Self) -> Self {
            Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
        }
    }
    impl Mul<f64> for Vec3 {
        type Output = Self;
        fn mul(self, s: f64) -> Self {
            Vec3::new(self.x * s, self.y * s, self.z * s)
        }
    }
    impl Div<f64> for Vec3 {
        type Output = Self;
        fn div(self, s: f64) -> Self {
            Vec3::new(self.x / s, self.y / s, self.z / s)
        }
    }
    impl Neg for Vec3 {
        type Output = Self;
        fn neg(self) -> Self {
            Vec3::new(-self.x, -self.y, -self.z)
        }
    }

    /// geom_core's one tolerance policy: linear in millimetres, angular in radians (BP-vug-2).
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Tol {
        pub lin: f64,
        pub ang: f64,
    }
    impl Tol {
        pub const DEFAULT: Tol = Tol { lin: 1e-6, ang: 1e-9 };
    }

    /// geom_core's named degeneracies (the subset brep_core meets).
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Degenerate {
        ZeroVector,
        ZeroRadius,
        NonFinite,
        Singular,
        Collinear,
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Unsure {
        Range,
        NonFinite,
    }

    /// The tri-state sign plus the refusal by name.
    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    pub enum Predicate {
        Negative,
        Zero,
        Positive,
        Uncertain(Unsure),
    }

    /// orient2d: positive when a, b, c turn counter-clockwise. SHIM: ONLY Shewchuk's stage-A filter
    /// (ccwerrboundA = (3 + 16e)e, e = 2^-53). When the filter cannot decide, it returns `Uncertain`, never a guessed
    /// sign. Zero is reported only when a product has an exactly-zero factor (equal coordinates). geom_core's exact
    /// expansion fallback replaces this at the fold.
    pub fn orient2d(a: Vec2, b: Vec2, c: Vec2) -> Predicate {
        let fin = |v: Vec2| v.x.is_finite() && v.y.is_finite();
        if !(fin(a) && fin(b) && fin(c)) {
            return Predicate::Uncertain(Unsure::NonFinite);
        }
        let (acx, bcx, acy, bcy) = (a.x - c.x, b.x - c.x, a.y - c.y, b.y - c.y);
        let detleft = acx * bcy;
        let detright = acy * bcx;
        let det = detleft - detright;
        let detsum = libm::fabs(detleft) + libm::fabs(detright);
        const EPS: f64 = 1.1102230246251565e-16; // 2^-53
        let errbound = (3.0 + 16.0 * EPS) * EPS * detsum;
        if det > errbound {
            Predicate::Positive
        } else if -det > errbound {
            Predicate::Negative
        } else if (acx == 0.0 || bcy == 0.0) && (acy == 0.0 || bcx == 0.0) {
            Predicate::Zero
        } else {
            Predicate::Uncertain(Unsure::Range)
        }
    }

    /// geom_core's 2D arc: radians, `sweep` signed (+ is counter-clockwise).
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Arc2 {
        pub centre: Vec2,
        pub radius: f64,
        pub start: f64,
        pub sweep: f64,
    }

    /// geom_core's plane: `normal` unit, the plane is `normal · p = d`.
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Plane {
        pub normal: Vec3,
        pub d: f64,
    }
    impl Plane {
        pub fn from_point_normal(p: Vec3, n: Vec3) -> Result<Plane, Degenerate> {
            let n = n.normalize()?;
            Ok(Plane { normal: n, d: n.dot(p) })
        }
    }

    /// geom_core's infinite line (`dir` unit).
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Line {
        pub point: Vec3,
        pub dir: Vec3,
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    pub enum PlanePlane {
        Line(Line),
        Parallel,
        Coincident,
    }

    pub fn plane_plane(a: &Plane, b: &Plane, tol: Tol) -> Result<PlanePlane, Degenerate> {
        let dir = a.normal.cross(b.normal);
        let s = dir.mag();
        if s <= tol.ang {
            let same = if a.normal.dot(b.normal) > 0.0 { a.d - b.d } else { a.d + b.d };
            return Ok(if libm::fabs(same) <= tol.lin { PlanePlane::Coincident } else { PlanePlane::Parallel });
        }
        let c = a.normal.dot(b.normal);
        let den = 1.0 - c * c;
        let c1 = (a.d - b.d * c) / den;
        let c2 = (b.d - a.d * c) / den;
        Ok(PlanePlane::Line(Line { point: a.normal * c1 + b.normal * c2, dir: dir / s }))
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct Circle2 {
        pub centre: Vec2,
        pub radius: f64,
    }

    #[derive(Clone, Copy, Debug, PartialEq)]
    pub enum CircleCircle {
        Two(Vec2, Vec2),
        Tangent(Vec2),
        Separate,
        Contained,
        Coincident,
    }

    pub fn circle_circle2(a: &Circle2, b: &Circle2, tol: Tol) -> Result<CircleCircle, Degenerate> {
        if a.radius <= 0.0 || b.radius <= 0.0 {
            return Err(Degenerate::ZeroRadius);
        }
        let dv = b.centre - a.centre;
        let d = dv.mag();
        let (r1, r2) = (a.radius, b.radius);
        if d <= tol.lin {
            return Ok(if libm::fabs(r1 - r2) <= tol.lin { CircleCircle::Coincident } else { CircleCircle::Contained });
        }
        let u = dv * (1.0 / d);
        if libm::fabs(d - (r1 + r2)) <= tol.lin {
            return Ok(CircleCircle::Tangent(a.centre + u * r1));
        }
        if libm::fabs(d - libm::fabs(r1 - r2)) <= tol.lin {
            let s = if r1 > r2 { r1 } else { -r1 };
            return Ok(CircleCircle::Tangent(a.centre + u * s));
        }
        if d > r1 + r2 {
            return Ok(CircleCircle::Separate);
        }
        if d < libm::fabs(r1 - r2) {
            return Ok(CircleCircle::Contained);
        }
        let x = (d * d + r1 * r1 - r2 * r2) / (2.0 * d);
        let h = libm::sqrt((r1 * r1 - x * x).max(0.0));
        let m = a.centre + u * x;
        let perp = Vec2::new(-u.y, u.x);
        Ok(CircleCircle::Two(m + perp * h, m - perp * h))
    }
}

// SHIM: sketch_core (SR465) not on its branch at build; the seat swaps this block for
// `pub use sketch_core::{Profile, Loop, Edge, EntityId};` + the Cargo dep. Only build.rs `loops_of` reads these.
mod sketch {
    use super::geom::{Arc2, Vec2};
    use alloc::vec::Vec;

    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
    pub struct EntityId(pub u32);

    /// One edge of a profile loop. A full circle is a one-edge loop: an Arc with |sweep| = 2π.
    #[derive(Clone, Debug, PartialEq)]
    pub enum Edge {
        Line { from: Vec2, to: Vec2, src: EntityId },
        Arc { arc: Arc2, src: EntityId },
    }

    /// A closed loop of edges. `area` is signed (+ counter-clockwise).
    #[derive(Clone, Debug, PartialEq)]
    pub struct Loop {
        pub edges: Vec<Edge>,
        pub area: f64,
    }

    /// A closed region: outer counter-clockwise, holes clockwise (material on the left).
    #[derive(Clone, Debug, PartialEq)]
    pub struct Profile {
        pub outer: Loop,
        pub holes: Vec<Loop>,
    }
}

// SHIM: mesh_core (SR466) not on its branch at build; the seat swaps this block for `pub use mesh_core::Mesh;` + the
// Cargo dep. The struct alone; manifold / chi checks are mesh_core's (`HalfEdge::build`), and the tests stand in a
// test-only edge-pair helper until the fold.
mod mesh {
    use super::geom::Vec3;
    use alloc::vec::Vec;

    /// mesh_core's indexed triangle mesh (f64 positions; the f32 conversion is mesh_core's `to_f32_buffers`).
    #[derive(Clone, Debug, PartialEq, Default)]
    pub struct Mesh {
        pub positions: Vec<Vec3>,
        pub triangles: Vec<[u32; 3]>,
    }
}
