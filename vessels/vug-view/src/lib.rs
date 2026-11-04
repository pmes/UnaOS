// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! CHARTER: Vug (CODEX §2, 3D), a host face — the viewport over mesh_core: draws a Mesh, orbits about the cursor,
//! picks with mesh_core's BVH; holds no geometry.
//!
//! `vug-view` (VUGFACE1, LEDGER SR470) is Vug's first face, in facet-view's shape: a controller between quartzite's
//! image surface and the model. [`Viewer`] turns window input (the left button with its modifiers as `BrowserButton`, the
//! right and middle as `BrowserAuxButton` and the pinch as `BrowserPinch` (VUG-FACE-BUTTONS, LEDGER SR477), the pointer
//! as `BrowserMouseMove`, the wheel as `BrowserScroll`, keys as `BrowserText` / `BrowserKey`, size as
//! `BrowserResize` — what quartzite's image surface emits) into view changes and answers with one `SurfaceBlit` per
//! settled burst of input. `model` is the ONE seam to mesh_core, `view` the navigation, `raster` the drawing.

pub mod extrude;
pub mod journal; // VUGFACETREE (SR513): the document, undo / redo, the tree panel
pub mod model;
pub mod photo; // VUGFACEPHOTO (SR506): the photo underlay and the proposal glyphs
pub mod raster;
pub mod sketch;
pub mod view;
// VUGFACEFILLET (SR511): the edge tool — fillet / chamfer an edge of the body.
pub mod fillet;
pub mod deviation; // VUGFACEDEVIATION (SR525): a scan against its design document, in the band's colours

use bandy::SMessage;
use model::Model;
use view::{Button, Preset, View};

/// The surface this vessel paints (quartzite's image surface id).
pub const SURFACE: &str = "vug://view";

/// The shortcuts, for `--help`.
pub const SHORTCUTS: &[(&str, &str)] = &[
    ("h / 0", "Home (isometric)"),
    ("1", "front view"),
    ("3", "right view"),
    ("7", "top view"),
    ("p", "next navigation preset (inside a sketch: the point tool)"),
    // VUGFACE2 (SR485): the sketch face's keys.
    ("l / c / a", "sketch a line / circle / arc (opens a sketch on the face under the cursor, else the plane facing you)"),
    ("e", "extrude the profile under the cursor: drag the distance, click to commit"),
    ("d", "dimension: a line, two points, a point and a line, two lines, a circle (d again: radius / diameter); type the value, Enter; while pulling, type the distance, Enter"), // VUGFACEDIMENSION (SR494)
    ("Esc", "back to select (again: clear the selection; cancels a pull)"),
    ("click", "pick a face"),
    ("middle / right drag", "orbit / pan / zoom per the preset"),
    ("Alt+drag", "the middle button, for a trackpad (orbit / pan / zoom per the preset)"),
    ("wheel / pinch", "zoom to the cursor"),
    ("f / Shift+f", "fillet / chamfer the body edge under the cursor (a click re-picks): type r (chamfer d1,d2), Enter; Esc cancels"), // VUGFACEFILLET (SR511)
    // VUGFACEPHOTO (SR506): a photo on graph paper opens as its measured sketch over the dimmed photo.
    ("i", "ask Vein for design intent on the photo's sketch (over the bus; the corner shows the request, then the proposals or the refusal)"), // VUGFACEINTENTSEND (SR514)
    ("Tab", "focus the next proposal"),
    ("Enter / Backspace", "on the focused proposal: apply it (verified against the photo) / reject it"),
    ("Escape", "on a focused proposal set (one card): reject the whole set (Backspace too); off a card, the sketch's Esc"), // VUGINTENTSINGLERENAMED (SR577): VUG-FACE-KEYS-CARD
    // VUGFACETREE (SR513): the feature tree over doc_core.
    ("t", "the feature tree panel (the document's statements)"),
    ("u / U", "undo / redo (the document's snapshots; the face regenerates)"),
    ("Up / Down, Delete, Enter", "in the tree panel: move, delete the row with its dependants, edit its value (type, Enter)"),
    ("s / Cmd+S", "save the document: the opened .vug, else <file stem>.vug beside the opened file (a sibling .tmp renamed over it); the title shows * while unsaved; a write error is named in the corner"), // VUGFACESAVE (SR534)
    ("Cmd+Shift+S", "save as: type a path (relative: beside the opened file; no extension: .vug), Enter; an existing file is refused once, Enter again overwrites; Esc or Cmd+Shift+S cancels"), // VUGFACESAVEAS (SR539)
    ("d (outside a sketch)", "the deviation map on / off (a scan opened with --against <design.vugdoc>; the corner names the worst faces)"), // VUGFACEDEVIATION (SR525)
];

