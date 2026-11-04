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
    let v = Viewer::open_deviation(scan().clone(), &doc, "fixture", SIZE, Preset::Una, None).expect("the design builds");
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

// VUGFACEDEVIATIONSOLIDFAULT (LEDGER SR565): a replay failure no solid reads (an Op with no machine, a conflicting
// stroke in an unnamed sketch) is counted on the corner and the map still measures; a failure in a solid's sketch still
// refuses by line. `Viewer::open_scan` (`vug-view part.vug --scan scan.ply`) opens the part as the journal.

/// An Op on the post with no machine in the document: replay's `NoMachine`, no solid reads it.
const DRILL: &str = "drill o1 on=post points=[[160 0]] top=150 depth=10 peck=0 tool=1 feed=300 plunge=100 rpm=10000\n";

#[test]
fn a_fault_no_solid_reads_still_measures() {
    let clean = open(scan().clone());
    let c0 = corner(clean.view.deviation.as_ref().unwrap());

    // The Op with no machine: measured as the clean document, the fault named and counted.
    let doc = doc_core::read(&format!("{DOC}{DRILL}")).expect("the document reads");
    let rp = doc_core::replay(&doc, doc.tol);
    assert_eq!(rp.failures.len(), 1, "{:?}", rp.failures);
    assert_eq!((rp.failures[0].id.as_str(), &rp.failures[0].fault), ("o1", &doc_core::Fault::NoMachine));
    let v = Viewer::open_deviation(scan().clone(), &doc, "fixture", SIZE, Preset::Una, None).expect("an Op fault does not refuse the design");
    let map = v.view.deviation.as_ref().unwrap();
    assert_eq!(map.faults, rp.failures);
    assert_eq!(
        (stats(&v).0.to_bits(), stats(&v).1.to_bits()),
        (stats(&clean).0.to_bits(), stats(&clean).1.to_bits()),
        "the design is the clean document's"
    );
    let lines = corner(map);
    println!("{}", lines.join("\n"));
    let line = rp.failures[0].line;
    assert_eq!(lines[..c0.len()], c0[..], "the measured lines unchanged");
    assert_eq!(lines[c0.len()..], [format!("fault line {line}: NoMachine o1"), "faults=1".to_string()]);

    // A conflicting stroke in a sketch no feature names: measured, its faults named on that sketch.
    let open_sketch = "sketch s3 o=[0 0 0] z=[0 0 1] x=[1 0 0]\npoint q1 sketch=s3 x=0 y=0\nfix q2 sketch=s3 p=q1 at=[0 0]\nfix q3 sketch=s3 p=q1 at=[5 0]\n";
    let doc = doc_core::read(&format!("{DOC}{open_sketch}")).expect("the document reads");
    let v = Viewer::open_deviation(scan().clone(), &doc, "fixture", SIZE, Preset::Una, None).expect("an unnamed sketch's fault does not refuse");
    let map = v.view.deviation.as_ref().unwrap();
    assert!(!map.faults.is_empty() && map.faults.iter().all(|f| ["s3", "q1", "q2", "q3"].contains(&f.id.as_str())), "{:?}", map.faults);
    assert_eq!(corner(map).last().cloned(), Some(format!("faults={}", map.faults.len())));

    // A conflicting radius in the post's sketch: a solid reads it, the design refuses by line as before.
    let doc = doc_core::read(&DOC.replace("radius r1 sketch=s2 circle=c1 r=40\n", "radius r1 sketch=s2 circle=c1 r=40\nradius r2 sketch=s2 circle=c1 r=45\n")).expect("reads");
    let e = Viewer::open_deviation(scan().clone(), &doc, "fixture", SIZE, Preset::Una, None).err().expect("a solid's fault refuses");
    assert!(e.starts_with("line ") && e.contains("Conflicting"), "{e}");

    // The journal: an Op enters the document, no solid statement moved: generation kept, the fault counted.
    let mut j = journal_viewer();
    let op = doc_core::read(&format!("{DOC}{DRILL}")).expect("reads").stmts.last().cloned().expect("the Op");
    let mut doc = j.view.design.journal.doc.clone();
    doc.stmts.push(op);
    j.view.design.journal.doc = doc;
    vug_view::deviation::follow(&mut j);
    let map = j.view.deviation.as_ref().unwrap();
    let src = map.source.as_ref().expect("a journal map");
    assert_eq!((src.generation, src.measured, src.refused.as_deref()), (0, 0, None), "an Op does not re-measure");
    let lines = corner(map);
    println!("{}", lines.join("\n"));
    assert!(lines.iter().any(|l| l.starts_with("fault line ") && l.ends_with(": NoMachine o1")), "{lines:?}");
    assert_eq!(lines.last().map(String::as_str), Some("design=journal@0 solids=2 faults=1"), "{lines:?}");
}

#[test]
fn scan_flag_opens_the_part_as_the_journal() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("vugfacedeviationsolidfault");
    std::fs::create_dir_all(&dir).expect("the fixture dir");
    let (part, ply) = (dir.join("part.vug"), dir.join("scan.ply"));
    std::fs::write(&part, DOC).expect("the part is written");
    std::fs::write(&ply, mesh_core::ply::write(&scan().mesh)).expect("the scan is written");
    let (part, ply) = (part.to_str().unwrap(), ply.to_str().unwrap());

    let j = Viewer::open_scan(part, ply, SIZE, Preset::Una).expect("the part opens as the journal with the scan measured");
    let p = Viewer::open_against(ply, part, SIZE, Preset::Una).expect("the path road");
    let map = j.view.deviation.as_ref().expect("a map");
    let src = map.source.as_ref().expect("a journal map");
    assert_eq!((src.generation, src.measured, src.refused.as_deref()), (0, 0, None));
    assert_eq!(j.save_to.as_deref(), Some(std::path::Path::new(part)), "the part saves back over itself");
    assert!(j.title.contains("part.vug"), "{}", j.title);
    let (lj, lp) = (corner(map), corner(p.view.deviation.as_ref().unwrap()));
    println!("{}", lj.join("\n"));
    assert_eq!(lj.last().map(String::as_str), Some("design=journal@0 solids=2"), "{lj:?}");
    assert_eq!(lj[..lj.len() - 1], lp[..], "the journal measures the scan as the path road does");

    let prb = dir.join("scan.prb");
    let e = Viewer::open_scan(part, prb.to_str().unwrap(), SIZE, Preset::Una).err().expect("a missing .prb refuses");
    assert!(e.contains("scan.prb"), "{e}"); // VUGFACEDEVIATIONPRB (SR610): a .prb is fused now, so a missing one refuses by its path
    let e = Viewer::open_scan(ply, ply, SIZE, Preset::Una).err().expect("a mesh is no part");
    assert!(e.ends_with("--scan opens a .vug part"), "{e}");
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
    assert_eq!(lines.last().map(String::as_str), Some("design=journal@1 solids=2"), "{lines:?}");
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




// VUGFACEJOURNALSOLIDFAULT (LEDGER SR572): a document with an Op naming no machine opens as the journal (it refused
// before), the map measures it, and a click on the corner's `fault line N` selects statement N in the panel through the
// panel's cursor; with the map off the body is the journal's own.

#[test]
fn a_click_on_a_corner_fault_line_selects_its_statement() {
    let doc = doc_core::read(&format!("{DOC}{DRILL}")).expect("the document reads");
    let mut v = Viewer::open_doc(doc, "fixture", SIZE, Preset::Una).expect("an Op's fault opens as the journal");
    assert_eq!(v.view.design.journal.faults.len(), 1, "{:?}", v.view.design.journal.faults);
    v.against_journal(scan().clone()).expect("the journal measures");
    let lines = corner(v.view.deviation.as_ref().unwrap());
    println!("{}", lines.join("\n"));
    let (i, line) = lines.iter().enumerate().find(|(_, l)| l.starts_with("fault line ")).expect("a fault line on the corner");
    let n: u32 = line["fault line ".len()..].split(':').next().unwrap().parse().expect("its line number");
    let text = line.to_uppercase();
    let (x0, y0) = vug_view::deviation::line_at(SIZE.0, &text, i);
    let (x, y) = ((x0 + 4.0 * text.chars().count() as f32) as f64, (y0 + vug_view::deviation::LINE_PX * 0.5) as f64);
    assert!(!v.view.design.journal.panel, "the panel starts hidden");
    v.input(&bandy::SMessage::BrowserButton { down: true, x, y, mods: 0, surface: 0 });
    assert!(v.input(&bandy::SMessage::BrowserButton { down: false, x, y, mods: 0, surface: 0 }), "the click is taken");
    let j = &v.view.design.journal;
    let o1 = j.doc.stmts.iter().position(|s| s.id() == Some("o1")).expect("the Op");
    println!("select line {n} -> row {}", j.cursor);
    assert!(j.panel, "the click shows the panel");
    assert_eq!((j.cursor, j.doc.line(j.cursor)), (o1, n), "the cursor is the faulted statement");
    assert!(j.faulted(o1), "its row is marked");

    // The map off: the body is the journal's own (regenerate carries the fault, no refusal).
    assert!(v.key("d"), "d toggles the map");
    assert!(v.view.design.body.is_some(), "the journal's body is shown with the map off");
}

// VUGFACEGDTREPORT (LEDGER SR571): the journal map with two callouts and a datum frame writes `<part>.vugreport` beside
// the part: the header, then the corner's own frame and callout lines (deviation::report_lines), equal to scan_core's
// formatter over scan_core's own reading of the map's basis; a map with no part path refuses ReportNoPath, nothing written.

