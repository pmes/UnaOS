// CHARTER: Facet — shared-core face (Images; FACETEDIT, rmbp-ledger B900)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! FACETEDIT — the model the editor window drives: the view transform ([`View`], integer, the host face's
//! `canvas::View`), the tool rail ([`Tool`]), the brush fields, and [`Editor`]: ONE raster_core `Document`, ONE
//! `History`, the active layer — and the gesture -> op table, mirrored one-to-one from
//! `vessels/facet-view/src/editor/mod.rs` (RASTERFACE SR234). Every gesture is ONE op (a `Command` through
//! `History::commit`, or a core call over a copy of a layer + `PastePixels`, tools/raster `ops.rs`'s shapes) and ONE
//! `:: FACET: gesture=` token. The cores do every edit and are the one gate; nothing here edits a pixel.
//! Design: `docs/dev/evidence/raster/FACETEDIT.md`.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use raster_core::{BlendMode, Command, Document, History, Layer, Mask, Selection};

/// The smallest and largest zoom exponents: scale `2^z` from 1/8 to 32 (the host's `ZOOM_MIN` / `ZOOM_MAX`).
pub const ZOOM_MIN: i32 = -3;
pub const ZOOM_MAX: i32 = 5;
/// The layer cap for this cut (memory: FACETEDIT.md §Memory).
pub const LAYER_CAP: usize = 8;
/// The history bound: at most this many steps and this many snapshot pixel bytes (the oldest go first).
pub const UNDO_MAX: usize = 16;
pub const UNDO_BUDGET: usize = 64 * 1024 * 1024;

/// `screen = origin + canvas * 2^zoom`; the origin is kept in Q8 (1/256 screen px), so no float.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct View {
    pub zoom: i32,
    pub ox: i64,
    pub oy: i64,
}

/// `v * 2^z` for a Q8 quantity: a left shift for z >= 0, an arithmetic (flooring) right shift below.
fn scale_q8(v: i64, z: i32) -> i64 {
    if z >= 0 { v << z } else { v >> -z }
}

impl View {
    /// The largest power of two (at most 1) that shows a `dw x dh` document in a `vw x vh` region, centred.
    pub fn fit(dw: u32, dh: u32, vw: u32, vh: u32) -> View {
        let mut zoom = 0;
        while zoom > ZOOM_MIN && (scale_q8(dw as i64 * 256, zoom) > vw as i64 * 256 || scale_q8(dh as i64 * 256, zoom) > vh as i64 * 256) {
            zoom -= 1;
        }
        let (sw, sh) = (scale_q8(dw as i64 * 256, zoom), scale_q8(dh as i64 * 256, zoom));
        // floor((v - d*s) / 2) whole px, as the host's `.floor()`.
        let half = |v: u32, s: i64| ((v as i64 * 256 - s).div_euclid(512)) * 256;
        View { zoom, ox: half(vw, sw), oy: half(vh, sh) }
    }

    /// The scale as the witness prints it: `2`, `1`, `1/4`.
    pub fn scale_word(&self) -> String {
        if self.zoom >= 0 { format!("{}", 1u32 << self.zoom) } else { format!("1/{}", 1u32 << -self.zoom) }
    }

    /// The origin as whole screen px (the token's `origin=`).
    pub fn origin_px(&self) -> (i64, i64) {
        (self.ox >> 8, self.oy >> 8)
    }

    /// The canvas point (Q8) under a screen point given in Q8.
    pub fn to_canvas_q8(&self, sx_q8: i64, sy_q8: i64) -> (i64, i64) {
        (scale_q8(sx_q8 - self.ox, -self.zoom), scale_q8(sy_q8 - self.oy, -self.zoom))
    }

    /// The canvas cell under the screen pixel `(sx, sy)` (sampled at its centre).
    pub fn cell(&self, sx: i32, sy: i32) -> (i64, i64) {
        let (x, y) = self.to_canvas_q8(sx as i64 * 256 + 128, sy as i64 * 256 + 128);
        (x >> 8, y >> 8)
    }

