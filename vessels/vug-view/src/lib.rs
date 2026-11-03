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
    ("i", "ask Vein for design intent on the photo's sketch (no provider today: the corner says so)"),
    ("Tab", "focus the next proposal"),
    ("Enter / Backspace", "on the focused proposal: apply it (verified against the photo) / reject it"),
    // VUGFACETREE (SR513): the feature tree over doc_core.
    ("t", "the feature tree panel (the document's statements)"),
    ("u / U", "undo / redo (the document's snapshots; the face regenerates)"),
    ("Up / Down, Delete, Enter", "in the tree panel: move, delete the row with its dependants, edit its value (type, Enter)"),
];

/// The controller: owns the model and the view (one task, no lock).
pub struct Viewer {
    pub model: Model,
    pub view: View,
    pub title: String,
}

impl Viewer {
    pub fn new(model: Model, title: &str, size: (u32, u32), preset: Preset) -> Viewer {
        let view = View::new(&model, size, preset);
        Viewer { model, view, title: title.to_string() }
    }

    /// VUGFACEPHOTO (SR506): THE open road — a .png / .jpg / .jpeg (else any file pixel_core recognises that is not a
    /// mesh by extension) is a photo on graph paper measured at `photo::PITCH_MM`; anything else is a mesh
    /// (`Model::open`). The title is the file name.
    pub fn open(path: &str, size: (u32, u32), preset: Preset) -> Result<Viewer, String> {
        let p = std::path::Path::new(path);
        let title = p.file_name().and_then(|s| s.to_str()).unwrap_or("vug").to_string();
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
        let mesh = matches!(ext.as_str(), "stl" | "ply" | "obj");
        if !mesh {
            let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
            if matches!(ext.as_str(), "png" | "jpg" | "jpeg") || pixel_core::sniff(&bytes).is_some() {
                return Viewer::open_photo(&bytes, &title, size, preset, photo::PITCH_MM).map_err(|e| format!("{path}: {e}"));
            }
        }
        Ok(Viewer::new(Model::open(path)?, &title, size, preset))
    }

    /// VUGFACEPHOTO (SR506): a photo's bytes on `pitch_mm` graph paper: photo_core measures it, the paper is the drawn
    /// model, the measured sketch is open (Select) on the XY plane, seen from the top.
    pub fn open_photo(bytes: &[u8], title: &str, size: (u32, u32), preset: Preset, pitch_mm: f64) -> Result<Viewer, String> {
        let (model, face, photo) = photo::open(bytes, pitch_mm)?;
        let mut v = Viewer::new(model, title, size, preset);
        v.view.top(&v.model);
        v.view.design.face = Some(face);
        v.view.photo = Some(photo);
        Ok(v)
    }

    /// A shortcut key; true when the frame changed.
    pub fn key(&mut self, k: &str) -> bool {
        self.view.key(&self.model, k)
    }

    /// One window message; true when the frame must be redrawn (or resent).
    pub fn input(&mut self, msg: &SMessage) -> bool {
        match msg {
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
            _ => false,
        }
    }

    /// Render the current view as the surface's whole frame.
    pub fn frame(&self) -> SMessage {
        let f = raster::render(&self.model, &self.view);
        SMessage::SurfaceBlit { url: SURFACE.to_string(), width: f.width, height: f.height, pixels: f.rgba, damage: None, scroll: None }
    }
}
