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
//!    components the holes; a stroke pair off every lattice line by more than the stroke width is a thin side of the
//!    part, not ink, and its dark band connected to the part is put back ([`thin_sides`], VUGPHOTOSIDETHIN SR651);
//!    on a lattice line the stretch whose core is darker than the sheet's own ink tone is put back the same way
//!    ([`dark_pair`], VUGPHOTOSIDETHINCROSS SR654).
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
    /// VUG-PHOTO-PP (SR569): the principal point in photo pixels; `None` is the photo centre. A cropped photo states it
    /// (the uncropped centre less the crop's offset) so [`camera_pose_at`] solves f and the pose about the right point.
    /// VUGFACEPHOTOCAMERAPREF (SR576): the lens term k1 is centred on it too ([`measure_lens`], [`undistort_gray_at`]).
    /// VUGPHOTOCROPMETA (SR582): a photo carrying its own crop (the PNG tEXt [`CROP_KEY`], [`crop_principal`]) outranks it.
    pub principal_px: Option<Vec2>,
}

impl PhotoParams {
    /// `pitch_mm` with the default band: 0.5 mm, 20°.
    pub fn new(pitch_mm: f64) -> PhotoParams {
        PhotoParams { pitch_mm, band: Tol { lin: 0.5, ang: 20.0 * core::f64::consts::PI / 180.0 }, camera_mm: None, principal_px: None }
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
    /// the principal point: `PhotoParams.principal_px`, else the photo's centre, VUGFACEPHOTOCAMERAPREF SR576; a crop's
    /// k1 is the full frame's times (half-diagonal of the crop / of the frame)²) the photo was undistorted by before the lattice and the silhouette; 0.0 when the refined
    /// term did not halve the wide-band lattice residual (the SR503 road, bit-exact). When non-zero, `h` maps paper mm to UNDISTORTED px: a
    /// consumer drawing over the raw photo composes `homography::distort(apply(h, p), k1, w, h)` (about the centre; about
    /// a stated principal point pp: `distort(q + d, k1, w, h) - d` with d = centre - pp).
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
    let pp = principal_of(img, &params); // VUGFACEPHOTOCAMERAPREF (SR576): the lens about the principal point; VUGPHOTOCROPMETA (SR582): the file's crop meta, the pref, the centre
    let (gray, g, lat, k1) = if lens { lens_passes(gray, g, lat, pitch, pp)? } else { (gray, g, lat, 0.0) };
    if lat.tilt_deg > max_deg {
        return Err(Refusal::GridUnsquare { deg: lat.tilt_deg });
    }
    let r = libm::round(g.pitch_px / pitch).clamp(4.0, 20.0);
    let line_width_mm = g.width_px / lat.px_per_mm;
    let sil = thin_sides(silhouette::extract(&gray, &lat, pitch, line_width_mm, r)?, &gray, &lat, &g.pairs, pitch, line_width_mm)?; // VUGPHOTOSIDETHIN (SR651): a pair off the lattice is a thin side
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
// the pass-0 values come back unchanged (the SR503 road, bit-exact). VUGFACEPHOTOCAMERAPREF (SR576): the model is
// centred on the principal point `pp` by working in a translated frame: with d = centre - pp, the raw crossings are
// shifted by +d into homography's centred model and the photo is undistorted by distort(p + d) - d; at pp = centre
// d is (0.0, 0.0) and every step is the SR533 one bit for bit.
fn lens_passes(
    gray0: edges::Gray,
    g0: grid::Grid,
    lat0: homography::Lattice,
    pitch: f64,
    pp: Vec2,
) -> Result<(edges::Gray, grid::Grid, homography::Lattice, f64), Refusal> {
    let (w, h) = (gray0.w, gray0.h);
    let d = lens_shift(w, h, pp);
    let Ok((paper, raw)) = homography::lens_set(&g0, pitch, &|q: Vec2| q + d) else { return Ok((gray0, g0, lat0, 0.0)) };
    let rms0 = homography::lens_residual(&paper, &raw, 0.0, w, h);
    let k = homography::fit_k1(&paper, &raw, w, h).0;
    match lens_refine(&gray0, k, pitch, pp) {
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
    pp: Vec2,
) -> Result<(edges::Gray, grid::Grid, homography::Lattice, f64, f64), Refusal> {
    let (w, h) = (gray0.w, gray0.h);
    let d = lens_shift(w, h, pp);
    for _ in 0..homography::LENS_PASSES {
        let (_, g) = lens_grid(gray0, k, pp)?;
        let kp = k;
        let (paper, raw) = homography::lens_set(&g, pitch, &|q: Vec2| homography::distort(q + d, kp, w, h))?;
        k = homography::fit_k1(&paper, &raw, w, h).0;
    }
    let (gray, g) = lens_grid(gray0, k, pp)?;
    let kf = k;
    let (paper, raw) = homography::lens_set(&g, pitch, &|q: Vec2| homography::distort(q + d, kf, w, h))?;
    let rms_f = homography::lens_residual(&paper, &raw, k, w, h);
    let lat = homography::fit_lattice(&g, pitch, w, h)?;
    Ok((gray, g, lat, k, rms_f))
}

fn lens_grid(gray0: &edges::Gray, k1: f64, pp: Vec2) -> Result<(edges::Gray, grid::Grid), Refusal> {
    let gray = undistort_gray_at(gray0, k1, pp);
    let g = grid::find(&edges::edgels(&gray), gray.w, gray.h)?;
    Ok((gray, g))
}

/// The photo undistorted by `k1`: every pixel is the raw luma (bilinear, `Gray::sample`) where the lens put it, the
/// coordinates clamped to the pixel centres' hull. The lens about the photo's centre ([`undistort_gray_at`]'s wrapper).
pub fn undistort_gray(g: &edges::Gray, k1: f64) -> edges::Gray {
    undistort_gray_at(g, k1, Vec2::new(g.w as f64 * 0.5, g.h as f64 * 0.5))
}

/// VUGFACEPHOTOCAMERAPREF (SR576): [`undistort_gray`] with the lens centred on the principal point `pp` (photo px): the
/// raw pixel of undistorted `p` is `distort(p + d) - d`, d = centre - pp (homography's centred model in a translated
/// frame; the normaliser stays the photo's half-diagonal). At pp = centre it is [`undistort_gray`] bit for bit.
pub fn undistort_gray_at(g: &edges::Gray, k1: f64, pp: Vec2) -> edges::Gray {
    let (w, h) = (g.w, g.h);
    let d = lens_shift(w, h, pp);
    let (xmax, ymax) = (w as f64 - 0.5 - 1e-9, h as f64 - 0.5 - 1e-9);
    let mut v = Vec::with_capacity(w * h);
    for j in 0..h {
        for i in 0..w {
            let q = homography::distort(Vec2::new(i as f64 + 0.5, j as f64 + 0.5) + d, k1, w, h) - d;
            v.push(g.sample(q.x.clamp(0.5, xmax), q.y.clamp(0.5, ymax)).unwrap_or(g.at(i, j)));
        }
    }
    edges::Gray { w, h, v }
}

/// VUGFACEPHOTOCAMERAPREF (SR576): d = the photo's centre - `pp`, the translation that puts the principal point on
/// homography's lens centre ((0.0, 0.0) when `pp` is the centre).
fn lens_shift(w: usize, h: usize, pp: Vec2) -> Vec2 {
    Vec2::new(w as f64 * 0.5, h as f64 * 0.5) - pp
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
    camera_pose_at(h, Vec2::new(w * 0.5, h_px * 0.5))
}

/// VUG-PHOTO-PP (SR569): [`camera_pose`] about the principal point `pp` (photo px). With g = H's rows 0 and 1 less
/// `pp` times row 2, and x = 1 / f²: r1·r2 = 0 is (g00 g01 + g10 g11) x + g20 g21 = 0 and |r1|² = |r2|² is
/// (g00² + g10² - g01² - g11²) x + (g20² - g21²) = 0, solved together by least squares.
pub fn camera_pose_at(h: &homography::H, pp: Vec2) -> Option<Camera> {
    let (u0, v0) = (pp.x, pp.y);
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
    camera_at(m, Vec2::new(w * 0.5, h_px * 0.5), camera_mm)
}

/// VUG-PHOTO-PP (SR569): [`camera`] about the principal point `pp` (photo px): the pose by [`camera_pose_at`], the
/// square camera's nadir `pp` through H⁻¹ (the principal ray is the optical axis).
pub fn camera_at(m: &Measured, pp: Vec2, camera_mm: Option<f64>) -> Result<Camera, Refusal> {
    let pose = if m.tilt_deg >= CALIB_MIN_DEG { camera_pose_at(&m.h, pp) } else { None };
    match (pose, camera_mm) {
        (Some(c), None) => Ok(c),
        (Some(c), Some(hc)) => Ok(Camera { height_mm: hc, ..c }),
        (None, Some(hc)) => {
            let inv = homography::invert(&m.h).map_err(Refusal::Degenerate)?;
            let nadir = homography::apply(&inv, pp).ok_or(Refusal::Uncertain)?;
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
    let cam = camera_at(&s, principal_of(side, &params), params.camera_mm)?; // VUG-PHOTO-PP (SR569); VUGPHOTOCROPMETA (SR582): the one order
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

// ---- VUGPHOTOCROPMETA (SR582): the principal point a cropped photo carries (named tail block) ----

/// The PNG tEXt keyword a cropped photo carries its crop in: `<x>,<y>,<W>,<H>`, the crop's offset in the frame it was cut
/// from and that frame's size (the principal point is the uncropped centre less the offset, so the frame's size travels
/// with it; [`fixture::crop_png`] writes it).
pub const CROP_KEY: &str = "vug:crop";

/// The principal point the photo's own [`CROP_KEY`] pair states (the first one), `(W / 2 - x, H / 2 - y)`, else None
/// (absent, or not four integers with W and H above 0).
pub fn crop_principal(img: &Image) -> Option<Vec2> {
    let [x, y, w, h] = crop_frame(img)?; // VUGFACEPHOTOCROPSAVE (SR640): the one parse, shared with crop_pair
    Some(Vec2::new(w as f64 * 0.5 - x as f64, h as f64 * 0.5 - y as f64))
}

/// THE order for the principal point: the photo's crop meta ([`crop_principal`]), else `params.principal_px` (the
/// person's pref), else the photo's centre.
fn principal_of(img: &Image, params: &PhotoParams) -> Vec2 {
    crop_principal(img).or(params.principal_px).unwrap_or(Vec2::new(img.width as f64 * 0.5, img.height as f64 * 0.5))
}

// ---- VUGPHOTOCROPMETAXMP (SR631): the crop meta inside an XMP packet (named tail block) ----

/// VUGPHOTOCROPMETAXMP (SR631): one `Image.text` pair's crop value: a [`CROP_KEY`] pair's value, or the [`CROP_KEY`]
/// property of an XMP packet (pixel_core's [`pixel_core::png::XMP_KEY`] pair: a PNG iTXt, a JPEG APP1 or a WebP `XMP `
/// chunk), else None.
fn crop_value(k: &str, v: &str) -> Option<alloc::string::String> {
    if k == CROP_KEY {
        return Some(alloc::string::String::from(v));
    }
    if k != pixel_core::png::XMP_KEY {
        return None;
    }
    xmp_crop(v)
}

/// VUGPHOTOCROPMETAXMP (SR631): the [`CROP_KEY`] property of an XMP packet, read through svg_core's one XML reader
/// (the qualified parse: XMP keys a property by its prefixed name): `vug:crop="…"` on an `rdf:Description`, or a
/// `<vug:crop>…</vug:crop>` child of one (its text, trimmed) — both forms, since the house writes no XMP. The first in
/// document order wins; None when the packet carries none or is not well-formed XML.
pub fn xmp_crop(packet: &str) -> Option<alloc::string::String> {
    use svg_core::xml::{Document, Kind, Ns};
    let doc = Document::parse_qualified(packet.as_bytes()).ok()?;
    for (i, n) in doc.nodes.iter().enumerate() {
        if !matches!(&n.kind, Kind::Element { ns: Ns::Other, name } if name == "Description") {
            continue;
        }
        if let Some(a) = n.attrs.iter().find(|a| a.ns == Ns::Other && a.name == CROP_KEY) {
            return Some(a.value.clone());
        }
        if let Some(c) = doc.elements(i).find(|&c| doc.nodes[c].qname() == Some(CROP_KEY)) {
            let t: alloc::string::String = doc.nodes[c].children.iter().filter_map(|&t| doc.nodes[t].text()).collect();
            return Some(alloc::string::String::from(t.trim()));
        }
    }
    None
}

// ---- VUGFACEPHOTOCROPSAVE (SR640): the crop frame read once and written once (named tail block) ----

/// The photo's own crop frame `[x, y, W, H]` (the first [`CROP_KEY`] pair, or the one inside an XMP packet, first in file
/// order), else None (absent, or not four integers with W and H above 0). THE parse of the key: [`crop_principal`] and
/// [`crop_pair`] read it.
pub fn crop_frame(img: &Image) -> Option<[u32; 4]> {
    let v = img.text.iter().find_map(|(k, v)| crop_value(k, v))?; // VUGPHOTOCROPMETAXMP (SR631): the CROP_KEY pair, or the one inside an XMP packet, first in file order
    let mut n = v.split(',').map(|t| t.trim().parse::<u32>().ok());
    let (x, y, w, h) = (n.next()??, n.next()??, n.next()??, n.next()??);
    if n.next().is_some() || w == 0 || h == 0 {
        return None;
    }
    Some([x, y, w, h])
}

/// THE writer of the key: the (keyword, value) pair a crop of `img` at offset (`x0`, `y0`) carries. The frame is `img`'s
/// own when it states one ([`crop_frame`]: its offset grows by the crop's), else `img` itself, so the principal point
/// [`crop_principal`] reads back from the crop is the one `img` had, less the offset.
pub fn crop_pair(img: &Image, x0: u32, y0: u32) -> (alloc::string::String, alloc::string::String) {
    let [x, y, w, h] = crop_frame(img).unwrap_or([0, 0, img.width, img.height]);
    (alloc::string::String::from(CROP_KEY), alloc::format!("{},{},{},{}", x.saturating_add(x0), y.saturating_add(y0), w, h))
}

// ---- VUGPHOTOSIDETHIN (SR651): a pair off the lattice is a thin side (named tail block) ----
// An ink stroke lies ON the paper's lattice; a part thinner than MAX_STROKE_PX pairs like a stroke but lies off it. The
// silhouette's openings (the stroke-width square, then pitch / 8) erase both, so the ONE rule is the pair's position in
// lattice paper mm: a pair whose centre line keeps further than the stroke width from every lattice line parallel to it
// is a thin side, and the dark pixels in its band that connect to the part are the part's again. No off pair: the
// silhouette is returned untouched (the SR503 road bit for bit).

/// The band's margin about a pair (photo px): the edge lines sit at the blurred edge's half height (the fixture's 3 px box).
const BLUR_PX: f64 = 1.5;

/// The pair's dense run along `dir`: the longest stretch of its sorted support with no gap wider than the pair's own
/// `width_px`. Not its first / last edgel: `grid` refines each edge line over every agreeing edgel on the infinite line,
/// so a short side's support carries stray collinear edgels from across the sheet (measured on the shim fixture: 396
/// edgels spanning 95 mm for a 20 mm shim).
fn pair_run(pair: &grid::GridLine) -> Option<(f64, f64)> {
    let s = &pair.support;
    let first = *s.first()?;
    let (mut best, mut start, mut prev) = ((first, first), first, first);
    for &t in &s[1..] {
        if t - prev > pair.width_px {
            start = t;
        }
        prev = t;
        if prev - start > best.1 - best.0 {
            best = (start, prev);
        }
    }
    Some(best)
}

/// The pair's dense run's ends (photo px): `point + dir · t` at the run's first and last parameter.
fn pair_ends(pair: &grid::GridLine) -> Option<(Vec2, Vec2)> {
    let (t0, t1) = pair_run(pair)?;
    Some((pair.point + pair.dir * t0, pair.point + pair.dir * t1))
}

/// Whether a stroke pair is off every lattice line by more than `width_mm`: its support ends through `h_inv` (photo px to
/// lattice paper mm; the dense run's, `pair_run`), the coordinate across the nearer-parallel family, and no `k · pitch` in [min − width, max + width].
pub fn off_lattice(pair: &grid::GridLine, h_inv: &homography::H, pitch: f64, width_mm: f64) -> bool {
    let Some((e0, e1)) = pair_ends(pair) else { return false };
    let (Some(p0), Some(p1)) = (homography::apply(h_inv, e0), homography::apply(h_inv, e1)) else { return false };
    let d = p1 - p0;
    let (c0, c1) = if d.x.abs() >= d.y.abs() { (p0.y, p1.y) } else { (p0.x, p1.x) };
    let (lo, hi) = (c0.min(c1) - width_mm, c0.max(c1) + width_mm);
    if !(lo.is_finite() && hi.is_finite()) {
        return false;
    }
    libm::ceil(lo / pitch) > libm::floor(hi / pitch)
}

/// The silhouette with each off-lattice pair's dark band put back where it connects to the part: a rectified pixel that
/// is dark (at or under the silhouette's Otsu threshold), in the photo with its 8 neighbours, off the border, not a hole,
/// and within width / 2 + [`BLUR_PX`] of an off pair's centre line along its dense run widened by the pair's width at
/// each end (a side's edges turn over its own width at a corner, so its edgels stop that far short of the body); kept when `silhouette::label`
/// (8-connected) puts it in the part's component. [`Refusal::PartClipped`] when a kept pixel is on the border.
pub fn thin_sides(
    sil: silhouette::Silhouette,
    gray: &edges::Gray,
    lat: &homography::Lattice,
    pairs: &[grid::GridLine],
    pitch: f64,
    width_mm: f64,
) -> Result<silhouette::Silhouette, Refusal> {
    let Ok(h_inv) = homography::invert(&lat.h) else { return Ok(sil) };
    let mut bands: Vec<(&grid::GridLine, f64, f64)> = pairs
        .iter()
        .filter(|p| off_lattice(p, &h_inv, pitch, width_mm))
        .filter_map(|p| pair_run(p).map(|(t0, t1)| (p, t0, t1)))
        .collect();
    // VUGPHOTOSIDETHINCROSS (SR654): an on-lattice pair's stretch darker than the sheet's ink is a thin side lying on the line.
    if let Some((_, _, cut)) = ink_cut(pairs, gray, &sil, &h_inv, pitch, width_mm) {
        let min_px = pitch / 8.0 * lat.px_per_mm;
        for p in pairs.iter().filter(|p| !off_lattice(p, &h_inv, pitch, width_mm)) {
            bands.extend(dark_pair(p, gray, &sil, &h_inv, cut, min_px).into_iter().map(|(t0, t1)| (p, t0, t1)));
        }
    }
    if bands.is_empty() {
        return Ok(sil);
    }
    let (w, h) = (sil.w, sil.h);
    let photo = |u: i64, v: i64| -> Option<(Vec2, f32)> {
        let p = sil.corner_mm(u as f64 + 0.5, v as f64 + 0.5);
        let q = homography::apply(&lat.h, p)?;
        gray.sample(q.x, q.y).map(|l| (q, l))
    };
    let in_band = |q: Vec2| -> bool {
        bands.iter().any(|&(pr, t0, t1)| {
            let d = q - pr.point;
            let t = d.dot(pr.dir);
            d.dot(pr.dir.perp()).abs() <= 0.5 * pr.width_px + BLUR_PX && t >= t0 - pr.width_px && t <= t1 + pr.width_px
        })
    };
    let mut mask = sil.part.clone();
    let mut cand = alloc::vec![false; w * h];
    let mut any = false;
    for v in 1..h.saturating_sub(1) {
        for u in 1..w.saturating_sub(1) {
            let i = v * w + u;
            if sil.part[i] || sil.hole[i] != 0 {
                continue;
            }
            let Some((q, l)) = photo(u as i64, v as i64) else { continue };
            if (l.clamp(0.0, 255.0) as usize) > sil.threshold || !in_band(q) {
                continue;
            }
            let inside = (-1i64..=1).all(|dv| (-1i64..=1).all(|du| photo(u as i64 + du, v as i64 + dv).is_some()));
            if inside {
                cand[i] = true;
                mask[i] = true;
                any = true;
            }
        }
    }
    if !any {
        return Ok(sil);
    }
    let Some(seed) = sil.part.iter().position(|&p| p) else { return Ok(sil) };
    let lab = silhouette::label(&mask, w, h, true);
    let root = lab[seed];
    let mut out = sil;
    for i in 0..w * h {
        if cand[i] && lab[i] == root {
            let (u, v) = (i % w, i / w);
            if u == 0 || v == 0 || u == w - 1 || v == h - 1 {
                return Err(Refusal::PartClipped);
            }
            out.part[i] = true;
        }
    }
    Ok(out)
}

// ---- VUGPHOTOSIDETHINCROSS (SR654): a pair ON a lattice line whose core is darker than ink (named tail block) ----
// A thin part lying on (or crossing) a lattice line has no pair of its own: its edges sit within refine's band of the
// ink's (a 0.5 mm shim on a 0.4 mm line: 0.4 px apart at ~8 px / mm), so its edgels refine INTO the line's pair and
// `off_lattice`'s position rule keeps it ink. The tell is its tone, per step along the pair: the core (the darkest
// pixel across the pair's own width) against the sheet's ink tone (the median core of the on-lattice pairs' dense runs)
// and its spread (their pooled MAD). A stretch darker than ink by more than DARK_K spreads, at least pitch / 8 long,
// is a band for `thin_sides`' one restore, which keeps only the Otsu-dark pixels connected to the part (the ink line's
// own pixels sit above Otsu and stay ink).
// VUGPHOTOSIDETHINEDGE (SR662): `dark_pair` reads a step whose cross-section touches the part on its half AWAY from the
// part (`core_luma`'s split), so a thin side alongside the plate's edge inside the skip zone is seen by its tone.

/// The tone tell's margin in spreads: ink to the part is ~40 spreads on the fixture (ink ~168, part ~40, MAD ~3).
const DARK_K: f64 = 10.0;

/// The pair's core luma at parameter `t`: the darkest bilinear sample across |s| <= width_px / 2. None when any point of
/// the cross-section widened by 2 · [`BLUR_PX`] is off the photo or in the silhouette's part (a step under the part, or
/// whose blur reach touches its edge, says nothing about the line: line / plate junctions, an edge alongside a line).
/// VUGPHOTOSIDETHINEDGE (SR662): with `split`, a cross-section touching the part on ONE side only, and only past
/// 2 · [`BLUR_PX`] from the centre, is read on its half away from the part (|s| in [0, width_px / 2] on that side, clear of
/// the part by the same widening): a part sample within 2 · BLUR_PX of the centre, or on both sides, is still None.
fn core_luma(pair: &grid::GridLine, gray: &edges::Gray, sil: &silhouette::Silhouette, h_inv: &homography::H, t: f64, split: bool) -> Option<f32> {
    let c = pair.point + pair.dir * t;
    let n = pair.dir.perp();
    let half = 0.5 * pair.width_px;
    let reach = half + 2.0 * BLUR_PX;
    let k = libm::ceil(reach) as i64;
    let (mut lo, mut hi) = (false, false); // VUGPHOTOSIDETHINEDGE (SR662): the part touched on the -n / +n side
    for j in -k..=k {
        let s = (j as f64).clamp(-reach, reach);
        let q = c + n * s;
        gray.sample(q.x, q.y)?;
        let p = homography::apply(h_inv, q)?;
        let (u, v) = (libm::floor((p.x - sil.x0) * sil.r), libm::floor((p.y - sil.y0) * sil.r));
        if u >= 0.0 && v >= 0.0 && (u as usize) < sil.w && (v as usize) < sil.h && sil.part[v as usize * sil.w + u as usize] {
            if !split || s.abs() <= 2.0 * BLUR_PX {
                return None;
            }
            if s < 0.0 {
                lo = true;
            } else {
                hi = true;
            }
        }
    }
    let (a, b) = match (lo, hi) {
        (true, true) => return None,
        (true, false) => (0.0, half),
        (false, true) => (-half, 0.0),
        (false, false) => (-half, half),
    };
    let m = libm::ceil(b - a).max(1.0) as usize;
    let mut best = f32::INFINITY;
    for j in 0..=m {
        let q = c + n * (a + (b - a) * j as f64 / m as f64);
        best = best.min(gray.sample(q.x, q.y)?);
    }
    Some(best)
}

/// The sheet's ink tone, its spread and the cut `(ink, spread, ink - DARK_K · spread)`: [`core_luma`] at 1 px steps along
/// every on-lattice pair's dense run (`pair_run`), the median and the MAD of the pooled samples (the spread floored at one
/// luma quantum). None with no sample.
pub fn ink_cut(
    pairs: &[grid::GridLine],
    gray: &edges::Gray,
    sil: &silhouette::Silhouette,
    h_inv: &homography::H,
    pitch: f64,
    width_mm: f64,
) -> Option<(f64, f64, f64)> {
    let mut v: Vec<f64> = Vec::new();
    for p in pairs.iter().filter(|p| !off_lattice(p, h_inv, pitch, width_mm)) {
        let Some((t0, t1)) = pair_run(p) else { continue };
        let n = libm::floor(t1 - t0) as usize;
        v.extend((0..=n).filter_map(|k| core_luma(p, gray, sil, h_inv, t0 + k as f64, false)).map(f64::from)); // VUGPHOTOSIDETHINEDGE (SR662): whole clean cross-sections only
    }
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let ink = v[v.len() / 2];
    let mut dev: Vec<f64> = v.iter().map(|x| (x - ink).abs()).collect();
    dev.sort_by(|a, b| a.total_cmp(b));
    let spread = dev[dev.len() / 2].max(1.0);
    Some((ink, spread, ink - DARK_K * spread))
}

/// The pair's dark stretches `(t0, t1)` (photo px along `dir`): over its WHOLE support span at 1 px, the runs of steps whose
/// [`core_luma`] is under `cut` (a skipped or light step ends a run), kept when at least `min_px` long; the core read with
/// its split (VUGPHOTOSIDETHINEDGE SR662: the half away from the part where the part touches one side only).
pub fn dark_pair(
    pair: &grid::GridLine,
    gray: &edges::Gray,
    sil: &silhouette::Silhouette,
    h_inv: &homography::H,
    cut: f64,
    min_px: f64,
) -> Vec<(f64, f64)> {
    let mut out = Vec::new();
    let (Some(&a), Some(&b)) = (pair.support.first(), pair.support.last()) else { return out };
    let n = libm::floor(b - a) as usize;
    let mut run: Option<(f64, f64)> = None;
    for k in 0..=n {
        let t = a + k as f64;
        if core_luma(pair, gray, sil, h_inv, t, true).is_some_and(|l| (l as f64) < cut) {
            run = Some(run.map_or((t, t), |(s, _)| (s, t)));
        } else if let Some((s, e)) = run.take() {
            if e - s >= min_px {
                out.push((s, e));
            }
        }
    }
    if let Some((s, e)) = run {
        if e - s >= min_px {
            out.push((s, e));
        }
    }
    out
}
