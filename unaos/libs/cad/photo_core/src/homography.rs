// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Step 3: the lattice homography. H maps paper millimetres (i · pitch, j · pitch) to photo pixels, `px ~ H [x, y, 1]`
//! (row-major). The normalised DLT (Hartley, "In defense of the eight-point algorithm", IEEE PAMI 1997; Hartley &
//! Zisserman, Multiple View Geometry 2nd ed., Alg 4.2): each point set translated to its centroid and scaled to a
//! mean distance of √2, two rows of A per correspondence (H&Z eq 4.3), H's nine entries the eigenvector of AᵀA with
//! the smallest eigenvalue (geom_core's `linalg::jacobi_sym`, cyclic Jacobi, Numerical Recipes §11.1; VUG-GEOM-LINALG
//! SR520: the one copy scan_core shares). RANSAC (Fischler & Bolles, CACM 1981; H&Z
//! Alg 4.6) over 4-intersection minimal sets: a set with three points collinear IN PAPER coordinates is skipped by
//! geom_core's exact `orient2d` (lattice points are exactly collinear, and the predicate says so exactly); inliers
//! within [`BAND_PX`]; the adaptive count `T = ln(1 − 0.99) / ln(1 − w⁴)` under a cap; a fixed-seed splitmix64
//! (geom_core's `linalg::SplitMix64`, this crate's seed), so a measurement is bit-reproducible; the consensus refit twice.
//!
//! The paper frame is y-up as seen from above, so paper → photo (y down) must reverse orientation: when the
//! lattice's local Jacobian has a positive determinant the j axis is negated (H ← H · diag(1, −1, 1)). The tilt
//! off square is read from the same Jacobian J at the inliers' centroid: with singular values s1 ≥ s2, a camera
//! square to the paper gives a similarity (s2 = s1) and a tilt t gives s2 / s1 ≈ cos t, skew included, so
//! `tilt_deg = acos(s2 / s1)`. (A full 8-dof H absorbs any tilt of a flat sheet: the lattice is perpendicular after
//! H by construction, so that cannot be the test.)

use alloc::vec::Vec;

use geom_core::{Mat3, Predicate, Vec3, orient2d};

use crate::grid::Grid;
use crate::{Degenerate, MIN_INTERSECTIONS, Refusal, Vec2};

/// A homography, row-major.
pub type H = [[f64; 3]; 3];

/// The RANSAC inlier band (px).
pub const BAND_PX: f64 = 2.0;
/// The probability a run draws at least one all-inlier minimal set.
pub const CONFIDENCE: f64 = 0.99;
/// The trial cap.
pub const MAX_TRIALS: u32 = 2000;
const SEED: u64 = 0x9A9E_4D06_0503_0001;

/// splitmix64: geom_core's one (VUG-GEOM-LINALG SR520), the deterministic sampler and the fixture's noise source.
pub(crate) use geom_core::linalg::SplitMix64 as SplitMix;

fn trials_for(inliers: usize, n: usize) -> u32 {
    let w = inliers as f64 / n as f64;
    let wk = libm::pow(w, 4.0);
    if wk >= 1.0 {
        return 1;
    }
    if wk <= 0.0 {
        return MAX_TRIALS;
    }
    let t = libm::ceil(libm::log(1.0 - CONFIDENCE) / libm::log(1.0 - wk));
    if t.is_finite() && t < MAX_TRIALS as f64 { (t as u32).max(1) } else { MAX_TRIALS }
}

fn to_mat3(h: &H) -> Mat3 {
    Mat3::from_cols(
        Vec3::new(h[0][0], h[1][0], h[2][0]),
        Vec3::new(h[0][1], h[1][1], h[2][1]),
        Vec3::new(h[0][2], h[1][2], h[2][2]),
    )
}

fn from_mat3(m: Mat3) -> H {
    let c = m.cols;
    [[c[0].x, c[1].x, c[2].x], [c[0].y, c[1].y, c[2].y], [c[0].z, c[1].z, c[2].z]]
}

