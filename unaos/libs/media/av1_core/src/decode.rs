//! §5.9.11–§5.11 tile decoding for intra frames: decode_tile, decode_partition, decode_block,
//! intra_frame_mode_info (segment id, skip, CDEF index, delta q / lf, y/uv modes, angle deltas,
//! CfL alphas, palette, filter intra), tx size, residual, transform_block, coeffs — and §8.3.2,
//! the CDF selection (context derivation) for every symbol read. Reconstruction calls into
//! [`crate::predict`] and [`crate::transform`].

use crate::bits::ceil_log2;
use crate::cdf::CdfContext;
use crate::obu::{FrameHeader, SequenceHeader};
use crate::refs::{Mv, RefStore};
use crate::symbol::SymbolDecoder;
use crate::tables::*;
use crate::{Error, Result};
use alloc::vec;
use alloc::vec::Vec;

/// One plane of samples (CurrFrame[ plane ]), padded right/bottom so whole transform blocks that
/// straddle the frame edge can be predicted and reconstructed without bounds checks.
#[derive(Clone)]
pub struct Plane {
    pub data: Vec<u16>,
    pub stride: usize,
    /// Allocated height.
    pub rows: usize,
}

impl Plane {
    pub fn new(w: usize, h: usize) -> Plane {
        Plane { data: vec![0; w * h], stride: w, rows: h }
    }
    #[inline]
    pub fn get(&self, x: usize, y: usize) -> u16 {
        self.data[y * self.stride + x]
    }
    #[inline]
    pub fn set(&mut self, x: usize, y: usize, v: u16) {
        self.data[y * self.stride + x] = v;
    }
}

/// Sentinel for cdef_idx = -1.
pub const CDEF_NONE: i8 = -1;

/// Per-frame state shared by the tile decode and the post filters (§7.14–§7.17).
pub struct FrameState {
    pub mi_rows: usize,
    pub mi_cols: usize,
    pub planes: [Plane; 3],
    pub num_planes: usize,
    pub ss_x: usize,
    pub ss_y: usize,
    pub bit_depth: u32,
    // per-mi arrays, row-major mi_rows x mi_cols
    pub y_modes: Vec<u8>,
    pub uv_modes: Vec<u8>,
    pub skips: Vec<u8>,
    pub tx_sizes: Vec<u8>,
    pub inter_tx_sizes: Vec<u8>,
    pub mi_sizes: Vec<u8>,
    pub segment_ids: Vec<u8>,
    pub palette_sizes: [Vec<u8>; 2],
    pub palette_colors: [Vec<[u16; 8]>; 2],
    pub delta_lfs: Vec<[i8; 4]>,
    /// TxTypes at 4x4 luma granularity.
    pub tx_types: Vec<u8>,
    /// LoopfilterTxSizes[ plane ][ row ][ col ] in units of 4x4 samples of that plane.
    pub lf_tx_sizes: [Vec<u8>; 3],
    pub cdef_idx: Vec<i8>,
    /// Tile bounds that were current when each mi was written (for CDEF's is_inside_filter_region
    /// this is the whole frame in this spec version, kept for clarity).
    /// Loop restoration: per plane, per unit.
    pub lr_type: [Vec<u8>; 3],
    pub lr_wiener: [Vec<[[i8; 3]; 2]>; 3],
    pub lr_sgr_set: [Vec<u8>; 3],
    pub lr_sgr_xqd: [Vec<[i16; 2]>; 3],
    pub lr_unit_rows: [usize; 3],
    pub lr_unit_cols: [usize; 3],
    /// Counters of which coding tools the bitstream exercised (for the honest report).
    pub stats: ToolStats,
    // ---- inter (§5.11.5 decode_block stores; §7.9 / §7.10 inputs)
    /// RefFrames[ row ][ col ][ list ]; RefFrames[..][0] == REF_UNWRITTEN until the block is decoded.
    pub ref_frames: Vec<[i8; 2]>,
    pub mvs: Vec<[Mv; 2]>,
    pub is_inters: Vec<u8>,
    pub skip_modes: Vec<u8>,
    pub interp_filters: Vec<[u8; 2]>,
    pub comp_group_idxs: Vec<u8>,
    pub compound_idxs: Vec<u8>,
    /// MotionFieldMvs[ ref ][ y8 ][ x8 ] (§7.9), (MiRows >> 1) x (MiCols >> 1) per reference.
    pub motion_field_mvs: [Vec<Mv>; 8],
    /// PrevSegmentIds (setup_past_independence / load_previous_segment_ids).
    pub prev_segment_ids: Vec<u8>,
    pub frame_width: u32,
    pub frame_height: u32,
    pub upscaled_width: u32,
}

/// RefFrames[ row ][ col ][ 0 ] before the block at (row, col) has been decoded in this frame.
pub const REF_UNWRITTEN: i8 = -2;

/// Which intra tools a decoded frame used — reported by the oracle harness.
#[derive(Debug, Default, Clone)]
pub struct ToolStats {
    pub blocks: u32,
    pub y_modes: [u32; 13],
    pub uv_modes: [u32; 14],
    pub angle_delta_nonzero: u32,
    pub filter_intra: u32,
    pub palette_y: u32,
    pub palette_uv: u32,
    pub cfl: u32,
    pub tx_types: [u32; 16],
    pub tx_sizes: [u32; 19],
    pub lossless_blocks: u32,
    pub skip_blocks: u32,
    pub nonzero_tx_blocks: u32,
    pub delta_q_reads: u32,
    pub segmentation: bool,
    /// Loop-restoration units by decoded type: [NONE, WIENER, SGRPROJ, -].
    pub lr_units: [u32; 4],
    /// Tiles whose exit_symbol() padding check (§8.2.4: the trailing one bit sits exactly where
    /// the arithmetic decoder says it must, zeros after it) passed / failed. A failure means the
    /// symbol decoder lost sync with the encoder somewhere in that tile.
    pub tiles_exit_ok: u32,
    pub tiles_exit_bad: u32,
    // ---- inter tools (counted per block)
    pub inter_blocks: u32,
    pub intrabc_blocks: u32,
    pub compound_blocks: u32,
    pub skip_mode_blocks: u32,
    pub global_mv_blocks: u32,
    pub newmv_blocks: u32,
    pub obmc_blocks: u32,
    pub local_warp_blocks: u32,
    pub global_warp_blocks: u32,
    pub interintra_blocks: u32,
    pub wedge_interintra_blocks: u32,
    pub compound_wedge_blocks: u32,
    pub compound_diffwtd_blocks: u32,
    pub compound_distance_blocks: u32,
    /// interp_filter counts [EIGHTTAP, SMOOTH, SHARP, BILINEAR] (per block, both directions).
    pub interp_filters: [u32; 4],
    pub dual_filter_blocks: u32,
    pub scaled_ref_blocks: u32,
    pub var_tx_splits: u32,
    pub temporal_mvs: bool,
}

impl FrameState {
    pub fn new(seq: &SequenceHeader, h: &FrameHeader) -> FrameState {
        let mi_rows = h.mi_rows as usize;
        let mi_cols = h.mi_cols as usize;
        let cc = &seq.color_config;
        let n = mi_rows * mi_cols;
        let (ssx, ssy) = (cc.subsampling_x as usize, cc.subsampling_y as usize);
        // Pad by a full 128 superblock so transform blocks can overhang the coded area.
        let lw = mi_cols * 4 + 160;
        let lh = mi_rows * 4 + 160;
        let mk = |p: usize| if p == 0 { Plane::new(lw, lh) } else { Plane::new((lw >> ssx) + 8, (lh >> ssy) + 8) };
        FrameState {
            mi_rows,
            mi_cols,
            planes: [mk(0), mk(1), mk(2)],
            num_planes: cc.num_planes,
            ss_x: ssx,
            ss_y: ssy,
            bit_depth: cc.bit_depth,
            y_modes: vec![0; n],
            uv_modes: vec![0; n],
            skips: vec![0; n],
            tx_sizes: vec![0; n],
            inter_tx_sizes: vec![0; n],
            mi_sizes: vec![0; n],
            segment_ids: vec![0; n],
            palette_sizes: [vec![0; n], vec![0; n]],
            palette_colors: [vec![[0; 8]; n], vec![[0; 8]; n]],
            delta_lfs: vec![[0; 4]; n],
            tx_types: vec![0; n],
            lf_tx_sizes: [vec![0; n], vec![0; n], vec![0; n]],
            cdef_idx: vec![CDEF_NONE; n + mi_cols * 32 + 64],
            lr_type: [Vec::new(), Vec::new(), Vec::new()],
            lr_wiener: [Vec::new(), Vec::new(), Vec::new()],
            lr_sgr_set: [Vec::new(), Vec::new(), Vec::new()],
            lr_sgr_xqd: [Vec::new(), Vec::new(), Vec::new()],
            lr_unit_rows: [0; 3],
            lr_unit_cols: [0; 3],
            stats: ToolStats::default(),
            ref_frames: vec![[REF_UNWRITTEN, -1]; n],
            mvs: vec![[[0; 2]; 2]; n],
            is_inters: vec![0; n],
            skip_modes: vec![0; n],
            interp_filters: vec![[0; 2]; n],
            comp_group_idxs: vec![0; n],
            compound_idxs: vec![0; n],
            motion_field_mvs: Default::default(),
            prev_segment_ids: vec![0; n],
            frame_width: h.frame_width,
            frame_height: h.frame_height,
            upscaled_width: h.upscaled_width,
        }
    }
    #[inline]
    pub fn mi(&self, row: usize, col: usize) -> usize {
        row * self.mi_cols + col
    }
}

/// count_units_in_frame (§5.11.57)
pub fn count_units_in_frame(unit_size: u32, frame_size: u32) -> u32 {
    core::cmp::max((frame_size + (unit_size >> 1)) / unit_size, 1)
}

#[inline]
pub fn round2(x: i64, n: u32) -> i64 {
    if n == 0 { x } else { (x + (1i64 << (n - 1))) >> n }
}

/// The intra-frame tile decoder. Field names follow the spec's block-level variables.
pub struct Dec<'a, 'f> {
    pub seq: &'f SequenceHeader,
    pub hdr: &'f FrameHeader,
    pub fs: &'f mut FrameState,
    pub cdf: CdfContext,
    pub sd: SymbolDecoder<'a>,
    // tile
    pub mi_row_start: usize,
    pub mi_row_end: usize,
    pub mi_col_start: usize,
    pub mi_col_end: usize,
    pub current_q_index: i32,
    pub delta_lf: [i32; 4],
    pub read_deltas: bool,
    pub above_level_ctx: [Vec<u8>; 3],
    pub above_dc_ctx: [Vec<u8>; 3],
    pub left_level_ctx: [Vec<u8>; 3],
    pub left_dc_ctx: [Vec<u8>; 3],
    /// BlockDecoded[ plane ][ y + 1 ][ x + 1 ] for y, x in -1..=32.
    pub block_decoded: [[[bool; 34]; 34]; 3],
    pub ref_sgr_xqd: [[i32; 2]; 3],
    pub ref_lr_wiener: [[[i32; 3]; 2]; 3],
    // block
    pub mi_row: usize,
    pub mi_col: usize,
    pub mi_size: usize,
    pub has_chroma: bool,
    pub avail_u: bool,
    pub avail_l: bool,
    pub avail_u_chroma: bool,
    pub avail_l_chroma: bool,
    pub skip: bool,
    pub segment_id: usize,
    pub lossless: bool,
    pub y_mode: usize,
    pub uv_mode: usize,
    pub angle_delta_y: i32,
    pub angle_delta_uv: i32,
    pub use_filter_intra: bool,
    pub filter_intra_mode: usize,
    pub cfl_alpha_u: i32,
    pub cfl_alpha_v: i32,
    pub palette_size_y: usize,
    pub palette_size_uv: usize,
    pub palette_colors_y: [u16; 8],
    pub palette_colors_u: [u16; 8],
    pub palette_colors_v: [u16; 8],
    pub color_map_y: Vec<u8>,  // 64 x 64 (stride 64)
    pub color_map_uv: Vec<u8>, // 64 x 64 (stride 64)
    pub tx_size: usize,
    pub max_luma_w: usize,
    pub max_luma_h: usize,
    pub plane_tx_type: usize,
    pub quant: [i32; 1024],
    /// Post-filter switches (all on for a conformant decode; the oracle toggles them).
    pub filters: crate::image::Filters,
    // ---- frame-level inter inputs
    pub refs: &'f RefStore,
    /// The frame CDFs every tile starts from (§8.2.2).
    pub frame_cdf: CdfContext,
    /// The Saved CDFs from tile context_update_tile_id (§8.2.4), if that tile was decoded.
    pub saved_cdf: Option<CdfContext>,
    pub tile_num: u32,
    // ---- inter block state (§5.11.18-§5.11.33)
    pub is_inter: bool,
    pub use_intrabc: bool,
    pub skip_mode: bool,
    /// RefFrame[ 0..1 ] (INTRA_FRAME = 0, NONE = -1).
    pub ref_frame: [i32; 2],
    pub mv: [Mv; 2],
    pub pred_mv: [Mv; 2],
    pub ref_stack_mv: [[Mv; 2]; 10],
    pub weight_stack: [u32; 10],
    pub num_mv_found: usize,
    pub new_mv_count: usize,
    pub global_mvs: [Mv; 2],
    pub found_match: bool,
    pub close_matches: usize,
    pub total_matches: usize,
    pub new_mv_context: usize,
    pub ref_mv_context: usize,
    pub zero_mv_context: usize,
    pub drl_ctx_stack: [usize; 10],
    pub ref_mv_idx: usize,
    pub motion_mode: usize,
    pub interintra: bool,
    pub interintra_mode: usize,
    pub wedge_interintra: bool,
    pub wedge_index: usize,
    pub wedge_sign: usize,
    pub mask_type: usize,
    pub compound_type: usize,
    pub comp_group_idx: usize,
    pub compound_idx: usize,
    pub interp_filter: [u8; 2],
    pub num_samples: usize,
    pub num_samples_scanned: usize,
    pub cand_list: [[i32; 4]; 8],
    pub local_warp_params: [i32; 6],
    pub local_valid: bool,
    pub left_ref_frame: [i32; 2],
    pub above_ref_frame: [i32; 2],
    pub left_intra: bool,
    pub above_intra: bool,
    pub left_single: bool,
    pub above_single: bool,
    pub above_seg_pred_context: Vec<u8>,
    pub left_seg_pred_context: Vec<u8>,
    /// Mask[ i ][ j ] for wedge / difference-weight / inter-intra blending, stride 128.
    pub mask: Vec<u8>,
    pub fwd_weight: i32,
    pub bck_weight: i32,
    pub inter_round0: u32,
    pub inter_round1: u32,
    pub inter_post_round: u32,
    pub is_inter_intra: bool,
}

