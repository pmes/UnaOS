// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! VUGSCANPOSE (SR478): the orbit tracked with its true poses hidden. Only names the geom shim and geom_core spell
//! alike are used here (no `normalize`, `inverse` / `invert`, `from_axes`, `Plane::distance`), so the swap leaves
//! this file as it is.

use core::f64::consts::PI;

use scan_core::fixture::{self, Noise, Scene};
use scan_core::{
    DepthScale, Frame, Tol, TrackError, Tracker, Transform, Tsdf, Vec3, extract, fit_cylinder, fit_plane, raycast,
};

const DEG: f64 = PI / 180.0;
const CENTRE: Vec3 = Vec3::new(-20.0, 0.0, 50.0);
const TOL: Tol = Tol { lin: 1e-3, ang: 1e-6 };
const X: Vec3 = Vec3::new(1.0, 0.0, 0.0);
const Y: Vec3 = Vec3::new(0.0, 1.0, 0.0);
const Z: Vec3 = Vec3::new(0.0, 0.0, 1.0);

/// The orbit's step: 1° of azimuth a frame. THE REGIME (amendment E): KinectFusion tracks a 30 Hz depth stream, where a
/// hand or a turntable moves a few millimetres and well under a degree between frames (Newcombe et al., ISMAR 2011,
/// §3.5: the small-motion assumption behind projective association; PCL KinFu's fixed 4 / 5 / 10 iterations rest on
/// it). This test's first fixture stepped 4° (about 42 mm at the 600 mm standoff) and measured projective ICP falling
/// short ALONG the motion (noise-free: 1.036 mm short at 4°, 62.7 mm off at 8°) whether or not the tracked frames were
/// integrated: outside the regime, not a tracker fault. 1° is about 10 mm a frame; the coarse-step road back is
/// VUG-SCAN-POSE-PREDICT (a constant-velocity predictor).
const STEP_DEG: f64 = 1.0;

/// Frame k's true pose: the orbit at [`STEP_DEG`] steps of azimuth.
fn truth(k: u32) -> Transform {
    fixture::orbit(CENTRE, 600.0, 40.0 * DEG, k as f64 * STEP_DEG * DEG)
}

fn render(k: u32, noise: Noise) -> Frame {
    fixture::render_depth(&Scene::bench(), &fixture::bench_intrinsics(), DepthScale::D400_DEFAULT, &truth(k), &noise, k as u64)
}

/// World → the frame of rigid `t` (through its own axes).
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

/// An estimate `e` (camera k → camera 0) against the truth P_0⁻¹·P_k: (mm, degrees).
fn pose_error(e: &Transform, p0: &Transform, pk: &Transform) -> (f64, f64) {
    let dt = (e.trans - local(p0, pk.trans)).mag();
    let mut tr = 0.0;
    for b in [X, Y, Z] {
        tr += e.apply_vector(b).dot(local_vec(p0, pk.apply_vector(b)));
    }
    (dt, libm::acos(((tr - 1.0) / 2.0).clamp(-1.0, 1.0)) / DEG)
}

/// The world's down (−z) in the axes of camera `pk`: what a perfect IMU reports, after its extrinsic.
fn down_in(pk: &Transform) -> Vec3 {
    local_vec(pk, Vec3::new(0.0, 0.0, -1.0))
}

