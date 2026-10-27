// CHARTER: Facet (Images) — host face over the shared cores (RASTERFACE, LEDGER SR234)
// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! `facet-view --edit` — Facet's editor face (RASTERFACE, LEDGER SR234), in stria-view's shape.
//!
//! ONE Quartzite tetra tree (the tool rail, the one surface, the layer / adjust / file row, the field line) is
//! evaluated by Quartzite's `eval_tetra` on AppKit (macOS) and GTK (`--features gtk`); no Qt. The model,
//! [`Editor`], is platform-free: ONE raster_core `Document`, ONE `History`, the active layer, the view
//! ([`canvas::View`]), the tool ([`tools::Tool`]) and the fields. [`Editor::on`] takes the bus messages the window
//! sends; every gesture is ONE op in tools/raster's `ops.rs` shapes (a `Command` through `History::commit`, or a
//! core call on a copy of a layer + `PastePixels`) and ONE witness token ([`witness`]). The face holds no editing
//! logic: the cores do every edit and are the one gate (a refused op leaves the document unchanged).
//!
//! ```text
//! facet-view --edit <file.unarastr> [--size WxH]                   # the window
//! facet-view --synth new 64x48 [--size WxH] [--do "<tokens>"]      # a new document; --do = headless witness
//! ```
//! Design, the gesture -> op table and the tokens: `docs/dev/evidence/raster/RASTERFACE.md`.

pub mod canvas;
pub mod layers;
pub mod tools;
pub mod witness;

use bandy::{SMessage, Synapse};
use quartzite::tetra::{TetraNode, TextAction};
use raster_core::{BlendMode, Command, Document, History, Layer, Selection};

use canvas::View;
use layers::{Hit, PANEL_W};
use tools::{AdjustFields, BrushFields, Tool};
use witness::Facts;

/// The surface the editor paints.
pub const SURFACE: &str = "facet://editor";
/// The `Input` target the buttons fire at.
pub const TARGET: &str = "facet.editor";

/// The byte order of the frame: GTK's surface hands its buffer to cairo ARgb32 (BGRA on little-endian), AppKit's
/// and the headless witness take RGBA (NLE2-ORDER).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelOrder {
    Rgba,
    Bgra,
}

/// A failed op's word: a core's `Error::word()` or the face's own.
type Word = &'static str;

/// The drag in flight.
#[derive(Debug, Clone)]
struct Drag {
    /// Started in the layers panel (a panel click on release).
    panel: bool,
    /// A view pan (the Move tool, or a drag armed by space).
    pan: bool,
    start: (f64, f64),
    last: (f64, f64),
    /// The screen points seen (lasso, brush).
    points: Vec<(f64, f64)>,
}

/// The editor's state.
pub struct Editor {
    pub doc: Document,
    pub hist: History,
    pub active: usize,
    pub view: View,
    pub tool: Tool,
    pub brush: BrushFields,
    pub colour: [u8; 4],
    pub tolerance: u8,
    pub adjust: AdjustFields,
    /// The surface size.
    pub size: (u32, u32),
    /// The file last opened or saved.
    pub path: Option<String>,
    pub order: PixelOrder,
    drag: Option<Drag>,
    pointer: (f64, f64),
    pan_armed: bool,
    /// The surface sends `BrowserButton` (its `BrowserClick` is then ignored: stria's NLE2B rule).
    buttons: bool,
    /// The witness tokens not yet printed.
    pub tokens: Vec<String>,
}

impl Editor {
    fn with(doc: Document, size: (u32, u32), order: PixelOrder, path: Option<String>) -> Editor {
        let size = (size.0.max(PANEL_W + 1), size.1.max(1));
        let view = View::fit(doc.width, doc.height, size.0 - PANEL_W, size.1);
        let active = doc.layers.len().saturating_sub(1);
        Editor {
            doc,
            hist: History::new(),
            active,
            view,
            tool: Tool::Brush,
            brush: BrushFields::default(),
            colour: [0, 0, 0, 255],
            tolerance: 32,
            adjust: AdjustFields::default(),
            size,
            path,
            order,
            drag: None,
            pointer: (0.0, 0.0),
            pan_armed: false,
            buttons: false,
            tokens: Vec::new(),
        }
    }

