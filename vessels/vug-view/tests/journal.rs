// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! VUGFACETREE (LEDGER SR513): the feature tree's host witness — the SR485 rectangle drawn by events and dimensioned by
//! the tool is a doc_core document that reads back and replays to the face's dof; `u` / `U` step the snapshots and the
//! face regenerates; deleting the extrude row removes the body; editing the width row regenerates the body; a refused
//! edit keeps the geometry and names its line. VUGFACESAVE (SR534): a saved document reopens through `Viewer::open` to the
//! same text and volume; the title's `*` follows an edit and clears on save; a path that cannot be written is named.
//! VUGFACESAVEAS (SR539): Save As (Cmd+Shift+S, a typed path) writes a new path beside the opened file and retitles; an
//! existing path is refused once and overwritten on the second Enter; Esc cancels with nothing written.

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

// ---- VUGFACESAVEAS (SR539): Save As, the chosen path ----

fn save_as_open(vw: &Viewer) -> bool {
    vw.view.design.journal.save_as.is_some()
}

#[test]
fn save_as_writes_a_new_path_beside_the_opened_file_and_retitles() {
    let mut vw = dimensioned();
    let orig = scratch("vugfacesaveas-orig.vug");
    let new = scratch("vugfacesaveas-new.vug");
    vw.save_to = Some(orig.clone());
    let before = doc(&vw).clone();
    assert!(key(&mut vw, "Meta+Shift+s"));
    assert!(save_as_open(&vw));
    // Relative and without an extension: beside the opened file, `.vug` appended. The field takes `u` / `t` (no undo,
    // no panel).
    assert!(typed(&mut vw, "vugfacesaveas-new"));
    assert_eq!(vw.view.design.journal.save_as.as_ref().map(|f| f.text.as_str()), Some("vugfacesaveas-new"));
    assert!(!vw.view.design.journal.panel);
    assert_eq!(doc(&vw), &before);
    assert!(key(&mut vw, "Return"));
    assert!(!save_as_open(&vw), "{:?}", vw.view.design.journal.disk);
    assert_eq!(vw.view.design.journal.disk, Some(Ok("saved vugfacesaveas-new.vug".to_string())));
    assert_eq!(std::fs::read_to_string(&new).expect("written"), doc_core::write(doc(&vw)));
    assert!(!orig.exists(), "the old target is not written");
    assert_eq!(vw.save_to.as_deref(), Some(new.as_path()));
    assert_eq!(vw.title, "vugfacesaveas-new.vug");
    // The chosen path is the face's own now: an edit and `s` write it again.
    assert!(typed(&mut vw, "u"));
    assert_eq!(vw.title, "vugfacesaveas-new.vug*");
    assert!(typed(&mut vw, "s"));
    assert_eq!(vw.title, "vugfacesaveas-new.vug", "{:?}", vw.view.design.journal.disk);
    assert_eq!(std::fs::read_to_string(&new).expect("rewritten"), doc_core::write(doc(&vw)));
}

#[test]
fn save_as_to_an_existing_path_refuses_once_then_overwrites() {
    let mut vw = dimensioned();
    let other = scratch("vugfacesaveas-existing.vug");
    std::fs::write(&other, "vug-doc version=1\n").expect("scratch doc");
    assert!(key(&mut vw, "Meta+Shift+s"));
    assert!(typed(&mut vw, other.to_str().expect("utf-8 path")));
    assert!(key(&mut vw, "Return"));
    assert!(
        matches!(&vw.view.design.journal.disk, Some(Err(e)) if e.contains("exists") && e.contains("Enter again")),
        "{:?}",
        vw.view.design.journal.disk
    );
    assert_eq!(std::fs::read_to_string(&other).expect("untouched"), "vug-doc version=1\n");
    assert!(save_as_open(&vw));
    assert_eq!(vw.title, "tree*");
    // The second Enter overwrites.
    assert!(key(&mut vw, "Return"));
    assert!(!save_as_open(&vw), "{:?}", vw.view.design.journal.disk);
    assert_eq!(std::fs::read_to_string(&other).expect("overwritten"), doc_core::write(doc(&vw)));
    assert_eq!(vw.title, "vugfacesaveas-existing.vug");
}

#[test]
fn esc_cancels_save_as_with_nothing_written() {
    let mut vw = dimensioned();
    let p = scratch("vugfacesaveas-esc.vug");
    assert!(key(&mut vw, "Meta+Shift+s"));
    assert!(typed(&mut vw, p.to_str().expect("utf-8 path")));
    assert!(key(&mut vw, "Escape"));
    assert!(!save_as_open(&vw));
    assert!(!p.exists());
    assert_eq!(vw.title, "tree*");
    assert_eq!(vw.view.design.journal.disk, None);
    // The chord again closes it too.
    assert!(key(&mut vw, "Meta+Shift+s"));
    assert!(typed(&mut vw, p.to_str().expect("utf-8 path")));
    assert!(key(&mut vw, "Meta+Shift+s"));
    assert!(!save_as_open(&vw));
    assert!(!p.exists());
    assert_eq!(vw.title, "tree*");
}

// VUGFACESAVEASCARET (LEDGER SR593): the Save As field edits at a caret on the `:` line's road (sketch.rs `caret_key` /
// `type_text_at`), painted by journal.rs `field_line_at`; Enter saves the edited name.

fn save_as_text(vw: &Viewer) -> (String, usize) {
    let f = vw.view.design.journal.save_as.as_ref().expect("the field is open");
    (f.text.clone(), f.caret)
}

