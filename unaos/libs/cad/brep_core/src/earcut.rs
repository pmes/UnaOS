//! Planar-face triangulation: ear clipping on one polygon, the holes first bridged into the outer loop at a mutually
//! visible vertex (Eberly, "Triangulation by Ear Clipping"). Every decision is geom_core's exact `orient2d`. An
//! `Uncertain` is never read as an answer: it blocks the candidate (the conservative side), and a polygon that
//! cannot then be clipped returns `Undecided`, not a guessed mesh.

use crate::dep::{Predicate, Vec2, orient2d};
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EarFail {
    /// No ear and no bridge, with every predicate decided.
    Stuck,
    /// No ear or bridge where some predicate was Uncertain.
    Undecided,
}

type Node = (u32, Vec2);

struct Ctx {
    undecided: bool,
}

impl Ctx {
    fn o(&mut self, a: Vec2, b: Vec2, c: Vec2) -> Option<Predicate> {
        match orient2d(a, b, c) {
            Predicate::Uncertain(_) => {
                self.undecided = true;
                None
            }
            p => Some(p),
        }
    }

    /// True when the segments p and q cross or touch, conservatively (an undecided predicate counts as crossing).
    /// Segments sharing an endpoint position do not count.
    fn crosses(&mut self, p1: Vec2, p2: Vec2, q1: Vec2, q2: Vec2) -> bool {
        if p1 == q1 || p1 == q2 || p2 == q1 || p2 == q2 {
            return false;
        }
        let (d1, d2, d3, d4) = (self.o(q1, q2, p1), self.o(q1, q2, p2), self.o(p1, p2, q1), self.o(p1, p2, q2));
        // Both ends of one segment strictly on one side of the other: no crossing, whatever the rest says.
        let one_side = |x: Option<Predicate>, y: Option<Predicate>| {
            x == y && matches!(x, Some(Predicate::Positive) | Some(Predicate::Negative))
        };
        if one_side(d1, d2) || one_side(d3, d4) {
            return false;
        }
        let (Some(d1), Some(d2), Some(d3), Some(d4)) = (d1, d2, d3, d4) else {
            return true;
        };
        if d1 == Predicate::Zero || d2 == Predicate::Zero || d3 == Predicate::Zero || d4 == Predicate::Zero {
            return true;
        }
        d1 != d2 && d3 != d4
    }
}

fn edges(poly: &[Node]) -> impl Iterator<Item = (Vec2, Vec2)> + '_ {
    (0..poly.len()).map(move |i| (poly[i].1, poly[(i + 1) % poly.len()].1))
}

fn bridge(ctx: &mut Ctx, poly: &mut Vec<Node>, hole: &[Node], rest: &[&[Node]]) -> Result<(), EarFail> {
    let mut mi = 0;
    for (i, h) in hole.iter().enumerate() {
        if h.1.x > hole[mi].1.x {
            mi = i;
        }
    }
    let m = hole[mi].1;
    let mut order: Vec<usize> = (0..poly.len()).collect();
    order.sort_by(|&a, &b| {
        let (da, db) = ((poly[a].1 - m).mag_sq(), (poly[b].1 - m).mag_sq());
        da.partial_cmp(&db).unwrap_or(core::cmp::Ordering::Equal)
    });
    let n = poly.len();
    for &pi in &order {
        let p = poly[pi].1;
        if p == m {
            continue;
        }
        let (a, b) = (poly[(pi + n - 1) % n].1, poly[(pi + 1) % n].1);
        // M inside the interior wedge at P (counter-clockwise polygon: interior on the left).
        let (Some(w0), Some(l1), Some(l2)) = (ctx.o(a, p, b), ctx.o(a, p, m), ctx.o(p, b, m)) else { continue };
        let inside = if w0 == Predicate::Positive {
            l1 == Predicate::Positive && l2 == Predicate::Positive
        } else {
            l1 == Predicate::Positive || l2 == Predicate::Positive
        };
        if !inside {
            continue;
        }
        let mut blocked = false;
        for (q1, q2) in edges(poly).chain(edges(hole)).collect::<Vec<_>>() {
            if ctx.crosses(m, p, q1, q2) {
                blocked = true;
                break;
            }
        }
        if !blocked {
            for r in rest {
                for (q1, q2) in edges(r).collect::<Vec<_>>() {
                    if ctx.crosses(m, p, q1, q2) {
                        blocked = true;
                        break;
                    }
                }
            }
        }
        if blocked {
            continue;
        }
        let mut out = Vec::with_capacity(poly.len() + hole.len() + 2);
        out.extend_from_slice(&poly[..=pi]);
        out.extend_from_slice(&hole[mi..]);
        out.extend_from_slice(&hole[..=mi]);
        out.push(poly[pi]);
        out.extend_from_slice(&poly[pi + 1..]);
        *poly = out;
        return Ok(());
    }
    Err(if ctx.undecided { EarFail::Undecided } else { EarFail::Stuck })
}

