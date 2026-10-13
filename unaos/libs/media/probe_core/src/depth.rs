// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The depth model: Z16 units → metres, the pinhole intrinsics, deprojection, a point cloud.
//!
//! FACTS (librealsense v2.55.1, Apache-2.0, R110's shape):
//! * The depth scale is stored by the D400 firmware as `depth_units` in MICROMETRES, the first `uint32_t`
//!   of `depth_table_control` (`src/ds/ds-private.h:488-495`), read with the HW-monitor command `GET_ADV`
//!   (`ds-private.h:102`) on table `etDepthTableControl = 9` (`ds-private.h:171`); librealsense's metres
//!   are `0.000001 * depth_units` (`src/ds/ds-options.cpp:378-382`).
//! * When the device does not answer that table (no advanced mode), the scale is the constant 0.001 m
//!   (`src/ds/d400/d400-device.cpp:910-917`) — [`DepthScale::D400_DEFAULT`].
//! * The rectified depth image is a pinhole camera with NO distortion: its intrinsics are `fx, fy, ppx, ppy`
//!   (`include/librealsense2/h/rs_types.h:58-68` names them), filled from the coefficients table's
//!   per-resolution `rect_params` in the order fx, fy, ppx, ppy with every distortion coefficient zeroed
//!   (`src/ds/d400/d400-private.cpp:162-175`).
//!
//! The deprojection below is the pinhole model, written from its definition (a pixel `(u, v)` at depth `z`
//! is the point `z * ((u - ppx) / fx, (v - ppy) / fy, 1)`), not from `rs2_deproject_pixel_to_point`
//! (`include/librealsense2/rsutil.h:23`, whose distortion branches a rectified depth image never takes).

use alloc::vec::Vec;

use crate::frame::{Frame, PixelFormat};

/// A point in the camera's frame, metres: +x right, +y down, +z forward (the image's own axes).
pub type Point3 = [f32; 3];

/// Metres per Z16 unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DepthScale {
    pub metres_per_unit: f32,
}

impl DepthScale {
    /// The D400's scale when the firmware's depth table is not read (`d400-device.cpp:912`).
    pub const D400_DEFAULT: DepthScale = DepthScale { metres_per_unit: 0.001 };

    /// From the firmware's `depth_units` (micrometres per unit, `ds-options.cpp:381`).
    pub fn from_micrometres(depth_units: u32) -> DepthScale {
        DepthScale { metres_per_unit: depth_units as f32 * 0.000_001 }
    }

    /// The firmware's micrometres for this scale, rounded to the nearest (the inverse of
    /// [`DepthScale::from_micrometres`]; `ds-options.cpp:369` writes `1000000 * value` truncated, and a
    /// rounding inverse is what makes `from_micrometres(n).micrometres() == n` hold for every `n`).
    pub fn micrometres(self) -> u32 {
        let v = self.metres_per_unit * 1_000_000.0;
        if v <= 0.0 { 0 } else { (v + 0.5) as u32 }
    }

    /// Metres for one raw sample; `None` for 0 (the D400's "no depth here").
    pub fn metres(self, raw: u16) -> Option<f32> {
        if raw == 0 { None } else { Some(raw as f32 * self.metres_per_unit) }
    }
}

/// The pinhole intrinsics of one stream at one resolution.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Intrinsics {
    pub width: u32,
    pub height: u32,
    /// Principal point, pixels from the left / top edge.
    pub ppx: f32,
    pub ppy: f32,
    /// Focal length in pixels (horizontal / vertical).
    pub fx: f32,
    pub fy: f32,
}

impl Intrinsics {
    /// The "ideal" intrinsics librealsense assumes when a table carries none (`d400-private.cpp:163-169`:
    /// fx = fy = 1.5 * width, principal point at the centre). The replay probe uses it; a live probe
    /// reads `rect_params`.
    pub fn ideal(width: u32, height: u32) -> Intrinsics {
        Intrinsics {
            width,
            height,
            ppx: width as f32 * 0.5,
            ppy: height as f32 * 0.5,
            fx: width as f32 * 1.5,
            fy: width as f32 * 1.5,
        }
    }

    /// From a coefficients-table `rect_params` row in its own order: fx, fy, ppx, ppy.
    pub fn from_rect_params(width: u32, height: u32, rect: [f32; 4]) -> Intrinsics {
        Intrinsics { width, height, fx: rect[0], fy: rect[1], ppx: rect[2], ppy: rect[3] }
    }

    /// The pixel `(u, v)` at `depth_m` metres, as a point. `None` when a focal length is zero (a table
    /// that was never filled — dividing by it would manufacture infinities).
    pub fn deproject(&self, u: f32, v: f32, depth_m: f32) -> Option<Point3> {
        if self.fx == 0.0 || self.fy == 0.0 {
            return None;
        }
        let x = (u - self.ppx) / self.fx;
        let y = (v - self.ppy) / self.fy;
        Some([depth_m * x, depth_m * y, depth_m])
    }

    /// The inverse: a point in front of the camera → its pixel. `None` behind or on the image plane.
    pub fn project(&self, p: Point3) -> Option<[f32; 2]> {
        if p[2] <= 0.0 {
            return None;
        }
        Some([p[0] / p[2] * self.fx + self.ppx, p[1] / p[2] * self.fy + self.ppy])
    }
}

/// The metres at `(x, y)` of a Z16 frame.
pub fn metres_at(frame: &Frame, scale: DepthScale, x: u32, y: u32) -> Option<f32> {
    if frame.format != PixelFormat::Z16 {
        return None;
    }
    scale.metres(frame.u16_at(x, y)?)
}

/// The point cloud of a Z16 frame, every `step`-th pixel in each direction (`step` 0 is taken as 1),
/// holes (raw 0) skipped. Pixel centres are used (`x + 0.5`), so a principal point at the image centre
/// puts the centre of a symmetric image on the optical axis.
pub fn point_cloud(frame: &Frame, scale: DepthScale, intr: &Intrinsics, step: u32) -> Vec<Point3> {
    let step = step.max(1);
    let mut out = Vec::new();
    if frame.format != PixelFormat::Z16 {
        return out;
    }
    let mut y = 0;
    while y < frame.height {
        let mut x = 0;
        while x < frame.width {
            if let Some(m) = metres_at(frame, scale, x, y) {
                if let Some(p) = intr.deproject(x as f32 + 0.5, y as f32 + 0.5, m) {
                    out.push(p);
                }
            }
            x += step;
        }
        y += step;
    }
    out
}

/// Valid-pixel census of a Z16 frame: (valid, holes, min raw, max raw). The kernel's and the face's
/// "is this a depth picture at all" check.
pub fn census(frame: &Frame) -> (u32, u32, u16, u16) {
    let (mut valid, mut holes, mut lo, mut hi) = (0u32, 0u32, u16::MAX, 0u16);
    if frame.format != PixelFormat::Z16 {
        return (0, 0, 0, 0);
    }
    for y in 0..frame.height {
        for x in 0..frame.width {
            match frame.u16_at(x, y) {
                Some(0) | None => holes += 1,
                Some(v) => {
                    valid += 1;
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
        }
    }
    if valid == 0 {
        lo = 0;
    }
    (valid, holes, lo, hi)
}
