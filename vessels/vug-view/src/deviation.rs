// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vug (CODEX §2, 3D), a host face — the deviation map (VUGFACEDEVIATION, LEDGER SR525): a scan measured
//! against a design document through scan_core, drawn in the band's colours; holds no geometry.
//!
//! VUGSCANDEVIATION (SR510) measured the made part against its model in tools/scan; this is the same measurement in
//! the face, built EXACTLY as tools/scan builds it: doc_core `replay` of the document (the first failing statement
//! refuses by line, no solid refuses `DesignEmpty`), every replayed solid through scan_core's ONE builder `design_of`,
//! `measure` from the identity with `AlignParams::DEFAULT` at [`BAND_MM`] (tools/scan's default `--band`). The model
//! the face draws is the scan at the aligned pose (the design's frame); each scan vertex's colour is scan_core's ONE
//! ramp `colour(shade(d, band))` (palette_core's status rows), and raster.rs paints each triangle on the per-channel
//! mean of its three vertex colours ([`albedo`], the one arm in `tri_colour`). The corner (top-right) carries the
//! statistics and the worst three faces by rms with their signed mean; a refused registration is named there
//! (`refused NoAlignment rms= pairs=` / `refused Unconstrained level=`) and the scan is drawn as read, in the albedo.
//! `d` outside a sketch toggles the map (inside a sketch `d` stays the dimension tool: the view asks the sketch first).
//! BP-vug-5: the map is a printed residual; neither the scan nor the design changes.

use crate::model::Model;
use crate::raster::{self, Frame, rgba};
use crate::view::View;
use doc_core::Doc;
use scan_core::{
    AlignParams, Deviation, DeviationError, Design, Intrinsics, Mesh, ScanMesh, Shade, Tol, Transform, colour, design_of, measure, shade,
}; // VUGFACEDEVIATIONPICK (SR543): Shade names the picked vertex's band

/// The band (mm) the map classifies against: tools/scan's default `--band`.
pub const BAND_MM: f64 = 3.0;
/// The worst faces the corner lists.
pub const WORST: usize = 3;
/// The corner's line height, surface pixels.
pub const LINE_PX: f32 = 12.0;

/// The measured scan the view carries: the map on or off, the band, the design's face names, the measurement or its
/// refusal, each scan vertex's colour (`0x00RRGGBB`, empty on a refusal) and each triangle's albedo (the per-channel
/// mean of its three vertex colours).
#[derive(Clone, Debug)]
pub struct Map {
    pub on: bool,
    pub band_mm: f64,
    pub names: Vec<String>,
    pub result: Result<Deviation, DeviationError>,
    pub vertex: Vec<u32>,
    pub tri: Vec<u32>,
    /// VUGFACEDEVIATIONPICK (SR543): the vertex a click on the map picked, held until the next click ([`pick`]).
    pub pick: Option<Pick>,
}

/// A scan mesh read from a file (a PLY carries no normals, no confidence and no intrinsics): mesh_core's
/// angle-weighted vertex normals (marching cubes winds counter-clockwise outward, so they face free space as the TSDF
/// normals do), confidence 1 each, the intrinsics a zero placeholder — `measure` reads neither confidence nor
/// intrinsics.
pub fn scan_mesh(mesh: Mesh) -> ScanMesh {
    let normals = mesh.normals_vertex(Tol::DEFAULT);
    let confidence = vec![1.0; mesh.positions.len()];
    ScanMesh { mesh, normals, confidence, intrinsics: Intrinsics::from_rect_params(0, 0, [0.0; 4]), frames: 0 }
}

/// The design of `doc` as tools/scan builds it: doc_core `replay`, the first failure refused by line, no solid
/// refused, every replayed solid through scan_core's `design_of`.
pub fn design(doc: &Doc) -> Result<(Design, Vec<String>), String> {
    let rp = doc_core::replay(doc, doc.tol);
    if let Some(f) = rp.failures.first() {
        return Err(format!("line {}: {:?} {}", f.line, f.fault, f.id));
    }
    if rp.solids.is_empty() {
        return Err("DesignEmpty".into());
    }
    design_of(rp.solids.iter().map(|s| (s.id.as_str(), &s.brep)), doc.tol).map_err(|e| format!("{e:?}"))
}

/// The per-channel mean of three `0x00RRGGBB` colours.
fn mean3(a: u32, b: u32, c: u32) -> u32 {
    (0..3u32).fold(0u32, |acc, i| {
        let s = 16 - 8 * i;
        let ch = |x: u32| (x >> s) & 0xff;
        acc | (((ch(a) + ch(b) + ch(c) + 1) / 3) << s)
    })
}

