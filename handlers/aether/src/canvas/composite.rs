//! `globalCompositeOperation` (HTML §4.12.5.1.15 over Compositing and Blending Level 1): the Porter-Duff
//! operators by their Fa/Fb factors, `lighter` (plus), and the sixteen blend modes composed with
//! source-over (§9.1.? "general formula": co = cs·(1−αb) + cb·(1−αs) + αs·αb·B(Cb, Cs)). All arithmetic is in
//! premultiplied f32.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Op {
    SourceOver,
    SourceIn,
    SourceOut,
    SourceAtop,
    DestinationOver,
    DestinationIn,
    DestinationOut,
    DestinationAtop,
    Lighter,
    Copy,
    Xor,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
}

pub const NAMES: [(&str, Op); 26] = [
    ("source-over", Op::SourceOver),
    ("source-in", Op::SourceIn),
    ("source-out", Op::SourceOut),
    ("source-atop", Op::SourceAtop),
    ("destination-over", Op::DestinationOver),
    ("destination-in", Op::DestinationIn),
    ("destination-out", Op::DestinationOut),
    ("destination-atop", Op::DestinationAtop),
    ("lighter", Op::Lighter),
    ("copy", Op::Copy),
    ("xor", Op::Xor),
    ("multiply", Op::Multiply),
    ("screen", Op::Screen),
    ("overlay", Op::Overlay),
    ("darken", Op::Darken),
    ("lighten", Op::Lighten),
    ("color-dodge", Op::ColorDodge),
    ("color-burn", Op::ColorBurn),
    ("hard-light", Op::HardLight),
    ("soft-light", Op::SoftLight),
    ("difference", Op::Difference),
    ("exclusion", Op::Exclusion),
    ("hue", Op::Hue),
    ("saturation", Op::Saturation),
    ("color", Op::Color),
    ("luminosity", Op::Luminosity),
];

impl Op {
    pub fn parse(s: &str) -> Option<Op> {
        NAMES.iter().find(|(n, _)| *n == s).map(|(_, o)| *o)
    }
    pub fn name(self) -> &'static str {
        NAMES.iter().find(|(_, o)| *o == self).map(|(n, _)| *n).unwrap_or("source-over")
    }
    /// Operators that change the destination where the source is transparent: they composite over the
    /// whole clip region, not just the shape (the spec's "infinite transparent black bitmap" rule).
    pub fn unbounded(self) -> bool {
        matches!(self, Op::SourceIn | Op::SourceOut | Op::DestinationIn | Op::DestinationAtop | Op::Copy)
    }
}

fn blend_sep(op: Op, cb: f32, cs: f32) -> f32 {
    match op {
        Op::Multiply => cb * cs,
        Op::Screen => cb + cs - cb * cs,
        Op::Overlay => blend_sep(Op::HardLight, cs, cb),
        Op::Darken => cb.min(cs),
        Op::Lighten => cb.max(cs),
        Op::ColorDodge => {
            if cb == 0.0 {
                0.0
            } else if cs >= 1.0 {
                1.0
            } else {
                (cb / (1.0 - cs)).min(1.0)
            }
        }
        Op::ColorBurn => {
            if cb >= 1.0 {
                1.0
            } else if cs <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - cb) / cs).min(1.0)
            }
        }
        Op::HardLight => {
            if cs <= 0.5 {
                cb * 2.0 * cs
            } else {
                let s = 2.0 * cs - 1.0;
                cb + s - cb * s
            }
        }
        Op::SoftLight => {
            if cs <= 0.5 {
                cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb)
            } else {
                let d = if cb <= 0.25 { ((16.0 * cb - 12.0) * cb + 4.0) * cb } else { cb.sqrt() };
                cb + (2.0 * cs - 1.0) * (d - cb)
            }
        }
        Op::Difference => (cb - cs).abs(),
        Op::Exclusion => cb + cs - 2.0 * cb * cs,
        _ => cs,
    }
}

fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

fn clip_color(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut o = c;
    if n < 0.0 && l - n != 0.0 {
        for v in o.iter_mut() {
            *v = l + (*v - l) * l / (l - n);
        }
    }
    if x > 1.0 && x - l != 0.0 {
        for v in o.iter_mut() {
            *v = l + (*v - l) * (1.0 - l) / (x - l);
        }
    }
    o
}

fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_color([c[0] + d, c[1] + d, c[2] + d])
}

fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    let mx = c[0].max(c[1]).max(c[2]);
    let mn = c[0].min(c[1]).min(c[2]);
    if mx <= mn {
        return [0.0; 3];
    }
    let mut o = [0.0; 3];
    for k in 0..3 {
        o[k] = (c[k] - mn) * s / (mx - mn);
    }
    o
}

fn blend_nonsep(op: Op, cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
    match op {
        Op::Hue => set_lum(set_sat(cs, sat(cb)), lum(cb)),
        Op::Saturation => set_lum(set_sat(cb, sat(cs)), lum(cb)),
        Op::Color => set_lum(cs, lum(cb)),
        _ => set_lum(cb, lum(cs)), // Luminosity
    }
}

/// Composite premultiplied `s` onto premultiplied `d`.
#[inline]
pub fn composite(op: Op, s: [f32; 4], d: [f32; 4]) -> [f32; 4] {
    let (sa, da) = (s[3], d[3]);
    let pd = |fa: f32, fb: f32| [s[0] * fa + d[0] * fb, s[1] * fa + d[1] * fb, s[2] * fa + d[2] * fb, sa * fa + da * fb];
    match op {
        Op::SourceOver => pd(1.0, 1.0 - sa),
        Op::SourceIn => pd(da, 0.0),
        Op::SourceOut => pd(1.0 - da, 0.0),
        Op::SourceAtop => pd(da, 1.0 - sa),
        Op::DestinationOver => pd(1.0 - da, 1.0),
        Op::DestinationIn => pd(0.0, sa),
        Op::DestinationOut => pd(0.0, 1.0 - sa),
        Op::DestinationAtop => pd(1.0 - da, sa),
        Op::Lighter => {
            let o = pd(1.0, 1.0);
            [o[0].min(1.0), o[1].min(1.0), o[2].min(1.0), o[3].min(1.0)]
        }
        Op::Copy => s,
        Op::Xor => pd(1.0 - da, 1.0 - sa),
        _ => {
            if sa <= 0.0 {
                return d;
            }
            let un = |c: [f32; 4]| -> [f32; 3] {
                if c[3] <= 0.0 { [0.0; 3] } else { [c[0] / c[3], c[1] / c[3], c[2] / c[3]] }
            };
            let (cs, cb) = (un(s), un(d));
            let b = match op {
                Op::Hue | Op::Saturation | Op::Color | Op::Luminosity => blend_nonsep(op, cb, cs),
                _ => [blend_sep(op, cb[0], cs[0]), blend_sep(op, cb[1], cs[1]), blend_sep(op, cb[2], cs[2])],
            };
            let ao = sa + da - sa * da;
            let mut o = [0f32; 4];
            for k in 0..3 {
                o[k] = s[k] * (1.0 - da) + d[k] * (1.0 - sa) + sa * da * b[k].clamp(0.0, 1.0);
            }
            o[3] = ao;
            o
        }
    }
}
