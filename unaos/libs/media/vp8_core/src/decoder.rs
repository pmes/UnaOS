// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Frame decoding: the frame tag and header (RFC 6386 §9, §19.2), per-macroblock modes and
//! motion vectors (§11, §16, §17, §19.3), the DCT token partitions (§13), reconstruction (§12,
//! §14, §18), the loop filter (§15, §20.? `loop_filter_frame`) and the reference-buffer updates
//! (§9.7–§9.8).

use alloc::vec;
use alloc::vec::Vec;

use crate::bool_decoder::BoolDecoder;
use crate::consts::*;
use crate::idct::{idct_add, iwht};
use crate::loopfilter;
use crate::predict::{self, CWS, RefPlane, WS};
use crate::tables::*;
use crate::{Error, Result};

/// One plane of a decoded frame, macroblock-aligned (`width`, `height` multiples of 16 for luma,
/// 8 for chroma).
#[derive(Clone, Debug, Default)]
pub struct Plane {
    pub data: Vec<u8>,
    pub stride: usize,
    pub width: usize,
    pub height: usize,
}

impl Plane {
    fn new(w: usize, h: usize) -> Plane {
        Plane { data: vec![0; w * h], stride: w, width: w, height: h }
    }
    fn as_ref(&self) -> RefPlane<'_> {
        RefPlane { data: &self.data, stride: self.stride, w: self.width as i32, h: self.height as i32 }
    }
}

/// A macroblock-aligned I420 frame buffer.
#[derive(Clone, Debug, Default)]
pub struct FrameBuffer {
    pub y: Plane,
    pub u: Plane,
    pub v: Plane,
}

impl FrameBuffer {
    fn new(mb_cols: usize, mb_rows: usize) -> FrameBuffer {
        FrameBuffer {
            y: Plane::new(mb_cols * 16, mb_rows * 16),
            u: Plane::new(mb_cols * 8, mb_rows * 8),
            v: Plane::new(mb_cols * 8, mb_rows * 8),
        }
    }
}

/// A motion vector: row and column in eighth-sample luma units (the bitstream's quarter-pel
/// values doubled, so luma uses the even six-tap phases and chroma all eight).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Mv {
    pub row: i16,
    pub col: i16,
}

impl Mv {
    const ZERO: Mv = Mv { row: 0, col: 0 };
    fn is_zero(self) -> bool {
        self.row == 0 && self.col == 0
    }
}

#[derive(Clone, Copy, Debug)]
struct MbInfo {
    ymode: u8,
    uv_mode: u8,
    ref_frame: u8,
    bmodes: [u8; 16],
    mvs: [Mv; 16],
    mv: Mv,
}

impl Default for MbInfo {
    fn default() -> Self {
        MbInfo { ymode: DC_PRED, uv_mode: DC_PRED, ref_frame: INTRA_FRAME, bmodes: [B_DC_PRED; 16], mvs: [Mv::ZERO; 16], mv: Mv::ZERO }
    }
}

/// The persistent ("frame context") probabilities that `refresh_entropy_probs` saves and restores.
#[derive(Clone)]
struct Probs {
    coef: [[[[u8; 11]; 3]; 8]; 4],
    ymode: [u8; 4],
    uv_mode: [u8; 3],
    mv: [[u8; 19]; 2],
}

impl Probs {
    fn defaults() -> Probs {
        Probs { coef: DEFAULT_COEF_PROBS, ymode: YMODE_PROBS, uv_mode: UV_MODE_PROBS, mv: DEFAULT_MV_PROBS }
    }
}

/// What the frame tag and the uncompressed key-frame header carry (§9.1, §19.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameTag {
    pub key_frame: bool,
    pub version: u8,
    pub show_frame: bool,
    pub first_part_size: usize,
    /// Key frames only (0 otherwise).
    pub width: u16,
    pub height: u16,
    pub horiz_scale: u8,
    pub vert_scale: u8,
}

/// Parse the 3-byte frame tag (and, for key frames, the start code and dimensions).
pub fn parse_tag(d: &[u8]) -> Result<FrameTag> {
    if d.len() < 3 {
        return Err(Error::Truncated);
    }
    let raw = d[0] as u32 | (d[1] as u32) << 8 | (d[2] as u32) << 16;
    let mut t = FrameTag {
        key_frame: raw & 1 == 0,
        version: ((raw >> 1) & 7) as u8,
        show_frame: (raw >> 4) & 1 == 1,
        first_part_size: (raw >> 5) as usize,
        width: 0,
        height: 0,
        horiz_scale: 0,
        vert_scale: 0,
    };
    if t.key_frame {
        if d.len() < 10 {
            return Err(Error::Truncated);
        }
        if d[3..6] != [0x9d, 0x01, 0x2a] {
            return Err(Error::Malformed("vp8 start code"));
        }
        let w = u16::from_le_bytes([d[6], d[7]]);
        let h = u16::from_le_bytes([d[8], d[9]]);
        t.width = w & 0x3fff;
        t.horiz_scale = (w >> 14) as u8;
        t.height = h & 0x3fff;
        t.vert_scale = (h >> 14) as u8;
        if t.width == 0 || t.height == 0 {
            return Err(Error::Malformed("vp8 zero dimension"));
        }
    }
    Ok(t)
}

/// The per-frame header fields (§9.2–§9.11, §19.2).
#[derive(Default)]
struct Header {
    key: bool,
    simple_filter: bool,
    filter_level: i32,
    sharpness: i32,
    qindex: i32,
    y1dc: i32,
    y2dc: i32,
    y2ac: i32,
    uvdc: i32,
    uvac: i32,
    refresh_golden: bool,
    refresh_alt: bool,
    copy_to_golden: u32,
    copy_to_alt: u32,
    refresh_probs: bool,
    refresh_last: bool,
    skip_enabled: bool,
    prob_skip: u8,
    prob_intra: u8,
    prob_last: u8,
    prob_gf: u8,
}

