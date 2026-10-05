//! Geometry: affine transforms and the `transform` attribute grammar (SVG 1.1 §7.6), the path data grammar
//! (SVG 1.1 §8.3 / SVG 2 §9.3 with SVG 2's "render up to the first error" rule), elliptical arcs converted to
//! center parameterization (SVG 1.1 Appendix F.6.5, out-of-range radii corrected per F.6.6) and then to cubic
//! Béziers, tight bounding boxes (curve extrema), and flattening to polylines.

use crate::fmath::{acos, ceil, hypot, sin_cos, sqrt, tan};
use alloc::vec::Vec;
use core::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Transform {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Default for Transform {
    fn default() -> Self {
        Transform::IDENTITY
    }
}

impl Transform {
    pub const IDENTITY: Transform = Transform { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };
    pub fn new(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Self {
        Transform { a, b, c, d, e, f }
    }
    pub fn translate(x: f64, y: f64) -> Self {
        Transform::new(1.0, 0.0, 0.0, 1.0, x, y)
    }
    pub fn scale(x: f64, y: f64) -> Self {
        Transform::new(x, 0.0, 0.0, y, 0.0, 0.0)
    }
    pub fn rotate(deg: f64) -> Self {
        let (s, c) = sin_cos(deg * PI / 180.0);
        Transform::new(c, s, -s, c, 0.0, 0.0)
    }
    /// `self * other`: apply `other` first, then `self` (the order of a transform list read left to right).
    pub fn then_pre(&self, o: &Transform) -> Transform {
        Transform {
            a: self.a * o.a + self.c * o.b,
            b: self.b * o.a + self.d * o.b,
            c: self.a * o.c + self.c * o.d,
            d: self.b * o.c + self.d * o.d,
            e: self.a * o.e + self.c * o.f + self.e,
            f: self.b * o.e + self.d * o.f + self.f,
        }
    }
    /// Alias of [`Transform::then_pre`]: the matrix product `self × o`.
    pub fn mul(&self, o: &Transform) -> Transform {
        self.then_pre(o)
    }
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (self.a * x + self.c * y + self.e, self.b * x + self.d * y + self.f)
    }
    pub fn apply_vec(&self, x: f64, y: f64) -> (f64, f64) {
        (self.a * x + self.c * y, self.b * x + self.d * y)
    }
    pub fn det(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }
    pub fn invert(&self) -> Option<Transform> {
        let det = self.det();
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        let id = 1.0 / det;
        Some(Transform {
            a: self.d * id,
            b: -self.b * id,
            c: -self.c * id,
            d: self.a * id,
            e: (self.c * self.f - self.d * self.e) * id,
            f: (self.b * self.e - self.a * self.f) * id,
        })
    }
    /// Geometric-mean scale factor (sqrt |det|), used for flattening tolerances and stroke widths.
    pub fn mean_scale(&self) -> f64 {
        sqrt(self.det().abs())
    }
    /// The larger singular value — the most a unit length can grow.
    pub fn max_scale(&self) -> f64 {
        let p = self.a * self.a + self.b * self.b + self.c * self.c + self.d * self.d;
        let q = self.det();
        let disc = sqrt((p * p - 4.0 * q * q).max(0.0));
        sqrt((p + disc) / 2.0)
    }
    pub fn is_finite(&self) -> bool {
        [self.a, self.b, self.c, self.d, self.e, self.f].iter().all(|v| v.is_finite())
    }
}

/// A number per the SVG/CSS number grammar, from `s[*i..]`, skipping leading whitespace (not commas).
pub fn number(s: &[u8], i: &mut usize) -> Option<f64> {
    while *i < s.len() && s[*i].is_ascii_whitespace() {
        *i += 1;
    }
    let st = *i;
    let mut j = *i;
    if j < s.len() && (s[j] == b'+' || s[j] == b'-') {
        j += 1;
    }
    let ds = j;
    while j < s.len() && s[j].is_ascii_digit() {
        j += 1;
    }
    let mut digits = j - ds;
    if j < s.len() && s[j] == b'.' {
        j += 1;
        let fs = j;
        while j < s.len() && s[j].is_ascii_digit() {
            j += 1;
        }
        digits += j - fs;
    }
    if digits == 0 {
        return None;
    }
    if j < s.len() && (s[j] == b'e' || s[j] == b'E') {
        let mut k = j + 1;
        if k < s.len() && (s[k] == b'+' || s[k] == b'-') {
            k += 1;
        }
        let es = k;
        while k < s.len() && s[k].is_ascii_digit() {
            k += 1;
        }
        if k > es {
            j = k;
        }
    }
    let t = core::str::from_utf8(&s[st..j]).ok()?;
    let v: f64 = t.parse().ok()?;
    *i = j;
    if v.is_finite() { Some(v) } else { None }
}

/// Skip whitespace and at most one comma.
pub fn comma_wsp(s: &[u8], i: &mut usize) {
    while *i < s.len() && s[*i].is_ascii_whitespace() {
        *i += 1;
    }
    if *i < s.len() && s[*i] == b',' {
        *i += 1;
        while *i < s.len() && s[*i].is_ascii_whitespace() {
            *i += 1;
        }
    }
}

/// A list of numbers separated by whitespace and/or commas. `None` on any garbage.
pub fn number_list(s: &str) -> Option<Vec<f64>> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    loop {
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        out.push(number(b, &mut i)?);
        comma_wsp(b, &mut i);
    }
    Some(out)
}