/// `a · b` (geom_core's Mat3 product).
pub fn mul(a: &H, b: &H) -> H {
    from_mat3(to_mat3(a) * to_mat3(b))
}

/// H⁻¹ (geom_core's Mat3 inverse).
pub fn invert(h: &H) -> Result<H, Degenerate> {
    Ok(from_mat3(to_mat3(h).inverse()?))
}

/// The image of `p`; None at the line at infinity.
pub fn apply(h: &H, p: Vec2) -> Option<Vec2> {
    let w = h[2][0] * p.x + h[2][1] * p.y + h[2][2];
    if !(w.abs() > 1e-300) {
        return None;
    }
    let q = Vec2::new((h[0][0] * p.x + h[0][1] * p.y + h[0][2]) / w, (h[1][0] * p.x + h[1][1] * p.y + h[1][2]) / w);
    if q.is_finite() { Some(q) } else { None }
}

/// The 2x2 Jacobian of `p ↦ apply(h, p)` at `p`, row-major.
pub fn jacobian(h: &H, p: Vec2) -> [[f64; 2]; 2] {
    let x = h[0][0] * p.x + h[0][1] * p.y + h[0][2];
    let y = h[1][0] * p.x + h[1][1] * p.y + h[1][2];
    let w = h[2][0] * p.x + h[2][1] * p.y + h[2][2];
    let w2 = w * w;
    [
        [(h[0][0] * w - x * h[2][0]) / w2, (h[0][1] * w - x * h[2][1]) / w2],
        [(h[1][0] * w - y * h[2][0]) / w2, (h[1][1] * w - y * h[2][1]) / w2],
    ]
}

/// The tilt off square (degrees) of a local affine: acos of its singular-value ratio.
pub fn tilt_deg(j: &[[f64; 2]; 2]) -> f64 {
    let (a, b, c, d) = (j[0][0], j[0][1], j[1][0], j[1][1]);
    let s = a * a + b * b + c * c + d * d;
    let det = (a * d - b * c).abs();
    let disc = libm::sqrt((s * s - 4.0 * det * det).max(0.0));
    let s1 = libm::sqrt((s + disc) * 0.5);
    if !(s1 > 0.0) {
        return 90.0;
    }
    let ratio = (det / s1 / s1).clamp(0.0, 1.0);
    libm::acos(ratio) * 180.0 / core::f64::consts::PI
}

/// Hartley's normalisation: the similarity taking `pts` to centroid 0 and mean distance √2, and the moved points.
fn normalise(pts: &[Vec2]) -> Result<(H, Vec<Vec2>), Degenerate> {
    let n = pts.len() as f64;
    let c = pts.iter().fold(Vec2::ZERO, |a, &p| a + p) / n;
    let md = pts.iter().map(|&p| p.dist(c)).sum::<f64>() / n;
    if !(md > 0.0 && md.is_finite()) {
        return Err(Degenerate::Collinear);
    }
    let s = core::f64::consts::SQRT_2 / md;
    let t = [[s, 0.0, -s * c.x], [0.0, s, -s * c.y], [0.0, 0.0, 1.0]];
    Ok((t, pts.iter().map(|&p| (p - c) * s).collect()))
}

