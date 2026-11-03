// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VUGFACEDEVIATION (LEDGER SR525): the SR510 fixture scan (scan_core tests/deviation.rs's fusion: 12 orbit frames at
//! 600 mm, 40° up, D435i noise) opened in the face against tools/scan's SR510 design document (the box and the post):
//! a coloured body whose vertex colours are scan_core's ramp of `shade` (sampled) and whose pixels follow `d`, the
//! corner naming the worst face, and a scan that does not lie on the design shown as `NoAlignment` by name.

use std::f64::consts::PI;
use std::sync::OnceLock;

use scan_core::fixture::{self, Noise, Scene};
use scan_core::{DepthScale, DeviationError, ScanMesh, Shade, Tol, Transform, Tsdf, Vec3, colour, extract, shade};
use vug_view::deviation::{BAND_MM, corner};
use vug_view::raster;
use vug_view::view::Preset;
use vug_view::Viewer;

const DEG: f64 = PI / 180.0;
const CENTRE: Vec3 = Vec3::new(-20.0, 0.0, 50.0);
const SIZE: (u32, u32) = (320, 240);

/// tools/scan's SR510 document: the fixture's box and post, built to the scene's dimensions.
const DOC: &str = r#"vug-doc version=1 lin=1e-6 ang=1e-9
sketch s1 o=[0 0 0] z=[0 0 1] x=[1 0 0]
point p1 sketch=s1 x=-230 y=-100
point p2 sketch=s1 x=70 y=-100
point p3 sketch=s1 x=70 y=100
point p4 sketch=s1 x=-230 y=100
line l1 sketch=s1 a=p1 b=p2
line l2 sketch=s1 a=p2 b=p3
line l3 sketch=s1 a=p3 b=p4
line l4 sketch=s1 a=p4 b=p1
fix f1 sketch=s1 p=p1 at=[-230 -100]
fix f2 sketch=s1 p=p2 at=[70 -100]
fix f3 sketch=s1 p=p3 at=[70 100]
fix f4 sketch=s1 p=p4 at=[-230 100]
sketch s2 o=[0 0 0] z=[0 0 1] x=[1 0 0]
point c0 sketch=s2 x=160 y=0
circle c1 sketch=s2 centre=c0 r=40
fix g1 sketch=s2 p=c0 at=[160 0]
radius r1 sketch=s2 circle=c1 r=40
extrude body sketch=s1 outer=l1 dir=[0 0 1] length=100
extrude post sketch=s2 outer=c1 dir=[0 0 1] length=150
"#;

/// The SR510 fixture scan, fused once.
fn scan() -> &'static ScanMesh {
    static S: OnceLock<ScanMesh> = OnceLock::new();
    S.get_or_init(|| {
        let (scene, intr, scale) = (Scene::bench(), fixture::bench_intrinsics(), DepthScale::D400_DEFAULT);
        let mut t = Tsdf::new(4.0, 12.0, Tol::DEFAULT).expect("volume");
        for k in 0..12u32 {
            let p = fixture::orbit(CENTRE, 600.0, 40.0 * DEG, k as f64 * 2.0 * PI / 12.0);
            let f = fixture::render_depth(&scene, &intr, scale, &p, &Noise::D435I, k as u64);
            t.integrate(&f, scale, &intr, &p).expect("frame integrates");
        }
        extract(&t).expect("surface")
    })
}

fn open(s: ScanMesh) -> Viewer {
    let doc = doc_core::read(DOC).expect("the document reads");
    Viewer::open_deviation(s, &doc, "fixture", SIZE, Preset::Una).expect("the design builds")
}

/// Green dominates the pixel (STATUS_POSITIVE under any light: the shade scales the three channels alike).
fn green(p: [u8; 4]) -> bool {
    p[1] > p[0] && p[1] > p[2]
}

