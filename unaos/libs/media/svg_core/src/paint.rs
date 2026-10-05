//! Paint servers as shaders (SVG 2 §14): solid colours, linear gradients, radial gradients as the two-point
//! conical gradient SVG 2 defines with `fr` (§14.3.3 — the focal circle (fx, fy, fr) to the end circle
//! (cx, cy, r), the largest `t` with a non-negative radius wins), spread methods pad / reflect / repeat
//! (§14.3.4), colour interpolation between stops in straight (non-premultiplied) sRGB, and image shaders
//! (patterns, `<image>`) with bilinear or nearest sampling and optional tiling.

use crate::fmath::{floor, sqrt};
use crate::geom::Transform;
use crate::raster::Pixmap;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Spread {
    Pad,
    Reflect,
    Repeat,
}

/// A gradient stop: offset in [0,1] and a straight RGBA colour (0..=1 floats).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stop {
    pub offset: f64,
    pub color: [f32; 4],
}

#[derive(Clone, Debug)]
pub enum Shader {
    /// Premultiplied colour.
    Solid([f32; 4]),
    Linear { inv: Transform, p1: (f64, f64), p2: (f64, f64), stops: Vec<Stop>, spread: Spread },
    Radial { inv: Transform, c: (f64, f64), r: f64, f: (f64, f64), fr: f64, stops: Vec<Stop>, spread: Spread },
    Image { inv: Transform, pix: Pixmap, smooth: bool, repeat: bool },
}

pub fn premul(c: [f32; 4]) -> [f32; 4] {
    [c[0] * c[3], c[1] * c[3], c[2] * c[3], c[3]]
}

fn spread_t(t: f64, s: Spread) -> f64 {
    match s {
        Spread::Pad => t.clamp(0.0, 1.0),
        Spread::Repeat => t - floor(t),
        Spread::Reflect => {
            let m = t - 2.0 * floor(t / 2.0);
            if m > 1.0 { 2.0 - m } else { m }
        }
    }
}

fn stops_at(stops: &[Stop], t: f64) -> [f32; 4] {
    if t <= stops[0].offset {
        return premul(stops[0].color);
    }
    let last = stops[stops.len() - 1];
    if t >= last.offset {
        return premul(last.color);
    }
    for w in stops.windows(2) {
        let (a, b) = (w[0], w[1]);
        if t >= a.offset && t < b.offset {
            let span = b.offset - a.offset;
            if span <= 0.0 {
                return premul(b.color);
            }
            let f = ((t - a.offset) / span) as f32;
            let mut c = [0f32; 4];
            for k in 0..4 {
                c[k] = a.color[k] + (b.color[k] - a.color[k]) * f;
            }
            return premul(c);
        }
    }
    premul(last.color)
}

fn sample(pix: &Pixmap, x: i64, y: i64, repeat: bool) -> [f32; 4] {
    let (w, h) = (pix.w as i64, pix.h as i64);
    let (x, y) = if repeat {
        (x.rem_euclid(w), y.rem_euclid(h))
    } else {
        if x < 0 || y < 0 || x >= w || y >= h {
            return [0.0; 4];
        }
        (x, y)
    };
    let i = ((y * w + x) * 4) as usize;
    let d = &pix.data[i..i + 4];
    [d[0] as f32 / 255.0, d[1] as f32 / 255.0, d[2] as f32 / 255.0, d[3] as f32 / 255.0]
}

