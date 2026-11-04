// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The synthetic photo: a known part on known graph paper seen by a known camera, so every measurement has a
//! truth (the tests, tools/photo's test and VUG-FACE-PHOTO's test read it). Paper: a [`PITCH`] mm grid of
//! [`INK_MM`] light-blue ink lines on off-white. Part: a [`PART_W`] x [`PART_H`] mm dark plate with ONE
//! [`CORNER_R`] corner (at its +x +y corner) and two [`HOLE_D`] holes at [`HOLES`], in the part's own frame, laid on
//! the paper at `part_at` turned `part_rot_deg`. Camera: a pinhole of `focal_px` at `distance_mm` above the paper
//! point `centre`, the paper tilted `tilt_deg` about an in-paper axis at `tilt_axis_deg` (the photo's y runs down;
//! the paper's y up). Each pixel is the mean of 4 x 4 samples, then a 3 x 3 box blur (the 3 px blur), then Gaussian
//! noise of `noise` luma (splitmix64 + Box-Muller, seeded). Pure: a value in, an `Image` (or PNG bytes) out.
//! The lens (`k1`, VUG-PHOTO-LENS SR533) takes each sample through `homography::undistort` before the camera, so the
//! measurement's own model renders the bowed grid exactly.
//! The side view (VUG-PHOTO-SIDE SR545): [`render_side`] lays the part on its side, a [`PART_W`] x [`PART_T`] rectangle.
//! The raised side (VUG-PHOTO-SIDE-PARALLAX SR568): [`render_side_raised`] stands the part on that face, its top
//! `standing_mm` above the paper, each ray from the [`camera`] centre tested against the box.
//! The cropped frame (VUG-PHOTO-PP SR569): [`crop`] cuts a window of a render, its principal point off the centre.

use alloc::vec;
use alloc::vec::Vec;

use geom_core::Vec3;

use crate::homography::{H, SplitMix, apply, invert, mul, undistort};
use crate::{Image, Vec2};

pub const PITCH: f64 = 5.0;
pub const INK_MM: f64 = 0.4;
pub const PART_W: f64 = 50.0;
pub const PART_H: f64 = 30.0;
pub const CORNER_R: f64 = 10.0;
pub const HOLE_D: f64 = 6.0;
/// The hole centres in the part frame (mm).
pub const HOLES: [(f64, f64); 2] = [(12.0, 15.0), (30.0, 15.0)];

const PAPER: [f64; 3] = [245.0, 245.0, 240.0];
const INK: [f64; 3] = [150.0, 175.0, 220.0];
const PART: [f64; 3] = [40.0, 40.0, 45.0];

/// The scene and the camera.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Spec {
    pub width: u32,
    pub height: u32,
    pub focal_px: f64,
    pub distance_mm: f64,
    /// The paper point on the optical axis.
    pub centre: Vec2,
    pub tilt_deg: f64,
    pub tilt_axis_deg: f64,
    /// The part frame's origin on the paper, and its turn (degrees, CCW).
    pub part_at: Vec2,
    pub part_rot_deg: f64,
    pub grid: bool,
    pub part: bool,
    /// Gaussian noise, luma units.
    pub noise: f64,
    pub seed: u64,
    /// VUG-PHOTO-LENS (SR533): the lens's Brown radial term (`homography::distort`); 0.0 is a perfect lens.
    pub k1: f64,
}

impl Spec {
    /// The measurable photo: 900 x 720 px, ~8 px / mm, a 12° tilt about an axis 30° off the grid, the part 1.5° off
    /// the paper's axes and off-centre.
    pub fn good() -> Spec {
        Spec {
            width: 900,
            height: 720,
            focal_px: 1400.0,
            distance_mm: 175.0,
            centre: Vec2::new(52.3, 41.7),
            tilt_deg: 12.0,
            tilt_axis_deg: 30.0,
            part_at: Vec2::new(33.3, 23.7),
            part_rot_deg: 1.5,
            grid: true,
            part: true,
            noise: 6.0,
            seed: 0x0503_F1A7_0000_0001,
            k1: 0.0,
        }
    }

