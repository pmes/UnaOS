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
//!    Jacobians, the normal equations dense (at most [`MAX_NODES`] nodes) by Cholesky.
//! 5. Every frame's pose is carried by its keyframe's correction; [`reintegrate`] fuses every frame again at its
//!    corrected pose into a cleared volume (all of them: BP-vug-5's confidence is the observation count).
//!
//! No truth is claimed: [`Loop`] reports the closures' disagreement with the poses ([`Gap`]) before and after the
//! solve, the only error a recording can know.

use alloc::vec;
use alloc::vec::Vec;

use geom_core::Quat;
use probe_core::{DepthScale, Frame, Intrinsics};

use crate::ScanError;
use crate::geom::{Tol, Transform, Vec3};
use crate::pose::{TrackError, Tracked, Tracker};
use crate::tsdf::Tsdf;

const DEG: f64 = core::f64::consts::PI / 180.0;

/// The dense rule: the normal equations are `6(N − 1)` square, 11.4 MB and ~2.8e8 flops a factorisation at 200
/// nodes; past it the graph is refused by name (VUG-SCAN-SUBMAP owes large scenes).
pub const MAX_NODES: usize = 200;

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
    /// More nodes than the dense rule holds ([`MAX_NODES`]).
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
            let mut hm = vec![0.0f64; m * m];
            let mut g = vec![0.0f64; m];
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
                    let oa = 6 * (*na - 1);
                    for k in 0..6 {
                        g[oa + k] += e.weight * dot6(&jna[k], &r);
                        for (nb, jnb) in blocks.iter() {
                            if *nb == 0 {
                                continue;
                            }
                            let ob = 6 * (*nb - 1);
                            for l in 0..6 {
                                hm[(oa + k) * m + ob + l] += e.weight * dot6(&jna[k], &jnb[l]);
                            }
                        }
                    }
                }
            }
            let mut accepted = false;
            let mut small = false;
            for _ in 0..10 {
                let mut a = hm.clone();
                for i in 0..m {
                    a[i * m + i] *= 1.0 + lambda;
                }
                let b: Vec<f64> = g.iter().map(|x| -x).collect();
                let Some(x) = cholesky_solve(m, a, b) else {
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
/// Cholesky, then the two triangular solves. `None` on a pivot that is not positive. (pose.rs's `solve6` is the
/// fixed 6x6 of the same loop; VUG-LINALG owes the one shared dense solve.)
fn cholesky_solve(m: usize, mut a: Vec<f64>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    for j in 0..m {
        let mut d = a[j * m + j];
        for k in 0..j {
            d -= a[j * m + k] * a[j * m + k];
        }
        if !(d > 0.0) || !d.is_finite() {
            return None;
        }
        let l = libm::sqrt(d);
        a[j * m + j] = l;
        for i in j + 1..m {
            let mut s = a[i * m + j];
            for k in 0..j {
                s -= a[i * m + k] * a[j * m + k];
            }
            a[i * m + j] = s / l;
        }
    }
    for i in 0..m {
        let mut s = b[i];
        for k in 0..i {
            s -= a[i * m + k] * b[k];
        }
        b[i] = s / a[i * m + i];
    }
    for i in (0..m).rev() {
        let mut s = b[i];
        for k in i + 1..m {
            s -= a[k * m + i] * b[k];
        }
        b[i] = s / a[i * m + i];
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
    let solved = graph.optimise(prm.iterations, prm.step_tol)?;
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