/// The controller: owns the model and the view (one task, no lock).
pub struct Viewer {
    pub model: Model,
    pub view: View,
    /// The window title: the file name, with `*` while the document differs from the last saved text (VUGFACESAVE).
    pub title: String,
    // VUGFACESAVE (SR534): where `s` writes (the opened .vug, else `<stem>.vug` beside the opened file), whether the
    // face opened or wrote that file (an existing file it did not is refused by name), the bare title.
    pub save_to: Option<std::path::PathBuf>,
    claimed: bool,
    name: String,
}

impl Viewer {
    pub fn new(model: Model, title: &str, size: (u32, u32), preset: Preset) -> Viewer {
        let view = View::new(&model, size, preset);
        Viewer { model, view, title: title.to_string(), save_to: None, claimed: false, name: title.to_string() }
    }

    /// VUGFACEPHOTO (SR506): THE open road — a .png / .jpg / .jpeg (else any file pixel_core recognises that is not a
    /// mesh by extension) is a photo on graph paper measured at `photo::PITCH_MM`; anything else is a mesh
    /// (`Model::open`). The title is the file name. No camera prefs ([`Viewer::open_with`]'s default).
    pub fn open(path: &str, size: (u32, u32), preset: Preset) -> Result<Viewer, String> {
        Viewer::open_with(path, size, preset, photo::PhotoPrefs::default())
    }

    /// VUGFACEPHOTOCAMERAPREF (SR576): [`Viewer::open`] with the camera's prefs (main.rs reads them once from
    /// Principia's store) handed to a photo's open road.
    pub fn open_with(path: &str, size: (u32, u32), preset: Preset, prefs: photo::PhotoPrefs) -> Result<Viewer, String> {
        let p = std::path::Path::new(path);
        let title = p.file_name().and_then(|s| s.to_str()).unwrap_or("vug").to_string();
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        // VUGFACESAVE (SR534): a .vug document opens as itself (and saves back over itself).
        if ext == "vug" {
            return Viewer::open_vug(path, &title, size, preset);
        }
        let mesh = matches!(ext.as_str(), "stl" | "ply" | "obj");
        let mut v = None;
        if !mesh {
            let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
            if matches!(ext.as_str(), "png" | "jpg" | "jpeg") || pixel_core::sniff(&bytes).is_some() {
                v = Some(Viewer::open_photo_with(&bytes, &title, size, preset, photo::PITCH_MM, prefs).map_err(|e| format!("{path}: {e}"))?); // VUGFACEPHOTOCAMERAPREF (SR576)
            }
        }
        let mut v = match v {
            Some(v) => v,
            None => Viewer::new(Model::open(path)?, &title, size, preset),
        };
        v.save_to = Some(p.with_extension("vug")); // VUGFACESAVE (SR534): `<stem>.vug` beside a mesh / photo
        Ok(v)
    }

