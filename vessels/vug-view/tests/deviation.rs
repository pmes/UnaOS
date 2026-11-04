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
use vug_view::view::{Button, Preset}; // VUGFACEDEVIATIONPICK (SR543): Button for the click
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
    Viewer::open_deviation(s, &doc, "fixture", SIZE, Preset::Una, None).expect("the design builds") // VUGFACEGDTREMEASURE (SR560): the fallback margin
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

// VUGFACEDEVIATIONPICK (LEDGER SR543): a click on the coloured scan names the vertex under it on the corner's first
// line; a click off the scan clears it.

/// A scan vertex that is on the design, visible at its own projection (the view's hit there lands on a triangle that
/// names it) and clear of that triangle's other two vertices by 0.5 mm (a marching-cubes sliver is never the target):
/// `(vertex, x, y)`.
fn visible_vertex(v: &Viewer) -> (u32, f64, f64) {
    let fr = raster::render(&v.model, &v.view);
    let (idx, pos) = (v.model.indices(), v.model.positions());
    let d = v.view.deviation.as_ref().unwrap().result.as_ref().unwrap();
    let at = |i: u32| [0, 1, 2].map(|k| pos[i as usize * 3 + k] as f64);
    let dist = |a: [f64; 3], b: [f64; 3]| (0..3).map(|k| (a[k] - b[k]) * (a[k] - b[k])).sum::<f64>().sqrt();
    for y in (20..fr.height - 20).step_by(7) {
        for x in (20..fr.width - 20).step_by(7) {
            let t = fr.id[(y * fr.width + x) as usize];
            if t == u32::MAX {
                continue;
            }
            for k in 0..3 {
                let i = idx[t as usize * 3 + k];
                if d.per_vertex[i as usize].is_none() {
                    continue;
                }
                let Some((sx, sy)) = v.view.project_f64(at(i)) else { continue };
                let Some((_, ht)) = v.view.hit(&v.model, sx, sy) else { continue };
                let tri = &idx[ht as usize * 3..ht as usize * 3 + 3];
                if tri.contains(&i) && tri.iter().all(|&j| j == i || dist(at(j), at(i)) > 0.5) {
                    return (i, sx, sy);
                }
            }
        }
    }
    panic!("no visible matched vertex on screen");
}

#[test]
fn a_click_on_a_vertex_reads_its_deviation_and_face() {
    let mut v = open(scan().clone());
    let (i, x, y) = visible_vertex(&v);
    v.view.press(&v.model, Button::Left, 0, x, y);
    assert!(v.view.release(&v.model, x, y), "the click picks");
    let map = v.view.deviation.as_ref().unwrap();
    let d = map.result.as_ref().unwrap();
    let p = map.pick.as_ref().expect("a pick");
    assert_eq!(p.vertex, i, "the hit triangle's nearest vertex");
    let want = d.per_vertex[i as usize].expect("matched");
    let got = p.d.expect("the pick carries the deviation");
    assert!((got - want).abs() <= 1e-9, "d {got} vs {want}");
    let face = p.face.expect("a matched vertex names its face");
    assert_eq!(Some(face), d.face_of_vertex[i as usize]);
    let name = &map.names[face as usize];
    assert!(name.starts_with("body/") || name.starts_with("post/"), "{name}");
    let band = match shade(Some(want), BAND_MM) {
        Shade::Inside => "inside".to_string(),
        Shade::Under(k) => format!("under {k:.2}"),
        Shade::Over(k) => format!("over {k:.2}"),
        Shade::Off => "off".to_string(),
    };
    let lines = corner(map);
    println!("{}", lines.join("\n"));
    assert_eq!(lines[0], format!("pick: {name} d={want:+.3} {band}"));
    assert!(lines[1].starts_with("deviation rms="), "{}", lines[1]);
    // The frame carries the corner and the ring without panicking.
    let _ = v.frame();
}

#[test]
fn a_click_off_the_scan_clears_the_pick() {
    let mut v = open(scan().clone());
    let (_, x, y) = visible_vertex(&v);
    v.view.press(&v.model, Button::Left, 0, x, y);
    assert!(v.view.release(&v.model, x, y));
    assert!(v.view.deviation.as_ref().unwrap().pick.is_some());
    assert!(v.view.hit(&v.model, 2.0, 2.0).is_none(), "the corner pixel is off the scan");
    v.view.press(&v.model, Button::Left, 0, 2.0, 2.0);
    assert!(v.view.release(&v.model, 2.0, 2.0), "the click off the scan clears the pick");
    let map = v.view.deviation.as_ref().unwrap();
    assert!(map.pick.is_none());
    assert!(corner(map)[0].starts_with("deviation rms="), "{:?}", corner(map));
}