/// The `transform` attribute (SVG 1.1 §7.6). `None` when the list is malformed (the attribute is then ignored).
pub fn parse_transform(s: &str) -> Option<Transform> {
    let b = s.as_bytes();
    let mut i = 0;
    let mut t = Transform::IDENTITY;
    loop {
        while i < b.len() && (b[i].is_ascii_whitespace() || b[i] == b',') {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let ns = i;
        while i < b.len() && b[i].is_ascii_alphabetic() {
            i += 1;
        }
        let name = &s[ns..i];
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if b.get(i) != Some(&b'(') {
            return None;
        }
        i += 1;
        let mut args = Vec::new();
        loop {
            while i < b.len() && b[i].is_ascii_whitespace() {
                i += 1;
            }
            if b.get(i) == Some(&b')') {
                i += 1;
                break;
            }
            args.push(number(b, &mut i)?);
            comma_wsp(b, &mut i);
        }
        let m = match (name, args.len()) {
            ("matrix", 6) => Transform::new(args[0], args[1], args[2], args[3], args[4], args[5]),
            ("translate", 1) => Transform::translate(args[0], 0.0),
            ("translate", 2) => Transform::translate(args[0], args[1]),
            ("scale", 1) => Transform::scale(args[0], args[0]),
            ("scale", 2) => Transform::scale(args[0], args[1]),
            ("rotate", 1) => Transform::rotate(args[0]),
            ("rotate", 3) => Transform::translate(args[1], args[2])
                .mul(&Transform::rotate(args[0]))
                .mul(&Transform::translate(-args[1], -args[2])),
            ("skewX", 1) => Transform::new(1.0, 0.0, tan(args[0] * PI / 180.0), 1.0, 0.0, 0.0),
            ("skewY", 1) => Transform::new(1.0, tan(args[0] * PI / 180.0), 0.0, 1.0, 0.0, 0.0),
            _ => return None,
        };
        t = t.mul(&m);
    }
    Some(t)
}


/// `preserveAspectRatio` (SVG 1.1 §7.8): alignment per axis (0 = min, 1 = mid, 2 = max) or `none`, and slice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Par {
    pub align: Option<(u8, u8)>,
    pub slice: bool,
}

impl Default for Par {
    fn default() -> Self {
        Par { align: Some((1, 1)), slice: false }
    }
}

pub fn parse_par(s: &str) -> Par {
    let mut w = s.split_whitespace();
    let mut first = w.next().unwrap_or("");
    if first == "defer" {
        first = w.next().unwrap_or("");
    }
    let align = match first {
        "none" => None,
        "xMinYMin" => Some((0, 0)),
        "xMidYMin" => Some((1, 0)),
        "xMaxYMin" => Some((2, 0)),
        "xMinYMid" => Some((0, 1)),
        "xMidYMid" => Some((1, 1)),
        "xMaxYMid" => Some((2, 1)),
        "xMinYMax" => Some((0, 2)),
        "xMidYMax" => Some((1, 2)),
        "xMaxYMax" => Some((2, 2)),
        _ => return Par::default(),
    };
    let slice = match w.next() {
        Some("slice") => true,
        Some("meet") | None => false,
        Some(_) => return Par::default(),
    };
    Par { align, slice }
}