#[test]
fn save_as_edits_at_its_caret_and_enter_saves_the_edited_name() {
    let mut vw = dimensioned();
    let orig = scratch("vugfacesaveascaret-orig.vug");
    let new = scratch("vugfacesaveascaret-new.vug");
    vw.save_to = Some(orig.clone());
    assert!(key(&mut vw, "Meta+Shift+s"));
    assert!(typed(&mut vw, "vugfacesaveascaret-nw"));
    assert_eq!(save_as_text(&vw), ("vugfacesaveascaret-nw".to_string(), 21), "typing leaves the caret at the end");
    let at_end = vug_view::raster::render(&vw.model, &vw.view);
    assert!(key(&mut vw, "Left"), "Left moves the caret");
    assert_eq!(save_as_text(&vw).1, 20);
    let mid = vug_view::raster::render(&vw.model, &vw.view);
    assert_ne!(mid.rgba, at_end.rgba, "the painted caret follows the caret");
    assert!(key(&mut vw, "Right") && !key(&mut vw, "Right"), "Right at the end does not move");
    assert!(key(&mut vw, "\u{f702}"), "the Cocoa Left moves");
    assert_eq!(save_as_text(&vw), ("vugfacesaveascaret-nw".to_string(), 20), "and is never typed");
    assert!(key(&mut vw, "Delete"), "Delete removes the char after the caret");
    assert_eq!(save_as_text(&vw), ("vugfacesaveascaret-n".to_string(), 20));
    assert!(typed(&mut vw, "w") && key(&mut vw, "Left"));
    assert!(typed(&mut vw, "e"), "a char inserts at the caret");
    assert_eq!(save_as_text(&vw), ("vugfacesaveascaret-new".to_string(), 21));
    assert!(typed(&mut vw, "x") && key(&mut vw, "\u{f702}") && key(&mut vw, "\u{f728}") && save_as_text(&vw) == ("vugfacesaveascaret-new".to_string(), 21), "VUGFACEFIELDKEYS (SR599): the Cocoa Delete reaches Delete's arm, never typed: {:?}", save_as_text(&vw));
    println!("save as caret {:?}", save_as_text(&vw));
    assert!(key(&mut vw, "Return"), "Enter saves the edited name");
    assert!(!save_as_open(&vw), "{:?}", vw.view.design.journal.disk);
    assert_eq!(std::fs::read_to_string(&new).expect("written"), doc_core::write(doc(&vw)));
    assert!(!orig.exists(), "the old target is not written");
    assert_eq!(vw.title, "vugfacesaveascaret-new.vug");
}

// ---- VUGMAKEMACHINELIBFACE (SR548): a part beside its machine library ----

/// The library's machine line (doc_core tests/doc.rs MACHINE_A's keys) and its axes / tool, as a `.vugmachine`.
const SHOP_MACHINE: &str = "machine shop name=\"Shop mill\" kind=mill3 kinematics=cartesian units=mm \
     envelope=[[-50 -50 -60] [100 100 50]] safe_z=10 power=spindle power_max=24000 ask_above=20000";
const SHOP_LINES: &str = "axis axis=x min=-50 max=100 max_feed=5000 max_accel=500 rapid=8000\n\
     axis axis=y min=-50 max=100 max_feed=5000 max_accel=500 rapid=8000\n\
     axis axis=z min=-60 max=50 max_feed=2000 max_accel=300 rapid=4000\n\
     tool number=1 kind=flat_end dc=6 lf=40 apmx=2 nof=2 sig=0\n";

#[test]
fn a_part_beside_its_library_opens_and_replays() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("vugmakemachinelibface");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    std::fs::write(dir.join("shop.vugmachine"), format!("vug-doc version=1 lin=1e-6 ang=1e-9\n{SHOP_MACHINE}\n{SHOP_LINES}")).expect("library");
    // The extruded rectangle's document with the machine named from the library.
    let body = doc_core::write(doc(&extruded()));
    let body = body.trim_end();
    let part = dir.join("part.vug");
    std::fs::write(&part, format!("{body}\nmachine mill lib=shop loaded=1\n")).expect("part");
    let vw = Viewer::open(part.to_str().expect("utf-8 path"), (W, H), Preset::Una).unwrap_or_else(|e| panic!("open: {e}"));
    assert!((volume(&vw) - 1600.0).abs() < 1e-4, "volume {}", volume(&vw));
    let Some(Stmt::Machine(m)) = doc(&vw).stmts.iter().find(|s| matches!(s, Stmt::Machine(_))) else { panic!("machine") };
    assert_eq!(m.lib.as_ref().map(|l| (l.name.as_str(), l.stmts.len())), Some(("shop", 4)), "the library's axes and tool are held");
    // The library machine replays as the same machine written inline.
    let shop_inline = SHOP_MACHINE.replacen("machine shop ", "machine mill ", 1);
    let inline = doc_core::read(&format!("{body}\n{shop_inline} loaded=1\n{SHOP_LINES}")).unwrap_or_else(|r| panic!("inline: {r:?}"));
    let (a, b) = (doc_core::machine_of(&inline).expect("inline machine"), doc_core::machine_of(doc(&vw)).expect("library machine"));
    assert_eq!(b, a, "the same make_core Machine");
    assert_eq!(b.digest(), a.digest());
    assert!(doc_core::write(doc(&vw)).contains("\nmachine mill lib=shop loaded=1\n"), "the part writes back the name only");
    // VUGMAKEMACHINELIBHELD (SR556): the delete re-reads from the library the part holds: no file, no shelf.
    std::fs::remove_file(dir.join("shop.vugmachine")).expect("library removed");
    let mut vw = vw;
    let x = index_of(&vw, |s| matches!(s, Stmt::Extrude { .. }));
    vw.view.design.journal.delete(x).unwrap_or_else(|e| panic!("delete: {e}"));
    assert!(!doc(&vw).stmts.iter().any(|s| matches!(s, Stmt::Extrude { .. })), "the extrude went");
    assert!(doc(&vw).stmts.iter().any(|s| matches!(s, Stmt::Machine(m) if m.lib.is_some())), "the library machine stays");
    // A library that is not beside the part refuses by name.
    let lost = dir.join("lost.vug");
    std::fs::write(&lost, format!("{body}\nmachine mill lib=nosuch loaded=1\n")).expect("part");
    let e = Viewer::open(lost.to_str().expect("utf-8 path"), (W, H), Preset::Una).err().expect("refused");
    assert!(e.contains("UnknownLib(\"nosuch\")"), "{e}");
}

