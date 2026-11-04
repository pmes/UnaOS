// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! VUGPHOTOGRID (LEDGER SR503): photo_core against its synthetic fixture (`photo_core::fixture`) and the steps' KATs.

use photo_core::fixture::{self, CORNER_R, HOLE_D, HOLES, PART_H, PART_T, PART_W, Spec};
use photo_core::homography::{apply, dlt};
use photo_core::{
    Measured, PhotoParams, Prim, Refusal, Tol, Vec2, contour, fit, measure, measure_lens, measure_side, measure_side_raised,
    silhouette,
};
use sketch_core::{Constraint, Entity, EntityId, PointRef, dof, profiles, solve};

fn run(s: &Spec) -> Result<Measured, Refusal> {
    let png = fixture::png(s);
    let img = pixel_core::decode(&png).expect("pixel_core decodes the fixture PNG");
    measure(&img, PhotoParams::new(5.0), Tol::DEFAULT)
}

/// The outer loop's lines, by length.
fn lines(m: &Measured) -> Vec<(Vec2, Vec2)> {
    m.shape.outer.iter().filter_map(|p| if let Prim::Line { a, b, .. } = *p { Some((a, b)) } else { None }).collect()
}

fn nearest_len(ls: &[(Vec2, Vec2)], want: f64) -> (Vec2, Vec2) {
    *ls.iter().min_by(|x, y| (x.0.dist(x.1) - want).abs().total_cmp(&(y.0.dist(y.1) - want).abs())).unwrap()
}

fn shared(a: (Vec2, Vec2), b: (Vec2, Vec2)) -> (Vec2, Vec2, Vec2) {
    // (common corner, a's other end, b's other end)
    let pairs = [(a.0, a.1, b.0, b.1), (a.0, a.1, b.1, b.0), (a.1, a.0, b.0, b.1), (a.1, a.0, b.1, b.0)];
    let best = pairs.iter().min_by(|x, y| x.0.dist(x.2).total_cmp(&y.0.dist(y.2))).unwrap();
    ((best.0 + best.2) * 0.5, best.1, best.3)
}

#[test]
fn good_photo_measures_the_part() {
    let m = run(&Spec::good()).expect("the good photo measures");
    eprintln!(
        "photo: grid={}x{} pitch_px={:.2} residual_px={:.4} residual_mm={:.4} sigma_mm={:.4} tilt_deg={:.2} lens_hint={:.4} k1={:.4} line_width_mm={:.3} inter={} inliers={} prims={:?} dof={}",
        m.grid.0, m.grid.1, m.pitch_px, m.residual_px, m.residual_mm, m.sigma_mm, m.tilt_deg, m.lens_hint, m.k1,
        m.line_width_mm, m.intersections, m.inliers, m.shape.counts(), m.solved.dof
    );
    assert!(m.sigma_mm < 0.3, "sigma_mm {}", m.sigma_mm);
    assert!(m.tilt_deg > 8.0 && m.tilt_deg < 16.0, "tilt {}", m.tilt_deg);
    assert_eq!(m.shape.counts(), (4, 1, 2), "{:?}", m.shape);
    assert_eq!(m.loops.len(), 3);
    assert_eq!(m.loops[0], m.outline);

    // The homography agrees with the truth up to the lattice's own frame: check the scale (mm per mm) instead.
    let h0 = apply(&m.h, Vec2::new(0.0, 0.0)).unwrap();
    let h1 = apply(&m.h, Vec2::new(5.0, 0.0)).unwrap();
    assert!((h0.dist(h1) - m.pitch_px).abs() < 0.25 * m.pitch_px);

    // Align the three sharp corners to the truth (2D Kabsch; a proper rotation, never a mirror).
    let ls = lines(&m);
    let l50 = nearest_len(&ls, PART_W);
    let l30 = nearest_len(&ls, PART_H);
    let (o, ex, ey) = shared(l50, l30);
    let (ux, uy) = (ex - o, ey - o);
    assert!(ux.cross(uy) > 0.0, "the measured frame is mirrored");
    let src = [o, ex, ey];
    let dst = [Vec2::new(0.0, 0.0), Vec2::new(PART_W, 0.0), Vec2::new(0.0, PART_H)];
    let cs = (src[0] + src[1] + src[2]) / 3.0;
    let cd = (dst[0] + dst[1] + dst[2]) / 3.0;
    let (mut sc, mut ss) = (0.0, 0.0);
    for k in 0..3 {
        let (a, b) = (src[k] - cs, dst[k] - cd);
        sc += a.dot(b);
        ss += a.cross(b);
    }
    let th = ss.atan2(sc);
    let to_part = |p: Vec2| {
        let q = p - cs;
        Vec2::new(th.cos() * q.x - th.sin() * q.y, th.sin() * q.x + th.cos() * q.y) + cd
    };
    for k in 0..3 {
        assert!(to_part(src[k]).dist(dst[k]) < 0.5, "corner {k}: {:?}", to_part(src[k]));
    }
    let worst = m.outline.iter().map(|&p| fixture::outline_distance(to_part(p))).fold(0.0f64, f64::max);
    assert!(worst < 0.5, "outline off the truth by {worst} mm");

    let arc_r = m.shape.outer.iter().find_map(|p| if let Prim::Arc { r, .. } = *p { Some(r) } else { None }).unwrap();
    assert!((arc_r - CORNER_R).abs() < 1.0, "corner radius {arc_r}");
    for hole in &m.shape.holes {
        let Prim::Circle { centre, r, .. } = hole[0] else { panic!("hole is not a circle: {hole:?}") };
        assert!((2.0 * r - HOLE_D).abs() < 0.5, "hole diameter {}", 2.0 * r);
        let c = to_part(centre);
        let near = HOLES.iter().map(|&(x, y)| c.dist(Vec2::new(x, y))).fold(f64::INFINITY, f64::min);
        assert!(near < 0.5, "hole centre {c:?}");
    }

    // The inferred sketch: H x2, V x2, two Coincident + two Tangent at the arc, EqualRadius: dof 10.
    assert_eq!(m.solved.dof, 10, "dof");
    assert_eq!(m.profiles.len(), 1);
    assert_eq!(m.profiles[0].holes.len(), 2);

    // Dimension it to dof 0: Fix the L50 / L30 corner, the two sides, R10, one hole's diameter, the hole centres.
    let mut sk = m.sketch.clone();
    let ents = sk.entities().to_vec();
    let id = |i: usize| EntityId(i as u32);
    let mut l_ids = Vec::new();
    let mut circles = Vec::new();
    let mut arc = None;
    for (i, e) in ents.iter().enumerate() {
        match e {
            Entity::Line { .. } => l_ids.push(id(i)),
            Entity::Circle { .. } => circles.push(id(i)),
            Entity::Arc { .. } => arc = Some(id(i)),
            _ => {}
        }
    }
    let len_of = |sk: &sketch_core::Sketch, e: EntityId| {
        sk.point(PointRef::Start(e)).unwrap().dist(sk.point(PointRef::End(e)).unwrap())
    };
    let pick = |want: f64| *l_ids.iter().min_by(|&&a, &&b| (len_of(&sk, a) - want).abs().total_cmp(&(len_of(&sk, b) - want).abs())).unwrap();
    let (s50, s30) = (pick(PART_W), pick(PART_H));
    let corner = sk.point(PointRef::Start(s50)).unwrap();
    let c50 = if sk.point(PointRef::Start(s30)).unwrap().dist(corner) < 1.0 || sk.point(PointRef::End(s30)).unwrap().dist(corner) < 1.0 {
        PointRef::Start(s50)
    } else {
        PointRef::End(s50)
    };
    let fixed = sk.point(c50).unwrap();
    sk.add(Constraint::Fix(c50, fixed)).unwrap();
    sk.add(Constraint::Distance(PointRef::Start(s50), PointRef::End(s50), PART_W)).unwrap();
    sk.add(Constraint::Distance(PointRef::Start(s30), PointRef::End(s30), PART_H)).unwrap();
    sk.add(Constraint::Radius(arc.unwrap(), CORNER_R)).unwrap();
    sk.add(Constraint::Diameter(circles[0], HOLE_D)).unwrap();
    let side = |sk: &sketch_core::Sketch, c: EntityId| sk.point(PointRef::Centre(c)).unwrap().dist(fixed);
    let (near, far) = if side(&sk, circles[0]) < side(&sk, circles[1]) { (circles[0], circles[1]) } else { (circles[1], circles[0]) };
    sk.add(Constraint::DistancePointLine(PointRef::Centre(near), s30, HOLES[0].0)).unwrap();
    sk.add(Constraint::DistancePointLine(PointRef::Centre(far), s30, HOLES[1].0)).unwrap();
    sk.add(Constraint::DistancePointLine(PointRef::Centre(near), s50, HOLES[0].1)).unwrap();
    sk.add(Constraint::DistancePointLine(PointRef::Centre(far), s50, HOLES[1].1)).unwrap();
    let solved = solve(&mut sk, Tol::DEFAULT).expect("the dimensioned sketch solves");
    assert_eq!(solved.dof, 0, "dof after ten dimensions");
    assert_eq!(dof(&sk).count, 0);
    let pr = profiles(&sk, Tol::DEFAULT);
    assert_eq!(pr.len(), 1);
    assert_eq!(pr[0].holes.len(), 2);
    assert!((pr[0].outer.area.abs() - (PART_W * PART_H - (1.0 - std::f64::consts::PI / 4.0) * CORNER_R * CORNER_R)).abs() < 1e-3);
}