// VUGFACEDEVIATIONJOURNAL (LEDGER SR550): the scan measured against the document the tree holds reads as the same
// document read from a path; an edit of the journal re-measures, and its undo measures as before.

/// The SR510 document opened as the face's journal, the fixture scan measured against it.
fn journal_viewer() -> Viewer {
    let doc = doc_core::read(DOC).expect("the document reads");
    let mut v = Viewer::open_doc(doc, "fixture", SIZE, Preset::Una).expect("the document opens");
    v.against_journal(scan().clone()).expect("the journal measures");
    v
}

/// (rms, max, inside) of the view's measurement.
fn stats(v: &Viewer) -> (f64, f64, f64) {
    let d = v.view.deviation.as_ref().expect("a map").result.as_ref().expect("aligned");
    (d.stats.rms_mm, d.stats.max_mm, d.stats.inside)
}

#[test]
fn the_journal_measures_as_the_path_does() {
    let mut v = journal_viewer();
    let path = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("vugfacedeviationjournal.vug");
    std::fs::write(&path, doc_core::write(&v.view.design.journal.doc)).expect("the journal's document is written");
    let doc = doc_core::read(&std::fs::read_to_string(&path).expect("read back")).expect("it reads");
    let p = Viewer::open_deviation(scan().clone(), &doc, "fixture", SIZE, Preset::Una, None).expect("the design builds"); // VUGFACEGDTREMEASURE (SR560)
    let (a, b) = (stats(&v), stats(&p));
    println!("journal {a:?} path {b:?}");
    assert_eq!((a.0.to_bits(), a.1.to_bits(), a.2.to_bits()), (b.0.to_bits(), b.1.to_bits(), b.2.to_bits()), "journal {a:?} vs path {b:?}");
    let map = v.view.deviation.as_ref().unwrap();
    assert!(map.on, "the map opens on");
    let lines = corner(map);
    assert_eq!(lines.last().map(String::as_str), Some("design=journal@0 solids=2"), "{lines:?}"); // VUGFACEDEVIATIONLIVE (SR558): the solid count
    assert!(corner(p.view.deviation.as_ref().unwrap()).iter().all(|l| !l.starts_with("design=")), "a path map has no source line");
    assert!(v.view.design.body.is_none(), "the scan is drawn while the map is on");
    assert!(v.key("d"), "d toggles the map");
    assert!(v.view.design.body.is_some(), "map off: the journal's body is back");
    let _ = v.frame();
    let mut e = Viewer::open_doc(doc_core::Doc::new(doc_core::Tol::DEFAULT), "empty", SIZE, Preset::Una).expect("an empty document opens");
    assert_eq!(e.against_journal(scan().clone()), Err(vug_view::deviation::JOURNAL_EMPTY.to_string()));
    assert!(e.view.deviation.is_none(), "a refusal leaves no map");
}

#[test]
fn a_journal_edit_re_measures() {
    let mut v = journal_viewer();
    let r0 = stats(&v).0;
    let n = v.view.design.journal.doc.stmts.len();
    assert_eq!(v.view.design.journal.cursor, n - 1, "the cursor opens on the post's extrude");
    for k in ["t", "Return", "1", "4", "0", "Return"] {
        v.key(k);
    }
    assert!(
        matches!(v.view.design.journal.doc.stmts.last(), Some(doc_core::Stmt::Extrude { length, .. }) if *length == 140.0),
        "the panel edit lands: {:?} note {:?}",
        v.view.design.journal.doc.stmts.last(),
        v.view.design.journal.note
    );
    let map = v.view.deviation.as_ref().unwrap();
    let src = map.source.as_ref().expect("a journal map");
    assert_eq!((src.generation, src.measured, src.refused.as_deref()), (1, 1, None));
    let r1 = stats(&v).0;
    println!("rms gen0={r0} gen1={r1}");
    assert!((r1 - r0).abs() > 1e-6, "the post's top moved 10 mm: rms {r1} vs {r0}");
    assert_eq!(corner(map).last().map(String::as_str), Some("design=journal@1 solids=2"), "{:?}", corner(map)); // VUGFACEDEVIATIONLIVE (SR558)
    assert!(map.on && v.view.design.body.is_none(), "the re-measured scan is drawn");
    assert!(v.key("u"), "u undoes the edit");
    let src = v.view.deviation.as_ref().unwrap().source.as_ref().unwrap();
    assert_eq!((src.generation, src.measured), (2, 2));
    let r2 = stats(&v).0;
    assert_eq!(r2.to_bits(), r0.to_bits(), "the undone document measures as before: {r2} vs {r0}");
}

// VUGFACEGDT (LEDGER SR553): a gdt callout on the plate top reads scan_core's flatness and its verdict on the corner;
// a FAIL tints the face's scan vertices in the band ramp's over colour; `gdt clear` removes the line and the tint.