    /// Zoom by `steps` powers of two about the screen point `(cx, cy)`: the canvas point under it stays put. False
    /// (and no change) when it would pass a bound.
    pub fn zoom_at(&mut self, steps: i32, cx: i32, cy: i32) -> bool {
        let z = self.zoom + steps;
        if !(ZOOM_MIN..=ZOOM_MAX).contains(&z) {
            return false;
        }
        let (c_x, c_y) = (cx as i64 * 256, cy as i64 * 256);
        let (px, py) = self.to_canvas_q8(c_x, c_y);
        self.zoom = z;
        self.ox = c_x - scale_q8(px, z);
        self.oy = c_y - scale_q8(py, z);
        true
    }

    pub fn pan(&mut self, dx: i32, dy: i32) {
        self.ox += dx as i64 * 256;
        self.oy += dy as i64 * 256;
    }
}

/// One tool of the rail (the host's `tools::Tool`, rail order).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    SelectRect,
    SelectEllipse,
    SelectLasso,
    Wand,
    Brush,
    Eraser,
    Fill,
    Move,
}

impl Tool {
    pub const ALL: [Tool; 8] =
        [Tool::SelectRect, Tool::SelectEllipse, Tool::SelectLasso, Tool::Wand, Tool::Brush, Tool::Eraser, Tool::Fill, Tool::Move];
    /// The rail labels, `ALL` order.
    pub const LABELS: [&'static [u8]; 8] = [b"Rect", b"Ellipse", b"Lasso", b"Wand", b"Brush", b"Eraser", b"Fill", b"Move"];

    pub fn word(self) -> &'static str {
        match self {
            Tool::SelectRect => "select-rect",
            Tool::SelectEllipse => "select-ellipse",
            Tool::SelectLasso => "select-lasso",
            Tool::Wand => "wand",
            Tool::Brush => "brush",
            Tool::Eraser => "eraser",
            Tool::Fill => "fill",
            Tool::Move => "move",
        }
    }

    pub fn index(self) -> usize {
        Tool::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

/// The eight adjust kinds, menu order: `(word, label)` (tools/raster's words).
pub const ADJUSTS: [(&str, &[u8]); 8] = [
    ("levels", b"Levels"),
    ("curves", b"Curves"),
    ("huesat", b"Hue/Sat"),
    ("invert", b"Invert"),
    ("threshold", b"Threshold"),
    ("boxblur", b"Box blur"),
    ("gaussian", b"Gaussian"),
    ("unsharp", b"Unsharp"),
];

/// The brush / colour fields a slider sets: `(key, label, max)`; the minimum is 1 for `diameter`, else 0.
pub const FIELDS: [(&str, &[u8], u32); 7] = [
    ("diameter", b"Size", 64),
    ("hardness", b"Hard", 255),
    ("flow", b"Flow", 255),
    ("red", b"R", 255),
    ("green", b"G", 255),
    ("blue", b"B", 255),
    ("alpha", b"A", 255),
];

/// A failed op's word: a core's `Error::word()` or the face's own.
pub type Word = &'static str;

/// The canvas drag in flight, in region coordinates.
#[derive(Debug, Clone)]
pub struct Drag {
    pub pan: bool,
    pub start: (i32, i32),
    pub last: (i32, i32),
    pub points: Vec<(i32, i32)>,
}

/// The editor's state: the model the window drives.
pub struct Editor {
    pub doc: Document,
    pub hist: History,
    pub active: usize,
    pub view: View,
    pub tool: Tool,
    /// `diameter` (px), `hardness`, `flow` — tools/raster's `brush` arguments less the layer and points.
    pub diameter: u32,
    pub hardness: u8,
    pub flow: u8,
    pub colour: [u8; 4],
    pub tolerance: u8,
    /// The canvas region's size (device px).
    pub region: (u32, u32),
    /// The container path Save writes and Open reads.
    pub save_path: String,
    pub drag: Option<Drag>,
    pub pan_armed: bool,
    /// The composite (and the rasterized selection) as last built; `None` after a document change.
    pub comp: Option<(Vec<u8>, Option<Mask>)>,
    /// `:: FACET:` tokens printed by this editor.
    pub tokens: u32,
    /// The last token's outcome and refusal word (`""` when it was ok).
    pub last_ok: bool,
    pub last_why: Word,
}

/// The one token (the host face's `witness::token`).
pub fn token(gesture: &str, fields: &str, result: Result<(), Word>, layers: usize, active: usize, undo: usize, redo: usize) -> String {
    let mut t = format!(":: FACET: gesture={}", gesture);
    if !fields.is_empty() {
        t.push(' ');
        t.push_str(fields);
    }
    if let Err(why) = result {
        t.push_str(&format!(" why={}", why));
    }
    t.push_str(&format!(" layers={} active={} undo={} redo={}", layers, active, undo, redo));
    t.push_str(if result.is_ok() { " -> ok ::" } else { " -> refused ::" });
    t
}

/// FNV-1a 64 (tools/raster's and the host witness's fnv).
pub fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &b| (h ^ b as u64).wrapping_mul(0x0000_0100_0000_01b3))
}

