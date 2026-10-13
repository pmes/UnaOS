//! Coverage and strokes. Filling goes through **font_core's exact-area scanline rasterizer**
//! (`font_core::raster::Rasterizer` — the accumulation FONTCORE draws glyphs with and svg_core fills SVG
//! with: one rasterizer, three users). Each device-space edge deposits its signed area; the running sum per
//! pixel is the integral of the winding number over the pixel, folded by the fill rule (nonzero: min(1, |a|);
//! evenodd: |a| mod 2 reflected about 1).
//!
//! Strokes (HTML §4.12.5.1.? "trace a path") are built in USER space — line width, caps, joins and dashes are
//! user-space quantities — as a union of positively oriented polygons (one quad per segment, a join piece per
//! interior vertex, a cap per open end), then mapped through the CTM and filled nonzero.

use super::geom::{Matrix, P};
use font_core::raster::Rasterizer;
use std::f64::consts::PI;

/// A coverage region: `w × h` values in 0..=1 placed at (`x`, `y`) on the canvas.
#[derive(Clone, Debug, Default)]
pub struct Coverage {
    pub x: usize,
    pub y: usize,
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Coverage {
    pub fn at(&self, x: usize, y: usize) -> f32 {
        if x < self.x || y < self.y || x >= self.x + self.w || y >= self.y + self.h {
            return 0.0;
        }
        self.data[(y - self.y) * self.w + (x - self.x)]
    }
}

/// Rasterize device-space polygons (every subpath implicitly closed) clipped to a `w × h` canvas.
pub fn fill(polys: &[(Vec<P>, bool)], evenodd: bool, w: usize, h: usize) -> Option<Coverage> {
    let (mut x0, mut y0, mut x1, mut y1) = (f64::INFINITY, f64::INFINITY, f64::NEG_INFINITY, f64::NEG_INFINITY);
    for (pts, _) in polys {
        for p in pts {
            if !(p.0.is_finite() && p.1.is_finite()) {
                return None;
            }
            x0 = x0.min(p.0);
            y0 = y0.min(p.1);
            x1 = x1.max(p.0);
            y1 = y1.max(p.1);
        }
    }
    if x0 > x1 || w == 0 || h == 0 {
        return None;
    }
    // Left of the canvas still matters for winding (edges clamp onto column 0), so x starts at 0 at least.
    let bx0 = x0.floor().max(0.0).min(w as f64) as usize;
    let by0 = y0.floor().max(0.0).min(h as f64) as usize;
    let bx1 = ((x1.ceil() + 1.0).max(0.0).min(w as f64)) as usize;
    let by1 = ((y1.ceil() + 1.0).max(0.0).min(h as f64)) as usize;
    if bx1 <= bx0 || by1 <= by0 {
        return None;
    }
    let (cw, ch) = (bx1 - bx0, by1 - by0);
    let mut r = Rasterizer::new(cw, ch);
    r.snap_26_6 = false;
    let (ox, oy) = (bx0 as f64, by0 as f64);
    // Keep coordinates in a range f32 holds exactly enough; far-off points clamp (winding is preserved by the
    // rasterizer's own x clamp, and y outside the box contributes nothing).
    let lim = 1.0e7;
    let cl = |v: f64| v.clamp(-lim, lim) as f32;
    for (pts, _) in polys {
        if pts.len() < 2 {
            continue;
        }
        for k in 0..pts.len() {
            let a = pts[k];
            let b = pts[(k + 1) % pts.len()];
            r.line((cl(a.0 - ox), cl(a.1 - oy)), (cl(b.0 - ox), cl(b.1 - oy)));
        }
    }
    let area = r.signed_area();
    let mut data = vec![0f32; cw * ch];
    let mut any = false;
    for (i, &a) in area.iter().enumerate() {
        let mut v = a.abs();
        if evenodd {
            v %= 2.0;
            if v > 1.0 {
                v = 2.0 - v;
            }
        } else if v > 1.0 {
            v = 1.0;
        }
        if v > 1.0 / 512.0 {
            any = true;
            data[i] = v;
        }
    }
    if !any {
        return None;
    }
    Some(Coverage { x: bx0, y: by0, w: cw, h: ch, data })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cap {
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Join {
    Round,
    Bevel,
    Miter,
}

#[derive(Clone, Debug)]
pub struct StrokeStyle {
    pub width: f64,
    pub cap: Cap,
    pub join: Join,
    pub miter: f64,
    pub dash: Vec<f64>,
    pub dash_offset: f64,
}

fn sub(a: P, b: P) -> P {
    (a.0 - b.0, a.1 - b.1)
}

fn unit(v: P) -> P {
    let l = v.0.hypot(v.1);
    if l == 0.0 { (0.0, 0.0) } else { (v.0 / l, v.1 / l) }
}

fn signed_area(pts: &[P]) -> f64 {
    let n = pts.len();
    let mut a = 0.0;
    for k in 0..n {
        let p = pts[k];
        let q = pts[(k + 1) % n];
        a += p.0 * q.1 - q.0 * p.1;
    }
    a / 2.0
}

fn push_oriented(out: &mut Vec<(Vec<P>, bool)>, mut pts: Vec<P>) {
    if pts.len() < 3 {
        return;
    }
    let a = signed_area(&pts);
    if a == 0.0 || !a.is_finite() {
        return;
    }
    if a < 0.0 {
        pts.reverse();
    }
    out.push((pts, true));
}

fn circle(c: P, r: f64, tol: f64) -> Vec<P> {
    let n = if r <= tol {
        8
    } else {
        ((2.0 * PI / (1.0 - tol / r).clamp(-1.0, 1.0).acos()).ceil() as usize).clamp(8, 512)
    };
    (0..n)
        .map(|k| {
            let a = 2.0 * PI * k as f64 / n as f64;
            (c.0 + r * a.cos(), c.1 + r * a.sin())
        })
        .collect()
}

/// Apply the dash pattern to user-space polylines (closed ones walk their closing edge too), yielding open
/// polylines for the "on" intervals. Each subpath restarts the pattern at the dash offset.
pub fn dash(polys: &[(Vec<P>, bool)], pattern: &[f64], offset: f64) -> Vec<(Vec<P>, bool)> {
    let total: f64 = pattern.iter().sum();
    if pattern.is_empty() || total <= 0.0 || !total.is_finite() {
        return polys.to_vec();
    }
    let mut out = Vec::new();
    for (pts, closed) in polys {
        if pts.len() < 2 {
            continue;
        }
        let mut pos = offset % total;
        if pos < 0.0 {
            pos += total;
        }
        let mut idx = 0usize;
        while pos >= pattern[idx] {
            pos -= pattern[idx];
            idx = (idx + 1) % pattern.len();
            if pattern.iter().all(|v| *v == 0.0) {
                break;
            }
        }
        let mut remaining = pattern[idx] - pos;
        let mut on = idx % 2 == 0;
        let mut cur: Vec<P> = Vec::new();
        if on {
            cur.push(pts[0]);
        }
        let n = pts.len();
        let edges = if *closed { n } else { n - 1 };
        for k in 0..edges {
            let a = pts[k];
            let b = pts[(k + 1) % n];
            let len = (b.0 - a.0).hypot(b.1 - a.1);
            let mut t0 = 0.0;
            let mut guard = 0usize;
            while len - t0 > remaining && guard < 1_000_000 {
                guard += 1;
                t0 += remaining;
                let f = t0 / len;
                let p = (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
                if on {
                    cur.push(p);
                    if cur.len() >= 2 {
                        out.push((std::mem::take(&mut cur), false));
                    }
                    cur.clear();
                } else {
                    cur.clear();
                    cur.push(p);
                }
                on = !on;
                idx = (idx + 1) % pattern.len();
                remaining = pattern[idx];
            }
            remaining -= len - t0;
            if on {
                if cur.is_empty() {
                    cur.push(a);
                }
                cur.push(b);
            }
        }
        if on && cur.len() >= 2 {
            out.push((cur, false));
        }
    }
    out
}

/// The stroke outline of user-space polylines as positively oriented polygons (union = the stroke).
/// `tol` is the user-space flattening tolerance for round joins/caps.
pub fn stroke_polys(polys: &[(Vec<P>, bool)], st: &StrokeStyle, tol: f64) -> Vec<(Vec<P>, bool)> {
    let hw = st.width / 2.0;
    let mut out = Vec::new();
    if hw <= 0.0 || !hw.is_finite() {
        return out;
    }
    let dashed;
    let polys = if st.dash.is_empty() {
        polys
    } else {
        dashed = dash(polys, &st.dash, st.dash_offset);
        &dashed[..]
    };
    for (raw, closed) in polys {
        // Prune zero-length segments.
        let mut pts: Vec<P> = Vec::with_capacity(raw.len());
        for p in raw {
            if pts.last() != Some(p) {
                pts.push(*p);
            }
        }
        if *closed && pts.len() > 1 && pts.first() == pts.last() {
            pts.pop();
        }
        if pts.len() < 2 {
            continue;
        }
        let n = pts.len();
        let edges = if *closed { n } else { n - 1 };
        // Segment bodies.
        for k in 0..edges {
            let a = pts[k];
            let b = pts[(k + 1) % n];
            let d = unit(sub(b, a));
            let nn = (-d.1 * hw, d.0 * hw);
            push_oriented(&mut out, vec![(a.0 + nn.0, a.1 + nn.1), (b.0 + nn.0, b.1 + nn.1), (b.0 - nn.0, b.1 - nn.1), (a.0 - nn.0, a.1 - nn.1)]);
        }
        // Joins.
        let join_at = |out: &mut Vec<(Vec<P>, bool)>, a: P, b: P, c: P| {
            let d0 = unit(sub(b, a));
            let d1 = unit(sub(c, b));
            let cross = d0.0 * d1.1 - d0.1 * d1.0;
            let dot = d0.0 * d1.0 + d0.1 * d1.1;
            if cross.abs() < 1e-12 && dot > 0.0 {
                return;
            }
            if st.join == Join::Round {
                push_oriented(out, circle(b, hw, tol));
                return;
            }
            let s = if cross > 0.0 { -1.0 } else { 1.0 };
            let n0 = (-d0.1, d0.0);
            let n1 = (-d1.1, d1.0);
            let o0 = (b.0 + s * n0.0 * hw, b.1 + s * n0.1 * hw);
            let o1 = (b.0 + s * n1.0 * hw, b.1 + s * n1.1 * hw);
            if st.join == Join::Miter {
                let cos_half = ((1.0 + dot) / 2.0).max(0.0).sqrt();
                if cos_half > 0.0 && 1.0 / cos_half <= st.miter {
                    let m = unit((n0.0 + n1.0, n0.1 + n1.1));
                    let len = hw / cos_half;
                    let tip = (b.0 + s * m.0 * len, b.1 + s * m.1 * len);
                    push_oriented(out, vec![b, o0, tip, o1]);
                    return;
                }
            }
            push_oriented(out, vec![b, o0, o1]);
        };
        if *closed {
            for k in 0..n {
                let a = pts[(k + n - 1) % n];
                join_at(&mut out, a, pts[k], pts[(k + 1) % n]);
            }
        } else {
            for k in 1..n - 1 {
                join_at(&mut out, pts[k - 1], pts[k], pts[k + 1]);
            }
            // Caps.
            let cap = |out: &mut Vec<(Vec<P>, bool)>, p: P, d: P| match st.cap {
                Cap::Butt => {}
                Cap::Round => push_oriented(out, circle(p, hw, tol)),
                Cap::Square => {
                    let nn = (-d.1 * hw, d.0 * hw);
                    let e = (p.0 + d.0 * hw, p.1 + d.1 * hw);
                    push_oriented(out, vec![(p.0 + nn.0, p.1 + nn.1), (e.0 + nn.0, e.1 + nn.1), (e.0 - nn.0, e.1 - nn.1), (p.0 - nn.0, p.1 - nn.1)]);
                }
            };
            let d_start = unit(sub(pts[0], pts[1]));
            let d_end = unit(sub(pts[n - 1], pts[n - 2]));
            cap(&mut out, pts[0], d_start);
            cap(&mut out, pts[n - 1], d_end);
        }
    }
    out
}

/// Map polygons through a matrix.
pub fn transform_polys(polys: &[(Vec<P>, bool)], m: &Matrix) -> Vec<(Vec<P>, bool)> {
    polys.iter().map(|(pts, c)| (pts.iter().map(|p| m.apply(*p)).collect(), *c)).collect()
}

/// A separable approximation of a Gaussian blur of standard deviation `sigma` over a single-channel
/// `w × h` buffer: three box blurs (W3C filter-effects §15.17's recipe for feGaussianBlur).
pub fn blur(data: &mut [f32], w: usize, h: usize, sigma: f64) {
    if sigma <= 0.0 || w == 0 || h == 0 {
        return;
    }
    let d = ((sigma * 3.0 * (2.0 * PI).sqrt() / 4.0) + 0.5).floor() as usize;
    if d == 0 {
        return;
    }
    let mut tmp = vec![0f32; data.len()];
    // Box sizes: for odd d three boxes of d centred; for even d two of d (offset) and one of d+1.
    let boxes: [(usize, isize); 3] = if d % 2 == 1 {
        [(d, (d / 2) as isize), (d, (d / 2) as isize), (d, (d / 2) as isize)]
    } else {
        [(d, (d / 2) as isize), (d, (d / 2) as isize - 1), (d + 1, (d / 2) as isize)]
    };
    let pass = |src: &[f32], dst: &mut [f32], len: usize, count: usize, stride: usize, step: usize, size: usize, left: isize| {
        for line in 0..count {
            let base = line * stride;
            let mut sum = 0.0f32;
            // Window covers [i - left, i - left + size).
            let get = |i: isize| -> f32 { if i < 0 || i >= len as isize { 0.0 } else { src[base + i as usize * step] } };
            for j in 0..size as isize {
                sum += get(j - left);
            }
            for i in 0..len as isize {
                dst[base + i as usize * step] = sum / size as f32;
                sum += get(i - left + size as isize) - get(i - left);
            }
        }
    };
    for (size, left) in boxes {
        pass(data, &mut tmp, w, h, w, 1, size, left);
        data.copy_from_slice(&tmp);
    }
    for (size, left) in boxes {
        pass(data, &mut tmp, h, w, 1, w, size, left);
        data.copy_from_slice(&tmp);
    }
}
