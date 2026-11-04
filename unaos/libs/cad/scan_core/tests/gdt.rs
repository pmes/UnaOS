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
    AlignParams, Cylinder, DepthScale, Design, Deviation, FaceGdt, GDT_MIN_POINTS, Kind, Nominal, ScanMesh, Tol, Transform, Tsdf, Vec3, Verdict, design_of, extract,
    fit_cylinder, fit_plane,
    gdt, measure, nominal, position_mm,
};

const DEG: f64 = PI / 180.0;
const CENTRE: Vec3 = Vec3::new(-20.0, 0.0, 50.0);
const BAND: f64 = 1.0;

/// tests/deviation.rs's fusion (SR510): 12 orbit frames at 600 mm, 40° up, D435i noise; with the volume's truncation,
/// the edge margin `gdt` takes (the seat's ruling: read from the fuse, no second constant).
fn fused() -> &'static (ScanMesh, f64) {
    static S: OnceLock<(ScanMesh, f64)> = OnceLock::new();
    S.get_or_init(|| {
        let (scene, intr, scale) = (Scene::bench(), fixture::bench_intrinsics(), DepthScale::D400_DEFAULT);
        let mut t = Tsdf::new(4.0, 12.0, Tol::DEFAULT).expect("volume");
        for k in 0..12u32 {
            let p = fixture::orbit(CENTRE, 600.0, 40.0 * DEG, k as f64 * 2.0 * PI / 12.0);
            let f = fixture::render_depth(&scene, &intr, scale, &p, &Noise::D435I, k as u64);
            t.integrate(&f, scale, &intr, &p).expect("frame integrates");
        }
        (extract(&t).expect("surface"), t.trunc_mm())
    })
}

fn scan() -> &'static ScanMesh {
    &fused().0
}

/// The fixture's box (-230..70, ±100, 0..100) and r 40 h 150 post at (160, 0) through `design_of`, each solid's
/// vertices moved into place (the box's faces come first), measured from the identity.
fn measured() -> &'static (Design, Vec<String>, Deviation) {
    static M: OnceLock<(Design, Vec<String>, Deviation)> = OnceLock::new();
    M.get_or_init(|| {
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
        let dev = measure(scan(), &d, Transform::IDENTITY, &AlignParams::DEFAULT, BAND, tol).expect("aligns");
        (d, names, dev)
    })
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

// ---- VUGSCANGDTSPEC (LEDGER SR559): the callout parsed once; profile of a surface ----

/// `Callout::parse` reads `<face>=<kind>:<tol>` at the LAST `=` and the LAST `:` (a face name carries `:` and may carry
/// `=`), its `Display` reads back to the same callout, and every malformed callout is refused by name.
#[test]
fn a_callout_parses_once_round_trips_and_refuses_by_name() {
    use scan_core::gdt::{Callout, GdtSpecError};
    let c = Callout::parse("box/f3:plane=flatness:0.05").expect("parses");
    assert_eq!(c, Callout { face: "box/f3:plane".into(), kind: Kind::Flatness, tol_mm: 0.05 });
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