    /// A new `w x h` document with one transparent "Background" layer (tools/raster's `new`). `Err` is the refused
    /// token.
    pub fn new(w: u32, h: u32, size: (u32, u32), order: PixelOrder) -> Result<Editor, String> {
        let fields = format!("width={w} height={h}");
        let doc = Document::new(w, h)
            .and_then(|d| d.apply(&Command::AddLayer { index: 0, layer: Layer::new("Background", w, h) }))
            .map_err(|e| witness::token("new", &fields, Err(e.word()), Facts { layers: 0, active: 0, undo: 0, redo: 0 }))?;
        let mut ed = Editor::with(doc, size, order, None);
        ed.emit("new", &fields, Ok(()));
        Ok(ed)
    }

    /// The document in `path` (raster_core's container; tools/raster's `open`). `Err` is the refused token.
    pub fn open(path: &str, size: (u32, u32), order: PixelOrder) -> Result<Editor, String> {
        let none = Facts { layers: 0, active: 0, undo: 0, redo: 0 };
        let doc = read_doc(path).map_err(|why| witness::token("open", &format!("path={}", witness::word(path)), Err(why), none))?;
        let fields = format!("path={} width={} height={}", witness::word(path), doc.width, doc.height);
        let mut ed = Editor::with(doc, size, order, Some(path.to_string()));
        ed.emit("open", &fields, Ok(()));
        Ok(ed)
    }

    pub fn facts(&self) -> Facts {
        Facts { layers: self.doc.layers.len(), active: self.active, undo: self.hist.past.len(), redo: self.hist.future.len() }
    }

    /// Record one gesture's token.
    fn emit(&mut self, gesture: &str, fields: &str, r: Result<(), Word>) {
        let t = witness::token(gesture, fields, r, self.facts());
        self.tokens.push(t);
    }

    /// The canvas region's size (the surface less the panel).
    pub fn region(&self) -> (u32, u32) {
        (self.size.0 - PANEL_W, self.size.1)
    }

    // ---- the ops (tools/raster ops.rs's shapes) --------------------------------------------------------------------

    /// `History::commit` on the document (ops.rs `State::commit`).
    fn commit(&mut self, cmd: Command) -> Result<(), Word> {
        let next = self.hist.commit(&self.doc, &cmd).map_err(|e| e.word())?;
        self.doc = next;
        Ok(())
    }

    /// A core call over a copy of layer `i`'s pixels, landed by `PastePixels` over the whole layer (ops.rs `paste`).
    fn paste(&mut self, i: usize, f: impl FnOnce(&Layer) -> Result<Vec<u8>, Word>) -> Result<(), Word> {
        let l = self.doc.layer(i).map_err(|e| e.word())?;
        let (w, h) = (l.width, l.height);
        let rgba = f(l)?;
        self.commit(Command::PastePixels { layer: i, x: 0, y: 0, w, h, rgba })
    }

    /// The active layer's index clamped to the stack (after an op that changed its length).
    fn clamp_active(&mut self) {
        self.active = self.active.min(self.doc.layers.len().saturating_sub(1));
    }

    fn undo(&mut self) {
        let r = if self.hist.past.is_empty() {
            Err("no-undo")
        } else {
            let cur = std::mem::replace(&mut self.doc, Document { width: 0, height: 0, layers: Vec::new(), selection: None });
            self.doc = self.hist.undo(cur).expect("past is not empty");
            self.clamp_active();
            Ok(())
        };
        self.emit("undo", "", r);
    }

    fn redo(&mut self) {
        let r = if self.hist.future.is_empty() {
            Err("no-redo")
        } else {
            let cur = std::mem::replace(&mut self.doc, Document { width: 0, height: 0, layers: Vec::new(), selection: None });
            self.doc = self.hist.redo(cur).expect("future is not empty");
            self.clamp_active();
            Ok(())
        };
        self.emit("redo", "", r);
    }

    fn zoom(&mut self, steps: i32, cx: f64, cy: f64) {
        let r = if self.view.zoom_at(steps, cx, cy) { Ok(()) } else { Err("zoom-bound") };
        let f = format!("zoom={} origin={},{}", self.view.scale_word(), self.view.origin.0, self.view.origin.1);
        self.emit("zoom", &f, r);
    }

    fn zoom_centre(&mut self, steps: i32) {
        let (w, h) = self.region();
        self.zoom(steps, w as f64 / 2.0, h as f64 / 2.0);
    }

    /// A layer op: `op` names it in the token.
    fn layer_op(&mut self, op: &str, cmd: Result<Command, Word>) {
        let index = self.active;
        let r = cmd.and_then(|c| self.commit(c));
        self.emit("layer", &format!("op={op} index={index}"), r);
    }

