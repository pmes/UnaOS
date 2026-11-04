// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The navigation (VUGFACE1, LEDGER SR470): the camera, the four presets as tables of (button, modifiers) -> action,
//! orbit about the cursor's hit point (else the model's centre), pan that keeps the grabbed point under the cursor,
//! zoom to the cursor, Home and the three standard views. Inertia off. Z is up (the printers', Onshape's, Blender's).
//! The camera is euclase's f32 math; the picks are the model seam's (mesh_core's BVH).

use crate::model::Model;
use bandy::{BUTTON_MIDDLE, BUTTON_RIGHT, MOD_ALT, MOD_CONTROL, MOD_SHIFT};
use euclase::{Mat4, Vec3};

/// A mouse button. The surface wire carries the left button as `BrowserButton` and the right and middle as
/// `BrowserAuxButton` (VUG-FACE-BUTTONS, LEDGER SR477: quartzite image_view's `rightMouse*:` / `otherMouse*:`); Alt+Left
/// stays the middle button for a trackpad or a one-button mouse (`Preset::resolve`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

impl Button {
    /// The button a `BrowserButton` (0) or `BrowserAuxButton` number names; `None` for the extra buttons (3 and up).
    pub fn from_wire(button: u8) -> Option<Button> {
        match button {
            0 => Some(Button::Left),
            BUTTON_RIGHT => Some(Button::Right),
            BUTTON_MIDDLE => Some(Button::Middle),
            _ => None,
        }
    }
}

/// What a drag (or a click) does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Orbit,
    Pan,
    Zoom,
    /// The context menu (owed: nothing opens yet).
    Menu,
    /// A click selects what is under the cursor.
    Pick,
    None,
}

/// The navigation presets. `Una` is the default (the Blender table): menu and orbit never share a button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    Una,
    SolidWorks,
    Fusion,
    Blender,
    Onshape,
}

pub const PRESETS: [Preset; 5] = [Preset::Una, Preset::SolidWorks, Preset::Fusion, Preset::Blender, Preset::Onshape];

/// A press that moves less than this (in surface pixels) before its release is a click.
pub const CLICK_PX: f64 = 4.0;

impl Preset {
    /// The table: (button, modifiers among Shift / Control) -> action; the first matching row wins. The wheel zooms to
    /// the cursor in every preset.
    pub fn table(self) -> &'static [(Button, u8, Action)] {
        use Action::*;
        use Button::*;
        match self {
            Preset::Una | Preset::Blender => &[(Middle, 0, Orbit), (Middle, MOD_SHIFT, Pan), (Middle, MOD_CONTROL, Zoom), (Right, 0, Menu)],
            Preset::SolidWorks => &[(Middle, 0, Orbit), (Middle, MOD_CONTROL, Pan), (Middle, MOD_SHIFT, Zoom), (Right, 0, Menu)],
            Preset::Fusion => &[(Middle, MOD_SHIFT, Orbit), (Middle, 0, Pan), (Right, 0, Menu)],
            // Onshape shares the right button between orbit (a drag) and the menu (a click): `click_menu` names it.
            Preset::Onshape => &[(Right, 0, Orbit), (Middle, 0, Pan), (Right, MOD_CONTROL, Pan), (Right, 0, Menu)],
        }
    }

    /// The button whose CLICK (a press that never moved `CLICK_PX`) is the menu while its drag orbits — Onshape's
    /// convention, the one preset where menu and orbit share a button; `None` everywhere else.
    pub fn click_menu(self) -> Option<Button> {
        match self {
            Preset::Onshape => Some(Button::Right),
            _ => None,
        }
    }

    /// The action of a press: Alt+Left is read as Middle first (the one-button / trackpad rows, Blender's emulated
    /// three-button mouse); a bare Left is a pick.
    pub fn resolve(self, button: Button, mods: u8) -> Action {
        let (button, mods) = if button == Button::Left && mods & MOD_ALT != 0 { (Button::Middle, mods & !MOD_ALT) } else { (button, mods) };
        let m = mods & (MOD_SHIFT | MOD_CONTROL);
        if let Some(&(_, _, a)) = self.table().iter().find(|(b, bm, _)| *b == button && *bm == m) {
            return a;
        }
        if button == Button::Left && m == 0 { Action::Pick } else { Action::None }
    }

    pub fn name(self) -> &'static str {
        match self {
            Preset::Una => "una",
            Preset::SolidWorks => "solidworks",
            Preset::Fusion => "fusion",
            Preset::Blender => "blender",
            Preset::Onshape => "onshape",
        }
    }

    pub fn parse(s: &str) -> Option<Preset> {
        PRESETS.iter().copied().find(|p| p.name().eq_ignore_ascii_case(s))
    }

    pub fn next(self) -> Preset {
        let i = PRESETS.iter().position(|p| *p == self).unwrap_or(0);
        PRESETS[(i + 1) % PRESETS.len()]
    }
}

