// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Vug (CODEX §2, 3D), seam shared-core — the deviation map read as ISO 1101 tolerances per design face:
//! flatness, cylindricity, position.
//!
//! VUGSCANGDT (LEDGER SR542); BP-vug-5: a verdict is a printed measurement against a named tolerance, never a change to
//! the design or the scan. Pure over the scan, the [`Design`] and the [`Deviation`] [`crate::deviation`] measured.
//!
//! 1. [`nominal`]: each design face's surface from its own tessellation — a Plane when every triangle normal agrees
//!    with the face's area-weighted mean within 0.5° (`fit_plane` through its corners, the normal outward), else a
//!    Cylinder when `fit_cylinder` through the triangle centroids and normals leaves no outlier (the radius re-read
//!    from the face's vertices, which lie on the surface), else Other (cone, torus, sphere: not toleranced here).
//! 2. The form vertices of a face: the scan vertices `deviations` paired with it (`Deviation::face_of_vertex`), at the
//!    aligned pose, whose TSDF normal agrees with the nominal's normal there within [`GDT_NORMAL_DEG`]: the TSDF's
//!    rounded edges pair with a face through deviation.rs's 60° gate and are a blend, not the face's form. The edge
//!    margin (SR542, the seat's ruling): a vertex is dropped when its projection onto the nominal surface lies within
//!    `margin_mm` of the face's boundary (the edges of the face's tessellation that one of its triangles alone uses,
//!    keyed by position so a seam a tessellation duplicates is not a boundary; the distance is Euclidean from the
//!    projected point, the chord, which on a cylinder is the axial distance to a rim and under the margin the arc to a
//!    straight edge); the caller passes the TSDF's truncation (`Tsdf::trunc_mm`), the width of the blend. Fewer than
//!    [`GDT_MIN_POINTS`] and the face is unmeasured.
//! 3. Flatness (ISO 1101, two parallel planes of free orientation): max − min of the signed distance of the form
//!    vertices to the plane `fit_plane` gives through them. Cylindricity (two coaxial cylinders, axis and radius free):
//!    max − min of the radial residual about the cylinder `fit_cylinder` gives through them. The reference is least
//!    squares after the RANSAC consensus; every form vertex counts in the band, the consensus only places the
//!    reference. The min-zone (Chebyshev) value under it, VUGSCANGDTMINZONE (SR554): [`flatness_minzone`] /
//!    [`cylindricity_minzone`], the discrete Chebyshev band as its dual LP (`chebyshev_band`, the exchange method).
//! 4. Position: the fitted axis against the nominal axis as a diameter zone, [`position_mm`]: twice the larger distance
//!    of the measured axis from the nominal one at the feature's two axial ends. The frame is the design's after the
//!    best-fit registration; the datum frame (ISO 5459, VUGSCANGDTDATUMS SR552) is [`frame_of`] / [`position_in`].
//! 5. `band_mm`: the nominal-relative band (max − min of the signed deviation over the form vertices), printed beside
//!    the form values; the zone that takes a verdict is `profile_mm` ([`Kind::Profile`], VUGSCANGDTSPEC SR559), located
//!    in a datum frame by [`in_frame`] / [`profile_in`] and offset by an unequal zone `@<off>` (VUGSCANGDTPROFILEDATUM SR567).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::ScanMesh;
use crate::deviation::{AlignParams, DESIGN_CHORD_MM, Design, Deviation};
use crate::fit::{Cylinder, fit_cylinder, fit_plane};
use crate::geom::{Plane, Tol, Vec3};

/// A form vertex's TSDF normal agrees with the nominal surface's normal within this (degrees).
pub const GDT_NORMAL_DEG: f64 = 10.0;
/// The fewest form vertices a face is measured on.
pub const GDT_MIN_POINTS: u32 = 16;
/// A design face is a plane when every triangle normal is within this of the face's mean (degrees).
const FLAT_DEG: f64 = 0.5;
/// The consensus band of the form fits: k·σ at the default scan noise (4.5 mm, D435i at 0.6 m), 30°.
const FIT_BAND: Tol = Tol { lin: AlignParams::DEFAULT.k_sigma * AlignParams::DEFAULT.sigma_mm, ang: 30.0 * core::f64::consts::PI / 180.0 };

/// A design face's surface, read from its tessellation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Nominal {
    /// The plane, its normal outward from the material.
    Plane(Plane),
    Cylinder(Cylinder),
    Other,
}

impl Nominal {
    pub fn name(&self) -> &'static str {
        match self {
            Nominal::Plane(_) => "plane",
            Nominal::Cylinder(_) => "cylinder",
            Nominal::Other => "other",
        }
    }
}

/// A tolerance ISO 1101 names, as `--gdt` spells it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Flatness,
    Cylindricity,
    Position,
    /// Profile of a surface (ISO 1101, no datum): the zone centred on the nominal, `FaceGdt::profile_mm`. VUGSCANGDTSPEC (SR559).
    Profile,
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "flatness" => Some(Kind::Flatness),
            "cylindricity" => Some(Kind::Cylindricity),
            "position" => Some(Kind::Position),
            "profile" => Some(Kind::Profile),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Flatness => "flatness",
            Kind::Cylindricity => "cylindricity",
            Kind::Position => "position",
            Kind::Profile => "profile",
        }
    }
}

/// A measured value against a named tolerance; `Unmeasured` where the face's surface does not take the kind or too few
/// form vertices stood on it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Verdict {
    Pass { value_mm: f64, tol_mm: f64 },
    Fail { value_mm: f64, tol_mm: f64 },
    Unmeasured,
}