/// `viewBox`: four numbers, width and height positive. `None` disables it (SVG 1.1 §7.7: ≤ 0 disables rendering
/// for width/height 0 — the caller checks that case).
pub fn parse_view_box(s: &str) -> Option<Rect> {
    let v = number_list(s)?;
    if v.len() != 4 {
        return None;
    }
    Some(Rect::new(v[0], v[1], v[2], v[3]))
}

/// The transform that maps `vb` into the viewport (x, y, w, h) per `par` (SVG 1.1 §7.8 algorithm).
pub fn view_box_transform(vb: &Rect, par: &Par, x: f64, y: f64, w: f64, h: f64) -> Transform {
    let mut sx = w / vb.w;
    let mut sy = h / vb.h;
    let (mut tx, mut ty) = (x - vb.x * sx, y - vb.y * sy);
    if let Some((ax, ay)) = par.align {
        let s = if par.slice { sx.max(sy) } else { sx.min(sy) };
        sx = s;
        sy = s;
        tx = x - vb.x * s;
        ty = y - vb.y * s;
        let ew = vb.w * s;
        let eh = vb.h * s;
        match ax {
            1 => tx += (w - ew) / 2.0,
            2 => tx += w - ew,
            _ => {}
        }
        match ay {
            1 => ty += (h - eh) / 2.0,
            2 => ty += h - eh,
            _ => {}
        }
    }
    Transform::new(sx, 0.0, 0.0, sy, tx, ty)
}

pub type P = (f64, f64);

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Move(P),
    Line(P),
    Quad(P, P),
    Cubic(P, P, P),
    Close,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    pub segs: Vec<Seg>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    pub fn new(x: f64, y: f64, w: f64, h: f64) -> Rect {
        Rect { x, y, w, h }
    }
    pub fn right(&self) -> f64 {
        self.x + self.w
    }
    pub fn bottom(&self) -> f64 {
        self.y + self.h
    }
    pub fn union(&self, o: &Rect) -> Rect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect::new(x, y, self.right().max(o.right()) - x, self.bottom().max(o.bottom()) - y)
    }
    /// Bounding box of this rectangle after `t`.
    pub fn transform(&self, t: &Transform) -> Rect {
        let pts = [t.apply(self.x, self.y), t.apply(self.right(), self.y), t.apply(self.x, self.bottom()), t.apply(self.right(), self.bottom())];
        let mut r = Bounds::new();
        for p in pts {
            r.add(p);
        }
        r.rect().unwrap_or(Rect::new(0.0, 0.0, 0.0, 0.0))
    }
}

pub struct Bounds {
    x0: f64,
    y0: f64,
    x1: f64,
    y1: f64,
}

impl Bounds {
    pub fn new() -> Self {
        Bounds { x0: f64::INFINITY, y0: f64::INFINITY, x1: f64::NEG_INFINITY, y1: f64::NEG_INFINITY }
    }
    pub fn add(&mut self, p: P) {
        self.x0 = self.x0.min(p.0);
        self.y0 = self.y0.min(p.1);
        self.x1 = self.x1.max(p.0);
        self.y1 = self.y1.max(p.1);
    }
    pub fn rect(&self) -> Option<Rect> {
        if self.x0 > self.x1 { None } else { Some(Rect::new(self.x0, self.y0, self.x1 - self.x0, self.y1 - self.y0)) }
    }
}

impl Default for Bounds {
    fn default() -> Self {
        Self::new()
    }
}

fn cubic_at(p0: f64, p1: f64, p2: f64, p3: f64, t: f64) -> f64 {
    let mt = 1.0 - t;
    mt * mt * mt * p0 + 3.0 * mt * mt * t * p1 + 3.0 * mt * t * t * p2 + t * t * t * p3
}

/// Parameters in (0,1) where a cubic's derivative in one coordinate vanishes.
fn cubic_extrema(p0: f64, p1: f64, p2: f64, p3: f64, out: &mut Vec<f64>) {
    let a = -p0 + 3.0 * p1 - 3.0 * p2 + p3;
    let b = 2.0 * (p0 - 2.0 * p1 + p2);
    let c = p1 - p0;
    if a.abs() < 1e-12 {
        if b.abs() > 1e-12 {
            out.push(-c / b);
        }
    } else {
        let disc = b * b - 4.0 * a * c;
        if disc >= 0.0 {
            let s = sqrt(disc);
            out.push((-b + s) / (2.0 * a));
            out.push((-b - s) / (2.0 * a));
        }
    }
}

