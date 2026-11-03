// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Primitive fitting with residuals: RANSAC (Fischler & Bolles, CACM 1981) over Schnabel, Wahl & Klein's minimal
//! sets ("Efficient RANSAC for Point-Cloud Shape Detection", Computer Graphics Forum 2007: a plane from three points,
//! a cylinder from two points with normals — axis `n1 × n2`, the axis point where the two normal lines meet in the
//! plane across the axis, the radius their mean distance; an inlier lies within the distance band AND its normal
//! within the angle band). The trial count adapts to the best inlier fraction `w` seen so far,
//! `T = ln(1 − 0.99) / ln(1 − w^k)` (Hartley & Zisserman, Multiple View Geometry §4.7), under a cap; the sampler is
//! a fixed-seed splitmix64 (geom_core's `linalg::SplitMix64`, this crate's seeds), so a fit is bit-reproducible; the
//! eigen-decompositions are geom_core's `linalg::jacobi_sym` (VUG-GEOM-LINALG SR520).
//!
//! The consensus set is refined: a plane by total least squares (the centroid and the covariance's smallest
//! eigenvector); a cylinder's axis as the smallest eigenvector of `Σ n nᵀ` over the inliers' normals (they lie
//! across the axis), then centre and radius by the algebraic circle fit (Kåsa, IEEE Trans. Instrum. Meas. 1976) of
//! the points projected on the plane across the axis. Residuals are reported over the refined inliers (BP-vug-5:
//! a fitted shape prints its residuals; it is a measurement, not design geometry).
//!
//! `band` is BP-vug-2's `Tol` used as the measurement band: `lin` the distance (mm), `ang` the normal deviation (rad).
//!
//! VUGSCANSHAPES (LEDGER SR497): [`detect`] is Schnabel's multi-shape loop beside the single-shape fns (which keep their
//! meaning): minimal sets sampled LOCALLY (three sorted cell grids at 2 %, 8 % and 32 % of the extent, the octree's
//! levels), planes / cylinders / spheres (two points with normals and a verifier) / cones (three tangent planes),
//! scored by the points within the distance band AND the angle band, a curved candidate whose normals turn less than
//! the angle band refused as a plane in disguise, the best candidate refit and cut to its largest connected component,
//! accepted only at or above BOTH limits of a [`Floor`], its points removed, repeated; a cloud of noise yields nothing.

use alloc::vec;
use alloc::vec::Vec;

use crate::geom::{Plane, Tol, Vec3};

/// The probability a run draws at least one all-inlier minimal set (the adaptive count's target).
pub const CONFIDENCE: f64 = 0.99;
/// The trial cap.
pub const MAX_TRIALS: u32 = 4096;
/// The sampler's fixed seed.
const SEED: u64 = 0x5CA4_C0DE_0469_0001;
/// Refinement passes (re-select inliers with the refined shape, refit).
const REFINE: u32 = 2;

/// A fitted shape and its residuals over the inliers.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit<T> {
    pub shape: T,
    pub rms_mm: f64,
    pub max_mm: f64,
    pub inliers: u32,
    pub outliers: u32,
}

impl<T> Fit<T> {
    pub fn inlier_fraction(&self) -> f64 {
        let n = self.inliers as f64 + self.outliers as f64;
        if n == 0.0 { 0.0 } else { self.inliers as f64 / n }
    }
}

/// A fitted cylinder: a point on the axis, the unit axis, the radius (mm). A measurement; a brep_core surface made
/// from it is a separate step that prints these residuals.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cylinder {
    pub point: Vec3,
    pub axis: Vec3,
    pub radius: f64,
}

/// Why a fit was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FitError {
    TooFewPoints { have: usize, need: usize },
    NormalsMismatch { points: usize, normals: usize },
    /// No candidate gathered more inliers than its minimal set.
    NoConsensus,
}

/// splitmix64: geom_core's one (VUG-GEOM-LINALG SR520), the deterministic sampler (also the fixture's noise source).
pub(crate) use geom_core::linalg::SplitMix64 as SplitMix;

fn trials_for(inliers: usize, n: usize, k: i32) -> u32 {
    let w = inliers as f64 / n as f64;
    let wk = libm::pow(w, k as f64);
    if wk >= 1.0 {
        return 1;
    }
    if wk <= 0.0 {
        return MAX_TRIALS;
    }
    let t = libm::ceil(libm::log(1.0 - CONFIDENCE) / libm::log(1.0 - wk));
    if t.is_finite() && t < MAX_TRIALS as f64 { (t as u32).max(1) } else { MAX_TRIALS }
}

/// A symmetric 3×3 row-major, as geom_core's `linalg::jacobi_sym` reads it.
fn flat(m: [[f64; 3]; 3]) -> [f64; 9] {
    [m[0][0], m[0][1], m[0][2], m[1][0], m[1][1], m[1][2], m[2][0], m[2][1], m[2][2]]
}

/// The unit eigenvector of the smallest eigenvalue.
fn smallest_eigvec(m: [[f64; 3]; 3]) -> Vec3 {
    // Ascending eigenpairs: column 0 is the smallest (the first strict minimum).
    let (_, v) = geom_core::linalg::jacobi_sym(&flat(m), 3);
    Vec3::new(v[0], v[3], v[6]).normalize().unwrap_or(Vec3::ZERO)
}