    /// The same scene under a 35° tilt (the camera far from square).
    pub fn unsquare() -> Spec {
        Spec { tilt_deg: 35.0, ..Spec::good() }
    }

    /// VUG-PHOTO-LENS (SR533): the same scene through a barrel lens, k1 = −0.08 (the grid lines bow).
    pub fn lens() -> Spec {
        Spec { k1: -0.08, ..Spec::good() }
    }

    /// The part on blank paper.
    pub fn no_grid() -> Spec {
        Spec { grid: false, ..Spec::good() }
    }

    /// The part slid over the photo's left border.
    pub fn clipped() -> Spec {
        Spec { part_at: Vec2::new(52.3 - 75.0, 23.7), ..Spec::good() }
    }
}

fn rot(v: Vec2, deg: f64) -> Vec2 {
    let (s, c) = (libm::sin(deg.to_radians()), libm::cos(deg.to_radians()));
    Vec2::new(c * v.x - s * v.y, s * v.x + c * v.y)
}

/// A part-frame point on the paper.
pub fn part_to_paper(s: &Spec, q: Vec2) -> Vec2 {
    rot(q, s.part_rot_deg) + s.part_at
}

/// A paper point in the part frame.
pub fn paper_to_part(s: &Spec, p: Vec2) -> Vec2 {
    rot(p - s.part_at, -s.part_rot_deg)
}

/// Whether a part-frame point is in the part (plate minus the rounded corner's outside, minus the holes).
pub fn in_part(q: Vec2) -> bool {
    if !(q.x >= 0.0 && q.y >= 0.0 && q.x <= PART_W && q.y <= PART_H) {
        return false;
    }
    let k = Vec2::new(PART_W - CORNER_R, PART_H - CORNER_R);
    if q.x > k.x && q.y > k.y && q.dist(k) > CORNER_R {
        return false;
    }
    HOLES.iter().all(|&(x, y)| q.dist(Vec2::new(x, y)) >= HOLE_D * 0.5)
}

/// The distance (mm) of a part-frame point from the part's OUTER boundary.
pub fn outline_distance(q: Vec2) -> f64 {
    let k = Vec2::new(PART_W - CORNER_R, PART_H - CORNER_R);
    if q.x > k.x && q.y > k.y {
        return (q.dist(k) - CORNER_R).abs();
    }
    let inside = q.x >= 0.0 && q.y >= 0.0 && q.x <= PART_W && q.y <= PART_H;
    if inside {
        q.x.min(PART_W - q.x).min(q.y).min(PART_H - q.y)
    } else {
        let dx = (-q.x).max(q.x - PART_W).max(0.0);
        let dy = (-q.y).max(q.y - PART_H).max(0.0);
        libm::hypot(dx, dy)
    }
}

/// The true homography, paper mm → photo px (row-major, `px ~ H [x, y, 1]`).
pub fn homography(s: &Spec) -> H {
    let (r1, r2) = basis(s);
    let (f, d) = (s.focal_px, s.distance_mm);
    let (u0, v0) = (s.width as f64 * 0.5, s.height as f64 * 0.5);
    let hp = [
        [f * r1.x - u0 * r1.z, f * r2.x - u0 * r2.z, u0 * d],
        [-f * r1.y - v0 * r1.z, -f * r2.y - v0 * r2.z, v0 * d],
        [-r1.z, -r2.z, d],
    ];
    mul(&hp, &[[1.0, 0.0, -s.centre.x], [0.0, 1.0, -s.centre.y], [0.0, 0.0, 1.0]])
}

/// The paper axes in the camera's frame: the tilt's Rodrigues rotation of x and y (VUG-PHOTO-SIDE-PARALLAX SR568 shares
/// it with [`camera`]).
fn basis(s: &Spec) -> (Vec3, Vec3) {
    let a = s.tilt_axis_deg.to_radians();
    let t = s.tilt_deg.to_radians();
    let axis = Vec3::new(libm::cos(a), libm::sin(a), 0.0);
    let rodrigues = |v: Vec3| v * libm::cos(t) + axis.cross(v) * libm::sin(t) + axis * (axis.dot(v) * (1.0 - libm::cos(t)));
    (rodrigues(Vec3::X), rodrigues(Vec3::Y))
}

