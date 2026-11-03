// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vug (CODEX §2, 3D), a host face — the profile -> Brep -> Mesh road of the sketch face (VUGFACE2, LEDGER
//! SR485): brep_core's `extrude`, `tessellate` and `volume`; holds no geometry of its own.
//!
//! The ONE file of vug-view that names brep_core. A sketch_core `Profile` placed by the sketch's `Frame` is extruded
//! along the frame's normal by a signed distance; every Brep of the design is tessellated (a stated chord) into ONE
//! mesh that enters the view through model.rs's `Model::from_triangles` (so mesh_core stays named in model.rs alone);
//! the Breps are kept beside it, with the per-triangle face map a sketch-on-face pick reads.
//!
//! brep_core takes sketch_core's `Profile` itself since VUGBREPSWAP (SR492: its `dep.rs` shims are gone), so a profile
//! passes straight through; [`Body::from_breps`] copies the tessellation's positions into model.rs's `[f64; 3]` seam.

use crate::model::Model;
use brep_core::{Brep, Entity, Frame, Id, Refusal, Surface};
use sketch_core::Profile;
use std::fmt;
use std::sync::Arc;

/// Extrude `p` (sketch coordinates of `plane`) along the plane's normal by `distance` mm; a negative distance goes
/// against the normal (brep_core mirrors the working frame below the plane).
pub fn extrude(p: &Profile, plane: &Frame, distance: f64, tol: brep_core::Tol) -> Result<Brep, Refusal> {
    let dir = if distance < 0.0 { -plane.z } else { plane.z };
    brep_core::extrude(p, plane, dir, distance.abs(), tol)
}

/// The chord a Brep is tessellated to: a thousandth of its vertex bounds' diagonal, never under 1e-3 mm.
fn chord_for(b: &Brep) -> f64 {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for v in &b.vertices {
        let p = [v.p.x, v.p.y, v.p.z];
        for k in 0..3 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let d = (0..3).map(|k| (hi[k] - lo[k]).max(0.0).powi(2)).sum::<f64>().sqrt();
    if d.is_finite() { (d * 1e-3).max(1e-3) } else { 1e-3 }
}

/// The design's solids: the Breps (side by side, unmerged: booleans are VUG-BREP-2), the face each drawn triangle
/// came from (`(brep index, face Id)`), and the ONE model the view draws in place of the opened mesh.
#[derive(Clone)]
pub struct Body {
    pub breps: Vec<Brep>,
    pub face_of: Vec<(u32, Id)>,
    pub model: Arc<Model>,
}

impl fmt::Debug for Body {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Body").field("breps", &self.breps.len()).field("triangles", &self.model.triangle_count()).finish()
    }
}

impl Body {
    /// Tessellate every Brep (brep_core's `tessellate`, its chord from `chord_for`) into one model.
    pub fn from_breps(breps: Vec<Brep>, tol: brep_core::Tol) -> Result<Body, String> {
        let mut pos: Vec<[f64; 3]> = Vec::new();
        let mut tris: Vec<[u32; 3]> = Vec::new();
        let mut face_of = Vec::new();
        for (k, b) in breps.iter().enumerate() {
            let t = brep_core::tessellate(b, chord_for(b), tol).map_err(|r| format!("tessellate: {r:?}"))?;
            let base = pos.len() as u32;
            pos.extend(t.mesh.positions.iter().map(|p| [p.x, p.y, p.z]));
            tris.extend(t.mesh.triangles.iter().map(|tr| [tr[0] + base, tr[1] + base, tr[2] + base]));
            face_of.extend(t.face_of.iter().map(|&id| (k as u32, id)));
        }
        let model = Model::from_triangles(&pos, &tris)?;
        Ok(Body { breps, face_of, model: Arc::new(model) })
    }

    /// The planar face a drawn triangle belongs to, as a sketch frame whose normal points out of the solid (the
    /// surface frame, turned over when the face is reversed); `None` for a curved face or a triangle out of range.
    pub fn face_frame(&self, tri: u32) -> Option<Frame> {
        let (k, id) = *self.face_of.get(tri as usize)?;
        let b = self.breps.get(k as usize)?;
        let Some(Entity::Face(i)) = b.find(id) else { return None };
        let face = b.faces.get(i as usize)?;
        match face.surface {
            Surface::Plane { f } => Some(if face.reversed { Frame { o: f.o, x: f.x, y: -f.y, z: -f.z } } else { f }),
            _ => None,
        }
    }

    /// The summed volume of the Breps (brep_core's closed-form `volume`).
    pub fn volume(&self) -> Result<f64, Refusal> {
        self.breps.iter().map(brep_core::volume).sum()
    }
}

/// An extrude in progress (`e`): the profile, its plane, the centroid it is pulled at (world mm), the signed distance
/// so far, the press point, and the preview body (every Brep of the design plus this one).
#[derive(Clone, Debug)]
pub struct Pull {
    pub profile: Profile,
    pub plane: Frame,
    pub centroid: [f64; 3],
    pub distance: f64,
    pub start: (f64, f64),
    pub preview: Option<Body>,
}
