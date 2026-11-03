// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vug (CODEX §2, 3D), a host face — the edge tool (VUGFACEFILLET, LEDGER SR511): an edge of the body picked
//! under the cursor, a typed radius (or two chamfer distances), brep_core's `fillet` / `chamfer` on Enter; holds no
//! geometry (the Breps are the design's Body, re-tessellated through extrude.rs `Body::from_breps`).
//!
//! The pick: the cursor ray's triangle (`View::hit` on the shown model) names its brep face (`Body::face_of`); every
//! edge of that face's loops is projected (`View::project_f64`: a line by its two vertices, a curve by 16 samples of
//! `Curve::point`) and the nearest within `SNAP_PX` (8 px) is the edge, kept by its `Id` and its persistent `Name` (the
//! Name is what doc_core's `fillet` statement writes). `f` opens Fillet at the edge under the cursor, Shift+f (`F`)
//! Chamfer; a click re-picks; the value is typed through the SR494 road (sketch.rs `type_into`: digits, one `.`, a
//! leading `-`, Backspace), a chamfer taking `d1,d2` (one value for both); Enter commits on the Body's Brep and
//! re-tessellates, Esc cancels. A refusal is shown by name at the edge (`RadiusTooLarge max <r>`), at the vertex
//! (`CornerNotBlended`), the body unchanged and the tool left open. The face writes no statement: the statement log
//! is VUG-FACE-FEATURE-TREE (SR513).

use crate::extrude::Body;
use crate::model::Model;
use crate::raster::{self, Frame as Raster, rgba};
use crate::sketch::{ACTIVE, REFUSED, SNAP_PX, fmt_value, refusal_word, type_into};
use crate::view::{Action, Button, View};
use brep_core::{Brep, Curve, Entity, Id, Name, Refusal};

/// Samples per curved edge for the pick and the highlight.
const SAMPLES: usize = 16;

/// Fillet (`f`) or chamfer (Shift+f).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Fillet,
    Chamfer,
}

/// The picked edge: the Brep it is in (an index in `Body::breps`), its Id and persistent Name, its points (world mm)
/// for the highlight.
#[derive(Clone, Debug, PartialEq)]
pub struct Picked {
    pub brep: u32,
    pub edge: Id,
    pub name: Name,
    pub pts: Vec<[f64; 3]>,
}

/// A refusal shown at a world point.
#[derive(Clone, Debug, PartialEq)]
pub struct Note {
    pub text: String,
    pub at: [f64; 3],
}

/// The open edge tool: its mode, the edge (none until one is picked), the typed text, the last refusal.
#[derive(Clone, Debug, PartialEq)]
pub struct EdgeTool {
    pub mode: Mode,
    pub edge: Option<Picked>,
    pub buffer: String,
    pub note: Option<Note>,
}

fn xyz(p: brep_core::Vec3) -> [f64; 3] {
    [p.x, p.y, p.z]
}

/// An edge as points: a line by its two vertices, any other curve sampled over [t0, t1].
fn edge_points(b: &Brep, e: u32) -> Vec<[f64; 3]> {
    let ed = &b.edges[e as usize];
    match ed.curve {
        Curve::Line { .. } => vec![xyz(b.vertices[ed.v[0] as usize].p), xyz(b.vertices[ed.v[1] as usize].p)],
        _ => (0..=SAMPLES).map(|k| xyz(ed.curve.point(ed.t0 + (ed.t1 - ed.t0) * k as f64 / SAMPLES as f64))).collect(),
    }
}

/// The middle of an edge's points (a line's midpoint, a curve's middle sample).
fn midpoint(pts: &[[f64; 3]]) -> [f64; 3] {
    match pts.len() {
        0 => [0.0; 3],
        2 => [0, 1, 2].map(|k| 0.5 * (pts[0][k] + pts[1][k])),
        n => pts[n / 2],
    }
}

/// The distance in pixels from `p` to the segment a-b.
fn seg_dist(p: (f64, f64), a: (f64, f64), b: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l2 = dx * dx + dy * dy;
    let t = if l2 > 0.0 { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / l2).clamp(0.0, 1.0) } else { 0.0 };
    (p.0 - a.0 - t * dx).hypot(p.1 - a.1 - t * dy)
}