/// The normalised DLT from `paper` to `px` (at least four correspondences).
pub fn dlt(paper: &[Vec2], px: &[Vec2]) -> Result<H, Degenerate> {
    if paper.len() < 4 || paper.len() != px.len() {
        return Err(Degenerate::Collinear);
    }
    let (tp, p) = normalise(paper)?;
    let (tx, x) = normalise(px)?;
    let mut ata = [0.0f64; 81];
    for k in 0..p.len() {
        let (xx, yy) = (p[k].x, p[k].y);
        let (u, v) = (x[k].x, x[k].y);
        let r1 = [0.0, 0.0, 0.0, -xx, -yy, -1.0, v * xx, v * yy, v];
        let r2 = [xx, yy, 1.0, 0.0, 0.0, 0.0, -u * xx, -u * yy, -u];
        for r in [r1, r2] {
            for i in 0..9 {
                for j in 0..9 {
                    ata[i * 9 + j] += r[i] * r[j];
                }
            }
        }
    }
    // Ascending eigenpairs: column 0 is the smallest (the first strict minimum).
    let (_, vecs) = geom_core::linalg::jacobi_sym(&ata, 9);
    let mut hn = [[0.0; 3]; 3];
    for i in 0..9 {
        hn[i / 3][i % 3] = vecs[i * 9];
    }
    let mut h = mul(&mul(&invert(&tx)?, &hn), &tp);
    let s = if h[2][2].abs() > 1e-12 {
        h[2][2]
    } else {
        libm::sqrt(h.iter().flatten().map(|v| v * v).sum::<f64>())
    };
    for row in &mut h {
        for v in row {
            *v /= s;
        }
    }
    if h.iter().flatten().all(|v| v.is_finite()) { Ok(h) } else { Err(Degenerate::NonFinite) }
}

fn inliers(h: &H, paper: &[Vec2], px: &[Vec2], band: f64) -> Vec<usize> {
    (0..paper.len()).filter(|&k| apply(h, paper[k]).is_some_and(|q| q.dist(px[k]) <= band)).collect()
}

fn pick<T: Copy>(v: &[T], idx: &[usize]) -> Vec<T> {
    idx.iter().map(|&k| v[k]).collect()
}

/// RANSAC around the normalised DLT: the refitted H and its inliers.
pub fn ransac(paper: &[Vec2], px: &[Vec2], band: f64) -> Result<(H, Vec<usize>), Refusal> {
    let n = paper.len();
    if n < MIN_INTERSECTIONS {
        return Err(Refusal::TooFewIntersections { have: n, need: MIN_INTERSECTIONS });
    }
    let mut rng = SplitMix::new(SEED);
    let mut best: Vec<usize> = Vec::new();
    let mut cap = MAX_TRIALS;
    let mut trials = 0u32;
    while trials < cap {
        trials += 1;
        let mut s = [0usize; 4];
        let mut k = 0;
        while k < 4 {
            let c = rng.below(n);
            if !s[..k].contains(&c) {
                s[k] = c;
                k += 1;
            }
        }
        let triples = [(0, 1, 2), (0, 1, 3), (0, 2, 3), (1, 2, 3)];
        let general = triples.iter().all(|&(a, b, c)| {
            matches!(orient2d(paper[s[a]], paper[s[b]], paper[s[c]]), Predicate::Positive | Predicate::Negative)
        });
        if !general {
            continue;
        }
        let Ok(h) = dlt(&pick(paper, &s), &pick(px, &s)) else { continue };
        let inl = inliers(&h, paper, px, band);
        if inl.len() > best.len() {
            best = inl;
            cap = trials_for(best.len(), n).max(trials.min(MAX_TRIALS));
        }
    }
    if best.len() < MIN_INTERSECTIONS {
        return Err(Refusal::TooFewIntersections { have: best.len(), need: MIN_INTERSECTIONS });
    }
    let mut h = dlt(&pick(paper, &best), &pick(px, &best)).map_err(Refusal::Degenerate)?;
    for _ in 0..2 {
        let inl = inliers(&h, paper, px, band);
        if inl.len() < MIN_INTERSECTIONS {
            break;
        }
        best = inl;
        h = dlt(&pick(paper, &best), &pick(px, &best)).map_err(Refusal::Degenerate)?;
    }
    Ok((h, best))
}

