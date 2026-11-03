// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Replay: the document through the cores. sketch_core solves each sketch, brep_core builds each feature's solid,
//! make_core generates the Program. Nothing is dropped: a statement that fails to regenerate is reported by its line
//! ([`Failure`]) and whatever depends on it fails as `Upstream`, naming the statement it waited on.

use alloc::collections::{BTreeMap, BTreeSet};
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use brep_core::{Brep, Frame};
use make_core::{Machine, Move, Program};
use sketch_core::{BadRef, Constraint, ConstraintId, EntityId, PointRef, Sketch, Solved};

use crate::{Cons, Doc, PRef, Stmt, Tol};

/// A solved sketch, by its statement id.
#[derive(Clone, Debug)]
pub struct SketchOut {
    pub id: String,
    pub sketch: Sketch,
    pub solved: Solved,
}

/// A feature's solid, by its statement id.
#[derive(Clone, Debug)]
pub struct SolidOut {
    pub id: String,
    pub brep: Brep,
}

/// Why a statement did not regenerate.
#[derive(Clone, Debug, PartialEq)]
pub enum Fault {
    /// sketch_core refused the reference as the wrong kind of entity (`Constraint::check` / `Sketch::add_*`): the
    /// text id it named.
    BadRef(String),
    /// sketch_core names this constraint among those that cannot all hold.
    Conflicting,
    /// The sketch's solve did not converge.
    Diverged,
    /// No profile of the solved sketch has the `outer` entity in its outer loop.
    NoProfile,
    /// It depends on a statement that failed (that statement's id).
    Upstream(String),
    Brep(brep_core::Refusal),
    Make(make_core::Refusal),
    /// An Op in a document with no machine.
    NoMachine,
    // VUGFACEFILLET (SR511): an edge name of a fillet / chamfer that does not resolve on the replayed solid.
    /// No edge of the `on` solid carries this name (its four words): the reference no longer resolves.
    NoSuchEdge(String),
    /// More than one edge of the `on` solid carries this name (its four words).
    AmbiguousEdge(String),
}

/// A failing statement: its line, its id (empty for none) and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Failure {
    pub line: u32,
    pub id: String,
    pub fault: Fault,
}

/// What replay regenerated, and every statement that did not (sorted by line).
#[derive(Clone, Debug, Default)]
pub struct Replay {
    pub sketches: Vec<SketchOut>,
    pub solids: Vec<SolidOut>,
    /// Some only when the document has a machine, at least one Op, and EVERY Op generated (a Program missing an Op is
    /// not the design's Make; the failures name each).
    pub program: Option<Program>,
    pub failures: Vec<Failure>,
}

/// One sketch under construction.
struct Work {
    id: String,
    line: u32,
    frame: ([f64; 3], [f64; 3], [f64; 3]),
    sk: Sketch,
    ents: BTreeMap<String, EntityId>,
    cons: Vec<(ConstraintId, usize)>,
    solved: Option<Solved>,
}

impl Work {
    fn name_of(&self, e: EntityId) -> String {
        self.ents.iter().find(|(_, v)| **v == e).map(|(k, _)| k.clone()).unwrap_or_default()
    }
}

fn make_tol(t: Tol) -> make_core::Tol {
    make_core::Tol { lin: t.lin, ang: t.ang }
}

fn bv2(x: f64, y: f64) -> brep_core::Vec2 {
    brep_core::Vec2::new(x, y)
}

fn bv3(v: [f64; 3]) -> brep_core::Vec3 {
    brep_core::Vec3::new(v[0], v[1], v[2])
}

/// THE seam from sketch_core's profile to the one brep_core takes: since VUGBREPSWAP (SR492) brep_core takes
/// sketch_core's `Profile` itself, so this is the identity (kept public for callers that named it).
pub fn brep_profile(p: &sketch_core::Profile) -> brep_core::Profile {
    p.clone()
}

/// A text point reference to sketch_core's, or the id it waited on.
fn pref(p: &PRef, ents: &BTreeMap<String, EntityId>) -> Result<PointRef, String> {
    let get = |id: &String| ents.get(id).copied().ok_or_else(|| id.clone());
    Ok(match p {
        PRef::Point(id) => PointRef::Point(get(id)?),
        PRef::Centre(id) => PointRef::Centre(get(id)?),
        PRef::Start(id) => PointRef::Start(get(id)?),
        PRef::End(id) => PointRef::End(get(id)?),
    })
}

