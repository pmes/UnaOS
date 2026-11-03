// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VUGFACETREE (LEDGER SR513): the feature tree's host witness — the SR485 rectangle drawn by events and dimensioned by
//! the tool is a doc_core document that reads back and replays to the face's dof; `u` / `U` step the snapshots and the
//! face regenerates; deleting the extrude row removes the body; editing the width row regenerates the body; a refused
//! edit keeps the geometry and names its line. VUGFACESAVE (SR534): a saved document reopens through `Viewer::open` to the
//! same text and volume; the title's `*` follows an edit and clears on save; a path that cannot be written is named.

use bandy::SMessage;
use doc_core::{Cons, Stmt};
use vug_view::Viewer;
use vug_view::model::Model;
use vug_view::sketch::SketchFace;
use vug_view::view::Preset;

const W: u32 = 400;
const H: u32 = 300;
/// The SR485 rectangle: 16 mm by 10 mm from the origin, counter-clockwise.
const RECT: [(f64, f64); 4] = [(0.0, 0.0), (16.0, 0.0), (16.0, 10.0), (0.0, 10.0)];

fn cube() -> Model {
    let s = 10.0;
    let p = vec![[-s, -s, -s], [s, -s, -s], [s, s, -s], [-s, s, -s], [-s, -s, s], [s, -s, s], [s, s, s], [-s, s, s]];
    let t = vec![[0, 2, 1], [0, 3, 2], [4, 5, 6], [4, 6, 7], [0, 1, 5], [0, 5, 4], [3, 7, 6], [3, 6, 2], [0, 4, 7], [0, 7, 3], [1, 2, 6], [1, 6, 5]];
    Model::from_triangles(&p, &t).expect("cube")
}

fn face(vw: &Viewer) -> &SketchFace {
    vw.view.design.face.as_ref().expect("a sketch is open")
}

fn screen(vw: &Viewer, q: (f64, f64)) -> (f64, f64) {
    face(vw).to_screen(&vw.view, sketch_core::Vec2::new(q.0, q.1)).expect("on screen")
}

fn button(vw: &mut Viewer, down: bool, p: (f64, f64)) -> bool {
    vw.input(&SMessage::BrowserButton { down, x: p.0, y: p.1, mods: 0, surface: 0 })
}

fn move_to(vw: &mut Viewer, p: (f64, f64)) -> bool {
    vw.input(&SMessage::BrowserMouseMove(p.0, p.1, 0))
}

fn key(vw: &mut Viewer, k: &str) -> bool {
    vw.input(&SMessage::BrowserKey(k.into()))
}

fn typed(vw: &mut Viewer, t: &str) -> bool {
    vw.input(&SMessage::BrowserText(t.into()))
}

fn stroke(vw: &mut Viewer, a: (f64, f64), b: (f64, f64)) {
    let (sa, sb) = (screen(vw, a), screen(vw, b));
    let mid = screen(vw, (0.5 * (a.0 + b.0), 0.5 * (a.1 + b.1)));
    button(vw, true, sa);
    assert!(move_to(vw, mid));
    assert!(move_to(vw, sb));
    assert!(button(vw, false, sb));
}

fn click(vw: &mut Viewer, q: (f64, f64)) {
    let p = screen(vw, q);
    assert!(button(vw, true, p));
    button(vw, false, p);
}

/// The SR485 rectangle by events (top view, the line tool), dimensioned by the tool: 16 on the bottom line, 10 on the
/// right one.
fn dimensioned() -> Viewer {
    let mut vw = Viewer::new(cube(), "tree", (W, H), Preset::Una);
    assert!(typed(&mut vw, "7"));
    assert!(typed(&mut vw, "l"));
    for i in 0..4 {
        stroke(&mut vw, RECT[i], RECT[(i + 1) % 4]);
    }
    assert!(typed(&mut vw, "d"));
    click(&mut vw, (8.0, 0.0));
    assert!(typed(&mut vw, "16"));
    assert!(key(&mut vw, "Return"));
    click(&mut vw, (16.0, 5.0));
    assert!(typed(&mut vw, "10"));
    assert!(key(&mut vw, "Return"));
    assert_eq!(vw.view.design.refusal, None);
    assert_eq!(face(&vw).dof, 0);
    vw
}

/// The dimensioned rectangle pulled 10 by a typed distance (the SR494 road): a 16 x 10 x 10 body.
fn extruded() -> Viewer {
    let mut vw = dimensioned();
    assert!(typed(&mut vw, "h"));
    assert!(typed(&mut vw, "e"));
    assert!(typed(&mut vw, "10"));
    assert!(key(&mut vw, "Return"));
    let vol = vw.view.design.body.as_ref().expect("a body").volume().expect("volume");
    assert!((vol - 1600.0).abs() < 1e-4, "volume {vol}");
    vw
}

