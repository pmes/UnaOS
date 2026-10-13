//! Fill and stroke styles (HTML §4.12.5.1.8): solid colours, `CanvasGradient` (linear, radial — the
//! two-circle cone of the spec — and conic), `CanvasPattern` and the image paint `drawImage` is built on.
//! A paint is evaluated per device pixel at the pixel centre, mapped back through the inverse of the
//! transform in force when the drawing operation ran. Gradient colours interpolate in straight
//! (non-premultiplied) RGBA, as the spec's colour-stop algorithm says; the result is premultiplied.

use super::geom::Matrix;
use std::f64::consts::PI;
use std::rc::Rc;

/// Straight RGBA, channels 0..=1.
pub type Rgba = [f64; 4];

#[derive(Clone, Debug, PartialEq)]
pub struct Stop {
    pub offset: f64,
    pub color: Rgba,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GradientKind {
    Linear { x0: f64, y0: f64, x1: f64, y1: f64 },
    Radial { x0: f64, y0: f64, r0: f64, x1: f64, y1: f64, r1: f64 },
    Conic { angle: f64, x: f64, y: f64 },
}

#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    pub kind: GradientKind,
    pub stops: Vec<Stop>,
}

/// Straight-RGBA 8-bit pixels of an image source, shared.
#[derive(Clone, Debug)]
pub struct Bitmap {
    pub w: usize,
    pub h: usize,
    pub rgba: Rc<Vec<u8>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Repeat {
    Both,
    X,
    Y,
    None,
}

#[derive(Clone, Debug)]
pub struct Pattern {
    pub image: Bitmap,
    pub repeat: Repeat,
    pub transform: Matrix,
}

/// A resolved paint ready for sampling: everything mapped to device space.
pub enum Paint {
    Solid([f32; 4]),
    Gradient { inv: Matrix, g: Gradient },
    Image { inv: Matrix, img: Bitmap, repeat: Repeat, smooth: bool, src: (f64, f64, f64, f64) },
    Nothing,
}

pub fn premul(c: Rgba) -> [f32; 4] {
    let a = c[3].clamp(0.0, 1.0);
    [(c[0] * a) as f32, (c[1] * a) as f32, (c[2] * a) as f32, a as f32]
}

impl Gradient {
    /// The colour at offset `t` (pad extension beyond the ends).
    pub fn color_at(&self, t: f64) -> [f32; 4] {
        let s = &self.stops;
        if s.is_empty() {
            return [0.0; 4];
        }
        if t <= s[0].offset {
            return premul(s[0].color);
        }
        let last = s.len() - 1;
        if t >= s[last].offset {
            return premul(s[last].color);
        }
        // The last stop at or before t, and the one after it.
        let mut i = 0;
        while i + 1 < s.len() && s[i + 1].offset <= t {
            i += 1;
        }
        let a = &s[i];
        let b = &s[(i + 1).min(last)];
        let span = b.offset - a.offset;
        if span <= 0.0 {
            return premul(b.color);
        }
        let f = (t - a.offset) / span;
        let mut c = [0.0; 4];
        for k in 0..4 {
            c[k] = a.color[k] + (b.color[k] - a.color[k]) * f;
        }
        premul(c)
    }

