// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una

//! VUGDOCFROMSKETCH (LEDGER SR516): doc_core's one sketch writer. A sketch written by `from_sketch` reads back to
//! the same document, replays clean to the same dof and the same live entity count, skips a tombstone while every
//! later slot keeps its name, and lands each live slot at replay's dense id in slot order.

use doc_core::{Doc, PlainNames, SketchNames, Stmt, Tol, from_sketch, read, replay, write};
use sketch_core::{Constraint, ConstraintId, Entity, EntityId, PointRef, Sketch, Vec2};

const XY: ([f64; 3], [f64; 3], [f64; 3]) = ([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]);

/// A 10 x 5 rectangle on four shared corners (slots 0-7), a circle of r 1 on a fixed centre (slots 8-9), a free point
/// (slot 10) and an arc on its own centre (slots 11-12); constraints k0-k8, the circle's radius and centre last.
fn sketch() -> (Sketch, u32) {
    let mut sk = Sketch::new();
    let p: Vec<EntityId> = [(0.0, 0.0), (10.0, 0.0), (10.0, 5.0), (0.0, 5.0)].iter().map(|&(x, y)| sk.add_point(x, y)).collect();
    let l: Vec<EntityId> = (0..4).map(|i| sk.add_line(p[i], p[(i + 1) % 4]).unwrap()).collect();
    let c = sk.add_point(5.0, 2.5);
    let circle = sk.add_circle(c, 1.0).unwrap();
    sk.add_point(20.0, 20.0);
    let ac = sk.add_point(30.0, 0.0);
    sk.add_arc(ac, 2.0, 0.0, 1.5).unwrap();
    let cons = [
        Constraint::Fix(PointRef::Point(p[0]), Vec2::new(0.0, 0.0)),
        Constraint::Horizontal(l[0]),
        Constraint::Vertical(l[1]),
        Constraint::Horizontal(l[2]),
        Constraint::Vertical(l[3]),
        Constraint::Distance(PointRef::Point(p[0]), PointRef::Point(p[1]), 10.0),
        Constraint::Distance(PointRef::Point(p[1]), PointRef::Point(p[2]), 5.0),
        Constraint::Radius(circle, 1.0),
        Constraint::Fix(PointRef::Point(c), Vec2::new(5.0, 2.5)),
    ];
    let mut issued = 0;
    for k in cons {
        issued = sk.add(k).unwrap().0 + 1;
    }
    (sk, issued)
}

fn doc_of(sk: &Sketch, issued: u32, names: &dyn SketchNames) -> Doc {
    let mut doc = Doc::new(Tol::DEFAULT);
    doc.stmts = from_sketch(sk, XY, issued, names);
    doc
}

fn live(sk: &Sketch) -> Vec<usize> {
    sk.entities().iter().enumerate().filter(|(_, e)| !matches!(e, Entity::Removed)).map(|(i, _)| i).collect()
}

fn kind(e: &Entity) -> &'static str {
    match e {
        Entity::Point { .. } => "point",
        Entity::Line { .. } => "line",
        Entity::Circle { .. } => "circle",
        Entity::Arc { .. } => "arc",
        Entity::Removed => "removed",
    }
}

fn entity_ids(doc: &Doc) -> Vec<String> {
    doc.stmts
        .iter()
        .filter_map(|s| match s {
            Stmt::Point { id, .. } | Stmt::Line { id, .. } | Stmt::Circle { id, .. } | Stmt::Arc { id, .. } => Some(id.clone()),
            _ => None,
        })
        .collect()
}

fn constraint_ids(doc: &Doc) -> Vec<String> {
    doc.stmts.iter().filter_map(|s| if let Stmt::Constraint { id, .. } = s { Some(id.clone()) } else { None }).collect()
}

