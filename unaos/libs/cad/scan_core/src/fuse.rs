// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Vug (CODEX §2, 3D), seam shared-core — THE fuse: depth shots on a pose road into one TSDF, and a `.prb`
//! recording (probe_core's) fused by it into a [`ScanMesh`]; no bus, no device, no I/O (bytes in, BP-vug-4).
//!
//! VUGFACEDEVIATIONPRB (LEDGER SR610): tools/scan composed the fuse in its own run (the posed integrate loop, its
//! `fuse_tracked` and `close_tracked_loop`); those arms are lifted here verbatim, in their order, so tools/scan and
//! vug-view fuse through ONE road. The tool keeps its report lines, formatted from the [`TrackLog`] this returns.
//! [`fuse_prb`] is the face's road: a recording carries no pose, so one depth frame is fused at the identity and
//! several through the tracker (tools/scan's `--track` road), at tools/scan's defaults ([`VOXEL_MM`],
//! [`TRUNC_VOXELS`]). A tracked scan's world is frame 0's camera (a coarse start against a design is owed:
//! VUG-SCAN-DEVIATION-COARSE).

use alloc::vec::Vec;

pub use probe_core::recording::{self, Recording, RecordingError};

use crate::graph::{GraphError, Keyframes, Loop, LoopParams, Reintegrate, Shot, close_loop, reintegrate};
use crate::pose::{TrackError, Tracked, Tracker};
use crate::tsdf::{Knobs, Tsdf};
use crate::{DepthScale, Frame, Intrinsics, ScanError, ScanMesh, Tol, Transform, extract};

/// The voxel edge (mm) when nothing states one: tools/scan's default `--voxel`.
pub const VOXEL_MM: f64 = 4.0;
/// The truncation band in voxels when nothing states one: tools/scan's default `--trunc` is this times the voxel.
pub const TRUNC_VOXELS: f64 = 3.0;

/// Where each shot's camera-to-world pose comes from.
#[derive(Clone, Copy, Debug)]
pub enum Road<'a> {
    /// One given pose per shot.
    Posed(&'a [Transform]),
    /// Shot 0 at the identity, each later shot tracked against the model, then the loop closed.
    Track,
}

/// One tracked shot's outcome, in shot order.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Step {
    /// The first integrated shot, at the identity.
    Anchor,
    /// Tracked and integrated; what the tracker measured (when it kept a measurement).
    Tracked(Option<Tracked>),
    /// Skipped: it did not pair with the model.
    Lost { inliers: u32, residual_mm: f64 },
    /// Skipped: it holds no depth pixel.
    NoDepth,
}

/// The tracked road's record: every shot's step, the keyframe graph's node count, and the loop closure (or the graph's
/// refusal, the tracked volume kept).
#[derive(Clone, Debug, PartialEq)]
pub struct TrackLog {
    pub steps: Vec<Step>,
    pub nodes: usize,
    pub closed: Result<Loop, GraphError>,
}

/// The fused volume and, on the tracked road, its record.
#[derive(Clone, Debug)]
pub struct Fused {
    pub tsdf: Tsdf,
    pub track: Option<TrackLog>,
}

/// Why the fuse stopped.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FuseError {
    /// The volume's parameters were refused.
    Volume(ScanError),
    /// The volume refused shot `frame` (a posed shot, the tracked anchor, or a re-integration after a closure).
    Integrate { frame: usize, error: ScanError },
    /// The tracker refused shot `frame` for a reason that is not a skip.
    Track { frame: usize, error: TrackError },
    /// A re-integration asked for a shot the caller did not give (or a posed road with fewer poses than shots).
    NoFrame { frame: usize },
}

