// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! Marching cubes (Lorensen & Cline, "Marching Cubes: A High Resolution 3D Surface Construction Algorithm",
//! SIGGRAPH 1987), with the corner and edge numbering and the cube index of Paul Bourke, "Polygonising a scalar
//! field" (1994): corner `i` sets bit `i` of the case when it is inside (signed distance < 0).
//!
//! The 256 triangle lists are GENERATED from one face rule, not transcribed (VUGSCAN1's measured choice: a 4096-entry
//! table typed from memory is a silent hole; a rule is checkable). On each cube face, walking its corners
//! counter-clockwise as seen from outside the cube, every outside→inside crossing is joined to the next crossing
//! (which is inside→outside), so the inside corners of an ambiguous face are separated. The rule reads only the
//! face's four corners, so the two cubes sharing a face draw the same segments (opposite directions): the surface is
//! watertight, without the 1987 table's holes. Each crossing is the tail of a segment on one of its edge's faces and
//! the head on the other, so the segments chain into loops; each loop is fanned into triangles whose normals point
//! out of the inside (toward positive distance). `edge_mask` is Bourke's edgeTable, computed; its first sixteen
//! published entries are pinned by a test.

use alloc::collections::BTreeMap;
use alloc::vec::Vec;

use crate::geom::Vec3;
use crate::tsdf::Tsdf;

/// Corner offsets (Bourke's numbering).
pub const CORNER: [[i32; 3]; 8] =
    [[0, 0, 0], [1, 0, 0], [1, 1, 0], [0, 1, 0], [0, 0, 1], [1, 0, 1], [1, 1, 1], [0, 1, 1]];
/// Edges as corner pairs (Bourke's numbering).
pub const EDGE: [[usize; 2]; 12] =
    [[0, 1], [1, 2], [2, 3], [3, 0], [4, 5], [5, 6], [6, 7], [7, 4], [0, 4], [1, 5], [2, 6], [3, 7]];
/// Faces, corners counter-clockwise seen from outside (right-hand rule gives the outward normal).
const FACE: [[usize; 4]; 6] =
    [[0, 3, 2, 1], [4, 5, 6, 7], [0, 1, 5, 4], [3, 7, 6, 2], [0, 4, 7, 3], [1, 2, 6, 5]];

fn edge_of(a: usize, b: usize) -> usize {
    let mut e = 0;
    while e < 12 {
        let [p, q] = EDGE[e];
        if (p == a && q == b) || (p == b && q == a) {
            return e;
        }
        e += 1;
    }
    0 // every FACE side is an EDGE: unreachable for the constant tables
}

/// Bourke's edgeTable entry: bit `e` set when edge `e` joins an inside and an outside corner.
pub fn edge_mask(case: u8) -> u16 {
    let mut m = 0u16;
    for (e, [a, b]) in EDGE.iter().enumerate() {
        if (case >> a & 1) != (case >> b & 1) {
            m |= 1 << e;
        }
    }
    m
}

/// The generated case table: per case, triangles as edge triples.
pub struct Table {
    tris: Vec<Vec<[u8; 3]>>,
}

impl Table {
    pub fn build() -> Table {
        let mut tris = Vec::with_capacity(256);
        for case in 0..256usize {
            let inside = |c: usize| (case >> c) & 1 == 1;
            let mut succ = [u8::MAX; 12];
            for f in FACE.iter() {
                let mut cr = [(0usize, false); 4];
                let mut n = 0;
                for k in 0..4 {
                    let (a, b) = (f[k], f[(k + 1) % 4]);
                    if inside(a) != inside(b) {
                        cr[n] = (edge_of(a, b), !inside(a) && inside(b));
                        n += 1;
                    }
                }
                for i in 0..n {
                    if cr[i].1 {
                        succ[cr[i].0] = cr[(i + 1) % n].0 as u8;
                    }
                }
            }
            let mut seen = [false; 12];
            let mut list = Vec::new();
            for e in 0..12 {
                if succ[e] == u8::MAX || seen[e] {
                    continue;
                }
                let mut lp: Vec<u8> = Vec::new();
                let mut cur = e;
                while !seen[cur] && succ[cur] != u8::MAX {
                    seen[cur] = true;
                    lp.push(cur as u8);
                    cur = succ[cur] as usize;
                }
                for i in 1..lp.len().saturating_sub(1) {
                    list.push([lp[0], lp[i], lp[i + 1]]);
                }
            }
            tris.push(list);
        }
        Table { tris }
    }

