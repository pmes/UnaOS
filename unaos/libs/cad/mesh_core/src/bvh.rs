//! A bounding volume hierarchy over a mesh's triangles for the ray pick: binary, built iteratively with an explicit
//! stack, each inner node split at the MEDIAN of its triangles' centroids along the longest axis of their centroid
//! box (`select_nth_unstable_by`), leaves of at most [`LEAF`] triangles. The pick culls nodes with geom's slab test
//! against the best `t` so far and tests triangles with geom's Moller-Trumbore; the nearest hit wins.

use alloc::vec::Vec;

use crate::Mesh;
use crate::geom::{self, Aabb3, Ray, Tol, Vec3};

/// The most triangles a leaf holds.
pub const LEAF: usize = 4;

/// A ray hit: the ray parameter, the triangle, and the barycentric weights of its three corners (summing to 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hit {
    pub t: f64,
    pub tri: u32,
    pub bary: [f64; 3],
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Node {
    bb: Aabb3,
    /// Leaf: first index into `order`; inner: the left child (the right child is `first + 1`).
    first: u32,
    /// Leaf: triangle count (> 0); inner: 0.
    count: u32,
}

/// The hierarchy; it holds triangle ids, not geometry, so a pick takes the mesh it was built from.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bvh {
    nodes: Vec<Node>,
    order: Vec<u32>,
}

impl Bvh {
    /// Build over every in-range triangle of `mesh` (out-of-range triangles are left out).
    pub fn build(mesh: &Mesh) -> Bvh {
        let nt = mesh.triangles.len();
        let mut boxes: Vec<Option<Aabb3>> = Vec::with_capacity(nt);
        let mut cent: Vec<Vec3> = Vec::with_capacity(nt);
        let mut order = Vec::new();
        for t in 0..nt {
            match mesh.corners(t) {
                Some(p) => {
                    boxes.push(geom::aabb_of(&p));
                    cent.push((p[0] + p[1] + p[2]) * (1.0 / 3.0));
                    order.push(t as u32);
                }
                None => {
                    boxes.push(None);
                    cent.push(Vec3::ZERO);
                }
            }
        }
        let mut bvh = Bvh { nodes: Vec::new(), order };
        if bvh.order.is_empty() {
            return bvh;
        }
        let empty = Aabb3 { min: Vec3::ZERO, max: Vec3::ZERO };
        bvh.nodes.push(Node { bb: empty, first: 0, count: 0 });
        let mut stack: Vec<(usize, usize, usize)> = alloc::vec![(0, 0, bvh.order.len())];
        while let Some((ni, start, end)) = stack.pop() {
            let ids = &mut bvh.order[start..end];
            let mut bb: Option<Aabb3> = None;
            let mut cb: Option<Aabb3> = None;
            for &t in ids.iter() {
                let tb = boxes[t as usize].unwrap_or(empty);
                bb = Some(bb.map_or(tb, |b| geom::aabb_union(b, tb)));
                let c = cent[t as usize];
                let cbox = Aabb3 { min: c, max: c };
                cb = Some(cb.map_or(cbox, |b| geom::aabb_union(b, cbox)));
            }
            let bb = bb.unwrap_or(empty);
            let count = end - start;
            if count <= LEAF {
                bvh.nodes[ni] = Node { bb, first: start as u32, count: count as u32 };
                continue;
            }
            let cb = cb.unwrap_or(empty);
            let ext = cb.max - cb.min;
            let axis = if ext.x >= ext.y && ext.x >= ext.z {
                0
            } else if ext.y >= ext.z {
                1
            } else {
                2
            };
            let half = count / 2;
            ids.select_nth_unstable_by(half, |a, b| {
                geom::comp(cent[*a as usize], axis).total_cmp(&geom::comp(cent[*b as usize], axis))
            });
            let left = bvh.nodes.len();
            bvh.nodes.push(Node { bb: empty, first: 0, count: 0 });
            bvh.nodes.push(Node { bb: empty, first: 0, count: 0 });
            bvh.nodes[ni] = Node { bb, first: left as u32, count: 0 };
            stack.push((left, start, start + half));
            stack.push((left + 1, start + half, end));
        }
        bvh
    }

