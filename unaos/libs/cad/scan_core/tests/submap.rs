// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! VUGSCANSUBMAP (SR522): the two-level road of the pose graph. Synthetic rings, no frames (`close_loop`'s road is
//! `PoseGraph::solve`; the ICP of hundreds of keyframes is the track's runtime, not the graph's): truth node i at
//! azimuth iθ on a 1000 mm circle, every odometry measurement the truth composed with a 1 % bias (1 % of the step's
//! turn about z and 1 % of its length sideways), closures at the truth relative pose. The dense road below
//! `LoopParams::submap_after` is bit for bit SR500's `optimise`; past it the 600-node ring closes through `Submaps`
//! inside 10 s; at 250 nodes both roads give the same poses within 1e-3 mm.

use core::f64::consts::PI;
use std::time::Instant;

use geom_core::Quat;
use scan_core::graph::{SUBMAP_NODES, Submaps};
use scan_core::{Edge, EdgeKind, LoopParams, PoseGraph, Tol, Transform, Tsdf, Vec3, gap};

const RADIUS_MM: f64 = 1000.0;
const DRIFT: f64 = 0.01;
/// Odometry information (σ² 0.01 mm²) and closure information (σ² 0.001 mm²).
const ODO_W: f64 = 100.0;
const LOOP_W: f64 = 1000.0;

fn rz(a: f64) -> Quat {
    Quat::from_axis_angle(Vec3::new(0.0, 0.0, 1.0), a).expect("axis")
}

fn inv(t: &Transform) -> Transform {
    t.invert().expect("rigid")
}

fn params() -> LoopParams {
    LoopParams::for_volume(&Tsdf::new(4.0, 12.0, Tol::DEFAULT).expect("volume"))
}

/// The truth ring of `n` nodes and its drifted graph: the nodes chained from the truth's node 0 through the biased
/// odometry, then one `Loop` edge per closure at the truth.
fn ring(n: usize, closures: &[(usize, usize)]) -> (Vec<Transform>, PoseGraph) {
    let th = 2.0 * PI / n as f64;
    let truth: Vec<Transform> = (0..n)
        .map(|i| {
            let a = i as f64 * th;
            Transform { rot: rz(a), scale: 1.0, trans: Vec3::new(RADIUS_MM * a.cos(), RADIUS_MM * a.sin(), 0.0) }
        })
        .collect();
    let mut nodes = vec![truth[0]];
    let mut edges = Vec::new();
    for i in 1..n {
        let rel = inv(&truth[i - 1]).compose(truth[i]);
        let bias = Transform { rot: rz(DRIFT * th), scale: 1.0, trans: Vec3::new(0.0, DRIFT * rel.trans.mag(), 0.0) };
        let z = rel.compose(bias);
        nodes.push(nodes[i - 1].compose(z));
        edges.push(Edge { from: i - 1, to: i, rel: z, weight: ODO_W, kind: EdgeKind::Odometry });
    }
    for &(a, b) in closures {
        edges.push(Edge { from: a, to: b, rel: inv(&truth[a]).compose(truth[b]), weight: LOOP_W, kind: EdgeKind::Loop });
    }
    (truth, PoseGraph { nodes, edges, lever_mm: params().lever_mm })
}

/// The last node's distance from its truth (mm).
fn end_mm(g: &PoseGraph, truth: &[Transform]) -> f64 {
    let n = truth.len() - 1;
    (g.nodes[n].trans - truth[n].trans).mag()
}

fn bits(t: &Transform) -> [u64; 8] {
    [
        t.rot.x.to_bits(),
        t.rot.y.to_bits(),
        t.rot.z.to_bits(),
        t.rot.w.to_bits(),
        t.scale.to_bits(),
        t.trans.x.to_bits(),
        t.trans.y.to_bits(),
        t.trans.z.to_bits(),
    ]
}