/// Per-segment dequantisation factors: [Y1 dc, Y1 ac, Y2 dc, Y2 ac, UV dc, UV ac] (§14.1).
type Dq = [i32; 6];

const NO_BUF: usize = usize::MAX;

/// A VP8 stream decoder: feed it compressed frames in decode order.
pub struct Decoder {
    width: usize,
    height: usize,
    mb_cols: usize,
    mb_rows: usize,
    probs: Probs,
    seg_enabled: bool,
    seg_update_map: bool,
    seg_abs: bool,
    seg_quant: [i32; 4],
    seg_lf: [i32; 4],
    seg_tree: [u8; 3],
    lf_delta_enabled: bool,
    ref_lf_deltas: [i32; 4],
    mode_lf_deltas: [i32; 4],
    segment_map: Vec<u8>,
    mbs: Vec<MbInfo>,
    bufs: Vec<FrameBuffer>,
    last: usize,
    golden: usize,
    altref: usize,
    shown: usize,
    sign_bias: [bool; 4],
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

/// A decoded picture, borrowed from the decoder: the visible `width × height` window of
/// macroblock-aligned planes.
#[derive(Clone, Copy, Debug)]
pub struct Picture<'a> {
    pub width: u32,
    pub height: u32,
    pub y: &'a [u8],
    pub u: &'a [u8],
    pub v: &'a [u8],
    pub y_stride: usize,
    pub uv_stride: usize,
}

impl Picture<'_> {
    pub fn chroma_width(&self) -> u32 {
        self.width.div_ceil(2)
    }
    pub fn chroma_height(&self) -> u32 {
        self.height.div_ceil(2)
    }
    /// The planes cropped and packed back to back (Y, then U, then V) — the `.i420` layout the
    /// reference decoder's MD5s are taken over.
    pub fn to_i420(&self) -> Vec<u8> {
        let (w, h) = (self.width as usize, self.height as usize);
        let (cw, ch) = (self.chroma_width() as usize, self.chroma_height() as usize);
        let mut out = Vec::with_capacity(w * h + 2 * cw * ch);
        for r in 0..h {
            out.extend_from_slice(&self.y[r * self.y_stride..r * self.y_stride + w]);
        }
        for p in [self.u, self.v] {
            for r in 0..ch {
                out.extend_from_slice(&p[r * self.uv_stride..r * self.uv_stride + cw]);
            }
        }
        out
    }
}

impl Decoder {
    pub fn new() -> Decoder {
        Decoder {
            width: 0,
            height: 0,
            mb_cols: 0,
            mb_rows: 0,
            probs: Probs::defaults(),
            seg_enabled: false,
            seg_update_map: false,
            seg_abs: false,
            seg_quant: [0; 4],
            seg_lf: [0; 4],
            seg_tree: [255; 3],
            lf_delta_enabled: false,
            ref_lf_deltas: [0; 4],
            mode_lf_deltas: [0; 4],
            segment_map: Vec::new(),
            mbs: Vec::new(),
            bufs: Vec::new(),
            last: NO_BUF,
            golden: NO_BUF,
            altref: NO_BUF,
            shown: NO_BUF,
            sign_bias: [false; 4],
        }
    }

    /// Forget every reference (after a seek: the next frame must be a key frame).
    pub fn reset(&mut self) {
        *self = Decoder::new();
    }

    pub fn width(&self) -> u32 {
        self.width as u32
    }
    pub fn height(&self) -> u32 {
        self.height as u32
    }

