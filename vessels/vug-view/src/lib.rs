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

pub mod model;
pub mod raster;
pub mod view;

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
    ("p", "next navigation preset"),
    ("click", "pick a face"),
    ("middle / right drag", "orbit / pan / zoom per the preset"),
    ("Alt+drag", "the middle button, for a trackpad (orbit / pan / zoom per the preset)"),
    ("wheel / pinch", "zoom to the cursor"),
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