#[test]
fn a_scan_against_its_document_is_a_coloured_body() {
    let mut v = open(scan().clone());
    let map = v.view.deviation.as_ref().expect("a map");
    let d = map.result.as_ref().expect("the fixture aligns from the identity");
    assert!(map.on, "the map opens on");
    assert_eq!(map.vertex.len(), d.per_vertex.len());
    assert_eq!(d.per_vertex.len(), scan().mesh.positions.len());
    let (mut inside, mut off) = (0, 0);
    for i in (0..d.per_vertex.len()).step_by(97) {
        let s = shade(d.per_vertex[i], BAND_MM);
        assert_eq!(map.vertex[i], colour(s), "vertex {i} {s:?}");
        match s {
            Shade::Inside => {
                assert_eq!(map.vertex[i], palette_core::STATUS_POSITIVE, "vertex {i}");
                inside += 1;
            }
            Shade::Off => {
                assert_eq!(map.vertex[i], palette_core::TEXT_SECONDARY.1, "vertex {i}");
                off += 1;
            }
            _ => {}
        }
    }
    println!("sampled inside={inside} off={off}");
    assert!(inside > 0, "no sampled vertex inside the band");
    assert!(off > 0, "the ground is no part of the design: some sampled vertex is off it");

    // A pixel on a triangle whose three vertices are inside the band is drawn green; after `d` it is not.
    let fr = raster::render(&v.model, &v.view);
    let idx = v.model.indices().to_vec();
    let all_inside = |t: u32| (0..3).all(|k| shade(d.per_vertex[idx[t as usize * 3 + k] as usize], BAND_MM) == Shade::Inside);
    let at = (0..fr.height)
        .filter(|&y| y > 60)
        .flat_map(|y| (60..fr.width).map(move |x| (x, y)))
        .find(|&(x, y)| {
            let t = fr.id[(y * fr.width + x) as usize];
            t != u32::MAX && all_inside(t)
        })
        .expect("an inside triangle is on screen");
    let before = fr.pixel(at.0, at.1);
    assert!(green(before), "inside pixel {at:?} = {before:?}");
    assert!(v.key("d"), "d toggles the map");
    assert!(!v.view.deviation.as_ref().unwrap().on);
    let after = raster::render(&v.model, &v.view).pixel(at.0, at.1);
    assert!(!green(after) && after != before, "map off: pixel {at:?} = {after:?} (was {before:?})");
}

#[test]
fn the_corner_names_the_worst_face() {
    let v = open(scan().clone());
    let map = v.view.deviation.as_ref().expect("a map");
    let d = map.result.as_ref().expect("aligned");
    let lines = corner(map);
    println!("{}", lines.join("\n"));
    assert!(lines[0].starts_with("deviation rms="), "{}", lines[0]);
    let worst = d.faces[0];
    assert!(worst.n > 0);
    let name = &map.names[worst.face as usize];
    assert!(name.starts_with("body/") || name.starts_with("post/"), "{name}");
    assert!(lines[1].starts_with(name.as_str()), "{} vs {name}", lines[1]);
    assert!(lines[1].contains(&format!("rms={:.3}", worst.rms_mm)) && lines[1].contains(&format!("mean={:+.3}", worst.mean_mm)), "{}", lines[1]);
    assert!(lines.len() <= 1 + vug_view::deviation::WORST);
    // The frame carries the corner without panicking.
    let _ = v.frame();
}

#[test]
fn a_scan_with_no_alignment_shows_no_alignment() {
    let t = Transform::from_translation(Vec3::new(500.0, 0.0, 0.0));
    let mut far = scan().clone();
    for p in &mut far.mesh.positions {
        *p = t.apply_point(*p);
    }
    let v = open(far);
    let map = v.view.deviation.as_ref().expect("a map");
    assert!(matches!(map.result, Err(DeviationError::NoAlignment { .. })), "{:?}", map.result.as_ref().err());
    assert!(!map.on && map.vertex.is_empty() && map.tri.is_empty());
    let lines = corner(map);
    assert!(lines[0].starts_with("refused NoAlignment"), "{lines:?}");
    let _ = v.frame();
}