/// Sign convention for a direction: its largest-magnitude component is positive (a deterministic orientation).
fn canonical(d: Vec3) -> Vec3 {
    let (ax, ay, az) = (d.x.abs(), d.y.abs(), d.z.abs());
    let c = if ax >= ay && ax >= az { d.x } else if ay >= az { d.y } else { d.z };
    if c < 0.0 { -d } else { d }
}

fn centroid(points: &[Vec3], idx: &[usize]) -> Vec3 {
    let mut c = Vec3::ZERO;
    for &i in idx {
        c = c + points[i];
    }
    c * (1.0 / idx.len() as f64)
}

fn outer_sum(vs: impl Iterator<Item = Vec3>) -> [[f64; 3]; 3] {
    let mut m = [[0.0; 3]; 3];
    for d in vs {
        let a = [d.x, d.y, d.z];
        for r in 0..3 {
            for c in 0..3 {
                m[r][c] += a[r] * a[c];
            }
        }
    }
    m
}

fn residuals(res: impl Iterator<Item = f64>) -> (f64, f64, u32) {
    let (mut ss, mut mx, mut n) = (0.0, 0.0f64, 0u32);
    for r in res {
        ss += r * r;
        mx = mx.max(r.abs());
        n += 1;
    }
    (if n == 0 { 0.0 } else { libm::sqrt(ss / n as f64) }, mx, n)
}

// ---- plane ----

fn plane_inliers(points: &[Vec3], pl: &Plane, band: Tol) -> Vec<usize> {
    (0..points.len()).filter(|&i| pl.signed_distance(points[i]).abs() <= band.lin).collect()
}

fn plane_tls(points: &[Vec3], idx: &[usize]) -> Plane {
    let c = centroid(points, idx);
    let n = canonical(smallest_eigvec(outer_sum(idx.iter().map(|&i| points[i] - c))));
    Plane { normal: n, d: n.dot(c) }
}

/// The plane through the most points within `band.lin`, refined by total least squares, with its residuals.
pub fn fit_plane(points: &[Vec3], band: Tol) -> Result<Fit<Plane>, FitError> {
    let n = points.len();
    if n < 3 {
        return Err(FitError::TooFewPoints { have: n, need: 3 });
    }
    let mut rng = SplitMix::new(SEED);
    let (mut best, mut best_count) = (None::<Plane>, 0usize);
    let (mut trial, mut budget) = (0u32, MAX_TRIALS);
    while trial < budget {
        trial += 1;
        let (i, j, k) = (rng.below(n), rng.below(n), rng.below(n));
        if i == j || j == k || i == k {
            continue;
        }
        let (a, b, c) = (points[i], points[j], points[k]);
        let nrm = (b - a).cross(c - a);
        let longest = (b - a).mag().max((c - a).mag()).max((c - b).mag());
        if !(nrm.mag() > band.lin * longest) {
            continue; // collinear within the band: no plane
        }
        let u = nrm.normalize().unwrap_or(Vec3::ZERO);
        let pl = Plane { normal: u, d: u.dot(a) };
        let count = (0..n).filter(|&q| pl.signed_distance(points[q]).abs() <= band.lin).count();
        if count > best_count {
            best_count = count;
            best = Some(pl);
            budget = budget.min(trials_for(count, n, 3).max(trial));
        }
    }
    let Some(mut pl) = best else { return Err(FitError::NoConsensus) };
    let mut idx = plane_inliers(points, &pl, band);
    for _ in 0..REFINE {
        if idx.len() <= 3 {
            return Err(FitError::NoConsensus);
        }
        pl = plane_tls(points, &idx);
        idx = plane_inliers(points, &pl, band);
    }
    if idx.len() <= 3 {
        return Err(FitError::NoConsensus);
    }
    let (rms, mx, cnt) = residuals(idx.iter().map(|&i| pl.signed_distance(points[i])));
    Ok(Fit { shape: pl, rms_mm: rms, max_mm: mx, inliers: cnt, outliers: (n - idx.len()) as u32 })
}

// ---- cylinder ----

/// Radial vector of `p` from the axis (`axis` unit).
fn radial(cy: &Cylinder, p: Vec3) -> Vec3 {
    let d = p - cy.point;
    d - cy.axis * cy.axis.dot(d)
}

fn cyl_inlier(cy: &Cylinder, p: Vec3, n: Vec3, band: Tol) -> Option<f64> {
    let r = radial(cy, p);
    let dist = r.mag();
    if !(dist > 0.0) {
        return None;
    }
    let res = dist - cy.radius;
    if res.abs() > band.lin {
        return None;
    }
    if n.dot(r).abs() / dist < libm::cos(band.ang) {
        return None;
    }
    Some(res)
}

fn cyl_inliers(points: &[Vec3], normals: &[Vec3], cy: &Cylinder, band: Tol) -> Vec<usize> {
    (0..points.len()).filter(|&i| cyl_inlier(cy, points[i], normals[i], band).is_some()).collect()
}

