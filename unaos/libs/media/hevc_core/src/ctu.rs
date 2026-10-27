// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! Slice segment data (7.3.8) for I slices, decoded as it is parsed: the CTUs in tile scan with
//! end_of_slice_segment_flag and end_of_subset_one_bit (the engine re-initialised at every tile and wavefront row,
//! 9.3.1 with the CTU-1 synchronisation of 9.3.2.4), the SAO syntax (7.3.8.3), coding_quadtree (7.3.8.4),
//! coding_unit (7.3.8.5: transquant bypass, part_mode, pcm_sample 7.3.8.7, the intra modes), transform_tree and
//! transform_unit (7.3.8.8 / 7.3.8.10), cu_qp_delta with the QP derivation (8.6.1), and residual_coding (7.3.8.11)
//! with the context selection of 9.3.4.2 (split_cu_flag, last_sig_coeff prefixes, coded_sub_block_flag,
//! sig_coeff_flag, the greater1 / greater2 context sets, sign data hiding, the Rice parameter). HEVCINTER (SR216): the
//! inter coding unit of P and B slices — cu_skip_flag, pred_mode_flag, the inter part_mode binarization (9.3.3.7 with
//! the AMP context), prediction_unit (7.3.8.6: merge_flag, merge_idx, inter_pred_idc, ref_idx, mvd_coding 7.3.8.9,
//! mvp flags) and rqt_root_cbf with the inter transform tree; the motion and the samples come from [`crate::inter`].

use alloc::vec;
use alloc::vec::Vec;

use crate::cabac::{init_type, Contexts, Engine};
use crate::inter::{amvp, merge_motion, predict_pu, wrap_mv, InterCtx, Part, Pb, PRED_BI, PRED_L0, PRED_L1};
use crate::intra::{chroma_mode, chroma_mode_422, luma_mode, predict, DC};
use crate::picture::{Picture, PuMotion, SaoParams, F_BYPASS, F_CBF, F_EDGE_H, F_EDGE_V, F_PCM, F_PU_EDGE_H, F_PU_EDGE_V, F_SKIP};
use crate::ps::{Geometry, Pps, ScalingList, Sps, SCAN_ORDER};
use crate::slice::{SliceHeader, SliceType};
use crate::tables::*;
use crate::transform::{add_residual, inverse_transform, scale, transform_skip};
use crate::{Error, Result};

/// The CABAC and QP state that outlives one slice segment within a picture (9.3.2.4 TableStateIdxWpp /
/// TableStateIdxDs; qPY_PREV across dependent slice segments).
#[derive(Default)]
pub struct Storage {
    pub wpp: Option<Contexts>,
    pub ds: Option<Contexts>,
    pub last_qp_y: i32,
}

pub struct SliceDecoder<'a> {
    sps: &'a Sps,
    pps: &'a Pps,
    geo: &'a Geometry,
    sh: &'a SliceHeader,
    pic: &'a mut Picture,
    store: &'a mut Storage,
    eng: Engine<'a>,
    slice_idx: u32,
    slice_addr_rs: u32,
    slice_qp: i32,
    init_type: usize,
    scaling: Option<&'a ScalingList>,
    qp_y: i32,
    qp_y_pred: i32,
    qg: (i32, i32),
    reset_qp_prev: bool,
    is_cu_qp_delta_coded: bool,
    cu_qp_delta_val: i32,
    log2_min_cu_qp_delta: u32,
    cu_bypass: bool,
    ctb_ts: u32,
    ctb_rs: u32,
    /// The reference lists of a P / B slice (HEVCINTER); None in an I slice.
    inter: Option<InterCtx<'a>>,
    /// The coding unit under decoding is intra (CuPredMode) / its transform tree splits by interSplitFlag.
    cu_intra: bool,
    inter_split: bool,
    /// HEVCREXT (SR334): ChromaArrayType 3 with an NxN intra coding unit carries one IntraPredModeC per partition:
    /// (the four modes, the CU's x, y and half size); None otherwise.
    nxn_cmodes: Option<([u32; 4], usize, usize, usize)>,
}

