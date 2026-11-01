// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The navigation (VUGFACE1, LEDGER SR470): the camera, the four presets as tables of (button, modifiers) -> action,
//! orbit about the cursor's hit point (else the model's centre), pan that keeps the grabbed point under the cursor,
//! zoom to the cursor, Home and the three standard views. Inertia off. Z is up (the printers', Onshape's, Blender's).
//! The camera is euclase's f32 math; the picks are the model seam's (mesh_core's BVH).

use crate::model::Model;
use bandy::{MOD_ALT, MOD_CONTROL, MOD_SHIFT};
use euclase::{Mat4, Vec3};

/// A mouse button. Today's surface wire (quartzite image_view `mouseDown:`) carries the LEFT button only; Middle and
/// Right reach the view through Alt+Left (Middle) until VUG-FACE-BUTTONS forwards them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
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
}

impl View {
    /// A view at Home over `model`.
    pub fn new(model: &Model, size: (u32, u32), preset: Preset) -> View {
        let camera = Camera { eye: Vec3::zero(), yaw: 0.0, pitch: 0.0, fov_y: 30f32.to_radians() };
        let mut v = View { camera, preset, size: (size.0.max(1), size.1.max(1)), picked: None, cursor: (0.0, 0.0), drag: None };
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
        match k {
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
        self.cursor = (x, y);
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
        let Some(mut d) = self.drag else { return false };
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
        let Some(d) = self.drag.take() else { return false };
        let click = (x - d.start.0).hypot(y - d.start.1) < CLICK_PX;
        if click && d.action == Action::Pick {
            let was = self.picked;
            self.picked = self.hit(model, x, y).map(|(_, t)| t);
            return self.picked != was;
        }
        false
    }

    /// The wheel at the cursor: zoom toward the model point under it (else toward the cursor ray at the centre's
    /// depth). `dy` is the surface's `BrowserScroll` dy (negative zooms in).
    pub fn wheel(&mut self, model: &Model, dy: f64) -> bool {
        if dy == 0.0 {
            return false;
        }
        let (x, y) = self.cursor;
        let target = match self.hit(model, x, y) {
            Some((p, _)) => p,
            None => {
                let (o, d) = self.ray(x, y);
                let depth = (Self::centre(model) - o).dot(self.camera.basis().2).max(model.radius() * 0.01);
                o + d * depth
            }
        };
        self.dolly(target, ((dy as f32) * 0.01).exp().clamp(0.5, 2.0));
        true
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
