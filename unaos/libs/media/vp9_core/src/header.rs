// SPDX-License-Identifier: LGPL-3.0-or-later
// Copyright (C) 2026 The Architect & Una
//
//! The frame headers: the uncompressed header (spec §6.2 / §7.2: frame marker, profile,
//! show_existing_frame, frame type, sync code, colour config, frame size / render size / size from
//! refs, refresh flags, interpolation filter, context flags, loop filter, quantization,
//! segmentation, tile info, header_size_in_bytes) and the compressed header (§6.3 / §7.3: tx_mode,
//! the probability deltas). Superframe indices (Annex B) live here too.

use crate::bits::BitReader;
use crate::bool_decoder::BoolDecoder;
use crate::consts::*;
use crate::probs::FrameContext;
use crate::{Error, Result};

/// The loop filter parameters; deltas persist across frames (§7.2.8).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopFilterParams {
    pub level: u8,
    pub sharpness: u8,
    pub delta_enabled: bool,
    pub ref_deltas: [i8; 4],
    pub mode_deltas: [i8; 2],
}

impl Default for LoopFilterParams {
    fn default() -> Self {
        LoopFilterParams { level: 0, sharpness: 0, delta_enabled: true, ref_deltas: [1, 0, -1, -1], mode_deltas: [0, 0] }
    }
}

/// Segmentation parameters; persist across frames (§7.2.10).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct Segmentation {
    pub enabled: bool,
    pub update_map: bool,
    pub temporal_update: bool,
    pub abs_delta: bool,
    pub tree_probs: [u8; 7],
    pub pred_probs: [u8; 3],
    pub feature_enabled: [[bool; 4]; 8],
    pub feature_data: [[i16; 4]; 8],
}

impl Segmentation {
    pub fn active(&self, seg: usize, feature: usize) -> bool {
        self.enabled && self.feature_enabled[seg][feature]
    }
    pub fn clear_features(&mut self) {
        self.feature_enabled = [[false; 4]; 8];
        self.feature_data = [[0; 4]; 8];
        self.abs_delta = false;
    }
    /// get_qindex (§8.6.1).
    pub fn qindex(&self, seg: usize, base: u8) -> u8 {
        if self.active(seg, SEG_LVL_ALT_Q) {
            let d = self.feature_data[seg][SEG_LVL_ALT_Q] as i32;
            let q = if self.abs_delta { d } else { base as i32 + d };
            q.clamp(0, 255) as u8
        } else {
            base
        }
    }
}

/// Persistent decoder-side state the uncompressed header reads and updates.
#[derive(Clone, Debug)]
pub struct HeaderState {
    pub bit_depth: u8,
    pub ss_x: u8,
    pub ss_y: u8,
    pub color_space: u8,
    pub color_range: u8,
    pub lf: LoopFilterParams,
    pub seg: Segmentation,
    /// Crop size of each reference slot (None = empty).
    pub ref_dims: [Option<(u32, u32)>; 8],
}