#[test]
fn a_report_writes_the_corner_lines_beside_the_part() {
    use vug_view::deviation::{GDT_MARGIN_MM, REPORT_NO_PATH, report_lines};
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("vugfacegdtreport");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the fixture dir");
    let (part, out) = (dir.join("part.vug"), dir.join("part.vugreport"));

    // No part path: a journal never saved, a path map.
    let mut v = journal_viewer();
    assert!(v.save_to.is_none());
    assert_eq!(v.report(), Err(REPORT_NO_PATH.to_string()));
    let p = open(scan().clone());
    assert_eq!(p.report(), Err(REPORT_NO_PATH.to_string()));
    assert!(!out.exists(), "a refusal writes nothing");

    v.save_to = Some(part.clone());
    let m0 = v.view.deviation.clone().unwrap();
    let (_, top) = plate_top(&m0);
    let (post, pname) = post_side(&m0);
    let (a, b, c) = box_datums(&m0);
    assert_eq!(vug_view::deviation::command(&mut v.view, &v.model, &format!("gdt {top}=flatness:0.001")), Ok(true));
    assert_eq!(vug_view::deviation::command(&mut v.view, &v.model, &format!("gdt {pname}=position:5")), Ok(true));
    assert_eq!(vug_view::deviation::command(&mut v.view, &v.model, &format!("datum A={a} B={b} C={c}")), Ok(true));
    assert_eq!(v.report(), Ok(out.clone()), "written beside the part");
    let text = std::fs::read_to_string(&out).expect("the report reads back");
    println!("{text}");

    let map = v.view.deviation.as_ref().unwrap();
    let tag = scan_core::gdt::scan_tag(&map.basis.0);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], format!("# vugreport part.vug design=0 scan={tag}"));
    let rl = report_lines(map);
    assert_eq!(rl.len(), 3, "the frame and two callouts: {rl:?}");
    assert_eq!(lines[1..], rl.iter().map(String::as_str).collect::<Vec<_>>()[..], "the file's lines are report_lines");
    assert!(rl[0].starts_with(&format!("frame A={a} B={b} C={c} residual=")), "{rl:?}");
    assert!(rl[1].starts_with(&format!("{top} flatness=")) && rl[1].ends_with(" FAIL"), "{rl:?}");
    assert!(rl[2].starts_with(&format!("{pname} position=")), "{rl:?}");
    // The corner paints the same three lines, in order.
    let cl = corner(map);
    let at = cl.iter().position(|l| *l == rl[0]).unwrap_or_else(|| panic!("no frame line on the corner: {cl:?}"));
    assert_eq!(cl[at..at + 3], rl[..], "the corner's lines: {cl:?}");

    // The same text from scan_core's own reading of the map's basis, through the one formatter.
    let (s, design) = &*map.basis;
    let d = map.result.as_ref().expect("aligned");
    let idx = |n: &str| map.names.iter().position(|x| x == n).unwrap() as u32;
    let mut g = scan_core::gdt(s, design, d, GDT_MARGIN_MM, Tol::DEFAULT);
    let fr = scan_core::gdt::frame_of(s, design, d, [idx(&a), idx(&b), idx(&c)], GDT_MARGIN_MM, Tol::DEFAULT).expect("the box datums fix a frame");
    scan_core::gdt::in_frame(s, design, d, GDT_MARGIN_MM, &mut g, &fr); // seat fold: SR567's in_frame
    let want = vec![
        scan_core::gdt::frame_line([a.as_str(), b.as_str(), c.as_str()], fr.residual_mm),
        scan_core::gdt::callout_line(&top, "flatness", 0.001, g[idx(&top) as usize].verdict(scan_core::Kind::Flatness, 0.001)),
        scan_core::gdt::callout_line(&pname, "position", 5.0, g[post as usize].verdict(scan_core::Kind::Position, 5.0)),
    ];
    assert_eq!(text, scan_core::gdt::report_text("part.vug", 0, &tag, &want), "scan_core's formatter on scan_core's reading");
}


// VUGFACEJOURNALFAULTFIX (LEDGER SR578): the shown panel covers the frame's right quarter; the corner's lines move left
// of it (deviation::corner_origin, the one right edge the paint and the hit read), and a click on the moved fault line
// still selects its statement.

#[test]
fn the_corner_moves_left_of_the_shown_panel_and_its_fault_click_hits() {
    use vug_view::deviation::{LINE_PX, corner_origin, line_at};
    let doc = doc_core::read(&format!("{DOC}{DRILL}")).expect("the document reads");
    let mut v = Viewer::open_doc(doc, "fixture", SIZE, Preset::Una).expect("an Op's fault opens as the journal");
    v.against_journal(scan().clone()).expect("the journal measures");
    assert_eq!(corner_origin(&v.view, SIZE.0), SIZE.0, "panel hidden: the frame's right edge");
    assert!(v.key("t"), "t shows the panel");
    let x0 = vug_view::journal::panel_x0(SIZE.0);
    let right = corner_origin(&v.view, SIZE.0);
    assert_eq!(right, x0 as u32, "panel shown: its left edge");
    let lines = corner(v.view.deviation.as_ref().unwrap());
    for (i, l) in lines.iter().enumerate() {
        let t = l.to_uppercase();
        let n = 8.0 * t.chars().count() as f32;
        if n + 6.0 <= right as f32 {
            let (x, _) = line_at(right, &t, i);
            assert!(x + n <= x0, "line {i} clears the panel: {t}");
        }
    }
    let (i, line) = lines.iter().enumerate().find(|(_, l)| l.starts_with("fault line ")).expect("a fault line on the corner");
    let n: u32 = line["fault line ".len()..].split(':').next().unwrap().parse().expect("its line number");
    let text = line.to_uppercase();
    let (fx, fy) = line_at(right, &text, i);
    let (x, y) = ((fx + 4.0 * text.chars().count() as f32) as f64, (fy + LINE_PX * 0.5) as f64);
    assert!((x as f32) < x0, "the click is left of the panel");
    let o1 = v.view.design.journal.doc.stmts.iter().position(|s| s.id() == Some("o1")).expect("the Op");
    v.view.design.journal.cursor = 0;
    v.input(&bandy::SMessage::BrowserButton { down: true, x, y, mods: 0, surface: 0 });
    assert!(v.input(&bandy::SMessage::BrowserButton { down: false, x, y, mods: 0, surface: 0 }), "the click is taken");
    let j = &v.view.design.journal;
    println!("corner right hidden={} shown={right} fault line {n} -> row {}", SIZE.0, j.cursor);
    assert!(j.panel, "the panel stays shown");
    assert_eq!((j.cursor, j.doc.line(j.cursor)), (o1, n), "the moved fault line selects the faulted statement");
}

// VUGFACEGDTREPORTKEY (LEDGER SR575): the `report` command on the map's road (Viewer::map_command) writes the report and the
// corner names it until the next command; the header names the scan by its file name; a path map (open_against) writes
// beside its design at generation 0; scan_core's one kind token reads a kind's own name off the frame.

#[test]
fn a_report_command_writes_beside_the_part_and_a_path_map_beside_its_design() {
    use vug_view::deviation::report_lines;
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("vugfacegdtreportkey");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the fixture dir");
    let (part, design, ply) = (dir.join("part.vug"), dir.join("design.vugdoc"), dir.join("scan.ply"));
    std::fs::write(&part, DOC).expect("the part is written");
    std::fs::write(&design, DOC).expect("the design is written");
    std::fs::write(&ply, mesh_core::ply::write(&scan().mesh)).expect("the scan is written");
    let (ps, ds, ss) = (part.to_str().unwrap(), design.to_str().unwrap(), ply.to_str().unwrap());

    // The journal road: the part's report beside the part, the corner naming it once.
    let mut j = Viewer::open_scan(ps, ss, SIZE, Preset::Una).expect("the part opens with the scan measured");
    let (_, top) = plate_top(j.view.deviation.as_ref().unwrap());
    assert_eq!(j.map_command(&format!("gdt {top}=flatness:0.001")), Ok(true), "any other line is deviation::command's");
    assert_eq!(j.map_command("report"), Ok(true));
    let out = dir.join("part.vugreport");
    let text = std::fs::read_to_string(&out).expect("the report is written beside the part");
    println!("{text}");
    let map = j.view.deviation.as_ref().unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines[0], "# vugreport part.vug design=0 scan=scan.ply", "the scan named by its file");
    let rl = report_lines(map);
    assert_eq!(rl.len(), 1, "{rl:?}");
    assert_eq!(lines[1..], rl.iter().map(String::as_str).collect::<Vec<_>>()[..]);
    let said = format!("report {}", out.display());
    let cl = corner(map);
    assert_eq!(cl.iter().filter(|l| **l == said).count(), 1, "the corner names the report: {cl:?}");
    assert_eq!(j.map_command("gdt clear"), Ok(true));
    assert!(!corner(j.view.deviation.as_ref().unwrap()).contains(&said), "the next command clears the report line");

    // The path road: the design's report beside the design, generation 0.
    let mut p = Viewer::open_against(ss, ds, SIZE, Preset::Una).expect("the path road");
    assert_eq!(p.map_command(&format!("gdt {top}=flatness:0.001")), Ok(true));
    assert_eq!(p.report_command(), Ok(dir.join("design.vugreport")), "written beside the design");
    let text = std::fs::read_to_string(dir.join("design.vugreport")).expect("the report reads back");
    let pm = p.view.deviation.as_ref().unwrap();
    assert_eq!(text, scan_core::gdt::report_text("design.vugdoc", 0, "scan.ply", &report_lines(pm)));
    assert_eq!(text.lines().nth(1), Some(rl[0].as_str()), "the path road measures as the journal does");

    // The one kind token both writers read a callout under.
    assert_eq!(scan_core::gdt::kind_token(scan_core::Kind::Flatness, None), "flatness");
    assert_eq!(scan_core::gdt::kind_token(scan_core::Kind::Profile, None), "profile");
}

// VUGFACEGDTREPORTOPEN (LEDGER SR581): `report open` (on Viewer::map_command's road) reads the file `report` wrote into
// the View's sheet, painted in the panel by journal.rs's one painter; Escape closes it. `:` opens the command line when a
// map is up: its keys are the first (a typed `s` is the line's, never the save), Enter runs the line through
// map_command, a refusal keeps the line open by name, Up recalls the last line run, Escape closes.

/// A saved part (`part.vug`) and its scan (`scan.ply`) in a fresh fixture dir, opened as `--scan` opens them.
fn report_open_fixture(name: &str) -> (Viewer, std::path::PathBuf) {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("the fixture dir");
    let (part, ply) = (dir.join("part.vug"), dir.join("scan.ply"));
    std::fs::write(&part, DOC).expect("the part is written");
    std::fs::write(&ply, mesh_core::ply::write(&scan().mesh)).expect("the scan is written");
    let v = Viewer::open_scan(part.to_str().unwrap(), ply.to_str().unwrap(), SIZE, Preset::Una).expect("the part opens with the scan measured");
    (v, dir)
}

