// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! VUGSCANLOOP (SR500): the 360° orbit tracked and closed, an open arc that does not close, and a false closure
//! candidate refused by the ICP gate. The orbit runs at HALF the bench resolution (212 x 120, fx = fy = 106): SR478's
//! 121 frames at 424 x 240 took 240 s in debug, and this test tracks 361.

use core::f64::consts::PI;
use std::time::Instant;

use scan_core::fixture::{self, BoxPrim, Noise, Scene};
use scan_core::{
    DepthScale, Frame, Intrinsics, Keyframes, LoopParams, Refused, Shot, Tol, Tracker, Transform, Tsdf, Vec3,
    close_loop, extract, fit_plane, reintegrate,
};

const DEG: f64 = PI / 180.0;
const CENTRE: Vec3 = Vec3::new(-20.0, 0.0, 50.0);
const TOL: Tol = Tol { lin: 1e-3, ang: 1e-6 };
const X: Vec3 = Vec3::new(1.0, 0.0, 0.0);
const Y: Vec3 = Vec3::new(0.0, 1.0, 0.0);
const Z: Vec3 = Vec3::new(0.0, 0.0, 1.0);

/// The bench intrinsics halved (the runtime cap).
fn intr() -> Intrinsics {
    Intrinsics { width: 212, height: 120, ppx: 106.0, ppy: 60.0, fx: 106.0, fy: 106.0 }
}

/// The orbit's pose at azimuth `deg` (tests/pose.rs's orbit).
fn at(deg: f64) -> Transform {
    fixture::orbit(CENTRE, 600.0, 40.0 * DEG, deg * DEG)
}

fn render(scene: &Scene, deg: f64, seq: u64) -> Frame {
    fixture::render_depth(scene, &intr(), DepthScale::D400_DEFAULT, &at(deg), &Noise::D435I, seq)
}

fn local_vec(t: &Transform, v: Vec3) -> Vec3 {
    Vec3::new(t.apply_vector(X).dot(v), t.apply_vector(Y).dot(v), t.apply_vector(Z).dot(v))
}

fn local(t: &Transform, p: Vec3) -> Vec3 {
    local_vec(t, p - t.trans)
}

fn unit(v: Vec3) -> Vec3 {
    v * (1.0 / v.mag())
}

fn angle_deg(a: Vec3, b: Vec3) -> f64 {
    libm::acos(unit(a).dot(unit(b)).abs().min(1.0)) / DEG
}

/// An estimate `e` (camera k → camera 0) against the truth P_0⁻¹·P_k: (mm, degrees) (tests/pose.rs's measure).
fn pose_error(e: &Transform, p0: &Transform, pk: &Transform) -> (f64, f64) {
    let dt = (e.trans - local(p0, pk.trans)).mag();
    let mut tr = 0.0;
    for b in [X, Y, Z] {
        tr += e.apply_vector(b).dot(local_vec(p0, pk.apply_vector(b)));
    }
    (dt, libm::acos(((tr - 1.0) / 2.0).clamp(-1.0, 1.0)) / DEG)
}

/// The least-squares slope through the origin of an error series, sum(k e_k) / sum(k²).
fn rate(series: &[f64]) -> f64 {
    let (mut ke, mut kk) = (0.0f64, 0.0f64);
    for (k, e) in series.iter().enumerate() {
        ke += k as f64 * e;
        kk += (k * k) as f64;
    }
    if kk > 0.0 { ke / kk } else { 0.0 }
}

/// Track `frames` (frame k at azimuth k°) from the identity, every accepted pose observed.
fn track(frames: &[Frame]) -> (Tsdf, Keyframes) {
    let (intr, scale) = (intr(), DepthScale::D400_DEFAULT);
    let mut t = Tsdf::new(4.0, 12.0, Tol::DEFAULT).expect("volume");
    let mut tr = Tracker::new(&t, TOL);
    let mut kf = Keyframes::new(LoopParams::for_volume(&t));
    for (k, f) in frames.iter().enumerate() {
        let (pose, _) =
            t.integrate_tracked(&mut tr, f, scale, &intr, None).unwrap_or_else(|err| panic!("frame {k}: {err:?}"));
        kf.observe(k as u32, pose, if k == 0 { None } else { tr.last() });
    }
    (t, kf)
}

