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
//!    rounded edges pair with a face through deviation.rs's 60° gate and are a blend, not the face's form. Fewer than
//!    [`GDT_MIN_POINTS`] and the face is unmeasured.
//! 3. Flatness (ISO 1101, two parallel planes of free orientation): max − min of the signed distance of the form
//!    vertices to the plane `fit_plane` gives through them. Cylindricity (two coaxial cylinders, axis and radius free):
//!    max − min of the radial residual about the cylinder `fit_cylinder` gives through them. The reference is least
//!    squares after the RANSAC consensus; every form vertex counts in the band, the consensus only places the
//!    reference. The min-zone (Chebyshev) value under it is owed (VUG-SCAN-GDT-MINZONE).
//! 4. Position: the fitted axis against the nominal axis as a diameter zone, [`position_mm`]: twice the larger distance
//!    of the measured axis from the nominal one at the feature's two axial ends. The frame is the design's after the
//!    best-fit registration; a datum frame is owed (VUG-SCAN-GDT-DATUMS).
//! 5. `band_mm`: the nominal-relative band (max − min of the signed deviation over the form vertices), printed beside
//!    the form values and never given a verdict: it carries the face's offset, tilt and radius error (ISO profile).

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
}

impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        match s {
            "flatness" => Some(Kind::Flatness),
            "cylindricity" => Some(Kind::Cylindricity),
            "position" => Some(Kind::Position),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Kind::Flatness => "flatness",
            Kind::Cylindricity => "cylindricity",
            Kind::Position => "position",
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

/// One design face read as tolerances: its nominal, the form vertices measured, each value its surface takes (`None`
/// where it does not, or unmeasured), the nominal-relative band, the fitted axis of a cylinder and the form vertices'
/// extent along the nominal axis (from the nominal's point; `(0, 0)` for a plane).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceGdt {
    pub face: u32,
    pub nominal: Nominal,
    pub n: u32,
    pub flatness_mm: Option<f64>,
    pub cylindricity_mm: Option<f64>,
    pub position_mm: Option<f64>,
    pub band_mm: Option<f64>,
    pub axis: Option<Cylinder>,
    pub extent: (f64, f64),
}

impl FaceGdt {
    /// Whether this face's surface takes the kind: flatness a plane, cylindricity and position a cylinder.
    pub fn accepts(&self, kind: Kind) -> bool {
        matches!((kind, self.nominal), (Kind::Flatness, Nominal::Plane(_)) | (Kind::Cylindricity | Kind::Position, Nominal::Cylinder(_)))
    }

    pub fn value(&self, kind: Kind) -> Option<f64> {
        match kind {
            Kind::Flatness => self.flatness_mm,
            Kind::Cylindricity => self.cylindricity_mm,
            Kind::Position => self.position_mm,
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

/// Every design face read as tolerances, in face-index order (see the module doc).
pub fn gdt(scan: &ScanMesh, design: &Design, dev: &Deviation, tol: Tol) -> Vec<FaceGdt> {
    let cos = libm::cos(GDT_NORMAL_DEG.to_radians());
    let pose = dev.aligned.pose;
    (0..design.faces)
        .map(|face| {
            let nominal = nominal(design, face, tol);
            let mut out = FaceGdt {
                face,
                nominal,
                n: 0,
                flatness_mm: None,
                cylindricity_mm: None,
                position_mm: None,
                band_mm: None,
                axis: None,
                extent: (0.0, 0.0),
            };
            if nominal == Nominal::Other {
                return out;
            }
            // The form vertices: paired with this face, their normal along the nominal's there.
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
                pts.push(q);
                nrm.push(n);
                dev_mm.push(*d);
            }
            out.n = pts.len() as u32;
            if out.n < GDT_MIN_POINTS {
                return out;
            }
            out.band_mm = spread(dev_mm.iter().copied());
            match nominal {
                Nominal::Plane(_) => {
                    if let Ok(f) = fit_plane(&pts, FIT_BAND) {
                        out.flatness_mm = spread(pts.iter().map(|&p| f.shape.signed_distance(p)));
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
                    }
                }
                Nominal::Other => {}
            }
            out
        })
        .collect()
}