    /// The button verbs (`tools::is_verb`).
    pub fn verb(&mut self, v: &str) -> bool {
        if let Some(t) = v.strip_prefix("tool-").and_then(Tool::from_word) {
            self.tool = t;
            self.emit("tool", &format!("tool={}", t.word()), Ok(()));
            return true;
        }
        if let Some(kind) = v.strip_prefix("adjust-") {
            return self.adjust(kind);
        }
        let a = self.active;
        match v {
            "zoom+" => self.zoom_centre(1),
            "zoom-" => self.zoom_centre(-1),
            "undo" => self.undo(),
            "redo" => self.redo(),
            "layer-add" => {
                let n = self.doc.layers.len();
                let layer = Layer::new(&format!("Layer {n}"), self.doc.width, self.doc.height);
                let r = self.commit(Command::AddLayer { index: n, layer });
                if r.is_ok() {
                    self.active = n;
                }
                self.emit("layer", &format!("op=add index={n}"), r);
            }
            "layer-remove" => {
                let r = self.commit(Command::RemoveLayer { index: a });
                self.clamp_active();
                self.emit("layer", &format!("op=remove index={a}"), r);
            }
            "layer-up" => {
                let r = self.commit(Command::MoveLayer { from: a, to: a + 1 });
                if r.is_ok() {
                    self.active = a + 1;
                }
                self.emit("layer", &format!("op=up index={a}"), r);
            }
            "layer-down" => {
                let r = match a.checked_sub(1) {
                    Some(to) => self.commit(Command::MoveLayer { from: a, to }),
                    None => Err(raster_core::Error::NoLayer.word()),
                };
                if r.is_ok() {
                    self.active = a - 1;
                }
                self.emit("layer", &format!("op=down index={a}"), r);
            }
            "layer-blend" => {
                let cmd = self.doc.layer(a).map_err(|e| e.word()).map(|l| {
                    let next = BlendMode::ALL[(l.blend.to_byte() as usize + 1) % BlendMode::ALL.len()];
                    Command::SetBlend { layer: a, blend: next }
                });
                self.layer_op("blend", cmd);
            }
            "layer-eye" => {
                let cmd = self.doc.layer(a).map_err(|e| e.word()).map(|l| Command::SetVisible { layer: a, visible: !l.visible });
                self.layer_op("visible", cmd);
            }
            "save" => match self.path.clone() {
                Some(p) => self.save(&p),
                None => self.emit("save", "", Err("no-path")),
            },
            "export" => match self.path.clone() {
                Some(p) => self.export(&std::path::Path::new(&p).with_extension("png").to_string_lossy()),
                None => self.emit("export", "", Err("no-path")),
            },
            _ => return false,
        }
        true
    }

    /// One adjust_core function on the active layer with the current fields (ops.rs `adjust` + `paste`).
    fn adjust(&mut self, kind: &str) -> bool {
        if !tools::ADJUSTS.iter().any(|(k, _)| *k == kind) {
            return false;
        }
        let a = self.active;
        let f = self.adjust.clone();
        let r = self.paste(a, |l| {
            let (p, w, h) = (&l.pixels[..], l.width, l.height);
            let out = match kind {
                "levels" => adjust_core::levels(p, w, h, f.levels),
                "curves" => {
                    let c = adjust_core::Curves { master: adjust_core::Curves::from_points(&f.curves), r: None, g: None, b: None };
                    adjust_core::curves(p, w, h, &c)
                }
                "huesat" => adjust_core::hue_sat(p, w, h, f.huesat),
                "invert" => adjust_core::invert(p, w, h),
                "threshold" => adjust_core::threshold(p, w, h, f.threshold),
                "boxblur" => adjust_core::box_blur(p, w, h, f.boxblur),
                "gaussian" => adjust_core::gaussian_blur(p, w, h, f.gaussian),
                _ => adjust_core::unsharp(p, w, h, f.unsharp),
            };
            out.map_err(|e| e.word())
        });
        self.emit("adjust", &format!("kind={kind} layer={a}"), r);
        true
    }

    fn save(&mut self, path: &str) {
        let r = raster_core::encode(&self.doc).map_err(|e| e.word()).and_then(|b| std::fs::write(path, b).map_err(|_| "io"));
        if r.is_ok() {
            self.path = Some(path.to_string());
        }
        self.emit("save", &format!("path={}", witness::word(path)), r);
    }

    fn export(&mut self, path: &str) {
        let d = &self.doc;
        let r = pixel_core::png::encode::encode_rgba(d.width, d.height, &d.composite())
            .map_err(|_| "png-encode")
            .and_then(|png| std::fs::write(path, png).map_err(|_| "io"));
        self.emit("export", &format!("path={}", witness::word(path)), r);
    }

