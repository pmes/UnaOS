// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! The truncated signed distance volume (TSDF) and the integration of one depth frame.
//!
//! Written from Curless & Levoy, "A Volumetric Method for Building Complex Models from Range Images" (SIGGRAPH 1996:
//! the cumulative weighted signed distance) in KinectFusion's projective form (Newcombe et al., ISMAR 2011: a voxel's
//! signed distance is the measured depth of the pixel it projects to minus the voxel's own camera depth, truncated to
//! ±trunc, skipped when more than `trunc` behind the surface; the running average `D = (W·D + d) / (W + 1)`,
//! `W = min(W + 1, W_MAX)`), stored as Open3D's VoxelBlockGrid stores it: fixed 8³ blocks allocated along each ray's
//! ±trunc band ("block activation") before the voxels are integrated, so a 2 m cube at 5 mm (400³ = 64M voxels dense)
//! holds only its surface band.
//!
//! VUGSCANTSDFOUTLIERS (SR555): with [`Tsdf::with_edge_mm`] a pixel on a depth edge (its 3x3 neighbourhood spans more
//! than [`Tsdf::edge_mm`]) integrates nothing that frame — see [`depth_edge`]; off by default (the seat's ruling: at
//! scan noise it costs more averaging than it removes, VUG-SCAN-TSDF-EDGEFAR owed).
//!
//! Sign: positive in front of the surface (free space, toward the camera), negative behind it (inside the solid).
//! Voxel `g` (integer) sits at `g · voxel_mm` in the world. The pinhole model is probe_core's (`Intrinsics::project`,
//! `Intrinsics::deproject`, `depth::metres_at`), called at the seam in its f32 metres and widened to f64 mm after.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet};

use probe_core::depth::metres_at;
use probe_core::{DepthScale, Frame, Intrinsics, PixelFormat};

use crate::ScanError;
use crate::geom::{Tol, Transform, Vec3};

/// Voxels per block edge (Open3D's default block resolution is 8 / 16; 8 keeps a thin band cheap).
pub const BLOCK: i32 = 8;
const BLOCK_VOXELS: usize = (BLOCK * BLOCK * BLOCK) as usize;
/// The cap on a voxel's observation weight (KinectFusion's W_max class: old frames keep a voice, new ones still move it).
pub const W_MAX: f32 = 64.0;
/// The default block budget: 65 536 blocks = 32M voxels, a 256 MiB ceiling at 8 bytes a voxel.
pub const DEFAULT_BUDGET: usize = 65_536;

/// One voxel: the fused signed distance (mm, within ±trunc) and its observation weight. The weight IS the voxel's
/// confidence (BP-vug-5): uniform weight 1 per frame that observed it, capped at [`W_MAX`]; 0 = never observed. With the
/// depth-edge guard on ([`Tsdf::with_edge_mm`]), a frame whose pixel at the voxel is a depth edge does not observe it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Voxel {
    pub sdf: f32,
    pub weight: f32,
}

impl Voxel {
    pub const EMPTY: Voxel = Voxel { sdf: 0.0, weight: 0.0 };
}

/// What one integration did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Integrated {
    pub blocks_new: u32,
    pub voxels_updated: u32,
}

type Key = (i32, i32, i32);

/// The volume. Owned by the caller and mutated only through `&mut` (no statics, no locks: BP-vug-4).
#[derive(Clone, Debug)]
pub struct Tsdf {
    voxel_mm: f64,
    trunc_mm: f64,
    tol: Tol,
    budget: usize,
    /// VUGSCANTSDFOUTLIERS (SR555): the depth jump across a pixel's 3x3 neighbourhood above which it integrates nothing
    /// (`f64::INFINITY`, the default: the guard off).
    edge_mm: f64,
    blocks: BTreeMap<Key, Box<[Voxel; BLOCK_VOXELS]>>,
    frames: u32,
    intrinsics: Option<Intrinsics>,
}

fn block_of(g: Key) -> (Key, usize) {
    let b = (g.0.div_euclid(BLOCK), g.1.div_euclid(BLOCK), g.2.div_euclid(BLOCK));
    let (lx, ly, lz) = (g.0.rem_euclid(BLOCK), g.1.rem_euclid(BLOCK), g.2.rem_euclid(BLOCK));
    (b, (lx + BLOCK * (ly + BLOCK * lz)) as usize)
}

fn global_of(b: Key, idx: usize) -> Key {
    let i = idx as i32;
    (b.0 * BLOCK + i % BLOCK, b.1 * BLOCK + (i / BLOCK) % BLOCK, b.2 * BLOCK + i / (BLOCK * BLOCK))
}