macro_rules! sym {
    ($s:ident, $cdf:expr) => {
        $s.sd.read_symbol(&mut $cdf)
    };
}

pub const CFL_SIGN_ZERO_V: usize = 0;
pub const CFL_SIGN_NEG_V: usize = 1;

/// find_tx_size( w, h ) (§5.11.36)
pub fn find_tx_size(w: usize, h: usize) -> usize {
    let mut tx_sz = 0;
    while tx_sz < TX_SIZES_ALL {
        if TX_WIDTH[tx_sz] as usize == w && TX_HEIGHT[tx_sz] as usize == h {
            break;
        }
        tx_sz += 1;
    }
    tx_sz
}

pub fn block_width(bs: usize) -> usize {
    4 * NUM_4X4_BLOCKS_WIDE[bs] as usize
}
pub fn block_height(bs: usize) -> usize {
    4 * NUM_4X4_BLOCKS_HIGH[bs] as usize
}

/// neg_deinterleave (§5.11.9)
fn neg_deinterleave(diff: i32, r: i32, max: i32) -> i32 {
    if r == 0 {
        return diff;
    }
    if r >= max - 1 {
        return max - diff - 1;
    }
    if 2 * r < max {
        if diff <= 2 * r {
            if diff & 1 != 0 {
                return r + ((diff + 1) >> 1);
            } else {
                return r - (diff >> 1);
            }
        }
        diff
    } else {
        if diff <= 2 * (max - r - 1) {
            if diff & 1 != 0 {
                return r + ((diff + 1) >> 1);
            } else {
                return r - (diff >> 1);
            }
        }
        max - (diff + 1)
    }
}

pub fn get_tx_class(tx_type: usize) -> usize {
    if tx_type == V_DCT || tx_type == V_ADST || tx_type == V_FLIPADST {
        TX_CLASS_VERT
    } else if tx_type == H_DCT || tx_type == H_ADST || tx_type == H_FLIPADST {
        TX_CLASS_HORIZ
    } else {
        TX_CLASS_2D
    }
}

fn get_mrow_scan(tx_sz: usize) -> &'static [u8] {
    match tx_sz {
        TX_4X4 => &MROW_SCAN_4X4,
        TX_4X8 => &MROW_SCAN_4X8,
        TX_8X4 => &MROW_SCAN_8X4,
        TX_8X8 => &MROW_SCAN_8X8,
        TX_8X16 => &MROW_SCAN_8X16,
        TX_16X8 => &MROW_SCAN_16X8,
        TX_16X16 => &MROW_SCAN_16X16,
        TX_4X16 => &MROW_SCAN_4X16,
        _ => &MROW_SCAN_16X4,
    }
}
fn get_mcol_scan(tx_sz: usize) -> &'static [u8] {
    match tx_sz {
        TX_4X4 => &MCOL_SCAN_4X4,
        TX_4X8 => &MCOL_SCAN_4X8,
        TX_8X4 => &MCOL_SCAN_8X4,
        TX_8X8 => &MCOL_SCAN_8X8,
        TX_8X16 => &MCOL_SCAN_8X16,
        TX_16X8 => &MCOL_SCAN_16X8,
        TX_16X16 => &MCOL_SCAN_16X16,
        TX_4X16 => &MCOL_SCAN_4X16,
        _ => &MCOL_SCAN_16X4,
    }
}