impl Path {
    pub fn new() -> Self {
        Path { segs: Vec::new() }
    }
    pub fn move_to(&mut self, x: f64, y: f64) {
        self.segs.push(Seg::Move((x, y)));
    }
    pub fn line_to(&mut self, x: f64, y: f64) {
        self.segs.push(Seg::Line((x, y)));
    }
    pub fn cubic_to(&mut self, a: P, b: P, p: P) {
        self.segs.push(Seg::Cubic(a, b, p));
    }
    pub fn quad_to(&mut self, a: P, p: P) {
        self.segs.push(Seg::Quad(a, p));
    }
    pub fn close(&mut self) {
        self.segs.push(Seg::Close);
    }
    pub fn is_empty(&self) -> bool {
        !self.segs.iter().any(|s| !matches!(s, Seg::Move(_) | Seg::Close))
    }

    pub fn transform(&self, t: &Transform) -> Path {
        let m = |p: P| t.apply(p.0, p.1);
        Path {
            segs: self
                .segs
                .iter()
                .map(|s| match *s {
                    Seg::Move(p) => Seg::Move(m(p)),
                    Seg::Line(p) => Seg::Line(m(p)),
                    Seg::Quad(a, p) => Seg::Quad(m(a), m(p)),
                    Seg::Cubic(a, b, p) => Seg::Cubic(m(a), m(b), m(p)),
                    Seg::Close => Seg::Close,
                })
                .collect(),
        }
    }

    pub fn extend(&mut self, o: &Path) {
        self.segs.extend_from_slice(&o.segs);
    }

    /// Tight bounding box (curve extrema included). `None` for an empty path.
    pub fn bbox(&self) -> Option<Rect> {
        let mut b = Bounds::new();
        let mut cur = (0.0, 0.0);
        let mut ts = Vec::new();
        for s in &self.segs {
            match *s {
                Seg::Move(p) => {
                    // A lone moveto contributes nothing; it is added with its first drawing segment.
                    cur = p;
                }
                Seg::Line(p) => {
                    b.add(cur);
                    b.add(p);
                    cur = p;
                }
                Seg::Quad(a, p) => {
                    let c1 = (cur.0 + 2.0 / 3.0 * (a.0 - cur.0), cur.1 + 2.0 / 3.0 * (a.1 - cur.1));
                    let c2 = (p.0 + 2.0 / 3.0 * (a.0 - p.0), p.1 + 2.0 / 3.0 * (a.1 - p.1));
                    cubic_bounds(&mut b, &mut ts, cur, c1, c2, p);
                    cur = p;
                }
                Seg::Cubic(c1, c2, p) => {
                    cubic_bounds(&mut b, &mut ts, cur, c1, c2, p);
                    cur = p;
                }
                Seg::Close => {}
            }
        }
        b.rect()
    }