fn near_line(v: f64) -> bool {
    (v - PITCH * libm::round(v / PITCH)).abs() <= INK_MM * 0.5
}

fn colour(s: &Spec, inside: &dyn Fn(Vec2) -> bool, p: Vec2) -> [f64; 3] {
    if s.part && inside(paper_to_part(s, p)) {
        PART
    } else if s.grid && (near_line(p.x) || near_line(p.y)) {
        INK
    } else {
        PAPER
    }
}

/// Render the photo.
pub fn render(s: &Spec) -> Image {
    render_shape(s, &in_part)
}

/// Render the photo of the part whose part-frame region is `inside` (VUG-PHOTO-SIDE SR545: [`render`] is the plate).
pub fn render_shape(s: &Spec, inside: &dyn Fn(Vec2) -> bool) -> Image {
    let (w, h) = (s.width as usize, s.height as usize);
    let hinv = invert(&homography(s)).unwrap_or([[0.0; 3]; 3]);
    let mut rgb = vec![0.0f64; w * h * 3];
    for j in 0..h {
        for i in 0..w {
            let mut acc = [0.0; 3];
            for b in 0..4 {
                for a in 0..4 {
                    let q = Vec2::new(i as f64 + (a as f64 + 0.5) / 4.0, j as f64 + (b as f64 + 0.5) / 4.0);
                    let q = undistort(q, s.k1, w, h); // VUG-PHOTO-LENS (SR533): the identity at k1 = 0.0
                    let c = apply(&hinv, q).map(|p| colour(s, inside, p)).unwrap_or(PAPER);
                    for k in 0..3 {
                        acc[k] += c[k];
                    }
                }
            }
            for k in 0..3 {
                rgb[(j * w + i) * 3 + k] = acc[k] / 16.0;
            }
        }
    }
    let mut blur = vec![0.0f64; w * h * 3];
    for j in 0..h {
        for i in 0..w {
            for k in 0..3 {
                let mut sum = 0.0;
                for dj in -1i64..=1 {
                    for di in -1i64..=1 {
                        let ii = (i as i64 + di).clamp(0, w as i64 - 1) as usize;
                        let jj = (j as i64 + dj).clamp(0, h as i64 - 1) as usize;
                        sum += rgb[(jj * w + ii) * 3 + k];
                    }
                }
                blur[(j * w + i) * 3 + k] = sum / 9.0;
            }
        }
    }
    let mut rng = SplitMix::new(s.seed);
    let mut rgba = vec![255u8; w * h * 4];
    for px in 0..w * h {
        for k in 0..3 {
            let u1 = 1.0 - rng.next_f64();
            let u2 = rng.next_f64();
            let z = libm::sqrt(-2.0 * libm::log(u1)) * libm::cos(2.0 * core::f64::consts::PI * u2);
            rgba[px * 4 + k] = libm::round(blur[px * 3 + k] + s.noise * z).clamp(0.0, 255.0) as u8;
        }
    }
    Image { width: s.width, height: s.height, rgba, frames: None, loop_count: None, orientation: 1, icc: None, text: Vec::new() }
}

/// The photo as PNG bytes (pixel_core's encoder).
pub fn png(s: &Spec) -> Vec<u8> {
    let img = render(s);
    pixel_core::png::encode::encode_rgba(img.width, img.height, &img.rgba).unwrap_or_default()
}

/// The measurable photo ([`Spec::good`]) as PNG bytes: the one fixture every consumer reads.
pub fn photo_png() -> Vec<u8> {
    png(&Spec::good())
}

// VUG-PHOTO-SIDE (SR545): the same part photographed on its side, through the same renderer.
/// The plate's thickness (mm): the fixture part is [`PART_W`] x [`PART_H`] x [`PART_T`].
pub const PART_T: f64 = 6.0;