// VUGFACEJOURNALSOLIDFAULT (LEDGER SR572): a document whose only failure no solid reads (an Op naming no machine) opens
// as the journal with that failure carried and the body built; its row is marked; a failure in the solid's own sketch
// still refuses by line.

/// A 20 x 10 x 5 box and a drill on it, with no machine in the document.
const BOX_DRILL: &str = "vug-doc version=1 lin=1e-6 ang=1e-9
sketch s1 o=[0 0 0] z=[0 0 1] x=[1 0 0]
point p1 sketch=s1 x=0 y=0
point p2 sketch=s1 x=20 y=0
point p3 sketch=s1 x=20 y=10
point p4 sketch=s1 x=0 y=10
line l1 sketch=s1 a=p1 b=p2
line l2 sketch=s1 a=p2 b=p3
line l3 sketch=s1 a=p3 b=p4
line l4 sketch=s1 a=p4 b=p1
fix f1 sketch=s1 p=p1 at=[0 0]
fix f2 sketch=s1 p=p2 at=[20 0]
fix f3 sketch=s1 p=p3 at=[20 10]
fix f4 sketch=s1 p=p4 at=[0 10]
extrude box sketch=s1 outer=l1 dir=[0 0 1] length=5
drill o1 on=box points=[[10 5]] top=5 depth=2 peck=0 tool=1 feed=300 plunge=100 rpm=10000
";

#[test]
fn a_doc_with_an_op_naming_no_machine_opens_with_its_fault_and_a_body() {
    let d = doc_core::read(BOX_DRILL).expect("the document reads");
    let vw = Viewer::open_doc(d, "box", (W, H), Preset::Una).expect("an Op's fault does not refuse the journal");
    let j = &vw.view.design.journal;
    let o1 = index_of(&vw, |s| s.id() == Some("o1"));
    let f = j.faults.iter().map(|f| (f.id.as_str(), f.fault.clone(), f.line)).collect::<Vec<_>>();
    println!("faults={f:?} volume={}", volume(&vw));
    assert_eq!(f, vec![("o1", doc_core::Fault::NoMachine, doc(&vw).line(o1))], "the one fault carried");
    assert!((volume(&vw) - 1000.0).abs() < 1e-4, "the box is the body: {}", volume(&vw));
    assert!(j.faulted(o1), "the drill row is marked");
    assert!((0..doc(&vw).stmts.len()).filter(|&i| i != o1).all(|i| !j.faulted(i)), "no other row is marked");

    // A conflicting fix in the box's sketch: a solid reads it, the journal refuses by line as before.
    let d = doc_core::read(&BOX_DRILL.replace("fix f4 sketch=s1 p=p4 at=[0 10]\n", "fix f4 sketch=s1 p=p4 at=[0 10]\nfix f5 sketch=s1 p=p1 at=[1 1]\n")).expect("reads");
    let e = Viewer::open_doc(d, "box", (W, H), Preset::Una).err().expect("a solid's fault refuses");
    assert!(e.starts_with("line ") && e.contains("Conflicting"), "{e}");
}

// VUGFACEJOURNALFAULTFIX (LEDGER SR578): the box and drill with the shop's library machine; the panel deletes the machine
// row, the drill turns red (NoMachine), the one machine the undo history holds is offered on the drill's row, and Enter
// puts it back as ONE snapshot: no fault, the same body, the document as it was; one `u` returns the fault. A document
// whose history holds no machine offers nothing.