const UP: Vec3 = Vec3::new(0.0, 0.0, 1.0);
const PITCH_MAX: f32 = 89.5 * core::f32::consts::PI / 180.0;
/// Radians of orbit per pixel of drag.
const ORBIT_PER_PX: f32 = 0.008;

/// The camera: an eye, a turntable orientation (yaw about +Z, pitch above the horizon) and a vertical field of view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Camera {
    pub eye: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub fov_y: f32,
}

impl Camera {
    /// The orthonormal basis (right, up, forward).
    pub fn basis(&self) -> (Vec3, Vec3, Vec3) {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        let f = Vec3::new(cp * cy, cp * sy, sp);
        let r = f.cross(UP).normalize();
        let u = r.cross(f);
        (r, u, f)
    }

    /// The view matrix (euclase's right-handed look-at: camera space looks down -Z).
    pub fn view(&self) -> Mat4 {
        let (_, _, f) = self.basis();
        Mat4::look_at_rh(self.eye, self.eye + f, UP)
    }
}

/// A drag in progress.
#[derive(Clone, Copy, Debug)]
struct Drag {
    action: Action,
    start: (f64, f64),
    last: (f64, f64),
    /// The point the drag orbits about / pans at / zooms toward.
    pivot: Vec3,
    /// The press has moved `CLICK_PX` (a click-sensitive press does nothing until it has).
    moved: bool,
}

/// The view state: camera, preset, surface size, the drag and the picked triangle.
#[derive(Clone, Debug)]
pub struct View {
    pub camera: Camera,
    pub preset: Preset,
    pub size: (u32, u32),
    pub picked: Option<u32>,
    pub cursor: (f64, f64),
    drag: Option<Drag>,
    /// VUGFACE2 (SR485): the sketch face, the body and the pull (sketch.rs); the drawn model is `design.shown()`.
    pub design: crate::sketch::Design,
    /// VUGFACEFILLET (SR511): the edge tool (`f` fillet, Shift+f chamfer) while it is open (fillet.rs).
    pub fillet: Option<crate::fillet::EdgeTool>,
    /// VUGFACEPHOTO (SR506): the opened photo — its underlay, the measured outline and the offered proposals (photo.rs).
    pub photo: Option<crate::photo::Photo>,
    /// VUGFACEDEVIATION (SR525): the opened scan measured against its design — the band's colours and the corner (deviation.rs).
    pub deviation: Option<crate::deviation::Map>,
}

impl View {
    /// A view at Home over `model`.
    pub fn new(model: &Model, size: (u32, u32), preset: Preset) -> View {
        let camera = Camera { eye: Vec3::zero(), yaw: 0.0, pitch: 0.0, fov_y: 30f32.to_radians() };
        let mut v = View { camera, preset, size: (size.0.max(1), size.1.max(1)), picked: None, cursor: (0.0, 0.0), drag: None, design: Default::default(), photo: None, fillet: None, deviation: None }; // VUGFACEFILLET (SR511): fillet; VUGFACEDEVIATION (SR525): deviation
        v.home(model);
        v
    }

    /// Focal length in pixels.
    pub fn focal_px(&self) -> f32 {
        (self.size.1 as f32 * 0.5) / (self.camera.fov_y * 0.5).tan()
    }

    /// The ray through surface point (px, py) (pixel centres at +0.5): `(origin, dir)` with `dir . forward = 1`, so a
    /// ray parameter IS the view depth.
    pub fn ray(&self, px: f64, py: f64) -> (Vec3, Vec3) {
        let (r, u, f) = self.camera.basis();
        let fp = self.focal_px();
        let a = (px as f32 - self.size.0 as f32 * 0.5) / fp;
        let b = -(py as f32 - self.size.1 as f32 * 0.5) / fp;
        (self.camera.eye, f + r * a + u * b)
    }

    /// Where a world point lands on the surface (`None` behind the eye).
    pub fn project(&self, p: Vec3) -> Option<(f32, f32)> {
        let (r, u, f) = self.camera.basis();
        let d = p - self.camera.eye;
        let z = d.dot(f);
        if z <= 0.0 {
            return None;
        }
        let fp = self.focal_px();
        Some((self.size.0 as f32 * 0.5 + d.dot(r) / z * fp, self.size.1 as f32 * 0.5 - d.dot(u) / z * fp))
    }