fn volume(vw: &Viewer) -> f64 {
    vw.view.design.body.as_ref().expect("a body").volume().expect("volume")
}

fn doc(vw: &Viewer) -> &doc_core::Doc {
    &vw.view.design.journal.doc
}

fn index_of(vw: &Viewer, f: impl Fn(&Stmt) -> bool) -> usize {
    doc(vw).stmts.iter().position(f).expect("the row is in the document")
}

#[test]
fn the_rectangle_is_a_document_that_reads_and_replays_to_the_same_dof() {
    let vw = dimensioned();
    let d = doc(&vw);
    let count = |f: &dyn Fn(&Stmt) -> bool| d.stmts.iter().filter(|s| f(s)).count();
    assert_eq!(count(&|s| matches!(s, Stmt::Sketch { .. })), 1);
    // The origin and two points per stroke; four lines.
    assert_eq!(count(&|s| matches!(s, Stmt::Point { .. })), 9);
    assert_eq!(count(&|s| matches!(s, Stmt::Line { .. })), 4);
    let cons = |f: &dyn Fn(&Cons) -> bool| d.stmts.iter().filter(|s| matches!(s, Stmt::Constraint { c, .. } if f(c))).count();
    assert_eq!(cons(&|c| matches!(c, Cons::Fix(..))), 1);
    assert_eq!(cons(&|c| matches!(c, Cons::Coincident(..))), 5);
    assert_eq!(cons(&|c| matches!(c, Cons::Horizontal(..))), 2);
    assert_eq!(cons(&|c| matches!(c, Cons::Vertical(..))), 2);
    assert_eq!(cons(&|c| matches!(c, Cons::Distance(..))), 2);
    assert_eq!(cons(&|_| true), 12);
    // The text reads back to the same document, and replays clean to the face's dof.
    let back = doc_core::read(&doc_core::write(d)).expect("the written document reads");
    assert_eq!(&back, d);
    let rep = doc_core::replay(d, d.tol);
    assert!(rep.failures.is_empty(), "{:?}", rep.failures);
    assert_eq!(rep.sketches.len(), 1);
    assert_eq!(sketch_core::dof(&rep.sketches[0].sketch).count, face(&vw).dof);
    // The panel lists every statement.
    assert_eq!(vw.view.design.journal.rows().len(), d.stmts.len());
}

#[test]
fn undo_and_redo_step_the_snapshots_and_the_face_regenerates() {
    let mut vw = dimensioned();
    let full = doc(&vw).clone();
    let rows = vw.view.design.journal.rows().len();
    assert!(typed(&mut vw, "u"));
    assert_eq!(face(&vw).dof, 1, "one dimension undone");
    assert_eq!(vw.view.design.journal.rows().len(), rows - 1);
    assert!(typed(&mut vw, "u"));
    assert_eq!(face(&vw).dof, 2, "both dimensions undone");
    assert_eq!(vw.view.design.journal.rows().len(), rows - 2);
    assert!(typed(&mut vw, "U"));
    assert_eq!(face(&vw).dof, 1);
    assert!(typed(&mut vw, "U"));
    assert_eq!(face(&vw).dof, 0);
    assert_eq!(doc(&vw), &full);
    assert_eq!(face(&vw).dims.len(), 2, "the dimensions are drawn again");
}

#[test]
fn deleting_the_extrude_row_removes_the_body() {
    let mut vw = extruded();
    assert!(vw.view.design.face.is_none());
    assert!(typed(&mut vw, "t"));
    assert!(vw.view.design.journal.panel);
    let x = index_of(&vw, |s| matches!(s, Stmt::Extrude { .. }));
    while vw.view.design.journal.cursor < x {
        assert!(key(&mut vw, "Down"));
    }
    while vw.view.design.journal.cursor > x {
        assert!(key(&mut vw, "Up"));
    }
    assert!(key(&mut vw, "Delete"));
    assert!(vw.view.design.body.is_none(), "{:?}", vw.view.design.journal.note);
    assert!(!doc(&vw).stmts.iter().any(|s| matches!(s, Stmt::Extrude { .. })));
    assert_eq!(vw.view.design.journal.note.as_deref(), Some("deleted x0"));
    // The sketch it pulled is open again, still exact.
    assert_eq!(face(&vw).dof, 0);
    assert_eq!(face(&vw).profiles.len(), 1);
}

#[test]
fn editing_the_width_row_regenerates_the_body() {
    let mut vw = extruded();
    assert!(typed(&mut vw, "t"));
    let i = index_of(&vw, |s| matches!(s, Stmt::Constraint { c: Cons::Distance(_, _, v), .. } if *v == 16.0));
    vw.view.design.journal.cursor = i;
    assert!(key(&mut vw, "Return"));
    assert_eq!(vw.view.design.journal.edit.as_deref(), Some(""));
    assert!(typed(&mut vw, "20"));
    assert!(key(&mut vw, "Return"));
    assert_eq!(vw.view.design.journal.note, None);
    let vol = volume(&vw);
    assert!((vol - 2000.0).abs() <= 1e-6 * 2000.0, "20 x 10 x 10: volume {vol}");
}