/// One design face read as tolerances: its nominal, the form vertices measured (`n`, of the `matched` vertices
/// `deviations` paired with the face), each value its surface takes (`None`
/// where it does not, or unmeasured), the nominal-relative band, the fitted axis of a cylinder and the form vertices'
/// extent along the nominal axis (from the nominal's point; `(0, 0)` for a plane).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceGdt {
    pub face: u32,
    pub nominal: Nominal,
    pub n: u32,
    pub matched: u32,
    pub flatness_mm: Option<f64>,
    pub cylindricity_mm: Option<f64>,
    pub position_mm: Option<f64>,
    pub band_mm: Option<f64>,
    /// VUGSCANGDTSPEC (SR559): profile of a surface, 2 · max |signed deviation| over the form vertices (the width of the
    /// zone centred on the nominal that holds them all; never below `band_mm`).
    pub profile_mm: Option<f64>,
    /// VUGSCANGDTPROFILEDATUM (SR567): the signed (min, max) of the form vertices' deviation at the best-fit registration
    /// (positive proud), the span an unequal zone reads ([`zone_mm`]).
    pub span_mm: Option<(f64, f64)>,
    /// VUGSCANGDTPROFILEDATUM (SR567): profile of a surface located in the datum frame ([`in_frame`]), 2 · max |d'|.
    pub profile_in_frame_mm: Option<f64>,
    /// VUGSCANGDTPROFILEDATUM (SR567): the signed (min, max) of d' in the datum frame ([`profile_in`]).
    pub frame_span_mm: Option<(f64, f64)>,
    pub axis: Option<Cylinder>,
    pub extent: (f64, f64),
    pub minzone_flatness_mm: Option<MinZone>,
    pub minzone_cylindricity_mm: Option<MinZone>,
}

impl FaceGdt {
    /// Whether this face's surface takes the kind: flatness a plane, cylindricity and position a cylinder, profile either.
    pub fn accepts(&self, kind: Kind) -> bool {
        matches!((kind, self.nominal), (Kind::Flatness, Nominal::Plane(_)) | (Kind::Cylindricity | Kind::Position, Nominal::Cylinder(_)) | (Kind::Profile, Nominal::Plane(_) | Nominal::Cylinder(_)))
    }

    pub fn value(&self, kind: Kind) -> Option<f64> {
        match kind {
            Kind::Flatness => self.flatness_mm,
            Kind::Cylindricity => self.cylindricity_mm,
            Kind::Position => self.position_mm,
            Kind::Profile => self.profile_in_frame_mm.or(self.profile_mm), // VUGSCANGDTPROFILEDATUM (SR567): read in the datum frame when one was given
        }
    }

    /// The value against `tol_mm`: inside or on it passes.
    pub fn verdict(&self, kind: Kind, tol_mm: f64) -> Verdict {
        if !self.accepts(kind) {
            return Verdict::Unmeasured;
        }
        match self.value(kind) {
            Some(v) if v <= tol_mm => Verdict::Pass { value_mm: v, tol_mm },
            Some(v) => Verdict::Fail { value_mm: v, tol_mm },
            None => Verdict::Unmeasured,
        }
    }
}

/// The radial vector of `p` from the cylinder's axis.
fn radial(c: &Cylinder, p: Vec3) -> Vec3 {
    let d = p - c.point;
    d - c.axis * c.axis.dot(d)
}

/// A position on a 1e-6 mm grid: the key that welds a tessellation's duplicated seam vertices.
fn key(p: Vec3) -> (i64, i64, i64) {
    let q = |x: f64| libm::round(x * 1e6) as i64;
    (q(p.x), q(p.y), q(p.z))
}

/// The boundary of design face `face`: the edges of its triangles that one of them alone uses (by position).
fn boundary(design: &Design, face: u32) -> Vec<(Vec3, Vec3)> {
    let mut uses: BTreeMap<((i64, i64, i64), (i64, i64, i64)), (u32, Vec3, Vec3)> = BTreeMap::new();
    for (t, _) in design.face_of.iter().enumerate().filter(|(_, f)| **f == face) {
        let Some([a, b, c]) = design.mesh.corners(t) else { continue };
        for (u, v) in [(a, b), (b, c), (c, a)] {
            let (ku, kv) = (key(u), key(v));
            if ku == kv {
                continue;
            }
            let k = if ku < kv { (ku, kv) } else { (kv, ku) };
            uses.entry(k).or_insert((0, u, v)).0 += 1;
        }
    }
    uses.into_values().filter(|e| e.0 == 1).map(|e| (e.1, e.2)).collect()
}

