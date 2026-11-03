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
//! (pitch default 5 mm; the sketch at `Tol::DEFAULT`) and written as a vug-doc: `sketch s` on the XY plane, one
//! statement per sketch_core entity (`e<n>`, n the entity slot) and per constraint (`k<n>`), and with `--thickness`
//! an `extrude x0` of the profile along +Z. The sketch's statements are doc_core's one writer, `from_sketch` with
//! `PlainNames` (VUGDOCFROMSKETCH, SR516: the face's names, one namespace). stdout carries one line:
//! `photo: w= h= grid=<n>x<m> pitch= residual_px= sigma_mm= outline=<n> holes=<k> primitives=<lines>/<arcs>/<circles> dof=`;
//! `--report` adds the residual / tilt / lens line and one line per primitive. A refusal prints
//! `photo: refused <Name> {..}` and exits 1; usage exits 2.

use std::fmt::Write as _;
use std::process::ExitCode;

use doc_core::{Doc, PlainNames, SketchNames, Stmt};
use photo_core::{Measured, PhotoParams, Prim, Tol, measure};
use sketch_core::ConstraintId;

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

/// The measurement as a vug-doc: the sketch through doc_core's `from_sketch` (`PlainNames`), then the extrude.
fn document(m: &Measured, thickness: Option<f64>) -> Doc {
    let sk = &m.sketch;
    // photo_core's sketch is fresh (no constraint was ever removed): the issued constraints are the dense run from 0
    // (VUG-SKETCH-ISSUED owes `Sketch::issued`).
    let issued = (0u32..).take_while(|k| sk.constraint(ConstraintId(*k)).is_some()).count() as u32;
    let mut doc = Doc::new(Tol::DEFAULT);
    doc.stmts = doc_core::from_sketch(sk, ([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]), issued, &PlainNames);
    if let (Some(t), Some(p)) = (thickness, m.profiles.first()) {
        if let Some(e) = p.outer.edges.first() {
            let outer = PlainNames.entity(e.src());
            doc.stmts.push(Stmt::Extrude { id: "x0".into(), sketch: PlainNames.sketch(), outer, dir: [0.0, 0.0, 1.0], length: t });
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
