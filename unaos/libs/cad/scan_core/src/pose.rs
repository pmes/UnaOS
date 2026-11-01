// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Vug (CODEX §2, 3D), seam shared-core — the pose of a depth frame estimated against the fused volume
//! (frame-to-model ICP), so a hand-held scan needs no given pose.
//!
//! VUGSCANPOSE (LEDGER SR478). Written from Newcombe et al., "KinectFusion: Real-Time Dense Surface Mapping and
//! Tracking" (ISMAR 2011) §3.4-3.5, PCL KinFu's defaults (distance 0.10 m, angle 20°, ICP iterations 10 / 5 / 4 fine
//! to coarse) and Low, "Linear Least-Squares Optimization for Point-to-Plane ICP Surface Registration" (2004):
//!
//! 1. [`raycast`] the TSDF from the model's pose: per pixel the ray through the pixel centre, clipped to the observed
//!    voxels' box, marched on the nearest voxel's distance; the first + → − crossing is refined by linear
//!    interpolation of trilinear samples and its normal is the trilinear cell's analytic gradient (toward free space).
//! 2. Projective data association: each new-frame point, carried by the current estimate, is projected into the
//!    MODEL's camera and paired with the surfel at that pixel when they are within [`TrackParams::dist_mm`] and their
//!    normals within [`TrackParams::normal_cos`].
//! 3. Point-to-plane: `r = n·(q − p)`, the left perturbation `q' = q + ω×q + τ` gives the row `[q × n, n]`; the 6x6
//!    normal equations are solved by Cholesky and the estimate moves by Rodrigues' exact rotation of `ω` then `τ`.
//! 4. Three levels, coarse to fine (pixel strides 4 / 2 / 1); a level stops when the step is inside the [`Tol`].
//! 5. Lost, by name, when too few pixels pair or the residual is too large; a refusal leaves the tracker as it was.
//!
//! Not reproduced: KinFu's depth-aware pyrDown and bilateral filter (the levels here are plain pixel strides).
//!
//! The IMU (probe_core::imu) gives gravity, a 2-DOF fact (roll and pitch about down), never yaw or translation.
//! Measured (SR478's design): as a soft term in the 6x6 at honest variances it carries ~1e-6 of the point data's
//! rotation information on the fixture, and on a scene that leaves the pose free (a bare floor) it constrains none
//! of the free directions. So it is a post-hoc CHECK: the first frame that carries gravity anchors the world's
//! down, and a tracked pose that disagrees with a later frame's gravity by more than [`TrackParams::tilt_rad`] is
//! refused as [`TrackError::GravityDisagrees`].
//!
//! Geometry: only names the shim and geom_core spell alike; a pose is built through `geom::from_axes` alone and a
//! pose's inverse is applied through its own axes (`Axes::local`), so the geom_core swap leaves this file as is.

use alloc::vec;
use alloc::vec::Vec;

use probe_core::depth::metres_at;
use probe_core::{DepthScale, Frame, Intrinsics, PixelFormat};

use crate::ScanError;
use crate::geom::{self, Tol, Transform, Vec3};
use crate::tsdf::Tsdf;

const DEG: f64 = core::f64::consts::PI / 180.0;

/// One raycast hit: the surface point and its unit normal (toward free space), world mm.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surfel {
    pub p: Vec3,
    pub n: Vec3,
}

/// The model as the camera at `pose` sees it: one optional surfel per pixel, row-major.
#[derive(Clone, Debug, PartialEq)]
pub struct ModelMap {
    pub width: u32,
    pub height: u32,
    pub intrinsics: Intrinsics,
    pub pose: Transform,
    pub surfels: Vec<Option<Surfel>>,
}

impl ModelMap {
    /// The surfel at pixel `(x, y)`.
    pub fn at(&self, x: u32, y: u32) -> Option<Surfel> {
        if x >= self.width || y >= self.height {
            return None;
        }
        self.surfels[(y as usize) * (self.width as usize) + x as usize]
    }

    /// The model depth at pixel `(x, y)`: the surfel's camera z, mm.
    pub fn depth_mm(&self, x: u32, y: u32) -> Option<f64> {
        let s = self.at(x, y)?;
        Some(Axes::of(&self.pose).local(s.p).z)
    }