    fn open_path(&mut self, path: &str) {
        let r = read_doc(path).map(|doc| {
            self.doc = doc;
            self.hist = History::new();
            self.active = self.doc.layers.len().saturating_sub(1);
            let (w, h) = self.region();
            self.view = View::fit(self.doc.width, self.doc.height, w, h);
            self.path = Some(path.to_string());
        });
        self.emit("open", &format!("path={}", witness::word(path)), r);
    }

    /// A field line from the TextField: `key=value`.
    fn field(&mut self, line: &str) -> bool {
        let Some((k, v)) = line.split_once('=') else { return false };
        let (k, v) = (k.trim(), v.trim());
        let a = self.active;
        let nums = |s: &str| -> Option<Vec<i64>> { s.split(',').map(|t| t.trim().parse().ok()).collect() };
        let ok = match k {
            "opacity" => {
                let cmd = v.parse::<u8>().map(|o| Command::SetOpacity { layer: a, opacity: o }).map_err(|_| "bad-field");
                self.layer_op("opacity", cmd);
                return true;
            }
            "blend" => {
                let cmd = tools::parse_blend(v).map(|b| Command::SetBlend { layer: a, blend: b }).ok_or("bad-field");
                self.layer_op("blend", cmd);
                return true;
            }
            "rename" => {
                self.layer_op("rename", Ok(Command::Rename { layer: a, name: v.to_string() }));
                return true;
            }
            "open" => {
                self.open_path(v);
                return true;
            }
            "save" => {
                self.save(v);
                return true;
            }
            "export" => {
                self.export(v);
                return true;
            }
            "color" | "colour" => tools::parse_rgba(v).map(|c| self.colour = c).is_some(),
            "diameter" => v.parse::<f32>().ok().filter(|d| *d > 0.0).map(|d| self.brush.diameter = d).is_some(),
            "hardness" => v.parse::<u8>().ok().map(|x| self.brush.hardness = x).is_some(),
            "flow" => v.parse::<u8>().ok().map(|x| self.brush.flow = x).is_some(),
            "spacing" => v.parse::<f32>().ok().filter(|s| *s > 0.0).map(|s| self.brush.spacing = s).is_some(),
            "tolerance" => v.parse::<u8>().ok().map(|x| self.tolerance = x).is_some(),
            "levels" => match nums(v).as_deref() {
                Some(&[a, b, g, c, d]) => (|| {
                    self.adjust.levels = adjust_core::Levels {
                        in_lo: a.try_into().ok()?,
                        in_hi: b.try_into().ok()?,
                        gamma_q8: g.try_into().ok()?,
                        out_lo: c.try_into().ok()?,
                        out_hi: d.try_into().ok()?,
                    };
                    Some(())
                })()
                .is_some(),
                _ => false,
            },
            "curves" => {
                let pts: Option<Vec<(u8, u8)>> = v
                    .split(|c: char| c == ';' || c.is_whitespace())
                    .filter(|s| !s.is_empty())
                    .map(|s| {
                        let (x, y) = s.split_once(',')?;
                        Some((x.parse().ok()?, y.parse().ok()?))
                    })
                    .collect();
                pts.map(|p| self.adjust.curves = p).is_some()
            }
            "huesat" => match nums(v).as_deref() {
                Some(&[h, s, l]) => (|| {
                    self.adjust.huesat =
                        adjust_core::HueSat { hue_deg: h.try_into().ok()?, sat_pct: s.try_into().ok()?, light_pct: l.try_into().ok()? };
                    Some(())
                })()
                .is_some(),
                _ => false,
            },
            "threshold" => v.parse::<u8>().ok().map(|x| self.adjust.threshold = x).is_some(),
            "boxblur" => v.parse::<u16>().ok().map(|x| self.adjust.boxblur = x).is_some(),
            "gaussian" => v.parse::<u16>().ok().map(|x| self.adjust.gaussian = x).is_some(),
            "unsharp" => match nums(v).as_deref() {
                Some(&[am, s, t]) => (|| {
                    self.adjust.unsharp = adjust_core::Unsharp {
                        amount_pct: am.try_into().ok()?,
                        sigma_q8: s.try_into().ok()?,
                        threshold: t.try_into().ok()?,
                    };
                    Some(())
                })()
                .is_some(),
                _ => false,
            },
            _ => false,
        };
        let r = if ok { Ok(()) } else { Err("bad-field") };
        self.emit("field", &format!("key={}", witness::word(k)), r);
        true
    }

    // ---- the input -----------------------------------------------------------------------------------------------