/// The box's top: the `body/` face whose nominal (scan_core `gdt`) is a plane facing +z — `(face, name)`.
fn plate_top(map: &vug_view::deviation::Map) -> (u32, String) {
    let f = map
        .gdt
        .iter()
        .find(|f| map.names[f.face as usize].starts_with("body/") && matches!(f.nominal, scan_core::Nominal::Plane(p) if p.normal.z > 0.99))
        .expect("the plate top is a plane");
    (f.face, map.names[f.face as usize].clone())
}

#[test]
fn a_flatness_callout_on_the_plate_top_reads_pass_then_fail() {
    let mut v = open(scan().clone());
    let (top, name) = plate_top(v.view.deviation.as_ref().unwrap());
    let flat = v.view.deviation.as_ref().unwrap().gdt[top as usize].flatness_mm.expect("the plate top is measured");
    assert!(flat > 0.001, "flatness {flat}");
    let before = v.view.deviation.as_ref().unwrap().vertex.clone();

    assert_eq!(vug_view::deviation::command(&mut v.view, &v.model, &format!("gdt {name}=flatness:100")), Ok(true));
    let map = v.view.deviation.as_ref().unwrap();
    let lines = corner(map);
    println!("{}", lines.join("\n"));
    assert_eq!(lines.last().unwrap(), &format!("{name} flatness={flat:.3} tol=100.000 PASS"));
    assert_eq!(map.vertex, before, "a PASS tints nothing");

    // The same face and kind replaces the callout.
    assert_eq!(vug_view::deviation::command(&mut v.view, &v.model, &format!("gdt {name}=flatness:0.001")), Ok(true));
    let map = v.view.deviation.as_ref().unwrap();
    assert_eq!(map.callouts.len(), 1, "one callout per face and kind");
    let lines = corner(map);
    println!("{}", lines.join("\n"));
    assert_eq!(lines.last().unwrap(), &format!("{name} flatness={flat:.3} tol=0.001 FAIL"));
    let d = map.result.as_ref().unwrap();
    let over = colour(Shade::Over(1.0));
    let (mut tinted, mut other) = (0, 0);
    for (i, f) in d.face_of_vertex.iter().enumerate() {
        if *f == Some(top) {
            assert_eq!(map.vertex[i], over, "vertex {i} on the failing face");
            tinted += 1;
        } else {
            assert_eq!(map.vertex[i], colour(shade(d.per_vertex[i], BAND_MM)), "vertex {i} off the failing face");
            other += 1;
        }
    }
    println!("tinted={tinted} other={other}");
    assert!(tinted > 0 && other > 0);
    // The albedo follows: a triangle whose three vertices are on the top is painted the over colour.
    let idx = v.model.indices();
    let t = (0..idx.len() / 3).find(|&t| (0..3).all(|k| d.face_of_vertex[idx[t * 3 + k] as usize] == Some(top))).expect("a triangle on the top");
    assert_eq!(map.tri[t], over);
    let _ = v.frame();
}

#[test]
fn gdt_clear_removes_the_callouts_and_the_tint() {
    let mut v = open(scan().clone());
    let (_, name) = plate_top(v.view.deviation.as_ref().unwrap());
    let m0 = v.view.deviation.clone().unwrap();
    let c0 = corner(&m0);
    assert_eq!(vug_view::deviation::command(&mut v.view, &v.model, &format!("gdt {name}=flatness:0.001")), Ok(true));
    let m1 = v.view.deviation.clone().unwrap();
    assert_ne!(m1.vertex, m0.vertex, "the FAIL tints");
    assert_eq!(corner(&m1).len(), c0.len() + 1);

    // Refusals by name change nothing.
    let e = vug_view::deviation::command(&mut v.view, &v.model, "gdt nosuch=flatness:1").unwrap_err();
    assert!(e.starts_with("GdtFace"), "{e}");
    let e = vug_view::deviation::command(&mut v.view, &v.model, &format!("gdt {name}=cylindricity:1")).unwrap_err();
    assert!(e.starts_with("GdtKind"), "{e}");
    let e = vug_view::deviation::command(&mut v.view, &v.model, &format!("gdt {name}=flatness:0")).unwrap_err();
    assert!(e.starts_with("usage"), "{e}");
    let m = v.view.deviation.as_ref().unwrap();
    assert_eq!((&m.callouts, &m.vertex, &m.tri), (&m1.callouts, &m1.vertex, &m1.tri));

    assert_eq!(vug_view::deviation::command(&mut v.view, &v.model, "gdt clear"), Ok(true));
    let m = v.view.deviation.as_ref().unwrap();
    assert!(m.callouts.is_empty());
    assert_eq!(corner(m), c0, "the callout line is gone");
    assert_eq!(m.vertex, m0.vertex, "the tint is gone");
    assert_eq!(m.tri, m0.tri, "the albedo is the open's");
    assert_eq!(vug_view::deviation::command(&mut v.view, &v.model, "gdt clear"), Ok(false), "nothing left to clear");
}

