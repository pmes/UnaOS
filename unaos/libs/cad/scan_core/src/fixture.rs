// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The synthetic depth fixture: Z16 frames of a known scene through probe_core's pinhole model. No D435i frame has
//! been captured on metal (SONDE2 SR105 fixed-unflown; every flight reads `SKIP(no-device)`), so the fusion and the
//! fits are scored against a scene whose every dimension is known. A face can show the same frames.
//!
//! Each pixel's ray is `Intrinsics::deproject(u + 0.5, v + 0.5, 1.0)` (probe_core's model and pixel-centre
//! convention, the inverse of what [`crate::tsdf`] does with `project`), carried to the world by the camera pose;
//! the nearest hit's camera depth (the ray's camera z is 1, so the hit parameter IS the depth) gets the noise and is
//! quantised to the frame's depth scale. A miss is raw 0, the D400's "no depth here".
//!
//! Noise (docs/dev/evidence/vug/research-scan-and-kernels.md §1.1 / §1.3, the RealSense tuning guide's error law:
//! depth error grows with the square of the distance): Gaussian with σ(z) = σ_ref · (z / z_ref)², drawn by Box–Muller
//! over a seeded splitmix64. [`Noise::D435I`] is 1.5 mm at 0.6 m (Gee: 1.06 mm at 0.621 m; the 1-2 mm at 0.6 m the
//! research reads), random noise only: the calibration bias the research names is not modelled.
//!
//! VUGFACECONFIDENCETINT (SR592): [`Noise::invalid_band_px`] zeroes depth within that many pixels of a silhouette (a hit
//! beside a miss or beside a depth step over [`SILHOUETTE_STEP`]), the D400's invalid band; 0 (both consts) is the
//! render as before, byte for byte (the band pass runs after the noise stream, only when armed).

use alloc::vec;
use alloc::vec::Vec;

use probe_core::{DepthScale, Frame, Intrinsics, PixelFormat, StreamKind};

use crate::fit::SplitMix;
pub use crate::geom::from_axes;
use crate::geom::{Transform, Vec3};

/// An axis-aligned box (world mm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BoxPrim {
    pub min: Vec3,
    pub max: Vec3,
}

/// A cylinder standing on `base` along +z (world mm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CylPrim {
    pub base: Vec3,
    pub radius: f64,
    pub height: f64,
}

/// The scene: a ground rectangle (half-extents x, y) in the plane z = `ground_centre.z`, boxes and cylinders on it.
/// World mm, z up.
#[derive(Clone, Debug, PartialEq)]
pub struct Scene {
    pub ground_centre: Vec3,
    pub ground: [f64; 2],
    pub boxes: Vec<BoxPrim>,
    pub cylinders: Vec<CylPrim>,
}

/// The depth noise model: σ(z) = `sigma_ref_mm` · (z / `z_ref_m`)², Gaussian, from `seed`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Noise {
    pub sigma_ref_mm: f64,
    pub z_ref_m: f64,
    pub seed: u64,
    /// VUGFACECONFIDENCETINT (SR592): the silhouette's invalid band in pixels (0: none).
    pub invalid_band_px: u32,
}

impl Noise {
    /// The D435i at 848x480-class precision: 1.5 mm at 0.6 m.
    pub const D435I: Noise = Noise { sigma_ref_mm: 1.5, z_ref_m: 0.6, seed: 0xD435, invalid_band_px: 0 };
    /// No noise (only the scale's quantisation).
    pub const NONE: Noise = Noise { sigma_ref_mm: 0.0, z_ref_m: 0.6, seed: 0, invalid_band_px: 0 };
}

impl Scene {
    /// The arc's scene: ground 700 x 600, a box 300 x 200 x 100 (x -230..70, y -100..100) and a cylinder r 40 h 150
    /// standing at (160, 0).
    pub fn bench() -> Scene {
        Scene {
            ground_centre: Vec3::ZERO,
            ground: [350.0, 300.0],
            boxes: vec![BoxPrim { min: Vec3::new(-230.0, -100.0, 0.0), max: Vec3::new(70.0, 100.0, 100.0) }],
            cylinders: vec![CylPrim { base: Vec3::new(160.0, 0.0, 0.0), radius: 40.0, height: 150.0 }],
        }
    }

    /// The same scene moved by `d`.
    pub fn translated(&self, d: Vec3) -> Scene {
        Scene {
            ground_centre: self.ground_centre + d,
            ground: self.ground,
            boxes: self.boxes.iter().map(|b| BoxPrim { min: b.min + d, max: b.max + d }).collect(),
            cylinders: self.cylinders.iter().map(|c| CylPrim { base: c.base + d, ..*c }).collect(),
        }
    }

