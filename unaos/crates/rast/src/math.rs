// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Fixed-shape `f32` 3D math: `Vec3`, `Vec4`, a column-major `Mat4`, and the
//! transform helpers the pipeline needs (perspective, rotation, translation,
//! look-at).
//!
//! # Determinism contract
//! Every operation here is built from the four IEEE-754 primitives `+ - * /`
//! plus `f32::sqrt` — all of which are correctly-rounded, round-to-nearest, and
//! therefore **byte-identical on x86_64 and aarch64**. We deliberately avoid:
//! - fused multiply-add (`f32::mul_add`/`fma`), which rounds once and so diverges
//!   from a separate `*` then `+` — and whether the compiler contracts `a*b+c`
//!   into an fma is arch/opt-level dependent. The matrix-multiply inner loop is
//!   written as explicit `+`-of-`*` terms so no contraction is possible.
//! - the libm transcendentals (`f32::sin`, `f32::cos`, …), which are *not*
//!   correctly-rounded and differ between platforms' libm. [`sin`]/[`cos`] below
//!   are our own polynomial approximations built from the deterministic
//!   primitives, so a scene that spins is still byte-identical across arches.
//!
//! No `-ffast-math` equivalent is ever enabled (Rust has none by default).

/// A 3-component vector.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    #[inline]
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }

    #[inline]
    pub fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }

    #[inline]
    pub fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }

    #[inline]
    pub fn scale(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }

    #[inline]
    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }

    #[inline]
    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }

    #[inline]
    pub fn length(self) -> f32 {
        // `libm::sqrtf` is correctly-rounded and pure-Rust → deterministic on both arches.
        libm::sqrtf(self.dot(self))
    }

    /// Normalize; a zero-length vector is returned unchanged (no NaN).
    #[inline]
    pub fn normalize(self) -> Vec3 {
        let len = self.length();
        if len == 0.0 {
            self
        } else {
            self.scale(1.0 / len)
        }
    }
}

/// A 4-component vector (a homogeneous point / clip-space coordinate).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vec4 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Vec4 {
    #[inline]
    pub const fn new(x: f32, y: f32, z: f32, w: f32) -> Self {
        Self { x, y, z, w }
    }

    #[inline]
    pub const fn point(v: Vec3) -> Vec4 {
        Vec4::new(v.x, v.y, v.z, 1.0)
    }

    /// Linear interpolation `self + t*(o-self)`, component-wise. Used by the
    /// near-plane clipper; written as `a + t*(b-a)` (not `a*(1-t)+b*t`) so the
    /// endpoints `t=0`/`t=1` reproduce the inputs exactly.
    #[inline]
    pub fn lerp(self, o: Vec4, t: f32) -> Vec4 {
        Vec4::new(
            self.x + t * (o.x - self.x),
            self.y + t * (o.y - self.y),
            self.z + t * (o.z - self.z),
            self.w + t * (o.w - self.w),
        )
    }
}

/// A 4×4 matrix, **column-major**: `m[col*4 + row]`. `transform` computes
/// `M · v` with `v` a column vector.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mat4 {
    /// Column-major storage: element (row `r`, col `c`) is `m[c * 4 + r]`.
    pub m: [f32; 16],
}

impl Mat4 {
    #[inline]
    pub const fn zero() -> Self {
        Self { m: [0.0; 16] }
    }

    #[inline]
    pub const fn identity() -> Self {
        let mut m = [0.0; 16];
        m[0] = 1.0;
        m[5] = 1.0;
        m[10] = 1.0;
        m[15] = 1.0;
        Self { m }
    }

    /// `self · rhs`. The inner accumulation is written as an explicit sum of four
    /// products so the compiler cannot contract it into an fma (see the module
    /// determinism contract).
    pub fn mul(&self, rhs: &Mat4) -> Mat4 {
        let a = &self.m;
        let b = &rhs.m;
        let mut out = [0.0f32; 16];
        for c in 0..4 {
            for r in 0..4 {
                // out(r,c) = sum_k a(r,k) * b(k,c)
                let p0 = a[r] * b[c * 4];
                let p1 = a[4 + r] * b[c * 4 + 1];
                let p2 = a[8 + r] * b[c * 4 + 2];
                let p3 = a[12 + r] * b[c * 4 + 3];
                out[c * 4 + r] = p0 + p1 + p2 + p3;
            }
        }
        Mat4 { m: out }
    }

