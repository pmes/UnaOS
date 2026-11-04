// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! CHARTER: Vug (CODEX §2, 3D), seam shared-core — loop closure for a tracked scan: keyframes, a pose graph over
//! SE(3) and its least-squares solve, so a walk-around scan ends where it began instead of on a spiral.
//!
//! VUGSCANLOOP (LEDGER SR500). Written from Whelan et al., "Kintinuous" (2012) / "ElasticFusion" (RSS 2015) for the
//! keyframe and loop rules, and Kümmerle et al., "g2o" (ICRA 2011) / Grisetti et al., "A Tutorial on Graph-Based SLAM"
//! (2010) for the pose graph:
//!
//! 1. [`Keyframes::observe`]: frame 0 is a keyframe; a frame becomes one when its pose moved at least
//!    [`LoopParams::kf_deg`] or [`LoopParams::kf_mm`] from the last keyframe. The last observed frame is always a
//!    node ([`Keyframes::nodes`]), so the end of a session is anchored.
//! 2. [`PoseGraph`]: nodes are keyframe poses (camera → world), node 0 fixed (the gauge). An odometry edge between
//!    consecutive nodes carries the tracker's own relative pose, weighted by `1 / Σ rms² / inliers` over the tracked
//!    frames inside it. The residual of an edge `Z` from `a` to `b` is `E = Z⁻¹ ∘ a⁻¹ ∘ b` split as SO(3) x R³:
//!    `[lever · Log(R_E), t_E]`, millimetres both (the lever is the working distance).
//! 3. Loop detection ([`close_loop`]): for each node, the nearest old node at least [`LoopParams::min_gap`] nodes
//!    back whose pose is within the radius; the closure is SR478's ICP ([`crate::pose::Tracker`]) of the new node's
//!    frame against the OLD node's frame alone (integrated into a fresh one-frame volume, raycast by `refresh`), so
//!    the model is that keyframe's own geometry, not the late drifted frames fused over it ([`match_keyframe`]).
//! 4. [`PoseGraph::optimise`]: Levenberg-Marquardt over the right perturbation `T ∘ Δ(ω, τ)`, central-difference
//!    Jacobians, the normal equations dense (at most [`MAX_NODES`] nodes) by Cholesky. VUGSCANSUBMAP (SR522): past
//!    [`LoopParams::submap_after`] nodes [`PoseGraph::solve`] takes the two-level road ([`Submaps`]): the same step,
//!    each run of [`SUBMAP_NODES`] nodes' interior eliminated (Schur), the separators dense, the interiors back-solved.
//! 5. Every frame's pose is carried by its keyframe's correction; [`reintegrate`] fuses every frame again at its
//!    corrected pose into a cleared volume (all of them: BP-vug-5's confidence is the observation count).
//!
//! No truth is claimed: [`Loop`] reports the closures' disagreement with the poses ([`Gap`]) before and after the
//! solve, the only error a recording can know.

use alloc::vec;
use alloc::vec::Vec;

use geom_core::Quat;
use geom_core::linalg::{cholesky_profile, cholesky_solve as dense_solve, cholesky_substitute};
use probe_core::{DepthScale, Frame, Intrinsics};

use crate::ScanError;
use crate::geom::{Tol, Transform, Vec3};
use crate::pose::{TrackError, Tracked, Tracker};
use crate::tsdf::Tsdf;

const DEG: f64 = core::f64::consts::PI / 180.0;

/// The dense rule: the normal equations are `6(N − 1)` square, 11.4 MB and ~2.8e8 flops a factorisation at 200
/// nodes; past it [`PoseGraph::optimise`] refuses the graph by name and [`PoseGraph::solve`] takes the two-level road
/// ([`Submaps`], VUGSCANSUBMAP SR522), whose separator graph is held to the same rule.
pub const MAX_NODES: usize = 200;

/// VUGSCANSUBMAP (SR522): the two-level road's run length: nodes `[k·50, (k+1)·50)` are run `k`, its first node a
/// separator (node 0, the gauge, excepted).
pub const SUBMAP_NODES: usize = 50;

/// The central-difference step of the Jacobians, in radians (times the lever in millimetres for the scaled
/// parameters): a derivative step, never a comparison (the stop rule takes [`LoopParams::step_tol`]).
const FD_STEP: f64 = 1e-6;

/// The keyframe, loop and solve rules.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LoopParams {
    /// A frame whose pose turned at least this far from the last keyframe is a keyframe (degrees).
    pub kf_deg: f64,
    /// A frame whose pose moved at least this far from the last keyframe is a keyframe (mm).
    pub kf_mm: f64,
    /// A loop candidate's pose is within this angle of the old node's (degrees).
    pub radius_deg: f64,
    /// A loop candidate's pose is within this distance of the old node's (mm).
    pub radius_mm: f64,
    /// A loop candidate is at least this many nodes after the old node.
    pub min_gap: usize,
    /// A closure is refused below this inlier fraction.
    pub close_frac: f64,
    /// A closure is refused above this rms (mm).
    pub close_rms_mm: f64,
    /// The rotation's lever: radians times this are millimetres in the residual.
    pub lever_mm: f64,
    /// The variance (mm²) one frame without a `Tracked` adds to its odometry edge.
    pub untracked_var_mm2: f64,
    /// Levenberg-Marquardt iterations at most.
    pub iterations: u32,
    /// The solve stops when no node moves more than `lin` mm and `ang` rad; `lin` also floors an rms (BP-vug-2).
    pub step_tol: Tol,
    /// VUGSCANSUBMAP (SR522): past this many nodes [`PoseGraph::solve`] takes the two-level road ([`Submaps`]); at
    /// or below it the dense road. A caller who raises it takes the dense road's cost up to it.
    pub submap_after: usize,
}