    /// VUGFACEPHOTO (SR506): a photo's bytes on `pitch_mm` graph paper: photo_core measures it, the paper is the drawn
    /// model, the measured sketch is open (Select) on the XY plane, seen from the top. No camera prefs.
    pub fn open_photo(bytes: &[u8], title: &str, size: (u32, u32), preset: Preset, pitch_mm: f64) -> Result<Viewer, String> {
        Viewer::open_photo_with(bytes, title, size, preset, pitch_mm, photo::PhotoPrefs::default())
    }

    /// VUGFACEPHOTOCAMERAPREF (SR576): [`Viewer::open_photo`] under the camera's prefs (`photo::open_with`).
    pub fn open_photo_with(bytes: &[u8], title: &str, size: (u32, u32), preset: Preset, pitch_mm: f64, prefs: photo::PhotoPrefs) -> Result<Viewer, String> {
        let (model, face, photo) = photo::open_with(bytes, pitch_mm, prefs)?;
        let mut v = Viewer::new(model, title, size, preset);
        v.view.top(&v.model);
        v.view.design.face = Some(face);
        v.view.photo = Some(photo);
        Ok(v)
    }

    /// VUGFACEDEVIATION (SR525): THE deviation road — a scan mesh (.ply / .stl / .obj through model.rs's readers; a
    /// .prb is refused by name: fusion is tools/scan's) measured against a design document (doc_core `read`), drawn at
    /// the aligned pose in the band's colours. The title is the scan's file name.
    pub fn open_against(scan: &str, design: &str, size: (u32, u32), preset: Preset) -> Result<Viewer, String> {
        let (title, mesh) = read_scan(scan)?; // VUGFACEDEVIATIONSOLIDFAULT (SR565): the one scan reader, shared with open_scan
        let text = std::fs::read_to_string(design).map_err(|e| format!("{design}: {e}"))?;
        let doc = doc_core::read_with(&text, &doc_host::DirLib::beside(std::path::Path::new(design))).map_err(|e| format!("{design}: {e:?}"))?; // VUGMAKEMACHINELIBFACE (SR548)
        Viewer::open_deviation(deviation::scan_mesh(mesh), &doc, &title, size, preset, None).map_err(|e| format!("{design}: {e}")) // VUGFACEGDTREMEASURE (SR560): a PLY carries no truncation
    }

    /// VUGFACEDEVIATION (SR525): a scan measured against `doc` at `deviation::BAND_MM` (the test door): the model is the
    /// scan at the aligned pose (as read on a refusal), the map the view's. VUGFACEGDTREMEASURE (SR560): `margin_mm` is
    /// the gdt edge margin, the fused volume's own truncation when the caller fused the scan (`Tsdf::trunc_mm()`); None
    /// is `deviation::GDT_MARGIN_MM`, the fallback, not the rule.
    pub fn open_deviation(scan: scan_core::ScanMesh, doc: &doc_core::Doc, title: &str, size: (u32, u32), preset: Preset, margin_mm: Option<f64>) -> Result<Viewer, String> {
        let (model, map) = deviation::open(&scan, doc, deviation::BAND_MM, margin_mm.unwrap_or(deviation::GDT_MARGIN_MM))?;
        let mut v = Viewer::new(model, title, size, preset);
        v.view.deviation = Some(map);
        Ok(v)
    }

    /// A shortcut key; true when the frame changed. VUGFACESAVE (SR534): Cmd+S (`Meta+s` on the bus) saves, and a
    /// plain `s` when no edit, pull or gesture holds the keys; the title follows the document.
    pub fn key(&mut self, k: &str) -> bool {
        if let Some(r) = self.save_as_key(k) { self.retitle(); return r; } // VUGFACESAVEAS (SR539): the open Save As field takes every key first
        let save = k == "Meta+s" || (k == "s" && !journal::busy(&self.view.design) && self.view.fillet.is_none());
        let r = if save {
            let _ = self.save();
            true
        } else {
            self.view.key(&self.model, k)
        };
        deviation::follow(self); self.retitle(); // VUGFACEDEVIATIONJOURNAL (SR550): a journal map re-measures when the document moved
        r
    }