    /// VUGFACE2 (SR485): the camera basis and focal length evaluated in f64 from the f32 camera's yaw, pitch and field
    /// of view (the same formula as `Camera::basis` / `focal_px`, orthonormal to f64), for the sketch plane's hit.
    fn basis_f64(&self) -> ([f64; 3], [f64; 3], [f64; 3], f64) {
        let (sy, cy) = (self.camera.yaw as f64).sin_cos();
        let (sp, cp) = (self.camera.pitch as f64).sin_cos();
        let f = [cp * cy, cp * sy, sp];
        let n = f[0].hypot(f[1]).max(1e-300);
        let r = [f[1] / n, -f[0] / n, 0.0];
        let u = [r[1] * f[2] - r[2] * f[1], r[2] * f[0] - r[0] * f[2], r[0] * f[1] - r[1] * f[0]];
        let fp = (self.size.1 as f64 * 0.5) / (self.camera.fov_y as f64 * 0.5).tan();
        (r, u, f, fp)
    }

    /// VUGFACE2 (SR485): `ray` in f64 — origin the eye, `dir . forward = 1` — so a plane hit and its `project_f64`
    /// round-trip to 1e-9 mm (an f32 ray quantises a 100 mm hit near 1e-3 mm).
    pub fn ray_f64(&self, px: f64, py: f64) -> ([f64; 3], [f64; 3]) {
        let (r, u, f, fp) = self.basis_f64();
        let e = self.camera.eye;
        let a = (px - self.size.0 as f64 * 0.5) / fp;
        let b = -(py - self.size.1 as f64 * 0.5) / fp;
        ([e.x as f64, e.y as f64, e.z as f64], [0, 1, 2].map(|k| f[k] + r[k] * a + u[k] * b))
    }

    /// VUGFACE2 (SR485): `project` in f64, the exact inverse of `ray_f64` (`None` behind the eye).
    pub fn project_f64(&self, p: [f64; 3]) -> Option<(f64, f64)> {
        let (r, u, f, fp) = self.basis_f64();
        let e = self.camera.eye;
        let d = [p[0] - e.x as f64, p[1] - e.y as f64, p[2] - e.z as f64];
        let dot = |a: [f64; 3]| a[0] * d[0] + a[1] * d[1] + a[2] * d[2];
        let z = dot(f);
        if z <= 0.0 {
            return None;
        }
        Some((self.size.0 as f64 * 0.5 + dot(r) / z * fp, self.size.1 as f64 * 0.5 - dot(u) / z * fp))
    }

    /// VUGFACE2 (SR485): the sketch overlay, drawn by `raster::render` over the frame.
    pub fn overlay(&self, fr: &mut crate::raster::Frame) {
        crate::sketch::overlay(self, fr); crate::fillet::overlay(self, fr); // VUGFACEFILLET (SR511): the edge tool over the sketch
        // VUGFACEPHOTO (SR506): the proposal glyphs and the corner note over the sketch.
        crate::photo::overlay(self, fr); crate::deviation::overlay(self, fr); // VUGFACEDEVIATION (SR525): the map's corner, top-right
        // VUGFACETREE (SR513): the tree panel over everything.
        crate::journal::overlay(self, fr);
    }

    /// The model point under (px, py), if the ray hits it.
    pub fn hit(&self, model: &Model, px: f64, py: f64) -> Option<(Vec3, u32)> {
        let (o, d) = self.ray(px, py);
        model.pick([o.x, o.y, o.z], [d.x, d.y, d.z]).map(|(t, tri)| (o + d * t, tri))
    }

    fn centre(model: &Model) -> Vec3 {
        let c = model.centre();
        Vec3::new(c[0], c[1], c[2])
    }

    /// Look from direction (yaw, pitch) at the model's centre, its bounding sphere fitted to the shorter side.
    fn look(&mut self, model: &Model, yaw: f32, pitch: f32) {
        let half_y = self.camera.fov_y * 0.5;
        let half_x = (half_y.tan() * self.size.0 as f32 / self.size.1 as f32).atan();
        let dist = model.radius() / half_y.min(half_x).sin() * 1.1;
        self.camera.yaw = yaw;
        self.camera.pitch = pitch.clamp(-PITCH_MAX, PITCH_MAX);
        let (_, _, f) = self.camera.basis();
        self.camera.eye = Self::centre(model) - f * dist;
        self.drag = None;
    }