    fn ground_hit(&self, o: Vec3, d: Vec3) -> Option<f64> {
        let centre = self.ground_centre;
        if d.z == 0.0 {
            return None;
        }
        let t = (centre.z - o.z) / d.z;
        if !(t > 0.0) {
            return None;
        }
        let p = o + d * t;
        if (p.x - centre.x).abs() <= self.ground[0] && (p.y - centre.y).abs() <= self.ground[1] { Some(t) } else { None }
    }

    /// The nearest positive hit parameter of the ray `o + t·d`.
    pub fn raycast(&self, o: Vec3, d: Vec3) -> Option<f64> {
        let mut best = self.ground_hit(o, d);
        let mut keep = |t: f64| {
            let nearer = match best {
                Some(b) => t < b,
                None => true,
            };
            if t > 0.0 && nearer {
                best = Some(t);
            }
        };
        for b in &self.boxes {
            let (mut t0, mut t1) = (f64::NEG_INFINITY, f64::INFINITY);
            let mut hit = true;
            for (oa, da, lo, hi) in [(o.x, d.x, b.min.x, b.max.x), (o.y, d.y, b.min.y, b.max.y), (o.z, d.z, b.min.z, b.max.z)]
            {
                if da == 0.0 {
                    if oa < lo || oa > hi {
                        hit = false;
                    }
                    continue;
                }
                let (a, c) = ((lo - oa) / da, (hi - oa) / da);
                let (a, c) = if a < c { (a, c) } else { (c, a) };
                t0 = t0.max(a);
                t1 = t1.min(c);
            }
            if hit && t0 <= t1 {
                keep(t0);
            }
        }
        for c in &self.cylinders {
            let (fx, fy) = (o.x - c.base.x, o.y - c.base.y);
            let a = d.x * d.x + d.y * d.y;
            if a > 0.0 {
                let b = 2.0 * (fx * d.x + fy * d.y);
                let cc = fx * fx + fy * fy - c.radius * c.radius;
                let disc = b * b - 4.0 * a * cc;
                if disc >= 0.0 {
                    let t = (-b - libm::sqrt(disc)) / (2.0 * a);
                    let z = o.z + d.z * t;
                    if z >= c.base.z && z <= c.base.z + c.height {
                        keep(t);
                    }
                }
            }
            if d.z != 0.0 {
                let t = (c.base.z + c.height - o.z) / d.z;
                let p = o + d * t;
                let (px, py) = (p.x - c.base.x, p.y - c.base.y);
                if px * px + py * py <= c.radius * c.radius {
                    keep(t);
                }
            }
        }
        best
    }
}

/// The fixture's intrinsics: 424 x 240 (the D435 family's half-width mode) with fx = fy = 212 (a 90° horizontal
/// field, the tuning guide's figure) and the principal point at the centre.
pub fn bench_intrinsics() -> Intrinsics {
    Intrinsics { width: 424, height: 240, ppx: 212.0, ppy: 120.0, fx: 212.0, fy: 212.0 }
}

/// A camera on an orbit around `centre`: `standoff_mm` away, `elevation_rad` above the horizontal, at `azimuth_rad`
/// about +z, looking at `centre` with the world's +z up. Camera → world, probe_core's axes (+x right, +y down,
/// +z forward). An elevation of ±90° has no "right" and is the caller's to build with [`from_axes`].
pub fn orbit(centre: Vec3, standoff_mm: f64, elevation_rad: f64, azimuth_rad: f64) -> Transform {
    let (ce, se) = (libm::cos(elevation_rad), libm::sin(elevation_rad));
    let (ca, sa) = (libm::cos(azimuth_rad), libm::sin(azimuth_rad));
    let eye = centre + Vec3::new(ce * ca, ce * sa, se) * standoff_mm;
    let fwd = (centre - eye).normalize().unwrap_or(Vec3::ZERO);
    let right = fwd.cross(Vec3::new(0.0, 0.0, 1.0)).normalize().unwrap_or(Vec3::ZERO);
    let down = fwd.cross(right);
    from_axes(right, down, fwd, eye)
}