/// A blend mode's word (tools/raster's words, `BlendMode::ALL` order).
pub fn blend_word(b: BlendMode) -> &'static str {
    const WORDS: [&str; 8] = ["normal", "multiply", "screen", "overlay", "darken", "lighten", "difference", "erase"];
    WORDS.get(b.to_byte() as usize).copied().unwrap_or("normal")
}

/// A field value as the token carries it: spaces become `_`.
pub fn word(s: &str) -> String {
    s.chars().map(|c| if c.is_whitespace() { '_' } else { c }).collect()
}

fn empty_doc() -> Document {
    Document { width: 0, height: 0, layers: Vec::new(), selection: None }
}

impl Editor {
    /// An editor over `doc` in a `region`-sized canvas (the active layer is the top one, the view fits).
    pub fn with(doc: Document, region: (u32, u32), save_path: String) -> Editor {
        let region = (region.0.max(1), region.1.max(1));
        let view = View::fit(doc.width, doc.height, region.0, region.1);
        let active = doc.layers.len().saturating_sub(1);
        Editor {
            doc,
            hist: History::new(),
            active,
            view,
            tool: Tool::Brush,
            diameter: 9,
            hardness: 255,
            flow: 255,
            colour: [0, 0, 0, 255],
            tolerance: 32,
            region,
            save_path,
            drag: None,
            pan_armed: false,
            comp: None,
            tokens: 0,
            last_ok: true,
            last_why: "",
        }
    }

    /// A `w x h` document whose one "Background" layer holds `rgba` (tools/raster's `new` with the pixels filled).
    pub fn doc_of(w: u32, h: u32, rgba: Vec<u8>) -> Result<Document, Word> {
        let mut layer = Layer::new("Background", w, h);
        layer.pixels = rgba;
        Document::new(w, h).and_then(|d| d.apply(&Command::AddLayer { index: 0, layer })).map_err(|e| e.word())
    }

    /// Record (print) one gesture's token.
    pub fn emit(&mut self, gesture: &str, fields: &str, r: Result<(), Word>) {
        let t = token(gesture, fields, r, self.doc.layers.len(), self.active, self.hist.past.len(), self.hist.future.len());
        serial_println!("{}", t);
        self.tokens += 1;
        self.last_ok = r.is_ok();
        self.last_why = r.err().unwrap_or("");
    }

    /// The composite, built once per document change.
    pub fn composite(&mut self) -> &(Vec<u8>, Option<Mask>) {
        if self.comp.is_none() {
            let c = self.doc.composite();
            let m = self.doc.selection.as_ref().map(|s| s.rasterize(self.doc.width, self.doc.height));
            self.comp = Some((c, m));
        }
        self.comp.as_ref().expect("built above")
    }

