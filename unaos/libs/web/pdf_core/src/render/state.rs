// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The graphics state (ISO 32000-1 §8.4, Table 52) and its text state (§9.3): what `q` saves and `Q` restores, the
//! ExtGState parameters (`gs`, §8.4.5) this arc honours, and the stroke parameters as svg_core's `StrokeStyle`.

use alloc::rc::Rc;
use alloc::vec::Vec;

use svg_core::geom::Transform;
use svg_core::raster::Mask;
use svg_core::stroke::{Cap, Join, StrokeStyle, normalize_dashes};

use super::colour::Colour;
use crate::doc::Document;
use crate::object::{Dict, Object};

/// The text state parameters (§9.3.1, Table 104).
#[derive(Clone, Debug)]
pub struct TextState {
    pub char_spacing: f64,
    pub word_spacing: f64,
    /// Tz / 100.
    pub h_scale: f64,
    pub leading: f64,
    /// The /Font resource name `Tf` selected, and that font's dictionary (empty when absent).
    pub font_name: Vec<u8>,
    pub font: Dict,
    pub size: f64,
    pub render_mode: u8,
    pub rise: f64,
}

impl Default for TextState {
    fn default() -> Self {
        TextState {
            char_spacing: 0.0,
            word_spacing: 0.0,
            h_scale: 1.0,
            leading: 0.0,
            font_name: Vec::new(),
            font: Dict::new(),
            size: 0.0,
            render_mode: 0,
            rise: 0.0,
        }
    }
}

/// The graphics state.
#[derive(Clone, Debug)]
pub struct GState {
    pub ctm: Transform,
    /// The pattern space's parent: the CTM at the start of the current content stream (page or form).
    pub base: Transform,
    /// The clipping region as a device-space coverage mask; `None` = the whole page.
    pub clip: Option<Rc<Mask>>,
    pub line_width: f64,
    pub cap: Cap,
    pub join: Join,
    pub miter_limit: f64,
    pub dash: Vec<f64>,
    pub dash_phase: f64,
    pub fill: Colour,
    pub stroke: Colour,
    /// ca and CA (§11.6.4.4 constant alpha).
    pub fill_alpha: f32,
    pub stroke_alpha: f32,
    pub text: TextState,
}

impl GState {
    pub fn new(ctm: Transform) -> GState {
        GState {
            ctm,
            base: ctm,
            clip: None,
            line_width: 1.0,
            cap: Cap::Butt,
            join: Join::Miter,
            miter_limit: 10.0,
            dash: Vec::new(),
            dash_phase: 0.0,
            fill: Colour::black(),
            stroke: Colour::black(),
            fill_alpha: 1.0,
            stroke_alpha: 1.0,
            text: TextState::default(),
        }
    }

    pub fn set_cap(&mut self, v: i64) {
        self.cap = match v {
            1 => Cap::Round,
            2 => Cap::Square,
            _ => Cap::Butt,
        };
    }

    pub fn set_join(&mut self, v: i64) {
        self.join = match v {
            1 => Join::Round,
            2 => Join::Bevel,
            _ => Join::Miter,
        };
    }

    /// The stroke as svg_core draws it. Width 0 is the thinnest line the device can draw (§8.4.3.2): one pixel.
    pub fn stroke_style(&self, ctm: &Transform) -> StrokeStyle {
        let width = if self.line_width > 0.0 { self.line_width } else { 1.0 / ctm.mean_scale().max(1e-9) };
        let dash = normalize_dashes(self.dash.clone()).map(|d| (d, self.dash_phase));
        StrokeStyle { width, cap: self.cap, join: self.join, miter_limit: self.miter_limit.max(1.0), dash }
    }

    /// Apply an ExtGState dictionary. Soft masks and blend modes other than Normal are refused by name and drawn as
    /// Normal without a mask (PDF-TRANSPARENCY is owed).
    pub fn apply_ext(&mut self, d: &Dict, doc: &Document, refused: &mut Vec<&'static str>) {
        let num = |k: &[u8]| d.get(k).map(|o| doc.get(o)).and_then(|o| o.as_f64());
        if let Some(v) = num(b"LW") {
            self.line_width = v.abs();
        }
        if let Some(v) = num(b"LC") {
            self.set_cap(v as i64);
        }
        if let Some(v) = num(b"LJ") {
            self.set_join(v as i64);
        }
        if let Some(v) = num(b"ML") {
            self.miter_limit = v;
        }
        if let Some(v) = num(b"CA") {
            self.stroke_alpha = v.clamp(0.0, 1.0) as f32;
        }
        if let Some(v) = num(b"ca") {
            self.fill_alpha = v.clamp(0.0, 1.0) as f32;
        }
        if let Some(Object::Array(a)) = d.get(b"D").map(|o| doc.get(o)) {
            if let (Some(arr), Some(ph)) = (a.first().map(|o| doc.get(o)), a.get(1).and_then(|o| doc.get(o).as_f64())) {
                if let Some(arr) = arr.as_array() {
                    self.dash = arr.iter().filter_map(|o| doc.get(o).as_f64()).collect();
                    self.dash_phase = ph;
                }
            }
        }
        if let Some(Object::Array(a)) = d.get(b"Font").map(|o| doc.get(o)) {
            if let (Some(Object::Dict(f)), Some(sz)) = (a.first().map(|o| doc.get(o)), a.get(1).and_then(|o| doc.get(o).as_f64())) {
                self.text.font = f;
                self.text.font_name = Vec::new();
                self.text.size = sz;
            }
        }
        match d.get(b"SMask").map(|o| doc.get(o)) {
            None | Some(Object::Null) => {}
            Some(Object::Name(n)) if n == b"None" => {}
            Some(_) => refuse(refused, "soft mask"),
        }
        let normal = |o: &Object| matches!(o.as_name(), Some(b"Normal") | Some(b"Compatible"));
        match d.get(b"BM").map(|o| doc.get(o)) {
            None => {}
            Some(Object::Array(a)) if a.first().is_some_and(|o| normal(&doc.get(o))) => {}
            Some(o) if normal(&o) => {}
            Some(_) => refuse(refused, "blend mode"),
        }
    }
}

/// Note a refused feature once, by name.
pub fn refuse(refused: &mut Vec<&'static str>, what: &'static str) {
    if !refused.contains(&what) {
        refused.push(what);
    }
}

/// The text matrices (§9.4.2): not part of the graphics state (`q` does not save them).
#[derive(Clone, Copy, Debug)]
pub struct TextMatrices {
    pub tm: Transform,
    pub tlm: Transform,
}

impl Default for TextMatrices {
    fn default() -> Self {
        TextMatrices { tm: Transform::IDENTITY, tlm: Transform::IDENTITY }
    }
}

impl TextMatrices {
    /// `Td`: a new line offset from the start of the current one.
    pub fn td(&mut self, tx: f64, ty: f64) {
        self.tlm = self.tlm.then_pre(&Transform::translate(tx, ty));
        self.tm = self.tlm;
    }
    /// `Tm`.
    pub fn set(&mut self, m: Transform) {
        self.tm = m;
        self.tlm = m;
    }
    /// Move the pen `tx` along the baseline (text space).
    pub fn advance(&mut self, tx: f64) {
        self.tm = self.tm.then_pre(&Transform::translate(tx, 0.0));
    }
}