/// THE CLAIM (SR500): the 360° orbit at 1° a frame closes on itself; after the closure the final frame is within
/// 0.5° / 2 mm of the truth and the drift over the loop is at most 0.005°/frame. SR478's clean run measured 0.0051
/// °/frame over 120° BEFORE any closure, so the before figures are printed, never asserted.
#[test]
fn orbit_loop_closes() {
    let (intr, scale) = (intr(), DepthScale::D400_DEFAULT);
    let n = 361u32;
    let bench = Scene::bench();
    let frames: Vec<Frame> = (0..n).map(|k| render(&bench, k as f64, k as u64)).collect();
    let clock = Instant::now();
    let (t, kf) = track(&frames);
    let track_s = clock.elapsed().as_secs_f64();
    let shot = |k: u32| frames.get(k as usize).map(|f| Shot { frame: f, scale, intr: &intr });

    let clock = Instant::now();
    let lp = close_loop(&kf, &t, shot).expect("graph");
    let close_s = clock.elapsed().as_secs_f64();
    assert_eq!(lp.poses.len(), n as usize);
    assert!(lp.closures >= 1, "no closure: {:?}", lp.candidates);

    let p0 = at(0.0);
    let (mut bt, mut bd, mut at_, mut ad) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (k, ((_, before), (_, after))) in kf.poses().iter().zip(&lp.poses).enumerate() {
        let truth = at(k as f64);
        let (t0, d0) = pose_error(before, &p0, &truth);
        let (t1, d1) = pose_error(after, &p0, &truth);
        bt.push(t0);
        bd.push(d0);
        at_.push(t1);
        ad.push(d1);
    }
    let last = n as usize - 1;
    let (rate_bd, rate_ad, rate_bt, rate_at) = (rate(&bd), rate(&ad), rate(&bt), rate(&at_));

    // The volume fused again at the corrected poses, and its box top.
    let clock = Instant::now();
    let t2 = reintegrate(&t, &lp.poses, shot).expect("reintegrate");
    let reint_s = clock.elapsed().as_secs_f64();
    assert_eq!(t2.frames(), n);
    let sm = extract(&t2).expect("surface");
    let pos: Vec<Vec3> = sm.mesh.positions.iter().map(|p| p0.apply_point(*p)).collect();
    let top: Vec<Vec3> =
        pos.iter().copied().filter(|p| (p.x + 80.0).abs() < 135.0 && p.y.abs() < 85.0 && p.z > 90.0 && p.z < 110.0).collect();
    assert!(top.len() > 100, "box top vertices: {}", top.len());
    let fp = fit_plane(&top, Tol { lin: 3.0, ang: 10.0 * DEG }).expect("plane");
    let normal_deg = angle_deg(fp.shape.normal, Z);

    let gb = lp.before.expect("a closure has a before gap");
    let ga = lp.after.expect("a closure has an after gap");
    println!(
        "loop: frames={n} keyframes={} candidates={} closures={} final_before={:.3}deg/{:.3}mm final_after={:.3}deg/{:.3}mm rate_before={:.4}deg/{:.4}mm rate_after={:.4}deg/{:.4}mm gap_before={:.3}deg/{:.3}mm gap_after={:.3}deg/{:.3}mm chi2={:.3}->{:.3} iters={} plane rms={:.3} normal_deg={:.3} track_s={track_s:.1} close_s={close_s:.1} reint_s={reint_s:.1}",
        lp.keyframes,
        lp.candidates.len(),
        lp.closures,
        bd[last],
        bt[last],
        ad[last],
        at_[last],
        rate_bd,
        rate_bt,
        rate_ad,
        rate_at,
        gb.deg,
        gb.mm,
        ga.deg,
        ga.mm,
        lp.chi2_before,
        lp.chi2_after,
        lp.iterations,
        fp.rms_mm,
        normal_deg
    );
    for c in &lp.candidates {
        println!("loop: candidate new={} old={} gap={:.3}deg/{:.3}mm {:?}", c.new, c.old, c.gap.deg, c.gap.mm, c.outcome);
    }

    assert!(ad[last] <= 0.5 && at_[last] <= 2.0, "final frame after closure {:.3} deg {:.3} mm", ad[last], at_[last]);
    assert!(rate_ad <= 0.005, "drift after closure {rate_ad:.4} deg/frame");
    assert!(normal_deg < 1.0, "box top normal {normal_deg} deg");
}