/// Schnabel's two-point candidate.
fn cyl_candidate(p1: Vec3, n1: Vec3, p2: Vec3, n2: Vec3, band: Tol) -> Option<Cylinder> {
    let a = n1.cross(n2);
    if !(a.mag() > libm::sin(band.ang)) {
        return None; // parallel normals: no axis
    }
    let a = a.normalize().unwrap_or(Vec3::ZERO);
    let (q1, q2) = (p1 - a * a.dot(p1), p2 - a * a.dot(p2));
    let b = n1.dot(n2);
    let w = q1 - q2;
    let (d, e) = (n1.dot(w), n2.dot(w));
    let den = 1.0 - b * b;
    if !(den > 0.0) {
        return None;
    }
    let s = (b * e - d) / den;
    let t = (e - b * d) / den;
    let c = ((q1 + n1 * s) + (q2 + n2 * t)) * 0.5;
    let radius = ((q1 - c).mag() + (q2 - c).mag()) * 0.5;
    if !(radius.is_finite() && radius > band.lin && c.x.is_finite() && c.y.is_finite() && c.z.is_finite()) {
        return None;
    }
    Some(Cylinder { point: c, axis: a, radius })
}

/// Solve the 3×3 system by Cramer's rule; `None` when singular.
fn solve3(m: [[f64; 3]; 3], r: [f64; 3]) -> Option<[f64; 3]> {
    let det = |m: &[[f64; 3]; 3]| {
        m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
            + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
    };
    let d = det(&m);
    if !(d.abs() > 0.0) || !d.is_finite() {
        return None;
    }
    let mut out = [0.0; 3];
    for (col, o) in out.iter_mut().enumerate() {
        let mut mm = m;
        for row in 0..3 {
            mm[row][col] = r[row];
        }
        *o = det(&mm) / d;
    }
    Some(out)
}

fn cyl_refine(points: &[Vec3], normals: &[Vec3], idx: &[usize], prev: &Cylinder) -> Cylinder {
    let mut axis = canonical(smallest_eigvec(outer_sum(idx.iter().map(|&i| normals[i].normalize().unwrap_or(Vec3::ZERO)))));
    if axis.mag() == 0.0 {
        axis = prev.axis;
    }
    // An orthonormal basis (u, w) across the axis.
    let helper = if axis.x.abs() < 0.9 { Vec3::new(1.0, 0.0, 0.0) } else { Vec3::new(0.0, 1.0, 0.0) };
    let u = axis.cross(helper).normalize().unwrap_or(Vec3::ZERO);
    let w = axis.cross(u);
    let m = centroid(points, idx);
    let (mut sxx, mut sxy, mut syy, mut sx, mut sy, mut sxz, mut syz, mut sz) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
    for &i in idx {
        let d = points[i] - m;
        let (x, y) = (d.dot(u), d.dot(w));
        let z = x * x + y * y;
        sxx += x * x;
        sxy += x * y;
        syy += y * y;
        sx += x;
        sy += y;
        sxz += x * z;
        syz += y * z;
        sz += z;
    }
    let nn = idx.len() as f64;
    let Some([dd, ee, ff]) = solve3([[sxx, sxy, sx], [sxy, syy, sy], [sx, sy, nn]], [-sxz, -syz, -sz]) else {
        return Cylinder { axis, ..*prev };
    };
    let (cx, cy) = (-dd * 0.5, -ee * 0.5);
    let r2 = cx * cx + cy * cy - ff;
    if !(r2 > 0.0) {
        return Cylinder { axis, ..*prev };
    }
    Cylinder { point: m + u * cx + w * cy, axis, radius: libm::sqrt(r2) }
}

/// The cylinder through the most points within `band` (distance `lin`, normal `ang`), refined, with its residuals.
pub fn fit_cylinder(points: &[Vec3], normals: &[Vec3], band: Tol) -> Result<Fit<Cylinder>, FitError> {
    let n = points.len();
    if normals.len() != n {
        return Err(FitError::NormalsMismatch { points: n, normals: normals.len() });
    }
    if n < 2 {
        return Err(FitError::TooFewPoints { have: n, need: 2 });
    }
    let mut rng = SplitMix::new(SEED);
    let (mut best, mut best_count) = (None::<Cylinder>, 0usize);
    let (mut trial, mut budget) = (0u32, MAX_TRIALS);
    while trial < budget {
        trial += 1;
        let (i, j) = (rng.below(n), rng.below(n));
        if i == j {
            continue;
        }
        let (n1, n2) = (normals[i].normalize().unwrap_or(Vec3::ZERO), normals[j].normalize().unwrap_or(Vec3::ZERO));
        let Some(cy) = cyl_candidate(points[i], n1, points[j], n2, band) else { continue };
        let count = (0..n).filter(|&q| cyl_inlier(&cy, points[q], normals[q].normalize().unwrap_or(Vec3::ZERO), band).is_some()).count();
        if count > best_count {
            best_count = count;
            best = Some(cy);
            budget = budget.min(trials_for(count, n, 2).max(trial));
        }
    }
    let Some(mut cy) = best else { return Err(FitError::NoConsensus) };
    let unit: Vec<Vec3> = normals.iter().map(|v| v.normalize().unwrap_or(Vec3::ZERO)).collect();
    let mut idx = cyl_inliers(points, &unit, &cy, band);
    for _ in 0..REFINE {
        if idx.len() < 3 {
            return Err(FitError::NoConsensus);
        }
        cy = cyl_refine(points, &unit, &idx, &cy);
        idx = cyl_inliers(points, &unit, &cy, band);
    }
    if idx.len() < 3 {
        return Err(FitError::NoConsensus);
    }
    let (rms, mx, cnt) = residuals(idx.iter().filter_map(|&i| cyl_inlier(&cy, points[i], unit[i], band)));
    Ok(Fit { shape: cy, rms_mm: rms, max_mm: mx, inliers: cnt, outliers: (n - idx.len()) as u32 })
}