impl Tsdf {
    /// A volume of `voxel_mm` voxels and a ±`trunc_mm` band. Refused: a voxel not finite or not above `tol.lin`
    /// ([`ScanError::BadVoxel`]); a band under two voxels ([`ScanError::BadTrunc`]: the zero crossing needs a sample
    /// on each side).
    pub fn new(voxel_mm: f64, trunc_mm: f64, tol: Tol) -> Result<Tsdf, ScanError> {
        if !voxel_mm.is_finite() || voxel_mm <= tol.lin {
            return Err(ScanError::BadVoxel { voxel_mm });
        }
        if !trunc_mm.is_finite() || trunc_mm < 2.0 * voxel_mm {
            return Err(ScanError::BadTrunc { trunc_mm, voxel_mm });
        }
        Ok(Tsdf {
            voxel_mm,
            trunc_mm,
            tol,
            budget: DEFAULT_BUDGET,
            edge_mm: f64::INFINITY, // VUGSCANTSDFOUTLIERS: the depth-edge guard off unless `with_edge_mm` arms it
            blocks: BTreeMap::new(),
            frames: 0,
            intrinsics: None,
        })
    }

    /// The same volume with a block budget of `blocks` (the memory ceiling is `blocks · 512 · 8` bytes).
    pub fn with_budget(mut self, blocks: usize) -> Tsdf {
        self.budget = blocks;
        self
    }

    pub fn voxel_mm(&self) -> f64 {
        self.voxel_mm
    }

    pub fn trunc_mm(&self) -> f64 {
        self.trunc_mm
    }

    pub fn tol(&self) -> Tol {
        self.tol
    }

    pub fn budget(&self) -> usize {
        self.budget
    }

    /// Frames integrated.
    pub fn frames(&self) -> u32 {
        self.frames
    }

    /// Blocks allocated.
    pub fn blocks(&self) -> usize {
        self.blocks.len()
    }

    /// The first frame's intrinsics (every later frame must match them).
    pub fn intrinsics(&self) -> Option<Intrinsics> {
        self.intrinsics
    }

    /// The voxel at integer index `g` (`None` when its block is not allocated).
    pub fn voxel(&self, g: (i32, i32, i32)) -> Option<Voxel> {
        let (b, i) = block_of(g);
        self.blocks.get(&b).map(|blk| blk[i])
    }

    /// The world position (mm) of voxel `g`.
    pub fn position(&self, g: (i32, i32, i32)) -> Vec3 {
        Vec3::new(g.0 as f64 * self.voxel_mm, g.1 as f64 * self.voxel_mm, g.2 as f64 * self.voxel_mm)
    }

    /// Every allocated voxel index, block by block (the march's cube origins).
    pub(crate) fn for_each_voxel(&self, mut f: impl FnMut((i32, i32, i32), Voxel)) {
        for (b, blk) in &self.blocks {
            for (i, v) in blk.iter().enumerate() {
                f(global_of(*b, i), *v);
            }
        }
    }

    fn key_of(&self, p: Vec3) -> Option<Key> {
        let s = 1.0 / self.voxel_mm;
        let (x, y, z) = (libm::floor(p.x * s + 0.5), libm::floor(p.y * s + 0.5), libm::floor(p.z * s + 0.5));
        let lim = (i32::MAX / 2) as f64;
        if !(x.abs() < lim && y.abs() < lim && z.abs() < lim) {
            return None; // not finite, or off the index range
        }
        Some(block_of((x as i32, y as i32, z as i32)).0)
    }