/// An open arc (0-60°) never returns near an old keyframe: no candidate, no closure, the poses as observed.
#[test]
fn open_arc_does_not_close() {
    let (intr, scale) = (intr(), DepthScale::D400_DEFAULT);
    let bench = Scene::bench();
    let frames: Vec<Frame> = (0..61u32).map(|k| render(&bench, k as f64, k as u64)).collect();
    let (t, kf) = track(&frames);
    let lp = close_loop(&kf, &t, |k: u32| frames.get(k as usize).map(|f| Shot { frame: f, scale, intr: &intr }))
        .expect("graph");
    assert!(lp.candidates.is_empty(), "{:?}", lp.candidates);
    assert_eq!(lp.closures, 0);
    assert_eq!((lp.before, lp.after), (None, None));
    assert_eq!(lp.poses.as_slice(), kf.poses(), "no closure leaves every pose as observed");
    println!("loop: open frames=61 keyframes={} candidates=0 closures=0", lp.keyframes);
}

/// Twelve keyframes at their TRUE poses (6° apart, 0-66°), then a thirteenth back at 0.5°: seen on the bench it
/// closes on keyframe 0; seen on a different scene from the same pose (the ground 150 mm lower, past the 100 mm pair
/// gate, and one tall box elsewhere) the ICP gate refuses it and nothing moves.
#[test]
fn false_closure_is_refused_by_the_icp_gate() {
    let (intr, scale) = (intr(), DepthScale::D400_DEFAULT);
    let bench = Scene::bench();
    let other = Scene {
        ground_centre: Vec3::new(0.0, 0.0, -150.0),
        ground: [350.0, 300.0],
        boxes: vec![BoxPrim { min: Vec3::new(-320.0, -300.0, -150.0), max: Vec3::new(-200.0, -200.0, 250.0) }],
        cylinders: Vec::new(),
    };
    let p0inv = at(0.0).invert().expect("rigid");
    let degs: Vec<f64> = (0..12).map(|k| 6.0 * k as f64).chain([0.5]).collect();
    let volume = Tsdf::new(4.0, 12.0, Tol::DEFAULT).expect("volume");

    for (scene, closes) in [(&bench, true), (&other, false)] {
        let frames: Vec<Frame> = degs
            .iter()
            .enumerate()
            .map(|(k, &d)| render(if k == 12 { scene } else { &bench }, d, k as u64))
            .collect();
        let mut kf = Keyframes::new(LoopParams::for_volume(&volume));
        for (k, &d) in degs.iter().enumerate() {
            kf.observe(k as u32, p0inv.compose(at(d)), None);
        }
        assert_eq!(kf.keyframes().len(), 13, "every truth pose is a keyframe");
        let lp = close_loop(&kf, &volume, |k: u32| frames.get(k as usize).map(|f| Shot { frame: f, scale, intr: &intr }))
            .expect("graph");
        println!("loop: false-candidate closes={closes} candidates={:?}", lp.candidates);
        assert_eq!(lp.candidates.len(), 1, "{:?}", lp.candidates);
        assert_eq!((lp.candidates[0].new, lp.candidates[0].old), (12, 0));
        if closes {
            assert_eq!(lp.closures, 1, "{:?}", lp.candidates);
            let ga = lp.after.expect("after gap");
            assert!(ga.deg <= 0.5 && ga.mm <= 2.0, "after gap {ga:?}");
        } else {
            assert_eq!(lp.closures, 0);
            assert!(
                matches!(
                    lp.candidates[0].outcome,
                    Err(Refused::Lost { .. }) | Err(Refused::Gate { .. }) | Err(Refused::Slid { .. })
                ),
                "{:?}",
                lp.candidates[0].outcome
            );
            assert_eq!(lp.poses.as_slice(), kf.poses(), "a refused candidate moves nothing");
        }
    }
}
