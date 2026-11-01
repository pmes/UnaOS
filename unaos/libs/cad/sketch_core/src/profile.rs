// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Profiles: the closed regions a sketch bounds, as VUG-BREP's extrude and revolve take them. Lines and arcs are
//! joined where their ends meet within `Tol.lin`; a node of degree other than two drops its chain (an open chain or a
//! branch bounds nothing); a circle is a one-edge loop. Each loop's signed area is exact (the chord polygon plus each
//! arc's circular segment); nesting is by parity (even depth is an outer loop, odd a hole of the loop around it);
//! the outer loop is normalised CCW and its holes CW, so the material is always on the left. Loops are assumed not
//! to cross (VUG-SKETCH-SPLINE owes the detection).

use crate::entity::{Entity, EntityId, PointRef, Sketch};
use crate::geom::{self, Arc2, Tol, Vec2};
use alloc::vec;
use alloc::vec::Vec;
use core::f64::consts::PI;

/// Arc samples per radian of sweep for the containment test only (areas are exact): 64 per turn.
const SAMPLES_PER_TURN: f64 = 64.0;

/// One edge of a loop, in the loop's direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Edge {
    Line { from: Vec2, to: Vec2, src: EntityId },
    /// `arc.sweep` is signed: positive runs CCW.
    Arc { arc: Arc2, src: EntityId },
}

fn arc_at(a: &Arc2, t: f64) -> Vec2 {
    Vec2::new(a.centre.x + a.radius * libm::cos(t), a.centre.y + a.radius * libm::sin(t))
}

impl Edge {
    pub fn start(&self) -> Vec2 {
        match self {
            Edge::Line { from, .. } => *from,
            Edge::Arc { arc, .. } => arc_at(arc, arc.start),
        }
    }
    pub fn end(&self) -> Vec2 {
        match self {
            Edge::Line { to, .. } => *to,
            Edge::Arc { arc, .. } => arc_at(arc, arc.start + arc.sweep),
        }
    }
    pub fn src(&self) -> EntityId {
        match self {
            Edge::Line { src, .. } | Edge::Arc { src, .. } => *src,
        }
    }
    fn reversed(&self) -> Edge {
        match *self {
            Edge::Line { from, to, src } => Edge::Line { from: to, to: from, src },
            Edge::Arc { arc, src } => {
                Edge::Arc { arc: Arc2 { start: arc.start + arc.sweep, sweep: -arc.sweep, ..arc }, src }
            }
        }
    }
    fn mid(&self) -> Vec2 {
        match self {
            Edge::Line { from, to, .. } => Vec2::new(0.5 * (from.x + to.x), 0.5 * (from.y + to.y)),
            Edge::Arc { arc, .. } => arc_at(arc, arc.start + 0.5 * arc.sweep),
        }
    }
    /// The area between the arc and its chord, signed by the direction of travel.
    fn segment_area(&self) -> f64 {
        match self {
            Edge::Line { .. } => 0.0,
            Edge::Arc { arc, .. } => 0.5 * arc.radius * arc.radius * (arc.sweep - libm::sin(arc.sweep)),
        }
    }
    fn samples(&self, out: &mut Vec<Vec2>) {
        match self {
            Edge::Line { from, .. } => out.push(*from),
            Edge::Arc { arc, .. } => {
                let k = libm::ceil(libm::fabs(arc.sweep) * SAMPLES_PER_TURN / (2.0 * PI));
                let k = if k < 1.0 { 1 } else if k > 4096.0 { 4096 } else { k as usize };
                for i in 0..k {
                    out.push(arc_at(arc, arc.start + arc.sweep * (i as f64) / (k as f64)));
                }
            }
        }
    }
}

/// A closed loop and its signed area (CCW positive).
#[derive(Clone, Debug, PartialEq)]
pub struct Loop {
    pub edges: Vec<Edge>,
    pub area: f64,
}

impl Loop {
    fn new(edges: Vec<Edge>) -> Loop {
        let verts: Vec<Vec2> = edges.iter().map(|e| e.start()).collect();
        let area = geom::area(&verts) + edges.iter().map(|e| e.segment_area()).sum::<f64>();
        Loop { edges, area }
    }
    fn oriented(mut self, ccw: bool) -> Loop {
        if (self.area > 0.0) != ccw {
            self.edges = self.edges.iter().rev().map(|e| e.reversed()).collect();
            self.area = -self.area;
        }
        self
    }
    fn polygon(&self) -> Vec<Vec2> {
        let mut out = Vec::new();
        for e in &self.edges {
            e.samples(&mut out);
        }
        out
    }
}

/// A region: the outer loop (CCW) and the holes inside it (CW).
#[derive(Clone, Debug, PartialEq)]
pub struct Profile {
    pub outer: Loop,
    pub holes: Vec<Loop>,
}