    /// Integrate one Z16 frame seen from `pose` (camera → world, mm; camera axes are probe_core's: +x right,
    /// +y down, +z forward). Atomic: the frame is validated, its blocks are activated into a set, the budget and the
    /// overlap are checked, and only then is the volume touched — a refusal leaves it exactly as it was.
    pub fn integrate(
        &mut self,
        frame: &Frame,
        scale: DepthScale,
        intr: &Intrinsics,
        pose: &Transform,
    ) -> Result<Integrated, ScanError> {
        if frame.format != PixelFormat::Z16 {
            return Err(ScanError::NotDepth { format: frame.format });
        }
        if frame.width != intr.width || frame.height != intr.height {
            return Err(ScanError::SizeMismatch {
                frame: (frame.width, frame.height),
                intrinsics: (intr.width, intr.height),
            });
        }
        if intr.fx == 0.0 || intr.fy == 0.0 {
            return Err(ScanError::NoFocal);
        }
        if let Some(first) = self.intrinsics {
            if first != *intr {
                return Err(ScanError::IntrinsicsChanged);
            }
        }

        // Block activation: the ray's ±trunc band around every valid pixel's depth, sampled at most half a block apart.
        let block_mm = self.voxel_mm * BLOCK as f64;
        let steps = libm::ceil(2.0 * self.trunc_mm / (0.5 * block_mm)) as i32;
        let steps = steps.max(1);
        let mut active: BTreeSet<Key> = BTreeSet::new();
        let mut valid = 0u32;
        for y in 0..frame.height {
            for x in 0..frame.width {
                let Some(m) = metres_at(frame, scale, x, y) else { continue };
                valid += 1;
                let d_mm = m as f64 * 1000.0;
                for s in 0..=steps {
                    let z_mm = d_mm - self.trunc_mm + 2.0 * self.trunc_mm * s as f64 / steps as f64;
                    if z_mm <= 0.0 {
                        continue;
                    }
                    let Some(c) = intr.deproject(x as f32 + 0.5, y as f32 + 0.5, (z_mm / 1000.0) as f32) else {
                        continue;
                    };
                    let p = Vec3::new(c[0] as f64 * 1000.0, c[1] as f64 * 1000.0, c[2] as f64 * 1000.0);
                    if let Some(k) = self.key_of(pose.apply_point(p)) {
                        active.insert(k);
                    }
                }
            }
        }
        if valid == 0 || active.is_empty() {
            return Err(ScanError::NoDepth);
        }
        let new = active.iter().filter(|k| !self.blocks.contains_key(k)).count();
        if self.frames > 0 && new == active.len() {
            return Err(ScanError::NoOverlap);
        }
        let need = self.blocks.len() + new;
        if need > self.budget {
            return Err(ScanError::VoxelTooFine { need_blocks: need, budget: self.budget });
        }

        // Mutate: allocate, then project every voxel of every active block and fold its signed distance in.
        for k in &active {
            self.blocks.entry(*k).or_insert_with(|| Box::new([Voxel::EMPTY; BLOCK_VOXELS]));
        }
        let inv = pose.invert().unwrap_or(Transform::IDENTITY);
        let (voxel_mm, trunc) = (self.voxel_mm, self.trunc_mm);
        let edge = depth_edges(frame, scale, self.edge_mm); // VUGSCANTSDFOUTLIERS: empty when the guard is off
        let mut updated = 0u32;
        for k in &active {
            let Some(blk) = self.blocks.get_mut(k) else { continue };
            for (i, vx) in blk.iter_mut().enumerate() {
                let g = global_of(*k, i);
                let p = Vec3::new(g.0 as f64 * voxel_mm, g.1 as f64 * voxel_mm, g.2 as f64 * voxel_mm);
                let q = inv.apply_point(p);
                if q.z <= 0.0 {
                    continue;
                }
                let Some(uv) = intr.project([(q.x / 1000.0) as f32, (q.y / 1000.0) as f32, (q.z / 1000.0) as f32])
                else {
                    continue;
                };
                if !(uv[0] >= 0.0 && uv[1] >= 0.0) {
                    continue;
                }
                let (px, py) = (uv[0] as u32, uv[1] as u32); // floor: pixel x covers u in [x, x + 1)
                if edge.get(py as usize * frame.width as usize + px as usize) == Some(&true) {
                    continue; // VUGSCANTSDFOUTLIERS: a depth-edge pixel cannot speak for a voxel within one pixel of it
                }
                let Some(m) = metres_at(frame, scale, px, py) else { continue };
                let sdf = m as f64 * 1000.0 - q.z;
                if sdf < -trunc {
                    continue; // unseen: behind the surface by more than the band
                }
                let d = sdf.min(trunc) as f32;
                vx.sdf = (vx.sdf * vx.weight + d) / (vx.weight + 1.0);
                vx.weight = (vx.weight + 1.0).min(W_MAX);
                updated += 1;
            }
        }
        self.frames += 1;
        self.intrinsics = Some(*intr);
        Ok(Integrated { blocks_new: new as u32, voxels_updated: updated })
    }
}

/// The brief's free-function spelling of [`Tsdf::integrate`].
pub fn integrate(
    t: &mut Tsdf,
    frame: &Frame,
    scale: DepthScale,
    intr: &Intrinsics,
    pose: &Transform,
) -> Result<Integrated, ScanError> {
    t.integrate(frame, scale, intr, pose)
}