/// The distance from `p` to the segment `ab`.
fn to_segment(p: Vec3, a: Vec3, b: Vec3) -> f64 {
    let ab = b - a;
    let l2 = ab.mag_sq();
    let t = if l2 > 0.0 { (ab.dot(p - a) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p - (a + ab * t)).mag()
}

/// max − min of the values, `None` when there are none.
fn spread(vals: impl Iterator<Item = f64>) -> Option<f64> {
    let (lo, hi) = vals.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
    if lo <= hi { Some(hi - lo) } else { None }
}

/// Design face `face`'s surface from its own triangles (see the module doc).
pub fn nominal(design: &Design, face: u32, tol: Tol) -> Nominal {
    let normals = design.mesh.normals_face(tol);
    let tris: Vec<usize> = design.face_of.iter().enumerate().filter(|(_, f)| **f == face).map(|(t, _)| t).collect();
    let mut mean = Vec3::ZERO;
    let (mut corners, mut centroids, mut tn) = (Vec::new(), Vec::new(), Vec::new());
    for &t in &tris {
        let (Some([a, b, c]), Some(&n)) = (design.mesh.corners(t), normals.get(t)) else { continue };
        if n.mag_sq() == 0.0 {
            continue;
        }
        mean += n * ((b - a).cross(c - a).mag() * 0.5);
        corners.extend_from_slice(&[a, b, c]);
        centroids.push((a + b + c) * (1.0 / 3.0));
        tn.push(n);
    }
    let Ok(mean) = mean.normalize() else { return Nominal::Other };
    if tn.is_empty() {
        return Nominal::Other;
    }
    let flat = libm::cos(FLAT_DEG.to_radians());
    if tn.iter().all(|n| n.dot(mean) >= flat) {
        return match fit_plane(&corners, Tol { lin: DESIGN_CHORD_MM, ang: tol.ang }) {
            Ok(f) if f.outliers == 0 => {
                let pl = f.shape;
                Nominal::Plane(if pl.normal.dot(mean) < 0.0 { Plane { normal: -pl.normal, d: -pl.d } } else { pl })
            }
            _ => Nominal::Other,
        };
    }
    match fit_cylinder(&centroids, &tn, Tol { lin: 2.0 * DESIGN_CHORD_MM, ang: 10.0f64.to_radians() }) {
        Ok(f) if f.outliers == 0 => {
            // The tessellation's vertices lie on the surface; the centroids sit inside it by the chord's sagitta.
            let c = f.shape;
            let r = corners.iter().map(|&p| radial(&c, p).mag()).sum::<f64>() / corners.len() as f64;
            Nominal::Cylinder(Cylinder { radius: r, ..c })
        }
        _ => Nominal::Other,
    }
}

/// Twice the larger distance of the measured axis from the nominal axis at the feature's two ends (`extent`, along the
/// nominal axis from its point): the diameter of the zone the axis lies in. Infinite for an axis across the nominal.
pub fn position_mm(measured: &Cylinder, nominal: &Cylinder, extent: (f64, f64)) -> f64 {
    let (an, am) = (nominal.axis, measured.axis);
    let cos = an.dot(am);
    if !(cos.abs() > 1e-9) {
        return f64::INFINITY;
    }
    let at = |t: f64| {
        let c = nominal.point + an * t;
        // The measured axis where it crosses the plane through `c` across the nominal axis.
        let s = an.dot(c - measured.point) / cos;
        (measured.point + am * s - c).mag()
    };
    2.0 * at(extent.0).max(at(extent.1))
}

/// Every design face read as tolerances, in face-index order, with the edge margin `margin_mm` (the TSDF's
/// truncation, `Tsdf::trunc_mm`; see the module doc).
pub fn gdt(scan: &ScanMesh, design: &Design, dev: &Deviation, margin_mm: f64, tol: Tol) -> Vec<FaceGdt> {
    (0..design.faces)
        .map(|face| {
            let nominal = nominal(design, face, tol);
            let mut out = FaceGdt {
                face,
                nominal,
                n: 0,
                matched: 0,
                flatness_mm: None,
                cylindricity_mm: None,
                position_mm: None,
                band_mm: None,
                profile_mm: None,
                span_mm: None,
                profile_in_frame_mm: None,
                frame_span_mm: None,
                axis: None,
                extent: (0.0, 0.0),
                minzone_flatness_mm: None,
                minzone_cylindricity_mm: None,
            };
            out.matched = dev.face_of_vertex.iter().zip(&dev.per_vertex).filter(|(f, d)| **f == Some(face) && d.is_some()).count() as u32;
            if nominal == Nominal::Other {
                return out;
            }
            let (pts, nrm, dev_mm) = form(scan, design, dev, face, nominal, margin_mm); // VUGSCANGDTDATUMS (SR552): the loop moved into form(), shared with the datum fits
            out.n = pts.len() as u32;
            if out.n < GDT_MIN_POINTS {
                return out;
            }
            out.band_mm = spread(dev_mm.iter().copied()); out.profile_mm = dev_mm.iter().map(|d| 2.0 * d.abs()).reduce(f64::max); out.span_mm = span(dev_mm.iter().copied()); // VUGSCANGDTPROFILEDATUM (SR567): the signed span; VUGSCANGDTSPEC (SR559): the zone centred on the nominal
            match nominal {
                Nominal::Plane(_) => {
                    if let Ok(f) = fit_plane(&pts, FIT_BAND) {
                        out.flatness_mm = spread(pts.iter().map(|&p| f.shape.signed_distance(p)));
                        out.minzone_flatness_mm = Some(flatness_minzone(&pts, &f.shape, tol));
                    }
                }
                Nominal::Cylinder(nc) => {
                    let ts = pts.iter().map(|&p| nc.axis.dot(p - nc.point));
                    let (lo, hi) = ts.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), t| (lo.min(t), hi.max(t)));
                    out.extent = (lo, hi);
                    if let Ok(f) = fit_cylinder(&pts, &nrm, FIT_BAND) {
                        let c = f.shape;
                        out.cylindricity_mm = spread(pts.iter().map(|&p| radial(&c, p).mag() - c.radius));
                        out.position_mm = Some(position_mm(&c, &nc, out.extent));
                        out.axis = Some(c);
                        out.minzone_cylindricity_mm = Some(cylindricity_minzone(&pts, &c, tol));
                    }
                }
                Nominal::Other => {}
            }
            out
        })
        .collect()
}

// ---- VUGSCANGDTMINZONE (LEDGER SR554): the ISO 1101 minimum zone under the least-squares band ----
//
// ISO 1101's form zone is the MINIMUM zone (Chebyshev): the narrowest two parallel planes / two coaxial cylinders
// holding every form vertex, never wider than the least-squares band above. Both forms are one problem: the residual
// v_i - phi_i . p of a linear model whose band (max - min) is minimised over p, the discrete Chebyshev approximation.
// [`chebyshev_band`] solves it as the dual of the LP  min w  s.t.  c <= v_i - phi_i . p <= c + w,  whose k + 2 rows
// make the simplex basis the contact set (3 + 1 or 2 + 2 for a plane) and each pivot the exchange step (Stiefel).
// Flatness is that LP in a frame about a reference normal, re-framed about the answer; cylindricity is that LP over
// the linearised radial distance about the current axis, a step kept only when the EXACT radial band narrows
// (successive LP, monotone from the least-squares axis). Pure, deterministic; `ls` names the fallback.

use alloc::string::String;

/// The most simplex pivots one `chebyshev_band` takes; past it the search has not converged.
pub const MINZONE_PIVOTS: u32 = 4096;
/// The most axis steps [`cylindricity_minzone`] takes before it is called not converged.
pub const MINZONE_ROUNDS: u32 = 12;
/// The most re-framings [`flatness_minzone`] takes.
const MINZONE_FRAMES: u32 = 4;
/// The most halvings of a cylinder step that did not narrow the band.
const MINZONE_HALVINGS: u32 = 4;

/// A minimum-zone value: `mm` the zone's width; `ls` when the search did not converge and `mm` IS the least-squares
/// band (printed `<mm>|ls`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MinZone {
    pub mm: f64,
    pub ls: bool,
}

impl core::fmt::Display for MinZone {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.ls { write!(f, "{:.3}|ls", self.mm) } else { write!(f, "{:.3}", self.mm) }
    }
}

