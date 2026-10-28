// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Aether — shared-core. PDFTRANSPARENCY (SR327): transparency groups (ISO 32000-1 §11.4, §11.6.6) and soft
//! masks (§11.6.5.2) for the content interpreter. Both render a form into an OFFSCREEN canvas (the interpreter's one
//! canvas swapped out and back) through the interpreter's own `nested`, under the group's initial state (blend Normal,
//! alpha 1, no soft mask). A group composites back ONCE: isolated through svg_core's `Pixmap::draw_pixmap` (Normal) or
//! [`super::blend::composite`] (any other mode); non-isolated as lerp(backdrop, group, ca x mask), exact for an
//! opaque backdrop and group BM Normal. Knockout groups are drawn non-knockout and refused by name. A soft mask's form is
//! drawn over its /BC backdrop (Luminosity: 0.30 R + 0.59 G + 0.11 B, §11.5.3) or transparent (Alpha), mapped through
//! /TR, and becomes the graphics state's device-space soft mask that `Interp::paint` multiplies into every coverage.

use alloc::rc::Rc;
use alloc::vec::Vec;

use svg_core::fe::{BlendMode, IRect};
use svg_core::geom::Transform;
use svg_core::raster::{FLATTEN_TOL, FillRule, Mask, Pixmap, div255, fill_polys};

use super::colour::ColourSpace;
use super::function::Function;
use super::state::refuse;
use super::{Interp, blend, rect_path};
use crate::object::{Dict, Object, Stream};