// VUGFACEGDTREMEASURE (LEDGER SR560): a callout and the datum frame survive a journal re-measure (re-read, re-tinted);
// the box datum frame A|B|C (the faces SR552's own test uses) moves the post's position line to scan_core's
// `position_in`; the margin argument changes the plate top's kept form vertices, and None is the constant.

/// The first `body/` plane face whose outward normal is within 8° of `n` (unit) — its name.
fn body_plane(map: &vug_view::deviation::Map, n: [f64; 3]) -> String {
    let f = map
        .gdt
        .iter()
        .find(|f| {
            map.names[f.face as usize].starts_with("body/")
                && matches!(f.nominal, scan_core::Nominal::Plane(p) if p.normal.x * n[0] + p.normal.y * n[1] + p.normal.z * n[2] > 0.99)
        })
        .unwrap_or_else(|| panic!("no body plane along {n:?}"));
    map.names[f.face as usize].clone()
}

/// The post's side: the `post/` face whose nominal is a cylinder — `(face, name)`.
fn post_side(map: &vug_view::deviation::Map) -> (u32, String) {
    let f = map
        .gdt
        .iter()
        .find(|f| map.names[f.face as usize].starts_with("post/") && matches!(f.nominal, scan_core::Nominal::Cylinder(_)))
        .expect("the post's side is a cylinder");
    (f.face, map.names[f.face as usize].clone())
}

/// The box datums: A the top (+z), B the -x side, C the -y side.
fn box_datums(map: &vug_view::deviation::Map) -> (String, String, String) {
    (body_plane(map, [0.0, 0.0, 1.0]), body_plane(map, [-1.0, 0.0, 0.0]), body_plane(map, [0.0, -1.0, 0.0]))
}

#[test]
fn a_callout_survives_a_journal_edit() {
    let mut v = journal_viewer();
    let m0 = v.view.deviation.clone().unwrap();
    let (_, top) = plate_top(&m0);
    let (a, b, c) = box_datums(&m0);
    assert_eq!(vug_view::deviation::command(&mut v.view, &v.model, &format!("gdt {top}=flatness:0.001")), Ok(true));
    assert_eq!(vug_view::deviation::command(&mut v.view, &v.model, &format!("datum A={a} B={b} C={c}")), Ok(true));
    for k in ["t", "Return", "1", "4", "0", "Return"] {
        v.key(k);
    }
    let map = v.view.deviation.as_ref().unwrap();
    let src = map.source.as_ref().expect("a journal map");
    assert_eq!((src.generation, src.measured, src.refused.as_deref()), (1, 1, None), "the edit re-measured");
    assert_ne!(map.basis.1, m0.basis.1, "the new map measured the edited design");
    let (top1, name1) = plate_top(map);
    assert_eq!(name1, top, "the plate top keeps its name");
    assert_eq!(map.callouts.len(), 1, "the callout followed: {:?}", map.callouts);
    assert_eq!((map.callouts[0].face, map.callouts[0].tol_mm), (top1, 0.001));
    // The gdt is the new measurement's: scan_core's own reading of the new basis.
    let (s, design) = &*map.basis;
    let d = map.result.as_ref().expect("aligned");
    let fresh = scan_core::gdt(s, design, d, vug_view::deviation::GDT_MARGIN_MM, Tol::DEFAULT);
    let flat = map.gdt[top1 as usize].flatness_mm.expect("the plate top is re-read");
    assert_eq!(Some(flat.to_bits()), fresh[top1 as usize].flatness_mm.map(f64::to_bits));
    let lines = corner(map);
    println!("{}", lines.join("\n"));
    assert!(lines.contains(&format!("{top} flatness={flat:.3} tol=0.001 FAIL")), "{lines:?}");
    assert!(lines.iter().any(|l| l.starts_with(&format!("frame A={a} B={b} C={c} residual="))), "the frame followed: {lines:?}");
    assert_eq!(lines.last().map(String::as_str), Some("design=journal@1 solids=2"), "{lines:?}"); // VUGFACEGDTDATUMSPICK (SR564): SR558's source line
    // Re-tinted: every vertex of the new map paired with the plate top is the over colour.
    let over = colour(Shade::Over(1.0));
    let mut tinted = 0;
    for (i, f) in d.face_of_vertex.iter().enumerate() {
        if *f == Some(top1) {
            assert_eq!(map.vertex[i], over, "vertex {i} on the failing face after the re-measure");
            tinted += 1;
        }
    }
    assert!(tinted > 0, "the plate top has vertices");
    let _ = v.frame();
}

