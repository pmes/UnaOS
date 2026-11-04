// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! VUGSCANGDT (SR542): the SR510 fixture scan (Noise::D435I) measured against its design built through the ONE builder
//! `design_of`, then read as ISO 1101 tolerances: the box top's flatness, the post's cylindricity, the post's position
//! against its nominal and against a nominal shifted 1 mm, the verdicts, and each face's nominal surface.

use core::f64::consts::PI;
use std::sync::OnceLock;

use brep_core::primitives;
use scan_core::fixture::{self, Noise, Scene};
use scan_core::gdt::{cylindricity_minzone, flatness_minzone};
use scan_core::{
    AlignParams, Cylinder, DepthScale, Design, Deviation, FaceGdt, Frame, GDT_MIN_POINTS, GDT_NORMAL_DEG, Kind, Nominal, ScanMesh, Tol, Transform, Tsdf, Vec3, Verdict, design_of, extract, fit_cylinder, fit_plane, gdt, measure, nominal, position_mm, tsdf,
};

const DEG: f64 = PI / 180.0;
const CENTRE: Vec3 = Vec3::new(-20.0, 0.0, 50.0);
const BAND: f64 = 1.0;

/// tests/deviation.rs's fusion (SR510): 12 orbit frames at 600 mm, 40° up, D435i noise; with the volume's truncation,
/// the edge margin `gdt` takes (the seat's ruling: read from the fuse, no second constant).
fn fused() -> &'static (ScanMesh, f64) {
    static S: OnceLock<(ScanMesh, f64)> = OnceLock::new();
    S.get_or_init(|| fuse(&Noise::D435I, Guard::Default, DepthScale::D400_DEFAULT, 0.0))
}

/// VUGSCANTSDFEDGEFAR (SR562): the volume's depth-edge guard for one fuse — its default (off), off explicitly, SR555's
/// both-sides rule at the truncation, or SR562's far-side guard at the truncation.
#[derive(Clone, Copy, PartialEq)]
enum Guard {
    Default,
    Off,
    Both,
    Far,
}

/// VUGSCANTSDFOUTLIERS (SR555) / VUGSCANTSDFEDGEFAR (SR562): the ONE fuse builder — the 12 orbit frames at `noise`
/// quantised to `scale`, camera k at azimuth k·30° + k·`phase_rad` (0: `pose(k)`), the volume's guard `guard`; the
/// surface and the volume's truncation.
fn fuse(noise: &Noise, guard: Guard, scale: DepthScale, phase_rad: f64) -> (ScanMesh, f64) {
    let (scene, intr) = (Scene::bench(), fixture::bench_intrinsics());
    let mut t = Tsdf::new(4.0, 12.0, Tol::DEFAULT).expect("volume");
    let trunc = t.trunc_mm();
    t = match guard {
        Guard::Default => t,
        Guard::Off => t.with_edge_far_mm(f64::INFINITY),
        Guard::Both => t.with_edge_mm(trunc),
        Guard::Far => t.with_edge_far_mm(trunc),
    };
    for k in 0..12u32 {
        let p = if phase_rad == 0.0 { pose(k) } else { pose_phased(k, phase_rad) };
        let f = fixture::render_depth(&scene, &intr, scale, &p, noise, k as u64);
        t.integrate(&f, scale, &intr, &p).expect("frame integrates");
    }
    (extract(&t).expect("surface"), t.trunc_mm())
}

fn scan() -> &'static ScanMesh {
    &fused().0
}

/// The fixture's box (-230..70, ±100, 0..100) and r 40 h 150 post at (160, 0) through `design_of`, each solid's
/// vertices moved into place (the box's faces come first), measured from the identity.
fn measured() -> &'static (Design, Vec<String>, Deviation) {
    static M: OnceLock<(Design, Vec<String>, Deviation)> = OnceLock::new();
    M.get_or_init(|| {
        let (d, names) = design();
        let dev = measure_of(scan(), &d);
        (d, names, dev)
    })
}

/// The scan measured against the design from the identity (VUGSCANTSDFOUTLIERS: one road for every fuse).
fn measure_of(scan: &ScanMesh, d: &Design) -> Deviation {
    measure(scan, d, Transform::IDENTITY, &AlignParams::DEFAULT, BAND, Tol::DEFAULT).expect("aligns")
}

/// The design, built once per call through `design_of` and moved into place.
fn design() -> (Design, Vec<String>) {
    let tol = Tol::DEFAULT;
    let (b, c) = (primitives::box_(300.0, 200.0, 100.0, tol).expect("box"), primitives::cylinder(40.0, 150.0, tol).expect("cylinder"));
    let (mut d, names) = design_of([("box", &b), ("post", &c)], tol).expect("design");
    let nbox = b.faces.len() as u32;
    let mut at = vec![Vec3::ZERO; d.mesh.positions.len()];
    for (tri, &f) in d.mesh.triangles.iter().zip(&d.face_of) {
        let off = if f < nbox { Vec3::new(-230.0, -100.0, 0.0) } else { Vec3::new(160.0, 0.0, 0.0) };
        for &v in tri {
            at[v as usize] = off;
        }
    }
    for (p, o) in d.mesh.positions.iter_mut().zip(&at) {
        *p = *p + *o;
    }
    (d, names)
}

fn faces() -> Vec<FaceGdt> {
    let (d, names, dev) = measured();
    let g = gdt(scan(), d, dev, fused().1, Tol::DEFAULT);
    for f in &g {
        println!(
            "gdt: {} {} n={}/{} flatness={:?} cylindricity={:?} position={:?} band={:?} extent={:?}{}",
            names[f.face as usize],
            f.nominal.name(),
            f.n,
            f.matched,
            f.flatness_mm,
            f.cylindricity_mm,
            f.position_mm,
            f.band_mm,
            f.extent,
            f.minzone_tokens()
        );
    }
    g
}

fn top(g: &[FaceGdt]) -> FaceGdt {
    *g.iter()
        .find(|f| matches!(f.nominal, Nominal::Plane(p) if p.normal.z > 0.99 && (p.d - 100.0).abs() < 0.05))
        .expect("box top")
}

