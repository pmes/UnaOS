// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The ONE seam to mesh_core (VUGFACE1, LEDGER SR470): the only file in vug-view that names the mesh core, so the
//! face holds no geometry (BP-vug-4) and the f64 -> f32 conversion happens only through mesh_core's
//! `to_f32_buffers` (BP-vug-1), with the f32 -> f64 `Ray` of a pick the one way back. Everything else in the vessel
//! sees plain arrays. mesh_core is VUGMESH (SR466), merged from exec-rmbp-vugmesh at 3190df0af.

use mesh_core::{obj, ply, stl, Bvh, Mesh, Ray, Tol, Vec3};

/// The mesh the face draws: mesh_core's `Mesh`, its BVH, its f32 buffers and its bounds.
pub struct Model {
    mesh: Mesh,
    bvh: Bvh,
    positions: Vec<f32>,
    indices: Vec<u32>,
    lo: [f32; 3],
    hi: [f32; 3],
}

impl Model {
    /// Open an STL / OBJ / PLY file through mesh_core's readers: by extension, else by the PLY / STL magic, else OBJ.
    /// An STL is a triangle soup: it is welded (mesh_core's `weld`, `Tol::DEFAULT`) so the pick and the bounds see
    /// one vertex per corner.
    pub fn open(path: &str) -> Result<Model, String> {
        let mesh = read_mesh(path)?;
        Model::from_mesh(mesh).map_err(|e| format!("{path}: {e}"))
    }

    /// A model from positions (millimetres) and counter-clockwise (outward) triangles: the fixtures' door.
    pub fn from_triangles(positions: &[[f64; 3]], triangles: &[[u32; 3]]) -> Result<Model, String> {
        let mesh = Mesh { positions: positions.iter().map(|p| Vec3::new(p[0], p[1], p[2])).collect(), triangles: triangles.to_vec() };
        Model::from_mesh(mesh)
    }

    /// Keep the mesh, build its BVH and its f32 buffers (the one conversion).
    pub fn from_mesh(mesh: Mesh) -> Result<Model, String> {
        if mesh.triangles.is_empty() {
            return Err("no triangles".into());
        }
        let n = mesh.positions.len();
        if let Some((t, i)) = mesh.triangles.iter().enumerate().find_map(|(t, tri)| tri.iter().find(|&&i| i as usize >= n).map(|&i| (t, i))) {
            return Err(format!("triangle {t} names vertex {i} of {n}"));
        }
        if mesh.positions.iter().any(|p| !p.is_finite()) {
            return Err("a position is not finite".into());
        }
        let bb = mesh.aabb().ok_or("no positions")?;
        let buffers = mesh.to_f32_buffers(Tol::DEFAULT);
        let bvh = Bvh::build(&mesh);
        Ok(Model {
            lo: [bb.min.x as f32, bb.min.y as f32, bb.min.z as f32],
            hi: [bb.max.x as f32, bb.max.y as f32, bb.max.z as f32],
            positions: buffers.positions,
            indices: buffers.indices,
            mesh,
            bvh,
        })
    }

    /// xyz-interleaved f32 positions, one triple per vertex (mesh_core's `Buffers::positions`).
    pub fn positions(&self) -> &[f32] {
        &self.positions
    }

    /// Three vertex indices per triangle (mesh_core's `Buffers::indices`).
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    /// The bounds `(min, max)` (mesh_core's `aabb`).
    pub fn bounds(&self) -> ([f32; 3], [f32; 3]) {
        (self.lo, self.hi)
    }

    pub fn centre(&self) -> [f32; 3] {
        [0, 1, 2].map(|k| (self.lo[k] + self.hi[k]) * 0.5)
    }

    /// The radius of the bounds' sphere (never zero: a point model gets 1 mm).
    pub fn radius(&self) -> f32 {
        let d = [0, 1, 2].map(|k| self.hi[k] - self.lo[k]);
        let r = 0.5 * (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        if r > 0.0 { r } else { 1.0 }
    }

    /// The nearest triangle a ray hits (mesh_core's `Bvh::pick`): `(t, triangle)` with `t` in units of `|dir|`.
    pub fn pick(&self, origin: [f32; 3], dir: [f32; 3]) -> Option<(f32, u32)> {
        let v = |a: [f32; 3]| Vec3::new(a[0] as f64, a[1] as f64, a[2] as f64);
        let ray = Ray { origin: v(origin), dir: v(dir) };
        self.bvh.pick(&self.mesh, &ray, Tol::DEFAULT).map(|h| (h.t as f32, h.tri))
    }
}

/// VUGFACEDEVIATION (SR525): the readers of [`Model::open`] alone — an STL / OBJ / PLY file to mesh_core's `Mesh` (by
/// extension, else by the PLY / STL magic, else OBJ; an STL welded at `Tol::DEFAULT`), for a face that needs the f64
/// mesh itself (the deviation map's scan).
pub fn read_mesh(path: &str) -> Result<Mesh, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let ext = std::path::Path::new(path).extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let stl_mesh = |b: &[u8]| stl::read(b).map(|s| s.to_mesh().weld(Tol::DEFAULT).mesh).map_err(|e| format!("{path}: STL {e:?}"));
    let ply_mesh = |b: &[u8]| ply::read(b).map_err(|e| format!("{path}: PLY {e:?}"));
    let obj_mesh = |b: &[u8]| obj::read_bytes(b).map(|o| o.mesh).map_err(|e| format!("{path}: OBJ {e:?}"));
    let mesh = match ext.as_str() {
        "stl" => stl_mesh(&bytes)?,
        "ply" => ply_mesh(&bytes)?,
        "obj" => obj_mesh(&bytes)?,
        _ if bytes.starts_with(b"ply") => ply_mesh(&bytes)?,
        _ => stl_mesh(&bytes).or_else(|_| obj_mesh(&bytes))?,
    };
    Ok(mesh)
}