#[test]
fn a_datum_frame_changes_the_post_position_line() {
    let mut v = open(scan().clone());
    let m0 = v.view.deviation.clone().unwrap();
    let (post, pname) = post_side(&m0);
    let (a, b, c) = box_datums(&m0);
    let p0 = m0.gdt[post as usize].position_mm.expect("the post's position is measured");
    assert_eq!(vug_view::deviation::command(&mut v.view, &v.model, &format!("gdt {pname}=position:5")), Ok(true));
    let c0 = corner(v.view.deviation.as_ref().unwrap());
    assert!(c0.last().unwrap().starts_with(&format!("{pname} position={p0:.3} tol=5.000")), "{c0:?}");

    // Refusals by name change nothing.
    let e = vug_view::deviation::command(&mut v.view, &v.model, &format!("datum A=nosuch B={b} C={c}")).unwrap_err();
    assert!(e.starts_with("GdtDatumFace A"), "{e}");
    let e = vug_view::deviation::command(&mut v.view, &v.model, &format!("datum A={a} B={b}")).unwrap_err();
    assert!(e.starts_with("usage"), "{e}");
    let e = vug_view::deviation::command(&mut v.view, &v.model, &format!("datum B={a} A={b} C={c}")).unwrap_err();
    assert!(e.starts_with("usage"), "{e}");
    let m = v.view.deviation.as_ref().unwrap();
    assert!(m.datums.is_none() && m.frame.is_none());
    assert_eq!(corner(m), c0);

    assert_eq!(vug_view::deviation::command(&mut v.view, &v.model, &format!("datum A={a} B={b} C={c}")), Ok(true));
    let m = v.view.deviation.as_ref().unwrap();
    let fr = m.frame.clone().expect("a frame").expect("the box datums fix a frame");
    let g = m.gdt[post as usize];
    let p1 = g.position_mm.expect("the post's position in the frame");
    let scan_core::Nominal::Cylinder(nc) = g.nominal else { panic!("the post is a cylinder") };
    assert_eq!(p1.to_bits(), scan_core::gdt::position_in(&fr, &g.axis.expect("a fitted axis"), &nc, g.extent).to_bits(), "scan_core's position_in");
    assert_ne!(p1.to_bits(), p0.to_bits(), "the frame moves the position: {p1} vs {p0}");
    let lines = corner(m);
    println!("{}", lines.join("\n"));
    let fl = format!("frame A={a} B={b} C={c} residual={:.3}", fr.residual_mm);
    let at = lines.iter().position(|l| *l == fl).unwrap_or_else(|| panic!("no frame line: {lines:?}"));
    assert_eq!(at + 2, lines.len(), "the frame line just before the callout line: {lines:?}");
    assert!(lines.last().unwrap().starts_with(&format!("{pname} position={p1:.3} tol=5.000")), "{lines:?}");

    assert_eq!(vug_view::deviation::command(&mut v.view, &v.model, "datum clear"), Ok(true));
    let m = v.view.deviation.as_ref().unwrap();
    assert!(m.datums.is_none() && m.frame.is_none());
    assert_eq!(corner(m), c0, "the registration's position is back");
    assert_eq!(vug_view::deviation::command(&mut v.view, &v.model, "datum clear"), Ok(false), "nothing left to clear");
}

#[test]
fn the_margin_argument_changes_n_kept() {
    let doc = doc_core::read(DOC).expect("the document reads");
    let at = |m: Option<f64>| Viewer::open_deviation(scan().clone(), &doc, "fixture", SIZE, Preset::Una, m).expect("the design builds");
    let (none, twelve, four) = (at(None), at(Some(12.0)), at(Some(4.0)));
    let map = |v: &Viewer| v.view.deviation.clone().expect("a map");
    let (mn, m12, m4) = (map(&none), map(&twelve), map(&four));
    assert_eq!(mn.margin_mm, vug_view::deviation::GDT_MARGIN_MM, "None is the fallback");
    assert_eq!(mn.gdt, m12.gdt, "the fallback is the constant, not another rule");
    let (top, _) = plate_top(&mn);
    let (n12, n4) = (mn.gdt[top as usize], m4.gdt[top as usize]);
    println!("plate top n={}/{} at 12 mm, n={}/{} at 4 mm", n12.n, n12.matched, n4.n, n4.matched);
    assert_eq!(n4.matched, n12.matched, "the margin does not change the pairing");
    assert!(n4.n > n12.n, "a narrower margin keeps more form vertices: {} vs {}", n4.n, n12.n);
}

// VUGFACEDEVIATIONLIVE (LEDGER SR558): a filleted document is measured against the fillet's solid only (journal.rs
// live_solids), and the journal map re-measures only when a statement replay feeds into a solid changes.