impl LoopParams {
    /// Keyframes every 5° or 50 mm, candidates within 15° and 100 mm at least 10 nodes back, closures at an inlier
    /// fraction of 0.5 and an rms of a third of the volume's truncation band, a 500 mm lever (the D435i's working
    /// distance class).
    pub fn for_volume(t: &Tsdf) -> LoopParams {
        LoopParams {
            kf_deg: 5.0,
            kf_mm: 50.0,
            radius_deg: 15.0,
            radius_mm: 100.0,
            min_gap: 10,
            close_frac: 0.5,
            close_rms_mm: t.trunc_mm() / 3.0,
            lever_mm: 500.0,
            untracked_var_mm2: 1e-4,
            iterations: 20,
            step_tol: Tol { lin: 1e-3, ang: 1e-6 },
            submap_after: MAX_NODES,
        }
    }
}

/// How far apart two poses are: the angle of the rotation between them (degrees) and the distance between their
/// origins (mm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Gap {
    pub deg: f64,
    pub mm: f64,
}

/// The [`Gap`] between poses `a` and `b`.
pub fn gap(a: &Transform, b: &Transform) -> Gap {
    let d = a.rot.conjugate() * b.rot;
    let s = libm::sqrt(d.x * d.x + d.y * d.y + d.z * d.z);
    Gap { deg: 2.0 * libm::atan2(s, d.w.abs()) / DEG, mm: (b.trans - a.trans).mag() }
}

/// A keyframe: the frame's index, its pose, and the variance (mm²) its odometry edge from the previous keyframe
/// accumulated.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Keyframe {
    pub frame: u32,
    pub pose: Transform,
    pub var_mm2: f64,
}

/// Every observed frame's pose and the keyframes among them. Caller-owned (BP-vug-4).
#[derive(Clone, Debug)]
pub struct Keyframes {
    params: LoopParams,
    keyframes: Vec<Keyframe>,
    poses: Vec<(u32, Transform)>,
    pending: f64,
}

impl Keyframes {
    pub fn new(params: LoopParams) -> Keyframes {
        Keyframes { params, keyframes: Vec::new(), poses: Vec::new(), pending: 0.0 }
    }

    pub fn params(&self) -> LoopParams {
        self.params
    }

    /// Frame `frame` (increasing) was integrated at `pose`; `tracked` is what its track measured (`None` for the
    /// first frame or a given pose). True when it became a keyframe.
    pub fn observe(&mut self, frame: u32, pose: Transform, tracked: Option<Tracked>) -> bool {
        self.poses.push((frame, pose));
        let Some(last) = self.keyframes.last().copied() else {
            self.keyframes.push(Keyframe { frame, pose, var_mm2: 0.0 });
            return true;
        };
        self.pending += match tracked {
            Some(s) if s.inliers > 0 && s.rms_mm.is_finite() => var_of(&self.params, s.rms_mm, s.inliers),
            _ => self.params.untracked_var_mm2,
        };
        let g = gap(&last.pose, &pose);
        if g.deg >= self.params.kf_deg || g.mm >= self.params.kf_mm {
            self.keyframes.push(Keyframe { frame, pose, var_mm2: self.pending });
            self.pending = 0.0;
            true
        } else {
            false
        }
    }

    pub fn keyframes(&self) -> &[Keyframe] {
        &self.keyframes
    }

    /// Every observed frame's pose, in observation order.
    pub fn poses(&self) -> &[(u32, Transform)] {
        &self.poses
    }

    /// The graph's nodes: the keyframes, and the last observed frame when it is not one.
    pub fn nodes(&self) -> Vec<Keyframe> {
        let mut n = self.keyframes.clone();
        let last_kf = n.last().map(|k| k.frame);
        if let (Some(&(frame, pose)), Some(kf)) = (self.poses.last(), last_kf) {
            if kf != frame {
                n.push(Keyframe { frame, pose, var_mm2: self.pending });
            }
        }
        n
    }
}

/// One frame's variance as an edge sees it: rms² / inliers, the rms floored at the stop rule's `lin`.
fn var_of(prm: &LoopParams, rms_mm: f64, inliers: u32) -> f64 {
    let r = rms_mm.max(prm.step_tol.lin);
    r * r / inliers.max(1) as f64
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EdgeKind {
    Odometry,
    Loop,
}

/// A measured relative pose `rel` = `node[from]⁻¹ ∘ node[to]` with its information weight (1 / mm²).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    pub from: usize,
    pub to: usize,
    pub rel: Transform,
    pub weight: f64,
    pub kind: EdgeKind,
}