    /// Home: the isometric view from (+1, -1, +1).
    pub fn home(&mut self, model: &Model) {
        self.look(model, 135f32.to_radians(), -(1.0f32 / 3f32.sqrt()).asin());
    }

    /// Front: from -Y looking along +Y.
    pub fn front(&mut self, model: &Model) {
        self.look(model, 90f32.to_radians(), 0.0);
    }

    /// Right: from +X looking along -X.
    pub fn right(&mut self, model: &Model) {
        self.look(model, 180f32.to_radians(), 0.0);
    }

    /// Top: from +Z looking down (at the pitch clamp; Y stays up the screen).
    pub fn top(&mut self, model: &Model) {
        self.look(model, 90f32.to_radians(), -PITCH_MAX);
    }

    /// A shortcut key; true when the frame changed. `h`/`0` Home, `1` front, `3` right, `7` top, `p` next preset.
    pub fn key(&mut self, model: &Model, k: &str) -> bool {
        // VUGFACE2 (SR485): the body replaces the drawn model; the sketch face takes its keys first (l c a p e Esc).
        let shown = self.design.shown();
        let model = shown.as_deref().unwrap_or(model); if let Some(r) = crate::fillet::key(self, model, k) { return r; } // VUGFACEFILLET (SR511): the edge tool's keys first
        // VUGFACEPHOTO (SR506): the proposal keys (i, Tab, Enter / Backspace on the focused one) before the sketch's.
        if let Some(r) = crate::photo::key(self, k) {
            return r;
        }
        // VUGFACETREE (SR513): the tree's keys (u / U undo / redo, t the panel, its rows) before the sketch's.
        if let Some(r) = crate::journal::key(&mut self.design, k) {
            return r;
        }
        if let Some(r) = crate::sketch::key(self, model, k) {
            return r;
        }
        if let Some(r) = crate::deviation::key(self, k) { return r; } match k { // VUGFACEDEVIATION (SR525): `d` outside a sketch toggles the map
            "h" | "H" | "0" | "Home" => self.home(model),
            "1" => self.front(model),
            "3" => self.right(model),
            "7" => self.top(model),
            "p" | "P" => self.preset = self.preset.next(),
            _ => return false,
        }
        true
    }

    pub fn resize(&mut self, w: u32, h: u32) -> bool {
        let s = (w.max(1), h.max(1));
        let changed = s != self.size;
        self.size = s;
        changed
    }

    /// A press: resolve the action; a navigation press picks its pivot at the cursor (else the model's centre) — the
    /// ONE pick of a drag (bounded: one BVH query per press, none per move).
    pub fn press(&mut self, model: &Model, button: Button, mods: u8, x: f64, y: f64) -> bool {
        // VUGFACE2 (SR485): the body replaces the drawn model; a bare left press is the sketch face's first.
        let shown = self.design.shown();
        let model = shown.as_deref().unwrap_or(model);
        self.cursor = (x, y); if let Some(r) = crate::fillet::press(self, model, button, mods, x, y) { return r; } // VUGFACEFILLET (SR511): a click re-picks the edge
        if let Some(r) = crate::sketch::press(self, button, mods, x, y) {
            return r;
        }
        let action = self.preset.resolve(button, mods);
        let pivot = match action {
            Action::Orbit | Action::Pan | Action::Zoom => self.hit(model, x, y).map(|(p, _)| p).unwrap_or_else(|| Self::centre(model)),
            _ => Self::centre(model),
        };
        let click_sensitive = matches!(action, Action::Pick) || self.preset.click_menu() == Some(button);
        self.drag = Some(Drag { action, start: (x, y), last: (x, y), pivot, moved: !click_sensitive });
        false
    }

    /// Pointer motion; true when the frame changed.
    pub fn motion(&mut self, x: f64, y: f64) -> bool {
        self.cursor = (x, y);
        // VUGFACEPHOTO (SR506): the proposal glyph under the cursor takes the focus (never consumes the motion).
        let focus = crate::photo::hover(self, x, y);
        // VUGFACE2 (SR485): with no navigation drag, the sketch face's gesture / hover / pull first.
        if self.drag.is_none() {
            if let Some(r) = crate::sketch::motion(self, x, y) {
                return r || focus;
            }
        }
        let Some(mut d) = self.drag else { return focus };
        if !d.moved {
            if (x - d.start.0).hypot(y - d.start.1) < CLICK_PX {
                return false;
            }
            d.moved = true;
        }
        let (dx, dy) = ((x - d.last.0) as f32, (y - d.last.1) as f32);
        d.last = (x, y);
        self.drag = Some(d);
        match d.action {
            Action::Orbit => self.orbit(d.pivot, dx, dy),
            Action::Pan => self.pan(d.pivot, dx, dy),
            Action::Zoom => self.dolly(d.pivot, (dy * 0.01).exp().clamp(0.5, 2.0)),
            _ => return false,
        }
        true
    }