/// Whether a part-frame point is in the side view: the rectangle `0..length_mm` x `0..thickness_mm`.
pub fn in_side(q: Vec2, length_mm: f64, thickness_mm: f64) -> bool {
    q.x >= 0.0 && q.y >= 0.0 && q.x <= length_mm && q.y <= thickness_mm
}

impl Spec {
    /// The good camera over the part on its side, the strip centred under the optical axis.
    pub fn side() -> Spec {
        Spec { part_at: Vec2::new(27.3, 38.7), seed: 0x0545_51DE_0000_0001, ..Spec::good() }
    }
}

/// Render the side photo of a `length_mm` x `thickness_mm` side.
pub fn render_side(s: &Spec, length_mm: f64, thickness_mm: f64) -> Image {
    render_shape(s, &|q: Vec2| in_side(q, length_mm, thickness_mm))
}

/// The side photo as PNG bytes (pixel_core's encoder).
pub fn side_png(s: &Spec, length_mm: f64, thickness_mm: f64) -> Vec<u8> {
    let img = render_side(s, length_mm, thickness_mm);
    pixel_core::png::encode::encode_rgba(img.width, img.height, &img.rgba).unwrap_or_default()
}

// VUG-PHOTO-SIDE-PARALLAX (SR568): the part standing on its side, its top above the paper, through the same renderer.
/// The camera centre in paper mm: `distance_mm` back along the optical axis from `centre` (x, y, and the height z).
pub fn camera(s: &Spec) -> Vec3 {
    let (r1, r2) = basis(s);
    let d = s.distance_mm;
    Vec3::new(s.centre.x + d * r1.z, s.centre.y + d * r2.z, d * r1.cross(r2).z)
}

impl Spec {
    /// The side camera at 400 mm (the focal raised to 3200 px to keep ~8 px / mm), the arc's raised-edge camera.
    pub fn side_raised() -> Spec {
        Spec { focal_px: 3200.0, distance_mm: 400.0, ..Spec::side() }
    }

    /// [`Spec::side_raised`] square to the paper (no tilt), the strip off the nadir on the paper's +y side.
    pub fn side_square() -> Spec {
        Spec { tilt_deg: 0.0, part_at: Vec2::new(27.3, 48.7), ..Spec::side_raised() }
    }
}

/// Whether the segment `p0`-`p1` meets the rectangle `0..w` x `0..h` (Liang-Barsky).
fn segment_hits_rect(p0: Vec2, p1: Vec2, w: f64, h: f64) -> bool {
    let d = p1 - p0;
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    for (p, q) in [(-d.x, p0.x), (d.x, w - p0.x), (-d.y, p0.y), (d.y, h - p0.y)] {
        if p == 0.0 {
            if q < 0.0 {
                return false;
            }
        } else {
            let r = q / p;
            if p < 0.0 {
                if r > t1 {
                    return false;
                }
                t0 = t0.max(r);
            } else {
                if r < t0 {
                    return false;
                }
                t1 = t1.min(r);
            }
        }
    }
    true
}

/// Render the side photo of a `length_mm` x `thickness_mm` side standing `standing_mm` tall: a sample is the part
/// when its ray from the camera meets the box (in the part frame, the ray's run from the paper up to `standing_mm`).
pub fn render_side_raised(s: &Spec, length_mm: f64, thickness_mm: f64, standing_mm: f64) -> Image {
    let c = camera(s);
    let cq = paper_to_part(s, Vec2::new(c.x, c.y));
    let a = standing_mm / c.z;
    render_shape(s, &|q: Vec2| segment_hits_rect(q, q + (cq - q) * a, length_mm, thickness_mm))
}

/// The raised side photo as PNG bytes (pixel_core's encoder).
pub fn side_raised_png(s: &Spec, length_mm: f64, thickness_mm: f64, standing_mm: f64) -> Vec<u8> {
    let img = render_side_raised(s, length_mm, thickness_mm, standing_mm);
    pixel_core::png::encode::encode_rgba(img.width, img.height, &img.rgba).unwrap_or_default()
}