/// The pose graph: node poses (camera → world; node 0 is held fixed) and the edges between them.
#[derive(Clone, Debug, PartialEq)]
pub struct PoseGraph {
    pub nodes: Vec<Transform>,
    pub edges: Vec<Edge>,
    pub lever_mm: f64,
}

/// What a solve did.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Solved {
    pub chi2_before: f64,
    pub chi2_after: f64,
    pub iterations: u32,
}

/// Why a graph was not solved.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum GraphError {
    /// More nodes than the dense rule holds ([`MAX_NODES`]); from [`PoseGraph::solve`], more separators in the
    /// two-level road's submap graph than it holds (VUGSCANSUBMAP SR522).
    TooManyNodes { nodes: usize, max: usize },
    /// No damped normal matrix factored: the edges do not constrain every free node.
    Singular,
}

impl PoseGraph {
    /// The chain of `nodes` with an odometry edge between each consecutive pair, at its own pose difference.
    pub fn from_nodes(nodes: &[Keyframe], prm: &LoopParams) -> PoseGraph {
        let mut edges = Vec::new();
        for i in 1..nodes.len() {
            let v = if nodes[i].var_mm2 > 0.0 { nodes[i].var_mm2 } else { prm.untracked_var_mm2 };
            edges.push(Edge {
                from: i - 1,
                to: i,
                rel: inv(&nodes[i - 1].pose).compose(nodes[i].pose),
                weight: 1.0 / v,
                kind: EdgeKind::Odometry,
            });
        }
        PoseGraph { nodes: nodes.iter().map(|k| k.pose).collect(), edges, lever_mm: prm.lever_mm }
    }

    /// Σ weight · |residual|² over the edges (mm² x 1 / mm²).
    pub fn chi2(&self) -> f64 {
        cost(&self.nodes, &self.edges, self.lever_mm)
    }

    /// Levenberg-Marquardt with node 0 fixed: at most `iterations`, stopping when no node moves more than `step_tol`.
    pub fn optimise(&mut self, iterations: u32, step_tol: Tol) -> Result<Solved, GraphError> {
        let n = self.nodes.len();
        if n > MAX_NODES {
            return Err(GraphError::TooManyNodes { nodes: n, max: MAX_NODES });
        }
        self.lm(iterations, step_tol, None)
    }

    /// VUGSCANSUBMAP (SR522): the solve by size. At or below [`LoopParams::submap_after`] nodes the dense road
    /// (bit for bit [`PoseGraph::optimise`]); past it the two-level road: the same Levenberg-Marquardt step, schedule
    /// and stop rule, the normal equations solved through [`Submaps`]. Refuses `TooManyNodes` when the separators
    /// pass [`MAX_NODES`].
    pub fn solve(&mut self, prm: &LoopParams) -> Result<Solved, GraphError> {
        let n = self.nodes.len();
        if n <= prm.submap_after {
            return self.lm(prm.iterations, prm.step_tol, None);
        }
        let s = Submaps::new(n, &self.edges, SUBMAP_NODES);
        if s.separators().len() > MAX_NODES {
            return Err(GraphError::TooManyNodes { nodes: s.separators().len(), max: MAX_NODES });
        }
        self.lm(prm.iterations, prm.step_tol, Some(&s))
    }