/// Render the Z16 depth frame `scene` shows a camera at `pose`, with `noise` drawn from its seed and `seq` (frame
/// `seq`'s stream), quantised to `scale`. Panics only on zero-sized intrinsics (`FrameError::Empty`), as
/// `probe_core::replay::depth_plane` does.
pub fn render_depth(
    scene: &Scene,
    intr: &Intrinsics,
    scale: DepthScale,
    pose: &Transform,
    noise: &Noise,
    seq: u64,
) -> Frame {
    let (w, h) = (intr.width, intr.height);
    let mut data = vec![0u8; w as usize * h as usize * 2];
    let mut rng = SplitMix::new(noise.seed ^ seq.wrapping_mul(0x9E37_79B9_7F4A_7C15));
    let origin = pose.trans;
    let unit = scale.metres_per_unit as f64 * 1000.0; // mm per raw unit
    let band = noise.invalid_band_px; // VUGFACECONFIDENCETINT (SR592): the noise-free hit depth per pixel, kept only when the band is armed
    let mut hit = if band > 0 { vec![0.0f64; w as usize * h as usize] } else { Vec::new() };
    for y in 0..h {
        for x in 0..w {
            // Box–Muller: two uniforms per pixel, drawn whether or not the pixel hits, so the stream is per-pixel.
            let (u1, u2) = (1.0 - rng.next_f64(), rng.next_f64());
            let g = libm::sqrt(-2.0 * libm::log(u1)) * libm::cos(2.0 * core::f64::consts::PI * u2);
            let Some(c) = intr.deproject(x as f32 + 0.5, y as f32 + 0.5, 1.0) else { continue };
            let dir = pose.apply_vector(Vec3::new(c[0] as f64, c[1] as f64, 1.0));
            let Some(z_mm) = scene.raycast(origin, dir) else { continue };
            if let Some(hz) = hit.get_mut(y as usize * w as usize + x as usize) {
                *hz = z_mm;
            }
            let zr = z_mm / 1000.0 / noise.z_ref_m;
            let z = z_mm + noise.sigma_ref_mm * zr * zr * g;
            if !(unit > 0.0) || !(z > 0.0) {
                continue;
            }
            let raw = libm::floor(z / unit + 0.5);
            let raw = if raw < 1.0 { 1u16 } else if raw > 65535.0 { 65535u16 } else { raw as u16 };
            let o = (y as usize * w as usize + x as usize) * 2;
            data[o..o + 2].copy_from_slice(&raw.to_le_bytes());
        }
    }
    if band > 0 {
        invalid_band(&mut data, &hit, w, h, band);
    }
    Frame::packed(StreamKind::Depth, PixelFormat::Z16, w, h, seq * 33_333, seq, data)
        .expect("render_depth sizes its own buffer")
}

// ---- VUGFACECONFIDENCETINT (LEDGER SR592): the D400's invalid band beside a silhouette ----
// The D400 returns no depth in a band beside an object's outline (the stereo pair's occlusion), so a real frame carries
// holes right where the TSDF's silhouette error lives (VUG-SCAN-TSDF-HOLES); without them the fixture had a hole beside 0
// of the post's 40 form rows (SR586). A silhouette pixel is a hit whose 4-neighbour is a miss or a hit whose noise-free
// depth differs by more than [`SILHOUETTE_STEP`] of the nearer (the bench ground's adjacent-pixel step stays under 1.5% at
// its grazing angles; the post's and the box's outlines step 20% and more); every pixel within Chebyshev distance under
// the band of a silhouette pixel reads raw 0.

/// The relative depth step between 4-neighbours that makes a silhouette (5% of the nearer depth).
pub const SILHOUETTE_STEP: f64 = 0.05;

/// Zero every pixel of the Z16 `data` (`w` x `h`, packed) within Chebyshev distance under `band` of a silhouette pixel of
/// the noise-free hit depths `hit` (0: a miss).
fn invalid_band(data: &mut [u8], hit: &[f64], w: u32, h: u32, band: u32) {
    let (w, h) = (w as i64, h as i64);
    let at = |x: i64, y: i64| hit.get((y * w + x) as usize).copied().unwrap_or(0.0);
    let step = |a: f64, b: f64| b <= 0.0 || (a - b).abs() > SILHOUETTE_STEP * a.min(b);
    let r = band as i64 - 1;
    for y in 0..h {
        for x in 0..w {
            let a = at(x, y);
            if a <= 0.0 {
                continue;
            }
            let edge = [(1i64, 0i64), (-1, 0), (0, 1), (0, -1)].iter().any(|(dx, dy)| {
                let (nx, ny) = (x + dx, y + dy);
                nx >= 0 && ny >= 0 && nx < w && ny < h && step(a, at(nx, ny))
            });
            if !edge {
                continue;
            }
            for yy in (y - r).max(0)..=(y + r).min(h - 1) {
                for xx in (x - r).max(0)..=(x + r).min(w - 1) {
                    let o = ((yy * w + xx) * 2) as usize;
                    data[o..o + 2].copy_from_slice(&0u16.to_le_bytes());
                }
            }
        }
    }
}