#[test]
fn report_open_paints_the_file_in_the_panel_and_escape_closes() {
    let (mut v, dir) = report_open_fixture("vugfacegdtreportopen-sheet");
    let err = v.map_command("report open").expect_err("no report written yet");
    assert!(err.starts_with("report open: "), "the read's refusal by name: {err}");
    assert!(v.view.sheet.is_none(), "a refusal shows nothing");
    let (_, top) = plate_top(v.view.deviation.as_ref().unwrap());
    assert_eq!(v.map_command(&format!("gdt {top}=flatness:0.001")), Ok(true));
    assert_eq!(v.map_command("report"), Ok(true));
    assert_eq!(v.map_command("report open"), Ok(true));
    let text = std::fs::read_to_string(dir.join("part.vugreport")).expect("the report is written");
    let sheet = v.view.sheet.as_ref().expect("the sheet is shown");
    assert_eq!(sheet.lines, text.lines().map(String::from).collect::<Vec<_>>(), "the sheet is the file's lines");
    assert_eq!(sheet.lines[0], "# vugreport part.vug design=0 scan=scan.ply");
    assert_eq!(sheet.top, 0);

    // The panel paints the sheet: the shown frame differs from the same view without it only in the panel.
    let shown = raster::render(&v.model, &v.view);
    let mut closed_view = v.view.clone();
    closed_view.sheet = None;
    let closed = raster::render(&v.model, &closed_view);
    let x0 = vug_view::journal::panel_x0(SIZE.0) as usize;
    let (mut inside, mut outside) = (0usize, 0usize);
    for (i, (a, b)) in shown.rgba.chunks(4).zip(closed.rgba.chunks(4)).enumerate() {
        if a != b {
            if i % SIZE.0 as usize >= x0 { inside += 1 } else { outside += 1 }
        }
    }
    println!("sheet {} lines: {inside} panel pixels differ, {outside} outside", sheet.lines.len());
    assert!(inside > 0, "the sheet is painted in the panel");
    assert_eq!(outside, 0, "and nowhere else");

    assert!(!v.key("Down"), "two lines fit: Down does not scroll");
    assert!(v.key("Escape"), "Escape closes the sheet");
    assert!(v.view.sheet.is_none());
    assert_eq!(raster::render(&v.model, &v.view).rgba, closed.rgba, "the frame is the view's without the sheet");

    // A map with no part and no design path is refused by name.
    let mut p = open(scan().clone());
    assert_eq!(p.map_command("report open"), Err(vug_view::deviation::REPORT_NO_PATH.to_string()));
    assert!(p.view.sheet.is_none());
}

#[test]
fn a_typed_command_line_runs_the_map_command() {
    let (mut v, _dir) = report_open_fixture("vugfacegdtreportopen-line");
    let (top, name) = plate_top(v.view.deviation.as_ref().unwrap());
    let disk = v.view.design.journal.disk.clone();
    assert!(v.key(":"), "`:` opens the line while a map is up");
    assert!(v.view.entry.open && v.view.entry.text.is_empty(), "the `:` is not typed");
    let line = format!("gdt {name}=flatness:0.05");
    assert!(v.input(&bandy::SMessage::BrowserText(line.clone())));
    assert_eq!(v.view.entry.text, line, "every key typed into the line");
    assert_eq!(v.view.design.journal.disk, disk, "the typed `s` of flatness is the line's, never the save");
    assert!(v.key("Return"), "Enter runs the line");
    assert!(!v.view.entry.open, "a line run closes");
    let m = v.view.deviation.as_ref().unwrap();
    assert_eq!(m.callouts, vec![vug_view::deviation::Callout { face: top, kind: scan_core::Kind::Flatness, tol_mm: 0.05 }], "the command ran");

    assert!(v.key(":"));
    assert!(v.key("Up"), "Up recalls the last line");
    assert_eq!(v.view.entry.text, line);
    while !v.view.entry.text.is_empty() {
        assert!(v.key("BackSpace"));
    }
    assert!(v.input(&bandy::SMessage::BrowserText("gdt nosuch=flatness:1".to_string())));
    assert!(v.key("Return"));
    assert!(v.view.entry.open, "a refused line stays open");
    assert_eq!(v.view.entry.refused.as_deref(), Some("GdtFace nosuch"), "named by map_command's refusal");
    assert!(v.key("x"));
    assert!(v.view.entry.refused.is_none(), "an edit clears the refusal");
    assert!(v.key("Escape"), "Escape closes the line");
    assert!(!v.view.entry.open);
    assert_eq!(v.view.deviation.as_ref().unwrap().callouts.len(), 1, "nothing else ran");

    // The line opens only over a map or a sheet (VUGFACESAVEASCARET SR593: this face has neither).
    let mut plain = Viewer::open_doc(doc_core::read(DOC).expect("the document reads"), "fixture", SIZE, Preset::Una).expect("the document opens");
    plain.key(":");
    assert!(!plain.view.entry.open, "no map: `:` opens nothing");
}

// VUGFACECOMMANDCARET (LEDGER SR587): the `:` line edits at a caret (sketch.rs `type_text_at` / `caret_key`, the Save As
// field's road with the caret at the end), painted by journal.rs `field_line_at`; any report file opens as the sheet
// (deviation.rs `Viewer::report_open_path`): `report open <path>` on the line, and `Viewer::open_report` with no map.

#[test]
fn the_command_line_edits_at_its_caret() {
    let (mut v, _dir) = report_open_fixture("vugfacecommandcaret-caret");
    assert!(v.key(":"));
    assert!(v.input(&bandy::SMessage::BrowserText("abcd".to_string())));
    assert_eq!((v.view.entry.text.as_str(), v.view.entry.caret), ("abcd", 4), "typing leaves the caret at the end");
    let at_end = raster::render(&v.model, &v.view);
    assert!(v.key("Left") && v.key("Left"), "Left moves the caret");
    assert_eq!(v.view.entry.caret, 2);
    let mid = raster::render(&v.model, &v.view);
    assert_ne!(mid.rgba, at_end.rgba, "the painted caret follows the caret");
    assert!(v.key("x"), "a char inserts at the caret");
    assert_eq!((v.view.entry.text.as_str(), v.view.entry.caret), ("abxcd", 3));
    assert!(v.key("Delete"), "Delete removes the char after the caret");
    assert_eq!((v.view.entry.text.as_str(), v.view.entry.caret), ("abxd", 3));
    println!("caret line {:?} at {}", v.view.entry.text, v.view.entry.caret);
    assert!(v.key("Right"));
    assert!(!v.key("Right"), "Right at the end does not move");
    assert!(v.key("\u{f702}"), "the Cocoa Left moves");
    assert_eq!((v.view.entry.text.as_str(), v.view.entry.caret), ("abxd", 3), "and is never typed");
    assert!(v.key("Home"));
    assert!(!v.key("Left"), "Left at the start does not move");
    v.key("BackSpace");
    assert_eq!((v.view.entry.text.as_str(), v.view.entry.caret), ("abxd", 0), "Backspace at the start removes nothing");
    assert!(v.key("End"));
    assert!(v.key("BackSpace"));
    assert_eq!((v.view.entry.text.as_str(), v.view.entry.caret), ("abx", 3), "Backspace removes the char before the caret");
    assert!(v.key("\u{7f}") && v.view.entry.text == "ab" && v.key("\u{f729}") && v.view.entry.caret == 0 && v.key("\u{f72b}") && v.view.entry.caret == 2, "VUGFACEFIELDKEYS (SR599): the one-char Backspace and the Cocoa Home / End reach the named arms: {:?} at {}", v.view.entry.text, v.view.entry.caret);

    // A run line recalled by Up puts the caret at its end: a typed char appends.
    while !v.view.entry.text.is_empty() {
        assert!(v.key("BackSpace"));
    }
    let (_, name) = plate_top(v.view.deviation.as_ref().unwrap());
    let line = format!("gdt {name}=flatness:0.05");
    assert!(v.input(&bandy::SMessage::BrowserText(line.clone())));
    assert!(v.key("Return"));
    assert!(!v.view.entry.open, "the line ran");
    assert!(v.key(":") && v.key("Up"));
    assert_eq!(v.view.entry.caret, line.chars().count(), "the recalled line's caret is at its end");
    assert!(v.key("9"));
    assert_eq!(v.view.entry.text, format!("{line}9"));
    assert!(v.key("Escape"));
    assert_eq!((v.view.entry.text.as_str(), v.view.entry.caret), ("", 0), "Escape clears the line and its caret");
}

#[test]
fn a_report_file_opens_as_the_sheet_with_no_map() {
    let (mut m, dir) = report_open_fixture("vugfacecommandcaret-file");
    let (_, top) = plate_top(m.view.deviation.as_ref().unwrap());
    assert_eq!(m.map_command(&format!("gdt {top}=flatness:0.001")), Ok(true));
    assert_eq!(m.map_command("report"), Ok(true), "SR571's road writes the file");
    let path = dir.join("part.vugreport");
    let text = std::fs::read_to_string(&path).expect("the report is written");
    let lines: Vec<String> = text.lines().map(String::from).collect();
    let ps = path.to_str().unwrap();

    // Opened on its own: no map, the file's lines as the sheet, painted in the panel and nowhere else.
    let mut v = Viewer::open_report(ps, SIZE, Preset::Una).expect("the report opens");
    assert!(v.view.deviation.is_none(), "no map");
    assert_eq!(v.title, "part.vugreport");
    assert_eq!(v.view.sheet.as_ref().expect("the sheet is shown").lines, lines, "the sheet is the file's lines");
    let shown = raster::render(&v.model, &v.view);
    let mut closed_view = v.view.clone();
    closed_view.sheet = None;
    let closed = raster::render(&v.model, &closed_view);
    let x0 = vug_view::journal::panel_x0(SIZE.0) as usize;
    let (mut inside, mut outside) = (0usize, 0usize);
    for (i, (a, b)) in shown.rgba.chunks(4).zip(closed.rgba.chunks(4)).enumerate() {
        if a != b {
            if i % SIZE.0 as usize >= x0 { inside += 1 } else { outside += 1 }
        }
    }
    println!("report file {} lines, no map: {inside} panel pixels differ, {outside} outside", lines.len());
    assert!(inside > 0, "the sheet is painted in the panel");
    assert_eq!(outside, 0, "and nowhere else");
    assert!(v.key("Escape"), "Escape closes the sheet");
    assert!(v.view.sheet.is_none());
    assert_eq!(v.map_command(&format!("report open {ps}")), Ok(true), "`report open <path>` needs no map");
    assert_eq!(v.view.sheet.as_ref().map(|s| s.lines.clone()), Some(lines.clone()));

    // On the map's face the typed line opens the file and closes.
    assert!(m.view.sheet.is_none());
    assert!(m.key(":"));
    assert!(m.input(&bandy::SMessage::BrowserText(format!("report open {ps}"))));
    assert!(m.key("Return"));
    assert!(!m.view.entry.open, "the line ran");
    assert_eq!(m.view.sheet.as_ref().map(|s| s.lines.clone()), Some(lines), "the typed path's file is the sheet");

    // A missing file is refused by name, nothing shown.
    let gone = dir.join("nosuch.vugreport");
    let gs = gone.to_str().unwrap();
    assert!(m.key("Escape"));
    let err = m.map_command(&format!("report open {gs}")).expect_err("no such file");
    assert!(err.starts_with("report open: "), "the read's refusal by name: {err}");
    assert!(m.view.sheet.is_none(), "a refusal shows nothing");
    assert!(Viewer::open_report(gs, SIZE, Preset::Una).is_err(), "open_report refuses it too");
}


// ---- VUGFACECONFIDENCETINT (LEDGER SR592): the confidence floor on the map's command road ----