fn post(g: &[FaceGdt]) -> FaceGdt {
    *g.iter().find(|f| matches!(f.nominal, Nominal::Cylinder(_))).expect("post side")
}

#[test]
fn each_face_reads_its_nominal_surface() {
    let (d, names, _) = measured();
    let mut planes = 0;
    for f in 0..d.faces {
        let n = nominal(d, f, Tol::DEFAULT);
        let name = &names[f as usize];
        match n {
            Nominal::Plane(p) => {
                assert!(name.ends_with(":plane"), "{name} read as {n:?}");
                assert!((p.normal.mag() - 1.0).abs() < 1e-9);
                planes += 1;
            }
            Nominal::Cylinder(c) => {
                assert!(name.ends_with(":cylinder"), "{name} read as {n:?}");
                assert!((c.radius - 40.0).abs() < 0.05, "post radius {}", c.radius);
                assert!(c.axis.z.abs() > 1.0 - 1e-6, "post axis {:?}", c.axis);
                let r = c.point - c.axis * c.axis.dot(c.point);
                assert!((r - Vec3::new(160.0, 0.0, 0.0)).mag() < 0.05, "post axis point {:?}", c.point);
            }
            Nominal::Other => panic!("{name} read as Other"),
        }
    }
    assert_eq!(planes, 8, "six box faces and the post's two caps");
}

/// The seat's ruling (SR542): the fixture's TSDF surface carries interior vertices 0.6-3 mm off the design even with no
/// noise (a fusion finding, VUG-SCAN-TSDF-OUTLIERS), so the form values are printed, not bounded; what gdt.rs promises
/// is finite values on at least GDT_MIN_POINTS kept vertices of the matched ones.
#[test]
fn the_form_values_are_finite_on_the_kept_vertices() {
    let g = faces();
    let (t, p) = (top(&g), post(&g));
    let fl = t.flatness_mm.expect("top measured");
    let cy = p.cylindricity_mm.expect("post measured");
    println!("gdt: outliers top={fl:.3} post={cy:.3} (VUG-SCAN-TSDF-OUTLIERS owed)");
    // Form beside profile, no bound between them: the least-squares reference carries none (VUG-SCAN-GDT-MINZONE).
    println!("gdt: form/band top={fl:.3}/{:.3} post={cy:.3}/{:.3}", t.band_mm.unwrap_or(f64::NAN), p.band_mm.unwrap_or(f64::NAN));
    for f in [t, p] {
        assert!(f.n >= GDT_MIN_POINTS && f.n <= f.matched, "face {} kept {} of {}", f.face, f.n, f.matched);
        assert!(f.band_mm.is_some_and(f64::is_finite), "face {} band {:?}", f.face, f.band_mm);
    }
    assert!(fl.is_finite() && fl >= 0.0, "top flatness {fl}");
    assert!(cy.is_finite() && cy >= 0.0, "post cylindricity {cy}");
    assert!(p.position_mm.is_some_and(f64::is_finite), "post position {:?}", p.position_mm);
    assert_eq!((t.cylindricity_mm, t.position_mm, p.flatness_mm), (None, None, None));
}

/// The nominals the form is read against: the box top's plane (0, 0, 1) at d 100 and the post's axis and radius.
#[test]
fn the_top_and_post_nominals_are_right() {
    let g = faces();
    let Nominal::Plane(pl) = top(&g).nominal else { panic!("top nominal") };
    assert!((pl.normal - Vec3::Z).mag() < 0.05 && (pl.d - 100.0).abs() < 0.05, "top {pl:?}");
    let Nominal::Cylinder(c) = post(&g).nominal else { panic!("post nominal") };
    let foot = c.point - c.axis * c.axis.dot(c.point);
    assert!((c.radius - 40.0).abs() < 0.05, "post r {}", c.radius);
    assert!((foot - Vec3::new(160.0, 0.0, 0.0)).mag() < 0.05, "post axis {:?} through {:?}", c.axis, c.point);
}

/// A nominal shifted 1 mm across the axis grows the position zone by its diameter.
#[test]
fn a_one_mm_shift_grows_the_post_position_by_two() {
    let p = post(&faces());
    let Nominal::Cylinder(nc) = p.nominal else { panic!("post nominal {:?}", p.nominal) };
    let axis = p.axis.expect("post axis fitted");
    let at = p.position_mm.expect("post position");
    let shifted = scan_core::Cylinder { point: nc.point + Vec3::X, ..nc };
    let off = position_mm(&axis, &shifted, p.extent);
    println!("gdt: post position own={at:.4} shifted_1mm={off:.4} extent={:?}", p.extent);
    assert!((off - (at + 2.0)).abs() <= 0.4 || off >= at + 1.6, "own {at} shifted {off}");
}

#[test]
fn verdicts_pass_fail_and_refuse_a_kind_the_surface_does_not_take() {
    let g = faces();
    let (t, p) = (top(&g), post(&g));
    let fl = t.flatness_mm.expect("top measured");
    assert!(matches!(t.verdict(Kind::Flatness, fl + 0.1), Verdict::Pass { .. }));
    assert!(matches!(t.verdict(Kind::Flatness, fl - 0.1), Verdict::Fail { .. }) || fl < 0.1);
    assert_eq!(t.verdict(Kind::Flatness, fl), Verdict::Pass { value_mm: fl, tol_mm: fl });
    assert!(!p.accepts(Kind::Flatness) && p.accepts(Kind::Cylindricity) && p.accepts(Kind::Position));
    assert_eq!(p.verdict(Kind::Flatness, 1.0), Verdict::Unmeasured);
    assert_eq!(t.verdict(Kind::Position, 1.0), Verdict::Unmeasured);
    assert_eq!(Kind::parse("cylindricity"), Some(Kind::Cylindricity));
    assert_eq!(Kind::parse("roundness"), None);
    for k in [Kind::Flatness, Kind::Cylindricity, Kind::Position] {
        assert_eq!(Kind::parse(k.name()), Some(k));
    }
}