    /// Flatten to polylines: one `(points, closed)` per subpath, curves subdivided so the chord error stays
    /// below `tol` (in the path's own units).
    pub fn flatten(&self, tol: f64) -> Vec<(Vec<P>, bool)> {
        let tol = tol.max(1e-6);
        let mut out: Vec<(Vec<P>, bool)> = Vec::new();
        let mut cur: Vec<P> = Vec::new();
        let mut start = (0.0, 0.0);
        let mut last = (0.0, 0.0);
        let flush = |cur: &mut Vec<P>, out: &mut Vec<(Vec<P>, bool)>, closed: bool| {
            if !cur.is_empty() {
                out.push((core::mem::take(cur), closed));
            }
        };
        for s in &self.segs {
            match *s {
                Seg::Move(p) => {
                    flush(&mut cur, &mut out, false);
                    cur.push(p);
                    start = p;
                    last = p;
                }
                Seg::Line(p) => {
                    if cur.is_empty() {
                        cur.push(last);
                    }
                    cur.push(p);
                    last = p;
                }
                Seg::Quad(a, p) => {
                    if cur.is_empty() {
                        cur.push(last);
                    }
                    let dd = hypot(last.0 - 2.0 * a.0 + p.0, last.1 - 2.0 * a.1 + p.1);
                    let n = (ceil(sqrt(dd / (4.0 * tol))) as usize).clamp(1, 1000);
                    for k in 1..=n {
                        let t = k as f64 / n as f64;
                        let mt = 1.0 - t;
                        cur.push((mt * mt * last.0 + 2.0 * mt * t * a.0 + t * t * p.0, mt * mt * last.1 + 2.0 * mt * t * a.1 + t * t * p.1));
                    }
                    last = p;
                }
                Seg::Cubic(c1, c2, p) => {
                    if cur.is_empty() {
                        cur.push(last);
                    }
                    let d1 = hypot(last.0 - 2.0 * c1.0 + c2.0, last.1 - 2.0 * c1.1 + c2.1);
                    let d2 = hypot(c1.0 - 2.0 * c2.0 + p.0, c1.1 - 2.0 * c2.1 + p.1);
                    let dd = d1.max(d2);
                    let n = (ceil(sqrt(3.0 * dd / (4.0 * tol))) as usize).clamp(1, 1000);
                    for k in 1..=n {
                        let t = k as f64 / n as f64;
                        cur.push((cubic_at(last.0, c1.0, c2.0, p.0, t), cubic_at(last.1, c1.1, c2.1, p.1, t)));
                    }
                    last = p;
                }
                Seg::Close => {
                    if !cur.is_empty() {
                        flush(&mut cur, &mut out, true);
                    }
                    last = start;
                    // A drawing command after Z starts a new subpath at the old start (handled by `cur` empty).
                }
            }
        }
        flush(&mut cur, &mut out, false);
        out
    }
}

fn cubic_bounds(b: &mut Bounds, ts: &mut Vec<f64>, p0: P, p1: P, p2: P, p3: P) {
    b.add(p0);
    b.add(p3);
    ts.clear();
    cubic_extrema(p0.0, p1.0, p2.0, p3.0, ts);
    cubic_extrema(p0.1, p1.1, p2.1, p3.1, ts);
    for &t in ts.iter() {
        if t > 0.0 && t < 1.0 {
            b.add((cubic_at(p0.0, p1.0, p2.0, p3.0, t), cubic_at(p0.1, p1.1, p2.1, p3.1, t)));
        }
    }
}

