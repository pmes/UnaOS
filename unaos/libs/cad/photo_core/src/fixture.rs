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

use alloc::vec;
use alloc::vec::Vec;

use geom_core::Vec3;

use crate::homography::{H, SplitMix, apply, invert, mul};
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
        }
    }

    /// The same scene under a 35° tilt (the camera far from square).
    pub fn unsquare() -> Spec {
        Spec { tilt_deg: 35.0, ..Spec::good() }
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
    let a = s.tilt_axis_deg.to_radians();
    let t = s.tilt_deg.to_radians();
    let axis = Vec3::new(libm::cos(a), libm::sin(a), 0.0);
    let rodrigues = |v: Vec3| v * libm::cos(t) + axis.cross(v) * libm::sin(t) + axis * (axis.dot(v) * (1.0 - libm::cos(t)));
    let r1 = rodrigues(Vec3::X);
    let r2 = rodrigues(Vec3::Y);
    let (f, d) = (s.focal_px, s.distance_mm);
    let (u0, v0) = (s.width as f64 * 0.5, s.height as f64 * 0.5);
    let hp = [
        [f * r1.x - u0 * r1.z, f * r2.x - u0 * r2.z, u0 * d],
        [-f * r1.y - v0 * r1.z, -f * r2.y - v0 * r2.z, v0 * d],
        [-r1.z, -r2.z, d],
    ];
    mul(&hp, &[[1.0, 0.0, -s.centre.x], [0.0, 1.0, -s.centre.y], [0.0, 0.0, 1.0]])
}

fn near_line(v: f64) -> bool {
    (v - PITCH * libm::round(v / PITCH)).abs() <= INK_MM * 0.5
}

fn colour(s: &Spec, p: Vec2) -> [f64; 3] {
    if s.part && in_part(paper_to_part(s, p)) {
        PART
    } else if s.grid && (near_line(p.x) || near_line(p.y)) {
        INK
    } else {
        PAPER
    }
}

/// Render the photo.
pub fn render(s: &Spec) -> Image {
    let (w, h) = (s.width as usize, s.height as usize);
    let hinv = invert(&homography(s)).unwrap_or([[0.0; 3]; 3]);
    let mut rgb = vec![0.0f64; w * h * 3];
    for j in 0..h {
        for i in 0..w {
            let mut acc = [0.0; 3];
            for b in 0..4 {
                for a in 0..4 {
                    let q = Vec2::new(i as f64 + (a as f64 + 0.5) / 4.0, j as f64 + (b as f64 + 0.5) / 4.0);
                    let c = apply(&hinv, q).map(|p| colour(s, p)).unwrap_or(PAPER);
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
    Image { width: s.width, height: s.height, rgba, frames: None, loop_count: None, orientation: 1, icc: None }
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