// ---- VUGSCANGDTMINZONE (SR554): the ISO 1101 minimum zone under the least-squares band ----

/// A deterministic value in [0, 1): the golden-ratio Weyl sequence (scan_core's sampler is crate-private).
fn weyl(i: u32) -> f64 {
    (i as f64 * 0.618_033_988_749_894_9).fract()
}

fn band(vals: impl Iterator<Item = f64>) -> f64 {
    let (lo, hi) = vals.fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), v| (lo.min(v), hi.max(v)));
    hi - lo
}

#[test]
fn minzone_flatness_reads_under_least_squares() {
    // A plane tilted 0.5 / 0.3 mm per metre over 100 x 100 mm, 21 x 21 points, heights within +-0.05 mm of it.
    let grid = |noise: f64| -> Vec<Vec3> {
        (0..21u32 * 21)
            .map(|k| {
                let (x, y) = ((k / 21) as f64 * 5.0, (k % 21) as f64 * 5.0);
                Vec3::new(x, y, 0.0005 * x + 0.0003 * y + noise * (weyl(k) - 0.5))
            })
            .collect()
    };
    let fit = Tol { lin: 1.0, ang: 30.0 * DEG };
    let pts = grid(0.1);
    let ls = fit_plane(&pts, fit).expect("plane fits").shape;
    let ls_mm = band(pts.iter().map(|&p| ls.signed_distance(p)));
    let mz = flatness_minzone(&pts, &ls, Tol::DEFAULT);
    println!("tilted plane: least squares {ls_mm:.6} min-zone {mz}");
    assert!(!mz.ls, "the search converged");
    assert!(mz.mm <= ls_mm + 1e-12, "min-zone {} over least squares {ls_mm}", mz.mm);
    assert!(mz.mm <= 0.1 + 1e-6, "min-zone {} wider than the generator's band", mz.mm);
    // Coplanar points: both bands are zero.
    let flat = grid(0.0);
    let ls = fit_plane(&flat, fit).expect("plane fits").shape;
    let ls_mm = band(flat.iter().map(|&p| ls.signed_distance(p)));
    let mz = flatness_minzone(&flat, &ls, Tol::DEFAULT);
    assert!(!mz.ls && (mz.mm - ls_mm).abs() <= 1e-9, "coplanar: min-zone {mz:?} least squares {ls_mm}");
    // The SR542 fixture's box top.
    let t = top(&faces());
    let m = t.minzone_flatness_mm.expect("top min-zone");
    let fl = t.flatness_mm.expect("top measured");
    println!("box top: least squares {fl:.6} min-zone {m}");
    assert!(!m.ls && m.mm <= fl + 1e-12, "box top min-zone {m:?} over least squares {fl}");
    assert!(t.minzone_tokens().starts_with(" minzone_flatness=") && t.minzone_cylindricity_mm.is_none());
}

#[test]
fn minzone_cylindricity_reads_under_least_squares_on_a_tilted_cylinder() {
    // r 40 h 100, the axis tilted 0.1 mm over its length, a 3-lobe form error of 0.03 mm and +-0.01 mm more.
    let a = Vec3::new(0.001, 0.0, 1.0).normalize().expect("axis");
    let u = a.any_perpendicular().expect("u");
    let v = a.cross(u);
    let base = Vec3::new(5.0, -3.0, 0.0);
    let (mut pts, mut nrm) = (Vec::new(), Vec::new());
    for k in 0..36u32 * 11 {
        let (th, z) = ((k % 36) as f64 * 10.0 * DEG, (k / 36) as f64 * 10.0);
        let n = u * th.cos() + v * th.sin();
        let r = 40.0 + 0.03 * (3.0 * th).cos() + 0.02 * (weyl(k) - 0.5);
        pts.push(base + a * z + n * r);
        nrm.push(n);
    }
    let ls = fit_cylinder(&pts, &nrm, Tol { lin: 1.0, ang: 10.0 * DEG }).expect("cylinder fits").shape;
    let radial = |c: &Cylinder, p: Vec3| {
        let d = p - c.point;
        (d - c.axis * c.axis.dot(d)).mag() - c.radius
    };
    let ls_mm = band(pts.iter().map(|&p| radial(&ls, p)));
    let mz = cylindricity_minzone(&pts, &ls, Tol::DEFAULT);
    println!("tilted cylinder: least squares {ls_mm:.6} min-zone {mz}");
    assert!(!mz.ls, "the search converged");
    assert!(mz.mm <= ls_mm + 1e-12, "min-zone {} over least squares {ls_mm}", mz.mm);
    assert!(mz.mm <= 0.08 + 1e-3, "min-zone {} wider than the generator's band", mz.mm);
    // The SR542 fixture's post.
    let p = post(&faces());
    let m = p.minzone_cylindricity_mm.expect("post min-zone");
    let cy = p.cylindricity_mm.expect("post measured");
    println!("post: least squares {cy:.6} min-zone {m}");
    assert!(!m.ls && m.mm <= cy + 1e-12, "post min-zone {m:?} over least squares {cy}");
    assert!(p.minzone_tokens().starts_with(" minzone_cylindricity=") && p.minzone_flatness_mm.is_none());
}

// ---- VUGSCANGDTDATUMS (LEDGER SR552): the datum frame A|B|C from the box faces, the post's position read in it ----

use scan_core::gdt::{DatumError, frame_of, position_in};

/// The first design face whose nominal is a plane with this outward normal.
fn plane_face(d: &Design, normal: Vec3) -> u32 {
    (0..d.faces)
        .find(|&f| matches!(nominal(d, f, Tol::DEFAULT), Nominal::Plane(p) if p.normal.dot(normal) > 0.99))
        .unwrap_or_else(|| panic!("no plane face along {normal:?}"))
}

