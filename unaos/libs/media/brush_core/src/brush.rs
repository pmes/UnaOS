// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The brush: its parameters and the pressure curve that scales its radius and flow.

use raster_core::BlendMode;

use crate::mul255;

/// How stylus pressure (0..=255) maps to the brush's response (0..=255).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PressureCurve {
    /// `p`.
    Linear,
    /// `round(p * p / 255)`: soft touches respond less.
    Quadratic,
    /// `table[p]`.
    Lut([u8; 256]),
}

impl PressureCurve {
    /// The curve's response to pressure `p`.
    pub fn map(&self, p: u8) -> u8 {
        match self {
            PressureCurve::Linear => p,
            PressureCurve::Quadratic => mul255(p as u32, p as u32) as u8,
            PressureCurve::Lut(t) => t[p as usize],
        }
    }
}

/// A round brush.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Brush {
    /// Diameter in px at full pressure.
    pub diameter: f32,
    /// 0 = fully soft falloff from the centre, 255 = hard edge.
    pub hardness: u8,
    /// Distance between dab centres as a fraction of `diameter` (default 0.25).
    pub spacing: f32,
    /// Scales the paint colour's alpha before every dab.
    pub opacity: u8,
    /// Per-dab opacity at full pressure.
    pub flow: u8,
    pub pressure: PressureCurve,
    pub mode: BlendMode,
}

impl Brush {
    /// A hard, opaque, Normal brush with spacing 0.25 and a Linear pressure curve.
    pub fn new(diameter: f32) -> Brush {
        Brush {
            diameter,
            hardness: 255,
            spacing: 0.25,
            opacity: 255,
            flow: 255,
            pressure: PressureCurve::Linear,
            mode: BlendMode::Normal,
        }
    }

    /// Radius in px at pressure `p`: `(diameter / 2) * map(p) / 255`.
    pub fn radius_at(&self, p: u8) -> f32 {
        self.diameter * 0.5 * self.pressure.map(p) as f32 / 255.0
    }

    /// Flow at pressure `p`: `round(flow * map(p) / 255)`.
    pub fn flow_at(&self, p: u8) -> u8 {
        mul255(self.flow as u32, self.pressure.map(p) as u32) as u8
    }
}