/// Triangulate the region inside `outer` (counter-clockwise) minus `holes` (clockwise). Triangles are node triples,
/// counter-clockwise.
pub(crate) fn earcut(outer: &[Node], holes: &[Vec<Node>]) -> Result<Vec<[u32; 3]>, EarFail> {
    let mut ctx = Ctx { undecided: false };
    let mut poly: Vec<Node> = outer.to_vec();
    let maxx = |h: &Vec<Node>| h.iter().map(|n| n.1.x).fold(f64::NEG_INFINITY, f64::max);
    let mut hs: Vec<&Vec<Node>> = holes.iter().collect();
    hs.sort_by(|a, b| maxx(*b).partial_cmp(&maxx(*a)).unwrap_or(core::cmp::Ordering::Equal));
    for k in 0..hs.len() {
        let rest: Vec<&[Node]> = hs[k + 1..].iter().map(|h| h.as_slice()).collect();
        bridge(&mut ctx, &mut poly, hs[k], &rest)?;
    }

    let mut v = poly;
    let mut tris = Vec::new();
    while v.len() > 3 {
        let n = v.len();
        let mut found = None;
        for i in 0..n {
            let (ip, inx) = ((i + n - 1) % n, (i + 1) % n);
            let (a, b, c) = (v[ip], v[i], v[inx]);
            if a.0 == b.0 || b.0 == c.0 || a.0 == c.0 {
                continue;
            }
            if ctx.o(a.1, b.1, c.1) != Some(Predicate::Positive) {
                continue;
            }
            let mut ok = true;
            for (k, p) in v.iter().enumerate() {
                if k == i || k == ip || k == inx || p.1 == a.1 || p.1 == b.1 || p.1 == c.1 {
                    continue;
                }
                let s = [ctx.o(a.1, b.1, p.1), ctx.o(b.1, c.1, p.1), ctx.o(c.1, a.1, p.1)];
                // One decided Negative puts p outside the triangle, whatever the others say.
                if s.contains(&Some(Predicate::Negative)) {
                    continue;
                }
                let (Some(s1), Some(s2), Some(s3)) = (s[0], s[1], s[2]) else {
                    ok = false;
                    break;
                };
                if s1 != Predicate::Negative && s2 != Predicate::Negative && s3 != Predicate::Negative {
                    ok = false;
                    break;
                }
            }
            if ok {
                found = Some(i);
                break;
            }
        }
        let Some(i) = found else {
            return Err(if ctx.undecided { EarFail::Undecided } else { EarFail::Stuck });
        };
        tris.push([v[(i + n - 1) % n].0, v[i].0, v[(i + 1) % n].0]);
        v.remove(i);
    }
    if v.len() == 3 && v[0].0 != v[1].0 && v[1].0 != v[2].0 && v[0].0 != v[2].0 {
        tris.push([v[0].0, v[1].0, v[2].0]);
    }
    Ok(tris)
}