    // ---- the ops (tools/raster ops.rs's shapes) ------------------------------------------------------------------

    /// `History::commit` on the document; then the history is held to its bound.
    fn commit(&mut self, cmd: Command) -> Result<(), Word> {
        let next = self.hist.commit(&self.doc, &cmd).map_err(|e| e.word())?;
        self.doc = next;
        self.comp = None;
        self.trim();
        Ok(())
    }

    /// The face's memory bound on the history (FACETEDIT.md §Memory): the oldest snapshots go first.
    fn trim(&mut self) {
        let bytes = |d: &Document| d.layers.iter().map(|l| l.pixels.len()).sum::<usize>();
        let mut total: usize = self.hist.past.iter().map(bytes).sum();
        while !self.hist.past.is_empty() && (self.hist.past.len() > UNDO_MAX || total > UNDO_BUDGET) {
            total -= bytes(&self.hist.past[0]);
            self.hist.past.remove(0);
        }
    }

    /// A core call over a copy of layer `i`'s pixels, landed by `PastePixels` over the whole layer (ops.rs `paste`).
    fn paste(&mut self, i: usize, f: impl FnOnce(&Layer) -> Result<Vec<u8>, Word>) -> Result<(), Word> {
        let l = self.doc.layer(i).map_err(|e| e.word())?;
        let (w, h) = (l.width, l.height);
        let rgba = f(l)?;
        self.commit(Command::PastePixels { layer: i, x: 0, y: 0, w, h, rgba })
    }

    fn clamp_active(&mut self) {
        self.active = self.active.min(self.doc.layers.len().saturating_sub(1));
    }

    pub fn undo(&mut self) {
        let r = if self.hist.past.is_empty() {
            Err("no-undo")
        } else {
            let cur = core::mem::replace(&mut self.doc, empty_doc());
            self.doc = self.hist.undo(cur).expect("past is not empty");
            self.comp = None;
            self.clamp_active();
            Ok(())
        };
        self.emit("undo", "", r);
    }

    pub fn redo(&mut self) {
        let r = if self.hist.future.is_empty() {
            Err("no-redo")
        } else {
            let cur = core::mem::replace(&mut self.doc, empty_doc());
            self.doc = self.hist.redo(cur).expect("future is not empty");
            self.comp = None;
            self.clamp_active();
            Ok(())
        };
        self.emit("redo", "", r);
    }

    pub fn zoom(&mut self, steps: i32, cx: i32, cy: i32) {
        let r = if self.view.zoom_at(steps, cx, cy) { Ok(()) } else { Err("zoom-bound") };
        let (ox, oy) = self.view.origin_px();
        let f = format!("zoom={} origin={},{}", self.view.scale_word(), ox, oy);
        self.emit("zoom", &f, r);
    }

    pub fn zoom_centre(&mut self, steps: i32) {
        let (w, h) = self.region;
        self.zoom(steps, (w / 2) as i32, (h / 2) as i32);
    }

    pub fn set_tool(&mut self, t: Tool) {
        self.tool = t;
        self.emit("tool", &format!("tool={}", t.word()), Ok(()));
    }

    /// A field slider: `key` set to `v` (clamped to the field's range).
    pub fn set_field(&mut self, key: &str, v: u32) {
        let b = v.min(255) as u8;
        let ok = match key {
            "diameter" => {
                self.diameter = v.clamp(1, 64);
                true
            }
            "hardness" => {
                self.hardness = b;
                true
            }
            "flow" => {
                self.flow = b;
                true
            }
            "red" => {
                self.colour[0] = b;
                true
            }
            "green" => {
                self.colour[1] = b;
                true
            }
            "blue" => {
                self.colour[2] = b;
                true
            }
            "alpha" => {
                self.colour[3] = b;
                true
            }
            _ => false,
        };
        self.emit("field", &format!("key={}", word(key)), if ok { Ok(()) } else { Err("bad-field") });
    }