/// What the TRACKER claims (amendment F): frame-to-model ICP holds the 2 mm / 0.5° per-frame bars over the measured
/// window (frames 0-14, amendment E's run), and over the whole 120° arc its drift is a slow, bounded RATE (the
/// least-squares slope through the origin of the per-frame error series: <= 0.05°/frame and <= 0.2 mm/frame; the
/// final frame within 5° / 15 mm). The per-frame bar over the whole arc is a loop-closure claim: the ~0.03°/frame drift
/// measured in amendment E is what VUG-SCAN-LOOP (a pose graph closing the loop) removes, not this tracker.
#[test]
fn tracked_orbit_meets_the_truth_and_the_fits() {
    let (intr, scale) = (fixture::bench_intrinsics(), DepthScale::D400_DEFAULT);
    let mut t = Tsdf::new(4.0, 12.0, Tol::DEFAULT).expect("volume");
    let mut tr = Tracker::new(&t, TOL);
    let p0 = truth(0);
    let frames = 121; // the same 0-120° arc as the 4° fixture's 31 frames
    let window = 15; // frames 0-14: the per-frame bars' window
    let (mut win_t, mut win_deg, mut fine_iters) = (0.0f64, 0.0f64, 0u32);
    let mut series: Vec<(f64, f64)> = Vec::new(); // (mm, deg) per frame
    for k in 0..frames {
        let f = render(k, Noise::D435I);
        let (e, _) =
            t.integrate_tracked(&mut tr, &f, scale, &intr, None).unwrap_or_else(|err| panic!("frame {k}: {err:?}"));
        let (dt, dd) = pose_error(&e, &p0, &truth(k));
        if k < window {
            assert!(dt <= 2.0 && dd <= 0.5, "frame {k}: {dt:.3} mm {dd:.3} deg ({:?})", tr.last());
            win_t = win_t.max(dt);
            win_deg = win_deg.max(dd);
        }
        series.push((dt, dd));
        fine_iters = fine_iters.max(tr.last().map_or(0, |l| l.level_iterations[2]));
    }
    // The drift rate: the least-squares slope through the origin, sum(k e_k) / sum(k^2).
    let (mut kt, mut kd, mut kk) = (0.0f64, 0.0f64, 0.0f64);
    for (k, (dt, dd)) in series.iter().enumerate() {
        let k = k as f64;
        kt += k * dt;
        kd += k * dd;
        kk += k * k;
    }
    let (rate_mm, rate_deg) = (kt / kk, kd / kk);
    let (final_t, final_deg) = series[series.len() - 1];

    // The mesh fused from the TRACKED poses, carried by P_0 into the scene's coordinates.
    let sm = extract(&t).expect("surface");
    assert_eq!(sm.frames, frames);
    let pos: Vec<Vec3> = sm.mesh.positions.iter().map(|p| p0.apply_point(*p)).collect();
    let nrm: Vec<Vec3> = sm.normals.iter().map(|n| p0.apply_vector(*n)).collect();

    // The box top (scan.rs's filter) and the cylinder's side (scan.rs's filter), fitted.
    let top: Vec<Vec3> =
        pos.iter().copied().filter(|p| (p.x + 80.0).abs() < 135.0 && p.y.abs() < 85.0 && p.z > 90.0 && p.z < 110.0).collect();
    assert!(top.len() > 200, "box top vertices: {}", top.len());
    let fp = fit_plane(&top, Tol { lin: 3.0, ang: 10.0 * DEG }).expect("plane");
    let normal_deg = angle_deg(fp.shape.normal, Z);
    let off = (fp.shape.normal.dot(Vec3::new(-80.0, 0.0, 100.0)) - fp.shape.d).abs();
    let (mut cp, mut cn) = (Vec::new(), Vec::new());
    for (p, q) in pos.iter().zip(&nrm) {
        if libm::hypot(p.x - 160.0, p.y) < 55.0 && p.z > 15.0 && p.z < 135.0 {
            cp.push(*p);
            cn.push(*q);
        }
    }
    assert!(cp.len() > 200, "cylinder vertices: {}", cp.len());
    let fc = fit_cylinder(&cp, &cn, Tol { lin: 3.0, ang: 15.0 * DEG }).expect("cylinder");
    let axis_deg = angle_deg(fc.shape.axis, Z);
    let s = (75.0 - fc.shape.point.z) / fc.shape.axis.z;
    let on = fc.shape.point + fc.shape.axis * s;
    let axis_off = libm::hypot(on.x - 160.0, on.y);

    println!(
        "pose: frames={} window={} win_max_t_mm={:.3} win_max_deg={:.3} drift_mm_per_frame={:.4} drift_deg_per_frame={:.4} final_t_mm={:.3} final_deg={:.3} fine_iters_max={} plane rms={:.3} normal_deg={:.3} off_mm={:.3} cyl r={:.3} axis_deg={:.3} axis_mm={:.3}",
        frames, window, win_t, win_deg, rate_mm, rate_deg, final_t, final_deg, fine_iters, fp.rms_mm, normal_deg, off,
        fc.shape.radius, axis_deg, axis_off
    );

    // The drift over the whole arc.
    assert!(rate_deg <= 0.05 && rate_mm <= 0.2, "drift {rate_mm:.4} mm/frame {rate_deg:.4} deg/frame");
    assert!(final_deg <= 5.0 && final_t <= 15.0, "final frame {final_t:.3} mm {final_deg:.3} deg");
    // The fits, at twice scan.rs's bars.
    assert!(normal_deg < 1.0, "box top normal {normal_deg} deg");
    assert!(off < 4.0, "box top height off by {off} mm");
    assert!(fp.rms_mm <= 4.0, "box top rms {}", fp.rms_mm);
    assert!(axis_deg < 2.0, "cylinder axis {axis_deg} deg");
    assert!((fc.shape.radius - 40.0).abs() < 4.0, "cylinder radius {}", fc.shape.radius);
    assert!(axis_off < 4.0, "cylinder axis {axis_off} mm off");
}