impl FaceGdt {
    /// The `gdt:` line's min-zone token: ` minzone_flatness=<mm>` for a plane, ` minzone_cylindricity=<mm>` for a
    /// cylinder (`|ls` on the fallback), empty when unmeasured.
    pub fn minzone_tokens(&self) -> String {
        match (self.nominal, self.minzone_flatness_mm, self.minzone_cylindricity_mm) {
            (Nominal::Plane(_), Some(m), _) => alloc::format!(" minzone_flatness={m}"),
            (Nominal::Cylinder(_), _, Some(m)) => alloc::format!(" minzone_cylindricity={m}"),
            _ => String::new(),
        }
    }
}

/// One simplex pivot of the `m`-row tableau `t` (row stride `w`, the last entry of a row its right-hand side) and its
/// objective row `obj` on row `r`, column `j`.
fn mz_pivot(t: &mut [f64], obj: &mut [f64], w: usize, m: usize, r: usize, j: usize) {
    let piv = t[r * w + j];
    for x in &mut t[r * w..(r + 1) * w] {
        *x /= piv;
    }
    for q in 0..m {
        let f = t[q * w + j];
        if q != r && f != 0.0 {
            for c in 0..w {
                t[q * w + c] -= f * t[r * w + c];
            }
        }
    }
    let f = obj[j];
    if f != 0.0 {
        for c in 0..w {
            obj[c] -= f * t[r * w + c];
        }
    }
}

/// Maximise over the tableau: enter a column below `limit` with a reduced cost under `-tol.lin` (the most negative;
/// Bland's first after a degenerate pivot), leave by the ratio test (ties to the smallest basic index). `false` when
/// unbounded or past [`MINZONE_PIVOTS`].
fn mz_run(t: &mut [f64], obj: &mut [f64], basis: &mut [usize], w: usize, limit: usize, tol: Tol, pivots: &mut u32) -> bool {
    let m = basis.len();
    let mut bland = false;
    loop {
        let mut enter: Option<usize> = None;
        for j in 0..limit {
            if obj[j] < -tol.lin && (enter.is_none() || (!bland && obj[j] < obj[enter.unwrap_or(j)])) {
                enter = Some(j);
                if bland {
                    break;
                }
            }
        }
        let Some(j) = enter else { return true };
        let mut leave: Option<(usize, f64)> = None;
        for r in 0..m {
            let a = t[r * w + j];
            if a > tol.ang {
                let ratio = t[r * w + w - 1].max(0.0) / a;
                let better = match leave {
                    None => true,
                    Some((lr, lb)) => ratio < lb || (ratio == lb && basis[r] < basis[lr]),
                };
                if better {
                    leave = Some((r, ratio));
                }
            }
        }
        let Some((r, ratio)) = leave else { return false };
        if *pivots >= MINZONE_PIVOTS {
            return false;
        }
        *pivots += 1;
        if ratio <= 0.0 {
            bland = true;
        }
        mz_pivot(t, obj, w, m, r, j);
        basis[r] = j;
    }
}

/// The `p` (length `k`) minimising max − min of `v_i − phi_i · p` (`phi` row-major n × k, entries normalised to
/// about [-1, 1]), by the dual LP  max Σ v_i (ν_i − μ_i)  s.t.  Σ (ν_i − μ_i) phi_i = 0, Σ (ν_i − μ_i) = 0, Σ ν_i = 1,
/// ν, μ ≥ 0: two phases over k + 2 artificials, the primal read from the reduced costs under the artificial columns
/// (y = c_B B⁻¹: p, then the zone's lower edge, then its width). Zeros: a reduced cost (mm) against `tol.lin`, a pivot
/// entry (dimensionless) against `tol.ang`. `None` past [`MINZONE_PIVOTS`] or on a short slice.
fn chebyshev_band(phi: &[f64], v: &[f64], k: usize, tol: Tol) -> Option<Vec<f64>> {
    let n = v.len();
    if n == 0 || phi.len() < n * k {
        return None;
    }
    let (m, ns) = (k + 2, 2 * n);
    let cols = ns + m;
    let w = cols + 1;
    let mut t = alloc::vec![0.0f64; m * w];
    for i in 0..n {
        for r in 0..k {
            t[r * w + i] = phi[i * k + r];
            t[r * w + n + i] = -phi[i * k + r];
        }
        t[k * w + i] = 1.0;
        t[k * w + n + i] = -1.0;
        t[(k + 1) * w + i] = 1.0;
    }
    for r in 0..m {
        t[r * w + ns + r] = 1.0;
    }
    t[(k + 1) * w + w - 1] = 1.0;
    let mut basis: Vec<usize> = (ns..cols).collect();
    let mut pivots = 0u32;
    // Phase 1: maximise −Σ artificials.
    let mut obj = alloc::vec![0.0f64; w];
    for r in 0..m {
        for c in 0..w {
            if c < ns || c == w - 1 {
                obj[c] -= t[r * w + c];
            }
        }
    }
    if !mz_run(&mut t, &mut obj, &mut basis, w, ns, tol, &mut pivots) || obj[w - 1] < -tol.lin {
        return None;
    }
    // An artificial left basic at zero leaves on any structural column; none and the row is redundant.
    for r in 0..m {
        if basis[r] >= ns {
            if let Some(j) = (0..ns).find(|&j| t[r * w + j].abs() > tol.ang) {
                mz_pivot(&mut t, &mut obj, w, m, r, j);
                basis[r] = j;
            }
        }
    }
    // Phase 2: the dual objective; artificials never re-enter.
    let cost = |j: usize| if j < n { v[j] } else if j < ns { -v[j - n] } else { 0.0 };
    for c in 0..w {
        let cj = if c < cols { cost(c) } else { 0.0 };
        obj[c] = (0..m).map(|r| cost(basis[r]) * t[r * w + c]).sum::<f64>() - cj;
    }
    if !mz_run(&mut t, &mut obj, &mut basis, w, ns, tol, &mut pivots) {
        return None;
    }
    Some((0..k).map(|r| obj[ns + r]).collect())
}