/// Measure `scan` against `doc` at `band_mm` from the identity: the model to draw (the scan at the aligned pose, or as
/// read on a refusal) and the map. A design that does not build refuses the whole open (`Err`); a registration
/// refusal is the map's (`Map::result`), shown by name.
pub fn open(scan: &ScanMesh, doc: &Doc, band_mm: f64) -> Result<(Model, Map), String> {
    let (design, names) = design(doc)?;
    let result = measure(scan, &design, Transform::IDENTITY, &AlignParams::DEFAULT, band_mm, Tol::DEFAULT);
    let (positions, vertex): (Vec<_>, Vec<u32>) = match &result {
        Ok(d) => (
            scan.mesh.positions.iter().map(|p| d.aligned.pose.apply_point(*p)).collect(),
            d.per_vertex.iter().map(|x| colour(shade(*x, band_mm))).collect(),
        ),
        Err(_) => (scan.mesh.positions.clone(), Vec::new()),
    };
    let mesh = Mesh { positions, triangles: scan.mesh.triangles.clone() };
    let tri = if vertex.is_empty() {
        Vec::new()
    } else {
        let at = |i: u32| vertex.get(i as usize).copied().unwrap_or(raster::ALBEDO);
        mesh.triangles.iter().map(|t| mean3(at(t[0]), at(t[1]), at(t[2]))).collect()
    };
    let model = Model::from_mesh(mesh)?;
    let on = result.is_ok();
    Ok((model, Map { on, band_mm, names, result, vertex, tri, pick: None })) // VUGFACEDEVIATIONPICK (SR543): no pick at open
}

/// The corner's lines: the statistics and the worst [`WORST`] faces (matched ones, by rms) with their signed mean, or
/// the refusal by name.
pub fn corner(map: &Map) -> Vec<String> {
    match &map.result {
        Ok(d) => {
            let s = &d.stats;
            let mut out: Vec<String> = pick_first(map); // VUGFACEDEVIATIONPICK (SR543): the pick's line first
            out.push(format!("deviation rms={:.3} max={:.3} inside={:.3} band={}", s.rms_mm, s.max_mm, s.inside, map.band_mm));
            for f in d.faces.iter().filter(|f| f.n > 0).take(WORST) {
                let name = map.names.get(f.face as usize).map(String::as_str).unwrap_or("?");
                out.push(format!("{name} rms={:.3} mean={:+.3}", f.rms_mm, f.mean_mm));
            }
            out
        }
        Err(DeviationError::NoAlignment { rms_mm, pairs }) => vec![format!("refused NoAlignment rms={rms_mm:.3} pairs={pairs}")],
        Err(DeviationError::Unconstrained { level }) => vec![format!("refused Unconstrained level={level}")],
        Err(e) => vec![format!("refused {e:?}")],
    }
}

/// The albedo raster.rs paints triangle `tri` on: the map's, while it is on and no body or pull replaces the scan.
pub fn albedo(view: &View, tri: u32) -> Option<u32> {
    let m = view.deviation.as_ref()?;
    if !m.on || view.design.body.is_some() || view.design.pull.is_some() {
        return None;
    }
    m.tri.get(tri as usize).copied()
}

/// `d` / `D` toggles the map while one is loaded (a refused map has nothing to colour: the key is taken, nothing
/// changes). The view asks this after the sketch's keys, so inside a sketch `d` is the dimension tool.
pub fn key(view: &mut View, k: &str) -> Option<bool> {
    if !matches!(k, "d" | "D") {
        return None;
    }
    let m = view.deviation.as_mut()?;
    if m.result.is_err() {
        return Some(false);
    }
    m.on = !m.on;
    Some(true)
}

/// The corner, top-right and right-aligned, upper-cased (the bitmap font's capitals): a refusal in the critical
/// status colour, the statistics and the faces in the primary text colour (dimmed to secondary while the map is off).
pub fn overlay(view: &View, fr: &mut Frame) {
    let Some(m) = view.deviation.as_ref() else { return };
    let col = match (&m.result, m.on) {
        (Err(_), _) => rgba(palette_core::STATUS_CRITICAL),
        (Ok(_), true) => rgba(palette_core::TEXT_PRIMARY.1),
        (Ok(_), false) => rgba(palette_core::TEXT_SECONDARY.1),
    };
    for (i, line) in corner(m).iter().enumerate() {
        let text = line.to_uppercase();
        let x = fr.width as f32 - 8.0 * text.chars().count() as f32 - 6.0;
        raster::text(fr, &text, x.max(0.0), 6.0 + LINE_PX * i as f32, col);
    }
    ring(view, m, fr); // VUGFACEDEVIATIONPICK (SR543): the ring at the picked vertex
}