    /// The triangles (edge triples) of `case`.
    pub fn tris(&self, case: u8) -> &[[u8; 3]] {
        &self.tris[case as usize]
    }
}

/// A marched surface: positions (mm), shared-vertex triangles, the interpolated weight and normal per vertex.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Surface {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<[u32; 3]>,
    pub weights: Vec<f32>,
    pub normals: Vec<Vec3>,
}

type Key = (i32, i32, i32);

fn add(a: Key, d: [i32; 3]) -> Key {
    (a.0 + d[0], a.1 + d[1], a.2 + d[2])
}

/// March the cubes whose origins `origins` yields, over the sample field `sample` (`(sdf, weight)` of voxel `g`;
/// `None` or weight 0 = unobserved, and a cube with such a corner is skipped). Voxel `g` sits at `g · voxel_mm`.
pub fn march_cells<F>(origins: impl Iterator<Item = Key>, sample: F, voxel_mm: f64) -> Surface
where
    F: Fn(Key) -> Option<(f32, f32)>,
{
    let table = Table::build();
    let observed = |g: Key| sample(g).filter(|s| s.1 > 0.0);
    let pos = |g: Key| Vec3::new(g.0 as f64 * voxel_mm, g.1 as f64 * voxel_mm, g.2 as f64 * voxel_mm);
    // Central differences where both neighbours are observed, one-sided where one is, zero where neither.
    let grad = |g: Key| {
        let mut out = [0.0f64; 3];
        let here = observed(g).map(|s| s.0 as f64);
        for (ax, o) in out.iter_mut().enumerate() {
            let mut d = [0i32; 3];
            d[ax] = 1;
            let p = observed(add(g, d)).map(|s| s.0 as f64);
            let m = observed(add(g, [-d[0], -d[1], -d[2]])).map(|s| s.0 as f64);
            *o = match (p, m, here) {
                (Some(p), Some(m), _) => (p - m) / (2.0 * voxel_mm),
                (Some(p), None, Some(h)) => (p - h) / voxel_mm,
                (None, Some(m), Some(h)) => (h - m) / voxel_mm,
                _ => 0.0,
            };
        }
        Vec3::new(out[0], out[1], out[2])
    };
    let mut s = Surface::default();
    let mut verts: BTreeMap<(i32, i32, i32, u8), u32> = BTreeMap::new();
    for o in origins {
        let mut sd = [0f32; 8];
        let mut wt = [0f32; 8];
        let mut ok = true;
        for c in 0..8 {
            match observed(add(o, CORNER[c])) {
                Some((d, w)) => {
                    sd[c] = d;
                    wt[c] = w;
                }
                None => {
                    ok = false;
                    break;
                }
            }
        }
        if !ok {
            continue;
        }
        let mut case = 0u8;
        for (c, d) in sd.iter().enumerate() {
            if *d < 0.0 {
                case |= 1 << c;
            }
        }
        if case == 0 || case == 255 {
            continue;
        }
        for tri in table.tris(case) {
            let mut idx = [0u32; 3];
            for (k, e) in tri.iter().enumerate() {
                let [a, b] = EDGE[*e as usize];
                let (ga, gb) = (add(o, CORNER[a]), add(o, CORNER[b]));
                let lo = if ga <= gb { ga } else { gb };
                let axis = if ga.0 != gb.0 { 0 } else if ga.1 != gb.1 { 1 } else { 2 };
                let key = (lo.0, lo.1, lo.2, axis);
                idx[k] = match verts.get(&key) {
                    Some(v) => *v,
                    None => {
                        let (da, db) = (sd[a] as f64, sd[b] as f64);
                        let t = (da / (da - db)).clamp(0.0, 1.0); // signs differ, so da - db != 0
                        let v = s.positions.len() as u32;
                        s.positions.push(pos(ga).lerp(pos(gb), t));
                        s.weights.push(wt[a] + (wt[b] - wt[a]) * t as f32);
                        s.normals.push(grad(ga).lerp(grad(gb), t).normalize().unwrap_or(Vec3::ZERO));
                        verts.insert(key, v);
                        v
                    }
                };
            }
            s.triangles.push(idx);
        }
    }
    s
}