/// The edge under (x, y): the brep face of the triangle the ray hits on `model` (the shown model, the body's), then
/// the nearest edge of that face's loops within `SNAP_PX` on the surface.
pub fn edge_at(view: &View, model: &Model, body: &Body, x: f64, y: f64) -> Option<Picked> {
    let (_, tri) = view.hit(model, x, y)?;
    let &(k, fid) = body.face_of.get(tri as usize)?;
    let b = body.breps.get(k as usize)?;
    let Some(Entity::Face(fi)) = b.find(fid) else { return None };
    let mut best: Option<(f64, u32)> = None;
    for l in b.face_loops(fi) {
        for &ce in &b.loops[l as usize].coedges {
            let e = b.coedges[ce as usize].edge;
            let scr: Option<Vec<(f64, f64)>> = edge_points(b, e).into_iter().map(|p| view.project_f64(p)).collect();
            let Some(scr) = scr else { continue };
            let d = scr.windows(2).map(|w| seg_dist((x, y), w[0], w[1])).fold(f64::INFINITY, f64::min);
            if d <= SNAP_PX && best.map_or(true, |(bd, _)| d < bd) {
                best = Some((d, e));
            }
        }
    }
    let (_, e) = best?;
    let ed = &b.edges[e as usize];
    Some(Picked { brep: k, edge: ed.id, name: ed.name, pts: edge_points(b, e) })
}

/// A brep_core refusal as a note: `RadiusTooLarge max <r>` at the edge, `CornerNotBlended` at its vertex, any other
/// by its name at the edge.
fn refusal_note(r: &Refusal, b: &Brep, e: &Picked) -> Note {
    let mid = midpoint(&e.pts);
    match *r {
        Refusal::RadiusTooLarge { max, .. } => Note { text: format!("RadiusTooLarge max {}", fmt_value(max)), at: mid },
        Refusal::CornerNotBlended { vertex } => {
            let at = match b.find(vertex) {
                Some(Entity::Vertex(i)) => xyz(b.vertices[i as usize].p),
                _ => mid,
            };
            Note { text: "CornerNotBlended".to_string(), at }
        }
        _ => Note { text: refusal_word(&format!("{r:?}")), at: mid },
    }
}

/// The view's key, first (before the sketch face): `f` / `F` open the tool at the edge under the cursor (with a body
/// and no pull); while it is open, the typed value, `,` (chamfer), Backspace, Enter, Esc. Other keys fall through.
pub fn key(view: &mut View, model: &Model, k: &str) -> Option<bool> {
    if matches!(k, "f" | "F") {
        if view.design.pull.is_some() {
            return None;
        }
        let body = view.design.body.as_ref()?;
        let (x, y) = view.cursor;
        let edge = edge_at(view, model, body, x, y);
        let mode = if k == "F" { Mode::Chamfer } else { Mode::Fillet };
        view.fillet = Some(EdgeTool { mode, edge, buffer: String::new(), note: None });
        return Some(true);
    }
    let tool = view.fillet.as_mut()?;
    if matches!(k, "Escape" | "\u{1b}") {
        view.fillet = None;
        return Some(true);
    }
    if matches!(k, "Return" | "KP_Enter" | "\r" | "\n") {
        commit(view);
        return Some(true);
    }
    let back = matches!(k, "BackSpace" | "Backspace" | "\u{8}" | "\u{7f}");
    let ch = {
        let mut it = k.chars();
        match (it.next(), it.next()) {
            (Some(c), None) => Some(c),
            _ => None,
        }
    };
    if ch == Some(',') {
        if tool.mode == Mode::Chamfer && !tool.buffer.is_empty() && !tool.buffer.contains(',') {
            tool.buffer.push(',');
        }
        tool.note = None;
        return Some(true);
    }
    if back && tool.buffer.ends_with(',') {
        tool.buffer.pop();
        tool.note = None;
        return Some(true);
    }
    // The SR494 road on the value after the comma (the whole text for a fillet).
    let at = tool.buffer.rfind(',').map_or(0, |i| i + 1);
    let mut seg = tool.buffer[at..].to_string();
    if !type_into(&mut seg, ch, back) {
        return None;
    }
    tool.buffer.truncate(at);
    tool.buffer.push_str(&seg);
    tool.note = None;
    Some(true)
}

