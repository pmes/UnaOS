// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `photo_core` — a photo of a flat part on graph paper to a measured outline and a sketch (VUG-PHOTO, LEDGER SR503).
//!
//! CHARTER: Vug — shared-core. Vug (`handlers/vug`, CODEX §2 3D, "CAD viewing, editing, and CAM/Slicing") owns CAD;
//! this crate is the photo road of docs/dev/USERLAND/VUG.md §7: the paper is the metrology, no model is asked for a
//! number (BP-vug-7: this is the MEASUREMENT a proposal is checked against; it proposes nothing).
//!
//! The pipeline, each step public for the tests, each refusal named ([`Refusal`]):
//! 1. [`edges`]: BT.601 luma, Sobel, Canny's non-maximum suppression with a sub-pixel parabola: oriented edgels.
//! 2. [`grid`]: a gradient-oriented Hough transform (Duda & Hart 1972; O'Gorman & Clowes 1976), TLS-refined; an ink
//!    stroke is a PAIR of opposite-polarity edges with outward normals (a part edge has no partner); the two line
//!    families by angle; lattice indices from the local spacing; the seen intersections.
//! 3. [`homography`]: the normalised DLT (Hartley 1997; Hartley & Zisserman Alg 4.2) inside RANSAC (H&Z Alg 4.6;
//!    Fischler & Bolles 1981) from the lattice (i, j) · pitch to the pixels; its residual (px, and mm through H⁻¹) is
//!    the uncertainty; the camera's tilt read from H's local affine (a full homography absorbs any tilt of a flat
//!    sheet, so "perpendicular after H" cannot fail: the local affine's departure from a similarity can).
//! 4. [`silhouette`]: the lattice rectified to paper millimetres, Otsu (1979), the ink suppressed by an opening at
//!    the measured stroke width, open / close at pitch / 8, the largest component the part, its enclosed
//!    components the holes.
//! 5. [`contour`]: crack following (Pavlidis 1982), outer CCW / holes CW checked by geom_core's exact `is_ccw`,
//!    Douglas-Peucker (1973) at 2 σ.
//! 6. [`fit`]: corners, TLS lines and Pratt (1987) arcs / circles between them by their residual against σ, the
//!    junctions through geom_core's closed-form intersections.
//! 7. [`sketch`]: [`sketch::to_sketch`] — sketch_core entities, the inferred constraints, solved, profiles.
//!
//! `σ` (`sigma_mm`) is the lattice's millimetre residual and the rectified pixel in quadrature: on a clean photo the
//! residual alone is far under one pixel, and the silhouette cannot be finer than its sampling step. Every band
//! that infers or accepts uses 3 σ. Geometry is f64 (BP-vug-1); the sketch tolerance is the caller's `Tol`
//! (BP-vug-2) and the measurement band is [`PhotoParams::band`] (scan_core's precedent: a `Tol` used as a band).

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::vec::Vec;

pub use geom_core::{Degenerate, Tol, Vec2};
pub use pixel_core::Image;
pub use sketch_core::{Profile, Sketch, Solved};

pub mod contour;
pub mod edges;
pub mod fit;
pub mod fixture;
pub mod grid;
pub mod homography;
pub mod silhouette;
pub mod sketch;

pub use fit::{Outline, Prim};

/// The fewest lines a family needs before it is called a grid.
pub const MIN_LINES: usize = 4;
/// The fewest seen intersections (and RANSAC inliers) the homography is fitted from.
pub const MIN_INTERSECTIONS: usize = 8;

/// What the caller states about the photo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PhotoParams {
    /// The paper's grid pitch in millimetres (the person states it; 5 mm graph paper by default).
    pub pitch_mm: f64,
    /// The measurement band: `lin` the outline acceptance the face shows (mm); `ang` the largest camera tilt off
    /// square accepted (radians) before [`Refusal::GridUnsquare`].
    pub band: Tol,
}

impl PhotoParams {
    /// `pitch_mm` with the default band: 0.5 mm, 20°.
    pub fn new(pitch_mm: f64) -> PhotoParams {
        PhotoParams { pitch_mm, band: Tol { lin: 0.5, ang: 20.0 * core::f64::consts::PI / 180.0 } }
    }
}

impl Default for PhotoParams {
    fn default() -> Self {
        PhotoParams::new(5.0)
    }
}

/// Why a photo did not measure.
#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    /// The RGBA buffer is not `width * height * 4` bytes (or the image is empty).
    BadImage { width: u32, height: u32, len: usize },
    /// The pitch is not a positive finite number.
    BadPitch,
    /// Fewer than [`MIN_LINES`] lines in a family (`a`, `b` the counts found).
    NoGrid { a: usize, b: usize },
    /// Fewer than `need` seen intersections or homography inliers.
    TooFewIntersections { have: usize, need: usize },
    /// The camera is tilted `deg` off square to the paper (H's local affine), past the band.
    GridUnsquare { deg: f64 },
    /// A geometric construction met a degenerate input (a singular homography, a zero direction).
    Degenerate(Degenerate),
    /// No dark component large enough to be a part (under (2 · pitch)²).
    NoPart,
    /// The part touches the rectified area's border or leaves the photo.
    PartClipped,
    /// The exact orientation predicate refused (a non-finite or out-of-range loop), or a loop did not close.
    Uncertain,
    /// sketch_core refused the inferred sketch.
    Sketch(sketch_core::Refusal),
}