/// ISO 1101 minimum-zone flatness of `points` from the least-squares plane `ls`: the band LP over (x, y) in the frame
/// of the current normal (x, y over the half extent), the normal tilted by its answer, re-framed while the exact
/// perpendicular band narrows by `tol.lin` or more, at most [`MINZONE_FRAMES`] times; never above the least-squares
/// band (`spread` of the signed distance to `ls`, the `flatness_mm` expression), which is the `ls` fallback when the
/// first LP refuses or the points are too few or flat in extent.
pub fn flatness_minzone(points: &[Vec3], ls: &Plane, tol: Tol) -> MinZone {
    let ls_mm = spread(points.iter().map(|&p| ls.signed_distance(p))).unwrap_or(0.0);
    let fallback = MinZone { mm: ls_mm, ls: true };
    if points.len() < 4 {
        return fallback;
    }
    let Ok(mut n) = ls.normal.normalize() else { return fallback };
    let c = points.iter().fold(Vec3::ZERO, |s, &p| s + p) * (1.0 / points.len() as f64);
    let mut best = ls_mm;
    for round in 0..MINZONE_FRAMES {
        let Ok(u) = n.any_perpendicular() else { return fallback };
        let v = n.cross(u);
        let l = points.iter().map(|&p| u.dot(p - c).abs().max(v.dot(p - c).abs())).fold(0.0f64, f64::max);
        if !(l > tol.lin) {
            return fallback;
        }
        let (mut phi, mut z) = (Vec::with_capacity(2 * points.len()), Vec::with_capacity(points.len()));
        for &p in points {
            let d = p - c;
            phi.push(u.dot(d) / l);
            phi.push(v.dot(d) / l);
            z.push(n.dot(d));
        }
        let Some(ab) = chebyshev_band(&phi, &z, 2, tol) else {
            if round == 0 {
                return fallback;
            }
            break;
        };
        let Ok(n2) = (n - u * (ab[0] / l) - v * (ab[1] / l)).normalize() else { break };
        let w = spread(points.iter().map(|&p| n2.dot(p))).unwrap_or(f64::INFINITY);
        if !(w < best - tol.lin) {
            best = best.min(w);
            break;
        }
        best = w;
        n = n2;
    }
    MinZone { mm: best.min(ls_mm), ls: false }
}

/// ISO 1101 minimum-zone cylindricity of `points` from the least-squares cylinder `ls`: about the current axis (frame
/// u, v, a at the points' mean along it, H the half extent), the band LP over phi = (cos, sin, z/H cos, z/H sin) with
/// value the radial distance gives the axis step (dx, dy, tx, ty); the step, or its half down to [`MINZONE_HALVINGS`]
/// halvings, is kept only when the EXACT radial band narrows. Converged when a kept step narrows it by under `tol.lin`
/// or no step narrows it; not converged within [`MINZONE_ROUNDS`], an LP refusing, or too few points: the least-squares
/// band (the `cylindricity_mm` expression) with `ls`.
pub fn cylindricity_minzone(points: &[Vec3], ls: &Cylinder, tol: Tol) -> MinZone {
    let band = |c: &Cylinder| spread(points.iter().map(|&p| radial(c, p).mag() - c.radius));
    let ls_mm = band(ls).unwrap_or(0.0);
    let fallback = MinZone { mm: ls_mm, ls: true };
    if points.len() < 5 {
        return fallback;
    }
    let (mut cur, mut best) = (*ls, ls_mm);
    for _ in 0..MINZONE_ROUNDS {
        let Ok(a) = cur.axis.normalize() else { return fallback };
        let Ok(u) = a.any_perpendicular() else { return fallback };
        let Ok(v) = a.cross(u).normalize() else { return fallback };
        let t0 = points.iter().map(|&p| a.dot(p - cur.point)).sum::<f64>() / points.len() as f64;
        let p0 = cur.point + a * t0;
        let h = points.iter().map(|&p| a.dot(p - p0).abs()).fold(0.0f64, f64::max);
        if !(h > tol.lin) {
            return fallback;
        }
        let (mut phi, mut rho) = (Vec::with_capacity(4 * points.len()), Vec::with_capacity(points.len()));
        for &p in points {
            let d = p - p0;
            let (x, y, z) = (u.dot(d), v.dot(d), a.dot(d) / h);
            let r = libm::sqrt(x * x + y * y);
            if r <= tol.lin {
                continue;
            }
            let (cx, sy) = (x / r, y / r);
            phi.extend_from_slice(&[cx, sy, z * cx, z * sy]);
            rho.push(r);
        }
        if rho.len() < 5 {
            return fallback;
        }
        let Some(s) = chebyshev_band(&phi, &rho, 4, tol) else { return fallback };
        let mut kept = None;
        let mut f = 1.0;
        for _ in 0..=MINZONE_HALVINGS {
            let axis = (a + u * (f * s[2] / h) + v * (f * s[3] / h)).normalize();
            if let Ok(axis) = axis {
                let cand = Cylinder { point: p0 + u * (f * s[0]) + v * (f * s[1]), axis, radius: cur.radius };
                if let Some(w) = band(&cand) {
                    if w < best {
                        kept = Some((cand, w));
                        break;
                    }
                }
            }
            f *= 0.5;
        }
        let Some((cand, w)) = kept else { return MinZone { mm: best, ls: false } };
        let gain = best - w;
        cur = cand;
        best = w;
        if gain < tol.lin {
            return MinZone { mm: best, ls: false };
        }
    }
    fallback
}

// ---- VUGSCANGDTDATUMS (LEDGER SR552): the ISO 5459 datum frame A|B|C, position read in it ----

use crate::geom::{Transform, from_axes};