    /// One window message; true when the frame must be redrawn (or resent).
    pub fn input(&mut self, msg: &SMessage) -> bool {
        let r = match msg {
            // The left button (Alt+Left is still the middle one, for a trackpad: `Preset::resolve`).
            SMessage::BrowserButton { down: true, x, y, mods, .. } => self.view.press(&self.model, Button::Left, *mods, *x, *y),
            SMessage::BrowserButton { down: false, x, y, .. } => self.view.release(&self.model, *x, *y),
            // VUG-FACE-BUTTONS (SR477): the right and middle buttons; an extra button (3 and up) does nothing.
            SMessage::BrowserAuxButton { button, down: true, x, y, mods, .. } => match Button::from_wire(*button) {
                Some(b) => self.view.press(&self.model, b, *mods, *x, *y),
                None => false,
            },
            SMessage::BrowserAuxButton { button, down: false, x, y, .. } => Button::from_wire(*button).is_some() && self.view.release(&self.model, *x, *y),
            SMessage::BrowserPinch { scale, x, y, .. } => self.view.pinch(&self.model, *scale, *x, *y),
            SMessage::BrowserMouseMove(x, y, _) => self.view.motion(*x, *y),
            SMessage::BrowserScroll(_, dy, _) => self.view.wheel(&self.model, *dy),
            SMessage::BrowserText(t) => t.chars().fold(false, |acc, c| self.key(&c.to_string()) | acc),
            SMessage::BrowserKey(k) => self.key(k),
            // Always a frame: the surface's first resize is also the first moment it is subscribed and can take one.
            SMessage::BrowserResize(w, h, _) => {
                self.view.resize(*w, *h);
                true
            }
            SMessage::SurfaceResend { url } => url == SURFACE,
            // VUGFACEINTENTSEND (SR514): Vein's answer to the face's ask (photo::answer matches the live request id).
            SMessage::Intent(ev) => photo::answer(&mut self.view, ev),
            _ => false,
        };
        deviation::follow(self); self.retitle(); // VUGFACEDEVIATIONJOURNAL (SR550): follow the journal; VUGFACESAVE (SR534): a release commits a stroke; the title's mark follows the document
        r
    }

    /// Render the current view as the surface's whole frame.
    pub fn frame(&self) -> SMessage {
        let f = raster::render(&self.model, &self.view);
        SMessage::SurfaceBlit { url: SURFACE.to_string(), width: f.width, height: f.height, pixels: f.rgba, damage: None, scroll: None }
    }

    /// VUGFACEINTENTSEND (SR514): THE send road — what the face asked since the last drain (`Photo::outbox`), as bus
    /// messages for the controller to fire; empty with no photo.
    pub fn outbox(&mut self) -> Vec<SMessage> {
        self.view.photo.as_mut().map(|ph| ph.outbox.drain(..).map(SMessage::Intent).collect()).unwrap_or_default()
    }
}

// ---- VUGFACESAVE (SR534): the save road ----

/// Half the side of the ground quad a document opens over (mm): the photo paper's shape, a base model for the view.
pub const GROUND_MM: f64 = 50.0;

