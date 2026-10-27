// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The verb table (RASTERCLI, LEDGER SR233): each [`Op`] is one raster_core `Command` through `History::commit`, or
//! one core call (brush_core / adjust_core on a copy of a layer's pixels) followed by `Command::PastePixels`, so
//! `undo` reaches every mutation. No editing logic lives here.

use raster_core::{Command, Document, History, Layer, Selection};

use crate::script::{Adjust, BrushArgs, Op};

/// The one document and its history.
#[derive(Default)]
pub struct State {
    pub doc: Option<Document>,
    pub hist: History,
}

/// A failed verb's word: a core's `Error::word()` or the tool's own (`io`, `no-document`, `no-undo`, `no-redo`,
/// `png-encode`).
pub type Word = &'static str;

impl State {
    fn doc(&self) -> Result<&Document, Word> {
        self.doc.as_ref().ok_or("no-document")
    }

    /// `History::commit` on the current document.
    fn commit(&mut self, cmd: Command) -> Result<(), Word> {
        let cur = self.doc.as_ref().ok_or("no-document")?;
        let next = self.hist.commit(cur, &cmd).map_err(|e| e.word())?;
        self.doc = Some(next);
        Ok(())
    }

    /// A core call over a copy of layer `i`'s pixels, landed by `PastePixels` over the whole layer.
    fn paste(&mut self, i: usize, f: impl FnOnce(&Layer) -> Result<Vec<u8>, Word>) -> Result<(), Word> {
        let l = self.doc()?.layer(i).map_err(|e| e.word())?;
        let (w, h) = (l.width, l.height);
        let rgba = f(l)?;
        self.commit(Command::PastePixels { layer: i, x: 0, y: 0, w, h, rgba })
    }

    pub fn run(&mut self, op: &Op) -> Result<(), Word> {
        match op {
            Op::New { w, h } => {
                let d = Document::new(*w, *h).map_err(|e| e.word())?;
                let d = d
                    .apply(&Command::AddLayer { index: 0, layer: Layer::new("Background", *w, *h) })
                    .map_err(|e| e.word())?;
                self.doc = Some(d);
                self.hist = History::new();
            }
            Op::Open(p) => {
                let bytes = std::fs::read(p).map_err(|_| "io")?;
                self.doc = Some(raster_core::decode(&bytes).map_err(|e| e.word())?);
                self.hist = History::new();
            }
            Op::Save(p) => {
                let bytes = raster_core::encode(self.doc()?).map_err(|e| e.word())?;
                std::fs::write(p, bytes).map_err(|_| "io")?;
            }
            Op::Export(p) => {
                let d = self.doc()?;
                let png = pixel_core::png::encode::encode_rgba(d.width, d.height, &d.composite())
                    .map_err(|_| "png-encode")?;
                std::fs::write(p, png).map_err(|_| "io")?;
            }
            Op::LayerAdd(name) => {
                let d = self.doc()?;
                let (index, layer) = (d.layers.len(), Layer::new(name, d.width, d.height));
                self.commit(Command::AddLayer { index, layer })?;
            }
            Op::LayerRemove(i) => self.commit(Command::RemoveLayer { index: *i })?,
            Op::LayerMove(from, to) => self.commit(Command::MoveLayer { from: *from, to: *to })?,
            Op::LayerOpacity(i, o) => self.commit(Command::SetOpacity { layer: *i, opacity: *o })?,
            Op::LayerBlend(i, b) => self.commit(Command::SetBlend { layer: *i, blend: *b })?,
            Op::LayerVisible(i, v) => self.commit(Command::SetVisible { layer: *i, visible: *v })?,
            Op::LayerOffset(i, x, y) => self.commit(Command::SetOffset { layer: *i, offset: (*x, *y) })?,
            Op::LayerRename(i, n) => self.commit(Command::Rename { layer: *i, name: n.clone() })?,
            Op::SelectRect(x, y, w, h) => {
                self.commit(Command::SetSelection(Some(Selection::Rect { x: *x, y: *y, w: *w, h: *h })))?
            }
            Op::SelectEllipse(cx, cy, rx, ry) => {
                self.commit(Command::SetSelection(Some(Selection::Ellipse { cx: *cx, cy: *cy, rx: *rx, ry: *ry })))?
            }
            Op::SelectPoly(pts) => self.commit(Command::SetSelection(Some(Selection::Polygon(pts.clone()))))?,
            Op::SelectWand { layer, x, y, tol } => {
                let s = Selection::magic_wand(self.doc()?, *layer, *x, *y, *tol).map_err(|e| e.word())?;
                self.commit(Command::SetSelection(Some(s)))?
            }
            Op::SelectNone => self.commit(Command::SetSelection(None))?,
            Op::Fill(i, rgba) => self.commit(Command::FillSelection { layer: *i, rgba: *rgba })?,
            Op::Clear(i) => self.commit(Command::ClearSelection { layer: *i })?,
            Op::Brush(b) => self.paste(b.layer, |l| Ok(brush(l, b)))?,
            Op::Adjust(i, a) => self.paste(*i, |l| adjust(l, a).map_err(|e| e.word()))?,
            Op::Undo => {
                self.doc()?;
                if self.hist.past.is_empty() {
                    return Err("no-undo");
                }
                let cur = self.doc.take().ok_or("no-document")?;
                self.doc = self.hist.undo(cur);
            }
            Op::Redo => {
                self.doc()?;
                if self.hist.future.is_empty() {
                    return Err("no-redo");
                }
                let cur = self.doc.take().ok_or("no-document")?;
                self.doc = self.hist.redo(cur);
            }
            Op::Stat => {}
        }
        Ok(())
    }
}

/// brush_core's `paint` on a copy of the layer's pixels (points in layer coordinates).
fn brush(l: &Layer, b: &BrushArgs) -> Vec<u8> {
    let mut br = brush_core::Brush::new(b.diameter);
    br.hardness = b.hardness;
    br.spacing = b.spacing;
    br.flow = b.flow;
    br.mode = b.mode;
    let stroke = brush_core::Stroke {
        points: b.points.iter().map(|&(x, y, pressure)| brush_core::StrokePoint { x, y, pressure }).collect(),
    };
    let mut px = l.pixels.clone();
    brush_core::paint(&mut px, l.width, l.height, &br, &stroke, b.rgba);
    px
}

/// One adjust_core call over the layer's pixels.
fn adjust(l: &Layer, a: &Adjust) -> Result<Vec<u8>, adjust_core::Error> {
    let (p, w, h) = (&l.pixels[..], l.width, l.height);
    match a {
        Adjust::Levels(v) => adjust_core::levels(p, w, h, *v),
        Adjust::Curves(pts) => {
            let c = adjust_core::Curves { master: adjust_core::Curves::from_points(pts), r: None, g: None, b: None };
            adjust_core::curves(p, w, h, &c)
        }
        Adjust::HueSat(v) => adjust_core::hue_sat(p, w, h, *v),
        Adjust::Invert => adjust_core::invert(p, w, h),
        Adjust::Threshold(v) => adjust_core::threshold(p, w, h, *v),
        Adjust::BoxBlur(r) => adjust_core::box_blur(p, w, h, *r),
        Adjust::Gaussian(s) => adjust_core::gaussian_blur(p, w, h, *s),
        Adjust::Unsharp(v) => adjust_core::unsharp(p, w, h, *v),
    }
}