/// The form vertices of design face `face` against its nominal (the module doc's item 2): their positions and TSDF
/// normals at the aligned pose and their signed deviations. The one selection `gdt` and [`frame_of`] read.
fn form(scan: &ScanMesh, design: &Design, dev: &Deviation, face: u32, nominal: Nominal, margin_mm: f64) -> (Vec<Vec3>, Vec<Vec3>, Vec<f64>) {
    let cos = libm::cos(GDT_NORMAL_DEG.to_radians());
    let pose = dev.aligned.pose;
    let edges = boundary(design, face);
    let (mut pts, mut nrm, mut dev_mm) = (Vec::new(), Vec::new(), Vec::new());
    for (i, f) in dev.face_of_vertex.iter().enumerate() {
        if *f != Some(face) {
            continue;
        }
        let (Some(&p), Some(Some(d))) = (scan.mesh.positions.get(i), dev.per_vertex.get(i)) else { continue };
        let q = pose.apply_point(p);
        let n = pose.apply_vector(scan.normals.get(i).copied().unwrap_or(Vec3::ZERO));
        let along = match nominal {
            Nominal::Plane(pl) => pl.normal,
            Nominal::Cylinder(c) => radial(&c, q).normalize().unwrap_or(Vec3::ZERO),
            Nominal::Other => Vec3::ZERO,
        };
        if n.dot(along).abs() < cos * n.mag() || n.mag_sq() == 0.0 {
            continue;
        }
        let on = match nominal {
            Nominal::Plane(pl) => pl.project(q),
            Nominal::Cylinder(c) => q - radial(&c, q) + along * c.radius,
            Nominal::Other => q,
        };
        if edges.iter().any(|&(a, b)| to_segment(on, a, b) < margin_mm) {
            continue;
        }
        pts.push(q);
        nrm.push(n);
        dev_mm.push(*d);
    }
    (pts, nrm, dev_mm)
}

/// The datum labels in precedence order: primary, secondary, tertiary.
pub const DATUM_LABELS: [char; 3] = ['A', 'B', 'C'];
/// A secondary or tertiary datum within this of the direction it must cross is degenerate; a cylinder datum within
/// this of A's normal stands along it (degrees).
const DATUM_DEG: f64 = 10.0;

/// Why a datum frame was refused, by the datum's label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DatumError {
    /// The face index is past the design's faces.
    NoFace { label: char, face: u32 },
    /// The face's surface cannot stand at this precedence (Other; A not a plane; a cylinder across A).
    DatumSurface { label: char, face: u32, surface: &'static str },
    /// Fewer than [`GDT_MIN_POINTS`] form vertices stood on the face, or its fit was refused; `n` the kept count.
    DatumFit { label: char, face: u32, n: u32 },
    /// The datum does not cross the frame the earlier datums left (parallel to them).
    Degenerate { label: char },
}

impl DatumError {
    pub fn label(&self) -> char {
        match *self {
            DatumError::NoFace { label, .. } | DatumError::DatumSurface { label, .. } | DatumError::DatumFit { label, .. } | DatumError::Degenerate { label } => label,
        }
    }
}

/// A datum reference frame (ISO 5459) as measured on the scan and as designed: each frame-local -> design
/// coordinates, z along A's outward normal, x from B (or C), the origin where the datums meet. `residual_mm` is the
/// largest rms of the three datum fits.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DatumFrame {
    pub measured: Transform,
    pub nominal: Transform,
    pub residual_mm: f64,
}

/// The frame three datum surfaces fix (see [`DatumFrame`]): A a plane takes z and the origin's height; B a plane
/// takes x (its normal across z) or a cylinder along A the origin; C completes the origin (a plane: the A∩B∩C point;
/// a cylinder along A: its axis's coordinate along y) or, after a cylinder B, takes x.
fn axes(s: [Nominal; 3], faces: [u32; 3]) -> Result<Transform, DatumError> {
    let surface = |k: usize| DatumError::DatumSurface { label: DATUM_LABELS[k], face: faces[k], surface: s[k].name() };
    let Nominal::Plane(a) = s[0] else { return Err(surface(0)) };
    let z = a.normal;
    let (along, cross) = (libm::cos(DATUM_DEG.to_radians()), libm::sin(DATUM_DEG.to_radians()));
    // A vector flattened across z, refused within DATUM_DEG of it.
    let flat = |v: Vec3, k: usize| {
        let f = v - z * z.dot(v);
        if f.mag() < cross * v.mag() { Err(DatumError::Degenerate { label: DATUM_LABELS[k] }) } else { f.normalize().map_err(|_| DatumError::Degenerate { label: DATUM_LABELS[k] }) }
    };
    // Where a cylinder datum standing along A meets A's plane.
    let foot = |c: &Cylinder, k: usize| {
        let cz = z.dot(c.axis);
        if cz.abs() < along { Err(surface(k)) } else { Ok(c.point + c.axis * ((a.d - z.dot(c.point)) / cz)) }
    };
    // The point on three planes (n_i . o = d_i), Cramer by triple products; refused as degenerate at label C.
    let meet = |p: [(Vec3, f64); 3]| {
        let det = p[0].0.dot(p[1].0.cross(p[2].0));
        if det.abs() < cross * cross {
            return Err(DatumError::Degenerate { label: DATUM_LABELS[2] });
        }
        Ok((p[1].0.cross(p[2].0) * p[0].1 + p[2].0.cross(p[0].0) * p[1].1 + p[0].0.cross(p[1].0) * p[2].1) * (1.0 / det))
    };
    let (x, origin) = match (s[1], s[2]) {
        (Nominal::Plane(b), Nominal::Plane(c)) => (flat(b.normal, 1)?, meet([(z, a.d), (b.normal, b.d), (c.normal, c.d)])?),
        (Nominal::Plane(b), Nominal::Cylinder(c)) => {
            let x = flat(b.normal, 1)?;
            let y = z.cross(x);
            let _ = foot(&c, 2)?;
            (x, meet([(z, a.d), (b.normal, b.d), (y, y.dot(c.point))])?)
        }
        (Nominal::Cylinder(b), Nominal::Plane(c)) => (flat(c.normal, 2)?, foot(&b, 1)?),
        (Nominal::Cylinder(b), Nominal::Cylinder(c)) => {
            let o = foot(&b, 1)?;
            (flat(foot(&c, 2)? - o, 2)?, o)
        }
        (Nominal::Other, _) => return Err(surface(1)),
        (_, Nominal::Other) => return Err(surface(2)),
    };
    Ok(from_axes(x, z.cross(x), z, origin))
}

