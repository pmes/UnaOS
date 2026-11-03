// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `photo` — Vug's headless photo face (VUGPHOTOGRID, LEDGER SR503).
//!
//! CHARTER: Vug (CODEX §2, 3D) — headless face: a photo in, photo_core's measurement out as a doc_core document;
//! no logic of its own (the measurement is photo_core's, the document's text doc_core's).
//!
//! ```text
//! photo <in.png | in.jpg> --pitch <mm> -o <out.vugdoc> [--thickness <mm>] [--report]
//! ```
//!
//! The photo is decoded by pixel_core (PNG / JPEG / BMP / QOI / GIF / WebP), measured by `photo_core::measure`
//! (pitch default 5 mm; the sketch at `Tol::DEFAULT`) and written as a vug-doc: `sketch s1` on the XY plane, one
//! statement per sketch_core entity (`p<n>` points, `l<n>` lines, `a<n>` arcs, `c<n>` circles, n the entity index)
//! and per constraint (`k<n>`), and with `--thickness` an `extrude e1` of the profile along +Z. The face writes the
//! statements itself (VUG-DOC-FROMSKETCH owes the writer in doc_core). stdout carries one line:
//! `photo: w= h= grid=<n>x<m> pitch= residual_px= sigma_mm= outline=<n> holes=<k> primitives=<lines>/<arcs>/<circles> dof=`;
//! `--report` adds the residual / tilt / lens line and one line per primitive. A refusal prints
//! `photo: refused <Name> {..}` and exits 1; usage exits 2.

use std::fmt::Write as _;
use std::process::ExitCode;

use doc_core::{Cons, Doc, PRef, Stmt};
use photo_core::{Measured, PhotoParams, Prim, Tol, measure};
use sketch_core::{Constraint, ConstraintId, Entity, EntityId, PointRef};

const USAGE: &str = "usage: photo <in.png|jpg> --pitch <mm> -o <out.vugdoc> [--thickness <mm>] [--report]";

struct Args {
    input: String,
    output: String,
    pitch: f64,
    thickness: Option<f64>,
    report: bool,
}

fn parse(argv: &[String]) -> Option<Args> {
    let (mut input, mut output, mut pitch, mut thickness, mut report) = (None, None, 5.0, None, false);
    let mut i = 0;
    while i < argv.len() {
        match argv[i].as_str() {
            "--pitch" => {
                pitch = argv.get(i + 1)?.parse().ok()?;
                i += 1;
            }
            "-o" => {
                output = Some(argv.get(i + 1)?.clone());
                i += 1;
            }
            "--thickness" => {
                let t: f64 = argv.get(i + 1)?.parse().ok()?;
                if !(t.is_finite() && t > 0.0) {
                    return None;
                }
                thickness = Some(t);
                i += 1;
            }
            "--report" => report = true,
            s if !s.starts_with('-') && input.is_none() => input = Some(s.to_string()),
            _ => return None,
        }
        i += 1;
    }
    Some(Args { input: input?, output: output?, pitch, thickness, report })
}

fn name(sk: &sketch_core::Sketch, id: EntityId) -> String {
    let p = match sk.entity(id) {
        Some(Entity::Point { .. }) => 'p',
        Some(Entity::Line { .. }) => 'l',
        Some(Entity::Arc { .. }) => 'a',
        Some(Entity::Circle { .. }) => 'c',
        _ => 'x',
    };
    format!("{p}{}", id.0)
}

fn pref(sk: &sketch_core::Sketch, r: PointRef) -> PRef {
    match r {
        PointRef::Point(e) => PRef::Point(name(sk, e)),
        PointRef::Centre(e) => PRef::Centre(name(sk, e)),
        PointRef::Start(e) => PRef::Start(name(sk, e)),
        PointRef::End(e) => PRef::End(name(sk, e)),
    }
}

fn cons(sk: &sketch_core::Sketch, c: &Constraint) -> Cons {
    let n = |e: &EntityId| name(sk, *e);
    let p = |r: &PointRef| pref(sk, *r);
    match c {
        Constraint::Coincident(a, b) => Cons::Coincident(p(a), p(b)),
        Constraint::PointOnLine(a, l) => Cons::PointOnLine(p(a), n(l)),
        Constraint::PointOnCircle(a, l) => Cons::PointOnCircle(p(a), n(l)),
        Constraint::Horizontal(l) => Cons::Horizontal(n(l)),
        Constraint::Vertical(l) => Cons::Vertical(n(l)),
        Constraint::Parallel(a, b) => Cons::Parallel(n(a), n(b)),
        Constraint::Perpendicular(a, b) => Cons::Perpendicular(n(a), n(b)),
        Constraint::TangentLineCircle(a, b) => Cons::TangentLineCircle(n(a), n(b)),
        Constraint::TangentCircles(a, b) => Cons::TangentCircles(n(a), n(b)),
        Constraint::EqualLength(a, b) => Cons::EqualLength(n(a), n(b)),
        Constraint::EqualRadius(a, b) => Cons::EqualRadius(n(a), n(b)),
        Constraint::Distance(a, b, d) => Cons::Distance(p(a), p(b), *d),
        Constraint::DistancePointLine(a, l, d) => Cons::DistancePointLine(p(a), n(l), *d),
        Constraint::Angle(a, b, v) => Cons::Angle(n(a), n(b), *v),
        Constraint::Radius(a, v) => Cons::Radius(n(a), *v),
        Constraint::Diameter(a, v) => Cons::Diameter(n(a), *v),
        Constraint::Fix(a, v) => Cons::Fix(p(a), [v.x, v.y]),
        Constraint::Midpoint(a, l) => Cons::Midpoint(p(a), n(l)),
        Constraint::Symmetric(a, b, l) => Cons::Symmetric(p(a), p(b), n(l)),
    }
}

