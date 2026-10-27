// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Colour spaces (ISO 32000-1 §8.6) to sRGB: DeviceGray / DeviceRGB / DeviceCMYK, CalGray / CalRGB read as their device
//! spaces, ICCBased by /N (1, 3, 4: no profile is applied — ICC profiles are owed), Lab drawn as its L* grey (owed),
//! Indexed, Separation and DeviceN through their tint transform ([`Function`]), and Pattern. CMYK is the naive
//! conversion R = (1-C)(1-K), G = (1-M)(1-K), B = (1-Y)(1-K).

use alloc::rc::Rc;
use alloc::vec;
use alloc::vec::Vec;

use super::function::Function;
use crate::doc::Document;
use crate::object::{Dict, Object};

/// The deepest colour-space nesting followed (Indexed / Separation / Pattern bases, resource names).
const MAX_CS_DEPTH: usize = 8;

/// A parsed colour space.
#[derive(Clone, Debug)]
pub enum ColourSpace {
    Gray,
    Rgb,
    Cmyk,
    /// Lab: drawn as its L* grey (Lab conversion owed).
    Lab,
    Indexed { base: Rc<ColourSpace>, hival: usize, lookup: Vec<u8> },
    /// Separation (n = 1) or DeviceN: n tints through `tint` into `alt`.
    Sep { n: usize, alt: Rc<ColourSpace>, tint: Option<Function> },
    /// Pattern, with the underlying space of an uncoloured pattern.
    Pattern(Option<Rc<ColourSpace>>),
}

/// A colour: its space, its components, and the pattern resource name when the space is Pattern.
#[derive(Clone, Debug)]
pub struct Colour {
    pub space: Rc<ColourSpace>,
    pub comps: Vec<f64>,
    pub pattern: Option<Vec<u8>>,
}

impl Colour {
    /// Black in DeviceGray (the initial colour, §8.4.1).
    pub fn black() -> Colour {
        Colour { space: Rc::new(ColourSpace::Gray), comps: vec![0.0], pattern: None }
    }
    /// A new space with its initial colour (§8.6.8).
    pub fn of(space: ColourSpace) -> Colour {
        let comps = space.initial();
        Colour { space: Rc::new(space), comps, pattern: None }
    }
    /// Straight sRGB, each 0..=1.
    pub fn rgb(&self) -> [f32; 3] {
        self.space.to_rgb(&self.comps)
    }
}

fn unit(v: f64) -> f64 {
    if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) }
}

impl ColourSpace {
    /// The number of components a colour in this space has.
    pub fn n(&self) -> usize {
        match self {
            ColourSpace::Gray | ColourSpace::Indexed { .. } => 1,
            ColourSpace::Rgb | ColourSpace::Lab => 3,
            ColourSpace::Cmyk => 4,
            ColourSpace::Sep { n, .. } => *n,
            ColourSpace::Pattern(_) => 0,
        }
    }

    /// The initial colour (§8.6.8 / §8.6.6): black, index 0, tint 1.
    pub fn initial(&self) -> Vec<f64> {
        match self {
            ColourSpace::Gray | ColourSpace::Indexed { .. } => vec![0.0],
            ColourSpace::Rgb | ColourSpace::Lab => vec![0.0; 3],
            ColourSpace::Cmyk => vec![0.0, 0.0, 0.0, 1.0],
            ColourSpace::Sep { n, .. } => vec![1.0; *n],
            ColourSpace::Pattern(_) => Vec::new(),
        }
    }

    /// An image's default /Decode array (§8.9.5.2, Table 90).
    pub fn default_decode(&self, bpc: u32) -> Vec<f64> {
        match self {
            ColourSpace::Indexed { .. } => vec![0.0, ((1u64 << bpc.min(16)) - 1) as f64],
            ColourSpace::Lab => vec![0.0, 100.0, -100.0, 100.0, -100.0, 100.0],
            s => (0..s.n()).flat_map(|_| [0.0, 1.0]).collect(),
        }
    }