/// The datum frame A|B|C on design faces `datums` (ISO 5459): the nominal frame from each face's [`nominal`], the
/// measured frame from `fit_plane` / `fit_cylinder` through each face's [`form`] vertices at the aligned pose (the
/// fitted normal or axis turned to agree with its nominal's), with the edge margin `margin_mm` as [`gdt`] takes it.
/// A face past the design, a surface that cannot stand there, or a fit that fails is refused by its label.
pub fn frame_of(scan: &ScanMesh, design: &Design, dev: &Deviation, datums: [u32; 3], margin_mm: f64, tol: Tol) -> Result<DatumFrame, DatumError> {
    let (mut nom, mut meas, mut residual_mm) = ([Nominal::Other; 3], [Nominal::Other; 3], 0.0f64);
    for (k, &face) in datums.iter().enumerate() {
        let label = DATUM_LABELS[k];
        if face >= design.faces {
            return Err(DatumError::NoFace { label, face });
        }
        let n = nominal(design, face, tol);
        if n == Nominal::Other {
            return Err(DatumError::DatumSurface { label, face, surface: n.name() });
        }
        let (pts, nrm, _) = form(scan, design, dev, face, n, margin_mm);
        let fail = DatumError::DatumFit { label, face, n: pts.len() as u32 };
        if (pts.len() as u32) < GDT_MIN_POINTS {
            return Err(fail);
        }
        meas[k] = match n {
            Nominal::Plane(np) => {
                let f = fit_plane(&pts, FIT_BAND).map_err(|_| fail)?;
                residual_mm = residual_mm.max(f.rms_mm);
                let pl = f.shape;
                Nominal::Plane(if pl.normal.dot(np.normal) < 0.0 { Plane { normal: -pl.normal, d: -pl.d } } else { pl })
            }
            Nominal::Cylinder(nc) => {
                let f = fit_cylinder(&pts, &nrm, FIT_BAND).map_err(|_| fail)?;
                residual_mm = residual_mm.max(f.rms_mm);
                let c = f.shape;
                Nominal::Cylinder(if c.axis.dot(nc.axis) < 0.0 { Cylinder { axis: -c.axis, ..c } } else { c })
            }
            Nominal::Other => return Err(DatumError::DatumSurface { label, face, surface: n.name() }),
        };
        nom[k] = n;
    }
    Ok(DatumFrame { measured: axes(meas, datums)?, nominal: axes(nom, datums)?, residual_mm })
}

/// [`position_mm`] read in the datum frame: the measured axis expressed in the measured frame against the nominal axis
/// in the nominal frame, so an error of the global registration (which moves the measured feature and the measured
/// datums together) does not leak in. `extent` as [`FaceGdt::extent`]. Infinite when a frame does not invert.
pub fn position_in(frame: &DatumFrame, measured: &Cylinder, nominal: &Cylinder, extent: (f64, f64)) -> f64 {
    let (Ok(m), Ok(n)) = (frame.measured.invert(), frame.nominal.invert()) else { return f64::INFINITY };
    let local = |t: Transform, c: &Cylinder| Cylinder { point: t.apply_point(c.point), axis: t.apply_vector(c.axis), ..*c };
    position_mm(&local(m, measured), &local(n, nominal), extent)
}

// ---- VUGSCANGDTSPEC (LEDGER SR559): the `<face>=<kind>:<tol>` callout parsed once ----
//
// VUGSCANGDT (SR542, tools/scan `--gdt`) and VUGFACEGDT (SR553, vug-view's `gdt` command) each split the callout
// themselves; [`Callout::parse`] is the one grammar both call: the face before the LAST `=` (a face name carries `:` and
// may carry `=`), the kind ([`Kind::parse`]) and the tolerance either side of the LAST `:` after it, the tolerance a
// finite number of millimetres above 0. Each refusal is named; the callers keep their own face lookup and refusals.

/// A tolerance named on a design face by its name: `<face>=<kind>:<tol>`.
#[derive(Clone, Debug, PartialEq)]
pub struct Callout {
    pub face: alloc::string::String,
    pub kind: Kind,
    pub tol_mm: f64,
    /// VUGSCANGDTPROFILEDATUM (SR567): the unequal zone's offset (`@<off>`, profile only); None the equal zone.
    pub off_mm: Option<f64>,
}

/// Why a callout did not parse.
#[derive(Clone, Debug, PartialEq)]
pub enum GdtSpecError {
    /// No `=`, or nothing before the last one.
    GdtFace,
    /// No `:` after the last `=`, or a kind [`Kind::parse`] does not read.
    GdtKind { kind: alloc::string::String },
    /// The tolerance is not a number, not finite, or not above 0.
    BadTol { tol: alloc::string::String },
    /// VUGSCANGDTPROFILEDATUM (SR567): an `@<off>` on a kind other than profile, or an offset that is not a finite number.
    BadUz { uz: alloc::string::String },
}

impl Callout {
    /// `<face>=<kind>:<tol>` (see the section comment).
    pub fn parse(s: &str) -> Result<Callout, GdtSpecError> {
        let (face, rest) = s.rsplit_once('=').ok_or(GdtSpecError::GdtFace)?;
        if face.is_empty() {
            return Err(GdtSpecError::GdtFace);
        }
        let (kind, tol) = rest.rsplit_once(':').ok_or_else(|| GdtSpecError::GdtKind { kind: rest.into() })?;
        let kind = Kind::parse(kind).ok_or_else(|| GdtSpecError::GdtKind { kind: kind.into() })?;
        let (tol, off_mm) = uz_split(kind, tol)?; // VUGSCANGDTPROFILEDATUM (SR567): `<tol>@<off>`
        let tol_mm = match tol.parse::<f64>() {
            Ok(t) if t.is_finite() && t > 0.0 => t,
            _ => return Err(GdtSpecError::BadTol { tol: tol.into() }),
        };
        Ok(Callout { face: face.into(), kind, tol_mm, off_mm })
    }
}

impl core::fmt::Display for Callout {
    /// `<face>=<kind>:<tol>`, which [`Callout::parse`] reads back to the same callout.
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}={}:{}", self.face, self.kind.name(), self.tol_mm)?;
        if let Some(off) = self.off_mm { write!(f, "@{off}")?; } // VUGSCANGDTPROFILEDATUM (SR567): the unequal zone reads back
        Ok(())
    }
}

