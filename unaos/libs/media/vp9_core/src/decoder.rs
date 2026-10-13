// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The frame decoding process (spec §8 and the syntax of §6.4): reference management (eight slots,
//! three active references with scale factors), frame contexts (four saved, setup_past_independence,
//! refresh), tiles, the partition quadtree with its contexts, mode info, residual tokens (§6.4.24
//! with the Pareto tail and the token-cache contexts), reconstruction, the loop filter, backward
//! adaptation, segmentation maps carried frame to frame, the previous frame's motion vectors, and
//! show_existing_frame. Chunks are split at their superframe index (Annex B); as in the reference
//! decoder, a chunk presents at most one picture — its last frame, when that frame is shown.

use crate::bool_decoder::BoolDecoder;
use crate::consts::*;
use crate::frame::{FrameBuf, Mv, MvRef};
use crate::header::{self, CompressedHeader, FrameHeader, HeaderState, Segmentation};
use crate::inter::{self, BlockPos, PredArgs, ScaleFactors};
use crate::intra::{self, IntraEdge};
use crate::loopfilter::{self, LfCell};
use crate::modeinfo::*;
use crate::probs::{self, Counts, FrameContext};
use crate::tables::*;
use crate::transform;
use crate::{Error, Frame, Result};
use alloc::boxed::Box;
use alloc::sync::Arc;
use alloc::vec;
use alloc::vec::Vec;

/// A VP9 decoder: feed it chunks (a WebM block / MP4 sample / IVF frame) in decode order.
pub struct Decoder {
    hs: HeaderState,
    refs: [Option<Arc<FrameBuf>>; 8],
    contexts: Box<[FrameContext; 4]>,
    last_seg_map: Vec<u8>,
    prev: Option<Arc<FrameBuf>>,
    width: u32,
    height: u32,
    last_width: u32,
    last_height: u32,
    last_show_frame: bool,
    /// cm->frame_type == KEY_FRAME of the last decoded frame.
    last_key: bool,
    /// cm->intra_only (kept across key frames, as the reference decoder does).
    intra_only: bool,
    started: bool,
    counts: Box<Counts>,
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    pub fn new() -> Decoder {
        Decoder {
            hs: HeaderState::default(),
            refs: Default::default(),
            contexts: Box::new([FrameContext::default(), FrameContext::default(), FrameContext::default(), FrameContext::default()]),
            last_seg_map: Vec::new(),
            prev: None,
            width: 0,
            height: 0,
            last_width: 0,
            last_height: 0,
            last_show_frame: false,
            last_key: false,
            intra_only: false,
            started: false,
            counts: Box::default(),
        }
    }

    /// Forget every reference (after a seek; the next chunk must start with a key frame).
    pub fn reset(&mut self) {
        *self = Decoder::new();
    }

    /// Decode one chunk; returns the picture it presents, if any.
    pub fn decode(&mut self, chunk: &[u8]) -> Result<Option<Frame>> {
        let mut shown = None;
        for data in header::superframe_frames(chunk) {
            shown = self.decode_frame(data)?;
        }
        Ok(shown.map(|f| Frame::from_buf(&f)))
    }

    /// Decode one frame; returns the frame buffer when the frame is shown.
    pub fn decode_frame(&mut self, data: &[u8]) -> Result<Option<Arc<FrameBuf>>> {
        let last_key = self.last_key;
        let last_intra_only = self.intra_only;
        let h = header::parse_uncompressed(data, &mut self.hs)?;
        if h.show_existing_frame {
            let f = self.refs[h.frame_to_show as usize].clone().ok_or(Error::Invalid("show_existing_frame of an empty slot"))?;
            self.last_width = self.width;
            self.last_height = self.height;
            return Ok(Some(f));
        }
        if !self.started && !h.intra() {
            return Err(Error::Invalid("stream does not start with a key or intra-only frame"));
        }
        self.started = true;
        self.last_key = h.key_frame;
        if !h.key_frame {
            self.intra_only = h.intra_only;
        }
        let (bd, ssx, ssy) = (self.hs.bit_depth, self.hs.ss_x, self.hs.ss_y);

        // Active references and their scale factors.
        let mut refs: [Option<Arc<FrameBuf>>; 3] = [None, None, None];
        let mut sfs: [Option<ScaleFactors>; 3] = [None; 3];
        if !h.intra() {
            let mut any_valid = false;
            for i in 0..3 {
                let r = self.refs[h.ref_frame_idx[i] as usize].clone().ok_or(Error::Invalid("reference slot is empty"))?;
                if r.bit_depth != bd || r.ss_x != ssx || r.ss_y != ssy {
                    return Err(Error::Invalid("reference has an incompatible colour format"));
                }
                sfs[i] = ScaleFactors::new(r.width, r.height, h.width, h.height);
                any_valid |= sfs[i].is_some();
                refs[i] = Some(r);
            }
            if !any_valid {
                return Err(Error::Invalid("no reference has a valid size"));
            }
        }

        let mi_cols = h.width.div_ceil(8) as usize;
        let mi_rows = h.height.div_ceil(8) as usize;
        if h.width != self.width || h.height != self.height || self.last_seg_map.len() != mi_cols * mi_rows {
            self.last_seg_map = vec![0; mi_cols * mi_rows];
        }
        self.width = h.width;
        self.height = h.height;

        if h.past_independence {
            let d = FrameContext::default();
            if h.key_frame || h.error_resilient || h.reset_frame_context == 3 {
                for c in self.contexts.iter_mut() {
                    *c = d.clone();
                }
            } else if h.reset_frame_context == 2 {
                self.contexts[h.frame_context_idx as usize] = d;
            }
            self.last_seg_map.fill(0);
        }
        let ctx_idx = if h.past_independence { 0 } else { h.frame_context_idx as usize };
        let mut fc = self.contexts[ctx_idx].clone();
        let cstart = h.uncompressed_size;
        let cend = cstart + h.compressed_size;
        if cend > data.len() {
            return Err(Error::Truncated);
        }
        let ch = header::parse_compressed(&data[cstart..cend], &h, &mut fc)?;

        let use_prev = !h.error_resilient && h.width == self.last_width && h.height == self.last_height && !last_intra_only && self.last_show_frame && !last_key;
        let prev = if use_prev { self.prev.clone().filter(|p| p.mi_cols == mi_cols && p.mi_rows == mi_rows) } else { None };

        let mut cur = FrameBuf::new(h.width, h.height, ssx, ssy, bd);
        cur.render_width = h.render_width;
        cur.render_height = h.render_height;
        cur.color_space = self.hs.color_space;
        cur.color_range = self.hs.color_range;

        *self.counts = Counts::default();
        let seg = self.hs.seg;
        let mut cur_seg_map = if seg.enabled { vec![0u8; mi_cols * mi_rows] } else { Vec::new() };
        {
            let mut fd = FrameDecoder::new(&h, &ch, &fc, &mut self.counts, &seg, &self.last_seg_map, &mut cur_seg_map, &mut cur, &refs, &sfs, prev.as_deref());
            fd.decode_tiles(&data[cend..])?;
            if self.hs.lf.level != 0 {
                let lvl = loopfilter::levels(&self.hs.lf, &seg);
                let cells: Vec<LfCell> = fd
                    .grid
                    .iter()
                    .map(|&i| {
                        let m = fd.mis.get(i as usize).copied().unwrap_or_default();
                        LfCell {
                            level: lvl[m.segment_id as usize][m.ref_frame[0].max(0) as usize][loopfilter::mode_delta_index(m.mode)],
                            sb_type: m.sb_type,
                            tx_size: m.tx_size,
                            skip_inter: m.skip && m.is_inter(),
                        }
                    })
                    .collect();
                drop(fd);
                loopfilter::filter_frame(&mut cur, &cells, self.hs.lf.sharpness);
            }
        }

        if !h.error_resilient && !h.frame_parallel {
            let pre = &self.contexts[ctx_idx];
            probs::adapt_coef(&mut fc, pre, &self.counts, h.intra(), last_key);
            if !h.intra() {
                probs::adapt_mode(&mut fc, pre, &self.counts, h.interp_filter == SWITCHABLE, ch.tx_mode == TX_MODE_SELECT);
                probs::adapt_mv(&mut fc, pre, &self.counts, h.allow_high_precision_mv);
            }
        }
        if h.refresh_frame_context {
            self.contexts[ctx_idx] = fc;
        }

        let cur = Arc::new(cur);
        for i in 0..8 {
            if h.refresh_frame_flags & (1 << i) != 0 {
                self.refs[i] = Some(cur.clone());
                self.hs.ref_dims[i] = Some((h.width, h.height));
            }
        }
        self.last_show_frame = h.show_frame;
        self.prev = Some(cur.clone());
        if seg.enabled {
            self.last_seg_map = cur_seg_map;
        }
        self.last_width = h.width;
        self.last_height = h.height;
        Ok(if h.show_frame { Some(cur) } else { None })
    }
}