/// A = the box top (+z), B = the box's -x side, C = its -y side: the frame from the scan's datum faces, the post's
/// position against its nominal read in it, and against the nominal shifted 1 mm along the frame's x. The seat's
/// ruling (SR552): the frame carries its datum fits' error over the lever from B to the post (0.15° on B's normal
/// over 390 mm is 1 mm; B's flatness is 1.1 mm from the TSDF outliers, SR555), so the test asserts what the frame
/// can promise on this fixture, not a tight position (VUG-SCAN-GDT-DATUM-CONDITION owed: re-tighten after SR555).
#[test]
fn the_post_position_reads_in_the_box_datum_frame() {
    let (d, names, dev) = measured();
    let datums = [plane_face(d, Vec3::Z), plane_face(d, -Vec3::X), plane_face(d, -Vec3::Y)];
    let fr = frame_of(scan(), d, dev, datums, fused().1, Tol::DEFAULT).expect("the box datums fix a frame");
    let p = post(&faces());
    let Nominal::Cylinder(nc) = p.nominal else { panic!("post nominal {:?}", p.nominal) };
    let axis = p.axis.expect("post axis fitted");
    let own = position_in(&fr, &axis, &nc, p.extent);
    let x = fr.nominal.apply_vector(Vec3::X);
    let shifted = position_in(&fr, &axis, &scan_core::Cylinder { point: nc.point + x, ..nc }, p.extent);
    let Nominal::Plane(b) = nominal(d, datums[1], Tol::DEFAULT) else { panic!("datum B is not a plane") };
    let lever = b.signed_distance(nc.point).abs();
    println!(
        "gdt: frame A={} B={} C={} own={own:.4} shifted={shifted:.4} residual={:.3} best_fit={:?}",
        names[datums[0] as usize],
        names[datums[1] as usize],
        names[datums[2] as usize],
        fr.residual_mm,
        p.position_mm
    );
    println!("gdt: datum lever A..B={lever:.1} (VUG-SCAN-GDT-DATUM-CONDITION owed)");
    assert!(fr.residual_mm.is_finite() && fr.residual_mm < 1.0, "frame residual {}", fr.residual_mm);
    for (what, v) in [("own", own), ("shifted", shifted)] {
        assert!(v.is_finite() && v < 5.0, "post position in the datum frame, {what} nominal: {v}");
    }
    // A 1 mm shift moves the zone by at most its diameter (2 mm) plus the frame's own error.
    assert!((shifted - own).abs() <= 2.4, "own {own} shifted {shifted}");
}

/// A datum face past the design, a surface that cannot stand as the primary, and an unseen face each refuse by label.
#[test]
fn a_missing_or_unseen_datum_face_refuses_by_name() {
    let (d, _, dev) = measured();
    let (top, mx, my, bottom) = (plane_face(d, Vec3::Z), plane_face(d, -Vec3::X), plane_face(d, -Vec3::Y), plane_face(d, -Vec3::Z));
    let side = post(&faces()).face;
    let at = |datums: [u32; 3]| frame_of(scan(), d, dev, datums, fused().1, Tol::DEFAULT);
    assert_eq!(at([top, d.faces + 3, my]), Err(DatumError::NoFace { label: 'B', face: d.faces + 3 }));
    assert_eq!(at([side, mx, my]), Err(DatumError::DatumSurface { label: 'A', face: side, surface: "cylinder" }));
    let unseen = at([bottom, mx, my]);
    println!("gdt: the box bottom as A -> {unseen:?}");
    assert!(matches!(unseen, Err(DatumError::DatumFit { label: 'A', face, .. }) if face == bottom), "{unseen:?}");
    assert_eq!(at([top, mx, my]).err().map(|e| e.label()), None);
}

// VUGSCANTSDFOUTLIERS (SR555): the probe of the fused surface's outliers, kept. Per fuse (Noise::NONE and D435i, the
// depth-edge guard off and on) the top's flatness and the post's cylindricity and position; for the noise-free fuses,
// the 20 worst vertices of the top and of the post by |deviation − the face's median| (the seat's Q1 answer B: every
// vertex `deviations` paired with the face, and again over the rows the KNOWN SCENE calls form vertices — at least one
// truncation from the face's boundary, the TSDF normal within GDT_NORMAL_DEG of the nominal), each with what the
// fixture's 12 cameras saw at it: `saw` (a valid depth, the vertex at most one truncation behind it), `edge` (of those,
// the pixel a depth edge: `tsdf::depth_edge` at the truncation), `shadow` (the pixel's depth in front of the vertex by
// more than the truncation: occluded) and the grazing angle (the ray against the nominal normal) over the cameras that
// saw it.

/// The fixture's camera `k` (tests/deviation.rs's orbit).
fn pose(k: u32) -> Transform {
    fixture::orbit(CENTRE, 600.0, 40.0 * DEG, k as f64 * 2.0 * PI / 12.0)
}

/// VUGSCANTSDFEDGEFAR (SR562), VUG-SCAN-FIXTURE-QUANT: camera `k` with a `phase_rad` offset per frame, so the 12
/// cameras no longer sit at the orbit's 12-fold symmetric azimuths (the quantisation stops correlating across them).
fn pose_phased(k: u32, phase_rad: f64) -> Transform {
    fixture::orbit(CENTRE, 600.0, 40.0 * DEG, k as f64 * (2.0 * PI / 12.0 + phase_rad))
}

/// What the fixture's cameras saw at one point.
struct Census {
    saw: u32,
    edge: u32,
    shadow: u32,
    graze: (f64, f64),
    /// VUGSCANTSDFEDGELATERAL (SR574) M1: (s < 0, all, zero gradient) over the cameras at which the vertex sits on an
    /// edge pixel holding a depth more than a truncation behind it (the known scene: the misassigned floor), and the
    /// same for the free-space control one truncation out along the nominal normal.
    mis: (u32, u32, u32),
    free: (u32, u32, u32),
}