#[test]
fn a_filleted_document_measures_the_fillet_not_its_original() {
    let mut doc = doc_core::read(DOC).expect("the document reads");
    let rp = doc_core::replay(&doc, doc.tol);
    let body = &rp.solids.iter().find(|s| s.id == "body").expect("the box").brep;
    let near = |a: f64, x: f64| (a - x).abs() < 1e-6;
    let edge = body
        .edges
        .iter()
        .find(|e| e.v.iter().all(|&i| near(body.vertices[i as usize].p.y, -100.0) && near(body.vertices[i as usize].p.z, 100.0)))
        .expect("the box's top front edge")
        .name;
    doc.stmts.push(doc_core::Stmt::Fillet { id: "f0".into(), on: "body".into(), edges: vec![edge], r: 20.0 });
    let rp = doc_core::replay(&doc, doc.tol);
    assert!(rp.failures.is_empty(), "{:?}", rp.failures);
    assert_eq!(rp.solids.len(), 3, "replay keeps the box beside its fillet");
    let v = Viewer::open_deviation(scan().clone(), &doc, "fixture", SIZE, Preset::Una).expect("the design builds");
    let map = v.view.deviation.as_ref().unwrap();
    assert!(map.names.iter().all(|n| !n.starts_with("body/")), "the filleted box is not measured: {:?}", map.names);
    assert!(map.names.iter().any(|n| n.starts_with("f0/")) && map.names.iter().any(|n| n.starts_with("post/")), "{:?}", map.names);
    let live = stats(&v).0;
    let (all, _) = scan_core::design_of(rp.solids.iter().map(|s| (s.id.as_str(), &s.brep)), doc.tol).expect("every solid builds");
    let every = scan_core::measure(scan(), &all, Transform::IDENTITY, &scan_core::AlignParams::DEFAULT, BAND_MM, Tol::DEFAULT)
        .expect("aligned")
        .stats
        .rms_mm;
    println!("live rms={live} every rms={every}");
    assert!((live - every).abs() > 1e-6, "the live design measures apart from every solid: {live} vs {every}");
}

#[test]
fn an_open_sketch_edit_keeps_the_generation_and_a_solid_edit_bumps_it() {
    use doc_core::{Cons, PRef, Stmt};
    let mut v = journal_viewer();
    let r0 = stats(&v).0;
    let seen = |v: &Viewer| {
        let s = v.view.deviation.as_ref().unwrap().source.as_ref().expect("a journal map");
        (s.generation, s.measured, s.refused.clone())
    };
    let last = |v: &Viewer| corner(v.view.deviation.as_ref().unwrap()).last().cloned().unwrap_or_default();

    // A sketch no feature names, with a point and a fix: no solid statement moved.
    let mut doc = v.view.design.journal.doc.clone();
    doc.stmts.push(Stmt::Sketch { id: "s3".into(), o: [0.0, 0.0, 0.0], z: [0.0, 0.0, 1.0], x: [1.0, 0.0, 0.0] });
    doc.stmts.push(Stmt::Point { id: "q1".into(), sketch: "s3".into(), at: [0.0, 0.0] });
    doc.stmts.push(Stmt::Constraint { id: "q2".into(), sketch: "s3".into(), c: Cons::Fix(PRef::Point("q1".into()), [0.0, 0.0]) });
    assert_eq!(vug_view::journal::solid_stmts(&doc), vug_view::journal::solid_stmts(&v.view.design.journal.doc));
    v.view.design.journal.doc = doc.clone();
    vug_view::deviation::follow(&mut v);
    assert_eq!(seen(&v), (0, 0, None), "an open sketch does not re-measure");
    assert!(v.view.deviation.as_ref().unwrap().source.as_ref().unwrap().doc == doc, "the document is seen");
    assert_eq!(last(&v), "design=journal@0 solids=2");

    // A constraint edit in that sketch: still no re-measure.
    if let Some(Stmt::Constraint { c, .. }) = doc.stmts.last_mut() {
        *c = Cons::Fix(PRef::Point("q1".into()), [5.0, 0.0]);
    }
    v.view.design.journal.doc = doc.clone();
    vug_view::deviation::follow(&mut v);
    assert_eq!(seen(&v), (0, 0, None), "a constraint no solid reads does not re-measure");
    assert_eq!(stats(&v).0.to_bits(), r0.to_bits());
    assert!(v.view.design.body.is_none(), "the scan stays drawn");

    // The post's extrude 150 -> 140: a solid statement, re-measured.
    for s in doc.stmts.iter_mut() {
        if let Stmt::Extrude { id, length, .. } = s {
            if *id == "post" {
                *length = 140.0;
            }
        }
    }
    v.view.design.journal.doc = doc.clone();
    vug_view::deviation::follow(&mut v);
    assert_eq!(seen(&v), (1, 1, None), "an extrude edit re-measures");
    let r1 = stats(&v).0;
    assert!((r1 - r0).abs() > 1e-6, "the post's top moved 10 mm: {r1} vs {r0}");
    assert_eq!(last(&v), "design=journal@1 solids=2");

    // The post's radius 40 -> 45: a dimension in a sketch an extrude names moves the profile, re-measured.
    for s in doc.stmts.iter_mut() {
        if let Stmt::Constraint { id, c: Cons::Radius(_, r), .. } = s {
            if *id == "r1" {
                *r = 45.0;
            }
        }
    }
    v.view.design.journal.doc = doc.clone();
    vug_view::deviation::follow(&mut v);
    assert_eq!(seen(&v), (2, 2, None), "a dimension a solid reads re-measures");
    let r2 = stats(&v).0;
    println!("gen0 rms={r0} gen1={r1} gen2={r2}");
    assert!((r2 - r1).abs() > 1e-6, "the post widened 5 mm: {r2} vs {r1}");
}