/// The measurement as a vug-doc.
fn document(m: &Measured, thickness: Option<f64>) -> Doc {
    let sk = &m.sketch;
    let x = sk.params();
    let s = "s1".to_string();
    let mut doc = Doc::new(Tol::DEFAULT);
    doc.stmts.push(Stmt::Sketch { id: s.clone(), o: [0.0, 0.0, 0.0], z: [0.0, 0.0, 1.0], x: [1.0, 0.0, 0.0] });
    for (i, e) in sk.entities().iter().enumerate() {
        let id = EntityId(i as u32);
        let me = name(sk, id);
        let st = match *e {
            Entity::Point { x: k } => Stmt::Point { id: me, sketch: s.clone(), at: [x[k], x[k + 1]] },
            Entity::Line { a, b } => Stmt::Line { id: me, sketch: s.clone(), a: name(sk, a), b: name(sk, b) },
            Entity::Circle { centre, r } => Stmt::Circle { id: me, sketch: s.clone(), centre: name(sk, centre), r: x[r] },
            Entity::Arc { centre, r } => Stmt::Arc {
                id: me,
                sketch: s.clone(),
                centre: name(sk, centre),
                r: x[r],
                start: x[r + 1],
                end: x[r + 2],
            },
            Entity::Removed => continue,
        };
        doc.stmts.push(st);
    }
    let mut k = 0u32;
    while let Some(c) = sk.constraint(ConstraintId(k)) {
        doc.stmts.push(Stmt::Constraint { id: format!("k{k}"), sketch: s.clone(), c: cons(sk, c) });
        k += 1;
    }
    if let (Some(t), Some(p)) = (thickness, m.profiles.first()) {
        if let Some(e) = p.outer.edges.first() {
            doc.stmts.push(Stmt::Extrude { id: "e1".into(), sketch: s, outer: name(sk, e.src()), dir: [0.0, 0.0, 1.0], length: t });
        }
    }
    doc
}

fn report(m: &Measured, w: u32, h: u32) -> String {
    let (nl, na, nc) = m.shape.counts();
    format!(
        "photo: w={w} h={h} grid={}x{} pitch={} residual_px={:.3} sigma_mm={:.3} outline={} holes={} primitives={nl}/{na}/{nc} dof={}",
        m.grid.0,
        m.grid.1,
        m.pitch_mm,
        m.residual_px,
        m.sigma_mm,
        m.outline.len(),
        m.holes.len(),
        m.solved.dof
    )
}

fn detail(m: &Measured) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "photo: residual_mm={:.4} tilt_deg={:.2} lens_hint={:.4} line_width_mm={:.3} pitch_px={:.2} intersections={} inliers={} rect_px_per_mm={}",
        m.residual_mm, m.tilt_deg, m.lens_hint, m.line_width_mm, m.pitch_px, m.intersections, m.inliers, m.rect_px_per_mm
    );
    for (k, lp) in std::iter::once(&m.shape.outer).chain(m.shape.holes.iter()).enumerate() {
        for p in lp {
            let _ = match *p {
                Prim::Line { a, b, rms } => writeln!(
                    s,
                    "photo: loop={k} line a=({:.3},{:.3}) b=({:.3},{:.3}) len={:.3} rms={:.4}",
                    a.x, a.y, b.x, b.y, a.dist(b), rms
                ),
                Prim::Arc { centre, r, ccw, tangent_in, tangent_out, rms, .. } => writeln!(
                    s,
                    "photo: loop={k} arc c=({:.3},{:.3}) r={:.3} ccw={ccw} tangent={tangent_in}/{tangent_out} rms={:.4}",
                    centre.x, centre.y, r, rms
                ),
                Prim::Circle { centre, r, rms } => writeln!(
                    s,
                    "photo: loop={k} circle c=({:.3},{:.3}) d={:.3} rms={:.4}",
                    centre.x, centre.y, 2.0 * r, rms
                ),
            };
        }
    }
    s
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let Some(a) = parse(&argv) else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let bytes = match std::fs::read(&a.input) {
        Ok(b) => b,
        Err(e) => {
            println!("photo: refused Read {{ path: {:?}, error: {:?} }}", a.input, e.kind());
            return ExitCode::from(1);
        }
    };
    let img = match pixel_core::decode(&bytes) {
        Ok(i) => i,
        Err(e) => {
            println!("photo: refused Decode {{ error: {e:?} }}");
            return ExitCode::from(1);
        }
    };
    let m = match measure(&img, PhotoParams::new(a.pitch), Tol::DEFAULT) {
        Ok(m) => m,
        Err(r) => {
            println!("photo: refused {r:?}");
            return ExitCode::from(1);
        }
    };
    let text = doc_core::write(&document(&m, a.thickness));
    if let Err(e) = std::fs::write(&a.output, text) {
        println!("photo: refused Write {{ path: {:?}, error: {:?} }}", a.output, e.kind());
        return ExitCode::from(1);
    }
    println!("{}", report(&m, img.width, img.height));
    if a.report {
        print!("{}", detail(&m));
    }
    ExitCode::SUCCESS
}