    /// The Levenberg-Marquardt loop of `optimise` (SR500), its normal equations dense (`sub` None) or two-level.
    fn lm(&mut self, iterations: u32, step_tol: Tol, sub: Option<&Submaps>) -> Result<Solved, GraphError> {
        let n = self.nodes.len();
        let before = self.chi2();
        if n < 2 {
            return Ok(Solved { chi2_before: before, chi2_after: before, iterations: 0 });
        }
        let lever = self.lever_mm;
        let m = 6 * (n - 1);
        let h = FD_STEP * lever;
        let (mut cost_now, mut lambda, mut iters, mut factored) = (before, 1e-4f64, 0u32, false);
        for _ in 0..iterations {
            iters += 1;
            let mut ne = Normal::new(m, sub);
            for e in &self.edges {
                let (a, b) = (self.nodes[e.from], self.nodes[e.to]);
                let r = residual(e, &a, &b, lever);
                let (mut ja, mut jb) = ([[0.0f64; 6]; 6], [[0.0f64; 6]; 6]); // [param][residual row]
                for k in 0..6 {
                    let (mut dp, mut dm) = ([0.0f64; 6], [0.0f64; 6]);
                    dp[k] = h;
                    dm[k] = -h;
                    let (ap, am) = (residual(e, &retract(&a, &dp, lever), &b, lever), residual(e, &retract(&a, &dm, lever), &b, lever));
                    let (bp, bm) = (residual(e, &a, &retract(&b, &dp, lever), lever), residual(e, &a, &retract(&b, &dm, lever), lever));
                    for i in 0..6 {
                        ja[k][i] = (ap[i] - am[i]) / (2.0 * h);
                        jb[k][i] = (bp[i] - bm[i]) / (2.0 * h);
                    }
                }
                let blocks = [(e.from, ja), (e.to, jb)];
                for (na, jna) in blocks.iter() {
                    if *na == 0 {
                        continue;
                    }
                    for k in 0..6 {
                        ne.add_g(*na, k, e.weight * dot6(&jna[k], &r));
                        for (nb, jnb) in blocks.iter() {
                            if *nb == 0 {
                                continue;
                            }
                            for l in 0..6 {
                                ne.add_h(*na, *nb, k, l, e.weight * dot6(&jna[k], &jnb[l]));
                            }
                        }
                    }
                }
            }
            let mut accepted = false;
            let mut small = false;
            for _ in 0..10 {
                let Some(x) = ne.solve(lambda) else {
                    lambda *= 10.0;
                    continue;
                };
                factored = true;
                let mut trial = self.nodes.clone();
                small = true;
                for j in 1..n {
                    let d = &x[6 * (j - 1)..6 * j];
                    let w = libm::sqrt(d[0] * d[0] + d[1] * d[1] + d[2] * d[2]) / lever;
                    let t = libm::sqrt(d[3] * d[3] + d[4] * d[4] + d[5] * d[5]);
                    if w > step_tol.ang || t > step_tol.lin {
                        small = false;
                    }
                    trial[j] = retract(&self.nodes[j], d, lever);
                }
                let c = cost(&trial, &self.edges, lever);
                if c < cost_now {
                    self.nodes = trial;
                    cost_now = c;
                    lambda = (lambda / 10.0).max(1e-12);
                    accepted = true;
                    break;
                }
                lambda *= 10.0;
            }
            if !factored {
                return Err(GraphError::Singular);
            }
            if !accepted || small {
                break;
            }
        }
        Ok(Solved { chi2_before: before, chi2_after: cost_now, iterations: iters })
    }
}

/// The inverse of a rigid pose (scale 1: geom_core's `invert` cannot refuse it).
fn inv(t: &Transform) -> Transform {
    t.invert().unwrap_or(Transform::IDENTITY)
}

/// The rotation vector of a unit quaternion (the shorter arc).
fn log_rot(q: Quat) -> Vec3 {
    let q = if q.w < 0.0 { Quat::new(-q.x, -q.y, -q.z, -q.w) } else { q };
    let v = Vec3::new(q.x, q.y, q.z);
    let s = v.mag();
    if !(s > 0.0) {
        return Vec3::ZERO;
    }
    v * (2.0 * libm::atan2(s, q.w) / s)
}

/// `t ∘ Δ`, Δ the rotation `x[0..3] / lever` (radians, Rodrigues through `Quat::from_axis_angle`) and the
/// translation `x[3..6]` (mm), in `t`'s own axes.
fn retract(t: &Transform, x: &[f64], lever: f64) -> Transform {
    let w = Vec3::new(x[0] / lever, x[1] / lever, x[2] / lever);
    let th = w.mag();
    let rot = if th > 0.0 { Quat::from_axis_angle(w, th).unwrap_or(Quat::IDENTITY) } else { Quat::IDENTITY };
    t.compose(Transform { rot, scale: 1.0, trans: Vec3::new(x[3], x[4], x[5]) })
}

/// Edge `e`'s residual at node poses `a` (from) and `b` (to): `[lever · Log(R_E), t_E]`, `E = rel⁻¹ ∘ a⁻¹ ∘ b`.
fn residual(e: &Edge, a: &Transform, b: &Transform, lever: f64) -> [f64; 6] {
    let err = inv(&e.rel).compose(inv(a).compose(*b));
    let w = log_rot(err.rot);
    [w.x * lever, w.y * lever, w.z * lever, err.trans.x, err.trans.y, err.trans.z]
}

fn cost(nodes: &[Transform], edges: &[Edge], lever: f64) -> f64 {
    let mut c = 0.0;
    for e in edges {
        let r = residual(e, &nodes[e.from], &nodes[e.to], lever);
        c += e.weight * dot6(&r, &r);
    }
    c
}

fn dot6(a: &[f64; 6], b: &[f64; 6]) -> f64 {
    let mut s = 0.0;
    for i in 0..6 {
        s += a[i] * b[i];
    }
    s
}

/// The dense symmetric positive-definite solve `a·x = b` (`a` is `m` x `m` row-major; its lower triangle is read):
/// geom_core's `linalg::cholesky_solve`, the all-zero profile of the envelope Cholesky the submap interiors take
/// (VUGGEOMLINALGSCAN SR540: SR522's `factor` / `substitute` moved to geom_core as `cholesky_profile` /
/// `cholesky_substitute`, the same operations in the same order). `None` on a pivot that is not positive or a
/// non-finite step.
fn solved(m: usize, mut a: Vec<f64>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    if !dense_solve(&mut a, m, &mut b) {
        return None;
    }
    if b.iter().all(|x| x.is_finite()) { Some(b) } else { None }
}

/// One frame as the closure and the re-integration read it.
#[derive(Clone, Copy)]
pub struct Shot<'a> {
    pub frame: &'a Frame,
    pub scale: DepthScale,
    pub intr: &'a Intrinsics,
}