/// March every allocated voxel of the volume as a cube origin.
pub fn march_tsdf(t: &Tsdf) -> Surface {
    let mut origins = Vec::new();
    t.for_each_voxel(|g, v| {
        if v.weight > 0.0 {
            origins.push(g);
        }
    });
    march_cells(origins.into_iter(), |g| t.voxel(g).map(|v| (v.sdf, v.weight)), t.voxel_mm())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::Mesh;

    /// Bourke's published edgeTable, its first sixteen entries.
    const EDGE_TABLE_HEAD: [u16; 16] = [
        0x000, 0x109, 0x203, 0x30a, 0x406, 0x50f, 0x605, 0x70c, 0x80c, 0x905, 0xa0f, 0xb06, 0xc0a, 0xd03, 0xe09, 0xf00,
    ];

    #[test]
    fn edge_table_matches_bourke() {
        for (c, m) in EDGE_TABLE_HEAD.iter().enumerate() {
            assert_eq!(edge_mask(c as u8), *m, "case {c}");
        }
    }

    #[test]
    fn table_cases() {
        let t = Table::build();
        assert!(t.tris(0).is_empty());
        assert!(t.tris(255).is_empty());
        assert_eq!(t.tris(1), &[[0, 3, 8]]);
        for case in 0..=255u8 {
            let mask = edge_mask(case);
            let mut used = 0u16;
            for tri in t.tris(case) {
                for e in tri {
                    used |= 1 << e;
                }
            }
            assert_eq!(used, mask, "case {case}: every crossing edge carries a vertex, no other edge does");
        }
    }

    #[test]
    fn sphere_is_closed_and_outward() {
        let r = 10.0f64;
        let n = 14;
        let sample = |g: Key| {
            let (x, y, z) = (g.0 as f64, g.1 as f64, g.2 as f64);
            Some(((libm::sqrt(x * x + y * y + z * z) - r) as f32, 1.0f32))
        };
        let mut origins = Vec::new();
        for z in -n..n {
            for y in -n..n {
                for x in -n..n {
                    origins.push((x, y, z));
                }
            }
        }
        let s = march_cells(origins.into_iter(), sample, 1.0);
        let m = Mesh { positions: s.positions.clone(), triangles: s.triangles.clone() };
        assert_eq!(m.boundary_loops(), Ok(Vec::new()), "closed manifold");
        assert_eq!(m.euler_characteristic(), 2);
        for p in &s.positions {
            assert!((p.mag() - r).abs() < 1.0, "vertex {p:?} off the sphere");
        }
        let mut vol = 0.0;
        for t in &s.triangles {
            let (a, b, c) = (s.positions[t[0] as usize], s.positions[t[1] as usize], s.positions[t[2] as usize]);
            vol += a.dot(b.cross(c)) / 6.0;
        }
        let exact = 4.0 / 3.0 * core::f64::consts::PI * r * r * r;
        assert!(vol > 0.0, "outward orientation");
        assert!((vol - exact).abs() / exact < 0.03, "volume {vol} vs {exact}");
        for (p, nrm) in s.positions.iter().zip(&s.normals) {
            assert!(nrm.dot(p.normalize().unwrap()) > 0.9, "normal points out");
        }
        assert!(s.weights.iter().all(|w| *w == 1.0));
    }
}