enum Scan {
    B(&'static [u8]),
    W(&'static [u16]),
}
impl Scan {
    #[inline]
    pub(crate) fn at(&self, i: usize) -> usize {
        match self {
            Scan::B(s) => s[i] as usize,
            Scan::W(s) => s[i] as usize,
        }
    }
}

fn get_default_scan(tx_sz: usize) -> Scan {
    match tx_sz {
        TX_4X4 => Scan::B(&DEFAULT_SCAN_4X4),
        TX_4X8 => Scan::B(&DEFAULT_SCAN_4X8),
        TX_8X4 => Scan::B(&DEFAULT_SCAN_8X4),
        TX_8X8 => Scan::B(&DEFAULT_SCAN_8X8),
        TX_8X16 => Scan::B(&DEFAULT_SCAN_8X16),
        TX_16X8 => Scan::B(&DEFAULT_SCAN_16X8),
        TX_16X16 => Scan::B(&DEFAULT_SCAN_16X16),
        TX_16X32 => Scan::W(&DEFAULT_SCAN_16X32),
        TX_32X16 => Scan::W(&DEFAULT_SCAN_32X16),
        TX_4X16 => Scan::B(&DEFAULT_SCAN_4X16),
        TX_16X4 => Scan::B(&DEFAULT_SCAN_16X4),
        TX_8X32 => Scan::B(&DEFAULT_SCAN_8X32),
        TX_32X8 => Scan::B(&DEFAULT_SCAN_32X8),
        _ => Scan::W(&DEFAULT_SCAN_32X32),
    }
}

impl<'a, 'f> Dec<'a, 'f> {
    pub fn new(
        seq: &'f SequenceHeader,
        hdr: &'f FrameHeader,
        fs: &'f mut FrameState,
        filters: crate::image::Filters,
        refs: &'f RefStore,
        frame_cdf: CdfContext,
    ) -> Self {
        static DUMMY: [u8; 2] = [0, 0];
        let mi_cols = fs.mi_cols;
        let mi_rows = fs.mi_rows;
        Dec {
            seq,
            hdr,
            fs,
            cdf: CdfContext::for_tile(&frame_cdf),
            sd: SymbolDecoder::new(&DUMMY, true).unwrap(),
            mi_row_start: 0,
            mi_row_end: 0,
            mi_col_start: 0,
            mi_col_end: 0,
            current_q_index: hdr.base_q_idx as i32,
            delta_lf: [0; 4],
            read_deltas: false,
            above_level_ctx: [vec![0; mi_cols + 64], vec![0; mi_cols + 64], vec![0; mi_cols + 64]],
            above_dc_ctx: [vec![0; mi_cols + 64], vec![0; mi_cols + 64], vec![0; mi_cols + 64]],
            left_level_ctx: [vec![0; mi_rows + 64], vec![0; mi_rows + 64], vec![0; mi_rows + 64]],
            left_dc_ctx: [vec![0; mi_rows + 64], vec![0; mi_rows + 64], vec![0; mi_rows + 64]],
            block_decoded: [[[false; 34]; 34]; 3],
            ref_sgr_xqd: [[0; 2]; 3],
            ref_lr_wiener: [[[0; 3]; 2]; 3],
            mi_row: 0,
            mi_col: 0,
            mi_size: 0,
            has_chroma: false,
            avail_u: false,
            avail_l: false,
            avail_u_chroma: false,
            avail_l_chroma: false,
            skip: false,
            segment_id: 0,
            lossless: false,
            y_mode: 0,
            uv_mode: 0,
            angle_delta_y: 0,
            angle_delta_uv: 0,
            use_filter_intra: false,
            filter_intra_mode: 0,
            cfl_alpha_u: 0,
            cfl_alpha_v: 0,
            palette_size_y: 0,
            palette_size_uv: 0,
            palette_colors_y: [0; 8],
            palette_colors_u: [0; 8],
            palette_colors_v: [0; 8],
            color_map_y: vec![0; 64 * 64],
            color_map_uv: vec![0; 64 * 64],
            tx_size: 0,
            max_luma_w: 0,
            max_luma_h: 0,
            plane_tx_type: 0,
            quant: [0; 1024],
            filters,
            refs,
            frame_cdf,
            saved_cdf: None,
            tile_num: 0,
            is_inter: false,
            use_intrabc: false,
            skip_mode: false,
            ref_frame: [INTRA_FRAME as i32, NONE as i32],
            mv: [[0; 2]; 2],
            pred_mv: [[0; 2]; 2],
            ref_stack_mv: [[[0; 2]; 2]; 10],
            weight_stack: [0; 10],
            num_mv_found: 0,
            new_mv_count: 0,
            global_mvs: [[0; 2]; 2],
            found_match: false,
            close_matches: 0,
            total_matches: 0,
            new_mv_context: 0,
            ref_mv_context: 0,
            zero_mv_context: 0,
            drl_ctx_stack: [0; 10],
            ref_mv_idx: 0,
            motion_mode: SIMPLE,
            interintra: false,
            interintra_mode: 0,
            wedge_interintra: false,
            wedge_index: 0,
            wedge_sign: 0,
            mask_type: 0,
            compound_type: COMPOUND_AVERAGE,
            comp_group_idx: 0,
            compound_idx: 0,
            interp_filter: [0; 2],
            num_samples: 0,
            num_samples_scanned: 0,
            cand_list: [[0; 4]; 8],
            local_warp_params: [0; 6],
            local_valid: false,
            left_ref_frame: [0; 2],
            above_ref_frame: [0; 2],
            left_intra: false,
            above_intra: false,
            left_single: false,
            above_single: false,
            above_seg_pred_context: vec![0; mi_cols + 64],
            left_seg_pred_context: vec![0; mi_rows + 64],
            mask: vec![0; 128 * 128],
            fwd_weight: 0,
            bck_weight: 0,
            inter_round0: 3,
            inter_round1: 11,
            inter_post_round: 0,
            is_inter_intra: false,
        }
    }

    /// Set up loop-restoration unit storage (§5.9.20 lr_params' consequences).
    pub fn init_lr(&mut self) {
        let h = self.hdr;
        for plane in 0..self.fs.num_planes {
            if h.frame_restoration_type[plane] == RESTORE_NONE as u32 {
                continue;
            }
            let (sx, sy) = if plane == 0 { (0, 0) } else { (self.fs.ss_x as u32, self.fs.ss_y as u32) };
            let unit_size = h.loop_restoration_size[plane];
            let rows = count_units_in_frame(unit_size, round2(h.frame_height as i64, sy) as u32) as usize;
            let cols = count_units_in_frame(unit_size, round2(h.upscaled_width as i64, sx) as u32) as usize;
            self.fs.lr_unit_rows[plane] = rows;
            self.fs.lr_unit_cols[plane] = cols;
            self.fs.lr_type[plane] = vec![0; rows * cols];
            self.fs.lr_wiener[plane] = vec![[[0; 3]; 2]; rows * cols];
            self.fs.lr_sgr_set[plane] = vec![0; rows * cols];
            self.fs.lr_sgr_xqd[plane] = vec![[0; 2]; rows * cols];
        }
    }

    /// tile_group_obu( sz ) for the bytes after the frame header (§5.11.1).
    pub fn decode_tile_group(&mut self, data: &'a [u8]) -> Result<bool> {
        let ti = &self.hdr.tile_info;
        let num_tiles = ti.tile_cols * ti.tile_rows;
        let mut r = crate::bits::BitReader::new(data);
        let mut tile_start_and_end_present_flag = false;
        if num_tiles > 1 {
            tile_start_and_end_present_flag = r.flag()?;
        }
        let (tg_start, tg_end) = if num_tiles == 1 || !tile_start_and_end_present_flag {
            (0, num_tiles - 1)
        } else {
            let tile_bits = ti.tile_cols_log2 + ti.tile_rows_log2;
            (r.f(tile_bits)?, r.f(tile_bits)?)
        };
        r.byte_alignment()?;
        let mut off = r.byte_pos();
        let mut sz = data.len() - off;
        for tile_num in tg_start..=tg_end {
            let tile_row = (tile_num / ti.tile_cols) as usize;
            let tile_col = (tile_num % ti.tile_cols) as usize;
            let last_tile = tile_num == tg_end;
            let tile_size;
            if last_tile {
                tile_size = sz;
            } else {
                let tsb = ti.tile_size_bytes as usize;
                let mut v = 0usize;
                for i in 0..tsb {
                    v |= (*data.get(off + i).ok_or(Error::Truncated)? as usize) << (8 * i);
                }
                tile_size = v + 1;
                off += tsb;
                sz = sz.checked_sub(tile_size + tsb).ok_or(Error::Truncated)?;
            }
            if off + tile_size > data.len() {
                return Err(Error::Truncated);
            }
            self.mi_row_start = ti.mi_row_starts[tile_row] as usize;
            self.mi_row_end = ti.mi_row_starts[tile_row + 1] as usize;
            self.mi_col_start = ti.mi_col_starts[tile_col] as usize;
            self.mi_col_end = ti.mi_col_starts[tile_col + 1] as usize;
            self.current_q_index = self.hdr.base_q_idx as i32;
            // init_symbol(): the Tile CDFs are copies of the frame CDFs.
            self.cdf = CdfContext::for_tile(&self.frame_cdf);
            self.sd = SymbolDecoder::new(&data[off..off + tile_size], self.hdr.disable_cdf_update)?;
            self.tile_num = tile_num;
            self.decode_tile()?;
            // exit_symbol(): keep the adapted CDFs of tile context_update_tile_id.
            if !self.hdr.disable_frame_end_update_cdf && tile_num == ti.context_update_tile_id {
                self.saved_cdf = Some(self.cdf.clone());
            }
            if self.sd.exit_check() {
                self.fs.stats.tiles_exit_ok += 1;
            } else {
                self.fs.stats.tiles_exit_bad += 1;
            }
            off += tile_size;
        }
        Ok(tg_end == num_tiles - 1)
    }

    pub(crate) fn clear_above_context(&mut self) {
        for p in 0..3 {
            self.above_level_ctx[p].iter_mut().for_each(|x| *x = 0);
            self.above_dc_ctx[p].iter_mut().for_each(|x| *x = 0);
        }
        self.above_seg_pred_context.iter_mut().for_each(|x| *x = 0);
    }
    pub(crate) fn clear_left_context(&mut self) {
        for p in 0..3 {
            self.left_level_ctx[p].iter_mut().for_each(|x| *x = 0);
            self.left_dc_ctx[p].iter_mut().for_each(|x| *x = 0);
        }
        self.left_seg_pred_context.iter_mut().for_each(|x| *x = 0);
    }

    pub(crate) fn decode_tile(&mut self) -> Result<()> {
        self.clear_above_context();
        self.delta_lf = [0; 4];
        for plane in 0..self.fs.num_planes {
            for pass in 0..2 {
                self.ref_sgr_xqd[plane][pass] = SGRPROJ_XQD_MID[pass] as i32;
                for i in 0..WIENER_COEFFS {
                    self.ref_lr_wiener[plane][pass][i] = WIENER_TAPS_MID[i] as i32;
                }
            }
        }
        let sb_size = if self.seq.use_128x128_superblock { BLOCK_128X128 } else { BLOCK_64X64 };
        let sb_size4 = NUM_4X4_BLOCKS_WIDE[sb_size] as usize;
        let mut r = self.mi_row_start;
        while r < self.mi_row_end {
            self.clear_left_context();
            let mut c = self.mi_col_start;
            while c < self.mi_col_end {
                self.read_deltas = self.hdr.delta_q_present;
                self.clear_cdef(r, c);
                self.clear_block_decoded_flags(r, c, sb_size4);
                self.read_lr(r, c, sb_size);
                self.decode_partition(r, c, sb_size)?;
                c += sb_size4;
            }
            r += sb_size4;
        }
        Ok(())
    }

    pub(crate) fn clear_cdef(&mut self, r: usize, c: usize) {
        let i = self.fs.mi(r, c);
        self.fs.cdef_idx[i] = CDEF_NONE;
        if self.seq.use_128x128_superblock {
            let s = NUM_4X4_BLOCKS_WIDE[BLOCK_64X64] as usize;
            let mc = self.fs.mi_cols;
            if c + s < mc {
                self.fs.cdef_idx[r * mc + c + s] = CDEF_NONE;
            }
            if r + s < self.fs.mi_rows {
                self.fs.cdef_idx[(r + s) * mc + c] = CDEF_NONE;
                if c + s < mc {
                    self.fs.cdef_idx[(r + s) * mc + c + s] = CDEF_NONE;
                }
            }
        }
    }

    pub(crate) fn clear_block_decoded_flags(&mut self, r: usize, c: usize, sb_size4: usize) {
        for plane in 0..self.fs.num_planes {
            let sub_x = if plane > 0 { self.fs.ss_x } else { 0 };
            let sub_y = if plane > 0 { self.fs.ss_y } else { 0 };
            let sb_width4 = ((self.mi_col_end - c) >> sub_x) as isize;
            let sb_height4 = ((self.mi_row_end - r) >> sub_y) as isize;
            let bd = &mut self.block_decoded[plane];
            for y in -1..=((sb_size4 >> sub_y) as isize) {
                for x in -1..=((sb_size4 >> sub_x) as isize) {
                    let v = if y < 0 && x < sb_width4 {
                        true
                    } else if x < 0 && y < sb_height4 {
                        true
                    } else {
                        false
                    };
                    bd[(y + 1) as usize][(x + 1) as usize] = v;
                }
            }
            bd[(sb_size4 >> sub_y) + 1][0] = false;
        }
    }

    #[inline]
    pub fn bd(&self, plane: usize, y: isize, x: isize) -> bool {
        self.block_decoded[plane][(y + 1) as usize][(x + 1) as usize]
    }

    pub(crate) fn is_inside(&self, r: isize, c: isize) -> bool {
        c >= self.mi_col_start as isize && c < self.mi_col_end as isize && r >= self.mi_row_start as isize && r < self.mi_row_end as isize
    }

    // ---------------------------------------------------------------- loop restoration syntax
    pub(crate) fn read_lr(&mut self, r: usize, c: usize, b_size: usize) {
        if self.hdr.allow_intrabc {
            return;
        }
        let w = NUM_4X4_BLOCKS_WIDE[b_size] as usize;
        let h = NUM_4X4_BLOCKS_HIGH[b_size] as usize;
        for plane in 0..self.fs.num_planes {
            if self.hdr.frame_restoration_type[plane] != RESTORE_NONE as u32 {
                let sub_x = if plane == 0 { 0 } else { self.fs.ss_x };
                let sub_y = if plane == 0 { 0 } else { self.fs.ss_y };
                let unit_size = self.hdr.loop_restoration_size[plane] as usize;
                let unit_rows = self.fs.lr_unit_rows[plane];
                let unit_cols = self.fs.lr_unit_cols[plane];
                let unit_row_start = (r * (MI_SIZE >> sub_y) + unit_size - 1) / unit_size;
                let unit_row_end = unit_rows.min(((r + h) * (MI_SIZE >> sub_y) + unit_size - 1) / unit_size);
                let (numerator, denominator) = if self.hdr.use_superres {
                    ((MI_SIZE >> sub_x) * self.hdr.superres_denom as usize, unit_size * SUPERRES_NUM)
                } else {
                    (MI_SIZE >> sub_x, unit_size)
                };
                let unit_col_start = (c * numerator + denominator - 1) / denominator;
                let unit_col_end = unit_cols.min(((c + w) * numerator + denominator - 1) / denominator);
                for unit_row in unit_row_start..unit_row_end {
                    for unit_col in unit_col_start..unit_col_end {
                        self.read_lr_unit(plane, unit_row, unit_col);
                    }
                }
            }
        }
    }

    pub(crate) fn read_lr_unit(&mut self, plane: usize, unit_row: usize, unit_col: usize) {
        let frt = self.hdr.frame_restoration_type[plane] as usize;
        let restoration_type = if frt == RESTORE_WIENER {
            if sym!(self, self.cdf.use_wiener) != 0 { RESTORE_WIENER } else { RESTORE_NONE }
        } else if frt == RESTORE_SGRPROJ {
            if sym!(self, self.cdf.use_sgrproj) != 0 { RESTORE_SGRPROJ } else { RESTORE_NONE }
        } else {
            sym!(self, self.cdf.restoration_type)
        };
        let ui = unit_row * self.fs.lr_unit_cols[plane] + unit_col;
        self.fs.lr_type[plane][ui] = restoration_type as u8;
        self.fs.stats.lr_units[restoration_type & 3] += 1;
        if restoration_type == RESTORE_WIENER {
            for pass in 0..2 {
                let first_coeff;
                if plane != 0 {
                    first_coeff = 1;
                    self.fs.lr_wiener[plane][ui][pass][0] = 0;
                } else {
                    first_coeff = 0;
                }
                for j in first_coeff..3 {
                    let min = WIENER_TAPS_MIN[j] as i32;
                    let max = WIENER_TAPS_MAX[j] as i32;
                    let k = WIENER_TAPS_K[j] as u32;
                    let v = self.decode_signed_subexp_with_ref_bool(min, max + 1, k, self.ref_lr_wiener[plane][pass][j]);
                    self.fs.lr_wiener[plane][ui][pass][j] = v as i8;
                    self.ref_lr_wiener[plane][pass][j] = v;
                }
            }
        } else if restoration_type == RESTORE_SGRPROJ {
            let lr_sgr_set = self.sd.read_literal(SGRPROJ_PARAMS_BITS as u32) as usize;
            self.fs.lr_sgr_set[plane][ui] = lr_sgr_set as u8;
            for i in 0..2 {
                let radius = SGR_PARAMS[lr_sgr_set][i * 2];
                let min = SGRPROJ_XQD_MIN[i] as i32;
                let max = SGRPROJ_XQD_MAX[i] as i32;
                let v;
                if radius != 0 {
                    v = self.decode_signed_subexp_with_ref_bool(min, max + 1, SGRPROJ_PRJ_SUBEXP_K as u32, self.ref_sgr_xqd[plane][i]);
                } else if i == 1 {
                    v = ((1 << SGRPROJ_PRJ_BITS) - self.ref_sgr_xqd[plane][0]).clamp(min, max);
                } else {
                    v = 0;
                }
                self.fs.lr_sgr_xqd[plane][ui][i] = v as i16;
                self.ref_sgr_xqd[plane][i] = v;
            }
        }
    }

    pub(crate) fn decode_signed_subexp_with_ref_bool(&mut self, low: i32, high: i32, k: u32, r: i32) -> i32 {
        let x = self.decode_unsigned_subexp_with_ref_bool(high - low, k, r - low);
        x + low
    }
    pub(crate) fn decode_unsigned_subexp_with_ref_bool(&mut self, mx: i32, k: u32, r: i32) -> i32 {
        let v = self.decode_subexp_bool(mx, k);
        if (r << 1) <= mx { inverse_recenter(r, v) } else { mx - 1 - inverse_recenter(mx - 1 - r, v) }
    }
    pub(crate) fn decode_subexp_bool(&mut self, num_syms: i32, k: u32) -> i32 {
        let mut i = 0;
        let mut mk = 0;
        loop {
            let b2 = if i != 0 { k + i - 1 } else { k };
            let a = 1 << b2;
            if num_syms <= mk + 3 * a {
                return self.sd.read_ns((num_syms - mk) as u32) as i32 + mk;
            } else if self.sd.read_literal(1) != 0 {
                i += 1;
                mk += a;
            } else {
                return self.sd.read_literal(b2) as i32 + mk;
            }
        }
    }

    // ---------------------------------------------------------------- partition
    pub(crate) fn decode_partition(&mut self, r: usize, c: usize, b_size: usize) -> Result<()> {
        if r >= self.fs.mi_rows || c >= self.fs.mi_cols {
            return Ok(());
        }
        let avail_u = self.is_inside(r as isize - 1, c as isize);
        let avail_l = self.is_inside(r as isize, c as isize - 1);
        let num4x4 = NUM_4X4_BLOCKS_WIDE[b_size] as usize;
        let half_block4x4 = num4x4 >> 1;
        let quarter_block4x4 = half_block4x4 >> 1;
        let has_rows = (r + half_block4x4) < self.fs.mi_rows;
        let has_cols = (c + half_block4x4) < self.fs.mi_cols;
        let partition = if b_size < BLOCK_8X8 {
            PARTITION_NONE
        } else {
            // ctx (§8.3.2 partition)
            let bsl = MI_WIDTH_LOG2[b_size] as usize;
            let above = avail_u && (MI_WIDTH_LOG2[self.fs.mi_sizes[self.fs.mi(r - 1, c)] as usize] as usize) < bsl;
            let left = avail_l && (MI_HEIGHT_LOG2[self.fs.mi_sizes[self.fs.mi(r, c - 1)] as usize] as usize) < bsl;
            let ctx = left as usize * 2 + above as usize;
            if has_rows && has_cols {
                match bsl {
                    1 => sym!(self, self.cdf.partition_w8[ctx]),
                    2 => sym!(self, self.cdf.partition_w16[ctx]),
                    3 => sym!(self, self.cdf.partition_w32[ctx]),
                    4 => sym!(self, self.cdf.partition_w64[ctx]),
                    _ => sym!(self, self.cdf.partition_w128[ctx]),
                }
            } else if has_cols {
                let pc: &[u16] = self.partition_cdf(bsl, ctx);
                let p = |i: usize| -> i32 { pc[i] as i32 - if i > 0 { pc[i - 1] as i32 } else { 0 } };
                let mut psum = p(PARTITION_VERT) + p(PARTITION_SPLIT) + p(PARTITION_HORZ_A) + p(PARTITION_VERT_A) + p(PARTITION_VERT_B);
                if b_size != BLOCK_128X128 {
                    psum += p(PARTITION_VERT_4);
                }
                let mut cdf = [((1 << 15) - psum) as u16, 1 << 15, 0];
                // read_bool-like temporary cdf: adaptation result discarded
                let split_or_horz = self.sd.read_symbol(&mut cdf);
                if split_or_horz != 0 { PARTITION_SPLIT } else { PARTITION_HORZ }
            } else if has_rows {
                let pc: &[u16] = self.partition_cdf(bsl, ctx);
                let p = |i: usize| -> i32 { pc[i] as i32 - if i > 0 { pc[i - 1] as i32 } else { 0 } };
                let mut psum = p(PARTITION_HORZ) + p(PARTITION_SPLIT) + p(PARTITION_HORZ_A) + p(PARTITION_HORZ_B) + p(PARTITION_VERT_A);
                if b_size != BLOCK_128X128 {
                    psum += p(PARTITION_HORZ_4);
                }
                let mut cdf = [((1 << 15) - psum) as u16, 1 << 15, 0];
                let split_or_vert = self.sd.read_symbol(&mut cdf);
                if split_or_vert != 0 { PARTITION_SPLIT } else { PARTITION_VERT }
            } else {
                PARTITION_SPLIT
            }
        };
        let sub_size = PARTITION_SUBSIZE[partition][b_size] as usize;
        let split_size = PARTITION_SUBSIZE[PARTITION_SPLIT][b_size] as usize;
        let (hb, qb) = (half_block4x4, quarter_block4x4);
        match partition {
            PARTITION_NONE => self.decode_block(r, c, sub_size)?,
            PARTITION_HORZ => {
                self.decode_block(r, c, sub_size)?;
                if has_rows {
                    self.decode_block(r + hb, c, sub_size)?;
                }
            }
            PARTITION_VERT => {
                self.decode_block(r, c, sub_size)?;
                if has_cols {
                    self.decode_block(r, c + hb, sub_size)?;
                }
            }
            PARTITION_SPLIT => {
                self.decode_partition(r, c, sub_size)?;
                self.decode_partition(r, c + hb, sub_size)?;
                self.decode_partition(r + hb, c, sub_size)?;
                self.decode_partition(r + hb, c + hb, sub_size)?;
            }
            PARTITION_HORZ_A => {
                self.decode_block(r, c, split_size)?;
                self.decode_block(r, c + hb, split_size)?;
                self.decode_block(r + hb, c, sub_size)?;
            }
            PARTITION_HORZ_B => {
                self.decode_block(r, c, sub_size)?;
                self.decode_block(r + hb, c, split_size)?;
                self.decode_block(r + hb, c + hb, split_size)?;
            }
            PARTITION_VERT_A => {
                self.decode_block(r, c, split_size)?;
                self.decode_block(r + hb, c, split_size)?;
                self.decode_block(r, c + hb, sub_size)?;
            }
            PARTITION_VERT_B => {
                self.decode_block(r, c, sub_size)?;
                self.decode_block(r, c + hb, split_size)?;
                self.decode_block(r + hb, c + hb, split_size)?;
            }
            PARTITION_HORZ_4 => {
                self.decode_block(r, c, sub_size)?;
                self.decode_block(r + qb, c, sub_size)?;
                self.decode_block(r + qb * 2, c, sub_size)?;
                if r + qb * 3 < self.fs.mi_rows {
                    self.decode_block(r + qb * 3, c, sub_size)?;
                }
            }
            _ => {
                self.decode_block(r, c, sub_size)?;
                self.decode_block(r, c + qb, sub_size)?;
                self.decode_block(r, c + qb * 2, sub_size)?;
                if c + qb * 3 < self.fs.mi_cols {
                    self.decode_block(r, c + qb * 3, sub_size)?;
                }
            }
        }
        Ok(())
    }

    pub(crate) fn partition_cdf(&self, bsl: usize, ctx: usize) -> &[u16] {
        match bsl {
            1 => &self.cdf.partition_w8[ctx],
            2 => &self.cdf.partition_w16[ctx],
            3 => &self.cdf.partition_w32[ctx],
            4 => &self.cdf.partition_w64[ctx],
            _ => &self.cdf.partition_w128[ctx],
        }
    }

    // ---------------------------------------------------------------- block
    pub(crate) fn decode_block(&mut self, r: usize, c: usize, sub_size: usize) -> Result<()> {
        self.mi_row = r;
        self.mi_col = c;
        self.mi_size = sub_size;
        let bw4 = NUM_4X4_BLOCKS_WIDE[sub_size] as usize;
        let bh4 = NUM_4X4_BLOCKS_HIGH[sub_size] as usize;
        let (ssx, ssy) = (self.fs.ss_x, self.fs.ss_y);
        if bh4 == 1 && ssy != 0 && (r & 1) == 0 {
            self.has_chroma = false;
        } else if bw4 == 1 && ssx != 0 && (c & 1) == 0 {
            self.has_chroma = false;
        } else {
            self.has_chroma = self.fs.num_planes > 1;
        }
        self.avail_u = self.is_inside(r as isize - 1, c as isize);
        self.avail_l = self.is_inside(r as isize, c as isize - 1);
        self.avail_u_chroma = self.avail_u;
        self.avail_l_chroma = self.avail_l;
        if self.has_chroma {
            if ssy != 0 && bh4 == 1 {
                self.avail_u_chroma = self.is_inside(r as isize - 2, c as isize);
            }
            if ssx != 0 && bw4 == 1 {
                self.avail_l_chroma = self.is_inside(r as isize, c as isize - 2);
            }
        } else {
            self.avail_u_chroma = false;
            self.avail_l_chroma = false;
        }
        // per-block defaults (the syntax only sets what it reads)
        self.use_intrabc = false;
        self.is_inter = false;
        self.skip_mode = false;
        self.motion_mode = SIMPLE;
        self.compound_type = COMPOUND_AVERAGE;
        self.interintra = false;
        self.wedge_interintra = false;
        self.comp_group_idx = 0;
        self.compound_idx = 0;
        self.interp_filter = [0; 2];
        self.use_filter_intra = false;
        self.angle_delta_y = 0;
        self.angle_delta_uv = 0;
        self.palette_size_y = 0;
        self.palette_size_uv = 0;
        self.ref_frame = [INTRA_FRAME as i32, NONE as i32];
        self.mv = [[0; 2]; 2];
        // mode_info()
        if self.hdr.frame_is_intra {
            self.intra_frame_mode_info()?;
        } else {
            self.inter_frame_mode_info()?;
        }
        self.palette_tokens();
        self.read_block_tx_size();
        if self.skip {
            self.reset_block_context(bw4, bh4);
        }
        let is_compound = self.ref_frame[1] > INTRA_FRAME as i32;
        let rows = bh4.min(self.fs.mi_rows - r);
        let cols = bw4.min(self.fs.mi_cols - c);
        for y in 0..rows {
            for x in 0..cols {
                let i = self.fs.mi(r + y, c + x);
                self.fs.y_modes[i] = self.y_mode as u8;
                if self.ref_frame[0] == INTRA_FRAME as i32 && self.has_chroma {
                    self.fs.uv_modes[i] = self.uv_mode as u8;
                }
                self.fs.ref_frames[i] = [self.ref_frame[0] as i8, self.ref_frame[1] as i8];
                if self.is_inter {
                    if !self.use_intrabc {
                        self.fs.comp_group_idxs[i] = self.comp_group_idx as u8;
                        self.fs.compound_idxs[i] = self.compound_idx as u8;
                    }
                    self.fs.interp_filters[i] = self.interp_filter;
                    self.fs.mvs[i][0] = self.mv[0];
                    if is_compound {
                        self.fs.mvs[i][1] = self.mv[1];
                    }
                }
            }
        }
        self.block_stats();
        self.compute_prediction();
        self.residual()?;
        for y in 0..rows {
            for x in 0..cols {
                let i = self.fs.mi(r + y, c + x);
                self.fs.is_inters[i] = self.is_inter as u8;
                self.fs.skip_modes[i] = self.skip_mode as u8;
                self.fs.skips[i] = self.skip as u8;
                self.fs.tx_sizes[i] = self.tx_size as u8;
                self.fs.mi_sizes[i] = self.mi_size as u8;
                self.fs.segment_ids[i] = self.segment_id as u8;
                self.fs.palette_sizes[0][i] = self.palette_size_y as u8;
                self.fs.palette_sizes[1][i] = self.palette_size_uv as u8;
                self.fs.palette_colors[0][i] = self.palette_colors_y;
                self.fs.palette_colors[1][i] = self.palette_colors_u;
                self.fs.delta_lfs[i] = [self.delta_lf[0] as i8, self.delta_lf[1] as i8, self.delta_lf[2] as i8, self.delta_lf[3] as i8];
            }
        }
        Ok(())
    }

    pub(crate) fn block_stats(&mut self) {
        let st = &mut self.fs.stats;
        st.blocks += 1;
        if self.is_inter {
            st.inter_blocks += 1;
            if self.use_intrabc {
                st.intrabc_blocks += 1;
            } else {
                if self.ref_frame[1] > INTRA_FRAME as i32 {
                    st.compound_blocks += 1;
                }
                if self.skip_mode {
                    st.skip_mode_blocks += 1;
                }
                if self.y_mode == GLOBALMV || self.y_mode == GLOBAL_GLOBALMV {
                    st.global_mv_blocks += 1;
                }
                if matches!(self.y_mode, NEWMV | NEW_NEWMV | NEAREST_NEWMV | NEW_NEARESTMV | NEAR_NEWMV | NEW_NEARMV) {
                    st.newmv_blocks += 1;
                }
                if self.motion_mode == OBMC {
                    st.obmc_blocks += 1;
                }
                if self.motion_mode == LOCALWARP {
                    st.local_warp_blocks += 1;
                }
                if self.interintra {
                    st.interintra_blocks += 1;
                    if self.wedge_interintra {
                        st.wedge_interintra_blocks += 1;
                    }
                }
                if self.ref_frame[1] > INTRA_FRAME as i32 {
                    match self.compound_type {
                        COMPOUND_WEDGE => st.compound_wedge_blocks += 1,
                        COMPOUND_DIFFWTD => st.compound_diffwtd_blocks += 1,
                        COMPOUND_DISTANCE => st.compound_distance_blocks += 1,
                        _ => {}
                    }
                }
                st.interp_filters[self.interp_filter[0] as usize & 3] += 1;
                if self.interp_filter[0] != self.interp_filter[1] {
                    st.dual_filter_blocks += 1;
                }
            }
        } else {
            st.y_modes[self.y_mode] += 1;
            if self.has_chroma {
                st.uv_modes[self.uv_mode] += 1;
            }
            if self.angle_delta_y != 0 || self.angle_delta_uv != 0 {
                st.angle_delta_nonzero += 1;
            }
            if self.use_filter_intra {
                st.filter_intra += 1;
            }
            if self.palette_size_y > 0 {
                st.palette_y += 1;
            }
            if self.palette_size_uv > 0 {
                st.palette_uv += 1;
            }
            if self.has_chroma && self.uv_mode == UV_CFL_PRED {
                st.cfl += 1;
            }
        }
        if self.lossless {
            st.lossless_blocks += 1;
        }
        if self.skip {
            st.skip_blocks += 1;
        }
    }

    /// compute_prediction() (§5.11.33): inter / inter-intra prediction for the whole block
    /// (intra blocks are predicted per transform block in transform_block).
    pub(crate) fn compute_prediction(&mut self) {
        let sb_mask = if self.seq.use_128x128_superblock { 31 } else { 15 };
        let sub_block_mi_row = self.mi_row & sb_mask;
        let sub_block_mi_col = self.mi_col & sb_mask;
        let planes = 1 + self.has_chroma as usize * 2;
        for plane in 0..planes {
            let plane_sz = self.get_plane_residual_size(self.mi_size, plane);
            let num4x4_w = NUM_4X4_BLOCKS_WIDE[plane_sz] as usize;
            let num4x4_h = NUM_4X4_BLOCKS_HIGH[plane_sz] as usize;
            let log2w = MI_SIZE_LOG2 as u32 + MI_WIDTH_LOG2[plane_sz] as u32;
            let log2h = MI_SIZE_LOG2 as u32 + MI_HEIGHT_LOG2[plane_sz] as u32;
            let sub_x = if plane > 0 { self.fs.ss_x } else { 0 };
            let sub_y = if plane > 0 { self.fs.ss_y } else { 0 };
            let base_x = (self.mi_col >> sub_x) * MI_SIZE;
            let base_y = (self.mi_row >> sub_y) * MI_SIZE;
            let mut cand_row = (self.mi_row >> sub_y) << sub_y;
            let mut cand_col = (self.mi_col >> sub_x) << sub_x;
            self.is_inter_intra = self.is_inter && self.ref_frame[1] == INTRA_FRAME as i32;
            if self.is_inter_intra {
                let mode = match self.interintra_mode {
                    II_DC_PRED => DC_PRED,
                    II_V_PRED => V_PRED,
                    II_H_PRED => H_PRED,
                    _ => SMOOTH_PRED,
                };
                let bry = (sub_block_mi_row >> sub_y) as isize;
                let brx = (sub_block_mi_col >> sub_x) as isize;
                let have_ar = self.bd(plane, bry - 1, brx + num4x4_w as isize);
                let have_bl = self.bd(plane, bry + num4x4_h as isize, brx - 1);
                let hl = if plane == 0 { self.avail_l } else { self.avail_l_chroma };
                let ha = if plane == 0 { self.avail_u } else { self.avail_u_chroma };
                self.predict_intra(plane, base_x, base_y, hl, ha, have_ar, have_bl, mode, log2w, log2h);
            }
            if self.is_inter {
                let mut pred_w = block_width(self.mi_size) >> sub_x;
                let mut pred_h = block_height(self.mi_size) >> sub_y;
                let mut some_use_intra = false;
                for rr in 0..(num4x4_h << sub_y) {
                    for cc in 0..(num4x4_w << sub_x) {
                        let (y, x) = (cand_row + rr, cand_col + cc);
                        if y < self.fs.mi_rows && x < self.fs.mi_cols && self.fs.ref_frames[self.fs.mi(y, x)][0] == INTRA_FRAME as i8 {
                            some_use_intra = true;
                        }
                    }
                }
                if some_use_intra {
                    pred_w = num4x4_w * 4;
                    pred_h = num4x4_h * 4;
                    cand_row = self.mi_row;
                    cand_col = self.mi_col;
                }
                let mut rr = 0;
                let mut y = 0;
                while y < num4x4_h * 4 {
                    let mut cc = 0;
                    let mut x = 0;
                    while x < num4x4_w * 4 {
                        self.predict_inter(plane, base_x + x, base_y + y, pred_w, pred_h, cand_row + rr, cand_col + cc);
                        x += pred_w;
                        cc += 1;
                    }
                    y += pred_h;
                    rr += 1;
                }
            }
        }
    }

    pub(crate) fn reset_block_context(&mut self, bw4: usize, bh4: usize) {
        let planes = if self.has_chroma { 3 } else { 1 };
        for plane in 0..planes {
            let sub_x = if plane > 0 { self.fs.ss_x } else { 0 };
            let sub_y = if plane > 0 { self.fs.ss_y } else { 0 };
            for i in (self.mi_col >> sub_x)..((self.mi_col + bw4) >> sub_x) {
                self.above_level_ctx[plane][i] = 0;
                self.above_dc_ctx[plane][i] = 0;
            }
            for i in (self.mi_row >> sub_y)..((self.mi_row + bh4) >> sub_y) {
                self.left_level_ctx[plane][i] = 0;
                self.left_dc_ctx[plane][i] = 0;
            }
        }
    }

    // ---------------------------------------------------------------- mode info
    pub(crate) fn intra_frame_mode_info(&mut self) -> Result<()> {
        self.skip = false;
        if self.hdr.seg_id_pre_skip {
            self.intra_segment_id();
        }
        self.read_skip();
        if !self.hdr.seg_id_pre_skip {
            self.intra_segment_id();
        }
        self.read_cdef();
        self.read_delta_qindex();
        self.read_delta_lf();
        self.read_deltas = false;
        self.ref_frame = [INTRA_FRAME as i32, NONE as i32];
        if self.hdr.allow_intrabc {
            self.use_intrabc = sym!(self, self.cdf.intrabc) != 0;
        } else {
            self.use_intrabc = false;
        }
        if self.use_intrabc {
            self.is_inter = true;
            self.y_mode = DC_PRED;
            self.uv_mode = DC_PRED;
            self.motion_mode = SIMPLE;
            self.compound_type = COMPOUND_AVERAGE;
            self.palette_size_y = 0;
            self.palette_size_uv = 0;
            self.interp_filter = [BILINEAR as u8, BILINEAR as u8];
            self.find_mv_stack(false);
            self.assign_mv(false);
            return Ok(());
        }
        self.is_inter = false;
        // intra_frame_y_mode
        let above_mode = if self.avail_u { self.fs.y_modes[self.fs.mi(self.mi_row - 1, self.mi_col)] as usize } else { DC_PRED };
        let left_mode = if self.avail_l { self.fs.y_modes[self.fs.mi(self.mi_row, self.mi_col - 1)] as usize } else { DC_PRED };
        let abovemode = INTRA_MODE_CONTEXT[above_mode] as usize;
        let leftmode = INTRA_MODE_CONTEXT[left_mode] as usize;
        self.y_mode = sym!(self, self.cdf.intra_frame_y_mode[abovemode][leftmode]);
        self.intra_angle_info_y();
        if self.has_chroma {
            let cfl_allowed = if self.lossless && self.get_plane_residual_size(self.mi_size, 1) == BLOCK_4X4 {
                true
            } else {
                !self.lossless && block_width(self.mi_size).max(block_height(self.mi_size)) <= 32
            };
            let ym = self.y_mode;
            self.uv_mode = if cfl_allowed {
                sym!(self, self.cdf.uv_mode_cfl_allowed[ym])
            } else {
                sym!(self, self.cdf.uv_mode_cfl_not_allowed[ym])
            };
            if self.uv_mode == UV_CFL_PRED {
                self.read_cfl_alphas();
            }
            self.intra_angle_info_uv();
        } else {
            self.uv_mode = DC_PRED;
        }
        self.palette_size_y = 0;
        self.palette_size_uv = 0;
        if self.mi_size >= BLOCK_8X8
            && block_width(self.mi_size) <= 64
            && block_height(self.mi_size) <= 64
            && self.hdr.allow_screen_content_tools
        {
            self.palette_mode_info();
        }
        self.filter_intra_mode_info();
        Ok(())
    }

    pub(crate) fn intra_segment_id(&mut self) {
        if self.hdr.segmentation_enabled {
            self.read_segment_id();
        } else {
            self.segment_id = 0;
        }
        self.lossless = self.hdr.lossless_array[self.segment_id];
    }

    pub(crate) fn read_segment_id(&mut self) {
        let (r, c) = (self.mi_row, self.mi_col);
        let prev_ul: i32 = if self.avail_u && self.avail_l { self.fs.segment_ids[self.fs.mi(r - 1, c - 1)] as i32 } else { -1 };
        let prev_u: i32 = if self.avail_u { self.fs.segment_ids[self.fs.mi(r - 1, c)] as i32 } else { -1 };
        let prev_l: i32 = if self.avail_l { self.fs.segment_ids[self.fs.mi(r, c - 1)] as i32 } else { -1 };
        let pred = if prev_u == -1 {
            if prev_l == -1 { 0 } else { prev_l }
        } else if prev_l == -1 {
            prev_u
        } else if prev_ul == prev_u {
            prev_u
        } else {
            prev_l
        };
        if self.skip {
            self.segment_id = pred as usize;
        } else {
            let ctx = if prev_ul < 0 {
                0
            } else if prev_ul == prev_u && prev_ul == prev_l {
                2
            } else if prev_ul == prev_u || prev_ul == prev_l || prev_u == prev_l {
                1
            } else {
                0
            };
            let s = sym!(self, self.cdf.segment_id[ctx]) as i32;
            let v = neg_deinterleave(s, pred, self.hdr.last_active_seg_id as i32 + 1);
            self.segment_id = v.clamp(0, 7) as usize;
            self.fs.stats.segmentation = true;
        }
    }

    pub(crate) fn seg_feature_active(&self, feature: usize) -> bool {
        self.hdr.seg_feature_active_idx(self.segment_id, feature)
    }

    pub(crate) fn read_skip(&mut self) {
        if self.hdr.seg_id_pre_skip && self.seg_feature_active(SEG_LVL_SKIP) {
            self.skip = true;
        } else {
            let mut ctx = 0;
            if self.avail_u {
                ctx += self.fs.skips[self.fs.mi(self.mi_row - 1, self.mi_col)] as usize;
            }
            if self.avail_l {
                ctx += self.fs.skips[self.fs.mi(self.mi_row, self.mi_col - 1)] as usize;
            }
            self.skip = sym!(self, self.cdf.skip[ctx]) != 0;
        }
    }

    pub(crate) fn read_cdef(&mut self) {
        if self.skip || self.hdr.coded_lossless || !self.seq.enable_cdef || self.hdr.allow_intrabc {
            return;
        }
        let cdef_size4 = NUM_4X4_BLOCKS_WIDE[BLOCK_64X64] as usize;
        let cdef_mask4 = !(cdef_size4 - 1);
        let r = self.mi_row & cdef_mask4;
        let c = self.mi_col & cdef_mask4;
        let i = self.fs.mi(r, c);
        if self.fs.cdef_idx[i] == CDEF_NONE {
            let v = self.sd.read_literal(self.hdr.cdef_bits) as i8;
            self.fs.cdef_idx[i] = v;
            let w4 = NUM_4X4_BLOCKS_WIDE[self.mi_size] as usize;
            let h4 = NUM_4X4_BLOCKS_HIGH[self.mi_size] as usize;
            let mut y = r;
            while y < r + h4 {
                let mut x = c;
                while x < c + w4 {
                    if y < self.fs.mi_rows && x < self.fs.mi_cols {
                        let j = self.fs.mi(y, x);
                        self.fs.cdef_idx[j] = v;
                    }
                    x += cdef_size4;
                }
                y += cdef_size4;
            }
        }
    }

    pub(crate) fn read_delta_qindex(&mut self) {
        let sb_size = if self.seq.use_128x128_superblock { BLOCK_128X128 } else { BLOCK_64X64 };
        if self.mi_size == sb_size && self.skip {
            return;
        }
        if self.read_deltas {
            self.fs.stats.delta_q_reads += 1;
            let mut delta_q_abs = sym!(self, self.cdf.delta_q) as i32;
            if delta_q_abs == DELTA_Q_SMALL as i32 {
                let delta_q_rem_bits = self.sd.read_literal(3) + 1;
                let delta_q_abs_bits = self.sd.read_literal(delta_q_rem_bits) as i32;
                delta_q_abs = delta_q_abs_bits + (1 << delta_q_rem_bits) + 1;
            }
            if delta_q_abs != 0 {
                let sign = self.sd.read_literal(1);
                let reduced = if sign != 0 { -delta_q_abs } else { delta_q_abs };
                self.current_q_index = (self.current_q_index + (reduced << self.hdr.delta_q_res)).clamp(1, 255);
            }
        }
    }

    pub(crate) fn read_delta_lf(&mut self) {
        let sb_size = if self.seq.use_128x128_superblock { BLOCK_128X128 } else { BLOCK_64X64 };
        if self.mi_size == sb_size && self.skip {
            return;
        }
        if self.read_deltas && self.hdr.delta_lf_present {
            let mut frame_lf_count = 1;
            if self.hdr.delta_lf_multi {
                frame_lf_count = if self.fs.num_planes > 1 { FRAME_LF_COUNT } else { FRAME_LF_COUNT - 2 };
            }
            for i in 0..frame_lf_count {
                let delta_lf_abs = if self.hdr.delta_lf_multi {
                    sym!(self, self.cdf.delta_lf_multi[i])
                } else {
                    sym!(self, self.cdf.delta_lf)
                } as i32;
                let delta_lf_abs_v = if delta_lf_abs == DELTA_LF_SMALL as i32 {
                    let n = self.sd.read_literal(3) + 1;
                    let bits = self.sd.read_literal(n) as i32;
                    bits + (1 << n) + 1
                } else {
                    delta_lf_abs
                };
                if delta_lf_abs_v != 0 {
                    let sign = self.sd.read_literal(1);
                    let reduced = if sign != 0 { -delta_lf_abs_v } else { delta_lf_abs_v };
                    self.delta_lf[i] = (self.delta_lf[i] + (reduced << self.hdr.delta_lf_res))
                        .clamp(-(MAX_LOOP_FILTER as i32), MAX_LOOP_FILTER as i32);
                }
            }
        }
    }

    pub(crate) fn is_directional_mode(mode: usize) -> bool {
        (V_PRED..=D67_PRED).contains(&mode)
    }

    pub(crate) fn intra_angle_info_y(&mut self) {
        self.angle_delta_y = 0;
        if self.mi_size >= BLOCK_8X8 && Self::is_directional_mode(self.y_mode) {
            let m = self.y_mode - V_PRED;
            let v = sym!(self, self.cdf.angle_delta[m]) as i32;
            self.angle_delta_y = v - MAX_ANGLE_DELTA as i32;
        }
    }
    pub(crate) fn intra_angle_info_uv(&mut self) {
        self.angle_delta_uv = 0;
        if self.mi_size >= BLOCK_8X8 && Self::is_directional_mode(self.uv_mode) {
            let m = self.uv_mode - V_PRED;
            let v = sym!(self, self.cdf.angle_delta[m]) as i32;
            self.angle_delta_uv = v - MAX_ANGLE_DELTA as i32;
        }
    }

    pub(crate) fn read_cfl_alphas(&mut self) {
        let cfl_alpha_signs = sym!(self, self.cdf.cfl_sign);
        let sign_u = (cfl_alpha_signs + 1) / 3;
        let sign_v = (cfl_alpha_signs + 1) % 3;
        if sign_u != CFL_SIGN_ZERO_V {
            let ctx = (sign_u - 1) * 3 + sign_v;
            let a = sym!(self, self.cdf.cfl_alpha[ctx]) as i32;
            self.cfl_alpha_u = 1 + a;
            if sign_u == CFL_SIGN_NEG_V {
                self.cfl_alpha_u = -self.cfl_alpha_u;
            }
        } else {
            self.cfl_alpha_u = 0;
        }
        if sign_v != CFL_SIGN_ZERO_V {
            let ctx = (sign_v - 1) * 3 + sign_u;
            let a = sym!(self, self.cdf.cfl_alpha[ctx]) as i32;
            self.cfl_alpha_v = 1 + a;
            if sign_v == CFL_SIGN_NEG_V {
                self.cfl_alpha_v = -self.cfl_alpha_v;
            }
        } else {
            self.cfl_alpha_v = 0;
        }
    }

    pub(crate) fn filter_intra_mode_info(&mut self) {
        self.use_filter_intra = false;
        if self.seq.enable_filter_intra
            && self.y_mode == DC_PRED
            && self.palette_size_y == 0
            && block_width(self.mi_size).max(block_height(self.mi_size)) <= 32
        {
            let ms = self.mi_size;
            self.use_filter_intra = sym!(self, self.cdf.filter_intra[ms]) != 0;
            if self.use_filter_intra {
                self.filter_intra_mode = sym!(self, self.cdf.filter_intra_mode);
            }
        }
    }

    // ---------------------------------------------------------------- palette
    pub(crate) fn get_palette_cache(&self, plane: usize, cache: &mut [u16; 16]) -> usize {
        let (r, c) = (self.mi_row, self.mi_col);
        let mut above_n = 0;
        if (r * MI_SIZE) % 64 != 0 {
            above_n = self.fs.palette_sizes[plane][self.fs.mi(r - 1, c)] as usize;
        }
        let mut left_n = 0;
        if self.avail_l {
            left_n = self.fs.palette_sizes[plane][self.fs.mi(r, c - 1)] as usize;
        }
        let above_c = if above_n > 0 { self.fs.palette_colors[plane][self.fs.mi(r - 1, c)] } else { [0; 8] };
        let left_c = if left_n > 0 { self.fs.palette_colors[plane][self.fs.mi(r, c - 1)] } else { [0; 8] };
        let (mut ai, mut li, mut n) = (0, 0, 0);
        while ai < above_n && li < left_n {
            let a = above_c[ai];
            let l = left_c[li];
            if l < a {
                if n == 0 || l != cache[n - 1] {
                    cache[n] = l;
                    n += 1;
                }
                li += 1;
            } else {
                if n == 0 || a != cache[n - 1] {
                    cache[n] = a;
                    n += 1;
                }
                ai += 1;
                if l == a {
                    li += 1;
                }
            }
        }
        while ai < above_n {
            let v = above_c[ai];
            ai += 1;
            if n == 0 || v != cache[n - 1] {
                cache[n] = v;
                n += 1;
            }
        }
        while li < left_n {
            let v = left_c[li];
            li += 1;
            if n == 0 || v != cache[n - 1] {
                cache[n] = v;
                n += 1;
            }
        }
        n
    }

    pub(crate) fn palette_mode_info(&mut self) {
        let bsize_ctx = MI_WIDTH_LOG2[self.mi_size] as usize + MI_HEIGHT_LOG2[self.mi_size] as usize - 2;
        let bit_depth = self.fs.bit_depth;
        let clip1 = |v: i32| -> u16 { v.clamp(0, (1 << bit_depth) - 1) as u16 };
        if self.y_mode == DC_PRED {
            let mut ctx = 0;
            if self.avail_u && self.fs.palette_sizes[0][self.fs.mi(self.mi_row - 1, self.mi_col)] > 0 {
                ctx += 1;
            }
            if self.avail_l && self.fs.palette_sizes[0][self.fs.mi(self.mi_row, self.mi_col - 1)] > 0 {
                ctx += 1;
            }
            let has_palette_y = sym!(self, self.cdf.palette_y_mode[bsize_ctx][ctx]) != 0;
            if has_palette_y {
                self.palette_size_y = sym!(self, self.cdf.palette_y_size[bsize_ctx]) + 2;
                let mut cache = [0u16; 16];
                let cache_n = self.get_palette_cache(0, &mut cache);
                let mut idx = 0;
                let mut i = 0;
                while i < cache_n && idx < self.palette_size_y {
                    if self.sd.read_literal(1) != 0 {
                        self.palette_colors_y[idx] = cache[i];
                        idx += 1;
                    }
                    i += 1;
                }
                if idx < self.palette_size_y {
                    self.palette_colors_y[idx] = self.sd.read_literal(bit_depth) as u16;
                    idx += 1;
                }
                let mut palette_bits = 0;
                if idx < self.palette_size_y {
                    let min_bits = bit_depth - 3;
                    palette_bits = min_bits + self.sd.read_literal(2);
                }
                while idx < self.palette_size_y {
                    let mut delta = self.sd.read_literal(palette_bits) as i32;
                    delta += 1;
                    self.palette_colors_y[idx] = clip1(self.palette_colors_y[idx - 1] as i32 + delta);
                    let range = (1i32 << bit_depth) - self.palette_colors_y[idx] as i32 - 1;
                    palette_bits = palette_bits.min(ceil_log2(range.max(0) as u32));
                    idx += 1;
                }
                let n = self.palette_size_y;
                self.palette_colors_y[..n].sort_unstable();
            }
        }
        if self.has_chroma && self.uv_mode == DC_PRED {
            let ctx = (self.palette_size_y > 0) as usize;
            let has_palette_uv = sym!(self, self.cdf.palette_uv_mode[ctx]) != 0;
            if has_palette_uv {
                self.palette_size_uv = sym!(self, self.cdf.palette_uv_size[bsize_ctx]) + 2;
                let mut cache = [0u16; 16];
                let cache_n = self.get_palette_cache(1, &mut cache);
                let mut idx = 0;
                let mut i = 0;
                while i < cache_n && idx < self.palette_size_uv {
                    if self.sd.read_literal(1) != 0 {
                        self.palette_colors_u[idx] = cache[i];
                        idx += 1;
                    }
                    i += 1;
                }
                if idx < self.palette_size_uv {
                    self.palette_colors_u[idx] = self.sd.read_literal(bit_depth) as u16;
                    idx += 1;
                }
                let mut palette_bits = 0;
                if idx < self.palette_size_uv {
                    let min_bits = bit_depth - 3;
                    palette_bits = min_bits + self.sd.read_literal(2);
                }
                while idx < self.palette_size_uv {
                    let delta = self.sd.read_literal(palette_bits) as i32;
                    self.palette_colors_u[idx] = clip1(self.palette_colors_u[idx - 1] as i32 + delta);
                    let range = (1i32 << bit_depth) - self.palette_colors_u[idx] as i32;
                    palette_bits = palette_bits.min(ceil_log2(range.max(0) as u32));
                    idx += 1;
                }
                let n = self.palette_size_uv;
                self.palette_colors_u[..n].sort_unstable();
                if self.sd.read_literal(1) != 0 {
                    let min_bits = bit_depth - 4;
                    let max_val = 1i32 << bit_depth;
                    let palette_bits = min_bits + self.sd.read_literal(2);
                    self.palette_colors_v[0] = self.sd.read_literal(bit_depth) as u16;
                    for idx in 1..self.palette_size_uv {
                        let mut delta = self.sd.read_literal(palette_bits) as i32;
                        if delta != 0 && self.sd.read_literal(1) != 0 {
                            delta = -delta;
                        }
                        let mut val = self.palette_colors_v[idx - 1] as i32 + delta;
                        if val < 0 {
                            val += max_val;
                        }
                        if val >= max_val {
                            val -= max_val;
                        }
                        self.palette_colors_v[idx] = clip1(val);
                    }
                } else {
                    for idx in 0..self.palette_size_uv {
                        self.palette_colors_v[idx] = self.sd.read_literal(bit_depth) as u16;
                    }
                }
            }
        }
    }

    pub(crate) fn get_palette_color_context(color_map: &[u8], r: usize, c: usize, n: usize, color_order: &mut [u8; 8]) -> usize {
        let mut scores = [0i32; 8];
        for i in 0..PALETTE_COLORS {
            color_order[i] = i as u8;
        }
        if c > 0 {
            let nb = color_map[r * 64 + c - 1] as usize;
            scores[nb] += 2;
        }
        if r > 0 && c > 0 {
            let nb = color_map[(r - 1) * 64 + c - 1] as usize;
            scores[nb] += 1;
        }
        if r > 0 {
            let nb = color_map[(r - 1) * 64 + c] as usize;
            scores[nb] += 2;
        }
        for i in 0..PALETTE_NUM_NEIGHBORS {
            let mut max_score = scores[i];
            let mut max_idx = i;
            for j in i + 1..n {
                if scores[j] > max_score {
                    max_score = scores[j];
                    max_idx = j;
                }
            }
            if max_idx != i {
                let max_score = scores[max_idx];
                let max_color_order = color_order[max_idx];
                let mut k = max_idx;
                while k > i {
                    scores[k] = scores[k - 1];
                    color_order[k] = color_order[k - 1];
                    k -= 1;
                }
                scores[i] = max_score;
                color_order[i] = max_color_order;
            }
        }
        let mut hash = 0;
        for i in 0..PALETTE_NUM_NEIGHBORS {
            hash += scores[i] * PALETTE_COLOR_HASH_MULTIPLIERS[i] as i32;
        }
        hash as usize
    }

    pub(crate) fn read_palette_color_idx(&mut self, uv: bool, n: usize, ctx: usize) -> usize {
        if !uv {
            match n {
                2 => sym!(self, self.cdf.palette_size_2_y_color[ctx]),
                3 => sym!(self, self.cdf.palette_size_3_y_color[ctx]),
                4 => sym!(self, self.cdf.palette_size_4_y_color[ctx]),
                5 => sym!(self, self.cdf.palette_size_5_y_color[ctx]),
                6 => sym!(self, self.cdf.palette_size_6_y_color[ctx]),
                7 => sym!(self, self.cdf.palette_size_7_y_color[ctx]),
                _ => sym!(self, self.cdf.palette_size_8_y_color[ctx]),
            }
        } else {
            match n {
                2 => sym!(self, self.cdf.palette_size_2_uv_color[ctx]),
                3 => sym!(self, self.cdf.palette_size_3_uv_color[ctx]),
                4 => sym!(self, self.cdf.palette_size_4_uv_color[ctx]),
                5 => sym!(self, self.cdf.palette_size_5_uv_color[ctx]),
                6 => sym!(self, self.cdf.palette_size_6_uv_color[ctx]),
                7 => sym!(self, self.cdf.palette_size_7_uv_color[ctx]),
                _ => sym!(self, self.cdf.palette_size_8_uv_color[ctx]),
            }
        }
    }

    pub(crate) fn read_color_map(&mut self, uv: bool, n: usize, block_width: usize, block_height: usize, onscreen_width: usize, onscreen_height: usize) {
        let mut map = core::mem::take(if uv { &mut self.color_map_uv } else { &mut self.color_map_y });
        map[0] = self.sd.read_ns(n as u32) as u8;
        let mut order = [0u8; 8];
        for i in 1..(onscreen_height + onscreen_width - 1) {
            let jmax = i.min(onscreen_width - 1) as isize;
            let jmin = (i as isize - onscreen_height as isize + 1).max(0);
            let mut j = jmax;
            while j >= jmin {
                let (rr, cc) = (i - j as usize, j as usize);
                let hash = Self::get_palette_color_context(&map, rr, cc, n, &mut order);
                let ctx = PALETTE_COLOR_CONTEXT[hash] as usize;
                let idx = self.read_palette_color_idx(uv, n, ctx);
                map[rr * 64 + cc] = order[idx];
                j -= 1;
            }
        }
        for i in 0..onscreen_height {
            for j in onscreen_width..block_width {
                map[i * 64 + j] = map[i * 64 + onscreen_width - 1];
            }
        }
        for i in onscreen_height..block_height {
            for j in 0..block_width {
                map[i * 64 + j] = map[(onscreen_height - 1) * 64 + j];
            }
        }
        if uv {
            self.color_map_uv = map;
        } else {
            self.color_map_y = map;
        }
    }

    pub(crate) fn palette_tokens(&mut self) {
        let mut block_height = block_height(self.mi_size);
        let mut block_width = block_width(self.mi_size);
        let mut onscreen_height = block_height.min((self.fs.mi_rows - self.mi_row) * MI_SIZE);
        let mut onscreen_width = block_width.min((self.fs.mi_cols - self.mi_col) * MI_SIZE);
        if self.palette_size_y != 0 {
            let n = self.palette_size_y;
            self.read_color_map(false, n, block_width, block_height, onscreen_width, onscreen_height);
        }
        if self.palette_size_uv != 0 {
            block_height >>= self.fs.ss_y;
            block_width >>= self.fs.ss_x;
            onscreen_height >>= self.fs.ss_y;
            onscreen_width >>= self.fs.ss_x;
            if block_width < 4 {
                block_width += 2;
                onscreen_width += 2;
            }
            if block_height < 4 {
                block_height += 2;
                onscreen_height += 2;
            }
            let n = self.palette_size_uv;
            self.read_color_map(true, n, block_width, block_height, onscreen_width, onscreen_height);
        }
    }

    // ---------------------------------------------------------------- tx size
    pub(crate) fn get_above_tx_width(&self, row: usize, col: usize) -> usize {
        if row == self.mi_row {
            if !self.avail_u {
                return 64;
            }
            let i = self.fs.mi(row - 1, col);
            if self.fs.skips[i] != 0 && self.fs.is_inters[i] != 0 {
                return block_width(self.fs.mi_sizes[i] as usize);
            }
        }
        TX_WIDTH[self.fs.inter_tx_sizes[self.fs.mi(row - 1, col)] as usize] as usize
    }
    pub(crate) fn get_left_tx_height(&self, row: usize, col: usize) -> usize {
        if col == self.mi_col {
            if !self.avail_l {
                return 64;
            }
            let i = self.fs.mi(row, col - 1);
            if self.fs.skips[i] != 0 && self.fs.is_inters[i] != 0 {
                return block_height(self.fs.mi_sizes[i] as usize);
            }
        }
        TX_HEIGHT[self.fs.inter_tx_sizes[self.fs.mi(row, col - 1)] as usize] as usize
    }

    pub(crate) fn read_tx_size(&mut self, allow_select: bool) {
        if self.lossless {
            self.tx_size = TX_4X4;
            return;
        }
        let max_rect_tx_size = MAX_TX_SIZE_RECT[self.mi_size] as usize;
        let max_tx_depth = MAX_TX_DEPTH_TABLE[self.mi_size] as usize;
        self.tx_size = max_rect_tx_size;
        if self.mi_size > BLOCK_4X4 && allow_select && self.hdr.tx_mode == TX_MODE_SELECT as u32 {
            // ctx (§8.3.2 tx_depth)
            let max_tx_width = TX_WIDTH[max_rect_tx_size] as usize;
            let max_tx_height = TX_HEIGHT[max_rect_tx_size] as usize;
            let above_w = if self.avail_u {
                let i = self.fs.mi(self.mi_row - 1, self.mi_col);
                if self.fs.is_inters[i] != 0 {
                    block_width(self.fs.mi_sizes[i] as usize)
                } else {
                    self.get_above_tx_width(self.mi_row, self.mi_col)
                }
            } else {
                0
            };
            let left_h = if self.avail_l {
                let i = self.fs.mi(self.mi_row, self.mi_col - 1);
                if self.fs.is_inters[i] != 0 {
                    block_height(self.fs.mi_sizes[i] as usize)
                } else {
                    self.get_left_tx_height(self.mi_row, self.mi_col)
                }
            } else {
                0
            };
            let ctx = (above_w >= max_tx_width) as usize + (left_h >= max_tx_height) as usize;
            let tx_depth = match max_tx_depth {
                4 => sym!(self, self.cdf.tx_64x64[ctx]),
                3 => sym!(self, self.cdf.tx_32x32[ctx]),
                2 => sym!(self, self.cdf.tx_16x16[ctx]),
                _ => sym!(self, self.cdf.tx_8x8[ctx]),
            };
            for _ in 0..tx_depth {
                self.tx_size = SPLIT_TX_SIZE[self.tx_size] as usize;
            }
        }
    }

    /// read_block_tx_size() (§5.11.15)
    pub(crate) fn read_block_tx_size(&mut self) {
        let bw4 = NUM_4X4_BLOCKS_WIDE[self.mi_size] as usize;
        let bh4 = NUM_4X4_BLOCKS_HIGH[self.mi_size] as usize;
        if self.hdr.tx_mode == TX_MODE_SELECT as u32 && self.mi_size > BLOCK_4X4 && self.is_inter && !self.skip && !self.lossless {
            let max_tx_sz = MAX_TX_SIZE_RECT[self.mi_size] as usize;
            let tx_w4 = TX_WIDTH[max_tx_sz] as usize / MI_SIZE;
            let tx_h4 = TX_HEIGHT[max_tx_sz] as usize / MI_SIZE;
            let mut row = self.mi_row;
            while row < self.mi_row + bh4 {
                let mut col = self.mi_col;
                while col < self.mi_col + bw4 {
                    self.read_var_tx_size(row, col, max_tx_sz, 0);
                    col += tx_w4;
                }
                row += tx_h4;
            }
        } else {
            self.read_tx_size(!self.skip || !self.is_inter);
            for row in self.mi_row..(self.mi_row + bh4).min(self.fs.mi_rows) {
                for col in self.mi_col..(self.mi_col + bw4).min(self.fs.mi_cols) {
                    let i = self.fs.mi(row, col);
                    self.fs.inter_tx_sizes[i] = self.tx_size as u8;
                }
            }
        }
    }

    /// read_var_tx_size( row, col, txSz, depth ) (§5.11.17)
    pub(crate) fn read_var_tx_size(&mut self, row: usize, col: usize, tx_sz: usize, depth: usize) {
        if row >= self.fs.mi_rows || col >= self.fs.mi_cols {
            return;
        }
        let txfm_split = if tx_sz == TX_4X4 || depth == MAX_VARTX_DEPTH {
            false
        } else {
            // ctx (§8.3.2 txfm_split)
            let above = (self.get_above_tx_width(row, col) < TX_WIDTH[tx_sz] as usize) as usize;
            let left = (self.get_left_tx_height(row, col) < TX_HEIGHT[tx_sz] as usize) as usize;
            let size = block_width(self.mi_size).max(block_height(self.mi_size)).min(64);
            let max_tx_sz = find_tx_size(size, size);
            let tx_sz_sqr_up = TX_SIZE_SQR_UP[tx_sz] as usize;
            let ctx = (tx_sz_sqr_up != max_tx_sz) as usize * 3 + (TX_SIZES - 1 - max_tx_sz) * 6 + above + left;
            sym!(self, self.cdf.txfm_split[ctx]) != 0
        };
        let w4 = TX_WIDTH[tx_sz] as usize / MI_SIZE;
        let h4 = TX_HEIGHT[tx_sz] as usize / MI_SIZE;
        if txfm_split {
            self.fs.stats.var_tx_splits += 1;
            let sub_tx_sz = SPLIT_TX_SIZE[tx_sz] as usize;
            let step_w = TX_WIDTH[sub_tx_sz] as usize / MI_SIZE;
            let step_h = TX_HEIGHT[sub_tx_sz] as usize / MI_SIZE;
            let mut i = 0;
            while i < h4 {
                let mut j = 0;
                while j < w4 {
                    self.read_var_tx_size(row + i, col + j, sub_tx_sz, depth + 1);
                    j += step_w;
                }
                i += step_h;
            }
        } else {
            for i in 0..h4 {
                for j in 0..w4 {
                    if row + i < self.fs.mi_rows && col + j < self.fs.mi_cols {
                        let k = self.fs.mi(row + i, col + j);
                        self.fs.inter_tx_sizes[k] = tx_sz as u8;
                    }
                }
            }
            self.tx_size = tx_sz;
        }
    }

    // ---------------------------------------------------------------- residual
    pub fn get_plane_residual_size(&self, subsize: usize, plane: usize) -> usize {
        let subx = if plane > 0 { self.fs.ss_x } else { 0 };
        let suby = if plane > 0 { self.fs.ss_y } else { 0 };
        SUBSAMPLED_SIZE[subsize][subx][suby] as usize
    }

    pub(crate) fn get_tx_size(&self, plane: usize, tx_sz: usize) -> usize {
        if plane == 0 {
            return tx_sz;
        }
        let uv_tx = MAX_TX_SIZE_RECT[self.get_plane_residual_size(self.mi_size, plane)] as usize;
        if TX_WIDTH[uv_tx] == 64 || TX_HEIGHT[uv_tx] == 64 {
            if TX_WIDTH[uv_tx] == 16 {
                return TX_16X32;
            }
            if TX_HEIGHT[uv_tx] == 16 {
                return TX_32X16;
            }
            return TX_32X32;
        }
        uv_tx
    }

    /// residual() (§5.11.34)
    pub(crate) fn residual(&mut self) -> Result<()> {
        let width_chunks = (block_width(self.mi_size) >> 6).max(1);
        let height_chunks = (block_height(self.mi_size) >> 6).max(1);
        let mi_size_chunk = if width_chunks > 1 || height_chunks > 1 { BLOCK_64X64 } else { self.mi_size };
        for chunk_y in 0..height_chunks {
            for chunk_x in 0..width_chunks {
                let mi_row_chunk = self.mi_row + (chunk_y << 4);
                let mi_col_chunk = self.mi_col + (chunk_x << 4);
                for plane in 0..(1 + self.has_chroma as usize * 2) {
                    let tx_sz = if self.lossless { TX_4X4 } else { self.get_tx_size(plane, self.tx_size) };
                    let step_x = TX_WIDTH[tx_sz] as usize >> 2;
                    let step_y = TX_HEIGHT[tx_sz] as usize >> 2;
                    let plane_sz = self.get_plane_residual_size(mi_size_chunk, plane);
                    let num4x4_w = NUM_4X4_BLOCKS_WIDE[plane_sz] as usize;
                    let num4x4_h = NUM_4X4_BLOCKS_HIGH[plane_sz] as usize;
                    let sub_x = if plane > 0 { self.fs.ss_x } else { 0 };
                    let sub_y = if plane > 0 { self.fs.ss_y } else { 0 };
                    if self.is_inter && !self.lossless && plane == 0 {
                        let base_x = (mi_col_chunk >> sub_x) * MI_SIZE;
                        let base_y = (mi_row_chunk >> sub_y) * MI_SIZE;
                        self.transform_tree(base_x, base_y, num4x4_w * 4, num4x4_h * 4)?;
                    } else {
                        let base_x_block = (self.mi_col >> sub_x) * MI_SIZE;
                        let base_y_block = (self.mi_row >> sub_y) * MI_SIZE;
                        let mut y = 0;
                        while y < num4x4_h {
                            let mut x = 0;
                            while x < num4x4_w {
                                self.transform_block(
                                    plane,
                                    base_x_block,
                                    base_y_block,
                                    tx_sz,
                                    x + ((chunk_x << 4) >> sub_x),
                                    y + ((chunk_y << 4) >> sub_y),
                                )?;
                                x += step_x;
                            }
                            y += step_y;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// transform_tree( startX, startY, w, h ) (§5.11.36)
    pub(crate) fn transform_tree(&mut self, start_x: usize, start_y: usize, w: usize, h: usize) -> Result<()> {
        let max_x = self.fs.mi_cols * MI_SIZE;
        let max_y = self.fs.mi_rows * MI_SIZE;
        if start_x >= max_x || start_y >= max_y {
            return Ok(());
        }
        let row = start_y >> MI_SIZE_LOG2;
        let col = start_x >> MI_SIZE_LOG2;
        let luma_tx_sz = self.fs.inter_tx_sizes[self.fs.mi(row, col)] as usize;
        let luma_w = TX_WIDTH[luma_tx_sz] as usize;
        let luma_h = TX_HEIGHT[luma_tx_sz] as usize;
        if w <= luma_w && h <= luma_h {
            let tx_sz = find_tx_size(w, h);
            self.transform_block(0, start_x, start_y, tx_sz, 0, 0)?;
        } else if w > h {
            self.transform_tree(start_x, start_y, w / 2, h)?;
            self.transform_tree(start_x + w / 2, start_y, w / 2, h)?;
        } else if w < h {
            self.transform_tree(start_x, start_y, w, h / 2)?;
            self.transform_tree(start_x, start_y + h / 2, w, h / 2)?;
        } else {
            self.transform_tree(start_x, start_y, w / 2, h / 2)?;
            self.transform_tree(start_x + w / 2, start_y, w / 2, h / 2)?;
            self.transform_tree(start_x, start_y + h / 2, w / 2, h / 2)?;
            self.transform_tree(start_x + w / 2, start_y + h / 2, w / 2, h / 2)?;
        }
        Ok(())
    }

    pub(crate) fn transform_block(&mut self, plane: usize, base_x: usize, base_y: usize, tx_sz: usize, x: usize, y: usize) -> Result<()> {
        let start_x = base_x + 4 * x;
        let start_y = base_y + 4 * y;
        let sub_x = if plane > 0 { self.fs.ss_x } else { 0 };
        let sub_y = if plane > 0 { self.fs.ss_y } else { 0 };
        let row = (start_y << sub_y) >> MI_SIZE_LOG2;
        let col = (start_x << sub_x) >> MI_SIZE_LOG2;
        let sb_mask = if self.seq.use_128x128_superblock { 31 } else { 15 };
        let sub_block_mi_row = row & sb_mask;
        let sub_block_mi_col = col & sb_mask;
        let step_x = TX_WIDTH[tx_sz] as usize >> MI_SIZE_LOG2;
        let step_y = TX_HEIGHT[tx_sz] as usize >> MI_SIZE_LOG2;
        let max_x = (self.fs.mi_cols * MI_SIZE) >> sub_x;
        let max_y = (self.fs.mi_rows * MI_SIZE) >> sub_y;
        if start_x >= max_x || start_y >= max_y {
            return Ok(());
        }
        if self.is_inter {
            // predicted by compute_prediction()
        } else if (plane == 0 && self.palette_size_y != 0) || (plane != 0 && self.palette_size_uv != 0) {
            self.predict_palette(plane, start_x, start_y, x, y, tx_sz);
        } else {
            let is_cfl = plane > 0 && self.uv_mode == UV_CFL_PRED;
            let mode = if plane == 0 { self.y_mode } else if is_cfl { DC_PRED } else { self.uv_mode };
            let log2w = TX_WIDTH_LOG2[tx_sz] as u32;
            let log2h = TX_HEIGHT_LOG2[tx_sz] as u32;
            let have_left = (if plane == 0 { self.avail_l } else { self.avail_l_chroma }) || x > 0;
            let have_above = (if plane == 0 { self.avail_u } else { self.avail_u_chroma }) || y > 0;
            let bry = (sub_block_mi_row >> sub_y) as isize;
            let brx = (sub_block_mi_col >> sub_x) as isize;
            let have_above_right = self.bd(plane, bry - 1, brx + step_x as isize);
            let have_below_left = self.bd(plane, bry + step_y as isize, brx - 1);
            self.predict_intra(plane, start_x, start_y, have_left, have_above, have_above_right, have_below_left, mode, log2w, log2h);
            if is_cfl {
                self.predict_chroma_from_luma(plane, start_x, start_y, tx_sz);
            }
        }
        if plane == 0 && !self.is_inter {
            self.max_luma_w = start_x + step_x * 4;
            self.max_luma_h = start_y + step_y * 4;
        }
        if !self.skip {
            let eob = self.coeffs(plane, start_x, start_y, tx_sz);
            self.fs.stats.tx_sizes[tx_sz] += 1;
            if eob > 0 {
                self.fs.stats.nonzero_tx_blocks += 1;
                self.fs.stats.tx_types[self.plane_tx_type] += 1;
                self.reconstruct(plane, start_x, start_y, tx_sz);
            }
        }
        for i in 0..step_y {
            for j in 0..step_x {
                let rr = (row >> sub_y) + i;
                let cc = (col >> sub_x) + j;
                if rr < self.fs.mi_rows && cc < self.fs.mi_cols {
                    let k = rr * self.fs.mi_cols + cc;
                    self.fs.lf_tx_sizes[plane][k] = tx_sz as u8;
                }
                let by = (sub_block_mi_row >> sub_y) + i;
                let bx = (sub_block_mi_col >> sub_x) + j;
                if by + 1 < 34 && bx + 1 < 34 {
                    self.block_decoded[plane][by + 1][bx + 1] = true;
                }
            }
        }
        Ok(())
    }

    /// get_tx_set( txSz ) (§5.11.48)
    pub(crate) fn get_tx_set(&self, tx_sz: usize) -> usize {
        let tx_sz_sqr = TX_SIZE_SQR[tx_sz] as usize;
        let tx_sz_sqr_up = TX_SIZE_SQR_UP[tx_sz] as usize;
        if tx_sz_sqr_up > TX_32X32 {
            return TX_SET_DCTONLY;
        }
        if self.is_inter {
            if self.hdr.reduced_tx_set || tx_sz_sqr_up == TX_32X32 {
                TX_SET_INTER_3
            } else if tx_sz_sqr == TX_16X16 {
                TX_SET_INTER_2
            } else {
                TX_SET_INTER_1
            }
        } else if tx_sz_sqr_up == TX_32X32 {
            TX_SET_DCTONLY
        } else if self.hdr.reduced_tx_set || tx_sz_sqr == TX_16X16 {
            TX_SET_INTRA_2
        } else {
            TX_SET_INTRA_1
        }
    }

    /// transform_type( x4, y4, txSz ) (§5.11.47)
    pub(crate) fn transform_type(&mut self, x4: usize, y4: usize, tx_sz: usize) {
        let set = self.get_tx_set(tx_sz);
        let q = if self.hdr.segmentation_enabled {
            self.hdr.get_qindex(true, self.segment_id, self.current_q_index)
        } else {
            self.hdr.base_q_idx as i32
        };
        let tx_type = if set > 0 && q > 0 {
            let sqr = TX_SIZE_SQR[tx_sz] as usize;
            if self.is_inter {
                if set == TX_SET_INTER_1 {
                    let v = sym!(self, self.cdf.inter_tx_type_set1[sqr]);
                    TX_TYPE_INTER_INV_SET1[v] as usize
                } else if set == TX_SET_INTER_2 {
                    let v = sym!(self, self.cdf.inter_tx_type_set2);
                    TX_TYPE_INTER_INV_SET2[v] as usize
                } else {
                    let v = sym!(self, self.cdf.inter_tx_type_set3[sqr]);
                    TX_TYPE_INTER_INV_SET3[v] as usize
                }
            } else {
                let intra_dir = if self.use_filter_intra { FILTER_INTRA_MODE_TO_INTRA_DIR[self.filter_intra_mode] as usize } else { self.y_mode };
                if set == TX_SET_INTRA_1 {
                    let v = sym!(self, self.cdf.intra_tx_type_set1[sqr][intra_dir]);
                    TX_TYPE_INTRA_INV_SET1[v] as usize
                } else {
                    let v = sym!(self, self.cdf.intra_tx_type_set2[sqr][intra_dir]);
                    TX_TYPE_INTRA_INV_SET2[v] as usize
                }
            }
        } else {
            DCT_DCT
        };
        self.set_tx_types(x4, y4, tx_sz, tx_type);
    }

    pub(crate) fn set_tx_types(&mut self, x4: usize, y4: usize, tx_sz: usize, tx_type: usize) {
        for i in 0..(TX_WIDTH[tx_sz] as usize >> 2) {
            for j in 0..(TX_HEIGHT[tx_sz] as usize >> 2) {
                let (yy, xx) = (y4 + j, x4 + i);
                if yy < self.fs.mi_rows && xx < self.fs.mi_cols {
                    let k = self.fs.mi(yy, xx);
                    self.fs.tx_types[k] = tx_type as u8;
                }
            }
        }
    }

    /// compute_tx_type( plane, txSz, blockX, blockY ) (§5.11.40)
    pub(crate) fn compute_tx_type(&self, plane: usize, tx_sz: usize, block_x: usize, block_y: usize) -> usize {
        let tx_sz_sqr_up = TX_SIZE_SQR_UP[tx_sz] as usize;
        if self.lossless || tx_sz_sqr_up > TX_32X32 {
            return DCT_DCT;
        }
        let tx_set = self.get_tx_set(tx_sz);
        if plane == 0 {
            return self.fs.tx_types[self.fs.mi(block_y, block_x)] as usize;
        }
        if self.is_inter {
            let x4 = self.mi_col.max(block_x << self.fs.ss_x);
            let y4 = self.mi_row.max(block_y << self.fs.ss_y);
            let tx_type = self.fs.tx_types[self.fs.mi(y4, x4)] as usize;
            if TX_TYPE_IN_SET_INTER[tx_set][tx_type] == 0 {
                return DCT_DCT;
            }
            return tx_type;
        }
        let tx_type = MODE_TO_TXFM[self.uv_mode] as usize;
        if TX_TYPE_IN_SET_INTRA[tx_set][tx_type] == 0 {
            return DCT_DCT;
        }
        tx_type
    }

    fn get_scan(&self, tx_sz: usize) -> Scan {
        if tx_sz == TX_16X64 {
            return Scan::W(&DEFAULT_SCAN_16X32);
        }
        if tx_sz == TX_64X16 {
            return Scan::W(&DEFAULT_SCAN_32X16);
        }
        if TX_SIZE_SQR_UP[tx_sz] as usize == TX_64X64 {
            return Scan::W(&DEFAULT_SCAN_32X32);
        }
        let t = self.plane_tx_type;
        if t == IDTX {
            return get_default_scan(tx_sz);
        }
        let prefer_row = t == V_DCT || t == V_ADST || t == V_FLIPADST;
        let prefer_col = t == H_DCT || t == H_ADST || t == H_FLIPADST;
        if prefer_row {
            Scan::B(get_mrow_scan(tx_sz))
        } else if prefer_col {
            Scan::B(get_mcol_scan(tx_sz))
        } else {
            get_default_scan(tx_sz)
        }
    }

    pub(crate) fn get_coeff_base_ctx(&self, tx_sz: usize, tx_class: usize, pos: usize, c: usize, is_eob: bool) -> usize {
        let adj = ADJUSTED_TX_SIZE[tx_sz] as usize;
        let bwl = TX_WIDTH_LOG2[adj] as usize;
        let width = 1usize << bwl;
        let height = TX_HEIGHT[adj] as usize;
        if is_eob {
            if c == 0 {
                return SIG_COEF_CONTEXTS - 4;
            }
            if c <= (height << bwl) / 8 {
                return SIG_COEF_CONTEXTS - 3;
            }
            if c <= (height << bwl) / 4 {
                return SIG_COEF_CONTEXTS - 2;
            }
            return SIG_COEF_CONTEXTS - 1;
        }
        let row = pos >> bwl;
        let col = pos - (row << bwl);
        let mut mag = 0i32;
        for idx in 0..SIG_REF_DIFF_OFFSET_NUM {
            let ref_row = row + SIG_REF_DIFF_OFFSET[tx_class][idx][0] as usize;
            let ref_col = col + SIG_REF_DIFF_OFFSET[tx_class][idx][1] as usize;
            if ref_row < height && ref_col < width {
                mag += self.quant[(ref_row << bwl) + ref_col].abs().min(3);
            }
        }
        let ctx = ((mag + 1) >> 1).min(4) as usize;
        if tx_class == TX_CLASS_2D {
            if row == 0 && col == 0 {
                return 0;
            }
            return ctx + COEFF_BASE_CTX_OFFSET[tx_sz][row.min(4)][col.min(4)] as usize;
        }
        let idx = if tx_class == TX_CLASS_VERT { row } else { col };
        ctx + COEFF_BASE_POS_CTX_OFFSET[idx.min(2)] as usize
    }

    pub(crate) fn get_coeff_br_ctx(&self, tx_sz: usize, tx_class: usize, pos: usize) -> usize {
        let adj = ADJUSTED_TX_SIZE[tx_sz] as usize;
        let bwl = TX_WIDTH_LOG2[adj] as usize;
        let txw = TX_WIDTH[adj] as usize;
        let txh = TX_HEIGHT[adj] as usize;
        let row = pos >> bwl;
        let col = pos - (row << bwl);
        let mut mag = 0i32;
        for idx in 0..3 {
            let ref_row = row + MAG_REF_OFFSET_WITH_TX_CLASS[tx_class][idx][0] as usize;
            let ref_col = col + MAG_REF_OFFSET_WITH_TX_CLASS[tx_class][idx][1] as usize;
            if ref_row < txh && ref_col < (1 << bwl) {
                mag += self.quant[ref_row * txw + ref_col].min((COEFF_BASE_RANGE + NUM_BASE_LEVELS + 1) as i32);
            }
        }
        let mag = ((mag + 1) >> 1).min(6) as usize;
        if pos == 0 {
            mag
        } else if tx_class == 0 {
            if row < 2 && col < 2 { mag + 7 } else { mag + 14 }
        } else if tx_class == 1 {
            if col == 0 { mag + 7 } else { mag + 14 }
        } else if row == 0 {
            mag + 7
        } else {
            mag + 14
        }
    }

    /// coeffs( plane, startX, startY, txSz ) — returns eob.
    pub(crate) fn coeffs(&mut self, plane: usize, start_x: usize, start_y: usize, tx_sz: usize) -> usize {
        let x4 = start_x >> 2;
        let y4 = start_y >> 2;
        let w4 = TX_WIDTH[tx_sz] as usize >> 2;
        let h4 = TX_HEIGHT[tx_sz] as usize >> 2;
        let tx_sz_ctx = (TX_SIZE_SQR[tx_sz] as usize + TX_SIZE_SQR_UP[tx_sz] as usize + 1) >> 1;
        let ptype = (plane > 0) as usize;
        let seg_eob = if tx_sz == TX_16X64 || tx_sz == TX_64X16 { 512 } else { (TX_WIDTH[tx_sz] as usize * TX_HEIGHT[tx_sz] as usize).min(1024) };
        for c in 0..seg_eob {
            self.quant[c] = 0;
        }
        let mut eob = 0usize;
        let mut cul_level = 0u32;
        let mut dc_category = 0u8;
        // all_zero ctx
        let mut max_x4 = self.fs.mi_cols;
        let mut max_y4 = self.fs.mi_rows;
        if plane > 0 {
            max_x4 >>= self.fs.ss_x;
            max_y4 >>= self.fs.ss_y;
        }
        let w = TX_WIDTH[tx_sz] as usize;
        let h = TX_HEIGHT[tx_sz] as usize;
        let ctx = {
            let bsize = self.get_plane_residual_size(self.mi_size, plane);
            let bw = block_width(bsize);
            let bh = block_height(bsize);
            if plane == 0 {
                let mut top = 0u32;
                let mut left = 0u32;
                for k in 0..w4 {
                    if x4 + k < max_x4 {
                        top = top.max(self.above_level_ctx[plane][x4 + k] as u32);
                    }
                }
                for k in 0..h4 {
                    if y4 + k < max_y4 {
                        left = left.max(self.left_level_ctx[plane][y4 + k] as u32);
                    }
                }
                let top = top.min(255);
                let left = left.min(255);
                if bw == w && bh == h {
                    0
                } else if top == 0 && left == 0 {
                    1
                } else if top == 0 || left == 0 {
                    2 + (top.max(left) > 3) as usize
                } else if top.max(left) <= 3 {
                    4
                } else if top.min(left) <= 3 {
                    5
                } else {
                    6
                }
            } else {
                let mut above = 0u8;
                let mut left = 0u8;
                for i in 0..w4 {
                    if x4 + i < max_x4 {
                        above |= self.above_level_ctx[plane][x4 + i];
                        above |= self.above_dc_ctx[plane][x4 + i];
                    }
                }
                for i in 0..h4 {
                    if y4 + i < max_y4 {
                        left |= self.left_level_ctx[plane][y4 + i];
                        left |= self.left_dc_ctx[plane][y4 + i];
                    }
                }
                let mut ctx = (above != 0) as usize + (left != 0) as usize;
                ctx += 7;
                if bw * bh > w * h {
                    ctx += 3;
                }
                ctx
            }
        };
        let all_zero = sym!(self, self.cdf.txb_skip[tx_sz_ctx][ctx]) != 0;
        if all_zero {
            if plane == 0 {
                self.set_tx_types(x4, y4, tx_sz, DCT_DCT);
            }
            self.plane_tx_type = DCT_DCT;
        } else {
            if plane == 0 {
                self.transform_type(x4, y4, tx_sz);
            }
            self.plane_tx_type = self.compute_tx_type(plane, tx_sz, x4, y4);
            let tx_class = get_tx_class(self.plane_tx_type);
            let scan = self.get_scan(tx_sz);
            let eob_multisize = (TX_WIDTH_LOG2[tx_sz] as usize).min(5) + (TX_HEIGHT_LOG2[tx_sz] as usize).min(5) - 4;
            let ectx = if tx_class == TX_CLASS_2D { 0 } else { 1 };
            let eob_pt = 1 + match eob_multisize {
                0 => sym!(self, self.cdf.eob_pt_16[ptype][ectx]),
                1 => sym!(self, self.cdf.eob_pt_32[ptype][ectx]),
                2 => sym!(self, self.cdf.eob_pt_64[ptype][ectx]),
                3 => sym!(self, self.cdf.eob_pt_128[ptype][ectx]),
                4 => sym!(self, self.cdf.eob_pt_256[ptype][ectx]),
                5 => sym!(self, self.cdf.eob_pt_512[ptype]),
                _ => sym!(self, self.cdf.eob_pt_1024[ptype]),
            };
            eob = if eob_pt < 2 { eob_pt } else { (1 << (eob_pt - 2)) + 1 };
            let eob_shift = eob_pt as i32 - 3;
            if eob_shift >= 0 {
                let eob_extra = sym!(self, self.cdf.eob_extra[tx_sz_ctx][ptype][eob_pt - 3]);
                if eob_extra != 0 {
                    eob += 1 << eob_shift;
                }
                let lim = (eob_pt as i32 - 2).max(0) as usize;
                for i in 1..lim {
                    let eob_shift = lim - 1 - i;
                    if self.sd.read_literal(1) != 0 {
                        eob += 1 << eob_shift;
                    }
                }
            }
            for c in (0..eob).rev() {
                let pos = scan.at(c);
                let mut level;
                if c == eob - 1 {
                    let cctx = self.get_coeff_base_ctx(tx_sz, tx_class, pos, c, true) - SIG_COEF_CONTEXTS + SIG_COEF_CONTEXTS_EOB;
                    level = sym!(self, self.cdf.coeff_base_eob[tx_sz_ctx][ptype][cctx]) as i32 + 1;
                } else {
                    let cctx = self.get_coeff_base_ctx(tx_sz, tx_class, pos, c, false);
                    level = sym!(self, self.cdf.coeff_base[tx_sz_ctx][ptype][cctx]) as i32;
                }
                if level > NUM_BASE_LEVELS as i32 {
                    let bctx = self.get_coeff_br_ctx(tx_sz, tx_class, pos);
                    let bsz = tx_sz_ctx.min(TX_32X32);
                    for _ in 0..(COEFF_BASE_RANGE / (BR_CDF_SIZE - 1)) {
                        let coeff_br = sym!(self, self.cdf.coeff_br[bsz][ptype][bctx]) as i32;
                        level += coeff_br;
                        if coeff_br < (BR_CDF_SIZE - 1) as i32 {
                            break;
                        }
                    }
                }
                self.quant[pos] = level;
            }
            for c in 0..eob {
                let pos = scan.at(c);
                let mut sign = 0;
                if self.quant[pos] != 0 {
                    if c == 0 {
                        // dc_sign ctx
                        let mut dc_sign: i32 = 0;
                        for k in 0..w4 {
                            if x4 + k < max_x4 {
                                let s = self.above_dc_ctx[plane][x4 + k];
                                if s == 1 {
                                    dc_sign -= 1;
                                } else if s == 2 {
                                    dc_sign += 1;
                                }
                            }
                        }
                        for k in 0..h4 {
                            if y4 + k < max_y4 {
                                let s = self.left_dc_ctx[plane][y4 + k];
                                if s == 1 {
                                    dc_sign -= 1;
                                } else if s == 2 {
                                    dc_sign += 1;
                                }
                            }
                        }
                        let dctx = if dc_sign < 0 { 1 } else if dc_sign > 0 { 2 } else { 0 };
                        sign = sym!(self, self.cdf.dc_sign[ptype][dctx]);
                    } else {
                        sign = self.sd.read_literal(1) as usize;
                    }
                }
                if self.quant[pos] > (NUM_BASE_LEVELS + COEFF_BASE_RANGE) as i32 {
                    let mut length = 0;
                    loop {
                        length += 1;
                        let b = self.sd.read_literal(1);
                        if b != 0 || length >= 32 {
                            break;
                        }
                    }
                    let mut x: u32 = 1;
                    let mut i = length as i32 - 2;
                    while i >= 0 {
                        x = (x << 1) | self.sd.read_literal(1);
                        i -= 1;
                    }
                    self.quant[pos] = (x as i64 + COEFF_BASE_RANGE as i64 + NUM_BASE_LEVELS as i64).min(i32::MAX as i64) as i32;
                }
                if pos == 0 && self.quant[pos] > 0 {
                    dc_category = if sign != 0 { 1 } else { 2 };
                }
                self.quant[pos] &= 0xFFFFF;
                cul_level += self.quant[pos] as u32;
                if sign != 0 {
                    self.quant[pos] = -self.quant[pos];
                }
            }
            cul_level = cul_level.min(63);
        }
        for i in 0..w4 {
            self.above_level_ctx[plane][x4 + i] = cul_level as u8;
            self.above_dc_ctx[plane][x4 + i] = dc_category;
        }
        for i in 0..h4 {
            self.left_level_ctx[plane][y4 + i] = cul_level as u8;
            self.left_dc_ctx[plane][y4 + i] = dc_category;
        }
        eob
    }
}

/// inverse_recenter (§5.9.28)
pub fn inverse_recenter(r: i32, v: i32) -> i32 {
    if v > 2 * r {
        v
    } else if v & 1 != 0 {
        r - ((v + 1) >> 1)
    } else {
        r + (v >> 1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    pub(crate) fn neg_deinterleave_kat() {
        // ref 0 -> identity; ref = max-1 -> mirrored
        assert_eq!(neg_deinterleave(3, 0, 8), 3);
        assert_eq!(neg_deinterleave(0, 7, 8), 7);
        assert_eq!(neg_deinterleave(1, 3, 8), 4);
        assert_eq!(neg_deinterleave(2, 3, 8), 2);
    }
    #[test]
    pub(crate) fn inverse_recenter_kat() {
        assert_eq!(inverse_recenter(5, 11), 11);
        assert_eq!(inverse_recenter(5, 1), 4);
        assert_eq!(inverse_recenter(5, 2), 6);
    }
}