/// An accepted closure: what SR478's ICP measured and the relative pose `old⁻¹ ∘ new` it found.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Closed {
    pub inliers: u32,
    pub valid: u32,
    pub rms_mm: f64,
    pub rel: Transform,
}

/// Why a loop candidate was not closed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Refused {
    /// SR478's own gates refused the track (too few pairs, or the rms past the band).
    Lost { inliers: u32, residual_mm: f64 },
    /// The track held but under the closure's inlier fraction or over its rms.
    Gate { frac: f64, rms_mm: f64 },
    /// The ICP moved the guess farther than the candidate radius: not the loop the poses predicted.
    Slid { deg: f64, mm: f64 },
    /// The caller gave no frame for this index.
    NoFrame { frame: u32 },
    /// The keyframe's frame or volume refused it.
    Scan(ScanError),
}

/// One loop candidate: the new and the old node's frames, their pose gap, and the closure's outcome.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Candidate {
    pub new: u32,
    pub old: u32,
    pub gap: Gap,
    pub outcome: Result<Closed, Refused>,
}

/// SR478's ICP of `new` against `old`'s frame alone: `old` integrated at the identity into a fresh volume shaped
/// like `t`, raycast there (`Tracker::refresh`), `new` tracked from `guess` (old⁻¹ ∘ new). Accepted under the
/// closure gates of `prm`.
pub fn match_keyframe(t: &Tsdf, prm: &LoopParams, old: Shot<'_>, new: Shot<'_>, guess: Transform) -> Result<Closed, Refused> {
    let mut kt = Tsdf::new(t.voxel_mm(), t.trunc_mm(), t.tol()).map_err(Refused::Scan)?.with_budget(t.budget());
    kt.integrate(old.frame, old.scale, old.intr, &Transform::IDENTITY).map_err(Refused::Scan)?;
    let mut tr = Tracker::new(&kt, t.tol());
    tr.refresh(&kt, old.intr);
    let mut tr = tr.with_pose(guess);
    let rel = match tr.track(new.frame, new.scale, new.intr, None) {
        Ok(p) => p,
        Err(TrackError::Lost { inliers, residual_mm }) => return Err(Refused::Lost { inliers, residual_mm }),
        Err(TrackError::Scan(e)) => return Err(Refused::Scan(e)),
        // Not reached: `refresh` set the model and no gravity is passed.
        Err(TrackError::NoModel) | Err(TrackError::GravityDisagrees { .. }) => {
            return Err(Refused::Lost { inliers: 0, residual_mm: f64::INFINITY });
        }
    };
    let Some(s) = tr.last() else { return Err(Refused::Lost { inliers: 0, residual_mm: f64::INFINITY }) };
    let frac = if s.valid > 0 { s.inliers as f64 / s.valid as f64 } else { 0.0 };
    if frac < prm.close_frac || !(s.rms_mm <= prm.close_rms_mm) {
        return Err(Refused::Gate { frac, rms_mm: s.rms_mm });
    }
    let moved = gap(&guess, &rel);
    if moved.deg > prm.radius_deg || moved.mm > prm.radius_mm {
        return Err(Refused::Slid { deg: moved.deg, mm: moved.mm });
    }
    Ok(Closed { inliers: s.inliers, valid: s.valid, rms_mm: s.rms_mm, rel })
}

/// The honest result: the nodes, every candidate with its outcome, the closures, the worst closure's gap to the
/// poses before and after the solve (`None` without a closure), the graph cost, and every observed frame's pose
/// (unchanged, bit for bit, without a closure).
#[derive(Clone, Debug, PartialEq)]
pub struct Loop {
    pub keyframes: usize,
    pub candidates: Vec<Candidate>,
    pub closures: u32,
    pub before: Option<Gap>,
    pub after: Option<Gap>,
    pub chi2_before: f64,
    pub chi2_after: f64,
    pub iterations: u32,
    pub poses: Vec<(u32, Transform)>,
}

/// The worst loop edge's disagreement with the graph's node poses (degrees and mm, each its own maximum).
fn worst_gap(g: &PoseGraph) -> Option<Gap> {
    let mut out: Option<Gap> = None;
    for e in g.edges.iter().filter(|e| e.kind == EdgeKind::Loop) {
        let d = gap(&e.rel, &inv(&g.nodes[e.from]).compose(g.nodes[e.to]));
        out = Some(match out {
            Some(o) => Gap { deg: o.deg.max(d.deg), mm: o.mm.max(d.mm) },
            None => d,
        });
    }
    out
}