    /// Pixels with a surfel.
    pub fn hits(&self) -> usize {
        self.surfels.iter().filter(|s| s.is_some()).count()
    }
}

/// The tracker's thresholds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackParams {
    /// A pair farther apart than this is not a pair (KinFu: 100 mm).
    pub dist_mm: f64,
    /// A pair whose normals' cosine is below this is not a pair (KinFu: cos 20°).
    pub normal_cos: f64,
    /// Lost when fewer of the sampled valid pixels pair.
    pub min_inlier_frac: f64,
    /// Lost when the rms point-to-plane residual is above this.
    pub max_rms_mm: f64,
    /// (pixel stride, iterations), coarse to fine (KinFu: 4 / 5 / 10 iterations).
    pub levels: [(u32, u32); 3],
    /// Refused when the tracked pose tilts the frame's gravity off the anchored down by more than this.
    pub tilt_rad: f64,
}

impl TrackParams {
    /// KinFu's association thresholds, a quarter of the pixels pairing, the volume's truncation band as the residual
    /// bound, and 5° of tilt (the hand-held accelerometer's linear-acceleration class).
    pub fn for_volume(t: &Tsdf) -> TrackParams {
        TrackParams {
            dist_mm: 100.0,
            normal_cos: libm::cos(20.0 * DEG),
            min_inlier_frac: 0.25,
            max_rms_mm: t.trunc_mm(),
            levels: [(4, 4), (2, 5), (1, 10)],
            tilt_rad: 5.0 * DEG,
        }
    }
}

/// What the last accepted track measured (the final iteration's pairing, before its step).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tracked {
    pub inliers: u32,
    pub valid: u32,
    pub rms_mm: f64,
    pub iterations: u32,
    /// The angle between the frame's gravity carried by the pose and the anchored down; `None` without gravity.
    pub tilt_deg: Option<f64>,
}

/// Why a frame was not tracked. Every refusal leaves the tracker as it was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TrackError {
    /// No model has been raycast yet (integrate a first frame, or `refresh`).
    NoModel,
    /// The frame does not pair with the model: `inliers` pairs, `residual_mm` rms (infinite when nothing paired).
    Lost { inliers: u32, residual_mm: f64 },
    /// The tracked pose contradicts the frame's gravity by `deg` degrees.
    GravityDisagrees { deg: f64 },
    /// The frame or the volume refused it (scan_core's own vocabulary).
    Scan(ScanError),
}

/// The tracker: the current pose, the model raycast at the last accepted pose, the gravity anchor. Caller-owned,
/// mutated through `&mut` only (BP-vug-4).
#[derive(Clone, Debug)]
pub struct Tracker {
    tol: Tol,
    params: TrackParams,
    pose: Transform,
    model: Option<ModelMap>,
    down: Option<Vec3>,
    last: Option<Tracked>,
}

impl Tracker {
    /// A tracker at the identity pose (the first frame is the world) for volume `t`; `tol` is the convergence
    /// tolerance of a step (BP-vug-2).
    pub fn new(t: &Tsdf, tol: Tol) -> Tracker {
        Tracker { tol, params: TrackParams::for_volume(t), pose: Transform::IDENTITY, model: None, down: None, last: None }
    }

    pub fn with_params(mut self, params: TrackParams) -> Tracker {
        self.params = params;
        self
    }

    /// The same tracker starting at `pose` (camera → world).
    pub fn with_pose(mut self, pose: Transform) -> Tracker {
        self.pose = pose;
        self
    }

    pub fn pose(&self) -> Transform {
        self.pose
    }

    pub fn params(&self) -> TrackParams {
        self.params
    }

    pub fn last(&self) -> Option<Tracked> {
        self.last
    }

    pub fn model(&self) -> Option<&ModelMap> {
        self.model.as_ref()
    }

    /// Raycast `t` from the current pose into the model.
    pub fn refresh(&mut self, t: &Tsdf, intr: &Intrinsics) {
        self.model = Some(raycast(t, intr, &self.pose));
    }

    /// The pose a frame was integrated at: commit it, anchor gravity if none is anchored, raycast the new model.
    pub(crate) fn accept(&mut self, t: &Tsdf, intr: &Intrinsics, pose: Transform, gravity: Option<Vec3>) {
        self.pose = pose;
        if self.down.is_none() {
            if let Some(g) = gravity.and_then(unit) {
                self.down = Some(pose.apply_vector(g));
            }
        }
        self.model = Some(raycast(t, intr, &pose));
    }