// VUGFACEGDTDATUMSPICK (LEDGER SR564): with a pick held on the live map, `A` `B` `C` make the picked face that datum and
// the third builds the frame through the `datum` command's own road (bit-equal to the command form); a set datum is
// replaced; `D` clears (and toggles the map as before with no pick); lowercase `a` still opens a sketch. A re-measure
// that renames a callout's face names it on the corner (`callout lost <face>`) until the next command.

/// Pick design face `face` through the click's own fn (`deviation::pick`): the hit is a vertex of a triangle whose three
/// vertices pair with the face, so the nearest vertex names it.
fn pick_face(v: &mut Viewer, face: u32) {
    let p = {
        let d = v.view.deviation.as_ref().unwrap().result.as_ref().unwrap();
        let (idx, pos) = (v.model.indices(), v.model.positions());
        let t = (0..idx.len() / 3)
            .find(|&t| (0..3).all(|k| d.face_of_vertex.get(idx[t * 3 + k] as usize).copied().flatten() == Some(face)))
            .unwrap_or_else(|| panic!("no triangle on face {face}"));
        let i = idx[t * 3] as usize;
        (euclase::Vec3::new(pos[i * 3], pos[i * 3 + 1], pos[i * 3 + 2]), t as u32)
    };
    vug_view::deviation::pick(&mut v.view, &v.model, Some(p));
    assert_eq!(v.view.deviation.as_ref().unwrap().pick.as_ref().and_then(|p| p.face), Some(face), "the pick names the face");
}

/// The corner's frame line (`frame ...`).
fn frame_of_corner(m: &vug_view::deviation::Map) -> String {
    corner(m).into_iter().find(|l| l.starts_with("frame ")).unwrap_or_else(|| panic!("no frame line: {:?}", corner(m)))
}

