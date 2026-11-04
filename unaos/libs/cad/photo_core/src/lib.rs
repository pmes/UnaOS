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
    /// VUG-PHOTO-SIDE-PARALLAX (SR568): the camera's height above the paper (mm), the person's once-per-setup datum
    /// for [`measure_side_raised`]; `None` self-calibrates it from the lattice homography ([`camera`]).
    pub camera_mm: Option<f64>,
}

impl PhotoParams {
    /// `pitch_mm` with the default band: 0.5 mm, 20°.
    pub fn new(pitch_mm: f64) -> PhotoParams {
        PhotoParams { pitch_mm, band: Tol { lin: 0.5, ang: 20.0 * core::f64::consts::PI / 180.0 }, camera_mm: None }
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
    /// VUG-PHOTO-SIDE (SR545): the side photo's long dimension `length_mm` matches no extent of the first outline:
    /// `nearest_mm` the nearer extent, `band_mm` the 3 σ band it missed by more than.
    NoSideMatch { length_mm: f64, nearest_mm: f64, band_mm: f64 },
    /// VUG-PHOTO-SIDE-PARALLAX (SR568): the camera is tilted `tilt_deg` (under [`CALIB_MIN_DEG`]), too square to the
    /// paper for the homography to give its height, and no `PhotoParams::camera_mm` was stated.
    NoCamera { tilt_deg: f64 },
    /// VUG-PHOTO-SIDE-PARALLAX (SR568): the camera (`height_mm`) is not above the standing part's top (`standing_mm`).
    Parallax { height_mm: f64, standing_mm: f64 },
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
            Refusal::NoSideMatch { .. } => "NoSideMatch",
            Refusal::NoCamera { .. } => "NoCamera",
            Refusal::Parallax { .. } => "Parallax",
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
    /// The radial residual pattern LEFT after the lens term: k in e_radial = k (r / r_max)³ (px at the photo's corner).
    pub lens_hint: f64,
    /// VUG-PHOTO-LENS (SR533): the Brown radial term k1 (r_d = r (1 + k1 r²), r normalised by the half-diagonal, about
    /// the photo's centre) the photo was undistorted by before the lattice and the silhouette; 0.0 when the refined
    /// term did not halve the wide-band lattice residual (the SR503 road, bit-exact). When non-zero, `h` maps paper mm to UNDISTORTED px: a
    /// consumer drawing over the raw photo composes `homography::distort(apply(h, p), k1, w, h)`.
    pub k1: f64,
    /// The lattice residual (px) with no lens term (= `residual_px` when `k1` is 0.0).
    pub residual_flat_px: f64,
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
    measure_lens(img, params, tol, true)
}

/// [`measure`] with the lens term on (`lens`) or off (VUG-PHOTO-LENS SR533; off is the SR503 road).
pub fn measure_lens(img: &Image, params: PhotoParams, tol: Tol, lens: bool) -> Result<Measured, Refusal> {
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
    // VUG-PHOTO-LENS (SR533): k1 from the lattice residual; the photo undistorted before the silhouette.
    let residual_flat_px = lat.residual_px;
    let (gray, g, lat, k1) = if lens { lens_passes(gray, g, lat, pitch)? } else { (gray, g, lat, 0.0) };
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
        k1,
        residual_flat_px,
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

// VUG-PHOTO-LENS (SR533): the passes. Pass 0 is the flat lattice (SR503's); k1 is fitted to its wide-band inliers
// (homography::LENS_BAND); each refinement pass undistorts the photo by the current k1, finds the grid again (the
// bowed lines now straight, so more of them are seen), takes the intersections back to raw pixels by the exact
// forward model and fits the TOTAL k1; the last k1 undistorts the photo the lattice and the silhouette are measured
// on. The gate is AFTER the refinement: pass 0 cannot judge the term, since its crossings are intersections of
// straight lines fitted to bowed strokes, a bias k1 cannot remove (measured on the k1 = -0.08 fixture: pass 0 alone
// went 2.836 -> 1.791 px, ratio 0.632, with k1 already at -0.0758). The term is taken when the final undistorted
// lattice's wide-band residual is at most LENS_GAIN x the flat one; otherwise, or when a refinement pass refuses,
// the pass-0 values come back unchanged (the SR503 road, bit-exact).
fn lens_passes(
    gray0: edges::Gray,
    g0: grid::Grid,
    lat0: homography::Lattice,
    pitch: f64,
) -> Result<(edges::Gray, grid::Grid, homography::Lattice, f64), Refusal> {
    let (w, h) = (gray0.w, gray0.h);
    let Ok((paper, raw)) = homography::lens_set(&g0, pitch, &|q: Vec2| q) else { return Ok((gray0, g0, lat0, 0.0)) };
    let rms0 = homography::lens_residual(&paper, &raw, 0.0, w, h);
    let k = homography::fit_k1(&paper, &raw, w, h).0;
    match lens_refine(&gray0, k, pitch) {
        Ok((gray, g, lat, k, rms_f)) if rms_f <= homography::LENS_GAIN * rms0 => Ok((gray, g, lat, k)),
        _ => Ok((gray0, g0, lat0, 0.0)),
    }
}

/// The refinement passes from `k`, then the final undistorted photo, its grid, its lattice, k1 and the final grid's
/// wide-band residual (px, undistorted).
fn lens_refine(
    gray0: &edges::Gray,
    mut k: f64,
    pitch: f64,
) -> Result<(edges::Gray, grid::Grid, homography::Lattice, f64, f64), Refusal> {
    let (w, h) = (gray0.w, gray0.h);
    for _ in 0..homography::LENS_PASSES {
        let (_, g) = lens_grid(gray0, k)?;
        let kp = k;
        let (paper, raw) = homography::lens_set(&g, pitch, &|q: Vec2| homography::distort(q, kp, w, h))?;
        k = homography::fit_k1(&paper, &raw, w, h).0;
    }
    let (gray, g) = lens_grid(gray0, k)?;
    let kf = k;
    let (paper, raw) = homography::lens_set(&g, pitch, &|q: Vec2| homography::distort(q, kf, w, h))?;
    let rms_f = homography::lens_residual(&paper, &raw, k, w, h);
    let lat = homography::fit_lattice(&g, pitch, w, h)?;
    Ok((gray, g, lat, k, rms_f))
}

fn lens_grid(gray0: &edges::Gray, k1: f64) -> Result<(edges::Gray, grid::Grid), Refusal> {
    let gray = undistort_gray(gray0, k1);
    let g = grid::find(&edges::edgels(&gray), gray.w, gray.h)?;
    Ok((gray, g))
}

/// The photo undistorted by `k1`: every pixel is the raw luma (bilinear, `Gray::sample`) where the lens put it, the
/// coordinates clamped to the pixel centres' hull.
pub fn undistort_gray(g: &edges::Gray, k1: f64) -> edges::Gray {
    let (w, h) = (g.w, g.h);
    let (xmax, ymax) = (w as f64 - 0.5 - 1e-9, h as f64 - 0.5 - 1e-9);
    let mut v = Vec::with_capacity(w * h);
    for j in 0..h {
        for i in 0..w {
            let q = homography::distort(Vec2::new(i as f64 + 0.5, j as f64 + 0.5), k1, w, h);
            v.push(g.sample(q.x.clamp(0.5, xmax), q.y.clamp(0.5, ymax)).unwrap_or(g.at(i, j)));
        }
    }
    edges::Gray { w, h, v }
}

// VUG-PHOTO-SIDE (SR545): the thickness from a second photo of the part on its side. The side photo goes through the
// one lattice road ([`measure`]); its outline's long dimension is paired with the first outline's matching extent by
// length (within BAND_SIGMA x the two photos' σ in quadrature), and its short dimension is the thickness, carrying the
// side photo's σ.

/// A part's thickness measured from a photo of it on its side.
#[derive(Clone, Debug)]
pub struct Side {
    /// The side outline's short dimension (mm).
    pub thickness_mm: f64,
    /// The side photo's σ (its lattice residual and rectified pixel in quadrature), the thickness's uncertainty (mm).
    pub sigma_mm: f64,
    /// The side outline's long dimension (mm).
    pub length_mm: f64,
    /// The first outline's extent the long dimension was paired with (mm).
    pub paired_mm: f64,
    /// The side photo's own measurement.
    pub side: Measured,
    /// VUG-PHOTO-SIDE-PARALLAX (SR568): the raised-edge correction [`measure_side_raised`] applied; `None` on the flat
    /// road ([`measure_side`]).
    pub parallax: Option<Parallax>,
}

/// An outline's (long, short) extents (mm) in the frame of its longest line (the paper's x when it has none): the box
/// (`geom_core::Aabb2`) of its line ends, its arcs' ends and their extreme points on the frame's axes, and its circles'
/// extreme points. `None` for an empty outline.
pub fn extents(o: &fit::Outline) -> Option<(f64, f64)> {
    let (_, b) = extents_box(o)?;
    let (ex, ey) = (b.max.x - b.min.x, b.max.y - b.min.y);
    Some((ex.max(ey), ex.min(ey)))
}

/// [`extents`]' frame and box: the longest line's direction `d` and the box of the outline's extreme points in the
/// local `(p · d, p · d.perp())` coordinates (VUG-PHOTO-SIDE-PARALLAX SR568 corrects each side of it).
pub fn extents_box(o: &fit::Outline) -> Option<(Vec2, geom_core::Aabb2)> {
    let d = o
        .outer
        .iter()
        .filter_map(|p| if let Prim::Line { a, b, .. } = *p { Some(b - a) } else { None })
        .max_by(|x, y| x.mag_sq().total_cmp(&y.mag_sq()))
        .and_then(|v| v.normalize().ok())
        .unwrap_or(Vec2::X);
    let n = d.perp();
    let axes = [d, -d, n, -n];
    let mut pts: Vec<Vec2> = Vec::new();
    for p in &o.outer {
        match *p {
            Prim::Line { a, b, .. } => {
                pts.push(a);
                pts.push(b);
            }
            Prim::Arc { centre, r, from, to, ccw, .. } => {
                pts.push(from);
                pts.push(to);
                for u in axes {
                    let q = centre + u * r;
                    if on_arc(centre, from, to, ccw, q) {
                        pts.push(q);
                    }
                }
            }
            Prim::Circle { centre, r, .. } => {
                for u in axes {
                    pts.push(centre + u * r);
                }
            }
        }
    }
    let local: Vec<Vec2> = pts.iter().map(|&p| Vec2::new(p.dot(d), p.dot(n))).collect();
    let b = geom_core::Aabb2::from_points(&local).ok()?;
    Some((d, b))
}

/// Whether `q` (on the circle about `c`) lies on the sweep from `from` to `to`, counterclockwise when `ccw`.
fn on_arc(c: Vec2, from: Vec2, to: Vec2, ccw: bool, q: Vec2) -> bool {
    let tau = 2.0 * core::f64::consts::PI;
    let ang = |p: Vec2| libm::atan2(p.y - c.y, p.x - c.x);
    let sign = if ccw { 1.0 } else { -1.0 };
    let wrap = |v: f64| {
        let m = v % tau;
        if m < 0.0 { m + tau } else { m }
    };
    let total = wrap((ang(to) - ang(from)) * sign);
    let at = wrap((ang(q) - ang(from)) * sign);
    at <= total
}

/// Measure the thickness of the part `first` measured, from `side`, a photo of it lying on its side on the same
/// graph paper. Refused by name when the side's long dimension matches no extent of the first outline within 3 σ.
pub fn measure_side(first: &Measured, side: &Image, params: PhotoParams, tol: Tol) -> Result<Side, Refusal> {
    let s = measure(side, params, tol)?;
    let (length, thickness) = extents(&s.shape).ok_or(Refusal::Uncertain)?;
    let (long, short) = extents(&first.shape).ok_or(Refusal::Uncertain)?;
    let paired = if (length - long).abs() <= (length - short).abs() { long } else { short };
    let band = fit::BAND_SIGMA * libm::hypot(first.sigma_mm, s.sigma_mm);
    if !((length - paired).abs() <= band) {
        return Err(Refusal::NoSideMatch { length_mm: length, nearest_mm: paired, band_mm: band });
    }
    Ok(Side { thickness_mm: thickness, sigma_mm: s.sigma_mm, length_mm: length, paired_mm: paired, side: s, parallax: None })
}

// VUG-PHOTO-SIDE-PARALLAX (SR568): a part standing on its side rises above the paper by its STANDING dimension (the
// first outline's extent the side photo is not paired with), so the camera, at height Hc over the paper point N (its
// nadir), sees a point at height Z where the paper point N + k (p - N) is, k = Hc / (Hc - Z). The side silhouette is the
// hull of the footprint and the top scaled by k about N; its support along any direction u is max(h_F(u), N.u + k (h_F(u)
// - N.u)), which inverts per side exactly: a box side beyond the nadir has its offset divided by k, a near side is the
// footprint's own. Hc and N come from the side photo's lattice homography (the principal point the photo centre, the
// lens model's centre, square pixels: the two image-of-the-absolute-conic constraints give 1 / f², then K⁻¹ H = λ [r1 r2 t]
// and the camera centre -(r1·t, r2·t, r3·t)) when the camera is tilted at least CALIB_MIN_DEG; a square camera knows only
// f / Hc, so the person states Hc once per setup (`PhotoParams::camera_mm`) and N is the photo centre through H⁻¹.

/// The least camera tilt (degrees) the homography self-calibrates the camera's height and nadir from.
pub const CALIB_MIN_DEG: f64 = 6.0;

/// The camera over the paper, in the lattice's paper millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    /// The paper point under the camera centre (mm).
    pub nadir: Vec2,
    /// The camera centre's height above the paper (mm).
    pub height_mm: f64,
    /// The self-calibrated focal length (px); `None` when the person stated the height.
    pub focal_px: Option<f64>,
}

/// The raised-edge correction a side measurement took.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Parallax {
    pub camera: Camera,
    /// The standing part's height above the paper (mm): the first outline's unpaired extent.
    pub standing_mm: f64,
    /// The top's magnification about the nadir, Hc / (Hc - standing).
    pub k: f64,
    /// The side outline's (long, short) extents before the correction (mm).
    pub raw_length_mm: f64,
    pub raw_thickness_mm: f64,
}

