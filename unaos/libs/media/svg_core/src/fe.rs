//! The filter primitives' pixel operations (Filter Effects Module Level 1 §15), on premultiplied RGBA8 images
//! that all share one working raster (the filter's device-aligned working space; [`crate::filter`] builds the
//! graph and maps every length into it). Each operation reads its inputs and writes only inside the primitive
//! subregion `b` (integer pixel bounds, `x0..x1 × y0..y1`); everything outside stays transparent black.
//!
//! Precision follows Skia, which is what Chromium (the oracle) runs these primitives on: 8-bit premultiplied
//! intermediates, colour filters (matrix, component transfer, colour-space conversion) evaluated on
//! unpremultiplied values, the blur as three box blurs summed in integers and rounded once per axis.

use crate::fmath::{floor, powf, sin_cos, sqrt};
use crate::raster::Pixmap;
use alloc::vec;
use alloc::vec::Vec;

/// Integer pixel bounds inside the working raster.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IRect {
    pub x0: usize,
    pub y0: usize,
    pub x1: usize,
    pub y1: usize,
}

impl IRect {
    pub fn is_empty(&self) -> bool {
        self.x1 <= self.x0 || self.y1 <= self.y0
    }
    pub fn intersect(&self, o: &IRect) -> IRect {
        let r = IRect { x0: self.x0.max(o.x0), y0: self.y0.max(o.y0), x1: self.x1.min(o.x1), y1: self.y1.min(o.y1) };
        if r.is_empty() { IRect { x0: 0, y0: 0, x1: 0, y1: 0 } } else { r }
    }
    pub fn contains(&self, x: usize, y: usize) -> bool {
        x >= self.x0 && x < self.x1 && y >= self.y0 && y < self.y1
    }
}

#[inline]
fn px(img: &Pixmap, x: usize, y: usize) -> [u8; 4] {
    let i = (y * img.w + x) * 4;
    [img.data[i], img.data[i + 1], img.data[i + 2], img.data[i + 3]]
}

#[inline]
fn put(img: &mut Pixmap, x: usize, y: usize, v: [u8; 4]) {
    let i = (y * img.w + x) * 4;
    img.data[i..i + 4].copy_from_slice(&v);
}

#[inline]
fn to8(v: f32) -> u8 {
    (v * 255.0 + 0.5).clamp(0.0, 255.0) as u8
}

/// Transparent everywhere outside `b`.
pub fn clip_to(img: &mut Pixmap, b: &IRect) {
    let w = img.w;
    for y in 0..img.h {
        for x in 0..w {
            if !b.contains(x, y) {
                let i = (y * w + x) * 4;
                img.data[i..i + 4].fill(0);
            }
        }
    }
}

// ---------------------------------------------------------------- colour spaces (§15.4 color-interpolation-filters)

/// sRGB → linear-light (IEC 61966-2-1 EOTF).
pub fn srgb_to_linear(v: f32) -> f32 {
    if v <= 0.04045 { v / 12.92 } else { powf(((v as f64) + 0.055) / 1.055, 2.4) as f32 }
}

/// linear-light → sRGB.
pub fn linear_to_srgb(v: f32) -> f32 {
    if v <= 0.003_130_8 { v * 12.92 } else { (1.055 * powf(v as f64, 1.0 / 2.4) - 0.055) as f32 }
}

/// Convert a premultiplied image between sRGB and linearRGB in place: unpremultiply, apply the transfer
/// function exactly, premultiply, round (Skia's `SRGBToLinearGamma` / `LinearToSRGBGamma` colour filters).
pub fn convert(img: &mut Pixmap, to_linear: bool) {
    let f = if to_linear { srgb_to_linear } else { linear_to_srgb };
    let mut lut = [0u8; 256];
    for (i, l) in lut.iter_mut().enumerate() {
        *l = to8(f(i as f32 / 255.0));
    }
    for p in img.data.chunks_exact_mut(4) {
        let a = p[3];
        if a == 0 {
            continue;
        }
        if a == 255 {
            for c in 0..3 {
                p[c] = lut[p[c] as usize];
            }
        } else {
            let af = a as f32 / 255.0;
            for c in 0..3 {
                let u = (p[c] as f32 / 255.0 / af).min(1.0);
                p[c] = to8(f(u) * af);
            }
        }
    }
}

/// A straight colour (0..=1 floats) in the operating colour space, premultiplied.
pub fn color_in_space(rgb: [u8; 3], alpha: f32, linear: bool) -> [f32; 4] {
    let mut c = [rgb[0] as f32 / 255.0, rgb[1] as f32 / 255.0, rgb[2] as f32 / 255.0];
    if linear {
        for v in c.iter_mut() {
            // Chromium adapts the colour through 8 bits (an SkColor).
            *v = to8(srgb_to_linear(*v)) as f32 / 255.0;
        }
    }
    let a = alpha.clamp(0.0, 1.0);
    [c[0] * a, c[1] * a, c[2] * a, a]
}

// ---------------------------------------------------------------- feFlood

pub fn flood(img: &mut Pixmap, b: &IRect, c: [f32; 4]) {
    let v = [to8(c[0]), to8(c[1]), to8(c[2]), to8(c[3])];
    for y in b.y0..b.y1 {
        for x in b.x0..b.x1 {
            put(img, x, y, v);
        }
    }
}

// ---------------------------------------------------------------- feGaussianBlur (§15.17)

/// The box size `d` for a standard deviation (the spec's note: d = floor(s · 3·√(2π)/4 + 0.5)).
pub fn box_size(sigma: f64) -> usize {
    if !(sigma > 0.0) {
        return 0;
    }
    let d = floor(sigma * 3.0 * sqrt(2.0 * core::f64::consts::PI) / 4.0 + 0.5);
    d.clamp(0.0, 4096.0) as usize
}

/// The three boxes of the approximation as (left offset, length): odd `d` → three centred boxes of size d;
/// even `d` → two of size d (centred on the pixel boundary left, then right, of the output pixel) and one of
/// size d+1 centred on the output pixel.
fn boxes(d: usize) -> [(i64, usize); 3] {
    let d64 = d as i64;
    if d % 2 == 1 {
        let lo = -(d64 - 1) / 2;
        [(lo, d), (lo, d), (lo, d)]
    } else {
        [(-d64 / 2, d), (-d64 / 2 + 1, d), (-d64 / 2, d + 1)]
    }
}