fn census(p: Vec3, n: Vec3, frames: &[(Transform, Frame)], trunc: f64) -> Census {
    let (intr, scale) = (fixture::bench_intrinsics(), DepthScale::D400_DEFAULT);
    let mut c = Census { saw: 0, edge: 0, shadow: 0, graze: (f64::INFINITY, f64::NEG_INFINITY), mis: (0, 0, 0), free: (0, 0, 0) };
    for (cam, f) in frames {
        let Ok(inv) = cam.invert() else { continue };
        lateral_tally(&mut c.mis, &inv, f, p, trunc); // VUGSCANTSDFEDGELATERAL (SR574) M1
        lateral_tally(&mut c.free, &inv, f, p + n * trunc, trunc);
        let q = inv.apply_point(p);
        if q.z <= 0.0 {
            continue;
        }
        let Some(uv) = intr.project([(q.x / 1000.0) as f32, (q.y / 1000.0) as f32, (q.z / 1000.0) as f32]) else { continue };
        if !(uv[0] >= 0.0 && uv[1] >= 0.0) {
            continue;
        }
        let (px, py) = (uv[0] as u32, uv[1] as u32);
        let Some(m) = f.u16_at(px, py).and_then(|raw| scale.metres(raw)) else { continue };
        let dz = m as f64 * 1000.0 - q.z;
        if dz < -trunc {
            c.shadow += 1;
            continue;
        }
        c.saw += 1;
        if tsdf::depth_edge(f, scale, px, py, trunc) {
            c.edge += 1;
        }
        if let Ok(ray) = (p - cam.trans).normalize() {
            let a = ray.dot(n).abs().min(1.0).acos() / DEG;
            c.graze = (c.graze.0.min(a), c.graze.1.max(a));
        }
    }
    c
}

/// VUGSCANTSDFEDGELATERAL (SR574) M1: one camera's lateral sample of point `p` into `t` = (s < 0, all, zero
/// gradient): counted only when `p`'s floor pixel is a depth edge at the truncation and its depth lies more than a
/// truncation behind `p` (a near point on a far pixel). `s` is `p`'s sub-pixel offset from that pixel's centre along
/// the 3x3's Sobel depth gradient (near -> far); s < 0 is the near neighbour's side.
fn lateral_tally(t: &mut (u32, u32, u32), inv: &Transform, f: &Frame, p: Vec3, trunc: f64) {
    let (intr, scale) = (fixture::bench_intrinsics(), DepthScale::D400_DEFAULT);
    let q = inv.apply_point(p);
    if q.z <= 0.0 {
        return;
    }
    let Some(uv) = intr.project([(q.x / 1000.0) as f32, (q.y / 1000.0) as f32, (q.z / 1000.0) as f32]) else { return };
    if !(uv[0] >= 0.0 && uv[1] >= 0.0) {
        return;
    }
    let (px, py) = (uv[0] as u32, uv[1] as u32);
    let at = |x: i64, y: i64| -> Option<f64> {
        if x < 0 || y < 0 {
            return None;
        }
        f.u16_at(x as u32, y as u32).and_then(|raw| scale.metres(raw)).map(|m| m as f64 * 1000.0)
    };
    let Some(m) = at(px as i64, py as i64) else { return };
    if m - q.z <= trunc || !tsdf::depth_edge(f, scale, px, py, trunc) {
        return;
    }
    // Sobel; a hole or an off-frame neighbour reads the centre's depth (neutral, as the 3x3 span treats it).
    let d = |dx: i64, dy: i64| at(px as i64 + dx, py as i64 + dy).unwrap_or(m);
    let gx = (d(1, -1) + 2.0 * d(1, 0) + d(1, 1)) - (d(-1, -1) + 2.0 * d(-1, 0) + d(-1, 1));
    let gy = (d(-1, 1) + 2.0 * d(0, 1) + d(1, 1)) - (d(-1, -1) + 2.0 * d(0, -1) + d(1, -1));
    t.1 += 1;
    if gx == 0.0 && gy == 0.0 {
        t.2 += 1;
        return;
    }
    let s = (uv[0] as f64 - px as f64 - 0.5) * gx + (uv[1] as f64 - py as f64 - 0.5) * gy;
    t.0 += (s < 0.0) as u32;
}

/// The known scene's oracle for a vertex of the top or the post: (the nominal normal there, the distance to the face's
/// boundary). The top: +z, the distance to the box's top rectangle edges; the post: radial, the axial distance to a rim.
fn oracle(f: &FaceGdt, p: Vec3) -> (Vec3, f64) {
    match f.nominal {
        Nominal::Plane(_) => (Vec3::Z, (p.x + 230.0).min(70.0 - p.x).min(p.y + 100.0).min(100.0 - p.y)),
        Nominal::Cylinder(c) => {
            let d = p - c.point;
            let r = d - c.axis * c.axis.dot(d);
            (r.normalize().unwrap_or(Vec3::ZERO), p.z.min(150.0 - p.z))
        }
        Nominal::Other => (Vec3::ZERO, 0.0),
    }
}