// VUG-PHOTO-PP (SR569): a cropped photo, the camera's principal point no longer the frame's centre.
/// The `w` x `h` window of `img` at (`x0`, `y0`) (clamped to the image): the photo a crop of the render is, its
/// principal point at the render's centre less (`x0`, `y0`).
pub fn crop(img: &Image, x0: u32, y0: u32, w: u32, h: u32) -> Image {
    let x0 = x0.min(img.width);
    let y0 = y0.min(img.height);
    let w = w.min(img.width - x0);
    let h = h.min(img.height - y0);
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for j in y0..y0 + h {
        let row = (j as usize * img.width as usize + x0 as usize) * 4;
        rgba.extend_from_slice(&img.rgba[row..row + w as usize * 4]);
    }
    Image { width: w, height: h, rgba, frames: None, loop_count: None, orientation: 1, icc: None, text: Vec::new() } // VUGPHOTOCROPMETA (SR582): meta-free in memory (crop_png carries it)
}

// ---- VUGPHOTOCROPMETA (SR582): a crop that carries its own principal point (named tail block) ----

/// [`crop`] as PNG bytes carrying its crop as the tEXt [`crate::CROP_KEY`] = `<x0>,<y0>,<W>,<H>` (the clamped offset
/// [`crop`] used and `img`'s size), so the photo states its principal point ([`crate::crop_principal`]) with no pref.
pub fn crop_png(img: &Image, x0: u32, y0: u32, w: u32, h: u32) -> Vec<u8> {
    let c = crop(img, x0, y0, w, h);
    let (key, meta) = crate::crop_pair(img, x0.min(img.width), y0.min(img.height)); // VUGFACEPHOTOCROPSAVE (SR640): the key written once
    pixel_core::png::encode::encode_rgba_text(c.width, c.height, &c.rgba, &[(key.as_str(), meta.as_str())]).unwrap_or_default()
}

// ---- VUGPHOTOSIDETHIN (SR651): the plate with a thin shim off its left side (named tail block) ----
/// The shim's width (mm): 4 px at the good camera's ~8 px / mm, under MAX_STROKE_PX (it pairs like ink) and under both
/// silhouette openings, over the outline's 2 σ simplification (both its sides survive into the loops).
pub const SHIM_W: f64 = 0.5;
/// The shim's length out from the plate's left side (mm).
pub const SHIM_L: f64 = 20.0;
/// The shim's centre line in the part frame (mm): on [`Spec::good`] it runs paper y 37.2 to 37.7, mid-cell.
pub const SHIM_Y: f64 = 14.0;

/// Whether a part-frame point is in the shim: x in [-SHIM_L, 0] (touching the plate's left side), y in SHIM_Y ± SHIM_W / 2.
pub fn in_shim(q: Vec2) -> bool {
    q.x >= -SHIM_L && q.x <= 0.0 && (q.y - SHIM_Y).abs() <= SHIM_W * 0.5
}

/// Render the plate with the shim.
pub fn render_shim(s: &Spec) -> Image {
    render_shape(s, &|q: Vec2| in_part(q) || in_shim(q))
}

/// The shim photo as PNG bytes (pixel_core's encoder).
pub fn shim_png(s: &Spec) -> Vec<u8> {
    let img = render_shim(s);
    pixel_core::png::encode::encode_rgba(img.width, img.height, &img.rgba).unwrap_or_default()
}

// ---- VUGPHOTOSIDETHINCROSS (SR654): the plate with the shim lying ON a lattice line (named tail block) ----
/// The lattice line the shim lies on (paper x, mm): on [`Spec::good`] it crosses the plate's bottom side at paper y ~24.0.
pub const SHIM_ON_X: f64 = 45.0;
/// The shim's tip (paper y, mm): mid-cell and inside the seen lattice; the shim runs [`SHIM_L`] up from it, under the plate.
pub const SHIM_ON_TIP: f64 = 7.5;

/// Whether a PAPER point is in the on-line shim: x in SHIM_ON_X ± SHIM_W / 2 (the 0.4 mm ink line under it), y in
/// [SHIM_ON_TIP, SHIM_ON_TIP + SHIM_L] (its top end under the plate, so it hangs off the plate's bottom side).
pub fn in_shim_on_line(p: Vec2) -> bool {
    (p.x - SHIM_ON_X).abs() <= SHIM_W * 0.5 && p.y >= SHIM_ON_TIP && p.y <= SHIM_ON_TIP + SHIM_L
}