/// Blur one line of `n` samples (stride `stride`, starting at `start`) of one channel, in place.
fn box3_line(data: &mut [u8], start: usize, stride: usize, n: usize, d: usize, scratch: &mut Vec<i64>, tmp: &mut Vec<i64>) {
    let pad = 2 * d + 2;
    let len = n + 2 * pad;
    scratch.clear();
    scratch.resize(len, 0);
    for i in 0..n {
        scratch[pad + i] = data[start + i * stride] as i64;
    }
    let bx = boxes(d);
    let mut div: i64 = 1;
    for &(lo, l) in &bx {
        div *= l as i64;
        // prefix sums
        tmp.clear();
        tmp.resize(len + 1, 0);
        for i in 0..len {
            tmp[i + 1] = tmp[i] + scratch[i];
        }
        for i in 0..len {
            let a = (i as i64 + lo).clamp(0, len as i64) as usize;
            let b = (i as i64 + lo + l as i64).clamp(0, len as i64) as usize;
            scratch[i] = tmp[b] - tmp[a];
        }
    }
    for i in 0..n {
        let v = (scratch[pad + i] + div / 2) / div;
        data[start + i * stride] = v.clamp(0, 255) as u8;
    }
}

/// Gaussian blur of `img` within bounds `b` (the input is taken as transparent outside `b`), with box sizes
/// `dx`, `dy` (0 or 1 = that axis unblurred). Horizontal pass first, rounded to 8 bits, then vertical.
pub fn blur(img: &mut Pixmap, b: &IRect, dx: usize, dy: usize) {
    clip_to(img, b);
    let (w, h) = (img.w, img.h);
    let mut s = Vec::new();
    let mut t = Vec::new();
    if dx > 1 {
        for y in b.y0..b.y1 {
            for c in 0..4 {
                box3_line(&mut img.data, (y * w + b.x0) * 4 + c, 4, b.x1 - b.x0, dx, &mut s, &mut t);
            }
        }
    }
    if dy > 1 {
        for x in b.x0..b.x1 {
            for c in 0..4 {
                box3_line(&mut img.data, (b.y0 * w + x) * 4 + c, w * 4, b.y1 - b.y0, dy, &mut s, &mut t);
            }
        }
    }
    let _ = h;
    fix_premul(img, b);
}

/// Keep colour ≤ alpha (rounding in separate channel blurs can break the premultiplied invariant by one).
fn fix_premul(img: &mut Pixmap, b: &IRect) {
    for y in b.y0..b.y1 {
        for x in b.x0..b.x1 {
            let i = (y * img.w + x) * 4;
            let a = img.data[i + 3];
            for c in 0..3 {
                if img.data[i + c] > a {
                    img.data[i + c] = a;
                }
            }
        }
    }
}

// ---------------------------------------------------------------- feOffset

/// Shift by (`dx`, `dy`) pixels, result clipped to `b`. A fractional shift resamples bilinearly (Skia draws
/// the offset image with a fractional translation and linear filtering).
pub fn offset(src: &Pixmap, b: &IRect, dx: f64, dy: f64) -> Pixmap {
    let mut out = Pixmap::new(src.w, src.h);
    let (fx, fy) = (dx - floor(dx), dy - floor(dy));
    let (ix, iy) = (floor(dx) as i64, floor(dy) as i64);
    let get = |x: i64, y: i64| -> [f32; 4] {
        if x < 0 || y < 0 || x >= src.w as i64 || y >= src.h as i64 {
            return [0.0; 4];
        }
        let p = px(src, x as usize, y as usize);
        [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32]
    };
    let exact = fx < 1e-6 && fy < 1e-6;
    let (wx, wy) = (fx as f32, fy as f32);
    for y in b.y0..b.y1 {
        for x in b.x0..b.x1 {
            let (sx, sy) = (x as i64 - ix, y as i64 - iy);
            if exact {
                let p = get(sx, sy);
                put(&mut out, x, y, [p[0] as u8, p[1] as u8, p[2] as u8, p[3] as u8]);
                continue;
            }
            // out(x) = (1 − f)·src(x − ⌊d⌋) + f·src(x − ⌊d⌋ − 1), per axis.
            let (a, bb, c, d) = (get(sx, sy), get(sx - 1, sy), get(sx, sy - 1), get(sx - 1, sy - 1));
            let mut o = [0u8; 4];
            for k in 0..4 {
                let top = a[k] * (1.0 - wx) + bb[k] * wx;
                let bot = c[k] * (1.0 - wx) + d[k] * wx;
                o[k] = (top * (1.0 - wy) + bot * wy + 0.5).clamp(0.0, 255.0) as u8;
            }
            put(&mut out, x, y, o);
        }
    }
    out
}

// ---------------------------------------------------------------- feComposite / feMerge / feBlend

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CompositeOp {
    Over,
    In,
    Out,
    Atop,
    Xor,
    Lighter,
    Arithmetic([f32; 4]),
}

/// `s` = in (source), `d` = in2 (destination), both premultiplied.
pub fn composite(s: &Pixmap, d: &Pixmap, b: &IRect, op: CompositeOp) -> Pixmap {
    let mut out = Pixmap::new(s.w, s.h);
    for y in b.y0..b.y1 {
        for x in b.x0..b.x1 {
            let sp = px(s, x, y);
            let dp = px(d, x, y);
            let sf = [sp[0] as f32 / 255.0, sp[1] as f32 / 255.0, sp[2] as f32 / 255.0, sp[3] as f32 / 255.0];
            let df = [dp[0] as f32 / 255.0, dp[1] as f32 / 255.0, dp[2] as f32 / 255.0, dp[3] as f32 / 255.0];
            let (sa, da) = (sf[3], df[3]);
            let mut o = [0f32; 4];
            for c in 0..4 {
                o[c] = match op {
                    CompositeOp::Over => sf[c] + df[c] * (1.0 - sa),
                    CompositeOp::In => sf[c] * da,
                    CompositeOp::Out => sf[c] * (1.0 - da),
                    CompositeOp::Atop => sf[c] * da + df[c] * (1.0 - sa),
                    CompositeOp::Xor => sf[c] * (1.0 - da) + df[c] * (1.0 - sa),
                    CompositeOp::Lighter => (sf[c] + df[c]).min(1.0),
                    CompositeOp::Arithmetic(k) => k[0] * sf[c] * df[c] + k[1] * sf[c] + k[2] * df[c] + k[3],
                };
            }
            o[3] = o[3].clamp(0.0, 1.0);
            for c in 0..3 {
                o[c] = o[c].clamp(0.0, o[3]);
            }
            put(&mut out, x, y, [to8(o[0]), to8(o[1]), to8(o[2]), to8(o[3])]);
        }
    }
    out
}