/// The 20 worst rows of face `f` (all paired vertices, then the known-scene form vertices) and their summaries.
fn probe_table(label: &str, f: &FaceGdt, s: &ScanMesh, dev: &Deviation, frames: &[(Transform, Frame)], trunc: f64) -> usize {
    let mut rows: Vec<(usize, f64)> = dev
        .face_of_vertex
        .iter()
        .zip(&dev.per_vertex)
        .enumerate()
        .filter_map(|(i, (fv, d))| if *fv == Some(f.face) { d.map(|d| (i, d)) } else { None })
        .collect();
    let mut ds: Vec<f64> = rows.iter().map(|r| r.1).collect();
    ds.sort_by(|a, b| a.total_cmp(b));
    let med = ds.get(ds.len() / 2).copied().unwrap_or(0.0);
    rows.sort_by(|a, b| (b.1 - med).abs().total_cmp(&(a.1 - med).abs()));
    let cos = GDT_NORMAL_DEG.to_radians().cos();
    let mut printed = 0;
    for (set, form_only) in [("all", false), ("form", true)] {
        let (mut n, mut edge, mut shadow, mut graze) = (0, 0, 0, 0);
        let (mut lat_mis, mut lat_free) = ((0u32, 0u32, 0u32), (0u32, 0u32, 0u32)); // VUGSCANTSDFEDGELATERAL (SR574) M1
        for &(i, d) in &rows {
            if n == 20 {
                break;
            }
            let p = s.mesh.positions[i];
            let (nn, bdist) = oracle(f, p);
            let sn = s.normals.get(i).copied().unwrap_or(Vec3::ZERO);
            let nang = sn.normalize().map(|u| u.dot(nn).abs().min(1.0).acos() / DEG).unwrap_or(90.0);
            let form = bdist >= trunc && sn.dot(nn).abs() >= cos * sn.mag() && sn.mag_sq() > 0.0;
            if form_only && !form {
                continue;
            }
            n += 1;
            let c = census(p, nn, frames, trunc);
            edge += (c.edge > 0) as u32;
            shadow += (c.shadow > 0) as u32;
            graze += (c.graze.1 > 75.0) as u32;
            for (acc, t) in [(&mut lat_mis, c.mis), (&mut lat_free, c.free)] {
                (acc.0, acc.1, acc.2) = (acc.0 + t.0, acc.1 + t.1, acc.2 + t.2); // VUGSCANTSDFEDGELATERAL (SR574) M1
            }
            let g = ((p.x / 4.0).floor() as i64, (p.y / 4.0).floor() as i64, (p.z / 4.0).floor() as i64);
            println!(
                "tsdf-probe: {label} {set} {n:2} voxel=({},{},{}) at=({:.1},{:.1},{:.1}) dev={d:+.3} off_median={:+.3} boundary={bdist:.1} normal={nang:.1}deg saw={}/12 edge={} shadow={} graze={:.1}-{:.1}deg lat_mis={}/{} lat_free={}/{}",
                g.0, g.1, g.2, p.x, p.y, p.z, d - med, c.saw, c.edge, c.shadow, c.graze.0, c.graze.1, c.mis.0, c.mis.1, c.free.0, c.free.1
            );
        }
        println!("tsdf-probe: {label} {set} worst{n} edge>0={edge} shadow>0={shadow} graze>75={graze} median={med:+.3}");
        println!(
            "tsdf-probe: {label} {set} lateral mis_near={}/{} free_near={}/{} zero_grad mis={} free={}",
            lat_mis.0, lat_mis.1, lat_free.0, lat_free.1, lat_mis.2, lat_free.2
        );
        printed += n;
    }
    printed
}

#[test]
fn tsdf_outliers_probe() {
    let (d, _) = design();
    let (scene, intr, scale) = (Scene::bench(), fixture::bench_intrinsics(), DepthScale::D400_DEFAULT);
    let frames: Vec<(Transform, Frame)> =
        (0..12u32).map(|k| (pose(k), fixture::render_depth(&scene, &intr, scale, &pose(k), &Noise::NONE, k as u64))).collect();
    for (noise, nname) in [(Noise::NONE, "none"), (Noise::D435I, "d435i")] {
        // The guards at the volume's own truncation (read from the volume, no second constant): off, SR555's
        // both-sides rule, SR562's far-side rule.
        for (guard, gname) in [(Guard::Off, "off"), (Guard::Both, "both"), (Guard::Far, "far")] {
            let (s, trunc) = fuse(&noise, guard, scale, 0.0);
            let dev = measure_of(&s, &d);
            let g = gdt(&s, &d, &dev, trunc, Tol::DEFAULT);
            let (t, p) = (top(&g), post(&g));
            println!(
                "tsdf-probe: noise={nname} guard={gname} verts={} top flatness={:?} n={}/{} post cylindricity={:?} position={:?} n={}/{} align_rms={:.3}",
                s.mesh.positions.len(),
                t.flatness_mm,
                t.n,
                t.matched,
                p.cylindricity_mm,
                p.position_mm,
                p.n,
                p.matched,
                dev.stats.rms_mm
            );
            if guard != Guard::Off {
                assert!(t.flatness_mm.is_some_and(f64::is_finite) && p.cylindricity_mm.is_some_and(f64::is_finite), "guard-{gname} values finite");
            }
            if nname == "none" {
                let label = |f: &str| format!("guard={gname} {f}");
                assert_eq!(probe_table(&label("top"), &t, &s, &dev, &frames, trunc), 40, "20 rows per set on the top");
                assert_eq!(probe_table(&label("post"), &p, &s, &dev, &frames, trunc), 40, "20 rows per set on the post");
            }
        }
    }
    // VUGSCANTSDFEDGEFAR (SR562), VUG-SCAN-FIXTURE-QUANT: the top's reading split into fixture and fusion — the same
    // noise-free fuse (the default: no guard) at the 1 mm scale and at the D400's 100 um depth units, on the symmetric
    // orbit and with a 7° phase per frame.
    for (um, phase_deg) in [(1000u32, 0.0f64), (100, 0.0), (1000, 7.0), (100, 7.0)] {
        let (s, trunc) = fuse(&Noise::NONE, Guard::Default, DepthScale::from_micrometres(um), phase_deg * DEG);
        let dev = measure_of(&s, &d);
        let g = gdt(&s, &d, &dev, trunc, Tol::DEFAULT);
        let (t, p) = (top(&g), post(&g));
        println!(
            "tsdf-probe: fixture scale_um={um} phase_deg={phase_deg} verts={} top flatness={:?} n={}/{} post cylindricity={:?} n={}/{} align_rms={:.3}",
            s.mesh.positions.len(),
            t.flatness_mm,
            t.n,
            t.matched,
            p.cylindricity_mm,
            p.n,
            p.matched,
            dev.stats.rms_mm
        );
        assert!(t.flatness_mm.is_some_and(f64::is_finite), "fixture leg {um} um {phase_deg} deg: top flatness");
    }
}

// ---- VUGSCANGDTSPEC (LEDGER SR559): the callout parsed once; profile of a surface ----

