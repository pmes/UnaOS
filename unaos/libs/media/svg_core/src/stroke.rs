//! Stroking by outline offsetting (SVG 2 §13.5 "Stroke shape"): curves are flattened to a user-space tolerance
//! derived from the device transform, the polyline is dashed (§13.5.3 `stroke-dasharray`/`stroke-dashoffset`),
//! and each piece becomes a polygon — the left offset, the end cap, the right offset back, the start cap; or for
//! a closed subpath two loops — with joins (`miter` within `stroke-miterlimit` else `bevel`, `round`, `bevel`)
//! on the outer side of each corner and a line through the vertex on the inner side (the Skia/SkStroke
//! construction, correct under the nonzero rule). Caps: `butt`, `round`, `square`; zero-length subpaths get
//! round/square caps (SVG 2 §13.5.5). The result is filled nonzero in device space.

use crate::fmath::{acos, ceil, hypot, sin_cos, sqrt};
use crate::geom::{P, Path, Seg, Transform};
use alloc::vec::Vec;
use core::f64::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cap {
    Butt,
    Round,
    Square,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Join {
    Miter,
    MiterClip,
    Round,
    Bevel,
    Arcs,
}

#[derive(Clone, Debug, PartialEq)]
pub struct StrokeStyle {
    pub width: f64,
    pub cap: Cap,
    pub join: Join,
    pub miter_limit: f64,
    /// Dash lengths (already validated: even count, non-negative, positive sum) and the offset.
    pub dash: Option<(Vec<f64>, f64)>,
}

struct Poly {
    pts: Vec<P>,
    corner: Vec<bool>,
    closed: bool,
}

fn flatten_for_stroke(path: &Path, tol: f64) -> Vec<Poly> {
    let tol = tol.max(1e-7);
    let mut out = Vec::new();
    let mut cur = Poly { pts: Vec::new(), corner: Vec::new(), closed: false };
    let mut start = (0.0, 0.0);
    let mut last = (0.0, 0.0);
    let push_poly = |cur: &mut Poly, out: &mut Vec<Poly>, closed: bool| {
        if !cur.pts.is_empty() {
            cur.closed = closed;
            out.push(core::mem::replace(cur, Poly { pts: Vec::new(), corner: Vec::new(), closed: false }));
        }
    };
    for s in &path.segs {
        match *s {
            Seg::Move(p) => {
                push_poly(&mut cur, &mut out, false);
                cur.pts.push(p);
                cur.corner.push(true);
                start = p;
                last = p;
            }
            Seg::Line(p) => {
                if cur.pts.is_empty() {
                    cur.pts.push(last);
                    cur.corner.push(true);
                }
                cur.pts.push(p);
                cur.corner.push(true);
                last = p;
            }
            Seg::Quad(a, p) => {
                if cur.pts.is_empty() {
                    cur.pts.push(last);
                    cur.corner.push(true);
                }
                let dd = hypot(last.0 - 2.0 * a.0 + p.0, last.1 - 2.0 * a.1 + p.1);
                let n = (ceil(sqrt(dd / (4.0 * tol))) as usize).clamp(1, 1000);
                for k in 1..=n {
                    let t = k as f64 / n as f64;
                    let mt = 1.0 - t;
                    cur.pts.push((mt * mt * last.0 + 2.0 * mt * t * a.0 + t * t * p.0, mt * mt * last.1 + 2.0 * mt * t * a.1 + t * t * p.1));
                    cur.corner.push(k == n);
                }
                last = p;
            }
            Seg::Cubic(c1, c2, p) => {
                if cur.pts.is_empty() {
                    cur.pts.push(last);
                    cur.corner.push(true);
                }
                let d1 = hypot(last.0 - 2.0 * c1.0 + c2.0, last.1 - 2.0 * c1.1 + c2.1);
                let d2 = hypot(c1.0 - 2.0 * c2.0 + p.0, c1.1 - 2.0 * c2.1 + p.1);
                let n = (ceil(sqrt(3.0 * d1.max(d2) / (4.0 * tol))) as usize).clamp(1, 1000);
                for k in 1..=n {
                    let t = k as f64 / n as f64;
                    let mt = 1.0 - t;
                    let (a, b, c, d) = (mt * mt * mt, 3.0 * mt * mt * t, 3.0 * mt * t * t, t * t * t);
                    cur.pts.push((a * last.0 + b * c1.0 + c * c2.0 + d * p.0, a * last.1 + b * c1.1 + c * c2.1 + d * p.1));
                    cur.corner.push(k == n);
                }
                last = p;
            }
            Seg::Close => {
                if !cur.pts.is_empty() {
                    push_poly(&mut cur, &mut out, true);
                }
                last = start;
            }
        }
    }
    push_poly(&mut cur, &mut out, false);
    out
}

/// Drop consecutive duplicate points (keeping the corner flag of either), and the closing duplicate.
fn dedup(p: &mut Poly, eps: f64) {
    let mut pts: Vec<P> = Vec::with_capacity(p.pts.len());
    let mut cor: Vec<bool> = Vec::with_capacity(p.pts.len());
    for (i, &q) in p.pts.iter().enumerate() {
        if let Some(&l) = pts.last() {
            if hypot(q.0 - l.0, q.1 - l.1) <= eps {
                let c = cor.last_mut().unwrap();
                *c = *c || p.corner[i];
                continue;
            }
        }
        pts.push(q);
        cor.push(p.corner[i]);
    }
    if p.closed && pts.len() > 1 {
        let (f, l) = (pts[0], *pts.last().unwrap());
        if hypot(f.0 - l.0, f.1 - l.1) <= eps {
            pts.pop();
            cor.pop();
            cor[0] = true;
        }
    }
    p.pts = pts;
    p.corner = cor;
}

fn dash_polys(polys: Vec<Poly>, dashes: &[f64], offset: f64) -> Vec<Poly> {
    let total: f64 = dashes.iter().sum();
    let mut out = Vec::new();
    for p in polys {
        let mut pts = p.pts.clone();
        let mut cor = p.corner.clone();
        if p.closed && !pts.is_empty() {
            pts.push(pts[0]);
            cor.push(true);
        }
        if pts.len() < 2 {
            continue;
        }
        // Phase at the start of each subpath: offset into the pattern.
        let mut phase = offset % total;
        if phase < 0.0 {
            phase += total;
        }
        let mut idx = 0;
        while phase >= dashes[idx] {
            phase -= dashes[idx];
            idx = (idx + 1) % dashes.len();
            if dashes.iter().all(|&d| d == 0.0) {
                break;
            }
        }
        let mut remain = dashes[idx] - phase; // length left in the current dash/gap
        let mut on = idx % 2 == 0;
        let mut cur = Poly { pts: Vec::new(), corner: Vec::new(), closed: false };
        if on {
            cur.pts.push(pts[0]);
            cur.corner.push(false);
        }
        for k in 0..pts.len() - 1 {
            let (a, b) = (pts[k], pts[k + 1]);
            let seg = hypot(b.0 - a.0, b.1 - a.1);
            let mut t0 = 0.0;
            while seg - t0 > remain {
                t0 += remain;
                let f = t0 / seg;
                let q = (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f);
                if on {
                    cur.pts.push(q);
                    cur.corner.push(false);
                    out.push(core::mem::replace(&mut cur, Poly { pts: Vec::new(), corner: Vec::new(), closed: false }));
                } else {
                    cur.pts.push(q);
                    cur.corner.push(false);
                }
                on = !on;
                idx = (idx + 1) % dashes.len();
                remain = dashes[idx];
                if remain == 0.0 && on {
                    // zero-length dash: emits a dot (caps only)
                }
            }
            remain -= seg - t0;
            if on {
                cur.pts.push(b);
                cur.corner.push(cor[k + 1]);
            }
        }
        if on && !cur.pts.is_empty() {
            out.push(cur);
        }
    }
    out
}

fn arc_points(out: &mut Vec<P>, c: P, r: f64, a0: f64, a1: f64, tol: f64) {
    // Points strictly between angles a0 → a1 (signed sweep), excluding both ends.
    let sweep = a1 - a0;
    let tol = tol * 0.25; // round joins/caps: a finer chord than curves, so discs keep their area
    let step_max = if r > tol { 2.0 * acos((1.0 - tol / r).clamp(-1.0, 1.0)) } else { PI / 2.0 };
    let n = (ceil(sweep.abs() / step_max.max(1e-3)) as usize).clamp(1, 1000);
    for k in 1..n {
        let a = a0 + sweep * k as f64 / n as f64;
        let (s, co) = sin_cos(a);
        out.push((c.0 + r * co, c.1 + r * s));
    }
}

fn angle(v: P) -> f64 {
    crate::fmath::atan2(v.1, v.0)
}

/// Add the join at vertex `p` between incoming normal `n0` and outgoing normal `n1` (unit, left-hand) to the
/// offset side with sign `side` (+1 left, −1 right).
#[allow(clippy::too_many_arguments)]
fn join(out: &mut Vec<P>, p: P, n0: P, n1: P, d0: P, d1: P, hw: f64, side: f64, st: &StrokeStyle, corner: bool, tol: f64) {
    let cross = d0.0 * d1.1 - d0.1 * d1.0;
    let dot = d0.0 * d1.0 + d0.1 * d1.1;
    let a = (p.0 + side * n0.0 * hw, p.1 + side * n0.1 * hw);
    let b = (p.0 + side * n1.0 * hw, p.1 + side * n1.1 * hw);
    if cross.abs() < 1e-12 && dot > 0.0 {
        out.push(a);
        return;
    }
    // Turning toward the left normal (cross > 0) makes the left side the inner side.
    let inner = (cross > 0.0) == (side > 0.0);
    if inner {
        out.push(a);
        out.push(p);
        out.push(b);
        return;
    }
    let jt = if corner { st.join } else { Join::Round };
    match jt {
        Join::Bevel => {
            out.push(a);
            out.push(b);
        }
        Join::Round => {
            out.push(a);
            let a0 = angle((side * n0.0, side * n0.1));
            let mut a1 = angle((side * n1.0, side * n1.1));
            // Sweep the short way around (outer side).
            while a1 - a0 > PI {
                a1 -= 2.0 * PI;
            }
            while a1 - a0 < -PI {
                a1 += 2.0 * PI;
            }
            arc_points(out, p, hw, a0, a1, tol);
            out.push(b);
        }
        Join::Miter | Join::MiterClip | Join::Arcs => {
            let cosphi = (n0.0 * n1.0 + n0.1 * n1.1).clamp(-1.0, 1.0);
            let ratio = sqrt(2.0 / (1.0 + cosphi).max(1e-12));
            if ratio <= st.miter_limit {
                let k = hw / (1.0 + cosphi);
                out.push((p.0 + side * (n0.0 + n1.0) * k, p.1 + side * (n0.1 + n1.1) * k));
            } else if jt == Join::MiterClip {
                // SVG 2 miter-clip: clip the miter at miterlimit*hw from the vertex.
                let bis = ((n0.0 + n1.0), (n0.1 + n1.1));
                let bl = hypot(bis.0, bis.1).max(1e-12);
                let bis = (side * bis.0 / bl, side * bis.1 / bl);
                let lim = st.miter_limit * hw;
                // Points on the two offset lines at distance lim along the bisector.
                let half = (1.0 - cosphi * 0.0).max(0.0);
                let _ = half;
                let t0 = (d0.0, d0.1);
                let t1 = (-d1.0, -d1.1);
                let proj = |n: P, t: P| {
                    // Offset line: a + s t ; find s where (q - p)·bis = lim.
                    let base = (p.0 + side * n.0 * hw, p.1 + side * n.1 * hw);
                    let bd = (base.0 - p.0) * bis.0 + (base.1 - p.1) * bis.1;
                    let td = t.0 * bis.0 + t.1 * bis.1;
                    let s = if td.abs() < 1e-12 { 0.0 } else { (lim - bd) / td };
                    (base.0 + s * t.0, base.1 + s * t.1)
                };
                out.push(a);
                out.push(proj(n0, t0));
                out.push(proj(n1, t1));
                out.push(b);
            } else {
                out.push(a);
                out.push(b);
            }
        }
    }
}

fn cap(out: &mut Vec<P>, p: P, d: P, n: P, hw: f64, c: Cap, tol: f64) {
    // From the left offset (p + n hw) around the end (direction d) to the right offset (p − n hw).
    match c {
        Cap::Butt => {
            out.push((p.0 + n.0 * hw, p.1 + n.1 * hw));
            out.push((p.0 - n.0 * hw, p.1 - n.1 * hw));
        }
        Cap::Square => {
            out.push((p.0 + n.0 * hw, p.1 + n.1 * hw));
            out.push((p.0 + (n.0 + d.0) * hw, p.1 + (n.1 + d.1) * hw));
            out.push((p.0 + (d.0 - n.0) * hw, p.1 + (d.1 - n.1) * hw));
            out.push((p.0 - n.0 * hw, p.1 - n.1 * hw));
        }
        Cap::Round => {
            out.push((p.0 + n.0 * hw, p.1 + n.1 * hw));
            let a0 = angle(n);
            // Sweep through d: from n to −n passing d.
            let ad = angle(d);
            let mut a1 = a0 + PI;
            let mid = a0 + PI / 2.0;
            let diff = |x: f64, y: f64| {
                let mut t = x - y;
                while t > PI {
                    t -= 2.0 * PI;
                }
                while t < -PI {
                    t += 2.0 * PI;
                }
                t.abs()
            };
            if diff(mid, ad) > PI / 2.0 {
                a1 = a0 - PI;
            }
            arc_points(out, p, hw, a0, a1, tol);
            out.push((p.0 - n.0 * hw, p.1 - n.1 * hw));
        }
    }
}

fn dir(a: P, b: P) -> P {
    let l = hypot(b.0 - a.0, b.1 - a.1);
    ((b.0 - a.0) / l, (b.1 - a.1) / l)
}

fn left(d: P) -> P {
    (-d.1, d.0)
}

/// The stroke outline of `path` (user space) as device-space polygons for nonzero filling.
pub fn stroke_polys(path: &Path, st: &StrokeStyle, ts: &Transform) -> Vec<(Vec<P>, bool)> {
    let scale = ts.max_scale().max(1e-9);
    let tol = crate::raster::FLATTEN_TOL / scale;
    let hw = st.width / 2.0;
    if !(hw > 0.0) || !hw.is_finite() {
        return Vec::new();
    }
    let mut polys = flatten_for_stroke(path, tol);
    let eps = 1e-9 * (1.0 + hw);
    if let Some((d, off)) = &st.dash {
        for p in polys.iter_mut() {
            dedup(p, eps);
        }
        polys = dash_polys(polys, d, *off);
    }
    let mut out: Vec<(Vec<P>, bool)> = Vec::new();
    for mut p in polys {
        // A lone moveto draws nothing; `M x y Z` and `M x y L x y` are zero-length subpaths (they get caps).
        if p.pts.len() == 1 && !p.closed {
            continue;
        }
        dedup(&mut p, eps);
        let n = p.pts.len();
        if n == 0 {
            continue;
        }
        if n == 1 {
            // Zero-length subpath: a dot for round/square caps.
            let c = p.pts[0];
            let mut poly = Vec::new();
            match st.cap {
                Cap::Butt => continue,
                Cap::Round => {
                    poly.push((c.0 + hw, c.1));
                    arc_points(&mut poly, c, hw, 0.0, 2.0 * PI, tol);
                }
                Cap::Square => {
                    poly.extend_from_slice(&[(c.0 - hw, c.1 - hw), (c.0 + hw, c.1 - hw), (c.0 + hw, c.1 + hw), (c.0 - hw, c.1 + hw)]);
                }
            }
            out.push((poly, true));
            continue;
        }
        let pts = &p.pts;
        let segs = if p.closed { n } else { n - 1 };
        let ds: Vec<P> = (0..segs).map(|i| dir(pts[i], pts[(i + 1) % n])).collect();
        let ns: Vec<P> = ds.iter().map(|&d| left(d)).collect();
        if p.closed && n >= 2 {
            let mut lft = Vec::new();
            let mut rgt = Vec::new();
            for j in 0..n {
                let pi = (j + segs - 1) % segs; // incoming segment
                join(&mut lft, pts[j], ns[pi], ns[j], ds[pi], ds[j], hw, 1.0, st, p.corner[j], tol);
                join(&mut rgt, pts[j], ns[pi], ns[j], ds[pi], ds[j], hw, -1.0, st, p.corner[j], tol);
            }
            rgt.reverse();
            out.push((lft, true));
            out.push((rgt, true));
        } else {
            let mut poly = Vec::new();
            // Left side forward.
            poly.push((pts[0].0 + ns[0].0 * hw, pts[0].1 + ns[0].1 * hw));
            for j in 1..n - 1 {
                join(&mut poly, pts[j], ns[j - 1], ns[j], ds[j - 1], ds[j], hw, 1.0, st, p.corner[j], tol);
            }
            let e = n - 1;
            let de = ds[e - 1];
            cap(&mut poly, pts[e], de, ns[e - 1], hw, st.cap, tol);
            // Right side backward: walking backward, the right side of the forward path is the left side.
            for j in (1..n - 1).rev() {
                let rd0 = (-ds[j].0, -ds[j].1);
                let rd1 = (-ds[j - 1].0, -ds[j - 1].1);
                join(&mut poly, pts[j], left(rd0), left(rd1), rd0, rd1, hw, 1.0, st, p.corner[j], tol);
            }
            let ds0 = (-ds[0].0, -ds[0].1);
            cap(&mut poly, pts[0], ds0, left(ds0), hw, st.cap, tol);
            out.push((poly, true));
        }
    }
    for (poly, _) in out.iter_mut() {
        for q in poly.iter_mut() {
            *q = ts.apply(q.0, q.1);
        }
    }
    out
}

/// Validate a `stroke-dasharray` (already in user units): `None` = draw solid.
pub fn normalize_dashes(mut d: Vec<f64>) -> Option<Vec<f64>> {
    if d.is_empty() || d.iter().any(|&v| v < 0.0 || !v.is_finite()) {
        return None;
    }
    if d.len() % 2 == 1 {
        let c = d.clone();
        d.extend(c);
    }
    if d.iter().sum::<f64>() <= 0.0 {
        return None;
    }
    Some(d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster::{fill_polys, FillRule};

    fn cov_sum(polys: &[(Vec<P>, bool)], w: usize, h: usize) -> f64 {
        fill_polys(polys, FillRule::NonZero, w, h, true).map(|c| c.data.iter().map(|&v| v as f64 / 255.0).sum()).unwrap_or(0.0)
    }

    fn st(w: f64, cap: Cap, join: Join) -> StrokeStyle {
        StrokeStyle { width: w, cap, join, miter_limit: 4.0, dash: None }
    }

    #[test]
    fn line_areas() {
        let mut p = Path::new();
        p.move_to(10.0, 10.0);
        p.line_to(30.0, 10.0);
        let a = cov_sum(&stroke_polys(&p, &st(4.0, Cap::Butt, Join::Miter), &Transform::IDENTITY), 50, 50);
        assert!((a - 80.0).abs() < 0.5, "{a}");
        let a = cov_sum(&stroke_polys(&p, &st(4.0, Cap::Square, Join::Miter), &Transform::IDENTITY), 50, 50);
        assert!((a - 96.0).abs() < 0.5, "{a}");
        let a = cov_sum(&stroke_polys(&p, &st(4.0, Cap::Round, Join::Miter), &Transform::IDENTITY), 50, 50);
        assert!((a - (80.0 + PI * 4.0)).abs() < 0.5, "{a}");
    }

    #[test]
    fn closed_square_ring_and_joins() {
        let mut p = Path::new();
        p.move_to(10.0, 10.0);
        p.line_to(30.0, 10.0);
        p.line_to(30.0, 30.0);
        p.line_to(10.0, 30.0);
        p.close();
        // Miter ring: 24² − 16² = 320.
        let a = cov_sum(&stroke_polys(&p, &st(4.0, Cap::Butt, Join::Miter), &Transform::IDENTITY), 50, 50);
        assert!((a - 320.0).abs() < 0.5, "{a}");
        // Bevel cuts 4 corner triangles of area 2 each.
        let a = cov_sum(&stroke_polys(&p, &st(4.0, Cap::Butt, Join::Bevel), &Transform::IDENTITY), 50, 50);
        assert!((a - 312.0).abs() < 0.5, "{a}");
        // Reverse winding gives the same ring.
        let mut q = Path::new();
        q.move_to(10.0, 10.0);
        q.line_to(10.0, 30.0);
        q.line_to(30.0, 30.0);
        q.line_to(30.0, 10.0);
        q.close();
        let a = cov_sum(&stroke_polys(&q, &st(4.0, Cap::Butt, Join::Miter), &Transform::IDENTITY), 50, 50);
        assert!((a - 320.0).abs() < 0.5, "{a}");
    }

    #[test]
    fn dashes_and_dots() {
        let mut p = Path::new();
        p.move_to(0.0, 10.0);
        p.line_to(40.0, 10.0);
        let s = StrokeStyle { width: 2.0, cap: Cap::Butt, join: Join::Miter, miter_limit: 4.0, dash: Some((alloc::vec![5.0, 5.0], 0.0)) };
        let polys = stroke_polys(&p, &s, &Transform::IDENTITY);
        let a = cov_sum(&polys, 50, 20);
        assert!((a - 40.0).abs() < 0.5, "{a} {polys:?}");
        let s = StrokeStyle { dash: Some((alloc::vec![5.0, 5.0], 2.5)), ..s };
        let a = cov_sum(&stroke_polys(&p, &s, &Transform::IDENTITY), 50, 20);
        assert!((a - 40.0).abs() < 0.5, "{a}");
        // Zero-length subpath + round cap = disc.
        let mut z = Path::new();
        z.move_to(10.0, 10.0);
        z.close();
        let a = cov_sum(&stroke_polys(&z, &st(6.0, Cap::Round, Join::Miter), &Transform::IDENTITY), 20, 20);
        assert!((a - PI * 9.0).abs() < 0.3, "{a}");
        assert_eq!(normalize_dashes(alloc::vec![1.0, 2.0, 3.0]).unwrap().len(), 6);
        assert!(normalize_dashes(alloc::vec![0.0, 0.0]).is_none());
        assert!(normalize_dashes(alloc::vec![1.0, -1.0]).is_none());
    }
}