    /// A strip row: the active layer (no op).
    pub fn select_layer(&mut self, i: usize) {
        self.active = i;
        self.emit("layer", &format!("op=select index={}", i), Ok(()));
    }

    /// A strip eye (or the Eye button on the active layer): `SetVisible`.
    pub fn toggle_eye(&mut self, i: usize) {
        let cmd = self.doc.layer(i).map_err(|e| e.word()).map(|l| Command::SetVisible { layer: i, visible: !l.visible });
        let r = cmd.and_then(|c| self.commit(c));
        self.emit("layer", &format!("op=visible index={}", i), r);
    }

    /// The layer / view verbs (the host face's `verb`, less its file verbs: the window runs Save / Open).
    pub fn verb(&mut self, v: &str) -> bool {
        let a = self.active;
        match v {
            "zoom+" => self.zoom_centre(1),
            "zoom-" => self.zoom_centre(-1),
            "undo" => self.undo(),
            "redo" => self.redo(),
            "layer-add" => {
                let n = self.doc.layers.len();
                let r = if n >= LAYER_CAP {
                    Err("layer-cap")
                } else {
                    let layer = Layer::new(&format!("Layer {}", n), self.doc.width, self.doc.height);
                    self.commit(Command::AddLayer { index: n, layer })
                };
                if r.is_ok() {
                    self.active = n;
                }
                self.emit("layer", &format!("op=add index={}", n), r);
            }
            "layer-remove" => {
                let r = self.commit(Command::RemoveLayer { index: a });
                self.clamp_active();
                self.emit("layer", &format!("op=remove index={}", a), r);
            }
            "layer-up" => {
                let r = self.commit(Command::MoveLayer { from: a, to: a + 1 });
                if r.is_ok() {
                    self.active = a + 1;
                }
                self.emit("layer", &format!("op=up index={}", a), r);
            }
            "layer-down" => {
                let r = match a.checked_sub(1) {
                    Some(to) => self.commit(Command::MoveLayer { from: a, to }),
                    None => Err(raster_core::Error::NoLayer.word()),
                };
                if r.is_ok() {
                    self.active = a - 1;
                }
                self.emit("layer", &format!("op=down index={}", a), r);
            }
            "layer-blend" => {
                let cmd = self.doc.layer(a).map_err(|e| e.word()).map(|l| {
                    let next = BlendMode::ALL[(l.blend.to_byte() as usize + 1) % BlendMode::ALL.len()];
                    Command::SetBlend { layer: a, blend: next }
                });
                let r = cmd.and_then(|c| self.commit(c));
                self.emit("layer", &format!("op=blend index={}", a), r);
            }
            "layer-eye" => self.toggle_eye(a),
            _ => return false,
        }
        true
    }

    /// One adjust_core function on the active layer with the host face's default fields (ops.rs `adjust` + `paste`).
    pub fn adjust(&mut self, kind: &str) -> bool {
        if !ADJUSTS.iter().any(|(k, _)| *k == kind) {
            return false;
        }
        let a = self.active;
        let r = self.paste(a, |l| {
            let (p, w, h) = (&l.pixels[..], l.width, l.height);
            let out = match kind {
                "levels" => adjust_core::levels(p, w, h, adjust_core::Levels { in_lo: 0, in_hi: 255, gamma_q8: 256, out_lo: 0, out_hi: 255 }),
                "curves" => {
                    let c = adjust_core::Curves { master: adjust_core::Curves::from_points(&[]), r: None, g: None, b: None };
                    adjust_core::curves(p, w, h, &c)
                }
                "huesat" => adjust_core::hue_sat(p, w, h, adjust_core::HueSat { hue_deg: 0, sat_pct: 0, light_pct: 0 }),
                "invert" => adjust_core::invert(p, w, h),
                "threshold" => adjust_core::threshold(p, w, h, 128),
                "boxblur" => adjust_core::box_blur(p, w, h, 1),
                "gaussian" => adjust_core::gaussian_blur(p, w, h, 256),
                _ => adjust_core::unsharp(p, w, h, adjust_core::Unsharp { amount_pct: 100, sigma_q8: 256, threshold: 0 }),
            };
            out.map_err(|e| e.word())
        });
        self.emit("adjust", &format!("kind={} layer={}", kind, a), r);
        true
    }