    /// Straight sRGB of one colour, each 0..=1.
    pub fn to_rgb(&self, c: &[f64]) -> [f32; 3] {
        let g = |i: usize| unit(c.get(i).copied().unwrap_or(0.0));
        match self {
            ColourSpace::Gray => {
                let v = g(0) as f32;
                [v, v, v]
            }
            ColourSpace::Rgb => [g(0) as f32, g(1) as f32, g(2) as f32],
            ColourSpace::Cmyk => {
                let k = 1.0 - g(3);
                [((1.0 - g(0)) * k) as f32, ((1.0 - g(1)) * k) as f32, ((1.0 - g(2)) * k) as f32]
            }
            ColourSpace::Lab => {
                let v = (c.first().copied().unwrap_or(0.0) / 100.0).clamp(0.0, 1.0) as f32;
                [v, v, v]
            }
            ColourSpace::Indexed { base, hival, lookup } => {
                let i = (c.first().copied().unwrap_or(0.0).max(0.0) as usize).min(*hival);
                let n = base.n();
                let comps: Vec<f64> = (0..n).map(|k| lookup.get(i * n + k).copied().unwrap_or(0) as f64 / 255.0).collect();
                // Lab lookup bytes map onto its range; every other base is 0..1.
                if matches!(**base, ColourSpace::Lab) {
                    let l = comps.first().copied().unwrap_or(0.0) * 100.0;
                    return base.to_rgb(&[l]);
                }
                base.to_rgb(&comps)
            }
            ColourSpace::Sep { n, alt, tint } => match tint {
                Some(f) => {
                    let ins: Vec<f64> = (0..*n).map(g).collect();
                    alt.to_rgb(&f.eval(&ins))
                }
                // No usable tint transform: a tint of 1 is full ink, drawn as black.
                None => {
                    let v = (1.0 - g(0)) as f32;
                    [v, v, v]
                }
            },
            ColourSpace::Pattern(_) => [0.0, 0.0, 0.0],
        }
    }

    /// Parse a colour space operand or entry; a name not naming a device space is looked up in `res` /ColorSpace.
    pub fn parse(o: &Object, res: &Dict, doc: &Document) -> Option<ColourSpace> {
        parse_depth(o, res, doc, 0)
    }
}

fn parse_depth(o: &Object, res: &Dict, doc: &Document, depth: usize) -> Option<ColourSpace> {
    if depth > MAX_CS_DEPTH {
        return None;
    }
    let o = doc.get(o);
    match &o {
        Object::Name(n) => match n.as_slice() {
            b"DeviceGray" | b"G" | b"CalGray" => Some(ColourSpace::Gray),
            b"DeviceRGB" | b"RGB" | b"CalRGB" => Some(ColourSpace::Rgb),
            b"DeviceCMYK" | b"CMYK" => Some(ColourSpace::Cmyk),
            b"Pattern" => Some(ColourSpace::Pattern(None)),
            name => {
                let table = doc.get(res.get(b"ColorSpace")?);
                let entry = table.as_dict()?.get(name)?.clone();
                parse_depth(&entry, res, doc, depth + 1)
            }
        },
        Object::Array(a) => {
            let fam = doc.get(a.first()?);
            let arg = |i: usize| a.get(i).map(|x| doc.get(x));
            match fam.as_name()? {
                b"DeviceGray" | b"G" | b"CalGray" => Some(ColourSpace::Gray),
                b"DeviceRGB" | b"RGB" | b"CalRGB" => Some(ColourSpace::Rgb),
                b"DeviceCMYK" | b"CMYK" => Some(ColourSpace::Cmyk),
                b"Lab" => Some(ColourSpace::Lab),
                b"ICCBased" => {
                    let s = arg(1)?;
                    let d = s.as_stream().map(|s| &s.dict).or(s.as_dict())?;
                    match doc.get(d.get(b"N")?).as_int()? {
                        1 => Some(ColourSpace::Gray),
                        3 => Some(ColourSpace::Rgb),
                        4 => Some(ColourSpace::Cmyk),
                        _ => None,
                    }
                }
                b"Indexed" | b"I" => {
                    let base = parse_depth(&arg(1)?, res, doc, depth + 1)?;
                    let hival = arg(2)?.as_int()?.clamp(0, 255) as usize;
                    let lookup = match arg(3)? {
                        Object::String(s) => s,
                        Object::Stream(s) => doc.decode(&s).ok()?.data,
                        _ => return None,
                    };
                    Some(ColourSpace::Indexed { base: Rc::new(base), hival, lookup })
                }
                b"Separation" | b"DeviceN" => {
                    let n = if fam.as_name()? == b"Separation" { 1 } else { arg(1)?.as_array()?.len().max(1) };
                    let alt = parse_depth(&arg(2)?, res, doc, depth + 1)?;
                    let tint = arg(3).and_then(|f| Function::parse(&f, doc));
                    Some(ColourSpace::Sep { n, alt: Rc::new(alt), tint })
                }
                b"Pattern" => {
                    let base = arg(1).and_then(|b| parse_depth(&b, res, doc, depth + 1)).map(Rc::new);
                    Some(ColourSpace::Pattern(base))
                }
                _ => None,
            }
        }
        _ => None,
    }
}