#[test]
fn a_no_machine_fault_offers_the_machine_and_enter_clears_it() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("vugfacejournalfaultfix");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    std::fs::write(dir.join("shop.vugmachine"), format!("vug-doc version=1 lin=1e-6 ang=1e-9\n{SHOP_MACHINE}\n{SHOP_LINES}")).expect("library");
    let part = dir.join("part.vug");
    std::fs::write(&part, BOX_DRILL.replace("drill o1 ", "machine mill lib=shop loaded=1\ndrill o1 ")).expect("part");
    let mut vw = Viewer::open(part.to_str().expect("utf-8 path"), (W, H), Preset::Una).unwrap_or_else(|e| panic!("open: {e}"));
    assert!(vw.view.design.journal.faults.is_empty(), "the drill generates with the machine: {:?}", vw.view.design.journal.faults);
    let held = doc(&vw).clone();

    // The panel deletes the machine row: the drill's NoMachine is carried, the box stays.
    let m = index_of(&vw, |s| matches!(s, Stmt::Machine(_)));
    vw.view.design.journal.delete(m).unwrap_or_else(|e| panic!("delete: {e}"));
    let o1 = index_of(&vw, |s| s.id() == Some("o1"));
    let line = doc(&vw).line(o1);
    let f = vw.view.design.journal.faults.iter().map(|f| (f.id.clone(), f.fault.clone())).collect::<Vec<_>>();
    assert_eq!(f, vec![("o1".to_string(), doc_core::Fault::NoMachine)], "the drill is faulted");

    // The drill's row selected: the history's one machine is offered on it.
    assert!(vw.view.design.journal.select_line(line), "the drill's line is a row");
    let fix = vw.view.design.journal.offered().expect("a fix is offered on the faulted row");
    assert_eq!((fix.text.as_str(), fix.line), ("machine mill lib=shop loaded=1", line));
    let depth = vw.view.design.journal.depth().0;

    // Enter takes it: one snapshot, no fault, the same box, the document it was before the delete.
    assert!(key(&mut vw, "Return"), "Enter takes the fix");
    let j = &vw.view.design.journal;
    println!("fix offered={} line={line} faults_after={} volume={}", fix.text, j.faults.len(), volume(&vw));
    assert!(j.faults.is_empty(), "{:?}", j.faults);
    assert!((volume(&vw) - 1000.0).abs() < 1e-4, "the body unchanged: {}", volume(&vw));
    assert_eq!(*doc(&vw), held, "the machine is back on its line");
    assert_eq!(j.depth().0, depth + 1, "ONE snapshot");
    assert_eq!(j.note.as_deref(), Some(format!("fixed line {line}: machine mill lib=shop loaded=1").as_str()));
    assert!(j.offered().is_none(), "nothing left to fix");
    assert!(key(&mut vw, "u"), "one u");
    assert_eq!(vw.view.design.journal.faults.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["o1"], "one u returns the fault");

    // No machine anywhere in the history: the fault is marked, nothing is offered.
    let d = doc_core::read(BOX_DRILL).expect("the document reads");
    let mut vw = Viewer::open_doc(d, "box", (W, H), Preset::Una).expect("opens");
    let o1 = index_of(&vw, |s| s.id() == Some("o1"));
    let line = doc(&vw).line(o1);
    assert!(vw.view.design.journal.select_line(line));
    assert!(vw.view.design.journal.faulted(o1) && vw.view.design.journal.offered().is_none(), "no machine to offer");
}

// VUGFACEJOURNALFAULTFIXMANY (LEDGER SR584): a fix with a choice is one card. The shop part's machine re-named `bench`
// (the same library) and then deleted: the drill's NoMachine offers both machines of the history, newest first; Tab
// picks the second and Enter puts the opened document back as ONE snapshot. A part with no machine in its history
// offers the one `.vugmachine` beside it.

#[test]
fn a_history_with_two_machines_offers_a_card_and_tab_picks_the_second() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("vugfacejournalfaultfixmany");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    std::fs::write(dir.join("shop.vugmachine"), format!("vug-doc version=1 lin=1e-6 ang=1e-9\n{SHOP_MACHINE}\n{SHOP_LINES}")).expect("library");
    let part = dir.join("part.vug");
    std::fs::write(&part, BOX_DRILL.replace("drill o1 ", "machine mill lib=shop loaded=1\ndrill o1 ")).expect("part");
    let mut vw = Viewer::open(part.to_str().expect("utf-8 path"), (W, H), Preset::Una).unwrap_or_else(|e| panic!("open: {e}"));
    let held = doc(&vw).clone();

    // The machine re-named `bench` (one snapshot), then deleted: two machines in the history.
    let m = index_of(&vw, |s| matches!(s, Stmt::Machine(_)));
    let mut cand = held.clone();
    cand.lines.clear();
    let Stmt::Machine(mill) = &held.stmts[m] else { panic!("machine") };
    cand.stmts[m] = Stmt::Machine(doc_core::MachineStmt { id: "bench".into(), ..mill.clone() });
    vw.view.design.journal.adopt(cand).unwrap_or_else(|e| panic!("adopt: {e}"));
    vw.view.design.journal.delete(m).unwrap_or_else(|e| panic!("delete: {e}"));
    let o1 = index_of(&vw, |s| s.id() == Some("o1"));
    let line = doc(&vw).line(o1);
    assert!(vw.view.design.journal.select_line(line), "the drill's line is a row");

    // The card: both machines, newest first; Tab picks the second.
    let texts = vw.view.design.journal.fixes_for(o1).into_iter().map(|f| f.text).collect::<Vec<_>>();
    assert_eq!(texts, ["machine bench lib=shop loaded=1", "machine mill lib=shop loaded=1"], "the history's two machines");
    assert_eq!(vw.view.design.journal.offered().map(|f| f.text).as_deref(), Some("machine bench lib=shop loaded=1"));
    assert!(key(&mut vw, "Tab"), "Tab walks the card");
    let picked = vw.view.design.journal.offered().expect("a pick").text;
    assert_eq!(picked, "machine mill lib=shop loaded=1", "Tab picked the second");
    let depth = vw.view.design.journal.depth().0;

    // Enter takes the pick: ONE snapshot, no fault, the opened document.
    assert!(key(&mut vw, "Return"), "Enter takes the pick");
    let j = &vw.view.design.journal;
    println!("fix choices={} picked={picked} faults_after={}", texts.len(), j.faults.len());
    assert!(j.faults.is_empty(), "{:?}", j.faults);
    assert_eq!(*doc(&vw), held, "the opened document, the mill on its line");
    assert_eq!(j.depth().0, depth + 1, "ONE snapshot");
    assert!(j.offered().is_none(), "nothing left to fix");
}