// VUGFACEDEVIATIONPICK (LEDGER SR543): a click on the coloured scan names the vertex under it. `View::release`'s Pick
// arm hands its ONE `hit` (the triangle and the point) to [`pick`]: the nearest of that triangle's three vertices to the
// hit point is the pick, its signed deviation read from scan_core's `per_vertex` and its design face from scan_core's
// `face_of_vertex` (the pairing's own face, never re-paired here). The corner's first line carries it
// (`pick: <face> d=<+mm> <band>`) and a ring marks the vertex until the next click; a click on no triangle clears it.

/// The ring's radius, surface pixels.
pub const RING_PX: f32 = 5.0;

/// The picked scan vertex: its index, its signed deviation (`None`: off the design), its design face and its
/// position (the drawn model's, the aligned pose).
#[derive(Clone, Debug, PartialEq)]
pub struct Pick {
    pub vertex: u32,
    pub d: Option<f64>,
    pub face: Option<u32>,
    pub at: [f64; 3],
}

/// The map is live for a pick: measured, on, and no body or pull replaces the scan (the condition [`albedo`] reads).
fn active(view: &View) -> bool {
    view.deviation.as_ref().is_some_and(|m| m.on && m.result.is_ok()) && view.design.body.is_none() && view.design.pull.is_none()
}

/// A click on the map: `hit` is the view's one hit of the click. On no triangle the pick clears; on triangle `t` the
/// nearest of its three vertices to the hit point is picked. Nothing changes while the map is not live. True when the
/// pick changed.
pub fn pick(view: &mut View, model: &Model, hit: Option<(euclase::Vec3, u32)>) -> bool {
    if !active(view) {
        return false;
    }
    let next = hit.and_then(|(p, t)| {
        let (pos, idx) = (model.positions(), model.indices());
        let at = |v: u32| -> Option<[f64; 3]> {
            let k = v as usize * 3;
            Some([*pos.get(k)? as f64, *pos.get(k + 1)? as f64, *pos.get(k + 2)? as f64])
        };
        let q = [p.x as f64, p.y as f64, p.z as f64];
        let dist2 = |a: [f64; 3]| (0..3).map(|k| (a[k] - q[k]) * (a[k] - q[k])).sum::<f64>();
        let tri = idx.get(t as usize * 3..t as usize * 3 + 3)?;
        let (v, a) = tri.iter().filter_map(|&v| at(v).map(|a| (v, a))).min_by(|x, y| dist2(x.1).total_cmp(&dist2(y.1)))?;
        let m = view.deviation.as_ref()?;
        let d = m.result.as_ref().ok()?;
        Some(Pick { vertex: v, d: d.per_vertex.get(v as usize).copied().flatten(), face: d.face_of_vertex.get(v as usize).copied().flatten(), at: a })
    });
    let Some(m) = view.deviation.as_mut() else { return false };
    let changed = m.pick != next;
    m.pick = next;
    changed
}

/// The pick's corner line: `pick: <face name, or -> d=<signed mm, or none> <inside | under k | over k | off>`.
pub fn pick_line(map: &Map, p: &Pick) -> String {
    let name = p.face.and_then(|f| map.names.get(f as usize)).map(String::as_str).unwrap_or("-");
    let d = p.d.map_or_else(|| "none".to_string(), |d| format!("{d:+.3}"));
    let band = match shade(p.d, map.band_mm) {
        Shade::Inside => "inside".to_string(),
        Shade::Under(k) => format!("under {k:.2}"),
        Shade::Over(k) => format!("over {k:.2}"),
        Shade::Off => "off".to_string(),
    };
    format!("pick: {name} d={d} {band}")
}

/// The corner's leading lines: the pick's line while the map is on and a pick is held, else none.
fn pick_first(map: &Map) -> Vec<String> {
    match (&map.pick, map.on) {
        (Some(p), true) => vec![pick_line(map, p)],
        _ => Vec::new(),
    }
}

/// A ring of [`RING_PX`] at the picked vertex while the map is live (16 chords through `raster::line`).
fn ring(view: &View, m: &Map, fr: &mut Frame) {
    let Some(p) = m.pick.as_ref() else { return };
    if !active(view) {
        return;
    }
    let Some((cx, cy)) = view.project_f64(p.at) else { return };
    let col = rgba(palette_core::TEXT_PRIMARY.1);
    let pt = |i: u32| {
        let a = i as f32 * core::f32::consts::TAU / 16.0;
        (cx as f32 + RING_PX * a.cos(), cy as f32 + RING_PX * a.sin())
    };
    for i in 0..16 {
        raster::line(fr, pt(i), pt(i + 1), col);
    }
}
