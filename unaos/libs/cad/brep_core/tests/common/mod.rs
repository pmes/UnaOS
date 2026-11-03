//! Test helpers. `manifold_chi` is a TEST-ONLY edge-pair check (written while mesh_core was shimmed, VUGBREP1); kept at
//! VUGBREPSWAP (SR492) because mesh_core's `HalfEdge::build` adds a vertex-fan refusal these counts were never measured
//! against (VUG-BREP-TESTCHI owed).
#![allow(dead_code)]

use brep_core::{Mesh, Vec3};
use brep_core::{Brep, Entity, Tessellation};
use std::collections::{HashMap, HashSet};

/// Every directed edge exactly once and its reverse present (closed, consistently oriented, edge-manifold);
/// returns chi = V - E + F over the referenced vertices.
pub fn manifold_chi(m: &Mesh) -> Result<i64, String> {
    let mut dir: HashMap<(u32, u32), u32> = HashMap::new();
    for t in &m.triangles {
        for k in 0..3 {
            let (a, b) = (t[k], t[(k + 1) % 3]);
            if a == b {
                return Err(format!("degenerate triangle {t:?}"));
            }
            *dir.entry((a, b)).or_insert(0) += 1;
        }
    }
    for (&(a, b), &c) in &dir {
        if c != 1 {
            return Err(format!("directed edge {a}->{b} used {c} times"));
        }
        if !dir.contains_key(&(b, a)) {
            return Err(format!("boundary edge {a}->{b}"));
        }
    }
    let used: HashSet<u32> = m.triangles.iter().flatten().copied().collect();
    Ok(used.len() as i64 - dir.len() as i64 / 2 + m.triangles.len() as i64)
}

/// The mesh's signed volume (positive when the triangles face outward).
pub fn mesh_volume(m: &Mesh) -> f64 {
    m.triangles
        .iter()
        .map(|t| {
            let (a, b, c) = (m.positions[t[0] as usize], m.positions[t[1] as usize], m.positions[t[2] as usize]);
            a.dot(b.cross(c)) / 6.0
        })
        .sum()
}

pub fn rel(a: f64, b: f64) -> f64 {
    ((a - b) / b).abs()
}

pub fn close(a: f64, b: f64, what: &str) {
    assert!(rel(a, b) < 1e-9, "{what}: {a} vs {b} (rel {:e})", rel(a, b));
}

/// The largest distance from a triangle sample to its face's exact surface, sampled on a barycentric grid of 8.
pub fn max_deviation(b: &Brep, t: &Tessellation) -> f64 {
    let n = 8;
    let mut worst: f64 = 0.0;
    for (ti, tri) in t.mesh.triangles.iter().enumerate() {
        let Some(Entity::Face(fi)) = b.find(t.face_of[ti]) else { panic!("face_of[{ti}] is not a face") };
        let s = &b.faces[fi as usize].surface;
        let p: Vec<Vec3> = tri.iter().map(|&i| t.mesh.positions[i as usize]).collect();
        for i in 0..=n {
            for j in 0..=(n - i) {
                let (u, v) = (i as f64 / n as f64, j as f64 / n as f64);
                let q = p[0] * (1.0 - u - v) + p[1] * u + p[2] * v;
                worst = worst.max(s.distance(q));
            }
        }
    }
    worst
}