// ---- sphere (VUGSCANSHAPES SR497) ----

/// A fitted sphere: the centre and the radius (mm). A measurement (BP-vug-5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sphere {
    pub centre: Vec3,
    pub radius: f64,
}

fn sphere_inlier(s: &Sphere, p: Vec3, n: Vec3, band: Tol) -> Option<f64> {
    let d = p - s.centre;
    let dist = d.mag();
    if !(dist > 0.0) {
        return None;
    }
    let res = dist - s.radius;
    if res.abs() > band.lin {
        return None;
    }
    if n.dot(d).abs() / dist < libm::cos(band.ang) {
        return None;
    }
    Some(res)
}

/// The midpoint of the closest approach of the lines `p1 + s n1` and `p2 + t n2` (unit normals); `None` when they are
/// parallel within the angle band (the same closed form `cyl_candidate` solves across the axis).
fn normal_lines_meet(p1: Vec3, n1: Vec3, p2: Vec3, n2: Vec3, band: Tol) -> Option<Vec3> {
    if !(n1.cross(n2).mag() > libm::sin(band.ang)) {
        return None;
    }
    let b = n1.dot(n2);
    let w = p1 - p2;
    let (d, e) = (n1.dot(w), n2.dot(w));
    let den = 1.0 - b * b;
    if !(den > 0.0) {
        return None;
    }
    let s = (b * e - d) / den;
    let t = (e - b * d) / den;
    let c = ((p1 + n1 * s) + (p2 + n2 * t)) * 0.5;
    if c.x.is_finite() && c.y.is_finite() && c.z.is_finite() { Some(c) } else { None }
}

/// Schnabel's sphere: the centre where the two normal lines meet, the radius the mean distance; the third sample
/// verifies (all three within the band, distance and normal).
fn sphere_candidate(s: &[(Vec3, Vec3); 3], band: Tol) -> Option<Sphere> {
    let c = normal_lines_meet(s[0].0, s[0].1, s[1].0, s[1].1, band)?;
    let radius = ((s[0].0 - c).mag() + (s[1].0 - c).mag()) * 0.5;
    if !(radius.is_finite() && radius > band.lin) {
        return None;
    }
    let sp = Sphere { centre: c, radius };
    if s.iter().all(|&(p, n)| sphere_inlier(&sp, p, n, band).is_some()) { Some(sp) } else { None }
}

/// The centre by least squares of the inliers' normal lines, `Σ(I − nnᵀ) c = Σ(I − nnᵀ) p`, the radius the mean
/// distance; `prev` when singular.
fn sphere_refine(points: &[Vec3], unit: &[Vec3], idx: &[usize], prev: &Sphere) -> Sphere {
    let mut m = [[0.0; 3]; 3];
    let mut r = [0.0; 3];
    for &i in idx {
        let (n, p) = (unit[i], points[i]);
        let a = [n.x, n.y, n.z];
        let q = p - n * n.dot(p);
        let qa = [q.x, q.y, q.z];
        for row in 0..3 {
            for col in 0..3 {
                let id = if row == col { 1.0 } else { 0.0 };
                m[row][col] += id - a[row] * a[col];
            }
            r[row] += qa[row];
        }
    }
    let Some([x, y, z]) = solve3(m, r) else { return *prev };
    let c = Vec3::new(x, y, z);
    let mut sum = 0.0;
    for &i in idx {
        sum += (points[i] - c).mag();
    }
    let radius = sum / idx.len() as f64;
    if radius.is_finite() && radius > 0.0 { Sphere { centre: c, radius } } else { *prev }
}

// ---- cone (VUGSCANSHAPES SR497) ----

/// A fitted cone: the apex, the unit axis pointing into the opening, the half-angle (rad). A measurement (BP-vug-5).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cone {
    pub apex: Vec3,
    pub axis: Vec3,
    pub half_angle: f64,
}

/// The signed distance to the nappe (perpendicular to the generator) when `p` lies over it and its normal agrees with
/// the cone's within the band; a point over the apex's own cap (nearest the apex) is no inlier.
fn cone_inlier(c: &Cone, p: Vec3, n: Vec3, band: Tol) -> Option<f64> {
    let v = p - c.apex;
    let h = v.dot(c.axis);
    let radial = v - c.axis * h;
    let rho = radial.mag();
    if !(rho > 0.0) {
        return None;
    }
    let (sa, ca) = (libm::sin(c.half_angle), libm::cos(c.half_angle));
    if h * ca + rho * sa < 0.0 {
        return None;
    }
    let res = rho * ca - h * sa;
    if res.abs() > band.lin {
        return None;
    }
    let nc = radial * (ca / rho) - c.axis * sa;
    if n.dot(nc).abs() < libm::cos(band.ang) {
        return None;
    }
    Some(res)
}

