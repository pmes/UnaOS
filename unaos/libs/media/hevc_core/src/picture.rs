// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The picture under decoding: three 8-bit planes (4:2:0) and the facts the parser, intra prediction, the deblocking
//! filter and SAO read back — per 4x4 luma block (coding quadtree depth, IntraPredModeY, QpY, pcm / transquant bypass,
//! transform / coding block edges) and per CTB (its slice, its SAO parameters) — with the z-scan availability
//! process of 6.4.1, and the conformance-window crop into h264_core's [`Frame`].

use alloc::vec;
use alloc::vec::Vec;

use crate::ps::Geometry;
use crate::Frame;

/// One sample plane, row-major, no padding.
#[derive(Clone, Debug, Default)]
pub struct Plane {
    pub w: usize,
    pub h: usize,
    pub data: Vec<u8>,
}

impl Plane {
    pub fn new(w: usize, h: usize, fill: u8) -> Plane {
        Plane { w, h, data: vec![fill; w * h] }
    }
    #[inline]
    pub fn at(&self, x: usize, y: usize) -> u8 {
        self.data[y * self.w + x]
    }
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, v: u8) {
        self.data[y * self.w + x] = v;
    }
}

/// What the loop filters need of a slice (the one whose header covers a CTB).
#[derive(Clone, Copy, Debug, Default)]
pub struct SliceInfo {
    /// SliceAddrRs: the address of the slice's first CTB (its independent segment).
    pub addr_rs: u32,
    pub deblocking_disabled: bool,
    pub beta_offset_div2: i32,
    pub tc_offset_div2: i32,
    pub loop_filter_across_slices: bool,
    pub sao_luma: bool,
    pub sao_chroma: bool,
}

/// SAO parameters of one CTB (7.4.9.3): per component SaoTypeIdx (0 off, 1 band, 2 edge), SaoOffsetVal[0..5],
/// sao_band_position and SaoEoClass.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SaoParams {
    pub type_idx: [u8; 3],
    pub offset_val: [[i16; 5]; 3],
    pub band_position: [u8; 3],
    pub eo_class: [u8; 3],
}

pub const F_PCM: u8 = 1;
pub const F_BYPASS: u8 = 2;
/// A transform or coding block edge lies on the left / top of this 4x4 block.
pub const F_EDGE_V: u8 = 4;
pub const F_EDGE_H: u8 = 8;
/// The block has been parsed (its CU's facts are filled).
pub const NO_SLICE: u32 = u32::MAX;

pub struct Picture {
    pub width: usize,
    pub height: usize,
    pub planes: [Plane; 3],
    pub w4: usize,
    pub h4: usize,
    pub ctdepth: Vec<u8>,
    pub intra_mode: Vec<u8>,
    pub qp: Vec<i8>,
    pub flags: Vec<u8>,
    /// Index into `slices` per raster CTB address; NO_SLICE until the CTB is decoded.
    pub ctb_slice: Vec<u32>,
    pub sao: Vec<SaoParams>,
    pub slices: Vec<SliceInfo>,
    pub poc: i32,
    pub tag: i64,
    pub full_range: bool,
    /// The conformance window in luma samples: (x0, y0, w, h).
    pub crop: (usize, usize, usize, usize),
    pub output: bool,
}

impl Picture {
    pub fn new(width: usize, height: usize, geo: &Geometry) -> Picture {
        let n4 = geo.w4 * geo.h4;
        let nctb = (geo.width_ctbs * geo.height_ctbs) as usize;
        Picture {
            width,
            height,
            planes: [Plane::new(width, height, 0), Plane::new(width / 2, height / 2, 0), Plane::new(width / 2, height / 2, 0)],
            w4: geo.w4,
            h4: geo.h4,
            ctdepth: vec![0; n4],
            intra_mode: vec![1; n4],
            qp: vec![0; n4],
            flags: vec![0; n4],
            ctb_slice: vec![NO_SLICE; nctb],
            sao: vec![SaoParams::default(); nctb],
            slices: Vec::new(),
            poc: 0,
            tag: 0,
            full_range: false,
            crop: (0, 0, width, height),
            output: true,
        }
    }

    #[inline]
    pub fn idx4(&self, x: usize, y: usize) -> usize {
        (y >> 2) * self.w4 + (x >> 2)
    }

    /// Fill a luma rectangle's 4x4 facts with `f`.
    pub fn fill4(&mut self, x0: usize, y0: usize, w: usize, h: usize, mut f: impl FnMut(&mut Picture, usize)) {
        for y in (y0..y0 + h).step_by(4) {
            for x in (x0..x0 + w).step_by(4) {
                let i = self.idx4(x, y);
                f(self, i);
            }
        }
    }

    /// The slice of the CTB holding luma sample (x, y).
    pub fn slice_at(&self, geo: &Geometry, x: usize, y: usize) -> Option<&SliceInfo> {
        let s = self.ctb_slice[geo.ctb_rs(x as u32, y as u32) as usize];
        if s == NO_SLICE { None } else { self.slices.get(s as usize) }
    }

    /// 6.4.1 z-scan order availability of the luma location (xn, yn) for the current location (xc, yc): inside the
    /// picture, not after the current block in z-scan order, decoded, in the same slice and the same tile.
    pub fn available(&self, geo: &Geometry, xc: i32, yc: i32, xn: i32, yn: i32) -> bool {
        if xn < 0 || yn < 0 || xn as usize >= self.width || yn as usize >= self.height {
            return false;
        }
        let (xn, yn, xc, yc) = (xn as usize, yn as usize, xc as usize, yc as usize);
        if geo.zs[(yn >> 2) * geo.w4 + (xn >> 2)] > geo.zs[(yc >> 2) * geo.w4 + (xc >> 2)] {
            return false;
        }
        let cn = geo.ctb_rs(xn as u32, yn as u32);
        let cc = geo.ctb_rs(xc as u32, yc as u32);
        let (sn, sc) = (self.ctb_slice[cn as usize], self.ctb_slice[cc as usize]);
        if sn == NO_SLICE || sc == NO_SLICE {
            return false;
        }
        if self.slices[sn as usize].addr_rs != self.slices[sc as usize].addr_rs {
            return false;
        }
        geo.tile_of_rs(cn) == geo.tile_of_rs(cc)
    }

    /// The cropped picture as h264_core's frame type (7.4.3.2.1 conformance window, 4:2:0).
    pub fn to_frame(&self) -> Frame {
        let (x0, y0, w, h) = self.crop;
        let crop = |pl: &Plane, x0: usize, y0: usize, w: usize, h: usize| {
            let mut v = Vec::with_capacity(w * h);
            for r in 0..h {
                let o = (y0 + r) * pl.w + x0;
                v.extend_from_slice(&pl.data[o..o + w]);
            }
            v
        };
        Frame {
            width: w as u32,
            height: h as u32,
            planes: [
                crop(&self.planes[0], x0, y0, w, h),
                crop(&self.planes[1], x0 / 2, y0 / 2, w / 2, h / 2),
                crop(&self.planes[2], x0 / 2, y0 / 2, w / 2, h / 2),
            ],
            poc: self.poc,
            tag: self.tag,
            full_range: self.full_range,
        }
    }
}