    /// The colour at a user-space point, or None where the gradient paints nothing.
    pub fn sample(&self, x: f64, y: f64) -> Option<[f32; 4]> {
        if self.stops.is_empty() {
            return None;
        }
        match self.kind {
            GradientKind::Linear { x0, y0, x1, y1 } => {
                let (dx, dy) = (x1 - x0, y1 - y0);
                let l2 = dx * dx + dy * dy;
                if l2 == 0.0 {
                    return None;
                }
                Some(self.color_at(((x - x0) * dx + (y - y0) * dy) / l2))
            }
            GradientKind::Radial { x0, y0, r0, x1, y1, r1 } => {
                if x0 == x1 && y0 == y1 && r0 == r1 {
                    return None;
                }
                let (cdx, cdy, dr) = (x1 - x0, y1 - y0, r1 - r0);
                let (px, py) = (x - x0, y - y0);
                let a = cdx * cdx + cdy * cdy - dr * dr;
                let b = px * cdx + py * cdy + r0 * dr;
                let c = px * px + py * py - r0 * r0;
                let ok = |w: f64| r0 + w * dr >= 0.0;
                let w = if a.abs() < 1e-12 {
                    if b == 0.0 {
                        return None;
                    }
                    let w = c / (2.0 * b);
                    if !ok(w) {
                        return None;
                    }
                    w
                } else {
                    let disc = b * b - a * c;
                    if disc < 0.0 {
                        return None;
                    }
                    let sq = disc.sqrt();
                    let (w1, w2) = ((b + sq) / a, (b - sq) / a);
                    let (hi, lo) = if w1 > w2 { (w1, w2) } else { (w2, w1) };
                    if ok(hi) {
                        hi
                    } else if ok(lo) {
                        lo
                    } else {
                        return None;
                    }
                };
                Some(self.color_at(w))
            }
            GradientKind::Conic { angle, x: cx, y: cy } => {
                let a = (y - cy).atan2(x - cx) - angle;
                let mut t = a / (2.0 * PI);
                t -= t.floor();
                Some(self.color_at(t))
            }
        }
    }
}

impl Bitmap {
    #[inline]
    fn texel(&self, x: i64, y: i64) -> [f32; 4] {
        let i = (y as usize * self.w + x as usize) * 4;
        let p = &self.rgba[i..i + 4];
        let a = p[3] as f32 / 255.0;
        [p[0] as f32 / 255.0 * a, p[1] as f32 / 255.0 * a, p[2] as f32 / 255.0 * a, a]
    }
}

fn wrap(v: i64, n: i64) -> i64 {
    let r = v % n;
    if r < 0 { r + n } else { r }
}

impl Paint {
    /// Premultiplied colour at device pixel (`px`, `py`) (its centre is sampled).
    #[inline]
    pub fn sample(&self, px: usize, py: usize) -> [f32; 4] {
        match self {
            Paint::Solid(c) => *c,
            Paint::Nothing => [0.0; 4],
            Paint::Gradient { inv, g } => {
                let (x, y) = inv.apply((px as f64 + 0.5, py as f64 + 0.5));
                g.sample(x, y).unwrap_or([0.0; 4])
            }
            Paint::Image { inv, img, repeat, smooth, src } => {
                let (u, v) = inv.apply((px as f64 + 0.5, py as f64 + 0.5));
                sample_image(img, *repeat, *smooth, *src, u, v)
            }
        }
    }
}

/// Sample an image at image-space (`u`, `v`), restricted to the source rectangle `src` (x, y, w, h) — a
/// pattern passes the whole image. Outside the rectangle in a non-repeating direction is transparent.
pub fn sample_image(img: &Bitmap, repeat: Repeat, smooth: bool, src: (f64, f64, f64, f64), u: f64, v: f64) -> [f32; 4] {
    if img.w == 0 || img.h == 0 || !u.is_finite() || !v.is_finite() {
        return [0.0; 4];
    }
    let (sx, sy, sw, sh) = src;
    let rx = matches!(repeat, Repeat::Both | Repeat::X);
    let ry = matches!(repeat, Repeat::Both | Repeat::Y);
    if (!rx && (u < sx || u >= sx + sw)) || (!ry && (v < sy || v >= sy + sh)) {
        return [0.0; 4];
    }
    let (w, h) = (img.w as i64, img.h as i64);
    let x_lo = sx.floor().max(0.0) as i64;
    let y_lo = sy.floor().max(0.0) as i64;
    let x_hi = ((sx + sw).ceil() as i64).min(w) - 1;
    let y_hi = ((sy + sh).ceil() as i64).min(h) - 1;
    if x_hi < x_lo || y_hi < y_lo {
        return [0.0; 4];
    }
    let fix_x = |x: i64| if rx { wrap(x, w) } else { x.clamp(x_lo, x_hi) };
    let fix_y = |y: i64| if ry { wrap(y, h) } else { y.clamp(y_lo, y_hi) };
    if !smooth {
        return img.texel(fix_x(u.floor() as i64), fix_y(v.floor() as i64));
    }
    let fu = u - 0.5;
    let fv = v - 0.5;
    let x0 = fu.floor();
    let y0 = fv.floor();
    let tx = (fu - x0) as f32;
    let ty = (fv - y0) as f32;
    let (x0, y0) = (x0 as i64, y0 as i64);
    let c00 = img.texel(fix_x(x0), fix_y(y0));
    let c10 = img.texel(fix_x(x0 + 1), fix_y(y0));
    let c01 = img.texel(fix_x(x0), fix_y(y0 + 1));
    let c11 = img.texel(fix_x(x0 + 1), fix_y(y0 + 1));
    let mut out = [0f32; 4];
    for k in 0..4 {
        let top = c00[k] + (c10[k] - c00[k]) * tx;
        let bot = c01[k] + (c11[k] - c01[k]) * tx;
        out[k] = top + (bot - top) * ty;
    }
    out
}
