//! Glyph outlines as paths of quadratic (TrueType) and cubic (CFF) Bézier segments, in font units, y up.

use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PathCmd {
    MoveTo(f32, f32),
    LineTo(f32, f32),
    QuadTo(f32, f32, f32, f32),
    CubicTo(f32, f32, f32, f32, f32, f32),
    Close,
}

/// Receives an outline. Every contour starts with `move_to` and ends with `close`.
pub trait OutlineSink {
    fn move_to(&mut self, x: f32, y: f32);
    fn line_to(&mut self, x: f32, y: f32);
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32);
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32);
    fn close(&mut self);
}

/// A recorded outline plus its control-box.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Path {
    pub cmds: Vec<PathCmd>,
    pub x_min: f32,
    pub y_min: f32,
    pub x_max: f32,
    pub y_max: f32,
}

impl Path {
    pub fn new() -> Self {
        Path { cmds: Vec::new(), x_min: f32::MAX, y_min: f32::MAX, x_max: f32::MIN, y_max: f32::MIN }
    }
    pub fn is_empty(&self) -> bool {
        self.cmds.is_empty()
    }
    fn grow(&mut self, x: f32, y: f32) {
        self.x_min = self.x_min.min(x);
        self.y_min = self.y_min.min(y);
        self.x_max = self.x_max.max(x);
        self.y_max = self.y_max.max(y);
    }
    /// Number of contours (MoveTo count).
    pub fn contours(&self) -> usize {
        self.cmds.iter().filter(|c| matches!(c, PathCmd::MoveTo(..))).count()
    }
    /// Replay into another sink.
    pub fn replay(&self, s: &mut impl OutlineSink) {
        for c in &self.cmds {
            match *c {
                PathCmd::MoveTo(x, y) => s.move_to(x, y),
                PathCmd::LineTo(x, y) => s.line_to(x, y),
                PathCmd::QuadTo(a, b, x, y) => s.quad_to(a, b, x, y),
                PathCmd::CubicTo(a, b, c2, d, x, y) => s.curve_to(a, b, c2, d, x, y),
                PathCmd::Close => s.close(),
            }
        }
    }
}

impl OutlineSink for Path {
    fn move_to(&mut self, x: f32, y: f32) {
        self.grow(x, y);
        self.cmds.push(PathCmd::MoveTo(x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.grow(x, y);
        self.cmds.push(PathCmd::LineTo(x, y));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        self.grow(x1, y1);
        self.grow(x, y);
        self.cmds.push(PathCmd::QuadTo(x1, y1, x, y));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.grow(x1, y1);
        self.grow(x2, y2);
        self.grow(x, y);
        self.cmds.push(PathCmd::CubicTo(x1, y1, x2, y2, x, y));
    }
    fn close(&mut self) {
        self.cmds.push(PathCmd::Close);
    }
}