impl Viewer {
    /// A .vug document: doc_core's reader (a refusal named by line), then [`Viewer::open_doc`]; `s` writes it back.
    fn open_vug(path: &str, title: &str, size: (u32, u32), preset: Preset) -> Result<Viewer, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{path}: {e}"))?;
        let lib = doc_host::DirLib::beside(std::path::Path::new(path)); // VUGMAKEMACHINELIBFACE (SR548): `lib=<name>` beside the part
        let doc = doc_core::read_with(&text, &lib).map_err(|r| format!("{path}: line {}: {:?}", r.line, r.what))?;
        let mut v = Viewer::open_doc(doc, title, size, preset).map_err(|e| format!("{path}: {e}"))?;
        v.save_to = Some(std::path::PathBuf::from(path));
        v.claimed = true;
        Ok(v)
    }

    /// A document as the face: regenerated through the journal (a replay failure named by line), over a ground quad
    /// at z = 0; the open sketch is the face, the solids the body, the camera homed on what is shown. Clean.
    pub fn open_doc(doc: doc_core::Doc, title: &str, size: (u32, u32), preset: Preset) -> Result<Viewer, String> {
        let g = GROUND_MM;
        let ground = Model::from_triangles(&[[-g, -g, 0.0], [g, -g, 0.0], [g, g, 0.0], [-g, g, 0.0]], &[[0, 1, 2], [0, 2, 3]])?;
        let mut v = Viewer::new(ground, title, size, preset);
        let r = v.view.design.journal.load(doc)?;
        v.view.design.face = r.face;
        v.view.design.body = r.body;
        match v.view.design.shown() {
            Some(m) => v.view.home(&m),
            None => v.view.home(&v.model),
        }
        v.retitle();
        Ok(v)
    }

    /// Write the document (doc_core's writer) to `save_to` atomically: no target names "no file"; an existing file the
    /// face neither opened nor wrote is refused by name (Save As chooses another: VUGFACESAVEAS). The outcome is the corner line.
    pub fn save(&mut self) -> Result<std::path::PathBuf, String> {
        let out = self.save_inner();
        self.view.design.journal.disk = Some(match &out {
            Ok(p) => Ok(format!("saved {}", p.file_name().and_then(|s| s.to_str()).unwrap_or(""))),
            Err(e) => Err(e.clone()),
        });
        self.retitle();
        out
    }

    fn save_inner(&mut self) -> Result<std::path::PathBuf, String> {
        let path = self.save_to.clone().ok_or("no file: Cmd+Shift+S saves as")?;
        if !self.claimed && path.exists() {
            return Err(format!("{} exists: open it to change it", path.display()));
        }
        let text = doc_core::write(&self.view.design.journal.doc);
        write_atomic(&path, &text)?;
        self.view.design.journal.mark_saved(text);
        self.claimed = true;
        Ok(path)
    }

    /// The title: the bare name, with `*` while the document differs from the last saved text.
    fn retitle(&mut self) {
        self.title = if self.view.design.journal.dirty() { format!("{}*", self.name) } else { self.name.clone() };
    }
}

/// `text` to `<path>.tmp` (created, written, synced), then renamed over `path`; a failed rename removes the .tmp.
/// Either error names its file.
fn write_atomic(path: &std::path::Path, text: &str) -> Result<(), String> {
    use std::io::Write;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    let named = |p: &std::path::Path, e: std::io::Error| format!("{}: {e}", p.display());
    let mut f = std::fs::File::create(&tmp).map_err(|e| named(tmp.as_path(), e))?;
    f.write_all(text.as_bytes()).and_then(|()| f.sync_all()).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        named(tmp.as_path(), e)
    })?;
    drop(f);
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        named(path, e)
    })
}

// ---- VUGFACESAVEAS (SR539): the chosen path ----

