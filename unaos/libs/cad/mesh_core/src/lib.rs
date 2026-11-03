//! CHARTER: Vug (CODEX §2, 3D), seam shared-core — the triangle mesh every CAD core tessellates into and every exchange
//! format reads and writes; the viewport's f32 buffers come from here and nowhere else.
//!
//! VUGMESH (LEDGER SR466). Geometry is `f64` (BP-vug-1); the one `f32` conversion is [`Mesh::to_f32_buffers`] (STL's
//! on-disk `f32` is that file format's, named in [`stl`]). Every threshold is a [`Tol`] field (BP-vug-2): the ONE
//! degenerate definition is [`degenerate`]. Every core fn is pure: a value in, a value or a named refusal out (BP-vug-4).
//! The formats are read from their specifications: [`stl`] (binary + ASCII), [`obj`] (Wavefront), [`ply`] (Stanford).
#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

mod bytes;
mod geom;

pub mod bvh;
pub mod halfedge;
pub mod obj;
pub mod ply;
pub mod stl;

pub use bvh::{Bvh, Hit};
pub use geom::{Aabb3, Ray, Tol, Vec3};
pub use halfedge::{HalfEdge, Topo};

/// An indexed triangle mesh: positions in millimetres and counter-clockwise (outward) triangles.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
}

/// Why [`Mesh::validate`] refuses a mesh (the first failure, checked in this order).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Invalid {
    /// A position with a NaN or infinite coordinate.
    NonFinite { vertex: u32 },
    /// A triangle naming a vertex that does not exist.
    IndexOutOfRange { tri: u32, index: u32 },
    /// A triangle that is [`degenerate`] under the `Tol`.
    Degenerate { tri: u32 },
    /// A triangle on the same three vertices as an earlier one (any rotation, either orientation).
    Duplicate { tri: u32, of: u32 },
}

/// The result of [`Mesh::weld`].
#[derive(Clone, Debug, PartialEq)]
pub struct Weld {
    /// The welded mesh (representatives keep their exact original coordinates).
    pub mesh: Mesh,
    /// Old vertex index -> new vertex index.
    pub remap: Vec<u32>,
    /// Indices (in the input) of the triangles dropped because two of their corners welded together.
    pub collapsed: Vec<u32>,
}

/// The viewport's buffers (BP-vug-1: the one `f32` conversion): `positions` and `normals` are xyz-interleaved, one
/// triple per vertex; `indices` three per triangle.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Buffers {
    pub positions: Vec<f32>,
    pub normals: Vec<f32>,
    pub indices: Vec<u32>,
}

/// THE degenerate definition: the triangle's height over its longest edge is at most `tol.lin`
/// (`|(b-a) x (c-a)| <= tol.lin * longest edge`). This covers repeated corners and zero-length edges.
pub fn degenerate(a: Vec3, b: Vec3, c: Vec3, tol: Tol) -> bool {
    let e = (b - a).magnitude().max((c - b).magnitude()).max((a - c).magnitude());
    let cr = (b - a).cross(c - a).magnitude();
    !(cr > tol.lin * e)
}

impl Mesh {
    /// The three corners of triangle `t`, or `None` when `t` or one of its indices is out of range.
    pub fn corners(&self, t: usize) -> Option<[Vec3; 3]> {
        let tri = self.triangles.get(t)?;
        Some([
            *self.positions.get(tri[0] as usize)?,
            *self.positions.get(tri[1] as usize)?,
            *self.positions.get(tri[2] as usize)?,
        ])
    }