impl Shader {
    /// Premultiplied colour at device point (x, y) (a pixel centre).
    pub fn at(&self, x: f64, y: f64) -> [f32; 4] {
        match self {
            Shader::Solid(c) => *c,
            Shader::Linear { inv, p1, p2, stops, spread } => {
                let (u, v) = inv.apply(x, y);
                let d = (p2.0 - p1.0, p2.1 - p1.1);
                let len2 = d.0 * d.0 + d.1 * d.1;
                let t = if len2 == 0.0 { 0.0 } else { ((u - p1.0) * d.0 + (v - p1.1) * d.1) / len2 };
                stops_at(stops, spread_t(t, *spread))
            }
            Shader::Radial { inv, c, r, f, fr, stops, spread } => {
                let (u, v) = inv.apply(x, y);
                let cd = (c.0 - f.0, c.1 - f.1);
                let pd = (u - f.0, v - f.1);
                let dr = r - fr;
                let a = cd.0 * cd.0 + cd.1 * cd.1 - dr * dr;
                let b = pd.0 * cd.0 + pd.1 * cd.1 + fr * dr;
                let cc = pd.0 * pd.0 + pd.1 * pd.1 - fr * fr;
                let t = if a.abs() < 1e-9 {
                    if b == 0.0 {
                        // Concentric equal circles: Chromium paints the inside with t = 0, the outside not at all.
                        if cc <= 0.0 {
                            return stops_at(stops, spread_t(0.0, *spread));
                        }
                        return [0.0; 4];
                    }
                    let t = cc / (2.0 * b);
                    if fr + t * dr < 0.0 {
                        return [0.0; 4];
                    }
                    t
                } else {
                    let disc = b * b - a * cc;
                    if disc < 0.0 {
                        return [0.0; 4];
                    }
                    let s = sqrt(disc);
                    let t1 = (b + s) / a;
                    let t2 = (b - s) / a;
                    let (hi, lo) = if t1 > t2 { (t1, t2) } else { (t2, t1) };
                    if fr + hi * dr >= 0.0 {
                        hi
                    } else if fr + lo * dr >= 0.0 {
                        lo
                    } else {
                        return [0.0; 4];
                    }
                };
                stops_at(stops, spread_t(t, *spread))
            }
            Shader::Image { inv, pix, smooth, repeat } => {
                let (u, v) = inv.apply(x, y);
                if !*smooth {
                    return sample(pix, floor(u) as i64, floor(v) as i64, *repeat);
                }
                let (fu, fv) = (u - 0.5, v - 0.5);
                let (x0, y0) = (floor(fu), floor(fv));
                let (ax, ay) = ((fu - x0) as f32, (fv - y0) as f32);
                let (x0, y0) = (x0 as i64, y0 as i64);
                let c00 = sample(pix, x0, y0, *repeat);
                let c10 = sample(pix, x0 + 1, y0, *repeat);
                let c01 = sample(pix, x0, y0 + 1, *repeat);
                let c11 = sample(pix, x0 + 1, y0 + 1, *repeat);
                let mut o = [0f32; 4];
                for k in 0..4 {
                    let top = c00[k] + (c10[k] - c00[k]) * ax;
                    let bot = c01[k] + (c11[k] - c01[k]) * ax;
                    o[k] = top + (bot - top) * ay;
                }
                o
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spread_and_linear() {
        assert_eq!(spread_t(1.25, Spread::Repeat), 0.25);
        assert_eq!(spread_t(1.25, Spread::Reflect), 0.75);
        assert_eq!(spread_t(-0.25, Spread::Reflect), 0.25);
        assert_eq!(spread_t(-0.25, Spread::Pad), 0.0);
        let stops = alloc::vec![Stop { offset: 0.0, color: [1.0, 0.0, 0.0, 1.0] }, Stop { offset: 1.0, color: [0.0, 0.0, 1.0, 1.0] }];
        let s = Shader::Linear { inv: Transform::IDENTITY, p1: (0.0, 0.0), p2: (10.0, 0.0), stops, spread: Spread::Pad };
        let c = s.at(5.0, 3.0);
        assert!((c[0] - 0.5).abs() < 1e-6 && (c[2] - 0.5).abs() < 1e-6);
    }

    #[test]
    fn radial_centered_and_focal() {
        let stops = alloc::vec![Stop { offset: 0.0, color: [0.0, 0.0, 0.0, 1.0] }, Stop { offset: 1.0, color: [1.0, 1.0, 1.0, 1.0] }];
        let s = Shader::Radial { inv: Transform::IDENTITY, c: (0.0, 0.0), r: 10.0, f: (0.0, 0.0), fr: 0.0, stops: stops.clone(), spread: Spread::Pad };
        assert!((s.at(5.0, 0.0)[0] - 0.5).abs() < 1e-6);
        assert!((s.at(0.0, -2.5)[0] - 0.25).abs() < 1e-6);
        // Focal at (5,0): the point (5,0) is t=0; (10,0) is on the circle (t=1); (-10,0) too.
        let s = Shader::Radial { inv: Transform::IDENTITY, c: (0.0, 0.0), r: 10.0, f: (5.0, 0.0), fr: 0.0, stops, spread: Spread::Pad };
        assert!(s.at(5.0, 0.0)[0].abs() < 1e-6);
        assert!((s.at(-10.0, 0.0)[0] - 1.0).abs() < 1e-6);
        assert!((s.at(-2.5, 0.0)[0] - 0.5).abs() < 1e-6);
    }
}
