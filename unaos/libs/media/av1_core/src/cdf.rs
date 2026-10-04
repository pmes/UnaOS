//! The CDF context of one tile (§8.2.2 "a copy is made of each of the CDF arrays ... prefixed with
//! Tile") — the arrays used by intra frames. Initialised from the spec's default tables, which are
//! generated into `tables.rs`: init_non_coeff_cdfs() and init_coeff_cdfs() (§6.8.2).
//!
//! The inter-only CDFs (motion vectors, reference frames, compound, OBMC, interp filter, ...)
//! are owed with inter prediction.

use crate::tables::*;

#[derive(Clone)]
pub struct CdfContext {
    pub intra_frame_y_mode: [[[u16; 14]; 5]; 5],
    pub uv_mode_cfl_not_allowed: [[u16; 14]; 13],
    pub uv_mode_cfl_allowed: [[u16; 15]; 13],
    pub angle_delta: [[u16; 8]; 8],
    pub intrabc: [u16; 3],
    pub partition_w8: [[u16; 5]; 4],
    pub partition_w16: [[u16; 11]; 4],
    pub partition_w32: [[u16; 11]; 4],
    pub partition_w64: [[u16; 11]; 4],
    pub partition_w128: [[u16; 9]; 4],
    pub segment_id: [[u16; 9]; 3],
    pub segment_id_predicted: [[u16; 3]; 3],
    pub tx_8x8: [[u16; 3]; 3],
    pub tx_16x16: [[u16; 4]; 3],
    pub tx_32x32: [[u16; 4]; 3],
    pub tx_64x64: [[u16; 4]; 3],
    pub txfm_split: [[u16; 3]; 21],
    pub filter_intra_mode: [u16; 6],
    pub filter_intra: [[u16; 3]; 22],
    pub skip: [[u16; 3]; 3],
    pub palette_y_mode: [[[u16; 3]; 3]; 7],
    pub palette_uv_mode: [[u16; 3]; 2],
    pub palette_y_size: [[u16; 8]; 7],
    pub palette_uv_size: [[u16; 8]; 7],
    pub palette_size_2_y_color: [[u16; 3]; 5],
    pub palette_size_3_y_color: [[u16; 4]; 5],
    pub palette_size_4_y_color: [[u16; 5]; 5],
    pub palette_size_5_y_color: [[u16; 6]; 5],
    pub palette_size_6_y_color: [[u16; 7]; 5],
    pub palette_size_7_y_color: [[u16; 8]; 5],
    pub palette_size_8_y_color: [[u16; 9]; 5],
    pub palette_size_2_uv_color: [[u16; 3]; 5],
    pub palette_size_3_uv_color: [[u16; 4]; 5],
    pub palette_size_4_uv_color: [[u16; 5]; 5],
    pub palette_size_5_uv_color: [[u16; 6]; 5],
    pub palette_size_6_uv_color: [[u16; 7]; 5],
    pub palette_size_7_uv_color: [[u16; 8]; 5],
    pub palette_size_8_uv_color: [[u16; 9]; 5],
    pub delta_q: [u16; 5],
    pub delta_lf: [u16; 5],
    pub delta_lf_multi: [[u16; 5]; 4],
    pub intra_tx_type_set1: [[[u16; 8]; 13]; 2],
    pub intra_tx_type_set2: [[[u16; 6]; 13]; 3],
    pub cfl_sign: [u16; 9],
    pub cfl_alpha: [[u16; 17]; 6],
    pub use_wiener: [u16; 3],
    pub use_sgrproj: [u16; 3],
    pub restoration_type: [u16; 4],
    // coefficient cdfs (init_coeff_cdfs, indexed by the base_q_idx context)
    pub txb_skip: [[[u16; 3]; 13]; 5],
    pub eob_pt_16: [[[u16; 6]; 2]; 2],
    pub eob_pt_32: [[[u16; 7]; 2]; 2],
    pub eob_pt_64: [[[u16; 8]; 2]; 2],
    pub eob_pt_128: [[[u16; 9]; 2]; 2],
    pub eob_pt_256: [[[u16; 10]; 2]; 2],
    pub eob_pt_512: [[u16; 11]; 2],
    pub eob_pt_1024: [[u16; 12]; 2],
    pub eob_extra: [[[[u16; 3]; 9]; 2]; 5],
    pub dc_sign: [[[u16; 3]; 3]; 2],
    pub coeff_base_eob: [[[[u16; 4]; 4]; 2]; 5],
    pub coeff_base: [[[[u16; 5]; 42]; 2]; 5],
    pub coeff_br: [[[[u16; 5]; 21]; 2]; 5],
}