/// Write, read back, replay: the same document, no failure, the same live entity count and the same dof.
fn round_trip(sk: &Sketch, doc: &Doc) {
    let back = read(&write(doc)).expect("the written sketch reads");
    assert_eq!(&back, doc);
    let rep = replay(&back, back.tol);
    assert!(rep.failures.is_empty(), "{:?}", rep.failures);
    assert_eq!(rep.sketches.len(), 1);
    let out = &rep.sketches[0];
    assert_eq!(out.sketch.entities().len(), live(sk).len());
    assert_eq!(sketch_core::dof(&out.sketch).count, sketch_core::dof(sk).count);
    let mut solved = sk.clone();
    assert_eq!(sketch_core::solve(&mut solved, Tol::DEFAULT).expect("the sketch solves").dof, out.solved.dof);
}

#[test]
fn a_written_sketch_reads_back_and_replays_to_the_same_dof() {
    let (sk, issued) = sketch();
    let doc = doc_of(&sk, issued, &PlainNames);
    assert!(matches!(&doc.stmts[0], Stmt::Sketch { id, .. } if id == "s"));
    assert_eq!(entity_ids(&doc), (0..13).map(|n| format!("e{n}")).collect::<Vec<_>>());
    assert_eq!(constraint_ids(&doc), (0..9).map(|n| format!("k{n}")).collect::<Vec<_>>());
    round_trip(&sk, &doc);
    // The free point and the arc keep their freedom: the dof is not zero.
    assert!(sketch_core::dof(&sk).count > 0);
}

#[test]
fn a_tombstone_is_skipped_and_the_names_stay_by_slot() {
    let (mut sk, issued) = sketch();
    let gone = sk.remove_entity(EntityId(8)).expect("the centre is live");
    assert_eq!(gone.entities, vec![EntityId(8), EntityId(9)]);
    assert_eq!(gone.constraints, vec![ConstraintId(7), ConstraintId(8)]);
    let doc = doc_of(&sk, issued, &PlainNames);
    let want: Vec<String> = [0, 1, 2, 3, 4, 5, 6, 7, 10, 11, 12].iter().map(|n| format!("e{n}")).collect();
    assert_eq!(entity_ids(&doc), want);
    assert_eq!(constraint_ids(&doc), (0..7).map(|n| format!("k{n}")).collect::<Vec<_>>());
    round_trip(&sk, &doc);
}

#[test]
fn replay_issues_the_live_slots_densely_in_slot_order() {
    let (mut sk, issued) = sketch();
    sk.remove_entity(EntityId(8)).unwrap();
    let doc = doc_of(&sk, issued, &PlainNames);
    let rep = replay(&doc, doc.tol);
    assert!(rep.failures.is_empty(), "{:?}", rep.failures);
    let replayed = &rep.sketches[0].sketch;
    let slots = live(&sk);
    let names = entity_ids(&doc);
    for (n, slot) in slots.iter().enumerate() {
        assert_eq!(kind(&replayed.entities()[n]), kind(&sk.entities()[*slot]), "dense id {n} is slot {slot}");
        assert_eq!(names[n], format!("e{slot}"));
    }
}

/// A prefixed namer, the journal's shape for a second sketch.
struct Prefixed(&'static str);

impl SketchNames for Prefixed {
    fn sketch(&self) -> String {
        self.0.to_string()
    }
    fn entity(&self, slot: EntityId) -> String {
        format!("{}e{}", self.0, slot.0)
    }
    fn constraint(&self, id: ConstraintId) -> String {
        format!("{}k{}", self.0, id.0)
    }
}

#[test]
fn every_id_comes_from_the_names() {
    let (sk, issued) = sketch();
    let doc = doc_of(&sk, issued, &Prefixed("s2"));
    for s in &doc.stmts {
        let id = s.id().expect("a sketch statement has an id");
        assert!(id.starts_with("s2"), "{id}");
        if let Stmt::Point { sketch, .. } | Stmt::Line { sketch, .. } | Stmt::Constraint { sketch, .. } = s {
            assert_eq!(sketch, "s2");
        }
    }
    assert!(matches!(&doc.stmts[1], Stmt::Point { id, .. } if id == "s2e0"));
    round_trip(&sk, &doc);
}
