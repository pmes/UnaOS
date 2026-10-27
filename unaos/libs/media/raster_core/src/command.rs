// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Every edit is a [`Command`], applied by [`Document::apply`] — pure: `&self -> Result<Document, Error>`, with
//! [`Document::validate`] at the end (timeline_core's op shape). [`History`] is the undo / redo stack.
//!
//! The stack holds whole documents: the simplest correct shape. Its cost is a full copy of every layer per step;
//! a later cut turns it into inverse commands (or shared tiles) behind the same `push` / `undo` / `redo`.

use alloc::string::String;
use alloc::vec::Vec;

use crate::Error;
use crate::composite::{mul255, over};
use crate::document::{BlendMode, Document, Layer, Mask};
use crate::selection::Selection;

/// One edit of a [`Document`]. Layer indices count from the bottom (0).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Insert `layer` at `index` (`index == layers.len()` puts it on top).
    AddLayer { index: usize, layer: Layer },
    RemoveLayer { index: usize },
    /// Take the layer at `from` out and insert it at `to` (both indices into the current stack).
    MoveLayer { from: usize, to: usize },
    SetOpacity { layer: usize, opacity: u8 },
    SetBlend { layer: usize, blend: BlendMode },
    SetVisible { layer: usize, visible: bool },
    SetOffset { layer: usize, offset: (i32, i32) },
    Rename { layer: usize, name: String },
    /// Fill the selected part of the layer (no selection: all of it) with `rgba`.
    FillSelection { layer: usize, rgba: [u8; 4] },
    /// Clear the selected part of the layer (no selection: all of it) to transparent.
    ClearSelection { layer: usize },
    SetMask { layer: usize, mask: Option<Mask> },
    SetSelection(Option<Selection>),
    /// Write `w x h` RGBA pixels into the layer at layer coordinates `(x, y)`.
    PastePixels { layer: usize, x: i32, y: i32, w: u32, h: u32, rgba: Vec<u8> },
}

/// The selection coverage at each layer pixel (canvas space less the layer's offset); `None` = all of the layer.
fn selected(doc: &Document, l: &Layer) -> Option<Mask> {
    let canvas = doc.selection.as_ref()?.rasterize(doc.width, doc.height);
    let (ox, oy) = (l.offset.0 as i64, l.offset.1 as i64);
    let mut m = Mask::filled(l.width, l.height, 0);
    for ly in 0..l.height as i64 {
        for lx in 0..l.width as i64 {
            m.coverage[(ly * l.width as i64 + lx) as usize] = canvas.at(lx + ox, ly + oy);
        }
    }
    Some(m)
}

impl Document {
    fn layer_mut(&mut self, index: usize) -> Result<&mut Layer, Error> {
        self.layers.get_mut(index).ok_or(Error::NoLayer)
    }

    /// Apply `cmd` to a copy of the document and return the copy, validated. `self` is never changed.
    pub fn apply(&self, cmd: &Command) -> Result<Document, Error> {
        let mut d = self.clone();
        match cmd {
            Command::AddLayer { index, layer } => {
                if *index > d.layers.len() {
                    return Err(Error::NoLayer);
                }
                d.layers.insert(*index, layer.clone());
            }
            Command::RemoveLayer { index } => {
                if *index >= d.layers.len() {
                    return Err(Error::NoLayer);
                }
                d.layers.remove(*index);
            }
            Command::MoveLayer { from, to } => {
                if *from >= d.layers.len() || *to >= d.layers.len() {
                    return Err(Error::NoLayer);
                }
                let l = d.layers.remove(*from);
                d.layers.insert(*to, l);
            }
            Command::SetOpacity { layer, opacity } => d.layer_mut(*layer)?.opacity = *opacity,
            Command::SetBlend { layer, blend } => d.layer_mut(*layer)?.blend = *blend,
            Command::SetVisible { layer, visible } => d.layer_mut(*layer)?.visible = *visible,
            Command::SetOffset { layer, offset } => d.layer_mut(*layer)?.offset = *offset,
            Command::Rename { layer, name } => d.layer_mut(*layer)?.name = name.clone(),
            Command::FillSelection { layer, rgba } => {
                let sel = selected(self, self.layer(*layer)?);
                let l = d.layer_mut(*layer)?;
                for (i, px) in l.pixels.chunks_exact_mut(4).enumerate() {
                    let c = sel.as_ref().map_or(255, |m| m.coverage[i]);
                    if c == 0 {
                        continue;
                    }
                    let out = if c == 255 {
                        *rgba
                    } else {
                        let src = [rgba[0], rgba[1], rgba[2], mul255(rgba[3] as u32, c as u32) as u8];
                        over(src, [px[0], px[1], px[2], px[3]])
                    };
                    px.copy_from_slice(&out);
                }
            }
            Command::ClearSelection { layer } => {
                let sel = selected(self, self.layer(*layer)?);
                let l = d.layer_mut(*layer)?;
                for (i, px) in l.pixels.chunks_exact_mut(4).enumerate() {
                    let c = sel.as_ref().map_or(255, |m| m.coverage[i]);
                    if c == 255 {
                        px.copy_from_slice(&[0, 0, 0, 0]);
                    } else if c != 0 {
                        px[3] = mul255(px[3] as u32, 255 - c as u32) as u8;
                    }
                }
            }
            Command::SetMask { layer, mask } => d.layer_mut(*layer)?.mask = mask.clone(),
            Command::SetSelection(s) => d.selection = s.clone(),
            Command::PastePixels { layer, x, y, w, h, rgba } => {
                let l = d.layer_mut(*layer)?;
                if rgba.len() != *w as usize * *h as usize * 4 {
                    return Err(Error::Dimensions);
                }
                let (x, y, w, h) = (*x as i64, *y as i64, *w as i64, *h as i64);
                if x < 0 || y < 0 || x + w > l.width as i64 || y + h > l.height as i64 {
                    return Err(Error::Bounds);
                }
                let lw = l.width as i64;
                for row in 0..h {
                    let s = (row * w * 4) as usize;
                    let t = (((y + row) * lw + x) * 4) as usize;
                    l.pixels[t..t + (w * 4) as usize].copy_from_slice(&rgba[s..s + (w * 4) as usize]);
                }
            }
        }
        d.validate()?;
        Ok(d)
    }
}

/// The undo / redo stack: the documents before each step (`past`) and the ones undone (`future`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    pub past: Vec<Document>,
    pub future: Vec<Document>,
}

impl History {
    pub fn new() -> History {
        History::default()
    }

    /// Record `prev` (the document before an edit); a new edit forgets what was undone.
    pub fn push(&mut self, prev: Document) {
        self.past.push(prev);
        self.future.clear();
    }

    /// Apply `cmd` to `current` and record `current`; the edited document is returned.
    pub fn commit(&mut self, current: &Document, cmd: &Command) -> Result<Document, Error> {
        let next = current.apply(cmd)?;
        self.push(current.clone());
        Ok(next)
    }

    /// Step back: `current` goes to `future`, the last recorded document comes back (`None`: nothing to undo).
    pub fn undo(&mut self, current: Document) -> Option<Document> {
        let prev = self.past.pop()?;
        self.future.push(current);
        Some(prev)
    }

    /// Step forward again: `current` goes to `past`, the last undone document comes back (`None`: nothing to redo).
    pub fn redo(&mut self, current: Document) -> Option<Document> {
        let next = self.future.pop()?;
        self.past.push(current);
        Some(next)
    }
}
