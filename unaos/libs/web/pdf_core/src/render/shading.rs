// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Shadings (ISO 32000-1 §8.7.4.5) and shading patterns (§8.7.3) as svg_core shaders: type 2 (axial) is
//! `Shader::Linear`, type 3 (radial) is svg_core's two-point conical `Shader::Radial` (the start circle as its focal
//! circle), each with stops sampled from the shading's function through its colour space. /Extend: an extended side is
//! svg_core's Pad; a side that does not extend widens the gradient's parameter to [-1, 2] with transparent stops
//! outside [0, 1], so Pad paints nothing past it. Types 1 and 4-7 are drawn as one average colour and refused by name
//! (owed); tiling patterns are drawn 50% grey and refused (PDF-TILING owed).

use alloc::vec::Vec;

use svg_core::geom::Transform;
use svg_core::paint::{Shader, Spread, Stop, premul};

use super::colour::ColourSpace;
use super::function::Function;
use super::state::refuse;
use crate::doc::Document;
use crate::object::{Dict, Object};

/// Samples taken of a shading's function for the gradient's stops.
pub const STOPS: usize = 64;

/// The grey a refused paint is drawn with (premultiplied, opaque).
pub const REFUSED_GREY: [f32; 4] = [0.5, 0.5, 0.5, 1.0];

/// A shading ready to paint: the shader (in device space) and the shading's /BBox in shading space.
pub struct Painted {
    pub shader: Shader,
    pub bbox: Option<[f64; 4]>,
}

fn nums(doc: &Document, d: &Dict, k: &[u8]) -> Option<Vec<f64>> {
    let a = doc.get(d.get(k)?);
    a.as_array()?.iter().map(|x| doc.get(x).as_f64()).collect()
}