/// Detect, close and solve the loops of `kf`. `t` is the scan's volume (its voxel, band, tol and budget shape the
/// one-frame keyframe volumes); `shot` gives the frame of an observed index. With no closure nothing is solved and
/// the poses come back as observed.
pub fn close_loop<'a, F>(kf: &Keyframes, t: &Tsdf, mut shot: F) -> Result<Loop, GraphError>
where
    F: FnMut(u32) -> Option<Shot<'a>>,
{
    let prm = kf.params();
    let nodes = kf.nodes();
    let mut graph = PoseGraph::from_nodes(&nodes, &prm);
    let mut candidates = Vec::new();
    for i in 0..nodes.len() {
        let mut best: Option<(usize, Gap)> = None;
        for j in 0..i {
            if j + prm.min_gap > i {
                break;
            }
            let g = gap(&nodes[j].pose, &nodes[i].pose);
            if g.deg <= prm.radius_deg && g.mm <= prm.radius_mm && best.map_or(true, |(_, b)| g.mm < b.mm) {
                best = Some((j, g));
            }
        }
        let Some((j, g)) = best else { continue };
        let guess = inv(&nodes[j].pose).compose(nodes[i].pose);
        let outcome = match (shot(nodes[j].frame), shot(nodes[i].frame)) {
            (Some(old), Some(new)) => match_keyframe(t, &prm, old, new, guess),
            (None, _) => Err(Refused::NoFrame { frame: nodes[j].frame }),
            (_, None) => Err(Refused::NoFrame { frame: nodes[i].frame }),
        };
        if let Ok(c) = outcome {
            graph.edges.push(Edge {
                from: j,
                to: i,
                rel: c.rel,
                weight: 1.0 / var_of(&prm, c.rms_mm, c.inliers),
                kind: EdgeKind::Loop,
            });
        }
        candidates.push(Candidate { new: nodes[i].frame, old: nodes[j].frame, gap: g, outcome });
    }
    let closures = graph.edges.iter().filter(|e| e.kind == EdgeKind::Loop).count() as u32;
    if closures == 0 {
        let c = graph.chi2();
        return Ok(Loop {
            keyframes: nodes.len(),
            candidates,
            closures,
            before: None,
            after: None,
            chi2_before: c,
            chi2_after: c,
            iterations: 0,
            poses: kf.poses().to_vec(),
        });
    }
    let before = worst_gap(&graph);
    let solved = graph.solve(&prm)?;
    let after = worst_gap(&graph);
    // Each frame carried by the correction of the latest node at or before it: P' = T' ∘ T⁻¹ ∘ P.
    let mut poses = Vec::with_capacity(kf.poses().len());
    let mut ni = 0usize;
    for &(f, p) in kf.poses() {
        while ni + 1 < nodes.len() && nodes[ni + 1].frame <= f {
            ni += 1;
        }
        let c = graph.nodes[ni].compose(inv(&nodes[ni].pose));
        poses.push((f, c.compose(p)));
    }
    Ok(Loop {
        keyframes: nodes.len(),
        candidates,
        closures,
        before,
        after,
        chi2_before: solved.chi2_before,
        chi2_after: solved.chi2_after,
        iterations: solved.iterations,
        poses,
    })
}

/// Why a re-integration stopped (the volume given is untouched; the partial one is dropped).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Reintegrate {
    /// The caller gave no frame for this index.
    NoFrame { frame: u32 },
    /// The volume refused this frame.
    Scan { frame: u32, error: ScanError },
}

/// A cleared volume shaped like `t` (voxel, band, tol, budget) with every frame of `poses` integrated at its pose.
pub fn reintegrate<'a, F>(t: &Tsdf, poses: &[(u32, Transform)], mut shot: F) -> Result<Tsdf, Reintegrate>
where
    F: FnMut(u32) -> Option<Shot<'a>>,
{
    let first = poses.first().map_or(0, |p| p.0);
    let mut out = Tsdf::new(t.voxel_mm(), t.trunc_mm(), t.tol())
        .map_err(|error| Reintegrate::Scan { frame: first, error })?
        .with_budget(t.budget());
    for &(frame, pose) in poses {
        let Some(s) = shot(frame) else { return Err(Reintegrate::NoFrame { frame }) };
        out.integrate(s.frame, s.scale, s.intr, &pose).map_err(|error| Reintegrate::Scan { frame, error })?;
    }
    Ok(out)
}

// VUGSCANSUBMAP (SR522): the two-level road. Written from the Schur complement of the Gauss-Newton normal equations
// (Triggs et al., "Bundle Adjustment — A Modern Synthesis" (1999) §6.1; the separator form of nested dissection,
// George (1973)): the nodes fall in runs of [`SUBMAP_NODES`]; each run's first node, and both ends of any edge that
// joins the interiors of two runs, are SEPARATORS (the submap graph); every other node is its run's interior, tied
// by edges only to its own run and the separators. A step factors each run's interior block once, solves the
// separators' reduced system dense, then each interior alone with its ends pinned at their step: the dense step to
// rounding, at ~n/50 dense nodes instead of n (measured in design: 17 mm for rigid submaps vs 3.5e-8 mm for this
// road against the dense optimum at 250 nodes).

/// The two-level structure of a graph: which node is the gauge, a separator or a run's interior, each run's
/// separators and its interior block's envelope. Built from the node count and the edges only.
#[derive(Clone, Debug)]
pub struct Submaps {
    slot: Vec<Slot>,
    seps: Vec<usize>,
    runs: Vec<Run>,
}

/// A node's place: the gauge (node 0, fixed), separator `i`, or interior node `at` of run `r`.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Slot {
    Gauge,
    Sep(usize),
    Inner(usize, usize),
}