/// get_tile_offset: tile boundaries in 8x8 units, on 64x64 superblock boundaries.
fn tile_offset(idx: usize, mis: usize, log2: u8) -> usize {
    let sbs = (mis + 7) >> 3;
    (((idx * sbs) >> log2) << 3).min(mis)
}

struct FrameDecoder<'a> {
    h: &'a FrameHeader,
    ch: &'a CompressedHeader,
    fc: &'a FrameContext,
    counts: &'a mut Counts,
    seg: &'a Segmentation,
    last_seg_map: &'a [u8],
    cur_seg_map: &'a mut Vec<u8>,
    cur: &'a mut FrameBuf,
    refs: &'a [Option<Arc<FrameBuf>>; 3],
    sfs: &'a [Option<ScaleFactors>; 3],
    prev: Option<&'a FrameBuf>,
    mi_rows: usize,
    mi_cols: usize,
    mis: Vec<ModeInfo>,
    grid: Vec<u32>,
    above_ctx: [Vec<u8>; 3],
    left_ctx: [[u8; 16]; 3],
    above_part: Vec<u8>,
    left_part: [u8; 8],
    tile_col_start: usize,
    tile_col_end: usize,
    /// [segment] -> (dc, ac) for luma and chroma.
    y_dq: [[i32; 2]; 8],
    uv_dq: [[i32; 2]; 8],
    coefs: Vec<i32>,
    intra: bool,
}

impl<'a> FrameDecoder<'a> {
    fn new(
        h: &'a FrameHeader,
        ch: &'a CompressedHeader,
        fc: &'a FrameContext,
        counts: &'a mut Counts,
        seg: &'a Segmentation,
        last_seg_map: &'a [u8],
        cur_seg_map: &'a mut Vec<u8>,
        cur: &'a mut FrameBuf,
        refs: &'a [Option<Arc<FrameBuf>>; 3],
        sfs: &'a [Option<ScaleFactors>; 3],
        prev: Option<&'a FrameBuf>,
    ) -> Self {
        let mi_rows = cur.mi_rows;
        let mi_cols = cur.mi_cols;
        let aligned_cols = (mi_cols + 7) & !7;
        let bdi = match cur.bit_depth {
            10 => 1,
            12 => 2,
            _ => 0,
        };
        let q = |qi: u8, delta: i8, dc: bool| {
            let i = (qi as i32 + delta as i32).clamp(0, 255) as usize;
            if dc { DC_QLOOKUP[bdi][i] } else { AC_QLOOKUP[bdi][i] }
        };
        let mut y_dq = [[0i32; 2]; 8];
        let mut uv_dq = [[0i32; 2]; 8];
        for s in 0..8 {
            let qi = seg.qindex(s, h.base_q_idx);
            y_dq[s] = [q(qi, h.y_dc_delta_q, true), q(qi, 0, false)];
            uv_dq[s] = [q(qi, h.uv_dc_delta_q, true), q(qi, h.uv_ac_delta_q, false)];
        }
        let ssx = cur.ss_x as usize;
        FrameDecoder {
            h,
            ch,
            fc,
            counts,
            seg,
            last_seg_map,
            cur_seg_map,
            refs,
            sfs,
            prev,
            mi_rows,
            mi_cols,
            mis: Vec::with_capacity(mi_rows * mi_cols / 2 + 1),
            grid: vec![u32::MAX; mi_rows * mi_cols],
            above_ctx: [vec![0; aligned_cols * 2 + 16], vec![0; ((aligned_cols * 2) >> ssx) + 16], vec![0; ((aligned_cols * 2) >> ssx) + 16]],
            left_ctx: [[0; 16]; 3],
            above_part: vec![0; aligned_cols + 8],
            left_part: [0; 8],
            tile_col_start: 0,
            tile_col_end: 0,
            y_dq,
            uv_dq,
            coefs: vec![0; 1024],
            intra: h.intra(),
            cur,
        }
    }

