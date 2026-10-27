// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! A stroke (a pressure-carrying polyline) and the dabs a brush lays along it.

use alloc::vec::Vec;

use crate::brush::Brush;
use crate::stamp::isqrt;
use crate::round;

/// The smallest step between dab centres in px: a zero or negative `spacing * diameter` cannot loop forever.
pub const MIN_STEP: f32 = 0.25;
/// The most dabs one stroke yields (a degenerate stroke across ~2^24 px stops here instead of allocating without bound).
pub const MAX_DABS: usize = 1 << 20;
/// Slack in px so a dab due exactly on a vertex is not lost to rounding.
const EPS: f32 = 1e-3;

/// One sampled point of a stroke.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StrokePoint {
    pub x: f32,
    pub y: f32,
    /// 0..=255.
    pub pressure: u8,
}

/// A polyline of stroke points, in input order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Stroke {
    pub points: Vec<StrokePoint>,
}

/// One brush imprint: centre, radius (px), flow (per-dab opacity) and the brush's hardness (so [`crate::stamp`]
/// needs only the dab).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Dab {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub flow: u8,
    pub hardness: u8,
}

/// Segment length in px through the integer square root on a 1/256-px fixed-point `d²` (no float root in no_std).
fn seg_len(a: &StrokePoint, b: &StrokePoint) -> f32 {
    let dx = round((b.x - a.x) * 256.0);
    let dy = round((b.y - a.y) * 256.0);
    let (ax, ay) = (dx.unsigned_abs(), dy.unsigned_abs());
    let d2 = ax.saturating_mul(ax).saturating_add(ay.saturating_mul(ay));
    isqrt(d2) as f32 / 256.0
}

/// The dabs `brush` lays along `stroke`: the first at the first point, then one every `spacing * diameter` px of
/// arc length (at least [`MIN_STEP`]), carried across vertices; pressure interpolated linearly along each segment.
/// An empty stroke gives none; a one-point stroke gives one; at most [`MAX_DABS`].
pub fn dabs(brush: &Brush, stroke: &Stroke) -> Vec<Dab> {
    let mut out = Vec::new();
    let pts = &stroke.points;
    let Some(first) = pts.first() else { return out };
    let dab_at = |x: f32, y: f32, p: u8| Dab { x, y, radius: brush.radius_at(p), flow: brush.flow_at(p), hardness: brush.hardness };
    out.push(dab_at(first.x, first.y, first.pressure));

    let mut step = brush.spacing * brush.diameter;
    if !(step >= MIN_STEP) {
        step = MIN_STEP; // also catches NaN
    }
    let mut to_next = step;
    for seg in pts.windows(2) {
        let (a, b) = (&seg[0], &seg[1]);
        let l = seg_len(a, b);
        if !(l >= EPS) {
            continue;
        }
        // t = to_next + k * step with an integer k: an f32 accumulator stops moving on a long segment.
        let mut k: u32 = 0;
        loop {
            let t = to_next + k as f32 * step;
            if !(t <= l + EPS) || out.len() >= MAX_DABS {
                to_next = t - l;
                break;
            }
            let f = (t / l).clamp(0.0, 1.0);
            let p = round(a.pressure as f32 + (b.pressure as f32 - a.pressure as f32) * f).clamp(0, 255) as u8;
            out.push(dab_at(a.x + (b.x - a.x) * f, a.y + (b.y - a.y) * f, p));
            k += 1;
        }
    }
    out
}