/// At or below `submap_after` (200) `solve` is SR500's dense road: the same `Solved` and the same node bits as
/// `optimise` on a 60-node ring closed end to start (tests/scan_loop.rs's orbit, 65 keyframes, rides this road).
#[test]
fn dense_road_unchanged_below_submap_after() {
    let prm = params();
    assert_eq!(prm.submap_after, 200);
    let n = 60;
    let (truth, g0) = ring(n, &[(0, n - 1)]);
    let (mut a, mut b) = (g0.clone(), g0.clone());
    let sa = a.optimise(prm.iterations, prm.step_tol).expect("optimise");
    let sb = b.solve(&prm).expect("solve");
    assert_eq!(sa, sb);
    for (j, (p, q)) in a.nodes.iter().zip(&b.nodes).enumerate() {
        assert_eq!(bits(p), bits(q), "node {j}");
    }
    let end = end_mm(&b, &truth);
    println!(
        "submap: dense-road nodes={n} bitwise=yes chi2={:.3}->{:.3} iters={} end_before_mm={:.3} end_mm={end:.6}",
        sb.chi2_before,
        sb.chi2_after,
        sb.iterations,
        end_mm(&g0, &truth)
    );
    assert!(end < 0.5, "end {end} mm");
}

/// THE CLAIM (SR522): a 600-node ring with 1 % drift, closed end to start plus a closure across runs (20-130) and
/// one inside a run (105-120), closes through `Submaps` (12 runs, 13 separators) to under 0.5 mm at the end, inside
/// a 10 s budget on this seat.
#[test]
fn six_hundred_node_loop_closes_through_submaps() {
    let prm = params();
    let n = 600;
    let (truth, mut g) = ring(n, &[(0, n - 1), (20, 130), (105, 120)]);
    let s = Submaps::new(n, &g.edges, SUBMAP_NODES);
    let mut want: Vec<usize> = (1..12).map(|k| 50 * k).chain([20, 130]).collect();
    want.sort_unstable();
    assert_eq!(s.runs(), 12);
    assert_eq!(s.separators(), want.as_slice());
    let before = end_mm(&g, &truth);
    let clock = Instant::now();
    let solved = g.solve(&prm).expect("solve");
    let solve_s = clock.elapsed().as_secs_f64();
    let after = end_mm(&g, &truth);
    println!(
        "submap: nodes={n} runs={} separators={} chi2={:.3}->{:.3} iters={} end_before_mm={before:.3} end_after_mm={after:.6} solve_s={solve_s:.3}",
        s.runs(),
        s.separators().len(),
        solved.chi2_before,
        solved.chi2_after,
        solved.iterations
    );
    assert!(after < 0.5, "end after {after} mm");
    assert!(solve_s < 10.0, "solve {solve_s} s");
}

/// The two roads agree: a 250-node ring with the same closure shapes, solved dense (`submap_after` raised to 250)
/// and through `Submaps` (the default 200), every node within 1e-3 mm and its rotation within 1e-3 mm at the lever.
#[test]
fn two_roads_agree_at_250() {
    let prm = params();
    let n = 250;
    let (truth, g0) = ring(n, &[(0, n - 1), (20, 130), (105, 120)]);
    let mut dense_prm = prm;
    dense_prm.submap_after = n;
    let (mut d, mut s) = (g0.clone(), g0.clone());
    let clock = Instant::now();
    let sd = d.solve(&dense_prm).expect("dense");
    let dense_s = clock.elapsed().as_secs_f64();
    let clock = Instant::now();
    let ss = s.solve(&prm).expect("submaps");
    let submap_s = clock.elapsed().as_secs_f64();
    let (mut max_mm, mut max_rot_mm) = (0.0f64, 0.0f64);
    for (a, b) in d.nodes.iter().zip(&s.nodes) {
        let gp = gap(a, b);
        max_mm = max_mm.max(gp.mm);
        max_rot_mm = max_rot_mm.max(gp.deg * PI / 180.0 * prm.lever_mm);
    }
    println!(
        "submap: agree nodes={n} max_mm={max_mm:.3e} max_rot_mm={max_rot_mm:.3e} chi2={:.3}/{:.3} iters={}/{} end_mm={:.6}/{:.6} dense_s={dense_s:.3} submap_s={submap_s:.3}",
        sd.chi2_after,
        ss.chi2_after,
        sd.iterations,
        ss.iterations,
        end_mm(&d, &truth),
        end_mm(&s, &truth)
    );
    assert!(max_mm <= 1e-3, "roads differ by {max_mm} mm");
    assert!(max_rot_mm <= 1e-3, "roads differ by {max_rot_mm} mm of rotation at the lever");
}