    /// A release: a click picks (Pick) or would open the menu (Menu, owed); true when the frame changed.
    pub fn release(&mut self, model: &Model, x: f64, y: f64) -> bool {
        self.cursor = (x, y);
        // VUGFACE2 (SR485): with no navigation drag, the sketch face's release first.
        if self.drag.is_none() {
            if let Some(r) = crate::sketch::release(self, x, y) {
                return r;
            }
        }
        let Some(d) = self.drag.take() else { return false };
        let click = (x - d.start.0).hypot(y - d.start.1) < CLICK_PX;
        if click && d.action == Action::Pick {
            let was = self.picked;
            let h = self.hit(model, x, y); self.picked = h.map(|(_, t)| t); let dv = crate::deviation::pick(self, model, h); // VUGFACEDEVIATIONPICK (SR543): the click's one hit also picks the map's vertex
            return self.picked != was || dv;
        }
        false
    }

    /// The wheel at the cursor: zoom toward the model point under it (else toward the cursor ray at the centre's
    /// depth). `dy` is the surface's `BrowserScroll` dy (negative zooms in).
    pub fn wheel(&mut self, model: &Model, dy: f64) -> bool {
        if dy == 0.0 {
            return false;
        }
        let target = self.zoom_target(model);
        self.dolly(target, ((dy as f32) * 0.01).exp().clamp(0.5, 2.0));
        true
    }

    /// A pinch at (x, y) (VUG-FACE-BUTTONS, LEDGER SR477): `scale` above 1 spreads the fingers and zooms in toward the
    /// same point the wheel would (`zoom_target`), by 1 / `scale` per event, clamped as the wheel's step.
    pub fn pinch(&mut self, model: &Model, scale: f64, x: f64, y: f64) -> bool {
        self.cursor = (x, y);
        if !(scale.is_finite() && scale > 0.0) || scale == 1.0 {
            return false;
        }
        let target = self.zoom_target(model);
        self.dolly(target, (1.0 / scale as f32).clamp(0.5, 2.0));
        true
    }

    /// The point a zoom at the cursor moves toward: the model point under it, else the cursor ray at the centre's depth.
    fn zoom_target(&self, model: &Model) -> Vec3 {
        // VUGFACE2 (SR485): zoom toward the body when it replaces the drawn model.
        let shown = self.design.shown();
        let model = shown.as_deref().unwrap_or(model);
        let (x, y) = self.cursor;
        match self.hit(model, x, y) {
            Some((p, _)) => p,
            None => {
                let (o, d) = self.ray(x, y);
                let depth = (Self::centre(model) - o).dot(self.camera.basis().2).max(model.radius() * 0.01);
                o + d * depth
            }
        }
    }

    /// Orbit about `pivot`: the camera frame turns rigidly about it, so the pivot's place on the surface is fixed.
    pub fn orbit(&mut self, pivot: Vec3, dx: f32, dy: f32) {
        let (r, u, f) = self.camera.basis();
        let rel = self.camera.eye - pivot;
        let (c, d, e) = (rel.dot(r), rel.dot(u), rel.dot(f));
        self.camera.yaw -= dx * ORBIT_PER_PX;
        self.camera.pitch = (self.camera.pitch + dy * ORBIT_PER_PX).clamp(-PITCH_MAX, PITCH_MAX);
        let (r2, u2, f2) = self.camera.basis();
        self.camera.eye = pivot + r2 * c + u2 * d + f2 * e;
    }

    /// Pan: the point at the pivot's depth under the cursor follows the cursor 1:1.
    pub fn pan(&mut self, pivot: Vec3, dx: f32, dy: f32) {
        let (r, u, f) = self.camera.basis();
        let depth = (pivot - self.camera.eye).dot(f).max(1e-6);
        let k = depth / self.focal_px();
        self.camera.eye = self.camera.eye - r * (dx * k) + u * (dy * k);
    }

    /// Dolly toward `target` by `scale` (< 1 closer): the eye slides on the line to the target with the orientation
    /// fixed, so the target (and every point on its sight line) keeps its place on the surface.
    pub fn dolly(&mut self, target: Vec3, scale: f32) {
        self.camera.eye = target + (self.camera.eye - target) * scale;
    }
}