impl CdfContext {
    /// init_non_coeff_cdfs() followed by init_coeff_cdfs() for `base_q_idx`.
    pub fn new(base_q_idx: u32) -> CdfContext {
        let idx = if base_q_idx <= 20 {
            0
        } else if base_q_idx <= 60 {
            1
        } else if base_q_idx <= 120 {
            2
        } else {
            3
        };
        CdfContext {
            intra_frame_y_mode: DEFAULT_INTRA_FRAME_Y_MODE_CDF,
            uv_mode_cfl_not_allowed: DEFAULT_UV_MODE_CFL_NOT_ALLOWED_CDF,
            uv_mode_cfl_allowed: DEFAULT_UV_MODE_CFL_ALLOWED_CDF,
            angle_delta: DEFAULT_ANGLE_DELTA_CDF,
            intrabc: DEFAULT_INTRABC_CDF,
            partition_w8: DEFAULT_PARTITION_W8_CDF,
            partition_w16: DEFAULT_PARTITION_W16_CDF,
            partition_w32: DEFAULT_PARTITION_W32_CDF,
            partition_w64: DEFAULT_PARTITION_W64_CDF,
            partition_w128: DEFAULT_PARTITION_W128_CDF,
            segment_id: DEFAULT_SEGMENT_ID_CDF,
            segment_id_predicted: DEFAULT_SEGMENT_ID_PREDICTED_CDF,
            tx_8x8: DEFAULT_TX_8X8_CDF,
            tx_16x16: DEFAULT_TX_16X16_CDF,
            tx_32x32: DEFAULT_TX_32X32_CDF,
            tx_64x64: DEFAULT_TX_64X64_CDF,
            txfm_split: DEFAULT_TXFM_SPLIT_CDF,
            filter_intra_mode: DEFAULT_FILTER_INTRA_MODE_CDF,
            filter_intra: DEFAULT_FILTER_INTRA_CDF,
            skip: DEFAULT_SKIP_CDF,
            palette_y_mode: DEFAULT_PALETTE_Y_MODE_CDF,
            palette_uv_mode: DEFAULT_PALETTE_UV_MODE_CDF,
            palette_y_size: DEFAULT_PALETTE_Y_SIZE_CDF,
            palette_uv_size: DEFAULT_PALETTE_UV_SIZE_CDF,
            palette_size_2_y_color: DEFAULT_PALETTE_SIZE_2_Y_COLOR_CDF,
            palette_size_3_y_color: DEFAULT_PALETTE_SIZE_3_Y_COLOR_CDF,
            palette_size_4_y_color: DEFAULT_PALETTE_SIZE_4_Y_COLOR_CDF,
            palette_size_5_y_color: DEFAULT_PALETTE_SIZE_5_Y_COLOR_CDF,
            palette_size_6_y_color: DEFAULT_PALETTE_SIZE_6_Y_COLOR_CDF,
            palette_size_7_y_color: DEFAULT_PALETTE_SIZE_7_Y_COLOR_CDF,
            palette_size_8_y_color: DEFAULT_PALETTE_SIZE_8_Y_COLOR_CDF,
            palette_size_2_uv_color: DEFAULT_PALETTE_SIZE_2_UV_COLOR_CDF,
            palette_size_3_uv_color: DEFAULT_PALETTE_SIZE_3_UV_COLOR_CDF,
            palette_size_4_uv_color: DEFAULT_PALETTE_SIZE_4_UV_COLOR_CDF,
            palette_size_5_uv_color: DEFAULT_PALETTE_SIZE_5_UV_COLOR_CDF,
            palette_size_6_uv_color: DEFAULT_PALETTE_SIZE_6_UV_COLOR_CDF,
            palette_size_7_uv_color: DEFAULT_PALETTE_SIZE_7_UV_COLOR_CDF,
            palette_size_8_uv_color: DEFAULT_PALETTE_SIZE_8_UV_COLOR_CDF,
            delta_q: DEFAULT_DELTA_Q_CDF,
            delta_lf: DEFAULT_DELTA_LF_CDF,
            delta_lf_multi: [DEFAULT_DELTA_LF_CDF; 4],
            intra_tx_type_set1: DEFAULT_INTRA_TX_TYPE_SET1_CDF,
            intra_tx_type_set2: DEFAULT_INTRA_TX_TYPE_SET2_CDF,
            cfl_sign: DEFAULT_CFL_SIGN_CDF,
            cfl_alpha: DEFAULT_CFL_ALPHA_CDF,
            use_wiener: DEFAULT_USE_WIENER_CDF,
            use_sgrproj: DEFAULT_USE_SGRPROJ_CDF,
            restoration_type: DEFAULT_RESTORATION_TYPE_CDF,
            txb_skip: DEFAULT_TXB_SKIP_CDF[idx],
            eob_pt_16: DEFAULT_EOB_PT_16_CDF[idx],
            eob_pt_32: DEFAULT_EOB_PT_32_CDF[idx],
            eob_pt_64: DEFAULT_EOB_PT_64_CDF[idx],
            eob_pt_128: DEFAULT_EOB_PT_128_CDF[idx],
            eob_pt_256: DEFAULT_EOB_PT_256_CDF[idx],
            eob_pt_512: DEFAULT_EOB_PT_512_CDF[idx],
            eob_pt_1024: DEFAULT_EOB_PT_1024_CDF[idx],
            eob_extra: DEFAULT_EOB_EXTRA_CDF[idx],
            dc_sign: DEFAULT_DC_SIGN_CDF[idx],
            coeff_base_eob: DEFAULT_COEFF_BASE_EOB_CDF[idx],
            coeff_base: DEFAULT_COEFF_BASE_CDF[idx],
            coeff_br: DEFAULT_COEFF_BR_CDF[idx],
        }
    }
}