    /// A decoded container replaces the document; the history is cleared and the view refitted (ops.rs `Op::Open`).
    pub fn replace(&mut self, doc: Document) {
        self.doc = doc;
        self.hist = History::new();
        self.comp = None;
        self.active = self.doc.layers.len().saturating_sub(1);
        self.view = View::fit(self.doc.width, self.doc.height, self.region.0, self.region.1);
    }

    // ---- the canvas drag -----------------------------------------------------------------------------------------

    /// A press in the canvas region (region coordinates): the drag begins.
    pub fn press(&mut self, x: i32, y: i32) {
        let pan = self.pan_armed || self.tool == Tool::Move;
        self.drag = Some(Drag { pan, start: (x, y), last: (x, y), points: alloc::vec![(x, y)] });
    }

    /// A sampled pointer position while the button is held.
    pub fn motion(&mut self, x: i32, y: i32) -> bool {
        let Some(d) = self.drag.as_mut() else { return false };
        if d.last == (x, y) {
            return false;
        }
        let (dx, dy) = (x - d.last.0, y - d.last.1);
        d.last = (x, y);
        if d.pan {
            self.view.pan(dx, dy);
        } else {
            d.points.push((x, y));
        }
        true
    }

    /// The button came up at `(x, y)`: a pan's token, or the tool's ONE op.
    pub fn release(&mut self, x: i32, y: i32) {
        let Some(mut d) = self.drag.take() else { return };
        if d.pan {
            self.view.pan(x - d.last.0, y - d.last.1);
            self.pan_armed = false;
            let (ox, oy) = self.view.origin_px();
            let f = format!("dx={} dy={} origin={},{}", x - d.start.0, y - d.start.1, ox, oy);
            self.emit("pan", &f, Ok(()));
            return;
        }
        if d.points.last() != Some(&(x, y)) {
            d.points.push((x, y));
        }
        d.last = (x, y);
        self.canvas_gesture(&d);
    }

    /// A canvas point (Q8 canvas) rounded and clamped to `0..=m` (the host's `round().clamp`).
    fn round_clamp(v: i64, m: u32) -> i32 {
        ((v + 128) >> 8).clamp(0, m as i64) as i32
    }

    /// The canvas cell box between two region points, clamped to the document: `(x0, y0, x1, y1)`.
    fn cell_box(&self, a: (i32, i32), b: (i32, i32)) -> (i32, i32, i32, i32) {
        let (ax, ay) = self.view.to_canvas_q8(a.0 as i64 * 256, a.1 as i64 * 256);
        let (bx, by) = self.view.to_canvas_q8(b.0 as i64 * 256, b.1 as i64 * 256);
        let (w, h) = (self.doc.width, self.doc.height);
        (Self::round_clamp(ax.min(bx), w), Self::round_clamp(ay.min(by), h), Self::round_clamp(ax.max(bx), w), Self::round_clamp(ay.max(by), h))
    }

    /// The stroke in the active layer's coordinates (canvas less the layer's offset), pressure 255.
    fn stroke(&self, d: &Drag) -> brush_core::Stroke {
        let off = self.doc.layers.get(self.active).map_or((0, 0), |l| l.offset);
        let points = d
            .points
            .iter()
            .map(|&(sx, sy)| {
                let (x, y) = self.view.to_canvas_q8(sx as i64 * 256, sy as i64 * 256);
                let (x, y) = (x - off.0 as i64 * 256, y - off.1 as i64 * 256);
                brush_core::StrokePoint { x: x as f32 / 256.0, y: y as f32 / 256.0, pressure: 255 }
            })
            .collect();
        brush_core::Stroke { points }
    }