    /// The most recently shown picture, if any.
    pub fn picture(&self) -> Option<Picture<'_>> {
        if self.shown == NO_BUF {
            return None;
        }
        let b = &self.bufs[self.shown];
        Some(Picture {
            width: self.width as u32,
            height: self.height as u32,
            y: &b.y.data,
            u: &b.u.data,
            v: &b.v.data,
            y_stride: b.y.stride,
            uv_stride: b.u.stride,
        })
    }

    /// Decode one compressed frame. Returns the picture when the frame is shown
    /// (`show_frame` = 1), `None` for a hidden (alt-ref) frame.
    pub fn decode(&mut self, data: &[u8]) -> Result<Option<Picture<'_>>> {
        let tag = parse_tag(data)?;
        let hdr_len = if tag.key_frame { 10 } else { 3 };
        if tag.key_frame {
            let (w, h) = (tag.width as usize, tag.height as usize);
            if w != self.width || h != self.height || self.bufs.is_empty() {
                self.allocate(w, h);
            }
        } else if self.last == NO_BUF {
            return Err(Error::Malformed("vp8 inter frame without a key frame"));
        }
        let first_end = hdr_len + tag.first_part_size;
        if first_end > data.len() {
            return Err(Error::Truncated);
        }
        let mut bd = BoolDecoder::new(&data[hdr_len..first_end]);
        let mut h = Header { key: tag.key_frame, ..Header::default() };
        self.read_header(&mut bd, &mut h);
        let nparts = 1usize << bd.literal(2);
        // Token partitions (§9.5): (n-1) 3-byte little-endian sizes, then the partitions.
        let sizes_at = first_end;
        let mut p = sizes_at + 3 * (nparts - 1);
        if p > data.len() {
            return Err(Error::Truncated);
        }
        let mut parts: Vec<BoolDecoder> = Vec::with_capacity(nparts);
        for i in 0..nparts {
            let size = if i + 1 < nparts {
                let s = sizes_at + 3 * i;
                data[s] as usize | (data[s + 1] as usize) << 8 | (data[s + 2] as usize) << 16
            } else {
                data.len() - p
            };
            let end = (p + size).min(data.len());
            parts.push(BoolDecoder::new(&data[p..end]));
            p = end;
        }
        let saved = self.read_header_tail(&mut bd, &mut h);

        let cur = self.free_buffer();
        let mut fb = core::mem::take(&mut self.bufs[cur]);
        let lf = self.decode_macroblocks(&mut bd, &mut parts, &h, tag.version, &mut fb);
        if h.filter_level > 0 {
            self.loop_filter(&mut fb, &h, &lf);
        }
        self.bufs[cur] = fb;

        if let Some(p) = saved {
            self.probs = p;
        }
        // Reference updates (§9.7, in the reference decoder's order: alt-ref copy, golden copy,
        // then the refreshes).
        if h.key {
            self.golden = cur;
            self.altref = cur;
            self.last = cur;
        } else {
            match h.copy_to_alt {
                1 => self.altref = self.last,
                2 => self.altref = self.golden,
                _ => {}
            }
            match h.copy_to_golden {
                1 => self.golden = self.last,
                2 => self.golden = self.altref,
                _ => {}
            }
            if h.refresh_golden {
                self.golden = cur;
            }
            if h.refresh_alt {
                self.altref = cur;
            }
            if h.refresh_last {
                self.last = cur;
            }
        }
        if tag.show_frame {
            self.shown = cur;
            Ok(self.picture())
        } else {
            // A hidden frame leaves the shown picture alone unless it overwrote its buffer.
            Ok(None)
        }
    }

    fn allocate(&mut self, w: usize, h: usize) {
        self.width = w;
        self.height = h;
        self.mb_cols = w.div_ceil(16);
        self.mb_rows = h.div_ceil(16);
        self.bufs = (0..5).map(|_| FrameBuffer::new(self.mb_cols, self.mb_rows)).collect();
        self.segment_map = vec![0; self.mb_cols * self.mb_rows];
        self.mbs = vec![MbInfo::default(); (self.mb_cols + 1) * (self.mb_rows + 1)];
        self.last = NO_BUF;
        self.golden = NO_BUF;
        self.altref = NO_BUF;
        self.shown = NO_BUF;
    }

    /// A buffer no reference slot (nor the shown picture) points at.
    fn free_buffer(&self) -> usize {
        (0..self.bufs.len())
            .find(|&i| i != self.last && i != self.golden && i != self.altref && i != self.shown)
            .or_else(|| (0..self.bufs.len()).find(|&i| i != self.last && i != self.golden && i != self.altref))
            .unwrap_or(0)
    }

    /// The header up to the token-partition count (§19.2).
    fn read_header(&mut self, bd: &mut BoolDecoder, h: &mut Header) {
        if h.key {
            let _color_space = bd.flag();
            let _clamping_type = bd.flag();
            // Key frames reset the frame context (§9.11, §9.3, §9.6).
            self.probs = Probs::defaults();
            self.seg_quant = [0; 4];
            self.seg_lf = [0; 4];
            self.seg_abs = false;
            self.ref_lf_deltas = [0; 4];
            self.mode_lf_deltas = [0; 4];
            self.sign_bias = [false; 4];
        }
        // Segmentation (§9.3).
        self.seg_enabled = bd.flag();
        self.seg_update_map = false;
        if self.seg_enabled {
            self.seg_update_map = bd.flag();
            let update_data = bd.flag();
            if update_data {
                self.seg_abs = bd.flag();
                for q in self.seg_quant.iter_mut() {
                    *q = bd.opt_signed(7);
                }
                for l in self.seg_lf.iter_mut() {
                    *l = bd.opt_signed(6);
                }
            }
            if self.seg_update_map {
                for p in self.seg_tree.iter_mut() {
                    *p = if bd.flag() { bd.literal(8) as u8 } else { 255 };
                }
            }
        }
        // Loop filter (§9.4).
        h.simple_filter = bd.flag();
        h.filter_level = bd.literal(6) as i32;
        h.sharpness = bd.literal(3) as i32;
        self.lf_delta_enabled = bd.flag();
        if self.lf_delta_enabled && bd.flag() {
            for d in self.ref_lf_deltas.iter_mut().chain(self.mode_lf_deltas.iter_mut()) {
                if bd.flag() {
                    *d = bd.signed(6);
                }
            }
        }
    }

    /// The header after the partition count: quantisers, reference flags, probability updates
    /// (§9.6–§9.11, §19.2). Returns the probabilities to restore after this frame when its
    /// updates are not to persist (`refresh_entropy_probs` = 0).
    fn read_header_tail(&mut self, bd: &mut BoolDecoder, h: &mut Header) -> Option<Probs> {
        h.qindex = bd.literal(7) as i32;
        h.y1dc = bd.opt_signed(4);
        h.y2dc = bd.opt_signed(4);
        h.y2ac = bd.opt_signed(4);
        h.uvdc = bd.opt_signed(4);
        h.uvac = bd.opt_signed(4);
        if h.key {
            h.refresh_golden = true;
            h.refresh_alt = true;
        } else {
            h.refresh_golden = bd.flag();
            h.refresh_alt = bd.flag();
            if !h.refresh_golden {
                h.copy_to_golden = bd.literal(2);
            }
            if !h.refresh_alt {
                h.copy_to_alt = bd.literal(2);
            }
            self.sign_bias[GOLDEN_FRAME as usize] = bd.flag();
            self.sign_bias[ALTREF_FRAME as usize] = bd.flag();
        }
        h.refresh_probs = bd.flag();
        let saved = if h.refresh_probs { None } else { Some(self.probs.clone()) };
        h.refresh_last = h.key || bd.flag();
        // Token probability updates (§13.4).
        for i in 0..4 {
            for j in 0..8 {
                for k in 0..3 {
                    for l in 0..11 {
                        if bd.read(COEF_UPDATE_PROBS[i][j][k][l]) {
                            self.probs.coef[i][j][k][l] = bd.literal(8) as u8;
                        }
                    }
                }
            }
        }
        h.skip_enabled = bd.flag();
        if h.skip_enabled {
            h.prob_skip = bd.literal(8) as u8;
        }
        if !h.key {
            h.prob_intra = bd.literal(8) as u8;
            h.prob_last = bd.literal(8) as u8;
            h.prob_gf = bd.literal(8) as u8;
            if bd.flag() {
                for p in self.probs.ymode.iter_mut() {
                    *p = bd.literal(8) as u8;
                }
            }
            if bd.flag() {
                for p in self.probs.uv_mode.iter_mut() {
                    *p = bd.literal(8) as u8;
                }
            }
            // Motion-vector probability updates (§17.2).
            for i in 0..2 {
                for j in 0..19 {
                    if bd.read(MV_UPDATE_PROBS[i][j]) {
                        let x = bd.literal(7) as u8;
                        self.probs.mv[i][j] = if x != 0 { x << 1 } else { 1 };
                    }
                }
            }
        }
        saved
    }

    fn dequant_factors(&self, h: &Header) -> [Dq; 4] {
        let q = |i: i32| i.clamp(0, 127) as usize;
        let mut out = [[0; 6]; 4];
        for (s, o) in out.iter_mut().enumerate() {
            let base = if self.seg_enabled {
                if self.seg_abs { self.seg_quant[s] } else { h.qindex + self.seg_quant[s] }
            } else {
                h.qindex
            }
            .clamp(0, 127);
            *o = [
                DC_QLOOKUP[q(base + h.y1dc)],
                AC_QLOOKUP[q(base)],
                DC_QLOOKUP[q(base + h.y2dc)] * 2,
                ((AC_QLOOKUP[q(base + h.y2ac)] * 101581) >> 16).max(8),
                DC_QLOOKUP[q(base + h.uvdc)].min(132),
                AC_QLOOKUP[q(base + h.uvac)],
            ];
        }
        out
    }

    /// The loop-filter level of one macroblock (§9.6 segment override, §9.6 delta adjustments).
    fn filter_level(&self, h: &Header, segment: usize, mb: &MbInfo) -> i32 {
        let mut level = h.filter_level;
        if self.seg_enabled {
            level = if self.seg_abs { self.seg_lf[segment] } else { level + self.seg_lf[segment] };
            level = level.clamp(0, 63);
        }
        if self.lf_delta_enabled {
            level += self.ref_lf_deltas[mb.ref_frame as usize];
            if mb.ref_frame == INTRA_FRAME {
                if mb.ymode == B_PRED {
                    level += self.mode_lf_deltas[0];
                }
            } else if mb.ymode == ZEROMV {
                level += self.mode_lf_deltas[1];
            } else if mb.ymode == SPLITMV {
                level += self.mode_lf_deltas[3];
            } else {
                level += self.mode_lf_deltas[2];
            }
            level = level.clamp(0, 63);
        }
        level
    }

    /// Parse modes, decode residuals and reconstruct every macroblock into `fb`. Returns the
    /// per-macroblock (filter level, filter inner edges) pairs for the loop filter.
    fn decode_macroblocks(
        &mut self,
        bd: &mut BoolDecoder,
        parts: &mut [BoolDecoder],
        h: &Header,
        version: u8,
        fb: &mut FrameBuffer,
    ) -> Vec<(u8, bool)> {
        let dq = self.dequant_factors(h);
        let (cols, rows) = (self.mb_cols, self.mb_rows);
        let ms = cols + 1;
        // The border entries (row -1, column -1) stay "intra, DC_PRED, zero motion".
        for m in self.mbs.iter_mut() {
            *m = MbInfo::default();
        }
        let mut lf = vec![(0u8, false); cols * rows];
        let mut above_ctx = vec![[0u8; 9]; cols];
        let bilinear = version != 0;
        let full_pixel = version == 3;
        let nparts = parts.len();
        for r in 0..rows {
            let mut left_ctx = [0u8; 9];
            let part = &mut parts[r % nparts];
            for c in 0..cols {
                let idx = (r + 1) * ms + c + 1;
                // --- Modes (§19.3) ---
                if self.seg_update_map {
                    let t = self.seg_tree;
                    self.segment_map[r * cols + c] = if bd.read(t[0]) { 2 + bd.read(t[2]) as u8 } else { bd.read(t[1]) as u8 };
                } else if h.key {
                    self.segment_map[r * cols + c] = 0;
                }
                let segment = self.segment_map[r * cols + c] as usize;
                let skip = h.skip_enabled && bd.read(h.prob_skip);
                let mb = if h.key { self.read_kf_modes(bd, idx, ms) } else { self.read_inter_modes(bd, h, idx, ms, r, c) };
                self.mbs[idx] = mb;

                // --- Residual (§13) ---
                let has_y2 = mb.ymode != B_PRED && mb.ymode != SPLITMV;
                let mut coeffs = [[0i16; 16]; 25];
                let mut nonzero = false;
                if !skip {
                    nonzero = self.read_residual(part, &mut above_ctx[c], &mut left_ctx, has_y2, &dq[segment], &mut coeffs);
                } else {
                    // A skipped macroblock leaves its neighbours' token contexts at zero; the Y2
                    // context only when it has a Y2 block (§13.3).
                    let keep_y2 = !has_y2;
                    let (a8, l8) = (above_ctx[c][8], left_ctx[8]);
                    above_ctx[c] = [0; 9];
                    left_ctx = [0; 9];
                    if keep_y2 {
                        above_ctx[c][8] = a8;
                        left_ctx[8] = l8;
                    }
                }
                if has_y2 && nonzero {
                    let dcs = iwht(&coeffs[24]);
                    for (i, &d) in dcs.iter().enumerate() {
                        coeffs[i][0] = d;
                    }
                }

                // --- Reconstruction ---
                if mb.ref_frame == INTRA_FRAME {
                    self.recon_intra(fb, &mb, r, c, &coeffs, nonzero);
                } else {
                    let rf = match mb.ref_frame {
                        LAST_FRAME => self.last,
                        GOLDEN_FRAME => self.golden,
                        _ => self.altref,
                    };
                    let rf = if rf == NO_BUF { self.last } else { rf };
                    self.recon_inter(fb, rf, &mb, r, c, &coeffs, nonzero, bilinear, full_pixel);
                }

                let level = self.filter_level(h, segment, &mb);
                let inner = !(has_y2 && (skip || !nonzero));
                lf[r * cols + c] = (level as u8, inner);
            }
        }
        lf
    }

    fn read_kf_modes(&self, bd: &mut BoolDecoder, idx: usize, ms: usize) -> MbInfo {
        let mut mb = MbInfo { ymode: bd.tree(&KF_YMODE_TREE, &KF_YMODE_PROBS), ..MbInfo::default() };
        if mb.ymode == B_PRED {
            let above = &self.mbs[idx - ms];
            let left = &self.mbs[idx - 1];
            for i in 0..16 {
                let a = if i < 4 { above.bmodes[i + 12] } else { mb.bmodes[i - 4] };
                let l = if i & 3 == 0 { left.bmodes[i + 3] } else { mb.bmodes[i - 1] };
                mb.bmodes[i] = bd.tree(&BMODE_TREE, &KF_BMODE_PROBS[a as usize][l as usize]);
            }
        } else {
            mb.bmodes = [implied_bmode(mb.ymode); 16];
        }
        mb.uv_mode = bd.tree(&UV_MODE_TREE, &KF_UV_MODE_PROBS);
        mb
    }

    fn read_mv(&self, bd: &mut BoolDecoder) -> Mv {
        let row = read_mv_component(bd, &self.probs.mv[0]) * 2;
        let col = read_mv_component(bd, &self.probs.mv[1]) * 2;
        Mv { row: row as i16, col: col as i16 }
    }

    fn read_inter_modes(&self, bd: &mut BoolDecoder, h: &Header, idx: usize, ms: usize, r: usize, c: usize) -> MbInfo {
        let mut mb = MbInfo::default();
        if !bd.read(h.prob_intra) {
            mb.ymode = bd.tree(&YMODE_TREE, &self.probs.ymode);
            if mb.ymode == B_PRED {
                for i in 0..16 {
                    mb.bmodes[i] = bd.tree(&BMODE_TREE, &BMODE_PROBS);
                }
            } else {
                mb.bmodes = [implied_bmode(mb.ymode); 16];
            }
            mb.uv_mode = bd.tree(&UV_MODE_TREE, &self.probs.uv_mode);
            return mb;
        }
        mb.ref_frame = if bd.read(h.prob_last) { GOLDEN_FRAME + bd.read(h.prob_gf) as u8 } else { LAST_FRAME };

        // Near-MV search (§16.3).
        let above = self.mbs[idx - ms];
        let left = self.mbs[idx - 1];
        let above_left = self.mbs[idx - ms - 1];
        let bias = |n: &MbInfo| -> Mv {
            let mut v = n.mv;
            if self.sign_bias[n.ref_frame as usize] != self.sign_bias[mb.ref_frame as usize] {
                v.row = v.row.wrapping_neg();
                v.col = v.col.wrapping_neg();
            }
            v
        };
        let mut near = [Mv::ZERO; 4];
        let mut cnt = [0i32; 4];
        let mut n = 0usize;
        if above.ref_frame != INTRA_FRAME {
            if !above.mv.is_zero() {
                n += 1;
                near[n] = bias(&above);
            }
            cnt[n] += 2;
        }
        if left.ref_frame != INTRA_FRAME {
            if !left.mv.is_zero() {
                let m = bias(&left);
                if m != near[n] {
                    n += 1;
                    near[n] = m;
                }
                cnt[n] += 2;
            } else {
                cnt[0] += 2;
            }
        }
        if above_left.ref_frame != INTRA_FRAME {
            if !above_left.mv.is_zero() {
                let m = bias(&above_left);
                if m != near[n] {
                    n += 1;
                    near[n] = m;
                }
                cnt[n] += 1;
            } else {
                cnt[0] += 1;
            }
        }

        // Bounds for clamping, in eighth-pel: the macroblock may point at most 16 pixels (plus
        // the margin) outside the frame (§16.3 / `vp8_clamp_mv2`).
        let to_left = -((c as i32 * 16) << 3) - 128;
        let to_right = (((self.mb_cols - 1 - c) as i32 * 16) << 3) + 128;
        let to_top = -((r as i32 * 16) << 3) - 128;
        let to_bottom = (((self.mb_rows - 1 - r) as i32 * 16) << 3) + 128;
        let clamp = |v: Mv| Mv {
            col: (v.col as i32).clamp(to_left, to_right) as i16,
            row: (v.row as i32).clamp(to_top, to_bottom) as i16,
        };

        if !bd.read(MODE_CONTEXTS[cnt[0] as usize][0]) {
            mb.ymode = ZEROMV;
            mb.mv = Mv::ZERO;
        } else {
            // Merge above-left with nearest when three distinct vectors were found.
            if cnt[3] > 0 && near[n] == near[1] {
                cnt[1] += 1;
            }
            if cnt[2] > cnt[1] {
                cnt.swap(1, 2);
                near.swap(1, 2);
            }
            if !bd.read(MODE_CONTEXTS[cnt[1].min(5) as usize][1]) {
                mb.ymode = NEARESTMV;
                mb.mv = clamp(near[1]);
            } else if !bd.read(MODE_CONTEXTS[cnt[2].min(5) as usize][2]) {
                mb.ymode = NEARMV;
                mb.mv = clamp(near[2]);
            } else {
                let best = clamp(if cnt[1] >= cnt[0] { near[1] } else { near[0] });
                let split_cnt = (above.ymode == SPLITMV) as usize * 2 + (left.ymode == SPLITMV) as usize * 2
                    + (above_left.ymode == SPLITMV) as usize;
                if !bd.read(MODE_CONTEXTS[split_cnt][3]) {
                    mb.ymode = NEWMV;
                    let d = self.read_mv(bd);
                    mb.mv = Mv { row: d.row.wrapping_add(best.row), col: d.col.wrapping_add(best.col) };
                } else {
                    mb.ymode = SPLITMV;
                    self.read_split(bd, &mut mb, &left, &above, best);
                    mb.mv = mb.mvs[15];
                    return mb;
                }
            }
        }
        mb.mvs = [mb.mv; 16];
        mb
    }

    /// SPLITMV partitions and their vectors (§16.4 / `decode_split_mv`).
    fn read_split(&self, bd: &mut BoolDecoder, mb: &mut MbInfo, left: &MbInfo, above: &MbInfo, best: Mv) {
        let s = if !bd.read(MBSPLIT_PROBS[0]) {
            3
        } else if !bd.read(MBSPLIT_PROBS[1]) {
            2
        } else {
            bd.read(MBSPLIT_PROBS[2]) as usize
        };
        let num = [2usize, 2, 4, 16][s];
        let map = &MBSPLITS[s];
        for j in 0..num {
            let k = map.iter().position(|&m| m as usize == j).unwrap_or(0);
            let lmv = if k & 3 == 0 {
                if left.ymode == SPLITMV { left.mvs[k + 3] } else { left.mv }
            } else {
                mb.mvs[k - 1]
            };
            let amv = if k < 4 {
                if above.ymode == SPLITMV { above.mvs[k + 12] } else { above.mv }
            } else {
                mb.mvs[k - 4]
            };
            let ctx = if lmv == amv {
                if amv.is_zero() { 4 } else { 3 }
            } else if amv.is_zero() {
                2
            } else if lmv.is_zero() {
                1
            } else {
                0
            };
            let p = &SUB_MV_REF_PROBS[ctx];
            let v = if !bd.read(p[0]) {
                lmv
            } else if !bd.read(p[1]) {
                amv
            } else if !bd.read(p[2]) {
                Mv::ZERO
            } else {
                let d = self.read_mv(bd);
                Mv { row: d.row.wrapping_add(best.row), col: d.col.wrapping_add(best.col) }
            };
            for (b, &m) in map.iter().enumerate() {
                if m as usize == j {
                    mb.mvs[b] = v;
                }
            }
        }
    }

    /// All 25 blocks' tokens for one macroblock. Returns whether any block had a token other
    /// than an immediate end-of-block.
    fn read_residual(
        &self,
        bd: &mut BoolDecoder,
        above: &mut [u8; 9],
        left: &mut [u8; 9],
        has_y2: bool,
        dq: &Dq,
        coeffs: &mut [[i16; 16]; 25],
    ) -> bool {
        let mut any = false;
        let (ytype, first) = if has_y2 {
            let ctx = (above[8] + left[8]) as usize;
            let nz = read_block(bd, &self.probs.coef[1], ctx, 0, dq[2], dq[3], &mut coeffs[24]);
            above[8] = nz as u8;
            left[8] = nz as u8;
            any |= nz;
            (0, 1)
        } else {
            (3, 0)
        };
        for y in 0..4 {
            for x in 0..4 {
                let ctx = (above[x] + left[y]) as usize;
                let nz = read_block(bd, &self.probs.coef[ytype], ctx, first, dq[0], dq[1], &mut coeffs[y * 4 + x]);
                above[x] = nz as u8;
                left[y] = nz as u8;
                any |= nz;
            }
        }
        for plane in 0..2 {
            for y in 0..2 {
                for x in 0..2 {
                    let (ai, li) = (4 + plane * 2 + x, 4 + plane * 2 + y);
                    let ctx = (above[ai] + left[li]) as usize;
                    let nz = read_block(bd, &self.probs.coef[2], ctx, 0, dq[4], dq[5], &mut coeffs[16 + plane * 4 + y * 2 + x]);
                    above[ai] = nz as u8;
                    left[li] = nz as u8;
                    any |= nz;
                }
            }
        }
        any
    }

    fn recon_intra(&self, fb: &mut FrameBuffer, mb: &MbInfo, r: usize, c: usize, coeffs: &[[i16; 16]; 25], residual: bool) {
        let cols = self.mb_cols;
        // Luma workspace with the §12.2 edges.
        let mut ws = [0u8; WS * 17];
        {
            let p = &fb.y;
            let (x0, y0) = (c * 16, r * 16);
            if r == 0 {
                ws[..WS].fill(127);
            } else {
                let row = (y0 - 1) * p.stride;
                ws[1..17].copy_from_slice(&p.data[row + x0..row + x0 + 16]);
                if c + 1 < cols {
                    ws[17..21].copy_from_slice(&p.data[row + x0 + 16..row + x0 + 20]);
                } else {
                    ws[17..21].fill(p.data[row + x0 + 15]);
                }
                ws[0] = if c == 0 { 129 } else { p.data[row + x0 - 1] };
            }
            for i in 0..16 {
                ws[(i + 1) * WS] = if c == 0 { 129 } else { p.data[(y0 + i) * p.stride + x0 - 1] };
            }
            if mb.ymode == B_PRED {
                // Sub-blocks on the right column take their above-right from the row above the
                // macroblock (§12.3).
                for k in [4usize, 8, 12] {
                    ws.copy_within(17..21, k * WS + 17);
                }
                for i in 0..16 {
                    let o = (1 + (i / 4) * 4) * WS + 1 + (i % 4) * 4;
                    predict::predict_sub(&mut ws, WS, o, mb.bmodes[i]);
                    if residual {
                        idct_add(&coeffs[i], &mut ws, o, WS);
                    }
                }
            } else {
                predict::predict_mb(&mut ws, WS, 16, mb.ymode, r > 0, c > 0);
                if residual {
                    for i in 0..16 {
                        let o = (1 + (i / 4) * 4) * WS + 1 + (i % 4) * 4;
                        idct_add(&coeffs[i], &mut ws, o, WS);
                    }
                }
            }
            let p = &mut fb.y;
            for i in 0..16 {
                let d = (y0 + i) * p.stride + x0;
                p.data[d..d + 16].copy_from_slice(&ws[(i + 1) * WS + 1..(i + 1) * WS + 17]);
            }
        }
        for (pi, plane) in [&mut fb.u, &mut fb.v].into_iter().enumerate() {
            let mut ws = [0u8; CWS * 9];
            let (x0, y0) = (c * 8, r * 8);
            if r == 0 {
                ws[..CWS].fill(127);
            } else {
                let row = (y0 - 1) * plane.stride;
                ws[1..9].copy_from_slice(&plane.data[row + x0..row + x0 + 8]);
                ws[0] = if c == 0 { 129 } else { plane.data[row + x0 - 1] };
            }
            for i in 0..8 {
                ws[(i + 1) * CWS] = if c == 0 { 129 } else { plane.data[(y0 + i) * plane.stride + x0 - 1] };
            }
            predict::predict_mb(&mut ws, CWS, 8, mb.uv_mode, r > 0, c > 0);
            if residual {
                for i in 0..4 {
                    let o = (1 + (i / 2) * 4) * CWS + 1 + (i % 2) * 4;
                    idct_add(&coeffs[16 + pi * 4 + i], &mut ws, o, CWS);
                }
            }
            for i in 0..8 {
                let d = (y0 + i) * plane.stride + x0;
                plane.data[d..d + 8].copy_from_slice(&ws[(i + 1) * CWS + 1..(i + 1) * CWS + 9]);
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn recon_inter(
        &self,
        fb: &mut FrameBuffer,
        rf: usize,
        mb: &MbInfo,
        r: usize,
        c: usize,
        coeffs: &[[i16; 16]; 25],
        residual: bool,
        bilinear: bool,
        full_pixel: bool,
    ) {
        let refb = &self.bufs[rf];
        let (x0, y0) = (c * 16, r * 16);
        let ry = refb.y.as_ref();
        {
            let p = &mut fb.y;
            let o = y0 * p.stride + x0;
            let stride = p.stride;
            if mb.ymode != SPLITMV {
                predict::predict_inter(&ry, x0 as i32, y0 as i32, 16, 16, mb.mv.col as i32, mb.mv.row as i32, bilinear, &mut p.data[o..], stride);
            } else {
                for i in 0..16 {
                    let (bx, by) = ((i % 4) * 4, (i / 4) * 4);
                    let v = mb.mvs[i];
                    predict::predict_inter(
                        &ry,
                        (x0 + bx) as i32,
                        (y0 + by) as i32,
                        4,
                        4,
                        v.col as i32,
                        v.row as i32,
                        bilinear,
                        &mut p.data[o + by * stride + bx..],
                        stride,
                    );
                }
            }
            if residual {
                for i in 0..16 {
                    idct_add(&coeffs[i], &mut p.data, o + (i / 4) * 4 * stride + (i % 4) * 4, stride);
                }
            }
        }
        // Chroma vectors (§17.4 / `build_uvmvs`): the whole-macroblock vector halved, or each
        // 2×2 group of luma vectors averaged, rounding away from zero; full-pixel streams drop the
        // fraction.
        let mask: i32 = if full_pixel { !7 } else { !0 };
        let mut cmv = [(0i32, 0i32); 4];
        if mb.ymode != SPLITMV {
            let half = |v: i32| (v + if v < 0 { -1 } else { 1 }) / 2;
            let m = (half(mb.mv.col as i32) & mask, half(mb.mv.row as i32) & mask);
            cmv = [m; 4];
        } else {
            for (k, cm) in cmv.iter_mut().enumerate() {
                let (i, j) = (k / 2, k % 2);
                let b = i * 8 + j * 2;
                let sum = |f: fn(&Mv) -> i16| -> i32 {
                    let s = f(&mb.mvs[b]) as i32 + f(&mb.mvs[b + 1]) as i32 + f(&mb.mvs[b + 4]) as i32 + f(&mb.mvs[b + 5]) as i32;
                    let s = s + 4 + if s < 0 { -8 } else { 0 };
                    (s / 8) & mask
                };
                *cm = (sum(|m| m.col), sum(|m| m.row));
            }
        }
        let (cx0, cy0) = (c * 8, r * 8);
        for (pi, (plane, rp)) in [(&mut fb.u, &refb.u), (&mut fb.v, &refb.v)].into_iter().enumerate() {
            let rr = rp.as_ref();
            let stride = plane.stride;
            let o = cy0 * stride + cx0;
            if mb.ymode != SPLITMV {
                predict::predict_inter(&rr, cx0 as i32, cy0 as i32, 8, 8, cmv[0].0, cmv[0].1, bilinear, &mut plane.data[o..], stride);
            } else {
                for k in 0..4 {
                    let (bx, by) = ((k % 2) * 4, (k / 2) * 4);
                    predict::predict_inter(
                        &rr,
                        (cx0 + bx) as i32,
                        (cy0 + by) as i32,
                        4,
                        4,
                        cmv[k].0,
                        cmv[k].1,
                        bilinear,
                        &mut plane.data[o + by * stride + bx..],
                        stride,
                    );
                }
            }
            if residual {
                for k in 0..4 {
                    idct_add(&coeffs[16 + pi * 4 + k], &mut plane.data, o + (k / 2) * 4 * stride + (k % 2) * 4, stride);
                }
            }
        }
    }

    /// §15: every macroblock in raster order — left edge, inner vertical edges, top edge, inner
    /// horizontal edges.
    fn loop_filter(&self, fb: &mut FrameBuffer, h: &Header, lf: &[(u8, bool)]) {
        let mut cache = [None; 64];
        for r in 0..self.mb_rows {
            for c in 0..self.mb_cols {
                let (level, inner) = lf[r * self.mb_cols + c];
                if level == 0 {
                    continue;
                }
                let p = *cache[level as usize].get_or_insert_with(|| loopfilter::params(level as i32, h.sharpness, h.key));
                let ys = fb.y.stride;
                let yo = r * 16 * ys + c * 16;
                if h.simple_filter {
                    let y = &mut fb.y.data;
                    if c > 0 {
                        loopfilter::simple_edge(y, yo, 1, ys, 16, p.mb_limit);
                    }
                    if inner {
                        for k in [4, 8, 12] {
                            loopfilter::simple_edge(y, yo + k, 1, ys, 16, p.sub_limit);
                        }
                    }
                    if r > 0 {
                        loopfilter::simple_edge(y, yo, ys, 1, 16, p.mb_limit);
                    }
                    if inner {
                        for k in [4, 8, 12] {
                            loopfilter::simple_edge(y, yo + k * ys, ys, 1, 16, p.sub_limit);
                        }
                    }
                    continue;
                }
                let cs = fb.u.stride;
                let co = r * 8 * cs + c * 8;
                if c > 0 {
                    loopfilter::normal_edge(&mut fb.y.data, yo, 1, ys, 16, p.mb_limit, &p, true);
                    loopfilter::normal_edge(&mut fb.u.data, co, 1, cs, 8, p.mb_limit, &p, true);
                    loopfilter::normal_edge(&mut fb.v.data, co, 1, cs, 8, p.mb_limit, &p, true);
                }
                if inner {
                    for k in [4, 8, 12] {
                        loopfilter::normal_edge(&mut fb.y.data, yo + k, 1, ys, 16, p.sub_limit, &p, false);
                    }
                    loopfilter::normal_edge(&mut fb.u.data, co + 4, 1, cs, 8, p.sub_limit, &p, false);
                    loopfilter::normal_edge(&mut fb.v.data, co + 4, 1, cs, 8, p.sub_limit, &p, false);
                }
                if r > 0 {
                    loopfilter::normal_edge(&mut fb.y.data, yo, ys, 1, 16, p.mb_limit, &p, true);
                    loopfilter::normal_edge(&mut fb.u.data, co, cs, 1, 8, p.mb_limit, &p, true);
                    loopfilter::normal_edge(&mut fb.v.data, co, cs, 1, 8, p.mb_limit, &p, true);
                }
                if inner {
                    for k in [4, 8, 12] {
                        loopfilter::normal_edge(&mut fb.y.data, yo + k * ys, ys, 1, 16, p.sub_limit, &p, false);
                    }
                    loopfilter::normal_edge(&mut fb.u.data, co + 4 * cs, cs, 1, 8, p.sub_limit, &p, false);
                    loopfilter::normal_edge(&mut fb.v.data, co + 4 * cs, cs, 1, 8, p.sub_limit, &p, false);
                }
            }
        }
    }
}

/// The sub-block mode a non-B_PRED macroblock implies for its neighbours' contexts (§11.3).
fn implied_bmode(ymode: u8) -> u8 {
    match ymode {
        V_PRED => B_VE_PRED,
        H_PRED => B_HE_PRED,
        TM_PRED => B_TM_PRED,
        _ => B_DC_PRED,
    }
}

/// One motion-vector component (§17.2), in quarter-pel units.
fn read_mv_component(bd: &mut BoolDecoder, p: &[u8; 19]) -> i32 {
    const IS_SHORT: usize = 0;
    const SIGN: usize = 1;
    const SHORT: usize = 2;
    const LONG: usize = 9;
    let mut x: i32;
    if bd.read(p[IS_SHORT]) {
        x = 0;
        for i in 0..3 {
            x += (bd.read(p[LONG + i]) as i32) << i;
        }
        for i in (4..10).rev() {
            x += (bd.read(p[LONG + i]) as i32) << i;
        }
        if x & 0xfff0 == 0 || bd.read(p[LONG + 3]) {
            x += 8;
        }
    } else {
        x = bd.tree(&SMALL_MV_TREE, &p[SHORT..SHORT + 7]) as i32;
    }
    if x != 0 && bd.read(p[SIGN]) {
        x = -x;
    }
    x
}

/// One block's tokens (§13.2–§13.3), dequantised into raster order. Returns whether anything
/// but an immediate end-of-block was read (the neighbour context).
fn read_block(bd: &mut BoolDecoder, probs: &[[[u8; 11]; 3]; 8], ctx: usize, first: usize, dc: i32, ac: i32, out: &mut [i16; 16]) -> bool {
    let mut i = first;
    let mut p = &probs[BANDS[i]][ctx];
    if !bd.read(p[0]) {
        return false;
    }
    loop {
        while !bd.read(p[1]) {
            i += 1;
            if i == 16 {
                return true;
            }
            p = &probs[BANDS[i]][0];
        }
        let next_ctx;
        let v: i32 = if !bd.read(p[2]) {
            next_ctx = 1;
            1
        } else {
            next_ctx = 2;
            if !bd.read(p[3]) {
                if !bd.read(p[4]) { 2 } else { 3 + bd.read(p[5]) as i32 }
            } else if !bd.read(p[6]) {
                if !bd.read(p[7]) {
                    5 + bd.read(159) as i32
                } else {
                    7 + 2 * bd.read(165) as i32 + bd.read(145) as i32
                }
            } else {
                let (cat, base): (&[u8], i32) = if !bd.read(p[8]) {
                    if !bd.read(p[9]) { (&PCAT3, 11) } else { (&PCAT4, 19) }
                } else if !bd.read(p[10]) {
                    (&PCAT5, 35)
                } else {
                    (&PCAT6, 67)
                };
                let mut e = 0;
                for &cp in cat {
                    e = (e << 1) | bd.read(cp) as i32;
                }
                base + e
            }
        };
        let v = if bd.flag() { -v } else { v };
        out[ZIGZAG[i]] = (v * if i == 0 { dc } else { ac }) as i16;
        i += 1;
        if i == 16 {
            return true;
        }
        p = &probs[BANDS[i]][next_ctx];
        if !bd.read(p[0]) {
            return true;
        }
    }
}