/// `conf 4` unpairs exactly the vertices paired in the measurement whose fused weight is under 4 (scan_core
/// `deviation::floored`), paints them the unmeasured tint, names the count on the corner and keeps the statistics;
/// `conf off` restores the measurement; a malformed floor is the usage.
#[test]
fn conf_floor_tints_the_low_weight_vertices_unmeasured() {
    let mut v = open(scan().clone());
    let raw = v.view.deviation.as_ref().expect("a map").result.clone().expect("the fixture aligns");
    let paired = raw.per_vertex.iter().filter(|x| x.is_some()).count();
    let low: Vec<usize> = (0..raw.per_vertex.len()).filter(|&i| raw.per_vertex[i].is_some() && scan().confidence[i] < 4.0).collect();
    assert!(!low.is_empty() && low.len() < paired, "some but not all paired vertices sit under weight 4: {} of {paired}", low.len());
    assert_eq!(v.map_command("conf 4"), Ok(true));
    let map = v.view.deviation.as_ref().expect("a map");
    let d = map.result.as_ref().expect("still measured");
    for i in 0..raw.per_vertex.len() {
        if low.contains(&i) {
            assert_eq!(d.per_vertex[i], None, "vertex {i}: under the floor, unpaired");
            assert_eq!(map.vertex[i], colour(Shade::Off), "vertex {i}: the unmeasured tint");
        } else {
            assert_eq!(d.per_vertex[i], raw.per_vertex[i], "vertex {i}: kept as measured");
        }
    }
    assert_eq!(d.stats, raw.stats, "the statistics stay as measured");
    let line = format!("conf floor=4 unpaired={}", low.len());
    assert!(corner(map).contains(&line), "the corner names the floor: {:?}", corner(map));
    assert_eq!(v.map_command("conf off"), Ok(true));
    let map = v.view.deviation.as_ref().expect("a map");
    assert_eq!(map.result.as_ref().expect("measured").per_vertex, raw.per_vertex, "conf off restores the measurement");
    assert!(!corner(map).iter().any(|l| l.starts_with("conf ")), "no floor line after conf off");
    for bad in ["conf", "conf -1", "conf x"] {
        assert_eq!(v.map_command(bad), Err("usage: conf <floor> | conf off".to_string()), "{bad}: the usage");
    }
}

// VUGFACEPATHBESIDE (LEDGER SR594): ONE typed-path rule (journal.rs `beside`): a bare name typed on the `:` line's
// `report open <name>` and in the Save As field both land beside the opened part; a path map's anchor is its design
// document; with nothing opened a relative name is the cwd's; an absolute path is as typed.

#[test]
fn a_bare_name_is_beside_the_opened_part_for_report_open_and_save_as() {
    let (mut v, dir) = report_open_fixture("vugfacepathbeside");
    let (_, top) = plate_top(v.view.deviation.as_ref().unwrap());
    assert_eq!(v.map_command(&format!("gdt {top}=flatness:0.001")), Ok(true));
    assert_eq!(v.map_command("report"), Ok(true), "the report is written beside the part");
    let report = dir.join("part.vugreport");
    let lines: Vec<String> = std::fs::read_to_string(&report).expect("the report is written").lines().map(String::from).collect();
    assert_eq!(vug_view::journal::beside(&v, "part.vugreport"), report, "a bare name is beside the opened part");

    // `report open <bare name>` reads the file `report` wrote, not a cwd file of that name.
    assert_eq!(v.map_command("report open part.vugreport"), Ok(true));
    assert_eq!(v.view.sheet.as_ref().map(|s| s.lines.clone()), Some(lines), "the sheet is the file beside the part");
    assert!(v.key("Escape"), "Escape closes the sheet");
    assert!(v.view.sheet.is_none());

    // Save As of a bare name lands at the same rule's path.
    let want = vug_view::journal::beside(&v, "copy.vug");
    assert_eq!(want, dir.join("copy.vug"));
    assert!(v.key("Meta+Shift+s"), "the Save As field opens");
    assert!(v.input(&bandy::SMessage::BrowserText("copy".to_string())));
    assert!(v.key("Return"), "Enter saves");
    println!("beside: report {} save as {}", report.display(), want.display());
    assert_eq!(v.save_to.as_deref(), Some(want.as_path()), "{:?}", v.view.design.journal.disk);
    assert!(want.exists(), "the document is written beside the part");

    // An absolute path is as typed.
    let abs = std::env::temp_dir().join("vugfacepathbeside-abs.vugreport");
    assert_eq!(vug_view::journal::beside(&v, abs.to_str().unwrap()), abs);

    // A path map (no part: save_to None) is anchored on its design document.
    let (part, ply) = (dir.join("part.vug"), dir.join("scan.ply"));
    let pm = Viewer::open_against(ply.to_str().unwrap(), part.to_str().unwrap(), SIZE, Preset::Una).expect("the path map opens");
    assert!(pm.save_to.is_none());
    assert_eq!(vug_view::journal::beside(&pm, "part.vugreport"), report, "beside the design document");

    // Nothing opened: a relative name is the cwd's, as typed.
    let plain = Viewer::open_doc(doc_core::read(DOC).expect("the document reads"), "fixture", SIZE, Preset::Una).expect("the document opens");
    assert_eq!(vug_view::journal::beside(&plain, "x.vugreport"), std::path::PathBuf::from("x.vugreport"));
}

// VUGFACESAVEASCARET (LEDGER SR593): the `:` line opens over a face with a sheet and no map (deviation.rs `line_key`'s
// gate reads map or sheet); `report open <path>` runs there and the map's words refuse by name.

#[test]
fn the_command_line_opens_over_a_sheet_with_no_map() {
    let (mut m, dir) = report_open_fixture("vugfacesaveascaret-nomap");
    let (_, top) = plate_top(m.view.deviation.as_ref().unwrap());
    assert_eq!(m.map_command(&format!("gdt {top}=flatness:0.001")), Ok(true));
    assert_eq!(m.map_command("report"), Ok(true), "SR571's road writes the file");
    let path = dir.join("part.vugreport");
    let lines: Vec<String> = std::fs::read_to_string(&path).expect("the report is written").lines().map(String::from).collect();
    let ps = path.to_str().unwrap();

    let mut v = Viewer::open_report(ps, SIZE, Preset::Una).expect("the report opens");
    assert!(v.view.deviation.is_none(), "no map");
    assert!(v.key(":"), "a sheet with no map opens the line");
    assert!(v.view.entry.open && v.view.entry.text.is_empty(), "the `:` is not typed");
    assert!(v.input(&bandy::SMessage::BrowserText(format!("report open {ps}"))));
    assert!(v.key("Return"), "Enter runs the line");
    assert!(!v.view.entry.open, "`report open <path>` ran with no map");
    assert_eq!(v.view.sheet.as_ref().map(|s| s.lines.clone()), Some(lines), "the typed path's file is the sheet");

    assert!(v.key(":"));
    assert!(v.input(&bandy::SMessage::BrowserText(format!("gdt {top}=flatness:0.1"))));
    assert!(v.key("Return"));
    assert!(v.view.entry.open, "a refused line stays open");
    assert_eq!(v.view.entry.refused.as_deref(), Some("gdt: no map"), "the map's word refused by name");
    println!("no-map line refused {:?}", v.view.entry.refused);
    assert!(v.key("Escape"), "Escape closes the line first");
    assert!(!v.view.entry.open && v.view.sheet.is_some(), "the sheet stays");
    assert!(v.key("Escape"), "Escape closes the sheet");
    assert!(v.view.sheet.is_none());
    v.key(":");
    assert!(!v.view.entry.open, "no map and no sheet: `:` opens nothing");
}

// ---- VUGFACEDEVIATIONHELD (LEDGER SR600): the map opens beside a held statement and re-measures when its fix lands ----
// SR598's shape on the SR510 part: the post's `outer` names its centre point (NoProfile, the solid half), so journal.rs
// `regenerate` holds it with its card while deviation.rs `design` refused the whole document; now the design is the
// standing document's (the box alone), the corner names `held=post`, and the card's Tab + Return replaces the statement,
// `follow` re-measures against both solids and the held word is gone. Two roots still refuse by the first line.

/// The SR510 document with the post's `outer` on the point `c0` and a second closed loop in s2 (a fixed circle away
/// from the part, listed before c1), so the post's card offers two loops.
fn held_doc() -> String {
    DOC.replace(
        "sketch s2 o=[0 0 0] z=[0 0 1] x=[1 0 0]\n",
        "sketch s2 o=[0 0 0] z=[0 0 1] x=[1 0 0]\npoint c8 sketch=s2 x=-400 y=300\ncircle c9 sketch=s2 centre=c8 r=10\nfix g8 sketch=s2 p=c8 at=[-400 300]\nradius r9 sketch=s2 circle=c9 r=10\n",
    )
    .replace("extrude post sketch=s2 outer=c1 ", "extrude post sketch=s2 outer=c0 ")
}

#[test]
fn a_held_document_opens_the_map_held_and_its_fix_re_measures() {
    let text = held_doc();
    let doc = doc_core::read(&text).expect("an outer on a point is geometry, not syntax: the document reads");
    let mut v = Viewer::open_doc(doc.clone(), "fixture", SIZE, Preset::Una).expect("SR598: the journal holds the post");
    v.against_journal(scan().clone()).expect("the map opens beside the held post");
    let post = v.view.design.journal.doc.stmts.iter().position(|s| s.id() == Some("post")).expect("the post's row");
    let line = v.view.design.journal.doc.line(post);
    let map = v.view.deviation.as_ref().expect("a map");
    let lines = corner(map);
    println!("{}", lines.join("\n"));
    assert_eq!(map.held.as_deref(), Some("post"));
    assert!(lines.contains(&"held=post".to_string()), "{lines:?}");
    assert!(lines.contains(&format!("fault line {line}: NoProfile post")), "{lines:?}");
    assert_eq!(lines.last().map(String::as_str), Some("design=journal@0 solids=1 faults=1"), "{lines:?}");
    assert!(!map.names.is_empty() && map.names.iter().all(|n| !n.starts_with("post/")), "the box alone: {:?}", map.names);
    let rms0 = map.result.as_ref().map(|d| d.stats.rms_mm).map_err(|e| format!("{e:?}"));

    // The path road opens the same held document.
    let p = Viewer::open_deviation(scan().clone(), &doc, "fixture", SIZE, Preset::Una, None).expect("the path map opens held");
    let pm = p.view.deviation.as_ref().expect("a map");
    assert_eq!(pm.held.as_deref(), Some("post"));
    assert!(corner(pm).contains(&"held=post".to_string()), "{:?}", corner(pm));

    // The card: Tab to the post's own circle, Return replaces the statement and the map re-measures.
    assert!(v.view.design.journal.select_line(line), "the post's line is a row");
    let texts: Vec<String> = v.view.design.journal.fixes_for(post).into_iter().map(|f| f.text).collect();
    assert_eq!(texts.len(), 2, "two closed loops: {texts:?}");
    let want = texts.iter().position(|t| t.contains("outer=c1 ")).expect("c1 is offered");
    for _ in 0..want {
        assert!(v.key("Tab"), "Tab walks the card");
    }
    assert_eq!(v.view.design.journal.offered().map(|f| f.text), Some(texts[want].clone()));
    assert!(v.key("Return"), "Return takes the pick");
    assert!(v.view.design.journal.faults.is_empty(), "{:?}", v.view.design.journal.faults);
    let map = v.view.deviation.as_ref().expect("a map");
    let src = map.source.as_ref().expect("a journal map");
    assert_eq!((src.generation, src.measured, src.refused.as_deref()), (1, 1, None), "the fix re-measured");
    let lines = corner(map);
    println!("{}", lines.join("\n"));
    assert_eq!(map.held, None);
    assert!(lines.iter().all(|l| !l.starts_with("held=")), "{lines:?}");
    assert_eq!(lines.last().map(String::as_str), Some("design=journal@1 solids=2"), "{lines:?}");
    assert!(map.names.iter().any(|n| n.starts_with("post/")), "the post joins the basis: {:?}", map.names);
    let rms1 = map.result.as_ref().expect("both solids align").stats.rms_mm;
    println!("deviationheld held=post solids=1->2 gen=1 rms0={rms0:?} rms1={rms1}");

    // Two roots: the box's outer on a point too; VUGFACEJOURNALHOLDMANY (SR609): both are held, no solid stands, so the map refuses DesignEmpty.
    let two = doc_core::read(&text.replace("extrude body sketch=s1 outer=l1 ", "extrude body sketch=s1 outer=p1 ")).expect("reads");
    let e = Viewer::open_deviation(scan().clone(), &two, "fixture", SIZE, Preset::Una, None).err().expect("nothing stands to measure");
    assert_eq!(e, "DesignEmpty", "{e}");
}

