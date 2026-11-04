// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vug (CODEX §2, 3D), a host face — the deviation map (VUGFACEDEVIATION, LEDGER SR525): a scan measured
//! against a design document through scan_core, drawn in the band's colours; holds no geometry.
//!
//! VUGSCANDEVIATION (SR510) measured the made part against its model in tools/scan; this is the same measurement in
//! the face, built EXACTLY as tools/scan builds it: doc_core `replay` of the document (the first failing statement
//! refuses by line, no solid refuses `DesignEmpty`), every live solid (journal.rs live_solids) through scan_core's ONE builder `design_of`,
//! `measure` from the identity with `AlignParams::DEFAULT` at [`BAND_MM`] (tools/scan's default `--band`). The model
//! the face draws is the scan at the aligned pose (the design's frame); each scan vertex's colour is scan_core's ONE
//! ramp `colour(shade(d, band))` (palette_core's status rows), and raster.rs paints each triangle on the per-channel
//! mean of its three vertex colours ([`albedo`], the one arm in `tri_colour`). The corner (top-right) carries the
//! statistics and the worst three faces by rms with their signed mean; a refused registration is named there
//! (`refused NoAlignment rms= pairs=` / `refused Unconstrained level=`) and the scan is drawn as read, in the albedo.
//! `d` outside a sketch toggles the map (inside a sketch `d` stays the dimension tool: the view asks the sketch first).
//! BP-vug-5: the map is a printed residual; neither the scan nor the design changes.
//! VUGFACEDEVIATIONJOURNAL (SR550): `Viewer::against_journal` measures against the document the tree holds and
//! [`follow`] re-measures when that document changes (the corner's last line `design=journal@<gen>`).

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
    /// VUGFACEDEVIATIONJOURNAL (SR550): the journal the map follows ([`follow`]); None for a document read from a path.
    pub source: Option<Source>,
    /// VUGFACEGDT (SR553): every design face read as ISO 1101 tolerances, once per build ([`gdt_of`]; empty on a refusal).
    pub gdt: Vec<scan_core::FaceGdt>,
    /// VUGFACEGDT (SR553): the tolerances named on the face (`gdt <face>=<kind>:<tol>`, [`command`]), in the order named.
    pub callouts: Vec<Callout>,
    /// VUGFACEGDTREMEASURE (SR560): the edge margin (mm) this map's gdt and frame read at ([`open`]'s argument).
    pub margin_mm: f64,
    /// VUGFACEGDTREMEASURE (SR560): the scan and the design the map measured, what scan_core `frame_of` reads after the open.
    pub basis: std::sync::Arc<(ScanMesh, Design)>,
    /// VUGFACEGDTREMEASURE (SR560): the datum faces A|B|C (`datum A=<f> B=<f> C=<f>`, [`command`]).
    pub datums: Option<[u32; 3]>,
    /// VUGFACEGDTREMEASURE (SR560): the datum frame position reads in, or its refusal on a re-measure ([`reframe`]).
    pub frame: Option<Result<scan_core::gdt::DatumFrame, String>>,
    /// VUGFACEGDTDATUMSPICK (SR564): the datum faces picked so far by the keys `A` `B` `C` ([`datum_key`]), before all three.
    pub chosen: [Option<u32>; 3],
    /// VUGFACEGDTDATUMSPICK (SR564): the callout faces a re-measure dropped ([`carry`]), named on the corner until the next command.
    pub lost: Vec<String>,
    /// VUGFACEDEVIATIONSOLIDFAULT (SR565): the replay failures no solid reads (an Op, an unnamed sketch), counted and
    /// named on the corner ([`fault_lines`]); a failure a solid reads refuses the design instead ([`design`]).
    pub faults: Vec<doc_core::Failure>,
    /// VUGFACEGDTREPORTKEY (SR575): the scan's file name (lib.rs `read_scan`), the report header's `scan=`; None reads
    /// scan_core `gdt::scan_tag`.
    pub scan_name: Option<String>,
    /// VUGFACEGDTREPORTKEY (SR575): a path map's design document path (`Viewer::open_against`), its report written beside it.
    pub design_path: Option<std::path::PathBuf>,
    /// VUGFACEGDTREPORTKEY (SR575): the report the `report` command wrote, named on the corner until the next command.
    pub reported: Option<std::path::PathBuf>,
    /// VUGFACECONFIDENCETINT (SR592): the confidence floor `conf <w>` named on the map and the measurement before it
    /// ([`Conf`]); None: no floor, `result` as measured.
    pub conf: Option<Conf>,
    /// VUGFACEDEVIATIONHELD (SR600): the statement journal.rs `hold` held (a solid's ONE root fault, its chain in
    /// `faults`), the design built over the standing document; named on the corner `held=<id>` ([`held_line`]).
    pub held: Option<String>,
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

