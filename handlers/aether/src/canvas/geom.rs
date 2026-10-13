//! Geometry of the 2D context (HTML §4.12.5.1.10–11): the affine current transformation matrix, the path
//! model with its "ensure there is a subpath" rules, the arc/ellipse angle canonicalisation, Path2D's SVG
//! path-data constructor, and curve flattening to polylines.

use std::f64::consts::PI;

pub type P = (f64, f64);

/// `[a c e; b d f; 0 0 1]` — the canvas `transform(a, b, c, d, e, f)` order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Matrix {
    pub const IDENTITY: Matrix = Matrix { a: 1.0, b: 0.0, c: 0.0, d: 1.0, e: 0.0, f: 0.0 };

    pub fn new(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Matrix {
        Matrix { a, b, c, d, e, f }
    }

    pub fn translate(x: f64, y: f64) -> Matrix {
        Matrix::new(1.0, 0.0, 0.0, 1.0, x, y)
    }

    pub fn scale(x: f64, y: f64) -> Matrix {
        Matrix::new(x, 0.0, 0.0, y, 0.0, 0.0)
    }

    pub fn rotate(angle: f64) -> Matrix {
        let (s, c) = angle.sin_cos();
        Matrix::new(c, s, -s, c, 0.0, 0.0)
    }

    /// `self × m`: the transform that applies `m` first, then `self` (what `ctx.transform(m)` does to the CTM).
    pub fn mul(&self, m: &Matrix) -> Matrix {
        let s = self;
        Matrix {
            a: s.a * m.a + s.c * m.b,
            b: s.b * m.a + s.d * m.b,
            c: s.a * m.c + s.c * m.d,
            d: s.b * m.c + s.d * m.d,
            e: s.a * m.e + s.c * m.f + s.e,
            f: s.b * m.e + s.d * m.f + s.f,
        }
    }

    pub fn apply(&self, p: P) -> P {
        (self.a * p.0 + self.c * p.1 + self.e, self.b * p.0 + self.d * p.1 + self.f)
    }

    pub fn invert(&self) -> Option<Matrix> {
        let det = self.a * self.d - self.b * self.c;
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        let i = 1.0 / det;
        Some(Matrix {
            a: self.d * i,
            b: -self.b * i,
            c: -self.c * i,
            d: self.a * i,
            e: (self.c * self.f - self.d * self.e) * i,
            f: (self.b * self.e - self.a * self.f) * i,
        })
    }

    pub fn is_finite(&self) -> bool {
        [self.a, self.b, self.c, self.d, self.e, self.f].iter().all(|v| v.is_finite())
    }

    pub fn is_invertible(&self) -> bool {
        self.invert().is_some()
    }

    /// The largest factor by which the matrix stretches a length (for flattening tolerances).
    pub fn max_scale(&self) -> f64 {
        let sx = (self.a * self.a + self.b * self.b).sqrt();
        let sy = (self.c * self.c + self.d * self.d).sqrt();
        sx.max(sy).max(1e-9)
    }

    pub fn to_array(&self) -> [f64; 6] {
        [self.a, self.b, self.c, self.d, self.e, self.f]
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Seg {
    Move(P),
    Line(P),
    Quad(P, P),
    Cubic(P, P, P),
    Close,
}

/// A path: subpaths of segments, plus the bookkeeping the canvas path API needs (the "need new subpath" flag
/// is `segs.is_empty()` or a trailing state; `cur` is the last point, `start` the open subpath's first point).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    pub segs: Vec<Seg>,
    pub cur: Option<P>,
    pub start: P,
}

fn finite(v: &[f64]) -> bool {
    v.iter().all(|x| x.is_finite())
}

/// `Err(())` = IndexSizeError (a negative radius).
pub type PathResult = Result<(), ()>;

impl Path {
    pub fn new() -> Path {
        Path::default()
    }

    pub fn clear(&mut self) {
        self.segs.clear();
        self.cur = None;
    }

    pub fn has_subpaths(&self) -> bool {
        self.cur.is_some()
    }

    fn ensure(&mut self, p: P) {
        if self.cur.is_none() {
            self.move_to_dev(p);
        }
    }

    // Device-space primitives (already transformed).
    pub fn move_to_dev(&mut self, p: P) {
        self.segs.push(Seg::Move(p));
        self.cur = Some(p);
        self.start = p;
    }
    pub fn line_to_dev(&mut self, p: P) {
        if self.cur.is_none() {
            self.move_to_dev(p);
            return;
        }
        self.segs.push(Seg::Line(p));
        self.cur = Some(p);
    }
    fn quad_dev(&mut self, c: P, p: P) {
        self.ensure(c);
        self.segs.push(Seg::Quad(c, p));
        self.cur = Some(p);
    }
    fn cubic_dev(&mut self, c1: P, c2: P, p: P) {
        self.ensure(c1);
        self.segs.push(Seg::Cubic(c1, c2, p));
        self.cur = Some(p);
    }
    pub fn close(&mut self) {
        if self.cur.is_none() {
            return;
        }
        self.segs.push(Seg::Close);
        self.cur = Some(self.start);
    }

    // The canvas path API in user space; `m` maps user space to the path's space (the CTM for the context's
    // own path, identity for a Path2D).
    pub fn move_to(&mut self, m: &Matrix, x: f64, y: f64) {
        if !finite(&[x, y]) {
            return;
        }
        self.move_to_dev(m.apply((x, y)));
    }

    pub fn line_to(&mut self, m: &Matrix, x: f64, y: f64) {
        if !finite(&[x, y]) {
            return;
        }
        self.line_to_dev(m.apply((x, y)));
    }

    pub fn quadratic_curve_to(&mut self, m: &Matrix, cx: f64, cy: f64, x: f64, y: f64) {
        if !finite(&[cx, cy, x, y]) {
            return;
        }
        self.quad_dev(m.apply((cx, cy)), m.apply((x, y)));
    }

    pub fn bezier_curve_to(&mut self, m: &Matrix, c1x: f64, c1y: f64, c2x: f64, c2y: f64, x: f64, y: f64) {
        if !finite(&[c1x, c1y, c2x, c2y, x, y]) {
            return;
        }
        self.cubic_dev(m.apply((c1x, c1y)), m.apply((c2x, c2y)), m.apply((x, y)));
    }

    pub fn close_path(&mut self) {
        self.close();
    }

    /// HTML `arcTo(x1, y1, x2, y2, radius)`.
    pub fn arc_to(&mut self, m: &Matrix, x1: f64, y1: f64, x2: f64, y2: f64, r: f64) -> PathResult {
        if !finite(&[x1, y1, x2, y2, r]) {
            return Ok(());
        }
        if r < 0.0 {
            return Err(());
        }
        let p1d = m.apply((x1, y1));
        if self.cur.is_none() {
            self.move_to_dev(p1d);
        }
        let cur = self.cur.unwrap_or(p1d);
        // The last point in user space.
        let Some(inv) = m.invert() else {
            self.line_to_dev(p1d);
            return Ok(());
        };
        let p0 = inv.apply(cur);
        let (x0, y0) = p0;
        if (x0 == x1 && y0 == y1) || (x1 == x2 && y1 == y2) || r == 0.0 {
            self.line_to_dev(p1d);
            return Ok(());
        }
        let (v1x, v1y) = (x0 - x1, y0 - y1);
        let (v2x, v2y) = (x2 - x1, y2 - y1);
        let cross = v1x * v2y - v1y * v2x;
        if cross.abs() < 1e-12 * (v1x.hypot(v1y) * v2x.hypot(v2y)).max(1e-300) {
            self.line_to_dev(p1d);
            return Ok(());
        }
        let l1 = v1x.hypot(v1y);
        let l2 = v2x.hypot(v2y);
        let (u1x, u1y) = (v1x / l1, v1y / l1);
        let (u2x, u2y) = (v2x / l2, v2y / l2);
        let cos_t = (u1x * u2x + u1y * u2y).clamp(-1.0, 1.0);
        let theta = cos_t.acos();
        let dist = r / (theta / 2.0).tan();
        let t1 = (x1 + u1x * dist, y1 + u1y * dist);
        let t2 = (x1 + u2x * dist, y1 + u2y * dist);
        // The centre lies along the bisector.
        let (bx, by) = (u1x + u2x, u1y + u2y);
        let bl = bx.hypot(by);
        let cd = r / (theta / 2.0).sin();
        let (cx, cy) = (x1 + bx / bl * cd, y1 + by / bl * cd);
        let a0 = (t1.1 - cy).atan2(t1.0 - cx);
        let a1 = (t2.1 - cy).atan2(t2.0 - cx);
        // Sweep direction: the arc turns the same way the corner does.
        let ccw = cross > 0.0;
        self.line_to_dev(m.apply(t1));
        self.ellipse_segments(m, cx, cy, r, r, 0.0, a0, a1, ccw);
        Ok(())
    }

    pub fn arc(&mut self, m: &Matrix, x: f64, y: f64, r: f64, start: f64, end: f64, ccw: bool) -> PathResult {
        self.ellipse(m, x, y, r, r, 0.0, start, end, ccw)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn ellipse(&mut self, m: &Matrix, x: f64, y: f64, rx: f64, ry: f64, rot: f64, start: f64, end: f64, ccw: bool) -> PathResult {
        if !finite(&[x, y, rx, ry, rot, start, end]) {
            return Ok(());
        }
        if rx < 0.0 || ry < 0.0 {
            return Err(());
        }
        let (s, e) = canonical_angles(start, end, ccw);
        let first = ellipse_point(x, y, rx, ry, rot, s);
        let fd = m.apply(first);
        if self.cur.is_some() {
            self.line_to_dev(fd);
        } else {
            self.move_to_dev(fd);
        }
        self.ellipse_segments(m, x, y, rx, ry, rot, s, e, ccw);
        Ok(())
    }

    /// Cubic segments of an elliptical arc from angle `s` to `e` (already canonical; `ccw` only for arcTo's
    /// raw angles, which are normalised here).
    #[allow(clippy::too_many_arguments)]
    fn ellipse_segments(&mut self, m: &Matrix, x: f64, y: f64, rx: f64, ry: f64, rot: f64, s: f64, e: f64, ccw: bool) {
        let mut sweep = e - s;
        // arcTo hands raw atan2 angles: take the short way in the requested direction.
        if ccw && sweep > 0.0 {
            sweep -= 2.0 * PI;
        } else if !ccw && sweep < 0.0 {
            sweep += 2.0 * PI;
        }
        if sweep == 0.0 {
            return;
        }
        let n = ((sweep.abs() / (PI / 2.0)).ceil() as usize).clamp(1, 64);
        let step = sweep / n as f64;
        let k = 4.0 / 3.0 * (step / 4.0).tan();
        let (sr, cr) = rot.sin_cos();
        let map = |ux: f64, uy: f64| -> P {
            // unit-circle point → ellipse → rotated → translated
            let ex = ux * rx;
            let ey = uy * ry;
            (x + ex * cr - ey * sr, y + ex * sr + ey * cr)
        };
        let mut a = s;
        for _ in 0..n {
            let b = a + step;
            let (sa, ca) = a.sin_cos();
            let (sb, cb) = b.sin_cos();
            let c1 = map(ca - k * sa, sa + k * ca);
            let c2 = map(cb + k * sb, sb - k * cb);
            let p = map(cb, sb);
            self.cubic_dev(m.apply(c1), m.apply(c2), m.apply(p));
            a = b;
        }
    }

    pub fn rect(&mut self, m: &Matrix, x: f64, y: f64, w: f64, h: f64) {
        if !finite(&[x, y, w, h]) {
            return;
        }
        self.move_to_dev(m.apply((x, y)));
        self.segs.push(Seg::Line(m.apply((x + w, y))));
        self.segs.push(Seg::Line(m.apply((x + w, y + h))));
        self.segs.push(Seg::Line(m.apply((x, y + h))));
        self.cur = Some(m.apply((x, y + h)));
        self.close();
    }

    /// HTML `roundRect(x, y, w, h, radii)`, the radii already normalised to four corners
    /// (upper-left, upper-right, lower-right, lower-left), each `(rx, ry)`.
    pub fn round_rect(&mut self, m: &Matrix, x: f64, y: f64, w: f64, h: f64, radii: [(f64, f64); 4]) {
        if !finite(&[x, y, w, h]) {
            return;
        }
        let [mut ul, mut ur, mut lr, mut ll] = radii;
        let (mut x, mut y, mut w, mut h) = (x, y, w, h);
        if w < 0.0 {
            std::mem::swap(&mut ul, &mut ur);
            std::mem::swap(&mut ll, &mut lr);
            x += w;
            w = -w;
        }
        if h < 0.0 {
            std::mem::swap(&mut ul, &mut ll);
            std::mem::swap(&mut ur, &mut lr);
            y += h;
            h = -h;
        }
        let top = ul.0 + ur.0;
        let right = ur.1 + lr.1;
        let bottom = lr.0 + ll.0;
        let left = ul.1 + ll.1;
        let mut scale: f64 = 1.0;
        for (sum, side) in [(top, w), (right, h), (bottom, w), (left, h)] {
            if sum > 0.0 {
                scale = scale.min(side / sum);
            }
        }
        if scale < 1.0 {
            for r in [&mut ul, &mut ur, &mut lr, &mut ll] {
                r.0 *= scale;
                r.1 *= scale;
            }
        }
        self.move_to_dev(m.apply((x + ul.0, y)));
        self.line_to_dev(m.apply((x + w - ur.0, y)));
        if ur.0 > 0.0 && ur.1 > 0.0 {
            self.ellipse_segments(m, x + w - ur.0, y + ur.1, ur.0, ur.1, 0.0, -PI / 2.0, 0.0, false);
        }
        self.line_to_dev(m.apply((x + w, y + h - lr.1)));
        if lr.0 > 0.0 && lr.1 > 0.0 {
            self.ellipse_segments(m, x + w - lr.0, y + h - lr.1, lr.0, lr.1, 0.0, 0.0, PI / 2.0, false);
        }
        self.line_to_dev(m.apply((x + ll.0, y + h)));
        if ll.0 > 0.0 && ll.1 > 0.0 {
            self.ellipse_segments(m, x + ll.0, y + h - ll.1, ll.0, ll.1, 0.0, PI / 2.0, PI, false);
        }
        self.line_to_dev(m.apply((x, y + ul.1)));
        if ul.0 > 0.0 && ul.1 > 0.0 {
            self.ellipse_segments(m, x + ul.0, y + ul.1, ul.0, ul.1, 0.0, PI, 1.5 * PI, false);
        }
        self.close();
        self.move_to_dev(m.apply((x, y)));
    }

    /// Append `other` mapped through `m` (Path2D `addPath`, and using a Path2D under the CTM).
    pub fn add_path(&mut self, other: &Path, m: &Matrix) {
        for s in &other.segs {
            match *s {
                Seg::Move(p) => self.move_to_dev(m.apply(p)),
                Seg::Line(p) => self.line_to_dev(m.apply(p)),
                Seg::Quad(c, p) => self.quad_dev(m.apply(c), m.apply(p)),
                Seg::Cubic(c1, c2, p) => self.cubic_dev(m.apply(c1), m.apply(c2), m.apply(p)),
                Seg::Close => self.close(),
            }
        }
    }

    pub fn transformed(&self, m: &Matrix) -> Path {
        let mut p = Path::new();
        p.add_path(self, m);
        p
    }

    /// Polylines, one per subpath: (points, closed). `tol` is the flattening tolerance in this path's units.
    pub fn flatten(&self, tol: f64) -> Vec<(Vec<P>, bool)> {
        let mut out: Vec<(Vec<P>, bool)> = Vec::new();
        let mut cur: Vec<P> = Vec::new();
        let mut last: P = (0.0, 0.0);
        let flush = |cur: &mut Vec<P>, out: &mut Vec<(Vec<P>, bool)>, closed: bool| {
            if !cur.is_empty() {
                out.push((std::mem::take(cur), closed));
            }
        };
        for s in &self.segs {
            match *s {
                Seg::Move(p) => {
                    flush(&mut cur, &mut out, false);
                    cur.push(p);
                    last = p;
                }
                Seg::Line(p) => {
                    if cur.is_empty() {
                        cur.push(last);
                    }
                    cur.push(p);
                    last = p;
                }
                Seg::Quad(c, p) => {
                    if cur.is_empty() {
                        cur.push(last);
                    }
                    let dd = ((last.0 - 2.0 * c.0 + p.0).powi(2) + (last.1 - 2.0 * c.1 + p.1).powi(2)).sqrt() * 0.25;
                    let n = ((dd / tol).sqrt().ceil() as usize).clamp(1, 500);
                    for k in 1..=n {
                        let t = k as f64 / n as f64;
                        let mt = 1.0 - t;
                        cur.push((
                            mt * mt * last.0 + 2.0 * mt * t * c.0 + t * t * p.0,
                            mt * mt * last.1 + 2.0 * mt * t * c.1 + t * t * p.1,
                        ));
                    }
                    last = p;
                }
                Seg::Cubic(c1, c2, p) => {
                    if cur.is_empty() {
                        cur.push(last);
                    }
                    let d1 = (last.0 - 2.0 * c1.0 + c2.0).powi(2) + (last.1 - 2.0 * c1.1 + c2.1).powi(2);
                    let d2 = (c1.0 - 2.0 * c2.0 + p.0).powi(2) + (c1.1 - 2.0 * c2.1 + p.1).powi(2);
                    let dd = d1.max(d2).sqrt() * 0.75;
                    let n = ((dd / tol).sqrt().ceil() as usize).clamp(1, 500);
                    for k in 1..=n {
                        let t = k as f64 / n as f64;
                        let mt = 1.0 - t;
                        let (a, b, cc, d) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
                        cur.push((
                            a * last.0 + b * c1.0 + cc * c2.0 + d * p.0,
                            a * last.1 + b * c1.1 + cc * c2.1 + d * p.1,
                        ));
                    }
                    last = p;
                }
                Seg::Close => {
                    let start = cur.first().copied().unwrap_or(last);
                    flush(&mut cur, &mut out, true);
                    last = start;
                }
            }
        }
        flush(&mut cur, &mut out, false);
        out
    }
}

/// HTML's arc angle rules (as Chromium canonicalises them): a sweep of 2π or more in the drawing direction is
/// a full ellipse; otherwise the end angle is moved into the half-open turn after (or before, ccw) the start.
pub fn canonical_angles(start: f64, end: f64, ccw: bool) -> (f64, f64) {
    let tau = 2.0 * PI;
    let mut s = start % tau;
    if s < 0.0 {
        s += tau;
    }
    let delta = s - start;
    let mut e = end + delta;
    if !ccw && e - s >= tau {
        e = s + tau;
    } else if ccw && s - e >= tau {
        e = s - tau;
    } else if !ccw && s > e {
        e = s + (tau - (s - e) % tau);
    } else if ccw && s < e {
        e = s - (tau - (e - s) % tau);
    }
    (s, e)
}

pub fn ellipse_point(x: f64, y: f64, rx: f64, ry: f64, rot: f64, a: f64) -> P {
    let (sa, ca) = a.sin_cos();
    let (sr, cr) = rot.sin_cos();
    let ex = rx * ca;
    let ey = ry * sa;
    (x + ex * cr - ey * sr, y + ex * sr + ey * cr)
}

// ------------------------------------------------------------------------------------------ SVG path data

/// Path2D's string constructor: SVG 2 path data (§9.3.9 grammar). Parsing stops at the first error, keeping
/// the segments before it, as SVG's error handling requires.
pub fn parse_svg_path(d: &str) -> Path {
    let mut p = Path::new();
    let id = Matrix::IDENTITY;
    let b = d.as_bytes();
    let mut i = 0usize;
    let skip_ws = |i: &mut usize| {
        while *i < b.len() && matches!(b[*i], b' ' | b'\t' | b'\n' | b'\r' | b'\x0c') {
            *i += 1;
        }
    };
    let skip_sep = |i: &mut usize| {
        skip_ws(i);
        if *i < b.len() && b[*i] == b',' {
            *i += 1;
            skip_ws(i);
        }
    };
    let num = |i: &mut usize| -> Option<f64> {
        skip_sep(i);
        let st = *i;
        if *i < b.len() && (b[*i] == b'+' || b[*i] == b'-') {
            *i += 1;
        }
        let mut digits = false;
        while *i < b.len() && b[*i].is_ascii_digit() {
            *i += 1;
            digits = true;
        }
        if *i < b.len() && b[*i] == b'.' {
            *i += 1;
            while *i < b.len() && b[*i].is_ascii_digit() {
                *i += 1;
                digits = true;
            }
        }
        if !digits {
            *i = st;
            return None;
        }
        if *i < b.len() && (b[*i] == b'e' || b[*i] == b'E') {
            let save = *i;
            *i += 1;
            if *i < b.len() && (b[*i] == b'+' || b[*i] == b'-') {
                *i += 1;
            }
            let es = *i;
            while *i < b.len() && b[*i].is_ascii_digit() {
                *i += 1;
            }
            if es == *i {
                *i = save;
            }
        }
        d[st..*i].parse::<f64>().ok()
    };
    let flag = |i: &mut usize| -> Option<bool> {
        skip_sep(i);
        match b.get(*i) {
            Some(b'0') => {
                *i += 1;
                Some(false)
            }
            Some(b'1') => {
                *i += 1;
                Some(true)
            }
            _ => None,
        }
    };
    let mut cur: P = (0.0, 0.0);
    let mut start: P = (0.0, 0.0);
    let mut last_ctrl: Option<(u8, P)> = None;
    let mut cmd: u8 = 0;
    skip_ws(&mut i);
    loop {
        skip_ws(&mut i);
        if i >= b.len() {
            break;
        }
        let c = b[i];
        if c.is_ascii_alphabetic() {
            cmd = c;
            i += 1;
        } else if cmd == 0 || matches!(cmd, b'Z' | b'z') {
            break;
        }
        let rel = cmd.is_ascii_lowercase();
        let base = if rel { cur } else { (0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let (Some(x), Some(y)) = (num(&mut i), num(&mut i)) else { break };
                cur = (base.0 + x, base.1 + y);
                start = cur;
                p.move_to(&id, cur.0, cur.1);
                // Subsequent pairs are implicit lineto.
                cmd = if rel { b'l' } else { b'L' };
                last_ctrl = None;
            }
            b'L' => {
                let (Some(x), Some(y)) = (num(&mut i), num(&mut i)) else { break };
                cur = (base.0 + x, base.1 + y);
                p.line_to(&id, cur.0, cur.1);
                last_ctrl = None;
            }
            b'H' => {
                let Some(x) = num(&mut i) else { break };
                cur = (if rel { cur.0 + x } else { x }, cur.1);
                p.line_to(&id, cur.0, cur.1);
                last_ctrl = None;
            }
            b'V' => {
                let Some(y) = num(&mut i) else { break };
                cur = (cur.0, if rel { cur.1 + y } else { y });
                p.line_to(&id, cur.0, cur.1);
                last_ctrl = None;
            }
            b'C' => {
                let v: Vec<f64> = (0..6).map_while(|_| num(&mut i)).collect();
                if v.len() < 6 {
                    break;
                }
                let c1 = (base.0 + v[0], base.1 + v[1]);
                let c2 = (base.0 + v[2], base.1 + v[3]);
                cur = (base.0 + v[4], base.1 + v[5]);
                p.bezier_curve_to(&id, c1.0, c1.1, c2.0, c2.1, cur.0, cur.1);
                last_ctrl = Some((b'C', c2));
            }
            b'S' => {
                let v: Vec<f64> = (0..4).map_while(|_| num(&mut i)).collect();
                if v.len() < 4 {
                    break;
                }
                let c1 = match last_ctrl {
                    Some((b'C', c)) => (2.0 * cur.0 - c.0, 2.0 * cur.1 - c.1),
                    _ => cur,
                };
                let c2 = (base.0 + v[0], base.1 + v[1]);
                cur = (base.0 + v[2], base.1 + v[3]);
                p.bezier_curve_to(&id, c1.0, c1.1, c2.0, c2.1, cur.0, cur.1);
                last_ctrl = Some((b'C', c2));
            }
            b'Q' => {
                let v: Vec<f64> = (0..4).map_while(|_| num(&mut i)).collect();
                if v.len() < 4 {
                    break;
                }
                let c = (base.0 + v[0], base.1 + v[1]);
                cur = (base.0 + v[2], base.1 + v[3]);
                p.quadratic_curve_to(&id, c.0, c.1, cur.0, cur.1);
                last_ctrl = Some((b'Q', c));
            }
            b'T' => {
                let v: Vec<f64> = (0..2).map_while(|_| num(&mut i)).collect();
                if v.len() < 2 {
                    break;
                }
                let c = match last_ctrl {
                    Some((b'Q', c)) => (2.0 * cur.0 - c.0, 2.0 * cur.1 - c.1),
                    _ => cur,
                };
                cur = (base.0 + v[0], base.1 + v[1]);
                p.quadratic_curve_to(&id, c.0, c.1, cur.0, cur.1);
                last_ctrl = Some((b'Q', c));
            }
            b'A' => {
                let (Some(rx), Some(ry), Some(rot)) = (num(&mut i), num(&mut i), num(&mut i)) else { break };
                let (Some(large), Some(sweep)) = (flag(&mut i), flag(&mut i)) else { break };
                let (Some(x), Some(y)) = (num(&mut i), num(&mut i)) else { break };
                let end = (base.0 + x, base.1 + y);
                svg_arc(&mut p, cur, rx, ry, rot, large, sweep, end);
                cur = end;
                last_ctrl = None;
            }
            b'Z' => {
                p.close_path();
                cur = start;
                last_ctrl = None;
            }
            _ => break,
        }
    }
    p
}

/// SVG's endpoint-parameterised elliptical arc (SVG 2 Appendix B.2.4 conversion to centre form).
#[allow(clippy::too_many_arguments)]
fn svg_arc(p: &mut Path, from: P, rx: f64, ry: f64, rot_deg: f64, large: bool, sweep: bool, to: P) {
    let id = Matrix::IDENTITY;
    if from == to {
        return;
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx == 0.0 || ry == 0.0 {
        p.line_to(&id, to.0, to.1);
        return;
    }
    let phi = rot_deg.to_radians();
    let (sp, cp) = phi.sin_cos();
    let dx = (from.0 - to.0) / 2.0;
    let dy = (from.1 - to.1) / 2.0;
    let x1 = cp * dx + sp * dy;
    let y1 = -sp * dx + cp * dy;
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut co = (num / den).max(0.0).sqrt();
    if large == sweep {
        co = -co;
    }
    let cx1 = co * rx * y1 / ry;
    let cy1 = -co * ry * x1 / rx;
    let cx = cp * cx1 - sp * cy1 + (from.0 + to.0) / 2.0;
    let cy = sp * cx1 + cp * cy1 + (from.1 + to.1) / 2.0;
    let ang = |ux: f64, uy: f64, vx: f64, vy: f64| -> f64 {
        (ux * vy - uy * vx).atan2(ux * vx + uy * vy)
    };
    let t1 = ang(1.0, 0.0, (x1 - cx1) / rx, (y1 - cy1) / ry);
    let mut dt = ang((x1 - cx1) / rx, (y1 - cy1) / ry, (-x1 - cx1) / rx, (-y1 - cy1) / ry);
    if !sweep && dt > 0.0 {
        dt -= 2.0 * PI;
    } else if sweep && dt < 0.0 {
        dt += 2.0 * PI;
    }
    p.ellipse_segments(&id, cx, cy, rx, ry, phi, t1, t1 + dt, !sweep);
    // Land exactly on the endpoint.
    if let Some(Seg::Cubic(c1, c2, _)) = p.segs.last().copied() {
        let n = p.segs.len();
        p.segs[n - 1] = Seg::Cubic(c1, c2, to);
        p.cur = Some(to);
    }
}

// ------------------------------------------------------------------------------------------ hit testing

/// Winding number of `pt` against closed polylines (every subpath implicitly closed, as filling does).
pub fn winding(polys: &[(Vec<P>, bool)], pt: P) -> (i32, u32) {
    let mut wn = 0i32;
    let mut crossings = 0u32;
    for (pts, _) in polys {
        let n = pts.len();
        if n < 2 {
            continue;
        }
        for k in 0..n {
            let a = pts[k];
            let b = pts[(k + 1) % n];
            if (a.1 <= pt.1) != (b.1 <= pt.1) {
                let t = (pt.1 - a.1) / (b.1 - a.1);
                let x = a.0 + t * (b.0 - a.0);
                if x > pt.0 {
                    crossings += 1;
                    wn += if b.1 > a.1 { 1 } else { -1 };
                }
            }
        }
    }
    (wn, crossings)
}

/// Whether `pt` lies on a polygon edge (the canvas counts points on the path's outline as inside).
pub fn on_edge(polys: &[(Vec<P>, bool)], pt: P) -> bool {
    for (pts, _) in polys {
        let n = pts.len();
        if n < 2 {
            continue;
        }
        for k in 0..n {
            let a = pts[k];
            let b = pts[(k + 1) % n];
            let (ex, ey) = (b.0 - a.0, b.1 - a.1);
            let len2 = ex * ex + ey * ey;
            if len2 == 0.0 {
                if a == pt {
                    return true;
                }
                continue;
            }
            let t = ((pt.0 - a.0) * ex + (pt.1 - a.1) * ey) / len2;
            if !(0.0..=1.0).contains(&t) {
                continue;
            }
            let (qx, qy) = (a.0 + t * ex - pt.0, a.1 + t * ey - pt.1);
            if qx * qx + qy * qy < 1e-12 {
                return true;
            }
        }
    }
    false
}