/// `Callout::parse` reads `<face>=<kind>:<tol>` at the LAST `=` and the LAST `:` (a face name carries `:` and may carry
/// `=`), its `Display` reads back to the same callout, and every malformed callout is refused by name.
#[test]
fn a_callout_parses_once_round_trips_and_refuses_by_name() {
    use scan_core::gdt::{Callout, GdtSpecError};
    let c = Callout::parse("box/f3:plane=flatness:0.05").expect("parses");
    assert_eq!(c, Callout { face: "box/f3:plane".into(), kind: Kind::Flatness, tol_mm: 0.05, off_mm: None }); // VUGSCANGDTPROFILEDATUM (SR567): the struct grew
    let c = Callout::parse("a=b:c=profile:1.5").expect("parses");
    assert_eq!((c.face.as_str(), c.kind, c.tol_mm), ("a=b:c", Kind::Profile, 1.5));
    for s in ["box/f3:plane=flatness:0.05", "post/f0:cylinder=cylindricity:0.1", "p=position:2", "a=b:c=profile:1.5", "x=profile:0.000125"] {
        let c = Callout::parse(s).expect(s);
        assert_eq!(Callout::parse(&c.to_string()), Ok(c.clone()), "{s} -> {c}");
    }
    assert_eq!(Callout::parse("nosuch"), Err(GdtSpecError::GdtFace));
    assert_eq!(Callout::parse("=flatness:1"), Err(GdtSpecError::GdtFace));
    assert_eq!(Callout::parse("f=flatness"), Err(GdtSpecError::GdtKind { kind: "flatness".into() }));
    assert_eq!(Callout::parse("f=roundness:1"), Err(GdtSpecError::GdtKind { kind: "roundness".into() }));
    for tol in ["0", "-1", "inf", "NaN", "x", ""] {
        assert_eq!(Callout::parse(&format!("f=flatness:{tol}")), Err(GdtSpecError::BadTol { tol: tol.into() }), "{tol}");
    }
    for k in [Kind::Flatness, Kind::Cylindricity, Kind::Position, Kind::Profile] {
        assert_eq!(Kind::parse(k.name()), Some(k));
    }
}

/// Profile of a surface on the SR510 fixture: the zone centred on the nominal (2 · max |d|) is never below the band
/// (max − min), plane and cylinder both take it, and its verdict passes on and above the value and fails below.
#[test]
fn a_profile_callout_reads_the_nominal_zone_pass_and_fail() {
    let g = faces();
    let (t, p) = (top(&g), post(&g));
    let (tp, pp) = (t.profile_mm.expect("top profile"), p.profile_mm.expect("post profile"));
    println!("gdt: profile top={tp:.3} band={:.3} post={pp:.3} band={:.3}", t.band_mm.unwrap_or(f64::NAN), p.band_mm.unwrap_or(f64::NAN));
    for f in [t, p] {
        let (pr, band) = (f.profile_mm.unwrap(), f.band_mm.expect("band"));
        assert!(pr.is_finite() && pr >= band, "face {} profile {pr} below band {band}", f.face);
        assert!(f.accepts(Kind::Profile));
        assert_eq!(f.value(Kind::Profile), Some(pr));
        assert!(matches!(f.verdict(Kind::Profile, pr + 0.1), Verdict::Pass { .. }));
        assert_eq!(f.verdict(Kind::Profile, pr), Verdict::Pass { value_mm: pr, tol_mm: pr });
        assert_eq!(f.verdict(Kind::Profile, pr * 0.5), Verdict::Fail { value_mm: pr, tol_mm: pr * 0.5 });
    }
}

// ---- VUGSCANGDTPROFILEDATUM (LEDGER SR567): profile located in the datum frame; the unequal zone ----

use scan_core::DESIGN_CHORD_MM;
use scan_core::gdt::{Callout, DatumFrame, GdtSpecError, in_frame, profile_in, zone_mm};

/// A frame whose measured datums ARE the nominal ones (the true-datum case): the relative transform is the identity.
const TRUE_FRAME: DatumFrame = DatumFrame { measured: Transform::IDENTITY, nominal: Transform::IDENTITY, residual_mm: 0.0 };

/// Under a true-datum frame the profile in the frame reads the best-fit profile within the facet sagitta (deviation.rs
/// measures to the faceted design, the nominal cylinder passes through its vertices: 2 · DESIGN_CHORD_MM); under the box
/// datums' MEASURED frame it stays within SR552's promise on this fixture (the seat's ruling D1). Datum face B's plane
/// nominal shifted 1 mm along x toward the side its span leans reads the profile 2 mm wider while the best fit stays
/// (ruling D2: on a plane the shift moves every d' by exactly 1); the post's shifted reading is printed.
#[test]
fn the_profile_reads_in_the_box_datum_frame() {
    let (d, names, dev) = measured();
    let margin = fused().1;
    let best = faces();
    let mut g = best.clone();
    in_frame(scan(), d, dev, margin, &mut g, &TRUE_FRAME);
    let (t, p) = (top(&g), post(&g));
    for f in [t, p] {
        let (pf, pr) = (f.profile_in_frame_mm.expect("profile in the frame"), f.profile_mm.expect("profile"));
        println!("gdt: profile_frame {}={pf:.3} profile={pr:.3} true", names[f.face as usize]);
        assert!((pf - pr).abs() <= 2.0 * DESIGN_CHORD_MM + 1e-9, "face {} profile_frame {pf} profile {pr}", f.face);
        assert_eq!(f.value(Kind::Profile), Some(pf), "the verdict reads in the frame");
    }
    let datums = [plane_face(d, Vec3::Z), plane_face(d, -Vec3::X), plane_face(d, -Vec3::Y)];
    let fr = frame_of(scan(), d, dev, datums, margin, Tol::DEFAULT).expect("the box datums fix a frame");
    let mut m = best.clone();
    in_frame(scan(), d, dev, margin, &mut m, &fr);
    let pm = post(&m);
    let (pf, pr) = (pm.profile_in_frame_mm.expect("post profile in the measured frame"), pm.profile_mm.expect("post profile"));
    println!("gdt: profile_frame post={pf:.3} profile={pr:.3} measured residual={:.3}", fr.residual_mm);
    assert!(pf.is_finite() && pf <= pr + 5.0, "post profile_frame {pf} against profile {pr} in the measured frame");
    // Datum face B (the box's -x side), its plane nominal shifted 1 mm along x.
    let b = g.iter().find(|f| f.face == datums[1]).copied().expect("B read");
    let Nominal::Plane(bp) = b.nominal else { panic!("B is not a plane") };
    let (lo, hi) = b.span_mm.expect("B span");
    let grow = if hi >= -lo { 1.0 } else { -1.0 };
    let own = profile_in(&TRUE_FRAME, scan(), d, dev, b.face, b.nominal, margin).expect("B own");
    let moved = profile_in(&TRUE_FRAME, scan(), d, dev, b.face, Nominal::Plane(scan_core::Plane { d: bp.d - grow, ..bp }), margin).expect("B shifted");
    let delta = zone_mm(moved, 0.0) - zone_mm(own, 0.0);
    let Nominal::Cylinder(nc) = p.nominal else { panic!("post nominal") };
    let post_moved = profile_in(&TRUE_FRAME, scan(), d, dev, p.face, Nominal::Cylinder(Cylinder { point: nc.point + Vec3::X, ..nc }), margin).map(|s| zone_mm(s, 0.0));
    println!("gdt: profile shift B={delta:.3} post={:?} (own {:.3})", post_moved, p.profile_in_frame_mm.unwrap_or(f64::NAN));
    assert!((delta - 2.0).abs() <= 0.3, "B shifted 1 mm: profile_frame wider by {delta}");
    assert!(post_moved.is_some_and(f64::is_finite), "post shifted reads");
    assert_eq!(b.profile_mm, best.iter().find(|f| f.face == datums[1]).and_then(|f| f.profile_mm), "best fit unchanged by in_frame");
}