/// The view's press, first: with the tool open, a bare pick re-picks the edge under the cursor (consumed).
pub fn press(view: &mut View, model: &Model, button: Button, mods: u8, x: f64, y: f64) -> Option<bool> {
    view.fillet.as_ref()?;
    if view.preset.resolve(button, mods) != Action::Pick {
        return None;
    }
    let edge = view.design.body.as_ref().and_then(|b| edge_at(view, model, b, x, y));
    let tool = view.fillet.as_mut()?;
    let changed = edge.is_some() && edge != tool.edge;
    if edge.is_some() {
        tool.edge = edge;
        tool.note = None;
    }
    Some(changed)
}

/// Enter: parse r (or d1[,d2]), run brep_core's `fillet` / `chamfer` on the picked edge's Brep, and on success put the
/// re-tessellated Body in place (the tool closes, the stale triangle pick clears); a refusal is a note, the body kept.
fn commit(view: &mut View) {
    let tol = geom_core::Tol::DEFAULT;
    let Some(tool) = view.fillet.as_ref() else { return };
    let Some(edge) = tool.edge.clone() else { return };
    let vals: Option<Vec<f64>> = tool.buffer.split(',').map(|s| s.parse::<f64>().ok()).collect();
    let Some(vals) = vals.filter(|v| !v.is_empty() && v.len() <= 2) else { return };
    let mode = tool.mode;
    let outcome: Result<Body, Note> = {
        let Some(body) = view.design.body.as_ref() else { return };
        let Some(src) = body.breps.get(edge.brep as usize) else { return };
        let got = match mode {
            Mode::Fillet => brep_core::fillet(src, &[edge.edge], vals[0], tol),
            Mode::Chamfer => brep_core::chamfer(src, &[edge.edge], vals[0], *vals.get(1).unwrap_or(&vals[0]), tol),
        };
        match got {
            Ok(nb) => {
                let mut breps = body.breps.clone();
                breps[edge.brep as usize] = nb;
                Body::from_breps(breps, tol).map_err(|e| Note { text: e, at: midpoint(&edge.pts) })
            }
            Err(r) => {
                log::warn!("vug-view: {mode:?} refused: {r:?}");
                Err(refusal_note(&r, src, &edge))
            }
        }
    };
    match outcome {
        Ok(b) => {
            view.design.body = Some(b);
            view.picked = None;
            view.fillet = None;
        }
        Err(n) => {
            if let Some(t) = view.fillet.as_mut() {
                t.note = Some(n);
            }
        }
    }
}

/// The edge in the accent (two pixels wide), the typed value at it, a refusal in the refusal colour.
pub fn overlay(view: &View, fr: &mut Raster) {
    let Some(tool) = view.fillet.as_ref() else { return };
    let scr = |p: [f64; 3]| view.project_f64(p).map(|(x, y)| (x as f32, y as f32));
    let col = rgba(ACTIVE);
    match &tool.edge {
        Some(e) => {
            let pts: Option<Vec<(f32, f32)>> = e.pts.iter().map(|&p| scr(p)).collect();
            if let Some(pts) = pts {
                for w in pts.windows(2) {
                    raster::line(fr, w[0], w[1], col);
                    raster::line(fr, (w[0].0, w[0].1 + 1.0), (w[1].0, w[1].1 + 1.0), col);
                }
            }
            if let Some((x, y)) = scr(midpoint(&e.pts)) {
                let label = match tool.mode {
                    Mode::Fillet => format!("r={}", tool.buffer),
                    Mode::Chamfer => format!("d={}", tool.buffer),
                };
                raster::text(fr, &label, x + 6.0, y - 14.0, col);
            }
        }
        None => raster::text(fr, "pick an edge", 6.0, 20.0, col),
    }
    if let Some(n) = &tool.note {
        if let Some((x, y)) = scr(n.at) {
            raster::text(fr, &n.text, x + 6.0, y + 4.0, rgba(REFUSED));
        }
    }
}