    /// One bus message from the window. True when the frame changed.
    pub fn on(&mut self, msg: &SMessage) -> bool {
        match msg {
            SMessage::Input { target, text } if target == TARGET => self.verb(text),
            SMessage::BrowserClick(x, y) if !self.buttons => {
                self.press(*x, *y);
                self.release(*x, *y)
            }
            SMessage::BrowserButton { down: true, x, y } => {
                self.buttons = true;
                self.press(*x, *y)
            }
            SMessage::BrowserButton { down: false, x, y } => self.release(*x, *y),
            SMessage::BrowserMouseMove(x, y) => self.motion(*x, *y),
            SMessage::BrowserScroll(_, dy) if *dy != 0.0 => {
                let (x, y) = self.pointer;
                self.zoom(if *dy < 0.0 { 1 } else { -1 }, x, y);
                true
            }
            SMessage::BrowserResize(w, h) if *w > PANEL_W && *h > 0 => {
                self.size = (*w, *h);
                true
            }
            SMessage::BrowserKey(k) => self.key(k),
            SMessage::BrowserText(t) if t.chars().count() == 1 => self.key(t),
            SMessage::BrowserText(t) if t.contains('=') => self.field(t),
            _ => false,
        }
    }

    /// A key: one-character keys come as `BrowserText`, named keys as `BrowserKey` (stria's KEYS rule).
    fn key(&mut self, k: &str) -> bool {
        match k {
            "+" | "=" => self.zoom_centre(1),
            "-" => self.zoom_centre(-1),
            "Z" | "Ctrl+Shift+Z" | "Meta+Shift+Z" => self.redo(),
            "Ctrl+z" | "Meta+z" => self.undo(),
            " " => self.pan_armed = true,
            "Escape" => {
                let had = self.drag.take().is_some();
                self.pan_armed = false;
                return had;
            }
            _ => return false,
        }
        true
    }

    fn press(&mut self, x: f64, y: f64) -> bool {
        self.pointer = (x, y);
        let panel = x >= self.region().0 as f64;
        let pan = !panel && (self.pan_armed || self.tool == Tool::Move);
        self.drag = Some(Drag { panel, pan, start: (x, y), last: (x, y), points: vec![(x, y)] });
        false
    }

    fn motion(&mut self, x: f64, y: f64) -> bool {
        self.pointer = (x, y);
        let Some(d) = self.drag.as_mut() else { return false };
        let (dx, dy) = (x - d.last.0, y - d.last.1);
        d.last = (x, y);
        if d.panel {
            return false;
        }
        if d.pan {
            self.view.pan(dx, dy);
        } else {
            d.points.push((x, y));
        }
        true
    }

    fn release(&mut self, x: f64, y: f64) -> bool {
        self.pointer = (x, y);
        let Some(mut d) = self.drag.take() else { return false };
        if d.panel {
            return self.panel_click(x - self.region().0 as f64, y);
        }
        if d.pan {
            self.view.pan(x - d.last.0, y - d.last.1);
            self.pan_armed = false;
            let f = format!(
                "dx={} dy={} origin={},{}",
                x - d.start.0,
                y - d.start.1,
                self.view.origin.0,
                self.view.origin.1
            );
            self.emit("pan", &f, Ok(()));
            return true;
        }
        if d.points.last() != Some(&(x, y)) {
            d.points.push((x, y));
        }
        self.canvas_gesture(&d);
        true
    }

    /// A click in the layers panel (panel-relative point).
    fn panel_click(&mut self, px: f64, py: f64) -> bool {
        match layers::hit(px, py, self.doc.layers.len()) {
            Hit::Row(i) => {
                self.active = i;
                self.emit("layer", &format!("op=select index={i}"), Ok(()));
            }
            Hit::Eye(i) => {
                let cmd = self.doc.layer(i).map_err(|e| e.word()).map(|l| Command::SetVisible { layer: i, visible: !l.visible });
                let r = cmd.and_then(|c| self.commit(c));
                self.emit("layer", &format!("op=visible index={i}"), r);
            }
            Hit::None => return false,
        }
        true
    }

    /// The canvas cell box between two screen points, clamped to the document: `(x0, y0, x1, y1)`.
    fn cell_box(&self, a: (f64, f64), b: (f64, f64)) -> (i32, i32, i32, i32) {
        let (ax, ay) = self.view.to_canvas(a.0, a.1);
        let (bx, by) = self.view.to_canvas(b.0, b.1);
        let (w, h) = (self.doc.width as f64, self.doc.height as f64);
        let cl = |v: f64, m: f64| v.round().clamp(0.0, m) as i32;
        (cl(ax.min(bx), w), cl(ay.min(by), h), cl(ax.max(bx), w), cl(ay.max(by), h))
    }