    /// The nearest hit of `ray` (t >= 0) on `mesh`, which must be the mesh this BVH was built from (an id out of
    /// range for `mesh` is skipped, never a panic).
    pub fn pick(&self, mesh: &Mesh, ray: &Ray, tol: Tol) -> Option<Hit> {
        let mut best: Option<Hit> = None;
        if self.nodes.is_empty() {
            return None;
        }
        let mut stack: Vec<usize> = alloc::vec![0];
        while let Some(ni) = stack.pop() {
            let Some(node) = self.nodes.get(ni) else { continue };
            let Some((tn, _)) = geom::ray_aabb(ray, &node.bb) else { continue };
            if let Some(b) = best {
                if tn > b.t {
                    continue;
                }
            }
            if node.count == 0 {
                stack.push(node.first as usize);
                stack.push(node.first as usize + 1);
                continue;
            }
            let s = node.first as usize;
            for &t in self.order.get(s..s + node.count as usize).unwrap_or(&[]) {
                let Some([a, b, c]) = mesh.corners(t as usize) else { continue };
                if let Some((th, u, v)) = geom::ray_triangle(ray, a, b, c, tol) {
                    if best.map_or(true, |h| th < h.t) {
                        best = Some(Hit { t: th, tri: t, bary: [1.0 - u - v, u, v] });
                    }
                }
            }
        }
        best
    }

    // VUGSCANDEVIATION (SR510): the closest-point query beside the ray pick, over the same nodes.
    /// The point of `mesh` (the mesh this BVH was built from) nearest `p` within `reach` mm: branch and bound on the
    /// squared point-box distance against the best so far (starting at `reach²`), triangles through geom_core's
    /// closest point (a degenerate triangle is skipped, as `pick` skips a miss). `None` when nothing is within reach.
    pub fn closest(&self, mesh: &Mesh, p: Vec3, reach: f64, tol: Tol) -> Option<Near> {
        if self.nodes.is_empty() || !p.is_finite() || !(reach >= 0.0) {
            return None;
        }
        let mut best_sq = reach * reach;
        let mut best: Option<Near> = None;
        let mut stack: Vec<usize> = alloc::vec![0];
        while let Some(ni) = stack.pop() {
            let Some(node) = self.nodes.get(ni) else { continue };
            if geom::aabb_dist_sq(p, &node.bb) > best_sq {
                continue;
            }
            if node.count == 0 {
                let (l, r) = (node.first as usize, node.first as usize + 1);
                let dl = self.nodes.get(l).map_or(f64::INFINITY, |n| geom::aabb_dist_sq(p, &n.bb));
                let dr = self.nodes.get(r).map_or(f64::INFINITY, |n| geom::aabb_dist_sq(p, &n.bb));
                // The nearer child is popped first, so its triangles tighten the bound before the farther is opened.
                if dl <= dr {
                    stack.push(r);
                    stack.push(l);
                } else {
                    stack.push(l);
                    stack.push(r);
                }
                continue;
            }
            let s = node.first as usize;
            for &t in self.order.get(s..s + node.count as usize).unwrap_or(&[]) {
                let Some([a, b, c]) = mesh.corners(t as usize) else { continue };
                let Some(q) = geom::closest_on_triangle(p, a, b, c, tol) else { continue };
                let d2 = (q - p).mag_sq();
                if d2 <= best_sq && best.map_or(true, |n| d2 < n.dist * n.dist) {
                    best_sq = d2;
                    best = Some(Near { tri: t, point: q, dist: libm::sqrt(d2) });
                }
            }
        }
        best
    }
}

/// VUGSCANDEVIATION (SR510): a closest-point answer: the triangle, the point on it and its distance (mm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Near {
    pub tri: u32,
    pub point: Vec3,
    pub dist: f64,
}