#[test]
fn unsquare_refuses() {
    match run(&Spec::unsquare()) {
        Err(Refusal::GridUnsquare { deg }) => assert!(deg > 20.0, "deg {deg}"),
        other => panic!("expected GridUnsquare, got {:?}", other.map(|m| m.tilt_deg)),
    }
}

#[test]
fn no_grid_refuses() {
    match run(&Spec::no_grid()) {
        Err(Refusal::NoGrid { .. }) => {}
        other => panic!("expected NoGrid, got {:?}", other.map(|m| m.grid)),
    }
}

#[test]
fn clipped_refuses() {
    match run(&Spec::clipped()) {
        Err(Refusal::PartClipped) => {}
        other => panic!("expected PartClipped, got {:?}", other.map(|m| m.outline.len())),
    }
}

#[test]
fn dlt_kat() {
    let s = Spec::good();
    let h = fixture::homography(&s);
    let mut paper = Vec::new();
    let mut px = Vec::new();
    for i in -4..=4 {
        for j in -3..=3 {
            let p = Vec2::new(40.0 + i as f64 * 5.0, 40.0 + j as f64 * 5.0);
            paper.push(p);
            px.push(apply(&h, p).unwrap());
        }
    }
    let got = dlt(&paper, &px).unwrap();
    for (p, x) in paper.iter().zip(&px) {
        assert!(apply(&got, *p).unwrap().dist(*x) < 1e-6);
    }
}

#[test]
fn pratt_kat() {
    let pts: Vec<Vec2> = (0..40)
        .map(|k| {
            let t = 0.2 + 1.3 * k as f64 / 39.0;
            let wobble = if k % 2 == 0 { 0.02 } else { -0.02 };
            Vec2::new(3.0 + (10.0 + wobble) * t.cos(), -2.0 + (10.0 + wobble) * t.sin())
        })
        .collect();
    let c = fit::fit_circle(&pts).unwrap();
    assert!(c.c.dist(Vec2::new(3.0, -2.0)) < 0.05, "{c:?}");
    assert!((c.r - 10.0).abs() < 0.05, "{c:?}");
    assert!(c.max < 0.05);
}

#[test]
fn otsu_kat() {
    let mut hist = [0u64; 256];
    hist[40] = 300;
    hist[41] = 100;
    hist[240] = 900;
    hist[200] = 50;
    let t = silhouette::otsu(&hist);
    assert!((41..200).contains(&t), "threshold {t}");
}

#[test]
fn dp_kat() {
    // A 10 x 4 rectangle sampled every 0.5 with a 0.01 zigzag: DP at 0.1 keeps exactly the four corners.
    let mut pts = Vec::new();
    let corners = [Vec2::new(0.0, 0.0), Vec2::new(10.0, 0.0), Vec2::new(10.0, 4.0), Vec2::new(0.0, 4.0)];
    for k in 0..4 {
        let (a, b) = (corners[k], corners[(k + 1) % 4]);
        let n = (a.dist(b) / 0.5) as usize;
        for s in 0..n {
            let t = s as f64 / n as f64;
            let z = if s % 2 == 1 { 0.01 } else { 0.0 };
            pts.push(a + (b - a) * t + (b - a).perp() * (z / a.dist(b)));
        }
    }
    let kept = contour::simplify(&pts, 0.1);
    assert_eq!(kept.len(), 4, "{kept:?}");
    for c in corners {
        assert!(kept.iter().any(|k| k.dist(c) < 1e-9));
    }
}

/// VUGPHOTOLENS (SR533): the scene through a barrel lens (k1 = -0.08): the radial term recovers k1 within 10 %, the
/// 50 x 30 plate within 0.1 mm, and the flat lattice's residual is at least 3x the corrected one.
#[test]
fn lens_photo_recovers_k1() {
    let m = run(&Spec::lens()).expect("the lens photo measures");
    eprintln!(
        "photo: k1={:.4} residual_px={:.4} residual_flat_px={:.4} residual_mm={:.4} sigma_mm={:.4} lens_hint={:.4} inter={} inliers={} prims={:?}",
        m.k1, m.residual_px, m.residual_flat_px, m.residual_mm, m.sigma_mm, m.lens_hint, m.intersections, m.inliers,
        m.shape.counts()
    );
    assert!((m.k1 - (-0.08)).abs() <= 0.008, "k1 {}", m.k1);
    assert!(m.residual_flat_px >= 3.0 * m.residual_px, "flat {} vs corrected {}", m.residual_flat_px, m.residual_px);
    assert_eq!(m.shape.counts(), (4, 1, 2), "{:?}", m.shape);
    assert_eq!(m.loops.len(), 3);
    let ls = lines(&m);
    let (a50, b50) = nearest_len(&ls, PART_W);
    let (a30, b30) = nearest_len(&ls, PART_H);
    let (w, h) = (a50.dist(b50), a30.dist(b30));
    eprintln!("photo: plate={w:.3}x{h:.3}");
    assert!((w - PART_W).abs() < 0.1, "plate side {w} vs {PART_W}");
    assert!((h - PART_H).abs() < 0.1, "plate side {h} vs {PART_H}");
}

/// VUGPHOTOLENS (SR533): a perfect lens (k1 = 0) takes no term, and every number is bit-equal to the SR503 road.
#[test]
fn flat_photo_is_sr503() {
    let img = pixel_core::decode(&fixture::png(&Spec::good())).expect("pixel_core decodes the fixture PNG");
    let m = measure(&img, PhotoParams::new(5.0), Tol::DEFAULT).expect("the good photo measures");
    let f = measure_lens(&img, PhotoParams::new(5.0), Tol::DEFAULT, false).expect("the SR503 road measures");
    assert_eq!(m.k1, 0.0);
    assert_eq!(f.k1, 0.0);
    assert_eq!(m.residual_flat_px, m.residual_px);
    assert_eq!(m.outline, f.outline);
    assert_eq!(m.loops, f.loops);
    assert_eq!(m.h, f.h);
    assert_eq!(m.residual_px, f.residual_px);
    assert_eq!(m.residual_mm, f.residual_mm);
    assert_eq!(m.sigma_mm, f.sigma_mm);
    assert_eq!(m.tilt_deg, f.tilt_deg);
    assert_eq!(m.lens_hint, f.lens_hint);
    assert_eq!(m.solved.dof, f.solved.dof);
}

/// VUGPHOTOSIDE (SR545): the 50 x 30 x 6 fixture photographed on its side: the side photo through the same lattice road
/// reads the thickness 6.0 within 0.1 mm, its long dimension paired to the first outline's 50 extent.
#[test]
fn side_photo_gives_thickness() {
    let first = run(&Spec::good()).expect("the good photo measures");
    let img = pixel_core::decode(&fixture::side_png(&Spec::side(), PART_W, PART_T)).expect("pixel_core decodes the side PNG");
    let s = measure_side(&first, &img, PhotoParams::new(5.0), Tol::DEFAULT).expect("the side photo measures");
    eprintln!(
        "photo: side thickness={:.3} sigma={:.4} length={:.3} paired={:.3} prims={:?}",
        s.thickness_mm, s.sigma_mm, s.length_mm, s.paired_mm, s.side.shape.counts()
    );
    assert!((s.thickness_mm - PART_T).abs() < 0.1, "thickness {} vs {PART_T}", s.thickness_mm);
    assert!((s.paired_mm - PART_W).abs() < 0.5, "paired {} vs {PART_W}", s.paired_mm);
    assert!((s.length_mm - PART_W).abs() < 0.5, "length {} vs {PART_W}", s.length_mm);
}

/// VUGPHOTOSIDE (SR545): a side photo of another part (40 x 6) matches neither the 50 nor the 30 extent: refused by name.
#[test]
fn side_photo_of_another_part_refuses() {
    let first = run(&Spec::good()).expect("the good photo measures");
    let img = pixel_core::decode(&fixture::side_png(&Spec::side(), 40.0, PART_T)).expect("pixel_core decodes the side PNG");
    match measure_side(&first, &img, PhotoParams::new(5.0), Tol::DEFAULT) {
        Err(e) => assert_eq!(e.name(), "NoSideMatch", "{e:?}"),
        Ok(s) => panic!("expected NoSideMatch, got length {} thickness {}", s.length_mm, s.thickness_mm),
    }
}

/// VUGPHOTOSIDEPARALLAX (SR568): the raw (long, short) extents the standing side reads under `s`'s TRUE camera: the
/// footprint and its top (`z` up) scaled by k = Hc / (Hc - z) about the nadir, measured along the part's axes.
fn predicted_raw(s: &Spec, l: f64, t: f64, z: f64) -> (f64, f64) {
    let c = fixture::camera(s);
    let n = Vec2::new(c.x, c.y);
    let k = c.z / (c.z - z);
    let foot: Vec<Vec2> =
        [(0.0, 0.0), (l, 0.0), (l, t), (0.0, t)].iter().map(|&(x, y)| fixture::part_to_paper(s, Vec2::new(x, y))).collect();
    let pts: Vec<Vec2> = foot.iter().flat_map(|&p| [p, n + (p - n) * k]).collect();
    let r = s.part_rot_deg.to_radians();
    let ext = |u: Vec2| {
        let hi = pts.iter().map(|p| p.dot(u)).fold(f64::MIN, f64::max);
        let lo = pts.iter().map(|p| p.dot(u)).fold(f64::MAX, f64::min);
        hi - lo
    };
    let (a, b) = (ext(Vec2::new(r.cos(), r.sin())), ext(Vec2::new(-r.sin(), r.cos())));
    (a.max(b), a.min(b))
}

