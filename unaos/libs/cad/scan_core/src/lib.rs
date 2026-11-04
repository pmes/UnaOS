// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU Lesser General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU Lesser General Public License for more details.
//
// You should have received a copy of the GNU Lesser General Public License
// along with this program.  If not, see <https://www.gnu.org/licenses/>.

//! # scan_core — a scan is a measurement
//!
//! CHARTER: Vug (CODEX §2, 3D), seam shared-core — a scan is a measurement: probe_core depth frames with given poses
//! fused into a TSDF, extracted as a mesh_core mesh with per-vertex confidence, and planes / cylinders fitted with
//! their residuals; no bus, no device.
//!
//! VUGSCAN1 (LEDGER SR469). Sonde owns the frames (`probe_core`, R112); this crate consumes them and never the bus
//! (docs/dev/blueprint/vug.md §3). BP-vug-5: the mesh carries per-vertex confidence and the probe's intrinsics, and a
//! fitted shape carries its residuals; nothing here promotes a scan to design geometry.
//!
//! | module | what it does | written from |
//! |---|---|---|
//! | [`tsdf`] | the block-sparse truncated signed distance volume and one frame's integration | Curless & Levoy 1996; KinectFusion (Newcombe et al. 2011); Open3D's VoxelBlockGrid |
//! | [`mc`] | marching cubes over the volume, one vertex per voxel edge, confidence and normals carried | Lorensen & Cline 1987; Bourke 1994 numbering |
//! | [`fit`] | plane and cylinder RANSAC with residuals | Schnabel, Wahl & Klein 2007; Fischler & Bolles 1981 |
//! | [`graph`] | keyframes, the pose graph and loop closure over the tracked poses (VUGSCANLOOP SR500) | Kintinuous / ElasticFusion (Whelan et al.); g2o (Kümmerle et al. 2011) |
//! | [`deviation`] | the scan registered to the design (point-to-plane ICP over mesh_core's `Bvh::closest`) and its signed deviation, stats and faces ranked (VUGSCANDEVIATION SR510) | Chen & Medioni 1992; Low 2004 |
//! | [`fixture`] | synthetic Z16 frames of a known scene through probe_core's pinhole model | the inverse of `Intrinsics::deproject` |
//! | [`fuse`] | THE fuse: shots on a pose road into one TSDF, a `.prb` recording fused into a mesh (VUGFACEDEVIATIONPRB SR610) | tools/scan's composition, lifted |
//!
//! Units: millimetres in the world (BP-vug-2's `Tol` is linear in mm), metres only at probe_core's seam.

#![no_std]
#![forbid(unsafe_code)]

extern crate alloc;

pub mod deviation;
pub mod fit;
pub mod fixture;
mod geom;
pub mod graph;
pub mod mc;
mod mesh;
pub mod pose;
pub mod tsdf;

use alloc::vec::Vec;

pub use deviation::{AlignParams, Aligned, DESIGN_CHORD_MM, Design, DesignError, Deviation, DeviationError, FaceReport, Shade, Stats, align, colour, design_of, deviations, measure, shade};
pub use fit::{Cone, Cylinder, Fit, FitError, Floor, Primitive, Shape, Sphere, detect, fit_cylinder, fit_plane};
pub use geom::{Plane, Tol, Transform, Vec3};
pub use graph::{
    Candidate, Closed, Edge, EdgeKind, Gap, GraphError, Keyframe, Keyframes, Loop, LoopParams, PoseGraph, Refused,
    Reintegrate, Shot, Solved, close_loop, gap, match_keyframe, reintegrate,
};
pub use mesh::{Mesh, Topo};
pub use pose::{ModelMap, Surfel, TrackError, TrackParams, Tracked, Tracker, raycast};
pub use probe_core::{DepthScale, Frame, Intrinsics, PixelFormat};
pub use tsdf::{Integrated, Tsdf, Voxel, integrate};
// VUGSCANGDT (LEDGER SR542): the deviation read as ISO 1101 flatness / cylindricity / position per design face.
pub mod gdt;
pub use gdt::{FaceGdt, GDT_MIN_POINTS, GDT_NORMAL_DEG, Kind, Nominal, Verdict, gdt, nominal, position_mm};
// VUGFACEDEVIATIONPRB (LEDGER SR610): THE fuse (tools/scan's composition lifted) and a .prb recording fused by it.
pub mod fuse;

/// Why a frame, a volume or an extraction was refused. Every refusal leaves the volume as it was.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScanError {
    /// The frame is not Z16 depth.
    NotDepth { format: PixelFormat },
    /// The frame's size is not the intrinsics' size.
    SizeMismatch { frame: (u32, u32), intrinsics: (u32, u32) },
    /// A focal length is zero (a table never filled).
    NoFocal,
    /// A later frame's intrinsics differ from the first frame's: the mesh carries ONE probe's intrinsics (BP-vug-5).
    IntrinsicsChanged,
    /// The frame has no valid depth pixel.
    NoDepth,
    /// A frame after the first touches no block the volume already holds; nothing was integrated.
    NoOverlap,
    /// The frame would grow the volume past its block budget (the voxel is too fine for it); nothing was integrated.
    VoxelTooFine { need_blocks: usize, budget: usize },
    /// The voxel size is not finite or not above the tolerance.
    BadVoxel { voxel_mm: f64 },
    /// The truncation band is narrower than two voxels.
    BadTrunc { trunc_mm: f64, voxel_mm: f64 },
    /// Extraction before any frame was integrated.
    NoFrames,
    /// Extraction found no zero crossing.
    NoSurface,
}

/// The extracted scan: a mesh with, per vertex, the TSDF normal and the confidence (the fused observation weight:
/// 1.0 for a single frame, up to the frame count), the probe's intrinsics and how many frames were fused.
#[derive(Clone, Debug, PartialEq)]
pub struct ScanMesh {
    pub mesh: Mesh,
    pub normals: Vec<Vec3>,
    pub confidence: Vec<f32>,
    pub intrinsics: Intrinsics,
    pub frames: u32,
}

/// Marching cubes over every allocated, observed cube of `t`.
pub fn extract(t: &Tsdf) -> Result<ScanMesh, ScanError> {
    let intrinsics = match (t.frames(), t.intrinsics()) {
        (0, _) | (_, None) => return Err(ScanError::NoFrames),
        (_, Some(i)) => i,
    };
    let s = mc::march_tsdf(t);
    if s.triangles.is_empty() {
        return Err(ScanError::NoSurface);
    }
    Ok(ScanMesh {
        mesh: Mesh { positions: s.positions, triangles: s.triangles },
        normals: s.normals,
        confidence: s.weights,
        intrinsics,
        frames: t.frames(),
    })
}