    /// Track `frame` against the model from the current pose. `gravity` is the direction of gravity (down) in the
    /// depth camera's axes (−accel at rest, after the IMU → depth extrinsic, the caller's); any length, zero ignored.
    /// On success the tracker moves to the returned pose (camera → world); the model stays the last raycast.
    pub fn track(
        &mut self,
        frame: &Frame,
        scale: DepthScale,
        intr: &Intrinsics,
        gravity: Option<Vec3>,
    ) -> Result<Transform, TrackError> {
        if frame.format != PixelFormat::Z16 {
            return Err(TrackError::Scan(ScanError::NotDepth { format: frame.format }));
        }
        if frame.width != intr.width || frame.height != intr.height {
            return Err(TrackError::Scan(ScanError::SizeMismatch {
                frame: (frame.width, frame.height),
                intrinsics: (intr.width, intr.height),
            }));
        }
        if intr.fx == 0.0 || intr.fy == 0.0 {
            return Err(TrackError::Scan(ScanError::NoFocal));
        }
        let Some(model) = self.model.as_ref() else { return Err(TrackError::NoModel) };
        if model.intrinsics != *intr {
            return Err(TrackError::Scan(ScanError::IntrinsicsChanged));
        }

        // The frame's camera points (mm), once.
        let (w, h) = (frame.width, frame.height);
        let mut pts: Vec<Option<Vec3>> = Vec::with_capacity(w as usize * h as usize);
        let mut any = false;
        for y in 0..h {
            for x in 0..w {
                let p = metres_at(frame, scale, x, y)
                    .and_then(|m| intr.deproject(x as f32 + 0.5, y as f32 + 0.5, m))
                    .map(|c| Vec3::new(c[0] as f64 * 1000.0, c[1] as f64 * 1000.0, c[2] as f64 * 1000.0));
                any |= p.is_some();
                pts.push(p);
            }
        }
        if !any {
            return Err(TrackError::Scan(ScanError::NoDepth));
        }

        let reference = Axes::of(&model.pose);
        let prm = self.params;
        let mut pose = self.pose;
        let (mut iterations, mut inliers, mut valid, mut rms) = (0u32, 0u32, 0u32, f64::INFINITY);
        for &(stride, iters) in prm.levels.iter() {
            let s = stride.max(1);
            for _ in 0..iters {
                let mut ata = [[0.0f64; 6]; 6];
                let mut atb = [0.0f64; 6];
                let (mut n_in, mut n_valid, mut sse) = (0u32, 0u32, 0.0f64);
                let mut y = 0;
                while y + s < h {
                    let mut x = 0;
                    while x + s < w {
                        let i = y as usize * w as usize + x as usize;
                        let (Some(p), Some(px), Some(py)) = (pts[i], pts[i + s as usize], pts[i + (s * w) as usize])
                        else {
                            x += s;
                            continue;
                        };
                        n_valid += 1;
                        x += s;
                        let Some(nc) = unit((px - p).cross(py - p)) else { continue };
                        let nc = if nc.dot(p) > 0.0 { -nc } else { nc };
                        let q = pose.apply_point(p);
                        let nq = pose.apply_vector(nc);
                        let Some(m) = lookup(model, intr, reference.local(q)) else { continue };
                        let diff = q - m.p;
                        if diff.mag() > prm.dist_mm || nq.dot(m.n) < prm.normal_cos {
                            continue;
                        }
                        let r = m.n.dot(diff);
                        let jw = q.cross(m.n);
                        let j = [jw.x, jw.y, jw.z, m.n.x, m.n.y, m.n.z];
                        for a in 0..6 {
                            for b in 0..6 {
                                ata[a][b] += j[a] * j[b];
                            }
                            atb[a] -= j[a] * r;
                        }
                        n_in += 1;
                        sse += r * r;
                    }
                    y += s;
                }
                iterations += 1;
                inliers = n_in;
                valid = n_valid;
                rms = if n_in > 0 { libm::sqrt(sse / n_in as f64) } else { f64::INFINITY };
                if n_in < 6 {
                    return Err(TrackError::Lost { inliers: n_in, residual_mm: rms });
                }
                let Some(xi) = solve6(ata, atb) else {
                    return Err(TrackError::Lost { inliers: n_in, residual_mm: rms });
                };
                let (dw, dt) = (Vec3::new(xi[0], xi[1], xi[2]), Vec3::new(xi[3], xi[4], xi[5]));
                pose = left_step(&pose, dw, dt);
                if dw.mag() <= self.tol.ang && dt.mag() <= self.tol.lin {
                    break;
                }
            }
        }
        if valid == 0 || (inliers as f64) < prm.min_inlier_frac * valid as f64 || !(rms <= prm.max_rms_mm) {
            return Err(TrackError::Lost { inliers, residual_mm: rms });
        }

        let g = gravity.and_then(unit);
        let mut tilt_deg = None;
        if let (Some(g), Some(down)) = (g, self.down) {
            let deg = libm::acos(pose.apply_vector(g).dot(down).clamp(-1.0, 1.0)) / DEG;
            if deg > prm.tilt_rad / DEG {
                return Err(TrackError::GravityDisagrees { deg });
            }
            tilt_deg = Some(deg);
        }

        self.pose = pose;
        self.last = Some(Tracked { inliers, valid, rms_mm: rms, iterations, tilt_deg });
        if self.down.is_none() {
            if let Some(g) = g {
                self.down = Some(pose.apply_vector(g));
            }
        }
        Ok(pose)
    }
}