impl Refusal {
    /// The refusal's name, for a face's one-line report.
    pub fn name(&self) -> &'static str {
        match self {
            Refusal::BadImage { .. } => "BadImage",
            Refusal::BadPitch => "BadPitch",
            Refusal::NoGrid { .. } => "NoGrid",
            Refusal::TooFewIntersections { .. } => "TooFewIntersections",
            Refusal::GridUnsquare { .. } => "GridUnsquare",
            Refusal::Degenerate(_) => "Degenerate",
            Refusal::NoPart => "NoPart",
            Refusal::PartClipped => "PartClipped",
            Refusal::Uncertain => "Uncertain",
            Refusal::Sketch(_) => "Sketch",
        }
    }
}

/// A measured part: the outline in paper millimetres (y up, as seen from above), its uncertainty, the grid it was
/// measured against, and the sketch inferred from it.
#[derive(Clone, Debug)]
pub struct Measured {
    /// The outer boundary, CCW, simplified at 2 σ (Douglas-Peucker vertices), paper mm.
    pub outline: Vec<Vec2>,
    /// Each hole's boundary, CW, simplified the same way.
    pub holes: Vec<Vec<Vec2>>,
    /// Every boundary at the measured σ: the outer loop first (CCW, = `outline`), then each hole (CW, = `holes`), so
    /// a residual against the measurement (doc_core's intent check) reads a part with holes whole.
    pub loops: Vec<Vec<Vec2>>,
    /// The fitted primitives.
    pub shape: Outline,
    /// The uncertainty every outline point carries (mm): `hypot(residual_mm, 1 / rect_px_per_mm)`.
    pub sigma_mm: f64,
    pub pitch_mm: f64,
    /// The median lattice spacing in the photo (px).
    pub pitch_px: f64,
    /// The homography's rms residual over its inliers, in pixels and in millimetres through H⁻¹.
    pub residual_px: f64,
    pub residual_mm: f64,
    /// The rectified sampling step (px per mm) the silhouette was taken at.
    pub rect_px_per_mm: f64,
    /// The radial residual pattern: k in e_radial = k (r / r_max)³ (px at the photo's corner; VUG-PHOTO-LENS owes the model).
    pub lens_hint: f64,
    /// The camera's tilt off square at the lattice centroid (degrees).
    pub tilt_deg: f64,
    /// The ink stroke width (mm), measured from the edge pairs.
    pub line_width_mm: f64,
    /// Lines in each family.
    pub grid: (usize, usize),
    /// Seen intersections, and the homography's inliers among them.
    pub intersections: usize,
    pub inliers: usize,
    /// Paper mm → photo px: `px ~ h · [x, y, 1]` (row-major; VUG-FACE-PHOTO's underlay reads it).
    pub h: [[f64; 3]; 3],
    /// The inferred sketch, its solve and its profiles.
    pub sketch: Sketch,
    pub solved: Solved,
    pub profiles: Vec<Profile>,
}

/// Measure a photo of a flat part lying on graph paper of `params.pitch_mm`; `tol` is the sketch's tolerance.
pub fn measure(img: &Image, params: PhotoParams, tol: Tol) -> Result<Measured, Refusal> {
    let pitch = params.pitch_mm;
    if !(pitch.is_finite() && pitch > 0.0) {
        return Err(Refusal::BadPitch);
    }
    let gray = edges::luma(img).ok_or(Refusal::BadImage { width: img.width, height: img.height, len: img.rgba.len() })?;
    let el = edges::edgels(&gray);
    let g = grid::find(&el, gray.w, gray.h)?;
    let lat = homography::fit_lattice(&g, pitch, gray.w, gray.h)?;
    let max_deg = params.band.ang * 180.0 / core::f64::consts::PI;
    if lat.tilt_deg > max_deg {
        return Err(Refusal::GridUnsquare { deg: lat.tilt_deg });
    }
    let r = libm::round(g.pitch_px / pitch).clamp(4.0, 20.0);
    let line_width_mm = g.width_px / lat.px_per_mm;
    let sil = silhouette::extract(&gray, &lat, pitch, line_width_mm, r)?;
    let sigma_mm = libm::hypot(lat.residual_mm, 1.0 / r);
    let loops = contour::trace(&sil)?;
    let outline = contour::simplify(&loops.outer, 2.0 * sigma_mm);
    let holes: Vec<Vec<Vec2>> = loops.holes.iter().map(|h| contour::simplify(h, 2.0 * sigma_mm)).collect();
    let mut all = Vec::with_capacity(1 + holes.len());
    all.push(outline.clone());
    all.extend(holes.iter().cloned());
    let shape = fit::outline(&loops, sigma_mm, pitch)?;
    let sk = sketch::to_sketch(&shape, tol)?;
    Ok(Measured {
        outline,
        holes,
        loops: all,
        shape,
        sigma_mm,
        pitch_mm: pitch,
        pitch_px: g.pitch_px,
        residual_px: lat.residual_px,
        residual_mm: lat.residual_mm,
        rect_px_per_mm: r,
        lens_hint: lat.lens_hint,
        tilt_deg: lat.tilt_deg,
        line_width_mm,
        grid: (g.a.len(), g.b.len()),
        intersections: g.inter.len(),
        inliers: lat.inliers,
        h: lat.h,
        sketch: sk.sketch,
        solved: sk.solved,
        profiles: sk.profiles,
    })
}