/// The fitted lattice: H (paper mm → px), its residuals, the tilt, the local scale, the radial hint and the paper
/// bounding box of the inliers.
#[derive(Clone, Debug, PartialEq)]
pub struct Lattice {
    pub h: H,
    pub inliers: usize,
    pub residual_px: f64,
    pub residual_mm: f64,
    pub tilt_deg: f64,
    pub px_per_mm: f64,
    pub lens_hint: f64,
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

/// Fit the lattice of `grid` at `pitch` mm in a photo `w` x `h` px.
pub fn fit_lattice(grid: &Grid, pitch: f64, w: usize, h: usize) -> Result<Lattice, Refusal> {
    let mut paper: Vec<Vec2> = grid.inter.iter().map(|q| Vec2::new(q.i as f64 * pitch, q.j as f64 * pitch)).collect();
    let px: Vec<Vec2> = grid.inter.iter().map(|q| q.px).collect();
    let (mut hm, inl) = ransac(&paper, &px, BAND_PX)?;
    let centroid = |paper: &[Vec2]| inl.iter().fold(Vec2::ZERO, |a, &k| a + paper[k]) / inl.len() as f64;
    let j = jacobian(&hm, centroid(&paper[..]));
    if j[0][0] * j[1][1] - j[0][1] * j[1][0] > 0.0 {
        for p in &mut paper {
            p.y = -p.y;
        }
        hm = mul(&hm, &[[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, 1.0]]);
    }
    let pc = centroid(&paper[..]);
    let j = jacobian(&hm, pc);
    let tilt = tilt_deg(&j);
    let px_per_mm = libm::sqrt((j[0][0] * j[1][1] - j[0][1] * j[1][0]).abs());
    let hinv = invert(&hm).map_err(Refusal::Degenerate)?;
    let (mut e_px, mut e_mm) = (0.0, 0.0);
    let centre = Vec2::new(w as f64 * 0.5, h as f64 * 0.5);
    let r_max = libm::hypot(w as f64 * 0.5, h as f64 * 0.5);
    let (mut num, mut den) = (0.0, 0.0);
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for &k in &inl {
        let q = apply(&hm, paper[k]).ok_or(Refusal::Degenerate(Degenerate::NonFinite))?;
        let e = px[k] - q;
        e_px += e.mag_sq();
        let back = apply(&hinv, px[k]).ok_or(Refusal::Degenerate(Degenerate::NonFinite))?;
        e_mm += back.dist(paper[k]) * back.dist(paper[k]);
        let rv = px[k] - centre;
        let r = rv.mag();
        if r > 0.0 {
            let s = libm::pow(r / r_max, 3.0);
            num += e.dot(rv / r) * s;
            den += s * s;
        }
        x0 = x0.min(paper[k].x);
        y0 = y0.min(paper[k].y);
        x1 = x1.max(paper[k].x);
        y1 = y1.max(paper[k].y);
    }
    let m = inl.len() as f64;
    Ok(Lattice {
        h: hm,
        inliers: inl.len(),
        residual_px: libm::sqrt(e_px / m),
        residual_mm: libm::sqrt(e_mm / m),
        tilt_deg: tilt,
        px_per_mm,
        lens_hint: if den > 0.0 { num / den } else { 0.0 },
        x0,
        y0,
        x1,
        y1,
    })
}

// VUG-PHOTO-LENS (SR533): the Brown one-parameter radial term (Brown, "Decentering distortion of lenses",
// Photogrammetric Engineering 1966; its k1 alone) about the photo's centre, normalised by the half-diagonal (the
// centre and normaliser `fit_lattice`'s `lens_hint` reads): an undistorted point at normalised radius r is seen at
// r_d = r (1 + k1 r²). k1 is estimated AFTER the homography from the lattice residual: a golden-section search
// (Kiefer 1953) on k1 in [−K1_SPAN, K1_SPAN] minimising the rms residual of the DLT refitted to the undistorted
// intersections. k1 = 0 is the identity by an explicit branch, so a flat photo measures bit-exactly as SR503.

/// The k1 search span (normalised by the half-diagonal; at −0.12 the corner, r_d = 1, is still invertible).
pub const K1_SPAN: f64 = 0.12;
/// The golden-section bracket width at which the search stops.
pub const K1_TOL: f64 = 1e-5;
/// The term is taken only when it brings the lattice residual to at most this fraction of the flat residual.
pub const LENS_GAIN: f64 = 0.5;
/// Refinement passes: the photo undistorted by the current k1, the grid found again, k1 re-estimated.
pub const LENS_PASSES: usize = 2;

fn lens_frame(w: usize, h: usize) -> (Vec2, f64) {
    (Vec2::new(w as f64 * 0.5, h as f64 * 0.5), libm::hypot(w as f64 * 0.5, h as f64 * 0.5))
}

/// Where the undistorted pixel `p` of a `w` x `h` photo is seen under `k1`.
pub fn distort(p: Vec2, k1: f64, w: usize, h: usize) -> Vec2 {
    if k1 == 0.0 {
        return p;
    }
    let (c, rm) = lens_frame(w, h);
    let n = (p - c) / rm;
    c + n * (rm * (1.0 + k1 * n.mag_sq()))
}

/// The undistorted pixel seen at `q` under `k1`: r (1 + k1 r²) = r_d by Newton from r = r_d.
pub fn undistort(q: Vec2, k1: f64, w: usize, h: usize) -> Vec2 {
    if k1 == 0.0 {
        return q;
    }
    let (c, rm) = lens_frame(w, h);
    let nd = (q - c) / rm;
    let rd = nd.mag();
    if !(rd > 0.0) {
        return q;
    }
    let mut r = rd;
    for _ in 0..20 {
        let d = 1.0 + 3.0 * k1 * r * r;
        if !(d > 0.0) {
            break;
        }
        let step = (r + k1 * r * r * r - rd) / d;
        r -= step;
        if !(step.abs() >= 1e-14) {
            break;
        }
    }
    c + nd * (rm * r / rd)
}

/// The rms residual (px, undistorted) of the DLT refitted to `raw` undistorted by `k1` against `paper`; +inf on a
/// degenerate fit.
pub fn lens_residual(paper: &[Vec2], raw: &[Vec2], k1: f64, w: usize, h: usize) -> f64 {
    let px: Vec<Vec2> = raw.iter().map(|&q| undistort(q, k1, w, h)).collect();
    let Ok(hm) = dlt(paper, &px) else { return f64::INFINITY };
    let mut e = 0.0;
    for k in 0..paper.len() {
        match apply(&hm, paper[k]) {
            Some(q) => e += q.dist(px[k]) * q.dist(px[k]),
            None => return f64::INFINITY,
        }
    }
    libm::sqrt(e / paper.len().max(1) as f64)
}

/// Golden-section search for the k1 minimising [`lens_residual`]: (k1, its rms residual).
pub fn fit_k1(paper: &[Vec2], raw: &[Vec2], w: usize, h: usize) -> (f64, f64) {
    let g = (libm::sqrt(5.0) - 1.0) * 0.5;
    let f = |k: f64| lens_residual(paper, raw, k, w, h);
    let (mut a, mut b) = (-K1_SPAN, K1_SPAN);
    let mut c = b - g * (b - a);
    let mut d = a + g * (b - a);
    let (mut fc, mut fd) = (f(c), f(d));
    while b - a > K1_TOL {
        if fc <= fd {
            b = d;
            d = c;
            fd = fc;
            c = b - g * (b - a);
            fc = f(c);
        } else {
            a = c;
            c = d;
            fc = fd;
            d = a + g * (b - a);
            fd = f(d);
        }
    }
    let k = 0.5 * (a + b);
    (k, f(k))
}

/// The lens fit's correspondences: the paper points of RANSAC's inliers among `grid`'s intersections (built as
/// [`fit_lattice`] builds them) and their pixels taken back to the RAW photo by `to_raw`.
pub(crate) fn lens_set(grid: &Grid, pitch: f64, to_raw: &dyn Fn(Vec2) -> Vec2) -> Result<(Vec<Vec2>, Vec<Vec2>), Refusal> {
    let paper: Vec<Vec2> = grid.inter.iter().map(|q| Vec2::new(q.i as f64 * pitch, q.j as f64 * pitch)).collect();
    let px: Vec<Vec2> = grid.inter.iter().map(|q| q.px).collect();
    let (_, inl) = ransac(&paper, &px, BAND_PX)?;
    Ok((pick(&paper, &inl), inl.iter().map(|&k| to_raw(px[k])).collect()))
}