/// Raycast `t` as the camera at `pose` (camera → world) with intrinsics `intr` sees it. An empty volume, or a
/// focal length of zero, gives a map with no surfel.
pub fn raycast(t: &Tsdf, intr: &Intrinsics, pose: &Transform) -> ModelMap {
    let (w, h) = (intr.width, intr.height);
    let mut surfels = vec![None; w as usize * h as usize];

    // The observed voxels' box (one pass), a voxel wider on each side.
    let (mut lo, mut hi, mut any) = ((i32::MAX, i32::MAX, i32::MAX), (i32::MIN, i32::MIN, i32::MIN), false);
    t.for_each_voxel(|g, v| {
        if v.weight > 0.0 {
            any = true;
            lo = (lo.0.min(g.0), lo.1.min(g.1), lo.2.min(g.2));
            hi = (hi.0.max(g.0), hi.1.max(g.1), hi.2.max(g.2));
        }
    });
    if any {
        let vm = t.voxel_mm();
        let bmin = Vec3::new((lo.0 - 1) as f64 * vm, (lo.1 - 1) as f64 * vm, (lo.2 - 1) as f64 * vm);
        let bmax = Vec3::new((hi.0 + 1) as f64 * vm, (hi.1 + 1) as f64 * vm, (hi.2 + 1) as f64 * vm);
        let o = pose.trans;
        for y in 0..h {
            for x in 0..w {
                let Some(c) = intr.deproject(x as f32 + 0.5, y as f32 + 0.5, 1.0) else { continue };
                // Camera z of the ray direction is 1: the ray parameter IS the camera depth (mm).
                let d = pose.apply_vector(Vec3::new(c[0] as f64, c[1] as f64, 1.0));
                let Some((s0, s1)) = slab(o, d, bmin, bmax) else { continue };
                surfels[y as usize * w as usize + x as usize] = march(t, o, d, s0.max(vm), s1);
            }
        }
    }
    ModelMap { width: w, height: h, intrinsics: *intr, pose: *pose, surfels }
}

/// A rigid pose's frame: its axes (columns) and origin, read through `apply_vector`, so its inverse is applied
/// without an inverse call (every pose here has scale 1).
#[derive(Clone, Copy, Debug)]
struct Axes {
    x: Vec3,
    y: Vec3,
    z: Vec3,
    o: Vec3,
}

impl Axes {
    fn of(t: &Transform) -> Axes {
        Axes {
            x: t.apply_vector(Vec3::new(1.0, 0.0, 0.0)),
            y: t.apply_vector(Vec3::new(0.0, 1.0, 0.0)),
            z: t.apply_vector(Vec3::new(0.0, 0.0, 1.0)),
            o: t.trans,
        }
    }

    /// World → this frame.
    fn local(&self, p: Vec3) -> Vec3 {
        let d = p - self.o;
        Vec3::new(self.x.dot(d), self.y.dot(d), self.z.dot(d))
    }
}