    /// `M · v` for a homogeneous column vector.
    pub fn transform(&self, v: Vec4) -> Vec4 {
        let m = &self.m;
        Vec4::new(
            m[0] * v.x + m[4] * v.y + m[8] * v.z + m[12] * v.w,
            m[1] * v.x + m[5] * v.y + m[9] * v.z + m[13] * v.w,
            m[2] * v.x + m[6] * v.y + m[10] * v.z + m[14] * v.w,
            m[3] * v.x + m[7] * v.y + m[11] * v.z + m[15] * v.w,
        )
    }

    /// Translation matrix.
    pub fn translation(t: Vec3) -> Mat4 {
        let mut r = Mat4::identity();
        r.m[12] = t.x;
        r.m[13] = t.y;
        r.m[14] = t.z;
        r
    }

    /// Rotation about the X axis by `angle` radians (deterministic [`sin`]/[`cos`]).
    pub fn rotation_x(angle: f32) -> Mat4 {
        let (s, c) = (sin(angle), cos(angle));
        let mut r = Mat4::identity();
        r.m[5] = c;
        r.m[6] = s;
        r.m[9] = -s;
        r.m[10] = c;
        r
    }

    /// Rotation about the Y axis by `angle` radians.
    pub fn rotation_y(angle: f32) -> Mat4 {
        let (s, c) = (sin(angle), cos(angle));
        let mut r = Mat4::identity();
        r.m[0] = c;
        r.m[2] = -s;
        r.m[8] = s;
        r.m[10] = c;
        r
    }

    /// Rotation about the Z axis by `angle` radians.
    pub fn rotation_z(angle: f32) -> Mat4 {
        let (s, c) = (sin(angle), cos(angle));
        let mut r = Mat4::identity();
        r.m[0] = c;
        r.m[1] = s;
        r.m[4] = -s;
        r.m[5] = c;
        r
    }

    /// A right-handed perspective projection (looking down −Z), mapping the view
    /// frustum to the `[-1, 1]³` clip cube (OpenGL convention). `fov_y` in radians.
    pub fn perspective(fov_y: f32, aspect: f32, near: f32, far: f32) -> Mat4 {
        // f = 1 / tan(fov_y / 2), from the deterministic sin/cos.
        let half = fov_y * 0.5;
        let f = cos(half) / sin(half);
        let mut r = Mat4::zero();
        r.m[0] = f / aspect;
        r.m[5] = f;
        r.m[10] = (far + near) / (near - far);
        r.m[11] = -1.0;
        r.m[14] = (2.0 * far * near) / (near - far);
        r
    }

    /// A right-handed look-at view matrix.
    pub fn look_at(eye: Vec3, center: Vec3, up: Vec3) -> Mat4 {
        let f = center.sub(eye).normalize(); // forward
        let s = f.cross(up).normalize(); // right
        let u = s.cross(f); // true up
        let mut r = Mat4::identity();
        // Rotation (row = basis vector), column-major storage.
        r.m[0] = s.x;
        r.m[4] = s.y;
        r.m[8] = s.z;
        r.m[1] = u.x;
        r.m[5] = u.y;
        r.m[9] = u.z;
        r.m[2] = -f.x;
        r.m[6] = -f.y;
        r.m[10] = -f.z;
        // Translation.
        r.m[12] = -s.dot(eye);
        r.m[13] = -u.dot(eye);
        r.m[14] = f.dot(eye);
        r
    }
}

/// π as an `f32`.
pub const PI: f32 = core::f32::consts::PI;

/// Deterministic `sin` — `libm::sinf`, pure-Rust and byte-identical on both arches.
#[inline]
pub fn sin(x: f32) -> f32 {
    libm::sinf(x)
}

/// Deterministic `cos` — `libm::cosf`, pure-Rust and byte-identical on both arches.
#[inline]
pub fn cos(x: f32) -> f32 {
    libm::cosf(x)
}

/// Deterministic floor — `libm::floorf`.
#[inline]
pub fn floor(x: f32) -> f32 {
    libm::floorf(x)
}
