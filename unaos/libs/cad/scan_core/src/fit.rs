// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Primitive fitting with residuals: RANSAC (Fischler & Bolles, CACM 1981) over Schnabel, Wahl & Klein's minimal
//! sets ("Efficient RANSAC for Point-Cloud Shape Detection", Computer Graphics Forum 2007: a plane from three points,
//! a cylinder from two points with normals — axis `n1 × n2`, the axis point where the two normal lines meet in the
//! plane across the axis, the radius their mean distance; an inlier lies within the distance band AND its normal
//! within the angle band). The trial count adapts to the best inlier fraction `w` seen so far,
//! `T = ln(1 − 0.99) / ln(1 − w^k)` (Hartley & Zisserman, Multiple View Geometry §4.7), under a cap; the sampler is
//! a fixed-seed splitmix64, so a fit is bit-reproducible.
//!
//! The consensus set is refined: a plane by total least squares (the centroid and the covariance's smallest
//! eigenvector); a cylinder's axis as the smallest eigenvector of `Σ n nᵀ` over the inliers' normals (they lie
//! across the axis), then centre and radius by the algebraic circle fit (Kåsa, IEEE Trans. Instrum. Meas. 1976) of
//! the points projected on the plane across the axis. Residuals are reported over the refined inliers (BP-vug-5:
//! a fitted shape prints its residuals; it is a measurement, not design geometry).
//!
//! `band` is BP-vug-2's `Tol` used as the measurement band: `lin` the distance (mm), `ang` the normal deviation (rad).

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

/// splitmix64 (Steele, Lea & Flood 2014): the deterministic sampler (also the fixture's noise source).
pub(crate) struct SplitMix(u64);

impl SplitMix {
    pub(crate) fn new(seed: u64) -> SplitMix {
        SplitMix(seed)
    }

    pub(crate) fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform in [0, 1).
    pub(crate) fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next_u64() % n as u64) as usize
    }
}

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

/// Eigen-decomposition of a symmetric 3×3 by cyclic Jacobi rotations (Numerical Recipes §11.1): the eigenvalues and
/// the eigenvectors as COLUMNS of the returned matrix.
fn eigen_sym3(mut a: [[f64; 3]; 3]) -> ([f64; 3], [[f64; 3]; 3]) {
    let mut v = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    for _ in 0..50 {
        let off = a[0][1] * a[0][1] + a[0][2] * a[0][2] + a[1][2] * a[1][2];
        let mut norm = 0.0;
        for row in &a {
            for x in row {
                norm += x * x;
            }
        }
        if off <= norm * (f64::EPSILON * f64::EPSILON) {
            break;
        }
        for (p, q) in [(0usize, 1usize), (0, 2), (1, 2)] {
            if a[p][q] == 0.0 {
                continue;
            }
            let theta = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
            let sign = if theta >= 0.0 { 1.0 } else { -1.0 };
            let t = sign / (theta.abs() + libm::sqrt(theta * theta + 1.0));
            let c = 1.0 / libm::sqrt(t * t + 1.0);
            let s = t * c;
            for k in 0..3 {
                let (akp, akq) = (a[k][p], a[k][q]);
                a[k][p] = c * akp - s * akq;
                a[k][q] = s * akp + c * akq;
            }
            for k in 0..3 {
                let (apk, aqk) = (a[p][k], a[q][k]);
                a[p][k] = c * apk - s * aqk;
                a[q][k] = s * apk + c * aqk;
            }
            for row in v.iter_mut() {
                let (vkp, vkq) = (row[p], row[q]);
                row[p] = c * vkp - s * vkq;
                row[q] = s * vkp + c * vkq;
            }
        }
    }
    ([a[0][0], a[1][1], a[2][2]], v)
}

/// The unit eigenvector of the smallest eigenvalue.
fn smallest_eigvec(m: [[f64; 3]; 3]) -> Vec3 {
    let (w, v) = eigen_sym3(m);
    let mut k = 0;
    for i in 1..3 {
        if w[i] < w[k] {
            k = i;
        }
    }
    Vec3::new(v[0][k], v[1][k], v[2][k]).normalize().unwrap_or(Vec3::ZERO)
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