#[test]
fn no_model_and_no_overlap_are_refused_by_name() {
    let (intr, scale) = (fixture::bench_intrinsics(), DepthScale::D400_DEFAULT);
    let mut t = Tsdf::new(4.0, 12.0, Tol::DEFAULT).expect("volume");
    let mut tr = Tracker::new(&t, TOL);
    let f0 = render(0, Noise::NONE);
    assert_eq!(tr.track(&f0, scale, &intr, None), Err(TrackError::NoModel));
    t.integrate_tracked(&mut tr, &f0, scale, &intr, None).expect("frame 0");
    t.integrate_tracked(&mut tr, &render(1, Noise::NONE), scale, &intr, None).expect("frame 1");

    // A wall 200 mm in front of the camera: nothing in the model is within 100 mm of it.
    let wall = probe_core::Frame::packed(
        probe_core::StreamKind::Depth,
        probe_core::PixelFormat::Z16,
        intr.width,
        intr.height,
        0,
        2,
        200u16.to_le_bytes().repeat((intr.width * intr.height) as usize),
    )
    .unwrap();
    let (pose, last, blocks, frames) = (tr.pose(), tr.last(), t.blocks(), t.frames());
    let r = t.integrate_tracked(&mut tr, &wall, scale, &intr, None);
    assert!(matches!(r, Err(TrackError::Lost { inliers: 0, .. })), "{r:?}");
    assert_eq!((tr.pose(), tr.last()), (pose, last), "a refused track leaves the tracker as it was");
    assert_eq!((t.blocks(), t.frames()), (blocks, frames), "and the volume");
}

#[test]
fn gravity_check_names_a_disagreeing_tilt() {
    let (intr, scale) = (fixture::bench_intrinsics(), DepthScale::D400_DEFAULT);
    let mut t = Tsdf::new(4.0, 12.0, Tol::DEFAULT).expect("volume");
    let mut tr = Tracker::new(&t, TOL);
    for k in 0..4 {
        let g = down_in(&truth(k));
        t.integrate_tracked(&mut tr, &render(k, Noise::NONE), scale, &intr, Some(g))
            .unwrap_or_else(|err| panic!("frame {k}: {err:?}"));
        if k > 0 {
            let tilt = tr.last().and_then(|l| l.tilt_deg).expect("tilt measured");
            assert!(tilt < 0.5, "frame {k}: tilt {tilt} deg");
        }
    }
    // Frame 4's down, rotated 10° about the camera's x axis.
    let g = down_in(&truth(4));
    let (c, s) = (libm::cos(10.0 * DEG), libm::sin(10.0 * DEG));
    let tilted = Vec3::new(g.x, g.y * c - g.z * s, g.y * s + g.z * c);
    let (pose, last) = (tr.pose(), tr.last());
    match tr.track(&render(4, Noise::NONE), scale, &intr, Some(tilted)) {
        Err(TrackError::GravityDisagrees { deg }) => assert!(deg > 9.0 && deg < 11.0, "deg {deg}"),
        other => panic!("expected GravityDisagrees, got {other:?}"),
    }
    assert_eq!((tr.pose(), tr.last()), (pose, last), "a refused track leaves the tracker as it was");
}

#[test]
fn raycast_kat() {
    let (intr, scale) = (fixture::bench_intrinsics(), DepthScale::D400_DEFAULT);
    let p = truth(0);
    let mut t = Tsdf::new(4.0, 12.0, Tol::DEFAULT).expect("volume");
    t.integrate(&render(0, Noise::NONE), scale, &intr, &p).expect("frame");
    let m = raycast(&t, &intr, &p);
    assert_eq!((m.width, m.height, m.surfels.len()), (424, 240, 424 * 240));

    let scene = Scene::bench();
    let ray = |x: u32, y: u32| {
        let c = intr.deproject(x as f32 + 0.5, y as f32 + 0.5, 1.0).unwrap();
        p.apply_vector(Vec3::new(c[0] as f64, c[1] as f64, 1.0))
    };
    // The centre pixel sees the box top (z = 100).
    let want = scene.raycast(p.trans, ray(212, 120)).expect("the centre ray hits the scene");
    let got = m.depth_mm(212, 120).expect("the centre pixel has a surfel");
    assert!((got - want).abs() <= 1.0, "centre depth {got} vs {want}");
    let n = m.at(212, 120).unwrap().n;
    assert!(angle_deg(n, Z) < 5.0, "centre normal {n:?}");

    let (mut hit, mut hit_ok, mut miss, mut miss_ok) = (0u32, 0u32, 0u32, 0u32);
    for y in 0..intr.height {
        for x in 0..intr.width {
            let truth_hit = scene.raycast(p.trans, ray(x, y)).is_some();
            let has = m.at(x, y).is_some();
            if truth_hit {
                hit += 1;
                hit_ok += has as u32;
            } else {
                miss += 1;
                miss_ok += !has as u32;
            }
        }
    }
    assert!(hit_ok as f64 >= 0.9 * hit as f64, "surfels on {hit_ok} of {hit} scene pixels");
    assert!(miss == 0 || miss_ok as f64 >= 0.95 * miss as f64, "no surfel on {miss_ok} of {miss} empty pixels");
    println!("raycast: hits={} scene={hit} covered={hit_ok} empty={miss} clear={miss_ok}", m.hits());
}