#[test]
fn a_part_with_one_machine_beside_it_offers_the_shelf() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("vugfacejournalfaultfixshelf");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch dir");
    std::fs::write(dir.join("shop.vugmachine"), format!("vug-doc version=1 lin=1e-6 ang=1e-9\n{SHOP_MACHINE}\n{SHOP_LINES}")).expect("library");
    let part = dir.join("part.vug");
    std::fs::write(&part, BOX_DRILL).expect("part");
    let mut vw = Viewer::open(part.to_str().expect("utf-8 path"), (W, H), Preset::Una).unwrap_or_else(|e| panic!("open: {e}"));
    assert_eq!(vw.view.design.journal.depth().0, 0, "no history");
    let o1 = index_of(&vw, |s| s.id() == Some("o1"));
    let line = doc(&vw).line(o1);
    assert!(vw.view.design.journal.faulted(o1), "the drill names no machine");
    assert!(vw.view.design.journal.select_line(line), "the drill's line is a row");
    assert_eq!(vw.view.design.journal.fixes_for(o1).len(), 1, "one machine on the shelf: no card");
    let offered = vw.view.design.journal.offered().expect("the shelf's machine is offered").text;
    assert_eq!(offered, "machine shop lib=shop");

    assert!(key(&mut vw, "Return"), "Enter takes it");
    let j = &vw.view.design.journal;
    println!("shelf offered={offered} faults_after={} volume={}", j.faults.len(), volume(&vw));
    assert!(j.faults.is_empty(), "{:?}", j.faults);
    assert!((volume(&vw) - 1000.0).abs() < 1e-4, "the box unchanged: {}", volume(&vw));
    assert!(doc_core::write(doc(&vw)).contains("\nmachine shop lib=shop\ndrill o1 "), "the shelf machine before the drill, by name");
}

// VUGFACEJOURNALFAULTFIXREF (LEDGER SR590): a line whose end names a line (a BadRef: the slot wanted a point) in a sketch
// no feature names is carried; its row offers the sketch's live points it does not already name, one card; Tab picks the
// second and Enter re-names the end as ONE snapshot: no fault, the box unchanged.

/// The 20 x 10 x 5 box and an open sketch whose `m2` starts at the line `m1`.
const BOX_BADREF: &str = "vug-doc version=1 lin=1e-6 ang=1e-9
sketch s1 o=[0 0 0] z=[0 0 1] x=[1 0 0]
point p1 sketch=s1 x=0 y=0
point p2 sketch=s1 x=20 y=0
point p3 sketch=s1 x=20 y=10
point p4 sketch=s1 x=0 y=10
line l1 sketch=s1 a=p1 b=p2
line l2 sketch=s1 a=p2 b=p3
line l3 sketch=s1 a=p3 b=p4
line l4 sketch=s1 a=p4 b=p1
fix f1 sketch=s1 p=p1 at=[0 0]
fix f2 sketch=s1 p=p2 at=[20 0]
fix f3 sketch=s1 p=p3 at=[20 10]
fix f4 sketch=s1 p=p4 at=[0 10]
extrude box sketch=s1 outer=l1 dir=[0 0 1] length=5
sketch s2 o=[0 0 20] z=[0 0 1] x=[1 0 0]
point q1 sketch=s2 x=0 y=0
point q2 sketch=s2 x=8 y=0
point q3 sketch=s2 x=8 y=6
fix g1 sketch=s2 p=q1 at=[0 0]
fix g2 sketch=s2 p=q2 at=[8 0]
fix g3 sketch=s2 p=q3 at=[8 6]
line m1 sketch=s2 a=q1 b=q2
line m2 sketch=s2 a=m1 b=q2
";

#[test]
fn a_bad_ref_offers_the_sketch_points_and_tab_return_fixes_it() {
    let d = doc_core::read(BOX_BADREF).expect("a wrong-kind end is geometry, not syntax: the document reads");
    let mut vw = Viewer::open_doc(d, "box", (W, H), Preset::Una).expect("a BadRef no solid reads does not refuse the journal");
    let m2 = index_of(&vw, |s| s.id() == Some("m2"));
    let line = doc(&vw).line(m2);
    let f = vw.view.design.journal.faults.iter().map(|f| (f.id.clone(), f.fault.clone())).collect::<Vec<_>>();
    assert_eq!(f, vec![("m2".to_string(), doc_core::Fault::BadRef { id: "m1".into(), kind: doc_core::RefKind::Point })], "the end wanted a point");
    assert!(vw.view.design.journal.select_line(line), "m2's line is a row");

    // The card: the sketch's points before m2 that m2 does not already name, in document order.
    let texts = vw.view.design.journal.fixes_for(m2).into_iter().map(|f| f.text).collect::<Vec<_>>();
    assert_eq!(texts, ["line m2 sketch=s2 a=q1 b=q2", "line m2 sketch=s2 a=q3 b=q2"], "q1 and q3, not q2 (named) nor a line");
    assert!(vw.view.design.journal.choosing_fix(), "two choices are a card");
    assert!(key(&mut vw, "Tab"), "Tab walks the card");
    let picked = vw.view.design.journal.offered().expect("a pick").text;
    assert_eq!(picked, "line m2 sketch=s2 a=q3 b=q2", "Tab picked the second");
    let depth = vw.view.design.journal.depth().0;

    // Enter takes the pick: ONE snapshot, no fault, the end re-named, the box unchanged.
    assert!(key(&mut vw, "Return"), "Enter takes the pick");
    let j = &vw.view.design.journal;
    println!("badref choices={} picked={picked} faults_after={} volume={}", texts.len(), j.faults.len(), volume(&vw));
    assert!(j.faults.is_empty(), "{:?}", j.faults);
    assert_eq!(doc(&vw).stmts[m2], Stmt::Line { id: "m2".into(), sketch: "s2".into(), a: "q3".into(), b: "q2".into() });
    assert_eq!(j.depth().0, depth + 1, "ONE snapshot");
    assert_eq!(j.note.as_deref(), Some(format!("fixed line {line}: line m2 sketch=s2 a=q3 b=q2").as_str()));
    assert!((volume(&vw) - 1000.0).abs() < 1e-4, "the box unchanged: {}", volume(&vw));
    assert!(j.offered().is_none(), "nothing left to fix");
}