/// VUGPHOTOSIDEPARALLAX (SR568): the fixture standing on its 50 x 6 face (its top PART_H above the paper) under the
/// 400 mm camera tilted 12°: the camera self-calibrated from the side photo's lattice, the raw outline (~55.4 x 12.3)
/// corrected to 50 x 6, the same as the flat render through the SR545 road; the raw photo refuses on that road.
#[test]
fn side_parallax_corrects_raised_photo() {
    let first = run(&Spec::good()).expect("the good photo measures");
    let s = Spec::side_raised();
    let img = pixel_core::decode(&fixture::side_raised_png(&s, PART_W, PART_T, PART_H)).expect("pixel_core decodes the raised PNG");
    let r = measure_side_raised(&first, &img, PhotoParams::new(5.0), Tol::DEFAULT).expect("the raised side photo measures");
    let p = r.parallax.expect("the raised road reports its parallax");
    let truth = fixture::camera(&s);
    let (pl, pt) = predicted_raw(&s, PART_W, PART_T, PART_H);
    // The lattice's paper frame is its own: compare the nadir in photo pixels, each through its homography.
    let seen = apply(&r.side.h, p.camera.nadir).expect("the nadir maps into the photo plane");
    let want = apply(&fixture::homography(&s), Vec2::new(truth.x, truth.y)).expect("the true nadir maps");
    let px_per_mm = r.side.pitch_px / 5.0;
    let nadir_mm = seen.dist(want) / px_per_mm;
    eprintln!(
        "photo: side-parallax raw={:.3}x{:.3} predicted_raw={:.3}x{:.3} corrected={:.3}x{:.3} paired={:.3} standing={:.3} k={:.4} height={:.2} truth_height={:.2} focal={:?} nadir_off_mm={:.3} sigma={:.4} tilt={:.2}",
        p.raw_length_mm, p.raw_thickness_mm, pl, pt, r.length_mm, r.thickness_mm, r.paired_mm, p.standing_mm, p.k,
        p.camera.height_mm, truth.z, p.camera.focal_px, nadir_mm, r.sigma_mm, r.side.tilt_deg
    );
    assert!((r.thickness_mm - PART_T).abs() < 0.1, "corrected thickness {} vs {PART_T}", r.thickness_mm);
    assert!((r.length_mm - PART_W).abs() < 0.5, "corrected length {} vs {PART_W}", r.length_mm);
    assert!((r.paired_mm - PART_W).abs() < 0.5, "paired {} vs {PART_W}", r.paired_mm);
    assert!((p.raw_thickness_mm - pt).abs() < 0.3, "raw thickness {} vs predicted {pt}", p.raw_thickness_mm);
    assert!(p.raw_thickness_mm > 11.5, "the parallax is in the raw photo: {}", p.raw_thickness_mm);
    assert!((p.camera.height_mm - truth.z).abs() < 0.01 * truth.z, "height {} vs {}", p.camera.height_mm, truth.z);
    assert!(nadir_mm < 1.0, "nadir off by {nadir_mm} mm");

    let flat = pixel_core::decode(&fixture::side_png(&s, PART_W, PART_T)).expect("pixel_core decodes the side PNG");
    let f = measure_side(&first, &flat, PhotoParams::new(5.0), Tol::DEFAULT).expect("the flat side photo measures");
    eprintln!("photo: side-parallax flat thickness={:.3} length={:.3} vs corrected {:.3}", f.thickness_mm, f.length_mm, r.thickness_mm);
    assert!((f.thickness_mm - r.thickness_mm).abs() < 0.05, "flat {} vs corrected {}", f.thickness_mm, r.thickness_mm);

    match measure_side(&first, &img, PhotoParams::new(5.0), Tol::DEFAULT) {
        Err(e) => assert_eq!(e.name(), "NoSideMatch", "{e:?}"),
        Ok(x) => panic!("the uncorrected road paired a raised photo: length {} thickness {}", x.length_mm, x.thickness_mm),
    }
}

/// VUGPHOTOSIDEPARALLAX (SR568): a square camera (no tilt) gives no height from the lattice: the person's
/// `camera_mm` = 400 corrects the off-nadir strip (raw ~54.1 x 7.1) to 50 x 6, the flat render's reading; without it
/// the photo refuses NoCamera by name.
#[test]
fn side_parallax_square_camera_takes_camera_mm() {
    let first = run(&Spec::good()).expect("the good photo measures");
    let s = Spec::side_square();
    let img = pixel_core::decode(&fixture::side_raised_png(&s, PART_W, PART_T, PART_H)).expect("pixel_core decodes the raised PNG");
    let params = PhotoParams { camera_mm: Some(400.0), ..PhotoParams::new(5.0) };
    let r = measure_side_raised(&first, &img, params, Tol::DEFAULT).expect("the square raised photo measures");
    let p = r.parallax.expect("the raised road reports its parallax");
    let (pl, pt) = predicted_raw(&s, PART_W, PART_T, PART_H);
    let flat = pixel_core::decode(&fixture::side_png(&s, PART_W, PART_T)).expect("pixel_core decodes the side PNG");
    let f = measure_side(&first, &flat, PhotoParams::new(5.0), Tol::DEFAULT).expect("the flat side photo measures");
    eprintln!(
        "photo: side-parallax square camera_mm=400 raw={:.3}x{:.3} predicted_raw={:.3}x{:.3} corrected={:.3}x{:.3} k={:.4} flat={:.3} tilt={:.2}",
        p.raw_length_mm, p.raw_thickness_mm, pl, pt, r.length_mm, r.thickness_mm, p.k, f.thickness_mm, r.side.tilt_deg
    );
    assert!((r.thickness_mm - PART_T).abs() < 0.1, "corrected thickness {} vs {PART_T}", r.thickness_mm);
    assert!((p.raw_thickness_mm - pt).abs() < 0.3, "raw thickness {} vs predicted {pt}", p.raw_thickness_mm);
    assert!((f.thickness_mm - r.thickness_mm).abs() < 0.05, "flat {} vs corrected {}", f.thickness_mm, r.thickness_mm);
    assert_eq!(p.camera.focal_px, None, "a square camera is not self-calibrated");

    match measure_side_raised(&first, &img, PhotoParams::new(5.0), Tol::DEFAULT) {
        Err(e) => assert_eq!(e.name(), "NoCamera", "{e:?}"),
        Ok(x) => panic!("a square camera with no camera_mm measured: thickness {}", x.thickness_mm),
    }
}

/// VUG-PHOTO-PP (SR569): the 400 mm raised side photo cropped to the 700 x 560 window at (200, 40): its principal point
/// is (250, 320), not the frame's centre. Stated, the camera solve reads the uncropped thickness within 0.1 (and 6.0,
/// the height within 1 %); left to the centre, the f solve is wrong and the photo refuses or reads more than 0.15 off.
#[test]
fn side_parallax_cropped_photo_takes_principal_point() {
    let first = run(&Spec::good()).expect("the good photo measures");
    let s = Spec::side_raised();
    let full = fixture::render_side_raised(&s, PART_W, PART_T, PART_H);
    let whole = measure_side_raised(&first, &full, PhotoParams::new(5.0), Tol::DEFAULT).expect("the uncropped photo measures");
    let (x0, y0) = (200u32, 40u32);
    let img = fixture::crop(&full, x0, y0, 700, 560);
    assert_eq!((img.width, img.height), (700, 560));
    let pp = Vec2::new(s.width as f64 * 0.5 - x0 as f64, s.height as f64 * 0.5 - y0 as f64);
    let params = PhotoParams { principal_px: Some(pp), ..PhotoParams::new(5.0) };
    let r = measure_side_raised(&first, &img, params, Tol::DEFAULT).expect("the cropped photo measures with its principal point");
    let p = r.parallax.expect("the raised road reports its parallax");
    let truth = fixture::camera(&s);
    let centred = measure_side_raised(&first, &img, PhotoParams::new(5.0), Tol::DEFAULT);
    eprintln!(
        "photo: side-pp whole={:.3} cropped_pp={:.3} height={:.2} truth_height={:.2} focal={:?} centred={:?}",
        whole.thickness_mm,
        r.thickness_mm,
        p.camera.height_mm,
        truth.z,
        p.camera.focal_px,
        centred.as_ref().map(|c| (c.thickness_mm, c.parallax.map(|q| q.camera.height_mm))).map_err(|e| e.name())
    );
    assert!((r.thickness_mm - whole.thickness_mm).abs() < 0.1, "cropped {} vs uncropped {}", r.thickness_mm, whole.thickness_mm);
    assert!((r.thickness_mm - PART_T).abs() < 0.1, "cropped thickness {} vs {PART_T}", r.thickness_mm);
    assert!((p.camera.height_mm - truth.z).abs() < 0.01 * truth.z, "height {} vs {}", p.camera.height_mm, truth.z);
    match centred {
        Err(e) => assert_eq!(e.name(), "NoSideMatch", "{e:?}"),
        Ok(c) => assert!((c.thickness_mm - PART_T).abs() > 0.15, "the centre assumption read the crop right: {}", c.thickness_mm),
    }
}