/// Render the plate with the on-line shim.
pub fn render_shim_on_line(s: &Spec) -> Image {
    render_shape(s, &|q: Vec2| in_part(q) || in_shim_on_line(part_to_paper(s, q)))
}

/// The on-line shim photo as PNG bytes (pixel_core's encoder).
pub fn shim_on_line_png(s: &Spec) -> Vec<u8> {
    let img = render_shim_on_line(s);
    pixel_core::png::encode::encode_rgba(img.width, img.height, &img.rgba).unwrap_or_default()
}

// ---- VUGPHOTOSIDETHINSPLIT (SR669): the shim with a light gap on its side, and its dense loop (named tail block) ----
/// The split probe's gap (mm): its length along the shim's side and its depth into the shim. The depth is 2.5 σ at the
/// photo's σ 0.125: past DP's 2 σ, inside the 3 σ band, leaving a 0.1875 mm bridge. Measured (SR669): no corner survives
/// and the fit absorbs it into the side's ONE line (its rms ~5 x the clean sides').
pub const SHIM_GAP: (f64, f64) = (0.25, 0.3125);

/// Whether a part-frame point is in the shim less a light gap `(along, depth)` centred at the side's mid-length
/// (x = -SHIM_L / 2) on the shim's upper side (y = SHIM_Y + SHIM_W / 2).
pub fn in_shim_gap(q: Vec2, gap: (f64, f64)) -> bool {
    in_shim(q) && !((q.x + SHIM_L * 0.5).abs() <= gap.0 * 0.5 && q.y > SHIM_Y + SHIM_W * 0.5 - gap.1)
}

/// The dense outer loop (paper mm, CCW) of the plate with the shim (less `gap` when given) laid on the paper by `s`,
/// traced at `r` px / mm by [`crate::contour::trace_region`] in contour::trace's y-up frame (crack midpoints at
/// `x0 + x / r`): a pure contour, no blur, no holes (the plate's holes are not this fixture's subject).
pub fn shim_loops(s: &Spec, r: f64, gap: Option<(f64, f64)>) -> crate::contour::Loops {
    let corners = [
        Vec2::new(-SHIM_L, 0.0),
        Vec2::new(PART_W, 0.0),
        Vec2::new(PART_W, PART_H),
        Vec2::new(-SHIM_L, PART_H),
    ];
    let paper: Vec<Vec2> = corners.iter().map(|&q| part_to_paper(s, q)).collect();
    let x0 = paper.iter().map(|p| p.x).fold(f64::INFINITY, f64::min) - 1.0;
    let y0 = paper.iter().map(|p| p.y).fold(f64::INFINITY, f64::min) - 1.0;
    let x1 = paper.iter().map(|p| p.x).fold(f64::NEG_INFINITY, f64::max) + 1.0;
    let y1 = paper.iter().map(|p| p.y).fold(f64::NEG_INFINITY, f64::max) + 1.0;
    let (w, h) = (libm::ceil((x1 - x0) * r) as usize, libm::ceil((y1 - y0) * r) as usize);
    let inside = |u: i64, v: i64| -> bool {
        if u < 0 || v < 0 || u as usize >= w || v as usize >= h {
            return false;
        }
        let q = paper_to_part(s, Vec2::new(x0 + (u as f64 + 0.5) / r, y0 + (v as f64 + 0.5) / r));
        in_part(q) || gap.map_or(in_shim(q), |g| in_shim_gap(q, g))
    };
    let outer = crate::contour::trace_region(&inside, w, h, true)
        .iter()
        .map(|l| l.iter().map(|&(x, y)| Vec2::new(x0 + x / r, y0 + y / r)).collect::<Vec<Vec2>>())
        .max_by(|a, b| geom_core::area(a).total_cmp(&geom_core::area(b)))
        .unwrap_or_default();
    crate::contour::Loops { outer, holes: vec![] }
}