    fn brush_of(&self, mode: BlendMode) -> brush_core::Brush {
        let mut b = brush_core::Brush::new(self.diameter as f32);
        b.hardness = self.hardness;
        b.flow = self.flow;
        b.mode = mode;
        b
    }

    /// The tool's ONE op for a finished canvas drag (or click) — the host face's `canvas_gesture`, arm for arm.
    fn canvas_gesture(&mut self, d: &Drag) {
        let end = d.last;
        let g = self.tool.word();
        match self.tool {
            Tool::SelectRect | Tool::SelectEllipse => {
                let (x0, y0, x1, y1) = self.cell_box(d.start, end);
                let (w, h) = (x1 - x0, y1 - y0);
                let sel = if self.tool == Tool::SelectRect {
                    (w > 0 && h > 0).then_some(Selection::Rect { x: x0, y: y0, w, h })
                } else {
                    let (rx, ry) = (w / 2, h / 2);
                    (rx > 0 && ry > 0).then_some(Selection::Ellipse { cx: x0 + rx, cy: y0 + ry, rx, ry })
                };
                let f = if sel.is_some() { format!("x={} y={} w={} h={}", x0, y0, w, h) } else { String::from("none") };
                let r = self.commit(Command::SetSelection(sel));
                self.emit(g, &f, r);
            }
            Tool::SelectLasso => {
                let (w, h) = (self.doc.width, self.doc.height);
                let mut pts: Vec<(i32, i32)> = Vec::new();
                for &(sx, sy) in &d.points {
                    let (x, y) = self.view.to_canvas_q8(sx as i64 * 256, sy as i64 * 256);
                    let p = (Self::round_clamp(x, w), Self::round_clamp(y, h));
                    if pts.last() != Some(&p) {
                        pts.push(p);
                    }
                }
                let n = pts.len();
                let sel = (n >= 3).then_some(Selection::Polygon(pts));
                let r = self.commit(Command::SetSelection(sel));
                self.emit(g, &format!("points={}", n), r);
            }
            Tool::Wand => {
                let (x, y) = self.view.cell(end.0, end.1);
                let (x, y) = (x.clamp(i32::MIN as i64, i32::MAX as i64) as i32, y.clamp(i32::MIN as i64, i32::MAX as i64) as i32);
                let a = self.active;
                let r = Selection::magic_wand(&self.doc, a, x, y, self.tolerance)
                    .map_err(|e| e.word())
                    .and_then(|s| self.commit(Command::SetSelection(Some(s))));
                self.emit(g, &format!("x={} y={} tol={} layer={}", x, y, self.tolerance, a), r);
            }
            Tool::Brush | Tool::Eraser => {
                let a = self.active;
                let mode = if self.tool == Tool::Eraser { BlendMode::Erase } else { BlendMode::Normal };
                let (stroke, brush, rgba) = (self.stroke(d), self.brush_of(mode), self.colour);
                let n = stroke.points.len();
                let r = self.paste(a, |l| {
                    let mut px = l.pixels.clone();
                    brush_core::paint(&mut px, l.width, l.height, &brush, &stroke, rgba);
                    Ok(px)
                });
                self.emit(g, &format!("points={} layer={}", n, a), r);
            }
            Tool::Fill => {
                let a = self.active;
                let c = self.colour;
                let r = self.commit(Command::FillSelection { layer: a, rgba: c });
                self.emit(g, &format!("layer={} rgba={:02x}{:02x}{:02x}{:02x}", a, c[0], c[1], c[2], c[3]), r);
            }
            Tool::Move => {}
        }
    }
}