/// A cone narrower than the angle band is a cylinder, one flatter than it a plane: neither is a cone.
fn cone_admissible(c: &Cone, band: Tol) -> bool {
    c.half_angle.is_finite() && c.half_angle >= band.ang && c.half_angle <= core::f64::consts::FRAC_PI_2 - band.ang
}

/// The half-angle: the mean angle between the axis and the directions from the apex to the points.
fn cone_angle(apex: Vec3, axis: Vec3, ps: impl Iterator<Item = Vec3>) -> f64 {
    let (mut sum, mut k) = (0.0, 0u32);
    for p in ps {
        if let Ok(d) = (p - apex).normalize() {
            sum += libm::acos(axis.dot(d).clamp(-1.0, 1.0));
            k += 1;
        }
    }
    if k == 0 { f64::NAN } else { sum / k as f64 }
}

/// Schnabel's cone: the apex where the three tangent planes meet, the axis the normal of the plane through the apex
/// plus the three unit directions to the points (oriented into the opening), the half-angle their mean angle; the
/// three samples verify.
fn cone_candidate(s: &[(Vec3, Vec3); 3], band: Tol) -> Option<Cone> {
    let m = [[s[0].1.x, s[0].1.y, s[0].1.z], [s[1].1.x, s[1].1.y, s[1].1.z], [s[2].1.x, s[2].1.y, s[2].1.z]];
    let [x, y, z] = solve3(m, [s[0].1.dot(s[0].0), s[1].1.dot(s[1].0), s[2].1.dot(s[2].0)])?;
    let apex = Vec3::new(x, y, z);
    let d1 = (s[0].0 - apex).normalize().ok()?;
    let d2 = (s[1].0 - apex).normalize().ok()?;
    let d3 = (s[2].0 - apex).normalize().ok()?;
    let mut axis = (d2 - d1).cross(d3 - d1).normalize().ok()?;
    if axis.dot(d1 + d2 + d3) < 0.0 {
        axis = -axis;
    }
    let c = Cone { apex, axis, half_angle: cone_angle(apex, axis, s.iter().map(|q| q.0)) };
    if !cone_admissible(&c, band) {
        return None;
    }
    if s.iter().all(|&(p, n)| cone_inlier(&c, p, n, band).is_some()) { Some(c) } else { None }
}

/// The apex by least squares of the inliers' tangent planes (`Σ nnᵀ x = Σ n (n·p)`: every tangent plane of a cone
/// passes its apex), the axis the smallest eigenvector of the centred normal covariance (`n·a` is constant on a cone),
/// oriented into the opening, the half-angle the mean angle; `prev`'s part where a step is singular.
fn cone_refine(points: &[Vec3], unit: &[Vec3], idx: &[usize], prev: &Cone) -> Cone {
    let m = outer_sum(idx.iter().map(|&i| unit[i]));
    let mut r = [0.0; 3];
    for &i in idx {
        let (n, k) = (unit[i], unit[i].dot(points[i]));
        r[0] += n.x * k;
        r[1] += n.y * k;
        r[2] += n.z * k;
    }
    let apex = match solve3(m, r) {
        Some([x, y, z]) => Vec3::new(x, y, z),
        None => prev.apex,
    };
    let nbar = centroid(unit, idx);
    let mut axis = smallest_eigvec(outer_sum(idx.iter().map(|&i| unit[i] - nbar)));
    if axis.mag() == 0.0 {
        axis = prev.axis;
    }
    if axis.dot(centroid(points, idx) - apex) < 0.0 {
        axis = -axis;
    }
    let half_angle = cone_angle(apex, axis, idx.iter().map(|&i| points[i]));
    if half_angle.is_finite() { Cone { apex, axis, half_angle } } else { *prev }
}

// ---- detect (VUGSCANSHAPES SR497) ----

/// The grid's finest cell as a fraction of the cloud's extent (Schnabel's octree cell); the coarser levels are 4x and
/// 16x it.
pub const CELL_FRACTION: f64 = 0.02;
/// Trials (one local three-point sample each, up to four candidates) per extraction round.
pub const TRIALS_PER_ROUND: u32 = 256;
/// The live points a candidate's support is estimated on (Schnabel's subset score).
pub const SUBSET: usize = 2048;
/// The candidates settled (refit, component, floor) per round, best estimate first.
const TRIED: usize = 8;
/// The most shapes one detect returns.
pub const MAX_SHAPES: usize = 32;
/// No shape is accepted on fewer points than this, whatever the floor says (a minimal set plus one).
const MIN_SUPPORT: usize = 4;
const DETECT_SEED: u64 = 0x5CA4_5A4E_0497_0001;
/// 21 bits of cell index per axis in a packed key.
const KEY_BITS: u32 = 21;
const KEY_MASK: u64 = (1 << KEY_BITS) - 1;

/// A detected primitive with its parameters.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Primitive {
    Plane(Plane),
    Cylinder(Cylinder),
    Sphere(Sphere),
    Cone(Cone),
}