/// The camera from a paper-to-photo homography `h` of a `w` x `h_px` photo (principal point the photo centre, square
/// pixels); `None` when the perspective is too weak to give a positive finite 1 / f².
pub fn camera_pose(h: &homography::H, w: f64, h_px: f64) -> Option<Camera> {
    let (u0, v0) = (w * 0.5, h_px * 0.5);
    let g = [
        [h[0][0] - u0 * h[2][0], h[0][1] - u0 * h[2][1], h[0][2] - u0 * h[2][2]],
        [h[1][0] - v0 * h[2][0], h[1][1] - v0 * h[2][1], h[1][2] - v0 * h[2][2]],
        h[2],
    ];
    let a1 = g[0][0] * g[0][1] + g[1][0] * g[1][1];
    let b1 = g[2][0] * g[2][1];
    let a2 = g[0][0] * g[0][0] + g[1][0] * g[1][0] - g[0][1] * g[0][1] - g[1][1] * g[1][1];
    let b2 = g[2][0] * g[2][0] - g[2][1] * g[2][1];
    let x = -(a1 * b1 + a2 * b2) / (a1 * a1 + a2 * a2);
    if !(x.is_finite() && x > 0.0) {
        return None;
    }
    let f = 1.0 / libm::sqrt(x);
    let col = |i: usize| geom_core::Vec3::new(g[0][i] / f, g[1][i] / f, g[2][i]);
    let (m0, m1, m2) = (col(0), col(1), col(2));
    let lam = 2.0 / (m0.mag() + m1.mag());
    if !lam.is_finite() {
        return None;
    }
    let (r1, r2, t) = (m0 * lam, m1 * lam, m2 * lam);
    let r3 = r1.cross(r2);
    let nadir = Vec2::new(-r1.dot(t), -r2.dot(t));
    let height_mm = r3.dot(t).abs();
    if !(nadir.is_finite() && height_mm.is_finite() && height_mm > 0.0) {
        return None;
    }
    Some(Camera { nadir, height_mm, focal_px: Some(f) })
}