/// A shading dictionary (or stream) under `space` (shading space -> device).
pub fn shading(o: &Object, space: Transform, res: &Dict, doc: &Document, refused: &mut Vec<&'static str>) -> Option<Painted> {
    let o = doc.get(o);
    let d = match &o {
        Object::Dict(d) => d.clone(),
        Object::Stream(s) => s.dict.clone(),
        _ => return None,
    };
    let ty = doc.get(d.get(b"ShadingType")?).as_int()?;
    let cs = ColourSpace::parse(d.get(b"ColorSpace")?, res, doc)?;
    let func = d.get(b"Function").and_then(|f| Function::parse(f, doc));
    let domain = nums(doc, &d, b"Domain").filter(|v| v.len() >= 2).unwrap_or_else(|| alloc::vec![0.0, 1.0]);
    let bbox = nums(doc, &d, b"BBox").filter(|b| b.len() == 4).map(|b| [b[0].min(b[2]), b[1].min(b[3]), b[0].max(b[2]), b[1].max(b[3])]);
    let colour = |t: f64| -> [f32; 3] {
        match &func {
            Some(f) => cs.to_rgb(&f.eval(&[t])),
            None => cs.to_rgb(&cs.initial()),
        }
    };
    let shader = match ty {
        2 | 3 => {
            let coords = nums(doc, &d, b"Coords")?;
            let ext: Vec<bool> = match d.get(b"Extend").map(|e| doc.get(e)) {
                Some(Object::Array(a)) => a.iter().map(|b| matches!(doc.get(b), Object::Bool(true))).collect(),
                _ => Vec::new(),
            };
            let (e0, e1) = (ext.first().copied().unwrap_or(false), ext.get(1).copied().unwrap_or(false));
            let (t0, t1) = (domain[0], domain[1]);
            let inv = space.invert()?;
            let widen = !(e0 && e1);
            let mut stops = Vec::with_capacity(STOPS + 4);
            let first = colour(t0);
            let last = colour(t1);
            if widen && !e0 {
                stops.push(Stop { offset: 0.0, color: [first[0], first[1], first[2], 0.0] });
                stops.push(Stop { offset: 1.0 / 3.0 - 1e-9, color: [first[0], first[1], first[2], 0.0] });
            }
            for i in 0..STOPS {
                let f = i as f64 / (STOPS - 1) as f64;
                let c = colour(t0 + f * (t1 - t0));
                let offset = if widen { (1.0 + f) / 3.0 } else { f };
                stops.push(Stop { offset, color: [c[0], c[1], c[2], 1.0] });
            }
            if widen && !e1 {
                stops.push(Stop { offset: 2.0 / 3.0 + 1e-9, color: [last[0], last[1], last[2], 0.0] });
                stops.push(Stop { offset: 1.0, color: [last[0], last[1], last[2], 0.0] });
            }
            let k = if widen { 1.0 } else { 0.0 };
            if ty == 2 {
                if coords.len() != 4 {
                    return None;
                }
                let (dx, dy) = (coords[2] - coords[0], coords[3] - coords[1]);
                let p1 = (coords[0] - k * dx, coords[1] - k * dy);
                let p2 = (coords[2] + k * dx, coords[3] + k * dy);
                Shader::Linear { inv, p1, p2, stops, spread: Spread::Pad }
            } else {
                if coords.len() != 6 {
                    return None;
                }
                let (dx, dy, dr) = (coords[3] - coords[0], coords[4] - coords[1], coords[5] - coords[2]);
                let f = (coords[0] - k * dx, coords[1] - k * dy);
                let fr = coords[2] - k * dr;
                let c = (coords[3] + k * dx, coords[4] + k * dy);
                let r = coords[5] + k * dr;
                Shader::Radial { inv, c, r, f, fr, stops, spread: Spread::Pad }
            }
        }
        1 | 4 | 5 | 6 | 7 => {
            refuse(refused, match ty {
                1 => "shading type 1",
                4 => "shading type 4",
                5 => "shading type 5",
                6 => "shading type 6",
                _ => "shading type 7",
            });
            // The average colour: the function at the centre of its domain, else the colour at the /Decode midpoints.
            let rgb = if ty == 1 {
                match &func {
                    Some(f) => {
                        let ins: Vec<f64> = domain.chunks(2).map(|p| (p[0] + p.get(1).copied().unwrap_or(p[0])) / 2.0).collect();
                        cs.to_rgb(&f.eval(&ins))
                    }
                    None => cs.to_rgb(&cs.initial()),
                }
            } else {
                let dec = nums(doc, &d, b"Decode").unwrap_or_default();
                match &func {
                    Some(f) => {
                        let t = dec.get(4..6).map(|p| (p[0] + p[1]) / 2.0).unwrap_or(0.5);
                        cs.to_rgb(&f.eval(&[t]))
                    }
                    None => {
                        let comps: Vec<f64> = dec.chunks(2).skip(2).map(|p| (p[0] + p.get(1).copied().unwrap_or(p[0])) / 2.0).collect();
                        cs.to_rgb(&comps)
                    }
                }
            };
            Shader::Solid(premul([rgb[0], rgb[1], rgb[2], 1.0]))
        }
        _ => return None,
    };
    Some(Painted { shader, bbox })
}

/// A pattern used as a fill or stroke colour (§8.7.3): its shader in device space. `base` is the CTM at the start of
/// the content stream the pattern is used in (the pattern matrix maps pattern space onto it).
pub fn pattern(o: &Object, base: Transform, res: &Dict, doc: &Document, refused: &mut Vec<&'static str>) -> Option<Shader> {
    let o = doc.get(o);
    let d = match &o {
        Object::Dict(d) => d.clone(),
        Object::Stream(s) => s.dict.clone(),
        _ => return None,
    };
    let m = nums(doc, &d, b"Matrix")
        .filter(|m| m.len() == 6)
        .map(|m| Transform::new(m[0], m[1], m[2], m[3], m[4], m[5]))
        .unwrap_or(Transform::IDENTITY);
    match doc.get(d.get(b"PatternType")?).as_int()? {
        2 => shading(d.get(b"Shading")?, base.then_pre(&m), res, doc, refused).map(|p| p.shader),
        1 => {
            refuse(refused, "tiling pattern");
            Some(Shader::Solid(REFUSED_GREY))
        }
        _ => None,
    }
}