impl<'a> SliceDecoder<'a> {
    pub fn new(
        sps: &'a Sps,
        pps: &'a Pps,
        geo: &'a Geometry,
        sh: &'a SliceHeader,
        pic: &'a mut Picture,
        store: &'a mut Storage,
        rbsp: &'a [u8],
        slice_idx: u32,
        slice_addr_rs: u32,
        inter: Option<InterCtx<'a>>,
    ) -> SliceDecoder<'a> {
        let slice_qp = sh.slice_qp(pps);
        SliceDecoder {
            sps,
            pps,
            geo,
            sh,
            pic,
            store,
            eng: Engine::new(rbsp, sh.data_offset),
            slice_idx,
            slice_addr_rs,
            slice_qp,
            init_type: init_type(sh.slice_type, sh.cabac_init_flag),
            scaling: if sps.scaling_list_enabled_flag { pps.scaling_list.as_ref().or(sps.scaling_list.as_ref()) } else { None },
            qp_y: slice_qp,
            qp_y_pred: slice_qp,
            qg: (-1, -1),
            reset_qp_prev: true,
            is_cu_qp_delta_coded: false,
            cu_qp_delta_val: 0,
            log2_min_cu_qp_delta: sps.log2_ctb - pps.diff_cu_qp_delta_depth,
            cu_bypass: false,
            ctb_ts: 0,
            ctb_rs: 0,
            inter,
            cu_intra: true,
            inter_split: false,
            nxn_cmodes: None,
        }
    }

    fn first_in_tile(&self, ts: u32) -> bool {
        ts == 0 || self.geo.tile_id[ts as usize] != self.geo.tile_id[ts as usize - 1]
    }

    fn row_start(&self, ts: u32, rs: u32) -> bool {
        let g = self.geo;
        rs % g.width_ctbs == 0 || g.tile_id[ts as usize] != g.tile_id[g.rs_to_ts[rs as usize - 1] as usize]
    }

    fn start_ctb(&mut self, ts: u32, rs: u32) {
        self.ctb_ts = ts;
        self.ctb_rs = rs;
        self.pic.ctb_slice[rs as usize] = self.slice_idx;
    }

    fn init(&mut self) {
        self.eng.init_contexts(self.init_type, self.slice_qp);
    }

    /// 9.3.1: at the first CTB of a wavefront row, synchronise with the storage made after the second CTB of the
    /// row above when its top-right CTB is available, else initialise.
    fn sync_or_init(&mut self, rs: u32) {
        let g = self.geo;
        let c = 1i32 << g.log2_ctb;
        let x0 = ((rs % g.width_ctbs) as i32) << g.log2_ctb;
        let y0 = ((rs / g.width_ctbs) as i32) << g.log2_ctb;
        match self.store.wpp {
            Some(s) if self.pic.available(g, x0, y0, x0 + c, y0 - c) => self.eng.load(&s),
            _ => self.init(),
        }
    }

    /// slice_segment_data() (7.3.8.1).
    pub fn decode(&mut self) -> Result<()> {
        let g = self.geo;
        let w = g.width_ctbs;
        let size = w * g.height_ctbs;
        let wpp = self.pps.entropy_coding_sync_enabled_flag;
        let mut rs = self.sh.segment_address;
        let mut ts = g.rs_to_ts[rs as usize];
        self.start_ctb(ts, rs);
        if self.first_in_tile(ts) {
            self.init();
        } else if wpp && self.row_start(ts, rs) {
            self.sync_or_init(rs);
        } else if self.sh.dependent_slice_segment_flag {
            match self.store.ds {
                Some(s) => self.eng.load(&s),
                None => return Err(Error::Invalid("dependent slice segment without stored contexts")),
            }
        } else {
            self.init();
        }
        // qPY_PREV: SliceQpY at the first quantization group of a slice, a tile, or a wavefront row.
        self.reset_qp_prev = !self.sh.dependent_slice_segment_flag || self.first_in_tile(ts) || (wpp && self.row_start(ts, rs));
        loop {
            self.coding_tree_unit()?;
            let end = self.eng.terminate() == 1;
            if wpp && (rs % w == 1 || (w > 1 && rs > 1 && g.tile_id[ts as usize] != g.tile_id[g.rs_to_ts[rs as usize - 2] as usize])) {
                self.store.wpp = Some(self.eng.save());
            }
            if end && self.pps.dependent_slice_segments_enabled_flag {
                self.store.ds = Some(self.eng.save());
            }
            if self.eng.overran() {
                return Err(Error::Truncated);
            }
            if end {
                return Ok(());
            }
            ts += 1;
            if ts >= size {
                return Err(Error::Invalid("slice segment data past the last CTB"));
            }
            rs = g.ts_to_rs[ts as usize];
            let tile_start = self.pps.tiles_enabled_flag && self.first_in_tile(ts);
            let row_start = wpp && self.row_start(ts, rs);
            self.start_ctb(ts, rs);
            if tile_start || row_start {
                if self.eng.terminate() != 1 {
                    return Err(Error::Invalid("end_of_subset_one_bit is 0"));
                }
                let p = self.eng.aligned_pos();
                self.eng.restart_at_byte(p);
                if tile_start {
                    self.init();
                } else {
                    self.sync_or_init(rs);
                }
                self.reset_qp_prev = true;
            }
        }
    }

    /// coding_tree_unit() (7.3.8.2).
    fn coding_tree_unit(&mut self) -> Result<()> {
        let g = self.geo;
        let rx = self.ctb_rs % g.width_ctbs;
        let ry = self.ctb_rs / g.width_ctbs;
        if self.sh.sao_luma_flag || self.sh.sao_chroma_flag {
            self.sao(rx, ry);
        }
        self.coding_quadtree((rx << g.log2_ctb) as i32, (ry << g.log2_ctb) as i32, g.log2_ctb, 0)
    }

    /// sao() (7.3.8.3) with the SaoOffsetVal derivation of 7.4.9.3.2.
    fn sao(&mut self, rx: u32, ry: u32) {
        let g = self.geo;
        let rs = self.ctb_rs as usize;
        let w = g.width_ctbs as usize;
        let ts = self.ctb_ts as usize;
        let mut merge_left = false;
        if rx > 0 {
            let in_slice = self.ctb_rs > self.slice_addr_rs;
            let in_tile = g.tile_id[ts] == g.tile_id[g.rs_to_ts[rs - 1] as usize];
            if in_slice && in_tile {
                merge_left = self.eng.decision(SAO_MERGE) == 1;
            }
        }
        let mut merge_up = false;
        if ry > 0 && !merge_left {
            let in_slice = self.ctb_rs as i64 - w as i64 >= self.slice_addr_rs as i64;
            let in_tile = g.tile_id[ts] == g.tile_id[g.rs_to_ts[rs - w] as usize];
            if in_slice && in_tile {
                merge_up = self.eng.decision(SAO_MERGE) == 1;
            }
        }
        if merge_left {
            self.pic.sao[rs] = self.pic.sao[rs - 1];
            return;
        }
        if merge_up {
            self.pic.sao[rs] = self.pic.sao[rs - w];
            return;
        }
        let mut p = SaoParams::default();
        for c in 0..3 {
            if !((self.sh.sao_luma_flag && c == 0) || (self.sh.sao_chroma_flag && c > 0)) {
                continue;
            }
            // cMax of sao_offset_abs: (1 << (Min(bitDepth, 10) - 5)) - 1 with the component's bit depth (HEVCREXT SR334).
            let bd = if c == 0 { self.sps.bit_depth_luma } else { self.sps.bit_depth_chroma }.min(10);
            let cmax = (1u32 << (bd - 5)) - 1;
            let ty = if c == 2 {
                p.type_idx[1]
            } else if self.eng.decision(SAO_TYPE) == 0 {
                0
            } else if self.eng.bypass() == 0 {
                1
            } else {
                2
            };
            p.type_idx[c] = ty;
            if ty == 0 {
                continue;
            }
            let mut abs = [0i16; 4];
            for a in abs.iter_mut() {
                *a = self.eng.bypass_tu(cmax) as i16;
            }
            if ty == 1 {
                for i in 0..4 {
                    let neg = abs[i] != 0 && self.eng.bypass() == 1;
                    p.offset_val[c][i + 1] = if neg { -abs[i] } else { abs[i] };
                }
                p.band_position[c] = self.eng.bypass_bits(5) as u8;
            } else {
                p.offset_val[c] = [0, abs[0], abs[1], -abs[2], -abs[3]];
                p.eo_class[c] = if c == 2 { p.eo_class[1] } else { self.eng.bypass_bits(2) as u8 };
            }
        }
        self.pic.sao[rs] = p;
    }

    fn avail(&self, xc: i32, yc: i32, xn: i32, yn: i32) -> bool {
        self.pic.available(self.geo, xc, yc, xn, yn)
    }

    /// coding_quadtree() (7.3.8.4).
    fn coding_quadtree(&mut self, x0: i32, y0: i32, log2: u32, depth: u32) -> Result<()> {
        let size = 1i32 << log2;
        let (w, h) = (self.sps.width as i32, self.sps.height as i32);
        let split = if x0 + size <= w && y0 + size <= h && log2 > self.sps.log2_min_cb {
            let mut inc = 0;
            if self.avail(x0, y0, x0 - 1, y0) && self.pic.ctdepth[self.pic.idx4(x0 as usize - 1, y0 as usize)] as u32 > depth {
                inc += 1;
            }
            if self.avail(x0, y0, x0, y0 - 1) && self.pic.ctdepth[self.pic.idx4(x0 as usize, y0 as usize - 1)] as u32 > depth {
                inc += 1;
            }
            self.eng.decision(SPLIT_CU + inc) == 1
        } else {
            log2 > self.sps.log2_min_cb
        };
        if self.pps.cu_qp_delta_enabled_flag && log2 >= self.log2_min_cu_qp_delta {
            self.is_cu_qp_delta_coded = false;
            self.cu_qp_delta_val = 0;
        }
        if split {
            let half = size >> 1;
            for (dx, dy) in [(0, 0), (half, 0), (0, half), (half, half)] {
                if x0 + dx < w && y0 + dy < h {
                    self.coding_quadtree(x0 + dx, y0 + dy, log2 - 1, depth + 1)?;
                }
            }
            Ok(())
        } else {
            self.coding_unit(x0, y0, log2, depth)
        }
    }

    /// 8.6.1: qPY_PRED at the start of a quantization group.
    fn start_qg(&mut self, xq: i32, yq: i32, xcb: i32, ycb: i32) {
        let prev = if self.reset_qp_prev { self.slice_qp } else { self.store.last_qp_y };
        self.reset_qp_prev = false;
        let g = self.geo;
        let cur_ctb = self.ctb_rs;
        let qp_at = |s: &Self, x: i32, y: i32| -> i32 {
            if s.avail(xcb, ycb, x, y) && g.ctb_rs(x as u32, y as u32) == cur_ctb { s.pic.qp[s.pic.idx4(x as usize, y as usize)] as i32 } else { prev }
        };
        let qa = qp_at(self, xq - 1, yq);
        let qb = qp_at(self, xq, yq - 1);
        self.qp_y_pred = (qa + qb + 1) >> 1;
        self.qg = (xq, yq);
    }

    /// QpY (8-283) from qPY_PRED and CuQpDeltaVal.
    fn update_qp(&mut self) {
        let off = 6 * (self.sps.bit_depth_luma as i32 - 8);
        self.qp_y = ((self.qp_y_pred + self.cu_qp_delta_val + 52 + 2 * off) % (52 + off)) - off;
    }

    fn mark_edges(&mut self, x0: usize, y0: usize, size: usize) {
        let (w, h) = (self.pic.width, self.pic.height);
        for y in (y0..(y0 + size).min(h)).step_by(4) {
            let i = self.pic.idx4(x0, y);
            self.pic.flags[i] |= F_EDGE_V;
        }
        for x in (x0..(x0 + size).min(w)).step_by(4) {
            let i = self.pic.idx4(x, y0);
            self.pic.flags[i] |= F_EDGE_H;
        }
    }

    /// coding_unit() (7.3.8.5) of an I slice.
    fn coding_unit(&mut self, x0: i32, y0: i32, log2: u32, depth: u32) -> Result<()> {
        let mask = (1i32 << self.log2_min_cu_qp_delta) - 1;
        let (xq, yq) = (x0 & !mask, y0 & !mask);
        if (xq, yq) != self.qg {
            self.start_qg(xq, yq, x0, y0);
        }
        self.update_qp();
        let size = 1usize << log2;
        let (xu, yu) = (x0 as usize, y0 as usize);
        self.cu_bypass = self.pps.transquant_bypass_enabled_flag && self.eng.decision(TRANSQUANT_BYPASS) == 1;
        if self.inter.is_some() && self.inter_coding_unit(x0, y0, log2, depth)? {
            return self.finish_cu(xu, yu, size);
        }
        self.cu_intra = true;
        self.inter_split = false;
        let nxn = log2 == self.sps.log2_min_cb && self.eng.decision(PART_MODE) == 0;
        let pcm = !nxn && self.sps.pcm.is_some_and(|p| log2 >= p.log2_min_cb && log2 <= p.log2_max_cb) && self.eng.terminate() == 1;
        let bypass = self.cu_bypass;
        self.pic.fill4(xu, yu, size, size, |p, i| {
            p.ctdepth[i] = depth as u8;
            p.flags[i] = (p.flags[i] & !(F_PCM | F_BYPASS)) | if pcm { F_PCM } else { 0 } | if bypass { F_BYPASS } else { 0 };
            p.intra_mode[i] = DC as u8;
        });
        self.mark_edges(xu, yu, size);
        if pcm {
            self.pcm_sample(xu, yu, log2)?;
        } else {
            let nparts = if nxn { 4 } else { 1 };
            let pb = if nxn { size / 2 } else { size };
            let mut prev = [false; 4];
            for p in prev.iter_mut().take(nparts) {
                *p = self.eng.decision(PREV_INTRA_LUMA) == 1;
            }
            let mut mpm = [0u32; 4];
            let mut rem = [0u32; 4];
            for i in 0..nparts {
                if prev[i] {
                    mpm[i] = self.eng.bypass_tu(2);
                } else {
                    rem[i] = self.eng.bypass_bits(5);
                }
            }
            let ctb_mask = !((1i32 << self.sps.log2_ctb) - 1);
            for i in 0..nparts {
                let xp = x0 + ((i & 1) * pb) as i32;
                let yp = y0 + ((i >> 1) * pb) as i32;
                let a = if self.avail(xp, yp, xp - 1, yp) { self.pic.intra_mode[self.pic.idx4(xp as usize - 1, yp as usize)] as u32 } else { DC };
                let b = if yp - 1 < (yp & ctb_mask) {
                    DC
                } else if self.avail(xp, yp, xp, yp - 1) {
                    self.pic.intra_mode[self.pic.idx4(xp as usize, yp as usize - 1)] as u32
                } else {
                    DC
                };
                let m = luma_mode(a, b, prev[i], mpm[i], rem[i]);
                self.pic.fill4(xp as usize, yp as usize, pb, pb, |p, j| p.intra_mode[j] = m as u8);
            }
            // intra_chroma_pred_mode (7.3.8.5): one per partition at ChromaArrayType 3 with NxN, else one; Table 8-2,
            // then Table 8-3 at ChromaArrayType 2 (HEVCREXT SR334).
            let cat = self.sps.chroma_array_type();
            let ncm = if cat == 3 && nxn { 4 } else { 1 };
            let mut cmodes = [DC; 4];
            for (i, m) in cmodes.iter_mut().enumerate().take(ncm) {
                let icpm = if self.eng.decision(INTRA_CHROMA) == 0 { 4 } else { self.eng.bypass_bits(2) };
                let (xp, yp) = (xu + (i & 1) * pb, yu + (i >> 1) * pb);
                let x = chroma_mode(icpm, self.pic.intra_mode[self.pic.idx4(xp, yp)] as u32);
                *m = if cat == 2 { chroma_mode_422(x) } else { x };
            }
            self.nxn_cmodes = if ncm == 4 { Some((cmodes, xu, yu, pb)) } else { None };
            let cmode = cmodes[0];
            let max_depth = self.sps.max_transform_hierarchy_depth_intra + nxn as u32;
            self.transform_tree(xu, yu, xu, yu, log2, 0, 0, 1, 1, nxn, max_depth, cmode)?;
            self.nxn_cmodes = None;
        }
        self.finish_cu(xu, yu, size)
    }

    /// The end of every coding unit: its QpY into the 4x4 facts, qPY_PREV for the next one.
    fn finish_cu(&mut self, xu: usize, yu: usize, size: usize) -> Result<()> {
        let qp = self.qp_y as i8;
        self.pic.fill4(xu, yu, size, size, |p, i| p.qp[i] = qp);
        self.store.last_qp_y = self.qp_y;
        if self.eng.overran() {
            return Err(Error::Truncated);
        }
        Ok(())
    }

    /// pcm_alignment_zero_bits and pcm_sample() (7.3.8.7), then the engine re-initialised after the samples (9.3.2.5).
    fn pcm_sample(&mut self, x0: usize, y0: usize, log2: u32) -> Result<()> {
        let pcm = self.sps.pcm.ok_or(Error::Invalid("pcm without pcm_enabled_flag"))?;
        let start = self.eng.aligned_pos();
        let data = self.eng.data();
        let mut bit = start * 8;
        let mut read = |n: u32| -> Result<u32> {
            let mut v = 0u32;
            for _ in 0..n {
                let b = *data.get(bit >> 3).ok_or(Error::Truncated)?;
                v = (v << 1) | ((b >> (7 - (bit & 7))) & 1) as u32;
                bit += 1;
            }
            Ok(v)
        };
        let n = 1usize << log2;
        let (bdy, bdc) = (self.sps.bit_depth_luma, self.sps.bit_depth_chroma);
        for y in 0..n {
            for x in 0..n {
                let v = read(pcm.bit_depth_luma)? << (bdy - pcm.bit_depth_luma);
                self.pic.planes[0].set(x0 + x, y0 + y, v as u16);
            }
        }
        // HEVCREXT (SR334): (CbSize / SubWidthC) x (CbSize / SubHeightC) samples per chroma component.
        let (sw, sh) = self.pic.chroma.sub();
        for c in 1..3 {
            for y in 0..n / sh {
                for x in 0..n / sw {
                    let v = read(pcm.bit_depth_chroma)? << (bdc - pcm.bit_depth_chroma);
                    self.pic.planes[c].set(x0 / sw + x, y0 / sh + y, v as u16);
                }
            }
        }
        self.eng.restart_at_byte(bit.div_ceil(8));
        Ok(())
    }

    /// transform_tree() (7.3.8.8). `parent_cb` / `parent_cr` are cbf_cb / cbf_cr of the parent node (nonzero at the root).
    /// HEVCREXT (SR334): cbf_cb / cbf_cr are 2-bit masks — bit 0 the (top) chroma block, bit 1 the second block of
    /// ChromaArrayType 2.
    fn transform_tree(
        &mut self,
        x0: usize,
        y0: usize,
        xb: usize,
        yb: usize,
        log2: u32,
        depth: u32,
        blk: u32,
        parent_cb: u8,
        parent_cr: u8,
        intra_split: bool,
        max_depth: u32,
        cmode: u32,
    ) -> Result<()> {
        let sps = self.sps;
        let split = if log2 <= sps.log2_max_tb && log2 > sps.log2_min_tb && depth < max_depth && !(intra_split && depth == 0) {
            self.eng.decision(SPLIT_TRANSFORM + 5 - log2 as usize) == 1
        } else {
            log2 > sps.log2_max_tb || (intra_split && depth == 0) || (self.inter_split && depth == 0)
        };
        let (mut cb, mut cr) = (0u8, 0u8);
        let cat = sps.chroma_array_type();
        if log2 > 2 || cat == 3 {
            let two = cat == 2 && (!split || log2 == 3);
            if depth == 0 || parent_cb != 0 {
                cb = self.eng.decision(CBF_CHROMA + depth as usize) as u8;
                if two {
                    cb |= (self.eng.decision(CBF_CHROMA + depth as usize) as u8) << 1;
                }
            }
            if depth == 0 || parent_cr != 0 {
                cr = self.eng.decision(CBF_CHROMA + depth as usize) as u8;
                if two {
                    cr |= (self.eng.decision(CBF_CHROMA + depth as usize) as u8) << 1;
                }
            }
        }
        if split {
            let half = 1usize << (log2 - 1);
            for i in 0..4u32 {
                let (dx, dy) = ((i as usize & 1) * half, (i as usize >> 1) * half);
                self.transform_tree(x0 + dx, y0 + dy, x0, y0, log2 - 1, depth + 1, i, cb, cr, intra_split, max_depth, cmode)?;
            }
            Ok(())
        } else {
            // cbf_luma is inferred 1 in an inter coding unit's root transform unit without chroma cbfs (7.3.8.8).
            let cbf_luma = if self.cu_intra || depth != 0 || cb != 0 || cr != 0 { self.eng.decision(CBF_LUMA + if depth == 0 { 1 } else { 0 }) == 1 } else { true };
            self.transform_unit(x0, y0, xb, yb, log2, blk, cbf_luma, cb, cr, parent_cb, parent_cr, cmode)
        }
    }

    /// transform_unit() (7.3.8.10) with the intra reconstruction of each block (8.4.4.1).
    fn transform_unit(
        &mut self,
        x0: usize,
        y0: usize,
        xb: usize,
        yb: usize,
        log2: u32,
        blk: u32,
        cbf_luma: bool,
        cb: u8,
        cr: u8,
        parent_cb: u8,
        parent_cr: u8,
        cmode: u32,
    ) -> Result<()> {
        let cat = self.sps.chroma_array_type();
        let chroma_here = log2 > 2 || cat == 3;
        let (ccb, ccr) = if chroma_here { (cb, cr) } else { (parent_cb, parent_cr) };
        if (cbf_luma || ccb != 0 || ccr != 0) && self.pps.cu_qp_delta_enabled_flag && !self.is_cu_qp_delta_coded {
            let mut v: i32 = 0;
            if self.eng.decision(CU_QP_DELTA_ABS) == 1 {
                v = 1;
                while v < 5 && self.eng.decision(CU_QP_DELTA_ABS + 1) == 1 {
                    v += 1;
                }
                if v == 5 {
                    v += self.eng.bypass_egk(0).min(1 << 16) as i32;
                }
            }
            if v > 0 && self.eng.bypass() == 1 {
                v = -v;
            }
            let off = 3 * (self.sps.bit_depth_luma as i32 - 8);
            if v < -(26 + off) || v > 25 + off {
                return Err(Error::Invalid("CuQpDeltaVal out of range"));
            }
            self.is_cu_qp_delta_coded = true;
            self.cu_qp_delta_val = v;
            self.update_qp();
        }
        self.mark_edges(x0, y0, 1 << log2);
        let bd = self.sps.bit_depth_luma;
        let lmode = self.pic.intra_mode[self.pic.idx4(x0, y0)] as u32;
        if self.cu_intra {
            predict(self.pic, self.geo, 0, x0, y0, log2, lmode, self.sps.strong_intra_smoothing_enabled_flag, bd);
        }
        if cbf_luma {
            let n = 1usize << log2;
            self.pic.fill4(x0, y0, n, n, |p, i| p.flags[i] |= F_CBF);
            self.residual(x0, y0, log2, 0, lmode)?;
        }
        // HEVCREXT (SR334): the chroma block(s) at log2TrafoSizeC (ChromaArrayType 3: log2TrafoSize; else log2TrafoSize
        // - 1, or 2 at blkIdx 3 of a 4x4 split), at (x / SubWidthC, y / SubHeightC).
        let (sw, sh) = self.pic.chroma.sub();
        let cmode = match self.nxn_cmodes {
            Some((m, xc, yc, half)) => m[(((y0 >= yc + half) as usize) << 1) | (x0 >= xc + half) as usize],
            None => cmode,
        };
        if chroma_here {
            let log2c = if cat == 3 { log2 } else { log2 - 1 };
            self.chroma_blocks(x0 / sw, y0 / sh, log2c, cb, cr, cmode)?;
        } else if blk == 3 {
            self.chroma_blocks(xb / sw, yb / sh, 2, parent_cb, parent_cr, cmode)?;
        }
        Ok(())
    }

    /// HEVCREXT (SR334): the chroma blocks of a transform unit in parse order (7.3.8.10) — per component one block, or
    /// two stacked at (xc, yc) and (xc, yc + (1 << log2c)) for ChromaArrayType 2 — each predicted (intra) then
    /// reconstructed; bit t of `cb` / `cr` is the cbf of block t.
    fn chroma_blocks(&mut self, xc: usize, yc: usize, log2c: u32, cb: u8, cr: u8, cmode: u32) -> Result<()> {
        let bdc = self.sps.bit_depth_chroma;
        let nblk = if self.sps.chroma_array_type() == 2 { 2 } else { 1 };
        for (c, cbf) in [(1usize, cb), (2, cr)] {
            for t in 0..nblk {
                let y = yc + (t << log2c);
                if self.cu_intra {
                    predict(self.pic, self.geo, c, xc, y, log2c, cmode, false, bdc);
                }
                if cbf & (1 << t) != 0 {
                    self.residual(xc, y, log2c, c, cmode)?;
                }
            }
        }
        Ok(())
    }

    /// Parse one block's residual_coding() and add its residual to the prediction (8.6.2 .. 8.6.4, 8.6.7).
    fn residual(&mut self, x0: usize, y0: usize, log2: u32, c: usize, mode: u32) -> Result<()> {
        let (mut coeff, ts) = self.residual_coding(log2, c, mode)?;
        let bd = if c == 0 { self.sps.bit_depth_luma } else { self.sps.bit_depth_chroma };
        let res = if self.cu_bypass {
            coeff
        } else {
            let qp = if c == 0 {
                self.qp_y + 6 * (bd as i32 - 8)
            } else {
                let off = 6 * (bd as i32 - 8);
                let (po, so) = if c == 1 { (self.pps.cb_qp_offset, self.sh.cb_qp_offset) } else { (self.pps.cr_qp_offset, self.sh.cr_qp_offset) };
                let qpi = (self.qp_y + po + so).clamp(-off, 57);
                // 8.6.1: Table 8-10 at ChromaArrayType 1, else Min(qPi, 51) (HEVCREXT SR334).
                (if self.sps.chroma_array_type() == 1 { qpc_420(qpi) } else { qpi.min(51) }) + off
            };
            let n = 1usize << log2;
            let factor = match self.scaling {
                Some(sl) if !(ts && n > 4) => Some(sl.factor[(log2 - 2) as usize][c + if self.cu_intra { 0 } else { 3 }].as_slice()),
                _ => None,
            };
            scale(&mut coeff, log2, qp, bd, factor);
            if ts { transform_skip(&coeff, log2, bd) } else { inverse_transform(&coeff, log2, c == 0 && log2 == 2 && self.cu_intra, bd) }
        };
        add_residual(&mut self.pic.planes[c], x0, y0, log2, &res, bd);
        Ok(())
    }

    /// residual_coding() (7.3.8.11): the TransCoeffLevel array ([y * nTbS + x]) and transform_skip_flag.
    fn residual_coding(&mut self, log2: u32, c: usize, pred_mode: u32) -> Result<(Vec<i32>, bool)> {
        let n = 1usize << log2;
        let mut coeff = vec![0i32; n * n];
        let ts = self.pps.transform_skip_enabled_flag && !self.cu_bypass && log2 <= 2 && self.eng.decision(TRANSFORM_SKIP + (c > 0) as usize) == 1;
        // last_sig_coeff_{x,y}_prefix (9.3.4.2.3) and suffix.
        let (off, shift) = if c == 0 { (3 * (log2 - 2) + ((log2 - 1) >> 2), (log2 + 1) >> 2) } else { (15, log2 - 2) };
        let cmax = (log2 << 1) - 1;
        let mut px = 0;
        while px < cmax && self.eng.decision(LAST_X + (off + (px >> shift)) as usize) == 1 {
            px += 1;
        }
        let mut py = 0;
        while py < cmax && self.eng.decision(LAST_Y + (off + (py >> shift)) as usize) == 1 {
            py += 1;
        }
        let mut lx = px as usize;
        if px > 3 {
            let nb = (px >> 1) - 1;
            lx = ((1usize << nb) * (2 + (px as usize & 1))) + self.eng.bypass_bits(nb) as usize;
        }
        let mut ly = py as usize;
        if py > 3 {
            let nb = (py >> 1) - 1;
            ly = ((1usize << nb) * (2 + (py as usize & 1))) + self.eng.bypass_bits(nb) as usize;
        }
        // scanIdx (8.4.4.1 / 7.4.9.11; HEVCREXT SR334: log2TrafoSize 3 of every component at ChromaArrayType 3).
        let scan_idx = if log2 == 2 || (log2 == 3 && (c == 0 || self.sps.chroma_array_type() == 3)) {
            if (6..=14).contains(&pred_mode) {
                2
            } else if (22..=30).contains(&pred_mode) {
                1
            } else {
                0
            }
        } else {
            0
        };
        if scan_idx == 2 {
            core::mem::swap(&mut lx, &mut ly);
        }
        if lx >= n || ly >= n {
            return Err(Error::Invalid("last significant coefficient outside the block"));
        }
        let log2sb = (log2 - 2) as usize;
        let nsb = 1usize << log2sb;
        let sb_scan = &SCAN_ORDER[log2sb][scan_idx];
        let sc = &SCAN_ORDER[2][scan_idx];
        let last_sb = (0..nsb * nsb).rev().find(|&i| sb_scan[i] == ((lx >> 2) as u8, (ly >> 2) as u8)).unwrap_or(0);
        let last_pos = (0..16).rev().find(|&p| sc[p] == ((lx & 3) as u8, (ly & 3) as u8)).unwrap_or(0);
        let mut csbf = [[false; 8]; 8];
        let mut c1: usize = 1;
        let sign_hiding = self.pps.sign_data_hiding_enabled_flag && !self.cu_bypass;
        let chroma = c > 0;
        for i in (0..=last_sb).rev() {
            let (xs, ys) = (sb_scan[i].0 as usize, sb_scan[i].1 as usize);
            let mut infer_dc = false;
            if i < last_sb && i > 0 {
                let mut cs = 0;
                if xs + 1 < nsb && csbf[xs + 1][ys] {
                    cs += 1;
                }
                if ys + 1 < nsb && csbf[xs][ys + 1] {
                    cs += 1;
                }
                csbf[xs][ys] = self.eng.decision(CSBF + cs.min(1) + if chroma { 2 } else { 0 }) == 1;
                infer_dc = true;
            } else {
                csbf[xs][ys] = true;
            }
            let mut nz = [0usize; 16];
            let mut nnz = 0;
            let start: i32 = if i == last_sb {
                nz[0] = last_pos;
                nnz = 1;
                last_pos as i32 - 1
            } else {
                15
            };
            if csbf[xs][ys] {
                let prev_csbf = (xs + 1 < nsb && csbf[xs + 1][ys]) as u32 | (((ys + 1 < nsb && csbf[xs][ys + 1]) as u32) << 1);
                let mut p = start;
                while p >= 0 {
                    let pu = p as usize;
                    let (xc, yc) = (xs * 4 + sc[pu].0 as usize, ys * 4 + sc[pu].1 as usize);
                    let sig = if p > 0 || !infer_dc {
                        let ctx = sig_ctx(xc, yc, xs, ys, log2, chroma, scan_idx, prev_csbf);
                        self.eng.decision(SIG + ctx) == 1
                    } else {
                        true
                    };
                    if sig {
                        nz[nnz] = pu;
                        nnz += 1;
                        infer_dc = false;
                    }
                    p -= 1;
                }
            }
            if nnz == 0 {
                continue;
            }
            // coeff_abs_level_greater1_flag / greater2_flag (9.3.4.2.6 / 9.3.4.2.7).
            let mut ctx_set = if i > 0 && !chroma { 2 } else { 0 };
            if c1 == 0 {
                ctx_set += 1;
            }
            c1 = 1;
            let mut abs = [1u32; 16];
            let mut first_c2: Option<usize> = None;
            for (k, a) in abs.iter_mut().enumerate().take(nnz.min(8)) {
                let f = self.eng.decision(GT1 + if chroma { 16 } else { 0 } + ctx_set * 4 + c1);
                if f == 1 {
                    c1 = 0;
                    if first_c2.is_none() {
                        first_c2 = Some(k);
                    }
                } else if c1 > 0 && c1 < 3 {
                    c1 += 1;
                }
                *a = 1 + f;
            }
            if c1 == 0 {
                if let Some(k) = first_c2 {
                    abs[k] += self.eng.decision(GT2 + if chroma { 4 } else { 0 } + ctx_set);
                }
            }
            let hidden = sign_hiding && nz[0] as i32 - nz[nnz - 1] as i32 > 3;
            let mut signs = [0u32; 16];
            for (k, s) in signs.iter_mut().enumerate().take(nnz) {
                if !(k == nnz - 1 && hidden) {
                    *s = self.eng.bypass();
                }
            }
            // coeff_abs_level_remaining (9.3.3.11) with the Rice parameter update.
            let mut first2 = 1u32;
            let mut rice = 0u32;
            for k in 0..nnz {
                let base = if k < 8 { 2 + first2 } else { 1 };
                if abs[k] == base {
                    let r = self.eng.coeff_abs_level_remaining(rice);
                    abs[k] = abs[k].saturating_add(r).min(1 << 16);
                    if abs[k] > 3 * (1 << rice) {
                        rice = (rice + 1).min(4);
                    }
                }
                if abs[k] >= 2 {
                    first2 = 0;
                }
            }
            let mut sum = 0u32;
            for k in 0..nnz {
                let pu = nz[k];
                let (xc, yc) = (xs * 4 + sc[pu].0 as usize, ys * 4 + sc[pu].1 as usize);
                let mut v = (abs[k] as i32).min(32768);
                sum = sum.wrapping_add(abs[k]);
                if signs[k] == 1 {
                    v = -v;
                }
                coeff[yc * n + xc] = v.clamp(-32768, 32767);
            }
            if hidden && sum & 1 == 1 {
                let pu = nz[nnz - 1];
                let (xc, yc) = (xs * 4 + sc[pu].0 as usize, ys * 4 + sc[pu].1 as usize);
                coeff[yc * n + xc] = -coeff[yc * n + xc];
            }
        }
        Ok((coeff, ts))
    }

    /// HEVCINTER: the inter half of coding_unit() (7.3.8.5) in a P or B slice — cu_skip_flag, pred_mode_flag,
    /// part_mode, the prediction units and rqt_root_cbf with the inter transform tree. Ok(false): the coding unit is
    /// intra and the caller's intra path decodes it.
    fn inter_coding_unit(&mut self, x0: i32, y0: i32, log2: u32, depth: u32) -> Result<bool> {
        let skip = self.eng.decision(CU_SKIP + self.skip_inc(x0, y0)) == 1;
        if !skip && self.eng.decision(PRED_MODE) == 1 {
            return Ok(false);
        }
        self.cu_intra = false;
        self.inter_split = false;
        let part = if skip { Part::P2Nx2N } else { self.inter_part_mode(log2) };
        let size = 1usize << log2;
        let (xu, yu) = (x0 as usize, y0 as usize);
        let fl = if self.cu_bypass { F_BYPASS } else { 0 } | if skip { F_SKIP } else { 0 };
        self.pic.fill4(xu, yu, size, size, |p, i| {
            p.ctdepth[i] = depth as u8;
            p.flags[i] = (p.flags[i] & !(F_PCM | F_BYPASS | F_SKIP)) | fl;
            p.intra_mode[i] = DC as u8;
        });
        self.mark_edges(xu, yu, size);
        let mut merge0 = false;
        for (k, &(xp, yp, w, h)) in part.pus(x0, y0, size as i32).iter().enumerate() {
            let pb = Pb { xc: x0, yc: y0, ncb: size as i32, x: xp, y: yp, w, h, part, idx: k };
            let merge = self.prediction_unit(&pb, skip, depth)?;
            if k == 0 {
                merge0 = merge;
            }
            self.mark_pu_edges(xp as usize, yp as usize, w as usize, h as usize);
        }
        if !skip {
            // rqt_root_cbf is inferred 1 for a merged 2Nx2N prediction unit.
            let root = (part == Part::P2Nx2N && merge0) || self.eng.decision(RQT_ROOT_CBF) == 1;
            if root {
                let max_depth = self.sps.max_transform_hierarchy_depth_inter;
                self.inter_split = max_depth == 0 && part != Part::P2Nx2N;
                self.transform_tree(xu, yu, xu, yu, log2, 0, 0, 1, 1, false, max_depth, DC)?;
                self.inter_split = false;
            }
        }
        Ok(true)
    }

    /// cu_skip_flag ctxInc (9.3.4.2.2): the available left and above blocks that were skipped.
    fn skip_inc(&self, x0: i32, y0: i32) -> usize {
        let f = |s: &Self, x: i32, y: i32| s.avail(x0, y0, x, y) && s.pic.flags[s.pic.idx4(x as usize, y as usize)] & F_SKIP != 0;
        f(self, x0 - 1, y0) as usize + f(self, x0, y0 - 1) as usize
    }

    /// part_mode of an inter coding unit (9.3.3.7, Table 9-43; bins 0 and 1 ctxInc 0 / 1, the third bin ctxInc 2 at
    /// the minimum size and 3 — the AMP flag — above it, the fourth bypass).
    fn inter_part_mode(&mut self, log2: u32) -> Part {
        if self.eng.decision(PART_MODE) == 1 {
            return Part::P2Nx2N;
        }
        let hor = self.eng.decision(PART_MODE + 1) == 1;
        if log2 == self.sps.log2_min_cb {
            if hor {
                return Part::P2NxN;
            }
            if log2 == 3 || self.eng.decision(PART_MODE + 2) == 1 {
                return Part::PNx2N;
            }
            return Part::PNxN;
        }
        if !self.sps.amp_enabled_flag || self.eng.decision(PART_MODE + 3) == 1 {
            return if hor { Part::P2NxN } else { Part::PNx2N };
        }
        match (hor, self.eng.bypass()) {
            (true, 0) => Part::P2NxnU,
            (true, _) => Part::P2NxnD,
            (false, 0) => Part::PnLx2N,
            (false, _) => Part::PnRx2N,
        }
    }

    /// The left / top edges of a prediction block (8.7.2.3: prediction block edges are filtered too).
    fn mark_pu_edges(&mut self, x0: usize, y0: usize, w: usize, h: usize) {
        let (pw, ph) = (self.pic.width, self.pic.height);
        for y in (y0..(y0 + h).min(ph)).step_by(4) {
            let i = self.pic.idx4(x0, y);
            self.pic.flags[i] |= F_PU_EDGE_V;
        }
        for x in (x0..(x0 + w).min(pw)).step_by(4) {
            let i = self.pic.idx4(x, y0);
            self.pic.flags[i] |= F_PU_EDGE_H;
        }
    }

    /// TR with cMax `cmax` whose first `ctx_bins` bins use contexts `slot + i` and the rest bypass (merge_idx, ref_idx).
    fn tr_ctx(&mut self, slot: usize, ctx_bins: u32, cmax: u32) -> u32 {
        let mut v = 0;
        while v < cmax {
            let b = if v < ctx_bins { self.eng.decision(slot + v as usize) } else { self.eng.bypass() };
            if b == 0 {
                break;
            }
            v += 1;
        }
        v
    }

    /// inter_pred_idc (9.3.3.8): bin 0 (ctxInc CtDepth) "1" is PRED_BI unless nPbW + nPbH is 12; the last bin (ctxInc
    /// 4) chooses PRED_L0 / PRED_L1.
    fn inter_pred_idc(&mut self, wh: i32, depth: u32) -> u32 {
        if wh != 12 && self.eng.decision(INTER_PRED_IDC + depth.min(3) as usize) == 1 {
            return PRED_BI;
        }
        if self.eng.decision(INTER_PRED_IDC + 4) == 1 { PRED_L1 } else { PRED_L0 }
    }

    /// mvd_coding() (7.3.8.9): MvdLX[0..2].
    fn mvd_coding(&mut self) -> [i32; 2] {
        let g0 = [self.eng.decision(ABS_MVD_GT0), self.eng.decision(ABS_MVD_GT0)];
        let mut g1 = [0u32; 2];
        for c in 0..2 {
            if g0[c] == 1 {
                g1[c] = self.eng.decision(ABS_MVD_GT1);
            }
        }
        let mut v = [0i32; 2];
        for c in 0..2 {
            if g0[c] == 1 {
                let mut a = 1 + g1[c] as i32;
                if g1[c] == 1 {
                    a += self.eng.bypass_egk(1).min(1 << 15) as i32;
                }
                if self.eng.bypass() == 1 {
                    a = -a;
                }
                v[c] = a;
            }
        }
        v
    }

    /// prediction_unit() (7.3.8.6): the syntax, the motion (merge 8.5.3.2.2 or AMVP 8.5.3.2.6 + MvdLX), stored per 4x4,
    /// and the predicted samples (8.5.3.3). Returns merge_flag.
    fn prediction_unit(&mut self, pb: &Pb, skip: bool, depth: u32) -> Result<bool> {
        let Some(ctx) = self.inter else {
            return Err(Error::Invalid("inter coding unit in an I slice"));
        };
        let max_merge = self.sh.max_num_merge_cand;
        let merge = skip || self.eng.decision(MERGE_FLAG) == 1;
        let motion = if merge {
            let idx = if max_merge > 1 { self.tr_ctx(MERGE_IDX, 1, max_merge - 1) } else { 0 };
            merge_motion(self.pic, self.geo, &ctx, self.sh, self.pps, pb, idx as usize)
        } else {
            let idc = if self.sh.slice_type == SliceType::B { self.inter_pred_idc(pb.w + pb.h, depth) } else { PRED_L0 };
            let mut m = PuMotion::default();
            let mut mvd = [[0i32; 2]; 2];
            let mut flag = [0u32; 2];
            for x in 0..2 {
                if (x == 0 && idc == PRED_L1) || (x == 1 && idc == PRED_L0) {
                    continue;
                }
                let active = if x == 0 { self.sh.num_ref_idx_l0_active } else { self.sh.num_ref_idx_l1_active };
                m.ref_idx[x] = if active > 1 { self.tr_ctx(REF_IDX, 2, active - 1) } else { 0 } as i8;
                if !(x == 1 && self.sh.mvd_l1_zero_flag && idc == PRED_BI) {
                    mvd[x] = self.mvd_coding();
                }
                flag[x] = self.eng.decision(MVP_FLAG);
                m.pred |= 1 << x;
            }
            ctx.complete(&mut m);
            for x in 0..2 {
                if m.uses(x) {
                    let mvp = amvp(self.pic, self.geo, &ctx, self.sh, pb, x, m.ref_idx[x] as usize)[flag[x] as usize];
                    m.mv[x] = [wrap_mv(mvp[0] + mvd[x][0]), wrap_mv(mvp[1] + mvd[x][1])];
                }
            }
            m
        };
        self.pic.fill4(pb.x as usize, pb.y as usize, pb.w as usize, pb.h as usize, |p, i| p.motion[i] = motion);
        let (bdy, bdc) = (self.sps.bit_depth_luma, self.sps.bit_depth_chroma);
        predict_pu(self.pic, &ctx, self.sh, self.pps, bdy, bdc, pb.x, pb.y, pb.w, pb.h, &motion);
        Ok(merge)
    }
}