/// One run: its interior nodes (ascending), the separators its edges reach (indices, ascending) and the envelope
/// (`first`, per row of its 6-wide interior block).
#[derive(Clone, Debug)]
struct Run {
    nodes: Vec<usize>,
    adj: Vec<usize>,
    first: Vec<usize>,
}

impl Submaps {
    /// `nodes` nodes in runs of `run` (at least 1) with `edges` between them.
    pub fn new(nodes: usize, edges: &[Edge], run: usize) -> Submaps {
        let run = run.max(1);
        let mut sep = vec![false; nodes];
        let mut j = run;
        while j < nodes {
            sep[j] = true;
            j += run;
        }
        for e in edges {
            let (a, b) = (e.from, e.to);
            if a != 0 && b != 0 && !sep[a] && !sep[b] && a / run != b / run {
                sep[a] = true;
                sep[b] = true;
            }
        }
        let mut slot = vec![Slot::Gauge; nodes];
        let mut seps = Vec::new();
        let mut runs: Vec<Run> =
            (0..nodes.div_ceil(run)).map(|_| Run { nodes: Vec::new(), adj: Vec::new(), first: Vec::new() }).collect();
        for j in 1..nodes {
            if sep[j] {
                slot[j] = Slot::Sep(seps.len());
                seps.push(j);
            } else {
                let r = j / run;
                slot[j] = Slot::Inner(r, runs[r].nodes.len());
                runs[r].nodes.push(j);
            }
        }
        // The envelope by block: interior node p's rows start at the lowest interior block an edge ties it to.
        let mut fb: Vec<Vec<usize>> = runs.iter().map(|r| (0..r.nodes.len()).collect()).collect();
        for e in edges {
            match (slot[e.from], slot[e.to]) {
                (Slot::Inner(r, p), Slot::Inner(r2, q)) if r == r2 => {
                    let (lo, hi) = (p.min(q), p.max(q));
                    fb[r][hi] = fb[r][hi].min(lo);
                }
                (Slot::Inner(r, _), Slot::Sep(i)) | (Slot::Sep(i), Slot::Inner(r, _)) => {
                    if !runs[r].adj.contains(&i) {
                        runs[r].adj.push(i);
                    }
                }
                _ => {}
            }
        }
        for (r, rn) in runs.iter_mut().enumerate() {
            rn.adj.sort_unstable();
            rn.first = fb[r].iter().flat_map(|&b| [6 * b; 6]).collect();
        }
        Submaps { slot, seps, runs }
    }

    /// How many runs.
    pub fn runs(&self) -> usize {
        self.runs.len()
    }

    /// The submap graph's nodes (the separators), ascending.
    pub fn separators(&self) -> &[usize] {
        &self.seps
    }
}

/// One Levenberg-Marquardt iteration's normal equations `H·x = −g` over the free nodes 1..n (row `6(j − 1) + k`):
/// dense (SR500's `hm` and `g`, the same accumulation and solve) or two-level over a [`Submaps`].
enum Normal<'a> {
    Dense { m: usize, hm: Vec<f64>, g: Vec<f64> },
    Two { s: &'a Submaps, m: usize, hss: Vec<f64>, gs: Vec<f64>, hkk: Vec<Vec<f64>>, hks: Vec<Vec<f64>>, gk: Vec<Vec<f64>> },
}

