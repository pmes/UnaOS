// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The document model: [`Document`], [`Layer`], [`BlendMode`], [`Mask`] and [`Document::validate`].

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::Error;
use crate::selection::Selection;

/// How a layer meets what lies under it. The wire byte is the variant's position (0..=7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendMode {
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    Difference,
    /// The eraser as a blend mode: destination-out (the layer's alpha removes the alpha under it).
    Erase,
}

impl BlendMode {
    /// Every mode, in wire order.
    pub const ALL: [BlendMode; 8] = [
        BlendMode::Normal,
        BlendMode::Multiply,
        BlendMode::Screen,
        BlendMode::Overlay,
        BlendMode::Darken,
        BlendMode::Lighten,
        BlendMode::Difference,
        BlendMode::Erase,
    ];

    /// The container's byte for this mode.
    pub fn to_byte(self) -> u8 {
        BlendMode::ALL.iter().position(|m| *m == self).unwrap_or(0) as u8
    }

    /// The mode a container byte names, or `None`.
    pub fn from_byte(b: u8) -> Option<BlendMode> {
        BlendMode::ALL.get(b as usize).copied()
    }
}

/// A coverage plane: one byte per pixel, 0 = out / hidden, 255 = in / shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mask {
    pub width: u32,
    pub height: u32,
    pub coverage: Vec<u8>,
}

impl Mask {
    /// A mask of `width x height` filled with `value`.
    pub fn filled(width: u32, height: u32, value: u8) -> Mask {
        Mask { width, height, coverage: vec![value; width as usize * height as usize] }
    }

    /// Coverage at `(x, y)`, 0 outside.
    pub fn at(&self, x: i64, y: i64) -> u8 {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return 0;
        }
        self.coverage.get(y as usize * self.width as usize + x as usize).copied().unwrap_or(0)
    }

    /// How many pixels have any coverage.
    pub fn count(&self) -> usize {
        self.coverage.iter().filter(|c| **c != 0).count()
    }
}

/// One layer: straight (non-premultiplied) RGBA8, row-major, `width * height * 4` bytes, placed at `offset`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layer {
    pub name: String,
    pub pixels: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub offset: (i32, i32),
    pub opacity: u8,
    pub blend: BlendMode,
    pub visible: bool,
    pub mask: Option<Mask>,
}

impl Layer {
    /// A layer of `width x height` filled with `rgba`, at the origin, opaque, Normal, visible, unmasked.
    pub fn filled(name: &str, width: u32, height: u32, rgba: [u8; 4]) -> Layer {
        let n = width as usize * height as usize;
        let mut pixels = Vec::with_capacity(n * 4);
        for _ in 0..n {
            pixels.extend_from_slice(&rgba);
        }
        Layer {
            name: String::from(name),
            pixels,
            width,
            height,
            offset: (0, 0),
            opacity: 255,
            blend: BlendMode::Normal,
            visible: true,
            mask: None,
        }
    }

    /// A transparent layer.
    pub fn new(name: &str, width: u32, height: u32) -> Layer {
        Layer::filled(name, width, height, [0, 0, 0, 0])
    }

    /// The pixel at layer coordinates `(x, y)`, or `None` outside the layer.
    pub fn pixel(&self, x: i64, y: i64) -> Option<[u8; 4]> {
        if x < 0 || y < 0 || x >= self.width as i64 || y >= self.height as i64 {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        let p = self.pixels.get(i..i + 4)?;
        Some([p[0], p[1], p[2], p[3]])
    }

    /// The layer's mask coverage at layer coordinates `(x, y)` (255 when unmasked).
    pub fn mask_at(&self, x: i64, y: i64) -> u8 {
        match &self.mask {
            Some(m) => m.at(x, y),
            None => 255,
        }
    }
}

/// A raster document: a canvas, its layers (bottom first) and the current selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Document {
    pub width: u32,
    pub height: u32,
    pub layers: Vec<Layer>,
    pub selection: Option<Selection>,
}

/// pixel_core's limits are the document's: a canvas or layer pixel_core could not round-trip is refused.
fn check_dims(width: u32, height: u32) -> Result<(), Error> {
    if width == 0 || height == 0 {
        return Err(Error::Empty);
    }
    if width > pixel_core::MAX_DIM
        || height > pixel_core::MAX_DIM
        || (width as u64) * (height as u64) > pixel_core::MAX_PIXELS
    {
        return Err(Error::TooLarge);
    }
    Ok(())
}

impl Document {
    /// An empty document (no layers, no selection).
    pub fn new(width: u32, height: u32) -> Result<Document, Error> {
        check_dims(width, height)?;
        Ok(Document { width, height, layers: Vec::new(), selection: None })
    }

    /// The layer at `index`, or [`Error::NoLayer`].
    pub fn layer(&self, index: usize) -> Result<&Layer, Error> {
        self.layers.get(index).ok_or(Error::NoLayer)
    }

    /// The invariants every document returned by this crate holds: the canvas and every layer are within
    /// pixel_core's limits and at least 1x1; a layer's pixel length is `w * h * 4`; a mask has its layer's size and
    /// `w * h` coverage bytes; the selection lies inside the canvas. Opacity is a `u8`, so 0..=255 by type.
    pub fn validate(&self) -> Result<(), Error> {
        check_dims(self.width, self.height)?;
        for l in &self.layers {
            check_dims(l.width, l.height)?;
            if l.pixels.len() != l.width as usize * l.height as usize * 4 {
                return Err(Error::Dimensions);
            }
            if let Some(m) = &l.mask {
                if m.width != l.width || m.height != l.height || m.coverage.len() != m.width as usize * m.height as usize
                {
                    return Err(Error::Mask);
                }
            }
        }
        if let Some(s) = &self.selection {
            if !s.inside(self.width, self.height) {
                return Err(Error::Selection);
            }
        }
        Ok(())
    }
}