// ---- VUGFACEJOURNALHOLDMANY (LEDGER SR609): two held roots are named together and the first fix re-measures ----
// SR600's held document with a second root (`post2`, its own sketch s3, its `outer` on a point): the map opens beside
// both, the corner names `held=post,post2` and both fault lines; the post's card (the first root's) takes c1 and
// `follow` re-measures with the post in the basis and `held=post2` left.

#[test]
fn two_held_roots_name_both_on_the_corner_and_the_first_fix_re_measures() {
    let text = held_doc() + "sketch s3 o=[0 0 0] z=[0 0 1] x=[1 0 0]\npoint d7 sketch=s3 x=-380 y=-300\nfix g7 sketch=s3 p=d7 at=[-380 -300]\nextrude post2 sketch=s3 outer=d7 dir=[0 0 1] length=10\n";
    let doc = doc_core::read(&text).expect("two outers on points are geometry, not syntax: the document reads");
    let mut v = Viewer::open_doc(doc, "fixture", SIZE, Preset::Una).expect("SR609: the journal holds both posts");
    v.against_journal(scan().clone()).expect("the map opens beside both held posts");
    let at = |v: &Viewer, id: &str| {
        let j = &v.view.design.journal;
        let i = j.doc.stmts.iter().position(|s| s.id() == Some(id)).expect("the row");
        (i, j.doc.line(i))
    };
    let ((post, line), (_, line2)) = (at(&v, "post"), at(&v, "post2"));
    let map = v.view.deviation.as_ref().expect("a map");
    let lines = corner(map);
    println!("{}", lines.join("\n"));
    assert_eq!(map.held.as_deref(), Some("post,post2"));
    assert!([String::from("held=post,post2"), format!("fault line {line}: NoProfile post"), format!("fault line {line2}: NoProfile post2")].map(|w| lines.iter().position(|l| *l == w)).windows(2).all(|p| p[0].is_some() && p[0] < p[1]), "VUGFACEJOURNALHOLDREST (SR611): held= order unchanged, named first: {lines:?}");
    assert!(lines.contains(&"held=post,post2".to_string()), "{lines:?}");
    assert!(lines.contains(&format!("fault line {line}: NoProfile post")), "{lines:?}");
    assert!(lines.contains(&format!("fault line {line2}: NoProfile post2")), "{lines:?}");
    assert_eq!(lines.last().map(String::as_str), Some("design=journal@0 solids=1 faults=2"), "{lines:?}");

    // The first root's card: Tab to the post's own circle, Return replaces it and the map re-measures.
    assert!(v.view.design.journal.select_line(line), "the post's line is a row");
    let texts: Vec<String> = v.view.design.journal.fixes_for(post).into_iter().map(|f| f.text).collect();
    let want = texts.iter().position(|t| t.contains("outer=c1 ")).expect("c1 is offered");
    for _ in 0..want {
        assert!(v.key("Tab"), "Tab walks the card");
    }
    assert!(v.key("Return"), "Return takes the pick");
    let ids: Vec<String> = v.view.design.journal.faults.iter().map(|f| f.id.clone()).collect();
    assert_eq!(ids, ["post2"], "the second root still held");
    let map = v.view.deviation.as_ref().expect("a map");
    let src = map.source.as_ref().expect("a journal map");
    assert_eq!((src.generation, src.measured, src.refused.as_deref()), (1, 1, None), "the fix re-measured");
    let lines = corner(map);
    println!("{}", lines.join("\n"));
    assert_eq!(map.held.as_deref(), Some("post2"));
    assert!(lines.contains(&"held=post2".to_string()), "{lines:?}");
    assert_eq!(lines.last().map(String::as_str), Some("design=journal@1 solids=2 faults=1"), "{lines:?}");
    assert!(map.names.iter().any(|n| n.starts_with("post/")), "the post joins the basis: {:?}", map.names);
    println!("deviationholdmany held=post,post2->post2 solids=1->2 gen=1");
}

// ---- VUGFACEDEVIATIONPRB (LEDGER SR610): a .prb fused by the face itself (scan_core `fuse::fuse_prb`) ----
// One D435i frame of the bench from the orbit's azimuth 0, written as a .prb through probe_core's writer (the bytes
// tools/scan reads). A recording carries no pose, so its one frame is fused at the identity: the design is DOC moved into
// that camera's frame (each sketch's o / z / x and each extrude's dir through the inverse pose). The map's basis is the
// mesh scan_core's own road fuses from the same frame (tools/scan's defaults: voxel 4, trunc 12), and the first callout
// reads the same on both maps.

#[test]
fn a_prb_opens_against_the_part_as_scan_core_fuses_it() {
    use scan_core::fuse::recording::{self, Record};
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("vugfacedeviationprb");
    std::fs::create_dir_all(&dir).expect("the fixture dir");
    let b = fixture::bench_intrinsics();
    let k4 = [b.fx, b.fy, b.ppx, b.ppy];
    let intr = scan_core::Intrinsics::from_rect_params(b.width, b.height, k4); // what the record reads back
    let scale = DepthScale::from_micrometres(1000); // what the record's units read back
    let pose = fixture::orbit(CENTRE, 600.0, 40.0 * DEG, 0.0);
    let frame = fixture::render_depth(&Scene::bench(), &intr, scale, &pose, &Noise::D435I, 0);
    let prb = dir.join("scan.prb");
    let rec = Record::new(frame.clone(), "replay:d435i", 1000, Some(k4)).expect("the record");
    std::fs::write(&prb, recording::write(&[rec]).expect("the recording")).expect("the .prb is written");

    let inv = pose.invert().expect("a rigid pose");
    let v3 = |p: Vec3| format!("{} {} {}", p.x, p.y, p.z);
    let (o, z, x) = (inv.apply_point(Vec3::ZERO), inv.apply_vector(Vec3::new(0.0, 0.0, 1.0)), inv.apply_vector(Vec3::new(1.0, 0.0, 0.0)));
    let text = DOC
        .replace("o=[0 0 0] z=[0 0 1] x=[1 0 0]", &format!("o=[{}] z=[{}] x=[{}]", v3(o), v3(z), v3(x)))
        .replace("dir=[0 0 1]", &format!("dir=[{}]", v3(z)));
    let part = dir.join("part.vugdoc");
    std::fs::write(&part, &text).expect("the part is written");

    let mut p = Viewer::open_against(prb.to_str().unwrap(), part.to_str().unwrap(), SIZE, Preset::Una).expect("the .prb fuses and measures");
    let mut t = Tsdf::new(4.0, 12.0, Tol::DEFAULT).expect("volume");
    t.integrate(&frame, scale, &intr, &Transform::IDENTITY).expect("the frame integrates");
    let direct = extract(&t).expect("surface");
    let doc = doc_core::read(&text).expect("the moved document reads");
    let mut d = Viewer::open_deviation(direct.clone(), &doc, "direct", SIZE, Preset::Una, None).expect("the design builds");

    let map = p.view.deviation.as_ref().expect("a map");
    assert!(map.result.is_ok(), "the frame lies on the moved design: {:?}", map.result.as_ref().err());
    println!("prb: verts={} direct={}", map.basis.0.mesh.positions.len(), direct.mesh.positions.len());
    assert_eq!(map.basis.0.mesh.positions.len(), direct.mesh.positions.len(), "the vertex count");
    assert!(map.basis.0 == ScanMesh { camera: Some(Vec3::ZERO), ..direct.clone() }, "the map's basis is scan_core's own fuse of the frame"); // VUGSCANCOARSEVIEWORIGIN (SR643): the posed road carries frame 0's camera

    let face = map
        .gdt
        .iter()
        .find(|f| map.names[f.face as usize].starts_with("body/") && matches!(f.nominal, scan_core::Nominal::Plane(_)) && f.flatness_mm.is_some())
        .map(|f| map.names[f.face as usize].clone())
        .expect("a measured body plane");
    let cmd = format!("gdt {face}=flatness:100");
    assert_eq!(vug_view::deviation::command(&mut p.view, &p.model, &cmd), Ok(true));
    assert_eq!(vug_view::deviation::command(&mut d.view, &d.model, &cmd), Ok(true));
    let (lp, ld) = (corner(p.view.deviation.as_ref().unwrap()), corner(d.view.deviation.as_ref().unwrap()));
    println!("{}", lp.join("\n"));
    assert_eq!(lp.last(), ld.last(), "the first callout reads the same");
    assert!(lp.last().is_some_and(|l| l.starts_with(face.as_str()) && l.ends_with("PASS")), "{lp:?}");
}

// ---- VUGSCANDEVIATIONCOARSE (LEDGER SR615): a scan with no starting pose opens through the coarse road ----

/// The known pose the scan is carried by: a rigid transform with a rotation far outside the fine basin
/// (scan_core `fixture::orbit`'s camera frame, off a centre the part is not on).
fn known_pose() -> Transform {
    fixture::orbit(Vec3::new(100.0, -50.0, 30.0), 400.0, 25.0 * DEG, 70.0 * DEG)
}