/// The unit vector, or `None` for a zero or non-finite one (geom_core's `normalize` refuses the same two).
fn unit(v: Vec3) -> Option<Vec3> {
    let m = v.mag();
    if m > 0.0 && m.is_finite() { Some(v * (1.0 / m)) } else { None }
}

/// Rodrigues: `v` rotated by the angle `|w|` about `w`.
fn rotate(w: Vec3, v: Vec3) -> Vec3 {
    let th = w.mag();
    if !(th > 0.0) {
        return v;
    }
    let k = w * (1.0 / th);
    let (c, s) = (libm::cos(th), libm::sin(th));
    v * c + k.cross(v) * s + k * (k.dot(v) * (1.0 - c))
}

/// `exp(ω, τ) ∘ t`: rotate the pose's axes and origin by `ω`, then translate by `τ`.
fn left_step(t: &Transform, w: Vec3, tau: Vec3) -> Transform {
    let a = Axes::of(t);
    geom::from_axes(rotate(w, a.x), rotate(w, a.y), rotate(w, a.z), rotate(w, a.o) + tau)
}

/// The surfel the model camera sees at camera-frame point `l`.
fn lookup(model: &ModelMap, intr: &Intrinsics, l: Vec3) -> Option<Surfel> {
    let uv = intr.project([(l.x / 1000.0) as f32, (l.y / 1000.0) as f32, (l.z / 1000.0) as f32])?;
    if !(uv[0] >= 0.0 && uv[1] >= 0.0) {
        return None;
    }
    model.at(uv[0] as u32, uv[1] as u32) // floor: pixel x covers u in [x, x + 1), as tsdf.rs integrates
}

/// The 6x6 symmetric positive-definite solve `a·x = b` by Cholesky (sketch_core's `linalg::cholesky_solve` loop
/// at a fixed size; that one is private to its crate). `None` on a pivot that is not positive: the pairs do not
/// constrain all six directions.
fn solve6(mut a: [[f64; 6]; 6], mut b: [f64; 6]) -> Option<[f64; 6]> {
    for j in 0..6 {
        let mut d = a[j][j];
        for k in 0..j {
            d -= a[j][k] * a[j][k];
        }
        if !(d > 0.0) || !d.is_finite() {
            return None;
        }
        let l = libm::sqrt(d);
        a[j][j] = l;
        for i in j + 1..6 {
            let mut s = a[i][j];
            for k in 0..j {
                s -= a[i][k] * a[j][k];
            }
            a[i][j] = s / l;
        }
    }
    for i in 0..6 {
        let mut s = b[i];
        for k in 0..i {
            s -= a[i][k] * b[k];
        }
        b[i] = s / a[i][i];
    }
    for i in (0..6).rev() {
        let mut s = b[i];
        for k in i + 1..6 {
            s -= a[k][i] * b[k];
        }
        b[i] = s / a[i][i];
    }
    if b.iter().all(|x| x.is_finite()) { Some(b) } else { None }
}

/// The ray `o + s·d` against the box: the entry and exit parameters (entry clamped at 0).
fn slab(o: Vec3, d: Vec3, lo: Vec3, hi: Vec3) -> Option<(f64, f64)> {
    let (mut s0, mut s1) = (0.0f64, f64::INFINITY);
    for (oa, da, l, h) in [(o.x, d.x, lo.x, hi.x), (o.y, d.y, lo.y, hi.y), (o.z, d.z, lo.z, hi.z)] {
        if da == 0.0 {
            if oa < l || oa > h {
                return None;
            }
            continue;
        }
        let (a, b) = ((l - oa) / da, (h - oa) / da);
        let (a, b) = if a < b { (a, b) } else { (b, a) };
        s0 = s0.max(a);
        s1 = s1.min(b);
    }
    if s0 <= s1 { Some((s0, s1)) } else { None }
}

/// The observed distance of voxel `g` (weight 0 = never observed = `None`).
fn sdf_at(t: &Tsdf, g: (i32, i32, i32)) -> Option<f64> {
    let v = t.voxel(g)?;
    if v.weight > 0.0 { Some(v.sdf as f64) } else { None }
}

/// Inside the index range the volume can address (`Tsdf::key_of`'s guard).
fn in_range(x: f64, y: f64, z: f64) -> bool {
    let lim = (i32::MAX / 2) as f64;
    x.abs() < lim && y.abs() < lim && z.abs() < lim
}