/// VUGFACEPHOTOCAMERAPREF (SR576): the lens term about the principal point. The SR533 lens photo with the principal
/// point stated at the centre measures bit for bit as with none; the lens render cropped to SR569's 700 x 560 window at
/// (200, 40) (principal point (250, 320), the crop's centre (350, 280)) with its principal point stated reads the plate
/// within 0.1 mm and k1 within 10 % of the frame's -0.08 x (half-diagonal of the crop / of the frame)².
#[test]
fn lens_term_about_the_principal_point() {
    let s = Spec::lens();
    let img = pixel_core::decode(&fixture::png(&s)).expect("pixel_core decodes the fixture PNG");
    let none = measure(&img, PhotoParams::new(5.0), Tol::DEFAULT).expect("the lens photo measures");
    let centre = Vec2::new(img.width as f64 * 0.5, img.height as f64 * 0.5);
    let at = measure(&img, PhotoParams { principal_px: Some(centre), ..PhotoParams::new(5.0) }, Tol::DEFAULT).expect("the lens photo measures about its centre");
    assert_eq!(at.k1, none.k1);
    assert_eq!(at.h, none.h);
    assert_eq!(at.outline, none.outline);
    assert_eq!(at.loops, none.loops);
    assert_eq!(at.residual_px, none.residual_px);
    assert_eq!(at.residual_mm, none.residual_mm);
    assert_eq!(at.sigma_mm, none.sigma_mm);

    let full = fixture::render(&s);
    let (x0, y0) = (200u32, 40u32);
    let crop = fixture::crop(&full, x0, y0, 700, 560);
    assert_eq!((crop.width, crop.height), (700, 560));
    let pp = Vec2::new(s.width as f64 * 0.5 - x0 as f64, s.height as f64 * 0.5 - y0 as f64);
    let m = measure(&crop, PhotoParams { principal_px: Some(pp), ..PhotoParams::new(5.0) }, Tol::DEFAULT).expect("the cropped lens photo measures with its principal point");
    let plate = |m: &Measured| {
        let ls = lines(m);
        let (a50, b50) = nearest_len(&ls, PART_W);
        let (a30, b30) = nearest_len(&ls, PART_H);
        (a50.dist(b50), a30.dist(b30))
    };
    let ratio = (350.0f64.hypot(280.0) / 450.0f64.hypot(360.0)).powi(2);
    let want = s.k1 * ratio;
    let (w, h) = plate(&m);
    let centred = measure(&crop, PhotoParams::new(5.0), Tol::DEFAULT);
    eprintln!(
        "photo: lens-pp centre_bits=same crop_pp plate={w:.3}x{h:.3} k1={:.4} want_k1={want:.4} crop_centred {:?}",
        m.k1,
        centred.as_ref().map(|c| (plate(c), c.k1)).map_err(|e| e.name())
    );
    assert!((m.k1 - want).abs() <= 0.1 * want.abs(), "k1 {} vs {want}", m.k1);
    assert!((w - PART_W).abs() < 0.1, "plate side {w} vs {PART_W}");
    assert!((h - PART_H).abs() < 0.1, "plate side {h} vs {PART_H}");
}

// ---- VUGPHOTOCROPMETA (SR582): the crop's principal point from the photo's own tEXt ----

/// VUGPHOTOCROPMETA (SR582): SR576's cropped lens photo written by `fixture::crop_png` (the tEXt `vug:crop=200,40,900,720`)
/// and decoded by pixel_core states its principal point (250, 320) itself: measured with NO principal point stated (the
/// pref unset) it reads the plate within 0.1 mm, its k1 and outline bit-equal to the same crop measured with the point
/// stated as SR576 did.
#[test]
fn a_cropped_png_carries_its_principal_point() {
    let s = Spec::lens();
    let full = fixture::render(&s);
    let img = pixel_core::decode(&fixture::crop_png(&full, 200, 40, 700, 560)).expect("pixel_core decodes the crop PNG");
    assert_eq!((img.width, img.height), (700, 560));
    let pp = photo_core::crop_principal(&img).expect("the crop carries its meta");
    assert_eq!(pp, Vec2::new(250.0, 320.0));
    let m = measure(&img, PhotoParams::new(5.0), Tol::DEFAULT).expect("the crop measures from its own meta");
    let stated = measure(&fixture::crop(&full, 200, 40, 700, 560), PhotoParams { principal_px: Some(pp), ..PhotoParams::new(5.0) }, Tol::DEFAULT)
        .expect("the meta-free crop measures with its point stated");
    let ls = lines(&m);
    let (a50, b50) = nearest_len(&ls, PART_W);
    let (a30, b30) = nearest_len(&ls, PART_H);
    let (w, h) = (a50.dist(b50), a30.dist(b30));
    eprintln!("photo: crop-meta pp={},{} plate={w:.3}x{h:.3} k1={:.4} stated_k1={:.4}", pp.x, pp.y, m.k1, stated.k1);
    assert_eq!(m.k1, stated.k1);
    assert_eq!(m.outline, stated.outline);
    assert!((w - PART_W).abs() < 0.1, "plate side {w} vs {PART_W}");
    assert!((h - PART_H).abs() < 0.1, "plate side {h} vs {PART_H}");
}

// ---- VUGPHOTOCROPMETAITXT (SR614): the crop meta in zTXt / iTXt and a JPEG COM (named tail block) ----

/// A PNG chunk: length, kind, data, CRC-32 over kind + data (PNG §5.3).
fn meta_chunk(kind: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let mut c = (data.len() as u32).to_be_bytes().to_vec();
    c.extend_from_slice(kind);
    c.extend_from_slice(data);
    let crc = pixel_core::crc::crc32(&c[4..]);
    c.extend_from_slice(&crc.to_be_bytes());
    c
}

/// `png` (a `fixture::crop_png`, its one tEXt at byte 33 right after IHDR) with that tEXt swapped for `chunks`.
fn meta_retext(png: &[u8], chunks: &[u8]) -> Vec<u8> {
    assert_eq!(&png[37..41], b"tEXt", "crop_png writes its tEXt after IHDR");
    let len = u32::from_be_bytes([png[33], png[34], png[35], png[36]]) as usize;
    let mut out = png[..33].to_vec();
    out.extend_from_slice(chunks);
    out.extend_from_slice(&png[33 + 12 + len..]);
    out
}

/// An iTXt chunk's data: keyword NUL flag method language NUL translated-keyword NUL text (zlib when `compressed`).
fn meta_itxt(key: &str, value: &str, compressed: bool) -> Vec<u8> {
    let mut d = key.as_bytes().to_vec();
    d.extend_from_slice(&[0, compressed as u8, 0]);
    d.extend_from_slice(b"en\0");
    d.push(0);
    if compressed {
        d.extend_from_slice(&pixel_core::deflate::compress(value.as_bytes()).expect("zlib"));
    } else {
        d.extend_from_slice(value.as_bytes());
    }
    d
}

/// SR576's cropped lens photo through SR582's `crop_png` (tEXt `vug:crop=200,40,900,720`), decoded: the tEXt road.
fn meta_crop() -> (Vec<u8>, pixel_core::Image) {
    let full = fixture::render(&Spec::lens());
    let png = fixture::crop_png(&full, 200, 40, 700, 560);
    let img = pixel_core::decode(&png).expect("the tEXt crop decodes");
    (png, img)
}

/// VUGPHOTOCROPMETAITXT (SR614): the crop meta in a zTXt (keyword NUL 0 zlib) decodes to the SAME image and pairs as the
/// tEXt road, so the photo opens cropped: its principal point (250, 320), which `measure`'s `principal_of` takes first.
#[test]
fn a_ztxt_crop_opens_cropped() {
    let (png, text_img) = meta_crop();
    let mut d = photo_core::CROP_KEY.as_bytes().to_vec();
    d.extend_from_slice(&[0, 0]);
    d.extend_from_slice(&pixel_core::deflate::compress(b"200,40,900,720").expect("zlib"));
    let img = pixel_core::decode(&meta_retext(&png, &meta_chunk(b"zTXt", &d))).expect("the zTXt crop decodes");
    assert_eq!(img.text, text_img.text);
    assert_eq!(img.rgba, text_img.rgba);
    let pp = photo_core::crop_principal(&img).expect("the zTXt carries the crop");
    assert_eq!(pp, Vec2::new(250.0, 320.0));
    eprintln!("photo: crop-meta-itxt ztxt={},{}", pp.x, pp.y);
}

/// VUGPHOTOCROPMETAITXT (SR614): the crop meta in an iTXt, uncompressed (flag 0) and compressed (flag 1, the one
/// inflater), opens cropped at (250, 320), the pairs the tEXt road's.
#[test]
fn an_itxt_crop_opens_cropped_plain_and_compressed() {
    let (png, text_img) = meta_crop();
    for compressed in [false, true] {
        let c = meta_chunk(b"iTXt", &meta_itxt(photo_core::CROP_KEY, "200,40,900,720", compressed));
        let img = pixel_core::decode(&meta_retext(&png, &c)).expect("the iTXt crop decodes");
        assert_eq!(img.text, text_img.text, "compressed={compressed}");
        assert_eq!(img.rgba, text_img.rgba);
        let pp = photo_core::crop_principal(&img).expect("the iTXt carries the crop");
        assert_eq!(pp, Vec2::new(250.0, 320.0), "compressed={compressed}");
        eprintln!("photo: crop-meta-itxt itxt compressed={compressed} pp={},{}", pp.x, pp.y);
    }
}

/// An 8 x 8 grey baseline JPEG built from bytes with `com` as a COM segment: SOI, COM, SOF0 (8-bit, 8 x 8, one
/// component, table 0), SOS on the decoder's Annex K default tables (no DHT), one entropy byte 0x2B (DC category 0
/// `00`, AC EOB `1010`, padded with 1s), EOI.
fn meta_jpeg(com: &[u8]) -> Vec<u8> {
    let mut j = vec![0xFF, 0xD8, 0xFF, 0xFE];
    j.extend_from_slice(&((com.len() + 2) as u16).to_be_bytes());
    j.extend_from_slice(com);
    j.extend_from_slice(&[0xFF, 0xC0, 0, 11, 8, 0, 8, 0, 8, 1, 1, 0x11, 0]);
    j.extend_from_slice(&[0xFF, 0xDA, 0, 8, 1, 1, 0x00, 0, 63, 0]);
    j.extend_from_slice(&[0x2B, 0xFF, 0xD9]);
    j
}