/// A text constraint to sketch_core's, or the id it waited on.
fn constraint(c: &Cons, ents: &BTreeMap<String, EntityId>) -> Result<Constraint, String> {
    let e = |id: &String| ents.get(id).copied().ok_or_else(|| id.clone());
    let v2 = |a: &[f64; 2]| sketch_core::Vec2::new(a[0], a[1]);
    Ok(match c {
        Cons::Coincident(a, b) => Constraint::Coincident(pref(a, ents)?, pref(b, ents)?),
        Cons::PointOnLine(p, l) => Constraint::PointOnLine(pref(p, ents)?, e(l)?),
        Cons::PointOnCircle(p, k) => Constraint::PointOnCircle(pref(p, ents)?, e(k)?),
        Cons::Horizontal(l) => Constraint::Horizontal(e(l)?),
        Cons::Vertical(l) => Constraint::Vertical(e(l)?),
        Cons::Parallel(a, b) => Constraint::Parallel(e(a)?, e(b)?),
        Cons::Perpendicular(a, b) => Constraint::Perpendicular(e(a)?, e(b)?),
        Cons::TangentLineCircle(l, k) => Constraint::TangentLineCircle(e(l)?, e(k)?),
        Cons::TangentCircles(a, b) => Constraint::TangentCircles(e(a)?, e(b)?),
        Cons::EqualLength(a, b) => Constraint::EqualLength(e(a)?, e(b)?),
        Cons::EqualRadius(a, b) => Constraint::EqualRadius(e(a)?, e(b)?),
        Cons::Distance(a, b, d) => Constraint::Distance(pref(a, ents)?, pref(b, ents)?, *d),
        Cons::DistancePointLine(p, l, d) => Constraint::DistancePointLine(pref(p, ents)?, e(l)?, *d),
        Cons::Angle(a, b, t) => Constraint::Angle(e(a)?, e(b)?, *t),
        Cons::Radius(k, r) => Constraint::Radius(e(k)?, *r),
        Cons::Diameter(k, d) => Constraint::Diameter(e(k)?, *d),
        Cons::Fix(p, at) => Constraint::Fix(pref(p, ents)?, v2(at)),
        Cons::Midpoint(p, l) => Constraint::Midpoint(pref(p, ents)?, e(l)?),
        Cons::Symmetric(a, b, l) => Constraint::Symmetric(pref(a, ents)?, pref(b, ents)?, e(l)?),
    })
}

fn fail(r: &mut Replay, line: u32, id: &str, fault: Fault) {
    r.failures.push(Failure { line, id: id.to_string(), fault });
}