// ---- VUGFACEGDTREMEASURE (LEDGER SR560): the frame applied to a whole reading, one fn for tools/scan and the face ----

/// Every cylinder face of `faces` (as [`gdt`] read them) with its `position_mm` re-read in `frame` through
/// [`position_in`] (its fitted axis against its nominal over its extent); planes and unmeasured faces unchanged. The one
/// place a datum frame reaches a reading: tools/scan's `--datum` and vug-view's `datum` command both call it.
///
/// VUGSCANGDTPROFILEDATUM (SR567): each plane and cylinder face measured on at least [`GDT_MIN_POINTS`] form vertices
/// also takes its profile located in the frame ([`profile_in`] against its own nominal): `frame_span_mm` and
/// `profile_in_frame_mm`; `profile_mm` (the best fit) is left as it is.
pub fn in_frame(scan: &ScanMesh, design: &Design, dev: &Deviation, margin_mm: f64, faces: &mut [FaceGdt], frame: &DatumFrame) {
    for f in faces.iter_mut() {
        if let (Nominal::Cylinder(nc), Some(axis)) = (f.nominal, f.axis) {
            f.position_mm = Some(position_in(frame, &axis, &nc, f.extent));
        }
        if f.n >= GDT_MIN_POINTS && matches!(f.nominal, Nominal::Plane(_) | Nominal::Cylinder(_)) {
            f.frame_span_mm = profile_in(frame, scan, design, dev, f.face, f.nominal, margin_mm);
            f.profile_in_frame_mm = f.frame_span_mm.map(|s| zone_mm(s, 0.0));
        }
    }
}

// ---- VUGSCANGDTPROFILEDATUM (LEDGER SR567): profile located in the datum frame; the unequal zone ----
//
// ISO 1101 profile with datums: the nominal is placed by the datum frame, not by the global registration, so a part
// whose datum faces are true and whose profiled face is shifted reads the shift. Each form vertex q (design
// coordinates at the aligned pose, [`form`]) goes through the measured frame into the nominal frame,
// q' = N ∘ M⁻¹ (q) as [`position_in`] reads an axis, and d' is the nominal's own signed distance there (a plane's along
// its outward normal; a cylinder's radial residual, signed by the material side its design triangles face), positive
// where the face stands proud like deviation.rs's. The unequal zone `<face>=profile:<tol>@<off>` is the zone
// [off − tol/2, off + tol/2]: its value is [`zone_mm`] about `off`, which passes at or under `tol`; off 0 is the equal zone.

/// The signed (min, max) of the values, `None` when there are none.
fn span(vals: impl Iterator<Item = f64>) -> Option<(f64, f64)> {
    let (lo, hi) = vals.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
    if lo <= hi { Some((lo, hi)) } else { None }
}

/// The tolerance text and the unequal zone's offset: `<tol>@<off>` (profile only, the offset any finite number).
fn uz_split(kind: Kind, tol: &str) -> Result<(&str, Option<f64>), GdtSpecError> {
    let Some((t, off)) = tol.split_once('@') else { return Ok((tol, None)) };
    match off.parse::<f64>() {
        Ok(o) if o.is_finite() && kind == Kind::Profile => Ok((t, Some(o))),
        _ => Err(GdtSpecError::BadUz { uz: off.into() }),
    }
}

/// The width of the zone centred on `off` that holds the signed span `(lo, hi)`: 2 · max(hi − off, off − lo).
pub fn zone_mm(span: (f64, f64), off: f64) -> f64 {
    2.0 * (span.1 - off).max(off - span.0)
}

/// +1 when cylinder `c` of design face `face` is a boss (its outward triangles face away from the axis), −1 a hole.
fn material_side(design: &Design, face: u32, c: &Cylinder) -> f64 {
    let s: f64 = design
        .face_of
        .iter()
        .enumerate()
        .filter(|(_, f)| **f == face)
        .filter_map(|(t, _)| design.mesh.corners(t))
        .map(|[a, b, k]| (b - a).cross(k - a).dot(radial(c, (a + b + k) * (1.0 / 3.0))))
        .sum();
    if s < 0.0 { -1.0 } else { 1.0 }
}

/// The signed span (min, max) of design face `face`'s form vertices ([`form`] against `placed`, at the edge margin
/// `margin_mm`) located in `frame` (see the section comment), `placed` the nominal surface in design coordinates (a
/// plane's normal outward). None for Other, under [`GDT_MIN_POINTS`] form vertices, or when the measured frame does
/// not invert.
pub fn profile_in(frame: &DatumFrame, scan: &ScanMesh, design: &Design, dev: &Deviation, face: u32, placed: Nominal, margin_mm: f64) -> Option<(f64, f64)> {
    let to = frame.nominal.compose(frame.measured.invert().ok()?);
    let (pts, _, _) = form(scan, design, dev, face, placed, margin_mm);
    if (pts.len() as u32) < GDT_MIN_POINTS {
        return None;
    }
    match placed {
        Nominal::Plane(pl) => span(pts.iter().map(|&q| pl.signed_distance(to.apply_point(q)))),
        Nominal::Cylinder(c) => {
            let side = material_side(design, face, &c);
            span(pts.iter().map(|&q| side * (radial(&c, to.apply_point(q)).mag() - c.radius)))
        }
        Nominal::Other => None,
    }
}

impl Callout {
    /// The callout's verdict on its face: the equal zone is [`FaceGdt::verdict`]; an unequal zone (`off_mm`) reads
    /// [`zone_mm`] about the offset over the frame's span when a frame was given, else the best-fit span.
    pub fn verdict(&self, f: &FaceGdt) -> Verdict {
        let Some(off) = self.off_mm else { return f.verdict(self.kind, self.tol_mm) };
        if !f.accepts(Kind::Profile) {
            return Verdict::Unmeasured;
        }
        match f.frame_span_mm.or(f.span_mm) {
            Some(s) if zone_mm(s, off) <= self.tol_mm => Verdict::Pass { value_mm: zone_mm(s, off), tol_mm: self.tol_mm },
            Some(s) => Verdict::Fail { value_mm: zone_mm(s, off), tol_mm: self.tol_mm },
            None => Verdict::Unmeasured,
        }
    }
}