/// VUGPHOTOCROPMETAITXT (SR614): a JPEG whose COM carries the tEXt chunk's bytes (`vug:crop` NUL `200,40,900,720`) opens
/// cropped at (250, 320); a plain comment (no NUL) surfaces no pair.
#[test]
fn a_jpeg_com_crop_opens_cropped() {
    let img = pixel_core::decode(&meta_jpeg(b"vug:crop\x00200,40,900,720")).expect("the hand-built JPEG decodes");
    assert_eq!((img.width, img.height), (8, 8));
    let pp = photo_core::crop_principal(&img).expect("the COM carries the crop");
    assert_eq!(pp, Vec2::new(250.0, 320.0));
    let plain = pixel_core::decode(&meta_jpeg(b"made by a camera")).expect("the plain-comment JPEG decodes");
    assert!(plain.text.is_empty());
    assert_eq!(photo_core::crop_principal(&plain), None);
    eprintln!("photo: crop-meta-itxt jpeg={},{}", pp.x, pp.y);
}

/// VUGPHOTOCROPMETAITXT (SR614): a PNG carrying the crop twice — the FIRST `vug:crop` in file order wins whatever chunk
/// carries it (`crop_principal`'s `find` over the one `Image.text`): tEXt then iTXt reads the tEXt's (250, 320), iTXt then
/// tEXt the iTXt's (350, 280).
#[test]
fn the_first_crop_in_the_file_wins_whatever_its_chunk() {
    let (png, _) = meta_crop();
    let mut t = photo_core::CROP_KEY.as_bytes().to_vec();
    t.push(0);
    t.extend_from_slice(b"200,40,900,720");
    let text = meta_chunk(b"tEXt", &t);
    let itxt = meta_chunk(b"iTXt", &meta_itxt(photo_core::CROP_KEY, "0,0,700,560", false));
    let first_text = pixel_core::decode(&meta_retext(&png, &[text.clone(), itxt.clone()].concat())).expect("decodes");
    let first_itxt = pixel_core::decode(&meta_retext(&png, &[itxt, text].concat())).expect("decodes");
    assert_eq!(first_text.text.len(), 2);
    let a = photo_core::crop_principal(&first_text).expect("crop");
    let b = photo_core::crop_principal(&first_itxt).expect("crop");
    assert_eq!(a, Vec2::new(250.0, 320.0));
    assert_eq!(b, Vec2::new(350.0, 280.0));
    eprintln!("photo: crop-meta-itxt first={},{}/{},{}", a.x, a.y, b.x, b.y);
}

// ---- VUGPHOTOCROPMETAXMP (SR631): the crop meta in an XMP packet (named tail block) ----

/// An XMP packet (the `<?xpacket?>` wrapper, `x:xmpmeta`, `rdf:RDF`) around one `rdf:Description` whose attributes are
/// `attrs` and whose children are `body`; the `vug` and `other` prefixes are bound on `x:xmpmeta`.
fn xmp_packet(attrs: &str, body: &str) -> String {
    format!(
        "<?xpacket begin=\"\u{feff}\" id=\"W5M0MpCehiHzreSzNTczkc9d\"?>\n<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" \
         xmlns:vug=\"urn:example:vug\" xmlns:other=\"urn:example:other\"><rdf:RDF \
         xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" {attrs}>{body}\
         </rdf:Description></rdf:RDF></x:xmpmeta>\n    \n<?xpacket end=\"w\"?>"
    )
}

/// SR614's hand-built 8 x 8 JPEG (an empty COM, which surfaces no pair) with `app1` spliced in after SOI as an APP1.
fn xmp_jpeg(app1: &[u8]) -> Vec<u8> {
    let base = meta_jpeg(b"");
    let mut j = vec![0xFF, 0xD8, 0xFF, 0xE1];
    j.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
    j.extend_from_slice(app1);
    j.extend_from_slice(&base[2..]);
    j
}

/// VUGPHOTOCROPMETAXMP (SR631): a JPEG whose APP1 carries an XMP packet with `vug:crop` opens cropped at (250, 320),
/// the property as an `rdf:Description` attribute or as its child element; a packet without it (another prefix's
/// `crop`, other properties) and an extended-XMP APP1 surface no crop.
#[test]
fn a_jpeg_xmp_crop_opens_cropped() {
    let app1 = |packet: &str| [&b"http://ns.adobe.com/xap/1.0/\0"[..], packet.as_bytes()].concat();
    let attr = pixel_core::decode(&xmp_jpeg(&app1(&xmp_packet("vug:crop=\"200,40,900,720\"", "")))).expect("decodes");
    assert_eq!((attr.width, attr.height), (8, 8));
    assert_eq!(attr.text.len(), 1);
    assert_eq!(attr.text[0].0, pixel_core::png::XMP_KEY);
    let pp = photo_core::crop_principal(&attr).expect("the XMP attribute carries the crop");
    assert_eq!(pp, Vec2::new(250.0, 320.0));
    let elem = pixel_core::decode(&xmp_jpeg(&app1(&xmp_packet("", "<vug:crop> 200,40,900,720 </vug:crop>")))).expect("decodes");
    assert_eq!(photo_core::crop_principal(&elem), Some(Vec2::new(250.0, 320.0)));
    let none = xmp_packet("other:crop=\"0,0,700,560\" other:CreatorTool=\"a camera\"", "<other:crop>0,0,700,560</other:crop>");
    let keyless = pixel_core::decode(&xmp_jpeg(&app1(&none))).expect("decodes");
    assert_eq!(keyless.text.len(), 1);
    assert_eq!(photo_core::crop_principal(&keyless), None);
    let ext = [&b"http://ns.adobe.com/xmp/extension/\0"[..], xmp_packet("vug:crop=\"200,40,900,720\"", "").as_bytes()].concat();
    let extended = pixel_core::decode(&xmp_jpeg(&ext)).expect("decodes");
    assert!(extended.text.is_empty());
    assert_eq!(photo_core::crop_principal(&extended), None);
    eprintln!("photo: crop-meta-xmp jpeg={},{}", pp.x, pp.y);
}

/// pixel_core's lossless WebP KAT fixture (VP8X + ICCP + VP8L) with the VP8X XMP flag set and an `XMP ` chunk carrying
/// `packet` appended after the image (RFC 9649 §2.7's order), the RIFF size rewritten.
fn xmp_webp(packet: &str) -> Vec<u8> {
    let mut w = include_bytes!("../../../media/pixel_core/tests/fixtures/webp/pal8v5-ll.webp").to_vec();
    assert_eq!(&w[12..16], b"VP8X");
    w[20] |= 0x04;
    w.extend_from_slice(b"XMP ");
    w.extend_from_slice(&(packet.len() as u32).to_le_bytes());
    w.extend_from_slice(packet.as_bytes());
    if packet.len() & 1 == 1 {
        w.push(0);
    }
    let riff = (w.len() - 8) as u32;
    w[4..8].copy_from_slice(&riff.to_le_bytes());
    w
}

/// VUGPHOTOCROPMETAXMP (SR631): a WebP whose `XMP ` chunk (after the image data) carries `vug:crop` opens cropped at
/// (250, 320) with the same pixels as the plain file; a packet without it surfaces no crop.
#[test]
fn a_webp_xmp_crop_opens_cropped() {
    let plain = pixel_core::decode(include_bytes!("../../../media/pixel_core/tests/fixtures/webp/pal8v5-ll.webp")).expect("the KAT decodes");
    assert!(plain.text.is_empty());
    let img = pixel_core::decode(&xmp_webp(&xmp_packet("vug:crop=\"200,40,900,720\"", ""))).expect("the XMP WebP decodes");
    assert_eq!((img.width, img.height), (plain.width, plain.height));
    assert_eq!(img.rgba, plain.rgba);
    assert_eq!(img.text.len(), 1);
    assert_eq!(img.text[0].0, pixel_core::png::XMP_KEY);
    let pp = photo_core::crop_principal(&img).expect("the XMP chunk carries the crop");
    assert_eq!(pp, Vec2::new(250.0, 320.0));
    let keyless = pixel_core::decode(&xmp_webp(&xmp_packet("other:crop=\"0,0,700,560\"", ""))).expect("decodes");
    assert_eq!(keyless.text.len(), 1);
    assert_eq!(photo_core::crop_principal(&keyless), None);
    eprintln!("photo: crop-meta-xmp webp={},{}", pp.x, pp.y);
}

// ---- VUGFACEPHOTOCROPPREFFRAC (SR648): the crop pair's optional principal point (named tail block) ----

/// The lens photo's crop 200,40 700x560 carrying `value` as its one `vug:crop` pair, and the meta-free crop itself.
fn frac_crop(value: &str) -> (pixel_core::Image, pixel_core::Image) {
    let full = fixture::render(&Spec::lens());
    let bare = fixture::crop(&full, 200, 40, 700, 560);
    let mut img = bare.clone();
    img.text = vec![(String::from(photo_core::CROP_KEY), String::from(value))];
    (img, bare)
}