    /// The stroke in the active layer's coordinates (canvas less the layer's offset), pressure 255.
    fn stroke(&self, d: &Drag) -> brush_core::Stroke {
        let off = self.doc.layers.get(self.active).map_or((0, 0), |l| l.offset);
        let points = d
            .points
            .iter()
            .map(|&(sx, sy)| {
                let (x, y) = self.view.to_canvas(sx, sy);
                brush_core::StrokePoint { x: (x - off.0 as f64) as f32, y: (y - off.1 as f64) as f32, pressure: 255 }
            })
            .collect();
        brush_core::Stroke { points }
    }

    fn brush_of(&self, mode: BlendMode) -> brush_core::Brush {
        let mut b = brush_core::Brush::new(self.brush.diameter);
        b.hardness = self.brush.hardness;
        b.spacing = self.brush.spacing;
        b.flow = self.brush.flow;
        b.mode = mode;
        b
    }

    fn brush_mode(&self) -> BlendMode {
        if self.tool == Tool::Eraser { BlendMode::Erase } else { BlendMode::Normal }
    }

    /// The tool's ONE op for a finished canvas drag (or click).
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
                let f = match &sel {
                    Some(_) => format!("x={x0} y={y0} w={w} h={h}"),
                    None => "none".to_string(),
                };
                let r = self.commit(Command::SetSelection(sel));
                self.emit(g, &f, r);
            }
            Tool::SelectLasso => {
                let (w, h) = (self.doc.width as f64, self.doc.height as f64);
                let mut pts: Vec<(i32, i32)> = Vec::new();
                for &(sx, sy) in &d.points {
                    let (x, y) = self.view.to_canvas(sx, sy);
                    let p = (x.round().clamp(0.0, w) as i32, y.round().clamp(0.0, h) as i32);
                    if pts.last() != Some(&p) {
                        pts.push(p);
                    }
                }
                let n = pts.len();
                let sel = (n >= 3).then_some(Selection::Polygon(pts));
                let r = self.commit(Command::SetSelection(sel));
                self.emit(g, &format!("points={n}"), r);
            }
            Tool::Wand => {
                let (x, y) = self.view.cell(end.0, end.1);
                let (x, y) = (x.clamp(i32::MIN as i64, i32::MAX as i64) as i32, y.clamp(i32::MIN as i64, i32::MAX as i64) as i32);
                let a = self.active;
                let r = Selection::magic_wand(&self.doc, a, x, y, self.tolerance)
                    .map_err(|e| e.word())
                    .and_then(|s| self.commit(Command::SetSelection(Some(s))));
                self.emit(g, &format!("x={x} y={y} tol={} layer={a}", self.tolerance), r);
            }
            Tool::Brush | Tool::Eraser => {
                let a = self.active;
                let (stroke, brush, rgba) = (self.stroke(d), self.brush_of(self.brush_mode()), self.colour);
                let n = stroke.points.len();
                let r = self.paste(a, |l| {
                    let mut px = l.pixels.clone();
                    brush_core::paint(&mut px, l.width, l.height, &brush, &stroke, rgba);
                    Ok(px)
                });
                self.emit(g, &format!("points={n} layer={a}"), r);
            }
            Tool::Fill => {
                let a = self.active;
                let c = self.colour;
                let r = self.commit(Command::FillSelection { layer: a, rgba: c });
                self.emit(g, &format!("layer={a} rgba={:02x}{:02x}{:02x}{:02x}", c[0], c[1], c[2], c[3]), r);
            }
            Tool::Move => {}
        }
    }

    // ---- the frame -----------------------------------------------------------------------------------------------

    /// The document as the frame shows it: with the brush stroke in flight painted on a copy.
    fn shown(&self) -> std::borrow::Cow<'_, Document> {
        let Some(d) = self.drag.as_ref().filter(|d| !d.panel && !d.pan) else { return std::borrow::Cow::Borrowed(&self.doc) };
        if !matches!(self.tool, Tool::Brush | Tool::Eraser) || self.active >= self.doc.layers.len() {
            return std::borrow::Cow::Borrowed(&self.doc);
        }
        let mut doc = self.doc.clone();
        let l = &mut doc.layers[self.active];
        let (w, h) = (l.width, l.height);
        brush_core::paint(&mut l.pixels, w, h, &self.brush_of(self.brush_mode()), &self.stroke(d), self.colour);
        std::borrow::Cow::Owned(doc)
    }

    /// The surface's frame: the canvas through the view, then the layers panel.
    pub fn frame(&self) -> SMessage {
        let (w, h) = self.size;
        let (rw, _) = self.region();
        let mut px = vec![0u8; (w * h * 4) as usize];
        let doc = self.shown();
        let comp = doc.composite();
        let sel = doc.selection.as_ref().map(|s| s.rasterize(doc.width, doc.height));
        canvas::paint(&mut px, w, rw, h, &comp, doc.width, doc.height, &self.view, sel.as_ref());
        layers::paint(&mut px, w, rw, h, &doc, self.active, self.colour);
        if self.order == PixelOrder::Bgra {
            for p in px.chunks_exact_mut(4) {
                p.swap(0, 2);
            }
        }
        SMessage::SurfaceBlit { url: SURFACE.into(), width: w, height: h, pixels: px }
    }

    /// The closing token: the document's facts, the view and the composite's fnv.
    pub fn stat(&self) -> String {
        let f = format!(
            "width={} height={} zoom={} origin={},{} tool={} fnv={:016x}",
            self.doc.width,
            self.doc.height,
            self.view.scale_word(),
            self.view.origin.0,
            self.view.origin.1,
            self.tool.word(),
            witness::fnv1a(&self.doc.composite())
        );
        witness::token("stat", &f, Ok(()), self.facts())
    }
}