// VUGFACEJOURNALSOLIDFAULTFIX (LEDGER SR598): an extrude whose `outer` names a point (NoProfile: the solid half, refused
// before) opens held: the box above it painted, the extrude's row faulted with the sketch's two closed loops; the card
// offers each loop into `outer=`, Tab picks the second and Enter repairs it as ONE snapshot: no fault, the post built.

/// The 20 x 10 x 5 box and, on its top, a sketch of two squares (4 x 4 `m*`, 2 x 5 `n*`) and a point `q9`; the post's
/// `outer` names the point.
const BOX_NOPROFILE: &str = "vug-doc version=1 lin=1e-6 ang=1e-9
sketch s1 o=[0 0 0] z=[0 0 1] x=[1 0 0]
point p1 sketch=s1 x=0 y=0
point p2 sketch=s1 x=20 y=0
point p3 sketch=s1 x=20 y=10
point p4 sketch=s1 x=0 y=10
line l1 sketch=s1 a=p1 b=p2
line l2 sketch=s1 a=p2 b=p3
line l3 sketch=s1 a=p3 b=p4
line l4 sketch=s1 a=p4 b=p1
fix f1 sketch=s1 p=p1 at=[0 0]
fix f2 sketch=s1 p=p2 at=[20 0]
fix f3 sketch=s1 p=p3 at=[20 10]
fix f4 sketch=s1 p=p4 at=[0 10]
extrude box sketch=s1 outer=l1 dir=[0 0 1] length=5
sketch s2 o=[0 0 5] z=[0 0 1] x=[1 0 0]
point q1 sketch=s2 x=0 y=0
point q2 sketch=s2 x=4 y=0
point q3 sketch=s2 x=4 y=4
point q4 sketch=s2 x=0 y=4
point r1 sketch=s2 x=10 y=0
point r2 sketch=s2 x=12 y=0
point r3 sketch=s2 x=12 y=5
point r4 sketch=s2 x=10 y=5
point q9 sketch=s2 x=16 y=8
line m1 sketch=s2 a=q1 b=q2
line m2 sketch=s2 a=q2 b=q3
line m3 sketch=s2 a=q3 b=q4
line m4 sketch=s2 a=q4 b=q1
line n1 sketch=s2 a=r1 b=r2
line n2 sketch=s2 a=r2 b=r3
line n3 sketch=s2 a=r3 b=r4
line n4 sketch=s2 a=r4 b=r1
fix g1 sketch=s2 p=q1 at=[0 0]
fix g2 sketch=s2 p=q2 at=[4 0]
fix g3 sketch=s2 p=q3 at=[4 4]
fix g4 sketch=s2 p=q4 at=[0 4]
fix g5 sketch=s2 p=r1 at=[10 0]
fix g6 sketch=s2 p=r2 at=[12 0]
fix g7 sketch=s2 p=r3 at=[12 5]
fix g8 sketch=s2 p=r4 at=[10 5]
fix g9 sketch=s2 p=q9 at=[16 8]
extrude post sketch=s2 outer=q9 dir=[0 0 1] length=5
";

#[test]
fn an_extrude_on_no_profile_opens_held_and_tab_return_fixes_it() {
    let d = doc_core::read(BOX_NOPROFILE).expect("an outer on a point is geometry, not syntax: the document reads");
    let mut vw = Viewer::open_doc(d, "box", (W, H), Preset::Una).expect("ONE solid fault with its data is held, not refused");
    let post = index_of(&vw, |s| s.id() == Some("post"));
    let line = doc(&vw).line(post);
    let faults = vw.view.design.journal.faults.clone();
    assert_eq!(faults.len(), 1, "{faults:?}");
    assert_eq!((faults[0].id.as_str(), faults[0].line), ("post", line));
    let doc_core::Fault::NoProfile { loops } = faults[0].fault.clone() else { panic!("{faults:?}") };
    assert_eq!(loops.len(), 2, "two closed loops: {loops:?}");
    let held_volume = volume(&vw);
    assert!((held_volume - 1000.0).abs() < 1e-4, "the box painted, the post held: {held_volume}");
    assert!(vw.view.design.journal.faulted(post), "the post row is marked");
    assert!(vw.view.design.journal.select_line(line), "the post's line is a row");

    // The card: each loop into `outer=`, in the fault's order.
    let texts = vw.view.design.journal.fixes_for(post).into_iter().map(|f| f.text).collect::<Vec<_>>();
    let want = loops.iter().map(|l| format!("extrude post sketch=s2 outer={l} dir=[0 0 1] length=5")).collect::<Vec<_>>();
    assert_eq!(texts, want, "one choice per loop");
    assert!(vw.view.design.journal.choosing_fix(), "two choices are a card");
    assert!(key(&mut vw, "Tab"), "Tab walks the card");
    let picked = vw.view.design.journal.offered().expect("a pick").text;
    assert_eq!(picked, want[1], "Tab picked the second");
    let depth = vw.view.design.journal.depth().0;

    // Enter takes the pick: ONE snapshot, no fault, the post built on the picked loop.
    assert!(key(&mut vw, "Return"), "Enter takes the pick");
    let j = &vw.view.design.journal;
    let area = if loops[1].starts_with('m') { 16.0 } else { 10.0 };
    println!("solidfault loops={} held_volume={held_volume} picked={picked} faults_after={} volume={}", loops.len(), j.faults.len(), volume(&vw));
    assert!(j.faults.is_empty(), "{:?}", j.faults);
    assert_eq!(j.depth().0, depth + 1, "ONE snapshot");
    assert_eq!(j.note.as_deref(), Some(format!("fixed line {line}: {picked}").as_str()));
    assert!((volume(&vw) - (1000.0 + 5.0 * area)).abs() < 1e-4, "the box and the post: {}", volume(&vw));
    assert!(j.offered().is_none(), "nothing left to fix");
}