/// The design of `doc` as tools/scan builds it: doc_core `replay`, the first failure on a statement a solid reads
/// (journal.rs solid_stmts) refused by line, no solid refused, every live solid (journal.rs live_solids) through
/// scan_core's `design_of`; VUGFACEDEVIATIONSOLIDFAULT (SR565): every other failure (an Op, an unnamed sketch) is
/// returned, counted on the corner ([`fault_lines`]), and the design still measures. VUGFACEDEVIATIONHELD (SR600): ONE
/// held solid fault (journal.rs `hold`, regenerate's rule) is carried too, the design is the standing document's solids
/// and the held root's id is returned.
pub fn design(doc: &Doc) -> Result<(Design, Vec<String>, Vec<doc_core::Failure>, Option<String>), String> {
    let rp = doc_core::replay(doc, doc.tol);
    let (on_solid, faults, held) = held_split(doc, rp.failures); // VUGFACEDEVIATIONHELD (SR600): split_faults then journal.rs hold
    if let Some(f) = on_solid.first() {
        return Err(fault_text(f));
    }
    if rp.solids.is_empty() {
        return Err("DesignEmpty".into());
    }
    let (design, names) = design_of(live_of(&crate::journal::standing(doc, &faults), &rp.solids), doc.tol).map_err(|e| format!("{e:?}"))?; // VUGFACEDEVIATIONLIVE (SR558): the live solids only, never a fillet's on solid beside it; VUGFACEDEVIATIONHELD (SR600): over the standing document
    Ok((design, names, faults, held))
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
/// read on a refusal) and the map, its gdt read at the edge margin `margin_mm` (VUGFACEGDTREMEASURE SR560). A design
/// that does not build refuses the whole open (`Err`); a registration refusal is the map's (`Map::result`), shown by name.
pub fn open(scan: &ScanMesh, doc: &Doc, band_mm: f64, margin_mm: f64) -> Result<(Model, Map), String> {
    let (design, names, faults, held) = design(doc)?; // VUGFACEDEVIATIONHELD (SR600): the held root; VUGFACEDEVIATIONSOLIDFAULT (SR565): the faults no solid reads
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
    let mut map = Map { on, band_mm, names, gdt: Vec::new(), result, vertex, tri, pick: None, source: None, callouts: Vec::new(), margin_mm, basis: std::sync::Arc::new((scan.clone(), design)), datums: None, frame: None, chosen: [None; 3], lost: Vec::new(), faults, scan_name: None, design_path: None, reported: None, conf: None, held }; reframe(&mut map); Ok((model, map)) // VUGFACEDEVIATIONJOURNAL (SR550) + VUGFACEGDT (SR553): no source, no callouts yet; VUGFACEGDTREMEASURE (SR560): the gdt read once per build through reframe
}

/// The corner's lines: the statistics and the worst [`WORST`] faces (matched ones, by rms) with their signed mean, or
/// the refusal by name.
pub fn corner(map: &Map) -> Vec<String> {
    let mut out = measured(map); out.extend(conf_line(map)); out.extend(fault_lines(map)); out.extend(source_line(map)); out // VUGFACEDEVIATIONJOURNAL (SR550): the journal's line last; VUGFACEDEVIATIONSOLIDFAULT (SR565): the faults before it
}

/// The corner's measured lines: the pick's, the statistics and the worst faces, or the refusal by name.
fn measured(map: &Map) -> Vec<String> {
    match &map.result {
        Ok(d) => {
            let s = &d.stats;
            let mut out: Vec<String> = pick_first(map); // VUGFACEDEVIATIONPICK (SR543): the pick's line first
            out.push(format!("deviation rms={:.3} max={:.3} inside={:.3} band={}", s.rms_mm, s.max_mm, s.inside, map.band_mm));
            for f in d.faces.iter().filter(|f| f.n > 0).take(WORST) {
                let name = map.names.get(f.face as usize).map(String::as_str).unwrap_or("?");
                out.push(format!("{name} rms={:.3} mean={:+.3}", f.rms_mm, f.mean_mm));
            }
            out.extend(report_lines(map)); out.extend(lost_lines(map)); out.extend(reported_line(map)); out // VUGFACEGDTREPORTKEY (SR575): `report <path>` after the lost lines; VUGFACEGDTREPORT (SR571): the frame and callout lines the .vugreport writes; VUGFACEGDT (SR553): a line per callout after the faces; VUGFACEGDTREMEASURE (SR560): the frame line first
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
    m.on = !m.on; show(view); // VUGFACEDEVIATIONJOURNAL (SR550): a journal map hides / regenerates the body
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
        let (x, y) = line_at(corner_origin(view, fr.width), &text, i); // VUGFACEJOURNALSOLIDFAULT (SR572): the ONE corner geometry, paint and hit; VUGFACEJOURNALFAULTFIX (SR578): its right edge left of a shown panel
        raster::text(fr, &text, x, y, col);
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

// ---- VUGFACEDEVIATIONJOURNAL (LEDGER SR550): measure against the document the tree holds ----
// `Viewer::against_journal` measures a scan against `view.design.journal.doc` through the ONE road [`open`] ->
// [`design`] -> doc_core `replay` -> scan_core `design_of`; the map keeps the Doc it measured and [`follow`] (lib.rs,
// after every key and input) re-measures when the journal's document differs from it (Doc's own equality, the test
// `Journal::commit` uses), so a record, an edge edit, a panel edit, an undo or a load each count once. The generation
// is the map's count of the documents it followed. While a journal map is on the scan is drawn (the body cleared);
// off, the body is the journal's own (`journal::regenerate`), never stashed.

use crate::Viewer;
use doc_core::Stmt;

/// The refusal when the journal's document holds no body (no extrude, no revolve).
pub const JOURNAL_EMPTY: &str = "JournalEmpty";

/// The journal a map follows: the scan, the document last seen, its generation, the generation the map shows, and
/// the refusal of the newest one when it did not measure (the map then still shows `measured`).
#[derive(Clone, Debug)]
pub struct Source {
    pub scan: ScanMesh,
    pub doc: Doc,
    pub generation: u64,
    pub measured: u64,
    pub refused: Option<String>,
}

/// `JournalEmpty` unless `doc` has a statement that makes a body; any other refusal is [`design`]'s, by line.
fn journal_body(doc: &Doc) -> Result<(), String> {
    if doc.stmts.iter().any(|s| matches!(s, Stmt::Extrude { .. } | Stmt::Revolve { .. })) { Ok(()) } else { Err(JOURNAL_EMPTY.to_string()) }
}

/// A journal map on: the body cleared, so the scan is drawn in the band's colours; off: the journal's body back.
fn show(view: &mut View) {
    let Some(on) = view.deviation.as_ref().filter(|m| m.source.is_some()).map(|m| m.on) else { return };
    view.design.body = if on { None } else { crate::journal::regenerate(&view.design.journal.doc).ok().and_then(|r| r.body) };
}

/// The corner's source line: `design=journal@<gen>`, with ` refused <why> map@<measured>` when the newest did not measure.
fn source_line(map: &Map) -> Option<String> {
    let s = map.source.as_ref()?;
    Some(match &s.refused {
        None => format!("design=journal@{} solids={}{}", s.generation, crate::journal::live_solids(&crate::journal::standing(&s.doc, &map.faults)).len(), fault_count(map)), // VUGFACEDEVIATIONHELD (SR600): the standing document's solids, a held one not counted // VUGFACEDEVIATIONLIVE (SR558): the measured design's solid count; VUGFACEDEVIATIONSOLIDFAULT (SR565): ` faults=<m>` when any
        Some(why) => format!("design=journal@{} refused {why} map@{}", s.generation, s.measured),
    })
}

impl Viewer {
    /// Measure `scan` against the journal's document at [`BAND_MM`]: generation 0, the drawn model the aligned scan,
    /// the camera homed on it. A document with no body refuses [`JOURNAL_EMPTY`]; a refusal changes nothing.
    pub fn against_journal(&mut self, scan: ScanMesh) -> Result<(), String> {
        let doc = self.view.design.journal.doc.clone();
        journal_body(&doc)?;
        let (model, mut map) = open(&scan, &doc, BAND_MM, GDT_MARGIN_MM)?; // VUGFACEGDTREMEASURE (SR560): a ScanMesh carries no truncation: the fallback
        map.source = Some(Source { scan, doc, generation: 0, measured: 0, refused: None });
        self.model = model;
        self.view.deviation = Some(map);
        show(&mut self.view);
        self.view.home(&self.model);
        Ok(())
    }
}

/// Re-measure a journal map when the journal's document is no longer the one it saw: the generation moves; a
/// measured one replaces the model and the map (the on state kept, the pick cleared), a refused one keeps both and
/// is named on the corner.
pub fn follow(v: &mut Viewer) {
    let Some(m) = v.view.deviation.as_mut() else { return };
    let Some(src) = m.source.as_mut() else { return };
    if src.doc == v.view.design.journal.doc || same_solids(src, &v.view.design.journal.doc) {
        return lazy(v); // VUGFACEDEVIATIONLIVE (SR558): no solid statement moved, no re-measure
    }
    let doc = v.view.design.journal.doc.clone();
    src.generation += 1;
    let generation = src.generation;
    let out = journal_body(&doc).and_then(|()| open(&src.scan, &doc, m.band_mm, m.margin_mm)); // VUGFACEGDTREMEASURE (SR560): the map's margin kept
    src.doc = doc;
    match out {
        Ok((model, mut map)) => {
            map.on = m.on && map.result.is_ok();
            map.source = m.source.take().map(|s| Source { measured: generation, refused: None, ..s });
            keep_conf(m, &mut map); carry(m, &mut map, &model); keep_names(m, &mut map); // VUGFACEGDTREPORTKEY (SR575): the scan name and design path follow; VUGFACEGDTREMEASURE (SR560): the callouts and the datums follow the re-measure, re-read and re-tinted
            *m = map;
            v.model = model;
        }
        Err(why) => src.refused = Some(why),
    }
    show(&mut v.view);
}

// ---- VUGFACEGDT (LEDGER SR553): the gdt callouts on the map ----
// VUGSCANGDT (SR542) reads each design face as ISO 1101 tolerances in tools/scan; this is the same reading in the face.
// [`open`] runs scan_core's ONE `gdt` once per build ([`gdt_of`]) and the Map keeps it; `gdt <face>=<kind>:<tol>`
// ([`command`], tools/scan's `--gdt` grammar and refusal names) names a tolerance on a face, and the verdict is
// scan_core's `FaceGdt::verdict`, read on demand. The corner lists one line per callout
// (`<face> <kind>=<mm> tol=<mm> PASS|FAIL|UNMEASURED`) and a FAIL face's scan vertices take the band ramp's over colour
// (`colour(Shade::Over(1.0))`, [`retint`]); `gdt clear` removes them. BP-vug-5: a printed verdict, nothing changes.

/// The FALLBACK edge margin (mm) for scan_core's `gdt` when the caller knows no truncation (a PLY carries none;
/// `Viewer::open_deviation(.., None)`): tools/scan's default `--trunc` (3 x its 4 mm voxel). A caller that fused the scan
/// passes the volume's own `Tsdf::trunc_mm()` (VUGFACEGDTREMEASURE SR560).
pub const GDT_MARGIN_MM: f64 = 12.0;

/// A tolerance named on a design face: the face (an index into `Map::names`), the kind and the tolerance (mm).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Callout {
    pub face: u32,
    pub kind: scan_core::Kind,
    pub tol_mm: f64,
}

/// Every design face read as tolerances (scan_core's `gdt` at the edge margin `margin_mm`), or none on a refused
/// registration.
pub fn gdt_of(scan: &ScanMesh, design: &Design, result: &Result<Deviation, DeviationError>, margin_mm: f64) -> Vec<scan_core::FaceGdt> {
    match result {
        Ok(d) => scan_core::gdt(scan, design, d, margin_mm, Tol::DEFAULT), // VUGFACEGDTREMEASURE (SR560): the margin an argument
        Err(_) => Vec::new(),
    }
}

/// One command on the map: `gdt <face>=<kind>:<tol>` names a tolerance (the face as the corner names it; the grammar
/// is scan_core's ONE `gdt::Callout::parse`, VUGSCANGDTSPEC SR559: flatness, cylindricity, position or profile, the
/// tolerance finite and above 0), replacing the face's callout of the same kind; `gdt clear` removes every callout. A
/// FAIL face is tinted ([`retint`]). Refused by name, nothing changed: no map, a refused map, `GdtFace` (no such face),
/// `GdtKind` (its surface does not take the kind), anything else the usage. True when the map changed.
pub fn command(view: &mut View, model: &Model, line: &str) -> Result<bool, String> {
    const USAGE: &str = "usage: gdt <face>=<kind>:<tol> | gdt clear";
    forget(view); if let Some(r) = datum_command(view, model, line) { return r; } if let Some(r) = conf_command(view, model, line) { return r; } // VUGFACEGDTREMEASURE (SR560): `datum ...` is the frame's command
    let rest = line.trim().strip_prefix("gdt ").map(str::trim).ok_or(USAGE)?;
    let m = view.deviation.as_mut().ok_or("gdt: no map")?;
    if let Err(e) = &m.result {
        return Err(format!("gdt: refused map {e:?}"));
    }
    if rest == "clear" {
        let changed = !m.callouts.is_empty();
        m.callouts.clear();
        retint(m, model);
        return Ok(changed);
    }
    let spec = scan_core::gdt::Callout::parse(rest).ok().filter(|s| s.off_mm.is_none()).ok_or(USAGE)?; // VUGSCANGDTPROFILEDATUM (SR567): an unequal zone is the usage (VUG-SCAN-GDT-PROFILE-UZ-WIRE owed); VUGSCANGDTSPEC (SR559): the ONE parse; a refusal is the usage
    let (face, kind, tol_mm) = (spec.face.as_str(), spec.kind, spec.tol_mm);
    let i = m.names.iter().position(|n| n == face).ok_or_else(|| format!("GdtFace {face}"))? as u32;
    if !m.gdt.get(i as usize).is_some_and(|f| f.accepts(kind)) {
        return Err(format!("GdtKind {face} {}", kind.name()));
    }
    let c = Callout { face: i, kind, tol_mm };
    match m.callouts.iter_mut().find(|x| x.face == i && x.kind == kind) {
        Some(x) => *x = c,
        None => m.callouts.push(c),
    }
    retint(m, model);
    Ok(true)
}

/// The callout's verdict: scan_core's `FaceGdt::verdict` of its face (Unmeasured when the face is not read).
pub fn verdict(map: &Map, c: &Callout) -> scan_core::Verdict {
    map.gdt.get(c.face as usize).map_or(scan_core::Verdict::Unmeasured, |f| f.verdict(c.kind, c.tol_mm))
}

/// The callout's corner line: `<face> <kind>=<mm, or none> tol=<mm> PASS|FAIL|UNMEASURED`.
pub fn gdt_line(map: &Map, c: &Callout) -> String {
    let name = map.names.get(c.face as usize).map(String::as_str).unwrap_or("?");
    let kind = scan_core::gdt::kind_token(c.kind, map.gdt.get(c.face as usize)); // VUGFACEGDTREPORTKEY (SR575): scan_core's one kind token (tools/scan's --vugreport calls it); VUGSCANGDTPROFILEDATUM (SR567): `profile_frame` when read in the frame
    scan_core::gdt::callout_line(name, kind, c.tol_mm, verdict(map, c)) // VUGFACEGDTREPORT (SR571): the one formatter tools/scan's --vugreport calls
}

/// The corner's callout lines, in the order named.
fn gdt_lines(map: &Map) -> Vec<String> {
    map.callouts.iter().map(|c| gdt_line(map, c)).collect()
}

/// The map's colours from its measurement and callouts: each scan vertex the band ramp's `colour(shade(d, band))` as
/// [`open`] paints it, the vertices paired with a face whose callout FAILs the ramp's over colour; each triangle the
/// [`mean3`] of its three vertices (the model's triangles are the scan's, in order). A refused map is left as it is.
pub fn retint(map: &mut Map, model: &Model) {
    let Ok(d) = &map.result else { return };
    let fail: Vec<u32> = {
        let m: &Map = map;
        m.callouts.iter().filter(|c| matches!(verdict(m, c), scan_core::Verdict::Fail { .. })).map(|c| c.face).collect()
    };
    let over = colour(Shade::Over(1.0));
    map.vertex = d
        .per_vertex
        .iter()
        .enumerate()
        .map(|(i, x)| match d.face_of_vertex.get(i).copied().flatten() {
            Some(f) if fail.contains(&f) => over,
            _ => colour(shade(*x, map.band_mm)),
        })
        .collect();
    let at = |i: u32| map.vertex.get(i as usize).copied().unwrap_or(raster::ALBEDO);
    map.tri = model.indices().chunks_exact(3).map(|t| mean3(at(t[0]), at(t[1]), at(t[2]))).collect();
}

// ---- VUGFACEDEVIATIONLIVE (LEDGER SR558): the live solids, re-measured only when a solid's statements change ----
// [`design`] builds from journal.rs `live_outs` (THE body rule `live_solids`: a fillet / chamfer stands in its `on`
// solid's place), so a filleted document is measured against the fillet's solid, not the original beside it.
// [`follow`] re-measures only when journal.rs `solid_stmts` (what replay feeds into solids) or the Tol moved, or when
// the map's newest document refused; any other change (a stroke in a sketch no feature names, an Op, the machine) is
// taken as seen with the generation kept.

/// The live solids of `doc` as (id, brep) for scan_core's `design_of`, in Body order.
fn live_of<'a>(doc: &Doc, solids: &'a [doc_core::SolidOut]) -> Vec<(&'a str, &'a brep_core::Brep)> {
    crate::journal::live_outs(doc, solids).into_iter().map(|o| (o.id.as_str(), &o.brep)).collect()
}

/// `doc` builds the same solids as the document the map last saw: its newest measured (no refusal), the same Tol and
/// the same solid statements.
fn same_solids(src: &Source, doc: &Doc) -> bool {
    src.refused.is_none() && src.doc.tol == doc.tol && crate::journal::solid_stmts(&src.doc) == crate::journal::solid_stmts(doc)
}

/// A journal document that moves no solid: seen (the generation kept) and the body shown as the map's state says.
fn lazy(v: &mut Viewer) {
    let doc = &v.view.design.journal.doc;
    let Some(src) = v.view.deviation.as_mut().and_then(|m| m.source.as_mut()) else { return };
    if src.doc == *doc {
        return;
    }
    src.doc = doc.clone();
    refault(&mut v.view); show(&mut v.view); // VUGFACEDEVIATIONSOLIDFAULT (SR565): an Op or an unnamed sketch moves the fault count, never the measurement
}

// ---- VUGFACEGDTREMEASURE (LEDGER SR560): the callouts and the datum frame follow a re-measure ----
// VUGFACEGDT (SR553) read the gdt once per build, but [`follow`] builds a new Map on every journal change, so the callouts
// were lost; [`carry`] moves them (and the datum faces) into the new map BY FACE NAME (an edit can renumber faces), then
// [`reframe`] re-reads the gdt and the frame and [`retint`] re-tints. `datum A=<f> B=<f> C=<f>` ([`command`]) names an
// ISO 5459 datum frame: scan_core `gdt::frame_of` on the map's own scan and design at its margin, every cylinder face's
// position re-read in it by scan_core `gdt::in_frame` (the one loop tools/scan's `--datum` also calls); the corner gains
// `frame A= B= C= residual=` before the callout lines. BP-vug-5: a printed verdict, nothing changes.

/// The datum frame on design faces `faces` (scan_core `frame_of` on the map's basis at its margin); refused by
/// tools/scan's name `GdtDatum <face> <error>` (the face of the refused label), or the refused map.
fn frame(map: &Map, faces: [u32; 3]) -> Result<scan_core::gdt::DatumFrame, String> {
    let Ok(d) = &map.result else { return Err("datum: refused map".to_string()) };
    let (scan, design) = &*map.basis;
    scan_core::gdt::frame_of(scan, design, d, faces, map.margin_mm, Tol::DEFAULT).map_err(|e| {
        let k = scan_core::gdt::DATUM_LABELS.iter().position(|&l| l == e.label()).unwrap_or(0);
        let name = map.names.get(faces[k] as usize).map(String::as_str).unwrap_or("?");
        format!("GdtDatum {name} {e:?}")
    })
}

/// The map's gdt re-read on its basis at its margin ([`gdt_of`]), then the frame of its datums (if any) and every
/// cylinder face's position read in it (scan_core `gdt::in_frame`). The verdicts are read on demand from `gdt`.
pub fn reframe(map: &mut Map) {
    let basis = map.basis.clone();
    let (scan, design) = &*basis;
    map.gdt = gdt_of(scan, design, &map.result, map.margin_mm);
    let datums = map.datums;
    let fr = datums.map(|f| frame(&*map, f));
    map.frame = fr;
    if let (Some(Ok(fr)), Ok(d)) = (map.frame.clone(), &map.result) { // VUGSCANGDTPROFILEDATUM (SR567): the deviation for the profile in the frame
        scan_core::gdt::in_frame(scan, design, d, map.margin_mm, &mut map.gdt, &fr);
    }
}

/// A re-measure keeps what was named on the old map: each callout whose face name the new design still has (by name,
/// the new index), the datum faces likewise (a datum whose face is gone leaves the frame refused
/// `GdtDatumFace <label> <name>`, named on the corner); then [`reframe`] and [`retint`] on the new measurement.
fn carry(old: &Map, new: &mut Map, model: &Model) {
    let name = |f: u32| old.names.get(f as usize).cloned();
    let names = new.names.clone();
    let at = |n: &str| names.iter().position(|x| x == n).map(|i| i as u32);
    new.callouts = old.callouts.iter().filter_map(|c| Some(Callout { face: at(name(c.face)?.as_str())?, ..*c })).collect();
    let mut lost = None;
    new.datums = None;
    if let Some(d) = old.datums {
        let mut faces = [0u32; 3];
        for k in 0..3 {
            match name(d[k]).as_deref().and_then(at) {
                Some(i) => faces[k] = i,
                None => {
                    lost.get_or_insert(format!("GdtDatumFace {} {}", scan_core::gdt::DATUM_LABELS[k], name(d[k]).unwrap_or_default()));
                }
            }
        }
        if lost.is_none() {
            new.datums = Some(faces);
        }
    }
    reframe(new);
    if lost.is_some() {
        new.frame = lost.map(Err);
    }
    retint(new, model); carry_named(old, new); // VUGFACEGDTDATUMSPICK (SR564): the lost callouts named, the partial picks by name
}

/// `datum ...` lines are the frame's ([`datum`]); any other line is not (None).
fn datum_command(view: &mut View, model: &Model, line: &str) -> Option<Result<bool, String>> {
    let rest = line.trim().strip_prefix("datum")?;
    if !(rest.is_empty() || rest.starts_with(' ')) {
        return None;
    }
    Some(datum(view, model, rest.trim()))
}

/// `datum A=<face> B=<face> C=<face>` (tools/scan's `--datum`: exactly three, labelled A, B, C in order, each face as the
/// corner names it, split at the FIRST `=`) names the datum frame; `datum clear` removes it. Refused by name, nothing
/// changed: no map, a refused map, `GdtDatumFace <label> <face>` (no such face), `GdtDatum <face> <error>` (scan_core
/// `frame_of`'s refusal), anything else the usage. A position FAIL tints ([`retint`]). True when the map changed.
fn datum(view: &mut View, model: &Model, rest: &str) -> Result<bool, String> {
    const USAGE: &str = "usage: datum A=<face> B=<face> C=<face> | datum clear";
    let m = view.deviation.as_mut().ok_or("datum: no map")?;
    if let Err(e) = &m.result {
        return Err(format!("datum: refused map {e:?}"));
    }
    if rest == "clear" {
        let changed = m.datums.is_some() || m.frame.is_some();
        m.datums = None; m.chosen = [None; 3]; // VUGFACEGDTDATUMSPICK (SR564): the partial picks go with the frame
        reframe(m);
        retint(m, model);
        return Ok(changed);
    }
    let args: Vec<&str> = rest.split_whitespace().collect();
    if args.len() != 3 {
        return Err(USAGE.into());
    }
    let mut faces = [0u32; 3];
    for (k, label) in ["A=", "B=", "C="].iter().enumerate() {
        let face = args[k].strip_prefix(*label).filter(|f| !f.is_empty()).ok_or(USAGE)?;
        faces[k] = m.names.iter().position(|n| n == face).ok_or_else(|| format!("GdtDatumFace {} {face}", scan_core::gdt::DATUM_LABELS[k]))? as u32;
    }
    frame(m, faces)?;
    m.datums = Some(faces);
    reframe(m);
    retint(m, model);
    Ok(true)
}

/// The corner's frame line: `frame A=<face> B=<face> C=<face> residual=<mm>`, or `frame refused <why>`; none without datums.
fn frame_line(map: &Map) -> Option<String> {
    let name = |f: u32| map.names.get(f as usize).map(String::as_str).unwrap_or("?");
    match (&map.frame, map.datums) {
        (Some(Ok(fr)), Some(d)) => Some(scan_core::gdt::frame_line([name(d[0]), name(d[1]), name(d[2])], fr.residual_mm)), // VUGFACEGDTREPORT (SR571): the one formatter
        (Some(Err(why)), _) => Some(format!("frame refused {why}")),
        _ => chosen_line(map), // VUGFACEGDTDATUMSPICK (SR564): the datums picked so far
    }
}

// ---- VUGFACEGDTDATUMSPICK (LEDGER SR564): datums chosen by clicking faces; a callout a re-measure loses is named ----
// With a pick held on the live map (SR543's [`pick`] names the face under the click) and no sketch open, the UPPERCASE
// keys `A` `B` `C` make the picked face that datum and `D` clears the datums ([`datum_key`], asked by `View::key` before
// the sketch's keys, which take `a` / `c` even outside a sketch). While fewer than three are named the corner reads
// `frame A=<face> B=- C=-`; the third calls the EXISTING [`datum`] with the command's own `A=<f> B=<f> C=<f>`, so the frame
// is the command's bit for bit. [`carry`] keeps the callouts whose face the new design lacks in `Map::lost` and the corner
// reads `callout lost <face>` until the next command. BP-vug-5: a printed verdict, nothing changes.

/// The datum keys: `A` / `B` / `C` set that datum to the held pick's face (a set datum replaced; the frame built by
/// [`datum`] once all three are named, a refusal named on the corner as `frame refused <why>`), `D` clears them. None
/// (the key falls through unchanged) unless the map is live, no sketch is open and the pick names a face.
pub fn datum_key(view: &mut View, model: &Model, k: &str) -> Option<bool> {
    let slot = match k {
        "A" => 0,
        "B" => 1,
        "C" => 2,
        "D" => 3,
        _ => return None,
    };
    if !active(view) || view.design.face.is_some() {
        return None;
    }
    let face = view.deviation.as_ref()?.pick.as_ref()?.face?;
    forget(view);
    if slot == 3 {
        let had = view.deviation.as_ref().is_some_and(|m| m.chosen.iter().any(Option::is_some));
        return Some(datum(view, model, "clear").unwrap_or(false) | had);
    }
    let m = view.deviation.as_mut()?;
    let mut c = m.datums.map(|d| d.map(Some)).unwrap_or(m.chosen);
    c[slot] = Some(face);
    m.chosen = c;
    let [Some(a), Some(b), Some(cc)] = c else { return Some(true) };
    let name = |f: u32| m.names.get(f as usize).cloned().unwrap_or_default();
    let args = format!("A={} B={} C={}", name(a), name(b), name(cc));
    if let Err(why) = datum(view, model, &args) {
        let m = view.deviation.as_mut()?;
        m.datums = None;
        reframe(m);
        m.frame = Some(Err(why));
        retint(m, model);
    }
    Some(true)
}

// ---- VUGFACEDEVIATIONSOLIDFAULT (LEDGER SR565): a replay failure no solid reads does not refuse the map ----
// doc_core `replay` keeps going past a failure (each by line, every solid it could build kept). [`design`] refuses only
// on a failure whose statement journal.rs `solid_stmts` holds (THE "feeds a solid" rule [`follow`] already reads): a
// solid missing from the design would paint the scan's own surface as deviation, a broken document read as a bad part.
// Every other failure is kept on the map, named on the corner (`fault line N: <fault> <id>`) and counted
// (` faults=<m>` on the journal's line, a `faults=<m>` line on a path map); [`lazy`] recounts without a re-measure.

/// `failures` split into (those on a statement a solid reads, the rest), each in replay's (line) order. A failure with
/// no id counts as a solid's.
pub(crate) fn split_faults(doc: &Doc, failures: Vec<doc_core::Failure>) -> (Vec<doc_core::Failure>, Vec<doc_core::Failure>) {
    let solid = crate::journal::solid_stmts(doc);
    failures.into_iter().partition(|f| f.id.is_empty() || solid.iter().any(|s| s.id() == Some(f.id.as_str())))
}

/// A failure as the refusal names it: `line N: <fault> <id>`.
fn fault_text(f: &doc_core::Failure) -> String {
    format!("line {}: {:?} {}", f.line, f.fault, f.id)
}

/// ` faults=<m>` when the map stood beside any failure, else nothing (the journal line's tail).
fn fault_count(map: &Map) -> String {
    if map.faults.is_empty() { String::new() } else { format!(" faults={}", map.faults.len()) }
}

/// The corner's fault lines: `fault <line N: fault id>` for the first [`WORST`], then `faults=<m>` on a path map (a
/// journal map carries the count on its source line). None without a failure.
fn fault_lines(map: &Map) -> Vec<String> {
    let mut out: Vec<String> = held_line(map).into_iter().chain(map.faults.iter().take(WORST).map(|f| format!("fault {}", fault_text(f)))).collect(); // VUGFACEDEVIATIONHELD (SR600): `held=<id>` first
    if map.source.is_none() && !map.faults.is_empty() {
        out.push(format!("faults={}", map.faults.len()));
    }
    out
}

/// The journal map's faults re-read from the journal's document (one replay, no measurement): the failures no solid
/// reads. Called by [`lazy`], where no solid statement moved.
fn refault(view: &mut View) {
    let doc = &view.design.journal.doc;
    let Some(m) = view.deviation.as_mut() else { return };
    let (_, faults, held) = held_split(doc, doc_core::replay(doc, doc.tol).failures); m.faults = faults; m.held = held; // VUGFACEDEVIATIONHELD (SR600): the held chain stays counted
}

/// The corner's partial frame line while datums are being picked: `frame A=<face|-> B=<face|-> C=<face|->`; none when
/// nothing is picked.
fn chosen_line(map: &Map) -> Option<String> {
    if map.chosen.iter().all(Option::is_none) {
        return None;
    }
    let name = |f: Option<u32>| f.and_then(|f| map.names.get(f as usize)).map_or("-", String::as_str);
    Some(format!("frame A={} B={} C={}", name(map.chosen[0]), name(map.chosen[1]), name(map.chosen[2])))
}

/// The corner's lost-callout lines: `callout lost <face>` per face a re-measure dropped.
fn lost_lines(map: &Map) -> Vec<String> {
    map.lost.iter().map(|f| format!("callout lost {f}")).collect()
}

/// A command (or a datum key) is the person's next word: the lost-callout lines are cleared.
fn forget(view: &mut View) {
    if let Some(m) = view.deviation.as_mut() {
        m.lost.clear();
        m.reported = None; // VUGFACEGDTREPORTKEY (SR575): the report line is shown once
    }
}

/// After [`carry`]: every old callout face the new design does not name joins the old map's `lost` (deduped), and the
/// partial picks follow by face name (a picked face that is gone is unpicked).
fn carry_named(old: &Map, new: &mut Map) {
    let name = |f: u32| old.names.get(f as usize).cloned();
    let mut lost = old.lost.clone();
    for c in &old.callouts {
        let Some(n) = name(c.face) else { continue };
        if !new.names.contains(&n) && !lost.contains(&n) {
            lost.push(n);
        }
    }
    new.lost = lost;
    new.chosen = old.chosen.map(|f| f.and_then(name).and_then(|n| new.names.iter().position(|x| *x == n).map(|i| i as u32)));
}

// ---- VUGFACEJOURNALSOLIDFAULT (LEDGER SR572): a click on a corner fault line reaches its statement ----
// The corner's lines sit at ONE geometry ([`line_at`], what [`overlay`] paints with). `View::release`'s Pick arm asks
// [`fault_click`] first: on a journal map (a path map's lines are another document's) a click inside a line's rect
// (8 px a glyph, [`LINE_PX`] high) whose text is `fault line N: ...` selects line N in the panel through journal.rs
// `Journal::select_line` (the panel's cursor). One rect per line, no replay, no BVH.

/// Where corner line `i` (its painted, upper-cased `text`) is drawn on a frame `width` wide: right-aligned 6 px in,
/// [`LINE_PX`] apart from 6 px down, clamped to the left edge.
pub fn line_at(width: u32, text: &str, i: usize) -> (f32, f32) {
    let x = width as f32 - 8.0 * text.chars().count() as f32 - 6.0;
    (x.max(0.0), 6.0 + LINE_PX * i as f32)
}

/// A click at (x, y) on a journal map's `fault line N` corner line selects statement N in the panel; true when it did
/// (the click is taken). Any other click, or a map with no journal, is not taken.
pub fn fault_click(view: &mut View, x: f64, y: f64) -> bool {
    let Some(m) = view.deviation.as_ref().filter(|m| m.source.is_some()) else { return false };
    let w = corner_origin(view, view.size.0); // VUGFACEJOURNALFAULTFIX (SR578): the paint's right edge, so the hit stays one geometry
    let line = corner(m).into_iter().enumerate().find_map(|(i, l)| {
        let text = l.to_uppercase();
        let (x0, y0) = line_at(w, &text, i);
        let inside = x as f32 >= x0 && (x as f32) < x0 + 8.0 * text.chars().count() as f32 && y as f32 >= y0 && (y as f32) < y0 + LINE_PX;
        if !inside {
            return None;
        }
        l.strip_prefix("fault line ")?.split(':').next()?.parse::<u32>().ok()
    });
    match line {
        Some(n) => view.design.journal.select_line(n),
        None => false,
    }
}

// ---- VUGFACEGDTREPORT (LEDGER SR571): the verdicts written beside the part, the corner's own lines ----
// [`report_lines`] is the frame line and the callout lines the corner paints ([`frame_line`], [`gdt_lines`], their strings
// scan_core's ONE `gdt::callout_line` / `gdt::frame_line`); `Viewer::report` writes them as scan_core `gdt::report_text`
// to `<part>.vugreport` beside the journal's part (`Viewer::save_to`), through lib.rs's atomic writer. tools/scan's
// `--vugreport` writes the same text from its own reading. A map with no part path (a path map, or a journal never saved)
// refuses [`REPORT_NO_PATH`]. BP-vug-5: a printed verdict, nothing changes.

/// The extension the report takes beside the part.
pub const REPORT_EXT: &str = "vugreport";
/// The refusal when the map has no part to write beside.
pub const REPORT_NO_PATH: &str = "ReportNoPath";

/// The report's lines: the corner's frame line (if any) and one line per callout, in the order named.
pub fn report_lines(map: &Map) -> Vec<String> {
    let mut out: Vec<String> = frame_line(map).into_iter().collect();
    out.extend(gdt_lines(map));
    out
}

impl Viewer {
    /// Write the map's verdicts to `<part>.vugreport` beside the journal's part: the header (the part's file name, the
    /// generation the map shows, scan_core `gdt::scan_tag` of the scan) and [`report_lines`]. Refused by name, nothing
    /// written: no map, a refused map, [`REPORT_NO_PATH`] (neither a saved journal part nor a path map's design: [`report_target`]). The path written on success.
    pub fn report(&self) -> Result<std::path::PathBuf, String> {
        let m = self.view.deviation.as_ref().ok_or("report: no map")?;
        if let Err(e) = &m.result {
            return Err(format!("report: refused map {e:?}"));
        }
        let Some((path, name, generation)) = report_target(self, m) else { return Err(REPORT_NO_PATH.to_string()) }; // VUGFACEGDTREPORTKEY (SR575): a path map writes beside its design
        let text = scan_core::gdt::report_text(&name, generation, &scan_label(m), &report_lines(m)); // VUGFACEGDTREPORTKEY (SR575): the scan's file name when held
        crate::write_atomic(&path, &text)?;
        Ok(path)
    }
}


// ---- VUGFACEJOURNALFAULTFIX (LEDGER SR578): the corner left of the shown panel ----
// The shown tree panel covers the frame's right quarter, so a right-aligned corner line (a `fault line N` the person just
// clicked to open the panel) would hide under it. [`corner_origin`] is the corner's ONE right edge: [`overlay`] paints
// at it and [`fault_click`] hit-tests at it, both through [`line_at`].

/// The right edge the corner's lines align to on a frame `width` wide: the frame's edge, or journal.rs `panel_x0` (the
/// panel's left edge) while the panel is shown.
pub fn corner_origin(view: &View, width: u32) -> u32 {
    if view.design.journal.panel { crate::journal::panel_x0(width) as u32 } else { width }
}

// ---- VUGFACEGDTREPORTKEY (LEDGER SR575): the report on the map's command road, the names it is written under ----
// VUGFACEGDTREPORT (SR571) left `Viewer::report` with no caller in the face: [`Viewer::map_command`] is the map's road
// with `report` on it (every other line is [`command`]'s), [`Viewer::report_command`] writes and the corner reads
// `report <path>` until the next command ([`forget`]); vug-view `--report` calls it. The Map holds the scan's file name
// and a path map's design path (lib.rs `open_against` / `open_scan` through [`Viewer::hold_names`]): the header reads
// `scan=<name>` (scan_core `gdt::scan_tag` without one) and a path map's report is written beside its design.
// BP-vug-5: a printed verdict, nothing changes.

/// Where the map's report goes and what it is headed: a journal map with a part path writes `<part>.vugreport` at the
/// generation it shows; a path map with its design path writes `<design>.vugreport` beside the design at generation 0 (a
/// document read once, as tools/scan's); else None ([`REPORT_NO_PATH`]).
fn report_target(v: &Viewer, m: &Map) -> Option<(std::path::PathBuf, String, u64)> {
    let (file, generation) = match (&m.source, v.save_to.as_ref(), m.design_path.as_ref()) {
        (Some(src), Some(part), _) => (part, src.measured),
        (None, _, Some(design)) => (design, 0),
        _ => return None,
    };
    let name = file.file_name().and_then(|s| s.to_str()).unwrap_or("vug").to_string();
    Some((file.with_extension(REPORT_EXT), name, generation))
}

/// The report header's scan: its file name when the map holds one, else scan_core `gdt::scan_tag` of its mesh.
fn scan_label(m: &Map) -> String {
    m.scan_name.clone().unwrap_or_else(|| scan_core::gdt::scan_tag(&m.basis.0))
}

/// The corner's report line: `report <path>` after the `report` command, until the next command.
fn reported_line(m: &Map) -> Option<String> {
    m.reported.as_ref().map(|p| format!("report {}", p.display()))
}

/// A re-measure ([`follow`]) keeps the names the old map was opened under.
fn keep_names(old: &Map, new: &mut Map) {
    new.scan_name = old.scan_name.clone();
    new.design_path = old.design_path.clone();
}

impl Viewer {
    /// One command on the map: `report` writes the report ([`Viewer::report_command`]); any other line is
    /// [`command`]'s. True when the map changed; refused by name, as each road refuses.
    pub fn map_command(&mut self, line: &str) -> Result<bool, String> {
        if line.trim() == "report open" { return self.report_open().map(|_| true); } if let Some(p) = line.trim().strip_prefix("report open ") { let p = crate::journal::beside(self, p.trim()); return self.report_open_path(&p).map(|_| true); } if line.trim() == "report" { // VUGFACEPATHBESIDE (SR594): the typed path through THE rule (journal.rs `beside`), where Save As puts it; VUGFACECOMMANDCARET (SR587): `report open <path>` reads any report file; VUGFACEGDTREPORTOPEN (SR581): `report open` shows the written report as a sheet
            return self.report_command().map(|_| true);
        }
        command(&mut self.view, &self.model, line)
    }

    /// Write the report ([`Viewer::report`]); on success the corner reads `report <path>` until the next command. The
    /// path written, or the refusal by name (nothing written, the corner's report line cleared).
    pub fn report_command(&mut self) -> Result<std::path::PathBuf, String> {
        forget(&mut self.view);
        let path = self.report()?;
        if let Some(m) = self.view.deviation.as_mut() {
            m.reported = Some(path.clone());
        }
        Ok(path)
    }

    /// Hold the scan's file name and a path map's design path on the map (lib.rs's open roads).
    pub(crate) fn hold_names(&mut self, scan_name: Option<&str>, design: Option<&str>) {
        if let Some(m) = self.view.deviation.as_mut() {
            m.scan_name = scan_name.map(String::from);
            m.design_path = design.map(std::path::PathBuf::from);
        }
    }
}

// ---- VUGFACEGDTREPORTOPEN (LEDGER SR581): the report opened as a sheet, the `:` command line ----
// VUGFACEGDTREPORTKEY (SR575) left the `.vugreport` unread in the face and `map_command` reachable only from the flags.
// `report open` ([`Viewer::report_open`], on [`Viewer::map_command`]'s road) reads the file `report` writes
// ([`report_target`]) into a [`Sheet`] on the View, painted by journal.rs's ONE panel painter (`paint_panel`), read-only.
// `:` opens the [`Entry`] when a map or a sheet is up and nothing else holds the keys; its keys are lib.rs `Viewer::key`'s
// first ([`Viewer::line_key`], ahead of the Save As field: a typed `s` is the entry's, never the save); its typing is
// sketch.rs `type_text_at` and its paint journal.rs `field_line_at` (the Save As field's too). Enter runs the line through
// [`Viewer::map_command`]. BP-vug-5: a printed verdict, nothing changes.

/// An opened `.vugreport`: its lines as read and the first row shown. Read-only: Escape closes, Up / Down scroll.
#[derive(Clone, Debug, Default)]
pub struct Sheet {
    pub lines: Vec<String>,
    pub top: usize,
}

/// The `:` command line: open or not, the typed text, the last line run (Up recalls it) and the last refusal (shown
/// above the line until the next edit).
#[derive(Clone, Debug, Default)]
pub struct Entry {
    pub open: bool,
    pub text: String,
    pub last: Option<String>,
    pub refused: Option<String>,
    /// VUGFACECOMMANDCARET (SR587): the caret, in chars of `text` (sketch.rs `type_text_at` / `caret_key`).
    pub caret: usize,
    /// VUGFACEPHOTOPREFLINE (SR595): the store folder a photo line writes (photo.rs `line_command`); None is
    /// Principia's standard store (`photo::command`), a test sets its own folder.
    pub store: Option<std::path::PathBuf>,
}

impl Viewer {
    /// Read the map's report (the file [`Viewer::report`] writes: [`report_target`]) into the sheet, shown in the panel
    /// until Escape. Refused by name, nothing shown: no map, a refused map, [`REPORT_NO_PATH`], the read's error. The
    /// path read on success.
    pub fn report_open(&mut self) -> Result<std::path::PathBuf, String> {
        forget(&mut self.view);
        let m = self.view.deviation.as_ref().ok_or("report open: no map")?;
        if let Err(e) = &m.result {
            return Err(format!("report open: refused map {e:?}"));
        }
        let Some((path, _, _)) = report_target(self, m) else { return Err(REPORT_NO_PATH.to_string()) };
        self.report_open_path(&path) // VUGFACECOMMANDCARET (SR587): the one sheet road
    }

    /// The command line's and the sheet's keys, before every other (lib.rs `Viewer::key`); None while the Save As field
    /// is open and for every key neither takes. The open entry takes EVERY key: a character is typed, Backspace pops, Up
    /// recalls the last line run, Escape closes, Enter runs the line through [`Viewer::map_command`] (run: the line is
    /// the last, the entry closes; refused: it stays open naming the refusal). `:` opens it while a map or a sheet is up
    /// (VUGFACESAVEASCARET SR593: with no map the map's words refuse by name) and no
    /// sketch face, edit, pull, gesture or edge tool holds the keys. The shown sheet takes Escape (closes) and Up / Down.
    pub fn line_key(&mut self, k: &str) -> Option<bool> {
        if self.view.design.journal.save_as.is_some() {
            return None;
        }
        let esc = matches!(k, "Escape" | "\u{1b}");
        let (up, down) = (matches!(k, "Up" | "\u{f700}"), matches!(k, "Down" | "\u{f701}"));
        if self.view.entry.open {
            if esc {
                let e = &mut self.view.entry;
                e.open = false;
                e.text.clear();
                e.caret = 0; // VUGFACECOMMANDCARET (SR587)
                e.refused = None;
                return Some(true);
            }
            if matches!(k, "Return" | "KP_Enter" | "\r" | "\n") {
                let line = self.view.entry.text.clone();
                match crate::photo::line_command(&mut self.view, &line).unwrap_or_else(|| self.map_command(&line)) { // VUGFACEPHOTOPREFLINE (SR595): a photo's line runs the photo's command road, the rest the map's
                    Ok(_) => {
                        let e = &mut self.view.entry;
                        e.last = Some(line);
                        e.text.clear();
                        e.caret = 0; // VUGFACECOMMANDCARET (SR587)
                        e.open = false;
                        e.refused = None;
                    }
                    Err(r) => self.view.entry.refused = Some(r),
                }
                return Some(true);
            }
            let e = &mut self.view.entry;
            if up {
                let Some(l) = e.last.clone() else { return Some(false) };
                e.caret = l.chars().count(); // VUGFACECOMMANDCARET (SR587): the recalled line's end
                e.text = l;
                e.refused = None;
                return Some(true);
            }
            if let Some(moved) = crate::sketch::caret_key(&e.text, &mut e.caret, k) {
                return Some(moved); // VUGFACECOMMANDCARET (SR587): Left / Right / Home / End move the caret, never typed
            }
            let back = matches!(k, "BackSpace" | "Backspace" | "\u{8}" | "\u{7f}");
            let del = matches!(k, "Delete" | "\u{f728}"); // VUGFACECOMMANDCARET (SR587): the char after the caret
            let ch = {
                let mut it = k.chars();
                match (it.next(), it.next()) {
                    (Some(c), None) => Some(c),
                    _ => None,
                }
            };
            let typed = crate::sketch::type_text_at(&mut e.text, &mut e.caret, ch, back, del); // VUGFACECOMMANDCARET (SR587): at the caret
            if typed {
                e.refused = None;
            }
            return Some(typed);
        }
        let free = (self.view.deviation.is_some() || self.view.sheet.is_some()) && self.view.design.face.is_none() && !crate::journal::busy(&self.view.design) && self.view.fillet.is_none() || crate::photo::line_free(&self.view); // VUGFACESAVEASCARET (SR593): a sheet with no map opens the line too // VUGFACEPHOTOPREFLINE (SR595): `:` opens over a photo too (its sketch face is open)
        if k == ":" && free {
            let e = &mut self.view.entry;
            e.open = true;
            e.text.clear();
            e.caret = 0; // VUGFACECOMMANDCARET (SR587)
            e.refused = None;
            return Some(true);
        }
        let fit = crate::journal::panel_fit(self.view.size.1);
        let s = self.view.sheet.as_mut()?;
        if esc {
            self.view.sheet = None;
            return Some(true);
        }
        if !(up || down) {
            return None;
        }
        let was = s.top;
        s.top = if up { was.saturating_sub(1) } else { (was + 1).min(s.lines.len().saturating_sub(fit)) };
        Some(was != s.top)
    }
}

/// The sheet in the panel (journal.rs `paint_panel`: its lines upper-cased in the primary text colour from its top row)
/// and the open command line bottom left (journal.rs `field_line_at`, label `:`), its refusal one row above in the critical
/// status colour.
pub fn line_overlay(view: &View, fr: &mut Frame) {
    if let Some(s) = &view.sheet {
        let ink = rgba(palette_core::TEXT_PRIMARY.1);
        crate::journal::paint_panel(fr, &s.lines, s.top, &|_, l| (l.to_uppercase(), ink, false));
    }
    let e = &view.entry;
    if !e.open {
        return;
    }
    let panel = view.design.journal.panel;
    crate::journal::field_line_at(fr, ":", &e.text, e.caret, panel); // VUGFACECOMMANDCARET (SR587): the caret where it is
    if let Some(r) = &e.refused {
        let (w, h) = (fr.width as f32, fr.height as f32);
        let cols = ((w - 12.0) / 8.0).floor().max(1.0) as usize;
        let text: String = r.to_uppercase().chars().take(cols).collect();
        let y = h - 3.0 * crate::journal::ROW_PX - if panel { crate::journal::ROW_PX } else { 0.0 };
        raster::text(fr, &text, 6.0, y, rgba(palette_core::STATUS_CRITICAL));
    }
}

// ---- VUGFACECOMMANDCARET (LEDGER SR587): any report file as the sheet, with or without a map ----
// VUGFACEGDTREPORTOPEN (SR581) read only the map's own report. [`Viewer::report_open_path`] is THE sheet road: the
// map's `report open` ([`Viewer::report_open`]), the typed `report open <path>` ([`Viewer::map_command`], beside the opened document: journal.rs `beside`, SR594)
// and main.rs's `.vugreport` open ([`Viewer::open_report`], an empty document's face with no map) all read through it.
// BP-vug-5: a printed verdict, nothing changes.

impl Viewer {
    /// Read the report file at `path` into the sheet, shown in the panel until Escape (no map needed). Refused by name,
    /// nothing shown: the read's error. The path read on success.
    pub fn report_open_path(&mut self, path: &std::path::Path) -> Result<std::path::PathBuf, String> {
        forget(&mut self.view);
        let text = std::fs::read_to_string(path).map_err(|e| format!("report open: {}: {e}", path.display()))?;
        self.view.sheet = Some(Sheet { lines: text.lines().map(String::from).collect(), top: 0 });
        Ok(path.to_path_buf())
    }

    /// A report file opened on its own (main.rs: `vug-view x.vugreport`): an empty document's face (lib.rs
    /// `Viewer::open_doc`, no map) titled by the file name, the file shown as the sheet ([`Viewer::report_open_path`]).
    pub fn open_report(path: &str, size: (u32, u32), preset: crate::view::Preset) -> Result<Viewer, String> {
        let p = std::path::Path::new(path);
        let title = p.file_name().and_then(|s| s.to_str()).unwrap_or("vug").to_string();
        let mut v = Viewer::open_doc(Doc::new(doc_core::Tol::DEFAULT), &title, size, preset)?;
        v.report_open_path(p)?;
        Ok(v)
    }
}


// ---- VUGFACECONFIDENCETINT (LEDGER SR592): the confidence floor on the map, a low-weight vertex tinted unmeasured ----
// VUGSCANTSDFCONFIDENCE (SR586) built scan_core's ONE floor rule `deviation::floored` (a paired vertex whose fused weight,
// `ScanMesh::confidence`, is under the floor is unpaired; stats and faces stay as measured) and left the face without it.
// `conf <w>` ([`conf_command`], on [`command`]'s road: the flags, `Viewer::map_command`, the `:` line) holds the floor on
// the Map ([`Conf`]) and re-reads the measurement through `floored` ([`refloor`]), so `result` is the floored one and every
// reader inherits it: [`reframe`] re-reads the gdt and the frame, [`retint`] paints an unpaired vertex `colour(Shade::Off)`,
// the one unmeasured tint, instead of judging it against the band. The corner reads `conf floor=<w> unpaired=<n>`
// ([`conf_line`]); `conf off` restores the measurement as taken. [`follow`] carries the floor into a re-measure
// ([`keep_conf`], ahead of [`carry`]'s reframe and retint). BP-vug-5: a printed reading, the scan and the design unchanged.

/// The floor named on the map: the weight under which a paired vertex is unpaired, the measurement before the floor (what
/// `conf off` or a new floor re-reads from) and the count of paired vertices the floor unpaired.
#[derive(Clone, Debug)]
pub struct Conf {
    pub floor: f32,
    pub raw: Deviation,
    pub unpaired: u32,
}

/// `conf <w>` names the floor (`w` finite and above 0), `conf off` removes it; any other line is not the floor's (None).
/// Refused by name, nothing changed: no map, a refused map, anything else the usage. True when the map changed.
fn conf_command(view: &mut View, model: &Model, line: &str) -> Option<Result<bool, String>> {
    const USAGE: &str = "usage: conf <floor> | conf off";
    let rest = line.trim().strip_prefix("conf")?;
    if !(rest.is_empty() || rest.starts_with(' ')) {
        return None;
    }
    let rest = rest.trim();
    let Some(m) = view.deviation.as_mut() else { return Some(Err("conf: no map".to_string())) };
    if let Err(e) = &m.result {
        return Some(Err(format!("conf: refused map {e:?}")));
    }
    let floor = if rest == "off" {
        None
    } else {
        match rest.parse::<f32>() {
            Ok(w) if w.is_finite() && w > 0.0 => Some(w),
            _ => return Some(Err(USAGE.to_string())),
        }
    };
    let changed = m.conf.as_ref().map(|c| c.floor) != floor;
    refloor(m, floor);
    reframe(m);
    retint(m, model);
    Some(Ok(changed))
}

/// The map's measurement re-read at `floor`: the measurement as taken (held in [`Conf`] while a floor is on) through
/// scan_core `deviation::floored`, the count it unpaired kept; None restores the measurement as taken. A refused map is
/// left as it is. The caller re-reads the gdt and re-tints ([`reframe`], [`retint`]).
fn refloor(map: &mut Map, floor: Option<f32>) {
    let raw = match map.conf.take() {
        Some(c) => c.raw,
        None => match &map.result {
            Ok(d) => d.clone(),
            Err(_) => return,
        },
    };
    let Some(w) = floor else {
        map.result = Ok(raw);
        return;
    };
    let d = scan_core::deviation::floored(&raw, &map.basis.0, w);
    let unpaired = raw.per_vertex.iter().zip(&d.per_vertex).filter(|(a, b)| a.is_some() && b.is_none()).count() as u32;
    map.result = Ok(d);
    map.conf = Some(Conf { floor: w, raw, unpaired });
}

/// A re-measure ([`follow`]) keeps the old map's floor: the new measurement re-read at it, before [`carry`] re-reads the
/// gdt and re-tints.
fn keep_conf(old: &Map, new: &mut Map) {
    if let Some(c) = &old.conf {
        refloor(new, Some(c.floor));
    }
}

/// The corner's floor line: `conf floor=<w> unpaired=<n>` while a floor is on.
fn conf_line(map: &Map) -> Option<String> {
    map.conf.as_ref().map(|c| format!("conf floor={} unpaired={}", c.floor, c.unpaired))
}

// ---- VUGFACEDEVIATIONHELD (LEDGER SR600): the map opens beside a held statement ----
// VUGFACEJOURNALSOLIDFAULTFIX (SR598) made journal.rs `regenerate` hold ONE solid root fault (`hold`) and paint the Body
// from the `standing` document, but [`design`] ran its own replay through SR565's `split_faults` alone, so the map
// refused the document the journal held (`against_journal`, `open_against`). [`held_split`] is the same pair regenerate
// reads: the design is the standing document's solids, the held chain is carried in `Map::faults` (and [`refault`]
// keeps it), the root named on the corner `held=<id>`. The fix's statement replace moves `solid_stmts`, so [`follow`]
// re-measures and the held word, re-derived per build, is gone. Two roots and the rest still refuse by the first line.

/// `failures` split by [`split_faults`] then journal.rs `hold`: (the solid half, the carried, the held root's id). The
/// root is the solid half's first failure that is not `Upstream`, kept only when the hold emptied that half.
fn held_split(doc: &Doc, failures: Vec<doc_core::Failure>) -> (Vec<doc_core::Failure>, Vec<doc_core::Failure>, Option<String>) {
    let split = split_faults(doc, failures);
    let root = split.0.iter().find(|f| !matches!(f.fault, doc_core::Fault::Upstream(_))).map(|f| f.id.clone());
    let (on_solid, carried) = crate::journal::hold(split);
    let held = if on_solid.is_empty() { root } else { None };
    (on_solid, carried, held)
}

/// The corner's held line: `held=<id>` while the design stands beside a held statement.
fn held_line(map: &Map) -> Option<String> {
    map.held.as_ref().map(|id| format!("held={id}"))
}