fn contains(poly: &[Vec2], p: Vec2) -> bool {
    let n = poly.len();
    let mut inside = false;
    let mut j = n.wrapping_sub(1);
    for i in 0..n {
        let (a, b) = (poly[i], poly[j]);
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// The closed regions of the sketch at its current parameters.
pub fn profiles(sk: &Sketch, tol: Tol) -> Vec<Profile> {
    let x = sk.params();
    let mut loops: Vec<Loop> = Vec::new();
    let mut edges: Vec<Edge> = Vec::new();
    for (i, e) in sk.entities().iter().enumerate() {
        let src = EntityId(i as u32);
        match *e {
            Entity::Line { .. } => {
                if let (Some(from), Some(to)) = (sk.point(PointRef::Start(src)), sk.point(PointRef::End(src))) {
                    edges.push(Edge::Line { from, to, src });
                }
            }
            Entity::Arc { r, .. } => {
                if let Some(centre) = sk.point(PointRef::Centre(src)) {
                    let mut sweep = x[r + 2] - x[r + 1];
                    sweep -= 2.0 * PI * libm::floor(sweep / (2.0 * PI));
                    if !(sweep > 0.0) {
                        sweep = 2.0 * PI;
                    }
                    edges.push(Edge::Arc { arc: Arc2 { centre, radius: x[r], start: x[r + 1], sweep }, src });
                }
            }
            Entity::Circle { r, .. } => {
                if let Some(centre) = sk.point(PointRef::Centre(src)) {
                    let arc = Arc2 { centre, radius: x[r], start: 0.0, sweep: 2.0 * PI };
                    loops.push(Loop::new(vec![Edge::Arc { arc, src }]));
                }
            }
            Entity::Point { .. } => {}
        }
    }

    // Nodes: ends joined within Tol.lin.
    let mut nodes: Vec<Vec2> = Vec::new();
    let mut node_of = |p: Vec2| -> usize {
        for (i, q) in nodes.iter().enumerate() {
            let (dx, dy) = (p.x - q.x, p.y - q.y);
            if dx * dx + dy * dy <= tol.lin * tol.lin {
                return i;
            }
        }
        nodes.push(p);
        nodes.len() - 1
    };
    let ends: Vec<(usize, usize)> = edges.iter().map(|e| (node_of(e.start()), node_of(e.end()))).collect();
    let nn = nodes.len();

    // Prune every edge at a node of degree other than two, until stable.
    let mut alive = vec![true; edges.len()];
    loop {
        let mut deg = vec![0usize; nn];
        for (k, &(a, b)) in ends.iter().enumerate() {
            if alive[k] {
                deg[a] += 1;
                deg[b] += 1;
            }
        }
        let mut changed = false;
        for (k, &(a, b)) in ends.iter().enumerate() {
            if alive[k] && (deg[a] != 2 || deg[b] != 2) {
                alive[k] = false;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }

    // Walk each remaining cycle.
    let mut used = vec![false; edges.len()];
    for e0 in 0..edges.len() {
        if !alive[e0] || used[e0] {
            continue;
        }
        used[e0] = true;
        let (start, mut cur) = ends[e0];
        let mut lp = vec![edges[e0]];
        let mut closed = start == cur;
        let mut guard = edges.len();
        while !closed && guard > 0 {
            guard -= 1;
            let next = (0..edges.len()).find(|&k| alive[k] && !used[k] && (ends[k].0 == cur || ends[k].1 == cur));
            let Some(k) = next else { break };
            used[k] = true;
            if ends[k].0 == cur {
                lp.push(edges[k]);
                cur = ends[k].1;
            } else {
                lp.push(edges[k].reversed());
                cur = ends[k].0;
            }
            closed = cur == start;
        }
        if closed {
            loops.push(Loop::new(lp));
        }
    }

    // Nesting by parity.
    let polys: Vec<Vec<Vec2>> = loops.iter().map(|l| l.polygon()).collect();
    let probe: Vec<Vec2> = loops.iter().map(|l| l.edges[0].mid()).collect();
    let nl = loops.len();
    let inside = |i: usize, j: usize| i != j && contains(&polys[j], probe[i]);
    let depth: Vec<usize> = (0..nl).map(|i| (0..nl).filter(|&j| inside(i, j)).count()).collect();
    let parent: Vec<Option<usize>> = (0..nl)
        .map(|i| {
            if depth[i] % 2 == 0 {
                return None;
            }
            (0..nl)
                .filter(|&j| inside(i, j) && depth[j] + 1 == depth[i])
                .min_by(|&a, &b| libm::fabs(loops[a].area).total_cmp(&libm::fabs(loops[b].area)))
        })
        .collect();

    let mut out = Vec::new();
    for i in 0..nl {
        if depth[i] % 2 != 0 {
            continue;
        }
        let holes = (0..nl).filter(|&h| parent[h] == Some(i)).map(|h| loops[h].clone().oriented(false)).collect();
        out.push(Profile { outer: loops[i].clone().oriented(true), holes });
    }
    out
}