    /// Index range, finiteness, degenerate and duplicate triangles, each refusal named.
    pub fn validate(&self, tol: Tol) -> Result<(), Invalid> {
        for (i, p) in self.positions.iter().enumerate() {
            if !p.is_finite() {
                return Err(Invalid::NonFinite { vertex: i as u32 });
            }
        }
        let n = self.positions.len();
        for (t, tri) in self.triangles.iter().enumerate() {
            for &ix in tri {
                if ix as usize >= n {
                    return Err(Invalid::IndexOutOfRange { tri: t as u32, index: ix });
                }
            }
        }
        for t in 0..self.triangles.len() {
            if let Some([a, b, c]) = self.corners(t) {
                if degenerate(a, b, c, tol) {
                    return Err(Invalid::Degenerate { tri: t as u32 });
                }
            }
        }
        let mut keys: Vec<([u32; 3], u32)> = self
            .triangles
            .iter()
            .enumerate()
            .map(|(t, tri)| {
                let mut k = *tri;
                k.sort_unstable();
                (k, t as u32)
            })
            .collect();
        keys.sort_unstable();
        // sorted by (vertex set, index): each run's first entry is the earliest; its second is the run's first duplicate
        let mut first: Option<(u32, u32)> = None; // (tri, of) with the smallest duplicate index
        let mut i = 0;
        while i < keys.len() {
            let mut j = i + 1;
            while j < keys.len() && keys[j].0 == keys[i].0 {
                j += 1;
            }
            if j - i >= 2 {
                let cand = (keys[i + 1].1, keys[i].1);
                if first.map_or(true, |f| cand.0 < f.0) {
                    first = Some(cand);
                }
            }
            i = j;
        }
        if let Some((tri, of)) = first {
            return Err(Invalid::Duplicate { tri, of });
        }
        Ok(())
    }

    /// Unit face normals (right hand, counter-clockwise); a degenerate or out-of-range triangle gets the zero vector.
    pub fn normals_face(&self, tol: Tol) -> Vec<Vec3> {
        (0..self.triangles.len())
            .map(|t| match self.corners(t) {
                Some([a, b, c]) if !degenerate(a, b, c, tol) => (b - a).cross(c - a).normalize().unwrap_or(Vec3::ZERO),
                _ => Vec3::ZERO,
            })
            .collect()
    }

    /// Angle-weighted vertex normals (Thurmer and Wuthrich 1998): the sum over the incident non-degenerate triangles
    /// of the corner angle times the unit face normal, normalised; a vertex with none gets the zero vector.
    pub fn normals_vertex(&self, tol: Tol) -> Vec<Vec3> {
        let mut acc = alloc::vec![Vec3::ZERO; self.positions.len()];
        for t in 0..self.triangles.len() {
            let Some(p) = self.corners(t) else { continue };
            if degenerate(p[0], p[1], p[2], tol) {
                continue;
            }
            let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize().unwrap_or(Vec3::ZERO);
            let tri = self.triangles[t];
            for k in 0..3 {
                let u = p[(k + 1) % 3] - p[k];
                let v = p[(k + 2) % 3] - p[k];
                let angle = libm::atan2(u.cross(v).magnitude(), u.dot(v));
                let i = tri[k] as usize;
                acc[i] = acc[i] + n * angle;
            }
        }
        acc.into_iter().map(|v| v.normalize().unwrap_or(Vec3::ZERO)).collect()
    }

    /// The box of every position (`None` for no positions).
    pub fn aabb(&self) -> Option<Aabb3> {
        geom::aabb_of(&self.positions)
    }

    /// The total surface area (out-of-range triangles contribute nothing).
    pub fn area(&self) -> f64 {
        (0..self.triangles.len())
            .filter_map(|t| self.corners(t))
            .map(|[a, b, c]| 0.5 * (b - a).cross(c - a).magnitude())
            .sum()
    }

    /// The signed enclosed volume (divergence theorem, positive for an outward-oriented solid); closed meshes only:
    /// a mesh whose half-edge table has a boundary is `Topo::NotClosed`. Degenerate triangles (under `tol`) are skipped.
    pub fn volume(&self, tol: Tol) -> Result<f64, Topo> {
        let he = HalfEdge::build(self)?;
        if !he.is_closed() {
            return Err(Topo::NotClosed { boundary_edges: he.boundary_edges() });
        }
        let mut v = 0.0;
        for t in 0..self.triangles.len() {
            if let Some([a, b, c]) = self.corners(t) {
                if !degenerate(a, b, c, tol) {
                    v += a.dot(b.cross(c));
                }
            }
        }
        Ok(v / 6.0)
    }

    /// V - E + F over the referenced vertices, the unique undirected edges and the triangles (in-range ones only).
    pub fn euler_characteristic(&self) -> i64 {
        let n = self.positions.len();
        let mut used = alloc::vec![false; n];
        let mut edges: Vec<(u32, u32)> = Vec::new();
        let mut f = 0i64;
        for tri in &self.triangles {
            if tri.iter().any(|&i| i as usize >= n) {
                continue;
            }
            f += 1;
            for k in 0..3 {
                used[tri[k] as usize] = true;
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                edges.push((a.min(b), a.max(b)));
            }
        }
        edges.sort_unstable();
        edges.dedup();
        used.iter().filter(|&&u| u).count() as i64 - edges.len() as i64 + f
    }

