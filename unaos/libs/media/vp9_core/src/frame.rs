// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Frame buffers: three u16 planes (8-bit streams use the low 8 bits) allocated to the 64x64
//! superblock grid so every block the decoder writes lands inside the allocation, plus the per-8x8
//! motion vectors the next frame's MV prediction reads (§6.4.20 PrevMvs / PrevRefFrames).

use alloc::vec;
use alloc::vec::Vec;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mv {
    pub row: i16,
    pub col: i16,
}

impl Mv {
    pub const ZERO: Mv = Mv { row: 0, col: 0 };
    pub fn new(row: i32, col: i32) -> Mv {
        Mv { row: row as i16, col: col as i16 }
    }
}

/// The motion of one 8x8 unit, kept for the next frame's candidate list.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MvRef {
    pub ref_frame: [i8; 2],
    pub mv: [Mv; 2],
}

#[derive(Clone, Debug)]
pub struct Plane {
    pub data: Vec<u16>,
    pub stride: usize,
    /// Allocated rows.
    pub rows: usize,
}

impl Plane {
    fn new(w: usize, h: usize) -> Plane {
        Plane { data: vec![0; w * h], stride: w, rows: h }
    }
}

#[derive(Clone, Debug)]
pub struct FrameBuf {
    /// The frame (crop) size.
    pub width: u32,
    pub height: u32,
    pub render_width: u32,
    pub render_height: u32,
    pub ss_x: u8,
    pub ss_y: u8,
    pub bit_depth: u8,
    pub color_space: u8,
    pub color_range: u8,
    pub mi_cols: usize,
    pub mi_rows: usize,
    pub planes: [Plane; 3],
    pub mvs: Vec<MvRef>,
}

impl FrameBuf {
    pub fn new(width: u32, height: u32, ss_x: u8, ss_y: u8, bit_depth: u8) -> FrameBuf {
        let mi_cols = width.div_ceil(8) as usize;
        let mi_rows = height.div_ceil(8) as usize;
        let aw = ((mi_cols + 7) & !7) * 8;
        let ah = ((mi_rows + 7) & !7) * 8;
        let (cw, ch) = (aw >> ss_x, ah >> ss_y);
        FrameBuf {
            width,
            height,
            render_width: width,
            render_height: height,
            ss_x,
            ss_y,
            bit_depth,
            color_space: 0,
            color_range: 0,
            mi_cols,
            mi_rows,
            planes: [Plane::new(aw, ah), Plane::new(cw, ch), Plane::new(cw, ch)],
            mvs: vec![MvRef::default(); mi_cols * mi_rows],
        }
    }

    /// The crop size of plane `p`.
    pub fn plane_size(&self, p: usize) -> (usize, usize) {
        if p == 0 {
            (self.width as usize, self.height as usize)
        } else {
            ((self.width as usize + self.ss_x as usize) >> self.ss_x, (self.height as usize + self.ss_y as usize) >> self.ss_y)
        }
    }

    /// The 8-aligned decoded size of plane `p` (the reference decoder's y_width / uv_width, which
    /// bounds intra edge availability).
    pub fn aligned_size(&self, p: usize) -> (usize, usize) {
        let aw = self.mi_cols * 8;
        let ah = self.mi_rows * 8;
        if p == 0 { (aw, ah) } else { (aw >> self.ss_x, ah >> self.ss_y) }
    }
}