/// Regenerate the document under `tol` (the header's `doc.tol`, or a tighter one to re-check).
pub fn replay(doc: &Doc, tol: Tol) -> Replay {
    let mut r = Replay::default();

    // 1. Sketches: entities and constraints in document order.
    let mut work: Vec<Work> = Vec::new();
    let mut by_id: BTreeMap<String, usize> = BTreeMap::new();
    for (i, s) in doc.stmts.iter().enumerate() {
        let line = doc.line(i);
        let w = |sketch: &String, by_id: &BTreeMap<String, usize>| by_id.get(sketch).copied();
        match s {
            Stmt::Sketch { id, o, z, x } => {
                by_id.insert(id.clone(), work.len());
                work.push(Work {
                    id: id.clone(),
                    line,
                    frame: (*o, *z, *x),
                    sk: Sketch::new(),
                    ents: BTreeMap::new(),
                    cons: Vec::new(),
                    solved: None,
                });
            }
            Stmt::Point { id, sketch, at } => {
                if let Some(k) = w(sketch, &by_id) {
                    let e = work[k].sk.add_point(at[0], at[1]);
                    work[k].ents.insert(id.clone(), e);
                }
            }
            Stmt::Line { id, sketch, a, b } => {
                let Some(k) = w(sketch, &by_id) else { continue };
                let wk = &mut work[k];
                let (ea, eb) = match (wk.ents.get(a), wk.ents.get(b)) {
                    (Some(ea), Some(eb)) => (*ea, *eb),
                    (None, _) => {
                        fail(&mut r, line, id, Fault::Upstream(a.clone()));
                        continue;
                    }
                    (_, None) => {
                        fail(&mut r, line, id, Fault::Upstream(b.clone()));
                        continue;
                    }
                };
                match wk.sk.add_line(ea, eb) {
                    Ok(e) => {
                        wk.ents.insert(id.clone(), e);
                    }
                    Err(BadRef(e)) => {
                        let n = wk.name_of(e);
                        fail(&mut r, line, id, Fault::BadRef(n));
                    }
                }
            }
            Stmt::Circle { id, sketch, centre, r: rad } | Stmt::Arc { id, sketch, centre, r: rad, .. } => {
                let Some(k) = w(sketch, &by_id) else { continue };
                let wk = &mut work[k];
                let Some(ec) = wk.ents.get(centre).copied() else {
                    fail(&mut r, line, id, Fault::Upstream(centre.clone()));
                    continue;
                };
                let got = match s {
                    Stmt::Arc { start, end, .. } => wk.sk.add_arc(ec, *rad, *start, *end),
                    _ => wk.sk.add_circle(ec, *rad),
                };
                match got {
                    Ok(e) => {
                        wk.ents.insert(id.clone(), e);
                    }
                    Err(BadRef(e)) => {
                        let n = wk.name_of(e);
                        fail(&mut r, line, id, Fault::BadRef(n));
                    }
                }
            }
            Stmt::Constraint { id, sketch, c } => {
                let Some(k) = w(sketch, &by_id) else { continue };
                let wk = &mut work[k];
                match constraint(c, &wk.ents) {
                    Err(missing) => fail(&mut r, line, id, Fault::Upstream(missing)),
                    Ok(sc) => match wk.sk.add(sc) {
                        Ok(cid) => wk.cons.push((cid, i)),
                        Err(BadRef(e)) => {
                            let n = wk.name_of(e);
                            fail(&mut r, line, id, Fault::BadRef(n));
                        }
                    },
                }
            }
            _ => {}
        }
    }

    // 2. Solve each sketch; a refusal is named on the constraints sketch_core names, or on the sketch.
    for wk in work.iter_mut() {
        match sketch_core::solve(&mut wk.sk, tol) {
            Ok(s) => wk.solved = Some(s),
            Err(sketch_core::Refusal::Conflicting(ids)) => {
                for cid in ids {
                    if let Some((_, i)) = wk.cons.iter().find(|(c, _)| *c == cid) {
                        let id = doc.stmts[*i].id().unwrap_or("");
                        fail(&mut r, doc.line(*i), id, Fault::Conflicting);
                    }
                }
            }
            Err(_) => {
                fail(&mut r, wk.line, &wk.id, Fault::Diverged);
            }
        }
    }

    // 3. Features, in document order.
    let bt = tol; // VUGBREPSWAP (SR492): the document's Tol is brep_core's (geom_core's one Tol).
    let mut built: BTreeSet<String> = BTreeSet::new();
    for (i, s) in doc.stmts.iter().enumerate() {
        let line = doc.line(i); if edit_step(&mut r, &mut built, s, line, bt) { continue; } // VUGFACEFILLET (SR511): fillet / chamfer after their feature
        let (id, sketch, outer) = match s {
            Stmt::Extrude { id, sketch, outer, .. } | Stmt::Revolve { id, sketch, outer, .. } => (id, sketch, outer),
            _ => continue,
        };
        let Some(wk) = by_id.get(sketch).map(|k| &work[*k]) else { continue };
        if wk.solved.is_none() {
            fail(&mut r, line, id, Fault::Upstream(sketch.clone()));
            continue;
        }
        let Some(eo) = wk.ents.get(outer).copied() else {
            fail(&mut r, line, id, Fault::Upstream(outer.clone()));
            continue;
        };
        let Some(p) =
            sketch_core::profiles(&wk.sk, tol).into_iter().find(|p| p.outer.edges.iter().any(|e| e.src() == eo))
        else {
            fail(&mut r, line, id, Fault::NoProfile);
            continue;
        };
        let (o, z, x) = wk.frame;
        let frame = match Frame::new(bv3(o), bv3(z), bv3(x)) {
            Ok(f) => f,
            Err(e) => {
                fail(&mut r, line, id, Fault::Brep(e));
                continue;
            }
        };
        let got = match s {
            Stmt::Extrude { dir, length, .. } => brep_core::extrude(&p, &frame, bv3(*dir), *length, bt),
            Stmt::Revolve { axis_o, axis_d, angle, .. } => brep_core::revolve(
                &p,
                &frame,
                bv2(axis_o[0], axis_o[1]),
                bv2(axis_d[0], axis_d[1]),
                *angle,
                bt,
            ),
            _ => continue,
        };
        match got {
            Ok(brep) => {
                built.insert(id.clone());
                r.solids.push(SolidOut { id: id.clone(), brep });
            }
            Err(e) => fail(&mut r, line, id, Fault::Brep(e)),
        }
    }

    // 4. The Make: the one machine, then each Op in order with the tool the previous Op left in the spindle.
    let mut machine: Option<Machine> = None;
    for s in &doc.stmts {
        match s {
            Stmt::Machine(m) => {
                machine = Some(Machine {
                    kind: m.kind,
                    kinematics: m.kinematics,
                    axes: Vec::new(),
                    power: m.power,
                    envelope: m.envelope,
                    safe_z: m.safe_z,
                    units: m.units,
                    tools: Vec::new(),
                    stock: None,
                    fixtures: Vec::new(),
                    loaded: m.loaded,
                })
            }
            Stmt::Axis(a) => machine.iter_mut().for_each(|m| m.axes.push(*a)),
            Stmt::Tool(t) => machine.iter_mut().for_each(|m| m.tools.push(*t)),
            Stmt::Stock(st) => machine.iter_mut().for_each(|m| m.stock = Some(*st)),
            Stmt::Fixture(fx) => machine.iter_mut().for_each(|m| m.fixtures.push(fx.clone())),
            _ => {}
        }
    }
    let mt = make_tol(tol);
    let mut program: Option<Program> = None;
    let mut all = true;
    let mut cur = machine.clone();
    for (i, s) in doc.stmts.iter().enumerate() {
        let Stmt::Op { id, on, op } = s else { continue };
        let line = doc.line(i);
        let Some(m) = cur.as_mut() else {
            fail(&mut r, line, id, Fault::NoMachine);
            all = false;
            continue;
        };
        if !built.contains(on) {
            fail(&mut r, line, id, Fault::Upstream(on.clone()));
            all = false;
            continue;
        }
        match make_core::generate(op, m, mt) {
            Ok(p) => {
                for mv in &p.moves {
                    if let Move::ToolChange { tool } = mv {
                        m.loaded = Some(*tool);
                    }
                }
                program.get_or_insert_with(|| Program::new(m.units)).moves.extend(p.moves);
            }
            Err(e) => {
                fail(&mut r, line, id, Fault::Make(e));
                all = false;
            }
        }
    }
    r.program = if all { program } else { None };

    // 5. The solved sketches move out last (the features above read them in place).
    for wk in work {
        if let Some(solved) = wk.solved {
            r.sketches.push(SketchOut { id: wk.id, sketch: wk.sk, solved });
        }
    }
    r.failures.sort_by_key(|f| f.line);
    r
}