/// VUGFACEPHOTOCROPPREFFRAC (SR648): a 6-field pair `200,40,900,720,446.5,357.25` keeps the 4-field frame and states
/// the carried point less the offset, (246.5, 317.25), which `measure` takes with no pref, bit-equal (k1, outline) to the
/// meta-free crop measured with that point stated; a crop of it carries the point unchanged and ignores a pref (the
/// meta outranks it); a 5-field pair and a non-finite point are no crop.
#[test]
fn a_six_field_crop_carries_its_principal_point() {
    let (img, bare) = frac_crop("200,40,900,720,446.5,357.25");
    assert_eq!(photo_core::crop_frame(&img), Some([200, 40, 900, 720]));
    assert_eq!(photo_core::crop_meta(&img), Some(([200, 40, 900, 720], Some(Vec2::new(446.5, 357.25)))));
    let pp = photo_core::crop_principal(&img).expect("the six-field pair carries the point");
    assert_eq!(pp, Vec2::new(246.5, 317.25));
    let m = measure(&img, PhotoParams::new(5.0), Tol::DEFAULT).expect("the crop measures from its own point");
    let stated = measure(&bare, PhotoParams { principal_px: Some(pp), ..PhotoParams::new(5.0) }, Tol::DEFAULT).expect("the meta-free crop measures with the point stated");
    assert_eq!(m.k1, stated.k1);
    assert_eq!(m.outline, stated.outline);
    let (k, v) = photo_core::crop_pair_with(&img, 10, 5, Some(Vec2::new(1.0, 2.0)));
    assert_eq!((k.as_str(), v.as_str()), (photo_core::CROP_KEY, "210,45,900,720,446.5,357.25"), "the point carried, the pref outranked");
    assert_eq!(photo_core::crop_pair(&img, 10, 5).1, "210,45,900,720,446.5,357.25");
    assert_eq!(photo_core::crop_frame(&frac_crop("200,40,900,720,446.5").0), None, "five fields are no crop");
    assert_eq!(photo_core::crop_principal(&frac_crop("200,40,900,720,NaN,357.25").0), None, "a non-finite point is no crop");
    assert_eq!(photo_core::crop_principal(&frac_crop("200,40,900,720,446.5,357.25,1").0), None, "seven fields are no crop");
    eprintln!("photo: crop-pref-frac six={},{} k1={:.4} stated_k1={:.4}", pp.x, pp.y, m.k1, stated.k1);
}

/// VUGFACEPHOTOCROPPREFFRAC (SR648): a 4-field pair reads as SR582 did (the frame's centre less the offset, (250, 320)),
/// and the writer with no point stated writes the 4-field pair byte for byte as `crop_pair` (the fixture's road); with
/// a point stated on a meta-free photo it appends the point as written.
#[test]
fn a_four_field_crop_reads_as_before() {
    let (img, bare) = frac_crop("200,40,900,720");
    assert_eq!(photo_core::crop_meta(&img), Some(([200, 40, 900, 720], None)));
    assert_eq!(photo_core::crop_principal(&img), Some(Vec2::new(250.0, 320.0)));
    let full = fixture::render(&Spec::lens());
    assert_eq!(photo_core::crop_pair_with(&full, 200, 40, None), photo_core::crop_pair(&full, 200, 40));
    assert_eq!(photo_core::crop_pair(&full, 200, 40).1, "200,40,900,720");
    assert_eq!(photo_core::crop_pair_with(&full, 200, 40, Some(Vec2::new(446.5, 357.25))).1, "200,40,900,720,446.5,357.25");
    assert_eq!(photo_core::crop_pair_with(&img, 10, 5, Some(Vec2::new(446.5, 357.25))).1, "210,45,900,720", "a 4-field meta outranks the pref");
    assert_eq!(photo_core::crop_pair_with(&bare, 0, 0, Some(Vec2::new(f64::NAN, 1.0))).1, "0,0,700,560", "a non-finite point is not written");
    eprintln!("photo: crop-pref-frac four=250,320");
}

/// VUGPHOTOSIDETHIN (SR651): a 0.5 mm shim off the plate's left side pairs like an ink stroke but lies off the lattice,
/// so it is a thin side of the part: its outline is in the loops at its width and length; the plate stays on the truth,
/// and the plain plate has nothing past its left side.
#[test]
fn a_thin_side_off_the_lattice_is_kept() {
    let s = Spec::good();
    let truth_inv = photo_core::homography::invert(&fixture::homography(&s)).expect("the true homography inverts");
    let to_part = |m: &Measured, p: Vec2| -> Vec2 {
        let q = apply(&m.h, p).expect("h maps the outline");
        fixture::paper_to_part(&s, apply(&truth_inv, q).expect("the truth maps back"))
    };
    let img = pixel_core::decode(&fixture::shim_png(&s)).expect("pixel_core decodes the shim PNG");
    let m = measure(&img, PhotoParams::new(5.0), Tol::DEFAULT).expect("the shim photo measures");
    let pts: Vec<Vec2> = m.loops[0].iter().map(|&p| to_part(&m, p)).collect();
    // VUGPHOTOSIDETHIN (SR651) diagnostic: the grid the measurement sees, through the public steps (the flat road; k1 below).
    {
        use photo_core::{edges, grid, homography};
        let gray = edges::luma(&img).expect("luma");
        let el = edges::edgels(&gray);
        let lines = grid::hough_lines(&el, gray.w, gray.h);
        let g = grid::find(&el, gray.w, gray.h).expect("grid");
        let lat = homography::fit_lattice(&g, 5.0, gray.w, gray.h).expect("lattice");
        let hi = homography::invert(&lat.h).expect("H inverts");
        let wmm = g.width_px / lat.px_per_mm;
        let paper = |q: Vec2| apply(&hi, q).unwrap_or(Vec2::new(f64::NAN, f64::NAN));
        let off = g.pairs.iter().filter(|p| photo_core::off_lattice(p, &hi, 5.0, wmm)).count();
        eprintln!("diag: k1={:.4} edge_lines={} pairs={} off_pairs={} width_mm={:.3} min_votes={}", m.k1, lines.len(), g.pairs.len(), off, wmm, (gray.w.min(gray.h) / 8).max(8));
        for p in &g.pairs {
            let (t0, t1) = (p.support[0], p.support[p.support.len() - 1]);
            let (a, b) = (paper(p.point + p.dir * t0), paper(p.point + p.dir * t1));
            eprintln!("diag pair: paper ({:.2},{:.2})-({:.2},{:.2}) width_px={:.2} support={} off={}", a.x, a.y, b.x, b.y, p.width_px, p.support.len(), photo_core::off_lattice(p, &hi, 5.0, wmm));
        }
        let r = (g.pitch_px / 5.0).round().clamp(4.0, 20.0);
        let sil = photo_core::silhouette::extract(&gray, &lat, 5.0, wmm, r).expect("silhouette");
        let before = sil.part.iter().filter(|&&p| p).count();
        let after = photo_core::thin_sides(sil.clone(), &gray, &lat, &g.pairs, 5.0, wmm).map(|t| t.part.iter().filter(|&&p| p).count());
        eprintln!("diag sil: r={r} threshold={} part_before={before} part_after={after:?} w={} h={}", sil.threshold, sil.w, sil.h);
        for pr in g.pairs.iter().filter(|p| photo_core::off_lattice(p, &hi, 5.0, wmm)) {
            let n = pr.support.len();
            eprintln!("diag off pair: point=({:.1},{:.1}) dir=({:.3},{:.3}) width_px={:.2} support q0={:.1} q25={:.1} q50={:.1} q75={:.1} q100={:.1}", pr.point.x, pr.point.y, pr.dir.x, pr.dir.y, pr.width_px, pr.support[0], pr.support[n / 4], pr.support[n / 2], pr.support[3 * n / 4], pr.support[n - 1]);
            let gaps: Vec<String> = pr.support.windows(2).filter(|w| w[1] - w[0] > 2.0).map(|w| format!("{:.1}..{:.1}", w[0], w[1])).collect();
            eprintln!("diag gaps>2px: {}", gaps.join(" "));
            for k in 0..=24 {
                let t = 108.0 + k as f64;
                let q = pr.point + pr.dir * t;
                let pp = paper(q);
                let (u, v) = (((pp.x - sil.x0) * sil.r) as i64, ((pp.y - sil.y0) * sil.r) as i64);
                let at = |du: i64, dv: i64| -> char {
                    let (uu, vv) = (u + du, v + dv);
                    if uu < 0 || vv < 0 || uu as usize >= sil.w || vv as usize >= sil.h { return '?' }
                    let i = vv as usize * sil.w + uu as usize;
                    if sil.part[i] { 'P' } else if sil.hole[i] != 0 { 'H' } else { '.' }
                };
                let nb: String = (-1..=1).flat_map(|dv| (-1..=1).map(move |du| (du, dv))).map(|(du, dv)| at(du, dv)).collect();
                eprintln!("diag joint: t={t:.0} rect=({u},{v}) luma={:?} part3x3={nb}", gray.sample(q.x, q.y));
            }
            for k in 0..=24 {
                let y = fixture::SHIM_Y - 1.5 + k as f64 * 0.125;
                let q = apply(&fixture::homography(&s), fixture::part_to_paper(&s, Vec2::new(-10.0, y))).expect("q");
                let pp = paper(q);
                let (u, v) = (((pp.x - sil.x0) * sil.r) as i64, ((pp.y - sil.y0) * sil.r) as i64);
                let part = u >= 0 && v >= 0 && (u as usize) < sil.w && (v as usize) < sil.h && sil.part[v as usize * sil.w + u as usize];
                let d = q - pr.point;
                eprintln!("diag x-sec: part_y={y:.3} px=({:.1},{:.1}) luma={:?} perp={:.2} t={:.1} rect=({u},{v}) part={part}", q.x, q.y, gray.sample(q.x, q.y), d.dot(pr.dir.perp()), d.dot(pr.dir));
            }
        }
        let shim_px = (apply(&fixture::homography(&s), fixture::part_to_paper(&s, Vec2::new(-10.0, fixture::SHIM_Y))).expect("shim"), fixture::homography(&s));
        for l in &lines {
            let d = (l.n.dot(shim_px.0) - l.rho).abs();
            if d < 25.0 {
                eprintln!("diag line near shim: n=({:.3},{:.3}) rho={:.1} dist_px={:.2} support={}", l.n.x, l.n.y, l.rho, d, l.pts.len());
            }
        }
    }
    let tab: Vec<Vec2> = pts.iter().copied().filter(|q| q.x < -1.0).collect();
    let band = fit::BAND_SIGMA * m.sigma_mm;
    assert!(tab.len() >= 2, "no shim vertices in the loop: {pts:?}");
    let (ylo, yhi) = tab.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |a, q| (a.0.min(q.y), a.1.max(q.y)));
    let tip = tab.iter().map(|q| q.x).fold(f64::INFINITY, f64::min);
    eprintln!("photo: shim width={:.3} tip={:.3} y={:.3}..{:.3} sigma={:.4} k1={:.4}", yhi - ylo, tip, ylo, yhi, m.sigma_mm, m.k1);
    assert!(((yhi - ylo) - fixture::SHIM_W).abs() <= band, "shim width {} vs {}", yhi - ylo, fixture::SHIM_W);
    assert!((tip + fixture::SHIM_L).abs() <= band, "shim tip {tip} vs {}", -fixture::SHIM_L);
    assert!(((ylo + yhi) * 0.5 - fixture::SHIM_Y).abs() <= band, "shim centre {}", (ylo + yhi) * 0.5);
    let worst = pts.iter().filter(|q| q.x > 0.5).map(|&q| fixture::outline_distance(q)).fold(0.0f64, f64::max);
    assert!(worst < 0.5, "plate outline off the truth by {worst} mm");
    let plain = run(&s).expect("the good photo measures");
    assert!(plain.loops[0].iter().all(|&p| to_part(&plain, p).x >= -1.0), "the plain plate has a vertex past its left side");
}