impl Default for HeaderState {
    fn default() -> Self {
        HeaderState {
            bit_depth: 8,
            ss_x: 1,
            ss_y: 1,
            color_space: 0,
            color_range: 0,
            lf: LoopFilterParams::default(),
            seg: Segmentation::default(),
            ref_dims: [None; 8],
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct FrameHeader {
    pub profile: u8,
    pub show_existing_frame: bool,
    pub frame_to_show: u8,
    pub key_frame: bool,
    pub show_frame: bool,
    pub error_resilient: bool,
    pub intra_only: bool,
    pub reset_frame_context: u8,
    pub refresh_frame_flags: u8,
    pub ref_frame_idx: [u8; 3],
    /// Indexed by reference frame (INTRA, LAST, GOLDEN, ALTREF).
    pub sign_bias: [bool; 4],
    pub width: u32,
    pub height: u32,
    pub render_width: u32,
    pub render_height: u32,
    pub allow_high_precision_mv: bool,
    pub interp_filter: u8,
    pub refresh_frame_context: bool,
    pub frame_parallel: bool,
    /// frame_context_idx as coded; the frame uses context 0 when `past_independence` is set
    /// (reset_frame_context = 2 still resets the coded index's context).
    pub frame_context_idx: u8,
    pub base_q_idx: u8,
    pub y_dc_delta_q: i8,
    pub uv_dc_delta_q: i8,
    pub uv_ac_delta_q: i8,
    pub lossless: bool,
    pub tile_cols_log2: u8,
    pub tile_rows_log2: u8,
    /// header_size_in_bytes: the compressed header's length.
    pub compressed_size: usize,
    /// Bytes the uncompressed header occupies.
    pub uncompressed_size: usize,
    /// Whether setup_past_independence ran (key, intra-only or error-resilient frames).
    pub past_independence: bool,
}

impl FrameHeader {
    pub fn intra(&self) -> bool {
        self.key_frame || self.intra_only
    }
}

fn sync_code(r: &mut BitReader) -> Result<()> {
    if r.f(8) != 0x49 || r.f(8) != 0x83 || r.f(8) != 0x42 {
        return Err(Error::Invalid("frame sync code"));
    }
    Ok(())
}

fn color_config(r: &mut BitReader, profile: u8, st: &mut HeaderState) -> Result<()> {
    st.bit_depth = if profile >= 2 { if r.flag() { 12 } else { 10 } } else { 8 };
    st.color_space = r.f(3) as u8;
    if st.color_space != 7 {
        // Not sRGB.
        st.color_range = r.bit() as u8;
        if profile == 1 || profile == 3 {
            st.ss_x = r.bit() as u8;
            st.ss_y = r.bit() as u8;
            r.bit(); // reserved_zero
        } else {
            st.ss_x = 1;
            st.ss_y = 1;
        }
    } else {
        st.color_range = 1;
        if profile == 1 || profile == 3 {
            st.ss_x = 0;
            st.ss_y = 0;
            r.bit();
        } else {
            return Err(Error::Invalid("4:4:4 sRGB in profile 0 or 2"));
        }
    }
    Ok(())
}

/// The 2-bit literal -> interp_filter map (§7.2: literal_to_type).
const LITERAL_TO_FILTER: [u8; 4] = [EIGHTTAP_SMOOTH, EIGHTTAP, EIGHTTAP_SHARP, BILINEAR];

/// Parse the uncompressed header (§6.2). Updates the persistent state (colour config, loop filter
/// deltas, segmentation) the way the reference decoder does, including setup_past_independence's
/// reset of the loop filter deltas and segmentation features (§7.2 "setup_past_independence").
pub fn parse_uncompressed(data: &[u8], st: &mut HeaderState) -> Result<FrameHeader> {
    let mut r = BitReader::new(data);
    let mut h = FrameHeader::default();
    if r.f(2) != 2 {
        return Err(Error::Invalid("frame marker"));
    }
    let lo = r.bit();
    let hi = r.bit();
    h.profile = ((hi << 1) | lo) as u8;
    if h.profile == 3 {
        r.bit(); // reserved_zero
    }
    if h.profile == 1 || h.profile == 3 {
        return Err(Error::Unsupported("profile 1/3 (4:2:2, 4:4:0, 4:4:4 or alpha) refused: profiles 0 and 2 only"));
    }
    h.show_existing_frame = r.flag();
    if h.show_existing_frame {
        h.frame_to_show = r.f(3) as u8;
        h.show_frame = true;
        h.uncompressed_size = r.bytes_read();
        if r.overran() {
            return Err(Error::Truncated);
        }
        return Ok(h);
    }
    h.key_frame = r.bit() == 0;
    h.show_frame = r.flag();
    h.error_resilient = r.flag();
    if h.key_frame {
        sync_code(&mut r)?;
        color_config(&mut r, h.profile, st)?;
        h.refresh_frame_flags = 0xff;
        frame_size(&mut r, &mut h);
        render_size(&mut r, &mut h);
    } else {
        h.intra_only = if h.show_frame { false } else { r.flag() };
        h.reset_frame_context = if h.error_resilient { 0 } else { r.f(2) as u8 };
        if h.intra_only {
            sync_code(&mut r)?;
            if h.profile > 0 {
                color_config(&mut r, h.profile, st)?;
            } else {
                // Profile 0 intra-only frames are 8-bit 4:2:0 BT.601 (normative note in §7.2).
                st.color_space = 1;
                st.color_range = 0;
                st.ss_x = 1;
                st.ss_y = 1;
                st.bit_depth = 8;
            }
            h.refresh_frame_flags = r.f(8) as u8;
            frame_size(&mut r, &mut h);
            render_size(&mut r, &mut h);
        } else {
            h.refresh_frame_flags = r.f(8) as u8;
            for i in 0..3 {
                h.ref_frame_idx[i] = r.f(3) as u8;
                h.sign_bias[LAST_FRAME as usize + i] = r.flag();
            }
            // frame_size_with_refs (§6.2.6).
            let mut found = false;
            for i in 0..3 {
                if r.flag() {
                    let (w, hh) = st.ref_dims[h.ref_frame_idx[i] as usize].ok_or(Error::Invalid("size from an empty reference"))?;
                    h.width = w;
                    h.height = hh;
                    found = true;
                    break;
                }
            }
            if !found {
                frame_size(&mut r, &mut h);
            }
            render_size(&mut r, &mut h);
            h.allow_high_precision_mv = r.flag();
            h.interp_filter = if r.flag() { SWITCHABLE } else { LITERAL_TO_FILTER[r.f(2) as usize] };
        }
    }
    if !h.error_resilient {
        h.refresh_frame_context = r.flag();
        h.frame_parallel = r.flag();
    } else {
        h.refresh_frame_context = false;
        h.frame_parallel = true;
    }
    h.frame_context_idx = r.f(2) as u8;
    if h.intra() || h.error_resilient {
        // setup_past_independence, header half.
        h.past_independence = true;
        st.seg.clear_features();
        st.lf.ref_deltas = [1, 0, -1, -1];
        st.lf.mode_deltas = [0, 0];
        st.lf.delta_enabled = true;
        h.sign_bias = [false; 4];
    }
    // loop_filter_params (§6.2.8).
    st.lf.level = r.f(6) as u8;
    st.lf.sharpness = r.f(3) as u8;
    st.lf.delta_enabled = r.flag();
    if st.lf.delta_enabled && r.flag() {
        for i in 0..4 {
            if r.flag() {
                st.lf.ref_deltas[i] = r.su(6) as i8;
            }
        }
        for i in 0..2 {
            if r.flag() {
                st.lf.mode_deltas[i] = r.su(6) as i8;
            }
        }
    }
    // quantization_params (§6.2.9).
    h.base_q_idx = r.f(8) as u8;
    let mut delta_q = || if r.flag() { r.su(4) as i8 } else { 0 };
    h.y_dc_delta_q = delta_q();
    h.uv_dc_delta_q = delta_q();
    h.uv_ac_delta_q = delta_q();
    h.lossless = h.base_q_idx == 0 && h.y_dc_delta_q == 0 && h.uv_dc_delta_q == 0 && h.uv_ac_delta_q == 0;
    // segmentation_params (§6.2.11).
    let seg = &mut st.seg;
    seg.update_map = false;
    seg.enabled = r.flag();
    if seg.enabled {
        seg.update_map = r.flag();
        if seg.update_map {
            for i in 0..7 {
                seg.tree_probs[i] = if r.flag() { r.f(8) as u8 } else { 255 };
            }
            seg.temporal_update = r.flag();
            for i in 0..3 {
                seg.pred_probs[i] = if seg.temporal_update && r.flag() { r.f(8) as u8 } else { 255 };
            }
        }
        if r.flag() {
            // segmentation_update_data
            seg.abs_delta = r.flag();
            seg.feature_enabled = [[false; 4]; 8];
            seg.feature_data = [[0; 4]; 8];
            for i in 0..8 {
                for j in 0..4 {
                    let mut v = 0i32;
                    if r.flag() {
                        seg.feature_enabled[i][j] = true;
                        v = r.f(SEG_FEATURE_BITS[j]) as i32;
                        v = v.min(SEG_FEATURE_MAX[j]);
                        if SEG_FEATURE_SIGNED[j] && r.flag() {
                            v = -v;
                        }
                    }
                    seg.feature_data[i][j] = v as i16;
                }
            }
        }
    }
    // tile_info (§6.2.13).
    let mi_cols = (h.width + 7) >> 3;
    let sb64_cols = (mi_cols + 7) >> 3;
    let mut min_log2 = 0u8;
    while (64u32 << min_log2) < sb64_cols {
        min_log2 += 1;
    }
    let mut max_log2 = 1u8;
    while (sb64_cols >> max_log2) >= 4 {
        max_log2 += 1;
    }
    max_log2 -= 1;
    h.tile_cols_log2 = min_log2;
    while h.tile_cols_log2 < max_log2 && r.flag() {
        h.tile_cols_log2 += 1;
    }
    h.tile_rows_log2 = r.bit() as u8;
    if h.tile_rows_log2 != 0 {
        h.tile_rows_log2 += r.bit() as u8;
    }
    h.compressed_size = r.f(16) as usize;
    h.uncompressed_size = r.bytes_read();
    if r.overran() {
        return Err(Error::Truncated);
    }
    if h.compressed_size == 0 {
        return Err(Error::Invalid("header_size_in_bytes is 0"));
    }
    if h.width == 0 || h.height == 0 {
        return Err(Error::Invalid("frame size"));
    }
    Ok(h)
}

fn frame_size(r: &mut BitReader, h: &mut FrameHeader) {
    h.width = r.f(16) + 1;
    h.height = r.f(16) + 1;
}

fn render_size(r: &mut BitReader, h: &mut FrameHeader) {
    if r.flag() {
        h.render_width = r.f(16) + 1;
        h.render_height = r.f(16) + 1;
    } else {
        h.render_width = h.width;
        h.render_height = h.height;
    }
}

/// What the compressed header decides besides probabilities.
#[derive(Clone, Copy, Debug, Default)]
pub struct CompressedHeader {
    pub tx_mode: u8,
    pub reference_mode: u8,
    pub comp_fixed_ref: i8,
    pub comp_var_ref: [i8; 2],
}

/// Parse the compressed header (§6.3) into `fc` (already loaded from frame_context_idx).
pub fn parse_compressed(data: &[u8], h: &FrameHeader, fc: &mut FrameContext) -> Result<CompressedHeader> {
    let mut d = BoolDecoder::new(data)?;
    let mut c = CompressedHeader::default();
    // read_tx_mode
    c.tx_mode = if h.lossless {
        ONLY_4X4
    } else {
        let mut m = d.literal(2) as u8;
        if m == ALLOW_32X32 {
            m += d.bit() as u8;
        }
        m
    };
    if c.tx_mode == TX_MODE_SELECT {
        for i in 0..2 {
            d.diff_update_prob(&mut fc.tx8[i][0]);
        }
        for i in 0..2 {
            for j in 0..2 {
                d.diff_update_prob(&mut fc.tx16[i][j]);
            }
        }
        for i in 0..2 {
            for j in 0..3 {
                d.diff_update_prob(&mut fc.tx32[i][j]);
            }
        }
    }
    // read_coef_probs
    let max_tx = TX_MODE_TO_BIGGEST_TX_SIZE[c.tx_mode as usize] as usize;
    for t in 0..=max_tx {
        if d.bit() != 0 {
            for i in 0..2 {
                for j in 0..2 {
                    for k in 0..6 {
                        let nctx = if k == 0 { 3 } else { 6 };
                        for l in 0..nctx {
                            for m in 0..3 {
                                d.diff_update_prob(&mut fc.coef[t][i][j][k][l][m]);
                            }
                        }
                    }
                }
            }
        }
    }
    for i in 0..3 {
        d.diff_update_prob(&mut fc.skip[i]);
    }
    if !h.intra() {
        for i in 0..7 {
            for j in 0..3 {
                d.diff_update_prob(&mut fc.inter_mode[i][j]);
            }
        }
        if h.interp_filter == SWITCHABLE {
            for i in 0..4 {
                for j in 0..2 {
                    d.diff_update_prob(&mut fc.interp_filter[i][j]);
                }
            }
        }
        for i in 0..4 {
            d.diff_update_prob(&mut fc.is_inter[i]);
        }
        // frame_reference_mode
        let compound_allowed = h.sign_bias[2] != h.sign_bias[1] || h.sign_bias[3] != h.sign_bias[1];
        c.reference_mode = if compound_allowed {
            if d.bit() != 0 {
                if d.bit() != 0 { REFERENCE_MODE_SELECT } else { COMPOUND_REFERENCE }
            } else {
                SINGLE_REFERENCE
            }
        } else {
            SINGLE_REFERENCE
        };
        if c.reference_mode != SINGLE_REFERENCE {
            // setup_compound_reference_mode
            if h.sign_bias[LAST_FRAME as usize] == h.sign_bias[GOLDEN_FRAME as usize] {
                c.comp_fixed_ref = ALTREF_FRAME;
                c.comp_var_ref = [LAST_FRAME, GOLDEN_FRAME];
            } else if h.sign_bias[LAST_FRAME as usize] == h.sign_bias[ALTREF_FRAME as usize] {
                c.comp_fixed_ref = GOLDEN_FRAME;
                c.comp_var_ref = [LAST_FRAME, ALTREF_FRAME];
            } else {
                c.comp_fixed_ref = LAST_FRAME;
                c.comp_var_ref = [GOLDEN_FRAME, ALTREF_FRAME];
            }
        }
        if c.reference_mode == REFERENCE_MODE_SELECT {
            for i in 0..5 {
                d.diff_update_prob(&mut fc.comp_mode[i]);
            }
        }
        if c.reference_mode != COMPOUND_REFERENCE {
            for i in 0..5 {
                d.diff_update_prob(&mut fc.single_ref[i][0]);
                d.diff_update_prob(&mut fc.single_ref[i][1]);
            }
        }
        if c.reference_mode != SINGLE_REFERENCE {
            for i in 0..5 {
                d.diff_update_prob(&mut fc.comp_ref[i]);
            }
        }
        for j in 0..4 {
            for i in 0..9 {
                d.diff_update_prob(&mut fc.y_mode[j][i]);
            }
        }
        for j in 0..16 {
            for i in 0..3 {
                d.diff_update_prob(&mut fc.partition[j][i]);
            }
        }
        // mv_probs (§6.3.17)
        for j in 0..3 {
            d.update_mv_prob(&mut fc.mv.joints[j]);
        }
        for i in 0..2 {
            let comp = &mut fc.mv.comps[i];
            d.update_mv_prob(&mut comp.sign);
            for j in 0..10 {
                d.update_mv_prob(&mut comp.classes[j]);
            }
            d.update_mv_prob(&mut comp.class0[0]);
            for j in 0..10 {
                d.update_mv_prob(&mut comp.bits[j]);
            }
        }
        for i in 0..2 {
            let comp = &mut fc.mv.comps[i];
            for j in 0..2 {
                for k in 0..3 {
                    d.update_mv_prob(&mut comp.class0_fp[j][k]);
                }
            }
            for k in 0..3 {
                d.update_mv_prob(&mut comp.fp[k]);
            }
        }
        if h.allow_high_precision_mv {
            for i in 0..2 {
                let comp = &mut fc.mv.comps[i];
                d.update_mv_prob(&mut comp.class0_hp);
                d.update_mv_prob(&mut comp.hp);
            }
        }
    }
    if d.overran() {
        return Err(Error::Truncated);
    }
    Ok(c)
}

/// Annex B: split a chunk at its superframe index. A chunk without an index is one frame.
pub fn superframe_frames(data: &[u8]) -> alloc::vec::Vec<&[u8]> {
    let mut out = alloc::vec::Vec::new();
    if let Some(&marker) = data.last() {
        if marker & 0xe0 == 0xc0 {
            let frames = (marker & 7) as usize + 1;
            let mag = ((marker >> 3) & 3) as usize + 1;
            let index_sz = 2 + mag * frames;
            if data.len() >= index_sz && data[data.len() - index_sz] == marker {
                let mut p = data.len() - index_sz + 1;
                let mut off = 0usize;
                let payload_end = data.len() - index_sz;
                for _ in 0..frames {
                    let mut sz = 0usize;
                    for b in 0..mag {
                        sz |= (data[p + b] as usize) << (8 * b);
                    }
                    p += mag;
                    if off + sz > payload_end {
                        break;
                    }
                    if sz > 0 {
                        out.push(&data[off..off + sz]);
                    }
                    off += sz;
                }
                return out;
            }
        }
    }
    out.push(data);
    out
}