#[test]
fn a_refused_edit_keeps_the_geometry_and_names_the_line() {
    let mut vw = extruded();
    let before = doc(&vw).clone();
    assert!(typed(&mut vw, "t"));
    let x = index_of(&vw, |s| matches!(s, Stmt::Extrude { .. }));
    vw.view.design.journal.cursor = x;
    assert!(key(&mut vw, "Return"));
    assert!(typed(&mut vw, "0"));
    assert!(key(&mut vw, "Return"));
    let note = vw.view.design.journal.note.clone().expect("refused");
    assert!(note.starts_with(&format!("line {}:", x + 2)), "the extrude's line named: {note}");
    assert_eq!(doc(&vw), &before);
    assert!((volume(&vw) - 1600.0).abs() < 1e-4);
}

// ---- VUGFACESAVE (SR534): the save road ----

/// A fresh path under cargo's per-test scratch dir (the private target), nothing left from an earlier run.
fn scratch(name: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    let _ = std::fs::remove_file(&p);
    p
}

#[test]
fn a_saved_document_reopens_to_the_same_text_and_volume() {
    let mut vw = extruded();
    let path = scratch("vugfacesave-reopen.vug");
    vw.save_to = Some(path.clone());
    assert!(typed(&mut vw, "s"));
    assert_eq!(vw.view.design.journal.disk, Some(Ok("saved vugfacesave-reopen.vug".to_string())));
    assert!(path.exists());
    assert!(!std::path::Path::new(&format!("{}.tmp", path.display())).exists(), "the temp file was renamed away");
    let back = Viewer::open(path.to_str().expect("utf-8 path"), (W, H), Preset::Una).expect("the saved document opens");
    assert_eq!(doc_core::write(doc(&back)), doc_core::write(doc(&vw)));
    assert!((volume(&back) - 1600.0).abs() < 1e-4, "volume {}", volume(&back));
    assert_eq!(back.title, "vugfacesave-reopen.vug", "a just-opened document is clean");
    assert!(!back.view.design.journal.dirty());
    // Opened from the file, the face writes back over it.
    let mut back = back;
    assert!(typed(&mut back, "s"));
    assert!(matches!(back.view.design.journal.disk, Some(Ok(_))), "{:?}", back.view.design.journal.disk);
}

#[test]
fn the_title_marks_an_edit_and_a_save_clears_it() {
    let mut vw = dimensioned();
    assert_eq!(vw.title, "tree*");
    assert!(vw.view.design.journal.dirty());
    vw.save_to = Some(scratch("vugfacesave-title.vug"));
    assert!(key(&mut vw, "Meta+s"));
    assert_eq!(vw.title, "tree", "{:?}", vw.view.design.journal.disk);
    assert!(typed(&mut vw, "u"));
    assert_eq!(vw.title, "tree*", "an undo differs from the saved text");
    assert!(typed(&mut vw, "U"));
    assert_eq!(vw.title, "tree", "redo is the saved text again");
}

#[test]
fn a_path_that_cannot_be_written_names_the_error() {
    let mut vw = dimensioned();
    let before = doc(&vw).clone();
    // A regular file where the directory should be: refused even for root (a chmod'ed read-only dir is not).
    let file = scratch("vugfacesave-not-a-dir");
    std::fs::write(&file, b"x").expect("scratch file");
    vw.save_to = Some(file.join("part.vug"));
    assert!(typed(&mut vw, "s"));
    let err = match &vw.view.design.journal.disk {
        Some(Err(e)) => e.clone(),
        other => panic!("refused by name, got {other:?}"),
    };
    assert!(err.contains("part.vug.tmp"), "{err}");
    assert_eq!(doc(&vw), &before);
    assert_eq!(vw.title, "tree*");
    // A face opened from no file has nowhere to write.
    let mut bare = Viewer::new(cube(), "bare", (W, H), Preset::Una);
    assert!(typed(&mut bare, "s"));
    assert!(matches!(&bare.view.design.journal.disk, Some(Err(e)) if e.starts_with("no file")), "{:?}", bare.view.design.journal.disk);
    // An existing document the face did not open is not overwritten.
    let other = scratch("vugfacesave-other.vug");
    std::fs::write(&other, "vug-doc version=1\n").expect("scratch doc");
    vw.save_to = Some(other.clone());
    assert!(typed(&mut vw, "s"));
    assert!(matches!(&vw.view.design.journal.disk, Some(Err(e)) if e.contains("exists")), "{:?}", vw.view.design.journal.disk);
    assert_eq!(std::fs::read_to_string(&other).expect("untouched"), "vug-doc version=1\n");
}