/// The unequal zone: `@<off>` parses on profile only, reads back, and is refused by name otherwise; a frame that puts
/// the box top 2 mm proud passes its UZ callout at `@2` with the tolerance its zone about 2 needs, where the same
/// tolerance on the equal zone fails; without an offset `Callout::verdict` is `FaceGdt::verdict`.
#[test]
fn a_profile_uz_callout_passes_a_proud_face_at_its_offset() {
    let c = Callout::parse("box/f3:plane=profile:1.5@-0.25").expect("parses");
    assert_eq!((c.kind, c.tol_mm, c.off_mm), (Kind::Profile, 1.5, Some(-0.25)));
    assert_eq!(Callout::parse(&c.to_string()), Ok(c.clone()), "{c}");
    assert_eq!(Callout::parse("f=flatness:1@0.5"), Err(GdtSpecError::BadUz { uz: "0.5".into() }));
    assert_eq!(Callout::parse("f=profile:1@inf"), Err(GdtSpecError::BadUz { uz: "inf".into() }));
    assert_eq!(Callout::parse("f=profile:1@x"), Err(GdtSpecError::BadUz { uz: "x".into() }));
    assert_eq!(Callout::parse("f=profile:0@1"), Err(GdtSpecError::BadTol { tol: "0".into() }));
    let (d, names, dev) = measured();
    let mut g = faces();
    let proud = DatumFrame { nominal: Transform::from_translation(Vec3::new(0.0, 0.0, 2.0)), ..TRUE_FRAME };
    in_frame(scan(), d, dev, fused().1, &mut g, &proud);
    let t = top(&g);
    let s = t.frame_span_mm.expect("top span in the frame");
    let tol = zone_mm(s, 2.0) + 0.01;
    let name = &names[t.face as usize];
    let uz = Callout { face: name.clone(), kind: Kind::Profile, tol_mm: tol, off_mm: Some(2.0) };
    let eq = Callout { off_mm: None, ..uz.clone() };
    println!("gdt: uz top span=({:.3}, {:.3}) zone@2={:.3} equal={:.3} tol={tol:.3}", s.0, s.1, zone_mm(s, 2.0), zone_mm(s, 0.0));
    assert!(s.0 > 0.0, "the top stands proud in the frame: {s:?}");
    assert_eq!(uz.verdict(&t), Verdict::Pass { value_mm: zone_mm(s, 2.0), tol_mm: tol });
    assert!(matches!(eq.verdict(&t), Verdict::Fail { .. }), "{:?}", eq.verdict(&t));
    for k in [Kind::Flatness, Kind::Profile] {
        let c = Callout { kind: k, off_mm: None, ..uz.clone() };
        assert_eq!(c.verdict(&t), t.verdict(k, tol));
    }
}

// ---- VUGSCANTSDFEDGELATERAL (LEDGER SR574), VUG-SCAN-TSDF-EDGE-CARRY: the guard knobs survive a re-integration ----

#[test]
fn knobs_carry_through_reintegrate() {
    use scan_core::tsdf::Knobs;
    let (scene, intr, scale) = (Scene::bench(), fixture::bench_intrinsics(), DepthScale::D400_DEFAULT);
    let frames: Vec<Frame> = (0..2u32).map(|k| fixture::render_depth(&scene, &intr, scale, &pose(k), &Noise::NONE, k as u64)).collect();
    let mut t = Tsdf::new(4.0, 12.0, Tol::DEFAULT).expect("volume");
    assert_eq!(t.knobs(), Knobs::OFF, "the default is both guards off");
    let k = Knobs { edge_mm: 30.0, edge_far_mm: 12.0 };
    t = t.with_knobs(k);
    assert_eq!((t.edge_mm(), t.edge_far_mm()), (30.0, 12.0), "the knobs read back through the SR555 / SR562 getters");
    let poses: Vec<(u32, Transform)> = (0..2u32).map(|i| (i, pose(i))).collect();
    for &(i, p) in &poses {
        t.integrate(&frames[i as usize], scale, &intr, &p).expect("frame integrates");
    }
    let shot = |i: u32| frames.get(i as usize).map(|frame| scan_core::Shot { frame, scale, intr: &intr });
    let t2 = scan_core::reintegrate(&t, &poses, shot).expect("reintegrate");
    assert_eq!(t2.knobs(), k, "reintegrate carries the guard knobs, not Tsdf::new's defaults");
    assert_eq!((t2.frames(), t2.blocks()), (t.frames(), t.blocks()), "the same frames at the same poses fill the same blocks");
}