fn to8(v: f32) -> u8 {
    (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

impl Interp<'_, '_> {
    /// Run `f` with the canvas replaced by `init`; return what it drew and put the canvas back.
    fn offscreen(&mut self, init: Pixmap, f: impl FnOnce(&mut Self)) -> Pixmap {
        let saved = core::mem::replace(&mut self.canvas, init);
        f(self);
        core::mem::replace(&mut self.canvas, saved)
    }

    /// Run a form under a group's initial state (§11.6.6: blend Normal, CA = ca = 1, no soft mask), the clip kept or not.
    fn initial_run(&mut self, m: Transform, bbox: Option<[f64; 4]>, data: &[u8], res: Dict, keep_clip: bool) {
        let saved = (self.gs.blend, self.gs.fill_alpha, self.gs.stroke_alpha, self.gs.soft_mask.take(), self.gs.clip.clone());
        self.gs.blend = BlendMode::Normal;
        self.gs.fill_alpha = 1.0;
        self.gs.stroke_alpha = 1.0;
        if !keep_clip {
            self.gs.clip = None;
        }
        self.nested(m, bbox, data, res);
        self.gs.blend = saved.0;
        self.gs.fill_alpha = saved.1;
        self.gs.stroke_alpha = saved.2;
        self.gs.soft_mask = saved.3;
        self.gs.clip = saved.4;
    }

    /// The device pixel bounds of a form's /BBox under `m` x CTM (the whole canvas without one).
    fn device_rect(&self, m: Transform, bbox: Option<[f64; 4]>) -> IRect {
        let (w, h) = (self.canvas.w, self.canvas.h);
        let all = IRect { x0: 0, y0: 0, x1: w, y1: h };
        let Some(b) = bbox else { return all };
        let ctm = self.gs.ctm.then_pre(&m);
        let polys = rect_path(b[0], b[1], b[2] - b[0], b[3] - b[1]).transform(&ctm).flatten(FLATTEN_TOL);
        match fill_polys(&polys, FillRule::NonZero, w, h, true) {
            Some(c) => IRect { x0: c.x, y0: c.y, x1: c.x + c.w, y1: c.y + c.h },
            None => IRect { x0: 0, y0: 0, x1: 0, y1: 0 },
        }
    }

    /// A form XObject whose /Group has /S /Transparency: drawn into its own canvas, composited back once with the
    /// current ca, soft mask and blend mode.
    pub(super) fn group(&mut self, s: &Stream, g: &Dict) {
        let Some((m, bbox, data, res)) = self.form_parts(s) else { return };
        let flag = |k: &[u8]| matches!(g.get(k).map(|o| self.doc.get(o)), Some(Object::Bool(true)));
        let (isolated, knockout) = (flag(b"I"), flag(b"K"));
        if knockout {
            refuse(&mut self.refused, "knockout group");
        }
        let (alpha, mask, mode) = (self.gs.fill_alpha, self.gs.soft_mask.clone(), self.gs.blend);
        // A non-isolated group under a non-Normal blend mode composites as isolated (owed: the backdrop-removal form).
        let isolated = isolated || mode != BlendMode::Normal;
        let init = if isolated { Pixmap::new(self.canvas.w, self.canvas.h) } else { self.canvas.clone() };
        let mut layer = self.offscreen(init, |it| it.initial_run(m, bbox, &data, res, true));
        let op = (alpha.clamp(0.0, 1.0) * 255.0 + 0.5) as u32;
        let k_at = |p: usize| match &mask {
            Some(mk) => div255(op * mk.data[p] as u32),
            None => op,
        };
        if !isolated {
            for p in 0..self.canvas.w * self.canvas.h {
                let k = k_at(p);
                if k == 0 {
                    continue;
                }
                let (d, l) = (&mut self.canvas.data[p * 4..p * 4 + 4], &layer.data[p * 4..p * 4 + 4]);
                for ch in 0..4 {
                    d[ch] = div255(d[ch] as u32 * (255 - k) + l[ch] as u32 * k) as u8;
                }
            }
        } else if mode == BlendMode::Normal {
            self.canvas.draw_pixmap(&layer, alpha, mask.as_deref());
        } else {
            for p in 0..layer.w * layer.h {
                let k = k_at(p);
                if k == 255 {
                    continue;
                }
                for ch in layer.data[p * 4..p * 4 + 4].iter_mut() {
                    *ch = div255(*ch as u32 * k) as u8;
                }
            }
            let r = self.device_rect(m, bbox);
            blend::composite(&mut self.canvas, &layer, r, mode);
        }
    }

    /// An ExtGState /SMask dictionary (§11.6.5.2): render /G under the current CTM and make it the soft mask.
    pub(super) fn soft_mask(&mut self, d: &Dict) {
        self.gs.soft_mask = None;
        let doc = self.doc;
        let Some(Object::Stream(g)) = d.get(b"G").map(|o| doc.get(o)) else {
            refuse(&mut self.refused, "soft mask");
            return;
        };
        let Some((m, bbox, data, res)) = self.form_parts(&g) else {
            refuse(&mut self.refused, "soft mask");
            return;
        };
        let s = d.get(b"S").map(|o| doc.get(o));
        let alpha = matches!(s.as_ref().and_then(Object::as_name), Some(b"Alpha"));
        let (w, h) = (self.canvas.w, self.canvas.h);
        let mut init = Pixmap::new(w, h);
        if !alpha {
            let bc: Vec<f64> = match d.get(b"BC").map(|o| doc.get(o)) {
                Some(Object::Array(a)) => a.iter().filter_map(|x| doc.get(x).as_f64()).collect(),
                _ => Vec::new(),
            };
            let rgb = if bc.is_empty() {
                [0.0; 3]
            } else {
                let group_cs = match g.dict.get(b"Group").map(|o| doc.get(o)) {
                    Some(Object::Dict(gd)) => gd.get(b"CS").and_then(|o| ColourSpace::parse(o, &res, doc)),
                    _ => None,
                };
                let cs = group_cs.unwrap_or(match bc.len() {
                    3 => ColourSpace::Rgb,
                    4 => ColourSpace::Cmyk,
                    _ => ColourSpace::Gray,
                });
                cs.to_rgb(&bc)
            };
            let px = [to8(rgb[0]), to8(rgb[1]), to8(rgb[2]), 255];
            for c in init.data.chunks_exact_mut(4) {
                c.copy_from_slice(&px);
            }
        }
        let pix = self.offscreen(init, |it| it.initial_run(m, bbox, &data, res, false));
        // /TR (a function; /Identity or absent = none) as a 256-entry table on the 8-bit mask value.
        let tr = match d.get(b"TR").map(|o| doc.get(o)) {
            Some(o) if o.as_name().is_none() => Function::parse(&o, doc),
            _ => None,
        };
        let mut lut = [0u8; 256];
        for (i, v) in lut.iter_mut().enumerate() {
            *v = match &tr {
                Some(f) => to8(f.eval(&[i as f64 / 255.0]).first().copied().unwrap_or(i as f64 / 255.0) as f32),
                None => i as u8,
            };
        }
        let mut mask = Mask::new(w, h, 0);
        for (p, mv) in mask.data.iter_mut().enumerate() {
            let c = &pix.data[p * 4..p * 4 + 4];
            let v = if alpha { c[3] } else { to8((0.30 * c[0] as f32 + 0.59 * c[1] as f32 + 0.11 * c[2] as f32) / 255.0) };
            *mv = lut[v as usize];
        }
        self.gs.soft_mask = Some(Rc::new(mask));
    }
}