// ---- VUGFACEJOURNALHOLDMANY (LEDGER SR609): two independent solid roots open held, each card follows the last fix ----
// SR598's box and post with a second post (`post2`, its own sketch s3 of two squares and a point, its `outer` on the
// point): both NoProfile roots are held (SR572 refused, SR598 held only ONE), the box painted; the card is offered for
// the FIRST root in line order (the cursor on it), the second root's row marked and offering nothing; Tab + Return
// fixes the first (ONE snapshot), the cursor moves to the second and its card follows; Tab + Return fixes it.

/// BOX_NOPROFILE and a sketch s3 on the box's top: a 2 x 2 square `t*` at (14, 6), a 3 x 3 square `v*` at (16, 0) and a
/// point `z9`; `post2`'s `outer` names the point.
fn two_roots() -> String {
    BOX_NOPROFILE.to_string()
        + "sketch s3 o=[0 0 5] z=[0 0 1] x=[1 0 0]
point u1 sketch=s3 x=14 y=6
point u2 sketch=s3 x=16 y=6
point u3 sketch=s3 x=16 y=8
point u4 sketch=s3 x=14 y=8
point w1 sketch=s3 x=16 y=0
point w2 sketch=s3 x=19 y=0
point w3 sketch=s3 x=19 y=3
point w4 sketch=s3 x=16 y=3
point z9 sketch=s3 x=5 y=8
line t1 sketch=s3 a=u1 b=u2
line t2 sketch=s3 a=u2 b=u3
line t3 sketch=s3 a=u3 b=u4
line t4 sketch=s3 a=u4 b=u1
line v1 sketch=s3 a=w1 b=w2
line v2 sketch=s3 a=w2 b=w3
line v3 sketch=s3 a=w3 b=w4
line v4 sketch=s3 a=w4 b=w1
fix h1 sketch=s3 p=u1 at=[14 6]
fix h2 sketch=s3 p=u2 at=[16 6]
fix h3 sketch=s3 p=u3 at=[16 8]
fix h4 sketch=s3 p=u4 at=[14 8]
fix h5 sketch=s3 p=w1 at=[16 0]
fix h6 sketch=s3 p=w2 at=[19 0]
fix h7 sketch=s3 p=w3 at=[19 3]
fix h8 sketch=s3 p=w4 at=[16 3]
fix h9 sketch=s3 p=z9 at=[5 8]
extrude post2 sketch=s3 outer=z9 dir=[0 0 1] length=5
"
}

#[test]
fn two_held_roots_open_held_and_each_card_follows_the_last_fix() {
    let d = doc_core::read(&two_roots()).expect("two outers on points are geometry, not syntax: the document reads");
    let mut vw = Viewer::open_doc(d, "box", (W, H), Preset::Una).expect("two solid roots with their data are held, not refused");
    let post = index_of(&vw, |s| s.id() == Some("post"));
    let post2 = index_of(&vw, |s| s.id() == Some("post2"));
    let (line, line2) = (doc(&vw).line(post), doc(&vw).line(post2));
    let ids = vw.view.design.journal.faults.iter().map(|f| (f.id.clone(), f.line)).collect::<Vec<_>>();
    assert_eq!(ids, vec![("post".to_string(), line), ("post2".to_string(), line2)], "both roots held, in line order");
    assert!(vw.view.design.journal.faults.iter().all(|f| matches!(f.fault, doc_core::Fault::NoProfile { .. })));
    let held_volume = volume(&vw);
    assert!((held_volume - 1000.0).abs() < 1e-4, "the box painted, both posts held: {held_volume}");
    assert!(vw.view.design.journal.faulted(post) && vw.view.design.journal.faulted(post2), "both rows marked");
    assert_eq!(vw.view.design.journal.cursor, post, "the cursor opens on the first root");

    // The panel shown: the first root's card is offered; the second root offers nothing yet.
    assert!(typed(&mut vw, "t"), "t shows the panel");
    let f = vw.view.design.journal.offered().expect("the first root's card");
    assert_eq!(f.line, line, "offered on the post's line");
    assert!(vw.view.design.journal.fixes_for(post2).is_empty(), "the card is the first root's only");
    assert!(vw.view.design.journal.choosing_fix(), "two loops are a card");
    assert!(key(&mut vw, "Tab"), "Tab walks the card");
    let first = vw.view.design.journal.offered().expect("a pick").text;
    let depth = vw.view.design.journal.depth().0;
    assert!(key(&mut vw, "Return"), "Return takes the pick");

    // ONE snapshot restored ONE root; the cursor and the card follow to the second.
    let j = &vw.view.design.journal;
    assert_eq!(j.depth().0, depth + 1, "ONE snapshot");
    assert_eq!(j.note.as_deref(), Some(format!("fixed line {line}: {first}").as_str()));
    assert_eq!(j.faults.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["post2"], "{:?}", j.faults);
    assert_eq!(j.cursor, post2, "the cursor on the next root");
    assert_eq!(j.offered().map(|f| f.line), Some(line2), "the next card follows");
    assert!(j.choosing_fix(), "two loops are a card");
    assert!(key(&mut vw, "Tab"), "Tab walks the card");
    let second = vw.view.design.journal.offered().expect("a pick").text;
    let depth = vw.view.design.journal.depth().0;
    assert!(key(&mut vw, "Return"), "Return takes the pick");

    let j = &vw.view.design.journal;
    let area = |t: &str, a: (&str, f64), b: f64| if t.contains(a.0) { a.1 } else { b };
    let want = 1000.0 + 5.0 * area(&first, ("outer=m", 16.0), 10.0) + 5.0 * area(&second, ("outer=t", 4.0), 9.0);
    println!("holdmany roots=post,post2 held_volume={held_volume} first={first} second={second} faults_after={} volume={}", j.faults.len(), volume(&vw));
    assert_eq!(j.depth().0, depth + 1, "ONE snapshot");
    assert!(j.faults.is_empty(), "{:?}", j.faults);
    assert!((volume(&vw) - want).abs() < 1e-4, "the box and both posts: {} vs {want}", volume(&vw));
    assert!(j.offered().is_none(), "nothing left to fix");
}