impl Viewer {
    /// The Save As field's keys: `Meta+Shift+s` opens it; while it is open it takes every key — a character is typed
    /// (sketch.rs `type_text`), Backspace pops, Esc or the chord again closes it with nothing written, Enter saves to
    /// the typed path, any other key does nothing. None when it is closed and the key is not the chord.
    pub fn save_as_key(&mut self, k: &str) -> Option<bool> {
        let chord = matches!(k, "Meta+Shift+s" | "Meta+Shift+S");
        let j = &mut self.view.design.journal;
        let Some(f) = j.save_as.as_mut() else {
            if chord {
                j.save_as = Some(journal::SaveAs::default());
                return Some(true);
            }
            return None;
        };
        if chord || matches!(k, "Escape" | "\u{1b}") {
            j.save_as = None;
            return Some(true);
        }
        if matches!(k, "Return" | "KP_Enter" | "\r" | "\n") {
            let (typed, armed) = (f.text.clone(), f.armed);
            self.save_as_enter(&typed, armed);
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
        if !sketch::type_text(&mut f.text, ch, back) {
            return Some(false);
        }
        if f.armed {
            f.armed = false;
            j.disk = None; // the "exists" line named the old text
        }
        Some(true)
    }

    /// Enter in the field: the typed path (relative: beside the opened file; no extension: `.vug`). An existing file
    /// that is not the face's own target is refused once by name (the field armed: the next Enter overwrites); else
    /// the path becomes the target and the SAME [`Viewer::save`] writes it — on success the title follows the new name
    /// and the field closes, on an error the old target is restored and the field stays.
    fn save_as_enter(&mut self, typed: &str, armed: bool) {
        if typed.is_empty() {
            self.view.design.journal.disk = Some(Err("save as: no path typed".to_string()));
            return;
        }
        let mut p = std::path::PathBuf::from(typed);
        if p.is_relative() {
            if let Some(dir) = self.save_to.as_deref().and_then(|s| s.parent()) {
                p = dir.join(p);
            }
        }
        if p.extension().is_none() {
            p.set_extension("vug");
        }
        let own = self.claimed && self.save_to.as_deref() == Some(p.as_path());
        if p.exists() && !armed && !own {
            let j = &mut self.view.design.journal;
            j.disk = Some(Err(format!("{} exists: Enter again overwrites", p.display())));
            if let Some(f) = j.save_as.as_mut() {
                f.armed = true;
            }
            return;
        }
        let was = (self.save_to.replace(p.clone()), self.claimed);
        self.claimed = true;
        match self.save() {
            Ok(_) => {
                self.name = p.file_name().and_then(|s| s.to_str()).unwrap_or("vug").to_string();
                self.view.design.journal.save_as = None;
            }
            Err(_) => {
                self.save_to = was.0;
                self.claimed = was.1;
            }
        }
        self.retitle();
    }
}

// ---- VUGFACEDEVIATIONSOLIDFAULT (LEDGER SR565): `vug-view part.vug --scan scan.ply`, the part as the journal ----

impl Viewer {
    /// A .vug part opened as itself (the journal, [`Viewer::open`]'s .vug road) with a scan mesh measured against the
    /// journal's document (`Viewer::against_journal`, generation 0): edits of the part re-measure the scan. The title
    /// is the part's file name; a part that is not a .vug, a .prb scan, or a document the journal refuses are named.
    pub fn open_scan(part: &str, scan: &str, size: (u32, u32), preset: Preset) -> Result<Viewer, String> {
        let p = std::path::Path::new(part);
        if !p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("vug")) {
            return Err(format!("{part}: --scan opens a .vug part"));
        }
        let title = p.file_name().and_then(|s| s.to_str()).unwrap_or("vug").to_string();
        let (_, mesh) = read_scan(scan)?;
        let mut v = Viewer::open_vug(part, &title, size, preset)?;
        v.against_journal(deviation::scan_mesh(mesh)).map_err(|e| format!("{part}: {e}"))?;
        Ok(v)
    }
}

/// A scan mesh by path (.ply / .stl / .obj through model.rs's readers) and its file name, the title; a .prb is refused
/// by name (fusion is tools/scan's). The one reader [`Viewer::open_against`] and [`Viewer::open_scan`] share.
fn read_scan(scan: &str) -> Result<(String, scan_core::Mesh), String> {
    let p = std::path::Path::new(scan);
    let title = p.file_name().and_then(|s| s.to_str()).unwrap_or("vug").to_string();
    if p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("prb")) {
        return Err(format!("{scan}: ScanPrb (fuse it first: `scan {scan} -o scan.ply`)"));
    }
    Ok((title, model::read_mesh(scan)?))
}