// VUGSCANPOSE (SR478): the one integrate-with-tracking convenience. The first frame goes in at the tracker's pose
// (the world's origin); every later frame is tracked against the model raycast after the previous one, then
// integrated, then the model is raycast again. `integrate` itself is unchanged. A refused track touches neither the
// volume nor the tracker; a refused integrate leaves the volume as it was (the tracker keeps the tracked pose
// against its old model).
impl Tsdf {
    pub fn integrate_tracked(
        &mut self,
        tracker: &mut crate::pose::Tracker,
        frame: &Frame,
        scale: DepthScale,
        intr: &Intrinsics,
        gravity: Option<Vec3>,
    ) -> Result<(Transform, Integrated), crate::pose::TrackError> {
        let pose = if self.frames == 0 { tracker.pose() } else { tracker.track(frame, scale, intr, gravity)? };
        let done = self.integrate(frame, scale, intr, &pose).map_err(crate::pose::TrackError::Scan)?;
        tracker.accept(self, intr, pose, gravity);
        Ok((pose, done))
    }
}

// VUGSCANTSDFOUTLIERS (SR555): the depth-edge guard. A voxel takes the depth of the pixel its centre floors into
// (`integrate`); within one pixel of a silhouette that pixel can belong to the OTHER surface (a free-space voxel
// beside the post takes the post's depth, an inside voxel beside an edge takes the ground behind it), and each
// camera's silhouette writes that error along the surface. A pixel whose 3x3 neighbourhood spans more than
// `edge_mm` of depth cannot speak for a voxel within one pixel of it, so with the guard armed it integrates nothing
// that frame, exactly as an unseen voxel; an adjacent-pixel step above the truncation is also the grazing case
// (beyond ~77° at 600 mm on the bench intrinsics). The weight stays the observation count (BP-vug-5). MEASURED, not
// a default (SR555 RUN 1, the seat's ruling (a)): armed at the truncation it cut the noise-free post's cylindricity
// 3.214 -> 2.775 mm but raised it 3.542 -> 4.016 at D435i noise (fewer frames averaged per voxel near a silhouette)
// and left the box top's 1.217 mm unchanged (that is the fixture's depth quantisation, VUG-SCAN-FIXTURE-QUANT), so it
// is off by default and `tests/gdt.rs` `tsdf_outliers_probe` arms it; the far-side-only guard is VUG-SCAN-TSDF-EDGEFAR.
impl Tsdf {
    /// The same volume with the depth-edge guard armed at the jump `mm` (`trunc_mm` is the measured setting);
    /// `f64::INFINITY` (the default) leaves it off.
    pub fn with_edge_mm(mut self, mm: f64) -> Tsdf {
        self.edge_mm = mm;
        self
    }

    /// The depth jump across a pixel's 3x3 neighbourhood above which the pixel is a depth edge.
    pub fn edge_mm(&self) -> f64 {
        self.edge_mm
    }
}

/// Whether pixel `(x, y)` of a Z16 frame is a depth edge: its valid depth and the valid depths of its 3x3
/// neighbourhood span more than `jump_mm`. A hole (raw 0) is neutral; a pixel with no depth of its own is no edge.
pub fn depth_edge(frame: &Frame, scale: DepthScale, x: u32, y: u32, jump_mm: f64) -> bool {
    let Some(c) = metres_at(frame, scale, x, y) else { return false };
    let (mut lo, mut hi) = (c, c);
    for dy in -1i64..=1 {
        for dx in -1i64..=1 {
            let (nx, ny) = (x as i64 + dx, y as i64 + dy);
            if nx < 0 || ny < 0 || nx >= frame.width as i64 || ny >= frame.height as i64 {
                continue;
            }
            if let Some(m) = metres_at(frame, scale, nx as u32, ny as u32) {
                lo = lo.min(m);
                hi = hi.max(m);
            }
        }
    }
    (hi - lo) as f64 * 1000.0 > jump_mm
}

/// The frame's depth-edge mask, row-major (`y · width + x`); empty when `jump_mm` is not finite (the guard off).
fn depth_edges(frame: &Frame, scale: DepthScale, jump_mm: f64) -> alloc::vec::Vec<bool> {
    if !jump_mm.is_finite() {
        return alloc::vec::Vec::new();
    }
    let mut out = alloc::vec::Vec::with_capacity(frame.width as usize * frame.height as usize);
    for y in 0..frame.height {
        for x in 0..frame.width {
            out.push(depth_edge(frame, scale, x, y, jump_mm));
        }
    }
    out
}