/// `s` with only the vertices `keep` holds (normals and confidence carried, the triangles whose three vertices are kept
/// remapped), each carried by `pose`.
fn fragment(s: &ScanMesh, keep: impl Fn(Vec3) -> bool, pose: Transform) -> ScanMesh {
    let mut out = s.clone();
    let mut index = vec![u32::MAX; s.mesh.positions.len()];
    out.mesh.positions.clear();
    out.normals.clear();
    out.confidence.clear();
    for (i, &p) in s.mesh.positions.iter().enumerate() {
        if keep(p) {
            index[i] = out.mesh.positions.len() as u32;
            out.mesh.positions.push(pose.apply_point(p));
            out.normals.push(pose.apply_vector(s.normals.get(i).copied().unwrap_or(Vec3::ZERO)));
            out.confidence.push(s.confidence.get(i).copied().unwrap_or(1.0));
        }
    }
    out.mesh.triangles = s
        .mesh
        .triangles
        .iter()
        .filter_map(|t| {
            let m = [index[t[0] as usize], index[t[1] as usize], index[t[2] as usize]];
            if m.contains(&u32::MAX) { None } else { Some(m) }
        })
        .collect();
    out
}

#[test]
fn a_rotated_orbit_scan_opens_through_coarse_and_fine() {
    let doc = doc_core::read(DOC).expect("the document reads");
    let p = known_pose();
    let moved = fragment(scan(), |_| true, p);
    let (_, fine) = vug_view::deviation::open(&moved, &doc, BAND_MM, vug_view::deviation::GDT_MARGIN_MM).expect("the design builds");
    assert!(matches!(fine.result, Err(DeviationError::NoAlignment { .. })), "the identity road cannot reach it: {:?}", fine.result.as_ref().err());
    let (_, map) = vug_view::deviation::open_coarse(&moved, &doc, BAND_MM, vug_view::deviation::GDT_MARGIN_MM).expect("the design builds");
    let lines = corner(&map);
    println!("{}", lines.join("\n"));
    let trimmed = match map.road {
        scan_core::deviation::Road::CoarseFine { rms_mm } => rms_mm,
        other => panic!("expected the coarse start kept, got {other:?} ({:?})", map.result.as_ref().err()),
    };
    let d = map.result.as_ref().expect("coarse + fine aligns the rotated scan");
    let (_, home) = vug_view::deviation::open(scan(), &doc, BAND_MM, vug_view::deviation::GDT_MARGIN_MM).expect("the design builds");
    let h = home.result.as_ref().expect("the scan aligns from the identity");
    // The recovered pose carried back through P is the identity road's pose.
    let back = d.aligned.pose.compose(p);
    let err_deg = 2.0 * back.rot.dot(h.aligned.pose.rot).abs().min(1.0).acos() / DEG;
    let err_mm = [Vec3::new(-230.0, -100.0, 0.0), Vec3::new(70.0, 100.0, 100.0), Vec3::new(160.0, 0.0, 150.0)]
        .iter()
        .map(|&x| (back.apply_point(x) - h.aligned.pose.apply_point(x)).mag())
        .fold(0.0, f64::max);
    println!("coarse: rotated trimmed={trimmed:.3} rms={:.3} stats_rms={:.3} err_deg={err_deg:.4} err_mm={err_mm:.4}", d.aligned.rms_mm, d.stats.rms_mm);
    assert!(d.aligned.rms_mm < 1.0 && d.stats.rms_mm < 1.0, "the SR510 floor: aligned {} stats {}", d.aligned.rms_mm, d.stats.rms_mm);
    assert!(err_deg < 0.5 && err_mm < 1.0, "pose error {err_deg} deg {err_mm} mm");
    assert!(lines.contains(&format!("align=coarse+fine rms={trimmed:.3}")), "{lines:?}");
    assert!(map.on, "the map opens on");
}

#[test]
fn a_half_scan_fragment_refuses_coarse() {
    let doc = doc_core::read(DOC).expect("the document reads");
    let half = fragment(scan(), |v| v.y > 0.0, known_pose());
    let (_, map) = vug_view::deviation::open_coarse(&half, &doc, BAND_MM, vug_view::deviation::GDT_MARGIN_MM).expect("the design builds");
    let lines = corner(&map);
    println!("{}", lines.join("\n"));
    let rms = match map.road {
        scan_core::deviation::Road::CoarseRefused { rms_mm } => rms_mm,
        other => panic!("expected the coarse start refused, got {other:?}"),
    };
    println!("coarse: fragment refused rms={rms:.3}");
    assert!(rms > scan_core::AlignParams::DEFAULT.gate(2), "the floor: {rms}");
    assert!(matches!(map.result, Err(DeviationError::NoAlignment { pairs: 0, .. })), "{:?}", map.result.as_ref().err());
    assert!(!map.on && map.vertex.is_empty());
    assert!(lines[0].starts_with("refused NoAlignment"), "{lines:?}");
    assert!(lines.contains(&format!("align=coarse refused rms={rms:.3}")), "{lines:?}");
}

// ---- VUGFACEDEVIATIONPRBPOSES (LEDGER SR621): a tracked .prb opens on the coarse road ----
// Twelve D435i frames of the bench from the orbit's azimuths 0°..11° (1° a frame: the tracker's regime and span, tools/scan
// tests/scan.rs `a_tracked_prb_recording_fuses_without_poses`) written as a .prb through probe_core's writer: fuse_prb
// tracks them, so the mesh's world is frame 0's camera, and the face opens it through `open_road`'s coarse road against
// the UNMOVED document (the world frame). SR621 MEASURED the near-single-view scan's four principal-axis starts above the
// floor (four frames over 3° 18.668, twelve over 11° 19.144, gate(2) 9); VUGSCANDEVIATIONCOARSEVIEW (SR626) turned the
// refusal into the acceptance: the scan is a view of frame 0's camera, so scan_core `coarse_view`'s yaw sweep with its
// translation re-solved starts it and it opens `align=coarse+fine` at frame 0's true pose. The refusal witness of the
// coarse floor is SR615's `a_half_scan_fragment_refuses_coarse` (not a view, its road unchanged). A one-frame .prb stays
// on the identity road (no `align=` line).

/// VUGSCANDEVIATIONCOARSEVIEW (SR626): `frames` D435i frames of the bench at the orbit's azimuths 0°, 1°, .. written as
/// `<name>/track.prb` beside the document `<name>/part.vug` (and frame 0 alone as `one.prb`): the paths and frame 0's
/// true camera → world pose.
fn tracked_prb(name: &str, frames: u32) -> (String, String, String, Transform) {
    use scan_core::fuse::recording::{self, Record};
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(&dir).expect("the fixture dir");
    let b = fixture::bench_intrinsics();
    let k4 = [b.fx, b.fy, b.ppx, b.ppy];
    let intr = scan_core::Intrinsics::from_rect_params(b.width, b.height, k4); // what the record reads back
    let scale = DepthScale::from_micrometres(1000); // what the record's units read back
    let truth = |k: u32| fixture::orbit(CENTRE, 600.0, 40.0 * DEG, k as f64 * DEG);
    let rec = |k: u32| {
        let f = fixture::render_depth(&Scene::bench(), &intr, scale, &truth(k), &Noise::D435I, k as u64);
        Record::new(f, "replay:d435i", 1000, Some(k4)).expect("the record")
    };
    let (track, one, part) = (dir.join("track.prb"), dir.join("one.prb"), dir.join("part.vug"));
    std::fs::write(&track, recording::write(&(0..frames).map(&rec).collect::<Vec<_>>()).expect("the recording")).expect("the .prb is written");
    std::fs::write(&one, recording::write(&[rec(0)]).expect("the recording")).expect("the .prb is written");
    std::fs::write(&part, DOC).expect("the part is written");
    let s = |p: std::path::PathBuf| p.to_str().unwrap().to_string();
    (s(track), s(one), s(part), truth(0))
}

/// VUGSCANDEVIATIONCOARSEVIEW (SR626): the tracked .prb at `track` opened against `part` on the view road: SR615's four
/// starts above the floor (printed), the road kept under gate(2), the corner's `align=coarse+fine`, the fine alignment
/// under SR615's 1.0 mm floor and its pose frame 0's camera `truth0`. The corner lines.
fn opens_on_the_view_road(tag: &str, track: &str, part: &str, truth0: Transform) -> Vec<String> {
    let p = Viewer::open_against(track, part, SIZE, Preset::Una).expect("the .prb fuses and measures");
    let map = p.view.deviation.as_ref().expect("a map");
    let lp = corner(map);
    println!("{}", lp.join("\n"));
    let ap = scan_core::AlignParams::DEFAULT;
    let (s, design) = &*map.basis;
    let bvh = mesh_core::bvh::Bvh::build(&design.mesh);
    let four: Vec<f64> = scan_core::deviation::coarse(s, design, &bvh, &ap, Tol::DEFAULT).expect("four starts").iter().map(|x| x.1).collect();
    println!("{tag}: four-start rms={four:.3?} gate2={:.3}", ap.gate(2));
    assert!(four.iter().all(|r| *r > ap.gate(2)), "SR615's four starts refuse a near-single view: {four:?}");
    let rms = match map.road {
        scan_core::deviation::Road::CoarseFine { rms_mm } => rms_mm,
        other => panic!("expected the view road kept, got {other:?} ({:?})", map.result.as_ref().err()),
    };
    let d = map.result.as_ref().expect("the view road aligns the tracked scan");
    let back = truth0.invert().expect("a rigid pose");
    let err_deg = 2.0 * d.aligned.pose.rot.dot(truth0.rot).abs().min(1.0).acos() / DEG;
    let err_mm = [Vec3::new(-230.0, -100.0, 0.0), Vec3::new(70.0, 100.0, 100.0), Vec3::new(160.0, 0.0, 150.0)]
        .iter()
        .map(|&x| (d.aligned.pose.apply_point(back.apply_point(x)) - x).mag())
        .fold(0.0, f64::max);
    println!("{tag}: road=CoarseFine rms={rms:.3} aligned={:.3} stats_rms={:.3} err_deg={err_deg:.4} err_mm={err_mm:.4}", d.aligned.rms_mm, d.stats.rms_mm);
    assert!(rms <= ap.gate(2), "the floor: {rms}");
    assert!(d.aligned.rms_mm < 1.0 && d.stats.rms_mm < 1.0, "the SR510 floor: aligned {} stats {}", d.aligned.rms_mm, d.stats.rms_mm);
    assert!(err_deg < 1.0 && err_mm < 2.0, "pose error against frame 0's camera {err_deg} deg {err_mm} mm");
    assert!(lp.contains(&format!("align=coarse+fine rms={rms:.3}")), "{lp:?}");
    assert!(map.on, "the map opens on");
    lp
}