impl Primitive {
    pub fn name(&self) -> &'static str {
        match self {
            Primitive::Plane(_) => "plane",
            Primitive::Cylinder(_) => "cylinder",
            Primitive::Sphere(_) => "sphere",
            Primitive::Cone(_) => "cone",
        }
    }

    /// The residual of `p` (unit normal `n`) when it lies within the band, distance AND normal (Schnabel's score).
    fn residual(&self, p: Vec3, n: Vec3, band: Tol) -> Option<f64> {
        match self {
            Primitive::Plane(pl) => {
                let r = pl.signed_distance(p);
                if r.abs() > band.lin || n.dot(pl.normal).abs() < libm::cos(band.ang) { None } else { Some(r) }
            }
            Primitive::Cylinder(cy) => cyl_inlier(cy, p, n, band),
            Primitive::Sphere(s) => sphere_inlier(s, p, n, band),
            Primitive::Cone(c) => cone_inlier(c, p, n, band),
        }
    }

    fn curved(&self) -> bool {
        !matches!(self, Primitive::Plane(_))
    }

    fn admissible(&self, band: Tol) -> bool {
        match self {
            Primitive::Plane(pl) => pl.normal.mag() > 0.0 && pl.d.is_finite(),
            Primitive::Cylinder(cy) => cy.radius.is_finite() && cy.radius > band.lin && cy.axis.mag() > 0.0,
            Primitive::Sphere(s) => s.radius.is_finite() && s.radius > band.lin,
            Primitive::Cone(c) => cone_admissible(c, band),
        }
    }

    /// The existing refits: `plane_tls`, `cyl_refine`, and the sphere / cone ones above.
    fn refit(&self, points: &[Vec3], unit: &[Vec3], idx: &[usize]) -> Primitive {
        match self {
            Primitive::Plane(_) => Primitive::Plane(plane_tls(points, idx)),
            Primitive::Cylinder(cy) => Primitive::Cylinder(cyl_refine(points, unit, idx, cy)),
            Primitive::Sphere(s) => Primitive::Sphere(sphere_refine(points, unit, idx, s)),
            Primitive::Cone(c) => Primitive::Cone(cone_refine(points, unit, idx, c)),
        }
    }
}

/// One detected shape: the primitive, the input indices it explains, and its confidence in the SR469 shape (the
/// fraction of the input it explains and its residuals against the band).
#[derive(Clone, Debug, PartialEq)]
pub struct Shape {
    pub kind: Primitive,
    pub inliers: Vec<u32>,
    pub support: u32,
    /// `support` over the input's point count.
    pub fraction: f64,
    pub rms_mm: f64,
    pub max_mm: f64,
}

impl Shape {
    /// The rms against the band's distance (0 exact, 1 at the band's edge).
    pub fn rms_band(&self, band: Tol) -> f64 {
        if band.lin > 0.0 { self.rms_mm / band.lin } else { f64::INFINITY }
    }
}

/// The minimum support: a shape is accepted only with at least `points` inliers AND at least `fraction` of the points
/// still unexplained when it is found; below either it is refused.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Floor {
    pub points: u32,
    pub fraction: f64,
}

impl Floor {
    pub const DEFAULT: Floor = Floor { points: 100, fraction: 0.01 };

    fn admits(&self, support: usize, remaining: usize) -> bool {
        support >= (self.points as usize).max(MIN_SUPPORT) && support as f64 >= self.fraction * remaining as f64
    }
}

/// A sorted cell grid: (packed cell key, point index), cells of `cell` mm from `origin`.
struct Grid {
    origin: Vec3,
    cell: f64,
    keys: Vec<(u64, u32)>,
}

impl Grid {
    fn coords(&self, p: Vec3) -> [u64; 3] {
        let f = |v: f64, o: f64| (((v - o) / self.cell) as i64).clamp(0, KEY_MASK as i64) as u64;
        [f(p.x, self.origin.x), f(p.y, self.origin.y), f(p.z, self.origin.z)]
    }

    fn pack(c: [u64; 3]) -> u64 {
        c[0] | (c[1] << KEY_BITS) | (c[2] << (2 * KEY_BITS))
    }

    fn key(&self, p: Vec3) -> u64 {
        Grid::pack(self.coords(p))
    }

    fn build(points: &[Vec3], origin: Vec3, cell: f64) -> Grid {
        let mut g = Grid { origin, cell, keys: Vec::with_capacity(points.len()) };
        for (i, &p) in points.iter().enumerate() {
            let k = g.key(p);
            g.keys.push((k, i as u32));
        }
        g.keys.sort_unstable();
        g
    }

    fn members(&self, key: u64) -> &[(u64, u32)] {
        let lo = self.keys.partition_point(|e| e.0 < key);
        let hi = self.keys.partition_point(|e| e.0 <= key);
        &self.keys[lo..hi]
    }
}

fn uf_find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