    fn decode_tiles(&mut self, mut data: &[u8]) -> Result<()> {
        let cols = 1usize << self.h.tile_cols_log2;
        let rows = 1usize << self.h.tile_rows_log2;
        for tr in 0..rows {
            for tc in 0..cols {
                let last = tr == rows - 1 && tc == cols - 1;
                let size = if last {
                    data.len()
                } else {
                    if data.len() < 4 {
                        return Err(Error::Truncated);
                    }
                    let s = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
                    data = &data[4..];
                    if s > data.len() {
                        return Err(Error::Truncated);
                    }
                    s
                };
                let (tile, rest) = data.split_at(size);
                data = rest;
                let row_start = tile_offset(tr, self.mi_rows, self.h.tile_rows_log2);
                let row_end = tile_offset(tr + 1, self.mi_rows, self.h.tile_rows_log2);
                self.tile_col_start = tile_offset(tc, self.mi_cols, self.h.tile_cols_log2);
                self.tile_col_end = tile_offset(tc + 1, self.mi_cols, self.h.tile_cols_log2);
                if row_start >= row_end || self.tile_col_start >= self.tile_col_end {
                    continue;
                }
                let mut r = BoolDecoder::new(tile)?;
                for mi_row in (row_start..row_end).step_by(8) {
                    self.left_ctx = [[0; 16]; 3];
                    self.left_part = [0; 8];
                    for mi_col in (self.tile_col_start..self.tile_col_end).step_by(8) {
                        self.decode_partition(&mut r, mi_row, mi_col, BLOCK_64X64, 4)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn decode_partition(&mut self, r: &mut BoolDecoder, mi_row: usize, mi_col: usize, bsize: u8, n4x4_l2: u32) -> Result<()> {
        if mi_row >= self.mi_rows || mi_col >= self.mi_cols {
            return Ok(());
        }
        let n8x8_l2 = n4x4_l2 - 1;
        let num_8x8 = 1usize << n8x8_l2;
        let hbs = num_8x8 >> 1;
        let has_rows = mi_row + hbs < self.mi_rows;
        let has_cols = mi_col + hbs < self.mi_cols;
        let above = (self.above_part[mi_col] >> n8x8_l2) & 1;
        let left = (self.left_part[mi_row & 7] >> n8x8_l2) & 1;
        let ctx = (left * 2 + above) as usize + n8x8_l2 as usize * 4;
        let probs = if self.intra { &KF_PARTITION_PROBS[ctx] } else { &self.fc.partition[ctx] };
        let p = if has_rows && has_cols {
            r.tree(&PARTITION_TREE, probs) as u8
        } else if !has_rows && has_cols {
            if r.read(probs[1]) { PARTITION_SPLIT } else { PARTITION_HORZ }
        } else if has_rows && !has_cols {
            if r.read(probs[2]) { PARTITION_SPLIT } else { PARTITION_VERT }
        } else {
            PARTITION_SPLIT
        };
        self.counts.partition[ctx][p as usize] += 1;
        let subsize = SUBSIZE_LOOKUP[p as usize][bsize as usize];
        if subsize == BLOCK_INVALID {
            return Err(Error::Invalid("partition"));
        }
        if hbs == 0 {
            self.decode_block(r, mi_row, mi_col, subsize, 1, 1)?;
        } else {
            match p {
                PARTITION_NONE => self.decode_block(r, mi_row, mi_col, subsize, n4x4_l2, n4x4_l2)?,
                PARTITION_HORZ => {
                    self.decode_block(r, mi_row, mi_col, subsize, n4x4_l2, n8x8_l2)?;
                    if has_rows {
                        self.decode_block(r, mi_row + hbs, mi_col, subsize, n4x4_l2, n8x8_l2)?;
                    }
                }
                PARTITION_VERT => {
                    self.decode_block(r, mi_row, mi_col, subsize, n8x8_l2, n4x4_l2)?;
                    if has_cols {
                        self.decode_block(r, mi_row, mi_col + hbs, subsize, n8x8_l2, n4x4_l2)?;
                    }
                }
                _ => {
                    self.decode_partition(r, mi_row, mi_col, subsize, n8x8_l2)?;
                    self.decode_partition(r, mi_row, mi_col + hbs, subsize, n8x8_l2)?;
                    self.decode_partition(r, mi_row + hbs, mi_col, subsize, n8x8_l2)?;
                    self.decode_partition(r, mi_row + hbs, mi_col + hbs, subsize, n8x8_l2)?;
                }
            }
        }
        if bsize == BLOCK_8X8 || p != PARTITION_SPLIT {
            let a = PARTITION_CONTEXT_ABOVE[subsize as usize];
            let l = PARTITION_CONTEXT_LEFT[subsize as usize];
            for i in 0..num_8x8 {
                self.above_part[mi_col + i] = a;
                self.left_part[(mi_row + i) & 7] = l;
            }
        }
        Ok(())
    }

    fn neighbour(&self, row: isize, col: isize) -> Option<usize> {
        let i = self.grid[row as usize * self.mi_cols + col as usize];
        if i == u32::MAX { None } else { Some(i as usize) }
    }

    fn decode_block(&mut self, r: &mut BoolDecoder, mi_row: usize, mi_col: usize, bsize: u8, bwl: u32, bhl: u32) -> Result<()> {
        let bw = 1usize << (bwl - 1);
        let bh = 1usize << (bhl - 1);
        let x_mis = bw.min(self.mi_cols - mi_col);
        let y_mis = bh.min(self.mi_rows - mi_row);
        if bsize >= BLOCK_8X8 && SS_SIZE_LOOKUP[bsize as usize][self.cur.ss_x as usize][self.cur.ss_y as usize] == BLOCK_INVALID {
            return Err(Error::Invalid("block size invalid for the subsampling"));
        }
        let pos = BlockPos {
            mi_row,
            mi_col,
            to_top: -((mi_row * 64) as i32),
            to_bottom: (self.mi_rows as i32 - bh as i32 - mi_row as i32) * 64,
            to_left: -((mi_col * 64) as i32),
            to_right: (self.mi_cols as i32 - bw as i32 - mi_col as i32) * 64,
        };
        let above = if mi_row > 0 { self.neighbour(mi_row as isize - 1, mi_col as isize) } else { None };
        let left = if mi_col > self.tile_col_start { self.neighbour(mi_row as isize, mi_col as isize - 1) } else { None };

        let mut mi = ModeInfo { sb_type: bsize, ..ModeInfo::default() };
        if self.intra {
            self.intra_frame_mode_info(r, &mut mi, above, left, mi_row, mi_col, x_mis, y_mis);
        } else {
            self.inter_frame_mode_info(r, &mut mi, above, left, &pos, x_mis, y_mis)?;
            let mvref = MvRef { ref_frame: mi.ref_frame, mv: mi.mv };
            for y in 0..y_mis {
                for x in 0..x_mis {
                    self.cur.mvs[(mi_row + y) * self.mi_cols + mi_col + x] = mvref;
                }
            }
        }
        let idx = self.mis.len() as u32;
        self.mis.push(mi);
        for y in 0..y_mis {
            for x in 0..x_mis {
                self.grid[(mi_row + y) * self.mi_cols + mi_col + x] = idx;
            }
        }

        let (ssx, ssy) = (self.cur.ss_x as usize, self.cur.ss_y as usize);
        if mi.skip {
            // dec_reset_skip_context over the whole block.
            for p in 0..3 {
                let (sx, sy) = if p == 0 { (0, 0) } else { (ssx, ssy) };
                let n4w = (bw << 1) >> sx;
                let n4h = (bh << 1) >> sy;
                let ax = (mi_col * 2) >> sx;
                let ly = ((mi_row * 2) & 15) >> sy;
                self.above_ctx[p][ax..ax + n4w].fill(0);
                for i in 0..n4h {
                    self.left_ctx[p][(ly + i) & 15] = 0;
                }
            }
        }

        if !mi.is_inter() {
            for p in 0..3 {
                let (sx, sy) = if p == 0 { (0, 0) } else { (ssx, ssy) };
                let tx = if p == 0 { mi.tx_size } else { UV_TXSIZE_LOOKUP[bsize as usize][mi.tx_size as usize][sx][sy] };
                let n4w = ((bw << 1) >> sx) as i32;
                let n4h = ((bh << 1) >> sy) as i32;
                let step = 1i32 << tx;
                let max_w = n4w + if pos.to_right >= 0 { 0 } else { pos.to_right >> (5 + sx) };
                let max_h = n4h + if pos.to_bottom >= 0 { 0 } else { pos.to_bottom >> (5 + sy) };
                let lim_w = if pos.to_right >= 0 { None } else { Some(max_w) };
                let lim_h = if pos.to_bottom >= 0 { None } else { Some(max_h) };
                let n4_wl = bwl as i32 - sx as i32;
                let mut row = 0;
                while row < max_h {
                    let mut col = 0;
                    while col < max_w {
                        let mut mode = if p == 0 { mi.mode } else { mi.uv_mode };
                        if bsize < BLOCK_8X8 && p == 0 {
                            mode = mi.sub_modes[((row << 1) + col) as usize];
                        }
                        let x0 = ((mi_col * 8) >> sx) + col as usize * 4;
                        let y0 = ((mi_row * 8) >> sy) + row as usize * 4;
                        let (fw, fh) = self.cur.aligned_size(p);
                        let e = IntraEdge {
                            x0,
                            y0,
                            have_top: row > 0 || above.is_some(),
                            have_left: col > 0 || left.is_some(),
                            have_right: (col + step) < (1 << n4_wl),
                            frame_w: fw,
                            frame_h: fh,
                            right_overhang: pos.to_right < 0,
                            bottom_overhang: pos.to_bottom < 0,
                        };
                        let bd = self.cur.bit_depth;
                        let plane = &mut self.cur.planes[p];
                        intra::predict(&mut plane.data, plane.stride, &e, tx, mode, bd);
                        if !mi.skip {
                            let tx_type = if p > 0 || self.h.lossless || tx == TX_32X32 { DCT_DCT } else { MODE_TO_TX_TYPE[mode as usize] };
                            let eob = self.block_tokens(r, p, mi_row, mi_col, col, row, tx, tx_type, false, mi.segment_id, lim_w, lim_h);
                            if eob > 0 {
                                self.reconstruct(p, x0, y0, tx, tx_type);
                            }
                        }
                        col += step;
                    }
                    row += step;
                }
            }
        } else {
            self.predict_inter(&mi, &pos, bw, bh)?;
            if !mi.skip {
                let mut eobtotal = 0;
                for p in 0..3 {
                    let (sx, sy) = if p == 0 { (0, 0) } else { (ssx, ssy) };
                    let tx = if p == 0 { mi.tx_size } else { UV_TXSIZE_LOOKUP[bsize as usize][mi.tx_size as usize][sx][sy] };
                    let n4w = ((bw << 1) >> sx) as i32;
                    let n4h = ((bh << 1) >> sy) as i32;
                    let step = 1i32 << tx;
                    let max_w = n4w + if pos.to_right >= 0 { 0 } else { pos.to_right >> (5 + sx) };
                    let max_h = n4h + if pos.to_bottom >= 0 { 0 } else { pos.to_bottom >> (5 + sy) };
                    let lim_w = if pos.to_right >= 0 { None } else { Some(max_w) };
                    let lim_h = if pos.to_bottom >= 0 { None } else { Some(max_h) };
                    let mut row = 0;
                    while row < max_h {
                        let mut col = 0;
                        while col < max_w {
                            let eob = self.block_tokens(r, p, mi_row, mi_col, col, row, tx, DCT_DCT, true, mi.segment_id, lim_w, lim_h);
                            if eob > 0 {
                                let x0 = ((mi_col * 8) >> sx) + col as usize * 4;
                                let y0 = ((mi_row * 8) >> sy) + row as usize * 4;
                                self.reconstruct(p, x0, y0, tx, DCT_DCT);
                            }
                            eobtotal += eob;
                            col += step;
                        }
                        row += step;
                    }
                }
                if bsize >= BLOCK_8X8 && eobtotal == 0 {
                    // The reference decoder marks the block skipped (for the loop filter and the
                    // neighbours' skip contexts).
                    self.mis[idx as usize].skip = true;
                }
            }
        }
        Ok(())
    }

    fn reconstruct(&mut self, p: usize, x0: usize, y0: usize, tx: u8, tx_type: u8) {
        let n = 4usize << tx;
        let bd = self.cur.bit_depth;
        let plane = &mut self.cur.planes[p];
        let off = y0 * plane.stride + x0;
        transform::inverse_transform_add(&self.coefs[..n * n], tx, tx_type, self.h.lossless, &mut plane.data, off, plane.stride, bd);
        self.coefs[..n * n].fill(0);
    }

    // ---- mode info ---------------------------------------------------------------------------

    fn read_segment_tree(&self, r: &mut BoolDecoder) -> u8 {
        r.tree(&SEGMENT_TREE, &self.seg.tree_probs) as u8
    }

    fn set_seg_map(&mut self, mi_row: usize, mi_col: usize, x_mis: usize, y_mis: usize, v: Option<u8>) {
        for y in 0..y_mis {
            for x in 0..x_mis {
                let i = (mi_row + y) * self.mi_cols + mi_col + x;
                self.cur_seg_map[i] = v.unwrap_or_else(|| self.last_seg_map.get(i).copied().unwrap_or(0));
            }
        }
    }

    fn read_skip(&mut self, r: &mut BoolDecoder, seg_id: u8, nb: &Neighbours) -> bool {
        if self.seg.active(seg_id as usize, SEG_LVL_SKIP) {
            return true;
        }
        let ctx = nb.skip_ctx();
        let s = r.read(self.fc.skip[ctx]);
        self.counts.skip[ctx][s as usize] += 1;
        s
    }

    fn read_tx_size(&mut self, r: &mut BoolDecoder, bsize: u8, allow_select: bool, nb: &Neighbours) -> u8 {
        let max_tx = MAX_TXSIZE[bsize as usize];
        if allow_select && self.ch.tx_mode == TX_MODE_SELECT && bsize >= BLOCK_8X8 {
            let ctx = nb.tx_ctx(max_tx);
            let probs: &[u8] = match max_tx {
                TX_8X8 => &self.fc.tx8[ctx],
                TX_16X16 => &self.fc.tx16[ctx],
                _ => &self.fc.tx32[ctx],
            };
            let mut tx = r.read(probs[0]) as u8;
            if tx != TX_4X4 && max_tx >= TX_16X16 {
                tx += r.read(probs[1]) as u8;
                if tx != TX_8X8 && max_tx >= TX_32X32 {
                    tx += r.read(probs[2]) as u8;
                }
            }
            match max_tx {
                TX_8X8 => self.counts.tx8[ctx][tx as usize] += 1,
                TX_16X16 => self.counts.tx16[ctx][tx as usize] += 1,
                _ => self.counts.tx32[ctx][tx as usize] += 1,
            }
            tx
        } else {
            max_tx.min(TX_MODE_TO_BIGGEST_TX_SIZE[self.ch.tx_mode as usize])
        }
    }

    fn intra_frame_mode_info(&mut self, r: &mut BoolDecoder, mi: &mut ModeInfo, above: Option<usize>, left: Option<usize>, mi_row: usize, mi_col: usize, x_mis: usize, y_mis: usize) {
        // read_intra_segment_id
        mi.segment_id = if !self.seg.enabled {
            0
        } else if !self.seg.update_map {
            self.set_seg_map(mi_row, mi_col, x_mis, y_mis, None);
            0
        } else {
            let s = self.read_segment_tree(r);
            self.set_seg_map(mi_row, mi_col, x_mis, y_mis, Some(s));
            s
        };
        let a = above.map(|i| self.mis[i]);
        let l = left.map(|i| self.mis[i]);
        let nb = Neighbours { above: a.as_ref(), left: l.as_ref() };
        mi.skip = self.read_skip(r, mi.segment_id, &nb);
        mi.tx_size = self.read_tx_size(r, mi.sb_type, true, &nb);
        mi.ref_frame = [INTRA_FRAME, NONE_FRAME];
        let read_b = |r: &mut BoolDecoder, mi: &ModeInfo, b: usize| -> u8 {
            let am = above_block_mode(mi, a.as_ref(), b);
            let lm = left_block_mode(mi, l.as_ref(), b);
            r.tree(&INTRA_MODE_TREE, &KF_Y_MODE_PROB[am as usize][lm as usize]) as u8
        };
        match mi.sb_type {
            BLOCK_4X4 => {
                for b in 0..4 {
                    let m = read_b(r, mi, b);
                    mi.sub_modes[b] = m;
                }
                mi.mode = mi.sub_modes[3];
            }
            BLOCK_4X8 => {
                let m0 = read_b(r, mi, 0);
                mi.sub_modes[0] = m0;
                mi.sub_modes[2] = m0;
                let m1 = read_b(r, mi, 1);
                mi.sub_modes[1] = m1;
                mi.sub_modes[3] = m1;
                mi.mode = m1;
            }
            BLOCK_8X4 => {
                let m0 = read_b(r, mi, 0);
                mi.sub_modes[0] = m0;
                mi.sub_modes[1] = m0;
                let m2 = read_b(r, mi, 2);
                mi.sub_modes[2] = m2;
                mi.sub_modes[3] = m2;
                mi.mode = m2;
            }
            _ => {
                let m = read_b(r, mi, 0);
                mi.mode = m;
                mi.sub_modes = [m; 4];
            }
        }
        mi.uv_mode = r.tree(&INTRA_MODE_TREE, &KF_UV_MODE_PROB[mi.mode as usize]) as u8;
    }

    fn inter_frame_mode_info(&mut self, r: &mut BoolDecoder, mi: &mut ModeInfo, above: Option<usize>, left: Option<usize>, pos: &BlockPos, x_mis: usize, y_mis: usize) -> Result<()> {
        let (mi_row, mi_col) = (pos.mi_row, pos.mi_col);
        let a = above.map(|i| self.mis[i]);
        let l = left.map(|i| self.mis[i]);
        let nb = Neighbours { above: a.as_ref(), left: l.as_ref() };
        // read_inter_segment_id
        if self.seg.enabled {
            let mut predicted = u8::MAX;
            for y in 0..y_mis {
                for x in 0..x_mis {
                    let i = (mi_row + y) * self.mi_cols + mi_col + x;
                    predicted = predicted.min(self.last_seg_map.get(i).copied().unwrap_or(0));
                }
            }
            if !self.seg.update_map {
                self.set_seg_map(mi_row, mi_col, x_mis, y_mis, None);
                mi.segment_id = predicted;
            } else {
                let s = if self.seg.temporal_update {
                    let p = self.seg.pred_probs[nb.seg_pred_ctx()];
                    mi.seg_id_predicted = r.read(p);
                    if mi.seg_id_predicted { predicted } else { self.read_segment_tree(r) }
                } else {
                    self.read_segment_tree(r)
                };
                self.set_seg_map(mi_row, mi_col, x_mis, y_mis, Some(s));
                mi.segment_id = s;
            }
        }
        mi.skip = self.read_skip(r, mi.segment_id, &nb);
        let seg_id = mi.segment_id as usize;
        let is_inter = if self.seg.active(seg_id, SEG_LVL_REF_FRAME) {
            self.seg.feature_data[seg_id][SEG_LVL_REF_FRAME] != INTRA_FRAME as i16
        } else {
            let ctx = nb.intra_inter_ctx();
            let v = r.read(self.fc.is_inter[ctx]);
            self.counts.is_inter[ctx][v as usize] += 1;
            v
        };
        mi.tx_size = self.read_tx_size(r, mi.sb_type, !mi.skip || !is_inter, &nb);
        if !is_inter {
            // read_intra_block_mode_info
            let read_y = |r: &mut BoolDecoder, group: usize, fc: &FrameContext, counts: &mut Counts| {
                let m = r.tree(&INTRA_MODE_TREE, &fc.y_mode[group]);
                counts.y_mode[group][m] += 1;
                m as u8
            };
            match mi.sb_type {
                BLOCK_4X4 => {
                    for b in 0..4 {
                        mi.sub_modes[b] = read_y(r, 0, self.fc, self.counts);
                    }
                    mi.mode = mi.sub_modes[3];
                }
                BLOCK_4X8 => {
                    let m0 = read_y(r, 0, self.fc, self.counts);
                    let m1 = read_y(r, 0, self.fc, self.counts);
                    mi.sub_modes = [m0, m1, m0, m1];
                    mi.mode = m1;
                }
                BLOCK_8X4 => {
                    let m0 = read_y(r, 0, self.fc, self.counts);
                    let m2 = read_y(r, 0, self.fc, self.counts);
                    mi.sub_modes = [m0, m0, m2, m2];
                    mi.mode = m2;
                }
                _ => {
                    let m = read_y(r, SIZE_GROUP[mi.sb_type as usize] as usize, self.fc, self.counts);
                    mi.mode = m;
                    mi.sub_modes = [m; 4];
                }
            }
            let uv = r.tree(&INTRA_MODE_TREE, &self.fc.uv_mode[mi.mode as usize]);
            self.counts.uv_mode[mi.mode as usize][uv] += 1;
            mi.uv_mode = uv as u8;
            mi.interp_filter = 3;
            mi.ref_frame = [INTRA_FRAME, NONE_FRAME];
            return Ok(());
        }
        self.read_ref_frames(r, mi, &nb);
        let is_compound = mi.has_second();
        let prev = self.prev.map(|p| p.mvs[mi_row * self.mi_cols + mi_col]);
        let search = MvSearch {
            mis: &self.mis,
            grid: &self.grid,
            mi_rows: self.mi_rows,
            mi_cols: self.mi_cols,
            tile_col_start: self.tile_col_start,
            tile_col_end: self.tile_col_end,
            mi_row,
            mi_col,
            bsize: mi.sb_type,
            prev,
            sign_bias: self.h.sign_bias,
            to_left: pos.to_left,
            to_right: pos.to_right,
            to_top: pos.to_top,
            to_bottom: pos.to_bottom,
        };
        let mode_ctx = search.mode_context();
        let bsize = mi.sb_type;
        let read_inter_mode = |r: &mut BoolDecoder, fc: &FrameContext, counts: &mut Counts| -> u8 {
            let m = r.tree(&INTER_MODE_TREE, &fc.inter_mode[mode_ctx]);
            counts.inter_mode[mode_ctx][m] += 1;
            NEARESTMV + m as u8
        };
        if self.seg.active(seg_id, SEG_LVL_SKIP) {
            mi.mode = ZEROMV;
            if bsize < BLOCK_8X8 {
                return Err(Error::Invalid("segment skip on a block smaller than 8x8"));
            }
        } else if bsize >= BLOCK_8X8 {
            mi.mode = read_inter_mode(r, self.fc, self.counts);
        }
        mi.interp_filter = if self.h.interp_filter == SWITCHABLE {
            let ctx = nb.interp_ctx();
            let f = r.tree(&SWITCHABLE_INTERP_TREE, &self.fc.interp_filter[ctx]);
            self.counts.interp_filter[ctx][f] += 1;
            f as u8
        } else {
            self.h.interp_filter
        };
        let allow_hp = self.h.allow_high_precision_mv;
        let mut best_ref = [Mv::ZERO; 2];
        if bsize < BLOCK_8X8 {
            let n4w = NUM_4X4_W[bsize as usize] as usize;
            let n4h = NUM_4X4_H[bsize as usize] as usize;
            let mut got_new = false;
            let mut best_sub = [Mv::ZERO; 2];
            let mut b_mode = ZEROMV;
            let mut idy = 0;
            while idy < 2 {
                let mut idx = 0;
                while idx < 2 {
                    let j = idy * 2 + idx;
                    b_mode = read_inter_mode(r, self.fc, self.counts);
                    if b_mode == NEARESTMV || b_mode == NEARMV {
                        for rf in 0..1 + is_compound as usize {
                            best_sub[rf] = append_sub8x8(&search, mi, b_mode, j, rf);
                        }
                    } else if b_mode == NEWMV && !got_new {
                        for rf in 0..1 + is_compound as usize {
                            let (list, _) = search.find_mv_refs(NEWMV, mi.ref_frame[rf], -1);
                            best_ref[rf] = lower_mv_precision(list[0], allow_hp);
                        }
                        got_new = true;
                    }
                    let mut mvs = [Mv::ZERO; 2];
                    match b_mode {
                        NEWMV => {
                            for rf in 0..1 + is_compound as usize {
                                mvs[rf] = read_mv(r, best_ref[rf], &self.fc.mv, &mut self.counts.mv, allow_hp)?;
                            }
                        }
                        NEARESTMV | NEARMV => mvs = best_sub,
                        _ => {}
                    }
                    mi.sub_mvs[j] = mvs;
                    mi.sub_modes[j] = b_mode;
                    if n4h == 2 {
                        mi.sub_mvs[j + 2] = mvs;
                        mi.sub_modes[j + 2] = b_mode;
                    }
                    if n4w == 2 {
                        mi.sub_mvs[j + 1] = mvs;
                        mi.sub_modes[j + 1] = b_mode;
                    }
                    idx += n4w;
                }
                idy += n4h;
            }
            mi.mode = b_mode;
            mi.mv = mi.sub_mvs[3];
        } else {
            if mi.mode != ZEROMV {
                for rf in 0..1 + is_compound as usize {
                    let (list, count) = search.find_mv_refs(mi.mode, mi.ref_frame[rf], -1);
                    best_ref[rf] = lower_mv_precision(list[count - 1], allow_hp);
                }
            }
            match mi.mode {
                NEWMV => {
                    for rf in 0..1 + is_compound as usize {
                        mi.mv[rf] = read_mv(r, best_ref[rf], &self.fc.mv, &mut self.counts.mv, allow_hp)?;
                    }
                }
                NEARESTMV | NEARMV => mi.mv = best_ref,
                _ => mi.mv = [Mv::ZERO; 2],
            }
            mi.sub_mvs = [mi.mv; 4];
        }
        if !is_compound {
            mi.mv[1] = Mv::ZERO;
        }
        Ok(())
    }

    fn read_ref_frames(&mut self, r: &mut BoolDecoder, mi: &mut ModeInfo, nb: &Neighbours) {
        let seg_id = mi.segment_id as usize;
        if self.seg.active(seg_id, SEG_LVL_REF_FRAME) {
            mi.ref_frame = [self.seg.feature_data[seg_id][SEG_LVL_REF_FRAME] as i8, NONE_FRAME];
            return;
        }
        let mode = if self.ch.reference_mode == REFERENCE_MODE_SELECT {
            let ctx = nb.comp_mode_ctx(self.ch.comp_fixed_ref);
            let v = r.read(self.fc.comp_mode[ctx]);
            self.counts.comp_mode[ctx][v as usize] += 1;
            if v { COMPOUND_REFERENCE } else { SINGLE_REFERENCE }
        } else {
            self.ch.reference_mode
        };
        if mode == COMPOUND_REFERENCE {
            let fixed = self.ch.comp_fixed_ref;
            let idx = self.h.sign_bias[fixed as usize] as usize;
            let ctx = nb.comp_ref_ctx(fixed, self.ch.comp_var_ref, &self.h.sign_bias);
            let bit = r.read(self.fc.comp_ref[ctx]);
            self.counts.comp_ref[ctx][bit as usize] += 1;
            mi.ref_frame[idx] = fixed;
            mi.ref_frame[1 - idx] = self.ch.comp_var_ref[bit as usize];
        } else {
            let ctx0 = nb.single_ref_p1_ctx();
            let b0 = r.read(self.fc.single_ref[ctx0][0]);
            self.counts.single_ref[ctx0][0][b0 as usize] += 1;
            mi.ref_frame[0] = if b0 {
                let ctx1 = nb.single_ref_p2_ctx();
                let b1 = r.read(self.fc.single_ref[ctx1][1]);
                self.counts.single_ref[ctx1][1][b1 as usize] += 1;
                if b1 { ALTREF_FRAME } else { GOLDEN_FRAME }
            } else {
                LAST_FRAME
            };
            mi.ref_frame[1] = NONE_FRAME;
        }
    }

    // ---- residual ----------------------------------------------------------------------------

    /// vp9_decode_block_tokens: one transform block's tokens into `self.coefs`, with the above /
    /// left non-zero contexts read and written (cleared past the frame edge).
    fn block_tokens(&mut self, r: &mut BoolDecoder, p: usize, mi_row: usize, mi_col: usize, col: i32, row: i32, tx: u8, tx_type: u8, is_inter: bool, seg_id: u8, lim_w: Option<i32>, lim_h: Option<i32>) -> usize {
        let (sx, sy) = if p == 0 { (0, 0) } else { (self.cur.ss_x as usize, self.cur.ss_y as usize) };
        let ax = ((mi_col * 2) >> sx) + col as usize;
        let ly = (((mi_row * 2) & 15) >> sy) + row as usize;
        let n = 1usize << tx;
        let a_nz = self.above_ctx[p][ax..ax + n].iter().any(|&v| v != 0);
        let l_nz = (0..n).any(|i| self.left_ctx[p][(ly + i) & 15] != 0);
        let ctx = a_nz as usize + l_nz as usize;
        let dq = if p == 0 { self.y_dq[seg_id as usize] } else { self.uv_dq[seg_id as usize] };
        let eob = self.decode_coefs(r, (p > 0) as usize, is_inter as usize, tx, tx_type, dq, ctx);
        let nz = (eob > 0) as u8;
        for i in 0..n {
            let keep_a = lim_w.map_or(true, |m| col + (i as i32) < m) || n == 1;
            self.above_ctx[p][ax + i] = if keep_a { nz } else { 0 };
            let keep_l = lim_h.map_or(true, |m| row + (i as i32) < m) || n == 1;
            self.left_ctx[p][(ly + i) & 15] = if keep_l { nz } else { 0 };
        }
        eob
    }

    fn decode_coefs(&mut self, r: &mut BoolDecoder, plane_type: usize, is_inter: usize, tx: u8, tx_type: u8, dq: [i32; 2], mut ctx: usize) -> usize {
        let max_eob = 16usize << (tx << 1);
        let (scan, nb): (&[u16], &[u16]) = match (tx, tx_type) {
            (TX_4X4, ADST_DCT) => (&ROW_SCAN_4X4, &ROW_SCAN_4X4_NEIGHBORS),
            (TX_4X4, DCT_ADST) => (&COL_SCAN_4X4, &COL_SCAN_4X4_NEIGHBORS),
            (TX_4X4, _) => (&DEFAULT_SCAN_4X4, &DEFAULT_SCAN_4X4_NEIGHBORS),
            (TX_8X8, ADST_DCT) => (&ROW_SCAN_8X8, &ROW_SCAN_8X8_NEIGHBORS),
            (TX_8X8, DCT_ADST) => (&COL_SCAN_8X8, &COL_SCAN_8X8_NEIGHBORS),
            (TX_8X8, _) => (&DEFAULT_SCAN_8X8, &DEFAULT_SCAN_8X8_NEIGHBORS),
            (TX_16X16, ADST_DCT) => (&ROW_SCAN_16X16, &ROW_SCAN_16X16_NEIGHBORS),
            (TX_16X16, DCT_ADST) => (&COL_SCAN_16X16, &COL_SCAN_16X16_NEIGHBORS),
            (TX_16X16, _) => (&DEFAULT_SCAN_16X16, &DEFAULT_SCAN_16X16_NEIGHBORS),
            _ => (&DEFAULT_SCAN_32X32, &DEFAULT_SCAN_32X32_NEIGHBORS),
        };
        let band_of: &[u8] = if tx == TX_4X4 { &COEFBAND_4X4 } else { &COEFBAND_8X8PLUS };
        let dq_shift = (tx == TX_32X32) as u32;
        let bd = self.cur.bit_depth;
        let (cat6, cat6_bits): (&[u8], usize) = match bd {
            12 => (&CAT6_PROB[..], 18),
            10 => (&CAT6_PROB[2..], 16),
            _ => (&CAT6_PROB[4..], 14),
        };
        let probs = &self.fc.coef[tx as usize][plane_type][is_inter];
        let mut token_cache = [0u8; 1024];
        let mut c = 0usize;
        let mut dqv = dq[0];
        let read_coeff = |r: &mut BoolDecoder, p: &[u8], n: usize| -> i32 {
            let mut v = 0i32;
            for i in 0..n {
                v = (v << 1) | r.read(p[i]) as i32;
            }
            v
        };
        while c < max_eob {
            let mut band = band_of[c] as usize;
            let mut prob = &probs[band][ctx];
            self.counts.eob_branch[tx as usize][plane_type][is_inter][band][ctx] += 1;
            if !r.read(prob[0]) {
                self.counts.coef[tx as usize][plane_type][is_inter][band][ctx][3] += 1;
                break;
            }
            while !r.read(prob[1]) {
                self.counts.coef[tx as usize][plane_type][is_inter][band][ctx][0] += 1;
                dqv = dq[1];
                token_cache[scan[c] as usize] = 0;
                c += 1;
                if c >= max_eob {
                    return c;
                }
                ctx = (1 + token_cache[nb[2 * c] as usize] as usize + token_cache[nb[2 * c + 1] as usize] as usize) >> 1;
                band = band_of[c] as usize;
                prob = &probs[band][ctx];
            }
            let v: i64;
            if !r.read(prob[2]) {
                self.counts.coef[tx as usize][plane_type][is_inter][band][ctx][1] += 1;
                token_cache[scan[c] as usize] = 1;
                v = (dqv as i64) >> dq_shift;
            } else {
                self.counts.coef[tx as usize][plane_type][is_inter][band][ctx][2] += 1;
                let p = &PARETO8_FULL[prob[2] as usize - 1];
                if r.read(p[0]) {
                    let val;
                    if r.read(p[3]) {
                        token_cache[scan[c] as usize] = 5;
                        if r.read(p[5]) {
                            if r.read(p[7]) {
                                val = 67 + read_coeff(r, cat6, cat6_bits);
                            } else {
                                val = 35 + read_coeff(r, &[180, 157, 141, 134, 130], 5);
                            }
                        } else if r.read(p[6]) {
                            val = 19 + read_coeff(r, &[176, 155, 140, 135], 4);
                        } else {
                            val = 11 + read_coeff(r, &[173, 148, 140], 3);
                        }
                    } else {
                        token_cache[scan[c] as usize] = 4;
                        if r.read(p[4]) {
                            val = 7 + read_coeff(r, &[165, 145], 2);
                        } else {
                            val = 5 + read_coeff(r, &[159], 1);
                        }
                    }
                    v = (val as i64 * dqv as i64) >> dq_shift;
                } else if r.read(p[1]) {
                    token_cache[scan[c] as usize] = 3;
                    v = ((3 + r.read(p[2]) as i64) * dqv as i64) >> dq_shift;
                } else {
                    token_cache[scan[c] as usize] = 2;
                    v = (2 * dqv as i64) >> dq_shift;
                }
            }
            let neg = r.read(128);
            self.coefs[scan[c] as usize] = if neg { -v } else { v } as i32;
            c += 1;
            if c < max_eob {
                ctx = (1 + token_cache[nb[2 * c] as usize] as usize + token_cache[nb[2 * c + 1] as usize] as usize) >> 1;
            }
            dqv = dq[1];
        }
        c
    }

    // ---- inter prediction --------------------------------------------------------------------

    fn predict_inter(&mut self, mi: &ModeInfo, pos: &BlockPos, bw: usize, bh: usize) -> Result<()> {
        let (ssx, ssy) = (self.cur.ss_x as u32, self.cur.ss_y as u32);
        let bd = self.cur.bit_depth;
        let (refs, sfs) = (self.refs, self.sfs);
        for rf in 0..1 + mi.has_second() as usize {
            let slot = (mi.ref_frame[rf] - 1) as usize;
            let reference = refs[slot].as_deref().ok_or(Error::Invalid("inter block without a reference"))?;
            let sf = sfs[slot].as_ref().ok_or(Error::Invalid("reference frame has invalid dimensions"))?;
            for p in 0..3 {
                let (sx, sy) = if p == 0 { (0, 0) } else { (ssx, ssy) };
                let n4w = ((bw << 1) >> sx) as i32;
                let n4h = ((bh << 1) >> sy) as i32;
                let plane = &mut self.cur.planes[p];
                let stride = plane.stride;
                if mi.sb_type < BLOCK_8X8 {
                    let mut i = 0;
                    for y in 0..n4h {
                        for x in 0..n4w {
                            let mv = average_split_mv(mi, rf, i, sx, sy);
                            i += 1;
                            let a = PredArgs { plane: p, ss_x: sx, ss_y: sy, bw: 4 * n4w, bh: 4 * n4h, x: 4 * x, y: 4 * y, w: 4, h: 4, mv, filter: mi.interp_filter, sf, reference, avg: rf == 1, bit_depth: bd };
                            inter::predict(&mut plane.data, stride, pos, &a);
                        }
                    }
                } else {
                    let a = PredArgs { plane: p, ss_x: sx, ss_y: sy, bw: 4 * n4w, bh: 4 * n4h, x: 0, y: 0, w: 4 * n4w, h: 4 * n4h, mv: mi.mv[rf], filter: mi.interp_filter, sf, reference, avg: rf == 1, bit_depth: bd };
                    inter::predict(&mut plane.data, stride, pos, &a);
                }
            }
        }
        Ok(())
    }
}

/// average_split_mvs: a chroma sub-block's vector from the luma sub-blocks it covers.
fn average_split_mv(mi: &ModeInfo, rf: usize, block: usize, ssx: u32, ssy: u32) -> Mv {
    let m = |b: usize| mi.sub_mvs[b][rf];
    let q4 = |v: i32| if v < 0 { (v - 2) / 4 } else { (v + 2) / 4 };
    let q2 = |v: i32| if v < 0 { (v - 1) / 2 } else { (v + 1) / 2 };
    match (ssx, ssy) {
        (0, 0) => m(block),
        (0, 1) => {
            let (a, b) = (m(block), m(block + 2));
            Mv::new(q2(a.row as i32 + b.row as i32), q2(a.col as i32 + b.col as i32))
        }
        (1, 0) => {
            let (a, b) = (m(block), m(block + 1));
            Mv::new(q2(a.row as i32 + b.row as i32), q2(a.col as i32 + b.col as i32))
        }
        _ => {
            let s = |f: fn(Mv) -> i32| f(m(0)) + f(m(1)) + f(m(2)) + f(m(3));
            Mv::new(q4(s(|v| v.row as i32)), q4(s(|v| v.col as i32)))
        }
    }
}

/// append_sub8x8_mvs_for_idx: NEARESTMV / NEARMV for sub-block `block` of reference `rf`.
fn append_sub8x8(search: &MvSearch, mi: &ModeInfo, b_mode: u8, block: usize, rf: usize) -> Mv {
    let ref_frame = mi.ref_frame[rf];
    let sub = |b: usize| mi.sub_mvs[b][rf];
    match block {
        0 => {
            let (list, count) = search.find_mv_refs(b_mode, ref_frame, 0);
            list[count - 1]
        }
        1 | 2 => {
            if b_mode == NEARESTMV {
                sub(0)
            } else {
                let (list, _) = search.find_mv_refs(b_mode, ref_frame, block as i32);
                for n in 0..2 {
                    if sub(0) != list[n] {
                        return list[n];
                    }
                }
                Mv::ZERO
            }
        }
        _ => {
            if b_mode == NEARESTMV {
                sub(2)
            } else {
                if sub(2) != sub(1) {
                    return sub(1);
                }
                if sub(2) != sub(0) {
                    return sub(0);
                }
                let (list, _) = search.find_mv_refs(b_mode, ref_frame, 3);
                for n in 0..2 {
                    if sub(2) != list[n] {
                        return list[n];
                    }
                }
                Mv::ZERO
            }
        }
    }
}

/// read_mv (§6.4.19): a joint, then per non-zero component sign, class, integer, fraction and
/// (when allowed for a small reference) the high-precision bit; the difference adds to `best`.
fn read_mv(r: &mut BoolDecoder, best: Mv, p: &crate::probs::MvProbs, counts: &mut crate::probs::MvCounts, allow_hp: bool) -> Result<Mv> {
    let joint = r.tree(&MV_JOINT_TREE, &p.joints);
    let use_hp = allow_hp && use_mv_hp(best);
    let mut diff = [0i32; 2];
    if joint == 2 || joint == 3 {
        diff[0] = read_mv_component(r, &p.comps[0], use_hp);
    }
    if joint == 1 || joint == 3 {
        diff[1] = read_mv_component(r, &p.comps[1], use_hp);
    }
    counts.add(diff[0], diff[1]);
    let row = best.row as i32 + diff[0];
    let col = best.col as i32 + diff[1];
    const MV_UPP: i32 = (1 << 14) - 1;
    const MV_LOW: i32 = -(1 << 14);
    if !(row > MV_LOW && row < MV_UPP && col > MV_LOW && col < MV_UPP) {
        return Err(Error::Invalid("motion vector out of range"));
    }
    Ok(Mv::new(row, col))
}

fn read_mv_component(r: &mut BoolDecoder, c: &crate::probs::MvComponentProbs, use_hp: bool) -> i32 {
    let sign = r.read(c.sign);
    let class = r.tree(&MV_CLASS_TREE, &c.classes);
    let class0 = class == 0;
    let (d, mut mag) = if class0 {
        (r.read(c.class0[0]) as i32, 0)
    } else {
        let mut d = 0i32;
        for i in 0..class {
            d |= (r.read(c.bits[i]) as i32) << i;
        }
        (d, 2 << (class + 2))
    };
    let fr = r.tree(&MV_FP_TREE, if class0 { &c.class0_fp[d as usize] } else { &c.fp }) as i32;
    let hp = if use_hp { r.read(if class0 { c.class0_hp } else { c.hp }) as i32 } else { 1 };
    mag += ((d << 3) | (fr << 1) | hp) + 1;
    if sign { -mag } else { mag }
}