/// Source-over `s` onto `acc` within `b`.
pub fn over_into(acc: &mut Pixmap, s: &Pixmap, b: &IRect) {
    for y in b.y0..b.y1 {
        for x in b.x0..b.x1 {
            let sp = px(s, x, y);
            if sp[3] == 0 {
                continue;
            }
            let dp = px(acc, x, y);
            let inv = 255 - sp[3] as u32;
            let mut o = [0u8; 4];
            for c in 0..4 {
                o[c] = (sp[c] as u32 + crate::raster::div255(dp[c] as u32 * inv)).min(255) as u8;
            }
            put(acc, x, y, o);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlendMode {
    Normal,
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

impl BlendMode {
    pub fn parse(s: &str) -> Option<BlendMode> {
        Some(match s.trim() {
            "normal" => BlendMode::Normal,
            "multiply" => BlendMode::Multiply,
            "screen" => BlendMode::Screen,
            "overlay" => BlendMode::Overlay,
            "darken" => BlendMode::Darken,
            "lighten" => BlendMode::Lighten,
            "color-dodge" => BlendMode::ColorDodge,
            "color-burn" => BlendMode::ColorBurn,
            "hard-light" => BlendMode::HardLight,
            "soft-light" => BlendMode::SoftLight,
            "difference" => BlendMode::Difference,
            "exclusion" => BlendMode::Exclusion,
            "hue" => BlendMode::Hue,
            "saturation" => BlendMode::Saturation,
            "color" => BlendMode::Color,
            "luminosity" => BlendMode::Luminosity,
            _ => return None,
        })
    }
}

fn hard_light(cb: f32, cs: f32) -> f32 {
    if cs <= 0.5 { cb * 2.0 * cs } else { let s = 2.0 * cs - 1.0; cb + s - cb * s }
}

/// The separable blend functions B(Cb, Cs) (Compositing and Blending 1 §5.3).
fn blend_sep(m: BlendMode, cb: f32, cs: f32) -> f32 {
    match m {
        BlendMode::Normal => cs,
        BlendMode::Multiply => cb * cs,
        BlendMode::Screen => cb + cs - cb * cs,
        BlendMode::Overlay => hard_light(cs, cb),
        BlendMode::Darken => cb.min(cs),
        BlendMode::Lighten => cb.max(cs),
        BlendMode::ColorDodge => {
            if cb <= 0.0 {
                0.0
            } else if cs >= 1.0 {
                1.0
            } else {
                (cb / (1.0 - cs)).min(1.0)
            }
        }
        BlendMode::ColorBurn => {
            if cb >= 1.0 {
                1.0
            } else if cs <= 0.0 {
                0.0
            } else {
                1.0 - ((1.0 - cb) / cs).min(1.0)
            }
        }
        BlendMode::HardLight => hard_light(cb, cs),
        BlendMode::SoftLight => {
            if cs <= 0.5 {
                cb - (1.0 - 2.0 * cs) * cb * (1.0 - cb)
            } else {
                let d = if cb <= 0.25 { ((16.0 * cb - 12.0) * cb + 4.0) * cb } else { sqrt(cb as f64) as f32 };
                cb + (2.0 * cs - 1.0) * (d - cb)
            }
        }
        BlendMode::Difference => (cb - cs).abs(),
        BlendMode::Exclusion => cb + cs - 2.0 * cb * cs,
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
    if n < 0.0 {
        for v in o.iter_mut() {
            *v = l + (*v - l) * l / (l - n);
        }
    }
    if x > 1.0 {
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
    let mut o = [0f32; 3];
    if mx > mn {
        for i in 0..3 {
            o[i] = (c[i] - mn) * s / (mx - mn);
        }
    }
    o
}

/// feBlend: `s` = in (source, on top), `d` = in2 (backdrop).
pub fn blend(s: &Pixmap, d: &Pixmap, b: &IRect, m: BlendMode) -> Pixmap {
    let mut out = Pixmap::new(s.w, s.h);
    for y in b.y0..b.y1 {
        for x in b.x0..b.x1 {
            let sp = px(s, x, y);
            let dp = px(d, x, y);
            let (sa, da) = (sp[3] as f32 / 255.0, dp[3] as f32 / 255.0);
            let sc = [sp[0] as f32 / 255.0, sp[1] as f32 / 255.0, sp[2] as f32 / 255.0];
            let dc = [dp[0] as f32 / 255.0, dp[1] as f32 / 255.0, dp[2] as f32 / 255.0];
            let us = if sa > 0.0 { [sc[0] / sa, sc[1] / sa, sc[2] / sa] } else { [0.0; 3] };
            let ub = if da > 0.0 { [dc[0] / da, dc[1] / da, dc[2] / da] } else { [0.0; 3] };
            let bl = match m {
                BlendMode::Hue => set_lum(set_sat(us, sat(ub)), lum(ub)),
                BlendMode::Saturation => set_lum(set_sat(ub, sat(us)), lum(ub)),
                BlendMode::Color => set_lum(us, lum(ub)),
                BlendMode::Luminosity => set_lum(ub, lum(us)),
                _ => [blend_sep(m, ub[0], us[0]), blend_sep(m, ub[1], us[1]), blend_sep(m, ub[2], us[2])],
            };
            let ao = sa + da - sa * da;
            let mut o = [0f32; 4];
            for c in 0..3 {
                o[c] = (sc[c] * (1.0 - da) + dc[c] * (1.0 - sa) + sa * da * bl[c]).clamp(0.0, ao);
            }
            o[3] = ao;
            put(&mut out, x, y, [to8(o[0]), to8(o[1]), to8(o[2]), to8(o[3])]);
        }
    }
    out
}

// ---------------------------------------------------------------- feColorMatrix / feComponentTransfer

/// A 4×5 row-major colour matrix on unpremultiplied 0..=1 values (offsets in the 5th column).
pub fn color_matrix(img: &Pixmap, b: &IRect, m: &[f32; 20]) -> Pixmap {
    let mut out = Pixmap::new(img.w, img.h);
    for y in b.y0..b.y1 {
        for x in b.x0..b.x1 {
            let p = px(img, x, y);
            let a = p[3] as f32 / 255.0;
            let u = if a > 0.0 { [p[0] as f32 / 255.0 / a, p[1] as f32 / 255.0 / a, p[2] as f32 / 255.0 / a, a] } else { [0.0; 4] };
            let mut o = [0f32; 4];
            for r in 0..4 {
                o[r] = (m[r * 5] * u[0] + m[r * 5 + 1] * u[1] + m[r * 5 + 2] * u[2] + m[r * 5 + 3] * u[3] + m[r * 5 + 4]).clamp(0.0, 1.0);
            }
            put(&mut out, x, y, [to8(o[0] * o[3]), to8(o[1] * o[3]), to8(o[2] * o[3]), to8(o[3])]);
        }
    }
    out
}

pub const IDENTITY_MATRIX: [f32; 20] = [1., 0., 0., 0., 0., 0., 1., 0., 0., 0., 0., 0., 1., 0., 0., 0., 0., 0., 1., 0.];

pub fn saturate_matrix(s: f32) -> [f32; 20] {
    [
        0.213 + 0.787 * s, 0.715 - 0.715 * s, 0.072 - 0.072 * s, 0., 0.,
        0.213 - 0.213 * s, 0.715 + 0.285 * s, 0.072 - 0.072 * s, 0., 0.,
        0.213 - 0.213 * s, 0.715 - 0.715 * s, 0.072 + 0.928 * s, 0., 0.,
        0., 0., 0., 1., 0.,
    ]
}

pub fn hue_rotate_matrix(deg: f64) -> [f32; 20] {
    let (s, c) = sin_cos(deg * core::f64::consts::PI / 180.0);
    let (s, c) = (s as f32, c as f32);
    [
        0.213 + c * 0.787 - s * 0.213, 0.715 - c * 0.715 - s * 0.715, 0.072 - c * 0.072 + s * 0.928, 0., 0.,
        0.213 - c * 0.213 + s * 0.143, 0.715 + c * 0.285 + s * 0.140, 0.072 - c * 0.072 - s * 0.283, 0., 0.,
        0.213 - c * 0.213 - s * 0.787, 0.715 - c * 0.715 + s * 0.715, 0.072 + c * 0.928 + s * 0.072, 0., 0.,
        0., 0., 0., 1., 0.,
    ]
}

pub const LUMINANCE_TO_ALPHA: [f32; 20] = [0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0., 0.2125, 0.7154, 0.0721, 0., 0.];

/// A transfer function (§15.11).
#[derive(Clone, Debug, PartialEq)]
pub enum TransferFn {
    Identity,
    Table(Vec<f64>),
    Discrete(Vec<f64>),
    Linear { slope: f64, intercept: f64 },
    Gamma { amplitude: f64, exponent: f64, offset: f64 },
}

impl TransferFn {
    /// The 256-entry lookup table, built the way Chromium builds it for Skia (values truncated to 8 bits).
    pub fn table(&self) -> [u8; 256] {
        let mut t = [0u8; 256];
        let q = |v: f64| -> u8 { v.clamp(0.0, 255.0) as u8 };
        for (i, e) in t.iter_mut().enumerate() {
            let c = i as f64 / 255.0;
            *e = match self {
                TransferFn::Identity => i as u8,
                TransferFn::Table(v) => {
                    let n = v.len();
                    if n == 0 {
                        i as u8
                    } else {
                        let k = ((c * (n - 1) as f64) as usize).min(n - 1);
                        let v1 = v[k];
                        let v2 = v[(k + 1).min(n - 1)];
                        q(255.0 * (v1 + (c * (n - 1) as f64 - k as f64) * (v2 - v1)))
                    }
                }
                TransferFn::Discrete(v) => {
                    let n = v.len();
                    if n == 0 {
                        i as u8
                    } else {
                        let k = ((c * n as f64) as usize).min(n - 1);
                        q(255.0 * v[k])
                    }
                }
                TransferFn::Linear { slope, intercept } => q(slope * i as f64 + 255.0 * intercept),
                TransferFn::Gamma { amplitude, exponent, offset } => q(255.0 * (amplitude * powf(c, *exponent) + offset)),
            };
        }
        t
    }
}

/// feComponentTransfer: 8-bit lookup tables on unpremultiplied values.
pub fn component_transfer(img: &Pixmap, b: &IRect, t: &[[u8; 256]; 4]) -> Pixmap {
    let mut out = Pixmap::new(img.w, img.h);
    for y in b.y0..b.y1 {
        for x in b.x0..b.x1 {
            let p = px(img, x, y);
            let a = p[3] as u32;
            let mut u = [0u8; 4];
            if a > 0 {
                for c in 0..3 {
                    u[c] = ((p[c] as u32 * 255 + a / 2) / a).min(255) as u8;
                }
            }
            u[3] = p[3];
            let o = [t[0][u[0] as usize], t[1][u[1] as usize], t[2][u[2] as usize], t[3][u[3] as usize]];
            let oa = o[3] as u32;
            put(&mut out, x, y, [crate::raster::div255(o[0] as u32 * oa) as u8, crate::raster::div255(o[1] as u32 * oa) as u8, crate::raster::div255(o[2] as u32 * oa) as u8, o[3]]);
        }
    }
    out
}

// ---------------------------------------------------------------- feMorphology

/// Erode (min) or dilate (max) each channel over a (2rx+1)×(2ry+1) window; the window is truncated at the
/// input bounds `src_b` (pixels outside do not take part), the result is written inside `b`.
pub fn morphology(img: &Pixmap, src_b: &IRect, b: &IRect, rx: usize, ry: usize, dilate: bool) -> Pixmap {
    let (w, h) = (img.w, img.h);
    let pick = |a: u8, c: u8| if dilate { a.max(c) } else { a.min(c) };
    let init = if dilate { 0u8 } else { 255u8 };
    // Horizontal pass over the rows the vertical pass will read.
    let mut tmp = Pixmap::new(w, h);
    let ry0 = b.y0.saturating_sub(ry).max(src_b.y0);
    let ry1 = (b.y1 + ry).min(src_b.y1);
    for y in ry0..ry1 {
        for x in b.x0..b.x1 {
            let lo = x.saturating_sub(rx).max(src_b.x0);
            let hi = (x + rx + 1).min(src_b.x1);
            let mut o = [init; 4];
            if lo >= hi {
                o = [0; 4];
            }
            for sx in lo..hi {
                let p = px(img, sx, y);
                for c in 0..4 {
                    o[c] = pick(o[c], p[c]);
                }
            }
            put(&mut tmp, x, y, o);
        }
    }
    let mut out = Pixmap::new(w, h);
    for y in b.y0..b.y1 {
        let lo = y.saturating_sub(ry).max(src_b.y0);
        let hi = (y + ry + 1).min(src_b.y1);
        for x in b.x0..b.x1 {
            let mut o = [init; 4];
            if lo >= hi {
                o = [0; 4];
            }
            for sy in lo..hi {
                let p = px(&tmp, x, sy);
                for c in 0..4 {
                    o[c] = pick(o[c], p[c]);
                }
            }
            put(&mut out, x, y, o);
        }
    }
    out
}

// ---------------------------------------------------------------- feTile

/// Tile the `src_b` rectangle of `src` across `b`, the pattern anchored at `src_b`'s origin.
pub fn tile(src: &Pixmap, src_b: &IRect, b: &IRect) -> Pixmap {
    let mut out = Pixmap::new(src.w, src.h);
    if src_b.is_empty() {
        return out;
    }
    let (tw, th) = ((src_b.x1 - src_b.x0) as i64, (src_b.y1 - src_b.y0) as i64);
    for y in b.y0..b.y1 {
        let sy = src_b.y0 as i64 + (y as i64 - src_b.y0 as i64).rem_euclid(th);
        for x in b.x0..b.x1 {
            let sx = src_b.x0 as i64 + (x as i64 - src_b.x0 as i64).rem_euclid(tw);
            put(&mut out, x, y, px(src, sx as usize, sy as usize));
        }
    }
    out
}

// ---------------------------------------------------------------- feTurbulence (§15.23, the spec's reference code)

const BSIZE: usize = 0x100;
const BM: i64 = 0xff;
const PERLIN_N: f64 = 4096.0;
const RAND_M: i64 = 2147483647;
const RAND_A: i64 = 16807;
const RAND_Q: i64 = 127773;
const RAND_R: i64 = 2836;

fn setup_seed(mut s: i64) -> i64 {
    if s <= 0 {
        s = -(s % (RAND_M - 1)) + 1;
    }
    if s > RAND_M - 1 {
        s = RAND_M - 1;
    }
    s
}

fn random(s: i64) -> i64 {
    let mut r = RAND_A * (s % RAND_Q) - RAND_R * (s / RAND_Q);
    if r <= 0 {
        r += RAND_M;
    }
    r
}

/// The lattice and gradient tables of the reference implementation for one seed.
pub struct Turbulence {
    lattice: [usize; BSIZE + BSIZE + 2],
    gradient: [[[f64; 2]; BSIZE + BSIZE + 2]; 4],
}

struct Stitch {
    width: i64,
    height: i64,
    wrap_x: i64,
    wrap_y: i64,
}

impl Turbulence {
    pub fn new(seed: i64) -> Turbulence {
        let mut t = Turbulence { lattice: [0; BSIZE + BSIZE + 2], gradient: [[[0.0; 2]; BSIZE + BSIZE + 2]; 4] };
        let mut s = setup_seed(seed);
        for k in 0..4 {
            for i in 0..BSIZE {
                t.lattice[i] = i;
                for j in 0..2 {
                    s = random(s);
                    t.gradient[k][i][j] = ((s % (BSIZE as i64 + BSIZE as i64)) - BSIZE as i64) as f64 / BSIZE as f64;
                }
                let g = t.gradient[k][i];
                let n = sqrt(g[0] * g[0] + g[1] * g[1]);
                t.gradient[k][i] = [g[0] / n, g[1] / n];
            }
        }
        let mut i = BSIZE - 1;
        while i > 0 {
            let k = t.lattice[i];
            s = random(s);
            let j = (s % BSIZE as i64) as usize;
            t.lattice[i] = t.lattice[j];
            t.lattice[j] = k;
            i -= 1;
        }
        for i in 0..BSIZE + 2 {
            t.lattice[BSIZE + i] = t.lattice[i];
            for k in 0..4 {
                t.gradient[k][BSIZE + i] = t.gradient[k][i];
            }
        }
        t
    }

    fn noise2(&self, ch: usize, vx: f64, vy: f64, st: Option<&Stitch>) -> f64 {
        let t = vx + PERLIN_N;
        let mut bx0 = (t as i64) & BM;
        let mut bx1 = (bx0 + 1) & BM;
        let rx0 = t - (t as i64) as f64;
        let rx1 = rx0 - 1.0;
        let t = vy + PERLIN_N;
        let mut by0 = (t as i64) & BM;
        let mut by1 = (by0 + 1) & BM;
        let ry0 = t - (t as i64) as f64;
        let ry1 = ry0 - 1.0;
        if let Some(s) = st {
            if bx0 >= s.wrap_x {
                bx0 -= s.width;
            }
            if bx1 >= s.wrap_x {
                bx1 -= s.width;
            }
            if by0 >= s.wrap_y {
                by0 -= s.height;
            }
            if by1 >= s.wrap_y {
                by1 -= s.height;
            }
        }
        let (bx0, bx1, by0, by1) = ((bx0 & BM) as usize, (bx1 & BM) as usize, (by0 & BM) as usize, (by1 & BM) as usize);
        let i = self.lattice[bx0];
        let j = self.lattice[bx1];
        let b00 = self.lattice[i + by0];
        let b10 = self.lattice[j + by0];
        let b01 = self.lattice[i + by1];
        let b11 = self.lattice[j + by1];
        let sx = rx0 * rx0 * (3.0 - 2.0 * rx0);
        let sy = ry0 * ry0 * (3.0 - 2.0 * ry0);
        let g = &self.gradient[ch];
        let u = rx0 * g[b00][0] + ry0 * g[b00][1];
        let v = rx1 * g[b10][0] + ry0 * g[b10][1];
        let a = u + sx * (v - u);
        let u = rx0 * g[b01][0] + ry1 * g[b01][1];
        let v = rx1 * g[b11][0] + ry1 * g[b11][1];
        let b = u + sx * (v - u);
        a + sy * (b - a)
    }

    /// `turbulence(nColorChannel, point, …)` of the reference code. `tile` = (x, y, w, h) when stitching.
    pub fn turbulence(&self, ch: usize, p: (f64, f64), mut fx: f64, mut fy: f64, octaves: u32, fractal: bool, tile: Option<(f64, f64, f64, f64)>) -> f64 {
        let mut stitch = None;
        if let Some((tx, ty, tw, th)) = tile {
            if fx != 0.0 {
                let lo = floor(tw * fx) / tw;
                let hi = crate::fmath::ceil(tw * fx) / tw;
                fx = if fx / lo < hi / fx { lo } else { hi };
            }
            if fy != 0.0 {
                let lo = floor(th * fy) / th;
                let hi = crate::fmath::ceil(th * fy) / th;
                fy = if fy / lo < hi / fy { lo } else { hi };
            }
            let w = (tw * fx + 0.5) as i64;
            let h = (th * fy + 0.5) as i64;
            stitch = Some(Stitch { width: w, height: h, wrap_x: (tx * fx + PERLIN_N + w as f64) as i64, wrap_y: (ty * fy + PERLIN_N + h as f64) as i64 });
        }
        let mut sum = 0.0;
        let (mut vx, mut vy) = (p.0 * fx, p.1 * fy);
        let mut ratio = 1.0;
        for _ in 0..octaves {
            let n = self.noise2(ch, vx, vy, stitch.as_ref());
            sum += if fractal { n / ratio } else { n.abs() / ratio };
            vx *= 2.0;
            vy *= 2.0;
            ratio *= 2.0;
            if let Some(s) = stitch.as_mut() {
                s.width *= 2;
                s.wrap_x = 2 * s.wrap_x - PERLIN_N as i64;
                s.height *= 2;
                s.wrap_y = 2 * s.wrap_y - PERLIN_N as i64;
            }
        }
        sum
    }
}

/// Fill `b` with noise; `map(x, y)` gives the point in the noise's (user) space for pixel (x, y).
#[allow(clippy::too_many_arguments)]
pub fn turbulence(img: &mut Pixmap, b: &IRect, map: &dyn Fn(usize, usize) -> (f64, f64), seed: i64, fx: f64, fy: f64, octaves: u32, fractal: bool, tile: Option<(f64, f64, f64, f64)>) {
    let t = Turbulence::new(seed);
    for y in b.y0..b.y1 {
        for x in b.x0..b.x1 {
            let p = map(x, y);
            let mut c = [0f32; 4];
            for (ch, cv) in c.iter_mut().enumerate() {
                let v = t.turbulence(ch, p, fx, fy, octaves, fractal, tile);
                let v = if fractal { (v * 255.0 + 255.0) / 2.0 } else { v * 255.0 };
                *cv = (v.clamp(0.0, 255.0) / 255.0) as f32;
            }
            let a = c[3];
            put(img, x, y, [to8(c[0] * a), to8(c[1] * a), to8(c[2] * a), to8(a)]);
        }
    }
}

// ---------------------------------------------------------------- feDisplacementMap

/// `map` is unpremultiplied for the lookup (Skia), `scale` already in working pixels per axis.
pub fn displacement(src: &Pixmap, map: &Pixmap, b: &IRect, src_b: &IRect, sx: f64, sy: f64, xc: usize, yc: usize) -> Pixmap {
    let mut out = Pixmap::new(src.w, src.h);
    for y in b.y0..b.y1 {
        for x in b.x0..b.x1 {
            let m = px(map, x, y);
            let a = m[3] as f64;
            let un = |c: usize| -> f64 {
                if c == 3 {
                    a
                } else if a > 0.0 {
                    (m[c] as f64 * 255.0 / a).min(255.0)
                } else {
                    0.0
                }
            };
            let dx = sx * (un(xc) / 255.0 - 0.5);
            let dy = sy * (un(yc) / 255.0 - 0.5);
            // Skia truncates the displacement (towards zero) after adding one half.
            let tx = x as i64 + (dx + 0.5) as i64;
            let ty = y as i64 + (dy + 0.5) as i64;
            if tx >= src_b.x0 as i64 && tx < src_b.x1 as i64 && ty >= src_b.y0 as i64 && ty < src_b.y1 as i64 {
                put(&mut out, x, y, px(src, tx as usize, ty as usize));
            }
        }
    }
    out
}

// ---------------------------------------------------------------- feConvolveMatrix

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeMode {
    Duplicate,
    Wrap,
    None,
}

pub struct Convolve<'k> {
    pub order: (usize, usize),
    /// Kernel in the spec's order (it is applied rotated by 180°, as the spec's formula reads).
    pub kernel: &'k [f64],
    pub divisor: f64,
    pub bias: f64,
    pub target: (usize, usize),
    pub edge: EdgeMode,
    pub preserve_alpha: bool,
}

pub fn convolve(src: &Pixmap, src_b: &IRect, b: &IRect, k: &Convolve) -> Pixmap {
    let mut out = Pixmap::new(src.w, src.h);
    if src_b.is_empty() {
        return out;
    }
    let (ox, oy) = k.order;
    let fetch = |x: i64, y: i64| -> [f32; 4] {
        let (bx0, by0, bx1, by1) = (src_b.x0 as i64, src_b.y0 as i64, src_b.x1 as i64, src_b.y1 as i64);
        let (x, y) = match k.edge {
            EdgeMode::Duplicate => (x.clamp(bx0, bx1 - 1), y.clamp(by0, by1 - 1)),
            EdgeMode::Wrap => (bx0 + (x - bx0).rem_euclid(bx1 - bx0), by0 + (y - by0).rem_euclid(by1 - by0)),
            EdgeMode::None => {
                if x < bx0 || x >= bx1 || y < by0 || y >= by1 {
                    return [0.0; 4];
                }
                (x, y)
            }
        };
        let p = px(src, x as usize, y as usize);
        let mut f = [p[0] as f32, p[1] as f32, p[2] as f32, p[3] as f32];
        if k.preserve_alpha && p[3] > 0 {
            let a = p[3] as f32 / 255.0;
            for c in f.iter_mut().take(3) {
                *c = (*c / a).min(255.0);
            }
        }
        f
    };
    let gain = 1.0 / k.divisor;
    let bias = k.bias * 255.0;
    for y in b.y0..b.y1 {
        for x in b.x0..b.x1 {
            let mut s = [0f64; 4];
            for j in 0..oy {
                for i in 0..ox {
                    let p = fetch(x as i64 - k.target.0 as i64 + i as i64, y as i64 - k.target.1 as i64 + j as i64);
                    let kv = k.kernel[(oy - 1 - j) * ox + (ox - 1 - i)];
                    for c in 0..4 {
                        s[c] += p[c] as f64 * kv;
                    }
                }
            }
            let o = if k.preserve_alpha {
                let a = px(src, x, y)[3];
                if !src_b.contains(x, y) {
                    [0, 0, 0, 0]
                } else {
                    let af = a as f64 / 255.0;
                    let ch = |v: f64| ((v * gain + bias).clamp(0.0, 255.0) * af + 0.5) as u8;
                    [ch(s[0]), ch(s[1]), ch(s[2]), a]
                }
            } else {
                let a = (s[3] * gain + bias).clamp(0.0, 255.0);
                let ch = |v: f64| ((v * gain + bias).clamp(0.0, a) + 0.5) as u8;
                [ch(s[0]), ch(s[1]), ch(s[2]), (a + 0.5) as u8]
            };
            put(&mut out, x, y, o);
        }
    }
    out
}

// ---------------------------------------------------------------- feDiffuseLighting / feSpecularLighting (§15.8, §15.22)

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Light {
    /// Unit vector towards the light.
    Distant { dir: [f64; 3] },
    Point { pos: [f64; 3] },
    /// `cos_outer`: cosine of the limiting cone angle (90° when none is given); Skia anti-aliases the cone
    /// edge over 0.016 in cosine.
    Spot { pos: [f64; 3], s: [f64; 3], exponent: f64, cos_outer: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Lighting {
    Diffuse { kd: f64 },
    Specular { ks: f64, exponent: f64 },
}

fn normalize(v: [f64; 3]) -> [f64; 3] {
    let n = sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
    if n == 0.0 { v } else { [v[0] / n, v[1] / n, v[2] / n] }
}

/// Light the alpha surface of `src` inside `b`. `origin` = the working raster's pixel (0,0) in the light's
/// coordinate system (light positions are given relative to it); `color` straight 0..=255 in operating space.
pub fn lighting(src: &Pixmap, b: &IRect, light: &Light, kind: Lighting, surface_scale: f64, color: [f64; 3]) -> Pixmap {
    let mut out = Pixmap::new(src.w, src.h);
    if b.is_empty() {
        return out;
    }
    let a = |x: usize, y: usize| -> f64 { src.data[(y * src.w + x) * 4 + 3] as f64 };
    let ss = surface_scale / 255.0;
    let (w, h) = (b.x1 - b.x0, b.y1 - b.y0);
    for y in b.y0..b.y1 {
        for x in b.x0..b.x1 {
            // Sobel normal with the spec's edge kernels (Skia's left/interior/right × top/interior/bottom).
            let (l, r) = (x > b.x0, x + 1 < b.x1);
            let (t, bo) = (y > b.y0, y + 1 < b.y1);
            let g = |dx: i64, dy: i64| -> f64 { a((x as i64 + dx) as usize, (y as i64 + dy) as usize) };
            let (nx, ny) = if w == 1 && h == 1 {
                (0.0, 0.0)
            } else {
                let xl = if l { -1 } else { 0 };
                let xr = if r { 1 } else { 0 };
                let yt = if t { -1 } else { 0 };
                let yb = if bo { 1 } else { 0 };
                // Horizontal gradient: columns xl and xr, rows weighted 1-2-1 (or 2-1 at an edge).
                // The spec's FACTORx / FACTORy for interior, edge and corner pixels.
                let fx = match (l && r, t && bo) {
                    (true, true) => 0.25,
                    (true, false) => 1.0 / 3.0,
                    (false, true) => 0.5,
                    (false, false) => 2.0 / 3.0,
                };
                let fy = match (t && bo, l && r) {
                    (true, true) => 0.25,
                    (true, false) => 1.0 / 3.0,
                    (false, true) => 0.5,
                    (false, false) => 2.0 / 3.0,
                };
                let col = |cx: i64| -> f64 {
                    let mut s = 2.0 * g(cx, 0);
                    if t {
                        s += g(cx, yt);
                    }
                    if bo {
                        s += g(cx, yb);
                    }
                    s
                };
                let row = |cy: i64| -> f64 {
                    let mut s = 2.0 * g(0, cy);
                    if l {
                        s += g(xl, cy);
                    }
                    if r {
                        s += g(xr, cy);
                    }
                    s
                };
                let gx = if w == 1 { 0.0 } else { (col(xr) - col(xl)) * fx };
                let gy = if h == 1 { 0.0 } else { (row(yb) - row(yt)) * fy };
                (gx, gy)
            };
            let n = normalize([-ss * nx, -ss * ny, 1.0]);
            let z = ss * a(x, y);
            let (lv, lc) = match light {
                Light::Distant { dir } => (*dir, color),
                Light::Point { pos } => (normalize([pos[0] - x as f64, pos[1] - y as f64, pos[2] - z]), color),
                Light::Spot { pos, s, exponent, cos_outer } => {
                    let lv = normalize([pos[0] - x as f64, pos[1] - y as f64, pos[2] - z]);
                    let cos_a = -(lv[0] * s[0] + lv[1] * s[1] + lv[2] * s[2]);
                    let mut k = 0.0;
                    if cos_a >= *cos_outer {
                        k = powf(cos_a.max(0.0), *exponent);
                        const AA: f64 = 0.016;
                        if cos_a < cos_outer + AA {
                            k *= (cos_a - cos_outer) / AA;
                        }
                    }
                    (lv, [color[0] * k, color[1] * k, color[2] * k])
                }
            };
            let o = match kind {
                Lighting::Diffuse { kd } => {
                    let f = (kd * (n[0] * lv[0] + n[1] * lv[1] + n[2] * lv[2])).max(0.0);
                    let c = |v: f64| (v * f + 0.5).clamp(0.0, 255.0) as u8;
                    [c(lc[0]), c(lc[1]), c(lc[2]), 255]
                }
                Lighting::Specular { ks, exponent } => {
                    let hv = normalize([lv[0], lv[1], lv[2] + 1.0]);
                    let d = n[0] * hv[0] + n[1] * hv[1] + n[2] * hv[2];
                    let f = (ks * powf(d.max(0.0), exponent)).max(0.0);
                    let c = |v: f64| (v * f + 0.5).clamp(0.0, 255.0) as u8;
                    let (r, g, bb) = (c(lc[0]), c(lc[1]), c(lc[2]));
                    [r, g, bb, r.max(g).max(bb)]
                }
            };
            put(&mut out, x, y, o);
        }
    }
    out
}

/// Alpha only (colour zeroed) — SourceAlpha and the drop-shadow mask.
pub fn alpha_only(img: &Pixmap) -> Pixmap {
    let mut o = img.clone();
    for p in o.data.chunks_exact_mut(4) {
        p[0] = 0;
        p[1] = 0;
        p[2] = 0;
    }
    o
}

/// Every pixel of `mask`'s alpha painted with the premultiplied colour `c` (SrcIn).
pub fn colorize(mask: &Pixmap, c: [f32; 4]) -> Pixmap {
    let mut o = Pixmap::new(mask.w, mask.h);
    for (d, s) in o.data.chunks_exact_mut(4).zip(mask.data.chunks_exact(4)) {
        let a = s[3] as f32 / 255.0;
        for ch in 0..4 {
            d[ch] = to8(c[ch] * a);
        }
    }
    o
}

pub fn empty_like(img: &Pixmap) -> Pixmap {
    Pixmap { w: img.w, h: img.h, data: vec![0; img.w * img.h * 4] }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn box_sizes_and_kernel_mass() {
        // d = floor(s·3√(2π)/4 + 0.5): s = 2 → 4 (even), s = 4 → 8, s = 1 → 2, s = 5 → 9 (odd).
        assert_eq!((box_size(1.0), box_size(2.0), box_size(4.0), box_size(5.0)), (2, 4, 8, 9));
        // A blurred opaque line keeps its mass and is symmetric.
        let mut img = Pixmap::new(41, 1);
        let i = 20 * 4 + 3;
        img.data[i] = 255;
        let b = IRect { x0: 0, y0: 0, x1: 41, y1: 1 };
        blur(&mut img, &b, 5, 0);
        let a: Vec<u32> = (0..41).map(|x| img.data[x * 4 + 3] as u32).collect();
        for k in 0..41 {
            assert_eq!(a[k], a[40 - k]);
        }
        // 3 boxes of 5 → support 13 px; centre weight 19/125.
        assert_eq!(a[20], (255 * 19 + 62) / 125);
        assert_eq!(a[13], 0);
        assert!(a[14] > 0);
        // Even d: the composite kernel is still centred (two half-pixel shifts cancel).
        let mut img = Pixmap::new(41, 1);
        img.data[i] = 255;
        blur(&mut img, &b, 4, 0);
        let a: Vec<u32> = (0..41).map(|x| img.data[x * 4 + 3] as u32).collect();
        for k in 0..41 {
            assert_eq!(a[k], a[40 - k]);
        }
    }

    #[test]
    fn color_space_round_trip() {
        assert_eq!(to8(srgb_to_linear(0.5)), 55);
        assert_eq!(to8(linear_to_srgb(55.0 / 255.0)), 128);
        let mut p = Pixmap::new(1, 1);
        p.data.copy_from_slice(&[64, 32, 0, 128]);
        convert(&mut p, true);
        // unpremultiplied (0.5, 0.25, 0) → linear (0.214, 0.0508, 0) → ×128/255 → (27.4, 6.5, 0)
        assert_eq!(p.data, vec![27, 7, 0, 128]);
    }

    #[test]
    fn composite_and_blend() {
        let mut s = Pixmap::new(1, 1);
        let mut d = Pixmap::new(1, 1);
        s.data.copy_from_slice(&[128, 0, 0, 128]);
        d.data.copy_from_slice(&[0, 0, 255, 255]);
        let b = IRect { x0: 0, y0: 0, x1: 1, y1: 1 };
        assert_eq!(composite(&s, &d, &b, CompositeOp::Over).data, vec![128, 0, 127, 255]);
        assert_eq!(composite(&s, &d, &b, CompositeOp::In).data, vec![128, 0, 0, 128]);
        assert_eq!(composite(&s, &d, &b, CompositeOp::Out).data, vec![0, 0, 0, 0]);
        assert_eq!(composite(&s, &d, &b, CompositeOp::Arithmetic([0.0, 0.5, 0.5, 0.0])).data, vec![64, 0, 128, 192]);
        // multiply red over blue → black where both are opaque.
        s.data.copy_from_slice(&[255, 0, 0, 255]);
        assert_eq!(blend(&s, &d, &b, BlendMode::Multiply).data, vec![0, 0, 0, 255]);
        assert_eq!(blend(&s, &d, &b, BlendMode::Screen).data, vec![255, 0, 255, 255]);
    }

    #[test]
    fn transfer_tables_and_matrices() {
        let t = TransferFn::Table(alloc::vec![0.0, 1.0]).table();
        assert_eq!((t[0], t[128], t[255]), (0, 128, 255));
        let t = TransferFn::Discrete(alloc::vec![0.0, 1.0]).table();
        assert_eq!((t[127], t[128]), (0, 255));
        let t = TransferFn::Linear { slope: 0.5, intercept: 0.25 }.table();
        assert_eq!(t[255], 191);
        let m = saturate_matrix(1.0);
        assert_eq!(m, IDENTITY_MATRIX);
        let h = hue_rotate_matrix(0.0);
        for k in 0..20 {
            assert!((h[k] - IDENTITY_MATRIX[k]).abs() < 1e-6);
        }
    }

    #[test]
    fn turbulence_reference_values() {
        // The reference generator: seed 0 → setup_seed → 1; first random(1) = 16807.
        assert_eq!(random(setup_seed(0)), 16807);
        assert_eq!(setup_seed(-20), 21);
        let t = Turbulence::new(0);
        // Gradients are unit vectors; the lattice is a permutation.
        for k in 0..4 {
            for i in 0..BSIZE {
                let g = t.gradient[k][i];
                assert!((g[0] * g[0] + g[1] * g[1] - 1.0).abs() < 1e-9);
            }
        }
        let mut seen = [false; BSIZE];
        for i in 0..BSIZE {
            seen[t.lattice[i]] = true;
        }
        assert!(seen.iter().all(|&s| s));
        // Noise is zero on lattice points.
        assert!(t.noise2(0, 3.0, 5.0, None).abs() < 1e-12);
    }

    #[test]
    fn morphology_and_tile() {
        let mut img = Pixmap::new(5, 1);
        img.data[2 * 4 + 3] = 255;
        let b = IRect { x0: 0, y0: 0, x1: 5, y1: 1 };
        let d = morphology(&img, &b, &b, 1, 0, true);
        assert_eq!((d.data[3], d.data[7], d.data[11], d.data[15], d.data[19]), (0, 255, 255, 255, 0));
        let e = morphology(&d, &b, &b, 1, 0, false);
        assert_eq!((e.data[7], e.data[11], e.data[15]), (0, 255, 0));
        let t = tile(&img, &IRect { x0: 1, y0: 0, x1: 3, y1: 1 }, &b);
        assert_eq!((t.data[3], t.data[7], t.data[11], t.data[15], t.data[19]), (255, 0, 255, 0, 255));
    }
}