impl<'a> Normal<'a> {
    fn new(m: usize, sub: Option<&'a Submaps>) -> Normal<'a> {
        match sub {
            None => Normal::Dense { m, hm: vec![0.0f64; m * m], g: vec![0.0f64; m] },
            Some(s) => {
                let ns = 6 * s.seps.len();
                Normal::Two {
                    s,
                    m,
                    hss: vec![0.0f64; ns * ns],
                    gs: vec![0.0f64; ns],
                    hkk: s.runs.iter().map(|r| vec![0.0f64; 36 * r.nodes.len() * r.nodes.len()]).collect(),
                    hks: s.runs.iter().map(|r| vec![0.0f64; 36 * r.nodes.len() * r.adj.len()]).collect(),
                    gk: s.runs.iter().map(|r| vec![0.0f64; 6 * r.nodes.len()]).collect(),
                }
            }
        }
    }

    /// `g[node a, row k] += v` (`a` ≥ 1).
    fn add_g(&mut self, a: usize, k: usize, v: f64) {
        match self {
            Normal::Dense { g, .. } => g[6 * (a - 1) + k] += v,
            Normal::Two { s, gs, gk, .. } => match s.slot[a] {
                Slot::Sep(i) => gs[6 * i + k] += v,
                Slot::Inner(r, p) => gk[r][6 * p + k] += v,
                Slot::Gauge => {}
            },
        }
    }

    /// `H[node a, row k][node b, column l] += v` (`a`, `b` ≥ 1). The two-level form keeps an interior / separator
    /// pair once, as `H_ks` (the dense H is exactly symmetric: `dot6` commutes term by term).
    fn add_h(&mut self, a: usize, b: usize, k: usize, l: usize, v: f64) {
        match self {
            Normal::Dense { m, hm, .. } => hm[(6 * (a - 1) + k) * *m + 6 * (b - 1) + l] += v,
            Normal::Two { s, hss, hkk, hks, .. } => match (s.slot[a], s.slot[b]) {
                (Slot::Inner(r, p), Slot::Inner(r2, q)) if r == r2 => {
                    let mk = 6 * s.runs[r].nodes.len();
                    hkk[r][(6 * p + k) * mk + 6 * q + l] += v;
                }
                (Slot::Inner(r, p), Slot::Sep(i)) => {
                    let adj = &s.runs[r].adj;
                    if let Some(c) = adj.iter().position(|&x| x == i) {
                        let ck = 6 * adj.len();
                        hks[r][(6 * p + k) * ck + 6 * c + l] += v;
                    }
                }
                (Slot::Sep(i), Slot::Sep(j)) => {
                    let ns = 6 * s.seps.len();
                    hss[(6 * i + k) * ns + 6 * j + l] += v;
                }
                _ => {}
            },
        }
    }

    /// The step `x` (dense layout) of the damped equations `(H + λ·diag H)·x = −g`, `None` when a factor fails.
    fn solve(&self, lambda: f64) -> Option<Vec<f64>> {
        match self {
            Normal::Dense { m, hm, g } => {
                let m = *m;
                let mut a = hm.clone();
                for i in 0..m {
                    a[i * m + i] *= 1.0 + lambda;
                }
                let b: Vec<f64> = g.iter().map(|x| -x).collect();
                solved(m, a, b)
            }
            Normal::Two { s, m, hss, gs, hkk, hks, gk } => {
                let ns = 6 * s.seps.len();
                let mut sm = hss.clone();
                for i in 0..ns {
                    sm[i * ns + i] *= 1.0 + lambda;
                }
                // The separators' reduced system: S = H_ss − Σ H_ksᵀ H_kk⁻¹ H_ks, r = b_s − Σ H_ksᵀ H_kk⁻¹ b_k.
                let mut rhs: Vec<f64> = gs.iter().map(|x| -x).collect();
                let mut parts: Vec<(Vec<f64>, Vec<f64>)> = Vec::with_capacity(s.runs.len());
                for (r, run) in s.runs.iter().enumerate() {
                    let mk = 6 * run.nodes.len();
                    let ck = 6 * run.adj.len();
                    let mut a = hkk[r].clone();
                    for i in 0..mk {
                        a[i * mk + i] *= 1.0 + lambda;
                    }
                    if !cholesky_profile(&mut a, mk, &run.first) {
                        return None;
                    }
                    let mut z: Vec<f64> = gk[r].iter().map(|x| -x).collect();
                    if !cholesky_substitute(&a, mk, &run.first, &mut z) {
                        return None;
                    }
                    let mut y = vec![0.0f64; ck * mk]; // column c of H_kk⁻¹ H_ks at y[c·mk ..]
                    for c in 0..ck {
                        let col = &mut y[c * mk..(c + 1) * mk];
                        for i in 0..mk {
                            col[i] = hks[r][i * ck + c];
                        }
                        if !cholesky_substitute(&a, mk, &run.first, col) {
                            return None;
                        }
                    }
                    for (ci, &si) in run.adj.iter().enumerate() {
                        for u in 0..6 {
                            let (row, c) = (6 * si + u, 6 * ci + u);
                            let mut t = 0.0;
                            for i in 0..mk {
                                t += hks[r][i * ck + c] * z[i];
                            }
                            rhs[row] -= t;
                            for (cj, &sj) in run.adj.iter().enumerate() {
                                for v in 0..6 {
                                    let c2 = 6 * cj + v;
                                    let mut t = 0.0;
                                    for i in 0..mk {
                                        t += hks[r][i * ck + c] * y[c2 * mk + i];
                                    }
                                    sm[row * ns + 6 * sj + v] -= t;
                                }
                            }
                        }
                    }
                    parts.push((z, y));
                }
                let xs = solved(ns, sm, rhs)?;
                let mut x = vec![0.0f64; *m];
                for (si, &node) in s.seps.iter().enumerate() {
                    x[6 * (node - 1)..6 * node].copy_from_slice(&xs[6 * si..6 * si + 6]);
                }
                // Each interior alone, its ends pinned at their step: x_k = H_kk⁻¹ b_k − (H_kk⁻¹ H_ks) x_s.
                for (r, run) in s.runs.iter().enumerate() {
                    let (z, y) = &parts[r];
                    let mk = 6 * run.nodes.len();
                    for (p, &node) in run.nodes.iter().enumerate() {
                        for u in 0..6 {
                            let i = 6 * p + u;
                            let mut v = z[i];
                            for (ci, &si) in run.adj.iter().enumerate() {
                                for w in 0..6 {
                                    v -= y[(6 * ci + w) * mk + i] * xs[6 * si + w];
                                }
                            }
                            x[6 * (node - 1) + u] = v;
                        }
                    }
                }
                if x.iter().all(|v| v.is_finite()) { Some(x) } else { None }
            }
        }
    }
}