#[test]
fn a_tracked_prb_opens_on_the_coarse_road() {
    let (track, one, part, truth0) = tracked_prb("vugfacedeviationprbposes", 12);
    let (track, one, part) = (track.as_str(), one.as_str(), part.as_str());

    // The tracked .prb: the coarse road, SR615's starts refused, the view road's start kept and aligned.
    let lp = opens_on_the_view_road("prbposes", track, part, truth0);

    // The part opened as the journal with the tracked .prb: the same road, the same corner but the source line.
    let j = Viewer::open_scan(part, track, SIZE, Preset::Una).expect("the part opens with the tracked scan on the coarse road");
    let jm = j.view.deviation.as_ref().expect("a journal map");
    assert!(matches!(jm.road, scan_core::deviation::Road::CoarseFine { .. }), "{:?}", jm.road);
    let lj = corner(jm);
    assert_eq!(lj[..lj.len() - 1], lp[..], "the journal measures the tracked scan as the path road does");

    // The one-frame .prb: the identity road, no align line (against the world-frame document it reads NoAlignment).
    let o = Viewer::open_against(one, part, SIZE, Preset::Una).expect("the one-frame .prb fuses and measures");
    let om = o.view.deviation.as_ref().expect("a map");
    assert_eq!(om.road, scan_core::deviation::Road::Fine);
    let lo = corner(om);
    assert!(!lo.iter().any(|l| l.starts_with("align=")), "{lo:?}");
    assert!(matches!(om.result, Err(DeviationError::NoAlignment { .. })), "{:?}", om.result.as_ref().err());
    println!("prbposes: one-frame road=Fine");
}

// ---- VUGSCANDEVIATIONCOARSEVIEW (LEDGER SR626): the near-single-view start ----

#[test]
fn a_four_frame_tracked_prb_opens_on_the_view_road() {
    let (track, _, part, truth0) = tracked_prb("vugscandeviationcoarseview", 4);
    opens_on_the_view_road("coarseview", &track, &part, truth0);
}

// ---- VUGSCANROADWORDMARGIN (LEDGER SR637): the road's one word; a fused scan's margin is its volume's truncation ----
// The corner's `align=` and tools/scan's `road: ` lines spell scan_core's `Road::word` (the SR615 / SR626 / SR627 tests
// read the same bytes). A .prb opened by path (`open_against`) or as the journal's scan (`open_scan`) carries the fused
// volume's own `Tsdf::trunc_mm()` (scan_core `fuse::fuse_prb_volume`) as the map's gdt margin; no knob sets it today
// (VOXEL_MM x TRUNC_VOXELS = 12), so it reads EQUAL to the `GDT_MARGIN_MM` fallback, said so.

#[test]
fn a_fused_scan_opens_at_its_volumes_truncation() {
    use scan_core::deviation::Road;
    assert_eq!(Road::Fine.word(), None);
    assert_eq!(Road::CoarseFine { rms_mm: 1.25 }.word().as_deref(), Some("coarse+fine rms=1.250"));
    assert_eq!(Road::CoarseRefused { rms_mm: 9.5 }.word().as_deref(), Some("coarse refused rms=9.500"));

    let (_, one, part, _) = tracked_prb("vugscanroadwordmargin", 1);
    let bytes = std::fs::read(&one).expect("the .prb reads");
    let (_, tracked, trunc) = scan_core::fuse::fuse_prb_volume(&bytes, scan_core::tsdf::Knobs::OFF).expect("the .prb fuses");
    assert!(!tracked, "one frame fuses at the identity");
    let p = Viewer::open_against(&one, &part, SIZE, Preset::Una).expect("the .prb fuses and measures");
    let j = Viewer::open_scan(&part, &one, SIZE, Preset::Una).expect("the part opens with the .prb");
    let (pm, jm) = (p.view.deviation.as_ref().expect("a map"), j.view.deviation.as_ref().expect("a journal map"));
    println!("margin: fused trunc={trunc:.3} fallback={:.3} path={:.3} journal={:.3} equal today", vug_view::deviation::GDT_MARGIN_MM, pm.margin_mm, jm.margin_mm);
    assert_eq!(pm.margin_mm, trunc, "the path map's margin is the fused volume's truncation");
    assert_eq!(jm.margin_mm, trunc, "the journal map's margin is the fused volume's truncation");
    assert_eq!(trunc, vug_view::deviation::GDT_MARGIN_MM, "equal to the fallback today: no knob sets the face's truncation");
}

// ---- VUGSCANDEVIATIONCOARSESYM (LEDGER SR636): a design with near-equal principal moments starts from its pivot ----
// The bench's box made near-square (150 x 156 x 100) with a post r20 h60 on its top at (35, 40), on a 480 x 400 ground
// (the part ~35% of the scan, SR615's bench 36%): the design's middle and largest moments pair (0.671 / 0.880 against
// scan_core's COARSE_SYM_RATIO 0.83), so its in-plane axes are the post's, not the box's, and SR615's four signs land far
// from the truth (modelled 177 / 165 / 119 / 63°, its road accepting the 180° turn). scan_core `sym_or` hands
// `measure_coarse` SR626's sweep about the pivot (the smallest axis) and the scan carried by a known pose opens
// `align=coarse+fine` at the identity road's pose.

/// The near-square box and its post, built to the scene's dimensions (the post's sketch on the box's top).
const SYM_DOC: &str = r#"vug-doc version=1 lin=1e-6 ang=1e-9
sketch s1 o=[0 0 0] z=[0 0 1] x=[1 0 0]
point p1 sketch=s1 x=-75 y=-78
point p2 sketch=s1 x=75 y=-78
point p3 sketch=s1 x=75 y=78
point p4 sketch=s1 x=-75 y=78
line l1 sketch=s1 a=p1 b=p2
line l2 sketch=s1 a=p2 b=p3
line l3 sketch=s1 a=p3 b=p4
line l4 sketch=s1 a=p4 b=p1
fix f1 sketch=s1 p=p1 at=[-75 -78]
fix f2 sketch=s1 p=p2 at=[75 -78]
fix f3 sketch=s1 p=p3 at=[75 78]
fix f4 sketch=s1 p=p4 at=[-75 78]
sketch s2 o=[0 0 100] z=[0 0 1] x=[1 0 0]
point c0 sketch=s2 x=35 y=40
circle c1 sketch=s2 centre=c0 r=20
fix g1 sketch=s2 p=c0 at=[35 40]
radius r1 sketch=s2 circle=c1 r=20
extrude body sketch=s1 outer=l1 dir=[0 0 1] length=100
extrude post sketch=s2 outer=c1 dir=[0 0 1] length=60
"#;

/// The near-square box's orbit scan (12 frames at 600 mm, 40° up, D435i noise), fused once in the design's world.
fn sym_scan() -> &'static ScanMesh {
    static S: OnceLock<ScanMesh> = OnceLock::new();
    S.get_or_init(|| {
        let scene = Scene {
            ground_centre: Vec3::ZERO,
            ground: [240.0, 200.0],
            boxes: vec![fixture::BoxPrim { min: Vec3::new(-75.0, -78.0, 0.0), max: Vec3::new(75.0, 78.0, 100.0) }],
            cylinders: vec![fixture::CylPrim { base: Vec3::new(35.0, 40.0, 100.0), radius: 20.0, height: 60.0 }],
        };
        let (intr, scale) = (fixture::bench_intrinsics(), DepthScale::D400_DEFAULT);
        let mut t = Tsdf::new(4.0, 12.0, Tol::DEFAULT).expect("volume");
        for k in 0..12u32 {
            let p = fixture::orbit(Vec3::new(0.0, 0.0, 50.0), 600.0, 40.0 * DEG, k as f64 * 2.0 * PI / 12.0);
            let f = fixture::render_depth(&scene, &intr, scale, &p, &Noise::D435I, k as u64);
            t.integrate(&f, scale, &intr, &p).expect("frame integrates");
        }
        extract(&t).expect("surface")
    })
}

#[test]
fn a_near_square_box_opens_on_the_sym_road() {
    let doc = doc_core::read(SYM_DOC).expect("the document reads");
    // SR615's known pose with a 50° turn about the box's z (the free yaw of its near-equal pair) first.
    let (s, c) = (50.0 * DEG).sin_cos();
    let yaw = fixture::from_axes(Vec3::new(c, s, 0.0), Vec3::new(-s, c, 0.0), Vec3::new(0.0, 0.0, 1.0), Vec3::ZERO);
    let p = known_pose().compose(yaw);
    let moved = fragment(sym_scan(), |_| true, p);
    let truth = p.invert().expect("a rigid pose");

    // Before: SR615's four sign starts do not span the truth.
    let (_, home) = vug_view::deviation::open(sym_scan(), &doc, BAND_MM, vug_view::deviation::GDT_MARGIN_MM).expect("the design builds");
    let h = home.result.as_ref().expect("the scan aligns from the identity");
    let (_, design) = &*home.basis;
    let ap = scan_core::AlignParams::DEFAULT;
    let bvh = mesh_core::bvh::Bvh::build(&design.mesh);
    let four: Vec<f64> = scan_core::deviation::coarse(&moved, design, &bvh, &ap, Tol::DEFAULT)
        .expect("four starts")
        .iter()
        .map(|x| 2.0 * x.0.rot.dot(truth.rot).abs().min(1.0).acos() / DEG)
        .collect();
    println!("coarsesym: four-start err_deg={four:.1?}");
    assert!(four.iter().all(|e| *e > 20.0), "SR615's four signs miss the near-square box's truth: {four:?}");

    // After: the pivot sweep starts it and the fine alignment lands on the identity road's pose.
    let (_, map) = vug_view::deviation::open_coarse(&moved, &doc, BAND_MM, vug_view::deviation::GDT_MARGIN_MM).expect("the design builds");
    let lines = corner(&map);
    println!("{}", lines.join("\n"));
    let rms = match map.road {
        scan_core::deviation::Road::CoarseFine { rms_mm } => rms_mm,
        other => panic!("expected the sym road kept, got {other:?} ({:?})", map.result.as_ref().err()),
    };
    let d = map.result.as_ref().expect("the sym road aligns the near-square box");
    let back = d.aligned.pose.compose(p);
    let err_deg = 2.0 * back.rot.dot(h.aligned.pose.rot).abs().min(1.0).acos() / DEG;
    let err_mm = [Vec3::new(-75.0, -78.0, 0.0), Vec3::new(75.0, 78.0, 100.0), Vec3::new(35.0, 40.0, 160.0)]
        .iter()
        .map(|&x| (back.apply_point(x) - h.aligned.pose.apply_point(x)).mag())
        .fold(0.0, f64::max);
    println!("coarsesym: road=CoarseFine rms={rms:.3} aligned={:.3} stats_rms={:.3} err_deg={err_deg:.4} err_mm={err_mm:.4}", d.aligned.rms_mm, d.stats.rms_mm);
    assert!(rms <= ap.gate(2), "the floor: {rms}");
    assert!(d.aligned.rms_mm < 1.0 && d.stats.rms_mm < 1.0, "the SR510 floor: aligned {} stats {}", d.aligned.rms_mm, d.stats.rms_mm);
    assert!(err_deg < 1.0 && err_mm < 2.0, "pose error {err_deg} deg {err_mm} mm");
    assert!(lines.contains(&format!("align=coarse+fine rms={rms:.3}")), "{lines:?}");
    assert!(map.on, "the map opens on");
}