// ---- VUGFACEJOURNALHOLDREST (LEDGER SR611): a first held root with no fix passes the card to the next ----
// SR598's box and post with a fillet `round` on the box, right after it, whose first edge name is gone and whose list
// already names every edge of the box: a NoSuchEdge with no resolving name left (its card offers nothing), the FIRST
// root in line order; the post (NoProfile, two loops) the second. SR609 offered the card on the first root only, so the
// post's card never showed; now the card is the first held root WITH a fix: the cursor opens on the post, Tab + Return
// fixes it (ONE snapshot), the round stays held and first in the failures (the `held=` list), the cursor falls back to
// it with no card.

#[test]
fn a_first_held_root_with_no_fix_passes_the_card_to_the_next() {
    let base = doc_core::read(BOX_NOPROFILE).expect("reads");
    let r = doc_core::replay(&base, base.tol);
    let solid = r.solids.iter().find(|s| s.id == "box").expect("the box replays");
    let gone = brep_core::Name { op: brep_core::OpKind::Extrude, role: brep_core::Role::CapEnd, a: 9, b: 9 };
    let mut edges = vec![gone];
    for e in &solid.brep.edges {
        if !edges.contains(&e.name) {
            edges.push(e.name);
        }
    }
    assert!(edges.len() > 1, "the box has edges: {edges:?}");
    let mut d = base.clone();
    d.lines.clear();
    let at = d.stmts.iter().position(|s| s.id() == Some("box")).expect("the box row") + 1;
    d.stmts.insert(at, Stmt::Fillet { id: "round".into(), on: "box".into(), edges, r: 1.0 });
    let d = doc_core::read(&doc_core::write(&d)).expect("the fillet line reads");

    let mut vw = Viewer::open_doc(d, "box", (W, H), Preset::Una).expect("two solid roots are held, not refused");
    let round = index_of(&vw, |s| s.id() == Some("round"));
    let post = index_of(&vw, |s| s.id() == Some("post"));
    let (rline, line) = (doc(&vw).line(round), doc(&vw).line(post));
    let ids = vw.view.design.journal.faults.iter().map(|f| (f.id.clone(), f.line)).collect::<Vec<_>>();
    assert_eq!(ids, vec![("round".to_string(), rline), ("post".to_string(), line)], "both roots held, the round first");
    assert!(matches!(vw.view.design.journal.faults[0].fault, doc_core::Fault::NoSuchEdge { .. }), "{:?}", vw.view.design.journal.faults);
    assert!(matches!(vw.view.design.journal.faults[1].fault, doc_core::Fault::NoProfile { .. }), "{:?}", vw.view.design.journal.faults);
    assert!((volume(&vw) - 1000.0).abs() < 1e-4, "the box painted, the round and the post held: {}", volume(&vw));
    assert!(vw.view.design.journal.faulted(round) && vw.view.design.journal.faulted(post), "both rows marked");
    assert!(vw.view.design.journal.fixes_for(round).is_empty(), "the round's card offers nothing");
    assert_eq!(vw.view.design.journal.cursor, post, "the cursor opens on the first root with a fix");

    // The post's card is offered though the round is first.
    assert!(typed(&mut vw, "t"), "t shows the panel");
    let f = vw.view.design.journal.offered().expect("the post's card");
    assert_eq!(f.line, line, "offered on the post's line");
    assert_eq!(vw.view.design.journal.fixes_for(post).len(), 2, "one choice per loop");
    assert!(key(&mut vw, "Tab"), "Tab walks the card");
    let picked = vw.view.design.journal.offered().expect("a pick").text;
    let depth = vw.view.design.journal.depth().0;
    assert!(key(&mut vw, "Return"), "Return takes the pick");

    // ONE snapshot restored the post; the round stays held and first, the cursor on it, no card.
    let j = &vw.view.design.journal;
    let area = if picked.contains("outer=m") { 16.0 } else { 10.0 };
    println!("holdrest roots=round,post first_fix=post picked={picked} held_after={} volume={}", j.faults.iter().map(|f| f.id.as_str()).collect::<Vec<_>>().join(","), volume(&vw));
    assert_eq!(j.depth().0, depth + 1, "ONE snapshot");
    assert_eq!(j.note.as_deref(), Some(format!("fixed line {line}: {picked}").as_str()));
    assert_eq!(j.faults.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["round"], "{:?}", j.faults);
    assert_eq!(j.cursor, index_of(&vw, |s| s.id() == Some("round")), "no root with a fix: the cursor on the first held root");
    assert!(vw.view.design.journal.offered().is_none(), "the round offers nothing");
    assert!((volume(&vw) - (1000.0 + 5.0 * area)).abs() < 1e-4, "the box and the post: {}", volume(&vw));
}