#[test]
fn picked_datums_read_as_the_command_form() {
    let mut v = open(scan().clone());
    let m0 = v.view.deviation.clone().unwrap();
    let (a, b, c) = box_datums(&m0);
    let at = |n: &str| m0.names.iter().position(|x| x == n).unwrap() as u32;
    let (fa, fb, fc) = (at(&a), at(&b), at(&c));
    let (post, _) = post_side(&m0);

    // No pick: `D` toggles the map as before.
    assert!(v.key("D"), "D toggles");
    assert!(!v.view.deviation.as_ref().unwrap().on);
    assert!(v.key("D"));
    assert!(v.view.deviation.as_ref().unwrap().on);

    pick_face(&mut v, fa);
    assert!(v.key("A"), "A takes the picked face");
    let m = v.view.deviation.as_ref().unwrap();
    assert_eq!(m.chosen, [Some(fa), None, None]);
    assert!(m.datums.is_none() && m.frame.is_none(), "no frame before three");
    assert_eq!(frame_of_corner(m), format!("frame A={a} B=- C=-"));
    pick_face(&mut v, fb);
    assert!(v.key("B"));
    pick_face(&mut v, fc);
    assert!(v.key("C"));
    let mk = v.view.deviation.clone().unwrap();
    assert_eq!(mk.datums, Some([fa, fb, fc]), "the third pick builds the frame");

    // The command form on a second viewer.
    let mut w = open(scan().clone());
    assert_eq!(vug_view::deviation::command(&mut w.view, &w.model, &format!("datum A={a} B={b} C={c}")), Ok(true));
    let mc = w.view.deviation.clone().unwrap();
    let (fk, fm) = (mk.frame.clone().unwrap().expect("a frame"), mc.frame.clone().unwrap().expect("a frame"));
    println!("keys: {}\ncommand: {}", frame_of_corner(&mk), frame_of_corner(&mc));
    assert_eq!(fk.residual_mm.to_bits(), fm.residual_mm.to_bits(), "bit-equal residual");
    assert_eq!(mk.gdt[post as usize].position_mm.map(f64::to_bits), mc.gdt[post as usize].position_mm.map(f64::to_bits), "the post's position in the same frame");
    assert_eq!(frame_of_corner(&mk), frame_of_corner(&mc));
    assert_eq!(frame_of_corner(&mk), format!("frame A={a} B={b} C={c} residual={:.3}", fm.residual_mm));
    assert_eq!(mk.vertex, mc.vertex, "the same tint");

    // A datum set twice replaces: B the top, then B the -x side again.
    pick_face(&mut v, fa);
    assert!(v.key("B"));
    let m = v.view.deviation.as_ref().unwrap();
    assert_eq!(m.chosen, [Some(fa), Some(fa), Some(fc)]);
    assert_ne!(frame_of_corner(m), frame_of_corner(&mc), "B replaced");
    pick_face(&mut v, fb);
    assert!(v.key("B"));
    let m = v.view.deviation.as_ref().unwrap();
    assert_eq!(m.datums, Some([fa, fb, fc]));
    assert_eq!(frame_of_corner(m), frame_of_corner(&mc), "the command's frame again");
    assert_eq!(m.frame.clone().unwrap().unwrap().residual_mm.to_bits(), fm.residual_mm.to_bits());

    // `D` with a pick clears (the map stays on).
    assert!(v.key("D"), "D clears");
    let m = v.view.deviation.as_ref().unwrap();
    assert!(m.on, "D with a pick clears, it does not toggle");
    assert!(m.datums.is_none() && m.frame.is_none() && m.chosen == [None; 3]);
    assert!(!corner(m).iter().any(|l| l.starts_with("frame ")), "{:?}", corner(m));
    assert_eq!(m.gdt[post as usize].position_mm.map(f64::to_bits), m0.gdt[post as usize].position_mm.map(f64::to_bits), "the registration's position is back");

    // Lowercase `a` still opens a sketch with the pick held; inside it `A` is the sketch's.
    assert!(v.key("a"));
    assert!(v.view.design.face.is_some(), "a opens a sketch");
    assert!(v.key("A"));
    assert!(v.view.deviation.as_ref().unwrap().chosen.iter().all(Option::is_none), "inside a sketch A is not a datum key");
}

#[test]
fn a_renamed_face_names_its_lost_callout() {
    let mut v = journal_viewer();
    let m0 = v.view.deviation.clone().unwrap();
    let (_, pname) = post_side(&m0);
    assert_eq!(vug_view::deviation::command(&mut v.view, &v.model, &format!("gdt {pname}=position:5")), Ok(true));
    let doc = doc_core::read(&DOC.replace("extrude post ", "extrude peg ")).expect("the renamed document reads");
    v.view.design.journal.doc = doc.clone();
    vug_view::deviation::follow(&mut v);
    let m = v.view.deviation.as_ref().unwrap();
    let src = m.source.as_ref().expect("a journal map");
    assert_eq!((src.generation, src.measured, src.refused.as_deref()), (1, 1, None), "the rename re-measured");
    assert!(m.names.iter().all(|n| !n.starts_with("post/")), "the post's faces are renamed: {:?}", m.names);
    assert!(m.callouts.is_empty(), "the callout's face is gone");
    assert_eq!(m.lost, vec![pname.clone()]);
    let lines = corner(m);
    println!("{}", lines.join("\n"));
    let want = format!("callout lost {pname}");
    assert_eq!(lines.iter().filter(|l| **l == want).count(), 1, "{lines:?}");
    assert_eq!(lines[lines.len() - 2], want, "after the callout lines, before the source line: {lines:?}");

    // A journal change that moves no solid keeps the line.
    let mut doc2 = doc.clone();
    doc2.stmts.push(doc_core::Stmt::Sketch { id: "s9".into(), o: [0.0, 0.0, 0.0], z: [0.0, 0.0, 1.0], x: [1.0, 0.0, 0.0] });
    v.view.design.journal.doc = doc2;
    vug_view::deviation::follow(&mut v);
    assert_eq!(v.view.deviation.as_ref().unwrap().source.as_ref().unwrap().measured, 1, "no re-measure");
    assert!(corner(v.view.deviation.as_ref().unwrap()).contains(&want), "kept until the next command");

    // The next command clears it.
    assert_eq!(vug_view::deviation::command(&mut v.view, &v.model, "gdt clear"), Ok(false));
    let m = v.view.deviation.as_ref().unwrap();
    assert!(m.lost.is_empty());
    assert!(!corner(m).iter().any(|l| l.starts_with("callout lost")), "{:?}", corner(m));
}