/// sig_coeff_flag ctxInc (9.3.4.2.5), without the range extension's transform_skip_context.
pub fn sig_ctx(xc: usize, yc: usize, xs: usize, ys: usize, log2: u32, chroma: bool, scan_idx: usize, prev_csbf: u32) -> usize {
    let s = if log2 == 2 {
        CTX_IDX_MAP[(yc << 2) + xc] as usize
    } else if xc + yc == 0 {
        0
    } else {
        let (xp, yp) = (xc & 3, yc & 3);
        let mut s = match prev_csbf {
            0 => {
                if xp + yp == 0 {
                    2
                } else if xp + yp < 3 {
                    1
                } else {
                    0
                }
            }
            1 => {
                if yp == 0 {
                    2
                } else if yp == 1 {
                    1
                } else {
                    0
                }
            }
            2 => {
                if xp == 0 {
                    2
                } else if xp == 1 {
                    1
                } else {
                    0
                }
            }
            _ => 2,
        };
        if !chroma {
            if (xs, ys) != (0, 0) {
                s += 3;
            }
            s += if log2 == 3 {
                if scan_idx == 0 { 9 } else { 15 }
            } else {
                21
            };
        } else {
            s += if log2 == 3 { 9 } else { 12 };
        }
        s
    };
    if chroma { 27 + s } else { s }
}
