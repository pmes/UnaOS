// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The non-destructive edit list. The decoded original is never touched: an export (or a render)
//! bakes the list over it, in order. Undo/redo/reset move whole states, so every one of them is
//! itself undoable.

use crate::raster::Raster;
use crate::source::{MAX_DIM, MAX_PIXELS};

/// One edit, in the coordinates of the picture as it stands after the previous ops.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Crop { x: u32, y: u32, width: u32, height: u32 },
    /// Clockwise quarter turns.
    Rotate { quarter_turns: u8 },
    Flip { horizontal: bool },
    /// The documented triangle filter ([`Raster::resized`]).
    Resize { width: u32, height: u32 },
    /// CSS `brightness(b) contrast(c)` ([`Raster::adjusted`]).
    Adjust { brightness: f32, contrast: f32 },
}

/// The largest brightness/contrast factor accepted (CSS allows any non-negative value; past 16 the
/// result is already saturated for every 8-bit input but 0).
pub const MAX_FACTOR: f32 = 16.0;

impl Op {
    /// The size the op produces from a `w x h` picture, or why it is refused.
    pub fn check(&self, w: u32, h: u32) -> Result<(u32, u32), String> {
        match *self {
            Op::Crop { x, y, width, height } => {
                if width == 0 || height == 0 {
                    return Err("crop: zero-sized rectangle".into());
                }
                let fits = x.checked_add(width).is_some_and(|r| r <= w) && y.checked_add(height).is_some_and(|b| b <= h);
                if !fits {
                    return Err(format!("crop: {width}x{height}+{x}+{y} leaves the {w}x{h} picture"));
                }
                Ok((width, height))
            }
            Op::Rotate { quarter_turns } => Ok(if quarter_turns % 2 == 1 { (h, w) } else { (w, h) }),
            Op::Flip { .. } => Ok((w, h)),
            Op::Resize { width, height } => {
                if width == 0 || height == 0 {
                    return Err("resize: zero dimension".into());
                }
                if width > MAX_DIM || height > MAX_DIM || width as u64 * height as u64 > MAX_PIXELS {
                    return Err(format!("resize: {width}x{height} exceeds the size limit"));
                }
                Ok((width, height))
            }
            Op::Adjust { brightness, contrast } => {
                let ok = |v: f32| v.is_finite() && (0.0..=MAX_FACTOR).contains(&v);
                if !ok(brightness) || !ok(contrast) {
                    return Err(format!("adjust: factors must be in 0..={MAX_FACTOR}"));
                }
                Ok((w, h))
            }
        }
    }

    /// Apply the op (already [`Op::check`]ed against `img`).
    pub fn apply(&self, img: &Raster) -> Raster {
        match *self {
            Op::Crop { x, y, width, height } => img.cropped(x, y, width, height),
            Op::Rotate { quarter_turns } => img.rotated(quarter_turns),
            Op::Flip { horizontal } => img.flipped(horizontal),
            Op::Resize { width, height } => img.resized(width, height),
            Op::Adjust { brightness, contrast } => img.adjusted(brightness, contrast),
        }
    }
}

/// The op list with whole-state undo history.
#[derive(Clone, Debug, Default)]
pub struct EditList {
    current: Vec<Op>,
    past: Vec<Vec<Op>>,
    future: Vec<Vec<Op>>,
}

impl EditList {
    pub fn ops(&self) -> &[Op] {
        &self.current
    }

    /// The size after every op, starting from `w x h`.
    pub fn size_from(&self, w: u32, h: u32) -> (u32, u32) {
        self.current.iter().fold((w, h), |(w, h), op| op.check(w, h).unwrap_or((w, h)))
    }

    /// Append `op` if it is legal on a `w x h` picture (the size after the current list).
    pub fn push(&mut self, op: Op, w: u32, h: u32) -> Result<(), String> {
        let (cw, ch) = self.size_from(w, h);
        op.check(cw, ch)?;
        self.past.push(self.current.clone());
        self.current.push(op);
        self.future.clear();
        Ok(())
    }

    pub fn undo(&mut self) -> bool {
        match self.past.pop() {
            Some(p) => {
                self.future.push(std::mem::replace(&mut self.current, p));
                true
            }
            None => false,
        }
    }

    pub fn redo(&mut self) -> bool {
        match self.future.pop() {
            Some(f) => {
                self.past.push(std::mem::replace(&mut self.current, f));
                true
            }
            None => false,
        }
    }

    /// Drop every op (undoable). A reset of an empty list is a no-op.
    pub fn reset(&mut self) -> bool {
        if self.current.is_empty() {
            return false;
        }
        self.past.push(std::mem::take(&mut self.current));
        self.future.clear();
        true
    }

    /// Bake the list over `original`.
    pub fn bake(&self, original: &Raster) -> Raster {
        let mut img = original.clone();
        for op in &self.current {
            img = op.apply(&img);
        }
        img
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_redo_reset_are_whole_states() {
        let mut e = EditList::default();
        e.push(Op::Rotate { quarter_turns: 1 }, 4, 2).unwrap();
        assert_eq!(e.size_from(4, 2), (2, 4));
        // The crop is checked against the ROTATED size.
        assert!(e.push(Op::Crop { x: 0, y: 0, width: 3, height: 1 }, 4, 2).is_err());
        e.push(Op::Crop { x: 0, y: 1, width: 2, height: 3 }, 4, 2).unwrap();
        assert_eq!(e.size_from(4, 2), (2, 3));
        assert!(e.undo());
        assert_eq!(e.ops().len(), 1);
        assert!(e.redo());
        assert_eq!(e.ops().len(), 2);
        assert!(!e.redo());
        assert!(e.reset());
        assert!(e.ops().is_empty());
        assert!(e.undo(), "reset is undoable");
        assert_eq!(e.ops().len(), 2);
        // A new op after an undo drops the redo branch.
        e.undo();
        e.push(Op::Flip { horizontal: true }, 4, 2).unwrap();
        assert!(!e.redo());
    }

    #[test]
    fn checks_refuse_bad_ops() {
        assert!(Op::Crop { x: 3, y: 0, width: 2, height: 1 }.check(4, 4).is_err());
        assert!(Op::Crop { x: u32::MAX, y: 0, width: 2, height: 1 }.check(4, 4).is_err());
        assert!(Op::Resize { width: 0, height: 1 }.check(4, 4).is_err());
        assert!(Op::Resize { width: 1 << 17, height: 1 }.check(4, 4).is_err());
        assert!(Op::Adjust { brightness: f32::NAN, contrast: 1.0 }.check(4, 4).is_err());
        assert!(Op::Adjust { brightness: -1.0, contrast: 1.0 }.check(4, 4).is_err());
        assert_eq!(Op::Rotate { quarter_turns: 3 }.check(4, 2), Ok((2, 4)));
    }
}