/// Append an SVG elliptical arc from `p0` to `p` as cubic Béziers (F.6.5 + F.6.6).
pub fn arc_to(path: &mut Path, p0: P, rx: f64, ry: f64, phi_deg: f64, large: bool, sweep: bool, p: P) {
    if p0 == p {
        return; // F.6.2: endpoints identical → omit the arc
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx == 0.0 || ry == 0.0 {
        path.line_to(p.0, p.1);
        return;
    }
    let (sphi, cphi) = sin_cos(phi_deg * PI / 180.0);
    // Step 1: (x1', y1')
    let dx2 = (p0.0 - p.0) / 2.0;
    let dy2 = (p0.1 - p.1) / 2.0;
    let x1p = cphi * dx2 + sphi * dy2;
    let y1p = -sphi * dx2 + cphi * dy2;
    // F.6.6 radius correction.
    let lambda = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
    if lambda > 1.0 {
        let s = sqrt(lambda);
        rx *= s;
        ry *= s;
    }
    // Step 2: (cx', cy')
    let num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
    let den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
    let mut coef = if den == 0.0 { 0.0 } else { sqrt((num / den).max(0.0)) };
    if large == sweep {
        coef = -coef;
    }
    let cxp = coef * rx * y1p / ry;
    let cyp = -coef * ry * x1p / rx;
    // Step 3: (cx, cy)
    let cx = cphi * cxp - sphi * cyp + (p0.0 + p.0) / 2.0;
    let cy = sphi * cxp + cphi * cyp + (p0.1 + p.1) / 2.0;
    // Step 4: θ1, Δθ
    let angle = |ux: f64, uy: f64, vx: f64, vy: f64| {
        let dot = ux * vx + uy * vy;
        let len = hypot(ux, uy) * hypot(vx, vy);
        let mut a = acos((dot / len).clamp(-1.0, 1.0));
        if ux * vy - uy * vx < 0.0 {
            a = -a;
        }
        a
    };
    let ux = (x1p - cxp) / rx;
    let uy = (y1p - cyp) / ry;
    let vx = (-x1p - cxp) / rx;
    let vy = (-y1p - cyp) / ry;
    let theta1 = angle(1.0, 0.0, ux, uy);
    let mut dtheta = angle(ux, uy, vx, vy);
    if !sweep && dtheta > 0.0 {
        dtheta -= 2.0 * PI;
    } else if sweep && dtheta < 0.0 {
        dtheta += 2.0 * PI;
    }
    // Split into ≤ 90° pieces, each a cubic with handle length 4/3 tan(θ/4).
    let n = ceil(dtheta.abs() / (PI / 2.0) - 1e-9).max(1.0) as usize;
    let step = dtheta / n as f64;
    let k = 4.0 / 3.0 * tan(step / 4.0);
    let pt = |t: f64| {
        let (st, ct) = sin_cos(t);
        (cx + rx * ct * cphi - ry * st * sphi, cy + rx * ct * sphi + ry * st * cphi)
    };
    let deriv = |t: f64| {
        let (st, ct) = sin_cos(t);
        (-rx * st * cphi - ry * ct * sphi, -rx * st * sphi + ry * ct * cphi)
    };
    let mut t = theta1;
    for i in 0..n {
        let t2 = t + step;
        let a = pt(t);
        let b = if i + 1 == n { p } else { pt(t2) };
        let da = deriv(t);
        let db = deriv(t2);
        path.cubic_to((a.0 + k * da.0, a.1 + k * da.1), (b.0 - k * db.0, b.1 - k * db.1), b);
        t = t2;
    }
}

/// Path data (`d`). Parsing stops at the first error, keeping everything before it (SVG 2 §9.5.4).
pub fn parse_path(d: &str) -> Path {
    let s = d.as_bytes();
    let mut i = 0;
    let mut path = Path::new();
    let mut cur = (0.0f64, 0.0f64);
    let mut start = (0.0f64, 0.0f64);
    let mut last_ctrl: Option<P> = None; // reflected control for S / T
    let mut last_cmd = 0u8;
    let mut cmd = 0u8;
    let mut first = true;
    loop {
        while i < s.len() && s[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= s.len() {
            break;
        }
        let c = s[i];
        if c.is_ascii_alphabetic() {
            if !b"MmZzLlHhVvCcSsQqTtAa".contains(&c) {
                break;
            }
            cmd = c;
            i += 1;
        } else if cmd == 0 || cmd == b'Z' || cmd == b'z' {
            break; // numbers with no command, or after closepath
        }
        if first && cmd != b'M' && cmd != b'm' {
            break; // path data must begin with a moveto
        }
        first = false;
        let rel = cmd.is_ascii_lowercase();
        let up = cmd.to_ascii_uppercase();
        macro_rules! num {
            () => {{
                match number(s, &mut i) {
                    Some(v) => {
                        comma_wsp(s, &mut i);
                        v
                    }
                    None => break,
                }
            }};
        }
        macro_rules! flag {
            () => {{
                while i < s.len() && s[i].is_ascii_whitespace() {
                    i += 1;
                }
                match s.get(i) {
                    Some(b'0') => {
                        i += 1;
                        comma_wsp(s, &mut i);
                        false
                    }
                    Some(b'1') => {
                        i += 1;
                        comma_wsp(s, &mut i);
                        true
                    }
                    _ => break,
                }
            }};
        }
        let base = if rel { cur } else { (0.0, 0.0) };
        match up {
            b'M' => {
                let x = num!();
                let y = num!();
                cur = (base.0 + x, base.1 + y);
                start = cur;
                path.move_to(cur.0, cur.1);
                cmd = if rel { b'l' } else { b'L' };
                last_ctrl = None;
            }
            b'Z' => {
                path.close();
                cur = start;
                last_ctrl = None;
                // A following command with no new moveto starts at the subpath start.
                comma_wsp(s, &mut i);
            }
            b'L' => {
                let x = num!();
                let y = num!();
                ensure_move(&mut path, last_cmd, start);
                cur = (base.0 + x, base.1 + y);
                path.line_to(cur.0, cur.1);
                last_ctrl = None;
            }
            b'H' => {
                let x = num!();
                ensure_move(&mut path, last_cmd, start);
                cur = (base.0 + x, cur.1);
                path.line_to(cur.0, cur.1);
                last_ctrl = None;
            }
            b'V' => {
                let y = num!();
                ensure_move(&mut path, last_cmd, start);
                cur = (cur.0, if rel { cur.1 + y } else { y });
                path.line_to(cur.0, cur.1);
                last_ctrl = None;
            }
            b'C' => {
                let (x1, y1, x2, y2, x, y) = (num!(), num!(), num!(), num!(), num!(), num!());
                ensure_move(&mut path, last_cmd, start);
                let c1 = (base.0 + x1, base.1 + y1);
                let c2 = (base.0 + x2, base.1 + y2);
                cur = (base.0 + x, base.1 + y);
                path.cubic_to(c1, c2, cur);
                last_ctrl = Some(c2);
            }
            b'S' => {
                let (x2, y2, x, y) = (num!(), num!(), num!(), num!());
                ensure_move(&mut path, last_cmd, start);
                let c1 = match (last_ctrl, last_cmd.to_ascii_uppercase()) {
                    (Some(lc), b'C' | b'S') => (2.0 * cur.0 - lc.0, 2.0 * cur.1 - lc.1),
                    _ => cur,
                };
                let c2 = (base.0 + x2, base.1 + y2);
                cur = (base.0 + x, base.1 + y);
                path.cubic_to(c1, c2, cur);
                last_ctrl = Some(c2);
            }
            b'Q' => {
                let (x1, y1, x, y) = (num!(), num!(), num!(), num!());
                ensure_move(&mut path, last_cmd, start);
                let c = (base.0 + x1, base.1 + y1);
                cur = (base.0 + x, base.1 + y);
                path.quad_to(c, cur);
                last_ctrl = Some(c);
            }
            b'T' => {
                let (x, y) = (num!(), num!());
                ensure_move(&mut path, last_cmd, start);
                let c = match (last_ctrl, last_cmd.to_ascii_uppercase()) {
                    (Some(lc), b'Q' | b'T') => (2.0 * cur.0 - lc.0, 2.0 * cur.1 - lc.1),
                    _ => cur,
                };
                cur = (base.0 + x, base.1 + y);
                path.quad_to(c, cur);
                last_ctrl = Some(c);
            }
            b'A' => {
                let rx = num!();
                let ry = num!();
                let rot = num!();
                let large = flag!();
                let sweep = flag!();
                let x = num!();
                let y = num!();
                ensure_move(&mut path, last_cmd, start);
                let p = (base.0 + x, base.1 + y);
                arc_to(&mut path, cur, rx, ry, rot, large, sweep, p);
                cur = p;
                last_ctrl = None;
            }
            _ => break,
        }
        last_cmd = up;
        if up == b'Z' {
            last_cmd = b'Z';
        }
    }
    path
}

/// After a closepath, a drawing command implicitly starts a new subpath at the previous start.
fn ensure_move(path: &mut Path, last_cmd: u8, start: P) {
    if last_cmd == b'Z' {
        path.move_to(start.0, start.1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9
    }

    #[test]
    fn transforms() {
        let t = parse_transform("translate(10,20) scale(2)").unwrap();
        assert_eq!(t.apply(1.0, 1.0), (12.0, 22.0));
        let r = parse_transform("rotate(90 10 10)").unwrap();
        let p = r.apply(20.0, 10.0);
        assert!(close(p.0, 10.0) && close(p.1, 20.0));
        let m = parse_transform(" matrix(1 0 0 1 5 6) , skewX(45)").unwrap();
        let p = m.apply(0.0, 10.0);
        assert!(close(p.0, 15.0) && close(p.1, 16.0));
        assert!(parse_transform("scale(1,2,3)").is_none());
        assert!(parse_transform("translate(10").is_none());
        let inv = t.invert().unwrap();
        let q = inv.apply(12.0, 22.0);
        assert!(close(q.0, 1.0) && close(q.1, 1.0));
    }

    #[test]
    fn view_box() {
        let vb = parse_view_box("0,0 100 50").unwrap();
        let t = view_box_transform(&vb, &parse_par("xMidYMid"), 0.0, 0.0, 200.0, 200.0);
        assert_eq!(t, Transform::new(2.0, 0.0, 0.0, 2.0, 0.0, 50.0));
        let t = view_box_transform(&vb, &parse_par("xMaxYMax slice"), 0.0, 0.0, 200.0, 200.0);
        assert_eq!(t, Transform::new(4.0, 0.0, 0.0, 4.0, -200.0, 0.0));
        let t = view_box_transform(&vb, &parse_par("none"), 0.0, 0.0, 200.0, 200.0);
        assert_eq!(t, Transform::new(2.0, 0.0, 0.0, 4.0, 0.0, 0.0));
        assert_eq!(parse_par("defer xMinYMin meet"), Par { align: Some((0, 0)), slice: false });
        assert_eq!(parse_par("bogus"), Par::default());
        assert!(parse_view_box("0 0 10").is_none());
    }

    #[test]
    fn path_grammar() {
        let p = parse_path("M10-20.5.5 1L1e1,2zl1 1H5V6h1v1C1 2 3 4 5 6S7 8 9 10Q1 1 2 2T3 3");
        assert_eq!(p.segs[0], Seg::Move((10.0, -20.5)));
        assert_eq!(p.segs[1], Seg::Line((0.5, 1.0))); // implicit lineto after M
        assert_eq!(p.segs[2], Seg::Line((10.0, 2.0)));
        assert_eq!(p.segs[3], Seg::Close);
        assert_eq!(p.segs[4], Seg::Move((10.0, -20.5)));
        assert_eq!(p.segs[5], Seg::Line((11.0, -19.5)));
        // S reflects the previous C's second control point.
        match p.segs[11] {
            Seg::Cubic(c1, _, _) => assert_eq!(c1, (7.0, 8.0)),
            _ => panic!(),
        }
        match p.segs[13] {
            Seg::Quad(c, e) => {
                assert_eq!(c, (3.0, 3.0));
                assert_eq!(e, (3.0, 3.0));
            }
            _ => panic!(),
        }
        // Error: keep what came before.
        let e = parse_path("M 10 10 L 20 20 L 30 X 40 40");
        assert_eq!(e.segs.len(), 2);
        assert!(parse_path("L 10 10").segs.is_empty());
        // Compact arc flags.
        let a = parse_path("M0 0a5 5 0 1010 0");
        assert!(matches!(a.segs.last(), Some(Seg::Cubic(_, _, (x, y))) if close(*x, 10.0) && close(*y, 0.0)));
    }

    #[test]
    fn arc_center_parameterization() {
        // Half circle radius 50 from (0,0) to (100,0), sweep=1 → passes through (50,-50)? With y down and
        // sweep-flag 1 (positive angle direction), the arc bulges to negative y... check the bbox instead.
        let mut p = Path::new();
        p.move_to(0.0, 0.0);
        arc_to(&mut p, (0.0, 0.0), 50.0, 50.0, 0.0, false, true, (100.0, 0.0));
        let b = p.bbox().unwrap();
        assert!(close(b.w, 100.0));
        assert!((b.h - 50.0).abs() < 1e-3);
        assert!(b.y < -1.0); // sweep=1 from left to right goes through y = -50 (clockwise on screen)
        // Radii too small are scaled up (F.6.6): same half circle.
        let mut q = Path::new();
        q.move_to(0.0, 0.0);
        arc_to(&mut q, (0.0, 0.0), 1.0, 1.0, 0.0, false, true, (100.0, 0.0));
        let bq = q.bbox().unwrap();
        assert!((bq.h - 50.0).abs() < 1e-3);
        // A full ellipse drawn as two arcs has area π a b.
        let mut e = Path::new();
        e.move_to(0.0, 0.0);
        arc_to(&mut e, (0.0, 0.0), 30.0, 10.0, 0.0, true, true, (60.0, 0.0));
        arc_to(&mut e, (60.0, 0.0), 30.0, 10.0, 0.0, true, true, (0.0, 0.0));
        let poly = &e.flatten(0.001)[0].0;
        let mut area = 0.0;
        for k in 0..poly.len() {
            let (a, b) = (poly[k], poly[(k + 1) % poly.len()]);
            area += a.0 * b.1 - b.0 * a.1;
        }
        assert!(((area / 2.0).abs() - PI * 300.0).abs() / (PI * 300.0) < 1e-3, "{area} {}", poly.len());
    }
}