/// The camera over the photo `m` measured (`w` x `h_px` px): self-calibrated when its tilt is at least
/// [`CALIB_MIN_DEG`] (the height replaced by `camera_mm` when the person stated one); else `camera_mm` with the nadir
/// the photo centre through H⁻¹ (exact for a square camera); else [`Refusal::NoCamera`].
pub fn camera(m: &Measured, w: f64, h_px: f64, camera_mm: Option<f64>) -> Result<Camera, Refusal> {
    let pose = if m.tilt_deg >= CALIB_MIN_DEG { camera_pose(&m.h, w, h_px) } else { None };
    match (pose, camera_mm) {
        (Some(c), None) => Ok(c),
        (Some(c), Some(hc)) => Ok(Camera { height_mm: hc, ..c }),
        (None, Some(hc)) => {
            let inv = homography::invert(&m.h).map_err(Refusal::Degenerate)?;
            let nadir = homography::apply(&inv, Vec2::new(w * 0.5, h_px * 0.5)).ok_or(Refusal::Uncertain)?;
            Ok(Camera { nadir, height_mm: hc, focal_px: None })
        }
        (None, None) => Err(Refusal::NoCamera { tilt_deg: m.tilt_deg }),
    }
}

/// [`extents`] of a raised outline: each side of [`extents_box`] beyond the nadir's coordinate pulled back by `1 / k`.
pub fn extents_raised(o: &fit::Outline, nadir: Vec2, k: f64) -> Option<(f64, f64)> {
    let (d, b) = extents_box(o)?;
    let c = Vec2::new(nadir.dot(d), nadir.dot(d.perp()));
    let side = |lo: f64, hi: f64, c: f64| {
        let hi = if hi > c { c + (hi - c) / k } else { hi };
        let lo = if lo < c { c + (lo - c) / k } else { lo };
        hi - lo
    };
    let (ex, ey) = (side(b.min.x, b.max.x, c.x), side(b.min.y, b.max.y, c.y));
    Some((ex.max(ey), ex.min(ey)))
}