/// THE fuse: `Tsdf::new(voxel_mm, trunc_mm, Tol::DEFAULT)` with `knobs`, every shot integrated on `road`.
pub fn fuse(shots: &[Shot<'_>], road: Road<'_>, voxel_mm: f64, trunc_mm: f64, knobs: Knobs) -> Result<Fused, FuseError> {
    let mut t = Tsdf::new(voxel_mm, trunc_mm, Tol::DEFAULT).map_err(FuseError::Volume)?.with_knobs(knobs);
    let track = match road {
        Road::Posed(poses) => {
            for (k, s) in shots.iter().enumerate() {
                let pose = poses.get(k).ok_or(FuseError::NoFrame { frame: k })?;
                t.integrate(s.frame, s.scale, s.intr, pose).map_err(|error| FuseError::Integrate { frame: k, error })?;
            }
            None
        }
        Road::Track => Some(fuse_tracked(&mut t, shots)?),
    };
    Ok(Fused { tsdf: t, track })
}

/// The tracked road (tools/scan's VUGSCANTRACK SR493 arms): shot 0 at the identity, each later shot through
/// `Tsdf::integrate_tracked`; a `Lost` or depthless shot skipped, never integrated; then [`close_tracked_loop`].
fn fuse_tracked(t: &mut Tsdf, shots: &[Shot<'_>]) -> Result<TrackLog, FuseError> {
    let mut tr = Tracker::new(t, Tol::DEFAULT);
    let mut kf = Keyframes::new(LoopParams::for_volume(t)); // VUGSCANLOOP (SR500): the tracked poses, for close_loop
    let mut steps = Vec::with_capacity(shots.len());
    for (k, s) in shots.iter().enumerate() {
        let anchor = t.frames() == 0;
        match t.integrate_tracked(&mut tr, s.frame, s.scale, s.intr, None) {
            Ok((pose, _)) if anchor => {
                kf.observe(k as u32, pose, None);
                steps.push(Step::Anchor);
            }
            Ok((pose, _)) => {
                kf.observe(k as u32, pose, tr.last());
                steps.push(Step::Tracked(tr.last()));
            }
            Err(TrackError::Scan(error)) if anchor => return Err(FuseError::Integrate { frame: k, error }),
            Err(TrackError::Lost { inliers, residual_mm }) => steps.push(Step::Lost { inliers, residual_mm }),
            Err(TrackError::Scan(ScanError::NoDepth)) => steps.push(Step::NoDepth),
            Err(error) => return Err(FuseError::Track { frame: k, error }),
        }
    }
    let nodes = kf.nodes().len();
    let closed = close_tracked_loop(t, &kf, shots)?;
    Ok(TrackLog { steps, nodes, closed })
}

/// The tracked road's loop closure (tools/scan's VUGSCANLOOP SR500 arm): `close_loop` over the observed poses; on a
/// closure the volume is replaced by every tracked shot fused again at its corrected pose (`reintegrate`). A graph
/// scan_core refuses is returned as `Err` inside `Ok`, the tracked volume kept.
fn close_tracked_loop(t: &mut Tsdf, kf: &Keyframes, shots: &[Shot<'_>]) -> Result<Result<Loop, GraphError>, FuseError> {
    let shot = move |k: u32| shots.get(k as usize).copied();
    let lp = match close_loop(kf, t, shot) {
        Ok(lp) => lp,
        Err(e) => return Ok(Err(e)),
    };
    if lp.closures > 0 {
        *t = reintegrate(t, &lp.poses, shot).map_err(|e| match e {
            Reintegrate::Scan { frame, error } => FuseError::Integrate { frame: frame as usize, error },
            Reintegrate::NoFrame { frame } => FuseError::NoFrame { frame: frame as usize },
        })?;
    }
    Ok(Ok(lp))
}

/// A recording's depth records, each with its scale (`units` when the caller states one, else the record's own) and
/// the intrinsics it carries; `Err(index)` names the first depth record with no units (tools/scan's `load_prb` rule).
pub fn prb_depth(rec: &Recording, units: Option<DepthScale>) -> Result<Vec<(&Frame, DepthScale, Option<Intrinsics>)>, usize> {
    let mut out = Vec::new();
    for (index, r) in rec.depth().enumerate() {
        let scale = match (units, r.depth_scale()) {
            (Some(u), _) => u,
            (None, Some(s)) => s,
            (None, None) => return Err(index),
        };
        out.push((&r.frame, scale, r.intrinsics()));
    }
    Ok(out)
}

/// Why [`fuse_prb`] refused a recording.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PrbError {
    Recording(RecordingError),
    NoDepthFrames,
    NoDepthUnits { index: usize },
    NoIntrinsics { index: usize },
    Fuse(FuseError),
    Extract(ScanError),
}

/// A `.prb` recording's bytes fused into the scan mesh: each depth record at its own units and intrinsics, one record
/// at the identity and several through the tracker, at [`VOXEL_MM`] and [`TRUNC_VOXELS`] with `knobs`.
pub fn fuse_prb(bytes: &[u8], knobs: Knobs) -> Result<ScanMesh, PrbError> {
    let rec = recording::read(bytes).map_err(PrbError::Recording)?;
    let depth = prb_depth(&rec, None).map_err(|index| PrbError::NoDepthUnits { index })?;
    if depth.is_empty() {
        return Err(PrbError::NoDepthFrames);
    }
    let mut intr = Vec::with_capacity(depth.len());
    for (index, d) in depth.iter().enumerate() {
        intr.push(d.2.ok_or(PrbError::NoIntrinsics { index })?);
    }
    let shots: Vec<Shot<'_>> = depth.iter().zip(&intr).map(|(d, i)| Shot { frame: d.0, scale: d.1, intr: i }).collect();
    let identity = [Transform::IDENTITY];
    let road = if shots.len() == 1 { Road::Posed(&identity) } else { Road::Track };
    let f = fuse(&shots, road, VOXEL_MM, TRUNC_VOXELS * VOXEL_MM, knobs).map_err(PrbError::Fuse)?;
    extract(&f.tsdf).map_err(PrbError::Extract)
}
