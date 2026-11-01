// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

// SHIM: mesh_core (SR466) not on its branch at build time; the seat swaps this file for re-exports
// (`pub use mesh_core::{Mesh, Topo};`). The names and the refusal vocabulary are mesh_core's design's
// (wave36/vugmesh/design.md): `Mesh { positions, triangles }`, `boundary_loops`, `euler_characteristic`, `Topo`.
//! The indexed triangle mesh scan_core extracts into: ONLY what the crate uses.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::geom::Vec3;

/// An indexed triangle mesh (counter-clockwise = outward).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mesh {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
}

/// Why the mesh's topology was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topo {
    /// More than two triangles use the undirected edge.
    NonManifold { edge: [u32; 2], uses: u32 },
    /// Two triangles traverse the edge in the same direction (orientation flips).
    Inconsistent { edge: [u32; 2] },
    /// A vertex with more than one outgoing boundary half-edge (a bowtie).
    NonManifoldVertex { v: u32 },
    /// A triangle names a vertex that does not exist.
    IndexOutOfRange { tri: u32, index: u32 },
    /// The mesh is open (`boundary_edges` half-edges without a twin).
    NotClosed { boundary_edges: u32 },
}

impl Mesh {
    /// Directed half-edges `(min, max, forward, a, b)` sorted by their undirected key, after the range check.
    fn half_edges(&self) -> Result<Vec<(u32, u32, bool, u32, u32)>, Topo> {
        let n = self.positions.len() as u32;
        let mut he = Vec::with_capacity(self.triangles.len() * 3);
        for (t, tri) in self.triangles.iter().enumerate() {
            for k in 0..3 {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                if a >= n {
                    return Err(Topo::IndexOutOfRange { tri: t as u32, index: a });
                }
                he.push((a.min(b), a.max(b), a < b, a, b));
            }
        }
        he.sort_unstable();
        Ok(he)
    }

    /// V − E + F over the referenced vertices and the unique undirected edges.
    pub fn euler_characteristic(&self) -> i64 {
        let mut used = alloc::vec![false; self.positions.len()];
        let mut edges: Vec<(u32, u32)> = Vec::with_capacity(self.triangles.len() * 3);
        for tri in &self.triangles {
            for k in 0..3 {
                let (a, b) = (tri[k], tri[(k + 1) % 3]);
                if let Some(u) = used.get_mut(a as usize) {
                    *u = true;
                }
                edges.push((a.min(b), a.max(b)));
            }
        }
        edges.sort_unstable();
        edges.dedup();
        let v = used.iter().filter(|u| **u).count() as i64;
        v - edges.len() as i64 + self.triangles.len() as i64
    }

    /// The boundary loops (vertex cycles along the half-edges with no twin), or the topology refusal.
    /// A closed manifold mesh returns an empty list.
    pub fn boundary_loops(&self) -> Result<Vec<Vec<u32>>, Topo> {
        let he = self.half_edges()?;
        let mut out: BTreeMap<u32, u32> = BTreeMap::new();
        let mut i = 0;
        while i < he.len() {
            let mut j = i + 1;
            while j < he.len() && he[j].0 == he[i].0 && he[j].1 == he[i].1 {
                j += 1;
            }
            let edge = [he[i].0, he[i].1];
            match j - i {
                1 => {
                    let (a, b) = (he[i].3, he[i].4);
                    if out.insert(a, b).is_some() {
                        return Err(Topo::NonManifoldVertex { v: a });
                    }
                }
                2 => {
                    if he[i].2 == he[i + 1].2 {
                        return Err(Topo::Inconsistent { edge });
                    }
                }
                uses => return Err(Topo::NonManifold { edge, uses: uses as u32 }),
            }
            i = j;
        }
        let mut loops = Vec::new();
        while let Some((&start, _)) = out.iter().next() {
            let mut lp = Vec::new();
            let mut cur = start;
            while let Some(next) = out.remove(&cur) {
                lp.push(cur);
                cur = next;
            }
            loops.push(lp);
        }
        Ok(loops)
    }
}