// ---- VUGFACEFILLET (SR511): a fillet / chamfer replays onto its feature's solid ----

/// The fillet / chamfer statement `s` (true; any other statement is false and untouched): its `on` solid must have
/// replayed (else `Upstream(on)`); each edge `Name` resolves to the ONE edge of that solid carrying it (`NoSuchEdge` /
/// `AmbiguousEdge` by its words; brep_core has no edges-by-name lookup, VUG-BREP-EDGES-NAMED owed), and brep_core's
/// `fillet` / `chamfer` runs on the resolved Ids. The result is this statement's own solid; the `on` solid stays.
fn edit_step(r: &mut Replay, built: &mut BTreeSet<String>, s: &Stmt, line: u32, tol: Tol) -> bool {
    let (id, on, edges) = match s {
        Stmt::Fillet { id, on, edges, .. } | Stmt::Chamfer { id, on, edges, .. } => (id, on, edges),
        _ => return false,
    };
    let src = if built.contains(on) { r.solids.iter().rev().find(|o| o.id == *on).map(|o| o.brep.clone()) } else { None };
    let Some(src) = src else {
        fail(r, line, id, Fault::Upstream(on.clone()));
        return true;
    };
    let mut ids = Vec::new();
    for n in edges {
        let hits: Vec<brep_core::Id> = src.edges.iter().filter(|e| e.name == *n).map(|e| e.id).collect();
        match hits.len() {
            1 => ids.push(hits[0]),
            0 => {
                fail(r, line, id, Fault::NoSuchEdge(crate::name_words(n)));
                return true;
            }
            _ => {
                fail(r, line, id, Fault::AmbiguousEdge(crate::name_words(n)));
                return true;
            }
        }
    }
    let got = match s {
        Stmt::Fillet { r: rad, .. } => brep_core::fillet(&src, &ids, *rad, tol),
        Stmt::Chamfer { d1, d2, .. } => brep_core::chamfer(&src, &ids, *d1, *d2, tol),
        _ => return false,
    };
    match got {
        Ok(brep) => {
            built.insert(id.clone());
            r.solids.push(SolidOut { id: id.clone(), brep });
        }
        Err(e) => fail(r, line, id, Fault::Brep(e)),
    }
    true
}