// ---- VUGSCANCOARSESYMPIVOT (LEDGER SR642): the scan's own unique axis takes the design's pivot ----
// The bench's post r40 h150 with four pins r10 h40 at radius 75 (C4: the design's lateral moments equal, so its pivot
// is the LARGEST axis, the post's; a bare cylinder's spin is unconstrained in `align`) on a 360 x 310 ground (modelled
// 66% of the scan): the scan's unique axis is its SMALLEST, the ground normal, so SR636's same-index pairing laid the
// post on the ground's long axis (modelled every kept start 90° off, the road refused). scan_core `partner` picks the
// scan's own unique end and the sweep about the post's axis opens `align=coarse+fine` at the identity road's pose,
// compared modulo the pins' four 90° turns.

/// The post and its four pins, one sketch each, built to the scene's dimensions.
const PIVOT_DOC: &str = r#"vug-doc version=1 lin=1e-6 ang=1e-9
sketch s1 o=[0 0 0] z=[0 0 1] x=[1 0 0]
point c1 sketch=s1 x=0 y=0
circle k1 sketch=s1 centre=c1 r=40
fix g1 sketch=s1 p=c1 at=[0 0]
radius r1 sketch=s1 circle=k1 r=40
extrude post sketch=s1 outer=k1 dir=[0 0 1] length=150
sketch s2 o=[0 0 0] z=[0 0 1] x=[1 0 0]
point c2 sketch=s2 x=75 y=0
circle k2 sketch=s2 centre=c2 r=10
fix g2 sketch=s2 p=c2 at=[75 0]
radius r2 sketch=s2 circle=k2 r=10
extrude pin1 sketch=s2 outer=k2 dir=[0 0 1] length=40
sketch s3 o=[0 0 0] z=[0 0 1] x=[1 0 0]
point c3 sketch=s3 x=0 y=75
circle k3 sketch=s3 centre=c3 r=10
fix g3 sketch=s3 p=c3 at=[0 75]
radius r3 sketch=s3 circle=k3 r=10
extrude pin2 sketch=s3 outer=k3 dir=[0 0 1] length=40
sketch s4 o=[0 0 0] z=[0 0 1] x=[1 0 0]
point c4 sketch=s4 x=-75 y=0
circle k4 sketch=s4 centre=c4 r=10
fix g4 sketch=s4 p=c4 at=[-75 0]
radius r4 sketch=s4 circle=k4 r=10
extrude pin3 sketch=s4 outer=k4 dir=[0 0 1] length=40
sketch s5 o=[0 0 0] z=[0 0 1] x=[1 0 0]
point c5 sketch=s5 x=0 y=-75
circle k5 sketch=s5 centre=c5 r=10
fix g5 sketch=s5 p=c5 at=[0 -75]
radius r5 sketch=s5 circle=k5 r=10
extrude pin4 sketch=s5 outer=k5 dir=[0 0 1] length=40
"#;

/// The post's orbit scan (12 frames at 600 mm, 40° up, D435i noise), fused once in the design's world.
fn pivot_scan() -> &'static ScanMesh {
    static S: OnceLock<ScanMesh> = OnceLock::new();
    S.get_or_init(|| {
        let cyl = |x: f64, y: f64, radius: f64, height: f64| fixture::CylPrim { base: Vec3::new(x, y, 0.0), radius, height };
        let scene = Scene {
            ground_centre: Vec3::ZERO,
            ground: [180.0, 155.0],
            boxes: Vec::new(),
            cylinders: vec![cyl(0.0, 0.0, 40.0, 150.0), cyl(75.0, 0.0, 10.0, 40.0), cyl(0.0, 75.0, 10.0, 40.0), cyl(-75.0, 0.0, 10.0, 40.0), cyl(0.0, -75.0, 10.0, 40.0)],
        };
        let (intr, scale) = (fixture::bench_intrinsics(), DepthScale::D400_DEFAULT);
        let mut t = Tsdf::new(4.0, 12.0, Tol::DEFAULT).expect("volume");
        for k in 0..12u32 {
            let p = fixture::orbit(Vec3::new(0.0, 0.0, 75.0), 600.0, 40.0 * DEG, k as f64 * 2.0 * PI / 12.0);
            let f = fixture::render_depth(&scene, &intr, scale, &p, &Noise::D435I, k as u64);
            t.integrate(&f, scale, &intr, &p).expect("frame integrates");
        }
        extract(&t).expect("surface")
    })
}

#[test]
fn a_post_on_its_ground_pivots_on_the_scan_s_own_axis() {
    let doc = doc_core::read(PIVOT_DOC).expect("the document reads");
    let p = known_pose();
    let moved = fragment(pivot_scan(), |_| true, p);
    let up = Vec3::new(0.0, 0.0, 1.0);
    let axis_deg = |t: Transform, u: Vec3| t.apply_vector(up).dot(u).clamp(-1.0, 1.0).acos() / DEG;

    // The partner: the sweep about the post's axis (the design's largest, index 0) puts it on the scan's ground normal.
    let (_, home) = vug_view::deviation::open(pivot_scan(), &doc, BAND_MM, vug_view::deviation::GDT_MARGIN_MM).expect("the design builds");
    let h = home.result.as_ref().expect("the scan aligns from the identity");
    let (_, design) = &*home.basis;
    let ap = scan_core::AlignParams::DEFAULT;
    let bvh = mesh_core::bvh::Bvh::build(&design.mesh);
    let starts = scan_core::deviation::coarse_view(&moved, design, &bvh, &ap, Tol::DEFAULT, scan_core::deviation::Sweep::About(0)).expect("the pivot sweep");
    let first = axis_deg(starts[0].0.compose(p), up);
    println!("coarsesympivot: sweep-start axis_deg={first:.2} score={:.3}", starts[0].1);
    assert!(first < 15.0, "the sweep's best start carries the post's axis onto the ground normal: {first}");

    // The road: coarse + fine lands on the identity road's pose, modulo the pins' four 90° turns about the post.
    let (_, map) = vug_view::deviation::open_coarse(&moved, &doc, BAND_MM, vug_view::deviation::GDT_MARGIN_MM).expect("the design builds");
    let lines = corner(&map);
    println!("{}", lines.join("\n"));
    let rms = match map.road {
        scan_core::deviation::Road::CoarseFine { rms_mm } => rms_mm,
        other => panic!("expected the sym road kept, got {other:?} ({:?})", map.result.as_ref().err()),
    };
    let d = map.result.as_ref().expect("the sym road aligns the post");
    let back = d.aligned.pose.compose(p);
    let axis = axis_deg(back, h.aligned.pose.apply_vector(up));
    let (err_deg, err_mm) = (0..4)
        .map(|k| {
            let (s, c) = (k as f64 * PI / 2.0).sin_cos();
            let turn = fixture::from_axes(Vec3::new(c, s, 0.0), Vec3::new(-s, c, 0.0), up, Vec3::ZERO).compose(h.aligned.pose);
            let deg = 2.0 * back.rot.dot(turn.rot).abs().min(1.0).acos() / DEG;
            let mm = [Vec3::ZERO, Vec3::new(0.0, 0.0, 150.0), Vec3::new(75.0, 0.0, 40.0)]
                .iter()
                .map(|&x| (back.apply_point(x) - turn.apply_point(x)).mag())
                .fold(0.0, f64::max);
            (deg, mm)
        })
        .fold((f64::INFINITY, f64::INFINITY), |a, b| if b.0 < a.0 { b } else { a });
    println!("coarsesympivot: road=CoarseFine rms={rms:.3} aligned={:.3} stats_rms={:.3} axis_deg={axis:.4} err_deg={err_deg:.4} err_mm={err_mm:.4}", d.aligned.rms_mm, d.stats.rms_mm);
    assert!(rms <= ap.gate(2), "the floor: {rms}");
    assert!(d.aligned.rms_mm < 1.0 && d.stats.rms_mm < 1.0, "the SR510 floor: aligned {} stats {}", d.aligned.rms_mm, d.stats.rms_mm);
    assert!(axis < 1.0 && err_deg < 1.0 && err_mm < 2.0, "pose error: axis {axis} deg, {err_deg} deg {err_mm} mm modulo C4");
    assert!(lines.contains(&format!("align=coarse+fine rms={rms:.3}")), "{lines:?}");
    assert!(map.on, "the map opens on");
}

// ---- VUGSCANCOARSEVIEWORIGIN (LEDGER SR643): the view road's front test reads the carried camera ----
// A tracked .prb's mesh carries frame 0's camera (scan_core `fuse::camera`, read from the loop's poses: [0,0,0] in the
// tracked world) and keeps SR626's view road; the same mesh with its camera removed is refused that road (SR615's four
// starts, their refusal), and a mesh file (`scan_mesh`) carries none.

#[test]
fn a_scan_without_a_camera_is_refused_the_view_road() {
    use scan_core::deviation::{Road, coarse, coarse_view, measure_coarse};
    let (track, _, part, _) = tracked_prb("vugscancoarsevieworigin", 4);
    let p = Viewer::open_against(&track, &part, SIZE, Preset::Una).expect("the .prb fuses and measures");
    let map = p.view.deviation.as_ref().expect("a map");
    let (s, design) = &*map.basis;
    assert_eq!(s.camera, Some(Vec3::ZERO), "the tracked world's frame-0 camera, read from the fuse's poses");
    let kept = match map.road {
        Road::CoarseFine { rms_mm } => rms_mm,
        other => panic!("expected the view road kept with the camera carried, got {other:?}"),
    };

    let ap = scan_core::AlignParams::DEFAULT;
    let bvh = mesh_core::bvh::Bvh::build(&design.mesh);
    let none = ScanMesh { camera: None, ..s.clone() };
    assert!(coarse_view(&none, design, &bvh, &ap, Tol::DEFAULT).is_none(), "no camera, no view road");
    let least = coarse(&none, design, &bvh, &ap, Tol::DEFAULT).expect("four starts").iter().map(|x| x.1).fold(f64::INFINITY, f64::min);
    let (road, r) = measure_coarse(&none, design, &ap, BAND_MM, Tol::DEFAULT);
    println!("vieworigin: camera={:?} road=CoarseFine rms={kept:.3} | camera=None view=None road={road:?} least-four={least:.3}", s.camera);
    assert_eq!(road, Road::CoarseRefused { rms_mm: least }, "SR615's road alone: its four starts' refusal");
    assert!(matches!(r, Err(DeviationError::NoAlignment { .. })), "{:?}", r.as_ref().err());

    assert_eq!(vug_view::deviation::scan_mesh(s.mesh.clone()).camera, None, "a mesh file carries no camera");
}