/// [`measure_side`] for a real photo: the part stands on its side, its top raised by the first outline's unpaired
/// extent; the side outline's extents are corrected for the parallax ([`extents_raised`]) under both pairings (paired
/// with the first's long extent, standing on its short one, and the converse), the smaller miss taken; outside the 3 σ
/// band [`Refusal::NoSideMatch`] carries the corrected length. The camera is [`camera`]'s.
pub fn measure_side_raised(first: &Measured, side: &Image, params: PhotoParams, tol: Tol) -> Result<Side, Refusal> {
    let s = measure(side, params, tol)?;
    let (raw_length_mm, raw_thickness_mm) = extents(&s.shape).ok_or(Refusal::Uncertain)?;
    let (long, short) = extents(&first.shape).ok_or(Refusal::Uncertain)?;
    let cam = camera(&s, side.width as f64, side.height as f64, params.camera_mm)?;
    let band = fit::BAND_SIGMA * libm::hypot(first.sigma_mm, s.sigma_mm);
    // (miss, length, thickness, paired, standing, k)
    let mut best: Option<(f64, f64, f64, f64, f64, f64)> = None;
    for (paired, standing) in [(long, short), (short, long)] {
        if !(cam.height_mm > standing) {
            continue;
        }
        let k = cam.height_mm / (cam.height_mm - standing);
        let (length, thickness) = extents_raised(&s.shape, cam.nadir, k).ok_or(Refusal::Uncertain)?;
        let miss = (length - paired).abs();
        if best.map_or(true, |b| miss < b.0) {
            best = Some((miss, length, thickness, paired, standing, k));
        }
    }
    let Some((miss, length, thickness, paired, standing, k)) = best else {
        return Err(Refusal::Parallax { height_mm: cam.height_mm, standing_mm: short });
    };
    if !(miss <= band) {
        return Err(Refusal::NoSideMatch { length_mm: length, nearest_mm: paired, band_mm: band });
    }
    let parallax = Parallax { camera: cam, standing_mm: standing, k, raw_length_mm, raw_thickness_mm };
    Ok(Side { thickness_mm: thickness, sigma_mm: s.sigma_mm, length_mm: length, paired_mm: paired, side: s, parallax: Some(parallax) })
}
