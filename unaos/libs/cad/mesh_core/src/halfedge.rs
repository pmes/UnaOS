//! Half-edge adjacency over a [`Mesh`]: half-edge `3t + k` runs `triangles[t][k] -> triangles[t][(k + 1) % 3]`, so
//! `next` and `face` are arithmetic and only `origin`, `twin` and one outgoing half-edge per vertex are stored. Twins
//! are found by sorting `(min, max, he)` edge keys: a key used once is a boundary, twice in opposite directions a twin
//! pair, twice in the same direction an orientation fault, more than twice a non-manifold edge. Closed (watertight)
//! means every half-edge has its twin.

use alloc::vec::Vec;

use crate::Mesh;

/// "No half-edge" (a boundary half-edge's twin, an unreferenced vertex's outgoing half-edge).
pub const NONE: u32 = u32::MAX;

/// Why a mesh has no (manifold, consistently oriented) half-edge table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Topo {
    /// A triangle naming a vertex that does not exist.
    IndexOutOfRange { tri: u32, index: u32 },
    /// An edge used by more than two triangles.
    NonManifold { edge: [u32; 2], uses: u32 },
    /// An edge whose two triangles run it in the same direction (the orientation flips across it).
    Inconsistent { edge: [u32; 2] },
    /// A vertex where two boundary fans meet (more than one outgoing boundary half-edge: a bowtie).
    NonManifoldVertex { v: u32 },
    /// The operation needs a closed mesh and this one has boundary half-edges.
    NotClosed { boundary_edges: u32 },
}

/// The half-edge table of a manifold, consistently oriented triangle mesh (possibly with boundary).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HalfEdge {
    /// Origin vertex of each half-edge.
    pub origin: Vec<u32>,
    /// Opposite half-edge, or [`NONE`] on the boundary.
    pub twin: Vec<u32>,
    /// One outgoing half-edge per vertex (the boundary one when the vertex is on the boundary), or [`NONE`].
    pub out: Vec<u32>,
}

impl HalfEdge {
    /// Build the table, refusing out-of-range indices, non-manifold edges and vertices, and inconsistent orientation.
    pub fn build(mesh: &Mesh) -> Result<HalfEdge, Topo> {
        let nv = mesh.positions.len();
        let n = mesh.triangles.len() * 3;
        let mut origin = Vec::with_capacity(n);
        for (t, tri) in mesh.triangles.iter().enumerate() {
            for &ix in tri {
                if ix as usize >= nv {
                    return Err(Topo::IndexOutOfRange { tri: t as u32, index: ix });
                }
                origin.push(ix);
            }
        }
        let mut keys: Vec<(u32, u32, u32)> = (0..n as u32)
            .map(|h| {
                let (a, b) = (origin[h as usize], origin[next(h) as usize]);
                (a.min(b), a.max(b), h)
            })
            .collect();
        keys.sort_unstable();
        let mut twin = alloc::vec![NONE; n];
        let mut i = 0;
        while i < keys.len() {
            let mut j = i + 1;
            while j < keys.len() && keys[j].0 == keys[i].0 && keys[j].1 == keys[i].1 {
                j += 1;
            }
            let edge = [keys[i].0, keys[i].1];
            match j - i {
                1 => {}
                2 => {
                    let (h0, h1) = (keys[i].2, keys[i + 1].2);
                    if origin[h0 as usize] == origin[h1 as usize] {
                        return Err(Topo::Inconsistent { edge });
                    }
                    twin[h0 as usize] = h1;
                    twin[h1 as usize] = h0;
                }
                uses => return Err(Topo::NonManifold { edge, uses: uses as u32 }),
            }
            i = j;
        }
        let mut out = alloc::vec![NONE; nv];
        for h in 0..n {
            out[origin[h] as usize] = h as u32;
        }
        let mut boundary_out = alloc::vec![0u32; nv];
        for h in 0..n {
            if twin[h] == NONE {
                let v = origin[h] as usize;
                boundary_out[v] += 1;
                if boundary_out[v] > 1 {
                    return Err(Topo::NonManifoldVertex { v: v as u32 });
                }
                out[v] = h as u32;
            }
        }
        Ok(HalfEdge { origin, twin, out })
    }

    /// The half-edge after `h` in its triangle.
    pub fn next(&self, h: u32) -> u32 {
        next(h)
    }

    /// The triangle `h` belongs to.
    pub fn face(&self, h: u32) -> u32 {
        h / 3
    }

    /// The vertex `h` points to.
    pub fn dest(&self, h: u32) -> u32 {
        self.origin.get(next(h) as usize).copied().unwrap_or(NONE)
    }

    /// Every half-edge has a twin.
    pub fn is_closed(&self) -> bool {
        self.twin.iter().all(|&t| t != NONE)
    }

    /// The number of boundary half-edges.
    pub fn boundary_edges(&self) -> u32 {
        self.twin.iter().filter(|&&t| t == NONE).count() as u32
    }

    /// The boundary loops as vertex cycles, each in the boundary half-edges' direction; empty for a closed mesh.
    pub fn boundary_loops(&self) -> Vec<Vec<u32>> {
        let n = self.twin.len();
        let mut seen = alloc::vec![false; n];
        let mut loops = Vec::new();
        for start in 0..n {
            if self.twin[start] != NONE || seen[start] {
                continue;
            }
            let mut lp = Vec::new();
            let mut h = start;
            loop {
                seen[h] = true;
                lp.push(self.origin[h]);
                let b = self.dest(h as u32);
                let nh = match self.out.get(b as usize) {
                    Some(&nh) if nh != NONE => nh as usize,
                    _ => break,
                };
                if self.twin[nh] != NONE || seen[nh] {
                    break;
                }
                h = nh;
            }
            loops.push(lp);
        }
        loops
    }
}

fn next(h: u32) -> u32 {
    h - h % 3 + (h + 1) % 3
}