/// The largest 26-connected component of `idx` over the finest grid's cells (two pipes on one axis are two
/// cylinders); ties go to the component holding the lowest index. Returned in ascending index order.
fn largest_component(points: &[Vec3], idx: &[usize], grid: &Grid) -> Vec<usize> {
    let mut cells: Vec<(u64, usize)> = idx.iter().map(|&i| (grid.key(points[i]), i)).collect();
    cells.sort_unstable();
    let mut uniq: Vec<u64> = cells.iter().map(|c| c.0).collect();
    uniq.dedup();
    let mut parent: Vec<usize> = (0..uniq.len()).collect();
    for u in 0..uniq.len() {
        let k = uniq[u];
        let c = [k & KEY_MASK, (k >> KEY_BITS) & KEY_MASK, k >> (2 * KEY_BITS)];
        for dx in -1i64..=1 {
            for dy in -1i64..=1 {
                for dz in -1i64..=1 {
                    let nc = [c[0] as i64 + dx, c[1] as i64 + dy, c[2] as i64 + dz];
                    if nc.iter().any(|&v| v < 0 || v > KEY_MASK as i64) {
                        continue;
                    }
                    let nk = Grid::pack([nc[0] as u64, nc[1] as u64, nc[2] as u64]);
                    if let Ok(v) = uniq.binary_search(&nk) {
                        let (a, b) = (uf_find(&mut parent, u), uf_find(&mut parent, v));
                        if a != b {
                            parent[a.max(b)] = a.min(b);
                        }
                    }
                }
            }
        }
    }
    let mut size = vec![0usize; uniq.len()];
    let mut low = vec![usize::MAX; uniq.len()];
    let mut root_of = Vec::with_capacity(cells.len());
    for &(k, i) in &cells {
        let u = uniq.binary_search(&k).unwrap_or(0);
        let r = uf_find(&mut parent, u);
        size[r] += 1;
        low[r] = low[r].min(i);
        root_of.push(r);
    }
    let mut best = 0usize;
    for r in 0..uniq.len() {
        if size[r] > size[best] || (size[r] == size[best] && low[r] < low[best]) {
            best = r;
        }
    }
    let mut out: Vec<usize> = cells.iter().zip(&root_of).filter(|(_, r)| **r == best).map(|(c, _)| c.1).collect();
    out.sort_unstable();
    out
}

/// How far the normals turn: sqrt(λ_mid / Σλ) of their outer-product sum (0 for one direction).
fn turns(m: [[f64; 3]; 3]) -> f64 {
    let (w, _) = geom_core::linalg::jacobi_sym(&flat(m), 3); // ascending
    let sum = w[0] + w[1] + w[2];
    if !(sum > 0.0) {
        return 0.0;
    }
    libm::sqrt((w[1] / sum).max(0.0))
}

/// A curved primitive whose inliers' normals turn less than the angle band is a plane in disguise.
fn bends(kind: &Primitive, unit: &[Vec3], idx: impl Iterator<Item = usize>, band: Tol) -> bool {
    !kind.curved() || turns(outer_sum(idx.map(|i| unit[i]))) >= libm::sin(band.ang)
}

fn inliers_of(kind: &Primitive, points: &[Vec3], unit: &[Vec3], over: impl Iterator<Item = usize>, band: Tol) -> Vec<usize> {
    over.filter(|&i| kind.residual(points[i], unit[i], band).is_some()).collect()
}

/// Refit a candidate over the live points (REFINE passes), keep its largest connected component, refit on it and keep
/// the component's points still within the band; `None` when it thins below a minimal set, leaves its admissible range
/// or stops bending.
fn settle(kind: Primitive, points: &[Vec3], unit: &[Vec3], live: &[u32], grid: &Grid, band: Tol) -> Option<(Primitive, Vec<usize>)> {
    let mut k = kind;
    let mut idx = inliers_of(&k, points, unit, live.iter().map(|&i| i as usize), band);
    for _ in 0..REFINE {
        if idx.len() < MIN_SUPPORT {
            return None;
        }
        k = k.refit(points, unit, &idx);
        if !k.admissible(band) {
            return None;
        }
        idx = inliers_of(&k, points, unit, live.iter().map(|&i| i as usize), band);
    }
    if idx.len() < MIN_SUPPORT {
        return None;
    }
    let comp = largest_component(points, &idx, grid);
    if comp.len() < MIN_SUPPORT {
        return None;
    }
    k = k.refit(points, unit, &comp);
    if !k.admissible(band) {
        return None;
    }
    let idx = inliers_of(&k, points, unit, comp.into_iter(), band);
    if idx.len() < MIN_SUPPORT || !bends(&k, unit, idx.iter().copied(), band) {
        return None;
    }
    Some((k, idx))
}