    /// The boundary loops as vertex cycles (empty for a closed mesh); refuses what `HalfEdge::build` refuses.
    pub fn boundary_loops(&self) -> Result<Vec<Vec<u32>>, Topo> {
        Ok(HalfEdge::build(self)?.boundary_loops())
    }

    /// Weld vertices within `tol.lin`: a spatial hash on cells of 1 x `tol.lin` (a point within `tol.lin` lies at most
    /// one cell away per axis, so the 27 neighbour cells are probed), each vertex compared against REPRESENTATIVES only,
    /// first come in input order (deterministic, no chaining). A non-positive `tol.lin` welds exact duplicates only.
    pub fn weld(&self, tol: Tol) -> Weld {
        let cell = if tol.lin > 0.0 && tol.lin.is_finite() { tol.lin } else { 1.0 };
        let reach = tol.lin.max(0.0);
        let key = |p: Vec3| -> (i64, i64, i64) {
            (libm::floor(p.x / cell) as i64, libm::floor(p.y / cell) as i64, libm::floor(p.z / cell) as i64)
        };
        let mut grid: BTreeMap<(i64, i64, i64), Vec<u32>> = BTreeMap::new();
        let mut reps: Vec<Vec3> = Vec::new();
        let mut remap = Vec::with_capacity(self.positions.len());
        for &p in &self.positions {
            let (kx, ky, kz) = key(p);
            let mut found = None;
            'probe: for dx in -1i64..=1 {
                for dy in -1i64..=1 {
                    for dz in -1i64..=1 {
                        let k = (kx.saturating_add(dx), ky.saturating_add(dy), kz.saturating_add(dz));
                        if let Some(list) = grid.get(&k) {
                            for &r in list {
                                if (reps[r as usize] - p).magnitude() <= reach {
                                    found = Some(r);
                                    break 'probe;
                                }
                            }
                        }
                    }
                }
            }
            let r = match found {
                Some(r) => r,
                None => {
                    let r = reps.len() as u32;
                    reps.push(p);
                    grid.entry((kx, ky, kz)).or_default().push(r);
                    r
                }
            };
            remap.push(r);
        }
        let mut triangles = Vec::with_capacity(self.triangles.len());
        let mut collapsed = Vec::new();
        for (t, tri) in self.triangles.iter().enumerate() {
            let m = tri.map(|i| remap.get(i as usize).copied().unwrap_or(u32::MAX));
            if m.contains(&u32::MAX) || m[0] == m[1] || m[1] == m[2] || m[0] == m[2] {
                collapsed.push(t as u32);
            } else {
                triangles.push(m);
            }
        }
        Weld { mesh: Mesh { positions: reps, triangles }, remap, collapsed }
    }

    /// Drop the [`degenerate`] (and out-of-range) triangles; vertex indices are unchanged. Returns the mesh and the
    /// input indices of the dropped triangles.
    pub fn remove_degenerate(&self, tol: Tol) -> (Mesh, Vec<u32>) {
        let mut keep = Vec::with_capacity(self.triangles.len());
        let mut removed = Vec::new();
        for (t, tri) in self.triangles.iter().enumerate() {
            match self.corners(t) {
                Some([a, b, c]) if !degenerate(a, b, c, tol) => keep.push(*tri),
                _ => removed.push(t as u32),
            }
        }
        (Mesh { positions: self.positions.clone(), triangles: keep }, removed)
    }

    /// The viewport's buffers, the ONE `f64 -> f32` conversion in the CAD cores (BP-vug-1): positions, the
    /// angle-weighted vertex normals, and the triangle indices.
    pub fn to_f32_buffers(&self, tol: Tol) -> Buffers {
        let mut positions = Vec::with_capacity(self.positions.len() * 3);
        for p in &self.positions {
            positions.extend_from_slice(&[p.x as f32, p.y as f32, p.z as f32]);
        }
        let mut normals = Vec::with_capacity(self.positions.len() * 3);
        for n in self.normals_vertex(tol) {
            normals.extend_from_slice(&[n.x as f32, n.y as f32, n.z as f32]);
        }
        let indices = self.triangles.iter().flat_map(|t| t.iter().copied()).collect();
        Buffers { positions, normals, indices }
    }
}