/// The nearest voxel's observed distance at world point `p`.
fn nearest(t: &Tsdf, p: Vec3) -> Option<f64> {
    let s = 1.0 / t.voxel_mm();
    let (x, y, z) = (libm::floor(p.x * s + 0.5), libm::floor(p.y * s + 0.5), libm::floor(p.z * s + 0.5));
    if !in_range(x, y, z) {
        return None;
    }
    sdf_at(t, (x as i32, y as i32, z as i32))
}

/// The trilinear distance at `p` and its analytic gradient (per mm); `None` unless all eight corners are observed.
fn trilinear(t: &Tsdf, p: Vec3) -> Option<(f64, Vec3)> {
    let vm = t.voxel_mm();
    let (fx, fy, fz) = (p.x / vm, p.y / vm, p.z / vm);
    let (x0, y0, z0) = (libm::floor(fx), libm::floor(fy), libm::floor(fz));
    if !in_range(x0, y0, z0) {
        return None;
    }
    let (u, v, s) = (fx - x0, fy - y0, fz - z0);
    let g = (x0 as i32, y0 as i32, z0 as i32);
    let mut c = [0.0f64; 8]; // c[dx + 2·dy + 4·dz]
    for (i, ci) in c.iter_mut().enumerate() {
        let (dx, dy, dz) = ((i & 1) as i32, ((i >> 1) & 1) as i32, ((i >> 2) & 1) as i32);
        *ci = sdf_at(t, (g.0 + dx, g.1 + dy, g.2 + dz))?;
    }
    let lerp = |a: f64, b: f64, k: f64| a + (b - a) * k;
    let c0 = lerp(lerp(c[0], c[1], u), lerp(c[2], c[3], u), v);
    let c1 = lerp(lerp(c[4], c[5], u), lerp(c[6], c[7], u), v);
    let value = lerp(c0, c1, s);
    let gx = lerp(lerp(c[1] - c[0], c[3] - c[2], v), lerp(c[5] - c[4], c[7] - c[6], v), s);
    let gy = lerp(lerp(c[2] - c[0], c[3] - c[1], u), lerp(c[6] - c[4], c[7] - c[5], u), s);
    let gz = c1 - c0;
    Some((value, Vec3::new(gx / vm, gy / vm, gz / vm)))
}

/// March the ray `o + s·d` over `[s0, s1]` to the first + → − crossing; a − → + crossing (seen from inside) ends
/// it with no hit.
fn march(t: &Tsdf, o: Vec3, d: Vec3, s0: f64, s1: f64) -> Option<Surfel> {
    let (vm, trunc) = (t.voxel_mm(), t.trunc_mm());
    let dn = d.mag();
    if !(dn > 0.0) {
        return None;
    }
    let mut s = s0;
    let mut prev: Option<(f64, f64)> = None;
    while s <= s1 {
        let Some(f) = nearest(t, o + d * s) else {
            prev = None;
            s += 0.8 * trunc / dn;
            continue;
        };
        if let Some((sp, fp)) = prev {
            if fp > 0.0 && f < 0.0 {
                return refine(t, o, d, (sp, fp), (s, f));
            }
            if fp < 0.0 && f > 0.0 {
                return None;
            }
        }
        prev = Some((s, f));
        let step = if f > 0.0 { (0.8 * f).max(0.5 * vm) } else { 0.5 * vm };
        s += step / dn;
    }
    None
}

/// The crossing between `a` (in front) and `b` (behind), by linear interpolation of trilinear samples (the
/// nearest-voxel values where a corner is unobserved), with the trilinear gradient as its normal.
fn refine(t: &Tsdf, o: Vec3, d: Vec3, a: (f64, f64), b: (f64, f64)) -> Option<Surfel> {
    let fa = trilinear(t, o + d * a.0).map(|x| x.0).unwrap_or(a.1);
    let fb = trilinear(t, o + d * b.0).map(|x| x.0).unwrap_or(b.1);
    let k = if fa != fb { (fa / (fa - fb)).clamp(0.0, 1.0) } else { 0.5 };
    let p = o + d * (a.0 + (b.0 - a.0) * k);
    let (_, g) = trilinear(t, p)?;
    Some(Surfel { p, n: unit(g)? })
}