/// VUGPHOTOSIDETHINCROSS (SR654): a 0.5 mm shim lying ON the lattice line x = 45 refines into the line's ink pair (no pair of
/// its own), so the tone tell keeps it: its stretch is darker than the sheet's ink by more than K spreads, no ink stretch
/// trips, and its outline is in the loop at its width, centre and tip; the plate stays on the truth.
#[test]
fn a_thin_side_on_a_lattice_line_is_kept() {
    use photo_core::{edges, grid, homography};
    let s = Spec::good();
    let truth_inv = homography::invert(&fixture::homography(&s)).expect("the true homography inverts");
    let img = pixel_core::decode(&fixture::shim_on_line_png(&s)).expect("pixel_core decodes the on-line shim PNG");
    // The rule through the public steps (the flat road: Spec::good has k1 = 0).
    let gray = edges::luma(&img).expect("luma");
    let el = edges::edgels(&gray);
    let g = grid::find(&el, gray.w, gray.h).expect("grid");
    let lat = homography::fit_lattice(&g, 5.0, gray.w, gray.h).expect("lattice");
    let hi = homography::invert(&lat.h).expect("H inverts");
    let wmm = g.width_px / lat.px_per_mm;
    let r = (g.pitch_px / 5.0).round().clamp(4.0, 20.0);
    let sil = silhouette::extract(&gray, &lat, 5.0, wmm, r).expect("silhouette");
    let (ink, spread, cut) = photo_core::ink_cut(&g.pairs, &gray, &sil, &hi, 5.0, wmm).expect("the on-lattice pairs give an ink tone");
    let min_px = 5.0 / 8.0 * lat.px_per_mm;
    let plate_bottom = 23.7 + (fixture::SHIM_ON_X - 33.3) * 1.5f64.to_radians().tan();
    let (mut stretches, mut on_shim, mut core) = (0usize, 0usize, Vec::new());
    for p in g.pairs.iter().filter(|p| !photo_core::off_lattice(p, &hi, 5.0, wmm)) {
        for (t0, t1) in photo_core::dark_pair(p, &gray, &sil, &hi, cut, min_px) {
            stretches += 1;
            let a = apply(&truth_inv, p.point + p.dir * t0).expect("the truth maps the stretch");
            let b = apply(&truth_inv, p.point + p.dir * t1).expect("the truth maps the stretch");
            eprintln!("diag stretch: paper ({:.2},{:.2})-({:.2},{:.2}) len_px={:.1}", a.x, a.y, b.x, b.y, t1 - t0);
            let near = |q: Vec2| (q.x - fixture::SHIM_ON_X).abs() <= 1.5 && q.y >= fixture::SHIM_ON_TIP - 1.0 && q.y <= plate_bottom + 1.0;
            assert!(near(a) && near(b), "an ink stretch tripped the tone tell: ({},{})-({},{})", a.x, a.y, b.x, b.y);
            if (a.y - b.y).abs() > 5.0 {
                on_shim += 1;
                let n = (t1 - t0) as usize;
                core.extend((0..=n).filter_map(|k| gray.sample((p.point + p.dir * (t0 + k as f64)).x, (p.point + p.dir * (t0 + k as f64)).y)).map(f64::from));
            }
        }
    }
    core.sort_by(|a, b| a.total_cmp(b));
    let core = core.get(core.len() / 2).copied().unwrap_or(f64::NAN);
    assert!(on_shim >= 1, "no dark stretch along the shim on x = {}", fixture::SHIM_ON_X);
    assert!(core < cut && cut < ink, "core {core} cut {cut} ink {ink}");
    let m = measure(&img, PhotoParams::new(5.0), Tol::DEFAULT).expect("the on-line shim photo measures");
    let pts: Vec<Vec2> = m.loops[0]
        .iter()
        .map(|&p| apply(&truth_inv, apply(&m.h, p).expect("h maps the outline")).expect("the truth maps back"))
        .collect();
    let tab: Vec<Vec2> = pts.iter().copied().filter(|q| q.y < plate_bottom - 1.0).collect();
    let band = fit::BAND_SIGMA * m.sigma_mm;
    assert!(tab.len() >= 2, "no shim vertices in the loop: {pts:?}");
    let (xlo, xhi) = tab.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |a, q| (a.0.min(q.x), a.1.max(q.x)));
    let tip = tab.iter().map(|q| q.y).fold(f64::INFINITY, f64::min);
    eprintln!(
        "photo: shim-on-line ink={ink:.1} spread={spread:.2} cut={cut:.1} core={core:.1} otsu={} stretches={stretches} width={:.3} tip={tip:.3} x={:.3} sigma={:.4}",
        sil.threshold,
        xhi - xlo,
        (xlo + xhi) * 0.5,
        m.sigma_mm
    );
    assert!(((xhi - xlo) - fixture::SHIM_W).abs() <= band, "shim width {} vs {}", xhi - xlo, fixture::SHIM_W);
    assert!(((xlo + xhi) * 0.5 - fixture::SHIM_ON_X).abs() <= band, "shim centre {}", (xlo + xhi) * 0.5);
    assert!((tip - fixture::SHIM_ON_TIP).abs() <= band, "shim tip {tip} vs {}", fixture::SHIM_ON_TIP);
    let worst = pts
        .iter()
        .filter(|q| q.y >= plate_bottom - 1.0 && (q.x - fixture::SHIM_ON_X).abs() > 1.0)
        .map(|&q| fixture::outline_distance(fixture::paper_to_part(&s, q)))
        .fold(0.0f64, f64::max);
    assert!(worst < 0.5, "plate outline off the truth by {worst} mm");
}