/// Read a raster_core container.
fn read_doc(path: &str) -> Result<Document, Word> {
    let bytes = std::fs::read(path).map_err(|_| "io")?;
    raster_core::decode(&bytes).map_err(|e| e.word())
}

// ---- the window ------------------------------------------------------------------------------------------------------

/// THE editor tree, declared once: the tool rail, the surface, the layer / adjust / file row, the field line.
pub fn tree() -> TetraNode {
    let button = |verb: &str, label: &str| TetraNode::Button {
        id: verb.into(),
        label: label.into(),
        action: SMessage::Input { target: TARGET.into(), text: verb.into() },
    };
    let mut rail: Vec<TetraNode> = Tool::ALL.iter().map(|t| button(&format!("tool-{}", t.word()), t.label())).collect();
    rail.extend(tools::VIEW_VERBS.iter().map(|(v, l)| button(v, l)));
    let mut row: Vec<TetraNode> = tools::LAYER_VERBS.iter().map(|(v, l)| button(v, l)).collect();
    row.extend(tools::ADJUSTS.iter().map(|(k, l)| button(&format!("adjust-{k}"), l)));
    row.extend(tools::FILE_VERBS.iter().map(|(v, l)| button(v, l)));
    TetraNode::VStack(vec![
        TetraNode::HStack(rail),
        TetraNode::Surface { id: SURFACE.into() },
        TetraNode::HStack(row),
        TetraNode::TextField {
            id: "facet.editor.field".into(),
            placeholder: "color=ff0000ff  diameter=9  opacity=128  blend=multiply  levels=0,255,256,0,255  save=pic.unarastr"
                .into(),
            action: TextAction::BrowserText,
        },
    ])
}

/// Serve the editor on the bus: window messages in, frames out, each new token printed.
pub async fn serve(bus: Synapse, mut rx: tokio::sync::broadcast::Receiver<SMessage>, mut ed: Editor) {
    use tokio::sync::broadcast::error::RecvError;
    for t in ed.tokens.drain(..) {
        println!("{t}");
    }
    bus.fire(ed.frame());
    loop {
        match rx.recv().await {
            Ok(m) => {
                let changed = ed.on(&m);
                for t in ed.tokens.drain(..) {
                    println!("{t}");
                }
                if changed {
                    bus.fire(ed.frame());
                }
            }
            Err(RecvError::Lagged(_)) => continue,
            Err(RecvError::Closed) => break,
        }
    }
}

// ---- the --synth driver ----------------------------------------------------------------------------------------------