/// Schnabel's multi-shape detection: every plane, cylinder, sphere and cone the cloud supports above `floor`, each
/// with the input indices it explains and its residuals, ordered by support (largest first). Greedy: each round draws
/// [`TRIALS_PER_ROUND`] local three-point samples (the first point uniform over the unexplained points, the other two
/// from its cell at one of three levels), makes up to four candidates from each, estimates their support on a
/// [`SUBSET`] of the unexplained points, settles the best (refit, largest connected component, the curvature test)
/// and accepts the first that meets the floor; its points leave the pool; the loop ends when no candidate does or
/// at [`MAX_SHAPES`]. Deterministic (a fixed seed). Fewer than three points, or a cloud of no extent, yields none.
pub fn detect(points: &[Vec3], normals: &[Vec3], band: Tol, floor: Floor) -> Result<Vec<Shape>, FitError> {
    let n = points.len();
    if normals.len() != n {
        return Err(FitError::NormalsMismatch { points: n, normals: normals.len() });
    }
    let mut out: Vec<Shape> = Vec::new();
    if n < 3 {
        return Ok(out);
    }
    let (mut lo, mut hi) = (points[0], points[0]);
    for p in points {
        lo = Vec3::new(lo.x.min(p.x), lo.y.min(p.y), lo.z.min(p.z));
        hi = Vec3::new(hi.x.max(p.x), hi.y.max(p.y), hi.z.max(p.z));
    }
    let extent = (hi.x - lo.x).max(hi.y - lo.y).max(hi.z - lo.z);
    if !(extent > 0.0 && extent.is_finite()) {
        return Ok(out);
    }
    let cell = extent * CELL_FRACTION;
    let unit: Vec<Vec3> = normals.iter().map(|v| v.normalize().unwrap_or(Vec3::ZERO)).collect();
    let grids = [Grid::build(points, lo, cell), Grid::build(points, lo, cell * 4.0), Grid::build(points, lo, cell * 16.0)];
    // A point with no normal can pass no normal test: it never joins a shape.
    let mut alive: Vec<bool> = unit.iter().map(|u| u.mag() > 0.0).collect();
    let mut rng = SplitMix::new(SEED ^ DETECT_SEED);
    let cos_ang = libm::cos(band.ang);
    while out.len() < MAX_SHAPES {
        let live: Vec<u32> = (0..n).filter(|&i| alive[i]).map(|i| i as u32).collect();
        if live.len() < (floor.points as usize).max(MIN_SUPPORT) {
            break;
        }
        let subset: Vec<usize> = if live.len() <= SUBSET {
            live.iter().map(|&i| i as usize).collect()
        } else {
            (0..SUBSET).map(|_| live[rng.below(live.len())] as usize).collect()
        };
        let mut cands: Vec<(usize, Primitive)> = Vec::new();
        for trial in 0..TRIALS_PER_ROUND {
            let first = live[rng.below(live.len())] as usize;
            let g = &grids[(trial % 3) as usize];
            let members = g.members(g.key(points[first]));
            if members.len() < 3 {
                continue;
            }
            let mut pick = [first; 3];
            let mut got = 1;
            for _ in 0..16 {
                if got == 3 {
                    break;
                }
                let m = members[rng.below(members.len())].1 as usize;
                if alive[m] && !pick[..got].contains(&m) {
                    pick[got] = m;
                    got += 1;
                }
            }
            if got < 3 {
                continue;
            }
            let s = [(points[pick[0]], unit[pick[0]]), (points[pick[1]], unit[pick[1]]), (points[pick[2]], unit[pick[2]])];
            let mut kinds: [Option<Primitive>; 4] = [None; 4];
            // plane: three points (fit_plane's collinearity test), each normal within the band.
            let nrm = (s[1].0 - s[0].0).cross(s[2].0 - s[0].0);
            let longest = (s[1].0 - s[0].0).mag().max((s[2].0 - s[0].0).mag()).max((s[2].0 - s[1].0).mag());
            if nrm.mag() > band.lin * longest {
                if let Ok(u) = nrm.normalize() {
                    if s.iter().all(|q| q.1.dot(u).abs() >= cos_ang) {
                        kinds[0] = Some(Primitive::Plane(Plane { normal: u, d: u.dot(s[0].0) }));
                    }
                }
            }
            kinds[1] = cyl_candidate(s[0].0, s[0].1, s[1].0, s[1].1, band).map(Primitive::Cylinder);
            kinds[2] = sphere_candidate(&s, band).map(Primitive::Sphere);
            kinds[3] = cone_candidate(&s, band).map(Primitive::Cone);
            for k in kinds.into_iter().flatten() {
                let hits: Vec<usize> = subset.iter().copied().filter(|&i| k.residual(points[i], unit[i], band).is_some()).collect();
                if hits.len() < 3 || !bends(&k, &unit, hits.iter().copied(), band) {
                    continue;
                }
                cands.push((hits.len(), k));
            }
        }
        // Best estimate first; the stable sort keeps the draw order among equals (deterministic).
        cands.sort_by(|a, b| b.0.cmp(&a.0));
        let mut found = None;
        for &(_, k) in cands.iter().take(TRIED) {
            if let Some((k, idx)) = settle(k, points, &unit, &live, &grids[0], band) {
                if floor.admits(idx.len(), live.len()) {
                    found = Some((k, idx));
                    break;
                }
            }
        }
        let Some((k, idx)) = found else { break };
        let (rms, mx, cnt) = residuals(idx.iter().filter_map(|&i| k.residual(points[i], unit[i], band)));
        for &i in &idx {
            alive[i] = false;
        }
        out.push(Shape {
            kind: k,
            inliers: idx.iter().map(|&i| i as u32).collect(),
            support: cnt,
            fraction: cnt as f64 / n as f64,
            rms_mm: rms,
            max_mm: mx,
        });
    }
    out.sort_by(|a, b| b.support.cmp(&a.support));
    Ok(out)
}