// ---- VUGPHOTOSIDETHINFIT (SR655): the shim's two sides fit as two parallel lines and its end (named tail block) ----
#[test]
fn a_thin_side_fits_two_parallel_lines() {
    let s = Spec::good();
    let truth_inv = photo_core::homography::invert(&fixture::homography(&s)).expect("the true homography inverts");
    let to_part = |m: &Measured, p: Vec2| -> Vec2 {
        let q = apply(&m.h, p).expect("h maps the outline");
        fixture::paper_to_part(&s, apply(&truth_inv, q).expect("the truth maps back"))
    };
    let img = pixel_core::decode(&fixture::shim_png(&s)).expect("pixel_core decodes the shim PNG");
    let m = measure(&img, PhotoParams::new(5.0), Tol::DEFAULT).expect("the shim photo measures");
    let band = fit::BAND_SIGMA * m.sigma_mm;
    // Every outer line in paper mm and in part mm.
    let ls: Vec<((Vec2, Vec2), (Vec2, Vec2))> = m
        .shape
        .outer
        .iter()
        .filter_map(|p| match *p {
            Prim::Line { a, b, .. } => Some(((a, b), (to_part(&m, a), to_part(&m, b)))),
            _ => None,
        })
        .collect();
    eprintln!("photo: thin fit outer={:?}", ls.iter().map(|l| l.1).collect::<Vec<_>>());
    let sides: Vec<&((Vec2, Vec2), (Vec2, Vec2))> =
        ls.iter().filter(|l| (l.1.0 + l.1.1).x * 0.5 < -1.0 && l.1.0.dist(l.1.1) >= fixture::SHIM_L * 0.5).collect();
    assert_eq!(sides.len(), 2, "the shim's sides: {:?}", ls.iter().map(|l| l.1).collect::<Vec<_>>());
    let (s0, s1) = (sides[0].1, sides[1].1);
    let (u, v) = (s0.1 - s0.0, s1.1 - s1.0);
    assert!(u.dot(v) < 0.0, "the sides run back against each other: {u:?} {v:?}");
    let (uh, vh) = (u / u.mag(), v / v.mag());
    assert!(uh.cross(vh).abs() * u.mag().min(v.mag()) <= band, "the sides are not parallel within the band: {u:?} {v:?}");
    let sep = ((s1.0 + s1.1) * 0.5 - s0.0).cross(uh).abs();
    assert!((sep - fixture::SHIM_W).abs() <= 2.0 * m.sigma_mm, "side separation {sep} vs {}", fixture::SHIM_W);
    let ends: Vec<(Vec2, Vec2)> =
        ls.iter().map(|l| l.1).filter(|l| ((l.0 + l.1).x * 0.5 + fixture::SHIM_L).abs() <= band).collect();
    assert_eq!(ends.len(), 1, "the shim's end: {ends:?}");
    let end = ends[0].0.dist(ends[0].1);
    assert!((end - fixture::SHIM_W).abs() <= band, "end length {end} vs {}", fixture::SHIM_W);
    let tip = ends[0].0.x.min(ends[0].1.x);
    // The spike: a line from the tip to the plate that is not one of the two parallel sides (the merged end's diagonal).
    let spans = ls.iter().filter(|l| l.1.0.x.min(l.1.1.x) < -fixture::SHIM_L * 0.5 && l.1.0.x.max(l.1.1.x) > -1.0).count();
    assert_eq!(spans, 2, "only the shim's two sides run from its tip to the plate (else the spike)");
    // Both sides take the same axis constraint from the sketch writer (paper frame: Horizontal or Vertical within
    // sketch::HV_DEG), which says they are parallel (on Spec::good the part's x runs along the paper's y: Vertical).
    let axis = |side: &&((Vec2, Vec2), (Vec2, Vec2))| -> Option<bool> {
        let d = side.0.1 - side.0.0;
        let mut ang = d.y.atan2(d.x).to_degrees();
        if ang < 0.0 {
            ang += 180.0;
        }
        if ang.min(180.0 - ang) <= photo_core::sketch::HV_DEG {
            Some(true)
        } else if (ang - 90.0).abs() <= photo_core::sketch::HV_DEG {
            Some(false)
        } else {
            None
        }
    };
    let (a0, a1) = (axis(&sides[0]), axis(&sides[1]));
    assert!(a0.is_some() && a0 == a1, "the shim's sides take different axis constraints: {a0:?} {a1:?}");
    eprintln!("photo: thin fit sides={} sep={sep:.3} end={end:.3} tip={tip:.3} sigma={:.4}", sides.len(), m.sigma_mm);
}

// ---- VUGPHOTOSIDETHINPARALLEL (SR661): a thin side off the paper's axes is Parallel in the sketch (named tail block) ----
/// A paper-mm point of `m` in the part frame of `s` (through the measured H and the true one, as SR655's test reads).
fn par_part(s: &Spec, m: &Measured, p: Vec2) -> Vec2 {
    let truth_inv = photo_core::homography::invert(&fixture::homography(s)).expect("the true homography inverts");
    let q = apply(&m.h, p).expect("h maps the outline");
    fixture::paper_to_part(s, apply(&truth_inv, q).expect("the truth maps back"))
}

/// Every constraint of the measured sketch with its id (the photo sketch removes none).
fn par_cons(m: &Measured) -> Vec<(sketch_core::ConstraintId, Constraint)> {
    (0u32..).map(sketch_core::ConstraintId).map_while(|id| m.sketch.constraint(id).map(|c| (id, *c))).collect()
}

/// The sketch's Line entities that are the shim's sides (part frame: mid x < -1, at least half the shim long).
fn par_shim_lines(s: &Spec, m: &Measured) -> Vec<EntityId> {
    (0..m.sketch.entities().len() as u32)
        .map(EntityId)
        .filter(|&e| matches!(m.sketch.entity(e), Some(Entity::Line { .. })))
        .filter(|&e| {
            let a = par_part(s, m, m.sketch.point(PointRef::Start(e)).expect("a line has a start"));
            let b = par_part(s, m, m.sketch.point(PointRef::End(e)).expect("a line has an end"));
            (a + b).x * 0.5 < -1.0 && a.dist(b) >= fixture::SHIM_L * 0.5
        })
        .collect()
}

fn par_on_axis(cs: &[(sketch_core::ConstraintId, Constraint)], e: EntityId) -> bool {
    cs.iter().any(|(_, c)| matches!(*c, Constraint::Horizontal(x) | Constraint::Vertical(x) if x == e))
}

fn par_parallels(cs: &[(sketch_core::ConstraintId, Constraint)]) -> Vec<(sketch_core::ConstraintId, EntityId, EntityId)> {
    cs.iter()
        .filter_map(|&(id, c)| match c {
            Constraint::Parallel(a, b) => Some((id, a, b)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_thin_side_off_the_axes_is_parallel() {
    // ROTATED: SR655's shim fixture turned to 5°, past HV_DEG of the paper's axes, the shim still inside its lattice cell
    // (paper y 37.65 to 35.91 in the 35..40 cell). At 11.5° it crosses the y = 35 line for ~2.5 mm: SR654's crossing case.
    let s = Spec { part_rot_deg: 5.0, ..Spec::good() };
    let img = pixel_core::decode(&fixture::shim_png(&s)).expect("pixel_core decodes the turned shim PNG");
    let m = measure(&img, PhotoParams::new(5.0), Tol::DEFAULT).expect("the turned shim photo measures");
    let sides: Vec<usize> = m
        .shape
        .outer
        .iter()
        .enumerate()
        .filter_map(|(k, p)| match *p {
            Prim::Line { a, b, .. } => {
                let (pa, pb) = (par_part(&s, &m, a), par_part(&s, &m, b));
                ((pa + pb).x * 0.5 < -1.0 && pa.dist(pb) >= fixture::SHIM_L * 0.5).then_some(k)
            }
            _ => None,
        })
        .collect();
    assert_eq!(sides.len(), 2, "the turned shim's sides: {:?}", m.shape.outer);
    assert_eq!(m.shape.thin.len(), 1 + m.shape.holes.len(), "one thin list per loop");
    assert_eq!(m.shape.thin[0].len(), 1, "one thin pair on the outer loop: {:?}", m.shape.thin);
    let (i, j) = m.shape.thin[0][0];
    let mut pair = vec![i, j];
    pair.sort_unstable();
    assert_eq!(pair, sides, "the thin pair is the shim's two sides");
    assert!(m.shape.thin[1..].iter().all(|t| t.is_empty()), "a hole carries a thin pair: {:?}", m.shape.thin);
    let cs = par_cons(&m);
    let par = par_parallels(&cs);
    assert_eq!(par.len(), 1, "exactly one Parallel: {par:?}");
    let (pid, ea, eb) = par[0];
    let mut shim = par_shim_lines(&s, &m);
    shim.sort_unstable();
    let mut got = vec![ea, eb];
    got.sort_unstable();
    assert_eq!(got, shim, "the Parallel is on the shim's two side lines");
    let hv = shim.iter().filter(|&&e| par_on_axis(&cs, e)).count();
    assert_eq!(hv, 0, "a turned side took Horizontal / Vertical");
    assert!(!m.solved.redundant.contains(&pid), "the Parallel is a redundant row: {:?}", m.solved.redundant);
    eprintln!(
        "photo: thin parallel rot={:.1} thin={:?} parallel={} hv_sides={hv} dof={}",
        s.part_rot_deg,
        m.shape.thin,
        par.len(),
        m.solved.dof
    );

    // UN-ROTATED: SR655's fixture as is; both sides on an axis, so nothing new.
    let s0 = Spec::good();
    let img0 = pixel_core::decode(&fixture::shim_png(&s0)).expect("pixel_core decodes the shim PNG");
    let m0 = measure(&img0, PhotoParams::new(5.0), Tol::DEFAULT).expect("the shim photo measures");
    assert_eq!(m0.shape.thin[0].len(), 1, "the shim's thin pair at 1.5°: {:?}", m0.shape.thin);
    let cs0 = par_cons(&m0);
    let shim0 = par_shim_lines(&s0, &m0);
    assert_eq!(shim0.len(), 2, "the shim's side lines at 1.5°");
    let hv0 = shim0.iter().filter(|&&e| par_on_axis(&cs0, e)).count();
    assert_eq!(hv0, 2, "both sides take an axis constraint at 1.5°");
    let par0 = par_parallels(&cs0).len();
    assert_eq!(par0, 0, "a Parallel beside both sides' axis constraints");
    eprintln!(
        "photo: thin parallel rot={:.1} thin={:?} parallel={par0} hv_sides={hv0} dof={}",
        s0.part_rot_deg, m0.shape.thin, m0.solved.dof
    );

    // PLAIN: the plate without the shim has no thin pair and no Parallel.
    let mp = run(&Spec::good()).expect("the good photo measures");
    assert!(mp.shape.thin.iter().all(|t| t.is_empty()), "the plain plate carries a thin pair: {:?}", mp.shape.thin);
    let parp = par_parallels(&par_cons(&mp)).len();
    assert_eq!(parp, 0, "the plain plate carries a Parallel");
    eprintln!("photo: thin parallel plain thin={:?} parallel={parp} dof={}", mp.shape.thin, mp.solved.dof);
}