/// A `--do` token as the messages the window would send (GTK's shapes: a release is `BrowserButton` up then
/// `BrowserClick`). `active:<i>` is not a message: [`drive`] sets the index unchecked.
pub fn messages(ed: &Editor, tok: &str) -> Option<Vec<SMessage>> {
    let px = |s: &str| s.parse::<f64>().ok();
    if let Some(kv) = tok.strip_prefix("set:") {
        return kv.contains('=').then(|| vec![SMessage::BrowserText(kv.to_string())]);
    }
    if let Some(name) = tok.strip_prefix("key:") {
        let mut out = vec![SMessage::BrowserKey(name.to_string())];
        let text = if name == "space" { " " } else { name };
        if text.chars().count() == 1 {
            out.push(SMessage::BrowserText(text.to_string()));
        }
        return Some(out);
    }
    let (lx, ly) = ed.pointer;
    let parts: Vec<&str> = tok.split(':').collect();
    Some(match parts.as_slice() {
        ["press", x, y] => vec![SMessage::BrowserButton { down: true, x: px(x)?, y: px(y)? }],
        ["move", x, y] => vec![SMessage::BrowserMouseMove(px(x)?, px(y)?)],
        ["release"] => vec![SMessage::BrowserButton { down: false, x: lx, y: ly }, SMessage::BrowserClick(lx, ly)],
        ["click", x, y] => {
            let (x, y) = (px(x)?, px(y)?);
            vec![
                SMessage::BrowserMouseMove(x, y),
                SMessage::BrowserButton { down: true, x, y },
                SMessage::BrowserButton { down: false, x, y },
                SMessage::BrowserClick(x, y),
            ]
        }
        ["wheel", n] => {
            let n = px(n)?;
            vec![SMessage::BrowserScroll(0.0, -n)]
        }
        [v] if tools::is_verb(v) => vec![SMessage::Input { target: TARGET.into(), text: v.to_string() }],
        _ => return None,
    })
}

/// The script (whitespace-separated tokens) through `Editor::on`; the tokens it could not read.
pub fn drive(ed: &mut Editor, script: &str) -> Vec<String> {
    let mut bad = Vec::new();
    for tok in script.split_whitespace() {
        if let Some(i) = tok.strip_prefix("active:").and_then(|s| s.parse().ok()) {
            ed.active = i;
            continue;
        }
        match messages(ed, tok) {
            Some(ms) => {
                for m in ms {
                    ed.on(&m);
                }
            }
            None => bad.push(tok.to_string()),
        }
    }
    bad
}

// ---- the CLI ---------------------------------------------------------------------------------------------------------

const USAGE: &str = "usage: facet-view --edit <file.unarastr> | --synth new WxH  [--size WxH] [--do \"TOKENS\"]  (TOKENS: press:X:Y move:X:Y release click:X:Y wheel:N key:NAME set:K=V active:I <button verb>)";

fn wxh(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x')?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// The pixel order this build's window surface reads.
fn surface_order() -> PixelOrder {
    if cfg!(all(target_os = "linux", feature = "gtk")) { PixelOrder::Bgra } else { PixelOrder::Rgba }
}

/// `facet-view --edit ... | --synth ...`: the window, or (with `--do`, or no window backend) the headless witness.
/// Returns the exit code.
pub fn main(args: &[String]) -> i32 {
    let (mut new, mut path, mut size, mut script) = (None, None, (800u32, 600u32), None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--edit" | "--synth" => match it.next().map(String::as_str) {
                Some("new") => new = it.next().and_then(|s| wxh(s)),
                Some(p) => path = Some(p.to_string()),
                None => {}
            },
            "--size" => size = it.next().and_then(|s| wxh(s)).unwrap_or(size),
            "--do" => script = it.next().cloned(),
            _ => {
                eprintln!("{USAGE}");
                return 2;
            }
        }
    }
    let windowed = cfg!(any(target_os = "macos", all(target_os = "linux", feature = "gtk")));
    let headless = script.is_some() || !windowed;
    let order = if headless { PixelOrder::Rgba } else { surface_order() };
    let made = match (new, &path) {
        (Some((w, h)), _) => Editor::new(w, h, size, order),
        (None, Some(p)) => Editor::open(p, size, order),
        _ => {
            eprintln!("{USAGE}");
            return 2;
        }
    };
    let mut ed = match made {
        Ok(ed) => ed,
        Err(token) => {
            println!("{token}");
            return 1;
        }
    };
    if headless {
        let bad = drive(&mut ed, script.as_deref().unwrap_or(""));
        for t in ed.tokens.drain(..) {
            println!("{t}");
        }
        println!("{}", ed.stat());
        if !bad.is_empty() {
            eprintln!("facet-view: bad token(s): {}", bad.join(" "));
            return 2;
        }
        return 0;
    }

    let title = path.as_deref().and_then(|p| std::path::Path::new(p).file_name()).and_then(|s| s.to_str()).unwrap_or("untitled").to_string();
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().build().expect("tokio runtime");
    let synapse = Synapse::new();
    {
        let rx = synapse.subscribe();
        rt.spawn(serve(synapse.clone(), rx, ed));
    }
    quartzite::Backend::new_tetra_vessel(
        "org.unaos.facet.editor",
        &format!("Facet — {title}"),
        (size.0 as f64, size.1 as f64 + 120.0),
        tree(),
        synapse.clone(),
    )
    .run();
    drop(rt);
    0
}
