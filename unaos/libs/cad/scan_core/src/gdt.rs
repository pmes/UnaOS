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
//!    reference. The min-zone (Chebyshev) value under it is owed (VUG-SCAN-GDT-MINZONE).
//! 4. Position: the fitted axis against the nominal axis as a diameter zone, [`position_mm`]: twice the larger distance
//!    of the measured axis from the nominal one at the feature's two axial ends. The frame is the design's after the
//!    best-fit registration; the datum frame (ISO 5459, VUGSCANGDTDATUMS SR552) is [`frame_of`] / [`position_in`].
//! 5. `band_mm`: the nominal-relative band (max − min of the signed deviation over the form vertices), printed beside
//!    the form values and never given a verdict: it carries the face's offset, tilt and radius error (ISO profile).

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
                axis: None,
                extent: (0.0, 0.0),
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
